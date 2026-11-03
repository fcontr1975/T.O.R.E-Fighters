//! The game's end of a networked flight: the client session, its socket and
//! files, the mission the drawn half is built from, and the plain data the
//! flight screen takes from them each frame. Windows, sound and the screens
//! stay in `main.rs`; this file holds what needs no `App`.
//!
//! The mission is built twice before the player is asked for a plane: once
//! by the client session (`World::new`, open seating, which is what the
//! manifest check reads) and once here with the game's own loaders, so the
//! drawn model of every aircraft type loads the way the Quick Mission
//! creator loads them and `CombatView` can draw it. The second world is the
//! app's copy of the mission: never stepped, it gives the screens the
//! terrain, the roster and the aircraft types (agent decision).
//!
//! A join through the Internet Lobby ([`Transport::Internet`]) carries on
//! here after the master's introduction (slice J5): the race of the host's
//! addresses, the relay when none answers, the 15-second give-up and a line
//! in Messages for each step ([`MasterTransport`]; `docs/ARCHITECTURE.md`,
//! "Joining through the master").
//!
//! Stage K (slice K7a; docs/ARCHITECTURE.md, "Host migration and rejoin"):
//! a game joined by address binds a dual-stack socket on the game port when
//! it is free ([`Join::to`]), its future host socket. A peers router
//! (`tore_net::peers::Peers`) sits in front of every joined socket: it
//! answers the host's reach tests and passes the rest to the client. A
//! joined game that may host runs a standby ([`crate::net::standby`]) and,
//! when its client says so, takes the game over: the socket goes to a new
//! hosting thread ([`HostThread::take_over`]), the client resumes with its
//! own new host over the in-process link, and the keepalive thread stops. A
//! hosting game whose thread steps down has its socket back and its client
//! resumes with the game's new host like any other.
use crate::{
    aircraft::Airframe,
    aircraft_type::AircraftType,
    net::{
        files::{self, DatedLog},
        guns::{self, Guns},
        hosting::{self, End, HostThread, Report, Side, TakeoverSetup},
        options::ConnectOptions,
        rejoin_store::{GameStore, Store},
        standby::{GameStandby, HeldSpec, note_line},
    },
    regen::{self, DeviceRelease, Effects, Motor},
    replay::library::Library,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{self, BufWriter},
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};
use tore_formats::aircraft::AircraftId;
use tore_net::master::candidate::canonical;
use tore_net::master::join::{JoinEvent, Joiner, RelayState};
use tore_net::master::local::{host_candidates, own_address_toward};
use tore_net::master::{
    JOIN_GIVE_UP, MappingType, Path as JoinedPath, RACE_BEFORE_RELAY, is_relayed,
};
use tore_net::peers::{Peers, Route};
use tore_net::{
    CloseReason, DEFAULT_PORT, Datagrams, Entropy, Keepalive, KeepaliveConfig, LINK_ADDRESS,
    LinkEnd, Listen, RealClock, ServerSocket,
};
use tore_session::{
    BuildId, Client, ClientConfig, ClientEvent, ClientFrame, ClientPhase, Controls,
    client::candidate::CandidateSettings,
    host::{Resumption, content::GameContent},
    wire::events::WireEvent,
};
use tore_sim::attitude::{Basis, Vector};
use tore_sim::combat::countermeasures::Release;
use tore_sim::combat::live::EffectKind;
use tore_world::snapshot::RenderSnapshot;
use tore_world::{
    WorldResult,
    mission::MissionSpec,
    resources::ResourceSource,
    world::{Hooks, Seating, World},
};

/// What the game's own build of the mission leaves for the screens.
pub struct Built {
    pub world: World,
    /// The drawn model of each aircraft type, in the order the world holds
    /// the types (`CombatView` draws them in that order).
    pub models: Vec<Airframe>,
    /// The mission's markup for the in-flight map, read from the world as
    /// built, before it flies: the same starts and objective area the
    /// creator's map shows, on every machine of the game.
    pub markup: crate::flight_map::Plan,
}

/// The build this game is, which the host's must match.
pub fn build_id() -> BuildId {
    BuildId {
        compatibility: compatibility_code(),
        version: crate::version::version().to_owned(),
        commit: crate::version::commit().to_owned(),
        // The same rule as the server and the bot: a stamped tag is a
        // release build.
        release: option_env!("TORE_BUILD_VERSION").is_some(),
    }
}

/// The code of the content worker's import, or the selected installed profile
/// for command-line discovery before the menu has loaded resources.
pub fn compatibility_code() -> u32 {
    let resources = {
        let slot = CONTENT.0.lock().unwrap_or_else(|e| e.into_inner());
        match &*slot {
            ContentSlot::Ready(_, content) => {
                return tore_session::compatibility::current(&content.content);
            }
            ContentSlot::Computing(resources, _) => Some(Arc::clone(resources)),
            ContentSlot::Idle => None,
        }
    };
    if let Some(resources) = resources {
        return tore_session::compatibility::current(&game_content(&resources).content);
    }
    static INSTALLED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *INSTALLED.get_or_init(|| {
        tore_import::data_directory()
            .ok()
            .and_then(|dir| tore_session::compatibility::installed(&dir).ok())
            .unwrap_or_else(|| tore_session::compatibility::current(&Default::default()))
    })
}

/// What the game's multiplayer screens know about this game's content
/// (stage L, slice L4): the import's source, set once at start-up, and the
/// content computed from it, once for each import.
static SOURCE: std::sync::Mutex<Option<tore_import::source::Source>> = std::sync::Mutex::new(None);
static CONTENT: ContentCache = ContentCache::new();

/// Where this game's content stands.
enum ContentSlot {
    /// Not asked for yet.
    Idle,
    /// A worker is computing it for these resources.
    Computing(
        Arc<BTreeMap<String, Vec<u8>>>,
        std::thread::JoinHandle<Arc<GameContent>>,
    ),
    /// Computed for these resources.
    Ready(Arc<BTreeMap<String, Vec<u8>>>, Arc<GameContent>),
}

/// Notes the import's Fighters Anthology build and importer (`Source::read`,
/// from the pack's own entry or, for an older pack, the import report), as
/// the game starts (and again after a re-import, which starts it afresh). A
/// game that never calls this reads the pack's entry alone.
pub fn remember_source(multiplayer: &BTreeMap<String, Vec<u8>>) {
    let data = crate::assets::data_directory().ok();
    let source = match &data {
        Some(data) => tore_import::source::Source::read(data, multiplayer),
        None => tore_import::source::Source::UNKNOWN,
    };
    *SOURCE.lock().unwrap_or_else(|e| e.into_inner()) = Some(source);
}

/// Nonblocking gate used before a windowed join consumes the content cache.
pub fn content_ready(resources: &Arc<BTreeMap<String, Vec<u8>>>) -> bool {
    let Ok(mut slot) = CONTENT.0.try_lock() else {
        return false;
    };
    ContentCache::start_in(&mut slot, resources);
    match &*slot {
        ContentSlot::Ready(_, _) => true,
        ContentSlot::Computing(_, handle) => handle.is_finished(),
        ContentSlot::Idle => false,
    }
}

/// This game's content over `resources` with the import's source.
fn compute_content(resources: &BTreeMap<String, Vec<u8>>) -> GameContent {
    let source = SOURCE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match source {
        Some(source) => GameContent::with_source(resources, source),
        None => GameContent::of(resources),
    }
}

/// Starts working out this game's content on a worker, when a multiplayer
/// screen first opens, so the first join or host does not wait for it (0.23
/// seconds in a release build, about 3 in a debug one). Does nothing when it
/// is under way or done for these resources.
pub fn start_content(resources: &Arc<BTreeMap<String, Vec<u8>>>) {
    CONTENT.start(resources);
}

/// This game's content over `resources`: the worker's, waiting for it when it
/// is still at work, else worked out now.
pub fn game_content(resources: &Arc<BTreeMap<String, Vec<u8>>>) -> Arc<GameContent> {
    CONTENT.get(resources)
}

/// The content worked out for one import at a time. The game keeps one
/// ([`CONTENT`]); a test keeps its own, so tests running side by side cannot
/// swap each other's import out between two calls.
struct ContentCache(std::sync::Mutex<ContentSlot>);

impl ContentCache {
    const fn new() -> Self {
        Self(std::sync::Mutex::new(ContentSlot::Idle))
    }

    fn start(&self, resources: &Arc<BTreeMap<String, Vec<u8>>>) {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Self::start_in(&mut slot, resources);
    }

    fn start_in(slot: &mut ContentSlot, resources: &Arc<BTreeMap<String, Vec<u8>>>) {
        let current = match &*slot {
            ContentSlot::Idle => None,
            ContentSlot::Computing(have, _) | ContentSlot::Ready(have, _) => Some(have),
        };
        if current.is_some_and(|have| Arc::ptr_eq(have, resources)) {
            return;
        }
        let work = Arc::clone(resources);
        let handle = std::thread::Builder::new()
            .name("content".into())
            .spawn(move || Arc::new(compute_content(&work)));
        *slot = match handle {
            Ok(handle) => ContentSlot::Computing(Arc::clone(resources), handle),
            // No thread: it is worked out when it is asked for.
            Err(_) => ContentSlot::Idle,
        };
    }

