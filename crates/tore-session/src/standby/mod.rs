//! The standby's side of host migration (stage K; docs/ARCHITECTURE.md,
//! "Standbys", "On the standby's side"): [`Standby`], a state machine that
//! takes the standby stream's records in order
//! ([`crate::journal::StreamReader`]), builds the fresh world and keeps a
//! spare built ahead, assembles and restores checkpoints ([`assemble`]),
//! replays ticks with [`crate::journal::apply_tick`] within a budget, keeps
//! the state parts, checks the host's hashes and reports its status; and
//! [`StandbyThread`], the worker thread that runs it in the game and in
//! `tore-bot`, never on the frame loop. A joined client passes the records on
//! unread ([`crate::Client::take_standby_records`]). Built by slice K2.
//!
//! How it holds the mission:
//!
//! - **Warm** (appointed warm): a copy of the host's world, stepped with each
//!   Ticks record as it arrives, a few ticks behind the host. It keeps no
//!   journal but the ticks it has not stepped yet. Each Check the host sends
//!   is compared with the copy's own hash at that tick.
//! - **Cold** (appointed cold, or warm without a copy): a base (the Flight's
//!   fresh world, or the newest checkpoint) and the journal since. It steps
//!   nothing until it takes over.
//!
//! *Agent decisions (K2)*, each in docs/ARCHITECTURE.md's K2 row:
//!
//! - A warm copy more than [`FALL_BEHIND_TICKS`] (2 seconds) behind the
//!   journal it holds is catching up, reported cold; if it loses another 2
//!   seconds it falls: it keeps its own checkpoint as a cold base (so it
//!   stays ready), drops the copy and asks for a checkpoint. The host's
//!   checkpoint, once whole, is restored at once and stepped up to the
//!   present, and the standby is warm again within [`CAUGHT_UP_TICKS`] of
//!   the journal's end. A burst of records it can step through does not
//!   make it fall.
//! - A Check that differs drops the copy and everything stepped from it: a
//!   base this machine stepped to would differ the same way. The standby is
//!   building (not ready) until a checkpoint arrives.
//! - A Check's tick is the world's [`World::tick`] when hashed, the tick the
//!   next step runs, as a checkpoint's tick is.
//! - A record that cannot be read breaks the stream, since every later
//!   seat input codes against the inputs before it: the standby reports
//!   itself behind until an Appoint or a Flight starts it afresh. A chunk
//!   that cannot be read only spoils its checkpoint, which is asked for
//!   again.
//! - Appointed in flight, the host sends Appoint and then a checkpoint, no
//!   Flight record: the standby builds the flight its game holds for the
//!   Appoint's mission ([`MissionKey`] with no spec hash), and the
//!   checkpoint's mission identity checks the build.

pub mod assemble;
pub mod takeover;
#[cfg(test)]
mod tests;
mod thread;

pub use assemble::Refusal;
pub use thread::StandbyThread;

use crate::journal::{
    Appoint, Check, FlightRecord, Part, Record, StatePart, StreamReader, Tick, Ticks, apply_tick,
    check_hash, drain,
};
use crate::wire::messages::EndReason;
use crate::wire::migration::{CheckResult, StandbyState, StandbyStatus};
use assemble::Assembly;
use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};
use tore_world::seats::{SeatId, SeatInput};
use tore_world::world::{TickOutput, World};

/// A warm copy this many ticks behind the journal it holds (2 seconds) falls
/// to cold ([architecture](../../../../docs/ARCHITECTURE.md#standbys)).
pub const FALL_BEHIND_TICKS: u64 = 240;
/// A copy restored from a checkpoint is warm again once it is within this
/// many ticks of the journal's end (half a second; agent decision, K2).
pub const CAUGHT_UP_TICKS: u64 = 60;
/// The most journal a standby holds since its base: 100 seconds, beyond a
/// cold standby's 10-second checkpoint cadence and its 8-second pacing.
/// Past it the standby drops its copy and base and asks for a checkpoint
/// (agent decision, K2).
pub const MAX_JOURNAL_TICKS: usize = 12_000;
/// Checks waiting for the copy to reach their tick, at most.
const MAX_PENDING_CHECKS: usize = 64;

