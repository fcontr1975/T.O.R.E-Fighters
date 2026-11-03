//! The host's standby stream (stage K; docs/ARCHITECTURE.md, "Standbys"),
//! built by slice K3: appointing and dismissing up to two standbys, the
//! journal's records out to each on its own connection, the state parts
//! after their tick, a Check every 600 ticks to the warm ones, a checkpoint
//! every 1,200 ticks to the cold ones, the pacing of those checkpoints, each
//! standby's status in, the resync a standby asks for, and a leaving standby
//! replaced.
//!
//! The host calls [`Host::standby_update`] at the end of every update, after
//! the ticks and the parts they changed. It drains the journal (slice K1)
//! and gives each standby its records in order:
//!
//! - **Ticks** stepped in an update go out at its end as one Ticks record
//!   (at most [`TICKS_PER_RECORD`] a record), in the same send as the
//!   snapshots of those ticks; any other record sends the waiting ticks
//!   first, so the stream keeps the journal's order. (Until the battery
//!   fixes of 2026-10-20 they waited for a snapshot interval of them, so a
//!   host killed between two updates could take the last tick it stepped,
//!   and showed its players, with it.)
//! - **State, Flight and Ended** go out as they come. A Flight or an Ended
//!   drops a checkpoint still going out: it belongs to the flight before.
//! - **Checks** go to the warm standbys at the first update at or after each
//!   600th tick, with the hash of the world between ticks then, named by the
//!   world's `tick()`.
//! - **Checkpoints** go to a standby appointed in flight, to a cold one at the
//!   first update 1,200 ticks after its last, and to one that asks for one (a
//!   failed check, a fall behind). A checkpoint goes in 4 KB chunks at a
//!   paced rate, at most four unacknowledged; Ticks and State records are
//!   never paced.
//!
//! *Agent decisions (K3)*, each in docs/ARCHITECTURE.md's K3 row:
//!
//! - **Off until asked.** A host appoints no standby until its caller
//!   switches them on ([`Host::set_standbys_enabled`]): a game that cannot
//!   run a standby yet would keep the stream unread.
//! - **Who.** The standbys are slice K6's best candidates
//!   (`Host::ranked_candidates`: in a game a player hosts, not the house,
//!   not relayed, its switch on, reached by every other direct player), less
//!   any leaving or dismissed as behind in this flight. A standby is kept
//!   while it may stay (`Host::may_stay_standby`: as eligible, but a player
//!   not yet in a reach test does not count against it, so a join does not
//!   dismiss the standbys); a free role is filled from the ranking. A
//!   remaining standby keeps the role it was appointed with: appointing it
//!   again would cost it a checkpoint and leave it not ready meanwhile.
//! - **Warm or cold.** A standby is warm when its Candidate report's class
//!   (platform and processor) is the host's; the checks find any difference
//!   within 5 seconds, and after [`MISMATCHES_TO_COLD`] failed checks the
//!   standby is appointed again, cold.
//! - **Succession.** Whenever the standbys or their readiness change, the
//!   ready ones go to K6's `send_succession`, which sends every player the
//!   Succession when it differs from the last.
//! - **Pacing.** The rate is the checkpoint's bytes over 8 seconds, at least
//!   32,000 and at most [`MAX_RATE`] bytes a second less the rest of the
//!   stream's bytes in the last second, so the whole stream with the
//!   transport's framing stays within John's 1 Mbit/s whenever the journal
//!   leaves room for the slowest pace. "Unacknowledged" is
//!   read from the transport's count of the connection's queued fragments,
//!   since the transport names no message's delivery.
//! - **Room on the wire.** In flight a player's packets carry 256 bytes of
//!   reliable messages each; the stream asks the transport for packets of
//!   messages alone, one for each fragment it queued and at most
//!   [`EXTRA_PACKETS`] an update, while the standby's connection has
//!   messages due, so the stream is not held to the snapshot packets'
//!   share.
//! - **Status.** A status from a game that is not a standby is ignored (a
//!   dismissed standby's last one may cross its Dismiss). A standby that
//!   reports itself behind is dismissed and not appointed again until the
//!   next flight. A checkpoint it asks for goes out at once, unless one is
//!   still going out to it or finished less than [`RESYNC_WAIT`] ago.
//! - **The host's log and status line.** [`Host::standby_figures`] gives each
//!   standby's role, state, checks and stream bytes; the printers belong to
//!   the game and the server (slices K7a and K9).

