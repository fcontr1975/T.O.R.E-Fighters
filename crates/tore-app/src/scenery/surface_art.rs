//! Surface units in the picture (docs/spec/surface-defenses.md, "Destroyed
//! looks and drawing"): what the static airport batch cannot hold by itself.
//!
//! - **Destroyed looks.** A unit whose wreck stays (a ship's `_A` hull, the
//!   `DEST.OT` wreck of a vehicle, SAM launcher or gun, a bunker's damaged
//!   `~` variant, a carrier's damaged island) has its wreck built beside its
//!   standing geometry, and the batch swaps one for the other when combat
//!   destroys it. Every other building gets one too (B16): its shape's
//!   damage branch, else a ruin generated from it ([`super::rubble`]), and
//!   pavement keeps its intact look, so nothing destroyed vanishes but men.
//! - **Launcher rails.** A launcher shape that draws one missile per round
//!   still loaded (CHAP, SA2, SA3, SCD) is redrawn from its hardpoints' loads
//!   whenever they change, through `shape::loaded_count_word`.
//! - **Carriers.** A carrier hull carries its island and deck parts from the
//!   FA.EXE carrier table (`tore_formats::carrier`), each standing on the
//!   flight deck by its ground offset, at the hull's placed scale.
//! - **Every frame** ([`SurfaceArt::frame`]): the units that follow a route
//!   where combat moved them, the men and deck crew as viewer-facing sprites,
//!   and the pieces a destroyed parked aircraft throws.
//!
//! Every texture these may draw is appended to the world pages when the
//! scenery is built, so the renderer uploads them once.
use crate::{AppResult, camera::Camera, static_art::Image};
use std::collections::{BTreeMap, BTreeSet};
use tore_formats::{
    Pic,
    shape::{
        Billboard, CURRENT_TICKS_WORD, DAMAGED_WORD, LOADED_COUNT_BASE, SPRITE_FRAME_WORD, Shape,
        hardpoint_angle_word, loaded_count_word,
    },
    static_object::Definition,
};
use tore_sim::attitude::{Basis, Vector};
use tore_world::grounding::{Footing, footing, lift_ft};
use tore_world::surface::{
    DestroyedLook, Surface, SurfaceState, UnitId,
    aim::{self, Aims, Rule, Track},
};

/// Floats in one vertex of the world batches.
const FLOATS: usize = 10;
/// The static scene's geometry budget, in floats (32 MiB).
pub(super) const BUDGET: usize = 32 * 1024 * 1024 / 4;
/// The range, feet, out to which a man (SOLDIER, TROOPS) is drawn at his own
/// size; farther he is drawn larger in proportion, so he stays about as many
/// pixels tall as at this range (opinionated, agent, 2026-10-12).
const MAN_FULL_SIZE_RANGE_FT: f64 = 1_000.;
/// The most a man is drawn larger than life: three times, from 3,000 ft out
/// (John, 2026-10-13: six times was too large).
const MAN_MAX_BOOST: f64 = 3.;
/// How much larger than life a man is drawn `distance` ft from the viewer.
fn man_boost(distance: f64) -> f64 {
    (distance / MAN_FULL_SIZE_RANGE_FT).clamp(1., MAN_MAX_BOOST)
}
/// Where a man's sprites (`sprites`, standing at `stand`) are drawn for a
/// viewer at `eye`: [`man_boost`] times larger, growing up from his feet,
/// which stay on the ground (`tore_world::grounding::scaled_origin`).
pub(crate) fn man_stand(sprites: &[Billboard], stand: &Stand, eye: Vector) -> Stand {
    boosted(
        stand,
        tore_world::grounding::sprites_bottom(sprites).unwrap_or(0.),
        eye,
    )
}

