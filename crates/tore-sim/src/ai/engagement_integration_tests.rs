use super::tests::{enable_test_radar, flat, object, perception_actor, setup, visible_object};
use super::*;
use crate::ai::{
    Experience,
    targeting::Side,
    wing::{Formation, TargetId, TargetOrder, WingRequest},
};

fn assignment(role: engagement::Role, stance: engagement::Stance) -> engagement::Assignment {
    engagement::Assignment {
        role,
        stance,
        protected_ids: Vec::new(),
        destroy_ids: Vec::new(),
        hostile_escorts: Vec::new(),
        patrol: None,
    }
}

fn world_with(
    mission: &AiMission,
    extras: impl IntoIterator<Item = WorldObject>,
) -> Vec<WorldObject> {
    mission
        .actors()
        .iter()
        .map(|actor| object(actor, actor.identity.side.0))
        .chain(extras)
        .collect()
}

fn charge(id: u32, position: [f64; 3]) -> WorldObject {
    let actor = AiActor::new(setup(id, 1, 0, position, 0.)).unwrap();
    let mut object = object(&actor, 1);
    object.human_controlled = true;
    object
}

fn radar_escort() -> AiActor {
    let mut actor = perception_actor(Experience::Ace);
    enable_test_radar(&mut actor);
    let mut duty = assignment(
        engagement::Role::Escort,
        engagement::Stance::ProtectAssigned,
    );
    duty.protected_ids.push(10);
    actor.set_assignment(duty);
    actor
}

fn neutral_wing_mission() -> AiMission {
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
    mission
}

#[test]
fn neutral_ai_wings_release_on_mission_permitted_contact_and_keep_objectives() {
    let mut mission = neutral_wing_mission();
    let mut intercept = assignment(
        engagement::Role::Intercept,
        engagement::Stance::EngageAssigned,
    );
    intercept.destroy_ids.push(3);
    mission
        .actor_mut(1)
        .unwrap()
        .set_assignment(intercept.clone());
    mission.start_in_formation();

    assert!(mission.actors().iter().all(AiActor::is_neutral));
    let world = world_with(&mission, []);
    let output = mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert!(output.launches.is_empty());
    assert!(mission.actors().iter().all(|a| !a.is_neutral()));
    // Orders are consumed after the decisions, so contact starts pursuit next tick.
    mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(1))
        .unwrap();
    assert_eq!(mission.actor(1).unwrap().controller().target(), Some(3));
    assert_eq!(mission.actor(1).unwrap().assignment(), &intercept);
    assert!(mission.actor(1).unwrap().awareness().snapshot(3).is_some());
}

#[test]
fn directed_attack_order_releases_only_the_recipient() {
    let mut mission = neutral_wing_mission();
    // Written for observation every tick.
    mission.set_test_settings(AiTestSettings::every_tick());
    let outcomes = mission
        .order_wing_report(
            Side(1),
            0,
            None,
            Some(2),
            WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(TargetId(3))),
        )
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].0, 2);
    assert!(!mission.actor(2).unwrap().is_neutral());
    assert!(mission.actor(1).unwrap().is_neutral());
    assert!(mission.actor(3).unwrap().is_neutral());
    assert!(mission.actor(4).unwrap().is_neutral());

    let world = world_with(&mission, []);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(2).unwrap().controller().target(), Some(3));
    assert_eq!(mission.actor(1).unwrap().controller().target(), None);
}

#[test]
fn formation_recall_cancels_combat_without_erasing_objective_or_memory() {
    let mut mission = AiMission::new();
    // Written for observation every tick.
    mission.set_test_settings(AiTestSettings::every_tick());
    let mut actor = perception_actor(Experience::Ace);
    enable_test_radar(&mut actor);
    let mut intercept = assignment(
        engagement::Role::Intercept,
        engagement::Stance::EngageAssigned,
    );
    intercept.destroy_ids.push(2);
    actor.set_assignment(intercept.clone());
    mission.push(actor);
    let seen = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., 1_000.]);
    mission
        .step(&world_with(&mission, [seen]), &flat, TimeOfDay(0))
        .unwrap();
    assert_eq!(mission.actor(1).unwrap().controller().target(), Some(2));

    let hidden = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., -40_000.]);
    mission
        .step(&world_with(&mission, [hidden.clone()]), &flat, TimeOfDay(1))
        .unwrap();
    assert_eq!(mission.actor(1).unwrap().search_target, Some(2));
    mission
        .order(1, WingRequest::FormationSelection(Formation::LineAstern))
        .unwrap()
        .unwrap();
    let actor = mission.actor(1).unwrap();
    assert!(actor.is_neutral());
    assert_eq!(actor.assignment(), &intercept);
    assert_eq!(actor.controller().target(), None);
    assert_eq!(actor.search_target, None);
    assert_eq!(actor.sensors().unwrap().selected(), None);
    assert!(actor.awareness().snapshot(2).is_some());

    let output = mission
        .step(&world_with(&mission, [hidden]), &flat, TimeOfDay(2))
        .unwrap();
    assert!(output.launches.is_empty());
    assert_eq!(mission.actor(1).unwrap().controller().target(), None);
    assert_eq!(mission.actor(1).unwrap().search_target, None);
}

