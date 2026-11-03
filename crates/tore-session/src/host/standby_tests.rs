//! Slice K3's tests (docs/ARCHITECTURE.md, "Host migration and rejoin", row
//! K3): on the network simulator a host and its bots fly the crowd fight,
//! two of the bots' games standing by in process with slice K2's
//! [`Standby`], one warm and one cold. The warm one matches every Check, the
//! cold one's checkpoints arrive within their pace, both hold the host's
//! world, a forced mismatch resyncs, a standby appointed in flight is ready
//! within its checkpoint's pace, a leaving standby is replaced, one behind
//! is dismissed, and the stream's bytes a second are recorded. Synthetic
//! resources, but for the ignored real-data 15 against 15 run.

use super::super::*;
use super::{MAX_RATE, MIN_RATE, PACE_SECONDS, StandbyFigures, StreamBytes};
use crate::client::candidate::CandidateSettings;
use crate::client::{Client, ClientConfig, ClientPhase, Controls};
use crate::journal::check_hash;
use crate::settings::{Mode, number};
use crate::standby::{Budget, MissionKey, Note, Standby};
use crate::wire::migration::{CheckResult, StandbyState};
use std::sync::Mutex;
use tore_formats::aircraft::AircraftId;
use tore_net::Entropy;
use tore_net::master::candidate::{Candidate, CandidateKind};
use tore_net::peers::{Peers, Route};
use tore_net::sim::{LinkConfig, SimNetwork, SimSocket};
use tore_world::mission::{Skill, Start};
use tore_world::snapshot::RenderSnapshot;
use tore_world::test_support::resources::{THEATER, resources};

const MS: Duration = Duration::from_millis(1);
/// A standby's status goes to the host this often (at most twice a second).
const STATUS_EVERY: Duration = Duration::from_millis(500);

fn host_address() -> SocketAddr {
    "10.0.0.1:26900".parse().unwrap()
}

/// The house: a game a player hosts, its own player not connected here
/// (the bots stand in for the other players' games).
fn house_address() -> SocketAddr {
    "10.0.0.9:26900".parse().unwrap()
}

fn build() -> BuildId {
    BuildId {
        compatibility: 0,
        version: "0.2.0-1-gtest".into(),
        commit: "test-commit".into(),
        release: false,
    }
}

/// The crowd fight's shape: four of ours against four bandits 2 nm ahead
/// at 10,000 feet.
fn crowd_spec() -> MissionSpec {
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.wings[0].count = 4;
    spec.wings[3].count = 4;
    spec.wings[3].skill = Skill::Average;
    spec.separation_nm = 2;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    spec
}

/// A game a player hosts, started by the console's `start now`.
fn config() -> HostConfig {
    HostConfig {
        entropy: Entropy::Seeded(31),
        open_planes: OpenPlanes::All,
        house: Some(house_address()),
        start: StartMode::King,
        crown: CrownRule::FirstPlayer,
        ..HostConfig::new(build())
    }
}

/// A platform other than this build's: a game of another class, so a cold
/// standby.
fn other_platform() -> Platform {
    Platform::ALL
        .into_iter()
        .find(|p| *p != Platform::current() && *p != Platform::Unknown)
        .unwrap()
}

/// One player's game: its client, its pilot chasing the other side, and
/// its standby run in process as the game's worker thread would run it.
struct Player {
    socket: SimSocket,
    /// The game's peers router in front of its joined socket (slice K6),
    /// which answers the host's reach tests.
    peers: Peers,
    client: Client,
    pilot: crate::bot::ScriptedPilot,
    picture: Option<RenderSnapshot>,
    last_frame: Option<Duration>,
    standby: Standby,
    /// Ticks the standby may step a millisecond: a starved game steps none.
    budget: usize,
    status_at: Option<Duration>,
    notes: Vec<(Duration, Note)>,
    statuses: Vec<(Duration, crate::wire::migration::StandbyStatus)>,
    /// A laser bomber's run (protocol 23, slice NA): it cruises and flies
    /// the run's commands and trigger.
    a2g: Option<crate::wire::readout_a2g_tests::A2gPilot>,
}

impl Player {
    fn controls(&mut self, now: Duration) -> Controls {
        let Some(prediction) = self.client.prediction() else {
            return Controls::default();
        };
        let enemies = crate::bot::enemies(&self.client);
        let mut controls = self.pilot.controls(
            now,
            &prediction.plane().flight,
            self.picture.as_ref(),
            &|id| enemies.contains(&id),
        );
        if let Some(run) = &mut self.a2g {
            run.drive(now, &mut controls);
        }
        controls
    }

    /// Every lobby id this player's lobby marks as a standby, with its mark.
    fn marks(&self) -> Vec<(String, messages::StandbyMark)> {
        self.client.lobby().map_or_else(Vec::new, |lobby| {
            lobby
                .players
                .iter()
                .filter(|p| p.standby != messages::StandbyMark::None)
                .map(|p| (p.callsign.clone(), p.standby))
                .collect()
        })
    }

    fn checkpoints(&self) -> Vec<(Duration, u64)> {
        self.notes
            .iter()
            .filter_map(|(at, note)| match note {
                Note::Checkpoint { tick, .. } => Some((*at, *tick)),
                _ => None,
            })
            .collect()
    }
}

/// A checkpoint the host began for a standby.
#[derive(Clone, Copy, Debug)]
struct Begun {
    player: u8,
    at: Duration,
    tick: u64,
    length: usize,
}

/// A host and its players' games on one simulated network.
struct Rig {
    net: SimNetwork,
    host: Host,
    socket: SimSocket,
    players: Vec<Player>,
    resources: Arc<BTreeMap<String, Vec<u8>>>,
    next_port: u16,
    /// The flight's spec text the players hold, which a standby builds.
    flight: Arc<Mutex<Option<String>>>,
    /// The host's world's hash by tick, for the ticks asked for.
    hashes: BTreeMap<u64, u64>,
    hash_from: Option<u64>,
    /// Checkpoints the host began, and how many each standby had begun.
    begun: Vec<Begun>,
    counts: BTreeMap<u8, u64>,
    /// The stream's bytes by second: (second, total bytes of every standby).
    seconds: Vec<(u64, u64)>,
    /// Each second, the bytes a second the host's transport sent each
    /// player, by callsign.
    wire: Vec<BTreeMap<String, u64>>,
    /// When every player flew, and each standby's stream bytes then.
    flying_at: Option<Duration>,
    at_flight: BTreeMap<u8, StreamBytes>,
    /// The host is gone (killed): it neither reads, updates nor sends.
    host_down: bool,
}

