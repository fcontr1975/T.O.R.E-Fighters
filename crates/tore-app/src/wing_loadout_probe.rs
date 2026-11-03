//! Windowless QMC accepted-load regression and start-state recording.
use crate::{AppResult, Assets, quick_mission::QuickMission, replay};
use std::path::Path;
use tore_formats::{aircraft::AircraftId, weapons::Weapon};
use tore_sim::combat::loadout::Loadout;
use tore_world::{
    aircraft_type::AircraftType,
    mission::{LoadoutSpec, Start},
    world::{Seating, World},
};

pub fn run(assets: &Assets, out: &Path) -> AppResult<()> {
    std::fs::create_dir_all(out)?;
    let data = &assets.theater_resources;
    let kind = AircraftType::load(data, AircraftId::F16C)?;
    let mut quick = QuickMission::new(AircraftId::F16C, assets.creator_options.clone(), data);
    quick.draft.values[4] = 4;
    for field in [7, 10, 21, 24, 27] {
        quick.draft.values[field] = 0;
    }
    let mut spec = quick.mission_spec()?;
    spec.share_flight_loadout = true;
    spec.theater = "UKR".into();
    // The desktop QMC sets the same AI adapter in App::build_mission.
    spec.ai_flight_model = tore_world::ai_wings::AiFlightModel::Standard;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    let mut load = Loadout::new(&kind.profile, |name| {
        data.get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(name.to_owned()))
    })?;
    let standard = load.clone();
    load.clear_tanks()?;
    for (station, quantity) in load.configuration.stations.iter().zip(&mut load.quantities) {
        if !kind.profile.id.mounts_gun(&station.weapon.source)
            && !station.weapon.source.starts_with("AIM9")
        {
            *quantity = 0;
        }
    }
    load.fuel_lbs = (load.internal_capacity_lbs * 0.6).floor();
    let bomb = Weapon::parse("MK82.JT", &data["MK82.JT"])?;
    let slot = (0..load.quantities.len())
        .find(|slot| load.capacity(*slot, &bomb) > 0)
        .ok_or("no Mk82 station")?;
    load.select(slot, bomb)?;
    load.quantities[slot] = load.quantities[slot].min(2);
    load.validate()?;
    spec.loadout = Some(LoadoutSpec::of(&load));
    // Same QMC and accepted stores, first with the old default-wing policy,
    // then with the creator's current policy. Both are recorded at tick zero.
    for before in [true, false] {
        let label = if before { "before" } else { "after" };
        let mut mission = spec.clone();
        if before {
            mission.share_flight_loadout = false;
        }
        std::fs::write(out.join(format!("{label}.mission")), mission.to_text())?;
        let mut world = World::new(&mission, data, Seating::SinglePlayer)?;
        dump(&world, label);
        record(&mut world, &out.join(format!("{label}.toreplay")))?;
        if !before {
            verify(&world, &load)?;
            world.restart(&kind, data)?;
            verify(&world, &load)?;
            mission.start = Start::GroundAuto {
                altitude_ft: 10_000,
            };
            let mut ground = World::new(&mission, data, Seating::SinglePlayer)?;
            verify(&ground, &load)?;
            ground.restart(&kind, data)?;
            verify(&ground, &load)?;
            record(&mut ground, &out.join("ground.toreplay"))?;
        }
    }
    // Also prove a positive tank quantity, and cheat-only carriage, survive.
    if let Some(tank) = standard.configuration.tanks.iter().find(|t| t.quantity > 0) {
        let mut with_tank = load.clone();
        let slot = with_tank
            .configuration
            .tanks
            .iter()
            .position(|t| t.hardpoint == tank.hardpoint)
            .ok_or("tank hardpoint missing")?;
        with_tank.select_tank(slot, tank.store.clone().ok_or("tank store missing")?)?;
        with_tank.configuration.tanks[slot].quantity = 1;
        with_tank.refresh_equipment()?;
        with_tank.validate()?;
        spec.loadout = Some(LoadoutSpec::of(&with_tank));
        let mut world = World::new(&spec, data, Seating::SinglePlayer)?;
        verify(&world, &with_tank)?;
        world.restart(&kind, data)?;
        verify(&world, &with_tank)?;
        println!("qmc wing loadout: one tank matches");
    }
    // The wingtip AIM-120 (one a tip) is an accepted normal load: the flight
    // leader's wingmen carry it, at start and after a restart, and the
    // multiplayer lobby's check takes it.
    let mut tips = load.clone();
    let tip = tips.configuration.hardpoint_slots[tore_formats::aircraft::stations::F16_TIP]
        .ok_or("no wingtip station")?;
    tips.select(tip, Weapon::parse("AIM120.JT", &data["AIM120.JT"])?)?;
    if tips.quantities[tip] != 2 {
        return Err("wingtip AIM-120 did not load one a tip".into());
    }
    tips.validate()?;
    spec.loadout = Some(LoadoutSpec::of(&tips));
    let mut world = World::new(&spec, data, Seating::SinglePlayer)?;
    verify(&world, &tips)?;
    world.restart(&kind, data)?;
    verify(&world, &tips)?;
    LoadoutSpec::of(&tips).check_for_plane(&kind.profile, data, false)?;
    println!("qmc wing loadout: wingtip AIM-120 matches, restarts and passes the lobby check");
    let mut cheat = load.clone();
    cheat.cheat = true;
    let novelty = Weapon::parse("~BLDR.JT", data.get("~BLDR.JT").ok_or("missing Boulder")?)?;
    let slot = (0..cheat.quantities.len())
        .find(|s| cheat.capacity(*s, &novelty) > 0)
        .ok_or("no cheat station")?;
    cheat.select(slot, novelty)?;
    cheat.quantities[slot] = cheat.store_pairs(slot).step(1) as u16;
    cheat.validate()?;
    spec.loadout = Some(LoadoutSpec::of(&cheat));
    let world = World::new(&spec, data, Seating::SinglePlayer)?;
    verify(&world, &cheat)?;
    println!("qmc wing loadout: cheat Boulder carriage matches");
    println!("qmc wing loadout: four F16 aircraft match, airborne, ground and restart passed");
    gun_pod_wing(assets)?;
    Ok(())
}

