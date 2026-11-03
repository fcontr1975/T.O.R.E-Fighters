//! The mission as data: a Quick Mission written with stable names instead of
//! the creator's list positions, which differ between installs, and its text
//! form, the mission file of docs/DEDICATED-SERVER.md.
//!
//! [`MissionSpec`] is everything [`World::new`](crate::world::World::new)
//! builds a mission from: the theater and weather, the start, the six wings,
//! the AI's orders, the cheats, the flight models and, for a single-player
//! start, the loadout of plane 0. The game's creator turns its draft into
//! one; a server reads one from its mission file and sends the same text to
//! every joining player.
//!
//! The text is one `key value` line each, `#` starts a comment, and the first
//! line is `tore-mission 1`. A parse error names the line. Unknown keys,
//! aircraft, theaters, skills and out-of-range values are refused.

use crate::{
    ai_wings::{AiFlightModel, Preset},
    mission_layout::SEPARATION_NM,
    resources::ResourceSource,
    terrain::{CONDITION_NAMES, Overrides},
};
use std::collections::BTreeMap;
use std::fmt;
use tore_formats::{aircraft::AircraftId, weapons::Weapon};
use tore_sim::{
    ai::{
        AiError, Experience,
        engagement::GroupObjective,
        experience::EnemySkillOverride,
        launch::{
            DUMMY_SKILL, MAX_WING_MEMBERS, Side, WINGS_PER_SIDE, WingId, WingLaunch, WingSelection,
            legacy_pairs, resolve_wings,
        },
    },
    cheats::{Cheats, Damage},
    combat::loadout::Loadout,
};

/// The version in the first line of the text form.
pub const FORMAT_VERSION: u32 = 1;

/// The sixteen theaters the creator offers, by code.
pub const THEATERS: [&str; 16] = [
    "BAL", "CUB", "EGY", "LFA", "FRA", "GRE", "IRA", "KURILE", "TVIET", "SPA", "APA", "PGU", "NSK",
    "WTA", "UKR", "VLA",
];

/// The airborne start altitudes the creator offers, in feet.
pub const ALTITUDES_FT: [u32; 4] = [5_000, 10_000, 20_000, 40_000];

/// The creator's default altitude setting.
pub const DEFAULT_ALTITUDE_FT: u32 = 5_000;

/// A runway object's id is this plus its ordinal in the theater's layout.
pub const RUNWAY_OBJECT_BASE: u32 = 0x4000_0000;

/// Wings in all: friendly 1 to 3, then enemy 1 to 3.
pub const WINGS: usize = 6;

/// The largest wing the creator offers (friendly wing 1 counts the player).
pub const MAX_WING: usize = MAX_WING_MEMBERS;

/// The most planes a mission has: six wings of five. Planes are numbered
/// from 0.
pub const MAX_PLANES: u32 = (WINGS * MAX_WING) as u32;

/// The most stations a loadout lists.
pub const MAX_STATIONS: usize = 64;

/// One of the six weather and time of day choices the creator offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    Clear,
    Cloudy,
    Foggy,
    Dawn,
    Sunset,
    Night,
}

impl Condition {
    pub const ALL: [Self; 6] = [
        Self::Clear,
        Self::Cloudy,
        Self::Foggy,
        Self::Dawn,
        Self::Sunset,
        Self::Night,
    ];

    /// Its place in the recovered source table, which is the order of
    /// [`CONDITION_NAMES`].
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    pub fn name(self) -> &'static str {
        CONDITION_NAMES[self.index()]
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// Where the player's aircraft, and with it the mission, starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// In the air at one of [`ALTITUDES_FT`].
    Airborne { altitude_ft: u32 },
    /// Parked on a runway, by its object id (`RUNWAY_OBJECT_BASE` plus its
    /// ordinal in the theater's layout). `altitude_ft` is the creator's
    /// altitude setting, which a ground start keeps: airborne aircraft of
    /// the mission need it to clear the ground.
    Ground { runway: u32, altitude_ft: u32 },
    /// Parked on a runway the world picks from the ground target (text form
    /// `start ground auto`; surface-defenses spec, "Start placement"). Until
    /// the world places starts from the target it takes the first friendly
    /// runway that fits the wing. `altitude_ft` is as for [`Self::Ground`].
    GroundAuto { altitude_ft: u32 },
}

impl Start {
    /// The creator's altitude setting.
    pub fn altitude_ft(self) -> u32 {
        match self {
            Self::Airborne { altitude_ft }
            | Self::Ground { altitude_ft, .. }
            | Self::GroundAuto { altitude_ft } => altitude_ft,
        }
    }

    /// The start is on the ground, on a chosen runway or one the world picks.
    pub fn on_ground(self) -> bool {
        !matches!(self, Self::Airborne { .. })
    }
}

/// Where Enemy Wing 1, the Redfor flight humans take online, starts: the
/// Redfor mirror of [`Start`] (slice CL-R, John 2026-10-11: "add redfor
/// ground starts, match the quick mission creator styles"). The other enemy
/// wings always start in the air. The altitude setting is [`Start`]'s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EnemyStart {
    /// In the air where the layout puts the enemy: by the ground target, or
    /// the separation from the player without one. Text form: no line, or
    /// `enemy-start airborne`.
    #[default]
    Airborne,
    /// Parked on a runway of a Redfor (or neutral) airfield, by its object
    /// id, as [`Start::Ground`]. Text form `enemy-start ground RUNWAY`.
    Ground { runway: u32 },
    /// Parked on the Redfor airfield the world picks: with a ground target
    /// the Red-side field nearest the target at least 15 nm from it, else
    /// the first Red-side runway that holds the wing. Text form
    /// `enemy-start ground auto`.
    GroundAuto,
}

impl EnemyStart {
    /// The start is on the ground, on a chosen runway or one the world picks.
    pub fn on_ground(self) -> bool {
        self != Self::Airborne
    }

    /// The chosen runway of a ground start.
    pub fn runway(self) -> Option<u32> {
        match self {
            Self::Ground { runway } => Some(runway),
            Self::Airborne | Self::GroundAuto => None,
        }
    }
}

/// How heavily a ground target's anti-aircraft guns or SAM sites are manned:
/// the creator's fields 31 and 32 (retail words "not", "lightly",
/// "moderately" and "heavily"; the text form writes the plain ones). The level
/// is the chance in percent that each slot of the target's template is manned
/// (0, 25, 60 and 100, `quick_template::tables::DEFENSE_PERCENT`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Defense {
    #[default]
    None,
    Light,
    Moderate,
    Heavy,
}

impl Defense {
    pub const ALL: [Defense; 4] = [Self::None, Self::Light, Self::Moderate, Self::Heavy];

    /// The text form's word.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Light => "light",
            Self::Moderate => "moderate",
            Self::Heavy => "heavy",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.name() == word)
    }

    /// The creator's list index: 0 for none up to 3 for heavy.
    pub fn level(self) -> usize {
        self as usize
    }

    pub fn from_level(level: usize) -> Option<Self> {
        Self::ALL.get(level).copied()
    }

    /// The chance in percent that a slot is manned.
    pub fn percent(self) -> u32 {
        tore_formats::quick_template::tables::DEFENSE_PERCENT[self.level()]
    }
}

/// The creator's "Your situation is" setting (field 16; manual p.19): the
/// friendly group holds the advantage (behind the enemy, on its tail), meets
/// it head-on (neutral, the default) or is at a disadvantage (the enemy
/// behind, on the friendly group's tail). It shapes an airborne start with no
/// ground target; see `docs/spec/quick-mission-menu.md#situation`. Text form
/// `situation advantage` or `situation disadvantage`, absent when neutral.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Situation {
    Advantage,
    #[default]
    Neutral,
    Disadvantage,
}

impl Situation {
    /// In the creator's list order (field 16).
    pub const ALL: [Situation; 3] = [Self::Advantage, Self::Neutral, Self::Disadvantage];

    /// The text form's word.
    pub fn name(self) -> &'static str {
        match self {
            Self::Advantage => "advantage",
            Self::Neutral => "neutral",
            Self::Disadvantage => "disadvantage",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == word)
    }

    /// The creator's list index (field 16): 0 advantage, 1 neutral, 2
    /// disadvantage.
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }
}

/// The template stems the creator offers as ground targets in a theater, in
/// menu order after the leading "nothing" entry, or none for an unknown
/// theater. A `~` layout variant offers its base theater's.
pub fn ground_targets(theater: &str) -> &'static [&'static str] {
    use tore_formats::quick_template::tables::TEMPLATES;
    tore_formats::theater::base_theater(theater)
        .and_then(|base| THEATERS.iter().position(|code| *code == base))
        .and_then(|index| TEMPLATES[index].get(1..))
        .unwrap_or(&[])
}

/// The nationality (creator list index) the creator gives the enemy in a
/// theater, the default of [`MissionSpec::enemy_nationality`]. A theater the
/// creator does not offer gets Ukraine's.
pub fn default_enemy_nationality(theater: &str) -> u8 {
    use tore_formats::quick_template::tables::ENEMY_NATIONALITY;
    tore_formats::theater::base_theater(theater)
        .and_then(|base| THEATERS.iter().position(|code| *code == base))
        .map_or(ENEMY_NATIONALITY[14], |index| ENEMY_NATIONALITY[index]) as u8
}

/// How many nationalities the creator lists.
pub const NATIONALITIES: u8 = 60;

/// How good a wing's pilots are. `Dummy` is the creator's training setting:
/// a constant-heading 400-knot target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skill {
    Novice,
    Average,
    Experienced,
    Ace,
    Dummy,
}

impl Skill {
    pub const ALL: [Self; 5] = [
        Self::Novice,
        Self::Average,
        Self::Experienced,
        Self::Ace,
        Self::Dummy,
    ];

    /// The level the launch payload takes: 0 to 3, or 4 for the dummy.
    pub fn level(self) -> i32 {
        match self {
            Self::Novice => Experience::Novice as i32,
            Self::Average => Experience::Average as i32,
            Self::Experienced => Experience::Experienced as i32,
            Self::Ace => Experience::Ace as i32,
            Self::Dummy => DUMMY_SKILL,
        }
    }

    /// The skill for a creator skill field (0 to 4).
    pub fn from_level(level: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|skill| skill.level() == level)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Novice => "novice",
            Self::Average => "average",
            Self::Experienced => "experienced",
            Self::Ace => "ace",
            Self::Dummy => "dummy",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|skill| skill.name() == name)
    }
}

/// One wing: what it flies, how many and how well. Friendly wing 1's count
/// includes the player, whose aircraft is that wing's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WingSpec {
    pub aircraft: AircraftId,
    pub count: usize,
    pub skill: Skill,
}

/// One station of the player's load: the weapon's resource name, the
/// station's capacity and how many it carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationLoad {
    pub weapon: String,
    pub count: u16,
    pub quantity: u16,
}

/// An accepted tank quantity at one original source hardpoint.
#[derive(Clone, Debug, PartialEq)]
pub struct TankLoad {
    pub hardpoint: u8,
    pub tank: String,
    pub quantity: u16,
}
/// The loadout of plane 0 when a player flies it from the start, as the
/// creator's Load Ordnance page leaves it. `None` in a spec means the
/// aircraft's standard load. A server does not use it: a networked plane
/// carries its aircraft's standard load (stage F lets a player choose).
#[derive(Clone, Debug, PartialEq)]
pub struct LoadoutSpec {
    pub fuel_lbs: f64,
    /// The loadout screen's Cheat: any store on any station.
    pub cheat: bool,
    /// One per station of the aircraft, in its station order.
    pub stations: Vec<StationLoad>,
    /// None preserves source defaults in older saved missions; Some(empty)
    /// explicitly removes every tank. New accepted loadouts always use Some.
    pub tanks: Option<Vec<TankLoad>>,
}

impl LoadoutSpec {
    /// What a loadout holds, by name.
    pub fn of(load: &Loadout) -> Self {
        Self {
            tanks: Some(
                load.configuration
                    .tanks
                    .iter()
                    .filter(|s| s.quantity > 0)
                    .filter_map(|s| {
                        s.store.as_ref().map(|store| TankLoad {
                            hardpoint: s.hardpoint as u8,
                            tank: store.source.clone(),
                            quantity: s.quantity,
                        })
                    })
                    .collect(),
            ),
            fuel_lbs: load.fuel_lbs,
            cheat: load.cheat,
            stations: load
                .configuration
                .stations
                .iter()
                .zip(&load.quantities)
                .enumerate()
                .map(|(slot, (station, &quantity))| match load.designator(slot) {
                    // A laser designator pod hangs where a weapon would: the
                    // row names the pod's SEE record and carries none of the
                    // station's weapon (X1, decision 7).
                    Some(pod) => StationLoad {
                        weapon: pod.source.clone(),
                        count: station.count,
                        quantity: 0,
                    },
                    None => StationLoad {
                        weapon: station.weapon.source.clone(),
                        count: station.count,
                        quantity,
                    },
                })
                .collect(),
        }
    }

    /// Puts this load into `base`, the aircraft's standard loadout. A station
    /// that carries another weapon than the standard one has that weapon read
    /// from the resources (`label` tidies its display name, as the game's
    /// loadout screen does for the ones it offers). The result is not yet
    /// validated.
    pub fn apply(
        &self,
        mut base: Loadout,
        resources: &dyn ResourceSource,
        label: Option<&dyn Fn(&mut Weapon)>,
    ) -> crate::WorldResult<Loadout> {
        let old_prefix = base
            .hardpoints
            .iter()
            .filter(|h| h.store.as_deref().is_some_and(|name| name.ends_with(".JT")))
            .count();
        let current = base.configuration.stations.len();
        if self.stations.len() != current
            && self.stations.len() != old_prefix
            && self.stations.len()
                + tore_formats::aircraft::stations::later_weapon_rows(base.aircraft)
                != current
        {
            return Err(format!(
                "the loadout has {} stations but {} has {}",
                self.stations.len(),
                base.aircraft.pt(),
                base.configuration.stations.len()
            )
            .into());
        }
        base.cheat = self.cheat;
        base.fuel_lbs = self.fuel_lbs;
        let mut pods = Vec::new();
        for (index, load) in self.stations.iter().enumerate() {
            let station = &mut base.configuration.stations[index];
            if load.weapon.ends_with(".SEE") {
                // A laser designator pod on this station (decision 7): the
                // station keeps its own weapon, unloaded.
                base.quantities[index] = 0;
                // A retired pod (the Pave Knife) loads as the pod that
                // replaced it (agent decision, 2026-10-16).
                pods.push((
                    index,
                    tore_sim::combat::sight::pod_record(&load.weapon).to_owned(),
                ));
                continue;
            }
            if station.weapon.source != load.weapon {
                let bytes = resources
                    .get(&load.weapon)
                    .ok_or_else(|| format!("missing loadout resource {}", load.weapon))?;
                let mut weapon = Weapon::parse(&load.weapon, bytes)?;
                tore_sim::combat::gunship::apply_tore_record(&mut weapon);
                if let Some(label) = label {
                    label(&mut weapon);
                }
                station.weapon = weapon;
            }
            station.count = load.count;
            base.quantities[index] = load.quantity;
        }
        if let Some(tanks) = &self.tanks {
            base.clear_tanks()?;
            let mut seen = std::collections::BTreeSet::new();
            for tank in tanks {
                if !seen.insert(tank.hardpoint) {
                    return Err("duplicate tank hardpoint".into());
                }
                let station = base
                    .configuration
                    .tanks
                    .iter_mut()
                    .find(|s| s.hardpoint == usize::from(tank.hardpoint))
                    .ok_or("the selected hardpoint cannot carry a tank")?;
                let bytes = resources
                    .get(&tank.tank)
                    .ok_or_else(|| format!("missing tank resource {}", tank.tank))?;
                station.store = Some(tore_sim::combat::live::TankStore::parse(&tank.tank, bytes)?);
                station.quantity = tank.quantity;
            }
            base.configuration.refresh_tanks()?;
        }
        for (index, name) in pods {
            let bytes = resources
                .get(&name)
                .ok_or_else(|| format!("missing loadout resource {name}"))?;
            let pod = tore_sim::combat::sight::DesignatorStore::parse(&name, bytes)?;
            base.load_designator(index, pod)?;
        }
        base.normalize_saved_store_pairs()?;
        Ok(base)
    }
}

