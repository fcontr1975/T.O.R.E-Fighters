//! The surface controllers in the world: launch gates, magazines, radars,
//! HARM shutdown, the RWR lock feed, batteries and the barrage zone, on
//! synthetic units with the retail records' numbers (plan 9.1 W3, 4.3 test 4).
//! No retail data.
use super::*;
use crate::surface::{
    Battery, DestroyedLook, Origin, SurfaceUnitState, Unit, UnitKind, supply,
    units::{BatteryState, Engager},
};
use tore_formats::weapons::{
    Burst, Damage, Effects, Guidance, Movement, Seeker as SeekerRecord, Zone,
};
use tore_sim::{
    ai::surface::Phase,
    checkpoint::{Models, round_trip},
    combat::live::{NO_SIDE, Side},
};

const TPS: u64 = 120;
const RED: Side = Side(2);
const BLUE: Side = Side(1);
/// The aircraft every test engages.
const JET: u32 = 7;

fn zone(
    heading: i16,
    min_range: i32,
    max_range: i32,
    min_altitude: i32,
    max_altitude: i32,
) -> Zone {
    Zone {
        heading,
        pitch: heading,
        minimum_range: min_range,
        maximum_range: max_range,
        minimum_altitude: min_altitude,
        maximum_altitude: max_altitude,
    }
}

/// A record with every field zero but the name; the tests set the retail
/// fields they need.
fn record(source: &str) -> Weapon {
    Weapon {
        source: source.into(),
        name: source.into(),
        hud_name: source.into(),
        shape: None,
        fire_sound: None,
        native_callback: String::new(),
        flags: 0,
        object_flags: 0,
        weight: 0,
        movement: Movement {
            minimum_speed: 0,
            corner_speed: 0,
            maximum_speed: 0,
            acceleration: 0,
            deceleration: 0,
            initial_speed: 0,
            final_speed: 0,
            launch_retard: 0,
            ignite_t: 0,
            fuel_t: 0,
            remove_t: 0,
            powered_turn_rate: 0,
            unpowered_turn_rate: 0,
            performance_at_0: 100,
            performance_at_20: 100,
            cruise: [0; 4],
            jink: [0; 3],
        },
        burst: Burst {
            projectiles_in_pod: 1,
            actual_rounds_per_game: 1,
            game_rounds_in_burst: 1,
            game_rounds_in_carpet_burst: 1,
            game_burst_t: 0,
            reload_t: 0,
            startup_shots: 0,
            random_fire_percent: 0,
            offset_fire_percent: 0,
            offset_fire_heading: 0,
            offset_fire_pitch: 0,
            sine_pattern: [0; 4],
        },
        seeker: SeekerRecord {
            flags: [0; 2],
            signature: 0,
            look_down: 0,
            doppler_above: 0,
            doppler_below: 0,
            doppler_minimum_range: 0,
            all_aspect: 0,
            zones: [zone(0, 0, 0, 0, 0); 2],
            chaff_flare_chance: 100,
            deception_chance: 100,
        },
        guidance: Guidance {
            track_t: 0,
            track_max_g_raw: 0,
            target_sun_chance: 0,
            max_aon: 0,
            chances: [0; 4],
            hit_modifiers: [0; 9],
        },
        damage: Damage {
            by_class: [0; 5],
            fuze_arm_t: 0,
            fuze_radius: 100,
            side_hit_fuze_failure: 0,
            collateral_radius: 0,
            collateral_percent: 0,
        },
        effects: Effects {
            object_explosion: 0,
            land_explosion: 0,
            water_explosion: 0,
            crater_size: 0,
            smoke: [0; 5],
            max_sound_distance: 0,
            frequency_adjustment: 0,
        },
    }
}

/// A surface missile from rest, as the LIB's SAM records.
fn missile(source: &str, signature: u8, zones: [Zone; 2], reload_t: u8, remove_t: u16) -> Weapon {
    let mut w = record(source);
    w.flags = 0x12341;
    w.seeker.signature = signature;
    w.seeker.zones = zones;
    w.guidance.track_t = 20;
    w.burst.reload_t = reload_t;
    w.movement.ignite_t = 8;
    w.movement.fuel_t = 92;
    w.movement.remove_t = remove_t;
    w.movement.final_speed = 1026;
    w.movement.maximum_speed = 2600;
    w.movement.corner_speed = 1500;
    w.movement.acceleration = 800;
    w.movement.powered_turn_rate = 2000;
    w.movement.unpowered_turn_rate = 1000;
    w.damage.by_class = [75, 7, 22, 15, 75];
    w.damage.fuze_arm_t = 4;
    w
}
fn sa6() -> Weapon {
    missile(
        "SA6.JT",
        3,
        [
            zone(32767, 0, 80_000, 500, i32::MAX),
            zone(5460, 9_000, 75_000, 2_000, 30_000),
        ],
        60,
        80,
    )
}
fn sa2() -> Weapon {
    missile(
        "SA2A.JT",
        3,
        [
            zone(32767, 0, 150_000, 3_000, i32::MAX),
            zone(5460, 7_500, 95_000, 5_000, 50_000),
        ],
        80,
        160,
    )
}
fn sa7() -> Weapon {
    missile(
        "SA7.JT",
        2,
        [
            zone(32767, 0, 15_000, 0, i32::MAX),
            zone(5460, 500, 9_000, i32::MIN, 6_000),
        ],
        40,
        80,
    )
}
fn gun(source: &str, flags: u32, signature: u8, zones: [Zone; 2], speed: i16) -> Weapon {
    let mut w = record(source);
    w.flags = flags;
    w.seeker.signature = signature;
    w.seeker.zones = zones;
    w.guidance.track_t = 4;
    w.movement.initial_speed = speed;
    w.movement.final_speed = speed;
    w.movement.minimum_speed = speed;
    w.movement.maximum_speed = speed;
    w.movement.corner_speed = speed;
    w.movement.remove_t = 20;
    w.damage.by_class = [20, 2, 6, 4, 20];
    w
}
fn zsu23() -> Weapon {
    let mut w = gun(
        "ZSU23.JT",
        0x140c0,
        3,
        [
            zone(32767, 0, 60_000, i32::MIN, i32::MAX),
            zone(8190, 50, 7_500, i32::MIN, 7_500),
        ],
        3666,
    );
    w.burst.game_rounds_in_burst = 4;
    w.burst.game_burst_t = 1;
    w.burst.reload_t = 4;
    w
}
fn m1939() -> Weapon {
    let mut w = gun(
        "M1939.JT",
        0x100c0,
        0,
        [
            zone(32767, 0, 20_000, i32::MIN, i32::MAX),
            zone(8190, 0, 10_000, i32::MIN, 6_000),
        ],
        3960,
    );
    w.burst.game_rounds_in_burst = 4;
    w.burst.game_burst_t = 2;
    w.burst.reload_t = 12;
    w
}
fn a_m1939() -> Weapon {
    let mut w = m1939();
    w.source = "A_M1939.JT".into();
    w.flags = 0x5140c0;
    w.burst.random_fire_percent = 33;
    w.burst.offset_fire_percent = 33;
    w.burst.offset_fire_heading = 3640;
    w.burst.offset_fire_pitch = 3640;
    w
}
fn ks19() -> Weapon {
    let mut w = gun(
        "KS19.JT",
        0x2940c0,
        3,
        [
            zone(32767, 0, 100_000, i32::MIN, i32::MAX),
            zone(8190, 0, 50_000, 4_000, 25_000),
        ],
        4400,
    );
    w.movement.remove_t = 60;
    w.burst.game_burst_t = 2;
    w.burst.reload_t = 16;
    w.burst.startup_shots = 8;
    w.damage.by_class = [80, 8, 24, 16, 80];
    w.damage.fuze_radius = 250;
    w.damage.collateral_radius = 750;
    w.damage.collateral_percent = 35;
    w
}
fn phalanx() -> Weapon {
    let mut w = gun(
        "PHALANX.JT",
        0x140c0,
        3,
        [
            zone(32767, 0, 60_000, i32::MIN, i32::MAX),
            zone(8190, 50, 20_000, i32::MIN, 15_000),
        ],
        3666,
    );
    w.movement.remove_t = 40;
    w.burst.game_rounds_in_burst = 6;
    w.burst.game_burst_t = 1;
    w.burst.reload_t = 4;
    w
}
fn agm88() -> Weapon {
    let mut w = record("AGM88.JT");
    w.flags = 0x2204f;
    w.seeker.signature = 4;
    w.seeker.zones = [
        zone(8190, 0, 200_000, i32::MIN, i32::MAX),
        zone(8190, 500, 200_000, i32::MIN, i32::MAX),
    ];
    w.movement.fuel_t = 100;
    w.movement.remove_t = 160;
    w.movement.final_speed = 1026;
    w.movement.maximum_speed = 2600;
    w.movement.acceleration = 800;
    w
}

