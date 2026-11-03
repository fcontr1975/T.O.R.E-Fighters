//! `--surface-dump` and `--surface-sheets`: development aids for the surface
//! units (docs/spec/surface-defenses.md). No window opens and nothing is
//! simulated.
//!
//! - `--surface-dump THEATER [STEM] [--defenses AAA SAM] [--surface-seed N]
//!   [--enemy-nationality N] [--night-stealth]` prints the theater's surface
//!   as a mission resolves it, with the ground target template `STEM` when
//!   one is named: every unit with its id, type, side, position and look, the
//!   removed slots, and the digest.
//! - `--surface-dump --all [--seeds N]` resolves every offered template of
//!   every theater at every defense level with seeds 1 to N (3 by default),
//!   builds each theater with every template at heavy defenses, and prints a
//!   line per resolution for the `surface-resolve-all` battery scenario.
//! - `--surface-sheets OUT_DIR [--variants] [THEATER[:STEM] ...]` renders
//!   each template from above, heavy defenses: at its retail spot (no jitter,
//!   no relocation, seed 1), and with `--variants` also placed with seeds 1
//!   and 2. A marker per unit: red Redfor, blue Blue, a ring for targets,
//!   yellow for parked aircraft, green for supply trucks, cyan for battery
//!   radars, orange for battery launchers, magenta for units whose shape the
//!   reader cannot draw yet.
//! - `--surface-dump --objectives [--seeds N] [--fast] [--uncached]
//!   [THEATER ...]` checks every distance and defense pair in the completed
//!   scene, retaining authored exceptions as findings. `--objective-starts`
//!   checks full mission aircraft starts, ground slots and assignment references.
//!   See docs/DEVELOPMENT.md for scope, reuse checks and known policy conflicts.
//!
//! - `--surface-dump --sweep [--seeds N] [THEATER ...]` places every
//!   template with seeds 1 to N (20) and reports the site rules each breaks,
//!   and each theater's base-layout batteries; `--surface-dump --starts
//!   [THEATER ...]` builds a mission per theater and reports where Blue and
//!   Red start. The `surface-relocate-sweep` and `surface-start-placement`
//!   scenarios.
//!
//! - `--surface-dump THEATER --area [--defenses AAA SAM] [--surface-seed N]`
//!   prints the theater's surface with the enemy area defenses of a Quick
//!   Mission with no ground target (B23); `--surface-dump --area-sweep
//!   [--seeds N] [--png DIR] [THEATER ...]` places them in every (named)
//!   theater at every strength with seeds 1 to N (3), reports the area rules
//!   each unit breaks and, with `--png`, draws each theater's heavy layout
//!   from above. The `surface-area-sweep` scenario.
//!
//! The import does not keep the templates or the unit types only they name
//! yet, so both commands add what the pack lacks from the retail `FA_2.LIB`
//! the data directory remembers (read at runtime, never stored).
mod objective_starts;
mod objectives;

use crate::{
    AppResult,
    camera::Camera,
    reel::{Gpu, HEIGHT, WIDTH},
    scenery::Scenery,
    terrain::{Overrides, Terrain},
};
use std::{collections::BTreeMap, path::Path};
use tore_formats::{
    quick_template::{Placeholder, Template, tables},
    surface_unit::class,
};
use tore_world::surface::{
    IdRange, Origin, Surface, UnitId,
    catalog::Catalog,
    layout::{self, Variation},
    resolve::{self, GroundTarget},
};

/// The world of `spec` as the weapon, radar and drive probes want it: the
/// ground target at its seeded layout without the Quick Mission front line,
/// whose Blue and Red units fighting each other would add contacts, losses
/// and replay events these probes do not measure
/// ([the front line](docs/spec/surface-defenses.md#the-front-line)).
pub(crate) fn world_without_front(
    spec: &tore_world::mission::MissionSpec,
    resources: &dyn tore_world::resources::ResourceSource,
    seating: tore_world::world::Seating,
) -> tore_world::WorldResult<tore_world::world::World> {
    Ok(tore_world::world::World::build(
        spec,
        resources,
        seating,
        &mut tore_world::world::Hooks {
            inspect_invalid_placement: true,
            ground_variation: Some(Variation {
                front: false,
                ..Variation::ON
            }),
            ..Default::default()
        },
    )?
    .world)
}

/// The pack's theater resources with what the import does not keep yet: the
/// ground target templates and the NT, OT, PT, shape and picture records,
/// read from the retail media: `TORE_GAME_DIR`, else the source the data
/// directory remembers, else the checkout's `gameassets/fighters-anthology`
/// link. Returns how many were added.
pub(crate) fn with_retail_surface(resources: &mut BTreeMap<String, Vec<u8>>) -> AppResult<usize> {
    with_retail_records(resources, false)
}

fn with_retail_records(
    resources: &mut BTreeMap<String, Vec<u8>>,
    missions: bool,
) -> AppResult<usize> {
    let data = crate::assets::data_directory()?;
    let candidates = std::env::var_os("TORE_GAME_DIR")
        .map(std::path::PathBuf::from)
        .into_iter()
        .chain(tore_import::media_source::remembered(&data).map(|(path, _)| path))
        .chain([std::path::PathBuf::from("gameassets/fighters-anthology")]);
    let Some(media) = candidates
        .into_iter()
        .find_map(|path| tore_import::MediaSource::detect(&path).ok())
    else {
        return Err(
            "no retail media to read the ground target templates from: set TORE_GAME_DIR".into(),
        );
    };
    let mut added = 0;
    for name in ["FA_2.LIB", "FA_1.LIB"] {
        let Some(archive) = media.optional_archive(name)? else {
            continue;
        };
        for entry in archive.entries.keys() {
            let wanted = (entry.starts_with("~Q") && entry.ends_with(".M"))
                || (missions && (entry.ends_with(".M") || entry.ends_with(".MM")))
                || [".NT", ".OT", ".PT", ".SH", ".PIC"]
                    .iter()
                    .any(|ext| entry.ends_with(ext));
            if wanted && !resources.contains_key(entry) {
                resources.insert(entry.clone(), archive.read(entry)?);
                added += 1;
            }
        }
    }
    Ok(added)
}

fn resources() -> AppResult<BTreeMap<String, Vec<u8>>> {
    let mut resources = crate::reel::load_assets()?.theater_resources;
    let added = with_retail_surface(&mut resources)?;
    println!("surface: {added} resources added from the retail media");
    Ok(resources)
}

/// The base theater index of a layout code (`~UKR1` is Ukraine).
fn theater_index(code: &str) -> AppResult<usize> {
    let base = tore_formats::theater::base_theater(&format!("{code}.MM"))
        .ok_or_else(|| format!("{code}: not a theater"))?;
    tables::THEATERS
        .iter()
        .position(|t| t.eq_ignore_ascii_case(base) || base.starts_with(t))
        .ok_or_else(|| format!("{code}: no ground targets for theater {base}").into())
}

/// The enemy distance the dumps place Blue at from Red, nm, unless
/// `--separation` says otherwise (the creator's 20 nm entry).
const DUMP_SEPARATION_NM: u32 = 20;

/// Noon, no wind, no cloud; the redrawn airports unless
/// `TORE_REDRAWN_AIRPORTS=0`.
fn overrides() -> AppResult<Overrides> {
    Ok(Overrides {
        time: Some([12, 0]),
        wind: None,
        cloud_altitude: Some(0),
        redrawn_airports: crate::scenery::redrawn_airports()?,
        city_fill: crate::scenery::city_fill()?,
        city_fill_cap: crate::scenery::city_fill_cap()?,
    })
}

pub fn run() -> AppResult<()> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut positional = Vec::new();
    let mut levels = (3usize, 3usize);
    let mut seed = 1u32;
    let mut nationality = None;
    let mut night = false;
    let mut objective_starts = false;
    let mut objectives = false;
    let mut fast = false;
    let mut uncached = false;
    let mut all = false;
    let mut sweep = false;
    let mut starts = false;
    let mut classes = false;
    let mut grounding = false;
    let mut grounding_sites = false;
    let mut ground_pads = false;
    let mut damage = false;
    let mut faces = false;
    let mut seeds = None;
    let mut variation = Variation::ON;
    let mut separation = DUMP_SEPARATION_NM;
    let mut area = false;
    let mut area_sweep = false;
    let mut png = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut next = || {
            it.next()
                .ok_or_else(|| format!("{arg} needs a value"))
                .cloned()
        };
        match arg.as_str() {
            "--defenses" => levels = (next()?.parse()?, next()?.parse()?),
            "--surface-seed" => seed = next()?.parse()?,
            "--enemy-nationality" => nationality = Some(next()?.parse()?),
            "--night-stealth" => night = true,
            "--objective-starts" => objective_starts = true,
            "--objectives" => objectives = true,
            "--fast" => fast = true,
            "--uncached" => uncached = true,
            "--all" => all = true,
            "--sweep" => sweep = true,
            "--starts" => starts = true,
            "--classes" => classes = true,
            "--grounding" => grounding = true,
            "--grounding-sites" => grounding_sites = true,
            "--ground-pads" => ground_pads = true,
            "--damage" => damage = true,
            "--faces" => faces = true,
            "--no-jitter" => variation.jitter = false,
            "--no-relocate" => variation.relocate = false,
            "--no-front" => variation.front = false,
            "--separation" => separation = next()?.parse()?,
            "--seeds" => seeds = Some(next()?.parse()?),
            "--area" => area = true,
            "--area-sweep" => area_sweep = true,
            "--png" => png = Some(std::path::PathBuf::from(next()?)),
            other if other.starts_with("--") => return Err(format!("unknown {other}").into()),
            other => positional.push(other.to_owned()),
        }
    }
    let mut resources = resources()?;
    if grounding_sites {
        with_retail_records(&mut resources, true)?;
    }
    if objective_starts {
        return objective_starts::run(&resources, seeds.unwrap_or(3), &positional, fast);
    }
    if objectives {
        return objectives::run(&resources, seeds.unwrap_or(3), &positional, fast, uncached);
    }
    if all {
        return resolve_all(&resources, seeds.unwrap_or(3));
    }
    if sweep {
        return relocate_sweep(&resources, seeds.unwrap_or(20), &positional);
    }
    if classes {
        return class_sweep(&resources, seeds.unwrap_or(20), &positional);
    }
    if starts {
        return start_placement(&resources, &positional);
    }
    if area_sweep {
        return area_sweep_run(&resources, seeds.unwrap_or(3), png.as_deref(), &positional);
    }
    if ground_pads {
        return ground_pads_survey(&resources, &positional);
    }
    if grounding_sites {
        return grounding_sites_survey(&resources);
    }
    if grounding {
        return grounding_survey(&resources, &positional);
    }
    if damage {
        return damage_survey(&resources, &positional);
    }
    if faces {
        return face_survey(&resources, &positional);
    }
    let [theater, rest @ ..] = positional.as_slice() else {
        return Err("--surface-dump THEATER [STEM] [--defenses AAA SAM] [--surface-seed N] [--enemy-nationality N] [--night-stealth] [--no-jitter] [--no-relocate] | --all [--seeds N] | --sweep [--seeds N] [THEATER...] | --starts [THEATER...]".into());
    };
    let index = theater_index(theater)?;
    let target = match rest {
        [] => None,
        [stem] => Some(GroundTarget {
            stem: stem.trim_start_matches('~').to_ascii_uppercase(),
            aaa: levels.0,
            sam: levels.1,
            seed,
            enemy_nationality: nationality.unwrap_or(tables::ENEMY_NATIONALITY[index]),
            night_stealth: night,
            variation,
            separation_nm: separation,
        }),
        _ => return Err("--surface-dump takes one template".into()),
    };
    let area = (area && target.is_none()).then(|| resolve::AreaDefense {
        aaa: levels.0,
        sam: levels.1,
        seed,
        enemy_nationality: nationality.unwrap_or(tables::ENEMY_NATIONALITY[index]),
        night_stealth: night,
        rolled: true,
    });
    let terrain = Terrain::for_mission_surface(
        &resources,
        theater,
        Some(0),
        &overrides()?,
        target.as_ref(),
        area.as_ref(),
    )?;
    print_surface(theater, &terrain.surface);
    Ok(())
}

