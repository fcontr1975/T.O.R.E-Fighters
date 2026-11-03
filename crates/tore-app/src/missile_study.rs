//! `--missile-study OUT_DIR [--seeds N] [--missiles A,B] [--profiles P,Q]
//! [--ranges near,close,short,medium,long,defeat-edge] [--aspects head,quarter,beam,tail] [--altitudes
//! deck,low,co,high,up,down] [--shooter ID] [--target ID] [--threads N] [--decoy-model
//! coast|lure] [--ai [--skill S] [--ai-model standard|hybrid] [--theater T]
//! [--trace]] [--catalog]`: a headless Monte Carlo of air-to-air missile shots, for studying missile accuracy and
//! countermeasures (docs/DEVELOPMENT.md, "Missile study"). No window opens
//! and nothing is drawn. Every shot is deterministic for its seed.
//!
//! Each shot builds a fresh combat state with the shooter's imported aircraft,
//! the study missile on its first missile station, and one target row with
//! the target aircraft's imported signatures, hit points and ECM record. The
//! shooter flies straight and level at 0.9 Mach, designates the target
//! through its own radar, waits for the mounted IR seeker where there is one,
//! and fires through the ordinary player trigger. From there the real combat
//! step flies the missile: motor, steering, seeker, notch, SARH support, fuze
//! and damage.
//!
//! The target is scripted, not flown by the AI: straight and level, or a
//! constant-G level break, beam or drag, and it dispenses flares (against
//! infrared seekers) or chaff (against radar seekers) on the AI's burst
//! program or at a hammered rate. Energy mode uses the live physical device
//! scene and collects its actual episode rolls. `--missile-model spec` retains
//! the older per-device evaluator for comparison. Geometry is jittered per
//! seed independently from the explicit gameplay decoy seed. `--ai` flies
//! against the existing AI; its defensive schedule is not modified here.
//! `--decoy-model lure` is an old Spec-only what-if, not Energy behaviour.
//!
//! Output: `runs.csv` (one row per shot), `cells.csv` (one row per cell with
//! Wilson 95 percent intervals) and, with `--catalog`, `catalog.csv` (each
//! missile's decoy susceptibility, seeker and turn rates and each aircraft's
//! dispenser record).
use crate::AppResult;
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tore_formats::{aircraft::AircraftId, weapons::Weapon};
use tore_sim::{
    ai::{
        DecisionRandom,
        threat::{self, DecoyOutcome, GuidingMissile, SeekerClass},
    },
    attitude::{Basis, Vector, dot},
    combat::{
        ledger::Resolution,
        live::{self, Command, Configuration, Event, Launcher, OwnshipInput},
        missiles::{self, Guidance, Profile, seeker::Status},
    },
    sensors,
};
use tore_workers::Executor;

mod ai;

const HZ: f64 = 120.;
const DT: f64 = 1. / HZ;
const G: f64 = 32.174;
const NMI: f64 = 6076.;
/// The AI's burst: two devices a quarter second apart, no sooner than two
/// seconds after the last burst began, once the threat is 6 seconds out
/// (`tore_sim::ai::defense`).
const AI_BURST: usize = 2;
const AI_BURST_GAP_TICKS: u64 = 30;
const AI_BURST_INTERVAL_TICKS: u64 = 240;
const AI_RELEASE_WINDOW_S: f64 = 6.;
/// A player hammering the key: four devices a second from one second after
/// launch until the dispenser is empty.
const HAMMER_GAP_TICKS: u64 = 30;
/// Every IR and radar air-to-air store with a reviewed profile, oldest
/// generation first within each class.
pub const MISSILES: [&str; 21] = [
    "AIM9B.JT",
    "AA2.JT",
    "AA8.JT",
    "R550.JT",
    "AIM9M.JT",
    "AA11.JT",
    "AA11B.JT",
    "AIM9X.JT",
    "R27T.JT",
    "R27ET.JT",
    "AIM7E.JT",
    "AIM7.JT",
    "R530.JT",
    "AA10.JT",
    "R27ER.JT",
    "AIM120.JT",
    "AA12.JT",
    "MICA.JT",
    "AIM54C.JT",
    "AAML.JT",
    "AEMP1.JT",
];

/// Speed of sound in the standard troposphere, feet per second.
pub fn sound_fps(altitude_ft: f64) -> f64 {
    let kelvin = 288.15 - 0.0019812 * altitude_ft.clamp(0., 36_089.);
    (1.4 * 287.053 * kelvin).sqrt() / 0.3048
}

/// Wilson score interval for `hits` of `n` at 95 percent.
pub fn wilson(hits: usize, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0., 1.);
    }
    let (n, p, z) = (n as f64, hits as f64 / n as f64, 1.96_f64);
    let centre = (p + z * z / (2. * n)) / (1. + z * z / n);
    let half = z * (p * (1. - p) / n + z * z / (4. * n * n)).sqrt() / (1. + z * z / n);
    ((centre - half).max(0.), (centre + half).min(1.))
}

