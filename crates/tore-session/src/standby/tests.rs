//! Slice K2's tests (docs/ARCHITECTURE.md, "Host migration and rejoin", row
//! K2): a standby fed, in process, the records a host makes with
//! `journal::apply_tick` on the crowd fight. A warm standby equals its source
//! at every check; a cold one restored at ten moments and replayed to the end
//! equals it; a damaged chunk is refused and asked for again; a standby
//! starved of time goes cold and comes back warm; a mismatch injected by hand
//! asks for a checkpoint; the thread stops cleanly. Synthetic resources.

use super::*;
use crate::journal::{Change, CheckpointBegin, CheckpointChunk, StreamWriter};
use crate::wire::messages::{Message, StandbyMark, kind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tore_formats::aircraft::AircraftId;
use tore_sim::combat::live;
use tore_sim::flight::PilotInput;
use tore_world::mission::{MissionSpec, Skill, Start};
use tore_world::resources::ResourceReads;
use tore_world::seats::{PlaneId, SeatCommand, SeatView};
use tore_world::test_support::resources::{THEATER, resources};
use tore_world::world::revive::RevivalWeapons;
use tore_world::world::{MissionCommand, Seating};

/// The mission's number in the records.
const MISSION: u32 = 7;
/// Ticks in a snapshot interval: a Ticks record's length.
const INTERVAL: u64 = 4;
/// Chunks the host sends with each interval while a checkpoint goes out.
const CHUNKS_PER_INTERVAL: usize = 2;

/// Four of ours, two of them human, against four bandits 2 nm ahead at
/// 10,000 feet: the crowd fight's shape on the session's synthetic import,
/// as the journal's tests fly it.
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

/// What a human does at `tick`: a weave, the trigger in bursts, the radar's
/// designation, chaff and flares, and the odd wing order.
fn pilot(seat: u8, tick: u64) -> SeatInput {
    let t = tick as f64 / 120. + f64::from(seat);
    let commands = match tick % 240 {
        5 => vec![SeatCommand::Combat(live::Command::Designate)],
        60 => vec![SeatCommand::ReleaseChaff, SeatCommand::ReleaseFlare],
        90 => vec![SeatCommand::CycleWeapon { forward: true }],
        150 => vec![SeatCommand::WingOrder(
            tore_sim::ai::wing::PlayerOrder::EngageMyTarget,
        )],
        _ => Vec::new(),
    };
    SeatInput {
        seat: SeatId(seat),
        tick,
        pilot: PilotInput {
            pitch: (t * 0.7).sin() * 0.25,
            roll: (t * 0.45).cos() * 0.3,
            throttle: Some(0.9),
            ..PilotInput::default()
        },
        trigger: tick % 240 < 40,
        commands,
        view: Some(SeatView {
            tick: tick.saturating_sub(10),
            interpolation_delay: 8,
        }),
        ..SeatInput::default()
    }
}

/// Counts the worlds a builder built.
#[derive(Clone, Default)]
struct Builds(Arc<AtomicUsize>);

impl Builds {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

/// The game's builder: the crowd fight from the synthetic import, whatever
/// the key (the tests' only mission).
fn builder(builds: &Builds) -> Builder {
    let import = Arc::new(resources());
    let spec = crowd_spec();
    let builds = builds.clone();
    Box::new(move |key: &MissionKey| {
        assert_eq!(key.mission, MISSION);
        builds.0.fetch_add(1, Ordering::SeqCst);
        World::new(&spec, &ResourceReads::new(&import), Seating::Open).map_err(|e| e.to_string())
    })
}

/// The host's side: its world stepped through `apply_tick` as a host steps
/// it, and its records coded in order through their messages' bytes.
struct Source {
    world: World,
    writer: StreamWriter,
    out: TickOutput,
    flying: Vec<u8>,
    /// A checkpoint going out: its chunks not yet sent.
    sending: VecDeque<CheckpointChunk>,
}

impl Source {
    fn new() -> Self {
        let import = resources();
        let world = World::new(&crowd_spec(), &ResourceReads::new(&import), Seating::Open).unwrap();
        Self {
            world,
            writer: StreamWriter::new(),
            out: TickOutput::default(),
            flying: Vec::new(),
            sending: VecDeque::new(),
        }
    }

    /// A record's bytes, through the Standby record message.
    fn code(&mut self, record: &Record) -> Vec<u8> {
        let message = Message::StandbyRecord(self.writer.encode(record).unwrap());
        let body = message.encode().unwrap();
        let Message::StandbyRecord(bytes) = Message::decode(kind::STANDBY_RECORD, &body).unwrap()
        else {
            panic!("not a standby record")
        };
        bytes
    }

    fn appoint(&mut self, warm: bool) -> Vec<u8> {
        self.code(&Record::Appoint(Appoint {
            role: StandbyMark::First,
            warm,
            check_every: crate::journal::CHECK_EVERY_TICKS,
            checkpoint_every: crate::journal::CHECKPOINT_EVERY_TICKS,
            mission: MISSION,
        }))
    }

    fn flight(&mut self) -> Vec<u8> {
        let flight = FlightRecord {
            mission: MISSION,
            spec_hash: tore_codec::fnv1a64(crowd_spec().to_text().as_bytes()),
            identity: self.world.mission_identity(),
            contrail_puffs: self.world.combat.contrail_puffs(),
        };
        self.code(&Record::Flight(flight))
    }

    /// Steps one tick as the host does and returns its journal: two humans
    /// from the start, a third at 300, one giving its plane back at 900 and
    /// taking an AI plane after an `ai-slot` store cut at 1,500.
    fn tick(&mut self) -> Tick {
        let number = self.world.tick();
        let mut tick = Tick::new(number);
        let take = |seat: u8, plane: u32| MissionCommand::Take {
            seat: SeatId(seat),
            plane: PlaneId(plane),
        };
        match number {
            0 => {
                tick.changes.push(Change::Scoring(true));
                tick.mission = vec![take(0, 0), take(1, 1)];
                self.flying = vec![0, 1];
            }
            300 => {
                tick.mission = vec![take(2, 2)];
                self.flying.push(2);
            }
            900 => {
                tick.mission = vec![MissionCommand::GiveBack { seat: SeatId(1) }];
                self.flying.retain(|&s| s != 1);
            }
            1_500 => {
                tick.changes.push(Change::StoreCut {
                    plane: PlaneId(3),
                    weapons: RevivalWeapons::Guns,
                });
                tick.mission = vec![take(1, 3)];
                self.flying.push(1);
            }
            _ => {}
        }
        for &seat in &self.flying {
            tick.push_input(pilot(seat, number), (number / 240) as u16);
        }
        apply_tick(&mut self.world, &tick, &mut self.out).unwrap();
        let _ = drain(&mut self.world);
        tick
    }

    /// A snapshot interval: its Ticks record, then the next chunks of a
    /// checkpoint going out.
    fn interval(&mut self) -> Vec<Vec<u8>> {
        let first = self.world.tick() as u32;
        let ticks = (0..INTERVAL).map(|_| self.tick()).collect();
        let mut records = vec![self.code(&Record::Ticks(Ticks { first, ticks }))];
        for _ in 0..CHUNKS_PER_INTERVAL {
            if let Some(chunk) = self.sending.pop_front() {
                records.push(self.code(&Record::CheckpointChunk(chunk)));
            }
        }
        records
    }

    /// A Check of the world as it is now.
    fn check(&mut self) -> Vec<u8> {
        let check = Check {
            tick: self.world.tick() as u32,
            hash: check_hash(&self.world).unwrap(),
        };
        self.code(&Record::Check(check))
    }

    /// Begins a checkpoint of the world as it is now; its chunks go out with
    /// the intervals that follow.
    fn begin_checkpoint(&mut self) -> Vec<u8> {
        let (begin, chunks) = assemble::cut(self.world.tick(), &self.world.checkpoint().unwrap());
        self.sending = chunks.into();
        self.code(&Record::CheckpointBegin(begin))
    }

    /// The whole checkpoint at once.
    fn whole_checkpoint(&mut self) -> Vec<Vec<u8>> {
        let mut records = vec![self.begin_checkpoint()];
        while let Some(chunk) = self.sending.pop_front() {
            records.push(self.code(&Record::CheckpointChunk(chunk)));
        }
        records
    }

    fn state(&mut self, part: Part, bytes: &[u8]) -> Vec<u8> {
        let tick = self.world.tick().saturating_sub(1) as u32;
        self.code(&Record::State(StatePart {
            part,
            tick,
            bytes: bytes.to_vec(),
        }))
    }
}

fn feed(standby: &mut Standby, records: &[Vec<u8>]) {
    for record in records {
        standby.receive(record).unwrap();
    }
}

/// The standby's world at the source's tick codes to the source's bytes.
fn assert_equal(world: &World, source: &Source, what: &str) {
    assert_eq!(world.tick(), source.world.tick(), "{what}: the tick");
    assert!(
        world.checkpoint().unwrap() == source.world.checkpoint().unwrap(),
        "{what}: the standby differs from its source at tick {}",
        world.tick()
    );
}

/// A warm standby fed `ticks` of the fight a snapshot interval at a time,
/// stepped after each, with a Check every 600 ticks: every check equal, warm
/// throughout, and the takeover's world the source's.
fn warm_run(ticks: u64) {
    let builds = Builds::default();
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&builds));
    let appoint = source.appoint(true);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    assert_eq!(standby.status().state, StandbyState::Warm);
    let mut equal = 0;
    while source.world.tick() < ticks {
        let records = source.interval();
        feed(&mut standby, &records);
        standby.step(Budget::Ticks(usize::MAX));
        let tick = source.world.tick();
        if tick.is_multiple_of(u64::from(crate::journal::CHECK_EVERY_TICKS)) {
            let check = source.check();
            feed(&mut standby, &[check]);
            let status = standby.status();
            assert_eq!(
                (status.check_tick, status.check),
                (tick as u32, CheckResult::Equal),
                "the check at tick {tick}"
            );
            equal += 1;
        }
        if tick.is_multiple_of(3_000) {
            let part = source.state(Part::Scores, &tick.to_le_bytes());
            feed(&mut standby, &[part]);
        }
        let status = standby.status();
        assert_eq!(status.state, StandbyState::Warm, "at tick {tick}");
        assert_eq!(u64::from(status.newest_tick), tick);
        assert!(!status.needs_checkpoint);
    }
    assert_eq!(equal, ticks / 600);
    assert!(standby.status().step_micros > 0);
    let takeover = standby.take_over().unwrap();
    assert_eq!(takeover.replayed, 0, "a warm takeover replays nothing");
    assert_equal(takeover.world.as_ref().unwrap(), &source, "the takeover");
    assert_eq!(takeover.tick, ticks);
    let scores = &takeover.parts[&Part::Scores];
    let last = (ticks / 3_000) * 3_000;
    assert_eq!(scores.bytes, last.to_le_bytes());
    // Seats 0, 1 and 2 fly at the end, each on its last recorded input.
    assert_eq!(
        takeover.last_inputs.keys().copied().collect::<Vec<_>>(),
        [SeatId(0), SeatId(1), SeatId(2)]
    );
    assert_eq!(takeover.last_inputs[&SeatId(0)].0.tick, ticks - 1);
    // The Flight's build, and nothing more: a warm standby needs no spare
    // until a restore, and nobody called `prepare`.
    assert_eq!(builds.count(), 1);
}

