//! The host's answers on a striker's guided shots from farther out
//! (docs/spec/ai-strike.md, "Stand-off reach"; John, 2026-10-29), the
//! [`ShotTable`] the strike tactics plan a stand-off release with.
//!
//! Each answer is the question the AI's own release gate asks, for a static
//! target on flat ground from a level launch toward it:
//!
//! - under the energy rules, a store with a motor: the energy model's
//!   exact-point answer ([`live::State::energy_trial`], the question the
//!   prediction service answers for the gate);
//! - a guided bomb: its glide reach from that launch ([`guided::reach`],
//!   the sight's);
//! - under the other rules, a seeker missile: its zone 1; a guided missile
//!   on the sight: its zone out to the sight's release bar, where the AI's
//!   gate lets it go (a helicopter gets no table under these rules).
//!
//! Answers are cached by store, launch band and range step on [`ShotCache`]
//! (on [`super::AiWings`], never checkpointed: each is a fixed function of
//! its key), so a plan asked again every step costs a lookup.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use tore_formats::weapons::Weapon;
use tore_sim::ai::strike::{ShotLaunch, ShotTable, StoreKind, reach::SHOT_STEP_FT};
use tore_sim::ai::weapon_service::StationId;
use tore_sim::combat::guided;
use tore_sim::combat::live;
use tore_sim::combat::missiles::{self, seeker};

/// The launch bands an answer is cached by (fitted, agent): the target's
/// height in steps of [`TARGET_BAND_FT`]; the launch's height over it in
/// steps of [`LOW_BAND_FT`] up to [`LOW_BANDS_TO_FT`] (where a helicopter's
/// shot changes most, and the AT-2's ground band lies), then
/// [`HIGH_BAND_FT`]; the speed in steps of [`SPEED_BAND_FPS`].
pub(super) const TARGET_BAND_FT: f64 = 250.;
pub(super) const LOW_BAND_FT: f64 = 250.;
pub(super) const LOW_BANDS_TO_FT: f64 = 2_000.;
pub(super) const HIGH_BAND_FT: f64 = 1_000.;
pub(super) const SPEED_BAND_FPS: f64 = 50.;
/// The target's centre over the ground, feet (a vehicle's, as
/// [`super::strike`]'s flight times take it).
const TARGET_OVER_GROUND_FT: f64 = 8.;

/// An answer's key: the record, the rules, the launch's bands and the range
/// step.
type Key = (String, u8, i64, i64, i64, i64);

/// The cached answers, shared by every striker's table.
#[derive(Default)]
pub(crate) struct ShotCache {
    answers: Mutex<BTreeMap<Key, Option<f64>>>,
    weapons: Mutex<BTreeMap<String, Arc<Weapon>>>,
}

impl ShotCache {
    /// The shared copy of record `w` (by its source).
    fn weapon(&self, w: &Weapon) -> Arc<Weapon> {
        let mut weapons = self.weapons.lock().unwrap_or_else(PoisonError::into_inner);
        weapons
            .entry(w.source.clone())
            .or_insert_with(|| Arc::new(w.clone()))
            .clone()
    }

