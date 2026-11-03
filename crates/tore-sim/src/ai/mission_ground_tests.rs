//! The strikers' ground picture (docs/spec/ai-strike.md, "What the AI knows"
//! and "Perceived threat and available speed"): briefed targets known from
//! the start, ground contacts from the A-G radar, FLIR and eye rebuilt every
//! 6 s and followed every quarter second, the perception limits, the threat
//! inputs, and CAP actors untouched. Synthetic objects only.
use super::super::tests::{flat, object, setup};
use super::super::{AiActor, AiMission};
use crate::ai::awareness::ground::{
    DefenseFacts, GROUND_FOLLOW_TICKS, GROUND_REBUILD_TICKS, GroundScene, GroundSubject,
    RecentWarnings, RetailAttention, TEAM_SPOT_FT, ThreatLevel, off_nose_deg, retail_attention,
    team_spot_ft,
};
use crate::ai::awareness::{Observation, ObservationSource};
use crate::ai::controller::TargetView;
use crate::ai::engagement::{StrikeObjective, StrikeRole};
use crate::ai::mission::WorldObject;
use crate::ai::targeting::{self, ObjectId, Side, StrikeSource};
use crate::ai::threat::TimeOfDay;
use crate::ai::{ScalarSpeed, mission::EquipmentFaults};
use crate::attitude::Vector;
use crate::checkpoint::{Models, from_bytes, to_bytes};
use crate::sensors::profile::{InfraredProfile, Preset, RadarProfile, Volume};
use crate::sensors::{self, FEET_PER_NAUTICAL_MILE, GroundObject, Sensors, Weather};
use tore_formats::aircraft::AircraftId;

const NMI: f64 = FEET_PER_NAUTICAL_MILE;
const STRIKER: u32 = 1;
const ALTITUDE: f64 = 5_000.;

fn volume(maximum_ft: f64, half_angle_deg: f64) -> Volume {
    Volume {
        azimuth_rad: half_angle_deg.to_radians(),
        elevation_rad: half_angle_deg.to_radians(),
        minimum_ft: 0.,
        maximum_ft,
        minimum_relative_ft: f64::NEG_INFINITY,
        maximum_relative_ft: f64::INFINITY,
    }
}

/// A 90 nmi radar, a 20 nmi FLIR and a 6 nmi eye, as the ground page tests'.
fn sensors() -> Sensors {
    let preset = Preset::Advanced;
    Sensors::new(sensors::SensorProfiles {
        aircraft: AircraftId::F18,
        radar: Some(RadarProfile {
            record: "TESTR.SEE".into(),
            search: volume(90. * NMI, 60.),
            track: volume(50. * NMI, 60.),
            look_down: 50.,
            preset,
            notch: preset.notch(),
            resistance: preset.resistance(),
            band: 0,
            source_flags: [0; 2],
            source_doppler: [0; 3],
        }),
        infrared: Some(InfraredProfile {
            record: "TESTI.SEE".into(),
            search: volume(20. * NMI, 60.),
            track: volume(10. * NMI, 60.),
        }),
        visual: Some(InfraredProfile {
            record: "TESTV.SEE".into(),
            search: volume(6. * NMI, 90.),
            track: volume(6. * NMI, 90.),
        }),
        jammer: None,
        signature: Default::default(),
    })
}

/// A side 1 F/A-18 at 5,000 ft over the origin, flying north (+z).
fn actor(id: u32, side: u32) -> AiActor {
    let mut setup = setup(id, side, 0, [0., ALTITUDE, 0.], 0.);
    setup.sensors = Some(sensors());
    let mut actor = AiActor::new(setup).unwrap();
    // Level, wings level, so the lists' volumes are the fixtures'.
    actor.flight_mut().pitch = 0.;
    actor.flight_mut().bank = 0.;
    actor
}

fn mission_with(actor: AiActor) -> AiMission {
    let mut mission = AiMission::new();
    mission.push(actor);
    mission
}

/// A side 2 tank `ahead_nmi` north, `right_ft` east, on flat ground: class
/// 0x400, listed, `maxVisDist` 78 (19,968 ft).
fn tank(id: u32, ahead_nmi: f64, right_ft: f64) -> GroundSubject {
    GroundSubject {
        object: GroundObject {
            id,
            position: [right_ft, 10., ahead_nmi * NMI],
            velocity: [0.; 3],
            class: 0x400,
            listed: true,
            max_visible: 78,
            radar: 100.,
            infrared: 100.,
            emitting: false,
            aircraft: false,
            airborne: false,
            airfield: false,
            destroyed: false,
        },
        side: Side(2),
        defense: None,
        hidden: false,
    }
}

fn sam(id: u32, ahead_nmi: f64, reach_ft: f64, working: bool) -> GroundSubject {
    let mut subject = tank(id, ahead_nmi, 0.);
    subject.object.class = 0x1000;
    subject.object.emitting = true;
    subject.defense = Some(DefenseFacts {
        missile_reach_ft: reach_ft,
        rotor_reach_ft: reach_ft,
        gun_reach_ft: 0.,
        radar: true,
        working,
        ship: false,
        heading_deg: 180.,
    });
    subject
}

fn aaa(id: u32, ahead_nmi: f64, reach_ft: f64) -> GroundSubject {
    let mut subject = tank(id, ahead_nmi, 0.);
    subject.object.class = 0x800;
    subject.defense = Some(DefenseFacts {
        missile_reach_ft: 0.,
        rotor_reach_ft: 0.,
        gun_reach_ft: reach_ft,
        radar: false,
        working: true,
        ship: false,
        heading_deg: 0.,
    });
    subject
}

fn observe_with(
    mission: &mut AiMission,
    tick: u64,
    subjects: &[GroundSubject],
    ground: &(dyn Fn(f64, f64) -> f64 + Sync),
) {
    mission.tick = tick;
    mission.observe_ground(
        GroundScene {
            painted: &[],
            launched: &[],
            subjects,
            weather: &Weather::default(),
        },
        ground,
    );
}

