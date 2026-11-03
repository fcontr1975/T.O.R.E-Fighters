//! Bounded combat-service replay: the tape file's writer and reader. Records
//! explicit launcher/environment inputs, not a native replay or a replacement
//! for the separate pilot-input tape. The names it records are `combat_tape`.
use crate::{
    AppResult,
    combat_tape::{Entry, airport_command, command},
    terrain::Terrain,
};
use std::{
    collections::BTreeMap,
    io::{BufRead, Write},
    path::Path,
};
use tore_sim::{
    attitude::{Basis, dot},
    combat::live::{Configuration, Launcher, OwnshipInput, State},
    sensors::{Channel, Controls, RANGE_LADDER_NMI},
};

/// Version 4 defines the spec missile rules and records world velocity and bay
/// permission. Mode, heat and emitter changes are explicit commands. Versions
/// 2/3 retain compatibility rules and their original control defaults.
/// Version 7 distinguishes a present falling wreck from a removed body.
/// Version 8 marks tapes recorded since bombs fall on the bomb equation and
/// rocket pods hold their rockets (docs/formats/weapons.md, "Bomb motion"):
/// its records are version 7's, and an older tape that dropped bombs or fired
/// pods replays with the new motion, so its outcome can differ.
const VERSION: u32 = 8;