use super::journal::JournalRecord;
use super::{ConnectionId, Host, HostLog, Life, Stage};
use crate::journal::{
    Appoint, CHECK_EVERY_TICKS, CHECKPOINT_EVERY_TICKS, Check, CheckpointBegin, CheckpointChunk,
    Record, StatePart, StreamWriter, Tick, Ticks, check_hash,
};
use crate::wire::Platform;
use crate::wire::messages::{Message, StandbyMark};
use crate::wire::migration::limits::{CHUNK_BYTES, STANDBYS, TICKS_PER_RECORD};
use crate::wire::migration::{CheckResult, Processor, StandbyState, StandbyStatus};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
#[path = "standby_tests.rs"]
mod tests;

/// A checkpoint's pace: its bytes over this many seconds.
pub const PACE_SECONDS: f64 = 8.;
/// The slowest pace, bytes a second.
pub const MIN_RATE: f64 = 32_000.;
/// The most a standby's stream sends, bytes a second: 1 Mbit/s (John,
/// 2026-10-05).
pub const LINE_RATE: f64 = 125_000.;
/// The transport's framing of a stream's fragments, and its resends: about
/// 12 percent of framing with one fragment a packet, as in flight, and the
/// rest resends (measured, slice K3). 1.2 let the real 15 against 15's busiest
/// second reach 126 KB/s against the 125,000 B/s line, so it is 1.25 (slice
/// KP).
pub const FRAMING: f64 = 1.25;
/// The fastest pace, bytes of chunks a second: the line less its framing.
/// The rest of the stream in the last second comes off it, down to
/// [`MIN_RATE`].
pub const MAX_RATE: f64 = LINE_RATE / FRAMING;
/// Chunks of a checkpoint unacknowledged at once, at most.
pub const CHUNKS_IN_FLIGHT: usize = 4;
/// A resync checkpoint waits this long after the last went out, so a
/// status sent before that one arrived does not ask for another.
pub const RESYNC_WAIT: Duration = Duration::from_secs(2);
/// Failed checks after which a warm standby is appointed again, cold.
pub const MISMATCHES_TO_COLD: u32 = 2;
/// Packets of reliable messages alone the stream asks for, at most, in one
/// update for one standby.
pub const EXTRA_PACKETS: usize = 16;

/// The transport's fragment size: a message's body is cut into these.
const FRAGMENT: usize = 256;

/// The bytes a standby's stream has carried since it was appointed, by
/// record: each record's bytes as the Standby record message holds them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamBytes {
    /// Ticks records, and the ticks and seat inputs in them.
    pub ticks: u64,
    pub tick_count: u64,
    pub seat_inputs: u64,
    /// State records.
    pub states: u64,
    /// Check records.
    pub checks: u64,
    /// Checkpoint begin and chunk records, and the checkpoints begun.
    pub checkpoints: u64,
    pub checkpoint_count: u64,
    /// Appoint, Dismiss, Flight, Ended and Handover.
    pub other: u64,
}

impl StreamBytes {
    /// Every byte.
    pub fn total(&self) -> u64 {
        self.ticks + self.states + self.checks + self.checkpoints + self.other
    }

    fn add(&mut self, record: &Record, bytes: usize) {
        let bytes = bytes as u64;
        match record {
            Record::Ticks(ticks) => {
                self.ticks += bytes;
                self.tick_count += ticks.ticks.len() as u64;
                self.seat_inputs += ticks
                    .ticks
                    .iter()
                    .map(|t| t.inputs.len() as u64)
                    .sum::<u64>();
            }
            Record::State(_) => self.states += bytes,
            Record::Check(_) => self.checks += bytes,
            Record::CheckpointBegin(_) => {
                self.checkpoints += bytes;
                self.checkpoint_count += 1;
            }
            Record::CheckpointChunk(_) => self.checkpoints += bytes,
            Record::Appoint(_)
            | Record::Dismiss
            | Record::Flight(_)
            | Record::Ended(_)
            | Record::Handover { .. } => self.other += bytes,
        }
    }
}

/// One standby as the host sees it, for the host's log and status line.
#[derive(Clone, Debug, PartialEq)]
pub struct StandbyFigures {
    /// The player's lobby id and callsign.
    pub player: u8,
    pub callsign: String,
    /// First or second.
    pub role: StandbyMark,
    /// Appointed warm.
    pub warm: bool,
    /// Its last status, once one came.
    pub status: Option<StandbyStatus>,
    /// Checks it reported equal and different.
    pub checks_equal: u64,
    pub mismatches: u64,
    /// The stream's bytes since it was appointed, and how long ago that was.
    pub bytes: StreamBytes,
    pub appointed_for: Duration,
}

impl StandbyFigures {
    /// Ready by its last status: holding a world the journal continues from.
    pub fn ready(&self) -> bool {
        self.status
            .is_some_and(|s| matches!(s.state, StandbyState::Warm | StandbyState::Cold))
    }
}

