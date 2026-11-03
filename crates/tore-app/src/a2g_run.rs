//! `--a2g-run THEATER STEM --aircraft ID --load REC --attack KIND [--from NM]
//! [--altitude FT] [--speed KT] [--dive DEG] [--heading DEG] [--seconds S]
//! [--target N] [--station N] [--count N] [--hold S] [--repeat N] [--gap S]
//! [--defenses AAA SAM] [--surface-seed N] [--unit ID] [--designate HOW]
//! [--break-at S] [--skill N] [--render OUT.png] [--record PATH
//! [--verify-render]]`: a development aid and the battery's driver for
//! air-to-ground weapons (plan section 11.2). No window opens; the whole
//! mission tick runs as in the game. `--record PATH` records the run as a
//! mission recording and `--verify-render` reads it back and checks that
//! every tick redraws the picture the run drew (the battery's `replay-a2g`).
//!
//! The player's aircraft flies a scripted straight pass at a ground target of
//! the Quick Mission `STEM` describes, invulnerable, and attacks it through
//! the real player path: the weapon keys select the station and the trigger
//! releases, never a projectile placed by hand. Each line starts `a2g-run:`.
//!
//! `--load REC` names the store. When the aircraft carries it the run uses
//! that station; otherwise it is fitted to `--station N` (default: the first
//! station carrying a bomb or a rocket pod), with `--count N` stores, for the
//! run only, as a Cheat loading would, so records the catalog does not offer
//! yet can be flown. `--target N` picks the Nth ground target (default 0).
//!
//! Attack kinds, one submodule each (each weapon slice adds its own):
//!
//! - `release` ([`release`]): a level pass (`--dive 0`) or a dive along
//!   `--dive DEG`, released when the bomb sight's mean impact reaches the
//!   target (bombs), or one second in (rockets). `--hold S` keeps the trigger
//!   down that long, `--repeat N` presses N times `--gap S` apart.
//! - `cluster` ([`cluster`]): the `release` pass with a cluster bomb, then
//!   the splash on every surface target and the bomblet flashes.
//! - `laser`, `jdam`, `at2` ([`guided`]): the guided weapons, released on
//!   the HUD's release range cue, with `--laser-off S`, `--buddy ID`,
//!   `--turn-away S` and `--condition C` of their own.
//! - `maverick`, `harm`, `antiship` and `sarh` ([`missile`], slice W2): the
//!   air-to-ground missiles. `--designate HOW` picks the target through the
//!   player's controls (`ag`, `ir`, `harm`, `visual` or `none` for BORE),
//!   `--unit ID` names the target, `--break-at S` turns the A-G page off S
//!   seconds after launch, `--skill N` sets the target's (and its battery's)
//!   experience, and `--render OUT.png` draws the HUD at the release.
//! - `ccip` ([`ccip`]): a dive (or level) pass with a bomb, released when the
//!   HUD's own CCIP pipper covers the target.
//! - `level` ([`level`]): the A-G page designates the target, the bomb is
//!   selected (LVL) and the trigger is held through a level pass; combat
//!   releases at the "T"'s cue.
//! - `rocket` ([`rocket`]): a shallow dive with a pod, fired when the rocket
//!   pipper covers the target, held `--hold S`.
//! - `strafe` ([`strafe`]): the `release` pass with a store selected and Tab,
//!   the gun key, held `--hold S` (2 by default); the gun fires, the store
//!   stays on its station.
//! - `orbit` ([`orbit`], slice nr-orbit): the AC-130's sensor console
//!   opens and orbit hold flies the attack orbit with no stick or throttle,
//!   `--orbit-sight pin|track|free`, `--orbit-slew S`, `--orbit-close S`.
//!
//! `--pilot ai` ([`ai`], AI strike slice ST-D) takes no `--attack`: the AI
//! flies the aircraft and the driver only watches and prints `ai-strike:`
//! lines (`--profile`, `--handback S`, `--order KIND`).
use crate::{AppResult, surface_dump};
use tore_formats::{aircraft::AircraftId, weapons::Weapon};
use tore_sim::{
    attitude::Vector,
    combat::{
        ballistics,
        live::{Launcher, Ownship, Station},
    },
};
use tore_world::{
    mission::{Defense, MissionSpec},
    seats::{SeatCommand, SeatId, SeatInput},
    world::{Seating, TickOutput, World},
};

