//! Pins the AAA tuning table (plan section 4.3, tests 1, 2, 3, 5 and 6).
use super::*;
use crate::combat::gunsight::tests::weapon;

/// Retail fire zones of the surface guns: (record, maximum range, ceiling), in
/// feet, from the shipped JT records (survey 3.2); the ignored real-data test
/// checks them against the LIB.
const ZONES: [(&str, i32, i32); 17] = [
    ("ZSU23.JT", 7_500, 7_500),
    ("2S6.JT", 15_000, 9_000),
    ("PHALANX.JT", 20_000, 15_000),
    ("AAA30.JT", 15_000, 7_500),
    ("AAA30BAD.JT", 10_000, 7_500),
    ("ZSU57.JT", 12_000, 12_000),
    ("M1939.JT", 10_000, 6_000),
    ("A_M1939.JT", 10_000, 6_000),
    ("KS12.JT", 40_000, 15_000),
    ("KS19.JT", 50_000, 25_000),
    ("M1.JT", 5_000, 3_000),
    ("T72.JT", 5_000, 3_000),
    ("BMP2.JT", 3_000, 3_000),
    ("BTR80.JT", 3_000, 3_000),
    ("M113.JT", 3_000, 3_000),
    ("M2.JT", 3_000, 3_000),
    ("SMLARMS.JT", 4_000, 2_000),
];

/// The retail record of a row, as a synthetic weapon built from the table's
/// own retail columns (retail bytes are never committed).
fn retail_weapon(row: &GunTuning) -> Weapon {
    let (_, range, ceiling) = *ZONES
        .iter()
        .find(|(record, ..)| *record == row.record)
        .expect("a fire zone for every record");
    let r = row.retail;
    let mut w = weapon();
    w.source = row.record.into();
    w.flags = 0x1_40c0;
    w.movement.minimum_speed = r.final_speed;
    w.movement.corner_speed = r.initial_speed;
    w.movement.maximum_speed = r.initial_speed;
    w.movement.acceleration = 0;
    w.movement.deceleration = 0;
    w.movement.initial_speed = r.initial_speed;
    w.movement.final_speed = r.final_speed;
    w.movement.launch_retard = 100;
    w.movement.remove_t = r.remove_t;
    w.burst.actual_rounds_per_game = r.rounds_per_game;
    w.burst.game_rounds_in_burst = r.rounds_in_burst;
    w.burst.game_rounds_in_carpet_burst = r.rounds_in_burst;
    w.burst.game_burst_t = r.burst_t;
    w.burst.reload_t = r.reload_t;
    w.burst.startup_shots = r.startup_shots;
    w.seeker.zones[1].maximum_range = range;
    w.seeker.zones[1].maximum_altitude = ceiling;
    w.damage.by_class = [20, 2, 6, 4, 20];
    w
}

fn applied(row: &GunTuning) -> Weapon {
    let mut w = retail_weapon(row);
    let unit = row.unit.unwrap_or(row.units[0]);
    assert_eq!(apply(unit, &mut w), Some(row));
    w
}

/// The documented values, one line per row: rate, burst, pause (quarter
/// seconds), opening, magazine, reload, muzzle. A change to the table has to
/// change this pin and the generated spec table together.
type Pin = (
    &'static str,
    Option<&'static str>,
    u32,
    u16,
    u16,
    u8,
    u32,
    u32,
    i16,
);
const PINNED: [Pin; 18] = [
    ("ZSU23.JT", None, 3_400, 99, 4, 0, 2_000, 120, 3_180),
    ("2S6.JT", None, 5_000, 105, 4, 0, 1_904, 120, 3_150),
    (
        "PHALANX.JT",
        Some("M163"),
        3_000,
        50,
        4,
        0,
        1_100,
        120,
        3_380,
    ),
    ("ZSU57.JT", None, 240, 5, 12, 0, 300, 120, 3_280),
    ("PHALANX.JT", None, 4_500, 150, 4, 0, 1_550, 120, 3_600),
    ("AAA30.JT", None, 4_000, 150, 4, 0, 2_000, 120, 2_950),
    ("AAA30BAD.JT", None, 2_000, 42, 4, 0, 1_000, 120, 3_440),
    ("M1939.JT", None, 160, 6, 12, 0, 200, 60, 2_890),
    ("A_M1939.JT", None, 160, 6, 12, 0, 200, 60, 2_890),
    ("KS12.JT", None, 5, 1, 50, 8, 60, 60, 2_620),
    ("KS19.JT", None, 5, 1, 50, 8, 60, 60, 2_950),
    ("M1.JT", None, 6, 1, 39, 0, 34, 60, 5_866),
    ("T72.JT", None, 8, 1, 29, 0, 22, 60, 5_866),
    ("BMP2.JT", None, 300, 20, 12, 0, 500, 60, 3_150),
    ("BTR80.JT", None, 600, 10, 12, 0, 500, 60, 3_280),
    ("M113.JT", None, 500, 21, 12, 0, 2_000, 60, 2_910),
    ("M2.JT", None, 200, 20, 12, 0, 300, 60, 3_600),
    ("SMLARMS.JT", None, 600, 5, 4, 0, 1_000, 60, 3_000),
];