impl LoadoutSpec {
    /// The loadout a player chose for one plane of an open mission, put onto
    /// the aircraft's standard load and checked. This is the multiplayer
    /// lobby's rule (EF4, agent decision): the checks are the ones the
    /// single-player Load Ordnance page makes before Fly, which live in
    /// `tore_sim::combat::loadout` and are shared, not copied
    /// ([`Loadout::validate`]: every quantity within its station's capacity
    /// for that weapon, only weapons connected to flight, the fuel within the
    /// tanks, the weight within the maximum take-off weight), plus three the
    /// page makes by its controls, which a message must be checked for:
    ///
    /// - one station for each of the aircraft's, in its order;
    /// - each station's capacity the one the page gives it: the standard
    ///   count for the standard weapon, else the station's capacity for the
    ///   weapon (what selecting it on the page sets);
    /// - with the creator's Guns only, nothing loaded but the gun (the
    ///   single-player build's own refusal, in its words).
    ///
    /// Cheat loading (any store on any station) is refused: a multiplayer
    /// aircraft carries what it really carries unless the King's `loadouts`
    /// setting says `any` ([`LoadoutSpec::check_in`]).
    pub fn check_for_plane(
        &self,
        aircraft: &tore_formats::aircraft::Aircraft,
        resources: &dyn ResourceSource,
        guns_only: bool,
    ) -> crate::WorldResult<Loadout> {
        self.check_under(aircraft, resources, guns_only, false)
    }

    /// [`LoadoutSpec::check_for_plane`] under the loadout rule of `mission`
    /// (stage F phase 2, slice F2-1): its Guns only, and its
    /// [`MissionSpec::cheat_loadouts`], under which a Cheat loading is
    /// allowed and checked by the Load Ordnance page's own cheat rules (each
    /// station's cheat capacity), as single player's page checks it.
    pub fn check_in(
        &self,
        aircraft: &tore_formats::aircraft::Aircraft,
        resources: &dyn ResourceSource,
        mission: &MissionSpec,
    ) -> crate::WorldResult<Loadout> {
        self.check_under(
            aircraft,
            resources,
            mission.guns_only,
            mission.cheat_loadouts,
        )
    }

    fn check_under(
        &self,
        aircraft: &tore_formats::aircraft::Aircraft,
        resources: &dyn ResourceSource,
        guns_only: bool,
        cheat_allowed: bool,
    ) -> crate::WorldResult<Loadout> {
        if self.cheat && !cheat_allowed {
            return Err("Cheat loading is not allowed in a multiplayer game.".into());
        }
        let standard = Loadout::new(aircraft, |name| {
            resources
                .get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing loadout resource {name}")))
        })?;
        let load = self.apply(standard.clone(), resources, None)?;
        // Saved local loads migrate in apply; a live lobby request must
        // already obey the current pair rule, including its layout count.
        // The refusal names pairing only when an odd count on a two-sided
        // point is the cause.
        for (index, (requested, actual)) in self
            .stations
            .iter()
            .zip(&load.configuration.stations)
            .enumerate()
        {
            if requested.count != actual.count {
                return Err(if load.store_pairs(index).valid(requested.count) {
                    "Loadout capacity must match the current loadout."
                } else {
                    "Paired stores must be loaded in pairs."
                }
                .into());
            }
        }
        for (index, requested) in self.stations.iter().enumerate() {
            if !requested.weapon.ends_with(".SEE") && requested.quantity != load.quantities[index] {
                return Err(if load.store_pairs(index).valid(requested.quantity) {
                    "Store quantity must match the current loadout limits."
                } else {
                    "Paired stores must be loaded in pairs."
                }
                .into());
            }
        }
        if let Some(tanks) = &self.tanks {
            for requested in tanks {
                if let Some(t) = load.configuration.tanks.iter().find(|t| {
                    t.hardpoint == usize::from(requested.hardpoint)
                        && t.quantity != requested.quantity
                }) {
                    return Err(
                        if tore_sim::combat::loading::StorePairs::of(t.rack, t.mount)
                            .valid(requested.quantity)
                        {
                            "Tank quantity must match the current loadout limits."
                        } else {
                            "Paired tanks must be loaded in pairs."
                        }
                        .into(),
                    );
                }
            }
        }
        for (index, station) in load.configuration.stations.iter().enumerate() {
            let default = &standard.configuration.stations[index];
            if default.count == 0 && station.count == 0 && load.quantities[index] == 0 {
                continue;
            }
            let expected = if default.count > 0 && station.weapon.source == default.weapon.source {
                i32::from(default.count)
            } else {
                load.capacity(index, &station.weapon)
            };
            if i32::from(station.count) != expected {
                return Err(format!(
                    "Station {} cannot hold {} {}.",
                    index + 1,
                    station.count,
                    station.weapon.source
                )
                .into());
            }
        }
        if guns_only
            && load
                .configuration
                .stations
                .iter()
                .zip(&load.quantities)
                .any(|(s, n)| !load.aircraft.mounts_gun(&s.weapon.source) && *n > 0)
        {
            return Err("Guns only is selected. Unload other weapons or return to setup and change the restriction.".into());
        }
        load.validate()?;
        Ok(load)
    }
}

/// One plane of an open mission, numbered as the game numbers them: plane 0
/// is the lead of friendly wing 1, then every other aircraft in wing order
/// (friendly wings 1 to 3, then enemy wings 1 to 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenPlane {
    pub id: u32,
    pub wing: WingId,
    /// Its place in the wing, from 0 (the lead).
    pub member: u8,
    pub aircraft: AircraftId,
}

/// A Quick Mission with stable names. See the module comment.
#[derive(Clone, Debug, PartialEq)]
pub struct MissionSpec {
    /// The theater's layout code: one of [`THEATERS`], or a `~` layout variant
    /// the game's developer options reach (the text form takes only the
    /// sixteen).
    pub theater: String,
    pub condition: Condition,
    /// The start time, wind and cloud deck that replace the condition's own:
    /// what the game's `TORE_WEATHER_TIME`, `TORE_WIND` and
    /// `TORE_CLOUD_ALTITUDE` resolve to, and the airfields (the redrawn
    /// airports unless single player's `TORE_REDRAWN_AIRPORTS=0`; the text
    /// form always means the redraw). Resolved by the caller, so building
    /// the mission reads no environment variable.
    pub weather: Overrides,
    pub start: Start,
    /// Where Enemy Wing 1 starts; see [`EnemyStart`].
    pub enemy_start: EnemyStart,
    /// The enemy's distance in nautical miles, one of the creator's
    /// [`SEPARATION_NM`].
    pub separation_nm: u32,
    /// Who is on whose tail at an airborne start; see [`Situation`].
    pub situation: Situation,
    /// The AI's standing orders for groups that inherit.
    pub preset: Preset,
    /// The creator's air combat setting: guns only.
    pub guns_only: bool,
    /// Friendly wings 1 to 3, then enemy wings 1 to 3.
    pub wings: [WingSpec; WINGS],
    /// Human-selectable wings; `wings` retains each AI default.
    pub player_choice: [bool; WINGS],
    /// Accepted human aircraft, keyed by stable mission plane number.
    pub plane_aircraft: BTreeMap<u32, AircraftId>,
    pub objectives: [GroupObjective; WINGS],
    /// Each wing must survive for the mission to succeed.
    pub must_survive: [bool; WINGS],
    /// The cheats in force from the first tick.
    pub cheats: Cheats,
    /// The human-flown planes fly the hybrid model (the default); `false` is
    /// the game's `--legacy-flight`.
    pub researched_flight: bool,
    /// The flight model of every AI aircraft.
    pub ai_flight_model: AiFlightModel,
    /// The game's `--enemy-skill`: every enemy wing at one level.
    pub enemy_skill: Option<EnemySkillOverride>,
    /// The game's `--fixture-wings`: straight-flight fixtures instead of AI
    /// wings. A development setting; a server does not use it.
    pub fixture_wings: bool,
    /// The loadout of plane 0; see [`LoadoutSpec`].
    pub loadout: Option<LoadoutSpec>,
    /// QMC: same-type AI members of the player flight share its accepted load.
    /// Absent in authored missions unless explicitly requested.
    pub share_flight_loadout: bool,
    /// The loadouts of an open mission's planes, by plane number, as the
    /// players who hold them chose them in the lobby (stage F, EF4). A plane
    /// with none carries its aircraft's standard load. Single player has
    /// none: it keeps [`Self::loadout`] for plane 0.
    pub plane_loadouts: BTreeMap<u32, LoadoutSpec>,
    /// Friendly fire (stage F phase 2, the King's `friendly-fire`): when
    /// `false` no round damages an aircraft of its shooter's side. Text form
    /// `friendly-fire off`, on when absent; a networked mission's setting,
    /// which single player's build refuses off.
    pub friendly_fire: bool,
    /// The King's `loadouts any` (stage F phase 2): a plane's loadout may be a
    /// Cheat loading, any store on any station. Text form `loadouts any`,
    /// `own` when absent; a networked mission's setting, which single
    /// player's build refuses.
    pub cheat_loadouts: bool,
    /// The creator's friendly ground target (field 30) as the stem of its
    /// template mission, for example `QUCOL`: one of [`ground_targets`] for
    /// the theater. `None` is the creator's "no ground target". Text form
    /// `ground-target QUCOL`, absent when `None`.
    pub ground_target: Option<String>,
    /// How heavily the target's AAA guns (field 31) and SAM sites (field 32)
    /// are manned. With no ground target they set how many enemy SAM sites
    /// and guns stand on Red's side of the front ([`Self::area_defenses`]).
    /// Text form `defenses aaa heavy sam moderate`, written only with a
    /// level above none.
    pub aaa: Defense,
    pub sam: Defense,
    /// The seed of everything the surface layout rolls (which slots are
    /// manned, which units, where they stand). The creator rolls one when
    /// the flight starts; a restart reuses it. 0 is "none drawn yet": the
    /// host draws one when the flight starts. Text form `surface-seed N`,
    /// written when not 0.
    pub surface_seed: u32,
    /// The enemy's nationality (field 20, an index into the creator's list of
    /// 60), which decides the equipment group the target's units are drawn
    /// from. The default is the theater's ([`default_enemy_nationality`]); the
    /// text form writes `enemy-nationality N` only when it differs.
    pub enemy_nationality: u8,
}

impl MissionSpec {
    /// A mission as the creator's defaults set it up: clear weather, an
    /// airborne start at 5,000 feet, the enemy 5 miles away, one friendly
    /// flight of one (the player) and no enemy.
    pub fn new(theater: &str, player: AircraftId) -> Self {
        let wing = |count, skill| WingSpec {
            aircraft: player,
            count,
            skill,
        };
        Self {
            theater: theater.to_owned(),
            condition: Condition::Clear,
            weather: Overrides::default(),
            start: Start::Airborne {
                altitude_ft: DEFAULT_ALTITUDE_FT,
            },
            enemy_start: EnemyStart::Airborne,
            separation_nm: 5,
            situation: Situation::Neutral,
            preset: Preset::Free,
            guns_only: false,
            wings: [
                wing(1, Skill::Average),
                wing(0, Skill::Novice),
                wing(0, Skill::Novice),
                wing(0, Skill::Novice),
                wing(0, Skill::Novice),
                wing(0, Skill::Novice),
            ],
            player_choice: [false; WINGS],
            plane_aircraft: BTreeMap::new(),
            objectives: [GroupObjective::Inherit; WINGS],
            must_survive: [false; WINGS],
            cheats: Cheats::default(),
            researched_flight: true,
            ai_flight_model: AiFlightModel::AllHybrid,
            enemy_skill: None,
            fixture_wings: false,
            loadout: None,
            share_flight_loadout: false,
            plane_loadouts: BTreeMap::new(),
            friendly_fire: true,
            cheat_loadouts: false,
            ground_target: None,
            aaa: Defense::None,
            sam: Defense::None,
            surface_seed: 0,
            enemy_nationality: default_enemy_nationality(theater),
        }
    }

    /// Enemy area defenses (B23, John 2026-10-12): no ground target, and an
    /// AAA or SAM strength above none. The layout scatters the enemy's SAM
    /// sites and guns over Red's side of the front.
    pub fn area_defenses(&self) -> bool {
        self.ground_target.is_none() && (self.aaa != Defense::None || self.sam != Defense::None)
    }

    /// The surface layout rolls something from the seed: a ground target,
    /// or area defenses. Only then is a `surface-seed` drawn.
    pub fn lays_out_surface(&self) -> bool {
        self.ground_target.is_some() || self.area_defenses()
    }

    /// The player's aircraft: friendly wing 1's.
    pub fn player(&self) -> AircraftId {
        self.wings[0].aircraft
    }

    /// The wing at position `index`: friendly 1 to 3, then enemy 1 to 3.
    pub fn wing_id(index: usize) -> WingId {
        let per_side = usize::from(WINGS_PER_SIDE);
        let side = if index < per_side {
            Side::Friendly
        } else {
            Side::Enemy
        };
        WingId::new(side, (index % per_side) as u8).expect("a fixed wing")
    }

    /// The position of a wing in [`MissionSpec::wings`].
    pub fn wing_index(id: WingId) -> usize {
        usize::from(id.index)
            + if id.side == Side::Enemy {
                usize::from(WINGS_PER_SIDE)
            } else {
                0
            }
    }

    /// The runway of a ground start.
    pub fn ground_runway(&self) -> Option<u32> {
        match self.start {
            Start::Ground { runway, .. } => Some(runway),
            Start::Airborne { .. } | Start::GroundAuto { .. } => None,
        }
    }

    /// The start is on the ground and the world picks the runway
    /// (`start ground auto`).
    pub fn ground_auto(&self) -> bool {
        matches!(self.start, Start::GroundAuto { .. })
    }

    /// The aircraft in Enemy Wing 1, which an enemy ground start parks.
    pub fn enemy_wing_size(&self) -> usize {
        self.wings[usize::from(WINGS_PER_SIDE)].count
    }

