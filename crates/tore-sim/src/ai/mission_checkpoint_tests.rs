//! Tests of the AI mission's coders (stage H slice H4): values built by
//! scripted missions round-trip, and a mission restored from its bytes steps
//! on exactly like the one it came from.
//!
//! The oracle is the one the golden fixtures use: the same host script drives
//! the original and its restored twin, and on every tick their outputs and
//! every actor's flight state must be equal; every 30 ticks and at the end
//! their codings must be byte for byte equal too, which compares everything
//! the coders hold (the why-records and scratch they leave out cannot differ
//! the future, or the flights would part).

use super::super::tests::{flat, object, perception_actor, setup, synthetic_profile};
use super::*;
use crate::ai::ScalarSpeed;
use crate::ai::airfield::{self, GroundStart, LandingOrder, LandingReason, Phase, RunwayView};
use crate::ai::controller::ThreatReport;
use crate::ai::engagement::{self, HostileEscort, PatrolRegion, Role, Stance};
use crate::ai::mission::{MissionOutput, WorldObject};
use crate::ai::targeting::Side;
use crate::ai::threat::{SeekerClass, TimeOfDay};
use crate::ai::wing::{TargetOrder, WingRequest};
use crate::ai::{Experience, incoming_fire};
use crate::airport::ApproachEnd;
use crate::checkpoint::{Coded, Models, from_bytes, round_trip, to_bytes};
use crate::combat::missiles::Guidance;
use crate::combat::threats::MissileSnapshot;
use crate::flight;
use crate::research::Surface;
use crate::sensors::{self, Sensors};
use tore_formats::aircraft::AircraftId;

// ---------------------------------------------------------------------------
// The harness.

/// Every flight model the mission's actors fly, as a world's table holds
/// them.
fn models_of(mission: &AiMission) -> Models {
    let mut models = Models::default();
    for actor in mission.actors() {
        models
            .insert(actor.identity().aircraft, actor.flight().import_model())
            .unwrap();
    }
    models
}

fn coded(mission: &AiMission) -> Coded {
    to_bytes(mission, &models_of(mission)).expect("the mission codes")
}

/// The mission restored from its own bytes.
fn restored(mission: &AiMission) -> AiMission {
    from_bytes(&coded(mission), &models_of(mission)).expect("the mission restores")
}

/// Steps `original` and its restored twin `ticks` ticks with the same host
/// script and requires everything to agree. Returns the twin.
fn step_on(
    original: &mut AiMission,
    host: &mut dyn FnMut(&mut AiMission) -> MissionOutput,
    ticks: u64,
) {
    let mut twin = restored(original);
    assert_eq!(coded(&twin), coded(original), "restored but not equal");
    for tick in 0..ticks {
        let a = host(original);
        let b = host(&mut twin);
        assert_eq!(a, b, "outputs differ {tick} ticks after the restore");
        for (x, y) in original.actors().iter().zip(twin.actors()) {
            assert_eq!(
                x.flight(),
                y.flight(),
                "actor {} flies differently {tick} ticks after the restore",
                x.id()
            );
            assert_eq!(x.activity(), y.activity());
        }
        if tick % 30 == 29 {
            assert_eq!(
                coded(&twin),
                coded(original),
                "state differs {tick} ticks after the restore"
            );
        }
    }
    assert_eq!(coded(&twin), coded(original), "state differs at the end");
}

// ---------------------------------------------------------------------------
// A fight with warnings, hits, a lost human leader and a mission of
// opportunity.

const HUMAN: u32 = 10;
/// The tick the human leader of wing 1 leaves the world.
const HUMAN_LOST: u64 = 700;

fn sensor_actor(id: u32, side: u32, member: u8, position: [f64; 3], yaw: f64) -> AiActor {
    let mut setup = setup(id, side, member, position, yaw);
    setup.sensors = Some(Sensors::new(sensors::SensorProfiles {
        aircraft: AircraftId::F18,
        radar: None,
        infrared: None,
        visual: None,
        jammer: None,
        signature: sensors::SignatureProfile::default(),
    }));
    AiActor::new(setup).unwrap()
}

