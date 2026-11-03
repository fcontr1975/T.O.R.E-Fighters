//! Finding the master again (0.2.2): a host's listing against a master with
//! several addresses, on a fake transport that can refuse sends the way a
//! host without a route does (ENETUNREACH) and can lack a family, and a fake
//! master that answers as the master protocol says from each of its
//! addresses that is up.

use std::collections::{BTreeMap, BTreeSet};

use super::*;
use crate::master::packet::{Challenge, HeartbeatAck, Listed, UnknownListing};
use crate::master::{CookieKey, MasterKind};
use crate::packet::DiscoverPhase;

const HOST_V4: &str = "203.0.113.5:26900";
const HOST_V6: &str = "[2001:db8:5::20]:26900";
const A: &str = "198.51.100.1:26901";
const B: &str = "198.51.100.2:26901";
const C: &str = "198.51.100.3:26901";
const V6: &str = "[2600:3c00::1]:26901";

fn address(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

fn summary() -> ListingSummary {
    ListingSummary {
        protocol_version: 7,
        phase: DiscoverPhase::Lobby,
        players: 1,
        capacity: 15,
        session_id: 99,
        game_version: "0.2.2".into(),
        game_commit: "abc".into(),
        name: "Co-op one".into(),
        mission: "UKR, 2 v 2".into(),
        callsigns: vec!["Pilot0".into()],
        ..ListingSummary::default()
    }
}

fn config() -> HostRendezvous {
    HostRendezvous {
        build: Build {
            protocol_version: 7,
            game_version: "0.2.2".into(),
            game_commit: "abc".into(),
            compatibility: 0,
            release: false,
        },
        dedicated: true,
        install_id: None,
        platform: 3,
        entropy: Entropy::Seeded(21),
    }
}

/// The game port: which families it has, which addresses it has no route
/// to, and every datagram it sent.
struct FakePort {
    v4: bool,
    v6: bool,
    unreachable: BTreeSet<IpAddr>,
    /// Every datagram that left: to where and its master kind.
    sent: Vec<(SocketAddr, Vec<u8>)>,
    /// Every send that failed, by address.
    refused: Vec<SocketAddr>,
}

impl FakePort {
    fn dual() -> Self {
        Self {
            v4: true,
            v6: true,
            unreachable: BTreeSet::new(),
            sent: Vec::new(),
            refused: Vec::new(),
        }
    }
}

impl Datagrams for FakePort {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        if self.unreachable.contains(&to.ip()) {
            self.refused.push(to);
            return Err(io::Error::from(io::ErrorKind::NetworkUnreachable));
        }
        self.sent.push((to, datagram.to_vec()));
        Ok(())
    }

    fn recv_datagram(&mut self, _buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        Ok(None)
    }

    fn can_send_to(&self, to: SocketAddr) -> bool {
        match to {
            SocketAddr::V4(_) => self.v4,
            SocketAddr::V6(_) => self.v6,
        }
    }
}

/// A master with several addresses, each up or down, answering Registers,
/// Heartbeats and Keeps as the protocol says.
struct FakeMaster {
    key: CookieKey,
    up: BTreeSet<SocketAddr>,
    /// Listings by token: the listing id.
    listings: BTreeMap<u64, u64>,
    next_id: u64,
    /// Every request that reached an address that is up: when, where, what.
    heard: Vec<(Duration, SocketAddr, MasterKind)>,
}

impl FakeMaster {
    fn new(up: &[&str]) -> Self {
        Self {
            key: CookieKey::new(Entropy::Seeded(5)),
            up: up.iter().map(|a| address(a)).collect(),
            listings: BTreeMap::new(),
            next_id: 1,
            heard: Vec::new(),
        }
    }

    /// Forgets every listing, as a restarted master.
    fn restart(&mut self) {
        self.listings.clear();
        self.key = CookieKey::new(Entropy::Seeded(6));
    }