fn observe(mission: &mut AiMission, tick: u64, subjects: &[GroundSubject]) {
    observe_with(mission, tick, subjects, &flat);
}

fn contacts(mission: &AiMission) -> Vec<u32> {
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    picture.contacts().iter().map(|c| c.id).collect()
}

fn striker(objective: StrikeObjective) -> AiMission {
    let mut actor = actor(STRIKER, 1);
    actor.set_strike_role(StrikeRole::Strike);
    assert!(actor.needs_briefing());
    assert!(actor.set_strike_objective(objective));
    assert!(!actor.needs_briefing());
    mission_with(actor)
}

fn briefed_on(targets: Vec<u32>) -> StrikeObjective {
    StrikeObjective {
        targets,
        defenses: Vec::new(),
        point: Some([0., 10., 40. * NMI]),
    }
}

#[test]
fn a_striker_knows_its_briefing_and_lists_ground_contacts_every_6_seconds() {
    let mut mission = striker(briefed_on(vec![100]));
    // The briefed target stands 40 nmi north, beyond every list; 101 is on
    // the radar only (5 nmi: inside the A-G page's 3 x 19,968 ft, outside the
    // FLIR box and the eye's visibility distance); 102 is on all three.
    let mut scene = vec![tank(100, 40., 0.), tank(101, 5., 0.), tank(102, 2., 0.)];
    observe(&mut mission, 0, &scene);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert_eq!(picture.built_tick(), Some(0));
    assert_eq!(contacts(&mission), [102, 101], "nearest first");
    let near = picture.contact(102).unwrap();
    assert!(near.channels.radar && near.channels.infrared && near.channels.visual);
    let far = picture.contact(101).unwrap();
    assert!(far.channels.radar && !far.channels.infrared && !far.channels.visual);
    let briefed = &picture.briefed()[0];
    assert_eq!(briefed.id, 100);
    assert!(briefed.standing && !briefed.perceived, "known, not seen");
    assert_eq!(briefed.position, scene[0].object.position);

    // A tank driving into view is not listed until the next rebuild; a listed
    // one follows its object every quarter second.
    scene.push(tank(103, 3., 0.));
    scene[2].object.position[0] = 400.;
    observe(&mut mission, 1, &scene);
    assert_eq!(contacts(&mission), [102, 101], "no refresh between");
    let followed = |mission: &AiMission| {
        mission
            .actor(STRIKER)
            .unwrap()
            .ground_picture()
            .unwrap()
            .contact(102)
            .unwrap()
            .position
    };
    assert_eq!(followed(&mission)[0], 0.);
    observe(&mut mission, GROUND_FOLLOW_TICKS, &scene);
    assert_eq!(followed(&mission)[0], 400., "followed");
    assert_eq!(contacts(&mission), [102, 101], "a follow adds nothing");
    observe(&mut mission, GROUND_REBUILD_TICKS - 1, &scene);
    assert_eq!(contacts(&mission), [102, 101]);
    observe(&mut mission, GROUND_REBUILD_TICKS, &scene);
    assert_eq!(contacts(&mission), [102, 103, 101], "rebuilt at 6 s");
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert_eq!(picture.built_tick(), Some(GROUND_REBUILD_TICKS));
    assert_eq!(
        picture.seen().iter().map(|s| s.id).collect::<Vec<_>>(),
        [101, 102, 103]
    );

    // The briefed target comes into the lists once the striker is near.
    mission.actor_mut(STRIKER).unwrap().flight_mut().position = [0., ALTITUDE, 37. * NMI];
    observe(&mut mission, 2 * GROUND_REBUILD_TICKS, &scene);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert!(picture.briefed()[0].perceived);
    assert!(picture.contact(100).unwrap().channels.visual);
}

#[test]
fn the_refresh_runs_only_when_due() {
    let mut mission = striker(briefed_on(vec![]));
    assert!(mission.ground_due(), "never built");
    observe(&mut mission, 0, &[tank(102, 2., 0.)]);
    for (tick, due) in [
        (0, false),
        (1, false),
        (GROUND_FOLLOW_TICKS - 1, false),
        (GROUND_FOLLOW_TICKS, true),
        (GROUND_FOLLOW_TICKS + 1, false),
        (GROUND_REBUILD_TICKS, true),
    ] {
        mission.tick = tick;
        assert_eq!(mission.ground_due(), due, "tick {tick}");
    }
    // A dead striker perceives nothing.
    mission.tick = GROUND_FOLLOW_TICKS;
    mission.actor_mut(STRIKER).unwrap().set_alive(false);
    assert!(!mission.ground_due());
}

