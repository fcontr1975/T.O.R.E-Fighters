//! `--check`: what a mission file will fly, printed without opening a port.
//! The plane numbers are the ones `open-planes` and the game's `--slot` use,
//! and the runway numbers are the ones `start ground` takes.

use crate::prepare::Prepared;
use tore_formats::surface_unit::class;
use tore_session::host::content::{GameContent, report_lines};
use tore_world::{
    mission::{EnemyStart, MissionSpec, RUNWAY_OBJECT_BASE, Start},
    terrain::Terrain,
    world::World,
};

/// Horizontal start clearance from hostile AAA/SAM units, with and without
/// base-layout defenses. Friendly and neutral units never constrain clearance.
/// Altitude must not make a start above a defense look safely distant.
pub fn defense_report(world: &World) -> Vec<String> {
    let defenses: Vec<_> = world
        .terrain
        .surface
        .units
        .iter()
        .filter(|unit| unit.class & (class::AAA | class::SAM) != 0)
        .collect();
    let mut lines = vec![format!(
        "Defense clearance: {} AAA/SAM units, horizontal nautical miles to hostile units",
        defenses.len()
    )];
    for plane in world.roster.planes() {
        let side = if plane.slot.wing.side.is_enemy() {
            "enemy"
        } else {
            "friendly"
        };
        let Some((start, _)) = world.plane_pose(plane.id) else {
            lines.push(format!(
                "  Defense start plane {} {side}: unknown position",
                plane.id.0
            ));
            continue;
        };
        let hostile = if plane.slot.wing.side.is_enemy() {
            tore_world::ai_wings::FRIENDLY_SIDE
        } else {
            tore_world::ai_wings::ENEMY_SIDE
        };
        for (label, include_layout) in [("Defense", true), ("Mission defense", false)] {
            let nearest = defenses
                .iter()
                .filter(|unit| unit.side == hostile)
                .filter(|unit| {
                    include_layout
                        || !matches!(unit.origin, tore_world::surface::Origin::Layout { .. })
                })
                .map(|unit| {
                    let distance = (start[0] - f64::from(unit.position[0]))
                        .hypot(start[2] - f64::from(unit.position[2]))
                        / 6076.12;
                    (distance, unit)
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            let result = nearest.map_or_else(
                || "none".into(),
                |(distance, unit)| {
                    format!("{distance:.6} nm, unit {} {}", unit.id.0, unit.resource)
                },
            );
            lines.push(format!(
                "  {label} start plane {} {side}: {result}",
                plane.id.0
            ));
        }
    }
    for (label, side) in [
        ("friendly", tore_world::ai_wings::FRIENDLY_SIDE),
        ("enemy", tore_world::ai_wings::ENEMY_SIDE),
    ] {
        let count = |kind, layout| {
            defenses
                .iter()
                .filter(|unit| {
                    unit.side == side
                        && unit.class & kind != 0
                        && matches!(unit.origin, tore_world::surface::Origin::Layout { .. })
                            == layout
                })
                .count()
        };
        lines.push(format!(
            "Defense inventory {label}: layout AAA {} SAM {}; mission AAA {} SAM {}",
            count(class::AAA, true),
            count(class::SAM, true),
            count(class::AAA, false),
            count(class::SAM, false)
        ));
    }
    lines
}

/// Why a mission's explicit ground start cannot be flown: `start ground N`
/// parks the friendly wing (Blue) on runway N, and an airport takes its
/// runway's layout side (slice AL1), so a Redfor field is refused. A neutral
/// field serves both sides. `None` for any other start, or a runway this
/// import does not have (the build reports that).
pub fn ground_start_problem(spec: &MissionSpec, terrain: &Terrain) -> Option<String> {
    let scene = &terrain.airport_scene;
    let runway = spec.ground_runway()?;
    let id = scene.runway(runway)?.airport;
    let airport = scene.airports.iter().find(|a| a.id == id)?;
    (!airport.serves(false)).then(|| {
        format!(
            "starts the friendly wing on runway {} at {}, an enemy airfield: a ground start needs a friendly or neutral runway (--check lists the runways and their sides)",
            runway.saturating_sub(RUNWAY_OBJECT_BASE),
            airport.name
        )
    })
}

/// How the runway list names an airport's side, from the friendly wing's
/// point of view, and which ground start may use it: a field that serves
/// both sides is neutral, one that serves only Redfor the enemy's, which
/// `enemy-start ground` takes (slice CL-R) and `start ground` refuses.
fn side_words(blue: bool, red: bool) -> &'static str {
    match (blue, red) {
        (true, true) => "neutral",
        (true, false) => "friendly",
        (false, true) => "enemy: enemy-start ground only",
        (false, false) => "no ground start",
    }
}

/// A one-line summary of the mission, for the start lines.
pub fn mission_summary(spec: &MissionSpec, aircraft: usize) -> String {
    let start = match spec.start {
        Start::Airborne { altitude_ft } => format!("airborne at {altitude_ft} ft"),
        Start::Ground { runway, .. } => format!(
            "ground start on runway {}",
            runway.saturating_sub(RUNWAY_OBJECT_BASE)
        ),
        Start::GroundAuto { .. } => "ground start on a runway the world picks".to_owned(),
    };
    let start = match spec.enemy_start {
        EnemyStart::Airborne => start,
        EnemyStart::Ground { runway } => format!(
            "{start}, enemy wing 1 on runway {}",
            runway.saturating_sub(RUNWAY_OBJECT_BASE)
        ),
        EnemyStart::GroundAuto => {
            format!("{start}, enemy wing 1 on a Redfor runway the world picks")
        }
    };
    let side = |range: std::ops::Range<usize>| -> String {
        let counts: Vec<String> = spec.wings[range]
            .iter()
            .filter(|wing| wing.count > 0)
            .map(|wing| format!("{} {}", wing.count, wing.aircraft.selection_key()))
            .collect();
        if counts.is_empty() {
            "none".into()
        } else {
            counts.join(", ")
        }
    };
    format!(
        "Mission: {} ({}), {start}, enemy {} nm away; friendly {}; enemy {}; {aircraft} aircraft",
        spec.theater,
        spec.condition.name(),
        spec.separation_nm,
        side(0..3),
        side(3..6),
    )
}

/// The lines `--check` prints.
pub fn report(prepared: &Prepared) -> Vec<String> {
    let spec = &prepared.spec;
    let planes = prepared.world.roster.planes();
    let mut lines = vec![
        mission_summary(spec, planes.len()),
        format!("Mission file: {}", prepared.config.mission.display()),
        String::new(),
        "Planes (the numbers open-planes and --slot use):".into(),
    ];
    if let Some(note) = &prepared.distance_note {
        lines.insert(1, note.clone());
    }
    if spec.lays_out_surface() {
        lines.push(tore_world::mission_layout::seed_retry::notice(
            prepared.placement_initial_seed,
            prepared.placement_preview_seed,
            prepared.placement_attempts,
        ));
    }
    if spec.lays_out_surface() && spec.surface_seed == 0 {
        lines.push(
            "Defense clearance is a preview: set a nonzero surface-seed to reproduce it in flight."
                .into(),
        );
    }
    for plane in planes {
        let wing = &spec.wings[MissionSpec::wing_index(plane.slot.wing)];
        let side = if plane.slot.wing.side.is_enemy() {
            "enemy"
        } else {
            "friendly"
        };
        if spec.player_choice[MissionSpec::wing_index(plane.slot.wing)] {
            lines.push(format!(
                "  Player Choice slot {} (AI default: {})",
                plane.id.0,
                wing.aircraft.selection_key()
            ));
        }
        lines.push(format!(
            "  {:>2}  {:<9} {side} wing {}, member {}, {}",
            plane.id.0,
            wing.aircraft.selection_key(),
            plane.slot.wing.index + 1,
            plane.slot.member + 1,
            wing.skill.name()
        ));
    }
    lines.extend(defense_report(&prepared.world));
    lines.push(String::new());
    lines.push(format!(
        "Runways of {} (the numbers start ground and enemy-start ground take):",
        spec.theater
    ));
    let scene = &prepared.world.terrain.airport_scene;
    let mut runways: Vec<_> = scene.runways.iter().collect();
    runways.sort_by_key(|runway| runway.object);
    if runways.is_empty() {
        lines.push("  none".into());
    }
    for runway in runways {
        let found = scene
            .airports
            .iter()
            .find(|airport| airport.id == runway.airport);
        let airport = found.map_or(runway.name.as_str(), |airport| airport.name.as_str());
        let side = found.map_or("no ground start", |a| {
            side_words(a.serves(false), a.serves(true))
        });
        // A STOL runway is a ground start like any other, labelled so the
        // host knows (John, 2026-10-11).
        let short = if runway.stol() { ", STOL" } else { "" };
        lines.push(format!(
            "  {:>3}  {airport} ({:.0} ft, {side}{short})",
            runway.object.saturating_sub(RUNWAY_OBJECT_BASE),
            runway.length_ft
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "Content manifest: {} resources, digest {:016x}",
        prepared.manifest.entries.len(),
        prepared.manifest.digest()
    ));
    // Stage L: the import's source and content, one line per item, so an
    // operator can compare two imports (`tore-bot --content-report` prints
    // the same lines).
    lines.push(String::new());
    let content = GameContent::read(&prepared.data_dir, &prepared.resources);
    lines.extend(report_lines(&content));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        options::Options,
        prepare::{
            prepare,
            tests::{MISSION, data_folder, data_folder_with},
        },
    };
    use std::fs;
    use tore_world::test_support::resources::owned_airport_resources;

    #[test]
    fn defense_clearance_measures_every_start_and_ignores_altitude_and_non_defenses() {
        use tore_world::test_support::surface::surface_resources;
        let dir = data_folder_with("check-defense", true, surface_resources());
        fs::write(dir.join("mission.txt"), MISSION).unwrap();
        let mut prepared = prepare(&Options::default(), &dir).unwrap();
        let world = &mut prepared.world;
        let start = world.plane_pose(world.roster.planes()[0].id).unwrap().0;
        let mut aaa = world.terrain.surface.units[0].clone();
        aaa.class = class::AAA;
        aaa.side = tore_world::ai_wings::ENEMY_SIDE;
        aaa.origin = tore_world::surface::Origin::Layout { ordinal: 0 };
        aaa.position = [
            (start[0] + 3. * 6076.12) as i32,
            0,
            (start[2] + 4. * 6076.12) as i32,
        ];
        let mut tank = aaa.clone();
        tank.class = class::TANK;
        tank.position = [start[0] as i32, 0, start[2] as i32];
        world.terrain.surface.units = vec![aaa.clone(), tank];
        let lines = defense_report(world);
        assert_eq!(lines.len(), 11);
        assert!(lines[0].contains("1 AAA/SAM"));
        assert!(lines[1].contains("plane 0 friendly: 4.999"), "{}", lines[1]);
        assert!(lines[7].contains("plane 3 enemy: none"));
        assert!(lines[2].ends_with(": none"));
        // Friendly defenses never constrain Blue, but do constrain Red.
        aaa.class = class::SAM;
        aaa.resource = "SYNTHETIC-SAM.NT".into();
        aaa.side = tore_world::ai_wings::FRIENDLY_SIDE;
        aaa.position = [start[0] as i32, -100_000, start[2] as i32];
        world.terrain.surface.units.push(aaa);
        assert!(!defense_report(world)[1].contains("SYNTHETIC-SAM.NT"));
        assert!(defense_report(world)[7].contains("SYNTHETIC-SAM.NT"));
        world.terrain.surface.units[0].origin = tore_world::surface::Origin::Added;
        assert!(!defense_report(world)[2].ends_with(": none"));
        world.terrain.surface.units.clear();
        assert!(defense_report(world)[1].ends_with(": none"));
        let _ = fs::remove_dir_all(dir);
    }

    /// A ground start on the synthetic field, owned by `nationality2`.
    fn ground_start_at(name: &str, nationality2: Option<u8>) -> Result<Prepared, String> {
        let dir = data_folder_with(name, true, owned_airport_resources(nationality2));
        fs::write(
            dir.join("mission.txt"),
            MISSION.replace("start airborne 10000", "start ground 0 10000"),
        )
        .unwrap();
        let prepared = prepare(&Options::default(), &dir);
        let _ = fs::remove_dir_all(dir);
        prepared
    }

    #[test]
    fn an_explicit_ground_start_at_an_enemy_field_is_refused() {
        // Bit 0x80 is Redfor: the friendly wing may not park there.
        let error = ground_start_at("check-enemy-field", Some(137))
            .err()
            .unwrap();
        assert!(
            error.contains("runway 0 at Synthetic Field, an enemy airfield"),
            "{error}"
        );
        assert!(error.contains("friendly or neutral runway"), "{error}");
        // A Blue field and an unowned (neutral) one are flown.
        let blue = ground_start_at("check-blue-field", Some(12)).unwrap();
        let text = report(&blue).join("\n");
        assert!(
            text.contains("    0  Synthetic Field (8000 ft, friendly)"),
            "{text}"
        );
        let neutral = ground_start_at("check-neutral-field", None).unwrap();
        assert!(report(&neutral).join("\n").contains("ft, neutral)"));
    }

    #[test]
    fn an_enemy_ground_start_takes_a_redfor_field_and_the_report_says_so() {
        let at = |name: &str, nationality2: Option<u8>| {
            let dir = data_folder_with(name, true, owned_airport_resources(nationality2));
            fs::write(
                dir.join("mission.txt"),
                format!("{MISSION}enemy-start ground 0\n"),
            )
            .unwrap();
            let prepared = prepare(&Options::default(), &dir);
            let _ = fs::remove_dir_all(dir);
            prepared
        };
        let red = at("check-enemy-start-red", Some(137)).unwrap();
        let text = report(&red).join("\n");
        assert!(text.contains("enemy wing 1 on runway 0"), "{text}");
        assert!(
            text.contains("Synthetic Field (8000 ft, enemy: enemy-start ground only)"),
            "{text}"
        );
        // A Blue field is the other side's.
        let error = at("check-enemy-start-blue", Some(12)).err().unwrap();
        assert!(error.contains("is a Blue airfield"), "{error}");
    }

    #[test]
    fn the_report_lists_planes_runways_and_the_digest() {
        let dir = data_folder("check-report", true);
        fs::write(dir.join("mission.txt"), MISSION).unwrap();
        let prepared = prepare(&Options::default(), &dir).unwrap();
        let text = report(&prepared).join("\n");
        assert!(
            text.contains("Mission: UKR (clear), airborne at 10000 ft, enemy 2 nm away"),
            "{text}"
        );
        assert!(text.contains("4 aircraft"), "{text}");
        assert!(
            text.contains("   0  F18.PT    friendly wing 1, member 1, average"),
            "{text}"
        );
        assert!(
            text.contains("   3  F18.PT    enemy wing 1, member 2, average"),
            "{text}"
        );
        assert!(text.contains("Runways of UKR"), "{text}");
        assert!(
            text.contains(&format!("digest {:016x}", prepared.manifest.digest())),
            "{text}"
        );
        // Stage L: the source and one line per item.
        assert!(
            text.contains(
                "Content: an unknown Fighters Anthology build, imported by an unknown T.O.R.E"
            ),
            "{text}"
        );
        assert!(
            text.contains("Content items: 1 aircraft, 1 theater, 2 weapons, the shared data"),
            "{text}"
        );
        assert!(text.contains("\n  aircraft F18.PT "), "{text}");
        assert!(text.contains("\n  shared data "), "{text}");
        let _ = fs::remove_dir_all(dir);
    }
}