    /// The enemy's distance in feet.
    pub fn separation_feet(&self) -> f64 {
        f64::from(self.separation_nm) * crate::mission_layout::FEET_PER_NM
    }

    /// The six wings as the launch payload takes them: friendly wing 1 gives
    /// up one slot to the player.
    pub fn wing_selections(&self) -> Vec<WingSelection> {
        self.selections(true)
    }

    /// The six wings for the AI, with the player's slot left out of friendly
    /// wing 1 when `player` flies its lead, and its full count otherwise.
    fn selections(&self, player: bool) -> Vec<WingSelection> {
        self.wings
            .iter()
            .enumerate()
            .map(|(index, wing)| WingSelection {
                wing: Self::wing_id(index),
                aircraft: wing.aircraft,
                count: wing.count.saturating_sub(usize::from(player && index == 0)),
                skill_level: wing.skill.level(),
            })
            .collect()
    }

    /// The six wings resolved into per-member launch rows, with the enemy
    /// skill override applied.
    pub fn wing_launches(&self) -> Result<Vec<WingLaunch>, AiError> {
        resolve_wings(&self.wing_selections(), self.enemy_skill)
    }

    /// [`Self::wing_launches`] for an open mission, where the AI flies every
    /// plane: friendly wing 1 keeps its full count, its lead (plane 0)
    /// included.
    pub fn open_wing_launches(&self) -> Result<Vec<WingLaunch>, AiError> {
        let wings = resolve_wings(&self.selections(false), self.enemy_skill)?;
        if self.plane_aircraft.is_empty() {
            return Ok(wings);
        }
        let mut plane = 0;
        let mut result = Vec::new();
        for wing in wings {
            for member in &wing.members {
                let mut launch = wing.clone();
                launch.members = vec![*member];
                launch.aircraft = self
                    .plane_aircraft
                    .get(&plane)
                    .copied()
                    .unwrap_or(wing.aircraft);
                result.push(launch);
                plane += 1;
            }
        }
        Ok(result)
    }

    /// The aircraft and count pairs the straight-flight fixtures take (no
    /// override: they are not the AI).
    pub fn fixture_pairs(&self) -> Vec<(AircraftId, usize)> {
        resolve_wings(&self.wing_selections(), None)
            .map(|wings| legacy_pairs(&wings))
            .unwrap_or_default()
    }

    /// Aircraft in the player's wing, the player included.
    pub fn player_wing_size(&self) -> usize {
        self.wings[0].count.clamp(1, MAX_WING)
    }

    /// Every plane of the mission when it is open (every plane on the AI,
    /// plane 0 included), in plane order.
    pub fn open_planes(&self) -> Vec<OpenPlane> {
        let mut planes = Vec::new();
        for (index, wing) in self.wings.iter().enumerate() {
            for member in 0..wing.count.min(MAX_WING) {
                planes.push(OpenPlane {
                    id: planes.len() as u32,
                    wing: Self::wing_id(index),
                    member: member as u8,
                    aircraft: self
                        .plane_aircraft
                        .get(&(planes.len() as u32))
                        .copied()
                        .unwrap_or(wing.aircraft),
                });
            }
        }
        planes
    }

    /// A one-line summary for a lobby or a game list, for example `UKR,
    /// clear, airborne at 20000 ft: F/A-18D Hornet x4 against MiG-29 Fulcrum-C x4`.
    pub fn summary(&self) -> String {
        let side = |enemy: bool| {
            let wings: Vec<String> = self
                .wings
                .iter()
                .enumerate()
                .filter(|(index, wing)| (*index >= 3) == enemy && wing.count > 0)
                .map(|(_, wing)| format!("{} x{}", wing.aircraft.label(), wing.count))
                .collect();
            if wings.is_empty() {
                "nobody".to_owned()
            } else {
                wings.join(", ")
            }
        };
        let start = match self.start {
            Start::Airborne { altitude_ft } => format!("airborne at {altitude_ft} ft"),
            Start::Ground { .. } | Start::GroundAuto { .. } => "ground start".to_owned(),
        };
        let start = if self.enemy_start.on_ground() {
            format!("{start}, enemy ground start")
        } else {
            start
        };
        format!(
            "{}, {}, {start}: {} against {}",
            self.theater,
            self.condition.name(),
            side(false),
            side(true)
        )
    }

    /// Refuses a spec no mission can be built from. A spec that parses from
    /// text is valid.
    pub fn validate(&self) -> Result<(), MissionError> {
        if self.theater.is_empty()
            || !self
                .theater
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'~' || b == b'_')
        {
            return refuse(format!("`{}` is not a theater code", self.theater));
        }
        if !ALTITUDES_FT.contains(&self.start.altitude_ft()) {
            return refuse(format!(
                "the altitude must be one of {}, not {}",
                join(ALTITUDES_FT),
                self.start.altitude_ft()
            ));
        }
        if let Start::Ground { runway, .. } = self.start
            && runway < RUNWAY_OBJECT_BASE
        {
            return refuse(format!("{runway} is not a runway object"));
        }
        if let Some(runway) = self.enemy_start.runway()
            && runway < RUNWAY_OBJECT_BASE
        {
            return refuse(format!("{runway} is not a runway object"));
        }
        if self.enemy_start.on_ground() {
            // Mirrors the player's ground start: something to park, on the
            // model that stands on a runway, and not on the player's runway.
            if self.enemy_wing_size() == 0 {
                return refuse(
                    "`enemy-start ground` parks enemy wing 1, which has no aircraft".into(),
                );
            }
            if self.fixture_wings {
                return refuse("`enemy-start ground` needs AI wings, not fixture wings".into());
            }
            if let (Some(enemy), Some(own)) = (self.enemy_start.runway(), self.ground_runway())
                && enemy == own
            {
                return refuse(
                    "the enemy cannot start on the player's runway; choose another airfield".into(),
                );
            }
        }
        if let Some(target) = &self.ground_target {
            let offered = ground_targets(&self.theater);
            if !offered.contains(&target.as_str()) {
                return refuse(if offered.is_empty() {
                    format!("{} has no ground targets", self.theater)
                } else {
                    format!(
                        "`{target}` is not a ground target of {}; the choices are {}",
                        self.theater,
                        offered.join(", ")
                    )
                });
            }
        }
        if self.enemy_nationality >= NATIONALITIES {
            return refuse(format!(
                "the enemy nationality is 0 to {}, not {}",
                NATIONALITIES - 1,
                self.enemy_nationality
            ));
        }
        if !SEPARATION_NM.contains(&f64::from(self.separation_nm)) {
            return refuse(format!(
                "the separation must be one of {} nautical miles, not {}",
                join(SEPARATION_NM.map(|nm| nm as u32)),
                self.separation_nm
            ));
        }
        if let Some([hour, minute]) = self.weather.time
            && (!(0..24).contains(&hour) || !(0..60).contains(&minute))
        {
            return refuse(format!("{hour:02}:{minute:02} is not a time of day"));
        }
        if let Some([heading, speed]) = self.weather.wind
            && (!(-360..=360).contains(&heading) || !(0..=200).contains(&speed))
        {
            return refuse(format!(
                "the wind heading must be -360 to 360 degrees and its speed 0 to 200 feet a second, not {heading} and {speed}"
            ));
        }
        if let Some(deck) = self.weather.cloud_altitude
            && !(0..=400_000).contains(&deck)
        {
            return refuse(format!(
                "the cloud deck must be 0 to 400000 feet, not {deck}"
            ));
        }
        for (index, wing) in self.wings.iter().enumerate() {
            let least = usize::from(index == 0);
            if !(least..=MAX_WING).contains(&wing.count) {
                return refuse(format!(
                    "{} holds {least} to {MAX_WING} aircraft, not {}",
                    if index == 0 {
                        "friendly wing 1 (with the player)"
                    } else {
                        "a wing"
                    },
                    wing.count
                ));
            }
        }
        // Friendly wing 1 is flown by people; every other wing is the AI's,
        // and it flies whatever `AircraftId::ai_flyable` allows (every
        // selectable aircraft since slice H3).
        for (index, wing) in self.wings.iter().enumerate().skip(1) {
            if wing.count > 0 && !wing.aircraft.ai_flyable() {
                return refuse(format!(
                    "{} cannot hold the {}: the AI cannot fly it yet",
                    wing_name(Self::wing_id(index)),
                    wing.aircraft.label()
                ));
            }
        }
        for (index, objective) in self.objectives.iter().enumerate() {
            let own = Self::wing_id(index);
            match *objective {
                GroupObjective::Intercept(target) if target.side == own.side => {
                    return refuse(format!(
                        "{} cannot intercept a wing on its own side",
                        wing_name(own)
                    ));
                }
                GroupObjective::Escort(target) if target.side != own.side || target == own => {
                    return refuse(format!(
                        "{} can only escort another wing on its own side",
                        wing_name(own)
                    ));
                }
                GroupObjective::Strike if self.ground_target.is_none() => {
                    return refuse(format!(
                        "{} cannot strike: the mission has no ground target",
                        wing_name(own)
                    ));
                }
                GroupObjective::Strike if own.side == Side::Enemy => {
                    return refuse(format!(
                        "{} cannot strike: Quick Mission gives Redfor no ground objectives",
                        wing_name(own)
                    ));
                }
                GroupObjective::Strike if index == 0 => {
                    return refuse(format!(
                        "{} cannot strike on its own: its wingmen strike on your orders",
                        wing_name(own)
                    ));
                }
                _ => {}
            }
        }
        for load in self.loadout.iter().chain(self.plane_loadouts.values()) {
            if !load.fuel_lbs.is_finite() || load.fuel_lbs < 0. {
                return refuse(format!("{} is not a fuel load", load.fuel_lbs));
            }
            if let Some(tanks) = &load.tanks
                && (tanks.len() > 9
                    || tanks.iter().any(|tank| {
                        tore_formats::aircraft::stations::fuel_bank(usize::from(tank.hardpoint))
                            .is_none()
                            || !resource_name(&tank.tank)
                            || !tank.tank.ends_with(".GAS")
                    }))
            {
                return refuse("a tank load requires at most nine valid source hardpoints and GAS resource names".into());
            }
            if load.stations.len() > MAX_STATIONS {
                return refuse(format!(
                    "a loadout has at most {MAX_STATIONS} stations, not {}",
                    load.stations.len()
                ));
            }
            if let Some(bad) = load.stations.iter().find(|s| !resource_name(&s.weapon)) {
                return refuse(format!("`{}` is not a weapon resource name", bad.weapon));
            }
        }
        let planes = self.open_planes();
        for plane in self.plane_aircraft.keys() {
            let Some(slot) = planes.iter().find(|p| p.id == *plane) else {
                return refuse(format!("plane {plane} is not in the mission"));
            };
            if !self.player_choice[Self::wing_index(slot.wing)] {
                return refuse(format!("plane {plane} is not a Player Choice slot"));
            }
        }
        for (index, choice) in self.player_choice.iter().enumerate() {
            if *choice && !self.wings[index].aircraft.ai_flyable() {
                return refuse(format!(
                    "{} needs an AI-flyable default",
                    wing_name(Self::wing_id(index))
                ));
            }
        }
        if let Some(plane) = self.plane_loadouts.keys().find(|p| **p >= MAX_PLANES) {
            return refuse(format!(
                "plane {plane} is beyond the mission's planes 0 to {}",
                MAX_PLANES - 1
            ));
        }
        Ok(())
    }

    /// The text form: the mission file of docs/DEDICATED-SERVER.md. Parsing
    /// it gives back an equal spec.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut line = |text: String| {
            out.push_str(&text);
            out.push('\n');
        };
        line(format!("tore-mission {FORMAT_VERSION}"));
        line(format!("theater {}", self.theater));
        line(format!("condition {}", self.condition.name()));
        if let Some([hour, minute]) = self.weather.time {
            line(format!("time-of-day {hour:02}:{minute:02}"));
        }
        if let Some([heading, speed]) = self.weather.wind {
            line(format!("wind {heading} {speed}"));
        }
        if let Some(deck) = self.weather.cloud_altitude {
            line(format!("cloud-deck {deck}"));
        }
        match self.start {
            Start::Airborne { altitude_ft } => line(format!("start airborne {altitude_ft}")),
            Start::Ground {
                runway,
                altitude_ft,
            } => {
                let ordinal = runway - RUNWAY_OBJECT_BASE;
                if altitude_ft == DEFAULT_ALTITUDE_FT {
                    line(format!("start ground {ordinal}"));
                } else {
                    line(format!("start ground {ordinal} {altitude_ft}"));
                }
            }
            Start::GroundAuto { altitude_ft } => {
                if altitude_ft == DEFAULT_ALTITUDE_FT {
                    line("start ground auto".to_owned());
                } else {
                    line(format!("start ground auto {altitude_ft}"));
                }
            }
        }
        // Written only for a ground start, so a mission without one reads as
        // it always did.
        match self.enemy_start {
            EnemyStart::Airborne => {}
            EnemyStart::Ground { runway } => {
                line(format!(
                    "enemy-start ground {}",
                    runway - RUNWAY_OBJECT_BASE
                ));
            }
            EnemyStart::GroundAuto => line("enemy-start ground auto".to_owned()),
        }
        line(format!("separation-nm {}", self.separation_nm));
        // Written only when not neutral, so a mission without one reads as
        // it always did.
        if self.situation != Situation::Neutral {
            line(format!("situation {}", self.situation.name()));
        }
        line(format!("preset {}", self.preset.name()));
        line(format!("guns-only {}", yes_no(self.guns_only)));
        let absent = self.absent_wing();
        for (index, wing) in self.wings.iter().enumerate() {
            if index == 0 || *wing != absent {
                let id = Self::wing_id(index);
                line(format!(
                    "wing {} {} {} {} {}",
                    side_name(id.side),
                    id.display_number(),
                    wing.aircraft.selection_key(),
                    wing.count,
                    wing.skill.name()
                ));
            }
        }
        for (index, choice) in self.player_choice.iter().enumerate() {
            if *choice {
                line(format!("wing-choice {}", wing_words(Self::wing_id(index))));
            }
        }
        for (plane, aircraft) in &self.plane_aircraft {
            line(format!(
                "plane-aircraft {plane} {}",
                aircraft.selection_key()
            ));
        }
        for (index, objective) in self.objectives.iter().enumerate() {
            if *objective != GroupObjective::Inherit {
                line(format!(
                    "objective {} {}",
                    wing_words(Self::wing_id(index)),
                    objective_words(*objective)
                ));
            }
        }
        for (index, survive) in self.must_survive.iter().enumerate() {
            if *survive {
                line(format!("survive {} yes", wing_words(Self::wing_id(index))));
            }
        }
        // The surface layout's settings, written only when set so a mission
        // without a ground target or defenses reads as it always did. With
        // no target the defenses are the enemy's area defenses (B23).
        if let Some(target) = &self.ground_target {
            line(format!("ground-target {target}"));
        }
        if self.aaa != Defense::None || self.sam != Defense::None {
            line(format!(
                "defenses aaa {} sam {}",
                self.aaa.name(),
                self.sam.name()
            ));
        }
        if self.surface_seed != 0 {
            line(format!("surface-seed {}", self.surface_seed));
        }
        if self.enemy_nationality != default_enemy_nationality(&self.theater) {
            line(format!("enemy-nationality {}", self.enemy_nationality));
        }
        line(format!("cheats {}", cheats_words(&self.cheats)));
        // A networked mission's settings, written only when they differ
        // from the default, so a single-player spec's text is unchanged.
        if !self.friendly_fire {
            line("friendly-fire off".to_owned());
        }
        if self.cheat_loadouts {
            line("loadouts any".to_owned());
        }
        line(format!(
            "flight-model human {}",
            if self.researched_flight {
                "hybrid"
            } else {
                "legacy"
            }
        ));
        line(format!(
            "flight-model ai {}",
            match self.ai_flight_model {
                AiFlightModel::Standard => "standard",
                AiFlightModel::AllHybrid => "hybrid",
            }
        ));
        if let Some(skill) = self.enemy_skill {
            line(format!(
                "enemy-skill {}",
                match skill {
                    EnemySkillOverride::AllNovice => "novice",
                    EnemySkillOverride::AllAverage => "average",
                }
            ));
        }
        if self.fixture_wings {
            line("fixture-wings yes".to_owned());
        }
        if self.share_flight_loadout {
            line("share-flight-loadout yes".to_owned());
        }
        if let Some(load) = &self.loadout {
            for text in load_lines(load) {
                line(format!("loadout {text}"));
            }
        }
        for (plane, load) in &self.plane_loadouts {
            for text in load_lines(load) {
                line(format!("plane-loadout {plane} {text}"));
            }
        }
        out
    }

    /// What a wing that has no line means: no aircraft, in the player's type.
    fn absent_wing(&self) -> WingSpec {
        WingSpec {
            aircraft: self.wings[0].aircraft,
            count: 0,
            skill: Skill::Novice,
        }
    }

    /// Reads the text form.
    pub fn from_text(text: &str) -> Result<Self, MissionError> {
        let mut parser = Parser::default();
        let mut first = true;
        for (number, raw) in text.lines().enumerate() {
            let number = number + 1;
            let content = raw.split('#').next().unwrap_or("").trim();
            if content.is_empty() {
                continue;
            }
            let words: Vec<&str> = content.split_whitespace().collect();
            if first {
                first = false;
                let header = if words.len() != 2 || words[0] != "tore-mission" {
                    Some("the first line must be `tore-mission 1`".to_owned())
                } else if words[1] != FORMAT_VERSION.to_string() {
                    Some(format!(
                        "this is mission file version {}, and only version {FORMAT_VERSION} is known",
                        words[1]
                    ))
                } else {
                    None
                };
                if let Some(message) = header {
                    return Err(MissionError(message).at_line(number));
                }
                continue;
            }
            parser.line(&words).map_err(|error| error.at_line(number))?;
        }
        if first {
            return refuse(
                "the mission file is empty: the first line must be `tore-mission 1`".into(),
            );
        }
        parser.finish()
    }
}

