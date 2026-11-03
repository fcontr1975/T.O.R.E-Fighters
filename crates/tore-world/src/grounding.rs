//! Grounding: every object stands on the bottom of its shape
//! (docs/formats/objects-and-shapes.md, "Grounding").
//!
//! **The rule.** Every ground object, surface unit, man, wreck, ruin and
//! parked aircraft stands with the lowest point of its drawn shape (faces, lines and
//! sprites, at the scale it is drawn) on the ground: [`lift_ft`] is how far
//! its origin stands above the ground for that. Only three kinds keep their
//! authored rule ([`Footing`]): runways and pavement stand on their pavement
//! plane, roads on the map's height at their origin, and ships
//! on their waterline (their origin). Bridges share bottom support and a
//! graded terrain corridor ([`bridges`]). A presentation scale-up (the far-man
//! boost) scales from the feet, never from the origin ([`scaled_origin`]).
//!
//! Every path that places or draws something on the ground goes through this
//! module: the layout and ground target placements (`terrain::Placements::
//! stance_on_terrain`, which also sets the contact, collision and hit box), moving
//! units, wrecks, ruins, the men's sprites, parked aircraft. Never ground an
//! object any other way: before this module each path did its own grounding
//! (faces only, sprites skipped, men exempt, the boost about the centre) and
//! the same bug came back each time a new path or a new kind of shape
//! appeared. `tore-app --surface-dump --grounding` audits every type of the
//! import against it.
//!
//! Provenance: fitted (B13, 2026-10-12, extended by slice GROUND,
//! 2026-10-13): the lowest point agrees with the shape's F2 ground offset,
//! the word FA 0x42e0c0 reads to stand an object on the ground, within 3 ft
//! for every placed type of the retail data.
pub mod bridges;
pub mod pads;

use crate::terrain::PlacedSize;
use tore_formats::{
    shape::{Billboard, Shape},
    static_object::Definition,
    surface_unit::class,
};

/// What a placed type stands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Footing {
    /// The bottom of its shape: every building, theater object, land unit,
    /// man, wreck and ruin.
    Bottom,
    /// A runway or strip (`_STRIPProc`): its pavement plane
    /// (`terrain::pavement_height`).
    Pavement,
    /// A road ([`PlacedSize::MapTied`]): the map's height at its
    /// origin, so it meets the terrain it was drawn for.
    MapHeight,
    /// A bridge assembly: its complete shape bottom, with joined pieces on
    /// a shared terrain support datum. Destroyed geometry keeps that datum.
    Bridge,
    /// A ship: its origin is its waterline.
    Waterline,
}

/// What `definition`'s placements stand on.
pub fn footing(definition: &Definition) -> Footing {
    if definition.callbacks.iter().any(|name| name == "_STRIPProc") {
        Footing::Pavement
    } else if bridges::is_bridge(&definition.resource_name) {
        Footing::Bridge
    } else if PlacedSize::of(definition) == PlacedSize::MapTied {
        Footing::MapHeight
    } else if definition
        .resource_name
        .to_ascii_uppercase()
        .ends_with(".NT")
        && definition.category & class::SHIP != 0
    {
        Footing::Waterline
    } else {
        Footing::Bottom
    }
}

/// The lowest and highest point of a drawn shape along its up axis, in shape
/// units about its origin, or `None` for an empty shape. Faces and lines count every
/// vertex; a viewer-facing sprite spans half its height either side of its
/// centre, as `Billboard::face` draws it.
pub fn height_range(shape: &Shape) -> Option<[f64; 2]> {
    let mut range = [f64::INFINITY, f64::NEG_INFINITY];
    for point in points(shape) {
        let v = f64::from(point[2]);
        range = [range[0].min(v), range[1].max(v)];
    }
    range.iter().all(|v| v.is_finite()).then_some(range)
}

