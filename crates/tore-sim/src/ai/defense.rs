//! Pure missile-defense assessment for actor-owned threat observations.

use crate::{
    attitude::Vector,
    combat::threats::{EvidenceSource, GuidanceClass, ThreatRecord},
};

use super::Experience;

pub const JINK_OFFSET_DEG: f64 = 45.0;
/// Gunfire fallback cadence; missile jinks wait for an established turn.
pub const JINK_LEG_TICKS: u64 = 240;
pub const DIVE_PITCH_DEG: f64 = -5.0;
pub const DIVE_LOOKAHEAD_S: u64 = 5;
pub const DIVE_CLEARANCE_FT: f64 = 1_000.0;
/// John, 2026-10-27: pair starts must occupy distinct two-second episodes.
pub const MIN_BURST_INTERVAL_TICKS: u64 = 240;
/// Fitted policy, specified in docs/spec/ai.md, "Missile defense flown by AI".
pub fn load_fraction(experience: Experience) -> f64 {
    [0.85, 0.90, 1.0, 1.0][skill_index(experience)]
}

fn skill_index(experience: Experience) -> usize {
    match experience {
        Experience::Novice => 0,
        Experience::Average => 1,
        Experience::Experienced => 2,
        Experience::Ace => 3,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefenseOwn {
    pub position: Vector,
    pub velocity: Vector,
    pub heading_deg: f64,
    pub flight_path_pitch_deg: f64,
    pub speed_ft_s: f64,
    pub bank_deg: f64,
    pub usable_turn_rate_deg_s: f64,
    pub usable_pitch_rate_deg_s: f64,
    pub roll_in_time_s: f64,
    pub dive_speed_safe: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Maneuver {
    Jink,
    Notch,
    Drag,
    Beam,
    Break,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSuggestion {
    pub maneuver: Maneuver,
    pub heading_deg: f64,
    pub flight_path_pitch_deg: f64,
    pub reduce_heat: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BurstRequest {
    pub chaff: u8,
    pub flares: u8,
}

impl BurstRequest {
    /// Keep a final pair for another missile's last-ditch break.
    /// Counts exclude devices already queued.
    pub fn with_inventory(self, chaff: u32, flares: u32, reserve_allowed: bool) -> Self {
        let reserve = if reserve_allowed { 0 } else { 2 };
        Self {
            chaff: u32::from(self.chaff).min(chaff.saturating_sub(reserve)) as u8,
            flares: u32::from(self.flares).min(flares.saturating_sub(reserve)) as u8,
        }
    }

    pub const RADAR: Self = Self {
        chaff: 2,
        flares: 0,
    };
    pub const INFRARED: Self = Self {
        chaff: 0,
        flares: 2,
    };
    pub const MIXED: Self = Self {
        chaff: 2,
        flares: 2,
    };
}

/// Write-only explanation of one defense decision. No decision reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefenseDebug {
    pub estimated_threat_time_s: Option<f64>,
    pub estimated_maneuver_time_s: f64,
    /// Skill-dependent last-ditch threshold, seconds.
    pub margin_s: f64,
    /// The current defense phase, including during the reaction delay.
    pub preferred: Maneuver,
    /// Its heading and flight-path pitch, degrees.
    pub heading_deg: f64,
    pub flight_path_pitch_deg: f64,
    /// A shallow descent keeps 1000 ft of clearance for 5 s.
    pub dive_safe: bool,
    /// The selected threat changed this tick.
    pub new_threat: bool,
    /// The reasons behind maneuvering now: only a bearing is known, a
    /// directed radar with no closing estimate, too little time, or stale
    /// evidence. Reaction delay still applies.
    pub bearing_only: bool,
    pub uncertain_directed_radar: bool,
    pub insufficient_time: bool,
    pub stale: bool,
    pub maneuver_now: bool,
    /// Fresh observations confirm sustained opening range.
    pub passed: bool,
    /// Cooldown, encounter budget and second-missile reserve allow a burst.
    pub may_burst: bool,
    /// The evidence calls for devices now.
    pub release_now: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefenseDecision {
    pub threat_id: u32,
    pub reserve_allowed: bool,
    pub motion: Option<MotionSuggestion>,
    pub burst: Option<BurstRequest>,
    pub debug: DefenseDebug,
}

/// Observed history only, retained through report gaps for this encounter.
#[derive(Clone, Debug, Default, PartialEq)]
struct MissileMemory {
    id: u32,
    class: Option<GuidanceClass>,
    sample_tick: Option<u64>,
    range_ft: Option<f64>,
    opening_tick: Option<u64>,
    opening_range_ft: f64,
    passed: bool,
    break_heading: Option<f64>,
    devices: u8,
}

impl MissileMemory {
    fn observe(&mut self, own: DefenseOwn, contact: ThreatRecord) {
        if let Some(class) = contact.guidance_class.or_else(|| {
            matches!(
                contact.source,
                EvidenceSource::ElectronicActive | EvidenceSource::ElectronicSupported
            )
            .then_some(GuidanceClass::Radar)
        }) {
            self.class = Some(class);
        }
        if contact.stale || self.sample_tick == Some(contact.observed_tick) {
            return;
        }
        let Some(position) = contact.position else {
            return;
        };
        // A sample after a report gap starts a fresh confirmation window.
        // The unobserved interval cannot establish that range kept opening.
        if self.sample_tick.is_some_and(|last| {
            contact.observed_tick.saturating_sub(last)
                > 2 * crate::combat::threats::REPORT_INTERVAL_TICKS
        }) {
            self.opening_tick = None;
            self.range_ft = None;
        }
        let range = position
            .iter()
            .zip(own.position)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt();
        if signed_time_to_cpa(own, &contact).is_some_and(|t| t > 0.0)
            || self.range_ft.is_some_and(|last| range < last - 0.25)
        {
            self.opening_tick = None;
            self.passed = false;
        } else if self.range_ft.is_some_and(|last| range > last + 0.25) {
            let since = *self.opening_tick.get_or_insert_with(|| {
                self.opening_range_ft = self.range_ft.unwrap_or(range);
                self.sample_tick.unwrap_or(contact.observed_tick)
            });
            if contact.observed_tick.saturating_sub(since) >= 90
                && range - self.opening_range_ft >= 500.0
                && range >= 1500.0
            {
                self.passed = true;
            }
        }
        self.sample_tick = Some(contact.observed_tick);
        self.range_ft = Some(range);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DefenseState {
    selected_threat: Option<u32>,
    missiles: Vec<MissileMemory>,
    last_contact: Option<ThreatRecord>,
    first_device_threat: Option<u32>,
    observed_tick: u64,
    phase: Option<Maneuver>,
    extension_finished: bool,
    passed: bool,
    beam_side: Option<f64>,
    break_heading_deg: Option<f64>,
    devices_requested: u8,
    /// Last fresh selected observation, used for the short motion grace.
    last_threat_tick: Option<u64>,
    last_encounter_tick: Option<u64>,
    jink_base_heading_deg: Option<f64>,
    jink_heading_deg: Option<f64>,
    jink_started_tick: u64,
    last_burst_tick: Option<u64>,
}

impl DefenseState {
    /// Combat confirmed this store is inert. Absence alone never calls this.
    pub fn end_threat(&mut self, id: u32) {
        self.missiles.retain(|missile| missile.id != id);
        if self
            .last_contact
            .is_some_and(|contact| contact.missile_id == id)
        {
            self.last_contact = None;
        }
        if self.selected_threat == Some(id) {
            self.selected_threat = None;
            self.phase = None;
            self.beam_side = None;
            self.break_heading_deg = None;
            self.passed = false;
            self.clear_threat();
        }
        // Encounter budget, reaction progress and release cooldown still apply
        // to other stores, including the reserved pair for a second threat.
    }

    /// No observation supports motion now. Keep the encounter memory until
    /// its quiet timeout, including class, break heading, reaction and budget.
    pub fn clear_threat(&mut self) {
        self.jink_base_heading_deg = None;
        self.jink_heading_deg = None;
    }

    /// A new encounter after a longer quiet spell. The aircraft's release
    /// cooldown carries over; missile-specific memory starts afresh.
    fn start_encounter(&mut self, tick: u64) {
        *self = Self {
            observed_tick: tick,
            last_encounter_tick: Some(tick),
            last_burst_tick: self.last_burst_tick,
            ..Self::default()
        };
    }
}

/// Fitted: a lost observation supports committed motion for two seconds,
/// the threat picture's own lost-contact grace. Encounter memory lasts longer.
pub const THREAT_MEMORY_TICKS: u64 = crate::combat::threats::LOST_GRACE_TICKS;
/// Fitted quiet window for classification, budget and phase memory: no
/// fresh observation of a missile aimed at this aircraft for this long.
pub const ENCOUNTER_MEMORY_TICKS: u64 = 30 * 120;

pub fn decide<F>(
    tick: u64,
    experience: Experience,
    own: DefenseOwn,
    contacts: &[ThreatRecord],
    state: &mut DefenseState,
    mut ground_elevation_ft: F,
) -> Option<DefenseDecision>
where
    F: FnMut(Vector) -> f64,
{
    if state
        .last_encounter_tick
        .is_none_or(|last| tick.saturating_sub(last) > ENCOUNTER_MEMORY_TICKS)
    {
        state.start_encounter(tick);
    }
    // Only a missile aimed at this aircraft keeps the encounter open. Its own
    // missiles in flight and missiles seen going elsewhere are not threats.
    if contacts.iter().any(|c| !c.stale && c.targeting_receiver) {
        state.last_encounter_tick = Some(tick);
    }
    for contact in contacts {
        let index = state
            .missiles
            .iter()
            .position(|m| m.id == contact.missile_id)
            .unwrap_or_else(|| {
                state.missiles.push(MissileMemory {
                    id: contact.missile_id,
                    ..MissileMemory::default()
                });
                state.missiles.len() - 1
            });
        state.missiles[index].observe(own, *contact);
    }
    let passed_ids: Vec<_> = state
        .missiles
        .iter()
        .filter(|m| m.passed)
        .map(|m| m.id)
        .collect();
    let selected = select_threat(own, contacts, state.selected_threat, &passed_ids).copied();
    if let Some(contact) = selected
        && !contact.stale
    {
        if state.last_threat_tick.is_none() {
            state.observed_tick = tick;
        }
        state.last_threat_tick = Some(tick);
        state.last_contact = Some(contact);
    }
    let Some(selected) = selected.or_else(|| {
        state
            .last_contact
            .filter(|_| {
                state
                    .last_threat_tick
                    .is_some_and(|last| tick.saturating_sub(last) <= THREAT_MEMORY_TICKS)
            })
            .map(|mut contact| {
                contact.stale = true;
                contact
            })
    }) else {
        state.clear_threat();
        return None;
    };
    let index = state
        .missiles
        .iter()
        .position(|m| m.id == selected.missile_id)
        .unwrap();
    let changed = state.selected_threat != Some(selected.missile_id);
    if changed {
        state.phase = state.missiles[index].break_heading.map(|_| Maneuver::Break);
        state.beam_side = None;
        state.break_heading_deg = state.missiles[index].break_heading;
    }
    state.selected_threat = Some(selected.missile_id);
    let skill = skill_index(experience);
    let time = time_to_cpa(own, &selected);
    let passed = state.missiles[index].passed;
    state.passed = passed;
    if passed {
        state.phase = None;
        state.extension_finished = true;
    }
    let known_class = state.missiles[index].class;
    let radar = match known_class {
        Some(GuidanceClass::Radar) => true,
        Some(GuidanceClass::Infrared | GuidanceClass::Passive) => false,
        None => selected.source != EvidenceSource::Visual,
    };
    let missile_world = wrap(own.heading_deg + selected.bearing_deg);
    let reference = if radar {
        selected
            .radar_bearing_deg
            .map_or(missile_world, |b| wrap(own.heading_deg + b))
    } else {
        missile_world
    };
    let break_time = [1.5, 2.5, 3.5, 4.5][skill];
    let preferred = if !passed
        && (state.phase == Some(Maneuver::Break) || time.is_some_and(|t| t <= break_time))
    {
        Maneuver::Break
    } else if !state.extension_finished
        && (state.phase.is_none() && time.is_some_and(|t| t > if radar { 18.0 } else { 14.0 })
            || state.phase == Some(Maneuver::Drag)
                && time.is_some_and(|t| t > if radar { 14.0 } else { 10.0 }))
    {
        Maneuver::Drag
    } else if known_class == Some(GuidanceClass::Passive) {
        Maneuver::Jink
    } else if radar {
        Maneuver::Notch
    } else {
        Maneuver::Beam
    };
    if !passed {
        state.phase = Some(preferred);
        state.extension_finished |= preferred != Maneuver::Drag;
    }
    // Elevation depends on horizontal position. Reuse only identical samples
    // within this decision, never stale heights from a previous tick.
    let mut samples = Vec::with_capacity(76);
    let mut terrain = |position: Vector| {
        let key = [position[0], position[2]];
        if let Some((_, height)) = samples.iter().find(|(point, _)| *point == key) {
            return *height;
        }
        let height = ground_elevation_ft(position);
        samples.push((key, height));
        height
    };
    let heading = if passed {
        own.heading_deg
    } else {
        match preferred {
            Maneuver::Drag => wrap(missile_world + 180.0),
            Maneuver::Break => {
                let heading = break_heading(own, missile_world, state.beam_side);
                let heading = *state.break_heading_deg.get_or_insert(heading);
                state.missiles[index].break_heading = Some(heading);
                heading
            }
            Maneuver::Notch | Maneuver::Beam => {
                beam_heading(own, reference, contacts, state, &mut terrain)
            }
            Maneuver::Jink => {
                jink_heading(tick, own, experience, selected.missile_id, changed, state)
            }
        }
    };
    let dive_safe = !passed && dive_is_safe(own, heading, &mut terrain);
    let pitch = if dive_safe { DIVE_PITCH_DEG } else { 0.0 };
    let maneuver_time = maneuver_time_s(own, heading, pitch);
    let margin = break_time;
    let insufficient_time = time.is_some_and(|value| value <= maneuver_time);
    let bearing_only = selected.position.is_none() || selected.velocity.is_none();
    let uncertain_directed_radar = time.is_none() && selected.targeting_receiver && radar;
    let maneuver_now =
        !passed && tick.saturating_sub(state.observed_tick) >= [180, 90, 30, 0][skill];
    let last_ditch = preferred == Maneuver::Break;
    let interval = if last_ditch {
        MIN_BURST_INTERVAL_TICKS
    } else {
        [720, 480, 360, 360][skill]
    };
    let reserve_allowed = last_ditch
        && state
            .first_device_threat
            .is_some_and(|id| id != selected.missile_id);
    let reserve_now = reserve_allowed && state.missiles[index].devices == 0;
    let may_burst = state.last_burst_tick.is_none_or(|last| {
        let elapsed = tick.saturating_sub(last);
        elapsed >= MIN_BURST_INTERVAL_TICKS && (reserve_now || elapsed >= interval)
    }) && state.devices_requested < if reserve_allowed { 8 } else { 6 }
        && state.missiles[index].devices < 6;
    let release_now = maneuver_now
        && (last_ditch
            || radar
                && preferred == Maneuver::Notch
                && (notch_started(own, heading) || time.is_some_and(|t| t <= 6.0))
            || !radar && time.is_some_and(|t| t <= [2.0, 3.0, 5.0, 6.0][skill]));
    let burst = (release_now && may_burst && !selected.stale).then_some(if radar {
        BurstRequest::RADAR
    } else {
        BurstRequest::INFRARED
    });
    if burst.is_some() {
        state.last_burst_tick = Some(tick);
        state.devices_requested += 2;
        state.missiles[index].devices += 2;
        state.first_device_threat.get_or_insert(selected.missile_id);
    }

    Some(DefenseDecision {
        threat_id: selected.missile_id,
        reserve_allowed,
        motion: maneuver_now.then_some(MotionSuggestion {
            maneuver: preferred,
            heading_deg: heading,
            flight_path_pitch_deg: pitch,
            reduce_heat: !radar
                && (known_class == Some(GuidanceClass::Infrared)
                    || selected.source == EvidenceSource::Visual)
                && preferred != Maneuver::Drag,
        }),
        burst,
        debug: DefenseDebug {
            estimated_threat_time_s: time,
            estimated_maneuver_time_s: maneuver_time,
            margin_s: margin,
            preferred,
            heading_deg: heading,
            flight_path_pitch_deg: pitch,
            dive_safe,
            new_threat: changed,
            bearing_only,
            uncertain_directed_radar,
            insufficient_time,
            stale: selected.stale,
            maneuver_now,
            passed,
            may_burst,
            release_now,
        },
    })
}

fn select_threat<'a>(
    own: DefenseOwn,
    contacts: &'a [ThreatRecord],
    current: Option<u32>,
    passed: &[u32],
) -> Option<&'a ThreatRecord> {
    contacts
        .iter()
        .filter(|contact| {
            contact.targeting_receiver || (contact.stale && contact.was_targeting_receiver)
        })
        .min_by(|a, b| {
            let priority = |contact: &ThreatRecord| {
                time_to_cpa(own, contact).unwrap_or_else(|| {
                    if passed.contains(&contact.missile_id) {
                        f64::INFINITY
                    } else if contact.position.is_none() || contact.velocity.is_none() {
                        -1.0
                    } else {
                        f64::INFINITY
                    }
                })
            };
            priority(a).total_cmp(&priority(b)).then_with(|| {
                match (current == Some(a.missile_id), current == Some(b.missile_id)) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.missile_id.cmp(&b.missile_id),
                }
            })
        })
}

fn time_to_cpa(own: DefenseOwn, contact: &ThreatRecord) -> Option<f64> {
    signed_time_to_cpa(own, contact).filter(|t| *t > 0.0)
}

fn signed_time_to_cpa(own: DefenseOwn, contact: &ThreatRecord) -> Option<f64> {
    let position = contact.position?;
    let velocity = contact.velocity?;
    let relative_position = std::array::from_fn::<_, 3, _>(|i| position[i] - own.position[i]);
    let relative_velocity = std::array::from_fn::<_, 3, _>(|i| velocity[i] - own.velocity[i]);
    let speed_sq = relative_velocity
        .iter()
        .map(|value| value * value)
        .sum::<f64>();
    if speed_sq <= f64::EPSILON {
        return None;
    }
    let dot = relative_position
        .iter()
        .zip(relative_velocity)
        .map(|(a, b)| a * b)
        .sum::<f64>();
    Some(-dot / speed_sq)
}

fn notch_started(own: DefenseOwn, heading: f64) -> bool {
    let turn = angle_delta(own.heading_deg, heading);
    turn.abs() <= 20.0 || own.bank_deg.abs() >= 5.0 && own.bank_deg.signum() == turn.signum()
}

/// Turn toward the missile's side, but always establish a hard crossing
/// turn. Merely pointing at an oncoming missile would cancel the break.
fn break_heading(own: DefenseOwn, missile_world: f64, beam_side: Option<f64>) -> f64 {
    let bearing = angle_delta(own.heading_deg, missile_world);
    let side = if bearing.abs() >= 15.0 {
        bearing.signum()
    } else if own.bank_deg.abs() > 5.0 {
        own.bank_deg.signum()
    } else {
        beam_side.unwrap_or(1.0)
    };
    wrap(own.heading_deg + side * bearing.abs().max(90.0))
}

fn beam_heading<F>(
    own: DefenseOwn,
    reference: f64,
    contacts: &[ThreatRecord],
    state: &mut DefenseState,
    terrain: &mut F,
) -> f64
where
    F: FnMut(Vector) -> f64,
{
    let left = wrap(reference - 90.0);
    let right = wrap(reference + 90.0);
    if state.beam_side.is_none() {
        let left_safe = turn_clearance(own, left, 0.0, terrain);
        let right_safe = turn_clearance(own, right, 0.0, terrain);
        let cost = |heading: f64, safe: bool| {
            angle_delta(own.heading_deg, heading).abs()
                + if safe { 0.0 } else { 360.0 }
                + contacts
                    .iter()
                    .filter(|c| c.targeting_receiver && !c.stale)
                    .map(|c| {
                        30.0 * angle_delta(heading, own.heading_deg + c.bearing_deg)
                            .to_radians()
                            .cos()
                            .max(0.0)
                    })
                    .fold(0.0_f64, f64::max)
        };
        state.beam_side = Some(if cost(left, left_safe) <= cost(right, right_safe) {
            -1.0
        } else {
            1.0
        });
    } else {
        let side = state.beam_side.unwrap_or(1.0);
        let (committed, opposite) = if side < 0.0 {
            (left, right)
        } else {
            (right, left)
        };
        if !turn_clearance(own, committed, 0.0, terrain)
            && turn_clearance(own, opposite, 0.0, terrain)
        {
            state.beam_side = Some(-side);
        }
    }
    wrap(reference + 90.0 * state.beam_side.unwrap_or(1.0))
}

fn jink_heading(
    tick: u64,
    own: DefenseOwn,
    experience: Experience,
    missile_id: u32,
    reset: bool,
    state: &mut DefenseState,
) -> f64 {
    let heading = own.heading_deg;
    let leg_s = (90.0 / (own.usable_turn_rate_deg_s * load_fraction(experience)).max(1.0)
        + own.roll_in_time_s
        + [3.0, 2.5, 2.0, 1.5][skill_index(experience)])
    .max(4.0);
    if reset || state.jink_heading_deg.is_none() {
        let side = if missile_id.is_multiple_of(2) {
            1.0
        } else {
            -1.0
        };
        state.jink_base_heading_deg = Some(heading);
        state.jink_heading_deg = Some(wrap(heading + side * JINK_OFFSET_DEG));
        state.jink_started_tick = tick;
    } else if tick.saturating_sub(state.jink_started_tick) as f64 >= leg_s * 120.0
        && angle_delta(heading, state.jink_heading_deg.unwrap_or(heading)).abs() <= 15.0
    {
        let previous = state.jink_heading_deg.unwrap_or(heading);
        let base = state.jink_base_heading_deg.unwrap_or(heading);
        state.jink_heading_deg = Some(wrap(2.0 * base - previous));
        state.jink_started_tick = tick;
    }
    state.jink_heading_deg.unwrap_or(heading)
}

fn maneuver_time_s(own: DefenseOwn, heading: f64, pitch: f64) -> f64 {
    let heading_time =
        angle_delta(own.heading_deg, heading).abs() / own.usable_turn_rate_deg_s.max(f64::EPSILON);
    let pitch_time =
        (pitch - own.flight_path_pitch_deg).abs() / own.usable_pitch_rate_deg_s.max(f64::EPSILON);
    own.roll_in_time_s.max(0.0) + heading_time.max(pitch_time)
}

fn dive_is_safe<F>(own: DefenseOwn, heading: f64, terrain: &mut F) -> bool
where
    F: FnMut(Vector) -> f64,
{
    if !own.dive_speed_safe || own.speed_ft_s <= 0.0 {
        return false;
    }
    turn_clearance(own, heading, DIVE_PITCH_DEG, terrain)
}

fn turn_clearance<F>(own: DefenseOwn, heading: f64, pitch_deg: f64, terrain: &mut F) -> bool
where
    F: FnMut(Vector) -> f64,
{
    let turn = angle_delta(own.heading_deg, heading);
    let pitch = pitch_deg.to_radians();
    (0..=4).all(|part| {
        let heading = (own.heading_deg + turn * part as f64 / 4.0).to_radians();
        (0..=DIVE_LOOKAHEAD_S).all(|second| {
            let distance = own.speed_ft_s * second as f64;
            let position = [
                own.position[0] + heading.sin() * pitch.cos() * distance,
                own.position[1] + pitch.sin() * distance,
                own.position[2] + heading.cos() * pitch.cos() * distance,
            ];
            position[1] - terrain(position) >= DIVE_CLEARANCE_FT
        })
    })
}

fn gunfire_dive_is_safe<F>(own: DefenseOwn, heading: f64, terrain: &mut F) -> bool
where
    F: FnMut(Vector) -> f64,
{
    if !own.dive_speed_safe || own.speed_ft_s <= 0.0 {
        return false;
    }
    let heading = heading.to_radians();
    let pitch = (-20.0_f64).to_radians();
    (1..=DIVE_LOOKAHEAD_S).all(|second| {
        let distance = own.speed_ft_s * second as f64;
        let position = [
            own.position[0] + heading.sin() * pitch.cos() * distance,
            own.position[1] + pitch.sin() * distance,
            own.position[2] + heading.cos() * pitch.cos() * distance,
        ];
        position[1] - terrain(position) >= DIVE_CLEARANCE_FT
    })
}

/// Preserve the established gunfire response, separate from missile defense.
pub fn safe_jink(
    own: DefenseOwn,
    heading: f64,
    mut terrain: impl FnMut(Vector) -> f64,
) -> MotionSuggestion {
    MotionSuggestion {
        maneuver: Maneuver::Jink,
        reduce_heat: false,
        heading_deg: heading,
        flight_path_pitch_deg: if gunfire_dive_is_safe(own, heading, &mut terrain) {
            -20.0
        } else {
            own.flight_path_pitch_deg.max(0.)
        },
    }
}

fn angle_delta(from: f64, to: f64) -> f64 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

fn wrap(value: f64) -> f64 {
    value.rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> DefenseOwn {
        DefenseOwn {
            position: [0.0, 5_000.0, 0.0],
            velocity: [0.0; 3],
            heading_deg: 0.0,
            flight_path_pitch_deg: 0.0,
            speed_ft_s: 500.0,
            bank_deg: 0.0,
            usable_turn_rate_deg_s: 30.0,
            usable_pitch_rate_deg_s: 20.0,
            roll_in_time_s: 0.5,
            dive_speed_safe: true,
        }
    }
    fn threat(source: EvidenceSource, class: Option<GuidanceClass>) -> ThreatRecord {
        ThreatRecord {
            missile_id: 2,
            source,
            observed_tick: 0,
            bearing_deg: 0.0,
            position: Some([0.0, 5_000.0, 5_000.0]),
            velocity: Some([0.0, 0.0, -1_000.0]),
            guidance_class: class,
            targeting_receiver: true,
            was_targeting_receiver: true,
            stale: false,
            radar_bearing_deg: None,
        }
    }

    fn assess(
        tick: u64,
        skill: Experience,
        own: DefenseOwn,
        mut contact: ThreatRecord,
        state: &mut DefenseState,
    ) -> DefenseDecision {
        contact.observed_tick = tick;
        decide(tick, skill, own, &[contact], state, |_| 0.0).unwrap()
    }

    #[test]
    fn a_confirmed_passed_record_cannot_hide_a_second_closing_missile() {
        let mut state = DefenseState::default();
        let mut first = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        first.position = Some([0.0, 5000.0, 1000.0]);
        assess(0, Experience::Ace, own(), first, &mut state);
        first.velocity = Some([0.0, 0.0, 2000.0]);
        for tick in [30, 60, 90, 120] {
            first.position = Some([0.0, 5000.0, 1000.0 + tick as f64 * 10.0]);
            assess(tick, Experience::Ace, own(), first, &mut state);
        }
        first.velocity = None;
        first.observed_tick = 120;
        let second = ThreatRecord {
            missile_id: 3,
            position: Some([0.0, 5000.0, 2000.0]),
            ..threat(EvidenceSource::Visual, None)
        };
        for tick in 121..200 {
            let d = decide(
                tick,
                Experience::Ace,
                own(),
                &[first, second],
                &mut state,
                |_| 0.0,
            )
            .unwrap();
            assert_eq!(d.threat_id, second.missile_id);
            assert!(!d.debug.passed && d.motion.is_some());
            if tick == 160 {
                state =
                    crate::checkpoint::round_trip(&state, &crate::checkpoint::Models::default())
                        .unwrap();
            }
        }
        let quiet = assess(200, Experience::Ace, own(), first, &mut state);
        assert!(quiet.debug.passed && quiet.motion.is_none() && quiet.burst.is_none());
    }

    #[test]
    fn missing_or_briefly_opening_estimates_keep_the_break_and_heading() {
        let mut contact = threat(
            EvidenceSource::ElectronicSupported,
            Some(GuidanceClass::Radar),
        );
        contact.position = Some([0.0, 5000.0, 1000.0]);
        let mut state = DefenseState::default();
        let heading = assess(0, Experience::Ace, own(), contact, &mut state)
            .motion
            .unwrap()
            .heading_deg;
        for tick in 1..=300 {
            contact.velocity = if tick % 60 < 30 {
                None
            } else {
                Some([0.0, 0.0, 2000.0])
            };
            contact.position = Some([0.0, 5000.0, 1000.0 + (tick % 60) as f64 * 2.0]);
            let d = assess(tick, Experience::Ace, own(), contact, &mut state);
            assert!(!d.debug.passed);
            let motion = d.motion.unwrap();
            assert_eq!(motion.maneuver, Maneuver::Break);
            assert_eq!(motion.heading_deg, heading);
        }
    }

    #[test]
    fn classification_and_break_survive_a_gap_longer_than_two_seconds() {
        let mut contact = threat(
            EvidenceSource::ElectronicSupported,
            Some(GuidanceClass::Radar),
        );
        contact.position = Some([0.0, 5000.0, 1000.0]);
        let mut state = DefenseState::default();
        let first = assess(0, Experience::Ace, own(), contact, &mut state);
        for tick in 1..600 {
            decide(tick, Experience::Ace, own(), &[], &mut state, |_| 0.0);
        }
        contact.source = EvidenceSource::Visual;
        contact.guidance_class = None;
        contact.velocity = None;
        let resumed = assess(600, Experience::Ace, own(), contact, &mut state);
        assert_eq!(resumed.burst, Some(BurstRequest::RADAR));
        assert_eq!(
            resumed.motion.unwrap().heading_deg,
            first.motion.unwrap().heading_deg
        );
        assert!(!resumed.motion.unwrap().reduce_heat);
    }

    #[test]
    fn another_missile_keeps_the_final_pair_within_the_encounter_budget() {
        let mut contact = threat(
            EvidenceSource::ElectronicSupported,
            Some(GuidanceClass::Radar),
        );
        contact.position = Some([0.0, 5000.0, 1000.0]);
        let mut state = DefenseState::default();
        let mut requests = 0;
        for tick in 0..600 {
            requests += assess(tick, Experience::Ace, own(), contact, &mut state)
                .burst
                .map_or(0, |b| b.chaff);
        }
        assert_eq!(requests, 6);
        contact.missile_id += 1;
        let waiting = assess(600, Experience::Ace, own(), contact, &mut state);
        assert!(waiting.reserve_allowed && waiting.burst.is_none());
        let second = assess(720, Experience::Ace, own(), contact, &mut state);
        assert!(second.reserve_allowed);
        assert_eq!(
            second
                .burst
                .unwrap()
                .with_inventory(2, 0, second.reserve_allowed)
                .chaff,
            2
        );
        for tick in 721..1200 {
            assert!(
                assess(tick, Experience::Ace, own(), contact, &mut state)
                    .burst
                    .is_none()
            );
        }
        assert_eq!(state.devices_requested, 8);
    }

    #[test]
    fn reaction_starts_at_the_first_incoming_warning_not_mission_start() {
        let mut state = DefenseState::default();
        for tick in 0..600 {
            assert!(decide(tick, Experience::Novice, own(), &[], &mut state, |_| 0.0).is_none());
        }
        let contact = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        assert!(
            assess(600, Experience::Novice, own(), contact, &mut state)
                .motion
                .is_none()
        );
        assert!(
            assess(779, Experience::Novice, own(), contact, &mut state)
                .motion
                .is_none()
        );
        assert!(
            assess(780, Experience::Novice, own(), contact, &mut state)
                .motion
                .is_some()
        );
    }

    #[test]
    fn mild_skill_load_follows_the_leads_limits() {
        assert_eq!(Experience::ALL.map(load_fraction), [0.85, 0.90, 1.0, 1.0]);
    }

    #[test]
    fn every_skill_chaffs_when_the_turn_starts_or_inside_six_seconds() {
        for skill in Experience::ALL {
            for (time, bank, expected) in [
                (10.0, 0.0, false),
                (10.0, -6.0, true),
                (10.0, 6.0, false),
                (6.0, 0.0, true),
            ] {
                let mut contact =
                    threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
                contact.position = Some([0.0, 5000.0, time * 1000.0]);
                let mut state = DefenseState::default();
                // Finish the reaction with level wings and a distant threat.
                let mut distant = contact;
                distant.position = Some([0.0, 5000.0, 10000.0]);
                for tick in 0..=180 {
                    assess(tick, skill, own(), distant, &mut state);
                }
                let mut flying = own();
                flying.bank_deg = bank;
                let answer = assess(181, skill, flying, contact, &mut state);
                assert_eq!(
                    answer.burst == Some(BurstRequest::RADAR),
                    expected,
                    "{skill:?} time={time} bank={bank}"
                );
            }
        }
    }

    #[test]
    fn passage_needs_fresh_opening_ranges_over_the_confirmation_window() {
        let mut contact = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        contact.position = Some([0.0, 5000.0, 1000.0]);
        let mut state = DefenseState::default();
        assess(0, Experience::Ace, own(), contact, &mut state);
        contact.velocity = Some([0.0, 0.0, 2000.0]);
        contact.position = Some([0.0, 5000.0, 1300.0]);
        contact.observed_tick = 30;
        for tick in 30..120 {
            let d = decide(tick, Experience::Ace, own(), &[contact], &mut state, |_| {
                0.0
            })
            .unwrap();
            assert!(
                !d.debug.passed,
                "re-reading one frozen report is not opening evidence"
            );
        }
        contact.position = Some([0.0, 5000.0, 2300.0]);
        let d = assess(120, Experience::Ace, own(), contact, &mut state);
        assert!(
            !d.debug.passed,
            "a gap cannot count as observed opening range"
        );
        for tick in [150, 180] {
            contact.position = Some([0.0, 5000.0, 2300.0 + (tick - 120) as f64 * 10.0]);
            assert!(
                !assess(tick, Experience::Ace, own(), contact, &mut state)
                    .debug
                    .passed
            );
        }
        contact.position = Some([0.0, 5000.0, 3200.0]);
        let d = assess(210, Experience::Ace, own(), contact, &mut state);
        assert!(d.debug.passed && d.motion.is_none() && d.burst.is_none());
    }

    #[test]
    fn changing_missiles_or_a_brief_gap_cannot_restart_a_completed_drag() {
        let mut contact = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        contact.position = Some([0.0, 5000.0, 10000.0]);
        let mut state = DefenseState::default();
        assert_eq!(
            assess(0, Experience::Ace, own(), contact, &mut state)
                .motion
                .unwrap()
                .maneuver,
            Maneuver::Notch
        );
        contact.missile_id += 1;
        contact.position = Some([0.0, 5000.0, 30000.0]);
        assert_eq!(
            assess(1, Experience::Ace, own(), contact, &mut state)
                .motion
                .unwrap()
                .maneuver,
            Maneuver::Notch
        );
        let mut restored =
            crate::checkpoint::round_trip(&state, &crate::checkpoint::Models::default()).unwrap();
        decide(2, Experience::Ace, own(), &[], &mut restored, |_| 0.0);
        assert_eq!(
            assess(240, Experience::Ace, own(), contact, &mut restored)
                .motion
                .unwrap()
                .maneuver,
            Maneuver::Notch
        );
        assert_eq!(
            assess(
                241 + ENCOUNTER_MEMORY_TICKS,
                Experience::Ace,
                own(),
                contact,
                &mut restored
            )
            .motion
            .unwrap()
            .maneuver,
            Maneuver::Drag
        );
    }

    #[test]
    fn terrain_queries_are_reused_within_a_tick_without_caching_old_heights() {
        let contact = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        let mut contact = ThreatRecord {
            position: Some([0.0, 5000.0, 10000.0]),
            ..contact
        };
        let mut old_calls = 0;
        let mut terrain = |_| {
            old_calls += 1;
            0.0
        };
        turn_clearance(own(), 270.0, 0.0, &mut terrain);
        turn_clearance(own(), 90.0, 0.0, &mut terrain);
        dive_is_safe(own(), 270.0, &mut terrain);
        assert_eq!(old_calls, 90);
        let mut state = DefenseState::default();
        let mut counts = Vec::new();
        for tick in 0..2 {
            let mut calls = 0;
            let answer = decide(tick, Experience::Ace, own(), &[contact], &mut state, |_| {
                calls += 1;
                0.0
            })
            .unwrap();
            assert!(answer.debug.dive_safe);
            counts.push(calls);
        }
        assert_eq!(counts, [71, 51]);
        let mut aligned = own();
        aligned.heading_deg = 270.0;
        contact.radar_bearing_deg = Some(90.0);
        let mut calls = 0;
        let answer = decide(2, Experience::Ace, aligned, &[contact], &mut state, |_| {
            calls += 1;
            0.0
        })
        .unwrap();
        assert!(answer.debug.dive_safe);
        assert_eq!(calls, 11);
        // Changed terrain at the same position is queried afresh next tick.
        let blocked = decide(3, Experience::Ace, aligned, &[contact], &mut state, |_| {
            5000.0
        })
        .unwrap();
        assert!(!blocked.debug.dive_safe);
        assert_eq!(blocked.motion.unwrap().flight_path_pitch_deg, 0.0);
    }

    #[test]
    fn last_ditch_crosses_a_head_on_missile_instead_of_straightening_at_it() {
        let mut flying = own();
        assert_eq!(break_heading(flying, 0.0, None), 90.0);
        assert_eq!(break_heading(flying, 0.0, Some(-1.0)), 270.0);
        flying.bank_deg = -40.0;
        assert_eq!(break_heading(flying, 10.0, Some(1.0)), 270.0);
        assert_eq!(break_heading(flying, 45.0, None), 90.0);
        assert_eq!(break_heading(flying, 135.0, None), 135.0);
        assert_eq!(break_heading(flying, 225.0, None), 225.0);
    }

    #[test]
    fn classification_is_kept_while_observed_even_before_a_threat_turns_incoming() {
        let mut state = DefenseState::default();
        let mut contact = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        contact.targeting_receiver = false;
        assert!(decide(0, Experience::Ace, own(), &[contact], &mut state, |_| 0.0).is_none());
        contact.source = EvidenceSource::Visual;
        contact.guidance_class = None;
        contact.targeting_receiver = true;
        contact.position = Some([0.0, 5_000.0, 1000.0]);
        assert_eq!(
            assess(1, Experience::Ace, own(), contact, &mut state).burst,
            Some(BurstRequest::RADAR)
        );
        decide(2, Experience::Ace, own(), &[], &mut state, |_| 0.0);
        assert!(
            state
                .missiles
                .iter()
                .any(|m| m.class == Some(GuidanceClass::Radar))
        );
    }

    #[test]
    fn electronic_classification_survives_visual_handoff_and_priority_switches() {
        let mut state = DefenseState::default();
        let mut radar = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        radar.position = Some([0.0, 5_000.0, 3_000.0]);
        let mut other = radar;
        other.missile_id = 3;
        other.position = Some([0.0, 5_000.0, 2_000.0]);
        decide(
            0,
            Experience::Ace,
            own(),
            &[radar, other],
            &mut state,
            |_| 0.0,
        );
        radar.source = EvidenceSource::Visual;
        radar.guidance_class = None;
        other.targeting_receiver = false;
        let answer = decide(
            120,
            Experience::Ace,
            own(),
            &[radar, other],
            &mut state,
            |_| 0.0,
        )
        .unwrap();
        assert_eq!(answer.threat_id, radar.missile_id);
        assert_eq!(answer.burst, None);
        let later = assess(240, Experience::Ace, own(), radar, &mut state);
        assert_eq!(later.burst, Some(BurstRequest::RADAR));
        assert!(!answer.motion.unwrap().reduce_heat);
    }

    #[test]
    fn routine_bursts_keep_a_final_pair_and_never_substitute_the_other_class() {
        assert_eq!(
            BurstRequest::RADAR.with_inventory(2, 30, false),
            BurstRequest {
                chaff: 0,
                flares: 0
            }
        );
        assert_eq!(
            BurstRequest::RADAR.with_inventory(3, 30, false),
            BurstRequest {
                chaff: 1,
                flares: 0
            }
        );
        assert_eq!(
            BurstRequest::RADAR.with_inventory(4, 30, false),
            BurstRequest::RADAR
        );
        assert_eq!(
            BurstRequest::INFRARED.with_inventory(30, 3, false),
            BurstRequest {
                chaff: 0,
                flares: 1
            }
        );
    }

    #[test]
    fn switching_priority_does_not_restart_a_pilots_reaction() {
        let mut state = DefenseState::default();
        let mut contact = threat(EvidenceSource::Visual, Some(GuidanceClass::Infrared));
        assert!(
            assess(0, Experience::Novice, own(), contact, &mut state)
                .motion
                .is_none()
        );
        for tick in 1..=180 {
            contact.missile_id = if tick % 2 == 0 { 2 } else { 3 };
            let decision = assess(tick, Experience::Novice, own(), contact, &mut state);
            assert_eq!(decision.motion.is_some(), tick == 180);
        }
    }

    #[test]
    fn ending_one_store_keeps_reaction_progress_and_encounter_budget() {
        let first = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        let second = ThreatRecord {
            missile_id: 3,
            ..first
        };
        let mut state = DefenseState::default();
        assert!(
            assess(0, Experience::Novice, own(), first, &mut state)
                .motion
                .is_none()
        );
        state.end_threat(first.missile_id);
        assert!(decide(1, Experience::Novice, own(), &[], &mut state, |_| 0.).is_none());
        let decision = assess(180, Experience::Novice, own(), second, &mut state);
        assert!(
            decision.motion.is_some(),
            "ending a store cannot restart the reaction"
        );
        assert!(decision.burst.is_some());
        let spent = state.devices_requested;
        let cooldown = state.last_burst_tick;
        state.end_threat(second.missile_id);
        let restored: DefenseState =
            crate::checkpoint::round_trip(&state, &crate::checkpoint::Models::default()).unwrap();
        assert_eq!(restored, state);
        assert_eq!(state.devices_requested, spent);
        assert_eq!(state.last_burst_tick, cooldown);
        assert!(decide(181, Experience::Novice, own(), &[], &mut state, |_| 0.).is_none());
    }

    #[test]
    fn committed_defense_checkpoint_preserves_future_decisions() {
        let mut contact = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        contact.position = Some([0.0, 5_000.0, 10_000.0]);
        contact.radar_bearing_deg = Some(90.0);
        let mut original = DefenseState::default();
        assess(0, Experience::Ace, own(), contact, &mut original);
        let mut restored =
            crate::checkpoint::round_trip(&original, &crate::checkpoint::Models::default())
                .unwrap();
        assert_eq!(original, restored);
        for tick in 1..600 {
            if tick == 400 {
                contact.position = Some([0.0, 5_000.0, 3_000.0]);
            }
            assert_eq!(
                assess(tick, Experience::Ace, own(), contact, &mut original),
                assess(tick, Experience::Ace, own(), contact, &mut restored)
            );
        }
    }

    #[test]
    fn radar_sequence_drags_notches_and_commits_to_break() {
        let mut state = DefenseState::default();
        let mut contact = threat(
            EvidenceSource::ElectronicSupported,
            Some(GuidanceClass::Radar),
        );
        contact.radar_bearing_deg = Some(30.0);
        contact.position = Some([0.0, 5_000.0, 30_000.0]);
        let drag = assess(0, Experience::Ace, own(), contact, &mut state);
        assert_eq!(drag.motion.unwrap().maneuver, Maneuver::Drag);
        assert_eq!(drag.motion.unwrap().heading_deg, 180.0);
        assert_eq!(drag.burst, None);
        contact.position = Some([0.0, 5_000.0, 13_000.0]);
        let beam = assess(120, Experience::Ace, own(), contact, &mut state);
        assert_eq!(beam.motion.unwrap().maneuver, Maneuver::Notch);
        assert_eq!(beam.motion.unwrap().heading_deg, 300.0);
        assert_eq!(beam.burst, None);
        let mut aligned = own();
        aligned.heading_deg = 300.0;
        contact.bearing_deg = 60.0;
        contact.radar_bearing_deg = Some(90.0);
        assert_eq!(
            assess(121, Experience::Ace, aligned, contact, &mut state).burst,
            Some(BurstRequest::RADAR)
        );
        contact.bearing_deg = 0.0;
        contact.position = Some([0.0, 5_000.0, 4_000.0]);
        let last = assess(300, Experience::Ace, own(), contact, &mut state);
        assert_eq!(last.motion.unwrap().maneuver, Maneuver::Break);
        assert_eq!(last.motion.unwrap().heading_deg, 270.0);
        assert_eq!(last.burst, None);
        assert_eq!(
            assess(361, Experience::Ace, own(), contact, &mut state).burst,
            Some(BurstRequest::RADAR)
        );
        contact.velocity = Some([0.0, 0.0, 1_000.0]);
        let opening = assess(362, Experience::Ace, own(), contact, &mut state);
        assert!(!opening.debug.passed);
        assert_eq!(opening.motion.unwrap().maneuver, Maneuver::Break);
        contact.position = Some([0.0, 5_000.0, 30_000.0]);
        contact.velocity = Some([0.0, 0.0, -1_000.0]);
        let later = assess(
            363 + ENCOUNTER_MEMORY_TICKS + 1,
            Experience::Ace,
            own(),
            contact,
            &mut state,
        );
        assert_eq!(later.motion.unwrap().maneuver, Maneuver::Drag);
    }

    #[test]
    fn a_brief_gap_in_the_threat_picture_does_not_restart_the_reaction() {
        let mut contact = threat(EvidenceSource::Visual, Some(GuidanceClass::Infrared));
        contact.position = Some([0.0, 5_000.0, 3_000.0]);
        let mut state = DefenseState::default();
        for tick in 0..=90 {
            assess(tick, Experience::Average, own(), contact, &mut state);
        }
        let before = assess(91, Experience::Average, own(), contact, &mut state)
            .motion
            .unwrap();
        // The end-game record drops out for a few ticks and comes back.
        for tick in 92..100 {
            assert_eq!(
                decide(tick, Experience::Average, own(), &[], &mut state, |_| 0.0)
                    .unwrap()
                    .motion
                    .unwrap(),
                before
            );
        }
        let resumed = assess(100, Experience::Average, own(), contact, &mut state);
        assert_eq!(resumed.motion.unwrap().maneuver, before.maneuver);
        assert_eq!(resumed.motion.unwrap().heading_deg, before.heading_deg);
        // After a quiet spell longer than the memory, a new encounter starts
        // with the pilot's reaction delay again.
        let fresh = assess(
            101 + ENCOUNTER_MEMORY_TICKS,
            Experience::Average,
            own(),
            contact,
            &mut state,
        );
        assert!(fresh.motion.is_none());
    }

    #[test]
    fn infrared_beams_cool_and_only_flare_in_the_endgame() {
        for class in [Some(GuidanceClass::Infrared), None] {
            let mut state = DefenseState::default();
            let mut contact = threat(EvidenceSource::Visual, class);
            contact.position = Some([0.0, 5_000.0, 10_000.0]);
            let beam = assess(0, Experience::Ace, own(), contact, &mut state);
            assert_eq!(beam.motion.unwrap().maneuver, Maneuver::Beam);
            assert!(beam.motion.unwrap().reduce_heat);
            assert_eq!(beam.burst, None);
            contact.position = Some([0.0, 5_000.0, 5_000.0]);
            assert_eq!(
                assess(120, Experience::Ace, own(), contact, &mut state).burst,
                Some(BurstRequest::INFRARED)
            );
        }
    }

    #[test]
    fn notch_side_survives_a_crossing_of_the_shortest_turn_boundary() {
        let mut state = DefenseState::default();
        let mut contact = threat(
            EvidenceSource::ElectronicSupported,
            Some(GuidanceClass::Radar),
        );
        contact.position = Some([0.0, 5_000.0, 10_000.0]);
        contact.radar_bearing_deg = Some(1.0);
        let first = assess(0, Experience::Ace, own(), contact, &mut state)
            .motion
            .unwrap();
        contact.radar_bearing_deg = Some(-1.0);
        let second = assess(1, Experience::Ace, own(), contact, &mut state)
            .motion
            .unwrap();
        assert_eq!(first.heading_deg, 271.0);
        assert_eq!(second.heading_deg, 269.0);
    }

    #[test]
    fn beam_side_avoids_terrain_and_observed_threat_cluster() {
        let mut state = DefenseState::default();
        let contact = threat(
            EvidenceSource::ElectronicSupported,
            Some(GuidanceClass::Radar),
        );
        let left_blocked = |p: Vector| if p[0] < -100.0 { 5_000.0 } else { 0.0 };
        assert_eq!(
            beam_heading(own(), 0.0, &[contact], &mut state, &mut { left_blocked }),
            90.0
        );
        let right_blocked = |p: Vector| if p[0] > 100.0 { 5_000.0 } else { 0.0 };
        assert_eq!(
            beam_heading(own(), 0.0, &[contact], &mut state, &mut { right_blocked }),
            270.0
        );
        state.beam_side = None;
        let mut flank = contact;
        flank.bearing_deg = -90.0;
        assert_eq!(
            beam_heading(own(), 0.0, &[contact, flank], &mut state, &mut |_| 0.0),
            90.0
        );
    }

    #[test]
    fn jink_waits_for_turn_to_establish_and_scales_with_speed_and_skill() {
        let mut state = DefenseState::default();
        assert_eq!(
            jink_heading(0, own(), Experience::Ace, 2, true, &mut state),
            45.0
        );
        assert_eq!(
            jink_heading(240, own(), Experience::Ace, 2, false, &mut state),
            45.0
        );
        assert_eq!(
            jink_heading(2400, own(), Experience::Ace, 2, false, &mut state),
            45.0
        );
        let mut arrived = own();
        arrived.heading_deg = 45.0;
        assert_eq!(
            jink_heading(2401, arrived, Experience::Ace, 2, false, &mut state),
            315.0
        );
        for (skill, rate, should_switch) in [
            (Experience::Ace, 30.0, true),
            (Experience::Novice, 30.0, false),
            (Experience::Ace, 10.0, false),
        ] {
            let mut state = DefenseState::default();
            let mut moving = own();
            moving.usable_turn_rate_deg_s = rate;
            jink_heading(0, moving, skill, 2, true, &mut state);
            moving.heading_deg = 45.0;
            assert_eq!(
                jink_heading(720, moving, skill, 2, false, &mut state) == 315.0,
                should_switch
            );
        }
    }

    #[test]
    fn skill_changes_reaction_break_timing_and_pull() {
        let mut contact = threat(EvidenceSource::Visual, Some(GuidanceClass::Infrared));
        contact.position = Some([0.0, 5_000.0, 3_000.0]);
        for (skill, delay, phase) in [
            (Experience::Novice, 180, Maneuver::Beam),
            (Experience::Average, 90, Maneuver::Beam),
            (Experience::Experienced, 30, Maneuver::Break),
            (Experience::Ace, 0, Maneuver::Break),
        ] {
            let mut state = DefenseState::default();
            let first = assess(0, skill, own(), contact, &mut state);
            assert_eq!(first.motion.is_some(), delay == 0);
            if delay > 0 {
                assert_eq!(
                    assess(delay - 1, skill, own(), contact, &mut state).motion,
                    None
                );
            }
            assert_eq!(
                assess(delay, skill, own(), contact, &mut state)
                    .motion
                    .unwrap()
                    .maneuver,
                phase
            );
        }
        assert!(load_fraction(Experience::Novice) < load_fraction(Experience::Average));
        assert!(load_fraction(Experience::Average) < load_fraction(Experience::Experienced));
        assert_eq!(load_fraction(Experience::Ace), 1.0);
    }

    #[test]
    fn descent_requires_clearance_along_current_and_turning_paths() {
        let mut own = own();
        assert!(dive_is_safe(own, 90.0, &mut |_| 0.0));
        assert!(!dive_is_safe(own, 90.0, &mut |p| if p[2] > 100.0 {
            5_000.0
        } else {
            0.0
        }));
        own.position[1] = 1_000.0;
        assert!(!dive_is_safe(own, 90.0, &mut |_| 0.0));
        assert_eq!(safe_jink(own, 90.0, |_| 0.0).flight_path_pitch_deg, 0.0);
    }

    #[test]
    fn each_skill_keeps_pairs_in_distinct_countermeasure_episodes() {
        for (skill, first) in Experience::ALL.into_iter().zip([180, 90, 30, 0]) {
            let mut state = DefenseState::default();
            let mut contact = threat(EvidenceSource::Visual, Some(GuidanceClass::Infrared));
            contact.position = Some([0., 5000., 1000.]);
            let mut starts = Vec::new();
            for tick in 0..1000 {
                if let Some(burst) = assess(tick, skill, own(), contact, &mut state).burst {
                    assert_eq!(burst, BurstRequest::INFRARED);
                    starts.push(tick);
                }
            }
            assert_eq!(starts, [first, first + 240, first + 480], "{skill:?}");
            assert_eq!(state.devices_requested, 6);
        }
    }
    #[test]
    fn routine_budget_cooldown_and_stale_observations_limit_dispensing() {
        let mut contact = threat(EvidenceSource::ElectronicActive, Some(GuidanceClass::Radar));
        contact.position = None;
        contact.radar_bearing_deg = Some(90.0);
        let mut state = DefenseState::default();
        let mut bursts = Vec::new();
        for tick in 0..1440 {
            if assess(tick, Experience::Ace, own(), contact, &mut state)
                .burst
                .is_some()
            {
                bursts.push(tick);
            }
        }
        assert_eq!(bursts, [0, 360, 720]);
        contact.missile_id += 1;
        assert_eq!(
            assess(1440, Experience::Ace, own(), contact, &mut state).burst,
            None
        );
        contact.position = Some([0.0, 5_000.0, 1000.0]);
        assert_eq!(
            assess(1441, Experience::Ace, own(), contact, &mut state).burst,
            Some(BurstRequest::RADAR)
        );
        contact.stale = true;
        assert_eq!(
            assess(1800, Experience::Ace, own(), contact, &mut state).burst,
            None
        );
    }

    #[test]
    fn own_missiles_in_flight_do_not_hold_the_encounter_budget() {
        // An own missile in flight (or one seen going elsewhere) is not an
        // incoming threat and must not keep an encounter, its spent budget and
        // its finished reaction open for a later missile.
        let mut own_missile = threat(EvidenceSource::OwnLaunch, Some(GuidanceClass::Radar));
        own_missile.missile_id = 50;
        own_missile.targeting_receiver = false;
        own_missile.was_targeting_receiver = false;
        own_missile.position = Some([0.0, 5000.0, 30_000.0]);
        own_missile.velocity = Some([0.0, 0.0, 2000.0]);
        let mut state = DefenseState::default();
        let mut step = |tick: u64, incoming: Option<ThreatRecord>, state: &mut DefenseState| {
            own_missile.observed_tick = tick;
            let mut contacts = vec![own_missile];
            if let Some(mut c) = incoming {
                c.observed_tick = tick;
                contacts.push(c);
            }
            decide(tick, Experience::Ace, own(), &contacts, state, |_| 0.0)
                .and_then(|d| d.burst)
                .map_or(0, |b| b.chaff)
        };
        let mut a = threat(
            EvidenceSource::ElectronicSupported,
            Some(GuidanceClass::Radar),
        );
        a.position = Some([0.0, 5000.0, 1000.0]);
        let mut spent = 0;
        for tick in 0..600 {
            spent += step(tick, Some(a), &mut state);
        }
        a.missile_id = 3;
        for tick in 600..1200 {
            spent += step(tick, Some(a), &mut state);
        }
        assert_eq!(spent, 8);
        for tick in 1200..20_000 {
            step(tick, None, &mut state);
        }
        // Over two minutes later a new missile arrives.
        a.missile_id = 4;
        let mut later = 0;
        for tick in 20_000..20_600 {
            later += step(tick, Some(a), &mut state);
        }
        assert!(later > 0, "a missile 2.5 minutes later got no devices");
    }

    #[test]
    fn clearing_threat_keeps_the_aircraft_release_cooldown() {
        let contact = threat(EvidenceSource::Visual, Some(GuidanceClass::Infrared));
        let mut state = DefenseState::default();
        assert!(
            assess(0, Experience::Ace, own(), contact, &mut state)
                .burst
                .is_some()
        );
        assert!(
            decide(1, Experience::Ace, own(), &[], &mut state, |_| 0.0)
                .unwrap()
                .burst
                .is_none()
        );
        assert_eq!(
            assess(100, Experience::Ace, own(), contact, &mut state).burst,
            None
        );
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "defense_checkpoint.rs"]
mod checkpoint;
