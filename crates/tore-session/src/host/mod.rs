//! The host session: one mission's `World` on a fixed 120 Hz clock, the
//! transport's server endpoint, and one record per connection (its seat, its
//! input buffer, its wire state and its event queue). See
//! docs/ARCHITECTURE.md, "The host session" and "Joining, leaving and the end
//! of a mission"; the dedicated server's rules are docs/DEDICATED-SERVER.md,
//! "The mission lifecycle".
//!
//! The host never reads a clock or touches a socket for the mission: the
//! caller passes the time in ([`Host::update`]), feeds it datagrams
//! ([`Host::receive`]) and sends what it gives ([`Host::poll_transmit`]).
//! Only the tick-cost statistic reads the process's clock, and nothing the
//! mission does depends on it.
//!
//! Each tick the host applies joins and departures as `MissionCommand::Take`
//! and `GiveBack`, takes every seat's input from its buffer, steps the world,
//! sorts the tick's output into each connection's events, notes what each
//! player's game could not foresee, and every few ticks sends each seated
//! player a snapshot and, when due, its plane's exact state.
//!
//! Before the mission flies, and again after each mission, the host is in its
//! **lobby** (slice EF4, docs/ARCHITECTURE.md "The lobby"): players stay
//! connected, take slots, choose loadouts and mark ready; the King (in a game
//! a player hosts) changes the mission, starts it, ends it and kicks; a
//! dedicated server starts as its `start` setting says. Every player is sent
//! the lobby's state whenever it changes.
//!
//! Stage F phase 2 (docs/ARCHITECTURE.md, "Phase 2: the rest of stage F")
//! adds the King's settings ([`crate::settings`]), the crown, revival,
//! scoring, the results, observers and the idle aircraft, each in a module
//! of its own (`king`, `revive`, `score`, `results`, `observe`, `away`) that
//! this one calls.
//!
//! Stage K (docs/ARCHITECTURE.md, "Host migration and rejoin") steps the
//! world through the journal ([`crate::journal::apply_tick`]) and adds its
//! own modules (`journal`, `standby`, `resume`, `rejoin`, `succession`,
//! `state`), each a seam slice K0 placed for a later slice to fill.
//!
//! Stage L (docs/ARCHITECTURE.md, "Compatibility") adds each player's
//! content, the gaps it leaves and the words about them (`content`): a
//! mission or a loadout that uses an item in a gap is refused, and a player
//! who cannot fly the mission is unable in words about the item.

// Stage F phase 2's parts (slice F2-0 adds them as hooks; each slice in
// docs/ARCHITECTURE.md, "Phase 2 slices", fills its own).
mod away;
// The lobby pass's Autobalance (slice A1).
mod balance;
mod callsigns;
mod chat;
pub mod config;
pub mod content;
#[cfg(test)]
mod content_tests;
mod discover;
mod flight_report;
pub mod inputs;
mod king;
#[cfg(test)]
mod king_tests;
mod lineage_take;
mod loading;
mod lobby;
mod observe;
#[cfg(test)]
mod observe_tests;
#[cfg(test)]
mod placement_seed_tests;
mod results;
mod revive;
mod score;
pub mod sequence;
mod sides;
mod sorting;
#[cfg(test)]
mod tests;
// Stage K's parts (slice K0 adds them as hooks; each slice in
// docs/ARCHITECTURE.md, "How stage K lands", fills its own).
mod journal;
pub(crate) mod rejoin;
pub mod resume;
#[cfg(test)]
mod resume_tests;
mod standby;
mod state;
mod succession;

pub use config::{AfterEnd, BuildId, CrownRule, HostConfig, HostError, OpenPlanes, StartMode};
pub use loading::LoadingSocket;
pub use lobby::LobbyEvent;
pub use resume::{
    DROP_AFTER, FAST_FORWARD_TICKS, Present, RESUME_WINDOW, ResumeNote, Resumption, hosting_answer,
    reach_packet,
};
pub use sorting::{BURST_SLACK_TICKS, round_interval};
pub use standby::{StandbyFigures, StreamBytes};

use crate::journal::Tick;
use crate::wire::chat::{RateLimit, Receiver};
use crate::wire::connection::HostConnection;
use crate::wire::entity::{Entity, EntityKind};
use crate::wire::events::WireEvent;
use crate::wire::inputs::{InputFrame, InputsSection};
use crate::wire::messages::{
    self, Debrief, DebriefObjective, DebriefPilot, EndReason, Goodbye, LobbyPhase, LobbyPlayer,
    LobbySlot, LobbyState, Message, MissionEnded, PilotStatus, RosterPilot, RosterPlane, Seated,
    SlotRequest, StartRule, kind,
};
use crate::wire::snapshot::SnapshotHeader;
use crate::wire::{
    PROTOCOL_VERSION, Path, Platform, SECTION_FILLER, SECTION_INPUTS, SECTION_OWN_STATE, WireError,
    from_world,
};
use inputs::InputBuffer;
use lobby::Entry;
use sorting::{Timed, Tracker, Wide};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tore_net::{
    AcceptInfo, CloseReason, ConnectDetails, ConnectionId, Datagrams, Decision, DisconnectReason,
    Event, Gate, RefuseReason, Server, ServerConfig, ServerEvent, Token, Transmit,
};
use tore_sim::ai::launch::Side;
use tore_sim::combat::live;
use tore_world::debrief;
use tore_world::mission::{LoadoutSpec, MissionSpec, StationLoad};
use tore_world::resources::{Manifest, ResourceReads};
use tore_world::seats::{Pilot, PlaneId, SeatId};
use tore_world::world::plane::{ExactState, OwnPlane, OwnshipTerms};
use tore_world::world::{Cue, MissionCommand, Seating, TickOutput, World};

/// Ticks a second, fixed.
pub const TICKS_PER_SECOND: u64 = 120;
/// Ticks the host runs at most in one update to catch up; beyond it the
/// backlog is dropped and noted as an overload. Simulated time is never
/// skipped: the mission runs slower than real time instead.
pub const MAX_CATCH_UP_TICKS: u64 = 30;
/// The Messages section's budget in a snapshot packet during flight.
pub const FLIGHT_MESSAGE_BUDGET: usize = 256;
/// A player's plane's exact state goes out at least this often (1 s).
pub const OWN_STATE_INTERVAL_TICKS: u64 = 120;
/// The longest the host holds a seat's exact states back for its Seated
/// message to be acknowledged, ticks (3 seconds): a lost fragment is sent
/// again after about a round trip, but a message queue that never empties
/// must not hold them for good.
pub const SEATED_HOLD_TICKS: u64 = 360;
/// How long a departing connection has for its last messages (the debrief,
/// Mission ended) to be acknowledged before the host disconnects it anyway.
pub const CLOSE_GRACE: Duration = Duration::from_secs(5);
/// The longest [`Host::next_wake`] when no tick is due: often enough for the
/// transport's keepalives and the lifecycle's timers.
pub const IDLE_WAKE: Duration = Duration::from_millis(10);
/// The seat ids a host gives out.
const SEAT_IDS: std::ops::RangeInclusive<u8> = 0..=254;
/// Lobby requests a connection may make a second; more are dropped
/// unanswered (agent decision, EF4 review): a screen makes a few, a flood
/// would fill the seated players' reliable budget with lobby states.
pub const LOBBY_REQUESTS_PER_SECOND: u32 = 20;
/// The shortest time between two lobby states to one player in the lobby,
/// and to one flying (agent decision, EF4 review): changes in between go
/// out together.
pub const LOBBY_INTERVAL: Duration = Duration::from_millis(250);
pub const LOBBY_INTERVAL_FLYING: Duration = Duration::from_secs(1);
/// The refusal of a phase 2 request whose slice is not built yet.
pub const NOT_AVAILABLE: &str = "Not available yet.";

/// The time from tick 0 to tick `ticks` of a clock at 120 a second, to the
/// nanosecond (rounded up, so a tick is never due early).
fn ticks_time(ticks: u64) -> Duration {
    Duration::from_nanos(
        (u128::from(ticks) * 1_000_000_000).div_ceil(u128::from(TICKS_PER_SECOND)) as u64,
    )
}

/// Whole ticks of a 120 a second clock in `time`.
fn whole_ticks(time: Duration) -> u64 {
    (time.as_nanos() * u128::from(TICKS_PER_SECOND) / 1_000_000_000) as u64
}

/// Where the host's mission is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The lobby: the mission is chosen and built at tick 0 but not flying,
    /// sending no snapshots, while players take slots, arm and mark ready,
    /// until it starts by the host's start rule ([`StartMode`]) or the
    /// console's `start now`.
    Lobby,
    /// Flying at 120 ticks a second.
    Flying,
    /// Ended: players get their debriefs and are disconnected. `next_in` is
    /// the time until the next mission, or `None` when the host stops after.
    Ended { next_in: Option<Duration> },
    /// Stopped: the caller may exit once it has transmitted.
    Stopped,
}

/// Why a player left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaveReason {
    /// The player ended the mission (Leave), or said so in its disconnect.
    Left,
    /// 5 seconds without a packet.
    Silent,
    /// The console's `kick`.
    Kicked,
    /// The mission ended and the host stopped.
    MissionEnded,
    /// The player who hosts the game left it.
    HostLeft,
    /// A new join from the same address replaced the connection.
    Replaced,
    /// The connection ended for another reason.
    Disconnected(DisconnectReason),
}

/// Something for the console and the log, each with the host tick.
#[derive(Clone, Debug, PartialEq)]
pub enum HostLog {
    Feedback(tore_replay::feedback::Feedback),
    /// A join was accepted.
    Connected {
        tick: u64,
        address: SocketAddr,
        callsign: String,
        /// How the player reached the host (slice J6).
        path: Path,
    },
    /// A join was refused.
    Refused {
        tick: u64,
        address: SocketAddr,
        callsign: String,
        reason: String,
    },
    /// A player's content differs from the host's.
    ContentRefused {
        tick: u64,
        callsign: String,
        names: Vec<String>,
    },
    /// A player asked for a plane and did not get one.
    SeatRefused {
        tick: u64,
        callsign: String,
        reason: String,
    },
    /// A player took a plane.
    Seated {
        tick: u64,
        seat: u8,
        callsign: String,
        plane: u32,
    },
    /// A player left; its plane, if it had one, went back to the AI (or, when
    /// destroyed, stays as it is).
    Left {
        tick: u64,
        seat: Option<u8>,
        callsign: String,
        plane: Option<u32>,
        reason: LeaveReason,
    },
    /// The mission started flying.
    MissionStarted {
        tick: u64,
    },
    /// The mission ended.
    MissionEnded {
        tick: u64,
        reason: EndReason,
    },
    /// A fresh copy of the mission was built.
    MissionRestarted {
        tick: u64,
    },
    /// The host fell behind real time by more than it catches up.
    Overloaded {
        tick: u64,
        ticks_behind: u64,
    },
    /// The mission could not go on (a step or a rebuild failed).
    Fault {
        tick: u64,
        text: String,
    },
    /// Something the host did on purpose that is neither a fault nor a
    /// player's doing: why the mission waits to start, a departed player's
    /// aircraft removed because AI is off.
    Note {
        tick: u64,
        text: String,
    },
    /// The host stopped.
    Stopped {
        tick: u64,
    },
    /// Something a player did in the lobby, or that happened to it there.
    Lobby {
        tick: u64,
        callsign: String,
        event: LobbyEvent,
    },
    /// A chat line the host routed (protocol 4): who sent it, to whom, and
    /// how many other players heard it.
    Chat {
        tick: u64,
        callsign: String,
        receiver: Receiver,
        text: String,
        heard: usize,
    },
    /// A player's game stalled: its keepalives stand in for it, and a
    /// seated player's plane flies neutral until it is back (EF-K follow-up).
    Stalled {
        tick: u64,
        seat: Option<u8>,
        callsign: String,
    },
    /// A stalled player's game is back, after this long without input.
    Resumed {
        tick: u64,
        seat: Option<u8>,
        callsign: String,
        stalled_for: Duration,
    },
    /// A mission build long enough for the loading thread to speak: it
    /// kept `players` connected through it with `rounds` rounds of loading
    /// keepalives (protocol 28, slice NET2).
    Loaded {
        tick: u64,
        players: usize,
        took: Duration,
        rounds: u64,
    },
}

impl HostLog {
    /// The host tick of the entry.
    pub fn tick(&self) -> u64 {
        match self {
            Self::Connected { tick, .. }
            | Self::Refused { tick, .. }
            | Self::ContentRefused { tick, .. }
            | Self::SeatRefused { tick, .. }
            | Self::Seated { tick, .. }
            | Self::Left { tick, .. }
            | Self::MissionStarted { tick }
            | Self::MissionEnded { tick, .. }
            | Self::MissionRestarted { tick }
            | Self::Overloaded { tick, .. }
            | Self::Fault { tick, .. }
            | Self::Note { tick, .. }
            | Self::Stopped { tick }
            | Self::Lobby { tick, .. }
            | Self::Chat { tick, .. }
            | Self::Stalled { tick, .. }
            | Self::Resumed { tick, .. }
            | Self::Loaded { tick, .. } => *tick,
            Self::Feedback(f) => f.submit_tick,
        }
    }

    /// The line a stall or its end prints in a log, the same in the
    /// dedicated server's and in a hosting game's: "seat 2 Viper: game
    /// stalled, flying neutral", "seat 2 Viper: game back after 7.4 s". `None`
    /// for any other entry.
    pub fn stall_text(&self) -> Option<String> {
        let who = |seat: &Option<u8>, callsign: &str| match seat {
            Some(seat) => format!("seat {seat} {callsign}"),
            None => callsign.to_owned(),
        };
        match self {
            Self::Stalled { seat, callsign, .. } => Some(format!(
                "{}: game stalled{}",
                who(seat, callsign),
                if seat.is_some() {
                    ", flying neutral"
                } else {
                    ""
                }
            )),
            Self::Resumed {
                seat,
                callsign,
                stalled_for,
                ..
            } => Some(format!(
                "{}: game back after {:.1} s",
                who(seat, callsign),
                stalled_for.as_secs_f64()
            )),
            _ => None,
        }
    }

    /// The line a long mission build prints in a log, the same in the
    /// dedicated server's, a hosting game's and a hosting bot's: "loaded the
    /// mission in 15.0 s; 3 players kept through it (30 rounds of loading
    /// keepalives)". `None` for any other entry.
    pub fn loading_text(&self) -> Option<String> {
        let Self::Loaded {
            players,
            took,
            rounds,
            ..
        } = self
        else {
            return None;
        };
        Some(format!(
            "loaded the mission in {:.1} s; {players} {} kept through it ({rounds} rounds of loading keepalives)",
            took.as_secs_f64(),
            if *players == 1 { "player" } else { "players" },
        ))
    }
}

/// Why a console command did nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// No connected player has this seat.
    NoSuchSeat(u8),
    /// No connected player has this lobby id.
    NoSuchPlayer(u8),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchSeat(seat) => write!(f, "no player has seat {seat}"),
            Self::NoSuchPlayer(id) => write!(f, "no player has the lobby id {id}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// The host's figures for a status line.
#[derive(Clone, Debug, PartialEq)]
pub struct HostStatus {
    pub phase: Phase,
    /// The next tick the mission steps.
    pub tick: u64,
    /// Simulated time flown.
    pub mission_time: Duration,
    /// Players connected.
    pub players: usize,
    /// Players the host can seat: the lesser of the maximum and the open
    /// planes.
    pub capacity: usize,
    /// Aircraft in the mission.
    pub aircraft: usize,
    /// Mean and longest elapsed time of a tick since the previous status.
    pub tick_cost_mean: Duration,
    pub tick_cost_max: Duration,
    /// The mean elapsed tick cost, including worker waits, as a fraction of
    /// the 120 Hz wall-time budget (0.11 is 11 percent).
    pub load: f64,
    /// Overloads since the host started.
    pub overloads: u64,
    /// Bytes a second the host sends and receives, over every connection.
    pub bytes_up_per_second: u64,
    pub bytes_down_per_second: u64,
}

/// One player's figures for the `players` command and the log.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerStatus {
    /// The player's lobby id.
    pub id: u8,
    pub seat: Option<u8>,
    pub callsign: String,
    pub address: SocketAddr,
    pub plane: Option<u32>,
    pub round_trip: Duration,
    /// Lost over sent, 0 to 1, of the host's packets; `None` before any was
    /// judged.
    pub loss: Option<f64>,
    /// The arrival spread of the player's input packets.
    pub spread: Duration,
    /// Ticks by which the player's inputs arrived before the host needed
    /// them, as last reported; `None` before seating.
    pub input_margin_ticks: Option<i32>,
    /// Ticks the host repeated the player's last input for.
    pub inputs_repeated: u64,
    /// Of those, the ticks flown neutral because its game was stalled.
    pub inputs_neutral: u64,
    /// Bytes a second the host sends the player, and receives from it.
    pub bytes_up_per_second: u64,
    pub bytes_down_per_second: u64,
}

/// Where a connection is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// In the lobby: accepted and sent the mission, not flying it.
    Lobby,
    /// Takes `plane` from `seat` at the next tick.
    Taking { seat: SeatId, plane: PlaneId },
    /// Flies its plane.
    Seated,
    /// Left the flight: its debrief is sent and its plane goes back at the
    /// next tick, and then it is back in the lobby.
    Leaving,
    /// Done: disconnected with `reason` once its messages are acknowledged,
    /// or at `deadline`.
    Closing {
        deadline: Duration,
        reason: DisconnectReason,
    },
}

/// One connection.
struct Peer {
    address: SocketAddr,
    callsign: String,
    /// This connection's requested name, before duplicate suffixes. Local only.
    requested_callsign: String,
    /// The operating system its game said it runs on when it joined.
    platform: Platform,
    /// How it reached the host (protocol 9): its Challenge answer's path,
    /// the relay for a relayed address whatever the answer says.
    path: Path,
    stage: Stage,
    /// The seat and plane while seated (and leaving).
    seat: Option<SeatId>,
    plane: Option<PlaneId>,
    wire: HostConnection,
    inputs: InputBuffer,
    /// This player's game cannot have foreseen its plane's state exactly
    /// since the last exact state went out.
    unforeseen: bool,
    /// The tick of the last exact state sent.
    last_own_state: u64,
    /// Seated was sent at this tick and its acknowledgement is awaited: no
    /// exact state goes out until it is (or [`SEATED_HOLD_TICKS`] pass), so
    /// none overtakes the Seated message.
    holding_since: Option<u64>,
    /// The newest mismatch already answered with an exact state.
    mismatch_answered: u32,
    /// The ownship terms the plane took last tick.
    terms: Option<OwnshipTerms>,
    /// What the seat's next snapshot needs of its last picture: debris,
    /// pilots' and surface units' places, for their velocities.
    picture: Option<from_world::PictureMemory>,
    /// The mission ended while it was connected, and the host stops.
    ended: bool,
    /// Its place in the lobby.
    lobby: Entry,
    /// The connection's flight: raised each time it is seated, so one
    /// flight's sections are never read against the last one's
    /// (protocol 3).
    flight: u8,
    /// It wears the crown (stage F phase 2: a lobby role, passed on).
    king: bool,
    /// The house: the connection whose game runs the host
    /// ([`HostConfig::house`]); its leaving ends the game.
    house: bool,
    /// The host said goodbye: why it disconnects the player.
    goodbye: Option<Goodbye>,
    /// The lobby changed since this player was last sent it, and when it
    /// was.
    lobby_stale: bool,
    lobby_sent: Option<Duration>,
    /// Lobby requests this second: when the second began and how many.
    requests: (Duration, u32),
    /// The last refusal written to the log, and when.
    refusal_logged: Option<(Duration, String)>,
    /// The lines this player has chatted lately (protocol 4).
    chat_rate: RateLimit,
    /// Watching the flying mission (stage F phase 2, slice F2-O1).
    watch: Option<observe::Watch>,
    /// What its Content said (stage L); `None` until it arrives.
    content: Option<content::PlayerContent>,
    /// It has been sent the newest Content gaps.
    gaps_sent: bool,
}

