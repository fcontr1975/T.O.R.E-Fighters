//! Slice K4's tests (docs/ARCHITECTURE.md, "Host migration and rejoin", row
//! K4), on the network simulator: a game a player hosts, its own player on
//! the in-process link, and three bots' games, two of them standing by in
//! process with slice K2's [`Standby`] as the game runs it. Every game runs
//! what slice K7a's game will: its peers router in front of its socket, its
//! client, its standby, and once it takes over its host, with its own client
//! on the link. Synthetic resources.
//!
//! The plan's simulator acceptance: the host cut off from every link in a
//! dogfight keeps running privately; the new host's world at T codes to the
//! old host's bytes at T; every client gets snapshots again within 5
//! simulated seconds of the cut; missiles in flight at the cut fly on and end
//! on the new host; kills from before the cut are in the Results. Also: a
//! handover under a second; standby 2 takes over when standby 1 is cut too;
//! an old host that comes back steps down and resumes as a player; on one
//! class no client's own plane is corrected at the resume; a player who
//! never resumes is dropped at 5 seconds with its plane reserved.

use super::*;
use crate::client::candidate::CandidateSettings;
use crate::client::migrate::{MigrationCounts, MigrationState, words};
use crate::client::{Client, ClientConfig, ClientEvent, ClientPhase, Controls};
use crate::standby::takeover::take_over;
use crate::standby::{Budget, MissionKey, Standby};
use std::sync::Mutex;
use tore_formats::aircraft::AircraftId;
use tore_net::master::candidate::{Candidate, CandidateKind};
use tore_net::peers::{Peers, Route};
use tore_net::sim::{LinkConfig, SimNetwork, SimSocket};
use tore_net::{Entropy, LINK_ADDRESS};
use tore_world::mission::{Skill, Start};
use tore_world::snapshot::RenderSnapshot;
use tore_world::test_support::resources::{THEATER, resources};

const MS: Duration = Duration::from_millis(1);
/// A standby's status goes to the host this often (at most twice a second).
const STATUS_EVERY: Duration = Duration::from_millis(500);
/// How often an old host's game asks standby 1 whether it hosts now.
const ASK_EVERY: Duration = Duration::from_secs(1);

fn host_address() -> SocketAddr {
    "10.0.0.1:26900".parse().unwrap()
}

fn build() -> BuildId {
    BuildId {
        compatibility: 0,
        version: "0.2.0-1-gtest".into(),
        commit: "test-commit".into(),
        release: false,
    }
}

/// The crowd fight: four of ours against four bandits `separation` nm ahead
/// at 10,000 feet.
fn crowd_spec(separation: u32) -> MissionSpec {
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.wings[0].count = 4;
    spec.wings[3].count = 4;
    spec.wings[3].skill = Skill::Average;
    spec.separation_nm = separation;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    spec
}

/// A game's host configuration: a game a player hosts, its own player on
/// the link.
fn config(seed: u64) -> HostConfig {
    HostConfig {
        entropy: Entropy::Seeded(seed),
        open_planes: OpenPlanes::All,
        house: Some(LINK_ADDRESS),
        start: StartMode::King,
        crown: CrownRule::FirstPlayer,
        ..HostConfig::new(build())
    }
}

/// An old host's game asking standby 1 whether it hosts now.
struct Asking {
    session: u64,
    nonce: u64,
    /// Standby 1's addresses, remembered when the host was lost.
    to: Vec<SocketAddr>,
    last: Duration,
}

/// One player's game.
struct Game {
    callsign: String,
    socket: SimSocket,
    peers: Peers,
    client: Client,
    pilot: crate::bot::ScriptedPilot,
    picture: Option<RenderSnapshot>,
    last_frame: Option<Duration>,
    standby: Option<Standby>,
    status_at: Option<Duration>,
    /// The game hosts: the original host, or one that took over.
    host: Option<Host>,
    /// When it took over and T.
    took: Option<(Duration, u64)>,
    /// The original host stepped down.
    stepped_down: bool,
    events: Vec<(Duration, ClientEvent)>,
    /// When the client's snapshot count went up.
    snapshots: Vec<Duration>,
    seen: u64,
    /// The game is gone: nothing of it runs.
    gone: bool,
    asking: Option<Asking>,
    /// The pilot fires the missile station: its radar on and the missile
    /// selected once it flies, the trigger held 6,000 to 9,000 feet from
    /// the enemy it pursues (the fixtures' AI fires nothing guided).
    shooter: bool,
    armed: bool,
    /// A laser bomber's run (protocol 23, slice NA).
    a2g: Option<crate::wire::readout_a2g_tests::A2gPilot>,
    /// Until then the game's datagrams to the network wait, and go out in
    /// order after: a game that falls silent for a while, as a busy machine
    /// is.
    hold_until: Option<Duration>,
    held: Vec<(SocketAddr, Vec<u8>)>,
    /// Its standby's notes, with when.
    notes: Vec<(Duration, crate::standby::Note)>,
}