/// A checkpoint going out to one standby, chunk by chunk.
#[derive(Debug)]
struct Outgoing {
    bytes: Arc<Vec<u8>>,
    /// The next chunk to send, of `chunks`.
    next: usize,
    chunks: usize,
    /// Bytes a second, and the bytes the pace allows now.
    rate: f64,
    allowance: f64,
    paced_at: Duration,
}

/// One standby's stream.
#[derive(Debug)]
struct Stream {
    connection: ConnectionId,
    /// The player's join order: its identity across connections.
    order: u64,
    role: StandbyMark,
    warm: bool,
    writer: StreamWriter,
    /// Ticks stepped in this update, waiting for its end.
    ticks: Vec<Tick>,
    outgoing: Option<Outgoing>,
    /// When the last checkpoint finished going out.
    sent_at: Option<Duration>,
    /// The tick of the last checkpoint begun, or of the Flight (0): a cold
    /// standby's next is due 1,200 ticks on. `None` with no flight.
    base_tick: Option<u64>,
    status: Option<StandbyStatus>,
    /// The newest check tick reported, and the counts.
    check_tick: u32,
    checks_equal: u64,
    mismatches: u64,
    /// Failed checks since it was appointed warm.
    warm_mismatches: u32,
    /// A status asked for a checkpoint not yet sent.
    asks_checkpoint: bool,
    /// The transport's fragments of the records queued since the last
    /// packets of messages alone.
    fragments: usize,
    /// The stream's bytes but the checkpoints': since `window_at`, and a
    /// second's worth before.
    window_at: Duration,
    window_bytes: usize,
    other_rate: f64,
    bytes: StreamBytes,
    appointed_at: Duration,
}

impl Stream {
    fn new(
        connection: ConnectionId,
        order: u64,
        role: StandbyMark,
        warm: bool,
        now: Duration,
    ) -> Self {
        Self {
            connection,
            order,
            role,
            warm,
            writer: StreamWriter::new(),
            ticks: Vec::new(),
            outgoing: None,
            sent_at: None,
            base_tick: None,
            status: None,
            check_tick: 0,
            checks_equal: 0,
            mismatches: 0,
            warm_mismatches: 0,
            asks_checkpoint: false,
            fragments: 0,
            window_at: now,
            window_bytes: 0,
            other_rate: 0.,
            bytes: StreamBytes::default(),
            appointed_at: now,
        }
    }

    fn ready(&self) -> bool {
        self.status
            .is_some_and(|s| matches!(s.state, StandbyState::Warm | StandbyState::Cold))
    }
}

/// The host's standbys and their streams.
#[derive(Debug, Default)]
pub(super) struct Standbys {
    /// The host appoints standbys at all ([`Host::set_standbys_enabled`]).
    enabled: bool,
    streams: Vec<Stream>,
    /// The next tick a Check is due at or after.
    next_check: u64,
    /// Join orders dismissed as behind: not appointed again until the next
    /// flight.
    excluded: BTreeSet<u64>,
    /// The standbys or their readiness changed since the last Succession.
    changed: bool,
    /// Tests: the next Check's hash is spoiled, as a standby whose machine
    /// computes differently would find it.
    #[cfg(test)]
    spoil_next_check: bool,
    /// Tests: every Succession this module asked for, its ready standbys.
    #[cfg(test)]
    successions: Vec<Vec<(ConnectionId, bool)>>,
}

impl Host {
    /// Lets the host appoint standbys, or not (off when a host starts):
    /// the game and `tore-bot` switch it on once their players' games run
    /// a standby (slices K7a and K9). Switched off, every standby is
    /// dismissed at the next update.
    pub fn set_standbys_enabled(&mut self, on: bool) {
        self.standbys.enabled = on;
    }

    /// Sets the host's AI test settings (observation rate, lean sensors):
    /// they apply from the next tick, and any standby is dismissed while they
    /// differ from the defaults (see [`Self::standbys_on`]).
    pub fn set_ai_test(&mut self, settings: tore_sim::ai::test_settings::AiTestSettings) {
        self.config.ai_test = settings;
    }

    /// The host's AI test settings.
    pub fn ai_test(&self) -> tore_sim::ai::test_settings::AiTestSettings {
        self.config.ai_test
    }

    /// Whether standbys are appointed now: switched on, and the AI test
    /// settings at their defaults. A standby replays the host's journal with
    /// the defaults, which the journal does not say otherwise, so a host
    /// running a slow AI observation rate or lean sensors (slice S7b) would
    /// only part from it; it keeps none.
    fn standbys_on(&self) -> bool {
        self.standbys.enabled && self.config.ai_test.is_default()
    }

