//! Resolution, ids, sides and the digest against a synthetic import: every
//! surface type the equipment lists name, generated here, in the synthetic
//! theater. No retail data.
use super::{
    catalog::Catalog,
    resolve::{self, GroundTarget, Purpose, Stream},
    *,
};
use crate::{
    ai_wings::{ENEMY_SIDE, FRIENDLY_SIDE},
    mission::{Condition, MissionSpec},
    terrain::{Overrides, Terrain},
    test_support::resources::THEATER,
    world::{Seating, World},
};
use std::collections::{BTreeMap, BTreeSet};
use tore_formats::{
    aircraft::AircraftId,
    quick_template::{Placeholder, Template, tables},
};

pub(super) use crate::test_support::surface::{MIDDLE, surface_resources, target};

fn resolve_with(
    r: &BTreeMap<String, Vec<u8>>,
    target: &GroundTarget,
) -> resolve::TemplateResolution {
    let name = format!("~{}.M", target.stem);
    let template = Template::parse(&name, &r[&name]).unwrap();
    resolve::template(&template, target, &mut Catalog::new(r), Some("UKR.T2")).unwrap()
}

/// The units that filled a `which` placeholder, by ordinal and type.
fn slots(resolved: &resolve::TemplateResolution, which: Placeholder) -> Vec<(u32, String)> {
    resolved
        .units
        .iter()
        .filter_map(|unit| match &unit.origin {
            Origin::Template {
                ordinal,
                placeholder: Some(p),
            } if *p == which => Some((*ordinal, unit.resource.clone())),
            _ => None,
        })
        .collect()
}

fn defenses(resolved: &resolve::TemplateResolution) -> usize {
    slots(resolved, Placeholder::Sam).len() + slots(resolved, Placeholder::Aaa).len()
}

#[test]
fn defense_rolls_man_slots_at_the_retail_percentages() {
    let r = surface_resources();
    for (level, percent) in [(0, 0.), (1, 25.), (2, 60.), (3, 100.)] {
        let (mut manned, mut seen) = (0usize, 0usize);
        for seed in 0..400 {
            let resolved = resolve_with(&r, &target("QUCITY", level, level, seed));
            manned += defenses(&resolved);
            seen += 12;
            // Every slot is either placed or listed as removed.
            assert_eq!(defenses(&resolved) + resolved.site.removed.len(), 12);
        }
        let share = 100. * manned as f64 / seen as f64;
        if level == 0 || level == 3 {
            assert_eq!(share, percent, "level {level}");
        } else {
            // 4,800 slots: three standard deviations is under 2.2 points.
            assert!((share - percent).abs() < 2.5, "level {level}: {share:.1}%");
        }
    }
}

#[test]
fn sam_and_aaa_levels_roll_separately() {
    let r = surface_resources();
    let resolved = resolve_with(&r, &target("QUCITY", 3, 0, 7));
    assert_eq!(slots(&resolved, Placeholder::Aaa).len(), 6);
    assert!(slots(&resolved, Placeholder::Sam).is_empty());
    let resolved = resolve_with(&r, &target("QUCITY", 0, 3, 7));
    assert!(slots(&resolved, Placeholder::Aaa).is_empty());
    assert_eq!(slots(&resolved, Placeholder::Sam).len(), 6);
}

#[test]
fn picks_come_uniformly_from_the_enemy_groups_lists() {
    let r = surface_resources();
    // Russian, French, Islamic Egyptian, Turkish, American: groups 2, 1, 3,
    // 4 and 0.
    for (nationality, group) in [(10, 2), (3, 1), (14, 3), (41, 4), (0, 0)] {
        let mut seen: BTreeMap<Placeholder, BTreeMap<String, usize>> = BTreeMap::new();
        for seed in 0..300 {
            let mut t = target("QUCITY", 3, 3, seed);
            t.enemy_nationality = nationality;
            let resolved = resolve_with(&r, &t);
            assert_eq!(resolved.site.group, group);
            for which in [Placeholder::Sam, Placeholder::Aaa, Placeholder::Tank] {
                for (_, resource) in slots(&resolved, which) {
                    *seen.entry(which).or_default().entry(resource).or_default() += 1;
                }
            }
        }
        for (which, counts) in &seen {
            let list: BTreeSet<String> = tables::equipment(*which, group)
                .unwrap()
                .iter()
                .map(|s| s.to_string())
                .collect();
            // Every list entry turns up, nothing else does, and roughly as
            // often as the others.
            assert_eq!(
                counts.keys().cloned().collect::<BTreeSet<_>>(),
                list,
                "{which:?} for group {group}"
            );
            let total: usize = counts.values().sum();
            let even = total as f64 / list.len() as f64;
            for (name, n) in counts {
                assert!(
                    (*n as f64 - even).abs() < even * 0.3,
                    "{name}: {n} of {total}"
                );
            }
        }
    }
}