impl Rig {
    fn with(spec: MissionSpec, resources: BTreeMap<String, Vec<u8>>, config: HostConfig) -> Self {
        let net = SimNetwork::new(29);
        net.set_default_link(LinkConfig::for_round_trip(40 * MS, 0., 0., 0.));
        let socket = net.bind(host_address()).unwrap();
        let resources = Arc::new(resources);
        let host = Host::new(spec, Arc::clone(&resources), config).unwrap();
        Self {
            net,
            host,
            socket,
            players: Vec::new(),
            resources,
            next_port: 42_000,
            flight: Arc::new(Mutex::new(None)),
            hashes: BTreeMap::new(),
            hash_from: None,
            begun: Vec::new(),
            counts: BTreeMap::new(),
            seconds: Vec::new(),
            wire: Vec::new(),
            flying_at: None,
            at_flight: BTreeMap::new(),
            host_down: false,
        }
    }

    fn new() -> Self {
        let mut rig = Self::with(crowd_spec(), resources(), config());
        rig.host.set_standbys_enabled(true);
        rig
    }

    /// The game's builder: the flight the player holds, checked against the
    /// Flight record's spec hash when it names one.
    fn builder(&self) -> crate::standby::Builder {
        let flight = Arc::clone(&self.flight);
        let resources = Arc::clone(&self.resources);
        Box::new(move |key: &MissionKey| {
            let text = flight
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| "no flight held".to_owned())?;
            if let Some(hash) = key.spec_hash
                && hash != tore_codec::fnv1a64(text.as_bytes())
            {
                return Err("another flight".into());
            }
            let spec = MissionSpec::from_text(&text).map_err(|e| e.to_string())?;
            World::new(&spec, &ResourceReads::new(&resources), Seating::Open)
                .map_err(|e| e.to_string())
        })
    }

    fn join(&mut self, callsign: &str, plane: u32, platform: Platform) {
        let builder = self.builder();
        self.join_with(callsign, plane, platform, builder);
    }

    fn join_with(
        &mut self,
        callsign: &str,
        plane: u32,
        platform: Platform,
        builder: crate::standby::Builder,
    ) {
        let address: SocketAddr = format!(
            "10.0.{}.{}:26900",
            1 + (self.next_port - 42_000) / 200,
            2 + (self.next_port - 42_000) % 200
        )
        .parse()
        .unwrap();
        let socket = self.net.bind(address).unwrap();
        let config = ClientConfig {
            entropy: Entropy::Seeded(u64::from(self.next_port)),
            plane: Some(plane),
            platform,
            ..ClientConfig::new(host_address(), callsign, build())
        };
        self.next_port += 1;
        let mut client =
            Client::connect(config, Arc::clone(&self.resources), self.net.now()).unwrap();
        // Its Candidate report: it may host, at its own address.
        client.set_candidate(CandidateSettings {
            candidates: vec![Candidate::new(CandidateKind::Local, address)],
            ..CandidateSettings::default()
        });
        let seed = u64::from(self.next_port);
        self.players.push(Player {
            socket,
            peers: Peers::new(crate::wire::PROTOCOL_VERSION, Entropy::Seeded(seed)),
            client,
            pilot: crate::bot::ScriptedPilot::new(),
            picture: None,
            last_frame: None,
            standby: Standby::new(builder),
            budget: 64,
            status_at: None,
            notes: Vec::new(),
            statuses: Vec::new(),
            a2g: None,
        });
        // One at a time, so the join order is the order joined.
        assert!(
            self.run_until(Duration::from_secs(2), |r| r
                .players
                .last()
                .is_some_and(|p| p.client.lobby().is_some())),
            "{callsign} joins"
        );
    }

    /// A laser bomber's game joins for `plane` (protocol 23, slice NA): it
    /// takes the slot, sends the synthetic laser bomber's Cheat loading and
    /// readies once the host takes it, then cruises and flies the run.
    fn join_bomber(&mut self, callsign: &str, plane: u32, platform: Platform) {
        self.join(callsign, plane, platform);
        let index = self.players.len() - 1;
        let player = &mut self.players[index];
        player.pilot.passive = true;
        player.a2g = Some(Default::default());
        // The automatic ready took the slot with the standard load: send the
        // bomber's and ready again.
        assert!(
            self.run_until(Duration::from_secs(2), |r| r.players[index]
                .client
                .lobby()
                .and_then(|l| l.me())
                .is_some_and(|me| me.slot.is_some())),
            "{callsign} holds a slot"
        );
        self.players[index]
            .client
            .send_loadout(Some(tore_world::test_support::resources::a2g_loadout()));
        assert!(
            self.run_until(Duration::from_secs(2), |r| r.players[index]
                .client
                .lobby()
                .and_then(|l| l.me())
                .is_some_and(|me| me.loadout)),
            "{callsign}'s loadout is taken"
        );
        self.players[index].client.set_ready(true);
    }

    /// One millisecond for everyone.
    fn step(&mut self) {
        self.net.advance(MS);
        let now = self.net.now();
        let before = self.host.world.tick();
        if !self.host_down {
            self.host.receive_from(now, &mut self.socket).unwrap();
            self.host.update(now);
            self.host.transmit(&mut self.socket).unwrap();
        }
        while let Some(log) = self.host.poll_log() {
            if matches!(log, HostLog::MissionEnded { .. } | HostLog::Fault { .. }) {
                eprintln!("{now:?}: {log:?}");
            }
        }
        let tick = self.host.world.tick();
        if matches!(self.host.life, Life::Flying) {
            let mut flight = self.flight.lock().unwrap();
            if flight.as_deref() != Some(self.host.spec_text.as_str()) {
                *flight = Some(self.host.spec_text.clone());
            }
            drop(flight);
            if tick != before && self.hash_from.is_some_and(|from| tick >= from) {
                self.hashes
                    .insert(tick, check_hash(&self.host.world).unwrap());
            }
        }
        self.note_checkpoints(now, tick);
        if now.subsec_millis() == 0 && self.flying_at.is_some() {
            let total = self
                .host
                .standby_figures()
                .iter()
                .map(|f| f.bytes.total())
                .sum();
            self.seconds.push((now.as_secs(), total));
            let wire = self
                .host
                .peers
                .iter()
                .filter_map(|(id, peer)| {
                    let stats = self.host.server.stats(*id)?;
                    Some((peer.callsign.clone(), stats.bytes_sent_per_second))
                })
                .collect();
            self.wire.push(wire);
        }
        for player in &mut self.players {
            let mut buf = [0u8; 2_048];
            while let Ok(Some((len, from))) = player.socket.recv_datagram(&mut buf) {
                if player.peers.route(now, from, &buf[..len]) == Route::Client {
                    player.client.receive(now, from, &buf[..len]);
                }
            }
            let controls = player.controls(now);
            player.client.update(now, &controls);
            if player
                .last_frame
                .is_none_or(|last| now - last >= Duration::from_millis(16))
            {
                player.last_frame = Some(now);
                if let Some(frame) = player.client.frame(now) {
                    player.picture = Some(frame.picture);
                }
            }
            // The game's standby: the records in order, stepped within its
            // budget, the spare built when idle, its status twice a second.
            for record in player.client.take_standby_records() {
                let _ = player.standby.receive(&record);
            }
            player.standby.step(Budget::Ticks(player.budget));
            if !player.standby.has_work() {
                player.standby.prepare();
            }
            for note in player.standby.take_notes() {
                player.notes.push((now, note));
            }
            if player.standby.appointed().is_some()
                && player
                    .status_at
                    .is_none_or(|at| now.saturating_sub(at) >= STATUS_EVERY)
            {
                player.status_at = Some(now);
                let status = player.standby.status();
                player.statuses.push((now, status));
                player
                    .client
                    .request(now, messages::Message::StandbyStatus(status));
            }
            player.client.drive_peers(now, &mut player.peers);
            player.peers.update(now);
            player.peers.transmit(&mut player.socket).unwrap();
            while let Some(t) = player.client.poll_transmit() {
                player.socket.send_datagram(t.to, &t.datagram).unwrap();
            }
            while player.client.poll_event().is_some() {}
        }
    }

    /// Notes each checkpoint the host began, with its tick and length.
    fn note_checkpoints(&mut self, now: Duration, tick: u64) {
        for figures in self.host.standby_figures() {
            let count = self.counts.entry(figures.player).or_default();
            if figures.bytes.checkpoint_count > *count {
                *count = figures.bytes.checkpoint_count;
                let length = self.host.world.checkpoint().unwrap().len();
                self.begun.push(Begun {
                    player: figures.player,
                    at: now,
                    tick,
                    length,
                });
            }
        }
    }

    fn run_until(&mut self, limit: Duration, mut done: impl FnMut(&Rig) -> bool) -> bool {
        let end = self.net.now() + limit;
        while self.net.now() < end {
            self.step();
            if done(self) {
                return true;
            }
        }
        false
    }

    fn run(&mut self, time: Duration) {
        let end = self.net.now() + time;
        while self.net.now() < end {
            self.step();
        }
    }

    fn flying(&self, player: usize) -> bool {
        self.players[player].client.phase() == ClientPhase::Flying
    }

    /// Every player joins, the console starts the mission, and every player
    /// flies.
    fn fly(&mut self) {
        self.settle();
        assert!(
            self.run_until(Duration::from_secs(5), |r| {
                r.host.peers.len() == r.players.len()
                    && r.host.peers.values().all(|p| p.lobby.ready)
            }),
            "every player ready"
        );
        self.host.start_now();
        assert!(
            self.run_until(Duration::from_secs(5), |r| (0..r.players.len())
                .all(|p| r.flying(p))),
            "every player flies"
        );
        self.flying_at = Some(self.net.now());
        self.at_flight = self
            .host
            .standby_figures()
            .into_iter()
            .map(|f| (f.player, f.bytes))
            .collect();
    }

    /// In a game a player hosts, waits for slice K6's reach tests to make
    /// three players (or every one, when fewer) eligible to stand by.
    fn settle(&mut self) {
        if self.host.config.house.is_none() {
            return;
        }
        // Each reach test tries the three best candidates.
        let wanted = self.players.len().min(3);
        assert!(
            self.run_until(Duration::from_secs(30), |r| r
                .host
                .ranked_candidates()
                .len()
                >= wanted),
            "the reach tests make {wanted} eligible"
        );
    }

    /// Flies on for `seconds`, keeping the host's hash of every tick in the
    /// last four.
    fn fly_for(&mut self, seconds: u64) {
        self.hash_from = Some(self.host.world.tick() + seconds.saturating_sub(4) * 120);
        self.run(Duration::from_secs(seconds));
    }

    /// The host's figures for the standby that is `player`'s game.
    fn figures(&self, player: usize) -> Option<StandbyFigures> {
        let id = self.players[player].client.lobby()?.you;
        self.host
            .standby_figures()
            .into_iter()
            .find(|f| f.player == id)
    }

    fn lobby_id(&self, player: usize) -> u8 {
        self.players[player].client.lobby().unwrap().you
    }

    /// The standby's world, replayed to the end of what it holds, is the
    /// host's at that tick.
    fn holds_the_hosts_world(&mut self, player: usize) {
        let world = self.players[player].standby.replica().unwrap();
        let tick = world.tick();
        let expected = self
            .hashes
            .get(&tick)
            .unwrap_or_else(|| panic!("no host hash at tick {tick}"));
        assert_eq!(
            check_hash(&world).unwrap(),
            *expected,
            "player {player}'s standby differs from the host at tick {tick}"
        );
    }

    /// The stream's bytes a second over the last `seconds`, every standby.
    fn bytes_per_second(&self, seconds: u64) -> f64 {
        let Some(&(end, total)) = self.seconds.last() else {
            return 0.;
        };
        let (start, before) = self
            .seconds
            .iter()
            .rev()
            .find(|(s, _)| *s + seconds <= end)
            .or(self.seconds.first())
            .copied()
            .unwrap_or_default();
        (total - before) as f64 / (end - start).max(1) as f64
    }
}

