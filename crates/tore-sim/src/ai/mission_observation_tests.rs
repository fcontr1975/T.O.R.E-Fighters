//! Independent pre-extraction observation reference and exact comparisons.
//! The reference body is retained from c1d1e736 for the threading boundary.
use super::tests::{enable_test_radar, flat, perception_actor, visible_object};
use super::*;
use crate::ai::Experience;

fn sensor_actor(level: Experience) -> AiActor {
    let mut actor = perception_actor(level);
    enable_test_radar(&mut actor);
    actor.flight.radar = true;
    let sensors = actor.sensors.as_mut().unwrap();
    let volume = sensors.profiles.radar.as_ref().unwrap().search;
    let passive = sensors::profile::PassiveProfile {
        record: "SYNTHETIC.SEE".into(),
        search: volume,
        track: volume,
    };
    sensors.profiles.infrared = Some(passive.clone());
    sensors.profiles.visual = Some(passive);
    sensors.controls.history = true;
    actor
}

fn assert_actor_state(left: &AiActor, right: &AiActor) {
    let flight_bytes = |actor: &AiActor| {
        let mut writer = tore_codec::BitWriter::new();
        actor.flight.write_exact(&mut writer, None).unwrap();
        writer.finish()
    };
    assert_eq!(flight_bytes(left), flight_bytes(right), "flight state bits");
    assert_eq!(left.sensors, right.sensors);
    assert_eq!(left.awareness, right.awareness);
    assert_eq!(left.trace, right.trace);
    assert_eq!(left.controller.trace(), right.controller.trace());
    // Several state types deliberately omit diagnostic records from PartialEq.
    // Debug covers every private field, including those records and RNG logs.
    // Its finite float representation also distinguishes signed zero.
    assert_eq!(format!("{left:?}"), format!("{right:?}"));
}

fn assert_target_bits(left: &[TargetView], right: &[TargetView]) {
    assert_eq!(left, right);
    for (left, right) in left.iter().zip(right) {
        let bits = |target: &TargetView| {
            [
                target.position[0],
                target.position[1],
                target.position[2],
                target.heading_deg,
                target.pitch_deg,
                target.speed.0,
                target.maximum_speed.0,
            ]
            .map(f64::to_bits)
        };
        assert_eq!(bits(left), bits(right));
    }
}

#[test]
fn prepared_target_visibility_matches_original_queries_and_complete_state() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let actor = sensor_actor(Experience::Experienced);
    let world: Vec<_> = (2..=17)
        .map(|id| {
            let mut object = visible_object(
                &actor,
                id,
                [f64::from(id) * 100., 20000., 3000. + f64::from(id) * 250.],
            );
            object.observable.as_mut().unwrap().radar_emitting = true;
            object
        })
        .collect();
    let reference_reads = AtomicUsize::new(0);
    let reference_ground = |_, _| {
        reference_reads.fetch_add(1, Ordering::Relaxed);
        0.
    };
    let prepared_reads = AtomicUsize::new(0);
    let prepared_ground = |_, _| {
        prepared_reads.fetch_add(1, Ordering::Relaxed);
        0.
    };
    let mut reference = actor.clone();
    let targets = reference.observe_unsplit(0, &world, &reference_ground);
    assert_eq!(targets.len(), 16, "the fixture must observe every emitter");
    let blocked: Vec<_> = targets
        .iter()
        .map(|target| {
            crate::combat::live::terrain_hit(
                actor.flight.position,
                target.position,
                &reference_ground,
            )
            .is_some()
        })
        .collect();
    let prepared =
        actor
            .observation_input(0)
            .unwrap()
            .prepare(0, &world, &prepared_ground, &|x, z| surface(x, z).height);
    assert_eq!(prepared.terrain_blocked, blocked);
    assert_target_bits(&prepared.observed.targets, &targets);
    assert!(
        prepared
            .memory
            .current_observations()
            .all(|s| !s.target.terrain_blocked)
    );
    let mut actual = actor.clone();
    actual.sensors = prepared.sensors;
    actual.awareness = prepared.memory;
    actual.trace.visual = prepared.visual;
    actual.apply_observation(prepared.observed);
    assert_actor_state(&reference, &actual);
    // Shared sight lines sample each line once: the same answers from fewer
    // ground reads than the original's repeated queries.
    let (reference, prepared) = (
        reference_reads.load(Ordering::Relaxed),
        prepared_reads.load(Ordering::Relaxed),
    );
    assert!(
        prepared > 0 && prepared < reference,
        "shared sight lines read the ground less: {prepared} of {reference}"
    );
}

#[test]
fn prepared_target_visibility_never_changes_observation_memory() {
    let mut actor = sensor_actor(Experience::Experienced);
    actor.sensors = None;
    let world = [visible_object(&actor, 2, [0., 20000., 6000.])];
    let ridge = |_: f64, z: f64| if z >= 2000. { 30000. } else { 0. };
    let mut reference = actor.clone();
    let targets = reference.observe_unsplit(0, &world, &ridge);
    // Production keeps sensorless fixtures inline. Exercise the prepared
    // result's boundary directly so its decision visibility can be true
    // while the unchanged observation memory still records false.
    let prepared = observation::Input {
        index: 0,
        context: actor.observation_context(),
        sensors: None,
        memory: &actor.awareness,
        use_surface: false,
        ceiling: None,
    }
    .prepare(0, &world, &ridge, &|x, z| surface(x, z).height);
    assert_eq!(prepared.terrain_blocked, [true]);
    assert_target_bits(&prepared.observed.targets, &targets);
    assert_eq!(prepared.memory, reference.awareness);
    assert!(
        prepared
            .memory
            .current_observations()
            .all(|s| !s.target.terrain_blocked)
    );
}

