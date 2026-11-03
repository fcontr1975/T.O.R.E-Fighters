//! AI goldens: the decision functions the replays work splits into
//! per-reason checks, standalone controllers fed scripted frames, a
//! two-sided mission flown through a small synthetic host, and an airfield
//! departure and landing.

use std::collections::BTreeSet;

use tore_formats::aircraft::AircraftId;
use tore_formats::weapons::Zone;

use super::{
    Fingerprint, Probe, fixtures, record_flight, record_input, record_threat, run_twice, verify,
};
use crate::ai::airfield::{AirfieldAnchors, GroundStart, LandingOrder, RunwayView};
use crate::ai::controller::{
    ActorIdentity, BehaviorFamily, BehaviorProfile, Completion, Controller, DecisionFrame,
    DefenseMotion, DefenseSource, FrameEvent, IntentBatch, LeaderView, MissionRole, MotionIntent,
    OwnState, RouteView, SearchContact, StationView, TargetView, ThreatReport, WingView,
};
use crate::ai::engagement::{
    self, Assignment, HostileEscort, PatrolRegion, Policy, ProtectedView, Role, Stance,
};
use crate::ai::experience::{ExperienceOrigin, ResolvedExperience};
use crate::ai::mission::{ActorSetup, AiActor, AiMission, MissionOutput, StationSpec, WorldObject};
use crate::ai::route::Position;
use crate::ai::targeting::Side;
use crate::ai::thought::{Message, Outcome};
use crate::ai::threat::{
    DecoyOutcome, DispenserStore, FlightState, GuidingMissile, SeekerClass, TimeOfDay,
};
use crate::ai::weapon_service::{
    ActorId, Delay, ProjectilePacing, Rounds, StationId, StoreCapability, StoreState,
};
use crate::ai::wing::{
    Formation, ReceiverOutcome, SpacingAxis, TargetId, TargetOrder, WingControl, WingRequest,
};
use crate::ai::{DecisionRandom, Experience, ScalarSpeed, SpeedLimits, threat};
use crate::airport::ApproachEnd;
use crate::attitude::{Basis, dot, unit};
use crate::combat::missiles::Guidance;
use crate::combat::threats::MissileSnapshot;
use crate::flight;
use crate::research::Surface;
use crate::sensors::{self, Observable, Sensors};

// Recorded on macOS aarch64. See the module comment in golden_tests.rs before
// changing any of these.
const DECISION_FUNCTIONS: u64 = 0x6b7a_6611_f77f_f972;
const CONTROLLERS: u64 = 0x60e4_6979_91de_258a;
const MISSION_ENGAGEMENT: u64 = 0xc1bb_2d61_7227_eb04;
const AIRFIELD: u64 = 0x0a4b_f5ea_aa0a_581f;

#[test]
fn ai_decision_functions_match_recorded_fingerprint() {
    verify([run_twice(
        "ai/decision-functions",
        DECISION_FUNCTIONS,
        decision_functions,
    )]);
}

#[test]
fn ai_controllers_match_recorded_fingerprint() {
    let coverage = std::cell::RefCell::new(Coverage::default());
    let outcome = run_twice("ai/controllers", CONTROLLERS, |probe| {
        let (value, seen) = controllers(probe);
        *coverage.borrow_mut() = seen;
        value
    });
    verify([outcome]);
    coverage.into_inner().require_controllers();
}

#[test]
fn ai_mission_engagement_matches_recorded_fingerprint() {
    let coverage = std::cell::RefCell::new(Coverage::default());
    let outcome = run_twice("ai/mission-engagement", MISSION_ENGAGEMENT, |probe| {
        let (value, seen) = mission_engagement(probe);
        *coverage.borrow_mut() = seen;
        value
    });
    verify([outcome]);
    coverage.into_inner().require_engagement();
}

#[test]
fn ai_airfield_matches_recorded_fingerprint() {
    let coverage = std::cell::RefCell::new(Coverage::default());
    let outcome = run_twice("ai/airfield", AIRFIELD, |probe| {
        let (value, seen) = airfield(probe);
        *coverage.borrow_mut() = seen;
        value
    });
    verify([outcome]);
    coverage.into_inner().require_airfield();
}

/// Reading every AI record and draining the message journal after every
/// step, as the replay recorder and the debug panels will, leaves both
/// scenarios exactly as a run that never looks: the same fingerprint, so the
/// recorded values above hold for both.
#[test]
fn reading_every_ai_record_changes_no_behaviour() {
    let mut seen = RecordCoverage::default();
    let (plain, _) = mission_engagement(&Probe::default());
    let (read, _) =
        mission_engagement_observed(&Probe::default(), &mut |_, _| {}, &mut |mission| {
            seen.read(mission)
        });
    assert_eq!(
        read, plain,
        "reading the AI records changed the engagement scenario"
    );
    let (plain, _) = airfield(&Probe::default());
    let (read, _) = airfield_observed(&Probe::default(), &mut |_, _| {}, &mut |mission| {
        seen.read(mission)
    });
    assert_eq!(
        read, plain,
        "reading the AI records changed the airfield scenario"
    );
    seen.require();
}

/// Exact checkpoints (docs/formats/checkpoint.md, stage H): each AI mission
/// replaced by a copy restored from its own checkpoint every 97 ticks (a
/// prime, so the restores fall at every phase of the AI's 30- and 60-tick
/// cycles) and on the ticks the engagement's script acts flies on exactly as
/// the mission that never was: the same fingerprint, so a restore keeps
/// every piece of AI state the fingerprint can see, through the engagement's
/// orders, hits, kills and warnings and every airfield phase of the
/// departure and the landing. The restore happens at the start of a tick,
/// before the host reads the mission: a restored actor reports neutral
/// controls (`last_input`, scratch) until its next step, and the fingerprint
/// reads them after each step.
#[test]
fn restoring_the_ai_missions_mid_flight_changes_no_behaviour() {
    const SCRIPTED: [u64; 12] = [
        120, 360, 600, 900, 1_800, 2_000, 2_400, 2_600, 3_000, 3_600, 4_200, 4_800,
    ];
    let due = |tick: u64| tick % 97 == 1 || SCRIPTED.contains(&tick);
    let restores = std::cell::Cell::new(0_usize);
    let mut restore = |tick: u64, mission: &mut AiMission| {
        if due(tick) {
            *mission = restored(mission);
            restores.set(restores.get() + 1);
        }
    };
    let (plain, _) = mission_engagement(&Probe::default());
    let (again, _) = mission_engagement_observed(&Probe::default(), &mut restore, &mut |_| {});
    assert_eq!(
        again, plain,
        "a restored mission flew the engagement scenario differently"
    );
    let (plain, _) = airfield(&Probe::default());
    let (again, _) = airfield_observed(&Probe::default(), &mut restore, &mut |_| {});
    assert_eq!(
        again, plain,
        "a restored mission flew the airfield scenario differently"
    );
    // The engagement's 7,200 ticks, the departure's 18,000 and the
    // landing's 48,000: 87, 198 and 507 restores.
    let expected: usize = [7_200, 150 * 120, 400 * 120]
        .into_iter()
        .map(|ticks| (0..ticks).filter(|tick| due(*tick)).count())
        .sum();
    assert_eq!(restores.get(), expected);
}

/// `mission` coded and decoded with the flight models its actors fly, as a
/// world's table holds them; the copy must code to the same bytes.
fn restored(mission: &AiMission) -> AiMission {
    let mut models = crate::checkpoint::Models::default();
    for actor in mission.actors() {
        models
            .insert(actor.identity().aircraft, actor.flight().import_model())
            .expect("a flight model");
    }
    crate::checkpoint::round_trip(mission, &models).expect("the mission restores")
}

/// What the records said across the two scenarios, so the neutrality test
/// cannot pass by reading records that stayed empty.
#[derive(Default)]
struct RecordCoverage {
    messages: BTreeSet<&'static str>,
    outcomes: BTreeSet<&'static str>,
    actor_paths: BTreeSet<String>,
    branches: BTreeSet<String>,
    draw_sites: BTreeSet<&'static str>,
    text_bytes: usize,
}

impl RecordCoverage {
    fn read(&mut self, mission: &mut AiMission) {
        for actor in mission.actors() {
            let controller = actor.controller();
            // Debug text reads every field of every record.
            self.text_bytes += format!(
                "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
                actor.trace(),
                controller.trace(),
                controller.draws(),
                actor.route_draws(),
                controller.last_batch(),
                controller.weapon_phase(),
                controller.weapon_deadline(),
                controller.active_maneuver(),
                controller.search_contact(),
                actor.defense_decision(),
            )
            .len();
            self.actor_paths.insert(format!("{:?}", actor.trace().path));
            let branch = format!("{:?}", controller.trace().motion.branch);
            self.branches.insert(
                branch
                    .split(['(', ' ', '{'])
                    .next()
                    .unwrap_or_default()
                    .to_string(),
            );
            self.draw_sites
                .extend(controller.draws().draws().filter_map(|draw| draw.site));
        }
        let journal = mission.take_journal();
        assert_eq!(journal.dropped, 0, "the journal overflowed within one tick");
        self.text_bytes += format!("{journal:?}").len();
        for entry in &journal.entries {
            self.messages.insert(match entry.message {
                Message::AttackEvidence(_) => "attack evidence",
                Message::FreeSelection { .. } => "free selection",
                Message::ContactSelection { .. } => "contact selection",
                Message::WingRequest(_) => "wing request",
                Message::EscortPriority { .. } => "escort priority",
                Message::MissileWarning(_) => "missile warning",
            });
            for receipt in &entry.receipts {
                self.outcomes.insert(match receipt.outcome {
                    Outcome::Queued => "queued",
                    Outcome::Delivered => "delivered",
                    Outcome::Ignored(_) => "ignored",
                    Outcome::Expired(_) => "expired",
                    Outcome::Order(_) => "order",
                    Outcome::WarningDue { .. } => "warning due",
                    Outcome::WarningReceived { .. } => "warning received",
                    Outcome::WarningDropped(_) => "warning dropped",
                });
            }
        }
    }

