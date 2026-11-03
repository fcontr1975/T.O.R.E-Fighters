//! The layout rules on a synthetic grid with synthetic units: jitter,
//! fallbacks, anchoring, relocation, starts, batteries, trucks and the
//! digest (plan 9.1, L1). No retail data. World-level checks (the player's
//! start, the automatic runway) are in `surface::tests`.
use super::{sites::Rule, *};
use crate::ai_wings::{ENEMY_SIDE, FRIENDLY_SIDE};
use crate::surface::{
    BATTERY_RADAR_BASE, Battery, BatterySystem, DestroyedLook, IdRange, LeftOut, ParkedAircraft,
    SUPPLY_TRUCK_BASE, SupplyTruck, TemplateSite, UnitKind,
    catalog::{Entry, Family},
    resolve::GroundTarget,
};
use std::collections::BTreeMap;
use tore_formats::{quick_template::Placeholder, surface_unit::class, theater::TerrainCell};
use tore_sim::combat::live::Side;

/// A square grid, `size` cells a side.
struct Grid {
    size: usize,
    cells: Vec<TerrainCell>,
}

impl Grid {
    fn uniform(size: usize, class: u8, elevation: u8) -> Self {
        Self {
            size,
            cells: vec![
                TerrainCell {
                    color: 0,
                    class,
                    elevation,
                };
                size * size
            ],
        }
    }
    fn land(size: usize) -> Self {
        Self::uniform(size, 2, 1)
    }
    fn set(&mut self, col: usize, row: usize, class: u8) {
        self.cells[row * self.size + col].class = class;
    }
    /// A shore cell drawn as the sea: the water colour with a land class.
    fn set_sea_colour(&mut self, col: usize, row: usize) {
        self.cells[row * self.size + col].color = ground::WATER_COLOR;
    }
}

impl Cells for Grid {
    fn cols(&self) -> usize {
        self.size
    }
    fn rows(&self) -> usize {
        self.size
    }
    fn cell(&self, col: usize, row: usize) -> TerrainCell {
        self.cells[row * self.size + col]
    }
}

/// The middle of a 128-cell grid, feet.
const MID: i32 = 64 * 8192;

/// A unit of `resource` at `x`, `z` feet, with the kind and slot its class
/// implies.
fn unit(id: UnitId, resource: &str, class: u16, at: [i32; 2], side: Side) -> Unit {
    let kind = if resource.ends_with(".OT") {
        UnitKind::Structure
    } else {
        UnitKind::Active
    };
    let placeholder = match class {
        class::SAM => Some(Placeholder::Sam),
        class::AAA => Some(Placeholder::Aaa),
        _ => None,
    };
    let origin = match id.range() {
        IdRange::Template => Origin::Template {
            ordinal: id.0 - crate::surface::SURFACE_UNIT_BASE,
            placeholder,
        },
        _ => Origin::Layout {
            ordinal: id.0 - crate::surface::LAYOUT_OBJECT_BASE,
        },
    };
    Unit {
        id,
        origin,
        resource: resource.into(),
        kind,
        class,
        name: resource.into(),
        nationality: Some(if side == ENEMY_SIDE { 0x8a } else { 0 }),
        side,
        position: [at[0], 0, at[1]],
        angles: [90, 0, 0],
        flags: 0x13,
        skill: 1,
        react: None,
        search_dist: None,
        start_time: None,
        route: None,
        hit_points: 100,
        look: DestroyedLook::Removed,
        explosion: None,
        crater: None,
        supply_truck: false,
        in_scene: true,
    }
}

fn t(ordinal: u32) -> UnitId {
    UnitId::template(ordinal).unwrap()
}

fn l(ordinal: u32) -> UnitId {
    UnitId::layout(ordinal).unwrap()
}

fn target(seed: u32, variation: Variation) -> GroundTarget {
    GroundTarget {
        stem: "QTEST".into(),
        aaa: 3,
        sam: 3,
        seed,
        enemy_nationality: 10,
        night_stealth: false,
        variation,
        separation_nm: 20,
    }
}

const JITTER_ONLY: Variation = Variation {
    jitter: true,
    relocate: false,
    front: false,
};

/// A surface holding `units`, with a template site when `settings` is
/// given, as resolution would leave it.
fn surface(units: Vec<Unit>, settings: Option<GroundTarget>) -> Surface {
    let mut units = units;
    units.sort_by_key(|u| u.id);
    let placement = |u: &Unit| tore_formats::mission::Placement {
        key: tore_formats::mission::SourceKey {
            layout: "~QTEST.M".into(),
            ordinal: u.id.0,
        },
        section: None,
        object_type: u.resource.clone(),
        position: u.position,
        angles: u.angles,
        source_nationality: u.nationality,
        nationality2: false,
        nationality3: true,
        nationality: u.nationality,
        flags: Some(u.flags),
        speed: Some(0),
        name: None,
        alias: None,
        unknown: Vec::new(),
    };
    Surface {
        object_sides: units.iter().map(|u| (u.id.0, u.side)).collect(),
        placements: units
            .iter()
            .filter(|u| u.id.range() == IdRange::Template)
            .map(|u| (u.id, placement(u)))
            .collect(),
        template: settings.map(|settings| TemplateSite {
            stem: settings.stem.clone(),
            settings,
            group: 2,
            quickpos: None,
            objects: 0,
            removed: Vec::new(),
            left_out: Vec::<LeftOut>::new(),
            anchor: None,
        }),
        units,
        ..Surface::default()
    }
}

/// Every type is a 40 by 60 ft box, except strips (400 by 6,000 ft along
/// their heading) and the SA-2 site (800 ft square).
fn types(name: &str) -> TypeInfo {
    let square = |half: i64| Footprint {
        min: [-half, -half],
        max: [half, half],
    };
    TypeInfo {
        footprint: if name.starts_with("STRIP") {
            Footprint {
                min: [-200, 0],
                max: [200, 6_000],
            }
        } else if name == "SA2A.NT" {
            square(400)
        } else {
            Footprint {
                min: [-20, -30],
                max: [20, 30],
            }
        },
        strip: name.starts_with("STRIP"),
        bridge_or_road: name.starts_with("BRD"),
    }
}

fn entry(resource: &str) -> Arc<Entry> {
    let (class, truck) = match resource {
        "GCI.NT" => (class::STRUCTURE, false),
        "MISTRK.NT" | "TRUCK.NT" => (class::VEHICLE, true),
        _ => (class::VEHICLE, false),
    };
    Arc::new(Entry {
        resource: resource.into(),
        family: Family::Unit(UnitKind::Passive),
        class,
        name: resource.into(),
        hit_points: 50,
        look: DestroyedLook::Removed,
        explosion: Some(21),
        crater: Some(6),
        supply_truck: truck,
        carrier: false,
        unit: None,
        aircraft: None,
    })
}

/// Lays `surface` out on `ground`; every added type is available but
/// `missing`.
fn lay(surface: &mut Surface, ground: &Ground<'_>, missing: &[&str]) {
    let mut types = types;
    let mut added = |name: &str| (!missing.contains(&name)).then(|| entry(name));
    place(
        surface,
        &mut Inputs {
            ground,
            types: &mut types,
            added: &mut added,
            layout: "TEST.MM",
        },
    );
}

fn at(unit: &Unit) -> [i64; 2] {
    xz(unit.position)
}

fn in_band(d2: i64, [lo, hi]: [i64; 2]) -> bool {
    d2 >= (lo - 1).pow(2) && d2 <= (hi + 1).pow(2)
}

/// A defended site at the middle of the map: four SAM slots (the first a
/// target), two AAA slots, a bunker target, a tank.
fn site_units() -> Vec<Unit> {
    let mut units = vec![
        unit(t(0), "SA9.NT", class::SAM, [MID, MID], ENEMY_SIDE),
        unit(t(1), "SA9.NT", class::SAM, [MID + 3_000, MID], ENEMY_SIDE),
        unit(t(2), "SA9.NT", class::SAM, [MID, MID + 3_000], ENEMY_SIDE),
        unit(t(3), "SA9.NT", class::SAM, [MID - 3_000, MID], ENEMY_SIDE),
        unit(t(4), "ZSU23.NT", class::AAA, [MID, MID - 3_000], ENEMY_SIDE),
        unit(
            t(5),
            "ZSU23.NT",
            class::AAA,
            [MID + 2_000, MID + 2_000],
            ENEMY_SIDE,
        ),
        unit(
            t(6),
            "BNK5.OT",
            class::STRUCTURE,
            [MID + 500, MID + 500],
            ENEMY_SIDE,
        ),
        unit(
            t(7),
            "T72.NT",
            class::TANK,
            [MID - 1_000, MID - 1_000],
            ENEMY_SIDE,
        ),
    ];
    units[0].flags |= 0x80;
    units[6].flags |= 0x80;
    units
}

fn positions(surface: &Surface) -> BTreeMap<UnitId, ([i32; 3], i32)> {
    surface
        .units
        .iter()
        .map(|u| (u.id, (u.position, u.angles[0])))
        .collect()
}

#[test]
fn jitter_is_seeded_bounded_and_skips_structures() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let build = |seed| {
        let mut s = surface(site_units(), Some(target(seed, JITTER_ONLY)));
        lay(&mut s, &ground, &[]);
        s
    };
    let (a, b, c) = (build(7), build(7), build(8));
    assert_eq!(positions(&a), positions(&b));
    assert_eq!(a.digest(), b.digest());
    assert_ne!(positions(&a), positions(&c));
    assert_ne!(a.digest(), c.digest());
    let retail: BTreeMap<UnitId, Unit> = site_units().into_iter().map(|u| (u.id, u)).collect();
    let mut moved = 0;
    for unit in a.template_units() {
        let before = &retail[&unit.id];
        let d = distance2(at(unit), at(before));
        let turn = trig::signed(unit.angles[0] - before.angles[0]).abs();
        match Rule::of(unit) {
            Rule::Defense => {
                assert!(d <= DEFENSE_JITTER_FT.pow(2), "{:?}", unit.id);
                assert!(turn <= DEFENSE_JITTER_DEG);
            }
            Rule::Vehicle => {
                assert!(d <= VEHICLE_JITTER_FT.pow(2));
                assert!(turn <= VEHICLE_JITTER_DEG);
            }
            Rule::Fixed => assert_eq!((d, turn), (0, 0)),
            other => panic!("unexpected {other:?}"),
        }
        moved += usize::from(d > 0);
    }
    assert_eq!(moved, 7, "every unit but the bunker moves");
    // The scene placements follow their units.
    for (id, placement) in &a.placements {
        let unit = a.unit(*id).unwrap();
        assert_eq!(
            (placement.position, placement.angles),
            (unit.position, unit.angles)
        );
    }
    // With jitter off nothing moves.
    let mut off = surface(site_units(), Some(target(7, Variation::OFF)));
    lay(&mut off, &ground, &[]);
    for unit in off.template_units() {
        assert_eq!(unit.position, retail[&unit.id].position);
    }
}

