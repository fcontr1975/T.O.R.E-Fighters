//! Bridge footing. Joined spans share one datum instead of sampling unrelated
//! terrain heights at each piece's origin. Fitted, see docs/spec/bridges.md.
use crate::terrain::Placements;
use tore_formats::{mission::Placement, shape::Shape};
use tore_sim::attitude::Basis;

/// The source bridge inventory is the bridge portion of the map-tied list.
pub fn is_bridge(name: &str) -> bool {
    crate::terrain::MAP_TIED_TYPES[..12]
        .iter()
        .any(|n| n.eq_ignore_ascii_case(name))
}

struct Part<'a> {
    placement: &'a Placement,
    shape: &'a Shape,
    scale: f64,
    basis: Basis,
    bounds: [[f64; 2]; 2],
}

impl<'a> Part<'a> {
    fn new(placement: &'a Placement, shape: &'a Shape, scale: f64) -> Self {
        let basis = Basis::new(
            f64::from(placement.angles[0]).to_radians(),
            f64::from(placement.angles[1]).to_radians(),
            f64::from(placement.angles[2]).to_radians(),
        );
        let mut part = Self {
            placement,
            shape,
            scale,
            basis,
            bounds: [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]],
        };
        for point in super::points(shape) {
            let at = part.at(point.map(f64::from));
            for (axis, world_axis) in [0, 2].into_iter().enumerate() {
                part.bounds[0][axis] = part.bounds[0][axis].min(at[world_axis]);
                part.bounds[1][axis] = part.bounds[1][axis].max(at[world_axis]);
            }
        }
        part
    }
    fn at(&self, p: [f64; 3]) -> [f64; 3] {
        super::placed(
            [p[0] * self.scale, p[2] * self.scale, p[1] * self.scale],
            &self.basis,
            [
                f64::from(self.placement.position[0]),
                0.,
                f64::from(self.placement.position[2]),
            ],
        )
    }
    fn joins(&self, other: &Self) -> bool {
        let name = &self.placement.object_type;
        let other_name = &other.placement.object_type;
        // Only the authored END/MID assemblies join. Complete BRD1..4
        // bridges remain independent even when their envelopes meet.
        fn family(s: &str) -> Option<&str> {
            s.strip_suffix("END.OT")
                .or_else(|| s.strip_suffix("MID.OT"))
        }
        family(name).is_some()
            && family(name) == family(other_name)
            && (self.basis.forward[0] * other.basis.forward[0]
                + self.basis.forward[2] * other.basis.forward[2])
                .abs()
                > 0.99
            && (0..2).all(|a| {
                self.bounds[0][a] <= other.bounds[1][a] + 16.
                    && other.bounds[0][a] <= self.bounds[1][a] + 16.
            })
    }
    fn support(&self, height: &impl Fn(f64, f64) -> f64) -> f64 {
        let mut datum = f64::NEG_INFINITY;
        let mut sample = |p: [f64; 3]| {
            let at = self.at(p);
            datum = datum.max(height(at[0], at[2]).max(0.) - at[1]);
        };
        for point in super::points(self.shape) {
            sample(point.map(f64::from));
        }
        // Faces crossing a ridge need interior samples too. 64 ft is a
        // fitted bound, much smaller than the 8,192 ft source terrain cell.
        for face in &self.shape.faces {
            if face.positions.len() < 3 {
                continue;
            }
            let a = face.positions[0].map(f64::from);
            for edge in face.positions[1..].windows(2) {
                let b = edge[0].map(f64::from);
                let c = edge[1].map(f64::from);
                let length = (b[0] - a[0])
                    .hypot(b[1] - a[1])
                    .max((c[0] - a[0]).hypot(c[1] - a[1]))
                    * self.scale;
                let steps = (length / 64.).ceil().clamp(1., 256.) as usize;
                for i in 0..=steps {
                    for j in 0..=steps - i {
                        sample(std::array::from_fn(|k| {
                            a[k] + (b[k] - a[k]) * i as f64 / steps as f64
                                + (c[k] - a[k]) * j as f64 / steps as f64
                        }));
                    }
                }
            }
        }
        datum
    }
}

/// Absolute vertical datum for this bridge's assembly. Authored relative
/// heights and all three source angles stay intact. Neither terrain nor roads
/// are changed. The intact shape determines the datum of its destroyed copy.
pub fn datum(sources: &Placements, p: &Placement, height: impl Fn(f64, f64) -> f64) -> Option<f64> {
    sources.shapes.get(&p.object_type)?;
    assembly(sources, p)
        .iter()
        .map(|part| part.support(&height))
        .reduce(f64::max)
        .filter(|v| v.is_finite())
}

