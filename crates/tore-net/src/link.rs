//! The in-process link: two ends that pass datagrams to each other inside one
//! process, for a game that hosts a session and flies in it as an ordinary
//! client (docs/ARCHITECTURE.md, "The host inside the game (stage E)").
//!
//! Each end is [`Datagrams`] and may live on its own thread. A datagram sent
//! on one end is ready at the other at once: no delay, no loss, no
//! reordering. Each direction holds at most [`LINK_QUEUE`] datagrams; a send
//! to a full queue, or to an end that has been dropped, is dropped, as a
//! network drops it.
//!
//! The host's transport is its UDP socket and its end of the link together
//! ([`Linked`]): datagrams from the link arrive from [`LINK_ADDRESS`], and
//! sends to that address go into the link. *Agent decision:* the address is
//! `[100::]:0`, in the IPv6 discard-only prefix (RFC 6666) and on port 0,
//! which no UDP sender can have, and [`Linked`] drops any datagram from the
//! socket that claims it, so no real peer can pose as the local player. The
//! handshake treats it as any other address: it has its own rate-limit
//! budget, its own cookies, and an answer is never larger than its request.
//!
//! Not to be confused with the simulator's links ([`crate::sim::LinkConfig`]),
//! which model a network's delay and loss on a virtual clock.

use std::collections::VecDeque;
use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::datagram::Datagrams;

/// The address datagrams from the link come from, at either end.
pub const LINK_ADDRESS: SocketAddr = SocketAddr::V6(SocketAddrV6::new(
    Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 0),
    0,
    0,
    0,
));

/// Datagrams one direction of the link holds at most: about four seconds of a
/// session's traffic at its busiest (agent decision).
pub const LINK_QUEUE: usize = 1024;

/// One direction: the datagrams waiting, and whether the receiving end is
/// still there.
#[derive(Debug, Default)]
struct Queue {
    datagrams: VecDeque<Vec<u8>>,
    closed: bool,
}

fn lock(queue: &Mutex<Queue>) -> MutexGuard<'_, Queue> {
    // A panic while holding the lock leaves the queue whole: every change
    // under it is a single push, pop or flag.
    queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One end of an in-process link. See the module documentation.
#[derive(Debug)]
pub struct LinkEnd {
    incoming: Arc<Mutex<Queue>>,
    outgoing: Arc<Mutex<Queue>>,
    capacity: usize,
    /// Datagrams this end could not send because the other end's queue was
    /// full.
    dropped: u64,
    /// False for a send-only copy ([`LinkEnd::sender`]): it reads nothing
    /// and its drop leaves the link open.
    reads: bool,
}

/// A new link: two ends, each the other's peer, with [`LINK_QUEUE`] datagrams
/// a direction.
pub fn pair() -> (LinkEnd, LinkEnd) {
    pair_with_capacity(LINK_QUEUE)
}

/// [`pair`] with another queue length, at least 1.
pub fn pair_with_capacity(capacity: usize) -> (LinkEnd, LinkEnd) {
    let capacity = capacity.max(1);
    let a_to_b = Arc::new(Mutex::new(Queue::default()));
    let b_to_a = Arc::new(Mutex::new(Queue::default()));
    (
        LinkEnd {
            incoming: Arc::clone(&b_to_a),
            outgoing: Arc::clone(&a_to_b),
            capacity,
            dropped: 0,
            reads: true,
        },
        LinkEnd {
            incoming: a_to_b,
            outgoing: b_to_a,
            capacity,
            dropped: 0,
            reads: true,
        },
    )
}

impl LinkEnd {
    /// Whether the other end still exists.
    pub fn peer_alive(&self) -> bool {
        !lock(&self.outgoing).closed
    }

    /// Datagrams dropped on sending because the other end's queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// A send-only copy of this end, for another thread: what it sends
    /// reaches the other end as this end's sends do, it reads nothing, and
    /// dropping it leaves the link open (a host's loading thread, protocol
    /// 28, slice NET2).
    pub fn sender(&self) -> LinkEnd {
        LinkEnd {
            incoming: Arc::clone(&self.incoming),
            outgoing: Arc::clone(&self.outgoing),
            capacity: self.capacity,
            dropped: 0,
            reads: false,
        }
    }

    /// Datagrams waiting at this end.
    pub fn waiting(&self) -> usize {
        lock(&self.incoming).datagrams.len()
    }
}

impl Drop for LinkEnd {
    fn drop(&mut self) {
        if !self.reads {
            return;
        }
        // Nothing more will be read here; the other end's sends are dropped.
        let mut incoming = lock(&self.incoming);
        incoming.closed = true;
        incoming.datagrams.clear();
    }
}