#[test]
fn a_units_jitter_does_not_depend_on_the_others() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut all = surface(site_units(), Some(target(3, JITTER_ONLY)));
    lay(&mut all, &ground, &[]);
    // The same site with slot 2 lost to its roll, given in reverse order.
    let mut fewer: Vec<Unit> = site_units().into_iter().filter(|u| u.id != t(2)).collect();
    fewer.reverse();
    let mut fewer = surface(fewer, Some(target(3, JITTER_ONLY)));
    lay(&mut fewer, &ground, &[]);
    for unit in fewer.template_units() {
        let same = all.unit(unit.id).unwrap();
        assert_eq!((unit.position, unit.angles), (same.position, same.angles));
    }
}

#[test]
fn a_unit_with_nowhere_to_stand_keeps_its_retail_spot() {
    // A theater object covering everything around the site: no candidate
    // stands, so every unit stays put.
    let grid = Grid::land(128);
    let cover = Placed {
        id: l(0).0,
        at: [i64::from(MID), i64::from(MID)],
        heading: 0,
        footprint: Footprint {
            min: [-6_000, -6_000],
            max: [6_000, 6_000],
        },
    };
    let ground = Ground::new(&grid, vec![cover], Vec::new(), None);
    let mut s = surface(site_units(), Some(target(5, JITTER_ONLY)));
    lay(&mut s, &ground, &[]);
    let retail: BTreeMap<UnitId, Unit> = site_units().into_iter().map(|u| (u.id, u)).collect();
    for unit in s.template_units() {
        assert_eq!(unit.position, retail[&unit.id].position);
    }
    // A ship on land has nowhere to go either; at sea it moves.
    let ship = |grid: &Grid| {
        let ground = Ground::new(grid, Vec::new(), Vec::new(), None);
        let mut s = surface(
            vec![unit(t(0), "KRIVAK.NT", class::SHIP, [MID, MID], ENEMY_SIDE)],
            Some(target(5, Variation::ON)),
        );
        lay(&mut s, &ground, &[]);
        s.units[0].position
    };
    assert_eq!(ship(&Grid::land(128)), [MID, 0, MID]);
    assert_ne!(
        ship(&Grid::uniform(128, ground::WATER_CLASS, 0)),
        [MID, 0, MID]
    );
}

fn object(ordinal: u32, at: [i64; 2], target: bool) -> sites::Object {
    sites::Object {
        ordinal,
        item: sites::Item::Unit(ordinal as usize),
        retail: at,
        heading: 0,
        rule: Rule::Defense,
        water: false,
        info: TypeInfo::default(),
        target,
        routed: false,
    }
}

#[test]
fn templates_anchor_by_rule() {
    let grid = Grid::land(128);
    let mid = i64::from(MID);
    let runway = Placed {
        id: l(9).0,
        at: [mid + 5_000, mid],
        heading: 0,
        footprint: types("STRIP.OT").footprint,
    };
    let house = Placed {
        id: l(8).0,
        at: [mid - 20_000, mid],
        heading: 0,
        footprint: types("HOUSE.OT").footprint,
    };
    let open = Ground::new(&grid, vec![house], Vec::new(), None);
    let airfield = Ground::new(&grid, vec![house], vec![runway], None);
    let site = vec![
        object(0, [mid, mid], true),
        object(1, [mid + 900, mid], false),
    ];
    assert_eq!(sites::anchor(&site, &open), None);
    // Its centroid within 1 nm of a runway (the runway's box is 4,350 ft
    // away), or a strip piece of its own.
    assert_eq!(sites::anchor(&site, &airfield), Some(Anchor::Runway));
    let mut strip = site.clone();
    strip[1].info.strip = true;
    assert_eq!(sites::anchor(&strip, &open), Some(Anchor::Strip));
    let mut bridge = site.clone();
    bridge[1].info.bridge_or_road = true;
    assert_eq!(sites::anchor(&bridge, &open), Some(Anchor::BridgeOrRoad));
    // A target within 2,000 ft of a layout object; a defense there does
    // not count.
    let mut town = site.clone();
    town[0].retail = [mid - 18_500, mid];
    assert_eq!(sites::anchor(&town, &open), Some(Anchor::Town));
    let mut not_town = site.clone();
    not_town[1].retail = [mid - 18_500, mid];
    assert_eq!(sites::anchor(&not_town, &open), None);
    let mut routed = site;
    routed[1].routed = true;
    assert_eq!(sites::anchor(&routed, &airfield), Some(Anchor::Route));
}

#[test]
fn an_anchored_template_never_moves() {
    let grid = Grid::land(128);
    let mid = i64::from(MID);
    let runway = Placed {
        id: l(9).0,
        at: [mid + 5_000, mid],
        heading: 0,
        footprint: types("STRIP.OT").footprint,
    };
    let ground = Ground::new(&grid, Vec::new(), vec![runway], None);
    for seed in 0..10 {
        let mut s = surface(site_units(), Some(target(seed, Variation::ON)));
        lay(&mut s, &ground, &[]);
        assert_eq!(s.template.as_ref().unwrap().anchor, Some(Anchor::Runway));
        assert!(s.transform.is_identity());
        assert_eq!(at(s.unit(t(6)).unwrap()), [mid + 500, mid + 500]);
    }
}

#[test]
fn a_relocated_site_obeys_the_site_rules() {
    let grid = Grid::land(128);
    let mid = i64::from(MID);
    // Blue to the south, Red to the north; scattered theater objects.
    let front = Front {
        blue: [mid, mid - 200_000],
        red: [mid, mid + 200_000],
    };
    let objects: Vec<Placed> = (0..40)
        .map(|i| Placed {
            id: l(i).0,
            at: [
                mid - 300_000 + i64::from(i % 8) * 80_000,
                mid - 200_000 + i64::from(i / 8) * 90_000,
            ],
            heading: 0,
            footprint: types("HOUSE.OT").footprint,
        })
        .collect();
    let ground = Ground::new(&grid, objects, Vec::new(), Some(front));
    let mut relocated = 0;
    for seed in 0..20 {
        let mut s = surface(site_units(), Some(target(seed, Variation::ON)));
        lay(&mut s, &ground, &[]);
        let mut types = types;
        assert_eq!(
            audit(&s, &ground, &mut types),
            Vec::<String>::new(),
            "seed {seed}"
        );
        if s.transform.is_identity() {
            continue;
        }
        relocated += 1;
        let shift = s.transform.translation.map(i64::from);
        let d2 = shift[0].pow(2) + shift[1].pow(2);
        assert!(in_band(
            d2,
            [RELOCATE_MIN_NM * NM_FT, RELOCATE_MAX_NM * NM_FT]
        ));
        // The bunker moved exactly with the group and turned with it.
        let bunker = s.unit(t(6)).unwrap();
        assert_eq!(
            at(bunker),
            sites::apply(&s.transform, [mid + 500, mid + 500])
        );
        assert_eq!(
            bunker.angles[0],
            trig::signed(90 + s.transform.rotation_deg)
        );
        // Still on the retail spot's side of the front, within 15 nm of its
        // depth.
        let pivot = s.transform.pivot.map(i64::from);
        assert!(front.same_band(
            [pivot[0] + shift[0], pivot[1] + shift[1]],
            pivot,
            RELOCATE_DEPTH_NM * NM_FT
        ));
    }
    assert!(relocated >= 15, "{relocated} of 20 relocated");
}

#[test]
fn a_cell_drawn_as_the_sea_is_water_unless_a_ground_texture_covers_it() {
    let mut grid = Grid::land(128);
    // Class 0 at sea level in the water colour: the Falklands' shore cells.
    grid.set(10, 10, 0);
    grid.set_sea_colour(10, 10);
    // Open sea by its class byte, whatever its colour.
    grid.set(20, 20, ground::WATER_CLASS);
    let feet = |cell: i64| [cell * ground::CELL_FT + 100, cell * ground::CELL_FT + 100];
    let bare = Ground::new(&grid, Vec::new(), Vec::new(), None);
    assert!(bare.water(feet(10)) && bare.water(feet(20)));
    assert!(!bare.water(feet(11)));
    // A texture placement (cell 8,8, four cells a side) draws its own shore:
    // the class byte alone says what is water under it.
    let tiled = Ground::new(&grid, Vec::new(), Vec::new(), None).with_tiles([(8, 8)]);
    assert!(!tiled.water(feet(10)));
    assert!(
        tiled.water(feet(20)),
        "class 1 is water under a texture too"
    );
}

/// Whether the grid cell under a unit is the water colour with no texture
/// over it, read straight from the cells (not from `Ground::water`).
fn drawn_as_sea(grid: &Grid, unit: &Unit) -> bool {
    let at = at(unit);
    let (c, r) = (
        (at[0] / ground::CELL_FT) as usize,
        (at[1] / ground::CELL_FT) as usize,
    );
    grid.cells[r * grid.size + c].color == ground::WATER_COLOR
}

#[test]
fn nothing_the_layout_moves_stands_on_a_shore_cell_drawn_as_the_sea() {
    // A band of sea-coloured class-0 cells between the retail site and the
    // east: 50 to 55 of 128 columns, all rows.
    let mut grid = Grid::land(128);
    for c in 50..=55 {
        for r in 0..128 {
            grid.set(c, r, 0);
            grid.set_sea_colour(c, r);
        }
    }
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut on_band = 0;
    for seed in 1..=40 {
        let mut s = surface(site_units(), Some(target(seed, Variation::ON)));
        lay(&mut s, &ground, &[]);
        assert!(
            class_audit(&s, &ground, None).is_empty(),
            "seed {seed}: {:?}",
            class_audit(&s, &ground, None)
        );
        for unit in &s.units {
            assert!(!drawn_as_sea(&grid, unit), "seed {seed}: {:?}", unit.id);
        }
        assert!(
            sites::audit(&s, &ground, &mut types).is_empty(),
            "seed {seed}"
        );
    }
    // Control: with a texture over the band the same seeds may stand there.
    let tiled = Ground::new(&grid, Vec::new(), Vec::new(), None)
        .with_tiles((12..=13).flat_map(|c| (0..32).map(move |r| (c * 4, r * 4))));
    for seed in 1..=40 {
        let mut s = surface(site_units(), Some(target(seed, Variation::ON)));
        lay(&mut s, &tiled, &[]);
        on_band += s.units.iter().filter(|u| drawn_as_sea(&grid, u)).count();
    }
    assert!(
        on_band > 0,
        "the band is reachable once a texture covers it"
    );
}

