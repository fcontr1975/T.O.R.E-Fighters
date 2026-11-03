//! A server's UDP sockets, shared by the dedicated server and the game that
//! hosts (moved here from `tore-server` in slice EF3).
//!
//! `Listen::Any` listens on every IPv4 and IPv6 address: one IPv6 socket that
//! takes IPv4 as well where the system allows it (Linux and macOS), and an
//! IPv4 socket beside it where it does not (Windows, whose IPv6 socket is
//! IPv6 only, and systems with no IPv6). The standard library cannot set or
//! read the IPv6-only option, so the program tries the IPv6 socket first,
//! then the IPv4 one, and takes "address in use" on the IPv4 one to mean the
//! IPv6 socket covers IPv4.
//!
//! That is only true when nothing else holds the port for IPv4. A socket on
//! `0.0.0.0` (a game's search on the game port, a server listening on IPv4
//! only) makes Linux refuse the IPv6 socket too, but macOS and Windows bind
//! it beside the other socket, and IPv4 datagrams then go to the other
//! socket: before slice EF-X the server listened on IPv6 alone there and
//! IPv4 players never reached it. So on "address in use" the program frees
//! its IPv6 socket and tries IPv4 alone: if that binds, the IPv6 socket was
//! covering it and is bound again; if not, another socket holds the port and
//! the bind fails, on every system (checked on the CI runners of all three,
//! 2026-10-01).

use crate::datagram::{Datagrams, bind_udp};
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket},
};

/// Where a server listens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Listen {
    /// Every IPv4 and IPv6 address the machine has.
    Any,
    /// One local address.
    Address(IpAddr),
}

/// One or two bound, non-blocking sockets behind the transport's datagram trait.
#[derive(Debug)]
pub struct ServerSocket {
    v6: Option<UdpSocket>,
    v4: Option<UdpSocket>,
}

impl ServerSocket {
    /// Binds the sockets for `listen` on `port`.
    pub fn bind(listen: Listen, port: u16) -> io::Result<Self> {
        match listen {
            Listen::Address(IpAddr::V4(ip)) => Ok(Self {
                v6: None,
                v4: Some(bind_udp(SocketAddr::new(IpAddr::V4(ip), port))?),
            }),
            Listen::Address(IpAddr::V6(ip)) => Ok(Self {
                v6: Some(bind_udp(SocketAddr::new(IpAddr::V6(ip), port))?),
                v4: None,
            }),
            Listen::Any => bind_any(port),
        }
    }

    /// The addresses the sockets are bound to, for the start lines.
    pub fn local_addresses(&self) -> Vec<SocketAddr> {
        [&self.v6, &self.v4]
            .into_iter()
            .flatten()
            .filter_map(|socket| socket.local_addr().ok())
            .collect()
    }

    /// Another handle to the same sockets (slice J3), for a thread that
    /// sends on them: a relayed game's keepalive thread
    /// ([`crate::Keepalive`]) gets one wrapped in
    /// [`crate::master::RelayFraming`]. Both handles send from the same
    /// ports and stay non-blocking.
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            v6: self.v6.as_ref().map(UdpSocket::try_clone).transpose()?,
            v4: self.v4.as_ref().map(UdpSocket::try_clone).transpose()?,
        })
    }
}

/// `Listen::Any`'s sockets on `port`; see the module documentation.
fn bind_any(port: u16) -> io::Result<ServerSocket> {
    let mut v6_address = SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port);
    let v6 = match bind_udp(v6_address) {
        Ok(v6) => v6,
        // Another socket holds the port (on Linux, an IPv4 one does too).
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Err(error),
        // No IPv6 on this machine.
        Err(_) => {
            return Ok(ServerSocket {
                v6: None,
                v4: Some(bind_udp(SocketAddr::new(
                    Ipv4Addr::UNSPECIFIED.into(),
                    port,
                ))?),
            });
        }
    };
    // Port 0 asks for any free port: the IPv4 socket takes the same one.
    v6_address.set_port(v6.local_addr()?.port());
    let v4_address = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), v6_address.port());
    match bind_udp(v4_address) {
        Ok(v4) => Ok(ServerSocket {
            v6: Some(v6),
            v4: Some(v4),
        }),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            // Whose is the IPv4 port: the IPv6 socket's own, or another's?
            drop(v6);
            drop(bind_udp(v4_address)?);
            // The IPv6 socket took IPv4 too; take it again.
            Ok(ServerSocket {
                v6: Some(bind_udp(v6_address)?),
                v4: None,
            })
        }
        Err(error) => Err(error),
    }
}

