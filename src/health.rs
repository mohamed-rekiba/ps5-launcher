//! The health socket: PS5 Launcher OS's boot health check asks the launcher whether its UI loop
//! runs. A client writes `ping\n`; the launcher answers `ok\n` only after a closure posted to the
//! UI event loop has run. When the loop is stuck, the client gets no answer.
//!
//! The UI thread never touches the socket: the posted closure only signals a channel, and the
//! connection's own thread writes the answer, with timeouts on every read and write.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))] // the launcher listens on Linux only

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

/// The socket's file name in the user's runtime folder.
const SOCKET_NAME: &str = "ps5-launcher-health.sock";
/// How long a connection's thread waits for the UI loop to run the closure. The client gives up
/// after the same time, so a later answer would reach nobody.
pub const UI_TIMEOUT: Duration = Duration::from_secs(2);
/// A connection with no new line for this long is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(10);
/// A client that does not read its answers for this long is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// The longest line a client may send.
const MAX_LINE: u64 = 64;

/// Runs a closure on the UI event loop. Returns false when the loop does not take it.
pub type Post = Arc<dyn Fn(Box<dyn FnOnce() + Send>) -> bool + Send + Sync>;

/// `$XDG_RUNTIME_DIR/ps5-launcher-health.sock`, or `/run/user/<uid>/…` without the variable.
pub fn socket_path(runtime_dir: Option<&str>, uid: u32) -> PathBuf {
    match runtime_dir.filter(|d| !d.is_empty()) {
        Some(dir) => Path::new(dir).join(SOCKET_NAME),
        None => PathBuf::from(format!("/run/user/{uid}")).join(SOCKET_NAME),
    }
}

/// The socket path of this launcher.
pub fn default_path() -> PathBuf {
    let dir = std::env::var("XDG_RUNTIME_DIR").ok();
    socket_path(dir.as_deref(), unsafe { libc::getuid() })
}

/// Listen at this launcher's path, answering from the Slint event loop. Logs and returns None when
/// the socket cannot be made.
pub fn start_for_launcher() -> Option<Health> {
    let path = default_path();
    let post: Post = Arc::new(|f| slint::invoke_from_event_loop(f).is_ok());
    start(&path, post, UI_TIMEOUT).map_err(|e| crate::log!("health socket {}: {e}", path.display())).ok()
}

/// The listening socket. Dropping it removes the socket file.
pub struct Health {
    path: PathBuf,
}

impl Drop for Health {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Listen at `path`. A socket left there by an earlier launcher is removed first. Each `ping`
/// gets `ok` once `post` has run its closure, within `ui_timeout`.
pub fn start(path: &Path, post: Post, ui_timeout: Duration) -> std::io::Result<Health> {
    remove_stale(path)?;
    let listener = UnixListener::bind(path)?;
    // bind() creates the file with the umask's mode: only this user may connect.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let health = Health { path: path.to_path_buf() };
    std::thread::Builder::new().name("health".into()).spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let post = post.clone();
            let spawned = std::thread::Builder::new()
                .name("health-conn".into())
                .spawn(move || serve(stream, &post, ui_timeout));
            if let Err(e) = spawned {
                crate::log!("health socket: {e}");
            }
        }
    })?;
    Ok(health)
}

/// Remove an old socket at `path`. Anything else there is left alone, and bind() fails on it.
fn remove_stale(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(path),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Answer the pings of one connection until it closes, goes idle or sends something else.
fn serve(stream: UnixStream, post: &Post, ui_timeout: Duration) {
    if stream.set_read_timeout(Some(IDLE_TIMEOUT)).is_err() || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err() {
        return;
    }
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        match (&mut reader).take(MAX_LINE).read_line(&mut line) {
            Ok(n) if n > 0 && line == "ping\n" => {}
            _ => return,
        }
        if !loop_alive(post, ui_timeout) || writer.write_all(b"ok\n").is_err() {
            return;
        }
    }
}

