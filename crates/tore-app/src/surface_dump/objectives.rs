//! Exhaustive, CPU-only placement inspection. Findings include authored
//! exceptions instead of silently excusing objects that did not move.
use super::*;
use std::collections::BTreeSet;
use tore_world::surface::layout::{Ground, Placed, TypeInfo};
use tore_world::terrain::SurfaceSite;

#[derive(Clone)]
struct Finding {
    object: String,
    at: [i32; 3],
    rule: String,
}

fn finding(object: impl Into<String>, at: [i32; 3], rule: impl Into<String>) -> Finding {
    Finding {
        object: object.into(),
        at,
        rule: rule.into(),
    }
}

/// Separating-axis test on actual rotated boxes, not bounding circles.
fn overlaps(a: Placed, b: Placed) -> bool {
    let reach = |p: Placed| {
        (0..2)
            .map(|i| p.footprint.min[i].abs().max(p.footprint.max[i].abs()))
            .sum::<i64>()
    };
    let limit = reach(a) + reach(b);
    if (0..2).any(|i| (a.at[i] - b.at[i]).abs() > limit) {
        return false;
    }
    let corners = |p: Placed| {
        let (s, c) = (f64::from(p.heading).to_radians()).sin_cos();
        [
            p.footprint.min,
            [p.footprint.min[0], p.footprint.max[1]],
            p.footprint.max,
            [p.footprint.max[0], p.footprint.min[1]],
        ]
        .map(|v| {
            [
                p.at[0] as f64 + c * v[0] as f64 + s * v[1] as f64,
                p.at[1] as f64 - s * v[0] as f64 + c * v[1] as f64,
            ]
        })
    };
    if a.footprint.min == a.footprint.max || b.footprint.min == b.footprint.max {
        return false;
    }
    let (ac, bc) = (corners(a), corners(b));
    for heading in [a.heading, b.heading] {
        let (s, c) = f64::from(heading).to_radians().sin_cos();
        for axis in [[c, -s], [s, c]] {
            let interval = |points: [[f64; 2]; 4]| {
                points
                    .into_iter()
                    .map(|p| p[0] * axis[0] + p[1] * axis[1])
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                        (lo.min(v), hi.max(v))
                    })
            };
            let (a, b) = (interval(ac), interval(bc));
            // Touching edges and sub-foot rounding are not overlap.
            if a.1 <= b.0 + 1. || b.1 <= a.0 + 1. {
                return false;
            }
        }
    }
    true
}