impl Game {
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
        if self.shooter {
            if !self.armed {
                self.armed = true;
                controls
                    .pilot
                    .commands
                    .push(tore_sim::flight::PilotCommand::Set(
                        tore_sim::flight::Switch::Radar,
                        true,
                    ));
                controls
                    .commands
                    .push(tore_world::seats::SeatCommand::CycleWeapon { forward: true });
            }
            controls.trigger = self
                .pilot
                .pursuing
                .is_some_and(|p| (6_000. ..9_000.).contains(&p.range));
        }
        if let Some(run) = &mut self.a2g {
            run.drive(now, &mut controls);
        }
        controls
    }

    fn address(&self) -> SocketAddr {
        self.socket.local_addr()
    }

    /// The first snapshot at or after `at`.
    fn snapshot_after(&self, at: Duration) -> Option<Duration> {
        self.snapshots.iter().copied().find(|t| *t >= at)
    }

    fn notices(&self) -> Vec<String> {
        self.events
            .iter()
            .filter_map(|(_, e)| match e {
                ClientEvent::Notice(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// The original host, before any takeover or step-down.
    fn original(&self) -> bool {
        self.host.is_some() && self.took.is_none()
    }
}

/// The games on one simulated network.
struct Rig {
    net: SimNetwork,
    games: Vec<Game>,
    resources: Arc<BTreeMap<String, Vec<u8>>>,
    /// The flight's spec text the players hold, which a standby builds.
    flight: Arc<Mutex<Option<String>>>,
    next: u16,
    /// The original host's world, coded at every tick from `record_from`.
    checkpoints: BTreeMap<u64, Vec<u8>>,
    record_from: Option<u64>,
    /// When the original host was cut off.
    cut_at: Option<Duration>,
    /// Each takeover: the game, when, T and the new host's world coded at T.
    takeovers: Vec<(usize, Duration, u64, Vec<u8>)>,
    /// The system the bots' games say they run on: another than this
    /// build's makes their standbys cold.
    platform: Platform,
    /// The original host's game is stalled: nothing of it runs.
    stalled: bool,
    /// How long a takeover's restore and replay take, as the new host sees
    /// it: its Resumption is that much older than its first update.
    replay_time: Duration,
}

impl Rig {
    /// The original host with its own player on the link, standbys on.
    fn new(spec: MissionSpec) -> Self {
        Self::with_resources(spec, Arc::new(resources()))
    }

    /// [`Rig::new`] over `resources` (a real import, say).
    fn with_resources(spec: MissionSpec, resources: Arc<BTreeMap<String, Vec<u8>>>) -> Self {
        Self::with_settings(spec, resources, &[])
    }

    /// [`Rig::with_resources`] with these host settings.
    fn with_settings(
        spec: MissionSpec,
        resources: Arc<BTreeMap<String, Vec<u8>>>,
        settings: &[(u8, u32)],
    ) -> Self {
        let net = SimNetwork::new(41);
        net.set_default_link(LinkConfig::for_round_trip(40 * MS, 0., 0., 0.));
        let socket = net.bind(host_address()).unwrap();
        let mut config = config(31);
        config.settings.extend_from_slice(settings);
        let mut host = Host::new(spec, Arc::clone(&resources), config).unwrap();
        host.set_standbys_enabled(true);
        let client = Client::connect(
            ClientConfig {
                entropy: Entropy::Seeded(7),
                plane: Some(0),
                ..ClientConfig::new(LINK_ADDRESS, "Lead", build())
            },
            Arc::clone(&resources),
            net.now(),
        )
        .unwrap();
        let mut rig = Self {
            net,
            games: Vec::new(),
            resources,
            flight: Arc::new(Mutex::new(None)),
            next: 0,
            checkpoints: BTreeMap::new(),
            record_from: None,
            cut_at: None,
            takeovers: Vec::new(),
            platform: Platform::current(),
            stalled: false,
            replay_time: Duration::ZERO,
        };
        rig.games.push(Game {
            callsign: "Lead".into(),
            socket,
            peers: Peers::new(crate::wire::PROTOCOL_VERSION, Entropy::Seeded(5)),
            client,
            pilot: crate::bot::ScriptedPilot::new(),
            picture: None,
            last_frame: None,
            standby: None,
            status_at: None,
            host: Some(host),
            took: None,
            stepped_down: false,
            events: Vec::new(),
            snapshots: Vec::new(),
            seen: 0,
            gone: false,
            asking: None,
            shooter: false,
            armed: false,
            a2g: None,
            hold_until: None,
            held: Vec::new(),
            notes: Vec::new(),
        });
        assert!(
            rig.run_until(Duration::from_secs(2), |r| r.games[0]
                .client
                .lobby()
                .is_some()),
            "the house joins its own host"
        );
        rig
    }

    /// The game's builder: the flight the player holds.
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

    /// A bot's game joins the original host for `plane`, its standby ready
    /// to be appointed; a `passive` pilot cruises and never fires.
    fn join(&mut self, callsign: &str, plane: u32, passive: bool) -> usize {
        let address: SocketAddr = format!("10.0.1.{}:26900", 2 + self.next).parse().unwrap();
        self.next += 1;
        let socket = self.net.bind(address).unwrap();
        let mut client = Client::connect(
            ClientConfig {
                entropy: Entropy::Seeded(100 + u64::from(self.next)),
                plane: Some(plane),
                platform: self.platform,
                ..ClientConfig::new(host_address(), callsign, build())
            },
            Arc::clone(&self.resources),
            self.net.now(),
        )
        .unwrap();
        client.set_candidate(CandidateSettings {
            candidates: vec![Candidate::new(CandidateKind::Local, address)],
            ..CandidateSettings::default()
        });
        let mut pilot = crate::bot::ScriptedPilot::new();
        pilot.passive = passive;
        let builder = self.builder();
        self.games.push(Game {
            callsign: callsign.into(),
            socket,
            peers: Peers::new(
                crate::wire::PROTOCOL_VERSION,
                Entropy::Seeded(200 + u64::from(self.next)),
            ),
            client,
            pilot,
            picture: None,
            last_frame: None,
            standby: Some(Standby::new(builder)),
            status_at: None,
            host: None,
            took: None,
            stepped_down: false,
            events: Vec::new(),
            snapshots: Vec::new(),
            seen: 0,
            gone: false,
            asking: None,
            shooter: false,
            armed: false,
            a2g: None,
            hold_until: None,
            held: Vec::new(),
            notes: Vec::new(),
        });
        let index = self.games.len() - 1;
        assert!(
            self.run_until(Duration::from_secs(2), |r| r.games[index]
                .client
                .lobby()
                .is_some()),
            "{callsign} joins"
        );
        index
    }

    /// A laser bomber's game joins for `plane` (protocol 23, slice NA): the
    /// synthetic laser bomber's Cheat loading in place of the standard load,
    /// then it cruises and flies the run.
    fn join_bomber(&mut self, callsign: &str, plane: u32) -> usize {
        let index = self.join(callsign, plane, true);
        self.games[index].a2g = Some(Default::default());
        assert!(
            self.run_until(Duration::from_secs(2), |r| r.games[index]
                .client
                .lobby()
                .and_then(|l| l.me())
                .is_some_and(|me| me.slot.is_some())),
            "{callsign} holds a slot"
        );
        self.games[index]
            .client
            .send_loadout(Some(tore_world::test_support::resources::a2g_loadout()));
        assert!(
            self.run_until(Duration::from_secs(2), |r| r.games[index]
                .client
                .lobby()
                .and_then(|l| l.me())
                .is_some_and(|me| me.loadout)),
            "{callsign}'s loadout is taken"
        );
        self.games[index].client.set_ready(true);
        index
    }

    fn host(&self) -> &Host {
        self.games[0].host.as_ref().unwrap()
    }

    fn host_mut(&mut self) -> &mut Host {
        self.games[0].host.as_mut().unwrap()
    }

    /// The host `game` runs.
    fn host_of(&self, game: usize) -> &Host {
        self.games[game].host.as_ref().unwrap()
    }

    /// One millisecond for every game.
    fn step(&mut self) {
        self.net.advance(MS);
        let now = self.net.now();
        for index in 0..self.games.len() {
            if self.games[index].gone || (index == 0 && self.stalled) {
                continue;
            }
            self.step_game(index, now);
            self.maybe_take_over(index, now);
        }
        // The original host's world while it runs, and the flight text.
        if let Some(host) = &self.games[0].host
            && self.games[0].took.is_none()
        {
            let tick = host.world.tick();
            if matches!(host.life, Life::Flying) {
                let mut flight = self.flight.lock().unwrap();
                if flight.as_deref() != Some(host.spec_text.as_str()) {
                    *flight = Some(host.spec_text.clone());
                }
                drop(flight);
                if self.record_from.is_some_and(|from| tick >= from)
                    && !self.checkpoints.contains_key(&tick)
                {
                    self.checkpoints
                        .insert(tick, host.world.checkpoint().unwrap());
                }
            }
        }
    }

    fn step_game(&mut self, index: usize, now: Duration) {
        let game = &mut self.games[index];
        let mut buf = [0u8; 2_048];
        while let Ok(Some((len, from))) = game.socket.recv_datagram(&mut buf) {
            let datagram = &buf[..len];
            if let Some(asking) = &game.asking
                && hosting_answer(datagram, asking.session, asking.nonce)
            {
                // Another game hosts now: this host steps down without a
                // word, and its own client resumes with the new host from
                // the socket the host had.
                if let Some(host) = &mut game.host {
                    host.step_down();
                }
                game.host = None;
                game.asking = None;
                game.stepped_down = true;
                game.client.move_to(now, &[from]);
                continue;
            }
            if game.original() {
                // The original host's socket is its host's.
                if let Some(host) = &mut game.host {
                    host.receive(now, from, datagram);
                }
                continue;
            }
            match game.peers.route(now, from, datagram) {
                Route::Host => {
                    if let Some(host) = &mut game.host {
                        host.receive(now, from, datagram);
                    }
                }
                Route::Client => game.client.receive(now, from, datagram),
                Route::Taken => {}
            }
        }
        if let Some(host) = &mut game.host {
            host.update(now);
            while let Some(t) = host.poll_transmit() {
                if t.to == LINK_ADDRESS {
                    game.client.receive(now, LINK_ADDRESS, &t.datagram);
                } else {
                    game.socket.send_datagram(t.to, &t.datagram).unwrap();
                }
            }
            while host.poll_log().is_some() {}
            // An old host that lost every player, or heard it was taken
            // over, asks standby 1 whether it hosts now, every second, at
            // the addresses it knew when it first asked.
            if game.took.is_none()
                && game.asking.is_none()
                && (host.lost_everyone() || host.moved_to().is_some())
            {
                game.asking = Some(Asking {
                    session: host.session(),
                    nonce: 0x5eed_0077,
                    to: host.standby_addresses(),
                    last: now.saturating_sub(ASK_EVERY),
                });
            }
            if let Some(asking) = &mut game.asking
                && now.saturating_sub(asking.last) >= ASK_EVERY
            {
                asking.last = now;
                let reach = reach_packet(asking.session, asking.nonce, 0);
                for to in &asking.to {
                    game.socket.send_datagram(*to, &reach).unwrap();
                }
            }
        }
        let controls = game.controls(now);
        game.client.update(now, &controls);
        if game
            .last_frame
            .is_none_or(|last| now - last >= Duration::from_millis(16))
        {
            game.last_frame = Some(now);
            if let Some(frame) = game.client.frame(now) {
                game.picture = Some(frame.picture);
            }
        }
        let seen = game.client.clone_stats().snapshots;
        if seen > game.seen {
            game.seen = seen;
            game.snapshots.push(now);
        }
        if let Some(standby) = &mut game.standby {
            for record in game.client.take_standby_records() {
                let _ = standby.receive(&record);
            }
            standby.step(Budget::Ticks(64));
            if !standby.has_work() {
                standby.prepare();
            }
            for note in standby.take_notes() {
                game.notes.push((now, note));
            }
            if standby.appointed().is_some()
                && game
                    .status_at
                    .is_none_or(|at| now.saturating_sub(at) >= STATUS_EVERY)
            {
                game.status_at = Some(now);
                let status = standby.status();
                game.client
                    .request(now, messages::Message::StandbyStatus(status));
            }
        }
        if game.took.is_some() {
            game.peers.set_hosting(true, game.client.old_host());
        }
        if !game.original() {
            game.client.drive_peers(now, &mut game.peers);
            game.peers.update(now);
            game.peers.transmit(&mut game.socket).unwrap();
        }
        let holding = game.hold_until.is_some_and(|until| now < until);
        if !holding {
            for (to, datagram) in std::mem::take(&mut game.held) {
                game.socket.send_datagram(to, &datagram).unwrap();
            }
        }
        while let Some(t) = game.client.poll_transmit() {
            if t.to == LINK_ADDRESS {
                if let Some(host) = &mut game.host {
                    host.receive(now, LINK_ADDRESS, &t.datagram);
                }
            } else if holding {
                game.held.push((t.to, t.datagram));
            } else {
                game.socket.send_datagram(t.to, &t.datagram).unwrap();
            }
        }
        while let Some(event) = game.client.poll_event() {
            game.events.push((now, event));
        }
    }

    /// A standby's game takes over when its client says so.
    fn maybe_take_over(&mut self, index: usize, now: Duration) {
        let game = &mut self.games[index];
        let Some(standby) = &game.standby else {
            return;
        };
        if game.host.is_some()
            || !game
                .client
                .takeover_due(standby.ready(), standby.handover().is_some())
        {
            return;
        }
        let standby = game.standby.take().unwrap();
        let house = game.client.lobby().unwrap().you;
        let present = game.client.present(now);
        let mut host = take_over(
            standby,
            Arc::clone(&self.resources),
            config(1_000 + index as u64),
            Resumption {
                house,
                now: now.saturating_sub(self.replay_time),
                present,
            },
        )
        .unwrap_or_else(|e| panic!("{} takes over: {e}", game.callsign));
        let tick = host.world.tick();
        let coded = host.world.checkpoint().unwrap();
        host.set_standbys_enabled(true);
        game.host = Some(host);
        game.took = Some((now, tick));
        game.client.host_here(now, LINK_ADDRESS, tick);
        game.peers.set_hosting(true, game.client.old_host());
        self.takeovers.push((index, now, tick, coded));
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

    fn flying(&self, game: usize) -> bool {
        self.games[game].client.phase() == ClientPhase::Flying
    }

    /// Two standbys ready, then every player flies.
    fn fly(&mut self) {
        assert!(
            self.run_until(Duration::from_secs(40), |r| r.host().ready_standbys().len()
                == 2),
            "two standbys ready"
        );
        assert!(
            self.run_until(Duration::from_secs(5), |r| {
                r.host().peers.len() == r.games.len()
                    && r.host().peers.values().all(|p| p.lobby.ready)
            }),
            "every player ready"
        );
        self.host_mut().start_now();
        assert!(
            self.run_until(Duration::from_secs(5), |r| (0..r.games.len())
                .all(|g| r.flying(g))),
            "every player flies"
        );
        assert!(
            self.run_until(Duration::from_secs(10), |r| r.games[1..].iter().all(|g| g
                .standby
                .as_ref()
                .is_none_or(|s| s.appointed().is_none() || s.ready()))
                && r.host().ready_standbys().len() == 2
                && r.games.iter().all(|g| g
                    .client
                    .succession()
                    .is_some_and(|s| s.standbys.len() == 2))),
            "the standbys ready in flight and the succession known"
        );
    }

    /// The games standing by, first then second.
    fn standbys(&self) -> (usize, usize) {
        let role = |mark: messages::StandbyMark| {
            (1..self.games.len())
                .find(|&g| self.games[g].client.standby_role() == mark)
                .unwrap()
        };
        (
            role(messages::StandbyMark::First),
            role(messages::StandbyMark::Second),
        )
    }

    /// Cuts `game`'s links to every other game.
    fn cut(&mut self, game: usize) {
        let lost = LinkConfig {
            loss: 1.,
            ..LinkConfig::for_round_trip(40 * MS, 0., 0., 0.)
        };
        let address = self.games[game].address();
        for other in 0..self.games.len() {
            if other != game {
                let to = self.games[other].address();
                self.net.set_link_both(address, to, lost);
            }
        }
        if game == 0 {
            self.cut_at = Some(self.net.now());
        }
    }

    /// Heals `game`'s links.
    fn heal(&mut self, game: usize) {
        let good = LinkConfig::for_round_trip(40 * MS, 0., 0., 0.);
        let address = self.games[game].address();
        for other in 0..self.games.len() {
            if other != game {
                let to = self.games[other].address();
                self.net.set_link_both(address, to, good);
            }
        }
    }

    /// The guided rounds in flight in `world`.
    fn missiles(world: &World) -> BTreeSet<u32> {
        world
            .combat
            .state
            .projectiles
            .iter()
            .filter(|p| p.guidance.is_some())
            .map(|p| p.id)
            .collect()
    }

    /// Each plane's aircraft kills.
    fn kills(world: &World) -> BTreeMap<u32, u32> {
        tore_world::debrief::results(world)
            .iter()
            .map(|r| (r.plane.0, r.aircraft_kills))
            .collect()
    }

    /// The flight's world restored from `bytes`.
    fn world_of(&self, bytes: &[u8]) -> World {
        let text = self.flight.lock().unwrap().clone().unwrap();
        let spec = MissionSpec::from_text(&text).unwrap();
        let mut world =
            World::new(&spec, &ResourceReads::new(&self.resources), Seating::Open).unwrap();
        world.restore(bytes).unwrap();
        world
    }

    /// Every remote player has snapshots again after the cut; each one's
    /// first, after the cut.
    fn back_after_cut(&mut self, within: Duration) -> Vec<(String, Duration)> {
        let cut_at = self.cut_at.unwrap();
        let from = cut_at + Duration::from_millis(200);
        let players: Vec<usize> = (1..self.games.len())
            .filter(|&g| !self.games[g].gone)
            .collect();
        let remaining = (cut_at + within).saturating_sub(self.net.now());
        let all = self.run_until(remaining, |r| {
            players
                .iter()
                .all(|&g| r.games[g].snapshot_after(from).is_some())
        });
        assert!(all, "every client has snapshots again within {within:?}");
        players
            .iter()
            .map(|&g| {
                let back = self.games[g].snapshot_after(from).unwrap();
                (self.games[g].callsign.clone(), back - cut_at)
            })
            .collect()
    }
}

/// The original host, its own player and three bots' games (Hawk with a
/// missile, the others `passive` or fighting) flying the crowd fight with
/// bandits `separation` nm ahead, two standbys ready: the rig and the games
/// standing by first and second.
fn flying(separation: u32, passive: bool) -> (Rig, usize, usize) {
    flying_on(separation, passive, Platform::current())
}

/// [`flying`] with the bots' games on `platform`.
fn flying_on(separation: u32, passive: bool, platform: Platform) -> (Rig, usize, usize) {
    let mut rig = Rig::new(crowd_spec(separation));
    rig.platform = platform;
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, passive);
    }
    rig.games[3].shooter = !passive;
    rig.fly();
    let (first, second) = rig.standbys();
    (rig, first, second)
}

/// The world as combat leaves it when `owner` shoots `victim` (an AI plane)
/// down.
fn shoot_down(host: &mut Host, owner: u32, victim: u32) {
    let state = &mut host.world.combat.state;
    state.ledger.kill(tore_sim::combat::ledger::Kill {
        owner,
        victim,
        category: 0x8000,
        aircraft: true,
    });
    state
        .targets
        .iter_mut()
        .find(|t| t.id == victim)
        .unwrap()
        .hp = 0;
}

/// The plan's simulator acceptance: the crowd fight with a kill booked, cut
/// off while a missile flies.
#[test]
fn a_host_cut_off_in_a_dogfight_is_taken_over_exactly() {
    let (mut rig, first, second) = flying(5, false);
    // A kill before the cut: the Lead shoots down the bandits' lead, booked
    // by hand as combat books it, and the standbys appointed again so their
    // copy holds it (a change by hand is not journaled).
    shoot_down(rig.host_mut(), 0, 4);
    rig.host_mut().set_standbys_enabled(false);
    rig.run(Duration::from_millis(20));
    rig.host_mut().set_standbys_enabled(true);
    assert!(
        rig.run_until(Duration::from_secs(15), |r| r.host().ready_standbys().len()
            == 2),
        "the standbys ready again"
    );
    // The fight until Hawk's missile flies.
    assert!(
        rig.run_until(Duration::from_secs(60), |r| !Rig::missiles(&r.host().world)
            .is_empty()),
        "a missile in flight within a minute"
    );
    // The host's world at every tick from here to the cut.
    rig.record_from = Some(rig.host().world.tick());
    rig.run(Duration::from_millis(200));
    let cut_tick = rig.host().world.tick();
    rig.cut(0);
    let cut_at = rig.cut_at.unwrap();

    // Standby 1 takes over after 1.5 s of silence.
    assert!(
        rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()),
        "standby 1 takes over"
    );
    rig.record_from = None;
    let (game, took_at, tick, coded) = rig.takeovers[0].clone();
    assert_eq!(game, first, "the first standby takes over");
    let after = took_at - cut_at;
    eprintln!(
        "took over {:.0} ms after the cut, at tick {tick} (the cut at {cut_tick})",
        after.as_secs_f64() * 1e3
    );
    assert!(after >= Duration::from_millis(1_400) && after < Duration::from_millis(2_000));
    // The new host's world at T codes to the old host's bytes at T.
    let old = rig
        .checkpoints
        .get(&tick)
        .unwrap_or_else(|| panic!("the old host's world at tick {tick}"))
        .clone();
    assert!(
        old == coded,
        "the new host's world at T differs from the old host's"
    );
    let at_t = rig.world_of(&old);
    let missiles = Rig::missiles(&at_t);
    let kills_before = Rig::kills(&at_t);
    assert!(kills_before.values().any(|k| *k > 0), "a kill before T");

    // Every client gets snapshots again within 5 seconds of the cut.
    for (callsign, back) in rig.back_after_cut(Duration::from_secs(5)) {
        eprintln!(
            "{callsign}: snapshots again {:.0} ms after the cut",
            back.as_secs_f64() * 1e3
        );
    }
    for g in 1..rig.games.len() {
        assert_eq!(rig.games[g].client.migration(), MigrationState::Steady);
        assert!(rig.flying(g), "{} flies on", rig.games[g].callsign);
        let counts = rig.games[g].client.migration_counts();
        eprintln!("{}: {counts:?}", rig.games[g].callsign);
        assert_eq!(counts.resumed, 1);
        // On one class the replay from T is the prediction: no client's own
        // plane is corrected at the resume.
        assert_eq!(counts.corrected, 0, "{} corrected", rig.games[g].callsign);
    }
    // The players read the design's words.
    let to = rig.games[first].callsign.clone();
    let notices = rig.games[second].notices();
    assert!(notices.contains(&words::lost(&to)), "{notices:?}");
    assert!(notices.contains(&words::moved(&to)), "{notices:?}");
    // The game that takes over names itself, not the other standby (slice
    // K10: its own address is not in its race).
    let own = rig.games[first].notices();
    assert!(own.contains(&words::lost(&to)), "{own:?}");

    // The old host keeps running privately with its own player.
    // (Every player is back in under 2 seconds: the 1.5 seconds of silence
    // alone are 180 ticks.)
    let private = rig.host().world.tick();
    assert!(private > cut_tick + 180, "the old host flies on: {private}");
    assert!(rig.flying(0), "its own player flies on");

    // The old host's own player never resumes: dropped 5 seconds after the
    // takeover, its plane kept for it.
    assert!(
        rig.run_until(Duration::from_secs(6), |r| r
            .host_of(game)
            .absent_players()
            .is_empty()),
        "the absent player dropped"
    );
    let dropped_at = rig.net.now() - took_at;
    assert!(
        dropped_at >= DROP_AFTER && dropped_at < DROP_AFTER + Duration::from_millis(50),
        "dropped {dropped_at:?} after the takeover"
    );
    assert!(
        rig.host_of(game)
            .rejoin
            .reserved_plane(PlaneId(0))
            .is_some(),
        "the old host's own plane is kept for it"
    );

    // The missiles in flight at T fly on and end on the new host.
    assert!(
        rig.run_until(Duration::from_secs(60), |r| {
            let now = Rig::missiles(r.host_of(game).world());
            missiles.iter().all(|id| !now.contains(id))
        }),
        "the missiles in flight at the cut end on the new host"
    );
    eprintln!(
        "{} missiles in flight at T ended on the new host",
        missiles.len()
    );

    // Kills from before the cut are in the Results.
    let watcher = second;
    rig.games[game].host.as_mut().unwrap().end();
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.games[watcher]
            .events
            .iter()
            .any(|(_, e)| matches!(e, ClientEvent::Results(_)))),
        "the Results arrive"
    );
    let results = rig.games[watcher]
        .events
        .iter()
        .find_map(|(_, e)| match e {
            ClientEvent::Results(results) => Some(results.clone()),
            _ => None,
        })
        .unwrap();
    for (plane, kills) in kills_before {
        let row = results.rows.iter().find(|r| r.plane == plane).unwrap();
        assert!(
            row.aircraft_kills >= kills,
            "plane {plane}: {} kills in the Results, {kills} before the cut",
            row.aircraft_kills
        );
    }
}

