//! Taking over and resuming (stage K; docs/ARCHITECTURE.md, "Losing the
//! host"), built by slice K4.
//!
//! **The new host.** [`Host::resume`] builds a host from a standby's
//! [`Takeover`]: the world at T (the tick the next step runs), the session's
//! state parts and each seat's last recorded input. Every player of the
//! players part is **absent** until its game connects again with its rejoin
//! token and resumes:
//!
//! - **Resume** (message 48) is answered with **Resumed** (49): a seated
//!   player gets a new flight, its seat and plane, the tick its plane stands
//!   at, the number of the last command the old host applied for it and its
//!   plane's exact state there; anyone else is told it is not flying (and
//!   gets the Mission again when its number or text differs).
//! - **Backlog** (50) brings a seated player's inputs from that tick on, and
//!   the commands not yet applied, numbered afresh.
//! - **The resume window.** The clock holds at T until every seated player
//!   has resumed and sent its backlog, at most [`RESUME_WINDOW`]. *Agent
//!   decision (K10):* the old house's player is not waited for when the host
//!   was lost: its game was that host, so it is gone with it or comes back
//!   only once its game learns of the takeover, after the window.
//! - **The fast-forward.** Then the host steps from T to the **present**,
//!   where the old host's clock would be now ([`Present`]), at most
//!   [`FAST_FORWARD_TICKS`] an update, sending no snapshot meanwhile: each
//!   resumed seat on its backlog, each absent one on its last recorded
//!   controls and then neutral by the stall rule. At the present it goes
//!   **live** and keeps the old host's pace.
//! - **The drop.** A player still absent [`DROP_AFTER`] after the takeover is
//!   dropped as a silent player is: its plane goes to the AI, reserved for it
//!   (slice K5), and its token brings it back later.
//!
//! **The old host.** A host that hears Taken over (52) from a standby, or
//! that loses every remote player at once while it had a ready standby
//! ([`Host::lost_everyone`]), has its game ask standby 1 whether it hosts
//! now ([`reach_packet`], [`hosting_answer`]); once it does, the host steps
//! down without a word ([`Host::step_down`]) and its game's own client
//! resumes like any other. A host that leaves on purpose hands over
//! ([`Host::hand_over`]): Handover to standby 1 after its last tick and Host
//! moving (51) to every player.
//!
//! *Agent decisions (K4)*, each in docs/ARCHITECTURE.md's K4 row:
//!
//! - **T and the exact state.** T is the tick the new host steps next (the
//!   world's `tick()`); Resumed's exact state is the plane's as tick T
//!   begins, which a client's prediction labels T - 1, and the Backlog's
//!   first tick is T. A player who resumes after the fast-forward has passed
//!   T is resumed at the host's tick then.
//! - **Absent players** are kept outside the connection table, each under a
//!   placeholder connection id that the revivals part may name; a resume
//!   renames it by coding the revivals part again (slice K1's coders, which
//!   name connections by join order).
//! - **Inputs while resuming** are kept per resumed seat and fed to its input
//!   buffer one tick at a time as the fast-forward reaches each, commands at
//!   their tick in number order, so a backlog seconds long is neither dropped
//!   as too far ahead nor applied early; at the present what is left goes in
//!   at once.
//! - **The crown** stays with its holder; a King who never resumes is
//!   dropped and the crown passes as for any departing King.
//! - **The house** is the new host's own player; the old house is a player
//!   like any other, but one that handed over in flight has left and is not
//!   expected. *Agent decision (K10):* one that handed over in the lobby is
//!   expected back, since the lobby's move to a better host hands over with
//!   the house staying; if it does not come back (it left the lobby) it is
//!   dropped at [`DROP_AFTER`] as having left, with no note.

use super::journal::Driver;
use super::lobby::state::{PlayerState, StageState};
use super::sorting::Tracker;
use super::state::Clock;
use super::{
    ConnectionId, Host, HostConfig, HostLog, LeaveReason, Life, Peer, Stage, StartMode,
    TICKS_PER_SECOND, queue, ticks_time,
};
use crate::journal::{Part, Tick};
use crate::standby::Takeover;
use crate::standby::takeover::DETECT_SILENCE;
use crate::wire::connection::HostConnection;
use crate::wire::inputs::{Command, InputFrame, InputsSection, NumberedCommand};
use crate::wire::messages::{Message, StandbyMark};
use crate::wire::migration::{Backlog, HostMoving, Resume, Resumed, ResumedFlight, TakenOver};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tore_net::ConnectDetails;
use tore_net::packet::{Packet, Reach, ReachRole};
use tore_world::seats::{PlaneId, SeatId, SeatInput, SeatView};
use tore_world::world::plane::{ExactState, OwnPlane, OwnshipTerms};

/// The clock holds at T this long at most for the seated players to resume
/// (docs/ARCHITECTURE.md, "Losing the host").
pub const RESUME_WINDOW: Duration = Duration::from_millis(1_500);
/// A player still absent this long after the takeover is dropped: its plane
/// goes to the AI, reserved for it (the transport's drop timeout).
pub const DROP_AFTER: Duration = Duration::from_secs(5);
/// Ticks the fast-forward steps in one update at most (agent decision: a
/// second of mission, so an update never holds the transport for long).
pub const FAST_FORWARD_TICKS: u64 = 120;

/// A Backlog from a game that is not resuming here.
pub const NOT_RESUMING: &str = "Nothing is being resumed for you here.";
/// A second Resume or Backlog.
pub const RESUMED_ALREADY: &str = "You have resumed already.";
/// A Backlog for a flight the host did not give.
pub const OTHER_FLIGHT: &str = "That backlog is for another flight.";
/// A Taken over from a game that is no standby.
pub const NOT_A_STANDBY: &str = "Only one of the game's standbys can take it over.";
/// A handover with no ready standby.
pub const NO_STANDBY: &str = "No other game can take over hosting.";

