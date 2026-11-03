//! Display trees for the debug panels, recordings and logs: an AI aircraft's
//! thinking (`ai.thought`), any aircraft's flight-model telemetry
//! (`flight.telemetry`) and a guided missile's guidance (`weapon.guidance`).
//!
//! Every builder here is a pure function of plain inputs: the simulation's
//! write-only "why" records and a few values the caller copies from the
//! tick. Nothing here reads the simulation directly, so the recorder and a
//! live panel build the same tree from the same tick. The layouts are agent
//! decisions (2026-09-26), described in docs/REPLAYS.md ("Display trees").
//!
//! Numbers go in node values, rounded to what a panel shows, and reasons go
//! in notes. Notes avoid numbers that change every tick, so a delta-coded
//! recording stays small; where a note carries a number (a failing range
//! check, a stall speed) it is rounded to the precision a reader needs.
//! Measurements (angle of attack, sideslip, Mach, dynamic pressure, height
//! above ground) are labelled as measurements and never given as causes.
//! Opinionated addition requested by John on 2026-09-26.

use tore_replay::{
    Node, Value,
    vocab::{node, unit},
};
use tore_sim::ai::{
    Draw,
    airfield::Phase as AirfieldPhase,
    controller::{Activity, EmploymentCheck, EmploymentFailure, StationVerdict, StationView},
    defense::{self, DefenseDecision},
    engagement::{CandidateExplanation, Explanation, Priority, Role, Stance},
    experience::{ExperienceOrigin, ResolvedExperience},
    fitted::Fallback,
    route::FuelState,
    tactics::{BehaviorChoice, MissileLaunch, MissileReaction, Quadrant},
    thought::{
        ActorPath, ActorTrace, AirfieldExit, ControllerTrace, ManeuverPath, ManeuverTrace,
        MissionTargetRefusal, MotionBranch, PitchSource, RejoinReason, ResolveTrace, StepPath,
        TargetPath, WeaponTrace,
    },
    threat::{ScriptReason, SeekerClass, Suppression, WarningReaction},
    weapon_service::{Phase, ServiceOutcome, StationId, WithholdReason},
};
use tore_sim::flight::trace::{
    Block, BurnerBlock, Contact, DeviceKind, Effect, FlightTrace, Path, Release, SpinExit, Stop,
};

/// Feet per second in a knot.
pub const FPS_PER_KT: f64 = 1.687_809_857;
/// Feet in a nautical mile.
pub const FT_PER_NM: f64 = 6_076.115_49;
/// Simulation ticks per second.
const TICKS: f64 = 120.;
/// AI deadlines count quarter seconds of 30 ticks.
const QUARTER_TICKS: u64 = 30;
/// Pounds per square foot in a pascal.
const LB_FT2_PER_PA: f64 = 0.020_885_434;

// ---------------------------------------------------------------------------
// Numbers and words

/// `v` rounded to `decimals` places (to tens for -1), so it stores as a
/// short decimal. Never negative zero; a value that is not finite is kept,
/// so the anomaly check still sees it.
pub fn round(v: f64, decimals: i32) -> f64 {
    if !v.is_finite() {
        return v;
    }
    let r = if decimals >= 0 {
        let scale = 10f64.powi(decimals);
        (v * scale).round() / scale
    } else {
        let scale = 10f64.powi(-decimals);
        (v / scale).round() * scale
    };
    if r == 0. { 0. } else { r }
}

/// A number for people: rounded, trailing zeros trimmed, never `-0`.
pub fn fixed(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let mut text = format!("{v:.decimals$}");
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" { "0".into() } else { text }
}

/// A whole number with thousands separators: `12,340`.
pub fn thousands(v: f64) -> String {
    if !v.is_finite() {
        return format!("{v}");
    }
    let rounded = v.round();
    let digits = format!("{:.0}", rounded.abs());
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if rounded < 0. { format!("-{out}") } else { out }
}

/// A distance for people: nautical miles to one decimal from a mile up,
/// otherwise whole feet.
pub fn distance(feet: f64) -> String {
    if feet.abs() >= FT_PER_NM {
        format!("{:.1} nm", feet / FT_PER_NM)
    } else {
        format!("{} ft", thousands(feet))
    }
}

/// Knots from feet per second.
pub fn knots(fps: f64) -> f64 {
    fps / FPS_PER_KT
}

/// Mission clock for a tick: `M:SS.s`, or `H:MM:SS.s` past an hour.
pub fn clock(tick: u64) -> String {
    let tenths = tick * 10 / 120;
    let (hours, rest) = (tenths / 36_000, tenths % 36_000);
    let (minutes, rest) = (rest / 600, rest % 600);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{:02}.{}", rest / 10, rest % 10)
    } else {
        format!("{minutes}:{:02}.{}", rest / 10, rest % 10)
    }
}

/// A tree under construction.
#[derive(Default)]
struct Tree {
    nodes: Vec<Node>,
}

impl Tree {
    fn add(&mut self, depth: u8, label: &str, value: Value, unit: &str, note: &str) {
        self.nodes.push(Node {
            depth,
            label: label.to_owned(),
            value,
            unit: unit.to_owned(),
            note: note.to_owned(),
        });
    }

    /// A heading line with no value of its own.
    fn head(&mut self, depth: u8, label: &str, note: &str) {
        self.add(depth, label, Value::None, "", note);
    }

    fn text(&mut self, depth: u8, label: &str, text: impl Into<String>, note: &str) {
        self.add(depth, label, Value::Text(text.into()), "", note);
    }

    fn num(&mut self, depth: u8, label: &str, v: f64, decimals: i32, unit: &str, note: &str) {
        self.add(depth, label, Value::Num(round(v, decimals)), unit, note);
    }

    fn int(&mut self, depth: u8, label: &str, v: i64, note: &str) {
        self.add(depth, label, Value::Int(v), "", note);
    }

    fn id(&mut self, depth: u8, label: &str, id: Option<u32>, note: &str) {
        let value = id.map_or(Value::None, Value::Id);
        self.add(depth, label, value, "", note);
    }

    fn flag(&mut self, depth: u8, label: &str, on: bool, note: &str) {
        self.add(depth, label, Value::Bool(on), "", note);
    }
}

// ---------------------------------------------------------------------------
// Labels shared by trees and events

pub fn role_label(role: Role) -> &'static str {
    match role {
        Role::FreeEngagement => "Free engagement",
        Role::CombatAirPatrol => "Combat air patrol",
        Role::Intercept => "Intercept",
        Role::Escort => "Escort",
        Role::Disengage => "Disengage",
    }
}

pub fn stance_label(stance: Stance) -> &'static str {
    match stance {
        Stance::WeaponsHold => "weapons hold",
        Stance::SelfDefense => "self-defense",
        Stance::ProtectAssigned => "protect assigned",
        Stance::EngageAssigned => "engage assigned",
    }
}

pub fn priority_label(priority: Priority) -> &'static str {
    match priority {
        Priority::OwnDefense => "own defense",
        Priority::ProtectedThreat => "threat to a protected aircraft",
        Priority::HostileEscort => "hostile escort",
        Priority::ApproachingThreat => "approaching threat",
        Priority::Assigned => "assigned",
        Priority::Free => "free",
    }
}

pub fn phase_label(phase: Phase) -> &'static str {
    match phase {
        Phase::Search => "Search",
        Phase::Prepare => "Prepare",
        Phase::LockCheck => "Lock check",
        Phase::Tracking => "Tracking",
        Phase::Fire => "Fire",
        Phase::Reload => "Reload",
        Phase::WindowExpired => "Window expired",
    }
}

pub fn airfield_label(phase: AirfieldPhase) -> &'static str {
    match phase {
        AirfieldPhase::Waiting => "waiting to take off",
        AirfieldPhase::Taxi => "taxiing out",
        AirfieldPhase::LineUp => "lining up",
        AirfieldPhase::TakeoffRoll => "takeoff roll",
        AirfieldPhase::ClimbOut => "climbing out",
        AirfieldPhase::Inbound => "inbound",
        AirfieldPhase::Marshal => "holding at marshal",
        AirfieldPhase::Approach => "approach",
        AirfieldPhase::Final => "final",
        AirfieldPhase::Rollout => "rollout",
        AirfieldPhase::TaxiClear => "taxiing to parking",
        AirfieldPhase::Parked => "parked",
        AirfieldPhase::VerticalHold => "holding before a vertical takeoff",
        AirfieldPhase::VerticalClimb => "vertical climb",
        AirfieldPhase::Transition => "transition to forward flight",
        AirfieldPhase::HoverApproach => "hover approach",
        AirfieldPhase::VerticalDescent => "vertical descent",
        AirfieldPhase::Settle => "landed, settling",
        AirfieldPhase::AirTaxi => "air taxiing to parking",
    }
}

pub fn fuel_label(state: FuelState) -> &'static str {
    match state {
        FuelState::NoManagement => "not managed (no home base)",
        FuelState::OutOfFuel => "out of fuel",
        FuelState::Critical => "critical",
        FuelState::Bingo => "bingo",
        FuelState::Caution => "caution",
        FuelState::Ok => "normal",
    }
}

pub fn seeker_label(seeker: SeekerClass) -> &'static str {
    match seeker {
        SeekerClass::Infrared => "infrared",
        SeekerClass::Radar => "radar",
    }
}

fn script_reason(reason: ScriptReason) -> &'static str {
    match reason {
        ScriptReason::Idle => "idle",
        ScriptReason::Evade => "evade",
        ScriptReason::Attack => "attack",
        ScriptReason::RadarLaunch => "radar launch",
        ScriptReason::IrLaunch => "infrared launch",
        ScriptReason::Hit => "hit",
    }
}

/// What an AI aircraft does with a launch warning.
pub fn warning_reaction(reaction: WarningReaction) -> String {
    match reaction {
        WarningReaction::Suppressed(Suppression::HoldTime) => {
            "suppressed until the mission's hold time".into()
        }
        WarningReaction::Ignored => "ignored while taking off or landing".into(),
        WarningReaction::NoReaction => "no reaction: no countermeasure dispenser".into(),
        WarningReaction::RadioOnly => "a radio call only: the launcher is on its side".into(),
        WarningReaction::NoManeuver => "no maneuver: the launcher is already its target".into(),
        WarningReaction::Maneuver {
            reason,
            wing_reaction,
        } => format!(
            "maneuver for a {} reason{}",
            script_reason(reason),
            if wing_reaction {
                ", and the wing reacts"
            } else {
                ""
            }
        ),
    }
}

fn refusal_label(refusal: MissionTargetRefusal) -> &'static str {
    match refusal {
        MissionTargetRefusal::NotObserved => "its own sensors do not see it",
        MissionTargetRefusal::Own => "it is itself",
        MissionTargetRefusal::SameSide => "it is on its own side",
        MissionTargetRefusal::Invalid => "it is no longer valid",
        MissionTargetRefusal::TypeNotAllowed => "its type may not be attacked",
        MissionTargetRefusal::NoUsableWeapon => "no store can engage it",
    }
}

fn fallback_list(fallbacks: &[Fallback]) -> String {
    fallbacks
        .iter()
        .map(|f| f.name())
        .collect::<Vec<_>>()
        .join(", ")
}