impl Rig {
    /// Prints, from the second second on, what the host's transport sent
    /// each standby's game a second over what it sent `plain`, who stands by
    /// for nobody: the stream on the wire, its framing and resends included.
    /// Every second with `K3_WIRE` set, the busiest one otherwise.
    fn print_wire(&self, standbys: &[&str], plain: &str) {
        for who in standbys {
            let over: Vec<u64> = self
                .wire
                .iter()
                .skip(2)
                .filter_map(|w| Some(w.get(*who)?.saturating_sub(*w.get(plain)?)))
                .collect();
            let mean = over.iter().sum::<u64>() as f64 / over.len().max(1) as f64;
            eprintln!(
                "{who} on the wire over {plain}: mean {mean:.0} B/s, busiest second {} B/s",
                over.iter().max().copied().unwrap_or(0)
            );
            if std::env::var_os("K3_WIRE").is_some() {
                eprintln!("  by second: {over:?}");
            }
        }
    }
}

/// The pace a checkpoint of `length` bytes goes at.
fn pace(length: usize) -> Duration {
    let rate = (length as f64 / PACE_SECONDS).clamp(MIN_RATE, MAX_RATE);
    Duration::from_secs_f64(length as f64 / rate)
}

/// A stream's bytes since `before`, field by field.
fn since(now: StreamBytes, before: StreamBytes) -> StreamBytes {
    StreamBytes {
        ticks: now.ticks - before.ticks,
        tick_count: now.tick_count - before.tick_count,
        seat_inputs: now.seat_inputs - before.seat_inputs,
        states: now.states - before.states,
        checks: now.checks - before.checks,
        checkpoints: now.checkpoints - before.checkpoints,
        checkpoint_count: now.checkpoint_count - before.checkpoint_count,
        other: now.other - before.other,
    }
}

