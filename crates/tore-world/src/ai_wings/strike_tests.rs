//! The strike bridge's lasing half (docs/spec/ai-strike.md, "Lasing", slice
//! ST-L) against the synthetic defended template: the lase order's API, the
//! cockpit's sight and laser it drives, the laser it lets go, and the
//! callsign the readout names an AI by. No retail data.
use crate::mission::{Skill, WingSpec};
use crate::seats::SeatInput;
use crate::test_support::surface::{spec_with_target, surface_resources, target};
use crate::world::{Seating, TickOutput, World};
use tore_formats::aircraft::AircraftId;
use tore_sim::ai::launch::Side as Wing;
use tore_sim::ai::strike::{LaseUntil, Phase};
use tore_sim::combat::live::StrikeTarget;
use tore_sim::combat::sight::{Gimbal, SightSensor};

fn world() -> World {
    let mut spec = spec_with_target(&target("QUCITY", 3, 3, 9));
    spec.wings[0].count = 2;
    spec.wings[3] = WingSpec {
        aircraft: AircraftId::F18,
        count: 2,
        skill: Skill::Average,
    };
    World::new(&spec, &surface_resources(), Seating::SinglePlayer).unwrap()
}

/// A target nobody defends and no enemy aircraft: nothing shoots at the
/// strikers, so their decisions are the ones under test.
fn quiet() -> World {
    let mut spec = spec_with_target(&target("QUCITY", 0, 0, 9));
    spec.wings[0].count = 2;
    World::new(&spec, &surface_resources(), Seating::SinglePlayer).unwrap()
}

fn step(world: &mut World, ticks: u64) {
    let mut out = TickOutput::default();
    for _ in 0..ticks {
        let input = SeatInput {
            tick: world.tick(),
            ..SeatInput::default()
        };
        world.step(&[input], &mut out).unwrap();
    }
}

/// A Pave Spike-like pod: the laser, ten miles, the hemisphere.
fn pod() -> SightSensor {
    SightSensor {
        laser: true,
        range_ft: 60_760.,
        gimbal: Gimbal::Hemisphere,
        eye: [0.; 3],
        default_look: [0., -0.17],
    }
}

fn friendly(world: &World) -> Vec<u32> {
    world
        .ai_wings
        .as_ref()
        .unwrap()
        .slots()
        .iter()
        .filter(|slot| slot.side == Wing::Friendly)
        .map(|slot| slot.id)
        .collect()
}

#[test]
fn a_lase_order_needs_a_designator_and_an_ai_aircraft() {
    let mut world = world();
    let wingman = friendly(&world)[0];
    let targets =
        super::super::outcome::ground_targets(&world.terrain.surface, &world.combat.state);
    let target = targets[0];
    let wings = world.ai_wings.as_mut().unwrap();
    assert!(!wings.can_lase(wingman), "a synthetic F/A-18 has none");
    assert!(!wings.order_lase(wingman, target, LaseUntil::TargetDies));
    assert_eq!(wings.lasing_for(wingman), None);
    assert!(!wings.order_lase(9_999, target, LaseUntil::TargetDies));
    assert!(!wings.cancel_lase(wingman));
    // Its callsign is the one the messages use: wing, member.
    assert_eq!(wings.callsign(wingman).as_deref(), Some("Friendly 1-2"));
    assert_eq!(wings.callsign(9_999), None);
}

#[test]
fn a_lase_order_points_the_sight_and_the_laser_at_the_target_and_cancelling_lets_go() {
    let mut world = world();
    let wingman = friendly(&world)[0];
    let target =
        super::super::outcome::ground_targets(&world.terrain.surface, &world.combat.state)[0];
    let wings = world.ai_wings.as_mut().unwrap();
    let mut config = wings.configs.get(&wingman).unwrap().clone();
    config.sight = Some(pod());
    wings.configs.insert(wingman, config);
    assert!(wings.can_lase(wingman));
    assert!(wings.order_lase(wingman, target, LaseUntil::TargetDies));
    assert_eq!(wings.lasing_for(wingman), Some(target));

    step(&mut world, 60);
    let wings = world.ai_wings.as_ref().unwrap();
    let actor = wings.mission().actor(wingman).unwrap();
    assert_eq!(actor.strike().map(|s| s.phase()), Some(Phase::Lase));
    let cockpit = world
        .combat
        .state
        .strike_cockpit(wingman)
        .expect("the AI's strike cockpit");
    assert_eq!(cockpit.target, Some(StrikeTarget::Object(target)));
    assert_eq!(cockpit.sight.and_then(|s| s.target()), Some(target));
    assert_eq!(
        cockpit.laser.manual,
        Some(true),
        "on by hand while it lases"
    );
    // Its spot is an ordinary spot of its side once the laser lases (here it
    // may be out of range or masked; the state says which).
    assert!(cockpit.laser.wanted());

    // The order changes: the duty ends, the order it made goes, the laser
    // is let go at the next step.
    let wings = world.ai_wings.as_mut().unwrap();
    assert!(wings.cancel_lase(wingman));
    assert_eq!(wings.lasing_for(wingman), None);
    assert_eq!(
        wings.mission().actor(wingman).unwrap().strike_order(),
        None,
        "the ordered target was the duty's"
    );
    step(&mut world, 2);
    let cockpit = world.combat.state.strike_cockpit(wingman).unwrap();
    assert_eq!(cockpit.laser.manual, None);
    assert!(!cockpit.laser.wanted());
}

