//! The AAA tuning table: how fast each surface gun fires, how many rounds it
//! carries and how long it takes to reload. Spec: docs/spec/surface-defenses.md
//! (section "AAA tuning"), generated from [`TABLE`].
//!
//! John's rule (2026-10-10): AAA and flak fire real physical shells, every gun
//! type has a fitted rate of fire, magazine and reload time, and the numbers
//! start from the retail record where it has them and from real-world figures
//! where it does not. The retail LIB gives every surface gun a short burst
//! and a pause (a ZSU-23 fires 4 rounds in a quarter second, then waits a
//! second) and an unlimited stock; the table gives each a real cyclic rate, a
//! longer burst, a magazine, and a magazine reload of one minute for towed
//! guns and small vehicles and two for self-propelled AA guns and ship guns.
//!
//! Like [`super::gunship::apply_tore_record`] for the AC-130, [`apply`] writes
//! the table onto a loaded record and leaves every other record alone. The
//! record fields it sets are the ones the projectile code already reads:
//!
//! - `game_rounds_in_burst` and `actual_rounds_per_game` (a burst of
//!   `burst` physical rounds is `burst / per_game` game rounds of `per_game`
//!   physical rounds each) and `game_burst_t` (quarter seconds), which give
//!   the cyclic rate while a burst is firing;
//! - `reload_t` (quarter seconds), the pause after a burst, and
//!   `startup_shots`, the opening barrage;
//! - `initial_speed`, `final_speed` and the speed limits, for the muzzle
//!   velocity.
//!
//! **Damage.** A game round does the record's damage (a quarter-second burst
//! of 4 rounds in retail does 4 times it). The table fires more physical
//! rounds a second than retail does, so each game round is split over
//! `per_game` physical rounds: `projectile_damage` in live.rs gives each
//! physical round `damage / per_game` (the remainder spread over the rounds
//! of a game round, so the average is exact) when the shooter sets the
//! round's index within its game round (`ordinal % per_game`, as
//! live.rs does for aircraft guns). Damage per second of sustained fire then
//! matches retail, within the tolerance [`GunTuning::damage_rate_ratio`]
//! reports. Where the table's rate is close to retail, `per_game` is 1 and
//! the retail damage per round stays.
//!
//! **Magazine.** The table's `magazine` is the physical rounds a mount fires
//! before it must reload, and `magazine_reload_s` the time that reload
//! takes; retail stock stays unlimited (`maxItems` 32767). The surface unit
//! state (controller slice W3) holds rounds left and the reload deadline, and
//! a supply truck within 0.1 mile restores a magazine at once or on the same
//! clock (also W3).
//!
//! Provenance (each value, see [`GunTuning::origins`]): *retail* where the
//! number is the LIB record's, *fitted* where an agent chose it from real-world
//! figures, and *opinionated* for the magazine reload times, which John chose.
use super::{axial_speed, commanded_speed, engine_phase, gun_round::service_ticks, launch_speed};
use tore_formats::weapons::Weapon;

/// Where a table value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The retail LIB record's own value.
    Retail,
    /// An agent's choice, after real-world figures, recorded here.
    Fitted,
    /// John's choice (2026-10-10).
    Opinionated,
}
impl Origin {
    /// The one-letter tag the spec table prints.
    pub const fn tag(self) -> &'static str {
        match self {
            Origin::Retail => "R",
            Origin::Fitted => "F",
            Origin::Opinionated => "O",
        }
    }
}

/// The origin of each tuned value of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origins {
    pub rate: Origin,
    pub burst: Origin,
    pub pause: Origin,
    pub opening: Origin,
    pub magazine: Origin,
    pub reload: Origin,
    pub muzzle: Origin,
    pub damage: Origin,
}

