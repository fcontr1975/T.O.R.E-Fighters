//! `--vegetation-preview OUT [JOB ...]`: offscreen pictures of the
//! exploratory vegetation. No window. 2560 by 1440 (or
//! `TORE_VEG_PREVIEW_SIZE=WxH`), four samples, High density (or
//! `TORE_VEG_PREVIEW_DENSITY=low|medium|high|ultra`).
//!
//! Jobs: `ukr` (chase view, woodland, field, hillside, river, meadow,
//! close-ups),
//! `ab` (the chase view with and without an afterburner's light, and the
//! ground detail's energy in each), `egy`, `pgu`, `ira` (desert views at
//! 100, 300, 500, 1,000 and 5,000 ft, a dune field, rocky ground, an oasis
//! when the tile art has one, a wadi and a river), `cost` (one Ukraine camera
//! at 1440p and 4K, Medium, High and Ultra), `lowpass` (a 100 ft, 480 kt, 3 nm
//! pass through the densest stand in ten theaters, at 1440p and 4K, High and
//! Ultra) and `overlay` (top-down classification pictures, Ukraine and Egypt).
//! No job named runs `ukr`, `ab`, `egy` and `pgu`.
//!
//! Every shot's camera is written to `cameras.txt`; `TORE_VEG_PREVIEW_CAMERAS`
//! renders such a file's cameras instead, so another build can take the
//! same pictures; with `TORE_VEG_PREVIEW_STATIC=1` those pictures include
//! the theater's static ground objects, and `=list` prints each one's box.

use crate::{AppResult, camera::Camera, scenery::Scenery, terrain::Terrain};
use std::{io::Write, path::Path, time::Instant};

#[derive(Clone, Copy)]
struct Shot {
    name: &'static str,
    spot: &'static str,
    /// Camera height above the ground, feet.
    height: f64,
    /// Look-down angle, degrees.
    pitch: f64,
    /// How far ahead of the camera the spot is, feet (0: from the pitch).
    ahead: f64,
}

const fn shot(name: &'static str, spot: &'static str, height: f64, pitch: f64, ahead: f64) -> Shot {
    Shot {
        name,
        spot,
        height,
        pitch,
        ahead,
    }
}

/// A camera, as written to and read from `cameras.txt`.
struct Cam {
    name: String,
    code: String,
    condition: usize,
    time: [i32; 2],
    x: f64,
    y: f64,
    z: f64,
    yaw: f64,
    pitch: f64,
}

impl Cam {
    fn parse(line: &str) -> AppResult<Self> {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() != 10 {
            return Err(format!("bad camera line: {line}").into());
        }
        let cam = Self {
            name: f[0].into(),
            code: f[1].into(),
            condition: f[2].parse()?,
            time: [f[3].parse()?, f[4].parse()?],
            x: f[5].parse()?,
            y: f[6].parse()?,
            z: f[7].parse()?,
            yaw: f[8].parse()?,
            pitch: f[9].parse()?,
        };
        if ![cam.x, cam.y, cam.z, cam.yaw, cam.pitch]
            .iter()
            .all(|v| v.is_finite())
            || cam.pitch.abs() > std::f64::consts::FRAC_PI_2 + 1e-6
        {
            return Err(
                "preview camera angles must be finite radians, pitch within +/- pi/2".into(),
            );
        }
        Ok(cam)
    }
    fn camera(&self) -> Camera {
        let mut camera = Camera::new();
        camera.position = [self.x, self.y, self.z];
        camera.yaw = self.yaw as f32;
        camera.pitch = self.pitch as f32;
        camera
    }
}
/// The same decoder as the GPU preview, also used by the CPU frame check.
pub(crate) fn preview_camera(line: &str) -> AppResult<Camera> {
    Ok(Cam::parse(line)?.camera())
}

/// Observer-only fixed-camera hook, also applied to the lead's baseline build.
fn seam_cost_camera(code: &str, height: f64, fallback: Camera) -> AppResult<Camera> {
    let Ok(path) = std::env::var("TORE_SEAM_COST_CAMERAS") else {
        return Ok(fallback);
    };
    let text = std::fs::read_to_string(path)?;
    let name = format!("{code}-seam-cost-{height:.0}ft");
    let line = text
        .lines()
        .find(|l| l.split_whitespace().next() == Some(name.as_str()))
        .ok_or("missing fixed seam cost camera")?;
    let f: Vec<_> = line.split_whitespace().collect();
    if f.len() != 10 {
        return Err("bad fixed seam cost camera".into());
    }
    let mut camera = Camera::new();
    camera.position = [f[5].parse()?, f[6].parse()?, f[7].parse()?];
    camera.yaw = f[8].parse()?;
    camera.pitch = f[9].parse()?;
    Ok(camera)
}

fn size() -> AppResult<[u32; 2]> {
    match std::env::var("TORE_VEG_PREVIEW_SIZE") {
        Ok(v) => {
            let (w, h) = v.split_once('x').ok_or("TORE_VEG_PREVIEW_SIZE is WxH")?;
            Ok([w.parse()?, h.parse()?])
        }
        Err(_) => Ok([2560, 1440]),
    }
}

fn density() -> AppResult<crate::graphics::VegDensity> {
    match std::env::var("TORE_VEG_PREVIEW_DENSITY") {
        Ok(v) => crate::graphics::VegDensity::parse(&v)
            .ok_or_else(|| "bad TORE_VEG_PREVIEW_DENSITY".into()),
        Err(_) => Ok(crate::graphics::VegDensity::High),
    }
}

fn options(density: crate::graphics::VegDensity) -> crate::graphics::Options {
    let mut options = crate::graphics::Options {
        vegetation_density: density,
        ..Default::default()
    };
    if density == crate::graphics::VegDensity::Ultra {
        options.vegetation_distance_nm = 6;
        options.vegetation_detail_nm = 3;
    }
    options
}

pub fn run() -> AppResult<()> {
    let out = std::env::args()
        .nth(2)
        .ok_or("--vegetation-preview needs an output directory")?;
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let mut jobs: Vec<String> = std::env::args().skip(3).collect();
    if jobs.is_empty() {
        jobs = ["ukr", "ab", "egy", "pgu"].map(String::from).to_vec();
    }
    let assets = crate::reel::load_assets()?;
    let resources = &assets.theater_resources;
    if jobs.iter().any(|job| job == "material-cameras") {
        let world = Terrain::for_mission(resources, "KURILE", Some(0), &overrides([12, 0])?)?;
        let mut lines = String::new();
        for (name, x, z, pitch) in [
            ("KURILE-top-500ft", 788480., 624640., -1.5697963267948967),
            ("KURILE-low-500ft", 787692., 623515., -0.3490658503988659),
        ] {
            let y = world.height(x as f32, z as f32) + 500.;
            lines.push_str(&format!(
                "{name} KURILE 0 12 0 {x} {y} {z} 0.61086524 {pitch}\n"
            ));
        }
        std::fs::write(out.join("material-cameras.txt"), lines)?;
        return Ok(());
    }
    if jobs.iter().any(|job| job == "limits-check") {
        return limits_check(resources, out);
    }
    if jobs.iter().any(|job| job == "cache-check") {
        let start = Instant::now();
        let world = crate::scenery::launch_terrain(resources, "UKR", None)?;
        let menu = Scenery::build_for(resources, &world, false)?;
        assert!(
            menu.vegetation.is_none()
                && menu.vegetation_meshes.is_none()
                && menu.canopy.pages_x == 0
        );
        let menu_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let first = Scenery::build(resources, &world)?;
        let cold_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let again = Scenery::build(resources, &world)?;
        let warm_ms = start.elapsed().as_secs_f64() * 1000.;
        if !again.vegetation_cache_hit
            || first.sky_indices != again.sky_indices
            || first.canopy != again.canopy
            || first.grass != again.grass
        {
            return Err("vegetation cache did not restore identical pages and map".into());
        }
        let report = format!(
            "vegetation cache: PASS identical atlas/cards/canopy/grass cache hit; menu without vegetation\nmenu_terrain_ms={menu_ms:.3} first_scenery_ms={cold_ms:.3} cached_scenery_ms={warm_ms:.3}\n"
        );
        print!("{report}");
        std::fs::write(out.join("cache-check.txt"), report)?;
        return Ok(());
    }
    if jobs.iter().any(|job| job == "seam-review") {
        return crate::terrain_seams::audit::review(resources, out);
    }
    if jobs.iter().any(|job| job == "seam-check") {
        return crate::terrain_seams::audit::run(resources, out);
    }
    if jobs.iter().any(|job| job == "seam-cost") {
        return seam_cost(resources, out);
    }
    if jobs.iter().any(|job| job == "grass-cameras") {
        return grass_cameras(resources, out);
    }
    if jobs.iter().any(|job| job == "stream-check") {
        return stream_check(resources, out);
    }
    if jobs.iter().any(|job| job == "pass5-cameras") {
        let mut file = std::fs::File::create(out.join("pass5-cameras.txt"))?;
        for (code, label, kind, agl, pitch) in [
            ("UKR", "UKR-forest-popin", "wood", 300., 8.),
            ("UKR", "UKR-coast", "coast", 450., 14.),
            ("TVIET", "TVIET-coast", "coast", 450., 14.),
            ("EGY", "EGY-acacias", "wadi", 100., 4.),
            ("UKR", "UKR-forest-far-1000ft", "wood", 1000., 5.),
            ("UKR", "UKR-forest-far-3000ft", "wood", 3000., 7.),
            ("UKR", "UKR-top-2000ft", "wood", 2000., 89.5),
            ("EGY", "EGY-top-2000ft", "wadi", 2000., 89.5),
        ] {
            let world = Terrain::for_mission(
                resources,
                code.split('-').next().unwrap_or(code),
                Some(0),
                &overrides([12, 0])?,
            )?;
            let atlas =
                crate::vegetation::Atlas::build(resources, &world)?.ok_or("vegetation disabled")?;
            let spots = crate::vegetation::spots(&atlas, &world);
            let coast = atlas.coast_spot();
            let spot = if kind == "coast" {
                coast.as_ref()
            } else {
                spots.iter().find(|s| s.label == kind)
            }
            .ok_or_else(|| format!("no {code} {kind} camera"))?;
            let cam = look_at(&world, spot, &shot(label, kind, agl, pitch, 0.));
            writeln!(
                file,
                "{label} {code} 0 12 0 {} {} {} {} {}",
                cam.position[0], cam.position[1], cam.position[2], cam.yaw, cam.pitch
            )?;
        }
        return Ok(());
    }
    if jobs.iter().any(|job| job == "cameras") {
        // CPU only: freeze additional camera coordinates before either build
        // renders. These are terrain-class candidates, not surveyed locations.
        let mut file = std::fs::File::create(out.join("extra-cameras.txt"))?;
        for (code, name, kind, height, pitch) in [
            ("UKR", "UKR-field-edge-pass4", "field", 220., 10.),
            ("UKR", "UKR-mountain-slope-pass4", "hill", 700., 10.),
            ("EGY", "EGY-delta-candidate-pass4", "river", 900., 22.),
            ("TVIET", "TVIET-jungle-pass4", "wood", 450., 12.),
        ] {
            let world = Terrain::for_mission(
                resources,
                code.split('-').next().unwrap_or(code),
                Some(0),
                &overrides([12, 0])?,
            )?;
            let atlas =
                crate::vegetation::Atlas::build(resources, &world)?.ok_or("vegetation disabled")?;
            let spots = crate::vegetation::spots(&atlas, &world);
            let spot = spots
                .iter()
                .find(|s| s.label == kind)
                .ok_or_else(|| format!("no {code} {kind} camera candidate"))?;
            let cam = look_at(&world, spot, &shot(name, kind, height, pitch, 0.));
            writeln!(
                file,
                "{name} {code} 0 12 0 {} {} {} {} {}",
                cam.position[0], cam.position[1], cam.position[2], cam.yaw, cam.pitch
            )?;
        }
        return Ok(());
    }
    if let Ok(file) = std::env::var("TORE_VEG_PREVIEW_CAMERAS") {
        return replay_cameras(resources, out, &std::fs::read_to_string(file)?);
    }
    let mut cameras = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out.join("cameras.txt"))?;
    let size = size()?;
    let density = density()?;
    for job in &jobs {
        match job.as_str() {
            "ukr" => scenes(
                resources,
                out,
                &mut cameras,
                size,
                density,
                "UKR",
                &[
                    shot("chase", "open", 300., 6., 0.),
                    shot("woodland", "wood", 450., 9., 0.),
                    shot("field", "field", 450., 9., 0.),
                    shot("hill", "hill", 700., 3., 7_000.),
                    shot("river", "river", 350., 7., 0.),
                    shot("meadow", "meadow", 400., 8., 0.),
                    shot("trees-close", "wood", 45., 2., 400.),
                    shot("grass-close", "field", 10., 4., 120.),
                    shot("chase", "open", 1_000., 10., 0.),
                ],
            )?,
            "egy" | "pgu" | "ira" | "spa" => {
                let code = job.to_ascii_uppercase();
                scenes(
                    resources,
                    out,
                    &mut cameras,
                    size,
                    density,
                    &code,
                    &[
                        shot("open", "open", 100., 4., 0.),
                        shot("open", "open", 300., 6., 0.),
                        shot("open", "open", 500., 8., 0.),
                        shot("open", "open", 1_000., 10., 0.),
                        shot("open", "open", 5_000., 22., 0.),
                        shot("dunes", "dunes", 300., 7., 0.),
                        shot("rocky", "rocky", 250., 5., 2_500.),
                        shot("oasis", "oasis", 300., 7., 0.),
                        shot("wadi", "wadi", 300., 7., 0.),
                        shot("river", "river", 350., 7., 0.),
                        shot("ground-close", "open", 12., 6., 100.),
                    ],
                )?
            }
            "ab" => afterburner(resources, out, size)?,
            "cost" => cost(resources, out)?,
            "lowpass" => lowpass(resources, out)?,
            "overlay" => overlay(resources, out)?,
            other => println!("vegetation preview: unknown job {other}"),
        }
    }
    Ok(())
}