#[test]
fn ai_leader_releases_its_wing_on_a_fresh_attack_but_not_another_wing() {
    let mut mission = neutral_wing_mission();
    let mut other_wing = setup(5, 1, 0, [2_000., 20_000., 0.], 0.);
    other_wing.identity.wing = 1;
    mission.push(AiActor::new(other_wing).unwrap());
    mission.start_in_formation();
    for id in [3, 4, 5] {
        mission.actor_mut(id).unwrap().set_assignment(assignment(
            engagement::Role::Disengage,
            engagement::Stance::SelfDefense,
        ));
    }
    mission.report_attack_evidence(
        2,
        engagement::ThreatReport {
            attacker_id: Some(3),
            defended_id: 2,
        },
        None,
        Some(90),
    );

    let output = mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(0))
        .unwrap();
    assert!(output.wing.iter().any(|(sender, request)| {
        *sender == 1 && *request == WingRequest::TargetAssignment(TargetOrder::FreeSelection)
    }));
    assert!(!mission.actor(1).unwrap().is_neutral());
    assert!(!mission.actor(2).unwrap().is_neutral());
    for id in [3, 4, 5] {
        assert!(
            mission.actor(id).unwrap().is_neutral(),
            "other wing actor {id} released"
        );
    }
    // AI leader commands are delivered after all aircraft decide this tick.
    assert_eq!(mission.actor(2).unwrap().controller().target(), None);
    mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(1))
        .unwrap();
    assert_eq!(mission.actor(2).unwrap().controller().target(), Some(3));
}

#[test]
fn old_attack_id_cannot_undo_recall_but_new_attack_can_release_ai_wing() {
    let mut mission = neutral_wing_mission();
    let report = engagement::ThreatReport {
        attacker_id: Some(3),
        defended_id: 2,
    };
    mission.report_attack_evidence(2, report, None, Some(90));
    mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(0))
        .unwrap();
    assert!(!mission.actor(1).unwrap().is_neutral());

    mission
        .order_wing(
            Side(1),
            0,
            None,
            WingRequest::FormationSelection(Formation::LineAstern),
        )
        .unwrap();
    assert!(mission.actor(1).unwrap().is_neutral());
    assert!(mission.actor(2).unwrap().is_neutral());
    mission.report_attack_evidence(2, report, None, Some(90));
    let output = mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(1))
        .unwrap();
    assert!(!output.wing.iter().any(|(sender, request)| {
        *sender == 1 && *request == WingRequest::TargetAssignment(TargetOrder::FreeSelection)
    }));
    assert!(mission.actor(1).unwrap().is_neutral());
    assert!(mission.actor(2).unwrap().is_neutral());
    assert_eq!(mission.actor(2).unwrap().controller().target(), None);

    mission.report_attack_evidence(2, report, None, Some(91));
    let output = mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(2))
        .unwrap();
    assert!(output.wing.iter().any(|(sender, request)| {
        *sender == 1 && *request == WingRequest::TargetAssignment(TargetOrder::FreeSelection)
    }));
    assert!(!mission.actor(1).unwrap().is_neutral());
    assert!(!mission.actor(2).unwrap().is_neutral());
}