pub(super) fn executors() -> Vec<tore_workers::Executor> {
    [0, 1, 2, 4, 8]
        .into_iter()
        .map(|workers| tore_workers::Executor::parallel(workers).unwrap())
        .chain(
            [7, 31, 120]
                .into_iter()
                .map(tore_workers::Executor::shuffled),
        )
        .collect()
}

fn runway() -> super::super::airfield::RunwayView {
    super::super::airfield::RunwayView {
        airport: 7,
        object: 70,
        center: [0., 0., 0.],
        heading: 0.,
        length_ft: 8000.,
        elevation_ft: 0.,
        anchors: None,
    }
}

pub(super) fn surface(x: f64, z: f64) -> Surface {
    if x.abs() <= 100. && z.abs() <= 4000. {
        Surface::runway(0.)
    } else {
        Surface::terrain(0.)
    }
}

pub(super) fn fixture(actors: u32, seed: u64) -> AiMission {
    let mut mission = AiMission::new();
    for id in 1..=actors {
        let level = Experience::ALL[(id as usize - 1) % 4];
        let donor = sensor_actor(level);
        let side = 1 + u32::from(id > actors.div_ceil(2));
        let slot = ((id - 1) % 5) as u8;
        // The first wing's leader is later in actor order, then its second
        // member. This exercises reads on both sides of the current actor.
        let member = if id <= 3 {
            [2, 0, 1][slot as usize]
        } else {
            slot
        };
        let yaw = if side == 1 { 0. } else { std::f64::consts::PI };
        let mut setup = super::tests::setup(
            id,
            side,
            member,
            [
                f64::from(slot) * 400.,
                12000.,
                if side == 1 { -6000. } else { 6000. },
            ],
            yaw,
        );
        setup.identity.wing = ((id - 1) / 5) as u8;
        setup.seed = seed + u64::from(id) * 31;
        setup.experience = donor.controller.experience();
        setup.sensors = donor.sensors;
        setup.flight.radar = true;
        if id % 2 == 0 {
            setup.flight.enable_research(id as i32).unwrap();
        }
        let mut actor = AiActor::new(setup).unwrap();
        actor.set_home_runway(Some(runway()));
        mission.push(actor);
    }
    mission
}

pub(super) fn snapshot(mission: &AiMission, human: Option<&AiActor>) -> Vec<WorldObject> {
    mission
        .actors
        .iter()
        .chain(human)
        .map(|actor| {
            let mut object = visible_object(actor, actor.id(), actor.flight.position);
            object.side = actor.identity.side;
            object.human_controlled = human.is_some_and(|human| human.id() == actor.id());
            object.on_ground = actor.flight.research.as_ref().is_some_and(|r| r.on_ground);
            let observable = object.observable.as_mut().unwrap();
            observable.basis = crate::attitude::Basis::new(
                actor.flight.yaw,
                actor.flight.pitch,
                actor.flight.bank,
            );
            observable.radar_emitting = actor.flight.radar;
            observable.airborne = !object.on_ground;
            object
        })
        .collect()
}

fn assert_mission_state(left: &AiMission, right: &AiMission) {
    assert_eq!(left.actors.len(), right.actors.len());
    for (left, right) in left.actors.iter().zip(&right.actors) {
        assert_actor_state(left, right);
    }
    assert_eq!(
        left.journal, right.journal,
        "actual entries, order and dropped count"
    );
    assert_eq!(
        format!("{left:?}"),
        format!("{right:?}"),
        "all mission state"
    );
}

pub(super) fn compare_step(
    reference: &mut AiMission,
    actual: &mut AiMission,
    workers: &tore_workers::Executor,
    human: Option<&AiActor>,
) -> Result<MissionOutput> {
    let world = snapshot(reference, human);
    let expected = reference.step_using(
        &world,
        &flat,
        &surface,
        TimeOfDay(reference.tick),
        &tore_workers::Executor::serial(),
    );
    let result = actual.step_using(&world, &flat, &surface, TimeOfDay(actual.tick), workers);
    assert_eq!(expected, result, "complete ordered mission output or error");
    assert_mission_state(reference, actual);
    result
}

