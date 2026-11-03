//! Original explosion, fire and crater artwork as textured sprites. The
//! simulation owns every effect's type, place and life; the sheets, frame
//! layouts and sizes are in docs/spec/explosions.md.
//!
//! Flak bursts draw `FLAKA` (the 85 mm shell's type 27, two seconds) or the
//! larger `FLAKB` (the 100 mm shell's type 28, one second): the simulation
//! picks the type from the shell's record. The light and dark puff a burst
//! leaves are `surface_fx.rs`'s.
//!
//! An impact is drawn at its weapon's size against its explosion type's
//! (the effect's `scale`, docs/spec/explosions.md, "Size by weapon").
//!
//! A large ground explosion also throws out a shockwave: a low skirt of
//! `SMOKE.PIC` dust, tan in dry air and pale in humid (or white spray on
//! water), that races outward from the blast and fades
//! (docs/spec/explosions.md, "Shockwave"). It is drawn from the effect and
//! the mission's climate alone, so replays and every networked client show
//! it too. The vapor and the lingering dust are `blast_climate`'s.
use crate::gpu_setup::NamedSteps as _;
pub mod cluster;

use crate::camera::Camera;
use crate::snapshot::{EffectPose, MarkPose};
use std::collections::BTreeMap;
use tore_formats::Pic;
use tore_sim::combat::{
    blast::{self, MarkKind},
    live::EffectKind,
};

/// Explosions, fires and craters drawn at once.
const MAX_INSTANCES: usize = 4096;
/// Craters and pock marks drawn at once at most, the nearest: the rest of
/// [`MAX_INSTANCES`] stays free for explosions and fires, however many
/// marks a long mission leaves (agent decision, 2026-10-12).
pub const MAX_MARK_INSTANCES: usize = 3072;
/// A flat mark is drawn out to this many times its half width, at most
/// [`MARK_DRAW_FT`]: a 5 ft gun pock mark to 10,000 ft, where it is half a
/// pixel across, a 500 lb bomb's crater to about 95 nmi (agent decision).
pub const MARK_DRAW_WIDTHS: f64 = 4_000.;
pub const MARK_DRAW_FT: f64 = 2_200_000.;
/// Center relative to the eye, extent, mode, art cell, layer, opacity,
/// emissive flag, tint and four ground-corner height offsets.
const INSTANCE_BYTES: usize = 20 * 4;
/// Every sheet is 256 wide; the tallest, GRNDMED3, is 386 rows.
const SHEET_WIDTH: usize = 256;
const SHEET_HEIGHT: usize = 386;

/// One animation sheet's frame layout as its shape describes it: the
/// frame size, the first frame's corner, the gap between frames, columns
/// and frame count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layout {
    width: u16,
    height: u16,
    x: u16,
    y: u16,
    gap_x: u16,
    gap_y: u16,
    columns: u16,
    frames: u16,
}
const fn layout(size: [u16; 8]) -> Layout {
    Layout {
        width: size[0],
        height: size[1],
        x: size[2],
        y: size[3],
        gap_x: size[4],
        gap_y: size[5],
        columns: size[6],
        frames: size[7],
    }
}
impl Layout {
    /// Pixel rectangle of one frame: corner, then size.
    fn cell(self, frame: u16) -> [f32; 4] {
        let frame = frame.min(self.frames - 1);
        let (column, row) = (frame % self.columns, frame / self.columns);
        [
            f32::from(self.x + column * (self.width + self.gap_x)),
            f32::from(self.y + row * (self.height + self.gap_y)),
            f32::from(self.width),
            f32::from(self.height),
        ]
    }
}