#[test]
fn recall_ignores_a_missile_report_waiting_for_the_leader() {
    use crate::combat::missiles::Guidance;

    let missile = |id| MissileSnapshot {
        id,
        owner: 3,
        position: [0., 20_000., -1_500.],
        velocity: [0., 0., 2_000.],
        guidance: Guidance::Supported,
        target: Some(2),
        radar_active: false,
        radar_acquired: false,
        supported: true,
        supporting_radar_position: Some([5_000., 20_000., 0.]),
        alive: true,
    };
    let mut mission = neutral_wing_mission();
    for actor in mission.actors_mut() {
        actor.set_assignment(assignment(
            engagement::Role::Disengage,
            engagement::Stance::SelfDefense,
        ));
    }
    mission.set_missiles(vec![missile(90)]);
    let first = mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(0))
        .unwrap();
    assert!(first.wing.is_empty());
    assert!(mission.actor(1).unwrap().perceived_attacks().is_empty());
    assert!(mission.actor(2).unwrap().defense_decision().is_some());

    // The wingman's warning has been queued for the leader's next tick.
    mission
        .order_wing(
            Side(1),
            0,
            None,
            WingRequest::FormationSelection(Formation::LineAstern),
        )
        .unwrap();
    for tick in 1..=2 {
        let output = mission
            .step(&world_with(&mission, []), &flat, TimeOfDay(tick))
            .unwrap();
        assert!(
            !output.wing.iter().any(|(sender, request)| {
                *sender == 1
                    && *request == WingRequest::TargetAssignment(TargetOrder::FreeSelection)
            }),
            "old missile released the wing at tick {tick}"
        );
        assert!(mission.actor(1).unwrap().is_neutral());
        assert!(mission.actor(2).unwrap().is_neutral());
        assert_eq!(mission.actor(1).unwrap().controller().target(), None);
    }

    mission.set_missiles(vec![missile(91)]);
    mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(3))
        .unwrap();
    let output = mission
        .step(&world_with(&mission, []), &flat, TimeOfDay(4))
        .unwrap();
    assert!(output.wing.iter().any(|(sender, request)| {
        *sender == 1 && *request == WingRequest::TargetAssignment(TargetOrder::FreeSelection)
    }));
    assert!(!mission.actor(1).unwrap().is_neutral());
    assert!(!mission.actor(2).unwrap().is_neutral());
}

#[test]
fn human_led_wing_stays_neutral_after_an_attack_until_ordered() {
    let mut mission = AiMission::new();
    mission.push(AiActor::new(setup(2, 1, 1, [0., 20_000., -500.], 0.)).unwrap());
    mission.set_external_leader(Side(1), 0, 10);
    mission.start_in_formation();
    mission.report_attack_evidence(
        10,
        engagement::ThreatReport {
            attacker_id: Some(3),
            defended_id: 10,
        },
        None,
        Some(90),
    );
    let hostile = visible_object(mission.actor(2).unwrap(), 3, [0., 20_000., 1_000.]);
    let world = world_with(&mission, [charge(10, [0., 20_000., 0.]), hostile]);
    let output = mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert!(output.wing.is_empty());
    assert!(output.launches.is_empty());
    assert!(mission.actor(2).unwrap().is_neutral());
    assert_eq!(mission.actor(2).unwrap().controller().target(), None);

    mission
        .order_wing(
            Side(1),
            0,
            None,
            WingRequest::TargetAssignment(TargetOrder::FreeSelection),
        )
        .unwrap();
    assert!(!mission.actor(2).unwrap().is_neutral());
}

#[test]
fn escort_acquires_and_engages_a_radar_detected_threat_beyond_visual_range() {
    let mut mission = AiMission::new();
    let mut escort = radar_escort();
    escort.stations[0].requires_radar = true;
    escort.stations[0].requires_sensor = true;
    mission.push(escort);
    let mut hostile = visible_object(
        mission.actor(1).unwrap(),
        2,
        [0., 20_000., 5.5 * sensors::FEET_PER_NAUTICAL_MILE],
    );
    hostile.velocity = [0., 0., -500.];
    hostile.observable.as_mut().unwrap().velocity = hostile.velocity;
    let charge = charge(10, [0., 20_000., 1_000.]);

    let first = world_with(&mission, [charge.clone(), hostile.clone()]);
    let first_output = mission.step(&first, &flat, TimeOfDay(0)).unwrap();
    assert!(first_output.launches.is_empty());
    let actor = mission.actor(1).unwrap();
    assert_eq!(
        actor.sensors().unwrap().contact(2).unwrap().channel,
        sensors::Channel::Radar
    );
    assert_eq!(actor.controller().target(), Some(2));
    assert_eq!(actor.sensors().unwrap().selected(), Some(2));
    assert_ne!(actor.activity(), Activity::Formation);

    let mut launched = false;
    for tick in 1..7_200 {
        let world = world_with(&mission, [charge.clone(), hostile.clone()]);
        let output = mission.step(&world, &flat, TimeOfDay(tick)).unwrap();
        if output
            .launches
            .iter()
            .any(|launch| launch.actor == 1 && launch.station == StationId(0))
        {
            assert!(mission.actor(1).unwrap().sensors().unwrap().supports(2));
            launched = true;
            break;
        }
    }
    assert!(
        launched,
        "escort never launched at its radar detected threat"
    );
}