/// An arc of `rest` plus or minus `limit` degrees (0 is unrestricted), its
/// mount 10 ft up.
fn arc(index: usize, rest: [f64; 2], limit: [f64; 2]) -> MountArc {
    MountArc {
        index,
        rest,
        limit,
        offset: [0., 10., 0.],
        barrel: None,
    }
}
const FREE: [f64; 2] = [0., 0.];
/// NPC times (search, unready, attack, retarget), quarter seconds: the SA-6's
/// and the ZSU-23's.
const SAM_NPC: [i32; 4] = [40, 144, 60, 32_767];
const GUN_NPC: [i32; 4] = [20, 80, 40, 32_767];

/// A surface unit and its arms.
struct Placed {
    unit: Unit,
    arms: Arms,
}

fn placed(id: u32, resource: &str, class: u16, x: f64, z: f64, skill: i32) -> Placed {
    let unit = Unit {
        id: UnitId(id),
        origin: Origin::Layout {
            ordinal: id & 0xffff,
        },
        resource: resource.into(),
        kind: UnitKind::Active,
        class,
        name: resource.into(),
        nationality: Some(0x80),
        side: RED,
        position: [x as i32, 0, z as i32],
        angles: [0; 3],
        flags: 0,
        skill,
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
    };
    let arms = Arms {
        unit: UnitId(id),
        position: [x, 0., z],
        heading: 0.,
        skill,
        side: RED,
        react: 0,
        search_limit: None,
        ship: class & tore_formats::surface_unit::class::SHIP != 0,
        weapons: Vec::new(),
        radar: None,
        battery: None,
        loads: Vec::new(),
        npc: SAM_NPC,
        team: false,
    };
    Placed { unit, arms }
}

impl Placed {
    fn with_missile(mut self, w: Weapon, rails: &[(MountArc, u32)]) -> Self {
        let mut weapon =
            WeaponArms::missile(w, rails[0].0, self.arms.skill, self.arms.npc, false).unwrap();
        weapon.mounts = rails.iter().map(|(a, _)| *a).collect();
        for (a, n) in rails {
            self.load(
                a.index,
                MountStock {
                    loaded: *n,
                    reserve: Some(0),
                    ordinal: 0,
                },
            );
        }
        self.arms.weapons.push(weapon);
        self
    }
    fn with_gun(mut self, w: Weapon, unit: &str, mount: MountArc, zone_dist: i32) -> Self {
        let (weapon, stock) = WeaponArms::gun(
            w,
            unit,
            mount,
            self.arms.skill,
            self.arms.ship,
            GUN_NPC,
            zone_dist,
        )
        .unwrap();
        self.load(mount.index, stock);
        self.arms.weapons.push(weapon);
        self
    }
    fn load(&mut self, index: usize, stock: MountStock) {
        if self.arms.loads.len() <= index {
            self.arms.loads.resize(index + 1, MountStock::default());
        }
        self.arms.loads[index] = stock;
    }
    fn radar(mut self, named: bool, battery: Option<bool>) -> Self {
        self.arms.radar = emitters::rule(&self.arms, named, None, battery);
        self
    }
}

/// A block of terrain `height` feet high over x and z ranges.
#[derive(Clone, Copy)]
struct Ridge {
    x: (f64, f64),
    z: (f64, f64),
    height: f64,
}

/// The surface, its state and combat, and one jet.
struct Fixture {
    surface: Surface,
    state: SurfaceState,
    live: live::State,
    jet: Aircraft,
    /// More aircraft beside the jet (a flight).
    others: Vec<Aircraft>,
    tick: u64,
    ridge: Option<Ridge>,
    /// Whether the scene carries a ground ceiling built from the ridge (the
    /// world's always does); every test runs with it unless it compares.
    ceiling: bool,
    daylight: bool,
    fired: Vec<(u64, Trace)>,
    traces: Vec<(u64, Trace)>,
    supports: Vec<ActorSupport>,
}

impl Fixture {
    fn new(units: Vec<Placed>, batteries: Vec<Battery>) -> Self {
        let mut surface = Surface::default();
        let mut live = live::State::without_ownships();
        for p in &units {
            live.add_ground_target(
                p.unit.id.0,
                tore_sim::airport::OrientedBox {
                    center: [p.arms.position[0], 8., p.arms.position[2]],
                    half: [15., 8., 15.],
                    heading: 0.,
                    pitch: 0.,
                    bank: 0.,
                },
                100,
                p.unit.class,
                p.unit.side,
            )
            .unwrap();
        }
        // Passive units (the resupply tests' trucks) have no arms.
        let mut arms: Vec<Arms> = units
            .iter()
            .filter(|p| p.unit.kind == UnitKind::Active)
            .map(|p| p.arms.clone())
            .collect();
        for (index, battery) in batteries.iter().enumerate() {
            for a in &mut arms {
                if a.unit == battery.radar || battery.launchers.contains(&a.unit) {
                    a.battery = Some(index);
                    if a.unit != battery.radar {
                        for w in &mut a.weapons {
                            w.battery = true;
                        }
                    }
                }
            }
        }
        surface.units = units.into_iter().map(|p| p.unit).collect();
        surface.units.sort_by_key(|u| u.id);
        arms.sort_by_key(|a| a.unit);
        surface.batteries = batteries;
        surface.arsenal.flak_teams = flak_teams(&arms);
        surface.arsenal.units = arms;
        for (index, battery) in surface.batteries.iter().enumerate() {
            let lead = surface.arsenal.arms(battery.launchers[0]).unwrap();
            let mut weapon = lead.weapons[0].clone();
            let skill = battery
                .launchers
                .iter()
                .map(|id| surface.arsenal.arms(*id).unwrap().skill)
                .max()
                .unwrap();
            let [search, unready, attack, retarget] = lead.npc;
            weapon.profile.timing = Timing::from_quarters(
                search,
                unready,
                attack,
                weapon.record.guidance.track_t,
                retarget,
                skill,
            );
            weapon.launch = launch_band(&weapon.record, Kind::Missile, skill);
            let entry = BatteryArms {
                index,
                system: battery.system,
                radar: battery.radar,
                launchers: battery.launchers.clone(),
                weapon,
                skill,
                side: battery.side,
                react: 0,
            };
            surface.arsenal.batteries.push(entry);
        }
        let state = surface.fresh_state();
        Self {
            surface,
            state,
            live,
            jet: Aircraft {
                id: JET,
                side: BLUE,
                position: [0., 15_000., 30_000.],
                velocity: [0., 0., 0.],
                category: 0x8000,
                airborne: true,
                jammer: false,
            },
            others: Vec::new(),
            tick: 0,
            ridge: None,
            ceiling: true,
            daylight: true,
            fired: Vec::new(),
            traces: Vec::new(),
            supports: Vec::new(),
        }
    }
    fn step(&mut self) {
        let ridge = self.ridge;
        let ground = move |x: f64, z: f64| match ridge {
            Some(r) if (r.x.0..=r.x.1).contains(&x) && (r.z.0..=r.z.1).contains(&z) => r.height,
            _ => 0.,
        };
        let aircraft: Vec<Aircraft> = std::iter::once(self.jet)
            .chain(self.others.iter().copied())
            .collect();
        let ceiling = self.ceiling.then(|| {
            let mut ceiling =
                tore_sim::ground_ceiling::GroundCeiling::from_grid(64, 64, 4096., |_, _| 0.);
            if let Some(r) = ridge {
                ceiling.raise([r.x.0, r.z.0], [r.x.1, r.z.1], r.height);
            }
            ceiling
        });
        let scene = Scene {
            tick: self.tick,
            aircraft: &aircraft,
            ground: &ground,
            daylight: self.daylight,
            ceiling: ceiling.as_ref(),
        };
        // Resupply (a no-op without trucks), as the world runs it.
        let delivery = supply::step(
            &self.surface,
            &mut self.state,
            &self.live,
            &|unit, state| match state.and_then(|s| s.mover) {
                Some(mover) => {
                    let at = mover.position();
                    [at[0], at[2]]
                }
                None => [f64::from(unit.position[0]), f64::from(unit.position[2])],
            },
        );
        let stepped = step(&self.surface, &mut self.state, &mut self.live, &scene);
        supply::deliver(&self.surface, &mut self.state, delivery);
        self.supports = stepped.supports;
        for trace in &self.state.trace {
            if matches!(trace, Trace::Shot { .. }) {
                self.fired.push((self.tick, trace.clone()));
            }
            self.traces.push((self.tick, trace.clone()));
        }
        // Rounds are not flown here: only how many leave and when.
        self.live.projectiles.clear();
        self.tick += 1;
    }
    fn run(&mut self, ticks: u64) {
        for _ in 0..ticks {
            self.step();
        }
    }
    fn rounds(&self) -> u32 {
        self.fired
            .iter()
            .map(|(_, t)| match t {
                Trace::Shot { rounds, .. } => *rounds,
                _ => 0,
            })
            .sum()
    }
    fn shooters(&self) -> Vec<u32> {
        self.fired
            .iter()
            .filter_map(|(_, t)| match t {
                Trace::Shot { unit, rounds, .. } if *rounds > 0 => Some(unit.0),
                _ => None,
            })
            .collect()
    }
    fn first(&self, phase: Phase) -> Option<u64> {
        self.traces
            .iter()
            .find(|(_, t)| matches!(t, Trace::Phase { phase: p, .. } if *p == phase))
            .map(|(tick, _)| *tick)
    }
    fn unit_state(&self, id: u32) -> &SurfaceUnitState {
        self.state.unit(UnitId(id)).unwrap()
    }
    fn phase(&self, id: u32) -> Phase {
        self.unit_state(id).engagers[0].controller.phase()
    }
    fn emitting(&self, id: u32) -> bool {
        self.live
            .targets
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .radar_emitting
    }
    fn kill(&mut self, id: u32) {
        self.live
            .targets
            .iter_mut()
            .find(|t| t.id == id)
            .unwrap()
            .hp = 0;
    }
}

