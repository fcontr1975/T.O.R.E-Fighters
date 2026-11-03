//! Pylons and the stores still on them, drawn under every aircraft that
//! carries a combat loadout (B3 and BP2, docs/spec/stores-on-pylons.md).
//!
//! Retail draws no stores on aircraft; this is an opinionated addition John
//! asked for on 2026-10-12. What each station holds comes from the render
//! snapshot ([`StorePose`]); this module only turns it into vertices:
//!
//! - **Where.** A station's stores hang where the simulation says they
//!   do ([`hung`], [`tore_sim::combat::hardpoints::Rack::store_point`]):
//!   a paired station's split between its position and its mirror, a wing
//!   side with two stores (or four or more) on an inboard and an outboard
//!   pylon, a fuselage side's in tandem; a wing store's lugs 1 ft under the
//!   model's underside over its pylon, a fuselage or centreline store's
//!   2 inches under the body on a short rail, and a wingtip store's centre
//!   line on the tip's edge with nothing drawn under it (John 2026-10-13).
//!   A store on a pylon's shoulder rail (the F-14's glove pylon AIM-9)
//!   hangs beside that pylon on an arm ([`shoulder_rail`]).
//!   A released store leaves from that same point, so it never jumps.
//! - **Several on one pylon** hang off a small bar at the pylon's foot: two
//!   alongside it, one each side, as a wingtip missile sits on the tip
//!   (John 2026-10-13); three as a triple ejector rack (one each side, one
//!   under the bar), four as two pairs one behind another, more as triple
//!   racks one behind another. The slots keep the full load's layout, and
//!   the highest-numbered slot empties first.
//! - **Stores** use the weapon's own shape, refitted to the real store's size
//!   ([`crate::store_sizes`]). Retail has no rocket pod or fuel tank shape,
//!   so those, and a store with no shape, are fitted bodies.
//! - **Pylons** are fitted slabs from the top of the store's body (its lugs)
//!   up into the model's underside, each corner meeting the surface above
//!   it ([`Airframe::underside`]). A store on a wing station with nothing
//!   of the model above it hangs on a rail that joins the wing's edge beside
//!   it ([`Airframe::tip_edge`]).
//!   A pylon stays when its stores are gone.
//!
//! All geometry is solid colour in the aircraft vertex format, so it joins
//! the batch of the aircraft it hangs from with no extra draw call.
use crate::{aircraft::Airframe, render_snapshot::CombatArt};
use std::collections::HashMap;
use std::sync::Mutex;
use tore_formats::shape::Shape;
use tore_sim::attitude::{Basis, Vector};
use tore_sim::combat::hardpoints::{
    Location, PylonPlace, SHOULDER_OUT, rack_group, shoulder, shoulder_rise,
};
use tore_sim::combat::store_sizes::StoreSize;
use tore_world::snapshot::{StoreLook, StorePose};

/// Pylon and rack grey, linear RGB (fitted).
const PYLON: [f32; 3] = [0.5, 0.52, 0.54];
/// Tank and pod body colours (fitted).
const TANK: [f32; 3] = [0.55, 0.57, 0.6];
const POD: [f32; 3] = [0.4, 0.43, 0.4];
/// A store with no shape: length and diameter, feet (fitted).
const PLAIN: [f64; 2] = tore_sim::combat::store_sizes::PLAIN;
/// A pylon's height when nothing of the model is above or beside it (a
/// rail), feet.
const RAIL: f64 = 0.35;
/// A pylon's thickness, feet (the simulation spaces stores beside a rack's
/// bar clear of it).
const PYLON_WIDTH: f64 = 2. * tore_sim::combat::hardpoints::PYLON_HALF_WIDTH;
/// How far a pylon reaches into the surface it hangs from, feet, so no
/// sliver of sky shows at the joint. Retail wings are single faces, so a
/// deeper sink would show through on top.
const SINK: f64 = 0.02;
/// A rack's bar's depth at the foot of its pylon, feet.
const RACK_DEPTH: f64 = tore_sim::combat::hardpoints::RACK_DEPTH;
/// A short attachment plus the existing surface-following tolerance, feet.
const NEAR_SURFACE: f64 = tore_sim::combat::hardpoints::PYLON_DEPTH + 1.5;

/// The model surfaces stores meet, looked up once per point
/// ([`Airframe::underside`], [`Airframe::tip_edge`]).
#[derive(Debug, Default)]
pub struct MountCache {
    undersides: Mutex<HashMap<[i64; 4], Option<f64>>>,
    edges: Mutex<HashMap<[i64; 4], Option<[f64; 3]>>>,
}

impl MountCache {
    pub fn underside(&self, key: [i64; 4], find: impl FnOnce() -> Option<f64>) -> Option<f64> {
        if let Some(found) = self
            .undersides
            .lock()
            .ok()
            .and_then(|m| m.get(&key).copied())
        {
            return found;
        }
        let found = find();
        if let Ok(mut m) = self.undersides.lock() {
            m.insert(key, found);
        }
        found
    }
    pub fn tip_edge(
        &self,
        key: [i64; 4],
        find: impl FnOnce() -> Option<[f64; 3]>,
    ) -> Option<[f64; 3]> {
        if let Some(found) = self.edges.lock().ok().and_then(|m| m.get(&key).copied()) {
            return found;
        }
        let found = find();
        if let Ok(mut m) = self.edges.lock() {
            m.insert(key, found);
        }
        found
    }
}

/// A store's size, feet: length, largest cross-section width (fins
/// included, which spaces a rack), how far its body's top (where its lugs
/// are) is above its origin, how far ahead of its origin its middle is,
/// and the body's half width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dims {
    pub length: f64,
    pub diameter: f64,
    pub top: f64,
    pub middle: f64,
    pub radius: f64,
}

impl Dims {
    /// A fitted body centred on its origin.
    const fn body([length, diameter]: [f64; 2]) -> Self {
        Self {
            length,
            diameter,
            top: diameter / 2.,
            middle: 0.,
            radius: diameter / 2.,
        }
    }
    /// A weapon shape's own size, its lugs on the top of its body.
    pub fn of(shape: &Shape) -> Self {
        let Some(b) = crate::store_sizes::body(shape) else {
            return Self::body(PLAIN);
        };
        Self {
            length: b.length,
            diameter: (2. * b.reach[0].max(b.reach[1])).max(0.2),
            top: b.centre[1] + b.half[1],
            middle: (b.along[0] + b.along[1]) / 2.,
            radius: b.half[0],
        }
    }
}