#[test]
fn escort_cannot_select_a_terrain_hidden_aircraft_near_its_charge() {
    let mut mission = AiMission::new();
    mission.push(radar_escort());
    let hostile = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., 30_000.]);
    let charge = charge(10, [0., 20_000., 1_000.]);
    let world = world_with(&mission, [charge, hostile]);
    let ridge = |_x: f64, z: f64| {
        if (10_000. ..20_000.).contains(&z) {
            30_000.
        } else {
            0.
        }
    };
    let output = mission.step(&world, &ridge, TimeOfDay(0)).unwrap();

    let actor = mission.actor(1).unwrap();
    assert!(actor.sensors().unwrap().contact(2).is_none());
    assert_eq!(actor.controller().target(), None);
    assert!(output.launches.is_empty());
}

#[test]
fn escort_defends_against_a_supported_missile_and_waits_for_active_seeker_acquisition() {
    use crate::combat::missiles::Guidance;
    let incoming = |guidance, acquired| MissileSnapshot {
        id: 90,
        owner: 2,
        position: [0., 20_000., -1_000.],
        velocity: [0., 0., 2_000.],
        guidance,
        target: Some(1),
        radar_active: guidance == Guidance::Active,
        radar_acquired: acquired,
        supported: guidance == Guidance::Supported,
        supporting_radar_position: Some([0., 20_000., -60_000.]),
        alive: true,
    };
    let mut mission = AiMission::new();
    mission.push(radar_escort());
    let world = world_with(&mission, [charge(10, [0., 20_000., 1_000.])]);

    mission.set_missiles(vec![incoming(Guidance::Active, false)]);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert!(mission.actor(1).unwrap().defense_decision().is_none());
    mission.set_missiles(vec![incoming(Guidance::Active, true)]);
    mission.step(&world, &flat, TimeOfDay(1)).unwrap();
    assert_eq!(mission.actor(1).unwrap().activity(), Activity::Defending);

    let mut supported = AiMission::new();
    supported.push(radar_escort());
    supported.set_missiles(vec![incoming(Guidance::Supported, false)]);
    let world = world_with(&supported, [charge(10, [0., 20_000., 1_000.])]);
    supported.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(supported.actor(1).unwrap().activity(), Activity::Defending);
}

#[test]
fn protected_receivers_active_warning_cues_escort_without_revealing_the_launcher() {
    use crate::combat::missiles::Guidance;
    let mut mission = AiMission::new();
    mission.push(radar_escort());
    mission.push(AiActor::new(setup(10, 1, 0, [0., 20_000., 1_000.], 0.)).unwrap());
    mission.set_missiles(vec![MissileSnapshot {
        id: 90,
        owner: 2,
        position: [0., 20_000., -1_000.],
        velocity: [0., 0., 2_000.],
        guidance: Guidance::Active,
        target: Some(10),
        radar_active: true,
        radar_acquired: true,
        supported: false,
        supporting_radar_position: None,
        alive: true,
    }]);
    let world = world_with(&mission, []);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert!(mission.actor(1).unwrap().perceived_attacks().is_empty());
    mission.step(&world, &flat, TimeOfDay(1)).unwrap();
    let escort = mission.actor(1).unwrap();
    assert!(
        escort.perceived_attacks().iter().any(|attack| {
            attack.report.defended_id == 10 && attack.report.attacker_id.is_none()
        })
    );
    assert_eq!(escort.controller().target(), None);
    assert_eq!(escort.activity(), Activity::Searching);
}

#[test]
fn protected_report_preempts_a_nearer_unrelated_hostile_next_tick() {
    let mut mission = AiMission::new();
    let mut escort = AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap();
    let mut duty = assignment(
        engagement::Role::Escort,
        engagement::Stance::ProtectAssigned,
    );
    duty.protected_ids.push(10);
    escort.set_assignment(duty);
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
    let near = visible_object(template, 2, [500., 20_000., 0.]);
    let attacker = visible_object(template, 3, [5_000., 20_000., 0.]);
    let world = world_with(
        &mission,
        [charge(10, [0., 20_000., 1_000.]), near, attacker],
    );
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();

    assert_eq!(mission.actor(1).unwrap().controller().target(), Some(3));
}

