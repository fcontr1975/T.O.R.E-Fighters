//! Building a mission from a [`MissionSpec`] and the synthetic import, headless.

use super::*;
use crate::{
    mission::{MissionSpec, Skill, Start},
    resources::ResourceReads,
    seats::SeatInput,
    test_support::resources::{THEATER, resources},
};
use tore_formats::aircraft::AircraftId;

/// A small mission in the synthetic theater: the player's flight of two
/// against an enemy flight of two.
fn spec() -> MissionSpec {
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.wings[0].count = 2;
    spec.wings[3].count = 2;
    spec.wings[3].skill = Skill::Average;
    spec.separation_nm = 2;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    spec
}

fn step(world: &mut World, ticks: usize) {
    let mut out = TickOutput::default();
    for _ in 0..ticks {
        let input = SeatInput {
            tick: world.tick(),
            ..SeatInput::default()
        };
        world.step(&[input], &mut out).unwrap();
    }
}

#[test]
fn a_single_player_mission_builds_steps_and_restarts_from_its_spec() {
    let map = resources();
    let built = World::build(&spec(), &map, Seating::SinglePlayer, &mut Hooks::default()).unwrap();
    let mut world = built.world;
    // The player's flight, one of its two aircraft flown by a human, and the
    // enemy's two: four planes, the AI flying three.
    assert_eq!(world.roster.seats().len(), 1);
    assert_eq!(world.cockpits.len(), 1);
    assert_eq!(built.restarted.ai_aircraft, Some(3));
    assert_eq!(
        world.setup.mission,
        Some((10_000., world.setup.mission.unwrap().1))
    );
    assert!(world.ai_wings.is_some());
    assert_eq!(world.tick(), 0);
    let start = world.cockpits[0].flight.position;
    assert!((world.cockpits[0].flight.position[1] - 10_000.).abs() < 1e-6);

    step(&mut world, 240);
    assert_eq!(world.tick(), 240);
    assert_ne!(world.cockpits[0].flight.position, start);

    // A restart puts the mission back where the build left it.
    let again = World::new(&spec(), &map, Seating::SinglePlayer).unwrap();
    let player = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
    world.restart(&player, &map).unwrap();
    assert_eq!(world.tick(), 0);
    assert_eq!(world.cockpits[0].flight.position, start);
    assert_eq!(again.cockpits[0].flight.position, start);
    step(&mut world, 10);
}

#[test]
fn the_manifest_lists_the_resources_the_build_read() {
    let map = resources();
    let reads = ResourceReads::new(&map);
    World::new(&spec(), &reads, Seating::SinglePlayer).unwrap();
    let manifest = reads.manifest();
    let names: Vec<&str> = manifest.entries.iter().map(|e| e.name.as_str()).collect();
    // The theater, the weather module of the condition, the aircraft with its
    // sensors, jammer and weapons.
    for name in [
        "UKR.MM", "UKR.T2", "DAY2.LAY", "F18.PT", "F18R.SEE", "F18V.SEE", "F18.ECM", "M61.JT",
    ] {
        assert!(names.contains(&name), "{name} was not read: {names:?}");
    }
    assert!(
        names.windows(2).all(|pair| pair[0] < pair[1]),
        "sorted: {names:?}"
    );
    // A resource the build never asked for is not in it.
    assert!(!names.contains(&"CLOUD1.LAY"));
    // Every entry the import has carries the hash of its bytes.
    for entry in &manifest.entries {
        assert_eq!(
            entry.hash,
            map.get(&entry.name)
                .map(|bytes| tore_codec::hash::fnv1a64(bytes))
        );
    }
    // The same build reads the same names.
    let again = ResourceReads::new(&map);
    World::new(&spec(), &again, Seating::SinglePlayer).unwrap();
    assert_eq!(manifest, again.manifest());
}