    /// A standby's status (message 47): kept, and acted on at the next
    /// update. A status from a game that is not a standby is ignored.
    pub(super) fn standby_status(
        &mut self,
        connection: ConnectionId,
        status: StandbyStatus,
    ) -> Result<(), String> {
        let Some(index) = self
            .standbys
            .streams
            .iter()
            .position(|s| s.connection == connection)
        else {
            return Ok(());
        };
        let stream = &mut self.standbys.streams[index];
        let was_ready = stream.ready();
        if status.check_tick > stream.check_tick {
            stream.check_tick = status.check_tick;
            match status.check {
                CheckResult::Equal => stream.checks_equal += 1,
                CheckResult::Different => {
                    stream.mismatches += 1;
                    if stream.warm {
                        stream.warm_mismatches += 1;
                    }
                }
                CheckResult::None => {}
            }
        }
        stream.status = Some(status);
        // The newest status says whether a checkpoint is still wanted: one
        // sent while a resync was on its way, and answered by it, asks for
        // nothing once a later status says so (the battery's planned
        // handover, 2026-10-24: kept from the older status, the ask sent a
        // second checkpoint the restored copy had no use for).
        stream.asks_checkpoint = status.needs_checkpoint;
        if stream.ready() != was_ready {
            self.standbys.changed = true;
            self.lobby_dirty = true;
        }
        // Acted on at the next update, after the journal's records before
        // it have gone out.
        Ok(())
    }

    /// What the standbys' statuses ask for: one behind is dismissed and not
    /// appointed again this flight, one failing its checks goes cold, one
    /// that needs a checkpoint gets one.
    fn act_on_statuses(&mut self) {
        let flying = matches!(self.life, Life::Flying);
        let now = self.now;
        let mut index = 0;
        while index < self.standbys.streams.len() {
            let stream = &mut self.standbys.streams[index];
            if stream
                .status
                .is_some_and(|s| s.state == StandbyState::Behind)
            {
                // It cannot hold this flight.
                let order = stream.order;
                self.standbys.excluded.insert(order);
                self.dismiss(index);
                continue;
            }
            if stream.warm && stream.warm_mismatches >= MISMATCHES_TO_COLD {
                // Its machine does not compute as the host's: cold from here.
                self.reappoint(index, false);
            } else if stream.asks_checkpoint && stream.outgoing.is_none() {
                let waited = stream
                    .sent_at
                    .is_none_or(|at| now.saturating_sub(at) >= RESYNC_WAIT);
                if waited {
                    stream.asks_checkpoint = false;
                    if flying {
                        self.begin_checkpoint(&[index]);
                    }
                }
            }
            index += 1;
        }
    }

    /// The standby stream's work for this update: the journal drained into
    /// each stream, the standbys appointed and dismissed, the checks and the
    /// checkpoints due, the paced chunks, and packets for the stream's
    /// messages. Called at the end of every update.
    pub(super) fn standby_update(&mut self) {
        if self.standbys.streams.is_empty() {
            // No stream to feed: the journal is not this module's until a
            // standby is appointed (a test may be reading it).
            self.appoint_standbys();
            if self.standbys.streams.is_empty() {
                return;
            }
        }
        let (records, lost) = self.drain_journal();
        if lost {
            // The journal dropped records nobody drained: every standby
            // starts again from a checkpoint.
            for index in 0..self.standbys.streams.len() {
                let warm = self.standbys.streams[index].warm;
                self.reappoint(index, warm);
            }
        } else {
            for record in records {
                self.distribute(record);
            }
        }
        self.act_on_statuses();
        self.appoint_standbys();
        if matches!(self.life, Life::Flying) {
            self.checks_due();
            self.cold_checkpoints_due();
        }
        for index in 0..self.standbys.streams.len() {
            // Every tick stepped in this update goes out in it, with the
            // snapshots of those ticks: a standby never lacks a tick a
            // player was shown, so a host killed between two updates loses
            // none of its stepped ticks.
            self.flush_ticks(index);
            self.pace(index);
            self.extra_packets(index);
        }
        // Every player hears of the ready standbys when they change (slice
        // K6's Succession).
        if std::mem::take(&mut self.standbys.changed) {
            let ready = self.ready_standbys();
            #[cfg(test)]
            self.standbys.successions.push(ready.clone());
            self.send_succession(&ready);
        }
    }