/// What kind of mount fires the gun, which sets the magazine reload time
/// (John, 2026-10-10: one minute for towed guns and small vehicles, two for
/// self-propelled AA guns and ship guns).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mount {
    /// A fixed, towed gun: M1939, KS-12, KS-19 and the barrage zone.
    Towed,
    /// A squad of infantry.
    Infantry,
    /// A tank, IFV or APC.
    Vehicle,
    /// A self-propelled AA gun (SPAAG): Shilka, Tunguska, ZSU-57, M163.
    Spaag,
    /// A ship's gun or CIWS.
    Ship,
}
impl Mount {
    /// Seconds a magazine reload takes.
    pub const fn magazine_reload_s(self) -> u32 {
        match self {
            Mount::Towed | Mount::Infantry | Mount::Vehicle => 60,
            Mount::Spaag | Mount::Ship => 120,
        }
    }
}

impl Mount {
    /// Spare magazines a mount carries beyond the loaded one (fitted, spec
    /// "AAA tuning"): two for land guns, unlimited (`None`) for ships, which
    /// are never resupplied by truck.
    pub const fn reserve_magazines(self) -> Option<u32> {
        match self {
            Mount::Ship => None,
            _ => Some(2),
        }
    }
    /// The reload class the spec table prints.
    pub const fn label(self) -> &'static str {
        match self {
            Mount::Towed => "towed",
            Mount::Infantry => "troops",
            Mount::Vehicle => "vehicle",
            Mount::Spaag => "self-propelled AAA",
            Mount::Ship => "ship",
        }
    }
}

/// The retail LIB record's firing fields for the gun (survey 3.2, read from
/// the shipped JT files; the ignored `real_data` test checks them against the
/// LIB). Times are quarter seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retail {
    pub rounds_in_burst: u8,
    pub rounds_per_game: u8,
    pub burst_t: u8,
    pub reload_t: u8,
    pub startup_shots: u8,
    pub initial_speed: i16,
    pub final_speed: i16,
    pub remove_t: u16,
}

/// One row of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GunTuning {
    /// The gun record, as the NT names it (`ZSU23.JT`).
    pub record: &'static str,
    /// Set for a row that applies to one unit only, where one record is
    /// shared by clearly different real guns (the M163 fires PHALANX.JT but
    /// is a 20 mm M168 Vulcan, not a ship's M61A1 Phalanx).
    pub unit: Option<&'static str>,
    /// The real gun the row is fitted to.
    pub gun: &'static str,
    pub mount: Mount,
    /// The NTs that fire the record (survey 2.3 to 2.5), for the spec table
    /// and the coverage test.
    pub units: &'static [&'static str],
    /// Cyclic rate in rounds a minute while a burst fires. A single-round
    /// burst has no cyclic rate, so this is its sustained rate, one round
    /// per loading cycle.
    pub rounds_per_minute: u32,
    /// Physical rounds a game round (of the record's damage) is split into.
    pub per_game: u8,
    /// Physical rounds in one burst, a multiple of `per_game`.
    pub burst: u16,
    /// The pause after a burst, in quarter seconds (the record's `reloadT`).
    pub pause_quarters: u16,
    /// Rounds of the opening barrage (the record's `startupShots`).
    pub opening_shots: u8,
    /// Physical rounds a mount fires before a magazine reload.
    pub magazine: u32,
    /// Seconds the magazine reload takes.
    pub magazine_reload_s: u32,
    /// Muzzle velocity in feet a second.
    pub muzzle_fps: i16,
    /// Whether the gun's rounds carry tracers; flak shells have none.
    pub tracer: bool,
    /// Replacement detection and fire ranges, where the record's are not the
    /// real gun's (opinionated); `None` keeps the record's zones.
    pub zones: Option<Zones>,
    pub retail: Retail,
}

/// Replacement ranges for a gun record's seeker zones, in feet (opinionated,
/// John 2026-10-13): the detection range (`zone0` maximum), and the fire
/// range and ceiling (`zone1` maximum range and maximum altitude). Minimums
/// and the other bounds stay the record's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zones {
    pub detection_ft: i32,
    pub fire_ft: i32,
    pub fire_altitude_ft: i32,
}