/// Where a man whose lowest point is `bottom` shape units about his origin,
/// standing at `stand`, is drawn for a viewer at `eye`: [`man_boost`] times
/// larger, growing up from his feet.
fn boosted(stand: &Stand, bottom: f64, eye: Vector) -> Stand {
    let away = (0..3)
        .map(|i| (eye[i] - stand.origin[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    let boost = man_boost(away);
    Stand {
        scale: stand.scale * boost,
        origin: tore_world::grounding::scaled_origin(
            stand.origin,
            stand.basis.up,
            bottom,
            stand.scale,
            boost,
        ),
        ..*stand
    }
}

/// The lowest drawn point, world feet, of `shape` standing at `stand`, as
/// the scenery draws it (`--surface-dump --grounding`): its faces, and its
/// sprites as a man is drawn to a level viewer at each of `ranges` feet
/// (the far-man boost), each `None` when there is nothing of that kind.
pub(crate) fn drawn_bottoms(
    shape: &Shape,
    stand: &Stand,
    ranges: &[f64],
    man: bool,
) -> (Option<f64>, Vec<Option<f64>>) {
    let low = |drawn: &Drawn| {
        drawn
            .vertices
            .chunks_exact(FLOATS)
            .chain(drawn.lines.chunks_exact(FLOATS))
            .map(|v| f64::from(v[1]))
            .reduce(f64::min)
    };
    let layers = Layers::new();
    // The faces (`emit` draws no sprites).
    let mut drawn = Drawn::default();
    let _ = emit(shape, stand, &layers, &mut drawn, usize::MAX);
    let sprites = ranges
        .iter()
        .map(|range| {
            let eye = [stand.origin[0] + range, stand.origin[1], stand.origin[2]];
            // A man drawn as cards (TROOPS): his faces, boosted.
            let cards = (man && !shape.faces.is_empty())
                .then(|| {
                    let bottom = tore_world::grounding::bottom(shape)?;
                    let mut drawn = Drawn::default();
                    let _ = emit(
                        shape,
                        &boosted(stand, bottom, eye),
                        &layers,
                        &mut drawn,
                        usize::MAX,
                    );
                    low(&drawn)
                })
                .flatten();
            if shape.billboards.is_empty() {
                return cards;
            }
            let at = man_stand(&shape.billboards, stand, eye);
            let faces = shape
                .billboards
                .iter()
                .map(|one| facing(one, &at, [0., 0., 1.], [0., 1., 0.]))
                .collect();
            let mut drawn = Drawn::default();
            let sprites = Shape {
                lines: Vec::new(),
                faces,
                billboards: Vec::new(),
                state_words: BTreeSet::new(),
            };
            let _ = emit(&sprites, &at, &layers, &mut drawn, usize::MAX);
            match (low(&drawn), cards) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        })
        .collect();
    (low(&drawn), sprites)
}

/// The deck crew sprite's frames (`shape::SPRITE_FRAME_WORD`, 0 to 10).
const CREW_FRAMES: i32 = 11;
/// Frames per second of the deck crew's signal cycle (fitted: the native
/// frame choice is not traced).
const CREW_FPS: f64 = 4.;

/// Textures already in the world pages, by upper-case name.
pub(super) type Layers = BTreeMap<String, Image>;

/// Where a shape stands: feet per unit, orientation and origin.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stand {
    pub scale: f64,
    pub basis: Basis,
    pub origin: Vector,
}

/// Triangles and lines of the world batches, ten floats a vertex.
#[derive(Clone, Debug, Default)]
pub(super) struct Drawn {
    pub vertices: Vec<f32>,
    pub lines: Vec<f32>,
}

impl Drawn {
    fn extend(&mut self, other: &Drawn) {
        self.vertices.extend_from_slice(&other.vertices);
        self.lines.extend_from_slice(&other.lines);
    }
}

/// Appends every texture `shape` draws with to the world pages, once each.
/// `first_page` is the page count before the pages vector (the terrain's).
pub(super) fn preload(
    shape: &Shape,
    resources: &BTreeMap<String, Vec<u8>>,
    first_page: usize,
    pages: &mut Vec<u8>,
    layers: &mut Layers,
) -> AppResult<()> {
    let names = shape
        .faces
        .iter()
        .filter(|face| !face.texture.is_empty() && !face.uv.is_empty())
        .map(|face| &face.texture)
        .chain(shape.billboards.iter().map(|sprite| &sprite.texture));
    for name in names {
        let name = name.to_ascii_uppercase();
        if layers.contains_key(&name) {
            continue;
        }
        let pic = Pic::parse(
            resources
                .get(&name)
                .ok_or_else(|| format!("missing static texture {name}"))?,
        )?;
        let first = (first_page + pages.len()) / 65536;
        layers.insert(name, Image::append(&pic, pages, first)?);
    }
    Ok(())
}

/// One world vertex: position, texture coordinates, layer (-1 untextured),
/// colour slots and the palette index with its fog mode.
fn vertex(position: Vector, uv: [f32; 2], index: f32) -> [f32; FLOATS] {
    [
        position[0] as f32,
        position[1] as f32,
        position[2] as f32,
        uv[0],
        uv[1],
        -1.0,
        0.0,
        0.0,
        0.0,
        index,
    ]
}

/// The palette colour retail paints a ground shadow decal with: a solid
/// quad flat on the shape's lowest plane (the suspension bridge's span on the
/// water, the crane's footprint). The weather palette holds a magenta there,
/// so drawn as a colour it was a magenta patch (nr-fx2). It is a shadow
/// polygon, so it is drawn as one: a dark, half-covered (screen-door)
/// patch ([`SHADOW_LAYER`], John 2026-10-13: draw it as our own ground
/// shadows are drawn), not in the palette colour. `docs/formats/objects-and-shapes.md`,
/// "Ground shadow decals".
pub(super) const SHADOW_DECAL_COLOR: u8 = 6;
/// The layer value that marks a shadow decal's vertices to the static solid
/// pass, which keeps half its pixels in a fixed ordered pattern
/// (`airport_solid_fragment`).
pub const SHADOW_LAYER: f32 = -9.0;
/// A shadow decal's colour, sRGB (near black, a little blue).
const SHADOW_COLOR: [f32; 3] = [0.03, 0.03, 0.05];
/// The solid-colour face record the decals use (`subtype` 0x41).
const SOLID_FACE: u8 = 0x41;
/// How far above the lowest point a decal's plane may lie, shape units: the
/// bridge's decals sit within a unit of their tower bases' lowest points.
const DECAL_REACH: f32 = 1.5;

/// Whether `face` is a ground shadow decal of a shape whose lowest point is
/// `low` ([`SHADOW_DECAL_COLOR`]): solid, one colour, flat and on that
/// lowest plane ([`DECAL_REACH`]).
pub(super) fn is_shadow_decal(face: &tore_formats::shape::Face, low: f32) -> bool {
    face.subtype == SOLID_FACE
        && !face.colors.is_empty()
        && face.colors.iter().all(|c| *c == SHADOW_DECAL_COLOR)
        && face.positions.iter().all(|p| p[2] <= low + DECAL_REACH)
        && face
            .positions
            .iter()
            .all(|p| (p[2] - face.positions[0][2]).abs() < 0.5)
}

/// The lowest point of `shape`, in shape units.
fn lowest_point(shape: &Shape) -> f32 {
    shape
        .faces
        .iter()
        .flat_map(|face| face.positions.iter().map(|p| p[2]))
        .fold(f32::INFINITY, f32::min)
}

/// A lifted bridge's painted shadow stays on terrain, never in midair.
pub(super) fn ground_bridge_shadows(
    drawn: &mut Drawn,
    terrain: &tore_world::terrain::Terrain,
    limit: usize,
) -> AppResult<()> {
    let mut vertices = Vec::new();
    for triangle in drawn.vertices.chunks_exact(3 * FLOATS) {
        let corners: [&[f32]; 3] = std::array::from_fn(|i| &triangle[i * FLOATS..(i + 1) * FLOATS]);
        if corners[0][5] != SHADOW_LAYER {
            vertices.extend_from_slice(triangle);
            continue;
        }
        let center: [f32; 3] =
            std::array::from_fn(|a| corners.iter().map(|p| p[a]).sum::<f32>() / 3.);
        let raised = corners
            .iter()
            .any(|p| p[1] > terrain.height(p[0], p[2]).max(0.) + 2.)
            || center[1] > terrain.height(center[0], center[2]).max(0.) + 2.;
        if !raised {
            vertices.extend_from_slice(triangle);
            continue;
        }
        // Merely projecting the four corners leaves a horizontal shadow
        // suspended between the raised banks. Subdivide across the river.
        let reach = (0..3)
            .map(|i| {
                let a = corners[i];
                let b = corners[(i + 1) % 3];
                (a[0] - b[0]).hypot(a[2] - b[2])
            })
            .fold(0f32, f32::max);
        let steps = (reach / 64.).ceil().clamp(1., 256.) as usize;
        let point = |i: usize, j: usize| {
            let mut v: [f32; FLOATS] = std::array::from_fn(|a| {
                corners[0][a]
                    + (corners[1][a] - corners[0][a]) * i as f32 / steps as f32
                    + (corners[2][a] - corners[0][a]) * j as f32 / steps as f32
            });
            v[1] = v[1].min(terrain.height(v[0], v[2]).max(0.) + 1.);
            v
        };
        for i in 0..steps {
            for j in 0..steps - i {
                vertices.extend(point(i, j));
                vertices.extend(point(i + 1, j));
                vertices.extend(point(i, j + 1));
                if i + j + 1 < steps {
                    vertices.extend(point(i + 1, j));
                    vertices.extend(point(i + 1, j + 1));
                    vertices.extend(point(i, j + 1));
                }
            }
        }
        if vertices.len() > limit {
            return Err("bridge shadow exceeds scenery geometry budget".into());
        }
    }
    if vertices.len() > limit {
        return Err("bridge shadow exceeds scenery geometry budget".into());
    }
    drawn.vertices = vertices;
    Ok(())
}

/// Appends `shape`'s faces and lines standing at `stand` to `out`, as the
/// static scene draws a placement: shape axes right, forward, up; textures
/// from `layers` (a face whose texture is not there draws in its colour).
/// `origin` may be a world point (the static batch) or one relative to the
/// render origin (the moving batch). Fails past `limit` floats.
pub(super) fn emit(
    shape: &Shape,
    stand: &Stand,
    layers: &Layers,
    out: &mut Drawn,
    limit: usize,
) -> AppResult<()> {
    let Stand {
        scale,
        basis,
        origin,
    } = *stand;
    let place = |point: [f32; 3]| -> Vector {
        let right = f64::from(point[0]) * scale;
        let up = f64::from(point[2]) * scale;
        let forward = f64::from(point[1]) * scale;
        std::array::from_fn(|axis| {
            origin[axis]
                + basis.right[axis] * right
                + basis.up[axis] * up
                + basis.forward[axis] * forward
        })
    };
    let low = lowest_point(shape);
    for face in &shape.faces {
        if face.positions.len() < 3 {
            continue;
        }
        let decal = is_shadow_decal(face, low);
        let image = if face.texture.is_empty() || face.uv.is_empty() {
            None
        } else {
            layers.get(&face.texture.to_ascii_uppercase())
        };
        for triangle in 1..face.positions.len() - 1 {
            if out.vertices.len() + 30 > limit {
                return Err("static scene exceeds 32 MiB geometry budget".into());
            }
            let points: Vec<[f32; FLOATS]> = [0, triangle, triangle + 1]
                .into_iter()
                .map(|at| {
                    let uv = face.uv.get(at).copied().unwrap_or([0.0; 2]);
                    let uv = image.map_or([0., 0.], |image| {
                        [uv[0] + 0.5, image.height as f32 - 0.5 - uv[1]]
                    });
                    if decal {
                        // Dark, half covered, in its own colour (no palette).
                        let mut v = vertex(place(face.positions[at]), [0., 0.], -1.);
                        v[5] = SHADOW_LAYER;
                        v[6..9].copy_from_slice(&SHADOW_COLOR);
                        return v;
                    }
                    vertex(
                        place(face.positions[at]),
                        uv,
                        f32::from(face.colors[at]) + f32::from(face.fog as u8) * 256.,
                    )
                })
                .collect();
            match image {
                Some(image) => image.triangle(
                    points.try_into().expect("three triangle corners"),
                    &mut out.vertices,
                    limit,
                )?,
                None => out.vertices.extend(points.into_iter().flatten()),
            }
        }
    }
    for line in &shape.lines {
        for point in line.positions {
            let mut v = vertex(
                place(point),
                [0., 0.],
                f32::from(line.color) + f32::from(line.fog as u8) * 256.,
            );
            v[5] = -1.;
            out.lines.extend(v);
        }
    }
    Ok(())
}

/// Appends one untextured `face` standing at `stand` in colour `color`
/// (sRGB, lit as the scene's own colours are): a ruin's charred faces.
pub(super) fn emit_tinted(
    face: &tore_formats::shape::Face,
    stand: &Stand,
    color: [f32; 3],
    out: &mut Drawn,
) {
    if face.positions.len() < 3 {
        return;
    }
    let place = |point: [f32; 3]| -> Vector {
        std::array::from_fn(|axis| {
            stand.origin[axis]
                + stand.basis.right[axis] * f64::from(point[0]) * stand.scale
                + stand.basis.up[axis] * f64::from(point[2]) * stand.scale
                + stand.basis.forward[axis] * f64::from(point[1]) * stand.scale
        })
    };
    for triangle in 1..face.positions.len() - 1 {
        for at in [0, triangle, triangle + 1] {
            let mut v = vertex(place(face.positions[at]), [0., 0.], -1.);
            v[6..9].copy_from_slice(&color);
            out.vertices.extend(v);
        }
    }
}

/// A sprite turned to face the viewer whose right and up are the world
/// vectors `right` and `up`, as a face in `stand`'s shape axes.
fn facing(
    sprite: &Billboard,
    stand: &Stand,
    right: Vector,
    up: Vector,
) -> tore_formats::shape::Face {
    let local = |v: Vector| -> [f32; 3] {
        let along = |axis: Vector| (0..3).map(|i| v[i] * axis[i]).sum::<f64>() as f32;
        [
            along(stand.basis.right),
            along(stand.basis.forward),
            along(stand.basis.up),
        ]
    };
    sprite.face(local(right), local(up))
}

/// Whether `definition` is a man (an NT of class OTHER: SOLDIER, TROOPS,
/// RUNNER), whom the far-man boost draws larger at range.
pub(crate) fn is_man(definition: &Definition) -> bool {
    definition
        .resource_name
        .to_ascii_uppercase()
        .ends_with(".NT")
        && definition.category & tore_formats::surface_unit::class::OTHER != 0
}

impl SurfaceArt {
    /// Whether placement `id`'s intact look is drawn every frame by
    /// [`Self::frame`] rather than by the static batch (a man drawn as
    /// cards); its destroyed look stays in the static batch.
    pub(super) fn draws_intact_every_frame(&self, id: u32) -> bool {
        self.cards.contains_key(&id)
    }
}

/// A shape with the scale it is drawn at.
pub(super) struct Model {
    pub shape: Shape,
    pub scale: f64,
}

/// A unit that follows a route: drawn where combat moved it.
struct Mover {
    intact: Model,
    wreck: Option<Model>,
    /// Feet its shape stands above its pose (a land unit on its lowest
    /// point, as its standing placement and hit box).
    lift: f64,
    /// The same for its wreck, on the wreck's own lowest point.
    wreck_lift: f64,
}

/// A unit whose picture aims (docs/spec/surface-defenses.md, "Aiming"):
/// its body turns, or its turrets, and its antennas spin. Drawn every frame
/// while anything is turned or spins, in the static batch otherwise.
struct Aimer {
    /// Where it stands when it does not follow a route.
    stand: Stand,
    /// Its main shape's bytes, projected again for turrets, antennas and
    /// rails.
    bytes: Vec<u8>,
    /// The scenery look, for a body that turns with nothing else changed.
    intact: Shape,
    /// Turrets: hardpoint and rest heading (binary angle units).
    turrets: Vec<(u8, i16)>,
    /// Its rotating antennas spin with the mission clock.
    antennas: bool,
    /// The hardpoints whose loads the shape draws and their full loads.
    rails: Vec<u8>,
    full: Vec<u32>,
    /// It follows a route (drawn at its pose).
    routed: bool,
}

/// An aimer's projected shape: the cached one for its rails, or one made
/// for this frame's turrets and antennas.
enum Projected<'a> {
    Cached(&'a Shape),
    Fresh(Shape),
}

impl Projected<'_> {
    fn shape(&self) -> &Shape {
        match self {
            Projected::Cached(shape) => shape,
            Projected::Fresh(shape) => shape,
        }
    }
}

impl Aimer {
    /// Nothing but its body turns: the shape depends only on the rails.
    fn body_only(&self) -> bool {
        self.turrets.is_empty() && !self.antennas
    }
    /// The rails' loaded-count words for `loads` (none: all full).
    fn rail_words(&self, loads: Option<&Vec<u32>>, state: &mut BTreeMap<usize, i32>) {
        for (at, hardpoint) in self.rails.iter().enumerate() {
            let loaded = loads.map_or(self.full[at], |l| l[at]);
            if loaded > 0 {
                state.insert(loaded_count_word(*hardpoint), loaded as i32);
            }
        }
    }
}

/// `basis` turned `angle` radians about its own up axis, positive toward
/// the right.
fn yawed(basis: Basis, angle: f64) -> Basis {
    if angle == 0. {
        return basis;
    }
    let (sin, cos) = angle.sin_cos();
    Basis {
        right: std::array::from_fn(|i| basis.right[i] * cos - basis.forward[i] * sin),
        up: basis.up,
        forward: std::array::from_fn(|i| basis.forward[i] * cos + basis.right[i] * sin),
    }
}

/// A traverse in radians as the binary angle units `@HardpointAngle@4`
/// returns (65,536 a turn).
fn binary_angle(radians: f64) -> i32 {
    (radians * 65536. / std::f64::consts::TAU).round() as i32
}

/// `_currentTicks` (256 a second) at mission time `seconds`.
fn current_ticks(seconds: f64) -> i32 {
    (seconds.max(0.) * 256.) as i64 as i32
}

/// A launcher whose rails show its load.
struct Launcher {
    bytes: Vec<u8>,
    stand: Stand,
    /// The hardpoints whose loads the shape draws.
    hardpoints: Vec<u8>,
    /// Their full loads: the scenery look.
    full: Vec<u32>,
}

/// A viewer-facing sprite standing in the scene: a man, or a carrier's
/// deck crew (one sprite per signal frame, front and back sheets).
/// A man drawn as cards: his shape where he stands, his lowest point in
/// shape units.
struct Cards {
    stand: Stand,
    shape: Shape,
    bottom: f64,
}

struct Sprite {
    /// The object whose loss removes it: the man, or the crew's carrier.
    owner: u32,
    stand: Stand,
    sprites: Vec<Billboard>,
    crew: Option<[Vec<Billboard>; 2]>,
}

/// What a static object looks like now, when not standing intact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Look {
    /// Not drawn: destroyed with nothing left, or never a combat target.
    Hidden,
    /// Its wreck.
    Wreck,
    /// A launcher with these loads on its drawn hardpoints.
    Rails(Vec<u8>),
    /// Aiming: drawn every frame ([`SurfaceArt::frame`]), not in the batch.
    Aimed,
}

