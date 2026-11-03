//! The surface engagement controller: one SAM launcher's, AAA gun's, flak
//! battery's, ship mount's or SAM battery's search, preparation, lock and fire
//! cycle (docs/spec/surface-defenses.md, "Engagement").
//!
//! Like the B42 weapon service it is host-agnostic: every fact it needs from
//! the world (which hostile aircraft are eligible, nearest first; whether the
//! launch gates pass for the chosen one; the rounds loaded and in reserve;
//! whether a battery's radar is lost) is an explicit [`Inputs`] field, and it
//! answers with at most one [`FireRequest`] and a magazine swap per tick. The
//! host realises the rounds, debits the magazine and owns the radar.
//!
//! ```text
//! Idle -> Search -> Prepare -> Track -> Fire -> Pause -> Track ...
//!                                         \-> Reload (guns) -> Track | Empty
//!                                         \-> Empty (last missile)
//! any -> Blind (battery radar lost) -> Search
//! ```
//!
//! Times come from the unit's NPC block and the weapon record (retail, in
//! quarter seconds) scaled by the unit's experience ([`delay_percent`]) and
//! are kept here in 120 Hz ticks. Rules the spec marks fitted, in one place:
//!
//! - The first search happens as the controller leaves Idle; the search
//!   period is the retry when nothing is eligible.
//! - The unready (first engagement) preparation time applies to a
//!   controller's first Prepare, the ordinary one afterwards.
//! - A lost target (no longer eligible) returns to Search at once; launch
//!   gates that fail hold the lock for up to 15 s, then Search.
//! - Search returns to Idle after 30 s with no hostile in detection range.
//! - Gun bursts spread their rounds evenly over the burst time; the opening
//!   barrage (`startupShots`) of a controller's first burst spaces its rounds
//!   by the burst's round spacing, at least one quarter second apart, or by
//!   the gun's own opening gap (flak: 0.75 s, John 2026-10-14).
//! - Flak (a controller with a retarget period) moves to the first eligible
//!   target (the host's order) every retarget period; every other controller
//!   keeps its target while it stays eligible.
//! - A controller told to hold ([`Inputs::hold`], a flak team waiting for a
//!   group) keeps its lock in Track and fires nothing; the launch gates'
//!   15 s window does not run meanwhile.
//! - A controller with a lay ([`Inputs::lay`], a tank gun on an aircraft)
//!   holds the target inside the gates that long before its first round,
//!   and starts the lay again whenever the gates fail.
//!
//! A target may be an aircraft or, since surface units fight each other
//! (John, 2026-10-11), a ground object or ship of the other side: the host
//! puts either in [`Inputs::eligible`], already in its order of preference.

use super::QUARTER_SECOND_TICKS;

/// Seconds, at 120 ticks a second.
const fn seconds(s: u64) -> u64 {
    s * super::TICKS_PER_SECOND
}
/// Launch gates may fail this long before the controller gives up its target
/// (B42's 15 s preparation window).
pub const GATES_WINDOW_TICKS: u64 = seconds(15);
/// Search goes back to Idle after this long with no hostile in range.
pub const IDLE_AFTER_TICKS: u64 = seconds(30);