/// What stops the lobby's mission starting now.
enum StartBlock {
    /// Players are still to come: not an error, the lobby waits.
    Wait(String),
    /// The mission cannot be flown as it is (a gap in the content).
    Refused(String),
}

/// The lifecycle's own state.
#[derive(Clone, Copy, Debug)]
enum Life {
    Lobby,
    Flying,
    Ended {
        /// When the next mission starts; `None` when the host stops.
        next_at: Option<Duration>,
        /// When the host stops regardless of connections still closing.
        stop_at: Duration,
    },
    Stopped,
}

/// Tick costs since the last status.
#[derive(Clone, Copy, Debug, Default)]
struct Costs {
    ticks: u64,
    total: Duration,
    max: Duration,
}

/// The host session. See the module documentation.
pub struct Host {
    recording: Option<crate::recording::Recorder>,
    recording_failed: bool,
    sequence: Option<Box<dyn sequence::MissionSequence>>,
    config: HostConfig,
    /// The King's settings in force (stage F phase 2): the configuration's
    /// to start with.
    settings: crate::settings::Store,
    /// The lobby's mission: the King's or the mission file's, with no
    /// player's loadout in it.
    spec: MissionSpec,
    /// The mission a joining player is sent and builds: the lobby's, or the
    /// flying one's with the players' loadouts.
    spec_text: String,
    /// The lobby mission's number: raised with each King's change. A
    /// flight's start sends the same mission again with the players'
    /// loadouts, under the same number.
    number: u32,
    resources: Arc<BTreeMap<String, Vec<u8>>>,
    /// The mission's world, behind the journal's door (stage K, slice K1):
    /// read anywhere, changed only through the driver.
    world: journal::Driver,
    manifest: Manifest,
    server: Server,
    session_id: u64,
    peers: BTreeMap<ConnectionId, Peer>,
    /// Death and revival (stage F phase 2, `revive`): lives, delays, the
    /// seats held for players whose plane is lost, the revivals asked for.
    /// It replaces the departed players' orphans: a lost plane is held for
    /// its player while it stays, and abandoned to the mission when it goes.
    revival: revive::Revivals,
    /// The AI flies idle players' aircraft (stage F phase 2, `away`): the
    /// planes kept for away players and the stall's count.
    idle: away::Idle,
    /// Seats whose plane goes back to the AI at the next tick.
    gives: Vec<(SeatId, String)>,
    tracker: Tracker,
    life: Life,
    now: Duration,
    /// When tick 0 of the flying clock was due, and the ticks run since.
    origin: Option<Duration>,
    ticks_run: u64,
    ever_seated: bool,
    empty_since: Option<Duration>,
    roster_dirty: bool,
    /// The lobby changed: every player gets its state.
    lobby_dirty: bool,
    /// Connections so far, for the lobby's order.
    joins: u64,
    /// The next lobby id to try.
    next_id: u8,
    logs: VecDeque<HostLog>,
    costs: Costs,
    overloads: u64,
    out: TickOutput,
    /// The observers' pictures and delay ring (stage F phase 2).
    stream: observe::Stream,
    /// The King's lobby beside the settings: slot locks, sides, a crowned
    /// server's own mission (stage F phase 2, `king`).
    court: king::Court,
    /// The mission's scores (stage F phase 2, `score`).
    score: score::Scoring,
    // Stage K's state, each filled by its slice (slice K0 places them).
    /// The journal's records on their way to the standbys (slice K1).
    journal: journal::Journal,
    /// The standbys and their streams (slice K3).
    standbys: standby::Standbys,
    /// The players absent until they resume, and the resume window (K4).
    resuming: resume::Resuming,
    /// Tokens and reservations (slice K5).
    rejoin: rejoin::Rejoin,
    /// Candidates, their measures and the succession (slice K6).
    succession: succession::Succession,
    /// The listing part: the hosting thread's `HostListing` coded, opaque
    /// here, journaled for the standbys (slice K7a).
    listing: Option<Vec<u8>>,
    /// The host's content, the players' gaps and their words (stage L,
    /// `content`).
    compat: content::Compat,
    /// The loading thread's socket and timing (protocol 28, `loading`).
    loader: loading::Loader,
    /// The last wait-to-start line logged, so a line is logged when the
    /// reason changes and not on every attempt.
    wait_logged: Option<String>,
    /// When a King's force-start countdown reaches zero. The host's clock
    /// owns it, so every player is sent the same seconds left.
    force_deadline: Option<Duration>,
    /// The seconds last written into the lobby state.
    force_shown: Option<u8>,
    /// What each seat's game could not foresee, by tick (tests only).
    #[cfg(test)]
    unforeseen_log: Vec<(u64, SeatId, Unforeseen)>,
    /// Targets tests give planes, for chat's Target receiver
    /// (`chat::designated_aircraft`).
    #[cfg(test)]
    pub(crate) test_designations: BTreeMap<PlaneId, u32>,
    /// Explicit snapshot execution in equivalence tests, never an environment
    /// change or a second production pool.
    #[cfg(test)]
    snapshot_executor: Option<Arc<tore_workers::Executor>>,
    /// Every seat takes a whole picture of its own and its entities as they
    /// were made before slice S11, for the tests that compare the two.
    #[cfg(test)]
    legacy_pictures: bool,
}

/// Pure per-seat output prepared from the completed world before any name
/// registration, packet budgeting, sending, or acknowledgement bookkeeping.
struct PreparedSeat {
    picture: SeatPicture,
    readout: Option<tore_world::readout::CockpitReadout>,
}

/// A seat's share of the tick's picture: its own plane beside the part
/// every seat shares ([`from_world::SharedPicture`], slice S11), or a whole
/// picture of its own where the seats share none.
enum SeatPicture {
    Own(from_world::SeatOwn),
    Whole(Box<tore_world::snapshot::RenderSnapshot>),
}

/// `world`'s picture for the seat flying `plane`, beside `shared` when the
/// tick has one.
fn prepare_seat(
    world: &World,
    plane: PlaneId,
    shared: Option<&from_world::SharedPicture>,
) -> Option<PreparedSeat> {
    let cockpit = world
        .cockpits
        .iter()
        .find(|cockpit| cockpit.plane == plane)?;
    let picture = match shared {
        Some(_) => SeatPicture::Own(from_world::SeatOwn::of(world, plane)?),
        None => SeatPicture::Whole(Box::new(from_world::seat_picture(world, plane)?)),
    };
    // Combat's readout with the plane's share of the data link, as
    // `World::cockpit_readout` builds it (slice G7).
    let readout = world
        .combat
        .cockpit_readout(
            plane.0,
            tore_world::combat::launcher(&cockpit.flight),
            world.ai_wings.as_ref(),
            Some(cockpit),
        )
        .map(|mut readout| {
            readout.link = world.datalink.readout(plane.0);
            readout
        });
    Some(PreparedSeat { picture, readout })
}

/// Why a seat's game could not foresee its plane's state at a tick, for the
/// network matrix's figures.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unforeseen {
    /// An input was late and repeated, or a command applied at another tick.
    LateInput,
    /// A change of the ownship terms (a release, fuel or stores) or an event
    /// about the plane (a hit, a blast, damage).
    Event,
}

/// The mission `spec` built fresh from `resources`, behind the journal's
/// door, and the resources it read.
fn build_world(
    spec: &MissionSpec,
    resources: &BTreeMap<String, Vec<u8>>,
    contrail_puffs: bool,
) -> Result<(journal::Driver, Manifest), HostError> {
    let reads = ResourceReads::new(resources);
    let mut world = World::new(spec, &reads, Seating::Open)
        .map_err(|error| HostError::Mission(error.to_string()))?;
    world.combat.set_contrail_puffs(contrail_puffs);
    Ok((journal::Driver::new(world), reads.manifest()))
}

/// `spec` with a surface seed when it has a ground target or area defenses
/// (B23) and none was drawn
/// (protocol 22; the Quick Mission creator's lobby mission, a server's file
/// and a bot's mission carry 0): drawn from the session and the mission's
/// number and text before the text is sent, so every player builds the same
/// layout from it, and kept by a restart of the same mission. Never 0.
pub(super) fn with_surface_seed(mut spec: MissionSpec, session: u64, number: u32) -> MissionSpec {
    if spec.lays_out_surface() && spec.surface_seed == 0 {
        let text = tore_codec::hash::fnv1a64(spec.to_text().as_bytes());
        let mixed = session ^ (u64::from(number) << 32) ^ text;
        let drawn = tore_net::SplitMix64::new(mixed).next_u64();
        spec.surface_seed = ((drawn >> 32) as u32).max(1);
    }
    spec
}

fn session_id(entropy: tore_net::Entropy) -> u64 {
    use std::hash::BuildHasher;
    match entropy {
        tore_net::Entropy::System => std::collections::hash_map::RandomState::new().hash_one(0u8),
        tore_net::Entropy::Seeded(seed) => tore_net::SplitMix64::new(seed ^ 0x5e55_1011).next_u64(),
    }
}

/// `callsign` made unique among `taken`: `Viper`, then `Viper_2`, `Viper_3`,
/// the name shortened first so the whole stays within 15 characters.
pub fn unique_callsign<'a>(callsign: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    if !taken.clone().any(|t| t == callsign) {
        return callsign.to_owned();
    }
    for n in 2.. {
        let suffix = format!("_{n}");
        let keep = 15usize.saturating_sub(suffix.len());
        let base: String = callsign.chars().take(keep).collect();
        let candidate = format!("{base}{suffix}");
        if !taken.clone().any(|t| t == candidate) {
            return candidate;
        }
    }
    unreachable!("some suffix is free")
}

pub(super) fn debrief_pilot(p: &debrief::Pilot) -> DebriefPilot {
    DebriefPilot {
        status: match p.status {
            debrief::Status::Alive => PilotStatus::Alive,
            debrief::Status::Ejected => PilotStatus::Ejected,
            debrief::Status::Dead => PilotStatus::Dead,
        },
        damage: p.damage,
        landing_grade: p.landing_grade,
        cause: p.cause.map(str::to_owned),
        kills: p.kills,
        friendly_fire: p.friendly_fire,
        air_to_air: p.air_to_air,
        air_to_ground: p.air_to_ground,
        gun: p.gun,
        bombs: p.bombs,
        enemy_aam: p.enemy_aam,
        enemy_sam: p.enemy_sam,
        enemy_gun: p.enemy_gun,
        enemy_aaa: p.enemy_aaa,
        shot_down_by: p.shot_down_by.clone(),
    }
}

/// The wire's debrief for a seat's report.
pub fn debrief_message(report: &debrief::Report) -> Debrief {
    Debrief {
        success: report.outcome == debrief::Outcome::Success,
        objectives: report
            .objectives
            .iter()
            .map(|objective| match *objective {
                debrief::Objective::Destroy { destroyed, total } => {
                    DebriefObjective::Destroy { destroyed, total }
                }
                debrief::Objective::Protect { protected, total } => {
                    DebriefObjective::Protect { protected, total }
                }
            })
            .collect(),
        elapsed_seconds: report.elapsed_seconds,
        player: debrief_pilot(&report.player),
        wingman: report.wingman.as_ref().map(debrief_pilot),
    }
}

/// The transport's gate: the build, the password, the capacity and whether
/// the host takes joins at all.
struct HostGate {
    build: BuildId,
    password: Option<String>,
    /// Why joins are refused now, when they are.
    closed: Option<String>,
    players: usize,
    capacity: usize,
    accept: AcceptInfo,
    tick: u64,
    logs: Vec<HostLog>,
    /// The rejoin tokens that work now (stage K, slice K5): their holders
    /// are admitted whatever the room.
    tokens: Vec<Token>,
}

impl Gate for HostGate {
    fn accept(&mut self, details: &ConnectDetails) -> Decision {
        let refusal = if let Some(text) = &self.closed {
            Some((RefuseReason::ShuttingDown, text.clone()))
        } else if !self.build.matches(details.compatibility) {
            Some((
                RefuseReason::GameBuild,
                format!(
                    "The server's compatibility code is {:08X}; yours is {:08X}.",
                    self.build.compatibility, details.compatibility
                ),
            ))
        } else if self
            .password
            .as_ref()
            .is_some_and(|password| *password != details.password)
        {
            Some((RefuseReason::WrongPassword, "Wrong password.".into()))
        } else if self.players >= self.capacity
            && !details
                .token
                .is_some_and(|token| self.tokens.contains(&token))
        {
            Some((
                RefuseReason::ServerFull,
                format!("The server is full ({} players).", self.capacity),
            ))
        } else {
            None
        };
        match refusal {
            Some((reason, text)) => {
                self.logs.push(HostLog::Refused {
                    tick: self.tick,
                    address: details.address,
                    callsign: details.callsign.clone(),
                    reason: text.clone(),
                });
                Decision::Refuse { reason, text }
            }
            None => {
                self.players += 1;
                Decision::Accept(self.accept)
            }
        }
    }

    fn check_section(&mut self, _: ConnectionId, kind: u8, body: &[u8]) -> bool {
        HostConnection::check(kind, body)
    }
}

impl Host {
    /// A host for the mission `spec`, built from the imported `resources`,
    /// with `config`. Refuses a setting out of range, the retail stall-speed
    /// switch, and a mission the import cannot build or an open mission
    /// refuses.
    pub fn new(
        spec: MissionSpec,
        resources: Arc<BTreeMap<String, Vec<u8>>>,
        mut config: HostConfig,
    ) -> Result<Host, HostError> {
        config.validate()?;
        // The settings the mission carries (friendly fire, the loadout
        // rule) come from the configuration's settings (stage F phase 2).
        let settings = crate::settings::Store::from_config(&config);
        let court = king::Court::new(&config, &spec);
        let mut spec = king::with_settings(spec, &settings);
        // A distance the creator would not offer moves as it would there.
        let distance_moved = tore_world::mission_layout::distance_table::adjust(&mut spec);
        let session_id = session_id(config.entropy);
        let mut spec = with_surface_seed(spec, session_id, 1);
        let initial_seed = spec.surface_seed;
        let ((world, manifest), placement_attempts) =
            tore_world::mission_layout::seed_retry::build(&mut spec, |candidate| {
                build_world(candidate, &resources, config.contrail_puffs).map_err(|error| {
                    match error {
                        HostError::Mission(text) => text,
                        other => other.to_string(),
                    }
                })
            })
            .map_err(HostError::Mission)?;
        let mut server = Server::new(ServerConfig {
            protocol_version: PROTOCOL_VERSION,
            max_connections: config.max_players,
            // A player's game may send the Filler section (protocol 13).
            max_section_kind: SECTION_FILLER,
            entropy: config.entropy,
        });
        // The host's transport answers a Reach for its own session as the
        // game that hosts it (stage K).
        server.set_reach_session(Some(session_id));
        let tracker = Tracker::new(&world);
        // Stage L: the host's own content is what every player's is
        // compared with; computed here when the caller gave none.
        let own = config
            .content
            .clone()
            .unwrap_or_else(|| content::GameContent::shared(&resources));
        config.build.compatibility = crate::compatibility::current(&own.content);
        server.set_compatibility(config.build.compatibility);
        let compat = content::Compat::new(own, &resources);
        let mut host = Host {
            sequence: None,
            session_id,
            spec_text: spec.to_text(),
            number: 1,
            spec,
            resources,
            world,
            manifest,
            server,
            peers: BTreeMap::new(),
            revival: revive::Revivals::default(),
            idle: away::Idle::default(),
            gives: Vec::new(),
            tracker,
            life: Life::Lobby,
            now: Duration::ZERO,
            origin: None,
            ticks_run: 0,
            ever_seated: false,
            empty_since: None,
            roster_dirty: false,
            lobby_dirty: false,
            joins: 0,
            next_id: 0,
            logs: VecDeque::new(),
            costs: Costs::default(),
            overloads: 0,
            #[cfg(test)]
            unforeseen_log: Vec::new(),
            #[cfg(test)]
            test_designations: BTreeMap::new(),
            #[cfg(test)]
            snapshot_executor: None,
            #[cfg(test)]
            legacy_pictures: false,
            out: TickOutput::default(),
            stream: observe::Stream::default(),
            score: score::Scoring::default(),
            journal: journal::Journal::default(),
            standbys: standby::Standbys::default(),
            resuming: resume::Resuming::default(),
            rejoin: rejoin::Rejoin::new(config.entropy),
            succession: succession::Succession::default(),
            listing: None,
            compat,
            loader: loading::Loader::default(),
            wait_logged: None,
            force_deadline: None,
            force_shown: None,
            court,
            settings,
            recording: None,
            recording_failed: false,
            config,
        };
        if let Some(moved) = distance_moved {
            host.log_distance(&host.spec.clone(), moved);
        }
        host.log_surface_seed(&host.spec.clone(), initial_seed, placement_attempts);
        if host.start_rule() == StartMode::Now {
            host.start_flying().map_err(HostError::Mission)?;
        } else {
            host.note_start_wait(true);
        }
        Ok(host)
    }

    // ----- Driving -----------------------------------------------------

    /// Takes one datagram that arrived at `now` from `from`.
    pub fn receive(&mut self, now: Duration, from: SocketAddr, datagram: &[u8]) {
        self.now = self.now.max(now);
        let mut gate = self.gate();
        self.server.receive(now, from, datagram, &mut gate);
        let logs = std::mem::take(&mut gate.logs);
        self.logs.extend(logs);
        self.pump();
        self.send_lobby();
    }

    /// Reads every datagram waiting on `socket` (at most 1,024) and takes
    /// each, at `now`.
    pub fn receive_from<D: Datagrams + ?Sized>(
        &mut self,
        now: Duration,
        socket: &mut D,
    ) -> io::Result<()> {
        self.now = self.now.max(now);
        let mut gate = self.gate();
        let result = self.server.receive_from(socket, now, &mut gate).map(|_| ());
        let logs = std::mem::take(&mut gate.logs);
        self.logs.extend(logs);
        self.pump();
        self.send_lobby();
        result
    }

    /// Runs whatever is due at `now`: the ticks the clock owes (at most 30,
    /// then an overload note), each seated player's snapshots, the
    /// transport's timeouts and keepalives, and the lifecycle's timers.
    pub fn update(&mut self, now: Duration) {
        self.now = self.now.max(now);
        let now = self.now;
        self.server.update(now);
        self.pump();
        // Stage K: a host that took the game over holds its flying clock
        // while the players resume, then steps to the present (slice K4).
        let held = self.resume_update(now);
        match self.life {
            Life::Flying => {
                if !held {
                    self.fly(now);
                }
            }
            Life::Ended { next_at, stop_at } => {
                if let Some(next_at) = next_at {
                    if now >= next_at {
                        self.next_mission();
                    }
                } else if self.peers.is_empty() || now >= stop_at {
                    self.stop();
                }
            }
            Life::Lobby | Life::Stopped => {}
        }
        // Stage F phase 2: a crowned server left empty goes back to its file.
        self.king_update(now);
        self.succession_update(now);
        self.close_departed(now);
        self.pump();
        if matches!(self.life, Life::Flying) {
            self.check_lifecycle(now);
        } else {
            // Stage K: in the lobby the parts change with no tick.
            self.journal_parts();
        }
        // Stage K: the journal's records out to the standbys (slice K3).
        self.standby_update();
        self.tick_force();
        self.send_lobby();
    }