/// The surface units' art beside the static batch. See the module comment.
#[derive(Default)]
pub struct SurfaceArt {
    pub(super) layers: Layers,
    wrecks: BTreeMap<u32, Drawn>,
    launchers: BTreeMap<u32, Launcher>,
    rails: BTreeMap<(u32, Vec<u8>), Drawn>,
    movers: BTreeMap<u32, Mover>,
    sprites: Vec<Sprite>,
    /// The men drawn as cards, faces rather than sprites (the airfield
    /// squad TROOPS), by owner: drawn every frame, for the far-man boost.
    cards: BTreeMap<u32, Cards>,
    /// Units whose pictures aim, how each turns, and their aim.
    aimers: BTreeMap<u32, Aimer>,
    rules: BTreeMap<UnitId, Rule>,
    aims: Aims,
    /// The loads an aimer's rails show now, when not full.
    aimer_loads: BTreeMap<u32, Vec<u32>>,
    /// The projected shape of a body-only aimer (no turret, no antenna) for
    /// its loads in `aimer_loads`, made when the loads change rather than
    /// every frame. `None`: the projection failed, so the intact look draws.
    rail_shapes: BTreeMap<u32, Option<Shape>>,
    /// Placements that stay drawn when destroyed: the runways (B15). A
    /// runway's shape, retail or redrawn, is the whole airfield's pavement
    /// (and a retail one its towers and hangars too), so hiding it at zero
    /// hit points took the airport away; the bomb craters on it show the
    /// damage instead.
    kept: BTreeSet<u32>,
    /// Each building type's destroyed look when retail gives it no `~`
    /// variant (B16): its shape's damage branch, a generated ruin, or its
    /// intact look kept; made once per type.
    ruins: BTreeMap<String, std::sync::Arc<Ruined>>,
}

/// How a destroyed building with no `~` variant is drawn (B16,
/// docs/spec/surface-defenses.md, "Destroyed looks").
enum Ruined {
    /// The ruin retail keeps inside the shape: its damage branch (`ac`),
    /// drawn with `shape::DAMAGED_WORD` set.
    Branch(Shape),
    /// A ruin generated from the intact shape ([`super::rubble`]).
    Generated(super::rubble::Ruin),
    /// Pavement or a rock: it stays as it stood.
    Intact,
}

/// What [`SurfaceArt::add`] needs from the scenery build.
pub(super) struct Build<'a> {
    pub resources: &'a BTreeMap<String, Vec<u8>>,
    pub terrain: &'a tore_world::terrain::Terrain,
    pub definitions: &'a BTreeMap<String, Definition>,
    pub first_page: usize,
    pub pages: &'a mut Vec<u8>,
}

impl Build<'_> {
    fn bytes(&self, name: &str) -> AppResult<&Vec<u8>> {
        self.resources
            .get(name)
            .ok_or_else(|| format!("missing {name}; re-import media").into())
    }
    /// A shape's scenery look, its textures preloaded.
    fn scenery(&mut self, layers: &mut Layers, name: &str) -> AppResult<Shape> {
        self.shape(layers, name, None)
    }
    fn shape(
        &mut self,
        layers: &mut Layers,
        name: &str,
        state: Option<&BTreeMap<usize, i32>>,
    ) -> AppResult<Shape> {
        let bytes = self.bytes(name)?;
        let shape = match state {
            Some(state) => Shape::with_export_state(bytes, state),
            None => Shape::scenery(bytes),
        }
        .map_err(|e| format!("{name}: {e}"))?;
        preload(&shape, self.resources, self.first_page, self.pages, layers)?;
        Ok(shape)
    }
    /// An object type's main shape and its placed scale.
    fn object(&mut self, layers: &mut Layers, resource: &str) -> AppResult<Model> {
        let definition = Definition::parse(self.bytes(resource)?)?;
        let name = definition
            .main_shape
            .clone()
            .ok_or_else(|| format!("{resource} names no shape"))?;
        let scale = tore_world::terrain::placed_shape_scale(&definition, self.bytes(&name)?)?;
        Ok(Model {
            shape: self.scenery(layers, &name)?,
            scale,
        })
    }
}

impl SurfaceArt {
    /// Prepares the extra looks of placement `id` (object type
    /// `object_type`, intact geometry `intact` already built at `stand`).
    /// Returns geometry to add to the placement's intact look (a carrier's
    /// parts) and whether the placement is drawn every frame instead of in
    /// the static batch (it follows a route).
    pub(super) fn add(
        &mut self,
        build: &mut Build,
        id: u32,
        object_type: &str,
        intact: &Shape,
        stand: &Stand,
        lift: f64,
    ) -> AppResult<(Drawn, bool)> {
        let surface = &build.terrain.surface;
        let unit = surface.unit(UnitId(id));
        let definition = build.definitions.get(object_type);
        let main_shape = definition.and_then(|d| d.main_shape.clone());
        // Its wreck: the unit's look, or for any other building its `~`
        // damaged variant when the import holds one (as the bunkers'), else
        // nothing (removed).
        let look = match unit {
            Some(unit) => unit.look.clone(),
            None => {
                let damaged = format!("~{object_type}");
                let wreck = tore_formats::surface_unit::DESTROYED_VEHICLE_OBJECT;
                if !object_type.starts_with('~') && build.resources.contains_key(&damaged) {
                    DestroyedLook::DamagedObject(damaged)
                } else if tore_world::surface::wreck::is_vehicle_type(object_type)
                    && build.resources.contains_key(wreck)
                {
                    // A layout's vehicle with no unit record leaves the
                    // vehicles' wreck (B16).
                    DestroyedLook::Wreck(wreck.to_owned())
                } else {
                    DestroyedLook::Removed
                }
            }
        };
        let wreck = (|| -> AppResult<Option<Model>> {
            Ok(match &look {
                DestroyedLook::DamagedShape(name) => match definition {
                    Some(definition) => {
                        let scale = tore_world::terrain::placed_shape_scale(
                            definition,
                            build.bytes(name)?,
                        )?;
                        Some(Model {
                            shape: build.scenery(&mut self.layers, name)?,
                            scale,
                        })
                    }
                    None => None,
                },
                // The fallen men: their own shape's damage branch, at the
                // size the men are drawn.
                DestroyedLook::Bodies(name) => Some(Model {
                    shape: build.shape(
                        &mut self.layers,
                        name,
                        Some(&BTreeMap::from([(DAMAGED_WORD, 1)])),
                    )?,
                    scale: stand.scale,
                }),
                DestroyedLook::Wreck(object) | DestroyedLook::DamagedObject(object) => {
                    Some(build.object(&mut self.layers, object)?)
                }
                DestroyedLook::Removed | DestroyedLook::Vanish => None,
            })
        })()
        .unwrap_or_else(|error| {
            log::warn!("Surface: {object_type} {id:#010x} has no destroyed look: {error}");
            None
        });
        // A building with no `~` variant: its retail ruin, a generated one,
        // or (pavement, rocks) its intact look (B16).
        let ruined = if matches!(look, DestroyedLook::Removed) && !self.kept.contains(&id) {
            self.ruined(build, object_type, main_shape.as_deref(), intact)
        } else {
            None
        };
        if ruined
            .as_deref()
            .is_some_and(|r| matches!(r, Ruined::Intact))
        {
            self.kept.insert(id);
        }
        // The men stand as sprites.
        if !intact.billboards.is_empty() {
            self.sprites.push(Sprite {
                owner: id,
                stand: *stand,
                sprites: intact.billboards.clone(),
                crew: None,
            });
        }
        // Men drawn as cards (TROOPS) are drawn every frame instead of in
        // the static batch, so they can be boosted at range like the
        // sprites (agent decision, slice GROUND, 2026-10-13).
        if definition.is_some_and(is_man)
            && !intact.faces.is_empty()
            && !surface.courses.contains_key(&UnitId(id))
            && let Some(name) = &main_shape
            && let Some(bottom) = tore_world::grounding::bottom(intact)
        {
            let shape = build.scenery(&mut self.layers, name)?;
            self.cards.insert(
                id,
                Cards {
                    stand: *stand,
                    shape,
                    bottom,
                },
            );
        }
        // A land unit's wreck lies on its own lowest point too (the DEST
        // wreck reaches below its origin), and so does a building's damaged
        // variant; a ship's `_A` hull keeps the waterline.
        let land = definition.is_some_and(|d| footing(d) == Footing::Bottom);
        let wreck_lift = match &wreck {
            Some(model) if land => lift_ft(&model.shape, model.scale),
            _ => 0.,
        };
        if let Some(name) = &main_shape
            && let Err(error) = self.add_aimer(build, id, object_type, name, intact, stand)
        {
            log::warn!("Surface: {object_type} {id:#010x} does not aim: {error}");
        }
        if surface.courses.contains_key(&UnitId(id))
            && let Some(name) = &main_shape
        {
            let shape = build.scenery(&mut self.layers, name)?;
            self.movers.insert(
                id,
                Mover {
                    intact: Model {
                        shape,
                        scale: stand.scale,
                    },
                    wreck,
                    lift,
                    wreck_lift,
                },
            );
            return Ok((Drawn::default(), true));
        }
        let mut wreck_drawn = wreck.map(|model| -> AppResult<Drawn> {
            let mut drawn = Drawn::default();
            // The wreck lies where the unit stood, on the ground.
            emit(
                &model.shape,
                &Stand {
                    scale: model.scale,
                    basis: stand.basis,
                    origin: std::array::from_fn(|i| {
                        stand.origin[i] + stand.basis.up[i] * (wreck_lift - lift)
                    }),
                },
                &self.layers,
                &mut drawn,
                BUDGET,
            )?;
            Ok(drawn)
        });
        if wreck_drawn.is_none()
            && let Some(ruin) = &ruined
        {
            wreck_drawn = self.ruin_drawn(ruin, stand, lift, land).map(Ok);
        }
        let mut parts = Drawn::default();
        if let Some(carrier) = main_shape
            .as_deref()
            .and_then(tore_formats::carrier::for_hull)
        {
            match self.carrier(build, id, carrier, stand) {
                Ok((intact_parts, damaged_parts)) => {
                    parts = intact_parts;
                    if let Some(Ok(wreck)) = wreck_drawn.as_mut() {
                        wreck.extend(&damaged_parts);
                    }
                }
                Err(error) => log::warn!("Surface: {} parts not drawn: {error}", carrier.hull),
            }
        }
        if let Some(wreck) = wreck_drawn {
            let mut wreck = wreck?;
            if tore_world::grounding::bridges::is_bridge(object_type) {
                ground_bridge_shadows(&mut wreck, build.terrain, BUDGET)?;
            }
            self.wrecks.insert(id, wreck);
        }
        // A launcher whose shape draws its load.
        if let Some(name) = &main_shape
            && let Some(arms) = surface.arsenal.arms(UnitId(id))
        {
            let bytes = build.bytes(name)?.clone();
            let hardpoints = Shape::with_export_state(&bytes, &BTreeMap::new())
                .map(|shape| rail_hardpoints(&shape))
                .unwrap_or_default();
            if !hardpoints.is_empty() {
                let full = hardpoints
                    .iter()
                    .map(|hp| arms.loads.get(usize::from(*hp)).map_or(0, |m| m.loaded))
                    .collect();
                self.launchers.insert(
                    id,
                    Launcher {
                        bytes,
                        stand: *stand,
                        hardpoints,
                        full,
                    },
                );
            }
        }
        Ok((parts, false))
    }

