//! The integer picture of the ground the layout rules test against: the
//! terrain grid's class and elevation bytes, every theater layout object's
//! footprint and every runway's box. Positions are whole feet and boxes are
//! measured with the integer table in [`super::trig`], so every platform
//! answers every test alike (docs/spec/surface-defenses.md, "Jitter",
//! "Relocation").
use super::trig;
use std::collections::{BTreeMap, BTreeSet};
use tore_formats::theater::{TerrainCell, Theater};

/// Feet per terrain grid cell.
pub const CELL_FT: i64 = 8192;
/// Spacing of the samples of [`Ground::sees`], feet.
pub const SIGHT_STEP_FT: i64 = 250;
/// The terrain class byte of open water (docs/formats/theater.md).
pub const WATER_CLASS: u8 = 1;
/// The colour byte of the water cutout. A cell with it and no ground texture
/// over it is the sea to the eye whatever its class byte says: such a shore
/// cell of class 0 (every one in all sixteen theaters has elevation 0) is
/// drawn as the sea, so a land unit standing on it stands in the water.
pub const WATER_COLOR: u8 = 255;

/// The terrain grid, as the layout reads it.
pub trait Cells {
    fn cols(&self) -> usize;
    fn rows(&self) -> usize;
    /// The cell at `col`, `row`, both inside the grid.
    fn cell(&self, col: usize, row: usize) -> TerrainCell;
}

impl Cells for Theater {
    fn cols(&self) -> usize {
        self.cols
    }
    fn rows(&self) -> usize {
        self.rows
    }
    fn cell(&self, col: usize, row: usize) -> TerrainCell {
        Theater::cell(self, col, row)
    }
}

/// A type's horizontal extent about its placement origin, in whole feet:
/// `[right, forward]` in its own frame, from its shape's integer vertex
/// bounds times the placed scale (`terrain::placed_shape_scale`, the one
/// scale seam), floored and ceiled outward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    pub min: [i64; 2],
    pub max: [i64; 2],
}

impl Footprint {
    /// The distance from the origin to the farthest corner, rounded up: the
    /// circle the footprint stays inside at any heading.
    pub fn radius(&self) -> i64 {
        let x = self.min[0].abs().max(self.max[0].abs());
        let z = self.min[1].abs().max(self.max[1].abs());
        let squared = (x * x + z * z) as u128;
        let root = trig::isqrt(squared) as i64;
        if (root * root) as u128 == squared {
            root
        } else {
            root + 1
        }
    }
}

/// What the layout needs to know about one placed type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TypeInfo {
    pub footprint: Footprint,
    /// A runway or strip piece (`_STRIPProc`).
    pub strip: bool,
    /// A bridge or road piece (its display name says Bridge or Road).
    pub bridge_or_road: bool,
}

/// An object standing on the ground: its id, origin, heading in whole
/// degrees and footprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placed {
    pub id: u32,
    pub at: [i64; 2],
    pub heading: i32,
    pub footprint: Footprint,
}

impl Placed {
    /// Whether `point` lies closer than `clearance` feet to the footprint
    /// box (inside it counts as closer). Exact: the box test runs in the
    /// table's scaled units.
    pub fn within(&self, point: [i64; 2], clearance: i64) -> bool {
        let local =
            trig::to_local_scaled([point[0] - self.at[0], point[1] - self.at[1]], self.heading);
        let one = i128::from(trig::ONE);
        let outside = |value: i64, min: i64, max: i64| -> i128 {
            let v = i128::from(value);
            let (lo, hi) = (i128::from(min) * one, i128::from(max) * one);
            if v < lo {
                lo - v
            } else if v > hi {
                v - hi
            } else {
                0
            }
        };
        let dx = outside(local[0], self.footprint.min[0], self.footprint.max[0]);
        let dz = outside(local[1], self.footprint.min[1], self.footprint.max[1]);
        let limit = i128::from(clearance) * one;
        dx * dx + dz * dz < limit * limit
    }
    /// The radius of the circle about its origin the object stays inside.
    fn reach(&self) -> i64 {
        self.footprint.radius()
    }
}

/// The front of a theater: the centroids of its Blue-side and Red-side
/// layout placements, whole feet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Front {
    pub blue: [i64; 2],
    pub red: [i64; 2],
}

