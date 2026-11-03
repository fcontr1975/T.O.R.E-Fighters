//! Reproducible bridge inventory using the imported pack and shared world stance.
use crate::{AppResult, camera::Camera, reel::Gpu, scenery::Scenery};
use std::{collections::BTreeSet, io::Write, path::Path};
use tore_formats::{
    quick_template::{ObjectKind, Template, tables},
    shape::{DAMAGED_WORD, Shape},
};
use tore_world::terrain::{Overrides, Placements, Terrain};

fn bridge(name: &str) -> bool {
    tore_world::grounding::bridges::is_bridge(name)
}

/// Sample the textured road faces, including ramps, on a barycentric grid.
/// Vertical trusses, solid shadow decals and untextured pier walls are excluded.
fn deck_points(shape: &Shape, scale: f64) -> Vec<[f64; 3]> {
    let mut points = Vec::new();
    for face in &shape.faces {
        if face.uv.is_empty() || face.positions.len() < 3 {
            continue;
        }
        let a = face.positions[0].map(f64::from);
        for edge in face.positions[1..].windows(2) {
            let b = edge[0].map(f64::from);
            let c = edge[1].map(f64::from);
            let u: [f64; 3] = std::array::from_fn(|i| b[i] - a[i]);
            let v: [f64; 3] = std::array::from_fn(|i| c[i] - a[i]);
            let n = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            if n[2].abs() <= n[0].hypot(n[1]) * 2. {
                continue;
            }
            let length = u
                .iter()
                .map(|x| x * x)
                .sum::<f64>()
                .sqrt()
                .max(v.iter().map(|x| x * x).sum::<f64>().sqrt())
                * scale;
            let steps = (length / 64.).ceil().clamp(1., 256.) as usize;
            for i in 0..=steps {
                for j in 0..=steps - i {
                    points.push(std::array::from_fn(|k| {
                        scale
                            * (a[k]
                                + u[k] * i as f64 / steps as f64
                                + v[k] * j as f64 / steps as f64)
                    }));
                }
            }
        }
    }
    points
}

fn world(p: [f64; 3], s: &tore_world::terrain::Stance) -> [f64; 3] {
    std::array::from_fn(|i| {
        s.origin[i] + s.basis.right[i] * p[0] + s.basis.forward[i] * p[1] + s.basis.up[i] * p[2]
    })
}

/// Conservative shape overlap candidates, using oriented solid boxes rather
/// than enclosing spheres. Touching bridge joins still require visual review.
fn overlaps(a: &tore_sim::airport::OrientedBox, b: &tore_sim::airport::OrientedBox) -> bool {
    use tore_sim::attitude::{Basis, dot};
    let basis = |b: &tore_sim::airport::OrientedBox| {
        let v = Basis::new(b.heading, b.pitch, b.bank);
        [v.right, v.up, v.forward]
    };
    let (aa, bb) = (basis(a), basis(b));
    let mut axes: Vec<_> = aa.into_iter().chain(bb).collect();
    for u in aa {
        for v in bb {
            axes.push([
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ]);
        }
    }
    let delta = std::array::from_fn(|i| b.center[i] - a.center[i]);
    axes.into_iter().all(|axis| {
        let radius = |half: [f64; 3], basis: [[f64; 3]; 3]| {
            (0..3)
                .map(|i| half[i] * dot(axis, basis[i]).abs())
                .sum::<f64>()
        };
        dot(delta, axis).abs() <= radius(a.half, aa) + radius(b.half, bb) + 1e-6
    })
}