/// Where the old host's clock stands: the tick it would step next at `at`,
/// on the new host's clock, fractional. Its game estimates it from the
/// snapshots' arrivals ([`crate::Client::present`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Present {
    pub at: Duration,
    pub tick: f64,
}

impl Present {
    /// The old host's next tick at `now`.
    pub fn tick_at(self, now: Duration) -> f64 {
        let per_second = TICKS_PER_SECOND as f64;
        if now >= self.at {
            self.tick + (now - self.at).as_secs_f64() * per_second
        } else {
            self.tick - (self.at - now).as_secs_f64() * per_second
        }
    }
}

/// What [`Host::resume`] needs beside the standby's takeover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resumption {
    /// The lobby id of the new host's own player: the house.
    pub house: u8,
    /// When the takeover happens, on the new host's clock.
    pub now: Duration,
    /// The old host's clock; `None` resumes at T with no fast-forward.
    pub present: Option<Present>,
}

/// What happened in a migration, for the host's log (the printers are the
/// game's and the server's, slices K7a and K9).
#[derive(Clone, Debug, PartialEq)]
pub enum ResumeNote {
    /// This host took the game over at `tick`, after replaying `replayed`
    /// ticks in `took`; `absent` players are expected back.
    TookOver {
        tick: u64,
        replayed: u64,
        took: Duration,
        absent: usize,
    },
    /// A player resumed, this long after the takeover.
    Resumed {
        callsign: String,
        flying: bool,
        after: Duration,
    },
    /// The host went live at `tick`, this long after the takeover, having
    /// stepped `fast_forward` ticks to get there.
    Live {
        tick: u64,
        after: Duration,
        fast_forward: u64,
    },
    /// A player never resumed: dropped, its plane kept for it.
    Dropped { callsign: String },
    /// The game's new host says it took over (this is the old host).
    TakenOver { by: u8, tick: u32 },
    /// This host handed the game over.
    HandedOver { to: u8, last_tick: u32 },
    /// This host stepped down: another hosts the game now.
    SteppedDown,
}

/// A player expected back, under the placeholder connection id the parts
/// may name until it resumes ([`placeholder`]).
#[derive(Debug)]
struct Absent {
    player: PlayerState,
}

/// A resumed seat's inputs until the host goes live.
#[derive(Debug)]
struct Store {
    flight: u8,
    /// By tick: the controls and the view (offset and delay).
    frames: BTreeMap<u64, (InputFrame, u8, u8)>,
    /// By number: the tick and the command.
    commands: BTreeMap<u16, (u64, Command)>,
    /// The last command number fed to the input buffer.
    fed: u16,
    backlog: bool,
}

/// The takeover's state while it lasts.
#[derive(Debug)]
struct Active {
    started: Duration,
    /// T.
    tick: u64,
    window_until: Duration,
    present: Option<Present>,
    live: bool,
    fast_forward: u64,
    absent: BTreeMap<u64, Absent>,
    /// The players back, and whether each is done resuming (answered, and
    /// a flying one's backlog in).
    back: BTreeMap<ConnectionId, bool>,
    stores: BTreeMap<ConnectionId, Store>,
    last_inputs: BTreeMap<SeatId, (SeatInput, u16)>,
    /// The old host's clock reading and this host's at the takeover.
    clock: Clock,
    /// The old house that handed over in the lobby, by join order: expected
    /// back from the lobby's move, and left on purpose if it does not come.
    handed_house: Option<u64>,
    /// The old house when the host was lost (no handover), by join order:
    /// its game was the host, so the resume window does not wait for it.
    lost_house: Option<u64>,
    /// When an absent player is dropped: [`DROP_AFTER`] from the new host's
    /// first update since the takeover, set by it.
    drop_at: Option<Duration>,
}

/// The migration's state on the host.
#[derive(Debug, Default)]
pub(super) struct Resuming {
    active: Option<Active>,
    /// The old host's side: the new host's lobby id and T, once Taken over
    /// came.
    moved: Option<(u8, u32)>,
    /// This host handed over to the standby on this connection.
    handed_to: Option<ConnectionId>,
    notes: Vec<ResumeNote>,
}

/// A placeholder connection id for the absent player `order`: the top of
/// the range, where a transport's random ids almost never fall.
fn placeholder(order: u64) -> ConnectionId {
    ConnectionId(u32::MAX - (order.min(u64::from(u32::MAX / 2)) as u32))
}

/// The controls a paused game sends (the stall rule): the scope and the
/// sight's zoom step kept, everything else centred, the throttle where it is.
fn neutral(frame: &InputFrame) -> InputFrame {
    InputFrame {
        sensors: frame.sensors,
        sight_zoom: frame.sight_zoom,
        ..InputFrame::default()
    }
}

/// A section that carries `commands` alone: its one frame is for the tick
/// before `next`, which the buffer has stepped already and drops.
fn commands_alone(flight: u8, next: u64, commands: Vec<NumberedCommand>) -> InputsSection {
    InputsSection {
        flight,
        newest_tick: u32::try_from(next.saturating_sub(1)).unwrap_or(u32::MAX),
        frames: vec![InputFrame::default()],
        view_offset: 0,
        interpolation_delay: 0,
        view_subject: None,
        mismatch: 0,
        commands,
    }
}