impl fmt::Display for MissionSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

/// Why a mission file or spec was refused, in words a person can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionError(String);

impl MissionError {
    fn at_line(self, number: usize) -> Self {
        Self(format!("line {number}: {}", self.0))
    }
}

impl fmt::Display for MissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MissionError {}

type Parsed<T> = Result<T, MissionError>;

fn refuse<T>(message: String) -> Parsed<T> {
    Err(MissionError(message))
}

fn once<T>(slot: &mut Option<T>, what: &str, value: T) -> Parsed<()> {
    if slot.is_some() {
        return refuse(format!("{what} appears twice"));
    }
    *slot = Some(value);
    Ok(())
}

/// The arguments of a line that must have exactly `count` of them.
fn arguments<'w, 'a>(words: &'w [&'a str], count: usize, usage: &str) -> Parsed<&'w [&'a str]> {
    if words.len() != count + 1 {
        return refuse(format!("expected `{usage}`"));
    }
    Ok(&words[1..])
}

fn whole(word: &str, what: &str) -> Parsed<u32> {
    word.parse::<u32>()
        .or_else(|_| refuse(format!("{what} must be a whole number, not `{word}`")))
}

fn signed(word: &str, what: &str) -> Parsed<i32> {
    word.parse::<i32>()
        .or_else(|_| refuse(format!("{what} must be a whole number, not `{word}`")))
}

fn yes_no_word(word: &str, key: &str) -> Parsed<bool> {
    match word {
        "yes" => Ok(true),
        "no" => Ok(false),
        _ => refuse(format!("`{key}` takes yes or no, not `{word}`")),
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Friendly => "friendly",
        Side::Enemy => "enemy",
    }
}

fn wing_name(id: WingId) -> String {
    format!("{} wing {}", side_name(id.side), id.display_number())
}

fn wing_words(id: WingId) -> String {
    format!("{} {}", side_name(id.side), id.display_number())
}

fn join<T: fmt::Display>(values: impl IntoIterator<Item = T>) -> String {
    values
        .into_iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn resource_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'~' | b'$' | b'-'))
}

fn parse_side(word: &str) -> Parsed<Side> {
    match word {
        "friendly" => Ok(Side::Friendly),
        "enemy" => Ok(Side::Enemy),
        _ => refuse(format!("a side is `friendly` or `enemy`, not `{word}`")),
    }
}

/// A `SIDE N` pair as a position in the spec's wings.
fn parse_wing(side: &str, number: &str) -> Parsed<usize> {
    let side = parse_side(side)?;
    match whole(number, "a wing number")? {
        n @ 1..=3 => Ok(MissionSpec::wing_index(
            WingId::new(side, (n - 1) as u8).expect("a fixed wing"),
        )),
        n => refuse(format!("a wing number is 1 to 3, not {n}")),
    }
}

fn parse_aircraft(word: &str) -> Parsed<AircraftId> {
    match AircraftId::SELECTABLE
        .into_iter()
        .find(|id| id.selection_key() == word)
    {
        Some(id) => Ok(id),
        None => refuse(format!(
            "`{word}` is not an aircraft; the choices are {}",
            join(AircraftId::SELECTABLE.map(AircraftId::selection_key))
        )),
    }
}

fn objective_words(objective: GroupObjective) -> String {
    match objective {
        GroupObjective::Inherit => "inherit".into(),
        GroupObjective::Free => "free".into(),
        GroupObjective::Cap => "cap".into(),
        GroupObjective::Intercept(target) => format!("intercept {}", wing_words(target)),
        GroupObjective::Escort(target) => format!("escort {}", wing_words(target)),
        GroupObjective::SelfDefense => "self-defense".into(),
        GroupObjective::Hold => "hold".into(),
        GroupObjective::Strike => "strike".into(),
    }
}

/// The cheat switches, by the names the text form uses.
const CHEAT_FLAGS: [&str; 15] = [
    "unlimited-ammo",
    "unlimited-fuel",
    "no-spins",
    "no-turbulence",
    "extra-g",
    "ignore-weapon-weights",
    "no-sun-whiteout",
    "no-g-effects",
    "no-screen-shake",
    "no-crashes",
    "easy-aiming",
    "ignore-midair-collisions",
    "easy-targeting",
    "guns-only",
    "easy-physics",
];

fn cheat_flag<'c>(cheats: &'c mut Cheats, name: &str) -> Option<&'c mut bool> {
    Some(match name {
        "unlimited-ammo" => &mut cheats.unlimited_ammo,
        "unlimited-fuel" => &mut cheats.unlimited_fuel,
        "no-spins" => &mut cheats.no_spins,
        "no-turbulence" => &mut cheats.no_turbulence,
        "extra-g" => &mut cheats.extra_g,
        "ignore-weapon-weights" => &mut cheats.ignore_weapon_weights,
        "no-sun-whiteout" => &mut cheats.no_sun_whiteout,
        "no-g-effects" => &mut cheats.no_g_effects,
        "no-screen-shake" => &mut cheats.no_screen_shake,
        "no-crashes" => &mut cheats.no_crashes,
        "easy-aiming" => &mut cheats.easy_aiming,
        "ignore-midair-collisions" => &mut cheats.ignore_midair_collisions,
        "easy-targeting" => &mut cheats.easy_targeting,
        "guns-only" => &mut cheats.guns_only,
        "easy-physics" => &mut cheats.easy_physics,
        _ => return None,
    })
}

fn cheats_words(cheats: &Cheats) -> String {
    let mut copy = *cheats;
    let mut words: Vec<String> = CHEAT_FLAGS
        .iter()
        .filter(|name| cheat_flag(&mut copy, name).is_some_and(|flag| *flag))
        .map(|name| (*name).to_owned())
        .collect();
    match cheats.damage {
        Damage::Normal => {}
        Damage::Invulnerable => words.push("damage=invulnerable".into()),
        Damage::Realistic => words.push("damage=realistic".into()),
    }
    if let Some(level) = cheats.enemy_ai {
        words.push(format!("enemy-ai={}", experience_name(level)));
    }
    if words.is_empty() {
        "none".into()
    } else {
        words.join(" ")
    }
}

fn experience_name(level: Experience) -> &'static str {
    match level {
        Experience::Novice => "novice",
        Experience::Average => "average",
        Experience::Experienced => "experienced",
        Experience::Ace => "ace",
    }
}

/// What the parser has read so far.
#[derive(Default)]
struct Parser {
    theater: Option<String>,
    condition: Option<Condition>,
    time: Option<[i32; 2]>,
    wind: Option<[i32; 2]>,
    cloud_deck: Option<i32>,
    start: Option<Start>,
    enemy_start: Option<EnemyStart>,
    separation: Option<u32>,
    situation: Option<Situation>,
    preset: Option<Preset>,
    guns_only: Option<bool>,
    wings: [Option<WingSpec>; WINGS],
    player_choice: [Option<bool>; WINGS],
    plane_aircraft: BTreeMap<u32, AircraftId>,
    objectives: [Option<GroupObjective>; WINGS],
    survive: [Option<bool>; WINGS],
    cheats: Option<Cheats>,
    human_model: Option<bool>,
    ai_model: Option<AiFlightModel>,
    enemy_skill: Option<Option<EnemySkillOverride>>,
    fixture_wings: Option<bool>,
    share_flight_loadout: Option<bool>,
    load: LoadParts,
    plane_loads: BTreeMap<u32, LoadParts>,
    friendly_fire: Option<bool>,
    cheat_loadouts: Option<bool>,
    ground_target: Option<String>,
    defenses: Option<(Defense, Defense)>,
    surface_seed: Option<u32>,
    enemy_nationality: Option<u8>,
}

/// One loadout's lines as the parser has read them.
#[derive(Default)]
struct LoadParts {
    fuel: Option<f64>,
    cheat: Option<bool>,
    stations: Vec<Option<StationLoad>>,
    tanks: Option<Vec<TankLoad>>,
}

impl LoadParts {
    fn is_empty(&self) -> bool {
        self.fuel.is_none()
            && self.cheat.is_none()
            && self.stations.is_empty()
            && self.tanks.is_none()
    }

    /// Reads `fuel POUNDS`, `cheat yes/no` or `station N WEAPON COUNT
    /// QUANTITY`; `key` is the line's key, for the messages.
    fn line(&mut self, key: &str, words: &[&str]) -> Parsed<()> {
        match words.first().copied() {
            Some("fuel") => {
                let [pounds] = arguments(words, 1, &format!("{key} fuel POUNDS"))? else {
                    unreachable!("one argument")
                };
                let Some(pounds) = pounds
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.)
                else {
                    return refuse(format!("`{pounds}` is not a fuel load"));
                };
                once(&mut self.fuel, &format!("`{key} fuel`"), pounds)
            }
            Some("cheat") => {
                let [value] = arguments(words, 1, &format!("{key} cheat yes/no"))? else {
                    unreachable!("one argument")
                };
                once(
                    &mut self.cheat,
                    &format!("`{key} cheat`"),
                    yes_no_word(value, &format!("{key} cheat"))?,
                )
            }
            Some("tanks") => {
                if words != ["tanks", "explicit"] {
                    return refuse(format!("expected `{key} tanks explicit`"));
                }
                if self.tanks.is_some() {
                    return refuse(format!("duplicate `{key} tanks`"));
                }
                self.tanks = Some(Vec::new());
                Ok(())
            }
            Some("tank") => {
                let [hardpoint, tank, quantity] =
                    arguments(words, 3, &format!("{key} tank HARDPOINT GAS QUANTITY"))?
                else {
                    unreachable!("three arguments")
                };
                let hardpoint = u8::try_from(whole(hardpoint, "the tank hardpoint")?)
                    .or_else(|_| refuse("tank hardpoint is 0 to 8".into()))?;
                if tore_formats::aircraft::stations::fuel_bank(usize::from(hardpoint)).is_none()
                    || !resource_name(tank)
                    || !tank.ends_with(".GAS")
                {
                    return refuse("invalid tank hardpoint or GAS resource".into());
                }
                let quantity = u16::try_from(whole(quantity, "the tank quantity")?)
                    .or_else(|_| refuse("tank quantity exceeds 65535".into()))?;
                let tanks = self.tanks.get_or_insert_with(Vec::new);
                if tanks.iter().any(|row| row.hardpoint == hardpoint) {
                    return refuse(format!("duplicate tank hardpoint {hardpoint}"));
                }
                tanks.push(TankLoad {
                    hardpoint,
                    tank: (*tank).to_owned(),
                    quantity,
                });
                Ok(())
            }
            Some("station") => {
                let [index, weapon, count, quantity] =
                    arguments(words, 4, &format!("{key} station N WEAPON COUNT QUANTITY"))?
                else {
                    unreachable!("four arguments")
                };
                let index = whole(index, "the station number")? as usize;
                if index >= MAX_STATIONS {
                    return refuse(format!("station {index} is beyond the aircraft's stations"));
                }
                if !resource_name(weapon) {
                    return refuse(format!("`{weapon}` is not a weapon resource name"));
                }
                let count = u16::try_from(whole(count, "the station's capacity")?)
                    .or_else(|_| refuse("the station's capacity is 0 to 65535".into()))?;
                let quantity = u16::try_from(whole(quantity, "the quantity")?)
                    .or_else(|_| refuse("the quantity is 0 to 65535".into()))?;
                if self.stations.len() <= index {
                    self.stations.resize(index + 1, None);
                }
                once(
                    &mut self.stations[index],
                    &format!("`{key} station {index}`"),
                    StationLoad {
                        weapon: (*weapon).to_owned(),
                        count,
                        quantity,
                    },
                )
            }
            _ => refuse(format!(
                "expected `{key} fuel`, `{key} cheat` or `{key} station`, each with its values"
            )),
        }
    }

    /// The loadout, once every line is read: the fuel and every station from
    /// 0 up must be there.
    fn finish(self, key: &str) -> Parsed<LoadoutSpec> {
        let Some(fuel_lbs) = self.fuel else {
            return refuse(format!("the loadout has no `{key} fuel` line"));
        };
        let mut stations = Vec::new();
        for (index, station) in self.stations.into_iter().enumerate() {
            let Some(station) = station else {
                return refuse(format!("the loadout has no `{key} station {index}` line"));
            };
            stations.push(station);
        }
        Ok(LoadoutSpec {
            tanks: self.tanks,
            fuel_lbs,
            cheat: self.cheat.unwrap_or(false),
            stations,
        })
    }
}