/// The F-4B's centreline SUU-16 gun pod (2026-10-16) in a shared flight
/// loadout: the lead's pod, the wingmen's pods with their rounds as guns, and
/// the AI's gun leaving the pod's nose, at the start and after a restart.
fn gun_pod_wing(assets: &Assets) -> AppResult<()> {
    let data = &assets.theater_resources;
    let kind = AircraftType::load(data, AircraftId::F4B)?;
    let mut quick = QuickMission::new(AircraftId::F4B, assets.creator_options.clone(), data);
    quick.draft.values[4] = 4;
    for field in [7, 10, 21, 24, 27] {
        quick.draft.values[field] = 0;
    }
    let mut spec = quick.mission_spec()?;
    spec.share_flight_loadout = true;
    spec.theater = "UKR".into();
    spec.ai_flight_model = tore_world::ai_wings::AiFlightModel::Standard;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    let mut load = Loadout::new(&kind.profile, |name| {
        data.get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(name.to_owned()))
    })?;
    let row = tore_formats::aircraft::stations::gun_pod_row(AircraftId::F4B).ok_or("no pod row")?;
    let slot = load.configuration.hardpoint_slots[row].ok_or("pod row has no slot")?;
    let pod = Weapon::parse("SUU16.JT", data.get("SUU16.JT").ok_or("missing SUU-16")?)?;
    load.select(slot, pod)?;
    load.validate()?;
    spec.loadout = Some(LoadoutSpec::of(&load));
    let mut world = World::new(&spec, data, Seating::SinglePlayer)?;
    let mut checked = 0;
    for restarted in [false, true] {
        if restarted {
            world.restart(&kind, data)?;
        }
        verify(&world, &load)?;
        let wings = world.ai_wings.as_ref().ok_or("no wings")?;
        for id in 1..4 {
            let actor = wings.mission().actor(id).ok_or("missing wingman")?;
            let config = wings.configuration(id).ok_or("missing configuration")?;
            let station = &config.stations[slot];
            let spec = &actor.stations()[slot];
            if config.gun_pod(slot).map(|p| p.quantity) != Some(1)
                || spec.store.rounds != tore_sim::ai::weapon_service::Rounds::Finite(600)
                || spec.mount != station.muzzle()
            {
                return Err(format!("wingman {id}'s gun pod differs from the lead's").into());
            }
            checked += 1;
        }
    }
    println!(
        "qmc wing loadout: F-4B gun pod matches on {checked} wingman starts, rounds and muzzle, airborne and restart passed"
    );
    Ok(())
}