    fn answer(&mut self, now: Duration, to: SocketAddr, datagram: &[u8]) -> Option<MasterPacket> {
        if !self.up.contains(&to) {
            return None;
        }
        let packet = MasterPacket::decode(datagram).expect("the host sends good packets");
        self.heard.push((now, to, packet.kind()));
        let seen = if to.is_ipv4() {
            address(HOST_V4)
        } else {
            address(HOST_V6)
        };
        match packet {
            MasterPacket::Register(register) => {
                if !self.key.check(seen, register.nonce, register.cookie, now) {
                    return Some(MasterPacket::Challenge(Challenge {
                        nonce: register.nonce,
                        cookie: self.key.cookie(seen, register.nonce, now),
                    }));
                }
                let listing_id = self.next_id;
                self.next_id += 1;
                let token = listing_id * 1_000_003;
                self.listings.insert(token, listing_id);
                Some(MasterPacket::Listed(Listed {
                    nonce: register.nonce,
                    listing_id,
                    token,
                    seen,
                    heartbeat_secs: 30,
                    keep_secs: 15,
                    expiry_secs: 90,
                }))
            }
            MasterPacket::Heartbeat(heartbeat) => Some(match self.listings.get(&heartbeat.token) {
                Some(&listing_id) => MasterPacket::HeartbeatAck(HeartbeatAck { listing_id, seen }),
                None => MasterPacket::UnknownListing(UnknownListing {
                    token: heartbeat.token,
                }),
            }),
            MasterPacket::Keep(keep) if !self.listings.contains_key(&keep.token) => {
                Some(MasterPacket::UnknownListing(UnknownListing {
                    token: keep.token,
                }))
            }
            _ => None,
        }
    }

    fn listed(&self) -> bool {
        !self.listings.is_empty()
    }
}

/// A host listing through a fake port to a fake master, in 10 ms steps.
struct Rig {
    now: Duration,
    rendezvous: Rendezvous,
    port: FakePort,
    master: FakeMaster,
    /// How much of the port's log the master has read.
    read: usize,
    events: Vec<(Duration, RendezvousEvent)>,
    /// The master's address in use after each step.
    used: Vec<SocketAddr>,
}

impl Rig {
    fn new(masters: &[&str], up: &[&str], port: FakePort) -> Self {
        Self::routed(masters, up, port, |_| true)
    }

    fn routed(
        masters: &[&str],
        up: &[&str],
        port: FakePort,
        routed: impl Fn(SocketAddr) -> bool,
    ) -> Self {
        let now = Duration::ZERO;
        let mut rendezvous = Rendezvous::host(config(), now);
        let local = Candidate::new(CandidateKind::Local, address(HOST_V4));
        rendezvous.set_masters_routed(
            masters.iter().map(|a| address(a)).collect(),
            vec![local],
            routed,
            now,
        );
        rendezvous.set_listed(true, now);
        Self {
            now,
            rendezvous,
            port,
            master: FakeMaster::new(up),
            read: 0,
            events: Vec::new(),
            used: Vec::new(),
        }
    }

    fn step(&mut self) {
        self.now += Duration::from_millis(10);
        let now = self.now;
        if self.rendezvous.wants_summary(now) {
            self.rendezvous.set_summary(now, summary());
        }
        self.rendezvous.update(now);
        self.rendezvous.transmit(&mut self.port).unwrap();
        // The master answers within the step (no delay: the timing under
        // test is the host's own).
        while self.read < self.port.sent.len() {
            let (to, datagram) = self.port.sent[self.read].clone();
            self.read += 1;
            if let Some(answer) = self.master.answer(now, to, &datagram) {
                self.rendezvous.receive(now, to, &answer.encode().unwrap());
            }
        }
        while let Some(event) = self.rendezvous.poll_event() {
            self.events.push((now, event));
        }
        self.used.push(self.rendezvous.master().unwrap());
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * 100.0).round() as u64 {
            self.step();
        }
    }

    /// Runs until `done`, at most `seconds`; how long it took.
    fn run_until(&mut self, seconds: f64, mut done: impl FnMut(&Self) -> bool) -> Option<f64> {
        let start = self.now;
        for _ in 0..(seconds * 100.0).round() as u64 {
            self.step();
            if done(self) {
                return Some((self.now - start).as_secs_f64());
            }
        }
        None
    }

    fn listed(&self) -> bool {
        matches!(self.rendezvous.state(), ListingState::Listed { .. })
    }

    /// The addresses each Register or Heartbeat went to, in order.
    fn requests(&self) -> Vec<SocketAddr> {
        self.port
            .sent
            .iter()
            .filter(|(_, d)| {
                matches!(
                    crate::master::packet::peek_kind(d),
                    Some(MasterKind::Register | MasterKind::Heartbeat)
                )
            })
            .map(|(to, _)| *to)
            .collect()
    }

    fn switches(&self) -> Vec<(SocketAddr, SocketAddr, MasterSwitch)> {
        self.events
            .iter()
            .filter_map(|(_, e)| match e {
                RendezvousEvent::MasterAddress { from, to, why } => Some((*from, *to, why.clone())),
                _ => None,
            })
            .collect()
    }
}