/// Renders `shots` in a theater at noon: the retail look (`-off`) and the
/// vegetation (`-after`), and prints the frame cost of each.
fn scenes(
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    out: &Path,
    cameras: &mut std::fs::File,
    size: [u32; 2],
    density: crate::graphics::VegDensity,
    code: &str,
    shots: &[Shot],
) -> AppResult<()> {
    println!("vegetation preview: building {code}");
    let world = Terrain::for_mission(resources, code, Some(0), &overrides([12, 0])?)?;
    let scenery = Scenery::build(resources, &world)?;
    let atlas = scenery
        .vegetation
        .as_ref()
        .ok_or("vegetation atlas missing (TORE_VEGETATION=0)")?;
    println!("  tone {:?}", atlas.tone);
    if let Some(lib) = &scenery.vegetation_meshes {
        for line in lib.report() {
            println!("  mesh {line}");
        }
    }
    let found = crate::vegetation::spots(atlas, &world);
    let mut gpu = pollster::block_on(View::new(&scenery, size, options(density), false))?;
    for spot in &found {
        println!(
            "  spot {} at {:.0}, {:.0} heading {:.0}",
            spot.label, spot.x, spot.z, spot.heading
        );
    }
    for shot in shots {
        let Some(spot) = found.iter().find(|s| s.label == shot.spot) else {
            println!("  skip {} (no {} spot)", shot.name, shot.spot);
            continue;
        };
        let camera = look_at(&world, spot, shot);
        let stem = format!("{code}-{}-{}ft", shot.name, shot.height as u32);
        writeln!(
            cameras,
            "{stem} {code} 0 12 0 {} {} {} {} {}",
            camera.position[0], camera.position[1], camera.position[2], camera.yaw, camera.pitch
        )?;
        gpu.set_shown(false);
        let off_ms = gpu.time(&camera, &world, &scenery)?;
        let before = gpu.pixels(&camera, &world, &scenery)?;
        gpu.set_shown(true);
        let on_ms = gpu.time(&camera, &world, &scenery)?;
        let after = gpu.pixels(&camera, &world, &scenery)?;
        let (placed, tufts) = gpu
            .sim
            .vegetation
            .as_ref()
            .map(|v| (v.placed(), v.tufts_drawn()))
            .unwrap_or_default();
        write_png(out, &format!("{stem}-off.png"), size, &before)?;
        write_png(out, &format!("{stem}-after.png"), size, &after)?;
        println!(
            "  {stem}: off {off_ms:.2} ms, on {on_ms:.2} ms, objects {placed}, grass tufts {tufts}"
        );
    }
    Ok(())
}

/// The chase view at 300 and 40 ft with and without an afterburner's light just
/// ahead of the camera (where the jet is). The ground detail must be the
/// same: the light only adds light. Prints the detail energy (mean
/// absolute difference from a 3 by 3 blur, ground half of the picture) of
/// each and their ratio, and writes both pictures.
fn afterburner(
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    out: &Path,
    size: [u32; 2],
) -> AppResult<()> {
    println!("vegetation preview: afterburner check");
    let world = Terrain::for_mission(resources, "UKR", Some(0), &overrides([12, 0])?)?;
    let scenery = Scenery::build(resources, &world)?;
    let atlas = scenery
        .vegetation
        .as_ref()
        .ok_or("vegetation atlas missing")?;
    let found = crate::vegetation::spots(atlas, &world);
    let spot = found
        .iter()
        .find(|s| s.label == "open")
        .ok_or("no open spot")?;
    let mut gpu = pollster::block_on(View::new(&scenery, size, options(density()?), false))?;
    gpu.set_shown(true);
    let devices = tore_sim::combat::countermeasures::Devices::default();
    let guns = crate::gun_flash::Drawn::default();
    for height in [300., 40.] {
        let camera = look_at(&world, spot, &shot("chase", "open", height, 6., 0.));
        gpu.sim.emitters(&gpu.queue, &devices, &[], &guns);
        gpu.time(&camera, &world, &scenery)?;
        let plain = gpu.pixels(&camera, &world, &scenery)?;
        let (sy, cy) = camera.yaw.sin_cos();
        let nozzle = [
            camera.position[0] + f64::from(sy) * 70.,
            camera.position[1] - 12.,
            camera.position[2] + f64::from(cy) * 70.,
        ];
        let burner = [crate::countermeasure_renderer::Afterburner {
            position: nozzle,
            share: 1.,
        }];
        gpu.sim.emitters(&gpu.queue, &devices, &burner, &guns);
        gpu.time(&camera, &world, &scenery)?;
        let lit = gpu.pixels(&camera, &world, &scenery)?;
        let stem = format!("UKR-chase-{}ft", height as u32);
        write_png(out, &format!("{stem}-no-afterburner.png"), size, &plain)?;
        write_png(out, &format!("{stem}-afterburner.png"), size, &lit)?;
        let a = detail_energy(&plain, size);
        let b = detail_energy(&lit, size);
        println!(
            "  {stem}: detail energy without {a:.3}, with afterburner {b:.3}, ratio {:.3}",
            b / a.max(1e-6)
        );
    }
    Ok(())
}

/// Mean absolute difference between each pixel's luma and its 3 by 3 mean,
/// over the lower half of the picture (the ground in a chase view).
pub fn detail_energy(rgba: &[u8], size: [u32; 2]) -> f64 {
    let (w, h) = (size[0] as usize, size[1] as usize);
    let luma = |x: usize, y: usize| {
        let p = &rgba[(y * w + x) * 4..];
        0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
    };
    let mut sum = 0.;
    let mut n = 0usize;
    for y in (h / 2).max(1)..h - 1 {
        for x in 1..w - 1 {
            let mut mean = 0.;
            for dy in 0..3 {
                for dx in 0..3 {
                    mean += luma(x + dx - 1, y + dy - 1);
                }
            }
            sum += (luma(x, y) - mean / 9.).abs();
            n += 1;
        }
    }
    sum / n.max(1) as f64
}

/// Same scene, population and view. Only the terrain/grass colour correction
/// switches, with alternating measurement order and mandatory GPU timestamps.
fn seam_cost(resources: &std::collections::BTreeMap<String, Vec<u8>>, out: &Path) -> AppResult<()> {
    let mut report = std::fs::File::create(out.join("seam-cost.csv"))?;
    writeln!(report, "theater,altitude_ft,repeat,off_ms,on_ms,seam_ms")?;
    for code in ["UKR", "FRA"] {
        let world = Terrain::for_mission(resources, code, Some(0), &overrides([12, 0])?)?;
        let mut scenery = Scenery::build(resources, &world)?;
        let spot = crate::terrain_seams::audit::spots(
            &scenery.vegetation.as_ref().ok_or("no vegetation")?.seams,
            crate::terrain_seams::BLEND,
        )
        .into_iter()
        .next()
        .ok_or("no blend seam")?;
        let selected = scenery.seams;
        for height in [100., 500., 2000., 10000.] {
            let camera = seam_cost_camera(
                code,
                height,
                crate::terrain_seams::audit::camera(&world, &spot, height),
            )?;
            scenery.resolve_palette(&world, camera.position[1]);
            scenery.set_origin(camera.position);
            let mut gpu = pollster::block_on(View::new(
                &scenery,
                [3840, 2160],
                options(crate::graphics::VegDensity::High),
                false,
            ))?;
            gpu.set_shown(true);
            gpu.time(&camera, &world, &scenery)?;
            for repeat in 0..8 {
                let mut times = [0.; 2];
                for state in if repeat % 2 == 0 { [0, 1] } else { [1, 0] } {
                    scenery.seams = if state == 0 { [0.; 3] } else { selected };
                    times[state] = gpu
                        .gpu_time(&camera, &world, &scenery)?
                        .ok_or("GPU timestamps required for seam cost")?;
                }
                writeln!(
                    report,
                    "{code},{height},{repeat},{:.6},{:.6},{:.6}",
                    times[0],
                    times[1],
                    times[1] - times[0]
                )?;
            }
            scenery.seams = selected;
        }
    }
    println!("terrain seams: GPU cost recorded at 4K High, identical scene with correction off/on");
    Ok(())
}

