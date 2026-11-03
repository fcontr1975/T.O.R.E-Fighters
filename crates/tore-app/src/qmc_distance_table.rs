//! `--qmc-distance-table`: measures which Quick Mission enemy distances fit
//! each creator theater and ground target, with the launch solver, and
//! writes the checked-in table the creator and the dedicated server read
//! (`tore_world::mission_layout::distance_table`). Headless; see
//! docs/DEVELOPMENT.md, "Quick Mission distance table".
//!
//! A distance is offered only when it fits every case of its row:
//!
//! - No ground target, airborne: every enemy formation the creator can make
//!   (each enemy wing 0 to 5 aircraft) in each of the three situations, from
//!   the free-flight start. Area defenses place no start, so they share it.
//! - No ground target, ground start: every airfield the creator offers the
//!   player, with every wing size it holds, and every enemy formation.
//! - A ground target, airborne: `--seeds` surface seeds, the AAA and SAM
//!   levels cycling through all sixteen pairs, at the theater's own enemy
//!   nationality. A ground start with a target offers every distance.
use crate::AppResult;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tore_formats::{aircraft::AircraftId, quick_template::tables};
use tore_world::{
    mission::{Defense, MissionSpec, Situation, ground_targets},
    mission_layout::{availability, distance_table, ground_layout, map_bounds},
    terrain::Terrain,
};

type Resources = BTreeMap<String, Vec<u8>>;

/// One bit per offered distance, bit 0 for 1 nm.
fn mask(fits: [bool; 11]) -> u16 {
    fits.iter()
        .enumerate()
        .fold(0, |m, (i, fit)| m | (u16::from(*fit) << i))
}