/// Where each whole store of `store` hangs, feet right, up and forward of
/// the aircraft: its side (0 right, 1 left) and slot, the pylon it hangs
/// from, its place on that pylon's rack and how many the pylon carries, as
/// the station was loaded. A store still held is drawn there; a released one
/// left from there ([`Rack::store_point`], the point the release uses).
pub fn hung(store: &StorePose) -> Vec<Hung> {
    let size = store.size();
    let loaded = store.rack.sides(store.mount, store.loaded.max(store.items));
    let held = store.rack.sides(store.mount, store.items);
    let mut out = Vec::new();
    for side in 0..2 {
        if loaded[side] == 0 {
            continue;
        }
        for pylon in store
            .rack
            .side_pylons(store.mount, side, loaded[side], size)
        {
            let on = pylon.slots.len() as u16;
            for (k, slot) in pylon.slots.iter().enumerate() {
                let offset = store
                    .rack
                    .rack_offset(store.mount, side, k as u16, on, size);
                out.push(Hung {
                    side,
                    slot: *slot,
                    held: *slot < held[side],
                    centre: if store.rack.location() == Location::Magazine {
                        store.rack.store_point(
                            store.mount,
                            store.loaded.max(store.items),
                            side,
                            *slot,
                            size,
                        )
                    } else {
                        std::array::from_fn(|i| pylon.point[i] + offset[i])
                    },
                    pylon: pylon.clone(),
                });
            }
        }
    }
    out
}

/// One store's place on its station ([`hung`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Hung {
    pub side: usize,
    pub slot: u16,
    /// Still on the pylon.
    pub held: bool,
    pub centre: Vector,
    pub pylon: PylonPlace,
}

/// The vertices of `stores`, hung under an aircraft at `position` (relative
/// to the render origin) with `attitude` (yaw, pitch, bank), appended to
/// `out`. `airframe` gives the surfaces the pylons meet; `palette` the
/// shapes' colours.
pub fn vertices<'a>(
    out: &mut Vec<f32>,
    stores: impl IntoIterator<Item = &'a StorePose>,
    position: Vector,
    [yaw, pitch, bank]: [f64; 3],
    airframe: Option<&Airframe>,
    art: &CombatArt,
    palette: &[[u8; 3]; 256],
) {
    let start = out.len();
    let basis = Basis::new(yaw, pitch, bank);
    let at = |offset: Vector| -> Vector {
        std::array::from_fn(|i| {
            position[i]
                + basis.right[i] * offset[0]
                + basis.up[i] * offset[1]
                + basis.forward[i] * offset[2]
        })
    };
    for store in stores {
        let shape = store
            .shape
            .as_deref()
            .and_then(|name| art.shape(name))
            .filter(|_| store.look == StoreLook::Store);
        let dims = dims(store, shape);
        let size = store.size();
        let places = hung(store);
        // Each pylon once, with what it carries.
        let mut drawn: Vec<&PylonPlace> = Vec::new();
        for place in &places {
            // A store on a shoulder rail rides beside the pylon it shares.
            if store.rack.location() == Location::Shoulder {
                if !drawn.contains(&&place.pylon) {
                    drawn.push(&place.pylon);
                    shoulder_rail(out, &at, airframe, &place.pylon, &dims, size);
                }
                continue;
            }
            // A wingtip store sits on the tip itself: no pylon or rail.
            if store.rack.location().external()
                && store.rack.location() != Location::Wingtip
                && !drawn.contains(&&place.pylon)
            {
                drawn.push(&place.pylon);
                hanger(
                    out,
                    &at,
                    airframe,
                    &place.pylon,
                    &dims,
                    place.pylon.slots.len() as u16,
                    size,
                );
            }
        }
        for place in places.iter().filter(|p| p.held) {
            let centre = at(place.centre);
            match (store.look, shape) {
                (StoreLook::Store, Some(shape)) => mesh(out, shape, centre, &basis, palette),
                (look, _) => body(
                    out,
                    &basis,
                    centre,
                    [dims.length, dims.diameter],
                    match look {
                        StoreLook::Tank => TANK,
                        StoreLook::Pod => POD,
                        StoreLook::Store => PYLON,
                    },
                ),
            }
        }
    }
    palette_lit(&mut out[start..], palette);
}

/// Gives every vertex in `vertices` that carries its own colour (the fitted
/// pylons, racks, tanks and pods) the palette entry nearest that colour, so
/// it is lit exactly as the airframe's untextured faces are: the scene's
/// weather and time-of-day palette (night darkening, fog and haze), then
/// the same surface lighting (John 2026-10-13: stores were drawn bright at
/// night). A weapon shape's faces already carry their own palette indices.
fn palette_lit(vertices: &mut [f32], palette: &[[u8; 3]; 256]) {
    let mut found: Vec<([u32; 3], f32)> = Vec::new();
    for v in vertices.chunks_exact_mut(10) {
        if v[9] >= 0. {
            continue;
        }
        let key = [v[6], v[7], v[8]].map(f32::to_bits);
        let index = match found.iter().find(|(k, _)| *k == key) {
            Some((_, index)) => *index,
            None => {
                let index = nearest(palette, [v[6], v[7], v[8]]) as f32;
                found.push((key, index));
                index
            }
        };
        v[9] = index;
    }
}

/// The palette entry nearest `color` (0 to 1 per channel), leaving out the
/// first and last entries, which palettes keep for special uses.
fn nearest(palette: &[[u8; 3]; 256], color: [f32; 3]) -> usize {
    (1..255)
        .min_by_key(|i| {
            palette[*i]
                .iter()
                .zip(color)
                .map(|(p, c)| {
                    let d = i32::from(*p) - (c * 255.).round() as i32;
                    d * d
                })
                .sum::<i32>()
        })
        .unwrap_or(1)
}

/// The size `store` draws at.
fn dims(store: &StorePose, shape: Option<&Shape>) -> Dims {
    match (store.look, shape) {
        (StoreLook::Pod | StoreLook::Tank, _) if store.body[0] > 0. => Dims::body(store.body),
        (StoreLook::Pod, _) => Dims::body(tore_world::snapshot::POD_BODY),
        (StoreLook::Tank, _) => Dims::body(tore_world::snapshot::tank_body(2_000.)),
        (StoreLook::Store, Some(shape)) => Dims::of(shape),
        (StoreLook::Store, None) => Dims::body(PLAIN),
    }
}

