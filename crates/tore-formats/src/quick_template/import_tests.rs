//! Ignored import tests over a user-owned retail install. They need
//! `FA_2.LIB` and `FA.EXE`: set `TORE_GAME_DIR` to the install folder, or keep
//! the `gameassets/fighters-anthology` link. Run with
//!
//! ```text
//! cargo test -p tore-formats quick_template::import_tests -- --ignored --nocapture
//! ```
//!
//! They prove the readers accept every retail record and that the recorded
//! tables in [`super::tables`] equal what the executable holds. Nothing here
//! writes or copies retail bytes.
use super::tables::{self, addresses};
use super::*;
use crate::{
    Archive,
    executable::{self, Build},
    mission::Layout,
    surface_unit::{MountKind, SurfaceUnit, class},
    ui::creator::{Image, Options},
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn game_dir() -> PathBuf {
    std::env::var_os("TORE_GAME_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../gameassets/fighters-anthology"),
        PathBuf::from,
    )
}

fn library() -> Archive {
    Archive::open(game_dir().join("FA_2.LIB")).expect("FA_2.LIB (set TORE_GAME_DIR)")
}

fn units(lib: &Archive) -> BTreeMap<String, SurfaceUnit> {
    lib.entries
        .keys()
        .filter(|name| name.ends_with(".NT"))
        .map(|name| {
            let bytes = lib.read(name).unwrap();
            let unit = SurfaceUnit::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(&unit.resource, name);
            (name.clone(), unit)
        })
        .collect()
}

#[test]
#[ignore = "needs a retail install (TORE_GAME_DIR or the gameassets link)"]
fn all_84_surface_units_parse() {
    let lib = library();
    let units = units(&lib);
    assert_eq!(units.len(), 84);
    let by_class = |mask: u16| units.values().filter(|u| u.class & mask != 0).count();
    // Survey 2.1: 27 plus 5 carrier ships, 17 SAM, 8 AAA, 9 tanks, 11 vehicles.
    assert_eq!(by_class(class::SHIP), 32);
    assert_eq!(by_class(class::SAM), 17);
    assert_eq!(by_class(class::AAA), 8);
    assert_eq!(by_class(class::TANK), 9);
    assert_eq!(by_class(class::VEHICLE), 11);

    for (name, unit) in &units {
        for resource in unit
            .shape
            .iter()
            .chain(unit.shadow_shape.iter())
            .chain(unit.mounts.iter().filter_map(|m| m.store.as_ref()))
        {
            assert!(
                lib.entries.contains_key(resource),
                "{name}: {resource} missing"
            );
        }
        if unit.is_ship() {
            let wreck = unit
                .damaged_shape
                .clone()
                .expect("a ship has a damaged look");
            assert!(lib.entries.contains_key(&wreck), "{name}: {wreck} missing");
        }
        assert!(unit.hit_points > 0, "{name}");
        assert!(unit.mounts.len() <= 6);
    }
    assert!(
        lib.entries
            .contains_key(crate::surface_unit::DESTROYED_VEHICLE_OBJECT)
    );
    for truck in crate::surface_unit::TRUCK_TYPES {
        assert!(units[truck].is_truck() && !units[truck].armed(), "{truck}");
        assert_eq!(
            units[truck].is_supply_truck(),
            crate::surface_unit::SUPPLY_TRUCKS.contains(&truck)
        );
    }

    // The shoulder-fired SAM teams are exactly the five MANPADS records.
    let mut teams: Vec<&str> = units
        .iter()
        .filter(|(_, unit)| unit.is_shoulder_fired_team())
        .map(|(name, _)| name.as_str())
        .collect();
    teams.sort_unstable();
    assert_eq!(
        teams,
        ["FIM92.NT", "MIS.NT", "SA14.NT", "SA16.NT", "SA7.NT"]
    );

    // Spot values from the survey tables.
    let sa6 = &units["SA6.NT"];
    assert_eq!(
        (
            sa6.hit_points,
            sa6.npc.search_frequency,
            sa6.npc.unready_attack,
            sa6.npc.attack
        ),
        (100, 40, 144, 60)
    );
    assert_eq!(sa6.mounts[0].store.as_deref(), Some("SA6.JT"));
    assert_eq!(sa6.mounts[0].limit_degrees(), [0.0, 70.0]);
    let sa2 = &units["SA2A.NT"];
    assert_eq!(sa2.hit_points, 650);
    assert_eq!(sa2.weapons().count(), 6);
    let nimz = &units["NIMZ.NT"];
    assert_eq!(nimz.callback, "_CARRIERProc");
    assert_eq!(nimz.mounts.len(), 4);
    assert_eq!(nimz.mounts[1].slew_degrees(), [180.0, 0.0]);
    assert_eq!(nimz.damaged_shape.as_deref(), Some("NIMZ_A.SH"));
    let gci = &units["GCI.NT"];
    assert_eq!(gci.sensor.as_deref(), Some("GCIR.SEE"));
    assert_eq!(units["BUTLER.NT"].sensor.as_deref(), Some("REDCR.SEE"));
    assert_eq!(units["BUTLER.NT"].weapons().count(), 2);
    assert_eq!(units["A_M1939.NT"].npc.zone_dist, 195);
    assert!(units["A_M1939.NT"].shape.is_none());
    assert_eq!(units["KS19.NT"].npc.retarget, 40);
    assert_eq!(units["SARAN.NT"].npc.script.as_deref(), Some("HYDRO.BI"));
    assert_eq!(
        units.values().filter(|u| u.sensor.is_some()).count(),
        2,
        "GCI and Red Crown"
    );
    let missiles = units
        .values()
        .flat_map(|u| u.weapons())
        .filter(|(_, m)| m.ammo() != crate::surface_unit::Ammo::Unlimited)
        .count();
    // Six SA-2 rails and one rack on each of 16 other launchers.
    assert_eq!(missiles, 22);
    eprintln!("84 NTs parsed, {missiles} finite weapon mounts");
}