pub(super) fn scripted_changes(mission: &mut AiMission, stage: u64, human: &mut Option<AiActor>) {
    use super::super::airfield::{LandingOrder, LandingReason};
    use super::super::wing::WingRequest;
    match stage {
        1 => {
            mission.set_missiles(vec![MissileSnapshot {
                id: 901,
                owner: 22,
                position: [0., 12000., -7000.],
                velocity: [0., 0., 2000.],
                guidance: crate::combat::missiles::Guidance::Supported,
                target: Some(1),
                radar_active: false,
                radar_acquired: false,
                supported: true,
                supporting_radar_position: Some([0., 12000., 30000.]),
                alive: true,
            }]);
        }
        2 => {
            // This tick must bypass prepared observations because damage
            // clears a previously selected target and sensor acquisition.
            let actor = mission.actor_mut(4).unwrap();
            assert!(actor.controller.target().is_some());
            actor.flight.systems.hit(34, 0.7);
            assert!(actor.observation_input(3).is_none());
        }
        3 => mission.actor_mut(2).unwrap().flight.crashed = true,
        4 => {
            mission
                .order(
                    5,
                    WingRequest::Land(LandingOrder {
                        runway: runway(),
                        reason: LandingReason::Ordered,
                    }),
                )
                .unwrap()
                .unwrap();
        }
        5 => {
            mission
                .order(5, WingRequest::FormationSelection(Formation::Echelon))
                .unwrap()
                .unwrap();
            assert!(mission.actor(5).unwrap().landing_order.is_none());
        }
        6 => {
            *human = mission.remove_actor(8);
            let identity = human.as_ref().unwrap().identity;
            mission.set_humans(vec![HumanMember {
                id: identity.actor.0,
                side: identity.side,
                wing: identity.wing,
                member: identity.member,
                pilot_alive: true,
            }]);
        }
        7 => mission.actor_mut(3).unwrap().flight.crashed = true,
        8 => {
            mission.set_humans(Vec::new());
            let flown = human.take().unwrap();
            let experience = flown.controller.experience();
            let home = flown.home_runway;
            let parts = flown.into_parts();
            let mut setup = super::tests::setup(
                parts.identity.actor.0,
                parts.identity.side.0,
                parts.identity.member,
                parts.flight.position,
                parts.flight.yaw,
            );
            setup.identity = parts.identity;
            setup.experience = experience;
            setup.flight = parts.flight;
            setup.sensors = parts.sensors;
            setup.stations = parts.stations;
            setup.dispensers = parts.dispensers;
            let mut returned = AiActor::new(setup).unwrap();
            returned.equipment = parts.equipment;
            returned.set_home_runway(home);
            assert!(returned.awareness.current_observations().next().is_none());
            mission.insert_actor(returned).unwrap();
            mission.set_missiles(Vec::new());
        }
        9 => {
            mission
                .actor_mut(9)
                .unwrap()
                .stations
                .iter_mut()
                .for_each(|station| station.store.inhibited = true);
        }
        _ => {}
    }
}

#[test]
fn prepared_observations_keep_complete_seeded_mission_state_and_output() {
    for workers in executors() {
        for seed in [7, 12345] {
            let mut reference = fixture(30, seed);
            // This sixteen-tick fixture needs an immediate defensive release;
            // novice reaction delay is tested in the mission defense tests.
            let controller = &mut reference.actor_mut(1).unwrap().controller;
            controller.set_experience(super::super::experience::ResolvedExperience {
                level: Experience::Ace,
                ..controller.experience()
            });
            let mut actual = reference.clone();
            let (mut reference_human, mut actual_human) = (None, None);
            let mut saw_devices = false;
            let mut saw_leadership = false;
            for tick in 0..16 {
                scripted_changes(&mut reference, tick, &mut reference_human);
                scripted_changes(&mut actual, tick, &mut actual_human);
                let output = compare_step(
                    &mut reference,
                    &mut actual,
                    &workers,
                    reference_human.as_ref(),
                )
                .unwrap();
                saw_devices |= !output.devices.is_empty();
                saw_leadership |= !output.leadership.is_empty();
                assert_eq!(reference.take_journal(), actual.take_journal());
            }
            assert!(saw_devices, "fixture must actually release countermeasures");
            assert!(saw_leadership, "fixture must actually pass leadership");
            assert!(reference.actor(4).unwrap().damage_return.is_some());
            assert!(
                reference.actor(8).is_some(),
                "human handback restored the actor"
            );
        }
    }
}

#[test]
fn prepared_observations_preserve_shared_airport_holds_and_cancelled_joins() {
    use super::super::airfield::{GroundStart, LandingOrder, LandingReason};
    use crate::airport::ApproachEnd;
    for workers in executors() {
        let mut reference = fixture(20, 11);
        reference.actors.truncate(10);
        for actor in &mut reference.actors {
            actor.flight.enable_research(actor.id() as i32).unwrap();
            let position = [0., 0., -3000. - f64::from(actor.id()) * 100.];
            actor.flight.start_on_runway(position, 0.).unwrap();
            actor.start_on_ground(GroundStart {
                runway: runway(),
                end: ApproachEnd::Near,
                order: actor.identity.member,
            });
        }
        let damaged = reference.actor_mut(1).unwrap();
        damaged.flight.systems.hit(7, 0.7);
        // Already recovering: eligible sensing is still discarded by the
        // subsequent ground-hold return, with no sensor/memory advancement.
        damaged.damage_return = Some(super::super::damage::Reason::Engine);
        assert!(damaged.observation_input(0).is_some());
        let sensors_before = damaged.sensors.clone();
        let mut actual = reference.clone();
        for _ in 0..4 {
            compare_step(&mut reference, &mut actual, &workers, None).unwrap();
            assert!(actual.actor(1).unwrap().trace.damage.ground_hold);
            assert_eq!(actual.actor(1).unwrap().sensors, sensors_before);
        }
        // Exercise the early join-cancellation branch with an airborne actor
        // and a later-index recovering leader.
        let mut reference = fixture(10, 13);
        for id in [1, 2] {
            let actor = reference.actor_mut(id).unwrap();
            actor.flight.position = [0., 6000., -30000.];
            actor.landing_order = Some(LandingOrder {
                runway: runway(),
                reason: if id == 1 {
                    LandingReason::JoinLeader
                } else {
                    LandingReason::Ordered
                },
            });
            actor.activity = Activity::ReturningToBase;
        }
        reference
            .order(
                1,
                super::super::wing::WingRequest::FormationSelection(Formation::Echelon),
            )
            .unwrap()
            .unwrap();
        assert!(reference.actor(1).unwrap().join_cancelled);
        let mut actual = reference.clone();
        for _ in 0..4 {
            compare_step(&mut reference, &mut actual, &workers, None).unwrap();
            assert!(actual.actor(1).unwrap().landing_order.is_none());
        }
    }
}

