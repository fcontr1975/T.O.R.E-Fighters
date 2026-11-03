//! `--surface-trace`: a development aid that flies the player's aircraft on a
//! straight, scripted line past one surface unit and prints what the surface
//! controllers do (docs/spec/surface-defenses.md, "Surface AI"). No window
//! opens; the whole mission tick runs as in the game.
//!
//! ```text
//! tore-app --surface-trace THEATER [STEM] [--defenses AAA SAM] [--surface-seed N]
//!     [--enemy-nationality N] [--aircraft ID] [--over TYPE [--index N] | --unit ID]
//!     [--altitude FT] [--speed KT] [--pass FT] [--from NM] [--seconds S]
//!     [--condition NAME] [--chaff S] [--flares S] [--harm-at NM] [--kill-at S]
//!     [--player-side red] [--invulnerable] [--quiet-shots] [--no-relocate] [--front]
//!     [--drain] [--drain-reserve] [--kill-truck-at S] [--solo] [--area]
//! ```
//!
//! `--area` with no `STEM` flies a Quick Mission with no ground target whose
//! `--defenses` scatter the enemy's area defenses (B23); `--unit 0x5f000000`
//! follows the first of them.
//!
//! The aircraft flies north (or along `--heading`) at `--altitude` feet above
//! the unit, `--pass` feet to its east, starting `--from` nm short of it. Each
//! line starts `surface-trace:`: the unit and its arms, every controller phase
//! change, shot, magazine swap, radar change, HARM roll and battery change;
//! the RWR's ground squares, lock and tone as they change; decoy rolls and
//! hits; and a closing summary line. `--chaff S` and `--flares S` release one
//! every S seconds while a missile is in flight at the aircraft;
//! `--harm-at NM` fits an AGM-88 to the aircraft, selects it, and fires it in
//! BORE through the trigger (the HARM boresight cue, slice W2) once the
//! aircraft is that close and the cue's candidate is the unit's radar; the
//! radar's destruction prints as `destroyed`. `--kill-at S` destroys the radar (the
//! battery's, or the unit itself) at S seconds. A battery launcher fights in
//! the battery the layout formed; the trace follows its radar and launchers.
//! The resupply checks (docs/spec/surface-defenses.md, "Resupply"): `--drain`
//! empties the followed units' rails and magazines at the start,
//! `--drain-reserve` empties their spare magazines too, and `--kill-truck-at
//! S` destroys every supply truck within 0.1 mile of the unit at S seconds.
//! A truck's rearm and reserve refill print as `rearm` and `refill` lines.
//! The accuracy probe (docs/spec/surface-defenses.md, "Small arms"): `--solo`
//! lays down every other armed unit's weapons so the summary's `hits` are the
//! subject's alone, and with `--near FT` the subject's own type within FT of it
//! keeps firing beside it; `--skill` and `--rng` fix the experience and the
//! surface's random stream.
//!
//! The import does not keep the ground target templates and the types only
//! they name yet, so the trace adds what the pack lacks from the retail
//! `FA_2.LIB` (read at runtime, never stored), as `--surface-dump` does, with
//! the weapon and sensor records too.
use crate::{AppResult, rwr_tone};
use std::collections::{BTreeMap, BTreeSet};
use tore_formats::{aircraft::AircraftId, quick_template::tables, weapons::Weapon};
use tore_sim::{
    attitude::Vector,
    combat::live::{self, EffectKind, Event, Ownship, Readiness},
    sensors::passive::Symbol,
};
use tore_world::{
    ai_wings::ENEMY_SIDE,
    mission::{Condition, Defense, MissionSpec},
    seats::{SeatCommand, SeatId, SeatInput},
    surface::{
        IdRange, RESUPPLY_RADIUS_FT, Surface, UnitId,
        fire::{Arsenal, Trace},
    },
    world::{Seating, TickOutput, World},
};

const SEAT: SeatId = SeatId(0);
const PLAYER: u32 = 0;
/// Id of the trace's injected AGM-88: below the surface shots' range and far
/// above any the player's own count reaches in a trace.
const FEET_PER_NM: f64 = 6_076.;
const FPS_PER_KNOT: f64 = 1.687_8;

struct Options {
    theater: String,
    stem: Option<String>,
    defenses: (usize, usize),
    seed: u32,
    nationality: Option<u8>,
    aircraft: AircraftId,
    over: Option<String>,
    index: usize,
    unit: Option<u32>,
    altitude: f64,
    speed_kt: f64,
    pass: f64,
    from_nm: f64,
    heading_deg: f64,
    seconds: f64,
    condition: Condition,
    chaff: Option<f64>,
    flares: Option<f64>,
    harm_at_nm: Option<f64>,
    kill_at: Option<f64>,
    drain: bool,
    drain_reserve: bool,
    kill_truck_at: Option<f64>,
    red: bool,
    invulnerable: bool,
    quiet_shots: bool,
    no_relocate: bool,
    no_front: bool,
    all_units: bool,
    solo: bool,
    near: f64,
    skill: Option<i32>,
    rng: Option<u64>,
    area: bool,
}