#[test]
fn perception_needs_a_working_sensor_a_listable_object_and_a_clear_line() {
    // Scenery without the listing flag, a neutral tank, a destroyed tank and
    // one beyond every list are never listed.
    let mut scenery = tank(110, 2., 0.);
    scenery.object.listed = false;
    let mut neutral = tank(111, 2., 300.);
    neutral.side = Side(0);
    let mut wreck = tank(112, 2., 600.);
    wreck.object.destroyed = true;
    let far = tank(113, 30., 0.);
    let seen = tank(114, 2., -300.);
    let scene = [scenery, neutral, wreck, far, seen];
    let mut mission = striker(briefed_on(vec![]));
    observe(&mut mission, 0, &scene);
    assert_eq!(contacts(&mission), [114]);

    // A ridge between 1 and 1.5 nmi hides everything beyond it.
    let ridge = |_x: f64, z: f64| {
        if (NMI..1.5 * NMI).contains(&z) {
            20_000.
        } else {
            0.
        }
    };
    let mut mission = striker(briefed_on(vec![]));
    observe_with(&mut mission, 0, &scene, &ridge);
    assert!(contacts(&mission).is_empty(), "terrain blocks the line");

    // A failed radar leaves the FLIR and the eye; failed everything, nothing.
    let mut mission = striker(briefed_on(vec![]));
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .fail_equipment(EquipmentFaults {
            radar: true,
            ..EquipmentFaults::default()
        });
    observe(&mut mission, 0, &[tank(101, 5., 0.), tank(102, 2., 0.)]);
    assert_eq!(contacts(&mission), [102]);
    let track = *mission
        .actor(STRIKER)
        .unwrap()
        .ground_picture()
        .unwrap()
        .contact(102)
        .unwrap();
    assert!(!track.channels.radar && track.channels.infrared && track.channels.visual);
    let mut mission = striker(briefed_on(vec![]));
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .fail_equipment(EquipmentFaults {
            radar: true,
            infrared: true,
            visual: true,
            rwr: false,
        });
    observe(&mut mission, 0, &[tank(102, 2., 0.)]);
    assert!(contacts(&mission).is_empty());
}

#[test]
fn a_destroyed_object_leaves_the_memory_and_the_briefing_knows_it() {
    let mut mission = striker(briefed_on(vec![102]));
    let mut scene = vec![tank(101, 2., 300.), tank(102, 2., 0.)];
    observe(&mut mission, 0, &scene);
    scene[0].object.destroyed = true;
    scene[1].object.destroyed = true;
    observe(&mut mission, GROUND_FOLLOW_TICKS, &scene);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert!(picture.seen().is_empty());
    assert!(
        picture.contact(101).unwrap().destroyed,
        "a listed wreck stays until the rebuild"
    );
    assert!(!picture.briefed()[0].standing);
    observe(&mut mission, GROUND_REBUILD_TICKS, &scene);
    assert!(contacts(&mission).is_empty());
}

#[test]
fn the_threat_inputs_hold_known_defences_and_fighters_within_30_nm() {
    let mut mission = striker(briefed_on(vec![102]));
    let target: Vector = [0., 10., 2. * NMI];
    // A SAM 3 nmi out covers the target (reach 30,000 ft); an AAA site 4 nmi
    // out reaches 6,000 ft, short of it; a SAM 50 nmi out is unknown.
    let scene = [
        tank(102, 2., 0.),
        sam(120, 3., 30_000., true),
        aaa(121, 4., 6_000.),
        sam(122, 50., 200_000., true),
    ];
    observe(&mut mission, 0, &scene);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    let threats = picture.threats();
    assert_eq!(
        threats.defenses.iter().map(|d| d.id).collect::<Vec<_>>(),
        [120, 121]
    );
    let own = Side(1);
    let calm = RecentWarnings::default();
    assert_eq!(threats.level(own, &[target], calm), ThreatLevel::Missiles);
    assert!(threats.fighters.is_empty());

    // Without a working radar the SAM no longer counts; the guns cover a
    // release point near them.
    let mut scene = scene.to_vec();
    scene[1] = sam(120, 3., 30_000., false);
    observe(&mut mission, GROUND_REBUILD_TICKS, &scene);
    let threats = mission
        .actor(STRIKER)
        .unwrap()
        .ground_picture()
        .unwrap()
        .threats()
        .clone();
    assert_eq!(threats.level(own, &[target], calm), ThreatLevel::Clear);
    let release: Vector = [0., 3_000., 4. * NMI - 2_000.];
    assert_eq!(
        threats.level(own, &[target, release], calm),
        ThreatLevel::Guns
    );
    // A surface-to-air warning is Missiles; an air-to-air one is Air.
    let warned = RecentWarnings {
        surface_to_air: true,
        ..calm
    };
    assert_eq!(threats.level(own, &[target], warned), ThreatLevel::Missiles);
    let warned = RecentWarnings {
        air_to_air: true,
        ..calm
    };
    assert_eq!(threats.level(own, &[target], warned), ThreatLevel::Air);
    // Our own side's defences are never threats.
    assert_eq!(
        threats.level(Side(2), &[target, release], calm),
        ThreatLevel::Clear
    );

    // A hostile fighter the striker sees within 30 nm is in the picture at the
    // next rebuild; one beyond is not.
    let fighter = |id: u32, ahead_nmi: f64| Observation {
        target: TargetView {
            infrared_strength: 1.,
            id,
            side: Side(2),
            position: [0., 20_000., ahead_nmi * NMI],
            heading_deg: 180.,
            pitch_deg: 0.,
            speed: ScalarSpeed(800.),
            maximum_speed: ScalarSpeed(1_800.),
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
        },
        velocity: [0., 0., -800.],
        source: ObservationSource::Radar,
    };
    mission.actor_mut(STRIKER).unwrap().awareness.observe(
        2 * GROUND_REBUILD_TICKS,
        &[fighter(200, 20.), fighter(201, 40.)],
    );
    observe(&mut mission, 2 * GROUND_REBUILD_TICKS, &scene);
    let threats = mission
        .actor(STRIKER)
        .unwrap()
        .ground_picture()
        .unwrap()
        .threats()
        .clone();
    assert_eq!(threats.fighters.len(), 1);
    let threat = threats.fighters[0];
    assert_eq!(threat.id, 200);
    assert!((threat.distance_ft - (20. * NMI).hypot(15_000.)).abs() < 1e-6);
    assert_eq!(threat.off_nose_deg, 0., "flying straight at the striker");
    assert_eq!(threat.retail_attention(), RetailAttention::Evaded);
    assert_eq!(threats.level(own, &[target], calm), ThreatLevel::Air);
}

