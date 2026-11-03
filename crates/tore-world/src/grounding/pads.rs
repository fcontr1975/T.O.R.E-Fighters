//! Fitted foundations for flat or broad static shapes. The finished
//! triangles are shared by terrain contact and drawing, including the skirt.
use std::collections::BTreeMap;
use tore_formats::{
    mission::Placement, shape::Shape, static_object::Definition, theater::CELL_FEET,
};

/// Agent-chosen transition width, feet. A foundation never ends in a cliff.
pub const SKIRT_FT: f64 = 512.;
/// World-axis conservative footprint, including two feet of edge clearance.
#[derive(Clone, Copy, Debug)]
pub struct Pad {
    pub min: [f64; 2],
    pub max: [f64; 2],
    pub height: f64,
}

impl Pad {
    /// Every flat static shape and every static footprint spanning 200 feet
    /// qualifies. A closed bottom face is not required: open-bottomed buildings,
    /// revetments and compound sites need the same support as concrete lots.
    /// Waterlines, map-tied bridges/roads and runways retain their own footing.
    pub fn of(definition: &Definition, shape: &Shape, scale: f64, p: &Placement) -> Option<Self> {
        if super::footing(definition) != super::Footing::Bottom
            || definition.resource_name.ends_with(".PT")
            || p.position[1] != 0
            || p.angles[1] != 0
            || p.angles[2] != 0
        {
            return None;
        }
        let [bottom, top] = super::height_range(shape)?;
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for point in super::points(shape) {
            for a in 0..2 {
                lo[a] = lo[a].min(f64::from(point[a]));
                hi[a] = hi[a].max(f64::from(point[a]));
            }
        }
        let size = [hi[0] - lo[0], hi[1] - lo[1]];
        if size.iter().any(|v| !v.is_finite() || *v <= 0.)
            || (top > bottom && size[0].max(size[1]) * scale < 200.)
        {
            return None;
        }
        let (sin, cos) = f64::from(p.angles[0]).to_radians().sin_cos();
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        for x in [lo[0], hi[0]] {
            for z in [lo[1], hi[1]] {
                let at = [
                    f64::from(p.position[0]) + scale * (x * cos + z * sin),
                    f64::from(p.position[2]) + scale * (-x * sin + z * cos),
                ];
                for a in 0..2 {
                    min[a] = min[a].min(at[a] - 2.);
                    max[a] = max[a].max(at[a] + 2.);
                }
            }
        }
        Some(Self {
            min,
            max,
            height: 0.,
        })
    }
    /// Whether the foundation or its skirt meets a level airfield rectangle.
    /// Separating-axis test avoids enlarging a rotated runway to a world box.
    pub fn meets_airfield(&self, field: &tore_sim::airport::OrientedBox) -> bool {
        if field.pitch != 0. || field.bank != 0. {
            return false;
        }
        let (sin, cos) = field.heading.sin_cos();
        let half = std::array::from_fn::<_, 2, _>(|a| (self.max[a] - self.min[a]) * 0.5 + SKIRT_FT);
        let d = [
            (self.min[0] + self.max[0]) * 0.5 - field.center[0],
            (self.min[1] + self.max[1]) * 0.5 - field.center[2],
        ];
        d[0].abs() <= half[0] + cos.abs() * field.half[0] + sin.abs() * field.half[2]
            && d[1].abs() <= half[1] + sin.abs() * field.half[0] + cos.abs() * field.half[2]
            && (d[0] * cos - d[1] * sin).abs()
                <= field.half[0] + cos.abs() * half[0] + sin.abs() * half[1]
            && (d[0] * sin + d[1] * cos).abs()
                <= field.half[2] + sin.abs() * half[0] + cos.abs() * half[1]
    }
    fn weight(&self, x: f64, z: f64) -> f64 {
        let d = (self.min[0] - x)
            .max(x - self.max[0])
            .max(self.min[1] - z)
            .max(z - self.max[1])
            .max(0.);
        (1. - d / SKIRT_FT).clamp(0., 1.)
    }
    fn overlaps(&self, other: &Self) -> bool {
        (0..2).all(|a| self.min[a] <= other.max[a] && self.max[a] >= other.min[a])
    }
}