/// Frame cost of a low pass over Ukraine at 1440p and 4K, Medium, High and Ultra:
/// everything off, then on.
fn cost(resources: &std::collections::BTreeMap<String, Vec<u8>>, out: &Path) -> AppResult<()> {
    println!("vegetation preview: frame cost");
    let world = Terrain::for_mission(resources, "UKR", Some(0), &overrides([12, 0])?)?;
    let scenery = Scenery::build(resources, &world)?;
    let atlas = scenery
        .vegetation
        .as_ref()
        .ok_or("vegetation atlas missing")?;
    let found = crate::vegetation::spots(atlas, &world);
    let mut report = String::new();
    for spot_name in ["open", "wood"] {
        let spot = found
            .iter()
            .find(|s| s.label == spot_name)
            .ok_or("no spot")?;
        let camera = look_at(&world, spot, &shot("pass", spot_name, 300., 6., 0.));
        for size in [[2560u32, 1440u32], [3840, 2160]] {
            for density in [
                crate::graphics::VegDensity::Medium,
                crate::graphics::VegDensity::High,
                crate::graphics::VegDensity::Ultra,
            ] {
                let mut gpu =
                    pollster::block_on(View::new(&scenery, size, options(density), false))?;
                gpu.set_shown(false);
                let off = gpu.time(&camera, &world, &scenery)?;
                let gpu_off = gpu.gpu_time(&camera, &world, &scenery)?;
                gpu.set_shown(true);
                let on = gpu.time(&camera, &world, &scenery)?;
                let gpu_on = gpu.gpu_time(&camera, &world, &scenery)?;
                let eye = [camera.position[0], camera.position[2]];
                let (placed, tufts, reach, squeeze) = gpu
                    .sim
                    .vegetation
                    .as_ref()
                    .map(|v| (v.placed(), v.tufts_drawn(), v.kept_reach(eye), v.squeeze()))
                    .unwrap_or_default();
                let line = format!(
                    "{spot_name} {}x{} {}: off {off:.2} ms, on {on:.2} ms, cost {:.2} ms, objects {placed} out to {:.1} nm, full density x{squeeze:.2}, grass tufts {tufts}\n",
                    size[0],
                    size[1],
                    density.label(),
                    on - off,
                    reach / crate::vegetation::NM as f32
                );
                print!("  {line}");
                report += &line;
                if let (Some(off), Some(on)) = (gpu_off, gpu_on) {
                    let line = format!(
                        "GPU {spot_name} {}x{} {}: off {off:.3} ms, on {on:.3} ms, cost {:.3} ms\n",
                        size[0],
                        size[1],
                        density.label(),
                        on - off
                    );
                    print!("  {line}");
                    report += &line;
                }
            }
        }
    }
    std::fs::write(out.join("frame-cost.txt"), report)?;
    Ok(())
}

/// Theaters the low pass flies, in the order John asked for on 2026-10-17.
const LOWPASS_THEATERS: &[(&str, &str)] = &[
    ("UKR", "temperate"),
    ("UKR-MEADOW", "temperate"),
    ("FRA", "temperate"),
    ("NSK", "temperate"),
    ("EGY", "desert"),
    ("PGU", "desert"),
    ("IRA", "desert"),
    ("TVIET", "tropical"),
    ("APA", "tropical"),
    ("WTA", "tropical"),
    ("KURILE", "arctic"),
];

/// One timed frame of a low pass. GPU times are timestamp queries.
#[derive(Clone, Copy)]
struct Sample {
    frame: usize,
    gpu_off: f64,
    gpu_on: f64,
    gpu_no_grass: f64,
    blades: u64,
    placed: u32,
    tufts: u64,
    object_chunks: u64,
    grass_chunks: u64,
    build_ms: f64,
    submit_ms: f64,
    vegetation_submit_ms: f64,
    pending: bool,
    stream: crate::vegetation_renderer::CacheFrame,
    popin: crate::vegetation_renderer::popin::Counts,
}

impl Sample {
    fn added(self) -> f64 {
        self.gpu_on - self.gpu_off
    }

    fn grass_cost(self) -> f64 {
        self.gpu_on - self.gpu_no_grass
    }

    fn builds(self) -> u64 {
        self.object_chunks + self.grass_chunks
    }
}

struct RunStats {
    samples: usize,
    added_mean: f64,
    added_p95: f64,
    added_max: f64,
    on_mean: f64,
    on_max: f64,
    placed_mean: f64,
    placed_max: u32,
    tufts_mean: f64,
    tufts_max: u64,
    builds_mean: f64,
    builds_max: u64,
    object_builds: u64,
    grass_builds: u64,
    rebuild_max: f64,
    rebuild_frame: usize,
    submit_max: f64,
    vegetation_submit: [f64; 3],
}

/// A 100 ft, 480 kt, 3 nm pass through each theater's densest vegetation.
/// Graphics distances stay at their defaults. Only density changes.
/// Vegetation off suppresses objects and grass and leaves ground detail on.
/// The patch cache uses the in-flight budget (a few chunks a frame), not the
/// preview's settle-everything path. Warmup is the fill at the start pose.
fn lowpass(resources: &std::collections::BTreeMap<String, Vec<u8>>, out: &Path) -> AppResult<()> {
    let theaters = lowpass_theaters()?;
    let sizes = lowpass_sizes()?;
    let densities = lowpass_densities()?;
    let frames = lowpass_frames()?;
    let stride = lowpass_stride()?;
    let indices = lowpass_indices(frames, stride);
    println!(
        "vegetation preview: low pass, {frames} frames, stride {stride}, {} samples, {} theaters",
        indices.len(),
        theaters.len()
    );
    let mut report = std::io::BufWriter::new(std::fs::File::create(out.join("frame-cost.txt"))?);
    let mut cameras = std::fs::File::create(out.join("cameras.txt"))?;
    writeln!(
        report,
        "100 ft AGL, 480 kt, 3 nm, {frames} frames over 22.5 s (stride {stride}, {} samples).\n\
         Nose-down 4 degrees, level roll. Ground detail stays at its default in both passes.\n\
         Off suppresses vegetation objects, canopy and grass. Density follows the requested matrix.\n\
         Draw distance and detail stay at the Graphics defaults (4 nm and 2 nm).\n\
         GPU times are timestamp queries around the frame. Rebuild cost is CPU time\n\
         spent building object chunks and grass chunks on the worker. Each worker job\n\
         builds up to 3 object chunks and 2 grass chunks. Normal frames upload at most\n\
         256 KiB of instances. Vegetation CPU submit measures preparation, uploads and\n\
         cull encoding; whole-frame submit remains separate. Warmup waits for a complete\n\
         selection and drained uploads, and is excluded from pass statistics.",
        indices.len()
    )?;
    let mut rows = Vec::new();
    let mut notes = Vec::new();
    let mut images = Vec::new();
    for (code, climate) in theaters {
        println!("vegetation preview: low pass {code}");
        let world = Terrain::for_mission(
            resources,
            code.split('-').next().unwrap_or(code),
            Some(0),
            &overrides([12, 0])?,
        )?;
        let scenery = Scenery::build(resources, &world)?;
        let atlas = scenery
            .vegetation
            .as_ref()
            .ok_or("vegetation atlas missing (TORE_VEGETATION=0)")?;
        println!("  choosing the densest stand");
        let started = Instant::now();
        let (spot, candidates) = dense_spot(atlas, &world, code)?;
        println!(
            "  {code} {} at {:.0}, {:.0} heading {:.0}: {} instances within 1 nm ({:.1} s)",
            spot.label,
            spot.x,
            spot.z,
            spot.heading,
            spot.instances,
            started.elapsed().as_secs_f64()
        );
        writeln!(
            report,
            "\n{code} ({climate}) spot {} at {:.1}, {:.1} heading {:.1}: {} instances within 1 nm",
            spot.label, spot.x, spot.z, spot.heading, spot.instances
        )?;
        for candidate in &candidates {
            writeln!(
                report,
                "  candidate {} at {:.1}, {:.1} heading {:.1}: {} instances",
                candidate.label, candidate.x, candidate.z, candidate.heading, candidate.instances
            )?;
        }
        let flight = lowpass_flight(&world, &spot, frames, &indices);
        let outside = flight
            .iter()
            .filter(|(_, camera)| lowpass_outside(&world, camera))
            .count();
        if outside > 0 {
            writeln!(
                report,
                "  {outside} of {} samples are outside the map; height is clamped to the edge",
                flight.len()
            )?;
        }
        let start = lowpass_camera(&world, &spot, -1.5 * crate::vegetation::NM);
        let mut gpu = pollster::block_on(View::new(
            &scenery,
            sizes[0],
            lowpass_options(densities[0]),
            true,
        ))?;
        for &size in &sizes {
            for density in &densities {
                println!("  {code} {}x{} {}", size[0], size[1], density.label());
                gpu.begin_run(size, *density)?;
                let trace = out.join(format!(
                    "{code}-{}x{}-{}-warmup.csv",
                    size[0],
                    size[1],
                    density.label()
                ));
                let warm = gpu.warmup(&start, &world, &scenery, &trace)?;
                println!("    warmup {} frames", warm.len());
                let refs: Vec<(usize, &Camera)> = flight
                    .iter()
                    .map(|(frame, camera)| (*frame, camera))
                    .collect();
                let timed = gpu.measure(&refs, &world, &scenery)?;
                let mut grass: Vec<_> = timed.iter().map(|s| s.grass_cost()).collect();
                let line = format!(
                    "{code} {}x{} {} grass={} on-minus-no-grass mean={:.6} p95={:.6} max={:.6} ms; blade candidates={} vertices={} grass chunk builds={}\n",
                    size[0],
                    size[1],
                    density.label(),
                    std::env::var("TORE_GRASS").unwrap_or_else(|_| "blades".into()),
                    mean(&grass),
                    percentile(&mut grass, 0.95),
                    maximum(&grass),
                    timed.iter().map(|s| s.blades).max().unwrap_or(0),
                    timed
                        .iter()
                        .map(|s| s.blades * 5 + s.tufts * 6)
                        .max()
                        .unwrap_or(0),
                    timed.iter().map(|s| s.grass_chunks).sum::<u64>()
                );
                print!("{line}");
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(out.join("grass-cost.txt"))?
                    .write_all(line.as_bytes())?;
                let mut csv = String::from(
                    "frame,gpu_off_ms,gpu_on_ms,vegetation_submit_ms,frame_submit_ms,pending,near_instances,appeared_inside_detail,near_fading,estimated_fps,gpu_no_grass_ms,grass_only_ms,blade_candidates,grass_builds\n",
                );
                for sample in &timed {
                    csv += &format!(
                        "{},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{:.3},{:.6},{:.6},{},{}\n",
                        sample.frame,
                        sample.gpu_off,
                        sample.gpu_on,
                        sample.vegetation_submit_ms,
                        sample.submit_ms,
                        sample.pending,
                        sample.popin.near,
                        sample.popin.appeared,
                        sample.popin.faded,
                        1000. / sample.gpu_on.max(sample.submit_ms).max(0.001),
                        sample.gpu_no_grass,
                        sample.grass_cost(),
                        sample.blades,
                        sample.grass_chunks
                    );
                }
                std::fs::write(
                    out.join(format!(
                        "{code}-{}x{}-{}-frames.csv",
                        size[0],
                        size[1],
                        density.label()
                    )),
                    csv,
                )?;
                let stats = run_stats(&timed);
                let warm_stats = run_stats(&warm);
                let detail = format_run(
                    code,
                    size,
                    density.label(),
                    &stats,
                    &warm,
                    &warm_stats,
                    &timed,
                );
                print!("{detail}");
                write!(report, "{detail}")?;
                report.flush()?;
                rows.push(summary_row(
                    code,
                    climate,
                    &spot,
                    size,
                    density.label(),
                    &stats,
                    warm.len(),
                ));
                if stats.added_p95 > 3. || stats.added_max > 10. {
                    notes.push(format_note(code, size, density.label(), &stats, &timed));
                }
                write_summary(out, &rows, &notes)?;
                if size == [3840, 2160] && *density == crate::graphics::VegDensity::High {
                    gpu.set_vegetation(true);
                    for (name, along) in [
                        ("start", -1.5 * crate::vegetation::NM),
                        ("dense", 0.),
                        ("end", 1.5 * crate::vegetation::NM),
                    ] {
                        let camera = lowpass_camera(&world, &spot, along);
                        writeln!(
                            cameras,
                            "{code}-3840x2160-high-{name} {code} 0 12 0 {} {} {} {} {}",
                            camera.position[0],
                            camera.position[1],
                            camera.position[2],
                            camera.yaw,
                            camera.pitch
                        )?;
                        let pixels = gpu.pixels(&camera, &world, &scenery)?;
                        let file = format!("{code}-3840x2160-high-{name}.png");
                        write_png(out, &file, size, &pixels)?;
                        images.push((file, code.to_string(), name.to_string(), spot.instances));
                        println!("    wrote {code}-3840x2160-high-{name}.png");
                    }
                    write_lowpass_readme(out, &images)?;
                }
            }
        }
    }
    write_summary(out, &rows, &notes)?;
    write_lowpass_readme(out, &images)?;
    println!("vegetation preview: low pass wrote frame-cost.txt and SUMMARY.md");
    Ok(())
}