fn verify(world: &World, load: &Loadout) -> AppResult<()> {
    let wings = world.ai_wings.as_ref().ok_or("no wings")?;
    for id in 1..4 {
        let actual = world
            .setup
            .ai
            .as_ref()
            .and_then(|ai| ai.loadouts.get(&id))
            .ok_or("wingman has default load")?;
        let actor = wings.mission().actor(id).ok_or("missing wingman")?;
        let config = wings.configuration(id).ok_or("missing configuration")?;
        let ammo = load.ammunition()?;
        if config.tanks.len() != load.configuration.tanks.len()
            || config
                .tanks
                .iter()
                .zip(&load.configuration.tanks)
                .any(|(a, b)| {
                    a.hardpoint != b.hardpoint
                        || a.quantity != b.quantity
                        || a.store.as_ref().map(|t| &t.source)
                            != b.store.as_ref().map(|t| &t.source)
                })
            || config.designator_pods != load.configuration.designator_pods
            || actor.flight().systems.fuel.external != load.configuration.external_fuel_lbs
            || actor.stations().iter().zip(&ammo).any(|(s, n)| {
                s.store.rounds != tore_sim::ai::weapon_service::Rounds::Finite(u32::from(*n))
            })
            || config
                .stations
                .iter()
                .zip(&load.configuration.stations)
                .any(|(a, b)| a.weapon.source != b.weapon.source || a.count != b.count)
            || LoadoutSpec::of(actual) != LoadoutSpec::of(load)
            || wings
                .mission()
                .actor(id)
                .ok_or("missing wingman")?
                .flight()
                .fuel
                != load.fuel_lbs
        {
            return Err(format!("plane {id} differs from player").into());
        }
    }
    Ok(())
}

fn dump(world: &World, label: &str) {
    let wings = world.ai_wings.as_ref().unwrap();
    for id in 0..4 {
        let (config, fuel, ammo) = if id == 0 {
            let own = world.combat.state.own();
            (
                own.configuration(),
                world.cockpits[0].flight.fuel,
                own.ammo.clone(),
            )
        } else {
            let actor = wings.mission().actor(id).unwrap();
            (
                wings.configuration(id).unwrap(),
                actor.flight().fuel,
                actor
                    .stations()
                    .iter()
                    .map(|s| match s.store.rounds {
                        tore_sim::ai::weapon_service::Rounds::Finite(n) => n as u16,
                        tore_sim::ai::weapon_service::Rounds::Unlimited => u16::MAX,
                    })
                    .collect(),
            )
        };
        println!(
            "{label} plane={id} fuel={fuel} tanks={:?} stations={:?}",
            config
                .tanks
                .iter()
                .map(|t| (t.hardpoint, t.store.as_ref().map(|s| &s.source), t.quantity))
                .collect::<Vec<_>>(),
            config
                .stations
                .iter()
                .zip(ammo)
                .map(|(s, n)| (&s.weapon.source, s.count, n))
                .collect::<Vec<_>>()
        );
    }
}

fn record(world: &mut World, path: &Path) -> AppResult<()> {
    use replay::{convert, recorder};
    let snapshot = world.combat.render_snapshot();
    let human = recorder::Human::single_player("F-16C", true);
    let roster = recorder::roster(
        snapshot,
        &human,
        &[],
        world.ai_wings.as_ref(),
        world.combat.dummy_types(),
    );
    let header = recorder::header(
        tore_replay::MissionKind::QuickMission,
        &world.terrain,
        &convert::Presentation::of(snapshot),
        Vec::new(),
        std::time::SystemTime::now(),
    );
    let mut recording = recorder::Recorder::start(path.to_owned(), &header, &roster)?;
    recording.wait_for_writer();
    let flight = &world.cockpits[0].flight;
    recording.begin(recorder::Tick {
        snapshot,
        combat: &world.combat,
        flight,
        previous: flight,
        pilot: &Default::default(),
        others: &[],
        wings: world.ai_wings.as_ref(),
        world: &world.terrain,
        events: &[],
        outcomes: &[],
        journal: None,
    });
    recording.end(None, &mut world.combat);
    recording
        .finish(&Default::default())
        .ok_or("recording did not finish")?;
    Ok(())
}