fn wing_member(id: u32, side: u32, wing: u8, member: u8, position: [f64; 3], yaw: f64) -> AiActor {
    let mut setup = setup(id, side, member, position, yaw);
    setup.identity.wing = wing;
    AiActor::new(setup).unwrap()
}

fn fight() -> AiMission {
    use std::f64::consts::PI;
    let mut mission = AiMission::new();
    // Wing 0 of side 1 against wing 0 of side 2, head on at 20,000 ft.
    mission.push(sensor_actor(1, 1, 0, [0., 20_000., 0.], 0.));
    mission.push(AiActor::new(setup(2, 1, 1, [1_500., 20_000., -300.], 0.)).unwrap());
    mission.push(sensor_actor(3, 2, 0, [0., 20_000., 34_000.], PI));
    mission.push(AiActor::new(setup(4, 2, 1, [-1_500., 20_000., 34_300.], PI)).unwrap());
    // Wing 1 of side 1: a human leader and two AI wingmen far to the east.
    mission.push(wing_member(5, 1, 1, 1, [60_000., 18_000., -2_000.], 0.));
    mission.push(wing_member(6, 1, 1, 2, [61_500., 18_000., -2_300.], 0.));
    mission.set_humans(vec![HumanMember {
        id: HUMAN,
        side: Side(1),
        wing: 1,
        member: 0,
        pilot_alive: true,
    }]);
    mission.set_wing_route(
        Side(1),
        1,
        vec![[60_000., 18_000., 30_000.], [20_000., 18_000., 60_000.]],
    );
    mission.set_must_survive(HUMAN, vec![5, 6]);
    mission.set_priority_landing(HUMAN, Some(7));
    mission.set_human_assignment(
        HUMAN,
        engagement::Assignment {
            role: Role::Escort,
            stance: Stance::ProtectAssigned,
            protected_ids: vec![5],
            destroy_ids: vec![3],
            hostile_escorts: vec![HostileEscort {
                principal_id: 3,
                escort_id: 4,
            }],
            patrol: Some(PatrolRegion {
                clip: None,
                center_ft: [0., 20_000., 20_000.],
                radius_ft: 30_000.,
            }),
        },
    );
    mission.start_in_formation();
    mission
}

fn human_object(template: &AiActor, tick: u64) -> WorldObject {
    let mut o = object(template, 1);
    o.id = HUMAN;
    o.human_controlled = true;
    o.position = [60_000., 18_000., -2_000. + 4. * tick as f64];
    o.velocity = [0., 0., 480.];
    o.speed = ScalarSpeed(480.);
    o.heading_deg = 0.;
    o
}

