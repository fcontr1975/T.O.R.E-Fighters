//! Per-plane loadouts of an open mission (slice EF4): the players' choices
//! in the lobby, carried in the spec's text, checked by the Load Ordnance
//! page's rules, flown by the AI until a human takes the plane, and kept by
//! the human. Synthetic resources only.

use super::*;
use crate::{
    mission::{LoadoutSpec, MissionSpec, Skill, Start},
    test_support::resources::{THEATER, resources},
};
use tore_formats::aircraft::AircraftId;

/// Friendly Wing 1 of three and the enemy's Wing 1 of two, airborne.
fn spec() -> MissionSpec {
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.wings[0].count = 3;
    spec.wings[3].count = 2;
    spec.wings[3].skill = Skill::Average;
    spec.separation_nm = 5;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    spec
}

/// The synthetic F/A-18D's standard load: the gun with 500 rounds and two
/// missiles.
fn standard() -> LoadoutSpec {
    let map = resources();
    let player = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
    LoadoutSpec::of(
        &tore_sim::combat::loadout::Loadout::new(&player.profile, |name| {
            map.get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
        })
        .unwrap(),
    )
}

/// One missile and 300 rounds, with half the fuel.
fn light() -> LoadoutSpec {
    let mut load = standard();
    load.fuel_lbs = (load.fuel_lbs / 2.).round();
    load.stations[0].quantity = 300;
    load.stations[1].quantity = 1;
    load
}