/// The house leaves on purpose: standby 1 gets Handover after the host's last
/// tick and takes over at once, every player is told and races it at once,
/// and every remote player has snapshots again within a second.
#[test]
fn a_host_that_leaves_hands_over_in_under_a_second() {
    let (mut rig, first, _) = flying(20, true);
    rig.run(Duration::from_secs(2));
    let at = rig.net.now();
    let to = rig.host_mut().hand_over().unwrap();
    assert_eq!(to, rig.games[first].client.lobby().unwrap().you);
    assert_eq!(
        rig.host_mut().hand_over().unwrap_err(),
        "The game has been handed over already."
    );
    // The old host's game stops it once its records are through.
    assert!(
        rig.run_until(Duration::from_secs(1), |r| r.host().handed_over()),
        "the handover's records through"
    );
    let last = rig.host().world.tick();
    rig.games[0].gone = true;
    assert!(
        rig.run_until(Duration::from_secs(1), |r| !r.takeovers.is_empty()),
        "standby 1 takes over at once"
    );
    let (game, took_at, tick, _) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    assert_eq!(
        tick, last,
        "the new host goes on from the old host's last tick"
    );
    eprintln!(
        "handed over: taken over {:.0} ms after",
        (took_at - at).as_secs_f64() * 1e3
    );
    let players: Vec<usize> = (1..rig.games.len()).collect();
    assert!(
        rig.run_until(Duration::from_secs(1), |r| players
            .iter()
            .all(|&g| r.games[g].snapshot_after(took_at).is_some())),
        "every player has snapshots again within a second"
    );
    for &g in &players {
        let back = rig.games[g].snapshot_after(took_at).unwrap() - at;
        eprintln!(
            "{}: snapshots again {:.0} ms after the handover",
            rig.games[g].callsign,
            back.as_secs_f64() * 1e3
        );
        assert!(back < Duration::from_secs(1));
        assert!(rig.flying(g));
        assert_eq!(rig.games[g].client.migration_counts().corrected, 0);
    }
    // The leaving house's plane is the AI's, kept for nobody: it left on
    // purpose.
    let new = rig.host_of(game);
    assert_eq!(
        new.world().roster.plane(PlaneId(0)).unwrap().pilot,
        tore_world::seats::Pilot::Ai
    );
    assert!(new.rejoin.reserved_plane(PlaneId(0)).is_none());
    assert!(
        new.absent_players().is_empty(),
        "{:?}",
        new.absent_players()
    );
}