    /// The next datagram to send, oldest first.
    pub fn poll_transmit(&mut self) -> Option<Transmit> {
        self.server.poll_transmit()
    }

    /// Sends every queued datagram on `socket`.
    pub fn transmit<D: Datagrams + ?Sized>(&mut self, socket: &mut D) -> io::Result<()> {
        self.server.transmit(socket)
    }

    /// How long from `now` until [`Host::update`] has something to do: the
    /// next tick while flying, else at most [`IDLE_WAKE`].
    pub fn next_wake(&self, now: Duration) -> Duration {
        match (self.life, self.origin) {
            (Life::Flying, Some(origin)) => {
                let due = origin + ticks_time(self.ticks_run);
                due.saturating_sub(now).min(IDLE_WAKE)
            }
            (Life::Flying, None) => Duration::ZERO,
            _ => IDLE_WAKE,
        }
    }

    /// What the host's loop is doing, for choosing how to nap between
    /// polls: whether a mission is flying (tick deadlines) and whether any
    /// connection is open (packets and keepalives to read).
    pub fn wake_pace(&self) -> tore_net::Pace {
        tore_net::Pace {
            flying: matches!(self.life, Life::Flying),
            connections: !self.peers.is_empty() || self.server.connections().next().is_some(),
        }
    }

    /// The next entry for the console and the log, oldest first.
    pub fn poll_log(&mut self) -> Option<HostLog> {
        self.logs.pop_front()
    }

    // ----- Console -----------------------------------------------------

    /// The console's `start now`: the lobby's mission starts flying, every
    /// ready player holding a slot in it.
    pub fn start_now(&mut self) {
        if matches!(self.life, Life::Lobby) {
            // A failure is logged (a wait as a note, a build failure as a
            // fault); the lobby stays. The operator asked, so a wait is
            // said again even when it is the same as the last.
            self.wait_logged = None;
            let _ = self.start_flying();
            self.send_lobby();
        }
    }

    /// The console's `kick SEAT`: the player's plane goes back to the AI at
    /// the next tick and the player is disconnected, with no debrief.
    pub fn kick(&mut self, seat: u8) -> Result<(), CommandError> {
        let connection = self
            .peers
            .iter()
            .find(|(_, peer)| {
                peer.seat == Some(SeatId(seat))
                    || matches!(peer.stage, Stage::Taking { seat: s, .. } if s == SeatId(seat))
            })
            .map(|(id, _)| *id)
            .ok_or(CommandError::NoSuchSeat(seat))?;
        self.server.disconnect(connection, DisconnectReason::Kicked);
        self.pump();
        self.send_lobby();
        Ok(())
    }

    /// Removes the player with lobby id `player`, telling it `reason`
    /// first (the King's kick): its plane, if it flies one, goes back to the
    /// AI at the next tick with no debrief, and it is disconnected once the
    /// goodbye is acknowledged.
    pub fn kick_player(&mut self, player: u8, reason: &str) -> Result<(), CommandError> {
        let connection = self
            .peers
            .iter()
            .find(|(_, peer)| {
                peer.lobby.id == player && !matches!(peer.stage, Stage::Closing { .. })
            })
            .map(|(id, _)| *id)
            .ok_or(CommandError::NoSuchPlayer(player))?;
        self.say_goodbye(
            connection,
            Goodbye::Kicked(reason.to_owned()),
            DisconnectReason::Kicked,
        );
        if let Some(peer) = self.peers.get(&connection) {
            let callsign = peer.callsign.clone();
            self.lobby_log(callsign, LobbyEvent::Kicked(reason.to_owned()));
        }
        self.send_lobby();
        Ok(())
    }

    /// The player who hosts the game left it: a flying mission ends for
    /// everyone with "the host left the game" and their debriefs, every
    /// player is told why and disconnected once that is acknowledged, and the
    /// host stops when they are gone (or after [`CLOSE_GRACE`]).
    pub fn host_left(&mut self) {
        if matches!(self.life, Life::Stopped) {
            return;
        }
        let now = self.now;
        if matches!(self.life, Life::Flying) {
            self.end_mission(EndReason::HostLeft, None);
        }
        let ids: Vec<ConnectionId> = self.peers.keys().copied().collect();
        for id in ids {
            self.say_goodbye(id, Goodbye::HostLeft, DisconnectReason::ServerStopping);
        }
        if !matches!(self.life, Life::Ended { next_at: None, .. }) {
            self.life = Life::Ended {
                next_at: None,
                stop_at: now + CLOSE_GRACE,
            };
            let tick = self.world.tick();
            self.log(HostLog::MissionEnded {
                tick,
                reason: EndReason::HostLeft,
            });
        }
        self.send_lobby();
    }

    /// The console's `end`: every seated player gets "Mission ended" and
    /// their debrief and is disconnected; the next mission starts after the
    /// restart delay, or the host stops, as configured.
    pub fn end(&mut self) {
        let next = self.after_end();
        self.end_mission(EndReason::EndedByServer, next);
        self.send_lobby();
    }

    /// The console's `restart`: ends the mission and starts it again at
    /// once (back to the lobby with every player, or flying for
    /// `start now`).
    pub fn restart(&mut self) {
        self.end_mission(EndReason::EndedByServer, Some(Duration::ZERO));
        self.next_mission();
        self.send_lobby();
    }

    /// The console's `quit`: disconnects every player with "server stopping"
    /// and stops.
    pub fn stop(&mut self) {
        if matches!(self.life, Life::Stopped) {
            return;
        }
        for peer in self.peers.values_mut() {
            peer.ended = true;
        }
        self.server.disconnect_all(DisconnectReason::ServerStopping);
        self.pump();
        self.life = Life::Stopped;
        let tick = self.world.tick();
        self.log(HostLog::Stopped { tick });
    }

    /// Where the mission is in its lifecycle.
    pub fn phase(&self) -> Phase {
        match self.life {
            Life::Lobby => Phase::Lobby,
            Life::Flying => Phase::Flying,
            Life::Ended { next_at, .. } => Phase::Ended {
                next_in: next_at.map(|at| at.saturating_sub(self.now)),
            },
            Life::Stopped => Phase::Stopped,
        }
    }

    // ----- Statistics ----------------------------------------------------

    /// The host's figures now; the tick costs cover the ticks since the
    /// previous call.
    pub fn status(&mut self, now: Duration) -> HostStatus {
        self.now = self.now.max(now);
        let costs = std::mem::take(&mut self.costs);
        let mean = if costs.ticks == 0 {
            Duration::ZERO
        } else {
            costs.total / costs.ticks as u32
        };
        let (mut up, mut down) = (0, 0);
        for id in self.peers.keys() {
            if let Some(stats) = self.server.stats(*id) {
                up += stats.bytes_sent_per_second;
                down += stats.bytes_received_per_second;
            }
        }
        HostStatus {
            phase: self.phase(),
            tick: self.world.tick(),
            mission_time: ticks_time(self.world.tick()),
            players: self.peers.len(),
            capacity: self.capacity(),
            aircraft: self.world.roster.planes().len(),
            tick_cost_mean: mean,
            tick_cost_max: costs.max,
            load: mean.as_secs_f64() * TICKS_PER_SECOND as f64,
            overloads: self.overloads,
            bytes_up_per_second: up,
            bytes_down_per_second: down,
        }
    }

    /// Every connected player's figures.
    pub fn players(&self) -> Vec<PlayerStatus> {
        self.peers
            .iter()
            .map(|(id, peer)| {
                let stats = self.server.stats(*id);
                let seated = peer.seat.is_some();
                PlayerStatus {
                    id: peer.lobby.id,
                    seat: peer.seat.map(|s| s.0),
                    callsign: peer.callsign.clone(),
                    address: peer.address,
                    plane: peer.plane.map(|p| p.0),
                    round_trip: stats.as_ref().map_or(Duration::ZERO, |s| s.round_trip),
                    loss: stats.as_ref().and_then(|s| s.loss),
                    spread: stats.as_ref().map_or(Duration::ZERO, |s| s.spread),
                    input_margin_ticks: seated.then(|| i32::from(peer.inputs.margin())),
                    inputs_repeated: peer.inputs.repeats_total(),
                    inputs_neutral: peer.inputs.neutral_total(),
                    bytes_up_per_second: stats.as_ref().map_or(0, |s| s.bytes_sent_per_second),
                    bytes_down_per_second: stats
                        .as_ref()
                        .map_or(0, |s| s.bytes_received_per_second),
                }
            })
            .collect()
    }

    /// What each seat's game could not foresee, as (tick, seat, why).
    #[cfg(test)]
    pub(crate) fn unforeseen_log(&self) -> &[(u64, SeatId, Unforeseen)] {
        &self.unforeseen_log
    }