/// Experience effects (docs/spec/surface-defenses.md, "Experience"; fitted).
/// `skill` is 0 (novice) to 3 (ace); anything else reads as average.
/// Search and preparation delays, percent of the record's.
pub fn delay_percent(skill: i32) -> u64 {
    match skill {
        0 => 150,
        2 => 85,
        3 => 70,
        _ => 100,
    }
}
/// The share of its weapon's launch range a missile unit uses, percent.
pub fn launch_range_percent(skill: i32) -> u32 {
    match skill {
        0 => 80,
        1 => 90,
        _ => 100,
    }
}
/// The chance an emitter turns its radar off when an anti-radiation missile
/// comes within 10 nm of it, percent.
pub fn harm_shutdown_percent(skill: i32) -> u8 {
    match skill {
        0 => 0,
        2 => 60,
        3 => 90,
        _ => 25,
    }
}
/// A gun's aim error per burst, degrees: radar-directed guns (`radar`) or
/// guns that eyeball their target.
pub fn aim_error_deg(skill: i32, radar: bool) -> f64 {
    let index = usize::try_from(skill.clamp(0, 3)).unwrap_or(1);
    if radar {
        [0.6, 0.4, 0.3, 0.2][index]
    } else {
        [1.5, 1.0, 0.7, 0.5][index]
    }
}
/// A squad's small arms aim error per burst at an aircraft, degrees: 4 times
/// the visual guns' ([`aim_error_deg`]), so 6.0, 4.0, 2.8 and 2.0 for novice,
/// average, veteran and ace (fitted, agent, 2026-10-12; John's report that
/// airfield guns "shoot with very high accuracy"). Riflemen and light machine
/// guns shoot by eye with no mount, no sight and no lead computer. Together
/// with their poor lead ([`SMALL_ARMS_LEAD`]) a squad hits about one round in a
/// hundred of a 400 kt jet 300 ft over it, whatever its experience
/// (docs/spec/surface-defenses.md, "Small arms"). Squads shooting at ground
/// units keep the old error, a quarter of the visual guns'
/// ([`surface_aim_error_deg`]).
pub fn small_arms_aim_error_deg(skill: i32) -> f64 {
    aim_error_deg(skill, false) * SMALL_ARMS_ERROR_FACTOR
}
/// The share of the correct lead a squad's small arms give an aircraft
/// (fitted, agent, 2026-10-12): the aim point carries the target on at this
/// share of its velocity. 0 shoots where the target was seen.
pub const SMALL_ARMS_LEAD: f64 = 0.3;
/// How much wider a squad's small arms aim error at an aircraft is than the
/// visual guns' (fitted, agent, 2026-10-12).
pub const SMALL_ARMS_ERROR_FACTOR: f64 = 4.0;
/// A gun's aim error per burst at a ground object or ship, degrees: a
/// quarter of [`aim_error_deg`] (fitted, agent, 2026-10-11). A target on the
/// ground or the sea moves in a plane, slowly, and a tank gunner lays on it
/// at leisure; with the air error a tank would need about fifteen minutes to
/// hit another at 3,000 ft. With this error and the gun's 0.25 degree round
/// dispersion a tank at that range hits another about once in five rounds
/// (docs/spec/surface-defenses.md, "Surface against surface").
pub fn surface_aim_error_deg(skill: i32, radar: bool) -> f64 {
    aim_error_deg(skill, radar) / 4.
}
/// The aim error a tank's fire control adds at an aircraft, degrees, the same
/// at every experience (fitted, agent, 2026-10-24; John's request that tanks
/// "not be godlike snipers" against aircraft). A tank's fire control is
/// built for targets on the ground: its ballistic computer has no air target
/// mode, its laser ranges poorly on a small target against the sky, and its
/// round must strike (no proximity fuze). A better crew tracks better
/// ([`aim_error_deg`] by skill), but cannot range better, so this part does
/// not shrink with experience (docs/spec/surface-defenses.md, "Tanks against
/// aircraft").
pub const TANK_AIR_RANGING_ERROR_DEG: f64 = 0.4;
/// The tracking lag of a tank gunner laying on an aircraft, seconds (fitted,
/// agent, 2026-10-24): the aim error at an aircraft grows by the target's
/// angular rate, as the gun sees it, times this. A tank's turret and gun
/// drives and its stabiliser are made to hold a slow ground target; a gunner
/// tracking a crossing jet by hand lags it, and the lead the fire control
/// takes from that tracking carries the lag. A hovering helicopter adds
/// nothing; a 100 kt helicopter crossing 3,000 ft away (about 3 degrees a
/// second) adds 0.6 degrees; a 450 kt jet 1,000 ft away crossing at right
/// angles (about 44 degrees a second) adds 9 degrees, a clean miss.
pub const TANK_AIR_TRACKING_LAG_S: f64 = 0.2;
/// Seconds a tank gun must hold an aircraft inside its fire gates (its reach
/// and its gun's elevation) before the first round at it (fitted, agent,
/// 2026-10-24): the gunner lays, lases and tracks long enough for the fire
/// control to settle a lead, which it can only do once the target is in the
/// sight inside the gun's reach. Each time the aircraft leaves the gates the
/// lay starts again. Later rounds at the same target follow the gun's own
/// loading cycle.
pub const TANK_AIR_LAY_S: f64 = 3.0;
/// A tank gun's aim error per round at an aircraft, degrees: the visual
/// guns' error by skill, plus [`TANK_AIR_RANGING_ERROR_DEG`], plus the
/// target's angular rate (`angular_rate_deg_s`, as the gun sees it) times
/// [`TANK_AIR_TRACKING_LAG_S`]: 1.9, 1.4, 1.1 and 0.9 degrees at a hovering
/// helicopter for novice, average, veteran and ace. Tanks shooting at ground
/// units and ships keep [`surface_aim_error_deg`].
pub fn tank_air_aim_error_deg(skill: i32, angular_rate_deg_s: f64) -> f64 {
    aim_error_deg(skill, false)
        + TANK_AIR_RANGING_ERROR_DEG
        + angular_rate_deg_s.abs() * TANK_AIR_TRACKING_LAG_S
}
/// How much a radar-directed gun's aim error widens while its target's
/// radio-frequency jammer is on. Visual guns are unaffected. Defined (John,
/// 2026-10-10: a factor of 2 is "fine for now").
pub const JAMMED_RADAR_GUN_ERROR_FACTOR: f64 = 2.0;