/// How the target was chosen, and a note.
pub fn target_path(
    path: TargetPath,
    who: &dyn Fn(u32) -> String,
) -> Option<(&'static str, String)> {
    Some(match path {
        TargetPath::NotRun => return None,
        TargetPath::HoldFire => ("weapons held", String::new()),
        TargetPath::Mission { requested, refused } => {
            let note = match (requested, refused) {
                (Some(id), Some(refusal)) => format!(
                    "the mission chose {} but {}",
                    who(id),
                    refusal_label(refusal)
                ),
                (None, _) => "the mission chose no target".into(),
                (Some(_), None) => String::new(),
            };
            ("mission ranking", note)
        }
        TargetPath::Retained { .. } => ("kept", "still valid and inside 20,000 ft".into()),
        TargetPath::Ranked { candidates, .. } => (
            "nearest first",
            format!("{candidates} candidates ranked by distance and penalties"),
        ),
    })
}

/// Why an aircraft could not be a mission target.
fn ineligible(c: &CandidateExplanation) -> Vec<&'static str> {
    let i = c.ineligible;
    let mut out = Vec::new();
    for (on, why) in [
        (i.own, "itself"),
        (i.same_side, "same side"),
        (i.invalid, "not valid"),
        (i.not_aircraft, "not an aircraft"),
        (i.type_not_allowed, "type not allowed"),
        (i.no_usable_weapon, "no usable store"),
    ] {
        if on {
            out.push(why);
        }
    }
    out
}

fn penalties(c: &CandidateExplanation) -> Vec<&'static str> {
    let mut out = Vec::new();
    if c.not_aircraft_penalty {
        out.push("not an aircraft");
    }
    if c.wing_attacking_penalty {
        out.push("a wing member attacks it");
    }
    if c.wing_full_penalty {
        out.push("its attackers are full");
    }
    out
}

/// One line about a candidate of the mission ranking: its priority or why it
/// could not be chosen, then its penalties.
fn candidate_note(c: &CandidateExplanation) -> String {
    let reasons = ineligible(c);
    if !reasons.is_empty() {
        return format!("not eligible: {}", reasons.join(", "));
    }
    let mut note = match c.priority {
        Some(p) => priority_label(p).to_owned(),
        None => "no priority".to_owned(),
    };
    let p = penalties(c);
    if !p.is_empty() {
        note.push_str(&format!(", penalties: {}", p.join(", ")));
    }
    note
}

/// The target's ranking summary for an `ai.target` event: priority, score
/// and the runner-up.
pub struct Ranking {
    pub priority: Option<Priority>,
    pub score_ft: Option<f64>,
    pub runner_up: Option<(u32, Option<Priority>, f64)>,
}

/// The mission ranking's view of `target`, when it ranked one.
pub fn ranking(explanation: &Explanation, target: Option<u32>) -> Ranking {
    let chosen = target.and_then(|id| explanation.candidates.iter().find(|c| c.id == id));
    let runner_up = explanation
        .candidates
        .iter()
        .find(|c| Some(c.id) != target && !c.ineligible.any() && c.priority.is_some())
        .map(|c| (c.id, c.priority, c.score_ft));
    Ranking {
        priority: chosen.and_then(|c| c.priority),
        score_ft: chosen.map(|c| c.score_ft),
        runner_up,
    }
}

/// Text for one random draw: `roll 37 < 50: best attack`.
pub fn draw_text(draw: &Draw) -> String {
    let site = draw.site.unwrap_or("draw");
    match (draw.threshold, draw.passed()) {
        (Some(threshold), Some(passed)) => format!(
            "roll {} {} {threshold}: {site}",
            draw.value,
            if passed { "<" } else { ">=" }
        ),
        _ => {
            let low = i64::from(draw.offset);
            let high = low + i64::from(draw.bound) - 1;
            format!("draw {} of {low}..{high}: {site}", draw.result())
        }
    }
}

pub fn choice_label(choice: &BehaviorChoice) -> String {
    match choice {
        BehaviorChoice::Pursuit => "Pursuit".into(),
        BehaviorChoice::LastDitch => "Last ditch".into(),
        BehaviorChoice::Straight => "Straight".into(),
        BehaviorChoice::WingSplit => "Wing split".into(),
        BehaviorChoice::SpecialApproach(a) => format!("Special approach ({a:?})"),
        BehaviorChoice::CoordinatedEscape(e) => format!("Coordinated escape ({e:?})"),
        BehaviorChoice::FlyAway => "Fly away".into(),
        BehaviorChoice::VerticalJink => "Vertical jink".into(),
        BehaviorChoice::NonAircraftEvasion(e) => format!("Evasion from a non-aircraft ({e:?})"),
        BehaviorChoice::MissileReaction(r) => reaction_label(*r),
        BehaviorChoice::SurfaceAttack(a) => format!("Surface attack ({a:?})"),
    }
}

fn reaction_label(reaction: MissileReaction) -> String {
    match reaction {
        MissileReaction::ClimbToPitch90 { .. } => "climb to 90 degrees".into(),
        MissileReaction::TurnTowardTarget { heading_deg } => {
            format!("turn toward the target (heading {heading_deg})")
        }
        MissileReaction::BreakNinety { heading_deg, side } => {
            format!("break 90 degrees {side:?} (heading {heading_deg})").to_lowercase()
        }
    }
}

pub fn maneuver_path(path: &ManeuverPath) -> String {
    match path {
        ManeuverPath::ReturnToBase { home_heading_deg } => {
            format!("return to base (home heading {home_heading_deg})")
        }
        ManeuverPath::MissileReaction { launch, response } => format!(
            "{} launch reaction: {}",
            match launch {
                MissileLaunch::Infrared => "infrared",
                MissileLaunch::Radar => "radar",
            },
            reaction_label(response.reaction)
        ),
        ManeuverPath::NoTarget => "no target: straight flight".into(),
        ManeuverPath::NoBearing => "target straight above or below".into(),
        ManeuverPath::Evasion => "evasion".into(),
        ManeuverPath::Approach => "ordinary approach".into(),
    }
}

fn quadrant_label(quadrant: Quadrant) -> &'static str {
    match quadrant {
        Quadrant::AheadFacing => "target ahead, facing us",
        Quadrant::AheadFacingAway => "target ahead, facing away",
        Quadrant::BehindFacing => "target behind, facing us",
        Quadrant::BehindFacingAway => "target behind, facing away",
    }
}

/// The motion branch that flew the aircraft, and a note.
pub fn motion_branch(
    branch: &MotionBranch,
    actor: &ActorTrace,
    who: &dyn Fn(u32) -> String,
) -> (&'static str, String) {
    match branch {
        MotionBranch::None => ("none", String::new()),
        MotionBranch::DamageRecovery => ("damage recovery", "returning with weapons held".into()),
        MotionBranch::Strike {
            heading_deg,
            pitch_deg,
        } => (
            "strike",
            format!("heading {heading_deg:.0}, pitch {pitch_deg:.0}"),
        ),
        MotionBranch::GunTracking {
            heading_deg,
            pitch_deg,
        } => (
            "gun lead tracking",
            format!("heading {heading_deg:.1}, flight pitch {pitch_deg:.1}"),
        ),
        MotionBranch::MissileDefense {
            heading_deg,
            pitch_deg,
        } => (
            "missile defense",
            format!("heading {:.0}, pitch {:.0}", heading_deg, pitch_deg),
        ),
        MotionBranch::IncomingFire {
            heading_deg,
            pitch_deg,
        } => (
            "incoming fire defense",
            format!(
                "heading {heading_deg:.0}, pitch {pitch_deg:.0}; shooter identity not required"
            ),
        ),
        MotionBranch::MissionRejoin { .. } => (
            "rejoining",
            match actor.rejoin.map(|r| r.reason) {
                Some(RejoinReason::EscortLeash) => "an escort beyond its 10 nm leash".into(),
                Some(RejoinReason::OutsidePatrol) => "outside its patrol region".into(),
                None => String::new(),
            },
        ),
        MotionBranch::SearchBearing { bearing_deg } => (
            "searching along a bearing",
            format!("bearing {:.0}", bearing_deg),
        ),
        MotionBranch::OrderedApproach { target, .. } => {
            ("ordered approach", format!("closing on {}", who(*target)))
        }
        MotionBranch::OrderedMotion => ("ordered maneuver", String::new()),
        MotionBranch::Search {
            contact,
            orbiting,
            opportunity,
            ..
        } => (
            if *opportunity {
                "searching for the enemy"
            } else {
                "investigating a contact"
            },
            format!(
                "{} last seen at {}, {}",
                who(contact.id),
                clock(contact.observed_tick),
                if *orbiting {
                    "orbiting over it"
                } else {
                    "flying to it"
                }
            ),
        ),
        MotionBranch::WingRoute { distance_ft, .. } => (
            "flying the wing's waypoints",
            format!(
                "nothing found on the search; next waypoint {:.1} nm",
                distance_ft / 6_076.12
            ),
        ),
        MotionBranch::SearchAbandoned { contact } => (
            "search abandoned",
            format!("an Ace gives up on {} after two minutes", who(contact.id)),
        ),
        MotionBranch::Formation(f) => (
            "formation",
            if f.close {
                "close formation".into()
            } else {
                "formation slot".into()
            },
        ),
        MotionBranch::ActiveContinues => ("tactics", "the running maneuver continues".into()),
        MotionBranch::NotYetDue => ("tactics", "the next choice is not due yet".into()),
        MotionBranch::NewManeuver => ("tactics", "a new choice this tick".into()),
    }
}

/// A coarse code for the motion branch, for change detection: the tactical
/// branches count as one, so a new choice every few seconds is a maneuver
/// change rather than a branch change.
pub fn branch_code(controller: &ControllerTrace, actor: &ActorTrace) -> u8 {
    if actor.path != ActorPath::Controller {
        return match actor.path {
            ActorPath::DamageHold => 23,
            ActorPath::NotRun => 0,
            ActorPath::Destroyed => 20,
            ActorPath::Dummy => 21,
            ActorPath::Airfield => 22,
            ActorPath::Controller => unreachable!(),
        };
    }
    match controller.motion.branch {
        MotionBranch::None => 1,
        MotionBranch::DamageRecovery => 13,
        MotionBranch::Strike { .. } => 14,
        MotionBranch::MissileDefense { .. } => 2,
        MotionBranch::IncomingFire { .. } => 11,
        MotionBranch::GunTracking { .. } => 12,
        MotionBranch::MissionRejoin { .. } => 3,
        MotionBranch::SearchBearing { .. } => 4,
        MotionBranch::OrderedApproach { .. } => 5,
        MotionBranch::OrderedMotion => 6,
        MotionBranch::Search { .. } | MotionBranch::WingRoute { .. } => 7,
        MotionBranch::SearchAbandoned { .. } => 8,
        MotionBranch::Formation(_) => 9,
        MotionBranch::ActiveContinues | MotionBranch::NotYetDue | MotionBranch::NewManeuver => 10,
    }
}

fn verdict_label(verdict: StationVerdict) -> &'static str {
    match verdict {
        StationVerdict::Inhibited => "switched off",
        StationVerdict::RadarOff => "needs its radar, which is off",
        StationVerdict::NoSensorTrack => "needs a sensor track",
        StationVerdict::WrongTargetClass => "wrong kind of target",
        StationVerdict::Empty => "empty",
        StationVerdict::OutsideEnvelope => "outside its envelope",
        StationVerdict::Usable { .. } => "usable",
    }
}

fn failure_words(failure: EmploymentFailure) -> &'static str {
    match failure {
        EmploymentFailure::BelowMinimumRange => "too close",
        EmploymentFailure::BeyondMaximumRange => "too far",
        EmploymentFailure::BeyondAngleLimit => "too far off the nose",
        EmploymentFailure::OutsideZone => "outside its zone",
    }
}