impl Front {
    /// The axis from Blue toward Red.
    pub fn axis(&self) -> [i64; 2] {
        [self.red[0] - self.blue[0], self.red[1] - self.blue[1]]
    }
    /// How far past the midpoint toward Red `point` lies along the axis,
    /// scaled by the axis length (so only its sign and ratios mean anything).
    pub fn depth(&self, point: [i64; 2]) -> i128 {
        let axis = self.axis();
        // Twice the offset from the midpoint, so the midpoint stays whole.
        let dx = 2 * i128::from(point[0]) - i128::from(self.blue[0] + self.red[0]);
        let dz = 2 * i128::from(point[1]) - i128::from(self.blue[1] + self.red[1]);
        dx * i128::from(axis[0]) + dz * i128::from(axis[1])
    }
    /// Whether `a` and `b` lie within `limit` feet of each other along the
    /// axis, and on the same side of the midpoint.
    pub fn same_band(&self, a: [i64; 2], b: [i64; 2], limit: i64) -> bool {
        let axis = self.axis();
        let along = i128::from(a[0] - b[0]) * i128::from(axis[0])
            + i128::from(a[1] - b[1]) * i128::from(axis[1]);
        let length2 = i128::from(axis[0]).pow(2) + i128::from(axis[1]).pow(2);
        let near = along * along <= i128::from(limit).pow(2) * length2;
        let (da, db) = (self.depth(a), self.depth(b));
        near && (da == 0 || db == 0 || (da > 0) == (db > 0))
    }
    /// Whether `point` lies on Blue's half of the front.
    pub fn blue_side(&self, point: [i64; 2]) -> bool {
        self.depth(point) <= 0
    }
}

/// The ground of one theater: grid, layout objects, runways and front.
pub struct Ground<'a> {
    pub cells: &'a dyn Cells,
    /// The theater layout's objects other than its runways.
    pub objects: Vec<Placed>,
    /// The theater layout's runways (strip pieces).
    pub runways: Vec<Placed>,
    pub front: Option<Front>,
    /// `objects` by grid cell, each listed in every cell its footprint
    /// reaches with [`Self::INDEXED_CLEARANCE`] added.
    index: BTreeMap<(i64, i64), Vec<usize>>,
    /// The ground texture placements by their grid cell (a multiple of four
    /// in each axis): a placement covers four cells a side.
    tiles: BTreeSet<(i64, i64)>,
}

impl<'a> Ground<'a> {
    /// The largest clearance the index answers; larger ones scan every
    /// object.
    const INDEXED_CLEARANCE: i64 = 2_000;