#[test]
fn prepared_observations_keep_each_adapters_terrain_height_query() {
    let raised = |_: f64, z: f64| {
        if z.abs() < 3000. {
            Surface::runway(20000.)
        } else {
            Surface::terrain(0.)
        }
    };
    for workers in executors() {
        let mut reference = fixture(8, 29);
        let mut actual = reference.clone();
        let world = snapshot(&reference, None);
        let expected = reference.step_using(
            &world,
            &flat,
            &raised,
            TimeOfDay(0),
            &tore_workers::Executor::serial(),
        );
        let result = actual.step_using(&world, &flat, &raised, TimeOfDay(0), &workers);
        assert_eq!(expected, result);
        assert_mission_state(&reference, &actual);
        let sees_enemy = |id| {
            reference
                .actor(id)
                .unwrap()
                .awareness
                .current_observations()
                .any(|seen| seen.target.side == super::super::targeting::Side(2))
        };
        assert!(sees_enemy(1), "legacy observation uses bare terrain");
        assert!(!sees_enemy(2), "hybrid observation uses the raised surface");
    }
}

#[test]
fn discarded_observations_leave_ejection_dummy_and_error_state_unchanged() {
    for workers in executors() {
        let mut reference = fixture(10, 17);
        let actor = reference.actor_mut(1).unwrap();
        let mut profile = super::tests::synthetic_profile(AircraftId::F18);
        profile.fields.get_mut("flags").unwrap().value = "16".into();
        actor.flight = flight::State::new(&profile, [0., 100., 0.]).unwrap();
        actor.flight.velocity = [0., -500., 500.];
        actor.flight.speed = 500_f64.hypot(500.);
        actor.flight.pitch = -45_f64.to_radians();
        actor.escape_monitor = crate::ejection::Monitor::seeded(0);
        let assessment = crate::ejection::assess(&actor.flight, flat).unwrap();
        for _ in 0..119 {
            assert!(actor.escape_monitor.step(Some(assessment)).is_none());
        }
        assert!(actor.observation_input(0).is_some());
        let sensors_before = actor.sensors.clone();
        reference.actor_mut(3).unwrap().set_dummy();
        let mut actual = reference.clone();
        compare_step(&mut reference, &mut actual, &workers, None).unwrap();
        assert!(actual.actor(1).unwrap().trace.ejection.unwrap().ejected);
        assert_eq!(actual.actor(1).unwrap().sensors, sensors_before);
        assert_eq!(actual.actor(3).unwrap().trace.path, ActorPath::Dummy);
        compare_step(&mut reference, &mut actual, &workers, None).unwrap();
        assert!(
            actual
                .actor(1)
                .unwrap()
                .trace
                .ejection
                .unwrap()
                .escape_running
        );

        let mut reference = fixture(10, 19);
        let mut actual = reference.clone();
        for _ in 0..2 {
            compare_step(&mut reference, &mut actual, &workers, None).unwrap();
        }
        for mission in [&mut reference, &mut actual] {
            mission.tick = 0;
            mission.actor_mut(1).unwrap().set_dummy();
        }
        let untouched = format!("{:?}", &actual.actors[2..]);
        assert!(matches!(
            compare_step(&mut reference, &mut actual, &workers, None),
            Err(super::super::AiError::InvalidInput(
                "controller tick went backwards"
            ))
        ));
        assert_eq!(
            format!("{:?}", &actual.actors[2..]),
            untouched,
            "an earlier actor error must not publish later observations"
        );
    }
}

#[test]
fn prepared_observations_preserve_bounded_journal_contents() {
    for workers in executors() {
        let mut reference = fixture(10, 23);
        for tick in 0..thought::JOURNAL_LIMIT + 3 {
            reference.journal.push(JournalEntry {
                tick: tick as u64,
                sender: Some(1),
                message: Message::ContactSelection { target: 8 },
                receipts: vec![Receipt {
                    actor: 1,
                    outcome: Outcome::Delivered,
                }],
            });
        }
        reference
            .actor_mut(1)
            .unwrap()
            .pending_threats
            .push(ThreatReport {
                missile_id: 501,
                seeker: SeekerClass::Infrared,
                launcher_id: 99,
                launcher_same_side: false,
                distance_at_launch_ft: 3000.,
                launch_tick: 0,
            });
        let mut actual = reference.clone();
        compare_step(&mut reference, &mut actual, &workers, None).unwrap();
        let expected = reference.take_journal();
        assert!(expected.dropped >= 3);
        assert_eq!(expected.entries.len(), thought::JOURNAL_LIMIT);
        assert_eq!(expected, actual.take_journal());
    }
}