    /// How many answers are cached (for the tests).
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// One striker's table for one step: its guided stations.
pub(super) struct StrikerShots {
    cache: Arc<ShotCache>,
    rules: missiles::Rules,
    stations: Vec<(StationId, StoreKind, Arc<Weapon>)>,
}

impl StrikerShots {
    /// The table for a striker's stores (`(station, kind, record)`), or
    /// `None` when none of them is a guided store a stand-off plans with.
    pub(super) fn new<'a>(
        cache: &Arc<ShotCache>,
        rules: missiles::Rules,
        stores: impl Iterator<Item = (StationId, StoreKind, &'a Weapon)>,
    ) -> Option<Self> {
        let stations: Vec<_> = stores
            .filter(|(_, kind, _)| planned(*kind))
            .map(|(station, kind, w)| (station, kind, cache.weapon(w)))
            .collect();
        (!stations.is_empty()).then(|| Self {
            cache: cache.clone(),
            rules,
            stations,
        })
    }
}

/// The kinds a stand-off release is planned for: the guided and seeker
/// stores.
fn planned(kind: StoreKind) -> bool {
    !kind.unguided()
}

fn band(value: f64, step: f64) -> i64 {
    (value / step).round() as i64
}

/// The launch height's band, and the height it stands for.
fn height_band(height_ft: f64) -> (i64, f64) {
    if height_ft.abs() <= LOW_BANDS_TO_FT {
        let b = band(height_ft, LOW_BAND_FT);
        (b, b as f64 * LOW_BAND_FT)
    } else {
        let b = band(height_ft, HIGH_BAND_FT);
        // Clear of the low bands' numbers.
        (b * 1_000, b as f64 * HIGH_BAND_FT)
    }
}

fn rules_code(rules: missiles::Rules) -> u8 {
    match rules {
        missiles::Rules::Compatibility => 0,
        missiles::Rules::Spec => 1,
        missiles::Rules::Energy => 2,
    }
}

impl ShotTable for StrikerShots {
    fn shot(&self, station: StationId, launch: ShotLaunch, range_ft: f64) -> Option<f64> {
        let (_, kind, w) = self.stations.iter().find(|(s, _, _)| *s == station)?;
        let target = band(launch.target_ft, TARGET_BAND_FT);
        let (height, height_ft) = height_band(launch.height_ft);
        let speed = band(launch.speed_fps.max(0.), SPEED_BAND_FPS);
        let step = band(range_ft, SHOT_STEP_FT).max(1);
        let key = (
            w.source.clone(),
            rules_code(self.rules),
            target,
            height,
            speed,
            step,
        );
        if let Some(answer) = self
            .cache
            .answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            return *answer;
        }
        let quantised = Launch {
            target_ft: target as f64 * TARGET_BAND_FT,
            height_ft,
            speed_fps: speed as f64 * SPEED_BAND_FPS,
            range_ft: step as f64 * SHOT_STEP_FT,
        };
        let answer = answer(w, *kind, self.rules, quantised);
        self.cache
            .answers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, answer);
        answer
    }
}

/// A launch on the bands, and the range asked.
#[derive(Clone, Copy, Debug)]
pub(super) struct Launch {
    pub target_ft: f64,
    pub height_ft: f64,
    pub speed_fps: f64,
    pub range_ft: f64,
}

/// A store's answer (see the module notes): seconds of flight when it hits.
pub(super) fn answer(
    w: &Weapon,
    kind: StoreKind,
    rules: missiles::Rules,
    at: Launch,
) -> Option<f64> {
    let zone = w.seeker.zones[1];
    let (minimum, maximum) = (
        f64::from(zone.minimum_range.max(0)),
        f64::from(zone.maximum_range.max(0)),
    );
    let slant = at.range_ft.hypot(at.height_ft);
    if maximum <= 0. || slant > maximum || slant < minimum {
        return None;
    }
    let ground = at.target_ft - TARGET_OVER_GROUND_FT;
    let launcher = live::Launcher {
        radar_power: true,
        position: [0., at.target_ft + at.height_ft, 0.],
        basis: tore_sim::attitude::Basis::new(0., 0., 0.),
        speed_fps: at.speed_fps,
        velocity: [0., 0., at.speed_fps],
        bay_ready: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: Default::default(),
    };
    let powered = w.movement.fuel_t > w.movement.ignite_t;
    if rules == missiles::Rules::Energy && powered {
        let observed = seeker::Observation {
            id: u32::MAX,
            position: [0., at.target_ft, at.range_ft],
            velocity: [0.; 3],
            quality: 1.,
            range: slant,
            off_axis: 0.,
        };
        return live::State::energy_trial(w, launcher, observed, [0.; 3], &|_, _| ground)
            .filter(|answer| answer.hit)
            .map(|answer| answer.seconds);
    }
    let top = f64::from(w.movement.maximum_speed.max(1));
    if kind.seeker() {
        return Some(slant / top);
    }
    if guided::is_glide_bomb(w) {
        let reach = guided::reach(w, &launcher, ground, 1.)?;
        return (at.range_ft >= reach.minimum_ft && at.range_ft <= reach.maximum_ft)
            .then(|| at.range_ft / at.speed_fps.max(GLIDE_FLOOR_FPS));
    }
    // A guided missile on the sight leaves no farther out than its bar.
    let bar = maximum - guided::LEVEL_DEPTH * (maximum - minimum);
    (at.range_ft <= bar).then(|| slant / top)
}

/// A guided bomb's time of flight is its range at the launch speed, at
/// least this, ft/s (fitted, agent: only the striker's wait for its round
/// reads it).
const GLIDE_FLOOR_FPS: f64 = 300.;

#[cfg(test)]
#[path = "strike_reach_tests.rs"]
mod tests;
