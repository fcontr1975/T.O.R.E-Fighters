//! Any moment of a recording, rebuilt from the recording alone: the picture
//! combat draws (from the nearest keyframe, a chunk at a time), smoke and
//! contrails (from their release ticks), and the player's wing vapor (from
//! the last 250 ticks of recorded poses). Nothing here depends on how the
//! playhead got to a tick, so reverse play shows exactly what forward play
//! showed. Playing forward only steps what the playhead crossed (the puffs
//! released since the last tick drawn, the poses since it); a seek, reverse
//! play or a long jump rebuilds from the recording as ever, and the tests
//! hold both ways to the same result at every tick. A few decoded chunks
//! around the playhead keep both directions smooth.
use crate::aircraft::Airframe;
use crate::flight;
use crate::replay::convert::{self, Identities, Presentation};
use crate::snapshot::{self, RenderSnapshot, set_devices};
use std::collections::BTreeMap;
use std::sync::Arc;
use tore_replay::{
    AircraftState, CONTRAIL_PUFF_CAP, Frame, LAYER_CONTRAILS, LONGEST_PUFF_TICKS, LivePuff,
    PuffKind, PuffSpawn, Recording, SMOKE_PUFF_CAP,
};
use tore_sim::combat::smoke::{Kind, Puff, Smoke};
use tore_sim::vapor::{COMMIT_TICKS, Vapor};

#[derive(Clone, Copy, Debug)]
struct RecordedMotor {
    start: u64,
    boost_end: u64,
    end: u64,
    boost: bool,
    sustain: bool,
}
impl RecordedMotor {
    fn read(e: &tore_replay::Event) -> Option<(u32, Self)> {
        if e.kind != tore_replay::vocab::kind::WEAPON_LAUNCH
            || e.get("missile-model")?.as_str()? != "energy-1"
        {
            return None;
        }
        let read = |key| u64::try_from(e.get(key)?.as_i64()?).ok();
        let result = Self {
            start: read("motor-ignition")?,
            boost_end: read("motor-boost-end")?,
            end: read("motor-end")?,
            boost: e.get("motor-boost-powered")?.as_bool()?,
            sustain: e.get("motor-sustain-powered")?.as_bool()?,
        };
        if result.start > result.boost_end || result.boost_end > result.end {
            return None;
        }
        Some((
            e.get(tore_replay::vocab::field::PROJECTILE)?.as_id()?,
            result,
        ))
    }
    fn powered(self, age: u64) -> bool {
        (self.boost && (self.start..self.boost_end).contains(&age))
            || (self.sustain && (self.boost_end..self.end).contains(&age))
    }
}
fn recorded_motors(recording: &Recording) -> BTreeMap<u32, RecordedMotor> {
    recording
        .events()
        .iter()
        .filter_map(|t| RecordedMotor::read(&t.event))
        .collect()
}

/// Decoded chunks kept around the playhead: enough for the vapor's look
/// back and a few seconds either way.
const CHUNK_CACHE: usize = 8;
/// Ticks of recorded poses the wing vapor is rebuilt from.
pub const VAPOR_TICKS: u64 = 250;

/// Adds a tick of each aircraft's recorded rotor speed per frame to its
/// blade angle (seconds at 100 percent).
fn add_rotor_turns(turns: &mut BTreeMap<u32, f64>, frames: &[Frame]) {
    for aircraft in frames.iter().flat_map(|frame| &frame.aircraft) {
        if aircraft.rotor_speed > 0. {
            *turns.entry(aircraft.id).or_default() += aircraft.rotor_speed * flight::DT;
        }
    }
}

/// The weather clock's native ticks after `tick` simulation ticks: 256 a
/// second, counted as the fixed clock counts them.
pub fn native_ticks(tick: u64) -> i64 {
    (u128::from(tick) * 256 / 120) as i64
}

/// Simulation ticks at which the vapor history commits a new entry, from
/// launch: the first tick at least [`COMMIT_TICKS`] native ticks after the
/// previous commit, as live flight's vapor does.
fn commit_ticks(last: u64) -> Vec<u64> {
    let mut commits = vec![0];
    extend_commits(&mut commits, 0, last);
    commits
}

/// Adds the commit ticks after tick `done` up to `last` to `commits`, which
/// holds every one up to `done`: a recording that grows (an observer's)
/// needs only its new ticks.
fn extend_commits(commits: &mut Vec<u64>, done: u64, last: u64) {
    let mut committed = commits.last().map_or(0, |tick| native_ticks(*tick));
    for tick in done + 1..=last {
        let native = native_ticks(tick);
        if native >= committed + COMMIT_TICKS {
            commits.push(tick);
            committed = native;
        }
    }
}

/// Moves `state` to a recorded aircraft's pose: what the wing vapor's
/// attachment points read.
pub fn place(state: &mut flight::State, recorded: &AircraftState) {
    state.position = recorded.position;
    [state.yaw, state.pitch, state.bank] = recorded.attitude;
    if recorded.flags.animated {
        set_devices(state, recorded.devices);
    }
}

/// The smoke and contrails of the last tick drawn, and the puffs it was
/// made from, so the next tick forward adds the puffs released since and
/// drops the ones that expired instead of reading them all again.
struct SmokeTrack {
    tick: u64,
    /// Every puff alive at `tick` with a known lifetime, as released: the
    /// release tick and the spawn, in the order the recording lists them.
    live: Vec<(u64, PuffSpawn)>,
    layers: [Smoke; 2],
    /// `live` holds exactly what the recording lists up to `tick`.
    exact: bool,
}

/// The player's wing vapor at the last tick drawn, and what is needed to
/// carry it forward a tick at a time to the same result a rebuild gives.
struct VaporTrack {
    tick: u64,
    vapor: Option<Vapor>,
    /// The rebuild stepped every tick from a commit tick of the launch
    /// schedule (see [`Playback::vapor`]), so stepping on from here gives
    /// what a rebuild at a later tick gives, and every pose was found.
    exact: bool,
    /// The history started on the launch schedule rather than at the
    /// recording's first pose.
    scheduled: bool,
    /// Whether the poses stepped so far carried animated devices, all alike.
    animated: Option<bool>,
}

pub struct Playback {
    recording: Arc<Recording>,
    pub presentation: Presentation,
    pub identities: Identities,
    first: u64,
    last: u64,
    /// Decoded chunks, most recently used last.
    chunks: Vec<(usize, Arc<Vec<Frame>>)>,
    /// The snapshots of the last tick drawn and the tick before it.
    pair: Option<(u64, Option<RenderSnapshot>, RenderSnapshot)>,
    smoke: Option<SmokeTrack>,
    vapor: Option<VaporTrack>,
    commits: Vec<u64>,
    /// Every crater and fire the recording started.
    marks: Vec<(u64, tore_replay::EffectSpawn)>,
    /// The city fill buildings the recording knocked down, tick by tick
    /// (nr-fx3): none in a recording from before them.
    city: tore_replay::CityTrack,
    /// Every aircraft's recorded stores over time (B3): none in a recording
    /// from before them.
    stores: tore_replay::stores::StoreTrack,
    /// Each rotorcraft's blade angle (seconds at 100 percent rotor speed) at
    /// the start of every chunk summed so far, in chunk order: the recorded
    /// rotor speed integrated frame by frame from the recording's start
    /// (slice P7b). A cache of the recording alone, so the blade angle at a
    /// tick does not depend on how the playhead got there.
    rotor_starts: Vec<BTreeMap<u32, f64>>,
    /// The motor of each weapon record the recording registers, read from
    /// the import ([`Playback::add_motors`]): a round draws its flame only
    /// while its motor burns (bug B7). Without them no flame is drawn.
    motors: BTreeMap<String, Option<crate::regen::Motor>>,
    recorded_motors: BTreeMap<u32, RecordedMotor>,
    /// Rebuilds from the recording that smoke and vapor made, and steps on
    /// from the tick before them (tests only).
    #[cfg(test)]
    rebuilds: [u32; 2],
    #[cfg(test)]
    steps_on: [u32; 2],
}