mod ai;
mod ai_jettison;
pub(crate) mod ai_render;
mod ccip;
mod cluster;
mod console_render;
mod guided;
pub mod guided_preview;
mod jettison;
mod level;
mod missile;
mod orbit;
pub mod preview;
mod release;
mod rocket;
mod scene_render;
mod strafe;

const SEAT: SeatId = SeatId(0);
pub(crate) const FEET_PER_NM: f64 = 6_076.;
pub(crate) const FPS_PER_KNOT: f64 = 1.687_8;

pub(crate) struct Options {
    theater: String,
    stem: String,
    aircraft: AircraftId,
    load: Option<String>,
    attack: String,
    pub from_nm: f64,
    pub altitude: f64,
    pub speed_kt: f64,
    pub dive_deg: f64,
    pub heading_deg: f64,
    pub seconds: f64,
    target: usize,
    station: Option<usize>,
    count: Option<u16>,
    pub hold: f64,
    pub repeat: usize,
    pub gap: f64,
    defenses: (usize, usize),
    seed: u32,
    unit: Option<u32>,
    pub designate: Option<String>,
    pub break_at: Option<f64>,
    pub skill: Option<i32>,
    pub render: Option<std::path::PathBuf>,
    record: Option<std::path::PathBuf>,
    verify: bool,
    /// `--scene-at S,...`: frames for `--render-scene DIR` on a player run
    /// ([`scene_render`]).
    scene_at: Vec<f64>,
    /// `--condition`: the mission's weather on a player run.
    condition: Option<tore_world::mission::Condition>,
    /// `--pilot ai` and its options ([`ai`]).
    ai: ai::Config,
}

const USAGE: &str = "--a2g-run THEATER STEM --aircraft ID --load REC --attack KIND [--from NM] [--altitude FT] [--speed KT] [--dive DEG] [--heading DEG] [--seconds S] [--target N] [--station N] [--count N] [--hold S] [--repeat N] [--gap S] [--defenses AAA SAM] [--surface-seed N] [--unit ID] [--designate ag|ir|harm|visual|none] [--break-at S] [--skill N] [--render OUT.png] [--record PATH [--verify-render]] [--laser-off S] [--buddy ID [--buddy-ai]] [--turn-away S] [--condition clear|cloudy|foggy|night] [--orbit-sight pin|track|free] [--orbit-slew S] [--orbit-close S] | --a2g-run THEATER STEM --pilot ai [--aircraft ID] [--load REC] [--target N] [--skill 0..3] [--profile straight|popup|fasthigh|dive|level|orbit|standoff|laydown|strafe] [--handback S [--mode ccip|lvl] [--trigger] [--laser] [--take-back S]] [--order KIND [--order-at S] [--order-class vehicles|tanks|structures|aaa|sams|ships] [--order-wingmen N] [--record PATH]] [--quick-wing [--clear-air-at S]] [--choose-store] [--buddy ID] [--render OUT.png --render-at S] [--mission-altitude FT] [--render-scene DIR] [--console-scene DIR --console-at S,...] [--condition clear|cloudy|foggy|night] [--offset FT] [--sead-at S [--sead-site ID]...]; player kinds also take [--render-scene DIR --scene-at S,...] [--condition clear|cloudy|foggy|night] [--from NM] [--altitude FT] [--speed KT] [--heading DEG] [--dive DEG] [--seconds S]";