/// Which mission a standby builds: the flight's (a Flight record's number
/// and spec hash), or, appointed in flight, the flight the game holds for
/// the Appoint's mission (no spec hash).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MissionKey {
    pub mission: u32,
    /// The FNV-1a 64 of the flight's spec text, when a Flight record named it.
    pub spec_hash: Option<u64>,
}

/// Builds the fresh world for a mission, as the player's game builds it
/// (`World::new(spec, resources, Seating::Open)` from the Mission and Flight
/// loadouts messages it holds). An error makes the standby unable to hold
/// the mission, which it reports.
pub type Builder = Box<dyn FnMut(&MissionKey) -> Result<World, String> + Send>;

/// How long [`Standby::step`] may run.
#[derive(Clone, Copy, Debug)]
pub enum Budget {
    /// At most this many ticks.
    Ticks(usize),
    /// Until this moment.
    Until(Instant),
}

/// What happened in a standby, for its game's net log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    Appointed {
        warm: bool,
    },
    Dismissed,
    /// A Flight record's world built: the journal starts at tick 0.
    Flight {
        mission: u32,
    },
    /// A checkpoint kept, and restored when warm.
    Checkpoint {
        tick: u64,
        restored: bool,
    },
    /// A checkpoint refused: asked for again, or the mission is foreign.
    Refused(String),
    /// The copy reached the journal's end after a restore: warm.
    Warm {
        tick: u64,
    },
    /// The copy fell behind and was dropped: cold until a checkpoint.
    FellBehind {
        tick: u64,
    },
    /// A Check differed: the copy is dropped until a checkpoint.
    Mismatch {
        tick: u64,
    },
    /// The journal outgrew [`MAX_JOURNAL_TICKS`]: a checkpoint is needed.
    Overflow,
    /// The stream cannot be read on, or the mission cannot be built.
    Broken(String),
    Ended(EndReason),
    Handover {
        last_tick: u32,
    },
}

/// Why a standby is behind (status state 3): the host dismisses it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Fault {
    /// A record could not be read or the journal has a gap.
    Broken(String),
    /// The mission could not be built, or is another than the host's.
    Mission(String),
}

/// What a standby holds its journal against when it has no copy.
#[derive(Debug)]
enum Base {
    /// The Flight record's fresh world, tick 0.
    Fresh,
    /// A checkpoint: the host's, or the copy's own when it fell behind.
    Checkpoint { tick: u64, bytes: Vec<u8> },
}

impl Base {
    fn tick(&self) -> u64 {
        match self {
            Self::Fresh => 0,
            Self::Checkpoint { tick, .. } => *tick,
        }
    }
}

/// A warm standby's copy of the host's world.
struct Copy {
    world: World,
    /// Restored from a checkpoint, or fallen more than [`FALL_BEHIND_TICKS`]
    /// behind, and not yet within [`CAUGHT_UP_TICKS`] of the journal's end:
    /// reported cold.
    catching_up: bool,
    /// The ticks it had not stepped when it began catching up.
    behind_from: u64,
}

/// Everything a standby hands the game that takes over (slice K4).
pub struct Takeover {
    pub appoint: Appoint,
    /// The flight's record, when a Flight began the copy.
    pub flight: Option<FlightRecord>,
    /// The mission replayed to the journal's end; `None` in the lobby.
    pub world: Option<World>,
    /// T: the tick the next step runs, the world's [`World::tick`] (0 in the
    /// lobby).
    pub tick: u64,
    /// The newest state part of each kind.
    pub parts: BTreeMap<Part, StatePart>,
    /// Each seat's last recorded input and the number of its last command
    /// applied, as of the journal's last tick (an absent seat flies on them).
    pub last_inputs: BTreeMap<SeatId, (SeatInput, u16)>,
    /// The last tick a handing-over host stepped, when it handed over.
    pub handover: Option<u32>,
    /// How the last mission ended, when the standby is in the lobby after one.
    pub ended: Option<EndReason>,
    /// The ticks the takeover replayed, and how long the restore and replay
    /// took.
    pub replayed: u64,
    pub took: Duration,
}