#[test]
fn a_warm_standby_equals_its_source_at_every_check_for_a_minute() {
    warm_run(7_200);
}

/// The acceptance's 5 minutes (36,000 ticks, 60 checks). Slow in a debug
/// build: for the full run (`cargo test -p tore-session -- --ignored
/// a_warm_standby_equals_its_source_at_every_check_for_five_minutes`).
#[test]
#[ignore = "slow: the full run's 5-minute warm standby"]
fn a_warm_standby_equals_its_source_at_every_check_for_five_minutes() {
    warm_run(36_000);
}

#[test]
fn a_cold_standby_restored_at_ten_moments_and_replayed_to_the_end_equals_its_source() {
    let builds = Builds::default();
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&builds));
    let appoint = source.appoint(false);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    // The Flight's world is the cold standby's spare.
    assert_eq!(builds.count(), 1);
    assert_eq!(standby.status().state, StandbyState::Cold);
    let ticks = 6_000;
    let moments: Vec<u64> = (1..=10).map(|n| n * 560).collect();
    let mut restored = 0;
    let mut checkpoints = 0;
    let mut longest = 0;
    while source.world.tick() < ticks {
        let records = source.interval();
        feed(&mut standby, &records);
        let tick = source.world.tick();
        if tick.is_multiple_of(u64::from(crate::journal::CHECKPOINT_EVERY_TICKS)) {
            let begin = source.begin_checkpoint();
            feed(&mut standby, &[begin]);
        }
        // A warm-only check is ignored by a cold standby.
        if tick.is_multiple_of(600) {
            let check = source.check();
            feed(&mut standby, &[check]);
        }
        for note in standby.take_notes() {
            if let Note::Checkpoint { restored, .. } = note {
                assert!(!restored, "a cold standby keeps its checkpoints");
                checkpoints += 1;
            }
        }
        assert_eq!(standby.step(Budget::Ticks(usize::MAX)), 0);
        longest = longest.max(standby.backlog());
        if moments.contains(&tick) {
            let world = standby.replica().unwrap();
            assert_equal(&world, &source, "a cold restore");
            restored += 1;
            standby.prepare();
        }
        let status = standby.status();
        assert_eq!(status.state, StandbyState::Cold);
        assert_eq!(status.check, CheckResult::None);
    }
    assert_eq!(restored, 10);
    assert_eq!(checkpoints, 4, "the checkpoints at 1,200 to 4,800");
    // The journal since the newest checkpoint: its 1,200 ticks and the
    // chunks' pacing.
    assert!(longest < 1_400, "{longest} ticks held");
    let takeover = standby.take_over().unwrap();
    assert!(takeover.replayed > 0);
    assert_equal(takeover.world.as_ref().unwrap(), &source, "the takeover");
}