fn refused(spec: &MissionSpec, seating: Seating) -> String {
    match World::new(spec, &resources(), seating) {
        Ok(_) => panic!("the mission built"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn plane_loadouts_survive_the_text_form() {
    let mut spec = spec();
    spec.plane_loadouts.insert(1, light());
    spec.plane_loadouts.insert(2, standard());
    let text = spec.to_text();
    assert!(
        text.contains(&format!("plane-loadout 1 fuel {}\n", light().fuel_lbs)),
        "{text}"
    );
    assert!(
        text.contains("plane-loadout 1 station 1 AIM9M.JT 2 1\n"),
        "{text}"
    );
    assert_eq!(MissionSpec::from_text(&text).unwrap(), spec);
}

#[test]
fn a_plane_loadout_line_out_of_place_is_refused_with_its_line() {
    let base = "tore-mission 1\ntheater UKR\nwing friendly 1 F18.PT 2 average\n";
    let error = |extra: &str| {
        MissionSpec::from_text(&format!("{base}{extra}"))
            .unwrap_err()
            .to_string()
    };
    assert_eq!(
        error("plane-loadout 1 station 0 M61.JT 500 500\n"),
        "the loadout has no `plane-loadout 1 fuel` line"
    );
    assert_eq!(
        error("plane-loadout 30 fuel 100\n"),
        "line 4: plane 30 is beyond the mission's planes 0 to 29"
    );
    assert_eq!(
        error("plane-loadout 1 fuel 100\nplane-loadout 1 fuel 200\n"),
        "line 5: `plane-loadout 1 fuel` appears twice"
    );
    assert_eq!(
        error("plane-loadout one fuel 100\n"),
        "line 4: the plane number must be a whole number, not `one`"
    );
}

#[test]
fn the_open_planes_are_the_rosters() {
    let spec = spec();
    let world = World::new(&spec, &resources(), Seating::Open).unwrap();
    let planes = spec.open_planes();
    assert_eq!(planes.len(), world.roster.planes().len());
    for plane in planes {
        let entry = world.roster.plane(PlaneId(plane.id)).unwrap();
        assert_eq!(entry.slot.wing, plane.wing);
        assert_eq!(entry.slot.member, plane.member);
        let slot = world.ai_wings.as_ref().unwrap().slot(plane.id).unwrap();
        assert_eq!(slot.aircraft, plane.aircraft);
    }
    assert_eq!(
        spec.summary(),
        "UKR, clear, airborne at 10000 ft: F/A-18D Hornet x3 against F/A-18D Hornet x2"
    );
}

#[test]
fn a_human_taking_a_loaded_plane_keeps_its_loadout() {
    let mut spec = spec();
    spec.plane_loadouts.insert(1, light());
    let mut world = World::new(&spec, &resources(), Seating::Open).unwrap();
    // The AI flies it with the loadout's fuel until a human takes it.
    let actor = world.ai_wings.as_ref().unwrap().mission().actor(1).unwrap();
    let fuel = light().fuel_lbs;
    assert!(fuel > 0. && fuel < standard().fuel_lbs);
    assert_eq!(actor.flight().fuel, fuel);
    world.take_plane(SeatId(0), PlaneId(1)).unwrap();
    let own = world.combat.state.ownship(1).unwrap();
    assert_eq!(own.ammo, [300, 1]);
    assert_eq!(world.cockpits[0].flight.fuel, fuel);
    // A plane with no loadout carries the standard load.
    world.take_plane(SeatId(1), PlaneId(2)).unwrap();
    assert_eq!(world.combat.state.ownship(2).unwrap().ammo, [500, 2]);
}

#[test]
fn a_loadout_the_page_would_not_allow_is_refused_with_its_reason() {
    let map = resources();
    let player = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
    let check = |load: &LoadoutSpec, guns_only: bool| {
        load.check_for_plane(&player.profile, &map, guns_only)
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    assert_eq!(check(&light(), false), Ok(()));
    let mut over = standard();
    over.stations[1].quantity = 3;
    assert_eq!(
        check(&over, false),
        Err("Station quantity exceeds capacity.".into())
    );
    let mut cheat = standard();
    cheat.cheat = true;
    assert_eq!(
        check(&cheat, false),
        Err("Cheat loading is not allowed in a multiplayer game.".into())
    );
    let mut stretched = standard();
    stretched.stations[0].count = 5_000;
    stretched.stations[0].quantity = 5_000;
    assert_eq!(
        check(&stretched, false),
        Err("Station 1 cannot hold 5000 M61.JT.".into())
    );
    let mut short = standard();
    short.stations.pop();
    assert!(check(&short, false).unwrap_err().contains("stations"));
    let mut tanks = standard();
    tanks.fuel_lbs = 1e9;
    assert!(check(&tanks, false).is_err());
    assert_eq!(
        check(&standard(), true),
        Err("Guns only is selected. Unload other weapons or return to setup and change the restriction.".into())
    );
    let mut guns = standard();
    guns.stations[1].quantity = 0;
    assert_eq!(check(&guns, true), Ok(()));
}

#[test]
fn a_migrated_single_is_kept_for_editing_but_noninteractive_combat_refuses_overweight() {
    use tore_sim::combat::loadout::Loadout;
    let map = resources();
    let mut player = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
    // Turn the synthetic missile point into paired fuselage carriage.
    let point = &mut player.profile.hardpoints[4];
    point.location = 1;
    point.flags |= 4;
    point.position[0] = 12;
    let mut base = Loadout::new(&player.profile, |name| Ok(map[name].clone())).unwrap();
    let slot = base.configuration.hardpoint_slots[4].unwrap();
    base.quantities[slot] = 1;
    let weight = f64::from(base.configuration.stations[slot].weapon.weight);
    assert!(weight > 0.);
    base.maximum_lbs = base.total_lbs() + weight / 2.;
    let saved = LoadoutSpec::of(&base);
    let migrated = saved.apply(base, &map, None).unwrap();
    assert_eq!(migrated.quantities[slot], 2);
    migrated.validate_for_editing().unwrap();
    let error = match crate::combat::Combat::with_loadout(&player, &migrated) {
        Ok(_) => panic!("overweight noninteractive start succeeded"),
        Err(error) => error.to_string(),
    };
    assert_eq!(
        error,
        "This plane is too heavy to take off. Remove stores or fuel."
    );
}

#[test]
fn a_lobby_request_names_pairing_only_when_an_odd_paired_count_is_the_cause() {
    let map = resources();
    let mut player = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
    // The synthetic missile point as paired fuselage carriage.
    let point = &mut player.profile.hardpoints[4];
    point.location = 1;
    point.flags |= 4;
    point.position[0] = 12;
    let base = tore_sim::combat::loadout::Loadout::new(&player.profile, |name| {
        map.get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
    })
    .unwrap();
    let slot = base.configuration.hardpoint_slots[4].unwrap();
    assert_eq!(base.store_pairs(slot).step(1), 2);
    let check = |load: &LoadoutSpec| {
        load.check_for_plane(&player.profile, &map, false)
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    let standard = LoadoutSpec::of(&base);
    assert_eq!(check(&standard), Ok(()));
    let mut single = standard.clone();
    single.stations[slot].quantity = 1;
    assert_eq!(
        check(&single),
        Err("Paired stores must be loaded in pairs.".into())
    );
    let mut odd_layout = standard.clone();
    odd_layout.stations[slot].count = 3;
    assert_eq!(
        check(&odd_layout),
        Err("Paired stores must be loaded in pairs.".into())
    );
    // An even count the point cannot take is not a pairing problem.
    let mut wide = standard.clone();
    wide.stations[slot].count = 2_000;
    assert_eq!(
        check(&wide),
        Err("Loadout capacity must match the current loadout.".into())
    );
}

#[test]
fn plane_loadouts_are_refused_where_they_do_not_belong() {
    let mut spec = spec();
    spec.plane_loadouts.insert(1, light());
    assert!(refused(&spec, Seating::SinglePlayer).contains("multiplayer"));
    let mut missing = self::spec();
    missing.plane_loadouts.insert(9, light());
    assert_eq!(
        refused(&missing, Seating::Open),
        "The mission has no plane 9 to load."
    );
    let mut over = self::spec();
    let mut load = standard();
    load.stations[1].quantity = 3;
    over.plane_loadouts.insert(0, load);
    assert_eq!(
        refused(&over, Seating::Open),
        "Plane 0's loadout: Station quantity exceeds capacity."
    );
}

#[test]
fn another_missile_the_station_takes_is_a_loadout_the_standard_load_never_reads() {
    let mut map = resources();
    map.insert(
        "AIM9X.JT".into(),
        crate::test_support::resources::loadable_missile("AIM9X.JT"),
    );
    let player = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
    let mut load = standard();
    let standard_load = tore_sim::combat::loadout::Loadout::new(&player.profile, |name| {
        map.get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
    })
    .unwrap();
    let missile = tore_formats::weapons::Weapon::parse("AIM9X.JT", &map["AIM9X.JT"]).unwrap();
    let capacity = standard_load.capacity(1, &missile);
    assert!(capacity > 0, "the station takes it");
    load.stations[1].weapon = "AIM9X.JT".into();
    load.stations[1].count = capacity as u16;
    load.stations[1].quantity = capacity as u16;
    load.check_for_plane(&player.profile, &map, false).unwrap();
    let mut spec = spec();
    spec.plane_loadouts.insert(1, load);
    let reads = crate::resources::ResourceReads::new(&map);
    let world = World::new(&spec, &reads, Seating::Open).unwrap();
    assert!(
        reads
            .manifest()
            .entries
            .iter()
            .any(|e| e.name == "AIM9X.JT")
    );
    let wings = world.ai_wings.as_ref().unwrap();
    assert_eq!(
        wings.configuration(1).unwrap().stations[1].weapon.source,
        "AIM9X.JT"
    );
}

#[test]
fn qmc_wing_loadout_copies_removed_stores_fuel_and_cheat_and_restarts() {
    for ground in [false, true] {
        for cheat in [false, true] {
            let mut spec = spec();
            spec.wings[0].count = 4;
            spec.wings[1].count = 1;
            spec.share_flight_loadout = true;
            let mut chosen = light();
            chosen.stations[1].quantity = 0;
            chosen.cheat = cheat;
            spec.loadout = Some(chosen.clone());
            if ground {
                spec.start = Start::GroundAuto {
                    altitude_ft: 10_000,
                };
            }
            assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
            let map = if ground {
                crate::test_support::resources::airport_resources()
            } else {
                resources()
            };
            let kind = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
            let mut world = World::new(&spec, &map, Seating::SinglePlayer).unwrap();
            for _ in 0..2 {
                let wings = world.ai_wings.as_ref().unwrap();
                for id in 1..4 {
                    let load = &world.setup.ai.as_ref().unwrap().loadouts[&id];
                    assert_eq!(LoadoutSpec::of(load), chosen);
                    let actor = wings.mission().actor(id).unwrap();
                    assert_eq!(actor.flight().fuel, chosen.fuel_lbs);
                    assert_eq!(
                        actor.stations()[0].store.rounds,
                        tore_sim::ai::weapon_service::Rounds::Finite(300)
                    );
                    assert_eq!(
                        actor.stations()[1].store.rounds,
                        tore_sim::ai::weapon_service::Rounds::Finite(0)
                    );
                    assert_eq!(
                        wings.configuration(id).unwrap().stations[1].weapon.source,
                        chosen.stations[1].weapon
                    );
                }
                assert!(!world.setup.ai.as_ref().unwrap().loadouts.contains_key(&4));
                assert!(!world.setup.ai.as_ref().unwrap().loadouts.contains_key(&5));
                assert_eq!(
                    wings.mission().actor(4).unwrap().flight().fuel,
                    standard().fuel_lbs
                );
                let spent = world
                    .ai_wings
                    .as_mut()
                    .unwrap()
                    .mission_mut()
                    .actor_mut(1)
                    .unwrap();
                spent.flight_mut().fuel = 1.;
                spent.stations_mut()[0].store.rounds =
                    tore_sim::ai::weapon_service::Rounds::Finite(0);
                world.restart(&kind, &map).unwrap();
            }
        }
    }
}

#[test]
fn authored_single_player_load_does_not_change_wingmen() {
    let mut spec = spec();
    spec.loadout = Some(light());
    let world = World::new(&spec, &resources(), Seating::SinglePlayer).unwrap();
    assert!(world.setup.ai.as_ref().unwrap().loadouts.is_empty());
    assert_eq!(
        world
            .ai_wings
            .as_ref()
            .unwrap()
            .mission()
            .actor(1)
            .unwrap()
            .flight()
            .fuel,
        standard().fuel_lbs
    );
}

#[test]
fn qmc_carries_a_store_even_without_ai_target_capabilities() {
    // This synthetic missile has no air/surface target flags. It remains
    // a valid carried store even though neither AI selector can employ it.
    let mut spec = spec();
    spec.share_flight_loadout = true;
    spec.loadout = Some(light());
    let world = World::new(&spec, &resources(), Seating::SinglePlayer).unwrap();
    let station = &world
        .ai_wings
        .as_ref()
        .unwrap()
        .mission()
        .actor(1)
        .unwrap()
        .stations()[1];
    assert!(!station.capability.air && !station.capability.surface);
    assert_eq!(
        station.store.rounds,
        tore_sim::ai::weapon_service::Rounds::Finite(1)
    );
}