impl Datagrams for LinkEnd {
    /// Sends to the other end, whatever `to` says: a link has one peer.
    fn send_datagram(&mut self, _to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        let mut queue = lock(&self.outgoing);
        if queue.closed {
            return Ok(());
        }
        if queue.datagrams.len() >= self.capacity {
            self.dropped += 1;
            return Ok(());
        }
        queue.datagrams.push_back(datagram.to_vec());
        Ok(())
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        if !self.reads {
            return Ok(None);
        }
        let Some(datagram) = lock(&self.incoming).datagrams.pop_front() else {
            return Ok(None);
        };
        let length = datagram.len().min(buf.len());
        buf[..length].copy_from_slice(&datagram[..length]);
        Ok(Some((length, LINK_ADDRESS)))
    }
}

/// A socket and the host's end of a link as one transport: the link's
/// datagrams come from [`LINK_ADDRESS`] and sends to it go into the link;
/// everything else is the socket's. The link is read first, so a flood on
/// the socket never holds back the local player.
#[derive(Debug)]
pub struct Linked<D> {
    pub socket: D,
    pub link: LinkEnd,
    /// Datagrams from the socket that claimed the link's address, dropped.
    pub spoofed: u64,
}

impl<D> Linked<D> {
    /// `socket` and `link` together.
    pub fn new(socket: D, link: LinkEnd) -> Self {
        Self {
            socket,
            link,
            spoofed: 0,
        }
    }
}

