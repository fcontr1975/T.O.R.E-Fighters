//! Offline placement audit with the same solver as launch: what the distance
//! table generator measures, and what launch refuses.
use super::*;
use crate::{
    WorldResult,
    mission::MissionSpec,
    resources::ResourceSource,
    surface::{layout, resolve},
    terrain::SurfaceSite,
};

#[derive(Clone, Debug)]
pub struct Availability {
    pub separations: [bool; 11],
    pub limitations: Vec<String>,
    pub blocked: bool,
}

/// Shared by normal mission construction and the older headless entry point.
pub fn surface_problem(
    surface: &crate::surface::Surface,
    separation_nm: u32,
    airborne: bool,
    inspect_invalid: bool,
) -> Option<String> {
    if airborne && let Some(starts) = &surface.starts {
        let distance = f64::from(starts.blue[0] - starts.red[0])
            .hypot(f64::from(starts.blue[1] - starts.red[1]));
        if (distance - f64::from(separation_nm) * layout::NM_FT as f64).abs() > 2. {
            return Some(distance_problem(separation_nm));
        }
    }
    if !inspect_invalid
        && let Some(reason) = surface.layout_notes.iter().find(|n| {
            n.starts_with("placement unresolved ") || n.starts_with("front clearance unresolved:")
        })
    {
        return Some(format!(
            "Vehicle placement unavailable: {reason}. Choose another target or seed."
        ));
    }
    None
}

pub fn distance_problem(separation_nm: u32) -> String {
    format!("Separation {separation_nm} nm does not fit this setup. Choose a shorter distance.")
}

/// Audit this imported theater, target, seed and wing geometry: which of
/// the eleven distances fit, and what the layout leaves out. Offline only:
/// the creator and the server read the checked-in [`super::distance_table`]
/// that `examples/qmc_distance_table.rs` generates with it, so no menu or
/// server change waits for a terrain build (docs/DEVELOPMENT.md).
pub fn audit(resources: &dyn ResourceSource, spec: &MissionSpec) -> WorldResult<Availability> {
    let target = resolve::GroundTarget::from_spec(spec);
    let area = resolve::AreaDefense::from_spec(spec);
    let terrain = Terrain::for_mission_surface(
        resources,
        &spec.theater,
        Some(spec.condition.index()),
        &spec.weather,
        target.as_ref(),
        area.as_ref(),
    )?;
    let site = SurfaceSite::load(resources, &spec.theater)?;
    let separations = separations(&terrain, &site, spec)?;
    let surface = &terrain.surface;
    let mut limitations = Vec::new();
    if let Some(t) = &surface.template
        && !t.left_out.is_empty()
    {
        let mut missing = std::collections::BTreeMap::new();
        for item in &t.left_out {
            let label = tore_formats::aircraft::AircraftId::parse(&item.resource)
                .map_or("target object", |id| id.label());
            let why = if item.why == "on no carrier's deck" {
                "no supported carrier deck"
            } else {
                "empty target slot"
            };
            *missing.entry((label, why)).or_insert(0usize) += 1;
        }
        limitations.push(format!(
            "Unavailable: {}.",
            missing
                .into_iter()
                .map(|((label, why), count)| format!("{count} {label} ({why})"))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if let Some(area) = area.as_ref() {
        let (sam, aaa) = site.area_site_counts(area);
        let requested = (sam + aaa) * layout::AREA_SITE_UNITS;
        let placed = layout::area::units(surface).count();
        if requested != placed {
            limitations.push(format!(
                "This map holds {placed} of {requested} area-defense units."
            ));
        }
    }
    let unresolved = surface
        .layout_notes
        .iter()
        .filter(|n| {
            n.starts_with("placement unresolved ") || n.starts_with("front clearance unresolved:")
        })
        .count();
    if unresolved > 0 {
        limitations.push(format!("{unresolved} vehicles have no valid nearby placement. Equipment is retained; choose the target again for a new layout, or choose another target."));
    }
    Ok(Availability {
        separations,
        limitations,
        blocked: unresolved > 0,
    })
}

/// Which of the eleven distances fit `spec` in this already built `terrain`
/// (with `spec`'s target and seed), by the same rules a launch applies. A
/// ground start with a ground target fits every distance: the template
/// places the enemy. With a target the starts follow it; without one the
/// enemy group, in `spec`'s situation, must fit the map at full distance
/// along some bearing from the player's start.
pub fn separations(
    terrain: &Terrain,
    site: &SurfaceSite,
    spec: &MissionSpec,
) -> WorldResult<[bool; 11]> {
    if spec.start.on_ground() && spec.ground_target.is_some() {
        return Ok([true; 11]);
    }
    if terrain.surface.template.is_some() {
        let ground = layout::Ground::new(&terrain.theater, vec![], vec![], site.front());
        let mut surface = terrain.surface.clone();
        let mut fits = [false; 11];
        for (fit, nm) in fits.iter_mut().zip(SEPARATION_NM) {
            if let Some(template) = &mut surface.template {
                template.settings.separation_nm = nm as u32;
            }
            *fit =
                layout::scene_starts(&surface, &ground, &terrain.airport_scene).is_some_and(|s| {
                    let distance =
                        f64::from(s.blue[0] - s.red[0]).hypot(f64::from(s.blue[1] - s.red[1]));
                    (distance - nm * layout::NM_FT as f64).abs() < 2.
                });
        }
        return Ok(fits);
    }
    let wings = spec.wing_launches().map_err(|e| e.to_string())?;
    let start = terrain.free_flight_start();
    let selected = match spec.start {
        crate::mission::Start::Ground { runway, .. } => Some(runway),
        crate::mission::Start::GroundAuto { .. } => {
            auto_runway_for(terrain, spec.player_wing_size(), false).ok()
        }
        _ => None,
    };
    let reference = selected
        .and_then(|id| ground_layout(terrain, id, spec.player_wing_size()).ok())
        .map_or([start[0], start[2]], |g| [g.slots[0][0], g.slots[0][2]]);
    Ok(separations_from(
        reference,
        &wings,
        effective_situation(spec.situation, spec.start.on_ground(), false),
        map_bounds(terrain),
    ))
}

/// [`separations`] without a ground target: the enemy group of `wings` in
/// `situation`, placed from `reference`, inside `bounds`.
pub fn separations_from(
    reference: [f64; 2],
    wings: &[tore_sim::ai::launch::WingLaunch],
    situation: crate::mission::Situation,
    bounds: MapBounds,
) -> [bool; 11] {
    let group = crate::ai_wings::enemy_group_offsets(wings, situation);
    SEPARATION_NM.map(|nm| {
        !aim_into_map(
            reference,
            situation_bearing(situation),
            nm * FEET_PER_NM,
            &group,
            bounds,
        )
        .shortened()
    })
}
