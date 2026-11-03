//! The write-only "why" records: the draw log, the controller and actor
//! traces and the message journal say what happened, and never change it.
//! Synthetic fixtures only.
use super::tests::{flat, object, setup, synthetic_profile, visible_object};
use super::*;
use crate::ai::airfield::{GroundStart, Phase, RunwayView};
use crate::ai::controller::{
    ActorIdentity, BehaviorFamily, BehaviorProfile, DecisionFrame, EmploymentFailure, MissionRole,
    RouteView, StationVerdict,
};
use crate::ai::engagement::{
    Assignment, HostileEscort, PatrolRegion, Policy, Priority, ProtectedView, Role, Stance,
};
use crate::ai::experience::ExperienceOrigin;
use crate::ai::targeting::Side;
use crate::ai::thought::{
    ActorPath, DropReason, ExpiryReason, IgnoreReason, JournalEntry, Message, MotionBranch,
    Outcome, StepPath, TargetPath, WarningStep,
};
use crate::ai::threat::{ScriptReason, ScriptStart, WarningReaction};
use crate::ai::weapon_service::{ActorId, Delay, ProjectilePacing, Rounds, StationId};
use crate::ai::wing::{ReceiverOutcome, RejectReason, TargetOrder, WingRequest};
use crate::ai::{DRAW_LOG_LIMIT, DecisionRandom, Experience};
use crate::airport::ApproachEnd;

fn entries(mission: &mut AiMission) -> Vec<JournalEntry> {
    let batch = mission.take_journal();
    assert_eq!(batch.dropped, 0, "the journal overflowed in a small test");
    batch.entries
}

fn world_of(mission: &AiMission, extra: impl IntoIterator<Item = WorldObject>) -> Vec<WorldObject> {
    mission
        .actors()
        .iter()
        .map(|actor| object(actor, actor.identity().side.0))
        .chain(extra)
        .collect()
}

fn step(mission: &mut AiMission, extra: &[WorldObject]) -> MissionOutput {
    let world = world_of(mission, extra.iter().cloned());
    let tick = mission.tick();
    mission.step(&world, &flat, TimeOfDay(tick)).unwrap()
}

// ---------------------------------------------------------------------------
// The draw log

#[test]
fn the_draw_log_names_each_draw_without_changing_any() {
    let mut labelled = DecisionRandom::seeded(42);
    let mut plain = DecisionRandom::seeded(42);
    assert_eq!(
        labelled.site("roll").chance(40),
        plain.chance(40),
        "a label never changes a draw"
    );
    assert_eq!(labelled.site("offset").range(-15, 14), plain.range(-15, 14));
    assert_eq!(labelled.site("nothing").below(0), plain.below(0));
    assert_eq!(labelled.choose(3), plain.choose(3));
    assert_eq!(labelled.percent(), plain.percent());
    assert_eq!(labelled, plain, "the same state compares equal");

    let draws: Vec<_> = labelled.log().draws().copied().collect();
    assert_eq!(draws.len(), 4, "a zero bound makes no draw: {draws:?}");
    assert_eq!(draws[0].site, Some("roll"));
    assert_eq!(draws[0].bound, 100);
    assert_eq!(draws[0].threshold, Some(40));
    assert_eq!(draws[0].passed(), Some(draws[0].value < 40));
    assert_eq!(draws[1].site, Some("offset"));
    assert_eq!(draws[1].bound, 30);
    assert_eq!(draws[1].result(), i64::from(draws[1].value) - 15);
    assert_eq!(
        draws[2].site, None,
        "the label of a draw that did not happen is not inherited"
    );
    assert_eq!(draws[2].bound, 3);
    assert_eq!(draws[3].threshold, None);
    assert!(draws[0].location.file().ends_with("thought_tests.rs"));

    labelled.clear_log();
    assert!(labelled.log().is_empty());
    assert_eq!(
        labelled, plain,
        "clearing the log leaves the generator alone"
    );
    assert_eq!(labelled.below(1_000), plain.below(1_000));

    let mut long = DecisionRandom::seeded(7);
    for _ in 0..DRAW_LOG_LIMIT + 5 {
        long.percent();
    }
    assert_eq!(long.log().len(), DRAW_LOG_LIMIT);
    assert_eq!(long.log().dropped(), 5);
}

// ---------------------------------------------------------------------------
// Controller records

fn identity(id: u32, member: u8) -> ActorIdentity {
    ActorIdentity {
        actor: ActorId(id),
        side: Side(1),
        wing: 0,
        member,
        leads: member == 0,
        aircraft: AircraftId::F18,
        human_controlled: false,
    }
}

