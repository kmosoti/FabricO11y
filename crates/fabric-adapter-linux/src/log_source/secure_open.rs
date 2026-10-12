//! No-follow component lookup through owned, checked directory descriptors.

use std::ffi::{CStr, CString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

/// Open a selected regular log without following any symlink or magic link.
/// Every lookup is relative to a checked directory FD and refuses symlinks.
/// A denied operation is an error, never an insecure fallback. Unlike openat2,
/// this mechanism also works under systemd's RestrictSUIDSGID syscall filter.
/// This does not authenticate file provenance or prevent hardlink/mount changes.
pub fn open_regular_log(path: &Path) -> io::Result<File> {
    open_checked(path, kernel_open)
}

fn open_checked(
    path: &Path,
    mut open: impl FnMut(RawFd, &CStr, libc::c_int) -> io::Result<File>,
) -> io::Result<File> {
    let bytes = path.as_os_str().as_bytes();
    if !path.is_absolute()
        || bytes.len() > 4096
        || bytes.contains(&0)
        || bytes.ends_with(b"/")
        || bytes.ends_with(b"/.")
        || path.components().any(|part| part == Component::ParentDir)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "log path must be absolute, bounded, NUL-free and contain no parent or trailing directory syntax",
        ));
    }
    let directory_flags = libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    let mut directory = open(libc::AT_FDCWD, c"/", directory_flags | libc::O_DIRECTORY)?;
    let mut parts = path
        .components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name),
            _ => None, // Absolute root and harmless interior `.` components only.
        })
        .peekable();
    while let Some(part) = parts.next() {
        let name = CString::new(part.as_bytes()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "log path contains a NUL byte")
        })?;
        if parts.peek().is_none() {
            let file = open(
                directory.as_raw_fd(),
                &name,
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW,
            )?;
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "log source is not a regular file",
                ));
            }
            return Ok(file);
        }
        // O_PATH | O_NOFOLLOW opens a symlink itself, without following it.
        // Inspect that FD, not its pathname. Do not use O_DIRECTORY here:
        // it would report ENOTDIR for symlinks instead of our ELOOP policy.
        let next = open(directory.as_raw_fd(), &name, directory_flags)?;
        let metadata = next.metadata()?;
        if metadata.file_type().is_symlink() {
            return Err(io::Error::from_raw_os_error(libc::ELOOP));
        }
        if !metadata.is_dir() {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }
        // Assignment closes the previous directory; only current/next FDs
        // coexist. Renaming the directory cannot redirect the next lookup.
        directory = next;
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "log source is not a regular file",
    ))
}