#[test]
#[ignore = "needs a retail install (TORE_GAME_DIR or the gameassets link)"]
fn all_129_templates_parse_and_match_the_survey() {
    let lib = library();
    let units = units(&lib);
    let names: Vec<_> = lib
        .entries
        .keys()
        .filter(|n| n.starts_with("~Q") && n.ends_with(".M"))
        .cloned()
        .collect();
    assert_eq!(names.len(), 129);
    let templates: BTreeMap<_, _> = names
        .iter()
        .map(|name| {
            let t = Template::parse(name, &lib.read(name).unwrap())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            (t.stem.clone(), t)
        })
        .collect();
    // The recorded names are exactly the archive's templates: 124 offered and
    // 5 left over.
    let mut recorded: Vec<_> = tables::TEMPLATES
        .iter()
        .flat_map(|l| l.iter().copied())
        .chain(tables::UNREFERENCED)
        .collect();
    recorded.sort_unstable();
    let mut archive: Vec<_> = templates.keys().map(String::as_str).collect();
    archive.sort_unstable();
    assert_eq!(recorded, archive);

    let all: Vec<_> = templates.values().flat_map(|t| &t.objects).collect();
    assert_eq!(all.len(), 5315);
    let count = |p: Placeholder| all.iter().filter(|o| o.placeholder() == Some(p)).count();
    for (p, n) in [
        (Placeholder::Sam, 891),
        (Placeholder::Aaa, 879),
        (Placeholder::Tank, 298),
        (Placeholder::Afv, 244),
        (Placeholder::Destroyer, 44),
        (Placeholder::Cargo, 31),
        (Placeholder::Cruiser, 14),
        (Placeholder::Small, 8),
        (Placeholder::Hovercraft, 6),
        (Placeholder::Carrier, 4),
        (Placeholder::Vehicle, 0),
        (Placeholder::Nothing, 0),
    ] {
        assert_eq!(count(p), n, "{p:?}");
    }
    assert_eq!(
        templates.values().filter(|t| t.quickpos.is_some()).count(),
        18
    );
    assert!(
        templates
            .values()
            .filter(|t| t.quickpos.is_some())
            .all(|t| t.objects.is_empty())
    );
    assert_eq!(all.iter().filter(|o| o.start_time.is_some()).count(), 159);
    assert_eq!(all.iter().filter(|o| o.skill == Some(1)).count(), 3752);
    assert_eq!(all.iter().filter(|o| o.skill == Some(0)).count(), 3);
    assert!(all.iter().all(|o| o.speed == 0));
    assert_eq!(
        all.iter().filter(|o| o.unknown.is_empty()).count(),
        all.len()
    );
    let largest = templates.values().map(|t| t.objects.len()).max().unwrap();
    assert_eq!(largest, 127);

    // Routes: QUCOL's nine tanks, three QTCARGO objects, one QUFACT truck, and
    // one each in the two unreferenced copies.
    let routed: BTreeMap<_, _> = templates
        .values()
        .map(|t| (t.stem.as_str(), t.routed().count()))
        .filter(|(_, n)| *n > 0)
        .collect();
    assert_eq!(
        routed,
        BTreeMap::from([
            ("QFACT", 1),
            ("QTCARGO", 3),
            ("QUBUNK", 1),
            ("QUCOL", 9),
            ("QUFACT", 1)
        ])
    );
    let column = &templates["QUCOL"];
    assert!(column.routed().all(|o| {
        let r = o.route.as_ref().unwrap();
        r.waypoints.len() == 5 && r.legs().count() == 3 && r.legs().all(|w| w.speed == 50)
    }));
    let total: f64 = column
        .routed()
        .map(|o| o.route.as_ref().unwrap().length_feet())
        .sum();
    eprintln!("QUCOL routes sum to {total:.0} ft over nine tanks");
    for stem in ["QTCARGO", "QUFACT", "QFACT", "QUBUNK"] {
        for object in templates[stem].routed() {
            let route = object.route.as_ref().unwrap();
            eprintln!(
                "{stem}: {:?} alias {} {} waypoints, speeds {:?}, {:.0} ft",
                object.kind,
                route.alias,
                route.waypoints.len(),
                route.legs().map(|w| w.speed).collect::<Vec<_>>(),
                route.length_feet()
            );
        }
    }

    // Every named object exists in the archive, NTs also parse, and every unit
    // the equipment lists can draw is an NT.
    for t in templates.values() {
        for o in &t.objects {
            if let ObjectKind::Named(name) = &o.kind {
                assert!(lib.entries.contains_key(name), "{}: {name} missing", t.stem);
            }
        }
    }
    for lists in tables::LISTS {
        for name in lists.groups.iter().flat_map(|g| g.iter()) {
            assert!(units.contains_key(*name), "{:?}: {name}", lists.placeholder);
        }
    }
    // The 0x80 targets by placeholder: UCITY flags 11 <AAA> and 12 <SAM>.
    let city = &templates["QUCITY"];
    let flagged = |p| {
        city.targets()
            .filter(|o| o.placeholder() == Some(p))
            .count()
    };
    assert_eq!(
        (flagged(Placeholder::Aaa), flagged(Placeholder::Sam)),
        (11, 12)
    );
    assert_eq!(flagged(Placeholder::Tank), 16);

    // Owners: nationality3 passes through, so QIRRETR keeps friendly objects.
    let retreat = &templates["QIRRETR"];
    let blue = retreat.objects.iter().filter(|o| !o.owner.redfor()).count();
    assert_eq!(blue, 40);
    assert_eq!(
        templates["QSPFRU"]
            .objects
            .iter()
            .filter(|o| !o.owner.redfor())
            .count(),
        21
    );
    eprintln!("129 templates, {} objects", all.len());
}