/// The controller's coarse phase, for the trace, the RWR lock feed and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No hostile aircraft inside detection range.
    Idle,
    /// Looking for an eligible target.
    Search,
    /// A target chosen; waiting out the preparation time.
    Prepare,
    /// Locked; waiting out the tracking delay or for the launch gates.
    Track,
    /// Releasing a burst or salvo.
    Fire,
    /// Between bursts or salvos.
    Pause,
    /// A gun swapping magazines.
    Reload,
    /// Nothing left to fire until a supply truck rearms the unit.
    Empty,
    /// A battery whose radar is destroyed or shut down.
    Blind,
}

impl Phase {
    /// Track, Fire or Pause: the unit holds a lock on its target, the RWR's
    /// painting state and lock tone. The lock holds between bursts and
    /// salvos (defined, John, 2026-10-10).
    pub fn locked(self) -> bool {
        matches!(self, Phase::Track | Phase::Fire | Phase::Pause)
    }
}

/// The controller's times, in ticks, already scaled by experience.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// Search retry when nothing is eligible.
    pub search: u64,
    /// First-engagement preparation.
    pub unready: u64,
    /// Later preparation.
    pub ready: u64,
    /// Lock held this long before the first round (`trackT`).
    pub track: u64,
    /// Flak's retarget period; `None` keeps the target while it is valid.
    pub retarget: Option<u64>,
}

impl Timing {
    /// The times from the record's quarter-second fields: the NPC block's
    /// search, unready and attack times and retarget period, and the
    /// weapon's tracking delay. Search and preparation scale with `skill`;
    /// `slow` doubles preparation (a blind battery's optical backup). A
    /// retarget period of 32767 or more (the record's "never") is none.
    pub fn from_quarters(
        search: i32,
        unready: i32,
        attack: i32,
        track: u8,
        retarget: i32,
        skill: i32,
    ) -> Self {
        let quarters = |q: i32| u64::try_from(q.max(0)).unwrap_or(0) * QUARTER_SECOND_TICKS;
        let scaled = |q: i32| quarters(q) * delay_percent(skill) / 100;
        Self {
            search: scaled(search).max(QUARTER_SECOND_TICKS),
            unready: scaled(unready),
            ready: scaled(attack),
            track: u64::from(track) * QUARTER_SECOND_TICKS,
            retarget: (retarget > 0 && retarget < 32_767).then(|| quarters(retarget)),
        }
    }
}

/// How the controller's weapon fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arm {
    /// `salvo` missiles `gap` ticks apart, then `spacing` ticks before the
    /// next salvo (`gameRoundsInBurst`, `gameBurstT`, `reloadT`).
    Missile { salvo: u32, gap: u64, spacing: u64 },
    /// `burst` rounds over `burst_ticks`, then `pause` ticks; the first
    /// burst of the first engagement is `opening` rounds when that is not 0,
    /// `opening_gap` ticks apart (0: the burst's round spacing, at least a
    /// quarter second); an empty magazine is swapped in `swap` ticks.
    Gun {
        burst: u32,
        burst_ticks: u64,
        pause: u64,
        opening: u32,
        opening_gap: u64,
        swap: u64,
    },
}

/// A controller's fixed description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    pub timing: Timing,
    pub arm: Arm,
}

/// Rounds behind the loaded ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reserve {
    /// A ship's guns: never run dry.
    Unlimited,
    /// Spare magazines (guns) or nothing behind the rails (missiles: 0).
    Magazines(u32),
}

