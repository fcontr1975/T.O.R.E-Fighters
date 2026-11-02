//! Player graphics choices for the 3D view: anti-aliasing, render scale, the
//! spotting aid, terrain distance filtering, city fill distances, the
//! exploratory vegetation distances, the frame rate and the frame-rate
//! counter, shadow quality and shadow distance. These are
//! opinionated host additions requested by John on 2026-09-22; the original
//! game had none of them. The vegetation rows were added on 2026-10-16, and
//! the frame-rate counter on 2026-10-17, and the frame rate choice on
//! 2026-10-19 (John chose its steps and the V-sync default), and the shadow
//! quality on 2026-10-20 (John asked for the setting; its levels and default
//! are agent choices). John requested separate shadow distance following
//! trees on 2026-10-21; its fixed steps are agent choices. The other values
//! and defaults are agent choices. John requested the graphics API choice on
//! 2026-11-02. They are saved in their own small file, `graphics-v1.conf`,
//! beside the preferences.
use std::path::Path;

/// Multisample anti-aliasing. 4x is the only count every GPU must support;
/// 2x and 8x are offered only when the adapter reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AntiAliasing {
    Off,
    X2,
    X4,
    X8,
}
impl AntiAliasing {
    pub const ALL: [Self; 4] = [Self::Off, Self::X2, Self::X4, Self::X8];
    pub fn samples(self) -> u32 {
        match self {
            Self::Off => 1,
            Self::X2 => 2,
            Self::X4 => 4,
            Self::X8 => 8,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::X2 => "2x",
            Self::X4 => "4x",
            Self::X8 => "8x",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::X2 => "2x",
            Self::X4 => "4x",
            Self::X8 => "8x",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == text)
    }
}

/// How strongly distant aircraft are outlined against the background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpottingAid {
    Off,
    Subtle,
    Strong,
}
impl SpottingAid {
    pub const ALL: [Self; 3] = [Self::Off, Self::Subtle, Self::Strong];
    /// Peak outline opacity handed to the shader.
    pub fn strength(self) -> f32 {
        match self {
            Self::Off => 0.,
            Self::Subtle => 0.5,
            Self::Strong => 0.85,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Subtle => "Subtle",
            Self::Strong => "Strong",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Subtle => "subtle",
            Self::Strong => "strong",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == text)
    }
}

/// Render scale steps, in percent of the window's pixel size.
pub const RENDER_SCALES: [u32; 5] = [75, 100, 125, 150, 200];
/// The size the 3D view is drawn at for an `output` size and a render scale
/// in percent. The renderer and the Graphics screen both use it.
pub fn scaled_size(output: [u32; 2], percent: u32) -> [u32; 2] {
    let scale = percent as f32 / 100.;
    output.map(|v| ((v as f32 * scale).round() as u32).clamp(1, 8192))
}

/// City fill building draw distance steps, nautical miles (CF0 prototype;
/// John, 2026-10-10).
pub const BUILDING_DISTANCES: [u32; 3] = [10, 20, 30];
/// City fill building texture and detail distance steps, nautical miles:
/// full detail inside, plain boxes beyond.
pub const BUILDING_DETAILS: [u32; 5] = [1, 2, 5, 10, 20];
/// The defaults: geometry to 30 nm, full detail inside 5 nm (John,
/// 2026-10-10).
pub const DEFAULT_BUILDING_DISTANCE: u32 = 30;
pub const DEFAULT_BUILDING_DETAIL: u32 = 5;

/// Close-range ground detail (fields, grass, sand, rock) on the terrain.
/// Opinionated, 2026-10-16. Off draws the retail tile alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroundTexture {
    Off,
    On,
    /// A finer grain, for a low pass.
    Fine,
}
impl GroundTexture {
    pub const ALL: [Self; 3] = [Self::Off, Self::On, Self::Fine];
    /// Shader mode: 0 off, 1 on, 2 fine.
    pub fn mode(self) -> f32 {
        match self {
            Self::Off => 0.,
            Self::On => 1.,
            Self::Fine => 2.,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::On => "On",
            Self::Fine => "Fine",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::Fine => "fine",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == text)
    }
}

/// How thick the vegetation objects are, relative to the pre-Pass-5 High setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VegDensity {
    Low,
    Medium,
    High,
    Ultra,
}
impl VegDensity {
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];
    /// John requested a one-step reduction on 2026-10-17. Low is an agent choice.
    pub fn scale(self) -> f32 {
        match self {
            Self::Low => 0.2,
            Self::Medium => 0.4,
            Self::High => 0.7,
            Self::Ultra => 1.,
        }
    }
    pub fn object_budget(self) -> usize {
        match self {
            Self::Low => 32_768,
            Self::Medium => 65_536,
            Self::High => 131_072,
            Self::Ultra => 262_144,
        }
    }
    pub fn grass(self) -> (f32, u32) {
        match self {
            Self::Low => (450., 24),
            Self::Medium => (700., 48),
            Self::High => (1_100., 96),
            Self::Ultra => (1_500., 160),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Med",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.key() == text)
    }
}

/// Presentation geometry only. Environment override is useful for the lead matrix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum VegCards {
    Billboard,
    Cross,
    CrossTop,
    #[default]
    BillboardTop,
}
impl VegCards {
    pub const ALL: [Self; 4] = [
        Self::Billboard,
        Self::Cross,
        Self::CrossTop,
        Self::BillboardTop,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Billboard => "billboard",
            Self::Cross => "cross",
            Self::CrossTop => "cross-top",
            Self::BillboardTop => "billboard-top",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Billboard => "Facing",
            Self::Cross => "Cross",
            Self::CrossTop => "Cross+top",
            Self::BillboardTop => "Face+top",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == text)
    }
    pub fn effective(self) -> Self {
        std::env::var("TORE_VEGETATION_CARDS")
            .ok()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or(self)
    }
    pub fn vertices(self) -> u32 {
        match self {
            Self::Billboard => 6,
            Self::CrossTop => 18,
            _ => 12,
        }
    }
}