fn controller(id: u32, member: u8) -> Controller {
    Controller::new(
        identity(id, member),
        BehaviorProfile {
            family: BehaviorFamily::FighterStrike,
            role: MissionRole::AirToAir,
        },
        ResolvedExperience {
            level: Experience::Experienced,
            origin: ExperienceOrigin::QuickMission {
                selected: Experience::Experienced,
            },
        },
        99,
    )
    .unwrap()
}

fn own_state() -> OwnState {
    OwnState {
        position: [0., 20_000., 0.],
        heading_deg: 0.,
        flight_path_pitch_deg: 0.,
        body_pitch_offset_deg: 2.,
        bank_deg: 0.,
        speed: ScalarSpeed(800.),
        limits: SpeedLimits {
            minimum: ScalarSpeed(220.),
            maximum: ScalarSpeed(1_600.),
            corner: ScalarSpeed(700.),
        },
        altitude_msl_ft: 20_000.,
        agl_ft: 20_000.,
        terrain_ahead_ft: 0.,
        terrain_climb_deg: -90.,
        minimum_altitude_ft: 300.,
        at_ceiling: false,
        on_ground: false,
        g_limit: 7.,
        roll_limit_deg_per_s: 180.,
        maximum_bank_deg: 80.,
        alive: true,
        fuel_endurance_s: 3_600.,
        time_home_s: Some(600.),
        rotorcraft: false,
        internal_fuel_lbs: 8_000.,
        radar_emitting: true,
    }
}

fn hostile(id: u32, position: [f64; 3]) -> TargetView {
    TargetView {
        infrared_strength: 1.,
        id,
        side: Side(2),
        position,
        heading_deg: 180.,
        pitch_deg: 0.,
        speed: ScalarSpeed(750.),
        maximum_speed: ScalarSpeed(1_500.),
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

fn missile_station(maximum_range_ft: f64) -> StationView {
    StationView {
        station: StationId(0),
        guided: true,
        capability: weapon_service::StoreCapability::AIR_TO_AIR_MISSILE,
        inhibited: false,
        rounds: Rounds::Finite(4),
        pointing_error_deg: 0.,
        employment_limit_deg: Some(30.),
        employment_fit: None,
        minimum_range_ft: 1_000.,
        maximum_range_ft: Some(maximum_range_ft),
        energy_target: None,
        infrared_envelope: None,
        requires_radar: false,
        requires_sensor: false,
        employment_zone: None,
        mount: [0.; 3],
        damage_vs_category: 100.,
        store_speed: ScalarSpeed(2_400.),
        tracking_delay: Delay::seconds(1),
        pacing: ProjectilePacing {
            burst_count: 1,
            burst_interval: Delay::seconds(0),
            reload: Delay::seconds(2),
            startup: Delay::seconds(0),
        },
    }
}

fn frame<'a>(
    tick: u64,
    targets: &'a [TargetView],
    stations: &'a [StationView],
    wing: WingView,
) -> DecisionFrame<'a> {
    DecisionFrame {
        tick,
        own: own_state(),
        targets,
        events: &[],
        stations,
        dispensers: &[],
        wing,
        route: RouteView {
            home_airport: Some(crate::ai::route::Position { x: 0., z: 0. }),
            leader_is_ai: true,
        },
        now: TimeOfDay(tick),
        flight_state: FlightState::Free,
    }
}

fn wing_view(leader: Option<LeaderView>) -> WingView {
    WingView {
        control: WingControl::Loose,
        formation: Formation::Echelon,
        horizontal_spacing_ft: 2_048,
        vertical_spacing_ft: 512,
        slot: 1,
        leader,
        wingmen_in_formation: 0,
        wing_combat: false,
        wing_approach: false,
        wing_approach_value_ft: None,
    }
}

#[test]
fn a_repeated_tick_keeps_the_record_and_an_advancing_tick_renews_it() {
    let targets = [hostile(2, [0., 20_000., 8_000.])];
    let stations = [missile_station(40_000.)];
    let mut c = controller(1, 0);
    assert_eq!(c.trace().path, StepPath::NotRun);
    c.step(&frame(0, &targets, &stations, wing_view(None)))
        .unwrap();
    let first = format!("{:?}", c.trace());
    let draws = c.draws().len();
    assert_eq!(c.trace().tick, Some(0));
    assert_eq!(c.trace().path, StepPath::Decided);
    assert_eq!(c.trace().target.chosen, Some(2));
    assert!(matches!(c.trace().target.path, TargetPath::Ranked { .. }));
    for _ in 0..5 {
        c.step(&frame(0, &targets, &stations, wing_view(None)))
            .unwrap();
    }
    assert_eq!(
        format!("{:?}", c.trace()),
        first,
        "a repeated tick changed it"
    );
    assert_eq!(c.draws().len(), draws, "a repeated tick cleared the draws");
    c.step(&frame(1, &targets, &stations, wing_view(None)))
        .unwrap();
    assert_eq!(c.trace().tick, Some(1));
    assert!(matches!(
        c.trace().target.path,
        TargetPath::Retained { distance_ft } if (distance_ft - 8_000.).abs() < 1.
    ));
}

