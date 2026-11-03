//! A synthetic surface import that tests in this crate and in the network
//! crates share (protocol 22): every surface type the equipment lists name,
//! a base layout with surface units on both sides, a defended template and
//! a fleet. No retail data. Moved from `surface/tests.rs` so the session's
//! tests can fly a ground target too.
use super::resources::{AIRPORT_AT, resources, schema};
use crate::mission::{Defense, MissionSpec};
use crate::surface::{layout, resolve::GroundTarget};
use std::collections::BTreeMap;
use tore_formats::{
    aircraft::AircraftId,
    quick_template::{Placeholder, tables},
    surface_unit::class,
};

pub const HEADER: &str = "[brent's_relocatable_format]\n";

/// One line per field of `layout`; `value` overrides a field by name.
pub fn fields(layout: &[(&str, &str)], value: &dyn Fn(&str) -> Option<String>) -> String {
    let mut text = String::new();
    for &(kind, name) in layout {
        let given = value(name);
        match (kind, given) {
            ("ptr", Some(block)) => text += &format!("ptr {block}\n"),
            ("ptr", None) => text += "dword 0\n",
            ("symbol", given) => text += &format!("symbol {}\n", given.unwrap_or_default()),
            (kind, given) => text += &format!("{kind} {}\n", given.unwrap_or_else(|| "0".into())),
        }
    }
    text
}

/// A synthetic NT: `class`, hit points, its proc and weapon mounts
/// (`store`, `maxItems`).
pub fn nt(stem: &str, class: u16, hp: i32, util: &str, mounts: &[(&str, i32)]) -> Vec<u8> {
    let size = 186 + 24 * mounts.len();
    let object = |name: &str| -> Option<String> {
        Some(match name {
            "structType" => "3".into(),
            "typeSize" => size.to_string(),
            "ot_names" => "ot_names".into(),
            "shape" => "shape".into(),
            "obj_class" => class.to_string(),
            "hitPoints" => hp.to_string(),
            "expType" => "21".into(),
            "craterSize" => "6".into(),
            "utilProc" => util.into(),
            _ => return None,
        })
    };
    let npc = |name: &str| -> Option<String> {
        Some(match name {
            "numHards" => mounts.len().to_string(),
            "hards" => "hards".into(),
            _ => return None,
        })
    };
    let mut text = String::from(HEADER);
    text += &fields(schema::OBJECT, &object);
    text += &fields(schema::NPC, &npc);
    text += ":hards\n";
    for (i, (_, items)) in mounts.iter().enumerate() {
        text += &format!(
            "word 8\nword 0\nword 30\nword 0\nword 0\nword 0\nword 0\nword 12740\nptr store{i}\nbyte 0\nword {items}\nbyte 0\n"
        );
    }
    text += &format!(
        ":ot_names\nstring \"{stem}\"\nstring \"Synthetic {stem}\"\nstring \"{stem}.NT\"\n"
    );
    text += &format!(":shape\nstring \"{stem}.SH\"\n");
    for (i, (store, _)) in mounts.iter().enumerate() {
        text += &format!(":store{i}\nstring \"{store}\"\n");
    }
    text += "end\n";
    text.into_bytes()
}

/// A synthetic static object (OT).
pub fn ot(stem: &str, class: u16, hp: i32) -> Vec<u8> {
    let object = |name: &str| -> Option<String> {
        Some(match name {
            "structType" => "1".into(),
            "ot_names" => "ot_names".into(),
            "shape" => "shape".into(),
            "obj_class" => class.to_string(),
            "hitPoints" => hp.to_string(),
            "utilProc" => "_OBJProc".into(),
            _ => return None,
        })
    };
    let mut text = String::from(HEADER);
    text += &fields(schema::OBJECT, &object);
    text += &format!(
        ":ot_names\nstring \"{stem}\"\nstring \"Synthetic {stem}\"\nstring \"{stem}.OT\"\n"
    );
    text += &format!(":shape\nstring \"{stem}.SH\"\nend\n");
    text.into_bytes()
}

/// The class a placeholder's picks carry in the fixture.
fn placeholder_class(placeholder: Placeholder) -> u16 {
    match placeholder {
        Placeholder::Sam => class::SAM,
        Placeholder::Aaa => class::AAA,
        Placeholder::Tank => class::TANK,
        Placeholder::Afv | Placeholder::Vehicle => class::VEHICLE,
        _ => class::SHIP,
    }
}