impl std::fmt::Debug for Takeover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Takeover")
            .field("appoint", &self.appoint)
            .field("flight", &self.flight)
            .field("world", &self.world.as_ref().map(|_| "World"))
            .field("tick", &self.tick)
            .field("parts", &self.parts.keys().collect::<Vec<_>>())
            .field("seats", &self.last_inputs.keys().collect::<Vec<_>>())
            .field("handover", &self.handover)
            .field("ended", &self.ended)
            .field("replayed", &self.replayed)
            .field("took", &self.took)
            .finish()
    }
}

/// The standby's state machine. Feed it the stream's records in order with
/// [`Standby::receive`], give it time with [`Standby::step`] and
/// [`Standby::prepare`], report [`Standby::status`], and hand everything to
/// a takeover with [`Standby::take_over`].
pub struct Standby {
    builder: Builder,
    reader: StreamReader,
    appoint: Option<Appoint>,
    /// In flight: a Flight, a checkpoint or ticks since the Appoint or the
    /// last Ended.
    flying: bool,
    flight: Option<FlightRecord>,
    key: Option<MissionKey>,
    /// The mission identity of the standby's own build, once built.
    identity: Option<u64>,
    copy: Option<Copy>,
    /// A fresh world built ahead, so a restore never waits for a build.
    spare: Option<World>,
    base: Option<Base>,
    /// The ticks after the copy (not yet stepped) or after the base; with
    /// neither, the newest window of them.
    journal: VecDeque<Tick>,
    /// The tick after the journal's last: the next a Ticks record must start
    /// at.
    next_tick: Option<u64>,
    assembly: Option<Assembly>,
    checks: VecDeque<Check>,
    parts: BTreeMap<Part, StatePart>,
    last_inputs: BTreeMap<SeatId, (SeatInput, u16)>,
    last_check: (u32, CheckResult),
    needs_checkpoint: bool,
    fault: Option<Fault>,
    ended: Option<EndReason>,
    handover: Option<u32>,
    /// The mean cost of a replayed tick, microseconds (a moving mean).
    step_micros: f64,
    out: TickOutput,
    notes: Vec<Note>,
}

impl std::fmt::Debug for Standby {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Standby")
            .field("status", &self.status())
            .field("fault", &self.fault)
            .finish_non_exhaustive()
    }
}

impl Standby {
    /// A game not yet appointed, building its worlds with `builder`.
    pub fn new(builder: Builder) -> Self {
        Self {
            builder,
            reader: StreamReader::new(),
            appoint: None,
            flying: false,
            flight: None,
            key: None,
            identity: None,
            copy: None,
            spare: None,
            base: None,
            journal: VecDeque::new(),
            next_tick: None,
            assembly: None,
            checks: VecDeque::new(),
            parts: BTreeMap::new(),
            last_inputs: BTreeMap::new(),
            last_check: (0, CheckResult::None),
            needs_checkpoint: false,
            fault: None,
            ended: None,
            handover: None,
            step_micros: 0.,
            out: TickOutput::default(),
            notes: Vec::new(),
        }
    }

    /// The appointment, while appointed.
    pub fn appointed(&self) -> Option<Appoint> {
        self.appoint
    }

    /// Whether the standby could take over now: appointed, not behind, and
    /// in the lobby or holding a copy or a base with the journal since.
    pub fn ready(&self) -> bool {
        self.appoint.is_some()
            && self.fault.is_none()
            && (!self.flying || self.copy.is_some() || self.base.is_some())
    }

    /// The last tick a handing-over host steps, once its Handover arrived.
    pub fn handover(&self) -> Option<u32> {
        self.handover
    }

    /// The tick a takeover now would hold the world before: the journal's
    /// end (0 with none).
    pub fn newest(&self) -> u64 {
        self.next_tick.unwrap_or(0)
    }

