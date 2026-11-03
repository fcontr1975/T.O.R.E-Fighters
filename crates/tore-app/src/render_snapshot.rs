//! Draws one tick's picture: the vertices for the other aircraft, debris,
//! weapons, tracers, ejected pilots and afterburner lights of a
//! [`RenderSnapshot`].
//!
//! The snapshot itself, and the blend of two of them at a frame's tick
//! fraction, are plain data in `snapshot.rs`. Live flight draws the blend of
//! its last two snapshots with [`aircraft_batches`] and [`combat_geometry`]; a
//! mission replay draws decoded snapshots through the same helpers, so both
//! show the same picture.
use crate::{
    AppResult,
    aircraft::Airframe,
    camera::Camera,
    flight,
    scenery::Scenery,
    sim_renderer::{CombatGeometry, Contact},
    snapshot::{AircraftPose, DebrisPose, Draw, RenderSnapshot, set_devices, wreck_in},
    terrain::Terrain,
};
use std::collections::BTreeMap;
use tore_formats::{Pic, aircraft::AircraftId, shape::Shape};
use tore_sim::{
    attitude::{Basis, Vector, cross, unit},
    combat::live,
};

/// One aircraft's lit afterburner as lights: one in each engine's flame,
/// behind its outlet in `offsets` (right, up and forward from the aircraft's
/// reference point, in feet), sharing the aircraft's strength
/// (docs/spec/engine-material.md#afterburner-glow).
pub fn afterburner_glow(
    position: Vector,
    [yaw, pitch, bank]: [f64; 3],
    offsets: &[Vector],
) -> Vec<crate::countermeasure_renderer::Afterburner> {
    use crate::countermeasure_renderer::{AFTERBURNER_BEHIND_FEET, AFTERBURNER_SHARE, Afterburner};
    let basis = Basis::new(yaw, pitch, bank);
    let share = AFTERBURNER_SHARE / offsets.len().max(1) as f64;
    offsets
        .iter()
        .map(|o| Afterburner {
            position: std::array::from_fn(|i| {
                position[i]
                    + basis.right[i] * o[0]
                    + basis.up[i] * o[1]
                    + basis.forward[i] * (o[2] - AFTERBURNER_BEHIND_FEET)
            }),
            share,
        })
        .collect()
}

/// Where an aircraft's engine outlets sit, for its flame lights: its own
/// model's, from `model_outlets` (parallel to `models`), when a model is
/// loaded for its type and that is not the player's type; otherwise the
/// player's airframe's.
pub fn engine_outlets<'a>(
    aircraft: Option<AircraftId>,
    player: AircraftId,
    models: &[Airframe],
    model_outlets: &'a [Vec<Vector>],
    player_outlets: &'a [Vector],
) -> &'a [Vector] {
    aircraft
        .filter(|id| *id != player)
        .and_then(|id| models.iter().position(|model| model.profile.id == id))
        .and_then(|index| model_outlets.get(index))
        .map_or(player_outlets, Vec::as_slice)
}

/// The lit afterburners of the aircraft in `snapshot` other than the player,
/// in target order, at their poses. `offsets` gives an aircraft's engine
/// outlets.
pub fn target_glows<'a>(
    snapshot: &RenderSnapshot,
    offsets: impl Fn(&AircraftPose) -> &'a [Vector],
) -> Vec<crate::countermeasure_renderer::Afterburner> {
    snapshot
        .targets
        .iter()
        .filter(|pose| pose.engine.flame)
        .flat_map(|pose| afterburner_glow(pose.position, pose.attitude, offsets(pose)))
        .collect()
}

/// A target drawn with its own model, over the model's start state.
fn model_pose(mut s: flight::State, pose: &AircraftPose, tick: u64) -> flight::State {
    s.ticks = tick;
    s.engine = pose.engine.lit;
    s.burner = pose.engine.afterburner;
    s.position = pose.position;
    s.damage_fraction = pose.damage.fraction();
    s.damage_variant = pose.damage.variant();
    s.damage_regions = pose.damage.regions();
    [s.yaw, s.pitch, s.bank] = pose.attitude;
    s.gear = 0.;
    s.flaps = 0.;
    s.exhaust = 0.;
    s.bay = 0.;
    if let Some(devices) = pose.devices {
        set_devices(&mut s, devices);
    }
    // A rotorcraft's blades turn at its rotor speed and its disks tilt as it
    // flies them (slice P7b).
    if pose.engine.rotor > 0. {
        crate::snapshot::set_rotor(&mut s, &pose.engine);
    }
    s
}

/// A straight-flight fixture drawn with the player's airframe over the
/// player's presented state, which supplies bay, speed and throttle. The
/// pose's resolved engine decides the nozzle heat, so a replay's rebuilt
/// player state draws fixtures exactly as the live one does.
fn ownship_pose(template: &flight::State, pose: &AircraftPose) -> flight::State {
    let mut s = template.clone();
    s.wreck = pose.wreck.map(wreck_in);
    s.crashed = pose.damage.hp <= 0;
    s.engine = pose.engine.lit;
    s.burner = pose.engine.afterburner;
    s.position = pose.position;
    s.damage_fraction = pose.damage.fraction();
    s.damage_variant = pose.damage.variant();
    s.damage_regions = pose.damage.regions();
    [s.yaw, s.pitch, s.bank] = pose.attitude;
    s.exhaust = 0.;
    s.gear = 0.;
    s.flaps = 0.;
    s.elevator = 0.;
    s.aileron = 0.;
    s.rudder = 0.;
    s.brake = 0.;
    s.hook = 0.;
    if let Some(devices) = pose.devices {
        set_devices(&mut s, devices);
    }
    s
}

/// Per-model vertices for the aircraft drawn with their own models: one batch
/// per `snapshot.models` entry found in `models`, with each airborne
/// aircraft's vertex range for the spotting aid, then that model's debris.
pub fn aircraft_batches<'a>(
    snapshot: &RenderSnapshot,
    models: &'a [Airframe],
    art: &CombatArt,
    camera: &Camera,
    world: &Terrain,
    scenery: &Scenery,
) -> Vec<(&'a Airframe, Vec<f32>, Vec<Contact>)> {
    let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
    aircraft_batches_with(
        tore_workers::shared(),
        snapshot,
        models,
        art,
        camera,
        world,
        scenery,
    )
}

/// One aircraft drawn with its own model: the airframe, then the pylons and
/// stores it carries (B3).
fn target_vertices(
    model: &Airframe,
    pose: &flight::State,
    target: &AircraftPose,
    art: &CombatArt,
    camera: &Camera,
    world: &Terrain,
    scenery: &Scenery,
) -> Vec<f32> {
    let mut vertices = model.vertices(pose, camera, world, scenery);
    if !pose.wreck_gone() {
        crate::pylons::vertices(
            &mut vertices,
            crate::pylons::visible(&target.stores, pose.bay),
            scenery.relative(pose.position),
            [pose.yaw, pose.pitch, pose.bank],
            Some(model),
            art,
            &model.palette,
        );
    }
    vertices
}

// Fitted dispatch threshold: preserve the inline path for small formations.
// The synthetic wall-time probe and integrated frame runs measure its cost.
const MIN_AIRCRAFT_JOBS: usize = 4;