const SA6_ID: u32 = 0x4000_0001;

fn sa6_site(skill: i32) -> Placed {
    placed(SA6_ID, "SA6.NT", 0x1000, 0., 0., skill)
        .with_missile(sa6(), &[(arc(0, [0., 0.], [0., 70.]), 3)])
        .radar(false, None)
}

#[test]
fn an_sa6_site_engages_at_the_record_times_and_paints_and_locks() {
    let mut f = Fixture::new(vec![sa6_site(1)], Vec::new());
    f.run(TPS);
    assert!(
        f.emitting(SA6_ID),
        "a hostile inside 80,000 ft turns the radar on"
    );
    // Search at once, 36 s unready preparation, 5 s lock: the launch at 41 s.
    f.run(41 * TPS);
    assert_eq!(f.rounds(), 1);
    assert_eq!(f.fired[0].0, 41 * TPS);
    // While it holds the lock it paints the jet and sounds a radar lock.
    assert_eq!(f.state.locks_on(JET), vec![3]);
    assert_eq!(f.state.painting(JET), vec![SA6_ID]);
    // Its own radar supports the missile.
    let support = f.supports.iter().find(|s| s.owner == SA6_ID).unwrap();
    assert!(support.supported && support.radar_emitting);
    assert_eq!(support.observation.unwrap().id, JET);
    // 15 s to the next salvo; three rails, then Empty and silent.
    f.run(60 * TPS);
    let times: Vec<u64> = f.fired.iter().map(|(t, _)| *t).collect();
    assert_eq!(times, vec![41 * TPS, 56 * TPS, 71 * TPS]);
    assert_eq!(f.unit_state(SA6_ID).mounts[0].loaded, 0);
    assert_eq!(f.phase(SA6_ID), Phase::Empty);
    assert!(f.state.locks_on(JET).is_empty());
}

#[test]
fn altitude_floors_keep_a_low_jet_from_launches() {
    let sa2 = placed(SA6_ID, "SA2A.NT", 0x1000, 0., 0., 1)
        .with_missile(sa2(), &[(arc(0, [0., 45.], [0., 15.]), 1)])
        .radar(false, None);
    let mut f = Fixture::new(vec![sa2], Vec::new());
    // 1,000 ft: below the seeker floor (3,000 ft), so never eligible, but
    // inside range: the radar is on.
    f.jet.position = [0., 1_000., 30_000.];
    f.run(120 * TPS);
    assert!(f.emitting(SA6_ID));
    assert_eq!(f.rounds(), 0);
    assert_eq!(f.phase(SA6_ID), Phase::Search);
    // 4,000 ft: seen (above 3,000 ft) but below the 5,000 ft launch floor.
    f.jet.position = [0., 4_000., 30_000.];
    f.run(60 * TPS);
    assert_eq!(f.rounds(), 0);
    assert!(f.first(Phase::Track).is_some());
    // 8,000 ft: launched.
    f.jet.position = [0., 8_000., 30_000.];
    f.run(60 * TPS);
    assert_eq!(f.rounds(), 1);
}

#[test]
fn terrain_masks_a_site_and_mount_arcs_gate_ship_guns() {
    let mut f = Fixture::new(vec![sa6_site(1)], Vec::new());
    f.ridge = Some(Ridge {
        x: (-1e9, 1e9),
        z: (14_700., 15_300.),
        height: 20_000.,
    });
    f.run(90 * TPS);
    assert_eq!(f.rounds(), 0, "a ridge between hides the jet");
    f.ridge = None;
    f.run(60 * TPS);
    assert!(f.rounds() > 0);

    // A ship's forward CIWS (rest 0, plus or minus 120 degrees) never fires
    // astern; its aft one (rest 180) does.
    let ship = placed(0x4000_0002, "NIMZ.NT", 0x2000, 0., 0., 1)
        .with_gun(phalanx(), "NIMZ", arc(0, [0., 0.], [120., 90.]), 0)
        .with_gun(phalanx(), "NIMZ", arc(1, [180., 0.], [120., 90.]), 0)
        .radar(false, None);
    let mut f = Fixture::new(vec![ship], Vec::new());
    f.jet.position = [0., 2_000., -6_000.];
    f.run(60 * TPS);
    let mounts: Vec<usize> = f
        .fired
        .iter()
        .filter_map(|(_, t)| match t {
            Trace::Shot { mount, rounds, .. } if *rounds > 0 => Some(*mount),
            _ => None,
        })
        .collect();
    assert!(!mounts.is_empty());
    assert!(mounts.iter().all(|m| *m == 1));
    // Ships never run out: unlimited reserve.
    assert_eq!(f.unit_state(0x4000_0002).mounts[1].reserve, None);
}

const ZSU_ID: u32 = 0x4000_0003;