/// A loadout's lines after their key: the fuel, the cheat and each station.
fn load_lines(load: &LoadoutSpec) -> Vec<String> {
    let mut lines = vec![
        format!("fuel {}", load.fuel_lbs),
        format!("cheat {}", yes_no(load.cheat)),
    ];
    if let Some(tanks) = &load.tanks {
        lines.push("tanks explicit".into());
        for tank in tanks {
            lines.push(format!(
                "tank {} {} {}",
                tank.hardpoint, tank.tank, tank.quantity
            ));
        }
    }
    for (index, station) in load.stations.iter().enumerate() {
        lines.push(format!(
            "station {index} {} {} {}",
            station.weapon, station.count, station.quantity
        ));
    }
    lines
}

impl Parser {
    fn line(&mut self, words: &[&str]) -> Parsed<()> {
        let key = words[0];
        match key {
            "tore-mission" => refuse("`tore-mission` is only the first line".into()),
            "theater" => {
                let [code] = arguments(words, 1, "theater CODE")? else {
                    unreachable!("one argument")
                };
                if !THEATERS.contains(code) {
                    return refuse(format!(
                        "`{code}` is not a theater; the choices are {}",
                        THEATERS.join(", ")
                    ));
                }
                once(&mut self.theater, "`theater`", (*code).to_owned())
            }
            "condition" => {
                let [name] = arguments(words, 1, "condition NAME")? else {
                    unreachable!("one argument")
                };
                match Condition::parse(name) {
                    Some(condition) => once(&mut self.condition, "`condition`", condition),
                    None => refuse(format!(
                        "`{name}` is not a condition; the choices are {}",
                        CONDITION_NAMES.join(", ")
                    )),
                }
            }
            "time-of-day" => {
                let [text] = arguments(words, 1, "time-of-day HH:MM")? else {
                    unreachable!("one argument")
                };
                let time = text
                    .split_once(':')
                    .and_then(|(h, m)| Some([h.parse::<i32>().ok()?, m.parse::<i32>().ok()?]))
                    .filter(|[h, m]| (0..24).contains(h) && (0..60).contains(m));
                match time {
                    Some(time) => once(&mut self.time, "`time-of-day`", time),
                    None => refuse(format!("`{text}` is not a time of day, write HH:MM")),
                }
            }
            "wind" => {
                let [heading, speed] = arguments(words, 2, "wind HEADING FEET-PER-SECOND")? else {
                    unreachable!("two arguments")
                };
                let heading = signed(heading, "the wind heading")?;
                let speed = signed(speed, "the wind speed")?;
                if !(-360..=360).contains(&heading) || !(0..=200).contains(&speed) {
                    return refuse(
                        "the wind heading is -360 to 360 degrees and its speed 0 to 200 feet a second"
                            .into(),
                    );
                }
                once(&mut self.wind, "`wind`", [heading, speed])
            }
            "cloud-deck" => {
                let [feet] = arguments(words, 1, "cloud-deck FEET")? else {
                    unreachable!("one argument")
                };
                let feet = signed(feet, "the cloud deck")?;
                if !(0..=400_000).contains(&feet) {
                    return refuse("the cloud deck is 0 to 400000 feet".into());
                }
                once(&mut self.cloud_deck, "`cloud-deck`", feet)
            }
            "start" => self.start(words),
            "enemy-start" => self.enemy_start(words),
            "separation-nm" => {
                let [nm] = arguments(words, 1, "separation-nm N")? else {
                    unreachable!("one argument")
                };
                let nm = whole(nm, "the separation")?;
                if !SEPARATION_NM.contains(&f64::from(nm)) {
                    return refuse(format!(
                        "the separation is one of {} nautical miles, not {nm}",
                        join(SEPARATION_NM.map(|nm| nm as u32))
                    ));
                }
                once(&mut self.separation, "`separation-nm`", nm)
            }
            "situation" => {
                let [word] = arguments(words, 1, "situation NAME")? else {
                    unreachable!("one argument")
                };
                match Situation::parse(word) {
                    Some(situation) => once(&mut self.situation, "`situation`", situation),
                    None => refuse(format!(
                        "`{word}` is not a situation; the choices are {}",
                        join(Situation::ALL.map(Situation::name))
                    )),
                }
            }
            "preset" => {
                let [name] = arguments(words, 1, "preset NAME")? else {
                    unreachable!("one argument")
                };
                match name.parse::<Preset>() {
                    Ok(preset) => once(&mut self.preset, "`preset`", preset),
                    Err(_) => refuse(format!(
                        "`{name}` is not a preset; the choices are {}",
                        join(Preset::ALL.map(Preset::name))
                    )),
                }
            }
            "guns-only" => {
                let [value] = arguments(words, 1, "guns-only yes/no")? else {
                    unreachable!("one argument")
                };
                once(&mut self.guns_only, "`guns-only`", yes_no_word(value, key)?)
            }
            "wing" => self.wing(words),
            "wing-choice" => {
                let [side, number] = arguments(words, 2, "wing-choice SIDE N")? else {
                    unreachable!()
                };
                let index = parse_wing(side, number)?;
                once(&mut self.player_choice[index], "wing-choice", true)
            }
            "plane-aircraft" => {
                let [plane, aircraft] = arguments(words, 2, "plane-aircraft PLANE AIRCRAFT")?
                else {
                    unreachable!()
                };
                let plane = whole(plane, "the plane number")?;
                let aircraft = parse_aircraft(aircraft)?;
                if self.plane_aircraft.insert(plane, aircraft).is_some() {
                    return refuse(format!("plane {plane}'s aircraft appears twice"));
                }
                Ok(())
            }
            "objective" => self.objective(words),
            "survive" => {
                let [side, number, value] = arguments(words, 3, "survive SIDE N yes/no")? else {
                    unreachable!("three arguments")
                };
                let index = parse_wing(side, number)?;
                let what = format!("`survive {side} {number}`");
                once(&mut self.survive[index], &what, yes_no_word(value, key)?)
            }
            "cheats" => {
                if words.len() < 2 {
                    return refuse("expected `cheats none` or `cheats` and a list".into());
                }
                let cheats = Self::cheats(&words[1..])?;
                once(&mut self.cheats, "`cheats`", cheats)
            }
            "flight-model" => self.flight_model(words),
            "enemy-skill" => {
                let [name] = arguments(words, 1, "enemy-skill novice/average/none")? else {
                    unreachable!("one argument")
                };
                let skill = match *name {
                    "novice" => Some(EnemySkillOverride::AllNovice),
                    "average" => Some(EnemySkillOverride::AllAverage),
                    "none" => None,
                    _ => {
                        return refuse(format!(
                            "`{name}` is not an enemy skill; the choices are novice, average and none"
                        ));
                    }
                };
                once(&mut self.enemy_skill, "`enemy-skill`", skill)
            }
            "fixture-wings" => {
                let [value] = arguments(words, 1, "fixture-wings yes/no")? else {
                    unreachable!("one argument")
                };
                once(
                    &mut self.fixture_wings,
                    "`fixture-wings`",
                    yes_no_word(value, key)?,
                )
            }
            "share-flight-loadout" => {
                let [value] = arguments(words, 1, "share-flight-loadout yes/no")? else {
                    unreachable!("one argument")
                };
                once(
                    &mut self.share_flight_loadout,
                    "`share-flight-loadout`",
                    yes_no_word(value, key)?,
                )
            }
            "loadout" => self.loadout(words),
            "plane-loadout" => self.plane_loadout(words),
            "friendly-fire" => {
                let [value] = arguments(words, 1, "friendly-fire on/off")? else {
                    unreachable!("one argument")
                };
                let on = match *value {
                    "on" => true,
                    "off" => false,
                    other => {
                        return refuse(format!("`friendly-fire` is `on` or `off`, not `{other}`"));
                    }
                };
                once(&mut self.friendly_fire, "`friendly-fire`", on)
            }
            "loadouts" => {
                let [value] = arguments(words, 1, "loadouts own/any")? else {
                    unreachable!("one argument")
                };
                let any = match *value {
                    "own" => false,
                    "any" => true,
                    other => {
                        return refuse(format!("`loadouts` is `own` or `any`, not `{other}`"));
                    }
                };
                once(&mut self.cheat_loadouts, "`loadouts`", any)
            }
            "ground-target" => {
                let [stem] = arguments(words, 1, "ground-target TEMPLATE")? else {
                    unreachable!("one argument")
                };
                if !resource_name(stem) {
                    return refuse(format!("`{stem}` is not a ground target name"));
                }
                once(
                    &mut self.ground_target,
                    "`ground-target`",
                    (*stem).to_owned(),
                )
            }
            "defenses" => {
                let usage = "defenses aaa LEVEL sam LEVEL";
                let (Some(&"aaa"), Some(&"sam")) = (words.get(1), words.get(3)) else {
                    return refuse(format!("expected `{usage}`"));
                };
                let [_, _, aaa, _, sam] = words else {
                    return refuse(format!("expected `{usage}`"));
                };
                let level = |word: &str| {
                    Defense::parse(word).ok_or_else(|| {
                        MissionError(format!(
                            "`{word}` is not a defense level; the choices are {}",
                            join(Defense::ALL.map(Defense::name))
                        ))
                    })
                };
                once(&mut self.defenses, "`defenses`", (level(aaa)?, level(sam)?))
            }
            "surface-seed" => {
                let [seed] = arguments(words, 1, "surface-seed N")? else {
                    unreachable!("one argument")
                };
                let seed = seed.parse::<u32>().or_else(|_| {
                    refuse(format!("the surface seed is a whole number, not `{seed}`"))
                })?;
                once(&mut self.surface_seed, "`surface-seed`", seed)
            }
            "enemy-nationality" => {
                let [index] = arguments(words, 1, "enemy-nationality N")? else {
                    unreachable!("one argument")
                };
                let index = index
                    .parse::<u8>()
                    .ok()
                    .filter(|index| *index < NATIONALITIES)
                    .map_or_else(
                        || {
                            refuse(format!(
                                "the enemy nationality is 0 to {}, not `{index}`",
                                NATIONALITIES - 1
                            ))
                        },
                        Ok,
                    )?;
                once(&mut self.enemy_nationality, "`enemy-nationality`", index)
            }
            _ => refuse(format!("unknown setting `{key}`")),
        }
    }

    fn start(&mut self, words: &[&str]) -> Parsed<()> {
        let start = match words.get(1).copied() {
            Some("airborne") => {
                let [_, feet] = arguments(words, 2, "start airborne FEET")? else {
                    unreachable!("two arguments")
                };
                let altitude_ft = whole(feet, "the altitude")?;
                if !ALTITUDES_FT.contains(&altitude_ft) {
                    return refuse(format!(
                        "the airborne altitude is one of {} feet, not {altitude_ft}",
                        join(ALTITUDES_FT)
                    ));
                }
                Start::Airborne { altitude_ft }
            }
            Some("ground") => {
                if words.len() != 3 && words.len() != 4 {
                    return refuse("expected `start ground RUNWAY [FEET]`".into());
                }
                let auto = words[2] == "auto";
                let ordinal = if auto {
                    0
                } else {
                    whole(words[2], "the runway number")?
                };
                if ordinal >= RUNWAY_OBJECT_BASE {
                    return refuse(format!("{ordinal} is not a runway number"));
                }
                let altitude_ft = match words.get(3) {
                    Some(feet) => whole(feet, "the altitude")?,
                    None => DEFAULT_ALTITUDE_FT,
                };
                if !ALTITUDES_FT.contains(&altitude_ft) {
                    return refuse(format!(
                        "the altitude is one of {} feet, not {altitude_ft}",
                        join(ALTITUDES_FT)
                    ));
                }
                if auto {
                    Start::GroundAuto { altitude_ft }
                } else {
                    Start::Ground {
                        runway: RUNWAY_OBJECT_BASE + ordinal,
                        altitude_ft,
                    }
                }
            }
            _ => {
                return refuse("expected `start airborne FEET` or `start ground RUNWAY`".into());
            }
        };
        once(&mut self.start, "`start`", start)
    }

    fn enemy_start(&mut self, words: &[&str]) -> Parsed<()> {
        let usage = "expected `enemy-start airborne`, `enemy-start ground RUNWAY` or `enemy-start ground auto`";
        let start = match words[1..] {
            ["airborne"] => EnemyStart::Airborne,
            ["ground", "auto"] => EnemyStart::GroundAuto,
            ["ground", number] => {
                let ordinal = whole(number, "the runway number")?;
                if ordinal >= RUNWAY_OBJECT_BASE {
                    return refuse(format!("{ordinal} is not a runway number"));
                }
                EnemyStart::Ground {
                    runway: RUNWAY_OBJECT_BASE + ordinal,
                }
            }
            _ => return refuse(usage.into()),
        };
        once(&mut self.enemy_start, "`enemy-start`", start)
    }

    fn wing(&mut self, words: &[&str]) -> Parsed<()> {
        let [side, number, aircraft, count, skill] =
            arguments(words, 5, "wing SIDE N AIRCRAFT COUNT SKILL")?
        else {
            unreachable!("five arguments")
        };
        let index = parse_wing(side, number)?;
        let aircraft = parse_aircraft(aircraft)?;
        let count = whole(count, "the aircraft count")? as usize;
        let least = usize::from(index == 0);
        if !(least..=MAX_WING).contains(&count) {
            return refuse(format!(
                "{} holds {least} to {MAX_WING} aircraft, not {count}",
                if index == 0 {
                    "friendly wing 1 (with the player)"
                } else {
                    "a wing"
                }
            ));
        }
        let Some(skill) = Skill::parse(skill) else {
            return refuse(format!(
                "`{skill}` is not a skill; the choices are {}",
                join(Skill::ALL.map(Skill::name))
            ));
        };
        let what = wing_name(MissionSpec::wing_id(index));
        once(
            &mut self.wings[index],
            &what,
            WingSpec {
                aircraft,
                count,
                skill,
            },
        )
    }

