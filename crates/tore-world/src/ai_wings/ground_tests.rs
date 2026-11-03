//! The bridge half of the strikers' ground picture against the synthetic
//! defended template (docs/spec/ai-strike.md, "Who strikes" and "What the AI
//! knows"): the Strike hook, the briefing from the Quick Mission objective,
//! side neutrality, the 6 s rebuild in a running world, and CAP untouched.
//! No retail data.
use super::super::FRIENDLY_SIDE;
use crate::mission::{Skill, WingSpec};
use crate::seats::SeatInput;
use crate::test_support::surface::{spec_with_target, surface_resources, target};
use crate::world::{Seating, TickOutput, World};
use tore_formats::aircraft::AircraftId;
use tore_sim::ai::awareness::ground::{GROUND_REBUILD_TICKS, GroundPicture};
use tore_sim::ai::engagement::StrikeRole;
use tore_sim::ai::launch::Side as Wing;
use tore_sim::combat::live;
use tore_sim::combat::missiles::TargetRole;

/// The defended template with the player's F/A-18 and an AI wingman, and two
/// enemy F/A-18s.
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

fn ai_of(world: &World, side: Wing) -> Vec<u32> {
    world
        .ai_wings
        .as_ref()
        .unwrap()
        .slots()
        .iter()
        .filter(|slot| slot.side == side)
        .map(|slot| slot.id)
        .collect()
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

fn picture(world: &World, id: u32) -> Option<&GroundPicture> {
    world
        .ai_wings
        .as_ref()
        .unwrap()
        .mission()
        .actor(id)
        .unwrap()
        .ground_picture()
}

#[test]
fn a_strike_wingman_is_briefed_on_the_hostile_objective_and_refreshes_every_6_s() {
    let mut world = world();
    let wingman = ai_of(&world, Wing::Friendly)[0];
    let targets =
        super::super::outcome::ground_targets(&world.terrain.surface, &world.combat.state);
    assert!(!targets.is_empty(), "the template names destroy targets");
    // Every actor flies CAP until told otherwise: no picture at all.
    let wings = world.ai_wings.as_mut().unwrap();
    assert!(
        wings
            .mission()
            .actors()
            .iter()
            .all(|a| a.ground_picture().is_none())
    );
    assert!(wings.set_strike_role(wingman, StrikeRole::Strike));
    assert!(
        !wings.set_strike_role(9_999, StrikeRole::Strike),
        "not an AI aircraft"
    );
    step(&mut world, 1);
    let first = picture(&world, wingman).expect("a striker has a picture");
    let objective = first.objective().expect("briefed at the first step");
    // Blue is briefed on Redfor's destroy targets, at their centre.
    let redfor: Vec<u32> = targets
        .iter()
        .copied()
        .filter(|id| {
            world
                .combat
                .state
                .targets
                .iter()
                .any(|t| t.id == *id && t.side != FRIENDLY_SIDE && t.side != live::NO_SIDE)
        })
        .collect();
    assert!(!redfor.is_empty());
    assert_eq!(objective.targets, redfor);
    assert!(objective.point.is_some());
    assert!(
        objective.defenses.is_empty(),
        "no anti-radiation store, no SEAD"
    );
    assert_eq!(first.briefed().len(), redfor.len());
    assert!(first.briefed().iter().all(|b| b.standing));
    let built = first.built_tick().unwrap();

    // The picture is rebuilt 6 s later, not before.
    step(&mut world, GROUND_REBUILD_TICKS - 1);
    assert_eq!(picture(&world, wingman).unwrap().built_tick(), Some(built));
    step(&mut world, 1);
    assert_eq!(
        picture(&world, wingman).unwrap().built_tick(),
        Some(built + GROUND_REBUILD_TICKS)
    );
    // Everyone else is still air to air.
    let wings = world.ai_wings.as_ref().unwrap();
    assert!(
        wings
            .mission()
            .actors()
            .iter()
            .filter(|a| a.id() != wingman)
            .all(|a| a.ground_picture().is_none())
    );
}

#[test]
fn a_redfor_striker_gets_the_same_picture_but_no_objective() {
    let mut world = world();
    let enemy = ai_of(&world, Wing::Enemy)[0];
    world
        .ai_wings
        .as_mut()
        .unwrap()
        .set_strike_role(enemy, StrikeRole::Strike);
    step(&mut world, 1);
    let picture = picture(&world, enemy).unwrap();
    let objective = picture.objective().unwrap();
    assert!(objective.targets.is_empty(), "Redfor's targets are its own");
    assert_eq!(objective.point, None);
    assert!(picture.built_tick().is_some());
}

#[test]
fn the_subjects_carry_sides_and_defence_facts_and_a_striker_over_the_target_lists_them() {
    let mut world = world();
    let subjects = super::subjects(
        &world.combat.state,
        &world.terrain.surface,
        &mut Default::default(),
    );
    // Every surface row is described; aircraft in flight are not.
    let rows = world
        .combat
        .state
        .targets
        .iter()
        .filter(|t| t.role == TargetRole::Surface)
        .count();
    assert!(subjects.len() >= rows);
    assert!(subjects.iter().all(|s| !s.object.airborne));
    // The armed units carry their reach.
    let armed: Vec<_> = subjects.iter().filter(|s| s.defense.is_some()).collect();
    assert!(!armed.is_empty(), "the template and layout are defended");
    assert!(armed.iter().all(|s| {
        let d = s.defense.unwrap();
        d.radar || d.missile_reach_ft > 0. || d.gun_reach_ft > 0.
    }));

    // The synthetic types carry no listing flag or signatures; give the
    // sided surface rows a retail ground unit's (flag 0x20, `maxVisDist` 78,
    // full radar and infrared returns), as the import registers them.
    let state = &mut world.combat.state;
    let sided: Vec<u32> = state
        .targets
        .iter()
        .filter(|t| t.role == TargetRole::Surface && t.side != live::NO_SIDE)
        .map(|t| t.id)
        .collect();
    for id in sided {
        state.set_ground_listing(
            id,
            live::GroundListing {
                flags: 0x20,
                max_visible: 78,
                team: false,
            },
        );
        let row = state.targets.iter_mut().find(|t| t.id == id).unwrap();
        row.signature.radar = 100.;
        row.signature.infrared = 100.;
    }
    // A Strike wingman put south of the objective at 5,000 ft perceives
    // ground contacts with its own sensors; all are sided and standing.
    let wingman = ai_of(&world, Wing::Friendly)[0];
    let wings = world.ai_wings.as_mut().unwrap();
    wings.set_strike_role(wingman, StrikeRole::Strike);
    step(&mut world, 1);
    let objective = picture(&world, wingman)
        .unwrap()
        .objective()
        .unwrap()
        .clone();
    let point = objective.point.unwrap();
    let wings = world.ai_wings.as_mut().unwrap();
    let actor = wings.mission_mut().actor_mut(wingman).unwrap();
    assert!(
        actor.sensors().is_some(),
        "an imported AI aircraft has sensors"
    );
    let flight = actor.flight_mut();
    flight.position = [point[0], point[1] + 5_000., point[2] - 15_000.];
    flight.yaw = 0.;
    flight.pitch = 0.;
    flight.bank = 0.;
    // Briefing again rebuilds the picture at the next step, from here.
    assert!(actor.set_strike_objective(objective));
    step(&mut world, 1);
    let picture = picture(&world, wingman).unwrap();
    assert!(
        !picture.contacts().is_empty(),
        "a striker over the target sees it"
    );
    assert!(
        picture
            .contacts()
            .iter()
            .all(|c| c.side != live::NO_SIDE && !c.destroyed)
    );
}