#[test]
fn an_out_of_range_store_says_which_limit_it_breaks() {
    let targets = [hostile(2, [0., 20_000., 60_000.])];
    let stations = [missile_station(40_000.)];
    let mut c = controller(1, 0);
    let batch = c
        .step(&frame(0, &targets, &stations, wing_view(None)))
        .unwrap();
    assert!(batch.weapons.is_empty());
    let weapons = c.trace().weapons.as_ref().expect("weapon record");
    assert_eq!(weapons.chosen, None);
    assert!(!weapons.lock.locked && !weapons.lock.has_station);
    assert_eq!(weapons.stations.len(), 1);
    let store = weapons.stations[0];
    assert_eq!(store.verdict, Some(StationVerdict::OutsideEnvelope));
    let check = store.employment.expect("every envelope reason");
    assert!(check.beyond_maximum_range);
    assert!(!check.below_minimum_range && !check.beyond_angle_limit);
    assert_eq!(
        check.first_failure(),
        Some(EmploymentFailure::BeyondMaximumRange)
    );
    assert!((check.range_ft - 60_000.).abs() < 1.);

    // Inside the envelope the same store is usable and scored.
    let near = [hostile(2, [0., 20_000., 8_000.])];
    let mut c = controller(1, 0);
    c.step(&frame(0, &near, &stations, wing_view(None)))
        .unwrap();
    let store = c.trace().weapons.as_ref().unwrap().stations[0];
    assert!(matches!(store.verdict, Some(StationVerdict::Usable { .. })));
    assert!(store.score.is_some() && store.hit_chance.is_some());
    assert_eq!(
        c.trace().weapons.as_ref().unwrap().chosen,
        Some(StationId(0))
    );
}

#[test]
fn station_verdicts_follow_the_store_filter_order() {
    let own = own_state();
    let target = hostile(2, [0., 20_000., 8_000.]);
    let air = weapon_service::TargetClass::Air;
    let mut store = missile_station(40_000.);
    assert!(matches!(
        store.verdict(&own, &target, air),
        StationVerdict::Usable { .. }
    ));
    store.rounds = Rounds::Finite(0);
    assert_eq!(store.verdict(&own, &target, air), StationVerdict::Empty);
    store.capability = weapon_service::StoreCapability::SURFACE_STORE;
    assert_eq!(
        store.verdict(&own, &target, air),
        StationVerdict::WrongTargetClass
    );
    store.requires_sensor = true;
    let mut unsupported = target;
    unsupported.sensor_supported = false;
    assert_eq!(
        store.verdict(&own, &unsupported, air),
        StationVerdict::NoSensorTrack
    );
    store.requires_radar = true;
    let mut silent = own;
    silent.radar_emitting = false;
    assert_eq!(
        store.verdict(&silent, &unsupported, air),
        StationVerdict::RadarOff
    );
    store.inhibited = true;
    assert_eq!(
        store.verdict(&silent, &unsupported, air),
        StationVerdict::Inhibited
    );
}