    /// The destroyed look of building type `object_type` (main shape
    /// `main_shape`, intact look `intact`) when retail gives it no `~`
    /// variant: made once per type. `None`: nothing can be drawn.
    fn ruined(
        &mut self,
        build: &mut Build,
        object_type: &str,
        main_shape: Option<&str>,
        intact: &Shape,
    ) -> Option<std::sync::Arc<Ruined>> {
        if let Some(ruined) = self.ruins.get(object_type) {
            return Some(std::sync::Arc::clone(ruined));
        }
        use tore_world::surface::wreck::{FireClass, object_fire};
        let bytes = main_shape.and_then(|name| build.resources.get(name));
        // Retail's damage branch, when following it changes the shape.
        let branch = main_shape.zip(bytes).and_then(|(name, bytes)| {
            if !tore_formats::shape::has_damage_branch(bytes) {
                return None;
            }
            let damaged = BTreeMap::from([(DAMAGED_WORD, 1)]);
            build.shape(&mut self.layers, name, Some(&damaged)).ok()
        });
        // What does not burn (pavement or a rock) stays as it
        // stood unless retail draws it broken.
        let burns = object_fire(object_type, false) != FireClass::None;
        let bridge_end = ["BR1END.OT", "BR2END.OT", "BR3END.OT", "BRDEND.OT"]
            .iter()
            .any(|n| n.eq_ignore_ascii_case(object_type));
        let ruined = match branch {
            Some(shape) => Ruined::Branch(shape),
            None if !burns && !bridge_end => Ruined::Intact,
            None => match super::rubble::collapse(intact) {
                Some(ruin) => Ruined::Generated(ruin),
                None => Ruined::Intact,
            },
        };
        let ruined = std::sync::Arc::new(ruined);
        self.ruins
            .insert(object_type.to_owned(), std::sync::Arc::clone(&ruined));
        Some(ruined)
    }

    /// A ruin's geometry standing where the building stood (`stand`, its
    /// shape `lift` feet above its placement point; a `land` object's ruin
    /// rests on its own lowest point). `None` for a kept intact look.
    fn ruin_drawn(&self, ruin: &Ruined, stand: &Stand, lift: f64, land: bool) -> Option<Drawn> {
        let (shape, charred): (&Shape, &[_]) = match ruin {
            Ruined::Branch(shape) => (shape, &[]),
            Ruined::Generated(ruin) => (&ruin.walls, &ruin.charred[..]),
            Ruined::Intact => return None,
        };
        let shape_lift = if land {
            match ruin {
                // The generated ruin keeps the intact shape's lowest point.
                Ruined::Generated(..) => lift,
                _ => lift_ft(shape, stand.scale),
            }
        } else {
            lift
        };
        let at = Stand {
            origin: std::array::from_fn(|i| {
                stand.origin[i] + stand.basis.up[i] * (shape_lift - lift)
            }),
            ..*stand
        };
        let mut drawn = Drawn::default();
        emit(shape, &at, &self.layers, &mut drawn, BUDGET).ok()?;
        for (face, color) in charred {
            emit_tinted(face, &at, *color, &mut drawn);
        }
        Some(drawn)
    }

    /// Registers unit `id` (object type `object_type`, main shape `name`)
    /// as an aimer when its picture can turn: a shape with turrets (its
    /// mounts traverse) or rotating antennas, or an armed land unit whose
    /// type turns (its whole body turns; docs/spec/surface-defenses.md,
    /// "Aiming").
    fn add_aimer(
        &mut self,
        build: &mut Build,
        id: u32,
        object_type: &str,
        name: &str,
        intact: &Shape,
        stand: &Stand,
    ) -> AppResult<()> {
        let surface = &build.terrain.surface;
        let Some(unit) = surface.unit(UnitId(id)) else {
            return Ok(());
        };
        let bytes = build.bytes(name)?.clone();
        let parts = tore_formats::shape::articulation(&bytes)?;
        let arms = surface.arsenal.arms(UnitId(id));
        use tore_formats::surface_unit::class;
        let land = unit.class & (class::SHIP | class::STRUCTURE) == 0;
        let rule = if !parts.turrets.is_empty() {
            arms.map(|_| {
                Rule::Turrets(
                    parts
                        .turrets
                        .iter()
                        .map(|(hardpoint, _)| usize::from(*hardpoint))
                        .collect(),
                )
            })
        } else if land && arms.is_some() && !intact.faces.is_empty() {
            // The type's turn rate (`_turnRate`, 182 a degree, a second): a
            // fixed site (the SA-2's six rails) reads 0 and does not turn.
            let record = tore_formats::surface_unit::SurfaceUnit::parse(build.bytes(object_type)?)?;
            let rate = f64::from(record.movement.turn_rate)
                / f64::from(tore_formats::surface_unit::ANGLE_UNITS_PER_DEGREE);
            (rate > 0.).then_some(Rule::Body { rate_deg_s: rate })
        } else {
            None
        };
        if rule.is_none() && parts.antennas.is_empty() {
            return Ok(());
        }
        let rails = Shape::with_export_state(&bytes, &BTreeMap::new())
            .map(|shape| rail_hardpoints(&shape))
            .unwrap_or_default();
        let full = rails
            .iter()
            .map(|hp| {
                arms.and_then(|a| a.loads.get(usize::from(*hp)))
                    .map_or(0, |m| m.loaded)
            })
            .collect();
        if let Some(rule) = rule {
            self.rules.insert(UnitId(id), rule);
        }
        self.aimers.insert(
            id,
            Aimer {
                stand: *stand,
                bytes,
                intact: build.scenery(&mut self.layers, name)?,
                turrets: parts.turrets,
                antennas: !parts.antennas.is_empty(),
                rails,
                full,
                routed: surface.courses.contains_key(&UnitId(id)),
            },
        );
        Ok(())
    }

    /// Steps every aimer to `picture`'s tick: `tracks` are what the units'
    /// weapons hold, `surface` the mission's units and arms, `stock` their
    /// launchers' loads when the viewer knows them.
    pub fn step_aim(
        &mut self,
        picture: &crate::snapshot::RenderSnapshot,
        surface: &Surface,
        tracks: &[Track],
        stock: Option<&SurfaceState>,
    ) {
        let stance = |id: UnitId| -> Option<aim::Stance> {
            if let Some(pose) = picture.surface.iter().find(|pose| pose.id == id) {
                return Some(aim::Stance {
                    origin: pose.position,
                    heading: pose.attitude[0],
                });
            }
            surface.arsenal.arms(id).map(|arms| aim::Stance {
                origin: arms.position,
                heading: arms.heading,
            })
        };
        let arc = |id: UnitId, hardpoint: usize| {
            surface
                .arsenal
                .arms(id)
                .and_then(|arms| aim::mount_arc(arms, hardpoint))
        };
        // Nothing aims at a crashed aircraft or a wreck: a unit keeps the
        // aim it had (a body) or turns back to rest (a turret).
        let target = |id: u32| -> Option<Vector> {
            if picture.player.id == id {
                return (!picture.player.crashed).then_some(picture.player.position);
            }
            if let Some(pose) = picture.targets.iter().find(|pose| pose.id == id) {
                return (!pose.crashed).then_some(pose.position);
            }
            if let Some(pose) = picture.surface.iter().find(|pose| pose.id.0 == id) {
                return (!pose.wrecked).then_some(pose.position);
            }
            surface
                .arsenal
                .arms(UnitId(id))
                .map(|arms| arms.position)
                .or_else(|| {
                    surface
                        .unit(UnitId(id))
                        .map(|unit| unit.position.map(f64::from))
                })
        };
        // A destroyed unit no longer aims.
        let alive: Vec<Track> = tracks
            .iter()
            .filter(|track| {
                !picture
                    .targets
                    .iter()
                    .any(|pose| pose.id == track.unit.0 && pose.crashed)
            })
            .copied()
            .collect();
        self.aims
            .step(picture.tick, &self.rules, &alive, &arc, &stance, &target);
        if let Some(stock) = stock {
            for (id, aimer) in &self.aimers {
                if aimer.rails.is_empty() {
                    continue;
                }
                // A unit the stock does not list is fully loaded (a replay
                // lists only the units whose rails changed).
                let Some(state) = stock.unit(UnitId(*id)).filter(|unit| unit.armed) else {
                    self.aimer_loads.remove(id);
                    self.rail_shapes.remove(id);
                    continue;
                };
                let loads: Vec<u32> = aimer
                    .rails
                    .iter()
                    .map(|hp| state.mounts.get(usize::from(*hp)).map_or(0, |m| m.loaded))
                    .collect();
                if loads == aimer.full {
                    self.aimer_loads.remove(id);
                    self.rail_shapes.remove(id);
                } else {
                    if aimer.body_only() && self.aimer_loads.get(id) != Some(&loads) {
                        let mut words = BTreeMap::new();
                        aimer.rail_words(Some(&loads), &mut words);
                        self.rail_shapes
                            .insert(*id, Shape::with_export_state(&aimer.bytes, &words).ok());
                    }
                    self.aimer_loads.insert(*id, loads);
                }
            }
        }
    }