#[test]
fn a_damaged_chunk_is_refused_and_asked_for_again() {
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&Builds::default()));
    let appoint = source.appoint(false);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    while source.world.tick() < 400 {
        let records = source.interval();
        feed(&mut standby, &records);
    }
    // One byte of the third chunk's payload flipped: the whole fails its
    // CRC-32 and is asked for again.
    let (begin, mut chunks) =
        assemble::cut(source.world.tick(), &source.world.checkpoint().unwrap());
    assert!(chunks.len() > 3);
    chunks[2].bytes[100] ^= 0x10;
    let mut records = vec![source.code(&Record::CheckpointBegin(begin))];
    for chunk in chunks {
        records.push(source.code(&Record::CheckpointChunk(chunk)));
    }
    feed(&mut standby, &records);
    assert!(standby.status().needs_checkpoint);
    assert!(
        standby
            .take_notes()
            .iter()
            .any(|n| matches!(n, Note::Refused(why) if why.contains("CRC")))
    );
    // Still ready on what it held: the Flight's world and the journal.
    assert_eq!(standby.status().state, StandbyState::Cold);
    assert_equal(&standby.replica().unwrap(), &source, "after a refusal");
    // A chunk record that cannot be read spoils only its checkpoint.
    let begin = source.begin_checkpoint();
    feed(&mut standby, &[begin]);
    let chunk = source.sending.pop_front().unwrap();
    let bytes = source.code(&Record::CheckpointChunk(chunk));
    assert!(standby.receive(&bytes[..bytes.len() / 2]).is_err());
    assert!(standby.status().needs_checkpoint);
    // The ticks after it read on, and the rest of that checkpoint's chunks
    // are refused for want of a begin.
    while source.world.tick() < 800 {
        let records = source.interval();
        feed(&mut standby, &records);
    }
    assert_ne!(standby.status().state, StandbyState::Behind);
    // Chunks out of range or of the wrong size are damage too.
    let (begin, chunks) = assemble::cut(source.world.tick(), &source.world.checkpoint().unwrap());
    let mut wrong = chunks[0].clone();
    wrong.index = begin.chunks;
    let bad_begin = source.code(&Record::CheckpointBegin(begin));
    let bad_chunk = source.code(&Record::CheckpointChunk(wrong));
    feed(&mut standby, &[bad_begin, bad_chunk]);
    assert!(standby.status().needs_checkpoint);
    let mut short = Assembly::begin(begin).unwrap();
    let mut cut_short = chunks[0].clone();
    cut_short.bytes.pop();
    assert!(matches!(short.add(cut_short), Err(Refusal::Damaged(_))));
    assert!(
        Assembly::begin(CheckpointBegin {
            chunks: begin.chunks + 1,
            ..begin
        })
        .is_err()
    );
    // Asked again, it comes whole: kept, and the request is withdrawn.
    let records = source.whole_checkpoint();
    feed(&mut standby, &records);
    let status = standby.status();
    assert!(!status.needs_checkpoint);
    assert_eq!(status.state, StandbyState::Cold);
    assert_eq!(standby.backlog(), 0);
    let takeover = standby.take_over().unwrap();
    assert_eq!(takeover.replayed, 0);
    assert_equal(takeover.world.as_ref().unwrap(), &source, "the takeover");
}

