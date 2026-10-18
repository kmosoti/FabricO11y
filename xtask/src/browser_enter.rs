//! Bounded native cgroup entry and release-before-exec handshake for browser fixtures.

use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CGROUP2_SUPER_MAGIC: libc::c_long = 0x6367_7270;
const MAX_HANDSHAKE_MS: u64 = 5_000;

#[derive(Debug, PartialEq, Eq)]
struct Options {
    group: PathBuf,
    ready_fd: RawFd,
    release_fd: RawFd,
    timeout_ms: u64,
    target: PathBuf,
    target_args: Vec<String>,
}

fn parse(arguments: &[String]) -> Result<Options, String> {
    let separator = arguments
        .iter()
        .position(|value| value == "--")
        .ok_or("-- separator required")?;
    let (flags, tail) = arguments.split_at(separator);
    let target = tail.get(1).ok_or("absolute target argv required")?;
    let target = PathBuf::from(target);
    if !target.is_absolute() {
        return Err("absolute target argv required".into());
    }

    let mut group = None;
    let mut ready_fd = None;
    let mut release_fd = None;
    let mut timeout_ms = None;
    let mut index = 0;
    while index < flags.len() {
        let flag = &flags[index];
        let value = flags.get(index + 1).ok_or("flag value required")?;
        let slot = match flag.as_str() {
            "--group" => &mut group,
            "--ready-fd" => {
                if ready_fd.is_some() {
                    return Err("duplicate flag".into());
                }
                ready_fd = Some(parse_fd(value)?);
                index += 2;
                continue;
            }
            "--release-fd" => {
                if release_fd.is_some() {
                    return Err("duplicate flag".into());
                }
                release_fd = Some(parse_fd(value)?);
                index += 2;
                continue;
            }
            "--timeout-ms" => {
                if timeout_ms.is_some() {
                    return Err("duplicate flag".into());
                }
                let parsed = value
                    .parse::<u64>()
                    .map_err(|_| "invalid timeout")?;
                if !(1..=MAX_HANDSHAKE_MS).contains(&parsed) {
                    return Err("timeout must be within 1..=5000 ms".into());
                }
                timeout_ms = Some(parsed);
                index += 2;
                continue;
            }
            _ => return Err("unknown or duplicate flag".into()),
        };
        if slot.replace(value.clone()).is_some() {
            return Err("duplicate flag".into());
        }
        index += 2;
    }
    let group = PathBuf::from(group.ok_or("--group required")?);
    if !group.is_absolute() {
        return Err("absolute cgroup path required".into());
    }
    let ready_fd = ready_fd.ok_or("--ready-fd required")?;
    let release_fd = release_fd.ok_or("--release-fd required")?;
    if ready_fd <= 2 || release_fd <= 2 || ready_fd == release_fd {
        return Err("distinct inherited control descriptors greater than 2 required".into());
    }
    Ok(Options {
        group,
        ready_fd,
        release_fd,
        timeout_ms: timeout_ms.ok_or("--timeout-ms required")?,
        target,
        target_args: tail[2..].to_vec(),
    })
}

fn parse_fd(value: &str) -> Result<RawFd, String> {
    let parsed = value.parse::<i32>().map_err(|_| "invalid descriptor")?;
    if parsed < 0 {
        return Err("invalid descriptor".into());
    }
    Ok(parsed)
}

fn fd_flags(fd: RawFd) -> Result<(libc::c_int, libc::mode_t), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err("control descriptor unavailable".into());
    }
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 || (stat.st_mode & libc::S_IFMT) != libc::S_IFIFO {
        return Err("control descriptor must be a pipe".into());
    }
    Ok((flags & libc::O_ACCMODE, stat.st_mode))
}

fn validate_control_fds(ready_fd: RawFd, release_fd: RawFd) -> Result<(), String> {
    if ready_fd <= 2 || release_fd <= 2 || ready_fd == release_fd {
        return Err("invalid control descriptor identity".into());
    }
    if fd_flags(ready_fd)?.0 != libc::O_WRONLY || fd_flags(release_fd)?.0 != libc::O_RDONLY {
        return Err("control descriptor direction mismatch".into());
    }
    let ready_flags = unsafe { libc::fcntl(ready_fd, libc::F_GETFD) };
    let release_flags = unsafe { libc::fcntl(release_fd, libc::F_GETFD) };
    if ready_flags < 0
        || release_flags < 0
        || ready_flags & libc::FD_CLOEXEC != 0
        || release_flags & libc::FD_CLOEXEC != 0
    {
        return Err("control descriptors must be inherited".into());
    }
    Ok(())
}