/// Where the fixture's objects stand: the middle of the synthetic theater.
pub const MIDDLE: i32 = AIRPORT_AT as i32;

/// The synthetic import with every equipment-list type, a few named types,
/// a base layout with surface units on both sides and two templates.
pub fn surface_resources() -> BTreeMap<String, Vec<u8>> {
    let mut r = resources();
    let shape = r["F18.SH"].clone();
    let add_nt = |r: &mut BTreeMap<String, Vec<u8>>, stem: &str, bytes: Vec<u8>| {
        r.insert(format!("{stem}.NT"), bytes);
        r.insert(format!("{stem}.SH"), shape.clone());
    };
    for lists in tables::LISTS {
        for group in lists.groups {
            for name in group {
                if r.contains_key(*name) {
                    continue;
                }
                let stem = name.trim_end_matches(".NT");
                let class = placeholder_class(lists.placeholder);
                let armed = matches!(class, class::SAM | class::AAA | class::TANK);
                let mounts: &[(&str, i32)] = if armed { &[("GUN.JT", 32767)] } else { &[] };
                let util = if matches!(stem, "KIEV" | "NIMZ" | "CLEM") {
                    "_CARRIERProc"
                } else {
                    "_GVProc"
                };
                add_nt(&mut r, stem, nt(stem, class, 100, util, mounts));
            }
        }
    }
    add_nt(
        &mut r,
        "GCI",
        nt("GCI", class::STRUCTURE, 100, "_OBJProc", &[("GCIR.SEE", 1)]),
    );
    add_nt(
        &mut r,
        "MISTRK",
        nt("MISTRK", class::VEHICLE, 50, "_GVProc", &[]),
    );
    add_nt(
        &mut r,
        "TROOPS",
        nt(
            "TROOPS",
            class::OTHER,
            5,
            "_GVProc",
            &[("SMLARMS.JT", 32767)],
        ),
    );
    // Ships have a damaged `_A` shape.
    for stem in ["KIEV", "KRIVAK", "CARGO"] {
        r.insert(format!("{stem}_A.SH"), shape.clone());
    }
    for stem in ["BNK5", "~BNK5", "STORE", "DEST"] {
        r.insert(format!("{stem}.OT"), ot(stem, class::STRUCTURE, 250));
        r.insert(format!("{stem}.SH"), shape.clone());
    }
    // The base layout: an enemy SA-6 (`nationality3`), a friendly ZSU-23
    // (`nationality3`), an enemy store (`nationality2`), an enemy supply
    // truck and a store with no owner.
    let place = |ty: &str, dx: i32, owner: &str| {
        format!(
            "obj\n\ttype {ty}\n\tpos {} 0 {}\n\tangle 0 0 0\n{owner}\tflags $13\n\t.\n",
            MIDDLE + dx,
            MIDDLE
        )
    };
    let layout = String::from("textFormat\nmap UKR.T2\nlayer CLEAR.LAY 0\ntime 12 0\n")
        + &place("SA6.NT", 0, "\tnationality3 152\n")
        + &place("ZSU23.NT", 500, "\tnationality3 39\n")
        + &place("STORE.OT", 1000, "\tnationality2 137\n")
        + &place("MISTRK.NT", 1500, "\tnationality3 152\n")
        + &place("STORE.OT", 2000, "");
    r.insert("UKR.MM".into(), layout.into_bytes());
    r.insert("~QUCITY.M".into(), test_template().into_bytes());
    r.insert("~QUSFLT.M".into(), fleet_template().into_bytes());
    r
}

/// One template object, at `dx` feet east of the middle.
fn object(ty: &str, dx: i32, owner: &str, flags: &str, extra: &str) -> String {
    format!(
        "obj\n\ttype {ty}\n\tpos {} 0 {}\n\tangle 90 0 0\n\t{owner}\n\tflags {flags}\n\tspeed 0\n\talias {}\n{extra}\t.\n",
        MIDDLE + dx,
        MIDDLE + 5000,
        dx / 100 + 1
    )
}