    pub fn new(
        cells: &'a dyn Cells,
        objects: Vec<Placed>,
        runways: Vec<Placed>,
        front: Option<Front>,
    ) -> Self {
        let mut index: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, object) in objects.iter().enumerate() {
            let reach = object.reach() + Self::INDEXED_CLEARANCE;
            let lo = [object.at[0] - reach, object.at[1] - reach].map(|v| v.div_euclid(CELL_FT));
            let hi = [object.at[0] + reach, object.at[1] + reach].map(|v| v.div_euclid(CELL_FT));
            for cx in lo[0]..=hi[0] {
                for cz in lo[1]..=hi[1] {
                    index.entry((cx, cz)).or_default().push(i);
                }
            }
        }
        Self {
            cells,
            objects,
            runways,
            front,
            index,
            tiles: BTreeSet::new(),
        }
    }

    /// The same ground with its ground texture placements, the grid cells
    /// (multiples of four) they stand on. A placed picture draws its own
    /// shore, so [`Self::water`] reads the class byte alone under one.
    pub fn with_tiles(mut self, tiles: impl IntoIterator<Item = (i64, i64)>) -> Self {
        self.tiles = tiles.into_iter().collect();
        self
    }

    /// The far corner of the grid, feet: terrain runs from 0 to
    /// `(cells - 1) * cell` on each axis.
    pub fn extent(&self) -> [i64; 2] {
        [
            (self.cells.cols() as i64 - 1) * CELL_FT,
            (self.cells.rows() as i64 - 1) * CELL_FT,
        ]
    }

    /// Whether `point` lies at least `margin` feet inside the grid.
    pub fn inside(&self, point: [i64; 2], margin: i64) -> bool {
        let extent = self.extent();
        (0..2).all(|axis| point[axis] >= margin && point[axis] <= extent[axis] - margin)
    }

    fn cell_index(&self, point: [i64; 2]) -> Option<(usize, usize)> {
        let extent = self.extent();
        if (0..2).any(|axis| point[axis] < 0 || point[axis] > extent[axis]) {
            return None;
        }
        Some(((point[0] / CELL_FT) as usize, (point[1] / CELL_FT) as usize))
    }

    /// Whether the cell under `point` is water: class byte [`WATER_CLASS`],
    /// or colour [`WATER_COLOR`] where no ground texture covers the cell (the
    /// sea as drawn). Outside the grid it is.
    pub fn water(&self, point: [i64; 2]) -> bool {
        self.cell_index(point).is_none_or(|(c, r)| {
            let cell = self.cells.cell(c, r);
            cell.class == WATER_CLASS
                || (cell.color == WATER_COLOR
                    && !self.tiles.contains(&((c & !3) as i64, (r & !3) as i64)))
        })
    }

    /// Whether the eight cells around the one under `point` are all water.
    pub fn open_sea(&self, point: [i64; 2]) -> bool {
        (-1..=1).all(|dc| {
            (-1..=1).all(|dr| self.water([point[0] + dc * CELL_FT, point[1] + dr * CELL_FT]))
        })
    }

    /// Whether the four integer corner elevations of the square under
    /// `point` span at most `units` elevation units (256 ft each).
    pub fn level(&self, point: [i64; 2], units: u8) -> bool {
        let Some((c, r)) = self.cell_index(point) else {
            return false;
        };
        let (cols, rows) = (self.cells.cols(), self.cells.rows());
        let mut lo = u8::MAX;
        let mut hi = u8::MIN;
        for (dc, dr) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let e = self
                .cells
                .cell((c + dc).min(cols - 1), (r + dr).min(rows - 1))
                .elevation;
            lo = lo.min(e);
            hi = hi.max(e);
        }
        hi - lo <= units
    }

    /// The terrain height at `point` in feet times [`CELL_FT`], exact: the
    /// grid's elevation bytes (256 ft each) interpolated over the two
    /// triangles of a cell as the terrain does (`Terrain::height`). A point
    /// off the grid reads the nearest cell.
    pub fn height_scaled(&self, point: [i64; 2]) -> i64 {
        let extent = self.extent();
        let at = |axis: usize, cells: usize| {
            let p = point[axis].clamp(0, extent[axis]);
            let c = (p / CELL_FT).min(cells as i64 - 2).max(0);
            (c as usize, p - c * CELL_FT)
        };
        let ((c, u), (r, v)) = (at(0, self.cells.cols()), at(1, self.cells.rows()));
        let h = |dc: usize, dr: usize| i64::from(self.cells.cell(c + dc, r + dr).elevation) * 256;
        if u + v <= CELL_FT {
            h(0, 0) * CELL_FT + (h(1, 0) - h(0, 0)) * u + (h(0, 1) - h(0, 0)) * v
        } else {
            h(1, 1) * CELL_FT
                + (h(0, 1) - h(1, 1)) * (CELL_FT - u)
                + (h(1, 0) - h(1, 1)) * (CELL_FT - v)
        }
    }

    /// Whether a unit at `a` sees one at `b` over the terrain: `eye_ft` above
    /// the ground at both ends, and no sample taken every [`SIGHT_STEP_FT`]
    /// along the line lies below the ground (the surface weapons' line of
    /// sight rule, on the same interpolated terrain, at a finer step).
    pub fn sees(&self, a: [i64; 2], b: [i64; 2], eye_ft: i64) -> bool {
        let (ha, hb) = (
            self.height_scaled(a) + eye_ft * CELL_FT,
            self.height_scaled(b) + eye_ft * CELL_FT,
        );
        let length = trig::isqrt(((a[0] - b[0]).pow(2) + (a[1] - b[1]).pow(2)) as u128) as i64;
        let n = (length / SIGHT_STEP_FT).clamp(2, 64);
        (1..n).all(|i| {
            let p = [a[0] + (b[0] - a[0]) * i / n, a[1] + (b[1] - a[1]) * i / n];
            self.height_scaled(p) <= (ha * (n - i) + hb * i) / n
        })
    }

    /// Whether any layout object (not a runway) lies within `clearance` feet
    /// of `point`.
    pub fn near_object(&self, point: [i64; 2], clearance: i64) -> bool {
        if clearance > Self::INDEXED_CLEARANCE {
            return self.objects.iter().any(|o| o.within(point, clearance));
        }
        let key = (point[0].div_euclid(CELL_FT), point[1].div_euclid(CELL_FT));
        self.index.get(&key).is_some_and(|list| {
            list.iter()
                .any(|&i| self.objects[i].within(point, clearance))
        })
    }

    /// Whether any theater runway lies within `clearance` feet of `point`.
    pub fn near_runway(&self, point: [i64; 2], clearance: i64) -> bool {
        self.runways.iter().any(|r| r.within(point, clearance))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_measures_distance_in_its_own_frame() {
        // 1,000 ft wide (east-west at heading 0), 6,000 ft long north.
        let runway = Placed {
            id: 1,
            at: [10_000, 10_000],
            heading: 0,
            footprint: Footprint {
                min: [-500, 0],
                max: [500, 6_000],
            },
        };
        assert!(runway.within([10_000, 13_000], 1));
        assert!(runway.within([10_799, 13_000], 300));
        assert!(!runway.within([10_800, 13_000], 300));
        // Turned to heading 90 the strip runs east.
        let east = Placed {
            heading: 90,
            ..runway
        };
        assert!(east.within([15_000, 10_000], 1));
        assert!(!east.within([10_000, 15_000], 300));
        assert_eq!(runway.footprint.radius(), 6_021);
    }

    #[test]
    fn the_front_tells_sides_and_depth_bands() {
        let front = Front {
            blue: [0, 0],
            red: [0, 100_000],
        };
        assert!(front.blue_side([50_000, 10_000]));
        assert!(!front.blue_side([0, 60_000]));
        assert!(front.same_band([0, 20_000], [90_000, 30_000], 15_000));
        assert!(!front.same_band([0, 20_000], [0, 40_000], 15_000));
        // Across the midpoint is never the same band.
        assert!(!front.same_band([0, 45_000], [0, 55_000], 15_000));
    }
}