#[test]
fn the_class_audit_reports_a_moved_unit_on_the_wrong_class_and_spares_retail() {
    let mut grid = Grid::land(128);
    for c in 0..64 {
        for r in 0..128 {
            grid.set(c, r, ground::WATER_CLASS);
        }
    }
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    // A tank in the sea and a ship on land.
    let sea = [MID - 40_000, MID];
    let land = [MID + 40_000, MID];
    let units = vec![
        unit(t(0), "T72.NT", class::TANK, sea, ENEMY_SIDE),
        unit(t(1), "CARGO.NT", class::SHIP, land, ENEMY_SIDE),
    ];
    let s = surface(units.clone(), Some(target(1, Variation::OFF)));
    // Both are where retail put them: not judged. Without a retail surface
    // to compare with, both are.
    assert!(class_audit(&s, &ground, Some(&s)).is_empty());
    let found = class_audit(&s, &ground, None);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("a land unit on water"));
    assert!(found[1].contains("a ship on land"));
    // The same units moved by one foot are the layout's doing.
    let mut moved = units;
    moved[0].position[0] += 1;
    let moved = surface(moved, Some(target(1, Variation::OFF)));
    assert_eq!(class_audit(&moved, &ground, Some(&s)).len(), 1);
}

#[test]
fn a_site_with_nowhere_to_go_stays_at_its_retail_spot() {
    // An island of three by three cells in an open sea: every candidate 3
    // to 30 nm away puts a target or the bunker in the water.
    let mut grid = Grid::uniform(128, ground::WATER_CLASS, 0);
    for c in 63..=65 {
        for r in 63..=65 {
            grid.set(c, r, 2);
        }
    }
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut s = surface(site_units(), Some(target(1, Variation::ON)));
    lay(&mut s, &ground, &[]);
    assert!(s.transform.is_identity());
    assert_eq!(s.template.as_ref().unwrap().anchor, None);
}

#[test]
fn relocation_never_changes_the_rolls_or_the_counts() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let ids = |variation| {
        let mut s = surface(site_units(), Some(target(11, variation)));
        lay(&mut s, &ground, &[]);
        // The front line is a variation of its own, not a roll.
        let units: Vec<(UnitId, String)> = s
            .units
            .iter()
            .filter(|u| u.id.range() != IdRange::FrontLine)
            .map(|u| (u.id, u.resource.clone()))
            .collect();
        (units, s.trucks.len(), s.batteries.len(), s.transform)
    };
    let (on, off) = (ids(Variation::ON), ids(Variation::OFF));
    assert!(!on.3.is_identity() && off.3.is_identity());
    assert_eq!((on.0, on.1, on.2), (off.0, off.1, off.2));
}

#[test]
fn red_defends_the_target_and_blue_starts_the_separation_away() {
    let grid = Grid::land(128);
    let mid = i64::from(MID);
    let front = Front {
        blue: [mid, mid - 200_000],
        red: [mid, mid + 200_000],
    };
    let strip = |ordinal: u32, dz: i64| Placed {
        id: l(ordinal).0,
        at: [mid, mid + dz],
        heading: 0,
        footprint: types("STRIP.OT").footprint,
    };
    let runways = vec![
        strip(1, -10 * NM_FT),
        strip(2, -20 * NM_FT),
        strip(3, -40 * NM_FT),
        strip(4, 25 * NM_FT),
    ];
    let ground = Ground::new(&grid, Vec::new(), runways, Some(front));
    let margin = START_MARGIN_CELLS * ground::CELL_FT;
    let gap = |a: [i32; 2], b: [i32; 2]| {
        let d = [i64::from(a[0] - b[0]), i64::from(a[1] - b[1])];
        d[0].pow(2) + d[1].pow(2)
    };
    let mut reds = std::collections::BTreeSet::new();
    for seed in 0..30 {
        let mut s = surface(site_units(), Some(target(seed, Variation::OFF)));
        s.object_sides.insert(l(2).0, FRIENDLY_SIDE);
        s.object_sides.insert(l(4).0, ENEMY_SIDE);
        lay(&mut s, &ground, &[]);
        let starts = s.starts.clone().expect("starts");
        // The targets' centroid: the SAM and the bunker.
        assert_eq!(starts.target, [MID + 250, MID + 250]);
        // Red within 5 nm of it, at a seeded spot.
        assert!(gap(starts.red, starts.target) <= (RED_START_NM * NM_FT).pow(2));
        reds.insert(starts.red);
        // Blue the separation (20 nm) from Red, toward Blue (south) within
        // the spread, heading at the target, on the map.
        assert!(in_band(
            gap(starts.blue, starts.red),
            [20 * NM_FT, 20 * NM_FT]
        ));
        let from_red = [
            i64::from(starts.blue[0] - starts.red[0]),
            i64::from(starts.blue[1] - starts.red[1]),
        ];
        let bearing = trig::bearing(from_red);
        assert!(
            trig::signed(bearing - 180).abs() <= BLUE_START_SPREAD_DEG,
            "{bearing}"
        );
        let to_target = [
            i64::from(starts.target[0] - starts.blue[0]),
            i64::from(starts.target[1] - starts.blue[1]),
        ];
        assert_eq!(starts.blue_heading_deg, trig::bearing(to_target));
        assert!(ground.inside(starts.blue.map(i64::from), margin));
        // Airfields 15 nm or more from the target, nearest first, by owner,
        // else by their side of the front.
        assert_eq!(starts.blue_airfields, [l(2).0, l(3).0]);
        assert_eq!(starts.red_airfields, [l(4).0]);
    }
    assert!(reds.len() > 20, "Red's spot varies with the seed");
    // A target near the south edge: the bearing turns until Blue fits.
    let near_edge = || {
        site_units()
            .into_iter()
            .map(|mut u| {
                u.position[2] -= MID - 40_000;
                u
            })
            .collect::<Vec<Unit>>()
    };
    let mut s = surface(near_edge(), Some(target(2, Variation::OFF)));
    lay(&mut s, &ground, &[]);
    let starts = s.starts.unwrap();
    assert!(ground.inside(starts.blue.map(i64::from), margin));
    assert!(ground.inside(starts.red.map(i64::from), margin));
    assert!(in_band(
        gap(starts.blue, starts.red),
        [20 * NM_FT, 20 * NM_FT]
    ));
    // A separation longer than the map: the farthest that fits.
    let mut far = target(2, Variation::OFF);
    far.separation_nm = 300;
    let mut s = surface(site_units(), Some(far));
    lay(&mut s, &ground, &[]);
    let starts = s.starts.unwrap();
    assert!(ground.inside(starts.blue.map(i64::from), margin));
    let d2 = gap(starts.blue, starts.red);
    assert!(
        d2 < (300 * NM_FT).pow(2) && d2 > (60 * NM_FT).pow(2),
        "{d2}"
    );
}

/// Launchers and radars for the battery tests: template SA-6s and SA-2s
/// about the middle, base-layout SA-3s with two GCIs to the west and a
/// friendly SA-6.
fn battery_units() -> Vec<Unit> {
    let sa6 = |o, dx, dz| unit(t(o), "SA6.NT", class::SAM, [MID + dx, MID + dz], ENEMY_SIDE);
    vec![
        // A chain of three, each within 1 nm of the next, then one far off.
        sa6(0, 0, 0),
        sa6(1, 5_000, 0),
        sa6(2, 10_000, 0),
        sa6(3, 60_000, 0),
        // Six in one cluster: two batteries of up to four.
        sa6(4, 0, 40_000),
        sa6(5, 1_000, 40_000),
        sa6(6, 2_000, 40_000),
        sa6(7, 0, 41_000),
        sa6(8, 1_000, 41_000),
        sa6(9, 2_000, 41_000),
        // An existing Straight Flush 1.5 nm from the chain's centroid.
        unit(
            t(10),
            "SFLUSH.NT",
            class::VEHICLE,
            [MID + 5_000, MID + 9_000],
            ENEMY_SIDE,
        ),
        // Two SA-2 sites near each other: one battery each.
        unit(
            t(11),
            "SA2A.NT",
            class::SAM,
            [MID, MID - 40_000],
            ENEMY_SIDE,
        ),
        unit(
            t(12),
            "SA2A.NT",
            class::SAM,
            [MID + 3_000, MID - 40_000],
            ENEMY_SIDE,
        ),
        // Base layout: SA-3s with a GCI 4 nm off (too far) and one 1 nm off;
        // a friendly SA-6 never joins an enemy battery.
        unit(l(0), "SA3.NT", class::SAM, [MID - 100_000, MID], ENEMY_SIDE),
        unit(l(1), "SA3.NT", class::SAM, [MID - 99_000, MID], ENEMY_SIDE),
        unit(
            l(2),
            "GCI.NT",
            class::STRUCTURE,
            [MID - 99_500, MID + 4 * 6_076],
            ENEMY_SIDE,
        ),
        unit(
            l(3),
            "GCI.NT",
            class::STRUCTURE,
            [MID - 99_500, MID + 6_076],
            ENEMY_SIDE,
        ),
        unit(
            l(4),
            "SA6.NT",
            class::SAM,
            [MID - 100_000, MID + 500],
            FRIENDLY_SIDE,
        ),
    ]
}