#[test]
#[ignore = "needs a retail install (TORE_GAME_DIR or the gameassets link)"]
fn layouts_resolve_nationality3_owners() {
    let lib = library();
    // Survey 4.3: enemy-side / friendly-side placements per base layout.
    let want = [
        ("BAL", 56, 109),
        ("CUB", 118, 11),
        ("EGY", 81, 71),
        ("LFA", 46, 18),
        ("FRA", 113, 105),
        ("GRE", 80, 111),
        ("IRA", 93, 67),
        ("KURILE", 73, 12),
        ("TVIET", 543, 1),
        ("SPA", 40, 95),
        ("APA", 64, 43),
        ("PGU", 94, 60),
        ("NSK", 59, 65),
        ("WTA", 29, 129),
        ("UKR", 81, 176),
        ("VLA", 67, 93),
    ];
    for (code, enemy, friendly) in want {
        let name = format!("{code}.MM");
        let layout = Layout::parse(&name, &lib.read(&name).unwrap()).unwrap();
        let red = layout
            .placements
            .iter()
            .filter(|p| p.redfor() == Some(true))
            .count();
        let blue = layout
            .placements
            .iter()
            .filter(|p| p.redfor() == Some(false))
            .count();
        assert_eq!((red, blue), (enemy, friendly), "{code}");
        let uses_three = layout.placements.iter().any(|p| p.nationality3);
        let expect_three = [
            "CUB", "LFA", "GRE", "IRA", "SPA", "APA", "PGU", "NSK", "WTA",
        ];
        assert_eq!(uses_three, expect_three.contains(&code), "{code}");
        assert!(
            layout
                .placements
                .iter()
                .all(|p| !p.unknown.iter().any(|(k, _)| k == "nationality3")),
            "{code}"
        );
    }
}

