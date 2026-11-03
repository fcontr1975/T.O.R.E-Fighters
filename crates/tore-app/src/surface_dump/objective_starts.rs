//! Final mission starts and assignment references, after the full world build.
use super::*;
use tore_formats::aircraft::AircraftId;
use tore_world::{
    ai_wings::Preset,
    mission::{Defense, MissionSpec, Start},
    world::{Seating, World},
};

pub(super) fn run(
    resources: &BTreeMap<String, Vec<u8>>,
    seeds: u32,
    wanted: &[String],
    fast: bool,
) -> AppResult<()> {
    let mut cases = 0;
    let mut findings = 0;
    for (index, theater, mut stems) in jobs(wanted)? {
        if fast {
            stems.truncate(1);
        }
        stems.push("NONE");
        for stem in stems {
            for seed in 1..=seeds {
                for separation in tore_world::mission_layout::SEPARATION_NM {
                    let mut spec = MissionSpec::new(theater, AircraftId::F18);
                    spec.weather = overrides()?;
                    spec.ground_target = (stem != "NONE").then(|| stem.to_owned());
                    spec.surface_seed = seed;
                    spec.enemy_nationality = tables::ENEMY_NATIONALITY[index] as u8;
                    spec.aaa = Defense::Heavy;
                    spec.sam = Defense::Heavy;
                    spec.separation_nm = separation as u32;
                    spec.start = Start::Airborne {
                        altitude_ft: 20_000,
                    };
                    for wing in &mut spec.wings {
                        wing.count = 5;
                        wing.aircraft = AircraftId::F18;
                    }
                    let mut world = match World::new(&spec, resources, Seating::SinglePlayer) {
                        Ok(world) => world,
                        Err(e)
                            if e.to_string().starts_with("Separation ")
                                || e.to_string().starts_with("Vehicle placement unavailable:") =>
                        {
                            cases += 1;
                            println!(
                                "objective-start-refused: {theater} {stem} separation {separation} seed {seed}: {e}"
                            );
                            continue;
                        }
                        Err(e) => return Err(e),
                    };
                    let extent = [
                        (world.terrain.theater.cols - 1) as f64 * 8192.,
                        (world.terrain.theater.rows - 1) as f64 * 8192.,
                    ];
                    let inside = |p: [f64; 3]| {
                        p.iter().all(|v| v.is_finite())
                            && (0. ..=extent[0]).contains(&p[0])
                            && (0. ..=extent[1]).contains(&p[2])
                    };
                    let mut issue = |object: String, p: [f64; 3], rule: &str| {
                        findings += 1;
                        println!(
                            "start-finding\t{theater}\t{separation}\t{stem}\t3\t3\t{seed}\t{object}\t{:.2}\t{:.2}\t{:.2}\t{rule}",
                            p[0], p[1], p[2]
                        );
                    };
                    // Exercise the actual five-aircraft ground placement for every
                    // offered runway, including both sides' automatic selection.
                    for runway in &world.terrain.airport_scene.runways {
                        if world.terrain.airport_scene.vertical_pad(runway.object) {
                            continue;
                        }
                        match tore_world::mission_layout::ground_layout(
                            &world.terrain,
                            runway.object,
                            5,
                        ) {
                            Ok(layout) => {
                                for (member, p) in layout.slots.iter().enumerate() {
                                    if !inside(*p) {
                                        issue(
                                            format!("runway-{}-slot-{member}", runway.object),
                                            *p,
                                            "ground-start-outside-map",
                                        );
                                    }
                                    let surface = world.terrain.surface(p[0], p[2]);
                                    if !surface.landable || (p[1] - surface.height).abs() > 0.05 {
                                        issue(
                                            format!("runway-{}-slot-{member}", runway.object),
                                            *p,
                                            "ground-start-not-supported",
                                        );
                                    }
                                }
                            }
                            Err(error) => println!(
                                "objective-ground-refused: {theater} {stem} separation {separation} seed {seed} runway {} {error}",
                                runway.object
                            ),
                        }
                    }
                    for red in [false, true] {
                        match tore_world::mission_layout::auto_runway_for(&world.terrain, 5, red) {
                            Ok(id) => println!(
                                "objective-ground-auto: {theater} {stem} separation {separation} seed {seed} red {red} runway {id}"
                            ),
                            Err(error) => println!(
                                "objective-ground-refused: {theater} {stem} separation {separation} seed {seed} red {red} {error}"
                            ),
                        }
                    }
                    let player = world.cockpits[0].flight.position;
                    let wings = world.ai_wings.as_mut().ok_or("mission has no AI wings")?;
                    if let Some(red) = wings.mission().actors().iter().find(|a| {
                        a.identity().side == tore_world::ai_wings::ENEMY_SIDE
                            && a.identity().wing == 0
                            && a.identity().member == 0
                    }) {
                        let p = red.flight().position;
                        let actual = (p[0] - player[0]).hypot(p[2] - player[2])
                            / tore_world::mission_layout::FEET_PER_NM;
                        if (actual - separation).abs() > 0.02 {
                            issue(
                                "blue-to-red".into(),
                                player,
                                &format!("actual-separation-nm:{actual:.3};requested={separation}"),
                            );
                        }
                    }
                    let ids: std::collections::BTreeSet<_> = std::iter::once(0)
                        .chain(
                            wings
                                .mission()
                                .actors()
                                .iter()
                                .map(|a| a.identity().actor.0),
                        )
                        .collect();
                    if ids.len() != 30 {
                        issue("mission".into(), player, "missing-or-duplicate-aircraft");
                    }
                    for (id, p) in std::iter::once((0, player)).chain(
                        wings
                            .mission()
                            .actors()
                            .iter()
                            .map(|a| (a.identity().actor.0, a.flight().position)),
                    ) {
                        if !inside(p) {
                            issue(format!("aircraft-{id}"), p, "outside-map");
                        }
                        if p[1] < f64::from(world.terrain.height(p[0] as f32, p[2] as f32)) + 100. {
                            issue(
                                format!("aircraft-{id}"),
                                p,
                                "airborne-under-terrain-clearance",
                            );
                        }
                    }
                    for preset in [Preset::Cap, Preset::Intercept, Preset::Escort] {
                        wings.apply_mission_preset(preset, player);
                        for actor in wings.mission().actors() {
                            let assignment = actor.assignment();
                            if let Some(patrol) = assignment.patrol {
                                if !inside(patrol.center_ft) {
                                    issue(
                                        format!("{preset}-{}", actor.identity().actor.0),
                                        patrol.center_ft,
                                        "objective-center-outside-map",
                                    );
                                }
                                if (0..4).any(|i| {
                                    let mut p = patrol.center_ft;
                                    p[if i < 2 { 0 } else { 2 }] += if i % 2 == 0 {
                                        patrol.radius_ft
                                    } else {
                                        -patrol.radius_ft
                                    };
                                    !inside(p) && patrol.contains(p)
                                }) {
                                    issue(
                                        format!("{preset}-{}", actor.identity().actor.0),
                                        patrol.center_ft,
                                        "patrol-region-crosses-map-edge",
                                    );
                                }
                            }
                            for id in assignment
                                .protected_ids
                                .iter()
                                .chain(&assignment.destroy_ids)
                            {
                                if !ids.contains(id) {
                                    issue(
                                        format!("{preset}-{}", actor.identity().actor.0),
                                        actor.flight().position,
                                        "objective-aircraft-reference-missing",
                                    );
                                }
                            }
                        }
                    }
                    if stem != "NONE" {
                        let mut objectives = [tore_sim::ai::engagement::GroupObjective::Inherit; 6];
                        objectives[1] = tore_sim::ai::engagement::GroupObjective::Strike;
                        wings.apply_strike_roles(&objectives, true);
                        for actor in wings.mission().actors().iter().filter(|a| {
                            a.strike_role() == tore_sim::ai::engagement::StrikeRole::Strike
                        }) {
                            println!(
                                "objective-striker: {theater} {stem} separation {separation} seed {seed} aircraft {}",
                                actor.identity().actor.0
                            );
                        }
                    }
                    cases += 1;
                    println!(
                        "objective-start-case: {theater} {stem} separation {separation} seed {seed} aircraft {}",
                        ids.len()
                    );
                }
            }
        }
        eprintln!("objective-start-progress: {theater} cases {cases} findings {findings}");
    }
    println!("objective-start-audit: cases {cases} findings {findings}");
    Ok(())
}