fn options() -> AppResult<Options> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut o = Options {
        theater: String::new(),
        stem: None,
        defenses: (3, 3),
        seed: 1,
        nationality: None,
        aircraft: AircraftId::F18,
        over: None,
        index: 0,
        unit: None,
        altitude: 15_000.,
        speed_kt: 450.,
        pass: 0.,
        from_nm: 20.,
        heading_deg: 0.,
        seconds: 300.,
        condition: Condition::Clear,
        chaff: None,
        flares: None,
        harm_at_nm: None,
        kill_at: None,
        drain: false,
        drain_reserve: false,
        kill_truck_at: None,
        red: false,
        invulnerable: false,
        quiet_shots: false,
        no_relocate: false,
        // A trace follows one unit's own behaviour: the front line's fighting
        // would draw from the surface's random stream and add contacts.
        no_front: true,
        all_units: false,
        solo: false,
        near: 0.,
        skill: None,
        rng: None,
        area: false,
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
            "--defenses" => o.defenses = (next()?.parse()?, next()?.parse()?),
            "--surface-seed" => o.seed = next()?.parse()?,
            "--enemy-nationality" => o.nationality = Some(next()?.parse()?),
            "--aircraft" => o.aircraft = AircraftId::parse(&next()?)?,
            "--over" => o.over = Some(next()?.to_ascii_uppercase()),
            "--index" => o.index = next()?.parse()?,
            "--unit" => {
                let text = next()?;
                o.unit = Some(match text.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16)?,
                    None => text.parse()?,
                });
            }
            "--altitude" => o.altitude = next()?.parse()?,
            "--speed" => o.speed_kt = next()?.parse()?,
            "--pass" => o.pass = next()?.parse()?,
            "--from" => o.from_nm = next()?.parse()?,
            "--heading" => o.heading_deg = next()?.parse()?,
            "--seconds" => o.seconds = next()?.parse()?,
            "--condition" => {
                let name = next()?.to_ascii_lowercase();
                o.condition = Condition::ALL
                    .into_iter()
                    .find(|c| format!("{c:?}").eq_ignore_ascii_case(&name))
                    .ok_or_else(|| format!("unknown condition {name}"))?;
            }
            "--chaff" => o.chaff = Some(next()?.parse()?),
            "--flares" => o.flares = Some(next()?.parse()?),
            "--harm-at" => o.harm_at_nm = Some(next()?.parse()?),
            "--kill-at" => o.kill_at = Some(next()?.parse()?),
            "--drain" => o.drain = true,
            "--drain-reserve" => o.drain_reserve = true,
            "--kill-truck-at" => o.kill_truck_at = Some(next()?.parse()?),
            "--player-side" => o.red = next()?.eq_ignore_ascii_case("red"),
            "--invulnerable" => o.invulnerable = true,
            "--quiet-shots" => o.quiet_shots = true,
            "--no-relocate" => o.no_relocate = true,
            "--front" => o.no_front = false,
            "--no-front" => o.no_front = true,
            "--all-units" => o.all_units = true,
            "--solo" => o.solo = true,
            "--near" => o.near = next()?.parse()?,
            "--skill" => o.skill = Some(next()?.parse()?),
            "--rng" => o.rng = Some(next()?.parse()?),
            "--area" => o.area = true,
            other if other.starts_with("--") => return Err(format!("unknown {other}").into()),
            other => positional.push(other.to_owned()),
        }
    }
    match positional.as_slice() {
        [theater] => o.theater = theater.to_ascii_uppercase(),
        [theater, stem] => {
            o.theater = theater.to_ascii_uppercase();
            o.stem = Some(stem.trim_start_matches('~').to_ascii_uppercase());
        }
        _ => return Err("--surface-trace THEATER [STEM] [options]; see surface_trace.rs".into()),
    }
    Ok(o)
}

/// The pack's resources with what the import does not keep yet: the ground
/// target templates and the unit, object, aircraft, shape, picture, weapon
/// and sensor records, read from the retail media (`TORE_GAME_DIR`, the
/// remembered source or the `gameassets` link).
pub(crate) fn resources() -> AppResult<BTreeMap<String, Vec<u8>>> {
    let mut resources = crate::reel::load_assets()?.theater_resources;
    let data = crate::assets::data_directory()?;
    let candidates = std::env::var_os("TORE_GAME_DIR")
        .map(std::path::PathBuf::from)
        .into_iter()
        .chain(tore_import::media_source::remembered(&data).map(|(path, _)| path))
        .chain([std::path::PathBuf::from("gameassets/fighters-anthology")]);
    let Some(media) = candidates
        .into_iter()
        .find_map(|path| tore_import::MediaSource::detect(&path).ok())
    else {
        return Err("no retail media to read the surface records from: set TORE_GAME_DIR".into());
    };
    let mut added = 0;
    for name in ["FA_2.LIB", "FA_1.LIB"] {
        let Some(archive) = media.optional_archive(name)? else {
            continue;
        };
        for entry in archive.entries.keys() {
            let wanted = (entry.starts_with("~Q") && entry.ends_with(".M"))
                || [".NT", ".OT", ".PT", ".SH", ".PIC", ".JT", ".SEE"]
                    .iter()
                    .any(|ext| entry.ends_with(ext));
            if wanted && !resources.contains_key(entry) {
                resources.insert(entry.clone(), archive.read(entry)?);
                added += 1;
            }
        }
    }
    println!("surface-trace: {added} resources added from the retail media");
    Ok(resources)
}