const fn retail(
    rounds_in_burst: u8,
    burst_t: u8,
    reload_t: u8,
    startup_shots: u8,
    initial_speed: i16,
    final_speed: i16,
    remove_t: u16,
) -> Retail {
    Retail {
        rounds_in_burst,
        rounds_per_game: 1,
        burst_t,
        reload_t,
        startup_shots,
        initial_speed,
        final_speed,
        remove_t,
    }
}

/// Every surface gun with a row. Rates, bursts, magazines and muzzle velocities
/// are fitted after the real weapons named in `gun`; the retail pause and
/// opening barrage are kept wherever they fit. Order is the order of the spec
/// table: self-propelled AA guns, ship guns, towed guns and flak, vehicles.
pub const TABLE: &[GunTuning] = &[
    GunTuning {
        record: "ZSU23.JT",
        unit: None,
        gun: "ZSU-23-4 Shilka, 4 x 23 mm 2A7",
        mount: Mount::Spaag,
        units: &["ZSU23"],
        rounds_per_minute: 3_400,
        per_game: 11,
        burst: 99,
        pause_quarters: 4,
        opening_shots: 0,
        magazine: 2_000,
        magazine_reload_s: 120,
        muzzle_fps: 3_180,
        tracer: true,
        zones: None,
        retail: retail(4, 1, 4, 0, 3_666, 3_666, 20),
    },
    GunTuning {
        record: "2S6.JT",
        unit: None,
        gun: "2S6 Tunguska, 2 x 30 mm 2A38M",
        mount: Mount::Spaag,
        units: &["2S6"],
        rounds_per_minute: 5_000,
        per_game: 15,
        burst: 105,
        pause_quarters: 4,
        opening_shots: 0,
        magazine: 1_904,
        magazine_reload_s: 120,
        muzzle_fps: 3_150,
        tracer: true,
        zones: None,
        retail: retail(4, 1, 4, 0, 3_666, 1_466, 20),
    },
    GunTuning {
        record: "PHALANX.JT",
        unit: Some("M163"),
        gun: "M163 VADS, 20 mm M168 Vulcan",
        mount: Mount::Spaag,
        units: &["M163"],
        rounds_per_minute: 3_000,
        per_game: 5,
        burst: 50,
        pause_quarters: 4,
        opening_shots: 0,
        magazine: 1_100,
        magazine_reload_s: 120,
        muzzle_fps: 3_380,
        tracer: true,
        zones: Some(Zones {
            detection_ft: 18_000,
            fire_ft: 7_500,
            fire_altitude_ft: 7_500,
        }),
        retail: retail(6, 1, 4, 0, 3_666, 1_466, 40),
    },
    GunTuning {
        record: "ZSU57.JT",
        unit: None,
        gun: "ZSU-57-2, twin 57 mm S-68",
        mount: Mount::Spaag,
        units: &["ZSU57", "ZIF31"],
        rounds_per_minute: 240,
        per_game: 1,
        burst: 5,
        pause_quarters: 12,
        opening_shots: 0,
        magazine: 300,
        magazine_reload_s: 120,
        muzzle_fps: 3_280,
        tracer: true,
        zones: None,
        retail: retail(4, 2, 12, 0, 3_666, 3_666, 20),
    },
    GunTuning {
        record: "PHALANX.JT",
        unit: None,
        gun: "Phalanx CIWS, 20 mm M61A1",
        mount: Mount::Ship,
        units: &["NIMZ", "KITT", "CLEM", "WASP", "IOWA", "TICON"],
        rounds_per_minute: 4_500,
        per_game: 10,
        burst: 150,
        pause_quarters: 4,
        opening_shots: 0,
        magazine: 1_550,
        magazine_reload_s: 120,
        muzzle_fps: 3_600,
        tracer: true,
        zones: None,
        retail: retail(6, 1, 4, 0, 3_666, 1_466, 40),
    },
    GunTuning {
        record: "AAA30.JT",
        unit: None,
        gun: "AK-630 class, 30 mm six-barrel",
        mount: Mount::Ship,
        units: &["KIROV", "SOVR", "KIEV", "SARAN", "BUTLER"],
        rounds_per_minute: 4_000,
        per_game: 15,
        burst: 150,
        pause_quarters: 4,
        opening_shots: 0,
        magazine: 2_000,
        magazine_reload_s: 120,
        muzzle_fps: 2_950,
        tracer: true,
        zones: None,
        retail: retail(4, 1, 4, 0, 3_666, 3_666, 20),
    },
    GunTuning {
        record: "AAA30BAD.JT",
        unit: None,
        gun: "AK-230 class, twin 30 mm",
        mount: Mount::Ship,
        units: &[
            "TYPE69", "KNOX", "JIANC", "JIANE", "KRIVAK", "CYCL", "PMORN",
        ],
        rounds_per_minute: 2_000,
        per_game: 6,
        burst: 42,
        pause_quarters: 4,
        opening_shots: 0,
        magazine: 1_000,
        magazine_reload_s: 120,
        muzzle_fps: 3_440,
        tracer: true,
        zones: None,
        retail: retail(4, 1, 4, 0, 3_666, 3_666, 20),
    },
    GunTuning {
        record: "M1939.JT",
        unit: None,
        gun: "61-K 37 mm M1939",
        mount: Mount::Towed,
        units: &["M1939"],
        rounds_per_minute: 160,
        per_game: 1,
        burst: 6,
        pause_quarters: 12,
        opening_shots: 0,
        magazine: 200,
        magazine_reload_s: 60,
        muzzle_fps: 2_890,
        tracer: true,
        zones: None,
        retail: retail(4, 2, 12, 0, 3_960, 3_960, 20),
    },
    GunTuning {
        record: "A_M1939.JT",
        unit: None,
        gun: "61-K 37 mm M1939, barrage zone",
        mount: Mount::Towed,
        units: &["A_M1939"],
        rounds_per_minute: 160,
        per_game: 1,
        burst: 6,
        pause_quarters: 12,
        opening_shots: 0,
        magazine: 200,
        magazine_reload_s: 60,
        muzzle_fps: 2_890,
        tracer: true,
        zones: None,
        retail: retail(4, 2, 12, 0, 3_960, 3_960, 20),
    },
    GunTuning {
        record: "KS12.JT",
        unit: None,
        gun: "52-K 85 mm (KS-12) flak",
        mount: Mount::Towed,
        units: &["KS12"],
        rounds_per_minute: 5,
        per_game: 1,
        burst: 1,
        pause_quarters: 50,
        opening_shots: 8,
        magazine: 60,
        magazine_reload_s: 60,
        muzzle_fps: 2_620,
        tracer: false,
        zones: None,
        retail: retail(1, 2, 16, 8, 3_520, 3_520, 40),
    },
    GunTuning {
        record: "KS19.JT",
        unit: None,
        gun: "KS-19 100 mm flak",
        mount: Mount::Towed,
        units: &["KS19"],
        rounds_per_minute: 5,
        per_game: 1,
        burst: 1,
        pause_quarters: 50,
        opening_shots: 8,
        magazine: 60,
        magazine_reload_s: 60,
        muzzle_fps: 2_950,
        tracer: false,
        zones: None,
        retail: retail(1, 2, 16, 8, 4_400, 4_400, 60),
    },
    GunTuning {
        record: "M1.JT",
        unit: None,
        gun: "M256 120 mm tank gun",
        mount: Mount::Vehicle,
        units: &["M1"],
        rounds_per_minute: 6,
        per_game: 1,
        burst: 1,
        pause_quarters: 39,
        opening_shots: 0,
        magazine: 34,
        magazine_reload_s: 60,
        muzzle_fps: 5_866,
        tracer: false,
        zones: None,
        retail: retail(1, 0, 16, 0, 5_866, 1_466, 20),
    },
    GunTuning {
        record: "T72.JT",
        unit: None,
        gun: "2A46 125 mm tank gun",
        mount: Mount::Vehicle,
        units: &["T72", "T80", "T90"],
        rounds_per_minute: 8,
        per_game: 1,
        burst: 1,
        pause_quarters: 29,
        opening_shots: 0,
        magazine: 22,
        magazine_reload_s: 60,
        muzzle_fps: 5_866,
        tracer: false,
        zones: None,
        retail: retail(1, 0, 16, 0, 5_866, 5_866, 20),
    },
    GunTuning {
        record: "BMP2.JT",
        unit: None,
        gun: "2A42 30 mm",
        mount: Mount::Vehicle,
        units: &["BMP2"],
        rounds_per_minute: 300,
        per_game: 5,
        burst: 20,
        pause_quarters: 12,
        opening_shots: 0,
        magazine: 500,
        magazine_reload_s: 60,
        muzzle_fps: 3_150,
        tracer: true,
        zones: None,
        retail: retail(2, 2, 12, 0, 3_666, 1_466, 20),
    },
    GunTuning {
        record: "BTR80.JT",
        unit: None,
        gun: "KPVT 14.5 mm",
        mount: Mount::Vehicle,
        units: &["BTR80"],
        rounds_per_minute: 600,
        per_game: 5,
        burst: 10,
        pause_quarters: 12,
        opening_shots: 0,
        magazine: 500,
        magazine_reload_s: 60,
        muzzle_fps: 3_280,
        tracer: true,
        zones: None,
        retail: retail(2, 2, 12, 0, 3_666, 1_466, 20),
    },
    GunTuning {
        record: "M113.JT",
        unit: None,
        gun: "M2HB .50 cal",
        mount: Mount::Vehicle,
        units: &["M113"],
        rounds_per_minute: 500,
        per_game: 7,
        burst: 21,
        pause_quarters: 12,
        opening_shots: 0,
        magazine: 2_000,
        magazine_reload_s: 60,
        muzzle_fps: 2_910,
        tracer: true,
        zones: None,
        retail: retail(2, 2, 12, 0, 3_666, 1_466, 20),
    },
    GunTuning {
        record: "M2.JT",
        unit: None,
        gun: "M242 25 mm",
        mount: Mount::Vehicle,
        units: &["M2"],
        rounds_per_minute: 200,
        per_game: 4,
        burst: 20,
        pause_quarters: 12,
        opening_shots: 0,
        magazine: 300,
        magazine_reload_s: 60,
        muzzle_fps: 3_600,
        tracer: true,
        zones: None,
        retail: retail(2, 2, 12, 0, 3_666, 1_466, 20),
    },
    GunTuning {
        record: "SMLARMS.JT",
        unit: None,
        gun: "Squad small arms",
        mount: Mount::Infantry,
        units: &["TROOPS"],
        rounds_per_minute: 600,
        per_game: 1,
        burst: 5,
        pause_quarters: 4,
        opening_shots: 0,
        magazine: 1_000,
        magazine_reload_s: 60,
        muzzle_fps: 3_000,
        tracer: true,
        zones: None,
        retail: retail(4, 1, 4, 0, 3_666, 3_666, 20),
    },
];