fn options() -> AppResult<Options> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut o = Options {
        theater: String::new(),
        stem: String::new(),
        aircraft: AircraftId::A4E,
        load: None,
        attack: String::new(),
        from_nm: 4.,
        altitude: 5_000.,
        speed_kt: 450.,
        dive_deg: 0.,
        heading_deg: 0.,
        seconds: 90.,
        target: 0,
        station: None,
        count: None,
        hold: 0.,
        repeat: 1,
        gap: 0.5,
        defenses: (0, 0),
        seed: 1,
        unit: None,
        designate: None,
        break_at: None,
        skill: None,
        render: None,
        record: None,
        verify: false,
        scene_at: Vec::new(),
        condition: None,
        ai: ai::Config::new(),
    };
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut next = || -> AppResult<String> {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value").into())
        };
        match arg.as_str() {
            "--aircraft" => o.aircraft = AircraftId::parse(&next()?)?,
            "--load" => o.load = Some(record_name(&next()?)),
            "--attack" => o.attack = next()?,
            "--from" => o.from_nm = next()?.parse()?,
            "--altitude" => o.altitude = next()?.parse()?,
            "--speed" => o.speed_kt = next()?.parse()?,
            "--dive" => o.dive_deg = next()?.parse()?,
            "--heading" => o.heading_deg = next()?.parse()?,
            "--seconds" => o.seconds = next()?.parse()?,
            "--target" => o.target = next()?.parse()?,
            "--station" => o.station = Some(next()?.parse()?),
            "--count" => o.count = Some(next()?.parse()?),
            "--hold" => o.hold = next()?.parse()?,
            "--repeat" => o.repeat = next()?.parse()?,
            "--gap" => o.gap = next()?.parse()?,
            "--defenses" => o.defenses = (next()?.parse()?, next()?.parse()?),
            "--surface-seed" => o.seed = next()?.parse()?,
            // The guided kinds' own options, read by `guided`.
            "--buddy-ai" => {}
            "--condition" => {
                o.condition = Some(match next()?.to_ascii_lowercase().as_str() {
                    "clear" => tore_world::mission::Condition::Clear,
                    "cloudy" => tore_world::mission::Condition::Cloudy,
                    "foggy" => tore_world::mission::Condition::Foggy,
                    "night" => tore_world::mission::Condition::Night,
                    other => return Err(format!("unknown --condition {other}").into()),
                })
            }
            "--scene-at" => {
                o.scene_at = next()?
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()?
            }
            other if guided::OPTIONS.contains(&other) || orbit::OPTIONS.contains(&other) => {
                next()?;
            }
            "--unit" => o.unit = Some(parse_id(&next()?)?),
            "--designate" => o.designate = Some(next()?.to_ascii_lowercase()),
            "--break-at" => o.break_at = Some(next()?.parse()?),
            "--skill" => o.skill = Some(next()?.parse()?),
            "--render" => o.render = Some(next()?.into()),
            "--record" => o.record = Some(next()?.into()),
            "--verify-render" => o.verify = true,
            "--pilot" => o.ai.pilot(&next()?)?,
            "--profile" => o.ai.set_profile(&next()?)?,
            "--render-at" => o.ai.render_at = Some(next()?.parse()?),
            "--handback" => o.ai.handback = Some(next()?.parse()?),
            "--mode" => o.ai.set_mode(&next()?)?,
            "--trigger" => o.ai.trigger = true,
            "--laser" => o.ai.laser = true,
            "--take-back" => o.ai.take_back = Some(next()?.parse()?),
            "--choose-store" => o.ai.choose = true,
            "--order" => o.ai.set_order(&next()?)?,
            "--order-at" => o.ai.order_at = next()?.parse()?,
            "--order-wingmen" => o.ai.order_wingmen = Some(next()?.parse()?),
            "--order-class" => {
                o.ai.order_class =
                    Some(match next()?.as_str() {
                        "vehicles" => 0x0200,
                        "tanks" => 0x0400,
                        "structures" => 0x0100,
                        "aaa" => 0x0800,
                        "sams" => 0x1000,
                        "ships" => 0x2000,
                        _ => return Err(
                            "--order-class needs vehicles, tanks, structures, aaa, sams or ships"
                                .into(),
                        ),
                    });
            }
            "--quick-wing" => o.ai.quick_wing = true,
            "--clear-air-at" => o.ai.clear_air_at = Some(next()?.parse()?),
            "--mission-altitude" => o.ai.mission_altitude = Some(next()?.parse()?),
            "--render-scene" => o.ai.render_scene = Some(next()?.into()),
            "--console-scene" => o.ai.console_scene = Some(next()?.into()),
            "--console-at" => {
                o.ai.console_at = next()?
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<_, _>>()?
            }
            "--offset" => o.ai.offset_ft = next()?.parse()?,
            "--sead-at" => o.ai.sead_at = Some(next()?.parse()?),
            "--sead-after-shot" => o.ai.sead_after_shot = Some(next()?.parse()?),
            "--sead-site" => {
                let usage = "--sead-site needs a site id (as 0x50000010 or decimal)";
                let text = next()?;
                let id = match text.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| usage)?,
                    None => text.parse().map_err(|_| usage)?,
                };
                o.ai.sead_sites.push(id);
            }
            "--follow-threat" => o.ai.follow_threat = true,
            "--follow-egress" => o.ai.follow_egress = true,
            "--place-site" => {
                let usage =
                    "--place-site needs ID,BEARING_DEG,RANGE_FT (the id as 0x50000010 or decimal)";
                let text = next()?;
                let parts: Vec<&str> = text.split(',').collect();
                let [id, bearing, range] = parts[..] else {
                    return Err(usage.into());
                };
                let id = match id.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| usage)?,
                    None => id.parse().map_err(|_| usage)?,
                };
                let (bearing, range): (f64, f64) = (
                    bearing.parse().map_err(|_| usage)?,
                    range.parse().map_err(|_| usage)?,
                );
                if !(bearing.is_finite() && range.is_finite() && range >= 0.) {
                    return Err(usage.into());
                }
                o.ai.place_sites.push((id, bearing, range));
            }
            "--site-sight" => {
                let usage = "--site-sight needs ID,FEET (the id as 0x50000010 or decimal; feet of at least 256)";
                let text = next()?;
                let Some((id, feet)) = text.split_once(',') else {
                    return Err(usage.into());
                };
                let id = match id.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).map_err(|_| usage)?,
                    None => id.parse().map_err(|_| usage)?,
                };
                let feet: f64 = feet.parse().map_err(|_| usage)?;
                if !(feet.is_finite() && feet >= 256.) {
                    return Err(usage.into());
                }
                o.ai.site_sights.push((id, feet));
            }
            "--terrain-nodes" => {
                let radius: f64 = next()?.parse()?;
                if !(radius.is_finite() && radius > 0.) {
                    return Err("--terrain-nodes needs RADIUS_FT above zero".into());
                }
                o.ai.terrain_nodes = Some(radius);
            }
            "--terrain-map" => {
                let usage = "--terrain-map needs RADIUS_FT,STEP_FT (a step of at least 100 ft)";
                let text = next()?;
                let Some((radius, step)) = text.split_once(',') else {
                    return Err(usage.into());
                };
                let (radius, step): (f64, f64) = (
                    radius.parse().map_err(|_| usage)?,
                    step.parse().map_err(|_| usage)?,
                );
                if !(radius.is_finite() && step.is_finite() && radius > 0. && step >= 100.) {
                    return Err(usage.into());
                }
                o.ai.terrain_map = Some((radius, step));
            }
            other if other.starts_with("--") => return Err(format!("unknown {other}").into()),
            other => positional.push(other.to_owned()),
        }
    }
    let [theater, stem] = positional.as_slice() else {
        return Err(USAGE.into());
    };
    if o.attack.is_empty() && !o.ai.active {
        return Err(USAGE.into());
    }
    o.ai.check(o.skill, o.record.is_some() || o.verify)?;
    if o.ai.active && o.render.is_some() != o.ai.render_at.is_some() {
        return Err("with --pilot ai, --render OUT.png and --render-at S go together".into());
    }
    if o.verify && o.record.is_none() {
        return Err("--verify-render checks a --record run".into());
    }
    o.theater = theater.to_ascii_uppercase();
    o.stem = stem.trim_start_matches('~').to_ascii_uppercase();
    Ok(o)
}