/// EXP.SH's sheets in its own order, then FIRE.SH's and CRATER.SH's, then
/// the smoke puffs the shockwave ring is made of.
const SHEETS: [(&str, Layout); 26] = [
    ("AIRSML.PIC", layout([78, 50, 1, 0, 2, 2, 3, 12])),
    ("AIRMED.PIC", layout([78, 64, 1, 0, 2, 2, 3, 12])),
    ("AIRMED2.PIC", layout([78, 64, 1, 0, 2, 2, 3, 12])),
    ("AIRMED3.PIC", layout([68, 58, 1, 0, 2, 2, 3, 15])),
    ("AIRLRG.PIC", layout([78, 56, 1, 0, 2, 2, 3, 12])),
    ("GRNDSML.PIC", layout([78, 64, 1, 0, 2, 2, 3, 12])),
    ("GRNDMED.PIC", layout([78, 64, 1, 0, 2, 2, 3, 12])),
    ("GRNDMED3.PIC", layout([67, 75, 1, 0, 2, 2, 3, 15])),
    ("GRNDLRG.PIC", layout([62, 60, 1, 0, 2, 2, 3, 15])),
    ("GRNDLRG2.PIC", layout([78, 64, 1, 0, 2, 2, 3, 12])),
    ("WATSML.PIC", layout([35, 59, 1, 0, 2, 2, 3, 6])),
    ("WATLRG.PIC", layout([78, 64, 1, 0, 2, 2, 3, 12])),
    ("FLAKA.PIC", layout([56, 47, 8, 8, 1, 1, 4, 28])),
    ("FLAKB.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("FLAKC.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("EMPEX.PIC", layout([78, 64, 1, 0, 2, 2, 3, 12])),
    ("AIRLRGAG.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("AIRLRGC.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("AIRLRGD.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("AIRSMLA.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("AIRSMLB2.PIC", layout([76, 62, 1, 0, 1, 1, 3, 12])),
    ("GRDLRGA.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("DIRTEXP.PIC", layout([76, 62, 4, 0, 1, 1, 3, 12])),
    ("FIREA.PIC", layout([76, 62, 4, 0, 1, 1, 3, 15])),
    ("CRATERS.PIC", layout([78, 66, 1, 0, 2, 0, 3, 3])),
    // Dark, grey and white puffs, 44 pixels square (the smoke renderer's
    // cells).
    ("SMOKE.PIC", layout([44, 43, 1, 0, 3, 0, 3, 3])),
];
const FIRE_SHEET: usize = 23;
const CRATER_SHEET: usize = 24;
const DEBRIS_SHEET: usize = 21;
const SMOKE_SHEET: usize = 25;

/// The white spray ring of a large water explosion (type 34; agent
/// design, X1, 2026-10-10, kept as it was by John on 2026-10-12): `PUFFS`
/// puffs on a ring that grows from the blast to `REACH` times the
/// explosion's drawn width over `GROW_TICKS`, easing out as a blast wave
/// slows, then drifts out a further `DRIFT` while it fades over the rest of
/// the explosion's life. Each puff grows from `PUFF_START` to `PUFF_END` of
/// the width across.
mod spray {
    pub const PUFFS: usize = 32;
    pub const REACH: f64 = 1.6;
    pub const DRIFT: f64 = 0.15;
    pub const GROW_TICKS: f64 = 84.;
    pub const PUFF_START: f64 = 0.12;
    pub const PUFF_END: f64 = 0.5;
    pub const OPACITY: f64 = 0.85;
    /// SMOKE.PIC's white puff.
    pub const CELL: u16 = 2;
}

/// The shockwave of a large land explosion (the shockwave overhaul, slice
/// B12, agent design within John's request of 2026-10-12; first version X1,
/// 2026-10-10): a low skirt of dust racing out from the blast, two
/// staggered rows of `SMOKE.PIC`'s grey puff, `OUTER` on the leading edge
/// and `INNER` behind it at `INNER_AT` of its radius, each puff squat
/// (`SQUAT` as tall as wide) and of its own size and place within
/// `SIZE_JITTER` and `RADIUS_JITTER`. The edge reaches `REACH` times the
/// explosion's drawn width in `GROW_TICKS` (a cubic ease-out, as a blast
/// wave slows), drifts a further `DRIFT` while it fades over the rest of the
/// explosion's life, and the puffs grow from `PUFF_START` to `PUFF_END` of
/// the width across. Coloured by the climate: tan in dry air, pale grey in
/// humid ([`crate::blast_climate::Climate::ring_tint`]).
mod shockwave {
    pub const OUTER: usize = 24;
    pub const INNER: usize = 12;
    pub const INNER_AT: f64 = 0.62;
    pub const REACH: f64 = 1.35;
    pub const DRIFT: f64 = 0.2;
    pub const GROW_TICKS: f64 = 60.;
    pub const PUFF_START: f64 = 0.14;
    pub const PUFF_END: f64 = 0.46;
    pub const SQUAT: f64 = 0.55;
    pub const SIZE_JITTER: f64 = 0.25;
    pub const RADIUS_JITTER: f64 = 0.08;
    pub const OPACITY: f64 = 0.8;
    pub const INNER_OPACITY: f64 = 0.5;
    pub const FADE_IN_TICKS: f64 = 4.;
    /// SMOKE.PIC's grey puff.
    pub const CELL: u16 = 1;
}

/// The ring an explosion type throws: dust for the large land types (21 to
/// 23, 35 to 37), white spray for the large water type (34); none for any
/// other type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ring {
    Dust,
    Spray,
}
fn ring_of(kind: u8) -> Option<Ring> {
    match kind {
        21..=23 | 35..=37 => Some(Ring::Dust),
        34 => Some(Ring::Spray),
        _ => None,
    }
}

/// The shockwave ring of one explosion `elapsed` ticks into its `duration`,
/// `width` feet across, standing on `position`, in `climate`.
fn shockwave(
    kind: u8,
    position: [f64; 3],
    width: f64,
    (elapsed, duration): (u16, u16),
    climate: crate::blast_climate::Climate,
    out: &mut Vec<Sprite>,
) {
    match ring_of(kind) {
        Some(Ring::Dust) => dust_ring(position, width, elapsed, duration, climate, out),
        Some(Ring::Spray) => spray_ring(position, width, elapsed, duration, out),
        None => {}
    }
}

fn dust_ring(
    position: [f64; 3],
    width: f64,
    elapsed: u16,
    duration: u16,
    climate: crate::blast_climate::Climate,
    out: &mut Vec<Sprite>,
) {
    use shockwave::*;
    let (_, layout) = SHEETS[SMOKE_SHEET];
    let t = f64::from(elapsed);
    let life = f64::from(duration.max(1));
    let grow = (t / GROW_TICKS).min(1.);
    let eased = 1. - (1. - grow).powi(3);
    let drift = ((t - GROW_TICKS).max(0.) / (life - GROW_TICKS).max(1.)).min(1.);
    let edge = width * (REACH * eased + DRIFT * drift);
    let fade = (t / FADE_IN_TICKS).min(1.) * (1. - t / life).clamp(0., 1.).powf(1.5);
    if fade <= 0. || edge <= 0. {
        return;
    }
    let tint = climate.ring_tint();
    let turn = f64::from(blast::pick(position, 4, 360)).to_radians();
    let puff = width * (PUFF_START + (PUFF_END - PUFF_START) * grow.sqrt()) / 2.;
    let aspect = f64::from(layout.height) / f64::from(layout.width);
    for (row, count, at, opacity, offset) in [
        (0_u64, OUTER, 1., OPACITY, 0.),
        (1, INNER, INNER_AT, INNER_OPACITY, 0.5),
    ] {
        for n in 0..count {
            let salt = 40 + row * 100 + n as u64 * 2;
            let jitter = |k: u64| f64::from(blast::pick(position, salt + k, 2001)) / 1000. - 1.;
            let angle = turn + std::f64::consts::TAU * (n as f64 + offset) / count as f64;
            let radius = edge * at * (1. + RADIUS_JITTER * jitter(0));
            // The inner row's puffs are larger and fainter: the dust behind
            // the edge.
            let half = puff * (1. + SIZE_JITTER * jitter(1)) * if row == 0 { 1. } else { 1.3 };
            out.push(Sprite {
                position: [
                    position[0] + radius * angle.cos(),
                    position[1],
                    position[2] + radius * angle.sin(),
                ],
                extent: [half, half * 2. * aspect * SQUAT],
                mode: Mode::Standing,
                cell: layout.cell(CELL),
                layer: SMOKE_SHEET,
                opacity: (opacity * fade) as f32,
                emissive: false,
                tint,
                glow: 0.,
                heat: 0.,
                fresh: 0.,
                ember: 0.,
            });
        }
    }
}

fn spray_ring(position: [f64; 3], width: f64, elapsed: u16, duration: u16, out: &mut Vec<Sprite>) {
    use spray::*;
    let (_, layout) = SHEETS[SMOKE_SHEET];
    let t = f64::from(elapsed);
    let life = f64::from(duration.max(1));
    let grow = (t / GROW_TICKS).min(1.);
    let drift = ((t - GROW_TICKS).max(0.) / (life - GROW_TICKS).max(1.)).min(1.);
    let radius = width * (REACH * (1. - (1. - grow).powi(2)) + DRIFT * drift);
    let half = width * (PUFF_START + (PUFF_END - PUFF_START) * grow.sqrt()) / 2.;
    let opacity = OPACITY * (1. - t / life).clamp(0., 1.).powf(1.5);
    if opacity <= 0. || radius <= 0. {
        return;
    }
    // Each blast's ring is turned by its own repeatable amount.
    let turn = f64::from(blast::pick(position, 4, 360)).to_radians();
    for n in 0..PUFFS {
        let angle = turn + std::f64::consts::TAU * n as f64 / PUFFS as f64;
        out.push(Sprite {
            position: [
                position[0] + radius * angle.cos(),
                position[1],
                position[2] + radius * angle.sin(),
            ],
            extent: [
                half,
                half * 2. * f64::from(layout.height) / f64::from(layout.width),
            ],
            mode: Mode::Standing,
            cell: layout.cell(CELL),
            layer: SMOKE_SHEET,
            opacity: opacity as f32,
            emissive: false,
            tint: PLAIN,
            glow: 0.,
            heat: 0.,
            fresh: 0.,
            ember: 0.,
        });
    }
}

/// The sheet EXP.SH draws for each explosion type, 15 to 38.
const EXPLOSION_SHEETS: [usize; 24] = [
    5, 22, 10, 0, 19, 20, 6, 6, 7, 1, 2, 3, 12, 13, 14, 4, 16, 17, 18, 11, 8, 9, 21, 15,
];

fn explosion_sheet(kind: u8) -> Option<usize> {
    EXPLOSION_SHEETS
        .get(usize::from(kind.checked_sub(blast::FIRST)?))
        .copied()
}

/// The imported sheets, loaded once. A sheet the cache lacks draws nothing.
pub struct Art {
    layers: Vec<u8>,
    present: [bool; SHEETS.len()],
}
impl Art {
    pub fn load(data: &BTreeMap<String, Vec<u8>>) -> Self {
        let mut layers = vec![255; SHEETS.len() * SHEET_WIDTH * SHEET_HEIGHT];
        let mut present = [false; SHEETS.len()];
        for (index, (name, _)) in SHEETS.iter().enumerate() {
            let Some(pic) = data.get(*name).and_then(|bytes| Pic::parse(bytes).ok()) else {
                log::warn!("Explosion art {name} unavailable; that effect is not drawn");
                continue;
            };
            if pic.width != SHEET_WIDTH || pic.height > SHEET_HEIGHT || !pic.palette.is_empty() {
                log::warn!("Explosion art {name} has an unreviewed layout");
                continue;
            }
            let layer = &mut layers[index * SHEET_WIDTH * SHEET_HEIGHT..];
            for (i, (&pixel, &visible)) in pic.pixels.iter().zip(&pic.mask).enumerate() {
                layer[i] = if visible { pixel } else { 255 };
            }
            present[index] = true;
        }
        Self { layers, present }
    }
    /// No sheets: nothing is drawn.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            layers: vec![255; SHEETS.len() * SHEET_WIDTH * SHEET_HEIGHT],
            present: [false; SHEETS.len()],
        }
    }
    #[cfg(test)]
    fn synthetic() -> Self {
        Self {
            layers: vec![7; SHEETS.len() * SHEET_WIDTH * SHEET_HEIGHT],
            present: [true; SHEETS.len()],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Faces the camera, centered on its point.
    Billboard = 0,
    /// Faces the camera with its base on its point.
    Standing = 1,
    /// Lies flat on the ground.
    Flat = 2,
}

/// One sprite to draw.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sprite {
    position: [f64; 3],
    /// Half width, then half height (or full height when standing).
    extent: [f64; 2],
    mode: Mode,
    cell: [f32; 4],
    layer: usize,
    opacity: f32,
    emissive: bool,
    /// Multiplies the art's colour: [`PLAIN`], or a shockwave's dust colour.
    tint: [f32; 3],
    /// Its night glow in flares, 0 for none, and how white-hot it is
    /// ([`crate::night_glow`]).
    glow: f32,
    heat: f32,
    /// Sustained fireball light, in flares, excluding the initial flash.
    ember: f32,
    /// A crater's freshness for the thermal picture, 1 new to 0 cold
    /// ([`thermal_freshness`]); 0 for everything else.
    fresh: f32,
}

/// Receiver height for decals, using the terrain's triangle split in f64.
/// Converting million-foot world coordinates to f32 before sampling can bury
/// a corner by hundredths of a foot, larger than the raster tie break nearby.
/// Pavement overrides terrain; roofs and vehicles are deliberately excluded.
fn ground_height(world: &crate::terrain::Terrain, x: f64, z: f64) -> f64 {
    if let Some((_, height)) = world.airport_scene.runway_surface(x, z) {
        return height;
    }
    use tore_formats::theater::{CELL_FEET, HEIGHT_FEET};
    let t = &world.theater;
    let fx = (x / f64::from(CELL_FEET)).clamp(0., (t.cols - 1) as f64 - 0.001);
    let fz = (z / f64::from(CELL_FEET)).clamp(0., (t.rows - 1) as f64 - 0.001);
    let (ix, iz) = (fx as usize, fz as usize);
    let (u, v) = (fx - ix as f64, fz - iz as f64);
    let h = |dx, dz| f64::from(t.cell(ix + dx, iz + dz).elevation) * f64::from(HEIGHT_FEET);
    if u + v <= 1. {
        h(0, 0) + (h(1, 0) - h(0, 0)) * u + (h(0, 1) - h(0, 0)) * v
    } else {
        h(1, 1) + (h(0, 1) - h(1, 1)) * (1. - u) + (h(1, 0) - h(1, 1)) * (1. - v)
    }
}

/// Corner heights relative to the mark centre, in UV order 00, 10, 11, 01.
/// Only ground marks sample the receiver; standing flames and billboards
/// retain their original geometry. No camera-dependent lift is added.
fn ground_corners(sprite: &Sprite, ground: &impl Fn(f64, f64) -> f64) -> [f32; 4] {
    if sprite.mode != Mode::Flat {
        return [0.; 4];
    }
    [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]].map(|[x, z]| {
        (ground(
            sprite.position[0] + x * sprite.extent[0],
            sprite.position[2] + z * sprite.extent[1],
        ) - sprite.position[1]) as f32
    })
}

/// The GPU instance, shared with the offscreen regression fixture.
fn instance_values(sprite: &Sprite, eye: [f64; 3], ground: &impl Fn(f64, f64) -> f64) -> [f32; 20] {
    let offset: [f32; 3] = std::array::from_fn(|i| (sprite.position[i] - eye[i]) as f32);
    let heights = ground_corners(sprite, ground);
    [
        offset[0],
        offset[1],
        offset[2],
        sprite.extent[0] as f32,
        sprite.extent[1] as f32,
        sprite.mode as u8 as f32,
        sprite.cell[0],
        sprite.cell[1],
        sprite.cell[2],
        sprite.cell[3],
        sprite.layer as f32,
        sprite.opacity,
        if sprite.emissive { 1. } else { -sprite.fresh },
        sprite.tint[0],
        sprite.tint[1],
        sprite.tint[2],
        heights[0],
        heights[1],
        heights[2],
        heights[3],
    ]
}

/// A crater cools in the thermal picture over this many seconds (agent
/// decision, nr-fltv: long enough to see where the rounds went on the next
/// pass of an orbit).
pub const CRATER_COOLS_S: f64 = 180.;

/// A crater's freshness after `age` ticks: 1 new, falling to 0 over
/// [`CRATER_COOLS_S`].
fn thermal_freshness(age: u64) -> f32 {
    (1. - age as f64 / (CRATER_COOLS_S * 120.)).clamp(0., 1.) as f32
}

/// Drawn in the art's own colours.
const PLAIN: [f32; 3] = [1.; 3];
impl Sprite {
    fn radius(&self) -> f64 {
        self.extent[0].max(self.extent[1]) * 2.
    }
}

/// Legacy recordings stored no explosion type: their kind's family, over
/// its old 45- or 240-tick life.
fn legacy(kind: EffectKind) -> Option<(u8, u16)> {
    match kind {
        EffectKind::Hit => Some((18, 45)),
        EffectKind::Ground => Some((15, 45)),
        EffectKind::Destroyed => Some((blast::AIRCRAFT, 240)),
        EffectKind::Flak => Some((27, 240)),
        _ => None,
    }
}

/// Every explosion, crater and fire in one picture, craters first, the
/// shockwave rings coloured by `climate`.
fn sprites(
    art: &Art,
    effects: &[EffectPose],
    marks: &[MarkPose],
    climate: crate::blast_climate::Climate,
) -> Vec<Sprite> {
    let mut out = Vec::new();
    for mark in marks {
        let sprite = match mark.kind {
            MarkKind::Crater(size) => {
                let (_, layout) = SHEETS[CRATER_SHEET];
                // At its weapon's scale; a gun round's pock mark is a small
                // crater (John, 2026-10-12).
                let half = blast::mark_half_width(size, mark.scale);
                Sprite {
                    position: mark.position,
                    extent: [half, half],
                    mode: Mode::Flat,
                    cell: layout.cell(u16::from(blast::crater_style(mark.position))),
                    layer: CRATER_SHEET,
                    opacity: 1.,
                    emissive: false,
                    tint: PLAIN,
                    glow: 0.,
                    heat: 0.,
                    fresh: thermal_freshness(mark.age),
                    ember: 0.,
                }
            }
            MarkKind::Fire => {
                let (_, layout) = SHEETS[FIRE_SHEET];
                // FIRE.SH loops its 15 frames once a second.
                let frame = (mark.age % 120) * u64::from(layout.frames) / 120;
                let width = f64::from(blast::FIRE_SIZE);
                Sprite {
                    position: mark.position,
                    extent: [
                        width / 2.,
                        width * f64::from(layout.height) / f64::from(layout.width),
                    ],
                    mode: Mode::Standing,
                    cell: layout.cell(frame as u16),
                    layer: FIRE_SHEET,
                    opacity: mark.strength,
                    emissive: true,
                    tint: PLAIN,
                    // Refit to the unit's width in `update`.
                    glow: (crate::night_glow::fire_light(width) * f64::from(mark.strength)) as f32,
                    heat: 0.,
                    fresh: 0.,
                    ember: 0.,
                }
            }
        };
        if art.present[sprite.layer] {
            out.push(sprite);
        }
    }
    for effect in effects {
        let (kind, duration, width, sheet) = if effect.kind == EffectKind::DebrisImpact {
            (0, 45, 15., DEBRIS_SHEET)
        } else if let Some(kind) = effect.blast {
            let Some(row) = blast::explosion(kind) else {
                continue;
            };
            (
                kind,
                u16::from(row.seconds) * 120,
                f64::from(blast::rolled_size(kind, effect.position)),
                explosion_sheet(kind).unwrap_or(0),
            )
        } else if let Some((kind, duration)) = legacy(effect.kind) {
            (
                kind,
                duration,
                f64::from(blast::rolled_size(kind, effect.position)),
                explosion_sheet(kind).unwrap_or(0),
            )
        } else {
            continue;
        };
        // The weapon's size (bugs B8 to B10): one factor for the fireball and
        // its shockwave alike, so a later shockwave pass inherits it.
        let width = if effect.kind == EffectKind::DebrisImpact {
            width
        } else {
            width * tore_sim::combat::blast_scale::factor(effect.scale)
        };
        if !art.present[sheet] || width <= 0. {
            continue;
        }
        if art.present[SMOKE_SHEET] && effect.kind != EffectKind::DebrisImpact {
            shockwave(
                kind,
                effect.position,
                width,
                (duration.saturating_sub(effect.ticks), duration),
                climate,
                &mut out,
            );
        }
        let (_, layout) = SHEETS[sheet];
        let elapsed = duration.saturating_sub(effect.ticks);
        let frame = u32::from(elapsed) * u32::from(layout.frames) / u32::from(duration.max(1));
        let aspect = f64::from(layout.height) / f64::from(layout.width);
        let standing = blast::explosion(kind).is_some_and(|row| row.surface)
            || effect.kind == EffectKind::DebrisImpact;
        let (glow, heat) = if effect.kind == EffectKind::DebrisImpact {
            (0., 0.)
        } else {
            let detonates = crate::impact_fx::detonation(effect).is_some()
                || matches!(effect.kind, EffectKind::Flak);
            let glow = crate::night_glow::explosion_glow(
                effect.position,
                detonates.then_some(kind),
                effect.scale,
                width,
                elapsed,
                duration,
            );
            let flash = 1. - f64::from(elapsed) / f64::from(duration.max(1));
            (glow as f32, (flash * flash) as f32)
        };
        out.push(Sprite {
            position: effect.position,
            extent: if standing {
                [width / 2., width * aspect]
            } else {
                [width / 2., width * aspect / 2.]
            },
            mode: if standing {
                Mode::Standing
            } else {
                Mode::Billboard
            },
            cell: layout.cell(frame as u16),
            layer: sheet,
            opacity: 1.,
            emissive: true,
            tint: PLAIN,
            glow,
            heat,
            fresh: 0.,
            ember: if crate::impact_fx::detonation(effect).is_some()
                || matches!(effect.kind, EffectKind::Flak)
            {
                crate::night_glow::explosion_ember(
                    width,
                    tore_sim::combat::blast_scale::factor(effect.scale),
                    elapsed,
                    duration,
                ) as f32
            } else {
                0.
            },
        });
        bomblets(art, effect, elapsed, &mut out);
    }
    out
}

/// The small ground flashes of a cluster burst that are playing `elapsed`
/// ticks into it (docs/spec/ordnance-presentation.md, "Cluster bombs").
fn bomblets(art: &Art, effect: &EffectPose, elapsed: u16, out: &mut Vec<Sprite>) {
    for flash in cluster::derive(effect) {
        let Some(sheet) = explosion_sheet(flash.kind).filter(|&sheet| art.present[sheet]) else {
            continue;
        };
        let Some(played) = elapsed
            .checked_sub(flash.start)
            .filter(|played| *played < cluster::FLASH_TICKS)
        else {
            continue;
        };
        let (_, layout) = SHEETS[sheet];
        let frame = u32::from(played) * u32::from(layout.frames) / u32::from(cluster::FLASH_TICKS);
        out.push(Sprite {
            position: flash.position,
            extent: [
                flash.width / 2.,
                flash.width * f64::from(layout.height) / f64::from(layout.width),
            ],
            mode: Mode::Standing,
            cell: layout.cell(frame as u16),
            layer: sheet,
            opacity: 1.,
            emissive: true,
            tint: PLAIN,
            // A bomblet's flash as an ember of its width, fading as it plays.
            glow: (crate::night_glow::EMBER_SHARE * crate::impact_fx::light_of(flash.width, 1.).0
                / crate::countermeasure_renderer::FLARE_STRENGTH
                * (1. - f64::from(played) / f64::from(cluster::FLASH_TICKS)).powi(2))
                as f32,
            heat: 0.5,
            fresh: 0.,
            ember: 0.,
        });
    }
}

pub struct EffectRenderer {
    pipeline: wgpu::RenderPipeline,
    flat_pipeline: wgpu::RenderPipeline,
    flat_count: u32,
    bind: Option<wgpu::BindGroup>,
    buffer: wgpu::Buffer,
    sprites: Vec<Sprite>,
    /// The fires that fit their unit: where each burns and how wide it is
    /// drawn, in feet.
    fires: Vec<([f64; 3], f64)>,
    /// The burning city buildings' fire sprites this frame
    /// ([`crate::city_fires`]), replaced by every [`Self::set_city_fires`].
    city: Vec<Sprite>,
    /// Whether the fire sheet is in the art.
    fire_present: bool,
    count: u32,
    /// Burning city buildings past the fire sprites' reach or budget, drawn
    /// as glow points only ([`crate::city_fires::Frame::glows`]).
    far_city: Vec<crate::night_glow::Glow>,
    /// This frame's glow points and sustained surface lights, built by `update` ([`crate::night_glow`]).
    glows: Vec<crate::night_glow::Glow>,
    sustained_lights: Vec<crate::countermeasure_renderer::FlareLight>,
    glow_on: bool,
    /// This frame's sprites with their fires fitted, and those sprites'
    /// glows and lights, shared by every view until the next `prepare` or
    /// `fit_fires`.
    fitted: Option<FittedSprites>,
}

type FittedSprites = (
    Vec<Sprite>,
    Vec<crate::night_glow::Glow>,
    Vec<crate::countermeasure_renderer::FlareLight>,
);

/// What the camera at `eye` draws, in drawing order: the nearest
/// [`MAX_MARK_INSTANCES`] flat marks within their draw distance, far to
/// near, then everything else far to near, [`MAX_INSTANCES`] in all.
fn visible<'a>(sprites: impl IntoIterator<Item = &'a Sprite>, eye: [f64; 3]) -> Vec<&'a Sprite> {
    let distance = |s: &Sprite| -> f64 {
        let offset: [f64; 3] = std::array::from_fn(|i| s.position[i] - eye[i]);
        offset.iter().map(|v| v * v).sum::<f64>().sqrt()
    };
    let mut flat: Vec<(f64, &Sprite)> = Vec::new();
    let mut rest: Vec<(f64, &Sprite)> = Vec::new();
    for s in sprites {
        let d = distance(s);
        if s.mode == Mode::Flat {
            let reach = (s.extent[0].max(s.extent[1]) * MARK_DRAW_WIDTHS).min(MARK_DRAW_FT);
            if d - s.radius() < reach {
                flat.push((d, s));
            }
        } else if d - s.radius() < MARK_DRAW_FT {
            rest.push((d, s));
        }
    }
    if flat.len() > MAX_MARK_INSTANCES {
        flat.select_nth_unstable_by(MAX_MARK_INSTANCES - 1, |a, b| a.0.total_cmp(&b.0));
        flat.truncate(MAX_MARK_INSTANCES);
    }
    flat.sort_by(|a, b| b.0.total_cmp(&a.0));
    rest.sort_by(|a, b| b.0.total_cmp(&a.0));
    let room = MAX_INSTANCES - flat.len();
    // Far to near, so when explosions outnumber the room the nearest stay.
    let skip = rest.len().saturating_sub(room);
    flat.into_iter()
        .chain(rest.into_iter().skip(skip))
        .map(|(_, s)| s)
        .collect()
}