/// The row for a unit's gun record: a row for that unit and record first,
/// then the record's own row. Names compare without case; a unit may carry
/// its `.NT` suffix.
pub fn tuning(unit: &str, record: &str) -> Option<&'static GunTuning> {
    let unit = unit
        .strip_suffix(".NT")
        .or_else(|| unit.strip_suffix(".nt"))
        .unwrap_or(unit);
    let for_record = || {
        TABLE
            .iter()
            .filter(|row| row.record.eq_ignore_ascii_case(record))
    };
    for_record()
        .find(|row| row.unit.is_some_and(|u| u.eq_ignore_ascii_case(unit)))
        .or_else(|| for_record().find(|row| row.unit.is_none()))
}

/// Every record the table tunes, for the lookup [`super::live::is_gun`] builds.
pub(super) fn records() -> impl Iterator<Item = &'static str> {
    TABLE.iter().map(|row| row.record)
}

/// Whether `record` is a surface gun the table tunes (any row). The shot code
/// treats these records as guns, as it does the aircraft guns.
pub fn is_surface_gun(record: &str) -> bool {
    TABLE
        .iter()
        .any(|row| row.record.eq_ignore_ascii_case(record))
}

/// Whether `record` is a squad's small arms (an infantry mount): the guns
/// whose aim error is wider than any cannon's
/// ([`crate::ai::surface::small_arms_aim_error_deg`]).
pub fn is_small_arms(record: &str) -> bool {
    TABLE
        .iter()
        .any(|row| row.mount == Mount::Infantry && row.record.eq_ignore_ascii_case(record))
}