/// A unit id, decimal or `0x` hexadecimal.
fn parse_id(text: &str) -> AppResult<u32> {
    Ok(match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16)?,
        None => text.parse()?,
    })
}

/// `mk82` or `MK82.JT` as the record's resource name.
fn record_name(name: &str) -> String {
    let upper = name.to_ascii_uppercase();
    if upper.ends_with(".JT") {
        upper
    } else {
        format!("{upper}.JT")
    }
}

pub fn run() -> AppResult<()> {
    let o = options()?;
    let mut resources = crate::reel::load_assets()?.theater_resources;
    surface_dump::with_retail_surface(&mut resources)?;
    if o.ai.active {
        return ai::run(&o, &resources);
    }
    if guided::KINDS.contains(&o.attack.as_str()) {
        return guided::run(&o, &resources);
    }
    let mut scene = Scene::new(&o, &resources)?;
    match o.attack.as_str() {
        "release" => release::run(&mut scene, &o)?,
        "jettison" => jettison::run(&mut scene, &o)?,
        "ai-jettison" => ai_jettison::run(&mut scene, &o, &resources)?,
        "cluster" => cluster::run(&mut scene, &o)?,
        kind @ ("maverick" | "harm" | "antiship" | "sarh" | "sarh-uncued") => {
            missile::run(&mut scene, &o, kind, &resources)?
        }
        "strafe" => {
            scene.print_record();
            strafe::run(&mut scene, &o)?
        }
        "orbit" => orbit::run(&mut scene, &o)?,
        "ccip" | "level" | "rocket" => {
            scene.print_record();
            match o.attack.as_str() {
                "ccip" => ccip::run(&mut scene, &o)?,
                "level" => level::run(&mut scene, &o)?,
                _ => rocket::run(&mut scene, &o)?,
            }
        }
        other => {
            return Err(format!(
                "unknown --attack {other}; kinds: release, cluster, ccip, level, rocket, strafe, orbit, maverick, harm, antiship, sarh, sarh-uncued, laser, jdam, at2"
            )
            .into());
        }
    }
    scene.finish()?;
    println!("a2g-run: done");
    Ok(())
}