#[test]
fn the_night_rule_mans_every_aaa_slot_with_a_novice_zsu23() {
    let r = surface_resources();
    let mut t = target("QUCITY", 3, 3, 11);
    t.night_stealth = true;
    let resolved = resolve_with(&r, &t);
    let aaa = slots(&resolved, Placeholder::Aaa);
    assert_eq!(aaa.len(), 6);
    for (ordinal, resource) in aaa {
        assert_eq!(resource, "ZSU23.NT");
        let unit = resolved
            .units
            .iter()
            .find(|u| u.id == UnitId::template(ordinal).unwrap())
            .unwrap();
        // The template wrote skill 3; the rule writes novice.
        assert_eq!(unit.skill, 0);
    }
    // SAM slots keep their picks and skills.
    let sams = slots(&resolved, Placeholder::Sam);
    assert!(sams.iter().any(|(_, r)| r != "ZSU23.NT"));
    assert_eq!(resolved.units[0].skill, 2);
    // Dormant today: no F-117 or B-2 is imported, so no spec triggers it.
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.condition = Condition::Night;
    assert!(!resolve::night_stealth(&spec));
}

#[test]
fn a_target_placeholder_that_fails_its_roll_is_no_target() {
    let r = surface_resources();
    let targets = |resolved: &resolve::TemplateResolution| -> BTreeSet<u32> {
        resolved
            .units
            .iter()
            .filter(|u| u.is_target())
            .map(|u| u.id.0)
            .chain(resolved.parked.iter().filter(|p| p.target).map(|p| p.id.0))
            .collect()
    };
    let none = resolve_with(&r, &target("QUCITY", 0, 0, 3));
    let heavy = resolve_with(&r, &target("QUCITY", 3, 3, 3));
    // The SAM slot target (ordinal 0), the bunker (12) and the parked
    // aircraft (16) at heavy; without SAMs only the last two.
    assert_eq!(
        targets(&heavy),
        BTreeSet::from([
            SURFACE_UNIT_BASE,
            SURFACE_UNIT_BASE + 12,
            SURFACE_UNIT_BASE + 16
        ])
    );
    assert_eq!(
        targets(&none),
        BTreeSet::from([SURFACE_UNIT_BASE + 12, SURFACE_UNIT_BASE + 16])
    );
    assert!(none.site.removed.contains(&0));
}

#[test]
fn ids_follow_template_ordinals_whatever_the_rolls() {
    let r = surface_resources();
    let heavy = resolve_with(&r, &target("QUCITY", 3, 3, 21));
    let heavy_ids: BTreeSet<u32> = heavy.units.iter().map(|u| u.id.0).collect();
    for seed in 0..50 {
        for level in 0..4 {
            let resolved = resolve_with(&r, &target("QUCITY", level, level, seed));
            for unit in &resolved.units {
                let Origin::Template { ordinal, .. } = unit.origin else {
                    panic!("a template unit from elsewhere");
                };
                assert_eq!(unit.id, UnitId::template(ordinal).unwrap());
                assert_eq!(unit.id.range(), IdRange::Template);
                assert!(heavy_ids.contains(&unit.id.0));
            }
            // A removed slot leaves its id unused; the named objects keep
            // theirs at every level.
            for ordinal in [12u32, 13, 14, 15] {
                assert!(
                    resolved
                        .units
                        .iter()
                        .any(|u| u.id.0 == SURFACE_UNIT_BASE + ordinal)
                );
            }
        }
    }
    // The same inputs give the same resolution.
    assert_eq!(
        resolve_with(&r, &target("QUCITY", 2, 1, 5)),
        resolve_with(&r, &target("QUCITY", 2, 1, 5))
    );
}

#[test]
fn owners_are_rewritten_except_nationality3() {
    let r = surface_resources();
    let resolved = resolve_with(&r, &target("QUCITY", 3, 3, 1));
    for unit in &resolved.units {
        if unit.id.0 == SURFACE_UNIT_BASE + 15 {
            // `nationality3 39` passes through: friendly Taiwanese.
            assert_eq!(unit.nationality, Some(39));
            assert_eq!(unit.side, FRIENDLY_SIDE);
        } else {
            // `nationality` and `nationality2` become the enemy, Redfor.
            assert_eq!(unit.nationality, Some(10 | 0x80), "{}", unit.resource);
            assert_eq!(unit.side, ENEMY_SIDE);
        }
    }
    assert_eq!(resolved.parked.len(), 1);
    assert_eq!(resolved.parked[0].side, ENEMY_SIDE);
    assert_eq!(resolved.parked[0].resource, "F18.PT");
}