#[test]
fn a_standby_starved_of_time_goes_cold_and_comes_back_warm() {
    let builds = Builds::default();
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&builds));
    let appoint = source.appoint(true);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    let run_to = |source: &mut Source, standby: &mut Standby, tick: u64, budget: usize| {
        while source.world.tick() < tick {
            let records = source.interval();
            feed(standby, &records);
            standby.step(Budget::Ticks(budget));
        }
    };
    run_to(&mut source, &mut standby, 600, usize::MAX);
    assert_eq!(standby.status().state, StandbyState::Warm);
    // No time at all for 300 ticks: more than 2 seconds behind, it is
    // catching up, reported cold and still ready.
    run_to(&mut source, &mut standby, 900, 0);
    let status = standby.status();
    assert_eq!(status.state, StandbyState::Cold);
    assert!(!status.needs_checkpoint);
    // Another 2 seconds lost: it drops its copy, keeps its own checkpoint
    // as a base and asks for the host's.
    run_to(&mut source, &mut standby, 1_200, 0);
    let status = standby.status();
    assert_eq!(status.state, StandbyState::Cold);
    assert!(status.needs_checkpoint);
    assert!(standby.ready());
    assert!(
        standby
            .take_notes()
            .contains(&Note::FellBehind { tick: 600 })
    );
    assert_equal(&standby.replica().unwrap(), &source, "fallen behind");
    // The host's checkpoint, paced over the intervals that follow while the
    // standby has little time: restored once whole, then caught up.
    let begin = source.begin_checkpoint();
    feed(&mut standby, &[begin]);
    let at = source.world.tick();
    run_to(&mut source, &mut standby, at + 200, 2);
    assert!(!standby.status().needs_checkpoint);
    assert!(standby.take_notes().contains(&Note::Checkpoint {
        tick: at,
        restored: true
    }));
    run_to(&mut source, &mut standby, at + 400, 8);
    assert_eq!(standby.status().state, StandbyState::Warm);
    assert!(
        standby
            .take_notes()
            .iter()
            .any(|n| matches!(n, Note::Warm { .. }))
    );
    // Warm again, its next check is equal.
    run_to(&mut source, &mut standby, 1_800, usize::MAX);
    let check = source.check();
    feed(&mut standby, &[check]);
    assert_eq!(standby.status().check, CheckResult::Equal);
    // Nobody called `prepare`, so each world was built when needed: the
    // Flight's, the replica's and the restore's.
    assert_eq!(builds.count(), 3);
    assert!(standby.prepare());
    assert!(!standby.prepare(), "one spare at a time");
    let takeover = standby.take_over().unwrap();
    assert_equal(takeover.world.as_ref().unwrap(), &source, "the takeover");
}

