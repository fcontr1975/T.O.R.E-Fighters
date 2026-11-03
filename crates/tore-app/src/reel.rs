//! Promo reel: surface-free, fixed-frame replay capture.
use crate::{
    AppResult,
    assets::Assets,
    camera::Camera,
    replay::viewer::{Options, Viewer},
    scenery::Scenery,
    terrain::Terrain,
};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    process::{Child, Command, Stdio},
};

pub struct Gpu {
    /// Seeker nose eye held after impact, matching the trailer capture view.
    pub seeker_camera: Option<Camera>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub sim: crate::sim_renderer::SimRenderer,
    pub cockpit: crate::cockpit_renderer::CockpitRenderer,
    pub cockpit_aircraft: Option<tore_formats::aircraft::AircraftId>,
    pub gun_sounds: std::collections::BTreeMap<u32, String>,
    pub last_gun_tick: Option<u64>,
    pub gun_models: Vec<(
        tore_formats::aircraft::AircraftId,
        tore_formats::weapons::Weapon,
        [f64; 3],
    )>,
    /// The visible HUD symbols of the last cockpit frame, premultiplied over
    /// transparent black, when the plan asks for a separate symbol layer.
    pub symbols: Option<Vec<u8>>,
    texture: wgpu::Texture,
    symbol_texture: wgpu::Texture,
    buffer: wgpu::Buffer,
    /// The sensor console's world picture, CRT pass and overlay, made on
    /// first use ([`Gpu::console_pixels`]).
    console: Option<(wgpu::Texture, crate::crt::Crt, wgpu::Texture, [u32; 2])>,
}
static WEATHER: std::sync::OnceLock<std::collections::BTreeMap<String, Vec<u8>>> =
    std::sync::OnceLock::new();
