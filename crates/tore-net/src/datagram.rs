//! Sending and receiving datagrams without blocking: UDP sockets and the
//! simulator's endpoints behind one small trait, and a real-time clock.

use std::collections::VecDeque;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// A datagram to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transmit {
    /// Where to.
    pub to: SocketAddr,
    /// The bytes.
    pub datagram: Vec<u8>,
}

/// Something that sends and receives datagrams without blocking.
pub trait Datagrams {
    /// Sends one datagram. A send the system cannot take right now is
    /// dropped, as the network might drop it, and is not an error.
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()>;

    /// Receives one waiting datagram into `buf`, or `None` when none is
    /// waiting. A datagram longer than `buf` is cut to its length.
    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>>;

    /// False when this socket has no way to send to `to` at all, for example
    /// an IPv4-only socket and an IPv6 address: a host's listing then does
    /// not pick that one of the master's addresses (0.2.2). True unless a
    /// socket knows better.
    fn can_send_to(&self, to: SocketAddr) -> bool {
        let _ = to;
        true
    }
}

/// Errors that say nothing about this socket: a full send buffer, or Windows
/// reporting that an earlier datagram's peer was unreachable.
fn ignorable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::ConnectionReset | io::ErrorKind::Interrupted
    )
}

/// Windows reports a datagram longer than the buffer as an error
/// (WSAEMSGSIZE) rather than cutting it; it is dropped like any oversize one.
fn oversize(error: &io::Error) -> bool {
    cfg!(windows) && error.raw_os_error() == Some(10040)
}

/// For a socket made non-blocking, for example by [`bind_udp`].
impl Datagrams for UdpSocket {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        match self.send_to(datagram, to) {
            Ok(_) => Ok(()),
            Err(error) if ignorable(&error) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        loop {
            match self.recv_from(buf) {
                Ok(received) => return Ok(Some(received)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) if ignorable(&error) || oversize(&error) => continue,
                Err(error) => return Err(error),
            }
        }
    }
}

/// Binds a UDP socket at `address` (IPv4 or IPv6; port 0 picks a free one)
/// and makes it non-blocking.
pub fn bind_udp(address: SocketAddr) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(address)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// Sends every queued datagram, carrying on past failures; returns the first
/// error.
pub(crate) fn transmit_all<D: Datagrams + ?Sized>(
    queue: &mut VecDeque<Transmit>,
    socket: &mut D,
) -> io::Result<()> {
    let mut first_error = None;
    while let Some(transmit) = queue.pop_front() {
        if let Err(error) = socket.send_datagram(transmit.to, &transmit.datagram) {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// The time since this clock was made, for driving endpoints on a real
/// network, and to wait on ([`crate::wait_until`]). The transport's endpoints
/// never read a clock.
#[derive(Debug, Clone, Copy)]
pub struct RealClock {
    origin: Instant,
}

impl RealClock {
    /// A clock starting at zero now.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    /// The time since the clock was made.
    pub fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}