#[test]
fn launchers_form_batteries_with_adopted_or_added_radars() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut s = surface(battery_units(), Some(target(4, Variation::OFF)));
    lay(&mut s, &ground, &[]);
    let by_lead: BTreeMap<UnitId, &Battery> =
        s.batteries.iter().map(|b| (b.launchers[0], b)).collect();
    // Battery order: ascending lowest launcher, the layout's first.
    let leads: Vec<UnitId> = s.batteries.iter().map(|b| b.launchers[0]).collect();
    let mut sorted = leads.clone();
    sorted.sort();
    assert_eq!(leads, sorted);
    // Base layout SA-3s: one battery, adopting the near GCI with its id.
    let sa3 = by_lead[&l(0)];
    assert_eq!(
        (sa3.system, sa3.radar, sa3.radar_added),
        (BatterySystem::Sa3, l(3), false)
    );
    assert_eq!(sa3.launchers, [l(0), l(1)]);
    assert_eq!(sa3.truck, None, "base layouts get no trucks");
    // The friendly SA-6 fights alone, with an added radar of its side.
    let friendly = by_lead[&l(4)];
    assert_eq!((friendly.side, friendly.radar_added), (FRIENDLY_SIDE, true));
    // The chain joins and adopts the Straight Flush; the far one is alone.
    let chain = by_lead[&t(0)];
    assert_eq!(chain.launchers, [t(0), t(1), t(2)]);
    assert_eq!((chain.radar, chain.radar_added), (t(10), false));
    let alone = by_lead[&t(3)];
    assert_eq!(alone.launchers, [t(3)]);
    assert!(alone.radar_added);
    // Six in one cluster: two batteries of at most four, every launcher in
    // exactly one.
    let split: Vec<&Battery> = s
        .batteries
        .iter()
        .filter(|b| {
            b.launchers
                .iter()
                .any(|id| (4..10).map(t).any(|x| x == *id))
        })
        .collect();
    assert_eq!(split.len(), 2);
    assert!(split.iter().all(|b| b.launchers.len() <= 4));
    assert_eq!(split.iter().map(|b| b.launchers.len()).sum::<usize>(), 6);
    // SA-2: one site per battery.
    assert_eq!(by_lead[&t(11)].launchers, [t(11)]);
    assert_eq!(by_lead[&t(12)].launchers, [t(12)]);
    // Added radars: numbered in battery order, beside their launchers, of
    // their system's element, never targets, in the scene.
    let added: Vec<&Battery> = s.batteries.iter().filter(|b| b.radar_added).collect();
    assert_eq!(added.len(), 6);
    for (n, battery) in added.iter().enumerate() {
        assert_eq!(battery.radar.0, BATTERY_RADAR_BASE + n as u32);
        let radar = s.unit(battery.radar).expect("the radar is a unit");
        assert_eq!(radar.origin, Origin::Added);
        assert_eq!(radar.flags & 0x80, 0);
        assert_eq!(
            (radar.side, s.side_of(radar.id.0)),
            (battery.side, battery.side)
        );
        let centre = centroid(battery.launchers.iter().map(|id| at(s.unit(*id).unwrap()))).unwrap();
        let band = if battery.system == BatterySystem::Sa2 {
            SA2_RADAR_FT
        } else {
            BATTERY_RADAR_FT
        };
        assert!(in_band(distance2(at(radar), centre), band));
        assert_eq!(radar.resource, batteries::element(battery.system));
        assert!(s.placements.iter().any(|(id, _)| *id == radar.id));
    }
    assert_eq!(batteries::element(BatterySystem::Sa2), "GCI.NT");
    assert_eq!(batteries::element(BatterySystem::Sa6), "SFLUSH.NT");
    assert_eq!(
        batteries::element(BatterySystem::Hawk),
        crate::surface::catalog::HAWK_RADAR
    );
}

#[test]
fn a_battery_whose_radar_cannot_be_added_is_not_formed() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut s = surface(battery_units(), Some(target(4, Variation::OFF)));
    lay(&mut s, &ground, &["SFLUSH.NT"]);
    // The SA-6 battery that adopts stays; those needing an added Straight
    // Flush are left out, and say so.
    assert!(s.batteries.iter().any(|b| b.launchers[0] == t(0)));
    assert!(!s.batteries.iter().any(|b| b.launchers[0] == t(3)));
    assert!(s.layout_notes.iter().any(|n| n.contains("SFLUSH.NT")));
}

#[test]
fn every_manned_slot_and_battery_gets_a_truck_beside_it() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut units = site_units();
    units.push(unit(
        t(8),
        "SA6.NT",
        class::SAM,
        [MID + 20_000, MID],
        ENEMY_SIDE,
    ));
    // A named launcher, not a slot: only the battery's truck serves it.
    let mut named = unit(t(9), "SA6.NT", class::SAM, [MID + 21_000, MID], ENEMY_SIDE);
    named.origin = Origin::Template {
        ordinal: 9,
        placeholder: None,
    };
    units.push(named);
    let mut s = surface(units, Some(target(9, Variation::ON)));
    lay(&mut s, &ground, &[]);
    // Four SAM, two AAA and the SA-6 slot; one battery.
    assert_eq!(s.batteries.len(), 1);
    let added: Vec<&SupplyTruck> = s.trucks.iter().filter(|t| t.added).collect();
    assert_eq!(added.len(), 7 + 1);
    for (n, truck) in added.iter().enumerate() {
        assert_eq!(truck.id.0, SUPPLY_TRUCK_BASE + n as u32);
        let unit = s.unit(truck.id).unwrap();
        let served = s.unit(truck.serves.unwrap()).unwrap();
        assert!(unit.supply_truck && unit.flags & 0x80 == 0);
        assert_eq!(unit.side, served.side);
        let want = if served.class == class::AAA {
            AAA_TRUCK
        } else {
            SAM_TRUCK
        };
        assert_eq!(unit.resource, want);
        assert!(in_band(distance2(at(unit), at(served)), TRUCK_FT));
        assert!(s.placements.iter().any(|(id, _)| *id == unit.id));
    }
    // Numbered in the order of the units they serve.
    let served: Vec<UnitId> = added.iter().map(|t| t.serves.unwrap()).collect();
    let mut sorted = served.clone();
    sorted.sort();
    assert_eq!(served, sorted);
    // The battery's truck serves its launcher without a slot truck.
    let battery_truck = s.batteries[0].truck.unwrap();
    assert_eq!(
        added.iter().find(|t| t.id == battery_truck).unwrap().serves,
        Some(t(9))
    );
}

#[test]
fn base_layouts_get_no_trucks_and_lay_out_alike_in_every_mission() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let radar_of_friendly = |seed| {
        let mut s = surface(battery_units(), Some(target(seed, Variation::ON)));
        lay(&mut s, &ground, &[]);
        assert!(
            s.trucks
                .iter()
                .filter(|t| t.added)
                .all(|t| t.serves.unwrap().range() == IdRange::Template)
        );
        let battery = s.batteries.iter().find(|b| b.launchers[0] == l(4)).unwrap();
        s.unit(battery.radar).unwrap().position
    };
    assert_eq!(radar_of_friendly(1), radar_of_friendly(2));
}

#[test]
fn the_digest_repeats_and_follows_the_seed() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let build = |seed| {
        let mut s = surface(battery_units(), Some(target(seed, Variation::ON)));
        lay(&mut s, &ground, &[]);
        s.digest()
    };
    assert_eq!(build(21), build(21));
    assert_ne!(build(21), build(22));
}

#[test]
fn a_fleet_moves_as_one_in_a_template_that_stays() {
    let grid = Grid::uniform(128, ground::WATER_CLASS, 0);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let ships = || {
        vec![
            unit(t(0), "KIEV.NT", class::SHIP, [MID, MID], ENEMY_SIDE),
            unit(
                t(1),
                "KRIVAK.NT",
                class::SHIP,
                [MID + 3_000, MID],
                ENEMY_SIDE,
            ),
            unit(
                t(2),
                "KRIVAK.NT",
                class::SHIP,
                [MID - 3_000, MID],
                ENEMY_SIDE,
            ),
        ]
    };
    let mut s = surface(ships(), Some(target(6, JITTER_ONLY)));
    lay(&mut s, &ground, &[]);
    // Each ship within the fleet's shift and turn plus its own disc; the
    // escorts keep their 6,000 ft spacing within twice a ship's disc.
    let limit = FLEET_SHIFT_FT + 3_000 + SHIP_JITTER_FT;
    for (unit, before) in s.units.iter().zip(ships()) {
        assert!(distance2(at(unit), at(&before)) <= limit.pow(2));
    }
    let spacing = distance2(at(&s.units[1]), at(&s.units[2]));
    let slack = 2 * SHIP_JITTER_FT + 2;
    assert!(
        in_band(spacing, [6_000 - slack, 6_000 + slack]),
        "{spacing}"
    );
    // The flagship moved off its retail spot.
    assert_ne!(at(&s.units[0]), [i64::from(MID), i64::from(MID)]);
}

#[test]
fn a_routed_unit_neither_jitters_nor_relocates() {
    // Its legs are authored in the template's frame, so the unit and its
    // template stay exactly where retail put them (movement follows them).
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut units = site_units();
    units[7].route = Some(tore_formats::quick_template::Route {
        alias: 7,
        waypoints: Vec::new(),
    });
    for seed in 0..10 {
        let mut s = surface(units.clone(), Some(target(seed, Variation::ON)));
        lay(&mut s, &ground, &[]);
        assert_eq!(s.template.as_ref().unwrap().anchor, Some(Anchor::Route));
        assert!(s.transform.is_identity());
        let tank = s.unit(t(7)).unwrap();
        assert_eq!(
            (tank.position, tank.angles),
            (units[7].position, units[7].angles)
        );
    }
}

#[test]
fn a_carrier_with_aircraft_on_its_deck_keeps_its_spot() {
    // Its deck aircraft are measured from where the template puts it
    // (`parked::deck_offset`), so it carries them by not moving.
    let grid = Grid::uniform(128, ground::WATER_CLASS, 0);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let fleet = || {
        vec![
            unit(t(0), "CLEM.NT", class::SHIP, [MID, MID], ENEMY_SIDE),
            unit(
                t(1),
                "KRIVAK.NT",
                class::SHIP,
                [MID + 3_000, MID],
                ENEMY_SIDE,
            ),
        ]
    };
    for seed in 0..6 {
        let mut s = surface(fleet(), Some(target(seed, JITTER_ONLY)));
        s.parked.push(ParkedAircraft {
            id: t(2),
            resource: "SPE.PT".into(),
            position: [MID + 40, 0, MID + 200],
            angles: [0; 3],
            nationality: Some(0x83),
            side: ENEMY_SIDE,
            target: false,
            deck: Some(t(0)),
        });
        lay(&mut s, &ground, &[]);
        let carrier = s.unit(t(0)).unwrap();
        assert_eq!(
            (carrier.position, carrier.angles),
            (fleet()[0].position, fleet()[0].angles),
            "seed {seed}"
        );
        assert_eq!(s.parked[0].position, [MID + 40, 0, MID + 200]);
    }
}

/// The front line's units, ascending id.
fn front_units(surface: &Surface) -> Vec<&Unit> {
    surface
        .units
        .iter()
        .filter(|u| u.id.range() == IdRange::FrontLine)
        .collect()
}

fn built_front(seed: u32, aaa: usize, grid: &Grid, missing: &[&str]) -> Surface {
    let ground = Ground::new(grid, Vec::new(), Vec::new(), None);
    let mut settings = target(seed, Variation::ON);
    settings.aaa = aaa;
    let mut s = surface(site_units(), Some(settings));
    lay(&mut s, &ground, missing);
    s
}