    fn objective(&mut self, words: &[&str]) -> Parsed<()> {
        if words.len() < 4 {
            return refuse("expected `objective SIDE N NAME [SIDE N]`".into());
        }
        let index = parse_wing(words[1], words[2])?;
        let own = MissionSpec::wing_id(index);
        let name = words[3];
        let rest = &words[4..];
        let target = || -> Parsed<WingId> {
            match rest {
                [side, number] => Ok(MissionSpec::wing_id(parse_wing(side, number)?)),
                _ => refuse(format!("`{name}` needs a wing: `{name} SIDE N`")),
            }
        };
        let objective = match name {
            "inherit" | "free" | "cap" | "self-defense" | "hold" | "strike" if !rest.is_empty() => {
                return refuse(format!("`{name}` takes no wing"));
            }
            "inherit" => GroupObjective::Inherit,
            "free" => GroupObjective::Free,
            "cap" => GroupObjective::Cap,
            "self-defense" => GroupObjective::SelfDefense,
            "hold" => GroupObjective::Hold,
            "strike" => GroupObjective::Strike,
            "intercept" => {
                let target = target()?;
                if target.side == own.side {
                    return refuse(format!(
                        "{} cannot intercept a wing on its own side",
                        wing_name(own)
                    ));
                }
                GroupObjective::Intercept(target)
            }
            "escort" => {
                let target = target()?;
                if target.side != own.side || target == own {
                    return refuse(format!(
                        "{} can only escort another wing on its own side",
                        wing_name(own)
                    ));
                }
                GroupObjective::Escort(target)
            }
            other => {
                return refuse(format!(
                    "`{other}` is not an objective; the choices are inherit, free, cap, intercept, escort, self-defense, hold and strike"
                ));
            }
        };
        once(
            &mut self.objectives[index],
            &format!("the objective of {}", wing_name(own)),
            objective,
        )
    }

    fn cheats(words: &[&str]) -> Parsed<Cheats> {
        let mut cheats = Cheats::default();
        if words == ["none"] {
            return Ok(cheats);
        }
        let mut seen: Vec<&str> = Vec::new();
        for &word in words {
            if word == "none" {
                return refuse("`none` stands alone".into());
            }
            if seen.contains(&word) {
                return refuse(format!("the cheat `{word}` is listed twice"));
            }
            seen.push(word);
            if let Some(flag) = cheat_flag(&mut cheats, word) {
                *flag = true;
            } else if let Some(level) = word.strip_prefix("damage=") {
                cheats.damage = match level {
                    "invulnerable" => Damage::Invulnerable,
                    "realistic" => Damage::Realistic,
                    _ => {
                        return refuse(format!(
                            "`{level}` is not a damage cheat; the choices are invulnerable and realistic"
                        ));
                    }
                };
            } else if let Some(level) = word.strip_prefix("enemy-ai=") {
                let Some(level) = Experience::ALL
                    .into_iter()
                    .find(|e| experience_name(*e) == level)
                else {
                    return refuse(format!(
                        "`{level}` is not an enemy level; the choices are novice, average, experienced and ace"
                    ));
                };
                cheats.enemy_ai = Some(level);
            } else {
                return refuse(format!(
                    "`{word}` is not a cheat; the choices are none, {}, damage=invulnerable, damage=realistic and enemy-ai=LEVEL",
                    CHEAT_FLAGS.join(", ")
                ));
            }
        }
        Ok(cheats)
    }

    fn flight_model(&mut self, words: &[&str]) -> Parsed<()> {
        let [who, model] = arguments(words, 2, "flight-model human/ai MODEL")? else {
            unreachable!("two arguments")
        };
        match (*who, *model) {
            ("human", "hybrid") => once(&mut self.human_model, "`flight-model human`", true),
            ("human", "legacy") => once(&mut self.human_model, "`flight-model human`", false),
            ("ai", "standard") => once(
                &mut self.ai_model,
                "`flight-model ai`",
                AiFlightModel::Standard,
            ),
            ("ai", "hybrid") => once(
                &mut self.ai_model,
                "`flight-model ai`",
                AiFlightModel::AllHybrid,
            ),
            ("human", _) => refuse(format!(
                "`{model}` is not a flight model for humans; the choices are hybrid and legacy"
            )),
            ("ai", _) => refuse(format!(
                "`{model}` is not a flight model for the AI; the choices are standard and hybrid"
            )),
            _ => refuse(format!(
                "`{who}` is not a flight model owner; the choices are human and ai"
            )),
        }
    }

    fn loadout(&mut self, words: &[&str]) -> Parsed<()> {
        self.load.line("loadout", &words[1..])
    }

    fn plane_loadout(&mut self, words: &[&str]) -> Parsed<()> {
        let Some(plane) = words.get(1) else {
            return refuse("expected `plane-loadout PLANE fuel`, `cheat` or `station`".into());
        };
        let plane = whole(plane, "the plane number")?;
        if plane >= MAX_PLANES {
            return refuse(format!(
                "plane {plane} is beyond the mission's planes 0 to {}",
                MAX_PLANES - 1
            ));
        }
        self.plane_loads
            .entry(plane)
            .or_default()
            .line(&format!("plane-loadout {plane}"), &words[2..])
    }