enum AircraftJob<'a> {
    Target(&'a AircraftPose),
    Debris(&'a DebrisPose),
}

fn aircraft_batches_with<'a>(
    workers: &tore_workers::Executor,
    snapshot: &RenderSnapshot,
    models: &'a [Airframe],
    art: &CombatArt,
    camera: &Camera,
    world: &Terrain,
    scenery: &Scenery,
) -> Vec<(&'a Airframe, Vec<f32>, Vec<Contact>)> {
    let selected = || {
        snapshot
            .models
            .iter()
            .filter_map(|id| models.iter().find(|model| model.profile.id == *id))
    };
    let visible = |target: &&AircraftPose, draw| {
        target.draw == draw && target.airborne && Some(target.id) != camera.hidden_target
    };
    let count = selected()
        .map(|model| {
            let draw = Draw::Model(model.profile.id);
            snapshot.targets.iter().filter(|t| visible(t, draw)).count()
                + snapshot.debris.iter().filter(|p| p.draw == draw).count()
        })
        .sum();
    if !workers.should_dispatch(count, MIN_AIRCRAFT_JOBS) {
        return aircraft_batches_serial(snapshot, models, art, camera, world, scenery);
    }

    let selected: Vec<_> = selected().collect();
    let mut jobs = Vec::with_capacity(count);
    for (batch, model) in selected.iter().enumerate() {
        let draw = Draw::Model(model.profile.id);
        jobs.extend(
            snapshot
                .targets
                .iter()
                .filter(|t| visible(t, draw))
                .map(|target| (batch, AircraftJob::Target(target))),
        );
        jobs.extend(
            snapshot
                .debris
                .iter()
                .filter(|piece| piece.draw == draw)
                .map(|piece| (batch, AircraftJob::Debris(piece))),
        );
    }
    // Each job is one aircraft or detached piece, including formations in
    // which every aircraft uses the same model. No GPU resource crosses here.
    let geometry = workers.ordered_map(&jobs, MIN_AIRCRAFT_JOBS, |_, (batch, job)| {
        let model = selected[*batch];
        match job {
            AircraftJob::Target(target) => {
                let pose = model_pose(model.start(world), target, snapshot.tick);
                target_vertices(model, &pose, target, art, camera, world, scenery)
            }
            AircraftJob::Debris(piece) => {
                let mut pose = model.start(world);
                pose.position = piece.position;
                pose.damage_variant = piece.variant;
                [pose.yaw, pose.pitch, pose.bank] = piece.attitude;
                model.fragment_vertices(&pose, camera, world, scenery)
            }
        }
    });
    let mut sizes = vec![0; selected.len()];
    for ((batch, _), vertices) in jobs.iter().zip(&geometry) {
        sizes[*batch] += vertices.len();
    }
    let extents: Vec<_> = selected.iter().map(|model| model.visual_extent()).collect();
    let mut batches: Vec<_> = selected
        .into_iter()
        .zip(sizes)
        .map(|(model, size)| (model, Vec::with_capacity(size), Vec::new()))
        .collect();
    // Completion order never decides draw order. Contacts use the offset in
    // the final model batch, and debris never gains a spotting-aid contact.
    for ((batch, job), vertices) in jobs.into_iter().zip(geometry) {
        let (_, merged, contacts) = &mut batches[batch];
        let first = merged.len() / 10;
        merged.extend(vertices);
        if let AircraftJob::Target(target) = job {
            contacts.extend(Contact::new(
                first,
                merged.len() / 10,
                scenery.relative(target.position),
                extents[batch],
            ));
        }
    }
    batches
}

/// Pre-threading ordered builder, retained for the inline path and as an independent
/// reference for worker output. It constructs no parallel staging buffers.
fn aircraft_batches_serial<'a>(
    snapshot: &RenderSnapshot,
    models: &'a [Airframe],
    art: &CombatArt,
    camera: &Camera,
    world: &Terrain,
    scenery: &Scenery,
) -> Vec<(&'a Airframe, Vec<f32>, Vec<Contact>)> {
    snapshot
        .models
        .iter()
        .filter_map(|id| models.iter().find(|model| model.profile.id == *id))
        .map(|model| {
            let draw = Draw::Model(model.profile.id);
            let mut vertices = Vec::new();
            let mut contacts = Vec::new();
            let extent = model.visual_extent();
            for target in snapshot
                .targets
                .iter()
                .filter(|t| t.draw == draw && t.airborne && Some(t.id) != camera.hidden_target)
            {
                let pose = model_pose(model.start(world), target, snapshot.tick);
                let first = vertices.len() / 10;
                vertices.extend(target_vertices(
                    model, &pose, target, art, camera, world, scenery,
                ));
                contacts.extend(Contact::new(
                    first,
                    vertices.len() / 10,
                    scenery.relative(pose.position),
                    extent,
                ));
            }
            for piece in snapshot.debris.iter().filter(|p| p.draw == draw) {
                let mut pose = model.start(world);
                pose.position = piece.position;
                pose.damage_variant = piece.variant;
                [pose.yaw, pose.pitch, pose.bank] = piece.attitude;
                vertices.extend(model.fragment_vertices(&pose, camera, world, scenery));
            }
            (model, vertices, contacts)
        })
        .collect()
}

/// The objects combat holds standing (hit points left), for the surface
/// drawing: a man or a deck crew disappears with its owner.
pub fn standing(targets: &[live::Target]) -> std::collections::BTreeSet<u32> {
    targets
        .iter()
        .filter(|target| target.hp > 0)
        .map(|target| target.id)
        .collect()
}