fn exe() -> Option<Vec<u8>> {
    let data = std::fs::read(game_dir().join("FA.EXE")).expect("FA.EXE (set TORE_GAME_DIR)");
    if executable::identify(&data)
        .expect("a reviewed FA.EXE")
        .build
        != Build::Patch102F
    {
        eprintln!("skipped: the disc 1.0 build holds these tables at addresses not yet located");
        return None;
    }
    Some(data)
}

struct Reader<'a>(&'a Image<'a>);

impl Reader<'_> {
    fn word(&self, va: usize) -> usize {
        let b = self.0.read(va, 4, false).unwrap();
        u32::from_le_bytes(b.try_into().unwrap()) as usize
    }
    fn text(&self, va: usize) -> String {
        let mut out = String::new();
        for i in 0..64 {
            let c = self.0.read(va + i, 1, false).unwrap()[0];
            if c == 0 {
                return out;
            }
            out.push(char::from(c));
        }
        panic!("unterminated string at {va:#x}");
    }
    /// A zero-terminated pointer list and the address of the next one: lists
    /// are padded to eight bytes.
    fn list(&self, va: usize) -> (Vec<String>, usize) {
        let mut names = Vec::new();
        let mut at = va;
        loop {
            let pointer = self.word(at);
            at += 4;
            if pointer == 0 {
                return (names, at.next_multiple_of(8));
            }
            names.push(self.text(pointer).to_ascii_uppercase());
        }
    }
}