pub fn visual_weather_module(world: &Terrain) -> AppResult<tore_formats::weather::Module> {
    let bytes = WEATHER
        .get()
        .and_then(|m| m.get(&world.environment.layer))
        .ok_or("missing visual weather")?;
    Ok(tore_formats::weather::Module::parse(bytes)?)
}
pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
fn target(device: &wgpu::Device, label: &str, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
impl Gpu {
    pub(crate) async fn new(scenery: &Scenery) -> AppResult<Self> {
        let instance = crate::graphics_api::instance();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await?;
        println!("Reel adapter: {:?}", adapter.get_info());
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let sim = crate::sim_renderer::SimRenderer::new(
            &device,
            &queue,
            format,
            scenery,
            crate::graphics::Options::default(),
            4,
        );
        let texture = target(&device, "Reel 1080p offscreen", format);
        let symbol_texture = target(&device, "Reel HUD symbol layer", format);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Reel RGBA readback"),
            size: u64::from(WIDTH * HEIGHT * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let cockpit = crate::cockpit_renderer::CockpitRenderer::new(&device, format);
        Ok(Self {
            seeker_camera: None,
            cockpit,
            cockpit_aircraft: None,
            gun_sounds: Default::default(),
            last_gun_tick: None,
            gun_models: Vec::new(),
            symbols: None,
            device,
            queue,
            sim,
            texture,
            symbol_texture,
            buffer,
            console: None,
        })
    }
    pub fn mirror(&mut self, camera: &Camera, world: &Terrain, scenery: &Scenery) {
        let view = self.cockpit.mirror_target.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.sim.draw_view(
            &self.device,
            &self.queue,
            &mut encoder,
            crate::sim_renderer::ViewKind::Mirror,
            &view,
            crate::mirrors::SIZE,
            camera,
            world,
            scenery,
        );
        self.queue.submit([encoder.finish()]);
    }
    pub fn pixels(
        &mut self,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
    ) -> AppResult<Vec<u8>> {
        let view = self.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.sim.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            [WIDTH, HEIGHT],
            camera,
            world,
            scenery,
        );
        self.cockpit.draw(&mut encoder, &view, [WIDTH, HEIGHT]);
        self.read(encoder, false)
    }
    /// The world drawn thermally and shown through the sensor console's CRT
    /// pass with `overlay` (straight-alpha sRGB RGBA, 1080p) on it: the
    /// offscreen form of the AC-130 sensor console. `camera` should be
    /// thermal.
    pub fn console_pixels(
        &mut self,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
        frame: &crate::crt::Frame,
        overlay: &[u8],
    ) -> AppResult<Vec<u8>> {
        self.console_pixels_sized(camera, world, scenery, frame, overlay, [WIDTH, HEIGHT])
    }
    /// [`Self::console_pixels`] with an overlay of `overlay_size` (the
    /// CRT pass stretches it over the picture): the replay's HUD camera and
    /// Sensor View (nr-replayviews).
    pub fn console_pixels_sized(
        &mut self,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
        frame: &crate::crt::Frame,
        overlay: &[u8],
        overlay_size: [u32; 2],
    ) -> AppResult<Vec<u8>> {
        if self
            .console
            .as_ref()
            .is_some_and(|(.., size)| *size != overlay_size)
        {
            self.console = None;
        }
        if self.console.is_none() {
            let picture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Reel sensor console world"),
                size: wgpu::Extent3d {
                    width: WIDTH,
                    height: HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let crt = crate::crt::Crt::new(
                &self.device,
                &self.queue,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            );
            let layer = crate::crt::overlay_texture(&self.device, overlay_size);
            self.console = Some((picture, crt, layer, overlay_size));
        }
        let (picture, crt, layer, _) = self.console.as_ref().expect("made above");
        crate::crt::write_overlay(&self.queue, layer, overlay_size, overlay);
        let picture = picture.create_view(&Default::default());
        let layer = layer.create_view(&Default::default());
        let view = self.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.sim.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &picture,
            [WIDTH, HEIGHT],
            camera,
            world,
            scenery,
        );
        crt.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &picture,
            Some(&layer),
            &view,
            [WIDTH, HEIGHT],
            frame,
        );
        self.read(encoder, false)
    }
    /// The cockpit pass again, emitting only the HUD symbols the glass shows.
    /// Must follow [`Gpu::pixels`] for the same frame.
    pub fn hud_symbols(&mut self) -> AppResult<Vec<u8>> {
        self.cockpit.symbols_only(&self.queue);
        let view = self.symbol_texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Clear reel HUD symbol layer"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        self.cockpit.draw(&mut encoder, &view, [WIDTH, HEIGHT]);
        self.read(encoder, true)
    }
    fn read(&mut self, mut encoder: wgpu::CommandEncoder, symbols: bool) -> AppResult<Vec<u8>> {
        let texture = if symbols {
            &self.symbol_texture
        } else {
            &self.texture
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(WIDTH * 4),
                    rows_per_image: Some(HEIGHT),
                },
            },
            wgpu::Extent3d {
                width: WIDTH,
                height: HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(60)),
        })?;
        rx.recv()??;
        let pixels = self.buffer.slice(..).get_mapped_range().to_vec();
        self.buffer.unmap();
        Ok(pixels)
    }
}

/// Python expands the checked TOML keyframes into these explicit frame rows.
/// tick, anchor id, eye xyz, target xyz (feet in aircraft heading axes), FOV,
/// visual minutes, view, playback rate and an optional fixed-eye tick.
#[derive(Clone, Debug)]
pub struct Frame {
    pub tick: f64,
    pub anchor: u32,
    pub eye: [f64; 3],
    pub target: [f64; 3],
    pub fov: f64,
    /// Minutes of the day, a fraction for seconds in between.
    pub time_minutes: Option<f64>,
    pub view: u8,
    pub rate: f64,
    /// When set, the eye is placed in the anchor's recorded heading frame at
    /// this tick and stays there, while the target follows the anchor.
    pub eye_tick: Option<u64>,
}
impl Frame {
    fn parse(line: &str) -> AppResult<Self> {
        let n = line
            .split_whitespace()
            .map(str::parse::<f64>)
            .collect::<Result<Vec<_>, _>>()?;
        if !(10..=13).contains(&n.len())
            || n.iter().any(|v| !v.is_finite())
            || !(5. ..=120.).contains(&n[8])
            || n[0] < 0.
            || n[1] < 0.
            || n[1].fract() != 0.
            || n.get(12).is_some_and(|t| t.fract() != 0.)
        {
            return Err(
                "invalid director frame: expected tick, id, eye[3], target[3], FOV 5..120, visual time minutes (-1 keeps recording), view, rate and fixed-eye tick (-1 follows)".into(),
            );
        }
        Ok(Self {
            tick: n[0],
            anchor: n[1] as u32,
            eye: [n[2], n[3], n[4]],
            target: [n[5], n[6], n[7]],
            fov: n[8],
            time_minutes: (n[9] >= 0.).then_some(n[9]),
            view: n.get(10).copied().unwrap_or(0.) as u8,
            rate: n.get(11).copied().unwrap_or(1.),
            eye_tick: n.get(12).filter(|t| **t >= 0.).map(|t| *t as u64),
        })
    }
}