#[test]
fn a_lase_order_ends_when_the_target_is_destroyed() {
    let mut world = world();
    let wingman = friendly(&world)[0];
    let target =
        super::super::outcome::ground_targets(&world.terrain.surface, &world.combat.state)[0];
    let wings = world.ai_wings.as_mut().unwrap();
    let mut config = wings.configs.get(&wingman).unwrap().clone();
    config.sight = Some(pod());
    wings.configs.insert(wingman, config);
    assert!(wings.order_lase(wingman, target, LaseUntil::TargetDies));
    step(&mut world, 30);
    assert_eq!(
        world
            .ai_wings
            .as_ref()
            .unwrap()
            .mission()
            .actor(wingman)
            .unwrap()
            .strike()
            .map(|s| s.phase()),
        Some(Phase::Lase)
    );
    world
        .combat
        .state
        .targets
        .iter_mut()
        .find(|t| t.id == target)
        .unwrap()
        .hp = 0;
    step(&mut world, 30);
    let wings = world.ai_wings.as_ref().unwrap();
    assert_eq!(wings.lasing_for(wingman), None);
}

#[test]
fn an_ai_flown_cockpit_with_a_laser_weapon_armed_lases_before_the_release() {
    let mut world = world();
    let wingman = friendly(&world)[0];
    let target =
        super::super::outcome::ground_targets(&world.terrain.surface, &world.combat.state)[0];
    let wings = world.ai_wings.as_mut().unwrap();
    let mut config = wings.configs.get(&wingman).unwrap().clone();
    config.sight = Some(pod());
    // A laser weapon on station 0: signature 1 with the designator flag.
    config.stations[0].weapon.seeker.signature = 1;
    config.stations[0].weapon.flags |= 0x400;
    wings.configs.insert(wingman, config.clone());
    // The AI flies a cockpit with that weapon selected and armed and its
    // sight on the target, the laser on its hand setting automatic (the
    // takeover's case).
    let side = world
        .combat
        .state
        .targets
        .iter()
        .find(|t| t.id == wingman)
        .unwrap()
        .side;
    {
        let cockpit = world
            .combat
            .state
            .ai_cockpit_mut(wingman, side, &config)
            .unwrap();
        cockpit.selected = 0;
        cockpit.armed = true;
        cockpit.target = Some(StrikeTarget::Object(target));
        cockpit.sight.as_mut().unwrap().track(target);
    }
    step(&mut world, 2);
    let cockpit = world.combat.state.strike_cockpit(wingman).unwrap();
    assert_eq!(cockpit.laser.manual, None);
    assert!(cockpit.laser.automatic, "armed: the automatic rule is on");
    assert!(cockpit.laser.wanted());
    // Disarmed, it is off again.
    world
        .combat
        .state
        .ai_cockpit_mut(wingman, side, &config)
        .unwrap()
        .armed = false;
    step(&mut world, 2);
    let cockpit = world.combat.state.strike_cockpit(wingman).unwrap();
    assert!(!cockpit.laser.automatic);
}

/// Makes station 1 of AI aircraft `id` a laser weapon: a glide bomb (it
/// falls on the bomb equation with no spot) or, with `missile`, one with a
/// motor (a laser missile, which cannot).
fn arm_with_laser(world: &mut World, id: u32, missile: bool) {
    let wings = world.ai_wings.as_mut().unwrap();
    let mut config = wings.configs.get(&id).unwrap().clone();
    let w = &mut config.stations[1].weapon;
    w.seeker.signature = 1;
    w.flags |= 0x400 | 0x20000;
    w.flags &= !0x10;
    w.movement.ignite_t = 0;
    w.movement.fuel_t = if missile { 40 } else { 0 };
    wings.configs.insert(id, config);
    // Everything else it carries is out of the way.
    let actor = wings.mission_mut().actor_mut(id).unwrap();
    for spec in actor.stations_mut() {
        if usize::from(spec.station.0) != 1 {
            spec.store.inhibited = true;
        }
    }
}

