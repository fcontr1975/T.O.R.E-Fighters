//! Aircraft and surface AI: isolated, spec-derived calculation components.
//!
//! Everything here is built from [`docs/spec/ai.md`](../../../../docs/spec/ai.md)
//! [`docs/spec/ai-experience.md`](../../../../docs/spec/ai-experience.md), and
//! [`docs/spec/ai-awareness.md`](../../../../docs/spec/ai-awareness.md).
//! Behavior IDs (B01, B15, ...) in doc comments refer to those specs. Nothing in
//! this module drives the human player. The mission and steering adapters
//! connect aircraft decisions to physical controls, with synthetic replay tests.
//!
//! Rules of the module:
//!
//! - Unknown behavior returns [`AiError::UnspecifiedRule`]; there are no silent
//!   fallback constants.
//! - Recovered quantities keep their recovered domain. Distances are feet,
//!   angles in recovered rules are degrees, and speeds are feet per second
//!   wrapped in [`ScalarSpeed`].
//! - Randomness is caller-owned [`DecisionRandom`] state. Percentages are draw
//!   thresholds, not a promise to reproduce the original sequence.
//! - The simulation runs at a fixed 120 Hz; nominal timings are simulation
//!   seconds on a quarter-second clock ([`QUARTER_SECOND_TICKS`]).
pub mod airfield;
// Exact checkpoints (docs/formats/checkpoint.md).
pub mod awareness;
mod checkpoint;
pub mod controller;
pub mod damage;
pub mod defense;
pub mod engagement;
pub mod experience;
pub mod fitted;
pub mod formation;
pub mod geometry;
pub mod gunnery;
pub mod helicopter_ahead;
pub mod helicopter_defense;
pub mod helicopter_threat;
pub mod incoming_fire;
pub mod jet_lift;
pub mod launch;
pub mod link;
pub mod mission;
pub mod motion;
pub mod opportunity;
pub mod pursuit;
pub mod rotor_adapter;
mod rotor_checkpoint;
pub mod rotor_probe;
pub mod route;
pub mod steering;
pub mod steering_adapter;
pub mod strike;
pub mod surface;
pub mod tactics;
pub mod targeting;
pub mod test_settings;
pub mod thought;
pub mod threat;
pub mod threat_route;
pub mod weapon_service;
pub mod wing;

use std::fmt;

/// Fixed simulation rate; the host steps at exactly this many ticks per second.
pub const TICKS_PER_SECOND: u64 = 120;
/// B13: nominal AI timings are evaluated on a quarter-second clock.
pub const QUARTER_SECOND_TICKS: u64 = TICKS_PER_SECOND / 4;

/// Errors from AI calculations. `UnspecifiedRule` names a branch the spec has
/// not closed; callers must surface it rather than substitute a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiError {
    /// The spec records this branch as unknown or unresolved.
    UnspecifiedRule(&'static str),
    /// A caller supplied something outside the specified input domain.
    InvalidInput(&'static str),
}
impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnspecifiedRule(what) => write!(f, "unspecified AI rule: {what}"),
            Self::InvalidInput(what) => write!(f, "invalid AI input: {what}"),
        }
    }
}
impl std::error::Error for AiError {}
pub type Result<T> = std::result::Result<T, AiError>;

/// One aircraft's resolved experience level (AI experience spec, "Experience
/// channels"). Four levels, one per object; not a difficulty multiplier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Experience {
    Novice = 0,
    Average = 1,
    Experienced = 2,
    Ace = 3,
}
impl Experience {
    pub const ALL: [Self; 4] = [Self::Novice, Self::Average, Self::Experienced, Self::Ace];
    /// Level 0..3; anything else is invalid input, never clamped silently.
    pub fn from_level(level: i32) -> Result<Self> {
        match level {
            0 => Ok(Self::Novice),
            1 => Ok(Self::Average),
            2 => Ok(Self::Experienced),
            3 => Ok(Self::Ace),
            _ => Err(AiError::InvalidInput("experience level outside 0..3")),
        }
    }
    pub fn level(self) -> u8 {
        self as u8
    }
    /// Editor and tactical tables index by level.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Scalar speed in the AI rules (B04, B15): feet per second, executable
/// confirmed on 2026-09-17 (the HUD shows the same value in knots). Kept as a
/// newtype so speeds cannot be mixed with distances or rates by accident.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct ScalarSpeed(pub f64);
impl ScalarSpeed {
    pub fn plus(self, delta: f64) -> Self {
        Self(self.0 + delta)
    }
    pub fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
    pub fn min(self, other: Self) -> Self {
        Self(self.0.min(other.0))
    }
}

/// Own aircraft speed limits in the recovered domain, as queried by the AI.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpeedLimits {
    pub minimum: ScalarSpeed,
    pub maximum: ScalarSpeed,
    pub corner: ScalarSpeed,
}

