//! The host's rendezvous on the simulator, against a scripted master that
//! answers as the master protocol says: proving addresses, listings,
//! heartbeats, keeps, expiry after 90 seconds, Unknown listing, Unregister and
//! the mapping test's probes.

use std::collections::BTreeMap;

use super::*;
use crate::master::packet::{
    Challenge, HeartbeatAck, Listed, ProbeAnswer, UnknownListing, Unsupported,
};
use crate::master::{CandidateKind, CookieKey, LISTING_EXPIRY, MASTER_VERSION};
use crate::packet::DiscoverPhase;
use crate::sim::{LinkConfig, Prefix, RouterConfig, SimNetwork, SimSocket};

const MASTER: &str = "198.51.100.1:26901";
const HOST: &str = "203.0.113.5:26900";

fn address(text: &str) -> SocketAddr {
    text.parse().unwrap()
}

/// One listing the scripted master keeps.
#[derive(Debug, Clone)]
struct Entry {
    listing_id: u64,
    from: SocketAddr,
    nonce: u64,
    summary: ListingSummary,
    heard: Duration,
}

/// A master that answers as the protocol says, on two sockets.
struct ScriptedMaster {
    main: SimSocket,
    second: SimSocket,
    key: CookieKey,
    listings: BTreeMap<u64, Entry>,
    next_id: u64,
    /// While false the master drops everything, as one that is down.
    up: bool,
    /// Every packet received: when, from where, its kind.
    log: Vec<(Duration, SocketAddr, MasterPacket)>,
}

impl ScriptedMaster {
    fn new(net: &SimNetwork, seed: u64) -> Self {
        let main = address(MASTER);
        Self {
            main: net.bind(main).unwrap(),
            second: net.bind(probe_address(main).unwrap()).unwrap(),
            key: CookieKey::new(Entropy::Seeded(seed)),
            listings: BTreeMap::new(),
            next_id: 1,
            up: true,
            log: Vec::new(),
        }
    }

    /// Forgets every listing and draws a new key, as a restarted master.
    fn restart(&mut self, seed: u64) {
        self.listings.clear();
        self.key = CookieKey::new(Entropy::Seeded(seed));
    }

    fn turn(&mut self, now: Duration) {
        self.listings
            .retain(|_, entry| now < entry.heard + LISTING_EXPIRY);
        let mut buf = [0u8; 2048];
        for second in [false, true] {
            loop {
                let socket = if second {
                    &mut self.second
                } else {
                    &mut self.main
                };
                let Some((length, from)) = socket.recv_datagram(&mut buf).unwrap() else {
                    break;
                };
                if !self.up {
                    continue;
                }
                let Ok(packet) = MasterPacket::decode(&buf[..length]) else {
                    panic!("the host sent a malformed packet");
                };
                self.log.push((now, from, packet.clone()));
                if let Some(answer) = self.answer(now, from, second, packet) {
                    let bytes = answer.encode().unwrap();
                    let socket = if second {
                        &mut self.second
                    } else {
                        &mut self.main
                    };
                    socket.send_datagram(from, &bytes).unwrap();
                }
            }
        }
    }

    fn answer(
        &mut self,
        now: Duration,
        from: SocketAddr,
        second: bool,
        packet: MasterPacket,
    ) -> Option<MasterPacket> {
        if second {
            let MasterPacket::Probe(probe) = packet else {
                return None;
            };
            return Some(MasterPacket::ProbeAnswer(ProbeAnswer {
                nonce: probe.nonce,
                port: ProbePort::Second,
                seen: from,
            }));
        }
        match packet {
            MasterPacket::Register(register) => {
                if !self.key.check(from, register.nonce, register.cookie, now) {
                    return Some(MasterPacket::Challenge(Challenge {
                        nonce: register.nonce,
                        cookie: self.key.cookie(from, register.nonce, now),
                    }));
                }
                let existing = self
                    .listings
                    .iter()
                    .find(|(_, e)| e.from == from)
                    .map(|(token, e)| (*token, e.nonce, e.listing_id));
                let (token, listing_id) = match existing {
                    Some((token, nonce, id)) if nonce == register.nonce => (token, id),
                    other => {
                        if let Some((token, ..)) = other {
                            self.listings.remove(&token);
                        }
                        let id = self.next_id;
                        self.next_id += 1;
                        (id * 1_000_003, id)
                    }
                };
                self.listings.insert(
                    token,
                    Entry {
                        listing_id,
                        from,
                        nonce: register.nonce,
                        summary: register.summary,
                        heard: now,
                    },
                );
                Some(MasterPacket::Listed(Listed {
                    nonce: register.nonce,
                    listing_id,
                    token,
                    seen: from,
                    heartbeat_secs: 30,
                    keep_secs: 15,
                    expiry_secs: 90,
                }))
            }
            MasterPacket::Heartbeat(heartbeat) => match self.listings.get_mut(&heartbeat.token) {
                Some(entry) => {
                    entry.summary = heartbeat.summary;
                    entry.heard = now;
                    entry.from = from;
                    Some(MasterPacket::HeartbeatAck(HeartbeatAck {
                        listing_id: entry.listing_id,
                        seen: from,
                    }))
                }
                None => Some(MasterPacket::UnknownListing(UnknownListing {
                    token: heartbeat.token,
                })),
            },
            MasterPacket::Keep(keep) => match self.listings.get_mut(&keep.token) {
                Some(entry) => {
                    entry.heard = now;
                    None
                }
                None => Some(MasterPacket::UnknownListing(UnknownListing {
                    token: keep.token,
                })),
            },
            MasterPacket::Unregister(unregister) => {
                self.listings.remove(&unregister.token);
                None
            }
            MasterPacket::Probe(probe) => Some(MasterPacket::ProbeAnswer(ProbeAnswer {
                nonce: probe.nonce,
                port: ProbePort::Main,
                seen: from,
            })),
            _ => None,
        }
    }