/// The glow points of the glowing sprites, at the middle of their art, and
/// the sustained fire and fireball light that reaches nearby surfaces.
fn glows_of(
    sprites: &[Sprite],
) -> (
    Vec<crate::night_glow::Glow>,
    Vec<crate::countermeasure_renderer::FlareLight>,
) {
    let mut glows = Vec::new();
    let mut fires = Vec::new();
    for s in sprites.iter().filter(|s| s.glow > 0.) {
        let lift = match s.mode {
            Mode::Standing => s.extent[1] / 2.,
            _ => 0.,
        };
        let width = 2. * s.extent[0];
        glows.push(crate::night_glow::Glow {
            position: [s.position[0], s.position[1] + lift, s.position[2]],
            width,
            strength: f64::from(s.glow),
            heat: s.heat,
        });
        if s.layer == FIRE_SHEET {
            fires.push(crate::countermeasure_renderer::FlareLight {
                sustained: true,
                radius: width / 2.,
                position: [
                    s.position[0],
                    s.position[1] + width * crate::night_glow::FIRE_LIGHT_RISE,
                    s.position[2],
                ],
                strength: f64::from(s.glow) * crate::countermeasure_renderer::FLARE_STRENGTH,
            });
        } else if s.ember > 0. {
            fires.push(crate::countermeasure_renderer::FlareLight {
                sustained: false,
                radius: width / 2.,
                position: [s.position[0], s.position[1] + lift, s.position[2]],
                strength: f64::from(s.ember) * crate::countermeasure_renderer::FLARE_STRENGTH,
            });
        }
    }
    (glows, fires)
}