fn error_of(spec: &MissionSpec, map: &std::collections::BTreeMap<String, Vec<u8>>) -> String {
    match World::new(spec, map, Seating::SinglePlayer) {
        Ok(_) => panic!("the mission built"),
        Err(error) => error.to_string(),
    }
}

/// The standard load of the synthetic F/A-18D.
fn standard(
    map: &std::collections::BTreeMap<String, Vec<u8>>,
) -> tore_sim::combat::loadout::Loadout {
    let player = aircraft_type::AircraftType::load(map, AircraftId::F18).unwrap();
    tore_sim::combat::loadout::Loadout::new(&player.profile, |name| {
        map.get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
    })
    .unwrap()
}

#[test]
fn the_spec_and_its_text_build_the_same_mission() {
    let map = resources();
    let mut spec = spec();
    spec.weather.time = Some([6, 30]);
    spec.weather.wind = Some([90, 30]);
    let parsed = MissionSpec::from_text(&spec.to_text()).unwrap();
    assert_eq!(parsed, spec);
    let (mut a, mut b) = (
        World::new(&spec, &map, Seating::SinglePlayer).unwrap(),
        World::new(&parsed, &map, Seating::SinglePlayer).unwrap(),
    );
    step(&mut a, 600);
    step(&mut b, 600);
    assert_eq!(a.cockpits[0].flight.position, b.cockpits[0].flight.position);
    assert_eq!(
        a.terrain.weather.seconds_of_day(),
        b.terrain.weather.seconds_of_day()
    );
    // The overrides reached the terrain.
    assert_eq!(a.terrain.environment.time, Some([6, 30]));
    let plain = World::new(&self::spec(), &map, Seating::SinglePlayer).unwrap();
    assert_ne!(a.terrain.wind(), plain.terrain.wind());
    assert_eq!(a.terrain.wind(), b.terrain.wind());
}

#[test]
fn the_condition_picks_the_weather_layer_and_the_hour() {
    let map = resources();
    let mut night = spec();
    night.condition = crate::mission::Condition::Night;
    let world = World::new(&night, &map, Seating::SinglePlayer).unwrap();
    assert_eq!(world.terrain.condition, Some(5));
    let seconds = tore_sim::environment::CONDITIONS[5].seconds_of_day;
    assert_eq!(
        world.terrain.environment.time,
        Some([seconds / 3600, seconds / 60 % 60])
    );
}

#[test]
fn a_loadout_in_the_spec_is_the_players_load() {
    let map = resources();
    let mut load = crate::mission::LoadoutSpec::of(&standard(&map));
    assert_eq!(load.stations.len(), 2);
    load.fuel_lbs = 500.;
    load.stations[1].quantity = 1;
    let mut spec = spec();
    spec.loadout = Some(load.clone());
    // It survives the text form.
    assert_eq!(
        MissionSpec::from_text(&spec.to_text()).unwrap().loadout,
        Some(load)
    );
    let world = World::new(&spec, &map, Seating::SinglePlayer).unwrap();
    assert_eq!(world.cockpits[0].flight.fuel, 500.);
    assert_eq!(world.setup.mission.unwrap().1, 500.);
    assert_eq!(world.combat.state.own().ammo, [500, 1]);
    // Without one, the aircraft's standard load.
    let standard = World::new(&self::spec(), &map, Seating::SinglePlayer).unwrap();
    assert_eq!(standard.combat.state.own().ammo, [500, 2]);
}

#[test]
fn a_loadout_that_does_not_fit_the_aircraft_is_refused() {
    let map = resources();
    let mut load = crate::mission::LoadoutSpec::of(&standard(&map));
    load.stations.pop();
    let mut spec = spec();
    spec.loadout = Some(load);
    assert!(error_of(&spec, &map).contains("stations"));
    let mut load = crate::mission::LoadoutSpec::of(&standard(&map));
    load.stations[1].quantity = 3;
    spec.loadout = Some(load);
    assert!(error_of(&spec, &map).contains("exceeds capacity"));
}