/// The main battle tanks' gun records: the M1's M256 and the 2A46 the T-72,
/// T-80 and T-90 fire. Each tank NT carries this one gun and no machine gun
/// (survey 2.3 to 2.5). Against aircraft these guns aim with the tank rules
/// ([`crate::ai::surface::tank_air_aim_error_deg`],
/// [`crate::ai::surface::TANK_AIR_LAY_S`], [`TANK_MAX_ELEVATION_DEG`]).
pub const TANK_GUN_RECORDS: [&str; 2] = ["M1.JT", "T72.JT"];

/// The highest a tank gun elevates, degrees above the hull's horizontal
/// plane (opinionated, John, 2026-10-24: "max elevation of fire set to like
/// 20 degrees up"; the M1's M256 elevates to +20 degrees, the T-72's 2A46 to
/// about +14). An aircraft higher than this above the hull is not engaged.
pub const TANK_MAX_ELEVATION_DEG: f64 = 20.;

/// Whether `record` is a main battle tank's gun ([`TANK_GUN_RECORDS`]).
pub fn is_tank_gun(record: &str) -> bool {
    TANK_GUN_RECORDS
        .iter()
        .any(|tank| tank.eq_ignore_ascii_case(record))
}

/// Puts a row of the table on the gun record `weapon` a unit of the NT
/// `unit` fires and returns the row, or leaves the record alone and returns
/// `None` when the table has no row for it (aircraft guns, missiles). The
/// record keeps its retail damage, life, flags and fuze, and its zones unless
/// the row replaces them. The call is
/// idempotent.
pub fn apply(unit: &str, weapon: &mut Weapon) -> Option<&'static GunTuning> {
    let row = tuning(unit, &weapon.source)?;
    let burst = &mut weapon.burst;
    burst.actual_rounds_per_game = row.per_game;
    burst.game_rounds_in_burst = (row.burst / u16::from(row.per_game)).min(255) as u8;
    burst.game_burst_t = row.burst_quarters();
    burst.reload_t = row.pause_quarters.min(255) as u8;
    burst.startup_shots = row.opening_shots;
    let movement = &mut weapon.movement;
    let (before, muzzle) = (movement.initial_speed, row.muzzle_fps);
    // The round keeps the retail record's slowing: its final speed is the same
    // fraction of the muzzle velocity.
    movement.final_speed = if before > 0 {
        (i32::from(movement.final_speed) * i32::from(muzzle) / i32::from(before)) as i16
    } else {
        muzzle
    };
    movement.initial_speed = muzzle;
    // A launch speed is clamped to the record's limits, and several retail
    // records pin both limits to their muzzle velocity, so the limits move with it.
    movement.minimum_speed = movement.minimum_speed.min(movement.final_speed);
    movement.maximum_speed = movement.maximum_speed.max(muzzle);
    movement.corner_speed = movement.corner_speed.max(muzzle);
    if weapon.damage.collateral_radius > 0 && weapon.damage.collateral_percent > 0 {
        // Heavy flak: its splash reaches less far than the retail record says.
        weapon.damage.collateral_radius =
            weapon.damage.collateral_radius.min(FLAK_SPLASH_RADIUS_FT);
    }
    if let Some(zones) = row.zones {
        let seeker = &mut weapon.seeker;
        seeker.zones[0].maximum_range = zones.detection_ft;
        seeker.zones[1].maximum_range = zones.fire_ft;
        seeker.zones[1].maximum_altitude = zones.fire_altitude_ft;
    }
    Some(row)
}