    /// What a Browse would list: each listing's summary.
    fn browse(&self) -> Vec<&ListingSummary> {
        self.listings.values().map(|e| &e.summary).collect()
    }

    fn count(&self, kind: MasterKind) -> usize {
        self.log.iter().filter(|(_, _, p)| p.kind() == kind).count()
    }

    fn times(&self, kinds: &[MasterKind]) -> Vec<Duration> {
        self.log
            .iter()
            .filter(|(_, _, p)| kinds.contains(&p.kind()))
            .map(|(at, ..)| *at)
            .collect()
    }
}

use crate::master::MasterKind;

fn summary(players: u8) -> ListingSummary {
    ListingSummary {
        protocol_version: 7,
        phase: DiscoverPhase::Lobby,
        players,
        capacity: 15,
        session_id: 99,
        game_version: "0.2.0".into(),
        game_commit: "abc".into(),
        compatibility: 0,
        name: "Friday night".into(),
        mission: "UKR, 2 v 2".into(),
        king: "Viper".into(),
        callsigns: (0..players).map(|n| format!("Pilot{n}")).collect(),
        ..ListingSummary::default()
    }
}

fn config(install_id: Option<u64>) -> HostRendezvous {
    HostRendezvous {
        build: Build {
            protocol_version: 7,
            game_version: "0.2.0".into(),
            game_commit: "abc".into(),
            compatibility: 0,
            release: false,
        },
        dedicated: true,
        install_id,
        platform: 3,
        entropy: Entropy::Seeded(11),
    }
}

/// A host on the simulator: its socket, its rendezvous, the players' count
/// its summary shows, and what reached its "transport".
struct Rig {
    net: SimNetwork,
    master: ScriptedMaster,
    socket: SimSocket,
    rendezvous: Rendezvous,
    players: u8,
    /// Datagrams the transport would have read.
    transport: Vec<(SocketAddr, Vec<u8>)>,
    events: Vec<(Duration, RendezvousEvent)>,
    /// While false the host is gone: it neither reads nor sends.
    alive: bool,
}

impl Rig {
    fn new(host: &str) -> Self {
        let net = SimNetwork::new(3);
        net.set_default_link(LinkConfig::one_way(Duration::from_millis(20)));
        Self::on(net, host)
    }

    fn on(net: SimNetwork, host: &str) -> Self {
        let master = ScriptedMaster::new(&net, 5);
        let socket = net.bind(address(host)).unwrap();
        let rendezvous = Rendezvous::host(config(None), net.now());
        Self {
            net,
            master,
            socket,
            rendezvous,
            players: 1,
            transport: Vec::new(),
            events: Vec::new(),
            alive: true,
        }
    }

    fn list(&mut self, own: &str) {
        let now = self.net.now();
        let local = Candidate::new(CandidateKind::Local, address(own));
        self.rendezvous
            .set_masters(vec![address(MASTER)], vec![local], now);
        self.rendezvous.set_listed(true, now);
    }

    fn step(&mut self, dt: Duration) {
        self.net.advance(dt);
        let now = self.net.now();
        self.master.turn(now);
        if !self.alive {
            return;
        }
        let mut buf = [0u8; 2048];
        let mut routed = self.rendezvous.over(&mut self.socket, now);
        while let Some((length, from)) = routed.recv_datagram(&mut buf).unwrap() {
            self.transport.push((from, buf[..length].to_vec()));
        }
        if self.rendezvous.wants_summary(now) {
            self.rendezvous.set_summary(now, summary(self.players));
        }
        self.rendezvous.update(now);
        self.rendezvous.transmit(&mut self.socket).unwrap();
        while let Some(event) = self.rendezvous.poll_event() {
            self.events.push((now, event));
        }
    }