fn inspect(
    surface: &Surface,
    ground: &Ground<'_>,
    infos: &BTreeMap<String, TypeInfo>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut ids = BTreeSet::new();
    let mut placed = Vec::new();
    for unit in &surface.units {
        let label = format!("{:#010x}:{}", unit.id.0, unit.resource);
        let mut add = |rule| out.push(finding(&label, unit.position, rule));
        if !ids.insert(unit.id) {
            add("duplicate-id".into());
        }
        let info = infos.get(&unit.resource).copied().unwrap_or_default();
        let at = [i64::from(unit.position[0]), i64::from(unit.position[2])];
        if layout::footing::vehicle(unit) {
            for rule in layout::footing::violations(ground, unit, info) {
                add(format!("vehicle-{rule}"));
            }
        }

        let ship = unit.class & class::SHIP != 0;
        if !ground.inside(at, 0) {
            add("outside-map".into());
        }
        if !info.bridge_or_road && !info.strip {
            let r = info.footprint.radius();
            let corners = [
                [at[0] - r, at[1] - r],
                [at[0] + r, at[1] - r],
                [at[0] - r, at[1] + r],
                [at[0] + r, at[1] + r],
            ];
            if corners.iter().any(|p| !ground.inside(*p, 0)) {
                add("footprint-outside-map".into());
            }
            if corners.iter().any(|p| ground.water(*p) != ship) {
                add("footprint-terrain-class".into());
            }
        }
        if !info.bridge_or_road && !info.strip {
            if ground.water(at) != ship {
                add(if ship {
                    "ship-on-land"
                } else {
                    "land-on-water"
                }
                .into());
            }
            if !ship && !ground.level(at, layout::LEVEL_UNITS) {
                add("slope-over-256ft-per-cell".into());
            }
            if ship && unit.position[1] != 0 {
                add("ship-authored-height-not-sea-level".into());
            }
        }
        let box_ = Placed {
            id: unit.id.0,
            at,
            heading: unit.angles[0],
            footprint: info.footprint,
        };
        // Pavement, roads and bridges are support geometry, not solid obstacles.
        if !info.strip && !info.bridge_or_road && unit.resource != layout::BARRAGE_ZONE {
            for other in ground.objects.iter().chain(&ground.runways) {
                if other.id == unit.id.0 {
                    continue;
                }
                if overlaps(box_, *other) {
                    add(format!("layout-footprint-overlap:{:#010x}", other.id));
                }
            }
            for (other, name) in &placed {
                if overlaps(box_, *other) {
                    add(format!("unit-footprint-overlap:{name}"));
                }
            }
            placed.push((box_, label.clone()));
        }
        if let Some(route) = &unit.route {
            if route.legs().next().is_none() {
                add("route-without-legs".into());
            }
            let mut from = at;
            for leg in route.legs() {
                let to = [i64::from(leg.position[0]), i64::from(leg.position[2])];
                let distance = ((to[0] - from[0]) as f64).hypot((to[1] - from[1]) as f64);
                let steps = (distance / 250.).ceil().max(1.) as i64;
                let mut rules = BTreeSet::new();
                for i in 0..=steps {
                    let p = [
                        from[0] + (to[0] - from[0]) * i / steps,
                        from[1] + (to[1] - from[1]) * i / steps,
                    ];
                    if !ground.inside(p, 0) {
                        rules.insert("outside-map");
                    }
                    if ground.water(p) != ship {
                        rules.insert("terrain-class");
                    }
                    if !ship && !ground.level(p, layout::LEVEL_UNITS) {
                        rules.insert("slope");
                    }
                }
                for rule in rules {
                    add(format!("route-leg-{}:{rule}", leg.index));
                }
                from = to;
            }
        }
    }
    for parked in &surface.parked {
        let label = format!("{:#010x}:{}", parked.id.0, parked.resource);
        if !ids.insert(parked.id) {
            out.push(finding(&label, parked.position, "duplicate-id"));
        }
        let at = [i64::from(parked.position[0]), i64::from(parked.position[2])];
        if !ground.inside(at, 0) {
            out.push(finding(&label, parked.position, "outside-map"));
        }
        if parked.deck.is_none() && ground.water(at) {
            out.push(finding(&label, parked.position, "parked-on-water"));
        }
        if parked.deck.is_some_and(|id| surface.unit(id).is_none()) {
            out.push(finding(&label, parked.position, "missing-carrier"));
        }
    }
    if let Some(site) = &surface.template {
        let present = surface.template_units().count() + surface.parked.len();
        if present + site.removed.len() + site.left_out.len() != site.objects {
            out.push(finding(
                "template",
                [0; 3],
                format!(
                    "object-count:expected={},present={present},removed={},unresolved={}",
                    site.objects,
                    site.removed.len(),
                    site.left_out.len()
                ),
            ));
        }
        for left in &site.left_out {
            out.push(finding(
                format!("{}:{}", left.ordinal, left.resource),
                [0; 3],
                format!("missing-element:{}", left.why),
            ));
        }
    }
    for truck in &surface.trucks {
        if let Some(served) = truck.serves {
            match (surface.unit(truck.id), surface.unit(served)) {
                (Some(a), Some(b)) => {
                    let distance = f64::from(a.position[0] - b.position[0])
                        .hypot(f64::from(a.position[2] - b.position[2]));
                    if distance > tore_world::surface::RESUPPLY_RADIUS_FT {
                        out.push(finding(
                            format!("{:#010x}", a.id.0),
                            a.position,
                            format!("supply-out-of-reach:{distance:.2}ft"),
                        ));
                    }
                }
                _ => out.push(finding(
                    format!("{:#010x}", truck.id.0),
                    [0; 3],
                    "supply-missing-reference",
                )),
            }
        }
    }
    if let Some(why) = &surface.unresolved {
        out.push(finding("template", [0; 3], format!("unresolved:{why}")));
    }
    // Count every normal-flight placement refusal, including retained front
    // groups and authored units, separately from individual support violations.
    if let Some(reason) =
        tore_world::mission_layout::availability::surface_problem(surface, 0, false, false)
    {
        out.push(finding(
            "mission",
            [0; 3],
            format!("placement-refused:{reason}"),
        ));
    }
    for note in &surface.layout_notes {
        if note.starts_with("support fallback ") || note.starts_with("placement unresolved ") {
            let unit = note
                .split_whitespace()
                .nth(2)
                .and_then(|id| u32::from_str_radix(id.trim_start_matches("0x"), 16).ok())
                .and_then(|id| surface.unit(UnitId(id)));
            out.push(finding(
                unit.map_or_else(
                    || "layout".into(),
                    |u| format!("{:#010x}:{}", u.id.0, u.resource),
                ),
                unit.map_or([0; 3], |u| u.position),
                format!("unresolved-support-placement:{note}"),
            ));
        }
        if note.starts_with("no ")
            || note.contains("no place")
            || note.contains("no <")
            || note.contains("no front")
        {
            out.push(finding("layout", [0; 3], format!("missing-element:{note}")));
        }
    }
    out
}