impl Playback {
    /// Reads the motor of every weapon the recording registers that has
    /// none yet, from the import `resources`.
    pub fn add_motors(&mut self, resources: &BTreeMap<String, Vec<u8>>) {
        for weapon in self.recording.weapons() {
            self.motors.entry(weapon.source.clone()).or_insert_with(|| {
                let bytes = resources.get(&weapon.source)?;
                crate::regen::Motor::of(
                    &tore_formats::weapons::Weapon::parse(&weapon.source, bytes).ok()?,
                )
            });
        }
    }

    pub fn new(recording: Arc<Recording>) -> Self {
        let first = recording.first_tick().unwrap_or(0);
        let last = recording.last_tick().unwrap_or(first);
        Self {
            presentation: Presentation::from_header(recording.header()),
            identities: Identities::of(&recording),
            commits: commit_ticks(last),
            marks: convert::recorded_marks(&recording),
            city: recording.city().clone(),
            stores: tore_replay::stores::StoreTrack::from_events(recording.events()),
            recorded_motors: recorded_motors(&recording),
            recording,
            first,
            last,
            chunks: Vec::new(),
            pair: None,
            smoke: None,
            vapor: None,
            rotor_starts: Vec::new(),
            motors: BTreeMap::new(),
            #[cfg(test)]
            rebuilds: [0; 2],
            #[cfg(test)]
            steps_on: [0; 2],
        }
    }

    #[cfg(test)]
    pub fn recording(&self) -> &Arc<Recording> {
        &self.recording
    }

    /// The recording has grown: `recording` is the same one read again with
    /// more frames after the old last (an observer's live recording). The
    /// decoded chunks stay, since a finished chunk never changes; what the
    /// newest ticks hold (the picture, smoke, the vapor) is rebuilt on next
    /// use, and the registry, the vapor's commit ticks and the craters and
    /// fires follow the new frames.
    pub fn extend(&mut self, recording: Arc<Recording>) {
        let old = self.last;
        self.first = recording.first_tick().unwrap_or(self.first);
        self.last = recording.last_tick().unwrap_or(old).max(old);
        extend_commits(&mut self.commits, old, self.last);
        if self.last > old
            && let Ok(spawns) = recording.spawns(old + 1, self.last)
        {
            self.marks
                .extend(spawns.effects.into_iter().filter(|(_, e)| {
                    matches!(
                        e.kind,
                        tore_replay::EffectKind::Crater(_) | tore_replay::EffectKind::Fire
                    )
                }));
        }
        self.identities = Identities::of(&recording);
        self.stores = tore_replay::stores::StoreTrack::from_events(recording.events());
        self.city = recording.city().clone();
        self.recorded_motors = recorded_motors(&recording);
        self.recording = recording;
        self.pair = None;
        self.smoke = None;
        self.vapor = None;
    }

    fn chunk(&mut self, index: usize) -> Option<Arc<Vec<Frame>>> {
        if let Some(at) = self.chunks.iter().position(|(i, _)| *i == index) {
            let entry = self.chunks.remove(at);
            let frames = Arc::clone(&entry.1);
            self.chunks.push(entry);
            return Some(frames);
        }
        match self.recording.decode_chunk(index) {
            Ok(frames) => {
                let frames = Arc::new(frames);
                if self.chunks.len() >= CHUNK_CACHE {
                    self.chunks.remove(0);
                }
                self.chunks.push((index, Arc::clone(&frames)));
                Some(frames)
            }
            Err(error) => {
                log::warn!("Replay: chunk {index} is unreadable: {error}");
                None
            }
        }
    }

    /// The frame recorded at `tick`; inside a gap, the last one before it.
    /// Returns the chunk's frames and the frame's place among them.
    pub fn frame(&mut self, tick: u64) -> Option<(Arc<Vec<Frame>>, usize)> {
        let chunks = self.recording.chunks();
        let after = chunks.partition_point(|c| c.first_tick <= tick);
        let info = *chunks.get(after.checked_sub(1)?)?;
        let frames = self.chunk(after - 1)?;
        let at = (tick.min(info.last_tick()) - info.first_tick) as usize;
        (at < frames.len()).then_some((frames, at))
    }

    /// Aircraft `id`'s recorded state at `tick`.
    pub fn aircraft(&mut self, tick: u64, id: u32) -> Option<AircraftState> {
        let (frames, at) = self.frame(tick)?;
        frames[at].aircraft.iter().find(|a| a.id == id).cloned()
    }

