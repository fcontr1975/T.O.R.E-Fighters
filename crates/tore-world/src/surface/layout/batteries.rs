//! SAM batteries (docs/spec/surface-defenses.md, "SAM batteries"): SA-2,
//! SA-3, SA-6 and HAWK launchers clustered into batteries around a search
//! radar, an existing radar adopted or an element added.
use super::{
    Added, Inputs, Obstacles, Pose, added_unit, centroid, distance2, ordinal_of, ring, stands,
    stream_for, xz, *,
};
use crate::surface::{Battery, BatterySystem, IdRange, Surface, UnitId, catalog};
use std::collections::{BTreeMap, BTreeSet};
use tore_sim::combat::live::{NO_SIDE, Side};

/// The radar element of a system: the LIB radar for SA-2, SA-3 and SA-6,
/// the TORE-defined HAWK radar for HAWK.
pub fn element(system: BatterySystem) -> &'static str {
    system.radar().unwrap_or(catalog::HAWK_RADAR)
}

/// One launcher as the battery rules see it.
#[derive(Clone, Copy, Debug)]
struct Launcher {
    id: UnitId,
    /// Where the rules cluster it: the template's own frame for template
    /// units, the world for the layout's.
    frame: [i64; 2],
}

/// A battery before its radar is settled.
struct Draft {
    system: BatterySystem,
    side: Side,
    /// Which pool it formed in ([`pool`]).
    pool: u8,
    template: bool,
    launchers: Vec<Launcher>,
}

/// The pool a launcher or radar forms batteries in: the base layout's (0),
/// the template's (1) or the area defenses' (2). Pools never share a
/// battery or a radar.
fn pool(id: UnitId) -> u8 {
    match id.range() {
        IdRange::Template => 1,
        IdRange::AreaDefense => 2,
        _ => 0,
    }
}