/// Complete drawn geometry used for footing. Thin lines can support an
/// antenna frame, but must not expand a solid collision volume. Components
/// retain their decoded offsets within the complete shape.
pub fn points(shape: &Shape) -> impl Iterator<Item = [f32; 3]> + Clone + '_ {
    shape
        .faces
        .iter()
        .flat_map(|f| f.positions.iter().copied())
        .chain(shape.lines.iter().flat_map(|l| l.positions))
        .chain(sprite_points(shape))
}

/// Vertices of solid contact geometry. Faces and sprites bound the fitted
/// body; zero-thickness line primitives (cables, wires, frames) do not make
/// the empty space around them solid. Grounding still uses all `points`.
pub fn solid_points(shape: &Shape) -> impl Iterator<Item = [f32; 3]> + Clone + '_ {
    shape
        .faces
        .iter()
        .flat_map(|f| f.positions.iter().copied())
        .chain(sprite_points(shape))
}

/// Solid placement bounds. A bridge's reviewed flat colour-6 shadow is a
/// decal on the river, not a wall filling the space beneath its deck.
/// Footing still uses every drawn point. Other object types keep their bounds.
pub fn placement_solid_points<'a>(
    definition: &Definition,
    shape: &'a Shape,
) -> impl Iterator<Item = [f32; 3]> + 'a {
    let bridge = footing(definition) == Footing::Bridge;
    let low = bottom(shape).unwrap_or(0.) as f32;
    shape
        .faces
        .iter()
        .filter(move |face| !(bridge && bridge_shadow(face, low)))
        .flat_map(|f| f.positions.iter().copied())
        .chain(sprite_points(shape))
}

/// Reviewed flat colour-6 bridge shadow, not structural support.
pub fn bridge_shadow(face: &tore_formats::shape::Face, low: f32) -> bool {
    face.subtype == 0x41
        && !face.colors.is_empty()
        && face.colors.iter().all(|c| *c == 6)
        && !face.positions.is_empty()
        && face
            .positions
            .iter()
            .all(|p| p[2] <= low + 1.5 && (p[2] - face.positions[0][2]).abs() < 0.5)
}

fn sprite_points(shape: &Shape) -> impl Iterator<Item = [f32; 3]> + Clone + '_ {
    shape.billboards.iter().flat_map(|sprite| {
        let [w, h] = sprite.size.map(|v| v * 0.5);
        let [x, y, z] = sprite.center;
        [[x - w, y - w, z - h], [x + w, y + w, z + h]]
    })
}

/// Vertical correction for a complete placed shape on uneven terrain. Keep
/// the authored orientation and internal mount offsets; translate the whole
/// assembly until its nearest geometry touches the terrain. The caller adds
/// any authored placement height after this correction. Fitted, agent choice.
/// This measures vertices (including line ends and sprite bounds), not the
/// interior of a face crossing a terrain ridge.
pub fn terrain_correction_ft(
    shape: &Shape,
    scale: f64,
    basis: tore_sim::attitude::Basis,
    origin: [f64; 3],
    height: impl Fn(f64, f64) -> f64,
) -> f64 {
    offsets(shape, scale)
        .map(|o| {
            let at = placed(o, &basis, origin);
            height(at[0], at[2]) - at[1]
        })
        .reduce(f64::max)
        .unwrap_or(0.)
}

/// Slope lift applies only to compact shapes: those reaching at most this
/// far (feet, across the ground) from their origin. Fitted, lead decision
/// 2026-10-14 from the nr-objg site audit: the compact objectives and
/// buildings reach at most 364 ft (HANGRB; the SA-2 and SA-3 site pads 312
/// and 351 ft), the composites from 457 ft (city blocks CTYBK*, crop fields,
/// town blocks, lot and reactor pads, CITY2 2037 ft).
pub const SLOPE_LIFT_MAX_REACH_FT: f64 = 400.;

/// Slope lift is refused when it would leave more than this many feet of air
/// under the shape's bottom on its downhill side. Fitted, lead decision
/// 2026-10-14: a rigid lift leaves a gap about twice the burial it removes;
/// the SA-2 site and Kuril factory fixes leave 27 to 28 ft.
pub const SLOPE_LIFT_MAX_GAP_FT: f64 = 30.;