#[test]
fn zsu23_bursts_empties_its_magazine_swaps_in_120_s_and_falls_silent() {
    let zsu = placed(ZSU_ID, "ZSU23.NT", 0x0800, 0., 0., 1)
        .with_gun(zsu23(), "ZSU23", arc(0, FREE, [0., 90.]), 0)
        .radar(false, None);
    let row = surface_guns::tuning("ZSU23", "ZSU23.JT").unwrap();
    let mut f = Fixture::new(vec![zsu], Vec::new());
    f.jet.position = [0., 1_000., 3_000.];
    // 20 s unready preparation, 1 s lock: the first burst.
    f.run(21 * TPS + 1);
    assert_eq!(f.fired[0].0, 21 * TPS);
    f.run(3 * TPS);
    // One whole burst of the table's rounds over its burst time, then the
    // 1 s pause.
    let burst_ticks = u64::from(row.burst_quarters()) * 30;
    let first: u32 = f
        .fired
        .iter()
        .filter(|(t, _)| *t < 21 * TPS + burst_ticks)
        .map(|(_, s)| match s {
            Trace::Shot { rounds, .. } => *rounds,
            _ => 0,
        })
        .sum();
    assert_eq!(first, u32::from(row.burst));
    let second = f
        .fired
        .iter()
        .find(|(t, _)| *t >= 21 * TPS + burst_ticks)
        .unwrap()
        .0;
    assert_eq!(second, 21 * TPS + burst_ticks + 120);
    // The magazine empties, a 120 s swap from the reserve, twice; then silent.
    f.run(600 * TPS);
    assert_eq!(f.rounds(), 3 * row.magazine);
    let swaps: Vec<u64> = f
        .traces
        .iter()
        .filter(|(_, t)| matches!(t, Trace::Swap { .. }))
        .map(|(tick, _)| *tick)
        .collect();
    assert_eq!(swaps.len(), 2);
    assert_eq!(swaps[0], f.first(Phase::Reload).unwrap() + 120 * TPS);
    let stock = f.unit_state(ZSU_ID).mounts[0];
    assert_eq!((stock.loaded, stock.reserve), (0, Some(0)));
    assert_eq!(f.phase(ZSU_ID), Phase::Empty);
    // Silent until a truck is in reach (the resupply slice's flag).
    let silent = f.rounds();
    f.run(200 * TPS);
    assert_eq!(f.rounds(), silent);
    f.state.unit_mut(UnitId(ZSU_ID)).unwrap().supply = true;
    // A 120 s swap from the truck, then a new search and preparation.
    f.run(140 * TPS);
    assert!(f.rounds() > silent);
}

/// An airfield's guns fire only at the other side's aircraft once they are
/// airborne: never at their own side, never with no side on either end, and
/// never at a jet still rolling on the runway. A Blue wing taking off from a
/// Redfor field draws fire; one taking off from its own field does not.
#[test]
fn guns_fire_only_at_the_other_sides_airborne_aircraft() {
    let rounds = |unit_side: Side, jet_side: Side, airborne: bool| {
        let mut zsu = placed(ZSU_ID, "ZSU23.NT", 0x0800, 0., 0., 1)
            .with_gun(zsu23(), "ZSU23", arc(0, FREE, [0., 90.]), 0)
            .radar(false, None);
        zsu.unit.side = unit_side;
        zsu.arms.side = unit_side;
        let mut f = Fixture::new(vec![zsu], Vec::new());
        f.jet.side = jet_side;
        f.jet.airborne = airborne;
        f.jet.position = [0., 300., 2_000.];
        f.run(30 * TPS);
        f.rounds()
    };
    assert!(rounds(RED, BLUE, true) > 0, "Redfor guns engage a Blue jet");
    assert!(rounds(BLUE, RED, true) > 0, "Blue guns engage a Redfor jet");
    assert_eq!(rounds(RED, RED, true), 0, "never their own side");
    assert_eq!(rounds(BLUE, BLUE, true), 0, "never their own side");
    assert_eq!(rounds(NO_SIDE, BLUE, true), 0, "a unit with no side");
    assert_eq!(rounds(RED, NO_SIDE, true), 0, "an aircraft with no side");
    assert_eq!(rounds(RED, BLUE, false), 0, "not while it is on the ground");
}

/// The retail SMLARMS record's fire zones: visual, 0 to 4,000 ft, to 2,000 ft.
fn smlarms() -> Weapon {
    let mut w = gun(
        "SMLARMS.JT",
        0x100c0,
        0,
        [
            zone(32767, 0, 20_000, i32::MIN, i32::MAX),
            zone(8190, 0, 4_000, i32::MIN, 2_000),
        ],
        3666,
    );
    w.burst.game_rounds_in_burst = 4;
    w.burst.game_burst_t = 1;
    w.burst.reload_t = 4;
    w
}

const SQUAD_ID: u32 = 0x5000_0021;

fn squad(side: Side, flags: i32) -> Placed {
    let mut squad = placed(SQUAD_ID, "TROOPS.NT", 0x0040, 0., 0., 1).with_gun(
        smlarms(),
        "TROOPS",
        arc(0, FREE, FREE),
        0,
    );
    squad.unit.side = side;
    squad.arms.side = side;
    squad.unit.flags = flags;
    squad
}

/// An airfield's defences fire at the other side's aircraft in reach whether
/// or not the airfield is a mission target (John, 2026-10-12: "airfields fire
/// at enemies at all times"): a squad of the field's template, flagged as a
/// destroy target or not, and the field's AAA gun beside it, engage a Blue
/// jet, and neither engages a Redfor one.
#[test]
fn an_airfields_squad_and_gun_fire_at_enemies_target_or_not_and_never_at_their_own_side() {
    const GUN: u32 = 0x5000_0022;
    let shooters = |flags: i32, jet_side: Side| {
        let aaa = placed(GUN, "ZSU23.NT", 0x0800, 40., 0., 1)
            .with_gun(zsu23(), "ZSU23", arc(0, FREE, [0., 90.]), 0)
            .radar(false, None);
        let mut f = Fixture::new(vec![squad(RED, flags), aaa], Vec::new());
        f.jet.side = jet_side;
        f.jet.position = [0., 600., 2_500.];
        f.run(40 * TPS);
        let mut who = f.shooters();
        who.sort_unstable();
        who.dedup();
        who
    };
    for flags in [0, tore_formats::quick_template::TARGET_FLAG] {
        assert_eq!(
            shooters(flags, BLUE),
            vec![SQUAD_ID, GUN],
            "flags {flags:#x}: squad and gun both engage"
        );
        assert!(shooters(flags, RED).is_empty(), "never their own side");
    }
}

/// A squad killed in the fight stops firing at once, and a squad at its
/// full strength fires at a hostile jet inside its reach only.
#[test]
fn a_squad_fires_inside_its_reach_and_stops_when_it_is_killed() {
    let mut f = Fixture::new(vec![squad(RED, 0)], Vec::new());
    f.jet.position = [0., 600., 2_500.];
    f.run(40 * TPS);
    let fired = f.rounds();
    assert!(fired > 0, "the squad engages a jet 2,500 ft away");
    f.kill(SQUAD_ID);
    f.run(1);
    let at_death = f.rounds();
    f.run(60 * TPS);
    assert_eq!(f.rounds(), at_death, "a dead squad is silent");
    // Out of its 4,000 ft reach, and above its 2,000 ft ceiling.
    for position in [[0., 600., 6_000.], [0., 3_000., 1_000.]] {
        let mut far = Fixture::new(vec![squad(RED, 0)], Vec::new());
        far.jet.position = position;
        far.run(40 * TPS);
        assert_eq!(far.rounds(), 0, "{position:?} is out of reach");
    }
}