#[test]
fn fleet_aircraft_off_every_deck_stay_out() {
    let r = surface_resources();
    // The fleet's aircraft stands 400 ft from a carrier whose fixture hull
    // is an aircraft shape a few dozen feet across.
    let resolved = resolve_with(&r, &target("QUSFLT", 3, 3, 1));
    assert!(resolved.parked.is_empty());
    assert_eq!(resolved.site.left_out.len(), 1);
    assert_eq!(resolved.site.left_out[0].resource, "F18.PT");
    assert_eq!(resolved.site.left_out[0].ordinal, 2);
    assert_eq!(resolved.site.left_out[0].why, "on no carrier's deck");
    let kiev = &resolved.units[0];
    assert!(kiev.is_target());
    assert_eq!(kiev.look, DestroyedLook::DamagedShape("KIEV_A.SH".into()));
}

#[test]
fn destroyed_looks_follow_the_unit_class() {
    let r = surface_resources();
    let mut catalog = Catalog::new(&r);
    let mut look = |name: &str| catalog.entry(name).unwrap().look.clone();
    assert_eq!(look("SA6.NT"), DestroyedLook::Wreck("DEST.OT".into()));
    assert_eq!(
        look("KRIVAK.NT"),
        DestroyedLook::DamagedShape("KRIVAK_A.SH".into())
    );
    assert_eq!(look("TROOPS.NT"), DestroyedLook::Vanish);
    assert_eq!(look("GCI.NT"), DestroyedLook::Removed);
    assert_eq!(
        look("BNK5.OT"),
        DestroyedLook::DamagedObject("~BNK5.OT".into())
    );
    assert_eq!(look("STORE.OT"), DestroyedLook::Removed);
    let sa6 = catalog.entry("SA6.NT").unwrap();
    assert_eq!((sa6.explosion, sa6.crater), (Some(21), Some(6)));
    assert!(catalog.entry("MISTRK.NT").unwrap().supply_truck);
    assert!(catalog.entry("TRUCK.NT").unwrap().supply_truck);
    assert!(!catalog.entry("TANKER.NT").unwrap().supply_truck);
}

#[test]
fn base_layout_units_keep_their_layout_ids_and_take_their_sides() {
    let r = surface_resources();
    let terrain = Terrain::for_mission(&r, THEATER, Some(0), &Overrides::default()).unwrap();
    let surface = &terrain.surface;
    let ids: Vec<u32> = surface.units.iter().map(|u| u.id.0).collect();
    // The SA-6, the ZSU-23 and the truck: layout ordinals 0, 1 and 3; then
    // the radar the layout adds to the SA-6's battery.
    assert_eq!(
        ids,
        [
            LAYOUT_OBJECT_BASE,
            LAYOUT_OBJECT_BASE + 1,
            LAYOUT_OBJECT_BASE + 3,
            BATTERY_RADAR_BASE,
        ]
    );
    assert_eq!(surface.units[0].side, ENEMY_SIDE);
    assert_eq!(surface.units[0].kind, UnitKind::Active);
    assert_eq!(surface.units[1].side, FRIENDLY_SIDE);
    assert_eq!(surface.units[2].kind, UnitKind::Passive);
    assert!(surface.units.iter().all(|u| u.in_scene && u.skill == 1));
    assert_eq!(surface.trucks.len(), 1);
    // Every placement keeps its scene id; the owned store is enemy, the
    // ownerless one neutral.
    let scene: Vec<u32> = terrain.airport_scene.objects.iter().map(|o| o.id).collect();
    assert_eq!(
        scene,
        (0..5)
            .map(|n| LAYOUT_OBJECT_BASE + n)
            .chain([BATTERY_RADAR_BASE])
            .collect::<Vec<_>>()
    );
    assert_eq!(surface.side_of(LAYOUT_OBJECT_BASE + 2), ENEMY_SIDE);
    assert_eq!(
        surface.side_of(LAYOUT_OBJECT_BASE + 4),
        tore_sim::combat::live::NO_SIDE
    );
    assert!(surface.template.is_none());
}

/// A mission whose spec names `target`, built as the creator's would be.
pub(super) fn world_with_target(r: &BTreeMap<String, Vec<u8>>, target: &GroundTarget) -> World {
    use crate::mission::Defense;
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.ground_target = Some(target.stem.clone());
    spec.aaa = Defense::from_level(target.aaa).unwrap();
    spec.sam = Defense::from_level(target.sam).unwrap();
    spec.surface_seed = target.seed;
    spec.enemy_nationality = target.enemy_nationality as u8;
    assert_eq!(GroundTarget::from_spec(&spec).as_ref(), Some(target));
    World::new(&spec, r, Seating::SinglePlayer).unwrap()
}