/// Points within this many feet of a shape's bottom count as its underside
/// when [`slope_lift_ft`] measures the gap below it.
pub const BOTTOM_BAND_FT: f64 = 1.;

/// A shape's points in feet at `scale`: right, up and forward of its origin.
fn offsets(shape: &Shape, scale: f64) -> impl Iterator<Item = [f64; 3]> + Clone + '_ {
    points(shape).map(move |p| [p[0], p[2], p[1]].map(|v| scale * f64::from(v)))
}

/// How far a shape reaches from its origin across the ground (its right and
/// forward axes), in feet at `scale`.
pub fn reach_ft(shape: &Shape, scale: f64) -> f64 {
    reach_of(offsets(shape, scale))
}

fn reach_of(offsets: impl Iterator<Item = [f64; 3]>) -> f64 {
    offsets.map(|[r, _, f]| r.hypot(f)).fold(0., f64::max)
}

/// Where `offset` (feet right, up, forward) lands from `origin` turned by `basis`.
fn placed(offset: [f64; 3], basis: &tore_sim::attitude::Basis, origin: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| {
        origin[i]
            + basis.right[i] * offset[0]
            + basis.up[i] * offset[1]
            + basis.forward[i] * offset[2]
    })
}

/// The largest air gap under the bottom of a shape placed at `origin`: the
/// highest clearance above the terrain among its underside points (within
/// [`BOTTOM_BAND_FT`] of its lowest point), 0 when none is above it.
pub fn bottom_gap_ft(
    shape: &Shape,
    scale: f64,
    basis: tore_sim::attitude::Basis,
    origin: [f64; 3],
    height: impl Fn(f64, f64) -> f64,
) -> f64 {
    gap_of(offsets(shape, scale), &basis, origin, &height)
}

fn gap_of(
    offsets: impl Iterator<Item = [f64; 3]> + Clone,
    basis: &tore_sim::attitude::Basis,
    origin: [f64; 3],
    height: &impl Fn(f64, f64) -> f64,
) -> f64 {
    let low = offsets.clone().map(|o| o[1]).fold(f64::INFINITY, f64::min);
    offsets
        .filter(|o| o[1] - low <= BOTTOM_BAND_FT)
        .map(|o| {
            let at = placed(o, basis, origin);
            at[1] - height(at[0], at[2])
        })
        .fold(0., f64::max)
}

/// The slope lift of a shape stood by the origin rule at `origin` (its
/// bottom on the ground under its origin, no authored height): how far to
/// raise (or lower) the whole rigid shape so its nearest point meets the
/// terrain, or `None` when the origin rule stands. It stands for a flat
/// shape (ground art: fields, lots, pads), one reaching beyond
/// [`SLOPE_LIFT_MAX_REACH_FT`], and where the lift would leave more than
/// [`SLOPE_LIFT_MAX_GAP_FT`] of air under the downhill side.
pub fn slope_lift_ft(
    shape: &Shape,
    scale: f64,
    basis: tore_sim::attitude::Basis,
    origin: [f64; 3],
    height: impl Fn(f64, f64) -> f64,
) -> Option<f64> {
    slope_lift_of(offsets(shape, scale), &basis, origin, &height)
}

/// [`slope_lift_ft`] for a unit that drives: `footing` is its shape as
/// [`Footing::of`] keeps it, `position` its pose (its bottom on the ground
/// under it), `basis` its attitude on the slope.
pub fn unit_slope_lift_ft(
    footing: &UnitFooting,
    basis: tore_sim::attitude::Basis,
    position: [f64; 3],
    height: impl Fn(f64, f64) -> f64,
) -> f64 {
    if footing.points.is_empty() {
        return 0.;
    }
    slope_lift_of(footing.points.iter().copied(), &basis, position, &height).unwrap_or(0.)
}