/// The viewer ranges, feet, at which `--surface-dump --grounding` draws a
/// man's sprites: at his own size, and at the far-man boost's most.
const GROUNDING_RANGES_FT: [f64; 2] = [300., 6_000.];

/// How far a drawn bottom may sit from the ground, feet, before the audit
/// calls it sunk or floating (float rounding of the drawn vertices).
const GROUNDING_TOLERANCE_FT: f64 = 0.05;

/// `--surface-dump --grounding [TYPE ...]`: the grounding audit
/// (docs/formats/objects-and-shapes.md, "Grounding"). Every placed type of
/// the import and the retail media (OT and NT, or the types named) is stood
/// on flat ground at 0 ft through the placement path
/// (`Placements::stance`, which also sets the hit box) and drawn through the
/// scenery's own code: its faces, and a man (sprites or cards) as he is
/// drawn at 300 ft and, boosted, at 6,000 ft. Every parked aircraft type (PT) is
/// stood through `surface::parked::place` and drawn gear down. Each row
/// gives the shape's lowest and highest point about its origin (world feet
/// at the size it is drawn), its F2 ground offset, the lift
/// (`tore_world::grounding`), and the lowest drawn point of the faces, the
/// hit box and the sprites above the ground. A type that stands on its
/// bottom and draws below the ground or floats above it is a problem.
fn grounding_survey(resources: &BTreeMap<String, Vec<u8>>, named: &[String]) -> AppResult<()> {
    use tore_world::grounding::{Footing, footing, height_range};
    let wanted: Vec<String> = named.iter().map(|n| n.to_ascii_uppercase()).collect();
    let chosen = |upper: &str, extensions: &[&str]| {
        if wanted.is_empty() {
            extensions.iter().any(|e| upper.ends_with(e))
        } else {
            wanted
                .iter()
                .any(|w| *w == upper || extensions.iter().any(|e| format!("{w}{e}") == upper))
        }
    };
    let types: Vec<&str> = resources
        .keys()
        .filter(|name| chosen(&name.to_ascii_uppercase(), &[".OT", ".NT"]))
        .map(String::as_str)
        .collect();
    let placements = tore_world::terrain::Placements::of_types(resources, types.iter().copied());
    let feet = |v: Option<f64>| v.map_or("-".to_owned(), |v| format!("{v:.2}"));
    let (mut total, mut lifted, mut problems) = (0, 0, Vec::new());
    for name in &types {
        let (Some(definition), Some(shape)) = (
            placements.definitions.get(*name),
            placements.shapes.get(*name),
        ) else {
            continue;
        };
        let placement = tore_formats::mission::Placement {
            key: tore_formats::mission::SourceKey {
                layout: "AUDIT".into(),
                ordinal: 0,
            },
            section: None,
            object_type: (*name).to_owned(),
            position: [0; 3],
            angles: [0; 3],
            source_nationality: None,
            nationality2: false,
            nationality3: false,
            nationality: None,
            flags: None,
            speed: None,
            name: None,
            alias: None,
            unknown: Vec::new(),
        };
        let (Some(stance), Some([low, high])) =
            (placements.stance(&placement, 0.), height_range(shape))
        else {
            continue;
        };
        let main_shape = definition.main_shape.as_deref().unwrap_or("-");
        let f2 = resources
            .get(main_shape)
            .and_then(|bytes| tore_formats::shape::contact_offset(bytes).ok().flatten())
            .map(|v| tore_world::terrain::PlacedSize::of(definition).feet(f64::from(v)));
        let lift = (0..3)
            .map(|i| (stance.origin[i] - stance.support_origin[i]) * stance.basis.up[i])
            .sum::<f64>();
        let stand = crate::scenery::surface_art::Stand {
            scale: stance.scale,
            basis: stance.basis,
            origin: stance.origin,
        };
        let (drawn, sprites) = crate::scenery::surface_art::drawn_bottoms(
            shape,
            &stand,
            &GROUNDING_RANGES_FT,
            crate::scenery::surface_art::is_man(definition),
        );
        let hit_box = stance.center[1] - stance.half[1];
        let foot = footing(definition);
        total += 1;
        if lift.abs() > GROUNDING_TOLERANCE_FT {
            lifted += 1;
        }
        let tall = (high - low) * stance.scale;
        if foot == Footing::Bottom {
            let mut wrong = |what: &str, at: f64| {
                if at.abs() > GROUNDING_TOLERANCE_FT {
                    problems.push(format!("{name} {what} at {at:+.2} ft, the ground at 0"));
                }
            };
            if let Some(at) = drawn {
                wrong("faces' bottom", at);
            }
            for (range, at) in GROUNDING_RANGES_FT.iter().zip(&sprites) {
                if let Some(at) = at {
                    wrong(&format!("man at {range:.0} ft"), *at);
                }
            }
            // The hit box is at least 2 ft tall: a thinner shape's box
            // reaches below its bottom by the difference.
            if tall >= 2. && hit_box < -GROUNDING_TOLERANCE_FT {
                // A thin frame may touch ground below the solid body. The
                // contact box must not sink, but need not touch the frame tip.
                wrong("hit box bottom", hit_box);
            }
        }
        println!(
            "grounding: {name} shape {main_shape} size {:?} class {:#06x} footing {foot:?} low {:.1} high {:.1} below {:.0}% f2 {} lift {lift:.1} drawn {} box {hit_box:.2} near {} far {} hp {}",
            tore_world::terrain::PlacedSize::of(definition),
            definition.category,
            low * stance.scale,
            high * stance.scale,
            if high > low {
                (-low).max(0.) / (high - low) * 100.
            } else {
                0.
            },
            f2.map_or("-".into(), |v| format!("{v:.1}")),
            feet(drawn),
            feet(sprites[0]),
            feet(sprites[1]),
            definition
                .hit_points
                .map_or("-".into(), |hp| hp.to_string()),
        );
    }
    // Parked aircraft, stood on their wheels the way a ground target's are.
    let surface = tore_world::surface::Surface {
        parked: resources
            .keys()
            .filter(|name| chosen(&name.to_ascii_uppercase(), &[".PT"]))
            .filter(|name| {
                tore_formats::parked_aircraft::ParkedType::parse(&resources[*name]).is_ok()
            })
            .enumerate()
            .map(|(n, name)| tore_world::surface::ParkedAircraft {
                id: tore_world::surface::UnitId(0x5100_0000 + n as u32),
                resource: name.clone(),
                position: [0; 3],
                angles: [0; 3],
                nationality: None,
                side: tore_sim::combat::live::Side(1),
                target: false,
                deck: None,
            })
            .collect(),
        ..Default::default()
    };
    let (poses, _) = tore_world::surface::parked::place(resources, &surface, &|_, _| 0.);
    for pose in poses {
        let Some(down) = resources
            .get(&pose.shape)
            .and_then(|bytes| tore_formats::parked_aircraft::gear(bytes).ok())
            .map(|gear| gear.down)
        else {
            continue;
        };
        let Some([low, high]) = height_range(&down) else {
            continue;
        };
        let stand = crate::scenery::surface_art::Stand {
            scale: pose.scale,
            basis: pose.basis,
            origin: pose.origin,
        };
        let (drawn, _) = crate::scenery::surface_art::drawn_bottoms(&down, &stand, &[], false);
        let lift = pose.origin[1] - pose.ground[1];
        total += 1;
        if lift.abs() > GROUNDING_TOLERANCE_FT {
            lifted += 1;
        }
        if let Some(at) = drawn.filter(|at| at.abs() > GROUNDING_TOLERANCE_FT) {
            problems.push(format!(
                "{} parked faces' bottom at {at:+.2} ft, the ground at 0",
                pose.resource
            ));
        }
        println!(
            "grounding: {} shape {} size Aircraft class {:#06x} footing Bottom low {:.1} high {:.1} below {:.0}% f2 - lift {lift:.1} drawn {} box - near - far - hp {}",
            pose.resource,
            pose.shape,
            pose.class,
            low * pose.scale,
            high * pose.scale,
            if high > low {
                (-low).max(0.) / (high - low) * 100.
            } else {
                0.
            },
            feet(drawn),
            pose.hit_points,
        );
    }
    for problem in &problems {
        println!("grounding: problem {problem}");
    }
    println!(
        "grounding: {total} types, {lifted} lifted, {} problems",
        problems.len()
    );
    Ok(())
}

