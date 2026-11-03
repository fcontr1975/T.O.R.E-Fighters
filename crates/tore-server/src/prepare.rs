//! Everything the server does before it opens a port: the configuration, the
//! import, the mission file and a trial build of the mission. A failure is a
//! plain message that says what to do, and nothing is started.

use crate::{
    config::{Config, OpenPlanes},
    options::Options,
};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tore_import::Resources;
use tore_world::{
    mission::MissionSpec,
    resources::{Manifest, ResourceReads},
    world::{Seating, World},
};

/// What start-up loaded and checked.
pub struct Prepared {
    pub rotation: Option<crate::rotation::Rotation>,
    pub rotation_report: Vec<String>,
    pub config: Config,
    pub data_dir: PathBuf,
    pub spec: MissionSpec,
    pub resources: Arc<Resources>,
    /// What the trial build of the mission read, with hashes.
    pub manifest: Manifest,
    /// The trial build, for `--check` and for counting the aircraft. The host
    /// builds its own.
    pub world: World,
    pub placement_initial_seed: u32,
    pub placement_preview_seed: u32,
    pub placement_attempts: usize,
    /// The log line when the mission's `separation-nm` was not one the
    /// creator offers for its theater, target and start, and moved
    /// (`distance_table::adjust`).
    pub distance_note: Option<String>,
}

/// The default configuration file in a data folder.
pub fn default_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("server.conf")
}

/// Refuses the game's retail stall-speed switch, on the command line or in the
/// environment: every machine in a session must fly one configuration, and
/// the players' games do not have the switch on.
pub fn refuse_retail_stall_speeds(
    options: &Options,
    environment: Option<OsString>,
) -> Result<(), String> {
    if options.retail_stall_speeds || environment.is_some() {
        let how = if options.retail_stall_speeds {
            "--retail-stall-speeds"
        } else {
            "TORE_RETAIL_STALL_SPEEDS"
        };
        return Err(format!(
            "{how} is on. It turns the weight-scaled stall speed off, and every machine in a session must fly one configuration, so the server will not start with it. Remove it and start again."
        ));
    }
    Ok(())
}

/// Reads the configuration file the options name, applying `--port` and
/// `--mission`. The default file is `server.conf` in the data folder; without
/// one the defaults apply.
pub fn load_config(options: &Options, data_dir: &Path) -> Result<Config, String> {
    let mut config = match &options.config {
        Some(path) => read_config(path)?,
        None => {
            let path = default_config_path(data_dir);
            if path.exists() {
                read_config(&path)?
            } else {
                Config::defaults(data_dir)
            }
        }
    };
    if let Some(port) = options.port {
        config.port = port;
    }
    if let Some(mission) = &options.mission {
        if config.rotation.is_some() {
            return Err("`--mission` and `rotation` are mutually exclusive".into());
        }
        config.mission = mission.clone();
    }
    Ok(config)
}

fn read_config(path: &Path) -> Result<Config, String> {
    let text = tore_import::files::read(path).map_err(|error| {
        format!(
            "Cannot read the configuration file {}: {error}",
            path.display()
        )
    })?;
    let base = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Config::parse(&text, base).map_err(|error| format!("{}: {error}", path.display()))
}

/// Loads the newest import in the data folder, pruning older ones as the game
/// does. A server needs only the simulation's data, so the check is the
/// import's own markers.
pub fn load_resources(data_dir: &Path) -> Result<Resources, String> {
    let has_pack = fs::read_dir(data_dir).is_ok_and(|entries| {
        entries.filter_map(Result::ok).any(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("menu-") && name.ends_with(".pack")
        })
    });
    if !has_pack {
        return Err(format!(
            "There is no imported game in {}. Import Fighters Anthology first: tore-server --import /path/to/FIGHTERS (an installed game folder or a disc folder), or point --data-dir at the folder the game imported into.",
            data_dir.display()
        ));
    }
    tore_import::load_with(data_dir, &|resources| {
        tore_import::check_markers(resources)
    })
    .map(|loaded| loaded.resources)
    .map_err(|error| {
        let text = error.to_string();
        if text.contains("re-import") {
            format!(
                "The import in {} is out of date ({text}). Import again: tore-server --import /path/to/FIGHTERS",
                data_dir.display()
            )
        } else {
            format!(
                "The import in {} cannot be used ({text}). Import again: tore-server --import /path/to/FIGHTERS",
                data_dir.display()
            )
        }
    })
}