#[test]
fn a_silent_master_is_asked_at_each_address_in_turn_and_back_to_the_first() {
    let mut rig = Rig::new(&[A, B, C], &[], FakePort::dual());
    rig.run(120.0);
    assert_eq!(rig.rendezvous.state(), ListingState::Silent);
    // Every 3 s to A until the silence at 10 s; then every request goes to
    // the next address, round and back to A.
    let requests = rig.requests();
    assert_eq!(&requests[..4], &[address(A); 4]);
    let after: Vec<SocketAddr> = requests[4..].to_vec();
    assert!(after.len() >= 9, "{after:?}");
    let round = [address(B), address(C), address(A)];
    for (n, to) in after.iter().enumerate() {
        assert_eq!(
            *to,
            round[n % 3],
            "request {n} after the silence: {after:?}"
        );
    }
    // One line per change, each "no answer".
    let switches = rig.switches();
    assert_eq!(switches.len(), after.len());
    assert!(
        switches
            .iter()
            .all(|(from, to, why)| from != to && *why == MasterSwitch::NoAnswer)
    );
    // The silent host never asks faster than a Keep's interval once the
    // back-off has grown.
    assert_eq!(MAX_BACKOFF, KEEP_INTERVAL);
}

#[test]
fn the_first_address_answering_again_lists_the_game_after_the_round_wraps() {
    let mut rig = Rig::new(&[A, B], &[], FakePort::dual());
    rig.run(60.0);
    assert_eq!(rig.rendezvous.state(), ListingState::Silent);
    // Only A comes back: within two waits of 15 s the round reaches it.
    rig.master.up.insert(address(A));
    let took = rig
        .run_until(32.0, |rig| rig.listed())
        .expect("listed again at A");
    assert!(took <= 2.0 * MAX_BACKOFF.as_secs_f64() + 0.1, "{took}");
    assert_eq!(rig.rendezvous.master(), Some(address(A)));
    // And it stays there: the next minutes' heartbeats all go to A.
    let before = rig.requests().len();
    rig.run(120.0);
    assert!(rig.listed());
    assert!(rig.requests()[before..].iter().all(|to| *to == address(A)));
}

#[test]
fn a_failed_send_moves_to_the_next_address_at_once() {
    let mut port = FakePort::dual();
    port.unreachable.insert(address(V6).ip());
    let mut rig = Rig::new(&[V6, A], &[V6, A], port);
    // The first Register to the IPv6 address fails; the next update sends
    // one to A, which lists the game, all well inside the 10-second silence.
    let took = rig
        .run_until(1.0, |rig| rig.listed())
        .expect("listed at once");
    assert!(took < 0.1, "{took}");
    assert_eq!(rig.rendezvous.master(), Some(address(A)));
    let switches = rig.switches();
    assert_eq!(switches.len(), 1, "{switches:?}");
    assert_eq!((switches[0].0, switches[0].1), (address(V6), address(A)));
    assert!(matches!(&switches[0].2, MasterSwitch::SendFailed(text) if !text.is_empty()));
    assert!(
        !rig.events
            .iter()
            .any(|(_, e)| *e == RendezvousEvent::MasterSilent)
    );
    // A minute of listing later nothing more goes to the dead address.
    let refused = rig.port.refused.len();
    rig.run(60.0);
    assert!(rig.listed());
    assert_eq!(rig.port.refused.len(), refused);
}