    /// Unit `id`'s aim now, when its picture aims.
    pub fn aim_of(&self, id: u32) -> Option<&aim::UnitAim> {
        self.aims.unit(UnitId(id))
    }

    /// Whether aimer `id` is drawn every frame now: aiming has begun and its
    /// antennas spin or a part stands turned.
    fn aiming(&self, id: u32) -> bool {
        let Some(aimer) = self.aimers.get(&id) else {
            return false;
        };
        self.aims.tick.is_some()
            && (aimer.antennas
                || !aimer.turrets.is_empty()
                || self.aims.unit(UnitId(id)).is_some_and(aim::UnitAim::turned))
    }

    /// Aimer `id`'s shape and the turn of its body now, at mission time
    /// `seconds`: projected again when a turret, antenna or rail changes the
    /// scenery look.
    fn aimed_shape(&self, id: u32, aimer: &Aimer, seconds: f64) -> (Option<Projected<'_>>, f64) {
        let aim = self.aims.unit(UnitId(id));
        let body = aim.map_or(0., |a| a.body);
        let loads = self.aimer_loads.get(&id);
        // A launcher whose body turned and whose rails emptied: its shape
        // changes only with the loads, so it was projected when they did.
        if aimer.body_only()
            && loads.is_some()
            && let Some(cached) = self.rail_shapes.get(&id)
        {
            return (cached.as_ref().map(Projected::Cached), body);
        }
        let mut state = BTreeMap::new();
        for (hardpoint, _) in &aimer.turrets {
            let angle = aim
                .and_then(|a| a.turrets.get(&usize::from(*hardpoint)))
                .copied()
                .unwrap_or(0.);
            state.insert(hardpoint_angle_word(*hardpoint), binary_angle(angle));
        }
        if aimer.antennas {
            state.insert(CURRENT_TICKS_WORD, current_ticks(seconds));
        }
        if state.is_empty() && loads.is_none() {
            return (None, body);
        }
        aimer.rail_words(loads, &mut state);
        (
            Shape::with_export_state(&aimer.bytes, &state)
                .ok()
                .map(Projected::Fresh),
            body,
        )
    }

    /// A carrier's island and deck parts at `stand` (docs/spec/surface-
    /// defenses.md, "Carriers"): intact, and as they stand by the burning
    /// hull (the island's damage branch, the tractors as they are, the deck
    /// crew gone). Each part stands on the hull's flight deck by its own
    /// ground offset (lead ruling after S2), its table offset and the deck
    /// at the hull's placed scale; the deck crew is a sprite.
    fn carrier(
        &mut self,
        build: &mut Build,
        hull_id: u32,
        carrier: &tore_formats::carrier::Carrier,
        stand: &Stand,
    ) -> AppResult<(Drawn, Drawn)> {
        let hull_bytes = build.bytes(carrier.hull)?;
        let authored = tore_formats::shape::object_scale(hull_bytes)?;
        let deck = tore_formats::carrier::flight_deck(&Shape::scenery(hull_bytes)?)
            .ok_or_else(|| format!("{}: no flight deck", carrier.hull))?;
        // Table offsets are retail feet around the hull at its scenery
        // scale; the hull stands at `stand.scale`.
        let factor = stand.scale / authored;
        let deck_feet = f64::from(deck.height) * stand.scale;
        let mut intact = Drawn::default();
        let mut damaged = Drawn::default();
        for (index, part) in carrier.parts.iter().enumerate() {
            let island = index + 1 == carrier.parts.len();
            let bytes = build.bytes(part.shape)?;
            let scale = tore_formats::shape::object_scale(bytes)? * factor;
            // The F2 ground offset is in retail feet (the island's matches
            // its lowest point at the scenery scale), so it takes the hull's
            // factor, not the part's shape scale.
            let contact =
                f64::from(tore_formats::shape::contact_offset(bytes)?.unwrap_or(0)) * factor;
            let [right, up, forward] = part.offset.map(|v| f64::from(v) * factor);
            let lift = deck_feet + up - contact;
            let turn = f64::from(part.heading) * std::f64::consts::TAU / 65536.;
            let yaw = Basis::new(turn, 0., 0.);
            // The part's axes turned by its heading, then by the hull's.
            let rotate = |v: Vector| -> Vector {
                std::array::from_fn(|i| {
                    stand.basis.right[i] * v[0]
                        + stand.basis.up[i] * v[1]
                        + stand.basis.forward[i] * v[2]
                })
            };
            let part_stand = Stand {
                scale,
                basis: Basis {
                    right: rotate(yaw.right),
                    up: rotate(yaw.up),
                    forward: rotate(yaw.forward),
                },
                origin: std::array::from_fn(|i| {
                    stand.origin[i]
                        + stand.basis.right[i] * right
                        + stand.basis.up[i] * lift
                        + stand.basis.forward[i] * forward
                }),
            };
            let shape = build.scenery(&mut self.layers, part.shape)?;
            if !shape.billboards.is_empty() {
                // The deck crew: front and back sheets, one per frame.
                let mut sheets: [Vec<Billboard>; 2] = Default::default();
                for frame in 0..CREW_FRAMES {
                    let state = BTreeMap::from([(SPRITE_FRAME_WORD, frame << 16)]);
                    let front = build.shape(&mut self.layers, part.shape, Some(&state))?;
                    let back = Shape::with_state(build.bytes(part.shape)?, &state)
                        .map_err(|e| format!("{}: {e}", part.shape))?;
                    preload(
                        &back,
                        build.resources,
                        build.first_page,
                        build.pages,
                        &mut self.layers,
                    )?;
                    sheets[0].extend(front.billboards.first().cloned());
                    sheets[1].extend(back.billboards.first().cloned());
                }
                self.sprites.push(Sprite {
                    owner: hull_id,
                    stand: part_stand,
                    sprites: shape.billboards.clone(),
                    crew: Some(sheets),
                });
                continue;
            }
            emit(&shape, &part_stand, &self.layers, &mut intact, BUDGET)?;
            let after = if island {
                build.shape(
                    &mut self.layers,
                    part.shape,
                    Some(&BTreeMap::from([(DAMAGED_WORD, 1)])),
                )?
            } else {
                shape
            };
            emit(&after, &part_stand, &self.layers, &mut damaged, BUDGET)?;
        }
        Ok((intact, damaged))
    }

    /// Prepares parked aircraft `id`'s destroyed look: its own `shape` (drawn
    /// gear down at `stand`) as a burnt hulk, charred, broken and settled
    /// onto its belly ([`super::rubble::hulk`]). The batch swaps it for the
    /// standing aircraft when combat destroys it, and a replay does the same
    /// from the recorded destruction, so every machine shows the same hulk.
    /// A shape with no faces leaves the aircraft hidden.
    pub(super) fn add_parked(&mut self, id: u32, shape: &Shape, stand: &Stand) {
        let Some(ruin) = super::rubble::hulk(shape) else {
            return;
        };
        // The hulk stands on its own bottom by the shared grounding rule: the
        // aircraft's bottom is on the ground, so the hulk (whose lowest face
        // may have been torn away) is moved to put its bottom there too.
        let whole = Shape {
            lines: Vec::new(),
            faces: ruin
                .walls
                .faces
                .iter()
                .chain(ruin.charred.iter().map(|(face, _)| face))
                .cloned()
                .collect(),
            billboards: Vec::new(),
            state_words: BTreeSet::new(),
        };
        let settle = lift_ft(&whole, stand.scale) - lift_ft(shape, stand.scale);
        let at = Stand {
            origin: std::array::from_fn(|i| stand.origin[i] + stand.basis.up[i] * settle),
            ..*stand
        };
        let mut drawn = Drawn::default();
        if emit(&ruin.walls, &at, &self.layers, &mut drawn, BUDGET).is_err() {
            return;
        }
        for (face, color) in &ruin.charred {
            emit_tinted(face, &at, *color, &mut drawn);
        }
        self.wrecks.insert(id, drawn);
    }

    /// Keeps placement `id` drawn when it is destroyed: a runway.
    pub(super) fn keep(&mut self, id: u32) {
        self.kept.insert(id);
    }

    /// Placement `id`'s look given whether it stands (`alive`), whether
    /// combat holds it at all (`known`), and its launcher loads in `surface`
    /// (none: a replay, which records no loads yet). `None` is its intact
    /// look.
    pub(super) fn look(
        &self,
        id: u32,
        alive: bool,
        known: bool,
        surface: Option<&SurfaceState>,
    ) -> Option<Look> {
        if !alive {
            if self.kept.contains(&id) {
                return (known && self.wrecks.contains_key(&id)).then_some(Look::Wreck);
            }
            return Some(if known && self.wrecks.contains_key(&id) {
                Look::Wreck
            } else {
                Look::Hidden
            });
        }
        if self.aiming(id) {
            return Some(Look::Aimed);
        }
        let launcher = self.launchers.get(&id)?;
        let state = surface?.unit(UnitId(id)).filter(|unit| unit.armed)?;
        let loads: Vec<u32> = launcher
            .hardpoints
            .iter()
            .map(|hp| state.mounts.get(usize::from(*hp)).map_or(0, |m| m.loaded))
            .collect();
        (loads != launcher.full)
            .then(|| Look::Rails(loads.iter().map(|n| (*n).min(255) as u8).collect()))
    }

    /// The geometry of look `look` for `id`, building a rail state the
    /// first time it is asked for; `None` when it draws nothing or cannot be
    /// built.
    pub(super) fn drawn(&mut self, id: u32, look: &Look) -> Option<&Drawn> {
        match look {
            Look::Hidden | Look::Aimed => None,
            Look::Wreck => self.wrecks.get(&id),
            Look::Rails(loads) => {
                let key = (id, loads.clone());
                if !self.rails.contains_key(&key) {
                    let launcher = self.launchers.get(&id)?;
                    let state: BTreeMap<usize, i32> = launcher
                        .hardpoints
                        .iter()
                        .zip(loads)
                        .filter(|(_, n)| **n > 0)
                        .map(|(hp, n)| (loaded_count_word(*hp), i32::from(*n)))
                        .collect();
                    let shape = Shape::with_export_state(&launcher.bytes, &state).ok()?;
                    let mut drawn = Drawn::default();
                    emit(&shape, &launcher.stand, &self.layers, &mut drawn, BUDGET).ok()?;
                    self.rails.insert(key.clone(), drawn);
                }
                self.rails.get(&key)
            }
        }
    }

