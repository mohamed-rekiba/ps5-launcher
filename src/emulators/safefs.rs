//! Reading inside a folder without leaving it: every folder and file below a starting folder is
//! opened relative to its parent's handle (openat), with O_NOFOLLOW, so a symbolic link is
//! refused and a folder swapped for a link between a check and an open is never followed. Files
//! open with O_NONBLOCK and must be regular files by fstat, so a FIFO cannot stall a read.
//!
//! The starting folder itself is opened by path and may be a link: it is the user's data folder.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// The largest file in an addon (a document, an image, a sound) the launcher opens or digests.
pub const MAX_FILE: u64 = 16 * 1024 * 1024;

/// What a name in a folder is, without following a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dir,
    File,
    Link,
    Other,
}

/// Why a name could not be opened or read.
#[derive(Debug)]
pub enum Refused {
    Missing,
    Link,
    NotDir,
    NotFile,
    TooBig { len: u64, limit: u64 },
    Io(io::Error),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Refused::Missing => f.write_str("is missing"),
            Refused::Link => f.write_str("is a symbolic link"),
            Refused::NotDir => f.write_str("is not a folder"),
            Refused::NotFile => f.write_str("is not a file"),
            Refused::TooBig { len, limit } => write!(f, "is {} KiB; the limit is {} KiB", len.div_ceil(1024), limit / 1024),
            Refused::Io(e) => write!(f, "cannot be read: {e}"),
        }
    }
}

/// A `Refused` with the name it is about, as an io::Error ("media is a symbolic link").
pub fn named(name: impl AsRef<OsStr>, why: Refused) -> io::Error {
    match why {
        Refused::Io(e) if e.kind() == io::ErrorKind::NotFound => io::Error::new(io::ErrorKind::NotFound, format!("{} is missing", name.as_ref().to_string_lossy())),
        Refused::Missing => io::Error::new(io::ErrorKind::NotFound, format!("{} is missing", name.as_ref().to_string_lossy())),
        why => io::Error::other(format!("{} {why}", name.as_ref().to_string_lossy())),
    }
}

/// An open folder.
pub struct Dir {
    fd: OwnedFd,
}

fn c_name(name: &OsStr) -> Result<CString, Refused> {
    if name.is_empty() || name == "." || name == ".." || name.as_bytes().contains(&b'/') {
        return Err(Refused::Io(io::Error::new(io::ErrorKind::InvalidInput, "not a single name")));
    }
    CString::new(name.as_bytes()).map_err(|_| Refused::Io(io::Error::new(io::ErrorKind::InvalidInput, "a name with a NUL")))
}

fn stat_kind(mode: libc::mode_t) -> Kind {
    match mode & libc::S_IFMT {
        libc::S_IFDIR => Kind::Dir,
        libc::S_IFREG => Kind::File,
        libc::S_IFLNK => Kind::Link,
        _ => Kind::Other,
    }
}

fn set_errno(value: libc::c_int) {
    // SAFETY: the C library's errno location for this thread, always valid to write.
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = value;
    }
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = value;
    }
}

impl Dir {
    /// Open the starting folder by its path (a link there is followed).
    pub fn open(path: &Path) -> Result<Dir, Refused> {
        let file = File::open(path).map_err(|e| if e.kind() == io::ErrorKind::NotFound { Refused::Missing } else { Refused::Io(e) })?;
        let meta = file.metadata().map_err(Refused::Io)?;
        if !meta.is_dir() {
            return Err(Refused::NotDir);
        }
        Ok(Dir { fd: file.into() })
    }

    /// Open the starting folder, then each part below it without following a link.
    pub fn open_below(path: &Path, parts: &[&str]) -> io::Result<Dir> {
        let mut dir = Dir::open(path).map_err(|e| named(path.as_os_str(), e))?;
        for (i, part) in parts.iter().enumerate() {
            dir = dir.dir(OsStr::new(part)).map_err(|e| named(parts[..=i].join("/"), e))?;
        }
        Ok(dir)
    }