fn spec(o: &Options) -> AppResult<MissionSpec> {
    let mut spec = MissionSpec::new(&o.theater, o.aircraft);
    spec.condition = o.condition;
    if let Some(stem) = &o.stem {
        let index = tables::THEATERS
            .iter()
            .position(|t| t.eq_ignore_ascii_case(&o.theater))
            .ok_or_else(|| format!("{}: no ground targets", o.theater))?;
        spec.ground_target = Some(stem.clone());
        spec.aaa = Defense::from_level(o.defenses.0).ok_or("defense levels are 0 to 3")?;
        spec.sam = Defense::from_level(o.defenses.1).ok_or("defense levels are 0 to 3")?;
        spec.surface_seed = o.seed;
        spec.enemy_nationality = o
            .nationality
            .unwrap_or(tables::ENEMY_NATIONALITY[index] as u8);
    }
    if o.area && o.stem.is_none() {
        let index = tables::THEATERS
            .iter()
            .position(|t| t.eq_ignore_ascii_case(&o.theater))
            .ok_or_else(|| format!("{}: not a Quick Mission theater", o.theater))?;
        spec.aaa = Defense::from_level(o.defenses.0).ok_or("defense levels are 0 to 3")?;
        spec.sam = Defense::from_level(o.defenses.1).ok_or("defense levels are 0 to 3")?;
        spec.surface_seed = o.seed;
        spec.enemy_nationality = o
            .nationality
            .unwrap_or(tables::ENEMY_NATIONALITY[index] as u8);
    }
    if o.invulnerable {
        spec.cheats.damage = tore_sim::cheats::Damage::Invulnerable;
    }
    spec.validate()?;
    Ok(spec)
}

/// The unit the trace flies past: `--unit`, or the `--index`th of type
/// `--over` hostile to the aircraft (template units first).
fn subject(o: &Options, surface: &Surface, player_side: live::Side) -> AppResult<UnitId> {
    if let Some(id) = o.unit {
        return surface
            .unit(UnitId(id))
            .map(|u| u.id)
            .ok_or_else(|| format!("no surface unit {id:#x}").into());
    }
    let wanted = o.over.as_deref().ok_or("--over TYPE or --unit ID")?;
    let mut matches: Vec<_> = surface
        .units
        .iter()
        .filter(|u| {
            u.resource
                .split('.')
                .next()
                .is_some_and(|stem| stem.eq_ignore_ascii_case(wanted))
                && u.side != live::NO_SIDE
                && u.side != player_side
                && surface.arsenal.arms(u.id).is_some()
        })
        .collect();
    matches.sort_by_key(|u| (u.id.range() != IdRange::Template, u.id));
    matches
        .get(o.index)
        .map(|u| u.id)
        .ok_or_else(|| format!("no hostile armed {wanted} number {}", o.index).into())
}

fn distance(a: Vector, b: Vector) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// One surface trace line, ids in hex.
pub(crate) fn describe(trace: &Trace) -> String {
    match trace {
        Trace::Phase {
            unit,
            weapon,
            phase,
            target,
        } => format!(
            "phase {:#x} w{weapon} {phase:?} target {}",
            unit.0,
            target.map_or("none".into(), |t| t.to_string())
        ),
        Trace::Shot {
            unit,
            mount,
            record,
            target,
            rounds,
            refused,
            opening,
            flak,
        } => format!(
            "shot {:#x} m{mount} {record} target {target} rounds {rounds} refused {refused}{}{}",
            unit.0,
            if *opening { " opening" } else { "" },
            if *flak { " flak" } else { "" }
        ),
        Trace::Swap { unit, mount } => format!("swap {:#x} m{mount}", unit.0),
        Trace::Rearm { unit } => format!("rearm {:#x}", unit.0),
        Trace::Refill { unit, mount } => format!("refill {:#x} m{mount}", unit.0),
        Trace::Radar { unit, on } => {
            format!("radar {:#x} {}", unit.0, if *on { "on" } else { "off" })
        }
        Trace::Shutdown {
            unit,
            missile,
            rolled,
        } => format!(
            "harm-roll {:#x} missile {missile:#x} {}",
            unit.0,
            if *rolled { "shutdown" } else { "stays-on" }
        ),
        Trace::Battery {
            battery,
            phase,
            target,
            optical,
        } => format!(
            "battery {battery} {phase:?} target {}{}",
            target.map_or("none".into(), |t| t.to_string()),
            if *optical { " optical" } else { "" }
        ),
        Trace::FlakTeam { team, guns, groups } => {
            let groups: Vec<String> = groups
                .iter()
                .map(|g| {
                    format!(
                        "[{}] in-range {}/{}{}{} {}",
                        g.members
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                        g.in_range,
                        g.members.len(),
                        if g.heading_in { " heading-in" } else { "" },
                        if g.firing { " firing" } else { "" },
                        if g.engaged { "engaged" } else { "holding" },
                    )
                })
                .collect();
            format!(
                "flak-team {team} guns {} groups {}",
                guns.iter()
                    .map(|g| format!("{:#x}", g.0))
                    .collect::<Vec<_>>()
                    .join(","),
                if groups.is_empty() {
                    "none".to_owned()
                } else {
                    groups.join(" ")
                }
            )
        }
    }
}