    /// The picture recorded at `tick`, with the effects still playing then
    /// when `effects` is true.
    fn snapshot(&mut self, tick: u64, effects: bool) -> Option<RenderSnapshot> {
        let (frames, at) = self.frame(tick)?;
        let frame = &frames[at];
        let playing = if effects {
            convert::playing_effects(&self.recording, frame.tick).unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut snapshot = convert::snapshot(frame, &playing, &self.presentation, &self.identities);
        snapshot.marks = convert::marks_at(&self.marks, frame.tick);
        tore_session::client::convert::stores::put_recorded(
            &self.stores,
            frame.tick,
            std::iter::once(&mut snapshot.player).chain(&mut snapshot.targets),
        );
        // The recorded age of each round dates its motor.
        for (pose, state) in snapshot.projectiles.iter_mut().zip(&frame.projectiles) {
            if let Some(motor) = self.recorded_motors.get(&state.id) {
                pose.powered = !pose.gun && motor.powered(u64::from(state.age));
                continue;
            }
            pose.powered = !pose.gun
                && self
                    .motors
                    .get(&pose.weapon)
                    .copied()
                    .flatten()
                    .is_some_and(|motor| motor.powered(u64::from(state.age)));
        }
        if let Some(turns) = self.rotor_turns(tick) {
            for pose in std::iter::once(&mut snapshot.player).chain(&mut snapshot.targets) {
                if pose.engine.rotor > 0.
                    && let Some(t) = turns.get(&pose.id)
                {
                    pose.engine.rotor_turns = *t;
                }
            }
        }
        Some(snapshot)
    }

    /// Every rotorcraft's blade angle at the frame drawn at `tick`: the
    /// recorded rotor speed summed over every frame up to it, a tick each.
    /// None in a recording without rotor speeds, which costs nothing.
    fn rotor_turns(&mut self, tick: u64) -> Option<BTreeMap<u32, f64>> {
        let (frames, at) = self.frame(tick)?;
        if !frames[at].aircraft.iter().any(|a| a.rotor_speed > 0.) {
            return None;
        }
        let chunks = self.recording.chunks();
        let index = chunks
            .partition_point(|c| c.first_tick <= tick)
            .checked_sub(1)?;
        while self.rotor_starts.len() <= index {
            let next = match self.rotor_starts.len().checked_sub(1) {
                None => BTreeMap::new(),
                Some(before) => {
                    let mut turns = self.rotor_starts[before].clone();
                    let frames = self.chunk(before)?;
                    add_rotor_turns(&mut turns, &frames);
                    turns
                }
            };
            self.rotor_starts.push(next);
        }
        let mut turns = self.rotor_starts[index].clone();
        add_rotor_turns(&mut turns, &frames[..=at]);
        Some(turns)
    }

    /// Where the city fill buildings flat at `tick` stand.
    pub fn city_at(&self, tick: u64) -> Vec<[i32; 2]> {
        self.city.flattened_at(tick)
    }

    /// Where the city fill buildings flat at `tick` stand, with the tick
    /// each fell on (nr-fx4).
    pub fn city_falls_at(&self, tick: u64) -> Vec<([i32; 2], u64)> {
        self.city.fell_ticks_at(tick)
    }

    /// Where every city fill building the recording knocks down stands.
    pub fn city_all(&self) -> Vec<[i32; 2]> {
        self.city.all()
    }

    /// How many recorded city changes lead up to `tick`.
    pub fn city_changes_at(&self, tick: u64) -> usize {
        self.city.changes_at(tick)
    }

    /// The craters and fires alive at `tick`.
    pub fn marks_at(&self, tick: u64) -> Vec<crate::snapshot::MarkPose> {
        convert::marks_at(&self.marks, tick)
    }

    /// The picture at `alpha` of the way from the tick before `tick` to
    /// `tick`, blended as live flight blends its last two ticks.
    pub fn picture(&mut self, tick: u64, alpha: f64) -> RenderSnapshot {
        if self.pair.as_ref().is_none_or(|(at, ..)| *at != tick) {
            let current = self.snapshot(tick, true).unwrap_or_default();
            let previous = if tick > self.first {
                self.snapshot(tick - 1, false)
            } else {
                None
            };
            self.pair = Some((tick, previous, current));
        }
        let (_, previous, current) = self.pair.as_ref().expect("just filled");
        snapshot::interpolate(previous.as_ref(), current, alpha)
    }

    /// Smoke and contrails alive at `tick`, rebuilt from their release ticks
    /// with the simulation's lifetimes, rise and caps. Moving forward from
    /// the tick drawn last, only the puffs released in between are read and
    /// the expired ones dropped; anything else (a seek, reverse play, a jump
    /// of a puff lifetime or more) reads the whole window again.
    pub fn smoke(&mut self, tick: u64) -> &[Smoke; 2] {
        if self.smoke.as_ref().is_none_or(|t| t.tick != tick) {
            self.advance_smoke(tick);
        }
        &self.smoke.as_ref().expect("just filled").layers
    }

    fn advance_smoke(&mut self, tick: u64) {
        let wind = self.recording.header().world.wind_fps;
        let (mut track, from) = match self.smoke.take() {
            Some(track)
                if track.exact && track.tick < tick && tick - track.tick < LONGEST_PUFF_TICKS =>
            {
                #[cfg(test)]
                {
                    self.steps_on[0] += 1;
                }
                let from = track.tick + 1;
                (track, from)
            }
            Some(mut track) => {
                #[cfg(test)]
                {
                    self.rebuilds[0] += 1;
                }
                track.live.clear();
                (track, tick.saturating_sub(LONGEST_PUFF_TICKS - 1))
            }
            None => {
                #[cfg(test)]
                {
                    self.rebuilds[0] += 1;
                }
                (
                    SmokeTrack {
                        tick,
                        live: Vec::new(),
                        layers: [Smoke::default(), Smoke::default()],
                        exact: true,
                    },
                    tick.saturating_sub(LONGEST_PUFF_TICKS - 1),
                )
            }
        };
        track.tick = tick;
        track.exact = true;
        for layer in &mut track.layers {
            layer.wind = wind;
            layer.puffs.clear();
        }
        match self.recording.spawns(from, tick) {
            Ok(spawns) => track.live.extend(
                spawns
                    .puffs
                    .into_iter()
                    .filter(|(_, puff)| puff.kind.lifetime_ticks().is_some()),
            ),
            Err(_) => {
                // Nothing is shown at a tick that cannot be read, and the
                // next one starts over.
                track.live.clear();
                track.exact = false;
            }
        }
        track
            .live
            .retain(|(spawn, puff)| LivePuff::at(*spawn, puff, tick).is_some());
        let in_contrails = |puff: &PuffSpawn| puff.layer == LAYER_CONTRAILS;
        let contrails = track.live.iter().filter(|(_, p)| in_contrails(p)).count();
        // Each layer keeps only its newest puffs up to the caps.
        let mut skip = [
            (track.live.len() - contrails).saturating_sub(SMOKE_PUFF_CAP),
            contrails.saturating_sub(CONTRAIL_PUFF_CAP),
        ];
        for (spawn, spawn_puff) in &track.live {
            let layer = usize::from(in_contrails(spawn_puff));
            if skip[layer] > 0 {
                skip[layer] -= 1;
                continue;
            }
            let puff = LivePuff::at(*spawn, spawn_puff, tick).expect("retained live");
            if let Some(puff) = Self::drifted(&puff, wind) {
                track.layers[layer].puffs.push_back(puff);
            }
        }
        self.smoke = Some(track);
    }

    /// A live puff as the simulation's smoke holds it: drifting with the
    /// recorded wind, a crash-site column also in its cone, as flight drifts
    /// them. `None` for a kind the simulation does not have.
    fn drifted(puff: &LivePuff, wind: [f64; 3]) -> Option<Puff> {
        let kind = match puff.kind {
            PuffKind::Missile => Kind::Missile,
            PuffKind::Aircraft => Kind::Aircraft,
            PuffKind::Contrail => Kind::Contrail,
            PuffKind::Burning => Kind::Burning,
            PuffKind::Other(_) => return None,
        };
        let seconds = puff.age_ticks as f64 / 120.;
        let mut drift = [wind[0], wind[2]];
        if kind == Kind::Burning {
            let mut release = puff.position;
            release[1] -= Kind::Burning.rise() * seconds;
            let cone = tore_sim::combat::smoke::burning_drift(release);
            drift = [drift[0] + cone[0], drift[1] + cone[1]];
        }
        let mut position = puff.position;
        position[0] += drift[0] * seconds;
        position[2] += drift[1] * seconds;
        Some(Puff {
            position,
            kind,
            drift,
            age: u16::try_from(puff.age_ticks).unwrap_or(u16::MAX),
        })
    }

    /// Where the player's recorded pose at `at` puts the wing vapor's
    /// attachment points, and whether the pose carries animated devices.
    /// `scratch` is moved to the pose. `None` when the player is not
    /// recorded there or the aircraft has no vapor points.
    fn vapor_points(
        &mut self,
        at: u64,
        ownship: &Airframe,
        scratch: &mut flight::State,
    ) -> Option<([[f64; 3]; 2], bool)> {
        let player = self.presentation.player;
        let (frames, index) = self.frame(at)?;
        let recorded = frames[index].aircraft.iter().find(|a| a.id == player)?;
        place(scratch, recorded);
        Some((ownship.streamer_points(scratch)?, recorded.flags.animated))
    }

    /// The player's wing vapor history at `tick`, rebuilt by stepping the
    /// vapor over the last [`VAPOR_TICKS`] recorded poses from one of live
    /// flight's commit ticks, so the rebuilt history is the one live flight
    /// had. `scratch` is a flight state for `ownship` whose pose is
    /// overwritten. `None` when the aircraft has no vapor points or the
    /// player is not recorded.
    ///
    /// Playing forward from the tick drawn last, the history is stepped on
    /// through the ticks crossed instead. That is the same history: the
    /// commits follow one schedule from launch, and a history that has
    /// stepped over 250 ticks of it holds only the last ten commits and the
    /// live pose, wherever it started on the schedule. It is only done when
    /// every pose was found, the devices were all animated or all not, and
    /// the history began on the schedule (not at a recording's first pose,
    /// which is a schedule of its own); otherwise, and on a seek, reverse
    /// play or a jump of more than [`VAPOR_TICKS`], the rebuild runs.
    pub fn vapor(
        &mut self,
        tick: u64,
        ownship: &Airframe,
        scratch: &mut flight::State,
    ) -> Option<Vapor> {
        if let Some(track) = &self.vapor
            && track.tick == tick
        {
            return track.vapor;
        }
        let (first, last) = (self.first, self.last);
        if let Some(track) = &self.vapor
            && let Some(mut vapor) = track.vapor
            && track.exact
            && track.tick >= first
            && track.tick < tick
            && tick <= last
            && tick - track.tick <= VAPOR_TICKS
            && track.scheduled == (self.vapor_start(tick) >= first)
        {
            let (mut animated, scheduled) = (track.animated, track.scheduled);
            let mut found = true;
            for at in track.tick + 1..=tick {
                match self.vapor_points(at, ownship, scratch) {
                    Some((points, now)) if animated.is_none_or(|before| before == now) => {
                        animated = Some(now);
                        vapor.step(native_ticks(at), points);
                    }
                    _ => {
                        found = false;
                        break;
                    }
                }
            }
            if found {
                #[cfg(test)]
                {
                    self.steps_on[1] += 1;
                }
                self.vapor = Some(VaporTrack {
                    tick,
                    vapor: Some(vapor),
                    exact: true,
                    scheduled,
                    animated,
                });
                return Some(vapor);
            }
        }
        self.rebuild_vapor(tick, ownship, scratch)
    }

    /// The commit tick of live flight's schedule the rebuild at `tick`
    /// starts from.
    fn vapor_start(&self, tick: u64) -> u64 {
        let back = tick.saturating_sub(VAPOR_TICKS);
        self.commits[self.commits.partition_point(|c| *c <= back) - 1]
    }

    /// The vapor history at `tick` from the recorded poses alone.
    fn rebuild_vapor(
        &mut self,
        tick: u64,
        ownship: &Airframe,
        scratch: &mut flight::State,
    ) -> Option<Vapor> {
        #[cfg(test)]
        {
            self.rebuilds[1] += 1;
        }
        let start = self.vapor_start(tick);
        let (first, last) = (self.first, self.last);
        let mut animated: Option<bool> = None;
        let mut exact = true;
        let mut note = |now: bool, exact: &mut bool| {
            if animated.is_some_and(|before| before != now) {
                *exact = false;
            }
            animated = Some(now);
        };
        // Live flight seeds the history at launch and commits on its own
        // schedule; starting on one of its commit ticks lines the rebuilt
        // history up with it once the seed has been pushed out. A recording
        // that starts after launch seeds from its first pose.
        let vapor = self
            .vapor_points(start.max(first), ownship, scratch)
            .map(|(seed, now)| {
                note(now, &mut exact);
                let mut vapor = Vapor::seeded(seed);
                if start > 0 && start >= first {
                    vapor.step(native_ticks(start), seed);
                }
                for at in (start + 1).max(first)..=tick.min(last) {
                    if let Some((p, now)) = self.vapor_points(at, ownship, scratch) {
                        note(now, &mut exact);
                        vapor.step(native_ticks(at), p);
                    } else {
                        exact = false;
                    }
                }
                vapor
            });
        self.vapor = Some(VaporTrack {
            tick,
            vapor,
            exact: exact && vapor.is_some(),
            scheduled: start >= first,
            animated,
        });
        vapor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::fixture;
    use crate::replay::tests::TempDir;
    use tore_replay::LAYER_SMOKE;

    fn playback(dir: &TempDir, name: &str) -> Playback {
        Playback::new(Arc::new(fixture::recording(dir.path(), name)))
    }

    /// A format 2 file the writer before format 3 made (the tore-replay
    /// crate keeps one): it plays, with pictures that hold no surface units
    /// and the draw rules its header gives.
    #[test]
    fn recorded_energy_motor_is_independent_of_imported_timers_and_seek_direction() {
        use tore_replay::{
            Event, Value,
            vocab::{field, kind},
        };
        let old = Event::new(kind::WEAPON_LAUNCH).with(field::PROJECTILE, Value::Id(7));
        assert!(RecordedMotor::read(&old).is_none());
        let event = old
            .with("missile-model", "energy-1")
            .with("motor-ignition", 30i64)
            .with("motor-boost-end", 270i64)
            .with("motor-end", 510i64)
            .with("motor-boost-powered", true)
            .with("motor-sustain-powered", false);
        let (id, motor) = RecordedMotor::read(&event).unwrap();
        assert_eq!(id, 7);
        for age in [0, 29, 30, 269, 270, 509, 510, 269, 30, 29] {
            assert_eq!(motor.powered(age), (30..270).contains(&age));
        }
    }
    #[test]
    fn a_format_2_recording_plays_with_no_surface_content() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tore-replay/tests/golden/format2.tore-replay");
        let recording = Arc::new(Recording::open(path).unwrap());
        assert_eq!(recording.header().format_version, 2);
        assert_eq!(recording.header().world.ground_target, None);
        let (first, last) = (
            recording.first_tick().unwrap(),
            recording.last_tick().unwrap(),
        );
        let mut playback = Playback::new(Arc::clone(&recording));
        for tick in [first, first + 10, last] {
            let picture = playback.picture(tick, 1.);
            assert_eq!(picture.tick, tick);
            assert!(picture.surface.is_empty());
            assert!(!picture.targets.is_empty() || !picture.projectiles.is_empty());
        }
        // The identities and the registry of a format 2 file are as ever.
        assert_eq!(playback.identities.surface.len(), 0);
        // From before the city section (nr-fx3): the city stands whole.
        assert!(playback.city_all().is_empty());
        assert!(playback.city_at(last).is_empty());
    }

    #[test]
    fn a_replayed_crash_column_ages_as_flight_ages_it() {
        use tore_sim::combat::smoke;
        assert_eq!(
            PuffKind::Burning.lifetime_ticks(),
            Some(u64::from(smoke::BURNING_LIFETIME_TICKS))
        );
        assert_eq!(PuffKind::Burning.rise_fps(), smoke::Kind::Burning.rise());
    }

    #[test]
    fn seeking_equals_reading_the_recording_in_order() {
        let dir = TempDir::new("playback-seek");
        let mut playback = playback(&dir, "seek");
        let recording = Arc::clone(playback.recording());
        let sequential: Vec<Frame> = recording
            .frames(fixture::FIRST, fixture::LAST)
            .map(Result::unwrap)
            .collect();
        // Every tick in a scattered order, crossing chunk boundaries both
        // ways, lands on the frame reading in order gave.
        let ticks: Vec<u64> = (0..sequential.len() as u64)
            .map(|i| (i * 7_919) % sequential.len() as u64)
            .collect();
        for i in ticks {
            let expected = &sequential[i as usize];
            let (frames, at) = playback.frame(expected.tick).unwrap();
            assert_eq!(&frames[at], expected);
        }
        // Inside the gap the last frame before it holds.
        let (frames, at) = playback.frame(fixture::GAP.0 + 10).unwrap();
        assert_eq!(frames[at].tick, fixture::GAP.0 - 1);
        assert!(playback.frame(fixture::FIRST - 1).is_none());
        let (frames, at) = playback.frame(fixture::LAST + 100).unwrap();
        assert_eq!(frames[at].tick, fixture::LAST);
        assert!(playback.chunks.len() <= CHUNK_CACHE);
    }

    #[test]
    fn a_whole_tick_shows_exactly_that_tick() {
        let dir = TempDir::new("playback-tick");
        let mut playback = playback(&dir, "tick");
        let recording = Arc::clone(playback.recording());
        let snapshot = |playback: &Playback, tick: u64, effects: bool| {
            let frame = recording.frame(tick).unwrap().unwrap();
            let effects = if effects {
                convert::playing_effects(&recording, tick).unwrap()
            } else {
                Vec::new()
            };
            convert::snapshot(
                &frame,
                &effects,
                &playback.presentation,
                &playback.identities,
            )
        };
        for tick in [
            fixture::FIRST,
            fixture::LAUNCH + 3,
            fixture::KILL,
            fixture::LAST,
        ] {
            let exact = snapshot(&playback, tick, true);
            let previous = (tick > fixture::FIRST).then(|| snapshot(&playback, tick - 1, false));
            let picture = playback.picture(tick, 1.);
            assert_eq!(
                picture,
                snapshot::interpolate(previous.as_ref(), &exact, 1.),
                "tick {tick}"
            );
            // Blending all the way lands on the tick itself.
            for (drawn, recorded) in picture.targets.iter().zip(&exact.targets) {
                for i in 0..3 {
                    assert!((drawn.position[i] - recorded.position[i]).abs() < 1e-6);
                }
            }
            assert_eq!(picture.projectiles, exact.projectiles);
            assert_eq!(picture.effects, exact.effects);
        }
        // The launch flash plays from the launch tick.
        assert_eq!(playback.picture(fixture::LAUNCH + 3, 1.).effects.len(), 1);
        // Halfway, aircraft sit halfway between the two ticks.
        let half = playback.picture(301, 0.5);
        let [a, b] = [300, 301].map(|t| fixture::position(1, t));
        let lead = half.targets.iter().find(|t| t.id == 1).unwrap();
        for i in 0..3 {
            assert!((lead.position[i] - (a[i] + b[i]) / 2.).abs() < 0.05);
        }
    }

    #[test]
    fn smoke_is_rebuilt_from_release_ticks() {
        let dir = TempDir::new("playback-smoke");
        let mut playback = playback(&dir, "smoke");
        let tick = fixture::LAUNCH + 100;
        let [smoke, contrails] = playback.smoke(tick).clone();
        // Missile puffs every 8 ticks from the launch, the newest first
        // released this tick or earlier.
        let released = (fixture::LAUNCH..=tick).filter(|t| t % 8 == 0).count();
        assert_eq!(smoke.puffs.len(), released);
        assert!(smoke.puffs.iter().all(|p| p.kind == Kind::Missile));
        let newest = smoke.puffs.back().unwrap();
        assert_eq!(u64::from(newest.age), tick - tick / 8 * 8);
        // Contrails every 12 ticks since the start, none lost to age yet.
        assert_eq!(
            contrails.puffs.len(),
            (fixture::FIRST..=tick).filter(|t| t % 12 == 0).count()
        );
        // Missile smoke has gone four seconds after the last puff.
        let [smoke, _] = playback.smoke(fixture::IMPACT + 480).clone();
        assert!(smoke.puffs.is_empty());
    }

    #[test]
    fn vapor_rebuilt_at_any_tick_matches_stepping_from_launch() {
        let dir = TempDir::new("playback-vapor");
        let mut playback = playback(&dir, "vapor");
        let mut ownship = crate::combat_view::render_hash_tests::hornet_airframe(true);
        ownship.streamer = Some(tore_formats::shape::StreamerDef {
            pivot: [0; 3],
            hinge_scale: 0,
            points: [
                [-30 * 256, 2 * 256, -10 * 256],
                [30 * 256, 2 * 256, -10 * 256],
            ],
        });
        let template = crate::combat_view::render_hash_tests::player();
        let mut scratch = template.clone();
        // Live flight's vapor: seeded at launch, stepped every tick.
        let mut live = None;
        let mut expected = std::collections::BTreeMap::new();
        for tick in fixture::FIRST..=fixture::LAST {
            let Some(recorded) = playback.aircraft(tick, 0) else {
                continue;
            };
            place(&mut scratch, &recorded);
            let points = ownship.streamer_points(&scratch).unwrap();
            let vapor = live.get_or_insert_with(|| Vapor::seeded(points));
            vapor.step(native_ticks(tick), points);
            expected.insert(tick, *vapor);
        }
        let mut scratch = template.clone();
        for tick in [300, 350, 399, 460, 520, 899, 900, 610, 611, 609] {
            let rebuilt = playback.vapor(tick, &ownship, &mut scratch).unwrap();
            let live = expected[&tick];
            for side in 0..2 {
                assert_eq!(
                    rebuilt.trail(side, 6.5, 30., false),
                    live.trail(side, 6.5, 30., false),
                    "tick {tick}"
                );
            }
        }
        // Early on there is less history, and it still matches.
        let rebuilt = playback.vapor(fixture::FIRST + 40, &ownship, &mut scratch);
        assert!(rebuilt.is_some());
        // An airframe without vapor points has none.
        let bare = crate::combat_view::render_hash_tests::hornet_airframe(true);
        playback.vapor = None;
        assert!(playback.vapor(500, &bare, &mut scratch).is_none());
    }

    /// Everything the viewer rebuilds for one playhead position.
    #[derive(Debug, PartialEq)]
    struct Moment {
        picture: RenderSnapshot,
        smoke: [Smoke; 2],
        vapor: Option<Vapor>,
        destroyed: std::collections::BTreeSet<u32>,
        weather: (
            tore_sim::environment::Environment,
            tore_sim::environment::Presentation,
        ),
    }

    #[test]
    fn reverse_play_shows_exactly_what_forward_play_showed() {
        use crate::replay::clock::Clock;
        use crate::replay::tracks::Tracks;
        use crate::replay::weather::WeatherTrack;
        use std::collections::BTreeMap;
        let dir = TempDir::new("playback-reverse");
        let recording = Arc::new(fixture::recording(dir.path(), "reverse"));
        let tracks = Tracks::scan(&recording);
        let mut ownship = crate::combat_view::render_hash_tests::hornet_airframe(true);
        ownship.streamer = Some(tore_formats::shape::StreamerDef {
            pivot: [0; 3],
            hinge_scale: 0,
            points: [[-30 * 256, 0, 0], [30 * 256, 0, 0]],
        });
        let template = crate::combat_view::render_hash_tests::player();
        // Plays the whole recording at half speed, one real tick at a time,
        // in `direction`, noting everything at every half-tick position.
        let play = |reverse: bool| {
            let mut playback = Playback::new(Arc::clone(&recording));
            let mut world = tore_world::test_support::terrain();
            let mut scenery = crate::scenery::tests::scenery();
            let mut weather = WeatherTrack::new(&world, &scenery, fixture::LAST);
            let mut scratch = template.clone();
            let mut clock = Clock::new(fixture::FIRST, fixture::LAST);
            if reverse {
                clock.end();
                clock.reverse();
            }
            clock.slower();
            clock.slower();
            assert_eq!(clock.speed(), 0.5);
            let mut seen = BTreeMap::new();
            loop {
                let tick = clock.tick();
                weather.seek(&mut world, &mut scenery, &tracks, tick);
                let moment = Moment {
                    picture: playback.picture(tick, clock.alpha()),
                    smoke: playback.smoke(tick).clone(),
                    vapor: playback.vapor(tick, &ownship, &mut scratch),
                    destroyed: tracks.destroyed(tick),
                    weather: (world.weather.clone(), scenery.weather_presentation.clone()),
                };
                seen.insert((clock.position() * 2.).round() as u64, moment);
                if !clock.advance(1. / 120.) {
                    break;
                }
            }
            seen
        };
        let forward = play(false);
        let reverse = play(true);
        assert_eq!(
            forward.len() as u64,
            (fixture::LAST - fixture::FIRST) * 2 + 1
        );
        assert_eq!(
            forward.keys().collect::<Vec<_>>(),
            reverse.keys().collect::<Vec<_>>()
        );
        for (position, moment) in &forward {
            assert!(
                *moment == reverse[position],
                "playhead {} differs",
                *position as f64 / 2.
            );
        }
        // The walk saw the missile, its smoke, the kill and the fallen
        // building.
        let at = |tick: u64| &forward[&(tick * 2)];
        assert_eq!(at(fixture::LAUNCH + 10).picture.projectiles.len(), 1);
        assert!(!at(fixture::LAUNCH + 100).smoke[0].puffs.is_empty());
        assert!(at(fixture::KILL + 5).picture.effects.len() == 1);
        assert!(at(fixture::LAST).destroyed.contains(&fixture::SURFACE[0].1));
        assert!(at(fixture::LAST).vapor.is_some());
    }

    #[test]
    fn commits_follow_the_native_clock() {
        let commits = commit_ticks(200);
        assert_eq!(commits[..4], [0, 12, 24, 36]);
        for pair in commits.windows(2) {
            assert!(native_ticks(pair[1]) - native_ticks(pair[0]) >= COMMIT_TICKS);
            assert!(native_ticks(pair[1] - 1) - native_ticks(pair[0]) < COMMIT_TICKS);
        }
    }

    /// A recording read again while it is still being written: what the
    /// playback shows at every tick is what the finished recording shows,
    /// and the craters and fires, the registry and the vapor's commits
    /// follow the new frames.
    #[test]
    fn a_growing_recording_plays_as_the_finished_one_does() {
        let dir = TempDir::new("playback-grow");
        let mut growing = fixture::Growing::new(dir.path(), "growing");
        growing.push(5, 300);
        let early = growing.open();
        let mut grown = Playback::new(Arc::clone(&early));
        let before = early.last_tick().unwrap();
        assert!((260..300).contains(&before), "{before}");
        assert_eq!(grown.marks_at(before).len(), 1, "the first crater");
        // The newest tick of the early read, drawn and cached.
        assert!(grown.picture(before, 1.).targets.len() >= 2);
        // More frames, and the recording is read again.
        growing.push(301, 700);
        growing.register(&tore_replay::AircraftInfo {
            id: 9,
            pt: "MIG29.PT".into(),
            ..Default::default()
        });
        growing.push(701, 760);
        let later = growing.open();
        let newest = later.last_tick().unwrap();
        assert!(newest > 700);
        grown.extend(Arc::clone(&later));
        let mut finished = Playback::new(Arc::new(growing.finish()));
        // Every kind of tick: before the first read, the first read's
        // newest, just after it, either side of the second crater.
        for tick in [5, 100, before, before + 1, 450, 639, 640, newest] {
            assert_eq!(
                grown.picture(tick, 1.),
                finished.picture(tick, 1.),
                "{tick}"
            );
            assert_eq!(grown.marks_at(tick), finished.marks_at(tick), "{tick}");
        }
        assert_eq!(grown.marks_at(newest).len(), 2);
        assert_eq!(grown.commits, commit_ticks(newest));
        assert!(grown.identities.aircraft.contains_key(&9));
    }

    /// A recording of a rotorcraft (id 0) whose rotor droops and recovers
    /// across several 40-tick chunks, beside an aircraft without rotors.
    fn rotorcraft_recording(dir: &TempDir) -> Recording {
        use tore_replay::{AircraftFlags, AircraftInfo, Frame, Writer, WriterOptions};
        let path = dir.path().join("rotors.tore-replay");
        let mut writer = Writer::create_with(
            &path,
            &tore_replay::Header::default(),
            WriterOptions {
                chunk_ticks: 40,
                sync_ticks: 3_600,
            },
        )
        .unwrap();
        for (id, pt) in [(0, "AH64.PT"), (1, "F18.PT")] {
            writer
                .register_aircraft(&AircraftInfo {
                    id,
                    pt: pt.into(),
                    ..Default::default()
                })
                .unwrap();
        }
        for tick in 0..=200 {
            let aircraft = [0, 1].map(|id| AircraftState {
                id,
                position: [f64::from(id) * 500., 3_000., tick as f64],
                flags: AircraftFlags {
                    engine_on: true,
                    airborne: true,
                    alive: true,
                    ..Default::default()
                },
                rotor_speed: if id == 0 { rotor_speed(tick) } else { 0. },
                disk_tilt: if id == 0 {
                    disk_tilt(tick)
                } else {
                    [[0.; 2]; 2]
                },
                hp: 100,
                max_hp: 100,
                ..Default::default()
            });
            writer
                .push(&Frame {
                    tick,
                    aircraft: aircraft.into(),
                    ..Default::default()
                })
                .unwrap();
        }
        let path = writer.finish(&tore_replay::Footer::default()).unwrap();
        Recording::open(path).unwrap()
    }

    /// The test rotorcraft's rotor speed at `tick`, in recorded thousandths.
    fn rotor_speed(tick: u64) -> f64 {
        let droop = (tick as f64 / 100.).min(1.) * 0.2;
        ((1. - droop + if tick > 150 { 0.15 } else { 0. }) * 1000.).round() / 1000.
    }

    /// The test rotorcraft's disk tilt at `tick`: leaning forward more as the
    /// flight goes on, with a steady lateral lean.
    fn disk_tilt(tick: u64) -> [[f64; 2]; 2] {
        [[0.02 + tick as f64 * 0.002, -0.02], [0., 0.]]
    }

    #[test]
    fn replayed_disks_tilt_as_recorded_and_blend_between_ticks() {
        let dir = TempDir::new("playback-tilts");
        let recording = Arc::new(rotorcraft_recording(&dir));
        let tilt = |picture: &RenderSnapshot, id: u32| {
            std::iter::once(&picture.player)
                .chain(&picture.targets)
                .find(|pose| pose.id == id)
                .unwrap()
                .engine
                .rotor_tilt
        };
        let step = tore_replay::precision::DISK_TILT;
        let mut playback = Playback::new(Arc::clone(&recording));
        for tick in (0..=200).step_by(3) {
            let picture = playback.picture(tick, 1.);
            let read = tilt(&picture, 0);
            let truth = disk_tilt(tick);
            for (rotor, axis) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                assert!(
                    (read[rotor][axis] - truth[rotor][axis]).abs() <= step / 2. + 1e-9,
                    "tick {tick}: {read:?} against {truth:?}"
                );
            }
            // The aircraft without rotors stays level.
            assert_eq!(tilt(&picture, 1), [[0.; 2]; 2]);
        }
        // Played backwards, and between ticks, the disk is where the
        // recording had it, blended as live flight blends two ticks.
        let half = playback.picture(120, 0.5);
        let blended = tilt(&half, 0)[0][0];
        let expected = (disk_tilt(119)[0][0] + disk_tilt(120)[0][0]) / 2.;
        assert!(
            (blended - expected).abs() <= step + 1e-9,
            "{blended} {expected}"
        );
        let back = playback.picture(37, 1.);
        assert!((tilt(&back, 0)[0][0] - disk_tilt(37)[0][0]).abs() <= step / 2. + 1e-9);
    }