/// Standby 1 cut off with the host: standby 2 waits 3 seconds more, racing
/// standby 1 meanwhile, then takes over, and the player left resumes with it.
#[test]
fn standby_two_takes_over_when_standby_one_is_cut_too() {
    let (mut rig, first, second) = flying(20, true);
    rig.run(Duration::from_secs(2));
    rig.cut(0);
    rig.cut(first);
    let cut_at = rig.cut_at.unwrap();
    assert!(
        rig.run_until(Duration::from_secs(6), |r| r
            .takeovers
            .iter()
            .any(|t| t.0 == second)),
        "standby 2 takes over"
    );
    let (_, took_at, _, _) = *rig.takeovers.iter().find(|t| t.0 == second).unwrap();
    let after = took_at - cut_at;
    eprintln!(
        "standby 2 took over {:.0} ms after the cut",
        after.as_secs_f64() * 1e3
    );
    assert!(after >= Duration::from_millis(4_400) && after < Duration::from_millis(5_000));
    // Standby 1, cut off, took over too, alone: a split is not healed.
    assert!(rig.takeovers.iter().any(|t| t.0 == first));
    // The other player resumes with standby 2.
    let other = (1..rig.games.len())
        .find(|&g| g != first && g != second)
        .unwrap();
    let from = took_at;
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.games[other]
            .snapshot_after(from)
            .is_some()),
        "the other player resumes with standby 2"
    );
    assert_eq!(
        rig.games[other].client.server(),
        rig.games[second].address(),
        "it joined standby 2"
    );
    let notices = rig.games[other].notices();
    let to = rig.games[second].callsign.clone();
    assert!(notices.contains(&words::moved(&to)), "{notices:?}");
    assert_eq!(rig.games[other].client.migration_counts().resumed, 1);
}

/// The old host cut off and taken over comes back: its game asks standby 1,
/// learns it hosts now, steps down without a word, and its own player
/// rejoins the new host as a player, back in its reserved aircraft.
#[test]
fn an_old_host_that_comes_back_steps_down_and_rejoins_as_a_player() {
    let (mut rig, first, _) = flying(20, true);
    rig.run(Duration::from_secs(2));
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()));
    assert!(
        rig.run_until(Duration::from_secs(1), |r| r.games[0].asking.is_some()),
        "the old host asks standby 1"
    );
    // Its own player is dropped by the new host, its plane kept for it.
    assert!(rig.run_until(Duration::from_secs(6), |r| {
        r.host_of(first).absent_players().is_empty()
    }));
    assert!(
        rig.host_of(first)
            .rejoin
            .reserved_plane(PlaneId(0))
            .is_some()
    );
    assert!(rig.host().world.tick() > 0 && !rig.games[0].stepped_down);
    rig.run(Duration::from_secs(1));
    rig.heal(0);
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.games[0].stepped_down),
        "the old host steps down"
    );
    let stepped_at = rig.net.now();
    let new_host = rig.games[first].address();
    assert!(
        rig.run_until(Duration::from_secs(6), |r| r.games[0].client.server()
            == new_host
            && r.flying(0)
            && r.games[0]
                .events
                .iter()
                .any(|(at, e)| *at >= stepped_at
                    && matches!(e, ClientEvent::Seated { plane: 0, .. }))),
        "its own player flies its reserved plane again, from the new host"
    );
    assert_eq!(rig.games[0].client.migration_counts().resumed, 1);
    let notices = rig.games[0].notices();
    assert!(
        notices
            .iter()
            .any(|n| n == "Welcome back, Lead: your aircraft is waiting."),
        "{notices:?}"
    );
    let to = rig.games[first].callsign.clone();
    assert!(notices.contains(&words::moved(&to)), "{notices:?}");
    let new = rig.host_of(first);
    assert!(new.rejoin.reserved_plane(PlaneId(0)).is_none());
}

/// A cold standby (its game of another class) restores its checkpoint and
/// replays the journal since: its world at T is the old host's too, and the
/// players fly on within 5 seconds.
#[test]
fn a_cold_standby_takes_over_at_the_same_world() {
    let other = Platform::ALL
        .into_iter()
        .find(|p| *p != Platform::current() && *p != Platform::Unknown)
        .unwrap();
    let (mut rig, first, _) = flying_on(20, true, other);
    rig.run(Duration::from_secs(3));
    rig.record_from = Some(rig.host().world.tick());
    rig.run(Duration::from_millis(500));
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()));
    rig.record_from = None;
    let (game, _, tick, coded) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    assert!(
        rig.checkpoints.get(&tick) == Some(&coded),
        "the cold standby's world at T differs from the old host's"
    );
    for (callsign, back) in rig.back_after_cut(Duration::from_secs(5)) {
        eprintln!(
            "{callsign}: snapshots again {:.0} ms after the cut (cold)",
            back.as_secs_f64() * 1e3
        );
    }
}

/// Slice S11: a host whose world makes no contrail puffs (as a dedicated
/// server's makes none) is cut off; the standby that takes over holds the
/// old host's world exactly (the switch is in the checkpoint), makes no
/// puffs either, and keeps the contrail sortie number, so every player's
/// own contrails go on at the same heights with no gap: they never came
/// from the host's puffs.
#[test]
fn a_host_without_contrail_puffs_is_taken_over_with_the_same_contrails() {
    let mut rig = Rig::new(crowd_spec(20));
    rig.host_mut().config.contrail_puffs = false;
    rig.host_mut().world.combat.set_contrail_puffs(false);
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    rig.fly();
    let (first, _) = rig.standbys();
    assert!(!rig.host().world.combat.contrail_puffs());
    let sortie = rig.host().world.combat.contrail_sortie();
    rig.run(Duration::from_secs(3));
    rig.record_from = Some(rig.host().world.tick());
    rig.run(Duration::from_millis(500));
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()));
    rig.record_from = None;
    let (game, _, tick, coded) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    assert!(
        rig.checkpoints.get(&tick) == Some(&coded),
        "the new host's world at T differs from the old host's"
    );
    rig.back_after_cut(Duration::from_secs(5));
    let world = &rig.host_of(game).world;
    assert!(!world.combat.contrail_puffs(), "the new host makes none");
    assert!(world.combat.contrails.puffs.is_empty());
    assert_eq!(world.combat.contrail_sortie(), sortie, "the same sortie");
}

/// Slice B6: the crowd fight in two-seaters. An AI two-seater shot down
/// ejects both its crew; the standbys, appointed again so their copy holds
/// it, restore a checkpoint taken mid-descent and the warm one's Checks stay
/// equal; the host is cut off, and the second crew member's chute falls on
/// from the new host's world, which codes to the old host's at T, and every
/// player still draws it.
#[test]
fn a_two_seaters_second_chute_falls_on_after_a_takeover() {
    use tore_world::test_support::resources::two_seat_resources;
    /// The bandits' lead, an AI two-seater.
    const VICTIM: u32 = 4;
    let mut rig = Rig::with_resources(crowd_spec(20), Arc::new(two_seat_resources()));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    rig.fly();
    let (first, _) = rig.standbys();
    // Shot down by hand, as combat leaves it; its escape monitor ejects the
    // crew in the next steps.
    shoot_down(rig.host_mut(), 0, VICTIM);
    let crew_of = |world: &World| {
        world
            .ai_wings
            .as_ref()
            .and_then(|w| w.crew_escapees().find(|(id, _)| *id == VICTIM))
            .map(|(_, escape)| escape.clone())
    };
    assert!(
        rig.run_until(Duration::from_secs(15), |r| crew_of(&r.host().world)
            .is_some()),
        "the AI two-seater's crew ejects"
    );
    // The standbys appointed again (a change by hand is not journaled), so
    // each restores a checkpoint holding both chutes.
    rig.host_mut().set_standbys_enabled(false);
    rig.run(Duration::from_millis(20));
    rig.host_mut().set_standbys_enabled(true);
    assert!(
        rig.run_until(Duration::from_secs(15), |r| r.host().ready_standbys().len()
            == 2),
        "the standbys ready again"
    );
    // A Check every 600 ticks: the warm standby's are equal.
    rig.run(Duration::from_secs(6));
    let figures = rig.host().standby_figures();
    assert!(figures.iter().all(|f| f.mismatches == 0), "{figures:?}");
    assert!(
        figures.iter().any(|f| f.warm && f.checks_equal >= 1),
        "{figures:?}"
    );

    rig.record_from = Some(rig.host().world.tick());
    rig.run(Duration::from_millis(200));
    rig.cut(0);
    assert!(
        rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()),
        "standby 1 takes over"
    );
    rig.record_from = None;
    let (game, _, tick, coded) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    let old = rig
        .checkpoints
        .get(&tick)
        .expect("the old host's world at T");
    assert!(
        *old == coded,
        "the new host's world at T differs from the old host's"
    );
    let at_t = crew_of(&rig.world_of(old)).expect("the second chute at T");
    assert!(
        !matches!(
            at_t.phase,
            tore_sim::ejection::Phase::Landed | tore_sim::ejection::Phase::Impact
        ),
        "mid-descent at T: {at_t:?}"
    );

    // The chute falls on from the new host, tick for tick, and every
    // player still draws two chutes for the aircraft.
    rig.run(Duration::from_secs(3));
    let new = rig.host_of(game).world();
    let now = crew_of(new).expect("the second chute on the new host");
    assert_eq!(now.ticks, at_t.ticks + (new.tick() - tick));
    assert!(now.position[1] < at_t.position[1], "{now:?} after {at_t:?}");
    for g in 1..rig.games.len() {
        let drawn: Vec<bool> = rig.games[g]
            .picture
            .iter()
            .flat_map(|p| p.pilots.iter())
            .filter(|p| p.owner == VICTIM)
            .map(|p| p.crew)
            .collect();
        assert_eq!(drawn, [false, true], "{}", rig.games[g].callsign);
    }
}

/// A host lost in the lobby: standby 1 takes the lobby over, the players
/// resume into it, the house mark moves, the crown passes once the old King
/// is dropped, and the mission flies from the new host.
#[test]
fn a_host_lost_in_the_lobby_moves_the_lobby() {
    let mut rig = Rig::new(crowd_spec(20));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    assert!(
        rig.run_until(Duration::from_secs(40), |r| r.host().ready_standbys().len()
            == 2),
        "two standbys ready in the lobby"
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| {
        r.games[1..]
            .iter()
            .all(|g| g.client.succession().is_some_and(|s| s.standbys.len() == 2))
    }));
    let (first, _) = rig.standbys();
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()));
    assert_eq!(rig.takeovers[0].0, first);
    let house = rig.games[first].client.lobby().unwrap().you;
    let players: Vec<usize> = (1..rig.games.len()).collect();
    assert!(
        rig.run_until(Duration::from_secs(3), |r| players.iter().all(|&g| {
            let client = &r.games[g].client;
            client.migration() == MigrationState::Steady
                && client.server()
                    == if g == first {
                        LINK_ADDRESS
                    } else {
                        r.games[first].address()
                    }
                && client.lobby().is_some_and(|l| l.host == Some(house))
        })),
        "every player is back in the new host's lobby"
    );
    assert_eq!(rig.host_of(first).phase(), Phase::Lobby);
    // The old King (the old house's player) is dropped: the crown passes.
    assert!(
        rig.run_until(Duration::from_secs(6), |r| r
            .host_of(first)
            .peers
            .values()
            .any(|p| p.king)),
        "the crown passes"
    );
    assert!(rig.run_until(Duration::from_secs(5), |r| {
        r.host_of(first).peers.values().all(|p| p.lobby.ready)
    }));
    rig.games[first].host.as_mut().unwrap().start_now();
    assert!(
        rig.run_until(Duration::from_secs(5), |r| players
            .iter()
            .all(|&g| r.flying(g))),
        "the mission flies from the new host"
    );
}