/// The offered distances, `1,2,5,...`.
fn list(mask: u16) -> String {
    let miles = distance_table::miles(mask);
    if miles.is_empty() {
        return "none".into();
    }
    miles
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The mask has a hole: a distance left out below an offered one.
fn gap(mask: u16) -> bool {
    mask & (mask + 1) != 0
}

/// The theater's no-target rows.
#[derive(Debug)]
struct Free {
    code: &'static str,
    airborne: u16,
    /// Each situation's airborne mask, in [`Situation::ALL`] order.
    situations: [u16; 3],
    ground: u16,
    /// Each offered airfield's mask, by runway object.
    airfields: Vec<(u32, u16)>,
    /// Wing sizes an airfield cannot park (it refuses them at launch too).
    unparked: Vec<(u32, usize)>,
}

/// A ground target's airborne row.
#[derive(Debug)]
struct Aimed {
    code: &'static str,
    stem: &'static str,
    airborne: u16,
    /// The largest fitting distance of each seed, nm.
    seed_tops: Vec<(u32, u32)>,
    /// Each limitation the layout reported, with the seeds that had it.
    limitations: BTreeMap<String, Vec<u32>>,
}

/// Area defenses with no target: only their limitations (they place no
/// start, so their distances are the theater's no-target rows).
#[derive(Debug)]
struct Area {
    code: &'static str,
    limitations: BTreeMap<String, Vec<u32>>,
}

#[derive(Debug)]
enum Outcome {
    Free(Free),
    Aimed(Aimed),
    Area(Area),
}

#[derive(Clone, Copy)]
enum Job {
    Free(&'static str),
    Aimed(&'static str, &'static str),
    Area(&'static str),
}

/// Every enemy formation the creator can make: enemy wings 1 to 3 of 0 to
/// 5 aircraft each.
fn formations() -> impl Iterator<Item = [usize; 3]> {
    (0..=5).flat_map(|a| (0..=5).flat_map(move |b| (0..=5).map(move |c| [a, b, c])))
}

fn free(resources: &Resources, code: &'static str) -> AppResult<Free> {
    let spec = MissionSpec::new(code, AircraftId::F18);
    let terrain = Terrain::for_mission_surface(
        resources,
        code,
        Some(spec.condition.index()),
        &spec.weather,
        None,
        None,
    )?;
    let bounds = map_bounds(&terrain);
    let launches: Vec<_> = formations()
        .map(|counts| {
            let mut spec = spec.clone();
            for (wing, count) in spec.wings[3..].iter_mut().zip(counts) {
                wing.count = count;
            }
            spec.wing_launches().map_err(|e| e.to_string())
        })
        .collect::<Result<_, _>>()?;
    let start = terrain.free_flight_start();
    let mut situations = [distance_table::ALL; 3];
    for (bits, situation) in situations.iter_mut().zip(Situation::ALL) {
        for wings in &launches {
            *bits &= mask(availability::separations_from(
                [start[0], start[2]],
                wings,
                situation,
                bounds,
            ));
        }
    }
    let airborne = situations.iter().fold(distance_table::ALL, |m, s| m & s);
    let mut airfields = Vec::new();
    let mut unparked = Vec::new();
    for runway in crate::quick_mission::QuickMission::player_ground_runways(code, resources) {
        let mut bits = distance_table::ALL;
        for size in 1..=5 {
            let Ok(parked) = ground_layout(&terrain, runway, size) else {
                unparked.push((runway, size));
                continue;
            };
            let reference = [parked.slots[0][0], parked.slots[0][2]];
            for wings in &launches {
                bits &= mask(availability::separations_from(
                    reference,
                    wings,
                    Situation::Neutral,
                    bounds,
                ));
            }
        }
        airfields.push((runway, bits));
    }
    // A theater with no field for the player has no ground start: its
    // ground row repeats the airborne one.
    let ground = if airfields.is_empty() {
        airborne
    } else {
        airfields
            .iter()
            .fold(distance_table::ALL, |m, (_, bits)| m & bits)
    };
    Ok(Free {
        code,
        airborne,
        situations,
        ground,
        airfields,
        unparked,
    })
}

/// The AAA and SAM levels of seed `seed`: all sixteen pairs in turn.
fn defenses(seed: u32) -> (Defense, Defense) {
    let n = (seed - 1) as usize;
    (
        Defense::from_level(n % 4).expect("level"),
        Defense::from_level(n / 4 % 4).expect("level"),
    )
}

fn aimed(
    resources: &Resources,
    code: &'static str,
    stem: &'static str,
    seeds: u32,
) -> AppResult<Aimed> {
    let mut airborne = distance_table::ALL;
    let mut seed_tops = Vec::new();
    let mut limitations: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for seed in 1..=seeds {
        let mut spec = MissionSpec::new(code, AircraftId::F18);
        spec.ground_target = Some(stem.into());
        spec.surface_seed = seed;
        (spec.aaa, spec.sam) = defenses(seed);
        let audit = availability::audit(resources, &spec)?;
        let bits = mask(audit.separations);
        airborne &= bits;
        seed_tops.push((
            seed,
            distance_table::miles(bits).last().copied().unwrap_or(0),
        ));
        for limitation in audit.limitations {
            limitations.entry(limitation).or_default().push(seed);
        }
    }
    Ok(Aimed {
        code,
        stem,
        airborne,
        seed_tops,
        limitations,
    })
}

fn area(resources: &Resources, code: &'static str, seeds: u32) -> AppResult<Area> {
    let mut limitations: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for seed in 1..=seeds {
        let mut spec = MissionSpec::new(code, AircraftId::F18);
        spec.surface_seed = seed;
        (spec.aaa, spec.sam) = (Defense::Heavy, Defense::Heavy);
        for limitation in availability::audit(resources, &spec)?.limitations {
            limitations.entry(limitation).or_default().push(seed);
        }
    }
    Ok(Area { code, limitations })
}

/// The seeds the checked-in table was measured at, per target.
const SEEDS: u32 = 64;

struct Options {
    jobs: usize,
    seeds: u32,
    write: Option<std::path::PathBuf>,
    check: bool,
    measure: bool,
    theaters: Vec<&'static str>,
}

fn options() -> AppResult<Options> {
    let mut options = Options {
        jobs: 4,
        seeds: SEEDS,
        write: None,
        check: false,
        measure: false,
        theaters: Vec::new(),
    };
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--jobs" => options.jobs = value()?.parse::<usize>()?.clamp(1, 8),
            "--seeds" => options.seeds = value()?.parse::<u32>()?.max(1),
            "--write" => options.write = Some(value()?.into()),
            "--check" => options.check = true,
            "--measure" => options.measure = true,
            code => match tables::THEATERS
                .iter()
                .find(|t| t.eq_ignore_ascii_case(code))
            {
                Some(code) => options.theaters.push(code),
                None => {
                    return Err(format!(
                        "{code}: not a creator theater ({}), nor --jobs N, --seeds N, --write FILE, --check or --measure",
                        tables::THEATERS.join(" ")
                    )
                    .into());
                }
            },
        }
    }
    if options.theaters.is_empty() {
        options.theaters = tables::THEATERS.to_vec();
    }
    if options.write.is_some() && options.theaters.len() != tables::THEATERS.len() {
        return Err("--write needs every theater".into());
    }
    Ok(options)
}

/// Runs the jobs on `workers` threads, the slowest kinds first.
fn measure(
    resources: &Resources,
    jobs: Vec<Job>,
    workers: usize,
    seeds: u32,
) -> AppResult<Vec<Outcome>> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let failure = Mutex::new(None::<String>);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(index).copied() else {
                        break;
                    };
                    let started = std::time::Instant::now();
                    let (label, outcome) = match job {
                        Job::Free(code) => {
                            (code.to_string(), free(resources, code).map(Outcome::Free))
                        }
                        Job::Aimed(code, stem) => (
                            format!("{code} {stem}"),
                            aimed(resources, code, stem, seeds).map(Outcome::Aimed),
                        ),
                        Job::Area(code) => (
                            format!("{code} area"),
                            area(resources, code, seeds.min(4)).map(Outcome::Area),
                        ),
                    };
                    match outcome {
                        Ok(outcome) => {
                            eprintln!(
                                "qmc-distance-table: {label} measured in {:.1} s",
                                started.elapsed().as_secs_f64()
                            );
                            results.lock().unwrap().push(outcome);
                        }
                        Err(error) => {
                            *failure.lock().unwrap() = Some(format!("{label}: {error}"));
                            next.store(jobs.len(), Ordering::Relaxed);
                        }
                    }
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap() {
        return Err(error.into());
    }
    Ok(results.into_inner().unwrap())
}