    /// Starts and collects under one lock, so the content handed back is
    /// always the content of `resources`.
    fn get(&self, resources: &Arc<BTreeMap<String, Vec<u8>>>) -> Arc<GameContent> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Self::start_in(&mut slot, resources);
        match std::mem::replace(&mut *slot, ContentSlot::Idle) {
            ContentSlot::Computing(have, handle) => {
                let content = handle
                    .join()
                    .unwrap_or_else(|_| Arc::new(compute_content(&have)));
                *slot = ContentSlot::Ready(have, Arc::clone(&content));
                content
            }
            ContentSlot::Ready(have, content) => {
                *slot = ContentSlot::Ready(have, Arc::clone(&content));
                content
            }
            ContentSlot::Idle => Arc::new(compute_content(resources)),
        }
    }
}

/// Builds the mission of `spec` with the game's loaders: the drawn model of
/// every aircraft type loads beside its simulation half.
fn build_mission(spec: &MissionSpec, resources: &BTreeMap<String, Vec<u8>>) -> WorldResult<Built> {
    let mut models = Vec::new();
    let mut load = |id| -> WorldResult<Arc<AircraftType>> {
        let model = Airframe::load(resources, id)?;
        let kind = Arc::clone(&model.kind);
        models.push(model);
        Ok(kind)
    };
    let built = World::build(
        spec,
        resources,
        Seating::Open,
        &mut Hooks {
            inspect_invalid_placement: false,
            player: None,
            load: Some(&mut load),
            weapon_label: None,
            ground_variation: None,
        },
    )?;
    let markup = crate::quick_mission::map::mission_plan(&built.world, false, None);
    Ok(Built {
        world: built.world,
        models,
        markup,
    })
}

/// Where the second build of the mission is left for the game.
type Sink = Arc<Mutex<Option<Result<Built, String>>>>;

/// What carries the session's datagrams: a UDP socket to a server, the
/// in-process link to the host this game runs itself, or the socket a join
/// through the master was introduced on.
pub enum Transport {
    /// A plain socket of one family (before stage K a join by address; the
    /// tests still join on one).
    #[cfg_attr(not(test), allow(dead_code))]
    Udp(UdpSocket),
    Link(LinkEnd),
    /// A join through the Internet Lobby (slices I4 and J5, with J2's joiner
    /// and J3's relay).
    Internet(Box<MasterTransport>),
    /// A join by address on a socket that can host the game later (stage
    /// K): dual-stack, on the game port when it was free.
    Joined(ServerSocket),
    /// A game that took the game over (stage K): its own host over the
    /// link, and the old host on the socket the hosting thread now reads.
    Hosted(Box<Hosted>),
    /// Nothing: a takeover could not keep the socket. Sends are dropped.
    Gone,
}

/// The transport of a game that took the game over (stage K, slice K7a):
/// its client joins its own new host over the link and keeps its old
/// connection a while on the socket, which the hosting thread reads, the
/// old host's datagrams coming to the [`Side`].
pub struct Hosted {
    link: LinkEnd,
    /// Another handle to the socket, to send to the old host.
    socket: ServerSocket,
    side: Arc<Side>,
}

/// How a join through the Internet Lobby may reach the host, as `tore-bot
/// --path` says it: race the host's addresses and ask for the relay when none
/// answers (the default), race only, or the relay only. *Agent decision
/// (J5):* the game reads it from the environment variable `TORE_JOIN_PATH`
/// (`auto`, `direct` or `relay`), for tests on one machine, where every
/// direct path works; a player never meets it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JoinPath {
    #[default]
    Auto,
    Direct,
    Relay,
}

impl JoinPath {
    /// The environment variable the game reads it from.
    pub const VARIABLE: &str = "TORE_JOIN_PATH";

    /// `auto`, `direct` or `relay`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Self::Auto),
            "direct" => Some(Self::Direct),
            "relay" => Some(Self::Relay),
            _ => None,
        }
    }

    /// The path `TORE_JOIN_PATH` names; `Auto` when it is not set, or (said
    /// in the log) not one of the three.
    pub fn from_env() -> Self {
        let Ok(text) = std::env::var(Self::VARIABLE) else {
            return Self::Auto;
        };
        Self::parse(&text).unwrap_or_else(|| {
            log::warn!(
                "{}={text:?} is not auto, direct or relay; joining as auto",
                Self::VARIABLE
            );
            Self::Auto
        })
    }
}

/// What a join through the master brings to the session besides its socket
/// and joiner: the race, the master's hint, the path allowed and the time
/// already spent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Introduction {
    /// The host's addresses to race, and the introduction's id.
    pub race: tore_session::client::Race,
    /// The master said only the relay reaches the host.
    pub relay_now: bool,
    /// How the join may reach the host.
    pub path: JoinPath,
    /// From pressing Join to the introduction; the whole join gives up
    /// [`JOIN_GIVE_UP`] after the press.
    pub asked: Duration,
}

/// The socket a join through the master was introduced on, read and written
/// through its joiner so the master's datagrams never reach the transport,
/// the race the client runs from it, and the relay when the race finds
/// nothing (slice J5; "Joining through the master" in the architecture
/// guide). [`NetSession::pump`] gives it its turn.
pub struct MasterTransport {
    socket: ServerSocket,
    joiner: Joiner,
    /// The session clock at the last turn: the router's and the joiner's
    /// time (agent decision: one clock for the session, the race and the
    /// relay).
    now: Duration,
    race: Option<tore_session::client::Race>,
    relay_now: bool,
    path: JoinPath,
    asked: Duration,
    /// When the race started, and when the whole join gives up, on the
    /// session clock.
    raced: Duration,
    give_up: Duration,
    /// The relay was asked for (at most once).
    relay_asked: bool,
}

impl MasterTransport {
    /// The introduced socket and joiner, and what the introduction said.
    pub fn new(socket: ServerSocket, joiner: Joiner, introduction: Introduction) -> Box<Self> {
        let Introduction {
            race,
            relay_now,
            path,
            asked,
        } = introduction;
        Box::new(Self {
            socket,
            joiner,
            now: Duration::ZERO,
            race: Some(race),
            relay_now,
            path,
            asked,
            raced: Duration::ZERO,
            give_up: JOIN_GIVE_UP.saturating_sub(asked),
            relay_asked: false,
        })
    }

    /// The race starts at `now` on the session clock.
    fn begin(&mut self, now: Duration) {
        self.now = now;
        self.raced = now;
        self.give_up = now + JOIN_GIVE_UP.saturating_sub(self.asked);
    }

    /// Why to ask for the relay now, as Messages says it, while no address
    /// has answered (`chosen` false): at once when only the relay is allowed
    /// or the master's hint says so, after [`RACE_BEFORE_RELAY`] otherwise,
    /// never for a join that races only or has asked already.
    fn relay_reason(&self, now: Duration, chosen: bool) -> Option<&'static str> {
        if self.relay_asked || chosen {
            return None;
        }
        match self.path {
            JoinPath::Direct => None,
            JoinPath::Relay => Some("Asking for the relay..."),
            JoinPath::Auto if self.relay_now => Some(
                "The Internet Lobby says only the relay reaches this game; asking for the relay...",
            ),
            JoinPath::Auto if now >= self.raced + RACE_BEFORE_RELAY => {
                Some("No direct path; asking for the relay...")
            }
            JoinPath::Auto => None,
        }
    }

    /// Why a join that never connected ended, for `label`.
    fn no_answer_text(&self, label: &str) -> String {
        if self.relay_asked {
            format!("Could not reach '{label}' directly or through the relay.")
        } else {
            format!("No answer from '{label}' at any of its addresses.")
        }
    }

    /// The player's own mapping type and the game bytes relayed each way,
    /// for the player's Report.
    fn facts(&self) -> ThroughFacts {
        let relay = self.joiner.relay_counters();
        ThroughFacts {
            mapping: self.joiner.mapping(),
            relayed_bytes: relay.bytes_in + relay.bytes_out,
            ..ThroughFacts::default()
        }
    }
}

/// What a join through the Internet Lobby tells the player's Report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThroughFacts {
    /// How the player's router maps the joining socket.
    pub mapping: MappingType,
    /// The game's bytes through the relay, both ways.
    pub relayed_bytes: u64,
    /// Host migrations the session resumed through, and those it was lost
    /// to (stage K).
    pub migrations: u8,
    pub failed_migrations: u8,
}

impl Default for ThroughFacts {
    fn default() -> Self {
        Self {
            mapping: MappingType::Unknown,
            relayed_bytes: 0,
            migrations: 0,
            failed_migrations: 0,
        }
    }
}