/// Combat geometry drawn with the player's airframe: fixture targets over the
/// player's presented state, debris, weapons, tracers and effects.
pub fn combat_geometry(
    snapshot: &RenderSnapshot,
    art: &CombatArt,
    ownship: &Airframe,
    ownship_state: &flight::State,
    camera: &Camera,
    world: &Terrain,
    scenery: &Scenery,
) -> CombatGeometry {
    let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Aircraft);
    let mut v = Vec::new();
    let mut contacts = Vec::new();
    let extent = ownship.visual_extent();
    // Everything here is built relative to the render origin, in f64 until
    // the vertex is written.
    let local = |p: Vector| scenery.relative(p);
    let mut eye = Camera::new();
    eye.position = local(camera.position);
    [eye.yaw, eye.pitch, eye.roll] = [camera.yaw, camera.pitch, camera.roll];
    for target in snapshot
        .targets
        .iter()
        .filter(|t| t.draw == Draw::Ownship && t.airborne && Some(t.id) != camera.hidden_target)
    {
        let pose = ownship_pose(ownship_state, target);
        let first = v.len() / 10;
        v.extend(ownship.vertices(&pose, camera, world, scenery));
        contacts.extend(Contact::new(
            first,
            v.len() / 10,
            local(pose.position),
            extent,
        ));
    }
    for piece in snapshot.debris.iter().filter(|p| p.draw == Draw::Ownship) {
        let mut pose = ownship_state.clone();
        // Detached pieces have their own lifecycle, not the observer's wreck state.
        pose.wreck = None;
        pose.position = piece.position;
        pose.damage_variant = piece.variant;
        [pose.yaw, pose.pitch, pose.bank] = piece.attitude;
        v.extend(ownship.fragment_vertices(&pose, camera, world, scenery));
    }
    // Stores still on their pylons are drawn with their aircraft
    // (`crate::pylons`); here only what is in flight.
    for p in &snapshot.projectiles {
        if Some(p.id) == camera.hidden_projectile {
            continue;
        }
        if let Some(body) = p.shape.as_deref().and_then(procedural_store_body) {
            let right = if p.direction[0].hypot(p.direction[2]) < 1e-9 {
                [1., 0., 0.]
            } else {
                unit([p.direction[2], 0., -p.direction[0]])
            };
            let basis = Basis {
                right,
                up: cross(p.direction, right),
                forward: p.direction,
            };
            crate::pylons::body(&mut v, &basis, local(p.position), body, [0.45; 3]);
        }
        if !p.gun
            && let Some(shape) = p.shape.as_ref().and_then(|name| art.shapes.get(name))
        {
            let right = unit([p.direction[2], 0., -p.direction[0]]);
            mesh(
                &mut v,
                shape,
                local(p.position),
                right,
                cross(p.direction, right),
                p.direction,
                &ownship.palette,
            );
        }
        if p.gun && p.tracer {
            gun_tracer(
                &mut v,
                local(p.previous),
                local(p.position),
                &eye,
                &p.weapon,
            );
        } else if !p.gun && p.powered {
            // The motor's flame: a thin bright strip over the swept segment
            // while the motor burns, and nothing after burnout or on a bomb
            // (bug B7).
            let right = Basis::new(f64::from(camera.yaw), f64::from(camera.pitch), 0.).right;
            let previous = local(p.previous);
            let a: Vector = std::array::from_fn(|i| previous[i] + right[i] * 0.4);
            let b: Vector = std::array::from_fn(|i| previous[i] - right[i] * 0.4);
            for pos in [a, b, local(p.position)] {
                vertex(&mut v, pos, [1., 0.8, 0.3]);
            }
        }
    }
    // The sparks of impacts (bug B11), as short tracer streaks.
    for spark in crate::impact_fx::sparks(&snapshot.effects, |x, z| {
        f64::from(world.height(x as f32, z as f32))
    }) {
        tracer(
            &mut v,
            local(spark.from),
            local(spark.to),
            &eye,
            spark.brightness,
            spark.width,
        );
    }
    // Explosions, fires and craters are drawn by effect_renderer.
    CombatGeometry {
        vertices: v,
        contacts,
    }
}

/// Combat effect and weapon art: the explosion, fire and crater sheets, the
/// smoke sheet, weapon shapes by name and ejected-pilot poses.
pub struct CombatArt {
    pub smoke: Pic,
    shapes: BTreeMap<String, Shape>,
    /// The same shapes as carried on a pylon, where that differs from in
    /// flight (a retarded bomb's fins are closed until release).
    stores: BTreeMap<String, Shape>,
    pub effects: crate::effect_renderer::Art,
    pub escape: Option<crate::ejection_art::Art>,
}
impl CombatArt {
    /// Loads the sampled original effect art. `palette` supplies the colours
    /// the effect sheets do not carry themselves.
    pub fn load(data: &BTreeMap<String, Vec<u8>>) -> AppResult<Self> {
        let smoke = Pic::parse(data.get("SMOKE.PIC").ok_or("missing SMOKE.PIC")?)?;
        if smoke.width != 256 || smoke.height != 43 {
            return Err("unreviewed smoke sheet dimensions".into());
        }
        Ok(Self {
            smoke,
            shapes: BTreeMap::new(),
            stores: BTreeMap::new(),
            effects: crate::effect_renderer::Art::load(data),
            escape: match crate::ejection_art::Art::load(data) {
                Ok(art) => Some(art),
                Err(error) => {
                    log::warn!("Optional ejection artwork unavailable: {error}");
                    None
                }
            },
        })
    }
    /// Loads every weapon shape a configuration's stations name.
    pub fn add_weapon_shapes(
        &mut self,
        config: &live::Configuration,
        data: &BTreeMap<String, Vec<u8>>,
    ) {
        self.add_shapes(
            config
                .stations
                .iter()
                .filter_map(|station| station.weapon.shape.as_deref()),
            data,
        );
    }
    /// A loaded weapon shape by name as carried on a pylon (the stores on
    /// pylons draw with it).
    pub(crate) fn shape(&self, name: &str) -> Option<&Shape> {
        self.stores.get(name).or_else(|| self.shapes.get(name))
    }
    /// Loads named weapon shapes; a missing or empty one draws as a strip.
    pub fn add_shapes<'a>(
        &mut self,
        names: impl IntoIterator<Item = &'a str>,
        data: &BTreeMap<String, Vec<u8>>,
    ) {
        for name in names {
            if procedural_store_body(name).is_some() {
                continue;
            }
            let shape = data
                .get(name)
                .and_then(|bytes| Shape::parse(bytes).ok())
                .filter(|shape| !shape.faces.is_empty());
            match shape {
                Some(shape) => {
                    // Refitted to the real store's size where it is known
                    // (crate::store_sizes, BP2), in flight and on a pylon.
                    let (store, flight) = crate::store_sizes::variants(name, shape);
                    if let Some(store) = store {
                        self.stores.insert(name.to_owned(), store);
                    }
                    self.shapes.insert(name.to_owned(), flight);
                }
                None => log::warn!(
                    "Combat: {name} uses a tracer marker; line/point drawing remains open"
                ),
            }
        }
    }
    /// Synthetic art for drawing tests without retail media.
    #[cfg(test)]
    pub(crate) fn synthetic(shapes: BTreeMap<String, Shape>) -> Self {
        Self {
            smoke: Pic {
                width: 1,
                height: 1,
                pixels: vec![0],
                mask: vec![true],
                palette: Vec::new(),
                glyphs: Vec::new(),
            },
            shapes,
            stores: BTreeMap::new(),
            effects: crate::effect_renderer::Art::empty(),
            escape: None,
        }
    }
}