pub struct Recorder {
    out: std::io::BufWriter<std::fs::File>,
    count: usize,
    error: Option<std::io::Error>,
}
fn fingerprint(data: &BTreeMap<String, Vec<u8>>) -> u64 {
    // Includes terrain and all imported dependencies. Stable FNV-1a, identity
    // guard only (not a cryptographic integrity guarantee).
    let mut h = 0xcbf29ce484222325u64;
    for (name, bytes) in data {
        // Virtual donor aliases and canonical authored tanks are loader
        // features, not changed retail media. Preserve old tapes' import
        // identity; edited variant bytes still participate in the guard.
        if tore_formats::weapons::corrections::resource_donor(name, data)
            .is_some_and(|donor| data.get(&donor) == Some(bytes))
            || tore_formats::aircraft::stations::resource(name).as_ref() == Some(bytes)
        {
            continue;
        }
        for b in name.bytes().chain([0]).chain(bytes.iter().copied()) {
            h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
    }
    h
}
impl Recorder {
    pub fn new(
        path: &Path,
        data: &BTreeMap<String, Vec<u8>>,
        config: &Configuration,
        theater: &str,
    ) -> AppResult<Self> {
        let mut out = std::io::BufWriter::new(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
        );
        writeln!(
            out,
            "tore-combat {VERSION} {:?} {} {:016x}",
            config.aircraft,
            theater,
            fingerprint(data)
        )?;
        Ok(Self {
            out,
            count: 0,
            error: None,
        })
    }
    /// Explicit model marker. Old unmarked tapes retain their historical rules.
    pub fn model(&mut self, state: &State) -> AppResult<()> {
        if state.weapon_rules == tore_sim::combat::missiles::Rules::Energy {
            writeln!(
                self.out,
                "missile-energy 1 {:016x} {}",
                tore_sim::combat::missiles::energy_profile::fingerprint(),
                state.energy_seed()
            )?;
        }
        Ok(())
    }
    pub fn record(&mut self, action: &str, l: Launcher) {
        if self.error.is_some() {
            return;
        }
        self.count += 1;
        if self.count > 432000 {
            self.error = Some(std::io::Error::other("combat tape exceeds 432000 records"));
            return;
        }
        let result = (|| {
            write!(self.out, "{action}")?;
            for v in l
                .position
                .into_iter()
                .chain(l.basis.right)
                .chain(l.basis.up)
                .chain(l.basis.forward)
                .chain([l.speed_fps])
            {
                write!(self.out, " {v}")?;
            }
            writeln!(
                self.out,
                " {} {} {} {} {} {} {} {} {} {} {} {}",
                u8::from(l.radar),
                u8::from(l.alive),
                u8::from(l.jammer),
                u8::from(l.controls.channel == Channel::Infrared),
                l.controls.range_index,
                u8::from(l.controls.history),
                l.velocity[0],
                l.velocity[1],
                l.velocity[2],
                u8::from(l.bay_ready),
                u8::from(l.radar_power),
                u8::from(l.body_present)
            )
        })();
        if let Err(e) = result {
            self.error = Some(e);
        }
    }
    /// Writes collected records, oldest first.
    pub fn write_all(&mut self, entries: impl IntoIterator<Item = Entry>) {
        for entry in entries {
            self.record(&entry.action, entry.launcher);
        }
    }
    pub fn flush(&mut self) -> AppResult<()> {
        if let Some(e) = self.error.take() {
            return Err(e.into());
        }
        self.out.flush()?;
        Ok(())
    }
}
/// Records carry the sensor controls from version 3 onward, so the field count
/// follows the header rather than being accepted either way.
fn fields_for(version: u32) -> usize {
    if version < 3 {
        17
    } else if version < 4 {
        20
    } else if version < 5 {
        24
    } else if version < 7 {
        25
    } else {
        26
    }
}
fn parse(line: &str, version: u32) -> AppResult<(&str, Launcher)> {
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() != fields_for(version) {
        return Err("invalid combat record fields".into());
    }
    let mut values = [0.; 13];
    for (out, text) in values.iter_mut().zip(&fields[1..14]) {
        *out = text.parse::<f64>()?;
        if !out.is_finite() || out.abs() > 1e9 {
            return Err("combat tape number outside bounds".into());
        }
    }
    let boolean = |s| match s {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err("invalid combat tape boolean"),
    };
    let basis = Basis {
        right: values[3..6].try_into()?,
        up: values[6..9].try_into()?,
        forward: values[9..12].try_into()?,
    };
    if [basis.right, basis.up, basis.forward]
        .iter()
        .any(|v| (dot(*v, *v) - 1.).abs() > 1e-6)
        || dot(basis.right, basis.up).abs() > 1e-6
        || dot(basis.right, basis.forward).abs() > 1e-6
        || dot(basis.up, basis.forward).abs() > 1e-6
        || !(0. ..=100000.).contains(&values[12])
    {
        return Err("invalid combat launcher basis/speed".into());
    }
    let controls = if version >= 3 {
        let range: usize = fields[18].parse()?;
        if range >= RANGE_LADDER_NMI.len() {
            return Err("combat tape scope range outside bounds".into());
        }
        Controls {
            channel: if boolean(fields[17])? {
                Channel::Infrared
            } else {
                Channel::Radar
            },
            range_index: range,
            history: boolean(fields[19])?,
        }
    } else {
        Controls::default()
    };
    Ok((
        fields[0],
        Launcher {
            position: values[..3].try_into()?,
            basis,
            bay_ready: version < 4 || boolean(fields[23])?,
            speed_fps: values[12],
            velocity: if version >= 4 {
                let mut velocity = [0.; 3];
                for (v, field) in velocity.iter_mut().zip(&fields[20..23]) {
                    *v = field.parse::<f64>()?;
                    if !v.is_finite() || v.abs() > 100000. {
                        return Err("invalid combat velocity".into());
                    }
                }
                velocity
            } else {
                basis.forward.map(|v| v * values[12])
            },
            radar_power: version < 5 || boolean(fields[24])?,
            radar: boolean(fields[14])?,
            jammer: boolean(fields[16])?,
            alive: boolean(fields[15])?,
            body_present: if version >= 7 {
                boolean(fields[25])?
            } else {
                boolean(fields[15])?
            },
            controls,
        },
    ))
}
pub fn replay(
    path: &Path,
    data: &BTreeMap<String, Vec<u8>>,
    config: Configuration,
    theater: &str,
    world: &Terrain,
) -> AppResult<State> {
    replay_reader(
        std::io::BufReader::new(std::fs::File::open(path)?),
        config.clone(),
        &format!(
            "tore-combat {VERSION} {:?} {} {:016x}",
            config.aircraft,
            theater,
            fingerprint(data)
        ),
        |x, z| f64::from(world.height(x as f32, z as f32)),
        Some(std::sync::Arc::new(world.ground_ceiling())),
        Some(&world.airport_scene),
        world.wind(),
    )
}
/// Replay a tape recorded without an airport scene, as the headless combat
/// smoke records: the range fixture has no airfields, so a replay that added the
/// theater's airport targets and ILS service would not match the live state.
pub fn replay_without_airports(
    path: &Path,
    data: &BTreeMap<String, Vec<u8>>,
    config: Configuration,
    theater: &str,
    world: &Terrain,
) -> AppResult<State> {
    replay_reader(
        std::io::BufReader::new(std::fs::File::open(path)?),
        config.clone(),
        &format!(
            "tore-combat {VERSION} {:?} {} {:016x}",
            config.aircraft,
            theater,
            fingerprint(data)
        ),
        |x, z| f64::from(world.height(x as f32, z as f32)),
        Some(std::sync::Arc::new(world.ground_ceiling())),
        None,
        world.wind(),
    )
}
fn replay_reader(
    mut reader: impl BufRead,
    config: Configuration,
    header: &str,
    ground: impl Fn(f64, f64) -> f64 + Sync,
    // The highest `ground` reaches over each square, as the host's combat
    // sets it before every step (`tore_world::combat`): host setup, not
    // recorded, so the replayed state matches the live one.
    ground_ceiling: Option<std::sync::Arc<tore_sim::ground_ceiling::GroundCeiling>>,
    airport_scene: Option<&tore_sim::airport::Scene>,
    wind: [f64; 3],
) -> AppResult<State> {
    use std::io::Read;
    let mut s = State::new(config, true)?;
    let mut buffer = Vec::new();
    let mut initialized = false;
    let mut version = VERSION;
    let mut ticks = 0;
    let mut bytes = 0;
    let mut airport_service = airport_scene
        .map(tore_sim::airport::Service::new)
        .transpose()
        .map_err(std::io::Error::other)?;
    let mut airport_nav = false;
    let mut airport_gear = false;
    let mut airport_supported = false;
    let register_airports = |state: &mut State, enabled: bool| -> AppResult<()> {
        if let Some(scene) = airport_scene.filter(|_| enabled) {
            for object in &scene.objects {
                state.add_ground_target(
                    object.id,
                    // Runways meet weapons at their pavement, as in flight.
                    tore_world::combat::weapon_contact(object, scene).unwrap_or(object.bounds),
                    object.hit_points,
                    object.category,
                    // Combat tapes predate surface sides: neutral, as recorded.
                    tore_sim::combat::live::NO_SIDE,
                )?;
                if let Some(target) = state
                    .targets
                    .iter_mut()
                    .find(|target| target.id == object.id)
                {
                    target.signature.radar = object.radar_signature;
                    target.signature.infrared = object.infrared_signature;
                }
            }
        }
        Ok(())
    };
    let mut energy_seed = None;
    let mut last_aircraft = None;
    for count in 0..=432001 {
        buffer.clear();
        let n = reader.by_ref().take(4097).read_until(b'\n', &mut buffer)?;
        if n == 0 {
            if !initialized || ticks == 0 {
                return Err("empty combat replay".into());
            }
            println!(
                "combat replay: ticks={ticks} shots={} hits={} kills={} ammo={:?} player-hp={} damage={} subsystem={:?} visual-failed={} radar-failed={} ecm-failed={}",
                s.own().shots,
                s.own().hits,
                s.own().kills,
                s.own().ammo,
                s.own().hp,
                s.own().damage,
                s.own().last_subsystem,
                s.own().visual_failed,
                s.own().radar_failed,
                s.own().ecm_failed
            );
            if let (Some(service), Some(scene), Some(aircraft)) =
                (&airport_service, airport_scene, last_aircraft)
            {
                println!(
                    "airport replay: selected={:?} clearance={:?} reply={:?} guidance={:?}",
                    service.selected(),
                    service.clearance(),
                    service.last_reply(),
                    service.guidance(scene, aircraft)
                );
            }
            return Ok(s);
        }
        bytes += n;
        if n > 4096 || count > 432000 || bytes > 256 * 1024 * 1024 {
            return Err("combat tape exceeds bounds".into());
        }
        let line = std::str::from_utf8(&buffer)?.trim();
        if count == 0 {
            // Existing version-2 tapes keep replaying; only the version token
            // differs, and their records carry the default sensor controls.
            version = (2..=VERSION)
                .find(|v| {
                    line == header.replacen(
                        &format!("tore-combat {VERSION}"),
                        &format!("tore-combat {v}"),
                        1,
                    )
                })
                .ok_or("combat tape version/aircraft/theater/assets mismatch")?;
            if version < 6 {
                airport_service = None;
            }
            continue;
        }
        if let Some(marker) = line.strip_prefix("missile-energy ") {
            let fields: Vec<_> = marker.split_whitespace().collect();
            if count != 1
                || initialized
                || version != VERSION
                || fields.len() != 3
                || fields[0] != "1"
            {
                return Err("invalid missile model marker".into());
            }
            if u64::from_str_radix(fields[1], 16)?
                != tore_sim::combat::missiles::energy_profile::fingerprint()
            {
                return Err("missile model fingerprint mismatch".into());
            }
            energy_seed = Some(fields[2].parse::<u64>()?);
            continue;
        }
        let (action, launcher) = parse(line, version)?;
        if !initialized && !matches!(action, "reset" | "reset-scene") {
            return Err("combat tape must start with reset".into());
        }
        match action {
            "reset" | "reset-scene" => {
                if action == "reset-scene" && version < 6 {
                    return Err("scene reset requires tape version6".into());
                }
                s = State::new(s.own().configuration().clone(), true)?;
                register_airports(&mut s, version >= 6)?;
                if let (Some(service), Some(scene)) = (&mut airport_service, airport_scene) {
                    service.reset(scene).map_err(std::io::Error::other)?;
                }
                airport_nav = false;
                airport_gear = false;
                airport_supported = false;
                s.weapon_rules = if let Some(seed) = energy_seed {
                    s.set_energy_seed(seed);
                    tore_sim::combat::missiles::Rules::Energy
                } else if version < 4 {
                    tore_sim::combat::missiles::Rules::Compatibility
                } else {
                    tore_sim::combat::missiles::Rules::Spec
                };
                if action == "reset" {
                    s.range_target(0, launcher);
                }
                initialized = true;
            }
            "release" => s.release(0),
            "fire" | "tick" => {
                // The host sets the mission wind before every step; a tape does
                // not record it, so the replay is given the same wind.
                s.smoke.wind = wind;
                s.devices.wind = wind;
                s.set_ground_ceiling(ground_ceiling.clone());
                let events = s.step(
                    &[OwnshipInput {
                        aircraft: 0,
                        held: action == "fire",
                        launcher,
                    }],
                    &ground,
                );
                if let (Some(service), Some(scene)) = (&mut airport_service, airport_scene) {
                    let _ = events;
                    service.synchronize_health(
                        s.targets
                            .iter()
                            .filter(|target| {
                                target.role == tore_sim::combat::missiles::TargetRole::Surface
                            })
                            .map(|target| (target.id, target.hp)),
                    );
                    let aircraft = tore_sim::airport::Aircraft {
                        position: launcher.position,
                        forward: launcher.basis.forward,
                        nav_mode: airport_nav,
                        gear_down: airport_gear,
                        supported: airport_supported,
                        alive: launcher.alive,
                        speed_fps: launcher.speed_fps,
                        ground_clearance_ft: 0.,
                        redfor: false,
                    };
                    service.step(scene, aircraft);
                    let _ = service.guidance(scene, aircraft);
                }
                ticks += 1;
            }
            "airport-nav:0" => airport_nav = false,
            "airport-nav:1" => airport_nav = true,
            _ if action.starts_with("airport-state:") => {
                let values: Vec<_> = action[14..].split(':').collect();
                if values.len() != 3 || values.iter().any(|v| !matches!(*v, "0" | "1")) {
                    return Err("invalid airport state record".into());
                }
                airport_nav = values[0] == "1";
                airport_gear = values[1] == "1";
                airport_supported = values[2] == "1";
            }
            _ if airport_command(action).is_some() => {
                let service = airport_service
                    .as_mut()
                    .ok_or("airport command without scene")?;
                let scene = airport_scene.unwrap();
                service.command(
                    scene,
                    tore_sim::airport::Aircraft {
                        position: launcher.position,
                        forward: launcher.basis.forward,
                        nav_mode: airport_nav,
                        gear_down: airport_gear,
                        supported: airport_supported,
                        alive: launcher.alive,
                        speed_fps: launcher.speed_fps,
                        ground_clearance_ft: 0.,
                        redfor: false,
                    },
                    airport_command(action).unwrap(),
                );
            }
            _ => s.command(
                0,
                command(action).ok_or("unknown combat tape action")?,
                launcher,
            ),
        }
        last_aircraft = Some(tore_sim::airport::Aircraft {
            position: launcher.position,
            forward: launcher.basis.forward,
            nav_mode: airport_nav,
            gear_down: airport_gear,
            supported: airport_supported,
            alive: launcher.alive,
            speed_fps: launcher.speed_fps,
            ground_clearance_ft: 0.,
            redfor: false,
        });
    }
    Err("combat tape record bound".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn virtual_stores_preserve_old_tape_assets_but_changed_records_do_not() {
        let mut data = std::collections::BTreeMap::from([
            ("AA10.JT".into(), vec![1, 2, 3]),
            ("OTHER".into(), vec![4, 5]),
            ("PAVEWAY.JT".into(), vec![7, 8]),
            ("GBU29.JT".into(), vec![9, 10]),
            ("F15.PT".into(), vec![11]),
            ("F16C.PT".into(), vec![12]),
            ("F22.PT".into(), vec![13]),
            ("MIG29.PT".into(), vec![14]),
            ("RAFALE.PT".into(), vec![15]),
        ]);
        let original = super::fingerprint(&data);
        tore_formats::weapons::corrections::extend_resources(&mut data);
        assert_eq!(super::fingerprint(&data), original);
        for name in [
            "GBU12.JT", "GBU16.JT", "GBU38.JT", "F610.GAS", "F370.GAS", "F600.GAS", "M396.GAS",
            "R528.GAS",
        ] {
            let mut changed = data.clone();
            changed.get_mut(name).unwrap().push(6);
            assert_ne!(super::fingerprint(&changed), original);
        }
        data.get_mut("R27ER.JT").unwrap().push(6);
        assert_ne!(super::fingerprint(&data), original);
    }

    use super::*;
    use crate::combat_tape::command_name;
    use tore_sim::combat::live::Command;
    fn launcher(x: f64) -> Launcher {
        Launcher {
            position: [x, 5000., 0.],
            basis: Basis::new(0.1, 0., 0.),
            speed_fps: 700.,
            velocity: [0., 0., 700.],
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: Controls::default(),
        }
    }
    #[test]
    fn explicit_energy_tapes_preserve_seed_and_reject_a_different_physical_table() {
        use tore_sim::combat::missiles::Rules;
        let dir = std::env::temp_dir().join(format!("tore-tape-energy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = tore_world::test_support::combat_fixture(true)
            .own()
            .configuration()
            .clone();
        let data = BTreeMap::new();
        let terrain = tore_world::test_support::terrain();
        for rules in [Rules::Spec, Rules::Energy] {
            let path = dir.join(format!("{rules:?}"));
            let mut state = State::new(config.clone(), true).unwrap();
            state.weapon_rules = rules;
            state.set_energy_seed(987654321);
            let mut recorder = Recorder::new(&path, &data, &config, "UKR").unwrap();
            recorder.model(&state).unwrap();
            recorder.record("reset", launcher(0.));
            recorder.record("tick", launcher(0.));
            recorder.flush().unwrap();
            let result =
                replay_without_airports(&path, &data, config.clone(), "UKR", &terrain).unwrap();
            assert_eq!(result.weapon_rules, rules);
            if rules == Rules::Energy {
                assert_eq!(result.energy_seed(), 987654321);
                let text = std::fs::read_to_string(&path).unwrap().replace(
                    &format!(
                        "{:016x}",
                        tore_sim::combat::missiles::energy_profile::fingerprint()
                    ),
                    "0000000000000000",
                );
                std::fs::write(&path, text).unwrap();
                let error = replay_without_airports(&path, &data, config.clone(), "UKR", &terrain)
                    .unwrap_err();
                assert!(error.to_string().contains("model fingerprint mismatch"));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    /// Records that combat collects and the app writes are the bytes a
    /// recorder writes when given the same records directly.
    #[test]
    fn collected_records_write_the_same_bytes() {
        let dir = std::env::temp_dir().join(format!("tore-tape-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = tore_world::test_support::combat_fixture(false)
            .own()
            .configuration()
            .clone();
        let data = BTreeMap::new();
        let names = ["tick", "fire", "release", "airport-state:1:0:1", "reset"];
        let mut direct = Recorder::new(&dir.join("direct"), &data, &config, "UKR").unwrap();
        let mut drained = Recorder::new(&dir.join("drained"), &data, &config, "UKR").unwrap();
        let mut entries = Vec::new();
        for (i, name) in names.into_iter().enumerate() {
            direct.record(name, launcher(i as f64));
            entries.push(Entry {
                action: name.into(),
                launcher: launcher(i as f64),
            });
        }
        drained.write_all(entries);
        direct.flush().unwrap();
        drained.flush().unwrap();
        assert_eq!(
            std::fs::read(dir.join("direct")).unwrap(),
            std::fs::read(dir.join("drained")).unwrap()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    /// A replay gives its state the ground ceiling the host's combat sets
    /// before every step, so the replayed state is the live one; the ceiling
    /// only skips samples, so the rest of the state is the same without it.
    #[test]
    fn replay_sets_the_hosts_ground_ceiling() {
        let dir = std::env::temp_dir().join(format!("tore-tape-ceiling-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tape");
        let config = tore_world::test_support::combat_fixture(false)
            .own()
            .configuration()
            .clone();
        let data = BTreeMap::new();
        let world = tore_world::test_support::terrain();
        let mut recorder = Recorder::new(&path, &data, &config, "UKR").unwrap();
        recorder.record("reset", launcher(0.));
        for i in 0..40 {
            recorder.record(if i < 20 { "fire" } else { "tick" }, launcher(0.));
        }
        recorder.flush().unwrap();
        let replayed = format!(
            "{:?}",
            replay_without_airports(&path, &data, config.clone(), "UKR", &world).unwrap()
        );
        let without = format!(
            "{:?}",
            replay_reader(
                std::io::BufReader::new(std::fs::File::open(&path).unwrap()),
                config.clone(),
                &format!(
                    "tore-combat {VERSION} {:?} UKR {:016x}",
                    config.aircraft,
                    fingerprint(&data)
                ),
                |x, z| f64::from(world.height(x as f32, z as f32)),
                None,
                None,
                world.wind(),
            )
            .unwrap()
        );
        std::fs::remove_dir_all(dir).unwrap();
        let (kept, ceiling) = replayed.split_once("ground_ceiling: ").unwrap();
        assert!(ceiling.starts_with("Some("));
        assert_eq!(without.split_once("ground_ceiling: ").unwrap().0, kept);
    }
    #[test]
    fn version_seven_distinguishes_falling_wreck_from_absent_body() {
        let line = "tick 0 1000 0 1 0 0 0 1 0 0 0 1 600 0 0 0 0 1 0 40 60 600 1 0";
        let wreck = parse(&format!("{line} 1"), 7).unwrap().1;
        assert!(!wreck.alive && wreck.body_present);
        assert!(!parse(&format!("{line} 0"), 7).unwrap().1.body_present);
        assert!(parse(&format!("{line} 2"), 7).is_err());
        assert!(!parse(line, 6).unwrap().1.body_present);
        assert!(parse(line, 7).is_err());
    }
    #[test]
    fn version_five_preserves_power_separately_from_transmission() {
        let line = "tick 0 1000 0 1 0 0 0 1 0 0 0 1 600 0 1 0 1 1 0 40 60 600 1";
        assert!(parse(&format!("{line} 1"), 5).unwrap().1.radar_power);
        assert!(!parse(&format!("{line} 0"), 5).unwrap().1.radar_power);
        assert!(!parse(&format!("{line} 1"), 5).unwrap().1.radar);
        assert!(parse(&format!("{line} 2"), 5).is_err());
        assert!(parse(line, 4).unwrap().1.radar_power);
    }
    #[test]
    fn version_four_preserves_velocity_bay_mode_and_heat_commands() {
        let line = "tick 0 1000 0 1 0 0 0 1 0 0 0 1 600 1 1 0 0 1 0 40 60 600 0";
        let (_, launcher) = parse(line, 4).unwrap();
        assert_eq!(launcher.velocity, [40., 60., 600.]);
        assert!(!launcher.bay_ready);
        assert!(parse(&line.replace("40 60 600", "NaN 60 600"), 4).is_err());
        for c in [
            Command::NextSelection,
            Command::PreviousSelection,
            Command::SelectNav,
            Command::ToggleSeekerMode,
            Command::CompatibilityWeapons,
            Command::TargetHeat(4),
            Command::ToggleTargetRadar,
        ] {
            assert_eq!(command(&command_name(c)), Some(c));
        }
        assert_eq!(command("target-heat:5"), None);
    }
    #[test]
    fn bounded_records_reject_nonfinite_axes_and_invalid_basis() {
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 0", 2).is_ok());
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 0 1 3 1", 3).is_ok());
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 0 1 9 1", 3).is_err());
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 0 2 3 1", 3).is_err());
        // A record must match the version its header declared.
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 0 1 3 1", 2).is_err());
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 0", 3).is_err());
        assert!(parse("tick NaN 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 0", 2).is_err());
        assert!(parse("tick 0 1000 0 0 0 0 0 1 0 0 0 1 300 1 1 0", 2).is_err());
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 2 1 0", 2).is_err());
        assert!(parse("tick 0 1000 0 1 0 0 0 1 0 0 0 1 300 1 1 2", 2).is_err());
    }
    #[test]
    fn version_five_and_six_records_have_the_same_field_count() {
        assert_eq!(fields_for(5), 25);
        assert_eq!(fields_for(6), 25);
    }
}