/// One employment failure with its numbers: `outside max range (6.1 nm >
/// 5.8 nm)`.
pub fn failure_text(
    failure: EmploymentFailure,
    check: &EmploymentCheck,
    view: Option<&StationView>,
) -> String {
    match failure {
        EmploymentFailure::BelowMinimumRange => match view {
            Some(v) => format!(
                "inside min range ({} < {})",
                distance(check.range_ft),
                distance(v.minimum_range_ft)
            ),
            None => "inside min range".into(),
        },
        EmploymentFailure::BeyondMaximumRange => match view.and_then(|v| v.maximum_range_ft) {
            Some(max) => format!(
                "outside max range ({} > {})",
                distance(check.range_ft),
                distance(max)
            ),
            None => "outside max range".into(),
        },
        EmploymentFailure::BeyondAngleLimit => match view.and_then(|v| v.employment_limit_deg) {
            Some(limit) => format!(
                "too far off the nose ({:.0} deg > {:.0} deg)",
                check.error_deg, limit
            ),
            None => "too far off the nose".into(),
        },
        EmploymentFailure::OutsideZone => "outside its launch zone".into(),
    }
}

/// A weapon service outcome in words. Deadlines are given as mission clock
/// times (`at`), so the words stay the same while the aircraft waits.
pub fn service_text(outcome: &ServiceOutcome, at: &dyn Fn(u64) -> String) -> String {
    match outcome {
        ServiceOutcome::Waiting(phase) => format!("waiting in {}", phase_label(*phase)),
        ServiceOutcome::NoTargetRetry { deadline } => {
            format!("no target; search again at {}", at(*deadline))
        }
        ServiceOutcome::Preparing { ready_at, .. } => {
            format!("preparing; ready at {}", at(*ready_at))
        }
        ServiceOutcome::TargetLost => "target lost; back to search".into(),
        ServiceOutcome::NoStationRetry { deadline } => {
            format!("no usable store; retry at {}", at(*deadline))
        }
        ServiceOutcome::LockRetry { deadline } => {
            format!("lock failed; retry at {}", at(*deadline))
        }
        ServiceOutcome::Locked { fire_at } => {
            format!("locked; fires at {} after its tracking delay", at(*fire_at))
        }
        ServiceOutcome::Withheld(WithholdReason::PathBlocked) => {
            "held: terrain blocks the firing path".into()
        }
        ServiceOutcome::Fire(_) => "fired".into(),
        ServiceOutcome::StoreDepleted { deadline } => {
            format!("store empty after firing; retry at {}", at(*deadline))
        }
        ServiceOutcome::WindowExpired => "attempt expired; bounded retry pending".into(),
    }
}

// ---------------------------------------------------------------------------
// ai.thought

/// A maneuver choice, kept until the next one so every thought sample can
/// show what chose the maneuver being flown.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// Recording tick of the choice.
    pub tick: u64,
    pub maneuver: ManeuverTrace,
    pub resolve: Option<ResolveTrace>,
    /// Every draw of that tick, with what each decided.
    pub draws: Vec<Draw>,
}

/// One decision change for the thought tree's "Recent" list.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    /// Recording tick.
    pub tick: u64,
    /// What changed: `Formation -> Attacking`, `target none -> You`.
    pub what: String,
    /// Why, when known.
    pub why: String,
}

/// The defended-against missile as the recording sees it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Threat {
    pub shooter: Option<u32>,
    pub weapon: Option<String>,
    /// True distance from the aircraft to the missile, feet.
    pub range_ft: Option<f64>,
}

/// Geometry against the target from the recorded states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Relative {
    /// Angle off the target's tail: 0 dead astern, 180 head on.
    pub aspect_deg: f64,
    pub closure_kt: f64,
    /// Target height minus own height, feet.
    pub height_ft: f64,
}

/// The aircraft's own state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Own {
    pub position: [f64; 3],
    pub speed_fps: f64,
    pub g: f64,
    /// Positive G limit the flight model applied, when known.
    pub g_limit: Option<f64>,
    pub fuel_lb: f64,
    pub afterburner: bool,
}

/// Everything one `ai.thought` tree reads.
pub struct Thought<'a> {
    /// The recording tick described.
    pub tick: u64,
    pub label: &'a str,
    /// Exact aircraft name, for example `MiG-29`.
    pub name: &'a str,
    pub side: &'a str,
    pub wing: u16,
    pub member: u16,
    pub leader: bool,
    pub experience: Option<ResolvedExperience>,
    pub activity: Activity,
    /// Recording tick the activity began.
    pub activity_since: u64,
    /// Why the activity last changed.
    pub activity_reason: &'a str,
    pub target: Option<u32>,
    pub weapon_phase: Phase,
    /// Holding formation until released.
    pub neutral: bool,
    pub airfield: Option<AirfieldPhase>,
    pub defense: Option<DefenseDecision>,
    pub threat: Option<Threat>,
    pub controller: &'a ControllerTrace,
    pub actor: &'a ActorTrace,
    /// The controller's draws on the latest tick.
    pub draws: &'a [Draw],
    pub choice: Option<&'a Choice>,
    pub relative: Option<Relative>,
    pub own: Own,
    /// Fitted fallbacks the mission reported for this aircraft this tick.
    pub fallbacks: &'a [Fallback],
    pub recent: &'a [Change],
    /// A name for an aircraft id.
    pub who: &'a dyn Fn(u32) -> String,
    /// The weapon on a station, by name.
    pub store: &'a dyn Fn(StationId) -> Option<String>,
}

impl Thought<'_> {
    /// The controller record describes the actor's latest step.
    fn fresh(&self) -> bool {
        self.controller.path == StepPath::Decided
            && self.controller.tick.is_some()
            && self.controller.tick == self.actor.tick
    }

    /// A mission clock time for an AI quarter-second count.
    fn at_quarter(&self, quarter: u64) -> String {
        let ai_now = self.actor.tick.unwrap_or(self.tick);
        let tick = (quarter * QUARTER_TICKS) as i64 + self.tick as i64 - ai_now as i64;
        clock(tick.max(0) as u64)
    }

    fn store_name(&self, station: StationId) -> String {
        match (self.store)(station) {
            Some(name) => format!("{name} on station {}", station.0),
            None => format!("station {}", station.0),
        }
    }
}

fn experience_note(experience: ResolvedExperience) -> &'static str {
    match experience.origin {
        ExperienceOrigin::ExplicitPerObject => "set for this aircraft",
        ExperienceOrigin::EditorAssignment { .. } => "the mission editor's assignment",
        ExperienceOrigin::QuickMission { .. } => "the Quick Mission wing setting",
        ExperienceOrigin::EnemyOverride => "the enemy skill option",
    }
}

fn thought_perception(tree: &mut Tree, t: &Thought) {
    if let Some(lookout) = t.actor.lookout {
        tree.text(
            0,
            "Lookout",
            ["forward", "right", "rear", "left", "above", "below"][lookout.sector],
            "body-relative scan; forward and measured attention also available",
        );
        for sight in &t.actor.visual {
            tree.text(
                1,
                &(t.who)(sight.id),
                format!("{:.1} nm", sight.distance_ft / 6076.),
                sight.result.label(),
            );
        }
    }
    if !t.actor.observation_sources.is_empty() {
        tree.text(
            0,
            "Current observations",
            "sensor sources",
            "current measured contacts, separate from target permission",
        );
        for (id, sources) in &t.actor.observation_sources {
            let mut names = Vec::new();
            if sources.visual.is_some() {
                names.push("visual");
            }
            if sources.radar.is_some() {
                names.push("radar");
            }
            if sources.infrared.is_some() {
                names.push("infrared");
            }
            if sources.fixture.is_some() {
                names.push("fixture");
            }
            tree.text(1, &(t.who)(*id), names.join(", "), "");
        }
    }
}

/// Ground attack decisions, independent of the air-to-air weapon service.
pub fn strike_thought(
    strike: &tore_sim::ai::strike::Strike,
    store: &dyn Fn(StationId) -> Option<String>,
) -> Vec<Node> {
    let mut tree = Tree::default();
    tree.text(
        0,
        "Ground attack",
        format!("{:?}", strike.phase()),
        "a pass phase is not a weapon release",
    );
    tree.text(1, "Segment", format!("{:?}", strike.segment()), "");
    if let Some(pass) = strike.pass() {
        tree.text(1, "Pass", pass.label(), "");
    }
    if let Some(selection) = strike.selection() {
        tree.text(
            1,
            "Ground target",
            format!("object {:#010x}", selection.target),
            "",
        );
        tree.int(2, "Object id", i64::from(selection.target), "");
        tree.text(
            1,
            "Surface weapon",
            store(selection.station).unwrap_or_else(|| format!("station {}", selection.station.0)),
            "",
        );
        tree.text(2, "Kind", format!("{:?}", selection.kind), "");
        tree.num(
            1,
            "Release gate",
            selection.gate_ft,
            0,
            "ft",
            "around the drawn aim point",
        );
    }
    tree.nodes
}