/// What the unit has to fire with, as the host keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stock {
    /// Rounds ready: a gun's magazine, a launcher's loaded rails.
    pub loaded: u32,
    pub reserve: Reserve,
    /// A live friendly supply truck is in range (the resupply slice sets
    /// it): an empty reserve can still swap a magazine.
    pub supply: bool,
}

impl Stock {
    fn can_swap(&self) -> bool {
        self.supply || !matches!(self.reserve, Reserve::Magazines(0))
    }
}

/// Everything the controller reads on one tick.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    pub tick: u64,
    /// A hostile aircraft, or a hostile surface unit the weapon may aim at,
    /// is inside detection range (eligible or not).
    pub hostile_in_range: bool,
    /// Ids of the eligible hostiles, aircraft or surface units, in the
    /// host's order of preference (the nearest of the preferred kind first).
    pub eligible: &'a [u32],
    /// The launch gates pass for the controller's current target
    /// ([`Controller::target`]), as the host measured them this tick.
    pub gates: bool,
    pub stock: Stock,
    /// The battery's radar is destroyed or shut down, with no optical backup.
    pub blind: bool,
    /// Preparation takes twice as long (a blind battery's optical backup).
    pub slow: bool,
    /// Hold fire on the current target: the controller keeps its lock in
    /// Track, ready, without counting the launch gates' failure, and fires
    /// nothing (a flak team that has not yet engaged the target's group).
    pub hold: bool,
    /// Ticks the target must stay inside the launch gates before the first
    /// round at it, on top of the record's tracking delay: a tank gun's lay
    /// on an aircraft ([`TANK_AIR_LAY_S`]). While it is above 0, every time
    /// the gates fail in Track the lay starts again when they pass. 0 for
    /// every other weapon and target, which keep their lock while the gates
    /// fail.
    pub lay: u64,
}

/// Rounds the host must fire this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FireRequest {
    pub target: u32,
    pub rounds: u32,
    /// The first round of a burst or salvo: a gun draws its aim error now.
    pub burst_start: bool,
    /// The opening barrage.
    pub opening: bool,
}

/// What one [`Controller::advance`] call asks of the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub fire: Option<FireRequest>,
    /// A magazine swap finished: load a magazine from the reserve (or the
    /// truck in range).
    pub swap: bool,
}

/// A burst or salvo in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Burst {
    pub(crate) start: u64,
    pub(crate) rounds: u32,
    pub(crate) released: u32,
    /// Ticks from the first round to the last plus one spacing.
    pub(crate) span: u64,
    pub(crate) opening: bool,
}

impl Burst {
    /// Rounds due by `tick`, counting from the burst's start.
    fn due(&self, tick: u64) -> u32 {
        if tick < self.start {
            return 0;
        }
        let elapsed = tick - self.start;
        let n = u64::from(self.rounds.max(1));
        // Round k leaves at start + k * span / n.
        let due = (elapsed * n) / self.span.max(1) + 1;
        u32::try_from(due.min(n)).unwrap_or(self.rounds)
    }
}