/// A target's limitations that every measured seed had: the creator shows
/// them as the target's note.
fn constant_limitations(target: &Aimed) -> Vec<&str> {
    target
        .limitations
        .iter()
        .filter(|(_, seeds)| seeds.len() == target.seed_tops.len())
        .map(|(text, _)| text.as_str())
        .collect()
}

/// The generated `distance_table_data.rs`.
fn source(free: &[&Free], aimed: &[&Aimed]) -> String {
    let mut out = String::from(
        "// Generated by `tore-app --qmc-distance-table --write`; do not edit by hand.\n\
         // See docs/DEVELOPMENT.md, \"Quick Mission distance table\".\n\n\
         /// Every creator theater, in the creator's order.\n\
         pub const THEATERS: [Theater; 16] = [\n",
    );
    for row in free {
        out.push_str(&format!(
            "    Theater {{\n        code: \"{}\",\n        airborne: {:#05x},\n        ground: {:#05x},\n    }},\n",
            row.code, row.airborne, row.ground
        ));
    }
    out.push_str(
        "];\n\n/// Ground targets whose airborne distances differ from their theater's.\npub const TARGETS: &[Target] = &[\n",
    );
    for target in aimed {
        let theater = free
            .iter()
            .find(|f| f.code == target.code)
            .expect("theater row");
        if target.airborne != theater.airborne {
            out.push_str(&format!(
                "    Target {{\n        theater: \"{}\",\n        stem: \"{}\",\n        airborne: {:#05x},\n    }},\n",
                target.code, target.stem, target.airborne
            ));
        }
    }
    out.push_str(
        "];\n\n/// What a ground target's layout always leaves out, the creator's note for it.\npub const NOTES: &[Note] = &[\n",
    );
    for target in aimed {
        let text = constant_limitations(target).join(" ");
        if !text.is_empty() {
            out.push_str(&format!(
                "    Note {{\n        theater: \"{}\",\n        stem: \"{}\",\n        text: {text:?},\n    }},\n",
                target.code, target.stem
            ));
        }
    }
    out.push_str("];\n");
    out
}

pub fn run() -> AppResult<()> {
    let options = options()?;
    let resources =
        crate::assets::Assets::load(&crate::assets::data_directory()?)?.theater_resources;
    if options.measure {
        return cost(&resources);
    }
    let mismatches = measure_and_report(&resources, &options)?;
    if options.check {
        if !mismatches.is_empty() {
            return Err(format!(
                "the checked-in distance table differs from the measurement:\n{}",
                mismatches.join("\n")
            )
            .into());
        }
        println!("qmc-distance-table: the checked-in table matches");
    }
    Ok(())
}