#[test]
fn a_send_failing_to_the_only_address_is_said_once_not_per_packet() {
    let mut port = FakePort::dual();
    port.unreachable.insert(address(A).ip());
    let mut rig = Rig::new(&[A], &[A], port);
    rig.run(120.0);
    assert!(rig.port.refused.len() > 10, "{}", rig.port.refused.len());
    let failed: Vec<_> = rig
        .events
        .iter()
        .filter(|(_, e)| matches!(e, RendezvousEvent::SendFailed { .. }))
        .collect();
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(rig.switches().is_empty());
    // The route comes back: the next request lists the game.
    rig.port.unreachable.clear();
    rig.run_until(16.0, |rig| rig.listed())
        .expect("listed once a send gets through");
}

#[test]
fn an_ipv4_only_socket_never_picks_an_ipv6_address() {
    let mut port = FakePort::dual();
    port.v6 = false;
    let mut rig = Rig::new(&[A, V6], &[], port);
    rig.run(300.0);
    assert_eq!(rig.rendezvous.state(), ListingState::Silent);
    assert!(rig.used.iter().all(|used| *used == address(A)));
    assert!(rig.port.sent.iter().all(|(to, _)| to.is_ipv4()));
    assert!(rig.switches().is_empty());
    // The IPv6 address first, as a resolver might give it: nothing is ever
    // sent there, and the listing is on A from the first update.
    let mut port = FakePort::dual();
    port.v6 = false;
    let mut rig = Rig::new(&[V6, A], &[V6, A], port);
    rig.run_until(1.0, |rig| rig.listed())
        .expect("listed at A at once");
    rig.run(300.0);
    assert!(rig.port.sent.iter().all(|(to, _)| to.is_ipv4()));
    assert_eq!(rig.rendezvous.master(), Some(address(A)));
}

#[test]
fn an_address_without_a_route_is_tried_only_when_nothing_else_will_do() {
    let no_v6 = |master: SocketAddr| master.is_ipv4();
    let mut rig = Rig::routed(&[A, V6], &[], FakePort::dual(), no_v6);
    rig.run(120.0);
    assert!(rig.used.iter().all(|used| *used == address(A)));
    // With no IPv4 address at all, the routeless one is still tried.
    let mut rig = Rig::routed(&[V6], &[V6], FakePort::dual(), no_v6);
    rig.run_until(1.0, |rig| rig.listed())
        .expect("listed at its only address");
}

/// The servers of 2026-10-23: listed at the master's IPv4 address, the
/// uplink saturated so the master's answers stop, the silence moves the
/// listing to its IPv6 address, which this host has no route to (and,
/// worst case, the route check missed it). The listing must come back to
/// IPv4 and be listed again once the uplink clears.
#[test]
fn the_unlisted_servers_of_2026_10_23_list_themselves_again() {
    let mut port = FakePort::dual();
    port.unreachable.insert(address(V6).ip());
    let mut rig = Rig::new(&[A, V6], &[A], port);
    rig.run(5.0);
    assert!(rig.listed());
    // The uplink is saturated for two minutes.
    rig.master.up.clear();
    rig.run(120.0);
    assert_eq!(rig.rendezvous.state(), ListingState::Silent);
    assert_eq!(rig.rendezvous.master(), Some(address(A)));
    // The one failed send was said once; then the round stays on A.
    assert_eq!(rig.port.refused.len(), 1, "{:?}", rig.port.refused);
    rig.master.up.insert(address(A));
    let took = rig
        .run_until(20.0, |rig| rig.listed())
        .expect("listed again");
    assert!(took <= MAX_BACKOFF.as_secs_f64() + 0.1, "{took}");
}

#[test]
fn a_restarted_master_lists_the_game_again_within_seconds_of_coming_back() {
    let mut rig = Rig::new(&[A, B], &[A, B], FakePort::dual());
    rig.run(5.0);
    assert!(rig.listed());
    // Down for ten minutes, then back with nothing listed.
    rig.master.up.clear();
    rig.run(600.0);
    assert_eq!(rig.rendezvous.state(), ListingState::Silent);
    rig.master.restart();
    rig.master.up = [address(A), address(B)].into();
    let took = rig
        .run_until(20.0, |rig| rig.master.listed() && rig.listed())
        .expect("registered again");
    assert!(took <= MAX_BACKOFF.as_secs_f64() + 0.1, "{took}");
    // It took a new Register (the old token was unknown), and a second
    // Listed event says so.
    let listed = rig
        .events
        .iter()
        .filter(|(_, e)| matches!(e, RendezvousEvent::Listed { .. }))
        .count();
    assert_eq!(listed, 2);
    assert!(
        rig.master
            .heard
            .iter()
            .any(|(at, _, kind)| *at > Duration::from_secs(605) && *kind == MasterKind::Register)
    );
}