/// Post a closure to the UI loop, and wait until it has run.
fn loop_alive(post: &Post, ui_timeout: Duration) -> bool {
    let (tx, rx) = mpsc::sync_channel::<()>(1);
    if !post(Box::new(move || {
        let _ = tx.try_send(());
    })) {
        return false;
    }
    rx.recv_timeout(ui_timeout).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Queue = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

    /// A fake UI loop: posted closures wait in a queue until the test runs them.
    fn fake_loop() -> (Post, Queue) {
        let queue: Queue = Arc::new(Mutex::new(Vec::new()));
        let q = queue.clone();
        (Arc::new(move |f| {
            q.lock().unwrap().push(f);
            true
        }), queue)
    }

    /// Run the closures posted so far; wait up to a second for the first one.
    fn run_loop(queue: &Queue) {
        for _ in 0..100 {
            if !queue.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let jobs: Vec<_> = queue.lock().unwrap().drain(..).collect();
        assert!(!jobs.is_empty(), "the ping posted nothing to the UI loop");
        for job in jobs {
            job();
        }
    }

    /// A short folder: a Unix socket path must fit in 108 bytes, and macOS's TMPDIR is long.
    fn temp() -> tempfile::TempDir {
        tempfile::Builder::new().prefix("ps5l-health").tempdir_in("/tmp").unwrap()
    }

    fn connect(path: &Path) -> UnixStream {
        let stream = UnixStream::connect(path).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream
    }

    fn read_answer(stream: &mut UnixStream) -> String {
        let mut buf = [0u8; 16];
        let n = stream.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    #[test]
    fn the_path_is_in_the_runtime_folder() {
        assert_eq!(socket_path(Some("/run/user/1000"), 1000), PathBuf::from("/run/user/1000/ps5-launcher-health.sock"));
        assert_eq!(socket_path(Some("/tmp/xdg"), 1000), PathBuf::from("/tmp/xdg/ps5-launcher-health.sock"));
        assert_eq!(socket_path(None, 1001), PathBuf::from("/run/user/1001/ps5-launcher-health.sock"));
        assert_eq!(socket_path(Some(""), 1002), PathBuf::from("/run/user/1002/ps5-launcher-health.sock"));
    }

    #[test]
    fn ping_gets_ok_only_after_the_ui_loop_runs() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        let (post, queue) = fake_loop();
        let _health = start(&path, post, UI_TIMEOUT).unwrap();
        let mut client = connect(&path);
        client.write_all(b"ping\n").unwrap();
        // Wait until the ping reached the loop, then make sure nothing was answered yet.
        for _ in 0..100 {
            if !queue.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        client.set_nonblocking(true).unwrap();
        let mut buf = [0u8; 4];
        let early = client.read(&mut buf);
        assert!(matches!(&early, Err(e) if e.kind() == std::io::ErrorKind::WouldBlock), "answered before the loop ran: {early:?}");
        client.set_nonblocking(false).unwrap();
        run_loop(&queue);
        assert_eq!(read_answer(&mut client), "ok\n");
    }

    #[test]
    fn a_stuck_loop_gets_no_answer() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        let (post, queue) = fake_loop();
        let _health = start(&path, post, Duration::from_millis(100)).unwrap();
        let mut client = connect(&path);
        client.write_all(b"ping\n").unwrap();
        // The loop never runs the closure: the launcher closes the connection with no answer.
        assert_eq!(read_answer(&mut client), "");
        // A loop that only wakes up later answers nobody.
        let late: Vec<_> = queue.lock().unwrap().drain(..).collect();
        assert_eq!(late.len(), 1);
        for job in late {
            job();
        }
    }

    #[test]
    fn a_loop_that_refuses_the_closure_gets_no_answer() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        let _health = start(&path, Arc::new(|_| false), UI_TIMEOUT).unwrap();
        let mut client = connect(&path);
        client.write_all(b"ping\n").unwrap();
        assert_eq!(read_answer(&mut client), "");
    }

    #[test]
    fn several_pings_on_one_connection_and_several_connections() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        let (post, queue) = fake_loop();
        let _health = start(&path, post, UI_TIMEOUT).unwrap();
        let mut first = connect(&path);
        let mut second = connect(&path);
        for _ in 0..3 {
            first.write_all(b"ping\n").unwrap();
            run_loop(&queue);
            assert_eq!(read_answer(&mut first), "ok\n");
        }
        second.write_all(b"ping\n").unwrap();
        run_loop(&queue);
        assert_eq!(read_answer(&mut second), "ok\n");
    }

    #[test]
    fn something_else_than_ping_closes_the_connection() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        let (post, _queue) = fake_loop();
        let _health = start(&path, post, UI_TIMEOUT).unwrap();
        let mut client = connect(&path);
        client.write_all(b"reboot\n").unwrap();
        assert_eq!(read_answer(&mut client), "");
    }

    #[test]
    fn only_the_user_may_connect() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        let (post, _queue) = fake_loop();
        let _health = start(&path, post, UI_TIMEOUT).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_stale_socket_is_replaced_and_the_socket_goes_at_exit() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        // An earlier launcher crashed: its socket file is still there, and nobody listens.
        drop(UnixListener::bind(&path).unwrap());
        assert!(path.exists());
        let (post, queue) = fake_loop();
        let health = start(&path, post, UI_TIMEOUT).unwrap();
        let mut client = connect(&path);
        client.write_all(b"ping\n").unwrap();
        run_loop(&queue);
        assert_eq!(read_answer(&mut client), "ok\n");
        drop(health);
        assert!(!path.exists(), "the socket is removed at exit");
    }

    #[test]
    fn a_file_that_is_not_a_socket_is_left_alone() {
        let dir = temp();
        let path = dir.path().join("h.sock");
        std::fs::write(&path, "data").unwrap();
        let (post, _queue) = fake_loop();
        assert!(start(&path, post, UI_TIMEOUT).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "data");
    }
}