    #[test]
    fn replayed_blades_turn_by_the_recorded_rotor_speed_wherever_playback_starts() {
        let dir = TempDir::new("playback-rotors");
        let recording = Arc::new(rotorcraft_recording(&dir));
        let turns = |playback: &mut Playback, tick: u64| {
            let picture = playback.picture(tick, 1.);
            let player = std::iter::once(&picture.player)
                .chain(&picture.targets)
                .find(|pose| pose.id == 0)
                .unwrap()
                .clone();
            (player.engine.rotor, player.engine.rotor_turns)
        };
        // The recorded rotor speed, a tick at a time from the first frame.
        let exact = |tick: u64| (0..=tick).map(rotor_speed).sum::<f64>() * flight::DT;
        let mut forward = Playback::new(Arc::clone(&recording));
        let mut last = 0.;
        for tick in 0..=200 {
            let (rotor, turns) = turns(&mut forward, tick);
            assert!((rotor - rotor_speed(tick)).abs() < 1e-9, "{tick}");
            assert!((turns - exact(tick)).abs() < 1e-9, "{tick}: {turns}");
            // Across chunk boundaries too, a tick turns the blades by that
            // tick's rotor speed.
            if tick > 0 {
                let step = turns - last;
                assert!(
                    (step - rotor_speed(tick) * flight::DT).abs() < 1e-9,
                    "{tick}"
                );
            }
            last = turns;
        }
        // Played backwards, or opened straight at a late tick, the blades
        // stand where forward play had them.
        let mut backward = Playback::new(Arc::clone(&recording));
        for tick in (0..=200).rev().step_by(7) {
            assert!((turns(&mut backward, tick).1 - exact(tick)).abs() < 1e-9);
        }
        let mut late = Playback::new(Arc::clone(&recording));
        assert!((turns(&mut late, 185).1 - exact(185)).abs() < 1e-9);
        // Between ticks the picture blends them.
        let half = forward.picture(120, 0.5);
        let pose = std::iter::once(&half.player)
            .chain(&half.targets)
            .find(|pose| pose.id == 0)
            .unwrap();
        assert!((pose.engine.rotor_turns - (exact(119) + exact(120)) / 2.).abs() < 1e-9);
        // The aircraft without rotors has none.
        let picture = forward.picture(100, 1.);
        let other = std::iter::once(&picture.player)
            .chain(&picture.targets)
            .find(|pose| pose.id == 1)
            .unwrap();
        assert_eq!((other.engine.rotor, other.engine.rotor_turns), (0., 0.));
    }