/// A portable synthetic diagnostic. Run in release mode on a quiet machine;
/// wall time includes cloning, scoped dispatch, ordered decisions and joins.
#[test]
#[ignore = "release observation dispatch timing, not a correctness gate"]
fn observation_worker_timing() {
    for count in [2, 4, 8, 30] {
        for worker_count in [0, 1, 2, 4, 8] {
            let workers = tore_workers::Executor::parallel(worker_count).unwrap();
            for run in 0..3 {
                let mut mission = fixture(count, 31);
                // Airport and other non-aircraft targets still enter installed
                // sensors. Keep the 286-target profile's scale synthetic.
                let mut world = snapshot(&mission, None);
                let template = world[0].clone();
                for id in count + 1..=286 {
                    let mut object = template.clone();
                    object.id = id;
                    object.is_aircraft = false;
                    object.position = [f64::from(id % 20) * 1200., 0., f64::from(id / 20) * 1500.];
                    let observable = object.observable.as_mut().unwrap();
                    observable.id = id;
                    observable.position = object.position;
                    observable.airborne = false;
                    observable.radar_emitting = false;
                    world.push(object);
                }
                for tick in 0..20 {
                    mission
                        .step_using(&world, &flat, &surface, TimeOfDay(tick), &workers)
                        .unwrap();
                }
                let start = std::time::Instant::now();
                for tick in 20..260 {
                    mission
                        .step_using(&world, &flat, &surface, TimeOfDay(tick), &workers)
                        .unwrap();
                }
                eprintln!(
                    "observation timing actors={count} workers={worker_count} run={run} mean_us={:.3}",
                    start.elapsed().as_secs_f64() * 1e6 / 240.
                );
            }
        }
    }
}

#[test]
fn serial_observation_extraction_matches_unsplit_reference() {
    for level in Experience::ALL {
        for sensor_mode in 0..4 {
            let mut reference = sensor_actor(level);
            if sensor_mode == 3 {
                reference.sensors = None;
            } else {
                reference.sensors.as_mut().unwrap().controls.channel = match sensor_mode {
                    0 => sensors::Channel::Radar,
                    1 => sensors::Channel::Infrared,
                    _ => sensors::Channel::Visual,
                };
            }
            let mut extracted = reference.clone();
            let mut world: Vec<_> = (1..=48)
                .map(|id| {
                    let angle = f64::from(id) * 0.17;
                    let mut object = visible_object(
                        &reference,
                        id,
                        [angle.sin() * 8000., 20000., angle.cos() * 8000.],
                    );
                    object.side = super::super::targeting::Side(1 + id % 2);
                    object.on_ground = id % 13 == 0;
                    object.is_aircraft = id % 17 != 0;
                    object.human_controlled = id % 7 == 0;
                    object.observable.as_mut().unwrap().radar_emitting = id % 3 == 0;
                    object
                })
                .collect();
            let ticks = [
                0, 1, 59, 60, 119, 120, 1799, 1800, 10799, 10800, 14399, 14400,
            ];
            for (stage, tick) in ticks.into_iter().enumerate() {
                for actor in [&mut reference, &mut extracted] {
                    if stage % 2 == 0 {
                        actor.trace = ActorTrace::begin(tick);
                    }
                    actor.flight.systems.counts[32] = u8::from(stage == 3) * 2;
                    actor.equipment.visual = stage == 4;
                    actor.equipment.radar = stage == 5;
                    actor.equipment.infrared = stage == 6;
                    if stage == 2 {
                        actor
                            .order(
                                super::super::wing::WingRequest::TargetAssignment(
                                    super::super::wing::TargetOrder::ConcreteTarget(
                                        super::super::wing::TargetId(3),
                                    ),
                                ),
                                tick,
                            )
                            .unwrap();
                        assert_eq!(actor.controller.target(), Some(3));
                        if let Some(sensors) = &mut actor.sensors {
                            sensors.designate(3);
                        }
                    }
                    if stage == 7 {
                        // The first damage-return response clears these before
                        // observation. Its copied context must use that state.
                        actor.controller.return_to_formation();
                        if let Some(sensors) = &mut actor.sensors {
                            sensors.clear_selection();
                        }
                    }
                    if stage == 8 {
                        actor.stations.iter_mut().for_each(|station| {
                            station.store.inhibited = true;
                        });
                    }
                }
                if stage == 1 {
                    world[3].alive = false;
                    world[5].destroyed = true;
                }
                if stage == 9 {
                    world.iter_mut().for_each(|object| object.observable = None);
                }
                let ridge = |_: f64, z: f64| {
                    if stage == 6 && z.abs() > 2000. {
                        30000.
                    } else {
                        0.
                    }
                };
                let expected = reference.observe_unsplit(tick, &world, &ridge);
                let actual = extracted.observe(tick, &world, &ridge);
                assert_target_bits(&expected, &actual);
                assert_actor_state(&reference, &extracted);
            }
            let expected = reference.observe_unsplit(14401, &[], &flat);
            let actual = extracted.observe(14401, &[], &flat);
            assert_target_bits(&expected, &actual);
            assert_actor_state(&reference, &extracted);
        }
    }
}