impl Datagrams for Transport {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        // A hosting game's own player, whose host steps down, still names
        // the link for a moment: nothing goes there over a socket.
        if to == LINK_ADDRESS && !matches!(self, Self::Link(_) | Self::Hosted(_)) {
            return Ok(());
        }
        match self {
            Self::Udp(socket) => socket.send_datagram(to, datagram),
            Self::Link(link) => link.send_datagram(to, datagram),
            Self::Joined(socket) => socket.send_datagram(to, datagram),
            Self::Hosted(hosted) if to == LINK_ADDRESS => hosted.link.send_datagram(to, datagram),
            Self::Hosted(hosted) => hosted.socket.send_datagram(to, datagram),
            Self::Gone => Ok(()),
            Self::Internet(t) => {
                // The relay only: nothing goes to the host's own addresses,
                // so the race cannot win on a machine where every path works.
                if t.path == JoinPath::Relay && !is_relayed(to) {
                    return Ok(());
                }
                let now = t.now;
                t.joiner
                    .over(&mut t.socket, now)
                    .send_datagram(to, datagram)
            }
        }
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        match self {
            Self::Udp(socket) => socket.recv_datagram(buf),
            Self::Link(link) => link.recv_datagram(buf),
            Self::Internet(t) => {
                let now = t.now;
                t.joiner.over(&mut t.socket, now).recv_datagram(buf)
            }
            Self::Joined(socket) => Ok(socket
                .recv_datagram(buf)?
                .map(|(length, from)| (length, canonical(from)))),
            Self::Hosted(hosted) => match hosted.link.recv_datagram(buf)? {
                Some(received) => Ok(Some(received)),
                None => Ok(hosted.side.pop(buf)),
            },
            Self::Gone => Ok(None),
        }
    }
}

/// A joined socket seen through its peers router: what the router takes
/// (reach tests, what only a host receives) never reaches the client.
struct ThroughPeers<'a> {
    socket: &'a mut Transport,
    peers: &'a mut Peers,
    now: Duration,
}

impl Datagrams for ThroughPeers<'_> {
    fn send_datagram(&mut self, to: SocketAddr, datagram: &[u8]) -> io::Result<()> {
        self.socket.send_datagram(to, datagram)
    }

    fn recv_datagram(&mut self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        loop {
            let Some((length, from)) = self.socket.recv_datagram(buf)? else {
                return Ok(None);
            };
            if self.peers.route(self.now, from, &buf[..length]) == Route::Client {
                return Ok(Some((length, from)));
            }
        }
    }
}

/// The keepalive thread of a game joined over UDP (slice EF-K): while the
/// game's loop is stalled (a window dragged on Windows, a long frame, a
/// screenshot), it tells the host once a second that the connection is
/// alive, for at most a minute. It starts once the host has accepted the
/// join, learns of every turn of the game's loop, and stops when the
/// connection closes or the session is dropped. The hosting game's own
/// connection, over the in-process link, has none: the host never drops it
/// for silence (EF4).
pub(crate) struct KeptAlive {
    config: KeepaliveConfig,
    thread: Option<Keepalive>,
    /// The thread could not be started; the game carries on without it.
    failed: bool,
    /// Keepalives already noted in the log.
    noted: u64,
    /// The host and the Keepalive packet the thread sends: a migration
    /// (stage K) moves the connection to a new host, and the thread follows.
    speaks_for: Option<(SocketAddr, Vec<u8>)>,
}

impl KeptAlive {
    pub(crate) fn new(config: KeepaliveConfig) -> Self {
        Self {
            config,
            thread: None,
            failed: false,
            noted: 0,
            speaks_for: None,
        }
    }

    /// After a turn of the game's loop, which has just sent what it had.
    pub(crate) fn turned(&mut self, client: &Client, transport: &Transport) {
        match client.phase() {
            tore_session::ClientPhase::Connecting => {}
            tore_session::ClientPhase::Closed => self.thread = None,
            _ if self.thread.is_some() && self.moved(client) => {
                // The connection moved to a new host (stage K): the thread
                // speaks for the new connection, or a stall after the move
                // would leave the new host hearing nothing.
                self.thread = None;
                self.start(client, transport);
            }
            _ => match &self.thread {
                Some(thread) => {
                    thread.turned();
                    let sent = thread.sent();
                    if sent > self.noted {
                        log::info!(
                            "Network: the game was held up; its keepalive kept the connection \
                             ({} keepalives, {} in all)",
                            sent - self.noted,
                            sent
                        );
                        self.noted = sent;
                    }
                }
                None if !self.failed => self.start(client, transport),
                None => {}
            },
        }
    }

    /// Whether the client's connection is no longer the one the thread
    /// speaks for.
    fn moved(&self, client: &Client) -> bool {
        client.keepalive_datagram().is_some_and(|datagram| {
            self.speaks_for
                .as_ref()
                .is_none_or(|(host, sent)| *host != client.server() || *sent != datagram)
        })
    }

    fn start(&mut self, client: &Client, transport: &Transport) {
        // Over the link (the hosting game's own connection, a game that
        // took over included) there is none.
        if matches!(
            transport,
            Transport::Link(_) | Transport::Hosted(_) | Transport::Gone
        ) {
            return;
        }
        let Some(datagram) = client.keepalive_datagram() else {
            return;
        };
        let (host, config) = (client.server(), self.config);
        self.speaks_for = Some((host, datagram.clone()));
        let started = match transport {
            Transport::Udp(socket) => socket
                .try_clone()
                .and_then(|socket| Keepalive::start(socket, host, datagram, config)),
            Transport::Joined(socket) => socket
                .try_clone()
                .and_then(|socket| Keepalive::start(socket, host, datagram, config)),
            // A join through the master: a clone of its socket, and through
            // the relay one that frames for the channel (slice J5, with J3's
            // `RelayFraming`), so a stalled relayed game stays connected
            // (John, 2026-10-05).
            Transport::Internet(t) => t.socket.try_clone().and_then(|socket| {
                if !is_relayed(host) {
                    return Keepalive::start(socket, host, datagram, config);
                }
                match t.joiner.keepalive_socket(socket) {
                    Some(framed) => Keepalive::start(framed, host, datagram, config),
                    None => Err(io::Error::other("the relay's channel is not open")),
                }
            }),
            Transport::Link(_) | Transport::Hosted(_) | Transport::Gone => return,
        };
        match started {
            Ok(thread) => {
                self.thread = Some(thread);
                self.noted = 0;
            }
            Err(error) => {
                self.failed = true;
                log::warn!("Network: no keepalive thread ({error}); a stalled game may be dropped");
            }
        }
    }

    /// Whether the thread runs.
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.thread.is_some()
    }

    /// Keepalives the thread has sent.
    #[cfg(test)]
    pub(crate) fn sent(&self) -> u64 {
        self.thread.as_ref().map_or(0, Keepalive::sent)
    }
}

/// Where and how a session joins: the host's address, the transport that
/// reaches it, and what the player gives.
pub struct Join {
    pub server: SocketAddr,
    pub transport: Transport,
    pub callsign: String,
    /// The plane to ask for; `None` for the first free one.
    pub slot: Option<u32>,
    /// Empty when the host has no password.
    pub password: String,
    /// The host as the capture's file name shows it.
    pub label: String,
    /// A lobby screen drives the session (EF8): the client does not take a
    /// slot or ready by itself, the King's game does not start missions by
    /// itself, and a joiner's End Mission returns it to the lobby instead of
    /// leaving the game. False for `--connect` and `--host`.
    pub lobby: bool,
    /// The rejoin token the game holds for this host's session, sent in the
    /// Challenge answer (stage K, slice K7b); `None` looks one up by the
    /// host's address, then joins as a new player.
    pub token: Option<tore_net::Token>,
}

impl Join {
    /// A join to the server `options` name: the name looked up, a socket
    /// opened.
    pub fn connect(options: &ConnectOptions) -> Result<Self, String> {
        let server = options.resolve()?;
        let mut join = Self::to(server, &options.callsign, &options.password, &options.host)?;
        join.slot = options.slot;
        Ok(join)
    }

    /// A join to an address already known (a game the Direct Connection
    /// screen found, or an address its lookup reached): a socket opened,
    /// nothing looked up. Since stage K the socket is the game's future
    /// host socket: dual-stack, on the game port when it is free (agent
    /// decision: the default game port, 26900, which a game hosts on unless
    /// told otherwise), on any port otherwise.
    pub fn to(
        server: SocketAddr,
        callsign: &str,
        password: &str,
        label: &str,
    ) -> Result<Self, String> {
        Self::to_from(server, callsign, password, label, DEFAULT_PORT)
    }

    /// [`Join::to`] from `port` when it is free (0 for any).
    pub fn to_from(
        server: SocketAddr,
        callsign: &str,
        password: &str,
        label: &str,
        port: u16,
    ) -> Result<Self, String> {
        let socket = ServerSocket::bind(Listen::Any, port)
            .or_else(|_| ServerSocket::bind(Listen::Any, 0))
            .map_err(|error| format!("Cannot open a socket: {error}"))?;
        Ok(Self {
            server,
            transport: Transport::Joined(socket),
            callsign: callsign.to_owned(),
            slot: None,
            password: password.to_owned(),
            label: label.to_owned(),
            lobby: false,
            token: None,
        })
    }
}