fn side_in_combat(world: &World, id: u32) -> Option<tore_sim::combat::live::Side> {
    world
        .combat
        .state
        .targets
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.side)
}

#[test]
fn combat_registers_every_unit_with_its_side() {
    let r = surface_resources();
    // The build registers the base layout's units with their sides.
    let spec = MissionSpec::new(THEATER, AircraftId::F18);
    let world = World::new(&spec, &r, Seating::SinglePlayer).unwrap();
    assert_eq!(side_in_combat(&world, LAYOUT_OBJECT_BASE), Some(ENEMY_SIDE));
    assert_eq!(
        side_in_combat(&world, LAYOUT_OBJECT_BASE + 1),
        Some(FRIENDLY_SIDE)
    );
    assert_eq!(
        side_in_combat(&world, LAYOUT_OBJECT_BASE + 4),
        Some(tore_sim::combat::live::NO_SIDE)
    );
    // With a ground target, its units join with theirs, and the parked
    // aircraft is a parked target of its own side.
    let mut world = world_with_target(&r, &target("QUCITY", 3, 3, 9));
    let surface = world.terrain.surface.clone();
    for unit in surface.template_units() {
        assert!(unit.in_scene);
        assert_eq!(
            side_in_combat(&world, unit.id.0),
            Some(unit.side),
            "{}",
            unit.resource
        );
    }
    assert_eq!(
        side_in_combat(&world, SURFACE_UNIT_BASE + 16),
        Some(ENEMY_SIDE)
    );
    assert!(world.combat.state.is_parked(SURFACE_UNIT_BASE + 16));
    // Units explode with their own record's look; base-layout buildings keep
    // the fitted ground-object blast. Since B16 a building that burns
    // scorches the ground round it (a crater as wide as its plan).
    let look = |id| {
        world
            .combat
            .state
            .ground_look(id)
            .map(|l| (l.explosion, l.crater))
    };
    assert_eq!(look(SURFACE_UNIT_BASE), Some((21, 6)));
    assert!(look(SURFACE_UNIT_BASE + 12).is_some_and(|(blast, scorch)| blast == 0 && scorch > 0));
    assert_eq!(look(LAYOUT_OBJECT_BASE), Some((21, 6)));
    assert!(
        look(LAYOUT_OBJECT_BASE + 2).is_some_and(|(blast, scorch)| blast
            == tore_sim::combat::blast::GROUND_OBJECT
            && scorch > 0),
        "{:?}",
        look(LAYOUT_OBJECT_BASE + 2)
    );
    assert_eq!(surface.targets().count(), 3);
    assert_eq!(world.combat.surface.units.len(), surface.units.len());
    // A restart keeps the sides.
    world.combat.reset(&mut world.cockpits[0].flight).unwrap();
    assert_eq!(side_in_combat(&world, SURFACE_UNIT_BASE), Some(ENEMY_SIDE));
    assert!(world.combat.state.is_parked(SURFACE_UNIT_BASE + 16));
}

#[test]
fn unit_hit_points_survive_a_checkpoint() {
    let r = surface_resources();
    let t = target("QUCITY", 3, 3, 4);
    let mut world = world_with_target(&r, &t);
    let damaged = [(SURFACE_UNIT_BASE + 12, 17), (LAYOUT_OBJECT_BASE, 0)];
    for (id, hp) in damaged {
        world
            .combat
            .state
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap()
            .hp = hp;
    }
    let bytes = world.checkpoint().unwrap();
    let mut fresh = world_with_target(&r, &t);
    fresh.restore(&bytes).unwrap();
    for (id, hp) in damaged {
        let row = fresh
            .combat
            .state
            .targets
            .iter()
            .find(|t| t.id == id)
            .unwrap();
        assert_eq!(row.hp, hp);
    }
    assert_eq!(fresh.combat.surface, world.combat.surface);
    assert_eq!(
        side_in_combat(&fresh, SURFACE_UNIT_BASE + 12),
        Some(ENEMY_SIDE)
    );
    // A world with another surface refuses it.
    let mut other = world_with_target(&r, &target("QUCITY", 1, 1, 4));
    assert!(other.restore(&bytes).is_err());
}