    fn finish(self) -> Parsed<MissionSpec> {
        let Some(theater) = self.theater else {
            return refuse("the mission has no `theater` line".into());
        };
        let Some(first) = self.wings[0] else {
            return refuse("the mission has no `wing friendly 1` line".into());
        };
        let mut spec = MissionSpec::new(&theater, first.aircraft);
        let absent = spec.absent_wing();
        for (index, wing) in self.wings.into_iter().enumerate() {
            spec.wings[index] = wing.unwrap_or(absent);
        }
        if let Some(condition) = self.condition {
            spec.condition = condition;
        }
        spec.weather = Overrides {
            time: self.time,
            wind: self.wind,
            cloud_altitude: self.cloud_deck,
            // The text has no airfield or city line: every mission it
            // describes, a session's or a server's, flies the redrawn
            // airports over the city fill.
            redrawn_airports: true,
            city_fill: true,
            // Nor a building budget: the whole fill, whatever
            // `TORE_CITY_FILL_CAP` says.
            city_fill_cap: None,
        };
        if let Some(start) = self.start {
            spec.start = start;
        }
        spec.enemy_start = self.enemy_start.unwrap_or_default();
        if let Some(nm) = self.separation {
            spec.separation_nm = nm;
        }
        spec.situation = self.situation.unwrap_or_default();
        if let Some(preset) = self.preset {
            spec.preset = preset;
        }
        spec.guns_only = self.guns_only.unwrap_or(false);
        spec.player_choice = self.player_choice.map(|v| v.unwrap_or(false));
        spec.plane_aircraft = self.plane_aircraft;
        for index in 0..WINGS {
            spec.objectives[index] = self.objectives[index].unwrap_or(GroupObjective::Inherit);
            spec.must_survive[index] = self.survive[index].unwrap_or(false);
        }
        spec.cheats = self.cheats.unwrap_or_default();
        spec.researched_flight = self.human_model.unwrap_or(true);
        spec.ai_flight_model = self.ai_model.unwrap_or(AiFlightModel::AllHybrid);
        spec.enemy_skill = self.enemy_skill.unwrap_or(None);
        spec.fixture_wings = self.fixture_wings.unwrap_or(false);
        spec.share_flight_loadout = self.share_flight_loadout.unwrap_or(false);
        spec.friendly_fire = self.friendly_fire.unwrap_or(true);
        spec.cheat_loadouts = self.cheat_loadouts.unwrap_or(false);
        spec.ground_target = self.ground_target;
        if let Some((aaa, sam)) = self.defenses {
            spec.aaa = aaa;
            spec.sam = sam;
        }
        spec.surface_seed = self.surface_seed.unwrap_or(0);
        if let Some(nationality) = self.enemy_nationality {
            spec.enemy_nationality = nationality;
        }
        if !self.load.is_empty() {
            spec.loadout = Some(self.load.finish("loadout")?);
        }
        for (plane, load) in self.plane_loads {
            let key = format!("plane-loadout {plane}");
            spec.plane_loadouts.insert(plane, load.finish(&key)?);
        }
        spec.validate()?;
        Ok(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_choice_text_and_member_launches_round_trip() {
        let text = "tore-mission 1\ntheater UKR\nwing friendly 1 F18.PT 2 average\nwing-choice friendly 1\nplane-aircraft 1 MIG29.PT\n";
        let spec = MissionSpec::from_text(text).unwrap();
        assert!(spec.player_choice[0]);
        assert_eq!(spec.open_planes()[0].aircraft, AircraftId::F18);
        assert_eq!(spec.open_planes()[1].aircraft.selection_key(), "MIG29.PT");
        let launches = spec.open_wing_launches().unwrap();
        assert_eq!(launches[1].aircraft, spec.open_planes()[1].aircraft);
        assert_eq!(launches[1].members[0].member, 1);
        assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
        assert!(MissionSpec::from_text(&text.replace("wing-choice friendly 1\n", "")).is_err());
        assert!(
            MissionSpec::from_text(&text.replace("plane-aircraft 1", "plane-aircraft 9")).is_err()
        );
        assert!(MissionSpec::from_text(&format!("{text}plane-aircraft 1 F18.PT\n")).is_err());
    }

    /// The first fenced `text` block under "The mission file" in the guide.
    fn guide_example() -> String {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/DEDICATED-SERVER.md"
        );
        // A Windows checkout may turn the guide's line ends into CR LF.
        let guide = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
        let section = guide.split("\n## The mission file").nth(1).unwrap();
        let block = section.split("```text\n").nth(1).unwrap();
        block.split("```").next().unwrap().to_owned()
    }

    #[test]
    fn tank_selections_round_trip_and_old_missions_keep_source_defaults() {
        for tanks in [
            None,
            Some(vec![]),
            Some(vec![TankLoad {
                hardpoint: 5,
                tank: "SYNTHETIC.GAS".into(),
                quantity: 2,
            }]),
        ] {
            let mut spec = MissionSpec::from_text(&guide_example()).unwrap();
            spec.loadout = Some(LoadoutSpec {
                fuel_lbs: 1000.,
                cheat: false,
                stations: vec![],
                tanks: tanks.clone(),
            });
            let text = spec.to_text();
            assert_eq!(text.contains("loadout tanks explicit"), tanks.is_some());
            let parsed = MissionSpec::from_text(&text).unwrap();
            assert_eq!(parsed.loadout.unwrap().tanks, tanks);
        }
    }

    #[test]
    fn the_guides_example_parses() {
        let spec = MissionSpec::from_text(&guide_example()).unwrap();
        assert_eq!(spec.theater, "UKR");
        assert_eq!(spec.condition, Condition::Clear);
        assert_eq!(
            spec.start,
            Start::Airborne {
                altitude_ft: 20_000
            }
        );
        assert_eq!(spec.separation_nm, 20);
        assert_eq!(spec.preset, Preset::Free);
        assert!(!spec.guns_only);
        let wing = |aircraft, count, skill| WingSpec {
            aircraft,
            count,
            skill,
        };
        assert_eq!(spec.wings[0], wing(AircraftId::F18, 4, Skill::Experienced));
        assert_eq!(spec.wings[1], wing(AircraftId::F14, 2, Skill::Average));
        assert_eq!(spec.wings[2].count, 0);
        assert_eq!(
            spec.wings[3],
            wing(AircraftId::Mig29, 4, Skill::Experienced)
        );
        assert_eq!(spec.wings[4], wing(AircraftId::Su27, 2, Skill::Ace));
        assert_eq!(spec.wings[5].count, 0);
        assert_eq!(spec.must_survive, [false, true, false, false, false, false]);
        assert_eq!(
            spec.objectives[4],
            GroupObjective::Intercept(MissionSpec::wing_id(0))
        );
        assert_eq!(spec.cheats, Cheats::default());
        // The defaults of what the example leaves out: the hybrid model for
        // everyone, as the guide says, and no loadout.
        assert!(spec.researched_flight);
        assert_eq!(spec.ai_flight_model, AiFlightModel::AllHybrid);
        assert_eq!(spec.loadout, None);
    }

    #[test]
    fn the_guides_example_round_trips_through_the_text_form() {
        let spec = MissionSpec::from_text(&guide_example()).unwrap();
        let text = spec.to_text();
        assert_eq!(MissionSpec::from_text(&text).unwrap(), spec);
        // The text is stable: writing what was read changes nothing.
        assert_eq!(MissionSpec::from_text(&text).unwrap().to_text(), text);
        assert!(text.starts_with("tore-mission 1\n"));
    }

    #[test]
    fn easy_physics_is_a_mission_cheat_in_the_text_form() {
        let mut spec = MissionSpec::from_text(&guide_example()).unwrap();
        assert!(!spec.cheats.easy_physics);
        spec.cheats.easy_physics = true;
        spec.cheats.no_spins = true;
        let text = spec.to_text();
        assert!(
            text.lines()
                .any(|line| line == "cheats no-spins easy-physics"),
            "{text}"
        );
        let back = MissionSpec::from_text(&text).unwrap();
        assert!(back.cheats.easy_physics && back.cheats.no_spins);
        assert_eq!(back, spec);
        // Alone, and listed twice (refused like any cheat).
        let text = text.replace("cheats no-spins easy-physics", "cheats easy-physics");
        let alone = MissionSpec::from_text(&text).unwrap();
        assert!(alone.cheats.easy_physics && !alone.cheats.no_spins);
        let twice = text.replace("cheats easy-physics", "cheats easy-physics easy-physics");
        assert!(
            MissionSpec::from_text(&twice)
                .unwrap_err()
                .to_string()
                .contains("the cheat `easy-physics` is listed twice")
        );
    }

    fn busy() -> MissionSpec {
        let mut spec = MissionSpec::new("KURILE", AircraftId::Rafale);
        spec.condition = Condition::Night;
        spec.weather = Overrides {
            time: Some([5, 7]),
            wind: Some([-90, 45]),
            cloud_altitude: Some(12_000),
            redrawn_airports: true,
            city_fill: true,
            city_fill_cap: None,
        };
        spec.start = Start::Ground {
            runway: RUNWAY_OBJECT_BASE + 3,
            altitude_ft: 10_000,
        };
        spec.separation_nm = 300;
        spec.situation = Situation::Disadvantage;
        spec.preset = Preset::SelfDefense;
        spec.guns_only = true;
        spec.wings = [
            WingSpec {
                aircraft: AircraftId::Rafale,
                count: 5,
                skill: Skill::Dummy,
            },
            WingSpec {
                aircraft: AircraftId::Faxx,
                count: 1,
                skill: Skill::Ace,
            },
            WingSpec {
                aircraft: AircraftId::X31,
                count: 3,
                skill: Skill::Novice,
            },
            WingSpec {
                aircraft: AircraftId::Su35,
                count: 5,
                skill: Skill::Experienced,
            },
            // A wing with no aircraft keeps its type and skill.
            WingSpec {
                aircraft: AircraftId::F22n,
                count: 0,
                skill: Skill::Ace,
            },
            WingSpec {
                aircraft: AircraftId::Rafale,
                count: 2,
                skill: Skill::Novice,
            },
        ];
        spec.objectives = [
            GroupObjective::Escort(MissionSpec::wing_id(1)),
            GroupObjective::Cap,
            GroupObjective::SelfDefense,
            GroupObjective::Intercept(MissionSpec::wing_id(2)),
            GroupObjective::Hold,
            GroupObjective::Free,
        ];
        spec.must_survive = [true, false, true, false, true, true];
        spec.cheats = Cheats {
            damage: Damage::Realistic,
            unlimited_ammo: true,
            unlimited_fuel: true,
            no_spins: true,
            no_turbulence: true,
            extra_g: true,
            ignore_weapon_weights: true,
            no_sun_whiteout: true,
            no_g_effects: true,
            no_screen_shake: true,
            no_crashes: true,
            easy_aiming: true,
            ignore_midair_collisions: true,
            easy_targeting: true,
            guns_only: true,
            easy_physics: true,
            enemy_ai: Some(Experience::Experienced),
        };
        spec.researched_flight = false;
        spec.ai_flight_model = AiFlightModel::Standard;
        spec.enemy_skill = Some(EnemySkillOverride::AllNovice);
        spec.fixture_wings = true;
        spec.friendly_fire = false;
        spec.cheat_loadouts = true;
        spec.loadout = Some(LoadoutSpec {
            tanks: None,
            fuel_lbs: 4_500.5,
            cheat: true,
            stations: vec![
                StationLoad {
                    weapon: "DEFA.JT".into(),
                    count: 600,
                    quantity: 450,
                },
                StationLoad {
                    weapon: "AIM9X.JT".into(),
                    count: 2,
                    quantity: 0,
                },
            ],
        });
        spec
    }

    #[test]
    fn the_situation_is_written_only_when_not_neutral() {
        let mut spec = MissionSpec::new("UKR", AircraftId::F18);
        assert!(!spec.to_text().contains("situation"));
        for (index, situation) in Situation::ALL.into_iter().enumerate() {
            assert_eq!(Situation::from_index(index), Some(situation));
            assert_eq!(situation.index(), index);
            spec.situation = situation;
            let text = spec.to_text();
            assert_eq!(
                text.contains(&format!("situation {}", situation.name())),
                situation != Situation::Neutral
            );
            assert_eq!(MissionSpec::from_text(&text).unwrap().situation, situation);
        }
        let text = MissionSpec::new("UKR", AircraftId::F18).to_text();
        let error = MissionSpec::from_text(&format!("{text}situation behind\n"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("not a situation"), "{error}");
        let twice =
            MissionSpec::from_text(&format!("{text}situation advantage\nsituation advantage\n"));
        assert!(twice.is_err());
    }

    #[test]
    fn every_setting_round_trips_through_the_text_form() {
        let spec = busy();
        spec.validate().unwrap();
        let text = spec.to_text();
        assert_eq!(MissionSpec::from_text(&text).unwrap(), spec, "{text}");
    }

    #[test]
    fn every_choice_of_each_setting_round_trips() {
        let mut specs = Vec::new();
        for theater in THEATERS {
            specs.push(MissionSpec::new(theater, AircraftId::F18));
        }
        for aircraft in AircraftId::SELECTABLE {
            let mut spec = MissionSpec::new("UKR", aircraft);
            for wing in &mut spec.wings {
                wing.aircraft = aircraft;
            }
            specs.push(spec);
        }
        for condition in Condition::ALL {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.condition = condition;
            specs.push(spec);
        }
        for skill in Skill::ALL {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.wings[0].skill = skill;
            spec.wings[5].skill = skill;
            spec.wings[5].count = 1;
            specs.push(spec);
        }
        for preset in Preset::ALL {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.preset = preset;
            specs.push(spec);
        }
        for situation in Situation::ALL {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.situation = situation;
            specs.push(spec);
        }
        for altitude_ft in ALTITUDES_FT {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.start = Start::Airborne { altitude_ft };
            specs.push(spec);
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.start = Start::Ground {
                runway: RUNWAY_OBJECT_BASE + 7,
                altitude_ft,
            };
            specs.push(spec);
        }
        for nm in SEPARATION_NM {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.separation_nm = nm as u32;
            specs.push(spec);
        }
        for count in 0..=MAX_WING {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.wings[1].count = count;
            spec.wings[0].count = count.max(1);
            specs.push(spec);
        }
        for damage in [Damage::Invulnerable, Damage::Normal, Damage::Realistic] {
            for enemy in [None, Some(Experience::Novice), Some(Experience::Ace)] {
                let mut spec = MissionSpec::new("UKR", AircraftId::F18);
                spec.cheats.damage = damage;
                spec.cheats.enemy_ai = enemy;
                specs.push(spec);
            }
        }
        for skill in [None, Some(EnemySkillOverride::AllAverage)] {
            let mut spec = MissionSpec::new("UKR", AircraftId::F18);
            spec.enemy_skill = skill;
            specs.push(spec);
        }
        for ai in [AiFlightModel::Standard, AiFlightModel::AllHybrid] {
            for human in [true, false] {
                let mut spec = MissionSpec::new("UKR", AircraftId::F18);
                spec.ai_flight_model = ai;
                spec.researched_flight = human;
                specs.push(spec);
            }
        }
        for index in 0..WINGS {
            let own = MissionSpec::wing_id(index);
            let mut objectives = vec![
                GroupObjective::Inherit,
                GroupObjective::Free,
                GroupObjective::Cap,
                GroupObjective::SelfDefense,
                GroupObjective::Hold,
            ];
            for other in 0..WINGS {
                let target = MissionSpec::wing_id(other);
                if target.side == own.side && target != own {
                    objectives.push(GroupObjective::Escort(target));
                } else if target.side != own.side {
                    objectives.push(GroupObjective::Intercept(target));
                }
            }
            for objective in objectives {
                let mut spec = MissionSpec::new("UKR", AircraftId::F18);
                spec.objectives[index] = objective;
                spec.must_survive[index] = true;
                specs.push(spec);
            }
        }
        for spec in specs {
            let text = spec.to_text();
            assert_eq!(MissionSpec::from_text(&text).unwrap(), spec, "{text}");
        }
    }

    #[test]
    fn comments_blank_lines_and_spacing_do_not_matter() {
        let spec = MissionSpec::from_text(
            "# a mission\n\n  tore-mission   1  # the version\ntheater UKR\n\twing  friendly 1 F18.PT 1 ace # me\n\n",
        )
        .unwrap();
        assert_eq!(spec.wings[0].skill, Skill::Ace);
        assert_eq!(spec.start, Start::Airborne { altitude_ft: 5_000 });
        assert_eq!(spec.condition, Condition::Clear);
    }

    /// The error a text produces.
    fn refused(text: &str) -> String {
        match MissionSpec::from_text(text) {
            Ok(spec) => panic!("parsed: {spec:?}"),
            Err(error) => error.to_string(),
        }
    }

    const BASE: &str = "tore-mission 1\ntheater UKR\nwing friendly 1 F18.PT 1 ace\n";

    #[test]
    fn a_refusal_names_the_line_and_what_is_wrong() {
        let with = |line: &str| format!("{BASE}{line}\n");
        let cases = [
            ("theater MOON", "line 4: `MOON` is not a theater"),
            ("theater UKR", "line 4: `theater` appears twice"),
            ("condition stormy", "line 4: `stormy` is not a condition"),
            (
                "wing friendly 2 F99.PT 1 ace",
                "line 4: `F99.PT` is not an aircraft",
            ),
            (
                "wing friendly 2 F22N.PT 1 legend",
                "line 4: `legend` is not a skill",
            ),
            (
                "wing friendly 2 F22N.PT 6 ace",
                "line 4: a wing holds 0 to 5 aircraft, not 6",
            ),
            (
                "wing friendly 2 F22N.PT x ace",
                "line 4: the aircraft count must be a whole number",
            ),
            (
                "wing friendly 4 F22N.PT 1 ace",
                "line 4: a wing number is 1 to 3, not 4",
            ),
            (
                "wing neutral 1 F22N.PT 1 ace",
                "line 4: a side is `friendly` or `enemy`",
            ),
            (
                "wing friendly 1 F14.PT 1 ace",
                "line 4: friendly wing 1 appears twice",
            ),
            (
                "wing friendly 2 F22N.PT 1",
                "line 4: expected `wing SIDE N AIRCRAFT COUNT SKILL`",
            ),
            (
                "start airborne 12345",
                "line 4: the airborne altitude is one of 5000, 10000, 20000, 40000 feet",
            ),
            (
                "start ground",
                "line 4: expected `start ground RUNWAY [FEET]`",
            ),
            (
                "start sideways 5",
                "line 4: expected `start airborne FEET` or `start ground RUNWAY`",
            ),
            (
                "separation-nm 7",
                "line 4: the separation is one of 1, 2, 5, 10, 20, 50, 75, 100, 150, 200, 300",
            ),
            ("preset boring", "line 4: `boring` is not a preset"),
            ("guns-only maybe", "line 4: `guns-only` takes yes or no"),
            ("time-of-day 25:00", "line 4: `25:00` is not a time of day"),
            ("time-of-day noon", "line 4: `noon` is not a time of day"),
            (
                "wind 90 500",
                "line 4: the wind heading is -360 to 360 degrees and its speed 0 to 200",
            ),
            (
                "cloud-deck 999999",
                "line 4: the cloud deck is 0 to 400000 feet",
            ),
            (
                "objective enemy 1 intercept enemy 2",
                "line 4: enemy wing 1 cannot intercept a wing on its own side",
            ),
            (
                "objective enemy 1 escort enemy 1",
                "line 4: enemy wing 1 can only escort another wing on its own side",
            ),
            (
                "objective enemy 1 escort friendly 1",
                "line 4: enemy wing 1 can only escort another wing on its own side",
            ),
            (
                "objective enemy 1 cap friendly 1",
                "line 4: `cap` takes no wing",
            ),
            (
                "objective enemy 1 intercept",
                "line 4: `intercept` needs a wing",
            ),
            (
                "objective enemy 1 dance",
                "line 4: `dance` is not an objective",
            ),
            (
                "survive enemy 1 sometimes",
                "line 4: `survive` takes yes or no",
            ),
            (
                "cheats infinite-lives",
                "line 4: `infinite-lives` is not a cheat",
            ),
            ("cheats none no-spins", "line 4: `none` stands alone"),
            (
                "cheats no-spins no-spins",
                "line 4: the cheat `no-spins` is listed twice",
            ),
            (
                "cheats damage=fatal",
                "line 4: `fatal` is not a damage cheat",
            ),
            ("cheats enemy-ai=god", "line 4: `god` is not an enemy level"),
            (
                "flight-model human quantum",
                "line 4: `quantum` is not a flight model for humans",
            ),
            (
                "flight-model ai legacy",
                "line 4: `legacy` is not a flight model for the AI",
            ),
            ("enemy-skill ace", "line 4: `ace` is not an enemy skill"),
            (
                "fixture-wings sure",
                "line 4: `fixture-wings` takes yes or no",
            ),
            ("loadout fuel lots", "line 4: `lots` is not a fuel load"),
            (
                "loadout station 0 M61.JT 500",
                "line 4: expected `loadout station N WEAPON COUNT QUANTITY`",
            ),
            (
                "loadout station 0 M 61.JT 500 500",
                "line 4: expected `loadout station N WEAPON COUNT QUANTITY`",
            ),
            (
                "loadout station 0 M61/JT 500 500",
                "line 4: `M61/JT` is not a weapon resource name",
            ),
            (
                "loadout station 0 M61.JT 70000 1",
                "line 4: the station's capacity is 0 to 65535",
            ),
            ("loadout paint red", "line 4: expected `loadout fuel`"),
            ("colour blue", "line 4: unknown setting `colour`"),
            (
                "tore-mission 1",
                "line 4: `tore-mission` is only the first line",
            ),
        ];
        for (line, expected) in cases {
            let error = refused(&with(line));
            assert!(
                error.starts_with(expected),
                "{line:?} gave {error:?}, expected {expected:?}"
            );
        }
    }

    #[test]
    fn a_file_without_its_header_version_or_essentials_is_refused() {
        assert!(refused("").contains("the mission file is empty"));
        assert!(refused("# nothing\n").contains("the mission file is empty"));
        assert_eq!(
            refused("theater UKR\n"),
            "line 1: the first line must be `tore-mission 1`"
        );
        assert_eq!(
            refused("tore-mission 2\ntheater UKR\n"),
            "line 1: this is mission file version 2, and only version 1 is known"
        );
        assert_eq!(
            refused("tore-mission 1\nwing friendly 1 F18.PT 1 ace\n"),
            "the mission has no `theater` line"
        );
        assert_eq!(
            refused("tore-mission 1\ntheater UKR\n"),
            "the mission has no `wing friendly 1` line"
        );
        assert_eq!(
            refused("tore-mission 1\ntheater UKR\nwing friendly 1 F18.PT 0 ace\n"),
            "line 3: friendly wing 1 (with the player) holds 1 to 5 aircraft, not 0"
        );
    }

    #[test]
    fn the_loadout_needs_every_station_and_its_fuel() {
        let base = format!("{BASE}loadout station 1 M61.JT 500 500\n");
        assert_eq!(refused(&base), "the loadout has no `loadout fuel` line");
        assert_eq!(
            refused(&format!("{base}loadout fuel 100\n")),
            "the loadout has no `loadout station 0` line"
        );
        let spec = MissionSpec::from_text(&format!(
            "{base}loadout fuel 100\nloadout station 0 M61.JT 500 500\n"
        ))
        .unwrap();
        assert_eq!(spec.loadout.unwrap().stations.len(), 2);
    }

    #[test]
    fn a_ground_start_names_its_runway_by_ordinal() {
        let text = format!("{BASE}start ground 12\n");
        let spec = MissionSpec::from_text(&text).unwrap();
        assert_eq!(spec.ground_runway(), Some(RUNWAY_OBJECT_BASE + 12));
        assert_eq!(spec.start.altitude_ft(), DEFAULT_ALTITUDE_FT);
        assert!(spec.to_text().contains("start ground 12\n"));
        let high = MissionSpec::from_text(&format!("{BASE}start ground 12 20000\n")).unwrap();
        assert_eq!(high.start.altitude_ft(), 20_000);
        assert!(high.to_text().contains("start ground 12 20000\n"));
    }

    #[test]
    fn an_enemy_ground_start_round_trips_and_is_checked() {
        let red = format!("{BASE}wing enemy 1 MIG29.PT 2 average\n");
        let spec = MissionSpec::from_text(&format!("{red}enemy-start ground 7\n")).unwrap();
        assert_eq!(
            spec.enemy_start,
            EnemyStart::Ground {
                runway: RUNWAY_OBJECT_BASE + 7
            }
        );
        assert!(spec.to_text().contains("enemy-start ground 7\n"));
        assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
        assert!(spec.summary().contains("enemy ground start"));
        let auto = MissionSpec::from_text(&format!("{red}enemy-start ground auto\n")).unwrap();
        assert_eq!(auto.enemy_start, EnemyStart::GroundAuto);
        assert!(auto.to_text().contains("enemy-start ground auto\n"));
        // Airborne is the default and writes no line.
        let air = MissionSpec::from_text(&format!("{red}enemy-start airborne\n")).unwrap();
        assert_eq!(air.enemy_start, EnemyStart::Airborne);
        assert!(!air.to_text().contains("enemy-start"));
        for (text, words) in [
            (
                format!("{BASE}enemy-start ground auto\n"),
                "enemy wing 1, which has no aircraft",
            ),
            (
                format!("{red}enemy-start ground\n"),
                "expected `enemy-start airborne`",
            ),
            (format!("{red}enemy-start ground x\n"), "the runway number"),
            (
                format!("{red}start ground 7\nenemy-start ground 7\n"),
                "the player's runway",
            ),
            (
                format!("{red}enemy-start ground 1\nenemy-start ground 2\n"),
                "`enemy-start`",
            ),
        ] {
            let error = MissionSpec::from_text(&text).unwrap_err().to_string();
            assert!(error.contains(words), "{text:?}: {error}");
        }
    }

    /// The text of a plain mission, written before the surface settings
    /// existed: it must not change (the mission hash is the text's).
    #[test]
    fn a_mission_without_a_ground_target_writes_the_text_it_always_did() {
        let spec = MissionSpec::new("EGY", AircraftId::F18);
        assert_eq!(
            spec.to_text(),
            "tore-mission 1\ntheater EGY\ncondition clear\nstart airborne 5000\n\
             separation-nm 5\npreset free\nguns-only no\nwing friendly 1 F18.PT 1 average\n\
             cheats none\nflight-model human hybrid\nflight-model ai hybrid\n"
        );
        let text = MissionSpec::from_text(&guide_example()).unwrap().to_text();
        for key in [
            "ground-target",
            "defenses",
            "surface-seed",
            "enemy-nationality",
            "ground auto",
            "enemy-start",
        ] {
            assert!(!text.contains(key), "{key}");
        }
    }

    #[test]
    fn a_strike_objective_round_trips_and_needs_a_ground_target_and_a_friendly_ai_wing() {
        let mut spec = MissionSpec::new("KURILE", AircraftId::F18);
        spec.wings[1] = WingSpec {
            aircraft: AircraftId::F18,
            count: 2,
            skill: Skill::Average,
        };
        spec.ground_target = Some("QKSILO".into());
        spec.objectives[1] = GroupObjective::Strike;
        spec.validate().unwrap();
        let text = spec.to_text();
        assert!(text.contains("\nobjective friendly 2 strike\n"), "{text}");
        assert_eq!(MissionSpec::from_text(&text).unwrap(), spec);
        // Without a ground target there is nothing to strike.
        let mut none = spec.clone();
        none.ground_target = None;
        assert!(
            none.validate()
                .unwrap_err()
                .to_string()
                .contains("no ground target")
        );
        // The player's own wing and Redfor's wings do not strike.
        let mut own = spec.clone();
        own.objectives = [GroupObjective::Inherit; WINGS];
        own.objectives[0] = GroupObjective::Strike;
        assert!(own.validate().is_err());
        let mut red = spec.clone();
        red.objectives = [GroupObjective::Inherit; WINGS];
        red.objectives[3] = GroupObjective::Strike;
        assert!(red.validate().unwrap_err().to_string().contains("Redfor"));
        assert!(MissionSpec::from_text("objective friendly 2 strike friendly 1").is_err());
    }

    #[test]
    fn a_ground_target_its_defenses_seed_and_enemy_round_trip() {
        let mut spec = MissionSpec::new("KURILE", AircraftId::F18);
        spec.ground_target = Some("QKSILO".into());
        spec.aaa = Defense::Heavy;
        spec.sam = Defense::Light;
        spec.surface_seed = 1_234_567;
        spec.enemy_nationality = 33;
        spec.validate().unwrap();
        let text = spec.to_text();
        assert!(text.contains("\nground-target QKSILO\n"), "{text}");
        assert!(text.contains("\ndefenses aaa heavy sam light\n"), "{text}");
        assert!(text.contains("\nsurface-seed 1234567\n"), "{text}");
        assert!(text.contains("\nenemy-nationality 33\n"), "{text}");
        assert_eq!(MissionSpec::from_text(&text).unwrap(), spec);
        // Every defense pair, and a target with no defenses, which writes no
        // `defenses` line and reads back as none.
        for aaa in Defense::ALL {
            for sam in Defense::ALL {
                spec.aaa = aaa;
                spec.sam = sam;
                let text = spec.to_text();
                let has_line = text.contains("defenses");
                assert_eq!(has_line, aaa != Defense::None || sam != Defense::None);
                assert_eq!(MissionSpec::from_text(&text).unwrap(), spec, "{text}");
            }
        }
        // The theater's own enemy is the default and is not written.
        spec.enemy_nationality = default_enemy_nationality("KURILE");
        assert!(!spec.to_text().contains("enemy-nationality"));
        assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
    }

    #[test]
    fn every_theaters_targets_and_enemy_come_from_the_retail_tables() {
        use tore_formats::quick_template::tables::{ENEMY_NATIONALITY, TEMPLATES};
        for (index, theater) in THEATERS.into_iter().enumerate() {
            let offered = ground_targets(theater);
            assert_eq!(offered.len() + 1, TEMPLATES[index].len(), "{theater}");
            assert!(!offered.is_empty() && !offered.iter().any(|s| s.ends_with("NOTH")));
            assert_eq!(
                usize::from(default_enemy_nationality(theater)),
                ENEMY_NATIONALITY[index]
            );
            for stem in offered {
                let mut spec = MissionSpec::new(theater, AircraftId::F18);
                spec.ground_target = Some((*stem).into());
                spec.aaa = Defense::Moderate;
                spec.validate()
                    .unwrap_or_else(|e| panic!("{theater} {stem}: {e}"));
                assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
            }
        }
        // A layout variant offers its base theater's targets.
        assert_eq!(ground_targets("~UKR1"), ground_targets("UKR"));
        assert!(ground_targets("NOWHERE").is_empty());
    }

    #[test]
    fn a_ground_target_is_refused_for_the_wrong_theater_or_the_nothing_entry() {
        let target = |text: &str| refused(&format!("{BASE}{text}"));
        assert!(target("ground-target QCSCUD\n").contains("not a ground target of UKR"));
        assert!(target("ground-target QUNOTH\n").contains("not a ground target"));
        assert!(target("ground-target\n").contains("expected `ground-target TEMPLATE`"));
        // Defenses without a target are the enemy's area defenses (B23).
        let area = MissionSpec::from_text(&format!("{BASE}defenses aaa heavy sam none\n")).unwrap();
        assert!(area.area_defenses() && area.lays_out_surface());
        assert_eq!(MissionSpec::from_text(&area.to_text()).unwrap(), area);
        let with = |text: &str| refused(&format!("{BASE}ground-target QUCOL\n{text}"));
        assert!(with("defenses aaa heavily sam none\n").contains("not a defense level"));
        assert!(with("defenses sam heavy aaa none\n").contains("expected `defenses aaa"));
        assert!(with("defenses aaa heavy\n").contains("expected `defenses aaa"));
        assert!(with("defenses aaa none sam none\ndefenses aaa none sam none\n").contains("twice"));
        assert!(with("surface-seed -1\n").contains("whole number"));
        assert!(with("surface-seed 4294967296\n").contains("whole number"));
        assert!(with("enemy-nationality 60\n").contains("0 to 59"));
        assert!(with("enemy-nationality x\n").contains("0 to 59"));
        assert!(with("ground-target QUCOL\n").contains("twice"));
        // Built in code, the same rules hold.
        let mut spec = MissionSpec::new("UKR", AircraftId::F18);
        spec.sam = Defense::Light;
        assert!(spec.validate().is_ok() && spec.area_defenses());
        spec.sam = Defense::None;
        assert!(!spec.area_defenses() && !spec.lays_out_surface());
        spec.enemy_nationality = 60;
        assert!(spec.validate().is_err());
    }

    #[test]
    fn a_ground_start_may_leave_its_runway_to_the_world() {
        let spec = MissionSpec::from_text(&format!("{BASE}start ground auto\n")).unwrap();
        assert!(spec.ground_auto() && spec.start.on_ground());
        assert_eq!(spec.ground_runway(), None);
        assert_eq!(spec.start.altitude_ft(), DEFAULT_ALTITUDE_FT);
        assert!(spec.to_text().contains("start ground auto\n"));
        let high = MissionSpec::from_text(&format!("{BASE}start ground auto 20000\n")).unwrap();
        assert_eq!(
            high.start,
            Start::GroundAuto {
                altitude_ft: 20_000
            }
        );
        assert!(high.to_text().contains("start ground auto 20000\n"));
        assert_eq!(MissionSpec::from_text(&high.to_text()).unwrap(), high);
        assert!(high.summary().contains("ground start"));
        assert!(refused(&format!("{BASE}start ground auto 123\n")).contains("altitude"));
        assert!(refused(&format!("{BASE}start ground automatic\n")).contains("runway number"));
        assert!(refused(&format!("{BASE}start ground\n")).contains("RUNWAY [FEET]"));
    }

    #[test]
    fn every_selectable_aircraft_may_fill_a_wing_the_ai_flies() {
        // Slice H3: the CH-47, the V-22, the AV-8 and the Yak-141 joined the
        // AH-64 and the Mi-24 as aircraft the AI flies, so a mission file or a
        // lobby spec may put any of them in a wing other than friendly wing 1.
        for id in AircraftId::SELECTABLE {
            // A person may fly any aircraft, alone or leading friendly wing 1.
            let mut spec = MissionSpec::new("UKR", id);
            spec.validate().unwrap();
            spec.wings[3].count = 0;
            spec.wings[0].count = 1;
            spec.wings[1].aircraft = id;
            spec.wings[1].count = 2;
            spec.wings[4].aircraft = id;
            spec.wings[4].count = 1;
            assert!(id.ai_flyable(), "{id:?}");
            spec.validate().unwrap();
            // An empty wing names no aircraft that matters.
            spec.wings[1].count = 0;
            spec.wings[4].count = 0;
            spec.validate().unwrap();
        }
    }

    #[test]
    fn a_spec_built_in_code_is_validated_like_a_parsed_one() {
        let mut spec = MissionSpec::new("UKR", AircraftId::F18);
        spec.validate().unwrap();
        spec.wings[0].count = 0;
        assert!(
            spec.validate()
                .unwrap_err()
                .to_string()
                .contains("friendly wing 1")
        );
        spec.wings[0].count = 1;
        spec.separation_nm = 3;
        assert!(spec.validate().is_err());
        spec.separation_nm = 5;
        spec.weather.time = Some([24, 0]);
        assert!(spec.validate().is_err());
        spec.weather.time = None;
        spec.objectives[0] = GroupObjective::Intercept(MissionSpec::wing_id(1));
        assert!(spec.validate().is_err());
        spec.objectives[0] = GroupObjective::Inherit;
        spec.theater = "no way".into();
        assert!(spec.validate().is_err());
        // A `~` layout variant is for the game's developer options.
        spec.theater = "~UKR1".into();
        spec.validate().unwrap();
    }

    #[test]
    fn friendly_wing_one_gives_a_slot_to_the_player() {
        let mut spec = MissionSpec::new("UKR", AircraftId::F18);
        spec.wings[0].count = 4;
        spec.wings[3].count = 2;
        let launches = spec.wing_launches().unwrap();
        let counts: Vec<usize> = launches.iter().map(|wing| wing.members.len()).collect();
        assert_eq!(counts, [3, 0, 0, 2, 0, 0]);
        assert_eq!(spec.player_wing_size(), 4);
        assert_eq!(spec.player(), AircraftId::F18);
    }

    #[test]
    fn friendly_fire_and_the_loadout_rule_are_lines_only_when_not_the_default() {
        // Slice F2-1: a single-player spec's text does not change.
        let spec = MissionSpec::new("UKR", AircraftId::F18);
        let text = spec.to_text();
        assert!(!text.contains("friendly-fire") && !text.contains("loadouts"));
        assert!(spec.friendly_fire && !spec.cheat_loadouts);
        let mut networked = spec.clone();
        networked.friendly_fire = false;
        networked.cheat_loadouts = true;
        let text = networked.to_text();
        assert!(text.contains("\nfriendly-fire off\n") && text.contains("\nloadouts any\n"));
        assert_eq!(MissionSpec::from_text(&text).unwrap(), networked);
        let on =
            MissionSpec::from_text(&format!("{BASE}friendly-fire on\nloadouts own\n")).unwrap();
        assert!(on.friendly_fire && !on.cheat_loadouts);
        assert_eq!(
            refused(&format!("{BASE}friendly-fire maybe\n")),
            "line 4: `friendly-fire` is `on` or `off`, not `maybe`"
        );
        assert_eq!(
            refused(&format!("{BASE}loadouts cheat\n")),
            "line 4: `loadouts` is `own` or `any`, not `cheat`"
        );
        assert_eq!(
            refused(&format!("{BASE}loadouts any\nloadouts own\n")),
            "line 5: `loadouts` appears twice"
        );
    }

    #[test]
    fn a_cheat_loading_passes_only_under_the_any_rule_and_its_own_capacities() {
        let map = crate::test_support::resources::resources();
        let player = crate::aircraft_type::AircraftType::load(&map, AircraftId::F18).unwrap();
        let standard = LoadoutSpec::of(
            &Loadout::new(&player.profile, |name| {
                map.get(name)
                    .cloned()
                    .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
            })
            .unwrap(),
        );
        let mut cheat = standard.clone();
        cheat.cheat = true;
        let mut mission = MissionSpec::new("UKR", AircraftId::F18);
        let check = |load: &LoadoutSpec, mission: &MissionSpec| {
            load.check_in(&player.profile, &map, mission)
                .map(|_| ())
                .map_err(|error| error.to_string())
        };
        assert_eq!(
            check(&cheat, &mission),
            Err("Cheat loading is not allowed in a multiplayer game.".into())
        );
        assert_eq!(check(&standard, &mission), Ok(()));
        mission.cheat_loadouts = true;
        assert_eq!(check(&cheat, &mission), Ok(()));
        assert_eq!(check(&standard, &mission), Ok(()));
        // The page's cheat capacities still bind.
        let mut over = cheat.clone();
        over.stations[1].quantity = u16::MAX;
        assert!(check(&over, &mission).is_err());
        // Guns only comes from the mission too.
        mission.guns_only = true;
        assert!(check(&cheat, &mission).unwrap_err().contains("Guns only"));
        // The single-player call keeps its rule.
        assert!(cheat.check_for_plane(&player.profile, &map, false).is_err());
    }

    #[test]
    fn friendly_fire_off_builds_into_an_open_missions_combat_and_single_player_refuses_it() {
        use crate::resources::ResourceReads;
        use crate::world::{Seating, World};
        use tore_sim::combat::live::FriendlyFire;
        let map = crate::test_support::resources::resources();
        let mut spec = MissionSpec::new(crate::test_support::resources::THEATER, AircraftId::F18);
        spec.wings[3].count = 1;
        let reads = ResourceReads::new(&map);
        let world = World::new(&spec, &reads, Seating::Open).unwrap();
        assert_eq!(world.combat.state.friendly_fire, FriendlyFire::On);
        spec.friendly_fire = false;
        let world = World::new(&spec, &reads, Seating::Open).unwrap();
        assert_eq!(world.combat.state.friendly_fire, FriendlyFire::Off);
        let refused = World::new(&spec, &reads, Seating::SinglePlayer)
            .err()
            .unwrap()
            .to_string();
        assert!(
            refused.contains("multiplayer mission's settings"),
            "{refused}"
        );
        spec.friendly_fire = true;
        spec.cheat_loadouts = true;
        assert!(World::new(&spec, &reads, Seating::SinglePlayer).is_err());
    }
}