/// Lossless RGBA or RGB frames piped to ffmpeg, with per-frame hashes.
fn capture_encoder(output: &Path, alpha: bool) -> AppResult<Child> {
    Ok(Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            "1920x1080",
            "-framerate",
            "60",
            "-i",
            "pipe:0",
            "-map",
            "0:v",
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-threads",
            "4",
            "-pix_fmt",
            if alpha { "gbrap" } else { "gbrp" },
        ])
        .arg(output)
        .args(["-map", "0:v", "-f", "framehash", "-hash", "sha256"])
        .arg(output.with_extension("sha256"))
        .stdin(Stdio::piped())
        .spawn()?)
}

pub(crate) fn load_assets() -> AppResult<Assets> {
    let assets = Assets::load(&crate::assets::data_directory()?)?;
    let _ = WEATHER.set(
        assets
            .theater_resources
            .iter()
            .filter(|(k, _)| k.ends_with(".LAY"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    );
    Ok(assets)
}

/// `--reel-music DIR`: every imported flight score from its start with seed 1,
/// with a cue sheet of the phrases chosen, and each named playlist recording.
pub fn music() -> AppResult<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let Some((dir, effects)) = args.split_first() else {
        return Err("--reel-music OUTPUT_DIRECTORY [EFFECT_CLIP ...]".into());
    };
    let dir = Path::new(dir);
    std::fs::create_dir_all(dir)?;
    let assets = load_assets()?;
    for (index, name) in tore_formats::music::SCORES.iter().enumerate() {
        // A fresh mixer per score, so every score starts from the same seed.
        let audio = crate::audio::Audio::offline(
            assets.sounds.clone(),
            &assets.music_scores,
            &assets.theater_resources,
        )?;
        let (pcm, cues) = audio.offline_music(index, 48_000 * 75);
        let stem = name.trim_end_matches(".MUS");
        std::fs::write(dir.join(format!("{stem}.f32")), pcm)?;
        let mut sheet = BufWriter::new(File::create(dir.join(format!("{stem}.cues")))?);
        for (sample, phrase) in cues {
            writeln!(sheet, "{sample} {phrase}")?;
        }
        sheet.flush()?;
    }
    let audio = crate::audio::Audio::offline(
        assets.sounds,
        &assets.music_scores,
        &assets.theater_resources,
    )?;
    // Every phrase a score can choose, and the named menu and debrief pieces.
    let mut names: std::collections::BTreeSet<String> = tore_formats::music::MAIN
        .iter()
        .chain(tore_formats::music::BRIEF)
        .chain(tore_formats::music::WIN)
        .chain(tore_formats::music::LOSE)
        .chain(["FINAL6.11K"].iter())
        .map(|n| n.to_string())
        .collect();
    let mut pools = BufWriter::new(File::create(dir.join("scores.txt"))?);
    for (score_name, bytes) in &assets.music_scores {
        if let Ok(score) = tore_formats::music::Score::parse(bytes) {
            let pool: Vec<_> = score.tracks.iter().map(|t| score.filename(*t)).collect();
            writeln!(pools, "{score_name} {}", pool.join(" "))?;
            names.extend(pool);
        }
    }
    pools.flush()?;
    // Named effect recordings for the edit, such as the cockpit switch click.
    names.extend(effects.iter().cloned());
    for name in names {
        if let Some(pcm) = audio.offline_clip(&name) {
            std::fs::write(dir.join(format!("{name}.f32")), pcm)?;
        }
    }
    println!("Reel music written to {}", dir.display());
    Ok(())
}

/// A director frame's anchor that is the world itself: its eye and target
/// are world feet (nr-fx3, for stills of a fixed place).
pub const WORLD_ANCHOR: u32 = u32::MAX - 1;

/// `--replay-stills REPLAY FRAME_PLAN OUT_DIR`: the director's frames of a
/// recording as PNG files, offscreen, no audio written and no ffmpeg (nr-fx3:
/// stills of a recorded city strike). Each plan row is a director frame
/// (`--reel-render`'s format); anchor 4294967294 places the eye and target
/// in world feet. Writes `OUT_DIR/still-NN-tTICK.png`.
pub fn stills() -> AppResult<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let (replay, plan, out, look_at) = match args.as_slice() {
        [replay, plan, out] => (replay, plan, out, None),
        [replay, plan, out, flag, object] if flag == "--look-at" => (
            replay,
            plan,
            out,
            Some(crate::replay::viewer::parse_object(object)?),
        ),
        _ => return Err(
            "--replay-stills REPLAY FRAME_PLAN OUT_DIR [--look-at aircraft:ID|weapon:ID|ground:ID]"
                .into(),
        ),
    };
    let assets = load_assets()?;
    let plan = std::fs::read_to_string(plan)?
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(Frame::parse)
        .collect::<AppResult<Vec<_>>>()?;
    let mut viewer = Viewer::open(
        Path::new(replay),
        &assets.theater_resources,
        &Options {
            look_at,
            ..Options::default()
        },
    )?;
    viewer.finish_tracks();
    let mut gpu = pollster::block_on(Gpu::new(&viewer.scenery))?;
    viewer.director_guns(&mut gpu, &assets.theater_resources)?;
    let audio = crate::audio::Audio::offline(
        assets.sounds,
        &assets.music_scores,
        &assets.theater_resources,
    )?;
    let out = Path::new(out);
    std::fs::create_dir_all(out)?;
    for (index, frame) in plan.iter().enumerate() {
        let previous = index.checked_sub(1).map(|i| &plan[i]);
        let pixels = viewer.director_frame(&mut gpu, frame, previous, &audio)?;
        let city = gpu.sim.city_report();
        if city.burning > 0 {
            println!(
                "Replay city smoke: tick {} burning {} fires {} puffs {} tallest {:.1} ft digest {:016x}",
                frame.tick, city.burning, city.fires, city.puffs, city.tallest_ft, city.digest
            );
        }
        let path = out.join(format!("still-{index:02}-t{}.png", frame.tick));
        std::fs::write(
            &path,
            crate::replay::png::encode_rgba(WIDTH, HEIGHT, &pixels)?,
        )?;
        println!("Replay still: {}", path.display());
    }
    Ok(())
}