fn open_owned_cgroup(path: &Path) -> Result<(OwnedFd, OwnedFd), String> {
    let canonical = fs::canonicalize(path).map_err(|_| "cgroup directory unavailable")?;
    if canonical != path {
        return Err("cgroup path must be canonical".into());
    }
    let path_c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| "invalid cgroup path")?;
    let directory = unsafe {
        libc::open(
            path_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if directory < 0 {
        return Err("cannot open cgroup directory without following symlinks".into());
    }
    let directory = unsafe { OwnedFd::from_raw_fd(directory) };
    let mut filesystem = unsafe { std::mem::zeroed::<libc::statfs>() };
    if unsafe { libc::fstatfs(directory.as_raw_fd(), &mut filesystem) } != 0
        || filesystem.f_type as libc::c_long != CGROUP2_SUPER_MAGIC
    {
        return Err("owned directory is not cgroup v2".into());
    }
    let cgroup_procs = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c"cgroup.procs".as_ptr(),
            libc::O_WRONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if cgroup_procs < 0 {
        return Err("cannot open cgroup.procs without following symlinks".into());
    }
    Ok((directory, unsafe { OwnedFd::from_raw_fd(cgroup_procs) }))
}

fn enter_and_verify(path: &Path, group_fd: RawFd, procs_fd: RawFd) -> Result<(), String> {
    let pid = std::process::id().to_string();
    let duplicate = unsafe { libc::dup(procs_fd) };
    if duplicate < 0 {
        return Err("cannot retain cgroup.procs descriptor".into());
    }
    let mut file = unsafe { fs::File::from_raw_fd(duplicate) };
    file.write_all(pid.as_bytes())
        .map_err(|_| "cannot enter owned cgroup")?;
    drop(file);

    let cgroup_contents = fs::read_to_string("/proc/self/cgroup")
        .map_err(|_| "cannot verify cgroup placement")?;
    let actual = cgroup_contents
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or("unified cgroup placement unavailable")?;
    let mount = Path::new("/sys/fs/cgroup");
    let relative = path
        .strip_prefix(mount)
        .map_err(|_| "cgroup path outside unified mount")?;
    let expected = format!("/{}", relative.display());
    if actual != expected {
        return Err("cgroup placement mismatch".into());
    }
    let _ = group_fd;
    Ok(())
}

fn monotonic_ns() -> Result<u64, String> {
    let mut value = unsafe { std::mem::zeroed::<libc::timespec>() };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
        return Err("monotonic clock unavailable".into());
    }
    Ok(value.tv_sec as u64 * 1_000_000_000 + value.tv_nsec as u64)
}

fn wait_fd(fd: RawFd, events: libc::c_short, deadline: u64) -> Result<libc::c_short, String> {
    loop {
        let now = monotonic_ns()?;
        if now >= deadline {
            return Err("entry handshake deadline".into());
        }
        let remaining_ms = ((deadline - now).saturating_add(999_999) / 1_000_000)
            .min(i32::MAX as u64) as i32;
        let mut item = libc::pollfd { fd, events, revents: 0 };
        let result = unsafe { libc::poll(&mut item, 1, remaining_ms) };
        if result > 0 {
            if monotonic_ns()? >= deadline {
                return Err("entry handshake deadline".into());
            }
            return Ok(item.revents);
        }
        if result == 0 {
            return Err("entry handshake deadline".into());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err("entry handshake poll failed".into());
        }
    }
}

fn set_nonblocking(fd: RawFd) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err("cannot configure control descriptor".into());
    }
    Ok(())
}

fn write_ready(fd: RawFd, deadline: u64) -> Result<(), String> {
    let byte = b'R';
    loop {
        let events = wait_fd(fd, libc::POLLOUT | libc::POLLHUP | libc::POLLERR, deadline)?;
        if events & (libc::POLLHUP | libc::POLLERR) != 0 || events & libc::POLLOUT == 0 {
            return Err("READY pipe closed".into());
        }
        let written = unsafe { libc::write(fd, (&byte as *const u8).cast(), 1) };
        if written == 1 {
            return Ok(());
        }
        if written < 0 {
            match io::Error::last_os_error().kind() {
                io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
                _ => return Err("cannot publish READY".into()),
            }
        }
        return Err("cannot publish READY".into());
    }
}

fn wait_frame(fd: RawFd, expected: u8, label: &str, deadline: u64) -> Result<(), String> {
    let mut received = false;
    loop {
        let events = wait_fd(fd, libc::POLLIN | libc::POLLHUP | libc::POLLERR, deadline)?;
        if events & libc::POLLERR != 0 {
            return Err(format!("{label} pipe error"));
        }
        let mut bytes = [0u8; 2];
        let count = unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count == 0 {
            return if received {
                Ok(())
            } else {
                Err(format!("{label} closed before frame"))
            };
        }
        if count > 0 {
            if received || count != 1 || bytes[0] != expected {
                return Err(format!("invalid {label} framing"));
            }
            received = true;
            continue;
        }
        match io::Error::last_os_error().kind() {
            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock => continue,
            _ => return Err(format!("{label} pipe read failed")),
        }
    }
}