#[test]
fn the_digest_repeats_and_tells_surfaces_apart() {
    let r = surface_resources();
    let build = |t: &GroundTarget| {
        Terrain::for_mission_with(&r, THEATER, Some(0), &Overrides::default(), Some(t))
            .unwrap()
            .surface
    };
    let a = build(&target("QUCITY", 2, 2, 100));
    let b = build(&target("QUCITY", 2, 2, 100));
    assert_eq!(a.digest(), b.digest());
    assert_eq!(a, b);
    // Other rolls, another digest; so for a moved unit or a changed side.
    let other = (0..20)
        .map(|seed| build(&target("QUCITY", 2, 2, seed)))
        .find(|s| s.units != a.units)
        .unwrap();
    assert_ne!(other.digest(), a.digest());
    let mut moved = a.clone();
    moved.units.last_mut().unwrap().position[0] += 1;
    assert_ne!(moved.digest(), a.digest());
    let mut turned = a.clone();
    turned.units[0].side = FRIENDLY_SIDE;
    assert_ne!(turned.digest(), a.digest());
    // No target: the base layout alone.
    let base = Terrain::for_mission(&r, THEATER, Some(0), &Overrides::default())
        .unwrap()
        .surface;
    assert_ne!(base.digest(), a.digest());
}

#[test]
fn streams_are_independent_per_object_and_purpose() {
    let draw = |seed, stem, ordinal, purpose| Stream::new(seed, stem, ordinal, purpose).next_u64();
    let base = draw(1, "QUCOL", 3, Purpose::Roll);
    assert_eq!(base, draw(1, "QUCOL", 3, Purpose::Roll));
    assert_eq!(base, draw(1, "qucol", 3, Purpose::Roll));
    for other in [
        draw(2, "QUCOL", 3, Purpose::Roll),
        draw(1, "QUCIT", 3, Purpose::Roll),
        draw(1, "QUCOL", 4, Purpose::Roll),
        draw(1, "QUCOL", 3, Purpose::Pick),
    ] {
        assert_ne!(base, other);
    }
    let mut stream = Stream::new(9, "QX", 0, Purpose::Pick);
    assert!((0..1000).all(|_| stream.below(7) < 7));
    // Pinned: the draws are part of the network contract.
    assert_eq!(
        Stream::new(1234567, "QUCOL", 0, Purpose::Roll).next_u64(),
        PINNED_DRAW
    );
}

/// The first `Roll` draw for seed 1234567, `QUCOL`, object 0.
const PINNED_DRAW: u64 = 12_584_977_789_727_618_140;

#[test]
fn reserved_ranges_do_not_overlap() {
    assert_eq!(UnitId::layout(5).unwrap().range(), IdRange::Layout);
    assert_eq!(UnitId::template(5).unwrap().0, 0x5000_0005);
    assert_eq!(UnitId::supply_truck(0).unwrap().0, 0x5800_0000);
    assert_eq!(
        UnitId::supply_truck(0).unwrap().range(),
        IdRange::SupplyTruck
    );
    assert_eq!(UnitId::battery_radar(2).unwrap().0, 0x5C00_0002);
    assert_eq!(
        UnitId::battery_radar(2).unwrap().range(),
        IdRange::BatteryRadar
    );
    assert!(UnitId::template(0x0800_0000).is_none());
    assert!(UnitId::layout(0x1000_0000).is_none());
    assert_eq!(UnitId(SURFACE_PROJECTILE_ID_BASE).range(), IdRange::Other);
    // Client cosmetic rounds and AI shots stay outside every surface range.
    for id in [0xE000_0000, 0xF000_0000, 1 << 24] {
        assert_eq!(UnitId(id).range(), IdRange::Other);
    }
}

#[test]
fn mount_positions_scale_with_the_unit_to_real_size() {
    // KRIVAK.NT's forward mount, z -225 retail feet, lies 75 ft aft here.
    let mount = tore_formats::surface_unit::Mount {
        location: 0,
        flags: 8,
        position: [0, 30, -225],
        slew: [0, 0],
        slew_limit: [0, 0],
        kind: tore_formats::surface_unit::MountKind::Weapon,
        store: None,
        count: 1,
        weight_class: 0,
    };
    let at = mount_position_ft(&mount);
    assert!((at[0]).abs() < 1e-12 && (at[1] - 10.).abs() < 1e-12 && (at[2] + 75.).abs() < 1e-12);
    assert!(ground_offset_ft(&[0; 8]).is_err());
}

/// The spec of a mission with `target`, one enemy aircraft and Red 20 nm
/// ahead.
fn spec_with_enemy(target: &GroundTarget) -> MissionSpec {
    use crate::mission::{Defense, Skill};
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.ground_target = Some(target.stem.clone());
    spec.aaa = Defense::from_level(target.aaa).unwrap();
    spec.sam = Defense::from_level(target.sam).unwrap();
    spec.surface_seed = target.seed;
    spec.enemy_nationality = target.enemy_nationality as u8;
    spec.wings[3].count = 1;
    spec.wings[3].skill = Skill::Average;
    spec.separation_nm = 20;
    spec
}