/// Presentation only. John requested procedural blades on 2026-10-17.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Grass {
    #[default]
    Blades,
    Tufts,
    Off,
}
impl Grass {
    pub const ALL: [Self; 3] = [Self::Blades, Self::Tufts, Self::Off];
    pub fn key(self) -> &'static str {
        match self {
            Self::Blades => "blades",
            Self::Tufts => "tufts",
            Self::Off => "off",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Blades => "Blades",
            Self::Tufts => "Tufts",
            Self::Off => "Off",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == text)
    }
    pub fn override_from_env() -> crate::AppResult<Option<Self>> {
        match std::env::var("TORE_GRASS") {
            Err(std::env::VarError::NotPresent) => Ok(None),
            Ok(s) => Self::parse(&s)
                .map(Some)
                .ok_or_else(|| "TORE_GRASS must be blades, tufts or off".into()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn effective(self) -> Self {
        Self::override_from_env().ok().flatten().unwrap_or(self)
    }
}

/// Which graphics API the game asks for. Opinionated, requested by John on
/// 2026-11-02 after issue #8. A Windows player sees Vulkan and DirectX 12. Auto is
/// not offered: it means nothing is saved yet, and `crate::graphics_api` then uses
/// Vulkan when a hardware Vulkan adapter exists, otherwise DirectX 12. Elsewhere only Auto
/// exists, which leaves the choice to wgpu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphicsApi {
    #[default]
    Auto,
    Vulkan,
    Dx12,
}
impl GraphicsApi {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Vulkan, Self::Dx12];
    /// What the player chooses between.
    pub const CHOICES: [Self; 2] = [Self::Vulkan, Self::Dx12];
    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Vulkan => "vulkan",
            Self::Dx12 => "dx12",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Vulkan => "Vulkan",
            Self::Dx12 => "DirectX 12",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == text)
    }
    /// A `--graphics-api` value, on this platform.
    pub fn from_flag(value: &str) -> Result<Self, String> {
        Self::from_flag_on(value, cfg!(target_os = "windows"))
    }
    fn from_flag_on(value: &str, windows: bool) -> Result<Self, String> {
        match Self::parse(value) {
            None => Err("--graphics-api needs auto, vulkan or dx12".into()),
            Some(Self::Auto) => Ok(Self::Auto),
            Some(api) if windows => Ok(api),
            Some(api) => Err(format!(
                "--graphics-api {} is available only on Windows; on this platform the game uses auto",
                api.key()
            )),
        }
    }
    /// The last `--graphics-api` in `flags`, if any. It lasts one run: it stays
    /// out of [`Options`] so a Graphics screen edit cannot save it by accident.
    pub fn flag(flags: &[(String, String)]) -> Result<Option<Self>, String> {
        let mut chosen = None;
        for (flag, value) in flags {
            if flag == "--graphics-api" {
                chosen = Some(Self::from_flag(value)?);
            }
        }
        Ok(chosen)
    }
}

/// How often the window is drawn. V-sync draws one frame per display refresh,
/// waiting for the display; the numbers cap the rate with a timer and present
/// without waiting. John chose these steps and the V-sync default on
/// 2026-10-19; `crate::frame_pacing` applies them, and `TORE_FRAME_LIMIT`
/// overrides them for one run (including `off`, uncapped, for benchmarks).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameLimit {
    Fps60,
    Fps100,
    Fps200,
    #[default]
    VSync,
}
impl FrameLimit {
    pub const ALL: [Self; 4] = [Self::Fps60, Self::Fps100, Self::Fps200, Self::VSync];
    /// Frames a second for the capped choices; None for V-sync.
    pub fn rate(self) -> Option<u32> {
        match self {
            Self::Fps60 => Some(60),
            Self::Fps100 => Some(100),
            Self::Fps200 => Some(200),
            Self::VSync => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Fps60 => "60",
            Self::Fps100 => "100",
            Self::Fps200 => "200",
            Self::VSync => "V-sync",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Fps60 => "60",
            Self::Fps100 => "100",
            Self::Fps200 => "200",
            Self::VSync => "vsync",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == text)
    }
}

/// How detailed the geometric shadows are: the shadow map size, how many
/// map cascades, the soft-edge sample count and which objects cast
/// (`crate::surface_lighting::Settings`; docs/spec/surface-lighting.md).
/// Off draws no geometric shadows. Medium is the default: the shadow maps
/// from before the setting existed, with every ground object casting; the
/// trees' shadows (High) cost about 0.5 to 0.8 ms at 1440p over woods
/// (agent choice, 2026-10-20).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowQuality {
    Off,
    Low,
    #[default]
    Medium,
    High,
    Ultra,
}
impl ShadowQuality {
    pub const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Ultra];
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Med",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Ultra => "ultra",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == text)
    }
    /// `TORE_SHADOWS=off|low|medium|high|ultra` sets the quality for one
    /// run (offscreen tools and captures included) without saving it.
    pub fn effective(self) -> Self {
        std::env::var("TORE_SHADOWS")
            .ok()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or(self)
    }
}