    /// Runs `seconds` in steps of 10 ms.
    fn run(&mut self, seconds: f64) {
        let steps = (seconds * 100.0).round() as u64;
        for _ in 0..steps {
            self.step(Duration::from_millis(10));
        }
    }

    /// Runs until `done`, at most `seconds`; the time it took.
    fn run_until(&mut self, seconds: f64, mut done: impl FnMut(&Self) -> bool) -> Option<f64> {
        let start = self.net.now();
        for _ in 0..(seconds * 100.0).round() as u64 {
            self.step(Duration::from_millis(10));
            if done(self) {
                return Some((self.net.now() - start).as_secs_f64());
            }
        }
        None
    }
}

#[test]
fn a_host_is_browsable_within_its_first_exchange() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    assert_eq!(rig.rendezvous.state(), ListingState::Registering);
    // Register, Challenge, Register with the cookie, Listed: two round trips
    // of 40 ms, and the master lists it on the second Register.
    let took = rig
        .run_until(1.0, |rig| !rig.master.browse().is_empty())
        .expect("listed");
    assert!(took <= 0.1, "{took}");
    assert_eq!(rig.master.browse()[0].name, "Friday night");
    rig.run(0.1);
    assert_eq!(
        rig.rendezvous.state(),
        ListingState::Listed {
            listing_id: 1,
            seen: address(HOST)
        }
    );
    assert!(rig.events.iter().any(
        |(_, e)| matches!(e, RendezvousEvent::Listed { listing_id: 1, seen } if *seen == address(HOST))
    ));
    assert_eq!(rig.master.count(MasterKind::Register), 2);
    // The register carried the build and no telemetry.
    let register = rig
        .master
        .log
        .iter()
        .find_map(|(_, _, p)| match p {
            MasterPacket::Register(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    assert!(register.dedicated && !register.telemetry && register.install_id == 0);
    assert_eq!(register.platform, 3);
    assert_eq!(register.candidates.len(), 1);
    // The master's datagrams never reached the transport.
    assert!(rig.transport.is_empty());
    assert_eq!(rig.rendezvous.counters.malformed, 0);
}

#[test]
fn the_install_id_goes_in_the_register_when_telemetry_is_on() {
    let mut rig = Rig::new(HOST);
    rig.rendezvous = Rendezvous::host(config(Some(0xfeed)), rig.net.now());
    rig.list(HOST);
    rig.run(0.5);
    let register = rig
        .master
        .log
        .iter()
        .find_map(|(_, _, p)| match p {
            MasterPacket::Register(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    assert!(register.telemetry);
    assert_eq!(register.install_id, 0xfeed);
    // An id of 0 is no id.
    assert_eq!(
        Rendezvous::host(config(Some(0)), Duration::ZERO)
            .config
            .install_id,
        None
    );
}

#[test]
fn heartbeats_every_30_seconds_keeps_between_and_never_two_within_2_seconds() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(181.0);
    let heartbeats = rig.master.times(&[MasterKind::Heartbeat]);
    let keeps = rig.master.times(&[MasterKind::Keep]);
    // Listed at about 0.04 s: heartbeats at 30, 60, ... 180.
    assert_eq!(heartbeats.len(), 6, "{heartbeats:?}");
    for pair in heartbeats.windows(2) {
        let gap = (pair[1] - pair[0]).as_secs_f64();
        assert!((29.9..30.1).contains(&gap), "{gap}");
    }
    // One Keep halfway between each pair.
    assert_eq!(keeps.len(), 6, "{keeps:?}");
    let mut all = rig.master.times(&[MasterKind::Heartbeat, MasterKind::Keep]);
    all.sort();
    for pair in all.windows(2) {
        assert!(pair[1] - pair[0] >= LISTING_GAP, "{all:?}");
    }
    // Each heartbeat was answered: the rendezvous never fell silent.
    assert!(
        !rig.events
            .iter()
            .any(|(_, e)| *e == RendezvousEvent::MasterSilent)
    );
}

#[test]
fn a_summary_change_reaches_the_master_within_5_seconds() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(12.0);
    for players in [2u8, 3, 4] {
        rig.players = players;
        let took = rig
            .run_until(10.0, |rig| rig.master.browse()[0].players == players)
            .expect("the change arrives");
        assert!(took <= 5.0, "{players}: {took}");
    }
    // A burst of changes costs one heartbeat each 5 seconds at most.
    let heartbeats = rig.master.times(&[MasterKind::Heartbeat]);
    for pair in heartbeats.windows(2) {
        assert!(
            pair[1] - pair[0] >= CHANGE_HEARTBEAT_DELAY,
            "{heartbeats:?}"
        );
    }
    // The change counter rose with each change.
    let last = rig
        .master
        .log
        .iter()
        .rev()
        .find_map(|(_, _, p)| match p {
            MasterPacket::Heartbeat(h) => Some(h.change),
            _ => None,
        })
        .unwrap();
    assert_eq!(last, 3);
}

#[test]
fn a_vanished_host_is_gone_from_the_list_within_90_seconds() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(20.0);
    assert_eq!(rig.master.browse().len(), 1);
    rig.alive = false;
    let took = rig
        .run_until(120.0, |rig| rig.master.browse().is_empty())
        .expect("expired");
    assert!(took <= 90.0, "{took}");
}

#[test]
fn a_master_restart_is_healed_within_one_heartbeat() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(5.0);
    rig.master.restart(6);
    assert!(rig.master.browse().is_empty());
    // The next Keep (at 15 s) already gets Unknown listing.
    let took = rig
        .run_until(40.0, |rig| !rig.master.browse().is_empty())
        .expect("listed again");
    assert!(took <= HEARTBEAT_INTERVAL.as_secs_f64(), "{took}");
    rig.run(0.1);
    let listed = rig
        .events
        .iter()
        .filter(|(_, e)| matches!(e, RendezvousEvent::Listed { .. }))
        .count();
    assert_eq!(listed, 2);
    assert_eq!(rig.master.count(MasterKind::UnknownListing), 0);
}

#[test]
fn a_silent_master_is_asked_with_back_off_never_more_than_once_a_second() {
    let mut rig = Rig::new(HOST);
    rig.master.up = false;
    rig.list(HOST);
    // The scripted master logs nothing while down; read the sends instead.
    rig.net.start_trace();
    rig.run(300.0);
    let sends: Vec<f64> = rig
        .net
        .take_trace()
        .iter()
        .filter(|t| t.from == address(HOST) && t.to == address(MASTER))
        .filter(|t| crate::master::packet::peek_kind(&t.datagram) == Some(MasterKind::Register))
        .map(|t| t.at.as_secs_f64())
        .collect();
    let gaps: Vec<f64> = sends.windows(2).map(|p| p[1] - p[0]).collect();
    assert!(gaps.iter().all(|gap| *gap >= 1.0), "{gaps:?}");
    // Every 3 s for the first 10 s, then 2, 4, 8 and 15 s (0.2.2; 16, 32
    // and 60 s before).
    let rounded: Vec<u64> = gaps.iter().map(|g| g.round() as u64).collect();
    assert_eq!(&rounded[..7], &[3, 3, 3, 3, 2, 4, 8], "{gaps:?}");
    assert!(rounded[7..].iter().all(|g| *g == 15), "{gaps:?}");
    let silent: Vec<_> = rig
        .events
        .iter()
        .filter(|(_, e)| *e == RendezvousEvent::MasterSilent)
        .collect();
    assert_eq!(silent.len(), 1);
    assert_eq!(rig.rendezvous.state(), ListingState::Silent);
    // When it comes back the next try lists the game.
    rig.master.up = true;
    rig.run_until(16.0, |rig| !rig.master.browse().is_empty())
        .expect("listed once the master answers");
    rig.run(0.1);
    assert!(matches!(
        rig.rendezvous.state(),
        ListingState::Listed { .. }
    ));
}

#[test]
fn unlisting_and_stopping_send_three_unregisters_and_the_listing_goes_at_once() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(1.0);
    assert_eq!(rig.master.browse().len(), 1);
    let now = rig.net.now();
    rig.rendezvous.set_listed(false, now);
    rig.rendezvous.transmit(&mut rig.socket).unwrap();
    rig.run(0.1);
    assert!(rig.master.browse().is_empty());
    assert_eq!(rig.master.count(MasterKind::Unregister), 3);
    assert_eq!(rig.rendezvous.state(), ListingState::Off);
    assert!(
        rig.events
            .iter()
            .any(|(_, e)| *e == RendezvousEvent::Unlisted)
    );
    // Nothing more goes out while unlisted.
    let before = rig.master.log.len();
    rig.run(60.0);
    assert_eq!(rig.master.log.len(), before);
    // Listing again registers again; stopping unregisters.
    rig.rendezvous.set_listed(true, rig.net.now());
    rig.run(1.0);
    assert_eq!(rig.master.browse().len(), 1);
    let now = rig.net.now();
    rig.rendezvous.stop(now);
    rig.rendezvous.transmit(&mut rig.socket).unwrap();
    rig.run(0.1);
    assert!(rig.master.browse().is_empty());
    assert_eq!(rig.master.count(MasterKind::Unregister), 6);
}