/// The world, the player's attack station and the ground target.
pub(crate) struct Scene {
    pub world: World,
    out: TickOutput,
    commands: Vec<SeatCommand>,
    pub station: usize,
    pub target: u32,
    pub owner: u32,
    /// The run as a mission recording, when `--record` asks.
    record: Option<crate::surface_objective::Record>,
    /// The frames `--render-scene` and `--scene-at` ask for.
    scenes: scene_render::Plan,
    /// What the run's motors did, for the `motors` line.
    motors: scene_render::Motors,
}

impl Scene {
    fn new(
        o: &Options,
        resources: &std::collections::BTreeMap<String, Vec<u8>>,
    ) -> AppResult<Self> {
        let mut spec = MissionSpec::new(&o.theater, o.aircraft);
        spec.ground_target = Some(o.stem.clone());
        if o.attack == "ai-jettison" {
            spec.wings[0].count = 2;
            spec.wings[3].aircraft = tore_formats::aircraft::AircraftId::Mig29;
            spec.wings[3].count = 1;
            spec.separation_nm = 50;
        }

        spec.aaa = Defense::from_level(o.defenses.0).ok_or("defense levels are 0 to 3")?;
        spec.sam = Defense::from_level(o.defenses.1).ok_or("defense levels are 0 to 3")?;
        spec.surface_seed = o.seed;
        spec.cheats.damage = tore_sim::cheats::Damage::Invulnerable;
        if let Some(condition) = o.condition {
            spec.condition = condition;
        }
        spec.validate()?;
        let mut world =
            crate::surface_dump::world_without_front(&spec, resources, Seating::SinglePlayer)?;
        if let Some(why) = &world.terrain.surface.unresolved {
            return Err(format!("the ground target stands nowhere: {why}").into());
        }
        let owner = world.combat.own_id();
        let targets = tore_world::ai_wings::outcome::ground_targets(
            &world.terrain.surface,
            &world.combat.state,
        );
        let target = match o.unit {
            Some(id) => id,
            None => *targets
                .get(o.target)
                .ok_or_else(|| format!("{} has {} ground targets", o.stem, targets.len()))?,
        };
        let station = fit_store(&mut world, owner, o, resources)?;
        let scene = Self {
            world,
            out: TickOutput::default(),
            commands: Vec::new(),
            station,
            target,
            owner,
            record: None,
            scenes: scene_render::Plan::new(
                o.ai.render_scene.clone().filter(|_| !o.ai.active),
                &o.scene_at,
            ),
            motors: Default::default(),
        };
        let scene = scene.recording(o)?;
        let row = scene.target_row().ok_or("the target has no row")?;
        let own = scene.world.combat.state.own();
        let s = &own.configuration().stations[station];
        println!(
            "a2g-run: setup aircraft {:?} station {station} {} count {} rounds {} target {target:#010x} at {:.0} {:.0} {:.0} hp {}",
            o.aircraft,
            s.weapon.source,
            s.count,
            own.rounds(station),
            row.position[0],
            row.position[1],
            row.position[2],
            row.hp
        );
        if let Some(b) = scene.world.combat.state.ground_bounds(target) {
            println!(
                "a2g-run: target box center {:.0} {:.0} {:.0} half {:.1} {:.1} {:.1}",
                b.center[0], b.center[1], b.center[2], b.half[0], b.half[1], b.half[2]
            );
        }
        Ok(scene)
    }