fn stream_check(
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    out: &Path,
) -> AppResult<()> {
    use crate::vegetation_renderer::stream::{self, Request};
    let mut summary = std::fs::File::create(out.join("stream-check.txt"))?;
    for (code, _) in lowpass_theaters()? {
        let world = Terrain::for_mission(
            resources,
            code.split('-').next().unwrap_or(code),
            Some(0),
            &overrides([12, 0])?,
        )?;
        let scenery = Scenery::build(resources, &world)?;
        let atlas = scenery
            .vegetation
            .as_ref()
            .ok_or("vegetation atlas missing")?;
        let library = scenery
            .vegetation_meshes
            .as_ref()
            .ok_or("vegetation library missing")?;
        let (spot, _) = dense_spot(atlas, &world, code)?;
        let camera = lowpass_camera(&world, &spot, -1.5 * crate::vegetation::NM);
        for density in lowpass_densities()? {
            let options = lowpass_options(density);
            let full = options.vegetation_detail_nm as f32 * crate::vegetation::NM as f32;
            let request = Request {
                epoch: 0,
                ahead: [0.; 2],
                eye: [camera.position[0], camera.position[2]],
                full,
                draw: options.vegetation_distance_nm as f32 * crate::vegetation::NM as f32,
                density: density.scale(),
                capacity: density.object_budget(),
                grass_radius: if std::env::var("TORE_GRASS").as_deref() == Ok("tufts") {
                    density.grass().0.min(full * 1.25)
                } else {
                    0.
                },
                settle: false,
            };
            let mut trace = std::io::BufWriter::new(std::fs::File::create(
                out.join(format!("{code}-{}-stream.csv", density.label())),
            )?);
            let stats = stream::probe(atlas.clone(), library.clone(), &world, request, &mut trace)?;
            let line = format!(
                "{code} {}: converged={} frames, builds={}, packed={}, uploaded={} bytes, selected={}, idle=120 frames",
                density.label(),
                stats.frames,
                stats.builds,
                stats.packed,
                stats.uploaded,
                stats.selected
            );
            println!("{line}");
            writeln!(summary, "{line}")?;
        }
    }
    Ok(())
}

/// CPU-only camera fixture. Labels describe classified land, not surveyed geography.
fn grass_cameras(
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    out: &Path,
) -> AppResult<()> {
    let mut file = std::fs::File::create(out.join("grass-cameras.txt"))?;
    let mut low = std::fs::File::create(out.join("grass-lowpass-cameras.txt"))?;
    let mut notes = std::fs::File::create(out.join("grass-camera-sites.txt"))?;
    for code in ["UKR", "TVIET", "EGY"] {
        let world = Terrain::for_mission(resources, code, Some(0), &overrides([12, 0])?)?;
        let atlas = std::sync::Arc::new(
            crate::vegetation::Atlas::build(resources, &world)?.ok_or("vegetation disabled")?,
        );
        let spots = crate::vegetation::spots(&atlas, &world);
        if code == "UKR" {
            let wood = spots
                .iter()
                .find(|s| s.label == "wood")
                .ok_or("no pine woodland candidate")?;
            let mut placement = crate::vegetation::Placement::new(std::sync::Arc::clone(&atlas));
            // Match the capture's actual fixed geographic quotas. A pine from
            // an unthinned candidate pool might not exist at the chosen density.
            let settings = options(density()?);
            placement.set_density(settings.vegetation_density.scale());
            placement.set_budget(settings.vegetation_density.object_budget());
            placement.update(
                &world,
                [wood.x, wood.z],
                settings.vegetation_detail_nm as f32 * crate::vegetation::NM as f32,
                settings.vegetation_distance_nm as f32 * crate::vegetation::NM as f32,
                None,
            );
            let pine = placement
                .object_chunks([wood.x, wood.z])
                .into_iter()
                .flat_map(|(_, objects)| objects)
                .filter(|p| p.species == crate::vegetation::Species::Conifer)
                .min_by(|a, b| {
                    let distance = |p: &crate::vegetation::Inst| {
                        (f64::from(p.pos[0]) - wood.x).hypot(f64::from(p.pos[2]) - wood.z)
                    };
                    distance(a).total_cmp(&distance(b))
                })
                .ok_or("no retained pine at woodland camera")?;
            writeln!(
                notes,
                "UKR pine actual placement: {:?}, height={}, variant={}",
                pine.pos, pine.height, pine.variant
            )?;
            for angle in [8f64, 25., 45., 70.] {
                let range = 160.;
                let x = f64::from(pine.pos[0]);
                let z = f64::from(pine.pos[2]) - range;
                let y = f64::from(pine.pos[1])
                    + f64::from(pine.height) * 0.45
                    + range * angle.to_radians().tan();
                writeln!(
                    file,
                    "UKR-pine-{}deg UKR 0 12 0 {x} {y} {z} 0 {}",
                    angle as u32,
                    -angle.to_radians()
                )?;
            }
        }
        let kinds: &[(&str, &str, &[f64])] = match code {
            "UKR" => &[
                ("meadow", "meadow", &[10., 50., 100., 200.]),
                ("field-edge", "field", &[50.]),
                ("riverbank", "river", &[50.]),
            ],
            "TVIET" => &[("grassland", "meadow", &[50.])],
            _ => &[("water-belt", "river", &[50.])],
        };
        for &(label, kind, heights) in kinds {
            let mut spot = *spots
                .iter()
                .find(|s| s.label == kind)
                .ok_or_else(|| format!("no {code} {kind} spot"))?;
            if kind == "meadow" {
                spot = grass_meadow(&atlas, &world, spot)?;
            }
            if label == "field-edge" {
                spot = grass_field_edge(&atlas, &world, spot)?;
            }
            let site = atlas.site(&world, spot.x, spot.z);
            writeln!(
                notes,
                "{code} {label} x={} z={} class={:?} blocked={} water_proximity={:.4}",
                spot.x, spot.z, site.kind, site.blocked, site.near_water
            )?;
            for &height in heights {
                let pitch = if height <= 50. { 8. } else { 14. };
                let camera = look_at(&world, &spot, &shot(label, kind, height, pitch, 0.));
                writeln!(
                    file,
                    "{code}-{label}-{height:.0}ft {code} 0 12 0 {} {} {} {} {}",
                    camera.position[0],
                    camera.position[1],
                    camera.position[2],
                    camera.yaw,
                    camera.pitch
                )?;
            }
            if code == "UKR" && kind == "meadow" {
                let meadow = (0..129)
                    .filter(|i| {
                        let along = (*i as f64 / 128. - 0.5) * 3. * crate::vegetation::NM;
                        let x = spot.x + along * spot.heading.to_radians().sin();
                        let z = spot.z + along * spot.heading.to_radians().cos();
                        let s = atlas.site(&world, x, z);
                        s.kind == crate::vegetation::Kind::Grass && !s.blocked
                    })
                    .count();
                writeln!(
                    notes,
                    "UKR lowpass centreline: {meadow}/129 meadow samples, 3 nm, heading={}",
                    spot.heading
                )?;
                let y = f64::from(world.height(spot.x as f32, spot.z as f32)) + 100.;
                // Same world path is available to both the dense and meadow aliases.
                for alias in ["UKR", "UKR-MEADOW"] {
                    writeln!(
                        low,
                        "{alias}-high-dense {alias} 0 12 0 {} {y} {} {} {}",
                        spot.x,
                        spot.z,
                        spot.heading.to_radians(),
                        -4f64.to_radians()
                    )?;
                }
            }
        }
    }
    Ok(())
}

// The older broad-area labels score neighbours, so their centre can have a
// different class. Grass diagnostics require the actual centre to be meadow.
fn grass_meadow(
    atlas: &crate::vegetation::Atlas,
    world: &Terrain,
    mut spot: crate::vegetation::Spot,
) -> AppResult<crate::vegetation::Spot> {
    use crate::vegetation::Kind;
    let mut best: Option<(f64, f64, f64, f64)> = None;
    for dz in -32..=32 {
        for dx in -32..=32 {
            let x = (spot.x / 128.).floor() * 128. + f64::from(dx) * 128. + 64.;
            let z = (spot.z / 128.).floor() * 128. + f64::from(dz) * 128. + 64.;
            if world.edge_distance_nm(x, z) > 0. || atlas.kind_at(x as i64, z as i64) != Kind::Grass
            {
                continue;
            }
            let site = atlas.site(world, x, z);
            if site.blocked || site.slope > 8. {
                continue;
            }
            for heading in [0f64, 35., 90., 135.] {
                let yaw = heading.to_radians();
                let clear = (0..17)
                    .filter(|i| {
                        let along = (*i as f64 / 16. - 0.5) * 3. * crate::vegetation::NM;
                        let (sx, sz) = (x + along * yaw.sin(), z + along * yaw.cos());
                        world.edge_distance_nm(sx, sz) == 0.
                            && atlas.kind_at(sx as i64, sz as i64) == Kind::Grass
                    })
                    .count();
                let score = clear as f64 * 1000. - (x - spot.x).hypot(z - spot.z) / 10000.;
                if best.is_none_or(|b| score > b.0) {
                    best = Some((score, x, z, heading));
                }
            }
        }
    }
    let (_, x, z, heading) =
        best.ok_or("no actual unblocked meadow near the broad-area candidate")?;
    spot.x = x;
    spot.z = z;
    spot.heading = heading;
    Ok(spot)
}
fn grass_field_edge(
    atlas: &crate::vegetation::Atlas,
    world: &Terrain,
    mut spot: crate::vegetation::Spot,
) -> AppResult<crate::vegetation::Spot> {
    use crate::vegetation::Kind;
    let mut best: Option<(f64, f64, f64, f64)> = None;
    for dz in -128..=128 {
        for dx in -128..=128 {
            let x = (spot.x / 128.).floor() * 128. + f64::from(dx) * 128. + 64.;
            let z = (spot.z / 128.).floor() * 128. + f64::from(dz) * 128. + 64.;
            if atlas.kind_at(x as i64, z as i64) != Kind::Field {
                continue;
            }
            for (ox, oz, heading) in [(128., 0., 0.), (0., 128., 90.)] {
                let other = atlas.kind_at((x + ox) as i64, (z + oz) as i64);
                if !matches!(other, Kind::Grass | Kind::Forest | Kind::Scrub) {
                    continue;
                }
                if atlas.site(world, x, z).blocked || atlas.site(world, x + ox, z + oz).blocked {
                    continue;
                }
                // Prefer a meadow edge, then proximity. A field/wood edge is still
                // a real field boundary, not a fabricated procedural plot line.
                let rank = if other == Kind::Grass { 1_000_000. } else { 0. }
                    - (x - spot.x).hypot(z - spot.z);
                if best.is_none_or(|b| rank > b.0) {
                    best = Some((rank, x + ox * 0.5, z + oz * 0.5, heading));
                }
            }
        }
    }
    let (_, x, z, heading) = best.ok_or("no field boundary near the broad-area candidate")?;
    spot.x = x;
    spot.z = z;
    spot.heading = heading;
    Ok(spot)
}