pub type Triangle = [[f32; 3]; 3];
#[derive(Clone, Default)]
pub struct Mesh {
    pub pads: Vec<Pad>,
    pub cells: BTreeMap<(usize, usize), Vec<Triangle>>,
    /// Local 256 ft buckets keep contact proportional to nearby triangles.
    queries: BTreeMap<(usize, usize), Vec<Vec<usize>>>,
}

impl Mesh {
    /// Merge intersecting foundations before assigning heights, so nested lots,
    /// factories and revetments stand on one plane regardless of source order.
    pub fn build(pads: Vec<Pad>, cols: usize, rows: usize, raw: impl Fn(f32, f32) -> f32) -> Self {
        Self::build_with_support(pads, cols, rows, raw, |_| None)
    }

    /// Existing level airport support takes precedence over a new foundation's
    /// centre sample. Buildings and vehicles then agree with runway contact.
    pub fn build_with_support(
        pads: Vec<Pad>,
        cols: usize,
        rows: usize,
        raw: impl Fn(f32, f32) -> f32,
        support: impl Fn(&Pad) -> Option<f64>,
    ) -> Self {
        Self::build_with_banks(pads, cols, rows, raw, support, &[])
    }

    /// Bridge bank earthworks share the exact mesh used for terrain contact.
    pub fn build_with_banks(
        mut pads: Vec<Pad>,
        cols: usize,
        rows: usize,
        raw: impl Fn(f32, f32) -> f32,
        support: impl Fn(&Pad) -> Option<f64>,
        banks: &[super::bridges::Bank],
    ) -> Self {
        // Snap support boundaries to the same coordinates stored in the mesh.
        // Otherwise rounding a core edge into its skirt can tilt the plateau
        // by a tenth of a foot on steep terrain far from the world origin.
        for pad in &mut pads {
            pad.min = pad.min.map(|v| f64::from(v as f32));
            pad.max = pad.max.map(|v| f64::from(v as f32));
        }
        pads.sort_by(|a, b| {
            a.min[0]
                .total_cmp(&b.min[0])
                .then(a.min[1].total_cmp(&b.min[1]))
        });
        let mut i = 0;
        while i < pads.len() {
            let mut j = i + 1;
            while j < pads.len() {
                if pads[i].overlaps(&pads[j]) {
                    let b = pads.remove(j);
                    for a in 0..2 {
                        pads[i].min[a] = pads[i].min[a].min(b.min[a]);
                        pads[i].max[a] = pads[i].max[a].max(b.max[a]);
                    }
                    i = 0;
                    j = 1;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
        for pad in &mut pads {
            pad.height = support(pad).unwrap_or_else(|| {
                f64::from(raw(
                    ((pad.min[0] + pad.max[0]) * 0.5) as f32,
                    ((pad.min[1] + pad.max[1]) * 0.5) as f32,
                ))
            });
        }
        let cell = f64::from(CELL_FEET);
        let mut affected: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();
        for (id, pad) in pads
            .iter()
            .chain(banks.iter().map(|b| &b.bounds))
            .enumerate()
        {
            let low = pad
                .min
                .map(|v| ((v - SKIRT_FT) / cell).floor().max(0.) as usize);
            let high = pad
                .max
                .map(|v| ((v + SKIRT_FT) / cell).floor().max(0.) as usize);
            for z in low[1]..=high[1].min(rows - 2) {
                for x in low[0]..=high[0].min(cols - 2) {
                    affected.entry((x, z)).or_default().push(id);
                }
            }
        }
        let mut cells = BTreeMap::new();
        for ((x, z), ids) in affected {
            let x0 = x as f64 * cell;
            let z0 = z as f64 * cell;
            let mut polygons = vec![
                vec![[x0, z0], [x0, z0 + cell], [x0 + cell, z0]],
                vec![[x0 + cell, z0], [x0, z0 + cell], [x0 + cell, z0 + cell]],
            ];
            for axis in 0..2 {
                let mut cuts = Vec::new();
                for id in &ids {
                    let Some(p) = pads.get(*id) else {
                        let b = &banks[*id - pads.len()];
                        cuts.extend([
                            b.bounds.min[axis],
                            b.core.min[axis],
                            b.core.max[axis],
                            b.bounds.max[axis],
                        ]);
                        let lo = b.bounds.min[axis].max(if axis == 0 { x0 } else { z0 });
                        let hi =
                            b.bounds.max[axis].min(if axis == 0 { x0 + cell } else { z0 + cell });
                        let mut at = (lo / 64.).ceil() * 64.;
                        while at < hi {
                            cuts.push(at);
                            at += 64.;
                        }
                        continue;
                    };
                    cuts.extend([
                        p.min[axis] - SKIRT_FT,
                        p.min[axis],
                        p.max[axis],
                        p.max[axis] + SKIRT_FT,
                    ]);
                }
                cuts.sort_by(f64::total_cmp);
                cuts.dedup();
                for cut in cuts {
                    let mut split = Vec::new();
                    for polygon in polygons {
                        if polygon.iter().all(|p| p[axis] <= cut)
                            || polygon.iter().all(|p| p[axis] >= cut)
                        {
                            split.push(polygon);
                            continue;
                        }
                        for side in [-1., 1.] {
                            let p = clip(&polygon, axis, cut, side);
                            if p.len() >= 3 {
                                split.push(p);
                            }
                        }
                    }
                    polygons = split;
                }
            }
            let point = |p: [f64; 2]| {
                // Quantize coordinates once. Contact sees precisely the positions
                // uploaded to the GPU, including far-from-origin float rounding.
                let x = p[0] as f32;
                let z = p[1] as f32;
                let mut weight = 0.;
                let mut top = 0.;
                for id in &ids {
                    let Some(pad) = pads.get(*id) else {
                        continue;
                    };
                    let w = pad.weight(f64::from(x), f64::from(z));
                    if w > weight {
                        weight = w;
                        top = pad.height;
                    }
                }
                let mut h = f64::from(raw(x, z));
                for id in &ids {
                    if let Some(b) = id.checked_sub(pads.len()).and_then(|i| banks.get(i)) {
                        h = b.height(f64::from(x), f64::from(z), h);
                    }
                }
                [x, (h + (top - h) * weight) as f32, z]
            };
            let mut triangles = Vec::new();
            for polygon in polygons {
                for k in 1..polygon.len() - 1 {
                    let t = [point(polygon[0]), point(polygon[k]), point(polygon[k + 1])];
                    if determinant(t).abs() > 0.001 {
                        triangles.push(t);
                    }
                }
            }
            cells.insert((x, z), triangles);
        }
        let queries = cells
            .iter()
            .map(|(&(x, z), triangles)| {
                let mut buckets = vec![Vec::new(); 32 * 32];
                for (id, t) in triangles.iter().enumerate() {
                    let low = [0, 2].map(|a| t.iter().map(|p| p[a]).fold(f32::INFINITY, f32::min));
                    let high =
                        [0, 2].map(|a| t.iter().map(|p| p[a]).fold(f32::NEG_INFINITY, f32::max));
                    for bz in bucket(low[1], z)..=bucket(high[1], z) {
                        for bx in bucket(low[0], x)..=bucket(high[0], x) {
                            buckets[bz * 32 + bx].push(id);
                        }
                    }
                }
                ((x, z), buckets)
            })
            .collect();
        Self {
            pads,
            cells,
            queries,
        }
    }
    pub fn height(&self, x: f32, z: f32) -> Option<f32> {
        let key = (
            (x / CELL_FEET).floor().max(0.) as usize,
            (z / CELL_FEET).floor().max(0.) as usize,
        );
        let triangles = self.cells.get(&key)?;
        self.queries.get(&key)?[bucket(z, key.1) * 32 + bucket(x, key.0)]
            .iter()
            .find_map(|id| sample(triangles[*id], x, z))
    }
}
fn bucket(value: f32, cell: usize) -> usize {
    ((f64::from(value) - cell as f64 * f64::from(CELL_FEET)) / 256.)
        .floor()
        .clamp(0., 31.) as usize
}

fn clip(polygon: &[[f64; 2]], axis: usize, cut: f64, side: f64) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    let mut prev = *polygon.last().unwrap();
    let mut before = (prev[axis] - cut) * side;
    for p in polygon {
        let after = (p[axis] - cut) * side;
        if (before < 0.) != (after < 0.) {
            let t = before / (before - after);
            let mut v = std::array::from_fn(|a| prev[a] + (p[a] - prev[a]) * t);
            v[axis] = cut;
            out.push(v);
        }
        if after >= 0. {
            out.push(*p);
        }
        prev = *p;
        before = after;
    }
    out
}
fn determinant(t: Triangle) -> f64 {
    (f64::from(t[1][0]) - f64::from(t[0][0])) * (f64::from(t[2][2]) - f64::from(t[0][2]))
        - (f64::from(t[2][0]) - f64::from(t[0][0])) * (f64::from(t[1][2]) - f64::from(t[0][2]))
}
fn sample(t: Triangle, x: f32, z: f32) -> Option<f32> {
    if x < t.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min)
        || x > t.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max)
        || z < t.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min)
        || z > t.iter().map(|p| p[2]).fold(f32::NEG_INFINITY, f32::max)
    {
        return None;
    }
    let x = f64::from(x) - f64::from(t[0][0]);
    let z = f64::from(z) - f64::from(t[0][2]);
    let d = determinant(t);
    let u = (x * (f64::from(t[2][2]) - f64::from(t[0][2]))
        - z * (f64::from(t[2][0]) - f64::from(t[0][0])))
        / d;
    let v = (z * (f64::from(t[1][0]) - f64::from(t[0][0]))
        - x * (f64::from(t[1][2]) - f64::from(t[0][2])))
        / d;
    (u >= -1e-7 && v >= -1e-7 && u + v <= 1. + 1e-7).then(|| {
        (f64::from(t[0][1]) + u * f64::from(t[1][1] - t[0][1]) + v * f64::from(t[2][1] - t[0][1]))
            as f32
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pad(min: [f64; 2], max: [f64; 2]) -> Pad {
        Pad {
            min,
            max,
            height: 0.,
        }
    }
    #[test]
    fn slope_and_interior_ridge_are_flat_with_a_continuous_skirt() {
        // Cross the source grid's diagonal, including a ridge through the pad
        // interior: vertex-only object lifting cannot fix this case.
        let raw = |x: f32, z: f32| 100. + (x + z).min(16384. - x - z) * 0.05;
        let m = Mesh::build(vec![pad([3800., 3800.], [4400., 4400.])], 2, 2, raw);
        let top = m.pads[0].height as f32;
        for x in (3800..=4400).step_by(17) {
            for z in (3800..=4400).step_by(19) {
                assert!((m.height(x as f32, z as f32).unwrap() - top).abs() < 0.001);
            }
        }
        for x in [3288., 4912.] {
            assert!((m.height(x, 4100.).unwrap() - raw(x, 4100.)).abs() < 0.001);
        }
        assert!((m.height(3799.99, 4100.).unwrap() - top).abs() < 0.02);
        // A short transition on this ridge must not turn into a retaining
        // wall: the former 64 ft skirt exceeded this 1:4 local grade.
        for x in 3288..3800 {
            assert!(
                (m.height(x as f32 + 1., 4100.).unwrap() - m.height(x as f32, 4100.).unwrap())
                    .abs()
                    < 0.25
            );
        }
        for t in m.cells.values().flatten() {
            let x = (t[0][0] + t[1][0] + t[2][0]) / 3.;
            let z = (t[0][2] + t[1][2] + t[2][2]) / 3.;
            let drawn = (t[0][1] + t[1][1] + t[2][1]) / 3.;
            assert!(
                (m.height(x, z).unwrap() - drawn).abs() < 0.01,
                "contact must match the rendered triangle"
            );
        }
    }
    #[test]
    fn overlapping_pads_share_one_support_plane_in_either_order() {
        let pads = vec![
            pad([100., 100.], [500., 500.]),
            pad([400., 200.], [800., 600.]),
            pad([700., 500.], [900., 700.]),
        ];
        let raw = |x: f32, z: f32| x * 0.1 + z * 0.2;
        let a = Mesh::build(pads.clone(), 2, 2, raw);
        let b = Mesh::build(pads.into_iter().rev().collect(), 2, 2, raw);
        assert_eq!(a.pads.len(), 1);
        assert_eq!(a.cells, b.cells);
        for x in [100., 450., 800., 900.] {
            for z in [100., 500., 700.] {
                assert_eq!(a.height(x, z), Some(130.));
            }
        }
    }
    #[test]
    fn cross_cell_edges_and_far_coordinates_have_no_contact_cracks() {
        let raw = |x: f32, z: f32| (x + z) * 0.01;
        let c = f64::from(CELL_FEET) * 120.;
        let m = Mesh::build(
            vec![pad([c - 100.013, c - 100.013], [c + 300.013, c + 300.013])],
            123,
            123,
            raw,
        );
        for dx in [-164., -100., -0.0625, 0., 0.0625, 300., 364.] {
            let x = (c + dx) as f32;
            let z = c as f32;
            let h = m.height(x, z).unwrap();
            if (-100. ..=300.).contains(&dx) {
                assert!((h - m.pads[0].height as f32).abs() < 0.01);
            }
        }
        assert_eq!(m.height(1., 1.), None);
    }
    #[test]
    fn a_foundation_touching_a_rotated_airfield_inherits_its_plane() {
        use tore_sim::airport::{OrientedBox, Runway};
        let field = OrientedBox {
            center: [1800., 500., 1800.],
            half: [400., 1., 400.],
            heading: 0.7,
            pitch: 0.,
            bank: 0.,
        };
        let mut t = crate::test_support::terrain();
        let p = pad([1000., 1000.], [2000., 2000.]);
        assert!(p.meets_airfield(&field));
        assert!(!pad([4000., 4000.], [4500., 4500.]).meets_airfield(&field));
        t.foundations = Mesh::build_with_support(
            vec![p],
            2,
            2,
            |x, z| t.raw_height(x, z),
            |p| p.meets_airfield(&field).then_some(field.center[1]),
        );
        t.airport_scene.runways.push(Runway {
            object: 1,
            airport: 1,
            name: "Synthetic".into(),
            surface: field,
            approach_center: field.center,
            elevation_ft: 500.,
            heading: 0.7,
            length_ft: 800.,
        });
        for (x, z) in [(1100., 1100.), (1800., 1800.), (1999., 1999.)] {
            assert_eq!(t.height(x, z), 500.);
            assert_eq!(t.surface_height(f64::from(x), f64::from(z)), 500.);
        }
    }

    #[test]
    fn terrain_surface_and_ground_ceiling_use_foundations() {
        let mut t = crate::test_support::terrain();
        t.foundations = Mesh::build(vec![pad([1000., 1000.], [2000., 2000.])], 2, 2, |x, z| {
            t.raw_height(x, z)
        });
        let top = t.foundations.pads[0].height;
        for (x, z) in [(1000., 1000.), (1500., 1500.), (1999., 1999.)] {
            assert_eq!(f64::from(t.height(x as f32, z as f32)), top);
            assert_eq!(t.surface_height(x, z), top);
            assert_eq!(t.surface(x, z).height, top);
        }
        let ceiling = t.ground_ceiling();
        assert!(!ceiling.clear_above([1000., top, 1000.], [2000., top, 2000.]));
        assert!(ceiling.clear_above([1000., 4000., 1000.], [2000., 4000., 2000.]));
    }
}