/// A defended site: 6 `<sam>` and 6 `<aaa>` slots (the first `<sam>` a
/// target), a bunker target (ordinal 12), a tank, a supply truck, a friendly
/// `nationality3` ZSU-23 (15) and a parked aircraft target (16).
fn test_template() -> String {
    let mut text = String::new();
    let mut dx = 0;
    let mut next = || {
        dx += 300;
        dx
    };
    text += &object("<sam>", next(), "nationality2 137", "$93", "\tskill 2\n");
    for _ in 0..5 {
        text += &object("<sam>", next(), "nationality2 137", "$13", "");
    }
    for _ in 0..6 {
        text += &object("<aaa>", next(), "nationality2 137", "$13", "\tskill 3\n");
    }
    text += &object("BNK5.OT", next(), "nationality2 137", "$93", "");
    text += &object("<tank>", next(), "nationality 0", "$13", "");
    text += &object("TRUCK.NT", next(), "nationality2 137", "$13", "");
    text += &object("ZSU23.NT", next(), "nationality3 39", "$13", "");
    text += &object("F18.PT", next(), "nationality2 137", "$97", "");
    text
}

/// A fleet with a carrier target and an aircraft, which is a deck launch.
fn fleet_template() -> String {
    object("KIEV.NT", 0, "nationality2 137", "$93", "")
        + &object("<destroyer>", 2000, "nationality2 137", "$13", "")
        + &object(
            "F18.PT",
            400,
            "nationality2 137",
            "$13",
            "\tstartTime 3600\n",
        )
}

pub fn target(stem: &str, aaa: usize, sam: usize, seed: u32) -> GroundTarget {
    GroundTarget {
        stem: stem.into(),
        aaa,
        sam,
        seed,
        // Russian: equipment group 2.
        enemy_nationality: 10,
        night_stealth: false,
        variation: layout::Variation::ON,
        separation_nm: 5,
    }
}

/// A Quick Mission spec of the synthetic theater that names `target`, as the
/// creator writes one: the player's F/A-18 and the target's settings.
pub fn spec_with_target(target: &GroundTarget) -> MissionSpec {
    let mut spec = MissionSpec::new(super::resources::THEATER, AircraftId::F18);
    spec.ground_target = Some(target.stem.clone());
    spec.aaa = Defense::from_level(target.aaa).expect("a defense level");
    spec.sam = Defense::from_level(target.sam).expect("a defense level");
    spec.surface_seed = target.seed;
    spec.enemy_nationality = target.enemy_nationality as u8;
    spec
}

/// A synthetic movable tank: turn rate 2,730, top speed 50.
pub fn mover_nt(stem: &str, class: u16, turn: i32, max_speed: i32) -> Vec<u8> {
    let object = |name: &str| -> Option<String> {
        Some(match name {
            "structType" => "3".into(),
            "typeSize" => "186".into(),
            "ot_names" => "ot_names".into(),
            "shape" => "shape".into(),
            "obj_class" => class.to_string(),
            "hitPoints" => "100".into(),
            "expType" => "21".into(),
            "craterSize" => "6".into(),
            "utilProc" => "_GVProc".into(),
            "_turnRate" => turn.to_string(),
            "_maxSpeed" => max_speed.to_string(),
            "_cornerSpeed" => "50".into(),
            "_acc" | "_dacc" => "50".into(),
            _ => return None,
        })
    };
    let npc = |name: &str| -> Option<String> { (name == "numHards").then(|| "0".to_owned()) };
    let mut text = String::from(HEADER);
    text += &fields(schema::OBJECT, &object);
    text += &fields(schema::NPC, &npc);
    text += &format!(
        ":ot_names\nstring \"{stem}\"\nstring \"Synthetic {stem}\"\nstring \"{stem}.NT\"\n"
    );
    text += &format!(":shape\nstring \"{stem}.SH\"\nend\n");
    text.into_bytes()
}

/// One waypoint block of a route.
pub fn waypoint(index: usize, flags: &str, head: &str, at: [i32; 3], speed: i32) -> String {
    format!(
        "\tw_index {index}\r\n\tw_flags {flags}\r\n\tw_goal 0\r\n\tw_next 0\r\n\tw_pos2 {head} {} {} {}\r\n\tw_speed {speed}\r\n\tw_wng 0 0 0 0\r\n\tw_react 0 0 0\r\n\tw_searchDist 0\r\n\tw_preferredTargetId 0\r\n\tw_name \r\n\r\n",
        at[0], at[1], at[2]
    )
}