#[test]
fn game_datagrams_pass_unchanged_and_no_master_datagram_reaches_the_transport() {
    let mut rig = Rig::new(HOST);
    let mut player = rig.net.bind(address("192.0.2.7:40000")).unwrap();
    let mut spoofer = rig.net.bind(address("[100::1:0:5]:0")).unwrap();
    let mut link_like = rig.net.bind(LINK_ADDRESS).unwrap();
    rig.list(HOST);
    for n in 0..50u8 {
        player.send_datagram(address(HOST), &[n, 1, 2, 3]).unwrap();
        spoofer.send_datagram(address(HOST), &[n]).unwrap();
        rig.step(Duration::from_millis(10));
    }
    link_like.send_datagram(address(HOST), b"link").unwrap();
    rig.run(40.0);
    // Every game datagram, in order, unchanged; the link's address passes
    // (the hosting game's `Linked` socket has already judged it).
    let from_player: Vec<_> = rig
        .transport
        .iter()
        .filter(|(from, _)| *from == address("192.0.2.7:40000"))
        .map(|(_, bytes)| bytes.clone())
        .collect();
    assert_eq!(from_player.len(), 50);
    assert!(
        from_player
            .iter()
            .enumerate()
            .all(|(n, b)| b == &[n as u8, 1, 2, 3])
    );
    assert!(
        rig.transport
            .iter()
            .any(|(from, b)| *from == LINK_ADDRESS && b == b"link")
    );
    // Not one datagram from the master's addresses, though it answered many.
    assert!(rig.rendezvous.counters.received >= 4);
    assert!(
        rig.transport
            .iter()
            .all(|(from, _)| !rig.rendezvous.is_master(*from))
    );
    // A claim of the relayed prefix is dropped and counted.
    assert_eq!(rig.rendezvous.counters.relayed_claims, 50);
    assert!(rig.transport.iter().all(|(from, _)| !is_relayed(*from)));
    // A send to a relayed address goes nowhere before the relay exists.
    let now = rig.net.now();
    rig.rendezvous
        .over(&mut rig.socket, now)
        .send_datagram(address("[100::1:0:5]:0"), b"x")
        .unwrap();
    assert_eq!(rig.rendezvous.counters.relay_sends_dropped, 1);
    // The master's IPv4-mapped form is recognised too.
    assert!(
        rig.rendezvous
            .is_master(address("[::ffff:198.51.100.1]:26901"))
    );
    assert!(rig.rendezvous.is_master(address("198.51.100.1:26902")));
    assert!(!rig.rendezvous.is_master(address("198.51.100.1:26903")));
}