fn dense_spot(
    atlas: &crate::vegetation::Atlas,
    world: &Terrain,
    code: &str,
) -> AppResult<(
    crate::vegetation::DensityCandidate,
    Vec<crate::vegetation::DensityCandidate>,
)> {
    let (mut spot, candidates) = if code == "UKR-MEADOW" {
        let spot = crate::vegetation::spots(atlas, world)
            .into_iter()
            .find(|s| s.label == "meadow")
            .ok_or("no Ukraine meadow candidate")?;
        let spot = grass_meadow(atlas, world, spot)?;
        (
            crate::vegetation::DensityCandidate {
                x: spot.x,
                z: spot.z,
                heading: spot.heading,
                label: "meadow".into(),
                instances: crate::vegetation::count_high(atlas, world, spot.x, spot.z),
            },
            Vec::new(),
        )
    } else {
        crate::vegetation::densest(atlas, world)
    };
    if let Ok(path) = std::env::var("TORE_VEG_LOWPASS_CAMERAS") {
        // Freeze the baseline's exact dense camera, avoiding a changed
        // coast mask or density choosing a different benchmark location.
        let text = std::fs::read_to_string(path)?;
        let fields = text
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>())
            .find(|f| f.len() == 10 && f[1] == code && f[0].ends_with("-high-dense"))
            .ok_or_else(|| format!("missing baseline lowpass camera for {code}"))?;
        spot.x = fields[5].parse()?;
        spot.z = fields[7].parse()?;
        spot.heading = fields[8].parse::<f64>()?.to_degrees();
        spot.label = "baseline-locked".into();
        spot.instances = crate::vegetation::count_high(atlas, world, spot.x, spot.z);
    }
    Ok((spot, candidates))
}

fn lowpass_theaters() -> AppResult<Vec<(&'static str, &'static str)>> {
    let Ok(text) = std::env::var("TORE_VEG_LOWPASS_THEATERS") else {
        return Ok(LOWPASS_THEATERS.to_vec());
    };
    let mut out = Vec::new();
    for part in text.split(',') {
        let code = part.trim().to_ascii_uppercase();
        if code.is_empty() {
            continue;
        }
        let Some(row) = LOWPASS_THEATERS.iter().find(|(name, _)| *name == code) else {
            return Err(format!("unknown lowpass theater {code}").into());
        };
        out.push(*row);
    }
    if out.is_empty() {
        return Err("TORE_VEG_LOWPASS_THEATERS is empty".into());
    }
    Ok(out)
}

fn lowpass_sizes() -> AppResult<Vec<[u32; 2]>> {
    let Ok(text) = std::env::var("TORE_VEG_LOWPASS_SIZES") else {
        return Ok(vec![[3840, 2160], [2560, 1440]]);
    };
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (w, h) = part
            .split_once('x')
            .ok_or_else(|| format!("TORE_VEG_LOWPASS_SIZES wants WxH, got {part}"))?;
        out.push([w.parse()?, h.parse()?]);
    }
    if out.is_empty() {
        return Err("TORE_VEG_LOWPASS_SIZES is empty".into());
    }
    Ok(out)
}

fn lowpass_densities() -> AppResult<Vec<crate::graphics::VegDensity>> {
    let Ok(text) = std::env::var("TORE_VEG_LOWPASS_DENSITIES") else {
        return Ok(vec![
            crate::graphics::VegDensity::High,
            crate::graphics::VegDensity::Ultra,
        ]);
    };
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(
            crate::graphics::VegDensity::parse(part)
                .ok_or_else(|| format!("bad lowpass density {part}"))?,
        );
    }
    if out.is_empty() {
        return Err("TORE_VEG_LOWPASS_DENSITIES is empty".into());
    }
    Ok(out)
}

fn lowpass_frames() -> AppResult<usize> {
    let frames = match std::env::var("TORE_VEG_LOWPASS_FRAMES") {
        Ok(text) => text.parse()?,
        Err(_) => 1_350,
    };
    if !(2..=1_350).contains(&frames) {
        return Err("TORE_VEG_LOWPASS_FRAMES must be 2 to 1350".into());
    }
    Ok(frames)
}

fn lowpass_stride() -> AppResult<usize> {
    let stride = match std::env::var("TORE_VEG_LOWPASS_STRIDE") {
        Ok(text) => text.parse()?,
        Err(_) => 1,
    };
    if stride == 0 {
        return Err("TORE_VEG_LOWPASS_STRIDE must be at least 1".into());
    }
    Ok(stride)
}

fn lowpass_indices(frames: usize, stride: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..frames).step_by(stride).collect();
    if indices.last().copied() != Some(frames - 1) {
        indices.push(frames - 1);
    }
    indices
}

fn lowpass_options(density: crate::graphics::VegDensity) -> crate::graphics::Options {
    crate::graphics::Options {
        vegetation_density: density,
        ..Default::default()
    }
}

fn lowpass_camera(
    world: &Terrain,
    spot: &crate::vegetation::DensityCandidate,
    along: f64,
) -> Camera {
    let yaw = spot.heading.to_radians();
    let x = spot.x + along * yaw.sin();
    let z = spot.z + along * yaw.cos();
    let ground = f64::from(world.height(x as f32, z as f32));
    let mut camera = Camera::new();
    camera.position = [x, ground + 100., z];
    camera.yaw = yaw as f32;
    camera.pitch = -4_f32.to_radians();
    camera.roll = 0.;
    camera
}

fn lowpass_flight(
    world: &Terrain,
    spot: &crate::vegetation::DensityCandidate,
    frames: usize,
    indices: &[usize],
) -> Vec<(usize, Camera)> {
    indices
        .iter()
        .map(|&frame| {
            let along = -1.5 * crate::vegetation::NM
                + 3. * crate::vegetation::NM * frame as f64 / (frames - 1) as f64;
            (frame, lowpass_camera(world, spot, along))
        })
        .collect()
}

fn lowpass_outside(world: &Terrain, camera: &Camera) -> bool {
    tore_world::terrain::edge_distance_nm(
        world.theater.cols,
        world.theater.rows,
        camera.position[0],
        camera.position[2],
    ) > 0.
}

fn run_stats(samples: &[Sample]) -> RunStats {
    let mut added: Vec<f64> = samples.iter().map(|sample| sample.added()).collect();
    let added_mean = mean(&added);
    let added_p95 = percentile(&mut added, 0.95);
    let added_max = maximum(&added);
    let on: Vec<f64> = samples.iter().map(|sample| sample.gpu_on).collect();
    let placed_max = samples
        .iter()
        .map(|sample| sample.placed)
        .max()
        .unwrap_or(0);
    let tufts_max = samples.iter().map(|sample| sample.tufts).max().unwrap_or(0);
    let builds_max = samples
        .iter()
        .map(|sample| sample.builds())
        .max()
        .unwrap_or(0);
    let rebuild = samples
        .iter()
        .max_by(|a, b| a.build_ms.total_cmp(&b.build_ms));
    let mut submit: Vec<f64> = samples.iter().map(|s| s.vegetation_submit_ms).collect();
    let vegetation_submit = [
        mean(&submit),
        percentile(&mut submit, 0.95),
        maximum(&submit),
    ];
    RunStats {
        vegetation_submit,
        samples: samples.len(),
        added_mean,
        added_p95,
        added_max,
        on_mean: mean(&on),
        on_max: maximum(&on),
        placed_mean: mean(
            &samples
                .iter()
                .map(|sample| sample.placed as f64)
                .collect::<Vec<_>>(),
        ),
        placed_max,
        tufts_mean: mean(
            &samples
                .iter()
                .map(|sample| sample.tufts as f64)
                .collect::<Vec<_>>(),
        ),
        tufts_max,
        builds_mean: mean(
            &samples
                .iter()
                .map(|sample| sample.builds() as f64)
                .collect::<Vec<_>>(),
        ),
        builds_max,
        object_builds: samples.iter().map(|sample| sample.object_chunks).sum(),
        grass_builds: samples.iter().map(|sample| sample.grass_chunks).sum(),
        rebuild_max: rebuild.map(|sample| sample.build_ms).unwrap_or(0.),
        rebuild_frame: rebuild.map(|sample| sample.frame).unwrap_or(0),
        submit_max: maximum(
            &samples
                .iter()
                .map(|sample| sample.submit_ms)
                .collect::<Vec<_>>(),
        ),
    }
}

fn maximum(values: &[f64]) -> f64 {
    values.iter().copied().reduce(f64::max).unwrap_or(0.)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    if values.is_empty() {
        return 0.;
    }
    values.sort_by(f64::total_cmp);
    let index = ((values.len() as f64) * p).ceil() as usize - 1;
    values[index.min(values.len() - 1)]
}

fn format_run(
    code: &str,
    size: [u32; 2],
    density: &str,
    stats: &RunStats,
    warm: &[Sample],
    warm_stats: &RunStats,
    timed: &[Sample],
) -> String {
    let mut text = format!(
        "\n{code} {}x{} {density}\n\
         warmup frames {}: added mean {:.3} ms, max {:.3} ms, on mean {:.3} ms, max {:.3} ms\n\
         warmup builds: {} object chunks, {} grass chunks, worst rebuild {:.3} ms, worst submit {:.3} ms\n\
         samples {}: added mean {:.3} ms, p95 {:.3} ms, max {:.3} ms\n\
         on mean {:.3} ms, max {:.3} ms\n\
         instances mean {:.0}, max {}\n\
         grass tufts mean {:.0}, max {}\n\
         cache during pass: {} object chunks, {} grass chunks, {:.2} builds/frame mean, {} max\n\
         worst rebuild {:.3} ms at frame {}, worst submit {:.3} ms\n",
        size[0],
        size[1],
        warm.len(),
        warm_stats.added_mean,
        warm_stats.added_max,
        warm_stats.on_mean,
        warm_stats.on_max,
        warm.iter().map(|sample| sample.object_chunks).sum::<u64>(),
        warm.iter().map(|sample| sample.grass_chunks).sum::<u64>(),
        warm_stats.rebuild_max,
        warm_stats.submit_max,
        stats.samples,
        stats.added_mean,
        stats.added_p95,
        stats.added_max,
        stats.on_mean,
        stats.on_max,
        stats.placed_mean,
        stats.placed_max,
        stats.tufts_mean,
        stats.tufts_max,
        stats.object_builds,
        stats.grass_builds,
        stats.builds_mean,
        stats.builds_max,
        stats.rebuild_max,
        stats.rebuild_frame,
        stats.submit_max
    );
    text += &format!(
        "vegetation CPU submit mean {:.3} ms, p95 {:.3} ms, max {:.3} ms\n",
        stats.vegetation_submit[0], stats.vegetation_submit[1], stats.vegetation_submit[2]
    );
    let mut spikes: Vec<&Sample> = timed.iter().filter(|sample| sample.added() > 10.).collect();
    spikes.sort_by(|a, b| b.added().total_cmp(&a.added()));
    spikes.truncate(8);
    for sample in spikes {
        text += &format!(
            "  spike frame {}: added {:.3} ms, on {:.3} ms, instances {}, tufts {}, object chunks {}, grass chunks {}, rebuild {:.3} ms, submit {:.3} ms\n",
            sample.frame,
            sample.added(),
            sample.gpu_on,
            sample.placed,
            sample.tufts,
            sample.object_chunks,
            sample.grass_chunks,
            sample.build_ms,
            sample.submit_ms
        );
    }
    text
}