#[test]
fn a_sead_strikers_briefed_defences_are_known_before_they_are_seen() {
    let mut mission = striker(StrikeObjective {
        targets: vec![102],
        defenses: vec![122],
        point: Some([0., 10., 50. * NMI]),
    });
    observe(
        &mut mission,
        0,
        &[tank(102, 50., 0.), sam(122, 50., 120_000., true)],
    );
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert!(picture.contacts().is_empty());
    let known = &picture.threats().defenses;
    assert_eq!(known.len(), 1);
    assert!(known[0].briefed);
    // 50 nmi away, its 120,000 ft reach covers a point 40 nmi out.
    let point: Vector = [0., 10_000., 40. * NMI];
    assert_eq!(
        picture
            .threats()
            .level(Side(1), &[point], RecentWarnings::default()),
        ThreatLevel::Missiles
    );
}

#[test]
fn retail_threat_geometry_bands() {
    use RetailAttention::{Evaded, Ignored};
    // Inside 20,000 ft: always, whatever the aspect.
    assert_eq!(retail_attention(false, 20_000., 180.), Evaded);
    assert_eq!(retail_attention(true, 19_999., 180.), Evaded);
    // Between: only within 100 degrees of the threat's nose.
    assert_eq!(retail_attention(false, 100_000., 100.), Evaded);
    assert_eq!(retail_attention(false, 100_000., 100.1), Ignored);
    assert_eq!(retail_attention(false, 158_400., 0.), Evaded);
    // Beyond 30 statute miles, or 40,000 ft for a ship: ignored.
    assert_eq!(retail_attention(false, 158_401., 0.), Ignored);
    assert_eq!(retail_attention(true, 40_000., 0.), Evaded);
    assert_eq!(retail_attention(true, 40_001., 0.), Ignored);
    assert_eq!(retail_attention(false, f64::NAN, 0.), Ignored);
    // The nose angle is horizontal and symmetric.
    let threat = [0., 0., 0.];
    assert_eq!(off_nose_deg(threat, 0., [0., 9_000., 1_000.]), 0.);
    assert!((off_nose_deg(threat, 0., [1_000., 0., 0.]) - 90.).abs() < 1e-9);
    assert!((off_nose_deg(threat, 0., [-1_000., 0., 0.]) - 90.).abs() < 1e-9);
    assert!((off_nose_deg(threat, 270., [1_000., 0., 0.]) - 180.).abs() < 1e-9);
    assert_eq!(off_nose_deg(threat, 45., [0., 5_000., 0.]), 0.);
}

#[test]
fn strike_candidates_feed_the_selector_briefing_first_then_opportunity() {
    let mut mission = striker(StrikeObjective {
        targets: vec![100],
        defenses: Vec::new(),
        point: Some([0., 10., 2. * NMI]),
    });
    let mut scene = vec![
        tank(100, 3., 0.),
        tank(101, 2., 0.),
        tank(102, 2.5, 30_000.),
    ];
    let mut far = tank(103, 2., 70_000.);
    far.object.position[2] = 2. * NMI;
    scene.push(far);
    observe(&mut mission, 0, &scene);
    let pick = |mission: &AiMission, attackers: &dyn Fn(u32) -> u32| {
        let actor = mission.actor(STRIKER).unwrap();
        let picture = actor.ground_picture().unwrap();
        let candidates =
            picture.strike_candidates(actor.flight().position, Side(1), attackers, &|_| true);
        targeting::select_strike_target(ObjectId(STRIKER), Side(1), &candidates)
            .map(|c| (c.id.0, c.source))
    };
    assert_eq!(pick(&mission, &|_| 0), Some((100, StrikeSource::Briefed)));
    // Two wingmen on the briefed target: this striker waits.
    assert_eq!(pick(&mission, &|id| u32::from(id == 100) * 2), None);
    // The briefed target destroyed: the nearest seen hostile within 10 nm of
    // the objective (103 is 11.5 nmi east of it).
    scene[0].object.destroyed = true;
    observe(&mut mission, GROUND_REBUILD_TICKS, &scene);
    assert_eq!(
        pick(&mission, &|_| 0),
        Some((101, StrikeSource::Opportunity))
    );
    // An order outranks everything.
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .set_strike_order(Some(103));
    assert!(mission.ground_due(), "an order rebuilds");
    observe(&mut mission, GROUND_REBUILD_TICKS + 1, &scene);
    assert_eq!(pick(&mission, &|_| 3), Some((103, StrikeSource::Ordered)));
}

#[test]
fn cap_actors_have_no_picture_and_fly_exactly_as_before() {
    let mut cap = actor(STRIKER, 1);
    assert_eq!(cap.strike_role(), StrikeRole::Cap);
    assert!(cap.ground_picture().is_none());
    cap.set_strike_role(StrikeRole::Cap);
    assert!(cap.ground_picture().is_none());
    assert!(!cap.set_strike_objective(briefed_on(vec![1])));
    // Strike then CAP drops the picture; an order alone keeps one while it
    // stands, without the Strike role.
    cap.set_strike_role(StrikeRole::Strike);
    assert!(cap.ground_picture().is_some());
    cap.set_strike_role(StrikeRole::Cap);
    assert!(cap.ground_picture().is_none());
    cap.set_strike_order(Some(7));
    assert_eq!(cap.strike_role(), StrikeRole::Cap);
    assert_eq!(cap.strike_order(), Some(7));
    assert!(!cap.needs_briefing());
    cap.set_strike_order(None);
    assert!(cap.ground_picture().is_none());

    // Two CAP fights, one host describing a defended ground target every
    // tick and one not, fly tick for tick alike.
    let fight = || {
        let mut mission = AiMission::new();
        mission.push(actor(1, 1));
        let mut enemy = AiActor::new(setup(
            2,
            2,
            0,
            [0., ALTITUDE, 12. * NMI],
            std::f64::consts::PI,
        ))
        .unwrap();
        enemy.flight_mut().pitch = 0.;
        mission.push(enemy);
        mission
    };
    let scene = [
        tank(100, 2., 0.),
        sam(120, 3., 30_000., true),
        aaa(121, 4., 6_000.),
    ];
    let (mut plain, mut described) = (fight(), fight());
    for tick in 0..600 {
        let world = |mission: &AiMission| -> Vec<WorldObject> {
            mission
                .actors()
                .iter()
                .map(|a| object(a, a.identity().side.0))
                .collect()
        };
        assert!(!described.ground_due());
        described.observe_ground(
            GroundScene {
                painted: &[],
                launched: &[],
                subjects: &scene,
                weather: &Weather::default(),
            },
            &flat,
        );
        let a = plain.step(&world(&plain), &flat, TimeOfDay(tick)).unwrap();
        let b = described
            .step(&world(&described), &flat, TimeOfDay(tick))
            .unwrap();
        assert_eq!(a, b, "tick {tick}");
        for (x, y) in plain.actors().iter().zip(described.actors()) {
            assert_eq!(x.flight(), y.flight(), "tick {tick}");
            assert!(y.ground_picture().is_none());
        }
    }
    assert_eq!(coded(&plain), coded(&described));
}