#[test]
fn receiver_shares_only_with_assigned_same_side_escort() {
    let mut mission = AiMission::new();
    mission.push(AiActor::new(setup(10, 1, 0, [0., 20_000., 0.], 0.)).unwrap());
    let mut escort = AiActor::new(setup(11, 1, 1, [1_000., 20_000., 0.], 0.)).unwrap();
    let mut duty = assignment(
        engagement::Role::Escort,
        engagement::Stance::ProtectAssigned,
    );
    duty.protected_ids.push(10);
    escort.set_assignment(duty);
    mission.push(escort);
    mission.push(AiActor::new(setup(12, 1, 2, [2_000., 20_000., 0.], 0.)).unwrap());
    mission.push(AiActor::new(setup(20, 2, 0, [3_000., 20_000., 0.], 0.)).unwrap());

    mission.report_attack(
        10,
        engagement::ThreatReport {
            attacker_id: Some(20),
            defended_id: 10,
        },
    );
    let world = world_with(&mission, []);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();

    assert_eq!(mission.actor(10).unwrap().perceived_attacks().len(), 1);
    assert_eq!(mission.actor(11).unwrap().perceived_attacks().len(), 1);
    assert!(mission.actor(12).unwrap().perceived_attacks().is_empty());
    assert!(mission.actor(20).unwrap().perceived_attacks().is_empty());
}

#[test]
fn unknown_bearing_cues_search_without_target_memory_or_fire() {
    let mut mission = AiMission::new();
    let mut escort = AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap();
    let mut duty = assignment(
        engagement::Role::Escort,
        engagement::Stance::ProtectAssigned,
    );
    duty.protected_ids.push(10);
    escort.set_assignment(duty);
    mission.push(escort);
    mission.set_external_leader(Side(1), 0, 10);
    mission.report_attack_bearing(
        10,
        engagement::ThreatReport {
            attacker_id: None,
            defended_id: 10,
        },
        Some(90.),
    );
    let world = world_with(&mission, [charge(10, [0., 20_000., 1_000.])]);
    let output = mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    let actor = mission.actor(1).unwrap();

    assert_eq!(actor.activity(), Activity::Searching);
    assert_eq!(actor.controller().target(), None);
    assert_eq!(actor.search_target, None);
    assert!(
        actor
            .awareness()
            .remembered()
            .all(|snapshot| snapshot.target.side == Side(1))
    );
    assert!(output.launches.is_empty());
    assert!(actor.flight().yaw > 0.);
}

#[test]
fn attack_report_expires_at_240_ticks_without_self_forwarding() {
    let mut mission = AiMission::new();
    mission.push(AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap());
    mission.report_attack(
        1,
        engagement::ThreatReport {
            attacker_id: None,
            defended_id: 1,
        },
    );
    let world = world_with(&mission, []);
    for _ in 0..240 {
        mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    }
    assert_eq!(mission.tick(), 240);
    assert_eq!(mission.actor(1).unwrap().perceived_attacks().len(), 1);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert!(mission.actor(1).unwrap().perceived_attacks().is_empty());
}

#[test]
fn known_but_unobserved_attacker_never_becomes_a_fire_target() {
    let mut mission = AiMission::new();
    let mut actor = perception_actor(Experience::Ace);
    actor.set_assignment(assignment(
        engagement::Role::Disengage,
        engagement::Stance::SelfDefense,
    ));
    mission.push(actor);
    mission.report_attack(
        1,
        engagement::ThreatReport {
            attacker_id: Some(2),
            defended_id: 1,
        },
    );
    let hidden = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., -40_000.]);
    let world = world_with(&mission, [hidden]);
    let output = mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(1).unwrap().controller().target(), None);
    assert!(output.launches.is_empty());
}

