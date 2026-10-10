//! The PC's power key, in PS5 Launcher OS only: a press opens the Power menu, as a held PS button
//! does. On other PCs logind keeps the key.
//!
//! The order matters. The launcher first opens the "Power Button" input devices (the image's udev
//! rule lets the user read them). Only then does it ask logind for the `handle-power-key`
//! inhibitor lock, through `systemd-inhibit` running a small holder program. The holder writes `r`
//! when the lock is held, then waits for end of file on its stdin. Only the launcher holds the
//! write end of that pipe, so when the launcher exits or crashes, the holder ends and logind
//! handles the key again. Without the lock, the launcher never acts on the key: logind does.
//!
//! This reads its own devices on its own thread; the controller loop is in `gamepad`.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))] // the evdev code is Linux-only

use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const EV_KEY: u16 = 1;
const KEY_POWER: u16 = 116;
/// The highest key code (KEY_MAX): the size of the key capability bitmap.
const KEY_MAX: usize = 0x2ff;
/// One `struct input_event` on 64-bit Linux, as in `gamepad`.
const EVENT_SIZE: usize = 24;
/// The name the kernel gives the ACPI power buttons (LNXPWRBN and PNP0C0C).
const DEVICE_NAME: &str = "Power Button";
/// Installed by the image: writes `r` once the lock is held, then waits for EOF on stdin.
const HOLDER: &str = "/usr/libexec/ps5-launcher-os/power-key-hold";
/// How long the holder may take to say the lock is held.
const READY_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the holder may take to exit after its stdin closes, before it is killed.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

/// Whether the key capability bitmap from EVIOCGBIT has `code`. The kernel fills an array of
/// `unsigned long`; bits past the returned length are not set.
pub fn has_key(words: &[libc::c_ulong], code: u16) -> bool {
    let bits = libc::c_ulong::BITS as usize;
    let (word, bit) = (code as usize / bits, code as usize % bits);
    words.get(word).is_some_and(|w| w >> bit & 1 == 1)
}

/// A press of the power key. Key repeats (value 2) and releases (value 0) do not count.
pub fn is_press(typ: u16, code: u16, value: i32) -> bool {
    typ == EV_KEY && code == KEY_POWER && value == 1
}

/// How many power-key presses a read from an evdev device holds.
pub fn presses(buf: &[u8]) -> usize {
    buf.chunks_exact(EVENT_SIZE)
        .filter(|ev| {
            let typ = u16::from_ne_bytes([ev[16], ev[17]]);
            let code = u16::from_ne_bytes([ev[18], ev[19]]);
            let value = i32::from_ne_bytes([ev[20], ev[21], ev[22], ev[23]]);
            is_press(typ, code, value)
        })
        .count()
}

/// The `/dev/input/event*` nodes named "Power Button", from the sysfs input class at `root`
/// (`/sys/class/input`): `<root>/eventN/device/name`.
pub fn power_buttons(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut nodes: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name.strip_prefix("event").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())))
        .filter(|name| std::fs::read_to_string(root.join(name).join("device/name")).is_ok_and(|n| n.trim_end() == DEVICE_NAME))
        .map(|name| format!("/dev/input/{name}"))
        .collect();
    nodes.sort();
    nodes
}

/// The `handle-power-key` inhibitor lock, held by a child process. Dropping it closes the
/// child's stdin, which releases the lock.
pub struct Inhibitor {
    child: Child,
    stdin: Option<ChildStdin>,
    /// The child's stdout, kept open: it hangs up when the holder ends, so the lock is gone.
    stdout: ChildStdout,
}