    fn require(&self) {
        let summary = format!(
            "messages {:?}, outcomes {:?}, paths {:?}, branches {:?}, draw sites {:?}",
            self.messages, self.outcomes, self.actor_paths, self.branches, self.draw_sites
        );
        if std::env::var_os("TORE_GOLDEN_VERBOSE").is_some() {
            eprintln!("records seen: {summary}");
        }
        assert!(self.text_bytes > 0);
        // Queued and delivered attack reports and a leader's release are
        // covered by the targeted tests in `ai/thought_tests.rs`; these
        // scenarios never produce them.
        for (what, set, required) in [
            (
                "message",
                self.messages.iter().copied().collect::<Vec<_>>(),
                &[
                    "attack evidence",
                    "wing request",
                    "escort priority",
                    "missile warning",
                ][..],
            ),
            (
                "outcome",
                self.outcomes.iter().copied().collect(),
                &["expired", "order", "warning due", "warning received"][..],
            ),
        ] {
            for kind in required {
                assert!(set.contains(kind), "no {what} {kind}: {summary}");
            }
        }
        for path in ["Controller", "Airfield", "Destroyed", "Dummy"] {
            assert!(
                self.actor_paths.contains(path),
                "no actor path {path}: {summary}"
            );
        }
        for branch in ["Formation", "NewManeuver", "MissileDefense"] {
            assert!(
                self.branches.contains(branch),
                "no motion branch {branch}: {summary}"
            );
        }
        assert!(!self.draw_sites.is_empty(), "no labelled draw: {summary}");
    }
}

// ---------------------------------------------------------------------------
// Recording

fn record_rounds(fp: &mut Fingerprint, rounds: Rounds) {
    match rounds {
        Rounds::Unlimited => fp.u64(u64::MAX),
        Rounds::Finite(count) => fp.int(count),
    }
}

fn record_landing_order(fp: &mut Fingerprint, order: &LandingOrder) {
    fp.int(order.runway.airport);
    fp.int(order.runway.object);
    fp.name(&order.reason);
}

fn record_wing_request(fp: &mut Fingerprint, request: &WingRequest) {
    match request {
        WingRequest::Break {
            heading_offset_deg,
            pitch_deg,
        } => {
            fp.u64(1);
            fp.int(*heading_offset_deg);
            fp.int(*pitch_deg);
        }
        WingRequest::Approach {
            heading_deg,
            pitch_deg,
            speed,
        } => {
            fp.u64(2);
            fp.int(*heading_deg);
            fp.int(*pitch_deg);
            fp.f64(speed.0);
        }
        WingRequest::Spacing { axis, feet } => {
            fp.u64(3);
            fp.name(axis);
            fp.int(*feet);
        }
        WingRequest::FormationSelection(formation) => {
            fp.u64(4);
            fp.name(formation);
        }
        WingRequest::WingControl(control) => {
            fp.u64(5);
            fp.name(control);
        }
        WingRequest::TargetAssignment(order) => {
            fp.u64(6);
            fp.name(order);
        }
        WingRequest::Land(order) => {
            fp.u64(7);
            record_landing_order(fp, order);
        }
    }
}

fn record_receiver_outcome(fp: &mut Fingerprint, outcome: &ReceiverOutcome) {
    match outcome {
        ReceiverOutcome::Applied(setting) => {
            fp.u64(1);
            fp.name(setting);
        }
        ReceiverOutcome::Rejected(reason) => {
            fp.u64(2);
            fp.name(reason);
        }
        ReceiverOutcome::AppliedNoMotion => fp.u64(3),
        ReceiverOutcome::MotionInstalled(summary) => {
            fp.u64(4);
            fp.int(summary.heading_deg);
            fp.int(summary.pitch_deg);
            fp.f64(summary.speed.0);
            fp.name(&summary.bank);
            fp.name(&summary.duration);
        }
    }
}

fn record_order_result(fp: &mut Fingerprint, result: Option<crate::ai::Result<ReceiverOutcome>>) {
    match result {
        None => fp.u64(0),
        Some(Ok(outcome)) => {
            fp.u64(1);
            record_receiver_outcome(fp, &outcome);
        }
        Some(Err(error)) => {
            fp.u64(2);
            fp.text(&error.to_string());
        }
    }
}

fn record_motion(fp: &mut Fingerprint, motion: &MotionIntent) {
    fp.u64(motion.id);
    fp.int(motion.request.heading_deg);
    fp.name(&motion.request.pitch);
    fp.name(&motion.request.bank);
    fp.name(&motion.request.speed);
    fp.name(&motion.request.duration);
    fp.f64(motion.heading_deg);
    fp.f64(motion.flight_path_pitch_deg);
    fp.f64(motion.speed.0);
    fp.name(&motion.bank);
    match motion.completion {
        Completion::Deadline(deadline) => {
            fp.u64(1);
            fp.u64(deadline.0);
        }
        Completion::Axis(axis) => {
            fp.u64(2);
            fp.name(&axis);
        }
    }
    fp.option(motion.steering_point, |fp, point| fp.vector(point));
    fp.name(&motion.mode);
    fp.bool(motion.formation_flight);
    fp.bool(motion.afterburner);
}

fn record_batch(fp: &mut Fingerprint, batch: &IntentBatch) {
    fp.option(batch.motion.as_ref(), record_motion);
    fp.option(batch.sensor.designate, |fp, id| fp.int(id));
    fp.bool(batch.sensor.clear_designation);
    fp.count(batch.weapons.len());
    for weapon in &batch.weapons {
        let request = weapon.request;
        fp.int(request.actor.0);
        fp.int(request.station.0);
        fp.int(request.target.0);
        fp.u64(request.request_id.0);
    }
    fp.option(batch.devices, |fp, devices| {
        fp.name(&devices.class);
        fp.int(devices.count);
    });
    fp.count(batch.wing.len());
    for request in &batch.wing {
        record_wing_request(fp, request);
    }
    fp.option(batch.activity, |fp, activity| fp.name(&activity));
    fp.count(batch.fallbacks.len());
    for fallback in &batch.fallbacks {
        fp.name(fallback);
    }
    fp.option(batch.reason, |fp, reason| fp.name(&reason));
    fp.option(batch.fuel_state, |fp, state| fp.name(&state));
    fp.count(batch.launch_calls.len());
    for launcher in &batch.launch_calls {
        fp.int(*launcher);
    }
}

fn record_controller(fp: &mut Fingerprint, controller: &Controller) {
    fp.option(controller.target(), |fp, id| fp.int(id));
    fp.option(controller.reason(), |fp, reason| fp.name(&reason));
    fp.option(controller.fuel_state(), |fp, state| fp.name(&state));
    let applied = controller.fallbacks().applied();
    fp.count(applied.len());
    for (fallback, count) in applied {
        fp.name(&fallback);
        fp.u64(count);
    }
    let (control, horizontal, vertical) = controller.wing_settings();
    fp.option(control, |fp, control| fp.name(&control));
    fp.option(horizontal, |fp, feet| fp.int(feet));
    fp.option(vertical, |fp, feet| fp.int(feet));
    // Only the formation trace fields other aircraft steer by.
    fp.option(controller.formation_trace(), |fp, trace| {
        fp.name(&trace.phase);
        fp.option(trace.planned_velocity, |fp, velocity| fp.vector(velocity));
    });
    fp.name(&controller.experience().level);
    fp.u64(controller.decision_draw_state());
}

fn record_target_view(fp: &mut Fingerprint, target: &TargetView) {
    fp.int(target.id);
    fp.int(target.side.0);
    fp.vector(target.position);
    fp.f64(target.heading_deg);
    fp.f64(target.pitch_deg);
    fp.f64(target.speed.0);
    fp.f64(target.maximum_speed.0);
    for flag in [
        target.is_aircraft,
        target.is_fighter,
        target.human_controlled,
        target.valid,
        target.type_allowed,
        target.seeker_eligible,
        target.terrain_blocked,
        target.sensor_supported,
    ] {
        fp.bool(flag);
    }
    fp.int(target.wing_attackers);
}

fn record_assignment(fp: &mut Fingerprint, assignment: &Assignment) {
    fp.name(&assignment.role);
    fp.name(&assignment.stance);
    fp.count(assignment.protected_ids.len());
    for id in &assignment.protected_ids {
        fp.int(*id);
    }
    fp.count(assignment.destroy_ids.len());
    for id in &assignment.destroy_ids {
        fp.int(*id);
    }
    fp.count(assignment.hostile_escorts.len());
    for escort in &assignment.hostile_escorts {
        fp.int(escort.principal_id);
        fp.int(escort.escort_id);
    }
    fp.option(assignment.patrol, |fp, patrol| {
        fp.vector(patrol.center_ft);
        fp.f64(patrol.radius_ft);
        if let Some(bounds) = patrol.clip {
            for point in bounds {
                for value in point {
                    fp.f64(value);
                }
            }
        }
    });
}

fn record_actor(fp: &mut Fingerprint, actor: &AiActor) {
    fp.int(actor.id());
    fp.bool(actor.alive());
    fp.name(&actor.activity());
    fp.bool(actor.is_neutral());
    fp.bool(actor.bugged_out());
    fp.bool(actor.is_dummy());
    record_controller(fp, actor.controller());
    record_input(fp, actor.last_input());
    record_flight(fp, actor.flight());
    fp.count(actor.stations().len());
    for station in actor.stations() {
        fp.int(station.station.0);
        record_rounds(fp, station.store.rounds);
        fp.bool(station.store.inhibited);
    }
    fp.count(actor.dispensers().len());
    for dispenser in actor.dispensers() {
        fp.name(&dispenser.class);
        fp.int(dispenser.count);
    }
    record_assignment(fp, actor.assignment());
    fp.count(actor.perceived_attacks().len());
    for attack in actor.perceived_attacks() {
        fp.option(attack.report.attacker_id, |fp, id| fp.int(id));
        fp.int(attack.report.defended_id);
        fp.option(attack.bearing_world_deg, |fp, bearing| fp.f64(bearing));
        fp.u64(attack.observed_tick);
        fp.option(attack.event_id, |fp, id| fp.int(id));
    }
    let threats: Vec<_> = actor.missile_threats().collect();
    fp.count(threats.len());
    for record in threats {
        record_threat(fp, record);
    }
    fp.option(actor.defense_decision(), |fp, decision| {
        fp.int(decision.threat_id);
        fp.option(decision.motion, |fp, motion| {
            fp.name(&motion.maneuver);
            fp.f64(motion.heading_deg);
            fp.f64(motion.flight_path_pitch_deg);
        });
        fp.option(decision.burst, |fp, burst| {
            fp.int(burst.chaff);
            fp.int(burst.flares);
        });
    });
    for snapshots in [
        actor.awareness().current_observations().collect::<Vec<_>>(),
        actor.awareness().remembered().collect::<Vec<_>>(),
    ] {
        fp.count(snapshots.len());
        for snapshot in snapshots {
            record_target_view(fp, &snapshot.target);
            fp.vector(snapshot.velocity);
            fp.u64(snapshot.first_observed_tick);
            fp.u64(snapshot.last_observed_tick);
        }
    }
    fp.option(actor.sensors(), |fp, sensors| {
        fp.option(sensors.selected(), |fp, id| fp.int(id));
        fp.option(sensors.acquired(), |fp, id| fp.int(id));
        fp.count(sensors.contacts().len());
        for contact in sensors.contacts() {
            fp.int(contact.id);
            fp.name(&contact.channel);
            fp.vector(contact.position);
            fp.vector(contact.velocity);
            fp.bool(contact.destroyed);
        }
    });
    fp.option(actor.airfield(), |fp, sequence| {
        fp.name(&sequence.phase());
        fp.count(sequence.leg());
        fp.option(sequence.slot(), |fp, slot| fp.int(slot));
        fp.int(sequence.go_arounds());
        fp.int(sequence.order());
        fp.bool(sequence.is_departure());
        fp.option(sequence.landing_reason(), |fp, reason| fp.name(&reason));
    });
    fp.option(actor.landing_order(), record_landing_order);
    fp.option(actor.home_runway(), |fp, runway| fp.int(runway.object));
}