#[test]
fn escort_leash_rejoins_until_the_charge_is_inside_eight_nm() {
    let mut mission = AiMission::new();
    let mut escort = AiActor::new(setup(1, 1, 0, [0., 20_000., 0.], 0.)).unwrap();
    let mut duty = assignment(engagement::Role::Escort, engagement::Stance::EngageAssigned);
    duty.protected_ids.push(10);
    escort.set_assignment(duty);
    mission.push(escort);

    let protected_at = |mission: &AiMission, distance: f64| {
        let own = mission.actor(1).unwrap().flight().position;
        charge(10, [own[0] + distance, own[1], own[2]])
    };
    let far = world_with(&mission, [protected_at(&mission, 11. * 6_076.)]);
    mission.step(&far, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(1).unwrap().activity(), Activity::Rejoining);
    assert!(mission.actor(1).unwrap().mission_policy.must_rejoin());

    let hysteresis = world_with(&mission, [protected_at(&mission, 8.5 * 6_076.)]);
    mission.step(&hysteresis, &flat, TimeOfDay(0)).unwrap();
    assert!(mission.actor(1).unwrap().mission_policy.must_rejoin());

    let inside = world_with(&mission, [protected_at(&mission, 8. * 6_076.)]);
    mission.step(&inside, &flat, TimeOfDay(0)).unwrap();
    assert!(!mission.actor(1).unwrap().mission_policy.must_rejoin());
}

#[test]
fn escort_inside_leash_uses_the_assigned_protected_pose_as_formation_leader() {
    let mut mission = AiMission::new();
    let mut escort = AiActor::new(setup(1, 1, 1, [0., 20_000., 0.], 0.)).unwrap();
    let mut duty = assignment(
        engagement::Role::Escort,
        engagement::Stance::ProtectAssigned,
    );
    duty.protected_ids.push(10);
    escort.set_assignment(duty);
    mission.push(escort);
    mission.set_external_leader(Side(1), 0, 10);
    let protected = charge(10, [5. * 6_076., 20_000., 0.]);
    let world = world_with(&mission, [protected]);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();

    assert!(mission.actor(1).unwrap().flight().yaw > 0.);
    assert!(!mission.actor(1).unwrap().mission_policy.must_rejoin());
}

#[test]
fn explicit_target_order_replaces_a_prior_intercept_assignment() {
    let mut mission = AiMission::new();
    let mut actor = perception_actor(Experience::Ace);
    let mut intercept = assignment(
        engagement::Role::Intercept,
        engagement::Stance::EngageAssigned,
    );
    intercept.destroy_ids.push(2);
    actor.set_assignment(intercept);
    mission.push(actor);
    mission
        .order(
            1,
            WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(TargetId(3))),
        )
        .unwrap()
        .unwrap();
    assert_eq!(mission.actor(1).unwrap().assignment().destroy_ids, vec![3]);
}

#[test]
fn weapons_hold_preserves_missile_defense_and_blocks_offense() {
    let mut mission = AiMission::new();
    mission.push(perception_actor(Experience::Ace));
    mission
        .order(1, WingRequest::TargetAssignment(TargetOrder::HoldFire))
        .unwrap()
        .unwrap();
    mission.set_missiles(vec![MissileSnapshot {
        id: 90,
        owner: 2,
        position: [0., 20_000., 5_000.],
        velocity: [0., 0., -1_000.],
        guidance: crate::combat::missiles::Guidance::Supported,
        target: Some(1),
        radar_active: false,
        radar_acquired: false,
        supported: true,
        supporting_radar_position: Some([5_000., 20_000., 0.]),
        alive: true,
    }]);
    let world = world_with(&mission, []);
    let output = mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert!(mission.actor(1).unwrap().defense_decision().is_some());
    assert_eq!(mission.actor(1).unwrap().activity(), Activity::Defending);
    assert!(output.launches.is_empty());
}

#[test]
fn cap_returns_to_region_and_assigned_distant_target_cannot_launch() {
    let mut mission = AiMission::new();
    // Written for observation every tick.
    mission.set_test_settings(AiTestSettings::every_tick());
    let mut actor = AiActor::new(setup(1, 1, 0, [101., 20_000., 0.], 0.)).unwrap();
    let mut cap = assignment(
        engagement::Role::CombatAirPatrol,
        engagement::Stance::EngageAssigned,
    );
    cap.patrol = Some(engagement::PatrolRegion {
        clip: None,
        center_ft: [0., 20_000., 0.],
        radius_ft: 100.,
    });
    actor.set_assignment(cap);
    mission.push(actor);
    let world = world_with(&mission, []);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(1).unwrap().activity(), Activity::Rejoining);

    let mut intercept = assignment(
        engagement::Role::Intercept,
        engagement::Stance::EngageAssigned,
    );
    intercept.destroy_ids.push(2);
    mission.actor_mut(1).unwrap().set_assignment(intercept);
    let distant = visible_object(mission.actor(1).unwrap(), 2, [100_000., 20_000., 0.]);
    let world = world_with(&mission, [distant]);
    let output = mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(1).unwrap().controller().target(), Some(2));
    assert!(output.launches.is_empty());
}

