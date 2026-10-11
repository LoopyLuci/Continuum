//! Blocking TCP driver over [`Session`], for command-line tools and tests.
//! Interactive viewers should drive [`Session`] from their own event loop.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::session::{Event, Session, SessionOptions};

#[derive(Debug)]
pub enum NetError {
    Io(io::Error),
    Client(crate::ClientError),
    Closed,
    Timeout,
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetError::Io(e) => write!(f, "network: {e}"),
            NetError::Client(e) => write!(f, "{e}"),
            NetError::Closed => write!(f, "server closed the connection"),
            NetError::Timeout => write!(f, "timed out"),
        }
    }
}

impl std::error::Error for NetError {}

impl From<io::Error> for NetError {
    fn from(e: io::Error) -> Self {
        NetError::Io(e)
    }
}

impl From<crate::ClientError> for NetError {
    fn from(e: crate::ClientError) -> Self {
        NetError::Client(e)
    }
}

pub struct Connection {
    sock: TcpStream,
    pub session: Session,
    chunk: Vec<u8>,
}

impl Connection {
    /// Connects and completes the handshake within `timeout`.
    pub fn connect(addr: impl ToSocketAddrs, opts: SessionOptions, timeout: Duration) -> Result<Self, NetError> {
        let mut c = Connection::open(addr, opts, timeout)?;
        let deadline = Instant::now() + timeout;
        while !c.session.is_connected() {
            c.pump_once(deadline)?;
        }
        Ok(c)
    }

    /// Opens the TCP connection without driving the handshake, for callers
    /// that want to stop part-way (e.g. to read the server's certificate).
    pub fn open(addr: impl ToSocketAddrs, opts: SessionOptions, timeout: Duration) -> Result<Self, NetError> {
        let mut last = None;
        let mut sock = None;
        for a in addr.to_socket_addrs()? {
            match TcpStream::connect_timeout(&a, timeout) {
                Ok(s) => {
                    sock = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        let sock = sock.ok_or_else(|| {
            NetError::Io(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address")))
        })?;
        sock.set_nodelay(true)?;
        sock.set_write_timeout(Some(timeout))?;
        Ok(Connection { sock, session: Session::new(opts), chunk: vec![0; 256 * 1024] })
    }

    /// Reads once (until `deadline`), feeds the session, flushes its output.
    pub fn pump_once(&mut self, deadline: Instant) -> Result<Vec<Event>, NetError> {
        self.flush()?;
        let left = deadline.checked_duration_since(Instant::now()).ok_or(NetError::Timeout)?;
        self.sock.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        let n = match self.sock.read(&mut self.chunk) {
            Ok(0) => return Err(NetError::Closed),
            Ok(n) => n,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                return Err(NetError::Timeout)
            }
            Err(e) => return Err(e.into()),
        };
        let events = self.session.receive(&self.chunk[..n])?;
        self.flush()?;
        Ok(events)
    }

    /// Pumps until `pred` returns true for an event, or the timeout passes.
    pub fn wait_for(&mut self, timeout: Duration, mut pred: impl FnMut(&Event) -> bool) -> Result<Event, NetError> {
        let deadline = Instant::now() + timeout;
        loop {
            for ev in self.pump_once(deadline)? {
                if pred(&ev) {
                    return Ok(ev);
                }
            }
        }
    }

    pub fn flush(&mut self) -> Result<(), NetError> {
        let out = self.session.take_output();
        if !out.is_empty() {
            self.sock.write_all(&out)?;
        }
        Ok(())
    }
}
