//! One descriptor, one kernel-enforced all-component path-resolution policy.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

// Linux UAPI open_how version zero: three consecutive, zero-initialized u64s.
// Use libc's architecture-specific SYS_openat2, never a numeric syscall ID.
#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

const NO_MAGICLINKS: u64 = 0x02;
const NO_SYMLINKS: u64 = 0x04;

/// Open a selected regular log without following any symlink or magic link.
/// Unsupported kernel/security policy is an error, never an insecure fallback.
/// This does not authenticate file provenance or prevent hardlink/mount changes.
pub fn open_regular_log(path: &Path) -> io::Result<File> {
    open_checked(path, kernel_open)
}

fn open_checked(
    path: &Path,
    open: impl FnOnce(&CStr, &OpenHow) -> io::Result<File>,
) -> io::Result<File> {
    if !path.is_absolute()
        || path.as_os_str().len() > 4096
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log path must be absolute, bounded and contain no parent traversal",
        ));
    }
    let name = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "log path contains a NUL byte"))?;
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW) as u64,
        mode: 0,
        resolve: NO_SYMLINKS | NO_MAGICLINKS,
    };
    let file = open(&name, &how)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log source is not a regular file",
        ));
    }
    Ok(file)
}

fn kernel_open(name: &CStr, how: &OpenHow) -> io::Result<File> {
    // SAFETY: name is NUL terminated and both pointers remain valid for this
    // synchronous syscall. OpenHow has the Linux UAPI layout and all fields are
    // initialized. No O_CREAT/TMPFILE or writable access is requested.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD as libc::c_long,
            name.as_ptr(),
            how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat2 returns a newly owned nonnegative int FD. It
    // has exactly one File owner, including metadata/refusal error paths.
    Ok(unsafe { File::from_raw_fd(fd as std::os::fd::RawFd) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log_source::{read_lines, unread_bytes};
    use std::fs;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let root = PathBuf::from(
                std::env::var_os("FABRIC_SCRATCH_ROOT").expect("resource launcher required"),
            )
            .join(format!(
                "secure-log-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::create_dir(root.join("direct")).unwrap();
            fs::write(root.join("direct/log"), b"approved\n").unwrap();
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let result = fs::remove_dir_all(&self.0);
            if !std::thread::panicking() {
                result.unwrap();
                assert!(!self.0.exists());
            }
        }
    }

    #[test]
    fn security_regular_log_is_read_and_backlog_is_exact() {
        let scratch = Scratch::new();
        let path = scratch.0.join("direct/log");
        assert_eq!(unread_bytes(&path, None).unwrap(), 9);
        let result = read_lines(&path, None, 4096).unwrap();
        assert_eq!(result.lines[0].body, "approved");
        assert_eq!(unread_bytes(&path, Some(&result.cursor)).unwrap(), 0);
    }

    #[test]
    fn security_leaf_and_parent_symlinks_are_refused_by_both_read_paths() {
        let scratch = Scratch::new();
        symlink("direct/log", scratch.0.join("leaf")).unwrap();
        symlink("direct", scratch.0.join("parent")).unwrap();
        for path in [scratch.0.join("leaf"), scratch.0.join("parent/log")] {
            assert_eq!(
                open_regular_log(&path).unwrap_err().raw_os_error(),
                Some(libc::ELOOP)
            );
            assert!(unread_bytes(&path, None).is_err());
            assert!(read_lines(&path, None, 4096).is_err());
        }
    }

    #[test]
    fn security_proc_magic_link_is_not_a_regular_log_alias() {
        let scratch = Scratch::new();
        let file = open_regular_log(&scratch.0.join("direct/log")).unwrap();
        let alias = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        assert!(open_regular_log(&alias).is_err());
        assert!(unread_bytes(&alias, None).is_err());
    }

    #[test]
    fn security_replacement_cannot_redirect_an_already_checked_descriptor() {
        let scratch = Scratch::new();
        let path = scratch.0.join("direct/log");
        let mut file = open_regular_log(&path).unwrap();
        fs::rename(&path, scratch.0.join("original")).unwrap();
        fs::write(scratch.0.join("sentinel"), b"not-approved\n").unwrap();
        symlink(scratch.0.join("sentinel"), &path).unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "approved\n");
        assert!(read_lines(&path, None, 4096).is_err());
    }

    #[test]
    fn security_unsupported_or_denied_secure_open_never_falls_back() {
        let scratch = Scratch::new();
        let path = scratch.0.join("direct/log");
        for code in [libc::ENOSYS, libc::EPERM, libc::EINVAL, libc::ELOOP] {
            let result = open_checked(&path, |_, how| {
                assert_eq!(how.resolve, NO_SYMLINKS | NO_MAGICLINKS);
                assert_ne!(how.flags & libc::O_NONBLOCK as u64, 0);
                assert_ne!(how.flags & libc::O_CLOEXEC as u64, 0);
                Err(io::Error::from_raw_os_error(code))
            });
            assert_eq!(result.unwrap_err().raw_os_error(), Some(code));
        }
    }

    #[test]
    fn security_relative_and_parent_paths_fail_before_syscall() {
        for path in [Path::new("relative"), Path::new("/var/log/../private")] {
            let result = open_checked(path, |_, _| panic!("invalid path reached open"));
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn security_fifo_and_directory_are_refused_without_blocking_for_a_writer() {
        let scratch = Scratch::new();
        let fifo = scratch.0.join("pipe");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: this owned synthetic pathname remains valid for mkfifo.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            open_regular_log(&fifo).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            open_regular_log(&scratch.0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(unread_bytes(&fifo, None).is_err());
    }
}