/// Riflemen shoot at where a fast jet was: a squad's aim point carries the
/// jet on at 30 percent of its velocity, a cannon's at all of it, and a
/// squad shooting at a ground unit leads it as any gun does.
#[test]
fn a_squads_small_arms_lead_an_aircraft_poorly() {
    let weapon = |record: Weapon, unit: &str| {
        WeaponArms::gun(record, unit, arc(0, FREE, FREE), 1, false, GUN_NPC, 0)
            .unwrap()
            .0
    };
    let rifle = weapon(smlarms(), "TROOPS");
    let cannon = weapon(zsu23(), "ZSU23");
    let (position, velocity) = ([0., 500., 2_000.], [0., 0., 675.]);
    let seen =
        |weapon: &WeaponArms, surface: bool| observation(weapon, surface, position, velocity, 0.5);
    let flat = seen(&cannon, false);
    assert_eq!(flat.velocity, velocity);
    assert!((flat.position[2] - (2_000. + 675. * 0.5)).abs() < 1e-9);
    let poor = seen(&rifle, false);
    let share = control::SMALL_ARMS_LEAD;
    assert!((poor.velocity[2] - 675. * share).abs() < 1e-9);
    assert!((poor.position[2] - (2_000. + 675. * 0.5 * share)).abs() < 1e-9);
    assert!(
        share > 0. && share < 0.5,
        "a poor lead, not none and not good"
    );
    assert_eq!(
        seen(&rifle, true).velocity,
        velocity,
        "a ground target is led in full"
    );
}

#[test]
fn an_m1939_swaps_in_60_s_and_a_ks19_opens_with_eight_shells() {
    let gun = placed(ZSU_ID, "M1939.NT", 0x0800, 0., 0., 1)
        .with_gun(m1939(), "M1939", arc(0, FREE, [0., 90.]), 0)
        .radar(false, None);
    // A visual gun: no radar, no emitter.
    assert!(gun.arms.radar.is_none());
    let mut f = Fixture::new(vec![gun], Vec::new());
    f.jet.position = [0., 1_000., 3_000.];
    f.run(400 * TPS);
    let reload = f.first(Phase::Reload).unwrap();
    let swap = f
        .traces
        .iter()
        .find(|(_, t)| matches!(t, Trace::Swap { .. }))
        .unwrap()
        .0;
    assert_eq!(swap - reload, 60 * TPS);

    let flak = placed(ZSU_ID, "KS19.NT", 0x0800, 0., 0., 1)
        .with_gun(ks19(), "KS19", arc(0, FREE, [0., 90.]), 0)
        .radar(false, None);
    let mut f = Fixture::new(vec![flak], Vec::new());
    // Below the 4,000 ft floor: never fires.
    f.jet.position = [0., 3_000., 12_000.];
    f.run(60 * TPS);
    assert_eq!(f.rounds(), 0);
    f.jet.position = [0., 15_000., 12_000.];
    f.run(40 * TPS);
    let opening: Vec<&Trace> = f
        .fired
        .iter()
        .map(|(_, t)| t)
        .filter(|t| matches!(t, Trace::Shot { opening: true, .. }))
        .collect();
    assert_eq!(opening.len(), 8);
    assert!(opening.iter().all(|t| matches!(
        t,
        Trace::Shot {
            flak: true,
            rounds: 1,
            ..
        }
    )));
}

#[test]
fn radar_turns_off_30_s_after_the_last_hostile_and_a_harm_shuts_it_down() {
    let mut f = Fixture::new(vec![sa6_site(3)], Vec::new());
    f.run(10 * TPS);
    assert!(f.emitting(SA6_ID));
    f.jet.position = [0., 15_000., 200_000.];
    f.run(30 * TPS - 1);
    assert!(f.emitting(SA6_ID));
    f.run(2);
    assert!(!f.emitting(SA6_ID));

    // An ace rolls 90 percent: of 20 HARMs inside 10 nm, most shut it down.
    let mut shut = 0;
    for n in 0..20 {
        let mut f = Fixture::new(vec![sa6_site(3)], Vec::new());
        f.state.rng = n;
        f.run(2 * TPS);
        launch_harm(&mut f, [0., 10_000., 40_000.]);
        f.step();
        if !f.emitting(SA6_ID) {
            shut += 1;
            // Off for 30 s, then back on while the jet is in range.
            f.run(30 * TPS - 2);
            assert!(!f.emitting(SA6_ID));
            f.run(2);
            assert!(f.emitting(SA6_ID));
        }
    }
    assert!(shut >= 14, "{shut} of 20");
    // A novice never does; nor does anyone for a HARM beyond 10 nm.
    let mut f = Fixture::new(vec![sa6_site(0)], Vec::new());
    f.run(2 * TPS);
    launch_harm(&mut f, [0., 10_000., 40_000.]);
    f.step();
    assert!(f.emitting(SA6_ID));
    let mut f = Fixture::new(vec![sa6_site(3)], Vec::new());
    f.run(2 * TPS);
    launch_harm(&mut f, [0., 10_000., 90_000.]);
    f.step();
    assert!(f.emitting(SA6_ID));
    assert!(f.state.harm_rolled.is_empty());
}

/// An AGM-88 in flight at the SA-6 from `from`: fired through the surface
/// path only to put a real emitter-homing projectile in the air.
fn launch_harm(f: &mut Fixture, from: Vector) {
    let id = f
        .live
        .fire_surface(SurfaceShot {
            owner: 0x4100_0000,
            weapon: agm88(),
            mount: 0,
            position: from,
            direction: [0., -0.2, -1.],
            velocity: [0., 0., -800.],
            target: Some(SA6_ID),
            observation: None,
            ordinal: 0,
            end_tick: None,
        })
        .unwrap();
    assert!(f.live.projectiles.iter().any(|p| p.id == id));
}

#[test]
fn an_infrared_sam_locks_without_emitting_and_guns_never_sound_a_lock() {
    let sa7 = placed(SA6_ID, "SA7.NT", 0x1000, 0., 0., 1)
        .with_missile(sa7(), &[(arc(0, FREE, FREE), 2)])
        .radar(false, None);
    assert!(sa7.arms.radar.is_none());
    let mut f = Fixture::new(vec![sa7], Vec::new());
    f.jet.position = [0., 3_000., 6_000.];
    f.run(45 * TPS);
    assert_eq!(f.rounds(), 1);
    assert!(!f.emitting(SA6_ID));
    assert_eq!(f.state.locks_on(JET), vec![2]);
    assert!(f.state.painting(JET).is_empty());
    let zsu = placed(ZSU_ID, "ZSU23.NT", 0x0800, 0., 0., 1)
        .with_gun(zsu23(), "ZSU23", arc(0, FREE, [0., 90.]), 0)
        .radar(false, None);
    let mut f = Fixture::new(vec![zsu], Vec::new());
    f.jet.position = [0., 1_000., 3_000.];
    f.run(25 * TPS);
    assert!(f.rounds() > 0);
    assert!(f.state.locks_on(JET).is_empty());
    assert_eq!(f.state.painting(JET), vec![ZSU_ID]);
}

const RADAR: u32 = 0x4000_0010;
const NEAR: u32 = 0x4000_0011;
const FAR: u32 = 0x4000_0012;

/// A battery: a radar at the origin, a launcher 800 ft east of it (skill 1)
/// and one 4,000 ft south (skill 2), three rails each.
fn battery(system: BatterySystem) -> Fixture {
    let missile = if system == BatterySystem::Sa2 {
        sa2()
    } else {
        sa6()
    };
    let resource = system.launcher();
    let radar = placed(RADAR, "SFLUSH.NT", 0x0200, 0., 0., 1).radar(true, Some(true));
    let near = placed(NEAR, resource, 0x1000, 800., 0., 1)
        .with_missile(missile.clone(), &[(arc(0, FREE, [0., 70.]), 3)])
        .radar(false, Some(false));
    let far = placed(FAR, resource, 0x1000, 0., -4_000., 2)
        .with_missile(missile, &[(arc(0, FREE, [0., 70.]), 3)])
        .radar(false, Some(false));
    Fixture::new(
        vec![radar, near, far],
        vec![Battery {
            system,
            side: RED,
            radar: UnitId(RADAR),
            radar_added: false,
            launchers: vec![UnitId(NEAR), UnitId(FAR)],
            truck: None,
        }],
    )
}