#[test]
fn surface_guns_table_is_pinned() {
    assert_eq!(TABLE.len(), PINNED.len());
    for (row, pin) in TABLE.iter().zip(PINNED) {
        let name = format!("{} {:?}", row.record, row.unit);
        assert_eq!((row.record, row.unit), (pin.0, pin.1), "{name}");
        assert_eq!(
            (
                row.rounds_per_minute,
                row.burst,
                row.pause_quarters,
                row.opening_shots,
                row.magazine,
                row.magazine_reload_s,
                row.muzzle_fps
            ),
            (pin.2, pin.3, pin.4, pin.5, pin.6, pin.7, pin.8),
            "{name}"
        );
        // The record fields the table writes give the table's cadence: within
        // one percent of the documented rate.
        let w = applied(row);
        let rate = record_rounds_per_minute(&w);
        // A single-round row states its sustained rate, rounded: the flak's
        // 4.7 a minute is listed as 5.
        let wanted = if row.burst <= 1 {
            row.sustained_rpm()
        } else {
            f64::from(row.rounds_per_minute)
        };
        assert!(
            (rate - wanted).abs() / wanted <= 0.01,
            "{name}: record cadence {rate:.1} rpm against {wanted}"
        );
        let burst = &w.burst;
        assert_eq!(
            u16::from(burst.game_rounds_in_burst) * u16::from(burst.actual_rounds_per_game),
            row.burst,
            "{name}"
        );
        assert_eq!(burst.actual_rounds_per_game, row.per_game, "{name}");
        assert_eq!(row.burst % u16::from(row.per_game), 0, "{name}");
        assert_eq!(u16::from(burst.reload_t), row.pause_quarters, "{name}");
        assert_eq!(burst.startup_shots, row.opening_shots, "{name}");
        assert_eq!(w.movement.initial_speed, row.muzzle_fps, "{name}");
        // A launch is not clamped back to a retail speed limit.
        assert_eq!(
            super::launch_speed(&w.movement, 0).unwrap(),
            i32::from(row.muzzle_fps),
            "{name}"
        );
        // John's reload times, 2026-10-10: a minute for towed guns and small
        // vehicles, two for self-propelled AA guns and ship guns.
        assert_eq!(
            row.magazine_reload_s,
            row.mount.magazine_reload_s(),
            "{name}"
        );
        assert!(matches!(row.magazine_reload_s, 60 | 120), "{name}");
        // Two spare magazines on land, unlimited on ships.
        assert_eq!(
            row.mount.reserve_magazines(),
            (row.mount != Mount::Ship).then_some(2),
            "{name}"
        );
        // Flak, 57 mm, 37 mm and tank guns keep the retail damage per round.
        if matches!(
            row.record,
            "KS12.JT" | "KS19.JT" | "ZSU57.JT" | "M1939.JT" | "A_M1939.JT" | "M1.JT" | "T72.JT"
        ) {
            assert_eq!(row.per_game, 1, "{name}");
        }
        assert!(row.magazine > 0 && row.burst > 0, "{name}");
        assert!(row.burst <= u16::from(row.per_game) * 255, "{name}");
        assert!(row.sustained_rpm() > 0., "{name}");
        // Damage per second of sustained fire matches retail: within 13
        // percent where the rate was scaled, never above retail by more than
        // 10 percent, and the retail damage per round where the split is 1.
        let ratio = row.damage_rate_ratio();
        assert!(ratio <= 1.10, "{name}: {ratio:.3}");
        if row.per_game > 1 {
            assert!(ratio >= 0.87, "{name}: {ratio:.3}");
        }
        // Retail damage per game round is untouched; the shot code splits it.
        // (Heavy flak's splash reach is shorter: FLAK_SPLASH_RADIUS_FT.)
        let mut retail_damage = retail_weapon(row).damage;
        if retail_damage.collateral_radius > 0 {
            retail_damage.collateral_radius = FLAK_SPLASH_RADIUS_FT;
        }
        assert_eq!(w.damage, retail_damage, "{name}");
    }
    // Flak shells carry no tracer, and fire an opening barrage of eight.
    // Their rate of fire is a third of the earlier fitted 14 a minute
    // (John, 2026-10-13), so the pause is his.
    for record in ["KS12.JT", "KS19.JT"] {
        let row = tuning("", record).unwrap();
        assert!(!row.tracer);
        assert_eq!(row.opening_shots, 8);
        assert_eq!(row.burst, 1);
        assert_eq!(row.origins().pause, Origin::Opinionated);
        let third = 14.1 / 3.;
        assert!(
            (row.sustained_rpm() - third).abs() < 0.2,
            "{record}: {:.2} a minute",
            row.sustained_rpm()
        );
    }
    assert!(tuning("", "ZSU23.JT").unwrap().tracer);
    // Small arms carry tracers like the other light guns (John, 2026-10-12);
    // only flak and the tank guns go without.
    let rifle = tuning("TROOPS", "SMLARMS.JT").unwrap();
    assert!(rifle.tracer);
    for row in TABLE {
        assert_eq!(
            row.tracer,
            !matches!(row.record, "KS12.JT" | "KS19.JT" | "M1.JT" | "T72.JT"),
            "{}",
            row.record
        );
    }
    // Only the squad's rifles are small arms.
    for row in TABLE {
        assert_eq!(
            is_small_arms(row.record),
            row.record == "SMLARMS.JT",
            "{}",
            row.record
        );
    }
    assert!(is_small_arms("smlarms.jt") && !is_small_arms("M2.JT"));
    // The origin labels follow the numbers: retail where the row repeats the
    // retail record, and John's reload times.
    for row in TABLE {
        let origins = row.origins();
        assert_eq!(origins.reload, Origin::Opinionated);
        assert_eq!(origins.magazine, Origin::Fitted);
        assert_eq!(origins.rate, Origin::Fitted);
        assert_eq!(
            origins.pause == Origin::Retail,
            row.pause_quarters == u16::from(row.retail.reload_t)
        );
        assert_eq!(
            origins.opening == Origin::Retail,
            row.opening_shots == row.retail.startup_shots
        );
        assert_eq!(
            origins.damage == Origin::Retail,
            row.per_game == 1,
            "{}",
            row.record
        );
    }
}