fn models_of(mission: &AiMission) -> Models {
    let mut models = Models::default();
    for actor in mission.actors() {
        models
            .insert(actor.identity().aircraft, actor.flight().import_model())
            .unwrap();
    }
    models
}

fn coded(mission: &AiMission) -> crate::checkpoint::Coded {
    to_bytes(mission, &models_of(mission)).unwrap()
}

#[test]
fn a_ground_picture_round_trips_and_refreshes_on_alike() {
    let mut mission = striker(StrikeObjective {
        targets: vec![100],
        defenses: vec![120],
        point: Some([0., 10., 3. * NMI]),
    });
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .set_strike_order(Some(101));
    let mut scene = vec![
        tank(100, 3., 0.),
        tank(101, 2., 0.),
        sam(120, 3., 30_000., true),
        aaa(121, 4., 6_000.),
    ];
    observe(&mut mission, 0, &scene);
    observe(&mut mission, GROUND_FOLLOW_TICKS, &scene);
    let mut twin: AiMission = from_bytes(&coded(&mission), &models_of(&mission)).unwrap();
    assert_eq!(coded(&twin), coded(&mission));
    assert_eq!(
        twin.actor(STRIKER).unwrap().ground_picture(),
        mission.actor(STRIKER).unwrap().ground_picture()
    );
    assert!(
        twin.actor(STRIKER)
            .unwrap()
            .ground_picture()
            .unwrap()
            .contact(101)
            .is_some()
    );
    for tick in [2 * GROUND_FOLLOW_TICKS, GROUND_REBUILD_TICKS] {
        scene[1].object.position[0] += 250.;
        observe(&mut mission, tick, &scene);
        observe(&mut twin, tick, &scene);
        assert_eq!(coded(&twin), coded(&mission), "tick {tick}");
    }
}

// -- a flight lead's ground orders (slice ST-O) --------------------------------

/// A CAP actor over the origin with no briefing: only an order gives it a
/// picture.
fn wingman_of_the_lead() -> AiMission {
    mission_with(actor(STRIKER, 1))
}

fn candidates_of(mission: &AiMission) -> Vec<(u32, StrikeSource)> {
    let actor = mission.actor(STRIKER).unwrap();
    let picture = actor.ground_picture().unwrap();
    let candidates = picture.strike_candidates(actor.flight().position, Side(1), &|_| 0, &|_| true);
    let mut ranked = Vec::new();
    let mut rest = candidates.clone();
    while let Some(best) = targeting::select_strike_target(ObjectId(STRIKER), Side(1), &rest) {
        ranked.push((best.id.0, best.source));
        let id = best.id;
        rest.retain(|c| c.id != id);
    }
    ranked
}

#[test]
fn a_class_order_briefs_its_targets_and_takes_no_opportunities() {
    let mut mission = wingman_of_the_lead();
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .order_strike_class(vec![102, 100, 100]);
    let actor = mission.actor(STRIKER).unwrap();
    assert!(actor.has_ground_order() && actor.strikes());
    assert_eq!(actor.strike_role(), StrikeRole::Cap);
    assert_eq!(actor.strike_order(), None);
    assert_eq!(
        actor.ground_picture().unwrap().class_order(),
        [100, 102],
        "ascending, once"
    );
    // A tank (101) the class order does not name stands near the objects.
    let scene = [tank(100, 4., 0.), tank(101, 2., 0.), tank(102, 3., 0.)];
    observe(&mut mission, 0, &scene);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert!(
        picture
            .briefed()
            .iter()
            .all(|b| b.kind == crate::ai::awareness::ground::BriefedKind::Target)
    );
    assert_eq!(picture.briefed().len(), 2);
    // Nearest first, and the unnamed tank is never an opportunity.
    assert_eq!(
        candidates_of(&mission),
        [(102, StrikeSource::Briefed), (100, StrikeSource::Briefed)]
    );
}