#[test]
fn a_mismatch_injected_by_hand_asks_for_a_checkpoint() {
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&Builds::default()));
    let appoint = source.appoint(true);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    while source.world.tick() < 600 {
        let records = source.interval();
        feed(&mut standby, &records);
        standby.step(Budget::Ticks(usize::MAX));
    }
    // A check whose hash is off by a bit.
    let check = Check {
        tick: 600,
        hash: check_hash(&source.world).unwrap() ^ 1,
    };
    let wrong = source.code(&Record::Check(check));
    feed(&mut standby, &[wrong]);
    let status = standby.status();
    assert_eq!(
        (status.check_tick, status.check),
        (600, CheckResult::Different)
    );
    assert!(status.needs_checkpoint);
    // Nothing it stepped can be trusted: building until a checkpoint.
    assert_eq!(status.state, StandbyState::Building);
    assert!(!standby.ready());
    assert!(standby.take_notes().contains(&Note::Mismatch { tick: 600 }));
    // The ticks go on; the checkpoint comes; it restores and is warm again.
    while source.world.tick() < 720 {
        let records = source.interval();
        feed(&mut standby, &records);
    }
    let records = source.whole_checkpoint();
    feed(&mut standby, &records);
    assert_eq!(standby.status().state, StandbyState::Warm);
    while source.world.tick() < 1_200 {
        let records = source.interval();
        feed(&mut standby, &records);
        standby.step(Budget::Ticks(usize::MAX));
    }
    let check = source.check();
    feed(&mut standby, &[check]);
    let status = standby.status();
    assert_eq!(
        (status.check_tick, status.check, status.needs_checkpoint),
        (1_200, CheckResult::Equal, false)
    );
}