/// Builds an AI aircraft's thought tree. See docs/REPLAYS.md for the layout.
pub fn ai_thought(t: &Thought) -> Vec<Node> {
    let mut tree = Tree::default();
    thought_identity(&mut tree, t);
    thought_mission(&mut tree, t);
    tree.text(0, node::ACTIVITY, t.activity.label(), t.activity_reason);
    // A clock time rather than a running count, so the line stays the same
    // from sample to sample; a panel shows the time in it from the two.
    tree.text(1, "Since", clock(t.activity_since), "");
    if let Some(cue) = t.actor.fire.cue {
        tree.text(
            0,
            "Incoming fire",
            cue.evidence.label(),
            "no shooter identity granted",
        );
        tree.text(
            1,
            "Observed",
            clock(cue.observed_tick),
            "expires after 2 seconds without new evidence",
        );
        tree.text(
            1,
            "Response",
            if t.actor.fire.selected {
                "jink"
            } else {
                "missile defense takes priority"
            },
            "gun-only danger consumes no devices",
        );
    }

    if let Some(reason) = t.actor.damage.returning {
        let d = t.actor.damage;
        tree.text(
            0,
            "Damage response",
            reason.label(),
            if d.ground_hold {
                "holding on the ground"
            } else {
                "offensive engagement ended"
            },
        );
        tree.text(
            1,
            "Recovery",
            if d.runway_known {
                "landing at a known runway"
            } else if d.home_known {
                "home position only; no runway known"
            } else {
                "no destination known; holding heading"
            },
            "defense and terrain avoidance remain available",
        );
        tree.text(
            1,
            "Power policy",
            match d.response.power {
                tore_sim::ai::damage::Power::Normal => "military power as needed",
                tore_sim::ai::damage::Power::Protect => "protect engine: at most 25%",
                tore_sim::ai::damage::Power::RestartIdle => "flameout: low throttle to arm restart",
                tore_sim::ai::damage::Power::RestartRaise => "restart: raise throttle to 50%",
            },
            if d.safety_power {
                "flight safety permits power above the protective cap"
            } else {
                "component fault response"
            },
        );
        if let Some(throttle) = d.throttle_requested {
            tree.text(
                1,
                "Throttle requested",
                format!("{:.0}%", throttle * 100.),
                "request before mechanical limits",
            );
        }
        if let Some(throttle) = d.throttle_locked {
            tree.text(
                1,
                "Throttle jammed",
                format!("{:.0}%", throttle * 100.),
                "the pilot cannot move the lever",
            );
        }
        tree.text(
            1,
            "Maneuver limits",
            format!("30 deg bank, {:.2} G", d.response.g_cap.unwrap_or(2.5)),
            "existing aircraft limits still apply",
        );
    }
    let fresh = t.fresh();
    thought_target(&mut tree, t, fresh);
    if fresh {
        thought_geometry(&mut tree, t);
    }
    match &t.controller.weapons {
        Some(weapons) if fresh => thought_weapon(&mut tree, t, weapons),
        _ => {
            tree.text(0, "Weapon", "none", "no target this tick");
            tree.text(1, "Phase", phase_label(t.weapon_phase), "");
        }
    }
    if fresh && let Some(gun) = t.controller.gun {
        tree.text(
            0,
            "Gunnery",
            if gun.requested_round {
                "round requested"
            } else {
                "waiting"
            },
            "fixed barrel; measured target motion only",
        );
        tree.text(
            1,
            "Ammunition",
            format!("{:?}", gun.view.ammunition),
            "before this tick's release; debit only emitted rounds",
        );
        if let Some(solution) = gun.view.solution {
            tree.text(
                1,
                "Barrel",
                if solution.aligned {
                    "on predicted volume"
                } else {
                    "not aligned"
                },
                "actual bullets use the shared dispersion",
            );
            tree.text(
                1,
                "Predicted miss",
                format!("{:.1} ft", solution.miss_ft),
                "before dispersion",
            );
            tree.text(
                1,
                "Flight time",
                format!("{:.2} s", solution.seconds),
                "shared projectile trajectory",
            );
        }
        tree.text(
            1,
            "Cycle",
            phase_label(gun.phase),
            "0.5 s burst, 0.5 s recovery; cadence retained",
        );
        if let Some(due) = gun.deadline {
            tree.text(1, "Cycle ends", clock(due), "simulation time");
        }
    }
    thought_defense(&mut tree, t);
    thought_motion(&mut tree, t, fresh);
    thought_steering(&mut tree, t);
    thought_perception(&mut tree, t);
    thought_fuel(&mut tree, t, fresh);
    thought_airfield(&mut tree, t);
    thought_ejection(&mut tree, t);
    let mut rules: Vec<Fallback> = t.fallbacks.to_vec();
    if let Some(adapter) = t.actor.fly.as_ref().and_then(|f| f.adapter.as_ref()) {
        for f in &adapter.fallbacks {
            if !rules.contains(f) {
                rules.push(*f);
            }
        }
    }
    if !rules.is_empty() {
        tree.text(
            0,
            "Fitted rules",
            fallback_list(&rules),
            "stand-ins for rules not yet recovered",
        );
    }
    if !t.recent.is_empty() {
        tree.int(0, "Recent", t.recent.len() as i64, "");
        for change in t.recent {
            tree.text(1, &clock(change.tick), change.what.as_str(), &change.why);
        }
    }
    tree.nodes
}

fn thought_identity(tree: &mut Tree, t: &Thought) {
    let mut note = t.name.to_owned();
    if let Some(e) = t.experience {
        note.push_str(&format!(", {:?} ({})", e.level, experience_note(e)));
    }
    if t.wing > 0 {
        note.push_str(&format!(", {} wing {}-{}", t.side, t.wing, t.member));
    } else if !t.side.is_empty() {
        note.push_str(&format!(", {}", t.side));
    }
    note.push_str(if t.leader { ", leader" } else { ", wingman" });
    tree.text(0, "Aircraft", t.label, &note);
}

fn thought_mission(tree: &mut Tree, t: &Thought) {
    let Some(engagement) = &t.actor.engagement else {
        if t.neutral {
            tree.text(
                0,
                "Mission",
                "holding formation",
                "until the leader releases the wing or an attack is seen",
            );
        }
        return;
    };
    tree.text(
        0,
        "Mission",
        role_label(engagement.role),
        &format!("stance: {}", stance_label(engagement.stance)),
    );
    if engagement.neutral || t.neutral {
        tree.flag(
            1,
            "Holding formation",
            true,
            "only self-defense until the leader releases the wing",
        );
    }
    if engagement.must_rejoin {
        tree.flag(1, "Must rejoin", true, "an escort beyond its leash");
    }
    for report in &engagement.reports {
        let attack = report.attack;
        let by = attack
            .report
            .attacker_id
            .map_or_else(|| "an unknown attacker".to_owned(), |id| (t.who)(id));
        let label = format!("Attack on {} by {by}", (t.who)(attack.report.defended_id));
        let note = match report.ignored {
            None => "acted on".to_owned(),
            Some(reason) => format!("ignored: {}", ignore_reason(reason)),
        };
        tree.text(1, &label, clock_or_seen(attack.observed_tick, t), &note);
    }
}

/// When an attack was seen, on the recording's clock.
fn clock_or_seen(observed_tick: u64, t: &Thought) -> String {
    let ai_now = t.actor.tick.unwrap_or(t.tick);
    let tick = observed_tick as i64 + t.tick as i64 - ai_now as i64;
    format!("seen at {}", clock(tick.max(0) as u64))
}

/// Why an attack report was not acted on.
pub fn ignore_reason(reason: tore_sim::ai::thought::IgnoreReason) -> String {
    use tore_sim::ai::thought::IgnoreReason;
    match reason {
        IgnoreReason::NotTheDefendedAircraft => {
            "the report is about another aircraft than its reporter".into()
        }
        IgnoreReason::UnknownReporter => "the reporter is not a known aircraft".into(),
        IgnoreReason::RecipientGone => "the recipient left the mission".into(),
        IgnoreReason::SeenBeforeRecall { recalled_at } => {
            format!("seen before the recall to formation at mission tick {recalled_at}")
        }
        IgnoreReason::KnownAtRecall { event_id } => {
            format!("shot {event_id} was already known at the recall")
        }
    }
}

fn thought_target(tree: &mut Tree, t: &Thought, fresh: bool) {
    let note = match t.target {
        Some(id) => (t.who)(id),
        None => "none".into(),
    };
    tree.id(0, node::TARGET, t.target, &note);
    if fresh && let Some((how, why)) = target_path(t.controller.target.path, t.who) {
        tree.text(1, "Chosen by", how, &why);
    }
    if !fresh {
        return;
    }
    if let Some(lost) = t.controller.events.lost_targets.first() {
        tree.id(
            1,
            "Lost",
            Some(*lost),
            "left the world or became unavailable",
        );
    }
    let Some(engagement) = &t.actor.engagement else {
        return;
    };
    let e = &engagement.explanation;
    let rank = ranking(e, t.target);
    if let Some(score) = rank.score_ft {
        let note = match rank.priority {
            Some(p) => format!("priority {}; lowest score wins", priority_label(p)),
            None => "lowest score wins".to_owned(),
        };
        tree.num(1, "Score", score, 0, unit::FT, &note);
    }
    if let Some((id, priority, score)) = rank.runner_up {
        let note = format!(
            "{}: {}",
            (t.who)(id),
            priority.map_or("no priority", priority_label)
        );
        tree.num(1, "Runner-up", score, 0, unit::FT, &note);
    }
    let mut flags = Vec::new();
    if e.weapons_hold {
        flags.push("weapons hold");
    }
    if e.own_defense_only {
        flags.push("only its own attackers");
    }
    if e.escort_outside_leash {
        flags.push("escort outside its leash");
    }
    if e.kept_current {
        flags.push("current target kept at the best priority");
    }
    if !flags.is_empty() {
        tree.text(1, "Rules", flags.join(", "), "");
    }
    if !e.own_attackers.is_empty() {
        tree.add(
            1,
            "Attacked by",
            Value::Ids(e.own_attackers.clone()),
            "",
            "identified attackers of this aircraft",
        );
    }
    if !e.protected_attackers.is_empty() {
        tree.add(
            1,
            "Protected attacked by",
            Value::Ids(e.protected_attackers.clone()),
            "",
            "identified attackers of the aircraft it protects",
        );
    }
    // Who could not be chosen at all, and why: the answer to "why is it
    // ignoring that aircraft".
    let excluded: Vec<&CandidateExplanation> = e
        .candidates
        .iter()
        .filter(|c| c.ineligible.any())
        .take(3)
        .collect();
    if !excluded.is_empty() {
        tree.int(
            1,
            "Not eligible",
            e.candidates.iter().filter(|c| c.ineligible.any()).count() as i64,
            &format!("of {} observed", e.candidates.len() + e.omitted),
        );
        for c in excluded {
            tree.text(2, &(t.who)(c.id), candidate_note(c), "");
        }
    }
}

fn thought_geometry(tree: &mut Tree, t: &Thought) {
    let Some(g) = t.controller.geometry else {
        return;
    };
    tree.num(
        0,
        "Geometry",
        g.spatial_distance_feet / FT_PER_NM,
        1,
        unit::NM,
        "range to the target",
    );
    match g.angles {
        Some(a) => tree.num(
            1,
            "Off nose",
            a.off_beam_deg,
            0,
            unit::DEG,
            match (a.ahead, a.facing) {
                (true, true) => "ahead, facing us",
                (true, false) => "ahead, facing away",
                (false, true) => "behind, facing us",
                (false, false) => "behind, facing away",
            },
        ),
        None => tree.text(
            1,
            "Off nose",
            "undefined",
            "the target is straight above or below",
        ),
    }
    if let Some(r) = t.relative {
        tree.num(
            1,
            "Aspect",
            r.aspect_deg,
            0,
            unit::DEG,
            "0 dead astern, 180 head on",
        );
        tree.num(1, "Closure", r.closure_kt, 0, unit::KT, "");
        // Tens of feet: a panel reads "2,300 ft below".
        tree.num(
            1,
            "Height",
            round(r.height_ft, -1),
            0,
            unit::FT,
            if r.height_ft >= 0. {
                "target above"
            } else {
                "target below"
            },
        );
    }
}

/// Why the weapon did not fire this tick, in words; `None` when it fired.
/// `views` are the stores' views of the target (the actor record's), and
/// `at` turns an AI quarter-second count into a mission clock time.
pub fn fire_reason(
    w: &WeaponTrace,
    target: Option<u32>,
    views: &[StationView],
    at: &dyn Fn(u64) -> String,
) -> Option<String> {
    if matches!(w.outcome, Some(ServiceOutcome::Fire(_))) {
        return None;
    }
    if target.is_none() || !w.lock.has_target {
        return Some("no target".into());
    }
    if w.chosen.is_none() {
        // The store closest to usable: one outside its envelope, with every
        // number, otherwise the first store's filter.
        let outside = w
            .stations
            .iter()
            .find(|s| matches!(s.verdict, Some(StationVerdict::OutsideEnvelope)));
        if let Some(s) = outside
            && let Some(check) = s.employment
            && let Some(failure) = check.first_failure()
        {
            let view = views.iter().find(|v| v.station == s.station);
            return Some(failure_text(failure, &check, view));
        }
        return Some(match w.stations.iter().find_map(|s| s.verdict) {
            Some(verdict) => format!("no usable store: {}", verdict_label(verdict)),
            None => "no store".into(),
        });
    }
    let lock = w.lock;
    if !lock.locked {
        let why = if !lock.has_station {
            "no store"
        } else if lock.terrain_blocked {
            "terrain blocks the line of fire"
        } else {
            match lock.target_ahead {
                Some(false) => "the target is behind",
                None => "the target's bearing is undefined",
                Some(true) => "the lock check failed",
            }
        };
        return Some(format!("no lock: {why}"));
    }
    Some(match &w.outcome {
        Some(outcome) => service_text(outcome, at),
        None => "the weapon service stopped on an unresolved rule".into(),
    })
}