/// One tick of the fight's host: the world snapshot, the missile and round
/// lists, and the scripted events, all functions of the tick alone.
fn fight_host(mission: &mut AiMission) -> MissionOutput {
    let tick = mission.tick();
    let mut world: Vec<WorldObject> = mission
        .actors()
        .iter()
        .map(|a| object(a, a.identity().side.0))
        .collect();
    if tick < HUMAN_LOST {
        let template = mission.actor(1).unwrap();
        world.push(human_object(template, tick));
    }
    // An infrared missile closes on actor 1 from behind between ticks 150
    // and 450.
    let missiles = match mission.actor(1) {
        Some(target) if (150..450).contains(&tick) => {
            let f = target.flight();
            let behind = crate::attitude::Basis::new(f.yaw, 0., 0.).forward;
            let travelled = (tick - 150) as f64 * 12.;
            vec![MissileSnapshot {
                id: 99,
                owner: 3,
                position: std::array::from_fn(|i| f.position[i] - behind[i] * (9_000. - travelled)),
                velocity: behind.map(|v| v * 1_900.),
                guidance: Guidance::Infrared,
                target: Some(1),
                radar_active: false,
                radar_acquired: false,
                supported: false,
                supporting_radar_position: None,
                alive: true,
            }]
        }
        _ => Vec::new(),
    };
    mission.set_missiles(missiles);
    // Tracer fire passes actor 2 between ticks 250 and 300.
    let rounds = match mission.actor(2) {
        Some(target) if (250..300).contains(&tick) => {
            let at = target.flight().position;
            vec![incoming_fire::Round {
                id: 700 + tick as u32,
                owner: 4,
                position: [at[0] + 20., at[1], at[2] + 5.],
                previous: [at[0] + 20., at[1], at[2] - 20.],
                tracer: true,
            }]
        }
        _ => Vec::new(),
    };
    mission.set_gun_rounds(rounds);
    match tick {
        200 => mission.actor_mut(2).unwrap().report_threat(ThreatReport {
            missile_id: 501,
            seeker: SeekerClass::Radar,
            launcher_id: 4,
            launcher_same_side: false,
            distance_at_launch_ft: 24_000.,
            launch_tick: 190,
        }),
        330 => mission.actor_mut(2).unwrap().report_hit(),
        360 => mission.report_attack(
            1,
            engagement::ThreatReport {
                attacker_id: Some(3),
                defended_id: 2,
            },
        ),
        500 => {
            let _ = mission.order(5, WingRequest::TargetAssignment(TargetOrder::HoldFire));
        }
        _ => {}
    }
    mission
        .step(&world, &flat, TimeOfDay(0))
        .expect("the mission steps")
}

/// What the scripted fight has put into the state a coder must hold.
#[derive(Default, Debug)]
struct Seen {
    defense_decision: bool,
    incoming_cue: bool,
    attack_memory: bool,
    awareness: bool,
    sensors: bool,
    device_schedule: bool,
    pending_events: bool,
    opportunity: bool,
    human_assignment: bool,
}

impl Seen {
    fn look(&mut self, mission: &AiMission) {
        for actor in mission.actors() {
            self.defense_decision |= actor.last_defense.is_some();
            self.incoming_cue |= actor.incoming_fire.cue().is_some();
            self.attack_memory |= !actor.observed_attacks.is_empty();
            self.awareness |= actor.awareness.current_observations().count() > 0;
            self.sensors |= actor.sensors.is_some();
            self.device_schedule |= !actor.device_schedule.is_empty();
            self.pending_events |= !actor.pending_events.is_empty();
        }
        self.opportunity |= !mission.opportunities.is_empty();
        self.human_assignment |= !mission.human_assignments.is_empty();
    }
}

#[test]
fn a_fight_restores_at_any_tick_and_steps_on_identically() {
    let mut mission = fight();
    let mut seen = Seen::default();
    let mut checked = Vec::new();
    while mission.tick() < 1_800 {
        if [1, 220, 340, 460, 800, 1_200].contains(&mission.tick()) {
            checked.push(mission.tick());
            step_on(&mut mission.clone(), &mut fight_host, 600);
        }
        fight_host(&mut mission);
        seen.look(&mission);
    }
    assert_eq!(checked.len(), 6);
    assert!(seen.defense_decision, "{seen:?}");
    assert!(seen.incoming_cue, "{seen:?}");
    assert!(seen.attack_memory, "{seen:?}");
    assert!(seen.awareness, "{seen:?}");
    assert!(seen.sensors, "{seen:?}");
    assert!(seen.opportunity, "{seen:?}");
    assert!(seen.human_assignment, "{seen:?}");
    // The lead of wing 1 passed from the lost human to an AI wingman.
    assert_eq!(mission.wing_leader(Side(1), 1), Some(5));
}