#[test]
fn a_front_line_stands_in_contact_clear_of_the_targets_and_the_starts() {
    let grid = Grid::land(128);
    for seed in 1..=12 {
        let s = built_front(seed, 3, &grid, &[]);
        let front = front_units(&s);
        // Blue's four then Red's four.
        assert_eq!(front.len(), 8, "seed {seed}");
        let blue: Vec<_> = front.iter().filter(|u| u.side == FRIENDLY_SIDE).collect();
        let red: Vec<_> = front.iter().filter(|u| u.side == ENEMY_SIDE).collect();
        assert_eq!((blue.len(), red.len()), (4, 4), "seed {seed}");
        assert!(front.iter().all(|u| !u.is_target() && u.route.is_none()));
        assert!(front.iter().all(|u| s.side_of(u.id.0) == u.side));
        // Red's carry the enemy nationality and the Redfor bit.
        assert!(red.iter().all(|u| u.nationality == Some(10 | 0x80)));
        assert!(blue.iter().all(|u| u.nationality == Some(0)));
        // The two lines face each other inside a tank gun's reach, and no
        // pair is closer than a unit's length.
        let nearest = blue
            .iter()
            .flat_map(|b| red.iter().map(move |r| distance2(at(b), at(r))))
            .min()
            .unwrap();
        assert!(
            (1_000i64.pow(2)..=3_200i64.pow(2)).contains(&nearest),
            "seed {seed}: {}",
            (nearest as f64).sqrt()
        );
        // Clear of every template unit and both starts.
        let clear = (FRONT_CLEAR_NM * NM_FT).pow(2);
        let starts = s.starts.as_ref().unwrap();
        let start = |p: [i32; 2]| [i64::from(p[0]), i64::from(p[1])];
        for u in &front {
            assert!(
                s.template_units().all(|t| distance2(at(u), at(t)) >= clear),
                "seed {seed}: {:?} is near a template unit",
                u.id
            );
            assert!(distance2(at(u), start(starts.red)) >= clear);
            assert!(distance2(at(u), start(starts.blue)) >= clear);
        }
        // Each side faces the other: a unit's heading is the line's axis
        // toward the other line, within the turn (the lines' middles differ
        // from the axis by the jitter, a few degrees).
        let middle = |units: &[&&Unit]| centroid(units.iter().map(|u| at(u))).expect("units");
        let (b, r) = (middle(&blue), middle(&red));
        let toward =
            |from: [i64; 2], to: [i64; 2]| trig::bearing([to[0] - from[0], to[1] - from[1]]);
        for u in &blue {
            let off = trig::signed(u.angles[0] - toward(b, r)).abs();
            assert!(
                off <= FRONT_TURN_DEG + 8,
                "seed {seed}: blue heading off by {off}"
            );
        }
        for u in &red {
            let off = trig::signed(u.angles[0] - toward(r, b)).abs();
            assert!(
                off <= FRONT_TURN_DEG + 8,
                "seed {seed}: red heading off by {off}"
            );
        }
    }
}

/// A route around the targets at `radius` feet: from the unit's spot, then
/// sixteen legs round a ring.
fn ring_route(from: [i32; 3], radius: f64) -> tore_formats::quick_template::Route {
    use tore_formats::quick_template::{Route, WAYPOINT_LEG, WAYPOINT_START, Waypoint};
    let point = |index: u32, flags: i32, position: [i32; 3]| Waypoint {
        index,
        flags,
        goal: 0,
        next: 0,
        head: [0, 0],
        position,
        speed: 20,
        wing: [0; 4],
        react: [0; 3],
        search_dist: 0,
        preferred_target: 0,
        name: String::new(),
    };
    let mut waypoints = vec![point(0, WAYPOINT_START, from)];
    for n in 0..=16u32 {
        let angle = f64::from(n) * std::f64::consts::TAU / 16.;
        let at = [
            MID + (radius * angle.sin()).round() as i32,
            0,
            MID + (radius * angle.cos()).round() as i32,
        ];
        waypoints.push(point(n + 1, WAYPOINT_LEG, at));
    }
    Route {
        alias: 7,
        waypoints,
    }
}

/// Review M3 and L1: the front keeps clear of every leg of a routed unit's
/// path, not only its start, and of every runway a ground start may use.
/// A route round the targets at 7.5 nm rules out the first band (5 to 10
/// nm), so the front stands 10 to 20 nm out, where the ground-start fields
/// are; one is then put where the front stood.
#[test]
fn a_front_line_keeps_clear_of_routes_and_ground_start_runways() {
    let grid = Grid::land(128);
    let mut units = site_units();
    units[7].route = Some(ring_route(units[7].position, 7.5 * NM_FT as f64));
    let path: Vec<[i64; 2]> = std::iter::once(at(&units[7]))
        .chain(
            units[7]
                .route
                .as_ref()
                .unwrap()
                .legs()
                .map(|leg| xz(leg.position)),
        )
        .collect();
    // The lines spread about the centre the rule keeps clear.
    let near = FRONT_CLEAR_NM * NM_FT - 3_000;
    let build = |seed: u32, runways: Vec<Placed>| {
        let ground = Ground::new(&grid, Vec::new(), runways, None);
        let mut s = surface(units.clone(), Some(target(seed, Variation::ON)));
        lay(&mut s, &ground, &[]);
        s
    };
    let (mut fronts, mut fields) = (0, 0);
    for seed in 1..=24 {
        let s = build(seed, Vec::new());
        let front = front_units(&s);
        if front.is_empty() {
            continue;
        }
        fronts += 1;
        for u in &front {
            assert!(
                !path
                    .windows(2)
                    .any(|leg| front::near_leg(at(u), leg[0], leg[1], near)),
                "seed {seed}: {:?} is near the route",
                u.id
            );
        }
        let middle = centroid(front.iter().map(|u| at(u))).unwrap();
        let starts = s.starts.as_ref().unwrap();
        let target = [i64::from(starts.target[0]), i64::from(starts.target[1])];
        if distance2(middle, target) < (16 * NM_FT).pow(2) {
            continue;
        }
        let runway = Placed {
            id: l(9).0,
            at: middle,
            heading: 0,
            footprint: types("STRIP.OT").footprint,
        };
        let again = build(seed, vec![runway]);
        let starts = again.starts.as_ref().unwrap();
        assert!(
            starts.blue_airfields.contains(&runway.id) || starts.red_airfields.contains(&runway.id),
            "seed {seed}: the field is a ground start's"
        );
        fields += 1;
        for u in front_units(&again) {
            assert!(
                !runway.within(at(u), near),
                "seed {seed}: {:?} is near the field",
                u.id
            );
        }
    }
    assert!(
        fronts >= 3 && fields >= 1,
        "{fronts} fronts, {fields} fields"
    );
}

#[test]
fn the_front_line_is_seeded_and_turns_off() {
    let grid = Grid::land(128);
    let a = built_front(7, 3, &grid, &[]);
    let b = built_front(7, 3, &grid, &[]);
    let c = built_front(8, 3, &grid, &[]);
    let places = |s: &Surface| {
        front_units(s)
            .iter()
            .map(|u| (u.id, u.resource.clone(), u.position, u.angles))
            .collect::<Vec<_>>()
    };
    assert_eq!(places(&a), places(&b));
    assert_eq!(a.digest(), b.digest());
    assert_ne!(places(&a), places(&c));
    // The placements follow, so the scene draws them.
    assert!(front_units(&a).iter().all(|u| {
        a.placements
            .iter()
            .any(|(id, p)| *id == u.id && p.position == u.position)
    }));
    // Off: no front, and nothing else about the template moves with it.
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut off = target(7, Variation::ON);
    off.variation.front = false;
    let mut s = surface(site_units(), Some(off));
    lay(&mut s, &ground, &[]);
    assert!(front_units(&s).is_empty());
    assert_eq!(
        s.template_units()
            .map(|u| (u.id, u.position))
            .collect::<Vec<_>>(),
        a.template_units()
            .map(|u| (u.id, u.position))
            .collect::<Vec<_>>()
    );
    assert_ne!(s.digest(), a.digest());
}

#[test]
fn a_front_line_has_a_gun_only_with_aaa_on_and_none_without_the_equipment_or_land() {
    let grid = Grid::land(128);
    assert_eq!(front_units(&built_front(3, 0, &grid, &[])).len(), 6);
    assert_eq!(front_units(&built_front(3, 1, &grid, &[])).len(), 8);
    // A side the import cannot equip: no front, and the layout says so.
    let red_kit = [
        "BMP2.NT", "BTR80.NT", "T72.NT", "T80.NT", "T90.NT", "ZSU23.NT", "ZSU57.NT",
    ];
    let s = built_front(3, 3, &grid, &red_kit);
    assert!(front_units(&s).is_empty());
    assert!(
        s.layout_notes.iter().any(|n| n.contains("no front line")),
        "{:?}",
        s.layout_notes
    );
    // All sea: no site, no front.
    let sea = Grid::uniform(128, ground::WATER_CLASS, 0);
    let s = built_front(3, 3, &sea, &[]);
    assert!(front_units(&s).is_empty());
    assert!(
        s.layout_notes.iter().any(|n| n.contains("no front line")),
        "{:?}",
        s.layout_notes
    );
}

#[test]
fn a_ridge_between_two_units_blocks_their_sight() {
    let mut grid = Grid::land(128);
    // A ridge along cell column 64: 768 ft above the plain at its crest.
    for row in 0..128 {
        grid.cells[row * 128 + 64].elevation = 4;
    }
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let (x, z) = (64 * 8192, MID.into());
    assert!(!ground.sees([x - 3_000, z], [x + 3_000, z], 10));
    // Both on one slope, or both on the plain, they see each other.
    assert!(ground.sees([x - 6_000, z], [x - 3_000, z], 10));
    assert!(ground.sees([x + 4_000, z], [x + 6_000, z], 10));
}

#[test]
fn a_front_line_keeps_its_lines_in_sight_of_each_other() {
    // Hills of 256 ft on a checker of cells: a site may stand on one cell's
    // slope, but every unit must see the other line.
    let mut grid = Grid::land(128);
    for row in 0..128 {
        for col in 0..128 {
            if (col / 2 + row / 2) % 2 == 0 {
                grid.cells[row * 128 + col].elevation = 2;
            }
        }
    }
    for seed in 1..=12 {
        let s = built_front(seed, 3, &grid, &[]);
        let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
        let front = front_units(&s);
        for a in front.iter().filter(|u| u.side == FRIENDLY_SIDE) {
            for b in front.iter().filter(|u| u.side == ENEMY_SIDE) {
                assert!(ground.sees(at(a), at(b), FRONT_EYE_FT), "seed {seed}");
            }
        }
    }
}

// Area defenses (B23): a mission with no ground target and a strength above
// none scatters the enemy's SAM sites and guns over Red's side.