    /// Starts the recording `--record` asks for, with the scene as it is.
    fn recording(mut self, o: &Options) -> AppResult<Self> {
        if o.attack != "ai-jettison"
            && let Some(path) = &o.record
        {
            let plane = o.aircraft.label().to_owned();
            self.record = Some(crate::surface_objective::Record::start(
                &self.world,
                path,
                o.verify,
                &plane,
                "the air-to-ground run",
            )?);
        }
        Ok(self)
    }

    /// Runs `step` as one recorded tick: the recorder sees the combat before
    /// and the picture after.
    fn recorded(
        &mut self,
        step: impl FnOnce(&mut World, &mut TickOutput) -> AppResult<()>,
    ) -> AppResult<()> {
        if let Some(record) = &mut self.record {
            record.before(&mut self.world);
        }
        step(&mut self.world, &mut self.out)?;
        if let Some(record) = &mut self.record {
            record.after(&mut self.world, &self.out);
        }
        Ok(())
    }

    pub fn target_row(&self) -> Option<&tore_sim::combat::live::Target> {
        self.world
            .combat
            .state
            .targets
            .iter()
            .find(|t| t.id == self.target)
    }

    pub fn station(&self) -> &Station {
        &self.world.combat.state.own().configuration().stations[self.station]
    }

    pub fn rounds(&self) -> u16 {
        self.world.combat.state.own().rounds(self.station)
    }

    pub fn ground(&self, x: f64, z: f64) -> f64 {
        f64::from(self.world.terrain.height(x as f32, z as f32))
    }

    /// The launcher combat takes from the cockpit now.
    pub fn launcher(&self) -> Launcher {
        tore_world::combat::launcher(&self.world.cockpits[0].flight)
    }

    pub fn seconds(&self) -> f64 {
        self.world.combat.state.tick() as f64 / 120.
    }