#[test]
fn the_mapping_test_tells_no_translation_from_a_home_router() {
    // No translation: the master sees the host's own address.
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(1.0);
    assert_eq!(rig.rendezvous.mapping(), MappingType::NoTranslation);
    assert!(
        rig.events
            .iter()
            .any(|(_, e)| *e == RendezvousEvent::MappingTested(MappingType::NoTranslation))
    );
    // Behind a home router: the same outside port to both master ports.
    let net = SimNetwork::new(4);
    net.add_router(RouterConfig::nat(
        "203.0.113.9".parse().unwrap(),
        "192.168.1.0/24".parse::<Prefix>().unwrap(),
    ))
    .unwrap();
    let mut rig = Rig::on(net, "192.168.1.20:26900");
    rig.list("192.168.1.20:26900");
    rig.run(1.0);
    assert_eq!(rig.rendezvous.mapping(), MappingType::SamePort);
    assert!(matches!(
        rig.rendezvous.state(),
        ListingState::Listed { seen, .. } if seen == address("203.0.113.9:26900")
    ));
    // Ten minutes on, the test runs again, once.
    let probes = rig.master.count(MasterKind::Probe);
    rig.run(601.0);
    assert_eq!(rig.master.count(MasterKind::Probe), probes + 2);
}

#[test]
fn an_unsupported_answer_stops_the_listing_with_its_text() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    let now = rig.net.now();
    let unsupported = MasterPacket::Unsupported(Unsupported {
        lowest: 2,
        highest: 3,
        text: "Update the game to see internet games.".into(),
    })
    .encode_in(MASTER_VERSION)
    .unwrap();
    rig.rendezvous.receive(now, address(MASTER), &unsupported);
    assert_eq!(
        rig.rendezvous.state(),
        ListingState::Refused("Update the game to see internet games.".into())
    );
    rig.run(30.0);
    assert!(rig.master.browse().is_empty());
}