/// The unit a trace line is about.
fn about(trace: &Trace) -> Option<UnitId> {
    match trace {
        Trace::Phase { unit, .. }
        | Trace::Shot { unit, .. }
        | Trace::Swap { unit, .. }
        | Trace::Rearm { unit }
        | Trace::Refill { unit, .. }
        | Trace::Radar { unit, .. }
        | Trace::Shutdown { unit, .. } => Some(*unit),
        Trace::Battery { .. } | Trace::FlakTeam { .. } => None,
    }
}

fn line(t: f64, text: impl std::fmt::Display) {
    println!("surface-trace: t={t:.2} {text}");
}

#[derive(Default)]
struct Summary {
    missiles: u32,
    rounds: u32,
    refused: u32,
    opening: u32,
    tracers: u32,
    flak: u32,
    min_burst_ft: Option<f64>,
    hits: u32,
    decoy_rolls: u32,
    decoyed: u32,
    shutdowns: u32,
    first_launch: Option<f64>,
    ground_square: bool,
    lock_tone: bool,
    radar_lock_tone: bool,
    inbound_tone: bool,
    swaps: u32,
    bursts: u32,
    rearms: u32,
    refills: u32,
}

/// A gun burst being summed up: first and last round's time, rounds.
#[derive(Clone, Copy)]
struct OpenBurst {
    first: f64,
    last: f64,
    rounds: u32,
}