/// Deterministic, caller-owned draw state for AI decisions. This is a host
/// generator (SplitMix64), deliberately not the retail generator: the specs
/// give draw thresholds, not sequences.
///
/// Every draw is also written to a bounded [`DrawLog`], a write-only record
/// for the replay debug panels that no decision reads. Equality compares the
/// generator state only, so the log never makes two generators differ.
#[derive(Clone, Debug)]
pub struct DecisionRandom {
    state: u64,
    log: DrawLog,
}
impl PartialEq for DecisionRandom {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
    }
}
impl Eq for DecisionRandom {}
impl DecisionRandom {
    pub fn seeded(seed: u64) -> Self {
        Self {
            state: seed,
            log: DrawLog::default(),
        }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// One uniform draw in `0..bound`, written to the log. A zero bound
    /// returns zero without a draw and without a log entry.
    #[track_caller]
    fn logged(&mut self, bound: u32, threshold: Option<u32>, offset: i32) -> u32 {
        let site = self.log.pending_site.take();
        if bound == 0 {
            return 0;
        }
        let value = (self.next_u64() % u64::from(bound)) as u32;
        self.log.push(Draw {
            site,
            location: std::panic::Location::caller(),
            bound,
            value,
            offset,
            threshold,
        });
        value
    }
    /// Uniform draw in `0..bound`; a zero bound returns zero without a draw.
    #[track_caller]
    pub fn below(&mut self, bound: u32) -> u32 {
        self.logged(bound, None, 0)
    }
    /// Uniform draw in `0..=99`, the domain of every recovered percentage rule.
    #[track_caller]
    pub fn percent(&mut self) -> u8 {
        self.below(100) as u8
    }
    /// True for exactly `threshold` of 100 draws (`draw < threshold`). Spell
    /// inclusive source comparisons such as `random 100 > 75` as 76 explicitly.
    #[track_caller]
    pub fn chance(&mut self, threshold: u8) -> bool {
        (self.logged(100, Some(u32::from(threshold)), 0) as u8) < threshold
    }
    /// Equal-probability choice of one of `n` outcomes, 0-based.
    #[track_caller]
    pub fn choose(&mut self, n: u32) -> u32 {
        self.below(n)
    }
    /// Signed draw in `low..=high`.
    #[track_caller]
    pub fn range(&mut self, low: i32, high: i32) -> i32 {
        debug_assert!(low <= high);
        low + self.logged((high - low + 1) as u32, None, low) as i32
    }
    /// Name what the next draw decides, for the draw log only. The label is
    /// consumed by that draw; it never changes a value.
    pub fn site(&mut self, label: &'static str) -> &mut Self {
        self.log.pending_site = Some(label);
        self
    }
    /// The draws since the log was last cleared, oldest first.
    pub fn log(&self) -> &DrawLog {
        &self.log
    }
    /// Forget the logged draws. Owners call this at the start of each
    /// advancing tick; it never touches the generator state.
    pub fn clear_log(&mut self) {
        self.log.clear();
    }
}

/// The most draws one [`DrawLog`] keeps. A generator whose owner never clears
/// its log keeps its latest draws and counts the older ones it dropped.
pub const DRAW_LOG_LIMIT: usize = 64;

/// One logged draw. The draw passes a percentage roll when `value` is below
/// `threshold`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// What the draw decided, when the caller named it.
    pub site: Option<&'static str>,
    /// The source line that made the draw.
    pub location: &'static std::panic::Location<'static>,
    /// The draw is uniform in `0..bound` (100 for a percentage roll).
    pub bound: u32,
    /// The raw draw, in `0..bound`.
    pub value: u32,
    /// Added to `value` before the caller used it (the low end of a signed
    /// range, otherwise 0).
    pub offset: i32,
    /// The percentage the draw was compared with, when the caller said.
    pub threshold: Option<u32>,
}

impl Draw {
    /// The number the caller received.
    pub fn result(&self) -> i64 {
        i64::from(self.offset) + i64::from(self.value)
    }
    /// For a percentage roll, whether it passed.
    pub fn passed(&self) -> Option<bool> {
        self.threshold.map(|threshold| self.value < threshold)
    }
}

/// Write-only record of the draws a [`DecisionRandom`] made since its owner
/// last cleared it.
#[derive(Clone, Debug, Default)]
pub struct DrawLog {
    draws: std::collections::VecDeque<Draw>,
    dropped: u64,
    pending_site: Option<&'static str>,
}

impl DrawLog {
    fn push(&mut self, draw: Draw) {
        if self.draws.len() == DRAW_LOG_LIMIT {
            self.draws.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.draws.push_back(draw);
    }
    fn clear(&mut self) {
        self.draws.clear();
        self.dropped = 0;
        self.pending_site = None;
    }
    /// The logged draws, oldest first.
    pub fn draws(&self) -> impl Iterator<Item = &Draw> {
        self.draws.iter()
    }
    pub fn len(&self) -> usize {
        self.draws.len()
    }
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }
    /// Older draws discarded because the log was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experience_levels_are_bounded() {
        assert_eq!(Experience::from_level(0), Ok(Experience::Novice));
        assert_eq!(Experience::from_level(3), Ok(Experience::Ace));
        assert!(Experience::from_level(4).is_err());
        assert!(Experience::from_level(-1).is_err());
    }

    #[test]
    fn random_is_deterministic_and_bounded() {
        let mut a = DecisionRandom::seeded(7);
        let mut b = DecisionRandom::seeded(7);
        for _ in 0..1000 {
            let x = a.percent();
            assert_eq!(x, b.percent());
            assert!(x < 100);
        }
        assert_eq!(a.below(0), 0);
        for _ in 0..200 {
            let r = a.range(-15, 14);
            assert!((-15..=14).contains(&r));
        }
    }

    #[test]
    fn chance_threshold_frequency_is_nominal() {
        let mut r = DecisionRandom::seeded(99);
        let hits = (0..100_000).filter(|_| r.chance(76)).count();
        assert!((75_000..77_000).contains(&hits), "{hits}");
        let mut r = DecisionRandom::seeded(3);
        assert!((0..1000).all(|_| !r.chance(0)));
        assert!((0..1000).all(|_| r.chance(100)));
    }
}

pub mod jettison;