#[test]
fn stale_and_foreign_packets_are_counted_and_change_nothing() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(1.0);
    let state = rig.rendezvous.state();
    let now = rig.net.now();
    for packet in [
        MasterPacket::Listed(Listed {
            nonce: 1,
            listing_id: 9,
            token: 9,
            seen: address(HOST),
            heartbeat_secs: 30,
            keep_secs: 15,
            expiry_secs: 90,
        }),
        MasterPacket::UnknownListing(UnknownListing { token: 12345 }),
        MasterPacket::HeartbeatAck(HeartbeatAck {
            listing_id: 77,
            seen: address("192.0.2.1:1"),
        }),
        MasterPacket::Challenge(Challenge {
            nonce: 3,
            cookie: 4,
        }),
    ] {
        rig.rendezvous
            .receive(now, address(MASTER), &packet.encode().unwrap());
    }
    rig.rendezvous
        .receive(now, address(MASTER), b"not a master packet");
    assert_eq!(rig.rendezvous.state(), state);
    assert_eq!(rig.rendezvous.counters.unexpected, 4);
    assert_eq!(rig.rendezvous.counters.malformed, 1);
}

#[test]
fn a_report_goes_only_with_an_install_id() {
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.run(1.0);
    let mut tally = HostTally::new(Duration::ZERO);
    assert!(!tally.anyone());
    let lan = address("192.168.1.30:40000");
    let far = address("198.51.100.77:40000");
    tally.present([LINK_ADDRESS, lan]);
    tally.present([LINK_ADDRESS, lan, far]);
    tally.present([LINK_ADDRESS, far]);
    tally.present([LINK_ADDRESS, far, lan]);
    assert!(tally.anyone());
    let report = tally.report(
        Duration::from_secs(600),
        Role::HostingGame,
        "0.2.0",
        3,
        MappingType::SamePort,
    );
    assert_eq!((report.minutes, report.humans), (10, 3));
    assert_eq!(report.players_by_path, [1, 1, 0, 0, 0, 0]);
    rig.rendezvous.report(report.clone());
    rig.run(0.1);
    assert_eq!(rig.master.count(MasterKind::Report), 0);
    let mut rig = Rig::new(HOST);
    rig.rendezvous = Rendezvous::host(config(Some(42)), rig.net.now());
    rig.list(HOST);
    rig.run(1.0);
    rig.rendezvous.report(report);
    rig.rendezvous.transmit(&mut rig.socket).unwrap();
    rig.run(0.1);
    let sent = rig
        .master
        .log
        .iter()
        .find_map(|(_, _, p)| match p {
            MasterPacket::Report(r) => Some(r.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(sent.install_id, 42);
    assert_eq!(sent.role, Role::HostingGame);
}

#[test]
fn paths_by_address() {
    for (text, path) in [
        ("192.168.1.2:1", Path::LocalNetwork),
        ("10.1.2.3:1", Path::LocalNetwork),
        ("127.0.0.1:1", Path::LocalNetwork),
        ("[fe80::1]:1", Path::LocalNetwork),
        ("[fd12::1]:1", Path::LocalNetwork),
        ("[::ffff:192.168.1.2]:1", Path::LocalNetwork),
        ("100.64.0.1:1", Path::ByAddress),
        ("198.51.100.7:1", Path::ByAddress),
        ("[2001:db8::1]:1", Path::ByAddress),
        ("[100::1:0:1]:0", Path::Relay),
    ] {
        assert_eq!(path_of(address(text)), path, "{text}");
    }
}

#[test]
fn a_master_with_two_addresses_is_tried_in_turn_when_silent() {
    let mut rig = Rig::new(HOST);
    let other = address("[2001:db8::1]:26901");
    let now = rig.net.now();
    rig.master.up = false;
    rig.rendezvous
        .set_masters(vec![address(MASTER), other], Vec::new(), now);
    rig.rendezvous.set_listed(true, now);
    // Silent at 10 s; the next request, 2 s on, goes to the next address.
    rig.run(10.5);
    assert_eq!(rig.rendezvous.master(), Some(address(MASTER)));
    rig.run(2.0);
    assert_eq!(rig.rendezvous.master(), Some(other));
    assert!(rig.rendezvous.is_master(address("[2001:db8::1]:26902")));
}

#[test]
fn state_lines_say_what_a_player_reads() {
    assert_eq!(
        state_text(&ListingState::Off),
        "Not listed on the Internet Lobby."
    );
    assert_eq!(
        state_text(&ListingState::Silent),
        "The Internet Lobby does not answer, so the game is not listed. Players can still join by address."
    );
    assert_eq!(
        state_text(&ListingState::Listed {
            listing_id: 1,
            seen: address(HOST)
        }),
        "Listed on the Internet Lobby, seen at 203.0.113.5:26900."
    );
}

#[test]
fn a_host_listing_looks_up_its_master_and_finds_its_own_address() {
    let mut listing = HostListing::new("127.0.0.1:26911", config(None), 26900, Duration::ZERO)
        .expect("a literal address");
    assert_eq!(listing.master_text(), "127.0.0.1:26911");
    assert!(HostListing::new("bad:0", config(None), 1, Duration::ZERO).is_err());
    // Not asked to list: no lookup, nothing to send.
    listing.update(Duration::ZERO, || summary(1));
    assert_eq!(listing.state(), ListingState::Off);
    listing.set_listed(true, Duration::ZERO);
    let mut now = Duration::ZERO;
    let started = std::time::Instant::now();
    while listing.state() == ListingState::FindingMaster {
        now += Duration::from_millis(1);
        listing.update(now, || summary(1));
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(listing.state(), ListingState::Registering);
    assert_eq!(
        listing.rendezvous().master(),
        Some(address("127.0.0.1:26911"))
    );
    assert_eq!(
        listing.rendezvous().candidates,
        vec![Candidate::new(
            CandidateKind::Local,
            address("127.0.0.1:26900")
        )]
    );
    // It asked for the master's attention: a Register and two Probes.
    let mut sent = Vec::new();
    while let Some(transmit) = listing.rendezvous_mut().poll_transmit() {
        sent.push((
            transmit.to,
            crate::master::packet::peek_kind(&transmit.datagram),
        ));
    }
    assert!(sent.contains(&(address("127.0.0.1:26911"), Some(MasterKind::Register))));
    assert!(sent.contains(&(address("127.0.0.1:26912"), Some(MasterKind::Probe))));
}

/// Slice J2: a listed host answers a Meet with five punches to each of the
/// player's addresses, 200 ms apart, from its game port, and a Meet ack with
/// its token; an unlisted host does nothing.
#[test]
fn a_meet_is_punched_and_acknowledged_only_while_listed() {
    use crate::master::packet::Meet;
    use crate::packet::Packet;
    let mut rig = Rig::new(HOST);
    let player_seen = address("192.0.2.9:40000");
    let mut player = rig.net.bind(player_seen).unwrap();
    let meet = |id| {
        MasterPacket::Meet(Meet {
            introduction_id: id,
            mapping: MappingType::SamePort,
            candidates: vec![
                Candidate::new(CandidateKind::Seen, player_seen),
                Candidate::new(CandidateKind::Local, address("10.0.0.9:40000")),
            ],
        })
        .encode()
        .unwrap()
    };
    // Not listed: no token for the ack, nothing punched.
    let now = rig.net.now();
    rig.rendezvous
        .set_masters(vec![address(MASTER)], Vec::new(), now);
    rig.net.inject(address(MASTER), address(HOST), &meet(1));
    rig.run(1.0);
    assert_eq!(rig.rendezvous.counters.meets_dropped, 1);
    rig.list(HOST);
    assert!(
        rig.run_until(5.0, |r| matches!(
            r.rendezvous.state(),
            ListingState::Listed { .. }
        ))
        .is_some()
    );
    let mut buf = [0u8; 2048];
    while player.recv_datagram(&mut buf).unwrap().is_some() {}
    let start = rig.net.now();
    rig.net.inject(address(MASTER), address(HOST), &meet(77));
    let mut punches = Vec::new();
    for _ in 0..200 {
        rig.step(Duration::from_millis(10));
        while let Some((length, from)) = player.recv_datagram(&mut buf).unwrap() {
            assert_eq!(from, address(HOST));
            let Ok(Packet::Punch(punch)) = Packet::decode(&buf[..length], 7) else {
                panic!("not a punch of the game's version")
            };
            assert_eq!((punch.introduction, length), (77, 13));
            punches.push(rig.net.now() - start);
        }
    }
    // Five, 200 ms apart (sent on 10 ms steps, 20 ms on the way).
    assert_eq!(punches.len(), 5, "{punches:?}");
    for pair in punches.windows(2) {
        assert_eq!(pair[1] - pair[0], Duration::from_millis(200));
    }
    let acks: Vec<_> = rig
        .master
        .log
        .iter()
        .filter_map(|(_, from, p)| match p {
            MasterPacket::MeetAck(ack) => Some((*from, *ack)),
            _ => None,
        })
        .collect();
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].0, address(HOST));
    assert_eq!(acks[0].1.introduction_id, 77);
    assert_eq!(rig.rendezvous.counters.meets, 1);
    assert_eq!(rig.rendezvous.counters.punches, 10);
    // The same Meet again is acknowledged, never punched again.
    rig.net.inject(address(MASTER), address(HOST), &meet(77));
    rig.run(1.0);
    assert_eq!(rig.rendezvous.counters.meets_again, 1);
    assert_eq!(rig.rendezvous.counters.punches, 10);
    assert_eq!(rig.master.count(MasterKind::MeetAck), 2);
}

/// The candidates the host sent in its latest Register or Heartbeat.
fn sent_candidates(master: &ScriptedMaster) -> Vec<Candidate> {
    master
        .log
        .iter()
        .rev()
        .find_map(|(_, _, p)| match p {
            MasterPacket::Register(r) => Some(r.candidates.clone()),
            MasterPacket::Heartbeat(h) => Some(h.candidates.clone()),
            _ => None,
        })
        .expect("the host sent a listing")
}

/// Slice J4b: the address a router's port mapping gave goes to the master
/// as a Mapped candidate, in the Register and every Heartbeat; a change
/// reaches the master within 5 seconds and raises the change counter, the
/// same address again does not, and taking it away removes it.
#[test]
fn a_mapped_address_is_a_candidate_and_a_change_reaches_the_master() {
    let mapped = address("203.0.113.5:26900");
    let local = Candidate::new(CandidateKind::Local, address(HOST));
    let mapped_candidate = Candidate::new(CandidateKind::Mapped, mapped);
    let mut rig = Rig::new(HOST);
    // Known before listing starts: the Register carries it.
    rig.rendezvous.set_mapped(Some(mapped), rig.net.now());
    rig.list(HOST);
    rig.run(1.0);
    assert_eq!(sent_candidates(&rig.master), vec![local, mapped_candidate]);
    // The same address again changes nothing.
    let heartbeats = rig.master.count(MasterKind::Heartbeat);
    rig.rendezvous.set_mapped(Some(mapped), rig.net.now());
    rig.run(4.0);
    assert_eq!(rig.master.count(MasterKind::Heartbeat), heartbeats);
    // A new address (a renewal that moved the port) reaches the master.
    let moved = address("203.0.113.5:26902");
    rig.rendezvous.set_mapped(Some(moved), rig.net.now());
    let took = rig
        .run_until(10.0, |rig| {
            sent_candidates(&rig.master).contains(&Candidate::new(CandidateKind::Mapped, moved))
        })
        .expect("the new candidate arrives");
    assert!(took <= 5.0, "{took}");
    assert_eq!(sent_candidates(&rig.master).len(), 2);
    let change = |rig: &Rig| {
        rig.master.log.iter().rev().find_map(|(_, _, p)| match p {
            MasterPacket::Heartbeat(h) => Some(h.change),
            _ => None,
        })
    };
    assert_eq!(change(&rig), Some(1));
    // Mapping removed: the candidate goes.
    rig.rendezvous.set_mapped(None, rig.net.now());
    rig.run_until(10.0, |rig| sent_candidates(&rig.master) == vec![local])
        .expect("the removal arrives");
    assert_eq!(change(&rig), Some(2));
}

/// The own candidates are rebuilt after every master lookup; the mapped one
/// stays, and the mapping test never takes it for the machine's own address.
#[test]
fn a_mapped_address_survives_a_new_set_of_own_candidates_and_the_mapping_test() {
    let mapped = address("203.0.113.5:26900");
    let mut rig = Rig::new(HOST);
    rig.list(HOST);
    rig.rendezvous.set_mapped(Some(mapped), rig.net.now());
    let local = Candidate::new(CandidateKind::Local, address(HOST));
    rig.rendezvous
        .set_masters(vec![address(MASTER)], vec![local], rig.net.now());
    assert_eq!(
        rig.rendezvous.candidates(),
        [local, Candidate::new(CandidateKind::Mapped, mapped)]
    );
    // No own candidate at all: the mapped one is not "the host's own
    // address", so the test cannot call a mapped host untranslated.
    let net = SimNetwork::new(4);
    net.add_router(RouterConfig::nat(
        "203.0.113.9".parse().unwrap(),
        "192.168.1.0/24".parse::<Prefix>().unwrap(),
    ))
    .unwrap();
    let mut rig = Rig::on(net, "192.168.1.20:26900");
    // The very address the master will see.
    let mapped = address("203.0.113.9:26900");
    rig.rendezvous.set_mapped(Some(mapped), rig.net.now());
    rig.rendezvous
        .set_masters(vec![address(MASTER)], Vec::new(), rig.net.now());
    rig.rendezvous.set_listed(true, rig.net.now());
    rig.run(1.0);
    assert_eq!(rig.rendezvous.mapping(), MappingType::SamePort);
}

/// A host listing keeps the mapped address across its master lookup.
#[test]
fn a_host_listing_adds_the_mapped_address_after_its_lookup() {
    let mapped = address("203.0.113.5:26900");
    let mut listing = HostListing::new("127.0.0.1:26911", config(None), 26900, Duration::ZERO)
        .expect("a literal address");
    listing.set_mapped(Some(mapped), Duration::ZERO);
    listing.set_listed(true, Duration::ZERO);
    let mut now = Duration::ZERO;
    let started = std::time::Instant::now();
    while listing.state() == ListingState::FindingMaster {
        now += Duration::from_millis(1);
        listing.update(now, || summary(1));
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        listing.rendezvous().candidates(),
        [
            Candidate::new(CandidateKind::Local, address("127.0.0.1:26900")),
            Candidate::new(CandidateKind::Mapped, mapped)
        ]
    );
}