impl GunTuning {
    /// The burst's duration written to `gameBurstT`, in quarter seconds: the
    /// time the burst's rounds take at the cyclic rate. A single round has no
    /// spacing, so its burst time is the one-quarter-second minimum.
    pub fn burst_quarters(&self) -> u8 {
        if self.burst <= 1 {
            return 1;
        }
        let rate = self.rounds_per_minute.max(1) as u64;
        let quarters = (240 * self.burst as u64 + rate / 2) / rate;
        if quarters == 0 {
            1
        } else if quarters > 255 {
            255
        } else {
            quarters as u8
        }
    }
    /// Quarter seconds between the shells of the opening barrage, or 0 for
    /// the burst's own round spacing: [`FLAK_OPENING_GAP_QUARTERS`] for the
    /// flak, the only rows with an opening barrage.
    pub fn opening_gap_quarters(&self) -> u8 {
        if self.opening_shots > 0 {
            FLAK_OPENING_GAP_QUARTERS
        } else {
            0
        }
    }
    /// Seconds from the first round of one burst to the first of the next.
    pub fn cycle_s(&self) -> f64 {
        (f64::from(self.burst_quarters()) + f64::from(self.pause_quarters)) / 4.
    }
    /// Physical rounds a minute over whole burst cycles, pauses included.
    pub fn sustained_rpm(&self) -> f64 {
        f64::from(self.burst) * 60. / self.cycle_s()
    }
    /// How much damage per second of sustained fire the row does relative to
    /// the retail record: 1.0 is the same. Each physical round does
    /// `1 / per_game` of a game round's damage.
    pub fn damage_rate_ratio(&self) -> f64 {
        let retail = &self.retail;
        let retail_cycle = (f64::from(retail.burst_t.max(1)) + f64::from(retail.reload_t)) / 4.;
        let retail_game_rounds = f64::from(retail.rounds_in_burst) / retail_cycle;
        (f64::from(self.burst) / self.cycle_s() / f64::from(self.per_game)) / retail_game_rounds
    }
    /// Where each value comes from: the retail record's where the row repeats
    /// it, otherwise fitted. The reload times are John's.
    pub fn origins(&self) -> Origins {
        let from = |same: bool| if same { Origin::Retail } else { Origin::Fitted };
        let retail = &self.retail;
        Origins {
            rate: Origin::Fitted,
            burst: from(self.burst == u16::from(retail.rounds_in_burst)),
            // The flak guns' pause is John's (2026-10-13): their rate of fire is
            // a third of what it was.
            pause: if self.opening_shots > 0 && self.pause_quarters != u16::from(retail.reload_t) {
                Origin::Opinionated
            } else {
                from(self.pause_quarters == u16::from(retail.reload_t))
            },
            opening: from(self.opening_shots == retail.startup_shots),
            magazine: Origin::Fitted,
            reload: Origin::Opinionated,
            muzzle: from(self.muzzle_fps == retail.initial_speed),
            damage: from(self.per_game == 1),
        }
    }
}