fn mesh(
    out: &mut Vec<f32>,
    shape: &Shape,
    position: Vector,
    right: Vector,
    up: Vector,
    forward: Vector,
    palette: &[[u8; 3]; 256],
) {
    for face in &shape.faces {
        // These are texture-only SH faces (including the missile exhaust
        // sheets). Palette index zero is not an opaque substitute for them.
        // Keep them omitted until the weapon texture/animation path is decoded.
        if matches!(face.subtype, 0x4c | 0x5c | 0x6c | 0x7c) {
            continue;
        }
        for i in 1..face.positions.len() - 1 {
            for j in [0, i, i + 1] {
                let q = face.positions[j];
                let pos = std::array::from_fn(|k| {
                    position[k]
                        + (right[k] * f64::from(q[0])
                            + up[k] * f64::from(q[2])
                            + forward[k] * f64::from(q[1]))
                            / 3.
                });
                vertex(
                    out,
                    pos,
                    palette[face.colors[j] as usize].map(|c| f32::from(c) / 255.),
                );
                let layer = out.len() - 5;
                out[layer] = -1.;
                // The face's own palette entry and fog mode, as an
                // airframe's faces and the stores on its pylons carry them,
                // so a weapon is lit by the weather as they are (PY2).
                let last = out.len() - 1;
                out[last] = f32::from(face.colors[j]) + 256. * f32::from(face.fog as u8);
            }
        }
    }
}
/// How bright a gun's tracer is drawn against the ordinary one: the AC-130's
/// 105 mm round half as bright again (John, 2026-10-09), so it reads at
/// gunship ranges.
pub const HOWITZER_TRACER: f64 = 1.5;
fn tracer_brightness(weapon: &str) -> f64 {
    if weapon == tore_sim::combat::gunship::GUNS[2] {
        HOWITZER_TRACER
    } else {
        1.
    }
}
/// How wide a gun's tracer ribbon is drawn against the ordinary one: the
/// AC-130's 105 mm round 1.75 times as wide (agent choice within John's
/// 1.5 to 2 times, 2026-10-10), so it reads next to the 40 mm's. The ordinary
/// ribbon is 2.4 feet across.
pub const HOWITZER_TRACER_WIDTH: f64 = 1.75;
fn tracer_width(weapon: &str) -> f64 {
    if weapon == tore_sim::combat::gunship::GUNS[2] {
        HOWITZER_TRACER_WIDTH
    } else {
        1.
    }
}
/// Opinionated ammunition colour families, John 2026-10-16. Exact record
/// keys, never aircraft nationality. Reviewed assignments and exceptions:
/// docs/spec/damage-smoke.md#tracer-colour-families.
const GREEN_TRACERS: &[&str] = &[
    "GSH23.JT",
    "GSH30.JT",
    "GSH301.JT",
    "GSH6_23.JT",
    "GSH6_30.JT",
    "T12_4.JT",
    "T30_1.JT",
    "ZSU23.JT",
    "2S6.JT",
    "ZSU57.JT",
    "AAA30.JT",
    "AAA30BAD.JT",
    "M1939.JT",
    "A_M1939.JT",
    "BMP2.JT",
    "BTR80.JT",
    "KS12.JT",
    "KS19.JT",
    "T72.JT",
];
fn green_tracer(weapon: &str) -> bool {
    GREEN_TRACERS
        .iter()
        .any(|record| record.eq_ignore_ascii_case(weapon))
}

/// The vertex color that the shader's sRGB decode turns into `linear`, so a
/// tracer's brightness survives the decode the world vertices go through.
fn encoded(linear: f64) -> f32 {
    if linear <= 0.0031308 {
        (linear * 12.92) as f32
    } else {
        (1.055 * linear.powf(1. / 2.4) - 0.055) as f32
    }
}
/// Camera-facing luminous ribbon over the actual swept gun segment, at
/// the weapon's colour family, relative radiance and relative width.
fn gun_tracer(
    out: &mut Vec<f32>,
    previous: Vector,
    position: Vector,
    camera: &Camera,
    weapon: &str,
) {
    let first = out.len();
    ribbon(
        out,
        previous,
        position,
        camera,
        tracer_brightness(weapon),
        tracer_width(weapon),
        true,
    );
    // Extend the existing tag, leaving red vertex bytes exactly unchanged.
    if green_tracer(weapon) {
        for vertex in out[first..].chunks_exact_mut(10) {
            vertex[9] -= 20.;
        }
    }
}

fn tracer(
    out: &mut Vec<f32>,
    previous: Vector,
    position: Vector,
    camera: &Camera,
    brightness: f64,
    width: f64,
) {
    ribbon(out, previous, position, camera, brightness, width, false);
}

// Gun vertices reuse the ribbon payload: RGB is side * width, index is
// -(2 + radiance + 10 if end-on + 20 if green); UV.x is signed projected half-length.
// Sparks retain the ordinary color payload.
// The shared object shader expands this in the actual view's pixel units.
#[allow(clippy::too_many_arguments)]
fn ribbon(
    out: &mut Vec<f32>,
    previous: Vector,
    position: Vector,
    camera: &Camera,
    brightness: f64,
    width: f64,
    gun: bool,
) {
    let level = encoded(brightness);
    let segment: Vector = std::array::from_fn(|i| position[i] - previous[i]);
    if tore_sim::attitude::dot(segment, segment) < 1e-12 {
        return;
    }
    let view: Vector = std::array::from_fn(|i| camera.position[i] - position[i]);
    let cross = cross(segment, view);
    let end_on = tore_sim::attitude::dot(cross, cross)
        <= 0.25 * tore_sim::attitude::dot(view, view).max(1e-12);
    let (start, ribbon, side) = if !end_on {
        (previous, segment, unit(cross))
    } else {
        // Viewed along its path, retain a small glow instead of collapsing
        // the ribbon into a line with zero screen area.
        let basis = Basis::new(f64::from(camera.yaw), f64::from(camera.pitch), 0.);
        (
            std::array::from_fn(|i| position[i] - basis.up[i] * 0.25 * width),
            basis.up.map(|v| v * 0.5 * width),
            basis.right,
        )
    };
    let projected_length = if end_on {
        0.5 * width
    } else {
        (tore_sim::attitude::dot(cross, cross) / tore_sim::attitude::dot(view, view).max(1e-12))
            .sqrt()
    };
    for [along, across] in [
        [0., -1.],
        [1., -1.],
        [1., 1.],
        [0., -1.],
        [1., 1.],
        [0., 1.],
    ] {
        let pos: Vector =
            std::array::from_fn(|i| start[i] + ribbon[i] * along + side[i] * across * 1.2 * width);
        out.extend([
            pos[0] as f32,
            pos[1] as f32,
            pos[2] as f32,
            if gun {
                ((along - 0.5) * projected_length) as f32
            } else {
                along as f32
            },
            across as f32,
            -8.,
            if gun { (side[0] * width) as f32 } else { level },
            if gun { (side[1] * width) as f32 } else { level },
            if gun { (side[2] * width) as f32 } else { level },
            if gun {
                -(2. + brightness + if end_on { 10. } else { 0. }) as f32
            } else {
                -1.
            },
        ]);
    }
}

fn vertex(out: &mut Vec<f32>, pos: Vector, color: [f32; 3]) {
    // Trailing -1 opts out of the weather palette: this color is already resolved.
    out.extend([
        pos[0] as f32,
        pos[1] as f32,
        pos[2] as f32,
        0.,
        0.,
        -6., // Emissive effect; mesh() opts solid weapon bodies into lighting.
        color[0],
        color[1],
        color[2],
        -1.,
    ]);
}

