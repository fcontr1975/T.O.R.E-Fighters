//! A small, reusable CRT display look: a picture (and an optional overlay
//! of symbology) shown as on a monochrome sensor monitor, with mild
//! pixelation and soft focus, slight chromatic aberration, scanlines, a
//! vignette, fine noise and occasional bursts of static.
//!
//! Two forms share one set of rules:
//!
//! - [`Crt`], a GPU pass (`crt.wgsl`) that composes a source texture and an
//!   overlay texture into a target. The AC-130 sensor console draws its
//!   full-screen picture through it ([`crate::sensor_console`]).
//! - [`apply_cpu`], the same look on a small RGBA buffer in place, for the
//!   instrument pages that are drawn on the CPU (the target camera's
//!   138 x 114 page, say for a second of static when a seeker weapon hits).
//!
//! [`Static`] decides when the occasional bursts come and lets a caller ask
//! for one ([`Static::burst`]). Presentation only: nothing here reads or
//! feeds the simulation, and the bursts run on the frame clock.
//!
//! Every constant in [`SENSOR`] and [`Static`] is an agent decision
//! (nr-fltv, 2026-10-13), tuned on offscreen renders to be noticeable but
//! not in the way; John asked for "nice static occasionally and a bit of
//! chromatic aberration and pixelation to make it look like a CRT display"
//! (opinionated, John, 2026-10-13).
use crate::gpu_setup::NamedSteps as _;

/// How strong each part of the look is. Pixel sizes are for a 1080-row
/// output and scale with the output's height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Red and blue pulled apart this many pixels at the screen edge.
    pub aberration_px: f32,
    /// The source is shown in square blocks this many pixels across.
    pub pixel_px: f32,
    /// Depth of the scanline darkening, 0..1.
    pub scanline: f32,
    /// Darkening toward the corners, 0..1.
    pub vignette: f32,
    /// Fine noise amplitude, 0..1 of full scale.
    pub noise: f32,
    /// Soft focus radius in pixels.
    pub soft_px: f32,
    /// Show the source in grey.
    pub monochrome: bool,
}

/// The AC-130 sensor console's look (agent decision, nr-fltv).
pub const SENSOR: Settings = Settings {
    aberration_px: 1.1,
    pixel_px: 2.,
    scanline: 0.12,
    vignette: 0.42,
    noise: 0.045,
    soft_px: 0.9,
    monochrome: true,
};

/// The replay's HUD camera tape (nr-replayviews, 2026-10-16): a colour
/// picture with a stronger aberration, the tape's own scanline period and no
/// block pixelation (the tape's softness comes from [`Tape`]). Agent
/// decision, tuned on offscreen renders.
pub const TAPE_SCREEN: Settings = Settings {
    aberration_px: 2.4,
    pixel_px: 1.,
    scanline: 0.09,
    vignette: 0.30,
    noise: 0.032,
    soft_px: 0.,
    monochrome: false,
};

/// An analogue videotape recording of the picture (VHS): what a 1980s or
/// 1990s HUD video recorder put on tape. Pixel sizes are for a 1080-row
/// output and scale with the output's height. Opinionated (John,
/// 2026-10-16: "like an old style VHS recording of the HUD ... chromatic
/// aberration, colour graded to an old VHS style recording", then "a nice
/// VHS texture smoothing"); every number is an agent decision, tuned on
/// offscreen renders so the HUD's numbers stay readable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tape {
    /// The tape's limited horizontal resolution: the picture's brightness
    /// is low-passed along each line with a Gaussian of this many pixels.
    pub luma_blur_px: f32,
    /// The colour is low-passed much more (VHS keeps about a tenth of the
    /// brightness bandwidth for colour): this many pixels.
    pub chroma_blur_px: f32,
    /// The colour arrives late along the line, this many pixels to the right.
    pub chroma_delay_px: f32,
    /// Colour saturation kept, 0..1.
    pub saturation: f32,
    /// Black level (lifted blacks) and white level (crushed highlights).
    pub black: f32,
    pub white: f32,
    /// A recorder's edge enhancement: a faint ring beside each edge.
    pub sharpen: f32,
    /// Bright parts bloom over their surroundings.
    pub bloom: f32,
    /// The grade's cast in YIQ (I, Q): negative I and Q tint toward green
    /// and cyan.
    pub cast: [f32; 2],
    /// Each line slips sideways up to this many pixels (time-base error).
    pub line_jitter_px: f32,
    /// The whole frame shakes up to this many pixels.
    pub frame_jitter_px: f32,
    /// The head-switching band at the bottom, a share of the height.
    pub head_switch: f32,
    /// Coloured noise in the chroma.
    pub chroma_noise: f32,
}

/// The HUD camera's tape (nr-replayviews, agent decision).
pub const VHS: Tape = Tape {
    luma_blur_px: 1.7,
    chroma_blur_px: 6.5,
    chroma_delay_px: 2.5,
    saturation: 0.62,
    black: 0.065,
    white: 0.92,
    sharpen: 0.16,
    bloom: 0.55,
    cast: [-0.018, -0.022],
    line_jitter_px: 0.45,
    frame_jitter_px: 0.5,
    head_switch: 0.016,
    chroma_noise: 0.028,
};