/// One engagement controller. One per weapon group of a unit, or one per
/// SAM battery (on its radar). Persistent, checkpointed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controller {
    pub(crate) phase: Phase,
    pub(crate) target: Option<u32>,
    /// The current phase's deadline: search retry, preparation end, pause
    /// end, magazine swap end.
    pub(crate) deadline: u64,
    pub(crate) lock_since: u64,
    pub(crate) gates_failed_since: Option<u64>,
    pub(crate) last_hostile: Option<u64>,
    /// A Prepare has been entered (the unready time is spent).
    pub(crate) engaged: bool,
    /// The opening barrage has been fired.
    pub(crate) opened: bool,
    pub(crate) retarget_at: u64,
    pub(crate) burst: Option<Burst>,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            target: None,
            deadline: 0,
            lock_since: 0,
            gates_failed_since: None,
            last_hostile: None,
            engaged: false,
            opened: false,
            retarget_at: 0,
            burst: None,
        }
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    /// The aircraft the controller engages, from Prepare on.
    pub fn target(&self) -> Option<u32> {
        match self.phase {
            Phase::Idle | Phase::Search | Phase::Empty | Phase::Blind => None,
            _ => self.target,
        }
    }
    /// Whether the controller fights its target now: the target is inside the
    /// launch gates (Track with the gates met, which needs the fire zone) or
    /// a burst has started (Fire, Pause, Reload). A target seen only in the
    /// detection zone (Prepare, or Track with the gates failing) does not
    /// count. A routed land unit holds while one of its weapons fights a
    /// ground unit or ship (docs/spec/surface-defenses.md, "Surface against
    /// surface").
    pub fn fighting(&self) -> bool {
        match self.phase {
            Phase::Fire | Phase::Pause | Phase::Reload => self.target.is_some(),
            Phase::Track => self.target.is_some() && self.gates_failed_since.is_none(),
            _ => false,
        }
    }
    /// The burst or salvo in progress, as `(start tick, rounds, span ticks,
    /// rounds released)`: round k leaves at the first tick at or after
    /// `start + k * span / rounds`. A networked host tells clients a gun
    /// burst's schedule from it (protocol 22).
    pub fn burst(&self) -> Option<(u64, u32, u64, u32)> {
        self.burst.map(|b| (b.start, b.rounds, b.span, b.released))
    }
    /// The phase's pending deadline, in ticks, where it has one.
    pub fn deadline(&self) -> Option<u64> {
        matches!(
            self.phase,
            Phase::Search | Phase::Prepare | Phase::Pause | Phase::Reload
        )
        .then_some(self.deadline)
    }

    fn enter_search(&mut self, tick: u64) {
        self.phase = Phase::Search;
        self.target = None;
        self.deadline = tick;
        self.burst = None;
        self.gates_failed_since = None;
    }

    /// Out of rounds: a gun swaps a magazine if it can, else the unit is
    /// Empty.
    fn out_of_rounds(&mut self, profile: &Profile, inputs: &Inputs<'_>) {
        self.burst = None;
        match profile.arm {
            Arm::Gun { swap, .. } if inputs.stock.can_swap() => {
                self.phase = Phase::Reload;
                self.deadline = inputs.tick + swap;
            }
            _ => {
                self.phase = Phase::Empty;
                self.target = None;
            }
        }
    }

    fn start_burst(&mut self, profile: &Profile, tick: u64) {
        let (rounds, span, opening) = match profile.arm {
            Arm::Missile { salvo, gap, .. } => {
                let salvo = salvo.max(1);
                (salvo, gap.max(1) * u64::from(salvo), false)
            }
            Arm::Gun {
                burst,
                burst_ticks,
                opening,
                opening_gap,
                ..
            } => {
                let burst = burst.max(1);
                if !self.opened && opening > 0 {
                    let spacing = if opening_gap > 0 {
                        opening_gap
                    } else {
                        (burst_ticks / u64::from(burst)).max(QUARTER_SECOND_TICKS)
                    };
                    (opening, spacing * u64::from(opening), true)
                } else {
                    (burst, burst_ticks.max(1), false)
                }
            }
        };
        self.opened = true;
        self.phase = Phase::Fire;
        self.burst = Some(Burst {
            start: tick,
            rounds,
            released: 0,
            span,
            opening,
        });
    }

    /// Advance one tick. Ticks never go backwards; the host calls it once a
    /// tick while the unit is alive.
    pub fn advance(&mut self, profile: &Profile, inputs: &Inputs<'_>) -> Outcome {
        let tick = inputs.tick;
        let mut outcome = Outcome::default();
        if inputs.hostile_in_range {
            self.last_hostile = Some(tick);
        }
        if inputs.blind {
            if self.phase != Phase::Blind {
                self.phase = Phase::Blind;
                self.target = None;
                self.burst = None;
            }
            return outcome;
        }
        let still = |id: Option<u32>| id.is_some_and(|id| inputs.eligible.contains(&id));
        // Each arm may move to the next phase and evaluate it on the same
        // tick; a few passes settle every chain.
        for _ in 0..4 {
            let before = self.phase;
            match self.phase {
                Phase::Blind => self.enter_search(tick),
                Phase::Idle => {
                    if inputs.hostile_in_range {
                        self.enter_search(tick);
                    }
                }
                Phase::Search => {
                    if !inputs.hostile_in_range
                        && self
                            .last_hostile
                            .is_none_or(|last| tick >= last + IDLE_AFTER_TICKS)
                    {
                        self.phase = Phase::Idle;
                    } else if inputs.stock.loaded == 0 {
                        self.out_of_rounds(profile, inputs);
                    } else if tick >= self.deadline {
                        match inputs.eligible.first() {
                            Some(&target) => {
                                self.target = Some(target);
                                self.phase = Phase::Prepare;
                                let mut delay = if self.engaged {
                                    profile.timing.ready
                                } else {
                                    profile.timing.unready
                                };
                                if inputs.slow {
                                    delay *= 2;
                                }
                                self.engaged = true;
                                self.deadline = tick + delay;
                            }
                            None => self.deadline = tick + profile.timing.search,
                        }
                    }
                }
                Phase::Prepare => {
                    if !still(self.target) {
                        self.enter_search(tick);
                    } else if tick >= self.deadline {
                        self.phase = Phase::Track;
                        self.lock_since = tick;
                        self.gates_failed_since = None;
                        self.retarget_at = tick + profile.timing.retarget.unwrap_or(0);
                    }
                }
                Phase::Track => {
                    if !still(self.target) {
                        self.enter_search(tick);
                    } else if inputs.stock.loaded == 0 {
                        self.out_of_rounds(profile, inputs);
                    } else if inputs.hold {
                        // Locked and ready; the gates' clock does not run.
                        self.gates_failed_since = None;
                    } else if !inputs.gates {
                        let since = *self.gates_failed_since.get_or_insert(tick);
                        if tick >= since + GATES_WINDOW_TICKS {
                            self.enter_search(tick);
                        }
                    } else {
                        if self.gates_failed_since.take().is_some() && inputs.lay > 0 {
                            // A lay restarts once the target is back in the
                            // gates.
                            self.lock_since = tick;
                        }
                        if tick >= self.lock_since + profile.timing.track + inputs.lay {
                            self.start_burst(profile, tick);
                        }
                    }
                }
                Phase::Fire => {
                    let Some(mut burst) = self.burst else {
                        self.phase = Phase::Track;
                        continue;
                    };
                    if !still(self.target) {
                        self.enter_search(tick);
                    } else if inputs.stock.loaded == 0 {
                        self.out_of_rounds(profile, inputs);
                    } else if !inputs.gates || inputs.hold {
                        // The target left the gates mid-burst (or the unit
                        // holds its fire): the rest of the burst is not
                        // fired.
                        self.burst = None;
                        self.phase = Phase::Track;
                        self.gates_failed_since = Some(tick);
                    } else {
                        let due = burst.due(tick).saturating_sub(burst.released);
                        let rounds = due.min(inputs.stock.loaded);
                        if rounds > 0 {
                            outcome.fire = Some(FireRequest {
                                target: self.target.unwrap_or_default(),
                                rounds,
                                burst_start: burst.released == 0,
                                opening: burst.opening,
                            });
                            burst.released += rounds;
                        }
                        self.burst = Some(burst);
                        if burst.released >= burst.rounds {
                            self.burst = None;
                            self.phase = Phase::Pause;
                            self.deadline = match profile.arm {
                                Arm::Missile { spacing, .. } => tick + spacing,
                                Arm::Gun { pause, .. } => burst.start + burst.span + pause,
                            };
                        }
                        // A burst releases its rounds over the coming ticks.
                        break;
                    }
                }
                Phase::Pause => {
                    if !still(self.target) {
                        self.enter_search(tick);
                    } else if inputs.stock.loaded == 0 {
                        self.out_of_rounds(profile, inputs);
                    } else if tick >= self.deadline {
                        self.phase = Phase::Track;
                        if let Some(period) = profile.timing.retarget
                            && tick >= self.retarget_at
                        {
                            self.retarget_at = tick + period;
                            let nearest = inputs.eligible.first().copied();
                            if nearest.is_some() && nearest != self.target {
                                self.target = nearest;
                                self.lock_since = tick;
                            }
                        }
                    }
                }
                Phase::Reload => {
                    if tick >= self.deadline {
                        if inputs.stock.can_swap() {
                            outcome.swap = true;
                            if still(self.target) {
                                self.phase = Phase::Track;
                            } else {
                                self.enter_search(tick);
                            }
                        } else {
                            self.phase = Phase::Empty;
                            self.target = None;
                        }
                        // The magazine is loaded after this tick.
                        break;
                    }
                }
                Phase::Empty => {
                    if inputs.stock.loaded > 0 {
                        self.enter_search(tick);
                    } else if matches!(profile.arm, Arm::Gun { .. }) && inputs.stock.can_swap() {
                        self.out_of_rounds(profile, inputs);
                    }
                }
            }
            if self.phase == before {
                break;
            }
        }
        outcome
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "surface_checkpoint.rs"]
mod checkpoint;

#[cfg(test)]
#[path = "surface_tests.rs"]
mod tests;