    /// The ticks received but not yet stepped by the warm copy, or held since
    /// the base.
    pub fn backlog(&self) -> usize {
        self.journal.len()
    }

    /// The newest state part of `part`.
    pub fn part(&self, part: Part) -> Option<&StatePart> {
        self.parts.get(&part)
    }

    /// The notes since the last call.
    pub fn take_notes(&mut self) -> Vec<Note> {
        std::mem::take(&mut self.notes)
    }

    /// What the standby reports to the host (message 47).
    pub fn status(&self) -> StandbyStatus {
        let state = if self.fault.is_some() {
            StandbyState::Behind
        } else if !self.ready() {
            StandbyState::Building
        } else if !self.flying {
            match self.appoint {
                Some(Appoint { warm: true, .. }) => StandbyState::Warm,
                _ => StandbyState::Cold,
            }
        } else {
            match &self.copy {
                Some(copy) if !copy.catching_up => StandbyState::Warm,
                _ => StandbyState::Cold,
            }
        };
        StandbyStatus {
            newest_tick: self.newest().min(u64::from(u32::MAX)) as u32,
            state,
            step_micros: self.step_micros.round().min(f64::from(u32::MAX)) as u32,
            check_tick: self.last_check.0,
            check: self.last_check.1,
            needs_checkpoint: self.needs_checkpoint,
        }
    }

    /// Takes one record's bytes, in the order the host sent them. A record
    /// that cannot be read is refused: a chunk spoils only its checkpoint,
    /// anything else breaks the stream until an Appoint or a Flight.
    pub fn receive(&mut self, bytes: &[u8]) -> Result<(), String> {
        let record = match self.reader.decode(bytes) {
            Ok(record) => record,
            Err(error) => {
                let why = format!("an unreadable standby record: {error}");
                if bytes.first().is_some_and(|b| b & 0x0F == 4) && self.appoint.is_some() {
                    self.refuse_checkpoint(why.clone());
                } else {
                    self.break_stream(why.clone());
                }
                return Err(why);
            }
        };
        self.apply(record);
        Ok(())
    }

    /// Applies one record already read. Records before an Appoint are
    /// ignored.
    pub fn apply(&mut self, record: Record) {
        match record {
            Record::Appoint(appoint) => self.appoint_as(appoint),
            _ if self.appoint.is_none() => {}
            Record::Dismiss => {
                self.reset_flight();
                self.appoint = None;
                self.key = None;
                self.parts.clear();
                self.fault = None;
                self.handover = None;
                self.ended = None;
                self.last_check = (0, CheckResult::None);
                self.notes.push(Note::Dismissed);
            }
            Record::Flight(flight) => self.start_flight(flight),
            Record::CheckpointBegin(begin) => {
                if self.fault.is_some() {
                    return;
                }
                self.flying = true;
                if let Err(why) = self.ensure_identity() {
                    self.fault(Fault::Mission(why));
                    return;
                }
                match Assembly::begin(begin) {
                    Ok(assembly) => self.assembly = Some(assembly),
                    Err(refusal) => self.refuse_checkpoint(refusal.to_string()),
                }
            }
            Record::CheckpointChunk(chunk) => {
                if self.fault.is_some() {
                    return;
                }
                let Some(assembly) = &mut self.assembly else {
                    self.refuse_checkpoint("a chunk with no checkpoint begun".into());
                    return;
                };
                let tick = assembly.tick();
                match assembly.add(chunk) {
                    Ok(None) => {}
                    Ok(Some(bytes)) => {
                        self.assembly = None;
                        self.whole_checkpoint(tick, bytes);
                    }
                    Err(refusal) => self.refuse_checkpoint(refusal.to_string()),
                }
            }
            Record::Ticks(ticks) => self.take_ticks(ticks),
            Record::State(part) => {
                self.parts.insert(part.part, part);
            }
            Record::Check(check) => self.take_check(check),
            Record::Ended(reason) => {
                self.reset_flight();
                self.fault = None;
                self.ended = Some(reason);
                self.notes.push(Note::Ended(reason));
            }
            Record::Handover { last_tick } => {
                self.handover = Some(last_tick);
                self.notes.push(Note::Handover { last_tick });
            }
        }
    }