pub fn run() -> AppResult<()> {
    let o = options()?;
    let resources = resources()?;
    let spec = spec(&o)?;
    let mut world = World::build(
        &spec,
        &resources,
        Seating::SinglePlayer,
        &mut tore_world::world::Hooks {
            ground_variation: Some(tore_world::surface::layout::Variation {
                jitter: true,
                relocate: !o.no_relocate,
                front: !o.no_front,
            }),
            ..Default::default()
        },
    )?
    .world;
    if let Some(why) = &world.terrain.surface.unresolved {
        return Err(why.clone().into());
    }
    let player_side = if o.red {
        ENEMY_SIDE
    } else {
        world
            .combat
            .state
            .ownship(PLAYER)
            .map_or(live::DEFAULT_OWNSHIP_SIDE, |own| own.side)
    };
    if o.red
        && let Some(own) = world.combat.state.ownship_mut(PLAYER)
    {
        own.side = ENEMY_SIDE;
    }
    let unit = subject(&o, &world.terrain.surface, player_side)?;
    if let Some(skill) = o.skill {
        // The subject and every unit of its type within 1 nm (its battery
        // mates) at this experience, for a deterministic scenario.
        let surface = &mut world.terrain.surface;
        let here = surface
            .unit(unit)
            .map(|u| (u.position, u.resource.clone()))
            .expect("chosen from the surface");
        for u in &mut surface.units {
            let d =
                f64::from(u.position[0] - here.0[0]).hypot(f64::from(u.position[2] - here.0[2]));
            if u.id == unit || (u.resource == here.1 && d <= FEET_PER_NM) {
                u.skill = skill;
            }
        }
    }
    if o.skill.is_some() {
        reload_arsenal(&mut world, &resources);
    }
    if let Some(rng) = o.rng {
        world.combat.surface.rng = rng;
    }
    if o.solo {
        // `--solo`: every other armed unit lays its weapons down, so the hits
        // the summary counts are the subject's alone (the accuracy probe).
        // With `--near FT` the subject's own type within FT of it keeps
        // fighting beside it (a squad's neighbours at an airfield).
        let surface = &mut world.terrain.surface;
        let here = surface
            .unit(unit)
            .map(|u| (u.position, u.resource.clone()))
            .expect("chosen from the surface");
        let keep: BTreeSet<UnitId> = surface
            .units
            .iter()
            .filter(|u| {
                u.id == unit
                    || (o.near > 0.
                        && u.resource == here.1
                        && f64::from(u.position[0] - here.0[0])
                            .hypot(f64::from(u.position[2] - here.0[2]))
                            <= o.near)
            })
            .map(|u| u.id)
            .collect();
        for other in &mut surface.arsenal.units {
            if !keep.contains(&other.unit) {
                other.weapons.clear();
            }
        }
    }
    let surface = &world.terrain.surface;
    let arms = surface.arsenal.arms(unit).ok_or("the unit has no arms")?;
    let armed = surface
        .arsenal
        .units
        .iter()
        .filter(|a| !a.weapons.is_empty())
        .count();
    let record = surface.unit(unit).expect("chosen from the surface");
    println!(
        "surface-trace: subject {:#x} {} side {} skill {} at {:.0} {:.0} {:.0} armed-units {armed} radars {} batteries {}",
        unit.0,
        record.resource,
        record.side.0,
        arms.skill,
        arms.position[0],
        arms.position[1],
        arms.position[2],
        surface
            .arsenal
            .units
            .iter()
            .filter(|a| a.radar.is_some())
            .count(),
        surface.arsenal.batteries.len(),
    );
    for (index, weapon) in arms.weapons.iter().enumerate() {
        println!(
            "surface-trace: weapon {index} {} {:?} mounts {:?} magazine {} launch {:.0}..{:.0} ft alt {:.0}..{:.0} battery {}",
            weapon.record.source,
            weapon.kind,
            weapon.mounts.iter().map(|m| m.index).collect::<Vec<_>>(),
            weapon.magazine,
            weapon.launch.min_range,
            weapon.launch.max_range,
            weapon.launch.min_altitude.max(-1e9),
            weapon.launch.max_altitude.min(1e9),
            weapon.battery,
        );
    }
    for (why_unit, why) in &surface.arsenal.unreadable {
        println!("surface-trace: unreadable {:#x} {why}", why_unit.0);
    }
    // The radar a HARM homes on and a kill silences: the battery's, or the
    // unit's own.
    let radar = surface
        .arsenal
        .batteries
        .iter()
        .find(|b| b.launchers.contains(&unit))
        .map_or(unit, |b| b.radar);
    if radar != unit {
        println!("surface-trace: radar {:#x}", radar.0);
    }
    let subject_battery = surface
        .arsenal
        .batteries
        .iter()
        .find(|b| b.launchers.contains(&unit))
        .map(|b| b.index);
    // The units the summary counts and the trace follows: the subject, its
    // radar and its battery's launchers.
    let focus: BTreeSet<UnitId> = surface
        .arsenal
        .batteries
        .iter()
        .filter(|b| b.launchers.contains(&unit))
        .flat_map(|b| b.launchers.iter().copied())
        .chain([unit, radar])
        // `--near FT`: every armed unit of the subject's side that close.
        .chain(
            surface
                .arsenal
                .units
                .iter()
                .filter(|a| {
                    !a.weapons.is_empty()
                        && a.side == arms.side
                        && distance(a.position, arms.position) <= o.near
                })
                .map(|a| a.unit),
        )
        .collect();
    if focus.len() > 1 {
        println!(
            "surface-trace: following {:?}",
            focus
                .iter()
                .map(|id| format!(
                    "{:#x} {}",
                    id.0,
                    surface.unit(*id).map_or("?", |u| u.resource.as_str())
                ))
                .collect::<Vec<_>>()
        );
    }
    // The supply trucks that serve the unit where it stands: its side, within
    // 0.1 mile of its placement.
    let trucks: Vec<UnitId> = surface
        .trucks
        .iter()
        .filter(|truck| {
            surface
                .unit(truck.id)
                .zip(surface.unit(unit))
                .is_some_and(|(truck, here)| {
                    truck.side == here.side
                        && f64::from(truck.position[0] - here.position[0])
                            .hypot(f64::from(truck.position[2] - here.position[2]))
                            <= RESUPPLY_RADIUS_FT
                })
        })
        .map(|truck| truck.id)
        .collect();
    println!(
        "surface-trace: trucks {}",
        if trucks.is_empty() {
            "none".to_owned()
        } else {
            trucks
                .iter()
                .map(|id| format!("{:#x}", id.0))
                .collect::<Vec<_>>()
                .join(" ")
        }
    );
    // The resupply checks start the followed units short of rounds.
    if o.drain || o.drain_reserve {
        world.combat.surface.arm(&surface.arsenal);
        for id in &focus {
            let Some(state) = world.combat.surface.unit_mut(*id) else {
                continue;
            };
            for stock in &mut state.mounts {
                if o.drain {
                    stock.loaded = 0;
                }
                if o.drain_reserve && stock.reserve.is_some() {
                    stock.reserve = Some(0);
                }
            }
        }
    }
    let center = arms.position;
    let heading = o.heading_deg.to_radians();
    let forward = [heading.sin(), 0., heading.cos()];
    let right = [heading.cos(), 0., -heading.sin()];
    let speed = o.speed_kt * FPS_PER_KNOT;
    let start: Vector = std::array::from_fn(|i| {
        center[i] - forward[i] * o.from_nm * FEET_PER_NM + right[i] * o.pass
    });
    let altitude = center[1] + o.altitude;
    let ticks = (o.seconds * 120.) as u64;
    let mut summary = Summary::default();
    let mut warnings = rwr_tone::Warnings::default();
    let mut rwr_seen: Option<(Vec<u32>, Vec<u8>, Option<rwr_tone::Tone>)> = None;
    let mut flak_seen: BTreeSet<[u64; 3]> = BTreeSet::new();
    let mut last_chaff = f64::NEG_INFINITY;
    let mut last_flare = f64::NEG_INFINITY;
    let mut harm_fired = false;
    let mut killed = false;
    let mut trucks_killed = false;
    let mut out = TickOutput::default();
    let mut bursts: BTreeMap<(UnitId, usize), OpenBurst> = BTreeMap::new();
    let harm = match o.harm_at_nm {
        Some(_) => fit_harm(&mut world, &resources)?,
        None => None,
    };
    let mut harm_pressed: Option<u64> = None;
    let mut radar_destroyed = false;
    for tick in 0..ticks {
        let t = tick as f64 / 120.;
        // The scripted line: position, attitude and velocity every tick.
        {
            let flight = &mut world.cockpits[0].flight;
            flight.position = [
                start[0] + forward[0] * speed * t,
                altitude,
                start[2] + forward[2] * speed * t,
            ];
            flight.velocity = [forward[0] * speed, 0., forward[2] * speed];
            flight.speed = speed;
            flight.yaw = heading;
            flight.pitch = 0.;
            flight.bank = 0.;
            flight.vertical_speed = 0.;
            flight.roll_rate = 0.;
            flight.pitch_rate = 0.;
        }
        let position = world.cockpits[0].flight.position;
        let threatened = world
            .combat
            .state
            .projectiles
            .iter()
            .any(|p| p.target == Some(PLAYER) && p.guidance.is_some());
        let mut commands = Vec::new();
        if let Some(every) = o.chaff
            && threatened
            && t - last_chaff >= every
        {
            commands.push(SeatCommand::ReleaseChaff);
            last_chaff = t;
        }
        if let Some(every) = o.flares
            && threatened
            && t - last_flare >= every
        {
            commands.push(SeatCommand::ReleaseFlare);
            last_flare = t;
        }
        // The HARM: selected, then fired through the trigger once the cue's
        // BORE candidate is the radar and the weapon reads READY.
        let mut trigger = false;
        if let (Some(at), Some(station)) = (o.harm_at_nm, harm)
            && !harm_fired
        {
            let own = world.combat.state.own();
            if !(own.armed && own.selected == station) {
                commands.push(SeatCommand::CycleWeapon { forward: true });
            }
            let target = world
                .terrain
                .surface
                .arsenal
                .arms(radar)
                .map(|a| world.combat.surface.place(a).eye);
            let launcher = tore_world::combat::launcher(&world.cockpits[0].flight);
            let ready = world
                .combat
                .state
                .view(PLAYER)
                .is_some_and(|view| view.readiness(launcher) == Readiness::Ready);
            if harm_pressed.is_none()
                && let Some(target) = target
                && distance(target, position) <= at * FEET_PER_NM
                && own.bore_observation.map(|o| o.id) == Some(radar.0)
                && ready
            {
                harm_pressed = Some(tick);
            }
            trigger = harm_pressed.is_some_and(|at| tick < at + 12);
        }
        if let Some(at) = o.kill_at
            && !killed
            && t >= at
        {
            if let Some(target) = world
                .combat
                .state
                .targets
                .iter_mut()
                .find(|target| target.id == radar.0)
            {
                target.hp = 0;
            }
            line(t, format_args!("killed {:#x}", radar.0));
            killed = true;
        }
        if let Some(at) = o.kill_truck_at
            && !trucks_killed
            && t >= at
        {
            for id in &trucks {
                if let Some(target) = world
                    .combat
                    .state
                    .targets
                    .iter_mut()
                    .find(|target| target.id == id.0)
                {
                    target.hp = 0;
                }
                line(t, format_args!("killed truck {:#x}", id.0));
            }
            trucks_killed = true;
        }
        let input = SeatInput {
            seat: SEAT,
            tick: world.tick(),
            trigger,
            commands,
            ..Default::default()
        };
        world.step(std::slice::from_ref(&input), &mut out)?;
        if let Some(station) = harm
            && !harm_fired
            && let Some(p) = world
                .combat
                .state
                .projectiles
                .iter()
                .find(|p| p.owner == PLAYER && p.station == station)
        {
            line(
                t,
                format_args!(
                    "harm launched at {:#x} mode BORE",
                    p.target.unwrap_or_default()
                ),
            );
            harm_fired = true;
        }
        // A HARM keeps homing after the shutdown (retail): the radar's death.
        if !radar_destroyed
            && !killed
            && world
                .combat
                .state
                .targets
                .iter()
                .any(|target| target.id == radar.0 && target.hp <= 0)
        {
            line(t, format_args!("destroyed {:#x}", radar.0));
            radar_destroyed = true;
        }
        // The surface's own trace.
        for trace in &world.combat.surface.trace {
            let followed = match trace {
                Trace::Battery { battery, .. } => Some(*battery) == subject_battery,
                _ => about(trace).is_none_or(|id| focus.contains(&id)),
            };
            if !followed {
                if o.all_units {
                    line(t, describe(trace));
                }
                continue;
            }
            // A gun's burst closes when its controller leaves Fire.
            if let Trace::Phase {
                unit,
                phase,
                weapon,
                ..
            } = trace
                && *phase != tore_sim::ai::surface::Phase::Fire
            {
                let mount = surface_mount(&world, *unit, *weapon);
                if let Some(burst) = bursts.remove(&(*unit, mount)) {
                    summary.bursts += 1;
                    let span = burst.last - burst.first;
                    let rpm = if span > 0. {
                        f64::from(burst.rounds.saturating_sub(1)) * 60. / span
                    } else {
                        0.
                    };
                    line(
                        t,
                        format_args!(
                            "burst {:#x} m{mount} rounds {} span {span:.2} s rpm {rpm:.0}",
                            unit.0, burst.rounds
                        ),
                    );
                }
            }
            match trace {
                Trace::Shot {
                    unit: shooter,
                    mount,
                    rounds,
                    refused,
                    opening,
                    record,
                    ..
                } => {
                    if tore_sim::combat::surface_guns::is_surface_gun(record) && *rounds > 0 {
                        let open = bursts.entry((*shooter, *mount)).or_insert(OpenBurst {
                            first: t,
                            last: t,
                            rounds: 0,
                        });
                        open.last = t;
                        open.rounds += rounds;
                    }
                    let missile = !tore_sim::combat::surface_guns::is_surface_gun(record);
                    if missile {
                        summary.missiles += rounds;
                        if *rounds > 0 && summary.first_launch.is_none() {
                            summary.first_launch = Some(t);
                        }
                    } else {
                        summary.rounds += rounds;
                    }
                    summary.refused += refused;
                    if *opening {
                        summary.opening += rounds;
                    }
                    if missile || !o.quiet_shots {
                        line(t, describe(trace));
                    }
                }
                Trace::Shutdown { rolled: true, .. } => {
                    summary.shutdowns += 1;
                    line(t, describe(trace));
                }
                Trace::Swap { .. } => {
                    summary.swaps += 1;
                    line(t, describe(trace));
                }
                Trace::Rearm { .. } => {
                    summary.rearms += 1;
                    line(t, describe(trace));
                }
                Trace::Refill { .. } => {
                    summary.refills += 1;
                    line(t, describe(trace));
                }
                _ => line(t, describe(trace)),
            }
        }
        // New rounds: tracers.
        for p in &world.combat.state.projectiles {
            if p.age == 0
                && p.tracer
                && focus.contains(&UnitId(p.owner))
                && world.combat.state.surface_round(p.id).is_some()
            {
                summary.tracers += 1;
            }
        }
        // New flak bursts.
        let mut flak_now = BTreeSet::new();
        for effect in &world.combat.state.effects {
            if effect.kind == EffectKind::Flak {
                let key = effect.position.map(f64::to_bits);
                if !flak_seen.contains(&key) {
                    summary.flak += 1;
                    let above = effect.position[1] - center[1];
                    summary.min_burst_ft =
                        Some(summary.min_burst_ft.map_or(above, |m: f64| m.min(above)));
                    // Where it fell from the player now (a steady target is where
                    // the gun led to, so this is the burst's place in its egg).
                    let player = world.cockpits[0].flight.position;
                    let east = effect.position[0] - player[0];
                    let north = effect.position[2] - player[2];
                    let up = effect.position[1] - player[1];
                    line(
                        t,
                        format_args!(
                            "flak-burst {above:.0} ft above the unit, {east:.0} ft east, {north:.0} ft north and {up:.0} ft up of the player"
                        ),
                    );
                }
                flak_now.insert(key);
            }
        }
        flak_seen = flak_now;
        // Decoys and hits.
        for roll in world.combat.state.take_decoy_rolls() {
            summary.decoy_rolls += 1;
            summary.decoyed += u32::from(roll.decoyed);
            let (owner, record) = world
                .combat
                .state
                .projectiles
                .iter()
                .find(|p| p.id == roll.projectile)
                .map_or((0, String::from("?")), |p| {
                    (p.owner, world.combat.state.weapon(p).source.clone())
                });
            line(
                t,
                format_args!(
                    "decoy {:?} missile {:#x} {record} from {owner:#x} chance {}/{} roll {} decoyed {}",
                    roll.kind, roll.projectile, roll.threshold, roll.bound, roll.roll, roll.decoyed
                ),
            );
        }
        for event in &out.events {
            match event {
                Event::OwnshipDamaged { aircraft, amount } if *aircraft == PLAYER => {
                    summary.hits += 1;
                    if !o.quiet_shots {
                        line(t, format_args!("player hit for {amount}"));
                    }
                }
                Event::Hit(id) if *id == PLAYER => summary.hits += 1,
                Event::OwnshipDestroyed { aircraft } if *aircraft == PLAYER => {
                    line(t, "player destroyed");
                }
                Event::Defeated(id) => line(t, format_args!("jammer defeated {id:#x}")),
                _ => {}
            }
        }
        // The RWR: ground squares, locks and the tone.
        let flight = &world.cockpits[0].flight;
        let readout = world.combat.cockpit_readout(
            PLAYER,
            tore_world::combat::launcher(flight),
            world.ai_wings.as_ref(),
            world.cockpits.first(),
        );
        if let Some(readout) = readout {
            let squares: Vec<u32> = readout
                .rwr
                .emitters
                .iter()
                .filter(|e| e.symbol == Symbol::Ground)
                .map(|e| e.id)
                .collect();
            let gone = flight.crashed || flight.escape.is_some();
            let tone = warnings.step(
                world.combat.state.tick(),
                rwr_tone::inbound(&readout, flight.position),
                &readout.rwr.locks,
                gone,
            );
            summary.ground_square |= squares.contains(&radar.0);
            summary.lock_tone |= matches!(
                tone,
                Some(rwr_tone::Tone::RadarLock | rwr_tone::Tone::InfraredLock)
            );
            summary.radar_lock_tone |= tone == Some(rwr_tone::Tone::RadarLock);
            summary.inbound_tone |= matches!(
                tone,
                Some(rwr_tone::Tone::RadarInbound | rwr_tone::Tone::InfraredInbound)
            );
            let now = (squares, readout.rwr.locks.clone(), tone);
            if rwr_seen.as_ref() != Some(&now) {
                line(
                    t,
                    format_args!(
                        "rwr ground-squares {:?} locks {:?} tone {:?}",
                        now.0
                            .iter()
                            .map(|id| format!("{id:#x}"))
                            .collect::<Vec<_>>(),
                        now.1,
                        now.2
                    ),
                );
                rwr_seen = Some(now);
            }
        }
        if world.cockpits[0].flight.crashed {
            line(t, "player crashed");
            break;
        }
    }
    let player_hp = world.combat.state.ownship(PLAYER).map_or(0, |own| own.hp);
    // The subject's stock first, then the other followed units'.
    for id in std::iter::once(unit).chain(focus.iter().copied().filter(|id| *id != unit)) {
        let Some(state) = world.combat.surface.unit(id) else {
            continue;
        };
        for (index, stock) in state.mounts.iter().enumerate() {
            println!(
                "surface-trace: stock {:#x} m{index} loaded {} reserve {}",
                id.0,
                stock.loaded,
                stock.reserve.map_or("unlimited".into(), |r| r.to_string())
            );
        }
    }
    println!(
        "surface-trace: summary bursts {} missiles {} rounds {} refused {} opening {} tracers {} swaps {} rearms {} refills {} flak-bursts {} min-burst-ft {} hits {} player-hp {player_hp} decoy-rolls {} decoyed {} shutdowns {} first-launch-s {} ground-square {} lock-tone {} radar-lock-tone {} inbound-tone {}",
        summary.bursts,
        summary.missiles,
        summary.rounds,
        summary.refused,
        summary.opening,
        summary.tracers,
        summary.swaps,
        summary.rearms,
        summary.refills,
        summary.flak,
        summary
            .min_burst_ft
            .map_or("none".into(), |v| format!("{v:.0}")),
        summary.hits,
        summary.decoy_rolls,
        summary.decoyed,
        summary.shutdowns,
        summary
            .first_launch
            .map_or("none".into(), |v| format!("{v:.2}")),
        summary.ground_square,
        summary.lock_tone,
        summary.radar_lock_tone,
        summary.inbound_tone,
    );
    Ok(())
}