fn summary_row(
    code: &str,
    climate: &str,
    spot: &crate::vegetation::DensityCandidate,
    size: [u32; 2],
    density: &str,
    stats: &RunStats,
    warmup: usize,
) -> String {
    format!(
        "| {code} | {climate} | {} ({}) | {}x{} | {density} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.0} / {} | {:.0} / {} | {:.2} / {} | {:.2} | {warmup} | {:.3} / {:.3} / {:.3} |",
        spot.label,
        spot.instances,
        size[0],
        size[1],
        stats.added_mean,
        stats.added_p95,
        stats.added_max,
        stats.on_mean,
        stats.on_max,
        stats.placed_mean,
        stats.placed_max,
        stats.tufts_mean,
        stats.tufts_max,
        stats.builds_mean,
        stats.builds_max,
        stats.rebuild_max,
        stats.vegetation_submit[0],
        stats.vegetation_submit[1],
        stats.vegetation_submit[2],
    )
}

fn format_note(
    code: &str,
    size: [u32; 2],
    density: &str,
    stats: &RunStats,
    timed: &[Sample],
) -> String {
    let hot: Vec<&Sample> = timed
        .iter()
        .filter(|sample| sample.added() >= stats.added_p95 && stats.added_p95 > 0.)
        .collect();
    let cool: Vec<&Sample> = timed
        .iter()
        .filter(|sample| sample.added() < stats.added_p95)
        .collect();
    let avg = |list: &[&Sample], f: fn(&Sample) -> f64| -> f64 {
        mean(&list.iter().map(|sample| f(sample)).collect::<Vec<_>>())
    };
    format!(
        "{code} {}x{} {density}: p95 {:.2} ms, max {:.2} ms. Frames at or above the 95th percentile \
         averaged {:.0} instances, {:.0} grass tufts and {:.2} ms of patch builds, against {:.0}, \
         {:.0} and {:.2} ms on the other frames. Worst rebuild was {:.2} ms at frame {} (submit {:.2} ms).\n",
        size[0],
        size[1],
        stats.added_p95,
        stats.added_max,
        avg(&hot, |sample| sample.placed as f64),
        avg(&hot, |sample| sample.tufts as f64),
        avg(&hot, |sample| sample.build_ms),
        avg(&cool, |sample| sample.placed as f64),
        avg(&cool, |sample| sample.tufts as f64),
        avg(&cool, |sample| sample.build_ms),
        stats.rebuild_max,
        stats.rebuild_frame,
        stats.submit_max
    )
}

fn write_summary(out: &Path, rows: &[String], notes: &[String]) -> AppResult<()> {
    let mut text = String::from(
        "# Low-level pass\n\n\
         100 ft above the ground, 480 kt, 3 nm, nose 4 degrees down. Added cost is GPU time \
         with vegetation objects, canopy and grass on, minus the same frame with them off. Ground \
         detail stays on in both. High and Ultra change density only. Distances stay at the \
         Graphics defaults, 4 nm draw and 2 nm detail. Instances are the objects submitted \
         to the cull. Grass tufts are the blades submitted, before the shader drops any. \
         Rebuilds are worker object and grass chunks delivered during the pass. CPU\n\
         vegetation submit includes preparation, uploads and cull encoding, excluding\n\
         the worker and GPU wait. All summary numbers exclude warmup.\n\n\
         | Theater | Climate | Spot (instances in 1 nm) | Size | Density | Added mean | Added p95 | Added max | On mean | On max | Instances mean / max | Tufts mean / max | Builds per frame mean / max | Worst rebuild ms | Warmup frames | CPU vegetation submit mean / p95 / max ms |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    text.push_str("\n## Above 3 ms at the 95th percentile, or a frame above 10 ms\n\n");
    if notes.is_empty() {
        text.push_str("No run crossed either line.\n");
    } else {
        for note in notes {
            text.push_str(note);
            text.push('\n');
        }
    }
    std::fs::write(out.join("SUMMARY.md"), text)?;
    Ok(())
}

fn write_lowpass_readme(out: &Path, images: &[(String, String, String, usize)]) -> AppResult<()> {
    let mut text = String::from(
        "# Low-pass frames\n\n\
         3840 by 2160, High density, noon, clear. The camera is 100 ft above the ground, \
         level, nose 4 degrees down, looking along the pass. `start` is 1.5 nm before the \
         densest point, `dense` is that point, `end` is 1.5 nm past it.\n\n",
    );
    for (file, code, name, instances) in images {
        text.push_str(&format!(
            "- `{file}`: {code} {name}, {instances} High-density instances within 1 nm of the dense point\n"
        ));
    }
    std::fs::write(out.join("README.md"), text)?;
    Ok(())
}

/// Top-down classification pictures: Ukraine around its woodland and its
/// fields, Egypt around its dunes and its river.
fn overlay(resources: &std::collections::BTreeMap<String, Vec<u8>>, out: &Path) -> AppResult<()> {
    for (code, labels) in [
        ("UKR", ["wood", "field", "hill"]),
        ("EGY", ["dunes", "rocky", "oasis"]),
    ] {
        println!("vegetation preview: overlay {code}");
        let world = Terrain::for_mission(
            resources,
            code.split('-').next().unwrap_or(code),
            Some(0),
            &overrides([12, 0])?,
        )?;
        let scenery = Scenery::build(resources, &world)?;
        let atlas = scenery
            .vegetation
            .as_ref()
            .ok_or("vegetation atlas missing")?;
        let found = crate::vegetation::spots(atlas, &world);
        for label in labels {
            let Some(spot) = found.iter().find(|s| s.label == label) else {
                continue;
            };
            for (span, px) in [(40_000., 1024usize), (8_000., 1024)] {
                let rgb = atlas.overlay(&world, [spot.x, spot.z], span, px);
                let mut rgba = Vec::with_capacity(px * px * 4);
                for c in rgb.chunks_exact(3) {
                    rgba.extend([c[0], c[1], c[2], 255]);
                }
                let png = crate::replay::png::encode_rgba(px as u32, px as u32, &rgba)?;
                let name = format!("overlay-{code}-{label}-{}ft.png", span as u32);
                std::fs::write(out.join(&name), png)?;
                println!("  {name}");
            }
        }
    }
    Ok(())
}

/// Renders every camera of a `cameras.txt`, writing `NAME.png`.
fn replay_cameras(
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    out: &Path,
    text: &str,
) -> AppResult<()> {
    let mut list = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        list.push(Cam::parse(line)?);
    }
    let size = size()?;
    let mut current: Option<(String, Terrain, Scenery, View)> = None;
    for cam in list {
        if current.as_ref().is_none_or(|c| c.0 != cam.code) {
            println!("vegetation preview: building {}", cam.code);
            let world = Terrain::for_mission(
                resources,
                &cam.code,
                Some(cam.condition),
                &overrides(cam.time)?,
            )?;
            let mut scenery = Scenery::build(resources, &world)?;
            if std::env::var("TORE_SEAM_MASK_ONLY").as_deref() == Ok("1") {
                scenery.seams[0] = 2.;
            }
            if std::env::var("TORE_TERRAIN_MATERIAL_ONLY").as_deref() == Ok("1") {
                scenery.seams[0] = 3.;
            }
            if std::env::var("TORE_TERRAIN_MATERIAL_ONLY").as_deref() == Ok("mask") {
                scenery.seams[0] = 4.;
            }
            let mut view =
                pollster::block_on(View::new(&scenery, size, options(density()?), false))?;
            view.set_shown(std::env::var("TORE_SEAM_SURFACE_ONLY").as_deref() != Ok("1"));
            // The theater's static ground objects (airports, buildings,
            // sites) standing, as a mission starts them.
            if std::env::var("TORE_VEG_PREVIEW_STATIC").as_deref() == Ok("1") {
                let geometry = std::sync::Arc::clone(
                    scenery.static_geometry_where(&std::collections::BTreeSet::new()),
                );
                view.sim.airports(&view.device, &view.queue, &geometry);
            }
            // `list`: print each placement's box (feet), to find scenes.
            if std::env::var("TORE_VEG_PREVIEW_STATIC").as_deref() == Ok("list") {
                let geometry = std::sync::Arc::clone(
                    scenery.static_geometry_where(&std::collections::BTreeSet::new()),
                );
                for range in &geometry.placements {
                    let v = &geometry.vertices[range.start as usize * 10..range.end as usize * 10];
                    let b = crate::terrain_chunks::Bounds::of_vertices(v);
                    println!(
                        "placement {} {:.0} {:.0} {:.0} {:.0} {:.0} {:.0} {}",
                        cam.code,
                        b.min[0],
                        b.min[1],
                        b.min[2],
                        b.max[0],
                        b.max[1],
                        b.max[2],
                        range.len() / 3
                    );
                }
            }
            current = Some((cam.code.clone(), world, scenery, view));
        }
        let (_, world, scenery, view) = current.as_mut().expect("built above");
        let camera = cam.camera();
        let ms = view.time(&camera, world, scenery)?;
        // With TORE_VEG_GPU_TIMING=1, also the GPU's own time: the median
        // of seven eight-frame means from timestamp queries.
        let mut gpu = Vec::new();
        if std::env::var("TORE_VEG_GPU_TIMING").as_deref() == Ok("1") {
            for _ in 0..7 {
                gpu.extend(view.gpu_time(&camera, world, scenery)?);
            }
            gpu.sort_by(f64::total_cmp);
        }
        let pixels = view.pixels(&camera, world, scenery)?;
        write_png(out, &format!("{}.png", cam.name), size, &pixels)?;
        match gpu.get(gpu.len() / 2) {
            Some(g) => println!("  {}: {ms:.2} ms, gpu {g:.3} ms", cam.name),
            None => println!("  {}: {ms:.2} ms", cam.name),
        }
    }
    Ok(())
}

/// Export the original material sprites without reading an import.
pub fn art() -> AppResult<()> {
    let out = std::env::args()
        .nth(2)
        .ok_or("--vegetation-art needs an output directory")?;
    let out = Path::new(&out);
    std::fs::create_dir_all(out)?;
    let mut pages = Vec::new();
    let lib =
        crate::vegetation_shapes::Library::build(crate::vegetation::region("UKR"), &mut pages, 0);
    let mut index = String::from("name\treference_height_ft\treference_width_ft\taspect\n");
    let mut crowns =
        String::from("source\tcrowns\tgrove_width\tmax_crown_width\ttransparent_pixels\n");
    for mesh in &lib.meshes {
        let name = mesh.source.trim_start_matches("original/");
        let reference = if mesh.look == crate::vegetation_shapes::Look::Solid {
            12.
        } else if name.contains("bush")
            || name.contains("scrub")
            || name.contains("fern")
            || name.contains("yucca")
        {
            4.
        } else {
            57.
        };
        index.push_str(&format!(
            "{name}\t{reference}\t{:.5}\t{:.5}\n",
            reference * 2. * mesh.radius,
            2. * mesh.radius
        ));
        for top in [false, true] {
            let slot = if top {
                (-mesh.top[0] - 1.) as usize
            } else {
                mesh.sprite_layer * 4 + usize::from(mesh.sprite_tile.unwrap())
            };
            let cell = crate::vegetation_shapes::cell(&pages, slot);
            let width = if top {
                124
            } else {
                (248. * mesh.radius).round().clamp(1., 2048.) as usize
            };
            let mut rgba = Vec::new();
            let mut transparent = 0;
            for y in 0..124 {
                for x in 0..width {
                    let p = cell[(y + 2) * 128 + 2 + x * 124 / width];
                    if p == 255 {
                        rgba.extend([0, 0, 0, 0]);
                        transparent += 1;
                    } else {
                        let tint = match p / 32 {
                            1 => mesh.recolour.unwrap(),
                            2 => crate::vegetation::lin([139, 130, 114]),
                            3 => crate::vegetation::lin([124, 104, 72]),
                            _ => crate::vegetation::lin([78, 103, 44]),
                        };
                        let rgb = tint.map(|c| {
                            (c * crate::vegetation_shapes::shading(p))
                                .clamp(0., 1.)
                                .powf(1. / 2.2)
                                * 255.
                        });
                        rgba.extend([rgb[0] as u8, rgb[1] as u8, rgb[2] as u8, 255]);
                    }
                }
            }
            let png = crate::replay::png::encode_cutout_rgba(width as u32, 124, &rgba)?;
            std::fs::write(
                out.join(format!("{name}{}.png", if top { "-top" } else { "" })),
                png,
            )?;
            if top {
                crowns.push_str(&format!(
                    "{name}\t1\t{:.5}\t{:.5}\t{transparent}\n",
                    2. * mesh.top[2],
                    2. * mesh.top[2]
                ));
            }
        }
    }
    std::fs::write(out.join("index.tsv"), index)?;
    std::fs::write(out.join("top-crowns.tsv"), crowns)?;
    println!(
        "original sprites: {} independent side/top material pairs",
        lib.meshes.len()
    );
    Ok(())
}