/// [`VHS`] without its smoothing: the brightness and colour kept sharp and
/// no edge ring (the before half of the smoothing's before-and-after
/// renders, `TORE_REPLAY_TAPE_SMOOTHING=0`).
pub const VHS_SHARP: Tape = Tape {
    luma_blur_px: 0.05,
    chroma_blur_px: 0.05,
    chroma_delay_px: 0.,
    sharpen: 0.,
    bloom: 0.,
    ..VHS
};

/// What changes on the tape from one field to the next, from the replay
/// tick alone, so a still at a tick always draws the same frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TapeField {
    /// A tracking band rolling through: its strength 0..1 and its height
    /// on the picture, 0 top to 1 bottom.
    pub tracking: f32,
    pub tracking_y: f32,
    /// A dropout (a white streak where the oxide flaked): its row (0..1),
    /// start (0..1 of the width) and length (0..1), or `None`.
    pub dropout: Option<[f32; 3]>,
}

/// The tape plays at 60 fields a second: two simulation ticks a field.
pub const TICKS_PER_FIELD: u64 = 2;
/// A tracking band comes in some of these windows of ticks (7.5 s).
pub const TRACKING_WINDOW_TICKS: u64 = 900;
/// The share of windows with a band, and how long one takes to roll up the
/// picture, ticks (agent decision: occasional and faint).
pub const TRACKING_CHANCE: f32 = 0.3;
pub const TRACKING_TICKS: u64 = 180;
/// A tracking band's strength at the middle of its run (faint: the HUD
/// under it stays readable).
pub const TRACKING_STRENGTH: f32 = 0.5;
/// The chance of a dropout in a field.
pub const DROPOUT_CHANCE: f32 = 0.025;

impl TapeField {
    /// The field shown at simulation tick `tick`.
    pub fn at(tick: u64, seed: u32) -> Self {
        let window = (tick / TRACKING_WINDOW_TICKS) as u32;
        let mut field = Self::default();
        if random(window, 0x7a3e, seed) < TRACKING_CHANCE {
            let span = TRACKING_WINDOW_TICKS - TRACKING_TICKS;
            let start = (random(window, 0x51ed, seed) * span as f32) as u64;
            let into = tick % TRACKING_WINDOW_TICKS;
            if (start..start + TRACKING_TICKS).contains(&into) {
                let t = (into - start) as f32 / TRACKING_TICKS as f32;
                // Rolls up from below the picture to above it, strongest
                // in the middle of its run.
                field.tracking_y = 1.1 - 1.2 * t;
                field.tracking = (std::f32::consts::PI * t).sin() * TRACKING_STRENGTH;
            }
        }
        let number = (tick / TICKS_PER_FIELD) as u32;
        if random(number, 0xd409, seed) < DROPOUT_CHANCE {
            field.dropout = Some([
                random(number, 0xd40a, seed),
                random(number, 0xd40b, seed) * 0.8,
                0.02 + random(number, 0xd40c, seed) * 0.12,
            ]);
        }
        field
    }
}

/// The pass's uniform: eleven vectors of four (`crt.wgsl`'s `Crt`).
const UNIFORM_FLOATS: usize = 44;

/// One frame's worth of the look: the settings, the clock, the static level
/// and how the source is mapped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub settings: Settings,
    /// Seconds on the frame clock; drives the noise and the rolling band.
    pub seconds: f64,
    /// Static, 0 none to 1 full snow.
    pub static_level: f32,
    /// Invert the source (the sensor's black-hot); the overlay is not.
    pub invert: bool,
    /// Source gain and lift, before any inversion: `out = source * gain + lift`.
    pub gain: f32,
    pub lift: f32,
    /// Varies the noise pattern between users of the module.
    pub seed: u32,
    /// The phosphor's colour: the picture (not the overlay) is multiplied by
    /// it (white keeps it; a night vision tube's green tints it).
    pub tint: [f32; 3],
    /// A coloured picture (a colour source, or a tint): the aberration's
    /// fringes are faded toward the unshifted picture rather than toward
    /// grey, so the picture keeps its colours.
    pub colour: bool,
    /// Bright parts of the picture bloom over their surroundings, 0 none: an
    /// image intensifier's halo round lights and fires (GPU pass only).
    pub bloom: f32,
    /// A logarithmic response after the gain (`log(1 + v) / log(1 + gain)`)
    /// instead of clipping: a high-gain picture's lights keep their
    /// falloff (GPU pass only).
    pub shoulder: bool,
    /// Show the picture as a videotape recording ([`Tape`], GPU pass only),
    /// with this field's tracking band and dropout.
    pub tape: Option<(Tape, TapeField)>,
}

