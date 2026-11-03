use super::{Reply, Request, custody::Receipt};
use crate::observation::MAX_FRAME;
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect as connect_socket,
        socket_with,
    },
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};
fn connect(path: &Path, deadline: Instant) -> Result<UnixStream, String> {
    let fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )
    .map_err(|e| e.to_string())?;
    let addr = SocketAddrUnix::new(path).map_err(|e| e.to_string())?;
    if let Err(e) = connect_socket(&fd, &addr)
        && e != rustix::io::Errno::INPROGRESS
        && e != rustix::io::Errno::AGAIN
    {
        return Err(e.to_string());
    }
    let mut fds = [PollFd::new(&fd, PollFlags::OUT)];
    let wait = deadline
        .checked_duration_since(Instant::now())
        .ok_or("deadline")?;
    let timeout = Timespec {
        tv_sec: wait.as_secs() as _,
        tv_nsec: wait.subsec_nanos() as _,
    };
    if poll(&mut fds, Some(&timeout)).map_err(|e| e.to_string())? == 0 {
        return Err("connect deadline; outcome unknown".into());
    }
    let stream = UnixStream::from(fd);
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    Ok(stream)
}
fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or("deadline; outcome unknown".into())
}
fn write_all(s: &mut UnixStream, mut bytes: &[u8], deadline: Instant) -> Result<(), String> {
    while !bytes.is_empty() {
        s.set_write_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = s
            .write(bytes)
            .map_err(|e| format!("write; outcome unknown: {e}"))?;
        if n == 0 {
            return Err("closed; outcome unknown".into());
        }
        bytes = &bytes[n..]
    }
    Ok(())
}
fn read_exact(s: &mut UnixStream, mut bytes: &mut [u8], deadline: Instant) -> Result<(), String> {
    while !bytes.is_empty() {
        s.set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = s
            .read(bytes)
            .map_err(|e| format!("read; outcome unknown: {e}"))?;
        if n == 0 {
            return Err("closed; outcome unknown".into());
        }
        bytes = &mut bytes[n..]
    }
    Ok(())
}
fn exchange(path: &Path, bytes: &[u8], deadline: Instant) -> Result<Reply, String> {
    if bytes.len() > MAX_FRAME {
        return Err("frame too large".into());
    }
    let mut stream = connect(path, deadline)?;
    write_all(&mut stream, &(bytes.len() as u32).to_be_bytes(), deadline)?;
    write_all(&mut stream, bytes, deadline)?;
    let mut header = [0; 4];
    read_exact(&mut stream, &mut header, deadline)?;
    let size = u32::from_be_bytes(header) as usize;
    if size > super::MAX_REPLY {
        return Err("reply too large".into());
    }
    let mut body = vec![0; size];
    read_exact(&mut stream, &mut body, deadline)?;
    super::wire::check_reply(&body)?;
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}
pub fn submit_bytes(path: &Path, bytes: &[u8], deadline: Instant) -> Result<Receipt, String> {
    // Submit frames are raw v2 observations; control frames have a command field.
    match exchange(path, bytes, deadline)? {
        Reply::Receipt { receipt } => Ok(receipt),
        Reply::Error { error } => Err(error),
        _ => Err("unexpected reply".into()),
    }
}
pub fn request(path: &Path, request: &Request, deadline: Instant) -> Result<Reply, String> {
    exchange(
        path,
        &serde_json::to_vec(request).map_err(|e| e.to_string())?,
        deadline,
    )
}