impl Inhibitor {
    /// Start `cmd` and wait up to `timeout` for its readiness byte `r`. On any failure the child
    /// is killed and reaped, and the error says why.
    pub fn start(mut cmd: Command, timeout: Duration) -> Result<Inhibitor, String> {
        let program = cmd.get_program().to_string_lossy().into_owned();
        // std makes both pipes close-on-exec in the launcher (pipe2 with O_CLOEXEC): only this
        // child gets them, as its stdin and stdout.
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped());
        #[cfg(target_os = "linux")]
        die_with_parent(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("could not start {program}: {e}"))?;
        let stdin = child.stdin.take();
        let ready = child.stdout.take().ok_or_else(|| "no stdout".to_string()).and_then(|mut stdout| wait_ready(&mut stdout, timeout).map(|()| stdout));
        match ready {
            Ok(stdout) => Ok(Inhibitor { child, stdin, stdout }),
            Err(e) => {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
                Err(format!("{program}: {e}"))
            }
        }
    }

    /// The fd to poll: it hangs up when the holder ends.
    fn lost_fd(&self) -> libc::c_int {
        self.stdout.as_raw_fd()
    }
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + RELEASE_TIMEOUT;
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        crate::log!("power key: the inhibitor did not end; killing it");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait for the one byte `r` on `out`. A child that ends first fails at once.
fn wait_ready(out: &mut ChildStdout, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("not ready within {} s", timeout.as_secs_f32()));
        }
        let mut pfd = libc::pollfd { fd: out.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = left.as_millis().clamp(1, i32::MAX as u128) as libc::c_int;
        let r = unsafe { libc::poll(&mut pfd, 1, ms) };
        if r < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e.to_string());
        }
        if r == 0 {
            continue;
        }
        let mut byte = [0u8; 1];
        return match out.read(&mut byte) {
            Ok(1) if byte[0] == b'r' => Ok(()),
            Ok(0) => Err("ended before the lock was held".into()),
            Ok(_) => Err(format!("unexpected readiness byte {:?}", byte[0] as char)),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => Err(e.to_string()),
        };
    }
}

/// The child gets SIGTERM when the launcher dies. The pipe's EOF is the main guarantee; this is
/// a second one. Linux sends it when the *thread* that started the child ends, so the thread that
/// starts the inhibitor keeps it for its whole life (see `run`).
#[cfg(target_os = "linux")]
fn die_with_parent(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent = unsafe { libc::getpid() };
    // SAFETY: prctl and getppid are async-signal-safe, and nothing here allocates.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The launcher died before prctl: the signal will never come, so do not start.
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

/// `systemd-inhibit … power-key-hold`.
fn inhibit_command() -> Command {
    let mut cmd = Command::new("systemd-inhibit");
    cmd.args([
        "--what=handle-power-key",
        "--mode=block",
        "--no-ask-password",
        "--who=ps5-launcher",
        "--why=Launcher power menu",
        HOLDER,
    ]);
    cmd
}

/// The reader. Dropping it releases the lock and stops reading the key.
pub struct PowerKey {
    _wake: std::os::unix::net::UnixStream,
}

/// Start reading the power key on its own thread. `emit` runs for each press, on that thread.
#[cfg(target_os = "linux")]
pub fn spawn(emit: impl Fn() + Send + 'static) -> Option<PowerKey> {
    let (wake, wake_reader) = std::os::unix::net::UnixStream::pair()
        .map_err(|e| crate::log!("power key: {e}"))
        .ok()?;
    std::thread::Builder::new()
        .name("power-key".into())
        .spawn(move || run(emit, wake_reader))
        .map_err(|e| crate::log!("power key: {e}"))
        .ok()?;
    Some(PowerKey { _wake: wake })
}

/// Open a power-button device, and keep it only when it reports KEY_POWER. It is not grabbed.
#[cfg(target_os = "linux")]
fn open_power_button(path: &str) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;
    let c = std::ffi::CString::new(path).ok()?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        crate::log!("power key: cannot open {path}: {}", std::io::Error::last_os_error());
        return None;
    }
    // SAFETY: open() just returned this fd, and nothing else owns it.
    let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    let bits = libc::c_ulong::BITS as usize;
    let mut words: Vec<libc::c_ulong> = vec![0; KEY_MAX / bits + 1];
    let len = std::mem::size_of_val(words.as_slice());
    // EVIOCGBIT(EV_KEY, len) = _IOC(_IOC_READ, 'E', 0x20 + EV_KEY, len)
    let req = (2u64 << 30) | ((len as u64) << 16) | ((b'E' as u64) << 8) | (0x20 + EV_KEY as u64);
    let n = unsafe { libc::ioctl(fd.as_raw_fd(), req as _, words.as_mut_ptr()) };
    if n < 0 || !has_key(&words, KEY_POWER) {
        crate::log!("power key: {path} has no KEY_POWER");
        return None;
    }
    Some(fd)
}

/// Read everything queued on `fd`. Returns the number of presses, or None when the device is gone.
#[cfg(target_os = "linux")]
fn read_presses(fd: &std::os::fd::OwnedFd) -> Option<usize> {
    let mut buf = [0u8; EVENT_SIZE * 16];
    let mut count = 0;
    loop {
        let n = unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            count += presses(&buf[..n as usize]);
            continue;
        }
        let err = std::io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(libc::EAGAIN) if n < 0 => Some(count),
            Some(libc::EINTR) if n < 0 => continue,
            _ => None,
        };
    }
}