#[test]
fn ground_orders_replace_each_other_and_clear() {
    let mut mission = wingman_of_the_lead();
    fn actor(mission: &mut AiMission) -> &mut AiActor {
        mission.actor_mut(STRIKER).unwrap()
    }
    actor(&mut mission).order_strike(100);
    assert_eq!(mission.actor(STRIKER).unwrap().strike_order(), Some(100));
    actor(&mut mission).order_strike_class(vec![101, 102]);
    let a = mission.actor(STRIKER).unwrap();
    assert_eq!(a.strike_order(), None, "a class replaces one object");
    assert_eq!(a.ground_picture().unwrap().class_order(), [101, 102]);
    actor(&mut mission).order_strike(103);
    let a = mission.actor(STRIKER).unwrap();
    assert_eq!(a.strike_order(), Some(103));
    assert!(a.ground_picture().unwrap().class_order().is_empty());
    // Lasing replaces them, and clears them.
    actor(&mut mission).order_lase(104);
    let a = mission.actor(STRIKER).unwrap();
    assert_eq!(a.lase_order(), Some(104));
    assert!(a.ground_picture().is_none() && !a.strikes());
    assert!(a.has_ground_order());
    actor(&mut mission).order_strike(100);
    assert_eq!(mission.actor(STRIKER).unwrap().lase_order(), None);
    actor(&mut mission).clear_ground_orders();
    let a = mission.actor(STRIKER).unwrap();
    assert!(!a.has_ground_order() && a.ground_picture().is_none());
    // An empty class order is no order.
    actor(&mut mission).order_strike_class(Vec::new());
    assert!(!mission.actor(STRIKER).unwrap().has_ground_order());
}

#[test]
fn clearing_a_lead_s_orders_leaves_a_missions_strike_role_and_briefing() {
    let mut mission = striker(briefed_on(vec![100]));
    let actor = mission.actor_mut(STRIKER).unwrap();
    actor.order_strike(101);
    actor.clear_ground_orders();
    let actor = mission.actor(STRIKER).unwrap();
    assert_eq!(actor.strike_role(), StrikeRole::Strike);
    assert!(actor.ground_picture().unwrap().objective().is_some());
    assert_eq!(actor.strike_order(), None);
}

#[test]
fn an_actor_that_cannot_take_an_order_says_why() {
    let mut cap = actor(STRIKER, 1);
    assert_eq!(cap.ground_order_refusal(), None);
    cap.set_dummy();
    assert_eq!(
        cap.ground_order_refusal(),
        Some(crate::ai::wing::RejectReason::Dummy)
    );
}

#[test]
fn class_and_lase_orders_round_trip() {
    let mut mission = wingman_of_the_lead();
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .order_strike_class(vec![100, 101]);
    observe(&mut mission, 0, &[tank(100, 3., 0.), tank(101, 2., 0.)]);
    let twin: AiMission = from_bytes(&coded(&mission), &models_of(&mission)).unwrap();
    assert_eq!(coded(&twin), coded(&mission));
    assert_eq!(
        twin.actor(STRIKER)
            .unwrap()
            .ground_picture()
            .unwrap()
            .class_order(),
        [100, 101]
    );
    let mut lasing = wingman_of_the_lead();
    lasing.actor_mut(STRIKER).unwrap().order_lase(7);
    let twin: AiMission = from_bytes(&coded(&lasing), &models_of(&lasing)).unwrap();
    assert_eq!(twin.actor(STRIKER).unwrap().lase_order(), Some(7));
}

/// One bomb store for a striker's inputs (slice ST-T2).
fn strike_inputs() -> crate::ai::strike::Inputs {
    use crate::ai::strike::{Inputs, Store, StoreKind};
    Inputs {
        class: crate::models::strike_class::StrikeClass::Attack,
        stores: vec![Store {
            station: crate::ai::weapon_service::StationId(1),
            kind: StoreKind::Bomb,
            rounds: 4,
            cadence_ticks: 30,
            fire_range_ft: 10_000.,
            minimum_range_ft: 0.,
            maximum_range_ft: 20_000.,
            round_speed_fps: None,
            release_speed_share: 1.,
            bomb_body: Default::default(),
            fuze_arm_s: 1.,
            frag_radius_ft: 300.,
            falls_free: false,
            flight: None,
        }],
        designator: false,
        mission_height_ft: None,
        solution: None,
        lase: crate::ai::strike::LaseInputs::default(),
        shots: Default::default(),
    }
}

#[test]
fn cap_actors_never_fly_a_strike_handling_and_take_no_strike_inputs() {
    // Slice ST-T2: the roll and pull and the planned dive floor reach only a
    // striker's attack steers. Two CAP fights, one host handing strike
    // inputs to every actor every tick and one not, fly tick for tick alike,
    // and every actor's adapter stays plain.
    let fight = || {
        let mut mission = AiMission::new();
        mission.push(actor(1, 1));
        let mut enemy = AiActor::new(setup(
            2,
            2,
            0,
            [0., ALTITUDE, 12. * NMI],
            std::f64::consts::PI,
        ))
        .unwrap();
        enemy.flight_mut().pitch = 0.;
        mission.push(enemy);
        mission
    };
    let (mut plain, mut handed) = (fight(), fight());
    for tick in 0..600 {
        let world = |mission: &AiMission| -> Vec<WorldObject> {
            mission
                .actors()
                .iter()
                .map(|a| object(a, a.identity().side.0))
                .collect()
        };
        for actor in handed.actors.iter_mut() {
            actor.set_strike_inputs(strike_inputs());
        }
        let a = plain.step(&world(&plain), &flat, TimeOfDay(tick)).unwrap();
        let b = handed
            .step(&world(&handed), &flat, TimeOfDay(tick))
            .unwrap();
        assert_eq!(a, b, "tick {tick}");
        for (x, y) in plain.actors().iter().zip(handed.actors()) {
            assert_eq!(x.flight(), y.flight(), "tick {tick}");
            assert!(y.strike().is_none());
            assert_eq!(
                y.adapter.strike_handling(),
                crate::ai::strike::Handling::PLAIN
            );
        }
    }
    assert_eq!(coded(&plain), coded(&handed));
}

