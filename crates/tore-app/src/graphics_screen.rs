//! The Graphics options screen, opened from main menu Pref > Graphics... Every
//! change takes effect on its own: the screen edits a draft of
//! `graphics::Options`, and the host hands the draft to the renderer and saves
//! it once the player has stopped changing things for [`SETTLE`]. Defaults puts
//! the recommended values back (applied the same way), and Close or Escape
//! leaves. The rows sit under short group headings in a list that scrolls with
//! the map's arrow bar, as the lobby's settings pages do.
//!
//! Layout, wording and behaviour are an opinionated agent design (2026-09-22)
//! in the style of the input configuration screen, drawn with the imported
//! raster font through that screen's shared helpers. Applying as you choose,
//! Close instead of Cancel, the scroll bar, the groups and the resolutions on
//! the render scale row were requested by John on 2026-10-19; the settle
//! delay, the group names and the resolution line are agent decisions.
use crate::controls_editor::{
    self as chrome, BUTTON, Editor as Chrome, FOCUS, GOOD, HEADER, INK, MUTED, PALE, PANEL, Rect,
    ResultAction, TITLE, WHITE, fit, inside, text_width,
};
use crate::graphics::{
    AntiAliasing, BUILDING_DETAILS, BUILDING_DISTANCES, FrameLimit, GRASS_DISTANCES,
    GROUND_TEXTURE_DISTANCES, GraphicsApi, Grass, GroundTexture, Options, RENDER_SCALES,
    ShadowDistance, ShadowQuality, SpottingAid, VEGETATION_DETAILS, VEGETATION_DISTANCES, VegCards,
    VegDensity, scaled_size,
};
use crate::menu::Canvas;
use crate::widgets::{ArrowBar, ArrowRows, RowPress};
use std::path::Path;
use std::time::{Duration, Instant};
use tore_formats::font::Font;

/// How long after the last change the settings are applied and saved. Holding
/// an arrow key or clicking through choices quickly applies only the value the
/// player stops on, so an anti-aliasing or vegetation change does not rebuild
/// the renderer's pipelines or buffers once per step (agent decision,
/// 2026-10-19).
pub const SETTLE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    AntiAliasing,
    RenderScale,
    SpottingAid,
    FrameLimit,
    GraphicsApi,
    TerrainFiltering,
    FrameRate,
    BuildingDistance,
    BuildingDetail,
    Shadows,
    ShadowDistance,
    GroundTexture,
    GroundTextureDistance,
    VegetationDistance,
    VegetationDetail,
    VegetationDensity,
    VegetationCards,
    Grass,
    GrassDensity,
    GrassDistance,
}

/// Every row in display order, grouped.
const ALL_ROWS: [Row; 20] = [
    Row::AntiAliasing,
    Row::RenderScale,
    Row::SpottingAid,
    Row::FrameLimit,
    Row::GraphicsApi,
    Row::TerrainFiltering,
    Row::GroundTexture,
    Row::GroundTextureDistance,
    Row::BuildingDistance,
    Row::BuildingDetail,
    Row::Shadows,
    Row::ShadowDistance,
    Row::VegetationDistance,
    Row::VegetationDetail,
    Row::VegetationDensity,
    Row::VegetationCards,
    Row::Grass,
    Row::GrassDensity,
    Row::GrassDistance,
    Row::FrameRate,
];

/// The rows shown. The city building rows hide when `TORE_CITY_FILL=0`, and
/// the ground detail, vegetation and grass rows when `TORE_VEGETATION=0`. Both
/// are developer escape hatches; both are on when the variable is unset. The
/// graphics API row is Windows only, the one platform with a choice to make.
fn rows() -> Vec<Row> {
    rows_on(cfg!(target_os = "windows"))
}
fn rows_on(windows: bool) -> Vec<Row> {
    let city = crate::scenery::city_fill().unwrap_or(true);
    let veg = crate::vegetation::enabled().unwrap_or(true);
    ALL_ROWS
        .into_iter()
        .filter(|row| match row {
            Row::BuildingDistance | Row::BuildingDetail => city,
            Row::GroundTexture
            | Row::GroundTextureDistance
            | Row::VegetationDistance
            | Row::VegetationDetail
            | Row::VegetationDensity
            | Row::VegetationCards
            | Row::Grass
            | Row::GrassDensity
            | Row::GrassDistance => veg,
            Row::GraphicsApi => windows,
            _ => true,
        })
        .collect()
}

/// One line of the scrolling list: a group heading, a setting row (by its
/// index in [`rows`]), or the render scale's resolution line under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Line {
    Heading(&'static str),
    Setting(usize),
    Resolution(usize),
}

/// The list's lines: a heading before each group's first row, and the
/// resolution line after the render scale row.
fn lines() -> Vec<Line> {
    let mut out = Vec::new();
    let mut group = None;
    for (i, row) in rows().into_iter().enumerate() {
        if group != Some(row.group()) {
            group = Some(row.group());
            out.push(Line::Heading(row.group()));
        }
        out.push(Line::Setting(i));
        if row == Row::RenderScale {
            out.push(Line::Resolution(i));
        }
    }
    out
}