fn assembly<'a>(sources: &'a Placements, p: &'a Placement) -> Vec<Part<'a>> {
    let mut parts = vec![Part::new(
        p,
        sources
            .shapes
            .get(&p.object_type)
            .expect("checked bridge shape"),
        sources.shape_scale(&p.object_type),
    )];
    for (_, candidate) in sources.placed().flatten() {
        if candidate.key != p.key
            && is_bridge(&candidate.object_type)
            && let Some(shape) = sources.shapes.get(&candidate.object_type)
        {
            parts.push(Part::new(
                candidate,
                shape,
                sources.shape_scale(&candidate.object_type),
            ));
        }
    }
    let mut included = vec![false; parts.len()];
    included[0] = true;
    loop {
        let mut changed = false;
        for i in 0..parts.len() {
            if !included[i] && (0..parts.len()).any(|j| included[j] && parts[i].joins(&parts[j])) {
                included[i] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    parts
        .into_iter()
        .zip(included)
        .filter_map(|(part, keep)| keep.then_some(part))
        .collect()
}

/// Physical support samples for the whole connected assembly. A suspended
/// MID is checked against its END feet, not against its shadow on the river.
pub fn footing_points(sources: &Placements, p: &Placement) -> Vec<[f64; 3]> {
    if !sources.shapes.contains_key(&p.object_type) {
        return Vec::new();
    }
    assembly(sources, p)
        .iter()
        .flat_map(|part| {
            let low = super::bottom(part.shape).unwrap_or(0.);
            support_points(part.shape)
                .filter(move |p| f64::from(p[2]) <= low + 1. / part.scale)
                .map(|point| {
                    let mut at = part.at(point.map(f64::from));
                    at[1] += f64::from(part.placement.position[1]) - f64::from(p.position[1]);
                    at
                })
        })
        .collect()
}

/// Terrain support for zero-height, level authored bridge pieces. The shared
/// foundation builder merges adjoining pieces and blends their perimeter.
/// Lowering the corridor to its lowest sampled terrain avoids raising a river
/// or suspending the low end of a bridge above it.
pub fn foundations(sources: &Placements) -> Vec<super::pads::Pad> {
    sources
        .placed()
        .flatten()
        .filter_map(|(_, p)| {
            if !is_bridge(&p.object_type)
                || p.position[1] != 0
                || p.angles[1] != 0
                || p.angles[2] != 0
            {
                return None;
            }
            let part = Part::new(
                p,
                sources.shapes.get(&p.object_type)?,
                sources.shape_scale(&p.object_type),
            );
            Some(super::pads::Pad {
                min: part.bounds[0].map(|v| v - 2.),
                max: part.bounds[1].map(|v| v + 2.),
                height: 0.,
            })
        })
        .collect()
}

/// Maximum grade of new road approaches. Side slopes use a 1:4 embankment.
pub const APPROACH_GRADE: f64 = 0.08;
const SIDE_GRADE: f64 = 0.25;

/// Bank earthwork, separate from flat foundations so the river stays open.
#[derive(Clone, Debug)]
pub struct Bank {
    pub core: super::pads::Pad,
    pub bounds: super::pads::Pad,
    // Negative/positive world-axis grades; only the outward road is gentle.
    grades: [[f64; 2]; 2],
}
impl Bank {
    /// Centreline from the road toe outward to untouched terrain.
    pub fn approach(&self) -> [[f64; 2]; 2] {
        let axis = usize::from(self.grades[1].contains(&APPROACH_GRADE));
        let side = usize::from(self.grades[axis][1] == APPROACH_GRADE);
        let mut start = std::array::from_fn(|a| (self.core.min[a] + self.core.max[a]) * 0.5);
        start[axis] = if side == 0 {
            self.core.min[axis]
        } else {
            self.core.max[axis]
        };
        let mut end = start;
        end[axis] = if side == 0 {
            self.bounds.min[axis]
        } else {
            self.bounds.max[axis]
        };
        [start, end]
    }
    pub fn height(&self, x: f64, z: f64, raw: f64) -> f64 {
        let mut drop = 0.;
        for (axis, at) in [x, z].into_iter().enumerate() {
            drop += (self.core.min[axis] - at).max(0.) * self.grades[axis][0]
                + (at - self.core.max[axis]).max(0.) * self.grades[axis][1];
        }
        raw.max(self.core.height - drop)
    }
}

/// Replace only corridors that would cut more than one foot of bank.
/// Their physical feet get a common high-bank datum and filled approaches.
/// A span's shadow decal is not a foundation across the river.
pub fn graded_foundations(
    sources: &Placements,
    raw: impl Fn(f32, f32) -> f32,
    allow: impl Fn(&super::pads::Pad) -> bool,
) -> (Vec<super::pads::Pad>, Vec<Bank>) {
    let mut flat = foundations(sources);
    let parts: Vec<_> = sources
        .placed()
        .flatten()
        .filter_map(|(_, p)| {
            (is_bridge(&p.object_type)
                && p.position[1] == 0
                && p.angles[1] == 0
                && p.angles[2] == 0)
                .then_some(p)
                .and_then(|p| {
                    Some(Part::new(
                        p,
                        sources.shapes.get(&p.object_type)?,
                        sources.shape_scale(&p.object_type),
                    ))
                })
        })
        .collect();
    let mut used = vec![false; parts.len()];
    let mut banks = Vec::new();
    for start in 0..parts.len() {
        if used[start] {
            continue;
        }
        let mut group = vec![start];
        used[start] = true;
        let mut i = 0;
        while i < group.len() {
            for j in 0..parts.len() {
                if !used[j] && parts[group[i]].joins(&parts[j]) {
                    used[j] = true;
                    group.push(j);
                }
            }
            i += 1;
        }
        let min = std::array::from_fn::<_, 2, _>(|a| {
            group
                .iter()
                .map(|i| parts[*i].bounds[0][a])
                .fold(f64::INFINITY, f64::min)
        });
        let max = std::array::from_fn::<_, 2, _>(|a| {
            group
                .iter()
                .map(|i| parts[*i].bounds[1][a])
                .fold(f64::NEG_INFINITY, f64::max)
        });
        let steps = [0, 1].map(|a| ((max[a] - min[a]) / 64.).ceil().max(1.) as usize);
        let mut low = f64::INFINITY;
        let mut high: f64 = 0.;
        for x in 0..=steps[0] {
            for z in 0..=steps[1] {
                let h = f64::from(raw(
                    (min[0] + (max[0] - min[0]) * x as f64 / steps[0] as f64) as f32,
                    (min[1] + (max[1] - min[1]) * z as f64 / steps[1] as f64) as f32,
                ))
                .max(0.);
                low = low.min(h);
                high = high.max(h);
            }
        }
        if high - low <= 1. {
            continue;
        }
        let datum = group
            .iter()
            .map(|i| high - super::bottom(parts[*i].shape).unwrap_or(0.) * parts[*i].scale)
            .fold(f64::NEG_INFINITY, f64::max);
        let axis = usize::from(max[1] - min[1] > max[0] - min[0]);
        let mut group_banks = Vec::new();
        for &i in &group {
            let part = &parts[i];
            let bottom = super::bottom(part.shape).unwrap_or(0.);
            let has_feet =
                support_points(part.shape).any(|p| f64::from(p[2]) <= bottom + 1. / part.scale);
            if !has_feet {
                continue;
            }
            // Include the approach toe, which sits slightly above the tower
            // feet in the source mesh. Supporting feet alone leaves it in air.
            let core = super::pads::Pad {
                min: part.bounds[0].map(|v| v - 2.),
                max: part.bounds[1].map(|v| v + 2.),
                height: datum + bottom * part.scale,
            };
            let mut grades = [[SIDE_GRADE; 2]; 2];
            let side = usize::from(core.min[axis] + core.max[axis] > min[axis] + max[axis]);
            grades[axis][side] = APPROACH_GRADE;
            let bounds = super::pads::Pad {
                min: std::array::from_fn(|a| {
                    core.min[a] - core.height.max(0.) / grades[a][0] - 64.
                }),
                max: std::array::from_fn(|a| {
                    core.max[a] + core.height.max(0.) / grades[a][1] + 64.
                }),
                height: core.height,
            };
            group_banks.push(Bank {
                core,
                bounds,
                grades,
            });
        }
        // Airport support wins for the entire assembly. Dropping one bank
        // after removing its old foundation would leave that end suspended.
        if group_banks.is_empty() || group_banks.iter().any(|b| !allow(&b.bounds)) {
            continue;
        }
        for i in group {
            let part = &parts[i];
            flat.retain(|p| {
                p.min != part.bounds[0].map(|v| v - 2.) || p.max != part.bounds[1].map(|v| v + 2.)
            });
        }
        banks.extend(group_banks);
    }
    (flat, banks)
}

/// Physical feet include tower lines, but never the painted shadow beneath a span.
pub fn support_points(shape: &Shape) -> impl Iterator<Item = [f32; 3]> + '_ {
    let low = super::bottom(shape).unwrap_or(0.) as f32;
    shape
        .faces
        .iter()
        .filter(move |f| !super::bridge_shadow(f, low))
        .flat_map(|f| f.positions.iter().copied())
        .chain(shape.lines.iter().flat_map(|l| l.positions))
}

/// A merged foundation that includes bridge support uses the lowest of its
/// nine terrain samples, clamped to the sea plane. Other foundations retain
/// their existing rule. Fitted sampling and 512 ft skirt, not retail grading.
pub fn support(
    pad: &super::pads::Pad,
    bridges: &[super::pads::Pad],
    raw: impl Fn(f32, f32) -> f32,
) -> Option<f64> {
    if !bridges
        .iter()
        .any(|b| (0..2).all(|a| pad.min[a] <= b.max[a] && b.min[a] <= pad.max[a]))
    {
        return None;
    }
    let mut low = f64::INFINITY;
    for u in [0., 0.5, 1.] {
        for v in [0., 0.5, 1.] {
            low = low.min(f64::from(raw(
                (pad.min[0] + (pad.max[0] - pad.min[0]) * u) as f32,
                (pad.min[1] + (pad.max[1] - pad.min[1]) * v) as f32,
            )));
        }
    }
    Some(low.max(0.))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tore_formats::{mission::Layout, shape::Face};

    fn sources() -> Placements {
        let mut sources = Placements::of_types(&BTreeMap::new(), std::iter::empty());
        sources.layout = Layout::parse("TEST.MM", b"textFormat\nobj\n type BR1END.OT\n pos 0 0 0\n angle 0 0 0\n .\nobj\n type BR1MID.OT\n pos 0 0 190\n angle 0 0 0\n .\nobj\n type BR1END.OT\n pos 0 0 380\n angle 180 0 0\n .\nobj\n type BR1MID.OT\n pos 0 0 10000\n angle 0 0 0\n .\n").unwrap();
        let shape = || Shape {
            faces: vec![Face {
                positions: vec![
                    [-10., -100., -20.],
                    [10., -100., -20.],
                    [10., 100., -20.],
                    [-10., 100., -20.],
                ],
                colors: vec![20],
                fog: Default::default(),
                uv: Vec::new(),
                texture: String::new(),
                subtype: 0x41,
                normal: None,
                address: 0,
            }],
            lines: Vec::new(),
            billboards: Vec::new(),
            state_words: Default::default(),
        };
        sources.shapes.insert("BR1END.OT".into(), shape());
        sources.shapes.insert("BR1MID.OT".into(), shape());
        sources
    }

    #[test]
    fn bridge_bottom_clears_ground_and_water_without_losing_authored_height() {
        let mut sources = sources();
        sources.layout.placements.truncate(1);
        sources.layout.placements[0].position[1] = 30;
        let p = &sources.layout.placements[0];
        // The explicit 30 ft remains above the corrected footing.
        assert_eq!(datum(&sources, p, |_, _| 100.), Some(120.));
        assert_eq!(datum(&sources, p, |_, _| -100.), Some(20.));
        assert!(is_bridge("brd3.ot"));
        assert!(!is_bridge("ROAD.OT"));
    }

    #[test]
    fn the_bridge_inventory_is_exactly_the_twelve_br_types() {
        // `is_bridge` slices the map-tied list; reordering it must not
        // silently turn a road into a bridge or drop a bridge type.
        let bridges: Vec<_> = crate::terrain::MAP_TIED_TYPES
            .iter()
            .filter(|n| is_bridge(n))
            .collect();
        assert_eq!(bridges.len(), 12);
        assert!(bridges.iter().all(|n| n.starts_with("BR")));
        assert!(
            crate::terrain::MAP_TIED_TYPES
                .iter()
                .filter(|n| !is_bridge(n))
                .all(|n| n.starts_with("ROAD"))
        );
    }

    #[test]
    fn connected_spans_share_support_in_both_directions() {
        let sources = sources();
        let height = |_: f64, z: f64| if z > 5000. { 1000. } else { z.max(0.) * 0.1 };
        let levels: Vec<_> = sources
            .layout
            .placements
            .iter()
            .map(|p| datum(&sources, p, height).unwrap())
            .collect();
        assert!((levels[0] - 68.).abs() < 1e-8);
        assert_eq!(levels[0], levels[1]);
        assert_eq!(levels[1], levels[2]);
        assert_eq!(levels[3], 1020.);
    }

    #[test]
    fn bridge_corridor_cuts_the_high_bank_and_shares_contact_triangles() {
        let sources = sources();
        let pads = foundations(&sources);
        let raw = |_: f32, z: f32| z.max(0.) * 0.1;
        let mesh = super::super::pads::Mesh::build_with_support(pads.clone(), 3, 3, raw, |p| {
            support(p, &pads, raw)
        });
        for z in [0., 190., 380.] {
            assert_eq!(mesh.height(0., z), Some(0.));
        }
        // Outside the blended skirt, the imported terrain is unchanged.
        assert!(
            mesh.height(2000., 380.)
                .is_none_or(|h| (h - 38.).abs() < 1e-4)
        );
        let p = super::super::pads::Pad {
            min: [2000., 2000.],
            max: [2100., 2100.],
            height: 0.,
        };
        assert_eq!(support(&p, &pads, raw), None);
    }

    #[test]
    fn graded_banks_fill_instead_of_cutting_and_leave_flat_bridges_unchanged() {
        let sources = sources();
        let raw = |_: f32, z: f32| z.clamp(0., 480.) * 0.1;
        let (flat, banks) = graded_foundations(&sources, raw, |_| true);
        assert_eq!(flat.len(), 1, "the separate flat crossing stays unchanged");
        assert_eq!(banks.len(), 3);
        let mesh = super::super::pads::Mesh::build_with_banks(
            flat.clone(),
            3,
            3,
            raw,
            |p| support(p, &flat, raw),
            &banks,
        );
        for z in [0., 190., 380.] {
            assert!((mesh.height(0., z).unwrap() - 48.).abs() < 0.001);
        }
        for z in (0..1200).step_by(8) {
            assert!(mesh.height(0., z as f32).unwrap() >= raw(0., z as f32) - 0.001);
        }
        let (flat, banks) = graded_foundations(&sources, |_, _| 0., |_| true);
        assert_eq!(flat.len(), foundations(&sources).len());
        assert!(banks.is_empty());
        let (flat, banks) = graded_foundations(&sources, raw, |p| p.min[1] > 300.);
        assert!(
            banks.is_empty(),
            "one protected bank keeps the entire old assembly foundation"
        );
        assert_eq!(flat.len(), foundations(&sources).len());
    }

    #[test]
    fn approaches_are_eight_percent_and_the_river_is_not_a_support_pad() {
        let core = super::super::pads::Pad {
            min: [2000., 2000.],
            max: [2400., 3000.],
            height: 352.,
        };
        let bank = Bank {
            core,
            bounds: super::super::pads::Pad {
                min: [500., 500.],
                max: [3900., 7500.],
                height: 352.,
            },
            grades: [[SIDE_GRADE; 2], [SIDE_GRADE, APPROACH_GRADE]],
        };
        let mesh =
            super::super::pads::Mesh::build_with_banks(vec![], 2, 2, |_, _| 0., |_| None, &[bank]);
        for z in (3000..7392).step_by(8) {
            let here = mesh.height(2200., z as f32).unwrap();
            let next = mesh.height(2200., z as f32 + 8.).unwrap();
            assert!((here - next).abs() / 8. <= APPROACH_GRADE as f32 + 0.00001);
        }
        assert_eq!(
            mesh.height(2200., 200.),
            Some(0.),
            "river beyond the bank remains open"
        );
        assert_eq!(mesh.height(2200., 7500.), Some(0.));
        for t in mesh.cells.values().flatten() {
            let x = t.iter().map(|p| p[0]).sum::<f32>() / 3.;
            let z = t.iter().map(|p| p[2]).sum::<f32>() / 3.;
            let y = t.iter().map(|p| p[1]).sum::<f32>() / 3.;
            assert!((mesh.height(x, z).unwrap() - y).abs() < 0.01);
        }
    }

    #[test]
    fn bridge_support_samples_a_ridge_inside_a_face() {
        let mut sources = sources();
        sources.layout.placements.truncate(1);
        let height = |_: f64, z: f64| (30. - z.abs()).max(0.);
        let level = datum(&sources, &sources.layout.placements[0], height).unwrap();
        assert_eq!(level, 50.);
    }
}