fn record_output(fp: &mut Fingerprint, output: &MissionOutput) {
    fp.count(output.launches.len());
    for launch in &output.launches {
        fp.int(launch.actor);
        fp.int(launch.station.0);
        fp.int(launch.target);
        fp.u64(launch.request_id.0);
        fp.int(launch.projectiles);
    }
    fp.count(output.devices.len());
    for device in &output.devices {
        fp.int(device.actor);
        fp.name(&device.class);
        fp.int(device.released);
    }
    fp.count(output.activities.len());
    for (id, activity) in &output.activities {
        fp.int(*id);
        fp.name(activity);
    }
    fp.count(output.fallbacks.len());
    for (id, fallback) in &output.fallbacks {
        fp.int(*id);
        fp.name(fallback);
    }
    fp.count(output.wing.len());
    for (sender, request) in &output.wing {
        fp.int(*sender);
        record_wing_request(fp, request);
    }
    fp.count(output.launch_calls.len());
    for (actor, launcher) in &output.launch_calls {
        fp.int(*actor);
        fp.int(*launcher);
    }
}

// ---------------------------------------------------------------------------
// Shared builders

fn resolved(level: Experience) -> ResolvedExperience {
    ResolvedExperience {
        level,
        origin: ExperienceOrigin::QuickMission { selected: level },
    }
}

fn fighter() -> BehaviorProfile {
    BehaviorProfile {
        family: BehaviorFamily::FighterStrike,
        role: MissionRole::AirToAir,
    }
}

fn identity(id: u32, side: u32, wing: u8, member: u8, aircraft: AircraftId) -> ActorIdentity {
    ActorIdentity {
        actor: ActorId(id),
        side: Side(side),
        wing,
        member,
        leads: member == 0,
        aircraft,
        human_controlled: false,
    }
}