    /// One journal record to every standby, in order.
    fn distribute(&mut self, record: JournalRecord) {
        for index in 0..self.standbys.streams.len() {
            match &record {
                JournalRecord::Tick(tick) => {
                    let stream = &mut self.standbys.streams[index];
                    stream.ticks.push(tick.clone());
                    if stream.ticks.len() >= TICKS_PER_RECORD {
                        self.flush_ticks(index);
                    }
                }
                JournalRecord::State(part) => {
                    self.flush_ticks(index);
                    self.stream_record(index, &Record::State(part.clone()));
                }
                JournalRecord::Flight(flight) => {
                    self.flush_ticks(index);
                    let stream = &mut self.standbys.streams[index];
                    stream.outgoing = None;
                    stream.base_tick = Some(0);
                    self.stream_record(index, &Record::Flight(*flight));
                }
                JournalRecord::Ended(reason) => {
                    self.flush_ticks(index);
                    let stream = &mut self.standbys.streams[index];
                    stream.outgoing = None;
                    stream.base_tick = None;
                    self.stream_record(index, &Record::Ended(*reason));
                }
            }
        }
        match record {
            JournalRecord::Flight(_) => {
                // A new flight: every game may stand by again.
                self.standbys.excluded.clear();
                self.standbys.next_check = 0;
            }
            JournalRecord::Ended(_) => self.standbys.next_check = 0,
            JournalRecord::Tick(_) | JournalRecord::State(_) => {}
        }
    }

    /// Sends a stream's waiting ticks as one Ticks record.
    fn flush_ticks(&mut self, index: usize) {
        let ticks = std::mem::take(&mut self.standbys.streams[index].ticks);
        let Some(first) = ticks.first() else {
            return;
        };
        let Ok(first) = u32::try_from(first.tick) else {
            return;
        };
        self.stream_record(index, &Record::Ticks(Ticks { first, ticks }));
    }

    /// Codes `record` in a stream and queues it to its standby. A record
    /// the writer refuses is a fault in the host: logged and left out (a
    /// standby missing a Ticks record finds the gap and reports itself
    /// behind).
    fn stream_record(&mut self, index: usize, record: &Record) {
        let stream = &mut self.standbys.streams[index];
        let bytes = match stream.writer.encode(record) {
            Ok(bytes) => bytes,
            Err(error) => {
                let tick = self.world.tick();
                self.log(HostLog::Fault {
                    tick,
                    text: format!("standby record of type {}: {error}", record.code()),
                });
                return;
            }
        };
        stream.bytes.add(record, bytes.len());
        if !matches!(
            record,
            Record::CheckpointBegin(_) | Record::CheckpointChunk(_)
        ) {
            let now = self.now;
            let elapsed = now.saturating_sub(stream.window_at);
            if elapsed >= Duration::from_secs(1) {
                stream.other_rate = stream.window_bytes as f64 / elapsed.as_secs_f64();
                stream.window_at = now;
                stream.window_bytes = 0;
            }
            stream.window_bytes += bytes.len();
        }
        stream.fragments += bytes.len().div_ceil(FRAGMENT).max(1);
        let connection = stream.connection;
        self.send(connection, &Message::StandbyRecord(bytes));
    }

    /// The games that may stand by, best first: slice K6's ranking of the
    /// candidates (by join order), less the ones this slice holds back.
    fn standby_ranking(&self, ranked: &[u64]) -> Vec<ConnectionId> {
        ranked
            .iter()
            .filter_map(|&order| {
                self.peers
                    .iter()
                    .find(|(_, p)| p.lobby.order == order)
                    .map(|(id, _)| *id)
            })
            .filter(|&id| self.may_stand_by(id, ranked))
            .collect()
    }

    /// Whether `connection`'s game may be a standby: standbys switched on,
    /// the game still running, an eligible candidate by slice K6's rules
    /// (in a game a player hosts, not the house, not relayed, its switch on,
    /// reached by every other direct player: it is in `ranked`), not
    /// leaving, and not dismissed as behind this flight.
    fn may_stand_by(&self, connection: ConnectionId, ranked: &[u64]) -> bool {
        let Some(peer) = self.peers.get(&connection) else {
            return false;
        };
        self.standbys_on()
            && !matches!(self.life, Life::Stopped)
            && !peer.house
            && !matches!(peer.stage, Stage::Closing { .. })
            && peer.goodbye.is_none()
            && !self.standbys.excluded.contains(&peer.lobby.order)
            && ranked.contains(&peer.lobby.order)
    }

    /// Whether a standby already appointed stays one: standbys switched on,
    /// the game still running, the player not leaving nor dismissed as
    /// behind, and slice K6's eligibility but for reach tests not yet made
    /// (a player who just joined does not dismiss it).
    fn may_stay(&self, connection: ConnectionId) -> bool {
        let Some(peer) = self.peers.get(&connection) else {
            return false;
        };
        self.standbys_on()
            && !matches!(self.life, Life::Stopped)
            && !matches!(peer.stage, Stage::Closing { .. })
            && peer.goodbye.is_none()
            && !self.standbys.excluded.contains(&peer.lobby.order)
            && self.may_stay_standby(peer.lobby.order)
    }