impl Store {
    /// The commands due by `tick`, next in number order.
    fn due_commands(&mut self, tick: u64) -> Vec<NumberedCommand> {
        let mut due = Vec::new();
        while let Some(&(at, command)) = self.commands.get(&self.fed.wrapping_add(1)) {
            if at > tick {
                break;
            }
            self.fed = self.fed.wrapping_add(1);
            due.push(NumberedCommand {
                number: self.fed,
                tick: u32::try_from(at).unwrap_or(u32::MAX),
                command,
            });
        }
        due
    }
}

impl Host {
    /// A host that takes the game over from a standby (docs/ARCHITECTURE.md,
    /// "Taking over"): the world and the session's state parts the standby
    /// held, every player of the players part absent until it resumes, its
    /// own player (`resumption.house`) the house. `config` is the hosting
    /// game's own; the King's settings and the session's id come from the
    /// parts. Refused for a dedicated server (`config.house` none) and for a
    /// takeover without the players and session parts.
    pub fn resume(
        takeover: Takeover,
        resources: Arc<BTreeMap<String, Vec<u8>>>,
        config: HostConfig,
        resumption: Resumption,
    ) -> Result<Host, String> {
        if config.house.is_none() {
            return Err("a dedicated server never takes a game over".into());
        }
        let Takeover {
            world,
            tick,
            parts,
            last_inputs,
            handover,
            replayed,
            took,
            ..
        } = takeover;
        let part = |part: Part| {
            parts
                .get(&part)
                .map(|p| p.bytes.clone())
                .ok_or_else(|| format!("the standby holds no {part:?} part"))
        };
        let session = part(Part::Session)?;
        let players = part(Part::Players)?;
        // Built as the session's mission, the flight's loadouts included,
        // for the manifest a joiner checks; a flight's world is the
        // standby's.
        let spec = super::state::session_spec(&session, &config).map_err(|e| e.to_string())?;
        // The mission's own rules (friendly fire and the loadout rule) are
        // the session's: the new host builds the flight's loadouts under
        // them, as the old host did, before the session part restores every
        // setting. Without this a Cheat loading under `loadouts any` was
        // refused at the takeover (found by slice NA's laser bombers).
        let mut config = config;
        {
            use crate::settings::{LoadoutRule, number};
            config
                .settings
                .retain(|(n, _)| *n != number::FRIENDLY_FIRE && *n != number::LOADOUTS);
            let loadouts = if spec.cheat_loadouts {
                LoadoutRule::Any
            } else {
                LoadoutRule::Own
            };
            config.settings.extend([
                (number::FRIENDLY_FIRE, u32::from(spec.friendly_fire)),
                (number::LOADOUTS, loadouts.value()),
            ]);
        }
        let start = config.start;
        let mut host = Host::new(
            spec,
            resources,
            HostConfig {
                start: StartMode::King,
                ..config
            },
        )
        .map_err(|e| e.to_string())?;
        host.config.start = start;
        host.now = resumption.now;
        let old_clock = host.session_clock(&session).map_err(|e| e.to_string())?;
        let mut ids = placeholder;
        host.restore_part(Part::Session, &session, &mut ids, old_clock)
            .map_err(|e| format!("the session part: {e}"))?;
        for kind in [
            Part::Court,
            Part::Scores,
            Part::Revivals,
            Part::Rejoin,
            Part::Candidates,
            Part::Listing,
        ] {
            if let Some(stored) = parts.get(&kind) {
                host.restore_part(kind, &stored.bytes, &mut ids, old_clock)
                    .map_err(|e| format!("the {kind:?} part: {e}"))?;
            }
        }
        let restored = host
            .restore_players(&players, &mut ids)
            .map_err(|e| format!("the players part: {e}"))?;
        let flying = matches!(host.life, Life::Flying);
        match world {
            Some(world) => {
                host.tracker = Tracker::new(&world);
                host.world = Driver::new(world);
            }
            None if flying => return Err("the standby holds no world for a flight".into()),
            // In the lobby the world is the session's mission built fresh.
            None => {}
        }
        let now = resumption.now;
        let clock = Clock {
            old: old_clock,
            new: now,
        };
        let mut absent = BTreeMap::new();
        let mut handed = None;
        let mut handed_house = None;
        let mut lost_house = None;
        for (_, mut player) in restored {
            if player.house && handover.is_none() {
                lost_house = Some(player.lobby.order);
            }
            if player.house && handover.is_some() {
                if flying {
                    // A house that handed over in flight left on purpose.
                    handed = Some(player);
                    continue;
                }
                // In the lobby the handover may be the lobby's move: the old
                // house's player comes back as itself, King included.
                handed_house = Some(player.lobby.order);
            }
            player.house = player.lobby.id == resumption.house;
            absent.insert(player.lobby.order, Absent { player });
        }
        let count = absent.len();
        host.resuming.active = Some(Active {
            started: now,
            tick,
            window_until: now + RESUME_WINDOW,
            present: resumption.present,
            live: false,
            fast_forward: 0,
            absent,
            back: BTreeMap::new(),
            stores: BTreeMap::new(),
            last_inputs,
            clock,
            handed_house,
            lost_house,
            drop_at: None,
        });
        host.resuming.notes.push(ResumeNote::TookOver {
            tick,
            replayed,
            took,
            absent: count,
        });
        if let Some(player) = handed {
            let peer = player.into_peer(clock, host.ticks_per_snapshot());
            host.drop_player(peer, LeaveReason::Left);
        }
        if !flying {
            host.go_live(now);
        }
        host.lobby_dirty = true;
        host.roster_dirty = true;
        Ok(host)
    }

    /// What happened in migrations since the last call, for the host's log.
    pub fn take_resume_notes(&mut self) -> Vec<ResumeNote> {
        std::mem::take(&mut self.resuming.notes)
    }