/// A joined (or joining) session.
pub struct NetSession {
    pub client: Client,
    /// The last accepted local respawn draft, scoped to the mission.
    pub revival_draft: Option<(
        u32,
        tore_formats::aircraft::AircraftId,
        tore_world::mission::LoadoutSpec,
    )>,
    socket: Transport,
    /// Speaks for the connection while the game's loop is stalled.
    kept: KeptAlive,
    clock: RealClock,
    sink: Sink,
    /// The mission the host last sent, to build the game's copy of it again
    /// for a plane taken a second time in the same running mission.
    spec: Arc<Mutex<Option<MissionSpec>>>,
    resources: Arc<BTreeMap<String, Vec<u8>>>,
    /// Chaff, flares, smoke and contrails, stepped once per client tick.
    pub effects: Effects,
    /// The client tick the effects have been stepped to.
    effects_tick: u64,
    /// Releases waiting for the next effects step.
    releases: Vec<DeviceRelease>,
    /// The gun rounds the host does not send.
    guns: Guns,
    /// The player's trigger, kept as the host will read it.
    fire: guns::Fire,
    /// Whether it was held at the last turn.
    trigger: bool,
    /// Whether the gun key (Tab) was held at the last turn.
    gun_key: bool,
    /// Each weapon record's motor, parsed once.
    motors: RefCell<BTreeMap<String, Option<Motor>>>,
    /// Each weapon record's firing sound and reach, for other
    /// aircraft's launches, which the wire names by record only.
    launches: crate::audio::LaunchSounds,
    /// Session events not yet taken by the game.
    events: Vec<ClientEvent>,
    /// When the player asked to leave.
    pub left_at: Option<Duration>,
    /// The capture file being written, kept out of the pruning.
    pub capture: Option<PathBuf>,
    /// The debrief the host sent, when it has.
    pub debrief: Option<tore_session::wire::messages::Debrief>,
    /// Why the host last ended the mission, until the debrief that follows
    /// it is shown or the player is seated again.
    pub ended: Option<tore_session::wire::messages::EndReason>,
    /// The host this game runs, when it hosts the session; dropping the
    /// session stops it.
    pub hosting: Option<HostThread>,
    /// Why the host this game runs ended without the session ending: its
    /// mission could not be built, or it panicked.
    host_failure: Option<String>,
    /// The lobby state the King's automatic start was last asked for.
    started_for: Option<tore_session::wire::messages::LobbyState>,
    /// A lobby screen drives the session (EF8, [`Join::lobby`]).
    pub lobby_screen: bool,
    /// The lobby's last line in the log.
    lobby_line: Option<String>,
    /// The chat window's lines and the open line (slice EF6).
    pub chat: crate::net::chat::Chat,
    /// What joining again needs: a join through the master joins the relay's
    /// address with a new client when the race finds nothing (slice J5).
    again: Again,
    /// The diagnostics log, shared with the client so the game can add the
    /// path line.
    log: Rc<RefCell<DatedLog>>,
    /// The session clock when the host accepted the join.
    connected_at: Option<Duration>,
    /// Why the game ended a join through the master itself (it gave up, or
    /// the relay was refused or closed): the words the player reads instead
    /// of the client's.
    ending: Option<String>,
    /// Stage K: the peers router in front of a joined socket.
    peers: Peers,
    /// The game's standby, while it is a player that may host.
    standby: Option<GameStandby>,
    /// The mission the client last built, which the standby builds.
    held: HeldSpec,
    /// The player's "Let my game take over hosting" switch (on by default;
    /// the Options panels' check box, kept in `network-v1.conf`).
    may_host: bool,
    /// The game sent its rejoin token (in the Challenge answer, or after the
    /// join in a Rejoin), or has no need to: it is done once for a session.
    rejoin_tried: bool,
    /// The tokens the game's file held when the session started. A Rejoin
    /// by session reads this and not the file: the token the host grants a
    /// newly joined player can reach the client with the Accepted, before
    /// the game reads it, and replaces the session's old entry in the file
    /// (slice B8).
    earlier: Store,
    /// The standby could not be started; the game carries on without one.
    standby_failed: bool,
    /// The game hosts and a standby is ready to take the game over (the
    /// hosting thread's report): leaving hands the game over.
    standby_ready: bool,
    /// The client has been told its candidates for this socket.
    candidate_set: bool,
    /// The data folder, for the remembered port-mapping setting.
    data: PathBuf,
    /// A join through the master's facts, kept after a takeover took its
    /// socket.
    through_last: Option<ThroughFacts>,
}

/// What a session keeps to start its client again (slice J5).
struct Again {
    /// The client's settings as the session started it.
    config: ClientConfig,
    replays: Option<Library>,
    label: String,
    /// The data folder and the host's address as the game joined it (empty
    /// for a game this game hosts), where the client's rejoin token is kept
    /// (stage K, slice K7b).
    data: PathBuf,
    address: String,
}

/// Writes into the diagnostics log the client holds, so the game can add a
/// line of its own.
struct SharedLog(Rc<RefCell<DatedLog>>);

impl io::Write for SharedLog {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

/// A path as the net log and the game's log say it (as `tore-bot` says it).
pub fn path_words(path: JoinedPath) -> &'static str {
    match path {
        JoinedPath::LocalNetwork => "local network",
        JoinedPath::ByAddress => "by address",
        JoinedPath::MappedPort => "mapped port",
        JoinedPath::Ipv6 => "IPv6",
        JoinedPath::Punched => "punched",
        JoinedPath::Relay => "relay",
    }
}

/// Gives a newly started `client` its capture (a new file beside the
/// replays, when there are replays), the diagnostics log and the game's
/// mission builder. The capture's path, when one was made.
fn equip(
    client: &mut Client,
    again: &Again,
    log: &Rc<RefCell<DatedLog>>,
    sink: &Sink,
    spec: &Arc<Mutex<Option<MissionSpec>>>,
    held: &HeldSpec,
    resources: &Arc<BTreeMap<String, Vec<u8>>>,
) -> Option<PathBuf> {
    let mut capture = None;
    if let Some(library) = &again.replays {
        match files::create_capture(library, SystemTime::now(), &again.label) {
            Ok((path, file)) => {
                client.set_capture(Box::new(BufWriter::new(file)));
                capture = Some(path);
            }
            Err(error) => log::warn!("Network capture not written: {error}"),
        }
    }
    client.set_diagnostics(Box::new(SharedLog(Rc::clone(log))));
    // The token the host grants goes to the game's file (stage K).
    client.set_token_store(Box::new(GameStore::new(&again.data, &again.address)));
    let (map, kept, spec_for_rebuild) = (Arc::clone(resources), Arc::clone(sink), Arc::clone(spec));
    let held = Arc::clone(held);
    client.set_background_mission_builder(Box::new(move |spec, reads: &dyn ResourceSource| {
        *spec_for_rebuild.lock().unwrap_or_else(|e| e.into_inner()) = Some(spec.clone());
        // The standby builds the flight the player holds (stage K).
        *held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(spec.clone());
        // The game's own build for the screens, then the session's: a
        // failure of either is the mission's.
        *kept.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(build_mission(spec, &map).map_err(|e| e.to_string()));
        // Read every resource the plain build reads, through `reads`, so
        // the manifest the host compares is the simulation's own.
        World::new(spec, reads, Seating::Open)
    }));
    capture
}

/// How long after Leave the game waits for the debrief and the disconnect
/// before it quits the connection itself.
pub const LEAVE_GRACE: Duration = Duration::from_secs(8);

impl NetSession {
    /// The import the session's mission is built from, which its capture
    /// converts with.
    pub fn game_label(&self) -> &str {
        &self.again.label
    }

    pub fn resources(&self) -> Arc<BTreeMap<String, Vec<u8>>> {
        Arc::clone(&self.resources)
    }

    /// The launch sound and reach of each weapon record.
    pub fn launch_sounds(&self) -> &crate::audio::LaunchSounds {
        &self.launches
    }