/// Blue at the south edge, Red at the north, the midpoint at the middle of
/// the 128-cell grid; Red's side runs north from `MID`.
fn area_front() -> Front {
    Front {
        blue: [i64::from(MID), 8 * 8192],
        red: [i64::from(MID), 120 * 8192],
    }
}

fn area(aaa: usize, sam: usize, seed: u32) -> crate::surface::resolve::AreaDefense {
    crate::surface::resolve::AreaDefense {
        aaa,
        sam,
        seed,
        enemy_nationality: 10,
        night_stealth: false,
        rolled: true,
    }
}

/// The first build's fixed counts (a recording made then).
fn legacy_area(aaa: usize, sam: usize, seed: u32) -> crate::surface::resolve::AreaDefense {
    crate::surface::resolve::AreaDefense {
        rolled: false,
        ..area(aaa, sam, seed)
    }
}

/// A Red runway north of the band's middle, as layout object 900.
fn red_runway() -> Placed {
    Placed {
        id: l(900).0,
        at: [i64::from(MID) + 200_000, i64::from(MID) + 400_000],
        heading: 0,
        footprint: Footprint {
            min: [-200, 0],
            max: [200, 6_000],
        },
    }
}

fn area_surface(settings: crate::surface::resolve::AreaDefense) -> Surface {
    let mut s = surface(Vec::new(), None);
    s.object_sides.insert(l(900).0, ENEMY_SIDE);
    s.area = Some(settings);
    s
}

fn area_units(s: &Surface) -> Vec<&Unit> {
    area::units(s).collect()
}

/// Feet past the midpoint toward Red (north).
fn north_of_mid(at: [i64; 2]) -> i64 {
    at[1] - i64::from(MID)
}

#[test]
fn area_defenses_are_seeded_and_the_same_for_the_same_seed() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), vec![red_runway()], Some(area_front()));
    let build = |seed| {
        let mut s = area_surface(area(3, 3, seed));
        lay(&mut s, &ground, &[]);
        s
    };
    let (a, b, c) = (build(11), build(11), build(12));
    assert!(!area_units(&a).is_empty());
    assert_eq!(positions(&a), positions(&b));
    assert_eq!(a.digest(), b.digest());
    assert_ne!(positions(&a), positions(&c));
    assert_ne!(a.digest(), c.digest());
    // The settings are in the digest even where they place the same units.
    let mut d = build(11);
    d.area.as_mut().unwrap().night_stealth = true;
    assert_ne!(a.digest(), d.digest());
}

const SAM_TYPES: [&str; 7] = [
    "SA6.NT", "SA7.NT", "SA9.NT", "SA13.NT", "SA14.NT", "SA15.NT", "2S6.NT",
];

#[test]
fn area_counts_are_rolled_like_the_template_slots() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), vec![red_runway()], Some(area_front()));
    for (aaa, sam) in [(0, 0), (1, 2), (2, 1), (3, 3), (0, 3), (3, 0)] {
        for seed in 1..=4 {
            let settings = area(aaa, sam, seed);
            let (sam_sites, aaa_sites) = area::site_counts(&settings, "TEST.MM");
            // Ten slots of a kind, none at "not", all at "heavy".
            assert!(sam_sites <= AREA_SLOTS && aaa_sites <= AREA_SLOTS);
            if sam == 0 {
                assert_eq!(sam_sites, 0);
            }
            if aaa == 3 {
                assert_eq!(aaa_sites, AREA_SLOTS);
            }
            if sam == 3 {
                assert_eq!(sam_sites, AREA_SLOTS);
            }
            let mut s = area_surface(settings);
            lay(&mut s, &ground, &[]);
            let units = area_units(&s);
            let count = |c: &[&str]| {
                units
                    .iter()
                    .filter(|u| c.contains(&u.resource.as_str()))
                    .count()
            };
            let sams = count(&SAM_TYPES);
            let guns = count(&["ZSU23.NT", "ZSU57.NT"]);
            assert_eq!(sams, sam_sites * AREA_SITE_UNITS, "{aaa} {sam} {seed}");
            assert_eq!(guns, aaa_sites * AREA_SITE_UNITS, "{aaa} {sam} {seed}");
            assert_eq!(units.len(), sams + guns);
            for unit in &units {
                assert_eq!(unit.id.range(), IdRange::AreaDefense);
                assert_eq!((unit.side, unit.nationality), (ENEMY_SIDE, Some(10 | 0x80)));
                assert!(!unit.is_target() && unit.route.is_none());
                assert_eq!(s.side_of(unit.id.0), ENEMY_SIDE);
            }
            // No front line and no starts: just the sites (and the SAM
            // sites' trucks).
            assert!(s.starts.is_none());
        }
    }
}

#[test]
fn rolled_area_counts_follow_the_template_chances_per_strength() {
    // Over many seeds each slot is manned 25, 60 or 100 percent of the
    // time (the template rule), so the mean site count of a kind is 2.5,
    // 6 and 10 of ten, and the range of any one mission is 0 to 10.
    let mean = |level: usize| {
        let (mut sam, mut aaa, mut low, mut high) = (0usize, 0usize, usize::MAX, 0usize);
        for seed in 1..=400u32 {
            let (a, b) = area::site_counts(&area(level, level, seed), "TEST.MM");
            sam += a;
            aaa += b;
            low = low.min(a).min(b);
            high = high.max(a).max(b);
        }
        (sam as f64 / 400., aaa as f64 / 400., low, high)
    };
    for (level, expect) in [(1, 2.5), (2, 6.0)] {
        let (sam, aaa, low, high) = mean(level);
        assert!((sam - expect).abs() < 0.4, "level {level} SAM mean {sam}");
        assert!((aaa - expect).abs() < 0.4, "level {level} AAA mean {aaa}");
        assert!(
            low <= 1 && high >= if level == 1 { 5 } else { 9 },
            "{level}"
        );
        assert!(high <= AREA_SLOTS);
    }
    assert_eq!(mean(3), (10.0, 10.0, 10, 10));
    assert_eq!(mean(0), (0.0, 0.0, 0, 0));
    // Sam and gun slots roll apart: some seed differs between the kinds.
    assert!((1..=50u32).any(|seed| {
        let (a, b) = area::site_counts(&area(2, 2, seed), "TEST.MM");
        a != b
    }));
    // The layout is part of the stream: another layout, another rolls.
    assert!(
        (1..=50u32).any(|seed| area::site_counts(&area(2, 2, seed), "TEST.MM")
            != area::site_counts(&area(2, 2, seed), "OTHER.MM"))
    );
}

#[test]
fn a_slot_manned_at_a_lower_strength_is_manned_at_a_higher_one() {
    // Each slot rolls apart against the strength's percent, so the same
    // seed at a higher strength keeps every site and may add more.
    for seed in 1..=40u32 {
        let counts = |level| area::site_counts(&area(level, level, seed), "TEST.MM");
        let (l, m, h) = (counts(1), counts(2), counts(3));
        assert!(
            l.0 <= m.0 && m.0 <= h.0 && l.1 <= m.1 && m.1 <= h.1,
            "{seed}"
        );
    }
}

#[test]
fn the_first_builds_fixed_counts_stay_readable() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), vec![red_runway()], Some(area_front()));
    for (level, (sam_sites, aaa_sites)) in [(1, (2, 4)), (2, (4, 8)), (3, (6, 12))] {
        let mut s = area_surface(legacy_area(level, level, 5));
        lay(&mut s, &ground, &[]);
        let units = area_units(&s);
        let sams = units
            .iter()
            .filter(|u| SAM_TYPES.contains(&u.resource.as_str()))
            .count();
        assert_eq!(sams, sam_sites * AREA_SITE_UNITS, "{level}");
        assert_eq!(units.len() - sams, aaa_sites * AREA_SITE_UNITS, "{level}");
        // The first build had no trucks.
        let rolled_digest = {
            let mut r = area_surface(area(level, level, 5));
            lay(&mut r, &ground, &[]);
            r.digest()
        };
        assert_ne!(s.digest(), rolled_digest);
    }
}

#[test]
fn area_sam_units_get_supply_trucks_that_reload_and_guns_get_none() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), vec![red_runway()], Some(area_front()));
    for seed in 1..=6u32 {
        let mut s = area_surface(area(3, 3, seed));
        lay(&mut s, &ground, &[]);
        let launchers: Vec<UnitId> = area_units(&s)
            .iter()
            .filter(|u| SAM_TYPES.contains(&u.resource.as_str()))
            .map(|u| u.id)
            .collect();
        assert!(!launchers.is_empty());
        let per_unit: Vec<_> = s.trucks.iter().filter_map(|t| t.serves).collect();
        for id in &launchers {
            let truck = s
                .trucks
                .iter()
                .find(|t| t.serves == Some(*id) && t.added)
                .unwrap_or_else(|| panic!("seed {seed}: {id:?} has no truck"));
            let (tu, lu) = (s.unit(truck.id).unwrap(), s.unit(*id).unwrap());
            assert_eq!(tu.resource, "MISTRK.NT");
            assert_eq!((tu.side, tu.nationality), (lu.side, lu.nationality));
            assert_eq!(tu.id.range(), IdRange::SupplyTruck);
            let apart = distance2(at(tu), at(lu)) as f64;
            let [lo, hi] = TRUCK_FT;
            assert!(apart >= (lo * lo) as f64 && apart <= ((hi * hi) as f64) * 1.0001);
            assert!(apart.sqrt() <= crate::surface::RESUPPLY_RADIUS_FT);
        }
        // No truck for a gun, and every battery has its own.
        assert!(per_unit.iter().all(|id| launchers.contains(id)));
        for battery in &s.batteries {
            assert!(battery.truck.is_some(), "seed {seed}");
        }
        assert!(
            s.trucks.len() >= launchers.len(),
            "seed {seed}: {} trucks for {} launchers",
            s.trucks.len(),
            launchers.len()
        );
        // Placement is seeded: the same seed, the same trucks.
        let mut again = area_surface(area(3, 3, seed));
        lay(&mut again, &ground, &[]);
        assert_eq!(s.digest(), again.digest());
        assert_eq!(s.trucks, again.trucks);
        assert!(area::audit(&s, &ground, &mut types).is_empty());
    }
}