#[test]
fn cap_exact_boundary_does_not_request_a_return() {
    let mut mission = AiMission::new();
    let mut actor = AiActor::new(setup(1, 1, 0, [100., 20_000., 0.], 0.)).unwrap();
    let mut cap = assignment(
        engagement::Role::CombatAirPatrol,
        engagement::Stance::EngageAssigned,
    );
    cap.patrol = Some(engagement::PatrolRegion {
        clip: None,
        center_ft: [0., 20_000., 0.],
        radius_ft: 100.,
    });
    actor.set_assignment(cap);
    mission.push(actor);
    let world = world_with(&mission, []);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert_ne!(mission.actor(1).unwrap().activity(), Activity::Rejoining);
}

#[test]
fn remembered_investigation_stops_when_the_mission_role_forbids_it() {
    let mut mission = AiMission::new();
    // Written for observation every tick.
    mission.set_test_settings(AiTestSettings::every_tick());
    let mut actor = perception_actor(Experience::Ace);
    let mut intercept = assignment(
        engagement::Role::Intercept,
        engagement::Stance::EngageAssigned,
    );
    intercept.destroy_ids.push(2);
    actor.set_assignment(intercept);
    mission.push(actor);
    let seen = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., 1_000.]);
    let world = world_with(&mission, [seen]);
    mission.step(&world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(1).unwrap().controller().target(), Some(2));

    let hidden = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., -40_000.]);
    let lost_world = world_with(&mission, [hidden]);
    mission.step(&lost_world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(1).unwrap().search_target, Some(2));

    mission.actor_mut(1).unwrap().set_assignment(assignment(
        engagement::Role::Disengage,
        engagement::Stance::SelfDefense,
    ));
    mission.step(&lost_world, &flat, TimeOfDay(0)).unwrap();
    assert_eq!(mission.actor(1).unwrap().search_target, None);
}

#[test]
fn cap_patrols_before_contact_and_engages_from_outside_station() {
    for side in [1, 2] {
        let mut mission = AiMission::new();
        let mut config = setup(1, side, 0, [0., 20_000., -70000.], 0.);
        config.sensors = perception_actor(Experience::Ace).sensors;
        let mut actor = AiActor::new(config).unwrap();
        enable_test_radar(&mut actor);
        actor.set_assignment(engagement::Assignment {
            role: engagement::Role::CombatAirPatrol,
            patrol: Some(engagement::PatrolRegion {
                clip: None,
                center_ft: [0., 20_000., 0.],
                radius_ft: 60760.,
            }),
            ..Default::default()
        });
        mission.push(actor);
        mission.start_in_formation();
        mission
            .step(&world_with(&mission, []), &flat, TimeOfDay(0))
            .unwrap();
        assert_eq!(mission.actor(1).unwrap().activity(), Activity::Rejoining);
        let mut target = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., -59000.]);
        target.side = Side(3 - side);
        for tick in 1..5 {
            mission
                .step(
                    &world_with(&mission, [target.clone()]),
                    &flat,
                    TimeOfDay(tick),
                )
                .unwrap();
        }
        let actor = mission.actor(1).unwrap();
        assert_eq!(actor.controller().target(), Some(2));
        assert_ne!(actor.activity(), Activity::Rejoining);
        // A live bandit outside the station must not extend the patrol's chase.
        target = visible_object(mission.actor(1).unwrap(), 2, [0., 20_000., -61000.]);
        target.side = Side(3 - side);
        // Let the ordinary radar scan refresh the measured position.
        for tick in 5..365 {
            mission
                .step(
                    &world_with(&mission, [target.clone()]),
                    &flat,
                    TimeOfDay(tick),
                )
                .unwrap();
        }
        assert_eq!(mission.actor(1).unwrap().controller().target(), None);
        assert_eq!(mission.actor(1).unwrap().activity(), Activity::Rejoining);
        // The last bandit's loss ends the engagement, not the patrol duty.
        mission
            .step(&world_with(&mission, []), &flat, TimeOfDay(365))
            .unwrap();
        assert!(!mission.actor(1).unwrap().controller().mission_complete());
        mission
            .order(1, WingRequest::TargetAssignment(TargetOrder::HoldFire))
            .unwrap()
            .unwrap();
        mission
            .step(&world_with(&mission, []), &flat, TimeOfDay(366))
            .unwrap();
        assert_ne!(mission.actor(1).unwrap().activity(), Activity::Rejoining);
    }
}