fn thought_weapon(tree: &mut Tree, t: &Thought, w: &WeaponTrace) {
    let chosen = w
        .chosen
        .map_or_else(|| "none".to_owned(), |s| t.store_name(s));
    tree.text(
        0,
        "Weapon",
        chosen,
        match w.class {
            tore_sim::ai::weapon_service::TargetClass::Air => "air target",
            tore_sim::ai::weapon_service::TargetClass::Surface => "surface target",
        },
    );
    let note = if w.phase_before == w.phase_after {
        String::new()
    } else {
        format!("was {}", phase_label(w.phase_before))
    };
    tree.text(1, "Phase", phase_label(w.phase_after), &note);
    let at = |q| t.at_quarter(q);
    let why = fire_reason(w, t.target, &t.actor.stations, &at);
    match &why {
        None => tree.text(1, "Fire?", "fired", ""),
        Some(why) => tree.text(1, "Fire?", "not yet", why),
    }
    if let Some(outcome) = &w.outcome {
        let service = service_text(outcome, &at);
        if why.as_ref() != Some(&service) && why.is_some() {
            tree.text(
                1,
                "Service",
                service,
                if w.burst_pacing_restart {
                    "burst pacing restarted the service (fitted)"
                } else {
                    ""
                },
            );
        }
    }
    // Every store against this target, stores alike grouped: `AIM-120:
    // outside its envelope (stations 2, 3; too far)`.
    let mut lines: Vec<(String, String, Vec<u8>, String)> = Vec::new();
    for s in &w.stations {
        let name = (t.store)(s.station).unwrap_or_else(|| "store".to_owned());
        let (verdict, words) = match s.verdict {
            None => ("not examined", "ten usable stores came first".to_owned()),
            Some(StationVerdict::OutsideEnvelope) => (
                "outside its envelope",
                s.employment
                    .map(|c| {
                        c.failures()
                            .map(failure_words)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default(),
            ),
            Some(v) => (verdict_label(v), String::new()),
        };
        match lines
            .iter_mut()
            .find(|(n, v, _, w)| *n == name && v == verdict && *w == words)
        {
            Some(line) => line.2.push(s.station.0),
            None => lines.push((name, verdict.to_owned(), vec![s.station.0], words)),
        }
    }
    for (name, verdict, stations, words) in lines {
        let list = stations
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let mut note = format!(
            "{} {list}",
            if stations.len() == 1 {
                "station"
            } else {
                "stations"
            }
        );
        if !words.is_empty() {
            note.push_str(&format!("; {words}"));
        }
        tree.text(2, &name, verdict, &note);
    }
}

fn thought_defense(tree: &mut Tree, t: &Thought) {
    let Some(d) = t.defense else {
        return;
    };
    let threat = t.threat.clone().unwrap_or_default();
    let mut note = match (&threat.weapon, threat.shooter) {
        (Some(w), Some(s)) => format!("{w} from {}", (t.who)(s)),
        (Some(w), None) => w.clone(),
        (None, Some(s)) => format!("from {}", (t.who)(s)),
        (None, None) => String::new(),
    };
    if d.debug.new_threat {
        if !note.is_empty() {
            note.push_str(", ");
        }
        note.push_str("new this tick");
    }
    tree.add(0, "Defense", Value::Id(d.threat_id), "", &note);
    if let Some(range) = threat.range_ft {
        tree.num(1, "Range", range / FT_PER_NM, 1, unit::NM, "");
    }
    let (maneuver, maneuver_note) = defense_maneuver(&d);
    tree.text(1, "Maneuver", maneuver, &maneuver_note);
    tree.text(
        1,
        "Countermeasures",
        burst_text(d.burst),
        if d.burst.is_some() {
            "requested now"
        } else if d.debug.release_now && !d.debug.may_burst {
            "waiting for cooldown or encounter device budget exhausted"
        } else {
            ""
        },
    );
    match d.debug.estimated_threat_time_s {
        Some(time) => tree.num(1, "Time to impact", time, 1, unit::S, "estimated"),
        None => tree.text(1, "Time to impact", "unknown", "no closing estimate"),
    }
    tree.num(
        1,
        "Maneuver time",
        d.debug.estimated_maneuver_time_s,
        1,
        unit::S,
        "",
    );
    tree.num(
        1,
        "Break threshold",
        d.debug.margin_s,
        1,
        unit::S,
        "by skill",
    );
    let why = defense_reasons(&d, t.experience);
    if !why.is_empty() {
        tree.text(1, "Why now", why, "");
    }
}

/// The defensive maneuver, and its heading and pitch.
pub fn defense_maneuver(d: &DefenseDecision) -> (&'static str, String) {
    let kind = |m: defense::Maneuver| match m {
        defense::Maneuver::Jink => "jink",
        defense::Maneuver::Notch => "notch",
        defense::Maneuver::Drag => "drag",
        defense::Maneuver::Beam => "beam",
        defense::Maneuver::Break => "break",
    };
    match d.motion {
        Some(m) => (
            kind(m.maneuver),
            format!(
                "heading {:.0}, pitch {:.0}{}",
                m.heading_deg,
                m.flight_path_pitch_deg,
                if d.debug.dive_safe {
                    " (dive safe)"
                } else {
                    ""
                }
            ),
        ),
        None if d.debug.passed => (
            "complete",
            "missile passage confirmed by opening range".into(),
        ),
        None => (
            "not yet",
            format!(
                "would {} to heading {:.0}",
                kind(d.debug.preferred),
                d.debug.heading_deg
            ),
        ),
    }
}

/// `chaff x2`, `flares x2`, `chaff x2, flares x2` or `none`.
pub fn burst_text(burst: Option<defense::BurstRequest>) -> String {
    let Some(b) = burst else {
        return "none".into();
    };
    let mut parts = Vec::new();
    if b.chaff > 0 {
        parts.push(format!("chaff x{}", b.chaff));
    }
    if b.flares > 0 {
        parts.push(format!("flares x{}", b.flares));
    }
    if parts.is_empty() {
        "none".into()
    } else {
        parts.join(", ")
    }
}

/// Every reason behind maneuvering or releasing now.
pub fn defense_reasons(d: &DefenseDecision, _experience: Option<ResolvedExperience>) -> String {
    let g = d.debug;
    let mut why = Vec::new();
    if g.passed {
        why.push("missile passage confirmed by opening range");
    } else if !g.maneuver_now {
        why.push("pilot reaction delay");
    }
    if g.bearing_only {
        why.push("only a bearing is known");
    }
    if g.uncertain_directed_radar {
        why.push("a radar is directed at it with no closing estimate");
    }
    if g.insufficient_time {
        why.push("too little time to maneuver");
    }
    if g.stale {
        why.push("the evidence is stale");
    }
    if g.maneuver_now && why.is_empty() {
        why.push("reaction complete, committed defense phase");
    }
    why.join(", ")
}

fn thought_motion(tree: &mut Tree, t: &Thought, fresh: bool) {
    if fresh {
        let (branch, note) = motion_branch(&t.controller.motion.branch, t.actor, t.who);
        tree.text(0, "Motion", branch, &note);
        if let MotionBranch::Formation(f) = t.controller.motion.branch {
            let d = (0..3)
                .map(|i| (f.slot_point[i] - t.own.position[i]).powi(2))
                .sum::<f64>()
                .sqrt();
            tree.num(1, "Slot distance", round(d, -1), 0, unit::FT, "");
            tree.num(
                1,
                "Formation speed",
                knots(f.speed_fps),
                0,
                unit::KT,
                if f.burner { "afterburner" } else { "" },
            );
        }
    } else {
        let why = match t.actor.path {
            ActorPath::Airfield => "a takeoff or landing sequence flies it",
            ActorPath::Destroyed => "destroyed",
            ActorPath::Dummy => "a training target flying straight",
            ActorPath::NotRun => "not stepped yet",
            ActorPath::Controller => "the decision did not run this tick",
            ActorPath::DamageHold => "holding on the ground because of system damage",
        };
        tree.text(0, "Motion", "not deciding", why);
    }
    if let Some(choice) = t.choice {
        let m = &choice.maneuver;
        let label = m
            .choice
            .as_ref()
            .map_or_else(|| maneuver_path(&m.path), choice_label);
        let path = maneuver_path(&m.path);
        let different = path != label;
        tree.text(1, "Choice", label, &draws_text(&choice.draws));
        tree.text(2, "Chosen at", clock(choice.tick), "");
        if different {
            tree.text(2, "Path", path, "");
        }
        if let Some(q) = m.quadrant {
            let note = m
                .thresholds
                .map(|th| {
                    format!(
                        "best attack {}%, random tactic {}%",
                        th.best_attack_percent, th.random_tactic_percent
                    )
                })
                .unwrap_or_default();
            tree.text(2, "Situation", quadrant_label(q), &note);
        }
        if let Some(f) = m.fallback {
            tree.text(2, "Fitted rule", f.name(), f.rule());
        }
        if let Some(c) = m.last_ditch {
            tree.text(2, "Last ditch", format!("{c:?}"), "");
        }
        if let Some(r) = &choice.resolve {
            if let PitchSource::Engagement { pitch_deg, .. } = r.pitch {
                tree.num(
                    2,
                    "Pitch",
                    pitch_deg,
                    1,
                    unit::DEG,
                    "fitted engagement pitch",
                );
            }
            if let Some(p) = r.pursuit {
                tree.text(
                    2,
                    "Pursuit",
                    format!("{:?}", p.condition),
                    &format!(
                        "offset {} lateral, {} longitudinal, {} vertical ft for {} s",
                        p.offsets.lateral_ft,
                        p.offsets.longitudinal_ft,
                        p.offsets.vertical_ft,
                        p.offsets.nominal_duration_s
                    ),
                );
            }
        }
    }
    if fresh && let Some(next) = t.controller.motion.next_choice_at {
        tree.text(1, "Next choice", t.at_quarter(next), "");
    }
    // Draws this tick that belong to no maneuver choice (countermeasure
    // rolls, formation variation, break sides).
    let own_choice = t.choice.is_some_and(|c| c.tick == t.tick);
    if fresh && !own_choice && !t.draws.is_empty() {
        tree.text(1, "Rolls", draws_text(t.draws), "this tick");
    }
}

/// Every draw, in order: `draw 37 of 0..99: offset; roll 12 < 50: best
/// attack`.
pub fn draws_text(draws: &[Draw]) -> String {
    draws.iter().map(draw_text).collect::<Vec<_>>().join("; ")
}

fn thought_steering(tree: &mut Tree, t: &Thought) {
    let Some(fly) = &t.actor.fly else {
        return;
    };
    let achieved = fly.achieved;
    tree.head(0, "Steering", "asked by the maneuver, and delivered");
    let intent = fly.intent;
    let axis = |tree: &mut Tree, label: &str, delivered: f64, asked: Option<f64>| {
        tree.num(1, label, delivered, 0, unit::DEG, "delivered");
        if let Some(a) = asked {
            tree.num(2, "Asked", a, 0, unit::DEG, "");
        }
    };
    axis(
        tree,
        "Heading",
        achieved.heading_deg,
        intent.map(|i| i.heading_deg),
    );
    axis(
        tree,
        "Pitch",
        achieved.flight_path_pitch_deg,
        intent.map(|i| i.flight_path_pitch_deg),
    );
    axis(
        tree,
        "Bank",
        achieved.bank_deg,
        intent.and_then(|i| match i.bank {
            tore_sim::ai::motion::Bank::Explicit(b) => Some(f64::from(b)),
            tore_sim::ai::motion::Bank::Unconstrained => None,
        }),
    );
    tree.num(
        1,
        "Speed",
        knots(achieved.speed_fps),
        0,
        unit::KT,
        "delivered",
    );
    if let Some(i) = intent {
        tree.num(2, "Asked", knots(i.speed.0), 0, unit::KT, "");
    }
    let at_limit = t.own.g_limit.is_some_and(|l| t.own.g >= l * 0.98);
    tree.num(
        1,
        "G",
        t.own.g,
        1,
        unit::G,
        if at_limit {
            "at the limit"
        } else {
            "delivered"
        },
    );
    if let Some(limit) = t.own.g_limit {
        tree.num(2, "Limit", limit, 1, unit::G, "");
    }
    if let Some(floor) = fly.terrain_floor_deg.filter(|f| *f > -89.5) {
        tree.num(
            1,
            "Terrain floor",
            floor,
            0,
            unit::DEG,
            "lowest pitch allowed",
        );
    }
    if let Some(adapter) = &fly.adapter {
        let input = &adapter.input;
        tree.head(0, "Controls", "");
        tree.num(1, "Pitch", input.pitch, 2, "", "");
        tree.num(1, "Roll", input.roll, 2, "", "");
        tree.num(1, "Rudder", input.yaw, 2, "", "");
        if let Some(throttle) = input.throttle {
            tree.num(1, "Throttle", throttle, 2, "", "");
        }
        tree.flag(1, "Afterburner", t.own.afterburner, "");
    }
}

fn thought_fuel(tree: &mut Tree, t: &Thought, fresh: bool) {
    let fuel = if fresh { t.controller.fuel } else { None };
    let note = fuel
        .and_then(|f| f.state)
        .map(|s| format!("state {}", fuel_label(s)))
        .unwrap_or_default();
    tree.num(0, "Fuel", t.own.fuel_lb, 0, unit::LB, &note);
    if let Some(f) = fuel {
        if f.endurance_s.is_finite() {
            tree.num(1, "Endurance", round(f.endurance_s, -1), 0, unit::S, "");
        }
        if let Some(home) = f.time_home_s.filter(|v| v.is_finite()) {
            tree.num(1, "Time home", round(home, -1), 0, unit::S, "at cruise");
        }
        if f.recovering {
            tree.flag(1, "Heading home", true, "bingo or critical fuel");
        }
    }
}

fn thought_airfield(tree: &mut Tree, t: &Thought) {
    let c = t.actor.clearance;
    let sequence = t.actor.airfield;
    if t.airfield.is_none() && sequence.is_none() {
        return;
    }
    let phase = t.airfield.map_or("left the sequence", airfield_label);
    let mut gates = Vec::new();
    gates.push(if c.turn { "turn ok" } else { "not its turn" });
    gates.push(if c.runway_free {
        "runway free"
    } else {
        "runway busy"
    });
    if !c.wing_landed {
        gates.push("wing members still landing");
    }
    if c.leader_landing {
        gates.push("leader landing");
    }
    tree.text(0, "Airfield", phase, &gates.join(", "));
    if let Some(slot) = c.free_slot {
        tree.int(1, "Parking slot", i64::from(slot), "");
    }
    if let Some(s) = sequence {
        if s.began_landing {
            tree.flag(1, "Began landing", true, "");
        }
        if let Some(exit) = s.left {
            let why = match exit {
                AirfieldExit::WingAbort => "its leader is neither landing nor on the ground".into(),
                AirfieldExit::MissileThreat { defending, warned } => format!(
                    "a missile threat{}{}",
                    if defending { ", defending" } else { "" },
                    if warned { ", warned" } else { "" }
                ),
            };
            tree.text(1, "Left", "free flight", &why);
        }
        if let Some(step) = s.step
            && step.go_around
        {
            tree.flag(1, "Go-around", true, "this tick abandoned the final");
        }
    }
}

/// A hazard in words: `diving toward the ground, impact in 3.2 s`.
pub fn hazard_text(assessment: &tore_sim::ejection::Assessment) -> String {
    let what = match assessment.hazard {
        tore_sim::ejection::Hazard::Destroyed => "the aircraft is destroyed",
        tore_sim::ejection::Hazard::Dive => "a dive it cannot pull out of",
        tore_sim::ejection::Hazard::Lift => "not enough lift to stay up",
        tore_sim::ejection::Hazard::Fire => "uncontained fire will destroy the aircraft",
    };
    if assessment.impact_seconds.is_finite() {
        let end = if assessment.hazard == tore_sim::ejection::Hazard::Fire {
            "fatal fire in"
        } else {
            "impact in"
        };
        format!("{what}, {end} {:.1} s", assessment.impact_seconds)
    } else {
        what.into()
    }
}

fn thought_ejection(tree: &mut Tree, t: &Thought) {
    let Some(e) = t.actor.ejection else {
        return;
    };
    if e.escape_running {
        tree.text(0, "Ejection", "ejected", "the pilot's escape is running");
        return;
    }
    let Some(a) = e.assessment else {
        return;
    };
    let decision = if e.ejected {
        "ejected"
    } else if e.go_around == Some(true) {
        "go-around instead"
    } else {
        "watching"
    };
    tree.text(0, "Ejection", decision, &hazard_text(&a));
    if let Some(phase) = e.guarded_phase {
        tree.text(
            1,
            "Guarded by",
            airfield_label(phase),
            "only a catastrophe ejects",
        );
    }
    if let Some(c) = e.catastrophic {
        tree.flag(1, "Catastrophic", c, "");
    }
}

// ---------------------------------------------------------------------------
// Flight-model effects

/// One flight-model effect in words: a label, the value it applied (a
/// factor, a limit or a state), the value as text for events, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectLine {
    pub label: String,
    pub value: Value,
    pub unit: &'static str,
    /// The value for people: `x0.72`, `limit 6.8 G`, `engine off`.
    pub factor: String,
    /// Why it applied. Numbers here are rounded coarsely, so the words stay
    /// the same from tick to tick while the cause holds.
    pub because: String,
}

fn ratio(label: &str, v: f64, because: String) -> EffectLine {
    EffectLine {
        label: label.into(),
        value: Value::Num(round(v, 3)),
        unit: unit::RATIO,
        factor: format!("x{}", fixed(v, 2)),
        because,
    }
}

fn limit(label: &str, g: f64, because: String) -> EffectLine {
    EffectLine {
        label: label.into(),
        value: Value::Num(round(g, 2)),
        unit: unit::G,
        factor: format!("limit {} G", fixed(g, 1)),
        because,
    }
}

fn state(label: &str, text: &str, because: String) -> EffectLine {
    EffectLine {
        label: label.into(),
        value: Value::Text(text.into()),
        unit: "",
        factor: text.into(),
        because,
    }
}

fn percent(v: f64) -> String {
    format!("{:.0}%", v * 100.)
}

fn kt_text(fps: f64) -> String {
    format!("{:.0} kt", knots(fps))
}

fn device_name(device: DeviceKind) -> &'static str {
    match device {
        DeviceKind::Gear => "Gear",
        DeviceKind::Flaps => "Flaps",
        DeviceKind::Airbrake => "Airbrake",
        DeviceKind::Hook => "Hook",
    }
}