#[test]
fn guns_only_unloads_the_standard_missiles_and_refuses_a_load_that_carries_them() {
    let map = resources();
    let mut spec = spec();
    spec.guns_only = true;
    let world = World::new(&spec, &map, Seating::SinglePlayer).unwrap();
    assert_eq!(world.combat.state.own().ammo, [500, 0]);
    // A load that still carries a missile does not meet the setting.
    spec.loadout = Some(crate::mission::LoadoutSpec::of(&standard(&map)));
    assert_eq!(
        error_of(&spec, &map),
        "Guns only is selected. Unload other weapons or return to setup and change the restriction."
    );
}

#[test]
fn a_ground_start_needs_the_hybrid_flight_model() {
    let map = resources();
    let mut spec = spec();
    spec.start = Start::Ground {
        runway: crate::mission::RUNWAY_OBJECT_BASE,
        altitude_ft: 5_000,
    };
    spec.researched_flight = false;
    assert_eq!(
        error_of(&spec, &map),
        "Ground start requires the researched flight model. Choose Airborne for this adapter."
    );
}

#[test]
fn an_altitude_under_the_terrain_is_refused_with_the_creators_message() {
    let mut map = resources();
    // Raise the whole land to 10,240 feet.
    for cell in map.get_mut("UKR.T2").unwrap()[149..]
        .chunks_exact_mut(3)
        .take(128 * 128)
    {
        cell[2] = 40;
    }
    assert_eq!(
        error_of(&spec(), &map),
        "Airborne altitude must exceed 10340 feet here. Choose a higher altitude."
    );
}

#[test]
fn fixture_wings_fly_straight_with_no_ai() {
    let map = resources();
    let mut spec = spec();
    spec.fixture_wings = true;
    let built = World::build(&spec, &map, Seating::SinglePlayer, &mut Hooks::default()).unwrap();
    let mut world = built.world;
    assert!(world.ai_wings.is_none());
    assert!(world.setup.ai.is_none());
    assert_eq!(built.restarted.ai_aircraft, None);
    assert!(world.combat.mission_layout.is_some());
    step(&mut world, 120);
}

#[test]
fn cheats_in_the_spec_are_in_force_from_the_first_tick() {
    let map = resources();
    let mut spec = spec();
    spec.cheats.unlimited_ammo = true;
    spec.cheats.no_spins = true;
    let world = World::new(&spec, &map, Seating::SinglePlayer).unwrap();
    assert!(world.cockpits[0].flight.cheats.unlimited_ammo);
    assert!(world.combat.state.cheats.no_spins);
    // No cheats sends nothing, so what the build set stands.
    let world = World::new(&self::spec(), &map, Seating::SinglePlayer).unwrap();
    assert_eq!(
        world.combat.state.cheats,
        tore_sim::cheats::Cheats::default()
    );
}

#[test]
fn the_callers_hooks_supply_the_types_and_the_weapon_labels() {
    let map = resources();
    let mut loaded = Vec::new();
    let mut load = |id: AircraftId| {
        loaded.push(id);
        crate::aircraft_type::load_type(&map, id)
    };
    let label = |weapon: &mut tore_formats::weapons::Weapon| weapon.name = "Tidy".into();
    let player = crate::aircraft_type::load_type(&map, AircraftId::F18).unwrap();
    let built = World::build(
        &spec(),
        &map,
        Seating::SinglePlayer,
        &mut Hooks {
            player: Some(std::sync::Arc::clone(&player)),
            load: Some(&mut load),
            weapon_label: Some(&label),
            ground_variation: None,
            inspect_invalid_placement: false,
        },
    )
    .unwrap();
    // Friendly and enemy wings fly the one type: combat asks once.
    assert_eq!(loaded, [AircraftId::F18]);
    let stations = &built.world.combat.state.own().configuration().stations;
    assert!(stations.iter().all(|s| s.weapon.name == "Tidy"));
}