    /// What the viewer did before playback stepped puffs and poses on: every
    /// puff alive at `tick` read from the recording and built again.
    fn smoke_rebuilt(playback: &Playback, tick: u64) -> [Smoke; 2] {
        let mut layers = [Smoke::default(), Smoke::default()];
        for layer in &mut layers {
            layer.wind = playback.recording.header().world.wind_fps;
        }
        for puff in playback.recording.live_puffs(tick).unwrap_or_default() {
            let kind = match puff.kind {
                PuffKind::Missile => Kind::Missile,
                PuffKind::Aircraft => Kind::Aircraft,
                PuffKind::Contrail => Kind::Contrail,
                PuffKind::Burning => Kind::Burning,
                PuffKind::Other(_) => continue,
            };
            let seconds = puff.age_ticks as f64 / 120.;
            let wind = playback.recording.header().world.wind_fps;
            let mut drift = [wind[0], wind[2]];
            if kind == Kind::Burning {
                let mut release = puff.position;
                release[1] -= Kind::Burning.rise() * seconds;
                let cone = tore_sim::combat::smoke::burning_drift(release);
                drift = [drift[0] + cone[0], drift[1] + cone[1]];
            }
            let mut position = puff.position;
            position[0] += drift[0] * seconds;
            position[2] += drift[1] * seconds;
            layers[usize::from(puff.layer == LAYER_CONTRAILS)]
                .puffs
                .push_back(Puff {
                    position,
                    kind,
                    drift,
                    age: u16::try_from(puff.age_ticks).unwrap_or(u16::MAX),
                });
        }
        layers
    }