    /// Starts the join over `join`'s transport. The mission is built when
    /// the host sends it.
    pub fn start(
        join: Join,
        resources: Arc<BTreeMap<String, Vec<u8>>>,
        data: &Path,
        replays: Option<&Library>,
    ) -> Result<Self, String> {
        let Join {
            server,
            transport: mut socket,
            callsign,
            slot,
            password,
            label,
            lobby,
            token,
        } = join;
        // The host's address as the game keeps it with the token; a game this
        // game hosts has none.
        let address = match &socket {
            Transport::Link(_) => String::new(),
            _ => server.to_string(),
        };
        // The tokens the game holds now: the one kept for this address, and
        // later the one kept for the host's session (a game the list found
        // has its own, by session, from the screen).
        let earlier = if address.is_empty() {
            Store::default()
        } else {
            Store::load(data, tore_session::client::rejoin::unix_now())
        };
        let token = token.or_else(|| earlier.for_address(&address).map(|entry| entry.kept.token));
        let clock = RealClock::new();
        let now = clock.now();
        // A join through the master races the host's addresses.
        let race = match &mut socket {
            Transport::Internet(t) => {
                t.begin(now);
                t.race.take()
            }
            _ => None,
        };
        let config = ClientConfig {
            race,
            password,
            plane: slot,
            entropy: Entropy::System,
            retail_stall_speeds: tore_sim::flight::retail_stall_speeds(),
            auto_ready: !lobby,
            // Stage L: the content the screens' worker worked out.
            content: Some(game_content(&resources)),
            // Stage K: the player comes back as itself.
            token,
            ..ClientConfig::new(server, &callsign, build_id())
        };
        let mut client = Client::connect(config.clone(), Arc::clone(&resources), now)
            .map_err(|error| error.to_string())?;

        // The files: the log, and a capture beside the replays, pruned by the
        // replays' own rule before a new one is made.
        if let Some(library) = replays {
            let settings = library.settings();
            let cleanup = files::prune(library, data, &settings, SystemTime::now(), &[]);
            for (path, error) in &cleanup.failed {
                log::warn!(
                    "Network files: could not remove {}: {error}",
                    path.display()
                );
            }
        }
        let again = Again {
            config,
            replays: replays.cloned(),
            label,
            data: data.to_owned(),
            address,
        };
        let log = Rc::new(RefCell::new(DatedLog::new(data.join(files::LOG_FOLDER))));
        let sink: Sink = Arc::new(Mutex::new(None));
        let spec_kept: Arc<Mutex<Option<MissionSpec>>> = Arc::new(Mutex::new(None));
        let held: HeldSpec = Arc::new(Mutex::new(None));
        let capture = equip(
            &mut client,
            &again,
            &log,
            &sink,
            &spec_kept,
            &held,
            &resources,
        );
        let launches = crate::audio::LaunchSounds::of(&resources);
        Ok(Self {
            client,
            revival_draft: None,
            socket,
            kept: KeptAlive::new(KeepaliveConfig::default()),
            clock,
            sink,
            spec: spec_kept,
            resources,
            effects: Effects::default(),
            effects_tick: 0,
            releases: Vec::new(),
            guns: Guns::default(),
            fire: guns::Fire::default(),
            trigger: false,
            gun_key: false,
            motors: RefCell::new(BTreeMap::new()),
            launches,
            events: Vec::new(),
            left_at: None,
            capture,
            debrief: None,
            ended: None,
            hosting: None,
            host_failure: None,
            started_for: None,
            lobby_screen: lobby,
            lobby_line: None,
            chat: Default::default(),
            again,
            log,
            connected_at: None,
            ending: None,
            peers: Peers::new(tore_session::wire::PROTOCOL_VERSION, Entropy::System),
            standby: None,
            held,
            // "Let my game take over hosting", as the player left it on the
            // Options panels (on by default).
            may_host: crate::net::settings::Remembered::load(data).may_host,
            rejoin_tried: token.is_some(),
            earlier,
            standby_failed: false,
            standby_ready: false,
            candidate_set: false,
            data: data.to_owned(),
            through_last: None,
        })
    }

    /// Whether this game currently runs the host, independently of the crown.
    pub fn hosting(&self) -> bool {
        self.hosting.is_some()
    }

    /// Whether leaving now ends the game for everyone (slice F2-O4): this
    /// game hosts it and no standby is ready to take it over. A game that
    /// only joined one (a dedicated server's, a friend's) just leaves.
    pub fn leaving_ends_game(&self) -> bool {
        self.hosting() && !self.standby_ready
    }

    /// Leaving on purpose as the hosting player with a ready standby: the
    /// hosting thread is stopped first, so it hands the game over (a stop
    /// with a ready standby does) before this game's own connection says
    /// goodbye, which would end the game. A game that does not host has
    /// nothing to hand over.
    pub fn hand_over_for_leave(&mut self) {
        if self.standby_ready
            && let Some(thread) = &mut self.hosting
        {
            thread.stop(hosting::JOIN_LIMIT);
        }
    }

    /// A test's say that a standby is ready.
    #[cfg(test)]
    pub fn set_standby_ready_for_test(&mut self, ready: bool) {
        self.standby_ready = ready;
    }

    /// The King's start for a game hosted from the command line (agent
    /// decision): as soon as every player holding a slot is ready, when
    /// `allowed` (the hosting player is not reading a debrief), once for each
    /// lobby state. A game with a lobby screen never starts by itself: its
    /// King presses Fly.
    pub fn auto_start(&mut self, allowed: bool) {
        if !allowed || self.hosting.is_none() || self.lobby_screen {
            return;
        }
        let Some(lobby) = self.client.lobby() else {
            return;
        };
        if lobby.is_king()
            && lobby.phase == tore_session::wire::messages::LobbyPhase::Lobby
            && lobby.all_ready()
            && self.started_for.as_ref() != Some(lobby)
        {
            self.started_for = Some(lobby.clone());
            log::info!("Network: starting the mission; every player holding a slot is ready");
            self.client.start_mission();
        }
    }

    /// The lobby in one line for the log, when it changed since the last.
    pub fn lobby_change(&mut self) -> Option<String> {
        let lobby = self.client.lobby()?;
        let players: Vec<String> = lobby
            .players
            .iter()
            .map(|p| {
                let mut text = p.callsign.clone();
                if lobby.king == Some(p.id) {
                    text.push_str(" (King)");
                }
                match p.slot {
                    Some(plane) => text.push_str(&format!(" plane {plane}")),
                    None => text.push_str(" no slot"),
                }
                if p.loadout {
                    text.push_str(" armed");
                }
                if p.flying {
                    text.push_str(" flying");
                } else if p.ready {
                    text.push_str(" ready");
                }
                if let Some(why) = &p.unable {
                    text.push_str(&format!(" unable ({why})"));
                }
                text
            })
            .collect();
        let line = format!(
            "{:?}, mission {} ({}): {}",
            lobby.phase,
            lobby.mission,
            lobby.summary,
            players.join("; ")
        );
        if self.lobby_line.as_ref() == Some(&line) {
            return None;
        }
        self.lobby_line = Some(line.clone());
        Some(line)
    }

    /// Sends what the client has queued (a Disconnect, say) without turning
    /// the session over.
    pub fn flush(&mut self) {
        let _ = self.client.transmit(&mut self.socket);
        if let Transport::Internet(t) = &mut self.socket {
            // A connection that ended closes its relay channel, after its
            // last datagrams went through it.
            if self.client.phase() == ClientPhase::Closed {
                t.joiner.close_relay();
            }
            let _ = t.joiner.transmit(&mut t.socket);
        }
    }

    /// The words for why the session ended: the game's own when it ended a
    /// join through the master itself, a plainer one for a join through the
    /// master that no address answered, else the client's.
    pub fn close_text(&self, reason: &CloseReason) -> String {
        if let Some(text) = &self.ending {
            return text.clone();
        }
        if let (Transport::Internet(t), CloseReason::NoAnswer) = (&self.socket, reason) {
            return t.no_answer_text(&self.again.label);
        }
        self.client.close_text(reason)
    }

    /// What a join through the Internet Lobby tells the player's Report;
    /// `None` for any other join.
    pub fn through_facts(&self) -> Option<ThroughFacts> {
        let mut facts = match &self.socket {
            Transport::Internet(t) => t.facts(),
            _ => self.through_last?,
        };
        let counts = self.client.migration_counts();
        facts.migrations = u8::try_from(counts.resumed).unwrap_or(u8::MAX);
        facts.failed_migrations = u8::try_from(counts.failed).unwrap_or(u8::MAX);
        Some(facts)
    }

    /// From the session's start to the host accepting the join, once it has.
    pub fn connected_after(&self) -> Option<Duration> {
        self.connected_at
    }

    /// The join through the master's turn, after the client's (slice J5):
    /// the joiner's timers and the master's answers; the relay asked for
    /// when the race finds nothing in time, the client started again on the
    /// relay's address when its channel opens, the relay closed when it is
    /// not wanted or the connection ended; the whole join given up after
    /// [`JOIN_GIVE_UP`]. Each step a player should read is a
    /// [`ClientEvent::Notice`] (agent decision: the session's own notices
    /// reach Messages the way the host's do).
    fn through(&mut self, now: Duration) {
        let Transport::Internet(t) = &mut self.socket else {
            return;
        };
        let label = &self.again.label;
        let phase = self.client.phase();
        let joining = phase == ClientPhase::Connecting;
        let mut notices = Vec::new();
        let mut ending = None;
        let mut relayed = None;
        t.joiner.update(now);
        while let Some(event) = t.joiner.poll_event() {
            match event {
                JoinEvent::Relayed { address } => {
                    if joining && !self.client.chosen() {
                        relayed = Some(address);
                    } else {
                        log::info!(
                            "Network: the relay opened after {label} answered directly; closing it"
                        );
                        t.joiner.close_relay();
                    }
                }
                JoinEvent::RelayRefused { text, .. } => {
                    log::info!("Network: the relay was refused: {text}");
                    if !joining {
                        continue;
                    }
                    if t.path == JoinPath::Relay {
                        ending = Some(text);
                    } else {
                        notices.push(text);
                    }
                }
                JoinEvent::RelaySilent => {
                    let text = "The Internet Lobby did not answer the relay request.".to_owned();
                    if !joining {
                        continue;
                    }
                    if t.path == JoinPath::Relay {
                        ending = Some(text);
                    } else {
                        notices.push(text);
                    }
                }
                JoinEvent::RelayClosed(reason) => {
                    let text = tore_net::master::relay::close_text(reason).to_owned();
                    log::info!("Network: {text}");
                    // The game connection through the relay is lost with it.
                    if is_relayed(self.client.server()) && phase != ClientPhase::Closed {
                        ending = Some(text);
                    }
                }
                // The introduction's events came before the session.
                other => log::info!("Network: the Internet Lobby said {other:?} after the join"),
            }
        }
        if ending.is_none()
            && joining
            && let Some(why) = t.relay_reason(now, self.client.chosen())
        {
            t.relay_asked = true;
            if t.joiner.ask_relay(now) {
                notices.push(why.to_owned());
            } else {
                log::info!("Network: the relay could not be asked for");
            }
        }
        if ending.is_none() && joining && now >= t.give_up {
            ending = Some(t.no_answer_text(label));
        }
        if phase == ClientPhase::Closed && t.joiner.relay_state() != RelayState::Ended {
            t.joiner.close_relay();
        }
        let _ = t.joiner.transmit(&mut t.socket);
        self.events
            .extend(notices.into_iter().map(ClientEvent::Notice));
        if let Some(text) = ending {
            self.ending = Some(text);
            self.client.disconnect(now);
            let _ = self.client.transmit(&mut self.socket);
        } else if let Some(address) = relayed {
            self.join_relayed(address, now);
        }
    }