fn limits() -> SpeedLimits {
    SpeedLimits {
        minimum: ScalarSpeed(220.),
        maximum: ScalarSpeed(1500.),
        corner: ScalarSpeed(700.),
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    dot(d, d).sqrt()
}

fn wrap(degrees: f64) -> f64 {
    (degrees + 180.).rem_euclid(360.) - 180.
}

fn pacing(burst: u32, interval: Delay, reload: Delay) -> ProjectilePacing {
    ProjectilePacing {
        burst_count: burst,
        burst_interval: interval,
        reload,
        startup: Delay::seconds(0),
    }
}

// ---------------------------------------------------------------------------
// Scenario: decision functions

fn own_state(position: [f64; 3], heading: f64, pitch: f64, bank: f64, speed: f64) -> OwnState {
    OwnState {
        position,
        heading_deg: heading,
        flight_path_pitch_deg: pitch,
        body_pitch_offset_deg: 3.,
        bank_deg: bank,
        speed: ScalarSpeed(speed),
        limits: limits(),
        altitude_msl_ft: position[1],
        agl_ft: position[1],
        terrain_ahead_ft: 0.,
        terrain_climb_deg: -90.,
        minimum_altitude_ft: 300.,
        at_ceiling: false,
        on_ground: false,
        g_limit: 7.,
        roll_limit_deg_per_s: 180.,
        maximum_bank_deg: 80.,
        alive: true,
        fuel_endurance_s: 3_000.,
        time_home_s: Some(600.),
        rotorcraft: false,
        internal_fuel_lbs: 6_000.,
        radar_emitting: true,
    }
}

fn target_view(id: u32, side: u32, position: [f64; 3]) -> TargetView {
    TargetView {
        infrared_strength: 1.,
        id,
        side: Side(side),
        position,
        heading_deg: 180.,
        pitch_deg: 0.,
        speed: ScalarSpeed(750.),
        maximum_speed: ScalarSpeed(1500.),
        is_aircraft: true,
        is_fighter: true,
        human_controlled: false,
        valid: true,
        type_allowed: true,
        seeker_eligible: true,
        wing_attackers: 0,
        terrain_blocked: false,
        sensor_supported: true,
        link_track: false,
    }
}

fn zone(heading_deg: f64, pitch_deg: f64, range: (i32, i32), altitude: (i32, i32)) -> Zone {
    let raw = |degrees: f64| {
        if degrees >= 180. {
            i16::MAX
        } else {
            (degrees * 182.) as i16
        }
    };
    Zone {
        heading: raw(heading_deg),
        pitch: raw(pitch_deg),
        minimum_range: range.0,
        maximum_range: range.1,
        minimum_altitude: altitude.0,
        maximum_altitude: altitude.1,
    }
}

fn station_view(
    station: u8,
    limit: Option<f64>,
    range: (f64, Option<f64>),
    employment_zone: Option<Zone>,
    mount: [f64; 3],
) -> StationView {
    StationView {
        station: StationId(station),
        guided: true,
        capability: StoreCapability::AIR_TO_AIR_MISSILE,
        inhibited: false,
        rounds: Rounds::Finite(4),
        pointing_error_deg: 0.,
        employment_limit_deg: limit,
        employment_fit: None,
        minimum_range_ft: range.0,
        maximum_range_ft: range.1,
        energy_target: None,
        infrared_envelope: None,
        requires_radar: false,
        requires_sensor: false,
        employment_zone,
        mount,
        damage_vs_category: 100.,
        store_speed: ScalarSpeed(2400.),
        tracking_delay: Delay::seconds(1),
        pacing: pacing(1, Delay::seconds(0), Delay::seconds(2)),
    }
}

/// Stores covering every employment reason: angle, minimum and maximum range,
/// zone cone, zone altitude band and mount offset.
fn sweep_stations() -> Vec<StationView> {
    vec![
        station_view(0, None, (0., None), None, [0.; 3]),
        station_view(1, Some(30.), (0., Some(40_000.)), None, [0.; 3]),
        station_view(2, Some(5.), (0., Some(6_000.)), None, [3., -2., 12.]),
        station_view(
            3,
            None,
            (1_000., Some(20_000.)),
            Some(zone(30., 20., (1_000, 20_000), (-5_000, 5_000))),
            [0.; 3],
        ),
        station_view(
            4,
            Some(45.),
            (3_000., Some(60_000.)),
            Some(zone(180., 180., (3_000, 60_000), (-100_000, 100_000))),
            [-8., 0., 0.],
        ),
        station_view(
            5,
            Some(10.),
            (0., Some(8_000.)),
            Some(zone(5., 5., (0, 8_000), (-500, 500))),
            [0., -3., 4.],
        ),
    ]
}

fn decision_functions(probe: &Probe) -> u64 {
    let mut fp = Fingerprint::default();
    // Employment checks: every store against a sphere of target bearings,
    // elevations and ranges from nine own-ship attitudes.
    for (index, station) in sweep_stations().iter().enumerate() {
        for heading in [0., 95., 210.] {
            for (pitch, bank) in [(0., 0.), (-12., 70.), (30., -150.)] {
                let own = own_state([1_000., 12_000., -3_000.], heading, pitch, bank, 700.);
                for azimuth in (-170..=180).step_by(35) {
                    for elevation in (-60..=60).step_by(30) {
                        for range in [400., 2_500., 9_000., 30_000., 70_000.] {
                            let direction = Basis::new(
                                (heading + f64::from(azimuth)).to_radians(),
                                f64::from(elevation).to_radians(),
                                0.,
                            )
                            .forward;
                            let position =
                                std::array::from_fn(|i| own.position[i] + direction[i] * range);
                            let target = target_view(9, 2, position);
                            fp.option(station.employment_error(&own, &target), |fp, error| {
                                fp.f64(error);
                            });
                        }
                    }
                }
            }
        }
        probe.part(format!("employment station {index}"), fp.value());
    }

    // Mission target ranking across roles, stances, charges and reports.
    let own_id = 1;
    let own = [0., 20_000., 0.];
    let targets = {
        let mut ineligible = target_view(15, 2, [1_000., 20_000., 3_000.]);
        ineligible.seeker_eligible = false;
        let mut surface = target_view(16, 2, [500., 0., 4_000.]);
        surface.is_aircraft = false;
        let mut destroyed = target_view(17, 2, [0., 20_000., 2_500.]);
        destroyed.valid = false;
        let mut crowded = target_view(13, 2, [-6_000., 21_000., 12_000.]);
        crowded.wing_attackers = 2;
        vec![
            target_view(11, 2, [2_000., 20_500., 9_000.]),
            target_view(12, 2, [-9_000., 19_000., 30_000.]),
            crowded,
            target_view(14, 2, [45_000., 18_000., 60_000.]),
            target_view(5, 1, [300., 20_000., 1_000.]),
            ineligible,
            surface,
            destroyed,
        ]
    };
    let charge = |position: [f64; 3], alive: bool| ProtectedView {
        id: 4,
        position,
        velocity: [0., 0., 700.],
        alive,
    };
    let protected_sets = [
        vec![],
        vec![charge([1_000., 20_000., 8_000.], true)],
        vec![charge([60_000., 20_000., 0.], true)],
        vec![charge([1_000., 20_000., 8_000.], false)],
    ];
    let report = |attacker: Option<u32>, defended: u32| engagement::ThreatReport {
        attacker_id: attacker,
        defended_id: defended,
    };
    let report_sets = [
        vec![],
        vec![report(Some(12), own_id)],
        vec![report(Some(11), 4)],
        vec![report(None, 4), report(Some(14), 4)],
    ];
    let assignment = |role, stance| Assignment {
        role,
        stance,
        ..Assignment::default()
    };
    let assignments = [
        assignment(Role::FreeEngagement, Stance::EngageAssigned),
        Assignment {
            patrol: Some(PatrolRegion {
                clip: None,
                center_ft: [0., 20_000., 10_000.],
                radius_ft: 15_000.,
            }),
            ..assignment(Role::CombatAirPatrol, Stance::EngageAssigned)
        },
        Assignment {
            destroy_ids: vec![12],
            ..assignment(Role::Intercept, Stance::EngageAssigned)
        },
        Assignment {
            protected_ids: vec![4],
            destroy_ids: vec![13],
            hostile_escorts: vec![HostileEscort {
                principal_id: 11,
                escort_id: 12,
            }],
            ..assignment(Role::Escort, Stance::ProtectAssigned)
        },
        Assignment {
            protected_ids: vec![4],
            destroy_ids: vec![11],
            ..assignment(Role::Escort, Stance::EngageAssigned)
        },
        assignment(Role::Disengage, Stance::SelfDefense),
        assignment(Role::FreeEngagement, Stance::WeaponsHold),
        assignment(Role::FreeEngagement, Stance::SelfDefense),
    ];
    for (index, assignment) in assignments.iter().enumerate() {
        // One policy per assignment keeps the escort leash memory running
        // across the charge positions, as a live actor's would.
        let mut policy = Policy::default();
        for protected in &protected_sets {
            for reports in &report_sets {
                for current in [None, Some(11), Some(13)] {
                    let selection = policy.select(
                        own_id,
                        Side(1),
                        own,
                        assignment,
                        &targets,
                        protected,
                        reports,
                        current,
                        2,
                    );
                    fp.option(selection, |fp, selection| {
                        fp.int(selection.id);
                        fp.name(&selection.priority);
                    });
                    fp.bool(policy.must_rejoin());
                    for target in &targets {
                        fp.bool(
                            policy.allows_investigation(
                                assignment, target, protected, reports, own_id,
                            ),
                        );
                    }
                }
            }
        }
        probe.part(format!("policy assignment {index}"), fp.value());
    }
    fp.value()
}

// ---------------------------------------------------------------------------
// Scenario: controllers on scripted frames

/// A point-mass own ship that follows the controller's motion intents, so the
/// frames respond to the decisions without a flight model in the loop.
#[derive(Clone, Copy)]
struct PointMass {
    position: [f64; 3],
    heading: f64,
    pitch: f64,
    bank: f64,
    speed: f64,
}

impl PointMass {
    fn follow(&mut self, motion: Option<&MotionIntent>) {
        let dt = flight::DT;
        if let Some(motion) = motion {
            let turn = wrap(motion.heading_deg - self.heading).clamp(-15. * dt, 15. * dt);
            self.heading = wrap(self.heading + turn);
            self.bank = (wrap(motion.heading_deg - self.heading) * 3.).clamp(-70., 70.);
            self.pitch += (motion.flight_path_pitch_deg.clamp(-30., 30.) - self.pitch)
                .clamp(-8. * dt, 8. * dt);
            self.speed += (motion.speed.0 - self.speed).clamp(-25. * dt, 25. * dt);
        }
        let forward = Basis::new(self.heading.to_radians(), self.pitch.to_radians(), 0.).forward;
        for (position, axis) in self.position.iter_mut().zip(forward) {
            *position += axis * self.speed * dt;
        }
    }

    fn own(&self, endurance_s: f64, radar: bool, damaged: bool) -> OwnState {
        let ahead = [
            self.position[0] + self.heading.to_radians().sin() * 1_000.,
            0.,
            self.position[2] + self.heading.to_radians().cos() * 1_000.,
        ];
        let mut own = own_state(
            self.position,
            self.heading,
            self.pitch,
            self.bank,
            self.speed,
        );
        own.agl_ft = self.position[1] - fixtures::terrain(self.position[0], self.position[2]);
        own.terrain_ahead_ft = fixtures::terrain(ahead[0], ahead[2]);
        own.g_limit = if damaged { 4.5 } else { 7. };
        own.fuel_endurance_s = endurance_s;
        own.time_home_s = Some(self.position[0].hypot(self.position[2]) / 700.);
        own.internal_fuel_lbs = endurance_s * 2.;
        own.radar_emitting = radar;
        own
    }
}

/// Six stores covering every store filter: radar and sensor requirements,
/// inhibition, an empty store, a surface store, a zone and plain angles.
fn controller_stations() -> Vec<(StationView, u32)> {
    let mut radar = station_view(0, Some(40.), (1_500., Some(40_000.)), None, [4., -2., 0.]);
    radar.requires_radar = true;
    let mut infrared = station_view(
        1,
        None,
        (1_000., Some(15_000.)),
        Some(zone(30., 30., (1_000, 15_000), (-15_000, 15_000))),
        [-4., -2., 0.],
    );
    infrared.requires_sensor = true;
    infrared.tracking_delay = Delay::quarters(2);
    let mut gun = station_view(2, Some(5.), (0., Some(6_000.)), None, [0., 0., 10.]);
    gun.guided = false;
    gun.capability = StoreCapability::GUN;
    gun.damage_vs_category = 10.;
    gun.store_speed = ScalarSpeed(3_000.);
    gun.tracking_delay = Delay::quarters(1);
    gun.pacing = pacing(10, Delay::quarters(1), Delay::quarters(2));
    let mut surface = station_view(3, Some(20.), (0., Some(30_000.)), None, [0.; 3]);
    surface.capability = StoreCapability::SURFACE_STORE;
    let mut inhibited = station_view(4, Some(60.), (0., Some(50_000.)), None, [0.; 3]);
    inhibited.inhibited = true;
    let mut empty = station_view(5, Some(60.), (0., Some(50_000.)), None, [0.; 3]);
    empty.rounds = Rounds::Finite(0);
    vec![
        (radar, 4),
        (infrared, 2),
        (gun, 400),
        (surface, 2),
        (inhibited, 4),
        (empty, 0),
    ]
}

struct ScriptedController {
    controller: Controller,
    ship: PointMass,
    stations: Vec<(StationView, u32)>,
    dispensers: Vec<DispenserStore>,
    events: Vec<(u64, FrameEvent)>,
    leader: bool,
}

/// The world the scripted controllers see at one tick: two circling and
/// crossing hostiles, one that appears and is later destroyed, a surface
/// object and a friendly.
fn scripted_targets(tick: u64) -> Vec<TargetView> {
    let t = tick as f64 / 120.;
    let mut circling = target_view(
        11,
        2,
        [
            12_000. * (t * 0.05).sin(),
            20_000. + 1_500. * (t * 0.1).sin(),
            25_000. + 12_000. * (t * 0.05).cos(),
        ],
    );
    circling.heading_deg = wrap(90. - t * 0.05f64.to_degrees());
    let mut head_on = target_view(12, 2, [-4_000. + t * 150., 21_000., 60_000. - t * 900.]);
    head_on.heading_deg = 170.;
    head_on.sensor_supported = (tick / 400).is_multiple_of(2);
    let mut late = target_view(13, 2, [30_000. - t * 700., 19_000., 15_000.]);
    late.heading_deg = -90.;
    late.valid = tick < 1_800;
    let mut surface = target_view(14, 2, [5_000., 50., 30_000.]);
    surface.is_aircraft = false;
    surface.is_fighter = false;
    surface.speed = ScalarSpeed(0.);
    let friendly = target_view(5, 1, [3_000., 20_000., 2_000. + t * 700.]);
    let mut targets = vec![circling, head_on, surface, friendly];
    if tick >= 600 {
        targets.push(late);
    }
    targets
}

fn controller_frame<'a>(
    scripted: &ScriptedController,
    tick: u64,
    targets: &'a [TargetView],
    events: &'a [FrameEvent],
    stations: &'a [StationView],
    dispensers: &'a [DispenserStore],
    leader: Option<&PointMass>,
) -> DecisionFrame<'a> {
    let endurance = 3_000. - tick as f64 * 0.9;
    let radar = !(1_200..1_500).contains(&tick);
    let damaged = events.iter().any(|event| matches!(event, FrameEvent::Hit)) || tick > 2_100;
    DecisionFrame {
        tick,
        own: scripted.ship.own(endurance, radar, damaged),
        targets,
        events,
        stations,
        dispensers,
        wing: WingView {
            control: WingControl::Loose,
            formation: Formation::Echelon,
            horizontal_spacing_ft: 2_048,
            vertical_spacing_ft: 512,
            slot: if scripted.leader { 1 } else { 2 },
            leader: leader.map(|ship| LeaderView {
                position: ship.position,
                velocity: Basis::new(ship.heading.to_radians(), ship.pitch.to_radians(), 0.)
                    .forward
                    .map(|axis| axis * ship.speed),
                heading_deg: ship.heading,
                speed: ScalarSpeed(ship.speed),
                target: None,
                recovering: false,
                on_ground: false,
            }),
            wingmen_in_formation: u32::from(scripted.leader),
            wing_combat: !targets.is_empty(),
            wing_approach: false,
            wing_approach_value_ft: None,
        },
        route: RouteView {
            home_airport: Some(Position { x: 0., z: 0. }),
            leader_is_ai: true,
        },
        now: TimeOfDay(tick),
        flight_state: FlightState::Free,
    }
}

fn threat_report(missile: u32, seeker: SeekerClass, launcher: u32, tick: u64) -> ThreatReport {
    ThreatReport {
        missile_id: missile,
        seeker,
        launcher_id: launcher,
        launcher_same_side: false,
        distance_at_launch_ft: 18_000.,
        launch_tick: tick,
    }
}