/// Measure the drawn support using the shared grounding path, after pads and
/// airport replacement. The bounds check above uses the final scene as well.
fn final_geometry(
    resources: &BTreeMap<String, Vec<u8>>,
    terrain: &Terrain,
    theater: &str,
) -> AppResult<Vec<Finding>> {
    use tore_world::{
        grounding::{self, Footing},
        terrain::Placements,
    };
    let sources = Placements::for_terrain(resources, terrain, theater)?;
    let mut out = Vec::new();
    for unit in terrain
        .surface
        .units
        .iter()
        .filter(|u| u.id.range() == tore_world::surface::IdRange::Layout)
    {
        if sources
            .placed()
            .filter_map(Result::ok)
            .find(|(id, _)| *id == unit.id.0)
            .is_none_or(|(_, p)| p.position != unit.position || p.angles != unit.angles)
        {
            out.push(finding(
                format!("{:#010x}:{}", unit.id.0, unit.resource),
                unit.position,
                "scene-placement-stale",
            ));
        }
    }
    for field in &terrain.redrawn {
        let boxes: Vec<_> = terrain
            .placement_runways
            .iter()
            .filter(|p| p.id == field.strip_id)
            .collect();
        for (i, runway) in field.runways.iter().enumerate() {
            let (sin, cos) = runway.heading.to_radians().sin_cos();
            let drawn = field.frame.world([
                runway.from[0] + sin * runway.length / 2.,
                runway.from[1] + cos * runway.length / 2.,
            ]);
            if boxes
                .get(i)
                .is_none_or(|p| (p.at[0] as f64 - drawn[0]).hypot(p.at[1] as f64 - drawn[2]) > 2.)
            {
                out.push(finding(
                    format!("runway-{}-{i}", field.strip_id),
                    drawn.map(|v| v.round() as i32),
                    format!(
                        "runway-clearance-datum-error:integer={:?},drawn={drawn:?}",
                        boxes.get(i).map(|p| p.at)
                    ),
                ));
            }
        }
    }
    let height = |x: f64, z: f64| f64::from(terrain.height(x as f32, z as f32));
    for (id, p) in &terrain.surface.placements {
        if p.object_type.ends_with(".PT") {
            continue;
        }
        let label = format!("{:#010x}:{}", id.0, p.object_type);
        let (Some(definition), Some(shape), Some(stand)) = (
            sources.definitions.get(&p.object_type),
            sources.shapes.get(&p.object_type),
            sources.stance_on_terrain(p, height),
        ) else {
            out.push(finding(label, p.position, "unavailable-grounding-geometry"));
            continue;
        };
        match grounding::footing(definition) {
            Footing::Bottom => {
                let bottom = grounding::points(shape)
                    .map(|point| {
                        let world: [f64; 3] = std::array::from_fn(|i| {
                            stand.origin[i]
                                + stand.scale
                                    * (stand.basis.right[i] * f64::from(point[0])
                                        + stand.basis.forward[i] * f64::from(point[1])
                                        + stand.basis.up[i] * f64::from(point[2]))
                        });
                        world[1] - height(world[0], world[2])
                    })
                    .reduce(f64::min);
                if let Some(bottom) = bottom.filter(|v| !v.is_finite() || v.abs() > 0.05) {
                    out.push(finding(
                        &label,
                        p.position,
                        format!("bottom-clearance-ft:{bottom:.3}"),
                    ));
                }
                if terrain.fill_contact(stand.center, stand.center) {
                    out.push(finding(&label, p.position, "city-building-at-unit-center"));
                }
            }
            Footing::Waterline if stand.origin[1].abs() > 0.05 => {
                out.push(finding(
                    &label,
                    p.position,
                    format!("ship-waterline-above-sea-ft:{:.3}", stand.origin[1]),
                ));
            }
            _ => {}
        }
    }
    Ok(out)
}