impl Rig {
    /// Prints a standby's stream figures since every player flew, for the
    /// slice's measurements.
    fn report(&self, what: &str, figures: &StandbyFigures) {
        let flying_at = self.flying_at.expect("flying");
        let seconds = (self.net.now() - flying_at).as_secs_f64();
        let before = self
            .at_flight
            .get(&figures.player)
            .copied()
            .unwrap_or_default();
        let b = since(figures.bytes, before);
        let per_seat_tick = b.ticks as f64 / b.seat_inputs.max(1) as f64;
        eprintln!(
            "{what}: {} ({}), {:.0} s of flight: ticks {:.0} B/s ({} ticks, {:.1} B a seat a \
             tick), states {:.0} B/s, checks {:.0} B/s, checkpoints {:.0} B/s ({}), other {} \
             B; all {:.0} B/s; checks equal {}, different {}",
            figures.callsign,
            if figures.warm { "warm" } else { "cold" },
            seconds,
            b.ticks as f64 / seconds,
            b.tick_count,
            per_seat_tick,
            b.states as f64 / seconds,
            b.checks as f64 / seconds,
            b.checkpoints as f64 / seconds,
            b.checkpoint_count,
            b.other,
            b.total() as f64 / seconds,
            figures.checks_equal,
            figures.mismatches,
        );
    }
}

/// The crowd fight with a warm and a cold standby for `seconds`: the
/// row's acceptance.
fn warm_and_cold(seconds: u64) {
    let mut rig = Rig::new();
    rig.join("Viper", 0, Platform::current());
    rig.join("Cobra", 1, other_platform());
    rig.join("Hawk", 2, Platform::current());
    // Appointed in the lobby: the two longest-connected, warm by class.
    rig.settle();
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.host.standby_figures().len()
            == 2)
    );
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.players[2].marks().len() == 2),
        "the lobby marks the standbys"
    );
    let marks = rig.players[2].marks();
    assert!(marks.contains(&("Viper".into(), messages::StandbyMark::First)));
    assert!(marks.contains(&("Cobra".into(), messages::StandbyMark::Second)));
    assert_eq!(rig.figures(2), None, "Hawk stands by for nobody");
    assert!(rig.figures(0).unwrap().warm);
    assert!(!rig.figures(1).unwrap().warm);
    assert!(
        rig.run_until(Duration::from_secs(2), |r| r.host.ready_standbys().len()
            == 2),
        "ready in the lobby"
    );
    rig.fly();
    assert!(
        rig.run_until(Duration::from_secs(1), |r| r.players[..2].iter().all(|p| p
            .notes
            .iter()
            .any(|(_, n)| matches!(n, Note::Flight { .. })))),
        "the Flight record built each standby's world"
    );
    rig.fly_for(seconds);

    // The warm standby matched every Check.
    let warm = rig.figures(0).unwrap();
    let checks = rig.host.world.tick() / 600;
    assert!(
        warm.checks_equal + 1 >= checks,
        "{} checks equal of {checks}",
        warm.checks_equal
    );
    assert_eq!(warm.mismatches, 0);
    assert!(
        rig.players[0]
            .statuses
            .iter()
            .all(|(_, s)| s.check != CheckResult::Different)
    );
    assert_eq!(warm.status.unwrap().state, StandbyState::Warm);
    // The cold standby's checkpoints, every 1,200 ticks, each within its
    // pace.
    let cold = rig.figures(1).unwrap();
    assert_eq!(cold.status.unwrap().state, StandbyState::Cold);
    let cobra = rig.lobby_id(1);
    let begun: Vec<Begun> = rig
        .begun
        .iter()
        .filter(|b| b.player == cobra)
        .copied()
        .collect();
    assert!(
        begun.len() as u64 + 1 >= rig.host.world.tick() / 1_200,
        "{} cold checkpoints",
        begun.len()
    );
    let arrived = rig.players[1].checkpoints();
    for b in &begun {
        let Some((at, _)) = arrived.iter().find(|(_, tick)| *tick == b.tick) else {
            // The last may still be on its way.
            assert!(rig.net.now() < b.at + pace(b.length) + Duration::from_secs(1));
            continue;
        };
        let took = *at - b.at;
        eprintln!(
            "cold checkpoint at tick {}: {} bytes in {:.2} s (pace {:.2} s)",
            b.tick,
            b.length,
            took.as_secs_f64(),
            pace(b.length).as_secs_f64()
        );
        assert!(
            took <= pace(b.length) + Duration::from_millis(500),
            "the checkpoint at tick {} took {took:?}",
            b.tick
        );
    }
    // Both hold the host's world.
    rig.holds_the_hosts_world(0);
    rig.holds_the_hosts_world(1);
    // The stream's bytes, recorded.
    rig.report("crowd fight, 3 humans", &warm);
    rig.report("crowd fight, 3 humans", &cold);
    eprintln!(
        "both standbys over the last 10 s: {:.0} B/s",
        rig.bytes_per_second(10)
    );
    rig.print_wire(&["Viper", "Cobra"], "Hawk");
    assert!(warm.bytes.ticks > 0 && warm.bytes.states > 0 && warm.bytes.checks > 0);
}

#[test]
fn a_warm_and_a_cold_standby_follow_the_crowd_fight() {
    warm_and_cold(30);
}