fn controllers(probe: &Probe) -> (u64, Coverage) {
    let mut fp = Fingerprint::default();
    let mut coverage = Coverage::default();
    let dispensers = || {
        vec![
            DispenserStore {
                class: SeekerClass::Infrared,
                count: 6,
            },
            DispenserStore {
                class: SeekerClass::Radar,
                count: 6,
            },
        ]
    };
    let scripted =
        |id, wing, member, aircraft, level, seed, position: [f64; 3], heading| ScriptedController {
            controller: Controller::new(
                identity(id, 1, wing, member, aircraft),
                fighter(),
                resolved(level),
                seed,
            )
            .unwrap(),
            ship: PointMass {
                position,
                heading,
                pitch: 0.,
                bank: 0.,
                speed: 750.,
            },
            stations: controller_stations(),
            dispensers: dispensers(),
            events: Vec::new(),
            leader: member == 0,
        };
    let mut leader = scripted(
        1,
        0,
        0,
        AircraftId::F18,
        Experience::Experienced,
        0x51,
        [0., 20_000., 0.],
        0.,
    );
    leader.events = vec![
        (
            900,
            FrameEvent::ThreatReported(threat_report(71, SeekerClass::Radar, 12, 890)),
        ),
        (1_100, FrameEvent::Hit),
        (1_800, FrameEvent::TargetUnavailable(13)),
    ];
    let mut wingman = scripted(
        2,
        0,
        1,
        AircraftId::Mig29,
        Experience::Novice,
        0x52,
        [-2_000., 19_500., -2_500.],
        0.,
    );
    wingman.events = vec![
        (
            1_500,
            FrameEvent::ThreatReported(threat_report(72, SeekerClass::Infrared, 11, 1_490)),
        ),
        (2_100, FrameEvent::Hit),
        (1_810, FrameEvent::ActorRemoved(13)),
    ];
    let mut ace = scripted(
        3,
        1,
        0,
        AircraftId::Su27,
        Experience::Ace,
        0x53,
        [8_000., 18_000., -6_000.],
        20.,
    );
    ace.events = vec![
        (
            600,
            FrameEvent::ThreatReported(threat_report(73, SeekerClass::Radar, 13, 600)),
        ),
        (
            1_000,
            FrameEvent::ThreatReported(threat_report(74, SeekerClass::Infrared, 11, 995)),
        ),
    ];
    let mut crew = [leader, wingman, ace];
    for tick in 0..3_000u64 {
        let targets = scripted_targets(tick);
        orders_for_scripted_ace(&mut crew[2], tick, &mut fp, &mut coverage);
        let leader_ship = crew[0].ship;
        for (index, scripted) in crew.iter_mut().enumerate() {
            let events: Vec<FrameEvent> = scripted
                .events
                .iter()
                .filter(|(at, _)| *at == tick)
                .map(|(_, event)| *event)
                .collect();
            // Point the stores at the retained target, or the nearest one,
            // as the mission host does.
            let own = scripted.ship.own(0., true, false);
            let aim = scripted
                .controller
                .target()
                .and_then(|id| targets.iter().find(|t| t.id == id))
                .or_else(|| {
                    targets.iter().min_by(|a, b| {
                        distance(own.position, a.position)
                            .total_cmp(&distance(own.position, b.position))
                    })
                });
            let stations: Vec<StationView> = scripted
                .stations
                .iter()
                .map(|(station, rounds)| {
                    let mut view = *station;
                    if !matches!(view.rounds, Rounds::Finite(0)) {
                        view.rounds = Rounds::Finite(*rounds);
                    }
                    view.employment_fit = aim.and_then(|t| view.employment_error(&own, t));
                    view.pointing_error_deg = view.employment_fit.unwrap_or(180.);
                    view
                })
                .collect();
            let leader_view = (index == 1).then_some(&leader_ship);
            let frame = controller_frame(
                scripted,
                tick,
                &targets,
                &events,
                &stations,
                &scripted.dispensers,
                leader_view,
            );
            let batch = scripted.controller.step(&frame).unwrap();
            let floor = scripted.controller.terrain_floor(&frame).unwrap();
            fp.count(index);
            record_batch(&mut fp, &batch);
            fp.option(floor, |fp, floor| fp.f64(floor));
            if let Some(motion) = &batch.motion {
                let request = scripted.controller.steering_request(&frame, motion, floor);
                fp.f64(request.heading_deg);
                fp.f64(request.flight_path_pitch_deg);
                fp.f64(request.bank_deg);
                fp.option(request.terrain_pitch_floor_deg, |fp, floor| fp.f64(floor));
                fp.name(&request.mode);
            }
            record_controller(&mut fp, &scripted.controller);
            if let Some(activity) = batch.activity {
                coverage.activities.insert(activity.label());
            }
            coverage.launches += batch.weapons.len() as u32;
            coverage.devices += u32::from(batch.devices.is_some());
            coverage.wing_requests += batch.wing.len() as u32;
            // The host debits what the controller asked for.
            for weapon in &batch.weapons {
                if let Some((_, rounds)) = scripted
                    .stations
                    .iter_mut()
                    .find(|(station, _)| station.station == weapon.request.station)
                {
                    *rounds = rounds.saturating_sub(1);
                }
            }
            if let Some(devices) = batch.devices
                && let Some(dispenser) = scripted
                    .dispensers
                    .iter_mut()
                    .find(|dispenser| dispenser.class == devices.class)
            {
                dispenser.count = dispenser.count.saturating_sub(u32::from(devices.count));
            }
            scripted.ship.follow(batch.motion.as_ref());
            fp.vector(scripted.ship.position);
        }
        if tick % 300 == 299 {
            probe.part(format!("tick {tick}"), fp.value());
        }
    }
    (fp.value(), coverage)
}