/// Quarter seconds between the shells of a flak gun's opening barrage: 0.75
/// s, three times the retail quarter second, as its rate of fire is a third
/// of what it was (opinionated, John, 2026-10-14: "The 8-shell opening
/// barrage should be slowed as well"; the value is an agent decision, the
/// same cut as the rate). The eight shells take 6 s instead of 2 s.
pub const FLAK_OPENING_GAP_QUARTERS: u8 = 3;

/// Half the width of the "egg" heavy flak bursts in, feet: 150 m, so the egg
/// is 300 m across (John, 2026-10-13: "within a sphere around the target of
/// about 300m wide and roughly 100m in altitude, not really a sphere but an
/// egg"; opinionated).
pub const FLAK_EGG_HALF_WIDTH_FT: f64 = 150. / 0.3048;
/// Half the height of the egg, feet: 50 m, so it is 100 m tall (John,
/// 2026-10-13; opinionated).
pub const FLAK_EGG_HALF_HEIGHT_FT: f64 = 50. / 0.3048;

/// Feet a flak burst's splash damage reaches (fitted, agent decision,
/// 2026-10-13), where the retail record's is 750 ft. The retail reach is
/// wider than the whole egg the bursts fall in, so every burst hurt the
/// target however far it fell from it, and that, with the aim, is what made
/// flak "super strong". 400 ft (120 m) is a little past the real fragment
/// reach of an 85 to 100 mm shell against an aircraft (about 100 m): inside
/// the egg a burst near the target hurts, one far from it does not. The
/// record's percent (35) and its straight-line fall to nothing at the edge
/// are unchanged.
pub const FLAK_SPLASH_RADIUS_FT: i16 = 400;