#[test]
fn the_employment_check_agrees_with_the_employment_error_everywhere() {
    use tore_formats::weapons::Zone;
    let zone = Zone {
        heading: (30. * 182.) as i16,
        pitch: (20. * 182.) as i16,
        minimum_range: 1_000,
        maximum_range: 20_000,
        minimum_altitude: -5_000,
        maximum_altitude: 5_000,
    };
    let mut stores = vec![missile_station(40_000.)];
    let mut zoned = missile_station(20_000.);
    zoned.employment_limit_deg = None;
    zoned.employment_zone = Some(zone);
    zoned.mount = [3., -2., 12.];
    stores.push(zoned);
    let mut unlimited = missile_station(1.);
    unlimited.maximum_range_ft = None;
    unlimited.employment_limit_deg = None;
    unlimited.minimum_range_ft = 0.;
    stores.push(unlimited);
    let mut failures_seen = std::collections::BTreeSet::new();
    for store in &stores {
        for heading in [0., 95., 210.] {
            for (pitch, bank) in [(0., 0.), (-12., 70.), (30., -150.)] {
                let mut own = own_state();
                own.heading_deg = heading;
                own.flight_path_pitch_deg = pitch;
                own.bank_deg = bank;
                for azimuth in (-170..=180).step_by(35) {
                    for elevation in (-60..=60).step_by(30) {
                        for range in [400., 2_500., 9_000., 30_000., 70_000.] {
                            let direction = crate::attitude::Basis::new(
                                (heading + f64::from(azimuth)).to_radians(),
                                f64::from(elevation).to_radians(),
                                0.,
                            )
                            .forward;
                            let position =
                                std::array::from_fn(|i| own.position[i] + direction[i] * range);
                            let target = hostile(9, position);
                            let error = store.employment_error(&own, &target);
                            let check = store.employment_check(&own, &target);
                            assert_eq!(check.permitted(), error.is_some());
                            if let Some(error) = error {
                                assert_eq!(check.error_deg.to_bits(), error.to_bits());
                            }
                            failures_seen.extend(check.failures().map(|f| format!("{f:?}")));
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        failures_seen.len(),
        4,
        "every reason reached: {failures_seen:?}"
    );
}

#[test]
fn the_engagement_explanation_names_the_same_choice_as_the_selection() {
    let own_id = 1;
    let own = [0., 20_000., 0.];
    let targets = {
        let mut ineligible = hostile(15, [1_000., 20_000., 3_000.]);
        ineligible.seeker_eligible = false;
        let mut destroyed = hostile(17, [0., 20_000., 2_500.]);
        destroyed.valid = false;
        let mut crowded = hostile(13, [-6_000., 21_000., 12_000.]);
        crowded.wing_attackers = 2;
        let mut friend = hostile(5, [300., 20_000., 1_000.]);
        friend.side = Side(1);
        vec![
            hostile(11, [2_000., 20_500., 9_000.]),
            hostile(12, [-9_000., 19_000., 30_000.]),
            crowded,
            hostile(14, [45_000., 18_000., 60_000.]),
            friend,
            ineligible,
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
    let duty = |role, stance| Assignment {
        role,
        stance,
        ..Assignment::default()
    };
    let assignments = [
        duty(Role::FreeEngagement, Stance::EngageAssigned),
        Assignment {
            patrol: Some(PatrolRegion {
                clip: None,
                center_ft: [0., 20_000., 10_000.],
                radius_ft: 15_000.,
            }),
            ..duty(Role::CombatAirPatrol, Stance::EngageAssigned)
        },
        Assignment {
            protected_ids: vec![4],
            destroy_ids: vec![13],
            hostile_escorts: vec![HostileEscort {
                principal_id: 11,
                escort_id: 12,
            }],
            ..duty(Role::Escort, Stance::ProtectAssigned)
        },
        duty(Role::Disengage, Stance::SelfDefense),
        duty(Role::FreeEngagement, Stance::WeaponsHold),
    ];
    let mut kinds = std::collections::BTreeSet::new();
    for assignment in &assignments {
        let mut policy = Policy::default();
        for protected in &protected_sets {
            for reports in &report_sets {
                for current in [None, Some(11), Some(13)] {
                    let args = (own_id, Side(1), own, assignment, &targets[..]);
                    let selection = policy.select(
                        args.0, args.1, args.2, args.3, args.4, protected, reports, current, 2,
                    );
                    let explanation = policy.explain(
                        args.0, args.1, args.2, args.3, args.4, protected, reports, current, 2,
                    );
                    assert_eq!(explanation.chosen, selection);
                    assert_eq!(explanation.best_priority, selection.map(|s| s.priority));
                    assert_eq!(
                        explanation.candidates.iter().filter(|c| c.chosen).count(),
                        usize::from(selection.is_some())
                    );
                    for candidate in &explanation.candidates {
                        if candidate.ineligible.any() {
                            assert_eq!(candidate.priority, None);
                        }
                        if let Some(priority) = candidate.priority {
                            kinds.insert(format!("{priority:?}"));
                        }
                    }
                    // Best priority first, then the lowest score.
                    let ranked: Vec<_> = explanation
                        .candidates
                        .iter()
                        .take_while(|c| c.priority.is_some())
                        .map(|c| (c.priority, c.score_ft))
                        .collect();
                    assert!(ranked.windows(2).all(|pair| pair[0].0 < pair[1].0
                        || (pair[0].0 == pair[1].0 && pair[0].1 <= pair[1].1)));
                }
            }
        }
    }
    for kind in ["OwnDefense", "ProtectedThreat", "HostileEscort", "Free"] {
        assert!(kinds.contains(kind), "never explained {kind}: {kinds:?}");
    }
}

// ---------------------------------------------------------------------------
// Mission records and the journal

fn one_v_one(separation_ft: f64) -> AiMission {
    let mut mission = AiMission::new();
    mission.push(AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap());
    mission.push(
        AiActor::new(setup(
            2,
            2,
            0,
            [0., 20_000., separation_ft],
            std::f64::consts::PI,
        ))
        .unwrap(),
    );
    mission
}

#[test]
fn a_mission_step_records_the_engagement_and_the_controls() {
    let mut mission = one_v_one(60_000.);
    step(&mut mission, &[]);
    let actor = mission.actor(1).unwrap();
    let trace = actor.trace();
    assert_eq!(trace.tick, Some(0));
    assert_eq!(trace.path, ActorPath::Controller);
    assert_eq!(trace.targets.len(), 1);
    assert_eq!(trace.stations.len(), actor.stations().len());
    assert_eq!(trace.station_aim, Some(2));
    let engagement = trace.engagement.as_ref().expect("engagement record");
    assert_eq!(
        engagement.selection.map(|s| (s.id, s.priority)),
        Some((2, Priority::Free))
    );
    assert_eq!(engagement.explanation.chosen, engagement.selection);
    let fly = trace.fly.as_ref().expect("controls record");
    let adapter = fly.adapter.as_ref().expect("adapter output");
    assert_eq!(adapter.input, *actor.last_input());
    assert!(fly.intent.is_some());
    assert!((fly.achieved.speed_fps - actor.flight().speed).abs() < 1e-9);
    // Both stores are out of range at 60,000 ft.
    let weapons = actor.controller().trace().weapons.as_ref().unwrap();
    assert!(weapons.stations.iter().all(|store| {
        store.verdict == Some(StationVerdict::OutsideEnvelope)
            && store
                .employment
                .is_some_and(|check| check.beyond_maximum_range)
    }));
}

#[test]
fn a_missile_warning_is_queued_answered_and_journaled() {
    let mut mission = one_v_one(60_000.);
    let report = ThreatReport {
        missile_id: 500,
        seeker: SeekerClass::Radar,
        launcher_id: 99,
        launcher_same_side: false,
        distance_at_launch_ft: 10_000.,
        launch_tick: 0,
    };
    mission.actor_mut(1).unwrap().report_threat(report);
    step(&mut mission, &[]);
    let warnings = &mission
        .actor(1)
        .unwrap()
        .controller()
        .trace()
        .events
        .warnings;
    assert_eq!(warnings.len(), 1);
    // Ordinary flight 6 s, no whole two-mile step, Experienced 1 s: 7 s.
    let due = 7 * 120;
    assert_eq!(warnings[0].due_tick, Some(due));
    assert_eq!(
        warnings[0].step,
        WarningStep::Queued {
            already_queued: false
        }
    );
    let journal = entries(&mut mission);
    let queued = journal
        .iter()
        .find(|e| matches!(e.message, Message::MissileWarning(r) if r.missile_id == 500))
        .expect("queued warning journaled");
    assert_eq!(queued.sender, Some(99));
    assert_eq!(queued.receipts[0].actor, 1);
    assert!(matches!(
        queued.receipts[0].outcome,
        Outcome::WarningDue { due_tick, already_queued: false } if due_tick == due
    ));

    while mission.tick() < due {
        step(&mut mission, &[]);
        entries(&mut mission);
    }
    step(&mut mission, &[]);
    let decided = mission.actor(1).unwrap().controller().trace();
    let warning = decided.events.warnings[0];
    assert!(warning.from_queue);
    let WarningStep::Received { outcome, .. } = warning.step else {
        panic!("the warning did not arrive: {warning:?}");
    };
    assert_eq!(
        outcome.reaction,
        WarningReaction::Maneuver {
            reason: ScriptReason::RadarLaunch,
            wing_reaction: true,
        }
    );
    assert_eq!(decided.events.highest, Some(ScriptReason::RadarLaunch));
    assert_eq!(decided.events.start, Some(ScriptStart::Restart));
    let journal = entries(&mut mission);
    assert!(journal.iter().any(|e| {
        matches!(e.message, Message::MissileWarning(r) if r.missile_id == 500)
            && matches!(e.receipts[0].outcome, Outcome::WarningReceived { .. })
    }));
}

#[test]
fn formation_flight_names_its_branch() {
    let mut mission = AiMission::new();
    mission.push(AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap());
    mission.push(AiActor::new(setup(2, 1, 1, [-3_000., 20_000., -3_000.], 0.)).unwrap());
    mission.start_in_formation();
    for _ in 0..60 {
        step(&mut mission, &[]);
    }
    let wingman = mission.actor(2).unwrap();
    let MotionBranch::Formation(formation) = wingman.controller().trace().motion.branch else {
        panic!(
            "not flying formation: {:?}",
            wingman.controller().trace().motion
        );
    };
    assert!(formation.continuing || formation.slot_point != [0.; 3]);
    let phase = wingman.controller().formation_trace().map(|t| t.phase);
    assert!(phase.is_some(), "the formation trace names its phase");
    assert!(wingman.trace().engagement.as_ref().unwrap().neutral);
    assert_eq!(wingman.activity(), Activity::Formation);
}

fn runway() -> RunwayView {
    RunwayView {
        airport: 7,
        object: 70,
        center: [0., 0., 0.],
        heading: 0.,
        length_ft: 8_000.,
        elevation_ft: 0.,
        anchors: None,
    }
}

fn runway_surface(x: f64, z: f64) -> Surface {
    if x.abs() <= 100. && z.abs() <= 4_000. {
        Surface::runway(0.)
    } else {
        Surface::terrain(0.)
    }
}

#[test]
fn an_airfield_departure_records_its_phase_and_drops_warnings() {
    let position = [40., 0., -3_250.];
    let mut setup = setup(1, 1, 1, position, 0.);
    setup.flight = flight::State::new(&synthetic_profile(AircraftId::F18), position).unwrap();
    setup.home_airport = None;
    let mut actor = AiActor::new(setup).unwrap();
    actor.flight_mut().enable_research(1).unwrap();
    actor.flight_mut().start_on_runway(position, 0.).unwrap();
    actor.start_on_ground(GroundStart {
        runway: runway(),
        end: ApproachEnd::Near,
        order: 1,
    });
    let mut mission = AiMission::new();
    mission.push(actor);
    mission.set_external_leader(Side(1), 0, 10);
    let mut leader = object(mission.actor(1).unwrap(), 1);
    leader.id = 10;
    leader.human_controlled = true;
    leader.on_ground = true;
    leader.position = [0., 0., -3_700.];
    let advance = |mission: &mut AiMission| {
        let world = world_of(mission, [leader.clone()]);
        let tick = mission.tick();
        mission
            .step_with_surface(
                &world,
                &|x, z| runway_surface(x, z).height,
                &runway_surface,
                TimeOfDay(tick),
            )
            .unwrap();
    };
    advance(&mut mission);
    let trace = mission.actor(1).unwrap().trace();
    assert_eq!(trace.path, ActorPath::Airfield);
    assert!(
        !trace.clearance.turn,
        "a parked human leader holds its wing"
    );
    let airfield = trace.airfield.expect("airfield record");
    assert_eq!(
        airfield.step.map(|s| s.command.activity),
        Some(Activity::Waiting)
    );
    assert!(airfield.situation.is_some());
    entries(&mut mission);

    let report = ThreatReport {
        missile_id: 600,
        seeker: SeekerClass::Infrared,
        launcher_id: 30,
        launcher_same_side: false,
        distance_at_launch_ft: 5_000.,
        launch_tick: 1,
    };
    mission.actor_mut(1).unwrap().report_threat(report);
    advance(&mut mission);
    let trace = mission.actor(1).unwrap().trace();
    assert_eq!(
        trace.dropped_warnings,
        vec![(
            report,
            DropReason::Airfield {
                phase: Phase::Waiting
            }
        )]
    );
    let journal = entries(&mut mission);
    assert!(journal.iter().any(|e| {
        e.sender == Some(30)
            && e.receipts[0].outcome
                == Outcome::WarningDropped(DropReason::Airfield {
                    phase: Phase::Waiting,
                })
    }));
}

#[test]
fn a_rejected_order_is_journaled_with_its_reason() {
    let mut mission = AiMission::new();
    mission.push(AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap());
    let mut dummy = AiActor::new(setup(2, 1, 1, [500., 20_000., -500.], 0.)).unwrap();
    dummy.set_dummy();
    mission.push(dummy);
    let request = WingRequest::Break {
        heading_offset_deg: 90,
        pitch_deg: 0,
    };
    let outcomes = mission
        .order_wing_report(Side(1), 0, Some(1), None, request)
        .unwrap();
    assert_eq!(
        outcomes,
        vec![(2, ReceiverOutcome::Rejected(RejectReason::Dummy))]
    );
    let journal = entries(&mut mission);
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].sender, Some(1));
    assert_eq!(journal[0].message, Message::WingRequest(Box::new(request)));
    assert_eq!(
        journal[0].receipts[0].outcome,
        Outcome::Order(ReceiverOutcome::Rejected(RejectReason::Dummy))
    );
}

#[test]
fn attack_evidence_is_queued_delivered_ignored_and_expires() {
    let mut mission = AiMission::new();
    mission.push(AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap());
    let mut escort = AiActor::new(setup(2, 1, 1, [1_000., 20_000., 0.], 0.)).unwrap();
    escort.set_assignment(Assignment {
        role: Role::Escort,
        stance: Stance::ProtectAssigned,
        protected_ids: vec![1],
        ..Assignment::default()
    });
    mission.push(escort);
    let report = engagement::ThreatReport {
        attacker_id: None,
        defended_id: 1,
    };
    mission.report_attack_evidence(1, report, Some(45.), Some(700));
    let journal = entries(&mut mission);
    assert_eq!(journal.len(), 1);
    assert_eq!(journal[0].sender, Some(1));
    let mut queued: Vec<_> = journal[0].receipts.iter().map(|r| r.actor).collect();
    queued.sort_unstable();
    assert_eq!(queued, [1, 2]);
    assert!(
        journal[0]
            .receipts
            .iter()
            .all(|r| r.outcome == Outcome::Queued)
    );

    // A second report of the same evidence only refreshes it.
    mission.report_attack_evidence(1, report, Some(46.), Some(700));
    assert!(entries(&mut mission).is_empty());

    step(&mut mission, &[]);
    let delivered: Vec<_> = entries(&mut mission)
        .into_iter()
        .filter(|e| matches!(e.message, Message::AttackEvidence(_)))
        .collect();
    assert_eq!(delivered.len(), 2, "{delivered:?}");
    assert!(
        delivered
            .iter()
            .all(|e| e.receipts[0].outcome == Outcome::Delivered)
    );

    // Reports the mission cannot use are journaled with the reason.
    mission.report_attack_evidence(
        1,
        engagement::ThreatReport {
            attacker_id: None,
            defended_id: 2,
        },
        None,
        None,
    );
    mission.report_attack_evidence(
        77,
        engagement::ThreatReport {
            attacker_id: None,
            defended_id: 77,
        },
        None,
        None,
    );
    let ignored: Vec<_> = entries(&mut mission)
        .into_iter()
        .map(|e| e.receipts[0].outcome)
        .collect();
    assert_eq!(
        ignored,
        [
            Outcome::Ignored(IgnoreReason::NotTheDefendedAircraft),
            Outcome::Ignored(IgnoreReason::UnknownReporter),
        ]
    );

    // A recalled escort ignores evidence seen before the recall.
    let tick = mission.tick();
    mission.actor_mut(2).unwrap().return_to_formation(tick);
    mission.report_attack_evidence(1, report, Some(47.), Some(701));
    step(&mut mission, &[]);
    let journal = entries(&mut mission);
    assert!(journal.iter().any(|e| {
        e.receipts[0].actor == 2
            && e.receipts[0].outcome
                == Outcome::Ignored(IgnoreReason::SeenBeforeRecall { recalled_at: tick })
    }));
    let escort = mission.actor(2).unwrap();
    assert!(
        escort
            .trace()
            .engagement
            .as_ref()
            .unwrap()
            .reports
            .iter()
            .any(|r| r.ignored.is_some())
    );

    // Unrefreshed evidence is forgotten after 240 ticks.
    let mut expired = Vec::new();
    for _ in 0..240 {
        step(&mut mission, &[]);
        expired.extend(
            entries(&mut mission)
                .into_iter()
                .filter(|e| matches!(e.receipts[0].outcome, Outcome::Expired(_))),
        );
    }
    assert!(!expired.is_empty());
    assert!(
        expired
            .iter()
            .all(|e| e.receipts[0].outcome == Outcome::Expired(ExpiryReason::Age { ticks: 240 }))
    );
}

#[test]
fn an_ai_leader_release_is_journaled_with_its_trigger_and_wing_outcomes() {
    let mut mission = AiMission::new();
    for (id, side, member, z) in [
        (1, 1, 0, 0.),
        (2, 1, 1, -500.),
        (3, 2, 0, 2_000.),
        (4, 2, 1, 2_500.),
    ] {
        mission.push(AiActor::new(setup(id, side, member, [0., 20_000., z], 0.)).unwrap());
    }
    mission.start_in_formation();
    mission.report_attack_evidence(
        2,
        engagement::ThreatReport {
            attacker_id: Some(3),
            defended_id: 2,
        },
        None,
        Some(90),
    );
    entries(&mut mission);
    step(&mut mission, &[]);
    let journal = entries(&mut mission);
    let release = journal
        .iter()
        .find(|e| matches!(e.message, Message::FreeSelection { .. }))
        .expect("release journaled");
    assert_eq!(release.sender, Some(1));
    let Message::FreeSelection { trigger } = release.message else {
        unreachable!()
    };
    assert_eq!(trigger.report.defended_id, 2);
    assert_eq!(trigger.event_id, Some(90));
    assert!(matches!(
        release.receipts[0].outcome,
        Outcome::Order(ReceiverOutcome::Applied(_))
    ));
    let free_selection = WingRequest::TargetAssignment(TargetOrder::FreeSelection);
    let order = journal
        .iter()
        .find(|e| e.message == Message::WingRequest(Box::new(free_selection)))
        .expect("wing order journaled");
    assert_eq!(order.sender, Some(1));
    assert_eq!(order.receipts.len(), 1);
    assert_eq!(order.receipts[0].actor, 2);
}

#[test]
fn an_escort_priority_change_is_journaled_once() {
    let mut mission = AiMission::new();
    let mut escort = AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap();
    escort.set_assignment(Assignment {
        role: Role::Escort,
        stance: Stance::ProtectAssigned,
        protected_ids: vec![10],
        ..Assignment::default()
    });
    mission.push(escort);
    mission.set_external_leader(Side(1), 0, 10);
    mission.report_attack(
        10,
        engagement::ThreatReport {
            attacker_id: Some(3),
            defended_id: 10,
        },
    );
    let template = mission.actor(1).unwrap();
    let attacker = visible_object(template, 3, [5_000., 20_000., 0.]);
    let mut charge = object(template, 1);
    charge.id = 10;
    charge.human_controlled = true;
    charge.position = [0., 20_000., 1_000.];
    let extra = [charge, attacker];
    step(&mut mission, &extra);
    let selections: Vec<_> = entries(&mut mission)
        .into_iter()
        .filter_map(|e| match e.message {
            Message::EscortPriority { selection } => Some(selection),
            _ => None,
        })
        .collect();
    assert_eq!(
        selections,
        [Some(engagement::Selection {
            id: 3,
            priority: Priority::ProtectedThreat,
        })]
    );
    step(&mut mission, &extra);
    assert!(
        !entries(&mut mission)
            .iter()
            .any(|e| matches!(e.message, Message::EscortPriority { .. })),
        "an unchanged selection is not journaled again"
    );
}

#[test]
fn the_ignore_reason_agrees_with_the_response_permission() {
    let mut actor = AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap();
    let attack = |observed_tick, event_id| ObservedAttack {
        report: engagement::ThreatReport {
            attacker_id: Some(3),
            defended_id: 1,
        },
        bearing_world_deg: None,
        observed_tick,
        event_id,
    };
    let cases = [
        attack(5, None),
        attack(10, Some(1)),
        attack(11, Some(2)),
        attack(20, Some(1)),
    ];
    for (neutral, recall) in [(false, None), (true, None), (true, Some(10))] {
        actor.neutral = neutral;
        actor.formation_order_tick = recall;
        actor.ignored_attack_ids = vec![1];
        for case in &cases {
            assert_eq!(
                actor.attack_ignore_reason(case).is_none(),
                actor.permits_attack_response(case),
                "{neutral} {recall:?} {case:?}"
            );
        }
    }
}

#[test]
fn ai_ir_head_on_uses_shared_seeker_but_keeps_minimum_range_discipline() {
    let mut station = missile_station(50_000.);
    station.infrared_envelope = Some((true, 50_000.));
    let own = frame(0, &[], &[], wing_view(None)).own;
    for (distance, usable) in [
        (500., false),
        (1000., true),
        (18_228., true),
        (21_000., false),
    ] {
        let t = hostile(2, [0., 20_000., distance]);
        assert_eq!(
            station.employment_error(&own, &t).is_some(),
            usable,
            "{distance}"
        );
    }
    let mut t = hostile(2, [0., 20_000., 21_000.]);
    t.infrared_strength = 1.5;
    assert!(station.employment_error(&own, &t).is_some());
    station.infrared_envelope = Some((false, 50_000.));
    assert!(station.employment_error(&own, &t).is_none());
    t.heading_deg = 0.;
    assert!(station.employment_error(&own, &t).is_some());
}

#[test]
fn an_energy_point_gap_blocks_employment_despite_a_larger_range_limit() {
    let mut station = missile_station(200_000.);
    let own = frame(0, &[], &[], wing_view(None)).own;
    let target = hostile(2, [0., 20_000., 100_000.]);
    // The world sets Some(None) for a pending or failed exact-point answer.
    // Only a successful answer for this target may make the station eligible.
    for (permission, allowed) in [(Some(2), true), (None, false), (Some(3), false)] {
        station.energy_target = Some(permission);
        assert_eq!(station.employment_error(&own, &target).is_some(), allowed);
        assert_eq!(
            station.employment_check(&own, &target).beyond_maximum_range,
            !allowed
        );
    }
}