/// A player who joins after the host last sent its Succession is sent it
/// at once, and follows a migration like everyone else (slice K10: in
/// `net-reach-upload` a bot that joined late never heard of the standby,
/// so it dropped 5 seconds after the handover).
#[test]
fn a_late_joiner_follows_a_migration() {
    let mut rig = Rig::new(crowd_spec(20));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2)] {
        rig.join(callsign, plane, true);
    }
    rig.fly();
    let (first, _) = rig.standbys();
    rig.run(Duration::from_secs(1));
    // Hawk joins in flight, after the last Succession went out.
    let late = rig.join("Hawk", 3, true);
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.flying(late)),
        "the late joiner flies"
    );
    assert!(
        rig.games[late]
            .client
            .succession()
            .is_some_and(|s| s.standbys.len() == 2),
        "the late joiner holds the succession: {:?}",
        rig.games[late].client.succession()
    );
    // It holds everything else a migration needs: its rejoin token and the
    // standby marks of the lobby.
    assert!(rig.games[late].client.token().is_some(), "its rejoin token");
    let marks = rig.games[late]
        .client
        .lobby()
        .unwrap()
        .players
        .iter()
        .filter(|p| p.standby != messages::StandbyMark::None)
        .count();
    assert_eq!(marks, 2, "the standby marks");

    // The house leaves: the late joiner resumes with the new host.
    rig.run(Duration::from_millis(500));
    rig.host_mut().hand_over().unwrap();
    assert!(
        rig.run_until(Duration::from_secs(1), |r| r.host().handed_over()),
        "the handover's records through"
    );
    rig.games[0].gone = true;
    assert!(
        rig.run_until(Duration::from_secs(1), |r| !r.takeovers.is_empty()),
        "standby 1 takes over"
    );
    let (game, took_at, _, _) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    let new_host = rig.games[first].address();
    assert!(
        rig.run_until(Duration::from_secs(2), |r| {
            let client = &r.games[late].client;
            client.migration_counts().resumed == 1
                && client.migration() == MigrationState::Steady
                && client.server() == new_host
        }),
        "the late joiner resumes with the new host: {:?}, {:?}",
        rig.games[late].client.migration_counts(),
        rig.games[late].client.migration()
    );
    assert!(
        rig.run_until(Duration::from_secs(1), |r| r.games[late]
            .snapshot_after(took_at)
            .is_some()),
        "the late joiner has snapshots again"
    );
    assert!(rig.flying(late));
    assert!(
        rig.run_until(Duration::from_secs(2), |r| r
            .host_of(game)
            .absent_players()
            .is_empty()),
        "nobody is left absent"
    );
}

/// A player whose Resume reaches the new host after it went live (its game
/// slow to answer, here a long round trip to the new host): the new host
/// sends it no snapshot before Resumed names its flight. A snapshot of the
/// placeholder flight started that flight on the client, whose name table
/// had dropped the Names message ahead of it (messages are read before a
/// packet's sections), so the next Names was refused as out of order and
/// the client left with a protocol error (the flaky `net-window-migrate`).
#[test]
fn a_late_resume_gets_no_snapshot_before_resumed() {
    let (mut rig, first, second) = flying(20, true);
    let late = (1..rig.games.len())
        .find(|&g| g != first && g != second)
        .unwrap();
    rig.run(Duration::from_secs(1));
    // The late player's link to standby 1 has a 0.8 second round trip: it
    // connects within the resume window, and its Resume arrives after the
    // new host went live.
    let slow = LinkConfig::for_round_trip(800 * MS, 0., 0., 0.);
    let (a, b) = (rig.games[late].address(), rig.games[first].address());
    rig.net.set_link_both(a, b, slow);
    rig.cut(0);
    // Whatever the old host had on the way has arrived.
    rig.run(Duration::from_millis(100));
    let mut seen = rig.games[late].seen;
    let mut early = 0;
    let resumed = rig.run_until(Duration::from_secs(6), |r| {
        let game = &r.games[late];
        let resumed = game.client.migration_counts().resumed;
        if game.seen > seen && resumed == 0 {
            early += 1;
        }
        seen = game.seen;
        resumed == 1
    });
    assert!(resumed, "the late player resumes");
    let (_, took_at, _, _) = rig.takeovers[0].clone();
    assert!(
        !rig.host_of(first).resuming(),
        "the new host went live before the late Resume came"
    );
    assert_eq!(early, 0, "snapshots before Resumed");
    assert!(
        rig.run_until(Duration::from_secs(2), |r| r.games[late]
            .snapshot_after(took_at)
            .is_some()),
        "the late player has snapshots again"
    );
    assert!(rig.flying(late));
    assert_eq!(rig.games[late].client.server(), rig.games[first].address());
}

/// A player whose Resume the new host answered in the resume window, but
/// whose Backlog came only after the host went live (the battery's planned
/// handover, 2026-10-24, where the new host's own game was that player): the
/// host kept its kept inputs until the Backlog, took the Backlog, and sent
/// the player snapshots again. Before, going live dropped every store, the
/// Backlog was refused ("Nothing is being resumed for you here."), the player
/// counted as never done resuming, and no snapshot went to it again.
#[test]
fn a_backlog_after_the_host_went_live_is_taken_and_the_snapshots_follow() {
    let (mut rig, first, second) = flying(20, true);
    let late = (1..rig.games.len())
        .find(|&g| g != first && g != second)
        .unwrap();
    rig.run(Duration::from_secs(1));
    rig.cut(0);
    // Once the new host has answered the late player's Resume, the late
    // game is silent for 2 seconds, past the 1.5 second window: its Backlog
    // comes after the host went live.
    let late_address = rig.games[late].address();
    let answered = rig.run_until(Duration::from_secs(8), |r| {
        r.games[first].host.as_ref().is_some_and(|host| {
            let connection = host
                .peers
                .iter()
                .find(|(_, p)| p.address == late_address)
                .map(|(id, _)| *id);
            // Resuming, the player back and not waiting for Resumed: its
            // Resume answered, its Backlog not yet in.
            host.resuming()
                && connection.is_some_and(|c| {
                    !host.awaiting_resume(c) && r.games[late].client.migration_counts().resumed == 0
                })
        })
    });
    assert!(
        answered,
        "the new host answers the late Resume in its window"
    );
    let now = rig.net.now();
    rig.games[late].hold_until = Some(now + Duration::from_secs(2));
    let (_, took_at, _, _) = rig.takeovers[0].clone();
    assert!(
        rig.run_until(Duration::from_secs(2), |r| !r.host_of(first).resuming()),
        "the new host goes live without the late Backlog"
    );
    assert!(
        rig.net.now() < now + Duration::from_secs(2),
        "live while the late game is silent"
    );
    assert!(
        rig.run_until(Duration::from_secs(4), |r| r.games[late]
            .snapshot_after(now + Duration::from_secs(2))
            .is_some()),
        "the late player has snapshots again"
    );
    let refused: Vec<String> = rig.games[late]
        .events
        .iter()
        .filter_map(|(_, e)| match e {
            ClientEvent::Refused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect();
    assert!(refused.is_empty(), "refused: {refused:?}");
    assert!(rig.flying(late));
    assert_eq!(rig.games[late].client.migration_counts().resumed, 1);
    assert!(rig.games[late].snapshot_after(took_at).is_some());
    let host = rig.host_of(first);
    let connection = host
        .peers
        .iter()
        .find(|(_, p)| p.address == late_address)
        .map(|(id, _)| *id)
        .unwrap();
    assert!(!host.awaiting_resume(connection));
}

/// A takeover whose restore and replay took 5 seconds (a standby far
/// behind on a busy machine; `net-window-migrate-smoke`, 2026-10-24, saw
/// 4.8): the drop timeout counts from the new host's first update, so every
/// player resumes, with the host live by then, and none is dropped. It had
/// counted from before the replay, and every player, the new host's own
/// included, was dropped at its first update.
#[test]
fn a_takeover_that_took_seconds_to_replay_drops_nobody() {
    let (mut rig, _, _) = flying(20, true);
    rig.run(Duration::from_secs(2));
    rig.replay_time = DROP_AFTER;
    rig.cut(0);
    let back = rig.back_after_cut(Duration::from_secs(5));
    eprintln!("snapshots again after a 5-second replay: {back:?}");
    let (game, _, _, _) = rig.takeovers[0].clone();
    rig.run(DROP_AFTER);
    let dropped: Vec<String> = rig.games[game]
        .host
        .as_mut()
        .unwrap()
        .take_resume_notes()
        .into_iter()
        .filter_map(|note| match note {
            ResumeNote::Dropped { callsign } => Some(callsign),
            _ => None,
        })
        .collect();
    // Only the old host's own player, lost with it, is dropped.
    assert_eq!(dropped, ["Lead"]);
    let new = rig.host_of(game);
    assert!(!new.resuming(), "the new host is live");
    assert_eq!(new.absent_players(), Vec::<String>::new());
    for g in 1..rig.games.len() {
        assert!(rig.flying(g), "{} flies", rig.games[g].callsign);
        assert_eq!(rig.games[g].client.migration_counts().resumed, 1);
    }
    assert!(
        new.rejoin.reserved_plane(PlaneId(0)).is_some(),
        "the old host's plane is kept for its player"
    );
}

/// A standby the new host appoints in the middle of its fast-forward (the
/// battery's planned handover, 2026-10-24, saw one there fail its first
/// check on the real import): its checkpoint is of the world between two
/// fast-forward ticks, and the copy restored from it steps on as the new
/// host's world does, warm and with every check equal.
#[test]
fn a_standby_appointed_in_the_fast_forward_follows_the_new_host() {
    use crate::standby::Note;
    let (mut rig, first, _) = flying(20, true);
    rig.run(Duration::from_secs(3));
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(5), |r| !r.takeovers.is_empty()));
    let (game, _, tick, _) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    rig.games[game]
        .host
        .as_mut()
        .unwrap()
        .set_standbys_enabled(false);
    assert!(
        rig.run_until(Duration::from_secs(5), |r| {
            let host = r.host_of(game);
            host.resuming() && host.world.tick() > tick
        }),
        "the new host fast-forwards"
    );
    let from = rig.host_of(game).world.tick();
    rig.games[game]
        .host
        .as_mut()
        .unwrap()
        .set_standbys_enabled(true);
    rig.run(Duration::from_secs(20));
    let standbys: Vec<usize> = (1..rig.games.len()).filter(|&g| g != game).collect();
    for g in standbys {
        let notes: Vec<&Note> = rig.games[g].notes.iter().map(|(_, n)| n).collect();
        let restored = notes.iter().find_map(|n| match n {
            Note::Checkpoint {
                tick,
                restored: true,
            } => Some(*tick),
            _ => None,
        });
        let callsign = &rig.games[g].callsign;
        assert!(
            restored.is_some_and(|t| (from..=from + FAST_FORWARD_TICKS).contains(&t)),
            "{callsign} restored the new host's checkpoint from the fast-forward: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| matches!(n, Note::Warm { .. })),
            "{callsign} is warm: {notes:?}"
        );
        assert!(
            !notes
                .iter()
                .any(|n| matches!(n, Note::Mismatch { .. } | Note::Refused(_))),
            "{callsign}: {notes:?}"
        );
    }
    let figures = rig.host_of(game).standby_figures();
    assert_eq!(figures.len(), 2);
    for f in figures {
        assert!(f.checks_equal >= 1 && f.mismatches == 0, "{f:?}");
    }
}