impl AiActor {
    /// Collect fresh measurements, then update frozen aircraft memory. Only
    /// the current observation set is allowed into combat target selection.
    fn observe_unsplit(
        &mut self,
        tick: u64,
        world: &[WorldObject],
        ground: &dyn Fn(f64, f64) -> f64,
    ) -> Vec<TargetView> {
        let live: Vec<u32> = world
            .iter()
            .filter(|o| o.alive && !o.destroyed)
            .map(|o| o.id)
            .collect();
        self.awareness.prune_lifecycle(&live);
        let observer = Observer {
            position: self.flight.position,
            basis: crate::attitude::Basis::new(
                self.flight.yaw,
                self.flight.pitch,
                self.flight.bank,
            ),
            radar_powered: self.flight.radar,
            radar_failed: self.equipment.radar,
            infrared_failed: self.equipment.infrared,
            visual_failed: self.equipment.visual,
        };
        let attention = self
            .controller
            .target()
            .and_then(|id| {
                self.awareness
                    .current_observations()
                    .find(|s| s.target.id == id)
                    .or_else(|| self.awareness.snapshot(id))
            })
            .or_else(|| {
                self.awareness
                    .current_observations()
                    .find(|s| s.target.side != self.identity.side)
            })
            .map(|s| s.target.position);
        let lookout = awareness::Lookout::new(tick, observer.position, observer.basis, attention);
        self.lookout = Some(lookout);
        self.trace.lookout = Some(lookout);
        let contacts = if let Some(sensors) = self.sensors.as_mut() {
            let observables: Vec<Observable> = world
                .iter()
                .filter(|o| o.id != self.identity.actor.0)
                .filter_map(|o| o.observable.clone())
                .collect();
            let obscured = |from, to| crate::combat::live::terrain_hit(from, to, &ground).is_some();
            let environment = sensors::Environment {
                ground,
                obscured: &obscured,
            };
            sensors.step(&observer, &observables, &environment);
            self.received_emitters = if self.flight.systems.counts[32] <= 1 {
                sensors::passive::emitters(
                    &observer,
                    &observables,
                    sensors.contacts(),
                    &environment,
                )
            } else {
                Vec::new()
            };
            Some(sensors.contacts().to_vec())
        } else {
            self.received_emitters.clear();
            None
        };
        let mut observations = Vec::new();
        // Aircraft on the ground are not air targets.
        for object in world.iter().filter(|o| {
            o.id != self.identity.actor.0
                && o.alive
                && !o.destroyed
                && o.is_aircraft
                && !o.on_ground
        }) {
            if let Some(contacts) = &contacts {
                for contact in contacts
                    .iter()
                    .filter(|c| c.id == object.id && !c.destroyed)
                {
                    if contact.channel == sensors::Channel::Visual
                        && lookout.check(
                            self.controller.experience().level,
                            contact.position,
                            None,
                            crate::combat::live::terrain_hit(
                                observer.position,
                                contact.position,
                                &ground,
                            )
                            .is_none(),
                        ) != awareness::VisualResult::Visible
                    {
                        continue;
                    }
                    observations.push(Observation {
                        target: self.observed_target(object, contact.position),
                        velocity: contact.velocity,
                        source: match contact.channel {
                            // AI sensors never select the HARM page; its emitters would be
                            // radar returns.
                            sensors::Channel::Radar | sensors::Channel::Harm => {
                                ObservationSource::Radar
                            }
                            sensors::Channel::Infrared => ObservationSource::Infrared,
                            sensors::Channel::Visual => ObservationSource::Visual,
                        },
                    });
                }
                // Pilot attention has its own circular, skill-scaled cone.
                // Imported visual equipment remains unchanged for player use.
                // Cloud/night visibility is not supplied by this host yet;
                // the explicit None limit is the fitted clear-air assumption.
                if let Some(observable) = object.observable.as_ref()
                    && !observable.destroyed
                    && observable.airborne
                {
                    let result = lookout.check(
                        self.controller.experience().level,
                        observable.position,
                        None,
                        crate::combat::live::terrain_hit(
                            observer.position,
                            observable.position,
                            &ground,
                        )
                        .is_none(),
                    );
                    if self.trace.visual.len() < 32 {
                        self.trace.visual.push(awareness::VisualTrace {
                            id: object.id,
                            distance_ft: distance(observer.position, observable.position),
                            result,
                        });
                    }
                    if result == awareness::VisualResult::Visible {
                        observations.push(Observation {
                            target: self.observed_target(object, observable.position),
                            velocity: observable.velocity,
                            source: ObservationSource::Visual,
                        });
                    }
                }
            } else {
                // Explicit sensorless synthetic/replay fixtures supply the
                // whole permitted list. Production actor loading must never
                // select this path as a fallback after a sensor import error.
                observations.push(Observation {
                    target: self.observed_target(object, object.position),
                    velocity: object.velocity,
                    source: ObservationSource::Fixture,
                });
            }
        }
        self.awareness.observe(tick, &observations);
        self.trace.observation_sources = self
            .awareness
            .current_observations()
            .take(32)
            .map(|s| (s.target.id, s.source_ticks))
            .collect();
        self.awareness
            .current_observations()
            .map(|snapshot| snapshot.target)
            .collect()
    }
}