fn overrides(time: [i32; 2]) -> AppResult<crate::terrain::Overrides> {
    Ok(crate::terrain::Overrides {
        time: Some(time),
        wind: None,
        cloud_altitude: Some(0),
        redrawn_airports: crate::scenery::redrawn_airports()?,
        city_fill: crate::scenery::city_fill()?,
        city_fill_cap: crate::scenery::city_fill_cap()?,
    })
}

/// A camera `shot.height` above the ground looking down `shot.pitch` at the
/// spot, along the spot's heading.
fn look_at(world: &Terrain, spot: &crate::vegetation::Spot, shot: &Shot) -> Camera {
    let pitch = shot.pitch.to_radians();
    let yaw = spot.heading.to_radians();
    let back = if shot.ahead > 0. {
        shot.ahead
    } else {
        (shot.height / pitch.tan().abs().max(1e-3)).min(12_000.)
    };
    let x = spot.x - back * yaw.sin();
    let z = spot.z - back * yaw.cos();
    let ground = f64::from(world.height(x as f32, z as f32));
    let mut camera = Camera::new();
    camera.yaw = yaw as f32;
    camera.pitch = -pitch as f32;
    camera.position = [x, ground + shot.height, z];
    camera
}

fn write_png(dir: &Path, name: &str, size: [u32; 2], rgba: &[u8]) -> AppResult<()> {
    let bytes = crate::replay::png::encode_rgba(size[0], size[1], rgba)?;
    std::fs::write(dir.join(name), bytes)?;
    Ok(())
}

struct View {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sim: crate::sim_renderer::SimRenderer,
    texture: wgpu::Texture,
    buffer: wgpu::Buffer,
    size: [u32; 2],
    options: crate::graphics::Options,
}

impl View {
    async fn new(
        scenery: &Scenery,
        size: [u32; 2],
        options: crate::graphics::Options,
        force_timestamps: bool,
    ) -> AppResult<Self> {
        let instance = crate::graphics_api::instance();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await?;
        println!("vegetation preview adapter: {}", adapter.get_info().name);
        let timing = force_timestamps || std::env::var("TORE_VEG_GPU_TIMING").as_deref() == Ok("1");
        let features = if timing {
            wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
        } else {
            wgpu::Features::empty()
        };
        if !adapter.features().contains(features) {
            return Err("vegetation GPU timestamps unsupported by this adapter".into());
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features: features,
                ..Default::default()
            })
            .await?;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut sim =
            crate::sim_renderer::SimRenderer::new(&device, &queue, format, scenery, options, 4);
        if let Some(vegetation) = &mut sim.vegetation {
            vegetation.set_settle(true);
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Vegetation preview"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vegetation preview readback"),
            size: u64::from(size[0]) * u64::from(size[1]) * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Ok(Self {
            device,
            queue,
            sim,
            texture,
            buffer,
            size,
            options,
        })
    }

    fn set_shown(&mut self, on: bool) {
        let mut options = self.options;
        if !on {
            options.ground_texture = crate::graphics::GroundTexture::Off;
        }
        self.sim.set_graphics(&self.device, options, 4);
        if let Some(vegetation) = &mut self.sim.vegetation {
            vegetation.set_suppressed(!on);
            vegetation.set_settle(true);
        }
    }

    fn begin_run(&mut self, size: [u32; 2], density: crate::graphics::VegDensity) -> AppResult<()> {
        self.ensure_size(size);
        let options = lowpass_options(density);
        self.options = options;
        self.sim.set_graphics(&self.device, options, 4);
        let vegetation = self
            .sim
            .vegetation
            .as_mut()
            .ok_or("vegetation renderer missing")?;
        vegetation.set_settle(false);
        vegetation.set_suppressed(false);
        vegetation.reset_cache();
        Ok(())
    }

    fn ensure_size(&mut self, size: [u32; 2]) {
        if self.size == size {
            return;
        }
        self.size = size;
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        self.texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Vegetation preview"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vegetation preview readback"),
            size: u64::from(size[0]) * u64::from(size[1]) * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    }

    fn set_vegetation(&mut self, on: bool) {
        if let Some(vegetation) = &mut self.sim.vegetation {
            vegetation.set_suppressed(!on);
        }
    }

    /// Fills the patch cache at the production per-frame budget. Stops on the
    /// first frame that builds nothing.
    fn warmup(
        &mut self,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
        trace: &Path,
    ) -> AppResult<Vec<Sample>> {
        let mut trace = std::fs::File::create(trace)?;
        write!(
            trace,
            "{}",
            crate::vegetation_renderer::CacheFrame::CSV_HEADER
        )?;
        let mut frames = Vec::new();
        while frames.len() < 4_000 {
            let start = frames.len();
            let batch = (8).min(4_000 - start);
            let jobs: Vec<(usize, &Camera)> = (0..batch).map(|i| (start + i, camera)).collect();
            let samples = self.measure(&jobs, world, scenery)?;
            for sample in &samples {
                sample.stream.write_csv(&mut trace, sample.frame)?;
            }
            if start % 240 == 0 {
                println!(
                    "    warmup frame {}: {:?}",
                    start + batch,
                    samples.last().map(|s| s.stream)
                );
            }
            if let Some(idle) = samples.iter().position(|sample| {
                !sample.pending && sample.object_chunks == 0 && sample.grass_chunks == 0
            }) {
                frames.extend(samples.into_iter().take(idle + 1));
                return Ok(frames);
            }
            frames.extend(samples);
        }
        Err(format!(
            "vegetation warmup did not converge within 4000 frames; last state: {:?}",
            frames.last().map(|s| s.stream)
        )
        .into())
    }

    /// One off frame and one on frame per camera. GPU timestamps exclude the
    /// worker patch build. Vegetation preparation and whole-frame CPU submit
    /// are recorded separately from GPU time.
    fn measure(
        &mut self,
        frames: &[(usize, &Camera)],
        world: &Terrain,
        scenery: &Scenery,
    ) -> AppResult<Vec<Sample>> {
        let period = f64::from(self.queue.get_timestamp_period());
        if period <= 0. {
            return Err("GPU timestamp period is zero".into());
        }
        let mut observer = crate::vegetation_renderer::popin::Monitor::default();
        let mut out = Vec::with_capacity(frames.len());
        for chunk in frames.chunks(8) {
            let count = (chunk.len() * 6) as u32;
            let queries = self.device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("Lowpass timestamps"),
                ty: wgpu::QueryType::Timestamp,
                count,
            });
            let bytes = u64::from(count) * 8;
            let resolved = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Lowpass timestamps resolved"),
                size: bytes,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Lowpass timestamps readback"),
                size: bytes,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut cpu = Vec::with_capacity(chunk.len());
            for (i, (frame, camera)) in chunk.iter().enumerate() {
                self.set_vegetation(false);
                self.stamp(&queries, (i * 6) as u32, camera, world, scenery);
                self.set_vegetation(true);
                let started = Instant::now();
                // Prime and upload once, outside both grass comparison intervals.
                self.draw(camera, world, scenery);
                let cache = self
                    .sim
                    .vegetation
                    .as_ref()
                    .map(|vegetation| vegetation.cache_frame())
                    .unwrap_or_default();
                let (placed, tufts) = self
                    .sim
                    .vegetation
                    .as_ref()
                    .map(|vegetation| (vegetation.placed(), vegetation.tufts_drawn()))
                    .unwrap_or_default();
                let submit_ms = started.elapsed().as_secs_f64() * 1_000.;
                let popin = self
                    .sim
                    .vegetation
                    .as_ref()
                    .map_or(Default::default(), |v| {
                        observer.sample(
                            v.near_population(camera, self.size),
                            [camera.position[0], camera.position[2]],
                            v.detail_reach(),
                        )
                    });
                let blades = self.sim.vegetation.as_ref().map_or(0, |v| v.blades_drawn());
                self.sim.vegetation.as_mut().unwrap().set_repeat_frame(true);
                // Alternate order to reduce systematic cache/clock bias. Both
                // intervals use identical tree pages, cull commands and uniforms.
                for grass_off in if frame % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    self.sim
                        .vegetation
                        .as_mut()
                        .unwrap()
                        .set_grass_suppressed(grass_off);
                    self.stamp(
                        &queries,
                        (i * 6 + if grass_off { 4 } else { 2 }) as u32,
                        camera,
                        world,
                        scenery,
                    );
                }
                let vegetation = self.sim.vegetation.as_mut().unwrap();
                vegetation.set_grass_suppressed(false);
                vegetation.set_repeat_frame(false);
                cpu.push((*frame, placed, tufts, blades, cache, submit_ms, popin));
            }
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.resolve_query_set(&queries, 0..count, &resolved, 0);
            encoder.copy_buffer_to_buffer(&resolved, 0, &readback, 0, bytes);
            self.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            self.wait()?;
            rx.recv()??;
            let mapped = readback.slice(..).get_mapped_range();
            let ticks: Vec<u64> = mapped
                .chunks_exact(8)
                .map(|word| u64::from_le_bytes(word.try_into().expect("timestamp")))
                .collect();
            drop(mapped);
            readback.unmap();
            for (i, (frame, placed, tufts, blades, cache, submit_ms, popin)) in
                cpu.into_iter().enumerate()
            {
                out.push(Sample {
                    frame,
                    gpu_off: ticks[i * 6 + 1].saturating_sub(ticks[i * 6]) as f64 * period
                        / 1_000_000.,
                    gpu_on: ticks[i * 6 + 3].saturating_sub(ticks[i * 6 + 2]) as f64 * period
                        / 1_000_000.,
                    gpu_no_grass: ticks[i * 6 + 5].saturating_sub(ticks[i * 6 + 4]) as f64 * period
                        / 1_000_000.,
                    blades,
                    placed,
                    tufts,
                    object_chunks: cache.object_chunks,
                    grass_chunks: cache.grass_chunks,
                    build_ms: cache.build_ms,
                    submit_ms,
                    vegetation_submit_ms: cache.submit_ms,
                    pending: cache.pending,
                    stream: cache,
                    popin,
                });
            }
            if frames.len() > 8 && (out.len() % 120 < 8 || out.len() == frames.len()) {
                println!("    sampled {}/{}", out.len(), frames.len());
            }
        }
        Ok(out)
    }

    fn stamp(
        &mut self,
        queries: &wgpu::QuerySet,
        index: u32,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
    ) {
        let view = self.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.write_timestamp(queries, index);
        self.sim.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            self.size,
            camera,
            world,
            scenery,
        );
        encoder.write_timestamp(queries, index + 1);
        self.queue.submit([encoder.finish()]);
    }

    fn draw(&mut self, camera: &Camera, world: &Terrain, scenery: &Scenery) {
        let view = self.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.sim.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            self.size,
            camera,
            world,
            scenery,
        );
        self.queue.submit([encoder.finish()]);
    }

    fn wait(&self) -> AppResult<()> {
        self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })?;
        Ok(())
    }

    /// Actual GPU command time, excluding CPU placement/submission and readback.
    /// Optional so the ordinary preview remains portable to adapters without queries.
    fn gpu_time(
        &mut self,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
    ) -> AppResult<Option<f64>> {
        if !self
            .device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
        {
            return Ok(None);
        }
        const FRAMES: u32 = 8;
        self.draw(camera, world, scenery);
        self.draw(camera, world, scenery);
        self.wait()?;
        let queries = self.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("Vegetation frame timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: FRAMES * 2,
        });
        let bytes = u64::from(FRAMES) * 16;
        let resolved = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vegetation timestamps resolved"),
            size: bytes,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Vegetation timestamps readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        for frame in 0..FRAMES {
            let view = self.texture.create_view(&Default::default());
            let mut encoder = self.device.create_command_encoder(&Default::default());
            encoder.write_timestamp(&queries, frame * 2);
            self.sim.draw(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                self.size,
                camera,
                world,
                scenery,
            );
            encoder.write_timestamp(&queries, frame * 2 + 1);
            if frame + 1 == FRAMES {
                encoder.resolve_query_set(&queries, 0..FRAMES * 2, &resolved, 0);
                encoder.copy_buffer_to_buffer(&resolved, 0, &readback, 0, bytes);
            }
            self.queue.submit([encoder.finish()]);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.wait()?;
        rx.recv()??;
        let mapped = readback.slice(..).get_mapped_range();
        let ticks: u64 = mapped
            .chunks_exact(16)
            .map(|pair| {
                let start = u64::from_le_bytes(pair[..8].try_into().expect("timestamp"));
                let end = u64::from_le_bytes(pair[8..].try_into().expect("timestamp"));
                end.saturating_sub(start)
            })
            .sum();
        let milliseconds = ticks as f64 * f64::from(self.queue.get_timestamp_period())
            / 1_000_000.
            / f64::from(FRAMES);
        drop(mapped);
        readback.unmap();
        Ok(Some(milliseconds))
    }

    /// Mean of eight frames after a warmup, from submit through the wait.
    /// The readback is not included.
    fn time(&mut self, camera: &Camera, world: &Terrain, scenery: &Scenery) -> AppResult<f64> {
        self.draw(camera, world, scenery);
        self.draw(camera, world, scenery);
        self.wait()?;
        let start = Instant::now();
        const FRAMES: u32 = 8;
        for _ in 0..FRAMES {
            self.draw(camera, world, scenery);
        }
        self.wait()?;
        Ok(start.elapsed().as_secs_f64() * 1_000. / f64::from(FRAMES))
    }

    fn pixels(
        &mut self,
        camera: &Camera,
        world: &Terrain,
        scenery: &Scenery,
    ) -> AppResult<Vec<u8>> {
        self.draw(camera, world, scenery);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.size[0] * 4),
                    rows_per_image: Some(self.size[1]),
                },
            },
            wgpu::Extent3d {
                width: self.size[0],
                height: self.size[1],
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result);
            });
        self.wait()?;
        rx.recv()??;
        let mapped = self.buffer.slice(..).get_mapped_range();
        let pixels = mapped.to_vec();
        drop(mapped);
        self.buffer.unmap();
        Ok(pixels)
    }
}