/// The AI test settings (slice S7b) are host setup, not state: a fight at a
/// 30 Hz observation rate with lean sensors restores between two
/// observations (the ticks are not multiples of 4 for most aircraft) and steps
/// on identically, byte for byte, when the host sets the settings again.
#[test]
fn a_fight_at_a_slow_observation_rate_restores_between_observations() {
    use crate::ai::test_settings::AiTestSettings;
    let settings = AiTestSettings::default()
        .with_observation_hz(30)
        .unwrap()
        .with_lean_sensors(true);
    let mut host = |mission: &mut AiMission| {
        mission.set_test_settings(settings);
        fight_host(mission)
    };
    let mut mission = fight();
    let mut checked = 0;
    while mission.tick() < 900 {
        if [221, 342, 463, 603].contains(&mission.tick()) {
            checked += 1;
            step_on(&mut mission.clone(), &mut host, 300);
        }
        host(&mut mission);
    }
    assert_eq!(checked, 4);
    // The sensors' clock kept pace with the mission's, four ticks a step.
    let ticks = mission.actor(1).unwrap().sensors().unwrap().tick();
    assert!(ticks.abs_diff(900) <= 3, "{ticks}");
}

// ---------------------------------------------------------------------------
// The airfield: ground starts, takeoffs and landings on a flat runway.

const AIRPORT: u32 = 7;

fn runway() -> RunwayView {
    RunwayView {
        airport: AIRPORT,
        object: 70,
        center: [0., 0., 0.],
        heading: 0.,
        length_ft: 8000.,
        elevation_ft: 0.,
        anchors: None,
    }
}