#[test]
#[ignore = "needs a retail install (TORE_GAME_DIR or the gameassets link)"]
fn retail_tables_match_the_executable() {
    let Some(data) = exe() else { return };
    let image = Image::parse(&data).unwrap();
    let exe = Reader(&image);

    // Unit lists: five lists per block, stored as groups 4, 0, 1, 2, 3.
    for (lists, start) in tables::LISTS.iter().zip(addresses::BLOCKS) {
        let mut at = start;
        for group in [4, 0, 1, 2, 3] {
            let (names, next) = exe.list(at);
            assert_eq!(
                names, lists.groups[group],
                "{:?} group {group}",
                lists.placeholder
            );
            at = next;
        }
        // The blocks run back to back, except that the SAM percentages sit
        // between the SAM and AAA blocks and the night list follows the AAA
        // block.
        let following = if start == addresses::BLOCKS[9] {
            addresses::SAM_PERCENT
        } else {
            addresses::BLOCKS
                .iter()
                .copied()
                .find(|b| *b > start)
                .unwrap_or(addresses::NIGHT_AAA_LIST)
        };
        assert_eq!(at, following, "{:?} block length", lists.placeholder);
    }
    let (night, after) = exe.list(addresses::NIGHT_AAA_LIST);
    assert_eq!(night, tables::NIGHT_AAA);
    assert_eq!(after, addresses::AAA_PERCENT);

    // Defense percentages.
    for at in [addresses::SAM_PERCENT, addresses::AAA_PERCENT] {
        let words: Vec<_> = (0..4).map(|i| exe.word(at + 4 * i) as u32).collect();
        assert_eq!(words, tables::DEFENSE_PERCENT);
    }
    assert_eq!(addresses::SAM_PERCENT + 16, addresses::BLOCKS[10]);

    // Placeholder spellings.
    for (placeholder, at) in addresses::PLACEHOLDER_TOKENS {
        assert_eq!(exe.text(at), format!("<{}>", placeholder.name()));
    }
    for stealth in tables::NIGHT_STEALTH_AIRCRAFT {
        let found = data.windows(stealth.len() + 1).any(|w| {
            w[..stealth.len()].eq_ignore_ascii_case(stealth.as_bytes()) && w[stealth.len()] == 0
        });
        assert!(found, "{stealth} not in the executable");
    }

    // Nationality to group, as 60 little-endian words.
    let bytes = image
        .read(addresses::NATIONALITY_GROUP, 120, false)
        .unwrap();
    let words: Vec<u8> = bytes
        .chunks_exact(2)
        .map(|w| u16::from_le_bytes([w[0], w[1]]) as u8)
        .collect();
    assert_eq!(words, tables::NATIONALITY_GROUP);

    // Template names: the flat pointer table is the theaters in
    // POINTER_TABLE_ORDER, each in menu order.
    let flat: Vec<String> = (addresses::TEMPLATE_POINTERS..addresses::TEMPLATE_POINTERS_END)
        .step_by(4)
        .map(|at| exe.word(at))
        .filter(|p| *p != 0)
        .map(|p| exe.text(p).to_ascii_uppercase())
        .collect();
    let want: Vec<String> = tables::POINTER_TABLE_ORDER
        .iter()
        .flat_map(|t| tables::TEMPLATES[*t].iter())
        .map(|stem| format!("~{stem}.M"))
        .collect();
    assert_eq!(flat, want);
    // And the creator's own target lists have as many entries per theater.
    let creator = Options::parse(&data).unwrap();
    for (theater, list) in tables::TEMPLATES.iter().enumerate() {
        assert_eq!(
            creator.targets[theater].len(),
            list.len(),
            "{}",
            tables::THEATERS[theater]
        );
    }
    eprintln!("tables match FA.EXE 1.02F");
}

#[test]
#[ignore = "needs a retail install (TORE_GAME_DIR or the gameassets link)"]
fn weapons_of_armed_units_exist_as_records() {
    // Every hardpoint store is an archive record; sensors and weapons differ.
    let lib = library();
    for unit in units(&lib).values() {
        for mount in &unit.mounts {
            let store = mount.store.as_deref().unwrap();
            assert!(
                lib.entries.contains_key(store),
                "{}: {store}",
                unit.resource
            );
            assert_eq!(
                mount.kind == MountKind::Sensor,
                store.ends_with(".SEE"),
                "{}",
                unit.resource
            );
        }
    }
}