    /// The mission's world.
    /// The accepted aircraft of a Player Choice slot, for server presentation.
    pub fn choice_aircraft(&self, plane: u32) -> Option<tore_formats::aircraft::AircraftId> {
        let entry = self.world.roster.plane(PlaneId(plane))?;
        self.spec.player_choice[MissionSpec::wing_index(entry.slot.wing)]
            .then(|| self.world.aircraft_of(PlaneId(plane)))
            .flatten()
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// The mission text the players build from, as the Mission message
    /// sends it (with the surface seed the host drew, protocol 22).
    pub fn mission_text(&self) -> &str {
        &self.spec_text
    }

    /// The lobby's mission (the players' loadouts are not in it).
    pub fn spec(&self) -> &MissionSpec {
        &self.spec
    }

    /// The Mission message's number now.
    pub fn mission_number(&self) -> u32 {
        self.number
    }

    /// The lobby as a player with lobby id `you` is sent it.
    pub fn lobby_state(&self, you: u8) -> LobbyState {
        self.lobby(you)
    }

    /// The content manifest: every resource the mission's build read.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The settings.
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Snapshots a second to each player: the King's setting in force (slice
    /// R1; a dedicated server's file's rate to start with).
    pub fn snapshot_rate(&self) -> u32 {
        self.settings.snapshot_rate()
    }

    /// Ticks between snapshots: 120 over [`Host::snapshot_rate`].
    pub fn ticks_per_snapshot(&self) -> u32 {
        120 / self.snapshot_rate().max(1)
    }

    /// The transport's counters of datagrams dropped before a connection.
    pub fn counters(&self) -> &tore_net::Counters {
        self.server.counters()
    }

    // ----- Joins and messages --------------------------------------------

    fn log(&mut self, entry: HostLog) {
        if let Some(recorder) = &mut self.recording {
            recorder.note(&entry);
        }
        if matches!(
            &entry,
            HostLog::MissionEnded { .. } | HostLog::Stopped { .. }
        ) {
            self.finish_recording();
        }
        if matches!(&entry, HostLog::MissionRestarted { .. }) {
            self.recording_failed = false;
        }
        self.logs.push_back(entry);
    }

    /// Players the host can seat: the lesser of the maximum and the open
    /// planes. A revival's new plane is its player's, so it seats nobody
    /// more (stage F phase 2).
    fn capacity(&self) -> usize {
        let open = self
            .world
            .roster
            .planes()
            .iter()
            .filter(|plane| {
                self.open(plane.id)
                    && !self.closed_slot(plane.id.0)
                    && !self.revival.added(plane.id)
            })
            .count();
        // The King's player limit (stage F phase 2), the configuration's to
        // start with; a closed slot seats nobody.
        open.min(self.settings.max_players() as usize)
    }

    /// Whether `plane` is open to players: the open planes, which follow the
    /// King's mode by default (stage F phase 2).
    fn open(&self, plane: PlaneId) -> bool {
        // A lineage is open by its root, the plane the mission started with
        // (the lobby pass's follow-up F1): a respawn of a listed plane is
        // open too.
        let root = self.root_of(plane);
        self.world.roster.plane(plane).is_some_and(|p| {
            self.config.open_planes.allows_in(
                root.0,
                p.slot.wing.side == Side::Friendly,
                self.settings.mode(),
            )
        })
    }

    fn gate(&self) -> HostGate {
        let closed = match self.life {
            Life::Lobby | Life::Flying => None,
            Life::Ended {
                next_at: Some(at), ..
            } => Some(format!(
                "The mission has ended; the next starts in {} seconds.",
                at.saturating_sub(self.now).as_secs()
            )),
            Life::Ended { next_at: None, .. } | Life::Stopped => {
                Some("The server is stopping.".into())
            }
        };
        HostGate {
            build: self.config.build.clone(),
            // The King's password (stage F phase 2), for the next joins.
            password: self.settings.password().map(str::to_owned),
            closed,
            // A departing connection's place is free already.
            players: self
                .peers
                .values()
                .filter(|peer| !matches!(peer.stage, Stage::Closing { .. }))
                .count(),
            capacity: self.capacity(),
            accept: AcceptInfo {
                session_id: self.session_id,
                ticks_per_second: TICKS_PER_SECOND as u8,
                ticks_per_snapshot: self.ticks_per_snapshot() as u8,
                host_tick: self.world.tick() as u32,
            },
            tick: self.world.tick(),
            logs: Vec::new(),
            tokens: self.rejoin.valid_tokens(self.token_clock()),
        }
    }

    /// Queues `message` to `connection`; a queue the transport refuses ends
    /// the connection.
    fn send(&mut self, connection: ConnectionId, message: &Message) {
        let sent = message.encode().map_err(|_| ()).and_then(|body| {
            self.server
                .send_message(connection, message.kind(), &body)
                .map_err(|_| ())
        });
        if sent.is_err() {
            self.server
                .disconnect(connection, DisconnectReason::ProtocolError);
        }
    }

    /// Handles every transport event.
    fn pump(&mut self) {
        while let Some(event) = self.server.poll_event() {
            match event {
                ServerEvent::Connected {
                    connection,
                    details,
                } => {
                    // Stage K: an absent player resumes its place (slice K4).
                    if !self.resume_connected(connection, &details) {
                        let token = details.token;
                        self.connected(connection, details);
                        self.rejoin_connected(connection, token);
                    }
                    // Stage K: the ready standbys, sent to the others when
                    // they last changed (slice K10).
                    self.succession_connected(connection);
                }
                ServerEvent::Closed {
                    connection, reason, ..
                } => self.closed(connection, reason),
                ServerEvent::Discover { from, query } => self.answer_discover(from, &query),
                ServerEvent::Reservation { from, query } => {
                    let answer = self.reservation_answer(query);
                    self.server.answer_reservation(from, answer);
                }
                ServerEvent::Connection { connection, event } => match event {
                    Event::Message { kind, body } => self.message(connection, kind, &body),
                    Event::Payload { sections, .. } => {
                        for section in sections {
                            match section.kind {
                                SECTION_INPUTS => self.inputs(connection, &section.body),
                                SECTION_FILLER => {
                                    self.filler_received(connection, section.body.len());
                                }
                                _ => {}
                            }
                        }
                    }
                    Event::Delivered { sequence } => {
                        if let Some(peer) = self.peers.get_mut(&connection) {
                            peer.wire.delivered(sequence);
                        }
                    }
                    Event::Lost { sequence } => {
                        if let Some(peer) = self.peers.get_mut(&connection) {
                            peer.wire.lost(sequence);
                        }
                    }
                    Event::Stalled => self.stalled(connection, None),
                    Event::Resumed { stalled_for } => self.stalled(connection, Some(stalled_for)),
                },
            }
        }
    }

    /// A connection's game stalled (its first keepalive; `None`) or came
    /// back after `Some` time: a seated player's plane flies neutral
    /// meanwhile (the lead's reading of John's pause rule, EF-K follow-up),
    /// and the log says so.
    fn stalled(&mut self, connection: ConnectionId, back_after: Option<Duration>) {
        let tick = self.world.tick();
        let Some(peer) = self.peers.get_mut(&connection) else {
            return;
        };
        let seat = match peer.stage {
            Stage::Seated => peer.seat.map(|s| s.0),
            _ => None,
        };
        let callsign = peer.callsign.clone();
        match back_after {
            None => {
                if seat.is_some() {
                    peer.inputs.set_stalled();
                }
                self.log(HostLog::Stalled {
                    tick,
                    seat,
                    callsign,
                });
            }
            Some(stalled_for) => self.log(HostLog::Resumed {
                tick,
                seat,
                callsign,
                stalled_for,
            }),
        }
    }

    fn connected(&mut self, connection: ConnectionId, details: ConnectDetails) {
        let callsign = unique_callsign(
            &details.callsign,
            self.peers.values().map(|p| p.callsign.as_str()),
        );
        // Lobby ids go round, so the id of a player who just left is not
        // given to the next one at once (a King's kick meant for the one
        // never lands on the other).
        let used: BTreeSet<u8> = self.peers.values().map(|p| p.lobby.id).collect();
        let id = (0..=u8::MAX)
            .map(|n| self.next_id.wrapping_add(n))
            .find(|id| !used.contains(id))
            .expect("the transport holds at most 30 connections");
        self.next_id = id.wrapping_add(1);
        self.joins += 1;
        let house = self.config.house == Some(details.address);
        let king = self.crowned_at_join(house);
        self.peers.insert(
            connection,
            Peer {
                address: details.address,
                callsign: callsign.clone(),
                requested_callsign: details.callsign.clone(),
                platform: details.platform,
                path: details.path,
                stage: Stage::Lobby,
                seat: None,
                plane: None,
                wire: HostConnection::new(self.ticks_per_snapshot()),
                inputs: InputBuffer::new(),
                unforeseen: false,
                last_own_state: 0,
                holding_since: None,
                mismatch_answered: 0,
                terms: None,
                picture: None,
                ended: false,
                lobby: Entry::new(id, self.joins),
                flight: 0,
                king,
                house,
                goodbye: None,
                lobby_stale: true,
                lobby_sent: None,
                requests: (Duration::ZERO, 0),
                refusal_logged: None,
                chat_rate: RateLimit::default(),
                watch: None,
                content: None,
                gaps_sent: false,
            },
        );
        let tick = self.world.tick();
        self.log(HostLog::Connected {
            tick,
            address: details.address,
            callsign: callsign.clone(),
            path: details.path,
        });
        if king {
            self.lobby_log(callsign, LobbyEvent::Crowned);
        }
        // The house's connection is the hosting game's own, over the
        // in-process link (a house at any other address is not exempt, and
        // the link drops socket datagrams that claim its address): it is never
        // dropped for silence. A window held still (dragged, resized, a
        // long load) stalls only that game; its plane flies on with its
        // last controls, as any late player's does, until it catches up.
        if house && details.address == tore_net::LINK_ADDRESS {
            self.server.set_silence_exempt(connection, true);
        }
        let mission = self.mission_message();
        self.send(connection, &mission);
        let roster = Message::Roster(self.roster());
        self.send(connection, &roster);
        // Stage F phase 2: the planes revivals have added to the mission.
        self.revive_connected(connection);
        self.lobby_dirty = true;
    }

    /// The Mission message a player builds the mission from now.
    fn mission_message(&self) -> Message {
        Message::Mission(messages::Mission {
            spec: self.spec_text.clone(),
            manifest: self.manifest.clone(),
            host_tick: self.world.tick() as u32,
            contrail_sortie: self.world.combat.contrail_sortie(),
            number: self.number,
        })
    }

    fn closed(&mut self, connection: ConnectionId, reason: CloseReason) {
        let Some(peer) = self.peers.remove(&connection) else {
            return;
        };
        self.lobby_dirty = true;
        // Stage L: the gaps no longer count it.
        self.compat.touch();
        let reason = if peer.goodbye == Some(Goodbye::HostLeft) {
            LeaveReason::HostLeft
        } else if peer.ended {
            LeaveReason::MissionEnded
        } else {
            match reason {
                CloseReason::Replaced => LeaveReason::Replaced,
                CloseReason::Disconnected { reason, .. } => match reason {
                    DisconnectReason::Left => LeaveReason::Left,
                    DisconnectReason::Timeout => LeaveReason::Silent,
                    DisconnectReason::Kicked => LeaveReason::Kicked,
                    other => LeaveReason::Disconnected(other),
                },
                CloseReason::NoAnswer | CloseReason::Refused { .. } => {
                    LeaveReason::Disconnected(DisconnectReason::Other(0))
                }
            }
        };
        // Stage K: its token's 24 hours start (a kick voids it) and a plane
        // it flew is kept for it.
        self.rejoin_left(&peer, reason);
        // A seated player's plane goes back to the AI at the next tick, with
        // no debrief; a leaving one's is on its way already.
        if peer.stage == Stage::Seated
            && let Some(seat) = peer.seat
        {
            self.gives.push((seat, peer.callsign.clone()));
        }
        let (seat, plane) = (peer.seat, peer.plane);
        let tick = self.world.tick();
        self.log(HostLog::Left {
            tick,
            seat: seat.map(|s| s.0),
            callsign: peer.callsign,
            plane: plane.map(|p| p.0),
            reason,
        });
        // The house's leaving ends a game a player hosts, unless it handed
        // the game over first (stage K, `Host::hand_over`); a King who is
        // not the house passes the crown to the longest-connected player.
        if peer.house
            && !self.resume_handed()
            && !matches!(self.life, Life::Stopped | Life::Ended { next_at: None, .. })
        {
            self.host_left();
        } else if peer.king {
            self.crown_departed();
        }
    }

    fn message(&mut self, connection: ConnectionId, kind: u8, body: &[u8]) {
        let Ok(message) = Message::decode(kind, body) else {
            self.server
                .disconnect(connection, DisconnectReason::ProtocolError);
            return;
        };
        if !message.from_player() {
            // Host-to-client messages from a client break the protocol.
            self.server
                .disconnect(connection, DisconnectReason::ProtocolError);
            return;
        }
        let Some(peer) = self.peers.get(&connection) else {
            return;
        };
        if matches!(peer.stage, Stage::Closing { .. }) {
            return;
        }
        let king = peer.king;
        // A resumed player's Backlog and a standby's status are not lobby
        // requests: neither counts against the 20 a second (stage K).
        if !matches!(
            message,
            Message::Leave
                | Message::ContentRefused(_)
                | Message::Backlog(_)
                | Message::StandbyStatus(_)
        ) {
            let now = self.now;
            let Some(peer) = self.peers.get_mut(&connection) else {
                return;
            };
            if now.saturating_sub(peer.requests.0) >= Duration::from_secs(1) {
                peer.requests = (now, 0);
            }
            peer.requests.1 += 1;
            if peer.requests.1 > LOBBY_REQUESTS_PER_SECOND {
                if peer.requests.1 == LOBBY_REQUESTS_PER_SECOND + 1 {
                    let callsign = peer.callsign.clone();
                    self.lobby_log(callsign, LobbyEvent::TooManyRequests);
                }
                return;
            }
        }
        // A request for an earlier mission than the one the player has now
        // been sent is refused: it was meant for a mission that changed.
        let mission = match &message {
            Message::TakePlane(m) => Some(m.mission),
            Message::Slot(m) => Some(m.mission),
            Message::Loadout(m) => Some(m.mission),
            Message::SetReady(m) => Some(m.mission),
            Message::SlotLock(m) => Some(m.mission),
            Message::Revive { mission, .. } => Some(*mission),
            _ => None,
        };
        if mission.is_some_and(|number| number != self.number) {
            let reason = "The mission has changed; choose again.".to_owned();
            if let Message::TakePlane(_) = message {
                self.refuse_seat(connection, reason);
            } else {
                self.refuse(connection, message.kind(), reason);
            }
            return;
        }
        let king_only = matches!(
            message,
            Message::ChangeMission(_)
                | Message::Start
                | Message::ForceStart
                | Message::Kick(_)
                | Message::EndMission
                | Message::PassCrown(_)
                | Message::Settings(_)
                | Message::SlotLock(_)
                | Message::Release(_)
        );
        if king_only && !king {
            self.refuse(
                connection,
                message.kind(),
                "Only the King may do that.".into(),
            );
            return;
        }
        match message {
            Message::TakePlane(take) => self.take_plane(connection, take.plane),
            Message::Leave => self.leave(connection),
            Message::ContentRefused(refused) => self.content_refused(connection, refused),
            // Stage L: the player's content, its first message.
            Message::Content(content) => self.content_arrived(connection, *content),
            Message::Slot(slot) => {
                let result = self.slot(connection, slot.request);
                self.answer(connection, kind::SLOT, "a slot", result);
            }
            Message::Loadout(load) => {
                let result =
                    self.choose_loadout(connection, load.plane, load.aircraft, load.loadout);
                self.answer(connection, kind::LOADOUT, "a loadout", result);
            }
            Message::SetReady(ready) => {
                let result = self.set_ready(connection, ready.ready);
                self.answer(connection, kind::SET_READY, "ready", result);
            }
            Message::ChangeMission(text) => {
                let result = match self.mission_refusal() {
                    Some(why) => Err(why),
                    None => self.change_mission(connection, &text),
                };
                self.answer(connection, kind::CHANGE_MISSION, "a new mission", result);
            }
            Message::Start => {
                let result = self.king_start();
                self.answer(connection, kind::START, "the start", result);
            }
            Message::ForceStart => {
                let result = self.king_force();
                self.answer(connection, kind::FORCE_START, "the force start", result);
            }
            Message::Kick(kick) => {
                let result = if self.peers.get(&connection).map(|p| p.lobby.id) == Some(kick.player)
                {
                    Err("The King cannot kick the King.".to_owned())
                } else if let Some(why) = self.kick_refusal(kick.player) {
                    Err(why)
                } else {
                    self.kick_player(kick.player, &kick.reason)
                        .map_err(|_| "There is no such player.".to_owned())
                };
                self.answer(connection, kind::KICK, "a kick", result);
            }
            Message::FeedbackSend(f) => self.feedback(connection, f),
            Message::ChatSend(send) => self.chat(connection, send),
            Message::EndMission => {
                let result = if matches!(self.life, Life::Flying) {
                    let next = self.after_end();
                    self.end_mission(EndReason::EndedByServer, next);
                    Ok(())
                } else {
                    Err("The mission is not flying.".to_owned())
                };
                self.answer(connection, kind::END_MISSION, "the end", result);
            }
            // Stage F phase 2: each slice's module answers its own.
            Message::PassCrown(player) => {
                let result = self.pass_crown(connection, player);
                self.answer(connection, kind::PASS_CROWN, "the crown", result);
            }
            Message::Settings(change) => {
                let result = self.change_settings(connection, &change);
                self.answer(connection, kind::SETTINGS, "the settings", result);
            }
            Message::SlotLock(lock) => {
                let result = self.lock_slot(connection, lock.plane, &lock.lock);
                self.answer(connection, kind::SLOT_LOCK, "a slot lock", result);
            }
            Message::Revive {
                aircraft, loadout, ..
            } => {
                let result = self.revive_selected(connection, aircraft, loadout);
                self.answer(connection, kind::REVIVE, "a revival", result);
            }
            Message::Observe(observe) => {
                let result = self.observe_request(connection, observe);
                self.answer(connection, kind::OBSERVE, "watching", result);
            }
            Message::Away => {
                let result = self.away_request(connection);
                self.answer(connection, kind::AWAY, "away", result);
            }
            Message::Back => {
                let result = self.back_request(connection);
                self.answer(connection, kind::BACK, "back", result);
            }
            // Stage K: each slice's module answers its own (slice K0 refuses
            // them all, "Not available yet.").
            Message::Candidate(report) => {
                let result = self.candidate_report(connection, &report);
                self.answer(connection, kind::CANDIDATE, "a candidate report", result);
            }
            Message::ReachReport(report) => {
                let result = self.reach_report(connection, &report);
                self.answer(connection, kind::REACH_REPORT, "a reach report", result);
            }
            Message::StandbyStatus(status) => {
                let result = self.standby_status(connection, status);
                self.answer(connection, kind::STANDBY_STATUS, "a standby status", result);
            }
            Message::Resume(resume) => {
                let result = self.resume_request(connection, resume);
                self.answer(connection, kind::RESUME, "a resume", result);
            }
            Message::Backlog(backlog) => {
                let result = self.backlog(connection, &backlog);
                self.answer(connection, kind::BACKLOG, "a backlog", result);
            }
            Message::TakenOver(taken) => {
                let result = self.taken_over(connection, taken);
                self.answer(connection, kind::TAKEN_OVER, "a takeover", result);
            }
            Message::Release(plane) => {
                let result = self.release_request(connection, plane);
                self.answer(connection, kind::RELEASE, "a release", result);
            }
            Message::Rejoin(token) => {
                let result = self.rejoin_request(connection, token);
                self.answer(connection, kind::REJOIN, "a rejoin", result);
            }
            _ => {}
        }
    }

    /// When the mission ends, how long until the next: `None` when the
    /// host stops.
    fn after_end(&self) -> Option<Duration> {
        match self.config.after_end {
            AfterEnd::Restart => Some(self.config.restart_delay),
            AfterEnd::Quit => None,
        }
    }

    /// Refuses a lobby request with `reason`, and logs it.
    fn refuse(&mut self, connection: ConnectionId, request: u8, reason: String) {
        self.send(
            connection,
            &Message::Refused {
                request,
                reason: reason.clone(),
            },
        );
        let what = match request {
            kind::SLOT => "a slot",
            kind::LOADOUT => "a loadout",
            kind::SET_READY => "ready",
            kind::CHANGE_MISSION => "a new mission",
            kind::START => "the start",
            kind::KICK => "a kick",
            kind::END_MISSION => "the end",
            kind::PASS_CROWN => "the crown",
            kind::SETTINGS => "the settings",
            kind::SLOT_LOCK => "a slot lock",
            kind::REVIVE => "a revival",
            kind::OBSERVE => "watching",
            kind::AWAY => "away",
            kind::BACK => "back",
            kind::CANDIDATE => "a candidate report",
            kind::REACH_REPORT => "a reach report",
            kind::STANDBY_STATUS => "a standby status",
            kind::RESUME => "a resume",
            kind::BACKLOG => "a backlog",
            kind::TAKEN_OVER => "a takeover",
            kind::RELEASE => "a release",
            kind::REJOIN => "a rejoin",
            _ => "a request",
        };
        self.log_refusal(connection, what, reason);
    }

    /// Writes a refusal to the log, unless the same one was written to the
    /// same player within a second.
    fn log_refusal(&mut self, connection: ConnectionId, what: &'static str, reason: String) {
        let now = self.now;
        let Some(peer) = self.peers.get_mut(&connection) else {
            return;
        };
        if peer.refusal_logged.as_ref().is_some_and(|(at, said)| {
            *said == reason && now.saturating_sub(*at) < Duration::from_secs(1)
        }) {
            return;
        }
        peer.refusal_logged = Some((now, reason.clone()));
        let callsign = peer.callsign.clone();
        self.lobby_log(
            callsign,
            LobbyEvent::Refused {
                request: what,
                reason,
            },
        );
    }

    /// A lobby request's outcome: a refusal is sent back with its reason.
    fn answer(
        &mut self,
        connection: ConnectionId,
        request: u8,
        _what: &'static str,
        result: Result<(), String>,
    ) {
        if let Err(reason) = result {
            self.refuse(connection, request, reason);
        }
    }

    fn lobby_log(&mut self, callsign: String, event: LobbyEvent) {
        let tick = self.world.tick();
        self.log(HostLog::Lobby {
            tick,
            callsign,
            event,
        });
    }

    /// Sends `goodbye` and disconnects the player with `reason` once it is
    /// acknowledged (or after [`CLOSE_GRACE`]); a flying player's plane goes
    /// back to the AI at the next tick, with no debrief.
    fn say_goodbye(
        &mut self,
        connection: ConnectionId,
        goodbye: Goodbye,
        reason: DisconnectReason,
    ) {
        let now = self.now;
        let Some(peer) = self.peers.get(&connection) else {
            return;
        };
        if matches!(peer.stage, Stage::Closing { .. }) && peer.goodbye.is_some() {
            return;
        }
        if peer.stage == Stage::Seated
            && let Some(seat) = peer.seat
        {
            self.gives.push((seat, peer.callsign.clone()));
        }
        self.send(connection, &Message::Goodbye(goodbye.clone()));
        if let Some(peer) = self.peers.get_mut(&connection) {
            peer.goodbye = Some(goodbye);
            peer.stage = Stage::Closing {
                deadline: now + CLOSE_GRACE,
                reason,
            };
        }
        self.lobby_dirty = true;
        // Stage L: a closing connection no longer counts for the gaps.
        self.compat.touch();
    }

    fn inputs(&mut self, connection: ConnectionId, body: &[u8]) {
        // Stage K: a resumed seat's inputs wait with its backlog (K4).
        if self.resume_inputs(connection, body) {
            return;
        }
        let next = self.world.tick();
        let Some(peer) = self.peers.get_mut(&connection) else {
            return;
        };
        if !matches!(peer.stage, Stage::Seated) {
            return;
        }
        let Ok(section) = InputsSection::decode(body) else {
            return;
        };
        // An earlier flight's inputs that came late fly nothing now.
        if section.flight != peer.flight {
            return;
        }
        let refused = peer.inputs.receive(&section, next).is_err();
        // The arrival spread, in the player's own time: the tick it sampled.
        let sent = ticks_time(u64::from(section.newest_tick));
        self.server.note_arrival(connection, sent, self.now);
        if refused {
            self.server
                .disconnect(connection, DisconnectReason::ProtocolError);
        }
    }

    /// Whether some other connection already takes or flies `plane`.
    fn reserved(&self, plane: PlaneId) -> bool {
        self.peers.values().any(|peer| {
            matches!(peer.stage, Stage::Taking { plane: p, .. } if p == plane)
                || (peer.plane == Some(plane) && !matches!(peer.stage, Stage::Closing { .. }))
        }) || self.away_reserved(plane)
    }

    /// The lowest seat id no connection, pending take or departed player's
    /// plane holds.
    fn free_seat(&self) -> Option<SeatId> {
        let used: BTreeSet<SeatId> = self
            .peers
            .values()
            .filter_map(|peer| match peer.stage {
                Stage::Taking { seat, .. } => Some(seat),
                _ => peer.seat,
            })
            .chain(self.revival.held_seats())
            .chain(self.revival.pending_seats())
            .chain(self.gives.iter().map(|(seat, _)| *seat))
            .collect();
        SEAT_IDS.map(SeatId).find(|seat| !used.contains(seat))
    }

    /// Why `plane` cannot be taken by `seat` for `connection` now, if it
    /// cannot: another player flies it or holds its slot, or it cannot fly.
    fn refusal(&self, seat: SeatId, plane: PlaneId, connection: ConnectionId) -> Option<String> {
        let Some(entry) = self.world.roster.plane(plane) else {
            return Some(format!("There is no plane {}.", plane.0));
        };
        if !self.open(plane) {
            return Some(format!("Plane {} is not open to players.", plane.0));
        }
        // A lost plane: abandoned to the mission, or held for its player
        // in the lobby (stage F phase 2's revival).
        if entry.pilot == Pilot::Lost
            || matches!(entry.pilot, Pilot::Human(seat) if self.held_callsign(seat).is_some())
        {
            return Some(format!(
                "Plane {} is destroyed or has lost its pilot.",
                plane.0
            ));
        }
        // Stage F phase 2: a plane the AI flies for an away player.
        if let Some(why) = self.away_take_refusal(connection, plane) {
            return Some(why);
        }
        if entry.pilot != Pilot::Ai || self.reserved(plane) {
            return Some(format!("Plane {} is flown by another player.", plane.0));
        }
        // A lineage's slot is its root's (the lobby pass's follow-up F1).
        let root = self.root_of(plane);
        if let Some(other) = self.holder(root, connection) {
            return Some(format!(
                "{} holds plane {}.",
                self.peers[&other].callsign, root.0
            ));
        }
        // Stage F phase 2: the King's rules, then the revival rules.
        if let Some(why) = self
            .king_take_refusal(connection, plane)
            .or_else(|| self.revive_take_refusal(connection, plane))
        {
            return Some(why);
        }
        if !self.settings.ai() && self.world.head_lost(plane) {
            return None;
        }
        self.world
            .can_take(seat, plane)
            .err()
            .map(|_| format!("Plane {} is destroyed or has lost its pilot.", plane.0))
    }

    /// The slots: the planes open to players, in plane order.
    fn slots(&self) -> Vec<tore_world::mission::OpenPlane> {
        self.spec
            .open_planes()
            .into_iter()
            .map(|mut plane| {
                if matches!(self.life, Life::Flying) {
                    let head = self.world.lineage_head(PlaneId(plane.id));
                    plane.aircraft = self
                        .revival
                        .choices
                        .get(&head)
                        .copied()
                        .or_else(|| self.world.aircraft_of(head))
                        .unwrap_or(plane.aircraft);
                }
                plane
            })
            .filter(|plane| {
                self.config.open_planes.allows_in(
                    plane.id,
                    plane.wing.side == Side::Friendly,
                    self.settings.mode(),
                )
            })
            .collect()
    }

    /// The connection holding `plane`'s slot, other than `except`.
    fn holder(&self, plane: PlaneId, except: ConnectionId) -> Option<ConnectionId> {
        self.peers
            .iter()
            .find(|(id, peer)| {
                **id != except
                    && peer.lobby.slot == Some(plane)
                    && !matches!(peer.stage, Stage::Closing { .. })
            })
            .map(|(id, _)| *id)
    }

    /// Whether the player is flying or about to (taking, seated, leaving).
    fn in_flight(peer: &Peer) -> bool {
        matches!(
            peer.stage,
            Stage::Taking { .. } | Stage::Seated | Stage::Leaving
        )
    }

    /// The player's slot request.
    fn slot(&mut self, connection: ConnectionId, request: SlotRequest) -> Result<(), String> {
        let peer = self.peers.get(&connection).ok_or("")?;
        if Self::in_flight(peer) {
            return Err("Leave your aircraft before you change your slot.".into());
        }
        if let Some(reason) = &peer.lobby.unable {
            return Err(format!("Your game cannot play this mission: {reason}"));
        }
        let held = peer.lobby.slot;
        let wanted = match request {
            // The lobby pass: under Autobalance a player keeps the side the
            // host gave it, so it keeps a slot (slice A1).
            SlotRequest::Leave if held.is_some() && self.settings.balanced() => {
                return Err(sides::SIDES_BALANCED.into());
            }
            SlotRequest::Leave => None,
            SlotRequest::Take(plane) => {
                let Some(slot) = self.slots().into_iter().find(|slot| slot.id == plane) else {
                    return Err(format!("Plane {plane} is not a slot players may take."));
                };
                // The lobby pass: Autobalance's side (slice A1).
                if let Some(why) = self.balance_side_refusal(connection, slot.wing.side) {
                    return Err(why);
                }
                if let Some(other) = self.holder(PlaneId(plane), connection) {
                    return Err(format!(
                        "{} holds plane {plane}.",
                        self.peers[&other].callsign
                    ));
                }
                // Stage F phase 2: the King's slot locks.
                if let Some(why) = self.lock_refusal(connection, plane) {
                    return Err(why);
                }
                Some(PlaneId(plane))
            }
            // The lobby pass (slice W0): a side's first free slot.
            SlotRequest::Side(side) => Some(self.side_slot(connection, side, held)?),
            SlotRequest::Any => match held {
                Some(plane) => Some(plane),
                // The lobby pass: the side Autobalance gives (slice A1).
                None if self.settings.balanced() => Some(self.balance_any(connection)?),
                None => Some(
                    self.slots()
                        .into_iter()
                        .map(|slot| PlaneId(slot.id))
                        .find(|plane| {
                            self.holder(*plane, connection).is_none()
                                && self.lock_refusal(connection, plane.0).is_none()
                        })
                        .ok_or("No slot is free.")?,
                ),
            },
        };
        if wanted == held {
            return Ok(());
        }
        if matches!(self.life, Life::Lobby)
            && let Some(old) = held
        {
            self.spec.plane_aircraft.remove(&old.0);
        }
        let peer = self.peers.get_mut(&connection).ok_or("")?;
        peer.lobby.release();
        peer.lobby.slot = wanted;
        let callsign = peer.callsign.clone();
        self.lobby_log(callsign, LobbyEvent::Slot(wanted.map(|p| p.0)));
        self.lobby_dirty = true;
        Ok(())
    }

    /// The loadout for the player's slot: checked by the Load Ordnance
    /// page's rules for that plane's aircraft, kept for the next start.
    fn choose_loadout(
        &mut self,
        connection: ConnectionId,
        plane: u32,
        aircraft: Option<tore_formats::aircraft::AircraftId>,
        loadout: Option<tore_world::mission::LoadoutSpec>,
    ) -> Result<(), String> {
        let old = self.spec.plane_aircraft.clone();
        if let Some(aircraft) = aircraft {
            let slot = self
                .spec
                .open_planes()
                .into_iter()
                .find(|p| p.id == plane)
                .ok_or("No such slot.")?;
            if !self.spec.player_choice[MissionSpec::wing_index(slot.wing)] {
                return Err("This slot has a fixed aircraft.".into());
            }
            let mut candidate = self.spec.clone();
            candidate.plane_aircraft.insert(plane, aircraft);
            if let Some(why) = self.mission_gap_refusal(&candidate) {
                return Err(why);
            }
            candidate.validate().map_err(|e| e.to_string())?;
            self.spec.plane_aircraft = candidate.plane_aircraft;
        }
        if let Err(error) = self.loadout(connection, plane, loadout) {
            self.spec.plane_aircraft = old;
            return Err(error);
        }
        if matches!(self.life, Life::Flying) {
            let selected = self.spec.plane_aircraft.get(&plane).copied();
            self.spec.plane_aircraft = old;
            if let Some(aircraft) = selected {
                let head = self.world.lineage_head(PlaneId(plane));
                self.revival.choices.insert(head, aircraft);
            }
        }
        self.lobby_dirty = true;
        Ok(())
    }

    fn loadout(
        &mut self,
        connection: ConnectionId,
        plane: u32,
        loadout: Option<tore_world::mission::LoadoutSpec>,
    ) -> Result<(), String> {
        let late_choice =
            matches!(self.life, Life::Flying)
                && self
                    .peers
                    .get(&connection)
                    .is_some_and(|p| p.stage == Stage::Lobby)
                && self.spec.open_planes().iter().any(|p| {
                    p.id == plane && self.spec.player_choice[MissionSpec::wing_index(p.wing)]
                });
        if !matches!(self.life, Life::Lobby) && !late_choice {
            return Err("Loadouts are chosen in the lobby, before the mission flies.".into());
        }
        let peer = self.peers.get(&connection).ok_or("")?;
        if peer.lobby.slot != Some(PlaneId(plane)) {
            return Err(format!("You do not hold plane {plane}'s slot."));
        }
        if let Some(load) = &loadout {
            let aircraft = self
                .spec
                .open_planes()
                .into_iter()
                .find(|p| p.id == plane)
                .ok_or("There is no such plane.")?
                .aircraft;
            let kind = tore_world::aircraft_type::AircraftType::load(&*self.resources, aircraft)
                .map_err(|error| error.to_string())?;
            // Under the King's loadout rule, which the mission carries.
            load.check_in(&kind.profile, &*self.resources, &self.spec)
                .map_err(|error| error.to_string())?;
            // Stage L: a weapon not everyone has.
            if let Some(why) = self.loadout_gap_refusal(load) {
                return Err(why);
            }
        }
        let own = loadout.is_some();
        let peer = self.peers.get_mut(&connection).ok_or("")?;
        if peer.lobby.loadout == loadout {
            return Ok(());
        }
        peer.lobby.loadout = loadout;
        let callsign = peer.callsign.clone();
        self.lobby_log(callsign, LobbyEvent::Loadout { plane, own });
        self.lobby_dirty = true;
        Ok(())
    }

    /// Ready or not. In the lobby the start rule may then start the
    /// mission; in flight, a player who gets ready takes its slot's plane.
    fn set_ready(&mut self, connection: ConnectionId, ready: bool) -> Result<(), String> {
        let peer = self.peers.get(&connection).ok_or("")?;
        if let Some(reason) = &peer.lobby.unable {
            return Err(format!("Your game cannot play this mission: {reason}"));
        }
        if ready && peer.lobby.slot.is_none() {
            return Err("Take a slot first.".into());
        }
        if Self::in_flight(peer) {
            return if ready {
                Ok(())
            } else {
                Err("You are flying.".into())
            };
        }
        match self.life {
            Life::Lobby | Life::Flying => {}
            Life::Ended { .. } | Life::Stopped => {
                return Err("The mission has ended.".into());
            }
        }
        let slot = peer.lobby.slot;
        let peer = self.peers.get_mut(&connection).ok_or("")?;
        if peer.lobby.ready != ready {
            peer.lobby.ready = ready;
            let callsign = peer.callsign.clone();
            self.lobby_log(callsign, LobbyEvent::Ready(ready));
            self.lobby_dirty = true;
        }
        if ready {
            match self.life {
                Life::Lobby if self.start_rule() == StartMode::FirstPlayer => {
                    match self.start_block() {
                        // Not an error: the player is ready, and the
                        // mission starts when the others are.
                        Some(StartBlock::Wait(_)) => self.note_start_wait(false),
                        Some(StartBlock::Refused(why)) => return Err(why),
                        None => self.start_flying()?,
                    }
                }
                Life::Flying => {
                    if let Some(plane) = slot {
                        self.take_now(connection, Some(plane.0));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Take a plane (stage D's Ready): in the lobby, hold its slot (or the
    /// first free one, or the one held) and mark ready; in flight, fly it
    /// now. A refusal is a Seat refused, which a game with no lobby screen
    /// answers by asking for any plane.
    fn take_plane(&mut self, connection: ConnectionId, wanted: Option<u32>) {
        let Some(peer) = self.peers.get(&connection) else {
            return;
        };
        if Self::in_flight(peer) {
            return;
        }
        if let Some(reason) = peer.lobby.unable.clone() {
            self.refuse_seat(
                connection,
                format!("Your game cannot play this mission: {reason}"),
            );
            return;
        }
        match self.life {
            Life::Lobby => {
                let request = match wanted {
                    Some(plane) => SlotRequest::Take(plane),
                    None => SlotRequest::Any,
                };
                let taken = self
                    .slot(connection, request)
                    .and_then(|()| self.set_ready(connection, true));
                if let Err(reason) = taken {
                    self.refuse_seat(connection, reason);
                }
            }
            Life::Flying => self.take_now(connection, wanted),
            Life::Ended { .. } | Life::Stopped => {
                self.refuse_seat(connection, "The mission has ended.".into());
            }
        }
    }

    /// A player in the lobby takes a plane of the flying mission at the
    /// next tick: `wanted`, or its slot's, or the first free friendly one
    /// (Friendly Wing 1's lead first), then any.
    fn take_now(&mut self, connection: ConnectionId, wanted: Option<u32>) {
        if let Some(why) = self.runtime_content_refusal() {
            self.refuse_seat(connection, why);
            return;
        }

        let Some(peer) = self.peers.get(&connection) else {
            return;
        };
        if peer.stage != Stage::Lobby {
            return;
        }
        // Stage K: a returned player's Join takes the plane kept for it.
        if let Some(result) = self.rejoin_take(connection) {
            if let Err(reason) = result {
                self.refuse_seat(connection, reason);
            }
            return;
        }
        // Stage F phase 2: Join after a loss flies again by the revival rules.
        if let Some(result) = self.revive_join(connection) {
            if let Err(reason) = result {
                self.refuse_seat(connection, reason);
            }
            return;
        }
        let Some(peer) = self.peers.get(&connection) else {
            return;
        };
        let held = peer.lobby.slot;
        // Stage F phase 2: join in progress off refuses every plane alike.
        if let Some(why) = self.new_pilot_refusal() {
            self.refuse_seat(connection, why);
            return;
        }
        let Some(seat) = self.free_seat() else {
            self.refuse_seat(connection, "No seat is free.".into());
            return;
        };
        let wanted = wanted.or(held.map(|p| p.0));
        if let Some(plane) = wanted.map(PlaneId)
            && let Some(slot) = self
                .spec
                .open_planes()
                .into_iter()
                .find(|p| p.id == self.root_of(plane).0)
            && self.spec.player_choice[MissionSpec::wing_index(slot.wing)]
        {
            let head = self.world.lineage_head(self.root_of(plane));
            if let Some(why) = self.refusal(seat, head, connection) {
                self.refuse_seat(connection, why);
            } else {
                self.join_empty(connection, seat, head);
            }
            return;
        }
        if !self.settings.ai()
            && let Some(plane) = wanted.map(PlaneId)
        {
            let head = self.world.lineage_head(self.root_of(plane));
            if self.world.head_lost(head) {
                if let Some(why) = self.refusal(seat, head, connection) {
                    self.refuse_seat(connection, why);
                } else {
                    self.join_empty(connection, seat, head);
                }
                return;
            }
        }
        let chosen = match wanted {
            // The lobby pass's follow-up F1: a lost plane's lineage, its
            // newest plane or a wait for its respawn.
            Some(plane) => match self.lineage_take(PlaneId(plane)) {
                lineage_take::LineageTake::Wait(words) => {
                    let root = self.root_of(PlaneId(plane));
                    match self.lineage_wait_refusal(connection, root) {
                        Some(reason) => Err(reason),
                        None => {
                            self.await_respawn(connection, root, words);
                            return;
                        }
                    }
                }
                lineage_take::LineageTake::Head(head) => {
                    match self.refusal(seat, head, connection) {
                        None => Ok(head),
                        Some(reason) => Err(reason),
                    }
                }
                lineage_take::LineageTake::AsAsked => {
                    match self.refusal(seat, PlaneId(plane), connection) {
                        None => Ok(PlaneId(plane)),
                        Some(reason) => Err(reason),
                    }
                }
            },
            None => {
                // Friendly Wing 1's lead first: the roster is in wing order.
                let planes: Vec<(PlaneId, bool)> = self
                    .world
                    .roster
                    .planes()
                    .iter()
                    .map(|p| (p.id, p.slot.wing.side == Side::Friendly))
                    .collect();
                planes
                    .iter()
                    .filter(|(_, friendly)| *friendly)
                    .chain(planes.iter().filter(|(_, friendly)| !*friendly))
                    .map(|(id, _)| *id)
                    .find(|plane| self.refusal(seat, *plane, connection).is_none())
                    .ok_or_else(|| "No plane is free.".to_string())
            }
        };
        match chosen {
            Ok(plane) => {
                // The slot is the lineage's root (follow-up F1).
                let root = self.root_of(plane);
                let is_slot = self.slots().iter().any(|slot| slot.id == root.0);
                self.revival.awaiting.remove(&connection);
                if let Some(peer) = self.peers.get_mut(&connection) {
                    peer.stage = Stage::Taking { seat, plane };
                    peer.lobby.ready = true;
                    if peer.lobby.slot != Some(root) && is_slot {
                        peer.lobby.release();
                        peer.lobby.slot = Some(root);
                        peer.lobby.ready = true;
                    }
                }
                self.lobby_dirty = true;
            }
            Err(reason) => self.refuse_seat(connection, reason),
        }
    }

    fn refuse_seat(&mut self, connection: ConnectionId, reason: String) {
        self.send(connection, &Message::SeatRefused(reason.clone()));
        let callsign = self
            .peers
            .get(&connection)
            .map(|p| p.callsign.clone())
            .unwrap_or_default();
        let tick = self.world.tick();
        self.log(HostLog::SeatRefused {
            tick,
            callsign,
            reason,
        });
    }

    /// The player's import cannot play the mission numbered `refused.mission`:
    /// it stays, marked unable, its slot free; a player the host was seating
    /// goes back to the lobby.
    fn content_refused(
        &mut self,
        connection: ConnectionId,
        refused: crate::wire::messages::ContentRefused,
    ) {
        let Some((callsign, stage, seat)) = self
            .peers
            .get(&connection)
            .map(|peer| (peer.callsign.clone(), peer.stage, peer.seat))
        else {
            return;
        };
        let tick = self.world.tick();
        self.log(HostLog::ContentRefused {
            tick,
            callsign: callsign.clone(),
            names: refused.names.clone(),
        });
        if refused.mission != self.number {
            return;
        }
        // Stage L: in the third person, about the item, for every player
        // (the player's own game keeps its own words).
        let reason = self.unable_text(connection, &refused.names, refused.flight);
        if stage == Stage::Seated
            && let Some(seat) = seat
        {
            self.gives.push((seat, callsign.clone()));
        }
        if let Some(peer) = self.peers.get_mut(&connection) {
            peer.lobby.release();
            peer.lobby.unable = Some(reason.clone());
            // Only a flight's build failed: the player may try the next.
            peer.lobby.unable_flight = refused.flight;
            // A take not yet made is cancelled; a plane already flown goes
            // back to the AI at the next tick, and then the player is back
            // in the lobby.
            match stage {
                Stage::Taking { .. } => peer.stage = Stage::Lobby,
                Stage::Seated => peer.stage = Stage::Leaving,
                _ => {}
            }
        }
        self.lobby_log(callsign, LobbyEvent::Unable(reason));
        self.lobby_dirty = true;
    }

    /// The King's new mission: built to check it, kept, slots that no
    /// longer exist freed, every ready flag cleared, and sent to everyone.
    fn change_mission(&mut self, connection: ConnectionId, text: &str) -> Result<(), String> {
        if !matches!(self.life, Life::Lobby) {
            return Err("The mission can change only in the lobby.".into());
        }
        let spec = MissionSpec::from_text(text).map_err(|error| error.to_string())?;
        // The King's settings decide what the mission carries of them
        // (friendly fire, the loadout rule), whatever the text says.
        let spec = king::with_settings(spec, &self.settings);
        let mut spec = with_surface_seed(spec, self.session_id, self.number.wrapping_add(1));
        if !spec.plane_loadouts.is_empty() {
            return Err(
                "A lobby's mission carries no loadouts: each player arms their own.".into(),
            );
        }
        // Stage L: an aircraft or a theater not everyone has.
        if let Some(why) = self.mission_gap_refusal(&spec) {
            return Err(why);
        }
        let (world, manifest) = self.build_seeded(&mut spec).map_err(|error| match error {
            HostError::Mission(text) => text,
            other => other.to_string(),
        })?;
        let old = self.spec.open_planes();
        self.spec = spec;
        self.spec_text = self.spec.to_text();
        self.world = world;
        self.manifest = manifest;
        self.tracker = Tracker::new(&self.world);
        self.revival = revive::Revivals::default();
        self.gives.clear();
        self.number = self.number.wrapping_add(1);
        self.king_mission_changed(&old);
        let slots = self.slots();
        for peer in self.peers.values_mut() {
            peer.lobby.ready = false;
            peer.lobby.unable = None;
            if let Some(plane) = peer.lobby.slot {
                match slots.iter().find(|slot| slot.id == plane.0) {
                    None => peer.lobby.release(),
                    Some(slot) => {
                        // A plane now flying another aircraft loses the
                        // loadout chosen for the old one.
                        let before = old.iter().find(|p| p.id == plane.0).map(|p| p.aircraft);
                        if before != Some(slot.aircraft) {
                            peer.lobby.loadout = None;
                        }
                    }
                }
            }
        }
        // A kept loadout the new mission refuses (Guns only now, say) goes
        // back to the standard load, and its player is told why.
        let mut dropped = Vec::new();
        for (id, peer) in &self.peers {
            let (Some(plane), Some(load)) = (peer.lobby.slot, &peer.lobby.loadout) else {
                continue;
            };
            let aircraft = slots
                .iter()
                .find(|slot| slot.id == plane.0)
                .map(|s| s.aircraft);
            let kind = aircraft.and_then(|aircraft| {
                self.world
                    .combat
                    .dummy_types()
                    .iter()
                    .find(|kind| kind.profile.id == aircraft)
            });
            let refused = match kind {
                Some(kind) => load
                    .check_in(&kind.profile, &*self.resources, &self.spec)
                    .err()
                    .map(|error| error.to_string()),
                None => Some("the mission holds no such aircraft".into()),
            };
            if let Some(reason) = refused {
                dropped.push((*id, plane.0, reason));
            }
        }
        for (id, plane, reason) in dropped {
            if let Some(peer) = self.peers.get_mut(&id) {
                peer.lobby.loadout = None;
            }
            self.send(
                id,
                &Message::Notice(format!(
                    "Your loadout for plane {plane} does not fit the new mission ({reason}); you have the standard load."
                )),
            );
        }
        self.lobby_dirty = true;
        let mission = self.mission_message();
        let to: Vec<ConnectionId> = self
            .peers
            .iter()
            .filter(|(_, peer)| !matches!(peer.stage, Stage::Closing { .. }))
            .map(|(id, _)| *id)
            .collect();
        for id in to {
            self.send(id, &mission);
        }
        let callsign = self
            .peers
            .get(&connection)
            .map(|p| p.callsign.clone())
            .unwrap_or_default();
        let summary = self.spec.summary();
        let number = self.number;
        self.lobby_log(callsign, LobbyEvent::MissionChanged { number, summary });
        self.roster_dirty = false;
        let roster = Message::Roster(self.roster());
        let to: Vec<ConnectionId> = self.peers.keys().copied().collect();
        for id in to {
            self.send(id, &roster);
        }
        Ok(())
    }

    /// The King's Start: accepted in the lobby when every player holding a
    /// slot is ready, and at least one holds one.
    fn king_start(&mut self) -> Result<(), String> {
        if !matches!(self.life, Life::Lobby) {
            return Err("The mission is not in the lobby.".into());
        }
        let holders: Vec<&Peer> = self
            .peers
            .values()
            .filter(|peer| {
                peer.lobby.slot.is_some() && !matches!(peer.stage, Stage::Closing { .. })
            })
            .collect();
        if holders.is_empty() {
            return Err("Nobody holds a slot.".into());
        }
        let waiting: Vec<&str> = holders
            .iter()
            .filter(|peer| !peer.lobby.ready)
            .map(|peer| peer.callsign.as_str())
            .collect();
        if !waiting.is_empty() {
            return Err(format!("Not ready: {}.", waiting.join(", ")));
        }
        self.start_flying()
    }

    /// How long a force start counts down (John, 2026-10-16).
    const FORCE_FOR: Duration = Duration::from_secs(10);

    /// The King starts a 10 second force start, or cancels one that is
    /// already running. The host's clock owns the deadline.
    fn king_force(&mut self) -> Result<(), String> {
        if !matches!(self.life, Life::Lobby) {
            return Err("The mission is not in the lobby.".into());
        }
        if self.force_deadline.is_some() {
            self.force_deadline = None;
            self.note_force_second();
            self.announce("The King cancelled the force start.");
            self.note("Force start cancelled.");
            return Ok(());
        }
        if let Some(why) = self.force_block() {
            return Err(why);
        }
        self.force_deadline = Some(self.now.saturating_add(Self::FORCE_FOR));
        self.note_force_second();
        self.announce("The King is force starting in 10 seconds.");
        self.note("Force start: the King is starting in 10 seconds.");
        Ok(())
    }

    /// Advances the countdown. Everyone ready starts at once. At zero the
    /// mission starts and slot holders who are still not ready fly with the
    /// loadout they have. Losing the minimum or the last ready player
    /// cancels the countdown, with AI either on or off.
    fn tick_force(&mut self) {
        if self.force_deadline.is_none() {
            return;
        }
        if !matches!(self.life, Life::Lobby) {
            self.force_deadline = None;
            self.note_force_second();
            return;
        }
        if let Some(why) = self.force_block() {
            self.force_deadline = None;
            self.note_force_second();
            self.announce("The force start was cancelled.");
            self.note(format!("Force start cancelled: {why}"));
            return;
        }
        let due = self.now >= self.force_deadline.unwrap_or(Duration::MAX);
        if !self.pending_unready() && self.start_block().is_none() {
            match self.start_flying() {
                Ok(()) => {
                    self.note("Force start: everyone was ready, so the mission started.");
                }
                Err(why) => {
                    // A build failure is already a fault. Drop the countdown
                    // so the next update does not log it again.
                    self.force_deadline = None;
                    self.note_force_second();
                    self.announce("The force start was cancelled.");
                    self.note(format!("Force start cancelled: {why}"));
                }
            }
            return;
        }
        if due {
            if let Err(why) = self.start_flying_in(true) {
                self.force_deadline = None;
                self.note_force_second();
                self.announce("The force start was cancelled.");
                self.note(format!("Force start cancelled: {why}"));
            }
            return;
        }
        self.note_force_second();
    }

    /// Why a force start cannot begin, or cannot keep counting.
    fn force_block(&self) -> Option<String> {
        if !matches!(self.life, Life::Lobby) {
            return Some("The mission is not in the lobby.".into());
        }
        if self.min_missing() > 0 {
            return self
                .start_wait_reason()
                .or_else(|| Some("The lobby is still waiting for players.".into()));
        }
        let ready = self
            .peers
            .values()
            .filter(|peer| Self::ready_holder(peer))
            .count();
        if ready == 0 {
            return Some("Nobody is ready.".into());
        }
        if !self.pending_unready() && self.force_deadline.is_none() {
            // Fly, not Force Start: everyone who can fly is ready.
            return Some("Everyone is ready.".into());
        }
        None
    }

    /// A slot holder who is not ready and whose game can play.
    fn pending_unready(&self) -> bool {
        self.peers.values().any(|peer| {
            peer.lobby.slot.is_some()
                && !peer.lobby.ready
                && peer.lobby.unable.is_none()
                && !matches!(peer.stage, Stage::Closing { .. })
        })
    }

    /// The seconds left, 1 to 10, while a countdown runs.
    fn force_seconds(&self) -> Option<u8> {
        let deadline = self.force_deadline?;
        let left = deadline.saturating_sub(self.now);
        if left.is_zero() {
            return Some(1);
        }
        let secs = left.as_secs() + u64::from(left.subsec_nanos() != 0);
        Some(secs.clamp(1, 10) as u8)
    }

    /// Marks the lobby stale when the seconds the players are shown change.
    fn note_force_second(&mut self) {
        let shown = self.force_seconds();
        if shown != self.force_shown {
            self.force_shown = shown;
            self.lobby_dirty = true;
        }
    }

    fn note(&mut self, text: impl Into<String>) {
        let tick = self.world.tick();
        self.log(HostLog::Note {
            tick,
            text: text.into(),
        });
    }

    /// Names the slot holders a force start did not find ready, and anyone
    /// whose game cannot play (they stay in the lobby).
    fn note_forced(&mut self) {
        let mut unready = Vec::new();
        let mut stayed = Vec::new();
        for peer in self.peers.values() {
            if peer.lobby.slot.is_none() || matches!(peer.stage, Stage::Closing { .. }) {
                continue;
            }
            if peer.lobby.unable.is_some() {
                stayed.push(peer.callsign.clone());
            } else if !peer.lobby.ready {
                unready.push(peer.callsign.clone());
            }
        }
        let names = |list: &[String]| match list {
            [one] => one.clone(),
            [a, b] => format!("{a} and {b}"),
            many => {
                let last = many.last().map(String::as_str).unwrap_or("");
                let rest = many[..many.len() - 1].join(", ");
                format!("{rest}, and {last}")
            }
        };
        if !unready.is_empty() {
            let verb = if unready.len() == 1 { "was" } else { "were" };
            self.note(format!(
                "Force start: {} {verb} not ready.",
                names(&unready)
            ));
        }
        if !stayed.is_empty() {
            self.note(format!(
                "Force start: {} stayed in the lobby.",
                names(&stayed)
            ));
        }
    }

    /// The player ends its flight: its debrief now, its plane back to the
    /// AI at the next tick, then it is back in the lobby, still connected,
    /// its slot and loadout kept and its ready mark cleared.
    fn leave(&mut self, connection: ConnectionId) {
        self.report_update();
        self.send_flight_report(connection);
        let Some(peer) = self.peers.get(&connection) else {
            return;
        };
        match peer.stage {
            Stage::Seated => {
                let seat = peer.seat.expect("a seated player has a seat");
                let callsign = peer.callsign.clone();
                if let Some(report) = self.player_report(connection) {
                    let message = Message::Debrief(Box::new(report));
                    self.send(connection, &message);
                }
                self.gives.push((seat, callsign));
                if let Some(peer) = self.peers.get_mut(&connection) {
                    peer.stage = Stage::Leaving;
                    peer.lobby.ready = false;
                }
            }
            Stage::Taking { .. } => {
                if let Some(peer) = self.peers.get_mut(&connection) {
                    peer.stage = Stage::Lobby;
                    peer.lobby.ready = false;
                }
            }
            // A player waiting for a lineage's respawn gives up its wait,
            // as a take is given up (the lobby pass's follow-up F1).
            Stage::Lobby => {
                if self.end_wait(connection)
                    && let Some(peer) = self.peers.get_mut(&connection)
                {
                    peer.lobby.ready = false;
                }
            }
            Stage::Leaving | Stage::Closing { .. } => {}
        }
        if !self.settings.ai()
            && let Some(peer) = self.peers.get_mut(&connection)
        {
            peer.lobby.release();
        }
        self.lobby_dirty = true;
    }

    /// Disconnects every departing connection whose last messages are
    /// acknowledged, or whose grace has run out.
    fn close_departed(&mut self, now: Duration) {
        let done: Vec<(ConnectionId, DisconnectReason)> = self
            .peers
            .iter()
            .filter_map(|(id, peer)| match peer.stage {
                Stage::Closing { deadline, reason } => {
                    let acknowledged = self
                        .server
                        .stats(*id)
                        .is_none_or(|stats| stats.messages_queued == 0);
                    (acknowledged || now >= deadline).then_some((*id, reason))
                }
                _ => None,
            })
            .collect();
        for (id, reason) in done {
            self.server.disconnect(id, reason);
        }
    }

    /// The roster message: every plane with its pilot.
    fn roster(&self) -> messages::Roster {
        let callsign = |seat: SeatId| {
            self.peers
                .values()
                .find(|peer| {
                    peer.seat == Some(seat)
                        || matches!(peer.stage, Stage::Taking { seat: s, .. } if s == seat)
                })
                .map(|peer| peer.callsign.clone())
                .or_else(|| self.held_callsign(seat))
                .unwrap_or_default()
        };
        messages::Roster {
            planes: self
                .world
                .roster
                .planes()
                .iter()
                .filter_map(|plane| {
                    Some(RosterPlane {
                        id: plane.id.0,
                        wing: plane.slot.wing,
                        member: plane.slot.member,
                        aircraft: aircraft_of(&self.world, plane.id)?,
                        pilot: match plane.pilot {
                            // A lost plane (phase 2) has no human; the
                            // roster names no pilot for it.
                            Pilot::Ai | Pilot::Lost => RosterPilot::Ai,
                            Pilot::Human(seat) => RosterPilot::Human {
                                seat: seat.0,
                                callsign: callsign(seat),
                            },
                        },
                    })
                })
                .collect(),
        }
    }

    fn broadcast_roster(&mut self) {
        let roster = Message::Roster(self.roster());
        let to: Vec<ConnectionId> = self
            .peers
            .iter()
            .filter(|(_, peer)| !matches!(peer.stage, Stage::Closing { .. }))
            .map(|(id, _)| *id)
            .collect();
        for id in to {
            self.send(id, &roster);
        }
    }

    /// The lobby as the player with lobby id `you` sees it.
    fn lobby(&self, you: u8) -> LobbyState {
        let mut peers: Vec<&Peer> = self
            .peers
            .values()
            .filter(|peer| !matches!(peer.stage, Stage::Closing { .. }))
            .collect();
        peers.sort_by_key(|peer| peer.lobby.order);
        let holder = |plane: u32| {
            peers
                .iter()
                .find(|peer| peer.lobby.slot == Some(PlaneId(plane)))
                .map(|peer| peer.lobby.id)
        };
        let king = peers
            .iter()
            .find(|peer| peer.king)
            .map(|peer| peer.lobby.id);
        LobbyState {
            feedback: self.config.feedback && !self.config.listable && !self.recording_failed,
            name: self.settings.name().to_owned(),
            summary: self.spec.summary(),
            mission: self.number,
            phase: match self.life {
                Life::Lobby => LobbyPhase::Lobby,
                Life::Flying => LobbyPhase::Flying,
                Life::Ended { .. } | Life::Stopped => LobbyPhase::Ended,
            },
            start: match self.start_rule() {
                StartMode::King => StartRule::King,
                StartMode::FirstPlayer => StartRule::FirstReady,
                StartMode::Now => StartRule::Flying,
            },
            king,
            // The house: the player whose machine runs the host.
            host: peers
                .iter()
                .find(|peer| peer.house)
                .map(|peer| peer.lobby.id),
            you,
            players: peers
                .iter()
                .map(|peer| LobbyPlayer {
                    id: peer.lobby.id,
                    callsign: peer.callsign.clone(),
                    slot: peer.lobby.slot.map(|p| p.0),
                    ready: peer.lobby.ready,
                    loadout: peer.lobby.loadout.is_some(),
                    flying: Self::in_flight(peer),
                    observing: peer.watch.as_ref().is_some_and(observe::Watch::started),
                    away: self.away_mark(peer.lobby.id),
                    unable: peer.lobby.unable.clone(),
                    platform: peer.platform,
                    path: peer.path,
                    // Each player's build, as its Content said (stage L).
                    build: peer
                        .content
                        .as_ref()
                        .map_or(messages::Build::Unknown, |content| content.build),
                    // The standbys slice K3 appoints.
                    standby: self.standby_mark(peer.lobby.order),
                })
                .collect(),
            slots: self
                .slots()
                .into_iter()
                .map(|slot| LobbySlot {
                    choice: self.spec.player_choice[MissionSpec::wing_index(slot.wing)]
                        .then_some(self.spec.wings[MissionSpec::wing_index(slot.wing)].aircraft),
                    plane: slot.id,
                    wing: slot.wing,
                    member: slot.member,
                    aircraft: slot.aircraft,
                    holder: holder(slot.id),
                    lock: self.court.lock(slot.id),
                    reserved: self.reserved_for(PlaneId(slot.id)),
                })
                .collect(),
            settings: self.settings.lobby_list(),
            // A dedicated server's `king-mission locked` (a hosted game's
            // config never sets it): the King's Mission... reads only.
            mission_locked: self.config.mission_locked,
            force_seconds: self.force_seconds(),
        }
    }

    /// Sends every player the lobby's state when it changed.
    ///
    /// Changes in quick succession go out together: a player in the lobby
    /// gets at most one state every [`LOBBY_INTERVAL`], a flying one every
    /// [`LOBBY_INTERVAL_FLYING`], since its messages share 256 bytes of each
    /// snapshot packet (agent decision, EF4 review).
    fn send_lobby(&mut self) {
        // The lobby pass: Autobalance seats the players waiting for a side
        // before any lobby state goes out, a joiner before its first (slice
        // A1).
        self.balance_update();
        // Stage L: the gaps, when they changed or a player's content came.
        self.send_gaps();
        self.send_stale_lobbies(false);
    }

    /// Sends the lobby's state to every player that has not got its latest,
    /// at once when `now_whatever_the_interval` (slice R1: a flight's start
    /// sends them first, so a player's game reads the settings in force,
    /// the snapshot rate among them, before its seat arrives).
    fn send_stale_lobbies(&mut self, now_whatever_the_interval: bool) {
        if std::mem::take(&mut self.lobby_dirty) {
            for peer in self.peers.values_mut() {
                peer.lobby_stale = true;
            }
        }
        let now = self.now;
        let to: Vec<(ConnectionId, u8)> = self
            .peers
            .iter()
            .filter(|(_, peer)| {
                let interval = if Self::in_flight(peer) {
                    LOBBY_INTERVAL_FLYING
                } else {
                    LOBBY_INTERVAL
                };
                peer.lobby_stale
                    && !matches!(peer.stage, Stage::Closing { .. })
                    && (now_whatever_the_interval
                        || peer
                            .lobby_sent
                            .is_none_or(|at| now.saturating_sub(at) >= interval))
            })
            .map(|(id, peer)| (*id, peer.lobby.id))
            .collect();
        // The lobby is built once and differs only in whom it is for: a
        // send changes nothing it is built from.
        let mut lobby: Option<Box<LobbyState>> = None;
        for (id, you) in to {
            let state = match lobby.take() {
                Some(mut state) => {
                    state.you = you;
                    state
                }
                None => Box::new(self.lobby(you)),
            };
            let message = Message::Lobby(state);
            self.send(id, &message);
            let Message::Lobby(state) = message else {
                unreachable!("the lobby message")
            };
            lobby = Some(state);
            if let Some(peer) = self.peers.get_mut(&id) {
                peer.lobby_stale = false;
                peer.lobby_sent = Some(now);
            }
        }
    }

    // ----- The clock and the tick ----------------------------------------

    /// Whether `peer` counts as a ready player: it holds a slot, is ready,
    /// can play the mission and is not leaving. All start rules count this way.
    fn ready_holder(peer: &Peer) -> bool {
        peer.lobby.slot.is_some()
            && peer.lobby.ready
            && peer.lobby.unable.is_none()
            && !matches!(peer.stage, Stage::Closing { .. })
    }

    /// The minimum players the lobby waits for: the `min-players` setting,
    /// never more than the host can seat (a player limit the King lowered
    /// or closed slots cannot make the mission wait for ever).
    pub fn min_players_needed(&self) -> usize {
        self.settings
            .min_players()
            .max(usize::from(!self.settings.ai()))
            .min(self.capacity())
    }

    /// How many more ready players the minimum needs; 0 when met.
    fn min_missing(&self) -> usize {
        let ready = self
            .peers
            .values()
            .filter(|p| Self::ready_holder(p))
            .count();
        self.min_players_needed().saturating_sub(ready)
    }

    /// Why the mission is not starting yet: fewer ready slot holders than
    /// the minimum. Empty slots do not add a separate gate when AI is off.
    pub fn start_wait_reason(&self) -> Option<String> {
        crate::settings::start_wait_text(self.min_missing(), self.min_players_needed())
    }

    /// What stops the mission starting now, if anything.
    fn start_block(&self) -> Option<StartBlock> {
        if !self.spec.plane_aircraft.is_empty()
            && let Some(why) = self.mission_gap_refusal(&self.spec)
        {
            return Some(StartBlock::Refused(why));
        }
        self.start_wait_reason().map(StartBlock::Wait)
    }

    /// Logs why the lobby waits to start, once for each reason (`fresh`
    /// for a lobby just opened). A wait is not a fault: nothing went
    /// wrong, the server is waiting for players.
    fn note_start_wait(&mut self, fresh: bool) {
        if fresh {
            self.wait_logged = None;
        }
        let Some(text) = self.start_wait_reason() else {
            self.wait_logged = None;
            return;
        };
        // A fresh lobby that only waits for its first player says nothing
        // more than the start rule's own line.
        if fresh && self.settings.ai() && self.min_players_needed() <= 1 {
            return;
        }
        if self.wait_logged.as_deref() != Some(text.as_str()) {
            self.wait_logged = Some(text.clone());
            let tick = self.world.tick();
            self.log(HostLog::Note { tick, text });
        }
    }

    /// The mission starts flying: built again with the loadouts of the held
    /// slots, which every player is sent to build its copy again with, and
    /// every ready player holding a slot takes its plane at the first tick.
    /// The minimum cannot be bypassed, even from the console.
    fn start_flying(&mut self) -> Result<(), String> {
        self.start_flying_in(false)
    }

    /// Starts the mission. `force` seats every slot holder whose game can
    /// play, ready or not, and still refuses a minimum that is not met.
    fn start_flying_in(&mut self, force: bool) -> Result<(), String> {
        if !force {
            match self.start_block() {
                Some(StartBlock::Refused(why)) => return Err(why),
                Some(StartBlock::Wait(why)) => {
                    self.note_start_wait(false);
                    return Err(why);
                }
                None => {}
            }
        } else {
            if !self.spec.plane_aircraft.is_empty()
                && let Some(why) = self.mission_gap_refusal(&self.spec)
            {
                return Err(why);
            }
            if self.min_missing() > 0 {
                let why = self
                    .start_wait_reason()
                    .unwrap_or_else(|| "The lobby is still waiting for players.".into());
                self.note_start_wait(false);
                return Err(why);
            }
            self.note_forced();
        }
        let lobby_manifest = self.manifest.clone();
        let mut spec = self.spec.clone();
        for peer in self.peers.values() {
            if let (Some(plane), Some(load)) = (peer.lobby.slot, &peer.lobby.loadout)
                && !matches!(peer.stage, Stage::Closing { .. })
            {
                spec.plane_loadouts.insert(plane.0, load.clone());
            }
        }
        // AI members follow the first human slot in their wing. Explicit
        // mission loads and every human slot keep their own choices.
        let held: std::collections::BTreeSet<_> = self
            .peers
            .values()
            .filter(|peer| !matches!(peer.stage, Stage::Closing { .. }))
            .filter_map(|peer| peer.lobby.slot.map(|p| p.0))
            .collect();
        let planes = spec.open_planes();
        for plane in &planes {
            if held.contains(&plane.id) || self.spec.plane_loadouts.contains_key(&plane.id) {
                continue;
            }
            if let Some(leader) = planes
                .iter()
                .filter(|p| p.wing == plane.wing && held.contains(&p.id))
                .min_by_key(|p| p.member)
                && leader.aircraft == plane.aircraft
                && let Some(load) = spec.plane_loadouts.get(&leader.id).cloned()
            {
                spec.plane_loadouts.insert(plane.id, load);
            }
        }
        if !spec.plane_aircraft.is_empty()
            || !spec.plane_loadouts.is_empty()
            || self.world.tick() != 0
            || self.ticks_run != 0
        {
            match self.build(&spec) {
                Ok((world, manifest)) => {
                    self.world = world;
                    self.manifest = manifest;
                    self.tracker = Tracker::new(&self.world);
                }
                Err(error) => {
                    // Every loadout was checked when it was chosen, so this
                    // is a fault: the lobby stays, and the King is told.
                    let text = format!("The mission could not be built to fly: {error}");
                    let tick = self.world.tick();
                    self.log(HostLog::Fault {
                        tick,
                        text: text.clone(),
                    });
                    return Err(text);
                }
            }
        }
        // The same mission, with the loadouts: its number stays, so a
        // request made just before the start still counts.
        self.spec_text = spec.to_text();
        // Every player reads the settings now in force before its seat comes:
        // a client takes the snapshot rate from them (slice R1).
        self.send_stale_lobbies(true);
        // Stage K: the journal starts from the flight's world at tick 0.
        self.journal_flight();
        self.gives.clear();
        self.ever_seated = false;
        self.empty_since = None;
        self.life = Life::Flying;
        self.force_deadline = None;
        self.force_shown = None;
        self.origin = None;
        self.ticks_run = 0;
        // Stage F phase 2: fresh scores, and the world records score facts;
        // nobody has a side yet for lock sides; revivals start afresh.
        self.score_start();
        self.king_mission_start();
        self.revive_start();
        // The players have the lobby's mission already: the loadouts are
        // all they need to build the flight's (a joiner in flight gets the
        // whole text, loadouts included).
        // With the resources the loadouts read that the lobby's mission did
        // not, for the players to check against their own (EF4 review).
        let added = Manifest {
            entries: self
                .manifest
                .entries
                .iter()
                .filter(|entry| !lobby_manifest.entries.contains(entry))
                .cloned()
                .collect(),
        };
        let loadouts = Message::FlightLoadouts(crate::wire::messages::FlightLoadouts {
            aircraft: spec.plane_aircraft.iter().map(|(p, a)| (*p, *a)).collect(),
            loadouts: spec
                .plane_loadouts
                .iter()
                .map(|(plane, load)| (*plane, load.clone()))
                .collect(),
            manifest: added,
        });
        let ids: Vec<ConnectionId> = self
            .peers
            .iter()
            .filter(|(_, peer)| !matches!(peer.stage, Stage::Closing { .. }))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.send(id, &loadouts);
            let peer = &self.peers[&id];
            if peer.stage == Stage::Lobby
                && peer.lobby.unable.is_none()
                && (peer.lobby.ready || force)
                && let Some(plane) = peer.lobby.slot
            {
                match self.free_seat() {
                    Some(seat) => {
                        if let Some(peer) = self.peers.get_mut(&id) {
                            peer.stage = Stage::Taking { seat, plane };
                        }
                    }
                    None => self.refuse_seat(id, "No seat is free.".into()),
                }
            }
        }
        self.lobby_dirty = true;
        let tick = self.world.tick();
        self.log(HostLog::MissionStarted { tick });
        Ok(())
    }

    /// Runs the ticks the clock owes at `now`.
    fn fly(&mut self, now: Duration) {
        let origin = *self.origin.get_or_insert(now);
        let owed = whole_ticks(now.saturating_sub(origin)) + 1;
        let behind = owed.saturating_sub(self.ticks_run);
        let run = behind.min(MAX_CATCH_UP_TICKS);
        let mut worst = Duration::ZERO;
        for _ in 0..run {
            if !matches!(self.life, Life::Flying) {
                return;
            }
            self.ticks_run += 1;
            let started = Instant::now();
            self.tick(now);
            worst = worst.max(started.elapsed());
        }
        if run > 0 {
            // Slice S9: the shared workers fan out only while the host is
            // slow or late (a tick over budget, or more than one tick owed).
            tore_workers::observe(tore_workers::Load {
                compression: 1.,
                tick_cost: worst,
                late: behind > 1,
            });
        }
        if behind > MAX_CATCH_UP_TICKS {
            let dropped = behind - MAX_CATCH_UP_TICKS;
            // The clock starts again from here: the backlog is not run.
            self.origin = Some(origin + ticks_time(dropped));
            self.overloads += 1;
            let tick = self.world.tick();
            self.log(HostLog::Overloaded {
                tick,
                ticks_behind: dropped,
            });
        }
    }

    /// One tick: handoffs, inputs, the step, the sorting, the snapshots.
    fn tick(&mut self, now: Duration) {
        let started = Instant::now();
        let tick = self.world.tick();
        let mut commands = Vec::new();
        if !self.settings.ai() && tick == 0 {
            for plane in self.world.roster.planes() {
                let held = self.peers.values().any(
                    |peer| matches!(peer.stage, Stage::Taking { plane: p, .. } if p == plane.id),
                );
                if !held {
                    commands.push(MissionCommand::Despawn { plane: plane.id });
                }
            }
        }

        // Planes going back to the AI; a plane that cannot (destroyed, or its
        // pilot gone) is held for its player while it stays connected, and
        // abandoned to the mission when it goes (stage F phase 2).
        let mut given = Vec::new();
        for (seat, callsign) in std::mem::take(&mut self.gives) {
            if self
                .world
                .roster
                .seat(seat)
                .is_none_or(|s| s.plane.is_none())
            {
                // Nothing to give back; a leaving player is done all the same.
                given.push(seat);
                continue;
            }
            let choice_departure = self
                .world
                .roster
                .seat(seat)
                .and_then(|s| s.plane)
                .and_then(|p| self.world.roster.plane(p))
                .is_some_and(|p| {
                    // A plane kept for its player (away or dropped) goes to
                    // the AI; only a departure removes it.
                    self.spec.player_choice[MissionSpec::wing_index(p.slot.wing)]
                        && self.rejoin.reserved_plane(p.id).is_none()
                });
            if (!self.settings.ai() || choice_departure) && self.world.can_give_back(seat).is_ok() {
                if let Some(plane) = self.world.roster.seat(seat).and_then(|s| s.plane) {
                    if !self.settings.ai() {
                        self.log(HostLog::Note {
                            tick,
                            text: format!("{callsign} left: plane {} removed (AI is off)", plane.0),
                        });
                    }
                    commands.push(MissionCommand::Despawn { plane });
                }
                given.push(seat);
            } else if self.world.can_give_back(seat).is_ok() {
                commands.push(MissionCommand::GiveBack { seat });
                given.push(seat);
            } else {
                self.revive_keep(seat, &mut commands);
                given.push(seat);
            }
        }

        // A Choice player may leave while already away. Its reserved AI
        // aircraft then returns to the authored default through normal respawn.
        if tick > 0 && self.settings.ai() && self.spec.player_choice.iter().any(|choice| *choice) {
            for (_, head) in self.world.lineage_heads() {
                let Some(plane) = self.world.roster.plane(head) else {
                    continue;
                };
                let wing = MissionSpec::wing_index(plane.slot.wing);
                if self.spec.player_choice[wing]
                    && plane.pilot == Pilot::Ai
                    && !self.world.head_lost(head)
                    && self.world.aircraft_of(head) != Some(self.spec.wings[wing].aircraft)
                    && matches!(self.lineage_holder(head), revive::Holder::Ai)
                    && !self
                        .peers
                        .values()
                        .any(|p| matches!(p.stage, Stage::Taking { plane, .. } if plane == head))
                {
                    commands.push(MissionCommand::Despawn { plane: head });
                }
            }
        }

        // Planes taken this tick.
        let mut takes = Vec::new();
        let taking: Vec<(ConnectionId, SeatId, PlaneId)> = self
            .peers
            .iter()
            .filter_map(|(id, peer)| match peer.stage {
                Stage::Taking { seat, plane } => Some((*id, seat, plane)),
                _ => None,
            })
            .collect();
        for (connection, seat, plane) in taking {
            if self.world.can_take(seat, plane).is_ok() {
                commands.push(MissionCommand::Take { seat, plane });
                takes.push((connection, seat, plane));
            } else {
                if let Some(peer) = self.peers.get_mut(&connection) {
                    peer.stage = Stage::Lobby;
                    peer.lobby.ready = false;
                }
                self.lobby_dirty = true;
                self.refuse_seat(
                    connection,
                    format!("Plane {} is destroyed or has lost its pilot.", plane.0),
                );
            }
        }

        // Stage F phase 2: handoffs for away and returning players, then
        // revivals and abandoned planes.
        self.away_commands(tick, &mut commands);
        self.revive_commands(tick, &mut commands);
        // The lobby pass's follow-up F1: each seated player's callsign.
        self.callsign_commands(&mut commands);

        // The tick as the journal records it (stage K): its mission
        // commands, and every flying seat's input with the number of the
        // last command applied for it.
        let mut journal = Tick::new(tick);
        journal.mission = commands;
        // Stage K: resumed seats' kept inputs reach their buffers (K4).
        self.resume_feed(tick);
        for peer in self.peers.values_mut() {
            if peer.stage != Stage::Seated {
                continue;
            }
            let Some(seat) = peer.seat else { continue };
            let (input, taken) = peer.inputs.take(seat, tick);
            peer.unforeseen |= taken.repeated || taken.command_moved;
            #[cfg(test)]
            if taken.repeated || taken.command_moved {
                self.unforeseen_log
                    .push((tick, seat, Unforeseen::LateInput));
            }
            journal.push_input(input, peer.inputs.applied());
        }
        for &(_, seat, _) in &takes {
            journal.push_input(InputFrame::default().seat_input(seat, tick, &[], None), 0);
        }
        for seat in self.revive_inputs() {
            if !journal.inputs.iter().any(|input| input.seat == seat) {
                journal.push_input(InputFrame::default().seat_input(seat, tick, &[], None), 0);
            }
        }
        // Stage K: the seats of players not yet back fly on (slice K4).
        self.resume_absent_inputs(tick, &mut journal);

        // The host's AI test settings govern the simulation (slice S7b).
        self.world.set_ai_test(self.config.ai_test);
        // One door into the world: the host steps through the journal's
        // driver, which adds the changes made since the last step and drains
        // the world after it, as a standby replays it.
        let mut out = std::mem::take(&mut self.out);
        if !takes.is_empty() || !given.is_empty() || !journal.mission.is_empty() {
            self.report_update();
        }
        let notes = match self.world.step(journal, &mut out) {
            Ok((journal, notes)) => {
                self.record_tick(&journal.inputs);
                self.journal_ticked(journal);
                notes
            }
            Err(error) => {
                self.out = out;
                self.fault(format!("tick {tick}: {error}"));
                return;
            }
        };

        if let Some(recorder) = &mut self.recording {
            recorder.combat_events(&out.events);
        }
        if !given.is_empty() || !takes.is_empty() {
            self.roster_dirty = true;
        }
        // Players whose plane went back are back in the lobby, still
        // connected; the lobby's messages may fill their packets again.
        let mut back = Vec::new();
        for seat in &given {
            for (id, peer) in self.peers.iter_mut() {
                if peer.seat == Some(*seat) && peer.stage == Stage::Leaving {
                    peer.stage = Stage::Lobby;
                    peer.seat = None;
                    peer.plane = None;
                    back.push((*id, peer.callsign.clone()));
                }
            }
        }
        for (id, callsign) in back {
            self.server.set_message_budget(id, tore_net::MAX_DATAGRAM);
            self.lobby_log(callsign, LobbyEvent::BackInLobby);
            self.lobby_dirty = true;
        }
        let _ = now;
        let wide = self.tracker.sort(&self.world, &out, notes, tick);
        for (connection, seat, plane) in takes {
            self.seated(connection, seat, plane, tick, &out);
        }
        // Stage F phase 2: revivals seated, and planes newly lost.
        self.revive_after(tick, &out);
        self.sort_seats(tick, &out, &wide);
        // Stage F phase 2: the tick's score facts.
        self.score_tick(tick, &out);
        self.report_tick(tick);
        if std::mem::take(&mut self.roster_dirty) {
            self.broadcast_roster();
        }
        self.snapshots(tick, now, &out);
        self.observe_tick(tick, now, &out, &wide);
        self.out = out;
        // Stage K: the parts of the session's state this tick changed.
        self.journal_parts();
        let cost = started.elapsed();
        self.costs.ticks += 1;
        self.costs.total += cost;
        self.costs.max = self.costs.max.max(cost);
    }

    /// A take went through at `tick`: the player gets its seat, its plane's
    /// exact state, its loadout, the roster and the destroyed ground objects,
    /// and the mission as it stands is queued for its first snapshot.
    fn seated(
        &mut self,
        connection: ConnectionId,
        seat: SeatId,
        plane: PlaneId,
        tick: u64,
        out: &TickOutput,
    ) {
        // An observer who takes a plane stops watching first; the first
        // plane flown fixes the player's side for lock sides.
        self.observe_seated(connection);
        self.king_seated(connection, plane);
        let Some(cockpit) = self.world.cockpits.iter().find(|c| c.plane == plane) else {
            return;
        };
        let terms = out.terms.iter().find(|(p, _)| *p == plane).map(|(_, t)| *t);
        let exact = ExactState::of(&OwnPlane::of(cockpit), terms.as_ref());
        let Ok(exact_bytes) = exact.encode(None) else {
            self.fault(format!("tick {tick}: plane {} has no exact state", plane.0));
            return;
        };
        let loadout = self
            .world
            .combat
            .state
            .ownship(plane.0)
            .map(|own| carried_loadout(own.configuration(), &own.ammo, cockpit.flight.fuel))
            .unwrap_or(LoadoutSpec {
                tanks: None,
                fuel_lbs: cockpit.flight.fuel,
                cheat: false,
                stations: Vec::new(),
            });
        // The rate in force as the flight starts (slice R1).
        let tps = self.ticks_per_snapshot();
        let Some(peer) = self.peers.get_mut(&connection) else {
            return;
        };
        // A new flight of the connection: its sections start afresh, apart
        // from any of an earlier flight still on the way.
        peer.flight = peer.flight.wrapping_add(1);
        peer.wire = HostConnection::for_flight(tps, peer.flight);
        peer.stage = Stage::Seated;
        peer.seat = Some(seat);
        peer.plane = Some(plane);
        peer.inputs = InputBuffer::new();
        peer.unforeseen = false;
        peer.last_own_state = tick;
        peer.holding_since = Some(tick);
        peer.terms = terms;
        peer.picture = None;
        let callsign = peer.callsign.clone();
        for Timed { tick, event } in Tracker::standing(&self.world, tick) {
            queue(peer, tick, &event);
        }
        // The flight's budget for messages starts once the Seated message
        // and whatever was queued before it are through (see the hold in
        // `snapshot`), so they are not held to 256 bytes a packet.
        self.ever_seated = true;
        self.empty_since = None;
        let flight = self.peers[&connection].flight;
        let seated = Message::Seated(Box::new(Seated {
            flight,
            seat: seat.0,
            plane: plane.0,
            tick: tick as u32,
            exact: exact_bytes,
            loadout,
            roster: self.roster(),
            destroyed: self.tracker.destroyed().iter().copied().collect(),
            surface_digest: self.world.terrain.surface.digest(),
        }));
        self.report_seated(connection, plane);
        self.send(connection, &seated);
        self.send_flight_report(connection);
        self.log(HostLog::Seated {
            tick,
            seat: seat.0,
            callsign,
            plane: plane.0,
        });
    }

    /// Queues the tick's events for every seated player and notes what each
    /// one's game could not foresee.
    fn sort_seats(&mut self, tick: u64, out: &TickOutput, wide: &[Timed]) {
        let mut behind = Vec::new();
        // The data link's changes of the tick, each seat getting those about
        // its own flight (slice G7).
        let link = self.world.take_link();
        for (id, peer) in self.peers.iter_mut() {
            if peer.stage != Stage::Seated {
                continue;
            }
            let (Some(seat), Some(plane)) = (peer.seat, peer.plane) else {
                continue;
            };
            let mut events: Vec<WireEvent> = Vec::new();
            let mut failed = false;
            for cue in &out.cues {
                // The seat's own explosion is its own event.
                if let Cue::Message { seat: s, text } = cue
                    && *s == seat
                    && let Some(on_impact) = exploded(text)
                {
                    events.push(WireEvent::YourAircraftExploded { on_impact });
                    continue;
                }
                match from_world::cue_event(cue, seat, &mut peer.wire.names) {
                    Ok(Some(event)) => events.push(event),
                    Ok(None) => {}
                    Err(_) => failed = true,
                }
            }
            for release in out.releases.iter().filter(|r| r.seat == seat) {
                match from_world::release_event(release, &mut peer.wire.names) {
                    Ok(event) => events.push(event),
                    Err(_) => failed = true,
                }
            }
            for reply in out.orders.iter().filter(|r| r.seat == seat) {
                events.push(from_world::order_event(reply));
            }
            events.extend(from_world::link_events(
                &self.world.datalink,
                plane.0,
                &link,
            ));
            for event in &events {
                failed |= peer.wire.event(tick as u32, event).is_err();
            }
            for timed in wide {
                failed |= !queue(peer, timed.tick, &timed.event);
            }
            if failed {
                behind.push(*id);
            }
            // What the player's game cannot foresee about its own plane.
            let terms = out.terms.iter().find(|(p, _)| *p == plane).map(|(_, t)| *t);
            let changed =
                terms != peer.terms || out.events.iter().any(|event| about(event, plane.0));
            peer.unforeseen |= changed;
            peer.terms = terms;
            #[cfg(test)]
            if changed {
                self.unforeseen_log.push((tick, seat, Unforeseen::Event));
            }
        }
        for id in behind {
            self.server.disconnect(id, DisconnectReason::ProtocolError);
        }
    }

    /// Each seated player's snapshot of `tick`, and its plane's exact state
    /// when due.
    /// The snapshots due at `tick`: each seat's has its own phase within the
    /// interval ([`wire::snapshot_phase`]), so the cost spreads over the
    /// interval's ticks instead of landing on one.
    fn snapshots(&mut self, tick: u64, now: Duration, out: &TickOutput) {
        // Stage K: none while a host that took over resumes (slice K4).
        if self.resume_quiet() {
            return;
        }
        #[cfg(test)]
        if let Some(executor) = self.snapshot_executor.clone() {
            return self.snapshots_with_executor(tick, now, out, &executor);
        }
        self.snapshots_with_executor(tick, now, out, tore_workers::shared());
    }

    fn snapshots_with_executor(
        &mut self,
        tick: u64,
        now: Duration,
        out: &TickOutput,
        executor: &tore_workers::Executor,
    ) {
        let tps = self.ticks_per_snapshot();
        let ids: Vec<ConnectionId> = self
            .peers
            .iter()
            .filter(|(id, peer)| {
                peer.stage == Stage::Seated
                    && peer.seat.is_some_and(|seat| {
                        tick % u64::from(tps) == crate::wire::snapshot_phase(seat.0, tps)
                    })
                    // Stage K: none before Resumed names the flight.
                    && !self.awaiting_resume(**id)
            })
            .map(|(id, _)| *id)
            .collect();
        if ids.is_empty() {
            return;
        }
        // Who each side's sensors track: every human-flown plane's contacts.
        let mut tracked: BTreeMap<bool, BTreeSet<u32>> = BTreeMap::new();
        for own in self.world.combat.state.ownships() {
            let Some(entry) = self.world.roster.plane(PlaneId(own.aircraft)) else {
                continue;
            };
            let set = tracked
                .entry(entry.slot.wing.side == Side::Friendly)
                .or_default();
            set.extend(own.sensors.contacts().iter().map(|c| c.id));
            set.extend(own.sensors.visual().iter().map(|c| c.id));
        }
        // The part of the pictures every seat shares, built once (slice
        // S11); none, and each seat takes a whole picture of its own.
        #[cfg(test)]
        let legacy = self.legacy_pictures;
        #[cfg(not(test))]
        let legacy = false;
        let shared = (!legacy && ids.iter().any(|id| self.peers[id].plane.is_some()))
            .then(|| from_world::SharedPicture::of(&self.world))
            .flatten();
        // Only immutable pictures and readouts cross the worker boundary.
        // Preserve connection-ID order, snapshot phases, and all transport
        // mutations below. Fitted: two due seats amortize the dispatch.
        let mut prepared = executor.should_dispatch(ids.len(), 2).then(|| {
            let planes: Vec<_> = ids.iter().map(|id| self.peers[id].plane).collect();
            let world = &self.world;
            let shared = shared.as_ref();
            executor.ordered_map(&planes, 2, |_, plane| {
                plane.and_then(|plane| prepare_seat(world, plane, shared))
            })
        });
        let mut failed = Vec::new();
        for (index, id) in ids.into_iter().enumerate() {
            let picture = match prepared.as_mut() {
                Some(pictures) => pictures[index].take(),
                None => self.peers[&id]
                    .plane
                    .and_then(|plane| prepare_seat(&self.world, plane, shared.as_ref())),
            };
            let Some(picture) = picture else {
                continue;
            };
            if self
                .snapshot(id, tick, now, out, &tracked, picture, shared.as_ref())
                .is_err()
            {
                failed.push(id);
            }
        }
        for id in failed {
            self.server.disconnect(id, DisconnectReason::ProtocolError);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn snapshot(
        &mut self,
        connection: ConnectionId,
        tick: u64,
        now: Duration,
        out: &TickOutput,
        tracked: &BTreeMap<bool, BTreeSet<u32>>,
        prepared: PreparedSeat,
        shared: Option<&from_world::SharedPicture>,
    ) -> Result<(), WireError> {
        let world = &self.world;
        let Some(peer) = self.peers.get_mut(&connection) else {
            return Ok(());
        };
        let Some(plane) = peer.plane else {
            return Ok(());
        };
        let Some(cockpit) = world.cockpits.iter().find(|c| c.plane == plane) else {
            return Ok(());
        };
        let PreparedSeat { picture, readout } = prepared;
        let slot = world.roster.plane(plane).map(|p| p.slot);
        let flight: BTreeSet<u32> = world
            .roster
            .planes()
            .iter()
            .filter(|p| Some(p.slot.wing) == slot.map(|s| s.wing))
            .map(|p| p.id.0)
            .collect();
        let side_tracks = slot.and_then(|s| tracked.get(&(s.wing.side == Side::Friendly)));
        let own = cockpit.flight.position;
        let (entities, memory) = match (&picture, shared) {
            (SeatPicture::Own(seat), Some(shared)) => from_world::seat_entities(
                shared,
                seat,
                peer.picture.as_ref(),
                &mut peer.wire.names,
            )?,
            #[cfg(test)]
            (SeatPicture::Whole(picture), _) if self.legacy_pictures => {
                let entities = from_world::entities_scan(
                    picture,
                    peer.picture.as_ref().and_then(|m| m.whole.as_deref()),
                    plane.0,
                    &mut peer.wire.names,
                )?;
                let mut memory = from_world::PictureMemory::of(picture);
                memory.keys = entities.iter().map(Entity::key).collect();
                memory.whole = Some(picture.clone());
                (entities, memory)
            }
            (SeatPicture::Whole(picture), _) => {
                let entities = from_world::entities_after(
                    picture,
                    peer.picture.as_ref(),
                    plane.0,
                    &mut peer.wire.names,
                )?;
                #[allow(unused_mut)]
                let mut memory = from_world::PictureMemory::of(picture);
                #[cfg(test)]
                {
                    memory.keys = entities.iter().map(Entity::key).collect();
                }
                (entities, memory)
            }
            (SeatPicture::Own(_), None) => return Err(WireError::Invalid("no shared picture")),
        };
        let entities: Vec<(Entity, _)> = entities
            .into_iter()
            .map(|entity| {
                let mut relevance = from_world::distance_relevance(&entity, own, plane.0);
                let aircraft = entity.state.kind() == EntityKind::Aircraft;
                relevance.own_flight = aircraft && flight.contains(&entity.id);
                relevance.friendly_tracked =
                    aircraft && side_tracks.is_some_and(|t| t.contains(&entity.id));
                relevance.viewed = peer.inputs.view_subject == Some(entity.key());
                (entity, relevance)
            })
            .collect();
        let terms = out.terms.iter().find(|(p, _)| *p == plane).map(|(_, t)| *t);
        let exact = ExactState::of(&OwnPlane::of(cockpit), terms.as_ref());
        let (input_margin, inputs_repeated) = peer.inputs.report();
        let header = SnapshotHeader {
            flight: peer.flight,
            tick: tick as u32,
            input_received: peer.inputs.newest(),
            input_margin,
            inputs_repeated,
            commands_applied: peer.inputs.applied(),
            own_hash: Some(exact.hash()?),
        };
        // New names go first, in a message the packet can carry.
        if let Some(names) = peer.wire.names.take_new() {
            let message = Message::Names(names);
            let body = message.encode()?;
            if self
                .server
                .send_message(connection, message.kind(), &body)
                .is_err()
            {
                return Err(WireError::Invalid("message queue"));
            }
        }
        let messages = self
            .server
            .messages_due_bytes(connection, now)
            .min(FLIGHT_MESSAGE_BUDGET);
        let packet =
            peer.wire
                .snapshot_with_readout(&header, &entities, readout.as_ref(), messages)?;
        match self
            .server
            .send_payload(now, connection, &packet.sections())
        {
            Ok(sequence) => peer.wire.sent(sequence),
            Err(_) => peer.wire.discard(),
        }
        peer.picture = Some(memory);

        // The Seated message comes first: an exact state is held until it
        // is acknowledged (every reliable message acknowledged, since the
        // transport reports no more than the count), and then goes out at
        // once if one was due meanwhile.
        if let Some(since) = peer.holding_since {
            let acknowledged = self
                .server
                .stats(connection)
                .is_some_and(|stats| stats.messages_queued == 0);
            if acknowledged || tick.saturating_sub(since) >= SEATED_HOLD_TICKS {
                peer.holding_since = None;
                self.server
                    .set_message_budget(connection, FLIGHT_MESSAGE_BUDGET);
            }
        }
        let mismatch = peer.inputs.mismatch > peer.mismatch_answered;
        let due = peer.holding_since.is_none()
            && (peer.unforeseen
                || mismatch
                || tick.saturating_sub(peer.last_own_state) >= OWN_STATE_INTERVAL_TICKS);
        if due {
            let bytes = peer.wire.own_state(tick as u32, &exact)?;
            match self
                .server
                .send_payload(now, connection, &[(SECTION_OWN_STATE, &bytes)])
            {
                Ok(sequence) => {
                    peer.wire.sent(sequence);
                    peer.unforeseen = false;
                    peer.last_own_state = tick;
                    peer.mismatch_answered = peer.inputs.mismatch;
                }
                Err(_) => peer.wire.discard(),
            }
        }
        Ok(())
    }

    // ----- The lifecycle ------------------------------------------------

    /// The time limit and the empty timeout.
    fn check_lifecycle(&mut self, now: Duration) {
        // A player the AI flies for while it is away still plays, and so
        // does one who lost that plane and may still fly again.
        let seated = self
            .peers
            .values()
            .any(|p| matches!(p.stage, Stage::Seated | Stage::Taking { .. }))
            || self.anyone_away()
            || self.anyone_awaiting_spawn()
            || self.seated_absent();
        if seated {
            self.empty_since = None;
        } else if self.ever_seated && self.empty_since.is_none() {
            self.empty_since = Some(now);
        }
        let next = self.after_end();
        let flown = ticks_time(self.world.tick());
        // The King's time limit (stage F phase 2), which starts as the
        // configuration's.
        let limit = self.settings.time_limit_seconds();
        if limit.is_some_and(|limit| flown >= Duration::from_secs(u64::from(limit))) {
            self.end_mission(EndReason::TimeLimit, next);
        } else if self
            .empty_since
            .is_some_and(|since| now.saturating_sub(since) >= self.config.empty_timeout)
        {
            self.end_mission(EndReason::EveryoneLeft, next);
        }
    }

    /// Ends the mission: "Mission ended" to every player, and each seated
    /// player's debrief. `next` is the delay to the next mission: every
    /// player stays connected, back in the lobby with its slot and loadout
    /// and its ready mark cleared. With `None` the host stops: each player is
    /// disconnected once its messages are acknowledged.
    fn end_mission(&mut self, reason: EndReason, next: Option<Duration>) {
        if matches!(self.life, Life::Ended { .. } | Life::Stopped) {
            return;
        }
        let now = self.now;
        // Stage F phase 2: the final scores, the observers stop watching,
        // and every plane's results go out, before Mission ended.
        self.report_update();
        self.score_end(reason);
        self.observe_end();
        self.away_end();
        self.send_results(reason);
        let ended = Message::MissionEnded(MissionEnded {
            reason,
            next_in_seconds: next.map(|d| d.as_secs().min(u64::from(u32::MAX)) as u32),
        });
        let ids: Vec<ConnectionId> = self.peers.keys().copied().collect();
        for id in ids {
            let peer = &self.peers[&id];
            if matches!(peer.stage, Stage::Closing { .. }) {
                continue;
            }
            let report = match peer.stage {
                Stage::Seated => self.player_report(id),
                _ => None,
            };
            self.send(id, &ended);
            if let Some(report) = report {
                self.send(id, &Message::Debrief(Box::new(report)));
            }
            if let Some(peer) = self.peers.get_mut(&id) {
                peer.lobby.ready = false;
                if next.is_some() {
                    peer.stage = Stage::Lobby;
                    peer.seat = None;
                    peer.plane = None;
                    // A player whose own build of the flight failed may try
                    // the next one: the lobby's mission was fine for it.
                    if std::mem::take(&mut peer.lobby.unable_flight) {
                        peer.lobby.unable = None;
                    }
                } else {
                    peer.ended = true;
                    peer.stage = Stage::Closing {
                        deadline: now + CLOSE_GRACE,
                        reason: DisconnectReason::ServerStopping,
                    };
                }
            }
            if next.is_some() {
                self.server.set_message_budget(id, tore_net::MAX_DATAGRAM);
            }
        }
        self.lobby_dirty = true;
        self.gives.clear();
        self.life = Life::Ended {
            next_at: next.map(|delay| now + delay),
            stop_at: now + CLOSE_GRACE,
        };
        self.sequence_notice();
        self.journal_ended(reason);
        let tick = self.world.tick();
        self.log(HostLog::MissionEnded { tick, reason });
    }

    /// A fresh copy of the lobby's mission, from its spec: back to the
    /// lobby, or flying again for `start now`.
    fn next_mission(&mut self) {
        if self.sequence_mission() {
            return;
        }
        let mut spec = self.spec.clone();
        match self.build_seeded(&mut spec) {
            Ok((world, manifest)) => {
                let seed_changed = self.spec.surface_seed != spec.surface_seed;
                self.spec = spec;
                self.world = world;
                self.manifest = manifest;
                self.spec_text = self.spec.to_text();
                if seed_changed {
                    self.number = self.number.wrapping_add(1);
                    let mission = self.mission_message();
                    let peers: Vec<_> = self.peers.keys().copied().collect();
                    for peer in peers {
                        self.send(peer, &mission);
                    }
                }
                self.tracker = Tracker::new(&self.world);
                self.revival = revive::Revivals::default();
                self.gives.clear();
                self.origin = None;
                self.ticks_run = 0;
                self.ever_seated = false;
                self.empty_since = None;
                self.lobby_dirty = true;
                self.log(HostLog::MissionRestarted { tick: 0 });
                self.life = Life::Lobby;
                if self.start_rule() == StartMode::Now {
                    // A failure is logged as a fault; the lobby stays.
                    let _ = self.start_flying();
                } else {
                    self.note_start_wait(true);
                }
            }
            Err(error) => {
                let tick = self.world.tick();
                self.log(HostLog::Fault {
                    tick,
                    text: error.to_string(),
                });
                self.stop();
            }
        }
    }

    /// The mission cannot go on: it ends as the operator's end would.
    fn fault(&mut self, text: String) {
        let tick = self.world.tick();
        self.log(HostLog::Fault { tick, text });
        let next = self.after_end();
        self.end_mission(EndReason::EndedByServer, next);
    }
}

/// Queues a mission-wide event for `peer`, numbering its names in the
/// connection's table; false when the connection is too far behind.
fn queue(peer: &mut Peer, tick: u64, event: &Wide) -> bool {
    let event = match event {
        Wide::Event(event) => event.clone(),
        Wide::Launch {
            shooter,
            projectile,
            weapon,
        } => match peer.wire.names.intern(weapon) {
            Ok(weapon) => WireEvent::Launch {
                shooter: *shooter,
                projectile: *projectile,
                weapon,
            },
            Err(_) => return false,
        },
    };
    peer.wire.event(tick as u32, &event).is_ok()
}

/// The explosion lines the tick gives a seat whose own plane exploded.
fn exploded(text: &str) -> Option<bool> {
    match text {
        "Your aircraft exploded on impact" => Some(true),
        "Your aircraft exploded" => Some(false),
        _ => None,
    }
}

/// Whether a combat event did something to `plane` that its player's game
/// cannot foresee: a hit, damage, a blast, its destruction or a release.
fn about(event: &live::Event, plane: u32) -> bool {
    use live::Event as E;
    match event {
        E::Fired { aircraft, .. }
        | E::OwnshipDamaged { aircraft, .. }
        | E::SubsystemDamaged { aircraft, .. }
        | E::OwnshipDestroyed { aircraft }
        | E::PilotKilled { aircraft }
        | E::OwnshipGroundImpact { aircraft } => *aircraft == plane,
        E::Jolt(jolt) => jolt.target == plane,
        E::Airburst(id) => *id == plane,
        _ => false,
    }
}

/// The aircraft that flies as `plane`.
fn aircraft_of(world: &World, plane: PlaneId) -> Option<tore_formats::aircraft::AircraftId> {
    world.aircraft_of(plane)
}

/// What a seated plane carries now, as a loadout names it: each station's
/// rounds, except that a gun pod station counts the pods still carrying
/// rounds (the loadout's unit, which the configuration multiplies by each
/// pod's rounds when the loadout is applied), and the external tanks
/// installed. A station's failure bit is kept.
fn carried_loadout(
    config: &tore_sim::combat::live::Configuration,
    ammo: &[u16],
    fuel_lbs: f64,
) -> LoadoutSpec {
    const FAILED: u16 = 0x8000;
    LoadoutSpec {
        tanks: Some(
            config
                .tanks
                .iter()
                .filter(|s| s.quantity > 0)
                .filter_map(|s| {
                    s.store.as_ref().map(|store| tore_world::mission::TankLoad {
                        hardpoint: s.hardpoint as u8,
                        tank: store.source.clone(),
                        quantity: s.quantity,
                    })
                })
                .collect(),
        ),
        fuel_lbs,
        cheat: false,
        stations: config
            .stations
            .iter()
            .zip(ammo)
            .enumerate()
            .map(|(index, (station, &rounds))| {
                let quantity = config
                    .gun_pods
                    .iter()
                    .find(|pod| pod.station == index)
                    .filter(|_| station.weapon.source == "SUU16.JT")
                    .map_or_else(
                        // A rocket pod's rockets are `projsInPod` a pod.
                        || {
                            let each = tore_sim::combat::live::pod_rounds(&station.weapon);
                            let units = u32::from(rounds & !FAILED).div_ceil(each);
                            units as u16 | (rounds & FAILED)
                        },
                        |pod| {
                            let units = (rounds & !FAILED)
                                .div_ceil(pod.rounds_per_pod.max(1))
                                .min(pod.quantity);
                            units | (rounds & FAILED)
                        },
                    );
                StationLoad {
                    weapon: station.weapon.source.clone(),
                    count: station.count,
                    quantity,
                }
            })
            .collect(),
    }
}

impl Host {
    fn feedback(&mut self, connection: ConnectionId, mut f: tore_replay::feedback::Feedback) {
        if !self.config.feedback
            || self.config.listable
            || !matches!(self.life, Life::Flying)
            || !tore_replay::feedback::Feedback::valid_text(&f.text)
        {
            return;
        }
        if self.recording_failed {
            self.send(
                connection,
                &Message::Notice("Feedback is unavailable: the server recording failed.".into()),
            );
            return;
        }
        let Some(peer) = self.peers.get_mut(&connection) else {
            return;
        };
        let Some(seat) = peer.seat else {
            return;
        };
        if peer.stage != Stage::Seated {
            return;
        }
        if !peer.chat_rate.allow(self.now) {
            self.send(
                connection,
                &Message::Notice(
                    "Feedback was not saved: please wait before submitting again.".into(),
                ),
            );
            return;
        }
        let Some(plane) = self.world.roster.seat(seat).and_then(|s| s.plane) else {
            return;
        };
        f.tick = f.tick.min(self.world.combat.render_snapshot().tick);
        f.submit_tick = self.world.combat.render_snapshot().tick;
        f.player = peer.callsign.clone();
        f.seat = seat.0;
        f.plane = plane.0;
        f.aircraft = self
            .world
            .aircraft_of(plane)
            .map_or(String::new(), |a| a.selection_key().into());
        self.send(connection, &Message::FeedbackAccepted(f.clone()));
        self.log(HostLog::Feedback(f));
    }
}

impl Host {
    fn record_tick(&mut self, inputs: &[tore_world::seats::SeatInput]) {
        let Some(options) = &self.config.recording else {
            return;
        };
        if self.recording_failed {
            return;
        }
        if self.recording.is_none() {
            match crate::recording::Recorder::start(
                options.clone(),
                &self.world,
                &self.config.build,
            ) {
                Ok(recorder) => self.recording = Some(recorder),
                Err(error) => {
                    self.recording_failed = true;
                    self.lobby_dirty = true;
                    self.log(HostLog::Note {
                        tick: self.world.tick(),
                        text: format!("Recording failed: {error}"),
                    });
                    return;
                }
            }
        }
        if let Some(recorder) = &mut self.recording
            && let Err(error) = recorder.tick(&self.world, inputs)
        {
            self.recording_failed = true;
            self.lobby_dirty = true;
            // Keep ownership until mission end: joining a stalled disk
            // writer here would put the I/O stall back on the tick thread.
            self.log(HostLog::Note {
                tick: self.world.tick(),
                text: format!("Recording failed: {error}"),
            });
        }
    }
    fn finish_recording(&mut self) {
        if let Some(recorder) = self.recording.take() {
            let text = match recorder.finish() {
                Ok(path) => format!("Replay saved: {}", path.display()),
                Err(error) => format!("Recording failed: {error}"),
            };
            self.logs.push_back(HostLog::Note {
                tick: self.world.tick(),
                text,
            });
        }
    }
}