/// A key that tells two effects apart for change detection: the variant,
/// and for held devices which device.
pub type EffectKey = (std::mem::Discriminant<Effect>, u8);

pub fn effect_key(effect: &Effect) -> EffectKey {
    let sub = match effect {
        Effect::DeviceHeld { device, .. } => *device as u8,
        _ => 0,
    };
    (std::mem::discriminant(effect), sub)
}

/// Effects that describe one moment rather than a lasting state: they have
/// an "on" event but no "off".
pub fn momentary(effect: &Effect) -> bool {
    matches!(
        effect,
        Effect::AutopilotReleased(_)
            | Effect::SpinCheck(_)
            | Effect::SpinEnded { .. }
            | Effect::DepartureCleared { .. }
            | Effect::LiftOff { .. }
            | Effect::SurfaceDropped
            | Effect::Touchdown(_)
            | Effect::UnsafeTouchdown(_)
            | Effect::LegacyFloor { .. }
            | Effect::BlastKick(_)
            | Effect::BuildingRebound
    )
}

/// The words for one effect the flight model applied.
pub fn effect_line(effect: &Effect) -> EffectLine {
    match effect {
        Effect::NotFlying(path) => state(
            "Not flying",
            "flight model off",
            match path {
                Path::Wreck => "the aircraft is a wreck".into(),
                Path::Stopped(Stop::NativeFault) => "a native research fault stopped it".into(),
                Path::Stopped(Stop::Crashed) => "a fatal system failure this step".into(),
                _ => String::new(),
            },
        ),
        Effect::NativePath => state(
            "Native adapter",
            "internal effects not recorded",
            "the restricted native research adapter flew this step".into(),
        ),
        Effect::Autopilot { mode, .. } => state(
            "Autopilot steering",
            "stick replaced",
            format!("autopilot {mode:?} mode").to_lowercase(),
        ),
        Effect::AutopilotReleased(release) => state(
            "Autopilot released",
            "switched off",
            match release {
                Release::SystemsDamage => "damaged systems or low engine power",
                Release::AirframeDamage => "wing or tail damage",
                Release::NavigationFailed => "navigation failed",
                Release::PilotOverride => "the pilot moved the stick",
                Release::Ground => "on the ground or crashed",
                Release::PilotCollective => "the pilot moved the collective",
                Release::EngineFailure => "engine failure",
                Release::HydraulicsLost => "hydraulics lost",
                Release::TooSlow => "too slow for the mode",
            }
            .into(),
        ),
        Effect::HydraulicsLost => state(
            "Hydraulics lost",
            "control surfaces frozen",
            "hydraulic pressure 0".into(),
        ),
        Effect::ControlResponse(c) => {
            let mut why = Vec::new();
            if c.hydraulic < 1. {
                why.push(format!("hydraulic pressure {}", percent(c.hydraulic)));
            }
            let k = c.condition;
            if k.authority.iter().any(|a| *a < 1.) {
                why.push("damaged control surfaces".into());
            }
            if k.bias.iter().any(|b| *b != 0.) {
                why.push("bent control surfaces".into());
            }
            if k.damaged_linkage {
                why.push("damaged control linkage".into());
            }
            if k.unstable {
                why.push("unstable flight controls".into());
            }
            let scale = c.hydraulic
                * k.authority.iter().copied().fold(1., f64::min)
                * if k.damaged_linkage { 0.3 } else { 1. };
            ratio("Control response", scale, why.join(", "))
        }
        Effect::ThrottleJammed(lock) => EffectLine {
            label: "Throttle jammed".into(),
            value: Value::Num(round(lock.held_at, 2)),
            unit: unit::RATIO,
            factor: format!("held at {}", percent(lock.held_at)),
            because: "the throttle system failed".into(),
        },
        Effect::RegionalDamage(r) => {
            let [left, right, tail] = r.damage;
            ratio(
                "Regional damage",
                r.effects.lift,
                format!(
                    "left wing {}, right wing {}, tail {}",
                    percent(left),
                    percent(right),
                    percent(tail)
                ),
            )
        }
        Effect::RunwayWind(w) => ratio(
            "Runway wind",
            1. - 0.5 * w.fraction,
            format!(
                "{:.0} kt crosswind, {:.0} kt tailwind",
                w.assessment.crosswind_knots, w.assessment.tailwind_knots
            ),
        ),
        Effect::Parked { attitude, position } => state(
            "Parked",
            match (attitude, position) {
                (true, true) => "attitude and position held",
                (true, false) => "attitude held",
                _ => "position held",
            },
            "on the wheels with idle power and no stick".into(),
        ),
        Effect::DeviceHeld { device, state: s } => EffectLine {
            label: format!("{} held", device_name(*device)),
            value: Value::Num(round(s.position, 2)),
            unit: unit::RATIO,
            factor: format!("stuck at {}", percent(s.position)),
            because: match s.blocked {
                Some(Block::NoHydraulics) => "no hydraulic pressure".into(),
                Some(Block::Jammed) => "jammed by damage".into(),
                None => String::new(),
            },
        },
        Effect::FuelStarved => state(
            "Fuel starved",
            "engine and afterburner off",
            "internal and external tanks empty".into(),
        ),
        Effect::EngineOff { power_available } => state(
            "Engine off",
            "no thrust",
            if *power_available <= 0. {
                "failed or flamed out".into()
            } else {
                "switched off".into()
            },
        ),
        Effect::EnginePowerReduced { power_available } => ratio(
            "Engine power reduced",
            *power_available,
            "engine damage".into(),
        ),
        Effect::AfterburnerBlocked {
            cause,
            afterburner_throttle,
            ..
        } => state(
            "Afterburner blocked",
            "no afterburner",
            match cause {
                BurnerBlock::EngineOff => "the engine is off".into(),
                BurnerBlock::Failed => "the afterburner failed".into(),
                BurnerBlock::NoPower => "no engine power".into(),
                BurnerBlock::NotFitted => "this aircraft has no afterburner".into(),
                BurnerBlock::NoFuel => "no fuel".into(),
                BurnerBlock::Vectoring => "nozzles in powered-lift position".into(),
                BurnerBlock::ThrottleLow => {
                    format!("throttle at or below {}", percent(*afterburner_throttle))
                }
            },
        ),
        Effect::UnlimitedFuel {
            fuel_flow_lbs_per_second,
        } => EffectLine {
            label: "Unlimited fuel".into(),
            value: Value::Num(round(*fuel_flow_lbs_per_second, 2)),
            unit: "lb/s",
            factor: "burn not taken from the tanks".into(),
            because: "the Unlimited fuel option".into(),
        },
        Effect::IgnoreWeaponWeights {
            carried_lbs,
            payload_lbs,
        } => EffectLine {
            label: "Ignore weapon weights".into(),
            value: Value::Num(round(*carried_lbs, 0)),
            unit: unit::LB,
            factor: format!(
                "carried {} of {} lb",
                thousands(*carried_lbs),
                thousands(*payload_lbs)
            ),
            because: "the Ignore weapon weights option".into(),
        },
        Effect::NoEnvelopeAtAltitude { altitude_ft } => state(
            "No 1 G envelope",
            "stall 900 ft/s, top 1,000 ft/s",
            format!("no 1 G envelope covers {} ft", thousands(*altitude_ft)),
        ),
        Effect::FlapStallSpeed {
            clean_stall_fps,
            stall_fps,
            flaps,
        } => EffectLine {
            label: "Flap stall speed".into(),
            value: Value::Num(round(knots(*stall_fps), 0)),
            unit: unit::KT,
            factor: format!(
                "stall {} (clean {})",
                kt_text(*stall_fps),
                kt_text(*clean_stall_fps)
            ),
            because: format!("flaps {}", percent(*flaps)),
        },
        Effect::LowSpeedAuthority {
            authority,
            stall_fps,
            ..
        } => ratio(
            "Low-speed authority",
            *authority,
            format!("airspeed below the stall speed of {}", kt_text(*stall_fps)),
        ),
        Effect::OutsideEnvelope { .. } => state(
            "Outside the envelope",
            "G limits +/-1 before loading",
            "no envelope row holds this speed at this height".into(),
        ),
        Effect::FastSideHold { hold, .. } => limit(
            "Fast-side G hold",
            hold.g,
            format!(
                "past the {} G row's fast edge at {}, the pull stays",
                fixed(hold.g, 0),
                kt_text(hold.edge_fps)
            ),
        ),
        Effect::LoadedLimits { divisor, loading } => EffectLine {
            label: "Loaded G limits".into(),
            value: Value::Num(round(*divisor, 3)),
            unit: unit::RATIO,
            factor: format!("limits / {}", fixed(*divisor, 2)),
            because: format!(
                "fuel and stores {} of empty weight",
                percent((*loading * 20.).round() / 20.)
            ),
        },
        Effect::ExtraG { from_g, limit_g } => limit(
            "Pull extra G",
            *limit_g,
            format!("the Pull extra G option (was {} G)", fixed(*from_g, 1)),
        ),
        Effect::LowSpeedCeiling {
            ceiling,
            positive_limit_g,
            ..
        } => limit(
            "Low-speed G ceiling",
            *positive_limit_g,
            format!(
                "speed near the stall speed, on the ramp from {} to {}",
                kt_text(ceiling.from_fps),
                kt_text(ceiling.to_fps)
            ),
        ),
        Effect::FlapLift { factor, flaps, .. } => {
            ratio("Flap lift", *factor, format!("flaps {}", percent(*flaps)))
        }
        Effect::WingDamaged => ratio("Wing damaged", 0.5, "the wing system failed".into()),
        Effect::SpinLiftLoss { factor, .. } => {
            ratio("Spin lift loss", *factor, "spin rotation".into())
        }
        Effect::StallScaling {
            severity_f8,
            controls,
            lift,
        } => EffectLine {
            label: "Stall scaling".into(),
            value: Value::Num(round(*lift, 3)),
            unit: unit::RATIO,
            factor: format!(
                "lift x{}, controls x{}",
                fixed(*lift, 2),
                fixed(controls.iter().copied().fold(1., f64::min), 2)
            ),
            because: format!(
                "stalled, severity {}",
                fixed(f64::from(*severity_f8) / 256., 2)
            ),
        },
        Effect::SpinControls { effectiveness, .. } => ratio(
            "Spin control loss",
            *effectiveness,
            "rotation in a spin".into(),
        ),
        Effect::SpinCheck(c) => state(
            "Spin direction rule",
            if c.entered { "spin entered" } else { "no spin" },
            format!(
                "stalled with back stick; direction {}{}",
                if c.direction >= 0 { "right" } else { "left" },
                match c.coin {
                    Some(_) => " by a 50% draw (wings level, no roll)",
                    None => " from roll rate and bank",
                }
            ),
        ),
        Effect::SpinEnded { exit, mode } => state(
            "Spin ended",
            &format!("mode {mode:?}").to_lowercase(),
            match exit {
                SpinExit::Recovered => "rotation slowed and the airflow reattached".into(),
                SpinExit::SpinsDisabled => "the No spins option".into(),
            },
        ),
        Effect::DepartureCleared { mode_before } => state(
            "Departure cleared",
            "normal flight",
            format!("wheels on the ground ended {mode_before:?}").to_lowercase(),
        ),
        Effect::LowSpeedTrim { trim_rad, .. } => EffectLine {
            label: "Low-speed trim".into(),
            value: Value::Num(round(trim_rad.to_degrees(), 1)),
            unit: unit::DEG,
            factor: format!("trim {} deg", fixed(trim_rad.to_degrees(), 1)),
            because: "below twice the clean stall speed".into(),
        },
        Effect::GroundSteering { yaw_rate } => EffectLine {
            label: "Ground steering".into(),
            value: Value::Num(round(yaw_rate.to_degrees(), 1)),
            unit: unit::DEG_S,
            factor: format!("yaw {} deg/s", fixed(yaw_rate.to_degrees(), 1)),
            because: "rudder with the wheels on the ground".into(),
        },
        Effect::GearDragOnWheels { .. } => state(
            "Gear drag off",
            "no gear drag",
            "wheels on the ground".into(),
        ),
        Effect::DeviceDragScaled { fraction } => ratio(
            "Device drag scaled",
            *fraction,
            "flap and airbrake drag follow the transonic drag percentage".into(),
        ),
        Effect::DragCapped {
            cap_lbf,
            uncapped_lbf,
        } => EffectLine {
            label: "Drag capped".into(),
            value: Value::Num(round(*cap_lbf, 0)),
            unit: unit::LB,
            factor: format!(
                "{} lb (was {} lb)",
                thousands(*cap_lbf),
                thousands(*uncapped_lbf)
            ),
            because: "drag would stop the aircraft in one step".into(),
        },
        Effect::SpeedCapped { from_fps } => EffectLine {
            label: "Speed capped".into(),
            value: Value::Num(6_000.),
            unit: unit::FT_S,
            factor: "6,000 ft/s".into(),
            because: format!("speed reached {} ft/s", thousands(*from_fps)),
        },
        Effect::LiftOff { wheel_load } => state(
            "Lift-off",
            "wheels left the ground",
            format!("wheel load {} while climbing", percent(*wheel_load)),
        ),
        Effect::SurfaceDropped => state(
            "Surface dropped",
            "wheels in the air",
            "the ground fell away under the wheels".into(),
        ),
        Effect::Touchdown(landing) => EffectLine {
            label: "Touchdown".into(),
            value: landing
                .score
                .map_or(Value::None, |s| Value::Int(i64::from(s))),
            unit: "",
            factor: match landing.score {
                Some(score) => format!("graded {score}"),
                None => "not graded".into(),
            },
            because: format!(
                "descent {} ft/s, bank {} deg",
                fixed(-landing.touchdown.vertical_fps, 1),
                fixed(landing.touchdown.bank_deg, 1)
            ),
        },
        Effect::UnsafeTouchdown(u) => {
            let mut why = Vec::new();
            if u.water {
                why.push("water".to_owned());
            }
            if u.not_landable {
                why.push("not a runway".to_owned());
            }
            if u.gear_up {
                why.push(format!("gear at {}", percent(u.gear)));
            }
            why.push(format!("landing limits {:?}", u.severity).to_lowercase());
            state(
                "Unsafe touchdown",
                if u.bounced { "bounced" } else { "crashed" },
                why.join(", "),
            )
        }
        Effect::LegacyFloor { bounced } => state(
            "Ground floor",
            if *bounced { "bounced" } else { "crashed" },
            "the legacy adapter reached the ground".into(),
        ),
        Effect::Rolling(r) => ratio(
            "Rolling",
            r.wind_grip,
            if r.brakes {
                "wheel brakes on".into()
            } else {
                "rolling resistance".into()
            },
        ),
        Effect::BlastKick(b) => EffectLine {
            label: "Blast kick".into(),
            value: Value::Num(round(b.strength, 2)),
            unit: unit::RATIO,
            factor: format!("strength {}", fixed(b.strength, 2)),
            because: "a missile blast nearby".into(),
        },
        Effect::Jolt { rates } => {
            let peak = rates.iter().fold(0., |m: f64, r| m.max(r.abs()));
            EffectLine {
                label: "Blast jolt".into(),
                value: Value::Num(round(peak.to_degrees(), 1)),
                unit: unit::DEG_S,
                factor: format!("rotation {} deg/s", fixed(peak.to_degrees(), 1)),
                because: "a missile blast is still rotating the aircraft".into(),
            }
        }
        Effect::Turbulence(d) => EffectLine {
            label: "Turbulence".into(),
            value: Value::Num(round(d.vertical_fps, 1)),
            unit: unit::FT_S,
            factor: format!("vertical {} ft/s", fixed(d.vertical_fps, 1)),
            because: "turbulence applied by the host".into(),
        },
        Effect::BuildingRebound => state(
            "Building rebound",
            "bounced back",
            "the No crashes option".into(),
        ),
    }
}