#[test]
fn a_striker_under_weapons_hold_flies_no_pass_until_the_hold_lifts() {
    // Slice ST-T2: the hold preset (or a hold fire order) holds the passes
    // too, not only the release; the strike starts from its ingress when the
    // hold lifts.
    let mut mission = mission_with(actor(STRIKER, 1));
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .set_strike_order(Some(100));
    let assignment = |stance| crate::ai::engagement::Assignment {
        stance,
        ..mission.actor(STRIKER).unwrap().assignment().clone()
    };
    let held = assignment(crate::ai::engagement::Stance::WeaponsHold);
    let free = mission.actor(STRIKER).unwrap().assignment().clone();
    mission.actor_mut(STRIKER).unwrap().set_assignment(held);
    let scene = [tank(100, 3., 0.)];
    let world = |mission: &AiMission| -> Vec<WorldObject> {
        mission.actors().iter().map(|a| object(a, 1)).collect()
    };
    for tick in 0..240 {
        observe(&mut mission, tick, &scene);
        mission
            .actor_mut(STRIKER)
            .unwrap()
            .set_strike_inputs(strike_inputs());
        mission
            .step(&world(&mission), &flat, TimeOfDay(tick))
            .unwrap();
        let striker = mission.actor(STRIKER).unwrap();
        assert!(
            striker
                .strike()
                .is_none_or(|s| s.phase() == crate::ai::strike::Phase::Ingress)
        );
        assert_eq!(
            striker.adapter.strike_handling(),
            crate::ai::strike::Handling::PLAIN
        );
    }
    mission.actor_mut(STRIKER).unwrap().set_assignment(free);
    let mut attacked = false;
    for tick in 240..480 {
        observe(&mut mission, tick, &scene);
        mission
            .actor_mut(STRIKER)
            .unwrap()
            .set_strike_inputs(strike_inputs());
        mission
            .step(&world(&mission), &flat, TimeOfDay(tick))
            .unwrap();
        attacked |= mission
            .actor(STRIKER)
            .unwrap()
            .strike()
            .is_some_and(|s| s.phase() == crate::ai::strike::Phase::Attack);
    }
    assert!(attacked, "the strike goes on once the hold lifts");
}

// -- hidden shoulder-fired teams (T.O.R.E opinionated, John 2026-10-29) --------

/// A side 2 shoulder-fired SAM team `ahead_ft` north on flat ground: class
/// SAM, listed, `maxVisDist` 78, an infrared missile with no radar, hidden.
fn team(id: u32, ahead_ft: f64) -> GroundSubject {
    let mut subject = sam(id, ahead_ft / NMI, 8_100., true);
    subject.object.emitting = false;
    subject.defense = subject.defense.map(|facts| DefenseFacts {
        rotor_reach_ft: 16_300.,
        radar: false,
        ..facts
    });
    subject.hidden = true;
    subject
}

/// A striker 300 ft over the origin (a helicopter's height), so slant and
/// ground distances nearly agree.
fn low_striker() -> AiMission {
    let mut mission = striker(briefed_on(vec![]));
    mission.actor_mut(STRIKER).unwrap().flight_mut().position = [0., 310., 0.];
    mission
}

fn observe_scene(
    mission: &mut AiMission,
    tick: u64,
    subjects: &[GroundSubject],
    launched: &[(u32, u32)],
    weather: &Weather,
) {
    mission.tick = tick;
    mission.observe_ground(
        GroundScene {
            painted: &[],
            launched,
            subjects,
            weather,
        },
        &flat,
    );
}

fn known(mission: &AiMission) -> Vec<u32> {
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    picture.threats().defenses.iter().map(|d| d.id).collect()
}

fn spot_ft(mission: &AiMission) -> f64 {
    team_spot_ft(mission.actor(STRIKER).unwrap().experience().level, 100.)
}

#[test]
fn the_spotting_distance_is_short_and_grows_with_skill() {
    for pair in TEAM_SPOT_FT.windows(2) {
        assert!(pair[0] < pair[1]);
    }
    // Every level spots far nearer than the 19,968 ft the eye lists every
    // other site from, and haze shortens it.
    assert!(TEAM_SPOT_FT.iter().all(|ft| *ft <= 78. * 256. / 4.));
    assert_eq!(team_spot_ft(crate::ai::Experience::Average, 100.), 3_000.);
    assert_eq!(team_spot_ft(crate::ai::Experience::Average, 50.), 1_500.);
}

#[test]
fn a_hidden_team_is_unlisted_beyond_its_spotting_distance() {
    let mut mission = low_striker();
    let spot = spot_ft(&mission);
    // Just past the spotting distance, well inside every list's reach: the
    // radar, the FLIR and the eye would all list a vehicle there.
    let scene = [team(130, spot + 400.), tank(102, (spot + 400.) / NMI, 300.)];
    for tick in [0, GROUND_FOLLOW_TICKS, GROUND_REBUILD_TICKS] {
        observe_scene(&mut mission, tick, &scene, &[], &Weather::default());
    }
    assert_eq!(contacts(&mission), [102]);
    assert!(known(&mission).is_empty(), "off the threat picture");
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert!(picture.seen().iter().all(|s| s.id != 130));
}

#[test]
fn a_hidden_team_is_listed_within_its_spotting_distance_by_the_eye_alone() {
    let mut mission = low_striker();
    let spot = spot_ft(&mission);
    let scene = [team(130, spot - 400.)];
    observe_scene(&mut mission, 0, &scene, &[], &Weather::default());
    assert_eq!(contacts(&mission), [130]);
    assert_eq!(known(&mission), [130]);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    let track = picture.contact(130).unwrap();
    assert!(track.channels.visual && !track.channels.radar && !track.channels.infrared);
    assert_eq!(picture.unmasked_at(130), None, "spotted, not unmasked");
    // Once spotted it stays known when the striker flies off.
    mission.actor_mut(STRIKER).unwrap().flight_mut().position = [0., 310., -2. * NMI];
    observe_scene(
        &mut mission,
        GROUND_REBUILD_TICKS,
        &scene,
        &[],
        &Weather::default(),
    );
    assert!(contacts(&mission).is_empty());
    assert_eq!(known(&mission), [130]);

    // Haze halving the eye's reach hides it at the same place.
    let mut hazy = low_striker();
    let weather = Weather {
        bands: vec![sensors::WeatherBand {
            low_ft: 0.,
            high_ft: 10_000.,
            effects: [50, 100, 100, 100, 100],
        }],
    };
    observe_scene(&mut hazy, 0, &scene, &[], &weather);
    assert!(contacts(&hazy).is_empty() && known(&hazy).is_empty());
}