    fn appoint_as(&mut self, appoint: Appoint) {
        self.reset_flight();
        self.appoint = Some(appoint);
        self.key = Some(MissionKey {
            mission: appoint.mission,
            spec_hash: None,
        });
        self.parts.clear();
        self.fault = None;
        self.handover = None;
        self.ended = None;
        self.last_check = (0, CheckResult::None);
        self.notes.push(Note::Appointed { warm: appoint.warm });
    }

    /// Drops everything the flight held; the appointment and the parts stay.
    fn reset_flight(&mut self) {
        self.flying = false;
        self.flight = None;
        self.identity = None;
        self.copy = None;
        self.spare = None;
        self.base = None;
        self.journal.clear();
        self.next_tick = None;
        self.assembly = None;
        self.checks.clear();
        self.last_inputs.clear();
        self.needs_checkpoint = false;
    }

    fn warm(&self) -> bool {
        self.appoint.is_some_and(|a| a.warm)
    }

    fn fault(&mut self, fault: Fault) {
        let why = match &fault {
            Fault::Broken(why) | Fault::Mission(why) => why.clone(),
        };
        self.notes.push(Note::Broken(why));
        self.copy = None;
        self.base = None;
        self.assembly = None;
        self.checks.clear();
        self.fault = Some(fault);
    }

    fn break_stream(&mut self, why: String) {
        if self.appoint.is_some() {
            self.fault(Fault::Broken(why));
        }
    }

    fn refuse_checkpoint(&mut self, why: String) {
        self.assembly = None;
        self.needs_checkpoint = true;
        self.notes.push(Note::Refused(why));
    }

    fn start_flight(&mut self, flight: FlightRecord) {
        self.reset_flight();
        self.fault = None;
        self.handover = None;
        self.ended = None;
        self.flying = true;
        self.flight = Some(flight);
        self.key = Some(MissionKey {
            mission: flight.mission,
            spec_hash: Some(flight.spec_hash),
        });
        let world = match self.build() {
            Ok(world) => world,
            Err(why) => {
                self.fault(Fault::Mission(why));
                return;
            }
        };
        let ours = world.mission_identity();
        if ours != flight.identity {
            self.fault(Fault::Mission(format!(
                "the flight's mission identity {:016x} is not this game's build, {ours:016x}",
                flight.identity
            )));
            return;
        }
        self.identity = Some(ours);
        self.next_tick = Some(world.tick());
        if self.warm() {
            self.copy = Some(Copy {
                world,
                catching_up: false,
                behind_from: 0,
            });
        } else {
            self.spare = Some(world);
            self.base = Some(Base::Fresh);
        }
        self.notes.push(Note::Flight {
            mission: flight.mission,
        });
    }

    /// Builds the mission with the builder, unchecked, making contrail
    /// puffs as the host's world does.
    fn build(&mut self) -> Result<World, String> {
        let key = self.key.ok_or_else(|| "no mission to build".to_owned())?;
        let mut world = (self.builder)(&key)
            .map_err(|why| format!("cannot build mission {}: {why}", key.mission))?;
        if let Some(flight) = &self.flight {
            world.combat.set_contrail_puffs(flight.contrail_puffs);
        }
        Ok(world)
    }

    /// Knows the identity of its own build, building the spare if it must.
    fn ensure_identity(&mut self) -> Result<u64, String> {
        if let Some(identity) = self.identity {
            return Ok(identity);
        }
        let world = self.build()?;
        let identity = world.mission_identity();
        self.identity = Some(identity);
        self.spare = Some(world);
        Ok(identity)
    }

    /// A fresh world of the mission: the spare, or one built now.
    fn fresh(&mut self) -> Result<World, String> {
        if let Some(world) = self.spare.take() {
            return Ok(world);
        }
        let world = self.build()?;
        if let Some(identity) = self.identity
            && world.mission_identity() != identity
        {
            return Err("the mission built differs from the one held".into());
        }
        Ok(world)
    }