#[test]
fn every_row_names_its_retail_record_and_the_rows_are_distinct() {
    for (i, a) in TABLE.iter().enumerate() {
        assert!(a.record.ends_with(".JT"));
        for b in &TABLE[..i] {
            assert!(
                (a.record, a.unit) != (b.record, b.unit),
                "{} twice",
                a.record
            );
        }
        assert!(!a.units.is_empty());
        // The retail pause is a quarter-second count that fits the record's byte.
        assert!(a.pause_quarters <= 255);
    }
    // Bursts are quarter seconds on the record: never zero.
    assert!(TABLE.iter().all(|row| row.burst_quarters() >= 1));
}

#[test]
fn apply_leaves_other_records_alone() {
    // An aircraft gun, an AC-130 gun, a missile and an unknown surface record.
    for source in [
        "M61.JT",
        "GSH301.JT",
        "C_25.JT",
        "SA6.JT",
        "AIM9X.JT",
        "NOPE.JT",
    ] {
        let mut w = weapon();
        w.source = source.into();
        let before = w.clone();
        assert_eq!(apply("ZSU23", &mut w), None, "{source}");
        assert_eq!(w, before, "{source}");
        assert_eq!(tuning("ZSU23", source), None);
        assert!(!is_surface_gun(source));
    }
    // The retail fields the table does not set survive on a record it does.
    for row in TABLE {
        let before = retail_weapon(row);
        let after = applied(row);
        let mut restored = after.clone();
        restored.burst = before.burst;
        restored.movement = before.movement;
        if row.zones.is_some() {
            // The row's own replacement zones are the one other change.
            restored.seeker = before.seeker;
        }
        assert_eq!(restored, before, "{}", row.record);
        assert_eq!(
            after.burst.projectiles_in_pod,
            before.burst.projectiles_in_pod
        );
        assert_eq!(
            after.burst.random_fire_percent,
            before.burst.random_fire_percent
        );
        assert_eq!(after.movement.remove_t, before.movement.remove_t);
        assert_eq!(
            after.burst.game_rounds_in_carpet_burst,
            before.burst.game_rounds_in_carpet_burst
        );
        // Applying twice is the same as applying once.
        let mut twice = after.clone();
        apply(row.unit.unwrap_or(row.units[0]), &mut twice);
        assert_eq!(twice, after, "{}", row.record);
    }
    // A unit with no row of its own gets the record's row, and the M163 its own.
    assert_eq!(tuning("NIMZ", "PHALANX.JT").unwrap().mount, Mount::Ship);
    assert_eq!(tuning("M163", "PHALANX.JT").unwrap().mount, Mount::Spaag);
    assert_eq!(tuning("m163.nt", "phalanx.jt").unwrap().unit, Some("M163"));
    assert_eq!(tuning("", "PHALANX.JT").unwrap().unit, None);
}