    /// The race found nothing and the relay's channel is open: the same
    /// join, to the channel's relayed address, with a new client (as
    /// `tore-bot` does). The race's capture, which holds no game, is
    /// removed and a new one started (agent decision).
    fn join_relayed(&mut self, address: SocketAddr, now: Duration) {
        let config = ClientConfig {
            server: address,
            race: None,
            ..self.again.config.clone()
        };
        let client = match Client::connect(config, Arc::clone(&self.resources), now) {
            Ok(client) => client,
            Err(error) => {
                self.ending = Some(error.to_string());
                self.client.disconnect(now);
                return;
            }
        };
        drop(std::mem::replace(&mut self.client, client));
        if let Some(raced) = self.capture.take()
            && let Err(error) = std::fs::remove_file(&raced)
        {
            log::warn!(
                "Network files: could not remove the race's capture {}: {error}",
                raced.display()
            );
        }
        self.capture = equip(
            &mut self.client,
            &self.again,
            &self.log,
            &self.sink,
            &self.spec,
            &self.held,
            &self.resources,
        );
        self.candidate_set = false;
        log::info!(
            "Network: the relay is open; joining {} through it at {address}",
            self.again.label
        );
        self.events.push(ClientEvent::Notice(
            "The relay is open; joining through it...".to_owned(),
        ));
        let _ = self.client.transmit(&mut self.socket);
    }

    /// The host accepted the join at `now`: the path it took goes to the net
    /// log and the game's log, and, for a join through the Internet Lobby,
    /// to Messages ("Connected through the relay.").
    fn connected(&mut self, now: Duration) {
        self.connected_at = Some(now);
        if matches!(self.socket, Transport::Link(_) | Transport::Hosted(_)) {
            return;
        }
        let path = self.client.path();
        let server = self.client.server();
        let words = path_words(path);
        let line = format!("{:.3}\tpath\t{words}\t{server}\n", now.as_secs_f64());
        let _ = io::Write::write_all(&mut *self.log.borrow_mut(), line.as_bytes());
        log::info!("Network: joined {server}, path {words}");
        if matches!(self.socket, Transport::Internet(_)) {
            self.events.push(ClientEvent::Notice(
                crate::net::telemetry::path_line(path).to_owned(),
            ));
        }
    }

    /// The host accepted the join, and the game holds a token for the
    /// session it names that the join did not send (a game joined at an
    /// address the token was not kept for): the Rejoin message brings the
    /// player back as itself (stage K, slice K7b). Once for a session.
    fn rejoin_known(&mut self, session_id: u64) {
        if std::mem::replace(&mut self.rejoin_tried, true) {
            return;
        }
        if let Some(entry) = self.earlier.for_session(session_id) {
            log::info!("Network: rejoining session {session_id:016x} with the token kept for it");
            self.net_line("rejoin", &format!("{session_id:016x}"));
            self.client.rejoin(entry.kept.token);
        }
    }

    /// The session clock now.
    pub fn now(&self) -> Duration {
        self.clock.now()
    }

    /// Receives, runs what is due with `controls`, and sends. The session's
    /// events collect for [`NetSession::take_events`].
    pub fn pump(&mut self, controls: &Controls) {
        let now = self.clock.now();
        self.follow_hosting(now);
        if let Transport::Internet(t) = &mut self.socket {
            t.now = now;
            self.through_last = Some(t.facts());
        }
        self.trigger = self.fire.turn(&controls.commands, controls.trigger);
        self.gun_key = self.fire.gun_key();
        if self.routed() {
            let mut over = ThroughPeers {
                socket: &mut self.socket,
                peers: &mut self.peers,
                now,
            };
            let _ = self.client.receive_from(now, &mut over);
        } else {
            let _ = self.client.receive_from(now, &mut self.socket);
        }
        self.client.update(now, controls);
        self.migrate(now);
        let _ = self.client.transmit(&mut self.socket);
        // A join through the master: the race, the relay, the give-up. It
        // may start the client again, before its events are read.
        self.through(now);
        self.kept.turned(&self.client, &self.socket);
        while let Some(event) = self.client.poll_event() {
            let connected = matches!(event, ClientEvent::Connected { .. });
            if let ClientEvent::Debrief(debrief) = &event {
                self.debrief = Some((**debrief).clone());
            }
            let session = match &event {
                ClientEvent::Connected { session_id, .. } => Some(*session_id),
                _ => None,
            };
            self.events.push(event);
            if connected {
                self.connected(now);
            }
            if let Some(session_id) = session {
                self.rejoin_known(session_id);
            }
        }
        // A player who left waits only so long for the host to answer.
        if self
            .left_at
            .is_some_and(|at| now.saturating_sub(at) > LEAVE_GRACE)
        {
            self.client.disconnect(now);
        }
    }