fn run(options: Options) -> Result<(), String> {
    validate_control_fds(options.ready_fd, options.release_fd)?;
    let ready = unsafe { OwnedFd::from_raw_fd(options.ready_fd) };
    let release = unsafe { OwnedFd::from_raw_fd(options.release_fd) };
    set_nonblocking(ready.as_raw_fd())?;
    set_nonblocking(release.as_raw_fd())?;
    let deadline = monotonic_ns()?
        .checked_add(options.timeout_ms.saturating_mul(1_000_000))
        .ok_or("invalid handshake deadline")?;
    let (group, procs) = open_owned_cgroup(&options.group)?;
    enter_and_verify(&options.group, group.as_raw_fd(), procs.as_raw_fd())?;
    write_ready(ready.as_raw_fd(), deadline)?;
    drop(ready);
    wait_frame(release.as_raw_fd(), b'G', "RELEASE", deadline)?;
    drop(release);
    drop(procs);
    drop(group);

    let error = Command::new(&options.target)
        .args(&options.target_args)
        .exec();
    Err(format!("target exec failed: {error}"))
}

pub fn cli(arguments: Vec<String>) -> u8 {
    match parse(&arguments).and_then(run) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("browser entry failed: {error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Vec<String> {
        [
            vec![
                "--group".into(),
                "/sys/fs/cgroup/fabric-work-test/browser-0".into(),
                "--ready-fd".into(),
                "7".into(),
                "--release-fd".into(),
                "8".into(),
                "--timeout-ms".into(),
                "5000".into(),
                "--".into(),
                "/usr/bin/true".into(),
            ],
            extra.iter().map(|s| (*s).to_owned()).collect(),
        ]
        .concat()
    }

    #[test]
    fn parser_requires_absolute_target_and_bounded_timeout() {
        assert!(parse(&args(&[])).is_ok());
        for timeout in ["0", "5001", "-1", "999999999999999999999"] {
            let mut bad = args(&[]);
            let index = bad.iter().position(|value| value == "5000").unwrap();
            bad[index] = timeout.into();
            assert!(parse(&bad).is_err());
        }
        let mut relative = args(&[]);
        *relative.last_mut().unwrap() = "true".into();
        assert!(parse(&relative).is_err());
        let mut duplicate = args(&[]);
        duplicate.splice(0..0, ["--group".into(), "/tmp/other".into()]);
        assert!(parse(&duplicate).is_err());
    }

    #[test]
    fn descriptors_require_distinct_pipe_directions_above_stdio() {
        let mut ready = [0; 2];
        let mut release = [0; 2];
        assert_eq!(unsafe { libc::pipe(ready.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::pipe(release.as_mut_ptr()) }, 0);
        let result = validate_control_fds(ready[1], release[0]);
        unsafe {
            libc::close(ready[0]);
            libc::close(ready[1]);
            libc::close(release[0]);
            libc::close(release[1]);
        }
        assert!(result.is_ok());
        assert!(validate_control_fds(ready[0], release[1]).is_err());
        assert!(validate_control_fds(3, 3).is_err());
        assert!(validate_control_fds(0, 4).is_err());
    }

    #[test]
    fn release_requires_exact_byte_then_eof_and_rejects_bad_framing() {
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::write(pipe[1], b"G".as_ptr().cast(), 1) }, 1);
        unsafe { libc::close(pipe[1]) };
        assert!(wait_frame(pipe[0], b'G', "RELEASE", monotonic_ns().unwrap() + 100_000_000).is_ok());
        unsafe {
            libc::close(pipe[0]);
        }

        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::write(pipe[1], b"X".as_ptr().cast(), 1) }, 1);
        unsafe { libc::close(pipe[1]) };
        assert!(wait_frame(pipe[0], b'G', "RELEASE", monotonic_ns().unwrap() + 100_000_000).is_err());
        unsafe {
            libc::close(pipe[0]);
        }

        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        unsafe { libc::close(pipe[1]) };
        assert!(wait_frame(pipe[0], b'G', "RELEASE", monotonic_ns().unwrap() + 100_000_000).is_err());
        unsafe { libc::close(pipe[0]) };

        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        assert!(wait_frame(pipe[0], b'G', "RELEASE", monotonic_ns().unwrap() + 1_000_000).is_err());
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
        }

        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::write(pipe[1], b"GX".as_ptr().cast(), 2) }, 2);
        unsafe { libc::close(pipe[1]) };
        assert!(wait_frame(pipe[0], b'G', "RELEASE", monotonic_ns().unwrap() + 100_000_000).is_err());
        unsafe { libc::close(pipe[0]) };

        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { libc::write(pipe[1], b"G".as_ptr().cast(), 1) }, 1);
        assert!(wait_frame(pipe[0], b'G', "RELEASE", monotonic_ns().unwrap() + 1_000_000).is_err());
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
        }
    }
}