    /// Puts the aircraft at `position`, its nose and velocity along
    /// `heading` and `pitch` (radians), at `speed` ft/s.
    pub fn fly(&mut self, position: Vector, heading: f64, pitch: f64, speed: f64) {
        let flight = &mut self.world.cockpits[0].flight;
        let forward = [
            heading.sin() * pitch.cos(),
            pitch.sin(),
            heading.cos() * pitch.cos(),
        ];
        flight.position = position;
        flight.velocity = forward.map(|v| v * speed);
        flight.speed = speed;
        flight.yaw = heading;
        flight.pitch = pitch;
        flight.bank = 0.;
        flight.vertical_speed = forward[1] * speed;
        flight.roll_rate = 0.;
        flight.pitch_rate = 0.;
    }

    /// One mission tick with the trigger as given and the queued commands.
    pub fn step(&mut self, trigger: bool) -> AppResult<()> {
        let input = SeatInput {
            seat: SEAT,
            tick: self.world.tick(),
            trigger,
            commands: std::mem::take(&mut self.commands),
            ..SeatInput::default()
        };
        self.step_input(input)
    }

    /// One mission tick with the seat's whole input.
    fn step_input(&mut self, input: SeatInput) -> AppResult<()> {
        self.recorded(|world, out| world.step(std::slice::from_ref(&input), out))?;
        scene_render::after_step(self)
    }

    /// Queues a seat command for the next step.
    pub fn queue(&mut self, command: SeatCommand) {
        self.commands.push(command);
    }

    /// The attack record's fuze, splash and range, which the sight's
    /// warnings and arc read.
    fn print_record(&self) {
        let w = &self.station().weapon;
        println!(
            "a2g-run: record {} fuze-arm {:.2} s fuze-radius {} collateral {} zone1-max {} name {}",
            w.source,
            f64::from(w.damage.fuze_arm_t) / 4.,
            w.damage.fuze_radius,
            w.damage.collateral_radius,
            w.seeker.zones[1].maximum_range,
            w.hud_name
        );
        println!(
            "a2g-run: target {:#010x} {}",
            self.target,
            self.world
                .combat
                .ground_name(self.target)
                .unwrap_or("unnamed")
        );
    }

    /// Whether the attack station is selected and armed.
    pub fn selected(&self) -> bool {
        let own = self.world.combat.state.own();
        own.armed && own.selected == self.station
    }

    /// Queues the weapon key for the next step until the station is chosen.
    pub fn select(&mut self) {
        if !self.selected() && self.commands.is_empty() {
            self.commands
                .push(SeatCommand::CycleWeapon { forward: true });
        }
    }

    /// Prints the kills and the target, and finishes the recording.
    fn finish(&mut self) -> AppResult<()> {
        println!("{}", self.motors.line());
        let state = &self.world.combat.state;
        for kill in state
            .ledger
            .kills()
            .iter()
            .filter(|k| k.owner == self.owner)
        {
            println!(
                "a2g-run: kill {:#010x} class {:#06x}",
                kill.victim, kill.category
            );
        }
        if let Some(row) = self.target_row() {
            println!(
                "a2g-run: target {:#010x} hp {}/{}",
                row.id, row.hp, row.initial_hp
            );
        }
        if let Some(record) = self.record.take() {
            let report =
                tore_world::debrief::capture(&self.world, SEAT).ok_or("the seat flies no plane")?;
            for line in record.finish(&self.world, &report)? {
                println!("a2g-run: {line}");
            }
        }
        Ok(())
    }
}