/// A variety aircraft's airborne start speed is chosen at the mission's
/// altitude, not at the free-flight default the type starts at; a ported
/// fighter keeps its fixed start speed.
#[test]
fn an_airborne_start_takes_the_variety_speed_at_the_mission_altitude() {
    use tore_sim::models::{AircraftModel, variety::VarietyFlightModel};
    let map = resources();
    let mut world = World::new(&spec(), &map, Seating::SinglePlayer).unwrap();
    let fighter = aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
    world.restart(&fighter, &map).unwrap();
    assert_eq!(world.cockpits[0].flight.speed, 450. * 1.68781);

    let mut profile = tore_formats::aircraft::Aircraft::parse(
        crate::resources::ResourceSource::get(&map, "F18.PT").unwrap(),
    )
    .unwrap();
    let (name, shape) = VarietyFlightModel::identity(AircraftId::F15).unwrap();
    profile.id = AircraftId::F15;
    profile.name = name.into();
    profile.shape = shape.into();
    let model = AircraftModel::for_aircraft(&profile).unwrap();
    let variety = aircraft_type::AircraftType::new(
        profile,
        model.clone(),
        fighter.sensors.clone(),
        Vec::new(),
    );
    let free_flight = variety.start(&world.terrain).speed;
    let at_mission = tore_sim::flight::State::from_model(model, [0., 10_000., 0.]).speed;
    assert_ne!(free_flight, at_mission, "the altitudes must matter here");
    world.restart(&variety, &map).unwrap();
    let flight = &world.cockpits[0].flight;
    assert_eq!(flight.speed, at_mission);
    assert!((flight.position[1] - 10_000.).abs() < 1e-6);
    // The velocity follows the speed and the wind, ground-relative.
    let wind = world.terrain.wind();
    let ground: f64 = (0..3)
        .map(|i| (flight.velocity[i] - wind[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    assert!((ground - at_mission).abs() < 1e-6, "{ground} {at_mission}");
}

#[test]
fn a_ground_target_missing_from_the_import_flies_without_it_and_says_why() {
    use crate::mission::Defense;
    let map = resources();
    let plain = World::new(&spec(), &map, Seating::SinglePlayer).unwrap();
    assert!(plain.terrain.surface.unresolved.is_none());
    let mut with = spec();
    with.ground_target = Some("QUCOL".into());
    with.aaa = Defense::Heavy;
    with.sam = Defense::Moderate;
    with.surface_seed = 77;
    // The synthetic import keeps no templates, as imports made before the
    // surface round do not: nothing stands at the target.
    let world = World::new(&with, &map, Seating::SinglePlayer).unwrap();
    let why = world.terrain.surface.unresolved.as_deref().unwrap();
    assert!(why.contains("~QUCOL.M"), "{why}");
    assert!(world.terrain.surface.template.is_none());
    assert_eq!(
        world.combat.state.targets.len(),
        plain.combat.state.targets.len()
    );
}

#[test]
fn an_airport_takes_its_runways_layout_side() {
    use crate::test_support::resources::owned_airport_resources;
    use tore_sim::airport::Allegiance;
    // Bit 0x80 of the owner is Redfor; no owner field is neutral, and a
    // neutral field grants both sides permission (slice AL1).
    for (owner, allegiance) in [
        (Some(12), Allegiance::Friendly),
        (Some(137), Allegiance::Hostile),
        (None, Allegiance::Neutral),
    ] {
        let world = World::new(
            &spec(),
            &owned_airport_resources(owner),
            Seating::SinglePlayer,
        )
        .unwrap();
        let airport = &world.terrain.airport_scene.airports[0];
        assert_eq!(airport.allegiance, allegiance, "{owner:?}");
        assert!(airport.neutral_permission);
    }
}

#[test]
fn a_theater_with_no_field_of_the_side_has_no_ground_start_or_home_for_it() {
    use crate::test_support::resources::{AIRPORT_RUNWAY, owned_airport_resources};
    use tore_sim::ai::launch::Side;
    // The one runway is Redfor's.
    let map = owned_airport_resources(Some(137));
    let mut auto = spec();
    auto.start = Start::GroundAuto {
        altitude_ft: 10_000,
    };
    assert!(error_of(&auto, &map).contains("No runway of your side"));
    let world = World::new(&spec(), &map, Seating::SinglePlayer).unwrap();
    assert_eq!(
        crate::mission_layout::auto_runway_for(&world.terrain, 1, true).unwrap(),
        AIRPORT_RUNWAY
    );
    // Blue's AI keeps its start point as home; Redfor's goes to the field.
    let fields = crate::ai_wings::Airfields::from_world(&world.terrain, None);
    assert_eq!(fields.home([0.; 3], Side::Friendly), None);
    assert_eq!(
        fields.home([0.; 3], Side::Enemy).map(|r| r.object),
        Some(AIRPORT_RUNWAY)
    );
}

#[test]
fn a_ground_start_on_auto_takes_a_runway_the_world_picks() {
    use crate::test_support::resources::{AIRPORT_RUNWAY, airport_resources};
    let map = airport_resources();
    let mut auto = spec();
    auto.start = Start::GroundAuto {
        altitude_ft: 10_000,
    };
    let mut named = spec();
    named.start = Start::Ground {
        runway: AIRPORT_RUNWAY,
        altitude_ft: 10_000,
    };
    let auto = World::new(&auto, &map, Seating::SinglePlayer).unwrap();
    let named = World::new(&named, &map, Seating::SinglePlayer).unwrap();
    // The synthetic theater has one runway: both park on it.
    assert_eq!(
        auto.cockpits[0].flight.position,
        named.cockpits[0].flight.position
    );
    // No runway at all is a plain refusal, not a panic.
    let mut none = spec();
    none.start = Start::GroundAuto { altitude_ft: 5_000 };
    assert!(error_of(&none, &resources()).contains("No runway"));
}

#[test]
fn an_enemy_ground_start_parks_enemy_wing_one_at_a_redfor_field() {
    use crate::mission::EnemyStart;
    use crate::seats::{PlaneId, SeatId};
    use crate::test_support::resources::{AIRPORT_RUNWAY, owned_airport_resources};
    use tore_sim::ai::launch::Side;
    let red = owned_airport_resources(Some(137));
    let mut auto = spec();
    auto.enemy_start = EnemyStart::GroundAuto;
    let mut named = spec();
    named.enemy_start = EnemyStart::Ground {
        runway: AIRPORT_RUNWAY,
    };
    for spec in [&auto, &named] {
        let mut world = World::new(spec, &red, Seating::SinglePlayer).unwrap();
        assert_eq!(world.setup.enemy_ground_start, Some(AIRPORT_RUNWAY));
        let wings = world.ai_wings.as_ref().unwrap();
        let parked: Vec<_> = wings
            .mission()
            .actors()
            .iter()
            .filter(|actor| actor.ground_start().is_some())
            .map(|actor| (actor.identity().side, actor.identity().wing))
            .collect();
        assert_eq!(parked.len(), 2, "{parked:?}");
        assert!(
            parked
                .iter()
                .all(|(side, wing)| *side == crate::ai_wings::ENEMY_SIDE && *wing == 0)
        );
        // The player's side flies as before.
        assert!((world.cockpits[0].flight.position[1] - 10_000.).abs() < 1e-6);
        step(&mut world, 120);
    }
    // A Blue field is the other side's: refused by name, and auto finds none.
    let blue = owned_airport_resources(Some(12));
    assert!(error_of(&named, &blue).contains("is a Blue airfield"));
    assert!(error_of(&auto, &blue).contains("No Redfor runway"));
    // Online a Redfor human takes the parked lead, and its home is the field.
    let mut open = auto.clone();
    open.wings[0].count = 1;
    let mut world = World::new(&open, &red, Seating::Open).unwrap();
    let lead = world
        .roster
        .planes()
        .iter()
        .find(|plane| plane.slot.wing.side == Side::Enemy && plane.slot.member == 0)
        .map(|plane| plane.id)
        .unwrap();
    world.take_plane(SeatId(0), lead).unwrap();
    let cockpit = &world.cockpits[0];
    assert_eq!(cockpit.plane, lead);
    assert!(
        cockpit
            .flight
            .research
            .as_ref()
            .is_some_and(|r| r.on_ground)
    );
    assert!(cockpit.result.home_base().is_some());
    assert!(world.roster.redfor(lead) && lead != PlaneId(0));
    // As a single-player ground start begins: on NAV, the field selected.
    assert!(cockpit.airport_nav_mode);
    assert!(cockpit.airport_service.selected().is_some());
}

#[test]
fn creator_distance_audit_matches_launch_and_never_silently_shortens() {
    let data = resources();
    let mut mission = spec();
    let choices = crate::mission_layout::availability::audit(&data, &mission).unwrap();
    assert!(choices.separations[1], "two nm fits the synthetic map");
    assert!(!choices.separations[10], "300 nm cannot fit");
    for (index, nm) in crate::mission_layout::SEPARATION_NM.into_iter().enumerate() {
        mission.separation_nm = nm as u32;
        match World::build(
            &mission,
            &data,
            Seating::SinglePlayer,
            &mut Hooks::default(),
        ) {
            Ok(built) => {
                assert!(
                    choices.separations[index],
                    "{nm} nm launched but was hidden"
                );
                assert!(!built.restarted.layout.as_ref().unwrap().enemy.shortened());
            }
            Err(e) => {
                assert!(
                    !choices.separations[index],
                    "{nm} nm offered but refused: {e}"
                );
                assert!(e.to_string().starts_with("Separation "), "{e}");
            }
        }
    }
}

#[test]
fn ground_starts_offer_every_distance_and_target_launches_accept_them() {
    use crate::test_support::surface::{spec_with_target, surface_resources, target};
    let mut data = surface_resources();
    data.extend(crate::test_support::resources::airport_resources());
    let mut mission = spec_with_target(&target("QUCITY", 0, 0, 7));
    mission.start = Start::GroundAuto { altitude_ft: 5000 };
    let first = World::new(&mission, &data, Seating::SinglePlayer).unwrap();
    let runway = first.setup.ground_start.unwrap();
    for start in [
        Start::GroundAuto { altitude_ft: 5000 },
        Start::Ground {
            runway,
            altitude_ft: 5000,
        },
    ] {
        mission.start = start;
        let choices = crate::mission_layout::availability::audit(&data, &mission).unwrap();
        assert!(choices.separations.into_iter().all(|fits| fits));
        for nm in crate::mission_layout::SEPARATION_NM {
            mission.separation_nm = nm as u32;
            World::new(&mission, &data, Seating::SinglePlayer).unwrap();
        }
    }
    mission.start = Start::Airborne { altitude_ft: 5000 };
    assert!(
        !crate::mission_layout::availability::audit(&data, &mission)
            .unwrap()
            .separations[10]
    );
}

#[test]
fn ground_start_without_target_still_requires_the_requested_distance_to_fit() {
    let data = crate::test_support::resources::airport_resources();
    let mut mission = spec();
    mission.start = Start::GroundAuto { altitude_ft: 5000 };
    mission.separation_nm = 300;
    let choices = crate::mission_layout::availability::audit(&data, &mission).unwrap();
    assert!(!choices.separations[10]);
    let error = World::new(&mission, &data, Seating::SinglePlayer)
        .err()
        .unwrap();
    assert!(error.to_string().starts_with("Separation "));
}