/// A checkpoint a warm copy in good standing has no use for (one begun before
/// the copy stepped past it, whose chunks arrive after): dropped quietly, not
/// refused as a checkpoint the journal does not continue, so it asks for no
/// other (the battery's planned handover, 2026-10-24, where each refusal
/// asked for another and the host sent one every 10 seconds).
#[test]
fn a_checkpoint_a_warm_copy_has_stepped_past_is_not_refused() {
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&Builds::default()));
    let appoint = source.appoint(true);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    while source.world.tick() < 600 {
        let records = source.interval();
        feed(&mut standby, &records);
        standby.step(Budget::Ticks(usize::MAX));
    }
    // Begun at 600; its chunks go out with the intervals while the copy
    // steps each one, so the copy is past 600 and holds no journal from it
    // when the last chunk comes.
    let begin = source.begin_checkpoint();
    feed(&mut standby, &[begin]);
    while !source.sending.is_empty() {
        let records = source.interval();
        feed(&mut standby, &records);
        standby.step(Budget::Ticks(usize::MAX));
    }
    let notes = standby.take_notes();
    assert!(
        !notes.iter().any(|n| matches!(n, Note::Refused(_))),
        "refused: {notes:?}"
    );
    let status = standby.status();
    assert_eq!(status.state, StandbyState::Warm);
    assert!(!status.needs_checkpoint, "it asks for no other");
    // The copy is still the source's.
    while source.world.tick() < 1_200 {
        let records = source.interval();
        feed(&mut standby, &records);
        standby.step(Budget::Ticks(usize::MAX));
    }
    let check = source.check();
    feed(&mut standby, &[check]);
    let status = standby.status();
    assert_eq!(
        (status.check_tick, status.check, status.needs_checkpoint),
        (1_200, CheckResult::Equal, false)
    );
}