/// Bounded host-generated tank/pod geometry, carried in the shape identity.
fn procedural_store_body(name: &str) -> Option<[f64; 2]> {
    let (length, diameter) = name.strip_prefix("TORE-BODY:")?.split_once(':')?;
    let body = [length.parse::<f64>().ok()?, diameter.parse::<f64>().ok()?];
    body.iter()
        .all(|v| v.is_finite() && *v > 0. && *v <= 100.)
        .then_some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    // CPU mirror of the shader, for measurable headless presentation checks.
    fn night_rule(sun: f64, thermal: bool) -> (f64, f64, f64) {
        let t = ((sun + 0.104528) / (0.0348995 + 0.104528)).clamp(0., 1.);
        let night = if thermal {
            0.
        } else {
            1. - t * t * (3. - 2. * t)
        };
        (night, 1. + night, 1. + 5. * night)
    }

    fn pixel_width(distance: f64, night: f64, width: f64) -> f64 {
        let pixel = 2. * distance / (1080. * 1.7320508);
        2. * (1.2 * width + night * (pixel - 1.2 * width).max(0.)) / pixel
    }

    // Halo-only peak, excluding the flare's core and canopy cross-streaks.
    fn glow_peak(night: f64) -> f64 {
        1.5 * 0.75 * night
    }

    fn colours(night: f32, green: bool) -> ([f32; 3], [f32; 3]) {
        let mix = |a: f32, b: f32| a * (1. - night) + b * night;
        if green {
            ([mix(0.78, 0.72), 1., mix(0.68, 0.38)], [0.09, 1., 0.025])
        } else if night > 0. {
            (
                [1., mix(0.92, 0.72), mix(0.68, 0.38)],
                [1., mix(0.25, 0.09), 0.025],
            )
        } else {
            ([1., 0.92, 0.68], [1., 0.25, 0.025])
        }
    }

    #[test]
    fn tracer_night_factors_and_pixel_floor() {
        assert_eq!(night_rule(1., false), (0., 1., 1.));
        assert_eq!(night_rule(-1., false), (1., 2., 6.));
        assert_eq!(night_rule(-1., true), (0., 1., 1.));
        let middle = (-0.104528 + 0.0348995) / 2.;
        assert_eq!(night_rule(middle, false), (0.5, 1.5, 3.5));
        for edge in [-0.104528, middle, 0.0348995] {
            let left = night_rule(edge - 1e-7, false);
            let right = night_rule(edge + 1e-7, false);
            assert!((left.0 - right.0).abs() < 3e-6);
            assert!((left.2 - right.2).abs() < 2e-5);
            assert!((glow_peak(left.0) - glow_peak(right.0)).abs() < 3e-6);
            let (lc, lb) = colours(left.0 as f32, false);
            let (rc, rb) = colours(right.0 as f32, false);
            assert!(
                lc.into_iter()
                    .zip(rc)
                    .chain(lb.into_iter().zip(rb))
                    .all(|(a, b)| (a - b).abs() < 3e-6)
            );
        }
        let visibility = |d: f64| 1. - ((d - 6000.) / 6000.).clamp(0., 1.);
        // Exact envelope scaling also holds for exponential aerial opacity
        // at fixed eye/source altitudes; dense clouds deliberately stay unscaled.
        for d in [0., 6000., 9000., 12000., 18000., 40000.] {
            assert_eq!(visibility(d / 3.), visibility(2. * d / 6.));
            assert_eq!((-d / 3. / 50000.).exp(), (-2. * d / 6. / 50000.).exp());
        }
        for nm in [3., 4., 5., 6., 8., 10.] {
            let d = nm * 6076.12;
            assert!((pixel_width(d, 1., 1.) - 2.).abs() < 1e-12);
            let flare_peak = 1.5;
            assert!((glow_peak(1.) / flare_peak - 0.75).abs() < 1e-12);
            println!(
                "{nm} nm: visibility {:.6} -> {:.6}; ribbon peak red {:.6} -> {:.6}; new glow peak {:.6}",
                visibility(d / 3.),
                visibility(d / 6.),
                7.3 * visibility(d / 3.),
                7.3 * visibility(d / 6.),
                glow_peak(1.) * visibility(d / 6.)
            );
        }
        let shader = include_str!("terrain.wgsl");
        for rule in [
            "in.distance/(1.0+5.0*night)",
            "in.color.g*(1.0+night)",
            "max(core_width,8.0*pixel)",
            "max(core_length,16.0*pixel)",
            "select(6.0,2.0,end_on)",
            "let strength=0.75*night;",
            "1.1*exp(-r*r*10.0)+0.4*exp(-r*3.2)",
            "vec3<f32>(1.0,0.72,0.38)",
            "vec3<f32>(1.0,0.09,0.025)",
            "vec3<f32>(1.0,0.06,0.025)",
            "cloud_occlusion(vec3<f32>(1.0),in.direction,in.altitude)",
        ] {
            assert!(shader.contains(rule), "missing shader contract: {rule}");
        }
        assert!(
            include_str!("countermeasures.wgsl").contains("1.1*exp(-r*r*10.0)+0.4*exp(-r*3.2)")
        );
        assert_eq!(colours(1., false), ([1., 0.72, 0.38], [1., 0.09, 0.025]));
        assert_eq!(colours(1., true), ([0.72, 1., 0.38], [0.09, 1., 0.025]));
    }

    #[test]
    fn tracer_halo_survives_with_the_streak_at_range() {
        let visibility = |d: f64| 1. - ((d / 6. - 6000.) / 6000.).clamp(0., 1.);
        for nm in [1., 2., 4., 6., 8.] {
            let d = nm * 6076.12;
            let new = glow_peak(1.) * visibility(d);
            let old = new / (1. + (d / 3000.).powi(2));
            let pixel = 2. * d / (1080. * 1.7320508);
            let width = 2. * (1.2_f64.max(pixel)).max(8. * pixel) / pixel;
            assert!((width - 16.).abs() < 1e-12);
            assert!((new / visibility(d) - 1.125).abs() < 1e-12);
            assert!(new > 0.73, "halo must survive at {nm} nm");
            println!(
                "halo {nm} nm: peak {old:.6} -> {new:.6}; full width {width:.2} -> {width:.2} px"
            );
        }
        // At 4 pixels off the centre (half of the 8 px halo radius),
        // outside the narrow ribbon, the halo must still have real radiance.
        let r = 0.5_f64;
        let shoulder = 0.75 * (1.1 * (-r * r * 10.).exp() + 0.4 * (-r * 3.2).exp());
        assert!((shoulder - 0.128289079).abs() < 1e-8);
        assert_eq!(glow_peak(1.) * visibility(72000.), 0.);
        assert_eq!(glow_peak(0.), 0.);
        assert!(!include_str!("terrain.wgsl").contains("pow(in.distance/3000.0,2.0)"));
    }

    #[test]
    fn tracer_glow_bounds_and_streak_shape() {
        let dimensions = |physical_half: f64, length: f64, pixel: f64, night: f64, end_on: bool| {
            let core = physical_half + night * (pixel - physical_half).max(0.);
            let wide = core + night * (8. * pixel - core).max(0.);
            let streak = length + night * ((if end_on { 2. } else { 6. }) * pixel - length).max(0.);
            let long = streak + night * (16. * pixel - streak).max(0.);
            [core, wide, streak, long]
        };
        for width in [1., HOWITZER_TRACER_WIDTH] {
            for length in [0.5 * width, 20., 500.] {
                for pixel in [0.1, 1., 20., 100.] {
                    assert_eq!(
                        dimensions(1.2 * width, length, pixel, 0., false),
                        [1.2 * width, 1.2 * width, length, length]
                    );
                    for end_on in [false, true] {
                        let [core, wide, streak, long] =
                            dimensions(1.2 * width, length, pixel, 1., end_on);
                        assert!(wide >= core && wide - core <= 8. * pixel);
                        assert!(long >= streak && long - streak <= 16. * pixel);
                        assert!(streak >= if end_on { 2. * pixel } else { 6. * pixel });
                        for night in [0., 0.5, 1.] {
                            let a = dimensions(
                                1.2 * width,
                                length,
                                pixel,
                                (night - 1e-8_f64).max(0.),
                                end_on,
                            );
                            let b = dimensions(
                                1.2 * width,
                                length,
                                pixel,
                                (night + 1e-8_f64).min(1.),
                                end_on,
                            );
                            assert!(a.into_iter().zip(b).all(|(a, b)| (a - b).abs() < 0.0001));
                        }
                    }
                }
            }
        }
        assert_eq!(
            dimensions(1.2, 20., 100., 1., false),
            [100., 800., 600., 1600.]
        );
        assert_eq!(
            dimensions(1.2, 0.5, 100., 1., true),
            [100., 800., 200., 1600.]
        );
    }

    #[test]
    fn tracer_families_and_red_day_bytes() {
        for weapon in GREEN_TRACERS {
            assert!(green_tracer(weapon));
            assert!(green_tracer(&weapon.to_lowercase()));
        }
        for weapon in [
            "M61.JT",
            "C_25.JT",
            "C_40.JT",
            "C_105.JT",
            "PHALANX.JT",
            "M2.JT",
            "M113.JT",
            "SMLARMS.JT",
            "UNKNOWN.JT",
            "GSH301.GN",
        ] {
            assert!(!green_tracer(weapon));
            for end in [[20., 0., 24000.], [0., 0., 24020.]] {
                let mut old = Vec::new();
                let mut new = Vec::new();
                let camera = Camera::new();
                ribbon(
                    &mut old,
                    [0., 0., 24000.],
                    end,
                    &camera,
                    tracer_brightness(weapon),
                    tracer_width(weapon),
                    true,
                );
                gun_tracer(&mut new, [0., 0., 24000.], end, &camera, weapon);
                assert_eq!(
                    old.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    new.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
                );
            }
        }
        // No new green assignment enables an unflagged flak/tank round.
        for record in ["KS12.JT", "KS19.JT", "T72.JT"] {
            assert!(green_tracer(record));
            assert!(
                !tore_sim::combat::surface_guns::tuning("", record)
                    .unwrap()
                    .tracer
            );
        }
        let (core, body) = colours(0., false);
        assert_eq!(
            core.map(f32::to_bits),
            [1_f32, 0.92, 0.68].map(f32::to_bits)
        );
        assert_eq!(
            body.map(f32::to_bits),
            [1_f32, 0.25, 0.025].map(f32::to_bits)
        );
        assert_eq!(glow_peak(0.).to_bits(), 0_f64.to_bits());
    }

    #[test]
    fn every_gun_family_uses_the_same_payload_and_sparks_do_not() {
        let camera = Camera::new();
        for weapon in [
            "ZSU23",
            "ZSU57",
            "M163",
            "SHIP",
            "SMLARMS",
            "M61A1.GN",
            "GSH23.GN",
            "HELI",
            "C_25.JT",
            "C_40.JT",
            "C_105.JT",
            "GSH301.JT",
            "GSH23.JT",
            "T12_4.JT",
            "T30_1.JT",
            "ZSU23.JT",
            "ZSU57.JT",
        ] {
            for end in [[20., 0., 0.], camera.position] {
                let mut vertices = Vec::new();
                gun_tracer(&mut vertices, [0.; 3], end, &camera, weapon);
                assert_eq!(vertices.len(), 60);
                for v in vertices.chunks_exact(10) {
                    assert_eq!(v[5], -8.);
                    assert!(v[9] < -2.);
                    let tag = v[9] + if green_tracer(weapon) { 20. } else { 0. };
                    assert_eq!(v[9] < -22., green_tracer(weapon));
                    let brightness = -tag - 2. - if tag < -12. { 10. } else { 0. };
                    assert_eq!(brightness, tracer_brightness(weapon) as f32);
                    assert!(
                        (v[6..9].iter().map(|x| x * x).sum::<f32>().sqrt()
                            - tracer_width(weapon) as f32)
                            .abs()
                            < 1e-5
                    );
                }
            }
        }
        let mut spark = Vec::new();
        tracer(&mut spark, [0.; 3], [20., 0., 0.], &camera, 2.2, 0.5);
        assert_eq!(spark[9], -1.);
    }

    #[test]
    fn fires_and_other_effects_never_carry_the_green_tracer_flag() {
        let camera = Camera::new();
        // Emissive effect colour, the path a flame uses. Fire sprites are a
        // different buffer and never reach this tag.
        let mut fire = Vec::new();
        vertex(&mut fire, [0., 4., 0.], [1., 0.45, 0.12]);
        assert!(fire.chunks_exact(10).all(|v| v[5] == -6. && v[9] == -1.));
        let mut sparks = Vec::new();
        tracer(&mut sparks, [0.; 3], [8., 1., 0.], &camera, 2.2, 0.4);
        assert!(sparks.chunks_exact(10).all(|v| v[5] == -8. && v[9] > -22.));
        for weapon in ["ZSU23.JT", "BMP2.JT"] {
            let mut ribbon = Vec::new();
            gun_tracer(&mut ribbon, [0.; 3], [30., 0., 0.], &camera, weapon);
            assert!(
                ribbon.chunks_exact(10).all(|v| v[9] < -22.),
                "{weapon} is a green tracer"
            );
        }
        let mut red = Vec::new();
        gun_tracer(&mut red, [0.; 3], [30., 0., 0.], &camera, "M163.JT");
        assert!(red.chunks_exact(10).all(|v| (-22. ..-2.).contains(&v[9])));
    }

    #[test]
    fn live_and_recorded_rounds_share_geometry_in_every_view() {
        use crate::replay::convert;
        use crate::snapshot::ProjectilePose;
        let world = tore_world::test_support::terrain();
        let scenery = crate::scenery::tests::scenery();
        let models = crate::combat_view::render_hash_tests::models();
        let art = CombatArt::synthetic(Default::default());
        for weapon in [
            "M61A1.GN",
            "GSH23.GN",
            "ZSU23",
            "ZSU57",
            "M163",
            "SHIP",
            "SMLARMS",
            "HELI",
            "C_25.JT",
            "C_40.JT",
            "C_105.JT",
            "GSH301.JT",
            "GSH23.JT",
            "T12_4.JT",
            "T30_1.JT",
            "ZSU23.JT",
            "ZSU57.JT",
        ] {
            let info = tore_replay::WeaponInfo {
                id: 1,
                source: weapon.into(),
                shape: None,
                name: weapon.into(),
                class: tore_replay::WeaponClass::Gun,
            };
            let pose = ProjectilePose {
                id: 1,
                owner: 0,
                weapon: weapon.into(),
                shape: None,
                gun: true,
                tracer: true,
                position: [20., 500., 24000.],
                previous: [0., 500., 24000.],
                direction: [1., 0., 0.],
                target: None,
                incoming: false,
                speed_f8: 1000,
                powered: false,
            };
            // net/observe uses this same conversion when recording a client.
            let recorded = convert::projectile_pose(
                &convert::projectile_state(&pose, 1, 1, None),
                Some(&info),
            );
            let live = RenderSnapshot {
                projectiles: vec![pose],
                ..Default::default()
            };
            let replay = RenderSnapshot {
                projectiles: vec![recorded],
                ..Default::default()
            };
            for mut camera in crate::combat_view::render_hash_tests::cameras() {
                for thermal in [false, true] {
                    camera.thermal = thermal;
                    let draw = |snapshot| {
                        combat_geometry(
                            snapshot,
                            &art,
                            &models[0],
                            &flight::State::new(
                                &tore_world::test_support::profile(),
                                [0., 5000., 0.],
                            )
                            .unwrap(),
                            &camera,
                            &world,
                            &scenery,
                        )
                        .vertices
                    };
                    let vertices = draw(&live);
                    assert_eq!(vertices.len(), 60);
                    assert!(vertices.chunks_exact(10).all(|v| v[9] < -2.));
                    assert_eq!(vertices, draw(&replay));
                }
            }
            let mut untraced = live;
            untraced.projectiles[0].tracer = false;
            assert!(
                combat_geometry(
                    &untraced,
                    &art,
                    &models[0],
                    &flight::State::new(&tore_world::test_support::profile(), [0., 5000., 0.])
                        .unwrap(),
                    &Camera::new(),
                    &world,
                    &scenery
                )
                .vertices
                .is_empty()
            );
        }
    }

    #[test]
    #[ignore = "manual CPU cost measurement, no GPU"]
    fn tracer_busy_scene_cpu_cost() {
        let camera = Camera::new();
        for gun in [false, true] {
            let start = std::time::Instant::now();
            let mut vertices = Vec::with_capacity(5000 * 60);
            for _ in 0..200 {
                vertices.clear();
                for round in 0..5000 {
                    let z = 6076.12 * (3. + (round % 300) as f64 / 100.);
                    if gun {
                        gun_tracer(
                            &mut vertices,
                            [0., 0., z],
                            [20., 0., z],
                            &camera,
                            if round % 2 == 0 { "M61.JT" } else { "ZSU23.JT" },
                        );
                    } else {
                        ribbon(
                            &mut vertices,
                            [0., 0., z],
                            [20., 0., z],
                            &camera,
                            1.,
                            1.,
                            false,
                        );
                    }
                }
                std::hint::black_box(&vertices);
            }
            println!(
                "5000 simultaneous tracers gun={gun}: {:.3} ms/frame, {} bytes, one existing draw",
                start.elapsed().as_secs_f64() * 1000. / 200.,
                vertices.len() * 4
            );
        }
    }

    fn formation(count: usize, mixed: bool) -> RenderSnapshot {
        let mut snapshot = RenderSnapshot {
            models: if mixed {
                // Deliberately differ from asset order, retain an empty loaded
                // model and a missing model, and repeat one batch identity.
                vec![
                    AircraftId::Rafale,
                    AircraftId::F18,
                    AircraftId::F14,
                    AircraftId::Mig29,
                    AircraftId::F18,
                ]
            } else {
                vec![AircraftId::F18]
            },
            ..Default::default()
        };
        for i in 0..count {
            let id = if mixed && i % 2 == 0 {
                AircraftId::Rafale
            } else {
                AircraftId::F18
            };
            snapshot.targets.push(AircraftPose {
                id: i as u32 + 1,
                aircraft: Some(id),
                draw: Draw::Model(id),
                position: [50. * i as f64, 5000. + i as f64, 1000. + 31. * i as f64],
                attitude: [0.07 * i as f64, -0.01 * i as f64, 0.1 * i as f64],
                devices: (i % 3 != 0).then_some([
                    1., 0.6, 0.1, 0.4, 0.3, 0.6, 0.2, -0.3, 0.1, 670., 0.75, 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0., 0.,
                ]),
                damage: crate::snapshot::Damage {
                    hp: if i % 7 == 0 { 0 } else { 85 },
                    initial_hp: 100,
                    sections: [0, 0, 0, 15, 0, 0],
                    structural: Some(live::DamageSection::LeftWing),
                },
                airborne: i % 11 != 0,
                ..Default::default()
            });
        }
        for (index, draw) in [Draw::Hidden, Draw::Ownship, Draw::Model(AircraftId::Mig29)]
            .into_iter()
            .enumerate()
        {
            snapshot.targets.push(AircraftPose {
                id: 1000 + index as u32,
                draw,
                airborne: true,
                ..Default::default()
            });
        }
        for (index, draw) in [
            Draw::Model(AircraftId::Rafale),
            Draw::Model(AircraftId::F18),
            Draw::Ownship,
            Draw::Model(AircraftId::F18),
        ]
        .into_iter()
        .enumerate()
        {
            snapshot.debris.push(DebrisPose {
                owner: index as u32 + 1,
                draw,
                position: [-30. * index as f64, 4900., 1000.],
                attitude: [0.25, 0.6, index as f64],
                // Exercise an empty debris job as well as drawn fragments.
                variant: (index != 3).then_some(live::DamageSection::LeftWing as usize),
            });
        }
        snapshot
    }

    fn assert_batches_identical(
        expected: &[(&Airframe, Vec<f32>, Vec<Contact>)],
        actual: &[(&Airframe, Vec<f32>, Vec<Contact>)],
    ) {
        assert_eq!(actual.len(), expected.len());
        for ((expected_model, expected_vertices, expected_contacts), (model, vertices, contacts)) in
            expected.iter().zip(actual)
        {
            assert_eq!(model.profile.id, expected_model.profile.id);
            assert_eq!(
                vertices.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                expected_vertices
                    .iter()
                    .map(|v| v.to_bits())
                    .collect::<Vec<_>>(),
                "vertex bytes for {:?}",
                model.profile.id,
            );
            assert_eq!(contacts.len(), expected_contacts.len());
            for (actual, expected) in contacts.iter().zip(expected_contacts) {
                assert_eq!(
                    (actual.first, actual.count),
                    (expected.first, expected.count)
                );
                assert_eq!(
                    actual.center.map(f32::to_bits),
                    expected.center.map(f32::to_bits)
                );
                assert_eq!(actual.extent.to_bits(), expected.extent.to_bits());
            }
        }
    }

    #[test]
    fn aircraft_workers_preserve_original_vertices_contacts_and_order() {
        let models = crate::combat_view::render_hash_tests::models();
        let world = tore_world::test_support::terrain();
        let mut scenery = crate::scenery::tests::scenery();
        scenery.set_origin([20_000., 4500., -11_000.]);
        let mut executors = vec![tore_workers::Executor::serial()];
        executors
            .extend([1, 2, 4, 8].map(|workers| tore_workers::Executor::parallel(workers).unwrap()));
        executors.extend([1, 73, 991].map(tore_workers::Executor::shuffled));
        for snapshot in [
            formation(30, false),
            formation(30, true),
            RenderSnapshot::default(),
        ] {
            for smooth in [false, true] {
                scenery.smooth_weather = smooth;
                for camera in crate::combat_view::render_hash_tests::cameras() {
                    let expected = aircraft_batches_serial(
                        &snapshot,
                        &models,
                        &CombatArt::synthetic(Default::default()),
                        &camera,
                        &world,
                        &scenery,
                    );
                    for executor in &executors {
                        let actual = aircraft_batches_with(
                            executor,
                            &snapshot,
                            &models,
                            &CombatArt::synthetic(Default::default()),
                            &camera,
                            &world,
                            &scenery,
                        );
                        assert_batches_identical(&expected, &actual);
                    }
                    if smooth && !snapshot.models.is_empty() {
                        let contacts: usize = expected.iter().map(|(_, _, c)| c.len()).sum();
                        assert!(contacts >= 20, "the formation must actually draw contacts");
                    }
                }
            }
        }
    }

    #[test]
    fn aircraft_workers_keep_the_small_inline_fallback() {
        let models = crate::combat_view::render_hash_tests::models();
        let world = tore_world::test_support::terrain();
        let scenery = crate::scenery::tests::scenery();
        let camera = Camera::new();
        let executor = tore_workers::Executor::parallel(2).unwrap();
        for count in 0..MIN_AIRCRAFT_JOBS {
            let mut snapshot = formation(count, false);
            snapshot.debris.clear();
            let expected = aircraft_batches_serial(
                &snapshot,
                &models,
                &CombatArt::synthetic(Default::default()),
                &camera,
                &world,
                &scenery,
            );
            let actual = aircraft_batches_with(
                &executor,
                &snapshot,
                &models,
                &CombatArt::synthetic(Default::default()),
                &camera,
                &world,
                &scenery,
            );
            assert_batches_identical(&expected, &actual);
        }
    }

    /// Portable CPU-only probe. Run separately on a quiet machine in release
    /// mode; actual frame gains still need the imported scene and graphics host.
    #[test]
    #[ignore = "opt-in elapsed geometry timing; run in release mode on a quiet machine"]
    fn aircraft_geometry_wall_time() {
        use std::{hint::black_box, time::Instant};
        let models = crate::combat_view::render_hash_tests::models();
        let world = tore_world::test_support::terrain();
        let scenery = crate::scenery::tests::scenery();
        let camera = crate::combat_view::render_hash_tests::cameras().remove(0);
        for count in [1, 4, 30] {
            let mut snapshot = formation(count, false);
            snapshot.debris.clear();
            snapshot
                .targets
                .retain(|target| target.draw == Draw::Model(AircraftId::F18));
            for target in &mut snapshot.targets {
                target.airborne = true;
            }
            let expected = aircraft_batches_serial(
                &snapshot,
                &models,
                &CombatArt::synthetic(Default::default()),
                &camera,
                &world,
                &scenery,
            );
            for workers in [0, 1, 2, 4, 8] {
                let executor = if workers == 0 {
                    tore_workers::Executor::serial()
                } else {
                    tore_workers::Executor::parallel(workers).unwrap()
                };
                let build = || {
                    aircraft_batches_with(
                        &executor,
                        &snapshot,
                        &models,
                        &CombatArt::synthetic(Default::default()),
                        &camera,
                        &world,
                        &scenery,
                    )
                };
                assert_batches_identical(&expected, &build());
                for _ in 0..20 {
                    black_box(build());
                }
                let start = Instant::now();
                let rounds = 500;
                for _ in 0..rounds {
                    black_box(build());
                }
                println!(
                    "aircraft_geometry aircraft={count} workers={workers} rounds={rounds} elapsed_us={:.3}",
                    start.elapsed().as_secs_f64() * 1e6 / f64::from(rounds),
                );
            }
        }
    }

    #[test]
    fn tracer_ribbon_is_finite_camera_facing_and_visible_end_on() {
        let mut camera = Camera::new();
        camera.position = [0., 0., -100.];
        camera.yaw = 0.;
        camera.pitch = 0.;
        for end in [[20., 0., 0.], [0., 0., 20.]] {
            let mut output = Vec::new();
            tracer(&mut output, [0.; 3], end, &camera, 1., 1.);
            assert_eq!(output.len(), 60);
            assert!(output.iter().all(|v| v.is_finite()));
            let points: Vec<[f32; 2]> = output.chunks_exact(10).map(|v| [v[0], v[1]]).collect();
            let a = [points[1][0] - points[0][0], points[1][1] - points[0][1]];
            let b = [points[2][0] - points[0][0], points[2][1] - points[0][1]];
            assert!((a[0] * b[1] - a[1] * b[0]).abs() > 0.1);
            assert!(output.chunks_exact(10).all(|v| v[5] == -8.));
        }
        let mut output = Vec::new();
        tracer(&mut output, [0.; 3], [0.; 3], &camera, 1., 1.);
        assert!(output.is_empty());
    }

    #[test]
    fn the_105_mm_tracer_is_half_as_bright_again_after_the_srgb_decode() {
        assert_eq!(tracer_brightness("C_105.JT"), 1.5);
        assert_eq!(tracer_brightness("C_25.JT"), 1.);
        assert_eq!(tracer_brightness("M61A1.GN"), 1.);
        // The shader decodes the vertex color from sRGB, as terrain.wgsl's
        // `linear` does; the ordinary tracer keeps its old white exactly.
        let decode = |c: f32| {
            let c = f64::from(c);
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        assert_eq!(encoded(1.), 1.);
        assert!((decode(encoded(1.5)) - 1.5).abs() < 1e-5);
        let camera = Camera::new();
        let mut output = Vec::new();
        tracer(&mut output, [0.; 3], [20., 0., 0.], &camera, 1.5, 1.);
        assert!(output.chunks_exact(10).all(|v| v[6] == encoded(1.5)));
    }

    #[test]
    fn the_105_mm_tracer_is_wider_than_the_others_and_nothing_else_changes() {
        assert_eq!(tracer_width("C_105.JT"), HOWITZER_TRACER_WIDTH);
        assert_eq!(tracer_width("C_40.JT"), 1.);
        assert_eq!(tracer_width("C_25.JT"), 1.);
        assert_eq!(tracer_width("M61A1.GN"), 1.);
        assert!((1.5..=2.).contains(&HOWITZER_TRACER_WIDTH), "John's range");
        // Seen side on, the ribbon's two edges are 2.4 feet apart at width 1
        // and that much times the width otherwise; its length is untouched.
        let mut camera = Camera::new();
        camera.position = [0., 0., -100.];
        let across = |width: f64| {
            let mut output = Vec::new();
            tracer(&mut output, [0.; 3], [20., 0., 0.], &camera, 1., width);
            let ys: Vec<f32> = output.chunks_exact(10).map(|v| v[1]).collect();
            let xs: Vec<f32> = output.chunks_exact(10).map(|v| v[0]).collect();
            let span = |v: &[f32]| {
                v.iter().cloned().fold(f32::MIN, f32::max)
                    - v.iter().cloned().fold(f32::MAX, f32::min)
            };
            (f64::from(span(&ys)), f64::from(span(&xs)))
        };
        let (narrow, length) = across(1.);
        let (wide, same_length) = across(HOWITZER_TRACER_WIDTH);
        assert!((narrow - 2.4).abs() < 1e-4, "{narrow}");
        assert!((wide / narrow - HOWITZER_TRACER_WIDTH).abs() < 1e-4);
        assert_eq!(length, same_length);
    }

    #[test]
    fn palette_mesh_does_not_turn_texture_only_exhaust_into_solid_faces() {
        let face = tore_formats::shape::Face {
            fog: tore_formats::shape::FogMode::Enabled,
            positions: vec![[0., 0., 0.], [3., 0., 0.], [0., 3., 0.]],
            colors: vec![1; 3],
            uv: vec![],
            texture: "SYNTHETIC.PIC".into(),
            subtype: 0x61,
            normal: None,
            address: 0,
        };
        let mut exhaust = face.clone();
        exhaust.subtype = 0x4c;
        let shape = Shape {
            billboards: Vec::new(),
            lines: vec![],
            faces: vec![face, exhaust],
            state_words: Default::default(),
        };
        let mut out = vec![];
        mesh(
            &mut out,
            &shape,
            [0.; 3],
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            &[[255; 3]; 256],
        );
        // One triangle of ten-float vertices; the exhaust face is omitted.
        assert_eq!(out.len(), 30);
    }
}