/// The armed surface NTs and the gun record each fires (survey 2.3 to 2.5).
const ARMED: [(&str, &str); 38] = [
    ("ZSU23", "ZSU23.JT"),
    ("2S6", "2S6.JT"),
    ("NIMZ", "PHALANX.JT"),
    ("KITT", "PHALANX.JT"),
    ("CLEM", "PHALANX.JT"),
    ("WASP", "PHALANX.JT"),
    ("IOWA", "PHALANX.JT"),
    ("TICON", "PHALANX.JT"),
    ("M163", "PHALANX.JT"),
    ("KIROV", "AAA30.JT"),
    ("SOVR", "AAA30.JT"),
    ("KIEV", "AAA30.JT"),
    ("SARAN", "AAA30.JT"),
    ("BUTLER", "AAA30.JT"),
    ("TYPE69", "AAA30BAD.JT"),
    ("KNOX", "AAA30BAD.JT"),
    ("JIANC", "AAA30BAD.JT"),
    ("JIANE", "AAA30BAD.JT"),
    ("KRIVAK", "AAA30BAD.JT"),
    ("CYCL", "AAA30BAD.JT"),
    ("PMORN", "AAA30BAD.JT"),
    ("ZSU57", "ZSU57.JT"),
    ("ZIF31", "ZSU57.JT"),
    ("M1939", "M1939.JT"),
    ("A_M1939", "A_M1939.JT"),
    ("KS12", "KS12.JT"),
    ("KS19", "KS19.JT"),
    ("M1", "M1.JT"),
    ("T72", "T72.JT"),
    ("T80", "T72.JT"),
    ("T90", "T72.JT"),
    ("BMP2", "BMP2.JT"),
    ("BTR80", "BTR80.JT"),
    ("M113", "M113.JT"),
    ("M2", "M2.JT"),
    ("TROOPS", "SMLARMS.JT"),
    // The retail records shared with the lines above, with their NT names
    // as the survey spells them.
    ("ZIF31.NT", "ZSU57.JT"),
    ("BUTLER.NT", "AAA30.JT"),
];