/// This thread's CPU time, from Linux's scheduler statistics; `None`
/// elsewhere.
fn thread_cpu() -> Option<std::time::Duration> {
    let text = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    let ns = text.split_whitespace().next()?.parse().ok()?;
    Some(std::time::Duration::from_nanos(ns))
}

/// `--measure`: what one creator setting change cost before the table (the
/// placement audit the creator ran on its own thread for every change of
/// theater, target, start, weather or wings) and what it costs now (a table
/// lookup and the distance's fit), wall and CPU time on this thread.
fn cost(resources: &Resources) -> AppResult<()> {
    let cases: [(&str, Option<&str>, bool); 5] = [
        ("KURILE", None, false),
        ("UKR", None, false),
        ("UKR", Some("QUCITY"), false),
        ("BAL", Some("QBFLT"), false),
        ("VLA", None, true),
    ];
    for (code, target, defended) in cases {
        let mut spec = MissionSpec::new(code, AircraftId::F18);
        spec.ground_target = target.map(str::to_owned);
        spec.surface_seed = 7;
        if defended {
            (spec.aaa, spec.sam) = (Defense::Heavy, Defense::Heavy);
        }
        for (wing, count) in spec.wings[3..].iter_mut().zip([4, 2, 2]) {
            wing.count = count;
        }
        for run in 0..4 {
            spec.surface_seed = 7 + run;
            let (wall, cpu) = (std::time::Instant::now(), thread_cpu());
            std::hint::black_box(availability::audit(resources, &spec)?);
            let cpu = cpu
                .zip(thread_cpu())
                .map(|(a, b)| (b - a).as_secs_f64() * 1e3);
            println!(
                "before {code} {} run {run}: wall {:.1} ms, cpu {} ms",
                target.unwrap_or(if defended { "area" } else { "-" }),
                wall.elapsed().as_secs_f64() * 1e3,
                cpu.map_or("?".into(), |ms| format!("{ms:.1}"))
            );
        }
        const CALLS: u32 = 1_000_000;
        let (wall, cpu) = (std::time::Instant::now(), thread_cpu());
        for i in 0..CALLS {
            let mask = distance_table::allowed(
                std::hint::black_box(code),
                std::hint::black_box(target),
                std::hint::black_box(i % 2 == 0),
            );
            std::hint::black_box(distance_table::clamp_index(mask, (i % 11) as usize));
        }
        let cpu = cpu
            .zip(thread_cpu())
            .map(|(a, b)| (b - a).as_secs_f64() * 1e9 / f64::from(CALLS));
        println!(
            "after {code} {}: {CALLS} lookups, wall {:.1} ns, cpu {} ns each",
            target.unwrap_or(if defended { "area" } else { "-" }),
            wall.elapsed().as_secs_f64() * 1e9 / f64::from(CALLS),
            cpu.map_or("?".into(), |ns| format!("{ns:.1}"))
        );
    }
    Ok(())
}