/// Prefer a nearby wing/body over a distant surface directly overhead. The
/// station sampler starts at the source record, but drawing starts at the
/// raised store: that can bring a rotor into its 12 ft search and mask the
/// stub wing inboard. Keep the existing search when no short attachment fits.
fn hanger_surface(airframe: &Airframe, right: f64, forward: f64, from: f64) -> Option<f64> {
    let surface = airframe.underside(right, forward, from, 6);
    if surface.is_some_and(|h| h - from > NEAR_SURFACE) {
        for step in 1..=6 {
            let x = right.abs() - f64::from(step) * 0.5;
            if x < 0. {
                break;
            }
            if let Some(h) = airframe
                .underside(x, forward, from, 0)
                .filter(|h| h - from <= NEAR_SURFACE)
            {
                return Some(h);
            }
        }
    }
    surface
}

/// A mesh's middle can lie beyond the short wing even though its station
/// origin is under it. Move only the hanger toward that origin, up to 12 ft
/// in quarter-foot steps, until it reaches the same nearby surface.
fn hanger_attachment(
    airframe: &Airframe,
    right: f64,
    middle: f64,
    origin: f64,
    from: f64,
) -> (f64, Option<f64>) {
    let surface = hanger_surface(airframe, right, middle, from);
    if surface.is_some_and(|h| h - from > NEAR_SURFACE) {
        let distance = (origin - middle).abs();
        let steps = (distance.min(12.) / 0.25).ceil() as u32;
        for step in 1..=steps {
            let forward =
                middle + (origin - middle).signum() * (f64::from(step) * 0.25).min(distance);
            if let Some(h) =
                hanger_surface(airframe, right, forward, from).filter(|h| h - from <= NEAR_SURFACE)
            {
                return (forward, Some(h));
            }
        }
    }
    (middle, surface)
}

/// What holds a pylon's `on` stores of `dims` up: the pylon slab from the
/// stores' lugs (or a rack's bar) into the model's underside, the small bar
/// at its foot that two or more stores hang beside; or, with nothing above,
/// a rail joining a wingtip beside it, or a short rail.
fn hanger(
    out: &mut Vec<f32>,
    at: &impl Fn(Vector) -> Vector,
    airframe: Option<&Airframe>,
    pylon: &PylonPlace,
    dims: &Dims,
    on: u16,
    size: StoreSize,
) {
    let p = pylon.point;
    let lugs = p[1] + dims.top;
    let desired_middle = p[2] + dims.middle;
    let (middle, surface) = airframe.map_or((desired_middle, None), |a| {
        hanger_attachment(a, p[0], desired_middle, p[2], lugs)
    });
    // A pylon is longer than it is deep: at least twice its depth below
    // the surface over it, and no longer than most of the store.
    let depth = surface.map_or(0., |surface| surface - lugs);
    let length = (0.45 * dims.length)
        .max(2. * depth)
        .min(0.7 * dims.length.max(4.))
        .clamp(1.2, 8.);
    // A rack: a small bar across the pylon's foot, reaching out to the
    // centre lines of the stores beside it, 0.7 of the pylon's length (and
    // the racks one behind another); the stores' centre lines at its middle.
    let mut top_of_rack = lugs;
    if on >= 2 {
        let per = rack_group(on);
        let groups = on.div_ceil(per);
        let long = f64::from(groups - 1) * 1.05 * size.length + 0.7 * length;
        let centre = p[1] + dims.top - dims.radius.min(dims.top);
        block(
            out,
            at,
            [p[0], centre, middle],
            [2. * shoulder(size), RACK_DEPTH, long],
            PYLON,
        );
        top_of_rack = centre + RACK_DEPTH / 2.;
    }
    let Some(airframe) = airframe else {
        block(
            out,
            at,
            [p[0], top_of_rack + RAIL / 2., middle],
            [PYLON_WIDTH, RAIL, length],
            PYLON,
        );
        return;
    };
    match hanger_surface(airframe, p[0], middle, top_of_rack) {
        Some(surface) if surface - top_of_rack <= 0.05 => {}
        Some(surface) => {
            // The slab runs fore and aft only as far as the surface over it
            // goes (not out ahead of a wing's leading edge), and its top
            // follows that surface from end to end.
            let [back, front] = along_surface(airframe, p[0], middle, length, top_of_rack, surface);
            let half = PYLON_WIDTH / 2.;
            slab(
                out,
                at,
                [p[0] - half, p[0] + half],
                [back[0], front[0]],
                top_of_rack,
                [back[1], front[1], back[1], front[1]].map(|h| h + SINK),
            );
        }
        None => {
            let rail = (0.6 * dims.length).clamp(3., 9.);
            match airframe.tip_edge(p[0], p[1], middle, rail / 2.) {
                // A wingtip launcher: from inside the tip out over the
                // store, as deep as both the tip's edge and the store's
                // upper half.
                Some([edge, low, high]) if edge < p[0].abs() => {
                    let side = p[0].signum();
                    let inner = (edge - 0.3) * side;
                    let outer = p[0] + 0.1 * side;
                    let bottom = low.min(p[1] + dims.top - dims.radius).min(lugs - 0.15);
                    let top = high.max(lugs);
                    block(
                        out,
                        at,
                        [(inner + outer) / 2., (top + bottom) / 2., middle],
                        [(outer - inner).abs(), top - bottom, rail],
                        PYLON,
                    );
                }
                _ => block(
                    out,
                    at,
                    [p[0], top_of_rack + RAIL / 2., middle],
                    [PYLON_WIDTH, RAIL, length],
                    PYLON,
                ),
            }
        }
    }
}