/// The no-laze radio lines the journal wrote since the last look: the
/// words and the call's number (a call has an entry for each stage it
/// passed).
fn spoken(world: &mut World) -> Vec<(String, Option<u64>, bool)> {
    world
        .comms
        .take_journal()
        .into_iter()
        .filter_map(|entry| match entry.origin.cause {
            crate::comms::journal::Cause::NoLaze { rtb } => Some((entry.text, entry.call, rtb)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_laser_bomb_with_no_laze_support_is_dropped_by_hand_and_the_radio_says_so() {
    let mut world = quiet();
    let wingman = friendly(&world)[0];
    let target =
        super::super::outcome::ground_targets(&world.terrain.surface, &world.combat.state)[0];
    arm_with_laser(&mut world, wingman, false);
    world
        .ai_wings
        .as_mut()
        .unwrap()
        .mission_mut()
        .actor_mut(wingman)
        .unwrap()
        .set_strike_order(Some(target));
    let mut told = None;
    let mut lines = Vec::new();
    for _ in 0..200 {
        step(&mut world, 120);
        lines.extend(spoken(&mut world));
        if let Some(call) = world
            .ai_wings
            .as_ref()
            .unwrap()
            .mission()
            .actor(wingman)
            .and_then(|a| a.strike())
            .and_then(|s| s.no_laze())
        {
            told = Some(*call);
        }
        if !lines.is_empty() {
            break;
        }
    }
    let call = told.expect("a no-laze decision");
    assert_eq!(call.kind, tore_sim::ai::strike::NoLaze::Manual, "{call:?}");
    step(&mut world, 120);
    lines.extend(spoken(&mut world));
    assert!(!lines.is_empty(), "the radio said nothing");
    assert!(
        lines
            .iter()
            .all(|(text, _, rtb)| !rtb && text == "No laze support, dropping manual"),
        "{lines:?}"
    );
    // Said once: every entry is the same call.
    let calls: std::collections::BTreeSet<_> = lines.iter().map(|l| l.1).collect();
    assert_eq!(calls.len(), 1, "{lines:?}");
}

#[test]
fn a_laser_missile_with_no_laze_support_goes_home_and_the_radio_says_so() {
    let mut world = quiet();
    let wingman = friendly(&world)[0];
    let target =
        super::super::outcome::ground_targets(&world.terrain.surface, &world.combat.state)[0];
    arm_with_laser(&mut world, wingman, true);
    world
        .ai_wings
        .as_mut()
        .unwrap()
        .mission_mut()
        .actor_mut(wingman)
        .unwrap()
        .set_strike_order(Some(target));
    let mut lines = Vec::new();
    for _ in 0..200 {
        step(&mut world, 120);
        lines.extend(spoken(&mut world));
        if !lines.is_empty() {
            break;
        }
    }
    assert!(!lines.is_empty(), "the radio said nothing");
    assert!(
        lines
            .iter()
            .all(|(text, _, rtb)| *rtb && text == "No laze support, RTB"),
        "{lines:?}"
    );
    // The strike was kept long enough to be told, then dropped: the wingman
    // is no longer striking and nothing is released.
    step(&mut world, 240);
    let actor = world
        .ai_wings
        .as_ref()
        .unwrap()
        .mission()
        .actor(wingman)
        .unwrap();
    assert!(
        actor
            .strike()
            .is_none_or(|s| s.no_laze().is_none_or(|c| c.told))
    );
    assert!(
        world
            .combat
            .state
            .projectiles
            .iter()
            .all(|p| p.owner != wingman)
    );
}

/// A helicopter's stand-off advance reads its guided store's time of flight
/// (John, 2026-10-29): the energy model's flight out to each share of the
/// zone, rising with the range, the same every time; under the other rules
/// the range at the record's top speed.
#[test]
fn a_guided_stores_flight_time_rises_with_range_and_is_the_same_every_time() {
    use tore_sim::combat::missiles::Rules;
    let mut world = quiet();
    let id = friendly(&world)[0];
    arm_with_laser(&mut world, id, true);
    let wings = world.ai_wings.as_ref().unwrap();
    let mut w = wings.configs.get(&id).unwrap().stations[1].weapon.clone();
    w.seeker.zones[1].maximum_range = 12_000;
    w.movement.maximum_speed = 1_200;
    let spec = super::flight_times(&w, Rules::Spec).expect("a zone and a speed");
    for (range, seconds) in spec.points {
        assert!((seconds - range / 1_200.).abs() < 1e-9, "{range} {seconds}");
    }
    let energy = super::flight_times(&w, Rules::Energy).expect("a zone and a speed");
    assert_eq!(super::flight_times(&w, Rules::Energy), Some(energy));
    let mut previous = (0., 0.);
    for point in energy.points {
        assert!(point.0 > previous.0 && point.1 > previous.1, "{energy:?}");
        previous = point;
    }
    assert_eq!(energy.points[3].0, 12_000.);
}