/// The one slope rule, over a shape's points in feet (right, up, forward)
/// from `origin`.
fn slope_lift_of(
    offsets: impl Iterator<Item = [f64; 3]> + Clone,
    basis: &tore_sim::attitude::Basis,
    origin: [f64; 3],
    height: &impl Fn(f64, f64) -> f64,
) -> Option<f64> {
    let (low, high) = offsets
        .clone()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), o| {
            (l.min(o[1]), h.max(o[1]))
        });
    if high - low <= f64::EPSILON || reach_of(offsets.clone()) > SLOPE_LIFT_MAX_REACH_FT {
        return None;
    }
    let lift = offsets
        .clone()
        .map(|o| {
            let at = placed(o, basis, origin);
            height(at[0], at[2]) - at[1]
        })
        .fold(f64::NEG_INFINITY, f64::max);
    let lifted = [origin[0], origin[1] + lift, origin[2]];
    (gap_of(offsets, basis, lifted, height) <= SLOPE_LIFT_MAX_GAP_FT).then_some(lift)
}

/// A driving unit's shape for the slope rule: its unique points in feet
/// (right, up, forward) from its pose, which stands its lowest point on the
/// ground. Kept on its course so each step measures the same points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitFooting {
    pub points: Vec<[f64; 3]>,
}

impl UnitFooting {
    /// The footing of `shape` drawn at `scale`, standing on its bottom.
    pub fn of(shape: &Shape, scale: f64) -> Self {
        let lift = lift_ft(shape, scale);
        let mut points: Vec<[f64; 3]> = offsets(shape, scale)
            .map(|[r, u, f]| [r, u + lift, f])
            .collect();
        points.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        points.dedup();
        Self { points }
    }
}

/// The lowest point of viewer-facing sprites, in shape units about their
/// origin (half each sprite's height below its centre), or `None` for none.
pub fn sprites_bottom(sprites: &[Billboard]) -> Option<f64> {
    sprites
        .iter()
        .map(|sprite| f64::from(sprite.center[2]) - f64::from(sprite.size[1]) * 0.5)
        .reduce(f64::min)
}

/// The bottom of a drawn shape: its lowest point (faces, lines and sprites) in shape
/// units about its origin, or `None` for an empty shape.
pub fn bottom(shape: &Shape) -> Option<f64> {
    height_range(shape).map(|[low, _]| low)
}

/// How far, in world feet, the origin of `shape` drawn at `scale` stands
/// above the ground so its bottom meets it: minus its lowest point at that
/// scale. Negative for a shape wholly above its origin, which is lowered
/// onto the ground the same way; 0 for an empty shape.
pub fn lift_ft(shape: &Shape, scale: f64) -> f64 {
    bottom(shape).map_or(0., |low| -low * scale)
}

/// How far the origin of a `definition` placement drawn as `shape` at
/// `scale` stands above its placement point: [`lift_ft`] for a type that
/// stands on its [bottom](Footing::Bottom), including bridge footing; 0 for a ship. A
/// runway's pavement rule is `terrain::Placements::stance`'s.
pub fn placement_lift_ft(definition: &Definition, shape: &Shape, scale: f64) -> f64 {
    match footing(definition) {
        Footing::Bottom | Footing::Bridge => lift_ft(shape, scale),
        Footing::Pavement | Footing::MapHeight | Footing::Waterline => 0.,
    }
}