/// Project only placement fields from a full mission. Unlike MM layouts,
/// missions repeat loadout and command fields within an object. They have no
/// bearing on footing and are not interpreted or passed to the strict layout
/// reader. The latter still validates every retained field and record bound.
fn mission_placements(name: &str, bytes: &[u8]) -> AppResult<tore_formats::mission::Layout> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("mission exceeds placement audit limit".into());
    }
    let text = tore_formats::text::decode_cp437(bytes);
    if !text.starts_with("textFormat") {
        return Err("mission has no textFormat header".into());
    }
    let mut filtered = String::from("textFormat\n");
    let mut object: Option<String> = None;
    for line in text.lines() {
        let field = line.trim();
        if field == "obj" {
            object = Some(String::from("obj\n"));
        } else if field == "." && object.is_some() {
            let record = object.take().unwrap_or_default();
            // `<selected>` is the player's own aircraft, airborne at the
            // start: no ground object, and no resource name to look up.
            if !record.lines().any(|l| l.trim() == "type <selected>") {
                filtered.push_str(&record);
                filtered.push_str(".\n");
            }
        } else if let Some(record) = object.as_mut() {
            let key = field.split_whitespace().next().unwrap_or("");
            if [
                "type",
                "pos",
                "angle",
                "nationality",
                "nationality2",
                "nationality3",
                "flags",
                "speed",
                "alias",
            ]
            .contains(&key)
            {
                record.push_str(line);
                record.push('\n');
            }
        } else if field.starts_with("map ") {
            filtered.push_str(field);
            filtered.push('\n');
        }
    }
    if let Some(record) = object {
        filtered.push_str(&record); // An unterminated record stays an error.
    }
    Ok(tore_formats::mission::Layout::parse(
        name,
        filtered.as_bytes(),
    )?)
}

/// Audit actual authored placements and resolved Quick Mission sites. The old
/// origin-only rule is measured alongside the live rule, using the same mesh.
fn grounding_sites_survey(resources: &BTreeMap<String, Vec<u8>>) -> AppResult<()> {
    use tore_world::terrain::{Placements, SurfaceSite};
    let types = resources
        .keys()
        .filter(|n| n.ends_with(".OT") || n.ends_with(".NT"));
    let shapes = Placements::of_types(resources, types.map(String::as_str));
    // What a driving unit's slope rule measures each step.
    let footing = shapes
        .shapes
        .iter()
        .filter(|(name, _)| name.ends_with(".NT"))
        .map(|(name, shape)| {
            let scale = shapes.shape_scale(name);
            (
                tore_world::grounding::UnitFooting::of(shape, scale)
                    .points
                    .len(),
                name,
            )
        })
        .max();
    if let Some((points, name)) = footing {
        println!("grounding-sites: unit footing at most {points} points ({name})");
    }
    // The types the origin rule keeps whatever the slope.
    for (name, shape) in &shapes.shapes {
        let scale = shapes.shape_scale(name);
        let reach = tore_world::grounding::reach_ft(shape, scale);
        let flat = tore_world::grounding::height_range(shape)
            .is_none_or(|[low, high]| high - low <= f64::EPSILON);
        if flat || reach > tore_world::grounding::SLOPE_LIFT_MAX_REACH_FT {
            let why = if flat { "flat" } else { "reach" };
            println!("grounding-origin-type: {name} {why} reach {reach:.0}");
        }
    }
    let mut layouts = Vec::new();
    let mut unread = 0;
    let mut binary = 0;
    for (name, bytes) in resources
        .iter()
        .filter(|(n, _)| (n.ends_with(".M") || n.ends_with(".MM")) && !n.starts_with("~Q"))
    {
        // Compiled campaign missions: the game has no campaign runtime and
        // never reads them (docs/formats/coverage.md), so nothing stands there.
        if !bytes.starts_with(b"textFormat") {
            binary += 1;
            continue;
        }
        match mission_placements(name, bytes) {
            Ok(layout) => layouts.push(layout),
            Err(error) => {
                unread += 1;
                println!("sites-unread: {name}: {error}");
            }
        }
    }
    let mut total_problems = 0;
    for (index, theater) in tables::THEATERS.iter().enumerate() {
        let terrain = Terrain::for_mission(
            resources,
            theater,
            Some(0),
            &Overrides {
                city_fill: false,
                ..overrides()?
            },
        )?;
        let mut audit = SiteAudit::default();
        let mut missions = 0;
        for layout in &layouts {
            let map = layout.map.as_deref().unwrap_or(&layout.resource);
            if tore_formats::theater::base_theater(map) != Some(*theater) {
                continue;
            }
            missions += 1;
            for p in &layout.placements {
                audit.placement(theater, &shapes, p, &terrain);
            }
        }
        // Check redrawn airfield pieces too, through exactly the scenery source.
        let drawn = Placements::for_terrain(resources, &terrain, theater)?;
        for p in drawn.placed() {
            audit.placement(theater, &drawn, p?.1, &terrain);
        }
        let site = SurfaceSite::load(resources, theater)?;
        let mut stems = tables::TEMPLATES[index].to_vec();
        if *theater == "UKR" {
            // The three unoffered Ukraine templates, documented in quick-templates.md.
            stems.extend(["QFACT", "QUBUNK", "QURADAR"]);
        }
        for stem in &stems {
            for variation in [Variation::OFF, Variation::ON] {
                let mut target = heavy(stem, index, 1);
                target.variation = variation;
                let surface = site.place(resources, &terrain.theater, Some(&target))?;
                let mut sources = Placements::of_types(resources, std::iter::empty());
                sources.add_surface(resources, &surface)?;
                for (_, p) in &surface.placements {
                    audit.placement(theater, &sources, p, &terrain);
                }
            }
        }
        println!(
            "grounding-sites: {theater} missions {missions} templates {} placements {} exceptions {} unavailable {} before {} problems {}",
            stems.len(),
            audit.placements,
            audit.exceptions,
            audit.unavailable,
            audit.before,
            audit.problems
        );
        let rule = |name| audit.rules.get(name).copied().unwrap_or(0);
        println!(
            "grounding-rules: {theater} slope {} origin flat {} origin reach {} origin gap {}",
            rule("slope"),
            rule("flat"),
            rule("reach"),
            rule("gap")
        );
        total_problems += audit.problems;
    }
    println!("grounding-sites: {binary} compiled campaign missions not read (no campaign runtime)");
    println!(
        "grounding-sites: {} theaters, {unread} unread resources, {total_problems} problems",
        tables::THEATERS.len()
    );
    if total_problems != 0 || unread != 0 {
        return Err(
            format!("{total_problems} grounding site problems, {unread} unread resources").into(),
        );
    }
    Ok(())
}

#[derive(Default)]
struct SiteAudit {
    /// Placements by the rule that stood them: `slope`, or the origin rule
    /// for a `flat` shape, one beyond the `reach` limit or the `gap` limit.
    rules: BTreeMap<&'static str, usize>,
    placements: usize,
    exceptions: usize,
    unavailable: usize,
    before: usize,
    problems: usize,
}

impl SiteAudit {
    fn placement(
        &mut self,
        theater: &str,
        sources: &tore_world::terrain::Placements,
        p: &tore_formats::mission::Placement,
        terrain: &Terrain,
    ) {
        use tore_world::grounding::{Footing, footing};
        if p.object_type.ends_with(".PT") {
            return;
        } // Airborne mission aircraft are not ground objects.
        let (Some(definition), Some(shape)) = (
            sources.definitions.get(&p.object_type),
            sources.shapes.get(&p.object_type),
        ) else {
            self.unavailable += 1;
            println!(
                "site-unavailable: {theater} {} #{} {}",
                p.key.layout, p.key.ordinal, p.object_type
            );
            return;
        };
        if footing(definition) != Footing::Bottom {
            self.exceptions += 1; // Pavement, map-tied bridges/roads, and waterlines keep authored support.
            return;
        }
        if p.position[1] != 0 {
            println!(
                "site-authored-height: {theater} {} #{} {} height {}",
                p.key.layout, p.key.ordinal, p.object_type, p.position[1]
            );
        }
        let height = |x: f64, z: f64| f64::from(terrain.height(x as f32, z as f32));
        let Some(stand) = sources.stance_on_terrain(p, height) else {
            self.unavailable += 1;
            return;
        };
        // Independent geometry measurement, including drawn line endpoints.
        let clearance = |origin: [f64; 3]| {
            tore_world::grounding::points(shape)
                .map(|point| {
                    let at: [f64; 3] = std::array::from_fn(|i| {
                        origin[i]
                            + stand.scale
                                * (stand.basis.right[i] * f64::from(point[0])
                                    + stand.basis.forward[i] * f64::from(point[1])
                                    + stand.basis.up[i] * f64::from(point[2]))
                    });
                    at[1] - height(at[0], at[2]) - f64::from(p.position[1])
                })
                .reduce(f64::min)
        };
        let old_bottom = shape
            .faces
            .iter()
            .flat_map(|f| &f.positions)
            .map(|p| f64::from(p[2]))
            .chain(
                shape
                    .billboards
                    .iter()
                    .map(|s| f64::from(s.center[2] - s.size[1] * 0.5)),
            )
            .reduce(f64::min)
            .unwrap_or(0.);
        let old_origin = std::array::from_fn(|i| {
            stand.support_origin[i] - stand.basis.up[i] * old_bottom * stand.scale
        });
        let before = clearance(old_origin).unwrap_or(0.);
        let after = clearance(stand.origin).unwrap_or(0.);
        // Which rule stood it, and why (tore_world::grounding::slope_lift_ft):
        // the full rigid lift and the gap it would leave, measured here again.
        use tore_world::grounding::{
            SLOPE_LIFT_MAX_GAP_FT, SLOPE_LIFT_MAX_REACH_FT, bottom_gap_ft, height_range, reach_ft,
            terrain_correction_ft,
        };
        let reach = reach_ft(shape, stand.scale);
        let mut on_ground = stand.origin;
        on_ground[1] -= stand.slope_lift + f64::from(p.position[1]);
        let lift = terrain_correction_ft(shape, stand.scale, stand.basis, on_ground, height);
        let mut lifted = on_ground;
        lifted[1] += lift;
        let gap = bottom_gap_ft(shape, stand.scale, stand.basis, lifted, height);
        let flat = height_range(shape).is_none_or(|[low, high]| high - low <= f64::EPSILON);
        let rule = if flat {
            "flat"
        } else if reach > SLOPE_LIFT_MAX_REACH_FT {
            "reach"
        } else if gap > SLOPE_LIFT_MAX_GAP_FT {
            "gap"
        } else {
            "slope"
        };
        self.placements += 1;
        *self.rules.entry(rule).or_default() += 1;
        if before.abs() > GROUNDING_TOLERANCE_FT
            || (rule != "slope" && lift.abs() > GROUNDING_TOLERANCE_FT)
        {
            self.before += 1;
            println!(
                "site-correction: {theater} {} #{} {} at {} {} height {} angles {:?} rule {rule} before {before:.2} after {after:.2} lift {lift:.2} gap {gap:.2} reach {reach:.0}",
                p.key.layout,
                p.key.ordinal,
                p.object_type,
                p.position[0],
                p.position[2],
                p.position[1],
                p.angles
            );
        }
        // The slope rule leaves nothing buried; the origin rule moves nothing
        // (a lift it refused must not have been applied).
        let wrong = if rule == "slope" {
            !after.is_finite()
                || after.abs() > GROUNDING_TOLERANCE_FT
                || (stand.slope_lift - lift).abs() > 1e-9
        } else {
            stand.slope_lift != 0.
        };
        if wrong {
            self.problems += 1;
            println!(
                "site-problem: {theater} {} #{} {} rule {rule} clearance {after:.2} slope-lift {:.2}",
                p.key.layout, p.key.ordinal, p.object_type, stand.slope_lift
            );
        }
    }
}