    /// Whether this host is still resuming: holding its clock for the
    /// players, or stepping to the present, and sending no snapshot.
    pub fn resuming(&self) -> bool {
        self.resuming.active.as_ref().is_some_and(|a| !a.live)
    }

    /// Whether a seated player is still expected back from a takeover: the
    /// mission is not empty while one is, or a game a player hosts (whose
    /// empty timeout is none) would end it before anyone resumed (slice
    /// K7a).
    pub(super) fn seated_absent(&self) -> bool {
        self.resuming.active.as_ref().is_some_and(|a| {
            a.absent
                .values()
                .any(|absent| absent.player.stage == StageState::Seated)
        })
    }

    /// The players still expected back, by callsign.
    pub fn absent_players(&self) -> Vec<String> {
        self.resuming.active.as_ref().map_or_else(Vec::new, |a| {
            a.absent
                .values()
                .map(|absent| absent.player.callsign.clone())
                .collect()
        })
    }

    /// A game connected (the hook before the transport's `Connected` makes
    /// a new player): one whose token names an absent player becomes that
    /// player again, with its seat and plane. Returns whether it did.
    pub(super) fn resume_connected(
        &mut self,
        connection: ConnectionId,
        details: &ConnectDetails,
    ) -> bool {
        let Some(token) = details.token else {
            return false;
        };
        let now = self.token_clock();
        let Ok(order) = self.rejoin.claim(token, now) else {
            return false;
        };
        let tps = self.ticks_per_snapshot();
        let Some(active) = &mut self.resuming.active else {
            return false;
        };
        let Some(absent) = active.absent.remove(&order) else {
            return false;
        };
        let clock = active.clock;
        active.back.insert(connection, false);
        let mut peer = absent.player.into_peer(clock, tps);
        peer.address = details.address;
        peer.path = details.path;
        // A plane that went back while it was away leaves it in the lobby.
        if peer.stage == Stage::Leaving
            && peer.seat.is_none_or(|seat| {
                self.world
                    .roster
                    .seat(seat)
                    .is_none_or(|s| s.plane.is_none())
            })
        {
            peer.stage = Stage::Lobby;
            peer.seat = None;
            peer.plane = None;
        }
        let callsign = peer.callsign.clone();
        let house = peer.house;
        self.peers.insert(connection, peer);
        self.journal.orders.insert(connection, order);
        if house && details.address == tore_net::LINK_ADDRESS {
            self.server.set_silence_exempt(connection, true);
        }
        // Whatever named the placeholder names the connection now.
        self.rename_placeholder(order, connection);
        if let Some(record) = self.rejoin.players.get_mut(&order) {
            record.left = None;
        }
        let tick = self.world.tick();
        self.log(HostLog::Connected {
            tick,
            address: details.address,
            callsign,
            path: details.path,
        });
        self.compat.touch();
        self.lobby_dirty = true;
        self.roster_dirty = true;
        let roster = Message::Roster(self.roster());
        self.send(connection, &roster);
        true
    }

    /// The revivals part names connections: coded by join order and
    /// restored with `connection` for `order` and every other player's own,
    /// it names the connection where it named the placeholder.
    fn rename_placeholder(&mut self, order: u64, connection: ConnectionId) {
        let Ok(bytes) = self.encode_part(Part::Revivals) else {
            return;
        };
        let present: BTreeMap<u64, ConnectionId> = self
            .peers
            .iter()
            .map(|(id, peer)| (peer.lobby.order, *id))
            .collect();
        let mut ids = |o: u64| {
            if o == order {
                connection
            } else {
                present.get(&o).copied().unwrap_or_else(|| placeholder(o))
            }
        };
        let now = self.now;
        let _ = self.restore_part(Part::Revivals, &bytes, &mut ids, now);
    }