/// CPU-only, real mission loader through vegetation. Every imported layout,
/// PGU first. No GPU instance, window, or simulation tick is created here.
pub(crate) fn limits_check(
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
    out: &Path,
) -> AppResult<()> {
    use crate::vegetation_limits as limits;
    std::fs::create_dir_all(out)?;
    let mut catalog = tore_formats::theater::map_catalog(resources)?;
    if crate::vegetation::REGIONS
        .iter()
        .any(|r| !catalog.iter().any(|(code, _)| code == r.code))
    {
        return Err("limits audit requires all 16 imported theaters".into());
    }
    catalog.sort_by_key(|(code, _)| (code != "PGU", code.clone()));
    let resources = std::sync::Arc::new(resources.clone());
    let player =
        crate::aircraft::Airframe::load(&resources, tore_formats::aircraft::AircraftId::F18)?.kind;
    let mut report = std::fs::File::create(out.join("theater-limits.tsv"))?;
    writeln!(
        report,
        "theater\tground_width\tground_height\thabitat_pages\thabitat_width\thabitat_height\thabitat_layers\tshared_art_pages\tshared_art_layers\tcanopy_pages\tmeshes\tmax_slot_bytes\tmax_drawn_bytes\tload_seconds"
    )?;
    for (code, _) in &catalog {
        let start = Instant::now();
        let data = std::sync::Arc::clone(&resources);
        let player = std::sync::Arc::clone(&player);
        let spec = crate::mission::MissionSpec::new(code, tore_formats::aircraft::AircraftId::F18);
        let job = crate::loading::Job::spawn(move |progress| {
            let (_, _, scene) = crate::loading::prepare_mission(&spec, &data, player, &progress)?;
            for device in [wgpu::Limits::downlevel_defaults(), wgpu::Limits::default()] {
                limits::validate(&scene, &device)?;
            }
            let lib = scene
                .vegetation_meshes
                .as_ref()
                .ok_or("vegetation missing from loaded mission")?;
            let habitat = scene.grass.mask.len() / (256 * 256 * 4);
            let size = limits::Pages::new(habitat, &wgpu::Limits::downlevel_defaults())?.size;
            let art = (scene.sky_indices.len() + scene.texture_indices.len()) / 65536;
            let canopy = scene.canopy.pages_x * (scene.canopy.area[3] as usize).div_ceil(256);
            Ok((
                scene.grass.terrain_size,
                habitat,
                size,
                art,
                canopy,
                lib.meshes.len(),
            ))
        })?;
        let (ground, habitat, size, art, canopy, meshes) = loop {
            if let Some(result) = job.poll() {
                break result.map_err(|e| format!("{code}: {e}"))?;
            }
            if start.elapsed().as_secs() > 300 {
                return Err(format!("{code}: loader timed out").into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let capacity = crate::graphics::VegDensity::Ultra.object_budget();
        writeln!(
            report,
            "{code}\t{}\t{}\t{habitat}\t{}\t{}\t{}\t{art}\t{}\t{canopy}\t{meshes}\t{}\t{}\t{:.3}",
            ground[0],
            ground[1],
            size.width,
            size.height,
            size.depth_or_array_layers,
            art.div_ceil(16),
            limits::instance_bytes(capacity),
            capacity * 96,
            start.elapsed().as_secs_f64()
        )?;
        report.flush()?;
        println!(
            "vegetation limits: PASS {code}, habitat {}x{}x{}, art 1024x1024x{}, loader {:.3}s",
            size.width,
            size.height,
            size.depth_or_array_layers,
            art.div_ceil(16),
            start.elapsed().as_secs_f64()
        );
    }
    let summary = format!(
        "PASS: {} imported layouts, all 16 base theaters and every imported variant; real mission loader through vegetation; default and downlevel texture/buffer/compute limits. No GPU creation.\n",
        catalog.len()
    );
    print!("{summary}");
    std::fs::write(out.join("limits-check.txt"), summary)?;
    Ok(())
}

#[cfg(test)]
mod grass_camera_tests {
    use super::*;
    #[test]
    fn vegetation_grass_cameras_require_actual_meadow_and_field_boundary() {
        let mut pixels = vec![10; 65536];
        for row in pixels.chunks_mut(256) {
            row[100..].fill(12);
            row[80..100].fill(20);
        }
        let atlas = crate::vegetation::Atlas::synthetic(pixels, 0);
        let mut world = tore_world::test_support::terrain();
        world.theater.cols = 5;
        world.theater.rows = 5;
        world.theater.cells = vec![
            tore_formats::theater::TerrainCell {
                color: 12,
                class: 2,
                elevation: 0
            };
            25
        ];
        let spot = crate::vegetation::Spot {
            x: 12288.,
            z: 4096.,
            heading: 35.,
            label: "meadow",
        };
        let meadow = grass_meadow(&atlas, &world, spot).unwrap();
        assert_eq!(
            atlas.site(&world, meadow.x, meadow.z).kind,
            crate::vegetation::Kind::Grass
        );
        let edge = grass_field_edge(&atlas, &world, spot).unwrap();
        assert_eq!(
            atlas.kind_at(edge.x as i64 - 8, edge.z as i64),
            crate::vegetation::Kind::Field
        );
        assert_eq!(
            atlas.kind_at(edge.x as i64 + 8, edge.z as i64),
            crate::vegetation::Kind::Grass
        );
    }
}

#[cfg(test)]
mod blade_pass_tests {
    use super::*;

    /// The grass candidate pass (one compute thread per candidate, then an
    /// indirect draw of the grown blades) draws the same blades as the
    /// vertex path, which evaluates every candidate in each vertex: over a
    /// real meadow, close and at the grass distance, both paths render the
    /// same picture to within floating-point noise, and the pass grew
    /// blades.
    fn blades(view: &mut View) -> &mut crate::vegetation::grass::gpu::Blades {
        view.sim.vegetation.as_mut().unwrap().blades_mut()
    }

    #[test]
    #[ignore = "requires a GPU adapter and TORE_DATA_DIR pointing to a full import"]
    fn gpu_candidate_pass_matches_the_vertex_path() {
        let assets = crate::reel::load_assets().unwrap();
        let resources = &assets.theater_resources;
        for code in ["UKR", "TVIET"] {
            let world =
                Terrain::for_mission(resources, code, Some(0), &overrides([12, 0]).unwrap())
                    .unwrap();
            let scenery = Scenery::build(resources, &world).unwrap();
            let atlas = crate::vegetation::Atlas::build(resources, &world)
                .unwrap()
                .unwrap();
            let spots = crate::vegetation::spots(&atlas, &world);
            let spot = spots
                .iter()
                .find(|s| s.label == "meadow" || s.label == "field")
                .expect("a meadow or field");
            let size = [640, 360];
            let mut view = pollster::block_on(View::new(
                &scenery,
                size,
                options(crate::graphics::VegDensity::High),
                false,
            ))
            .unwrap();
            for (height, pitch) in [(6., 12.), (30., 4.), (450., 40.)] {
                let mut scenery = Scenery::build(resources, &world).unwrap();
                let camera = look_at(&world, spot, &shot("grass", "meadow", height, pitch, 0.));
                scenery.resolve_palette(&world, camera.position[1]);
                scenery.set_origin(camera.position);
                blades(&mut view).set_vertex_only(false);
                let listed = view.pixels(&camera, &world, &scenery).unwrap();
                let grown = view
                    .sim
                    .vegetation
                    .as_ref()
                    .unwrap()
                    .blades()
                    .grown(&view.device, &view.queue);
                blades(&mut view).set_vertex_only(true);
                let direct = view.pixels(&camera, &world, &scenery).unwrap();
                let changed = listed
                    .chunks_exact(4)
                    .zip(direct.chunks_exact(4))
                    .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > 2))
                    .count();
                println!(
                    "{code} {height} ft: grown {grown:?}, {changed} pixels differ by more than 2"
                );
                assert!(grown.is_some_and(|n| n > 0) || height > 400.);
                assert!(
                    changed * 2000 < listed.len() / 4,
                    "{code} {height} ft: {changed} pixels differ"
                );
            }
        }
    }
}