impl Row {
    /// The heading the row sits under.
    fn group(self) -> &'static str {
        match self {
            Row::AntiAliasing
            | Row::RenderScale
            | Row::SpottingAid
            | Row::FrameLimit
            | Row::GraphicsApi => "DISPLAY",
            Row::TerrainFiltering
            | Row::GroundTexture
            | Row::GroundTextureDistance
            | Row::BuildingDistance
            | Row::BuildingDetail
            | Row::Shadows
            | Row::ShadowDistance => "TERRAIN AND SCENERY",
            Row::VegetationDistance
            | Row::VegetationDetail
            | Row::VegetationDensity
            | Row::VegetationCards
            | Row::Grass
            | Row::GrassDensity
            | Row::GrassDistance => "VEGETATION AND GRASS",
            Row::FrameRate => "PERFORMANCE",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Row::AntiAliasing => "Anti-aliasing",
            Row::RenderScale => "Render scale",
            Row::SpottingAid => "Spotting aid",
            Row::TerrainFiltering => "Terrain filtering",
            Row::FrameLimit => "Frame rate",
            Row::GraphicsApi => "Graphics API",
            Row::FrameRate => "Frame rate counter",
            Row::BuildingDistance => "Building distance",
            Row::BuildingDetail => "Building detail",
            Row::Shadows => "Shadows",
            Row::ShadowDistance => "Shadow distance",
            Row::GroundTexture => "Ground detail",
            Row::GroundTextureDistance => "Ground detail distance",
            Row::VegetationDistance => "Vegetation distance",
            Row::VegetationDetail => "Vegetation detail",
            Row::VegetationDensity => "Vegetation density",
            Row::VegetationCards => "Vegetation cards",
            Row::Grass => "Grass",
            Row::GrassDensity => "Grass density",
            Row::GrassDistance => "Grass distance",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Row::AntiAliasing => {
                "Smooths the jagged, stair-stepped edges of aircraft, terrain and the horizon."
            }
            Row::RenderScale => {
                "Draws the 3D view with more or fewer pixels than the window: higher is sharper, lower is faster."
            }
            Row::SpottingAid => {
                "Outlines distant aircraft so they are easier to see against the sky and ground."
            }
            Row::TerrainFiltering => "Reduces shimmer and crawling patterns on distant ground.",
            Row::FrameLimit => {
                "V-sync draws one frame per display refresh. 60, 100 and 200 cap the rate without waiting for the display: less delay, more power."
            }
            Row::GraphicsApi => {
                "Try the other one if the game crashes or will not start. Restart the game to apply a change."
            }
            Row::FrameRate => {
                "Shows frame rate, CPU/GPU milliseconds and the recent low. Keeps rendering while menus or flight are paused."
            }
            Row::BuildingDistance => "How far away city buildings are drawn.",
            Row::BuildingDetail => {
                "City buildings inside this distance are drawn with full texture and detail, farther ones as plain blocks."
            }
            Row::Shadows => {
                "Sun shadow detail. Low: terrain, aircraft and ground objects. Med adds city buildings, High trees, Ultra sharper maps."
            }
            Row::ShadowDistance => {
                "How far sun shadows reach. Trees follows Vegetation distance; fixed values stay put. Farther costs more. Ignored with Shadows Off."
            }
            Row::GroundTexture => {
                "Close-range field, grass, sand and rock detail on the ground. Fine is a tighter grain."
            }
            Row::GroundTextureDistance => {
                "How far that ground detail reaches. It also fades out as you climb, so high altitude keeps the retail look."
            }
            Row::VegetationDistance => "How far away trees, bushes and ground clutter are drawn.",
            Row::VegetationDetail => {
                "Vegetation stays denser inside this distance and thins gradually beyond it."
            }
            Row::GrassDensity => {
                "How thick the grass is, independent of trees. Off removes grass geometry."
            }
            Row::GrassDistance => {
                "How far grass reaches before blending into ground detail. Default 500 ft."
            }
            Row::Grass => {
                "Opaque wind-blown blades, the earlier sprite tufts, or no grass geometry."
            }
            Row::VegetationCards => {
                "Camera-facing cards, fixed crossed cards, or either with a canopy top view."
            }
            Row::VegetationDensity => {
                "How thick the vegetation is. High is the tuned look. Ultra is denser."
            }
        }
    }
    fn choices(self) -> Vec<String> {
        match self {
            Row::AntiAliasing => AntiAliasing::ALL.map(|a| a.label().to_owned()).to_vec(),
            Row::RenderScale => RENDER_SCALES.map(|s| format!("{s}%")).to_vec(),
            Row::SpottingAid => SpottingAid::ALL.map(|a| a.label().to_owned()).to_vec(),
            Row::TerrainFiltering => vec!["Off".into(), "On".into()],
            Row::FrameLimit => FrameLimit::ALL.map(|a| a.label().to_owned()).to_vec(),
            Row::GraphicsApi => GraphicsApi::CHOICES.map(|a| a.label().to_owned()).to_vec(),
            Row::FrameRate => vec!["Off".into(), "On".into()],
            Row::BuildingDistance => BUILDING_DISTANCES.map(|n| format!("{n} nm")).to_vec(),
            Row::BuildingDetail => BUILDING_DETAILS.map(|n| format!("{n} nm")).to_vec(),
            Row::Shadows => ShadowQuality::ALL.map(|q| q.label().to_owned()).to_vec(),
            Row::ShadowDistance => ShadowDistance::ALL.map(ShadowDistance::label).to_vec(),
            Row::GroundTexture => GroundTexture::ALL.map(|a| a.label().to_owned()).to_vec(),
            Row::GroundTextureDistance => {
                GROUND_TEXTURE_DISTANCES.map(|n| format!("{n} nm")).to_vec()
            }
            Row::VegetationDistance => VEGETATION_DISTANCES.map(|n| format!("{n} nm")).to_vec(),
            Row::VegetationDetail => VEGETATION_DETAILS.map(|n| format!("{n} nm")).to_vec(),
            Row::VegetationDensity => VegDensity::ALL.map(|a| a.label().to_owned()).to_vec(),
            Row::Grass => Grass::ALL.map(|a| a.label().to_owned()).to_vec(),
            Row::GrassDensity => std::iter::once("Off".to_owned())
                .chain(VegDensity::ALL.map(|a| a.label().to_owned()))
                .collect(),
            Row::GrassDistance => GRASS_DISTANCES.map(|n| format!("{n} ft")).to_vec(),
            Row::VegetationCards => VegCards::ALL.map(|a| a.label().to_owned()).to_vec(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Row(usize),
    Footer(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Row(usize),
    /// Row and choice index.
    Choice(usize, usize),
    Footer(usize),
}

const FOOTER: [&str; 2] = ["Defaults", "Close"];
const DEFAULTS: usize = 0;
const CHOICE_X: i32 = 248;
const CHOICE_W: i32 = 62;
const CHOICE_GAP: i32 = 4;
const OUTLINE: [u8; 4] = [110, 130, 156, 255];

/// The settings panel, and the About panel under it.
const SETTINGS: Rect = (6, 24, 628, 312);
const ABOUT: Rect = (6, 340, 628, 94);
/// The list window: where its first line starts, the pitch between lines,
/// and how many lines show at once.
const VIEW_TOP: i32 = 44;
const PITCH: i32 = 19;
const VISIBLE: usize = 15;
/// The arrow bar inside the settings panel's right edge.
const BAR_X: i32 = SETTINGS.0 + SETTINGS.2 - 4 - crate::widgets::arrow_bar::THICK;

pub struct Editor {
    /// The values shown and edited. They are applied once the player stops
    /// changing them for [`SETTLE`].
    pub draft: Options,
    /// The values last handed to the renderer.
    applied: Options,
    /// When the draft last changed, for the settle delay.
    edited: Option<Instant>,
    /// Whether the settings in effect are also saved to disk.
    saved: bool,
    /// Whether the adapter can draw each `AntiAliasing::ALL` level exactly.
    supported: [bool; 4],
    pub message: String,
    /// Shown at the top right: where the screen was opened from.
    context: &'static str,
    /// The window's size in physical pixels: the size the 3D view is shown
    /// at, which the render scale multiplies. None before a window exists.
    window: Option<[u32; 2]>,
    focus: Focus,
    /// The row the About panel describes; the last row that had focus.
    row: usize,
    pressed: Option<Hit>,
    /// The first line of the list shown.
    first: usize,
    bar: ArrowBar,
}

impl Editor {
    pub fn new(applied: Options, supported: [bool; 4], context: &'static str) -> Self {
        Self {
            draft: applied,
            applied,
            edited: None,
            saved: true,
            supported,
            message: "Choose with the arrows or the mouse. Changes are used and saved at once."
                .into(),
            context,
            window: None,
            focus: Focus::Row(0),
            row: 0,
            pressed: None,
            first: 0,
            bar: ArrowBar::vertical((BAR_X, VIEW_TOP), VISIBLE as i32 * PITCH - 2),
        }
    }
    /// True while the draft has changes not yet applied.
    pub fn dirty(&self) -> bool {
        self.draft != self.applied
    }
    /// When the pending changes should be applied: [`SETTLE`] after the last
    /// one. None when nothing is pending.
    pub fn apply_at(&self) -> Option<Instant> {
        self.dirty()
            .then(|| self.edited.map_or_else(Instant::now, |at| at + SETTLE))
    }
    /// True when pending changes have settled and should be applied now.
    pub fn apply_due(&self, now: Instant) -> bool {
        self.apply_at().is_some_and(|at| now >= at)
    }
    /// The window's size in physical pixels, for the resolution line.
    pub fn set_window(&mut self, size: Option<[u32; 2]>) {
        self.window = size.filter(|s| s[0] > 0 && s[1] > 0);
    }
    pub fn cancel_press(&mut self) {
        self.pressed = None;
        self.bar.release();
    }

    // ---- model ----------------------------------------------------------

    fn choice(&self, row: usize) -> usize {
        let d = &self.draft;
        match rows()[row] {
            Row::AntiAliasing => AntiAliasing::ALL
                .iter()
                .position(|a| *a == d.anti_aliasing)
                .unwrap_or(0),
            Row::RenderScale => RENDER_SCALES
                .iter()
                .position(|s| *s == d.render_scale)
                .unwrap_or(1),
            Row::SpottingAid => SpottingAid::ALL
                .iter()
                .position(|a| *a == d.spotting_aid)
                .unwrap_or(0),
            Row::TerrainFiltering => usize::from(d.terrain_filtering),
            Row::FrameLimit => FrameLimit::ALL
                .iter()
                .position(|a| *a == d.frame_limit)
                .unwrap_or(FrameLimit::ALL.len() - 1),
            Row::GraphicsApi => GraphicsApi::CHOICES
                .iter()
                .position(|a| *a == crate::graphics_api::shown(d.graphics_api))
                .unwrap_or(0),
            Row::FrameRate => usize::from(d.show_frame_rate),
            Row::BuildingDistance => BUILDING_DISTANCES
                .iter()
                .position(|n| *n == d.building_distance_nm)
                .unwrap_or(BUILDING_DISTANCES.len() - 1),
            Row::ShadowDistance => ShadowDistance::ALL
                .iter()
                .position(|d| *d == self.draft.shadow_distance)
                .unwrap_or(0),
            Row::Shadows => ShadowQuality::ALL
                .iter()
                .position(|q| *q == d.shadow_quality)
                .unwrap_or(2),
            Row::BuildingDetail => BUILDING_DETAILS
                .iter()
                .position(|n| *n == d.building_detail_nm)
                .unwrap_or(2),
            Row::GroundTexture => GroundTexture::ALL
                .iter()
                .position(|a| *a == d.ground_texture)
                .unwrap_or(1),
            Row::GroundTextureDistance => GROUND_TEXTURE_DISTANCES
                .iter()
                .position(|n| *n == d.ground_texture_distance_nm)
                .unwrap_or(1),
            Row::VegetationDistance => VEGETATION_DISTANCES
                .iter()
                .position(|n| *n == d.vegetation_distance_nm)
                .unwrap_or(1),
            Row::VegetationDetail => VEGETATION_DETAILS
                .iter()
                .position(|n| *n == d.vegetation_detail_nm)
                .unwrap_or(1),
            Row::VegetationCards => d.vegetation_cards as usize,
            Row::Grass => d.grass as usize,
            Row::GrassDensity => d.grass_density.map_or(0, |v| v as usize + 1),
            Row::GrassDistance => GRASS_DISTANCES
                .iter()
                .position(|&n| n == d.grass_distance_ft)
                .unwrap_or(1),
            Row::VegetationDensity => VegDensity::ALL
                .iter()
                .position(|a| *a == d.vegetation_density)
                .unwrap_or(2),
        }
    }
    fn set_choice(&mut self, row: usize, i: usize) {
        let d = &mut self.draft;
        match rows()[row] {
            Row::AntiAliasing => d.anti_aliasing = AntiAliasing::ALL[i],
            Row::RenderScale => d.render_scale = RENDER_SCALES[i],
            Row::SpottingAid => d.spotting_aid = SpottingAid::ALL[i],
            Row::TerrainFiltering => d.terrain_filtering = i == 1,
            Row::FrameLimit => d.frame_limit = FrameLimit::ALL[i],
            Row::GraphicsApi => d.graphics_api = GraphicsApi::CHOICES[i],
            Row::FrameRate => d.show_frame_rate = i == 1,
            Row::BuildingDistance => d.building_distance_nm = BUILDING_DISTANCES[i],
            Row::BuildingDetail => d.building_detail_nm = BUILDING_DETAILS[i],
            Row::Shadows => d.shadow_quality = ShadowQuality::ALL[i],
            Row::ShadowDistance => d.shadow_distance = ShadowDistance::ALL[i],
            Row::GroundTexture => d.ground_texture = GroundTexture::ALL[i],
            Row::GroundTextureDistance => {
                d.ground_texture_distance_nm = GROUND_TEXTURE_DISTANCES[i]
            }
            Row::VegetationDistance => d.vegetation_distance_nm = VEGETATION_DISTANCES[i],
            Row::VegetationDetail => d.vegetation_detail_nm = VEGETATION_DETAILS[i],
            Row::VegetationDensity => d.vegetation_density = VegDensity::ALL[i],
            Row::VegetationCards => d.vegetation_cards = VegCards::ALL[i],
            Row::Grass => d.grass = Grass::ALL[i],
            Row::GrassDensity => d.grass_density = i.checked_sub(1).map(|n| VegDensity::ALL[n]),
            Row::GrassDistance => d.grass_distance_ft = GRASS_DISTANCES[i],
        }
    }
    fn available(&self, row: usize, i: usize) -> bool {
        rows()[row] != Row::AntiAliasing || self.supported[i]
    }
    fn unsupported(&self) -> Vec<&'static str> {
        AntiAliasing::ALL
            .iter()
            .zip(self.supported)
            .filter(|(_, s)| !s)
            .map(|(a, _)| a.label())
            .collect()
    }
    /// The window size as text, `3840x2160`.
    fn size_text([w, h]: [u32; 2]) -> String {
        format!("{w}x{h}")
    }
    /// The 3D view's size at render scale `percent` for this window.
    fn scaled(&self, percent: u32) -> Option<[u32; 2]> {
        self.window.map(|w| scaled_size(w, percent))
    }
    /// The resolution line's label: the window the 3D view is shown in.
    fn resolution_label(&self) -> String {
        match self.window {
            Some(size) => format!("Window {}", Self::size_text(size)),
            None => "Window size not known yet".into(),
        }
    }
    /// The render scale's summary, as the About panel shows it:
    /// `150%: 5760x3240 drawn for a 3840x2160 window`.
    fn resolution_summary(&self) -> Option<String> {
        let window = self.window?;
        let percent = self.draft.render_scale;
        let drawn = self.scaled(percent)?;
        let how = match percent.cmp(&100) {
            std::cmp::Ordering::Less => "upscaled to",
            std::cmp::Ordering::Equal => {
                return Some(format!(
                    "{percent}%: the 3D view is drawn at the window's own {}.",
                    Self::size_text(window)
                ));
            }
            std::cmp::Ordering::Greater => "downsampled to",
        };
        Some(format!(
            "{percent}%: the 3D view is drawn at {} and {how} the {} window.",
            Self::size_text(drawn),
            Self::size_text(window)
        ))
    }
    /// A one-line note under the focused row's description, if it has one.
    fn note(&self, row: usize) -> Option<String> {
        match rows()[row] {
            Row::RenderScale => return self.resolution_summary(),
            Row::FrameLimit => {
                return crate::frame_pacing::Limit::overridden()
                    .then(|| "TORE_FRAME_LIMIT replaces this choice for this run.".into());
            }
            Row::GraphicsApi => return crate::graphics_api::status_note(self.draft.graphics_api),
            Row::AntiAliasing => {}
            _ => return None,
        }
        let chosen = self.draft.anti_aliasing;
        let list = self.unsupported();
        if !self.supported[self.choice(row)] {
            Some(format!(
                "{} is not available on this graphics card; the next lower level is used.",
                chosen.label()
            ))
        } else if list.is_empty() {
            None
        } else {
            Some(format!(
                "{} not available on this graphics card.",
                list.join(" and ")
            ))
        }
    }

    // ---- editing --------------------------------------------------------

    /// The draft changed: start the settle delay again.
    fn edited(&mut self) {
        self.edited = Some(Instant::now());
    }
    fn pick(&mut self, row: usize, i: usize) -> ResultAction {
        self.set_focus(Focus::Row(row));
        let label = &rows()[row].choices()[i];
        if !self.available(row, i) {
            self.message = format!("{label} is not available on this graphics card");
            return ResultAction::Changed;
        }
        if self.choice(row) == i {
            return ResultAction::None;
        }
        self.set_choice(row, i);
        self.edited();
        self.message = if rows()[row] == Row::GraphicsApi {
            format!("Graphics API set to {label}. Restart the game to apply.")
        } else {
            format!("{} set to {label}.", rows()[row].label())
        };
        ResultAction::Changed
    }
    /// Left/right: the next available choice in that direction, stopping at
    /// the ends.
    fn step(&mut self, row: usize, delta: i32) -> ResultAction {
        let count = rows()[row].choices().len() as i32;
        let mut i = self.choice(row) as i32 + delta;
        while (0..count).contains(&i) {
            if self.available(row, i as usize) {
                return self.pick(row, i as usize);
            }
            i += delta;
        }
        ResultAction::None
    }
    /// Enter, a click or the gamepad's A: the next available choice, wrapping.
    fn cycle(&mut self, row: usize) -> ResultAction {
        let count = rows()[row].choices().len();
        let start = self.choice(row);
        (1..count)
            .map(|k| (start + k) % count)
            .find(|i| self.available(row, *i))
            .map_or(ResultAction::None, |i| self.pick(row, i))
    }
    fn defaults(&mut self) -> ResultAction {
        // The picture settings, not a choice that may be keeping the game running.
        self.draft = Options {
            graphics_api: self.draft.graphics_api,
            ..Options::default()
        };
        self.edited();
        self.message = "Recommended settings restored.".into();
        ResultAction::Changed
    }
    /// Makes the draft the settings in effect and saves it to `path` when
    /// there is one. Returns the options the host hands to the renderer.
    pub fn apply(&mut self, path: Option<&Path>) -> Options {
        self.applied = self.draft;
        self.edited = None;
        match path.map(|p| self.draft.save(p)) {
            None => self.saved = false,
            Some(Ok(())) => self.saved = true,
            Some(Err(e)) => {
                self.saved = false;
                self.message = format!("Graphics settings in use but not saved: {e}");
            }
        }
        self.draft
    }

    // ---- scrolling ------------------------------------------------------

    fn bar_rows(&self) -> ArrowRows {
        ArrowRows {
            total: lines().len(),
            visible: VISIBLE,
            first: self.first,
        }
    }
    fn scroll_to(&mut self, first: usize) {
        self.first = first.min(self.bar_rows().max_first());
    }
    fn scroll_by(&mut self, lines: i32) {
        self.scroll_to(self.bar_rows().shifted(i64::from(lines)));
    }
    /// Scrolls so `row` shows, with its group heading when it is the group's
    /// first row and with the resolution line under the render scale.
    fn reveal(&mut self, row: usize) {
        let lines = lines();
        let Some(at) = lines.iter().position(|l| *l == Line::Setting(row)) else {
            return;
        };
        let top = if at > 0 && matches!(lines[at - 1], Line::Heading(_)) {
            at - 1
        } else {
            at
        };
        let bottom = if lines.get(at + 1) == Some(&Line::Resolution(row)) {
            at + 1
        } else {
            at
        };
        if top < self.first {
            self.first = top;
        } else if bottom >= self.first + VISIBLE {
            self.first = bottom + 1 - VISIBLE;
        }
        self.scroll_to(self.first);
    }

    // ---- navigation -----------------------------------------------------

    fn order() -> Vec<Focus> {
        (0..rows().len())
            .map(Focus::Row)
            .chain((0..FOOTER.len()).map(Focus::Footer))
            .collect()
    }
    fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        if let Focus::Row(row) = focus {
            self.row = row;
            self.reveal(row);
        }
    }
    fn move_focus(&mut self, key: &str) -> ResultAction {
        let last = rows().len() - 1;
        let focus = match (self.focus, key) {
            (Focus::Row(r), "ArrowUp") => Focus::Row(r.saturating_sub(1)),
            (Focus::Row(r), "ArrowDown") if r < last => Focus::Row(r + 1),
            (Focus::Row(_), "ArrowDown") => Focus::Footer(0),
            (Focus::Footer(_), "ArrowUp") => Focus::Row(last),
            (Focus::Footer(i), "ArrowLeft") => Focus::Footer(i.saturating_sub(1)),
            (Focus::Footer(i), "ArrowRight") => Focus::Footer((i + 1).min(FOOTER.len() - 1)),
            (focus, _) => focus,
        };
        self.set_focus(focus);
        // Moving focus redraws without the click sound activation makes.
        ResultAction::None
    }
    fn activate(&mut self, hit: Hit) -> ResultAction {
        match hit {
            Hit::Row(row) => {
                self.set_focus(Focus::Row(row));
                ResultAction::None
            }
            Hit::Choice(row, i) => self.pick(row, i),
            Hit::Footer(i) => {
                self.focus = Focus::Footer(i);
                if i == DEFAULTS {
                    self.defaults()
                } else {
                    ResultAction::Close
                }
            }
        }
    }
    /// Keyboard input, and controller menu actions mapped to arrow keys,
    /// Enter and Escape.
    pub fn key(&mut self, key: &str, shift: bool) -> ResultAction {
        match (key, self.focus) {
            ("Escape", _) => ResultAction::Close,
            ("ArrowLeft", Focus::Row(r)) => self.step(r, -1),
            ("ArrowRight", Focus::Row(r)) => self.step(r, 1),
            ("ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight", _) => self.move_focus(key),
            ("Enter" | "Space", Focus::Row(r)) => self.cycle(r),
            ("Enter" | "Space", Focus::Footer(i)) => self.activate(Hit::Footer(i)),
            ("Tab", focus) => {
                let order = Self::order();
                let at = order.iter().position(|f| *f == focus).unwrap_or(0);
                let n = order.len();
                let next = if shift { at + n - 1 } else { at + 1 };
                self.set_focus(order[next % n]);
                ResultAction::None
            }
            _ => ResultAction::None,
        }
    }
    /// A wheel notch scrolls the list a line, up for a positive count, as
    /// the lobby's settings pages do.
    pub fn wheel(&mut self, notches: i32) -> ResultAction {
        self.scroll_by(-notches);
        ResultAction::None
    }

    // ---- pointer --------------------------------------------------------

    /// Where list line `line` is drawn now, or none when it is scrolled away.
    fn line_rect(&self, line: usize) -> Option<Rect> {
        if line < self.first || line >= self.first + VISIBLE {
            return None;
        }
        let y = VIEW_TOP + (line - self.first) as i32 * PITCH;
        Some((SETTINGS.0 + 4, y, BAR_X - 4 - (SETTINGS.0 + 4), PITCH - 2))
    }
    fn row_rect(&self, row: usize) -> Option<Rect> {
        let line = lines().iter().position(|l| *l == Line::Setting(row))?;
        self.line_rect(line)
    }
    fn choice_x(i: usize) -> i32 {
        CHOICE_X + i as i32 * (CHOICE_W + CHOICE_GAP)
    }
    fn choice_rect(&self, row: usize, i: usize) -> Option<Rect> {
        let r = self.row_rect(row)?;
        // Six distance choices must fit before the scroll bar too.
        let count = rows()[row].choices().len() as i32;
        let width = if rows()[row] == Row::GraphicsApi {
            // "DirectX 12" is wider than the numbers and short words elsewhere.
            CHOICE_W * 3 / 2
        } else {
            CHOICE_W.min((BAR_X - 4 - CHOICE_X - (count - 1) * CHOICE_GAP) / count)
        };
        Some((
            CHOICE_X + i as i32 * (width + CHOICE_GAP),
            r.1 + 1,
            width,
            (r.3 - 2).max(8),
        ))
    }
    /// The footer buttons, right-aligned where the input configuration
    /// screen's last two sit.
    fn footer_rect(i: usize) -> Rect {
        (420 + i as i32 * 108, 456, 100, 18)
    }
    fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        for (row, kind) in rows().iter().enumerate() {
            let Some(r) = self.row_rect(row) else {
                continue;
            };
            if !inside(p, r) {
                continue;
            }
            return Some(
                (0..kind.choices().len())
                    .find(|i| self.choice_rect(row, *i).is_some_and(|c| inside(p, c)))
                    .map_or(Hit::Row(row), |i| Hit::Choice(row, i)),
            );
        }
        (0..FOOTER.len())
            .find(|i| inside(p, Self::footer_rect(*i)))
            .map(Hit::Footer)
    }
    /// Left button: a click needs press and release on the same control. A
    /// press on the scroll bar scrolls, or holds its thumb for [`Self::moved`].
    pub fn pointer(&mut self, point: Option<(f64, f64)>, down: bool) -> ResultAction {
        if !down {
            self.bar.release();
        }
        if down && let Some((x, y)) = point {
            let at = (x.floor() as i32, y.floor() as i32);
            let press = self.bar.press_rows(at, self.bar_rows());
            match press {
                RowPress::Miss => {}
                RowPress::Less => self.scroll_by(-1),
                RowPress::More => self.scroll_by(1),
                RowPress::PageLess => self.scroll_by(-(VISIBLE as i32)),
                RowPress::PageMore => self.scroll_by(VISIBLE as i32),
                RowPress::Grab => {}
            }
            if press != RowPress::Miss {
                self.pressed = None;
                return ResultAction::None;
            }
        }
        let hit = point.and_then(|p| self.hit(p));
        if down {
            self.pressed = hit;
            return ResultAction::None;
        }
        let pressed = self.pressed.take();
        if pressed != hit {
            return ResultAction::None;
        }
        hit.map_or(ResultAction::None, |h| self.activate(h))
    }
    /// The pointer moved: a held scroll bar thumb follows it. True when the
    /// list scrolled, so the host redraws.
    pub fn moved(&mut self, point: Option<(f64, f64)>) -> bool {
        let before = self.first;
        if let Some((x, y)) = point
            && let Some(first) = self
                .bar
                .drag_rows((x.floor() as i32, y.floor() as i32), self.bar_rows())
        {
            self.scroll_to(first);
        }
        self.first != before
    }

    // ---- drawing --------------------------------------------------------

    pub fn draw(&self, pixels: &mut [u8], font: &Font) {
        chrome::title_bar(pixels, font, "GRAPHICS OPTIONS   |   3D view", self.context);
        self.draw_rows(pixels, font);
        self.draw_about(pixels, font);
        chrome::status_bar(pixels, font, &self.message);
        for (i, label) in FOOTER.iter().enumerate() {
            chrome::footer_button(
                pixels,
                font,
                Self::footer_rect(i),
                label,
                self.focus == Focus::Footer(i),
            );
        }
    }
    /// The panel's state at its top right: applying, or in use (and saved).
    fn state(&self) -> (&'static str, [u8; 4]) {
        if self.dirty() {
            ("APPLYING", TITLE)
        } else if self.saved {
            ("IN USE AND SAVED", GOOD)
        } else {
            ("IN USE, NOT SAVED", TITLE)
        }
    }
    fn draw_rows(&self, pixels: &mut [u8], font: &Font) {
        let panel = SETTINGS;
        Canvas(pixels).rect(panel, PANEL);
        Chrome::text(
            pixels,
            font,
            panel,
            TITLE,
            "3D VIEW SETTINGS",
            (panel.0 + 6, 29),
        );
        let (state, color) = self.state();
        let x = BAR_X - 4 - text_width(font, state);
        Chrome::text(pixels, font, panel, color, state, (x, 29));
        let rows = rows();
        for (index, line) in lines().into_iter().enumerate() {
            let Some(r) = self.line_rect(index) else {
                continue;
            };
            let y = r.1 + (r.3 - font.height as i32) / 2;
            match line {
                Line::Heading(title) => {
                    Chrome::text(pixels, font, r, TITLE, title, (r.0 + 4, y));
                    let after = r.0 + 4 + text_width(font, title) + 8;
                    let mid = r.1 + r.3 / 2;
                    Canvas(pixels).rect((after, mid, r.0 + r.2 - after, 1), BUTTON);
                }
                Line::Setting(row) => self.draw_setting(pixels, font, row, rows[row], r),
                Line::Resolution(_) => self.draw_resolution(pixels, font, r),
            }
        }
        let mut canvas = Canvas(pixels);
        self.bar.draw_rows(&mut canvas, self.bar_rows());
    }
    fn draw_setting(&self, pixels: &mut [u8], font: &Font, row: usize, kind: Row, r: Rect) {
        if self.focus == Focus::Row(row) {
            Canvas(pixels).rect(r, FOCUS);
            Canvas(pixels).rect((r.0, r.1, 3, r.3), GOOD);
        }
        let y = r.1 + (r.3 - font.height as i32) / 2;
        Chrome::text(pixels, font, r, WHITE, kind.label(), (r.0 + 14, y));
        let chosen = self.choice(row);
        for (i, label) in kind.choices().iter().enumerate() {
            let Some(c) = self.choice_rect(row, i) else {
                continue;
            };
            let available = self.available(row, i);
            let color = if i == chosen {
                Canvas(pixels).rect(c, PALE);
                INK
            } else {
                Canvas(pixels).outline(c, if available { OUTLINE } else { HEADER });
                if available { WHITE } else { MUTED }
            };
            let label = fit(font, label, c.2 - 4);
            let w = text_width(font, &label);
            let x = c.0 + (c.2 - w) / 2;
            let y = c.1 + (c.3 - font.height as i32) / 2;
            Chrome::text(pixels, font, c, color, &label, (x, y));
            if !available {
                // Struck through: shown for completeness, not selectable.
                let mid = c.1 + c.3 / 2;
                Canvas(pixels).rect(
                    (x - 2, mid, w + 4, 1),
                    if i == chosen { INK } else { MUTED },
                );
            }
        }
    }
    /// Under the render scale: the window's size, and under each choice the
    /// size the 3D view is drawn at with it, the chosen one brighter.
    fn draw_resolution(&self, pixels: &mut [u8], font: &Font, r: Rect) {
        let y = r.1 + (r.3 - font.height as i32) / 2;
        let label = self.resolution_label();
        Chrome::text(pixels, font, r, MUTED, &label, (r.0 + 24, y));
        for (i, percent) in RENDER_SCALES.iter().enumerate() {
            let Some(size) = self.scaled(*percent) else {
                break;
            };
            let x = Self::choice_x(i);
            let text = fit(font, &Self::size_text(size), CHOICE_W + CHOICE_GAP - 2);
            let at = x + (CHOICE_W - text_width(font, &text)) / 2;
            let color = if *percent == self.draft.render_scale {
                WHITE
            } else {
                MUTED
            };
            Chrome::text(
                pixels,
                font,
                (x - 2, r.1, CHOICE_W + CHOICE_GAP, r.3),
                color,
                &text,
                (at, y),
            );
        }
    }
    fn draw_about(&self, pixels: &mut [u8], font: &Font) {
        let panel = ABOUT;
        Canvas(pixels).rect(panel, PANEL);
        let (title, description, note) = match self.focus {
            Focus::Footer(i) => (
                FOOTER[i].to_ascii_uppercase(),
                if i == DEFAULTS {
                    "Put back the recommended settings. They are used and saved at once."
                } else {
                    "Close this screen. Every change is already in use and saved."
                },
                None,
            ),
            Focus::Row(_) => (
                rows()[self.row].label().to_ascii_uppercase(),
                rows()[self.row].description(),
                self.note(self.row),
            ),
        };
        Chrome::text(
            pixels,
            font,
            panel,
            TITLE,
            &format!("ABOUT: {title}"),
            (panel.0 + 6, panel.1 + 5),
        );
        let width = panel.2 - 16;
        Chrome::text(
            pixels,
            font,
            panel,
            WHITE,
            &fit(font, description, width),
            (panel.0 + 8, panel.1 + 26),
        );
        if let Some(note) = note {
            Chrome::text(
                pixels,
                font,
                panel,
                TITLE,
                &fit(font, &note, width),
                (panel.0 + 8, panel.1 + 44),
            );
        }
        Canvas(pixels).rect((panel.0 + 6, panel.1 + 66, panel.2 - 12, 1), BUTTON);
        Chrome::text(
            pixels,
            font,
            panel,
            MUTED,
            &fit(
                font,
                "These are optional presentation choices. Most changes are used and saved as you make them.",
                width,
            ),
            (panel.0 + 8, panel.1 + 76),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ALL: [bool; 4] = [true; 4];
    fn editor() -> Editor {
        Editor::new(Options::default(), ALL, "Main menu")
    }
    fn press(e: &mut Editor, key: &str) -> ResultAction {
        e.key(key, false)
    }
    fn center(r: Rect) -> (f64, f64) {
        ((r.0 + r.2 / 2) as f64, (r.1 + r.3 / 2) as f64)
    }
    fn click(e: &mut Editor, r: Rect) -> ResultAction {
        e.pointer(Some(center(r)), true);
        e.pointer(Some(center(r)), false)
    }
    fn choice(e: &Editor, row: usize, i: usize) -> Rect {
        e.choice_rect(row, i).expect("the row is shown")
    }
    fn row_of(kind: Row) -> usize {
        rows().iter().position(|r| *r == kind).unwrap()
    }
    /// A synthetic font: every byte a 5 pixel advance with one dot.
    fn font() -> Font {
        Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 5,
                    pixels: vec![(1, 3)],
                })
                .collect(),
        }
    }
    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("tore-graphics-{name}-{}", std::process::id()))
    }

    #[test]
    fn footer_is_defaults_and_close() {
        assert_eq!(FOOTER, ["Defaults", "Close"]);
        let mut e = editor();
        assert_eq!(press(&mut e, "Escape"), ResultAction::Close);
        assert_eq!(click(&mut e, Editor::footer_rect(1)), ResultAction::Close);
        // No Apply: nothing on the screen answers Save any more.
        for i in 0..FOOTER.len() {
            assert_ne!(
                click(&mut editor(), Editor::footer_rect(i)),
                ResultAction::Save
            );
        }
        // The footer stays inside the window, clear of the status line's hint.
        let last = Editor::footer_rect(FOOTER.len() - 1);
        assert!(last.0 + last.2 <= 640);
        assert!(Editor::footer_rect(0).0 >= 312);
    }
    #[test]
    fn rows_are_grouped_under_headings() {
        let lines = lines();
        let headings: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Heading(t) => Some(*t),
                _ => None,
            })
            .collect();
        assert_eq!(
            headings,
            [
                "DISPLAY",
                "TERRAIN AND SCENERY",
                "VEGETATION AND GRASS",
                "PERFORMANCE"
            ]
        );
        // Every row once, each under its own group's heading.
        let mut current = "";
        let mut seen = 0;
        for line in &lines {
            match line {
                Line::Heading(t) => current = t,
                Line::Setting(row) => {
                    assert_eq!(rows()[*row].group(), current);
                    assert_eq!(*row, seen, "rows keep their order");
                    seen += 1;
                }
                Line::Resolution(row) => assert_eq!(rows()[*row], Row::RenderScale),
            }
        }
        assert_eq!(seen, rows().len());
        assert!(lines.len() > VISIBLE, "the list is longer than its window");
        // The window and the About panel stay above the status bar.
        assert!(VIEW_TOP + VISIBLE as i32 * PITCH <= SETTINGS.1 + SETTINGS.3);
        assert!(SETTINGS.1 + SETTINGS.3 < ABOUT.1);
        assert!(ABOUT.1 + ABOUT.3 <= 438);
    }
    #[test]
    fn the_graphics_api_row_is_windows_only_and_says_restart() {
        assert!(rows_on(true).contains(&Row::GraphicsApi));
        assert!(!rows_on(false).contains(&Row::GraphicsApi));
        assert_eq!(Row::GraphicsApi.group(), "DISPLAY");
        assert_eq!(Row::GraphicsApi.choices(), ["Vulkan", "DirectX 12"]);
        assert!(Row::GraphicsApi.description().contains("Restart the game"));
        assert_eq!(GraphicsApi::CHOICES[1], GraphicsApi::Dx12);
        assert_eq!(crate::graphics_api::status_note(GraphicsApi::Vulkan), None);
    }
    #[test]
    fn defaults_keep_the_graphics_api_choice() {
        let mut e = editor();
        e.draft.graphics_api = GraphicsApi::Dx12;
        e.draft.render_scale = 150;
        e.defaults();
        assert_eq!(e.draft.graphics_api, GraphicsApi::Dx12);
        assert_eq!(e.draft.render_scale, 100);
    }
    #[test]
    fn grass_rows_edit_without_changing_tree_density() {
        let mut editor = editor();
        let tree = editor.draft.vegetation_density;
        let density = row_of(Row::GrassDensity);
        let distance = row_of(Row::GrassDistance);
        editor.set_choice(density, 0);
        editor.set_choice(distance, 3);
        assert_eq!(editor.draft.grass_density, None);
        assert_eq!(editor.draft.grass_distance_ft, 1000);
        assert_eq!(editor.draft.vegetation_density, tree);
        editor.set_choice(density, 4);
        assert_eq!(editor.draft.grass_density, Some(VegDensity::Ultra));
    }
    #[test]
    fn navigation_reaches_rows_and_footer_and_scrolls() {
        let mut e = editor();
        assert_eq!(e.focus, Focus::Row(0));
        press(&mut e, "ArrowUp");
        assert_eq!(e.focus, Focus::Row(0), "the top row stops the focus");
        let last = rows().len() - 1;
        for _ in 0..last {
            press(&mut e, "ArrowDown");
            // The focused row is always on screen.
            assert!(e.row_rect(e.row).is_some(), "row {} hidden", e.row);
        }
        assert_eq!(e.focus, Focus::Row(last));
        assert_eq!(e.first, e.bar_rows().max_first(), "scrolled to the end");
        press(&mut e, "ArrowDown");
        assert_eq!(e.focus, Focus::Footer(0));
        press(&mut e, "ArrowRight");
        press(&mut e, "ArrowRight");
        assert_eq!(e.focus, Focus::Footer(1));
        press(&mut e, "ArrowUp");
        assert_eq!(e.focus, Focus::Row(last));
        // Tab walks the two footer buttons and wraps; Shift-Tab goes back.
        for _ in 0..3 {
            press(&mut e, "Tab");
        }
        assert_eq!(e.focus, Focus::Row(0));
        assert_eq!(e.first, 0, "back at the top with its heading");
        e.key("Tab", true);
        assert_eq!(e.focus, Focus::Footer(1));
        assert_eq!(e.row, 0);
        assert_eq!(e.draft, Options::default(), "moving never edits");
    }
    #[test]
    fn values_step_cycle_and_click() {
        let mut e = editor();
        // Anti-aliasing: 4x to 8x, then stops at the end.
        assert_eq!(press(&mut e, "ArrowRight"), ResultAction::Changed);
        assert_eq!(e.draft.anti_aliasing, AntiAliasing::X8);
        assert_eq!(press(&mut e, "ArrowRight"), ResultAction::None);
        // Enter wraps round to Off.
        press(&mut e, "Enter");
        assert_eq!(e.draft.anti_aliasing, AntiAliasing::Off);
        press(&mut e, "ArrowDown");
        press(&mut e, "ArrowLeft");
        assert_eq!(e.draft.render_scale, 75);
        press(&mut e, "ArrowLeft");
        assert_eq!(e.draft.render_scale, 75);
        press(&mut e, "ArrowDown");
        press(&mut e, "ArrowRight");
        assert_eq!(e.draft.spotting_aid, SpottingAid::Strong);
        press(&mut e, "ArrowDown");
        press(&mut e, "ArrowLeft");
        assert_eq!(e.draft.frame_limit, FrameLimit::Fps200);
        // The next row differs by platform (Windows has the graphics API row).
        e.set_focus(Focus::Row(row_of(Row::TerrainFiltering)));
        press(&mut e, "Space");
        assert!(!e.draft.terrain_filtering);
        // A click on a choice picks it and focuses its row.
        let scale = row_of(Row::RenderScale);
        let r = choice(&e, scale, 4);
        assert_eq!(click(&mut e, r), ResultAction::Changed);
        assert_eq!(e.draft.render_scale, 200);
        assert_eq!(e.focus, Focus::Row(scale));
        let filtering = row_of(Row::TerrainFiltering);
        let r = choice(&e, filtering, 1);
        assert_eq!(click(&mut e, r), ResultAction::Changed);
        assert!(e.draft.terrain_filtering);
        // Press and release must land on the same control.
        let (a, b) = (choice(&e, scale, 0), choice(&e, scale, 2));
        e.pointer(Some(center(a)), true);
        e.pointer(Some(center(b)), false);
        assert_eq!(e.draft.render_scale, 200);
        assert!(e.dirty());
    }
    #[test]
    fn unsupported_anti_aliasing_is_shown_but_skipped() {
        let mut e = Editor::new(Options::default(), [true, false, true, false], "Main menu");
        assert_eq!(press(&mut e, "ArrowRight"), ResultAction::None);
        assert_eq!(e.draft.anti_aliasing, AntiAliasing::X4);
        press(&mut e, "ArrowLeft");
        assert_eq!(e.draft.anti_aliasing, AntiAliasing::Off, "2x is skipped");
        press(&mut e, "Enter");
        assert_eq!(e.draft.anti_aliasing, AntiAliasing::X4);
        let r = choice(&e, 0, 3);
        click(&mut e, r);
        assert_eq!(e.draft.anti_aliasing, AntiAliasing::X4);
        assert!(e.message.contains("8x is not available"), "{}", e.message);
        assert_eq!(
            e.note(0).unwrap(),
            "2x and 8x not available on this graphics card."
        );
        // A saved level this adapter lacks stays selected, with a note.
        let saved = Options {
            anti_aliasing: AntiAliasing::X8,
            ..Options::default()
        };
        let e = Editor::new(saved, [true, false, true, false], "Main menu");
        assert!(e.note(0).unwrap().starts_with("8x is not available"));
        assert_eq!(e.note(row_of(Row::SpottingAid)), None);
    }
    #[test]
    fn changes_apply_once_they_settle() {
        let mut e = editor();
        assert!(!e.dirty());
        assert_eq!(e.apply_at(), None, "nothing pending on opening");
        // Three quick steps: one pending change, due SETTLE after the last.
        let before = Instant::now();
        press(&mut e, "ArrowLeft");
        press(&mut e, "ArrowLeft");
        press(&mut e, "ArrowLeft");
        assert_eq!(e.draft.anti_aliasing, AntiAliasing::Off);
        assert!(e.dirty());
        let at = e.apply_at().expect("pending");
        assert!(at >= before + SETTLE);
        assert!(!e.apply_due(at - Duration::from_millis(1)));
        assert!(e.apply_due(at));
        assert_eq!(e.state().0, "APPLYING");
        // The host applies it: in use, saved, nothing pending.
        let dir = temp("settle");
        let path = dir.join("graphics-v1.conf");
        let applied = e.apply(Some(&path));
        assert_eq!(applied.anti_aliasing, AntiAliasing::Off);
        assert!(!e.dirty());
        assert_eq!(e.apply_at(), None);
        assert_eq!(e.state().0, "IN USE AND SAVED");
        assert_eq!(Options::load(&path).anti_aliasing, AntiAliasing::Off);
        // Picking the value already chosen is not a change.
        let off = choice(&e, 0, 0);
        assert_eq!(click(&mut e, off), ResultAction::None);
        assert!(!e.dirty());
        // A save that fails says so; the settings are still in use.
        e.draft.render_scale = 150;
        e.apply(Some(&path.join("inside-a-file")));
        assert!(e.message.contains("not saved"), "{}", e.message);
        assert_eq!(e.state().0, "IN USE, NOT SAVED");
        // Without a settings file they are in use for this session only.
        e.draft.render_scale = 125;
        assert_eq!(e.apply(None).render_scale, 125);
        assert_eq!(e.state().0, "IN USE, NOT SAVED");
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn defaults_apply_too() {
        let changed = Options {
            anti_aliasing: AntiAliasing::X2,
            render_scale: 150,
            ..Options::default()
        };
        let mut e = Editor::new(changed, ALL, "Main menu");
        assert!(!e.dirty());
        assert_eq!(
            click(&mut e, Editor::footer_rect(DEFAULTS)),
            ResultAction::Changed
        );
        assert_eq!(e.draft, Options::default());
        assert!(e.dirty(), "defaults are pending, to be applied");
        assert!(e.apply_at().is_some());
        let dir = temp("defaults");
        let path = dir.join("graphics-v1.conf");
        assert_eq!(e.apply(Some(&path)), Options::default());
        assert_eq!(Options::load(&path), Options::default());
        // Enter on the focused Defaults button does the same.
        let mut e = Editor::new(changed, ALL, "Main menu");
        e.set_focus(Focus::Footer(DEFAULTS));
        assert_eq!(press(&mut e, "Enter"), ResultAction::Changed);
        assert!(e.apply_due(Instant::now() + SETTLE));
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn wheel_arrows_track_and_thumb_scroll_the_list() {
        let mut e = editor();
        let total = lines().len();
        let max = total - VISIBLE;
        let bar = e.bar.bounds();
        // At the top the last group is out of view.
        let frame = row_of(Row::FrameRate);
        assert!(e.row_rect(frame).is_none());
        let thumb_top = |e: &Editor| {
            let r = e.bar_rows();
            e.bar
                .thumb(r.total as f64, r.visible as f64, r.first as f64)
                .0
        };
        let top = thumb_top(&e);
        // The wheel: down a notch is a line on; it stops at the end.
        e.wheel(-1);
        assert_eq!(e.first, 1);
        e.wheel(-100);
        assert_eq!(e.first, max);
        assert!(e.row_rect(frame).is_some(), "the last row is reachable");
        assert!(e.row_rect(0).is_none());
        assert!(thumb_top(&e) > top, "the thumb moved down");
        e.wheel(100);
        assert_eq!(e.first, 0);
        // The arrow buttons step a line.
        let less = (bar.0 + bar.2 / 2, bar.1 + 4);
        let more = (bar.0 + bar.2 / 2, bar.1 + bar.3 - 4);
        let at = |p: (i32, i32)| Some((p.0 as f64 + 0.5, p.1 as f64 + 0.5));
        e.pointer(at(more), true);
        e.pointer(at(more), false);
        assert_eq!(e.first, 1);
        e.pointer(at(less), true);
        e.pointer(at(less), false);
        assert_eq!(e.first, 0);
        // The track below the thumb pages on.
        e.pointer(at((bar.0 + 8, bar.1 + bar.3 - 20)), true);
        e.pointer(at((bar.0 + 8, bar.1 + bar.3 - 20)), false);
        assert_eq!(e.first, max.min(VISIBLE));
        e.wheel(100);
        // Dragging the thumb to the bottom shows the end.
        let grab = (bar.0 + 8, thumb_top(&e) + 2);
        e.pointer(at(grab), true);
        assert!(e.bar.dragging());
        e.moved(at((grab.0, bar.1 + bar.3)));
        assert_eq!(e.first, max);
        e.pointer(at((grab.0, bar.1 + bar.3)), false);
        assert!(!e.bar.dragging());
        // A press on the bar never edits or focuses anything.
        assert_eq!(e.draft, Options::default());
        assert_eq!(e.focus, Focus::Row(0));
        // A row scrolled into view takes clicks at its new place.
        let veg = row_of(Row::GrassDistance);
        let r = choice(&e, veg, 3);
        assert_eq!(click(&mut e, r), ResultAction::Changed);
        assert_eq!(e.draft.grass_distance_ft, 1000);
        // Scrolled away, a row takes none.
        e.wheel(100);
        assert!(e.choice_rect(veg, 3).is_none());
        assert_ne!(e.hit(center(r)), Some(Hit::Choice(veg, 3)));
    }
    #[test]
    fn the_render_scale_shows_the_window_and_drawn_sizes() {
        let mut e = editor();
        let scale = row_of(Row::RenderScale);
        assert_eq!(e.resolution_label(), "Window size not known yet");
        assert_eq!(e.note(scale), None);
        e.set_window(Some([3840, 2160]));
        assert_eq!(e.resolution_label(), "Window 3840x2160");
        assert_eq!(
            e.note(scale).unwrap(),
            "100%: the 3D view is drawn at the window's own 3840x2160."
        );
        let sizes = |e: &Editor| {
            RENDER_SCALES
                .map(|p| Editor::size_text(e.scaled(p).unwrap()))
                .to_vec()
        };
        assert_eq!(
            sizes(&e),
            [
                "2880x1620",
                "3840x2160",
                "4800x2700",
                "5760x3240",
                "7680x4320"
            ]
        );
        e.draft.render_scale = 75;
        assert_eq!(
            e.note(scale).unwrap(),
            "75%: the 3D view is drawn at 2880x1620 and upscaled to the 3840x2160 window."
        );
        // A resize is picked up; odd sizes round as the renderer does.
        e.set_window(Some([2560, 1440]));
        assert_eq!(sizes(&e)[0], "1920x1080");
        e.set_window(Some([1366, 768]));
        assert_eq!(
            sizes(&e),
            ["1025x576", "1366x768", "1708x960", "2049x1152", "2732x1536"]
        );
        // The renderer keeps every side within 8192 pixels.
        e.set_window(Some([5120, 2880]));
        assert_eq!(sizes(&e)[4], "8192x5760");
        e.draft.render_scale = 200;
        assert!(
            e.note(scale)
                .unwrap()
                .contains("downsampled to the 5120x2880")
        );
        // A minimised window has no size.
        e.set_window(Some([0, 0]));
        assert_eq!(e.resolution_label(), "Window size not known yet");
    }
    #[test]
    fn draws_every_focus_and_scroll_without_panicking() {
        let font = font();
        let mut pixels = vec![0; 640 * 480 * 4];
        let mut e = Editor::new(
            Options {
                anti_aliasing: AntiAliasing::X8,
                ..Options::default()
            },
            [true, true, true, false],
            "Main menu",
        );
        e.set_window(Some([3840, 2160]));
        for _ in 0..Editor::order().len() {
            e.draw(&mut pixels, &font);
            press(&mut e, "Tab");
        }
        // The header strip is the panel colour, the body the paper colour.
        assert_eq!(pixels[..4], PANEL);
        assert_eq!(pixels[(338 * 640 + 2) * 4..][..4], chrome::PAPER);
        e.wheel(-100);
        e.draw(&mut pixels, &font);
    }
    #[test]
    fn the_frame_rate_row_saves_and_comes_back() {
        let mut e = editor();
        let row = row_of(Row::FrameRate);
        for _ in 0..row {
            press(&mut e, "ArrowDown");
        }
        assert!(!e.draft.show_frame_rate);
        press(&mut e, "ArrowRight");
        assert!(e.draft.show_frame_rate);
        assert!(e.dirty());
        let dir = temp("fps");
        let path = dir.join("graphics-v1.conf");
        let applied = e.apply(Some(&path));
        assert!(applied.show_frame_rate);
        assert!(applied.text().contains("frame-rate true"));
        assert!(Options::load(&path).show_frame_rate);
        // Off again is the default, so the file omits the line.
        press(&mut e, "ArrowLeft");
        e.apply(Some(&path));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("frame-rate"));
        assert!(!Options::load(&path).show_frame_rate);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn shadow_distance_is_next_to_quality_and_applies_and_saves() {
        let row = row_of(Row::ShadowDistance);
        assert_eq!(rows()[row - 1], Row::Shadows);
        assert_eq!(Row::ShadowDistance.group(), "TERRAIN AND SCENERY");
        assert_eq!(
            Row::ShadowDistance.choices(),
            ["Trees", "1 nm", "2 nm", "4 nm", "6 nm", "8 nm"]
        );
        let mut e = editor();
        assert_eq!(e.choice(row), 0);
        let dir = temp("shadow-distance");
        let path = dir.join("graphics-v1.conf");
        e.set_focus(Focus::Row(row));
        for (i, distance) in ShadowDistance::ALL.into_iter().enumerate().skip(1) {
            let r = choice(&e, row, i);
            assert!(r.0 + r.2 < BAR_X, "every choice fits before the scroll bar");
            assert_eq!(click(&mut e, r), ResultAction::Changed);
            assert_eq!(e.apply(Some(&path)).shadow_distance, distance);
            assert_eq!(Options::load(&path).shadow_distance, distance);
        }
        // Keyboard cycles from 8 nm back to Trees, omitting the default line.
        e.set_focus(Focus::Row(row));
        press(&mut e, "Space");
        e.apply(Some(&path));
        assert_eq!(Options::load(&path).shadow_distance, ShadowDistance::Trees);
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("shadow-distance")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_shadow_row_sits_in_terrain_and_saves() {
        let row = row_of(Row::Shadows);
        assert_eq!(Row::Shadows.group(), "TERRAIN AND SCENERY");
        assert_eq!(Row::Shadows.label(), "Shadows");
        assert_eq!(
            Row::Shadows.choices(),
            ["Off", "Low", "Med", "High", "Ultra"]
        );
        let mut e = editor();
        // Med is the default.
        assert_eq!(e.choice(row), 2);
        let dir = temp("shadows");
        let path = dir.join("graphics-v1.conf");
        for (i, quality) in ShadowQuality::ALL.into_iter().enumerate() {
            let r = choice(&e, row, i);
            assert_eq!(click(&mut e, r), ResultAction::Changed);
            assert_eq!(e.draft.shadow_quality, quality);
            let applied = e.apply(Some(&path));
            assert_eq!(applied.shadow_quality, quality);
            assert_eq!(Options::load(&path).shadow_quality, quality);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("shadows ultra"));
        e.set_focus(Focus::Row(row));
        press(&mut e, "ArrowLeft");
        press(&mut e, "ArrowLeft");
        e.apply(Some(&path));
        // Back at Med the file leaves the line out.
        assert!(!std::fs::read_to_string(&path).unwrap().contains("shadows"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn the_frame_rate_choice_sits_in_display_and_saves() {
        let row = row_of(Row::FrameLimit);
        assert_eq!(Row::FrameLimit.group(), "DISPLAY");
        assert_eq!(Row::FrameLimit.label(), "Frame rate");
        assert_eq!(Row::FrameLimit.choices(), ["60", "100", "200", "V-sync"]);
        assert_eq!(rows()[row - 1], Row::SpottingAid);
        let mut e = editor();
        // V-sync is the default and the last choice.
        assert_eq!(e.choice(row), 3);
        let dir = temp("frame-limit");
        let path = dir.join("graphics-v1.conf");
        for (i, limit) in FrameLimit::ALL.into_iter().enumerate() {
            let r = choice(&e, row, i);
            // Each choice differs from the one before (V-sync, then 60...).
            assert_eq!(click(&mut e, r), ResultAction::Changed);
            assert_eq!(e.draft.frame_limit, limit);
            let applied = e.apply(Some(&path));
            assert_eq!(applied.frame_limit, limit);
            assert_eq!(Options::load(&path).frame_limit, limit);
        }
        // Back at V-sync the file leaves the line out.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("frame-limit"));
        // Left from V-sync steps to 200, then 100, then stops at 60.
        e.set_focus(Focus::Row(row));
        for expected in [
            FrameLimit::Fps200,
            FrameLimit::Fps100,
            FrameLimit::Fps60,
            FrameLimit::Fps60,
        ] {
            press(&mut e, "ArrowLeft");
            assert_eq!(e.draft.frame_limit, expected);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