#[test]
fn every_armed_surface_gun_has_a_row() {
    for (unit, record) in ARMED {
        let row = tuning(unit, record).unwrap_or_else(|| panic!("{unit} {record} has no row"));
        assert_eq!(row.record, record, "{unit}");
        let short = unit.trim_end_matches(".NT");
        assert!(
            row.units.contains(&short),
            "{unit} is not listed under {record}"
        );
    }
    // Every unit of every row is one of the armed NTs, and each is in one row.
    for row in TABLE {
        for unit in row.units {
            assert!(
                ARMED.iter().any(|(u, r)| u == unit && *r == row.record),
                "{unit} {}",
                row.record
            );
            let rows = TABLE.iter().filter(|r| r.units.contains(unit)).count();
            assert_eq!(rows, 1, "{unit}");
        }
    }
    // The 17 retail records the survey names.
    let mut records: Vec<_> = TABLE.iter().map(|row| row.record).collect();
    records.sort_unstable();
    records.dedup();
    assert_eq!(records.len(), 17);
    // John (2026-10-10): the M163 has a Vulcan row, and the Butler stays on
    // the retail AAA30 record with no Bofors row.
    let vulcan = tuning("M163", "PHALANX.JT").unwrap();
    assert!(vulcan.gun.contains("Vulcan"));
    assert_ne!(
        vulcan,
        tuning("NIMZ", "PHALANX.JT").unwrap(),
        "Phalanx and Vulcan differ"
    );
    assert_eq!(tuning("BUTLER", "AAA30.JT").unwrap().record, "AAA30.JT");
    assert!(TABLE.iter().all(|row| !row.gun.contains("Bofors")));
    assert!(
        TABLE
            .iter()
            .all(|row| row.unit.is_none() || row.units == [row.unit.unwrap()])
    );
}

#[test]
fn the_tank_guns_are_the_main_battle_tanks_rows_only() {
    let tanks: Vec<&str> = TABLE
        .iter()
        .filter(|row| is_tank_gun(row.record))
        .flat_map(|row| row.units.iter().copied())
        .collect();
    assert_eq!(tanks, ["M1", "T72", "T80", "T90"]);
    assert!(is_tank_gun("t72.jt"));
    // The IFVs, APCs and everything else keep their rules.
    for record in [
        "BMP2.JT",
        "BTR80.JT",
        "M113.JT",
        "M2.JT",
        "SMLARMS.JT",
        "ZSU57.JT",
    ] {
        assert!(!is_tank_gun(record), "{record}");
    }
    assert!(
        TABLE
            .iter()
            .filter(|row| is_tank_gun(row.record))
            .all(|row| row.mount == Mount::Vehicle)
    );
}