#[test]
fn a_hidden_team_is_known_right_after_it_launches() {
    let mut mission = low_striker();
    let scene = [team(130, 10_900.)];
    observe_scene(&mut mission, 0, &scene, &[], &Weather::default());
    assert!(known(&mission).is_empty());
    // It launches at the striker: known at the next quarter-second refresh,
    // not at the next rebuild.
    let at = GROUND_FOLLOW_TICKS;
    observe_scene(
        &mut mission,
        at,
        &scene,
        &[(STRIKER, 130)],
        &Weather::default(),
    );
    assert_eq!(known(&mission), [130]);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert_eq!(picture.unmasked_at(130), Some(at));
    assert!(picture.contacts().is_empty(), "known, not on a list");
    // The rings: known at once for planning; for the ambush rule the shot
    // that gave it away still came from a site it did not know, and a later
    // one comes from a known site.
    use crate::ai::helicopter_threat::{UNMASK_SHOT_TICKS, known_rings, rings_before_shot};
    assert!(known_rings(picture, Side(1)).iter().any(|r| r.id == 130));
    assert!(rings_before_shot(picture, Side(1), at + 200).is_empty());
    let later = at + UNMASK_SHOT_TICKS + 1;
    assert!(
        rings_before_shot(picture, Side(1), later)
            .iter()
            .any(|r| r.id == 130)
    );
    // It stays known after the round is gone, and the rebuild keeps it.
    observe_scene(
        &mut mission,
        GROUND_REBUILD_TICKS,
        &scene,
        &[],
        &Weather::default(),
    );
    assert_eq!(known(&mission), [130]);
    // A second launch keeps the first unmasking tick.
    observe_scene(
        &mut mission,
        GROUND_REBUILD_TICKS + GROUND_FOLLOW_TICKS,
        &scene,
        &[(STRIKER, 130)],
        &Weather::default(),
    );
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert_eq!(picture.unmasked_at(130), Some(at));

    // Destroyed, it leaves the picture and its unmasking with it.
    let mut wreck = scene.to_vec();
    wreck[0].object.destroyed = true;
    observe_scene(
        &mut mission,
        2 * GROUND_REBUILD_TICKS,
        &wreck,
        &[],
        &Weather::default(),
    );
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert!(known(&mission).is_empty() && picture.unmasked_at(130).is_none());
}

#[test]
fn a_launch_at_a_friendly_in_its_picture_gives_a_team_away() {
    let friend = |id: u32| Observation {
        target: TargetView {
            infrared_strength: 1.,
            id,
            side: Side(1),
            position: [2_000., 300., 8_000.],
            heading_deg: 0.,
            pitch_deg: 0.,
            speed: ScalarSpeed(200.),
            maximum_speed: ScalarSpeed(300.),
            is_aircraft: true,
            is_fighter: false,
            human_controlled: false,
            valid: true,
            type_allowed: true,
            seeker_eligible: true,
            wing_attackers: 0,
            terrain_blocked: false,
            sensor_supported: true,
            link_track: false,
        },
        velocity: [0., 0., 200.],
        source: ObservationSource::Visual,
    };
    let scene = [team(130, 10_900.), team(131, 30_000.)];
    let mut mission = low_striker();
    mission
        .actor_mut(STRIKER)
        .unwrap()
        .awareness
        .observe(0, &[friend(7)]);
    // At a friendly it sees, from within its own listing distance: known.
    // At an aircraft it does not hold: nothing. From beyond the listing
    // distance (30,000 ft): nothing.
    observe_scene(
        &mut mission,
        0,
        &scene,
        &[(7, 130), (7, 131), (8, 130)],
        &Weather::default(),
    );
    assert_eq!(known(&mission), [130]);
    let mut mission = low_striker();
    observe_scene(&mut mission, 0, &scene, &[(8, 130)], &Weather::default());
    assert!(known(&mission).is_empty());
}

#[test]
fn a_vehicle_sam_still_lists_at_the_old_distance() {
    let mut mission = low_striker();
    // An SA-9-like vehicle 18,000 ft out, inside its 19,968 ft listing
    // distance: listed by the eye (and the radar) at the first rebuild. A
    // launch from it unmasks nothing, as it was known.
    let mut vehicle = sam(140, 18_000. / NMI, 16_200., true);
    vehicle.object.emitting = false;
    observe_scene(
        &mut mission,
        0,
        &[vehicle.clone()],
        &[],
        &Weather::default(),
    );
    assert_eq!(contacts(&mission), [140]);
    assert_eq!(known(&mission), [140]);
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert!(picture.contact(140).unwrap().channels.visual);
    observe_scene(
        &mut mission,
        GROUND_FOLLOW_TICKS,
        &[vehicle],
        &[(STRIKER, 140)],
        &Weather::default(),
    );
    let picture = mission.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert_eq!(picture.unmasked_at(140), None);
}

#[test]
fn an_unmasked_team_round_trips() {
    let mut mission = low_striker();
    let scene = [team(130, 10_900.)];
    observe_scene(
        &mut mission,
        0,
        &scene,
        &[(STRIKER, 130)],
        &Weather::default(),
    );
    let twin: AiMission = from_bytes(&coded(&mission), &models_of(&mission)).unwrap();
    assert_eq!(coded(&twin), coded(&mission));
    let picture = twin.actor(STRIKER).unwrap().ground_picture().unwrap();
    assert_eq!(picture.unmasked_at(130), Some(0));
}