    /// A player resumes with this host (message 48): Resumed with its plane
    /// at the host's tick when it flies, else not flying; the Mission again
    /// when the one it holds differs.
    pub(super) fn resume_request(
        &mut self,
        connection: ConnectionId,
        resume: Resume,
    ) -> Result<(), String> {
        if !self.peers.contains_key(&connection) {
            return Ok(());
        }
        let back = self
            .resuming
            .active
            .as_ref()
            .and_then(|a| a.back.get(&connection).copied());
        if back == Some(true)
            || self
                .resuming
                .active
                .as_ref()
                .is_some_and(|a| a.stores.contains_key(&connection))
        {
            return Err(RESUMED_ALREADY.into());
        }
        let same_mission = resume.mission == self.number
            && resume.mission_hash == tore_codec::fnv1a64(self.spec_text.as_bytes());
        if !same_mission {
            let mission = self.mission_message();
            self.send(connection, &mission);
            self.revive_connected(connection);
        }
        let peer = &self.peers[&connection];
        let seated = peer.stage == Stage::Seated && matches!(self.life, Life::Flying);
        let flying = seated && same_mission && back.is_some() && resume.newest_tick != 0;
        let (seat, plane) = (peer.seat, peer.plane);
        let exact = match (flying, plane) {
            (true, Some(plane)) => self.exact_now(plane),
            _ => None,
        };
        let (Some(seat), Some(plane), Some(exact)) = (seat, plane, exact) else {
            if seated && let Some(seat) = seat {
                // Its game does not fly the plane: it goes back to the AI,
                // and the player takes one again from the lobby.
                let callsign = peer.callsign.clone();
                self.gives.push((seat, callsign));
                if let Some(peer) = self.peers.get_mut(&connection) {
                    peer.stage = Stage::Leaving;
                    peer.lobby.ready = false;
                }
            }
            self.send(connection, &Message::Resumed(Box::new(Resumed::NotFlying)));
            self.mark_resumed(connection, false);
            return Ok(());
        };
        let tick = self.world.tick();
        let terms = self
            .world
            .combat
            .state
            .ownship(plane.0)
            .map(|own| OwnshipTerms::of(own, self.world.combat.state.tick()));
        let last_command = self
            .resuming
            .active
            .as_ref()
            .and_then(|a| a.last_inputs.get(&seat))
            .map_or(0, |(_, applied)| *applied);
        // A new flight of the connection: later than the old host's.
        let flight = match resume.flight.wrapping_add(1) {
            0 => 1,
            flight => flight,
        };
        let tps = self.ticks_per_snapshot();
        let standing = Tracker::standing(&self.world, tick);
        let destroyed: Vec<u32> = self.tracker.destroyed().iter().copied().collect();
        let Some(peer) = self.peers.get_mut(&connection) else {
            return Ok(());
        };
        peer.flight = flight;
        peer.wire = HostConnection::for_flight(tps, flight);
        peer.inputs = super::inputs::InputBuffer::new();
        peer.unforeseen = false;
        peer.last_own_state = tick;
        peer.holding_since = Some(tick);
        peer.mismatch_answered = 0;
        peer.terms = terms;
        peer.picture = None;
        for timed in standing {
            queue(peer, timed.tick, &timed.event);
        }
        self.ever_seated = true;
        self.send(
            connection,
            &Message::Resumed(Box::new(Resumed::Flying(ResumedFlight {
                flight,
                seat: seat.0,
                plane: plane.0,
                tick: u32::try_from(tick).unwrap_or(u32::MAX),
                last_command,
                exact,
                destroyed,
                surface_digest: self.world.terrain.surface.digest(),
            }))),
        );
        if let Some(active) = &mut self.resuming.active {
            active.stores.insert(
                connection,
                Store {
                    flight,
                    frames: BTreeMap::new(),
                    commands: BTreeMap::new(),
                    fed: 0,
                    backlog: false,
                },
            );
            if active.live {
                // Resumed after the host went live: nothing to hold for.
                self.mark_resumed(connection, true);
            }
        }
        self.lobby_dirty = true;
        Ok(())
    }

    /// The plane's exact state as the next tick begins, coded with no
    /// baseline.
    fn exact_now(&self, plane: PlaneId) -> Option<Vec<u8>> {
        let cockpit = self.world.cockpits.iter().find(|c| c.plane == plane)?;
        let terms = self
            .world
            .combat
            .state
            .ownship(plane.0)
            .map(|own| OwnshipTerms::of(own, self.world.combat.state.tick()));
        ExactState::of(&OwnPlane::of(cockpit), terms.as_ref())
            .encode(None)
            .ok()
    }

    /// A player is done resuming: noted, and the window no longer waits for
    /// it.
    fn mark_resumed(&mut self, connection: ConnectionId, flying: bool) {
        let now = self.now;
        let callsign = self
            .peers
            .get(&connection)
            .map(|p| p.callsign.clone())
            .unwrap_or_default();
        let Some(active) = &mut self.resuming.active else {
            return;
        };
        let Some(done) = active.back.get_mut(&connection) else {
            return;
        };
        if *done {
            return;
        }
        *done = true;
        let after = now.saturating_sub(active.started);
        self.resuming.notes.push(ResumeNote::Resumed {
            callsign,
            flying,
            after,
        });
    }

    /// A resumed player's backlog (message 50): its inputs from the tick
    /// Resumed named, kept until the fast-forward reaches each.
    pub(super) fn backlog(
        &mut self,
        connection: ConnectionId,
        backlog: &Backlog,
    ) -> Result<(), String> {
        let next = self.world.tick();
        let store = self
            .resuming
            .active
            .as_mut()
            .and_then(|a| a.stores.get_mut(&connection))
            .ok_or(NOT_RESUMING)?;
        if backlog.flight != store.flight {
            return Err(OTHER_FLIGHT.into());
        }
        if store.backlog {
            return Err(RESUMED_ALREADY.into());
        }
        let first = u64::from(backlog.first_tick);
        for (index, tick) in backlog.ticks.iter().enumerate() {
            let at = first + index as u64;
            if at >= next {
                store.frames.entry(at).or_insert((
                    tick.frame,
                    tick.view_offset,
                    tick.interpolation_delay,
                ));
            }
        }
        for (index, command) in backlog.commands.iter().enumerate() {
            let number = u16::try_from(index + 1).unwrap_or(u16::MAX);
            store
                .commands
                .entry(number)
                .or_insert((first + u64::from(command.offset), command.command));
        }
        store.backlog = true;
        let live = self.resuming.active.as_ref().is_some_and(|a| a.live);
        self.mark_resumed(connection, true);
        if live {
            // Resumed after the host went live: straight to the buffer.
            self.flush_store(connection);
        }
        Ok(())
    }

    /// An Inputs section while the seat's kept inputs wait: kept with them
    /// (the hook at the start of `inputs`). Returns whether it took the
    /// section.
    pub(super) fn resume_inputs(&mut self, connection: ConnectionId, body: &[u8]) -> bool {
        let Some(store) = self
            .resuming
            .active
            .as_mut()
            .and_then(|a| a.stores.get_mut(&connection))
        else {
            return false;
        };
        let Ok(section) = InputsSection::decode(body) else {
            return true;
        };
        if section.flight != store.flight {
            return true;
        }
        for (index, frame) in section.frames.iter().enumerate() {
            let tick = u64::from(section.frame_tick(index));
            store.frames.entry(tick).or_insert((
                *frame,
                section.view_offset,
                section.interpolation_delay,
            ));
        }
        for command in &section.commands {
            store
                .commands
                .entry(command.number)
                .or_insert((u64::from(command.tick), command.command));
        }
        if let Some(peer) = self.peers.get_mut(&connection) {
            peer.inputs.view_subject = section.view_subject;
        }
        let sent = ticks_time(u64::from(section.newest_tick));
        let now = self.now;
        self.server.note_arrival(connection, sent, now);
        true
    }