/// Shadow reach independent of quality. Trees follows the current vegetation
/// draw distance; fixed steps are nautical miles (agent choices, 2026-10-21).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowDistance {
    #[default]
    Trees,
    Nm1,
    Nm2,
    Nm4,
    Nm6,
    Nm8,
}
impl ShadowDistance {
    pub const ALL: [Self; 6] = [
        Self::Trees,
        Self::Nm1,
        Self::Nm2,
        Self::Nm4,
        Self::Nm6,
        Self::Nm8,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Trees => "trees",
            Self::Nm1 => "1",
            Self::Nm2 => "2",
            Self::Nm4 => "4",
            Self::Nm6 => "6",
            Self::Nm8 => "8",
        }
    }
    pub fn label(self) -> String {
        if self == Self::Trees {
            "Trees".into()
        } else {
            format!("{} nm", self.key())
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|v| v.key() == text)
    }
    pub fn nautical_miles(self, trees_nm: u32) -> u32 {
        match self {
            Self::Trees => trees_nm,
            Self::Nm1 => 1,
            Self::Nm2 => 2,
            Self::Nm4 => 4,
            Self::Nm6 => 6,
            Self::Nm8 => 8,
        }
    }
    /// Like TORE_SHADOWS, a measuring override that does not change the file.
    pub fn effective(self) -> Self {
        std::env::var("TORE_SHADOW_DISTANCE")
            .ok()
            .as_deref()
            .and_then(Self::parse)
            .unwrap_or(self)
    }
}