#[test]
fn area_sites_stand_in_the_band_or_by_a_red_airfield() {
    let grid = Grid::land(128);
    let runway = red_runway();
    let ground = Ground::new(&grid, Vec::new(), vec![runway], Some(area_front()));
    let [near, far] = AREA_DEPTH_NM.map(|nm| nm * NM_FT);
    let slack = AREA_MEMBER_FT[1] + 1;
    for seed in 1..=8 {
        let mut s = area_surface(area(3, 3, seed));
        lay(&mut s, &ground, &[]);
        let mut by_field = 0;
        for unit in area_units(&s) {
            let p = at(unit);
            let depth = north_of_mid(p);
            assert!(depth > 0, "seed {seed}: {p:?} is on Blue's side");
            let in_band = depth >= near - slack && depth <= far + slack;
            let d = distance2(p, runway.at);
            let by_airfield = in_band_of(d, AREA_AIRFIELD_NM.map(|nm| nm * NM_FT), slack);
            assert!(in_band || by_airfield, "seed {seed}: {p:?} depth {depth}");
            by_field += usize::from(by_airfield && !in_band);
            assert!(!runway.within(p, AREA_RUNWAY_CLEARANCE_FT));
        }
        // One site in three of each kind (the first included) guards the
        // airfield: ceil(sites / 3) of each, two units each.
        let (sam_sites, aaa_sites) = area::site_counts(&area(3, 3, seed), "TEST.MM");
        let guarded = |n: usize| n.div_ceil(AREA_AIRFIELD_EVERY);
        // With one airfield in the theater and up to eight sites asking for
        // it (3 nm apart in a 2 to 5 nm ring), a late one may find the ring
        // full and stand in the band instead (the fallback), so the count
        // is at most that and at least half of it.
        let want = (guarded(sam_sites) + guarded(aaa_sites)) * AREA_SITE_UNITS;
        assert!(
            by_field <= want && by_field * 2 >= want,
            "seed {seed}: {by_field} of {want}"
        );
        assert_eq!(by_field % AREA_SITE_UNITS, 0, "seed {seed}");
    }
}

fn in_band_of(d2: i64, [lo, hi]: [i64; 2], slack: i64) -> bool {
    d2 >= (lo - slack).max(0).pow(2) && d2 <= (hi + slack).pow(2)
}

#[test]
fn area_units_keep_off_water_slopes_towns_and_runways() {
    // A sea across the band's lower half, a hill to the east, a town.
    let mut grid = Grid::land(128);
    for col in 0..128 {
        for row in 66..80 {
            grid.set(col, row, ground::WATER_CLASS);
        }
    }
    for row in 0..128 {
        for col in 90..100 {
            let at = row * 128 + col;
            grid.cells[at].elevation = if col % 2 == 0 { 1 } else { 9 };
        }
    }
    let town = Placed {
        id: l(5).0,
        at: [i64::from(MID) - 100_000, i64::from(MID) + 200_000],
        heading: 0,
        footprint: Footprint {
            min: [-3_000, -3_000],
            max: [3_000, 3_000],
        },
    };
    let ground = Ground::new(&grid, vec![town], vec![red_runway()], Some(area_front()));
    for seed in 1..=6 {
        let mut s = area_surface(area(3, 3, seed));
        lay(&mut s, &ground, &[]);
        assert!(!area_units(&s).is_empty());
        for unit in area_units(&s) {
            let p = at(unit);
            assert!(!ground.water(p), "seed {seed}: {p:?} on water");
            assert!(
                ground.level(p, LEVEL_UNITS),
                "seed {seed}: {p:?} on a slope"
            );
            assert!(!town.within(p, AREA_OBJECT_CLEARANCE_FT), "{p:?} in town");
            assert!(!red_runway().within(p, AREA_RUNWAY_CLEARANCE_FT));
        }
    }
}

#[test]
fn an_area_sa6_site_gets_its_radar_and_its_own_battery() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), Some(area_front()));
    // Only the SA-6 can be placed, so every SAM site is one.
    let missing = [
        "SA7.NT", "SA9.NT", "SA13.NT", "SA14.NT", "SA15.NT", "2S6.NT",
    ];
    let mut s = area_surface(area(0, 3, 3));
    lay(&mut s, &ground, &missing);
    let launchers: Vec<UnitId> = area_units(&s)
        .iter()
        .filter(|u| u.resource == "SA6.NT")
        .map(|u| u.id)
        .collect();
    assert_eq!(launchers.len(), AREA_SLOTS * AREA_SITE_UNITS);
    assert_eq!(s.batteries.len(), AREA_SLOTS);
    for battery in &s.batteries {
        assert_eq!(battery.system, BatterySystem::Sa6);
        // The radar is added and the battery has its own supply truck.
        assert!(battery.radar_added && battery.truck.is_some());
        assert_eq!(battery.launchers.len(), 2);
        assert!(battery.launchers.iter().all(|id| launchers.contains(id)));
        let radar = s.unit(battery.radar).unwrap();
        assert_eq!(radar.side, ENEMY_SIDE);
    }
}

#[test]
fn no_area_defenses_without_a_front_with_a_target_or_at_none() {
    let grid = Grid::land(128);
    let no_front = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut s = area_surface(area(3, 3, 1));
    lay(&mut s, &no_front, &[]);
    assert!(area_units(&s).is_empty());
    assert!(s.layout_notes.iter().any(|n| n.contains("no front")));
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), Some(area_front()));
    let mut none = area_surface(area(0, 0, 1));
    lay(&mut none, &ground, &[]);
    assert!(area_units(&none).is_empty());
    // With a ground target the defenses are its own: never both.
    let mut both = surface(site_units(), Some(target(1, JITTER_ONLY)));
    both.area = Some(area(3, 3, 1));
    lay(&mut both, &ground, &[]);
    assert!(area_units(&both).is_empty());
}

#[test]
fn exhausted_support_search_is_explicit_without_removing_equipment() {
    // Authored launchers may leave no valid support spot in the allowed ring.
    // Keep the existing requested equipment, but expose every unresolved spot.
    let grid = Grid::uniform(128, ground::WATER_CLASS, 0);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut s = surface(
        vec![unit(t(0), "SA6.NT", class::SAM, [MID, MID], ENEMY_SIDE)],
        Some(target(1, Variation::OFF)),
    );
    lay(&mut s, &ground, &[]);
    assert_eq!(s.batteries.len(), 1);
    assert_eq!(s.trucks.len(), 2);
    assert_eq!(s.units.len(), 4);
    assert_eq!(
        s.layout_notes
            .iter()
            .filter(|n| n.starts_with("support fallback "))
            .count(),
        3
    );
}

#[test]
fn a_ship_footprint_cannot_extend_beyond_the_map_into_implicit_water() {
    let grid = Grid::uniform(8, ground::WATER_CLASS, 0);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let obstacles = Obstacles {
        fixed: &[],
        strips: &[],
    };
    assert!(!stands(&ground, &obstacles, [10, 8192], 50, true));
    assert!(stands(&ground, &obstacles, [100, 8192], 50, true));
}

#[test]
fn support_units_do_not_overlap_other_mobile_units_or_each_other() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    for seed in 1..=20 {
        let mut units = vec![unit(t(0), "SA6.NT", class::SAM, [MID, MID], ENEMY_SIDE)];
        // A crowded target, including a unit that owns no supply truck.
        for ordinal in 1..=5 {
            units.push(unit(
                t(ordinal),
                "T72.NT",
                class::VEHICLE,
                [MID + 200 * ordinal as i32 - 500, MID + 220],
                ENEMY_SIDE,
            ));
        }
        let mut s = surface(units, Some(target(seed, Variation::OFF)));
        lay(&mut s, &ground, &[]);
        assert!(!s.trucks.is_empty());
        for added in s
            .units
            .iter()
            .filter(|u| matches!(u.id.range(), IdRange::SupplyTruck | IdRange::BatteryRadar))
        {
            for other in s.units.iter().filter(|u| u.id != added.id) {
                let clearance = types(&added.resource).footprint.radius()
                    + types(&other.resource).footprint.radius();
                assert!(
                    distance2(xz(added.position), xz(other.position)) >= clearance.pow(2),
                    "seed {seed}: {:?} overlaps {:?}",
                    added.id,
                    other.id
                );
            }
        }
    }
}

#[test]
fn jitter_reserves_neighbours_fallbacks_and_accepted_spots() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    for seed in 1..=20 {
        let units = (0..20)
            .map(|ordinal| {
                unit(
                    t(ordinal),
                    "T72.NT",
                    class::VEHICLE,
                    [
                        MID + (ordinal % 5) as i32 * 200,
                        MID + (ordinal / 5) as i32 * 200,
                    ],
                    ENEMY_SIDE,
                )
            })
            .collect();
        let mut s = surface(units, Some(target(seed, JITTER_ONLY)));
        lay(&mut s, &ground, &[]);
        let units: Vec<_> = s.template_units().collect();
        for (index, a) in units.iter().enumerate() {
            for b in &units[index + 1..] {
                let clearance =
                    types(&a.resource).footprint.radius() + types(&b.resource).footprint.radius();
                assert!(
                    distance2(xz(a.position), xz(b.position)) >= clearance.pow(2),
                    "seed {seed}: {:?} overlaps {:?}",
                    a.id,
                    b.id
                );
            }
        }
    }
}

#[test]
fn starts_follow_targets_after_the_airport_moves_them() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut target_unit = unit(t(0), "HANGAR.OT", class::STRUCTURE, [MID, MID], ENEMY_SIDE);
    target_unit.flags |= 0x80;
    let mut s = surface(vec![target_unit], Some(target(3, Variation::OFF)));
    lay(&mut s, &ground, &[]);
    let before = s.starts.clone().unwrap();
    s.units[0].position[0] += 8_000;
    let scene = tore_sim::airport::Scene::default();
    let after = scene_starts(&s, &ground, &scene).unwrap();
    assert_eq!(after.target, [MID + 8_000, MID]);
    assert_ne!(after.blue, before.blue);
    assert_ne!(after.red, before.red);
    assert!(
        distance2(after.red.map(i64::from), after.target.map(i64::from))
            <= (RED_START_NM * NM_FT).pow(2)
    );
    let distance = (distance2(after.blue.map(i64::from), after.red.map(i64::from)) as f64).sqrt();
    assert!((distance - 20. * NM_FT as f64).abs() <= 2.);
    assert_eq!(Some(after), scene_starts(&s, &ground, &scene));
}