#[test]
fn a_ground_target_puts_red_by_it_and_blue_the_separation_away() {
    use crate::surface::layout::{NM_FT, RED_START_NM};
    let r = surface_resources();
    let spec = spec_with_enemy(&target("QUCITY", 3, 3, 5));
    let mut world = World::new(&spec, &r, Seating::SinglePlayer).unwrap();
    let starts = world.terrain.surface.starts.clone().expect("starts");
    let nm = NM_FT as f64;
    let gap = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let target = starts.target.map(f64::from);
    let red_start = starts.red.map(f64::from);
    // Red within 5 nm of the target, Blue the 20 nm separation from Red.
    assert!(gap(red_start, target) <= RED_START_NM as f64 * nm);
    assert!((gap(starts.blue.map(f64::from), red_start) - 20. * nm).abs() <= 2.);
    let check = |world: &World| {
        let flight = &world.cockpits[0].flight;
        let blue = [flight.position[0], flight.position[2]];
        assert_eq!(blue, starts.blue.map(f64::from));
        let heading = f64::from(starts.blue_heading_deg).to_radians();
        assert!((flight.yaw - heading).abs() < 1e-9);
        // Flying at the target.
        let to_target = [target[0] - blue[0], target[1] - blue[1]];
        let off = (to_target[0].atan2(to_target[1]) - heading + std::f64::consts::PI)
            .rem_euclid(std::f64::consts::TAU)
            - std::f64::consts::PI;
        assert!(off.abs() < 0.5f64.to_radians(), "{off}");
        // The enemy aircraft starts on Red's spot.
        let red = world
            .combat
            .state
            .targets
            .iter()
            .find(|t| t.side == ENEMY_SIDE && t.aircraft.is_some())
            .expect("the enemy aircraft")
            .position;
        assert!(
            gap([red[0], red[2]], red_start) < 100.,
            "{red:?} {red_start:?}"
        );
    };
    check(&world);
    // A restart starts in the same place.
    let player = load_player(&r);
    world.restart(&player, &r).unwrap();
    check(&world);
    // Without a target the free-flight start stands, as before.
    let plain = World::new(
        &MissionSpec::new(THEATER, AircraftId::F18),
        &r,
        Seating::SinglePlayer,
    )
    .unwrap();
    assert!(plain.terrain.surface.starts.is_none());
    let free = plain.terrain.free_flight_start();
    let flight = &plain.cockpits[0].flight;
    assert_eq!([flight.position[0], flight.position[2]], [free[0], free[2]]);
}

fn load_player(
    r: &BTreeMap<String, Vec<u8>>,
) -> std::sync::Arc<crate::aircraft_type::AircraftType> {
    crate::aircraft_type::load_type(r, AircraftId::F18).unwrap()
}

/// [`surface_resources`] with four airfields about `~QUCITY`: one beside
/// it (anchoring it), a Blue one 20 nm west, a Red one 17 nm east and a
/// Blue one 40 nm south.
fn airfield_resources() -> BTreeMap<String, Vec<u8>> {
    let mut r = surface_resources();
    let airport = crate::test_support::resources::airport_resources();
    for name in ["STRIP.OT", "AIRPORT.SH"] {
        r.insert(name.into(), airport[name].clone());
    }
    let nm = 6_076;
    let centre = [MIDDLE + 2_550, MIDDLE + 5_000];
    let strip = |dx: i32, dz: i32, owner: &str| {
        format!(
            "obj\n\ttype STRIP.OT\n\tpos {} 0 {}\n\tangle 0 0 0\n{owner}\tflags $13\n\t.\n",
            centre[0] + dx,
            centre[1] + dz
        )
    };
    let mut layout = String::from_utf8(r["UKR.MM"].clone()).unwrap();
    layout += &strip(0, 3_000, "");
    layout += &strip(-20 * nm, 0, "\tnationality3 0\n");
    layout += &strip(17 * nm, 0, "\tnationality3 138\n");
    layout += &strip(0, -40 * nm, "\tnationality3 0\n");
    r.insert("UKR.MM".into(), layout.into_bytes());
    r
}