    /// Whether `connection`'s game steps the mission alongside the host:
    /// its class (the platform and processor of its Candidate report) is
    /// the host's.
    fn standby_warm(&self, connection: ConnectionId) -> bool {
        self.peers.get(&connection).is_some_and(|peer| {
            self.candidate_of(peer.lobby.order).is_some_and(|report| {
                report.platform == Platform::current() && report.processor == Processor::current()
            })
        })
    }

    /// Drops standbys that left or stopped being eligible, and fills the
    /// free roles from the ranking. The journal records while any standby
    /// is appointed.
    fn appoint_standbys(&mut self) {
        let ranked = if self.standbys_on() {
            self.ranked_candidates()
        } else {
            Vec::new()
        };
        let mut index = 0;
        while index < self.standbys.streams.len() {
            let connection = self.standbys.streams[index].connection;
            if !self.peers.contains_key(&connection) {
                // Gone: nothing to tell it.
                self.standbys.streams.remove(index);
                self.standbys.changed = true;
                self.lobby_dirty = true;
                if self.standbys.streams.is_empty() {
                    self.record_journal(false);
                }
            } else if !self.may_stay(connection) {
                self.dismiss(index);
            } else {
                index += 1;
            }
        }
        if self.standbys.streams.len() < STANDBYS {
            for connection in self.standby_ranking(&ranked) {
                if self.standbys.streams.len() >= STANDBYS {
                    break;
                }
                if !self
                    .standbys
                    .streams
                    .iter()
                    .any(|s| s.connection == connection)
                {
                    self.appoint_new(connection);
                }
            }
        }
    }

    /// Makes the game with join order `order` standby 1, for the lobby's
    /// move (slice KP, agent decision): [`Host::hand_over`] hands the game
    /// to standby 1, so the player the lobby moves to must hold that role.
    /// The standby in it changes places with it when it is standby 2 (each
    /// is appointed again with its new role: in the lobby that costs the
    /// state parts only), and a game not standing by takes the role from
    /// the one that holds it. True when `order` is standby 1 now (ready or
    /// not yet); false when the standbys are off, the game is not in its
    /// lobby, or the player may not stand by.
    pub(super) fn make_first_standby(&mut self, order: u64) -> bool {
        if !self.standbys_on() || !matches!(self.life, Life::Lobby) {
            return false;
        }
        let Some(connection) = self
            .peers
            .iter()
            .find(|(_, p)| p.lobby.order == order)
            .map(|(id, _)| *id)
        else {
            return false;
        };
        let ranked = self.ranked_candidates();
        if !self.may_stand_by(connection, &ranked) {
            return false;
        }
        let position = |host: &Self, role: StandbyMark| {
            host.standbys.streams.iter().position(|s| s.role == role)
        };
        let first = position(self, StandbyMark::First);
        match self.standbys.streams.iter().position(|s| s.order == order) {
            Some(index) if Some(index) == first => return true,
            Some(index) => {
                // Standby 2 and standby 1 change places.
                self.standbys.streams[index].role = StandbyMark::First;
                let warm = self.standbys.streams[index].warm;
                self.reappoint(index, warm);
                if let Some(other) = first {
                    self.standbys.streams[other].role = StandbyMark::Second;
                    let warm = self.standbys.streams[other].warm;
                    self.reappoint(other, warm);
                }
            }
            None => {
                if let Some(index) = first {
                    self.dismiss(index);
                }
                self.appoint_new(connection);
            }
        }
        true
    }

    /// Appoints `connection`'s game in the free role.
    fn appoint_new(&mut self, connection: ConnectionId) {
        let first_taken = self
            .standbys
            .streams
            .iter()
            .any(|s| s.role == StandbyMark::First);
        let role = if first_taken {
            StandbyMark::Second
        } else {
            StandbyMark::First
        };
        let warm = self.standby_warm(connection);
        let order = self.peers[&connection].lobby.order;
        if self.standbys.streams.is_empty() {
            // The first standby: the journal records from now on. The parts
            // it makes at once are every part, which the new standby gets
            // with its appointment.
            self.record_journal(true);
            self.journal_parts();
            let _ = self.drain_journal();
        }
        let now = self.now;
        self.standbys
            .streams
            .push(Stream::new(connection, order, role, warm, now));
        self.appoint(self.standbys.streams.len() - 1);
    }