struct SceneCase {
    input: Surface,
    terrain: Terrain,
    findings: Vec<Finding>,
}

// Reuse requires equal raw geometry and a fresh final-front check at the current
// separation. Starts are recomputed from the repaired target, not copied from raw
// placement. Compare full normalized inputs, never just hashes.
fn geometry_input(mut surface: Surface) -> Surface {
    surface.starts = None;
    if let Some(site) = &mut surface.template {
        site.settings.separation_nm = 0;
    }
    surface
}

// Coordinates depend on the final target, map and original front. Canonical
// runway ranks depend on that same fixed target and final airport geometry, not
// separation, so keep those lists from the independently built cached scene.
fn cached_starts(
    surface: &Surface,
    site: &SurfaceSite,
    resources: &BTreeMap<String, Vec<u8>>,
    terrain: &Terrain,
) -> Option<tore_world::surface::StartPoints> {
    let mut starts = layout::scene_starts(
        surface,
        &site.inspection_ground(resources, &terrain.theater),
        &terrain.airport_scene,
    )?;
    if let Some(finalized) = &terrain.surface.starts {
        starts.blue_airfields.clone_from(&finalized.blue_airfields);
        starts.red_airfields.clone_from(&finalized.red_airfields);
    }
    Some(starts)
}

pub(super) fn run(
    resources: &BTreeMap<String, Vec<u8>>,
    seeds: u32,
    wanted: &[String],
    fast: bool,
    uncached: bool,
) -> AppResult<()> {
    if seeds == 0 {
        return Err("--seeds must be positive".into());
    }
    let mut cases = 0usize;
    let mut findings = 0usize;
    let mut errors = 0usize;
    let mut scene_builds = 0usize;
    let mut cache_hits = 0usize;
    println!(
        "objective-audit-schema: 3; final scene; coordinates feet; class grid 8192ft; route samples 250ft; separation tolerance 0.02nm; objective tolerance 5.02nm"
    );
    println!("finding\ttheater\tseparation\ttemplate\taaa\tsam\tseed\tobject\tx\ty\tz\trule");
    for (index, theater, mut stems) in jobs(wanted)? {
        if fast {
            let field = stems
                .iter()
                .copied()
                .find(|s| s.contains("AIR") || s.contains("STRIP"));
            stems.truncate(1);
            if let Some(field) = field.filter(|field| !stems.contains(field)) {
                stems.push(field);
            }
        }
        stems.push("NONE");
        let base = Terrain::for_mission(resources, theater, Some(0), &overrides()?)?;
        let site = SurfaceSite::load(resources, theater)?;
        let mut infos = BTreeMap::new();
        for stem in stems {
            for seed in 1..=seeds {
                for aaa in 0..=3 {
                    for sam in 0..=3 {
                        if fast && aaa != sam {
                            continue;
                        }
                        let mut scenes: Vec<SceneCase> = Vec::new();
                        for separation in tore_world::mission_layout::SEPARATION_NM {
                            let target = (stem != "NONE").then(|| GroundTarget {
                                aaa,
                                sam,
                                separation_nm: separation as u32,
                                ..heavy(stem, index, seed)
                            });
                            let area = (stem == "NONE" && (aaa != 0 || sam != 0)).then_some(
                                resolve::AreaDefense {
                                    aaa,
                                    sam,
                                    seed,
                                    enemy_nationality: tables::ENEMY_NATIONALITY[index],
                                    night_stealth: false,
                                    rolled: true,
                                },
                            );
                            let raw = site.place_with(
                                resources,
                                &base.theater,
                                target.as_ref(),
                                area.as_ref(),
                            )?;
                            let input = geometry_input(raw.clone());
                            let found = (!uncached)
                                .then(|| {
                                    scenes.iter().position(|s| {
                                        s.input == input
                                            && site.inspection_front_matches(
                                                resources, &s.terrain, &raw,
                                            )
                                    })
                                })
                                .flatten();
                            let at = match found {
                                Some(at) => {
                                    cache_hits += 1;
                                    at
                                }
                                None => {
                                    if uncached {
                                        scenes.clear();
                                    }
                                    let terrain = match Terrain::for_mission_surface(
                                        resources,
                                        theater,
                                        Some(0),
                                        &overrides()?,
                                        target.as_ref(),
                                        area.as_ref(),
                                    ) {
                                        Ok(terrain) => terrain,
                                        Err(error) => {
                                            cases += 1;
                                            findings += 1;
                                            errors += 1;
                                            println!(
                                                "finding\t{theater}\t{separation}\t{stem}\t{aaa}\t{sam}\t{seed}\tmission\t0\t0\t0\tbuild-failed:{error}"
                                            );
                                            continue;
                                        }
                                    };
                                    scene_builds += 1;
                                    for u in &terrain.surface.units {
                                        infos.entry(u.resource.clone()).or_insert_with(|| {
                                            site.inspection_type(resources, &u.resource)
                                        });
                                    }
                                    let ground = site.inspection_scene_ground(&terrain);
                                    let mut checked = inspect(&terrain.surface, &ground, &infos);
                                    checked.extend(final_geometry(resources, &terrain, theater)?);
                                    scenes.push(SceneCase {
                                        input,
                                        terrain,
                                        findings: checked,
                                    });
                                    scenes.len() - 1
                                }
                            };
                            let cached = &scenes[at];
                            let terrain = &cached.terrain;
                            let mut surface = terrain.surface.clone();
                            if let Some(site) = &mut surface.template {
                                site.settings.separation_nm = separation as u32;
                            }
                            // A direct build is the oracle: never rewrite its output.
                            if found.is_some() {
                                surface.starts = cached_starts(&surface, &site, resources, terrain);
                            }
                            let ground = site.inspection_scene_ground(terrain);
                            let mut broken = cached.findings.clone();
                            let mut objective = surface.starts.as_ref().map(|s| s.target);
                            if let Some(starts) = &surface.starts {
                                let actual = layout::centroid(
                                    surface
                                        .template_units()
                                        .filter(|u| u.is_target())
                                        .map(|u| {
                                            [i64::from(u.position[0]), i64::from(u.position[2])]
                                        })
                                        .chain(surface.parked.iter().filter(|p| p.target).map(
                                            |p| {
                                                [i64::from(p.position[0]), i64::from(p.position[2])]
                                            },
                                        )),
                                );
                                if let Some(actual) = actual {
                                    objective = Some(actual.map(|v| v as i32));
                                    let gap = (actual[0] - i64::from(starts.target[0])) as f64;
                                    let gap =
                                        gap.hypot((actual[1] - i64::from(starts.target[1])) as f64);
                                    if gap > 2. {
                                        errors += 1;
                                        broken.push(finding("objective", [starts.target[0], 0, starts.target[1]],
                                            format!("objective-anchor-stale:actual={},{};gap-ft={gap:.2}", actual[0], actual[1])));
                                    }
                                }
                            }
                            if separation == 1. {
                                let again = site.place_with(
                                    resources,
                                    &base.theater,
                                    target.as_ref(),
                                    area.as_ref(),
                                )?;
                                if again != raw {
                                    errors += 1;
                                    broken.push(finding(
                                        "mission",
                                        [0; 3],
                                        "nondeterministic-layout",
                                    ));
                                }
                                if aaa == 0 && sam == 0 {
                                    let again = Terrain::for_mission_surface(
                                        resources,
                                        theater,
                                        Some(0),
                                        &overrides()?,
                                        target.as_ref(),
                                        area.as_ref(),
                                    )?;
                                    if again.surface.digest() != surface.digest() {
                                        errors += 1;
                                        broken.push(finding(
                                            "mission",
                                            [0; 3],
                                            "nondeterministic-final-scene",
                                        ));
                                    }
                                }
                            }
                            if let Some(area) = &area {
                                let (sam_count, aaa_count) = site.area_site_counts(area);
                                let requested = (sam_count + aaa_count) * layout::AREA_SITE_UNITS;
                                let actual = layout::area::units(&surface).count();
                                if requested != actual {
                                    broken.push(finding("area", [0; 3], format!("missing-area-units:requested={requested},actual={actual}")));
                                }
                            }
                            errors += broken
                                .iter()
                                .filter(|f| {
                                    f.rule.starts_with("runway-clearance-datum-error")
                                        || f.rule == "scene-placement-stale"
                                        || f.rule.starts_with("placement-refused:")
                                })
                                .count();
                            errors += broken
                                .iter()
                                .filter(|f| {
                                    (f.object.starts_with("0x580") || f.object.starts_with("0x5c0"))
                                        && matches!(
                                            f.rule.as_str(),
                                            "land-on-water"
                                                | "outside-map"
                                                | "footprint-outside-map"
                                                | "footprint-terrain-class"
                                                | "vehicle-footprint-slope"
                                                | "vehicle-runway-clearance"
                                                | "vehicle-object-clearance"
                                        )
                                })
                                .count();
                            if let Some(starts) = &surface.starts {
                                let target = objective.unwrap_or(starts.target);
                                let distance = |a: [i32; 2], b: [i32; 2]| {
                                    f64::from(a[0] - b[0]).hypot(f64::from(a[1] - b[1]))
                                        / layout::NM_FT as f64
                                };
                                for (name, p) in [
                                    ("blue-start", starts.blue),
                                    ("red-start", starts.red),
                                    ("objective", target),
                                ] {
                                    if !ground.inside(p.map(i64::from), 0) {
                                        broken.push(finding(name, [p[0], 0, p[1]], "outside-map"));
                                    }
                                }
                                let actual = distance(starts.blue, starts.red);
                                if (actual - separation).abs() > 0.02 {
                                    broken.push(finding("blue-start", [starts.blue[0], 0, starts.blue[1]], format!("separation-shortened:requested={separation:.2},actual={actual:.2}")));
                                }
                                if distance(starts.red, target) > 5.02 {
                                    broken.push(finding(
                                        "red-start",
                                        [starts.red[0], 0, starts.red[1]],
                                        "defender-over-5nm-from-objective",
                                    ));
                                }
                                if (distance(starts.blue, target) - separation).abs() > 5.02 {
                                    broken.push(finding(
                                        "objective",
                                        [target[0], 0, target[1]],
                                        "objective-distance-outside-separation-plus-minus-5.02nm",
                                    ));
                                }
                            }
                            // The strict table includes authored exceptions. Only existing
                            // relocated-site invariants fail the command's regression gate.
                            let gate = site.audit(resources, &terrain.theater, &raw);
                            errors += gate.len();
                            for problem in gate {
                                broken.push(finding("placement-contract", [0; 3], problem));
                            }
                            for f in &broken {
                                println!(
                                    "finding\t{theater}\t{separation}\t{stem}\t{aaa}\t{sam}\t{seed}\t{}\t{}\t{}\t{}\t{}",
                                    f.object, f.at[0], f.at[1], f.at[2], f.rule
                                );
                            }
                            cases += 1;
                            findings += broken.len();
                            println!(
                                "objective-case: {theater} {stem} separation {separation} aaa {aaa} sam {sam} seed {seed} units {} parked {} findings {} digest {:#018x}",
                                surface.units.len(),
                                surface.parked.len(),
                                broken.len(),
                                surface.digest()
                            );
                        }
                    }
                }
            }
        }
        eprintln!(
            "objective-audit-progress: {theater} cases {cases} findings {findings} errors {errors}"
        );
    }
    println!("objective-cache: scene-builds {scene_builds} cache-hits {cache_hits}");
    println!("objective-audit: cases {cases} findings {findings} errors {errors}");
    if errors > 0 {
        return Err(format!("{errors} placement contract failures").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlap_uses_rotated_footprints_and_excludes_touching_edges() {
        let a = Placed {
            id: 1,
            at: [0, 0],
            heading: 0,
            footprint: layout::Footprint {
                min: [-10, -100],
                max: [10, 100],
            },
        };
        assert!(!overlaps(a, Placed { at: [20, 0], ..a }));
        assert!(overlaps(a, Placed { heading: 90, ..a }));
        assert!(!overlaps(
            a,
            Placed {
                at: [111, 0],
                heading: 90,
                ..a
            }
        ));
    }
}