#[test]
fn each_airfield_takes_its_layout_side_and_ai_homes_follow_it() {
    use crate::ai_wings::Airfields;
    use tore_sim::{ai::launch::Side, airport::Allegiance};
    let r = airfield_resources();
    let spec = MissionSpec::new(THEATER, AircraftId::F18);
    let world = World::new(&spec, &r, Seating::SinglePlayer).unwrap();
    let scene = &world.terrain.airport_scene;
    let strip = |n| LAYOUT_OBJECT_BASE + n;
    let side_of = |n| {
        let id = scene.runway(strip(n)).unwrap().airport;
        scene
            .airports
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .allegiance
    };
    // Beside the target (no owner), west (Blue), east (Red), south (Blue).
    assert_eq!(
        [5, 6, 7, 8].map(side_of),
        [
            Allegiance::Neutral,
            Allegiance::Friendly,
            Allegiance::Hostile,
            Allegiance::Friendly
        ]
    );
    let fields = Airfields::from_world(&world.terrain, None);
    let at = |n| world.terrain.runway_view(strip(n)).unwrap().center;
    let home = |n, side| fields.home(at(n), side).map(|r| r.object);
    // Over the Red field Blue goes home to the neutral one beside the
    // target, the nearest it may use; over a Blue field Redfor does too.
    assert_eq!(home(7, Side::Friendly), Some(strip(5)));
    assert_eq!(home(7, Side::Enemy), Some(strip(7)));
    assert_eq!(home(6, Side::Enemy), Some(strip(5)));
    assert_eq!(home(6, Side::Friendly), Some(strip(6)));
}

#[test]
fn an_automatic_ground_start_takes_the_nearest_own_airfield_15_nm_out() {
    use crate::mission::Start;
    let r = airfield_resources();
    // This test measures field ranking, not support beside its synthetic runway.
    let mut spec = spec_with_enemy(&target("QUCITY", 0, 0, 2));
    // Ground starts keep their documented distance override.
    spec.separation_nm = 300;
    spec.start = Start::GroundAuto {
        altitude_ft: 10_000,
    };
    let world = World::new(&spec, &r, Seating::SinglePlayer).unwrap();
    let surface = &world.terrain.surface;
    // Beside a runway, the template stays.
    assert_eq!(
        surface.template.as_ref().unwrap().anchor,
        Some(layout::Anchor::Runway)
    );
    let starts = surface.starts.as_ref().unwrap();
    // Layout ordinals 5 to 8: beside, west (Blue), east (Red), south (Blue).
    let strip = |n| LAYOUT_OBJECT_BASE + n;
    assert_eq!(starts.blue_airfields, [strip(6), strip(8)]);
    assert_eq!(starts.red_airfields, [strip(7)]);
    assert_eq!(world.setup.ground_start, Some(strip(6)));
    assert_eq!(
        crate::mission_layout::auto_runway_for(&world.terrain, 1, true).unwrap(),
        strip(7)
    );
}

#[test]
fn the_hawk_radar_draws_srdr2_with_the_straight_flush_record() {
    use crate::surface::catalog::{HAWK_RADAR, HAWK_RADAR_NAME, hawk_radar_definition};
    let r = surface_resources();
    let mut catalog = Catalog::new(&r);
    let hawk = catalog.entry(HAWK_RADAR).unwrap();
    let flush = catalog.entry("SFLUSH.NT").unwrap();
    assert_eq!(
        (hawk.resource.as_str(), hawk.name.as_str()),
        (HAWK_RADAR, HAWK_RADAR_NAME)
    );
    assert_eq!(
        (hawk.class, hawk.hit_points),
        (flush.class, flush.hit_points)
    );
    let unit = hawk.unit.as_ref().unwrap();
    assert_eq!(unit.shape.as_deref(), Some("SRDR2.SH"));
    assert_eq!(unit.signatures, flush.unit.as_ref().unwrap().signatures);
    let definition = hawk_radar_definition(&r).unwrap();
    assert_eq!(definition.main_shape.as_deref(), Some("SRDR2.SH"));
    // Without its shape the element cannot be placed.
    let mut without = r.clone();
    without.remove("SRDR2.SH");
    assert!(hawk_radar_definition(&without).is_none());
    assert!(Catalog::new(&without).entry(HAWK_RADAR).is_err());
}