/// Reads and parses a mission file; an error names the file and its line.
pub fn load_mission(path: &Path) -> Result<MissionSpec, String> {
    let text = tore_import::files::read(path)
        .map_err(|error| format!("Cannot read the mission file {}: {error}", path.display()))?;
    MissionSpec::from_text(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Loads everything and builds the mission once to prove it can be built.
pub fn prepare(options: &Options, data_dir: &Path) -> Result<Prepared, String> {
    let config = load_config(options, data_dir)?;
    let mut resources = load_resources(data_dir)?;
    add_source(data_dir, &mut resources);
    // City layouts placed once are read back on the next start.
    tore_world::city::cache::configure(data_dir);
    if config.rotation.is_some() {
        let mut rotation = crate::rotation::Rotation::read(&config)?;
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        rotation.start(seed);
        let resources = Arc::new(resources);
        let mut first = None;
        let mut report = vec![rotation.heading()];
        let mut errors = Vec::new();
        for (i, entry) in rotation.entries.iter().enumerate() {
            let prepared = match load_mission(&entry.config.mission).and_then(|spec| {
                build_shared(entry.config.clone(), data_dir, spec, resources.clone())
            }) {
                Ok(prepared) => prepared,
                Err(why) => {
                    errors.push(format!(
                        "Rotation entry {} (line {}): {why}",
                        i + 1,
                        entry.line
                    ));
                    continue;
                }
            };
            report.push(format!(
                "Rotation {}/{}: {}",
                i + 1,
                rotation.entries.len(),
                prepared.spec.summary()
            ));
            report.extend(prepared.distance_note.clone());
            report.extend(crate::rotation::effective_settings(&entry.config));
            report.extend(crate::check::defense_report(&prepared.world));
            if i == rotation.current {
                first = Some(prepared);
            }
        }
        if !errors.is_empty() {
            return Err(format!("{}\n{}", report.join("\n"), errors.join("\n")));
        }
        let mut prepared = first.unwrap();
        prepared.rotation = Some(rotation);
        prepared.rotation_report = report;
        Ok(prepared)
    } else {
        let spec = load_mission(&config.mission)?;
        build(config, data_dir, spec, resources)
    }
}

/// Stage L: an import made before the import recorded its source has no
/// `TORE_SOURCE_V1` entry; its build is then read from the import report
/// beside the pack and added to the loaded resources as that entry, so the
/// host, which reads the source from its resources, says the right build
/// (agent decision: the host is given resources, not the data folder).
/// Nothing a mission builds reads the entry. A pack with the entry, or a
/// report that names no build, is left as it is.
pub fn add_source(data_dir: &Path, resources: &mut Resources) {
    use tore_import::source::{RESOURCE, Source};
    if resources.contains_key(RESOURCE) {
        return;
    }
    let source = Source::read(data_dir, resources);
    if source.build.is_some() {
        resources.insert(RESOURCE.to_owned(), source.encode());
    }
}

/// The trial build and the checks that need the built mission.
pub fn build(
    config: Config,
    data_dir: &Path,
    spec: MissionSpec,
    resources: Resources,
) -> Result<Prepared, String> {
    build_shared(config, data_dir, spec, Arc::new(resources))
}

pub fn build_shared(
    config: Config,
    data_dir: &Path,
    spec: MissionSpec,
    resources: Arc<Resources>,
) -> Result<Prepared, String> {
    let mut settings = tore_session::settings::Store::defaults(tore_session::settings::Mode::Coop);
    settings
        .apply(&config.settings)
        .map_err(|e| e.to_string())?;
    let mut spec = tore_session::host::sequence::configured_spec(spec, &settings);
    // The fixed table, checked without loading terrain: a distance the
    // creator would not offer moves as the creator moves it, and says so.
    let distance_note = tore_world::mission_layout::distance_table::adjust(&mut spec)
        .map(|moved| tore_world::mission_layout::distance_table::adjust_notice(&spec, moved));
    let placement_initial_seed = spec.surface_seed;
    let mut preview = spec.clone();
    let ((world, manifest), placement_attempts) =
        tore_world::mission_layout::seed_retry::build(&mut preview, |candidate| {
            let reads = ResourceReads::new(&resources);
            let world =
                World::new(candidate, &reads, Seating::Open).map_err(|error| error.to_string())?;
            Ok((world, reads.manifest()))
        })
        .map_err(|error| {
            let hint = if spec.ground_runway().is_some() {
                " (`start ground` takes a runway number from --check)"
            } else {
                ""
            };
            format!(
                "The mission in {} cannot be built from this import: {error}{hint}",
                config.mission.display()
            )
        })?;
    validate_world(&config, &preview, &world)?;
    Ok(Prepared {
        rotation: None,
        rotation_report: Vec::new(),
        config,
        data_dir: data_dir.to_owned(),
        spec,
        resources,
        manifest,
        world,
        placement_initial_seed,
        placement_preview_seed: preview.surface_seed,
        placement_attempts,
        distance_note,
    })
}

/// Checks shared by startup, reload, and a runtime rotation build.
pub fn validate_world(config: &Config, spec: &MissionSpec, world: &World) -> Result<(), String> {
    if let Some(problem) = crate::check::ground_start_problem(spec, &world.terrain) {
        return Err(format!(
            "The mission in {} {problem}.",
            config.mission.display()
        ));
    }
    if let OpenPlanes::List(planes) = &config.open_planes {
        let count = world.roster.planes().len() as u32;
        if let Some(plane) = planes.iter().find(|plane| **plane >= count) {
            return Err(format!(
                "open-planes names plane {plane}, but the mission has planes 0 to {}. --check lists them.",
                count.saturating_sub(1)
            ));
        }
    }
    let min = config
        .settings
        .iter()
        .find(|(n, _)| *n == tore_session::settings::number::MIN_PLAYERS)
        .map_or(0, |&(_, v)| v);
    let open = open_slots(config, spec);
    if min > open {
        let at = config
            .min_players_at
            .map_or(String::new(), |(source, line)| {
                format!(" (`min-players` is on line {line} of {})", source.words())
            });
        return Err(format!(
            "min-players is {min}, but the mission in {} has only {open} open slot{} for players{at}. --check lists the planes; `open-planes` and `mode` change which are open.",
            config.mission.display(),
            if open == 1 { "" } else { "s" }
        ));
    }
    Ok(())
}

/// How many planes of `spec` players may take under `config`: the friendly
/// planes (both sides in PvP) or the ones `open-planes` names.
pub fn open_slots(config: &Config, spec: &MissionSpec) -> u32 {
    use tore_session::settings::{Mode, number};
    let pvp = config
        .settings
        .iter()
        .any(|&(n, v)| n == number::MODE && v == Mode::Pvp.value());
    spec.open_planes()
        .iter()
        .filter(|plane| match &config.open_planes {
            OpenPlanes::Friendly => !plane.wing.side.is_enemy() || pvp,
            OpenPlanes::All => true,
            OpenPlanes::List(planes) => planes.contains(&plane.id),
        })
        .count() as u32
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::log::tests::scratch;
    use tore_import::pack::write_pack;
    use tore_world::test_support::resources::resources;

    /// A data folder holding a synthetic import with the markers.
    pub fn data_folder(name: &str, markers: bool) -> PathBuf {
        data_folder_with(name, markers, resources())
    }

    /// [`data_folder`] holding the synthetic import `map`.
    pub fn data_folder_with(
        name: &str,
        markers: bool,
        mut map: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> PathBuf {
        let dir = scratch(name);
        fs::create_dir_all(&dir).unwrap();
        if markers {
            map.insert("TORE_MUSIC_V1".into(), b"PCM1".to_vec());
            map.insert("TORE_COMBAT_V1".into(), b"RAW1".to_vec());
            map.insert("TORE_AIRPORTS_V1".into(), b"SCENE1".to_vec());
            map.insert("TORE_SPEECH_V1".into(), b"ALL1".to_vec());
            map.insert(
                tore_import::surface::MARKER.into(),
                tore_import::surface::MARKER_VALUE.to_vec(),
            );
        }
        write_pack(&dir.join("menu-1.pack"), &map).unwrap();
        dir
    }

    pub const MISSION: &str = "tore-mission 1\ntheater UKR\nstart airborne 10000\nseparation-nm 2\nwing friendly 1 F18.PT 2 average\nwing enemy 1 F18.PT 2 average\n";

    #[test]
    fn the_retail_stall_switch_is_refused_either_way() {
        let flag = Options {
            retail_stall_speeds: true,
            ..Default::default()
        };
        assert!(
            refuse_retail_stall_speeds(&flag, None)
                .unwrap_err()
                .contains("--retail-stall-speeds is on")
        );
        assert!(
            refuse_retail_stall_speeds(&Options::default(), Some("1".into()))
                .unwrap_err()
                .contains("TORE_RETAIL_STALL_SPEEDS is on")
        );
        assert!(refuse_retail_stall_speeds(&Options::default(), None).is_ok());
    }

    #[test]
    fn a_missing_import_says_how_to_make_one() {
        let dir = scratch("prep-none");
        fs::create_dir_all(&dir).unwrap();
        let error = load_resources(&dir).unwrap_err();
        assert!(error.contains("no imported game"), "{error}");
        assert!(error.contains("--import"), "{error}");
        let missing = load_resources(&dir.join("absent")).unwrap_err();
        assert!(missing.contains("no imported game"), "{missing}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stale_import_says_to_import_again() {
        let dir = data_folder("prep-stale", false);
        let error = load_resources(&dir).unwrap_err();
        assert!(error.contains("out of date"), "{error}");
        assert!(error.contains("tore-server --import"), "{error}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_good_import_loads() {
        let dir = data_folder("prep-good", true);
        assert!(load_resources(&dir).unwrap().contains_key("UKR.T2"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_bad_mission_line_is_refused_with_its_number() {
        let dir = scratch("prep-mission");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mission.txt");
        fs::write(
            &path,
            "tore-mission 1\ntheater UKR\ncondition clear\ntheater MOON\n",
        )
        .unwrap();
        let error = load_mission(&path).unwrap_err();
        assert!(error.contains("mission.txt: line 4"), "{error}");
        fs::write(
            &path,
            "tore-mission 1\nwing friendly 1 F18.PT 2 average\ntheater MOON\n",
        )
        .unwrap();
        assert!(load_mission(&path).unwrap_err().contains("line 3"));
        assert!(
            load_mission(&dir.join("none.txt"))
                .unwrap_err()
                .contains("Cannot read the mission file")
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_options_override_the_configuration_and_the_default_file_is_optional() {
        let dir = scratch("prep-config");
        fs::create_dir_all(&dir).unwrap();
        // No server.conf: defaults, the mission beside it.
        let config = load_config(&Options::default(), &dir).unwrap();
        assert_eq!(config, Config::defaults(&dir));
        fs::write(dir.join("server.conf"), "port 27000\nmission duel.txt\n").unwrap();
        let config = load_config(&Options::default(), &dir).unwrap();
        assert_eq!(
            (config.port, config.mission.clone()),
            (27000, dir.join("duel.txt"))
        );
        let options = Options {
            port: Some(28000),
            mission: Some("other.txt".into()),
            ..Default::default()
        };
        let config = load_config(&options, &dir).unwrap();
        assert_eq!(
            (config.port, config.mission),
            (28000, PathBuf::from("other.txt"))
        );
        // A named file that is missing or wrong is an error naming it.
        let named = Options {
            config: Some(dir.join("nope.conf")),
            ..Default::default()
        };
        assert!(load_config(&named, &dir).unwrap_err().contains("nope.conf"));
        fs::write(dir.join("bad.conf"), "port 1\nprot 2\n").unwrap();
        let named = Options {
            config: Some(dir.join("bad.conf")),
            ..Default::default()
        };
        let error = load_config(&named, &dir).unwrap_err();
        assert!(
            error.contains("bad.conf: line 2: `prot` is not a setting"),
            "{error}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_good_mission_builds_and_gives_its_manifest() {
        let dir = data_folder("prep-build", true);
        fs::write(dir.join("mission.txt"), MISSION).unwrap();
        let prepared = prepare(&Options::default(), &dir).unwrap();
        assert_eq!(prepared.world.roster.planes().len(), 4);
        assert!(prepared.manifest.entries.iter().any(|e| e.name == "F18.PT"));
        assert_ne!(prepared.manifest.digest(), 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_mission_naming_what_the_import_lacks_is_refused() {
        let dir = data_folder("prep-lacks", true);
        // The synthetic import has the F/A-18D and no Su-27.
        fs::write(
            dir.join("mission.txt"),
            MISSION.replace("wing enemy 1 F18.PT", "wing enemy 1 SU27.PT"),
        )
        .unwrap();
        let error = prepare(&Options::default(), &dir).err().unwrap();
        assert!(
            error.contains("cannot be built from this import"),
            "{error}"
        );
        assert!(error.contains("SU27"), "{error}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn an_old_pack_takes_its_build_from_the_import_report() {
        use tore_import::source::{Build, RESOURCE, Source};
        let dir = data_folder("prep-source", true);
        fs::write(dir.join("mission.txt"), MISSION).unwrap();
        // No entry and no report: the source stays unknown.
        let prepared = prepare(&Options::default(), &dir).unwrap();
        assert!(!prepared.resources.contains_key(RESOURCE));
        // The report names the build: the entry is added from it.
        fs::write(
            dir.join("import-report.txt"),
            "T.O.R.E import report\n\nSource: disc\nFA.EXE: 1.0 (disc) SHA-256 c7d2; 1299968 bytes\n",
        )
        .unwrap();
        let prepared = prepare(&Options::default(), &dir).unwrap();
        let source = Source::parse(&prepared.resources[RESOURCE]).unwrap();
        assert_eq!((source.build, source.importer), (Some(Build::Disc10), None));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn min_players_cannot_exceed_the_mission_s_open_slots() {
        let dir = data_folder("prep-min", true);
        // MISSION has two friendly and two enemy planes.
        fs::write(dir.join("mission.txt"), MISSION).unwrap();
        fs::write(dir.join("server.conf"), "min-players 3\n").unwrap();
        let error = prepare(&Options::default(), &dir).err().unwrap();
        assert!(
            error.contains("min-players is 3")
                && error.contains("only 2 open slots")
                && error.contains("line 1 of the configuration file"),
            "{error}"
        );
        // The same file is fine in PvP, where both sides' planes are open, and
        // with a list of open planes it counts the list.
        fs::write(dir.join("server.conf"), "min-players 3\nmode pvp\n").unwrap();
        assert!(prepare(&Options::default(), &dir).is_ok());
        fs::write(
            dir.join("server.conf"),
            "min-players 3\nopen-planes 0 1 2\n",
        )
        .unwrap();
        assert!(prepare(&Options::default(), &dir).is_ok());
        fs::write(dir.join("server.conf"), "min-players 3\nopen-planes 0\n").unwrap();
        assert!(
            prepare(&Options::default(), &dir)
                .err()
                .unwrap()
                .contains("only 1 open slot ")
        );
        fs::write(dir.join("server.conf"), "min-players 2\n").unwrap();
        assert!(prepare(&Options::default(), &dir).is_ok());
        // Each rotation entry is checked on its own, and the message names it.
        fs::write(dir.join("server.conf"), "rotation rotation.txt\n").unwrap();
        fs::write(dir.join("one.txt"), MISSION).unwrap();
        fs::write(
            dir.join("rotation.txt"),
            "tore-rotation 1\nmission one.txt\nmin-players 2\nmission one.txt\nmin-players 3\n",
        )
        .unwrap();
        let error = prepare(&Options::default(), &dir).err().unwrap();
        assert!(
            error.contains("Rotation entry 2 (line 4)")
                && error.contains("min-players is 3")
                && error.contains("line 5 of the rotation file"),
            "{error}"
        );
        assert!(!error.contains("Rotation entry 1"), "{error}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn open_planes_must_name_planes_the_mission_has() {
        let dir = data_folder("prep-open", true);
        fs::write(dir.join("mission.txt"), MISSION).unwrap();
        fs::write(dir.join("server.conf"), "open-planes 0 4\n").unwrap();
        let error = prepare(&Options::default(), &dir).err().unwrap();
        assert!(
            error.contains("plane 4") && error.contains("0 to 3"),
            "{error}"
        );
        fs::write(dir.join("server.conf"), "open-planes 0 3\n").unwrap();
        assert!(prepare(&Options::default(), &dir).is_ok());
        let _ = fs::remove_dir_all(dir);
    }
    #[test]
    fn check_preview_uses_the_same_bounded_seed_policy_as_runtime() {
        use tore_world::test_support::surface::{
            placement_retry_resources, spec_with_target, target,
        };
        let (resources, chosen) = placement_retry_resources(0);
        let spec = spec_with_target(&target("QUCITY", 0, 3, 0));
        let dir = scratch("preview-placement-seed");
        let prepared = build(Config::defaults(&dir), &dir, spec, resources).unwrap();
        assert_eq!(
            prepared.spec.surface_seed, 0,
            "runtime must still draw its first seed"
        );
        assert_eq!(prepared.placement_preview_seed, chosen);
        assert_eq!(prepared.placement_initial_seed, 0);
        assert!(prepared.placement_attempts > 1 && prepared.placement_attempts <= 16);
        let lines = crate::check::report(&prepared).join("\n");
        assert!(lines.contains(&format!("Surface placement seed {chosen} selected")));
        assert!(lines.contains("preview"));
        let _ = fs::remove_dir_all(dir);
    }
}