    /// Feeds each resumed seat's input buffer its kept input for `tick` and
    /// the commands due by then, in number order (the hook before the tick
    /// takes the inputs).
    pub(super) fn resume_feed(&mut self, tick: u64) {
        let Some(active) = &mut self.resuming.active else {
            return;
        };
        for (connection, store) in &mut active.stores {
            let Some(peer) = self.peers.get_mut(connection) else {
                continue;
            };
            if peer.stage != Stage::Seated {
                continue;
            }
            let frame = store.frames.remove(&tick);
            while store
                .frames
                .first_key_value()
                .is_some_and(|(&at, _)| at < tick)
            {
                store.frames.pop_first();
            }
            let commands = store.due_commands(tick);
            let section = match frame {
                Some((frame, offset, delay)) => InputsSection {
                    flight: store.flight,
                    newest_tick: u32::try_from(tick).unwrap_or(u32::MAX),
                    frames: vec![frame],
                    view_offset: offset,
                    interpolation_delay: delay,
                    view_subject: peer.inputs.view_subject,
                    mismatch: 0,
                    commands,
                },
                None if !commands.is_empty() => commands_alone(store.flight, tick, commands),
                None => continue,
            };
            let _ = peer.inputs.receive(&section, tick);
        }
    }

    /// What is left of a seat's kept inputs, to its input buffer, and the
    /// store gone: the host takes its Inputs as usual from here.
    fn flush_store(&mut self, connection: ConnectionId) {
        let next = self.world.tick();
        let Some(mut store) = self
            .resuming
            .active
            .as_mut()
            .and_then(|a| a.stores.remove(&connection))
        else {
            return;
        };
        let Some(peer) = self.peers.get_mut(&connection) else {
            return;
        };
        let mut commands = store.due_commands(u64::MAX);
        for command in &mut commands {
            command.tick = command.tick.max(u32::try_from(next).unwrap_or(u32::MAX));
        }
        let frames = std::mem::take(&mut store.frames);
        let mut sections: Vec<InputsSection> = frames
            .into_iter()
            .filter(|(at, _)| *at >= next)
            .map(|(at, (frame, offset, delay))| InputsSection {
                flight: store.flight,
                newest_tick: u32::try_from(at).unwrap_or(u32::MAX),
                frames: vec![frame],
                view_offset: offset,
                interpolation_delay: delay,
                view_subject: peer.inputs.view_subject,
                mismatch: 0,
                commands: Vec::new(),
            })
            .collect();
        if !commands.is_empty() {
            sections.insert(0, commands_alone(store.flight, next, commands));
        }
        for section in sections {
            let _ = peer.inputs.receive(&section, next);
        }
    }

    /// Each absent player's seat flies on its last recorded controls, then
    /// neutral by the stall rule, with no commands (the hook after the
    /// tick's inputs are taken).
    pub(super) fn resume_absent_inputs(&self, tick: u64, journal: &mut Tick) {
        let Some(active) = &self.resuming.active else {
            return;
        };
        for absent in active.absent.values() {
            let player = &absent.player;
            if player.stage != StageState::Seated {
                continue;
            }
            let Some(seat) = player.seat else { continue };
            if journal.inputs.iter().any(|input| input.seat == seat)
                || self
                    .world
                    .roster
                    .seat(seat)
                    .is_none_or(|s| s.plane.is_none())
            {
                continue;
            }
            let (input, applied) = match active.last_inputs.get(&seat) {
                Some((last, applied)) => {
                    let frame = InputFrame::of(&last.pilot, last.trigger, last.sensors);
                    let since = tick.saturating_sub(active.tick);
                    let frame = if since > super::inputs::STALL_TICKS {
                        neutral(&frame)
                    } else {
                        frame
                    };
                    let view = last.view.map(|view| SeatView {
                        tick: view.tick + tick.saturating_sub(last.tick),
                        ..view
                    });
                    (frame.seat_input(seat, tick, &[], view), *applied)
                }
                None => (InputFrame::default().seat_input(seat, tick, &[], None), 0),
            };
            journal.push_input(input, applied);
        }
    }

    /// The resume's timers at `now` (the hook before the flying clock): the
    /// drop, the window, the fast-forward and going live. Returns whether it
    /// holds the flying clock this update.
    pub(super) fn resume_update(&mut self, now: Duration) -> bool {
        if self.resuming.handed_to.is_some() {
            // A host that handed over steps no more.
            return true;
        }
        let Some(active) = &mut self.resuming.active else {
            return false;
        };
        // The drop timeout counts from the new host's first update, when it
        // can first answer anyone: a standby that took seconds to restore
        // and replay (a cold one, or a warm one far behind on a busy machine)
        // had spent them before it, and every player was dropped at once
        // (agent decision, 2026-10-24, found by `net-window-migrate-smoke`).
        // The resume window still counts from the takeover: once it is over
        // the host goes on, and players resume with it live.
        let drop_at = *active.drop_at.get_or_insert(now + DROP_AFTER);
        if now >= drop_at && !active.absent.is_empty() {
            self.drop_absent();
        }
        let Some(active) = &self.resuming.active else {
            return false;
        };
        if active.live {
            return false;
        }
        if !matches!(self.life, Life::Flying) {
            self.go_live(now);
            return false;
        }
        // The old house's player was lost with the host it ran: the window
        // waits for it no longer than for anyone who resumes late (agent
        // decision, K10: waiting cost every kill the full 1.5 seconds).
        let waiting = active.absent.iter().any(|(order, a)| {
            a.player.stage == StageState::Seated && Some(*order) != active.lost_house
        }) || active.back.values().any(|done| !done);
        if waiting && now < active.window_until {
            return true;
        }
        let target = active
            .present
            .map_or(active.tick, |p| p.tick_at(now).floor().max(0.) as u64);
        let mut stepped = 0;
        while self.world.tick() < target
            && stepped < FAST_FORWARD_TICKS
            && matches!(self.life, Life::Flying)
        {
            self.tick(now);
            stepped += 1;
        }
        if let Some(active) = &mut self.resuming.active {
            active.fast_forward += stepped;
        }
        if self.world.tick() >= target || !matches!(self.life, Life::Flying) {
            self.go_live(now);
        }
        true
    }