#[test]
fn flak_and_every_other_shell_reaches_its_fire_zone_before_it_expires() {
    for row in TABLE {
        let w = applied(row);
        let (_, range, ceiling) = *ZONES.iter().find(|(r, ..)| *r == row.record).unwrap();
        let reach = reach_ft(&w);
        // The round's life is `removeT` quarter seconds at the muzzle velocity
        // (the retail records neither speed up nor slow down in flight).
        let expected = f64::from(row.muzzle_fps) * f64::from(w.movement.remove_t) / 4.;
        assert!(
            (reach - expected).abs() / expected < 0.01,
            "{}: {reach:.0} against {expected:.0}",
            row.record
        );
        assert!(
            reach >= f64::from(ceiling),
            "{} reaches {reach:.0} ft, short of its {ceiling} ft ceiling",
            row.record
        );
        // Flak shells burst on a time fuze set to the lead point, so their
        // 4,000 ft floor and ceiling are what must be reachable.
        if !matches!(row.record, "KS12.JT" | "KS19.JT") {
            assert!(
                reach >= 0.95 * f64::from(range),
                "{} reaches {reach:.0} ft of its {range} ft range",
                row.record
            );
        }
    }
    // The two flak records, with the figures the plan names.
    let ks19 = applied(tuning("", "KS19.JT").unwrap());
    assert!(reach_ft(&ks19) >= 25_000.);
    assert_eq!(ks19.movement.remove_t, 60);
    let ks12 = applied(tuning("", "KS12.JT").unwrap());
    assert!(reach_ft(&ks12) >= 15_000.);
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn seconds(quarters: u64) -> String {
    format!("{}.{:02}", quarters / 4, (quarters % 4) * 25)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn seconds_with_point(quarters: u64) -> String {
    let text = seconds(quarters);
    if text.contains('.') {
        text
    } else {
        format!("{text}.0")
    }
}

/// The generated AAA table, as Markdown.
fn aaa_markdown() -> String {
    let mut out = String::new();
    out.push_str(
        "| Gun | Record (units) | Retail burst / pause s / opening / muzzle ft/s | \
Rate rpm | Burst rounds (s) | Pause s | Opening | Magazine | Magazine reload s | \
Reload class | Reserve magazines | Muzzle ft/s | Tracer | Damage per round | Damage per second vs retail |\n",
    );
    out.push_str(
        "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- | --- | ---: |\n",
    );
    for row in TABLE {
        let o = row.origins();
        let r = row.retail;
        let record = row.record.trim_end_matches(".JT");
        let record = if row.units == [record] {
            record.to_string()
        } else {
            format!("{record} ({})", row.units.join(", "))
        };
        let tag = |value: String, origin: Origin| format!("{value} {}", origin.tag());
        let burst_t = u64::from(row.burst_quarters());
        let cells = [
            row.gun.to_string(),
            record,
            format!(
                "{} in {} s / {} s / {} / {}",
                r.rounds_in_burst,
                seconds_with_point(u64::from(r.burst_t.max(1))),
                seconds_with_point(u64::from(r.reload_t)),
                r.startup_shots,
                thousands(r.initial_speed as u64)
            ),
            tag(thousands(u64::from(row.rounds_per_minute)), o.rate),
            if row.burst > 1 {
                format!(
                    "{} ({} s)",
                    tag(row.burst.to_string(), o.burst),
                    seconds_with_point(burst_t)
                )
            } else {
                format!("{} (single shot)", tag(row.burst.to_string(), o.burst))
            },
            tag(seconds_with_point(u64::from(row.pause_quarters)), o.pause),
            match row.opening_gap_quarters() {
                0 => tag(row.opening_shots.to_string(), o.opening),
                gap => format!(
                    "{}, {} s apart O",
                    tag(row.opening_shots.to_string(), o.opening),
                    seconds_with_point(u64::from(gap))
                ),
            },
            tag(thousands(u64::from(row.magazine)), o.magazine),
            tag(row.magazine_reload_s.to_string(), o.reload),
            row.mount.label().to_string(),
            row.mount
                .reserve_magazines()
                .map_or("unlimited".to_string(), |n| n.to_string())
                + " F",
            tag(thousands(row.muzzle_fps as u64), o.muzzle),
            if row.tracer { "every 3rd" } else { "none" }.to_string(),
            tag(
                if row.per_game == 1 {
                    "retail".into()
                } else {
                    format!("1/{} of retail", row.per_game)
                },
                o.damage,
            ),
            format!("{:.2}", row.damage_rate_ratio()),
        ];
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
    out
}

/// `docs/spec/surface-defenses.md#aaa-tuning` is generated from [`TABLE`].
/// Regenerate with
/// `TORE_UPDATE_SURFACE_GUNS_DOC=1 cargo test -p tore-sim surface_guns_doc`.
#[test]
fn surface_guns_doc_matches_the_table() {
    const START: &str = "<!-- surface-guns-table:start -->\n";
    const END: &str = "<!-- surface-guns-table:end -->";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/spec/surface-defenses.md");
    // Windows checkouts may convert the doc to CRLF line endings.
    let text = std::fs::read_to_string(&path)
        .expect("docs/spec/surface-defenses.md")
        .replace("\r\n", "\n");
    let (head, rest) = text.split_once(START).expect("start marker");
    let (current, tail) = rest.split_once(END).expect("end marker");
    let generated = aaa_markdown();
    if std::env::var_os("TORE_UPDATE_SURFACE_GUNS_DOC").is_some() {
        std::fs::write(&path, format!("{head}{START}{generated}{END}{tail}")).unwrap();
        return;
    }
    assert!(
        current == generated,
        "docs/spec/surface-defenses.md is out of date; run TORE_UPDATE_SURFACE_GUNS_DOC=1 cargo test -p tore-sim surface_guns_doc"
    );
}

/// Run with `TORE_DATA_DIR` naming a data folder with an import: the retail
/// columns, fire zones and burst fields of every row match the shipped JT
/// records, and the table applies to each.
#[test]
#[ignore = "needs an imported data profile (TORE_DATA_DIR)"]
fn real_data_every_row_matches_the_shipped_record() {
    use std::io::Read;
    let directory = std::env::var_os("TORE_DATA_DIR").expect("TORE_DATA_DIR names an import");
    let mut packs: Vec<_> = std::fs::read_dir(&directory)
        .expect("the data folder")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "pack")
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("menu-"))
        })
        .collect();
    packs.sort();
    let mut bytes = Vec::new();
    std::fs::File::open(packs.pop().expect("an import pack"))
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .expect("the pack reads");
    assert_eq!(&bytes[..12], b"TOREMENU\x01\0\0\0");
    let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    let mut resources = std::collections::BTreeMap::new();
    let (count, mut at) = (word(12), 16);
    for _ in 0..count {
        let len = u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
        let name = String::from_utf8(bytes[at + 2..at + 2 + len].to_vec()).unwrap();
        let size = word(at + 2 + len);
        let start = at + 6 + len;
        resources.insert(name, bytes[start..start + size].to_vec());
        at = start + size;
    }
    for row in TABLE {
        let data = resources
            .get(row.record)
            .unwrap_or_else(|| panic!("{} is not in the import", row.record));
        let mut real = Weapon::parse(row.record, data).expect("the record parses");
        let (_, range, ceiling) = *ZONES.iter().find(|(r, ..)| *r == row.record).unwrap();
        let r = row.retail;
        assert_eq!(
            (
                real.burst.game_rounds_in_burst,
                real.burst.actual_rounds_per_game,
                real.burst.game_burst_t,
                real.burst.reload_t,
                real.burst.startup_shots,
                real.movement.initial_speed,
                real.movement.final_speed,
                real.movement.remove_t,
                real.movement.acceleration,
                real.movement.deceleration,
            ),
            (
                r.rounds_in_burst,
                r.rounds_per_game,
                r.burst_t,
                r.reload_t,
                r.startup_shots,
                r.initial_speed,
                r.final_speed,
                r.remove_t,
                0,
                0
            ),
            "{}",
            row.record
        );
        assert_eq!(real.seeker.zones[1].maximum_range, range, "{}", row.record);
        assert_eq!(
            real.seeker.zones[1].maximum_altitude, ceiling,
            "{}",
            row.record
        );
        assert_ne!(real.flags & 0x40, 0, "{}", row.record);
        assert_eq!(real.flags & 4, 0, "{} falls", row.record);
        let before = real.damage;
        let unit = row.unit.unwrap_or(row.units[0]);
        assert_eq!(apply(unit, &mut real), Some(row));
        assert_eq!(real.damage, before);
        let rate = record_rounds_per_minute(&real);
        let wanted = f64::from(row.rounds_per_minute);
        assert!((rate - wanted).abs() / wanted <= 0.01, "{}", row.record);
        assert!(reach_ft(&real) >= f64::from(ceiling), "{}", row.record);
    }
    // No other gun-round record the import holds is one a surface NT fires
    // without a row: every record the table does not name is an aircraft gun,
    // an AC-130 gun, a pod or an unreferenced test record.
    let known = [
        "20MM_4.JT",
        "AAA20.JT",
        "ADEN.JT",
        "ADENIN.JT",
        "BK27.JT",
        "C_105.JT",
        "C_25.JT",
        "C_40.JT",
        "DEFA.JT",
        "GAU12.JT",
        "GAU13.JT",
        "GAU8.JT",
        "GSH23.JT",
        "GSH30.JT",
        "GSH301.JT",
        "GSH6_23.JT",
        "GSH6_30.JT",
        "M163.JT",
        "M61.JT",
        "MK12.JT",
        "SUU16.JT",
        "T12_4.JT",
        "T20_1.JT",
        "T23_1.JT",
        "T30_1.JT",
        "~VOMIT.JT",
    ];
    for name in known {
        assert!(!is_surface_gun(name), "{name}");
    }
}