/// Single-linkage clusters of `launchers` (ascending id) within
/// [`BATTERY_CLUSTER_FT`], each ascending id, ordered by lowest id.
fn clusters(launchers: &[Launcher]) -> Vec<Vec<Launcher>> {
    let n = launchers.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let limit = BATTERY_CLUSTER_FT * BATTERY_CLUSTER_FT;
    for i in 0..n {
        for j in i + 1..n {
            if distance2(launchers[i].frame, launchers[j].frame) <= limit {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                // The lower index stays the root, so roots are lowest ids.
                parent[a.max(b)] = a.min(b);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<Launcher>> = BTreeMap::new();
    for (i, launcher) in launchers.iter().enumerate() {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(*launcher);
    }
    groups.into_values().collect()
}

/// A cluster larger than `cap` split into `ceil(n / cap)` batteries: seeds
/// are the lowest-id launcher, then each time the launcher farthest from
/// every seed (lower id on a tie); the others, in ascending id order, join
/// the nearest seed's battery still under the cap (lower seed on a tie).
fn split(cluster: Vec<Launcher>, cap: usize) -> Vec<Vec<Launcher>> {
    if cluster.len() <= cap {
        return vec![cluster];
    }
    let count = cluster.len().div_ceil(cap);
    let mut seeds = vec![0usize];
    while seeds.len() < count {
        let far = (0..cluster.len())
            .filter(|i| !seeds.contains(i))
            .max_by_key(|&i| {
                let d = seeds
                    .iter()
                    .map(|&s| distance2(cluster[i].frame, cluster[s].frame))
                    .min()
                    .unwrap_or(0);
                (d, std::cmp::Reverse(i))
            })
            .expect("more launchers than seeds");
        seeds.push(far);
    }
    let mut groups: Vec<Vec<Launcher>> = seeds.iter().map(|&s| vec![cluster[s]]).collect();
    for (i, launcher) in cluster.iter().enumerate() {
        if seeds.contains(&i) {
            continue;
        }
        let best = (0..groups.len())
            .filter(|&g| groups[g].len() < cap)
            .min_by_key(|&g| (distance2(launcher.frame, cluster[seeds[g]].frame), g))
            .expect("enough room in the batteries");
        groups[best].push(*launcher);
    }
    for group in &mut groups {
        group.sort_by_key(|l| l.id);
    }
    groups.sort_by_key(|g| g[0].id);
    groups
}

/// Forms every battery of the surface, in battery order (ascending lowest
/// launcher id), adopting radars or adding them (`added`). A battery whose
/// radar element the import cannot place is not formed: its launchers stay
/// self-contained, and `notes` says so.
pub(super) fn form(
    surface: &Surface,
    inputs: &mut Inputs<'_, '_>,
    frames: &BTreeMap<UnitId, [i64; 2]>,
    obstacles: &Obstacles<'_>,
    added: &mut Vec<Added>,
    notes: &mut Vec<String>,
) -> Vec<Battery> {
    // Launchers by (origin, system, side): templates, base layouts and area
    // defenses never share a battery.
    let mut pools: BTreeMap<(u8, BatterySystem, u32), Vec<Launcher>> = BTreeMap::new();
    for unit in &surface.units {
        let Some(system) = BatterySystem::of_launcher(&unit.resource) else {
            continue;
        };
        if unit.side == NO_SIDE {
            continue;
        }
        pools
            .entry((pool(unit.id), system, unit.side.0))
            .or_default()
            .push(Launcher {
                id: unit.id,
                frame: frames.get(&unit.id).copied().unwrap_or(xz(unit.position)),
            });
    }
    let mut drafts: Vec<Draft> = Vec::new();
    for ((pool, system, side), mut launchers) in pools {
        launchers.sort_by_key(|l| l.id);
        for cluster in clusters(&launchers) {
            for launchers in split(cluster, system.cap()) {
                drafts.push(Draft {
                    system,
                    side: Side(side),
                    pool,
                    template: pool == 1,
                    launchers,
                });
            }
        }
    }
    drafts.sort_by_key(|d| d.launchers[0].id);
    let mut adopted: BTreeSet<UnitId> = BTreeSet::new();
    let mut batteries = Vec::new();
    let mut next_radar = 0u32;
    for draft in drafts {
        let frame_centre = centroid(draft.launchers.iter().map(|l| l.frame)).expect("a launcher");
        let lead = surface
            .unit(draft.launchers[0].id)
            .expect("a launcher is a unit");
        // Adoption: the nearest existing radar of the element type, same
        // side and origin, within reach, not yet adopted.
        let reach = BATTERY_ADOPT_FT * BATTERY_ADOPT_FT;
        let found = draft.system.radar().and_then(|radar| {
            surface
                .units
                .iter()
                .filter(|u| {
                    u.resource.eq_ignore_ascii_case(radar)
                        && u.side == draft.side
                        && pool(u.id) == draft.pool
                        && !adopted.contains(&u.id)
                })
                .map(|u| {
                    let at = frames.get(&u.id).copied().unwrap_or(xz(u.position));
                    (distance2(at, frame_centre), u.id)
                })
                .filter(|(d, _)| *d <= reach)
                .min()
                .map(|(_, id)| id)
        });
        let (radar, radar_added) = match found {
            Some(id) => {
                adopted.insert(id);
                (id, false)
            }
            None => {
                let kind = element(draft.system);
                let Some(entry) = (inputs.added)(kind) else {
                    notes.push(format!(
                        "no {kind} to add for the {:?} battery of {:#010x}: its launchers stay self-contained",
                        draft.system, draft.launchers[0].id.0
                    ));
                    continue;
                };
                let Some(id) = UnitId::battery_radar(next_radar) else {
                    continue;
                };
                next_radar += 1;
                let (pose, valid) = radar_pose(surface, inputs, &draft, obstacles, &entry, added);
                if !valid {
                    notes.push(format!("support fallback {:#010x} {kind}: no valid spot for the {:?} battery of {:#010x}; retained for final footprint placement", id.0, draft.system, draft.launchers[0].id.0));
                }
                let mut unit = added_unit(id, &entry, pose, lead);
                unit.skill = draft
                    .launchers
                    .iter()
                    .filter_map(|l| surface.unit(l.id))
                    .map(|u| u.skill)
                    .max()
                    .unwrap_or(unit.skill);
                let source = match (&surface.template, draft.template) {
                    (Some(site), true) => format!("~{}.M", site.stem),
                    _ => inputs.layout.to_owned(),
                };
                added.push(Added { unit, source });
                (id, true)
            }
        };
        batteries.push(Battery {
            system: draft.system,
            side: draft.side,
            radar,
            radar_added,
            launchers: draft.launchers.iter().map(|l| l.id).collect(),
            truck: None,
        });
    }
    batteries
}

/// Where an added radar stands: up to [`SUPPORT_CANDIDATES`] seeded spots
/// in the ring around the launchers' placed centroid, clear of them; the
/// first valid, unoccupied spot wins. The fallback is explicitly reported.
fn radar_pose(
    surface: &Surface,
    inputs: &mut Inputs<'_, '_>,
    draft: &Draft,
    obstacles: &Obstacles<'_>,
    entry: &catalog::Entry,
    added: &[Added],
) -> (Pose, bool) {
    let placed: Vec<([i64; 2], i64)> = draft
        .launchers
        .iter()
        .filter_map(|l| surface.unit(l.id))
        .map(|u| {
            (
                xz(u.position),
                (inputs.types)(&u.resource).footprint.radius(),
            )
        })
        .collect();
    let centre = centroid(placed.iter().map(|(at, _)| *at)).expect("a launcher");
    let size = (inputs.types)(&entry.resource).footprint.radius();
    let lead = surface.unit(draft.launchers[0].id).expect("a launcher");
    let mut draws = stream_for(
        surface,
        inputs.layout,
        draft.template,
        ordinal_of(lead),
        Purpose::Radar,
    );
    let band = if draft.system == BatterySystem::Sa2 {
        SA2_RADAR_FT
    } else {
        BATTERY_RADAR_FT
    };
    let occupied = super::occupied(
        surface.units.iter().chain(added.iter().map(|a| &a.unit)),
        inputs.types,
    );
    let mut first = None;
    for _ in 0..SUPPORT_CANDIDATES {
        let (offset, bearing) = ring(&mut draws, band);
        let pose = Pose {
            at: [centre[0] + offset[0], centre[1] + offset[1]],
            // The radar faces away from its launchers.
            heading: bearing,
        };
        first.get_or_insert(pose);
        let clear = placed
            .iter()
            .all(|(at, r)| distance2(*at, pose.at) >= (r + size).pow(2));
        if clear
            && clear_of_units(pose.at, size, &occupied)
            && stands(inputs.ground, obstacles, pose.at, size, false)
        {
            return (pose, true);
        }
    }
    (first.expect("a nonempty support search"), false)
}