/// Battery fix (2026-10-20, `net-migrate-kill`): a host killed (SIGKILL)
/// right after any update has already sent its standbys every tick it
/// stepped, and so every tick its players were shown. The stream used to
/// hold ticks back for a snapshot interval of them (two at 60 a second), so
/// a host killed after an update that left one waiting took that tick with
/// it and the new host's world started a tick behind the old host's last.
/// Killed once after an even tick and once after an odd one.
#[test]
fn a_host_killed_after_any_update_leaves_its_standbys_every_tick_it_stepped() {
    for parity in [0, 1] {
        let mut rig = Rig::new();
        rig.join("Viper", 0, Platform::current());
        rig.join("Cobra", 1, other_platform());
        rig.join("Hawk", 2, Platform::current());
        rig.settle();
        assert!(
            rig.run_until(Duration::from_secs(3), |r| r.host.ready_standbys().len()
                == 2),
            "two ready standbys"
        );
        rig.fly();
        rig.run(Duration::from_secs(2));
        let killed = loop {
            let before = rig.host.world.tick();
            rig.step();
            let tick = rig.host.world.tick();
            if tick != before && tick % 2 == parity {
                break tick;
            }
        };
        // Killed: what it sent arrives, nothing more comes.
        rig.host_down = true;
        rig.run(Duration::from_millis(300));
        for player in 0..2 {
            let world = rig.players[player].standby.replica().unwrap();
            assert_eq!(
                world.tick(),
                killed,
                "player {player}'s standby holds the world at tick {}, the host's last \
                 update ended at {killed}",
                world.tick()
            );
        }
    }
}

/// Slice S11: a host whose world makes no contrail puffs (as a dedicated
/// server's makes none) tells its standbys so in the Flight record and the
/// checkpoints: the warm copy and the cold one step without puffs too and
/// match every Check.
#[test]
fn standbys_of_a_host_without_contrail_puffs_match_its_world() {
    let mut rig = Rig::with(
        crowd_spec(),
        resources(),
        HostConfig {
            contrail_puffs: false,
            ..config()
        },
    );
    rig.host.set_standbys_enabled(true);
    rig.join("Viper", 0, Platform::current());
    rig.join("Cobra", 1, other_platform());
    rig.join("Hawk", 2, Platform::current());
    rig.settle();
    assert!(
        rig.run_until(Duration::from_secs(5), |r| r.host.ready_standbys().len()
            == 2),
        "ready in the lobby"
    );
    rig.fly();
    rig.fly_for(12);
    assert!(!rig.host.world.combat.contrail_puffs());
    let warm = rig.figures(0).unwrap();
    assert_eq!(warm.mismatches, 0);
    assert!(warm.checks_equal >= 1, "{} checks equal", warm.checks_equal);
    for player in [0, 1] {
        rig.holds_the_hosts_world(player);
        let replica = rig.players[player].standby.replica().unwrap();
        assert!(!replica.combat.contrail_puffs(), "player {player}'s copy");
    }
}

/// The acceptance at full length (5 minutes), for the full run: slow
/// (minutes in a debug build).
#[test]
#[ignore = "slow: the full suite runs it (slice K3's five-minute crowd fight)"]
fn a_warm_and_a_cold_standby_follow_a_five_minute_crowd_fight() {
    warm_and_cold(300);
}

/// Protocol 22, slice N1: a warm and a cold standby follow a ground target
/// whose column drives: the warm one's Checks stay equal and the cold one's
/// checkpoints arrive, and both hold the host's world, the surface's movers,
/// rails and timers included (they are in the combat checkpoint).
#[test]
fn a_warm_and_a_cold_standby_follow_a_moving_ground_target() {
    use tore_world::surface::{SURFACE_UNIT_BASE, UnitId};
    use tore_world::test_support::surface::{routed_resources, spec_with_target, target};
    let crowd = crowd_spec();
    let mut spec = spec_with_target(&target("QUCOL", 0, 0, 5));
    spec.wings = crowd.wings;
    spec.start = crowd.start;
    spec.separation_nm = crowd.separation_nm;
    let mut rig = Rig::with(spec, routed_resources(), config());
    rig.host.set_standbys_enabled(true);
    rig.join("Viper", 0, Platform::current());
    rig.join("Cobra", 1, other_platform());
    rig.join("Hawk", 2, Platform::current());
    rig.settle();
    assert!(
        rig.run_until(Duration::from_secs(5), |r| r.host.ready_standbys().len()
            == 2),
        "ready in the lobby"
    );
    rig.fly();
    rig.fly_for(12);
    let warm = rig.figures(0).unwrap();
    assert_eq!(warm.mismatches, 0);
    assert!(warm.checks_equal >= 1, "{warm:?}");
    rig.holds_the_hosts_world(0);
    rig.holds_the_hosts_world(1);
    let tank = |world: &World| {
        world
            .combat
            .surface
            .unit(UnitId(SURFACE_UNIT_BASE))
            .and_then(|u| u.mover)
    };
    let driving = tank(&rig.host.world).expect("the column drives");
    assert!(driving.speed_feet() > 0. || driving.leg > 0);
    for player in [0, 1] {
        let replica = rig.players[player].standby.replica().unwrap();
        if replica.tick() == rig.host.world.tick() {
            assert_eq!(replica.combat.surface, rig.host.world.combat.surface);
        }
        assert!(
            tank(&replica).is_some(),
            "standby {player} holds the column"
        );
    }
}