    /// The vapor rebuild as it was before playback stepped it on: every
    /// pose of the window fetched and cloned, nothing remembered.
    fn vapor_rebuilt(
        playback: &mut Playback,
        tick: u64,
        ownship: &Airframe,
        scratch: &mut flight::State,
    ) -> Option<Vapor> {
        let back = tick.saturating_sub(VAPOR_TICKS);
        let start = playback.commits[playback.commits.partition_point(|c| *c <= back) - 1];
        let (first, last) = (playback.first, playback.last);
        let mut points = |this: &mut Playback, at: u64| {
            let player = this.presentation.player;
            let recorded = this.aircraft(at, player)?;
            place(scratch, &recorded);
            ownship.streamer_points(scratch)
        };
        points(playback, start.max(first)).map(|seed| {
            let mut vapor = Vapor::seeded(seed);
            if start > 0 && start >= first {
                vapor.step(native_ticks(start), seed);
            }
            for at in (start + 1).max(first)..=tick.min(last) {
                if let Some(p) = points(playback, at) {
                    vapor.step(native_ticks(at), p);
                }
            }
            vapor
        })
    }

    /// Playheads of a viewing session over `first..=last`: forward a tick at
    /// a time (past both ends), a seek, forward again, reverse across the
    /// middle, the speeds one after another (2x to 64x, and a jump too long
    /// to step), a tick asked twice, a seek to each end and back.
    fn session(first: u64, last: u64) -> Vec<u64> {
        let mut ticks: Vec<u64> = (first.saturating_sub(4)..=last + 4).collect();
        let at = |fraction: f64| first + ((last - first) as f64 * fraction) as u64;
        ticks.push(at(0.1));
        ticks.extend(at(0.1) + 1..at(0.1) + 40);
        ticks.extend((at(0.4)..at(0.6)).rev());
        for speed in [2, 4, 8, 16, 64, 249, 250, 251] {
            ticks.extend((0..6).map(|i| at(0.2) + i * speed));
        }
        ticks.extend([at(0.3), at(0.3), at(0.3) + 1, last, first, last, at(0.5)]);
        ticks
    }