/// Rebuilds the arsenal and the surface state after the trace changed the
/// surface (an experience set).
fn reload_arsenal(world: &mut World, resources: &BTreeMap<String, Vec<u8>>) {
    let terrain = &world.terrain;
    let arsenal = Arsenal::load(&terrain.surface, resources, &|x, z| {
        f64::from(terrain.height(x as f32, z as f32))
    });
    world.terrain.surface.arsenal = arsenal;
    world.combat.surface = world.terrain.surface.fresh_state();
}

/// The hardpoint of `unit`'s weapon `weapon` (a gun's only mount).
fn surface_mount(world: &World, unit: UnitId, weapon: usize) -> usize {
    world
        .terrain
        .surface
        .arsenal
        .arms(unit)
        .and_then(|arms| arms.weapons.get(weapon))
        .and_then(|w| w.mounts.first())
        .map_or(0, |m| m.index)
}

/// Fits an AGM-88 pair to the aircraft's first missile station (a Cheat
/// loading would carry it there), for `--harm-at`. Returns the station.
fn fit_harm(world: &mut World, resources: &BTreeMap<String, Vec<u8>>) -> AppResult<Option<usize>> {
    let Some(bytes) = resources.get("AGM88.JT") else {
        return Ok(None);
    };
    let record = Weapon::parse("AGM88.JT", bytes)?;
    let state = &mut world.combat.state;
    let own = state.ownship(PLAYER).ok_or("no ownship")?;
    let mut config = own.configuration().clone();
    let Some(station) = config
        .stations
        .iter()
        .position(|s| s.weapon.seeker.signature != 0)
    else {
        return Ok(None);
    };
    config.stations[station].weapon = record;
    config.stations[station].count = 2;
    let (side, friendlies) = (own.side, own.friendlies.clone());
    state.remove_ownship(PLAYER);
    let mut own = Ownship::new(PLAYER, side, config, true)?;
    own.friendlies = friendlies;
    state.add_ownship(own)?;
    println!("surface-trace: harm station {station}");
    Ok(Some(station))
}