/// Where a flak shell bursts relative to the point the gun aimed at (the
/// target's predicted position): a point drawn uniformly through the volume
/// of an ellipsoid [`FLAK_EGG_HALF_WIDTH_FT`] wide in the horizontal plane
/// and [`FLAK_EGG_HALF_HEIGHT_FT`] high, widened by `widen` (1 for a clear
/// radar picture). The distribution (uniform in volume) is an agent decision
/// (fitted); `draws` are three uniform numbers in 0 to 1 from the surface's
/// seeded stream, so a flight, its replay and a networked host all agree.
/// Axes are the world's: x, up, z.
pub fn flak_egg_offset(draws: [f64; 3], widen: f64) -> [f64; 3] {
    let [radial, pole, turn] = draws.map(|d| d.clamp(0., 1.));
    // A uniform point in the unit ball: cube-root radius, uniform height on
    // the axis, uniform angle around it.
    let radius = radial.cbrt();
    let up = 2. * pole - 1.;
    let ring = (1. - up * up).max(0.).sqrt();
    let angle = std::f64::consts::TAU * turn;
    [
        radius * ring * angle.cos() * FLAK_EGG_HALF_WIDTH_FT * widen,
        radius * up * FLAK_EGG_HALF_HEIGHT_FT * widen,
        radius * ring * angle.sin() * FLAK_EGG_HALF_WIDTH_FT * widen,
    ]
}

/// A gun record's cyclic rate in rounds a minute, read back from the burst
/// fields the way the projectile code paces rounds: a burst's physical rounds
/// over its burst time, or for a single round the whole loading cycle.
pub fn record_rounds_per_minute(weapon: &Weapon) -> f64 {
    let burst = &weapon.burst;
    let rounds = f64::from(burst.game_rounds_in_burst.max(1))
        * f64::from(burst.actual_rounds_per_game.max(1));
    let quarters = f64::from(burst.game_burst_t.max(1));
    if rounds <= 1. {
        240. / (quarters + f64::from(burst.reload_t))
    } else {
        240. * rounds / quarters
    }
}

/// The straight-line distance a round flies in its life, in feet, under the
/// shot code's speed law (a launch speed, then the record's slowing toward
/// its final speed), as `gunsight::solve_observed` steps it. Gun rounds do not
/// fall (flag 4 is clear on every surface gun).
pub fn reach_ft(weapon: &Weapon) -> f64 {
    let movement = &weapon.movement;
    let Ok(launch) = launch_speed(movement, 0) else {
        return 0.;
    };
    let mut speed_f8 = launch * 256;
    let mut distance = 0.;
    for tick in 0..u64::from(movement.remove_t) * 30 {
        let service = service_ticks(tick);
        if weapon.flags & 0x40 != 0 {
            let phase = engine_phase(movement, (tick / 30) as u16, 0);
            let target = commanded_speed(movement, phase, speed_f8, 0) as i16;
            let Ok(next) = axial_speed(movement, speed_f8, target, false, service) else {
                return 0.;
            };
            speed_f8 = next;
        }
        distance += f64::from(speed_f8) * f64::from(service) / 65_536.;
    }
    distance
}

#[cfg(test)]
mod tests;
