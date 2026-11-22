use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt},
            net::UnixListener,
        },
    },
    path::{Path, PathBuf},
};

pub const ROOT: &str = "/tmp/ngai_tunes";
pub const SOCKET: &str = "/tmp/ngai_tunes/daemon.sock";

pub struct Runtime {
    pub listener: UnixListener,
    socket: PathBuf,
    _lock: File,
}

fn unsafe_path() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "Unsafe runtime path")
}

pub fn verify_directory(root: &Path) -> io::Result<()> {
    let info = fs::symlink_metadata(root)?;
    if !info.is_dir() || info.uid() != unsafe { libc::geteuid() } || info.mode() & 0o777 != 0o700 {
        return Err(unsafe_path());
    }
    Ok(())
}

fn prepare_directory(root: &Path) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(root) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    verify_directory(root)
}

pub(crate) fn lock_file(root: &Path, name: &str, operation: libc::c_int) -> io::Result<File> {
    prepare_directory(root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join(name))?;
    let info = lock.metadata()?;
    if !info.is_file() || info.uid() != unsafe { libc::geteuid() } || info.mode() & 0o077 != 0 {
        return Err(unsafe_path());
    }
    if unsafe { libc::flock(lock.as_raw_fd(), operation) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(lock)
}

impl Runtime {
    pub fn bind(root: &Path) -> io::Result<Self> {
        let lock = lock_file(root, "daemon.lock", libc::LOCK_EX | libc::LOCK_NB)?;
        let socket = root.join("daemon.sock");
        match fs::symlink_metadata(&socket) {
            Ok(info)
                if info.file_type().is_socket() && info.uid() == unsafe { libc::geteuid() } =>
            {
                fs::remove_file(&socket)?
            }
            Ok(_) => return Err(unsafe_path()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(&socket)?;
        let runtime = Self {
            listener,
            socket,
            _lock: lock,
        };
        runtime.listener.set_nonblocking(true)?;
        Ok(runtime)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn locks_and_cleans_up_socket() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("runtime");
        let running = Runtime::bind(&root).unwrap();
        assert!(Runtime::bind(&root).is_err());
        assert!(UnixStream::connect(root.join("daemon.sock")).is_ok());
        drop(running);
        assert!(!root.join("daemon.sock").exists());
        let stale = UnixListener::bind(root.join("daemon.sock")).unwrap();
        drop(stale);
        assert!(Runtime::bind(&root).is_ok());
    }
}