/// `--surface-dump --faces TYPE [...]`: every face of each named object
/// type's main shape, intact and damaged, with its texture, whether the
/// texture exists in the pack, its colour slots and its extent, so a
/// missing texture shows (the bridge's magenta patch, nr-fx2).
fn face_survey(resources: &BTreeMap<String, Vec<u8>>, named: &[String]) -> AppResult<()> {
    use tore_formats::shape::{DAMAGED_WORD, Shape};
    for name in named {
        let upper = name.to_ascii_uppercase();
        let upper = if upper.contains('.') {
            upper
        } else {
            format!("{upper}.OT")
        };
        let Some(bytes) = resources.get(&upper) else {
            println!("faces: {upper} not in the pack");
            continue;
        };
        let Some(shape_name) = tore_formats::static_object::Definition::parse(bytes)?.main_shape
        else {
            println!("faces: {upper} no shape");
            continue;
        };
        let Some(shape_bytes) = resources.get(&shape_name) else {
            println!("faces: {upper} shape {shape_name} missing");
            continue;
        };
        for (label, state) in [
            ("intact", BTreeMap::new()),
            ("damaged", BTreeMap::from([(DAMAGED_WORD, 1)])),
        ] {
            let shape = Shape::with_export_state(shape_bytes, &state)?;
            println!(
                "faces: {upper} {shape_name} {label}: {} faces",
                shape.faces.len()
            );
            for (n, face) in shape.faces.iter().enumerate() {
                let mut low = [f32::MAX; 3];
                let mut high = [f32::MIN; 3];
                for p in &face.positions {
                    for i in 0..3 {
                        low[i] = low[i].min(p[i]);
                        high[i] = high[i].max(p[i]);
                    }
                }
                let texture = if face.texture.is_empty() {
                    "-".to_owned()
                } else {
                    let found = resources.contains_key(&face.texture.to_ascii_uppercase());
                    format!("{}{}", face.texture, if found { "" } else { " MISSING" })
                };
                println!(
                    "faces:   {n:3} points {} texture {texture} uv {} colors {:?} fog {:?} subtype {} from ({:.0} {:.0} {:.0}) to ({:.0} {:.0} {:.0})",
                    face.positions.len(),
                    face.uv.len(),
                    face.colors,
                    face.fog,
                    face.subtype,
                    low[0],
                    low[1],
                    low[2],
                    high[0],
                    high[1],
                    high[2]
                );
            }
        }
    }
    Ok(())
}

/// `--surface-dump --damage [TYPE ...]`: where retail keeps the destroyed
/// look of every object type (or the named ones): its `~NAME.OT` variant,
/// the damage branch inside its main shape (an `ac` record followed with
/// `shape::DAMAGED_WORD` set), both or neither, with the intact and damaged
/// face counts and heights (docs/formats/objects-and-shapes.md, "Destroyed
/// looks of buildings and bridges").
fn damage_survey(resources: &BTreeMap<String, Vec<u8>>, named: &[String]) -> AppResult<()> {
    use tore_formats::shape::{DAMAGED_WORD, Shape};
    let wanted: Vec<String> = named.iter().map(|n| n.to_ascii_uppercase()).collect();
    let (mut total, mut branch, mut variant) = (0, 0, 0);
    let summary = |shape: &Shape| {
        let faces = shape.faces.len();
        let range = tore_world::grounding::height_range(shape).unwrap_or([0.; 2]);
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for face in &shape.faces {
            for p in &face.positions {
                for v in p {
                    h = (h ^ u64::from(v.to_bits())).wrapping_mul(0x0100_0000_01b3);
                }
            }
            h = (h ^ face.texture.len() as u64).wrapping_mul(0x0100_0000_01b3);
        }
        (faces, range, h)
    };
    for (name, bytes) in resources {
        let upper = name.to_ascii_uppercase();
        let chosen = if wanted.is_empty() {
            upper.ends_with(".OT")
        } else {
            wanted
                .iter()
                .any(|w| *w == upper || format!("{w}.OT") == upper)
        };
        if !chosen {
            continue;
        }
        let Ok(definition) = tore_formats::static_object::Definition::parse(bytes) else {
            continue;
        };
        let Some(shape_name) = definition.main_shape.clone() else {
            println!("damage: {name} no shape");
            continue;
        };
        let Some(shape_bytes) = resources.get(&shape_name) else {
            println!("damage: {name} shape {shape_name} missing");
            continue;
        };
        total += 1;
        let intact = Shape::with_export_state(shape_bytes, &BTreeMap::new());
        let damaged = Shape::with_export_state(shape_bytes, &BTreeMap::from([(DAMAGED_WORD, 1)]));
        let tilde = format!("~{upper}");
        let has_variant = !upper.starts_with('~') && resources.contains_key(&tilde);
        if has_variant {
            variant += 1;
        }
        let (intact, damaged) = match (intact, damaged) {
            (Ok(i), Ok(d)) => (i, d),
            (i, d) => {
                println!(
                    "damage: {name} shape {shape_name} unreadable intact {:?} damaged {:?}",
                    i.err().map(|e| e.to_string()),
                    d.err().map(|e| e.to_string())
                );
                continue;
            }
        };
        let (fi, ri, hi) = summary(&intact);
        let (fd, rd, hd) = summary(&damaged);
        let differs = hi != hd;
        if differs {
            branch += 1;
        }
        println!(
            "damage: {name} shape {shape_name} cat {:#06x} hp {} exp {} crater {} intact {fi} faces {:.0}..{:.0} damaged {fd} faces {:.0}..{:.0} branch {} variant {}",
            definition.category,
            definition
                .hit_points
                .map_or("-".into(), |hp| hp.to_string()),
            definition.explosion,
            definition.crater,
            ri[0],
            ri[1],
            rd[0],
            rd[1],
            if differs { "yes" } else { "no" },
            if has_variant { tilde.as_str() } else { "-" },
        );
    }
    println!("damage: {total} types, {branch} with a damage branch, {variant} with a ~ variant");
    Ok(())
}

fn side(surface_side: tore_sim::combat::live::Side) -> &'static str {
    match surface_side.0 {
        1 => "blue",
        2 => "red",
        _ => "neutral",
    }
}

/// The layout's lines: the template's anchor and move, every battery and
/// supply truck, the starts and what could not be added.
fn print_layout(surface: &Surface) {
    if let Some(site) = &surface.template {
        let t = surface.transform;
        println!(
            "surface: layout anchor {} jitter {} relocate {} transform rotation {} translation {} {} pivot {} {} moved-ft {}",
            site.anchor.map_or("none", layout::Anchor::name),
            u8::from(site.settings.variation.jitter),
            u8::from(site.settings.variation.relocate),
            t.rotation_deg,
            t.translation[0],
            t.translation[1],
            t.pivot[0],
            t.pivot[1],
            moved_ft(surface),
        );
    }
    for battery in &surface.batteries {
        let radar = surface.unit(battery.radar);
        println!(
            "surface: battery {:?} side {} radar {:#010x} {} {} launchers {} truck {}",
            battery.system,
            side(battery.side),
            battery.radar.0,
            radar.map_or("?", |u| u.resource.as_str()),
            if battery.radar_added {
                "added"
            } else {
                "adopted"
            },
            battery
                .launchers
                .iter()
                .map(|id| format!("{:#010x}", id.0))
                .collect::<Vec<_>>()
                .join(","),
            battery
                .truck
                .map_or("none".into(), |id| format!("{:#010x}", id.0)),
        );
    }
    for truck in &surface.trucks {
        let unit = surface.unit(truck.id);
        let served = truck.serves.and_then(|id| surface.unit(id));
        let gap = match (unit, served) {
            (Some(u), Some(s)) => {
                let d = [
                    f64::from(u.position[0] - s.position[0]),
                    f64::from(u.position[2] - s.position[2]),
                ];
                format!(" gap-ft {:.0}", d[0].hypot(d[1]))
            }
            _ => String::new(),
        };
        println!(
            "surface: truck {:#010x} {} {} serves {}{gap}",
            truck.id.0,
            unit.map_or("?", |u| u.resource.as_str()),
            if truck.added { "added" } else { "standing" },
            truck
                .serves
                .map_or("any".into(), |id| format!("{:#010x}", id.0)),
        );
    }
    if let Some(starts) = &surface.starts {
        let nm = |a: [i32; 2], b: [i32; 2]| {
            f64::from(a[0] - b[0]).hypot(f64::from(a[1] - b[1])) / layout::NM_FT as f64
        };
        println!(
            "surface: starts target {} {} red {} {} red-target-nm {:.1} blue {} {} blue-red-nm {:.1} heading {} blue-airfields {} red-airfields {}",
            starts.target[0],
            starts.target[1],
            starts.red[0],
            starts.red[1],
            nm(starts.red, starts.target),
            starts.blue[0],
            starts.blue[1],
            nm(starts.blue, starts.red),
            starts.blue_heading_deg,
            ids(&starts.blue_airfields),
            ids(&starts.red_airfields),
        );
    }
    for note in &surface.layout_notes {
        println!("surface: layout note: {note}");
    }
}