    /// This frame's moving surface geometry, relative to the render origin
    /// `origin`: the routed units at their blended poses (their wreck once
    /// destroyed), the men and deck crew facing `camera` (gone with their
    /// owner), and parked aircraft pieces in flight. `standing` says whether
    /// an object stands; `seconds` is mission time, for the crew's signals.
    pub fn frame(
        &self,
        picture: &crate::snapshot::RenderSnapshot,
        standing: &dyn Fn(u32) -> bool,
        camera: &Camera,
        origin: Vector,
        seconds: f64,
    ) -> Vec<f32> {
        let mut out = Drawn::default();
        let local = |p: Vector| -> Vector { std::array::from_fn(|i| p[i] - origin[i]) };
        let eye = local(camera.position);
        for pose in &picture.surface {
            let Some(mover) = self.movers.get(&pose.id.0) else {
                continue;
            };
            let model = if pose.wrecked {
                match &mover.wreck {
                    Some(wreck) => wreck,
                    None => continue,
                }
            } else {
                &mover.intact
            };
            let [yaw, pitch, bank] = pose.attitude;
            let mut basis = Basis::new(yaw, pitch, bank);
            let at = local(pose.position);
            // A wreck lies on the ground; the unit stands on its lowest point.
            let lift = if pose.wrecked {
                mover.wreck_lift
            } else {
                mover.lift
            };
            // An aiming unit turns its body or turrets.
            let mut aimed = None;
            if !pose.wrecked
                && let Some(aimer) = self.aimers.get(&pose.id.0)
                && self.aims.tick.is_some()
            {
                let (shape, body) = self.aimed_shape(pose.id.0, aimer, seconds);
                basis = yawed(basis, body);
                aimed = shape;
            }
            let stand = Stand {
                scale: model.scale,
                basis,
                origin: std::array::from_fn(|i| at[i] + basis.up[i] * lift),
            };
            // Each frame's batch is bounded by the moving buffer, not the
            // static budget.
            let shape = aimed.as_ref().map_or(&model.shape, Projected::shape);
            let _ = emit(shape, &stand, &self.layers, &mut out, usize::MAX);
            if !pose.wrecked {
                self.sprites_of(pose.id.0, &stand, camera, eye, seconds, &mut out);
            }
        }
        // Standing units that aim, at their place with their aim.
        for (id, aimer) in &self.aimers {
            if aimer.routed || !standing(*id) || !self.aiming(*id) {
                continue;
            }
            let (shape, body) = self.aimed_shape(*id, aimer, seconds);
            let basis = yawed(aimer.stand.basis, body);
            let mut stand = Stand {
                scale: aimer.stand.scale,
                basis,
                origin: local(aimer.stand.origin),
            };
            // A squad turning to its target is boosted like any man.
            if let Some(man) = self.cards.get(id) {
                stand = boosted(&stand, man.bottom, eye);
            }
            let shape = shape.as_ref().map_or(&aimer.intact, Projected::shape);
            let _ = emit(shape, &stand, &self.layers, &mut out, usize::MAX);
        }
        for sprite in &self.sprites {
            if self.movers.contains_key(&sprite.owner) || !standing(sprite.owner) {
                continue;
            }
            let stand = Stand {
                origin: local(sprite.stand.origin),
                ..sprite.stand
            };
            self.draw_sprite(sprite, &stand, camera, eye, seconds, &mut out);
        }
        for (id, man) in &self.cards {
            // An aiming squad is drawn turned, above.
            if !standing(*id) || self.aiming(*id) {
                continue;
            }
            let stand = Stand {
                origin: local(man.stand.origin),
                ..man.stand
            };
            let _ = emit(
                &man.shape,
                &boosted(&stand, man.bottom, eye),
                &self.layers,
                &mut out,
                usize::MAX,
            );
        }
        out.vertices
    }

    /// The sprites a moving unit carries, at its pose. `eye` is the camera
    /// in the same frame as `stand`.
    fn sprites_of(
        &self,
        owner: u32,
        stand: &Stand,
        camera: &Camera,
        eye: Vector,
        seconds: f64,
        out: &mut Drawn,
    ) {
        for sprite in self.sprites.iter().filter(|s| s.owner == owner) {
            self.draw_sprite(sprite, stand, camera, eye, seconds, out);
        }
    }

    fn draw_sprite(
        &self,
        sprite: &Sprite,
        stand: &Stand,
        camera: &Camera,
        eye: Vector,
        seconds: f64,
        out: &mut Drawn,
    ) {
        // A man stays a few pixels tall at the range a pilot sees a field:
        // beyond 1,000 ft he is drawn taller by the distance over 1,000 ft,
        // up to three times (presentation only; the hit box is unchanged). He
        // grows up from his feet, which stay on the ground.
        let stand = &if sprite.crew.is_none() {
            man_stand(&sprite.sprites, stand, eye)
        } else {
            *stand
        };
        let view = camera.uniform(1., [0.; 4], [0; 3]);
        let right: Vector = std::array::from_fn(|i| f64::from(view[4 + i]));
        let up: Vector = std::array::from_fn(|i| f64::from(view[8 + i]));
        let sprites: &[Billboard] = match &sprite.crew {
            Some(sheets) => {
                // The front sheet while the viewer is ahead of the crewman,
                // the back sheet behind; the signal frame cycles.
                let toward: Vector = std::array::from_fn(|i| eye[i] - stand.origin[i]);
                let ahead = (0..3)
                    .map(|i| toward[i] * stand.basis.forward[i])
                    .sum::<f64>()
                    >= 0.;
                let sheet = &sheets[usize::from(!ahead)];
                let frame = ((seconds.max(0.) * CREW_FPS) as usize) % sheet.len().max(1);
                match sheet.get(frame) {
                    Some(one) => std::slice::from_ref(one),
                    None => &sprite.sprites,
                }
            }
            None => &sprite.sprites,
        };
        let faces = sprites
            .iter()
            .map(|one| facing(one, stand, right, up))
            .collect();
        let shape = Shape {
            lines: Vec::new(),
            faces,
            billboards: Vec::new(),
            state_words: BTreeSet::new(),
        };
        let _ = emit(&shape, stand, &self.layers, out, usize::MAX);
    }
}

/// The hardpoints whose loaded count `shape` reads to draw its rails,
/// ascending; empty for a shape that draws no load.
pub(super) fn rail_hardpoints(shape: &Shape) -> Vec<u8> {
    shape
        .state_words
        .iter()
        .filter_map(|word| word.checked_sub(LOADED_COUNT_BASE))
        .filter_map(|hp| u8::try_from(hp).ok())
        .collect()
}