/// A hilly test ground on a grid of points `HILL_CELL_FT` apart from the
/// origin, blended over each square's triangles and clamped at the edges as
/// the host terrain is, with the ceiling built from the same points.
struct Hills {
    cols: usize,
    rows: usize,
    heights: Vec<f64>,
}

const HILL_CELL_FT: f64 = 2000.;

impl Hills {
    fn random(cols: usize, rows: usize, seed: u64, highest: u64) -> Self {
        let mut state = seed;
        let heights = (0..cols * rows)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                // Mostly low ground with scattered tall hills.
                if state.is_multiple_of(5) {
                    (state >> 8) as f64 % highest as f64
                } else {
                    (state >> 8) as f64 % 800.
                }
            })
            .collect();
        Self {
            cols,
            rows,
            heights,
        }
    }
    fn point(&self, col: usize, row: usize) -> f64 {
        self.heights[row * self.cols + col]
    }
    fn height(&self, x: f64, z: f64) -> f64 {
        let fx = (x / HILL_CELL_FT).clamp(0., (self.cols - 1) as f64 - 0.001);
        let fz = (z / HILL_CELL_FT).clamp(0., (self.rows - 1) as f64 - 0.001);
        let (col, row) = (fx as usize, fz as usize);
        let (u, v) = (fx - col as f64, fz - row as f64);
        let h = |dc, dr| self.point(col + dc, row + dr);
        if u + v <= 1. {
            h(0, 0) + (h(1, 0) - h(0, 0)) * u + (h(0, 1) - h(0, 0)) * v
        } else {
            h(1, 1) + (h(0, 1) - h(1, 1)) * (1. - u) + (h(1, 0) - h(1, 1)) * (1. - v)
        }
    }
    fn ceiling(&self) -> crate::ground_ceiling::GroundCeiling {
        crate::ground_ceiling::GroundCeiling::from_grid(
            self.cols,
            self.rows,
            HILL_CELL_FT,
            |col, row| self.point(col, row),
        )
    }
}

/// A hill between two aircraft hides each from the other, the shared sight
/// lines say so once for every reader, and a line over the hill's top is
/// answered from the ceiling without reading the ground.
#[test]
fn a_hill_between_two_actors_blocks_their_sight_line() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // Flat ground at 100 ft with one 5,000 ft hill point between them.
    let mut hills = Hills {
        cols: 12,
        rows: 12,
        heights: vec![100.; 144],
    };
    hills.heights[5 * 12 + 5] = 5000.;
    let ceiling = hills.ceiling();
    let reads = AtomicUsize::new(0);
    let ground = |x: f64, z: f64| {
        reads.fetch_add(1, Ordering::Relaxed);
        hills.height(x, z)
    };
    let mut actor = sensor_actor(Experience::Ace);
    actor.flight.position = [10_000., 3000., 2000.];
    actor.flight.yaw = 0.;
    let behind = [10_000., 3000., 18_000.];
    let over = [10_000., 8_000., 18_000.];
    let sight = observation::SightLines::new(actor.flight.position, &ground, Some(&ceiling));
    assert!(sight.blocked(actor.flight.position, behind));
    assert!(crate::combat::live::terrain_hit(actor.flight.position, behind, &ground).is_some());
    let before = reads.load(Ordering::Relaxed);
    // Asked again: the kept answer, no new samples.
    assert!(sight.blocked(actor.flight.position, behind));
    assert_eq!(reads.load(Ordering::Relaxed), before);
    // From somewhere else: sampled again, the same answer.
    let other = [10_000., 3000., 2000.5];
    assert!(sight.blocked(other, behind));
    assert!(reads.load(Ordering::Relaxed) > before);
    // Above the ceiling: clear without a single ground read.
    let high = [10_000., 5_500., 2000.];
    let sight_high = observation::SightLines::new(high, &ground, Some(&ceiling));
    let before = reads.load(Ordering::Relaxed);
    assert!(!sight_high.blocked(high, over));
    assert_eq!(reads.load(Ordering::Relaxed), before);
    assert!(crate::combat::live::terrain_hit(high, over, &ground).is_none());
    // The lookout reports the hill, and the observation keeps the target out
    // exactly as the original did.
    let world = [
        visible_object(&actor, 2, behind),
        visible_object(&actor, 3, over),
    ];
    let mut reference = actor.clone();
    let expected = reference.observe_unsplit(0, &world, &|x, z| hills.height(x, z));
    let mut actual = actor.clone();
    let sight = observation::SightLines::new(actor.flight.position, &ground, Some(&ceiling));
    let targets = actual.observe_sighted(0, &world, &sight);
    assert_target_bits(&expected, &targets);
    assert_actor_state(&reference, &actual);
    let result = |id| {
        actual
            .trace
            .visual
            .iter()
            .find(|t| t.id == id)
            .map(|t| t.result)
    };
    assert_eq!(result(2), Some(awareness::VisualResult::Terrain));
    assert_eq!(result(3), Some(awareness::VisualResult::Visible));
    assert!(targets.iter().all(|t| t.id != 2));
    assert!(targets.iter().any(|t| t.id == 3));
}