/// Orders and mission overrides delivered to the third controller, the way a
/// wing leader and the mission layer would.
fn orders_for_scripted_ace(
    scripted: &mut ScriptedController,
    tick: u64,
    fp: &mut Fingerprint,
    coverage: &mut Coverage,
) {
    let heading = scripted.ship.heading;
    let controller = &mut scripted.controller;
    let order = match tick {
        240 => Some(WingRequest::WingControl(WingControl::Tight)),
        480 => Some(WingRequest::Spacing {
            axis: SpacingAxis::Horizontal,
            feet: 3_000,
        }),
        720 => Some(WingRequest::Break {
            heading_offset_deg: 90,
            pitch_deg: 10,
        }),
        960 => Some(WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(
            TargetId(12),
        ))),
        1_200 => Some(WingRequest::Approach {
            heading_deg: 45,
            pitch_deg: 5,
            speed: ScalarSpeed(0.),
        }),
        2_700 => Some(WingRequest::FormationSelection(Formation::LineAstern)),
        2_900 => Some(WingRequest::TargetAssignment(TargetOrder::HoldFire)),
        2_950 => Some(WingRequest::TargetAssignment(TargetOrder::FreeSelection)),
        _ => None,
    };
    if let Some(order) = order {
        controller.prepare_order(heading, limits());
        let outcome = controller.receive_order(order, tick);
        if matches!(
            outcome,
            Ok(ReceiverOutcome::Applied(_) | ReceiverOutcome::MotionInstalled(_))
        ) {
            coverage.orders_applied += 1;
        }
        record_order_result(fp, Some(outcome));
    }
    match tick {
        1_300 => controller.track_ordered_approach(11, 30., 5.),
        1_440 => controller.set_defense_motion(Some(DefenseMotion {
            source: DefenseSource::Missile,
            heading_deg: 200.,
            pitch_deg: -10.,
            rotorcraft: false,
            extend: false,
        })),
        1_560 => controller.set_defense_motion(None),
        1_680 => controller.set_search_contact(Some(SearchContact {
            id: 13,
            position: [9_000., 19_000., 15_000.],
            observed_tick: 1_600,
        })),
        1_800 => controller.set_search_contact(None),
        1_900 => controller.set_mission_target(Some(11)),
        2_000 => controller.set_mission_rejoin(Some([0., 20_000., 30_000.])),
        2_200 => controller.set_mission_rejoin(None),
        2_300 => controller.set_mission_target(None),
        2_400 => controller.set_mission_search_bearing(Some(135.)),
        2_600 => controller.set_mission_search_bearing(None),
        2_760 => controller.return_to_formation(),
        2_800 => controller.set_experience(resolved(Experience::Novice)),
        2_850 => controller.set_mission_hold_fire(true),
        2_875 => controller.set_mission_hold_fire(false),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Scenario: a two-sided engagement through a small synthetic host

/// What a scenario actually exercised, so a later change cannot quietly turn
/// a scenario into one that no longer covers what its name promises.
#[derive(Default)]
struct Coverage {
    activities: BTreeSet<&'static str>,
    launches: u32,
    gun_bursts: u32,
    devices: u32,
    decoy_rolls: u32,
    kills: u32,
    defenses: u32,
    ejections: u32,
    orders_applied: u32,
    wing_requests: u32,
    airfield_phases: BTreeSet<&'static str>,
}

impl Coverage {
    fn see(&mut self, mission: &AiMission) {
        for actor in mission.actors() {
            self.activities.insert(actor.activity().label());
            if actor.defense_decision().is_some() {
                self.defenses += 1;
            }
            if let Some(phase) = actor.airfield_phase() {
                self.airfield_phases.insert(match phase {
                    crate::ai::airfield::Phase::Waiting => "waiting",
                    crate::ai::airfield::Phase::Taxi => "taxi",
                    crate::ai::airfield::Phase::LineUp => "line up",
                    crate::ai::airfield::Phase::TakeoffRoll => "takeoff roll",
                    crate::ai::airfield::Phase::ClimbOut => "climb out",
                    crate::ai::airfield::Phase::Inbound => "inbound",
                    crate::ai::airfield::Phase::Marshal => "marshal",
                    crate::ai::airfield::Phase::Approach => "approach",
                    crate::ai::airfield::Phase::Final => "final",
                    crate::ai::airfield::Phase::Rollout => "rollout",
                    crate::ai::airfield::Phase::TaxiClear => "taxi clear",
                    crate::ai::airfield::Phase::Parked => "parked",
                    crate::ai::airfield::Phase::VerticalHold => "vertical hold",
                    crate::ai::airfield::Phase::VerticalClimb => "vertical climb",
                    crate::ai::airfield::Phase::Transition => "transition",
                    crate::ai::airfield::Phase::HoverApproach => "hover approach",
                    crate::ai::airfield::Phase::VerticalDescent => "vertical descent",
                    crate::ai::airfield::Phase::Settle => "settle",
                    crate::ai::airfield::Phase::AirTaxi => "air taxi",
                });
            }
        }
    }

    fn require_engagement(&self) {
        let summary = format!("{:?}", self.activities);
        for activity in ["In formation", "Attacking", "Defending", "Destroyed"] {
            assert!(
                self.activities.contains(activity),
                "the engagement scenario never showed {activity}: {summary}"
            );
        }
        for (what, count) in [
            ("missile launches", self.launches),
            ("gun bursts", self.gun_bursts),
            ("countermeasure releases", self.devices),
            ("decoy rolls", self.decoy_rolls),
            ("kills", self.kills),
            ("ejections", self.ejections),
            ("missile defenses", self.defenses),
            ("applied orders", self.orders_applied),
            ("wing requests", self.wing_requests),
        ] {
            assert!(count > 0, "the engagement scenario produced no {what}");
        }
    }

    fn require_controllers(&self) {
        let summary = format!("{:?}", self.activities);
        for activity in [
            "Pursuing",
            "Defending",
            "Searching",
            "Rejoining",
            "Returning to base",
        ] {
            assert!(
                self.activities.contains(activity),
                "the controller scenario never showed {activity}: {summary}"
            );
        }
        for (what, count) in [
            ("weapon releases", self.launches),
            ("countermeasure requests", self.devices),
            ("applied orders", self.orders_applied),
            ("wing requests", self.wing_requests),
        ] {
            assert!(count > 0, "the controller scenario produced no {what}");
        }
    }

    fn require_airfield(&self) {
        for phase in [
            "waiting",
            "line up",
            "takeoff roll",
            "climb out",
            "marshal",
            "approach",
            "final",
            "rollout",
            "taxi clear",
            "parked",
        ] {
            assert!(
                self.airfield_phases.contains(phase),
                "the airfield scenario never reached {phase}: {:?}",
                self.airfield_phases
            );
        }
    }
}

/// One mission store. The ordnance values are synthetic.
fn store(station: u8, kind: SeekerClass, gun: bool, rounds: u32, radar: bool) -> StationSpec {
    if gun {
        return StationSpec {
            station: StationId(station),
            guided: false,
            capability: StoreCapability::GUN,
            store: StoreState {
                inhibited: false,
                rounds: Rounds::Finite(rounds),
            },
            debit: 10,
            external_round_lbs: 0.,
            projectile_count: 10,
            employment_limit_deg: Some(12.),
            damage_vs_category: 10.,
            store_speed: ScalarSpeed(3_000.),
            tracking_delay: Delay::quarters(1),
            pacing: pacing(10, Delay::quarters(1), Delay::quarters(2)),
            maximum_range_ft: Some(9_000.),
            energy_target: None,
            minimum_range_ft: 0.,
            infrared_envelope: None,
            requires_radar: false,
            requires_sensor: false,
            employment_zone: None,
            mount: [0., 0., 10.],
        };
    }
    let infrared = kind == SeekerClass::Infrared;
    StationSpec {
        station: StationId(station),
        guided: true,
        capability: StoreCapability::AIR_TO_AIR_MISSILE,
        store: StoreState {
            inhibited: false,
            rounds: Rounds::Finite(rounds),
        },
        debit: 1,
        external_round_lbs: if infrared { 190. } else { 350. },
        projectile_count: 1,
        employment_limit_deg: (!infrared).then_some(40.),
        damage_vs_category: 100.,
        store_speed: ScalarSpeed(2_400.),
        tracking_delay: if infrared {
            Delay::quarters(2)
        } else {
            Delay::seconds(1)
        },
        pacing: pacing(
            1,
            Delay::seconds(0),
            Delay::seconds(if infrared { 2 } else { 4 }),
        ),
        maximum_range_ft: Some(if infrared { 14_000. } else { 42_000. }),
        energy_target: None,
        minimum_range_ft: if infrared { 1_000. } else { 2_000. },
        infrared_envelope: None,
        requires_radar: radar && !infrared,
        requires_sensor: infrared,
        employment_zone: infrared.then(|| zone(30., 30., (1_000, 14_000), (-15_000, 15_000))),
        mount: [if infrared { -4. } else { 4. }, -2., 0.],
    }
}

struct Pilot {
    id: u32,
    side: u32,
    wing: u8,
    member: u8,
    aircraft: AircraftId,
    level: Experience,
    position: [f64; 3],
    heading_deg: f64,
    sensors: bool,
    researched: bool,
    guns_only: bool,
    home: Option<Position>,
}

fn build_actor(pilot: &Pilot) -> AiActor {
    let heading = pilot.heading_deg.to_radians();
    let mut state = flight::State::new(&fixtures::aircraft(pilot.aircraft), pilot.position)
        .expect("synthetic aircraft");
    state.yaw = heading;
    state.velocity = Basis::new(heading, 0., 0.)
        .forward
        .map(|axis| axis * state.speed);
    if pilot.researched {
        state.enable_research(pilot.id as i32 * 17 + 3).unwrap();
    }
    let radar = pilot.sensors;
    let mut stations = vec![
        store(0, SeekerClass::Radar, false, 4, radar),
        store(1, SeekerClass::Infrared, false, 2, radar),
        store(2, SeekerClass::Radar, true, 500, radar),
    ];
    if pilot.level == Experience::Novice {
        stations.remove(1);
    }
    if pilot.guns_only {
        stations.retain(|station| !station.guided);
    }
    let payload: f64 = stations
        .iter()
        .map(|s| {
            s.external_round_lbs
                * match s.store.rounds {
                    Rounds::Finite(n) => f64::from(n),
                    Rounds::Unlimited => 0.,
                }
        })
        .sum();
    state.set_payload(payload).unwrap();
    AiActor::new(ActorSetup {
        identity: identity(
            pilot.id,
            pilot.side,
            pilot.wing,
            pilot.member,
            pilot.aircraft,
        ),
        profile: fighter(),
        experience: resolved(pilot.level),
        seed: u64::from(pilot.id) * 0x9e37 + 11,
        flight: state,
        sensors: pilot
            .sensors
            .then(|| Sensors::new(fixtures::sensor_profiles(pilot.aircraft))),
        stations,
        dispensers: vec![
            DispenserStore {
                class: SeekerClass::Infrared,
                count: 10,
            },
            DispenserStore {
                class: SeekerClass::Radar,
                count: 10,
            },
        ],
        wing_slot: pilot.member.max(1),
        home_airport: pilot.home,
    })
    .expect("fighter actor")
}

/// Every actor as a world object, with the observable a sensor needs.
fn world_objects(mission: &AiMission) -> Vec<WorldObject> {
    mission
        .actors()
        .iter()
        .map(|actor| {
            let f = actor.flight();
            let on_ground = f.research.as_ref().is_some_and(|r| r.on_ground);
            let alive = actor.alive();
            WorldObject {
                infrared_strength: 1.,
                id: actor.id(),
                side: actor.identity().side,
                position: f.position,
                velocity: f.velocity,
                heading_deg: f.yaw.to_degrees(),
                pitch_deg: f.pitch.to_degrees(),
                speed: ScalarSpeed(f.speed),
                maximum_speed: ScalarSpeed(1_600.),
                is_aircraft: true,
                is_fighter: true,
                human_controlled: false,
                alive,
                destroyed: !alive,
                on_ground,
                observable: Some(
                    Observable {
                        id: actor.id(),
                        position: f.position,
                        velocity: f.velocity,
                        basis: Basis::new(f.yaw, f.pitch, f.bank),
                        configuration: sensors::Configuration::default(),
                        signature: sensors::SignatureProfile::default(),
                        jammer: None,
                        jammer_active: false,
                        radar_emitting: f.radar && alive,
                        airborne: !on_ground,
                        destroyed: !alive,
                    }
                    .on_ground(on_ground),
                ),
            }
        })
        .collect()
}

/// A missile flown by the synthetic host: constant speed, a turn-rate
/// limited pure pursuit and a proximity fuze. The simulation's own combat
/// layer has its own goldens; this one only has to be deterministic.
struct HostMissile {
    id: u32,
    owner: u32,
    target: Option<u32>,
    guidance: Guidance,
    position: [f64; 3],
    direction: [f64; 3],
    age: u64,
}

const MISSILE_SPEED_FPS: f64 = 2_400.;

struct Host {
    mission: AiMission,
    missiles: Vec<HostMissile>,
    next_missile: u32,
    hit_points: Vec<(u32, i32)>,
    decoys: DecisionRandom,
    coverage: Coverage,
}

impl Host {
    fn actor_position(&self, id: u32) -> Option<[f64; 3]> {
        self.mission
            .actor(id)
            .filter(|actor| actor.alive())
            .map(|actor| actor.flight().position)
    }

    fn radar_active(&self, missile: &HostMissile) -> bool {
        missile.guidance == Guidance::Active
            && missile
                .target
                .and_then(|id| self.actor_position(id))
                .is_some_and(|target| {
                    distance(missile.position, target) <= 5. * sensors::FEET_PER_NAUTICAL_MILE
                })
    }

    fn supported(&self, missile: &HostMissile) -> bool {
        missile.guidance == Guidance::Supported
            && missile.target.is_some()
            && self
                .mission
                .actor(missile.owner)
                .is_some_and(|owner| owner.alive() && owner.flight().radar)
    }

    fn snapshots(&self) -> Vec<MissileSnapshot> {
        self.missiles
            .iter()
            .map(|missile| {
                let active = self.radar_active(missile);
                let acquired = active
                    && missile
                        .target
                        .and_then(|id| self.actor_position(id))
                        .is_some_and(|target| {
                            dot(
                                unit(std::array::from_fn(|i| target[i] - missile.position[i])),
                                missile.direction,
                            ) > 30_f64.to_radians().cos()
                        });
                let supported = self.supported(missile);
                MissileSnapshot {
                    id: missile.id,
                    owner: missile.owner,
                    position: missile.position,
                    velocity: missile.direction.map(|axis| axis * MISSILE_SPEED_FPS),
                    guidance: missile.guidance,
                    target: missile.target,
                    radar_active: active,
                    radar_acquired: acquired,
                    supported,
                    supporting_radar_position: supported
                        .then(|| self.actor_position(missile.owner))
                        .flatten(),
                    alive: true,
                }
            })
            .collect()
    }

    fn damage(&mut self, victim: u32, amount: i32, blast: Option<[f64; 3]>, fp: &mut Fingerprint) {
        let Some(entry) = self.hit_points.iter_mut().find(|(id, _)| *id == victim) else {
            return;
        };
        entry.1 -= amount;
        let remaining = entry.1;
        let Some(actor) = self.mission.actor_mut(victim) else {
            return;
        };
        if !actor.alive() {
            return;
        }
        actor.report_hit();
        let section = (victim as usize + remaining.unsigned_abs() as usize) % 6;
        let flight = actor.flight_mut();
        if let Some(from) = blast {
            flight.jolt_from(from, f64::from(amount) / 100.);
        }
        flight.damage_regions[section] = (flight.damage_regions[section] + 0.4).min(1.);
        flight.damage_fraction = (1. - f64::from(remaining.max(0)) / 100.).clamp(0., 1.);
        if remaining <= 0 {
            flight.crashed = true;
            actor.set_alive(false);
            for other in self.mission.actors_mut() {
                other.report_removed(victim);
            }
            self.coverage.kills += 1;
        }
        fp.int(victim);
        fp.int(remaining);
    }

    fn realise_launches(&mut self, output: &MissionOutput, tick: u64, fp: &mut Fingerprint) {
        for launch in &output.launches {
            let Some(shooter) = self.mission.actor(launch.actor) else {
                continue;
            };
            let Some(station) = shooter
                .stations()
                .iter()
                .find(|s| s.station == launch.station)
            else {
                continue;
            };
            let f = shooter.flight();
            let forward = Basis::new(f.yaw, f.pitch, f.bank).forward;
            let origin = f.position;
            let side = shooter.identity().side;
            let guided = station.guided;
            let infrared = station.employment_zone.is_some();
            let Some(target) = self.actor_position(launch.target) else {
                continue;
            };
            if !guided {
                // A gun burst is resolved at once against the nose geometry.
                self.coverage.gun_bursts += 1;
                let to_target = unit(std::array::from_fn(|i| target[i] - origin[i]));
                let hit = distance(origin, target) <= 5_000.
                    && dot(forward, to_target) >= 5_f64.to_radians().cos();
                fp.bool(hit);
                if hit {
                    self.damage(launch.target, 15, None, fp);
                }
                continue;
            }
            self.coverage.launches += 1;
            let guidance = if infrared {
                Guidance::Infrared
            } else if side == Side(1) {
                Guidance::Active
            } else {
                Guidance::Supported
            };
            let id = self.next_missile;
            self.next_missile += 1;
            self.missiles.push(HostMissile {
                id,
                owner: launch.actor,
                target: Some(launch.target),
                guidance,
                position: std::array::from_fn(|i| origin[i] + forward[i] * 30.),
                direction: forward,
                age: 0,
            });
            if let Some(victim) = self.mission.actor_mut(launch.target) {
                victim.report_threat(ThreatReport {
                    missile_id: id,
                    seeker: if infrared {
                        SeekerClass::Infrared
                    } else {
                        SeekerClass::Radar
                    },
                    launcher_id: launch.actor,
                    launcher_same_side: false,
                    distance_at_launch_ft: distance(origin, target),
                    launch_tick: tick,
                });
            }
        }
    }

    fn decoy(&mut self, output: &MissionOutput, fp: &mut Fingerprint) {
        for device in &output.devices {
            self.coverage.devices += 1;
            for index in 0..self.missiles.len() {
                if self.missiles[index].target != Some(device.actor) {
                    continue;
                }
                let missile = &self.missiles[index];
                let (seeker, guiding) = match missile.guidance {
                    Guidance::Infrared => (SeekerClass::Infrared, true),
                    Guidance::Active => (SeekerClass::Radar, self.radar_active(missile)),
                    _ => (SeekerClass::Radar, self.supported(missile)),
                };
                for _ in 0..device.released {
                    self.coverage.decoy_rolls += 1;
                    let outcome = threat::decoy_missile(
                        &GuidingMissile {
                            seeker,
                            guiding_on_releaser: guiding,
                            decoy_susceptibility_percent: 35,
                        },
                        device.class,
                        100,
                        &mut self.decoys,
                    )
                    .unwrap();
                    fp.name(&outcome);
                    if outcome == DecoyOutcome::Decoyed {
                        self.missiles[index].target = None;
                        break;
                    }
                }
            }
        }
    }

    fn fly_missiles(&mut self, fp: &mut Fingerprint) {
        let turn = 30_f64.to_radians() * flight::DT;
        let mut index = 0;
        while index < self.missiles.len() {
            let target = self.missiles[index]
                .target
                .and_then(|id| self.actor_position(id));
            let missile = &mut self.missiles[index];
            if let Some(target) = target {
                let desired = unit(std::array::from_fn(|i| target[i] - missile.position[i]));
                let angle = dot(missile.direction, desired).clamp(-1., 1.).acos();
                missile.direction = if angle <= turn {
                    desired
                } else {
                    let share = turn / angle;
                    unit(std::array::from_fn(|i| {
                        missile.direction[i] + (desired[i] - missile.direction[i]) * share
                    }))
                };
            }
            for (position, axis) in missile.position.iter_mut().zip(missile.direction) {
                *position += axis * MISSILE_SPEED_FPS * flight::DT;
            }
            missile.age += 1;
            let (id, owner, position, age) =
                (missile.id, missile.owner, missile.position, missile.age);
            fp.int(id);
            fp.vector(position);
            let fused = target.filter(|target| distance(position, *target) <= 120.);
            let spent =
                age >= 30 * 120 || position[1] <= fixtures::terrain(position[0], position[2]);
            if let (Some(_), Some(victim)) = (fused, self.missiles[index].target) {
                self.missiles.remove(index);
                fp.int(owner);
                self.damage(victim, 100, Some(position), fp);
            } else if spent {
                self.missiles.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Orders, wing settings, radar switching and host-reported damage at
    /// fixed ticks.
    fn scripted_events(&mut self, tick: u64, fp: &mut Fingerprint) {
        let mut outcomes = Vec::new();
        match tick {
            120 => {
                outcomes = self
                    .mission
                    .order_wing_report(
                        Side(1),
                        0,
                        None,
                        Some(3),
                        WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(TargetId(14))),
                    )
                    .unwrap();
            }
            360 => {
                self.mission.set_wing_control(WingControl::Medium);
                self.mission.set_spacing(3_000, 400);
            }
            600 => {
                outcomes = self
                    .mission
                    .order_wing_report(
                        Side(1),
                        0,
                        Some(1),
                        None,
                        WingRequest::Spacing {
                            axis: SpacingAxis::Vertical,
                            feet: -800,
                        },
                    )
                    .unwrap();
            }
            900 => {
                outcomes = self
                    .mission
                    .order_wing_report(
                        Side(2),
                        0,
                        None,
                        Some(12),
                        WingRequest::Break {
                            heading_offset_deg: -70,
                            pitch_deg: 5,
                        },
                    )
                    .unwrap();
            }
            1_800 => {
                let outcome = self.mission.order(
                    2,
                    WingRequest::Approach {
                        heading_deg: 30,
                        pitch_deg: 0,
                        speed: ScalarSpeed(0.),
                    },
                );
                record_order_result(fp, outcome);
            }
            2_000 => {
                if let Some(actor) = self.mission.actor_mut(1) {
                    actor.flight_mut().radar = false;
                }
            }
            2_400 => {
                self.mission.set_formation(Formation::LineAbreast);
                outcomes = self
                    .mission
                    .order_wing_report(
                        Side(1),
                        0,
                        None,
                        None,
                        WingRequest::FormationSelection(Formation::LineAbreast),
                    )
                    .unwrap();
            }
            2_600 => {
                if let Some(actor) = self.mission.actor_mut(1) {
                    actor.flight_mut().radar = true;
                }
            }
            3_000 => {
                outcomes = self
                    .mission
                    .order_wing_report(
                        Side(1),
                        0,
                        None,
                        None,
                        WingRequest::TargetAssignment(TargetOrder::FreeSelection),
                    )
                    .unwrap();
            }
            3_600 => {
                if let Some(actor) = self.mission.actor_mut(12) {
                    actor.set_experience(resolved(Experience::Ace));
                }
            }
            4_200 => {
                let outcome = self
                    .mission
                    .order(11, WingRequest::TargetAssignment(TargetOrder::HoldFire));
                record_order_result(fp, outcome);
            }
            _ => {}
        }
        // Damage the host's own combat layer would report: a kill, a partial
        // hit with a blast, and a second kill.
        let (victim, amount, blast) = match tick {
            1_800 => (4, 100, true),
            3_000 => (13, 40, true),
            4_800 => (12, 100, false),
            _ => (0, 0, false),
        };
        if amount > 0
            && let Some(position) = self.actor_position(victim)
        {
            let from = blast.then_some([position[0] + 40., position[1] - 30., position[2]]);
            self.damage(victim, amount, from, fp);
        }
        fp.count(outcomes.len());
        for (id, outcome) in &outcomes {
            fp.int(*id);
            record_receiver_outcome(fp, outcome);
            if matches!(
                outcome,
                ReceiverOutcome::Applied(_) | ReceiverOutcome::MotionInstalled(_)
            ) {
                self.coverage.orders_applied += 1;
            }
        }
    }
}

fn mission_engagement(probe: &Probe) -> (u64, Coverage) {
    mission_engagement_observed(probe, &mut |_, _| {}, &mut |_| {})
}

/// The engagement scenario with `before` called on the mission at the start
/// of every tick, before the host reads it, and `observe` right after every
/// step, before the host acts on the output.
fn mission_engagement_observed(
    probe: &Probe,
    before: &mut dyn FnMut(u64, &mut AiMission),
    observe: &mut dyn FnMut(&mut AiMission),
) -> (u64, Coverage) {
    let pilot = |id, side, wing, member, aircraft, level, position, heading_deg| Pilot {
        id,
        side,
        wing,
        member,
        aircraft,
        level,
        position,
        heading_deg,
        sensors: false,
        researched: false,
        guns_only: false,
        home: Some(Position { x: 0., z: -30_000. }),
    };
    let mut mission = AiMission::new();
    // Blue wing 0 starts in formation, as a Quick Mission wing does.
    let mut blue_leader = pilot(
        1,
        1,
        0,
        0,
        AircraftId::F18,
        Experience::Experienced,
        [0., 20_000., 0.],
        0.,
    );
    blue_leader.sensors = true;
    let mut blue_three = pilot(
        3,
        1,
        0,
        2,
        AircraftId::Rafale,
        Experience::Ace,
        [2_000., 19_500., -2_000.],
        0.,
    );
    blue_three.researched = true;
    blue_three.guns_only = true;
    let mut blue_two = pilot(
        2,
        1,
        0,
        1,
        AircraftId::F14,
        Experience::Average,
        [-2_000., 20_500., -2_000.],
        0.,
    );
    blue_two.guns_only = true;
    for pilot in [blue_leader, blue_two, blue_three] {
        mission.push(build_actor(&pilot));
    }
    mission.start_in_formation();
    // A low-level singleton short of fuel, heading home.
    let mut singleton = build_actor(&pilot(
        4,
        1,
        1,
        0,
        AircraftId::A4E,
        Experience::Novice,
        [-15_000., 5_000., 8_000.],
        180.,
    ));
    singleton.set_internal_fuel(120.);
    mission.push(singleton);
    // Red: an ace-led pair, an escort for the leader and a training target.
    let mut red_leader = pilot(
        11,
        2,
        0,
        0,
        AircraftId::Mig29,
        Experience::Ace,
        [0., 21_000., 45_000.],
        180.,
    );
    red_leader.sensors = true;
    // The red wingman starts behind the blue singleton with guns only.
    let mut red_wingman = pilot(
        12,
        2,
        0,
        1,
        AircraftId::Su27,
        Experience::Novice,
        [-15_000., 5_200., 10_500.],
        180.,
    );
    red_wingman.researched = true;
    red_wingman.guns_only = true;
    mission.push(build_actor(&red_leader));
    mission.push(build_actor(&red_wingman));
    let mut escort = pilot(
        13,
        2,
        1,
        0,
        AircraftId::Mig21,
        Experience::Experienced,
        [-3_000., 22_000., 48_000.],
        180.,
    );
    escort.guns_only = true;
    let mut escort = build_actor(&escort);
    escort.set_assignment(Assignment {
        role: Role::Escort,
        stance: Stance::ProtectAssigned,
        protected_ids: vec![11],
        ..Assignment::default()
    });
    mission.push(escort);
    let mut dummy = build_actor(&pilot(
        14,
        2,
        2,
        0,
        AircraftId::Su25,
        Experience::Average,
        [1_500., 17_500., 9_000.],
        0.,
    ));
    dummy.set_dummy();
    mission.push(dummy);

    let hit_points = mission.actors().iter().map(|a| (a.id(), 100)).collect();
    let mut host = Host {
        mission,
        missiles: Vec::new(),
        next_missile: 1_000,
        hit_points,
        decoys: DecisionRandom::seeded(0x0dec_0de5),
        coverage: Coverage::default(),
    };
    let mut fp = Fingerprint::default();
    let terrain = fixtures::terrain;
    let surface = |x: f64, z: f64| Surface::terrain(fixtures::terrain(x, z));
    for tick in 0..7_200u64 {
        before(tick, &mut host.mission);
        host.scripted_events(tick, &mut fp);
        let world = world_objects(&host.mission);
        host.mission.set_missiles(host.snapshots());
        let output = host
            .mission
            .step_with_surface(&world, &terrain, &surface, TimeOfDay(tick))
            .expect("mission step");
        observe(&mut host.mission);
        record_output(&mut fp, &output);
        host.coverage.wing_requests += output.wing.len() as u32;
        host.realise_launches(&output, tick, &mut fp);
        host.decoy(&output, &mut fp);
        host.fly_missiles(&mut fp);
        for actor in host.mission.actors() {
            record_actor(&mut fp, actor);
            if actor
                .flight()
                .escape
                .as_ref()
                .is_some_and(|escape| escape.ticks == 1)
            {
                host.coverage.ejections += 1;
            }
        }
        host.coverage.see(&host.mission);
        if tick % 600 == 599 {
            probe.part(format!("tick {tick}"), fp.value());
        }
    }
    (fp.value(), host.coverage)
}

// ---------------------------------------------------------------------------
// Scenario: airfield departure and landing

const AIRPORT: u32 = 7;
const HUMAN_LEADER: u32 = 10;

fn anchored_runway() -> RunwayView {
    let at = |x: f64, z: f64| [x, 0., z];
    RunwayView {
        airport: AIRPORT,
        object: 70,
        center: [0., 0., 0.],
        heading: 0.,
        length_ft: 8_000.,
        elevation_ft: 0.,
        anchors: Some(AirfieldAnchors {
            taxi_out: [
                at(1_200., -1_000.),
                at(600., -1_000.),
                at(600., -4_100.),
                at(0., -4_070.),
            ],
            takeoff_spot: at(0., -3_700.),
            takeoff_heading: 0.,
            landing_point: at(0., -2_000.),
            landing_heading: 0.,
            taxi_in: [
                at(0., 1_000.),
                at(600., 1_300.),
                at(600., 0.),
                at(1_200., -500.),
            ],
            parking: std::array::from_fn(|k| at(1_500., -2_000. + 250. * k as f64)),
            parking_heading: std::f64::consts::FRAC_PI_2,
        }),
    }
}

/// The human leader as the host reports it: parked until `release_s`, then
/// a takeoff roll at 8 ft/s^2, liftoff at 250 ft/s and a 20 ft/s climb.
fn human_leader(t: f64, release_s: f64) -> WorldObject {
    let rolling = (t - release_s).max(0.);
    let speed = (8. * rolling).min(400.);
    let liftoff = 250. / 8.;
    let along = if rolling < liftoff {
        4. * rolling * rolling
    } else {
        4. * liftoff * liftoff + 250. * (rolling - liftoff) + 4. * (rolling - liftoff).powi(2)
    };
    let climb = (rolling - liftoff).max(0.) * 20.;
    let position = [0., climb, -3_700. + along];
    WorldObject {
        infrared_strength: 1.,
        id: HUMAN_LEADER,
        side: Side(1),
        position,
        velocity: [0., if climb > 0. { 20. } else { 0. }, speed],
        heading_deg: 0.,
        pitch_deg: 0.,
        speed: ScalarSpeed(speed),
        maximum_speed: ScalarSpeed(1_600.),
        is_aircraft: true,
        is_fighter: true,
        human_controlled: true,
        alive: true,
        destroyed: false,
        on_ground: climb <= 0.,
        observable: None,
    }
}

fn airfield(probe: &Probe) -> (u64, Coverage) {
    airfield_observed(probe, &mut |_, _| {}, &mut |_| {})
}

/// The airfield scenario with `before` called on the mission at the start of
/// every tick of each part (the tick counted from that part's start) and
/// `observe` right after every step.
fn airfield_observed(
    probe: &Probe,
    before: &mut dyn FnMut(u64, &mut AiMission),
    observe: &mut dyn FnMut(&mut AiMission),
) -> (u64, Coverage) {
    let mut fp = Fingerprint::default();
    let mut coverage = Coverage::default();
    let terrain = |x: f64, z: f64| fixtures::runway_surface(x, z).height;
    let actor_at = |id, member, aircraft, position, heading_deg| {
        build_actor(&Pilot {
            id,
            side: 1,
            wing: 0,
            member,
            aircraft,
            level: Experience::Experienced,
            position,
            heading_deg,
            sensors: false,
            researched: false,
            guns_only: false,
            home: None,
        })
    };

    // Part one: two wingmen parked on a plain runway wait for their human
    // leader, then take off one at a time and climb out.
    let runway = RunwayView {
        anchors: None,
        ..anchored_runway()
    };
    let mut mission = AiMission::new();
    for (id, order, aircraft, position) in [
        (1, 1u8, AircraftId::F18, [40., 0., -3_250.]),
        (2, 2u8, AircraftId::F14, [-40., 0., -3_500.]),
    ] {
        let mut actor = actor_at(id, order, aircraft, position, 0.);
        actor.flight_mut().enable_research(id as i32).unwrap();
        actor.flight_mut().start_on_runway(position, 0.).unwrap();
        actor.start_on_ground(GroundStart {
            runway,
            end: ApproachEnd::Near,
            order,
        });
        mission.push(actor);
    }
    mission.set_external_leader(Side(1), 0, HUMAN_LEADER);
    mission.start_in_formation();
    for tick in 0..(150 * 120u64) {
        before(tick, &mut mission);
        let mut world = world_objects(&mission);
        world.push(human_leader(tick as f64 / 120., 5.));
        let output = mission
            .step_with_surface(&world, &terrain, &fixtures::runway_surface, TimeOfDay(tick))
            .expect("mission step");
        observe(&mut mission);
        record_output(&mut fp, &output);
        for actor in mission.actors() {
            record_actor(&mut fp, actor);
        }
        coverage.see(&mission);
        if tick % 1_200 == 1_199 {
            probe.part(format!("departure second {}", (tick + 1) / 120), fp.value());
        }
    }

    // Part two: an ordered landing at the anchored airport, held briefly at
    // marshal by the player's landing priority, then approach, final,
    // rollout and the taxi to a parking slot. 400 s since the 3 degree ILS
    // path (2026-09-29): the shallower approach takes longer than the 340 s
    // the 6 degree path needed, and the coverage check wants the parked slot.
    let runway = anchored_runway();
    let mut mission = AiMission::new();
    mission.push(actor_at(
        3,
        0,
        AircraftId::Rafale,
        [0., 3_000., -45_000.],
        0.,
    ));
    let outcome = mission.order(
        3,
        WingRequest::Land(LandingOrder {
            runway,
            reason: crate::ai::airfield::LandingReason::Ordered,
        }),
    );
    record_order_result(&mut fp, outcome);
    mission.set_priority_landing(HUMAN_LEADER, Some(AIRPORT));
    for tick in 0..(400 * 120u64) {
        before(tick, &mut mission);
        if tick == 20 * 120 {
            mission.set_priority_landing(HUMAN_LEADER, None);
        }
        let world = world_objects(&mission);
        let output = mission
            .step_with_surface(&world, &terrain, &fixtures::runway_surface, TimeOfDay(tick))
            .expect("mission step");
        observe(&mut mission);
        record_output(&mut fp, &output);
        for actor in mission.actors() {
            record_actor(&mut fp, actor);
        }
        coverage.see(&mission);
        if tick % 1_200 == 1_199 {
            probe.part(format!("landing second {}", (tick + 1) / 120), fp.value());
        }
    }
    (fp.value(), coverage)
}