/// Protocol 23, slice NA: a warm and a cold standby follow three laser
/// bombers (the synthetic F/A-18D's pod and two GBU-10s): every sight
/// pinned, every laser set by hand, bombs released and guiding. The warm
/// one's Checks stay equal, both hold the host's world, and their copies of
/// the sights, lasers and guided flights are the host's (they are in the
/// combat checkpoint and stepped from the same commands).
#[test]
fn a_warm_and_a_cold_standby_follow_laser_bombers() {
    use crate::settings::{LoadoutRule, number};
    use tore_sim::combat::sight::SightMode;
    let mut spec = crate::wire::readout_a2g_tests::a2g_spec();
    // An open mission: each player's loadout is the lobby's.
    spec.loadout = None;
    spec.wings[0].count = 3;
    let mut config = config();
    config
        .settings
        .push((number::LOADOUTS, LoadoutRule::Any.value()));
    let mut resources = tore_world::test_support::surface::routed_resources();
    tore_world::test_support::resources::add_a2g_stores(&mut resources);
    let mut rig = Rig::with(spec, resources, config);
    rig.host.set_standbys_enabled(true);
    rig.join_bomber("Viper", 0, Platform::current());
    rig.join_bomber("Cobra", 1, other_platform());
    rig.join_bomber("Hawk", 2, Platform::current());
    rig.settle();
    assert!(
        rig.run_until(Duration::from_secs(5), |r| r.host.ready_standbys().len()
            == 2),
        "ready in the lobby"
    );
    rig.fly();
    rig.fly_for(12);
    let state = &rig.host.world.combat.state;
    for plane in 0..3 {
        let own = state.ownship(plane).unwrap();
        assert!(
            matches!(own.sight.map(|s| s.mode), Some(SightMode::Pinned(_))),
            "plane {plane}'s sight is pinned: {:?}",
            own.sight.map(|s| s.mode)
        );
        assert!(
            state.guided_impact(plane).is_some()
                || state.guided_flights().any(|(_, f)| f.owner == plane),
            "plane {plane} released a guided bomb"
        );
    }
    let warm = rig.figures(0).unwrap();
    assert_eq!(warm.mismatches, 0);
    assert!(warm.checks_equal >= 1, "{warm:?}");
    rig.holds_the_hosts_world(0);
    rig.holds_the_hosts_world(1);
    for player in [0, 1] {
        let replica = rig.players[player].standby.replica().unwrap();
        if replica.tick() != rig.host.world.tick() {
            continue;
        }
        let (ours, host) = (&replica.combat.state, &rig.host.world.combat.state);
        for plane in 0..3 {
            let (a, b) = (ours.ownship(plane).unwrap(), host.ownship(plane).unwrap());
            assert_eq!((a.sight, a.laser), (b.sight, b.laser), "plane {plane}");
        }
        assert!(ours.guided_flights().eq(host.guided_flights()));
    }
}

/// Slice FX-J: three laser bombers fly the run, and Hawk leaves its game
/// after its release, its laser on and its sight pinned. The host gives the
/// plane to the AI and keeps Hawk's cockpit, whose sight and laser step on
/// while the AI flies. A warm and a cold standby hold the same kept cockpit
/// (it is in the combat checkpoint, and the give-back is a journal
/// command), and their worlds are the host's.
#[test]
fn a_warm_and_a_cold_standby_carry_a_dropped_bombers_kept_cockpit() {
    use crate::settings::{LoadoutRule, number};
    use tore_sim::combat::sight::SightMode;
    let mut spec = crate::wire::readout_a2g_tests::a2g_spec();
    spec.loadout = None;
    spec.wings[0].count = 3;
    let mut config = config();
    config
        .settings
        .push((number::LOADOUTS, LoadoutRule::Any.value()));
    let mut resources = tore_world::test_support::surface::routed_resources();
    tore_world::test_support::resources::add_a2g_stores(&mut resources);
    let mut rig = Rig::with(spec, resources, config);
    rig.host.set_standbys_enabled(true);
    rig.join_bomber("Viper", 0, Platform::current());
    rig.join_bomber("Cobra", 1, other_platform());
    rig.join_bomber("Hawk", 2, Platform::current());
    rig.settle();
    assert!(
        rig.run_until(Duration::from_secs(5), |r| r.host.ready_standbys().len()
            == 2),
        "ready in the lobby"
    );
    rig.fly();
    rig.run(Duration::from_secs(6));
    let left = rig.host.world.combat.state.ownship(2).unwrap().clone();
    assert!(matches!(
        left.sight.map(|s| s.mode),
        Some(SightMode::Pinned(_))
    ));
    let now = rig.net.now();
    rig.players[2].client.leave_game(now);
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r
            .host
            .world
            .combat
            .state
            .kept(2)
            .is_some()),
        "the host keeps Hawk's cockpit"
    );
    rig.fly_for(6);
    let kept = rig.host.world.combat.state.kept(2).unwrap().clone();
    assert_eq!(kept.sight.map(|s| s.mode), left.sight.map(|s| s.mode));
    assert_eq!(kept.selected, left.selected);
    let warm = rig.figures(0).unwrap();
    assert_eq!(warm.mismatches, 0);
    rig.holds_the_hosts_world(0);
    rig.holds_the_hosts_world(1);
    for player in [0, 1] {
        let replica = rig.players[player].standby.replica().unwrap();
        if replica.tick() == rig.host.world.tick() {
            assert_eq!(
                replica.combat.state.kept(2),
                Some(&kept),
                "standby {player}"
            );
        } else {
            assert!(replica.combat.state.kept(2).is_some(), "standby {player}");
        }
    }
}

#[test]
fn a_forced_mismatch_resyncs_the_warm_standby() {
    let mut rig = Rig::new();
    rig.join("Viper", 0, Platform::current());
    rig.join("Hawk", 1, Platform::current());
    rig.fly();
    // The first check equal, then a spoiled one.
    assert!(rig.run_until(Duration::from_secs(8), |r| {
        r.figures(0).is_some_and(|f| f.checks_equal >= 1)
    }));
    rig.host.standbys.spoil_next_check = true;
    assert!(
        rig.run_until(Duration::from_secs(8), |r| r
            .figures(0)
            .is_some_and(|f| f.mismatches == 1)),
        "the standby reports the mismatch"
    );
    let mismatch_at = rig.net.now();
    // The host sends a checkpoint; the standby restores it and is warm.
    assert!(
        rig.run_until(Duration::from_secs(6), |r| r.players[0]
            .notes
            .iter()
            .any(|(at, n)| *at >= mismatch_at - Duration::from_secs(1)
                && matches!(n, Note::Checkpoint { restored: true, .. }))),
        "a checkpoint resyncs it"
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| {
        r.figures(0)
            .is_some_and(|f| f.status.unwrap().state == StandbyState::Warm)
    }));
    // The next check is equal again, and the standby is still warm.
    let equal = rig.figures(0).unwrap().checks_equal;
    assert!(rig.run_until(Duration::from_secs(8), |r| {
        r.figures(0).is_some_and(|f| f.checks_equal > equal)
    }));
    let figures = rig.figures(0).unwrap();
    assert!(figures.warm);
    assert_eq!(figures.mismatches, 1);
    rig.hash_from = Some(rig.host.world.tick() + 240);
    rig.run(Duration::from_secs(3));
    rig.holds_the_hosts_world(0);
}