pub fn run() -> AppResult<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let hud_layer = args.iter().any(|a| a == "--hud-layer");
    let args: Vec<_> = args.into_iter().filter(|a| a != "--hud-layer").collect();
    if args.len() != 3 {
        return Err("--reel-render REPLAY FRAME_PLAN OUTPUT.mkv [--hud-layer]".into());
    }
    let assets = load_assets()?;
    let plan = std::fs::read_to_string(&args[1])?
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(Frame::parse)
        .collect::<AppResult<Vec<_>>>()?;
    if plan.is_empty() {
        return Err("director plan is empty".into());
    }
    let mut viewer = Viewer::open(
        Path::new(&args[0]),
        &assets.theater_resources,
        &Options::default(),
    )?;
    viewer.finish_tracks();
    if let Some(path) = std::env::var_os("TORE_REEL_GEOMETRY_AUDIT") {
        let mut file = File::create(path)?;
        for face in &viewer.ownship.poses[0].faces {
            writeln!(
                file,
                "{:x} normal={:?} positions={:?} uv={:?}",
                face.address, face.normal, face.positions, face.uv
            )?;
        }
    }
    if plan.iter().any(|f| f.tick > viewer.clock.last() as f64) {
        return Err("director frame past replay end".into());
    }
    let mut gpu = pollster::block_on(Gpu::new(&viewer.scenery))?;
    viewer.director_guns(&mut gpu, &assets.theater_resources)?;
    // AI launch events retain weapon identity but not a cockpit release cue.
    // Resolve its own retail fire sample for the director's watched gun burst.
    for weapon in tore_replay::Recording::open(&args[0])?.weapons() {
        if weapon.class == tore_replay::WeaponClass::Gun
            && let Some(bytes) = assets.theater_resources.get(&weapon.source)
            && let Some(sound) =
                tore_formats::weapons::Weapon::parse(&weapon.source, bytes)?.fire_sound
        {
            gpu.gun_sounds.insert(weapon.id, sound);
        }
    }
    let audio = crate::audio::Audio::offline(
        assets.sounds,
        &assets.music_scores,
        &assets.theater_resources,
    )?;
    audio.set_volumes(crate::audio::Volumes {
        overall: 0.8,
        engine: 0.7,
        weapon_lock: 0.2,
        rwr: 0.2,
        stall: 0.2,
        radio: 1.,
        flight_music: 0.,
        other_music: 0.,
        separation: 50,
        swap: false,
    });
    let output = Path::new(&args[2]);
    if let Some(dir) = output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut radio = BufWriter::new(File::create(output.with_extension("radio"))?);
    let mut pcm = BufWriter::new(File::create(output.with_extension("f32"))?);
    let mut speech = BufWriter::new(File::create(output.with_extension("speech"))?);
    let mut starts = BufWriter::new(File::create(output.with_extension("speech-starts"))?);
    let mut encoder = capture_encoder(output, false)?;
    let mut pipe = encoder.stdin.take().ok_or("no ffmpeg pipe")?;
    let mut layer = if hud_layer {
        gpu.symbols = Some(Vec::new());
        let mut child = capture_encoder(&output.with_extension("hud.mkv"), true)?;
        let pipe = child.stdin.take().ok_or("no ffmpeg pipe")?;
        Some((child, pipe))
    } else {
        None
    };
    let result: AppResult<()> = (|| {
        for (index, frame) in plan.iter().enumerate() {
            let previous = index.checked_sub(1).map(|i| &plan[i]);
            let pixels = viewer.director_frame(&mut gpu, frame, previous, &audio)?;
            pipe.write_all(&pixels)?;
            if let Some((_, layer_pipe)) = &mut layer {
                match gpu.symbols.as_deref() {
                    Some(symbols) if symbols.len() == pixels.len() => {
                        layer_pipe.write_all(symbols)?
                    }
                    _ => layer_pipe.write_all(&vec![0; pixels.len()])?,
                }
            }
            writeln!(radio, "{}", u8::from(audio.offline_radio_active()))?;
            let block = audio.offline_samples(800);
            pcm.write_all(&block.effects)?;
            speech.write_all(&block.speech)?;
            for offset in block.speech_starts {
                writeln!(starts, "{}", index * 800 + offset)?;
            }
            if index % 60 == 0 {
                println!("Reel frame {index}/{}", plan.len());
            }
        }
        Ok(())
    })();
    drop(pipe);
    let status = encoder.wait()?;
    let layer_status = match layer {
        Some((mut child, layer_pipe)) => {
            drop(layer_pipe);
            Some(child.wait()?)
        }
        None => None,
    };
    result?;
    for file in [&mut pcm, &mut speech, &mut radio, &mut starts] {
        file.flush()?;
    }
    if !status.success() || layer_status.is_some_and(|s| !s.success()) {
        return Err("ffmpeg capture failed".into());
    }
    println!(
        "Reel captured {} frames to {}",
        plan.len(),
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn director_rows_accept_an_optional_fixed_eye_tick() {
        let follow = Frame::parse("1180 0 -40 5 80 0 5 -6 42 420 0 1 -1").unwrap();
        assert_eq!(follow.eye_tick, None);
        let fixed = Frame::parse("1180 0 -40 5 80 0 5 -6 42 420 0 1 1300").unwrap();
        assert_eq!(fixed.eye_tick, Some(1300));
        let legacy = Frame::parse("1180 0 -40 5 80 0 5 -6 42 -1").unwrap();
        assert_eq!((legacy.view, legacy.rate, legacy.eye_tick), (0, 1., None));
        assert!(Frame::parse("1180 0 -40 5 80 0 5 -6 42 420 0 1 12.5").is_err());
        assert!(Frame::parse("1180 0 -40 5 80 0 5 -6 200 420 0 1 -1").is_err());
    }
}