    /// The host is at the present: what is left of the kept inputs goes to
    /// the input buffers, the flying clock takes the old host's pace, and
    /// snapshots go out again.
    fn go_live(&mut self, now: Duration) {
        let Some(active) = &mut self.resuming.active else {
            return;
        };
        if active.live {
            return;
        }
        active.live = true;
        let present = active.present;
        let after = now.saturating_sub(active.started);
        let fast_forward = active.fast_forward;
        // A player whose Backlog is still on its way keeps its store: the
        // Backlog finds it, and its Inputs meanwhile are kept and fed from it
        // tick by tick, as for a player resumed after the host went live
        // (the battery's planned handover, 2026-10-24: dropped here, the late
        // Backlog was refused, the player was never done resuming, and no
        // snapshot went to it again).
        let flushed: Vec<ConnectionId> = active
            .stores
            .iter()
            .filter(|(_, store)| store.backlog)
            .map(|(connection, _)| *connection)
            .collect();
        for connection in flushed {
            self.flush_store(connection);
        }
        let next = self.world.tick();
        if matches!(self.life, Life::Flying) {
            // The old host's pace, so every client's clock and prediction
            // stay where they were: the flying clock counts from an origin
            // a fraction of a tick back, with as many ticks run as the host
            // is ahead of the present.
            let at = present.map_or(next as f64, |p| p.tick_at(now)).max(0.);
            let owed = at.floor() as u64;
            let fraction = at - at.floor();
            self.origin = Some(
                now.saturating_sub(Duration::from_secs_f64(fraction / TICKS_PER_SECOND as f64)),
            );
            self.ticks_run = 1 + next.saturating_sub(owed);
        }
        self.resuming.notes.push(ResumeNote::Live {
            tick: next,
            after,
            fast_forward,
        });
        self.lobby_dirty = true;
        if matches!(self.life, Life::Flying) {
            self.broadcast_roster();
            self.roster_dirty = false;
        }
    }

    /// Every player still absent is dropped as a silent player is: its
    /// token's 24 hours start, its plane goes to the AI, reserved for it,
    /// and a King's crown passes on.
    fn drop_absent(&mut self) {
        let Some(active) = &mut self.resuming.active else {
            return;
        };
        let clock = active.clock;
        let handed_house = active.handed_house;
        let absent = std::mem::take(&mut active.absent);
        let tps = self.ticks_per_snapshot();
        for (order, Absent { player, .. }) in absent {
            let callsign = player.callsign.clone();
            let peer = player.into_peer(clock, tps);
            if Some(order) == handed_house {
                // The old house left the lobby rather than moving with it.
                self.drop_player(peer, LeaveReason::Left);
                continue;
            }
            self.drop_player(peer, LeaveReason::Silent);
            self.resuming.notes.push(ResumeNote::Dropped { callsign });
        }
    }

    /// A player that will not come back on its own: as `closed` handles a
    /// connection that ended.
    fn drop_player(&mut self, peer: Peer, reason: LeaveReason) {
        self.rejoin_left(&peer, reason);
        if peer.stage == Stage::Seated
            && let Some(seat) = peer.seat
        {
            self.gives.push((seat, peer.callsign.clone()));
        }
        let tick = self.world.tick();
        self.log(HostLog::Left {
            tick,
            seat: peer.seat.map(|s| s.0),
            callsign: peer.callsign.clone(),
            plane: peer.plane.map(|p| p.0),
            reason,
        });
        if peer.king {
            self.crown_departed();
        }
        self.lobby_dirty = true;
        self.roster_dirty = true;
    }

    /// No snapshot goes out while the host resumes or after it handed over
    /// (the hook at the start of the snapshots).
    pub(super) fn resume_quiet(&self) -> bool {
        self.resuming() || self.resuming.handed_to.is_some()
    }

    /// Whether `connection` came back by its token and its Resume has not
    /// been answered yet: it has no flight of this host until Resumed names
    /// one, so no snapshot goes to it before then. A snapshot of the
    /// placeholder flight would start a flight on the client whose name
    /// table had dropped the Names message before it, and the next Names
    /// would be refused as out of order.
    pub(super) fn awaiting_resume(&self, connection: ConnectionId) -> bool {
        self.resuming.active.as_ref().is_some_and(|a| {
            a.back.get(&connection) == Some(&false) && !a.stores.contains_key(&connection)
        })
    }

    // ----- The old host --------------------------------------------------

    /// A new host says it has taken over (message 52, from its game's own
    /// client on its old connection): kept for [`Host::moved_to`]. Taken
    /// only from a standby that names itself.
    pub(super) fn taken_over(
        &mut self,
        connection: ConnectionId,
        taken: TakenOver,
    ) -> Result<(), String> {
        let peer = self.peers.get(&connection).ok_or("")?;
        let standby = self.standby_mark(peer.lobby.order) != StandbyMark::None;
        if !standby || peer.lobby.id != taken.new_host {
            return Err(NOT_A_STANDBY.into());
        }
        self.resuming.moved = Some((taken.new_host, taken.tick));
        self.resuming.notes.push(ResumeNote::TakenOver {
            by: taken.new_host,
            tick: taken.tick,
        });
        Ok(())
    }