impl Frame {
    /// The HUD camera's tape at simulation tick `tick`: [`TAPE_SCREEN`] and
    /// [`VHS`], the noise and the field's events all from the tick, so a
    /// still at a tick is always the same picture.
    pub fn tape(tick: u64, seed: u32) -> Self {
        let mut frame = Self::new(TAPE_SCREEN, tick as f64 / 120., 0.);
        frame.seed = seed;
        frame.colour = true;
        frame.tape = Some((VHS, TapeField::at(tick, seed)));
        frame
    }

    pub fn new(settings: Settings, seconds: f64, static_level: f32) -> Self {
        Self {
            settings,
            seconds,
            static_level,
            invert: false,
            gain: 1.,
            lift: 0.,
            seed: 0x1d2c_3b4a,
            tint: [1.; 3],
            colour: false,
            bloom: 0.,
            shoulder: false,
            tape: None,
        }
    }

    /// The pass's uniform, as `crt.wgsl` reads it.
    fn uniform(&self, size: [u32; 2], overlay: bool) -> [f32; UNIFORM_FLOATS] {
        let s = &self.settings;
        let (tape, field) = self.tape.unwrap_or((VHS, TapeField::default()));
        let on = f32::from(u8::from(self.tape.is_some()));
        let dropout = field.dropout.unwrap_or([-1., 0., 0.]);
        [
            (self.seconds % 3600.) as f32,
            self.static_level,
            s.aberration_px,
            s.pixel_px,
            s.scanline,
            s.vignette,
            s.noise,
            s.soft_px,
            f32::from(u8::from(s.monochrome)),
            f32::from(u8::from(self.invert)),
            self.gain,
            self.lift,
            size[0] as f32,
            size[1] as f32,
            (self.seed & 0xff_ffff) as f32,
            f32::from(u8::from(overlay)),
            self.tint[0],
            self.tint[1],
            self.tint[2],
            f32::from(u8::from(self.colour)),
            self.bloom,
            f32::from(u8::from(self.shoulder)),
            0.,
            0.,
            tape.luma_blur_px,
            tape.chroma_blur_px,
            tape.chroma_delay_px,
            tape.saturation,
            tape.black,
            tape.white,
            tape.sharpen,
            tape.bloom,
            tape.cast[0],
            tape.cast[1],
            tape.line_jitter_px,
            tape.frame_jitter_px,
            tape.head_switch,
            field.tracking,
            field.tracking_y,
            tape.chroma_noise,
            on,
            dropout[0],
            dropout[1],
            dropout[2],
        ]
    }
}

/// When the occasional static comes, and any burst a caller asked for.
///
/// Agent decision (nr-fltv): a burst every 7 to 18 seconds, 0.08 to 0.3
/// seconds long, 35 to 80 percent strong, rising over a twentieth of a
/// second and fading out; the sequence is fixed by the seed, so a still
/// render can ask for a frame inside a burst.
#[derive(Clone, Debug)]
pub struct Static {
    state: u64,
    /// The next occasional burst: start, length (s) and strength.
    next: (f64, f64, f32),
    /// A requested burst at full strength: start and end.
    forced: Option<(f64, f64)>,
    /// Occasional bursts on or off.
    pub occasional: bool,
}

/// The gap between occasional bursts, seconds.
pub const BURST_GAP_S: (f64, f64) = (7., 18.);
/// An occasional burst's length, seconds.
pub const BURST_LENGTH_S: (f64, f64) = (0.08, 0.3);
/// An occasional burst's strength.
pub const BURST_STRENGTH: (f32, f32) = (0.35, 0.8);

impl Static {
    /// A schedule whose first burst comes a gap after `start`.
    pub fn new(seed: u64, start: f64) -> Self {
        let mut s = Self {
            state: seed | 1,
            next: (0., 0., 0.),
            forced: None,
            occasional: true,
        };
        s.next = s.draw(start);
        s
    }