/// What holds a store on a shoulder rail (the F-14's glove pylon
/// Sidewinder): the pylon it rides beside, [`SHOULDER_OUT`] inboard of its
/// centre line with its foot below the store's body by
/// [`tore_sim::combat::hardpoints::SHOULDER_GAP`] (the pylon
/// the station under it hangs from, drawn again so it stands when that
/// station is empty), and an arm from the pylon's side out along the
/// store's top, 0.6 of its length (3 to 6 ft), the rail it hangs from:
/// a jut off the pylon's side just above the store under the pylon (John
/// 2026-10-14).
fn shoulder_rail(
    out: &mut Vec<f32>,
    at: &impl Fn(Vector) -> Vector,
    airframe: Option<&Airframe>,
    pylon: &PylonPlace,
    dims: &Dims,
    size: StoreSize,
) {
    let p = pylon.point;
    if pylon.slots.len() == 2 {
        // F-15 twin shoulder rails share the central wing tank pylon.
        let foot = p[1] - shoulder_rise(size);
        hanger(
            out,
            at,
            airframe,
            &PylonPlace {
                point: [p[0], foot - dims.top, p[2]],
                slots: pylon.slots.clone(),
            },
            dims,
            1,
            size,
        );
        let spread = tore_sim::combat::hardpoints::TWIN_SHOULDER_OUT;
        block(
            out,
            at,
            [p[0], p[1] + dims.top + ARM / 2., p[2] + dims.middle],
            [2. * spread, ARM, (0.6 * dims.length).clamp(3., 6.)],
            PYLON,
        );
        return;
    }
    let side = if p[0] < 0. { -1. } else { 1. };
    let inboard = p[0] - side * SHOULDER_OUT;
    let foot = p[1] - shoulder_rise(size);
    hanger(
        out,
        at,
        airframe,
        &PylonPlace {
            point: [inboard, foot - dims.top, p[2]],
            slots: pylon.slots.clone(),
        },
        dims,
        1,
        size,
    );
    let inner = inboard + side * PYLON_WIDTH / 2.;
    let top = p[1] + dims.top;
    block(
        out,
        at,
        [(inner + p[0]) / 2., top + ARM / 2., p[2] + dims.middle],
        [(p[0] - inner).abs(), ARM, (0.6 * dims.length).clamp(3., 6.)],
        PYLON,
    );
}

/// A shoulder rail's arm's depth, feet (fitted).
const ARM: f64 = 0.15;

/// The back and front ends of a pylon `length` long centred at `middle`
/// (feet forward) under `right`, as [forward, surface height]: as far as
/// the model's underside over it reaches, at least 1.5 ft, and within
/// 1.5 ft of `surface`, the height over its middle. Where nothing of the
/// model is straight over the middle (the pylon reached inboard for it), it
/// keeps its length and meets `surface` flat.
fn along_surface(
    airframe: &Airframe,
    right: f64,
    middle: f64,
    length: f64,
    from: f64,
    surface: f64,
) -> [[f64; 2]; 2] {
    const STEPS: i32 = 6;
    let over = |forward: f64| {
        airframe
            .underside(right, forward, from, 0)
            .filter(|h| (h - surface).abs() < 1.5)
    };
    if over(middle).is_none() {
        return [
            [middle - length / 2., surface],
            [middle + length / 2., surface],
        ];
    }
    let step = length / 2. / f64::from(STEPS);
    let end = |sign: f64| -> [f64; 2] {
        let mut last = [middle, surface];
        for k in 1..=STEPS {
            let forward = middle + sign * step * f64::from(k);
            match over(forward) {
                Some(h) => last = [forward, h],
                None => break,
            }
        }
        // At least 0.75 ft either side of the middle.
        if (last[0] - middle).abs() < 0.75 {
            last[0] = middle + sign * 0.75;
        }
        last
    };
    [end(-1.), end(1.)]
}

/// A pylon slab between `x` and `z` (right and forward, feet, aircraft
/// frame), its flat bottom at `bottom` and each top corner at its own
/// height ([left back, left front, right back, right front]), both windings.
fn slab(
    out: &mut Vec<f32>,
    at: &impl Fn(Vector) -> Vector,
    x: [f64; 2],
    z: [f64; 2],
    bottom: f64,
    tops: [f64; 4],
) {
    let low = |i: usize, j: usize| at([x[i], bottom, z[j]]);
    let high = |i: usize, j: usize| at([x[i], tops[i * 2 + j], z[j]]);
    let quads = [
        [low(0, 0), low(0, 1), high(0, 1), high(0, 0)],
        [low(1, 0), high(1, 0), high(1, 1), low(1, 1)],
        [low(0, 1), low(1, 1), high(1, 1), high(0, 1)],
        [low(0, 0), high(0, 0), high(1, 0), low(1, 0)],
        [low(0, 0), low(1, 0), low(1, 1), low(0, 1)],
        [high(0, 0), high(0, 1), high(1, 1), high(1, 0)],
    ];
    for quad in quads {
        quad_both(out, quad, PYLON);
    }
}

/// The texture-only faces (the missile exhaust sheets) the flying store
/// omits too.
pub(crate) fn texture_only(face: &tore_formats::shape::Face) -> bool {
    matches!(face.subtype, 0x4c | 0x5c | 0x6c | 0x7c)
}

/// A weapon shape at `centre`, along the aircraft's axes: what
/// `render_snapshot` draws for the same store once it is released.
fn mesh(
    out: &mut Vec<f32>,
    shape: &Shape,
    centre: Vector,
    basis: &Basis,
    palette: &[[u8; 3]; 256],
) {
    for face in &shape.faces {
        if texture_only(face) {
            continue;
        }
        for i in 1..face.positions.len().saturating_sub(1) {
            for j in [0, i, i + 1] {
                let q = face.positions[j];
                let pos = std::array::from_fn(|k| {
                    centre[k]
                        + (basis.right[k] * f64::from(q[0])
                            + basis.up[k] * f64::from(q[2])
                            + basis.forward[k] * f64::from(q[1]))
                            / 3.
                });
                vertex(
                    out,
                    pos,
                    palette[face.colors[j] as usize].map(|c| f32::from(c) / 255.),
                );
                // The face's own palette entry and fog mode, as the
                // airframe's faces carry them, for the weather's lighting.
                let last = out.len() - 1;
                out[last] = f32::from(face.colors[j]) + 256. * f32::from(face.fog as u8);
            }
        }
    }
}

/// A box centred on `centre` (aircraft frame, feet) of `size` right, up and
/// forward, both windings so either cull mode shows it.
fn block(
    out: &mut Vec<f32>,
    at: &impl Fn(Vector) -> Vector,
    centre: Vector,
    size: Vector,
    color: [f32; 3],
) {
    let corner = |sx: f64, sy: f64, sz: f64| {
        at([
            centre[0] + sx * size[0] / 2.,
            centre[1] + sy * size[1] / 2.,
            centre[2] + sz * size[2] / 2.,
        ])
    };
    let faces: [[[f64; 3]; 4]; 6] = [
        [[1., -1., -1.], [1., 1., -1.], [1., 1., 1.], [1., -1., 1.]],
        [
            [-1., -1., 1.],
            [-1., 1., 1.],
            [-1., 1., -1.],
            [-1., -1., -1.],
        ],
        [[-1., 1., -1.], [-1., 1., 1.], [1., 1., 1.], [1., 1., -1.]],
        [
            [-1., -1., 1.],
            [-1., -1., -1.],
            [1., -1., -1.],
            [1., -1., 1.],
        ],
        [[-1., -1., 1.], [1., -1., 1.], [1., 1., 1.], [-1., 1., 1.]],
        [
            [1., -1., -1.],
            [-1., -1., -1.],
            [-1., 1., -1.],
            [1., 1., -1.],
        ],
    ];
    for quad in faces {
        let p = quad.map(|[x, y, z]| corner(x, y, z));
        quad_both(out, p, color);
    }
}