/// The lobby moves to a better host (a handover in the lobby, as the host's
/// own succession update makes one): the old house's player resumes with
/// the new host as itself, the crown kept, rather than being dropped as
/// having left and welcomed back by its token (slice K10).
#[test]
fn a_lobby_handed_over_keeps_the_old_house_and_its_crown() {
    let mut rig = Rig::new(crowd_spec(20));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    assert!(
        rig.run_until(Duration::from_secs(40), |r| r.host().ready_standbys().len()
            == 2),
        "two standbys ready in the lobby"
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| {
        r.games
            .iter()
            .all(|g| g.client.succession().is_some_and(|s| s.standbys.len() == 2))
    }));
    let king = |host: &Host| {
        host.peers
            .values()
            .find(|p| p.king)
            .map(|p| p.callsign.clone())
    };
    assert_eq!(king(rig.host()).as_deref(), Some("Lead"));
    let (first, _) = rig.standbys();
    rig.host_mut().hand_over().unwrap();
    assert!(
        rig.run_until(Duration::from_secs(1), |r| r.host().handed_over()),
        "the handover's records through"
    );
    // The old host stops hosting; its game is a player of the new host.
    rig.games[0].host = None;
    rig.games[0].stepped_down = true;
    assert!(
        rig.run_until(Duration::from_secs(1), |r| !r.takeovers.is_empty()),
        "standby 1 takes over"
    );
    assert_eq!(rig.takeovers[0].0, first);
    let house = rig.games[first].client.lobby().unwrap().you;
    assert!(
        rig.run_until(Duration::from_secs(3), |r| (0..r.games.len()).all(|g| {
            let client = &r.games[g].client;
            client.migration() == MigrationState::Steady
                && client.lobby().is_some_and(|l| l.host == Some(house))
        })),
        "every player is in the new host's lobby"
    );
    let new = rig.host_of(first);
    assert!(
        new.absent_players().is_empty(),
        "{:?}",
        new.absent_players()
    );
    assert_eq!(
        king(new).as_deref(),
        Some("Lead"),
        "the King keeps the crown"
    );
    assert_eq!(rig.games[0].client.migration_counts().resumed, 1);
    let notices = rig.games[0].notices();
    assert!(
        !notices.iter().any(|n| n.starts_with("Welcome back")),
        "{notices:?}"
    );
    // Still the King 5 seconds on: nobody is dropped.
    rig.run(DROP_AFTER);
    assert_eq!(king(rig.host_of(first)).as_deref(), Some("Lead"));
}

/// The real-data 15 against 15 mission, as `tests/host_load.rs` and slice
/// K3's real-data test fly it: three wings of five F/A-18s against three of
/// five MiG-29s, 10 nm apart at 10,000 feet.
fn real_spec() -> MissionSpec {
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
    spec
}

/// Slice K10's measurement: the takeover's times on the real-data 15
/// against 15 mission with four humans (the house and three bots' games,
/// two standing by), for a kill (the host cut off in the fight) and for a
/// handover, with warm standbys and with cold ones (the bots' games on
/// another system), at the default snapshot rates. Simulated time on a
/// 40 ms round trip; the real time the takeover's replay took, and the
/// real time from the loss until every player had snapshots again (every
/// game's work on one thread), are printed beside it. Reads a real import
/// through `TORE_DATA_DIR`; run it in release:
///
/// ```sh
/// TORE_DATA_DIR=$PWD/.local/mpb-data-k10 cargo test --release --locked -p tore-session \
///     --lib real_data_15_against_15_takeovers -- --ignored --nocapture
/// ```
///
/// `K10_FIGHT_SECONDS` sets how long the fight runs before the loss (25 by
/// default, so a cold standby replays up to 10 seconds of journal).
#[test]
#[ignore = "reads a real import through TORE_DATA_DIR; the full suite runs it in release"]
fn real_data_15_against_15_takeovers_warm_and_cold() {
    let directory = tore_import::data_directory().expect("TORE_DATA_DIR");
    let import = Arc::new(tore_import::load(&directory).expect("an imported pack"));
    let other = Platform::ALL
        .into_iter()
        .find(|p| *p != Platform::current() && *p != Platform::Unknown)
        .unwrap();
    let fight = Duration::from_secs(
        std::env::var("K10_FIGHT_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(25),
    );
    for (label, platform) in [("warm", Platform::current()), ("cold", other)] {
        for handover in [false, true] {
            real_takeover(&import, label, platform, handover, fight);
        }
    }
}

fn real_takeover(
    import: &Arc<BTreeMap<String, Vec<u8>>>,
    label: &str,
    platform: Platform,
    handover: bool,
    fight: Duration,
) {
    let kind = if handover { "handover" } else { "kill" };
    let mut rig = Rig::with_resources(real_spec(), Arc::clone(import));
    rig.platform = platform;
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 5)] {
        rig.join(callsign, plane, false);
    }
    rig.fly();
    rig.run(fight);
    let kills: u32 = Rig::kills(&rig.host().world).values().sum();
    let missiles = Rig::missiles(&rig.host().world).len();
    let at = rig.net.now();
    let real = std::time::Instant::now();
    if handover {
        rig.host_mut().hand_over().unwrap();
        assert!(
            rig.run_until(Duration::from_secs(1), |r| r.host().handed_over()),
            "{label} {kind}: the handover's records through"
        );
        rig.games[0].gone = true;
    } else {
        rig.cut(0);
    }
    assert!(
        rig.run_until(Duration::from_secs(8), |r| !r.takeovers.is_empty()),
        "{label} {kind}: a standby takes over"
    );
    let (game, took_at, tick, _) = rig.takeovers[0].clone();
    let players: Vec<usize> = (1..rig.games.len()).collect();
    assert!(
        rig.run_until(Duration::from_secs(10), |r| players
            .iter()
            .all(|&g| r.games[g].snapshot_after(took_at).is_some())),
        "{label} {kind}: every player has snapshots again"
    );
    let real = real.elapsed();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    eprintln!(
        "{label} {kind}: {kills} kills, {missiles} guided missiles in flight at the loss; \
         {} took over {:.0} ms after the loss at tick {tick}; real time to every player back {:.0} ms",
        rig.games[game].callsign,
        ms(took_at - at),
        ms(real)
    );
    for note in rig.games[game].host.as_mut().unwrap().take_resume_notes() {
        match note {
            ResumeNote::TookOver { replayed, took, .. } => {
                eprintln!("  replayed {replayed} ticks in {:.1} ms (real)", ms(took))
            }
            ResumeNote::Resumed {
                callsign, after, ..
            } => eprintln!(
                "  {callsign} resumed {:.0} ms after the takeover",
                ms(after)
            ),
            ResumeNote::Live {
                after,
                fast_forward,
                ..
            } => eprintln!(
                "  live {:.0} ms after the takeover, {fast_forward} ticks fast-forwarded",
                ms(after)
            ),
            _ => {}
        }
    }
    for &g in &players {
        let noticed = rig.games[g]
            .events
            .iter()
            .find(|(t, e)| {
                *t >= at && matches!(e, ClientEvent::Notice(n) if n.starts_with("Lost contact"))
            })
            .map(|(t, _)| *t);
        let back = rig.games[g].snapshot_after(took_at).unwrap();
        eprintln!(
            "  {}: noticed {}, snapshots again {:.0} ms after the loss{}",
            rig.games[g].callsign,
            noticed.map_or_else(|| "-".to_owned(), |t| format!("{:.0} ms", ms(t - at))),
            ms(back - at),
            noticed.map_or_else(String::new, |t| format!(
                " ({:.0} ms after noticing)",
                ms(back.saturating_sub(t))
            )),
        );
        assert!(back - at < Duration::from_secs(5), "{label} {kind}");
    }
}

// The lobby pass's slice R1: an AI respawn pending at a takeover.
#[path = "ai_respawn_resume_tests.rs"]
mod ai_respawn_resume_tests;

/// Protocol 22, slice N1: the routed column's units keep moving through a
/// takeover. The old host hurts the standing tank (by hand, so the standbys
/// are appointed again to hold it) and is cut off while the column drives;
/// the standby's world at T codes to the old host's (the surface state is
/// in the checkpoint), every player resumes with the same surface digest,
/// and each draws the moving tank and is told the hurt one by the new host.
#[test]
fn a_takeover_carries_the_ground_target_and_every_player_resumes_on_it() {
    use tore_world::surface::{SURFACE_UNIT_BASE, UnitId};
    use tore_world::test_support::surface::{routed_resources, spec_with_target, target};
    const TANK: u32 = SURFACE_UNIT_BASE;
    const STANDING: u32 = SURFACE_UNIT_BASE + 2;
    let crowd = crowd_spec(20);
    let mut spec = spec_with_target(&target("QUCOL", 0, 0, 7));
    spec.wings = crowd.wings;
    spec.start = crowd.start;
    spec.separation_nm = crowd.separation_nm;
    let mut rig = Rig::with_resources(spec, Arc::new(routed_resources()));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    rig.fly();
    let (first, _) = rig.standbys();
    let hurt = {
        let row = rig
            .host_mut()
            .world
            .combat
            .state
            .targets
            .iter_mut()
            .find(|t| t.id == STANDING)
            .unwrap();
        row.hp /= 2;
        row.hp
    };
    rig.host_mut().set_standbys_enabled(false);
    rig.run(Duration::from_millis(20));
    rig.host_mut().set_standbys_enabled(true);
    rig.run(Duration::from_secs(3));
    let digest = rig.host().world.terrain.surface.digest();
    rig.record_from = Some(rig.host().world.tick());
    rig.run(Duration::from_millis(500));
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()));
    rig.record_from = None;
    let (game, _, tick, coded) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    assert!(
        rig.checkpoints.get(&tick) == Some(&coded),
        "the standby's surface at T differs from the old host's"
    );
    rig.back_after_cut(Duration::from_secs(5));
    rig.run(Duration::from_secs(1));
    let new = rig.host_of(first);
    assert_eq!(new.world().terrain.surface.digest(), digest);
    let mover = new
        .world()
        .combat
        .surface
        .unit(UnitId(TANK))
        .and_then(|u| u.mover)
        .expect("the tank drives on the new host");
    assert!(mover.speed_feet() > 0. || mover.leg > 0);
    let now = rig.net.now();
    for g in 1..rig.games.len() {
        if rig.games[g].gone {
            continue;
        }
        let callsign = rig.games[g].callsign.clone();
        let frame = rig.games[g]
            .client
            .frame(now)
            .unwrap_or_else(|| panic!("{callsign} flies on"));
        assert!(
            frame.picture.surface.iter().any(|p| p.id.0 == TANK),
            "{callsign} draws the tank"
        );
        assert_eq!(
            frame.surface_units.get(&STANDING).map(|v| v.hp),
            Some(hurt),
            "{callsign} is told the hurt tank"
        );
    }
}