/// The station the run attacks with, the store fitted to it when the
/// aircraft does not carry it (an agent decision for this development
/// driver: records the catalog does not list yet are flown as a Cheat
/// loading would carry them).
fn fit_store(
    world: &mut World,
    owner: u32,
    o: &Options,
    resources: &std::collections::BTreeMap<String, Vec<u8>>,
) -> AppResult<usize> {
    let state = &mut world.combat.state;
    let own = state.ownship(owner).ok_or("no ownship")?;
    let mut config = own.configuration().clone();
    let carried = o.load.as_ref().and_then(|name| {
        config
            .stations
            .iter()
            .position(|s| s.weapon.source.eq_ignore_ascii_case(name))
    });
    // The row retail loads this store on by default (the A-4E's Mk 82s,
    // whose centreline row carries a tank in T.O.R.E's default load since
    // PY3).
    let record = tore_formats::aircraft::Aircraft::parse(
        resources
            .get(o.aircraft.pt())
            .ok_or("missing aircraft record")?,
    )?;
    let retail = o.load.as_ref().and_then(|name| {
        let hardpoint = record.hardpoints.iter().position(|h| {
            h.store
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(name))
        })?;
        *config.hardpoint_slots.get(hardpoint)?
    });
    let station = match (carried, o.station, retail) {
        (_, Some(station), _) => station,
        (_, None, Some(station)) => station,
        (Some(station), None, _) => station,
        (None, None, _) => config
            .stations
            .iter()
            .position(|s| {
                ballistics::is_bomb(&s.weapon)
                    || ballistics::is_pod(&s.weapon)
                    || missile::air_to_ground(&s.weapon)
            })
            .ok_or("the aircraft has no air-to-ground station; give --station N")?,
    };
    // `--station N` one past the last adds an internal bay there, for an
    // aircraft with no ground station (a heavy for the AI strike driver's
    // level run, slice ST-T2); it needs `--load`.
    let added = station == config.stations.len() && o.load.is_some();
    if added {
        let name = o.load.as_deref().expect("checked");
        let bytes = resources
            .get(name)
            .ok_or_else(|| format!("missing {name}"))?;
        config.stations.push(tore_sim::combat::live::Station {
            weapon: Weapon::parse(name, bytes)?,
            mount: [0., -4., 0.],
            count: o.count.unwrap_or(1),
            internal: true,
            rack: Default::default(),
        });
        println!("a2g-run: added station {station} (an internal bay)");
    }
    if station >= config.stations.len() {
        return Err(format!("the aircraft has {} stations", config.stations.len()).into());
    }
    // An empty row (the A-4E's centreline Mk 82 row, a tank there by
    // default since PY3) given no `--count` carries what retail loads on it,
    // in place of its tank.
    let mut retail_row = false;
    if o.count.is_none()
        && !added
        && config.stations[station].count == 0
        && let Some(hardpoint) = config
            .hardpoint_slots
            .iter()
            .position(|s| *s == Some(station))
        && let Some(h) = record
            .hardpoints
            .get(hardpoint)
            .filter(|h| h.store.as_deref().is_some_and(|n| n.ends_with(".JT")))
    {
        for tank in config.tanks.iter_mut().filter(|t| t.hardpoint == hardpoint) {
            tank.quantity = 0;
        }
        config.refresh_tanks()?;
        config.stations[station].count = u16::try_from(h.count).unwrap_or(0);
        retail_row = true;
    }
    let fitted = match &o.load {
        Some(name)
            if !config.stations[station]
                .weapon
                .source
                .eq_ignore_ascii_case(name) =>
        {
            let bytes = resources
                .get(name)
                .ok_or_else(|| format!("missing {name}"))?;
            let mut weapon = Weapon::parse(name, bytes)?;
            tore_sim::combat::gunship::apply_tore_record(&mut weapon);
            config.stations[station].weapon = weapon;
            true
        }
        _ => false,
    };
    if let Some(count) = o.count {
        config.stations[station].count = count;
    }
    if fitted || added || retail_row || o.count.is_some() {
        let (side, friendlies) = (own.side, own.friendlies.clone());
        state.remove_ownship(owner);
        let mut own = Ownship::new(owner, side, config, true)?;
        own.friendlies = friendlies;
        state.add_ownship(own)?;
        println!("a2g-run: fitted station {station}");
    }
    Ok(station)
}