#[test]
fn a_battery_detects_from_its_radar_fires_the_nearest_launcher_and_the_radar_supports() {
    let mut f = battery(BatterySystem::Sa6);
    f.jet.position = [0., 15_000., 40_000.];
    f.run(60 * TPS);
    // The radar is the only emitter.
    assert!(f.emitting(RADAR));
    assert!(!f.emitting(NEAR) && !f.emitting(FAR));
    // The battery's best skill (veteran, x0.85) on the launcher's unready 36 s,
    // then 5 s of lock.
    let first = f.fired.first().unwrap();
    assert_eq!(first.0, (36 * TPS * 85 / 100) + 5 * TPS);
    // The nearest launcher with a rail, sight and the jet in its zone fires.
    assert_eq!(f.shooters()[0], NEAR);
    // Its missile is the launcher's, supported from the radar's position.
    let support = f.supports.iter().find(|s| s.owner == NEAR).unwrap();
    assert!(support.supported && support.radar_emitting);
    assert_eq!(support.radar_position, [0., EYE_HEIGHT_FT, 0.]);
    // The lock tone and painting are the radar's.
    assert_eq!(f.state.locks_on(JET), vec![3]);
    assert_eq!(f.state.painting(JET), vec![RADAR]);
    // A ridge hiding the near launcher, not the radar or the far one: the
    // far one fires next.
    let before = f.shooters().len();
    f.ridge = Some(Ridge {
        x: (600., 1_000.),
        z: (1_000., 3_000.),
        height: 3_000.,
    });
    f.run(60 * TPS);
    let after = f.shooters();
    assert!(after.len() > before);
    assert!(after[before..].iter().all(|id| *id == FAR));
}

#[test]
fn a_dead_radar_blinds_an_sa6_battery_and_its_missiles_lose_support() {
    let mut f = battery(BatterySystem::Sa6);
    f.jet.position = [0., 15_000., 40_000.];
    f.run(40 * TPS);
    assert_eq!(f.rounds(), 1);
    f.kill(RADAR);
    f.run(TPS);
    assert_eq!(f.state.batteries[0].controller.phase(), Phase::Blind);
    assert!(!f.emitting(RADAR));
    assert!(f.state.locks_on(JET).is_empty());
    f.run(120 * TPS);
    assert_eq!(f.rounds(), 1, "a blind SA-6 battery never launches");
}

#[test]
fn a_blind_sa2_battery_launches_optically_in_daylight_only() {
    // Daylight: inside half range and 10 nm, prepared twice as long, with no
    // lock tone and no emitter.
    let mut f = battery(BatterySystem::Sa2);
    f.kill(RADAR);
    f.jet.position = [0., 15_000., 30_000.];
    f.run(120 * TPS);
    assert!(f.state.batteries[0].optical);
    assert!(f.rounds() > 0);
    let first = f.fired.first().unwrap().0;
    assert_eq!(first, 2 * (36 * TPS * 85 / 100) + 5 * TPS);
    assert!(f.state.locks_on(JET).is_empty());
    assert!(f.state.painting(JET).is_empty());
    assert!(!f.emitting(RADAR) && !f.emitting(NEAR));
    let support = f.supports.iter().find(|s| s.owner == NEAR).unwrap();
    assert!(support.supported && !support.radar_emitting);
    // Beyond half range: nothing.
    let mut f = battery(BatterySystem::Sa2);
    f.kill(RADAR);
    f.jet.position = [0., 15_000., 60_000.];
    f.run(120 * TPS);
    assert_eq!(f.rounds(), 0);
    // At night: blind.
    let mut f = battery(BatterySystem::Sa2);
    f.kill(RADAR);
    f.daylight = false;
    f.jet.position = [0., 15_000., 30_000.];
    f.run(120 * TPS);
    assert_eq!(f.rounds(), 0);
    assert_eq!(f.state.batteries[0].controller.phase(), Phase::Blind);
}

#[test]
fn a_barrage_zone_wakes_within_its_radius_and_fires_a_third_of_its_bursts() {
    let zone_unit = placed(ZSU_ID, "A_M1939.NT", 0x0800, 0., 0., 1)
        .with_gun(a_m1939(), "A_M1939", arc(0, FREE, [0., 90.]), 195)
        .radar(false, None);
    assert!(matches!(
        zone_unit.arms.weapons[0].kind,
        Kind::Barrage { chance: 33, .. }
    ));
    let mut f = Fixture::new(vec![zone_unit], Vec::new());
    // Outside 195 x 256 ft: asleep.
    f.jet.position = [0., 15_000., 60_000.];
    f.run(60 * TPS);
    assert_eq!(f.phase(ZSU_ID), Phase::Idle);
    // Inside: a burst every 5.25 s, about a third of them fired.
    f.jet.position = [0., 15_000., 30_000.];
    f.run(1_200 * TPS);
    let bursts = f
        .traces
        .iter()
        .filter(|(_, t)| {
            matches!(
                t,
                Trace::Phase {
                    phase: Phase::Pause,
                    ..
                }
            )
        })
        .count();
    assert!(bursts > 100, "{bursts}");
    let share = f64::from(f.rounds()) / (bursts as f64 * 6.);
    assert!((0.2..0.46).contains(&share), "{share} of {bursts} bursts");
}

#[test]
fn the_surface_state_round_trips_mid_fight() {
    let mut f = battery(BatterySystem::Sa6);
    f.jet.position = [0., 15_000., 40_000.];
    f.run(38 * TPS);
    let copy = round_trip(&f.state, &Models::default()).expect("it round-trips");
    assert_eq!(copy, f.state);
    assert!(copy.places.is_empty() && copy.locks.is_empty());
    assert!(copy.batteries.iter().any(|b| b != &BatteryState::default()));
    assert!(
        copy.units
            .iter()
            .any(|u| u.engagers.iter().any(|e| e != &Engager::default())
                || u.radar != Default::default())
    );
    // A restart clears everything back to an unarmed start.
    let mut reset = f.state.clone();
    reset.reset();
    assert_eq!(reset, f.surface.fresh_state());
}

#[test]
fn a_moving_unit_fires_from_where_it_is_with_its_mount_turned() {
    use crate::surface::movement::{Halt, Mover};
    let zsu = placed(ZSU_ID, "ZSU23.NT", 0x0800, 0., 0., 1)
        .with_gun(zsu23(), "ZSU23", arc(0, FREE, [0., 90.]), 0)
        .radar(false, None);
    let mut f = Fixture::new(vec![zsu], Vec::new());
    // The unit has driven 2,000 ft east and turned to head east (90
    // degrees), moving at 50 ft/s.
    let fine = 65_536.;
    f.state.unit_mut(UnitId(ZSU_ID)).unwrap().mover = Some(Mover {
        x: (2_000. * fine) as i64,
        y: 0,
        z: 0,
        heading: i32::MAX / 2 + 1,
        pitch: 0,
        bank: 0,
        speed: (50. * fine) as i32,
        leg: 1,
        halt: Halt::Moving,
    });
    f.jet.position = [2_000., 1_000., 3_000.];
    f.run(22 * TPS);
    assert!(f.rounds() > 0);
    let place = f.state.place(&f.surface.arsenal.units[0]);
    assert!((place.origin[0] - 2_000.).abs() < 1e-6);
    assert!((place.velocity[0] - 50.).abs() < 1e-6);
    // A mount 3 ft forward of the hull centre sits 3 ft east once the hull
    // heads east.
    let mount = MountArc {
        offset: [0., 10., 3.],
        ..f.surface.arsenal.units[0].weapons[0].mounts[0]
    };
    let at = place.mount(&mount, &|_, _| 0.);
    assert!(
        (at[0] - 2_003.).abs() < 1e-6 && at[2].abs() < 1e-6,
        "{at:?}"
    );
    assert!((at[1] - 10.).abs() < 1e-6);
    // A mount at ground level is lifted clear of the terrain.
    let low = MountArc {
        offset: [0., 0., 0.],
        ..mount
    };
    assert!((place.mount(&low, &|_, _| 0.)[1] - MUZZLE_CLEARANCE_FT).abs() < 1e-6);
}