/// An object that follows a route, and the route.
pub fn routed(
    ty: &str,
    at: [i32; 3],
    angle: i32,
    alias: i32,
    legs: &[[i32; 3]],
    speed: i32,
) -> String {
    let mut text = format!(
        "obj\r\n\ttype {ty}\r\n\tpos {} {} {}\r\n\tangle {angle} 0 0\r\n\tnationality2 137\r\n\tflags $93\r\n\tspeed 0\r\n\talias {alias}\r\n\t.\r\n",
        at[0], at[1], at[2]
    );
    text += &format!("waypoint2 {}\r\n", legs.len() + 2);
    text += &waypoint(0, "1", "0 0", at, 0);
    for (n, leg) in legs.iter().enumerate() {
        text += &waypoint(n + 1, "$4", "1 0", *leg, speed);
    }
    text += &waypoint(legs.len() + 1, "2", "0 0", [0, 0, 0], 0);
    text += &format!("  w_for {alias}\r\n\t.\r\n");
    text
}

/// The synthetic import with a routed template: a tank on a short route with
/// a corner, a boat (which keeps its water level) and a tank that
/// stands still.
pub fn routed_resources() -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut r = surface_resources();
    let shape = r["F18.SH"].clone();
    for (stem, class, turn, max) in [
        ("MOVER", class::TANK, 2730, 50),
        ("BOAT", class::SHIP, 910, 50),
        ("STAND", class::TANK, 2730, 50),
    ] {
        r.insert(format!("{stem}.NT"), mover_nt(stem, class, turn, max));
        r.insert(format!("{stem}.SH"), shape.clone());
    }
    r.insert("BOAT_A.SH".into(), shape);
    let z = MIDDLE + 8000;
    let mut text = String::from("textFormat\r\n");
    text += &routed(
        "MOVER.NT",
        [MIDDLE, 0, z],
        180,
        -1,
        &[[MIDDLE, 0, z - 800], [MIDDLE + 800, 0, z - 800]],
        50,
    );
    text += &routed(
        "BOAT.NT",
        [MIDDLE + 2000, 0, z],
        180,
        -2,
        &[[MIDDLE + 2000, 0, z - 5000]],
        16,
    );
    text += "obj\r\n\ttype STAND.NT\r\n\tpos 524288 0 540000\r\n\tangle 0 0 0\r\n\tnationality2 137\r\n\tflags $13\r\n\tspeed 0\r\n\talias -3\r\n\t.\r\n";
    r.insert("~QUCOL.M".into(), text.into_bytes());
    r
}

/// A seed-dependent impossible support footprint, entirely synthetic. The first
/// SAM pick is enlarged so its truck cannot be both clear and within supply reach.
/// The next seed selecting another type can build normally.
pub fn placement_retry_resources(seed: u32) -> (BTreeMap<String, Vec<u8>>, u32) {
    use crate::surface::{catalog::Catalog, resolve};
    use tore_formats::quick_template::Template;
    let mut r = surface_resources();
    r.insert("UKR.MM".into(), resources()["UKR.MM"].clone());
    let text = object("<sam>", 20_000, "nationality2 137", "$93", "")
        + &object("STORE.OT", 22_000, "nationality2 137", "$93", "");
    r.insert("~QUCITY.M".into(), text.into_bytes());
    let template = Template::parse("~QUCITY.M", &r["~QUCITY.M"]).unwrap();
    let selected = |seed| {
        resolve::template(
            &template,
            &target("QUCITY", 0, 3, seed),
            &mut Catalog::new(&r),
            Some("UKR.T2"),
        )
        .unwrap()
        .units[0]
            .resource
            .clone()
    };
    let bad = selected(seed);
    let mut good_seed = seed;
    loop {
        good_seed = good_seed.wrapping_add(1).max(1);
        if selected(good_seed) != bad {
            break;
        }
        assert!(good_seed.wrapping_sub(seed) < 16);
    }
    let shape = r.get_mut(&bad.replace(".NT", ".SH")).unwrap();
    // The fixture's first vertex X also supplies the CODE scale exponent.
    // Keep it at 8 (one foot per unit), within the reviewed 0..=16 bound.
    // Its three i16 vertices follow the six-byte opcode.
    for (index, vertex) in [[8i16, -30_000, 0], [30_000, -30_000, 0], [0, 30_000, 0]]
        .into_iter()
        .enumerate()
    {
        for (axis, word) in vertex.into_iter().enumerate() {
            let at = 256 + 6 + index * 6 + axis * 2;
            shape[at..at + 2].copy_from_slice(&word.to_le_bytes());
        }
    }
    assert_eq!(tore_formats::shape::object_scale(shape).unwrap(), 1.);
    (r, good_seed)
}
