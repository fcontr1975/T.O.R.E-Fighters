//! Fitted vehicle placement policy, applied after airport replacement.
use super::*;
use tore_formats::surface_unit::class;

/// Maximum rise per 100 horizontal feet. Agent fitted defaults, not retail
/// measurements: tanks 30%; vehicles, guns, launchers and added radars 20%.
/// John requested loosening the latter policy on 2026-10-26.
pub fn slope_limit(unit: &Unit) -> i64 {
    if unit.class & class::TANK != 0 {
        30
    } else if unit.class & (class::SAM | class::AAA | class::VEHICLE) != 0
        || unit.id.range() == super::super::IdRange::BatteryRadar
    {
        20
    } else {
        10
    }
}

pub fn vehicle(unit: &Unit) -> bool {
    unit.kind != super::super::UnitKind::Structure
        && unit.class & class::SHIP == 0
        && (unit.class & class::STRUCTURE == 0
            || unit.id.range() == super::super::IdRange::BatteryRadar)
        && unit.resource != BARRAGE_ZONE
}

/// Nine samples on the oriented shape footprint, with pairwise grade tests.
/// Integer interpolated terrain heights retain deterministic placement.
pub fn slope_ok(ground: &Ground<'_>, p: Placed, limit: i64) -> bool {
    let xs = [
        p.footprint.min[0],
        (p.footprint.min[0] + p.footprint.max[0]) / 2,
        p.footprint.max[0],
    ];
    let zs = [
        p.footprint.min[1],
        (p.footprint.min[1] + p.footprint.max[1]) / 2,
        p.footprint.max[1],
    ];
    let mut samples = [([0i64; 2], 0i64); 9];
    let mut sample = 0;
    for x in xs {
        for z in zs {
            let d = trig::rotate([x, z], p.heading);
            let at = [p.at[0] + d[0], p.at[1] + d[1]];
            if !ground.inside(at, 0) {
                return false;
            }
            samples[sample] = (at, ground.height_scaled(at));
            sample += 1;
        }
    }
    for (i, (a, ha)) in samples.iter().enumerate() {
        for (b, hb) in &samples[i + 1..] {
            let rise = i128::from((ha - hb).abs()) * 100;
            let run2 = i128::from(distance2(*a, *b));
            let scale = i128::from(limit * ground::CELL_FT);
            if rise * rise > run2 * scale * scale {
                return false;
            }
        }
    }
    true
}

/// Independent categories for both placement and the objective audit.
pub fn violations(ground: &Ground<'_>, unit: &Unit, info: TypeInfo) -> Vec<&'static str> {
    let p = Placed {
        id: unit.id.0,
        at: xz(unit.position),
        heading: unit.angles[0],
        footprint: info.footprint,
    };
    let mut out = Vec::new();
    let r = info.footprint.radius();
    let corners = [
        p.at,
        [p.at[0] - r, p.at[1] - r],
        [p.at[0] + r, p.at[1] - r],
        [p.at[0] - r, p.at[1] + r],
        [p.at[0] + r, p.at[1] + r],
    ];
    if corners.iter().any(|at| !ground.inside(*at, 0)) {
        out.push("map-bounds");
    }
    if corners.iter().any(|at| ground.water(*at)) {
        out.push("terrain-class");
    }
    if !slope_ok(ground, p, slope_limit(unit)) {
        out.push("footprint-slope");
    }
    if ground.near_runway(p.at, r + RUNWAY_CLEARANCE_FT) {
        out.push("runway-clearance");
    }
    if ground
        .objects
        .iter()
        .any(|o| o.id != p.id && o.within(p.at, r))
    {
        out.push("object-clearance");
    }
    out
}

/// Nearest valid point on a 25-foot lattice within one 8,192-foot cell. Ties use X,
/// then Z. Keep every unit and report exhaustion so launch can explain it.
pub fn repair(surface: &mut Surface, ground: &Ground<'_>, types: &mut dyn FnMut(&str) -> TypeInfo) {
    let infos: BTreeMap<_, _> = surface
        .units
        .iter()
        .map(|u| (u.resource.clone(), types(&u.resource)))
        .collect();
    static OFFSETS: std::sync::OnceLock<Vec<[i64; 2]>> = std::sync::OnceLock::new();
    let offsets = OFFSETS.get_or_init(|| {
        let mut offsets = Vec::new();
        let steps = ground::CELL_FT / 25;
        for x in -steps..=steps {
            for z in -steps..=steps {
                if (x * x + z * z) * 25 * 25 <= ground::CELL_FT.pow(2) {
                    offsets.push([x * 25, z * 25]);
                }
            }
        }
        offsets.sort_by_key(|d| (d[0] * d[0] + d[1] * d[1], d[0], d[1]));
        offsets
    });
    surface
        .layout_notes
        .retain(|n| !n.starts_with("support fallback ") && !n.starts_with("placement unresolved "));
    for index in 0..surface.units.len() {
        let original = surface.units[index].clone();
        if !vehicle(&original) {
            continue;
        }
        let info = infos[&original.resource];
        let radius = info.footprint.radius();
        let generated = matches!(
            original.id.range(),
            super::super::IdRange::SupplyTruck
                | super::super::IdRange::BatteryRadar
                | super::super::IdRange::AreaDefense
        );
        if !generated {
            let rules = violations(ground, &original, info);
            if !rules.contains(&"footprint-slope") && !rules.contains(&"map-bounds") {
                continue;
            }
        }
        let serves = surface
            .trucks
            .iter()
            .find(|t| t.id == original.id)
            .and_then(|t| t.serves)
            .and_then(|id| surface.unit(id))
            .map(|u| xz(u.position));
        let occupied: Vec<_> = surface
            .units
            .iter()
            .filter(|u| u.id != original.id && u.resource != BARRAGE_ZONE)
            .filter_map(|u| {
                let i = infos[&u.resource];
                (!i.strip && !i.bridge_or_road).then_some((xz(u.position), i.footprint.radius()))
            })
            .collect();
        let valid = |u: &Unit| {
            serves.is_none_or(|at| distance2(at, xz(u.position)) <= 528 * 528)
                && clear_of_units(xz(u.position), radius, &occupied)
                && violations(ground, u, info).is_empty()
        };
        if valid(&original) {
            continue;
        }
        let at = xz(original.position);
        let mut candidate = original.clone();
        let found = offsets.iter().find_map(|d| {
            let unit = &mut candidate;
            unit.position[0] = (at[0] + d[0]) as i32;
            unit.position[2] = (at[1] + d[1]) as i32;
            valid(unit).then(|| unit.clone())
        });
        if let Some(unit) = found {
            if let Some(course) = surface.courses.get_mut(&unit.id) {
                course.start[0] = unit.position[0];
                course.start[2] = unit.position[2];
            }
            surface.units[index] = unit;
        } else {
            let mut reasons = violations(ground, &original, info);
            if !clear_of_units(at, radius, &occupied) {
                reasons.push("unit-clearance");
            }
            if serves.is_some_and(|p| distance2(p, at) > 528 * 528) {
                reasons.push("supply-reach");
            }
            surface.layout_notes.push(format!(
                "placement unresolved {:#010x} {}: {}; equipment retained",
                original.id.0,
                original.resource,
                reasons.join(",")
            ));
        }
    }
    for (id, p) in &mut surface.placements {
        if let Some(u) = surface.units.iter().find(|u| u.id == *id) {
            p.position = u.position;
            p.angles = u.angles;
        }
    }
}