#[test]
fn appointed_in_flight_a_standby_builds_the_flight_it_holds_from_the_checkpoint() {
    let builds = Builds::default();
    let mut source = Source::new();
    while source.world.tick() < 480 {
        let _ = source.interval();
    }
    let mut standby = Standby::new(builder(&builds));
    // Appoint, then the checkpoint with the ticks after it: no Flight.
    let appoint = source.appoint(true);
    let begin = source.begin_checkpoint();
    feed(&mut standby, &[appoint, begin]);
    assert_eq!(standby.status().state, StandbyState::Building);
    while !source.sending.is_empty() {
        let records = source.interval();
        feed(&mut standby, &records);
        standby.step(Budget::Ticks(usize::MAX));
    }
    let records = source.interval();
    feed(&mut standby, &records);
    standby.step(Budget::Ticks(usize::MAX));
    assert_eq!(standby.status().state, StandbyState::Warm);
    let takeover = standby.take_over().unwrap();
    assert_eq!(takeover.flight, None);
    assert_equal(takeover.world.as_ref().unwrap(), &source, "the takeover");
    assert_eq!(builds.count(), 1, "the build for the checkpoint, used");
}

#[test]
fn another_mission_a_gap_or_an_unreadable_record_puts_the_standby_behind() {
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&Builds::default()));
    // Records before an Appoint are ignored.
    let flight = source.flight();
    feed(&mut standby, std::slice::from_ref(&flight));
    assert!(standby.appointed().is_none());
    // A Flight of another mission identity.
    let appoint = source.appoint(true);
    let foreign = source.code(&Record::Flight(FlightRecord {
        mission: MISSION,
        spec_hash: 1,
        identity: source.world.mission_identity() ^ 1,
        contrail_puffs: true,
    }));
    feed(&mut standby, &[appoint, foreign]);
    assert_eq!(standby.status().state, StandbyState::Behind);
    assert!(standby.replica().is_err());
    // A Flight again starts it afresh.
    let flight = source.flight();
    feed(&mut standby, &[flight]);
    assert_eq!(standby.status().state, StandbyState::Warm);
    // A gap in the journal: the record after it reads against inputs the
    // standby never had, or is refused; either way the stream is broken.
    let _ = source.interval();
    let records = source.interval();
    let _ = standby.receive(&records[0]);
    assert_eq!(standby.status().state, StandbyState::Behind);
    assert!(
        standby
            .take_notes()
            .iter()
            .any(|n| matches!(n, Note::Broken(_)))
    );
    // An unreadable Ticks record breaks the stream too.
    let mut standby = Standby::new(builder(&Builds::default()));
    let mut source = Source::new();
    let appoint = source.appoint(false);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    let records = source.interval();
    assert!(standby.receive(&records[0][..3]).is_err());
    assert_eq!(standby.status().state, StandbyState::Behind);
    assert!(standby.take_over().is_err());
    // A builder that cannot build the mission.
    let mut standby = Standby::new(Box::new(|_: &MissionKey| {
        Err::<World, String>("no such mission".into())
    }));
    let mut source = Source::new();
    let appoint = source.appoint(true);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    assert_eq!(standby.status().state, StandbyState::Behind);
}

