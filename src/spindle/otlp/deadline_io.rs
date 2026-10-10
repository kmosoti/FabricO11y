//! Absolute socket deadlines beneath BufReader/read_exact/write_all.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn expired() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "OTLP I/O deadline exceeded")
}

pub(super) fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(expired)
}

fn normalize(error: io::Error) -> io::Error {
    if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) {
        expired()
    } else {
        error
    }
}

pub(super) struct DeadlineStream {
    stream: TcpStream,
    deadline: Instant,
}

impl DeadlineStream {
    pub(super) fn new(stream: TcpStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }

    pub(super) fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = deadline;
    }
}

impl Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.stream.set_read_timeout(Some(remaining(self.deadline)?))?;
        let result = self.stream.read(buffer).map_err(normalize);
        remaining(self.deadline)?;
        result
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.stream.set_write_timeout(Some(remaining(self.deadline)?))?;
        let result = self.stream.write(buffer).map_err(normalize);
        remaining(self.deadline)?;
        result
    }

    fn flush(&mut self) -> io::Result<()> {
        remaining(self.deadline)?;
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Shutdown, TcpListener};
    use std::os::fd::AsRawFd;
    use std::sync::mpsc;

    #[test]
    fn security_expired_deadline_is_not_a_zero_socket_timeout() {
        assert_eq!(remaining(Instant::now()).unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn security_nonreading_peer_cannot_block_response_writes_forever() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let size: libc::c_int = 4096;
        // SAFETY: valid live socket and initialized integer with the supplied
        // size; this changes only the synthetic test socket's send buffer.
        assert_eq!(unsafe {
            libc::setsockopt(
                server.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                &size as *const libc::c_int as *const libc::c_void,
                std::mem::size_of_val(&size) as libc::socklen_t,
            )
        }, 0);
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut writer = DeadlineStream::new(server, Instant::now() + Duration::from_millis(150));
            let result = writer.write_all(&vec![0; 4 * 1024 * 1024]);
            let _ = tx.send(result.map_err(|error| error.kind()));
        });
        let observed = rx.recv_timeout(Duration::from_secs(2));
        // Unblock the worker before asserting, including on a defective
        // implementation without a write timeout. No detached test threads.
        let _ = client.shutdown(Shutdown::Both);
        worker.join().unwrap();
        assert!(matches!(observed, Ok(Err(io::ErrorKind::TimedOut))));
    }
}