/// Measures, prints the rows, writes the table when asked, and returns how
/// the checked-in table differs from the measurement.
fn measure_and_report(resources: &Resources, options: &Options) -> AppResult<Vec<String>> {
    // The target jobs are the slow ones: start them first.
    let mut jobs: Vec<Job> = options
        .theaters
        .iter()
        .flat_map(|code| {
            ground_targets(code)
                .iter()
                .map(|stem| Job::Aimed(code, stem))
        })
        .collect();
    jobs.extend(options.theaters.iter().map(|code| Job::Area(code)));
    jobs.extend(options.theaters.iter().map(|code| Job::Free(code)));
    let started = std::time::Instant::now();
    let outcomes = measure(resources, jobs, options.jobs, options.seeds)?;
    let order = |code: &str| tables::THEATERS.iter().position(|t| *t == code);
    let mut free: Vec<&Free> = Vec::new();
    let mut aimed: Vec<&Aimed> = Vec::new();
    let mut areas: Vec<&Area> = Vec::new();
    for outcome in &outcomes {
        match outcome {
            Outcome::Free(f) => free.push(f),
            Outcome::Aimed(a) => aimed.push(a),
            Outcome::Area(a) => areas.push(a),
        }
    }
    free.sort_by_key(|f| order(f.code));
    areas.sort_by_key(|a| order(a.code));
    aimed.sort_by_key(|a| {
        (
            order(a.code),
            ground_targets(a.code).iter().position(|s| *s == a.stem),
        )
    });
    let mut mismatches = Vec::new();
    for row in &free {
        let names = Situation::ALL.map(Situation::name);
        println!(
            "theater {} airborne {} ({}) ground {}{}",
            row.code,
            list(row.airborne),
            names
                .iter()
                .zip(row.situations)
                .map(|(name, bits)| format!("{name} {}", list(bits)))
                .collect::<Vec<_>>()
                .join("; "),
            list(row.ground),
            if gap(row.airborne) || gap(row.ground) {
                " GAP"
            } else {
                ""
            },
        );
        for (runway, bits) in &row.airfields {
            println!("  airfield {} {runway:#x}: {}", row.code, list(*bits));
        }
        for (runway, size) in &row.unparked {
            println!(
                "  airfield {} {runway:#x}: cannot park a wing of {size}",
                row.code
            );
        }
        if let Some(table) = distance_table::THEATERS.iter().find(|t| t.code == row.code)
            && (table.airborne != row.airborne || table.ground != row.ground)
        {
            mismatches.push(format!(
                "{}: table airborne {} ground {}, measured airborne {} ground {}",
                row.code,
                list(table.airborne),
                list(table.ground),
                list(row.airborne),
                list(row.ground)
            ));
        }
    }
    for target in &aimed {
        let tops: BTreeSet<u32> = target.seed_tops.iter().map(|(_, top)| *top).collect();
        println!(
            "target {} {} airborne {} (largest per seed: {}){}",
            target.code,
            target.stem,
            list(target.airborne),
            tops.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
            if gap(target.airborne) { " GAP" } else { "" },
        );
        for (limitation, seeds) in &target.limitations {
            println!(
                "  limitation {} {} in {} of {} seeds: {limitation}",
                target.code,
                target.stem,
                seeds.len(),
                target.seed_tops.len()
            );
        }
        let note = constant_limitations(target).join(" ");
        if distance_table::note(target.code, target.stem).unwrap_or("") != note {
            mismatches.push(format!(
                "{} {}: table note {:?}, measured {note:?}",
                target.code,
                target.stem,
                distance_table::note(target.code, target.stem)
            ));
        }
        let table = distance_table::allowed(target.code, Some(target.stem), false);
        if table != target.airborne {
            mismatches.push(format!(
                "{} {}: table {}, measured {}",
                target.code,
                target.stem,
                list(table),
                list(target.airborne)
            ));
        }
    }
    for area in &areas {
        for (limitation, seeds) in &area.limitations {
            println!(
                "  limitation {} area defenses in {} seeds: {limitation}",
                area.code,
                seeds.len()
            );
        }
    }
    println!(
        "qmc-distance-table: {} theaters, {} targets, {} seeds each, {:.1} s",
        free.len(),
        aimed.len(),
        options.seeds,
        started.elapsed().as_secs_f64()
    );
    if let Some(path) = &options.write {
        std::fs::write(path, source(&free, &aimed))?;
        println!("qmc-distance-table: wrote {}", path.display());
    }
    Ok(mismatches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_read_as_mile_lists_and_holes_are_gaps() {
        assert_eq!(mask([true; 11]), distance_table::ALL);
        assert_eq!(list(0x3ff), "1,2,5,10,20,50,75,100,150,200");
        assert_eq!(list(0), "none");
        assert!(!gap(0x1ff) && !gap(distance_table::ALL) && !gap(0));
        assert!(gap(0b101));
        assert_eq!(defenses(1), (Defense::None, Defense::None));
        assert_eq!(defenses(16), (Defense::Heavy, Defense::Heavy));
        assert_eq!(defenses(17), (Defense::None, Defense::None));
        assert_eq!(formations().count(), 216);
    }

    /// The checked-in table against a fresh measurement of the Kurile and
    /// Ukraine theaters, every target at the table's 64 seeds. Needs an
    /// imported profile: `TORE_DATA_DIR=<profile> cargo test -p tore-app
    /// qmc_distance -- --ignored`.
    #[test]
    #[ignore = "requires TORE_DATA_DIR with an imported profile"]
    fn the_checked_in_table_matches_a_fresh_measurement() {
        let resources = crate::assets::Assets::load(&crate::assets::data_directory().unwrap())
            .unwrap()
            .theater_resources;
        let options = Options {
            jobs: 4,
            seeds: SEEDS,
            write: None,
            check: true,
            measure: false,
            theaters: vec!["KURILE", "UKR"],
        };
        let mismatches = measure_and_report(&resources, &options).unwrap();
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