#[test]
fn a_land_guns_shell_leaves_the_end_of_its_barrel_toward_the_target() {
    let place = Place {
        origin: [100., 5., 200.],
        eye: [100., 20., 200.],
        basis: Basis::new(0., 0., 0.),
        heading: 0.,
        velocity: [0.; 3],
    };
    let hardpoint = arc(0, FREE, FREE);
    // No barrel: the hardpoint, 10 ft up, whatever the bearing.
    let at = place.muzzle(&hardpoint, 1.0, &|_, _| 0.);
    assert_eq!(at, [100., 15., 200.]);
    // A barrel ending 20 ft forward, 3 ft right and 25 ft up: forward is the
    // bearing the gun has turned to, so facing east it is 20 ft east.
    let gun = MountArc {
        barrel: Some([3., 25., 20.]),
        ..hardpoint
    };
    let east = place.muzzle(&gun, std::f64::consts::FRAC_PI_2, &|_, _| 0.);
    assert!((east[0] - 120.).abs() < 1e-9 && (east[2] - 197.).abs() < 1e-9);
    assert!((east[1] - 30.).abs() < 1e-9);
    let north = place.muzzle(&gun, 0., &|_, _| 0.);
    assert!((north[0] - 103.).abs() < 1e-9 && (north[2] - 220.).abs() < 1e-9);
}

#[test]
fn a_flak_burst_falls_in_the_egg_about_the_target_as_it_will_be_when_the_shell_arrives() {
    let mut record = ks19();
    surface_guns::apply("KS19", &mut record).unwrap();
    let muzzle = [0., 30., 0.];
    let target = TargetObservation {
        position: [3_000., 15_000., 6_000.],
        velocity: [0., 0., 760.],
    };
    for egg in [
        [0., 0., 0.],
        [200., 50., -300.],
        [-400., -120., 250.],
        [100., 160., 450.],
    ] {
        let first = [3_000., 15_000., 12_000.];
        let (burst, ticks) = flak_burst_point(&record, muzzle, target, egg, first);
        let seconds = ticks.expect("in reach") as f64 / 120.;
        // The target, carried on to the moment the shell arrives, stands
        // exactly the egg's offset from where the shell bursts.
        let arrived: Vector =
            std::array::from_fn(|i| target.position[i] + target.velocity[i] * seconds);
        let off = sub(burst, arrived);
        assert!(
            length(sub(off, egg)) < 12.,
            "{egg:?}: burst {burst:?}, target then {arrived:?}"
        );
    }
}

#[path = "supply_tests.rs"]
mod supply_tests;

#[path = "surface_vs_tests.rs"]
mod surface_vs_tests;

/// What the units' pictures aim at (docs/spec/surface-defenses.md,
/// "Aiming"): a unit's controller from choosing a target while it holds it,
/// and every launcher of a battery whose controller does.
#[test]
fn aim_tracks_follow_the_locked_controllers_and_batteries() {
    use crate::surface::aim::{Track, Weapon as Held, tracks};
    let mut f = Fixture::new(vec![sa6_site(1)], Vec::new());
    assert!(
        tracks(&f.surface, &f.state).is_empty(),
        "nothing chosen yet"
    );
    // Searching at once, it chooses the jet and turns toward it while it
    // prepares (36 s), then holds it locked.
    f.run(TPS);
    assert_eq!(f.phase(SA6_ID), Phase::Prepare);
    assert_eq!(tracks(&f.surface, &f.state).len(), 1);
    f.run(41 * TPS);
    assert_eq!(
        tracks(&f.surface, &f.state),
        vec![Track {
            unit: UnitId(SA6_ID),
            weapon: Held::Station(0),
            target: JET,
        }]
    );
    let mut f = battery(BatterySystem::Sa6);
    f.jet.position = [0., 15_000., 40_000.];
    f.run(60 * TPS);
    let held: Vec<(u32, u32)> = tracks(&f.surface, &f.state)
        .iter()
        .map(|t| (t.unit.0, t.target))
        .collect();
    assert_eq!(held, vec![(NEAR, JET), (FAR, JET)]);
}

/// The heavy flak guns' teams (John, 2026-10-14): chained within a nautical
/// mile, one side each; other guns never join.
#[test]
fn flak_guns_within_a_mile_form_a_team_and_chain() {
    let ks = |id: u32, x: f64, z: f64| {
        placed(id, "KS19.NT", 0x0800, x, z, 1).with_gun(ks19(), "KS19", arc(0, FREE, [0., 90.]), 0)
    };
    let mut blue = ks(0x4000_0010, 2_000., 0.);
    blue.arms.side = BLUE;
    let units = [
        ks(0x4000_0001, 0., 0.),
        ks(0x4000_0002, 5_000., 0.),
        // A mile from the second, two from the first: the chain holds.
        ks(0x4000_0003, 10_000., 0.),
        ks(0x4000_0004, 40_000., 0.),
        blue,
        placed(0x4000_0005, "ZSU23.NT", 0x0800, 1_000., 0., 1).with_gun(
            zsu23(),
            "ZSU23",
            arc(0, FREE, [0., 90.]),
            0,
        ),
    ];
    let mut arms: Vec<Arms> = units.iter().map(|p| p.arms.clone()).collect();
    arms.sort_by_key(|a| a.unit);
    let teams = flak_teams(&arms);
    let ids: Vec<Vec<u32>> = teams
        .iter()
        .map(|t| t.guns.iter().map(|g| g.0 & 0xff).collect())
        .collect();
    assert_eq!(ids, vec![vec![1, 2, 3], vec![4], vec![0x10]]);
    assert_eq!(teams[2].side, BLUE);
}

const FLAK_A: u32 = 0x4000_0021;
const FLAK_B: u32 = 0x4000_0022;

/// Two KS-19s 3,000 ft apart, ace crews (short preparation).
fn flak_pair() -> Fixture {
    let ks = |id: u32, x: f64| {
        placed(id, "KS19.NT", 0x0800, x, 0., 3)
            .with_gun(ks19(), "KS19", arc(0, FREE, [0., 90.]), 0)
            .radar(false, None)
    };
    Fixture::new(vec![ks(FLAK_A, 0.), ks(FLAK_B, 3_000.)], Vec::new())
}

/// A flight of `n` at 15,000 ft strung north, the first `inside` of them
/// just inside the guns' reach (2,000 ft apart, coming nearer) and the rest
/// beyond it at 4,000 ft intervals, flying along `velocity`.
fn strung_flight(f: &mut Fixture, n: u32, inside: u32, velocity: [f64; 3]) {
    let reach = f.surface.arsenal.units[0].weapons[0].launch.max_range;
    let altitude = 15_000.;
    let first = ((reach - 2_000.).powi(2) - altitude * altitude).sqrt();
    let at = |k: u32| {
        let z = if k < inside {
            first - 2_000. * f64::from(k)
        } else {
            first + 4_000. * f64::from(k + 1 - inside)
        };
        [0., altitude, z]
    };
    f.jet.position = at(0);
    f.jet.velocity = velocity;
    f.others = (1..n)
        .map(|k| Aircraft {
            id: JET + k,
            position: at(k),
            ..f.jet
        })
        .collect();
}