fn incoming_defense_snapshot(id: u32) -> MissileSnapshot {
    MissileSnapshot {
        id,
        owner: 3,
        position: [0., 20_000., -1500.],
        velocity: [0., 0., 2000.],
        guidance: crate::combat::missiles::Guidance::Supported,
        target: Some(2),
        radar_active: false,
        radar_acquired: false,
        supported: true,
        supporting_radar_position: Some([5000., 20_000., 0.]),
        alive: true,
    }
}

fn step_defense(mission: &mut AiMission) {
    mission
        .step(&world_with(mission, []), &flat, TimeOfDay(0))
        .unwrap();
}

#[test]
fn removing_an_inert_missile_snapshot_ends_defense_without_a_grace_period() {
    let mut mission = neutral_wing_mission();
    mission.set_missiles(vec![incoming_defense_snapshot(90)]);
    step_defense(&mut mission);
    assert!(mission.actor(2).unwrap().defense_decision().is_some());
    // Combat omits the body and explicitly confirms it is inert. A missing
    // observation alone must retain the existing tracking-gap grace.
    mission.set_missiles(Vec::new());
    mission.end_missile_threats(&[90]);
    step_defense(&mut mission);
    let actor = mission.actor(2).unwrap();
    assert_eq!(actor.missile_threats().count(), 0);
    assert!(actor.defense_decision().is_none());
    assert!(!actor.controller().defending());
    assert!(actor.device_schedule.iter().all(|entry| !entry.3));
    for _ in 0..=defense::THREAT_MEMORY_TICKS {
        step_defense(&mut mission);
        assert!(mission.actor(2).unwrap().defense_decision().is_none());
    }
}

#[test]
fn live_store_tracking_gap_keeps_defense_for_two_seconds() {
    let mut mission = neutral_wing_mission();
    mission.set_missiles(vec![incoming_defense_snapshot(90)]);
    step_defense(&mut mission);
    mission.set_missiles(Vec::new());
    for _ in 0..defense::THREAT_MEMORY_TICKS {
        step_defense(&mut mission);
        let actor = mission.actor(2).unwrap();
        assert_eq!(actor.missile_threats().count(), 0);
        let decision = actor.defense_decision().expect("bridge the live-store gap");
        assert_eq!(decision.threat_id, 90);
        assert!(decision.debug.stale);
        assert!(decision.burst.is_none());
    }
    step_defense(&mut mission);
    assert!(mission.actor(2).unwrap().defense_decision().is_none());
}

#[test]
fn inert_notice_preserves_other_live_threats_and_checkpoint_resume() {
    use crate::checkpoint::{Models, from_bytes, to_bytes};
    let mut mission = neutral_wing_mission();
    mission.set_missiles(vec![incoming_defense_snapshot(90)]);
    step_defense(&mut mission);
    // Save while the selected threat has no current observation, then deliver
    // its end notice after restoring, as the combat host does each tick.
    mission.set_missiles(Vec::new());
    step_defense(&mut mission);
    let mut models = Models::default();
    for actor in mission.actors() {
        models
            .insert(actor.identity().aircraft, actor.flight().import_model())
            .unwrap();
    }
    let saved = to_bytes(&mission, &models).unwrap();
    let mut twin: AiMission = from_bytes(&saved, &models).unwrap();
    for current in [&mut mission, &mut twin] {
        current.set_missiles(vec![incoming_defense_snapshot(91)]);
        current.end_missile_threats(&[90, 999]);
        step_defense(current);
        assert_eq!(
            current
                .actor(2)
                .unwrap()
                .defense_decision()
                .unwrap()
                .threat_id,
            91
        );
        current.end_missile_threats(&[90]); // Repeated notices are harmless.
        step_defense(current);
        assert_eq!(
            current
                .actor(2)
                .unwrap()
                .defense_decision()
                .unwrap()
                .threat_id,
            91
        );
        current.set_missiles(Vec::new());
        current.end_missile_threats(&[91]);
        step_defense(current);
        assert!(current.actor(2).unwrap().defense_decision().is_none());
    }
    assert_eq!(
        to_bytes(&mission, &models).unwrap(),
        to_bytes(&twin, &models).unwrap()
    );
}