/// A fitted round body (tank or pod) centred on `centre`: an eight-sided
/// tube of `[length, diameter]` along the aircraft's forward axis, with
/// pointed ends a fifth of its length each.
pub(crate) fn body(
    out: &mut Vec<f32>,
    basis: &Basis,
    centre: Vector,
    [length, diameter]: [f64; 2],
    color: [f32; 3],
) {
    const SIDES: usize = 8;
    let radius = diameter / 2.;
    let ring = |k: usize, along: f64, scale: f64| -> Vector {
        let angle = k as f64 * std::f64::consts::TAU / SIDES as f64;
        let (s, c) = angle.sin_cos();
        std::array::from_fn(|i| {
            centre[i]
                + basis.right[i] * c * radius * scale
                + basis.up[i] * s * radius * scale
                + basis.forward[i] * along
        })
    };
    let half = length / 2.;
    let shoulder = half - length / 5.;
    let tip =
        |along: f64| -> Vector { std::array::from_fn(|i| centre[i] + basis.forward[i] * along) };
    for k in 0..SIDES {
        let n = (k + 1) % SIDES;
        quad_both(
            out,
            [
                ring(k, -shoulder, 1.),
                ring(n, -shoulder, 1.),
                ring(n, shoulder, 1.),
                ring(k, shoulder, 1.),
            ],
            color,
        );
        for (end, along) in [(half, shoulder), (-half, -shoulder)] {
            let t = [tip(end), ring(k, along, 1.), ring(n, along, 1.)];
            for p in [t, [t[0], t[2], t[1]]] {
                for q in p {
                    vertex(out, q, color);
                }
            }
        }
    }
}

fn quad_both(out: &mut Vec<f32>, p: [Vector; 4], color: [f32; 3]) {
    for tri in [
        [p[0], p[1], p[2]],
        [p[0], p[2], p[3]],
        [p[0], p[2], p[1]],
        [p[0], p[3], p[2]],
    ] {
        for q in tri {
            vertex(out, q, color);
        }
    }
}

/// One lit solid-colour vertex in the aircraft format, as a flying weapon's.
fn vertex(out: &mut Vec<f32>, pos: Vector, color: [f32; 3]) {
    out.extend([
        pos[0] as f32,
        pos[1] as f32,
        pos[2] as f32,
        0.,
        0.,
        -1.,
        color[0],
        color[1],
        color[2],
        -1.,
    ]);
}

/// Closed doors occlude bay stores, including through masked texels in the
/// imported skin. Counts and poses stay in snapshots and replay records.
pub fn visible(stores: &[StorePose], bay: f64) -> impl Iterator<Item = &StorePose> {
    stores
        .iter()
        .filter(move |store| bay > 0. || !store.rack.location().bay())
}