/// How far the close-range ground detail reaches, nautical miles.
pub const GROUND_TEXTURE_DISTANCES: [u32; 3] = [1, 2, 4];
/// How far vegetation objects are drawn, nautical miles.
pub const VEGETATION_DISTANCES: [u32; 4] = [2, 4, 6, 8];
/// Full vegetation meshes inside this, cheaper crosses beyond, nautical miles.
pub const VEGETATION_DETAILS: [u32; 4] = [1, 2, 3, 4];
/// Tuned for an RTX 4070 at 4K: detail close in, objects out to 4 nm, full
/// meshes inside 2 nm, high density (2026-10-16).
pub const DEFAULT_GROUND_TEXTURE: GroundTexture = GroundTexture::On;
pub const DEFAULT_GROUND_TEXTURE_DISTANCE: u32 = 2;
pub const DEFAULT_VEGETATION_DISTANCE: u32 = 4;
pub const DEFAULT_VEGETATION_DETAIL: u32 = 2;
pub const GRASS_DISTANCES: [u32; 4] = [250, 500, 750, 1000];
pub const DEFAULT_GRASS_DISTANCE: u32 = 500;
pub const DEFAULT_GRASS_DENSITY: Option<VegDensity> = Some(VegDensity::High);
pub const DEFAULT_VEGETATION_DENSITY: VegDensity = VegDensity::High;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub anti_aliasing: AntiAliasing,
    /// Percent; one of `RENDER_SCALES`.
    pub render_scale: u32,
    pub spotting_aid: SpottingAid,
    pub terrain_filtering: bool,
    /// City fill buildings draw out to this, nautical miles; one of
    /// `BUILDING_DISTANCES`.
    pub building_distance_nm: u32,
    /// City fill buildings draw with full texture and detail inside this,
    /// nautical miles; one of `BUILDING_DETAILS`.
    pub building_detail_nm: u32,
    /// Close-range ground detail. One of `GroundTexture::ALL`.
    pub ground_texture: GroundTexture,
    /// How far that detail reaches, nautical miles; one of
    /// `GROUND_TEXTURE_DISTANCES`.
    pub ground_texture_distance_nm: u32,
    /// Vegetation objects draw out to this, nautical miles; one of
    /// `VEGETATION_DISTANCES`.
    pub vegetation_distance_nm: u32,
    /// Full vegetation meshes inside this, nautical miles; one of
    /// `VEGETATION_DETAILS`.
    pub vegetation_detail_nm: u32,
    /// How thick the vegetation is. One of `VegDensity::ALL`.
    pub vegetation_density: VegDensity,
    pub vegetation_cards: VegCards,
    pub grass: Grass,
    pub grass_density: Option<VegDensity>,
    pub grass_distance_ft: u32,
    /// The frames-per-second readout in the window's top right. Off by
    /// default (John, 2026-10-17). `TORE_SHOW_FPS=1` shows it for one run
    /// without saving.
    pub show_frame_rate: bool,
    /// How often the window is drawn. V-sync by default (John, 2026-10-19).
    pub frame_limit: FrameLimit,
    /// How detailed the geometric shadows are, and what casts them.
    pub shadow_quality: ShadowQuality,
    /// Shadow reach, following trees by default.
    pub shadow_distance: ShadowDistance,
    /// The graphics API for the next launch; Auto until the player chooses. Only
    /// Windows offers a choice; a file carried from Windows keeps its value
    /// elsewhere and is ignored.
    pub graphics_api: GraphicsApi,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            anti_aliasing: AntiAliasing::X4,
            render_scale: 100,
            spotting_aid: SpottingAid::Subtle,
            terrain_filtering: true,
            building_distance_nm: DEFAULT_BUILDING_DISTANCE,
            building_detail_nm: DEFAULT_BUILDING_DETAIL,
            ground_texture: DEFAULT_GROUND_TEXTURE,
            ground_texture_distance_nm: DEFAULT_GROUND_TEXTURE_DISTANCE,
            vegetation_distance_nm: DEFAULT_VEGETATION_DISTANCE,
            vegetation_detail_nm: DEFAULT_VEGETATION_DETAIL,
            vegetation_density: DEFAULT_VEGETATION_DENSITY,
            vegetation_cards: VegCards::default(),
            grass: Grass::default(),
            grass_density: DEFAULT_GRASS_DENSITY,
            grass_distance_ft: DEFAULT_GRASS_DISTANCE,
            show_frame_rate: false,
            frame_limit: FrameLimit::default(),
            shadow_quality: ShadowQuality::default(),
            shadow_distance: ShadowDistance::default(),
            graphics_api: GraphicsApi::default(),
        }
    }
}
impl Options {
    /// Every addition off: the unmodified presentation, for pixel tests.
    /// Vegetation objects keep their distances, as the city buildings do;
    /// the ground detail texture turns off with the other image filters.
    pub fn original() -> Self {
        Self {
            anti_aliasing: AntiAliasing::Off,
            render_scale: 100,
            spotting_aid: SpottingAid::Off,
            terrain_filtering: false,
            building_distance_nm: DEFAULT_BUILDING_DISTANCE,
            building_detail_nm: DEFAULT_BUILDING_DETAIL,
            ground_texture: GroundTexture::Off,
            ground_texture_distance_nm: DEFAULT_GROUND_TEXTURE_DISTANCE,
            vegetation_distance_nm: DEFAULT_VEGETATION_DISTANCE,
            vegetation_detail_nm: DEFAULT_VEGETATION_DETAIL,
            vegetation_density: DEFAULT_VEGETATION_DENSITY,
            vegetation_cards: VegCards::Billboard,
            grass: Grass::Tufts,
            grass_density: DEFAULT_GRASS_DENSITY,
            grass_distance_ft: DEFAULT_GRASS_DISTANCE,
            show_frame_rate: false,
            frame_limit: FrameLimit::default(),
            shadow_quality: ShadowQuality::default(),
            shadow_distance: ShadowDistance::default(),
            graphics_api: GraphicsApi::default(),
        }
    }
    pub fn scale(&self) -> f32 {
        self.render_scale as f32 / 100.
    }
    /// The size the 3D view is drawn at for an `output` (window) size in
    /// physical pixels: the render scale applied, rounded, and kept within
    /// the 8192 pixel texture limit.
    pub fn scaled_size(&self, output: [u32; 2]) -> [u32; 2] {
        scaled_size(output, self.render_scale)
    }
    /// The settings file. The city fill building distances are written only
    /// when they differ from the defaults, so a default file is unchanged.
    pub fn text(&self) -> String {
        let mut text = format!(
            "tore-graphics 1\nanti-aliasing {}\nrender-scale {}\nspotting-aid {}\nterrain-filtering {}\n",
            self.anti_aliasing.key(),
            self.render_scale,
            self.spotting_aid.key(),
            self.terrain_filtering,
        );
        if self.building_distance_nm != DEFAULT_BUILDING_DISTANCE {
            text += &format!("building-distance {}\n", self.building_distance_nm);
        }
        if self.building_detail_nm != DEFAULT_BUILDING_DETAIL {
            text += &format!("building-detail {}\n", self.building_detail_nm);
        }
        if self.ground_texture != DEFAULT_GROUND_TEXTURE {
            text += &format!("vegetation-texture {}\n", self.ground_texture.key());
        }
        if self.ground_texture_distance_nm != DEFAULT_GROUND_TEXTURE_DISTANCE {
            text += &format!(
                "vegetation-texture-distance {}\n",
                self.ground_texture_distance_nm
            );
        }
        if self.vegetation_distance_nm != DEFAULT_VEGETATION_DISTANCE {
            text += &format!("vegetation-distance {}\n", self.vegetation_distance_nm);
        }
        if self.vegetation_detail_nm != DEFAULT_VEGETATION_DETAIL {
            text += &format!("vegetation-detail {}\n", self.vegetation_detail_nm);
        }
        if self.vegetation_density != DEFAULT_VEGETATION_DENSITY {
            text += &format!("vegetation-density {}\n", self.vegetation_density.key());
        }
        if self.vegetation_cards != VegCards::default() {
            text += &format!("vegetation-cards {}\n", self.vegetation_cards.key());
        }
        if self.grass != Grass::default() {
            text += &format!("grass {}\n", self.grass.key());
        }
        if self.grass_density != DEFAULT_GRASS_DENSITY {
            text += &format!(
                "grass-density {}\n",
                self.grass_density.map_or("off", VegDensity::key)
            );
        }
        if self.grass_distance_ft != DEFAULT_GRASS_DISTANCE {
            text += &format!("grass-distance {}\n", self.grass_distance_ft);
        }
        if self.show_frame_rate {
            text += "frame-rate true\n";
        }
        if self.frame_limit != FrameLimit::default() {
            text += &format!("frame-limit {}\n", self.frame_limit.key());
        }
        if self.shadow_distance != ShadowDistance::default() {
            text += &format!("shadow-distance {}\n", self.shadow_distance.key());
        }
        if self.shadow_quality != ShadowQuality::default() {
            text += &format!("shadows {}\n", self.shadow_quality.key());
        }
        if self.graphics_api != GraphicsApi::default() {
            text += &format!("graphics-api {}\n", self.graphics_api.key());
        }
        text
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 4096 {
            return Err("graphics settings too large".into());
        }
        let mut lines = text.lines();
        if lines.next() != Some("tore-graphics 1") {
            return Err("unsupported graphics settings version".into());
        }
        let mut values = std::collections::BTreeMap::new();
        for line in lines {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 2 || values.insert(fields[0], fields[1]).is_some() {
                return Err("invalid or duplicate graphics setting".into());
            }
        }
        let optional = [
            "building-distance",
            "building-detail",
            "vegetation-texture",
            "vegetation-texture-distance",
            "vegetation-distance",
            "vegetation-detail",
            "vegetation-density",
            "vegetation-cards",
            "grass",
            "grass-density",
            "grass-distance",
            "frame-rate",
            "frame-limit",
            "shadows",
            "shadow-distance",
            "graphics-api",
        ]
        .iter()
        .filter(|k| values.contains_key(*k))
        .count();
        if values.len() != 4 + optional {
            return Err("unknown or missing graphics setting".into());
        }
        let get = |k: &str| values.get(k).copied().ok_or(format!("missing {k}"));
        let step = |k: &str, steps: &[u32], default: u32| match values.get(k) {
            None => Ok(default),
            Some(v) => v
                .parse::<u32>()
                .ok()
                .filter(|n| steps.contains(n))
                .ok_or(format!("invalid {k}")),
        };
        let boolean = |k: &str| get(k)?.parse::<bool>().map_err(|_| format!("invalid {k}"));
        let render_scale = get("render-scale")?
            .parse::<u32>()
            .ok()
            .filter(|s| RENDER_SCALES.contains(s))
            .ok_or("invalid render-scale")?;
        Ok(Self {
            anti_aliasing: AntiAliasing::parse(get("anti-aliasing")?)
                .ok_or("invalid anti-aliasing")?,
            render_scale,
            spotting_aid: SpottingAid::parse(get("spotting-aid")?).ok_or("invalid spotting-aid")?,
            terrain_filtering: boolean("terrain-filtering")?,
            building_distance_nm: step(
                "building-distance",
                &BUILDING_DISTANCES,
                DEFAULT_BUILDING_DISTANCE,
            )?,
            building_detail_nm: step(
                "building-detail",
                &BUILDING_DETAILS,
                DEFAULT_BUILDING_DETAIL,
            )?,
            ground_texture: match values.get("vegetation-texture") {
                None => DEFAULT_GROUND_TEXTURE,
                Some(v) => GroundTexture::parse(v).ok_or("invalid vegetation-texture")?,
            },
            ground_texture_distance_nm: step(
                "vegetation-texture-distance",
                &GROUND_TEXTURE_DISTANCES,
                DEFAULT_GROUND_TEXTURE_DISTANCE,
            )?,
            vegetation_distance_nm: step(
                "vegetation-distance",
                &VEGETATION_DISTANCES,
                DEFAULT_VEGETATION_DISTANCE,
            )?,
            vegetation_detail_nm: step(
                "vegetation-detail",
                &VEGETATION_DETAILS,
                DEFAULT_VEGETATION_DETAIL,
            )?,
            vegetation_density: match values.get("vegetation-density") {
                None => DEFAULT_VEGETATION_DENSITY,
                Some(v) => VegDensity::parse(v).ok_or("invalid vegetation-density")?,
            },
            vegetation_cards: match values.get("vegetation-cards") {
                None => VegCards::default(),
                Some(v) => VegCards::parse(v).ok_or("invalid vegetation-cards")?,
            },
            grass: match values.get("grass") {
                None => Grass::default(),
                Some(v) => Grass::parse(v).ok_or("invalid grass")?,
            },
            grass_density: match values.get("grass-density") {
                None => DEFAULT_GRASS_DENSITY,
                Some(&"off") => None,
                Some(v) => Some(VegDensity::parse(v).ok_or("invalid grass-density")?),
            },
            grass_distance_ft: step("grass-distance", &GRASS_DISTANCES, DEFAULT_GRASS_DISTANCE)?,
            show_frame_rate: match values.get("frame-rate") {
                None => false,
                Some(_) => boolean("frame-rate")?,
            },
            frame_limit: match values.get("frame-limit") {
                None => FrameLimit::default(),
                Some(v) => FrameLimit::parse(v).ok_or("invalid frame-limit")?,
            },
            shadow_distance: match values.get("shadow-distance") {
                None => ShadowDistance::default(),
                Some(v) => ShadowDistance::parse(v).ok_or("invalid shadow-distance")?,
            },
            shadow_quality: match values.get("shadows") {
                None => ShadowQuality::default(),
                Some(v) => ShadowQuality::parse(v).ok_or("invalid shadows")?,
            },
            // Unlike the visual settings, a value this build does not know falls
            // back alone. Rejecting it would throw away every other setting in a
            // file written by a newer build, and this one matters only at launch.
            graphics_api: values
                .get("graphics-api")
                .and_then(|v| GraphicsApi::parse(v))
                .unwrap_or_default(),
        })
    }
    /// Command-line overrides for one run; they are not saved by themselves.
    pub fn apply_flags(&mut self, flags: &[(String, String)]) -> Result<(), String> {
        let switch = |flag: &str, value: &str| match value {
            "on" => Ok(true),
            "off" => Ok(false),
            _ => Err(format!("{flag} needs on or off")),
        };
        for (flag, value) in flags {
            match flag.as_str() {
                "--original-graphics" => {
                    *self = Self {
                        graphics_api: self.graphics_api,
                        ..Self::original()
                    }
                }
                // Applied by `crate::graphics_api` for this run only; see
                // `GraphicsApi::flag`.
                "--graphics-api" => {
                    GraphicsApi::from_flag(value)?;
                }
                "--anti-aliasing" => {
                    self.anti_aliasing = AntiAliasing::parse(value)
                        .ok_or("--anti-aliasing needs off, 2x, 4x or 8x")?
                }
                "--render-scale" => {
                    self.render_scale = value
                        .parse()
                        .ok()
                        .filter(|s| RENDER_SCALES.contains(s))
                        .ok_or("--render-scale needs 75, 100, 125, 150 or 200")?
                }
                "--spotting-aid" => {
                    self.spotting_aid = SpottingAid::parse(value)
                        .ok_or("--spotting-aid needs off, subtle or strong")?
                }
                "--terrain-filtering" => self.terrain_filtering = switch(flag, value)?,
                "--building-distance" => {
                    self.building_distance_nm = value
                        .parse()
                        .ok()
                        .filter(|n| BUILDING_DISTANCES.contains(n))
                        .ok_or("--building-distance needs 10, 20 or 30")?
                }
                "--building-detail" => {
                    self.building_detail_nm = value
                        .parse()
                        .ok()
                        .filter(|n| BUILDING_DETAILS.contains(n))
                        .ok_or("--building-detail needs 1, 2, 5, 10 or 20")?
                }
                "--vegetation-texture" => {
                    self.ground_texture = GroundTexture::parse(value)
                        .ok_or("--vegetation-texture needs off, on or fine")?
                }
                "--vegetation-texture-distance" => {
                    self.ground_texture_distance_nm = value
                        .parse()
                        .ok()
                        .filter(|n| GROUND_TEXTURE_DISTANCES.contains(n))
                        .ok_or("--vegetation-texture-distance needs 1, 2 or 4")?
                }
                "--vegetation-distance" => {
                    self.vegetation_distance_nm = value
                        .parse()
                        .ok()
                        .filter(|n| VEGETATION_DISTANCES.contains(n))
                        .ok_or("--vegetation-distance needs 2, 4, 6 or 8")?
                }
                "--vegetation-detail" => {
                    self.vegetation_detail_nm = value
                        .parse()
                        .ok()
                        .filter(|n| VEGETATION_DETAILS.contains(n))
                        .ok_or("--vegetation-detail needs 1, 2, 3 or 4")?
                }
                "--shadow-distance" => {
                    self.shadow_distance = ShadowDistance::parse(value)
                        .ok_or("--shadow-distance needs trees, 1, 2, 4, 6 or 8")?
                }
                "--shadows" => {
                    self.shadow_quality = ShadowQuality::parse(value)
                        .ok_or("--shadows needs off, low, medium, high or ultra")?
                }
                "--vegetation-density" => {
                    self.vegetation_density = VegDensity::parse(value)
                        .ok_or("--vegetation-density needs low, medium, high or ultra")?
                }
                _ => return Err(format!("unknown graphics flag {flag}")),
            }
        }
        Ok(())
    }
    /// A missing file gives the defaults. An unreadable or damaged one does
    /// too, with a warning in the session log saying why, like the other
    /// settings files.
    pub fn load(path: &Path) -> Self {
        match crate::preferences::read(path) {
            Ok(text) => Self::parse(&text).unwrap_or_else(|error| {
                log::warn!(
                    "Graphics options not loaded from {}: {error}; using the defaults",
                    path.display()
                );
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                log::warn!(
                    "Graphics options not loaded from {}: {error}; using the defaults",
                    path.display()
                );
                Self::default()
            }
        }
    }
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::preferences::write(path, &self.text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn density_steps_shift_down_and_default_remains_high() {
        assert_eq!(VegDensity::ALL.map(VegDensity::scale), [0.2, 0.4, 0.7, 1.]);
        assert_eq!(
            VegDensity::ALL.map(VegDensity::object_budget),
            [32768, 65536, 131072, 262144]
        );
        assert_eq!(
            VegDensity::ALL.map(VegDensity::grass),
            [(450., 24), (700., 48), (1100., 96), (1500., 160)]
        );
        assert_eq!(Options::default().vegetation_density, VegDensity::High);
    }
    #[test]
    fn all_card_modes_roundtrip_and_have_the_expected_card_count() {
        for mode in VegCards::ALL {
            assert_eq!(VegCards::parse(mode.key()), Some(mode));
            let options = Options {
                vegetation_cards: mode,
                ..Options::default()
            };
            assert_eq!(
                Options::parse(&options.text()).unwrap().vegetation_cards,
                mode
            );
        }
        assert_eq!(VegCards::default(), VegCards::BillboardTop);
        assert_eq!(Options::default().vegetation_cards, VegCards::BillboardTop);
        assert_eq!(
            Options::parse(&Options::default().text())
                .unwrap()
                .vegetation_cards,
            VegCards::BillboardTop
        );
        assert_eq!(Options::original().vegetation_cards, VegCards::Billboard);
        assert_eq!(VegCards::ALL.map(VegCards::vertices), [6, 12, 18, 12]);
        assert_eq!(VegCards::parse("triangle"), None);
    }
    #[test]
    fn grass_density_and_distance_are_independent_and_backwards_compatible() {
        let old = Options::default().text();
        let parsed = Options::parse(&old).unwrap();
        assert_eq!(parsed.grass_density, Some(VegDensity::High));
        assert_eq!(parsed.grass_distance_ft, 500);
        for density in std::iter::once(None).chain(VegDensity::ALL.map(Some)) {
            for distance in GRASS_DISTANCES {
                let options = Options {
                    grass_density: density,
                    grass_distance_ft: distance,
                    ..parsed
                };
                assert_eq!(Options::parse(&options.text()).unwrap(), options);
                assert_eq!(options.vegetation_density, parsed.vegetation_density);
            }
        }
        for value in [
            "grass-density invalid",
            "grass-distance 0",
            "grass-distance 501",
        ] {
            assert!(Options::parse(&format!("{old}{value}\n")).is_err());
        }
    }
    #[test]
    fn grass_modes_default_and_roundtrip() {
        assert_eq!(Options::default().grass, Grass::Blades);
        for grass in Grass::ALL {
            let options = Options {
                grass,
                ..Default::default()
            };
            assert_eq!(Options::parse(&options.text()).unwrap().grass, grass);
            assert_eq!(Grass::parse(grass.key()), Some(grass));
        }
        assert_eq!(Grass::parse("invalid"), None);
        assert_eq!(
            Options::parse(&Options::default().text()).unwrap().grass,
            Grass::Blades
        );
    }
    #[test]
    fn roundtrip_and_reject_malformed() {
        let options = Options {
            anti_aliasing: AntiAliasing::X8,
            render_scale: 150,
            spotting_aid: SpottingAid::Strong,
            terrain_filtering: false,
            building_distance_nm: 20,
            building_detail_nm: 2,
            ground_texture: GroundTexture::Fine,
            ground_texture_distance_nm: 4,
            vegetation_distance_nm: 6,
            vegetation_detail_nm: 1,
            vegetation_density: VegDensity::Low,
            vegetation_cards: VegCards::CrossTop,
            grass: Grass::Off,
            grass_density: Some(VegDensity::Ultra),
            grass_distance_ft: 1000,
            show_frame_rate: true,
            frame_limit: FrameLimit::Fps100,
            shadow_quality: ShadowQuality::Low,
            shadow_distance: ShadowDistance::Nm6,
            graphics_api: GraphicsApi::Dx12,
        };
        assert_eq!(Options::parse(&options.text()).unwrap(), options);
        // The default building and vegetation distances are not written, and
        // a file without them reads as the defaults.
        let defaults = Options::default().text();
        assert!(!defaults.contains("building"));
        assert!(!defaults.contains("vegetation"));
        assert!(!defaults.contains("vegetation-texture"));
        assert!(!defaults.contains("frame-rate"));
        assert!(options.text().contains("frame-rate true"));
        assert!(
            Options::parse(
                &options
                    .text()
                    .replace("building-detail 2", "building-detail 3")
            )
            .is_err()
        );
        assert_eq!(
            Options::parse(&Options::default().text()).unwrap(),
            Options::default()
        );
        let text = options.text();
        assert!(Options::parse(&text.replace("150", "140")).is_err());
        assert!(Options::parse(&text.replace("8x", "16x")).is_err());
        assert!(Options::parse(&(text.clone() + "terrain-filtering true\n")).is_err());
        assert!(Options::parse(&(text.clone() + "sun-glint true\n")).is_err());
        assert!(Options::parse(&text.replace("tore-graphics 1", "tore-graphics 2")).is_err());
        assert!(Options::parse("tore-graphics 1\n").is_err());
    }
    #[test]
    fn frame_limit_defaults_to_vsync_and_roundtrips() {
        assert_eq!(Options::default().frame_limit, FrameLimit::VSync);
        assert_eq!(Options::original().frame_limit, FrameLimit::VSync);
        // A default file does not name it, and a file written before the
        // choice existed reads as V-sync.
        assert!(!Options::default().text().contains("frame-limit"));
        let old = "tore-graphics 1\nanti-aliasing 4x\nrender-scale 100\nspotting-aid subtle\nterrain-filtering true\nframe-rate true\n";
        let parsed = Options::parse(old).unwrap();
        assert_eq!(parsed.frame_limit, FrameLimit::VSync);
        assert!(parsed.show_frame_rate);
        for limit in FrameLimit::ALL {
            let options = Options {
                frame_limit: limit,
                ..Options::default()
            };
            assert_eq!(Options::parse(&options.text()).unwrap(), options);
            assert_eq!(FrameLimit::parse(limit.key()), Some(limit));
        }
        assert!(
            Options {
                frame_limit: FrameLimit::Fps60,
                ..Options::default()
            }
            .text()
            .contains("frame-limit 60\n")
        );
        assert_eq!(
            FrameLimit::ALL.map(FrameLimit::rate),
            [Some(60), Some(100), Some(200), None]
        );
        assert_eq!(
            FrameLimit::ALL.map(FrameLimit::label),
            ["60", "100", "200", "V-sync"]
        );
        // Uncapped is not a saved choice.
        for bad in ["off", "144", "unlimited", ""] {
            assert!(Options::parse(&format!("{old}frame-limit {bad}\n")).is_err());
        }
    }
    #[test]
    fn shadow_distance_roundtrips_and_follows_only_in_trees_mode() {
        let old = Options::default().text();
        assert!(!old.contains("shadow-distance"));
        assert_eq!(
            Options::parse(&old).unwrap().shadow_distance,
            ShadowDistance::Trees
        );
        for distance in ShadowDistance::ALL {
            let mut options = Options {
                shadow_distance: distance,
                ..Options::default()
            };
            assert_eq!(Options::parse(&options.text()).unwrap(), options);
            for trees in VEGETATION_DISTANCES {
                options.vegetation_distance_nm = trees;
                let expected = if distance == ShadowDistance::Trees {
                    trees
                } else {
                    distance.key().parse().unwrap()
                };
                assert_eq!(
                    distance.nautical_miles(options.vegetation_distance_nm),
                    expected
                );
            }
            let flags = [("--shadow-distance".to_owned(), distance.key().to_owned())];
            let mut cli = Options::default();
            cli.apply_flags(&flags).unwrap();
            assert_eq!(cli.shadow_distance, distance);
        }
        for bad in ["0", "3", "10", "4nm", "-1", "NaN", ""] {
            assert!(Options::parse(&format!("{old}shadow-distance {bad}\n")).is_err());
            assert!(
                Options::default()
                    .apply_flags(&[("--shadow-distance".into(), bad.into())])
                    .is_err()
            );
        }
        assert!(Options::parse(&format!("{old}shadow-distance 2\nshadow-distance 4\n")).is_err());
    }

    #[test]
    fn shadow_quality_defaults_to_medium_and_roundtrips() {
        assert_eq!(Options::default().shadow_quality, ShadowQuality::Medium);
        assert_eq!(Options::original().shadow_quality, ShadowQuality::Medium);
        // A default file does not name it, and an older file reads as Medium.
        assert!(!Options::default().text().contains("shadows"));
        let old = "tore-graphics 1\nanti-aliasing 4x\nrender-scale 100\nspotting-aid subtle\nterrain-filtering true\n";
        assert_eq!(
            Options::parse(old).unwrap().shadow_quality,
            ShadowQuality::Medium
        );
        for quality in ShadowQuality::ALL {
            let options = Options {
                shadow_quality: quality,
                ..Options::default()
            };
            assert_eq!(Options::parse(&options.text()).unwrap(), options);
            assert_eq!(ShadowQuality::parse(quality.key()), Some(quality));
        }
        assert!(
            Options {
                shadow_quality: ShadowQuality::Off,
                ..Options::default()
            }
            .text()
            .contains("shadows off\n")
        );
        assert_eq!(
            ShadowQuality::ALL.map(ShadowQuality::label),
            ["Off", "Low", "Med", "High", "Ultra"]
        );
        for bad in ["none", "max", ""] {
            assert!(Options::parse(&format!("{old}shadows {bad}\n")).is_err());
        }
        let mut options = Options::default();
        let flag = |v: &str| [("--shadows".to_owned(), v.to_owned())];
        options.apply_flags(&flag("ultra")).unwrap();
        assert_eq!(options.shadow_quality, ShadowQuality::Ultra);
        assert!(options.apply_flags(&flag("on")).is_err());
    }
    #[test]
    fn graphics_api_defaults_to_auto_and_files_stay_compatible() {
        assert_eq!(Options::default().graphics_api, GraphicsApi::Auto);
        // A default file does not name it, so an older build still reads it, and
        // a file from an older build reads as Auto.
        let old = "tore-graphics 1\nanti-aliasing 4x\nrender-scale 100\nspotting-aid subtle\nterrain-filtering true\n";
        assert!(!Options::default().text().contains("graphics-api"));
        assert_eq!(Options::parse(old).unwrap().graphics_api, GraphicsApi::Auto);
        for api in GraphicsApi::ALL {
            let options = Options {
                graphics_api: api,
                ..Options::default()
            };
            assert_eq!(Options::parse(&options.text()).unwrap(), options);
            assert_eq!(GraphicsApi::parse(api.key()), Some(api));
        }
        // A value from a newer build costs only this one setting.
        let future = Options::parse(&format!("{old}render-scale-note\n"));
        assert!(future.is_err(), "the file format itself stays strict");
        let future = Options::parse(&format!("{old}graphics-api metal\n")).unwrap();
        assert_eq!(future.graphics_api, GraphicsApi::Auto);
        assert_eq!(future.anti_aliasing, AntiAliasing::X4);
        assert!(Options::parse(&format!("{old}graphics-api dx12\ngraphics-api auto\n")).is_err());
    }
    #[test]
    fn graphics_api_flag_lasts_one_run_and_explains_itself_off_windows() {
        let flag = |v: &str| vec![("--graphics-api".to_owned(), v.to_owned())];
        assert_eq!(GraphicsApi::flag(&[]), Ok(None));
        assert_eq!(
            GraphicsApi::flag(&flag("auto")),
            Ok(Some(GraphicsApi::Auto))
        );
        for windows in [false, true] {
            assert!(GraphicsApi::from_flag_on("opengl", windows).is_err());
            assert_eq!(
                GraphicsApi::from_flag_on("auto", windows),
                Ok(GraphicsApi::Auto)
            );
        }
        assert_eq!(
            GraphicsApi::from_flag_on("dx12", true),
            Ok(GraphicsApi::Dx12)
        );
        assert_eq!(
            GraphicsApi::from_flag_on("vulkan", true),
            Ok(GraphicsApi::Vulkan)
        );
        for api in ["dx12", "vulkan"] {
            let error = GraphicsApi::from_flag_on(api, false).unwrap_err();
            assert!(
                error.contains(api) && error.contains("only on Windows"),
                "{error}"
            );
        }
        // Accepted by the shared flag path, but it changes nothing that is saved.
        let mut options = Options::default();
        options.apply_flags(&flag("auto")).unwrap();
        assert_eq!(options, Options::default());
        assert!(options.apply_flags(&flag("opengl")).is_err());
        // --original-graphics resets the picture, not the saved API choice.
        options.graphics_api = GraphicsApi::Vulkan;
        options
            .apply_flags(&[("--original-graphics".to_owned(), String::new())])
            .unwrap();
        assert_eq!(options.graphics_api, GraphicsApi::Vulkan);
    }
    #[test]
    fn a_missing_or_damaged_file_gives_the_defaults() {
        let dir = std::env::temp_dir().join(format!("tore-graphics-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(Options::load(&dir.join("none.conf")), Options::default());
        let bad = dir.join("bad.conf");
        std::fs::write(&bad, b"\xff\xfe not a config").unwrap();
        assert_eq!(Options::load(&bad), Options::default());
        let good = dir.join("good.conf");
        let options = Options {
            render_scale: 150,
            ..Options::default()
        };
        options.save(&good).unwrap();
        assert_eq!(Options::load(&good), options);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn flags_override_in_order() {
        let flag = |f: &str, v: &str| (f.to_owned(), v.to_owned());
        let mut options = Options::default();
        options
            .apply_flags(&[
                flag("--original-graphics", ""),
                flag("--spotting-aid", "strong"),
                flag("--render-scale", "150"),
            ])
            .unwrap();
        assert_eq!(options.anti_aliasing, AntiAliasing::Off);
        assert_eq!(options.spotting_aid, SpottingAid::Strong);
        assert_eq!(options.render_scale, 150);
        assert!(
            options
                .apply_flags(&[flag("--render-scale", "110")])
                .is_err()
        );
        assert!(
            options
                .apply_flags(&[flag("--terrain-filtering", "yes")])
                .is_err()
        );
    }
}