/// Protocol 23, slice NA: three laser bombers (the synthetic F/A-18D's pod
/// and two GBU-10s) pin their sights, set their lasers by hand and release;
/// the host is cut off while the bombs fly. The standby's world at T codes
/// to the old host's (the sights, lasers and guided flights are in the
/// checkpoint), and the new host holds every pinned sight, every hand-set
/// laser and the bombs' flights, which end there.
#[test]
fn a_takeover_carries_the_laser_bombers_sights_lasers_and_bombs() {
    use crate::settings::{LoadoutRule, number};
    use tore_sim::combat::sight::SightMode;
    let mut spec = crate::wire::readout_a2g_tests::a2g_spec();
    spec.loadout = None;
    spec.wings[0].count = 4;
    let mut resources = tore_world::test_support::surface::routed_resources();
    tore_world::test_support::resources::add_a2g_stores(&mut resources);
    let mut rig = Rig::with_settings(
        spec,
        Arc::new(resources),
        &[(number::LOADOUTS, LoadoutRule::Any.value())],
    );
    let planes = [1, 2, 3];
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join_bomber(callsign, plane);
    }
    rig.fly();
    let (first, _) = rig.standbys();
    // Past every release (a bomb leaves four seconds after a game flies).
    rig.run(Duration::from_secs(5));
    let flights = |world: &World| -> Vec<u32> {
        world
            .combat
            .state
            .guided_flights()
            .map(|(id, _)| id)
            .collect()
    };
    let before = flights(&rig.host().world);
    assert!(!before.is_empty(), "bombs are guiding at the cut");
    rig.record_from = Some(rig.host().world.tick());
    rig.run(Duration::from_millis(500));
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()));
    rig.record_from = None;
    let (game, _, tick, coded) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    assert!(
        rig.checkpoints.get(&tick) == Some(&coded),
        "the standby's world at T differs from the old host's"
    );
    let at_t = rig.world_of(&coded);
    for plane in planes {
        let own = at_t.combat.state.ownship(plane).unwrap();
        assert!(
            matches!(own.sight.map(|s| s.mode), Some(SightMode::Pinned(_))),
            "plane {plane}'s pinned sight at T"
        );
        assert!(
            own.laser.manual.is_some() || own.laser.point.is_some(),
            "plane {plane}'s laser at T"
        );
    }
    assert!(!flights(&at_t).is_empty(), "bombs in flight at T");
    rig.back_after_cut(Duration::from_secs(5));
    // The bombs fly on and end on the new host.
    assert!(
        rig.run_until(Duration::from_secs(60), |r| flights(
            r.host_of(first).world()
        )
        .is_empty()),
        "the bombs end on the new host"
    );
    let new = rig.host_of(first).world();
    for plane in planes {
        assert!(
            new.combat.state.guided_impact(plane).is_some(),
            "plane {plane}'s bomb ended on the new host"
        );
    }
}

/// Slice FX-J: three laser bombers release; the one whose game is not a
/// standby leaves, and the host keeps its cockpit while the AI flies its
/// plane (the sight and the laser stepping on). The host is then cut off.
/// The standby's world at T codes to the old host's, the kept cockpit is in
/// it, the new host carries it on, and a human who takes that plane on the
/// new host's world finds the cockpit as it was left.
#[test]
fn a_takeover_carries_a_dropped_bombers_kept_cockpit() {
    use crate::settings::{LoadoutRule, number};
    use tore_sim::combat::sight::SightMode;
    let mut spec = crate::wire::readout_a2g_tests::a2g_spec();
    spec.loadout = None;
    spec.wings[0].count = 4;
    let mut resources = tore_world::test_support::surface::routed_resources();
    tore_world::test_support::resources::add_a2g_stores(&mut resources);
    let mut rig = Rig::with_settings(
        spec,
        Arc::new(resources),
        &[(number::LOADOUTS, LoadoutRule::Any.value())],
    );
    let mut games = Vec::new();
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        games.push((rig.join_bomber(callsign, plane), plane));
    }
    rig.fly();
    let (first, second) = rig.standbys();
    rig.run(Duration::from_secs(5));
    let (leaver, plane) = games
        .iter()
        .copied()
        .find(|(g, _)| *g != first && *g != second)
        .expect("a bomber that stands by for nobody");
    let left = rig
        .host()
        .world
        .combat
        .state
        .ownship(plane)
        .unwrap()
        .clone();
    assert!(matches!(
        left.sight.map(|s| s.mode),
        Some(SightMode::Pinned(_))
    ));
    let now = rig.net.now();
    rig.games[leaver].client.leave_game(now);
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r
            .host()
            .world
            .combat
            .state
            .kept(plane)
            .is_some()),
        "the host keeps the leaver's cockpit"
    );
    // Its game has left: nothing of it runs from here.
    rig.games[leaver].gone = true;
    rig.run(Duration::from_secs(1));
    rig.record_from = Some(rig.host().world.tick());
    rig.run(Duration::from_millis(500));
    rig.cut(0);
    assert!(rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()));
    rig.record_from = None;
    let (game, _, tick, coded) = rig.takeovers[0].clone();
    assert_eq!(game, first);
    assert!(
        rig.checkpoints.get(&tick) == Some(&coded),
        "the standby's world at T differs from the old host's"
    );
    let mut at_t = rig.world_of(&coded);
    let kept = at_t.combat.state.kept(plane).cloned().expect("kept at T");
    assert_eq!(kept.sight.map(|s| s.mode), left.sight.map(|s| s.mode));
    assert_eq!((kept.selected, kept.armed), (left.selected, left.armed));
    rig.back_after_cut(Duration::from_secs(5));
    assert!(
        rig.host_of(first)
            .world()
            .combat
            .state
            .kept(plane)
            .is_some(),
        "the new host carries the kept cockpit"
    );
    // A human takes the plane on the world at T: the cockpit as left.
    at_t.take_plane(
        tore_world::seats::SeatId(9),
        tore_world::seats::PlaneId(plane),
    )
    .unwrap();
    let own = at_t.combat.state.ownship(plane).unwrap();
    assert_eq!(own.sight.map(|s| s.mode), left.sight.map(|s| s.mode));
    assert_eq!((own.selected, own.armed), (left.selected, left.armed));
    assert_eq!(own.laser.manual, kept.laser.manual);
    assert_eq!(own.sensors.air_ground(), left.sensors.air_ground());
    assert!(at_t.combat.state.kept(plane).is_none());
}

/// The original host, stalled in a mission build for `stall` of simulated
/// time with its players and two standbys in the lobby: its loading thread
/// is stood in for by its players' loading keepalives every half second
/// when `loading` (protocol 28, slice NET2). Returns whether a standby took
/// over, the players still joined, and the players that heard the host
/// say it was loading.
fn host_build_stall(stall: Duration, loading: bool) -> (bool, usize, usize) {
    let mut rig = Rig::new(crowd_spec(20));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    assert!(
        rig.run_until(Duration::from_secs(40), |r| r.host().ready_standbys().len()
            == 2),
        "two standbys ready in the lobby"
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| {
        r.games[1..]
            .iter()
            .all(|g| g.client.succession().is_some_and(|s| s.standbys.len() == 2))
    }));
    let start = rig.net.now();
    let mut next = start + Duration::from_millis(250);
    let mut heard = BTreeSet::new();
    while rig.net.now() < start + stall {
        rig.net.advance(MS);
        let now = rig.net.now();
        if loading && now >= next {
            next += Duration::from_millis(500);
            for (to, datagram) in rig.host().server.loading_datagrams() {
                if to != LINK_ADDRESS {
                    rig.games[0].socket.send_datagram(to, &datagram).unwrap();
                }
            }
        }
        for index in 1..rig.games.len() {
            rig.step_game(index, now);
            rig.maybe_take_over(index, now);
            if rig.games[index].client.host_loading() {
                heard.insert(index);
            }
        }
        if !rig.takeovers.is_empty() {
            break;
        }
    }
    // The build is done: the host's loop turns again.
    rig.run(Duration::from_secs(2));
    let joined = (1..rig.games.len())
        .filter(|&g| {
            let client = &rig.games[g].client;
            client.phase() != ClientPhase::Closed
                && client.migration() == MigrationState::Steady
                && client.server() == host_address()
                && !client.host_loading()
        })
        .count();
    (!rig.takeovers.is_empty(), joined, heard.len())
}

/// A host that builds a mission for 20 seconds with its players and two
/// standbys in the lobby keeps every one of them through it, and no
/// standby takes over, when its loading keepalives say it is loading
/// (protocol 28, slice NET2).
#[test]
fn a_loading_host_keeps_its_players_and_standbys_through_a_long_build() {
    let (took_over, joined, heard) = host_build_stall(Duration::from_secs(20), true);
    assert!(
        !took_over,
        "a standby took the game over from a loading host"
    );
    assert_eq!((joined, heard), (3, 3));
}

/// Without the loading keepalives the same build is a lost host: standby 1
/// takes over after 1.5 seconds, as before protocol 28.
#[test]
fn a_host_silent_in_a_build_is_taken_over() {
    let (took_over, _, heard) = host_build_stall(Duration::from_secs(5), false);
    assert!(took_over);
    assert_eq!(heard, 0);
}

/// A host that said it was loading and then fell silent (it died in its
/// build) is still taken over, after the 4 seconds of loading silence.
#[test]
fn a_loading_host_that_dies_in_its_build_is_taken_over() {
    let mut rig = Rig::new(crowd_spec(20));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    assert!(
        rig.run_until(Duration::from_secs(40), |r| r.host().ready_standbys().len()
            == 2)
    );
    assert!(rig.run_until(Duration::from_secs(3), |r| {
        r.games[1..]
            .iter()
            .all(|g| g.client.succession().is_some_and(|s| s.standbys.len() == 2))
    }));
    for (to, datagram) in rig.host().server.loading_datagrams() {
        if to != LINK_ADDRESS {
            rig.games[0].socket.send_datagram(to, &datagram).unwrap();
        }
    }
    let said = rig.net.now();
    rig.games[0].gone = true;
    assert!(rig.run_until(Duration::from_secs(10), |r| !r.takeovers.is_empty()));
    let after = rig.takeovers[0].1.saturating_sub(said);
    assert!(
        after >= Duration::from_secs(4) && after < Duration::from_millis(4_500),
        "taken over {after:?} after the last loading keepalive"
    );
}