impl Datagrams for ServerSocket {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        // An IPv4 peer goes out of the IPv4 socket when there is one; else the
        // IPv6 socket sends to its IPv4-mapped address.
        match (&mut self.v4, &mut self.v6, to) {
            (Some(v4), _, SocketAddr::V4(_)) => v4.send_datagram(to, datagram),
            (_, Some(v6), SocketAddr::V6(_)) => v6.send_datagram(to, datagram),
            (None, Some(v6), SocketAddr::V4(v4_to)) => {
                let mapped = SocketAddr::new(v4_to.ip().to_ipv6_mapped().into(), v4_to.port());
                v6.send_datagram(mapped, datagram)
            }
            _ => Ok(()),
        }
    }

    fn can_send_to(&self, to: SocketAddr) -> bool {
        // An IPv6 socket sends to an IPv4 address at its IPv4-mapped one.
        match to {
            SocketAddr::V4(_) => self.v4.is_some() || self.v6.is_some(),
            SocketAddr::V6(_) => self.v6.is_some(),
        }
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        for socket in [&mut self.v6, &mut self.v4].into_iter().flatten() {
            if let Some(received) = socket.recv_datagram(buf)? {
                return Ok(Some(received));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn receive(socket: &mut ServerSocket) -> Option<(Vec<u8>, SocketAddr)> {
        let mut buf = [0u8; 64];
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if let Some((length, from)) = socket.recv_datagram(&mut buf).unwrap() {
                return Some((buf[..length].to_vec(), from));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        None
    }

    #[test]
    fn a_loopback_socket_receives_and_answers() {
        let mut server =
            ServerSocket::bind(Listen::Address(Ipv4Addr::LOCALHOST.into()), 0).unwrap();
        let address = server.local_addresses()[0];
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.send_to(b"hello", address).unwrap();
        let (bytes, from) = receive(&mut server).expect("a datagram arrives");
        assert_eq!(bytes, b"hello");
        server.send_datagram(from, b"hi").unwrap();
        let mut buf = [0u8; 16];
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (length, _) = client.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..length], b"hi");
        assert!(server.recv_datagram(&mut buf).unwrap().is_none());
    }

    #[test]
    fn a_busy_port_is_an_error() {
        let first = ServerSocket::bind(Listen::Address(Ipv4Addr::LOCALHOST.into()), 0).unwrap();
        let port = first.local_addresses()[0].port();
        assert!(ServerSocket::bind(Listen::Address(Ipv4Addr::LOCALHOST.into()), port).is_err());
    }

    #[test]
    fn any_listens_for_ipv4_clients() {
        // Find a free port first; the two sockets of `any` share it.
        let port = {
            let probe = UdpSocket::bind("0.0.0.0:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        let Ok(mut server) = ServerSocket::bind(Listen::Any, port) else {
            return; // the port was taken in between; nothing to learn
        };
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.send_to(b"x", ("127.0.0.1", port)).unwrap();
        let (bytes, _) = receive(&mut server).expect("an IPv4 datagram reaches `any`");
        assert_eq!(bytes, b"x");
    }

    /// `Listen::Any` on a free port: one dual-stack IPv6 socket on Linux and
    /// macOS, an IPv6 socket and an IPv4 one on Windows, all on one port even
    /// when port 0 asks for any (CI runners, 2026-10-01).
    #[test]
    fn any_binds_one_socket_or_two_on_one_port() {
        let server = ServerSocket::bind(Listen::Any, 0).unwrap();
        let addresses = server.local_addresses();
        let port = addresses[0].port();
        assert!(
            addresses
                .iter()
                .all(|a| a.port() == port && a.ip().is_unspecified()),
            "{addresses:?}"
        );
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert_eq!(addresses, [SocketAddr::from((Ipv6Addr::UNSPECIFIED, port))]);
        }
        if cfg!(windows) {
            assert_eq!(
                addresses,
                [
                    SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
                    SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
                ]
            );
        }
    }

    /// A socket holding the port for IPv4, as a game's search holds the game
    /// port, makes `Listen::Any` refuse the port on every system; macOS and
    /// Windows would bind the IPv6 socket beside it (slice EF-X). The other
    /// way round, `Listen::Any` keeps such a socket off its port.
    #[test]
    fn any_refuses_a_port_another_socket_holds_for_ipv4() {
        let search = UdpSocket::bind("0.0.0.0:0").unwrap();
        let port = search.local_addr().unwrap().port();
        let refused = ServerSocket::bind(Listen::Any, port).expect_err("the port is held");
        assert_eq!(refused.kind(), io::ErrorKind::AddrInUse);
        drop(search);
        let Ok(server) = ServerSocket::bind(Listen::Any, port) else {
            return; // the port was taken in between; nothing to learn
        };
        assert!(!server.local_addresses().is_empty());
        assert!(UdpSocket::bind(("0.0.0.0", port)).is_err());
    }

    /// A clone sends from the same port and stays non-blocking, and what it
    /// sends to a peer is answered to the original.
    #[test]
    fn a_clone_sends_from_the_same_port() {
        let mut server =
            ServerSocket::bind(Listen::Address(Ipv4Addr::LOCALHOST.into()), 0).unwrap();
        let mut clone = server.try_clone().unwrap();
        assert_eq!(clone.local_addresses(), server.local_addresses());
        let mut buf = [0u8; 16];
        assert!(
            clone.recv_datagram(&mut buf).unwrap().is_none(),
            "non-blocking"
        );
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        clone
            .send_datagram(peer.local_addr().unwrap(), b"keep")
            .unwrap();
        let (length, from) = peer.recv_from(&mut buf).unwrap();
        assert_eq!(
            (&buf[..length], from),
            (&b"keep"[..], server.local_addresses()[0])
        );
        peer.send_to(b"back", from).unwrap();
        let (bytes, _) = receive(&mut server).expect("the original reads the answer");
        assert_eq!(bytes, b"back");
    }

    /// Discovery's question to a host on the network is a datagram to the
    /// limited broadcast address on the game port. A dual-stack IPv6 socket
    /// takes IPv4 broadcast on Linux, so `Listen::Any` needs no IPv4 socket of
    /// its own for discovery (agent finding, 2026-10-01). The loopback
    /// network's broadcast address stands in here, since a machine's firewall
    /// may drop broadcast on its real interfaces (ufw does until the port is
    /// allowed); the test only asserts on Linux, where it was settled.
    #[test]
    fn any_receives_an_ipv4_broadcast() {
        let port = {
            let probe = UdpSocket::bind("0.0.0.0:0").unwrap();
            probe.local_addr().unwrap().port()
        };
        let Ok(mut server) = ServerSocket::bind(Listen::Any, port) else {
            return;
        };
        let sender = UdpSocket::bind("0.0.0.0:0").unwrap();
        sender.set_broadcast(true).unwrap();
        if sender.send_to(b"who", ("127.255.255.255", port)).is_err() {
            return; // no broadcast route here
        }
        let got = receive(&mut server);
        if cfg!(target_os = "linux") {
            let (bytes, from) = got.expect("an IPv4 broadcast reaches `any`");
            assert_eq!(bytes, b"who");
            // The answer goes back to the asker.
            server.send_datagram(from, b"here").unwrap();
            let mut buf = [0u8; 16];
            sender
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let length = sender.recv(&mut buf).unwrap();
            assert_eq!(&buf[..length], b"here");
        }
    }
}
