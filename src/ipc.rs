//! Linux IPC migration. Never sends requests before authenticating the server.
#[cfg(test)]
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const MAX_LINE: usize = 8 * 1024 * 1024;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn uid() -> libc::uid_t {
    // SAFETY: getuid takes no pointers and cannot fail.
    unsafe { libc::getuid() }
}
fn runtime() -> PathBuf {
    crate::storage::Paths::new().runtime
}
pub fn private_runtime(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err(invalid("unsafe runtime path"));
    }
    // Walk by directory descriptors so concurrent symlink replacement cannot redirect checks.
    let mut parent = OpenOptions::new().read(true).open("/")?;
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let name = std::ffi::CString::new(part.as_bytes()).map_err(|_| invalid("invalid path"))?;
        // SAFETY: name is NUL terminated and parent is a live directory descriptor.
        let made = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if made < 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat receives a valid descriptor and C string; O_NOFOLLOW rejects links.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor.
        parent = unsafe { File::from_raw_fd(fd) };
        use std::os::unix::fs::MetadataExt;
        let meta = parent.metadata()?;
        let mode = meta.mode() & 0o7777;
        let safe = if i + 1 == parts.len() {
            meta.uid() == uid() && mode == 0o700
        } else {
            (meta.uid() == 0 || meta.uid() == uid())
                && (mode & 0o022 == 0 || (meta.uid() == 0 && mode & 0o1000 != 0))
        };
        if !safe {
            return Err(invalid("unsafe runtime directory ownership or permissions"));
        }
    }
    Ok(())
}
pub fn connect() -> io::Result<UnixStream> {
    let path = runtime();
    private_runtime(&path)?;
    let stream = UnixStream::connect(path.join("daemon.sock"))?;
    // Authenticate before receiving state or transmitting credentials.
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: both output pointers reference writable memory of the declared sizes.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if len as usize != std::mem::size_of::<libc::ucred>() || credentials.uid != uid() {
        return Err(invalid("refusing daemon owned by another user"));
    }
    Ok(stream)
}
fn line(reader: &mut impl BufRead, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let n = reader
        .take(limit as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if n == 0 {
        return Ok(None);
    }
    if n > limit || bytes.last() != Some(&b'\n') {
        return Err(invalid("IPC line too large or truncated"));
    }
    Ok(Some(bytes))
}
pub fn json_line(reader: &mut impl BufRead) -> io::Result<Option<(Vec<u8>, serde_json::Value)>> {
    let Some(bytes) = line(reader, MAX_LINE)? else {
        return Ok(None);
    };
    let value = serde_json::from_slice(&bytes).map_err(|_| invalid("invalid IPC JSON"))?;
    Ok(Some((bytes, value)))
}
pub fn bridge() -> io::Result<()> {
    let stream = connect()?;
    let mut outgoing = stream.try_clone()?;
    println!("{{\"type\":\"connected\"}}");
    io::stdout().flush()?;
    std::thread::spawn(move || {
        let result = (|| -> io::Result<()> {
            let mut input = io::stdin().lock();
            while let Some((bytes, value)) = json_line(&mut input)? {
                if !value.is_object() {
                    return Err(invalid("request must be an object"));
                }
                outgoing.write_all(&bytes)?;
            }
            Ok(())
        })();
        // End the bridge on stdin EOF, even while the daemon is idle.
        if result.is_err() {
            eprintln!("omarchy-cloudkit: IPC input failed");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    });
    let mut reader = BufReader::new(stream);
    let mut output = io::stdout().lock();
    while let Some((bytes, _)) = json_line(&mut reader)? {
        output.write_all(&bytes)?;
        output.flush()?;
    }
    Err(invalid("daemon disconnected"))
}
pub fn busy() -> io::Result<()> {
    // No runtime/socket means stopped; every other failure is unknown, never "idle".
    let path = runtime();
    private_runtime(&path)?;
    let socket = path.join("daemon.sock");
    match fs::symlink_metadata(&socket) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            println!("0");
            return Ok(());
        }
        Err(e) => return Err(e),
        Ok(_) => (),
    }
    let stream = connect()?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream);
    let (_, state) = json_line(&mut reader)?.ok_or_else(|| invalid("missing state"))?;
    if state["type"] != "state" {
        return Err(invalid("expected state"));
    }
    let summary = &state["data"]["summary"];
    let uploads = summary["uploads"]
        .as_u64()
        .ok_or_else(|| invalid("missing uploads"))?;
    let syncing = summary["syncing"]
        .as_u64()
        .ok_or_else(|| invalid("missing syncing"))?;
    println!(
        "{}",
        uploads
            .checked_add(syncing)
            .ok_or_else(|| invalid("invalid counts"))?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn framing_rejects_truncation_and_oversize() {
        assert!(line(&mut Cursor::new(b"abc"), 10).is_err());
        assert!(line(&mut Cursor::new(b"abcd\n"), 4).is_err());
        assert_eq!(
            line(&mut Cursor::new(b"abc\nnext\n"), 4).unwrap().unwrap(),
            b"abc\n"
        );
        assert!(line(&mut Cursor::new(b""), 4).unwrap().is_none());
    }
    #[test]
    fn rejects_insecure_runtime_without_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let path = env::temp_dir().join(format!("cloudkit-ipc-test-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_runtime(&path).is_err());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        private_runtime(&path).unwrap();
        let link = path.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(private_runtime(&link).is_err());
        fs::remove_file(link).unwrap();
        fs::remove_dir(path).unwrap();
    }
}