    /// Sends a stream its Appoint, the world when flying (a checkpoint at
    /// the world's tick now), and every state part, which hold after the
    /// last tick stepped.
    fn appoint(&mut self, index: usize) {
        let now = self.now;
        let stream = &mut self.standbys.streams[index];
        stream.ticks.clear();
        stream.outgoing = None;
        stream.sent_at = None;
        stream.base_tick = None;
        stream.status = None;
        stream.asks_checkpoint = false;
        stream.appointed_at = now;
        let appoint = Appoint {
            role: stream.role,
            warm: stream.warm,
            check_every: CHECK_EVERY_TICKS,
            checkpoint_every: CHECKPOINT_EVERY_TICKS,
            mission: self.number,
        };
        self.standbys.changed = true;
        self.lobby_dirty = true;
        self.stream_record(index, &Record::Appoint(appoint));
        if matches!(self.life, Life::Flying) {
            self.begin_checkpoint(&[index]);
        }
        let tick = u32::try_from(self.world.tick().saturating_sub(1)).unwrap_or(u32::MAX);
        for part in super::state::JOURNALED {
            // A part that does not code is logged by the journal's own
            // pass, and left out here.
            if let Ok(bytes) = self.encode_part(part) {
                self.stream_record(index, &Record::State(StatePart { part, tick, bytes }));
            }
        }
    }

    /// Appoints a stream's game again, warm or cold: everything after its
    /// Appoint starts afresh.
    fn reappoint(&mut self, index: usize, warm: bool) {
        let stream = &mut self.standbys.streams[index];
        stream.warm = warm;
        stream.warm_mismatches = 0;
        self.appoint(index);
    }

    /// Sends a stream Dismiss and drops it.
    fn dismiss(&mut self, index: usize) {
        self.flush_ticks(index);
        self.stream_record(index, &Record::Dismiss);
        self.standbys.streams.remove(index);
        self.standbys.changed = true;
        self.lobby_dirty = true;
        if self.standbys.streams.is_empty() {
            self.record_journal(false);
        }
    }

    /// A Check to every warm standby at the first update at or after each
    /// 600th tick.
    fn checks_due(&mut self) {
        let tick = self.world.tick();
        if tick < self.standbys.next_check {
            return;
        }
        let every = u64::from(CHECK_EVERY_TICKS);
        self.standbys.next_check = (tick / every + 1) * every;
        if tick == 0 || !self.standbys.streams.iter().any(|s| s.warm) {
            return;
        }
        let (Ok(hash), Ok(tick)) = (check_hash(&self.world), u32::try_from(tick)) else {
            return;
        };
        #[cfg(test)]
        let hash = if std::mem::take(&mut self.standbys.spoil_next_check) {
            !hash
        } else {
            hash
        };
        for index in 0..self.standbys.streams.len() {
            if self.standbys.streams[index].warm {
                self.flush_ticks(index);
                self.stream_record(index, &Record::Check(Check { tick, hash }));
            }
        }
    }