fn ids(list: &[u32]) -> String {
    if list.is_empty() {
        return "none".into();
    }
    list.iter()
        .map(|id| format!("{id:#010x}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// How far the relocation moved the template's targets' centroid, feet.
fn moved_ft(surface: &Surface) -> i64 {
    let t = surface.transform.translation.map(i64::from);
    layout::trig::isqrt((t[0] * t[0] + t[1] * t[1]) as u128) as i64
}

fn print_surface(theater: &str, surface: &Surface) {
    match &surface.template {
        Some(site) => println!(
            "surface: theater {theater} template {} aaa {} sam {} seed {} nationality {} group {} objects {}",
            site.stem,
            site.settings.aaa,
            site.settings.sam,
            site.settings.seed,
            site.settings.enemy_nationality,
            site.group,
            site.objects
        ),
        None => println!("surface: theater {theater} no ground target"),
    }
    for unit in &surface.units {
        let placeholder = match &unit.origin {
            tore_world::surface::Origin::Template {
                placeholder: Some(p),
                ..
            } => format!(" slot <{}>", p.name()),
            Origin::Added => " added".into(),
            _ => String::new(),
        };
        println!(
            "surface: unit {:#010x} {} {:?} class {:#06x} side {} nationality {} pos {} {} {} angle {} target {} skill {} hp {} look {:?} scene {}{placeholder}",
            unit.id.0,
            unit.resource,
            unit.kind,
            unit.class,
            side(unit.side),
            unit.nationality.unwrap_or(-1),
            unit.position[0],
            unit.position[1],
            unit.position[2],
            unit.angles[0],
            u8::from(unit.is_target()),
            unit.skill,
            unit.hit_points,
            unit.look,
            u8::from(unit.in_scene),
        );
    }
    for parked in &surface.parked {
        println!(
            "surface: parked {:#010x} {} side {} pos {} {} {} target {}",
            parked.id.0,
            parked.resource,
            side(parked.side),
            parked.position[0],
            parked.position[1],
            parked.position[2],
            u8::from(parked.target)
        );
    }
    if let Some(site) = &surface.template {
        for ordinal in &site.removed {
            println!(
                "surface: removed {:#010x}",
                UnitId::template(*ordinal).map_or(0, |id| id.0)
            );
        }
        for left in &site.left_out {
            println!(
                "surface: left-out {:#010x} {} ({})",
                UnitId::template(left.ordinal).map_or(0, |id| id.0),
                left.resource,
                left.why
            );
        }
    }
    if let Some(why) = &surface.unresolved {
        println!("surface: unresolved ground target: {why}");
    }
    print_layout(surface);
    for (name, why) in &surface.unreadable {
        println!("surface: unreadable {name}: {why}");
    }
    let count = |range| {
        surface
            .units
            .iter()
            .filter(|u| u.id.range() == range)
            .count()
    };
    println!(
        "surface: totals units {} layout {} template {} parked {} trucks {} added-trucks {} batteries {} added-radars {} area {} targets {} not-drawn {} unreadable {}",
        surface.units.len(),
        count(IdRange::Layout),
        count(IdRange::Template),
        surface.parked.len(),
        surface.trucks.len(),
        count(IdRange::SupplyTruck),
        surface.batteries.len(),
        count(IdRange::BatteryRadar),
        count(IdRange::AreaDefense),
        surface.targets().count(),
        surface.units.iter().filter(|u| !u.in_scene).count(),
        surface.unreadable.len()
    );
    println!("surface: digest {:#018x}", surface.digest());
}

/// `--surface-dump --all`: see the module comment.
fn resolve_all(resources: &BTreeMap<String, Vec<u8>>, seeds: u32) -> AppResult<()> {
    let mut errors = 0usize;
    let mut resolutions = 0usize;
    let mut templates = 0usize;
    for (index, theater) in tables::THEATERS.iter().enumerate() {
        // The base layout's air defenses, by side.
        let base = Terrain::for_mission(resources, theater, Some(0), &overrides()?)?;
        let defenses = |side_id: u32| {
            base.surface
                .units
                .iter()
                .filter(|u| u.side.0 == side_id && u.class & (class::SAM | class::AAA) != 0)
                .count()
        };
        println!(
            "surface-base: {theater} units {} sam-aaa-red {} sam-aaa-blue {} trucks {} not-drawn {} digest {:#018x}",
            base.surface.units.len(),
            defenses(2),
            defenses(1),
            base.surface.trucks.len(),
            base.surface.units.iter().filter(|u| !u.in_scene).count(),
            base.surface.digest()
        );
        let nationality = tables::ENEMY_NATIONALITY[index];
        for stem in tables::TEMPLATES[index] {
            templates += 1;
            let name = format!("~{stem}.M");
            let template = match resources
                .get(&name)
                .ok_or_else(|| format!("missing {name}"))
                .and_then(|bytes| Template::parse(&name, bytes).map_err(|e| e.to_string()))
            {
                Ok(template) => template,
                Err(error) => {
                    errors += 1;
                    println!("surface-all: error {theater} {stem}: {error}");
                    continue;
                }
            };
            let slots = |which: Placeholder| {
                template
                    .objects
                    .iter()
                    .filter(|o| o.placeholder() == Some(which))
                    .count()
            };
            let (sam_slots, aaa_slots) = (slots(Placeholder::Sam), slots(Placeholder::Aaa));
            for level in 0..4 {
                for seed in 1..=seeds {
                    resolutions += 1;
                    let target = GroundTarget {
                        stem: (*stem).to_owned(),
                        aaa: level,
                        sam: level,
                        seed,
                        enemy_nationality: nationality,
                        night_stealth: false,
                        variation: Variation::ON,
                        separation_nm: DUMP_SEPARATION_NM,
                    };
                    let mut catalog = Catalog::new(resources);
                    let resolved = match resolve::template(
                        &template,
                        &target,
                        &mut catalog,
                        Some(base.environment.map.as_str()),
                    )
                    .and_then(|t| resolve::surface(Default::default(), Some(t)))
                    {
                        Ok(surface) => surface,
                        Err(error) => {
                            errors += 1;
                            println!(
                                "surface-all: error {theater} {stem} level {level} seed {seed}: {error}"
                            );
                            continue;
                        }
                    };
                    let site = resolved.template.as_ref().expect("a template site");
                    let manned = resolved
                        .units
                        .iter()
                        .filter(|u| {
                            matches!(&u.origin, tore_world::surface::Origin::Template { placeholder: Some(p), .. } if p.is_defense())
                        })
                        .count();
                    println!(
                        "surface-all: {theater} {stem} level {level} seed {seed} objects {} sam-slots {sam_slots} aaa-slots {aaa_slots} targets {} manned {manned} removed {} units {} parked {} left-out {} digest {:#018x}",
                        site.objects,
                        resolved.targets().count(),
                        site.removed.len(),
                        resolved.units.len(),
                        resolved.parked.len(),
                        site.left_out.len(),
                        resolved.digest()
                    );
                }
            }
            // The whole scene with the template at heavy defenses: every type
            // reads, and its units join the airport scene.
            let target = GroundTarget {
                stem: (*stem).to_owned(),
                aaa: 3,
                sam: 3,
                seed: 1,
                enemy_nationality: nationality,
                night_stealth: false,
                variation: Variation::ON,
                separation_nm: DUMP_SEPARATION_NM,
            };
            match Terrain::for_mission_with(
                resources,
                theater,
                Some(0),
                &overrides()?,
                Some(&target),
            ) {
                Ok(built) => {
                    let surface = &built.surface;
                    let not_drawn: Vec<&str> = surface
                        .template_units()
                        .filter(|u| !u.in_scene)
                        .map(|u| u.resource.as_str())
                        .collect();
                    println!(
                        "surface-scene: {theater} {stem} units {} drawn {} not-drawn {} {}",
                        surface.template_units().count(),
                        surface.template_units().filter(|u| u.in_scene).count(),
                        not_drawn.len(),
                        summary(&not_drawn)
                    );
                }
                Err(error) => {
                    errors += 1;
                    println!("surface-all: error {theater} {stem} scene: {error}");
                }
            }
        }
    }
    println!("surface-all: {resolutions} resolutions, {templates} templates, {errors} errors");
    if errors > 0 {
        return Err(format!("{errors} surface resolutions failed").into());
    }
    Ok(())
}

/// The stems `--sweep` and `--starts` cover: every offered template but the
/// "nothing" ones, of the named theaters or of all.
fn jobs(wanted: &[String]) -> AppResult<Vec<(usize, &'static str, Vec<&'static str>)>> {
    let mut out = Vec::new();
    for (index, theater) in tables::THEATERS.iter().enumerate() {
        if !wanted.is_empty() && !wanted.iter().any(|w| w.eq_ignore_ascii_case(theater)) {
            continue;
        }
        let stems = tables::TEMPLATES[index]
            .iter()
            .copied()
            .filter(|stem| !stem.ends_with("NOTH"))
            .collect();
        out.push((index, *theater, stems));
    }
    if out.is_empty() {
        return Err(format!("no theater among {wanted:?}").into());
    }
    Ok(out)
}

fn heavy(stem: &str, index: usize, seed: u32) -> GroundTarget {
    GroundTarget {
        stem: stem.to_owned(),
        aaa: 3,
        sam: 3,
        seed,
        enemy_nationality: tables::ENEMY_NATIONALITY[index],
        night_stealth: false,
        variation: Variation::ON,
        separation_nm: DUMP_SEPARATION_NM,
    }
}

/// `--surface-dump --sweep [--seeds N] [THEATER...]`: every offered
/// template of every (named) theater placed at heavy defenses with seeds 1
/// to N (20 by default): its anchor and move, the site rules it breaks
/// ([`Terrain::audit_surface`]'s), its batteries and trucks against their
/// rules, and its digest; each theater's base-layout batteries. For the
/// `surface-relocate-sweep` scenario.
fn relocate_sweep(
    resources: &BTreeMap<String, Vec<u8>>,
    seeds: u32,
    wanted: &[String],
) -> AppResult<()> {
    let (mut placements, mut relocated, mut problems, mut errors) =
        (0usize, 0usize, 0usize, 0usize);
    for (index, theater, stems) in jobs(wanted)? {
        let terrain = Terrain::for_mission(resources, theater, Some(0), &overrides()?)?;
        let base = &terrain.surface;
        let count = |system| base.batteries.iter().filter(|b| b.system == system).count();
        use tore_world::surface::BatterySystem as S;
        let in_battery: usize = base.batteries.iter().map(|b| b.launchers.len()).sum();
        let launchers = base
            .units
            .iter()
            .filter(|u| S::of_launcher(&u.resource).is_some() && u.side.0 != 0)
            .count();
        println!(
            "surface-base-batteries: {theater} batteries {} sa2 {} sa3 {} sa6 {} hawk {} adopted {} added {} launchers {launchers} in-batteries {in_battery} notes {} digest {:#018x}",
            base.batteries.len(),
            count(S::Sa2),
            count(S::Sa3),
            count(S::Sa6),
            count(S::Hawk),
            base.batteries.iter().filter(|b| !b.radar_added).count(),
            base.batteries.iter().filter(|b| b.radar_added).count(),
            base.layout_notes.len(),
            base.digest(),
        );
        let site = tore_world::terrain::SurfaceSite::load(resources, theater)?;
        for stem in stems {
            for seed in 1..=seeds {
                placements += 1;
                let surface = match site.place(
                    resources,
                    &terrain.theater,
                    Some(&heavy(stem, index, seed)),
                ) {
                    Ok(surface) => surface,
                    Err(error) => {
                        errors += 1;
                        println!("surface-sweep: error {theater} {stem} seed {seed}: {error}");
                        continue;
                    }
                };
                let mut broken = site.audit(resources, &terrain.theater, &surface);
                broken.extend(rule_problems(&surface));
                let anchor = surface.template.as_ref().and_then(|t| t.anchor);
                if anchor.is_some() && !surface.transform.is_identity() {
                    broken.push("an anchored template moved".into());
                }
                if !surface.transform.is_identity() {
                    relocated += 1;
                }
                problems += broken.len();
                for problem in &broken {
                    println!("surface-sweep-problem: {theater} {stem} seed {seed}: {problem}");
                }
                println!(
                    "surface-sweep: {theater} {stem} seed {seed} anchor {} moved-ft {} rotation {} units {} trucks {} batteries {} problems {} digest {:#018x}",
                    anchor.map_or("none", layout::Anchor::name),
                    moved_ft(&surface),
                    surface.transform.rotation_deg,
                    surface.template_units().count(),
                    surface
                        .units
                        .iter()
                        .filter(|u| u.id.range() == IdRange::SupplyTruck)
                        .count(),
                    surface.batteries.len(),
                    broken.len(),
                    surface.digest(),
                );
            }
        }
    }
    println!(
        "surface-sweep: {placements} placements, {relocated} relocated, {problems} problems, {errors} errors"
    );
    if errors > 0 {
        return Err(format!("{errors} placements failed").into());
    }
    Ok(())
}

/// `--surface-dump --classes [--seeds N] [THEATER...]`: the terrain class
/// rule ([`Terrain::audit_classes`]) over each theater's base layout and
/// every offered template placed at heavy defenses with seeds 1 to N (20),
/// and at its retail spot (no jitter, no relocation) with a `retail` tag so
/// a fault in the data tells from one the placement made. For the
/// `surface-class-sweep` scenario.
fn class_sweep(
    resources: &BTreeMap<String, Vec<u8>>,
    seeds: u32,
    wanted: &[String],
) -> AppResult<()> {
    let (mut placements, mut problems) = (0usize, 0usize);
    for (index, theater, stems) in jobs(wanted)? {
        let terrain = Terrain::for_mission(resources, theater, Some(0), &overrides()?)?;
        let site = tore_world::terrain::SurfaceSite::load(resources, theater)?;
        let base = terrain.audit_classes(resources)?;
        placements += 1;
        problems += base.len();
        for problem in &base {
            println!("surface-class-problem: {theater} base layout: {problem}");
        }
        for stem in stems {
            let mut retail_target = heavy(stem, index, 1);
            retail_target.variation = Variation::OFF;
            let retail = site.place(resources, &terrain.theater, Some(&retail_target))?;
            for seed in 1..=seeds {
                let target = heavy(stem, index, seed);
                let surface = site.place(resources, &terrain.theater, Some(&target))?;
                let broken =
                    site.audit_classes(resources, &terrain.theater, &surface, Some(&retail));
                placements += 1;
                problems += broken.len();
                for problem in &broken {
                    println!("surface-class-problem: {theater} {stem} seed {seed}: {problem}");
                }
            }
        }
    }
    println!("surface-class-sweep: {placements} placements, {problems} problems");
    if problems > 0 {
        return Err(format!("{problems} units on the wrong terrain class").into());
    }
    Ok(())
}

/// The battery and truck rules a placed surface breaks: battery sizes
/// against their caps, one added truck per manned slot and per template
/// battery, each 200 to 400 ft from the unit it serves.
fn rule_problems(surface: &Surface) -> Vec<String> {
    let mut out = Vec::new();
    for battery in &surface.batteries {
        if battery.launchers.len() > battery.system.cap() {
            out.push(format!(
                "{:?} battery of {:#010x} has {} launchers",
                battery.system,
                battery.launchers[0].0,
                battery.launchers.len()
            ));
        }
        if battery.launchers[0].range() == IdRange::Template && battery.truck.is_none() {
            out.push(format!(
                "battery of {:#010x} has no truck",
                battery.launchers[0].0
            ));
        }
    }
    let slots = surface
        .template_units()
        .filter(|u| {
            matches!(&u.origin, Origin::Template { placeholder: Some(p), .. } if p.is_defense())
        })
        .count();
    let template_batteries = surface
        .batteries
        .iter()
        .filter(|b| b.launchers[0].range() == IdRange::Template)
        .count();
    let added = surface.trucks.iter().filter(|t| t.added).count();
    if added != slots + template_batteries && surface.layout_notes.is_empty() {
        out.push(format!(
            "{added} added trucks for {slots} manned slots and {template_batteries} batteries"
        ));
    }
    for truck in surface.trucks.iter().filter(|t| t.added) {
        let (Some(unit), Some(served)) = (
            surface.unit(truck.id),
            truck.serves.and_then(|id| surface.unit(id)),
        ) else {
            continue;
        };
        let d2 = (i64::from(unit.position[0] - served.position[0])).pow(2)
            + (i64::from(unit.position[2] - served.position[2])).pow(2);
        let [lo, hi] = layout::TRUCK_FT;
        if d2 < (lo - 1).pow(2) || d2 > (hi + 1).pow(2) {
            out.push(format!(
                "truck {:#010x} stands {d2} sq ft from {:#010x}",
                truck.id.0, served.id.0
            ));
        }
    }
    out
}

/// `--surface-dump --starts [THEATER...]`: for one template per theater
/// (the first that relocates with seed 1, else the first), builds the
/// mission with a ground target, an airborne start at 20,000 ft and a 50 nm
/// enemy distance, and prints where Blue and Red start: Red's aircraft from
/// the target, Blue from Red and its bearing off the line toward its own
/// side, Blue's heading off the target, and whether both are on the map. For
/// the `surface-start-placement` scenario.
fn start_placement(resources: &BTreeMap<String, Vec<u8>>, wanted: &[String]) -> AppResult<()> {
    use tore_world::{
        mission::{Defense, MissionSpec, Skill, Start},
        world::{Seating, World},
    };
    let mut checked = 0;
    for (index, theater, stems) in jobs(wanted)? {
        let terrain = Terrain::for_mission(resources, theater, Some(0), &overrides()?)?;
        let site = tore_world::terrain::SurfaceSite::load(resources, theater)?;
        let stem = stems
            .iter()
            .copied()
            .find(|stem| {
                site.place(resources, &terrain.theater, Some(&heavy(stem, index, 1)))
                    .is_ok_and(|s| !s.transform.is_identity())
            })
            .unwrap_or(stems[0]);
        let mut spec = MissionSpec::new(theater, tore_formats::aircraft::AircraftId::F18);
        spec.ground_target = Some(stem.to_owned());
        spec.aaa = Defense::Heavy;
        spec.sam = Defense::Heavy;
        spec.surface_seed = 1;
        spec.enemy_nationality = tables::ENEMY_NATIONALITY[index] as u8;
        spec.wings[3].count = 1;
        spec.wings[3].skill = Skill::Average;
        spec.separation_nm = 50;
        spec.start = Start::Airborne {
            altitude_ft: 20_000,
        };
        let world = World::new(&spec, resources, Seating::SinglePlayer)?;
        let Some(starts) = world.terrain.surface.starts.clone() else {
            println!("surface-start: {theater} {stem} no starts");
            continue;
        };
        let nm = layout::NM_FT as f64;
        let target = starts.target.map(f64::from);
        let blue = world.cockpits[0].flight.position;
        let yaw = world.cockpits[0].flight.yaw;
        let red = world
            .combat
            .state
            .targets
            .iter()
            .filter(|t| t.side == tore_world::ai_wings::ENEMY_SIDE && t.aircraft.is_some())
            .map(|t| t.position)
            .next();
        let angle = |v: [f64; 2]| v[0].atan2(v[1]).to_degrees();
        let off = |a: f64, b: f64| ((a - b + 540.).rem_euclid(360.) - 180.).abs();
        // Red's aircraft from the target; Blue from Red, on which side; Blue's
        // heading against the line to the target.
        let red_target_nm = red.map_or(-1., |r| (r[0] - target[0]).hypot(r[2] - target[1]) / nm);
        let from_red = red.map_or([0.; 2], |r| [blue[0] - r[0], blue[2] - r[2]]);
        let blue_red_nm = from_red[0].hypot(from_red[1]) / nm;
        let side_off = site.front().map_or(0., |front| {
            let toward = [
                (front.blue[0] - front.red[0]) as f64,
                (front.blue[1] - front.red[1]) as f64,
            ];
            off(angle(from_red), angle(toward))
        });
        let heading_off = off(
            yaw.to_degrees(),
            angle([target[0] - blue[0], target[1] - blue[2]]),
        );
        let extent = [
            (terrain.theater.cols as f64 - 1.) * f64::from(tore_formats::theater::CELL_FEET),
            (terrain.theater.rows as f64 - 1.) * f64::from(tore_formats::theater::CELL_FEET),
        ];
        let on_map =
            |p: [f64; 3]| (0. ..=extent[0]).contains(&p[0]) && (0. ..=extent[1]).contains(&p[2]);
        println!(
            "surface-start: {theater} {stem} red-target-nm {red_target_nm:.2} blue-red-nm {blue_red_nm:.2} separation-nm {} side-off-deg {side_off:.1} heading-off-deg {heading_off:.1} blue-on-map {} red-on-map {} front {}",
            spec.separation_nm,
            u8::from(on_map(blue)),
            u8::from(red.is_some_and(on_map)),
            u8::from(site.front().is_some()),
        );
        checked += 1;
    }
    println!("surface-start: {checked} theaters");
    Ok(())
}

/// `T72.NT x3, SA3.NT x9`.
fn summary(names: &[&str]) -> String {
    let mut counts = BTreeMap::<&str, usize>::new();
    for name in names {
        *counts.entry(name).or_default() += 1;
    }
    counts
        .iter()
        .map(|(name, n)| format!("{name} x{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `--surface-sheets`: see the module comment.
pub fn sheets() -> AppResult<()> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let Some((out, wanted)) = args.split_first() else {
        return Err("--surface-sheets OUTPUT_DIRECTORY [--variants] [THEATER[:STEM] ...]".into());
    };
    // `--variants`: the retail spot (no jitter, no relocation) and two
    // seeds with both on; otherwise the retail spot alone.
    let all_variants = wanted.iter().any(|w| w == "--variants");
    let wanted: Vec<String> = wanted
        .iter()
        .filter(|w| *w != "--variants")
        .cloned()
        .collect();
    let wanted = wanted.as_slice();
    let variants: &[(&str, Variation, u32)] = if all_variants {
        &[
            ("retail", Variation::OFF, 1),
            ("seed1", Variation::ON, 1),
            ("seed2", Variation::ON, 2),
        ]
    } else {
        &[("retail", Variation::OFF, 1)]
    };
    let out = Path::new(out);
    std::fs::create_dir_all(out)?;
    let resources = resources()?;
    let mut jobs: Vec<(String, Vec<String>)> = Vec::new();
    if wanted.is_empty() {
        for (index, theater) in tables::THEATERS.iter().enumerate() {
            jobs.push((
                (*theater).to_owned(),
                tables::TEMPLATES[index]
                    .iter()
                    .filter(|stem| !stem.ends_with("NOTH"))
                    .map(|s| (*s).to_owned())
                    .collect(),
            ));
        }
    } else {
        for item in wanted {
            let (theater, stem) = item.split_once(':').unwrap_or((item, ""));
            let index = theater_index(theater)?;
            let stems = if stem.is_empty() {
                tables::TEMPLATES[index]
                    .iter()
                    .filter(|stem| !stem.ends_with("NOTH"))
                    .map(|s| (*s).to_owned())
                    .collect()
            } else {
                vec![stem.to_ascii_uppercase()]
            };
            jobs.push((theater.to_ascii_uppercase(), stems));
        }
    }
    let mut gpu: Option<Gpu> = None;
    for (theater, stems) in jobs {
        let index = theater_index(&theater)?;
        for (stem, (variant, variation, seed)) in stems
            .iter()
            .flat_map(|stem| variants.iter().map(move |variant| (stem.clone(), *variant)))
        {
            let target = GroundTarget {
                stem: stem.clone(),
                aaa: 3,
                sam: 3,
                seed,
                enemy_nationality: tables::ENEMY_NATIONALITY[index],
                night_stealth: false,
                variation,
                separation_nm: DUMP_SEPARATION_NM,
            };
            let world = Terrain::for_mission_with(
                &resources,
                &theater,
                Some(0),
                &overrides()?,
                Some(&target),
            )?;
            let mut scenery = Scenery::build(&resources, &world)?;
            let gpu = match &mut gpu {
                Some(gpu) => {
                    // Another theater's land and sky on the same device.
                    gpu.sim = crate::sim_renderer::SimRenderer::new(
                        &gpu.device,
                        &gpu.queue,
                        wgpu::TextureFormat::Rgba8UnormSrgb,
                        &scenery,
                        crate::graphics::Options::default(),
                        4,
                    );
                    gpu
                }
                None => gpu.insert(pollster::block_on(Gpu::new(&scenery))?),
            };
            let surface = &world.surface;
            let points: Vec<([f64; 3], Marker)> = marks(&world, surface);
            if points.is_empty() {
                continue;
            }
            for (view, pitch, scale) in [("top", -70.0, 1.0), ("close", -38.0, 0.45)] {
                let camera = overhead(&points, pitch, scale);
                scenery.resolve_palette(&world, camera.position[1]);
                scenery.set_origin(camera.position);
                let hidden = std::collections::BTreeSet::new();
                let geometry = std::sync::Arc::clone(scenery.static_geometry_where(&hidden));
                gpu.sim.airports(&gpu.device, &gpu.queue, &geometry);
                let mut pixels = gpu.pixels(&camera, &world, &scenery)?;
                for (point, marker) in &points {
                    if let Some(at) = camera.project([WIDTH, HEIGHT], *point) {
                        marker.draw(&mut pixels, at, view == "top");
                    }
                }
                let path = out.join(format!("{theater}-{stem}-{variant}-{view}.png"));
                std::fs::write(
                    &path,
                    crate::replay::png::encode_rgba(WIDTH, HEIGHT, &pixels)?,
                )?;
                println!(
                    "surface sheet: {} ({} units, {} parked, {} not drawn, digest {:#018x})",
                    path.display(),
                    surface.template_units().count(),
                    surface.parked.len(),
                    surface.template_units().filter(|u| !u.in_scene).count(),
                    surface.digest()
                );
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Marker {
    color: [u8; 3],
    ring: bool,
}

impl Marker {
    /// Draws the marker at pixel `at`; a `full` dot, or a small one beside
    /// the unit in a close view so the shape stays visible.
    fn draw(&self, pixels: &mut [u8], at: [f64; 2], full: bool) {
        let [mut cx, mut cy] = at.map(|v| v.round() as i32);
        if !full {
            cx += 14;
            cy -= 14;
        }
        let radius: i32 = match (self.ring, full) {
            (true, true) => 9,
            (true, false) => 6,
            (false, true) => 4,
            (false, false) => 3,
        };
        let inner = if full { 49 } else { 16 };
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let d2 = dx * dx + dy * dy;
                let inside = if self.ring {
                    (inner..=radius * radius).contains(&d2)
                } else {
                    d2 <= radius * radius
                };
                let (x, y) = (cx + dx, cy + dy);
                if !inside || x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
                    continue;
                }
                let at = ((y as u32 * WIDTH + x as u32) * 4) as usize;
                pixels[at..at + 3].copy_from_slice(&self.color);
                pixels[at + 3] = 255;
            }
        }
    }
}

/// A marker per template unit and parked aircraft, at its ground point.
fn marks(world: &Terrain, surface: &Surface) -> Vec<([f64; 3], Marker)> {
    let ground = |p: [i32; 3]| {
        let (x, z) = (f64::from(p[0]), f64::from(p[2]));
        [x, f64::from(world.height(x as f32, z as f32)) + 20., z]
    };
    let mut out = Vec::new();
    let radars: std::collections::BTreeSet<UnitId> =
        surface.batteries.iter().map(|b| b.radar).collect();
    let launchers: std::collections::BTreeSet<UnitId> = surface
        .batteries
        .iter()
        .flat_map(|b| b.launchers.iter().copied())
        .collect();
    let template_or_added = surface
        .units
        .iter()
        .filter(|u| u.id.range() != IdRange::Layout || radars.contains(&u.id));
    for unit in template_or_added {
        let color = if !unit.in_scene {
            [255, 0, 255]
        } else if radars.contains(&unit.id) {
            [0, 230, 230]
        } else if launchers.contains(&unit.id) {
            [255, 150, 0]
        } else if unit.supply_truck {
            [40, 220, 60]
        } else if unit.side.0 == 1 {
            [40, 120, 255]
        } else {
            [235, 40, 40]
        };
        out.push((ground(unit.position), Marker { color, ring: false }));
        if unit.is_target() {
            out.push((
                ground(unit.position),
                Marker {
                    color: [255, 255, 255],
                    ring: true,
                },
            ));
        }
    }
    for parked in &surface.parked {
        out.push((
            ground(parked.position),
            Marker {
                color: [255, 220, 0],
                ring: parked.target,
            },
        ));
    }
    out
}

/// A camera over the units at `pitch_deg` (down), looking north, far
/// enough back to frame all of them, then brought in by `scale`.
fn overhead(points: &[([f64; 3], Marker)], pitch_deg: f64, scale: f64) -> Camera {
    let (mut min, mut max) = ([f64::MAX; 2], [f64::MIN; 2]);
    let mut top = f64::MIN;
    for (p, _) in points {
        min = [min[0].min(p[0]), min[1].min(p[2])];
        max = [max[0].max(p[0]), max[1].max(p[2])];
        top = top.max(p[1]);
    }
    let center = [(min[0] + max[0]) / 2., (min[1] + max[1]) / 2.];
    let span = (max[0] - min[0])
        .max((max[1] - min[1]) * 16. / 9.)
        .max(2000.);
    let pitch = pitch_deg.to_radians();
    // The view is 60 degrees high, so tan(half width) is (16 / 9) / sqrt(3).
    let half_width = 16. / 9. / 3f64.sqrt();
    let distance = (span * 1.15 / 2. + 500.) / half_width * scale;
    let mut camera = Camera::new();
    camera.yaw = 0.;
    camera.pitch = pitch as f32;
    camera.position = [
        center[0],
        top + distance * -pitch.sin(),
        center[1] - distance * pitch.cos(),
    ];
    camera
}

/// `--surface-dump --area-sweep`: see the module comment.
fn area_sweep_run(
    resources: &BTreeMap<String, Vec<u8>>,
    seeds: u32,
    png: Option<&Path>,
    wanted: &[String],
) -> AppResult<()> {
    let (mut builds, mut problems, mut short) = (0usize, 0usize, 0usize);
    for (index, theater, _) in jobs(wanted)? {
        let terrain = Terrain::for_mission(resources, theater, Some(0), &overrides()?)?;
        let site = tore_world::terrain::SurfaceSite::load(resources, theater)?;
        let front = site.front();
        for seed in 1..=seeds {
            for level in 1..=3usize {
                let area = resolve::AreaDefense {
                    aaa: level,
                    sam: level,
                    seed,
                    enemy_nationality: tables::ENEMY_NATIONALITY[index],
                    night_stealth: false,
                    rolled: true,
                };
                let surface = site.place_with(resources, &terrain.theater, None, Some(&area))?;
                builds += 1;
                let broken = site.audit_area(resources, &terrain.theater, &surface);
                problems += broken.len();
                for problem in &broken {
                    println!(
                        "surface-area-problem: {theater} seed {seed} level {level}: {problem}"
                    );
                }
                let units: Vec<_> = layout::area::units(&surface).collect();
                let (sam_sites, aaa_sites) = site.area_site_counts(&area);
                let wanted_units = (sam_sites + aaa_sites) * layout::AREA_SITE_UNITS;
                if front.is_some() && units.len() < wanted_units {
                    short += 1;
                }
                let depths: Vec<f64> = match front {
                    Some(f) => units
                        .iter()
                        .map(|u| {
                            let axis = f.axis();
                            let len = (axis[0] as f64).hypot(axis[1] as f64);
                            let mid = [
                                (f.blue[0] + f.red[0]) as f64 / 2.,
                                (f.blue[1] + f.red[1]) as f64 / 2.,
                            ];
                            ((f64::from(u.position[0]) - mid[0]) * axis[0] as f64
                                + (f64::from(u.position[2]) - mid[1]) * axis[1] as f64)
                                / len
                                / layout::NM_FT as f64
                        })
                        .collect(),
                    None => Vec::new(),
                };
                let (lo, hi) = depths
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(lo, hi), d| (lo.min(*d), hi.max(*d)));
                let radars = surface
                    .batteries
                    .iter()
                    .filter(|b| b.launchers[0].range() == IdRange::AreaDefense)
                    .count();
                println!(
                    "surface-area: {theater} seed {seed} level {level} front {} units {} of {wanted_units} sa-batteries {radars} depth-nm {} problems {} digest {:#018x}",
                    u8::from(front.is_some()),
                    units.len(),
                    if depths.is_empty() {
                        "none".to_owned()
                    } else {
                        format!("{lo:.1}..{hi:.1}")
                    },
                    broken.len(),
                    surface.digest(),
                );
                for note in surface
                    .layout_notes
                    .iter()
                    .filter(|n| n.starts_with("area"))
                {
                    println!("surface-area-note: {theater} seed {seed} level {level}: {note}");
                }
                if let (Some(dir), 3) = (png, level) {
                    std::fs::create_dir_all(dir)?;
                    let path = dir.join(format!("area-{theater}-seed{seed}.png"));
                    area_map(&terrain, &surface, front, &path)?;
                    println!("surface-area-png: {}", path.display());
                }
            }
        }
    }
    println!("surface-area-sweep: builds {builds} problems {problems} short {short}");
    if problems > 0 {
        return Err(format!("{problems} area rule problems").into());
    }
    Ok(())
}

/// A pixel writer for [`area_map`].
type Put<'a> = dyn FnMut(&mut Vec<u8>, i64, i64, [u8; 3]) + 'a;

/// The theater from above, north up: water blue, land green by height,
/// the front axis white from Blue's centroid to Red's, the midpoint line
/// and the 10 and 60 nm band edges yellow, runways as squares (red Red's,
/// blue Blue's, grey neutral), area SAM launchers magenta, guns orange and
/// battery radars cyan.
fn area_map(
    terrain: &Terrain,
    surface: &Surface,
    front: Option<layout::Front>,
    path: &Path,
) -> AppResult<()> {
    let theater = &terrain.theater;
    let (cols, rows) = (theater.cols, theater.rows);
    let scale = (1024 / cols.max(rows)).max(1);
    let (w, h) = (cols * scale, rows * scale);
    let mut rgba = vec![0u8; w * h * 4];
    let cell = layout::ground::CELL_FT as f64;
    // World feet to pixel: x right, z up.
    let px = |x: f64, z: f64| -> (i64, i64) {
        (
            (x / cell * scale as f64) as i64,
            h as i64 - 1 - (z / cell * scale as f64) as i64,
        )
    };
    let mut put = |rgba: &mut Vec<u8>, x: i64, y: i64, c: [u8; 3]| {
        if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
            let at = (y as usize * w + x as usize) * 4;
            rgba[at..at + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    };
    for r in 0..rows {
        for c in 0..cols {
            let t = theater.cell(c, r);
            let colour = if t.class == 1 || t.color == 255 {
                [30, 60, 120]
            } else {
                let e = (t.elevation as u32 * 6).min(140) as u8;
                [40 + e, 90 + e / 2, 40 + e / 3]
            };
            for dy in 0..scale {
                for dx in 0..scale {
                    let (x, y) = (c * scale + dx, (rows - 1 - r) * scale + dy);
                    put(&mut rgba, x as i64, y as i64, colour);
                }
            }
        }
    }
    let line = |rgba: &mut Vec<u8>, a: [f64; 2], b: [f64; 2], c: [u8; 3], put: &mut Put<'_>| {
        let steps = 2000;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let (x, y) = px(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            put(rgba, x, y, c);
        }
    };
    if let Some(f) = front {
        let (b, r) = (
            [f.blue[0] as f64, f.blue[1] as f64],
            [f.red[0] as f64, f.red[1] as f64],
        );
        line(&mut rgba, b, r, [255, 255, 255], &mut put);
        let axis = [r[0] - b[0], r[1] - b[1]];
        let len = axis[0].hypot(axis[1]);
        let (u, v) = (
            [axis[0] / len, axis[1] / len],
            [-axis[1] / len, axis[0] / len],
        );
        let mid = [(b[0] + r[0]) / 2., (b[1] + r[1]) / 2.];
        let far = 3.0e6;
        for nm in [0., 10., 60.] {
            let d = nm * layout::NM_FT as f64;
            let o = [mid[0] + u[0] * d, mid[1] + u[1] * d];
            line(
                &mut rgba,
                [o[0] - v[0] * far, o[1] - v[1] * far],
                [o[0] + v[0] * far, o[1] + v[1] * far],
                if nm == 0. {
                    [255, 255, 255]
                } else {
                    [250, 220, 40]
                },
                &mut put,
            );
        }
    }
    let blob = |rgba: &mut Vec<u8>, x: f64, z: f64, radius: i64, c: [u8; 3]| {
        let (cx, cy) = px(x, z);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy <= radius * radius {
                    put(rgba, cx + dx, cy + dy, c);
                }
            }
        }
    };
    for runway in &terrain.airport_scene.runways {
        let colour = match surface.side_of(runway.object).0 {
            1 => [60, 120, 255],
            2 => [230, 40, 40],
            _ => [170, 170, 170],
        };
        let c = runway.surface.center;
        blob(&mut rgba, c[0], c[2], 4, [0, 0, 0]);
        blob(&mut rgba, c[0], c[2], 3, colour);
    }
    let radars: Vec<UnitId> = surface
        .batteries
        .iter()
        .filter(|b| b.launchers[0].range() == IdRange::AreaDefense)
        .map(|b| b.radar)
        .collect();
    for unit in surface
        .units
        .iter()
        .filter(|u| u.id.range() == IdRange::AreaDefense || radars.contains(&u.id))
    {
        let colour = if radars.contains(&unit.id) {
            [40, 230, 230]
        } else if unit.class & class::SAM != 0 {
            [240, 40, 240]
        } else {
            [255, 150, 20]
        };
        let (x, z) = (f64::from(unit.position[0]), f64::from(unit.position[2]));
        blob(&mut rgba, x, z, 4, [0, 0, 0]);
        blob(&mut rgba, x, z, 3, colour);
    }
    std::fs::write(
        path,
        crate::replay::png::encode_rgba(w as u32, h as u32, &rgba)?,
    )?;
    Ok(())
}

/// Exercise the actual world construction, not isolated origin corrections.
/// Every offered objective is checked at its authored and relocated position.
fn ground_pads_survey(resources: &BTreeMap<String, Vec<u8>>, only: &[String]) -> AppResult<()> {
    use tore_world::terrain::Placements;
    let mut total = 0;
    let mut problems = 0;
    let mut worlds = 0;
    for (index, theater) in tables::THEATERS.iter().enumerate() {
        if !only.is_empty() && !only.iter().any(|t| t.eq_ignore_ascii_case(theater)) {
            continue;
        }
        let mut cases = vec![(theater.to_string(), None, "base")];
        for name in resources.keys().filter(|n| {
            n.ends_with(".MM")
                && n.starts_with('~')
                && tore_formats::theater::base_theater(n) == Some(*theater)
        }) {
            cases.push((name.trim_end_matches(".MM").to_owned(), None, "layout"));
        }
        let mut stems = tables::TEMPLATES[index].to_vec();
        if *theater == "UKR" {
            stems.extend(["QFACT", "QUBUNK", "QURADAR"]);
        }
        for stem in stems {
            for (variation, label) in [(Variation::OFF, "authored"), (Variation::ON, "relocated")] {
                let mut target = heavy(stem, index, 1);
                target.variation = variation;
                cases.push((theater.to_string(), Some(target), label));
            }
        }
        let mut count = 0;
        for (layout, target, variant) in cases {
            let terrain = Terrain::for_mission_with(
                resources,
                &layout,
                Some(0),
                &Overrides {
                    city_fill: false,
                    ..overrides()?
                },
                target.as_ref(),
            )?;
            let sources = Placements::for_terrain(resources, &terrain, &layout)?;
            let stem = target.as_ref().map_or("base", |t| t.stem.as_str());
            worlds += 1;
            for placed in sources.placed() {
                let (_, p) = placed?;
                let Some(pad) = sources.foundation(p) else {
                    continue;
                };
                let shape = &sources.shapes[&p.object_type];
                let before = sources
                    .stance_on_terrain(p, |x, z| f64::from(terrain.raw_height(x as f32, z as f32)))
                    .unwrap();
                let after = terrain.placement_stance(&sources, p).unwrap();
                let bottom = tore_world::grounding::height_range(shape).unwrap()[0] * after.scale;
                let old_bottom = before.origin[1] + bottom;
                let new_bottom = after.origin[1] + bottom;
                let mut buried = 0f64;
                let mut error = 0f64;
                for ix in 0..=16 {
                    for iz in 0..=16 {
                        // Exclude the two-foot rounding guard around actual geometry.
                        let x =
                            pad.min[0] + 2. + (pad.max[0] - pad.min[0] - 4.) * f64::from(ix) / 16.;
                        let z =
                            pad.min[1] + 2. + (pad.max[1] - pad.min[1] - 4.) * f64::from(iz) / 16.;
                        buried = buried
                            .max(f64::from(terrain.raw_height(x as f32, z as f32)) - old_bottom);
                        error = error.max(
                            (f64::from(terrain.height(x as f32, z as f32)) - new_bottom).abs(),
                        );
                        error = error.max((terrain.surface_height(x, z) - new_bottom).abs());
                    }
                }
                if error > 0.05 {
                    problems += 1;
                }
                count += 1;
                println!(
                    "ground-pad: {theater} {layout} {stem} {variant} {} #{} at {} {} before-buried {buried:.2} after-error {error:.4}",
                    p.object_type, p.key.ordinal, p.position[0], p.position[2]
                );
            }
        }
        total += count;
        println!("ground-pads-theater: {theater} placements {count}");
    }
    println!("ground-pads: worlds {worlds} placements {total} problems {problems}");
    if problems > 0 {
        return Err(format!("{problems} foundation support failures").into());
    }
    Ok(())
}

#[cfg(test)]
mod grounding_tests {
    use super::mission_placements;

    #[test]
    fn campaign_loadouts_do_not_hide_object_placements_from_the_audit() {
        let bytes = b"textFormat\nmap UKR.T2\nobj\n type TEST.OT\n pos 10 7 20\n angle 90 0 0\n hardpoint 1 2\n hardpoint 2 3\n .\n";
        let layout = mission_placements("TEST.M", bytes).unwrap();
        assert_eq!(layout.placements.len(), 1);
        assert_eq!(layout.placements[0].position, [10, 7, 20]);
        assert_eq!(layout.placements[0].angles, [90, 0, 0]);
        assert_eq!(layout.map.as_deref(), Some("UKR.T2"));
    }

    #[test]
    fn the_players_selected_aircraft_is_not_a_ground_placement() {
        let bytes = b"textFormat\nobj\n type <selected>\n pos 1 10000 2\n .\nobj\n type TEST.OT\n pos 3 0 4\n .\n";
        let layout = mission_placements("TEST.M", bytes).unwrap();
        assert_eq!(layout.placements.len(), 1);
        assert_eq!(layout.placements[0].position, [3, 0, 4]);
    }

    #[test]
    fn audited_placement_fields_remain_strict() {
        assert!(
            mission_placements(
                "TEST.M",
                b"textFormat\nobj\n type TEST.OT\n pos 0 0 0\n pos 0 1 0\n .\n"
            )
            .is_err()
        );
        assert!(
            mission_placements("TEST.M", b"textFormat\nobj\n type TEST.OT\n pos 0 0 0\n").is_err()
        );
    }
}