/// The battery's planned handover (2026-10-24): a status sent while a resync
/// checkpoint was still on its way asked for it again, and the host sent a
/// second one 2 seconds after the first. The warm copy, restored from the
/// first and stepped on, refused the second as a checkpoint its journal does
/// not continue, asked again, and every checkpoint after was refused the same
/// way. One resync is one checkpoint, and a warm copy in good standing never
/// refuses one.
#[test]
fn a_resync_sends_one_checkpoint_and_nothing_is_refused() {
    let mut rig = Rig::new();
    rig.join("Viper", 0, Platform::current());
    rig.join("Hawk", 1, Platform::current());
    rig.fly();
    assert!(rig.run_until(Duration::from_secs(8), |r| {
        r.figures(0).is_some_and(|f| f.checks_equal >= 1)
    }));
    let viper = rig.lobby_id(0);
    let before = rig.figures(0).unwrap().bytes.checkpoint_count;
    rig.host.standbys.spoil_next_check = true;
    assert!(
        rig.run_until(Duration::from_secs(8), |r| r
            .figures(0)
            .is_some_and(|f| f.mismatches == 1)),
        "the standby reports the mismatch"
    );
    // Long enough for the resync, a redundant one after it, and that one's
    // refusal.
    rig.run(Duration::from_secs(25));
    let figures = rig.figures(0).unwrap();
    assert_eq!(
        figures.bytes.checkpoint_count - before,
        1,
        "one checkpoint for one mismatch: {:?}",
        rig.begun
            .iter()
            .filter(|b| b.player == viper)
            .collect::<Vec<_>>()
    );
    let refused: Vec<&Note> = rig.players[0]
        .notes
        .iter()
        .map(|(_, n)| n)
        .filter(|n| matches!(n, Note::Refused(_)))
        .collect();
    assert!(refused.is_empty(), "refused: {refused:?}");
    assert!(figures.warm);
    assert_eq!(figures.status.unwrap().state, StandbyState::Warm);
}

#[test]
fn a_standby_failing_its_checks_twice_goes_cold() {
    let mut rig = Rig::new();
    rig.join("Viper", 0, Platform::current());
    rig.fly();
    for n in 1..=2 {
        rig.host.standbys.spoil_next_check = true;
        assert!(rig.run_until(Duration::from_secs(8), |r| {
            r.figures(0).is_some_and(|f| f.mismatches == n || !f.warm)
        }));
    }
    assert!(!rig.figures(0).unwrap().warm, "appointed again, cold");
    assert!(
        rig.run_until(Duration::from_secs(10), |r| r.figures(0).is_some_and(|f| f
            .status
            .is_some_and(|s| s.state == StandbyState::Cold))),
        "ready, cold"
    );
}

#[test]
fn a_standby_appointed_in_flight_is_ready_within_its_checkpoints_pace() {
    let mut rig = Rig::new();
    rig.host.set_standbys_enabled(false);
    rig.join("Viper", 0, Platform::current());
    rig.join("Cobra", 1, other_platform());
    rig.fly();
    rig.run(Duration::from_secs(4));
    assert!(rig.host.standby_figures().is_empty(), "none until enabled");
    assert!(rig.players[0].client.take_standby_records().is_empty());
    rig.host.set_standbys_enabled(true);
    let appointed = rig.net.now();
    assert!(rig.run_until(
        Duration::from_millis(50),
        |r| r.host.standby_figures().len() == 2
    ));
    let lengths: Vec<usize> = rig.begun.iter().map(|b| b.length).collect();
    assert_eq!(lengths.len(), 2, "a checkpoint for each");
    let limit = pace(lengths[0]) + Duration::from_millis(500);
    assert!(
        rig.run_until(limit, |r| r.host.ready_standbys().len() == 2),
        "both ready within {limit:?}"
    );
    eprintln!(
        "appointed in flight: ready after {:.2} s, checkpoints of {lengths:?} bytes",
        (rig.net.now() - appointed).as_secs_f64()
    );
    // No Flight record: the checkpoint carried the world.
    assert!(rig.players.iter().all(|p| {
        !p.notes
            .iter()
            .any(|(_, n)| matches!(n, Note::Flight { .. }))
    }));
    rig.hash_from = Some(rig.host.world.tick() + 240);
    rig.run(Duration::from_secs(3));
    rig.holds_the_hosts_world(0);
    rig.holds_the_hosts_world(1);
    // Switched off, both are dismissed and the journal stops.
    rig.host.set_standbys_enabled(false);
    rig.run(Duration::from_millis(500));
    assert!(rig.host.standby_figures().is_empty());
    assert!(
        rig.players[..2]
            .iter()
            .all(|p| p.notes.iter().any(|(_, n)| *n == Note::Dismissed))
    );
    rig.run(Duration::from_millis(100));
    assert!(rig.host.drain_journal().0.is_empty(), "the journal stopped");
}

#[test]
fn a_leaving_standby_is_replaced_and_one_behind_is_dismissed() {
    let mut rig = Rig::new();
    rig.join("Viper", 0, Platform::current());
    // Cobra's game cannot build the mission: it reports itself behind.
    rig.join_with(
        "Cobra",
        1,
        Platform::current(),
        Box::new(|_: &MissionKey| Err("no such theater".into())),
    );
    rig.join("Hawk", 2, Platform::current());
    rig.fly();
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.players[1]
            .notes
            .iter()
            .any(|(_, n)| *n == Note::Dismissed)),
        "Cobra, behind, is dismissed"
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| r.figures(2).is_some()));
    let hawk = rig.figures(2).unwrap();
    assert_eq!(hawk.role, messages::StandbyMark::Second);
    // Not appointed again this flight, though eligible by order.
    rig.run(Duration::from_secs(2));
    assert_eq!(rig.figures(1), None);
    // Viper leaves: Hawk stays second and Cobra, still behind this flight,
    // is not appointed; the first role waits.
    rig.players[0].client.disconnect(rig.net.now());
    assert!(
        rig.run_until(Duration::from_secs(8), |r| r.figures(0).is_none()
            && r.host.standby_figures().len() == 1)
    );
    assert_eq!(
        rig.figures(2).unwrap().role,
        messages::StandbyMark::Second,
        "a remaining standby keeps its role"
    );
    // A new player fills the free role and becomes ready.
    rig.join("Mako", 3, Platform::current());
    assert!(rig.run_until(Duration::from_secs(10), |r| r.flying(3)));
    assert!(
        rig.run_until(Duration::from_secs(30), |r| r
            .figures(3)
            .is_some_and(|f| f.role == messages::StandbyMark::First && f.ready())),
        "Mako stands by first: ranked {:?}, figures {:?}, players {:?}",
        rig.host.ranked_candidates(),
        rig.host.standby_figures(),
        rig.host
            .peers
            .values()
            .map(|p| (p.callsign.clone(), p.lobby.order))
            .collect::<Vec<_>>()
    );
    let ready = rig.host.ready_standbys();
    assert_eq!(ready.len(), 2);
    // Every player heard the ready standbys, Mako first.
    let mako = rig
        .host
        .peers
        .iter()
        .find(|(_, p)| p.callsign == "Mako")
        .map(|(id, _)| *id)
        .unwrap();
    let last = rig.host.standbys.successions.last().unwrap();
    assert_eq!(last, &ready);
    assert_eq!(last[0].0, mako);
}