/// Over random hills, observation through shared sight lines and the ground
/// ceiling gives the original's targets, memory, traces and sensor state bit
/// for bit, and the decision frame's terrain answers match `terrain_hit`.
#[test]
fn shared_sight_lines_over_hills_match_the_unsplit_reference() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut blocked_seen = 0;
    let mut clear_seen = 0;
    for round in 0..24 {
        let hills = Hills::random(24, 24, next() | 1, 14_000);
        let ceiling = hills.ceiling();
        let ground = |x: f64, z: f64| hills.height(x, z);
        let level = Experience::ALL[round % 4];
        let mut actor = sensor_actor(level);
        actor.sensors.as_mut().unwrap().controls.channel = match round % 3 {
            0 => sensors::Channel::Radar,
            1 => sensors::Channel::Infrared,
            _ => sensors::Channel::Visual,
        };
        actor.flight.yaw = (next() % 628) as f64 / 100.;
        let span = HILL_CELL_FT * 26.;
        let mut place = |low: f64, high: f64| {
            [
                (next() % 1_000_000) as f64 / 1e6 * span - HILL_CELL_FT,
                low + (next() % 1_000_000) as f64 / 1e6 * (high - low),
                (next() % 1_000_000) as f64 / 1e6 * span - HILL_CELL_FT,
            ]
        };
        actor.flight.position = place(500., 16_000.);
        let world: Vec<_> = (2..=31)
            .map(|id| {
                let mut object = visible_object(&actor, id, place(0., 18_000.));
                object.side = super::super::targeting::Side(1 + id % 2);
                let observable = object.observable.as_mut().unwrap();
                observable.radar_emitting = id % 3 == 0;
                observable.airborne = id % 11 != 0;
                object
            })
            .collect();
        let mut reference = actor.clone();
        let expected = reference.observe_unsplit(round as u64, &world, &ground);
        let mut actual = actor.clone();
        let sight = observation::SightLines::new(actor.flight.position, &ground, Some(&ceiling));
        let targets = actual.observe_sighted(round as u64, &world, &sight);
        assert_target_bits(&expected, &targets);
        assert_actor_state(&reference, &actual);
        // The prepared path, with the ceiling, against the original queries.
        let mut input = actor.observation_input(0).unwrap();
        input.ceiling = Some(&ceiling);
        let prepared = input.prepare(round as u64, &world, &ground, &ground);
        assert_target_bits(&prepared.observed.targets, &expected);
        for (target, blocked) in expected.iter().zip(&prepared.terrain_blocked) {
            let original =
                crate::combat::live::terrain_hit(actor.flight.position, target.position, &ground)
                    .is_some();
            assert_eq!(*blocked, original);
        }
        // Every pair, every order: the same answer as sampling.
        for object in &world {
            let to = object.position;
            let original =
                crate::combat::live::terrain_hit(actor.flight.position, to, &ground).is_some();
            assert_eq!(sight.blocked(actor.flight.position, to), original);
            assert_eq!(
                sight.blocked(to, actor.flight.position),
                crate::combat::live::terrain_hit(to, actor.flight.position, &ground).is_some()
            );
            if original {
                blocked_seen += 1;
            } else {
                clear_seen += 1;
            }
        }
    }
    assert!(
        blocked_seen > 50 && clear_seen > 50,
        "the hills must hide some lines and not others: {blocked_seen} {clear_seen}"
    );
}

/// A whole mission over hills: the ground ceiling and shared sight lines,
/// on every worker count, leave every actor's state, the journal and the
/// output exactly as the serial mission without the ceiling.
#[test]
fn the_ground_ceiling_keeps_hilly_missions_exact_on_every_executor() {
    let hills = Hills::random(20, 20, 0x1234_5678_9abc_def1, 14_000);
    let ceiling = std::sync::Arc::new(hills.ceiling());
    let terrain = |x: f64, z: f64| hills.height(x, z);
    let raised = |x: f64, z: f64| {
        if x.abs() <= 100. && z.abs() <= 4000. {
            Surface::runway(hills.height(x, z) + 50.)
        } else {
            Surface::terrain(hills.height(x, z))
        }
    };
    for workers in executors() {
        let mut reference = fixture(30, 41);
        // Over the hills: the fixture's two sides face each other across
        // the grid's middle.
        for actor in &mut reference.actors {
            actor.flight.position[0] += 18_000.;
            actor.flight.position[2] += 20_000.;
        }
        let mut actual = reference.clone();
        actual.set_ground_ceiling(Some(ceiling.clone()));
        for _ in 0..12 {
            let world = snapshot(&reference, None);
            let expected = reference.step_using(
                &world,
                &terrain,
                &raised,
                TimeOfDay(reference.tick),
                &tore_workers::Executor::serial(),
            );
            let result =
                actual.step_using(&world, &terrain, &raised, TimeOfDay(actual.tick), &workers);
            assert_eq!(expected, result);
            actual.set_ground_ceiling(None);
            assert_mission_state(&reference, &actual);
            actual.set_ground_ceiling(Some(ceiling.clone()));
        }
    }
}