/// The host's own build sends each player's loading keepalive on the
/// socket the program gave it, a quarter second into the build and twice a
/// second after, and logs how long it took (protocol 28, slice NET2).
#[test]
fn a_slow_build_sends_loading_keepalives_and_logs_the_build() {
    use std::sync::Mutex as StdMutex;
    type Sent = Vec<(SocketAddr, Vec<u8>)>;
    #[derive(Clone, Default)]
    struct Captured(Arc<StdMutex<Sent>>);
    impl Datagrams for Captured {
        fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
            self.0.lock().unwrap().push((to, datagram.to_vec()));
            Ok(())
        }
        fn recv_datagram(&mut self, _: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
            Ok(None)
        }
    }
    let mut rig = Rig::new(crowd_spec(20));
    rig.join("Viper", 1, true);
    rig.join("Cobra", 2, true);
    let captured = Captured::default();
    let theirs = captured.clone();
    let host = rig.host_mut();
    host.set_loading_socket(Box::new(move || {
        Ok(Box::new(theirs.clone()) as Box<dyn Datagrams + Send>)
    }));
    host.set_loading_config(tore_net::LoadingConfig {
        quiet: 20 * MS,
        interval: 100 * MS,
        ..tore_net::LoadingConfig::default()
    });
    host.set_slow_build(Some(Duration::from_millis(600)));
    while host.poll_log().is_some() {}
    let spec = host.spec.clone();
    host.build(&spec).unwrap();
    let sent = captured.0.lock().unwrap().clone();
    let players: BTreeSet<SocketAddr> = sent.iter().map(|(to, _)| *to).collect();
    // The two players and the house's own player over the link.
    assert_eq!(players.len(), 3, "{players:?}");
    assert!(players.contains(&LINK_ADDRESS));
    for (_, datagram) in &sent {
        let (kind, body) = tore_net::packet::open(datagram, crate::wire::PROTOCOL_VERSION).unwrap();
        assert_eq!(kind, tore_net::packet::PacketKind::Keepalive);
        assert!(tore_net::packet::decode_keepalive(body).unwrap().loading);
    }
    let rounds = sent.len() / 3;
    assert!((2..=7).contains(&rounds), "{rounds} rounds");
    let logged: Vec<String> = std::iter::from_fn(|| rig.host_mut().poll_log())
        .filter_map(|entry| entry.loading_text())
        .collect();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(logged[0].contains("3 players kept"), "{}", logged[0]);
}

/// Standby 1 takes over after the cut and every remote player resumes: each
/// one's migration counts.
fn resumed_counts(rig: &mut Rig) -> Vec<(String, MigrationCounts)> {
    assert!(
        rig.run_until(Duration::from_secs(3), |r| !r.takeovers.is_empty()),
        "standby 1 takes over"
    );
    let _ = rig.back_after_cut(Duration::from_secs(5));
    (1..rig.games.len())
        .map(|g| {
            let game = &rig.games[g];
            (game.callsign.clone(), game.client.migration_counts())
        })
        .collect()
}

/// Slice NET2: the old host repeats every remote player's inputs for the
/// last 150 ms before it is cut (they stop reaching it). Its state at T is
/// then not what those players predicted, so each resume corrects the plane,
/// as the next snapshot would have: inherited, not corrected by the replay.
#[test]
fn inputs_the_old_host_repeated_before_the_cut_are_inherited_at_the_resume() {
    let (mut rig, _, _) = flying(5, false);
    rig.run(Duration::from_secs(3));
    let host = rig.games[0].address();
    let lost = LinkConfig {
        loss: 1.,
        ..LinkConfig::for_round_trip(40 * MS, 0., 0., 0.)
    };
    let repeats = |rig: &Rig| -> u64 {
        rig.host()
            .peers
            .values()
            .filter(|p| p.callsign != "Lead")
            .map(|p| p.inputs.repeats_total())
            .sum()
    };
    let before = repeats(&rig);
    for g in 1..rig.games.len() {
        let from = rig.games[g].address();
        rig.net.set_link(from, host, lost);
    }
    rig.run(Duration::from_millis(150));
    assert!(repeats(&rig) > before, "the old host repeated their inputs");
    rig.cut(0);
    for (callsign, counts) in resumed_counts(&mut rig) {
        assert_eq!(counts.resumed, 1, "{callsign}");
        assert_eq!(counts.corrected, 0, "{callsign}: {counts:?}");
        assert_eq!(counts.inherited, 1, "{callsign}: {counts:?}");
    }
}

/// Slice NET2: the host's whole game stalls for 400 ms (an overloaded
/// machine), catches up for a moment and is cut. No input was late, so no
/// resume corrects anything.
#[test]
fn a_host_that_lagged_before_the_cut_corrects_no_plane_at_the_resume() {
    let (mut rig, _, _) = flying(5, false);
    rig.run(Duration::from_secs(3));
    rig.stalled = true;
    rig.run(Duration::from_millis(400));
    rig.stalled = false;
    rig.run(Duration::from_millis(1));
    rig.cut(0);
    for (callsign, counts) in resumed_counts(&mut rig) {
        assert_eq!(counts.resumed, 1, "{callsign}");
        assert_eq!(counts.corrected, 0, "{callsign}: {counts:?}");
        assert_eq!(counts.inherited, 0, "{callsign}: {counts:?}");
    }
}

#[test]
fn the_mission_countdown_keeps_its_deadline_through_handover() {
    let mut rig = Rig::new(crowd_spec(150));
    rig.games[0]
        .client
        .change_settings(crate::wire::messages::SettingsChange {
            values: vec![(crate::settings::number::TIME_LIMIT, 600)],
            ..Default::default()
        });
    rig.run(Duration::from_millis(200));
    assert_eq!(rig.host().settings().time_limit_seconds(), Some(600));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    rig.fly();
    rig.run(Duration::from_millis(2500));
    let before = rig.games[1].client.seconds_left().unwrap();
    rig.host_mut().hand_over().unwrap();
    assert!(rig.run_until(Duration::from_secs(1), |r| r.host().handed_over()));
    rig.games[0].gone = true;
    assert!(rig.run_until(Duration::from_secs(1), |r| !r.takeovers.is_empty()));
    let (game, took_at, _, _) = rig.takeovers[0].clone();
    assert!(rig.run_until(Duration::from_secs(2), |r| {
        (1..r.games.len()).all(|g| r.games[g].snapshot_after(took_at).is_some())
    }));
    rig.run(Duration::from_millis(500));
    let host = rig.host_of(game);
    let expected = crate::client::scores::remaining_seconds(
        host.settings().time_limit_seconds(),
        host.world().tick(),
    )
    .unwrap();
    assert!(expected <= before && before - expected <= 3);
    for g in 1..rig.games.len() {
        assert_eq!(rig.games[g].client.seconds_left(), Some(expected));
    }
}

#[test]
fn departed_player_and_side_scores_survive_handover_and_still_reach_the_limit() {
    use crate::settings::{Mode, number};
    use tore_world::score::{Fact, Flown, Victim};
    let mut rig = Rig::new(crowd_spec(150));
    rig.games[0]
        .client
        .change_settings(crate::wire::messages::SettingsChange {
            values: vec![(number::MODE, Mode::Pvp.value()), (number::KILL_LIMIT, 3)],
            ..Default::default()
        });
    rig.run(Duration::from_millis(200));
    for (callsign, plane) in [("Viper", 1), ("Cobra", 2), ("Hawk", 3)] {
        rig.join(callsign, plane, true);
    }
    rig.games[0].client.ready(Some(0));
    rig.run(Duration::from_millis(500));
    rig.fly();
    rig.run(Duration::from_millis(1500));
    let credit = |host: &mut Host, plane: u32| {
        let flown = Flown {
            plane: PlaneId(plane),
            pilot: host.world.roster.plane(PlaneId(plane)).unwrap().pilot,
        };
        let seats = host
            .peers
            .values()
            .filter_map(|p| Some((p.seat?, p.lobby.order)))
            .collect();
        host.tally(
            Fact::Kill {
                shooter: Some(flown),
                victim: Victim {
                    target: 4,
                    flown: Some(Flown {
                        plane: PlaneId(4),
                        pilot: Pilot::Ai,
                    }),
                    aircraft: true,
                },
                pilot_aboard: true,
            },
            &seats,
        );
    };
    credit(rig.host_mut(), 1);
    credit(rig.host_mut(), 1);
    let order = rig
        .host()
        .peers
        .values()
        .find(|p| p.callsign == "Viper")
        .unwrap()
        .lobby
        .order;
    // The accounting part includes nonzero shot and kill totals too.
    let totals = rig
        .host_mut()
        .score
        .accounts
        .totals
        .entry(order)
        .or_default();
    totals.kills[0] = 2;
    totals.air_to_air.launched = 4;
    totals.air_to_air.hit = 2;
    rig.host_mut().score.accounts.side_hits[0] = 2;
    rig.run(Duration::from_millis(1200));
    rig.games[1].client.leave_game(rig.net.now());
    assert!(
        rig.run_until(Duration::from_secs(3), |r| r.games[1].client.phase()
            == ClientPhase::Closed)
    );
    rig.games[1].gone = true;
    rig.run(Duration::from_secs(2));
    assert_eq!(
        rig.host().world.roster.plane(PlaneId(1)).unwrap().pilot,
        Pilot::Ai
    );
    rig.host_mut().hand_over().unwrap();
    assert!(rig.run_until(Duration::from_secs(1), |r| r.host().handed_over()));
    rig.games[0].gone = true;
    assert!(rig.run_until(Duration::from_secs(2), |r| !r.takeovers.is_empty()));
    let (game, took_at, _, _) = rig.takeovers[0].clone();
    assert!(rig.run_until(Duration::from_secs(2), |r| {
        (2..r.games.len()).all(|g| r.games[g].snapshot_after(took_at).is_some())
    }));
    let host = rig.host_of(game);
    let scores = host.final_scores(EndReason::EndedByServer);
    assert_eq!(scores.sides[0].kills, 2);
    assert_eq!(scores.sides[0].hits, 2);
    assert!(
        scores
            .players
            .iter()
            .any(|p| p.callsign == "Viper" && p.left && p.kills == 2)
    );
    assert_eq!(host.score.accounts.totals[&order].air_to_air.hit, 2);
    // The old contribution plus a remaining player's kill ends the mission.
    credit(rig.games[game].host.as_mut().unwrap(), 3);
    rig.run(Duration::from_secs(1));
    let results = rig.games[2].client.results().unwrap();
    assert_eq!(results.reason, EndReason::KillLimit);
    assert_eq!(results.scores.as_ref().unwrap().sides[0].kills, 3);
    assert!(results.rows.iter().any(|r| r.plane == 1 && r.left));
}