fn flak_group(f: &Fixture) -> Vec<FlakGroup> {
    f.traces
        .iter()
        .rev()
        .find_map(|(_, t)| match t {
            Trace::FlakTeam { groups, .. } => Some(groups.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[test]
fn a_team_holds_its_fire_on_a_group_skirting_its_reach() {
    // Five aircraft, one in reach of both guns (20 percent), flying east,
    // across the line to the team: the team holds, even on the one it could
    // reach (the ambush rule; John, 2026-10-14).
    let mut f = flak_pair();
    strung_flight(&mut f, 5, 1, [760., 0., 0.]);
    f.run(60 * TPS);
    assert_eq!(f.rounds(), 0, "holds fire");
    let groups = flak_group(&f);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].members.len(), 5);
    assert_eq!(groups[0].in_range, 1);
    assert!(!groups[0].heading_in && !groups[0].engaged);
    // The guns are locked on and ready meanwhile: a group that turns in is
    // fired at at once.
    assert!(f.phase(FLAK_A) == Phase::Track && f.phase(FLAK_B) == Phase::Track);
    f.jet.velocity = [0., 0., -760.];
    for other in &mut f.others {
        other.velocity = f.jet.velocity;
    }
    let turned = f.tick;
    f.run(2 * TPS);
    assert!(flak_group(&f)[0].heading_in && flak_group(&f)[0].engaged);
    let first = f.fired.first().expect("fires once the group heads in").0;
    assert!(
        first - turned <= 2,
        "fires within a tick or two: {}",
        first - turned
    );
}

#[test]
fn a_quarter_of_the_group_in_range_of_the_whole_team_opens_fire() {
    // Eight aircraft, two in reach: 25 percent and the minimum of two,
    // flying east.
    let mut f = flak_pair();
    strung_flight(&mut f, 8, 2, [760., 0., 0.]);
    f.run(60 * TPS);
    let groups = flak_group(&f);
    assert_eq!((groups[0].in_range, groups[0].heading_in), (2, false));
    assert!(groups[0].engaged);
    assert!(f.rounds() > 0);
}

#[test]
fn one_aircraft_of_a_small_flight_in_range_is_not_enough() {
    // John, 2026-10-14: "a minimum of 2 aircraft". One of four in reach is
    // 25 percent but under the minimum: held. One of two likewise.
    for n in [4, 2] {
        let mut f = flak_pair();
        strung_flight(&mut f, n, 1, [760., 0., 0.]);
        f.run(60 * TPS);
        let groups = flak_group(&f);
        assert_eq!((groups[0].in_range, groups[0].engaged), (1, false), "{n}");
        assert_eq!(f.rounds(), 0, "{n}");
    }
    // A lone aircraft in reach is the whole group: engaged.
    let mut f = flak_pair();
    strung_flight(&mut f, 1, 1, [760., 0., 0.]);
    f.run(60 * TPS);
    assert!(flak_group(&f)[0].engaged);
    assert!(f.rounds() > 0);
}

#[test]
fn a_team_spreads_its_fire_over_the_group() {
    // Two aircraft in trail heading at the guns, both in reach, the lead the
    // nearer to both guns: the first gun takes the lead, the second the
    // wingman instead of the nearer lead.
    let mut f = flak_pair();
    f.jet.position = [1_500., 15_000., 20_000.];
    f.jet.velocity = [0., 0., -760.];
    f.others = vec![Aircraft {
        id: JET + 1,
        position: [1_500., 15_000., 22_000.],
        ..f.jet
    }];
    f.run(60 * TPS);
    // After the opening barrages (spread over both, below), each gun keeps
    // to its own.
    let targets: Vec<(u32, u32)> = f
        .fired
        .iter()
        .filter_map(|(_, t)| match t {
            Trace::Shot {
                unit,
                target,
                opening: false,
                ..
            } => Some((unit.0, *target)),
            _ => None,
        })
        .collect();
    for gun in [FLAK_A, FLAK_B] {
        let opening: Vec<u32> = f
            .fired
            .iter()
            .filter_map(|(_, t)| match t {
                Trace::Shot {
                    unit,
                    target,
                    opening: true,
                    ..
                } if unit.0 == gun => Some(*target),
                _ => None,
            })
            .collect();
        assert_eq!(opening.len(), 8);
        // Round the flight, its own target first.
        let first = opening[0];
        let other = if first == JET { JET + 1 } else { JET };
        assert_eq!(opening, [first, other].repeat(4), "{gun:#x}");
    }
    let of = |gun: u32| -> BTreeSet<u32> {
        targets
            .iter()
            .filter(|(u, _)| *u == gun)
            .map(|(_, t)| *t)
            .collect()
    };
    assert!(!of(FLAK_A).is_empty() && !of(FLAK_B).is_empty());
    assert!(
        of(FLAK_A).is_disjoint(&of(FLAK_B)),
        "{:?} {:?}",
        of(FLAK_A),
        of(FLAK_B)
    );
}

#[test]
fn a_team_detects_at_twice_its_guns_range() {
    // The KS-19 sees 100,000 ft; the team 200,000 ft.
    let mut f = flak_pair();
    f.jet.position = [0., 15_000., 150_000.];
    f.jet.velocity = [0., 0., -760.];
    f.run(TPS);
    assert_eq!(flak_group(&f)[0].members, vec![JET]);
    assert_ne!(f.phase(FLAK_A), Phase::Idle);
    f.jet.position = [0., 15_000., 210_000.];
    f.run(TPS);
    assert!(flak_group(&f).is_empty());
}

#[test]
fn a_mixed_team_counts_its_widest_reach() {
    // John, 2026-10-14: in range of the team is inside its longest reach.
    // Gun B reaches only 10,000 ft: two of eight aircraft, inside gun A's
    // reach alone, are a quarter of the group in range of the team.
    let mut f = flak_pair();
    f.surface.arsenal.units[1].weapons[0].launch.max_range = 10_000.;
    strung_flight(&mut f, 8, 2, [760., 0., 0.]);
    f.run(60 * TPS);
    let groups = flak_group(&f);
    assert_eq!((groups[0].in_range, groups[0].heading_in), (2, false));
    assert!(groups[0].engaged);
    assert!(f.shooters().contains(&FLAK_A));
}

#[test]
fn a_lone_guns_barrage_stays_on_a_single_target() {
    let mut f = flak_pair();
    f.kill(FLAK_B);
    f.jet.position = [1_500., 15_000., 20_000.];
    f.jet.velocity = [0., 0., -760.];
    f.run(30 * TPS);
    let opening: Vec<u32> = f
        .fired
        .iter()
        .filter_map(|(_, t)| match t {
            Trace::Shot {
                target,
                opening: true,
                ..
            } => Some(*target),
            _ => None,
        })
        .collect();
    assert_eq!(opening, vec![JET; 8]);
}

/// The ground ceiling only skips sampling: with and without it, a battery
/// and a site under a moving jet and moving ridges do exactly the same.
#[test]
fn the_ground_ceiling_never_changes_what_the_surface_does() {
    let ridges = [
        None,
        Some(Ridge {
            x: (-1e9, 1e9),
            z: (14_700., 15_300.),
            height: 20_000.,
        }),
        Some(Ridge {
            x: (600., 1_000.),
            z: (1_000., 3_000.),
            height: 3_000.,
        }),
        Some(Ridge {
            x: (-2_000., 2_000.),
            z: (8_000., 9_000.),
            height: 6_000.,
        }),
    ];
    let scenarios: [fn() -> Fixture; 3] = [
        || battery(BatterySystem::Sa6),
        || battery(BatterySystem::Sa2),
        || Fixture::new(vec![sa6_site(1)], Vec::new()),
    ];
    for scenario in scenarios {
        let mut with = scenario();
        let mut without = scenario();
        without.ceiling = false;
        for step in 0..48u32 {
            let ridge = ridges[(step / 6) as usize % ridges.len()];
            let at = f64::from(step);
            let position = [
                (at * 0.7).sin() * 6_000.,
                1_500. + (at * 0.3).cos().abs() * 18_000.,
                5_000. + at * 900.,
            ];
            for f in [&mut with, &mut without] {
                f.ridge = ridge;
                f.jet.position = position;
                f.run(5 * TPS);
            }
            assert_eq!(with.traces, without.traces, "step {step}");
            assert_eq!(with.fired, without.fired);
            assert_eq!(with.supports, without.supports);
        }
        assert!(with.rounds() > 0, "the scenario must fire");
    }
}