    fn whole_checkpoint(&mut self, tick: u64, bytes: Vec<u8>) {
        let identity = match self.ensure_identity() {
            Ok(identity) => identity,
            Err(why) => {
                self.fault(Fault::Mission(why));
                return;
            }
        };
        if let Err(refusal) = assemble::check(&bytes, tick, identity) {
            match refusal {
                Refusal::Foreign { .. } => self.fault(Fault::Mission(refusal.to_string())),
                Refusal::Damaged(_) => self.refuse_checkpoint(refusal.to_string()),
            }
            return;
        }
        if let Some(copy) = &self.copy
            && (copy.world.tick() >= tick || !copy.catching_up)
        {
            // A copy in good standing, or already past it: nothing to gain.
            // Judged before the journal, which such a copy has stepped and
            // dropped up to its own tick: a checkpoint it does not need is
            // not refused, so it does not ask for another.
            self.needs_checkpoint = false;
            return;
        }
        // The journal must run on from the checkpoint's tick.
        let from = self.journal_from();
        match self.next_tick {
            None => self.next_tick = Some(tick),
            Some(next) if from <= tick && tick <= next => {}
            Some(next) => {
                self.refuse_checkpoint(format!(
                    "a checkpoint at tick {tick} the journal ({from} to {next}) does not continue"
                ));
                return;
            }
        }
        let restored = if self.warm() {
            let mut world = match self.fresh() {
                Ok(world) => world,
                Err(why) => {
                    self.fault(Fault::Mission(why));
                    return;
                }
            };
            if let Err(error) = world.restore(&bytes) {
                // A half-restored world is thrown away.
                self.refuse_checkpoint(format!("the checkpoint did not restore: {error}"));
                return;
            }
            self.trim_journal(tick);
            self.copy = Some(Copy {
                world,
                catching_up: true,
                behind_from: self.journal.len() as u64,
            });
            self.base = None;
            true
        } else {
            self.trim_journal(tick);
            self.base = Some(Base::Checkpoint { tick, bytes });
            false
        };
        self.needs_checkpoint = false;
        self.notes.push(Note::Checkpoint { tick, restored });
        // A restore with little journal after it is warm at once.
        self.judge_lag();
    }

    /// Drops the journal's ticks before `tick`, and the checks before it.
    fn trim_journal(&mut self, tick: u64) {
        while self.journal.front().is_some_and(|t| t.tick < tick) {
            self.journal.pop_front();
        }
        self.checks.retain(|check| u64::from(check.tick) >= tick);
    }

    /// The tick the journal's first entry is, or would be.
    fn journal_from(&self) -> u64 {
        match self.journal.front() {
            Some(tick) => tick.tick,
            None => self.next_tick.unwrap_or(0),
        }
    }

    fn take_ticks(&mut self, ticks: Ticks) {
        if self.fault.is_some() {
            return;
        }
        self.flying = true;
        let first = u64::from(ticks.first);
        match self.next_tick {
            Some(next) if next != first => {
                self.fault(Fault::Broken(format!(
                    "the journal jumps from tick {next} to {first}"
                )));
                return;
            }
            _ => {}
        }
        for tick in ticks.ticks {
            for (input, applied) in tick.inputs.iter().zip(&tick.applied) {
                self.last_inputs
                    .insert(input.seat, (input.clone(), *applied));
            }
            self.next_tick = Some(tick.tick + 1);
            self.journal.push_back(tick);
        }
        if self.journal.len() > MAX_JOURNAL_TICKS {
            if self.copy.is_some() || self.base.is_some() {
                self.copy = None;
                self.base = None;
                self.needs_checkpoint = true;
                self.notes.push(Note::Overflow);
            }
            while self.journal.len() > MAX_JOURNAL_TICKS {
                self.journal.pop_front();
            }
        }
    }

    fn take_check(&mut self, check: Check) {
        if self.fault.is_some() || !self.warm() {
            return;
        }
        let tick = u64::from(check.tick);
        match &self.copy {
            Some(copy) if copy.world.tick() == tick => self.compare(check),
            Some(copy) if copy.world.tick() > tick => {}
            _ => {
                if self.checks.len() == MAX_PENDING_CHECKS {
                    self.checks.pop_front();
                }
                self.checks.push_back(check);
            }
        }
    }