/// John (2026-10-13): the M163 sees to about 3 nm and fires to 7,500 ft, the
/// real Vulcan's reach; the ships' Phalanx keeps the retail record's zones.
#[test]
fn the_m163_has_its_own_zones_and_the_ships_phalanx_keeps_the_records() {
    let m163 = tuning("M163", "PHALANX.JT").unwrap();
    let zones = m163.zones.expect("the M163 row replaces the zones");
    assert_eq!(zones.detection_ft, 18_000);
    assert_eq!(zones.fire_ft, 7_500);
    let mut w = retail_weapon(m163);
    w.seeker.zones[0].maximum_range = 50_000;
    w.seeker.zones[1].maximum_range = 20_000;
    w.seeker.zones[1].maximum_altitude = 15_000;
    apply("M163", &mut w).unwrap();
    assert_eq!(w.seeker.zones[0].maximum_range, 18_000);
    assert_eq!(w.seeker.zones[1].maximum_range, 7_500);
    assert_eq!(w.seeker.zones[1].maximum_altitude, 7_500);
    // Idempotent, and a ship's Phalanx is untouched.
    apply("M163", &mut w).unwrap();
    assert_eq!(w.seeker.zones[1].maximum_range, 7_500);
    let ship = tuning("NIMZ", "PHALANX.JT").unwrap();
    assert!(ship.zones.is_none());
    let mut w = retail_weapon(ship);
    w.seeker.zones[0].maximum_range = 50_000;
    apply("NIMZ", &mut w).unwrap();
    assert_eq!(w.seeker.zones[0].maximum_range, 50_000);
}