#[test]
fn a_dedicated_server_and_a_relayed_player_stand_by_for_nobody() {
    // A dedicated server: no house.
    let mut rig = Rig::with(
        crowd_spec(),
        resources(),
        HostConfig {
            entropy: Entropy::Seeded(31),
            open_planes: OpenPlanes::All,
            start: StartMode::FirstPlayer,
            ..HostConfig::new(build())
        },
    );
    rig.host.set_standbys_enabled(true);
    rig.join("Viper", 0, Platform::current());
    assert!(rig.run_until(Duration::from_secs(5), |r| r.flying(0)));
    rig.run(Duration::from_secs(1));
    assert!(rig.host.standby_figures().is_empty());
    assert!(rig.host.drain_journal().0.is_empty(), "no journal kept");

    // A relayed player in a game a player hosts.
    let mut rig = Rig::new();
    rig.join("Viper", 0, Platform::current());
    rig.join("Hawk", 1, Platform::current());
    assert!(rig.run_until(Duration::from_secs(3), |r| r.host.peers.len() == 2));
    rig.settle();
    rig.host.set_standbys_enabled(false);
    rig.step();
    for peer in rig.host.peers.values_mut() {
        if peer.callsign == "Viper" {
            peer.path = Path::Relay;
        }
    }
    rig.host.set_standbys_enabled(true);
    rig.run(Duration::from_millis(100));
    let figures = rig.host.standby_figures();
    assert_eq!(figures.len(), 1);
    assert_eq!(figures[0].callsign, "Hawk");
}

// ----- Measurements, for the full run ------------------------------------

/// Thirty humans, fifteen a side in PvP, on the synthetic import: the
/// stream's bytes a seat a tick and a second, warm and cold.
#[test]
#[ignore = "slow: the full suite runs it (slice K3's thirty-human stream measure)"]
fn the_stream_of_thirty_humans_is_measured() {
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    for wing in &mut spec.wings {
        wing.count = 5;
    }
    spec.separation_nm = 2;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    let config = HostConfig {
        max_players: 30,
        settings: vec![(number::MODE, Mode::Pvp.value()), (number::KILL_LIMIT, 0)],
        ..config()
    };
    let mut rig = Rig::with(spec, resources(), config);
    rig.host.set_standbys_enabled(true);
    for plane in 0..30 {
        let platform = if plane == 1 {
            other_platform()
        } else {
            Platform::current()
        };
        rig.join(&format!("Pilot{plane}"), plane, platform);
    }
    rig.fly();
    let seconds = 60;
    rig.fly_for(seconds);
    for player in 0..2 {
        let figures = rig.figures(player).unwrap();
        rig.report("30 humans", &figures);
        assert_eq!(figures.mismatches, 0);
    }
    eprintln!(
        "both standbys over the last 20 s: {:.0} B/s",
        rig.bytes_per_second(20)
    );
    rig.print_wire(&["Pilot0", "Pilot1"], "Pilot2");
    rig.holds_the_hosts_world(0);
    rig.holds_the_hosts_world(1);
}

/// The real-data 15 against 15 mission (as `tests/host_load.rs` flies it)
/// with four humans, a warm and a cold standby, for five minutes. Reads a
/// real import through `TORE_DATA_DIR`; run it in release:
///
/// ```sh
/// TORE_DATA_DIR=$PWD/.local/mpb-data-k3 cargo test --release -p tore-session \
///     --lib host::standby::tests::real -- --ignored --nocapture
/// ```
#[test]
#[ignore = "reads a real import through TORE_DATA_DIR; the full suite runs it in release"]
fn real_data_15_against_15_with_a_warm_and_a_cold_standby() {
    let directory = tore_import::data_directory().expect("TORE_DATA_DIR");
    let import = tore_import::load(&directory).expect("an imported pack");
    let mut spec = MissionSpec::new("UKR", AircraftId::F18);
    for (index, wing) in spec.wings.iter_mut().enumerate() {
        wing.count = 5;
        wing.skill = Skill::Average;
        if index >= 3 {
            wing.aircraft = AircraftId::Mig29;
        }
    }
    spec.separation_nm = 10;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    let mut rig = Rig::with(spec, import, config());
    rig.host.set_standbys_enabled(true);
    rig.join("Viper", 0, Platform::current());
    rig.join("Cobra", 1, other_platform());
    rig.join("Hawk", 2, Platform::current());
    rig.join("Mako", 5, Platform::current());
    rig.fly();
    let seconds: u64 = std::env::var("K3_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);
    rig.hash_from = Some(rig.host.world.tick() + (seconds - 4) * 120);
    let start = rig.net.now();
    let mut minute = 1;
    while rig.net.now() < start + Duration::from_secs(seconds) {
        rig.run(Duration::from_secs(10));
        if rig.net.now() >= start + Duration::from_secs(60 * minute) {
            eprintln!(
                "minute {minute}: both standbys {:.0} B/s over the last 60 s",
                rig.bytes_per_second(60)
            );
            minute += 1;
        }
    }
    let warm = rig.figures(0).unwrap();
    let cold = rig.figures(1).unwrap();
    rig.report("real 15 v 15, 4 humans", &warm);
    rig.report("real 15 v 15, 4 humans", &cold);
    let cobra = rig.lobby_id(1);
    let arrived = rig.players[1].checkpoints();
    for b in rig.begun.iter().filter(|b| b.player == cobra) {
        if let Some((at, _)) = arrived.iter().find(|(_, tick)| *tick == b.tick) {
            eprintln!(
                "cold checkpoint at tick {}: {} bytes in {:.2} s (pace {:.2} s)",
                b.tick,
                b.length,
                (*at - b.at).as_secs_f64(),
                pace(b.length).as_secs_f64()
            );
        }
    }
    rig.print_wire(&["Viper", "Cobra"], "Hawk");
    assert_eq!(warm.mismatches, 0);
    rig.holds_the_hosts_world(0);
    rig.holds_the_hosts_world(1);
}