// ---------------------------------------------------------------------------
// flight.telemetry

/// Everything one `flight.telemetry` tree reads.
pub struct Telemetry<'a> {
    pub label: &'a str,
    pub name: &'a str,
    pub trace: &'a FlightTrace,
    /// Measured air data, when the atmosphere covers the altitude.
    pub air: Option<&'a tore_sim::telemetry::AirData>,
    pub altitude_ft: f64,
    /// Height above the surface under the aircraft (terrain or runway).
    pub agl_ft: f64,
    pub g: f64,
    pub fuel_lb: f64,
    /// Body rates, [roll, pitch, yaw] rad/s.
    pub rates: [f64; 3],
    pub throttle: f64,
    pub afterburner: bool,
}

fn path_label(path: Path) -> &'static str {
    match path {
        Path::NotStepped => "not stepped yet",
        Path::Wreck => "wreck",
        Path::Stopped(_) => "stopped",
        Path::Native => "native research adapter",
        Path::Legacy => "legacy adapter",
        Path::Hybrid => "hybrid adapter",
    }
}

/// Why the positive G limit is what it is.
pub fn g_limit_reason(trace: &FlightTrace) -> String {
    let Some(a) = &trace.adapter else {
        return String::new();
    };
    let e = a.envelope;
    let mut why = Vec::new();
    if e.low_speed_ceiling.is_some() {
        why.push("low-speed ceiling".to_owned());
    }
    if e.extra_g {
        why.push("Pull extra G".to_owned());
    } else if e.load_divisor != 1. {
        why.push(format!(
            "fuel and stores divide by {}",
            fixed(e.load_divisor, 2)
        ));
    }
    if let Some(hold) = e.fast_hold {
        why.push(format!("fast-side hold at {} G", fixed(hold.g, 0)));
    } else if e.rows == 0 {
        why.push("outside every envelope row".to_owned());
    } else {
        why.push(format!(
            "envelope {} G at this speed",
            fixed(e.envelope_g[1], 0)
        ));
    }
    why.join(", ")
}