    #[test]
    fn smoke_stepped_forward_is_the_smoke_rebuilt_at_every_tick() {
        let dir = TempDir::new("playback-smoke-forward");
        let mut playback = playback(&dir, "smokes");
        let (first, last) = (playback.first, playback.last);
        let mut moves = 0;
        for tick in session(first, last) {
            let drawn = playback.smoke(tick).clone();
            assert_eq!(drawn, smoke_rebuilt(&playback, tick), "tick {tick}");
            moves += 1;
        }
        // Most of the session is playing forward: those ticks did not read
        // the recording's window again.
        let [rebuilds, steps] = [playback.rebuilds[0], playback.steps_on[0]];
        assert!(
            steps > 800 && rebuilds < 260,
            "{steps} steps, {rebuilds} rebuilds"
        );
        assert!(moves > 1_000);
        assert!(!playback.smoke(last)[1].puffs.is_empty());
    }

    /// A recording of `ticks` ticks with `spawns` of puffs every tick and
    /// the player (id 0) flown by `player` where it returns a pose.
    fn synthetic(
        dir: &TempDir,
        name: &str,
        range: std::ops::RangeInclusive<u64>,
        spawns: impl Fn(u64) -> Vec<PuffSpawn>,
        player: impl Fn(u64) -> Option<AircraftState>,
    ) -> Recording {
        use tore_replay::{AircraftInfo, Frame, Writer, WriterOptions};
        let path = dir.path().join(format!("{name}.tore-replay"));
        let mut writer = Writer::create_with(
            &path,
            &tore_replay::Header::default(),
            WriterOptions {
                chunk_ticks: 120,
                sync_ticks: 3_600,
            },
        )
        .unwrap();
        writer
            .register_aircraft(&AircraftInfo {
                id: 0,
                pt: "F18.PT".into(),
                ..Default::default()
            })
            .unwrap();
        for tick in range {
            writer
                .push(&Frame {
                    tick,
                    aircraft: player(tick).into_iter().collect(),
                    new_puffs: spawns(tick),
                    ..Default::default()
                })
                .unwrap();
        }
        let path = writer.finish(&tore_replay::Footer::default()).unwrap();
        Recording::open(path).unwrap()
    }

    fn puff(layer: u8, kind: PuffKind, tick: u64, n: u64) -> PuffSpawn {
        let k = (tick * 31 + n * 7) % 997;
        PuffSpawn {
            layer,
            kind,
            position: [
                100_000. + k as f64 * 3.5,
                6_000. + (n * 40 + tick % 90) as f64,
                90_000. + (k * 5 % 777) as f64,
            ],
        }
    }

    #[test]
    fn puffs_of_every_kind_expire_and_are_capped_as_the_rebuild_does() {
        let dir = TempDir::new("playback-puff-kinds");
        // Every kind, one unknown kind, and enough aircraft smoke that the
        // smoke layer passes its cap once the first puffs are 960 ticks old.
        let recording = synthetic(
            &dir,
            "kinds",
            0..=1_500,
            |t| {
                let mut spawns: Vec<PuffSpawn> = (0..10)
                    .map(|n| puff(LAYER_SMOKE, PuffKind::Aircraft, t, n))
                    .collect();
                if t % 3 == 0 {
                    spawns.push(puff(LAYER_SMOKE, PuffKind::Missile, t, 11));
                }
                if t % 97 == 0 {
                    spawns.push(puff(LAYER_SMOKE, PuffKind::Burning, t, 12));
                }
                if t % 2 == 0 {
                    spawns.push(puff(LAYER_CONTRAILS, PuffKind::Contrail, t, 13));
                }
                if t % 111 == 0 {
                    spawns.push(puff(LAYER_SMOKE, PuffKind::Other(9), t, 14));
                }
                spawns
            },
            |_| None,
        );
        let mut playback = Playback::new(Arc::new(recording));
        let mut capped = false;
        for tick in (0..=1_500).chain([200, 199, 198, 900, 1_400, 1_401, 1_500]) {
            let drawn = playback.smoke(tick).clone();
            capped |= drawn[0].puffs.len() == SMOKE_PUFF_CAP;
            assert_eq!(drawn, smoke_rebuilt(&playback, tick), "tick {tick}");
        }
        assert!(capped, "the smoke layer reached its cap");
        assert!(playback.rebuilds[0] < 8, "{}", playback.rebuilds[0]);
    }