    /// The player's "Let my game take over hosting" switch (stage K; on by
    /// default, read from `network-v1.conf` when the session starts). Off,
    /// the game tells the host it may not host and runs no standby.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_may_host(&mut self, on: bool) {
        if self.may_host != on {
            self.may_host = on;
            self.candidate_set = false;
            if !on {
                self.standby = None;
            }
        }
    }

    /// Whether the game runs a standby now (stage K).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn standing_by(&self) -> bool {
        self.standby.is_some()
    }

    /// Whether the transport is a joined socket, read through the peers
    /// router.
    fn routed(&self) -> bool {
        matches!(
            self.socket,
            Transport::Udp(_) | Transport::Joined(_) | Transport::Internet(_)
        )
    }

    /// Whether this game could take the game over: a socket that can host,
    /// not reached through the relay (a relayed game is never a host, John,
    /// 2026-09-28), and the player's switch on.
    fn may_take_over(&self) -> bool {
        self.may_host
            && match &self.socket {
                Transport::Joined(_) => true,
                Transport::Internet(_) => !is_relayed(self.client.server()),
                _ => false,
            }
    }

    /// A line of the net log, as the client's own lines are.
    fn net_line(&self, kind: &str, text: &str) {
        let line = format!("{:.3}\t{kind}\t{text}\n", self.now().as_secs_f64());
        let _ = io::Write::write_all(&mut *self.log.borrow_mut(), line.as_bytes());
    }

    /// The hosting thread's reports (stage K added the takeover and the
    /// thread's ends that leave the game in the session): a failure for the
    /// player; the takeover's tick, where the game's own player resumes; the
    /// socket back after a step-down or a failed takeover.
    fn follow_hosting(&mut self, now: Duration) {
        let Some(hosting) = &mut self.hosting else {
            return;
        };
        let mut ended = None;
        let mut took = None;
        let mut lines = Vec::new();
        let mut said = Vec::new();
        for report in hosting.poll() {
            match report {
                Report::Ended(end) => {
                    if let Some(text) = end.failure() {
                        self.host_failure.get_or_insert(text);
                    }
                    ended = Some(end);
                }
                Report::TookOver { tick } => took = Some(tick),
                Report::Migration(line) => lines.push(line),
                Report::Said(line) => said.push(line),
                Report::Standby { ready } => self.standby_ready = ready,
                _ => {}
            }
        }
        for line in lines {
            self.net_line("host", &line);
        }
        // What the host says about the migration, for the player's Messages.
        self.events
            .extend(said.into_iter().map(ClientEvent::Notice));
        if let Some(tick) = took {
            self.client.host_here(now, LINK_ADDRESS, tick);
            self.net_line("migrate", &format!("took-over\t{tick}"));
        }
        match ended {
            Some(End::SteppedDown(to)) => {
                log::info!("Network: another game hosts now, at {to}; resuming with it");
                self.socket_back(now, Some(to));
            }
            Some(End::MovedOn(to)) => {
                log::info!("Network: the game moved to a better machine; resuming with it");
                self.socket_back(now, None);
                if !to.is_empty() {
                    self.client.move_to(now, &to);
                }
            }
            Some(End::TakeoverFailed(why)) => {
                log::warn!("Network: this game could not take the game over ({why})");
                self.socket_back(now, None);
            }
            _ => {}
        }
    }

    /// The hosting thread ended without ending the game: its socket is the
    /// game's again, behind a new peers router, and the client resumes with
    /// the game's new host at `to` (a step-down) or races on (a failed
    /// takeover).
    fn socket_back(&mut self, now: Duration, to: Option<SocketAddr>) {
        let Some(thread) = self.hosting.take() else {
            return;
        };
        let socket = thread.take_socket();
        drop(thread);
        match socket {
            Some(socket) => {
                self.socket = Transport::Joined(socket);
                self.peers = Peers::new(tore_session::wire::PROTOCOL_VERSION, Entropy::System);
                self.candidate_set = false;
                self.kept = KeptAlive::new(KeepaliveConfig::default());
            }
            None => {
                log::warn!("Network: the hosting thread kept its socket; the game cannot rejoin")
            }
        }
        if let Some(to) = to {
            self.client.move_to(now, &[to]);
        }
    }

    /// Stage K's turn after the client's update: the standby, the
    /// takeover, the old host's address for the hosting thread's router,
    /// the candidates and the peers router's work.
    fn migrate(&mut self, now: Duration) {
        self.ensure_standby();
        if let Some(standby) = &mut self.standby {
            let notes = standby.turn(now, &mut self.client);
            for note in notes {
                let line = note_line(&note);
                log::info!("Network: standby: {line}");
                self.net_line("standby", &line);
            }
        }
        self.maybe_take_over(now);
        if let Transport::Hosted(hosted) = &self.socket {
            hosted.side.set_old_host(self.client.old_host());
        }
        if self.routed() {
            self.candidate();
            self.client.drive_peers(now, &mut self.peers);
            self.peers.update(now);
            let _ = self.peers.transmit(&mut self.socket);
        }
    }

    /// Starts the standby once the game is in the lobby and may take over.
    fn ensure_standby(&mut self) {
        if self.standby.is_some()
            || self.standby_failed
            || self.hosting.is_some()
            || matches!(
                self.client.phase(),
                ClientPhase::Connecting | ClientPhase::Closed
            )
            || self.client.lobby().is_none()
            || !self.may_take_over()
        {
            return;
        }
        match GameStandby::start(Arc::clone(&self.held), Arc::clone(&self.resources)) {
            Ok(standby) => self.standby = Some(standby),
            Err(error) => {
                self.standby_failed = true;
                log::warn!("Network: no standby thread ({error}); this game cannot take over");
            }
        }
    }

    /// Tells the client, once for each socket, what it reports to the host
    /// as a candidate: the switch, the socket's own addresses toward the
    /// host (Local IPv4, Global IPv6), the router's mapping type when a join
    /// through the master tested it, and the CPU measure.
    fn candidate(&mut self) {
        if self.candidate_set
            || matches!(
                self.client.phase(),
                ClientPhase::Connecting | ClientPhase::Closed
            )
        {
            return;
        }
        self.candidate_set = true;
        let (port, mapping) = match &self.socket {
            Transport::Joined(socket) => (port_of(socket), MappingType::Unknown),
            Transport::Internet(t) => (port_of(&t.socket), t.joiner.mapping()),
            _ => (0, MappingType::Unknown),
        };
        let able = self.may_take_over() && port != 0;
        let candidates = if able {
            host_candidates(&[self.client.server()], port, own_address_toward)
        } else {
            Vec::new()
        };
        self.client.set_candidate(CandidateSettings {
            may_host: able,
            candidates,
            mapping,
            measure_cpu: able,
        });
    }

    /// Takes the game over when the client says the standby is due to
    /// (slice K4's rule): the socket goes to a new hosting thread, which
    /// builds the host from the standby; the client joins it over the link
    /// once it hosts ([`Report::TookOver`]).
    fn maybe_take_over(&mut self, now: Duration) {
        let Some(standby) = &self.standby else {
            return;
        };
        if self.hosting.is_some()
            || !self
                .client
                .takeover_due(standby.ready(), standby.handed_over())
        {
            return;
        }
        let Some(house) = self.client.lobby().map(|lobby| lobby.you) else {
            return;
        };
        let socket = match std::mem::replace(&mut self.socket, Transport::Gone) {
            Transport::Joined(socket) => socket,
            Transport::Internet(t) => {
                // A game that may host is not relayed: its joiner holds no
                // channel, and the new host's listing is the old host's.
                let MasterTransport { socket, .. } = *t;
                socket
            }
            other => {
                self.socket = other;
                return;
            }
        };
        let sends = match socket.try_clone() {
            Ok(sends) => sends,
            Err(error) => {
                log::warn!("Network: cannot take the game over ({error}); joining whoever does");
                self.socket = Transport::Joined(socket);
                self.standby = None;
                self.standby_failed = true;
                return;
            }
        };
        let standby = self.standby.take().expect("a standby");
        let resumption = Resumption {
            house,
            now,
            present: self.client.present(now),
        };
        let forward = hosting::forward::choose(
            crate::net::settings::Remembered::load(&self.data).port_forward,
        );
        log::info!(
            "Network: the host is lost or handed over; this game takes the game over on UDP {}",
            port_of(&socket)
        );
        self.net_line("migrate", "taking-over");
        match HostThread::take_over(TakeoverSetup {
            standby: standby.into_thread(),
            resources: Arc::clone(&self.resources),
            config: hosting::takeover_config(&self.resources),
            resumption,
            socket,
            clock: self.clock,
            forward,
        }) {
            Ok((thread, link, side)) => {
                self.socket = Transport::Hosted(Box::new(Hosted {
                    link,
                    socket: sends,
                    side,
                }));
                self.hosting = Some(thread);
                // The game hosts: no keepalive speaks for it any more.
                self.kept = KeptAlive::new(KeepaliveConfig::default());
            }
            Err(error) => {
                log::warn!("Network: cannot take the game over ({error}); joining whoever does");
                self.socket = Transport::Joined(sends);
                self.standby_failed = true;
            }
        }
    }

    /// How long until the session next needs a turn, at most 10 ms.
    pub fn next_wake(&self) -> Duration {
        self.client.next_wake(self.clock.now())
    }

    /// The plain message for a host this game runs that failed (its
    /// mission could not be built, or it panicked), once.
    pub fn take_host_failure(&mut self) -> Option<String> {
        self.host_failure.take()
    }

    /// The session events since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<ClientEvent> {
        std::mem::take(&mut self.events)
    }

    /// The game's build of the mission, once the host has sent it. `Some`
    /// once; an `Err` is the build's failure.
    pub fn take_built(&mut self) -> Option<WorldResult<Built>> {
        self.sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .map(|result| result.map_err(Into::into))
    }

    /// The game's build of the running mission again, for a player who left
    /// its flight and takes a plane in the same mission once more (the
    /// first seating took the build [`NetSession::take_built`] gave).
    /// `None` until the host has sent a mission.
    pub fn rebuild_background(
        &self,
    ) -> Option<Result<crate::loading::Job<crate::loading::Prepared>, String>> {
        let spec = self
            .spec
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        let resources = Arc::clone(&self.resources);
        let progress = crate::loading::Progress::mission();
        Some(crate::loading::Job::continue_load(
            Some(progress),
            false,
            move |progress| {
                progress.stage(crate::loading::Stage::Mission);
                build_mission(&spec, &resources)
                    .map(|built| crate::loading::Prepared::NetRebuilt(Box::new(built)))
                    .map_err(|e| e.to_string())
            },
        ))
    }
    #[cfg(test)]
    pub fn rebuild(&mut self) -> Option<WorldResult<Built>> {
        let spec = self
            .spec
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()?;
        Some(build_mission(&spec, &self.resources))
    }

    /// The shared polite disconnect used by Leave, Return to Main Menu and Exit.
    pub fn disconnect(&mut self) {
        self.hand_over_for_leave();
        let now = self.now();
        self.client.disconnect(now);
        self.flush();
    }

    /// End Mission in flight. The hosting player, the King, ends the
    /// mission for everyone, who return to the lobby with their debriefs
    /// (and the King's game starts the next once the debrief is closed);
    /// any other player leaves the game, with its debrief, since a game with
    /// no lobby screen has nowhere else to go (agent decisions, EF4). With a
    /// lobby screen (EF8) any other player returns to the lobby, its debrief
    /// shown, while the others fly on.
    pub fn leave(&mut self) {
        if self.hosting.is_some() {
            self.client.end_mission();
            return;
        }
        if self.lobby_screen {
            // Back to the lobby while the others fly on (EF8).
            let now = self.clock.now();
            self.client.leave(now);
            return;
        }
        if self.left_at.is_none() {
            let now = self.clock.now();
            self.left_at = Some(now);
            self.client.leave_game(now);
        }
    }

    /// The client's frame for this render, with the countermeasure releases
    /// its events brought (kept for [`NetSession::step_effects`]). `None`
    /// until the player has a plane.
    pub fn frame(&mut self) -> Option<ClientFrame> {
        let frame = self.client.frame(self.clock.now())?;
        for received in &frame.events {
            if let WireEvent::Countermeasure {
                aircraft,
                flare,
                position,
                velocity,
                attitude,
                number,
                ..
            } = &received.event
            {
                self.releases.push(release_of(
                    *aircraft,
                    *flare,
                    position,
                    velocity,
                    attitude,
                    *number,
                    received.tick,
                ));
            }
        }
        Some(frame)
    }

    /// Adds the gun rounds the host does not send to `frame`'s picture: the
    /// player's own from its trigger and its predicted aircraft, and other
    /// aircraft's from the host's gun burst events.
    pub fn step_guns(&mut self, frame: &mut ClientFrame, around: &GunContext<'_>) {
        let ground = |x: f64, z: f64| f64::from(around.terrain.height(x as f32, z as f32));
        let stations = |id: AircraftId| -> &[tore_sim::combat::live::Station] {
            around
                .configurations
                .iter()
                .find(|config| config.aircraft == id)
                .map_or(&[][..], |config| config.stations.as_slice())
        };
        let inputs = guns::Inputs {
            tick: frame.tick,
            render_tick: frame.render_tick,
            trigger: self.trigger,
            gun_key: self.gun_key,
            plane: frame.plane.0,
            own: crate::combat::launcher(&frame.presented),
            gun_aim: frame.presented.gun_aim,
            stores: frame
                .readout
                .as_ref()
                .map(|readout| guns::Stores::of(readout, &frame.config)),
            config: &frame.config,
            events: &frame.events,
        };
        let mut picture = std::mem::take(&mut frame.picture);
        self.guns.step(
            &inputs,
            &guns::Around {
                ground: &ground,
                stations: &stations,
                surface: Some(&around.terrain.surface),
            },
            &mut picture,
        );
        frame.picture = picture;
    }

    /// What the surface units' weapons hold at host tick `tick`, as far as
    /// a client knows: the targets of their recent gun bursts. The host
    /// sends no lock or missile target of a surface unit, so a launcher's
    /// picture does not aim on a client (docs/spec/surface-defenses.md,
    /// "Aiming").
    pub fn surface_aim_tracks(&self, tick: u64) -> Vec<tore_world::surface::aim::Track> {
        tore_world::surface::aim::burst_tracks(tick, self.guns.surface_sights())
    }

    /// Steps the regenerated effects to client tick `to`, one step per tick
    /// since the last call (at most [`MAX_EFFECT_TICKS`]), each over
    /// `picture`, the newest the screen has; the releases the frames brought
    /// go with the first step.
    /// Marks the rounds of `picture` whose motor burns now (bug B7), by the
    /// regenerated effects' dating; call after [`Self::step_effects`].
    pub fn mark_powered(
        &self,
        picture: &mut RenderSnapshot,
        resources: &BTreeMap<String, Vec<u8>>,
    ) {
        self.effects
            .mark_powered(picture, &|name| motor_of(&self.motors, name, resources));
    }

    pub fn step_effects(&mut self, to: u64, picture: &RenderSnapshot, around: &EffectContext<'_>) {
        let behind = to.saturating_sub(self.effects_tick);
        // A long gap (a stall, or the first frame) is not caught up on.
        let steps = behind.clamp(1, MAX_EFFECT_TICKS);
        self.effects_tick = to;
        let outlets = |id: AircraftId| -> &[Vector] {
            around
                .models
                .iter()
                .find(|model| model.profile.id == id)
                .map_or(&[][..], |model| &model.kind.contrail_offsets)
        };
        let motors = &self.motors;
        let motor = |name: &str| motor_of(motors, name, around.resources);
        let surroundings = regen::Surroundings {
            terrain: around.terrain,
            outlets: &outlets,
            motor: &motor,
            sortie: around.sortie,
        };
        for step in 0..steps {
            let releases = if step == 0 {
                std::mem::take(&mut self.releases)
            } else {
                Vec::new()
            };
            self.effects.step(picture, &surroundings, &releases);
        }
    }
}