#[test]
fn flak_bursts_inside_an_egg_300_m_wide_and_100_m_tall() {
    // Corners and the middle of the three draws: every point is inside the
    // ellipsoid, none outside, and the extremes reach its half axes.
    let (w, h) = (FLAK_EGG_HALF_WIDTH_FT, FLAK_EGG_HALF_HEIGHT_FT);
    assert!((w * 0.3048 - 150.).abs() < 1e-9 && (h * 0.3048 - 50.).abs() < 1e-9);
    let mut state = 7u64;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let (mut widest, mut tallest, mut mean_scaled) = (0f64, 0f64, 0.);
    let n = 20_000;
    for _ in 0..n {
        let p = flak_egg_offset([next(), next(), next()], 1.);
        let scaled = (p[0] / w).powi(2) + (p[1] / h).powi(2) + (p[2] / w).powi(2);
        assert!(scaled <= 1. + 1e-9, "{p:?} is outside the egg");
        widest = widest.max(p[0].hypot(p[2]));
        tallest = tallest.max(p[1].abs());
        mean_scaled += scaled.sqrt();
    }
    // The volume is filled evenly: the mean scaled radius of a uniform ball is 3/4.
    assert!((mean_scaled / f64::from(n) - 0.75).abs() < 0.01);
    assert!(widest > 0.98 * w && tallest > 0.98 * h);
    // A jammer widens it twice over.
    let p = flak_egg_offset([1., 0.5, 0.], 2.);
    assert!((p[0] - 2. * w).abs() < 1e-6);
    // The same draws give the same point.
    assert_eq!(
        flak_egg_offset([0.3, 0.6, 0.9], 1.),
        flak_egg_offset([0.3, 0.6, 0.9], 1.)
    );
}

#[test]
fn heavy_flak_splash_reaches_400_ft_not_the_records_750_ft() {
    for record in ["KS12.JT", "KS19.JT"] {
        let mut w = applied_with_splash(tuning("", record).unwrap(), 750);
        assert_eq!(w.damage.collateral_radius, FLAK_SPLASH_RADIUS_FT);
        assert_eq!(w.damage.collateral_percent, 35, "the percent stays");
        // Idempotent, and a record with a shorter reach keeps it.
        apply("", &mut w);
        assert_eq!(w.damage.collateral_radius, FLAK_SPLASH_RADIUS_FT);
        assert_eq!(
            applied_with_splash(tuning("", record).unwrap(), 200)
                .damage
                .collateral_radius,
            200
        );
    }
    // A gun with no splash gets none.
    assert_eq!(
        applied(tuning("", "ZSU23.JT").unwrap())
            .damage
            .collateral_radius,
        0
    );
}

fn applied_with_splash(row: &GunTuning, radius: i16) -> Weapon {
    let mut w = retail_weapon(row);
    w.damage.collateral_radius = radius;
    w.damage.collateral_percent = 35;
    apply("", &mut w).expect("a row");
    w
}