    #[test]
    fn contrails_past_their_cap_keep_the_newest_as_the_rebuild_does() {
        let dir = TempDir::new("playback-contrail-cap");
        let recording = synthetic(
            &dir,
            "contrails",
            0..=3_700,
            |t| {
                (0..20)
                    .map(|n| puff(LAYER_CONTRAILS, PuffKind::Contrail, t, n))
                    .collect()
            },
            |_| None,
        );
        let mut playback = Playback::new(Arc::new(recording));
        // Forward in strides (each far fewer ticks than a puff's lifetime),
        // across the cap at 3,600 ticks of 20.
        for tick in [
            100, 150, 1_000, 2_999, 3_599, 3_600, 3_601, 3_650, 3_700, 50,
        ] {
            let drawn = playback.smoke(tick).clone();
            assert_eq!(drawn, smoke_rebuilt(&playback, tick), "tick {tick}");
        }
        assert_eq!(playback.smoke(3_700)[1].puffs.len(), CONTRAIL_PUFF_CAP);
        assert_eq!(playback.rebuilds[0], 2, "first read and the seek back");
    }

    /// The ownship the vapor tests fly: wing points on both sides.
    fn winged() -> Airframe {
        let mut ownship = crate::combat_view::render_hash_tests::hornet_airframe(true);
        ownship.streamer = Some(tore_formats::shape::StreamerDef {
            pivot: [0; 3],
            hinge_scale: 0,
            points: [
                [-30 * 256, 2 * 256, -10 * 256],
                [30 * 256, 2 * 256, -10 * 256],
            ],
        });
        ownship
    }

    #[test]
    fn vapor_stepped_forward_is_the_vapor_rebuilt_at_every_tick() {
        let dir = TempDir::new("playback-vapor-forward");
        let mut playback = playback(&dir, "vapors");
        let ownship = winged();
        let template = crate::combat_view::render_hash_tests::player();
        let (mut scratch, mut reference) = (template.clone(), template);
        let (first, last) = (playback.first, playback.last);
        let mut checked = Playback::new(Arc::clone(playback.recording()));
        for tick in session(first, last) {
            let drawn = playback.vapor(tick, &ownship, &mut scratch);
            let rebuilt = vapor_rebuilt(&mut checked, tick, &ownship, &mut reference);
            assert_eq!(drawn, rebuilt, "tick {tick}");
        }
        let [rebuilds, steps] = [playback.rebuilds[1], playback.steps_on[1]];
        assert!(
            steps > 800 && rebuilds < 260,
            "{steps} steps, {rebuilds} rebuilds"
        );
    }

    /// The player at `tick` of the vapor edge recordings: a climbing turn,
    /// animated unless `animated` says otherwise.
    fn flown(tick: u64, animated: bool) -> AircraftState {
        let t = tick as f64 / 120.;
        AircraftState {
            id: 0,
            position: [
                100_000. + t.sin() * 3_000.,
                8_000. + t * 15.,
                90_000. + t.cos() * 3_000.,
            ],
            attitude: [t * 0.3, 0.05, 0.6 + (t * 2.).sin() * 0.3],
            flags: tore_replay::AircraftFlags {
                engine_on: true,
                airborne: true,
                alive: true,
                animated,
                ..Default::default()
            },
            hp: 100,
            max_hp: 100,
            ..Default::default()
        }
    }

    /// Plays `recording` through `session` and holds every tick to the
    /// rebuild; returns the playback.
    fn vapor_matches_rebuild_at_every_tick(recording: Recording) -> Playback {
        let ownship = winged();
        let template = crate::combat_view::render_hash_tests::player();
        let (mut scratch, mut reference) = (template.clone(), template);
        let recording = Arc::new(recording);
        let mut playback = Playback::new(Arc::clone(&recording));
        let mut checked = Playback::new(recording);
        let (first, last) = (playback.first, playback.last);
        for tick in session(first, last) {
            let drawn = playback.vapor(tick, &ownship, &mut scratch);
            let rebuilt = vapor_rebuilt(&mut checked, tick, &ownship, &mut reference);
            assert_eq!(drawn, rebuilt, "tick {tick}");
        }
        playback
    }

    #[test]
    fn vapor_stepped_forward_matches_a_rebuild_wherever_the_recording_begins() {
        // From tick 0 (the history is live flight's from launch), and from
        // late ticks, one on the commit schedule and one off it.
        for (name, first) in [("zero", 0), ("late", 143), ("later", 1_000)] {
            let dir = TempDir::new(&format!("playback-vapor-{name}"));
            let recording = synthetic(
                &dir,
                name,
                first..=first + 900,
                |_| Vec::new(),
                |t| Some(flown(t, true)),
            );
            let playback = vapor_matches_rebuild_at_every_tick(recording);
            assert!(
                playback.steps_on[1] > 800,
                "{name}: {}",
                playback.steps_on[1]
            );
        }
    }

    #[test]
    fn vapor_falls_back_to_the_rebuild_where_a_pose_is_missing_or_changes_kind() {
        // The player is missing from some frames, and the devices stop being
        // animated for a while: stepping on would not give the rebuild's
        // history there, so the rebuild runs, and the result is its own.
        let dir = TempDir::new("playback-vapor-gaps");
        let recording = synthetic(
            &dir,
            "missing",
            0..=900,
            |_| Vec::new(),
            |t| {
                (!(300..=310).contains(&t) && t != 620).then(|| flown(t, !(450..=470).contains(&t)))
            },
        );
        let playback = vapor_matches_rebuild_at_every_tick(recording);
        assert!(playback.rebuilds[1] > 60, "{}", playback.rebuilds[1]);
    }

    /// Plays a real recording (`TORE_S14_REPLAY=path`) forward, then through
    /// a session of seeks, reverse stretches and strides, holding the
    /// smoke and the vapor to the rebuild at every tick. Run by hand:
    /// `TORE_S14_REPLAY=x.tore-replay cargo test -p tore-app a_recording_from_a_run -- --ignored`.
    #[test]
    #[ignore = "needs a recording named by TORE_S14_REPLAY"]
    fn a_recording_from_a_run_steps_forward_as_it_rebuilds() {
        let Some(path) = std::env::var_os("TORE_S14_REPLAY") else {
            return;
        };
        let recording = Arc::new(Recording::open(path).unwrap());
        let mut playback = Playback::new(Arc::clone(&recording));
        let mut checked = Playback::new(recording);
        let ownship = winged();
        let template = crate::combat_view::render_hash_tests::player();
        let (mut scratch, mut reference) = (template.clone(), template);
        let (first, last) = (playback.first, playback.last);
        let mut vapors = 0;
        let mut puffs = 0;
        for tick in session(first, last) {
            let smoke = playback.smoke(tick).clone();
            puffs += smoke[0].puffs.len() + smoke[1].puffs.len();
            assert_eq!(smoke, smoke_rebuilt(&checked, tick), "smoke at tick {tick}");
            let vapor = playback.vapor(tick, &ownship, &mut scratch);
            vapors += usize::from(vapor.is_some());
            let rebuilt = vapor_rebuilt(&mut checked, tick, &ownship, &mut reference);
            assert_eq!(vapor, rebuilt, "vapor at tick {tick}");
        }
        eprintln!(
            "{first}..={last}: {puffs} puff-ticks, {vapors} vapors; smoke {:?} vapor {:?} (rebuilds, steps)",
            (playback.rebuilds[0], playback.steps_on[0]),
            (playback.rebuilds[1], playback.steps_on[1]),
        );
    }
}