pub fn run() -> AppResult<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let out = args
        .first()
        .ok_or("--bridge-audit OUT_DIR [LAYOUT ...] [--capture] [--strict]")?;
    let capture = args.iter().any(|a| a == "--capture");
    let strict = args.iter().any(|a| a == "--strict");
    let only: Vec<_> = args[1..].iter().filter(|a| !a.starts_with("--")).collect();
    let out = Path::new(out);
    std::fs::create_dir_all(out)?;
    let resources = crate::reel::load_assets()?.theater_resources;
    let mut assets = std::fs::File::create(out.join("assets.tsv"))?;
    writeln!(
        assets,
        "type\tshape\thp\tclass\texplosion\tcrater\tintact_faces\tdestroyed_faces\tbranch\ttextures\tmissing"
    )?;
    let mut missing = 0;
    for name in &tore_world::terrain::MAP_TIED_TYPES[..12] {
        let def = tore_formats::static_object::Definition::parse(
            resources
                .get(*name)
                .ok_or_else(|| format!("missing imported {name}"))?,
        )?;
        let sh = def.main_shape.as_ref().ok_or("bridge has no shape")?;
        let bytes = resources.get(sh).ok_or("bridge shape not imported")?;
        let intact = Shape::with_export_state(bytes, &Default::default())?;
        let damaged = Shape::with_export_state(bytes, &[(DAMAGED_WORD, 1)].into())?;
        let textures: BTreeSet<_> = intact
            .faces
            .iter()
            .chain(&damaged.faces)
            .filter(|f| !f.uv.is_empty())
            .map(|f| f.texture.as_str())
            .filter(|n| !n.is_empty())
            .collect();
        let absent: Vec<_> = textures
            .iter()
            .filter(|n| !resources.contains_key(**n))
            .copied()
            .collect();
        missing += absent.len();
        writeln!(
            assets,
            "{name}\t{sh}\t{}\t{:#x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            def.hit_points.unwrap_or(0),
            def.category,
            def.explosion,
            def.crater,
            intact.faces.len(),
            damaged.faces.len(),
            tore_formats::shape::has_damage_branch(bytes),
            textures.into_iter().collect::<Vec<_>>().join(","),
            absent.join(",")
        )?;
    }
    let mut table = std::fs::File::create(out.join("placements.tsv"))?;
    writeln!(
        table,
        "layout\tid\ttype\tx\ty\tz\theading\tpitch\tbank\tdeck_samples\tdeck_min_clearance_ft\tdeck_max_clearance_ft\tend_a_ft\tend_b_ft\tbottom_min_ft\tbottom_max_ft\twater_min_ft\toverlaps\tstatus"
    )?;
    let mut banks_table = std::fs::File::create(out.join("banks.tsv"))?;
    writeln!(
        banks_table,
        "layout\tbank_x\tbank_z\tsupport_ft\tmax_cut_ft\tmax_approach_grade\tramp_toe_x\tramp_toe_z"
    )?;
    let mut total = 0;
    let mut bad = 0;
    let mut layouts = 0;
    let mut pictured_types = BTreeSet::new();
    let mut cases: Vec<_> = resources
        .keys()
        .filter(|n| n.ends_with(".MM") && tore_formats::theater::base_theater(n).is_some())
        .map(|n| (n.trim_end_matches(".MM").to_owned(), None))
        .collect();
    for (index, code) in tables::THEATERS.iter().enumerate() {
        for stem in tables::TEMPLATES[index] {
            let name = format!("~{stem}.M");
            let template = Template::parse(
                &name,
                resources.get(&name).ok_or("missing imported template")?,
            )?;
            if template
                .objects
                .iter()
                .any(|o| matches!(&o.kind, ObjectKind::Named(n) if bridge(n)))
            {
                cases.push((
                    (*code).to_owned(),
                    Some(tore_world::surface::resolve::GroundTarget {
                        stem: (*stem).to_owned(),
                        aaa: 3,
                        sam: 3,
                        seed: 1,
                        enemy_nationality: tables::ENEMY_NATIONALITY[index],
                        night_stealth: false,
                        variation: tore_world::surface::layout::Variation::ON,
                        separation_nm: 20,
                    }),
                ));
            }
        }
    }
    for (code, target) in cases {
        if !only.is_empty() && !only.iter().any(|a| a.eq_ignore_ascii_case(&code)) {
            continue;
        }
        let label = target
            .as_ref()
            .map_or_else(|| code.clone(), |t| format!("{code}-{}", t.stem));
        let terrain = Terrain::for_mission_with(
            &resources,
            &code,
            Some(0),
            &Overrides {
                city_fill: false,
                time: Some([12, 0]),
                ..Default::default()
            },
            target.as_ref(),
        )?;
        let sources = Placements::for_terrain(&resources, &terrain, &code)?;
        let (_, banks) = tore_world::grounding::bridges::graded_foundations(
            &sources,
            |x, z| terrain.raw_height(x, z),
            |bounds| {
                !terrain
                    .airport_scene
                    .runways
                    .iter()
                    .any(|r| bounds.meets_airfield(&r.surface))
            },
        );
        for bank in &banks {
            let [start, end] = bank.approach();
            let distance = (end[0] - start[0]).hypot(end[1] - start[1]);
            let steps = (distance / 16.).ceil().max(1.) as usize;
            let mut grade: f64 = 0.;
            let mut last = f64::from(terrain.height(start[0] as f32, start[1] as f32));
            for i in 1..=steps {
                let at = std::array::from_fn::<_, 2, _>(|a| {
                    start[a] + (end[a] - start[a]) * i as f64 / steps as f64
                });
                let h = f64::from(terrain.height(at[0] as f32, at[1] as f32));
                // Measure the constructed road, stopping where raw terrain
                // takes over. Unmodified terrain has no new grade guarantee.
                if last > f64::from(terrain.raw_height(at[0] as f32, at[1] as f32)) + 1. {
                    grade = grade.max((h - last).abs() / (distance / steps as f64));
                }
                last = h;
            }
            let mut cut: f64 = 0.;
            for x in 0..=32 {
                for z in 0..=32 {
                    let at = [x, z].map(|v| v as f64 / 32.);
                    let x =
                        (bank.core.min[0] + (bank.core.max[0] - bank.core.min[0]) * at[0]) as f32;
                    let z =
                        (bank.core.min[1] + (bank.core.max[1] - bank.core.min[1]) * at[1]) as f32;
                    cut = cut.max(f64::from(terrain.raw_height(x, z) - terrain.height(x, z)));
                }
            }
            writeln!(
                banks_table,
                "{label}\t{:.2}\t{:.2}\t{:.2}\t{cut:.3}\t{grade:.5}\t{:.2}\t{:.2}",
                (bank.core.min[0] + bank.core.max[0]) * 0.5,
                (bank.core.min[1] + bank.core.max[1]) * 0.5,
                bank.core.height,
                start[0],
                start[1]
            )?;
            if cut > 1. || grade > 0.081 {
                bad += 1;
            }
        }
        let has_bridges = sources
            .placed()
            .flatten()
            .any(|(_, p)| bridge(&p.object_type));
        let mut scenery = if capture && has_bridges {
            Some(Scenery::build_for(&resources, &terrain, false)?)
        } else {
            None
        };
        let mut gpu = match &scenery {
            Some(s) => Some(pollster::block_on(Gpu::new(s))?),
            None => None,
        };
        let mut combat = tore_sim::combat::live::State::open_mission();
        if capture {
            for o in &terrain.airport_scene.objects {
                combat.add_ground_target(
                    o.id,
                    o.bounds,
                    o.hit_points,
                    o.category,
                    tore_sim::combat::live::NO_SIDE,
                )?;
            }
        }
        let surface = tore_world::surface::SurfaceState::default();
        layouts += 1;
        for placed in sources.placed() {
            let (id, p) = placed?;
            if !bridge(&p.object_type) {
                continue;
            }
            total += 1;
            let s = terrain
                .placement_stance(&sources, p)
                .ok_or("bridge has no stance")?;
            let shape = &sources.shapes[&p.object_type];
            let points = deck_points(shape, s.scale);
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;
            let mut water = f64::INFINITY;
            let mut ends = [(f64::INFINITY, 0.), (f64::NEG_INFINITY, 0.)];
            for &point in &points {
                let at = world(point, &s);
                let clearance = at[1] - f64::from(terrain.height(at[0] as f32, at[2] as f32));
                min = min.min(clearance);
                max = max.max(clearance);
                if point[1] < ends[0].0 {
                    ends[0] = (point[1], clearance);
                }
                if point[1] > ends[1].0 {
                    ends[1] = (point[1], clearance);
                }
                // Sea plane is zero. Painted inland rivers share terrain height.
                water = water.min(at[1]);
            }
            let feet = tore_world::grounding::bridges::footing_points(&sources, p);
            let mut bottom = [f64::INFINITY, f64::NEG_INFINITY];
            for mut at in feet.iter().copied() {
                at[1] += s.origin[1];
                let gap = at[1] - f64::from(terrain.height(at[0] as f32, at[2] as f32));
                bottom[0] = bottom[0].min(gap);
                bottom[1] = bottom[1].max(gap);
            }
            let bounds = tore_sim::airport::OrientedBox {
                center: s.center,
                half: s.half,
                heading: s.heading,
                pitch: s.pitch,
                bank: s.bank,
            };
            let overlaps: Vec<_> = terrain
                .airport_scene
                .objects
                .iter()
                .filter(|o| o.id != id && overlaps(&bounds, &o.bounds))
                .map(|o| format!("{}:{:#x}", o.object_type, o.id))
                .collect();
            let status = if points.is_empty() {
                "unknown-deck"
            } else if feet.is_empty() {
                "unknown-support"
            } else if min < -1. || water < -1. {
                "buried"
            } else if p.object_type.ends_with("END.OT") && ends[0].1.min(ends[1].1) > 30. {
                "approach-gap"
            } else if bottom[1] > 30. {
                "support-gap"
            } else {
                "clear"
            };
            if status != "clear" {
                bad += 1;
            }
            writeln!(
                table,
                "{label}\t{id:#x}\t{}\t{:.2}\t{:.2}\t{:.2}\t{}\t{}\t{}\t{}\t{min:.2}\t{max:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{water:.2}\t{}\t{status}",
                p.object_type,
                s.origin[0],
                s.origin[1],
                s.origin[2],
                p.angles[0],
                p.angles[1],
                p.angles[2],
                points.len(),
                ends[0].1,
                ends[1].1,
                bottom[0],
                bottom[1],
                overlaps.join(",")
            )?;
            if let (Some(scenery), Some(gpu)) = (&mut scenery, &mut gpu) {
                if let Some(wanted) = args.iter().find_map(|a| a.strip_prefix("--id="))
                    && wanted.split(',').all(|v| {
                        u32::from_str_radix(v.trim_start_matches("0x"), 16).ok() != Some(id)
                    })
                {
                    continue;
                }
                let mut camera = Camera::new();
                camera.yaw = (s.heading + 45f64.to_radians()) as f32;
                camera.pitch = -25f32.to_radians();
                let view = args.iter().find_map(|a| a.strip_prefix("--view="));
                match view {
                    Some("side") => {
                        camera.yaw = (s.heading + std::f64::consts::FRAC_PI_2) as f32;
                        camera.pitch = -8f32.to_radians();
                    }
                    Some("along") => {
                        camera.yaw = s.heading as f32;
                        camera.pitch = -8f32.to_radians();
                    }
                    Some("near") => {
                        camera.yaw = s.heading as f32;
                        camera.pitch = -4f32.to_radians();
                    }
                    Some(_) => return Err("bridge view must be side, along or near".into()),
                    None => {}
                }
                let distance = s.half[0].hypot(s.half[2]).max(200.)
                    * if view == Some("near") { 2.1 } else { 2.6 };
                camera.position = [
                    s.center[0] - f64::from(camera.yaw.sin() * camera.pitch.cos()) * distance,
                    s.center[1] - f64::from(camera.pitch.sin()) * distance,
                    s.center[2] - f64::from(camera.yaw.cos() * camera.pitch.cos()) * distance,
                ];
                camera.near_clip = 1.;
                scenery.resolve_palette(&terrain, camera.position[1]);
                scenery.set_origin(camera.position);
                let hp = combat
                    .targets
                    .iter()
                    .find(|t| t.id == id)
                    .ok_or("bridge missing combat target")?
                    .initial_hp;
                if hp <= 0 {
                    return Err("bridge is not destructible".into());
                }
                let mut states = vec![("intact", hp), ("destroyed", 0)];
                if pictured_types.insert(p.object_type.clone()) {
                    states.insert(1, ("damaged", (hp / 2).max(1)));
                }
                let mut intact_geometry = None;
                for (state, health) in states {
                    combat.targets.iter_mut().find(|t| t.id == id).unwrap().hp = health;
                    if scenery.drawn_vertex_floats(id, &combat.targets, &surface) == 0 {
                        return Err(
                            format!("{label} {id:#x} {state}: bridge geometry vanished").into()
                        );
                    }
                    let geometry =
                        std::sync::Arc::clone(scenery.static_geometry(&combat.targets, &surface));
                    if state == "intact" {
                        intact_geometry = Some(std::sync::Arc::clone(&geometry));
                    } else if let Some(intact) = &intact_geometry {
                        let same =
                            geometry.vertices == intact.vertices && geometry.lines == intact.lines;
                        if same != (health > 0) {
                            return Err(format!(
                                "{label} {id:#x} {state}: wrong damage appearance"
                            )
                            .into());
                        }
                    }
                    let hidden = if health > 0 {
                        BTreeSet::new()
                    } else {
                        BTreeSet::from([id])
                    };
                    let replay = scenery.static_geometry_replay(&hidden, &surface);
                    if geometry.vertices != replay.vertices || geometry.lines != replay.lines {
                        return Err(format!(
                            "{label} {id:#x} {state}: live/replay geometry differs"
                        )
                        .into());
                    }
                    gpu.sim.airports(&gpu.device, &gpu.queue, &geometry);
                    let pixels = gpu.pixels(&camera, &terrain, scenery)?;
                    std::fs::write(
                        out.join(format!("{label}-{id:x}-{state}.png")),
                        crate::replay::png::encode_rgba(
                            crate::reel::WIDTH,
                            crate::reel::HEIGHT,
                            &pixels,
                        )?,
                    )?;
                }
                combat.targets.iter_mut().find(|t| t.id == id).unwrap().hp = hp;
            }
        }
        println!("bridge-audit: {label} complete");
    }
    println!(
        "bridge-audit: {layouts} layouts {total} placements {bad} flagged {missing} missing textures"
    );
    if layouts == 0 || total == 0 || missing > 0 || (strict && bad > 0) {
        return Err("bridge audit failed; see TSV tables".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_overlap_candidates_respect_heading_width_and_height() {
        let a = tore_sim::airport::OrientedBox {
            center: [0.; 3],
            half: [5., 10., 100.],
            heading: std::f64::consts::FRAC_PI_4,
            pitch: 0.,
            bank: 0.,
        };
        let mut b = a;
        // Enclosing circles overlap, but the parallel narrow decks do not.
        b.center = [40., 0., -40.];
        assert!(!overlaps(&a, &b));
        // Adjoining pieces overlap along the length of the crossing.
        b.center = [40., 0., 40.];
        assert!(overlaps(&a, &b));
        b.center[1] = 30.;
        assert!(!overlaps(&a, &b));
    }
}