#[cfg(test)]
impl SurfaceArt {
    /// A wreck for `id`, as the build makes one.
    pub(super) fn with_wreck(mut self, id: u32, drawn: Drawn) -> Self {
        self.wrecks.insert(id, drawn);
        self
    }
    /// A unit with spinning antennas whose aim has been stepped once: it
    /// is drawn by the frame pass, never by the static batch.
    pub(super) fn with_spinning_antenna(mut self, id: u32) -> Self {
        self.aimers.insert(
            id,
            Aimer {
                stand: Stand {
                    scale: 1.,
                    basis: Basis::new(0., 0., 0.),
                    origin: [0.; 3],
                },
                bytes: Vec::new(),
                intact: Shape {
                    lines: Vec::new(),
                    faces: Vec::new(),
                    billboards: Vec::new(),
                    state_words: BTreeSet::new(),
                },
                turrets: Vec::new(),
                antennas: true,
                rails: Vec::new(),
                full: Vec::new(),
                routed: false,
            },
        );
        self.aims.tick = Some(0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_formats::shape::{Face, FogMode};
    use tore_world::surface::{MountStock, SurfacePose, SurfaceUnitState};

    fn triangle(at: f32) -> Shape {
        Shape {
            lines: Vec::new(),
            faces: vec![Face {
                fog: FogMode::Enabled,
                positions: vec![[at, 0., 0.], [at + 1., 0., 0.], [at, 1., 0.]],
                colors: vec![7; 3],
                uv: Vec::new(),
                texture: String::new(),
                subtype: 0,
                normal: None,
                address: 0,
            }],
            billboards: Vec::new(),
            state_words: BTreeSet::new(),
        }
    }

    fn level(origin: Vector) -> Stand {
        Stand {
            scale: 2.,
            basis: Basis::new(0., 0., 0.),
            origin,
        }
    }

    fn positions(vertices: &[f32]) -> Vec<[f32; 3]> {
        vertices
            .chunks_exact(FLOATS)
            .map(|v| [v[0], v[1], v[2]])
            .collect()
    }

    #[test]
    fn raised_bridge_shadow_samples_the_ground_between_its_corners() {
        let terrain = tore_world::test_support::terrain();
        let mut drawn = Drawn::default();
        for at in [
            [1000., 2000., 1000.],
            [2000., 2000., 1000.],
            [1000., 2000., 2000.],
        ] {
            let mut v = vertex(at, [0.; 2], -1.);
            v[5] = SHADOW_LAYER;
            drawn.vertices.extend(v);
        }
        ground_bridge_shadows(&mut drawn, &terrain, BUDGET).unwrap();
        assert!(drawn.vertices.len() > 3 * FLOATS);
        for v in drawn.vertices.chunks_exact(FLOATS) {
            assert!(v[1] <= terrain.height(v[0], v[2]).max(0.) + 1.001);
        }
        assert!(ground_bridge_shadows(&mut drawn, &terrain, 1).is_err());
    }

    #[test]
    fn a_ground_shadow_decal_is_drawn_dark_for_the_dithered_pass() {
        // The suspension bridge's span on the water and the crane's
        // footprint are solid colour 6 quads on the lowest plane: a magenta
        // patch when drawn as a colour (nr-fx2).
        let quad = |z: f32, color: u8, subtype: u8| {
            let mut face = triangle(0.).faces[0].clone();
            face.positions = vec![[0., 0., z], [4., 0., z], [4., 9., z], [0., 9., z]];
            face.colors = vec![color; 4];
            face.subtype = subtype;
            face
        };
        let shape = |faces: Vec<Face>| Shape {
            faces,
            ..triangle(0.)
        };
        let drawn = |shape: &Shape| {
            let mut out = Drawn::default();
            emit(shape, &level([0.; 3]), &Layers::new(), &mut out, usize::MAX).unwrap();
            out.vertices.len() / FLOATS
        };
        // A deck at 10 over a floor at -60 (both ordinary faces).
        let base = vec![quad(10., 124, 0), quad(-60., 124, 0)];
        let with = |extra: Face| {
            let mut faces = base.clone();
            faces.push(extra);
            drawn(&shape(faces))
        };
        assert_eq!(drawn(&shape(base.clone())), 12);
        // The decal (colour 6, solid, flat on the lowest plane) is drawn as a
        // dark shadow marked for the dithered pass, not in the palette's
        // magenta; anything else is an ordinary face.
        let layers_of = |extra: Face| {
            let mut faces = base.clone();
            faces.push(extra);
            let mut out = Drawn::default();
            emit(
                &shape(faces),
                &level([0.; 3]),
                &Layers::new(),
                &mut out,
                usize::MAX,
            )
            .unwrap();
            out.vertices
                .chunks_exact(FLOATS)
                .filter(|v| v[5] == SHADOW_LAYER)
                .count()
        };
        assert_eq!(with(quad(-60., 6, 0x41)), 18);
        assert_eq!(layers_of(quad(-60., 6, 0x41)), 6);
        assert_eq!(layers_of(quad(-60.5, 6, 0x41)), 6);
        // The same colour elsewhere, another colour, or another record kind.
        assert_eq!(layers_of(quad(0., 6, 0x41)), 0);
        assert_eq!(layers_of(quad(-60., 7, 0x41)), 0);
        assert_eq!(layers_of(quad(-60., 6, 0x44)), 0);
    }

    #[test]
    fn a_shape_is_placed_by_its_scale_basis_and_origin() {
        let mut out = Drawn::default();
        emit(
            &triangle(0.),
            &level([100., 10., -5.]),
            &Layers::new(),
            &mut out,
            BUDGET,
        )
        .unwrap();
        // Shape axes right, forward, up: the second corner is 2 ft east,
        // the third 2 ft north, at the palette colour without a texture.
        assert_eq!(
            positions(&out.vertices),
            [[100., 10., -5.], [102., 10., -5.], [100., 10., -3.]]
        );
        assert!(
            out.vertices
                .chunks_exact(FLOATS)
                .all(|v| v[5] == -1. && v[9] == 7.)
        );
        let mut small = Drawn::default();
        assert!(
            emit(
                &triangle(0.),
                &level([0.; 3]),
                &Layers::new(),
                &mut small,
                20
            )
            .is_err()
        );
    }

    /// A 20 ft cube standing on its origin, its faces textured nothing.
    fn cube() -> Shape {
        let corners = |pick: [[f32; 3]; 4]| Face {
            fog: FogMode::Enabled,
            positions: pick.to_vec(),
            colors: vec![7; 4],
            uv: Vec::new(),
            texture: String::new(),
            subtype: 0,
            normal: None,
            address: 0,
        };
        let (l, h) = (0., 20.);
        Shape {
            lines: Vec::new(),
            faces: vec![
                corners([[l, l, l], [h, l, l], [h, h, l], [l, h, l]]),
                corners([[l, l, h], [h, l, h], [h, h, h], [l, h, h]]),
                corners([[l, l, l], [h, l, l], [h, l, h], [l, l, h]]),
                corners([[l, h, l], [h, h, l], [h, h, h], [l, h, h]]),
                corners([[l, l, l], [l, h, l], [l, h, h], [l, l, h]]),
                corners([[h, l, l], [h, h, l], [h, h, h], [h, l, h]]),
            ],
            billboards: Vec::new(),
            state_words: BTreeSet::new(),
        }
    }

    #[test]
    fn a_destroyed_building_never_vanishes_and_pavement_stays_as_it_stood() {
        // B16: a building with no `~` variant and no retail damage branch
        // falls into a generated ruin, lower than it stood; a road keeps
        // its look; neither hides.
        let terrain = tore_world::test_support::terrain();
        let resources = BTreeMap::new();
        let definitions = BTreeMap::new();
        let mut pages = Vec::new();
        let mut build = Build {
            resources: &resources,
            terrain: &terrain,
            definitions: &definitions,
            first_page: 0,
            pages: &mut pages,
        };
        let mut art = SurfaceArt::default();
        let stand = level([100., 0., 100.]);
        let intact = cube();
        for (id, kind) in [(0x4000_0001, "HANGR.OT"), (0x4000_0002, "ROAD.OT")] {
            art.add(&mut build, id, kind, &intact, &stand, 0.).unwrap();
        }
        let hangar = art.look(0x4000_0001, false, true, None);
        assert_eq!(hangar, Some(Look::Wreck));
        let ruin = art.drawn(0x4000_0001, &Look::Wreck).unwrap();
        assert!(!ruin.vertices.is_empty());
        let top = positions(&ruin.vertices)
            .iter()
            .map(|p| p[1])
            .fold(f32::MIN, f32::max);
        // The cube stood 40 ft high (scale 2); its ruin is at most 30
        // percent of that.
        assert!(
            top <= 40. * (super::super::rubble::KEEP[0] + super::super::rubble::KEEP[1]) + 1e-3,
            "{top}"
        );
        // Some faces are charred: drawn in a colour, not the palette.
        assert!(
            ruin.vertices
                .chunks_exact(FLOATS)
                .any(|v| v[9] == -1. && v[6] > 0.)
        );
        // The road is kept as it stood (its intact look).
        assert_eq!(art.look(0x4000_0002, false, true, None), None);
        // Nothing else changed: a living building keeps its intact look.
        assert_eq!(art.look(0x4000_0001, true, true, None), None);
    }

    #[test]
    fn all_four_bridge_ends_collapse_deterministically_from_hp_and_replay_restore() {
        let terrain = tore_world::test_support::terrain();
        let resources = BTreeMap::new();
        let definitions = BTreeMap::new();
        let mut pages = Vec::new();
        let mut build = Build {
            resources: &resources,
            terrain: &terrain,
            definitions: &definitions,
            first_page: 0,
            pages: &mut pages,
        };
        let stand = level([100., 200., 300.]);
        let shape = cube();
        for kind in ["BR1END.OT", "BR2END.OT", "BR3END.OT", "BRDEND.OT"] {
            let mut a = SurfaceArt::default();
            let mut b = SurfaceArt::default();
            a.add(&mut build, 7, kind, &shape, &stand, 0.).unwrap();
            b.add(&mut build, 7, kind, &shape, &stand, 0.).unwrap();
            let ruin = a.drawn(7, &Look::Wreck).unwrap();
            assert_eq!(ruin.vertices, b.drawn(7, &Look::Wreck).unwrap().vertices);
            let top = positions(&ruin.vertices)
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(top > 200. && top <= 212., "{kind}: {top}");
            for hp in [500, 250, 1, 0, 1, 0, 500] {
                assert_eq!(
                    a.look(7, hp > 0, true, None),
                    if hp > 0 { None } else { Some(Look::Wreck) }
                );
            }
            assert_eq!(
                tore_world::surface::wreck::object_fire(kind, false),
                tore_world::surface::wreck::FireClass::None
            );
        }
    }

    #[test]
    fn bridge_damage_and_replay_restore_preserve_the_placed_datum() {
        let mut fallen = cube();
        for face in &mut fallen.faces {
            for p in &mut face.positions {
                p[2] -= 10.;
            }
        }
        let stand = level([100., 200., 300.]);
        let mut art = SurfaceArt::default();
        let drawn = art
            .ruin_drawn(&Ruined::Branch(fallen), &stand, 40., false)
            .unwrap();
        let low = positions(&drawn.vertices)
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min);
        // A fallen span is allowed below the intact datum. Regrounding its
        // new lowest point would incorrectly raise it back into the road.
        assert!((low - 180.).abs() < 1e-5);
        art.wrecks.insert(7, drawn);
        for hp in [500, 250, 1, 0, 250, 0, 500] {
            assert_eq!(
                art.look(7, hp > 0, true, None),
                if hp > 0 { None } else { Some(Look::Wreck) }
            );
        }
    }

    #[test]
    fn a_destroyed_runway_stays_drawn() {
        // B15: a runway at zero hit points keeps its pavement (its intact
        // look), whether or not combat registered it; any other placement
        // with no wreck hides.
        let mut art = SurfaceArt::default();
        art.keep(7);
        assert_eq!(art.look(7, false, true, None), None);
        assert_eq!(art.look(7, false, false, None), None);
        assert_eq!(art.look(7, true, true, None), None);
        assert_eq!(art.look(8, false, true, None), Some(Look::Hidden));
        // A runway with a wreck of its own would show it.
        let mut art = SurfaceArt::default().with_wreck(7, Drawn::default());
        art.keep(7);
        assert_eq!(art.look(7, false, true, None), Some(Look::Wreck));
    }

    #[test]
    fn a_placement_shows_its_wreck_its_rails_or_nothing() {
        let mut art = SurfaceArt::default().with_wreck(1, Drawn::default());
        art.launchers.insert(
            2,
            Launcher {
                bytes: Vec::new(),
                stand: level([0.; 3]),
                hardpoints: vec![0, 2],
                full: vec![2, 1],
            },
        );
        // Destroyed: its wreck if it keeps one, else hidden; a placement
        // combat never registered hides.
        assert_eq!(art.look(1, false, true, None), Some(Look::Wreck));
        assert_eq!(art.look(1, false, false, None), Some(Look::Hidden));
        assert_eq!(art.look(3, false, true, None), Some(Look::Hidden));
        assert_eq!(art.look(3, true, true, None), None);
        // A launcher at full load, or not armed yet, keeps its scenery look;
        // with rounds gone its rails show what is left.
        let mut unit = SurfaceUnitState::new(UnitId(2));
        let state = |unit: &SurfaceUnitState| SurfaceState::new(0, vec![unit.clone()]);
        assert_eq!(art.look(2, true, true, Some(&state(&unit))), None);
        unit.armed = true;
        unit.mounts = [2, 0, 1]
            .map(|loaded| MountStock {
                loaded,
                reserve: None,
                ordinal: 0,
            })
            .to_vec();
        assert_eq!(art.look(2, true, true, Some(&state(&unit))), None);
        unit.mounts[0].loaded = 1;
        assert_eq!(
            art.look(2, true, true, Some(&state(&unit))),
            Some(Look::Rails(vec![1, 1]))
        );
        unit.mounts[2].loaded = 0;
        assert_eq!(
            art.look(2, true, true, Some(&state(&unit))),
            Some(Look::Rails(vec![1, 0]))
        );
        // A replay records no loads: the scenery look.
        assert_eq!(art.look(2, true, true, None), None);
    }

    #[test]
    fn a_moving_unit_draws_where_it_is_on_its_lowest_point_and_as_a_wreck() {
        let mut art = SurfaceArt::default();
        art.movers.insert(
            5,
            Mover {
                intact: Model {
                    shape: triangle(0.),
                    scale: 1.,
                },
                wreck: Some(Model {
                    shape: triangle(10.),
                    scale: 1.,
                }),
                lift: 3.,
                wreck_lift: 0.,
            },
        );
        art.movers.insert(
            6,
            Mover {
                intact: Model {
                    shape: triangle(0.),
                    scale: 1.,
                },
                wreck: None,
                lift: 0.,
                wreck_lift: 0.,
            },
        );
        let pose = |id: u32, wrecked: bool| SurfacePose {
            id: UnitId(id),
            position: [1000., 50., 2000.],
            attitude: [0.; 3],
            shape: None,
            wrecked,
        };
        let camera = Camera::new();
        let draw = |poses: Vec<SurfacePose>| {
            let picture = crate::snapshot::RenderSnapshot {
                surface: poses,
                ..Default::default()
            };
            positions(&art.frame(&picture, &|_| true, &camera, [1000., 0., 2000.], 0.))
        };
        // Relative to the render origin, lifted onto its lowest point.
        assert_eq!(draw(vec![pose(5, false)])[0], [0., 53., 0.]);
        // Destroyed: its wreck, lying on the ground; with none, nothing.
        assert_eq!(draw(vec![pose(5, true)])[0], [10., 50., 0.]);
        assert!(draw(vec![pose(6, true)]).is_empty());
        assert_eq!(draw(vec![pose(6, false)]).len(), 3);
    }

    /// docs/spec/surface-defenses.md, "Aiming": a land unit with no turret
    /// turns as a whole toward what its weapon holds, leaving the static
    /// batch for the frame's, and keeps its aim once nothing is held.
    #[test]
    fn an_aiming_unit_turns_its_body_and_is_drawn_every_frame() {
        use tore_world::surface::aim::{Rule, Track, Weapon};
        let mut art = SurfaceArt::default();
        art.aimers.insert(
            7,
            Aimer {
                stand: level([1000., 50., 2000.]),
                bytes: Vec::new(),
                intact: triangle(0.),
                turrets: Vec::new(),
                antennas: false,
                rails: Vec::new(),
                full: Vec::new(),
                routed: false,
            },
        );
        art.rules.insert(UnitId(7), Rule::Body { rate_deg_s: 90. });
        let mut surface = Surface::default();
        surface.arsenal.units.push(tore_world::surface::fire::Arms {
            unit: UnitId(7),
            position: [1000., 50., 2000.],
            heading: 0.,
            skill: 1,
            side: tore_sim::combat::live::Side(2),
            react: 0,
            search_limit: None,
            ship: false,
            weapons: Vec::new(),
            radar: None,
            battery: None,
            loads: Vec::new(),
            npc: [0; 4],
            team: false,
        });
        let camera = Camera::new();
        // A jet due east of it.
        let picture = |tick: u64| crate::snapshot::RenderSnapshot {
            tick,
            targets: vec![crate::snapshot::AircraftPose {
                id: 3,
                position: [9000., 3000., 2000.],
                ..Default::default()
            }],
            ..Default::default()
        };
        let held = [Track {
            unit: UnitId(7),
            weapon: Weapon::Station(0),
            target: 3,
        }];
        let frame = |art: &SurfaceArt, standing: bool| {
            positions(&art.frame(&picture(0), &|_| standing, &camera, [1000., 0., 2000.], 0.))
        };
        // Before aiming begins, and at rest, it stays in the static batch.
        assert_eq!(art.look(7, true, true, None), None);
        art.step_aim(&picture(0), &surface, &[], None);
        assert_eq!(art.look(7, true, true, None), None);
        assert!(frame(&art, true).is_empty());
        // Held: a quarter turn right at 90 degrees a second takes a second.
        for tick in 1..=60 {
            art.step_aim(&picture(tick), &surface, &held, None);
        }
        assert!((art.aim_of(7).unwrap().body.to_degrees() - 45.).abs() < 1e-6);
        for tick in 61..=200 {
            art.step_aim(&picture(tick), &surface, &held, None);
        }
        assert_eq!(art.look(7, true, true, None), Some(Look::Aimed));
        assert_eq!(art.drawn(7, &Look::Aimed).map(|d| d.vertices.len()), None);
        // Its nose (shape forward) now points east, its right side south.
        let turned = frame(&art, true);
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4);
        assert!(close(turned[0], [0., 50., 0.]), "{turned:?}");
        assert!(close(turned[1], [0., 50., -2.]), "{turned:?}");
        assert!(close(turned[2], [2., 50., 0.]), "{turned:?}");
        // Nothing held: it keeps that aim. Destroyed: it is not drawn.
        art.step_aim(&picture(201), &surface, &[], None);
        assert_eq!(art.look(7, true, true, None), Some(Look::Aimed));
        assert!(frame(&art, false).is_empty());
        assert_eq!(art.look(7, false, true, None), Some(Look::Hidden));
    }