    /// What `name` is, without following a link.
    pub fn kind(&self, name: &OsStr) -> Result<Kind, Refused> {
        let c = c_name(name)?;
        // SAFETY: `st` is written by fstatat before it is read; `c` is a valid C string.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatat(self.fd.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            let e = io::Error::last_os_error();
            return Err(if e.kind() == io::ErrorKind::NotFound { Refused::Missing } else { Refused::Io(e) });
        }
        Ok(stat_kind(st.st_mode))
    }

    fn open_at(&self, name: &OsStr, flags: libc::c_int) -> Result<OwnedFd, Refused> {
        let c = c_name(name)?;
        // SAFETY: openat with a valid descriptor and C string; the result is checked.
        let fd = unsafe { libc::openat(self.fd.as_raw_fd(), c.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            let e = io::Error::last_os_error();
            // Say why from the name itself: a link, a file where a folder was expected, …
            return Err(match self.kind(name) {
                Ok(Kind::Link) => Refused::Link,
                Ok(Kind::File | Kind::Other) if flags & libc::O_DIRECTORY != 0 => Refused::NotDir,
                Err(Refused::Missing) => Refused::Missing,
                _ => Refused::Io(e),
            });
        }
        // SAFETY: `fd` is a new descriptor that nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Open a folder in this one.
    pub fn dir(&self, name: &OsStr) -> Result<Dir, Refused> {
        Ok(Dir { fd: self.open_at(name, libc::O_RDONLY | libc::O_DIRECTORY)? })
    }

    /// Open a regular file in this one, at most `limit` bytes long.
    pub fn file(&self, name: &OsStr, limit: u64) -> Result<File, Refused> {
        let file = File::from(self.open_at(name, libc::O_RDONLY | libc::O_NONBLOCK)?);
        let meta = file.metadata().map_err(Refused::Io)?;
        if !meta.file_type().is_file() {
            return Err(Refused::NotFile);
        }
        if meta.len() > limit {
            return Err(Refused::TooBig { len: meta.len(), limit });
        }
        Ok(file)
    }

    /// Read a regular file in this one, at most `limit` bytes long.
    pub fn read(&self, name: &OsStr, limit: u64) -> Result<Vec<u8>, Refused> {
        let file = self.file(name, limit)?;
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1)).read_to_end(&mut bytes).map_err(Refused::Io)?;
        if bytes.len() as u64 > limit {
            return Err(Refused::TooBig { len: bytes.len() as u64, limit });
        }
        Ok(bytes)
    }

    /// Read a relative resource path through directory handles, refusing links and
    /// special files at every step. Each component is checked by `dir` or `read`.
    pub fn read_relative(&self, relative: &str, limit: u64) -> Result<Vec<u8>, Refused> {
        match relative.split_once('/') {
            Some((first, rest)) => self.dir(OsStr::new(first))?.read_relative(rest, limit),
            None => self.read(OsStr::new(relative), limit),
        }
    }

    /// The names in this folder, with what each is, in byte order.
    pub fn entries(&self) -> io::Result<Vec<(OsString, Kind)>> {
        // SAFETY: dup gives fdopendir a descriptor of its own, which closedir closes.
        let fd = unsafe { libc::dup(self.fd.as_raw_fd()) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let dirp = unsafe { libc::fdopendir(fd) };
        if dirp.is_null() {
            let e = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(e);
        }
        let mut names = Vec::new();
        // SAFETY: the dup shares its position with `self.fd`; start from the beginning.
        unsafe { libc::rewinddir(dirp) };
        loop {
            // readdir returns null both at the end and on an error; only errno tells them
            // apart, so it is cleared first. A listing is never quietly partial.
            set_errno(0);
            // SAFETY: readdir returns null at the end, else an entry valid until the next call.
            let entry = unsafe { libc::readdir(dirp) };
            if entry.is_null() {
                let e = io::Error::last_os_error();
                if e.raw_os_error().unwrap_or(0) != 0 {
                    unsafe { libc::closedir(dirp) };
                    return Err(e);
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if name != b"." && name != b".." {
                names.push(OsStr::from_bytes(name).to_os_string());
            }
        }
        unsafe { libc::closedir(dirp) };
        names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        let mut out = Vec::new();
        for name in names {
            match self.kind(&name) {
                Ok(kind) => out.push((name, kind)),
                Err(Refused::Missing) => {}
                Err(Refused::Io(e)) => return Err(e),
                Err(other) => return Err(io::Error::other(other.to_string())),
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_file_that_became_a_fifo_is_refused_without_blocking() {
        // The open is the check: whatever passed an earlier look, a FIFO in its place is opened
        // without blocking (no writer is there) and refused by fstat.
        let dir = tempfile::Builder::new().prefix("addons-").tempdir().unwrap();
        std::fs::write(dir.path().join("icon.svg"), "<svg/>").unwrap();
        let handle = Dir::open(dir.path()).unwrap();
        assert!(handle.file(OsStr::new("icon.svg"), 1024).is_ok(), "a regular file passes");
        std::fs::remove_file(dir.path().join("icon.svg")).unwrap();
        let c = CString::new(dir.path().join("icon.svg").as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid C string; the result is checked.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let result = handle.file(OsStr::new("icon.svg"), 1024).map(|_| ()).map_err(|e| e.to_string());
            send.send(result).unwrap();
        });
        let result = receive.recv_timeout(Duration::from_secs(5)).expect("the open blocked on the FIFO");
        assert_eq!(result, Err("is not a file".to_string()));
    }
}