    /// The new host's lobby id and T, once a standby said it took over.
    pub fn moved_to(&self) -> Option<(u8, u32)> {
        self.resuming.moved
    }

    /// Whether this host has heard nothing from any remote player for
    /// [`DETECT_SILENCE`] while it had a ready standby: cut off, it may have
    /// been taken over (docs/ARCHITECTURE.md, "The old host"). Its game then
    /// asks standby 1 ([`Host::standby_addresses`]) whether it hosts now.
    pub fn lost_everyone(&self) -> bool {
        if self.ready_standbys().is_empty() {
            return false;
        }
        let mut remote = self
            .peers
            .iter()
            .filter(|(_, peer)| !peer.house && !matches!(peer.stage, Stage::Closing { .. }))
            .peekable();
        remote.peek().is_some()
            && remote.all(|(id, _)| {
                self.server
                    .stats(*id)
                    .is_none_or(|stats| stats.since_last_received >= DETECT_SILENCE)
            })
    }

    /// Standby 1's addresses as the Succession names them: where the old
    /// host's game asks whether it hosts now.
    pub fn standby_addresses(&self) -> Vec<SocketAddr> {
        let ready = self.ready_standbys();
        self.succession_message(&ready)
            .standbys
            .first()
            .map(|s| s.addresses.iter().map(|c| c.address).collect())
            .unwrap_or_default()
    }

    /// The session's id, which a Reach to the new host names.
    pub fn session(&self) -> u64 {
        self.session_id
    }

    /// Another game hosts now: this host stops without a word to anyone,
    /// and its game's own client resumes with the new host like any other.
    pub fn step_down(&mut self) {
        if matches!(self.life, Life::Stopped) {
            return;
        }
        self.life = Life::Stopped;
        let tick = self.world.tick();
        self.log(HostLog::Stopped { tick });
        self.resuming.notes.push(ResumeNote::SteppedDown);
    }

    /// The house leaves on purpose and the game moves to standby 1
    /// (docs/ARCHITECTURE.md, "Leaving on purpose"): the house's flight ends
    /// as usual (its debrief, its plane to the AI at one more tick), standby
    /// 1 gets Handover after that tick, every player Host moving, and the
    /// host steps no more. Returns standby 1's lobby id. Refused with no
    /// ready standby: the house's leaving then ends the game, as before.
    pub fn hand_over(&mut self) -> Result<u8, String> {
        if self.resuming.handed_to.is_some() {
            return Err("The game has been handed over already.".into());
        }
        let Some(&(standby, _)) = self.ready_standbys().first() else {
            return Err(NO_STANDBY.into());
        };
        let to = self
            .peers
            .get(&standby)
            .map(|p| p.lobby.id)
            .ok_or(NO_STANDBY)?;
        let house = self
            .peers
            .iter()
            .find(|(_, peer)| peer.house)
            .map(|(id, _)| *id);
        if matches!(self.life, Life::Flying) {
            if let Some(house) = house {
                self.leave(house);
            }
            // One more tick takes the plane back, so the standby holds it
            // the AI's.
            let now = self.now;
            self.ticks_run += 1;
            self.tick(now);
        }
        self.journal_parts();
        self.standby_update();
        let last_tick = u32::try_from(self.world.tick().saturating_sub(1)).unwrap_or(u32::MAX);
        self.standby_handover(standby, last_tick);
        let moving = Message::HostMoving(HostMoving {
            standby: to,
            last_tick,
        });
        let everyone: Vec<ConnectionId> = self
            .peers
            .iter()
            .filter(|(_, peer)| !matches!(peer.stage, Stage::Closing { .. }))
            .map(|(id, _)| *id)
            .collect();
        for connection in everyone {
            self.send(connection, &moving);
        }
        self.finish_recording();
        self.resuming.handed_to = Some(standby);
        self.resuming
            .notes
            .push(ResumeNote::HandedOver { to, last_tick });
        Ok(to)
    }

    /// Whether this host has handed the game over: the house's connection
    /// ending no longer ends the game (the hook in `closed`).
    pub(super) fn resume_handed(&self) -> bool {
        self.resuming.handed_to.is_some()
    }

    /// Whether a handover's records are all through to standby 1 (or it has
    /// gone): the game may stop this host.
    pub fn handed_over(&self) -> bool {
        self.resuming.handed_to.is_some_and(|standby| {
            self.server
                .stats(standby)
                .is_none_or(|stats| stats.messages_queued == 0)
        })
    }
}

/// A Reach that asks a game whether it hosts `session` now, from the game
/// with lobby id `from`: what an old host's game sends standby 1.
pub fn reach_packet(session: u64, nonce: u64, from: u8) -> Vec<u8> {
    Packet::Reach(Reach {
        session_id: session,
        nonce,
        from,
    })
    .encode(crate::wire::PROTOCOL_VERSION)
    .unwrap_or_default()
}

/// Whether `datagram` answers the Reach `nonce` for `session` with "hosting
/// this session now".
pub fn hosting_answer(datagram: &[u8], session: u64, nonce: u64) -> bool {
    matches!(
        Packet::decode(datagram, crate::wire::PROTOCOL_VERSION),
        Ok(Packet::ReachAnswer(answer))
            if answer.nonce == nonce
                && answer.session_id == session
                && answer.role == ReachRole::Hosting
    )
}