/// The runway with anchors: parking east, a taxiway, a takeoff spot.
fn anchored_runway() -> RunwayView {
    let at = |x: f64, z: f64| [x, 0., z];
    RunwayView {
        anchors: Some(airfield::AirfieldAnchors {
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
        ..runway()
    }
}

/// Landable within the runway rectangle, flat terrain elsewhere.
fn surface(x: f64, z: f64) -> Surface {
    if x.abs() <= 100. && z.abs() <= 4000. {
        Surface::runway(0.)
    } else {
        Surface::terrain(0.)
    }
}

fn hornet(id: u32, member: u8, position: [f64; 3], yaw: f64) -> AiActor {
    let mut setup = setup(id, 1, member, position, yaw);
    let mut flight = flight::State::new(&synthetic_profile(AircraftId::F18), position).unwrap();
    flight.yaw = yaw;
    flight.velocity = crate::attitude::Basis::new(yaw, 0., 0.)
        .forward
        .map(|v| v * flight.speed);
    setup.flight = flight;
    setup.home_airport = None;
    AiActor::new(setup).unwrap()
}

fn parked(id: u32, order: u8, position: [f64; 3], runway: RunwayView) -> AiActor {
    let mut actor = hornet(id, order, position, 0.);
    actor.flight_mut().enable_research(id as i32).unwrap();
    actor.flight_mut().start_on_runway(position, 0.).unwrap();
    actor.start_on_ground(GroundStart {
        runway,
        end: ApproachEnd::Near,
        order,
    });
    actor
}

/// One tick of the airfield host: every actor as a world object, plus a
/// human leader that is parked for 30 s and then rolls and climbs
/// away (the wingmen wait for it).
fn airfield_host(mission: &mut AiMission) -> MissionOutput {
    let tick = mission.tick();
    let mut world: Vec<WorldObject> = mission
        .actors()
        .iter()
        .map(|a| {
            let mut o = object(a, 1);
            o.on_ground = a.flight().research.as_ref().is_some_and(|r| r.on_ground);
            o
        })
        .collect();
    {
        let mut human = object(&hornet(99, 0, [0.; 3], 0.), 1);
        human.id = HUMAN;
        human.human_controlled = true;
        let t = tick as f64 / 120.;
        let rolling = (t - 30.).max(0.);
        let speed = (8. * rolling).min(400.);
        let liftoff = 250. / 8.;
        let along = if rolling < liftoff {
            4. * rolling * rolling
        } else {
            4. * liftoff * liftoff
                + 250. * (rolling - liftoff)
                + 0.5 * 8. * (rolling - liftoff).powi(2)
        };
        let climb = (rolling - liftoff).max(0.) * 20.;
        human.position = [0., climb, -3000. + along];
        human.velocity = [0., if climb > 0. { 20. } else { 0. }, speed];
        human.speed = ScalarSpeed(speed);
        human.on_ground = climb <= 0.;
        world.push(human);
    }
    mission
        .step_with_surface(&world, &|x, z| surface(x, z).height, &surface, TimeOfDay(0))
        .expect("the mission steps")
}

/// The first tick each phase is seen on any actor, running `mission` on with
/// `host`: restores at each, steps 600 ticks on, and returns the phases met.
fn restore_at_each_phase(
    mission: &mut AiMission,
    host: &mut dyn FnMut(&mut AiMission) -> MissionOutput,
    limit: u64,
) -> Vec<Phase> {
    let mut met: Vec<Phase> = Vec::new();
    while mission.tick() < limit {
        let phase = mission
            .actors()
            .iter()
            .find_map(|a| a.airfield_phase().filter(|p| !met.contains(p)));
        if let Some(phase) = phase {
            met.push(phase);
            step_on(&mut mission.clone(), host, 600);
        }
        host(mission);
    }
    met
}

#[test]
fn a_ground_start_restores_in_every_phase_of_the_departure() {
    for runway in [runway(), anchored_runway()] {
        let mut mission = AiMission::new();
        let anchored = runway.anchors.is_some();
        let parking = |slot: usize| match runway.anchors {
            Some(anchors) => anchors.parking[slot],
            None => [40., 0., -3250. - 250. * slot as f64],
        };
        mission.push(parked(1, 1, parking(0), runway));
        mission.push(parked(2, 2, parking(1), runway));
        mission.set_external_leader(Side(1), 0, HUMAN);
        mission.start_in_formation();
        let met = restore_at_each_phase(&mut mission, &mut airfield_host, 30_000);
        assert!(met.contains(&Phase::Waiting), "{met:?}");
        assert!(met.contains(&Phase::TakeoffRoll), "{met:?}");
        assert!(met.contains(&Phase::ClimbOut), "{met:?}");
        if anchored {
            assert!(met.contains(&Phase::Taxi), "{met:?}");
            assert!(met.contains(&Phase::LineUp), "{met:?}");
        }
    }
}

#[test]
fn a_landing_restores_in_every_phase_of_the_approach() {
    let runway = anchored_runway();
    let mut mission = AiMission::new();
    let mut actor = hornet(1, 1, [-3_000., 5_000., -40_000.], 0.3);
    actor.set_home_runway(Some(runway));
    mission.push(actor);
    mission
        .order(
            1,
            WingRequest::Land(LandingOrder {
                runway,
                reason: LandingReason::Ordered,
            }),
        )
        .unwrap()
        .unwrap();
    let mut host = |m: &mut AiMission| {
        let world: Vec<WorldObject> = m
            .actors()
            .iter()
            .map(|a| {
                let mut o = object(a, 1);
                o.on_ground = a.flight().research.as_ref().is_some_and(|r| r.on_ground);
                o
            })
            .collect();
        m.step_with_surface(&world, &|x, z| surface(x, z).height, &surface, TimeOfDay(0))
            .expect("the mission steps")
    };
    let met = restore_at_each_phase(&mut mission, &mut host, 60_000);
    for phase in [Phase::Marshal, Phase::Approach, Phase::Final] {
        assert!(met.contains(&phase), "{phase:?} never met: {met:?}");
    }
}

/// A wounded aircraft returning to land, a gun carrier, a ground start, a
/// dummy and a lost aircraft restore and step on together; the runway views
/// of all of them, and the gun record of both guns, are one shared record
/// each.
#[test]
fn orders_guns_and_runway_views_restore_and_share_their_records() {
    let runway = anchored_runway();
    let mut mission = AiMission::new();
    // Wounded: returns to land under a damage order.
    let mut wounded = hornet(1, 0, [0., 4_000., -30_000.], 0.);
    wounded.set_home_runway(Some(runway));
    wounded.flight_mut().systems.hit(34, 0.7);
    // Guns on two stations, the same record.
    let mut gunner = hornet(2, 1, [3_000., 15_000., -20_000.], 0.);
    let gun = crate::combat::gunsight::tests::weapon();
    gunner.set_guns(std::collections::BTreeMap::from([
        (1, gun.clone()),
        (2, gun),
    ]));
    mission.push(wounded);
    mission.push(gunner);
    mission.push(parked(
        3,
        3,
        anchored_runway().anchors.unwrap().parking[2],
        runway,
    ));
    let mut dummy = hornet(4, 0, [9_000., 12_000., 0.], 1.);
    dummy.set_dummy();
    mission.push(dummy);
    let mut lost = hornet(5, 0, [-9_000., 12_000., 0.], 1.);
    lost.set_alive(false);
    mission.push(lost);
    mission.set_external_leader(Side(1), 0, HUMAN);
    mission.start_in_formation();
    let mut host = |m: &mut AiMission| airfield_host(m);
    for _ in 0..200 {
        host(&mut mission);
    }
    let wounded = mission.actor(1).unwrap();
    assert_eq!(
        wounded.damage_return(),
        Some(crate::ai::damage::Reason::Pilot)
    );
    assert_eq!(
        wounded.landing_order().unwrap().reason,
        LandingReason::Damage
    );
    assert!(mission.actor(3).unwrap().ground_start().is_some());
    assert!(mission.actor(4).unwrap().is_dummy());
    let coding = coded(&mission);
    assert_eq!(
        coding.records.len(),
        2,
        "one runway view and one gun record, however many copies"
    );
    step_on(&mut mission, &mut host, 600);
}

// ---------------------------------------------------------------------------
// Values.

fn same<T: Checkpoint + PartialEq + std::fmt::Debug>(value: T) {
    let copy = round_trip(&value, &Models::default()).expect("it round-trips");
    assert_eq!(copy, value);
}

#[test]
fn awareness_defense_and_assignment_values_round_trip() {
    use crate::ai::awareness::{Lookout, ObservationSource, Snapshot, SourceTimestamps};
    use crate::ai::defense::{BurstRequest, DefenseState, Maneuver, MotionSuggestion};
    // Memory and lookout of a perception actor that has seen a target.
    let mut actor = perception_actor(Experience::Ace);
    let seen = {
        let mut o = object(&actor, 2);
        o.id = 2;
        o.position = [0., 20_000., 4_000.];
        o
    };
    let mut mission = AiMission::new();
    mission.push(std::mem::replace(
        &mut actor,
        perception_actor(Experience::Novice),
    ));
    let mut world: Vec<WorldObject> = mission.actors().iter().map(|a| object(a, 1)).collect();
    world.push(seen);
    for _ in 0..30 {
        mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    }
    same(mission.actor(1).unwrap().awareness.clone());
    same(Snapshot {
        target: crate::ai::controller::TargetView {
            infrared_strength: 1.,
            id: 2,
            side: Side(2),
            position: [1., 2., 3.],
            heading_deg: 4.,
            pitch_deg: 5.,
            speed: ScalarSpeed(6.),
            maximum_speed: ScalarSpeed(7.),
            is_aircraft: true,
            is_fighter: false,
            human_controlled: true,
            valid: false,
            type_allowed: true,
            seeker_eligible: false,
            wing_attackers: 9,
            terrain_blocked: true,
            sensor_supported: false,
            link_track: false,
        },
        velocity: [-1., 0., f64::MIN_POSITIVE],
        first_observed_tick: 11,
        last_observed_tick: 12,
        source_ticks: SourceTimestamps {
            visual: Some(1),
            radar: None,
            infrared: Some(3),
            fixture: Some(4),
        },
    });
    for source in [
        ObservationSource::Visual,
        ObservationSource::Radar,
        ObservationSource::Infrared,
        ObservationSource::Fixture,
    ] {
        same(source);
    }
    same(Lookout {
        position: [1., 2., 3.],
        forward: [0., 0., 1.],
        scan: [1., 0., 0.],
        attention: Some([4., 5., 6.]),
        sector: 5,
    });
    same(DefenseState::default());
    same(BurstRequest::MIXED);
    for maneuver in [
        Maneuver::Jink,
        Maneuver::Notch,
        Maneuver::Drag,
        Maneuver::Beam,
        Maneuver::Break,
    ] {
        same(MotionSuggestion {
            maneuver,
            heading_deg: -45.,
            flight_path_pitch_deg: 3.5,
            reduce_heat: true,
        });
    }
    same(engagement::Policy::default());
    for role in [
        Role::FreeEngagement,
        Role::CombatAirPatrol,
        Role::Intercept,
        Role::Escort,
        Role::Disengage,
    ] {
        same(role);
    }
    for stance in [
        Stance::WeaponsHold,
        Stance::SelfDefense,
        Stance::ProtectAssigned,
        Stance::EngageAssigned,
    ] {
        same(stance);
    }
    same(engagement::Assignment::default());
}

#[test]
fn opportunity_route_and_ejection_values_round_trip() {
    use crate::ai::opportunity::{HomeReason, LastKnown, Opportunity, SearchEnd};
    use crate::ai::route::{Octant, Position};
    let mut opportunity = Opportunity::new(Side(2), 3, 900, vec![[1., 2., 3.], [4., 5., 6.]]);
    opportunity.points = vec![
        LastKnown {
            id: 7,
            position: [10., 20., 30.],
            observed_tick: 800,
            searched: true,
        },
        LastKnown {
            id: 9,
            position: [-1., -2., -3.],
            observed_tick: 850,
            searched: false,
        },
    ];
    opportunity.searching = Some(9);
    opportunity.arrived_tick = Some(1_000);
    opportunity.search_over = Some((1_100, SearchEnd::SearchTimeUp));
    opportunity.route_sector = Some(Octant::new(5).unwrap());
    opportunity.route_started = true;
    for reason in [
        HomeReason::NotCleared,
        HomeReason::SearchOver(SearchEnd::NothingKnown),
        HomeReason::SearchOver(SearchEnd::AllSearched),
        HomeReason::SearchOver(SearchEnd::SearchTimeUp),
        HomeReason::RouteFlown,
    ] {
        let mut homed = opportunity.clone();
        homed.home = Some((1_200, reason));
        same(homed);
    }
    same(opportunity);
    same(Position { x: -1.5, z: 2.5 });
    // An octant outside 0..8 in the bytes is refused.
    let mut s = crate::checkpoint::Saver::new();
    9u8.save(&mut s, None).unwrap();
    let coded = Coded {
        body: s.finish_section(),
        records: Vec::new(),
    };
    assert!(from_bytes::<Octant>(&coded, &Models::default()).is_err());
    for reason in [
        crate::ai::damage::Reason::Fire,
        crate::ai::damage::Reason::Structure,
    ] {
        same(reason);
    }
    // The escape monitor's count and random stream, mid-episode.
    let mut monitor = crate::ejection::Monitor::seeded(77);
    for _ in 0..500 {
        monitor.step(Some(crate::ejection::Assessment {
            hazard: crate::ejection::Hazard::Fire,
            impact_seconds: 30.,
        }));
    }
    let copy = round_trip(&monitor, &Models::default()).unwrap();
    assert_eq!(copy, monitor);
    let mut a = monitor;
    let mut b = copy;
    let danger = Some(crate::ejection::Assessment {
        hazard: crate::ejection::Hazard::Dive,
        impact_seconds: 1.,
    });
    for _ in 0..600 {
        assert_eq!(a.step(danger), b.step(danger));
    }
}

#[test]
fn a_mission_without_actors_and_damaged_bytes_are_handled() {
    let mut mission = AiMission::new();
    mission.set_formation(crate::ai::wing::Formation::LineAbreast);
    mission.set_wing_control(crate::ai::wing::WingControl::Tight);
    mission.set_spacing(1_234, -567);
    let copy = restored(&mission);
    assert_eq!(copy.formation(), crate::ai::wing::Formation::LineAbreast);
    assert_eq!(coded(&copy), coded(&mission));
    // A mission with actors, cut short or flipped, is refused or decodes to
    // something else, and never panics.
    let fought = {
        let mut m = fight();
        for _ in 0..300 {
            fight_host(&mut m);
        }
        m
    };
    let models = models_of(&fought);
    let good = to_bytes(&fought, &models).unwrap();
    for cut in [0, 1, 7, good.body.len() / 3, good.body.len() - 1] {
        let short = Coded {
            body: good.body[..cut].to_vec(),
            records: good.records.clone(),
        };
        assert!(
            from_bytes::<AiMission>(&short, &models).is_err(),
            "cut {cut}"
        );
    }
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..300 {
        let mut body = good.body.clone();
        for _ in 0..3 {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let at = (seed >> 33) as usize % body.len();
            body[at] ^= 1 << ((seed >> 20) % 8);
        }
        let damaged = Coded {
            body,
            records: good.records.clone(),
        };
        let _ = from_bytes::<AiMission>(&damaged, &models);
    }
}

#[test]
fn the_sort_clock_and_the_yields_of_the_data_link_leads_round_trip() {
    // Slice G4: each flight's last sort and each actor's yields are state, so
    // a restored mission holds them and steps on as the original does.
    let mut mission = AiMission::new();
    mission.push(sensor_actor(1, 1, 0, [0., 20_000., 0.], 0.));
    mission.push(sensor_actor(2, 1, 1, [800., 20_000., 0.], 0.));
    mission.stamp_sort(Side(1), 0, 4_321);
    mission.stamp_sort(Side(2), 1, 99);
    assert!(mission.yield_target(2, 77));
    assert!(mission.yield_target(2, 78));
    let before = coded(&mission);
    let copy = restored(&mission);
    assert_eq!(coded(&copy), before);
    assert_eq!(copy.last_sort(Side(1), 0), Some(4_321));
    assert_eq!(copy.last_sort(Side(2), 1), Some(99));
    assert_eq!(copy.last_sort(Side(2), 0), None);
    assert_eq!(
        copy.actor(2).unwrap().yields(),
        mission.actor(2).unwrap().yields()
    );
    assert_eq!(copy.actor(2).unwrap().yields().len(), 2);
    // A yield announced by a step is announced once, restored or not.
    let mut original = mission;
    let mut host = |m: &mut AiMission| {
        let world: Vec<_> = m.actors().iter().map(|a| object(a, 1)).collect();
        let tick = m.tick();
        m.step(&world, &flat, TimeOfDay(tick)).unwrap()
    };
    step_on(&mut original, &mut host, 60);
    assert!(
        original
            .actor(2)
            .unwrap()
            .yields()
            .iter()
            .all(|y| y.announced)
    );
}

// ---------------------------------------------------------------------------
// Traffic avoidance: the held heading and the aircraft that set it.

fn avoidance_host(mission: &mut AiMission) -> MissionOutput {
    let world: Vec<WorldObject> = mission.actors().iter().map(|a| object(a, 1)).collect();
    mission
        .step_with_surface(&world, &|x, z| surface(x, z).height, &surface, TimeOfDay(0))
        .expect("the mission steps")
}

#[test]
fn a_held_avoidance_heading_restores_with_the_aircraft_that_set_it() {
    // B4b (2026-10-06): `avoiding_from` is coded after `avoiding`. Two
    // aircraft meeting head-on hold their headings; the twin restored while
    // they hold must code the same and fly on identically through the pass.
    let mut mission = AiMission::new();
    mission.push(hornet(1, 0, [0., 6_000., -6_000.], 0.));
    let mut other = hornet(2, 0, [0., 6_000., 6_000.], std::f64::consts::PI);
    other.identity.wing = 2;
    mission.push(other);
    let mut held = false;
    for _ in 0..10 * 120 {
        avoidance_host(&mut mission);
        if mission.actor(1).unwrap().avoiding_heading_deg().is_some() {
            held = true;
            break;
        }
    }
    assert!(held, "the head-on pair never took up an avoidance heading");
    step_on(&mut mission.clone(), &mut avoidance_host, 15 * 120);
}