#[test]
fn ended_returns_to_the_lobby_and_dismiss_drops_everything() {
    let mut source = Source::new();
    let mut standby = Standby::new(builder(&Builds::default()));
    let appoint = source.appoint(false);
    let flight = source.flight();
    feed(&mut standby, &[appoint, flight]);
    let records = source.interval();
    feed(&mut standby, &records);
    let players = source.state(Part::Players, b"players");
    let ended = source.code(&Record::Ended(EndReason::EndedByServer));
    feed(&mut standby, &[players, ended]);
    // In the lobby: ready with nothing to hold, the parts kept.
    let status = standby.status();
    assert_eq!((status.state, status.newest_tick), (StandbyState::Cold, 0));
    assert!(standby.ready());
    assert_eq!(standby.part(Part::Players).unwrap().bytes, b"players");
    let handover = source.code(&Record::Handover { last_tick: 4 });
    feed(&mut standby, &[handover]);
    assert_eq!(standby.handover(), Some(4));
    let dismiss = source.code(&Record::Dismiss);
    let mut copy = Standby::new(builder(&Builds::default()));
    let mut twin = Source::new();
    let appoint = twin.appoint(false);
    let players = twin.state(Part::Players, b"players");
    let ended = twin.code(&Record::Ended(EndReason::EndedByServer));
    feed(&mut copy, &[appoint, players, ended]);
    let takeover = copy.take_over().unwrap();
    assert!(takeover.world.is_none());
    assert_eq!(takeover.ended, Some(EndReason::EndedByServer));
    assert_eq!(takeover.parts[&Part::Players].bytes, b"players");
    feed(&mut standby, &[dismiss]);
    assert!(standby.appointed().is_none());
    assert!(standby.part(Part::Players).is_none());
    assert!(standby.take_over().is_err());
}

/// Waits up to 60 seconds for `done`.
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_thread_follows_its_source_and_hands_over_its_world() {
    let mut source = Source::new();
    let thread = StandbyThread::spawn(builder(&Builds::default())).unwrap();
    let appoint = source.appoint(true);
    let flight = source.flight();
    thread.records(vec![appoint, flight]);
    while source.world.tick() < 1_200 {
        let records = source.interval();
        thread.records(records);
        if source.world.tick().is_multiple_of(600) {
            let check = source.check();
            thread.records(vec![check]);
            let tick = source.world.tick() as u32;
            wait_for("the check", || thread.status().check_tick == tick);
            assert_eq!(thread.status().check, CheckResult::Equal);
        }
    }
    let tick = source.world.tick() as u32;
    wait_for("the end of the journal", || {
        let status = thread.status();
        status.newest_tick == tick && thread.backlog() == 0
    });
    assert!(thread.ready());
    assert!(thread.appointed());
    assert_eq!(thread.refused(), 0);
    let handover = source.code(&Record::Handover {
        last_tick: tick - 1,
    });
    thread.records(vec![handover]);
    wait_for("the handover", || thread.handover().is_some());
    assert!(
        thread
            .take_notes()
            .contains(&Note::Appointed { warm: true })
    );
    let takeover = thread.take_over().unwrap();
    assert_eq!(takeover.handover, Some(tick - 1));
    assert_equal(takeover.world.as_ref().unwrap(), &source, "the takeover");
}

#[test]
fn the_thread_stops_cleanly() {
    let mut source = Source::new();
    // Stopped with work in hand.
    let thread = StandbyThread::spawn(builder(&Builds::default())).unwrap();
    let appoint = source.appoint(true);
    let flight = source.flight();
    let mut records = vec![appoint, flight];
    for _ in 0..50 {
        records.extend(source.interval());
    }
    thread.records(records);
    thread.stop().unwrap();
    // Dropped while idle.
    let thread = StandbyThread::spawn(builder(&Builds::default())).unwrap();
    drop(thread);
    // A takeover refused: not appointed; the thread ends all the same.
    let thread = StandbyThread::spawn(builder(&Builds::default())).unwrap();
    assert!(thread.take_over().is_err());
    // A builder that panics: the stop reports it.
    let thread = StandbyThread::spawn(Box::new(|_: &MissionKey| panic!("the builder"))).unwrap();
    let mut source = Source::new();
    let appoint = source.appoint(true);
    let flight = source.flight();
    thread.records(vec![appoint, flight]);
    assert!(thread.stop().is_err());
}