impl<D: Datagrams> Datagrams for Linked<D> {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        if to == LINK_ADDRESS {
            self.link.send_datagram(to, datagram)
        } else {
            self.socket.send_datagram(to, datagram)
        }
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        if let Some(received) = self.link.recv_datagram(buf)? {
            return Ok(Some(received));
        }
        loop {
            match self.socket.recv_datagram(buf)? {
                Some((_, from)) if from == LINK_ADDRESS => self.spoofed += 1,
                other => return Ok(other),
            }
        }
    }

    fn can_send_to(&self, to: SocketAddr) -> bool {
        to == LINK_ADDRESS || self.socket.can_send_to(to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimNetwork;
    use crate::{
        AcceptInfo, Client, ClientConfig, ClientEvent, ConnectDetails, Decision, Entropy, Platform,
        Server, ServerConfig,
    };
    use std::time::Duration;

    fn receive(end: &mut impl Datagrams) -> Option<(Vec<u8>, SocketAddr)> {
        let mut buf = [0u8; 64];
        end.recv_datagram(&mut buf)
            .unwrap()
            .map(|(length, from)| (buf[..length].to_vec(), from))
    }

    #[test]
    fn datagrams_cross_at_once_in_order_from_the_link_address() {
        let (mut a, mut b) = pair();
        a.send_datagram(LINK_ADDRESS, b"one").unwrap();
        a.send_datagram("10.0.0.1:5".parse().unwrap(), b"two")
            .unwrap();
        assert_eq!(b.waiting(), 2);
        assert_eq!(receive(&mut b), Some((b"one".to_vec(), LINK_ADDRESS)));
        assert_eq!(receive(&mut b), Some((b"two".to_vec(), LINK_ADDRESS)));
        assert_eq!(receive(&mut b), None);
        b.send_datagram(LINK_ADDRESS, b"back").unwrap();
        assert_eq!(receive(&mut a), Some((b"back".to_vec(), LINK_ADDRESS)));
        // A long datagram is cut to the buffer, as a socket cuts it.
        a.send_datagram(LINK_ADDRESS, &[7u8; 100]).unwrap();
        assert_eq!(receive(&mut b).unwrap().0.len(), 64);
    }

    #[test]
    fn a_full_queue_and_a_dropped_end_drop_sends_without_error() {
        let (mut a, b) = pair_with_capacity(2);
        for _ in 0..5 {
            a.send_datagram(LINK_ADDRESS, b"x").unwrap();
        }
        assert_eq!((b.waiting(), a.dropped()), (2, 3));
        assert!(a.peer_alive());
        drop(b);
        assert!(!a.peer_alive());
        a.send_datagram(LINK_ADDRESS, b"y").unwrap();
        assert_eq!(receive(&mut a), None);
    }

    #[test]
    fn a_send_only_copy_sends_as_its_end_reads_nothing_and_leaves_the_link_open() {
        let (mut a, mut b) = pair();
        let mut copy = a.sender();
        copy.send_datagram(LINK_ADDRESS, b"loading").unwrap();
        assert_eq!(receive(&mut b).unwrap().0, b"loading".to_vec());
        b.send_datagram(LINK_ADDRESS, b"for a").unwrap();
        assert_eq!(receive(&mut copy), None);
        drop(copy);
        assert!(b.peer_alive());
        assert_eq!(receive(&mut a).unwrap().0, b"for a".to_vec());
    }

    #[test]
    fn the_two_ends_work_from_two_threads() {
        let (mut a, mut b) = pair();
        let sender = std::thread::spawn(move || {
            for n in 0..500u16 {
                a.send_datagram(LINK_ADDRESS, &n.to_le_bytes()).unwrap();
            }
            a
        });
        let mut seen = Vec::new();
        let started = std::time::Instant::now();
        while seen.len() < 500 && started.elapsed() < Duration::from_secs(10) {
            match receive(&mut b) {
                Some((bytes, _)) => seen.push(u16::from_le_bytes([bytes[0], bytes[1]])),
                None => std::thread::yield_now(),
            }
        }
        let _a = sender.join().unwrap();
        assert_eq!(seen, (0..500).collect::<Vec<_>>());
    }

    #[test]
    fn the_linked_transport_routes_by_address_and_drops_a_socket_posing_as_the_link() {
        let net = SimNetwork::new(1);
        let host_address: SocketAddr = "10.0.0.1:26900".parse().unwrap();
        let socket = net.bind(host_address).unwrap();
        let mut remote = net.bind("10.0.0.2:4000".parse().unwrap()).unwrap();
        let mut spoofer = net.bind(LINK_ADDRESS).unwrap();
        let (host_end, mut local) = pair();
        let mut linked = Linked::new(socket, host_end);

        local.send_datagram(LINK_ADDRESS, b"local").unwrap();
        remote.send_datagram(host_address, b"remote").unwrap();
        spoofer.send_datagram(host_address, b"spoof").unwrap();
        let mut got = Vec::new();
        while let Some(received) = receive(&mut linked) {
            got.push(received);
        }
        assert_eq!(
            got,
            vec![
                (b"local".to_vec(), LINK_ADDRESS),
                (b"remote".to_vec(), "10.0.0.2:4000".parse().unwrap()),
            ]
        );
        assert_eq!(linked.spoofed, 1);

        linked.send_datagram(LINK_ADDRESS, b"to local").unwrap();
        linked
            .send_datagram("10.0.0.2:4000".parse().unwrap(), b"to remote")
            .unwrap();
        assert_eq!(receive(&mut local).unwrap().0, b"to local");
        assert_eq!(receive(&mut remote).unwrap().0, b"to remote");
        assert_eq!(receive(&mut spoofer), None);
    }

    /// The full handshake over the link: the link's address passes the rate
    /// limits and the cookie check like any other, and a client and a remote
    /// one join the same server side by side. The hosting player's own game
    /// names its platform the same way a remote one does.
    #[test]
    fn a_client_joins_a_server_over_the_link_beside_a_remote_one() {
        let net = SimNetwork::new(4);
        let host_address: SocketAddr = "10.0.0.1:26900".parse().unwrap();
        let (host_end, mut local_end) = pair();
        let mut transport = Linked::new(net.bind(host_address).unwrap(), host_end);
        let mut remote_socket = net.bind("10.0.0.2:4000".parse().unwrap()).unwrap();
        let mut server = Server::new(ServerConfig {
            entropy: Entropy::Seeded(5),
            ..ServerConfig::new(1)
        });
        let mut joined = Vec::new();
        let mut gate = |details: &ConnectDetails| {
            joined.push((details.address, details.platform));
            Decision::Accept(AcceptInfo {
                session_id: 1,
                ticks_per_second: 120,
                ticks_per_snapshot: 4,
                host_tick: 0,
            })
        };
        let config = |seed, callsign| ClientConfig {
            entropy: Entropy::Seeded(seed),
            ..ClientConfig::new(1, callsign)
        };
        let mut local = Client::connect(config(6, "Host"), LINK_ADDRESS, net.now()).unwrap();
        // The remote player is on another system than this build's.
        let elsewhere = if Platform::current() == Platform::Windows {
            Platform::Linux
        } else {
            Platform::Windows
        };
        let remote_config = ClientConfig {
            platform: elsewhere,
            ..config(7, "Guest")
        };
        let mut remote = Client::connect(remote_config, host_address, net.now()).unwrap();
        let mut connected = [false, false];
        for _ in 0..200 {
            net.advance(Duration::from_millis(1));
            let now = net.now();
            server.receive_from(&mut transport, now, &mut gate).unwrap();
            server.update(now);
            server.transmit(&mut transport).unwrap();
            for (n, (client, socket)) in [
                (&mut local, &mut local_end as &mut dyn Datagrams),
                (&mut remote, &mut remote_socket as &mut dyn Datagrams),
            ]
            .into_iter()
            .enumerate()
            {
                client.receive_from(socket, now).unwrap();
                client.update(now);
                client.transmit(socket).unwrap();
                while let Some(event) = client.poll_event() {
                    connected[n] |= matches!(event, ClientEvent::Connected(_));
                }
            }
        }
        assert_eq!(connected, [true, true]);
        assert!(joined.contains(&(LINK_ADDRESS, Platform::current())));
        assert!(joined.contains(&("10.0.0.2:4000".parse().unwrap(), elsewhere)));
        assert_eq!(server.counters().rate_limited, 0);
        assert_eq!(server.counters().bad_cookie, 0);
    }
}