    fn random(&mut self) -> f64 {
        // xorshift64*: a fixed sequence for a seed.
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        (self.state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn draw(&mut self, after: f64) -> (f64, f64, f32) {
        let lerp = |(a, b): (f64, f64), t: f64| a + (b - a) * t;
        let gap = lerp(BURST_GAP_S, self.random());
        let length = lerp(BURST_LENGTH_S, self.random());
        let strength = lerp(
            (f64::from(BURST_STRENGTH.0), f64::from(BURST_STRENGTH.1)),
            self.random(),
        ) as f32;
        (after + gap, length, strength)
    }

    /// Full static from `now` for `seconds` (for example a second when a
    /// seeker weapon hits).
    // Offered to the seeker view's impact static (slice nr-seek).
    #[allow(dead_code)]
    pub fn burst(&mut self, now: f64, seconds: f64) {
        self.forced = Some((now, now + seconds.max(0.)));
    }

    /// Whether a requested burst is still running at `now`.
    pub fn forced(&self, now: f64) -> bool {
        self.forced
            .is_some_and(|(start, end)| (start..end).contains(&now))
    }

    /// The static level at `now`, 0..1. Time only moves forward here: a
    /// frame earlier than the last burst's end draws no new burst.
    pub fn level(&mut self, now: f64) -> f32 {
        let forced = if self.forced(now) { 1. } else { 0. };
        if !self.occasional {
            return forced;
        }
        // Skip bursts that ended unseen (a pause, a long frame).
        while now >= self.next.0 + self.next.1 {
            let end = self.next.0 + self.next.1;
            self.next = self.draw(end);
        }
        let (start, length, strength) = self.next;
        let occasional = if now >= start {
            let t = now - start;
            let rise = (t / 0.05).min(1.);
            let fall = ((length - t) / (length * 0.6)).clamp(0., 1.);
            strength * (rise * fall) as f32
        } else {
            0.
        };
        occasional.max(forced)
    }
}

/// An inverted source shows at this share of full scale, so a cool scene in
/// black hot is not a white page (agent decision; `crt.wgsl` uses it too).
const INVERTED: f32 = 0.85;

/// The integer hash `crt.wgsl` uses.
fn hash(v: u32) -> u32 {
    let mut x = v.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = ((x >> ((x >> 28) + 4)) ^ x).wrapping_mul(277_803_737);
    (x >> 22) ^ x
}
fn random(a: u32, b: u32, c: u32) -> f32 {
    f32::from((hash(a ^ hash(b ^ hash(c))) & 0xffff) as u16) / 65535.
}

/// The look on a small sRGB RGBA buffer, in place (alpha kept): the CPU
/// twin of the GPU pass for instrument pages. No overlay and no soft focus
/// (a page this small is soft already); the rest follows `crt.wgsl` with
/// sizes taken for the buffer's own height as if it filled a 1080-row
/// screen region, clamped to whole pixels.
pub fn apply_cpu(pixels: &mut [u8], width: usize, height: usize, frame: &Frame) {
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return;
    }
    let s = frame.settings;
    let source = pixels.to_vec();
    let level = frame.static_level.clamp(0., 1.);
    let tick = (frame.seconds * 60.) as u32;
    let seed = frame.seed;
    let block = (s.pixel_px * height as f32 / 1080.).round().max(1.) as usize;
    let shift_px = (s.aberration_px * height as f32 / 1080.).max(0.);
    // The block is chosen from the pixel itself, then the aberration's shift
    // is added, so all three colours read the same block (`crt.wgsl`).
    let read = |x: f64, y: f64, shift: (f64, f64), channel: usize| -> f32 {
        let snap = |v: f64, n: usize| ((v.max(0.) as usize).min(n - 1) / block * block) as f64;
        let xi = ((snap(x, width) + shift.0).round() as i64).clamp(0, width as i64 - 1) as usize;
        let yi = ((snap(y, height) + shift.1).round() as i64).clamp(0, height as i64 - 1) as usize;
        let at = (yi * width + xi) * 4;
        let mut v = if s.monochrome {
            (0.2126 * f32::from(source[at])
                + 0.7152 * f32::from(source[at + 1])
                + 0.0722 * f32::from(source[at + 2]))
                / 255.
        } else {
            f32::from(source[at + channel]) / 255.
        };
        v = (v * frame.gain + frame.lift).clamp(0., 1.);
        let v = if frame.invert { (1. - v) * INVERTED } else { v };
        v * frame.tint[channel]
    };
    for y in 0..height {
        let mut dx = 0.;
        if level > 0. {
            let band = (y / 6.max(height / 180)) as u32;
            if random(band, tick + 7, seed) >= 0.55 {
                dx = f64::from((random(band, tick, seed) - 0.5) * 0.04 * level * level)
                    * width as f64;
            }
        }
        let streak = random(y as u32, tick, seed ^ 0x68e3_1da4);
        for x in 0..width {
            let cx = (x as f64 + 0.5) / width as f64 - 0.5;
            let cy = (y as f64 + 0.5) / height as f64 - 0.5;
            let r = cx.hypot(cy);
            let k = f64::from(shift_px) * 2. * (0.35 + r * 1.3);
            let (sx, sy) = (cx * k, cy * k);
            let fx = x as f64 + dx;
            let mut c = [
                read(fx, y as f64, (sx, sy), 0),
                read(fx, y as f64, (0., 0.), 1),
                read(fx, y as f64, (-sx, -sy), 2),
            ];
            let line = 0.5 + 0.5 * (std::f32::consts::TAU * y as f32 / 3.).cos();
            let edge = (cx.hypot(cy * height as f64 / width as f64) * 1.6).min(1.4) as f32;
            let grain = random(x as u32 / 2, y as u32 / 2, tick ^ seed);
            let snow = random(
                x as u32 / 2,
                y as u32 / 2,
                tick.wrapping_mul(31) ^ seed ^ 0x5bd1_e995,
            );
            // Faint fringes, as `crt.wgsl`'s ABERRATION_COLOUR: toward grey,
            // or toward the unshifted picture for a colour source.
            if frame.colour {
                let base = [
                    read(fx, y as f64, (0., 0.), 0),
                    c[1],
                    read(fx, y as f64, (0., 0.), 2),
                ];
                for (v, b) in c.iter_mut().zip(base) {
                    *v = b + (*v - b) * 0.4;
                }
            } else {
                let luma = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                for v in &mut c {
                    *v = luma + (*v - luma) * 0.4;
                }
            }
            for v in &mut c {
                *v *= 1. - s.scanline * line;
                *v *= 1. - s.vignette * edge.powf(2.4);
                *v += (grain - 0.5) * s.noise;
                if level > 0. {
                    *v += (snow - *v) * 0.6 * level;
                    if streak > 1. - 0.04 * level {
                        *v += (1. - *v) * 0.6;
                    }
                    if streak < 0.03 * level {
                        *v *= 0.2;
                    }
                }
            }
            let at = (y * width + x) * 4;
            for (i, v) in c.into_iter().enumerate() {
                pixels[at + i] = (v.clamp(0., 1.) * 255.).round() as u8;
            }
        }
    }
}

/// The GPU pass: `source` and `overlay` composed into a target of the
/// pipeline's format.
pub struct Crt {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    /// A transparent 1 x 1 overlay for frames without one.
    empty: wgpu::Texture,
}

impl Crt {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.named_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("CRT look"),
            source: wgpu::ShaderSource::Wgsl(include_str!("crt.wgsl").into()),
        });
        let pipeline = device.named_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("CRT look"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("crt_vertex"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("crt_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("CRT look"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("CRT look"),
            size: (UNIFORM_FLOATS * 4) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let empty = overlay_texture(device, [1, 1]);
        write_overlay(queue, &empty, [1, 1], &[0; 4]);
        Self {
            pipeline,
            sampler,
            uniform,
            empty,
        }
    }

    /// Draws `source` (and `overlay`, straight alpha, when given) into
    /// `target`, `size` pixels.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        overlay: Option<&wgpu::TextureView>,
        target: &wgpu::TextureView,
        size: [u32; 2],
        frame: &Frame,
    ) {
        let values = frame.uniform(size, overlay.is_some());
        crate::sim_renderer::write_floats(queue, &self.uniform, 0, &values);
        let empty = self.empty.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("CRT look"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(overlay.unwrap_or(&empty)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: self.uniform.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("CRT look"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// An sRGB RGBA texture for an overlay layer of `size`.
pub fn overlay_texture(device: &wgpu::Device, size: [u32; 2]) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("CRT overlay"),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Uploads straight-alpha sRGB RGBA `pixels` of `size` into `texture`.
pub fn write_overlay(queue: &wgpu::Queue, texture: &wgpu::Texture, size: [u32; 2], pixels: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size[0] * 4),
            rows_per_image: Some(size[1]),
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occasional_bursts_come_within_the_gap_and_last_their_length() {
        let mut s = Static::new(42, 0.);
        let mut bursts = Vec::new();
        let mut inside = false;
        let mut start = 0.;
        let mut t = 0.;
        while t < 300. {
            let level = s.level(t);
            assert!((0. ..=1.).contains(&level));
            if level > 0. && !inside {
                inside = true;
                start = t;
            } else if level == 0. && inside {
                inside = false;
                bursts.push((start, t - start));
            }
            t += 1. / 120.;
        }
        assert!(bursts.len() >= 300 / 18 - 1 && bursts.len() <= 300 / 7 + 1);
        for pair in bursts.windows(2) {
            let gap = pair[1].0 - pair[0].0;
            assert!(gap >= BURST_GAP_S.0 - 0.01, "{gap}");
            assert!(gap <= BURST_GAP_S.1 + BURST_LENGTH_S.1 + 0.02, "{gap}");
        }
        for (_, length) in &bursts {
            assert!(*length <= BURST_LENGTH_S.1 + 0.02, "{length}");
        }
    }

    #[test]
    fn a_requested_burst_is_full_static_for_its_length() {
        let mut s = Static::new(7, 0.);
        s.occasional = false;
        assert_eq!(s.level(1.), 0.);
        s.burst(2., 1.);
        assert_eq!(s.level(2.), 1.);
        assert_eq!(s.level(2.99), 1.);
        assert!(s.forced(2.5));
        assert_eq!(s.level(3.), 0.);
    }

    #[test]
    fn the_same_seed_gives_the_same_bursts() {
        let (mut a, mut b) = (Static::new(9, 0.), Static::new(9, 0.));
        for i in 0..5000 {
            let t = f64::from(i) / 60.;
            assert_eq!(a.level(t), b.level(t));
        }
    }

    #[test]
    fn the_cpu_look_greys_inverts_and_keeps_alpha() {
        let (w, h) = (16, 12);
        let mut pixels: Vec<u8> = (0..w * h)
            .flat_map(|_| [200, 40, 40, 77])
            .collect::<Vec<u8>>();
        let mut settings = SENSOR;
        settings.noise = 0.;
        settings.scanline = 0.;
        settings.vignette = 0.;
        settings.aberration_px = 0.;
        let mut frame = Frame::new(settings, 0., 0.);
        apply_cpu(&mut pixels, w, h, &frame);
        let grey = pixels[0];
        assert!(
            pixels
                .chunks_exact(4)
                .all(|p| p[0] == p[1] && p[1] == p[2] && p[0] == grey && p[3] == 77)
        );
        let mut inverted: Vec<u8> = (0..w * h).flat_map(|_| [200, 40, 40, 77]).collect();
        frame.invert = true;
        apply_cpu(&mut inverted, w, h, &frame);
        let expected = (1. - f32::from(grey) / 255.) * INVERTED * 255.;
        assert!(
            (f32::from(inverted[0]) - expected).abs() <= 1.,
            "{}",
            inverted[0]
        );
        // Full static replaces most of the picture with snow.
        let mut snowy: Vec<u8> = vec![0; w * h * 4];
        let frame = Frame::new(settings, 1., 1.);
        apply_cpu(&mut snowy, w, h, &frame);
        let lit = snowy.chunks_exact(4).filter(|p| p[1] > 40).count();
        assert!(lit > w * h / 3, "{lit}");
    }

    /// The sensor console's TV picture keeps its colours; night vision's
    /// phosphor tints a grey picture green (sensor_console.rs).
    #[test]
    fn a_colour_source_keeps_its_colours_and_a_tint_colours_the_grey() {
        let (w, h) = (16, 12);
        let mut settings = SENSOR;
        settings.noise = 0.;
        settings.scanline = 0.;
        settings.vignette = 0.;
        settings.monochrome = false;
        let mut colour: Vec<u8> = (0..w * h).flat_map(|_| [200, 40, 40, 255]).collect();
        let mut frame = Frame::new(settings, 0., 0.);
        frame.colour = true;
        apply_cpu(&mut colour, w, h, &frame);
        assert!(
            colour
                .chunks_exact(4)
                .all(|p| p[0] >= 198 && p[1] <= 42 && p[2] <= 42),
            "{:?}",
            &colour[..4]
        );
        settings.monochrome = true;
        let mut green: Vec<u8> = (0..w * h).flat_map(|_| [200, 40, 40, 255]).collect();
        let mut frame = Frame::new(settings, 0., 0.);
        frame.tint = [0.4, 1., 0.5];
        frame.colour = true;
        apply_cpu(&mut green, w, h, &frame);
        assert!(
            green
                .chunks_exact(4)
                .all(|p| p[1] > p[0] && p[1] > p[2] && p[2] > p[0])
        );
    }

    #[test]
    fn the_hash_matches_the_shader_constants() {
        // Pinned values: the WGSL twin uses the same wrapping arithmetic.
        assert_eq!(hash(0), hash(0));
        assert_ne!(hash(1), hash(2));
        let v = random(3, 4, 5);
        assert!((0. ..=1.).contains(&v));
    }
}

#[cfg(test)]
mod shader_tests {
    use wgpu::naga;

    fn validate(label: &str, source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{label}: {}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("{label}: {e:?}"));
    }

    fn world_source() -> String {
        [
            include_str!("surface_lighting.wgsl"),
            include_str!("terrain.wgsl"),
            include_str!("spotting.wgsl"),
            include_str!("countermeasures.wgsl"),
            include_str!("gun_flash.wgsl"),
            include_str!("airfield_lights.wgsl"),
            include_str!("night_glow.wgsl"),
            include_str!("grass_blades.wgsl"),
        ]
        .join("\n")
    }

    fn pipeline_bindings(source: &str, entries: &[&str]) -> std::collections::BTreeSet<(u32, u32)> {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        let mut used = std::collections::BTreeSet::new();
        for name in entries {
            let index = module
                .entry_points
                .iter()
                .position(|e| e.name == *name)
                .unwrap_or_else(|| panic!("missing entry point {name}"));
            let entry = info.get_entry_point(index);
            for (handle, global) in module.global_variables.iter() {
                // Naga's entry-point analysis includes all transitively called functions.
                if !entry[handle].is_empty()
                    && let Some(binding) = &global.binding
                {
                    used.insert((binding.group, binding.binding));
                }
            }
        }
        used
    }

    #[test]
    fn world_pipeline_bindings_fit_declared_and_bound_layouts() {
        let world = world_source();
        // Layout contracts from EffectRenderer::pipelines, SmokeRenderer::new,
        // SimRenderer's surface/sky/rim layouts, and Countermeasure/Glow pipelines.
        // Auto-layout smoke/effects bind ONLY group 0 at draw time. Checking the
        // whole module's globals would incorrectly require all three groups.
        let contracts: &[(&str, &[&str], u32)] = &[
            (
                "effect standing and flat",
                &["effect_vertex", "effect_fragment"],
                1,
            ),
            ("smoke", &["smoke_vertex", "smoke_fragment"], 1),
            ("flares", &["flare_vertex", "flare_fragment"], 2),
            ("chaff", &["chaff_vertex", "chaff_fragment"], 2),
            (
                "gun and surface flashes",
                &["flash_vertex", "flash_fragment"],
                2,
            ),
            ("night glow", &["glow_vertex", "glow_fragment"], 2),
            ("tracers", &["object_vertex", "tracer_fragment"], 2),
            ("glare", &["glare_vertex", "glare_fragment"], 3),
            ("glare MSAA", &["glare_vertex_ms", "glare_fragment"], 3),
            ("clouds", &["vertex", "cloud_fragment"], 2),
            ("vapor", &["vapor_vertex", "vapor_fragment"], 2),
            (
                "terrain and water",
                &["terrain_vertex", "terrain_fragment"],
                2,
            ),
            ("objects", &["object_vertex", "fragment"], 2),
            (
                "airfield lights",
                &["airfield_light_vertex", "airfield_light_fragment"],
                2,
            ),
            ("sky", &["sky_vertex", "sky_fragment"], 2),
            ("celestial", &["celestial_vertex", "celestial_fragment"], 2),
            (
                "canopy depth and blend",
                &["object_vertex", "canopy_fragment"],
                2,
            ),
            ("airport solid", &["vertex", "airport_solid_fragment"], 2),
            ("airport decal", &["vertex", "airport_fragment"], 2),
            (
                "moving solid",
                &["object_vertex", "airport_solid_fragment"],
                2,
            ),
            ("moving decal", &["object_vertex", "airport_fragment"], 2),
            ("scenery lines", &["vertex", "scenery_line_fragment"], 2),
            ("rim dark", &["rim_vertex", "rim_dark_fragment"], 3),
            ("rim light", &["rim_vertex", "rim_light_fragment"], 3),
            ("rim dark MSAA", &["rim_vertex", "rim_dark_fragment_ms"], 3),
            (
                "rim light MSAA",
                &["rim_vertex", "rim_light_fragment_ms"],
                3,
            ),
            (
                "shadow terrain",
                &["shadow_vertex", "terrain_shadow_fragment"],
                2,
            ),
            (
                "shadow objects",
                &["shadow_object_vertex", "shadow_fragment"],
                2,
            ),
            ("fill solid", &["fill_vertex", "fill_solid_fragment"], 2),
            ("fill fade", &["fill_vertex", "fill_solid_fade_fragment"], 2),
            ("fill decal", &["fill_vertex", "airport_fragment"], 2),
            (
                "fill decal fade",
                &["fill_vertex", "fill_decal_fade_fragment"],
                2,
            ),
            ("fill distant", &["fill_box_vertex", "fill_far_fragment"], 2),
            (
                "fill shadow 0",
                &["fill_shadow_vertex0", "shadow_fragment"],
                2,
            ),
            (
                "fill shadow 1",
                &["fill_shadow_vertex1", "shadow_fragment"],
                2,
            ),
            (
                "fill shadow 2",
                &["fill_shadow_vertex2", "shadow_fragment"],
                2,
            ),
            ("vegetation", &["veg_vertex", "veg_fragment"], 2),
            ("vegetation depth", &["veg_vertex", "veg_depth"], 2),
            ("grass", &["grass_vertex", "grass_fragment"], 2),
            ("opaque blades", &["blade_vertex", "blade_fragment"], 3),
            ("blade list", &["blade_list_vertex", "blade_fragment"], 3),
            // Group 1 is an empty placeholder in the candidate pass.
            ("blade candidates", &["blade_cull"], 4),
            (
                "vegetation shadow 0",
                &["veg_shadow_vertex0", "terrain_shadow_fragment"],
                2,
            ),
            (
                "vegetation shadow 1",
                &["veg_shadow_vertex1", "terrain_shadow_fragment"],
                2,
            ),
            (
                "vegetation shadow 2",
                &["veg_shadow_vertex2", "terrain_shadow_fragment"],
                2,
            ),
        ];
        let module = naga::front::wgsl::parse_str(&world).unwrap();
        for entry in &module.entry_points {
            assert!(
                contracts
                    .iter()
                    .any(|(_, names, _)| names.contains(&entry.name.as_str())),
                "unaudited world entry point {}",
                entry.name
            );
        }
        for (label, entries, groups) in contracts {
            let bindings = pipeline_bindings(&world, entries);
            eprintln!("{label}: shader {bindings:?}; layout groups 0..{groups}");
            for (group, binding) in bindings {
                if label.contains("shadow") && group == 1 {
                    assert_eq!(binding, 0, "shadow caster layout exposes only its uniform");
                }
                assert!(
                    group < *groups,
                    "{label}: unavailable group {group}, binding {binding}"
                );
                assert!(
                    binding
                        < match *label {
                            // Group 1 is the surface lighting: uniform,
                            // shadow maps, comparison and gather samplers.
                            "opaque blades" | "blade list" => [9, 4, 3, 0],
                            "blade candidates" => [9, 0, 3, 2],
                            _ => [9, 4, 2, 0],
                        }[group as usize],
                    "{label}: unavailable binding {binding}"
                );
            }
        }
        let small = [
            include_str!("surface_lighting.wgsl"),
            include_str!("terrain.wgsl"),
        ]
        .join("\n");
        assert!(
            pipeline_bindings(&small, &["smoke_vertex", "smoke_fragment"])
                .iter()
                .all(|(g, _)| *g == 0)
        );
        let reflection = format!(
            "{small}\n@fragment fn reflection_probe(in:VertexOut)->@location(0) vec4<f32>{{return vec4<f32>(water_sun(vec3<f32>(0.05),normalize(vec3<f32>(1.0,-1.0,0.0)),scene.eye.xyz+in.direction,1000.0),1.0);}}"
        );
        let used = pipeline_bindings(&reflection, &["vertex", "reflection_probe"]);
        eprintln!("reflection probe: shader {used:?}; layout groups 0..2");
        assert!(used.iter().all(|(g, _)| *g < 2));

        // Demonstrate this test catches the exact real-GPU regression even
        // though Naga's ordinary module validator accepts the broken shader.
        let broken = world.replace(
            "return scene.view.z;",
            "return 1.0-smoothstep(-0.104528,0.0348995,surface.reserved.y);",
        );
        let used = pipeline_bindings(&broken, &["effect_vertex", "effect_fragment"]);
        assert!(
            used.contains(&(1, 0)),
            "fault injection must expose the missing surface binding"
        );
    }

    #[test]
    fn the_crt_pass_and_the_thermal_world_shaders_validate() {
        validate("crt.wgsl", include_str!("crt.wgsl"));
        validate("vegetation cull", include_str!("vegetation_cull.wgsl"));
        // The world module as `SimRenderer::new` concatenates it.
        let world = [
            include_str!("surface_lighting.wgsl"),
            include_str!("terrain.wgsl"),
            include_str!("spotting.wgsl"),
            include_str!("countermeasures.wgsl"),
            include_str!("gun_flash.wgsl"),
            include_str!("airfield_lights.wgsl"),
            include_str!("night_glow.wgsl"),
            include_str!("grass_blades.wgsl"),
        ]
        .join("\n");
        validate("world", &world);
    }

    /// The type a `BuiltIn SampleMask` variable points at, as an opcode.
    #[cfg(any(windows, target_os = "linux"))]
    fn sample_mask_type_opcode(words: &[u32]) -> u32 {
        const OP_TYPE_POINTER: u32 = 32;
        const OP_VARIABLE: u32 = 59;
        const OP_DECORATE: u32 = 71;
        const DECORATION_BUILT_IN: u32 = 11;
        const BUILT_IN_SAMPLE_MASK: u32 = 20;
        let mut definitions = std::collections::HashMap::new();
        let mut mask = None;
        let mut at = 5;
        while at < words.len() {
            let (count, opcode) = ((words[at] >> 16) as usize, words[at] & 0xffff);
            let operands = &words[at + 1..at + count];
            match opcode {
                OP_DECORATE if operands[1..] == [DECORATION_BUILT_IN, BUILT_IN_SAMPLE_MASK] => {
                    mask = Some(operands[0]);
                }
                // Types define their first operand; a variable its second.
                OP_VARIABLE => {
                    definitions.insert(operands[1], (opcode, operands[0]));
                }
                OP_TYPE_POINTER => {
                    definitions.insert(operands[0], (opcode, operands[2]));
                }
                16..=31 => {
                    definitions.insert(operands[0], (opcode, 0));
                }
                _ => {}
            }
            at += count;
        }
        let variable = definitions[&mask.expect("no SampleMask variable")];
        let pointer = definitions[&variable.1];
        definitions[&pointer.1].0
    }

    /// AMD's Windows driver crashes compiling a `SampleMask` that is not the
    /// array Vulkan requires (issue #8). `crates/vendor/naga` declares one;
    /// stock naga does not.
    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn the_sample_mask_is_an_array_in_spirv() {
        const OP_TYPE_ARRAY: u32 = 28;
        let source = "struct Out { @location(0) color:vec4<f32>, @builtin(sample_mask) mask:u32 }
            @fragment fn written()->Out { return Out(vec4<f32>(1.0),5u); }
            @fragment fn read(@builtin(sample_mask) mask:u32)->@location(0) vec4<f32> {
             return vec4<f32>(f32(mask));
            }";
        let module = naga::front::wgsl::parse_str(source).unwrap();
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        for entry_point in ["written", "read"] {
            let words = naga::back::spv::write_vec(
                &module,
                &info,
                &naga::back::spv::Options::default(),
                Some(&naga::back::spv::PipelineOptions {
                    shader_stage: naga::ShaderStage::Fragment,
                    entry_point: entry_point.to_string(),
                }),
            )
            .unwrap();
            assert_eq!(
                sample_mask_type_opcode(&words),
                OP_TYPE_ARRAY,
                "{entry_point}"
            );
        }
    }
}