    /// A launcher whose body turned and whose rails are not full is
    /// projected once when its loads change, not on every frame.
    #[test]
    fn a_turned_launchers_rails_are_projected_when_the_loads_change() {
        let mut art = SurfaceArt::default();
        art.aimers.insert(
            4,
            Aimer {
                stand: level([0.; 3]),
                bytes: Vec::new(),
                intact: triangle(0.),
                turrets: Vec::new(),
                antennas: false,
                rails: vec![0],
                full: vec![2],
                routed: false,
            },
        );
        let surface = Surface::default();
        let picture = crate::snapshot::RenderSnapshot::default();
        let mut unit = SurfaceUnitState::new(UnitId(4));
        unit.armed = true;
        let mut stock = |loaded: u32| {
            unit.mounts = vec![MountStock {
                loaded,
                reserve: None,
                ordinal: 0,
            }];
            SurfaceState::new(0, vec![unit.clone()])
        };
        // Full: nothing is projected or kept.
        art.step_aim(&picture, &surface, &[], Some(&stock(2)));
        assert!(art.rail_shapes.is_empty() && art.aimer_loads.is_empty());
        // One round gone: projected once and kept (here the empty test
        // bytes cannot project, which the entry records as the intact look).
        art.step_aim(&picture, &surface, &[], Some(&stock(1)));
        assert_eq!(art.aimer_loads.get(&4), Some(&vec![1]));
        assert!(art.rail_shapes.contains_key(&4));
        art.rail_shapes.insert(4, Some(triangle(5.)));
        // The same loads on the next step keep the entry as it was.
        art.step_aim(&picture, &surface, &[], Some(&stock(1)));
        assert!(art.rail_shapes[&4].is_some());
        let aimer = &art.aimers[&4];
        let (shape, _) = art.aimed_shape(4, aimer, 0.);
        assert!(matches!(shape, Some(Projected::Cached(_))));
        // New loads project again; a reload drops the entry.
        art.step_aim(&picture, &surface, &[], Some(&stock(0)));
        assert!(art.rail_shapes[&4].is_none());
        art.step_aim(&picture, &surface, &[], Some(&stock(2)));
        assert!(art.rail_shapes.is_empty());
    }

    #[test]
    fn a_man_faces_the_viewer_and_goes_with_his_owner() {
        let sprite = Billboard {
            center: [0., 0., 6.],
            size: [7., 12.],
            texture: String::new(),
            uv: None,
            fog: FogMode::Enabled,
            address: 0,
        };
        let mut art = SurfaceArt::default();
        art.sprites.push(Sprite {
            owner: 9,
            stand: Stand {
                scale: 1. / 3.,
                basis: Basis::new(1.2, 0., 0.),
                origin: [0.; 3],
            },
            sprites: vec![sprite],
            crew: None,
        });
        let mut camera = Camera::new();
        camera.position = [0., 2., -100.];
        camera.yaw = 0.;
        camera.pitch = 0.;
        let picture = crate::snapshot::RenderSnapshot::default();
        let quad = positions(&art.frame(&picture, &|_| true, &camera, [0.; 3], 0.));
        assert_eq!(quad.len(), 6);
        // Seen from the south, whatever its heading: the quad lies across
        // the view (constant z), 4 ft tall from its feet, 7/3 ft wide.
        assert!(quad.iter().all(|p| p[2].abs() < 1e-4), "{quad:?}");
        let ys: Vec<f32> = quad.iter().map(|p| p[1]).collect();
        let xs: Vec<f32> = quad.iter().map(|p| p[0]).collect();
        let span = |v: &[f32]| {
            v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!((span(&ys) - 4.).abs() < 1e-4 && (span(&xs) - 7. / 3.).abs() < 1e-4);
        assert!(
            art.frame(&picture, &|id| id != 9, &camera, [0.; 3], 0.)
                .is_empty()
        );
    }

    #[test]
    fn a_man_stays_visible_at_a_pilots_range() {
        // Life size to 1,000 ft, then larger by the distance, to three times.
        assert_eq!(man_boost(300.), 1.);
        assert_eq!(man_boost(1_000.), 1.);
        assert_eq!(man_boost(3_000.), 3.);
        assert_eq!(man_boost(50_000.), MAN_MAX_BOOST);
    }

    #[test]
    fn a_far_man_grows_up_from_his_feet() {
        // A man sprite 12 units tall centred on his origin, stood on his
        // feet: his origin 2 ft above the ground at 0.
        let sprite = Billboard {
            center: [0., 0., 0.],
            size: [7., 12.],
            texture: String::new(),
            uv: None,
            fog: FogMode::Enabled,
            address: 0,
        };
        let man = Shape {
            lines: Vec::new(),
            faces: Vec::new(),
            billboards: vec![sprite],
            state_words: BTreeSet::new(),
        };
        let stand = Stand {
            scale: 1. / 3.,
            basis: Basis::new(0.4, 0., 0.),
            origin: [0., 2., 0.],
        };
        // Near, at his own size, and far, three times larger: his feet stay
        // on the ground and only his head rises.
        let (faces, sprites) = drawn_bottoms(&man, &stand, &[300., 6_000., 50_000.], true);
        assert_eq!(faces, None);
        for bottom in sprites {
            assert!(bottom.is_some_and(|y| y.abs() < 1e-4), "{bottom:?}");
        }
        let far = man_stand(&man.billboards, &stand, [6_000., 2., 0.]);
        assert!((far.scale - 1.).abs() < 1e-12);
        assert!((far.origin[1] - 6.).abs() < 1e-9, "{:?}", far.origin);
        // A man drawn as cards (faces) grows from his feet the same way.
        let mut card = triangle(0.);
        card.faces[0].positions = vec![[0., 0., -6.], [3., 0., -6.], [0., 0., 6.]];
        let (faces, men) = drawn_bottoms(&card, &stand, &[300., 6_000.], true);
        assert!(faces.is_some_and(|y| y.abs() < 1e-4), "{faces:?}");
        for bottom in men {
            assert!(bottom.is_some_and(|y| y.abs() < 1e-4), "{bottom:?}");
        }
    }

    #[test]
    fn a_destroyed_parked_aircraft_draws_its_own_shape_as_a_hulk() {
        let mut art = SurfaceArt::default();
        let stand = Stand {
            scale: 1.,
            basis: Basis::new(0., 0., 0.),
            origin: [100., 0., 200.],
        };
        // A broad low shape: some faces, 12 ft tall.
        let mut plane = triangle(0.);
        plane.faces = (0..40)
            .map(|n| {
                let at = n as f32 * 2.;
                let mut face = plane.faces[0].clone();
                face.positions = vec![[at, 0., 0.], [at + 2., 8., 0.], [at + 1., 4., 12.]];
                face.colors = vec![3; 3];
                face
            })
            .collect();
        art.add_parked(0x5000_0008, &plane, &stand);
        // Alive it is its standing look (none here: the placement itself);
        // destroyed it shows the hulk, never nothing.
        assert_eq!(art.look(0x5000_0008, true, true, None), None);
        assert_eq!(art.look(0x5000_0008, false, true, None), Some(Look::Wreck));
        let hulk = art.drawn(0x5000_0008, &Look::Wreck).unwrap();
        assert!(!hulk.vertices.is_empty());
        // Low, and standing where the aircraft stood: its lowest point stays
        // at the placement's, nothing above 60 percent of 12 ft.
        let ys: Vec<f32> = positions(&hulk.vertices).iter().map(|p| p[1]).collect();
        assert!(
            ys.iter().all(|y| *y >= -1e-3 && *y <= 12. * 0.6 + 1e-3),
            "{ys:?}"
        );
    }
}