/// A fire this near a fitted fire's spot (feet, level) is that fire.
const FIRE_FIT_FT: f64 = 2.;

/// Raster tolerance that lets a flat mark win against the independently
/// triangulated terrain it lies on, never a world-space lift.
///
/// The depth buffer is `Depth32Float` with reversed Z on every backend, so
/// the constant is in units of the float's own resolution at the primitive:
/// sixteen of them are a fixed fraction of the mark's depth (about 2e-6),
/// far under a vehicle's six feet at any range, and cover the rounding
/// between coplanar triangles seen head on. They do not cover a tilted
/// receiver: two triangulations of one slope disagree by more as depth
/// changes faster across a pixel (a hillside, a grazing view, a long zoom),
/// which lost one triangle of a crater on a 15 degree slope. The slope term
/// adds half of the mark's steepest per-pixel depth change, so the lift is
/// under one pixel's footprint of height at the mark at any range and zoom.
/// Fitted by sweeping both terms in `depth_tests` (every case passes with
/// the slope term from 0.1 to 4.5; 0.5 sits well inside, and above 5 a roof
/// six feet up is covered at grazing views).
const GROUND_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: 16,
    slope_scale: 0.5,
    clamp: 0.,
};

/// `sprites` with every fire sprite that stands on one of `fires` drawn at
/// that fire's width (the fire's sheet is as wide as it is tall by the
/// layout's aspect).
fn fit_fires(sprites: &[Sprite], fires: &[([f64; 3], f64)]) -> Vec<Sprite> {
    let (_, layout) = SHEETS[FIRE_SHEET];
    sprites
        .iter()
        .map(|sprite| {
            let mut sprite = *sprite;
            if sprite.layer == FIRE_SHEET
                && let Some((_, width)) = fires.iter().find(|(at, _)| {
                    (at[0] - sprite.position[0]).hypot(at[2] - sprite.position[2]) <= FIRE_FIT_FT
                })
            {
                // Its glow by its own width, at the strength its art shows.
                if sprite.glow > 0. {
                    let shown = f64::from(sprite.glow)
                        / crate::night_glow::fire_light(f64::from(blast::FIRE_SIZE));
                    sprite.glow = (crate::night_glow::fire_light(*width) * shown) as f32;
                }
                sprite.extent = [
                    width / 2.,
                    width * f64::from(layout.height) / f64::from(layout.width),
                ];
            }
            sprite
        })
        .collect()
}
/// The burning city buildings' fire sprites ([`crate::city_fires`]): each a
/// standing fire of its width, its flames' frame turned by `tick` and its own
/// phase so neighbours do not flicker in step.
fn city_sprites(fires: &[crate::city_fires::CityFire], tick: u64) -> Vec<Sprite> {
    let (_, layout) = SHEETS[FIRE_SHEET];
    fires
        .iter()
        .map(|fire| {
            let frame = ((tick + fire.phase) % 120) * u64::from(layout.frames) / 120;
            Sprite {
                position: fire.position,
                extent: [
                    fire.width / 2.,
                    fire.width * f64::from(layout.height) / f64::from(layout.width),
                ],
                mode: Mode::Standing,
                cell: layout.cell(frame as u16),
                layer: FIRE_SHEET,
                opacity: 1.,
                emissive: true,
                tint: PLAIN,
                glow: crate::night_glow::fire_light(fire.width) as f32,
                heat: 0.,
                fresh: 0.,
                ember: 0.,
            }
        })
        .collect()
}
impl EffectRenderer {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        shader: &wgpu::ShaderModule,
        samples: u32,
    ) -> Self {
        let (pipeline, flat_pipeline) = Self::pipelines(device, format, shader, samples);
        Self {
            pipeline,
            flat_pipeline,
            flat_count: 0,
            bind: None,
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Bounded explosion sprites"),
                size: (MAX_INSTANCES * INSTANCE_BYTES) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            sprites: Vec::new(),
            fires: Vec::new(),
            city: Vec::new(),
            fire_present: false,
            count: 0,
            far_city: Vec::new(),
            glows: Vec::new(),
            sustained_lights: Vec::new(),
            glow_on: crate::night_glow::enabled(),
            fitted: None,
        }
    }
    /// The fires of destroyed units and the width each is drawn at, for the
    /// next `update`.
    pub fn fit_fires(&mut self, fires: &[([f64; 3], f64)]) {
        self.fires = fires.to_vec();
        self.fitted = None;
    }
    /// The burning city buildings' fires for this frame, `tick` the
    /// presentation tick that turns their flames' frames. Replaces the last
    /// frame's, so drawing a frame twice (a mirror, a second view) does not
    /// double them.
    pub fn set_city_fires(&mut self, fires: &[crate::city_fires::CityFire], tick: u64) {
        self.city = if self.fire_present {
            city_sprites(fires, tick)
        } else {
            Vec::new()
        };
    }
    /// The burning city buildings drawn only as glow points this frame.
    pub fn set_far_city(&mut self, glows: Vec<crate::night_glow::Glow>) {
        self.far_city = glows;
    }
    /// This frame's glow points, after `update`.
    pub fn glows(&self) -> &[crate::night_glow::Glow] {
        &self.glows
    }
    /// Sustained fire and fireball lights nearest `eye`, after `update`.
    pub fn sustained_lights(
        &self,
        eye: [f64; 3],
        night: bool,
    ) -> Vec<crate::countermeasure_renderer::FlareLight> {
        if night {
            // Keep the existing three-mile effect-light range, including embers.
            let nearby: Vec<_> = self
                .sustained_lights
                .iter()
                .copied()
                .filter(|light| {
                    (0..3)
                        .map(|i| (light.position[i] - eye[i]).powi(2))
                        .sum::<f64>()
                        <= crate::night_glow::FIRE_LIGHT_FT.powi(2)
                })
                .collect();
            crate::night_glow::night_lights(&nearby, eye, crate::night_glow::MAX_FIRE_LIGHTS)
        } else {
            crate::night_glow::nearby_lights(&self.sustained_lights, eye)
        }
    }
    /// Rebuild for a new anti-aliasing sample count; the next `prepare`
    /// recreates the bindings.
    pub fn set_samples(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        shader: &wgpu::ShaderModule,
        samples: u32,
    ) {
        (self.pipeline, self.flat_pipeline) = Self::pipelines(device, format, shader, samples);
        self.bind = None;
    }
    fn pipelines(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        shader: &wgpu::ShaderModule,
        samples: u32,
    ) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
        let pipeline = Self::pipeline(
            device,
            format,
            shader,
            samples,
            wgpu::DepthBiasState::default(),
            None,
        );
        // Both passes use the same resources. Share the layout explicitly:
        // independently inferred layouts cannot share a bind group.
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ground mark and airborne effect resources"),
            bind_group_layouts: &[&pipeline.get_bind_group_layout(0)],
            push_constant_ranges: &[],
        });
        let flat = Self::pipeline(
            device,
            format,
            shader,
            samples,
            GROUND_DEPTH_BIAS,
            Some(&layout),
        );
        (pipeline, flat)
    }
    fn pipeline(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        shader: &wgpu::ShaderModule,
        samples: u32,
        bias: wgpu::DepthBiasState,
        layout: Option<&wgpu::PipelineLayout>,
    ) -> wgpu::RenderPipeline {
        device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Original explosion, fire and crater sprites"),
            layout,
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("effect_vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: INSTANCE_BYTES as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x2,
                        2 => Float32,
                        3 => Float32x4,
                        4 => Float32,
                        5 => Float32,
                        6 => Float32,
                        7 => Float32x3,
                        8 => Float32x4
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("effect_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Greater,
                stencil: Default::default(),
                // `GROUND_DEPTH_BIAS` for flat marks; standing effects use none.
                bias,
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        uniform: &wgpu::Buffer,
        weather: (&wgpu::Texture, &wgpu::TextureView),
        art: &Art,
        effects: &[EffectPose],
        marks: &[MarkPose],
        climate: crate::blast_climate::Climate,
    ) {
        if self.bind.is_none() {
            let size = wgpu::Extent3d {
                width: SHEET_WIDTH as u32,
                height: SHEET_HEIGHT as u32,
                depth_or_array_layers: SHEETS.len() as u32,
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Runtime indexed explosion, fire and crater sheets"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                texture.as_image_copy(),
                &art.layers,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SHEET_WIDTH as u32),
                    rows_per_image: Some(SHEET_HEIGHT as u32),
                },
                size,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let palette = weather.0.create_view(&Default::default());
            let material_storage = crate::sim_renderer::tile_layout(device, false);
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Explosion sheets and camera"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&palette),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(weather.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: material_storage.as_entire_binding(),
                    },
                ],
            }));
        }
        self.fire_present = art.present[FIRE_SHEET];
        self.sprites = sprites(art, effects, marks, climate);
        self.fitted = None;
    }
    /// Sorts what the camera can see: craters first, then everything else
    /// far to near.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        camera: &Camera,
        world: &crate::terrain::Terrain,
    ) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        let eye = camera.position;
        // The fitted sprites and their glows do not depend on the camera:
        // made once per frame (after `prepare` and `fit_fires`), shared by
        // every view. The city's fires are per view and follow them.
        let glow_on = self.glow_on;
        let (sprites, fires) = (&self.sprites, &self.fires);
        let (base, base_glows, base_lights) = self.fitted.get_or_insert_with(|| {
            let fitted = fit_fires(sprites, fires);
            let (glows, lights) = if glow_on {
                glows_of(&fitted)
            } else {
                Default::default()
            };
            (fitted, glows, lights)
        });
        self.glows.clear();
        self.sustained_lights.clear();
        if self.glow_on {
            let (city_glows, city_lights) = glows_of(&self.city);
            self.glows.extend_from_slice(base_glows);
            self.glows.extend(city_glows);
            self.sustained_lights.extend_from_slice(base_lights);
            self.sustained_lights.extend(city_lights);
            self.glows.extend_from_slice(&self.far_city);
        }
        let visible = visible(base.iter().chain(&self.city), eye);
        self.flat_count = visible.iter().take_while(|s| s.mode == Mode::Flat).count() as u32;
        let mut instances: Vec<f32> =
            Vec::with_capacity(visible.len().min(MAX_INSTANCES) * INSTANCE_BYTES / 4);
        for s in visible {
            instances.extend(instance_values(s, eye, &|x, z| ground_height(world, x, z)));
        }
        self.count = (instances.len() * 4 / INSTANCE_BYTES) as u32;
        crate::sim_renderer::write_floats(queue, &self.buffer, 0, &instances);
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Effects);
        if self.count == 0 {
            return;
        }
        let Some(bind) = &self.bind else {
            return;
        };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        crate::frame_log::count(crate::frame_log::Count::Effects, self.count as usize);
        if self.flat_count > 0 {
            pass.set_pipeline(&self.flat_pipeline);
            pass.draw(0..6, 0..self.flat_count);
            pass.set_pipeline(&self.pipeline);
        }
        pass.draw(0..6, self.flat_count..self.count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(kind: EffectKind, blast: Option<u8>, ticks: u16) -> EffectPose {
        EffectPose {
            kind,
            position: [100., 0., 200.],
            ticks,
            blast,
            scale: tore_sim::combat::blast_scale::FULL,
        }
    }

    #[test]
    fn night_effect_lights_follow_fire_size_fade_and_fireball_lifetime() {
        let art = Art::synthetic();
        let climate = crate::blast_climate::Climate::default();
        let fire = |strength| {
            let drawn = sprites(
                &art,
                &[],
                &[MarkPose {
                    kind: MarkKind::Fire,
                    position: [100., 0., 200.],
                    age: 120,
                    strength,
                    scale: 100,
                }],
                climate,
            );
            glows_of(&fit_fires(&drawn, &[([100., 0., 200.], 90.)])).1
        };
        let full = fire(1.);
        let half = fire(0.5);
        assert_eq!(full.len(), 1);
        assert!(full[0].sustained);
        assert_eq!([full[0].position[0], full[0].position[2]], [100., 200.]);
        assert!((full[0].position[1] - 31.5).abs() < 1e-9);
        assert_eq!(half[0].position, full[0].position);
        assert_eq!(full[0].radius, 45.);
        assert_eq!(half[0].radius, full[0].radius);
        assert!(
            (full[0].strength - 9. * crate::countermeasure_renderer::FLARE_STRENGTH).abs() < 0.1
        );
        assert!((half[0].strength / full[0].strength - 0.5).abs() < 1e-6);
        assert!(fire(0.).is_empty());

        let burst = |ticks| {
            let e = effect(EffectKind::Ground, Some(35), ticks);
            glows_of(&sprites(&art, &[e], &[], climate)).1
        };
        // At one second the initial flash has ended, but the fireball still lights surfaces.
        assert!(crate::impact_fx::light([100., 0., 200.], 35, 100, 120.).is_none());
        let middle = burst(120);
        assert_eq!(middle.len(), 1);
        assert!(!middle[0].sustained);
        assert_eq!([middle[0].position[0], middle[0].position[2]], [100., 200.]);
        assert!(middle[0].strength > 0.);
        assert!(middle[0].radius > 0.);
        assert_eq!(burst(60)[0].radius, middle[0].radius);
        assert!((burst(60)[0].strength / middle[0].strength - 0.25).abs() < 1e-6);
        assert!(burst(0).is_empty());
        let dud = EffectPose {
            scale: 0,
            ..effect(EffectKind::Ground, Some(35), 120)
        };
        assert!(glows_of(&sprites(&art, &[dud], &[], climate)).1.is_empty());
    }

    #[test]
    fn each_explosion_type_draws_the_sheet_exp_sh_names() {
        let name = |kind| SHEETS[explosion_sheet(kind).unwrap()].0;
        assert_eq!(name(15), "GRNDSML.PIC");
        assert_eq!(name(16), "DIRTEXP.PIC");
        assert_eq!(name(17), "WATSML.PIC");
        assert_eq!(name(18), "AIRSML.PIC");
        assert_eq!(name(22), "GRNDMED.PIC");
        assert_eq!(name(27), "FLAKA.PIC");
        assert_eq!(name(30), "AIRLRG.PIC");
        assert_eq!(name(34), "WATLRG.PIC");
        assert_eq!(name(35), "GRNDLRG.PIC");
        assert_eq!(name(37), "GRDLRGA.PIC");
        assert_eq!(name(38), "EMPEX.PIC");
        assert!(explosion_sheet(14).is_none() && explosion_sheet(39).is_none());
        // Every frame of every sheet lies inside the 256-wide texture.
        for (name, layout) in SHEETS {
            let last = layout.cell(layout.frames - 1);
            assert!(last[0] + last[2] <= 256., "{name}");
            assert!(last[1] + last[3] <= SHEET_HEIGHT as f32, "{name}");
        }
        assert_eq!(SHEETS[4].1.cell(4), [81., 58., 78., 56.]);
    }

    #[test]
    fn flak_bursts_draw_the_small_sheet_for_85_mm_and_the_large_one_for_100_mm() {
        let art = Art::synthetic();
        let burst = |kind: u8, ticks: u16| {
            sprites(
                &art,
                &[effect(EffectKind::Flak, Some(kind), ticks)],
                &[],
                crate::blast_climate::Climate::default(),
            )
        };
        // Type 27: FLAKA, 28 frames over two seconds, floating where it bursts.
        let small = burst(27, 120);
        assert_eq!(small.len(), 1, "no shockwave, no second sprite");
        assert_eq!(small[0].layer, 12);
        assert_eq!(SHEETS[small[0].layer].0, "FLAKA.PIC");
        assert_eq!(small[0].mode, Mode::Billboard);
        assert_eq!(small[0].cell, SHEETS[12].1.cell(14));
        assert!(small[0].emissive);
        // Type 28: FLAKB, 12 frames over one second, drawn larger.
        let large = burst(28, 60);
        assert_eq!(SHEETS[large[0].layer].0, "FLAKB.PIC");
        assert_eq!(large[0].cell, SHEETS[13].1.cell(6));
        let width = |kind: u8| f64::from(blast::rolled_size(kind, [100., 0., 200.]));
        assert!(width(28) > width(27));
        assert_eq!(large[0].extent[0], width(28) / 2.);
        // An old recording's flak (no type) draws as the small one.
        let legacy = sprites(
            &art,
            &[effect(EffectKind::Flak, None, 240)],
            &[],
            crate::blast_climate::Climate::default(),
        );
        assert_eq!(SHEETS[legacy[0].layer].0, "FLAKA.PIC");
    }

    #[test]
    fn an_impact_draws_at_its_weapons_scale_with_its_shockwave() {
        let art = Art::synthetic();
        let at = |scale: u8| {
            let pose = EffectPose {
                scale,
                ..effect(EffectKind::Ground, Some(35), 200)
            };
            sprites(&art, &[pose], &[], crate::blast_climate::Climate::default())
        };
        let full = at(100);
        let big = at(159);
        let dud = at(tore_sim::combat::blast_scale::INERT);
        let fireball = |s: &[Sprite]| *s.iter().find(|s| s.layer == 8).unwrap();
        let width = f64::from(blast::rolled_size(35, [100., 0., 200.]));
        assert_eq!(fireball(&full).extent[0], width / 2.);
        assert!((fireball(&big).extent[0] - 1.59 * width / 2.).abs() < 1e-9);
        // The inert marker draws full size.
        assert_eq!(fireball(&dud).extent[0], width / 2.);
        // The shockwave ring scales with it.
        let ring = |s: &[Sprite]| {
            s.iter()
                .filter(|s| s.layer == SMOKE_SHEET)
                .map(|s| s.extent[0])
                .fold(0., f64::max)
        };
        assert!((ring(&big) - 1.59 * ring(&full)).abs() < 1e-9);
        // A 20 mm hit at half size.
        let hit = sprites(
            &art,
            &[EffectPose {
                scale: 50,
                ..effect(EffectKind::Hit, Some(18), 60)
            }],
            &[],
            crate::blast_climate::Climate::default(),
        );
        let small = f64::from(blast::rolled_size(18, [100., 0., 200.]));
        assert_eq!(hit[0].extent[0], small / 4.);
    }

    #[test]
    fn explosions_animate_over_their_life_and_sit_or_float_by_type() {
        let art = Art::synthetic();
        // Type 30: one second, twelve frames, floating in the air.
        let start = sprites(
            &art,
            &[effect(EffectKind::Destroyed, Some(30), 120)],
            &[],
            crate::blast_climate::Climate::default(),
        );
        let end = sprites(
            &art,
            &[effect(EffectKind::Destroyed, Some(30), 1)],
            &[],
            crate::blast_climate::Climate::default(),
        );
        assert_eq!(start[0].cell, SHEETS[4].1.cell(0));
        assert_eq!(end[0].cell, SHEETS[4].1.cell(11));
        assert_eq!(start[0].mode, Mode::Billboard);
        let width = f64::from(blast::rolled_size(30, [100., 0., 200.]));
        assert_eq!(start[0].extent[0], width / 2.);
        // Type 35 stands on the ground for two seconds.
        let ground = sprites(
            &art,
            &[effect(EffectKind::Ground, Some(35), 120)],
            &[],
            crate::blast_climate::Climate::default(),
        );
        let blast = ground.iter().find(|s| s.layer == 8).unwrap();
        assert_eq!(blast.mode, Mode::Standing);
        assert_eq!(blast.cell, SHEETS[8].1.cell(7));
        // Recordings without types draw their family; launches draw nothing.
        let old = sprites(
            &art,
            &[
                effect(EffectKind::Destroyed, None, 240),
                effect(EffectKind::Launch, None, 45),
            ],
            &[],
            crate::blast_climate::Climate::default(),
        );
        assert_eq!(old.len(), 1);
        assert_eq!(old[0].layer, 4);
        let debris = sprites(
            &art,
            &[effect(EffectKind::DebrisImpact, None, 45)],
            &[],
            crate::blast_climate::Climate::default(),
        );
        assert_eq!((debris[0].layer, debris[0].extent[0]), (DEBRIS_SHEET, 7.5));
    }

    #[test]
    fn large_ground_explosions_throw_out_a_fading_shockwave_ring() {
        use crate::blast_climate::Climate;
        let art = Art::synthetic();
        let ring_in = |kind: u8, ticks: u16, climate: Climate| -> Vec<Sprite> {
            sprites(
                &art,
                &[effect(EffectKind::Ground, Some(kind), ticks)],
                &[],
                climate,
            )
            .into_iter()
            .filter(|s| s.layer == SMOKE_SHEET)
            .collect()
        };
        let ring = |kind, ticks| ring_in(kind, ticks, Climate::default());
        let center = [100., 0., 200.];
        let width = f64::from(blast::rolled_size(35, center));
        let reach = |s: &Sprite| {
            let p = s.position;
            ((p[0] - center[0]).powi(2) + (p[2] - center[2]).powi(2)).sqrt()
        };
        let outer = |puffs: &[Sprite]| puffs[..shockwave::OUTER].to_vec();
        let widest = |puffs: &[Sprite]| puffs.iter().map(reach).fold(0., f64::max);
        // Just after the blast: a tight skirt of dust on the ground, two rows.
        let early = ring(35, 236);
        assert_eq!(early.len(), shockwave::OUTER + shockwave::INNER);
        let aspect = {
            let l = SHEETS[SMOKE_SHEET].1;
            f64::from(l.height) / f64::from(l.width)
        };
        assert!(early.iter().all(|s| s.mode == Mode::Standing
            && s.position[1] == 0.
            && s.cell == SHEETS[SMOKE_SHEET].1.cell(1)
            && !s.emissive
            // Squat: wider than tall.
            && s.extent[1] < s.extent[0] * 2. * aspect * 0.6));
        assert!(widest(&early) < 0.3 * width);
        // The leading edge reaches its full radius as the blast wave slows,
        // each puff within its jitter; the inner row trails behind it.
        let grown = ring(35, 240 - shockwave::GROW_TICKS as u16);
        let edge = shockwave::REACH * width;
        for s in outer(&grown) {
            let r = reach(&s) / edge;
            assert!(
                (1. - shockwave::RADIUS_JITTER - 1e-9..=1. + shockwave::RADIUS_JITTER + 1e-9)
                    .contains(&r)
            );
        }
        let inner_most = grown[shockwave::OUTER..]
            .iter()
            .map(reach)
            .fold(f64::MAX, f64::min);
        assert!(inner_most < 0.8 * edge);
        // Then drifting on, the puffs grown, and fading out.
        let late = ring(35, 10);
        assert!(widest(&late) > widest(&grown));
        assert!(late[0].extent[0] > early[0].extent[0]);
        assert!(early[0].opacity > grown[0].opacity && grown[0].opacity > late[0].opacity);
        assert!(late[0].opacity < 0.01);
        // Tan in a desert, pale in a jungle.
        let arid = ring_in(35, 200, Climate { humidity: 0.1 });
        let humid = ring_in(35, 200, Climate { humidity: 0.9 });
        assert!(arid[0].tint[0] > arid[0].tint[2] + 0.3);
        assert!(humid[0].tint[0] - humid[0].tint[2] < 0.1);
        // Every large land type has one.
        for kind in [21, 22, 23, 36, 37] {
            assert_eq!(
                ring(kind, 200).len(),
                shockwave::OUTER + shockwave::INNER,
                "{kind}"
            );
        }
        // On water it is the white spray as before, whatever the climate.
        let spray_at = ring_in(
            34,
            240 - spray::GROW_TICKS as u16,
            Climate { humidity: 0.1 },
        );
        assert_eq!(spray_at.len(), spray::PUFFS);
        let water_width = f64::from(blast::rolled_size(34, center));
        assert!(
            spray_at
                .iter()
                .all(|s| s.cell == SHEETS[SMOKE_SHEET].1.cell(2)
                    && s.tint == PLAIN
                    && (reach(s) - spray::REACH * water_width).abs() < 1e-6)
        );
        // Gun puffs, air bursts, flak and debris landing have none.
        for kind in [15, 16, 17, 18, 27, 30, 38] {
            assert!(ring(kind, 40).is_empty(), "{kind}");
        }
        let debris = sprites(
            &art,
            &[effect(EffectKind::DebrisImpact, Some(35), 40)],
            &[],
            Climate::default(),
        );
        assert!(debris.iter().all(|s| s.layer != SMOKE_SHEET));
        // Without SMOKE.PIC the explosion still draws, alone.
        let mut missing = Art::synthetic();
        missing.present[SMOKE_SHEET] = false;
        assert_eq!(
            sprites(
                &missing,
                &[effect(EffectKind::Ground, Some(35), 200)],
                &[],
                Climate::default()
            )
            .len(),
            1
        );
    }

    #[test]
    fn a_fire_that_fits_its_unit_is_drawn_at_the_units_width() {
        let art = Art::synthetic();
        let fire = |x| MarkPose {
            kind: MarkKind::Fire,
            position: [x, 10., 0.],
            age: 0,
            strength: 1.,
            scale: 100,
        };
        let drawn = sprites(
            &art,
            &[],
            &[fire(0.), fire(500.)],
            crate::blast_climate::Climate::default(),
        );
        // Both are the crash site's 100 feet until one is fitted.
        assert!(drawn.iter().all(|s| s.extent[0] == 50.));
        let (_, layout) = SHEETS[FIRE_SHEET];
        let fitted = fit_fires(&drawn, &[([0., 10., 0.], 30.)]);
        assert_eq!(fitted[0].extent[0], 15.);
        assert_eq!(
            fitted[0].extent[1],
            30. * f64::from(layout.height) / f64::from(layout.width)
        );
        assert_eq!(
            fitted[1].extent, drawn[1].extent,
            "the other keeps its size"
        );
    }

    #[test]
    fn city_fires_are_standing_fire_sprites_of_their_width_that_loop() {
        let (_, layout) = SHEETS[FIRE_SHEET];
        let fire = |phase| crate::city_fires::CityFire {
            position: [10., 5., 20.],
            width: 24.,
            phase,
        };
        let at = |tick, phase| city_sprites(&[fire(phase)], tick)[0];
        let first = at(0, 0);
        assert_eq!(first.mode, Mode::Standing);
        assert_eq!(first.layer, FIRE_SHEET);
        assert_eq!(first.extent[0], 12.);
        assert_eq!(
            first.extent[1],
            24. * f64::from(layout.height) / f64::from(layout.width)
        );
        assert_eq!((first.opacity, first.emissive), (1., true));
        // The flames loop once a second, a neighbour's offset by its phase.
        assert_eq!(at(120, 0).cell, first.cell);
        assert_ne!(at(60, 0).cell, first.cell);
        assert_eq!(at(60, 60).cell, first.cell);
        assert!(city_sprites(&[], 7).is_empty());
    }

    #[test]
    fn craters_lie_flat_first_and_fires_loop_and_fade() {
        let art = Art::synthetic();
        let mark = |kind, age, strength| MarkPose {
            kind,
            position: [0., 10., 0.],
            age,
            strength,
            scale: 100,
        };
        let drawn = sprites(
            &art,
            &[effect(EffectKind::Destroyed, Some(30), 60)],
            &[
                mark(MarkKind::Crater(18), 0, 1.),
                mark(MarkKind::Fire, 60, 0.25),
            ],
            crate::blast_climate::Climate::default(),
        );
        assert_eq!(drawn[0].mode, Mode::Flat);
        assert_eq!(drawn[0].extent, [288., 288.]);
        assert_eq!(drawn[0].cell[1..], [0., 78., 66.]);
        assert_eq!(drawn[1].mode, Mode::Standing);
        assert_eq!(drawn[1].cell, SHEETS[FIRE_SHEET].1.cell(7));
        assert_eq!(drawn[1].opacity, 0.25);
        assert!(!drawn[0].emissive && drawn[1].emissive && drawn[2].emissive);
        let mut missing = Art::synthetic();
        missing.present[CRATER_SHEET] = false;
        assert_eq!(
            sprites(
                &missing,
                &[],
                &[mark(MarkKind::Crater(3), 0, 1.)],
                crate::blast_climate::Climate::default()
            )
            .len(),
            0
        );
    }

    #[test]
    fn a_cluster_burst_adds_small_ground_flashes_that_play_out_with_it() {
        let art = Art::synthetic();
        let burst = |kind, ticks| effect(kind, Some(18), ticks);
        let drawn = |kind, ticks| {
            sprites(
                &art,
                &[burst(kind, ticks)],
                &[],
                crate::blast_climate::Climate::default(),
            )
        };
        // The burst itself is the one AIRSML flash the original draws; the
        // flashes come after it.
        let all = drawn(EffectKind::Ground, 120);
        assert_eq!(SHEETS[all[0].layer].0, "AIRSML.PIC");
        let flashes = |kind, ticks| drawn(kind, ticks).split_off(1);
        // At its first tick only the flashes that start at once are playing.
        let first = flashes(EffectKind::Ground, 120);
        let starting = cluster::derive(&burst(EffectKind::Ground, 120))
            .iter()
            .filter(|f| f.start == 0)
            .count();
        assert_eq!(first.len(), starting);
        assert!(first.iter().all(|s| s.mode == Mode::Standing && s.emissive));
        // Midway most are playing, spread around the burst.
        let middle = flashes(EffectKind::Ground, 60);
        assert!(middle.len() > 10 && middle.len() <= cluster::BOMBLETS);
        assert!(middle.iter().any(|s| s.position[0] > 100.));
        assert!(middle.iter().any(|s| s.position[0] < 100.));
        // Each plays its twelve frames over half a second; none outlives the
        // burst's last tick.
        assert!(flashes(EffectKind::Ground, 1).is_empty());
        let frames: std::collections::BTreeSet<u32> =
            middle.iter().map(|s| s.cell[0].to_bits()).collect();
        assert!(frames.len() > 1, "flashes are at different frames");
        // Anything else of the same type draws none: a gun hit on an
        // aircraft, or a bomb's ground burst.
        assert_eq!(drawn(EffectKind::Hit, 60).len(), 1);
        assert_eq!(
            sprites(
                &art,
                &[effect(EffectKind::Ground, Some(21), 60)],
                &[],
                crate::blast_climate::Climate::default()
            )
            .iter()
            .filter(|s| s.layer == 0)
            .count(),
            0
        );
        // A missing flash sheet skips its flashes only.
        let mut missing = Art::synthetic();
        missing.present[19] = false;
        let some = sprites(
            &missing,
            &[burst(EffectKind::Ground, 60)],
            &[],
            crate::blast_climate::Climate::default(),
        );
        assert!(some.iter().all(|s| s.layer != 19));
        assert!(some.len() > 1);
    }

    /// The CPU cost of a long mission's marks each frame (slice BF,
    /// 2026-10-12): 20,000 gun pock marks and 1,000 craters spread over a
    /// 20 nmi square, the camera over its middle. Run with `--ignored
    /// --nocapture` in a release build for frame numbers.
    #[test]
    #[ignore]
    fn a_long_missions_marks_cost_a_frame() {
        let art = Art::synthetic();
        let marks: Vec<MarkPose> = (0..21_000_u64)
            .map(|n| MarkPose {
                kind: MarkKind::Crater(if n < 20_000 { blast::POCK } else { 9 }),
                position: [
                    (n * 37_301 % 120_000) as f64,
                    0.,
                    (n * 91_127 % 120_000) as f64,
                ],
                age: n,
                strength: 1.,
                scale: if n < 20_000 { 50 } else { 159 },
            })
            .collect();
        let eye = [60_000., 3_000., 60_000.];
        let frames = 60;
        let start = std::time::Instant::now();
        let mut drawn = 0;
        for _ in 0..frames {
            let all = sprites(&art, &[], &marks, crate::blast_climate::Climate::default());
            let fitted = fit_fires(&all, &[]);
            drawn = visible(&fitted, eye).len();
        }
        let per_frame = start.elapsed().as_secs_f64() * 1000. / f64::from(frames);
        println!(
            "mark stress: {per_frame:.2} ms a frame for 21000 marks, {drawn} drawn (cap {MAX_MARK_INSTANCES}); sprite memory {} bytes",
            marks.len() * std::mem::size_of::<Sprite>()
        );
        assert!(drawn <= MAX_MARK_INSTANCES);
    }
}

#[cfg(test)]
mod depth_tests;