    /// Compares the copy's hash with a Check at the copy's tick.
    fn compare(&mut self, check: Check) {
        let Some(copy) = &self.copy else {
            return;
        };
        let equal = check_hash(&copy.world).is_ok_and(|hash| hash == check.hash);
        if equal {
            self.last_check = (check.tick, CheckResult::Equal);
            return;
        }
        self.last_check = (check.tick, CheckResult::Different);
        self.notes.push(Note::Mismatch {
            tick: u64::from(check.tick),
        });
        self.copy = None;
        self.base = None;
        self.checks.clear();
        self.needs_checkpoint = true;
    }

    /// Steps the warm copy along the journal within `budget`, compares the
    /// checks it reaches, and decides warm or cold: a copy in good standing
    /// [`FALL_BEHIND_TICKS`] behind falls to cold; one catching up is warm
    /// again within [`CAUGHT_UP_TICKS`]. Returns the ticks stepped.
    pub fn step(&mut self, budget: Budget) -> usize {
        let mut stepped = 0;
        loop {
            match budget {
                Budget::Ticks(most) if stepped >= most => break,
                Budget::Until(end) if Instant::now() >= end => break,
                _ => {}
            }
            let Some(copy) = &mut self.copy else {
                return stepped;
            };
            let Some(tick) = self.journal.pop_front() else {
                break;
            };
            let started = Instant::now();
            let result = apply_tick(&mut copy.world, &tick, &mut self.out);
            let _ = drain(&mut copy.world);
            let now = copy.world.tick();
            let micros = started.elapsed().as_secs_f64() * 1e6;
            self.step_micros += (micros - self.step_micros) / 64.;
            stepped += 1;
            if let Err(error) = result {
                // The copy refused what the host stepped: it is not the host's.
                self.notes.push(Note::Broken(format!(
                    "the copy refused tick {}: {error}",
                    tick.tick
                )));
                self.copy = None;
                self.checks.clear();
                self.needs_checkpoint = true;
                return stepped;
            }
            while let Some(check) = self.checks.front().copied() {
                if u64::from(check.tick) < now {
                    self.checks.pop_front();
                } else if u64::from(check.tick) == now {
                    self.checks.pop_front();
                    self.compare(check);
                } else {
                    break;
                }
            }
        }
        self.judge_lag();
        stepped
    }

    /// Warm or cold by the ticks the copy has not stepped: a warm copy more
    /// than [`FALL_BEHIND_TICKS`] behind is catching up (reported cold); one
    /// catching up is warm again within [`CAUGHT_UP_TICKS`], and falls when
    /// it has lost another [`FALL_BEHIND_TICKS`] since it began catching up.
    fn judge_lag(&mut self) {
        let lag = self.journal.len() as u64;
        let Some(copy) = &mut self.copy else {
            return;
        };
        let tick = copy.world.tick();
        match copy.catching_up {
            false if lag > FALL_BEHIND_TICKS => {
                copy.catching_up = true;
                copy.behind_from = lag;
            }
            true if lag <= CAUGHT_UP_TICKS => {
                copy.catching_up = false;
                self.notes.push(Note::Warm { tick });
            }
            true if lag > copy.behind_from + FALL_BEHIND_TICKS => self.fall_behind(),
            _ => {}
        }
    }

    /// The copy cannot keep up: keep its own checkpoint as a cold base (it
    /// stays ready), drop the copy, and ask the host for a checkpoint to come
    /// back warm.
    fn fall_behind(&mut self) {
        let Some(copy) = self.copy.take() else {
            return;
        };
        let tick = copy.world.tick();
        self.base = copy
            .world
            .checkpoint()
            .ok()
            .map(|bytes| Base::Checkpoint { tick, bytes });
        self.checks.clear();
        self.needs_checkpoint = true;
        self.notes.push(Note::FellBehind { tick });
    }