fn kernel_open(directory: RawFd, name: &CStr, flags: libc::c_int) -> io::Result<File> {
    // SAFETY: name is NUL terminated and live during this synchronous call;
    // directory is AT_FDCWD or a live owned directory FD. Flags never request
    // creation or writable access, so openat requires no variadic mode argument.
    let fd = unsafe { libc::openat(directory, name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful openat returns a newly owned nonnegative int FD. It
    // has exactly one File owner, including metadata/refusal error paths.
    Ok(unsafe { File::from_raw_fd(fd) })
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

    #[test]
    fn security_service_filter_keeps_regular_logs_readable_without_following_links() {
        const CHILD: &str = "FABRIC_SECURE_OPEN_FILTER_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "log_source::secure_open::tests::security_service_filter_keeps_regular_logs_readable_without_following_links",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "child: {output:?}");
            assert!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .contains("1 passed")
            );
            return;
        }

        // Fault injection only: deny openat2 in this isolated child test thread.
        // Every other syscall is allowed. The numeric syscall comes from libc
        // for the compiled target; no filter escapes into the parent process.
        let filter = [
            libc::sock_filter {
                code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                jt: 0,
                jf: 0,
                k: 0,
            },
            libc::sock_filter {
                code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                jt: 0,
                jf: 1,
                k: libc::SYS_openat2 as u32,
            },
            libc::sock_filter {
                code: (libc::BPF_RET | libc::BPF_K) as u16,
                jt: 0,
                jf: 0,
                k: libc::SECCOMP_RET_ERRNO | libc::ENOSYS as u32,
            },
            libc::sock_filter {
                code: (libc::BPF_RET | libc::BPF_K) as u16,
                jt: 0,
                jf: 0,
                k: libc::SECCOMP_RET_ALLOW,
            },
        ];
        let program = libc::sock_fprog {
            len: filter.len() as u16,
            filter: filter.as_ptr().cast_mut(),
        };
        // SAFETY: prctl reads the initialized BPF array synchronously. The
        // process is disposable; no_new_privs is set before installing it.
        unsafe {
            assert_eq!(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), 0);
            assert_eq!(
                libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program),
                0
            );
        }
        let scratch = Scratch::new();
        let path = scratch.0.join("direct/log");
        // Retain the exact defective mechanism as the representative control.
        #[repr(C)]
        struct OldOpenHow {
            flags: u64,
            mode: u64,
            resolve: u64,
        }
        let name = CString::new(path.as_os_str().as_bytes()).unwrap();
        let how = OldOpenHow {
            flags: (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOFOLLOW) as u64,
            mode: 0,
            resolve: 0x02 | 0x04,
        };
        // SAFETY: both pointers remain live; the struct has the UAPI layout.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                libc::AT_FDCWD,
                name.as_ptr(),
                &how,
                std::mem::size_of::<OldOpenHow>(),
            )
        };
        assert_eq!(fd, -1);
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOSYS)
        );
        assert_eq!(unread_bytes(&path, None).unwrap(), 9);
        let result = read_lines(&path, None, 4096).unwrap();
        assert_eq!(result.lines[0].body, "approved");
        assert_eq!(unread_bytes(&path, Some(&result.cursor)).unwrap(), 0);
        symlink("direct/log", scratch.0.join("leaf")).unwrap();
        symlink("direct", scratch.0.join("parent")).unwrap();
        for denied in [scratch.0.join("leaf"), scratch.0.join("parent/log")] {
            assert_eq!(
                open_regular_log(&denied).unwrap_err().raw_os_error(),
                Some(libc::ELOOP)
            );
            assert!(read_lines(&denied, None, 4096).is_err());
            assert!(unread_bytes(&denied, None).is_err());
        }
        let file = open_regular_log(&path).unwrap();
        for alias in [
            format!("/proc/self/fd/{}", file.as_raw_fd()),
            format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd()),
        ] {
            assert!(open_regular_log(Path::new(&alias)).is_err());
        }
    }

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
        // Numeric PID removes the ordinary /proc/self parent symlink, so this
        // also exercises refusal of a magic link at the leaf itself.
        for alias in [
            PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
            PathBuf::from(format!(
                "/proc/{}/fd/{}",
                std::process::id(),
                file.as_raw_fd()
            )),
        ] {
            assert!(open_regular_log(&alias).is_err());
            assert!(unread_bytes(&alias, None).is_err());
            assert!(read_lines(&alias, None, 4096).is_err());
        }
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
        let stages = 1 + path
            .components()
            .filter(|part| matches!(part, Component::Normal(_)))
            .count();
        for failed_stage in 1..=stages {
            for code in [libc::ENOSYS, libc::EPERM, libc::EINVAL, libc::ELOOP] {
                let mut calls = 0;
                let result = open_checked(&path, |directory, name, flags| {
                    calls += 1;
                    assert_ne!(flags & libc::O_NOFOLLOW, 0);
                    assert_ne!(flags & libc::O_CLOEXEC, 0);
                    assert_eq!(flags & libc::O_ACCMODE, libc::O_RDONLY);
                    if calls == stages {
                        assert_eq!(flags & libc::O_PATH, 0);
                        assert_ne!(flags & libc::O_NONBLOCK, 0);
                    } else {
                        assert_ne!(flags & libc::O_PATH, 0);
                    }
                    if calls == failed_stage {
                        Err(io::Error::from_raw_os_error(code))
                    } else {
                        kernel_open(directory, name, flags)
                    }
                });
                assert_eq!(result.unwrap_err().raw_os_error(), Some(code));
                assert_eq!(calls, failed_stage, "a failed lookup must not be retried");
            }
        }
    }

    #[test]
    fn security_parent_replacement_during_walk_cannot_redirect_the_leaf() {
        let scratch = Scratch::new();
        let path = scratch.0.join("direct/log");
        fs::create_dir(scratch.0.join("sentinel")).unwrap();
        fs::write(scratch.0.join("sentinel/log"), b"not-approved\n").unwrap();
        let mut replaced = false;
        let mut file = open_checked(&path, |directory, name, flags| {
            let file = kernel_open(directory, name, flags)?;
            if name == c"direct" {
                assert!(!replaced);
                fs::rename(scratch.0.join("direct"), scratch.0.join("retired")).unwrap();
                symlink("sentinel", scratch.0.join("direct")).unwrap();
                replaced = true;
            }
            Ok(file)
        })
        .unwrap();
        assert!(replaced, "replacement must occur before the leaf lookup");
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "approved\n");
        assert_eq!(
            open_regular_log(&path).unwrap_err().raw_os_error(),
            Some(libc::ELOOP)
        );
        assert!(read_lines(&path, None, 4096).is_err());
        assert!(unread_bytes(&path, None).is_err());
    }

    #[test]
    fn security_directory_syntax_and_invalid_names_fail_before_lookup() {
        let paths = [
            PathBuf::from("/"),
            PathBuf::from("/var/log/file/"),
            PathBuf::from("/var/log/file/."),
            PathBuf::from(std::ffi::OsStr::from_bytes(b"/var/log/file\0private")),
            PathBuf::from(format!("/{}", "x".repeat(4096))),
        ];
        for path in paths {
            assert_eq!(
                open_checked(&path, |_, _, _| panic!("invalid path reached open"))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        let scratch = Scratch::new();
        assert_eq!(
            open_regular_log(&scratch.0.join("direct/log/child"))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ENOTDIR)
        );
        let path = scratch
            .0
            .join(std::ffi::OsStr::from_bytes(b"direct/log-\xff"));
        fs::write(&path, b"approved\n").unwrap();
        assert_eq!(unread_bytes(&path, None).unwrap(), 9);
        assert_eq!(
            read_lines(&scratch.0.join("direct/./log"), None, 4096)
                .unwrap()
                .lines[0]
                .body,
            "approved"
        );
    }

    #[test]
    fn security_relative_and_parent_paths_fail_before_syscall() {
        for path in [Path::new("relative"), Path::new("/var/log/../private")] {
            let result = open_checked(path, |_, _, _| panic!("invalid path reached open"));
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
        let socket = scratch.0.join("socket");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        for path in [&socket, Path::new("/dev/null")] {
            assert!(open_regular_log(path).is_err());
            assert!(read_lines(path, None, 4096).is_err());
            assert!(unread_bytes(path, None).is_err());
        }
    }

    #[test]
    fn security_regular_replacement_and_hardlinks_remain_outside_origin_authentication() {
        let scratch = Scratch::new();
        let path = scratch.0.join("direct/log");
        fs::hard_link(&path, scratch.0.join("hardlink")).unwrap();
        assert_eq!(
            read_lines(&scratch.0.join("hardlink"), None, 4096)
                .unwrap()
                .lines[0]
                .body,
            "approved"
        );
        fs::rename(&path, scratch.0.join("retired")).unwrap();
        fs::write(&path, b"rotated\n").unwrap();
        assert_eq!(
            read_lines(&path, None, 4096).unwrap().lines[0].body,
            "rotated"
        );
    }
}