#[test]
fn an_answer_makes_its_address_the_one_in_use() {
    let mut rig = Rig::new(&[A, B], &[], FakePort::dual());
    rig.run(13.0);
    // The silence moved the listing to B; a late Challenge from A brings
    // it back, and the Register with the cookie goes to A.
    assert_eq!(rig.rendezvous.master(), Some(address(B)));
    rig.master.up.insert(address(A));
    let nonce = rig.rendezvous.nonce;
    let challenge = MasterPacket::Challenge(Challenge {
        nonce,
        cookie: rig.master.key.cookie(address(HOST_V4), nonce, rig.now),
    });
    let now = rig.now;
    rig.rendezvous
        .receive(now, address(A), &challenge.encode().unwrap());
    assert_eq!(rig.rendezvous.master(), Some(address(A)));
    rig.run_until(1.0, |rig| rig.listed()).expect("listed at A");
    assert!(
        rig.switches()
            .iter()
            .any(|(_, to, why)| *to == address(A) && *why == MasterSwitch::Answered)
    );
}

#[test]
fn a_lookup_does_not_pin_a_dead_address() {
    let mut rig = Rig::new(&[A, V6], &[A], FakePort::dual());
    rig.run(5.0);
    assert!(rig.listed());
    // A listed host's silence starts with its next Heartbeat (30 s), and is
    // silence 10 s after; the next request, 2 s on, goes to the IPv6 address.
    rig.master.up.clear();
    rig.run(38.0);
    assert_eq!(rig.rendezvous.state(), ListingState::Silent);
    assert_eq!(rig.rendezvous.master(), Some(address(V6)));
    rig.master.up.insert(address(A));
    // A lookup gives the same two addresses, the IPv6 one without a route
    // now: the listing moves to A at once, keeping its listing.
    let now = rig.now;
    let candidates = rig.rendezvous.own.clone();
    rig.rendezvous.set_masters_routed(
        vec![address(A), address(V6)],
        candidates,
        |master| master.is_ipv4(),
        now,
    );
    assert_eq!(rig.rendezvous.master(), Some(address(A)));
    assert!(rig.rendezvous.listing.is_some());
    rig.run_until(1.0, |rig| rig.listed())
        .expect("listed again at once");
    assert!(
        rig.switches()
            .iter()
            .any(|(_, to, why)| *to == address(A) && *why == MasterSwitch::Lookup)
    );
    // A lookup that gives the address in use, usable, keeps it.
    let switches = rig.switches().len();
    let candidates = rig.rendezvous.own.clone();
    rig.rendezvous
        .set_masters(vec![address(A), address(V6)], candidates, rig.now);
    assert_eq!(rig.rendezvous.master(), Some(address(A)));
    assert_eq!(rig.switches().len(), switches);
}

/// The listing looks the master's name up again every minute while the
/// master is silent, and every 10 minutes while it answers.
#[test]
fn a_silent_master_is_looked_up_again_every_minute() {
    let mut listing = HostListing::new("127.0.0.1:9", config(), 26900, Duration::ZERO)
        .expect("a literal address");
    listing.set_listed(true, Duration::ZERO);
    let mut port = FakePort::dual();
    let mut now = Duration::ZERO;
    let mut lookups = Vec::new();
    let mut looking = false;
    let started = std::time::Instant::now();
    while now < Duration::from_secs(200) {
        listing.update(now, summary);
        listing.transmit(&mut port).unwrap();
        if listing.lookup.is_some() && !looking {
            lookups.push(now.as_secs());
        }
        looking = listing.lookup.is_some();
        if looking {
            // The lookup's thread answers in real time: wait for it.
            std::thread::sleep(Duration::from_millis(1));
            assert!(started.elapsed() < Duration::from_secs(20));
        } else {
            now += Duration::from_millis(100);
        }
    }
    assert_eq!(listing.state(), ListingState::Silent);
    // At the start, at the silence (10 s), then every minute.
    assert_eq!(lookups, vec![0, 10, 70, 130, 190], "{lookups:?}");
}