    /// Whether [`Standby::step`] has ticks to step.
    pub fn has_work(&self) -> bool {
        self.copy.is_some() && !self.journal.is_empty()
    }

    /// Builds the spare fresh world when the flight has none, so a restore
    /// never waits for a build (40 to 80 ms). Call it when idle. Returns
    /// whether it built one.
    pub fn prepare(&mut self) -> bool {
        if !self.flying || self.spare.is_some() || self.fault.is_some() || self.key.is_none() {
            return false;
        }
        match self.fresh_build() {
            Ok(world) => {
                self.spare = Some(world);
                true
            }
            Err(why) => {
                self.fault(Fault::Mission(why));
                false
            }
        }
    }

    fn fresh_build(&mut self) -> Result<World, String> {
        let world = self.build()?;
        let identity = world.mission_identity();
        match self.identity {
            Some(ours) if ours != identity => {
                Err("the mission built differs from the one held".into())
            }
            _ => {
                self.identity = Some(identity);
                Ok(world)
            }
        }
    }

    /// The world a takeover would hold now, leaving the standby as it was:
    /// the copy's (through its own checkpoint) or the base's, restored into a
    /// fresh world and replayed to the journal's end. For tests and for
    /// checking a standby by hand.
    pub fn replica(&mut self) -> Result<World, String> {
        if !self.ready() || !self.flying {
            return Err("the standby holds no mission".into());
        }
        let (bytes, from) = match (&self.copy, &self.base) {
            (Some(copy), _) => (
                Some(copy.world.checkpoint().map_err(|e| e.to_string())?),
                copy.world.tick(),
            ),
            (None, Some(Base::Checkpoint { tick, bytes })) => (Some(bytes.clone()), *tick),
            (None, Some(Base::Fresh)) => (None, 0),
            (None, None) => return Err("the standby holds no mission".into()),
        };
        let mut world = self.fresh()?;
        if let Some(bytes) = bytes {
            world.restore(&bytes).map_err(|e| e.to_string())?;
        }
        debug_assert_eq!(world.tick(), from);
        let mut out = TickOutput::default();
        for tick in &self.journal {
            apply_tick(&mut world, tick, &mut out).map_err(|e| e.to_string())?;
            let _ = drain(&mut world);
        }
        Ok(world)
    }

    /// Hands everything to a takeover: replays what it holds to the
    /// journal's end (a cold standby restores its base first) and gives the
    /// world, the parts and each seat's last input. Refused when not ready.
    pub fn take_over(mut self) -> Result<Takeover, String> {
        let Some(appoint) = self.appoint else {
            return Err("not a standby".into());
        };
        if let Some(Fault::Broken(why) | Fault::Mission(why)) = &self.fault {
            return Err(format!("the standby is behind: {why}"));
        }
        if !self.ready() {
            return Err("the standby holds no world yet".into());
        }
        let started = Instant::now();
        let mut replayed = 0;
        let world = if self.flying {
            let mut world = match (self.copy.take(), self.base.take()) {
                (Some(copy), _) => copy.world,
                (None, Some(base)) => {
                    let mut world = self.fresh()?;
                    if let Base::Checkpoint { bytes, .. } = &base {
                        world.restore(bytes).map_err(|e| e.to_string())?;
                    }
                    if world.tick() != base.tick() {
                        return Err("the base restored to another tick".into());
                    }
                    world
                }
                (None, None) => return Err("the standby holds no world yet".into()),
            };
            let mut out = TickOutput::default();
            for tick in self.journal.drain(..) {
                apply_tick(&mut world, &tick, &mut out).map_err(|e| e.to_string())?;
                let _ = drain(&mut world);
                replayed += 1;
            }
            Some(world)
        } else {
            None
        };
        Ok(Takeover {
            appoint,
            flight: self.flight,
            tick: world.as_ref().map_or(0, World::tick),
            world,
            parts: self.parts,
            last_inputs: self.last_inputs,
            handover: self.handover,
            ended: self.ended,
            replayed,
            took: started.elapsed(),
        })
    }
}