/// A Quick Mission with no ground target and heavy AAA and SAM strengths
/// (B23): the enemy's area defenses stand on Red's side, the same on a
/// single-player build and an open (networked) one, as combat targets.
#[test]
fn a_mission_with_no_target_gets_the_enemys_area_defenses() {
    let r = surface_resources();
    let mut spec = MissionSpec::new(THEATER, AircraftId::F18);
    spec.aaa = crate::mission::Defense::Heavy;
    spec.sam = crate::mission::Defense::Heavy;
    spec.surface_seed = 7;
    assert!(spec.area_defenses());
    let single = World::new(&spec, &r, Seating::SinglePlayer).unwrap();
    let open = World::new(&spec, &r, Seating::Open).unwrap();
    let surface = &single.terrain.surface;
    assert!(surface.template.is_none() && surface.starts.is_none());
    assert_eq!(
        surface.area,
        Some(resolve::AreaDefense {
            aaa: 3,
            sam: 3,
            seed: 7,
            enemy_nationality: usize::from(spec.enemy_nationality),
            night_stealth: false,
            rolled: true,
        })
    );
    let area: Vec<&Unit> = layout::area::units(surface).collect();
    let wanted = 2 * layout::AREA_SLOTS * layout::AREA_SITE_UNITS;
    assert_eq!(area.len(), wanted, "{:?}", surface.layout_notes);
    for unit in &area {
        assert_eq!(unit.side, ENEMY_SIDE);
        assert!(!unit.is_target());
        assert!(
            single
                .combat
                .state
                .targets
                .iter()
                .any(|t| t.id == unit.id.0),
            "{:#x} is no combat target",
            unit.id.0
        );
    }
    assert_eq!(surface.digest(), open.terrain.surface.digest());
    // John, 2026-10-13: the SAM units have supply trucks, which are combat
    // targets like the template's.
    assert!(!surface.trucks.is_empty());
    for truck in &surface.trucks {
        assert!(
            single
                .combat
                .state
                .targets
                .iter()
                .any(|t| t.id == truck.id.0),
            "truck {:#x} is no combat target",
            truck.id.0
        );
    }
    // Another seed, other places.
    spec.surface_seed = 8;
    let other = World::new(&spec, &r, Seating::SinglePlayer).unwrap();
    assert_ne!(surface.digest(), other.terrain.surface.digest());
    // At none, or with a ground target, there are none.
    spec.aaa = crate::mission::Defense::None;
    spec.sam = crate::mission::Defense::None;
    let none = World::new(&spec, &r, Seating::SinglePlayer).unwrap();
    assert!(none.terrain.surface.area.is_none());
    assert_eq!(layout::area::units(&none.terrain.surface).count(), 0);
    let with_target = crate::test_support::surface::spec_with_target(&target("QUCITY", 3, 3, 7));
    let targeted = World::new(&with_target, &r, Seating::SinglePlayer).unwrap();
    assert!(targeted.terrain.surface.area.is_none());
    assert_eq!(layout::area::units(&targeted.terrain.surface).count(), 0);
}

/// Reuse checks the current separation and the final obstacles, not just raw layout.
#[test]
fn audit_front_recheck_agrees_with_independent_scene_builds() {
    let resources = surface_resources();
    let site = crate::terrain::SurfaceSite::load(&resources, THEATER).unwrap();
    let overrides = Overrides {
        city_fill: false,
        ..Overrides::default()
    };
    let front = |surface: &Surface| {
        surface
            .units
            .iter()
            .filter(|u| u.id.range() == IdRange::FrontLine)
            .cloned()
            .collect::<Vec<_>>()
    };
    let mut matches = 0;
    for seed in 1..=3 {
        let mut target = target("QUCITY", 0, 0, seed);
        target.separation_nm = 1;
        let mut first = Terrain::for_mission_surface(
            &resources,
            THEATER,
            Some(0),
            &overrides,
            Some(&target),
            None,
        )
        .unwrap();
        assert!(!front(&first.surface).is_empty());
        for distance in [1, 2, 5, 10, 20, 50, 100, 200, 300] {
            target.separation_nm = distance;
            let raw = site
                .place(&resources, &first.theater, Some(&target))
                .unwrap();
            let direct = Terrain::for_mission_surface(
                &resources,
                THEATER,
                Some(0),
                &overrides,
                Some(&target),
                None,
            )
            .unwrap();
            let expected = front(&first.surface) == front(&direct.surface);
            assert_eq!(
                site.inspection_front_matches(&resources, &first, &raw),
                expected,
                "seed {seed}, separation {distance}"
            );
            if expected {
                matches += 1;
            }
        }
        // An airport replacement can block an earlier candidate even when the
        // raw front has not moved. The cache must reject the stale final front.
        let unit = front(&first.surface).remove(0);
        first.placement_objects.push(layout::Placed {
            id: 0xffff_fffe,
            at: [i64::from(unit.position[0]), i64::from(unit.position[2])],
            heading: 0,
            footprint: layout::Footprint {
                min: [-200, -200],
                max: [200, 200],
            },
        });
        target.separation_nm = 1;
        let raw = site
            .place(&resources, &first.theater, Some(&target))
            .unwrap();
        assert!(!site.inspection_front_matches(&resources, &first, &raw));
    }
    assert!(matches > 0, "must exercise reuse as well as rejection");
}