    /// A checkpoint to every cold standby whose last is 1,200 ticks old and
    /// gone out; one still going out delays the next.
    fn cold_checkpoints_due(&mut self) {
        let tick = self.world.tick();
        let every = u64::from(CHECKPOINT_EVERY_TICKS);
        let due: Vec<usize> = self
            .standbys
            .streams
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                !s.warm
                    && s.outgoing.is_none()
                    && s.base_tick.is_some_and(|base| tick >= base + every)
            })
            .map(|(index, _)| index)
            .collect();
        if !due.is_empty() {
            self.begin_checkpoint(&due);
        }
    }

    /// Takes the world's checkpoint now, between ticks, and starts it going
    /// out to the streams at `indices`: its begin record at once (after the
    /// ticks before it), its chunks paced.
    fn begin_checkpoint(&mut self, indices: &[usize]) {
        let tick = self.world.tick();
        let bytes = match self.world.checkpoint() {
            Ok(bytes) => Arc::new(bytes),
            Err(error) => {
                self.log(HostLog::Fault {
                    tick,
                    text: format!("standby checkpoint: {error}"),
                });
                return;
            }
        };
        let (Ok(tick32), Ok(length), Ok(chunks)) = (
            u32::try_from(tick),
            u32::try_from(bytes.len()),
            u16::try_from(bytes.len().div_ceil(CHUNK_BYTES)),
        ) else {
            return;
        };
        let begin = CheckpointBegin {
            tick: tick32,
            length,
            chunks,
        };
        let rate = (bytes.len() as f64 / PACE_SECONDS).clamp(MIN_RATE, MAX_RATE);
        let now = self.now;
        for &index in indices {
            self.flush_ticks(index);
            self.stream_record(index, &Record::CheckpointBegin(begin));
            let stream = &mut self.standbys.streams[index];
            stream.base_tick = Some(tick);
            stream.outgoing = Some(Outgoing {
                bytes: Arc::clone(&bytes),
                next: 0,
                chunks: usize::from(chunks),
                rate,
                // The first chunk goes at once.
                allowance: CHUNK_BYTES as f64,
                paced_at: now,
            });
        }
    }

    /// Sends a stream's checkpoint chunks the pace and the window allow.
    fn pace(&mut self, index: usize) {
        let now = self.now;
        let connection = self.standbys.streams[index].connection;
        let queued = self
            .server
            .stats(connection)
            .map_or(0, |stats| stats.messages_queued);
        let stream = &mut self.standbys.streams[index];
        // The rest of the stream comes off the line first.
        let room = (MAX_RATE - stream.other_rate).max(MIN_RATE);
        let Some(outgoing) = &mut stream.outgoing else {
            return;
        };
        let elapsed = now.saturating_sub(outgoing.paced_at).as_secs_f64();
        outgoing.paced_at = now;
        // The allowance never banks more than the window's worth.
        let window = (CHUNKS_IN_FLIGHT * CHUNK_BYTES) as f64;
        let rate = outgoing.rate.min(room);
        outgoing.allowance = (outgoing.allowance + rate * elapsed).min(window);
        // A chunk's record is a few bytes over 4 KB: 17 fragments.
        let fragments = CHUNK_BYTES.div_ceil(FRAGMENT) + 1;
        let mut in_flight = queued.div_ceil(fragments);
        let mut chunks = Vec::new();
        while outgoing.next < outgoing.chunks && in_flight < CHUNKS_IN_FLIGHT {
            let start = outgoing.next * CHUNK_BYTES;
            let end = (start + CHUNK_BYTES).min(outgoing.bytes.len());
            let length = (end - start) as f64;
            if outgoing.allowance < length {
                break;
            }
            outgoing.allowance -= length;
            chunks.push(CheckpointChunk {
                index: outgoing.next as u16,
                bytes: outgoing.bytes[start..end].to_vec(),
            });
            outgoing.next += 1;
            in_flight += 1;
        }
        let done = outgoing.next >= outgoing.chunks;
        for chunk in chunks {
            self.stream_record(index, &Record::CheckpointChunk(chunk));
        }
        if done {
            let stream = &mut self.standbys.streams[index];
            stream.outgoing = None;
            stream.sent_at = Some(now);
        }
    }

    /// Packets of reliable messages alone to a standby, as many as the
    /// fragments the stream queued since the last (at most
    /// [`EXTRA_PACKETS`] an update), while its messages are due: room for
    /// the stream beyond what the snapshot packets and the transport's own
    /// message packets carry, and no more, so the transport's resends do
    /// not ride on them.
    fn extra_packets(&mut self, index: usize) {
        let now = self.now;
        let stream = &mut self.standbys.streams[index];
        let connection = stream.connection;
        let wanted = stream.fragments.min(EXTRA_PACKETS);
        stream.fragments -= wanted;
        for _ in 0..wanted {
            if self.server.messages_due_bytes(connection, now) == 0
                || self.server.send_payload(now, connection, &[]).is_err()
            {
                self.standbys.streams[index].fragments = 0;
                break;
            }
        }
    }

    /// Slice K4's handover: the standby on `connection` gets the ticks
    /// waiting in its stream, then Handover with the last tick the host
    /// steps.
    pub(super) fn standby_handover(&mut self, connection: ConnectionId, last_tick: u32) {
        if let Some(index) = self
            .standbys
            .streams
            .iter()
            .position(|s| s.connection == connection)
        {
            self.flush_ticks(index);
            self.stream_record(index, &Record::Handover { last_tick });
        }
    }

    /// The standby mark of the player with join order `order`, for the
    /// lobby.
    pub(super) fn standby_mark(&self, order: u64) -> StandbyMark {
        self.standbys
            .streams
            .iter()
            .find(|s| s.order == order)
            .map_or(StandbyMark::None, |s| s.role)
    }

    /// The ready standbys in their roles' order, each with whether it is
    /// warm: what the Succession names.
    pub(crate) fn ready_standbys(&self) -> Vec<(ConnectionId, bool)> {
        let mut ready: Vec<&Stream> = self.standbys.streams.iter().filter(|s| s.ready()).collect();
        ready.sort_by_key(|s| s.role.code());
        ready
            .into_iter()
            .map(|s| {
                let warm = s.status.is_some_and(|st| st.state == StandbyState::Warm);
                (s.connection, warm)
            })
            .collect()
    }

    /// Each standby's figures, for the host's log and status line.
    pub fn standby_figures(&self) -> Vec<StandbyFigures> {
        self.standbys
            .streams
            .iter()
            .map(|s| {
                let peer = self.peers.get(&s.connection);
                StandbyFigures {
                    player: peer.map_or(0, |p| p.lobby.id),
                    callsign: peer.map_or_else(String::new, |p| p.callsign.clone()),
                    role: s.role,
                    warm: s.warm,
                    status: s.status,
                    checks_equal: s.checks_equal,
                    mismatches: s.mismatches,
                    bytes: s.bytes,
                    appointed_for: self.now.saturating_sub(s.appointed_at),
                }
            })
            .collect()
    }
}