/// The origin of a shape drawn `factor` times larger than at `origin`,
/// keeping its feet where they are: `bottom` is its lowest point in shape
/// units at `scale` feet per unit, `up` its up axis. A presentation scale-up
/// (the far-man boost) grows upward from the ground, never into it.
pub fn scaled_origin(
    origin: [f64; 3],
    up: [f64; 3],
    bottom: f64,
    scale: f64,
    factor: f64,
) -> [f64; 3] {
    // The feet stay put: origin + up * bottom * scale before and after.
    std::array::from_fn(|i| origin[i] + up[i] * bottom * scale * (1. - factor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_formats::shape::Face;

    /// A shape of one triangle spanning `low` to `high` along its up axis.
    fn upright(low: f32, high: f32) -> Shape {
        Shape {
            lines: Vec::new(),
            faces: vec![Face {
                positions: vec![[0., 0., low], [10., 0., low], [0., 0., high]],
                colors: vec![0],
                fog: Default::default(),
                uv: Vec::new(),
                texture: String::new(),
                subtype: 0,
                normal: None,
                address: 0,
            }],
            billboards: Vec::new(),
            state_words: Default::default(),
        }
    }

    /// A man: one sprite `height` tall centred `centre` above the origin.
    fn man(centre: f32, height: f32) -> Shape {
        Shape {
            lines: Vec::new(),
            faces: Vec::new(),
            billboards: vec![Billboard {
                center: [0., 0., centre],
                size: [height * 0.4, height],
                texture: "MAN.PIC".into(),
                uv: None,
                fog: Default::default(),
                address: 0,
            }],
            state_words: Default::default(),
        }
    }

    fn definition(name: &str, callback: &str, category: u16) -> Definition {
        Definition {
            display_name: String::new(),
            class_name: String::new(),
            resource_name: name.into(),
            main_shape: None,
            callbacks: vec![callback.into()],
            hit_points: None,
            category,
            radar_signature: 0,
            infrared_signature: 0,
            explosion: 0,
            crater: 0,
            flags: 0,
            max_visible: 0,
        }
    }

    #[test]
    fn foundation_selection_uses_geometry_scale_and_grounding_exceptions() {
        let mut shape = upright(0., 0.);
        shape.faces[0].positions = vec![
            [-300., -150., 0.],
            [300., -150., 0.],
            [300., 150., 0.],
            [-300., 150., 0.],
        ];
        let layout = tore_formats::mission::Layout::parse(
            "SYNTH.MM",
            b"textFormat\nobj\n\ttype BASE.OT\n\tpos 1000 0 2000\n\tangle 45 0 0\n\t.\n",
        )
        .unwrap();
        let mut p = layout.placements[0].clone();
        let def = definition("BASE.OT", "_OBJProc", 0);
        let pad = pads::Pad::of(&def, &shape, 1. / 3., &p).unwrap();
        assert!((pad.max[0] - pad.min[0] - (300f64 / 2f64.sqrt() + 4.)).abs() < 1e-8);
        assert!(
            pads::Pad::of(&def, &shape, 0.1, &p).is_some(),
            "small flat pads also need support"
        );
        for fixed in [
            definition("BASE.PT", "_APProc", 0),
            definition("STRIP.OT", "_STRIPProc", 0),
            definition("BRDMID.OT", "_OBJProc", 0),
        ] {
            assert!(pads::Pad::of(&fixed, &shape, 1., &p).is_none());
        }
        p.position[1] = 10;
        assert!(pads::Pad::of(&def, &shape, 1., &p).is_none());
        p.position[1] = 0;
        p.angles[1] = 5;
        assert!(pads::Pad::of(&def, &shape, 1., &p).is_none());
        p.angles[1] = 0;
        shape.faces[0].positions[0][2] = -20.;
        assert!(
            pads::Pad::of(&def, &shape, 1., &p).is_some(),
            "broad open-bottomed structures need support too"
        );
        assert!(pads::Pad::of(&def, &shape, 0.1, &p).is_none());
    }

    #[test]
    fn bridge_shadow_grounds_the_shape_without_blocking_below_the_deck() {
        let mut span = upright(10., 10.);
        let mut shadow = upright(-60., -60.).faces.remove(0);
        shadow.subtype = 0x41;
        shadow.colors = vec![6];
        span.faces.push(shadow);
        let bridge = definition("BRDMID.OT", "_OBJProc", 0);
        let points: Vec<_> = placement_solid_points(&bridge, &span).collect();
        assert!(points.iter().all(|p| p[2] == 10.));
        assert_eq!(bottom(&span), Some(-60.));
        // No change to other objects' existing contact policy.
        let building = definition("TEST.OT", "_OBJProc", 0);
        assert!(placement_solid_points(&building, &span).any(|p| p[2] == -60.));
        // A vertical colour-6 face is still solid geometry.
        span.faces[1].positions[0][2] = 10.;
        assert!(placement_solid_points(&bridge, &span).any(|p| p[2] == -60.));
        span.faces[1].positions[0][2] = -60.;
        let mut placements = crate::terrain::Placements::of_types(
            &std::collections::BTreeMap::new(),
            std::iter::empty(),
        );
        placements.definitions.insert("BRDMID.OT".into(), bridge);
        placements.shapes.insert("BRDMID.OT".into(), span);
        let layout = tore_formats::mission::Layout::parse(
            "TEST.MM",
            b"textFormat\nobj\n type BRDMID.OT\n pos 0 0 0\n angle 0 0 0\n .\n",
        )
        .unwrap();
        let stance = placements
            .stance_on_terrain(&layout.placements[0], |_, _| 0.)
            .unwrap();
        let body = tore_sim::airport::OrientedBox {
            center: stance.center,
            half: stance.half,
            heading: stance.heading,
            pitch: stance.pitch,
            bank: stance.bank,
        };
        assert_eq!(body.center[1], 70.);
        assert!(
            body.segment_fraction([-20., 35., 0.], [20., 35., 0.])
                .is_none()
        );
        assert!(
            body.segment_fraction([-20., 70., 0.], [20., 70., 0.])
                .is_some()
        );
    }

    #[test]
    fn every_ground_object_stands_on_the_bottom_of_its_shape() {
        // A factory hall whose origin is 89 percent of the way up (B13).
        let hall = upright(-128., 16.);
        assert_eq!(height_range(&hall), Some([-128., 16.]));
        let factory = definition("FACTD.OT", "_OBJProc", 0);
        assert_eq!(footing(&factory), Footing::Bottom);
        assert!((placement_lift_ft(&factory, &hall, 1. / 3.) - 128. / 3.).abs() < 1e-9);
        // A shape wholly above its origin comes down onto the ground.
        assert_eq!(lift_ft(&upright(4., 20.), 1.), -4.);
        // A man centred on his origin (the airfield squad TROOPS is too)
        // stands half his height up; a SOLDIER's feet are already on his
        // origin.
        let troops = definition("TROOPS.NT", "_NTProc", class::OTHER);
        assert_eq!(footing(&troops), Footing::Bottom);
        assert!((placement_lift_ft(&troops, &man(0., 20.), 1. / 3.) - 10. / 3.).abs() < 1e-12);
        assert_eq!(lift_ft(&man(6., 12.), 1.), 0.);
        assert_eq!(sprites_bottom(&man(0., 20.).billboards), Some(-10.));
        assert_eq!(sprites_bottom(&[]), None);
        // Land units, launchers and bunkers stand on their bottom too.
        for category in [class::TANK, class::SAM, class::STRUCTURE, class::VEHICLE] {
            assert_eq!(
                footing(&definition("X.NT", "_NTProc", category)),
                Footing::Bottom
            );
        }
        // Bridges stand on their bottom; runways, roads and ships retain their datums.
        let strip = definition("STRIP.OT", "_STRIPProc", 0);
        assert_eq!(footing(&strip), Footing::Pavement);
        let bridge = definition("BRDMID.OT", "_OBJProc", 0);
        assert_eq!(footing(&bridge), Footing::Bridge);
        assert_eq!(placement_lift_ft(&bridge, &hall, 1.), 128.);
        let ship = definition("KIROV.NT", "_NTProc", class::SHIP);
        assert_eq!(footing(&ship), Footing::Waterline);
        for fixed in [&strip, &ship] {
            assert_eq!(placement_lift_ft(fixed, &hall, 1.), 0.);
        }
        let empty = Shape {
            faces: Vec::new(),
            ..upright(0., 1.)
        };
        assert_eq!(lift_ft(&empty, 1.), 0.);
    }

    #[test]
    fn antenna_frame_and_complete_assembly_define_the_footing() {
        let mut shape = upright(-13., 20.);
        shape.lines.push(tore_formats::shape::Line {
            positions: [[0., 0., -14.], [0., 0., 18.]],
            color: 1,
            fog: Default::default(),
        });
        assert_eq!(bottom(&shape), Some(-14.));
        assert!((lift_ft(&shape, 4. / 3.) - 56. / 3.).abs() < 1e-12);
        // The whole dish and frame move together, not separately onto the ground.
        let basis = tore_sim::attitude::Basis::new(0., 0., 0.);
        assert_eq!(
            terrain_correction_ft(&shape, 1., basis, [0., 13., 0.], |_, _| 0.),
            1.
        );
    }

    #[test]
    fn footing_samples_the_slope_under_the_geometry_not_only_the_origin() {
        let shape = upright(0., 20.);
        let basis = tore_sim::attitude::Basis::new(0., 0., 0.);
        // One foot is ten feet east, where the ground rises five feet.
        assert_eq!(
            terrain_correction_ft(&shape, 1., basis, [0.; 3], |x, _| x * 0.5),
            5.
        );
        // A floating complete assembly is lowered, not clamped to upward moves.
        assert_eq!(
            terrain_correction_ft(&shape, 1., basis, [0., 9., 0.], |_, _| 0.),
            -9.
        );
    }

    #[test]
    fn large_flat_or_steep_shapes_keep_the_origin_rule() {
        let level = tore_sim::attitude::Basis::new(0., 0., 0.);
        let slope = |x: f64, _: f64| x * 0.5;
        // Compact, gentle: lifted 5 ft, 5 ft gap under its low end.
        assert_eq!(
            slope_lift_ft(&upright(0., 20.), 1., level, [0.; 3], slope),
            Some(5.)
        );
        // Reaching 500 ft (a city block): the origin rule, cut into the hill.
        assert_eq!(
            slope_lift_ft(&upright(0., 20.), 50., level, [0.; 3], slope),
            None
        );
        // Flat ground art (a field, a lot): the origin rule.
        assert_eq!(
            slope_lift_ft(&upright(0., 0.), 1., level, [0.; 3], slope),
            None
        );
        // Compact but steep: a 60 ft rise across 100 ft leaves 60 ft of air.
        let steep = |x: f64, _: f64| x * 0.6;
        assert_eq!(
            slope_lift_ft(&upright(0., 20.), 10., level, [0.; 3], steep),
            None
        );
        assert_eq!(reach_ft(&upright(0., 20.), 50.), 500.);
    }

    #[test]
    fn a_driving_unit_takes_the_same_lift_as_it_stands() {
        let shape = upright(-5., 20.);
        let footing = UnitFooting::of(&shape, 1.);
        let level = tore_sim::attitude::Basis::new(0., 0., 0.);
        let slope = |x: f64, _: f64| x * 0.5;
        // Its pose stands its bottom on the ground under it, as the origin
        // rule stands the shape; the lift is the standing shape's.
        let standing = slope_lift_ft(&shape, 1., level, [0., 5., 0.], slope);
        assert_eq!(standing, Some(5.));
        assert_eq!(unit_slope_lift_ft(&footing, level, [0.; 3], slope), 5.);
        // Tilted with a plane slope, nothing is left to lift.
        let tilted =
            tore_sim::attitude::Basis::new(std::f64::consts::FRAC_PI_2, (0.5f64).atan(), 0.);
        assert!(unit_slope_lift_ft(&footing, tilted, [0.; 3], slope).abs() < 1e-9);
        // A unit with no footing (not in the scene) is not lifted.
        assert_eq!(
            unit_slope_lift_ft(&UnitFooting::default(), level, [0.; 3], slope),
            0.
        );
    }

    #[test]
    fn placed_contact_and_draw_origin_rise_together_and_keep_authored_height() {
        use crate::terrain::Placements;
        use std::collections::BTreeMap;
        let resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut sources = Placements::of_types(&resources, std::iter::empty());
        sources
            .definitions
            .insert("TEST.OT".into(), definition("TEST.OT", "_OBJProc", 0));
        sources.shapes.insert("TEST.OT".into(), upright(0., 20.));
        let layout = tore_formats::mission::Layout::parse(
            "TEST.MM",
            b"textFormat\nobj\n type TEST.OT\n pos 0 7 0\n.\n",
        )
        .unwrap();
        let p = &layout.placements[0];
        let flat = sources.stance(p, 0.).unwrap();
        let slope = sources.stance_on_terrain(p, |x, _| x * 0.5).unwrap();
        assert_eq!(flat.origin[1], 7.);
        assert_eq!(slope.origin[1], 12.);
        assert_eq!(slope.center[1] - flat.center[1], 5.);
        assert_eq!(slope.support_origin, flat.support_origin);
        assert_eq!(slope.half, flat.half);
    }

    #[test]
    fn thin_lines_ground_the_shape_without_filling_the_air_above_its_solid_body() {
        use crate::terrain::Placements;
        use std::collections::BTreeMap;
        let resources: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        let mut sources = Placements::of_types(&resources, std::iter::empty());
        sources
            .definitions
            .insert("TEST.OT".into(), definition("TEST.OT", "_OBJProc", 0));
        let mut shape = upright(0., 20.);
        shape.lines.push(tore_formats::shape::Line {
            positions: [[0., 0., -1.], [0., 0., 1000.]],
            color: 1,
            fog: Default::default(),
        });
        sources.shapes.insert("TEST.OT".into(), shape);
        let layout = tore_formats::mission::Layout::parse(
            "TEST.MM",
            b"textFormat\nobj\n type TEST.OT\n pos 0 0 0\n.\n",
        )
        .unwrap();
        let placement = &layout.placements[0];
        let stance = sources.stance(placement, 0.).unwrap();
        assert_eq!(
            stance.origin[1], 1.,
            "the low line still establishes footing"
        );
        assert_eq!(stance.min[1], 0.);
        assert_eq!(stance.max[1], 20., "the high line does not raise contact");
        let bounds = tore_sim::airport::OrientedBox {
            center: stance.center,
            half: stance.half,
            heading: stance.heading,
            pitch: stance.pitch,
            bank: stance.bank,
        };
        assert!(
            bounds
                .segment_fraction([5., 100., -50.], [5., 100., 50.])
                .is_none()
        );
        assert!(
            bounds
                .segment_fraction([5., 10., -50.], [5., 10., 50.])
                .is_some()
        );
        // A downward round meets the solid top at 21 ft, not the cable at 1001.
        let at = bounds
            .segment_fraction([5., 100., 0.], [5., 0., 0.])
            .unwrap();
        assert!((at - 0.79).abs() < 1e-12);
        // A tall polygon is solid even when a line at that height is not.
        sources.shapes.get_mut("TEST.OT").unwrap().faces[0]
            .positions
            .push([0., 0., 1000.]);
        assert_eq!(sources.stance(placement, 0.).unwrap().max[1], 1000.);
    }

    #[test]
    fn a_scale_up_grows_from_the_feet() {
        // A man 20 units tall centred on his origin, lifted 10 ft to stand.
        let up = [0., 1., 0.];
        let origin = [5., 110., 7.];
        let boosted = scaled_origin(origin, up, -10., 1., 6.);
        // Six times larger, his feet stay at 100 ft and his origin rises.
        assert_eq!(boosted, [5., 160., 7.]);
        assert_eq!(boosted[1] - 10. * 6., 100.);
        // Feet already on the origin: nothing moves.
        assert_eq!(scaled_origin(origin, up, 0., 1., 6.), origin);
    }
}