/// The stores of the flown aircraft, none once its wreck is gone.
pub fn for_flown(
    stores: &[StorePose],
    state: &crate::flight::State,
    airframe: &Airframe,
    art: &CombatArt,
    scenery: &crate::scenery::Scenery,
) -> Vec<f32> {
    let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
    let mut out = Vec::new();
    if !state.wreck_gone() {
        vertices(
            &mut out,
            visible(stores, state.bay),
            scenery.relative(state.position),
            [state.yaw, state.pitch, state.bank],
            Some(airframe),
            art,
            &airframe.palette,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_sim::combat::hardpoints::Rack;

    fn rack(location: u8) -> Rack {
        Rack {
            location,
            paired: true,
            ..Default::default()
        }
    }

    #[test]
    fn closed_bays_occlude_stores_without_losing_their_layout() {
        let art = CombatArt::synthetic(Default::default());
        let store = StorePose {
            hardpoint: 7,
            mount: [3.9, -1.05, 1.7],
            rack: rack(tore_sim::combat::hardpoints::MAGAZINE_BYTE),
            shape: None,
            look: StoreLook::Store,
            items: 4,
            loaded: 4,
            body: [0.; 2],
        };
        let stores = [store];
        let draw = |bay| {
            let mut out = Vec::new();
            vertices(
                &mut out,
                visible(&stores, bay),
                [0.; 3],
                [0.; 3],
                None,
                &art,
                &[[0; 3]; 256],
            );
            out
        };
        assert!(draw(0.).is_empty());
        assert!(!draw(0.5).is_empty());
        assert_eq!(draw(0.5), draw(1.));
        assert_eq!(stores[0].items, 4);
        assert_eq!(hung(&stores[0]).iter().filter(|s| s.held).count(), 4);
    }

    #[test]
    fn a_pair_draws_one_store_a_side_and_its_pylons_stay() {
        let art = CombatArt::synthetic(Default::default());
        let pair = StorePose {
            hardpoint: 6,
            mount: [10., -2., 0.],
            rack: rack(4),
            shape: None,
            look: StoreLook::Store,
            items: 2,
            loaded: 2,
            body: [0.; 2],
        };
        let draw = |items: u16| {
            let mut out = Vec::new();
            vertices(
                &mut out,
                &[StorePose {
                    items,
                    ..pair.clone()
                }],
                [0.; 3],
                [0.; 3],
                None,
                &art,
                &[[0; 3]; 256],
            );
            out
        };
        let full = draw(2);
        let one = draw(1);
        let none = draw(0);
        assert!(full.len() > one.len() && one.len() > none.len());
        // Both pylons remain with nothing on them, one a side.
        let xs: Vec<f32> = none.chunks_exact(10).map(|v| v[0]).collect();
        assert!(xs.iter().any(|x| *x > 9.) && xs.iter().any(|x| *x < -9.));
        // With one left, it hangs on the right: the left went first.
        let count =
            |v: &[f32], right: bool| v.chunks_exact(10).filter(|v| (v[0] > 0.) == right).count();
        assert!(count(&one, true) > count(&none, true));
        assert_eq!(count(&one, false), count(&none, false));
    }

    /// A synthetic airframe whose model is one flat wing face at the
    /// reference height, 0 to 15 ft out and 8 ft either side of the
    /// reference point fore and aft.
    fn flat_wing() -> Airframe {
        let wing = Shape {
            lines: Vec::new(),
            faces: vec![tore_formats::shape::Face {
                positions: vec![
                    [0., -24., 0.],
                    [45., -24., 0.],
                    [45., 24., 0.],
                    [0., 24., 0.],
                ],
                colors: vec![1; 4],
                fog: Default::default(),
                uv: Vec::new(),
                texture: String::new(),
                subtype: 0x20,
                normal: None,
                address: 0x1000,
            }],
            billboards: Vec::new(),
            state_words: Default::default(),
        };
        Airframe::synthetic(
            tore_formats::aircraft::AircraftId::Rafale,
            vec![wing],
            None,
            crate::combat_view::render_hash_tests::damage(),
            None,
        )
    }

    fn plain(mount: Vector, location: u8, items: u16) -> StorePose {
        StorePose {
            hardpoint: 0,
            mount,
            rack: rack(location),
            shape: None,
            look: StoreLook::Store,
            items,
            loaded: items,
            body: [0.; 2],
        }
    }

    /// The drawn grey (pylon and rack) vertices: [right, up, forward].
    fn grey(out: &[f32]) -> Vec<[f32; 3]> {
        out.chunks_exact(10)
            .filter(|v| v[6] == PYLON[0] && v[7] == PYLON[1] && v[8] == PYLON[2])
            .map(|v| [v[0], v[1], v[2]])
            .collect()
    }

    fn draw(stores: &[StorePose], airframe: &Airframe) -> Vec<f32> {
        let art = CombatArt::synthetic(Default::default());
        let mut out = Vec::new();
        // Shapeless stores draw grey too: draw them as tanks to tell apart.
        let stores: Vec<StorePose> = stores
            .iter()
            .map(|s| StorePose {
                look: StoreLook::Tank,
                body: PLAIN,
                ..s.clone()
            })
            .collect();
        vertices(
            &mut out,
            &stores,
            [0.; 3],
            [0.; 3],
            Some(airframe),
            &art,
            &[[0; 3]; 256],
        );
        out
    }

    #[test]
    fn a_pylon_runs_from_the_stores_lugs_into_the_wing_with_no_gap() {
        let airframe = flat_wing();
        let store = plain([8., -2., 0.], 4, 2);
        let out = draw(std::slice::from_ref(&store), &airframe);
        let lugs = -2. + PLAIN[1] / 2.;
        let ys: Vec<f32> = grey(&out).iter().map(|v| v[1]).collect();
        let low = ys.iter().copied().fold(f32::MAX, f32::min);
        let high = ys.iter().copied().fold(f32::MIN, f32::max);
        assert!(
            (f64::from(low) - lugs).abs() < 1e-4,
            "pylon bottom {low} vs lugs {lugs}"
        );
        assert!(
            f64::from(high) >= 0. && f64::from(high) <= SINK + 1e-4,
            "pylon top {high} meets the wing at 0"
        );
        // The store's top touches the pylon's bottom.
        let tank_top = out
            .chunks_exact(10)
            .filter(|v| v[6] == TANK[0])
            .map(|v| v[1])
            .fold(f32::MIN, f32::max);
        assert!((f64::from(tank_top) - lugs).abs() < 1e-4);
    }

    #[test]
    fn four_stores_on_a_wing_row_hang_two_a_wing_on_twin_rail_pylons() {
        let wing = rack(4);
        let mount = [8., -2., 0.];
        let size = StoreSize::body(PLAIN);
        let pylons =
            |rack: Rack, mount, side, per_side| rack.side_pylons(mount, side, per_side, size);
        // Two a wing: one pylon at the record with a twin-rail bar (John
        // 2026-10-14, the F/A-18's dual launchers).
        let right = pylons(wing, mount, 0, 2);
        assert_eq!(right.len(), 1);
        assert_eq!(right[0].slots, [0, 1]);
        assert!((right[0].point[0] - 8.).abs() < 1e-9);
        // Four a wing (beyond retail's maxima): an inboard and an outboard
        // pylon, each with two.
        let spread = tore_sim::combat::hardpoints::DUAL_PYLON_SPREAD;
        let four = pylons(wing, mount, 0, 4);
        assert_eq!(four.len(), 2);
        assert_eq!(
            [four[0].slots.clone(), four[1].slots.clone()],
            [vec![0, 2], vec![1, 3]]
        );
        assert!((four[0].point[0] - (8. - spread)).abs() < 1e-9, "inboard");
        assert!((four[1].point[0] - (8. + spread)).abs() < 1e-9, "outboard");
        let left = pylons(wing, mount, 1, 4);
        assert!((left[1].point[0] + 8. + spread).abs() < 1e-9, "mirrored");
        // The racks are mirrored too: both wings' outboard stores alike.
        let hung_at = |side| {
            hung(&plain(mount, 4, 4))
                .into_iter()
                .filter(|h| h.side == side && h.slot == 1)
                .map(|h| h.centre[0])
                .next()
                .unwrap()
        };
        assert!((hung_at(0) + hung_at(1)).abs() < 1e-9 && hung_at(0) > 8.);
        // One a side: one pylon. Three: one pylon's triple rack. Six: two
        // pylons with three each. A fuselage pair hangs one behind another.
        assert_eq!(pylons(wing, mount, 0, 1).len(), 1);
        assert_eq!(pylons(wing, mount, 0, 3).len(), 1);
        let six = pylons(wing, mount, 0, 6);
        assert_eq!(
            six.iter().map(|p| p.slots.len()).collect::<Vec<_>>(),
            [3, 3]
        );
        let tandem = pylons(rack(1), [3., -5., 0.], 0, 2);
        assert_eq!(tandem.len(), 2);
        assert!(tandem[0].point[2] < 0. && tandem[1].point[2] > 0.);
        // Drawn: one grey slab a wing over the twin rail, which stays as the
        // stores leave.
        let airframe = flat_wing();
        for items in [4, 1, 0] {
            let out = draw(
                &[StorePose {
                    loaded: 4,
                    ..plain(mount, 4, items)
                }],
                &airframe,
            );
            let xs = grey(&out);
            for x in [8., -8.] {
                assert!(
                    xs.iter().any(|v| (f64::from(v[0]) - x).abs() < PYLON_WIDTH),
                    "a pylon at {x} with {items} left"
                );
            }
        }
    }

    #[test]
    fn a_wingtip_store_sits_on_the_tips_edge_with_nothing_under_it() {
        let airframe = flat_wing();
        // A store recorded 1.2 ft out past the 15 ft tip, a little above it.
        let tip = on_flat_wing(plain([16.2, 0.2, 0.], 5, 2));
        let out = draw(std::slice::from_ref(&tip), &airframe);
        assert!(grey(&out).is_empty(), "no pylon or rail");
        let centres: Vec<Vector> = hung(&tip).iter().map(|h| h.centre).collect();
        assert_eq!(centres, [[15., 0., 0.], [-15., 0., 0.]]);
    }

    /// [`flat_wing`]'s stores as a loadout reads them: the wing's underside
    /// over each pylon (at the reference height).
    fn on_flat_wing(mut store: StorePose) -> StorePose {
        let model = tore_sim::combat::hardpoints::Underside::of(&flat_wing().poses[0], 1. / 3.);
        let h = tore_formats::aircraft::Hardpoint {
            location: store.rack.location,
            flags: 0x4,
            position: store.mount.map(|v| (v * 3.).round() as i32),
            store: None,
            count: i32::from(store.loaded),
            weight_class: 0,
        };
        let (_, placed) = tore_sim::combat::hardpoints::placed(&h, Some(&model));
        store.rack.over = placed.over;
        store.rack.tip = placed.tip;
        store
    }

    #[test]
    fn a_wing_pylon_is_a_foot_deep_and_a_fuselage_rail_two_inches() {
        let airframe = flat_wing();
        // Recorded 5 ft below the wing: the store moves up to hang 1 ft under
        // it on a wing, 2 inches under it at a fuselage station.
        for (location, loaded, depth) in [
            (4, 2, tore_sim::combat::hardpoints::PYLON_DEPTH),
            (1, 2, tore_sim::combat::hardpoints::RAIL_DEPTH),
        ] {
            let store = on_flat_wing(plain([9., -5., 0.], location, loaded));
            let out = draw(std::slice::from_ref(&store), &airframe);
            let tank_top = out
                .chunks_exact(10)
                .filter(|v| v[6] == TANK[0])
                .map(|v| v[1])
                .fold(f32::MIN, f32::max);
            assert!(
                (f64::from(tank_top) + depth).abs() < 1e-4,
                "store top {tank_top} with {loaded} loaded at {location}"
            );
            // The pylon fills the depth with no gap at either end.
            let ys: Vec<f32> = grey(&out).iter().map(|v| v[1]).collect();
            let low = ys.iter().copied().fold(f32::MAX, f32::min);
            let high = ys.iter().copied().fold(f32::MIN, f32::max);
            assert!((f64::from(low) - f64::from(tank_top)).abs() < 1e-4);
            assert!(f64::from(high) >= 0. && f64::from(high) <= SINK + 1e-4);
        }
    }

    #[test]
    fn a_rotor_above_the_station_does_not_hide_the_stub_wing_inboard() {
        let mut airframe = flat_wing();
        let mut rotor = airframe.poses[0].faces[0].clone();
        for p in &mut rotor.positions {
            p[2] = 19.;
        }
        // The stub wing ends half a foot inboard of the station. The rotor
        // spans both, 6.33 ft above it, as in the reported Apache geometry.
        for p in &mut airframe.poses[0].faces[0].positions {
            p[0] = p[0].min(28.5);
        }
        airframe.poses[0].faces.push(rotor);
        assert!(airframe.underside(10., 0., -1., 6).unwrap() > 6.);
        for count in [2, 4] {
            let mut store = plain([10., -20. / 3., 0.], 4, count);
            store.rack.over = [Some(0.); 5];
            let origins = hung(&store);
            for items in [count, 1, 0] {
                store.items = items;
                let vertices = draw(std::slice::from_ref(&store), &airframe);
                let pylon = grey(&vertices);
                assert!(!pylon.is_empty(), "empty launchers retain their pylons");
                let top = pylon.iter().map(|v| v[1]).reduce(f32::max).unwrap();
                assert!((f64::from(top) - SINK).abs() < 1e-5);
                assert!(pylon.iter().any(|v| v[0] < 0.));
                assert!(pylon.iter().any(|v| v[0] > 0.));
                for (before, after) in origins.iter().zip(hung(&store)) {
                    assert_eq!(before.centre, after.centre, "store origins stay fixed");
                }
            }
        }
        // A genuine high wing with no lower surface keeps its attachment.
        let mut high_wing = flat_wing();
        for p in &mut high_wing.poses[0].faces[0].positions {
            p[2] = 19.;
        }
        assert!(hanger_surface(&high_wing, 10., 0., -1.).unwrap() > 6.);
    }

    #[test]
    fn a_mesh_middle_aft_of_the_stub_wing_keeps_its_hanger_on_the_wing() {
        let mut airframe = flat_wing();
        let mut rotor = airframe.poses[0].faces[0].clone();
        for p in &mut rotor.positions {
            p[2] = 19.;
        }
        for p in &mut airframe.poses[0].faces[0].positions {
            p[0] = p[0].min(28.5);
            p[1] = p[1].max(0.);
        }
        airframe.poses[0].faces.push(rotor);
        let (middle, surface) = hanger_attachment(&airframe, 10., -0.75, 0., -1.);
        assert_eq!(middle, 0.);
        assert_eq!(surface, Some(0.));
        let mut vertices = Vec::new();
        hanger(
            &mut vertices,
            &|p| p,
            Some(&airframe),
            &PylonPlace {
                point: [10., -1.45, 0.],
                slots: vec![0],
            },
            &Dims {
                middle: -0.75,
                ..Dims::body(PLAIN)
            },
            1,
            StoreSize::body(PLAIN),
        );
        let top = grey(&vertices)
            .iter()
            .map(|v| v[1])
            .reduce(f32::max)
            .unwrap();
        assert!((f64::from(top) - SINK).abs() < 1e-5);
    }

    #[test]
    fn every_store_leaves_from_where_it_is_drawn() {
        use tore_sim::combat::hardpoints::units;
        let station = |location: u8, mount: Vector, count: u16, weapon_shape: &str| {
            let resources = tore_world::test_support::resources::resources();
            let mut weapon =
                tore_formats::weapons::Weapon::parse("AIM9M.JT", &resources["AIM9M.JT"]).unwrap();
            weapon.shape = Some(weapon_shape.into());
            weapon.burst.actual_rounds_per_game = 1;
            let store = on_flat_wing(plain(mount, location, count));
            tore_sim::combat::live::Station {
                weapon,
                mount,
                count,
                internal: false,
                rack: store.rack,
            }
        };
        for (location, mount, count, shape) in [
            (4, [9., -5., -1.], 4, "SPAR.SH"),
            (4, [6., -4., 0.], 6, "MK82.SH"),
            (4, [6., -4., 0.], 2, "MAVRK.SH"),
            (1, [3., -6., 0.], 4, "PHENX.SH"),
            (0, [0., -6., 0.], 4, "MK82.SH"),
            (5, [16.2, 0.2, 0.], 2, "SIDEM.SH"),
            (
                tore_sim::combat::hardpoints::SHOULDER_BYTE,
                [8.75, -5., 0.],
                2,
                "SIDEM.SH",
            ),
        ] {
            let s = station(location, mount, count, shape);
            let pose = StorePose {
                hardpoint: 0,
                mount: s.mount,
                rack: s.rack,
                shape: s.weapon.shape.clone(),
                look: StoreLook::Store,
                items: count,
                loaded: count,
                body: [0.; 2],
            };
            for rounds in (1..=u32::from(count)).rev() {
                // The store about to leave is drawn at its slot ...
                let before = StorePose {
                    items: rounds as u16,
                    ..pose.clone()
                };
                let after = StorePose {
                    items: rounds as u16 - 1,
                    ..pose.clone()
                };
                let gone: Vec<Vector> = hung(&before)
                    .into_iter()
                    .filter(|h| h.held)
                    .map(|h| h.centre)
                    .filter(|c| !hung(&after).iter().any(|h| h.held && h.centre == *c))
                    .collect();
                // ... and released from that very point.
                let release = s.release_point(units(rounds - 1, 1));
                assert_eq!(gone, [release], "{shape} on {location}, {rounds} left");
            }
        }
    }

    /// A store on a shoulder rail (the F-14's glove pylon Sidewinder)
    /// hangs beside the pylon it shares, its body just above that pylon's
    /// foot (the top of the store under the pylon),
    /// with the pylon drawn (it stands when the station under it is empty)
    /// and an arm out over the store; nothing is drawn straight above it.
    #[test]
    fn a_shoulder_rail_store_hangs_beside_its_pylon() {
        use tore_sim::combat::hardpoints::{PYLON_DEPTH, SHOULDER_BYTE};
        let airframe = flat_wing();
        let store = on_flat_wing(plain([8.75, -5., 0.], SHOULDER_BYTE, 2));
        assert_eq!(store.rack.location(), Location::Shoulder);
        let centres: Vec<Vector> = hung(&store).iter().map(|h| h.centre).collect();
        let up = -PYLON_DEPTH + shoulder_rise(store.size());
        assert_eq!(centres, [[8.75, up, 0.], [-8.75, up, 0.]]);
        let out = draw(std::slice::from_ref(&store), &airframe);
        let grey = grey(&out);
        let pylon = 8.75 - SHOULDER_OUT;
        // The pylon, from its foot into the wing.
        let slab: Vec<&[f32; 3]> = grey
            .iter()
            // Its inboard face: the arm juts off the outboard one.
            .filter(|v| (f64::from(v[0]) - (pylon - PYLON_WIDTH / 2.)).abs() < 1e-4)
            .collect();
        let low = slab.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
        let high = slab.iter().map(|v| v[1]).fold(f32::MIN, f32::max);
        assert!((f64::from(low) + PYLON_DEPTH).abs() < 1e-4, "foot {low}");
        assert!(f64::from(high) >= 0. && f64::from(high) <= SINK + 1e-4);
        // The arm reaches out to the store's centre line, above it.
        let reach = grey.iter().map(|v| v[0]).fold(f32::MIN, f32::max);
        assert!((f64::from(reach) - 8.75).abs() < 1e-4, "arm to {reach}");
        assert!(
            grey.iter()
                .filter(|v| f64::from(v[0]) > pylon + PYLON_WIDTH)
                .all(|v| f64::from(v[1]) > up),
            "the arm is above the store's centre line"
        );
    }

    /// Every pylon, rack, fitted body and weapon face is lit as the
    /// airframe's untextured faces are: a palette entry (with its fog mode)
    /// that the weather's time-of-day rows darken, never a fixed colour
    /// (John 2026-10-13: stores drawn bright at night).
    #[test]
    fn stores_and_pylons_take_the_airframes_palette_lighting() {
        let mut palette = [[0u8; 3]; 256];
        for (i, entry) in palette.iter_mut().enumerate() {
            *entry = [i as u8, i as u8, (255 - i) as u8];
        }
        let airframe = flat_wing();
        let art = CombatArt::synthetic(std::collections::BTreeMap::from([(
            "SPAR.SH".to_owned(),
            flat_wing().poses.swap_remove(0),
        )]));
        let stores = [
            on_flat_wing(plain([9., -5., 0.], 4, 4)),
            StorePose {
                look: StoreLook::Tank,
                body: [13., 2.],
                ..on_flat_wing(plain([3., -5., 0.], 1, 2))
            },
            StorePose {
                look: StoreLook::Pod,
                body: [5.5, 1.38],
                ..on_flat_wing(plain([12., -5., 0.], 4, 2))
            },
            StorePose {
                shape: Some("SPAR.SH".into()),
                ..on_flat_wing(plain([6., -5., 0.], 4, 2))
            },
        ];
        let mut out = Vec::new();
        vertices(
            &mut out,
            &stores,
            [0.; 3],
            [0.; 3],
            Some(&airframe),
            &art,
            &palette,
        );
        assert!(!out.is_empty());
        for v in out.chunks_exact(10) {
            assert_eq!(v[5], -1., "an untextured face, as the airframe's");
            assert!(v[9] >= 0., "a palette entry, not a fixed colour");
            let index = v[9] as usize % 256;
            assert!((1..255).contains(&index));
        }
        // The fitted grey takes the palette's nearest grey-ish entry.
        assert_eq!(nearest(&palette, PYLON), 126);
        // A weapon face keeps its own entry: the synthetic face's colour 1.
        let mut weapon = Vec::new();
        vertices(
            &mut weapon,
            &stores[3..],
            [0.; 3],
            [0.; 3],
            None,
            &art,
            &palette,
        );
        assert!(weapon.chunks_exact(10).any(|v| v[9] == 1.));
    }
}