/// Builds a flight-model telemetry tree. See docs/REPLAYS.md for the layout.
pub fn flight_telemetry(t: &Telemetry) -> Vec<Node> {
    let mut tree = Tree::default();
    tree.text(
        0,
        "Aircraft",
        t.label,
        &format!("{}, {}", t.name, path_label(t.trace.path)),
    );
    tree.num(0, "Altitude", t.altitude_ft, 0, unit::FT, "above sea level");
    tree.num(0, node::AGL, t.agl_ft, 0, unit::FT, "measured");
    if let Some(air) = t.air {
        tree.head(0, "Air data", "measurements, never causes");
        tree.num(1, node::TAS, air.true_airspeed_knots, 0, unit::KT, "");
        tree.num(1, node::MACH, air.mach, 2, "", "");
        if let Some(aoa) = air.angle_of_attack_deg {
            tree.num(1, node::AOA, aoa, 1, unit::DEG, "");
        }
        if let Some(slip) = air.sideslip_deg {
            tree.num(1, node::SIDESLIP, slip, 1, unit::DEG, "");
        }
        tree.num(
            1,
            "q",
            air.dynamic_pressure_pa * LB_FT2_PER_PA,
            0,
            unit::LB_FT2,
            "dynamic pressure",
        );
    }
    tree.head(0, "Load", "");
    tree.num(1, node::LOAD, t.g, 2, unit::G, "delivered");
    if let Some(a) = &t.trace.adapter {
        let e = a.envelope;
        tree.num(
            1,
            "Asked",
            e.stick_g,
            2,
            unit::G,
            "the stick's share of the limit, times authority",
        );
        tree.num(
            1,
            node::G_LIMIT,
            e.limits_g[1],
            2,
            unit::G,
            &g_limit_reason(t.trace),
        );
    }
    tree.head(0, "Rates", "");
    for (label, rate) in ["Roll", "Pitch", "Yaw"].into_iter().zip(t.rates) {
        tree.num(1, label, rate.to_degrees(), 0, unit::DEG_S, "");
    }
    if let Some(a) = &t.trace.adapter {
        // Thrust and drag to ten pounds: thousands of pounds each.
        let p = a.power;
        tree.num(
            0,
            "Thrust",
            round(p.thrust_lbf, -1),
            0,
            unit::LB,
            "rated x lapse x power available",
        );
        tree.num(
            1,
            "Lapse",
            p.lapse,
            2,
            unit::RATIO,
            "the aircraft model's curve for this height and speed",
        );
        tree.num(1, "Power available", p.power_available, 2, unit::RATIO, "");
        tree.num(1, "Throttle", p.throttle, 2, unit::RATIO, "");
        tree.flag(1, "Afterburner", p.afterburner, "");
        let d = a.forces.drag;
        tree.num(0, "Drag", round(d.total_lbf, -1), 0, unit::LB, "applied");
        for (label, v) in [
            ("Airframe", d.airframe_lbf),
            ("Fuel and stores", d.load_lbf),
            ("Pull", d.pull_lbf),
            ("Gear", d.gear_lbf),
            ("Flaps", d.flaps_lbf),
            ("Airbrake", d.airbrake_lbf),
            ("Slip", d.slip_lbf),
        ] {
            tree.num(1, label, round(v, -1), 0, unit::LB, "");
        }
        if d.damage_percent > 0. {
            tree.num(1, "Damage", d.damage_percent, 0, unit::PERCENT, "");
        }
        let e = a.envelope;
        tree.num(0, "Stall speed", knots(e.stall_fps), 0, unit::KT, "");
        tree.num(1, "Authority", e.authority, 2, unit::RATIO, "");
    } else {
        tree.num(0, "Throttle", t.throttle, 2, unit::RATIO, "");
        tree.flag(1, "Afterburner", t.afterburner, "");
    }
    tree.num(0, "Fuel", t.fuel_lb, 0, unit::LB, "");
    if let Some(contact) = t.trace.contact {
        tree.text(0, "Contact", contact_label(&contact), "");
    }
    let effects = t.trace.effects();
    tree.int(0, "Effects applied", effects.len() as i64, "this tick");
    for effect in &effects {
        let line = effect_line(effect);
        tree.add(1, &line.label, line.value, line.unit, &line.because);
    }
    tree.nodes
}

fn contact_label(contact: &Contact) -> &'static str {
    match contact {
        Contact::Airborne => "airborne",
        Contact::SurfaceDropped => "surface dropped away",
        Contact::LiftOff { .. } => "lift-off",
        Contact::Unsafe(_) => "unsafe touchdown",
        Contact::Rolling(_) => "on the wheels",
        Contact::LegacyFloor { .. } => "ground floor",
    }
}

// ---------------------------------------------------------------------------
// weapon.guidance

/// A seeker as the guidance tree shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeekerView {
    pub status: &'static str,
    pub quality: f64,
    pub acquired: bool,
    /// The aircraft the seeker holds, if any.
    pub tracking: Option<u32>,
}

/// Everything one `weapon.guidance` tree reads.
pub struct Guidance<'a> {
    pub weapon: &'a str,
    pub shooter: u32,
    pub target: Option<u32>,
    /// Launch mode: `cued` or `boresight`.
    pub mode: &'a str,
    /// Guidance kind: `active radar`, `infrared` and so on.
    pub guidance: &'a str,
    pub seeker: Option<SeekerView>,
    pub enabled: bool,
    /// Ticks since launch.
    pub age: u64,
    pub speed_fps: f64,
    /// Distance to the intended target now, feet.
    pub range_ft: Option<f64>,
    /// Closest distance to the intended target so far, feet, and when.
    pub closest: Option<(f64, u64)>,
    /// Seconds to the predicted intercept, when the missile has a solution.
    pub intercept_s: Option<f64>,
    /// The latest decoy roll against it, in words.
    pub decoy: Option<String>,
    pub who: &'a dyn Fn(u32) -> String,
}

/// Builds a guided weapon's guidance tree. See docs/REPLAYS.md.
pub fn weapon_guidance(g: &Guidance) -> Vec<Node> {
    let mut tree = Tree::default();
    tree.text(0, "Weapon", g.weapon, &format!("{} guidance", g.guidance));
    tree.id(0, "Shooter", Some(g.shooter), &(g.who)(g.shooter));
    let target_note = g.target.map_or_else(|| "none".to_owned(), |id| (g.who)(id));
    tree.id(0, node::TARGET, g.target, &target_note);
    tree.text(0, "Mode", g.mode, "");
    match g.seeker {
        Some(s) => {
            tree.text(
                0,
                "Seeker",
                s.status,
                if !g.enabled {
                    "not yet enabled"
                } else if s.acquired {
                    "acquired"
                } else {
                    "searching"
                },
            );
            tree.num(1, "Quality", s.quality, 2, "", "");
            let note = s.tracking.map_or_else(String::new, |id| (g.who)(id));
            tree.id(1, "Tracking", s.tracking, &note);
        }
        None => tree.text(0, "Seeker", "none", "unguided"),
    }
    tree.num(0, "Time of flight", g.age as f64 / TICKS, 1, unit::S, "");
    tree.num(0, "Speed", knots(g.speed_fps), 0, unit::KT, "");
    if let Some(r) = g.range_ft {
        tree.num(0, "Range to target", r, 0, unit::FT, "");
    }
    if let Some((d, tick)) = g.closest {
        tree.num(
            0,
            "Closest approach",
            d,
            0,
            unit::FT,
            &format!("at {}", clock(tick)),
        );
    }
    if let Some(s) = g.intercept_s.filter(|s| s.is_finite()) {
        tree.num(0, "Intercept in", s, 1, unit::S, "predicted");
    }
    if let Some(decoy) = &g.decoy {
        tree.text(0, "Decoy roll", decoy.as_str(), "");
    }
    tree.nodes
}

/// A multi-line plain-text rendering of a tree, for tests.
#[cfg(test)]
pub(crate) fn render(nodes: &[Node]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for n in nodes {
        let value = match &n.value {
            Value::None => String::new(),
            Value::Bool(b) => if *b { "yes" } else { "no" }.into(),
            Value::Int(v) => v.to_string(),
            Value::Num(v) => fixed(*v, 3),
            Value::Text(t) => t.clone(),
            Value::Id(id) => format!("#{id}"),
            Value::Ids(ids) => ids
                .iter()
                .map(|id| format!("#{id}"))
                .collect::<Vec<_>>()
                .join(" "),
        };
        let _ = write!(out, "{}{}", "  ".repeat(usize::from(n.depth)), n.label);
        if !value.is_empty() {
            let _ = write!(out, " = {value}");
        }
        if !n.unit.is_empty() {
            let _ = write!(out, " {}", n.unit);
        }
        if !n.note.is_empty() {
            let _ = write!(out, "  ({})", n.note);
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests;