/// The motor of weapon record `name`, read once from `resources` into
/// `motors`.
fn motor_of(
    motors: &RefCell<BTreeMap<String, Option<Motor>>>,
    name: &str,
    resources: &BTreeMap<String, Vec<u8>>,
) -> Option<Motor> {
    motors
        .borrow_mut()
        .entry(name.to_owned())
        .or_insert_with(|| {
            let bytes = resources.get(name)?;
            Motor::of(&tore_formats::weapons::Weapon::parse(name, bytes).ok()?)
        })
        .to_owned()
}

/// The port a socket is bound on, 0 when it cannot say.
fn port_of(socket: &ServerSocket) -> u16 {
    socket.local_addresses().first().map_or(0, SocketAddr::port)
}

/// What the gun rounds need that the frame does not say.
pub struct GunContext<'a> {
    pub terrain: &'a crate::terrain::Terrain,
    /// The mission's usual loadout of each aircraft type.
    pub configurations: &'a [tore_sim::combat::live::Configuration],
}

/// What the regenerated effects need that the picture does not say.
pub struct EffectContext<'a> {
    pub terrain: &'a crate::terrain::Terrain,
    /// The drawn model of every aircraft type of the mission, the player's
    /// included: their engine outlets place the contrails.
    pub models: &'a [&'a Airframe],
    pub resources: &'a BTreeMap<String, Vec<u8>>,
    pub sortie: u64,
}

/// The most effect steps one frame takes, so a stall cannot become a long
/// burst of work.
const MAX_EFFECT_TICKS: u64 = 24;

/// A wire countermeasure release as the effects fly it.
fn release_of(
    aircraft: u32,
    flare: bool,
    position: &[i64; 3],
    velocity: &[i64; 3],
    attitude: &[u16; 3],
    number: u64,
    tick: u32,
) -> DeviceRelease {
    use tore_session::wire::entity::{POSITION_STEP, VELOCITY_STEP, radians};
    let [yaw, pitch, bank] = attitude.map(radians);
    DeviceRelease {
        owner: aircraft,
        kind: if flare {
            EffectKind::Flare
        } else {
            EffectKind::Chaff
        },
        release: Release {
            position: position.map(|v| v as f64 * POSITION_STEP),
            velocity: velocity.map(|v| v as f64 * VELOCITY_STEP),
            basis: Basis::new(yaw, pitch, bank),
        },
        number,
        tick: u64::from(tick),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_sim::combat::live::DeviceRelease as Combat;
    use tore_world::test_support::resources::resources;

    /// The content of an import is worked out once for the import: the
    /// worker's result is what a join and a host are then given, for the
    /// same resources, and another import gets its own.
    #[test]
    fn the_games_content_is_worked_out_once_for_each_import() {
        // Its own cache: other tests use the game's at the same time.
        let cache = ContentCache::new();
        let one = Arc::new(resources());
        cache.start(&one);
        // Asking again while it works, or after, starts nothing new.
        cache.start(&one);
        let first = cache.get(&one);
        let again = cache.get(&one);
        assert!(Arc::ptr_eq(&first, &again));
        assert!(!first.digests().is_empty());
        // Another import is another computation, and the first stays right.
        let mut changed = resources();
        changed.insert("EXTRA.JT".into(), vec![1, 2, 3]);
        let two = Arc::new(changed);
        let other = cache.get(&two);
        assert!(!Arc::ptr_eq(&first, &other));
        let back = cache.get(&one);
        assert_eq!(back.digests(), first.digests());
    }

    /// What combat released, sent by the host and read back here, is the
    /// release combat made to within the wire's steps.
    #[test]
    fn a_countermeasure_release_comes_back_as_combat_made_it() {
        let made = Combat {
            owner: 3,
            kind: EffectKind::Flare,
            release: Release {
                position: [12_345.5, 20_000.25, -4_321.125],
                velocity: [310.5, -12.25, 480.75],
                basis: Basis::new(1.2, -0.3, 0.45),
            },
            number: 7,
            tick: 900,
            left: Some(12),
        };
        let WireEvent::Countermeasure {
            aircraft,
            flare,
            position,
            velocity,
            attitude,
            number,
            ..
        } = tore_session::wire::from_world::countermeasure_event(&made)
        else {
            panic!("a countermeasure event");
        };
        let read = release_of(
            aircraft, flare, &position, &velocity, &attitude, number, 900,
        );
        assert_eq!(
            (read.owner, read.kind, read.number),
            (3, EffectKind::Flare, 7)
        );
        for i in 0..3 {
            assert!((read.release.position[i] - made.release.position[i]).abs() < 1. / 32.);
            assert!((read.release.velocity[i] - made.release.velocity[i]).abs() < 1. / 64.);
            // A turn in 65,536 steps: about a ten-thousandth of a radian.
            for (a, b) in [
                (read.release.basis.forward[i], made.release.basis.forward[i]),
                (read.release.basis.up[i], made.release.basis.up[i]),
            ] {
                assert!((a - b).abs() < 2e-4, "{a} {b}");
            }
        }
        let chaff = Combat {
            kind: EffectKind::Chaff,
            ..made
        };
        let WireEvent::Countermeasure { flare, .. } =
            tore_session::wire::from_world::countermeasure_event(&chaff)
        else {
            panic!("a countermeasure event");
        };
        assert!(!flare);
    }
}

#[cfg(test)]
#[path = "keepalive_tests.rs"]
mod keepalive_tests;

#[cfg(test)]
#[path = "rejoin_tests.rs"]
mod rejoin_tests;

#[cfg(test)]
#[path = "join_tests.rs"]
mod join_tests;

#[cfg(test)]
#[path = "rejoin_game_tests.rs"]
mod rejoin_game_tests;