/// SplitMix64: per-seed jitter, independent of every gameplay stream.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}
fn uniform(seed: u64, stream: u64, lo: f64, hi: f64) -> f64 {
    let u = (mix(seed ^ mix(stream)) >> 11) as f64 / (1_u64 << 53) as f64;
    lo + (hi - lo) * u
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Aspect {
    Quarter,
    Head,
    Beam,
    Tail,
}
impl Aspect {
    fn name(self) -> &'static str {
        match self {
            Self::Quarter => "quarter",
            Self::Head => "head",
            Self::Beam => "beam",
            Self::Tail => "tail",
        }
    }
    fn parse(s: &str) -> AppResult<Self> {
        Ok(match s {
            "quarter" | "45" => Self::Quarter,
            "head" => Self::Head,
            "beam" => Self::Beam,
            "tail" => Self::Tail,
            _ => return Err(format!("unknown aspect {s}").into()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Altitudes {
    Low,
    High,
    Deck,
    /// Both at 15,000 ft.
    Co,
    /// Shooter at 5,000 ft, target at 20,000 ft.
    Up,
    /// Shooter at 20,000 ft, target at 3,000 ft over flat ground.
    Down,
}
impl Altitudes {
    fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
            Self::Deck => "deck",
            Self::Co => "co",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
    fn feet(self) -> (f64, f64) {
        match self {
            Self::Low => (5_000., 5_000.),
            Self::High => (35_000., 35_000.),
            Self::Deck => (500., 500.),
            Self::Co => (15_000., 15_000.),
            Self::Up => (5_000., 20_000.),
            Self::Down => (20_000., 3_000.),
        }
    }
    fn parse(s: &str) -> AppResult<Self> {
        Ok(match s {
            "low" => Self::Low,
            "high" => Self::High,
            "deck" => Self::Deck,
            "co" => Self::Co,
            "up" => Self::Up,
            "down" => Self::Down,
            _ => return Err(format!("unknown altitude case {s}").into()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Band {
    /// Stationary-target shot just beyond the shared post-burn defeat edge.
    DefeatEdge,
    Near,
    Close,
    Short,
    Medium,
    Long,
}
impl Band {
    fn name(self) -> &'static str {
        match self {
            Self::DefeatEdge => "defeat-edge",
            Self::Near => "near",
            Self::Close => "close",
            Self::Short => "short",
            Self::Medium => "medium",
            Self::Long => "long",
        }
    }
    fn parse(s: &str) -> AppResult<Self> {
        Ok(match s {
            "defeat-edge" => Self::DefeatEdge,
            "near" => Self::Near,
            "close" => Self::Close,
            "short" => Self::Short,
            "medium" => Self::Medium,
            "long" => Self::Long,
            _ => return Err(format!("unknown range band {s}").into()),
        })
    }
    /// The nominal horizontal launch range for a weapon, feet: near/close are
    /// 0.25/0.5 nmi, both raised to 1.1 times the decoded minimum. Short is
    /// 1.5 nmi (or just past the record's minimum), medium is 5 nmi for a
    /// radar missile (clamped to 60 percent of its maximum) and two thirds of
    /// an IR missile's maximum, long is 90 percent of the rated maximum.
    pub fn feet(self, w: &Weapon) -> f64 {
        let zone = &w.seeker.zones[1];
        let max = f64::from(zone.maximum_range);
        let min = f64::from(zone.minimum_range);
        let infrared = w.seeker.signature == 2;
        match self {
            Self::DefeatEdge => (1.5 * NMI).max(min * 1.1),
            Self::Near => (0.25 * NMI).max(min * 1.1),
            Self::Close => (0.5 * NMI).max(min * 1.1),
            Self::Short => (1.5 * NMI).max(min * 1.1).min(max * 0.5),
            Self::Medium if infrared => max * 0.65,
            Self::Medium => (5. * NMI).min(max * 0.6).max(min * 1.5),
            Self::Long => max * 0.9,
        }
    }
}

/// What the target does about the shot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Maneuver {
    /// Descend at 25 degrees to 100 feet, then level.
    Dive,
    /// Straight and level.
    None,
    /// A 7 G level break that puts the missile on the beam, 3 to 5 seconds
    /// before impact.
    Break,
    /// A 5 G turn to put the radar source (SARH: the shooter; others: the
    /// missile) on the beam, from 1 to 3 seconds after launch, held.
    Beam,
    /// A 5 G turn away from the missile, from 1 to 3 seconds after launch,
    /// held.
    Drag,
    /// The game's AI flies and dispenses (`--ai`).
    Ai,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Devices {
    None,
    /// The AI's burst program.
    Ai,
    /// Four devices a second from one second after launch.
    Hammer,
    /// One device when the closest approach is this many seconds away.
    Single(u8),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Tactic {
    pub maneuver: Maneuver,
    pub devices: Devices,
    /// Target engine heat: 0 idle, 1 military, 2 afterburner.
    pub heat: u8,
}
impl Tactic {
    pub fn name(self) -> String {
        let maneuver = match self.maneuver {
            Maneuver::Dive => "dive",
            Maneuver::None => "",
            Maneuver::Break => "break",
            Maneuver::Beam => "beam",
            Maneuver::Drag => "drag",
            Maneuver::Ai => "ai",
        };
        let devices = match self.devices {
            Devices::None => String::new(),
            Devices::Ai => "cm-ai".into(),
            Devices::Hammer => "cm-hammer".into(),
            Devices::Single(s) => format!("cm-at{s}s"),
        };
        let mut name = [maneuver, devices.as_str()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("+");
        if name.is_empty() {
            name = "straight".into();
        }
        match self.heat {
            0 => name + "@idle",
            2 => name + "@burner",
            3 => name + "@off",
            _ => name,
        }
    }
    pub fn parse(s: &str) -> AppResult<Self> {
        let (body, heat) = match s.split_once('@') {
            Some((b, "idle")) => (b, 0),
            Some((b, "off")) => (b, 3),
            Some((b, "burner")) => (b, 2),
            Some((_, other)) => return Err(format!("unknown heat {other}").into()),
            None => (s, 1),
        };
        let mut t = Self {
            maneuver: Maneuver::None,
            devices: Devices::None,
            heat,
        };
        for part in body.split('+') {
            match part {
                "straight" => {}
                "break" => t.maneuver = Maneuver::Break,
                "beam" => t.maneuver = Maneuver::Beam,
                "drag" => t.maneuver = Maneuver::Drag,
                "dive" => t.maneuver = Maneuver::Dive,
                "cm-ai" => t.devices = Devices::Ai,
                "cm-hammer" => t.devices = Devices::Hammer,
                "ai" => t.maneuver = Maneuver::Ai,
                other => match other
                    .strip_prefix("cm-at")
                    .and_then(|r| r.strip_suffix('s'))
                {
                    Some(n) => t.devices = Devices::Single(n.parse()?),
                    None => return Err(format!("unknown tactic part {other}").into()),
                },
            }
        }
        Ok(t)
    }
}

/// One cell of the study.
#[derive(Clone, Debug)]
pub struct Cell {
    pub missile: String,
    pub altitudes: Altitudes,
    pub band: Band,
    pub aspect: Aspect,
    pub tactic: Tactic,
}

/// What one shot did.
#[derive(Clone, Debug, Default)]
pub struct Shot {
    pub range_ft: f64,
    pub fired: bool,
    pub locked_at_launch: bool,
    pub outcome: &'static str,
    pub hit: bool,
    pub kill: bool,
    pub miss_ft: f64,
    pub time_s: f64,
    pub devices: u32,
    pub rolls: u32,
    pub control_loss_s: Option<f64>,
    pub defeat_s: Option<f64>,
    pub closest_inert_ft: Option<f64>,
    pub inert_radius_ft: f64,
    pub inert_damage: i32,
    pub ground_impact: bool,
    pub first_notch_s: Option<f64>,
    pub notched_s: f64,
    pub decoyed_at_s: Option<f64>,
    /// The closest approach was this many seconds away when the decoy took.
    pub decoyed_tcpa_s: Option<f64>,
    pub speed_at_end_fps: f64,
    pub target_speed_fps: f64,
    pub status_at_end: &'static str,
    pub first_lock_s: Option<f64>,
    pub tracked_s: f64,
    /// The AI target entered a defensive activity (`--ai` only).
    pub defended: bool,
    /// When the target released its first device, seconds after launch.
    pub first_device_s: Option<f64>,
    /// How far the target turned after the launch, degrees, all turns added.
    pub target_turn_deg: f64,
    /// Devices of the other class (chaff against an IR missile, flares
    /// against a radar one).
    pub other_devices: u32,
}

/// The aircraft and missile records a cell needs, loaded once.
pub struct Base {
    pub rules: missiles::Rules,
    /// What-if, not game behaviour: a decoyed missile steers for the device
    /// that decoyed it and bursts there, instead of flying straight on
    /// (`--decoy-model lure`).
    pub follow_decoy: bool,
    pub shooter: Configuration,
    pub target: Configuration,
    pub station: usize,
    pub weapons: BTreeMap<String, Weapon>,
}

/// The target's scripted flight.
struct Flyer {
    position: Vector,
    heading: f64,
    speed: f64,
    bank: f64,
    pitch: f64,
}
impl Flyer {
    fn forward(&self) -> Vector {
        [
            self.heading.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.heading.cos() * self.pitch.cos(),
        ]
    }
    fn velocity(&self) -> Vector {
        self.forward().map(|v| v * self.speed)
    }
    /// Turns toward `desired` heading at no more than `g` load, level.
    fn turn_toward(&mut self, desired: f64, g: f64) {
        let rate = G * (g * g - 1.).max(0.).sqrt() / self.speed.max(1.);
        let error = wrap(desired - self.heading);
        let step = error.clamp(-rate * DT, rate * DT);
        self.heading = wrap(self.heading + step);
        // Bank for the turn being flown, right wing down for a right turn.
        let pulled = if error.abs() > 1e-4 {
            (1. / g).acos()
        } else {
            0.
        };
        self.bank = pulled * error.signum();
    }
    fn advance(&mut self) {
        let v = self.velocity();
        for (p, v) in self.position.iter_mut().zip(v) {
            *p += v * DT;
        }
    }
}
fn wrap(a: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    (a + std::f64::consts::PI).rem_euclid(tau) - std::f64::consts::PI
}
/// Of the two headings that put `bearing` on the beam, the nearer to
/// `heading`.
fn beam_of(bearing: f64, heading: f64) -> f64 {
    let half = std::f64::consts::FRAC_PI_2;
    let (a, b) = (wrap(bearing + half), wrap(bearing - half));
    if wrap(a - heading).abs() <= wrap(b - heading).abs() {
        a
    } else {
        b
    }
}
fn heading_of(v: Vector) -> f64 {
    v[0].atan2(v[2])
}
fn sub(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] - b[i])
}
fn length(v: Vector) -> f64 {
    dot(v, v).sqrt()
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Unguided => "dumb",
        Status::Midcourse => "midcourse",
        Status::Search => "search",
        Status::Acquiring => "acquiring",
        Status::Locked => "locked",
        Status::Pitbull => "pitbull",
        Status::Memory => "memory",
        Status::Lost => "lost",
        Status::Expired => "expired",
    }
}

/// Flies one shot of `cell` with `seed`.
pub fn fly(base: &Base, cell: &Cell, seed: u64) -> AppResult<Shot> {
    let weapon = base
        .weapons
        .get(&cell.missile)
        .ok_or_else(|| format!("{} is not imported", cell.missile))?;
    let profile =
        Profile::for_weapon(weapon).ok_or_else(|| format!("{} has no profile", cell.missile))?;
    let mut config = base.shooter.clone();
    config.stations[base.station].weapon = weapon.clone();
    config.stations[base.station].count = 2;
    let mut state = live::State::new(config, true)?;
    state.weapon_rules = base.rules;
    state.energy_predictions_enabled = false;
    state.set_energy_seed(seed);
    let serial = Executor::serial();
    let ground = |_: f64, _: f64| 0.;
    let step = |state: &mut live::State, l: Launcher, held: bool| {
        state.step_rewound_with_executor(
            &[OwnshipInput {
                aircraft: 0,
                held,
                launcher: l,
            }],
            &[],
            ground,
            |_, _| false,
            &serial,
        )
    };
    state.own_mut().selected = base.station;
    let (shooter_alt, target_alt) = cell.altitudes.feet();
    let shooter_speed = 0.9 * sound_fps(shooter_alt);
    let basis = Basis::new(0., 0., 0.);
    let mut l = Launcher {
        position: [0., shooter_alt, 0.],
        basis,
        speed_fps: shooter_speed,
        velocity: [0., 0., shooter_speed],
        bay_ready: true,
        radar_power: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: sensors::Controls::default(),
    };
    state.range_target(0, l);
    // Per-seed geometry.
    let mut range = cell.band.feet(weapon) * uniform(seed, 1, 0.93, 1.07);
    let aspect_jitter = if cell.band == Band::DefeatEdge {
        0.
    } else {
        uniform(seed, 2, -10., 10.).to_radians()
    };
    let side = if uniform(seed, 3, 0., 1.) < 0.5 {
        1.
    } else {
        -1.
    };
    let target_speed = if cell.band == Band::DefeatEdge {
        0.
    } else {
        0.8 * sound_fps(target_alt) * uniform(seed, 4, 0.95, 1.05)
    };
    let reaction_s = uniform(seed, 5, 1., 3.);
    let break_at_s = uniform(seed, 6, 3., 5.);
    let mut target = Flyer {
        position: [0., target_alt, range],
        heading: wrap(
            match cell.aspect {
                Aspect::Quarter => side * std::f64::consts::FRAC_PI_4,
                Aspect::Head => std::f64::consts::PI,
                Aspect::Beam => side * std::f64::consts::FRAC_PI_2,
                Aspect::Tail => 0.,
            } + aspect_jitter,
        ),
        speed: target_speed,
        bank: 0.,
        pitch: 0.,
    };
    let target_id = state.targets[0].id;
    let heat = match cell.tactic.heat {
        0 => (0., false),
        2 => (1., true),
        _ => (1., false),
    };
    {
        let t = &mut state.targets[0];
        let c = &base.target;
        t.aircraft = Some(c.aircraft);
        t.signature = c.sensors.signature;
        t.jammer = c.sensors.jammer.clone();
        t.hp = c.hit_points;
        t.initial_hp = c.hit_points;
        t.fragment_offsets = c.fragment_offsets;
        t.wreck_power = c.wreck_power;
        t.category = c.target_category;
        t.heat = missiles::seeker::Heat::Engine {
            on: true,
            throttle: heat.0,
            afterburner: heat.1,
        };
    }
    if cell.band == Band::DefeatEdge {
        if base.rules != missiles::Rules::Energy {
            return Err("defeat-edge requires Energy flight".into());
        }
        let trial = |distance: f64| {
            missiles::prediction::launch_question(
                weapon,
                l,
                missiles::seeker::Observation {
                    id: target_id,
                    position: [0., target_alt, distance],
                    velocity: [0.; 3],
                    range: distance,
                    quality: 1.,
                    off_axis: 0.,
                },
                state.targets[0].radius + f64::from(weapon.damage.fuze_radius.max(0)),
                [0.; 3],
            )
        };
        let hit = |distance| trial(distance).finish(&|_, _| 0.).hit;
        let mut low = 2000.;
        let mut high = low;
        if !hit(low) {
            return Err("defeat-edge has no short reference intercept".into());
        }
        while hit(high) {
            low = high;
            high *= 1.5;
            if high > 3_000_000. {
                return Err("defeat-edge failed to find an endpoint".into());
            }
        }
        for _ in 0..28 {
            let mid = (low + high) * 0.5;
            if hit(mid) {
                low = mid;
            } else {
                high = mid;
            }
        }
        range = high + 1.;
        let mut beyond = trial(range);
        while beyond.answer.is_none() {
            beyond.step(&|_, _| 0.);
        }
        if !beyond.flight.defeated {
            return Err("the selected edge is expiry/ground, not inert defeat".into());
        }
        target.position[2] = range;
    }
    let place = |state: &mut live::State, target: &Flyer| {
        let t = &mut state.targets[0];
        t.position = target.position;
        t.velocity = target.velocity();
        t.basis = Basis::new(target.heading, target.pitch, target.bank);
    };
    place(&mut state, &target);
    step(&mut state, l, false);
    state.command(0, Command::DesignateTarget(target_id), l);
    // Let the radar settle on the track and, for an IR store, the mounted
    // seeker lock, both with the geometry held still.
    let mut locked = false;
    for tick in 0..360 {
        place(&mut state, &target);
        step(&mut state, l, false);
        let view = state.own_view();
        let tracked = view.designated() == Some(target_id);
        let seeker_ok = match profile.guidance {
            Guidance::Infrared => view.seeker_tone(l).is_some_and(|t| t.locked),
            _ => view.ownship().sensors.supports(target_id),
        };
        if tracked && seeker_ok {
            locked = true;
            if tick >= 90 {
                break;
            }
        }
    }
    let mut shot = Shot {
        range_ft: range,
        locked_at_launch: locked,
        target_speed_fps: target_speed,
        ..Default::default()
    };
    let before: Vec<u32> = state.projectiles.iter().map(|p| p.id).collect();
    place(&mut state, &target);
    step(&mut state, l, true);
    state.release(0);
    let Some(id) = state
        .projectiles
        .iter()
        .map(|p| p.id)
        .find(|id| !before.contains(id))
    else {
        shot.outcome = "no-launch";
        shot.status_at_end = state.own().release_readiness.label();
        return Ok(shot);
    };
    shot.fired = true;
    let class = match weapon.seeker.signature {
        2 => SeekerClass::Infrared,
        _ => SeekerClass::Radar,
    };
    let effectiveness = match class {
        SeekerClass::Infrared => base.target.ecm.flare[1],
        SeekerClass::Radar => base.target.ecm.chaff[1],
    }
    .min(100);
    let mut stock = u32::from(match class {
        SeekerClass::Infrared => base.target.ecm.flare[0],
        SeekerClass::Radar => base.target.ecm.chaff[0],
    });
    let mut random = DecisionRandom::seeded(mix(seed ^ 0x5eed));
    let mut last_burst: Option<u64> = None;
    let mut pending: Vec<u64> = Vec::new();
    let mut single_done = false;
    let mut maneuvering = false;
    let mut miss = f64::INFINITY;
    let mut inert_hp: Option<i32> = None;
    let mut last_status = Status::Search;
    let mut last_speed = 0.;
    let mut decoyed = false;
    let mut lure: Option<u32> = None;
    let mut last_heading = target.heading;
    let limit = if base.rules == missiles::Rules::Energy {
        u64::from(missiles::energy_profile::body(weapon).life) + 240
    } else {
        u64::from(weapon.movement.remove_t) * 30 + 240
    };
    for tick in 1..=limit {
        let t = tick as f64 * DT;
        if cell.tactic.heat == 3 && t >= reaction_s {
            state.targets[0].heat = missiles::seeker::Heat::Engine {
                on: false,
                throttle: 0.,
                afterburner: false,
            };
        }
        let Some(p) = state.projectiles.iter().find(|p| p.id == id) else {
            break;
        };
        let missile = p.position;
        let missile_velocity = p
            .motion
            .map_or_else(|| p.direction.map(|v| v * 1000.), |m| m.velocity);
        // Closest approach from present motion, as the AI's estimate reads.
        let relative = sub(missile, target.position);
        let closing = sub(missile_velocity, target.velocity());
        let tcpa = -dot(relative, closing) / dot(closing, closing).max(1e-9);
        // The scripted defence.
        let to_missile = heading_of(sub(missile, target.position));
        match cell.tactic.maneuver {
            Maneuver::Dive => {
                if t >= reaction_s {
                    target.pitch = if target.position[1] > 100. {
                        -25f64.to_radians()
                    } else {
                        0.
                    };
                }
            }
            Maneuver::None => target.bank = 0.,
            Maneuver::Break => {
                if !maneuvering && tcpa > 0. && tcpa <= break_at_s {
                    maneuvering = true;
                }
                if maneuvering {
                    // Put the missile on the 3/9 line at full load: the turn
                    // that makes it pull hardest, into it from behind and
                    // across it from ahead.
                    target.turn_toward(beam_of(to_missile, target.heading), 7.);
                }
            }
            Maneuver::Beam => {
                if t >= reaction_s {
                    let source = if profile.guidance == Guidance::Supported {
                        l.position
                    } else {
                        missile
                    };
                    let bearing = heading_of(sub(source, target.position));
                    target.turn_toward(beam_of(bearing, target.heading), 5.);
                }
            }
            Maneuver::Drag => {
                if t >= reaction_s {
                    target.turn_toward(wrap(to_missile + std::f64::consts::PI), 5.);
                }
            }
            Maneuver::Ai => return Err("the AI tactic needs --ai".into()),
        }
        // The scripted devices.
        let mut release = 0;
        match cell.tactic.devices {
            Devices::None => {}
            Devices::Ai => {
                if tcpa > 0.
                    && tcpa <= AI_RELEASE_WINDOW_S
                    && last_burst.is_none_or(|b| tick - b >= AI_BURST_INTERVAL_TICKS)
                {
                    last_burst = Some(tick);
                    for n in 0..AI_BURST as u64 {
                        pending.push(tick + n * AI_BURST_GAP_TICKS);
                    }
                }
            }
            Devices::Hammer => {
                if tick >= 120 && (tick - 120).is_multiple_of(HAMMER_GAP_TICKS) {
                    release += 1;
                }
            }
            Devices::Single(s) => {
                if !single_done && tcpa > 0. && tcpa <= f64::from(s) {
                    single_done = true;
                    release += 1;
                }
            }
        }
        pending.retain(|due| {
            if *due == tick {
                release += 1;
                false
            } else {
                true
            }
        });
        for _ in 0..release {
            if stock == 0 {
                break;
            }
            stock -= 1;
            shot.devices += 1;
            shot.first_device_s.get_or_insert(t);
            let (rolled, spoofed, device) = release_device(
                &mut state,
                target_id,
                class,
                effectiveness,
                &mut random,
                &target,
            )?;
            shot.rolls += rolled;
            if spoofed.contains(&id) && !decoyed {
                decoyed = true;
                lure = device;
                shot.decoyed_at_s = Some(t);
                shot.decoyed_tcpa_s = Some(tcpa);
            }
        }
        if base.follow_decoy && decoyed && follow(&mut state, id, class, lure, weapon) {
            // Burst on the lure: the shot ends there.
            state.projectiles.retain(|p| p.id != id);
            shot.time_s = t;
            break;
        }
        shot.target_turn_deg += wrap(target.heading - last_heading).abs().to_degrees();
        last_heading = target.heading;
        target.advance();
        l.position[2] += shooter_speed * DT;
        place(&mut state, &target);
        let previous_missile = missile;
        let previous_hp = state.targets[0].hp;
        let events = step(&mut state, l, false);
        if base.rules == missiles::Rules::Energy {
            for roll in state
                .take_decoy_rolls()
                .into_iter()
                .filter(|r| r.projectile == id)
            {
                shot.rolls += 1;
                if roll.decoyed && !decoyed {
                    decoyed = true;
                    shot.decoyed_at_s = Some(t);
                    shot.decoyed_tcpa_s = Some(tcpa);
                }
            }
        }
        let hit = events
            .iter()
            .any(|e| matches!(e, Event::Hit(v) if *v == target_id));
        let now = state.projectiles.iter().find(|p| p.id == id);
        // Closest approach over the tick, relative motion taken as straight.
        let end = now.map_or(missile, |p| p.position);
        let a = sub(
            previous_missile,
            sub(target.position, target.velocity().map(|v| v * DT)),
        );
        let b = sub(end, target.position);
        let d = sub(b, a);
        let u = (-dot(a, d) / dot(d, d).max(1e-12)).clamp(0., 1.);
        let nearest = length(std::array::from_fn(|i| a[i] + d[i] * u));
        miss = miss.min(nearest);
        if let Some(hp) = inert_hp {
            shot.inert_damage = (hp - state.targets[0].hp).max(0);
        }
        if let Some(p) = now {
            if let Some(energy) = p.motion.and_then(|m| m.energy) {
                if energy.flight.lost {
                    shot.control_loss_s.get_or_insert(t);
                }
                if energy.flight.defeated {
                    shot.defeat_s.get_or_insert(t);
                    let hp = *inert_hp.get_or_insert(previous_hp);
                    shot.inert_damage = (hp - state.targets[0].hp).max(0);
                    shot.closest_inert_ft =
                        Some(shot.closest_inert_ft.unwrap_or(f64::MAX).min(nearest));
                    shot.inert_radius_ft =
                        state.targets[0].radius + f64::from(weapon.damage.fuze_radius.max(0));
                }
                if energy.tracking.notched
                    && !energy.flight.lost
                    && p.guidance.as_ref().is_some_and(|f| f.enabled)
                {
                    shot.first_notch_s.get_or_insert(t);
                    shot.notched_s += DT;
                }
            }
            last_speed = p.motion.map_or(0., |m| length(m.velocity));
            if let Some(flight) = &p.guidance {
                last_status = flight.seeker.status;
                if flight.seeker.observation.is_some() {
                    shot.tracked_s += DT;
                    if shot.first_lock_s.is_none() && flight.seeker.acquired {
                        shot.first_lock_s = Some(t);
                    }
                }
            } else {
                last_status = Status::Unguided;
            }
        }
        if hit {
            shot.hit = true;
            shot.kill = state.targets[0].hp <= 0;
            shot.time_s = t;
            shot.miss_ft = miss;
            break;
        }
        if events.contains(&Event::Ground) && now.is_none() {
            shot.ground_impact = true;
            shot.outcome = "ground";
        }
        shot.time_s = t;
        // A decoyed missile flies straight on: once it is past and opening it
        // can no longer reach the target, so stop early.
        if let Some(p) = now
            && base.rules != missiles::Rules::Energy
            && p.guidance.is_none()
            && p.motion
                .is_some_and(|m| dot(b, sub(m.velocity, target.velocity())) > 0.)
            && length(b) > 3000.
        {
            break;
        }
    }
    shot.miss_ft = miss;
    shot.speed_at_end_fps = last_speed;
    shot.status_at_end = status_name(last_status);
    shot.outcome = if shot.hit {
        if decoyed { "hit-after-decoy" } else { "hit" }
    } else if decoyed {
        "decoyed"
    } else if shot.outcome == "ground" {
        "ground"
    } else if matches!(last_status, Status::Expired) {
        "expired"
    } else if matches!(
        last_status,
        Status::Memory | Status::Lost | Status::Search | Status::Midcourse
    ) {
        "seeker-lost"
    } else if last_speed < target_speed {
        "energy"
    } else {
        "guidance-miss"
    };
    Ok(shot)
}

/// One device from the target, rolled against each missile as an AI
/// release rolls it (`tore_world::ai_wings`, `realise_device`). Returns the
/// rolls made and the missiles decoyed.
fn release_device(
    state: &mut live::State,
    releaser: u32,
    class: SeekerClass,
    effectiveness: u8,
    random: &mut DecisionRandom,
    target: &Flyer,
) -> AppResult<(u32, Vec<u32>, Option<u32>)> {
    state.device_released(
        tore_sim::combat::countermeasures::Release {
            position: target.position,
            velocity: target.velocity(),
            basis: Basis::new(target.heading, 0., target.bank),
        },
        match class {
            SeekerClass::Infrared => live::EffectKind::Flare,
            SeekerClass::Radar => live::EffectKind::Chaff,
        },
        releaser,
    );
    if state.weapon_rules == missiles::Rules::Energy {
        state.energy_devices.effectiveness(
            releaser,
            match class {
                SeekerClass::Infrared => missiles::decoys::Kind::Flare,
                SeekerClass::Radar => missiles::decoys::Kind::Chaff,
            },
            effectiveness,
        );
        return Ok((0, Vec::new(), None));
    }
    let seekers: Vec<(u8, u8)> = state
        .projectiles
        .iter()
        .map(|p| {
            let seeker = &state.weapon(p).seeker;
            (seeker.signature, seeker.chaff_flare_chance)
        })
        .collect();
    let device = match class {
        SeekerClass::Infrared => state.devices.flares.back().map(|f| f.seed()),
        SeekerClass::Radar => state.devices.chaff.back().map(|c| c.seed),
    };
    let mut rolls = 0;
    let mut spoofed = Vec::new();
    for (p, (signature, chance)) in state.projectiles.iter_mut().zip(seekers) {
        let seeker = match signature {
            2 => SeekerClass::Infrared,
            3 => SeekerClass::Radar,
            _ => continue,
        };
        let missile = GuidingMissile {
            seeker,
            guiding_on_releaser: p.target == Some(releaser)
                && p.guidance.as_ref().is_none_or(|f| {
                    f.enabled && f.seeker.acquired && f.seeker.observation.is_some()
                }),
            // This branch deliberately reproduces the historical Spec API.
            // Energy already returned above and clamps the complete product.
            decoy_susceptibility_percent: chance.min(100),
        };
        let outcome = threat::decoy_missile(&missile, class, effectiveness, random)
            .map_err(|e| e.to_string())?;
        if outcome != DecoyOutcome::NotEligible {
            rolls += 1;
        }
        if outcome == DecoyOutcome::Decoyed {
            p.target = None;
            p.guidance = None;
            spoofed.push(p.id);
        }
    }
    for id in &spoofed {
        state.ledger.resolve(*id, Resolution::Spoofed);
    }
    Ok((rolls, spoofed, device))
}

/// What-if only: steer a decoyed missile for its device with the record's
/// own turn limits. Returns true once it reaches the device (inside its fuze
/// radius, or passing it), where it bursts.
fn follow(
    state: &mut live::State,
    id: u32,
    class: SeekerClass,
    lure: Option<u32>,
    weapon: &Weapon,
) -> bool {
    let Some(seed) = lure else {
        return false;
    };
    let point = match class {
        SeekerClass::Infrared => state
            .devices
            .flares
            .iter()
            .find(|f| f.seed() == seed && f.burning())
            .map(|f| f.position),
        SeekerClass::Radar => state
            .devices
            .chaff
            .iter()
            .find(|c| c.seed == seed)
            .map(|c| c.position),
    };
    let (Some(point), Some(p)) = (point, state.projectiles.iter_mut().find(|p| p.id == id)) else {
        return false;
    };
    let Some(motion) = p.motion else {
        return false;
    };
    let to = sub(point, p.position);
    if dot(to, motion.velocity) <= 0. || length(to) <= f64::from(weapon.damage.fuze_radius.max(0)) {
        return true;
    }
    let desired = tore_sim::attitude::unit(to);
    let heading = missiles::commanded_heading(p.direction, motion.path(), desired);
    p.direction = missiles::steer(&weapon.movement, p.age, p.direction, heading);
    false
}

/// The study's cells from the options.
fn cells(o: &Options) -> Vec<Cell> {
    let mut cells = Vec::new();
    for missile in &o.missiles {
        for &altitudes in &o.altitudes {
            for &band in &o.bands {
                for &aspect in &o.aspects {
                    // A tactic names devices; IR stores meet flares and
                    // radar stores chaff, so one list serves both.
                    for tactic in &o.tactics {
                        cells.push(Cell {
                            missile: missile.clone(),
                            altitudes,
                            band,
                            aspect,
                            tactic: *tactic,
                        });
                    }
                }
            }
        }
    }
    cells
}

struct Options {
    out: PathBuf,
    seeds: u64,
    missiles: Vec<String>,
    tactics: Vec<Tactic>,
    bands: Vec<Band>,
    aspects: Vec<Aspect>,
    altitudes: Vec<Altitudes>,
    shooter: AircraftId,
    target: AircraftId,
    threads: usize,
    catalog: bool,
    rules: missiles::Rules,
    follow_decoy: bool,
    /// Fly the shots against the game's AI instead of the scripted target.
    ai: Option<ai::AiSetup>,
}

const USAGE: &str = "--missile-study OUT_DIR [--ai [--skill novice|average|experienced|ace] [--ai-model standard|hybrid] [--theater BAL] [--trace]] [--seeds N] [--missiles AIM9M,AIM120,...] [--profiles straight,break,beam,drag,dive,cm-ai,cm-hammer,cm-at4s,break+cm-ai,...[@off|@idle|@burner]] [--ranges near,close,short,medium,long,defeat-edge] [--aspects head,quarter,beam,tail] [--altitudes deck,low,co,high,up,down] [--shooter ID] [--target ID] [--threads N] [--missile-model energy|spec] [--decoy-model coast|lure] [--catalog]";

fn options() -> AppResult<Options> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut it = args.iter();
    let out = PathBuf::from(it.next().ok_or(USAGE)?);
    let list = |s: &str| -> Vec<String> { s.split(',').map(str::to_owned).collect() };
    let mut o = Options {
        out,
        seeds: 60,
        missiles: MISSILES.iter().map(|s| (*s).to_owned()).collect(),
        tactics: [
            "straight",
            "break",
            "beam",
            "drag",
            "cm-ai",
            "cm-hammer",
            "break+cm-ai",
            "break+cm-hammer",
            "beam+cm-ai",
            "beam+cm-hammer",
        ]
        .iter()
        .map(|s| Tactic::parse(s))
        .collect::<AppResult<_>>()?,
        bands: vec![Band::Short, Band::Medium, Band::Long],
        aspects: vec![Aspect::Head, Aspect::Beam, Aspect::Tail],
        altitudes: vec![Altitudes::Co],
        shooter: AircraftId::F16C,
        target: AircraftId::Mig29,
        threads: std::thread::available_parallelism().map_or(4, |n| n.get()),
        catalog: false,
        rules: missiles::Rules::Energy,
        follow_decoy: false,
        ai: None,
    };
    let mut skill = tore_world::mission::Skill::Average;
    let mut theater = "BAL".to_owned();
    let mut use_ai = false;
    let mut trace = false;
    let mut model = tore_world::ai_wings::AiFlightModel::Standard;
    while let Some(arg) = it.next() {
        let mut next = || -> AppResult<&String> {
            it.next()
                .ok_or_else(|| format!("{arg} needs a value").into())
        };
        match arg.as_str() {
            "--seeds" => o.seeds = next()?.parse()?,
            "--missiles" => {
                o.missiles = list(next()?)
                    .into_iter()
                    .map(|m| {
                        let m = m.to_ascii_uppercase();
                        if m.ends_with(".JT") { m } else { m + ".JT" }
                    })
                    .collect()
            }
            "--profiles" => {
                o.tactics = list(next()?)
                    .iter()
                    .map(|s| Tactic::parse(s))
                    .collect::<AppResult<_>>()?
            }
            "--ranges" => {
                o.bands = list(next()?)
                    .iter()
                    .map(|s| Band::parse(s))
                    .collect::<AppResult<_>>()?
            }
            "--aspects" => {
                o.aspects = list(next()?)
                    .iter()
                    .map(|s| Aspect::parse(s))
                    .collect::<AppResult<_>>()?
            }
            "--altitudes" => {
                o.altitudes = list(next()?)
                    .iter()
                    .map(|s| Altitudes::parse(s))
                    .collect::<AppResult<_>>()?
            }
            "--shooter" => o.shooter = AircraftId::parse(next()?)?,
            "--target" => o.target = AircraftId::parse(next()?)?,
            "--threads" => o.threads = next()?.parse::<usize>()?.max(1),
            "--catalog" => o.catalog = true,
            "--missile-model" => {
                o.rules = match next()?.as_str() {
                    "energy" => missiles::Rules::Energy,
                    "spec" => missiles::Rules::Spec,
                    _ => return Err("missile model must be energy or spec".into()),
                }
            }
            "--decoy-model" => {
                o.follow_decoy = match next()?.as_str() {
                    "coast" => false,
                    "lure" => true,
                    other => return Err(format!("unknown decoy model {other}").into()),
                }
            }
            "--ai" => use_ai = true,
            "--trace" => trace = true,
            "--skill" => skill = ai::parse_skill(next()?)?,
            "--theater" => theater = next()?.to_ascii_uppercase(),
            "--ai-model" => {
                model = match next()?.as_str() {
                    "standard" => tore_world::ai_wings::AiFlightModel::Standard,
                    "hybrid" => tore_world::ai_wings::AiFlightModel::AllHybrid,
                    other => return Err(format!("unknown AI model {other}").into()),
                }
            }
            other => return Err(format!("unknown {other}; {USAGE}").into()),
        }
    }
    if o.bands.contains(&Band::DefeatEdge) && (use_ai || o.rules != missiles::Rules::Energy) {
        return Err("defeat-edge requires scripted Energy flight".into());
    }
    if use_ai {
        o.ai = Some(ai::AiSetup {
            model,
            theater,
            shooter: o.shooter,
            target: o.target,
            skill,
            trace,
        });
        o.tactics = vec![Tactic::parse("ai")?];
    }
    Ok(o)
}

fn configuration(
    resources: &BTreeMap<String, Vec<u8>>,
    id: AircraftId,
) -> AppResult<Configuration> {
    let kind = tore_world::aircraft_type::AircraftType::load(resources, id)?;
    Ok(Configuration::from_source(&kind.profile, |name| {
        resources
            .get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
    })?)
}

/// Every A2A missile's decoy susceptibility and every aircraft's devices,
/// with the per-device decoy chance the rule gives each pair.
fn write_catalog(
    out: &Path,
    resources: &BTreeMap<String, Vec<u8>>,
    weapons: &BTreeMap<String, Weapon>,
) -> AppResult<()> {
    let mut text = String::from(
        "kind,name,signature,guidance,chaff_flare_chance,all_aspect,zone0_max_ft,zone0_h_deg,zone1_min_ft,zone1_max_ft,remove_s,fuel_s,powered_turn_dps,coast_turn_dps,max_speed_fps,fuze_ft,damage_by_class,hit_points,damage_class,flares,flare_chance,chaff,chaff_chance,ir_deception,radar_deception,mode_flags\n",
    );
    for (name, w) in weapons {
        let guidance = Profile::for_weapon(w).map_or("none".to_owned(), |p| {
            format!("{:?}", p.guidance).to_ascii_lowercase()
        });
        writeln!(
            text,
            "missile,{name},{},{guidance},{},{},{},{:.1},{},{},{},{},{:.1},{:.1},{},{},{},,,,,,,,,",
            w.seeker.signature,
            w.seeker.chaff_flare_chance,
            w.seeker.all_aspect,
            w.seeker.zones[0].maximum_range,
            missiles::half_angle(w.seeker.zones[0].heading).to_degrees(),
            w.seeker.zones[1].minimum_range,
            w.seeker.zones[1].maximum_range,
            f64::from(w.movement.remove_t) / 4.,
            f64::from(w.movement.fuel_t) / 4.,
            f64::from(w.movement.powered_turn_rate) * 360. / 65520.,
            f64::from(w.movement.unpowered_turn_rate) * 360. / 65520.,
            w.movement.maximum_speed,
            w.damage.fuze_radius,
            w.damage.by_class.map(|d| d.to_string()).join(" "),
        )?;
    }
    for id in AircraftId::ALL {
        let Ok(c) = configuration(resources, id) else {
            continue;
        };
        let e = c.ecm;
        writeln!(
            text,
            "aircraft,{id:?},,,,,,,,,,,,,,,,{},{},{},{},{},{},{},{},{:#x}",
            c.hit_points,
            live::damage_class(c.target_category),
            e.flare[0],
            e.flare[1],
            e.chaff[0],
            e.chaff[1],
            e.infrared_deception_chance,
            e.radar_deception_chance,
            e.mode_flags
        )?;
    }
    std::fs::write(out.join("catalog.csv"), text)?;
    Ok(())
}

pub fn run() -> AppResult<()> {
    let o = options()?;
    std::fs::create_dir_all(&o.out)?;
    let resources = crate::reel::load_assets()?.theater_resources;
    let mut weapons = BTreeMap::new();
    let names: Vec<String> = if o.catalog {
        MISSILES.iter().map(|s| (*s).to_owned()).collect()
    } else {
        o.missiles.clone()
    };
    for name in names {
        let bytes = resources
            .get(&name)
            .ok_or_else(|| format!("{name} is not imported"))?;
        weapons.insert(name.clone(), Weapon::parse(&name, bytes)?);
    }
    if o.catalog {
        write_catalog(&o.out, &resources, &weapons)?;
        println!(
            "missile-study: wrote {}",
            o.out.join("catalog.csv").display()
        );
        return Ok(());
    }
    let shooter = configuration(&resources, o.shooter)?;
    let station = shooter
        .stations
        .iter()
        .position(|s| Profile::for_weapon(&s.weapon).is_some())
        .ok_or("the shooter has no missile station")?;
    if o.follow_decoy && o.rules == missiles::Rules::Energy {
        return Err("the lure what-if is only available with --missile-model spec".into());
    }
    std::fs::write(
        o.out.join("model.txt"),
        format!(
            "{:?}\nmodel-fingerprint={:016x}\n",
            o.rules,
            tore_sim::combat::missiles::energy_profile::fingerprint()
        ),
    )?;
    let base = Base {
        rules: o.rules,
        follow_decoy: o.follow_decoy,
        shooter,
        target: configuration(&resources, o.target)?,
        station,
        weapons,
    };
    let cells = cells(&o);
    let jobs: Vec<(usize, u64)> = (0..cells.len())
        .flat_map(|c| (0..o.seeds).map(move |s| (c, s)))
        .collect();
    println!(
        "missile-study: {} cells x {} seeds = {} shots on {} threads, shooter {:?} target {:?}",
        cells.len(),
        o.seeds,
        jobs.len(),
        o.threads,
        o.shooter,
        o.target
    );
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, u64, Shot)>> = Mutex::new(Vec::with_capacity(jobs.len()));
    let failure: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..o.threads {
            scope.spawn(|| {
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&(c, seed)) = jobs.get(n) else {
                        break;
                    };
                    let flown = match &o.ai {
                        Some(setup) => ai::fly(
                            &resources,
                            setup,
                            &base.weapons[&cells[c].missile],
                            &cells[c],
                            seed,
                            base.rules,
                        ),
                        None => fly(&base, &cells[c], seed),
                    };
                    match flown {
                        Ok(shot) => results.lock().expect("results").push((c, seed, shot)),
                        Err(e) => {
                            *failure.lock().expect("failure") =
                                Some(format!("{} seed {seed}: {e}", cells[c].missile));
                            break;
                        }
                    }
                    if n.is_multiple_of(2000) {
                        eprintln!("missile-study: {n}/{}", jobs.len());
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().expect("failure") {
        return Err(e.into());
    }
    let mut results = results.into_inner().expect("results");
    results.sort_by_key(|(c, s, _)| (*c, *s));
    write_results(&o, &base, &cells, &results)?;
    Ok(())
}

fn write_results(
    o: &Options,
    base: &Base,
    cells: &[Cell],
    results: &[(usize, u64, Shot)],
) -> AppResult<()> {
    let mut runs = std::io::BufWriter::new(std::fs::File::create(o.out.join("runs.csv"))?);
    writeln!(
        runs,
        "missile,altitudes,range,aspect,profile,seed,range_ft,fired,locked_at_launch,outcome,hit,kill,miss_ft,time_s,devices,rolls,decoyed_at_s,decoyed_tcpa_s,first_device_s,other_devices,defended,target_turn_deg,first_lock_s,tracked_s,speed_at_end_fps,target_speed_fps,status_at_end,control_loss_s,ground_impact,first_notch_s,notched_s,defeat_s,closest_inert_ft,inert_radius_ft,inert_damage"
    )?;
    for (c, seed, s) in results {
        let cell = &cells[*c];
        writeln!(
            runs,
            "{},{},{},{},{},{seed},{:.0},{},{},{},{},{},{:.1},{:.2},{},{},{},{},{},{},{},{:.0},{},{:.2},{:.0},{:.0},{},{},{},{},{:.3},{},{},{:.3},{}",
            cell.missile,
            cell.altitudes.name(),
            cell.band.name(),
            cell.aspect.name(),
            cell.tactic.name(),
            s.range_ft,
            u8::from(s.fired),
            u8::from(s.locked_at_launch),
            s.outcome,
            u8::from(s.hit),
            u8::from(s.kill),
            s.miss_ft,
            s.time_s,
            s.devices,
            s.rolls,
            s.decoyed_at_s.map_or(String::new(), |t| format!("{t:.2}")),
            s.decoyed_tcpa_s
                .map_or(String::new(), |t| format!("{t:.2}")),
            s.first_device_s
                .map_or(String::new(), |t| format!("{t:.2}")),
            s.other_devices,
            u8::from(s.defended),
            s.target_turn_deg,
            s.first_lock_s.map_or(String::new(), |t| format!("{t:.2}")),
            s.tracked_s,
            s.speed_at_end_fps,
            s.target_speed_fps,
            s.status_at_end,
            s.control_loss_s
                .map_or(String::new(), |t| format!("{t:.3}")),
            u8::from(s.ground_impact),
            s.first_notch_s.map_or(String::new(), |t| format!("{t:.3}")),
            s.notched_s,
            s.defeat_s.map_or(String::new(), |t| format!("{t:.3}")),
            s.closest_inert_ft
                .map_or(String::new(), |v| format!("{v:.3}")),
            s.inert_radius_ft,
            s.inert_damage,
        )?;
    }
    let mut summary = std::io::BufWriter::new(std::fs::File::create(o.out.join("cells.csv"))?);
    writeln!(
        summary,
        "missile,altitudes,range,aspect,profile,n,fired,hits,hit_rate,hit_lo,hit_hi,kills,decoyed,decoy_rate,decoy_lo,decoy_hi,mean_devices,mean_rolls,mean_miss_ft,mean_time_s,outcomes,shooter,target,flare_chance,chaff_chance"
    )?;
    // Results are sorted by cell/seed. Summarize each run once, even for a
    // million-shot matrix, rather than scanning the matrix for every cell.
    let mut first = 0;
    for (c, cell) in cells.iter().enumerate() {
        let mut end = first;
        while end < results.len() && results[end].0 == c {
            end += 1;
        }
        let shots: Vec<&Shot> = results[first..end]
            .iter()
            .map(|(_, _, shot)| shot)
            .collect();
        first = end;
        let n = shots.len();
        let fired = shots.iter().filter(|s| s.fired).count();
        let hits = shots.iter().filter(|s| s.hit).count();
        let kills = shots.iter().filter(|s| s.kill).count();
        let decoyed = shots.iter().filter(|s| s.decoyed_at_s.is_some()).count();
        let (hl, hh) = wilson(hits, n);
        let (dl, dh) = wilson(decoyed, n);
        let mean = |f: &dyn Fn(&Shot) -> f64| {
            if fired == 0 {
                0.
            } else {
                shots.iter().filter(|s| s.fired).map(|s| f(s)).sum::<f64>() / fired as f64
            }
        };
        let mut outcomes: BTreeMap<&str, usize> = BTreeMap::new();
        for s in &shots {
            *outcomes.entry(s.outcome).or_default() += 1;
        }
        let outcomes = outcomes
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect::<Vec<_>>()
            .join(" ");
        writeln!(
            summary,
            "{},{},{},{},{},{n},{fired},{hits},{:.3},{hl:.3},{hh:.3},{kills},{decoyed},{:.3},{dl:.3},{dh:.3},{:.2},{:.2},{:.0},{:.2},{outcomes},{:?},{:?},{},{}",
            cell.missile,
            cell.altitudes.name(),
            cell.band.name(),
            cell.aspect.name(),
            cell.tactic.name(),
            hits as f64 / n.max(1) as f64,
            decoyed as f64 / n.max(1) as f64,
            mean(&|s| f64::from(s.devices)),
            mean(&|s| f64::from(s.rolls)),
            mean(&|s| s.miss_ft.min(1e6)),
            mean(&|s| s.time_s),
            base.shooter.aircraft,
            base.target.aircraft,
            base.target.ecm.flare[1],
            base.target.ecm.chaff[1],
        )?;
    }
    println!(
        "missile-study: wrote {} and {}",
        o.out.join("runs.csv").display(),
        o.out.join("cells.csv").display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wilson_interval_brackets_the_rate() {
        let (lo, hi) = wilson(30, 60);
        assert!(lo < 0.5 && hi > 0.5);
        assert!((lo - 0.377).abs() < 0.005 && (hi - 0.623).abs() < 0.005);
        assert_eq!(wilson(0, 60).0, 0.);
        assert!(wilson(60, 60).1 >= 0.999_999);
    }

    #[test]
    fn tactics_round_trip_their_names() {
        for name in [
            "straight",
            "break",
            "beam+cm-ai",
            "drag+cm-hammer",
            "cm-at4s",
            "straight@idle",
            "break+cm-ai@burner",
        ] {
            assert_eq!(Tactic::parse(name).unwrap().name(), name);
        }
        assert!(Tactic::parse("loop").is_err());
    }

    #[test]
    fn close_study_ranges_respect_the_minimum_without_changing_old_bands() {
        let state = tore_world::test_support::combat_fixture(true);
        let mut w = state.own().configuration().stations[0].weapon.clone();
        w.seeker.signature = 3;
        w.seeker.zones[1].minimum_range = 1000;
        w.seeker.zones[1].maximum_range = 100000;
        assert_eq!(Band::Near.feet(&w), 1519.);
        assert_eq!(Band::Close.feet(&w), 3038.);
        assert_eq!(Band::Short.feet(&w), 9114.);
        assert_eq!(Band::Medium.feet(&w), 30380.);
        assert_eq!(Band::Long.feet(&w), 90000.);
        w.seeker.zones[1].minimum_range = 4000;
        assert_eq!(Band::Near.feet(&w), 4400.);
        assert_eq!(Band::Close.feet(&w), 4400.);
        for name in ["near", "close", "short", "medium", "long"] {
            assert_eq!(Band::parse(name).unwrap().name(), name);
        }
    }

    #[test]
    fn a_constant_g_turn_rate_matches_the_load() {
        let mut f = Flyer {
            position: [0.; 3],
            heading: 0.,
            speed: 900.,
            bank: 0.,
            pitch: 0.,
        };
        // 7 G at 900 ft/s: g sqrt(48) / v = 0.2477 rad/s.
        for _ in 0..120 {
            f.turn_toward(std::f64::consts::PI / 2., 7.);
        }
        assert!((f.heading - 0.2477).abs() < 1e-3, "{}", f.heading);
        assert!(f.bank > 1.4);
        assert!((sound_fps(0.) - 1116.4).abs() < 1.);
        assert!((sound_fps(15_000.) - 1057.4).abs() < 2.);
    }
}