#[test]
fn automatic_airfields_rank_integer_origins_and_canonicalize_ids() {
    use tore_sim::airport::{OrientedBox, Runway, Scene};
    let grid = Grid::land(128);
    let mut target_unit = unit(t(0), "HANGAR.OT", class::STRUCTURE, [MID, MID], ENEMY_SIDE);
    target_unit.flags |= 0x80;
    let mut s = surface(vec![target_unit], Some(target(3, Variation::OFF)));
    let runway = |id, distance| {
        let center = [f64::from(MID), 0., f64::from(MID) + distance];
        Runway {
            object: id,
            airport: id,
            name: "synthetic".into(),
            surface: OrientedBox {
                center,
                half: [100., 1., 5_000.],
                heading: 0.,
                pitch: 0.,
                bank: 0.,
            },
            approach_center: center,
            elevation_ft: 0.,
            heading: 0.,
            length_ft: 10_000.,
        }
    };
    s.object_sides.insert(l(0).0, FRIENDLY_SIDE);
    s.object_sides.insert(l(1).0, FRIENDLY_SIDE);
    let scene = Scene {
        runways: vec![
            runway(l(0).0, 25. * NM_FT as f64),
            runway(l(1).0, 16. * NM_FT as f64),
        ],
        ..Scene::default()
    };
    let ground = Ground::new(
        &grid,
        vec![],
        vec![
            Placed {
                id: l(0).0,
                at: [i64::from(MID), i64::from(MID) + 25 * NM_FT],
                heading: 0,
                footprint: Footprint::default(),
            },
            Placed {
                id: l(1).0,
                at: [i64::from(MID), i64::from(MID) + 16 * NM_FT],
                heading: 0,
                footprint: Footprint::default(),
            },
        ],
        None,
    );
    let starts = scene_starts(&s, &ground, &scene).unwrap();
    assert_eq!(starts.blue_airfields, vec![l(1).0, l(0).0]);
    assert!(starts.red_airfields.is_empty());
}

#[test]
fn a_carrier_and_its_deck_aircraft_relocate_together_on_water() {
    let grid = Grid::uniform(128, ground::WATER_CLASS, 0);
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut carrier = unit(t(0), "CLEM.NT", class::SHIP, [MID, MID], ENEMY_SIDE);
    carrier.flags |= 0x80;
    let mut s = surface(vec![carrier], Some(target(1, Variation::ON)));
    s.parked.push(ParkedAircraft {
        id: t(1),
        resource: "SPE.PT".into(),
        position: [MID + 40, 0, MID + 200],
        angles: [0; 3],
        nationality: Some(0x83),
        side: ENEMY_SIDE,
        target: false,
        deck: Some(t(0)),
    });
    lay(&mut s, &ground, &[]);
    assert!(
        !s.transform.is_identity(),
        "fixed-in-group must not mean land-only"
    );
    assert!(ground.water(xz(s.unit(t(0)).unwrap().position)));
    assert!(ground.water(xz(s.parked[0].position)));
    let gap =
        (distance2(xz(s.unit(t(0)).unwrap().position), xz(s.parked[0].position)) as f64).sqrt();
    assert!((gap - 40_f64.hypot(200.)).abs() < 2.);
}

#[test]
fn relocation_rejects_land_centers_whose_footprints_cross_water() {
    let mut grid = Grid::land(128);
    for col in (1..128).step_by(2) {
        for row in 0..128 {
            grid.set(col, row, ground::WATER_CLASS);
        }
    }
    let ground = Ground::new(&grid, Vec::new(), Vec::new(), None);
    let mut target = object(0, [i64::from(MID) + 4096, i64::from(MID)], true);
    target.info.footprint = Footprint {
        min: [-3500; 2],
        max: [3500; 2],
    };
    // Every land band is one cell wide; this footprint cannot fit inside it.
    assert!(
        sites::relocate(
            std::slice::from_ref(&target),
            &ground,
            1,
            "QTEST",
            target.retail
        )
        .is_none()
    );
}

#[test]
fn vehicle_grade_uses_its_footprint_and_type_limit() {
    let mut grid = Grid::land(4);
    // 8 elevation units per cell: exactly 25 percent grade.
    for row in 0..4 {
        for col in 0..4 {
            grid.cells[row * 4 + col].elevation = (col * 8) as u8;
        }
    }
    let ground = Ground::new(&grid, vec![], vec![], None);
    let p = Placed {
        id: 1,
        at: [10_000, 10_000],
        heading: 0,
        footprint: Footprint {
            min: [-10, -20],
            max: [10, 20],
        },
    };
    assert!(footing::slope_ok(&ground, p, 30));
    assert!(!footing::slope_ok(&ground, p, 20));
    assert!(footing::slope_ok(&ground, p, 25));
    assert!(!ground.level(p.at, LEVEL_UNITS));
}

#[test]
fn vehicle_repair_chooses_nearest_valid_grid_point_and_retains_equipment() {
    let grid = Grid::land(128);
    let obstruction = Placed {
        id: l(99).0,
        at: [i64::from(MID), i64::from(MID)],
        heading: 0,
        footprint: Footprint {
            min: [-50, -50],
            max: [50, 50],
        },
    };
    let ground = Ground::new(&grid, vec![obstruction], vec![], None);
    let u = unit(
        UnitId::supply_truck(0).unwrap(),
        "TRUCK.NT",
        class::VEHICLE,
        [MID, MID],
        ENEMY_SIDE,
    );
    let mut s = surface(vec![u], Some(target(1, Variation::OFF)));
    footing::repair(&mut s, &ground, &mut types);
    assert_eq!(s.units.len(), 1);
    assert!(footing::violations(&ground, &s.units[0], types("TRUCK.NT")).is_empty());
    let chosen = xz(s.units[0].position);
    let distance = distance2(chosen, [i64::from(MID); 2]);
    for x in -10..=10 {
        for z in -10..=10 {
            let mut candidate = s.units[0].clone();
            candidate.position[0] = MID + x * 25;
            candidate.position[2] = MID + z * 25;
            if distance2(xz(candidate.position), [i64::from(MID); 2]) < distance {
                assert!(!footing::violations(&ground, &candidate, types("TRUCK.NT")).is_empty());
            }
        }
    }
    let water = Grid::uniform(128, ground::WATER_CLASS, 0);
    let sea = Ground::new(&water, vec![], vec![], None);
    footing::repair(&mut s, &sea, &mut types);
    assert_eq!(s.units.len(), 1);
    assert_eq!(xz(s.units[0].position), chosen);
    assert!(
        s.layout_notes
            .iter()
            .any(|n| n.starts_with("placement unresolved "))
    );
}

#[test]
fn an_added_fixed_radar_keeps_an_explicit_failure_when_no_footprint_fits() {
    let mut grid = Grid::land(4);
    for row in 0..4 {
        for col in 0..4 {
            grid.cells[row * 4 + col].elevation = (col * 8) as u8;
        }
    }
    let ground = Ground::new(&grid, vec![], vec![], None);
    let radar = unit(
        UnitId::battery_radar(0).unwrap(),
        "GCI.NT",
        class::STRUCTURE,
        [10_000, 10_000],
        ENEMY_SIDE,
    );
    let mut s = surface(vec![radar], None);
    s.layout_notes
        .push("support fallback 0x5c000000 GCI.NT: initial search exhausted".into());
    footing::repair(&mut s, &ground, &mut types);
    assert_eq!(s.units.len(), 1);
    assert_eq!(xz(s.units[0].position), [10_000, 10_000]);
    assert!(
        s.layout_notes
            .iter()
            .any(|n| n.starts_with("placement unresolved ") && n.contains("footprint-slope"))
    );
    let authored = unit(
        l(0),
        "GCI.NT",
        class::STRUCTURE,
        [10_000, 10_000],
        ENEMY_SIDE,
    );
    assert!(!footing::vehicle(&authored));
}

#[test]
fn the_final_front_avoids_a_vehicle_at_its_previous_candidate() {
    let grid = Grid::land(128);
    let ground = Ground::new(&grid, vec![], vec![], None);
    let variation = Variation {
        jitter: false,
        relocate: false,
        front: true,
    };
    let mut s = surface(
        vec![unit(
            t(0),
            "BUNKER.OT",
            class::STRUCTURE,
            [MID, MID],
            ENEMY_SIDE,
        )],
        Some(target(17, variation)),
    );
    lay(&mut s, &ground, &[]);
    let count = front_units(&s).len();
    assert!(count > 0);
    let at = xz(front_units(&s)[0].position);
    let base = unit(
        l(99),
        "T72.NT",
        class::TANK,
        at.map(|v| v as i32),
        ENEMY_SIDE,
    );
    let obstacle = Placed {
        id: base.id.0,
        at,
        heading: base.angles[0],
        footprint: types(&base.resource).footprint,
    };
    s.units.push(base);
    s.units.sort_by_key(|u| u.id);
    let mut added = |name: &str| Some(entry(name));
    finish_layout(
        &mut s,
        &mut Inputs {
            ground: &ground,
            types: &mut types,
            added: &mut added,
            layout: "TEST.MM",
        },
    );
    assert_eq!(front_units(&s).len(), count);
    for unit in front_units(&s) {
        assert!(!obstacle.within(xz(unit.position), types(&unit.resource).footprint.radius()));
    }
}

#[test]
fn relocating_fixed_structures_keeps_the_coarse_site_limit() {
    let mut grid = Grid::land(128);
    for row in 0..128 {
        for col in 0..128 {
            grid.cells[row * 128 + col].elevation = (col * 2) as u8;
        }
    }
    let ground = Ground::new(&grid, vec![], vec![], None);
    let mut bunker = unit(t(0), "BUNKER.OT", class::STRUCTURE, [MID, MID], ENEMY_SIDE);
    bunker.flags |= 0x80;
    let mut s = surface(
        vec![bunker],
        Some(target(
            14,
            Variation {
                jitter: false,
                relocate: true,
                front: false,
            },
        )),
    );
    lay(&mut s, &ground, &[]);
    assert!(
        s.transform.is_identity(),
        "fixed structures must not relocate onto 512-ft cell spans"
    );
}

#[test]
fn launchers_guns_and_added_radars_accept_twenty_percent_grades() {
    let mut grid = Grid::land(4);
    for row in 0..4 {
        for col in 0..4 {
            grid.cells[row * 4 + col].elevation = (col * 6) as u8;
        }
    }
    let ground = Ground::new(&grid, vec![], vec![], None);
    for (id, resource, class) in [
        (UnitId::template(0).unwrap(), "SA6.NT", class::SAM),
        (UnitId::template(1).unwrap(), "ZSU23.NT", class::AAA),
        (
            UnitId::battery_radar(0).unwrap(),
            "GCI.NT",
            class::STRUCTURE,
        ),
    ] {
        let u = unit(id, resource, class, [10_000, 10_000], ENEMY_SIDE);
        assert_eq!(footing::slope_limit(&u), 20);
        let info = types(resource);
        let p = Placed {
            id: id.0,
            at: [10_000, 10_000],
            heading: 0,
            footprint: info.footprint,
        };
        assert!(!footing::slope_ok(&ground, p, 10));
        assert!(footing::slope_ok(&ground, p, footing::slope_limit(&u)));
    }
}