#[cfg(target_os = "linux")]
fn run(emit: impl Fn(), wake: std::os::unix::net::UnixStream) {
    let mut devices: Vec<(String, std::os::fd::OwnedFd)> = power_buttons(Path::new("/sys/class/input"))
        .into_iter()
        .filter_map(|path| open_power_button(&path).map(|fd| (path, fd)))
        .collect();
    if devices.is_empty() {
        crate::log!("power key: no power button to read; logind handles the key");
        return;
    }
    // This thread starts the inhibitor and keeps it until it returns (see `die_with_parent`).
    let inhibitor = match Inhibitor::start(inhibit_command(), READY_TIMEOUT) {
        Ok(inhibitor) => inhibitor,
        Err(e) => {
            crate::log!("power key: no inhibitor lock ({e}); logind handles the key");
            return;
        }
    };
    // Presses from before the lock went to logind: drop them.
    devices.retain(|(_, fd)| read_presses(fd).is_some());
    crate::log!("power key: the launcher handles the power key");
    while !devices.is_empty() {
        let mut fds = vec![
            libc::pollfd { fd: wake.as_raw_fd(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: inhibitor.lost_fd(), events: libc::POLLIN, revents: 0 },
        ];
        fds.extend(devices.iter().map(|(_, fd)| libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }));
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
        if r < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            crate::log!("power key: poll: {}", std::io::Error::last_os_error());
            break;
        }
        // The launcher is closing: release the lock.
        if fds[0].revents != 0 {
            break;
        }
        // The holder ended (someone killed it): logind has the key again, so stop acting on it.
        if fds[1].revents != 0 {
            crate::log!("power key: the inhibitor ended; logind handles the key");
            break;
        }
        let mut pressed = false;
        let mut gone = Vec::new();
        for (i, pfd) in fds[2..].iter().enumerate() {
            if pfd.revents == 0 {
                continue;
            }
            match read_presses(&devices[i].1) {
                Some(n) => pressed |= n > 0,
                None => gone.push(i),
            }
        }
        for i in gone.into_iter().rev() {
            crate::log!("power key: {} is gone", devices[i].0);
            devices.remove(i);
        }
        if pressed {
            emit();
        }
    }
    if devices.is_empty() {
        crate::log!("power key: no power button left; logind handles the key");
    }
    drop(inhibitor);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(codes: &[u16]) -> Vec<libc::c_ulong> {
        let bits = libc::c_ulong::BITS as usize;
        let mut words = vec![0 as libc::c_ulong; KEY_MAX / bits + 1];
        for &c in codes {
            words[c as usize / bits] |= 1 << (c as usize % bits);
        }
        words
    }

    #[test]
    fn the_capability_bitmap_must_have_key_power() {
        assert!(has_key(&bitmap(&[KEY_POWER]), KEY_POWER));
        assert!(has_key(&bitmap(&[1, KEY_POWER, 0x2ff]), KEY_POWER));
        assert!(!has_key(&bitmap(&[115, 117]), KEY_POWER), "volume keys, no power key");
        assert!(!has_key(&bitmap(&[]), KEY_POWER));
        assert!(!has_key(&[], KEY_POWER), "an empty answer");
        assert!(!has_key(&[libc::c_ulong::MAX], KEY_POWER), "a bitmap too short for code 116");
        assert!(has_key(&bitmap(&[0x2ff]), 0x2ff), "the last key code");
    }

    fn event(typ: u16, code: u16, value: i32) -> Vec<u8> {
        let mut ev = vec![0u8; 16];
        ev.extend(typ.to_ne_bytes());
        ev.extend(code.to_ne_bytes());
        ev.extend(value.to_ne_bytes());
        ev
    }

    #[test]
    fn only_presses_count_not_repeats_or_releases() {
        assert!(is_press(EV_KEY, KEY_POWER, 1));
        assert!(!is_press(EV_KEY, KEY_POWER, 2), "the kernel's key repeat");
        assert!(!is_press(EV_KEY, KEY_POWER, 0), "the release");
        assert!(!is_press(EV_KEY, 115, 1), "another key");
        assert!(!is_press(0, KEY_POWER, 1), "not a key event");

        let mut buf = Vec::new();
        for ev in [event(EV_KEY, KEY_POWER, 1), event(0, 0, 0), event(EV_KEY, KEY_POWER, 2), event(EV_KEY, KEY_POWER, 0), event(0, 0, 0)] {
            buf.extend(ev);
        }
        assert_eq!(presses(&buf), 1);
        buf.extend(event(EV_KEY, KEY_POWER, 1));
        assert_eq!(presses(&buf), 2);
        assert_eq!(presses(&buf[..EVENT_SIZE - 1]), 0, "a cut event is ignored");
    }

    #[test]
    fn power_buttons_are_found_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let add = |node: &str, name: &str| {
            let d = dir.path().join(node).join("device");
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("name"), format!("{name}\n")).unwrap();
        };
        add("event0", "Power Button");
        add("event1", "Sleep Button");
        add("event2", "AT Translated Set 2 keyboard");
        add("event11", "Power Button");
        add("mouse0", "Power Button");
        add("input3", "Power Button");
        std::fs::create_dir_all(dir.path().join("event5")).unwrap(); // no name file
        assert_eq!(power_buttons(dir.path()), ["/dev/input/event0", "/dev/input/event11"]);
        assert!(power_buttons(&dir.path().join("missing")).is_empty());
    }

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    fn wait_for(path: &Path, limit: Duration) -> bool {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        path.exists()
    }

    fn alive(pid: libc::pid_t) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn read_pid(path: &Path) -> libc::pid_t {
        std::fs::read_to_string(path).unwrap().trim().parse().unwrap()
    }

    #[test]
    fn the_holder_says_ready_and_ends_when_the_pipe_closes() {
        let dir = tempfile::tempdir().unwrap();
        let done = dir.path().join("done");
        let script = format!("printf r; cat >/dev/null; echo eof > '{}'", done.display());
        let inhibitor = Inhibitor::start(sh(&script), READY_TIMEOUT).expect("ready");
        std::thread::sleep(Duration::from_millis(100));
        assert!(!done.exists(), "the holder waits while the launcher keeps the pipe");
        drop(inhibitor);
        assert!(wait_for(&done, Duration::from_secs(2)), "the holder saw EOF on its stdin");
    }

    #[test]
    fn a_failed_inhibitor_falls_back_at_once() {
        let started = Instant::now();
        let err = Inhibitor::start(sh("echo 'Failed to inhibit: Access denied' >&2; exit 1"), READY_TIMEOUT).err().unwrap();
        assert!(err.contains("ended before the lock was held"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(2), "no need to wait the full timeout");
        let err = Inhibitor::start(Command::new("/nonexistent/systemd-inhibit"), READY_TIMEOUT).err().unwrap();
        assert!(err.contains("could not start"), "{err}");
        let err = Inhibitor::start(sh("printf x; exec sleep 30"), READY_TIMEOUT).err().unwrap();
        assert!(err.contains("unexpected readiness byte"), "{err}");
    }

    #[test]
    fn a_holder_that_never_says_ready_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!("echo $$ > '{}'; exec sleep 30", pid_file.display());
        let err = Inhibitor::start(sh(&script), Duration::from_millis(300)).err().unwrap();
        assert!(err.contains("not ready"), "{err}");
        let pid = read_pid(&pid_file);
        assert!(!alive(pid), "the holder was killed and reaped");
    }

    /// Set by `killing_the_owner_closes_the_pipe_and_the_holder_exits` for its child process.
    const OWNER_DIR: &str = "PS5L_POWER_KEY_OWNER_DIR";

    /// The launcher's side in a separate process: it takes the "lock", then waits to be killed.
    /// It does nothing unless the test below starts it.
    #[test]
    fn owner_process() {
        let Ok(dir) = std::env::var(OWNER_DIR) else { return };
        let dir = Path::new(&dir);
        // The holder ignores SIGTERM: only the end of its stdin can stop it.
        let script = format!(
            "trap '' TERM; echo $$ > '{}'; printf r; cat >/dev/null; echo eof > '{}'",
            dir.join("holder.pid").display(),
            dir.join("holder.done").display()
        );
        let _inhibitor = Inhibitor::start(sh(&script), READY_TIMEOUT).expect("ready");
        std::fs::write(dir.join("owner.ready"), "").unwrap();
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn killing_the_owner_closes_the_pipe_and_the_holder_exits() {
        let dir = tempfile::tempdir().unwrap();
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args(["power_key::tests::owner_process", "--exact", "--nocapture", "--test-threads=1"])
            .env(OWNER_DIR, dir.path())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        assert!(wait_for(&dir.path().join("owner.ready"), Duration::from_secs(10)), "the owner took the lock");
        let holder = read_pid(&dir.path().join("holder.pid"));
        assert!(alive(holder));
        // A crash: no destructor runs in the owner.
        owner.kill().unwrap();
        owner.wait().unwrap();
        // The holder writes this file as its last step, after `cat` saw EOF, and then exits. (It
        // is not ours to reap now, so checking its pid could see a zombie.)
        assert!(wait_for(&dir.path().join("holder.done"), Duration::from_secs(5)), "the holder saw EOF after the owner died");
    }
}
