//! The AI strike driver: `--a2g-run THEATER STEM --pilot ai` (plan section 5,
//! slice ST-D, `.local/ai-a2g/plan.md`).
//!
//! The player-side kinds of this driver fly an aircraft through the seat's
//! own controls. This one lets the AI fly it: the aircraft starts `--from` NM
//! short of the target on the pass `--heading`, `--altitude`, `--speed` and
//! `--dive` describe, and from the first tick the AI's own controller flies
//! it, in the same mission step as the game. The driver only watches and
//! reports what the simulation shows. Every line starts `ai-strike:`.
//!
//! ```text
//! --a2g-run THEATER STEM --pilot ai [--aircraft ID] [--load REC] [--target N]
//!     [--station N] [--count N] [--skill 0..3]
//!     [--profile straight|popup|fasthigh|dive|level|orbit|standoff]
//!     [--handback S [--mode ccip|lvl] [--trigger] [--laser] [--take-back S]]
//!     [--order KIND [--order-at S]] [--quick-wing] [--render OUT.png --render-at S]
//!     [--choose-store] [--buddy ID]
//!     [--from NM] [--altitude FT] [--speed KT] [--heading DEG] [--dive DEG]
//!     [--defenses AAA SAM] [--surface-seed N] [--seconds S]
//!     [--sead-at S | --sead-after-shot S] [--sead-site ID]...
//!     [--follow-threat] [--follow-egress]
//!     [--place-site ID,BEARING,RANGE_FT]... [--site-sight ID,FEET]...
//!     [--terrain-map RADIUS_FT,STEP_FT] [--terrain-nodes RADIUS_FT]
//! ```
//!
//! - `--aircraft`, `--load`, `--station`, `--count`: the striker and its store,
//!   fitted as the player-side kinds fit it (a store the aircraft does not
//!   carry is fitted to a bomb, pod or missile station for the run).
//! - `--target N`: the Nth ground target of the stem (0 by default).
//! - `--skill 0..3`: the striker's wing, Novice to Ace (Average by default).
//! - `--profile`: **forces** the pass for coverage. The real AI chooses its
//!   pass from the perceived threat and its energy (John, 2026-10-11, answer
//!   3); the flag exists so a test can fly each pass once.
//! - `--handback S`: a human-flown run handed to the AI. A seat flies the
//!   striker on the scripted pass, designates the target, selects the store
//!   (and `--mode` of the bomb sight) and gives the aircraft back to the AI
//!   `S` seconds into the pass, for the takeover tests. `--trigger` has the
//!   human hold the trigger from the selection on (pickle and hold), so a
//!   bomb is released, and reported, before the handoff. `--laser` has the
//!   human turn the laser on by hand (Ctrl+L) once the store is selected.
//!   `--take-back S`: the seat takes the aircraft again `S` seconds after the
//!   handback, and the run ends there with what the human finds (slice
//!   ST-K, docs/spec/ai-strike.md "Takeover and handback").
//! - `--order KIND`: wing orders. The seat leads a two-ship wing on the
//!   scripted pass, designates the target and gives the wingman (the striker)
//!   the order `--order-at S` seconds into the pass (5 by default). Kinds:
//!   see [`ORDERS`]; the orders the strike round adds join that table.
//! - `--quick-wing`: a Quick Mission's own strike wing (slice ST-O). No seat
//!   flies and nothing orders it: friendly wing 2 is two `--aircraft`, and the
//!   mission's default role (bomber-class aircraft strike when a ground target
//!   is set) sends it. It starts where the mission puts it and flies to the
//!   target itself; `ROLE` reports the role it was given.
//!
//! - `--choose-store`: the AI chooses its own store instead of the fitted
//!   one (slice ST-L): a laser store only with a spot assured.
//! - `--buddy ID`: a second seat flies aircraft ID as a buddy of the striker
//!   (slice ST-L): a scripted pass 1 nm ahead of the striker, 3,000 ft right
//!   and 1,000 ft higher, which designates the nearest ground object with
//!   Ctrl+X and lases it by hand (Ctrl+L). The striker is then ordered onto
//!   what the buddy designated. A striker with no designator of its own drops
//!   on the buddy's spot; the RELEASE line names the spot's owner.
//!
//! What it prints, in this order, then one `summary` and `done`:
//!
//! - `setup`: the aircraft, store, target, skill, forced profile, handback
//!   and order.
//! - `start`: the striker's place at tick 0 (range to the target, altitude).
//! - `HANDBACK`, `ORDER`: the handoff and the order, with what was kept.
//! - `TAKEOVER`: what the AI continues after a handback (target, station,
//!   bomb sight mode), or `none`.
//! - `KEPT`: the kept cockpit while the AI flies it (mode, laser hand
//!   setting and state, selected station), each time that changes.
//! - `TAKEBACK`: a `--take-back`: what the human finds (designation,
//!   station, mode, laser, rounds left).
//! - `PROFILE name=P requested=R`: the pass the AI reports it flies. `none`
//!   until slice ST-T reports one (see [`reported_profile`]).
//! - `RELEASE`: a store left the striker (time, store, station, height above
//!   the ground, speed, range to the target, rounds left).
//! - `IMPACT miss_ft=F`: a released round ended, and how far (horizontal) from
//!   the target it ended.
//! - `BURST`: an AC-130 burst began (its gun rounds are not RELEASE lines):
//!   its number, its walked-in aim offset, and the orbit's radius, height,
//!   slant range and bank (slice ST-F).
//! - `KILL`: a ground object the striker destroyed.
//! - `EGRESS`: after a release the striker is opening from the target (one
//!   nautical mile past its closest approach).
//! - `LOST`: the striker was destroyed, crashed, ejected or left the AI.
//! - `ACTIVITY`: what the AI says it is doing, each time that changes.
//! - `PHASE`: the striker's strike phase (ingress, attack, lase, exit, ...)
//!   each time it changes (slice ST-L); `HOLD` every five seconds of a lase
//!   hold: its range from the target, height above it, speed and bank.
//! - `NOLAZE decision=D reason=R weapon=W station=S target=T words="..."`: a
//!   striker with laser stores and no laser support decided (slice ST-G):
//!   `manual` (a laser bomb let go as an unguided bomb) or `rtb` (a laser
//!   store that cannot fall unguided, so home), once, with the radio's
//!   words; the replay records it as an `ai.strike` event.
//! - `THREAT t= decision=D target=T ring=R words="..."`: a helicopter's
//!   threat decision (slice H4): `threat_hold_off`, `threat_resume`,
//!   `threat_retarget`, `threat_home` or `threat_leave`, the target it
//!   concerns and the ring (a defence's id, 0 for an ambush); the replay
//!   records it as an `ai.strike` event. `HOLDOFF` every ten seconds of a
//!   hold-off: seconds held, distance from the hold point, height above
//!   the ground and range to the target. `SEAD t= killed=N`: `--sead-at`
//!   (or `--sead-after-shot`, a set time after a site first fired)
//!   destroyed the hostile sites (only the `--sead-site` ones when named).
//!   `SHOT t= site=ID range_ft=R known=yes|no`: a ground site launched a
//!   round (not a gun's) at the striker, R feet from it, and whether the
//!   striker knew the site then (its ring was in its ground picture). `RINGS` with each `THREAT`: the rings it
//!   knew (id, radius, distance from the target, feet). `AHEAD t=
//!   range_ft=R fire_in_s=S point=X,Z aim_moves_ft=D from_ft=F` each time a
//!   helicopter plans a move-ahead ambush on a moving target (John,
//!   2026-10-29): the firing range, the seconds to the planned shot, the
//!   firing point, how far the target drives to the predicted aim point and
//!   how far the helicopter is from the firing point; `RINGS` follows.
//!   The summary's `reach_margin_ft` is the least margin the striker kept,
//!   sampled every quarter second, to the reach of any defence it knew at
//!   the time (its ring less the ring's margin); negative inside one.
//! - `LASER plane=P state=S mask=M`: the laser of the striker, and of the
//!   buddy when there is one, each time its state or mask changes (slice
//!   ST-L). The summary counts the seconds a laser weapon of the run was in
//!   flight guiding (`fall_s`) and how many of them the spot it followed was
//!   lost (`blind_s`).
//!
//! The hooks the strike slices fill are in one place, [`hooks`]: ST-T makes
//! the AI take the target and report its profile, ST-K reports what a takeover
//! kept, ST-O adds order kinds. Until they land the driver reports what the
//! existing AI does with a ground target, which is nothing (no RELEASE), and
//! says so.
use super::{FEET_PER_NM, FPS_PER_KNOT, Options, SEAT, Scene, ccip::Pass, fit_store};
use crate::AppResult;
use std::collections::{BTreeMap, BTreeSet};
use tore_sim::{
    ai::{
        controller::Activity, engagement::StrikeRole, mission::AiActor, strike::Pass as StrikePass,
        weapon_service::Rounds, wing::PlayerOrder,
    },
    attitude::Vector,
    combat::{
        blast::MarkKind,
        delivery::DeliveryMode,
        laser::{LaserState, Mask},
        live::{self, Command, EffectKind, GuidedMode},
    },
};
use tore_world::{
    mission::{Defense, MissionSpec, Skill, WingSpec},
    seats::{PlaneId, SeatCommand, SeatId, SeatInput},
    world::{OrderOutcome, Seating, TickOutput, World},
};

/// The seat a handoff of the striker uses when the main seat leads the wing.
const WINGMAN_SEAT: SeatId = SeatId(1);
/// Seconds the seat may spend designating the target.
const DESIGNATE_SECONDS: f64 = 12.;
/// Seconds after the last release or loss the run goes on, to see where the
/// rounds land and the striker leaves.
const SETTLE_SECONDS: f64 = 20.;
/// An impact is the burst nearest the round's last place within this many feet.
const BURST_WITHIN_FT: f64 = 60.;
/// The striker is egressing once it is this far past its closest approach
/// after a release.
const EGRESS_OPENING_FT: f64 = FEET_PER_NM;
/// `--render-scene` draws its frame this long after the first release.
const RENDER_AFTER_SECONDS: f64 = 0.4;
/// `--render-scene`'s companion lead starts this far behind the striker
/// and to its right, feet, well clear of the pass.
const COMPANION_BEHIND_FT: f64 = 15_000.;
const COMPANION_RIGHT_FT: f64 = 12_000.;
/// The companion's plane: friendly wing 2's only aircraft.
const COMPANION: PlaneId = PlaneId(1);

/// The passes `--profile` can force (docs/spec/ai.md, the retail script's
/// four passes, and the level run the heavy bombers fly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    Straight,
    Popup,
    FastHigh,
    Dive,
    Level,
    /// The gunship's orbit (slice ST-F).
    Orbit,
    /// A helicopter's stand-off attack (slice ST-F).
    StandOff,
    Laydown,
    Strafe,
}

impl Profile {
    pub(crate) const ALL: [Self; 9] = [
        Self::Straight,
        Self::Popup,
        Self::FastHigh,
        Self::Dive,
        Self::Level,
        Self::Orbit,
        Self::StandOff,
        Self::Laydown,
        Self::Strafe,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Straight => "straight",
            Self::Popup => "popup",
            Self::FastHigh => "fasthigh",
            Self::Dive => "dive",
            Self::Level => "level",
            Self::Orbit => "orbit",
            Self::StandOff => "standoff",
            Self::Laydown => "laydown",
            Self::Strafe => "strafe",
        }
    }

    fn pass(self) -> StrikePass {
        match self {
            Self::Straight => StrikePass::Straight,
            Self::Popup => StrikePass::PopUp,
            Self::FastHigh => StrikePass::FastHigh,
            Self::Dive => StrikePass::Dive,
            Self::Level => StrikePass::Level,
            Self::Orbit => StrikePass::Orbit,
            Self::StandOff => StrikePass::StandOff,
            Self::Laydown => StrikePass::Laydown,
            Self::Strafe => StrikePass::Strafe,
        }
    }

    fn from_pass(pass: StrikePass) -> Self {
        match pass {
            StrikePass::Straight => Self::Straight,
            StrikePass::PopUp => Self::Popup,
            StrikePass::FastHigh => Self::FastHigh,
            StrikePass::Dive => Self::Dive,
            StrikePass::Level => Self::Level,
            StrikePass::Orbit => Self::Orbit,
            StrikePass::StandOff => Self::StandOff,
            StrikePass::Laydown => Self::Laydown,
            StrikePass::Strafe => Self::Strafe,
        }
    }

    fn parse(text: &str) -> AppResult<Self> {
        Self::ALL
            .into_iter()
            .find(|p| p.label() == text.to_ascii_lowercase())
            .ok_or_else(|| {
                format!(
                    "unknown --profile {text}; use straight, popup, fasthigh, dive, level, orbit, standoff, laydown or strafe"
                )
                    .into()
            })
    }
}

/// The wing orders `--order` can give, by name. The last three are the strike
/// round's (slice ST-O): Alt+F, Alt+W and "Lase my target".
const ORDERS: [(&str, PlayerOrder); 11] = [
    ("engage-my-target", PlayerOrder::EngageMyTarget),
    ("engage-from-formation", PlayerOrder::EngageFromFormation),
    ("attack-on-contact", PlayerOrder::AttackOnContact),
    ("protect-me", PlayerOrder::ProtectMe),
    ("disengage", PlayerOrder::Disengage),
    ("bug-out", PlayerOrder::BugOut),
    ("land-selected", PlayerOrder::LandAtSelected),
    ("sort", PlayerOrder::Sort),
    ("engage-ir-laser", PlayerOrder::EngageIrLaser),
    ("engage-class", PlayerOrder::EngageClass),
    ("lase-my-target", PlayerOrder::LaseMyTarget),
];

/// The command-line options of this driver. The shared parser in
/// `a2g_run.rs` fills it.
#[derive(Default)]
pub(crate) struct Config {
    /// `--pilot ai`.
    pub active: bool,
    pub profile: Option<Profile>,
    pub handback: Option<f64>,
    pub mode: Option<DeliveryMode>,
    /// `--trigger`: the human leg holds the trigger once the store is
    /// selected (pickle and hold), so the human releases before it hands back.
    pub trigger: bool,
    /// `--laser`: the human leg turns the laser on by hand.
    pub laser: bool,
    /// `--take-back S`: the seat takes the aircraft again `S` seconds after
    /// the handback.
    pub take_back: Option<f64>,
    pub order: Option<(&'static str, PlayerOrder)>,
    pub order_at: f64,
    /// Ground class for an explicit class-order probe, otherwise the historical SAM probe.
    pub order_class: Option<u16>,
    /// Number of wingmen in an order probe (default one).
    pub order_wingmen: Option<u8>,
    /// `--quick-wing`.
    pub quick_wing: bool,
    /// Remove the last enemy aircraft at this second, testing unfinished strikes.
    pub clear_air_at: Option<f64>,
    /// `--render-at S` (with `--render OUT.png`): one offscreen picture of
    /// the striker and its target `S` seconds into the run (slice ST-F).
    pub render_at: Option<f64>,
    /// `--choose-store`: the AI chooses its own store (the fitted one is not
    /// forced), as in a mission; a buddy run does too.
    pub choose: bool,
    /// `--mission-altitude FT`: the Quick Mission altitude setting (5,000,
    /// 10,000, 20,000 or 40,000 ft), the mission's airborne height a heavy
    /// bombs from (slice ST-T2).
    pub mission_altitude: Option<u32>,
    /// `--render-scene DIR`: draws the world offscreen, looking at the
    /// striker, at its first release (slice ST-T2).
    pub render_scene: Option<std::path::PathBuf>,
    /// `--console-scene DIR --console-at S,...`: frames of the AC-130 sensor
    /// console from the striker's own sensor at those seconds into the run
    /// (slice nr-fltv, [`super::console_render`]).
    pub console_scene: Option<std::path::PathBuf>,
    pub console_at: Vec<f64>,
    /// `--offset FT`: the striker starts this far right of the pass line
    /// (negative: left), for the first-pass convergence tests.
    pub offset_ft: f64,
    /// `--sead-at S`: every hostile SAM and gun site but the target is
    /// destroyed `S` seconds into the run, a stand-in for a SEAD flight
    /// (slice H4: a helicopter holding off resumes once the ring is gone).
    pub sead_at: Option<f64>,
    /// `--sead-site ID` (repeatable): the SEAD stand-in destroys only these
    /// sites, so a site the helicopter has not met yet stays for an ambush
    /// (2026-10-28).
    pub sead_sites: Vec<u32>,
    /// `--sead-after-shot S`: the SEAD stand-in goes in off the clock: S
    /// seconds after the first round one of the `--sead-site` sites (any
    /// site when none is named) launches at the striker, those sites are
    /// destroyed. For an ambush by a site whose second round would bring
    /// the helicopter down: the layout does not hang on when the first
    /// launch comes (Mi-24 ambush layout, 2026-10-29).
    pub sead_after_shot: Option<f64>,
    /// `--follow-threat`: after the last impact the run goes on (within
    /// `--seconds`) while a helicopter keeps out of a ring it means to come
    /// back from, to see whether it resumes (slice H2).
    pub follow_threat: bool,
    /// `--follow-egress`: after the last impact the run goes on (within
    /// `--seconds`) until the striker egresses, and settles from there, so
    /// what happens on the way out is seen (slice HF1).
    pub follow_egress: bool,
    /// `--place-site ID,BEARING_DEG,RANGE_FT`: the hostile SAM or gun site
    /// `ID` is moved, before the first tick, to that compass bearing and
    /// range from the target, so a test can lay out the threats a
    /// helicopter meets, an ambush among them (slice HF1, John
    /// 2026-10-13). Repeatable: each site moves in the order given.
    pub place_sites: Vec<(u32, f64, f64)>,
    /// `--terrain-map RADIUS_FT,STEP_FT`: before the first tick, prints the
    /// ground height on a grid round the target (`TERRAIN` lines, north
    /// first), for finding the relief a layout can hide a site behind.
    pub terrain_map: Option<(f64, f64)>,
    /// `--terrain-nodes RADIUS_FT`: before the first tick, prints the ground
    /// height at every terrain grid node (the imported grid's 8,192 ft
    /// lattice, which the ground between nodes is interpolated from) within
    /// that radius of the target (`TERRAINNODES` and `NODE` lines), so a
    /// test can work out sight lines exactly.
    pub terrain_nodes: Option<f64>,
    /// `--site-sight ID,FEET` (repeatable): the hostile site `ID` can only be
    /// listed by an eye within FEET of it, in place of the type's 19,968 ft
    /// (`maxVisDist`, 78 units of 256 ft). A site dug in and hidden well: the
    /// helicopter does not know it, and so does not keep out of it, until it
    /// is near; the site shoots as it always does. For laying out an ambush
    /// by a site the helicopter did not know (Mi-24 ambush layout,
    /// 2026-10-29): in this simulation the terrain cannot hide a site from
    /// the eye and leave it a line to the helicopter, and a rebuild every 6 s
    /// lists it long before its 15 s of preparation are over.
    pub site_sights: Vec<(u32, f64)>,
}

impl Config {
    pub(crate) fn new() -> Self {
        Self {
            order_at: 5.,
            ..Self::default()
        }
    }

    /// `--pilot player|ai`.
    pub(crate) fn pilot(&mut self, text: &str) -> AppResult<()> {
        self.active = match text.to_ascii_lowercase().as_str() {
            "ai" => true,
            "player" => false,
            other => return Err(format!("unknown --pilot {other}; use player or ai").into()),
        };
        Ok(())
    }

    pub(crate) fn set_profile(&mut self, text: &str) -> AppResult<()> {
        self.profile = Some(Profile::parse(text)?);
        Ok(())
    }

    pub(crate) fn set_mode(&mut self, text: &str) -> AppResult<()> {
        self.mode = Some(match text.to_ascii_lowercase().as_str() {
            "ccip" => DeliveryMode::Ccip,
            "lvl" | "level" => DeliveryMode::Level,
            other => return Err(format!("unknown --mode {other}; use ccip or lvl").into()),
        });
        Ok(())
    }

    pub(crate) fn set_order(&mut self, text: &str) -> AppResult<()> {
        let kinds: Vec<&str> = ORDERS.iter().map(|(name, _)| *name).collect();
        self.order = Some(
            *ORDERS
                .iter()
                .find(|(name, _)| *name == text.to_ascii_lowercase())
                .ok_or_else(|| format!("unknown --order {text}; kinds: {}", kinds.join(", ")))?,
        );
        Ok(())
    }

    /// Refuses combinations the driver cannot fly.
    pub(crate) fn check(&self, skill: Option<i32>, records: bool) -> AppResult<()> {
        let flags = self.handback.is_some()
            || self.order.is_some()
            || self.mode.is_some()
            || self.quick_wing
            || self.clear_air_at.is_some();
        if !self.active {
            if self.profile.is_some() || flags {
                return Err(
                    "--profile, --handback, --mode, --order and --quick-wing need --pilot ai"
                        .into(),
                );
            }
            return Ok(());
        }
        if self.order_wingmen.is_some_and(|n| !(1..=9).contains(&n)) {
            return Err("--order-wingmen needs 1 through 9".into());
        }
        if (self.order_class.is_some() || self.order_wingmen.is_some()) && self.order.is_none() {
            return Err("--order-class and --order-wingmen need --order".into());
        }
        if let Some(at) = self.clear_air_at
            && (!self.quick_wing || !at.is_finite() || at < 0.)
        {
            return Err("--clear-air-at needs --quick-wing and a finite nonnegative second".into());
        }
        if self.quick_wing
            && (self.handback.is_some()
                || self.order.is_some()
                || self.profile.is_some()
                || self.mode.is_some())
        {
            return Err("--quick-wing is the mission's own wing: no --handback, --order, --profile or --mode".into());
        }
        if self.handback.is_some() && self.order.is_some() {
            return Err("--handback and --order are separate runs: pick one".into());
        }
        if (self.mode.is_some() || self.trigger || self.laser || self.take_back.is_some())
            && self.handback.is_none()
        {
            return Err(
                "--mode, --trigger, --laser and --take-back set the human legs: they need --handback"
                    .into(),
            );
        }
        if records && self.order.is_none() {
            return Err("AI --record currently needs --order and its human lead".into());
        }
        if self.sead_at.is_some() && self.sead_after_shot.is_some() {
            return Err(
                "--sead-at and --sead-after-shot are two ways to time the SEAD stand-in: pick one"
                    .into(),
            );
        }
        if self
            .sead_after_shot
            .is_some_and(|s| !s.is_finite() || s < 0.)
        {
            return Err("--sead-after-shot needs a finite nonnegative number of seconds".into());
        }
        if let Some(skill) = skill
            && !(0..=3).contains(&skill)
        {
            return Err("--skill is 0 (Novice) to 3 (Ace)".into());
        }
        Ok(())
    }
}

/// What the strike slices fill in. Each hook says what it does now.
mod hooks {
    use super::*;

    /// Slice ST-T: orders the AI onto the target (an ordered ground target
    /// outranks any briefing, docs/spec/ai-strike.md "What the AI knows"),
    /// keeps it to the driver's store, and forces its pass when `--profile`
    /// asks. Returns the note the `PROFILE` line carries.
    pub(super) fn assign(
        world: &mut World,
        plane: PlaneId,
        target: u32,
        station: Option<usize>,
        forced: Option<Profile>,
    ) -> &'static str {
        let Some(wings) = world.ai_wings.as_mut() else {
            return "no-ai";
        };
        if !wings.set_strike_order(plane.0, Some(target)) {
            return "not-ai-flown";
        }
        wings.force_strike(plane.0, forced.map(Profile::pass), station);
        if forced.is_some() { "forced" } else { "chosen" }
    }

    /// Slice ST-T: the pass the AI reports it is flying, if any.
    pub(super) fn reported_profile(actor: &AiActor) -> Option<Profile> {
        actor
            .strike()
            .and_then(|s| s.pass())
            .map(Profile::from_pass)
    }

    /// The aim of the striker's current pass: the drawn offset's distance
    /// from the target, feet, whether it lies inside the circle, and the
    /// pass number. A smart weapon aims at the target itself (0 ft).
    /// The striker's flight path below the horizon, degrees (a dive's
    /// angle; negative climbing).
    pub(super) fn dive_deg(velocity: Vector) -> f64 {
        -velocity[1]
            .atan2(velocity[0].hypot(velocity[2]))
            .to_degrees()
    }

    /// The lead the striker lays on a moving target, feet (0 on a
    /// standing one).
    pub(super) fn lead(actor: &AiActor) -> f64 {
        actor
            .strike()
            .map_or(0., |s| s.lead()[0].hypot(s.lead()[1]))
    }

    pub(super) fn aim(actor: &AiActor) -> Option<(f64, bool, u32)> {
        let s = actor.strike()?;
        // A gun run's aim is its current burst's (slice ST-T2).
        let (offset, inside) = s.aim();
        let unguided = s.selection().is_none_or(|sel| sel.kind.unguided());
        let distance = if unguided {
            offset[0].hypot(offset[1])
        } else {
            0.
        };
        Some((distance, inside || !unguided, s.pass_number()))
    }

    /// Slice ST-K: what the AI continues after a handback: the target, the
    /// kept station and the bomb sight mode.
    pub(super) fn takeover(actor: &AiActor) -> Option<(u32, Option<usize>, DeliveryMode)> {
        actor
            .strike()
            .and_then(|s| s.takeover())
            .map(|t| (t.target, t.station.map(|s| usize::from(s.0)), t.mode))
    }

    /// Slice ST-K: the kept cockpit in words (mode, laser hand setting and
    /// state, selected station), while the AI flies with it.
    pub(super) fn kept_words(state: &live::State, plane: PlaneId) -> Option<String> {
        let kept = state.kept(plane.0)?;
        Some(format!(
            "mode={} laser={} lasing={} selected={}",
            kept.delivery_mode.label(),
            hand_words(kept.laser.manual),
            if kept.laser.lasing() { "yes" } else { "no" },
            kept.selected
        ))
    }

    /// The laser hand setting (Ctrl+L) in words.
    pub(super) fn hand_words(manual: Option<bool>) -> &'static str {
        match manual {
            Some(true) => "hand-on",
            Some(false) => "hand-off",
            None => "auto",
        }
    }

    /// Slice ST-F: the gunship's burst while its trigger is held: the burst
    /// number at the target, the walked-in aim offset's distance from the
    /// target, feet, and whether the draw lies inside the circle.
    pub(super) fn burst(actor: &AiActor, tick: u64) -> Option<(u32, f64, bool)> {
        let fire = actor.strike()?.gunship_fire(tick)?;
        Some((
            fire.burst,
            fire.aim_ft[0].hypot(fire.aim_ft[1]),
            fire.inside,
        ))
    }

    /// A gun run's current (pass, burst) (slice ST-T2).
    pub(super) fn strafe_burst(actor: &AiActor) -> Option<(u32, u32)> {
        let s = actor.strike()?;
        (s.burst() > 0).then(|| (s.pass_number(), s.burst()))
    }

    /// Slice ST-O: a wing order is the seat's command, as in the game: the
    /// world reads the ground target from the seat's designation and sight,
    /// and the wingman takes or refuses it.
    pub(super) fn order_command(order: PlayerOrder) -> SeatCommand {
        SeatCommand::WingOrder(order)
    }
}

/// `--buddy ID`: the buddy's aircraft (the shared parser skips the option).
fn buddy_option() -> AppResult<Option<tore_formats::aircraft::AircraftId>> {
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == "--buddy")
        .map(|pair| tore_formats::aircraft::AircraftId::parse(&pair[1]))
        .transpose()
        .map_err(Into::into)
}

/// The buddy of a striker run: its plane, the commands it is to send and
/// whether it has designated.
struct Buddy {
    plane: PlaneId,
    commands: Vec<SeatCommand>,
    designated: Option<u32>,
}

/// A straight pass: where the striker starts and what it flies.
fn pass(scene: &Scene, o: &Options) -> AppResult<Pass> {
    let row = scene.target_row().ok_or("no target row")?;
    let aim = [
        row.position[0],
        scene.ground(row.position[0], row.position[2]),
        row.position[2],
    ];
    let heading = o.heading_deg.to_radians();
    let pitch = -o.dive_deg.abs().to_radians();
    let speed = o.speed_kt * FPS_PER_KNOT;
    let forward = [
        heading.sin() * pitch.cos(),
        pitch.sin(),
        heading.cos() * pitch.cos(),
    ];
    // The pass runs through the point `--altitude` above the target.
    let length = o.from_nm * FEET_PER_NM / pitch.cos();
    let start = std::array::from_fn(|i| {
        let through = if i == 1 { aim[1] + o.altitude } else { aim[i] };
        through - forward[i] * length
    });
    Ok(Pass {
        heading,
        pitch,
        speed,
        forward,
        start,
        aim,
    })
}

/// Puts `plane`'s cockpit on the pass at `along` feet from its start and
/// `right` feet to the right of it.
fn place(world: &mut World, plane: PlaneId, pass: &Pass, along: f64, right: f64) {
    place_at(world, plane, pass, [along, right, 0.]);
}

/// As [`place`], `up` feet higher.
fn place_at(world: &mut World, plane: PlaneId, pass: &Pass, [along, right, up]: [f64; 3]) {
    let Some(cockpit) = world.cockpits.iter_mut().find(|c| c.plane == plane) else {
        return;
    };
    let side = [pass.heading.cos(), 0., -pass.heading.sin()];
    let position: Vector = std::array::from_fn(|i| {
        pass.start[i] + pass.forward[i] * along + side[i] * right + if i == 1 { up } else { 0. }
    });
    let flight = &mut cockpit.flight;
    flight.position = position;
    flight.velocity = pass.forward.map(|v| v * pass.speed);
    flight.speed = pass.speed;
    flight.yaw = pass.heading;
    flight.pitch = pass.pitch;
    flight.bank = 0.;
    flight.vertical_speed = pass.forward[1] * pass.speed;
    flight.roll_rate = 0.;
    flight.pitch_rate = 0.;
}

/// One mission step. `human` is whether the main seat flies a plane;
/// `buddy` the buddy seat's commands (slice ST-L); `companion` whether
/// the render companion sits on the second seat (it gets an empty input:
/// nobody flies it).
fn step(
    scene: &mut Scene,
    human: bool,
    trigger: bool,
    buddy: Option<&mut Buddy>,
    companion: bool,
) -> AppResult<()> {
    if human {
        return scene.step(trigger);
    }
    let tick = scene.world.tick();
    let inputs: Vec<SeatInput> = match buddy {
        Some(b) => vec![SeatInput {
            seat: WINGMAN_SEAT,
            tick,
            commands: std::mem::take(&mut b.commands),
            ..SeatInput::default()
        }],
        // The render companion: an empty input, nobody flies it.
        None if companion => vec![SeatInput {
            seat: WINGMAN_SEAT,
            tick,
            ..SeatInput::default()
        }],
        None => Vec::new(),
    };
    scene.recorded(|world, out| world.step(&inputs, out))
}

/// What a laser last showed: its state and why it was masked.
type LaserShown = (LaserState, Option<Mask>);

/// Everything the watcher reports about the striker.
struct Watch {
    plane: PlaneId,
    target: u32,
    /// Rounds in flight: id to (last place, release number).
    flying: BTreeMap<u32, (Vector, usize)>,
    /// The target the striker released at, by release number: a Quick
    /// Mission wing picks its own target, which is not always the driver's
    /// (the miss is measured to the one it struck at, where it stands when
    /// the round ends).
    struck: BTreeMap<usize, u32>,
    seen: std::collections::BTreeSet<u32>,
    releases: usize,
    impacts: usize,
    kills: usize,
    last_release: Option<f64>,
    last_impact: Option<f64>,
    closest: Option<(f64, f64)>,
    egress: bool,
    /// When the striker egressed (`--follow-egress`).
    egress_at: Option<f64>,
    lost: Option<f64>,
    activity: Option<Activity>,
    profile: Option<Profile>,
    /// The gunship's gun rounds and the last burst reported (slice ST-F).
    gunship_rounds: usize,
    gunship_seen: BTreeSet<u32>,
    gunship_burst: Option<u32>,
    gunship_bursts: usize,
    /// The strike phase last reported.
    phase: Option<tore_sim::ai::strike::Phase>,
    /// The strike target last reported, and whether the committed-to-release
    /// window was open and when it first held a defence back (T.O.R.E,
    /// 2026-10-28).
    strike_target: Option<u32>,
    committed: bool,
    commit_held: Option<f64>,
    /// The no-laze decision last reported (slice ST-G).
    no_laze: Option<tore_sim::ai::strike::NoLaze>,
    /// A helicopter's threat decision last reported (slice H4).
    threat_call: Option<(tore_sim::ai::helicopter_threat::Decision, u64)>,
    /// A helicopter's move-ahead plan last reported (its planning tick).
    ahead: Option<u64>,
    /// The planes whose lasers are reported, and what each last showed.
    lasers: Vec<(u32, Option<LaserShown>)>,
    /// Ticks a laser weapon of the run flew guiding, and ticks of those its
    /// spot was gone (masked or off).
    fall_ticks: u64,
    blind_ticks: u64,
    /// Gun rounds in flight: id to (burst key, last place).
    gun_rounds: BTreeMap<u32, ((u32, u32), Vector)>,
    /// A gun run's bursts by (pass, burst): each is one release.
    bursts: BTreeMap<(u32, u32), Burst>,
    /// The last combat tick whose hits on the target were counted.
    hits_tick: u64,
    /// The lowest height above the ground the striker flew at, feet, and the
    /// seconds a helicopter break was flown (slice ST-H).
    min_agl_ft: f64,
    /// The least margin the striker kept to the reach of any defence it
    /// knew then (its ring less the ring's margin), feet; negative inside.
    reach_margin_ft: f64,
    break_ticks: u64,
    /// The striker's hit points last reported (`HURT` lines, slice ST-H).
    hp: Option<i32>,
    /// Rounds a ground site launched at the striker, already reported
    /// (`SHOT` lines).
    shots_seen: BTreeSet<u32>,
    /// Hostile sites the striker's ground picture has held (`SEEN` lines),
    /// and the last terrain line to each within 30,000 ft (`SIGHT` lines).
    sites_seen: BTreeSet<u32>,
    sight: BTreeMap<u32, bool>,
    /// How many ground contacts the picture last held (`PICTURE` lines).
    contacts: Option<usize>,
    /// When a site first launched at the striker (`SHOT` lines): the time
    /// and the site. `--sead-after-shot` counts from it.
    first_shot: Option<(f64, u32)>,
}

/// One gun burst of a strafing run: its release number, the rounds fired,
/// where those that reached the ground ended, the last round's time, and
/// whether its impact line is out.
struct Burst {
    n: usize,
    fired: usize,
    ends: Vec<Vector>,
    /// Rounds that ended within 50 ft of the target where it stood then.
    near: usize,
    /// Rounds that struck the target, and the hit points they took.
    hits: usize,
    damage: i32,
    last_fired: f64,
    reported: bool,
}

/// A burst's impact line goes out once its rounds have all ended and none
/// has been fired for this long (the AI's bursts are 0.5 s).
const BURST_DONE_SECONDS: f64 = 0.75;
/// A gun round that ends this close above the ground there reached it.
const ROUND_GROUND_FT: f64 = 100.;
/// A gun round that ends this close to the target (horizontally, where the
/// target stands then) counts as near it.
const NEAR_FT: f64 = 50.;

impl Watch {
    fn new(plane: PlaneId, target: u32) -> Self {
        Self {
            plane,
            target,
            flying: BTreeMap::new(),
            struck: BTreeMap::new(),
            seen: Default::default(),
            releases: 0,
            impacts: 0,
            kills: 0,
            last_release: None,
            last_impact: None,
            closest: None,
            egress: false,
            egress_at: None,
            lost: None,
            activity: None,
            profile: None,
            gunship_rounds: 0,
            gunship_seen: BTreeSet::new(),
            gunship_burst: None,
            gunship_bursts: 0,
            phase: None,
            strike_target: None,
            committed: false,
            commit_held: None,
            no_laze: None,
            threat_call: None,
            ahead: None,
            lasers: vec![(plane.0, None)],
            fall_ticks: 0,
            blind_ticks: 0,
            gun_rounds: BTreeMap::new(),
            bursts: BTreeMap::new(),
            hits_tick: 0,
            min_agl_ft: f64::INFINITY,
            reach_margin_ft: f64::INFINITY,
            break_ticks: 0,
            hp: None,
            shots_seen: BTreeSet::new(),
            sites_seen: BTreeSet::new(),
            sight: BTreeMap::new(),
            contacts: None,
            first_shot: None,
        }
    }

    /// Rounds still flying, or a burst whose impact is not out yet.
    fn busy(&self) -> bool {
        !self.flying.is_empty() || self.bursts.values().any(|b| !b.reported)
    }

    /// A helicopter keeping out of a ring it means to come back from: its
    /// latest threat decision is "leave", or it is holding off (slice H2,
    /// `--follow-threat`); or it has resumed and not fired since, so the
    /// shot after the resume is seen too (slice HF1).
    fn keeping_out(&self) -> bool {
        use tore_sim::ai::helicopter_threat::Decision;
        self.phase == Some(tore_sim::ai::strike::Phase::HoldOff)
            || self.threat_call.is_some_and(|(decision, tick)| {
                decision == Decision::Leave
                    || (decision == Decision::Resume
                        && self.last_release.is_none_or(|at| at < tick as f64 / 120.))
            })
    }
}

fn horizontal(a: Vector, b: Vector) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

impl Scene {
    /// The striker as the AI holds it, while the AI flies it.
    fn actor(&self, plane: PlaneId) -> Option<&AiActor> {
        self.world.ai_wings.as_ref()?.mission().actor(plane.0)
    }

    /// The striker's velocity, from the AI's flight or the seat's cockpit.
    fn striker_velocity(&self, plane: PlaneId) -> Vector {
        if let Some(actor) = self.actor(plane) {
            return actor.flight().velocity;
        }
        self.world
            .cockpits
            .iter()
            .find(|c| c.plane == plane)
            .map_or([0.; 3], |c| c.flight.velocity)
    }

    /// The striker's position, from the AI's flight or the seat's cockpit.
    fn striker_at(&self, plane: PlaneId) -> Option<(Vector, f64)> {
        if let Some(actor) = self.actor(plane) {
            let f = actor.flight();
            return Some((f.position, f.speed));
        }
        let f = &self
            .world
            .cockpits
            .iter()
            .find(|c| c.plane == plane)?
            .flight;
        Some((f.position, f.speed))
    }
}

/// The strike target whenever it changes (an early retarget picks the next
/// one inside a pass), and the committed-to-release window as it opens and
/// closes, with whether it held a defence back (T.O.R.E, John, 2026-10-28).
fn report_strike_flow(scene: &Scene, w: &mut Watch, t: f64) {
    let Some(wings) = scene.world.ai_wings.as_ref() else {
        return;
    };
    let Some(actor) = wings.mission().actor(w.plane.0) else {
        return;
    };
    let Some(strike) = actor.strike() else {
        return;
    };
    if strike.target() != w.strike_target {
        w.strike_target = strike.target();
        if let Some(target) = strike.target() {
            println!(
                "ai-strike: TARGET t={t:.3} target={target:#010x} phase={:?}",
                strike.phase()
            );
        }
    }
    let open = strike.holds_run(wings.mission().tick());
    if open
        && w.commit_held.is_none()
        && (actor.defense_decision().is_some_and(|d| d.motion.is_some())
            || actor.incoming_fire_cue().is_some())
    {
        w.commit_held = Some(t);
    }
    if open != w.committed {
        w.committed = open;
        println!(
            "ai-strike: COMMIT t={t:.3} open={} release_s={} held_defence={} held_at={}",
            if open { "yes" } else { "no" },
            strike
                .release_in_s()
                .filter(|s| s.is_finite())
                .map_or("none".into(), |s| format!("{s:.2}")),
            if w.commit_held.is_some() { "yes" } else { "no" },
            w.commit_held.map_or("none".into(), |at| format!("{at:.3}"))
        );
        if !open {
            w.commit_held = None;
        }
    }
}

/// The reporter: called after every tick.
fn observe(scene: &mut Scene, w: &mut Watch) {
    let t = scene.seconds();
    let tick = scene.world.combat.state.tick();
    let target_at = scene.target_row().map(|r| r.position);
    // The AI's activity, when it changes.
    let (activity, alive) = match scene.actor(w.plane) {
        Some(actor) => (Some(actor.activity()), actor.alive()),
        None => (
            None,
            scene.world.cockpits.iter().any(|c| c.plane == w.plane),
        ),
    };
    if let Some(activity) = activity
        && w.activity != Some(activity)
    {
        w.activity = Some(activity);
        println!("ai-strike: ACTIVITY t={t:.3} name={}", activity.label());
    }
    if let Some(profile) = scene.actor(w.plane).and_then(hooks::reported_profile)
        && w.profile != Some(profile)
    {
        w.profile = Some(profile);
        println!("ai-strike: PROFILE t={t:.3} name={}", profile.label());
    }
    // The gunship's gun rounds and bursts (slice ST-F): each burst once,
    // with the orbit's geometry as it starts.
    let state = &scene.world.combat.state;
    let guns: Vec<u32> = state
        .projectiles
        .iter()
        .filter(|p| p.owner == w.plane.0 && !w.gunship_seen.contains(&p.id))
        .filter(|p| p.gun_round.is_some() || p.weapon.as_ref().is_some_and(live::is_gun))
        .map(|p| p.id)
        .collect();
    w.gunship_rounds += guns.len();
    w.gunship_seen.extend(guns);
    let burst = scene
        .world
        .ai_wings
        .as_ref()
        .and_then(|wings| {
            wings
                .mission()
                .actor(w.plane.0)
                .map(|a| (wings.mission().tick(), a))
        })
        .and_then(|(tick, actor)| hooks::burst(actor, tick).map(|b| (b, actor.flight())));
    if let Some(((n, aim, inside), flight)) = burst
        && w.gunship_burst != Some(n)
        && let Some(tp) = target_at
    {
        w.gunship_burst = Some(n);
        w.gunship_bursts += 1;
        let radius = horizontal(flight.position, tp);
        let height = flight.position[1] - tp[1];
        println!(
            "ai-strike: BURST t={t:.3} n={n} aim_ft={aim:.0} inside={} radius_ft={radius:.0} height_ft={height:.0} slant_ft={:.0} bank_deg={:.1} speed_kt={:.0} rounds={}",
            if inside { "yes" } else { "no" },
            radius.hypot(height),
            flight.bank.to_degrees(),
            flight.speed / FPS_PER_KNOT,
            w.gunship_rounds,
        );
    }
    // The lowest the striker flew and, for a helicopter, its break once a
    // second (slice ST-H): heading, the threat's bearing and the break's
    // angle off it, what the terrain hid, the height, speed and bank.
    if alive && let Some(actor) = scene.actor(w.plane) {
        let f = actor.flight();
        w.min_agl_ft = w
            .min_agl_ft
            .min(f.position[1] - scene.ground(f.position[0], f.position[2]));
        // How near it came to the reach of a defence it knew (the move-ahead
        // check, John 2026-10-29), a quarter-second sample.
        if tick.is_multiple_of(30)
            && let Some(picture) = actor.ground_picture()
        {
            for ring in tore_sim::ai::helicopter_threat::known_rings(picture, actor.identity().side)
            {
                let reach = ring.radius_ft - tore_sim::ai::helicopter_threat::RING_MARGIN_FT;
                w.reach_margin_ft = w
                    .reach_margin_ft
                    .min(horizontal(f.position, ring.centre) - reach);
            }
        }
        if let Some((b, bearing)) = actor.trace().rotor_break {
            w.break_ticks += 1;
            if tick.is_multiple_of(120) {
                println!(
                    "ai-strike: BREAK t={t:.3} heading={:.0} track={:.0} bearing={} offset={:.0} masked_s={} blocked_s={} pitch={:.0} agl_ft={:.0} speed_kt={:.0} bank={:.0}",
                    f.yaw.to_degrees().rem_euclid(360.),
                    f.velocity[0]
                        .atan2(f.velocity[2])
                        .to_degrees()
                        .rem_euclid(360.),
                    bearing.map_or("none".to_owned(), |b| format!("{b:.0}")),
                    b.offset_deg,
                    b.masked_s,
                    b.blocked_s,
                    b.flight_path_pitch_deg,
                    f.position[1] - scene.ground(f.position[0], f.position[2]),
                    f.speed / FPS_PER_KNOT,
                    f.bank.to_degrees(),
                );
            }
        }
    }
    if let Some(row) = state.targets.iter().find(|row| row.id == w.plane.0)
        && w.hp != Some(row.hp)
    {
        if w.hp.is_some() {
            // The systems the hits have damaged: fault index and count.
            let faults = scene.actor(w.plane).map_or(String::new(), |a| {
                a.flight()
                    .systems
                    .counts
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| **n > 0)
                    .map(|(i, n)| format!("{i}x{n}"))
                    .collect::<Vec<_>>()
                    .join(",")
            });
            println!(
                "ai-strike: HURT t={t:.3} hp={}/{} faults={}",
                row.hp,
                row.initial_hp,
                if faults.is_empty() { "none" } else { &faults }
            );
        }
        w.hp = Some(row.hp);
    }
    if let Some(phase) = scene
        .actor(w.plane)
        .and_then(|a| a.strike())
        .map(|s| s.phase())
        && w.phase != Some(phase)
    {
        w.phase = Some(phase);
        println!("ai-strike: PHASE t={t:.3} name={phase:?}");
        if let Some(selection) = scene
            .actor(w.plane)
            .and_then(|a| a.strike())
            .and_then(|s| s.selection())
        {
            let row = scene
                .world
                .combat
                .state
                .targets
                .iter()
                .find(|r| r.id == selection.target);
            println!(
                "ai-strike: SELECTION t={t:.3} target={:#010x} station={} kind={:?} side={} class={:#06x} name={}",
                selection.target,
                selection.station.0,
                selection.kind,
                row.map_or(0, |r| r.side.0),
                row.map_or(0, |r| r.category),
                scene
                    .world
                    .combat
                    .ground_name(selection.target)
                    .unwrap_or("unnamed")
                    .replace(' ', "_")
            );
        }
        if scene.actor(w.plane).is_some_and(AiActor::is_rotorcraft) {
            print_rings(scene, w, t, target_at);
        }
    }
    report_strike_flow(scene, w, t);
    let no_laze = scene
        .actor(w.plane)
        .and_then(|a| a.strike())
        .and_then(|s| s.no_laze())
        .copied();
    if let Some(call) = no_laze
        && w.no_laze != Some(call.kind)
    {
        w.no_laze = Some(call.kind);
        let weapon = scene
            .world
            .ai_wings
            .as_ref()
            .and_then(|wings| wings.station_weapon(w.plane.0, call.station.0))
            .unwrap_or("?")
            .to_owned();
        println!(
            "ai-strike: NOLAZE t={t:.3} decision={} reason={} weapon={weapon} station={} target={} words=\"{}\"",
            match call.kind {
                tore_sim::ai::strike::NoLaze::Manual => "manual",
                tore_sim::ai::strike::NoLaze::Rtb => "rtb",
            },
            call.kind.reason(),
            call.station.0,
            call.target
                .map_or_else(|| "none".into(), |id| format!("{id:#x}")),
            call.kind.words(),
        );
    }
    // A helicopter's threat decision (slice H4), and every ten seconds of a
    // hold-off where it circles, how far out and how high.
    let threat_call = scene
        .actor(w.plane)
        .and_then(|a| a.strike())
        .and_then(|s| s.threat_call())
        .copied();
    if let Some(call) = threat_call
        && w.threat_call != Some((call.decision, call.tick))
    {
        w.threat_call = Some((call.decision, call.tick));
        println!(
            "ai-strike: THREAT t={t:.3} decision={} target={} ring={} words=\"{}\"",
            call.decision.reason(),
            call.target
                .map_or_else(|| "none".into(), |id| format!("{id:#x}")),
            call.ring
                .map_or_else(|| "none".into(), |id| format!("{id:#x}")),
            call.decision.words(),
        );
        print_rings(scene, w, t, target_at);
    }
    // A helicopter's move-ahead ambush, each time it is planned.
    let ahead = scene
        .actor(w.plane)
        .and_then(|a| a.strike())
        .and_then(|s| s.move_ahead())
        .copied();
    if ahead.map(|a| a.planned_tick) != w.ahead {
        w.ahead = ahead.map(|a| a.planned_tick);
        if let Some(plan) = ahead {
            let from = scene
                .striker_at(w.plane)
                .map_or(-1., |(p, _)| horizontal(p, plan.point));
            println!(
                "ai-strike: AHEAD t={t:.3} range_ft={:.0} fire_in_s={:.1} point={:.0},{:.0} aim_moves_ft={:.0} from_ft={from:.0}",
                plan.range_ft,
                plan.fire_tick.saturating_sub(tick) as f64 / 120.,
                plan.point[0],
                plan.point[2],
                target_at.map_or(-1., |tp| horizontal(tp, plan.aim)),
            );
            print_rings(scene, w, t, target_at);
        }
    }
    if w.phase == Some(tore_sim::ai::strike::Phase::HoldOff)
        && tick.is_multiple_of(1200)
        && let Some((p, _)) = scene.striker_at(w.plane)
        && let Some((centre, since)) = scene
            .actor(w.plane)
            .and_then(|a| a.strike())
            .and_then(|s| s.hold_off())
    {
        let agl = p[1] - scene.ground(p[0], p[2]);
        println!(
            "ai-strike: HOLDOFF t={t:.3} held_s={:.1} from_hold_ft={:.0} agl_ft={agl:.0} range_ft={:.0}",
            tick.saturating_sub(since) as f64 / 120.,
            (p[0] - centre[0]).hypot(p[2] - centre[2]),
            target_at.map_or(-1., |tp| (p[0] - tp[0]).hypot(p[2] - tp[2])),
        );
    }
    // The offset turn of a lase hold, every five seconds.
    if w.phase == Some(tore_sim::ai::strike::Phase::Lase)
        && tick.is_multiple_of(600)
        && let (Some((p, v)), Some(tp)) = (scene.striker_at(w.plane), target_at)
    {
        let bank = scene
            .actor(w.plane)
            .map_or(f64::NAN, |a| a.flight().bank.to_degrees());
        println!(
            "ai-strike: HOLD t={t:.3} range_ft={:.0} above_ft={:.0} speed_kt={:.0} bank_deg={bank:.0}",
            horizontal(p, tp),
            p[1] - tp[1],
            v / FPS_PER_KNOT,
        );
    }
    let state = &scene.world.combat.state;
    for (plane, last) in &mut w.lasers {
        let now = state
            .strike_cockpit(*plane)
            .map(|c| (c.laser.state, c.laser.mask));
        if now != *last {
            *last = now;
            if let Some((laser_state, mask)) = now {
                let range = state
                    .strike_cockpit(*plane)
                    .map_or(0., |c| c.laser.range_ft);
                println!(
                    "ai-strike: LASER t={t:.3} plane={plane} state={laser_state:?} mask={} range_ft={range:.0}",
                    mask.map_or("none".to_owned(), |m| format!("{m:?}"))
                );
            }
        }
    }
    // The fall of the run's laser weapons: guiding, and with the spot there.
    let guiding: Vec<_> = state
        .guided_flights()
        .filter(|(_, f)| f.mode == GuidedMode::Guiding && f.owner != u32::MAX)
        .filter(|(_, f)| f.spot_owner.is_some())
        .collect();
    if !guiding.is_empty() {
        w.fall_ticks += 1;
        let seen = guiding.iter().all(|(_, f)| {
            f.spot_owner
                .and_then(|owner| state.strike_cockpit(owner))
                .is_some_and(|c| c.laser.state == LaserState::Lasing)
        });
        if !seen {
            w.blind_ticks += 1;
        }
    }
    // Releases: rounds of the striker not seen before.
    let fresh: Vec<_> = state
        .projectiles
        .iter()
        .filter(|p| p.owner == w.plane.0 && !w.seen.contains(&p.id))
        .filter(|p| p.weapon.as_ref().is_none_or(|wp| !live::is_gun(wp)))
        .filter(|p| p.gun_round.is_none())
        .map(|p| (p.id, p.position, p.station, p.weapon.clone()))
        .collect();
    let at = scene.striker_at(w.plane);
    for (id, position, station, weapon) in fresh {
        w.seen.insert(id);
        if let Some(store) = weapon
            .as_ref()
            .filter(|wp| wp.source.starts_with("JETTISON-"))
        {
            println!(
                "ai-strike: JETTISON t={t:.3} id={id} weapon={} station={station}",
                store.source
            );
            continue;
        }
        w.releases += 1;
        w.flying.insert(id, (position, w.releases));
        w.last_release = Some(t);
        if let Some(struck) = scene
            .actor(w.plane)
            .and_then(|a| a.strike())
            .and_then(|s| s.target())
        {
            w.struck.insert(w.releases, struck);
        }
        let own = scene
            .world
            .combat
            .state
            .ownship(w.plane.0)
            .map(|o| o.configuration().stations[station].weapon.source.clone());
        let name = weapon.map(|wp| wp.source).or(own).unwrap_or_default();
        let (alt, speed, range) = match (at, target_at) {
            (Some((p, s)), Some(tp)) => (
                p[1] - scene.ground(p[0], p[2]),
                s / FPS_PER_KNOT,
                horizontal(p, tp),
            ),
            _ => (f64::NAN, f64::NAN, f64::NAN),
        };
        let aim = scene.actor(w.plane).and_then(hooks::aim).map_or(
            String::new(),
            |(offset, inside, pass)| {
                format!(
                    " pass={pass} aim_ft={offset:.0} inside={}",
                    if inside { "yes" } else { "no" }
                )
            },
        );
        let guided = scene
            .world
            .combat
            .state
            .guided_flight(id)
            .map_or(String::new(), |f| {
                format!(
                    " guided={:?} spot_owner={}",
                    f.mode,
                    f.spot_owner.map_or("none".to_owned(), |o| o.to_string())
                )
            });
        let lead = scene.actor(w.plane).map_or(0., hooks::lead);
        let dive = hooks::dive_deg(scene.striker_velocity(w.plane));
        println!(
            "ai-strike: RELEASE t={t:.3} n={} weapon={} station={station} alt_ft={alt:.0} speed_kt={speed:.0} range_ft={range:.0}{aim} lead_ft={lead:.0} dive_deg={dive:.0}{guided}",
            w.releases,
            if name.is_empty() { "unknown" } else { &name },
        );
    }
    // Rounds that ended this tick.
    let state = &scene.world.combat.state;
    let gone: Vec<u32> = w
        .flying
        .keys()
        .copied()
        .filter(|id| !state.projectiles.iter().any(|p| p.id == *id))
        .collect();
    for id in gone {
        let (last, n) = w.flying.remove(&id).expect("tracked");
        let burst = state
            .marks
            .iter()
            .filter(|m| m.born == tick && matches!(m.kind, MarkKind::Crater(_)))
            .map(|m| m.position)
            .chain(
                state
                    .effects
                    .iter()
                    .filter(|e| {
                        matches!(
                            e.kind,
                            EffectKind::Ground | EffectKind::Hit | EffectKind::Destroyed
                        )
                    })
                    .map(|e| e.position),
            )
            .map(|at| (horizontal(at, last), at))
            .filter(|(d, _)| *d < BURST_WITHIN_FT)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(last, |(_, at)| at);
        w.impacts += 1;
        w.last_impact = Some(t);
        let struck_at = w
            .struck
            .get(&n)
            .and_then(|id| state.targets.iter().find(|r| r.id == *id))
            .map(|r| r.position);
        match struck_at.or(target_at) {
            Some(tp) => println!(
                "ai-strike: IMPACT t={t:.3} n={n} miss_ft={:.1} at={:.0},{:.0},{:.0}",
                horizontal(burst, tp),
                burst[0],
                burst[1],
                burst[2]
            ),
            None => println!("ai-strike: IMPACT t={t:.3} n={n} miss_ft=nan"),
        }
    }
    // Rounds still flying keep their last place for the next tick's impact.
    for (id, (last, _)) in w.flying.iter_mut() {
        if let Some(p) = state.projectiles.iter().find(|p| p.id == *id) {
            *last = p.position;
        }
    }
    observe_guns(scene, w, t, target_at);
    observe_shots(scene, w, t);
    observe_sites(scene, w, t, tick);
    trace(scene, w, tick, target_at);
    let state = &scene.world.combat.state;
    // Kills credited to the striker.
    let kills = state
        .ledger
        .kills()
        .iter()
        .filter(|k| k.owner == w.plane.0)
        .count();
    while w.kills < kills {
        let kill = state
            .ledger
            .kills()
            .iter()
            .filter(|k| k.owner == w.plane.0)
            .nth(w.kills)
            .expect("counted");
        println!(
            "ai-strike: KILL t={t:.3} victim={:#010x} category={:#06x} target={}",
            kill.victim,
            kill.category,
            if kill.victim == w.target { "yes" } else { "no" }
        );
        w.kills += 1;
    }
    // The approach, the closest point and the egress.
    if let (Some((p, _)), Some(tp)) = (scene.striker_at(w.plane), target_at) {
        let range = horizontal(p, tp);
        if w.closest.is_none_or(|(r, _)| range < r) {
            w.closest = Some((range, t));
        }
        if let (Some(_), false) = (w.last_release, w.egress)
            && let Some((closest, _)) = w.closest
            && range > closest + EGRESS_OPENING_FT
            && !w.busy()
        {
            w.egress = true;
            w.egress_at = Some(t);
            println!(
                "ai-strike: EGRESS t={t:.3} range_ft={range:.0} alt_ft={:.0}",
                p[1] - scene.ground(p[0], p[2])
            );
        }
    }
    if w.lost.is_none() && !alive {
        w.lost = Some(t);
        let reason = scene
            .world
            .combat
            .state
            .targets
            .iter()
            .find(|row| row.id == w.plane.0)
            .map_or("left-the-mission", |row| {
                if row.hp <= 0 {
                    "destroyed"
                } else {
                    "not-flying"
                }
            });
        let crashed = scene.actor(w.plane).is_some_and(|a| a.flight().crashed);
        println!(
            "ai-strike: LOST t={t:.3} reason={reason} crashed={}",
            if crashed { "yes" } else { "no" }
        );
    }
}

/// `--place-site`: moves the standing hostile site `id` to `bearing` degrees
/// and `range` feet from the target, on the ground there: its surface unit
/// (where it sees and fires from), its arms and its combat row and hit box
/// (where it is seen and hit). The drawn scene keeps it where the mission
/// put it: the driver draws nothing of it. Prints a `PLACED` line.
fn place_site(
    scene: &mut Scene,
    id: u32,
    bearing: f64,
    range: f64,
    target_at: Vector,
) -> AppResult<()> {
    let (sin, cos) = bearing.to_radians().sin_cos();
    let [x, z] = [target_at[0] + range * sin, target_at[2] + range * cos];
    let ground = scene.ground(x, z);
    let row = scene
        .world
        .combat
        .state
        .targets
        .iter()
        .find(|row| row.id == id && row.aircraft.is_none() && row.category & 0x1800 != 0)
        .ok_or_else(|| format!("--place-site: {id:#x} is not a SAM or gun site"))?;
    let from = row.position;
    let bounds = scene
        .world
        .combat
        .state
        .ground_bounds(id)
        .ok_or_else(|| format!("--place-site: {id:#x} has no hit box"))?;
    let old_ground = scene.ground(from[0], from[2]);
    let shift = [x - from[0], ground - old_ground, z - from[2]];
    let moved = tore_sim::airport::OrientedBox {
        center: std::array::from_fn(|i| bounds.center[i] + shift[i]),
        ..bounds
    };
    if !scene
        .world
        .combat
        .state
        .move_ground_target(id, moved, [0.; 3], false)
    {
        return Err(format!("--place-site: {id:#x} could not be moved").into());
    }
    let surface = &mut scene.world.terrain.surface;
    let unit = surface
        .units
        .iter_mut()
        .find(|u| u.id.0 == id)
        .ok_or_else(|| format!("--place-site: {id:#x} is not a surface unit"))?;
    unit.position[0] = x.round() as i32;
    unit.position[2] = z.round() as i32;
    if let Some(arms) = surface.arsenal.units.iter_mut().find(|a| a.unit.0 == id) {
        arms.position = [x, arms.position[1] + shift[1], z];
    }
    println!(
        "ai-strike: PLACED id={id:#010x} bearing={bearing:.0} range_ft={range:.0} at={x:.0},{z:.0} moved_ft={:.0}",
        shift[0].hypot(shift[2])
    );
    Ok(())
}

/// `--site-sight`: limits how far an eye lists the hostile site `id` from
/// (its `maxVisDist`, in units of 256 ft, at least 1: 0 means no limit).
/// Prints a `SIGHTLIMIT` line.
fn site_sight(scene: &mut Scene, id: u32, feet: f64) -> AppResult<()> {
    let state = &mut scene.world.combat.state;
    if !state
        .targets
        .iter()
        .any(|row| row.id == id && row.aircraft.is_none() && row.category & 0x1800 != 0)
    {
        return Err(format!("--site-sight: {id:#x} is not a SAM or gun site").into());
    }
    let listing = state
        .ground_listing(id)
        .ok_or_else(|| format!("--site-sight: {id:#x} has no listing"))?;
    let units = (feet / 256.).round().clamp(1., f64::from(u16::MAX)) as u16;
    state.set_ground_listing(
        id,
        live::GroundListing {
            max_visible: units,
            ..listing
        },
    );
    println!(
        "ai-strike: SIGHTLIMIT id={id:#010x} sight_ft={:.0} was_ft={:.0}",
        f64::from(units) * 256.,
        f64::from(listing.max_visible) * 256.
    );
    Ok(())
}

/// `--terrain-map`: the ground height (feet, rounded) on a grid round the
/// target, one `TERRAIN` line per row from the north down, columns from the
/// west. The header gives the grid: the target's place and the step.
fn print_terrain_map(scene: &Scene, target_at: Vector, radius: f64, step: f64) {
    let n = (radius / step).floor() as i32;
    println!(
        "ai-strike: TERRAINMAP radius_ft={radius:.0} step_ft={step:.0} cells={} target_ground_ft={:.0}",
        2 * n + 1,
        scene.ground(target_at[0], target_at[2])
    );
    for row in (-n..=n).rev() {
        let heights: Vec<String> = (-n..=n)
            .map(|col| {
                let x = target_at[0] + f64::from(col) * step;
                let z = target_at[2] + f64::from(row) * step;
                format!("{:.0}", scene.ground(x, z))
            })
            .collect();
        println!(
            "ai-strike: TERRAIN north_ft={:.0} heights={}",
            f64::from(row) * step,
            heights.join(",")
        );
    }
}

/// `--terrain-nodes`: the ground height at each node of the terrain lattice
/// within `radius` feet of the target. The header gives the lattice spacing,
/// the node numbers of the first (south-west) node and the grid size; each
/// `NODE` line is one row of east-going heights from the south.
fn print_terrain_nodes(scene: &Scene, target_at: Vector, radius: f64) {
    let cell = f64::from(tore_formats::theater::CELL_FEET);
    let first = |centre: f64| ((centre - radius) / cell).floor().max(0.) as i32;
    let last = |centre: f64| ((centre + radius) / cell).ceil() as i32;
    let (i0, i1) = (first(target_at[0]), last(target_at[0]));
    let (j0, j1) = (first(target_at[2]), last(target_at[2]));
    println!(
        "ai-strike: TERRAINNODES cell_ft={cell:.0} i0={i0} j0={j0} cols={} rows={}",
        i1 - i0 + 1,
        j1 - j0 + 1
    );
    for j in j0..=j1 {
        let heights: Vec<String> = (i0..=i1)
            .map(|i| {
                format!(
                    "{:.0}",
                    scene
                        .world
                        .terrain
                        .raw_height((f64::from(i) * cell) as f32, (f64::from(j) * cell) as f32)
                )
            })
            .collect();
        println!("ai-strike: NODE j={j} heights={}", heights.join(","));
    }
}

/// `RINGS`: the rings a helicopter striker knows now (slice H4): id,
/// radius and distance from the target, feet.
fn print_rings(scene: &Scene, w: &Watch, t: f64, target_at: Option<Vector>) {
    if let Some(actor) = scene.actor(w.plane)
        && let Some(picture) = actor.ground_picture()
    {
        let rings = tore_sim::ai::helicopter_threat::known_rings(picture, actor.identity().side);
        let text: Vec<String> = rings
            .iter()
            .map(|r| {
                format!(
                    "{:#x}:{:.0}:{:.0}",
                    r.id,
                    r.radius_ft,
                    target_at.map_or(-1., |tp| (r.centre[0] - tp[0]).hypot(r.centre[2] - tp[2]))
                )
            })
            .collect();
        println!(
            "ai-strike: RINGS t={t:.3} n={} rings={}",
            rings.len(),
            text.join(",")
        );
    }
}

/// `TORE_AI_STRIKE_TRACE=1`: a `trace` line every half second with the
/// striker's flight and strike state, for tuning a pass by eye.
fn trace(scene: &Scene, w: &Watch, tick: u64, target_at: Option<Vector>) {
    if !tick.is_multiple_of(60) || std::env::var_os("TORE_AI_STRIKE_TRACE").is_none() {
        return;
    }
    let (Some(actor), Some(tp)) = (scene.actor(w.plane), target_at) else {
        return;
    };
    let f = actor.flight();
    let v = f.velocity;
    let fpp = v[1].atan2(v[0].hypot(v[2])).to_degrees();
    let (phase, segment) = actor
        .strike()
        .map_or(("none".to_owned(), "none".to_owned()), |s| {
            (format!("{:?}", s.phase()), format!("{:?}", s.segment()))
        });
    // A helicopter's margin to the nearest ring it knows (slice H4):
    // negative inside one.
    let ring_ft = actor
        .ground_picture()
        .map(|picture| {
            tore_sim::ai::helicopter_threat::known_rings(picture, actor.identity().side)
                .iter()
                .map(|r| horizontal(f.position, r.centre) - r.radius_ft)
                .fold(f64::INFINITY, f64::min)
        })
        .filter(|m| m.is_finite())
        .map_or_else(|| "none".to_owned(), |m| format!("{m:.0}"));
    println!(
        "ai-strike: trace t={:.1} range_ft={:.0} agl_ft={:.0} speed_fps={:.0} fpp={fpp:.1} bank={:.0} heading={:.0} track={:.0} bearing={:.0} pitch={:.0} phase={phase} segment={segment} ring_ft={ring_ft}",
        tick as f64 / 120.,
        horizontal(f.position, tp),
        f.position[1] - scene.ground(f.position[0], f.position[2]),
        f.speed,
        f.bank.to_degrees(),
        f.yaw.to_degrees().rem_euclid(360.),
        v[0].atan2(v[2]).to_degrees().rem_euclid(360.),
        (tp[0] - f.position[0])
            .atan2(tp[2] - f.position[2])
            .to_degrees()
            .rem_euclid(360.),
        f.pitch.to_degrees(),
    );
}

/// `PICTURE`, `SEEN` and `SIGHT`: how many ground contacts the striker's
/// picture holds each time that changes (it holds the 50 nearest the eye
/// sees), when a hostile SAM or gun site first enters the
/// striker's ground picture (`SEEN`: the site, the range, and the ring it
/// gives), and each time the terrain line between the striker and a site
/// within 30,000 ft opens or closes (`SIGHT`, the sight line the eye uses),
/// for reading why a site was or was not known when it fired. Looked at
/// every quarter second, the cadence the picture follows its contacts at.
fn observe_sites(scene: &Scene, w: &mut Watch, t: f64, tick: u64) {
    if !tick.is_multiple_of(30) {
        return;
    }
    let state = &scene.world.combat.state;
    let Some((own, _)) = scene.striker_at(w.plane) else {
        return;
    };
    let known: Option<Vec<u32>> = scene.actor(w.plane).and_then(|actor| {
        let picture = actor.ground_picture()?;
        let rings = tore_sim::ai::helicopter_threat::known_rings(picture, actor.identity().side);
        Some(rings.iter().map(|r| r.id).collect())
    });
    let held = scene
        .actor(w.plane)
        .and_then(|actor| actor.ground_picture())
        .map(|picture| picture.contacts().len());
    if held.is_some() && w.contacts != held {
        w.contacts = held;
        println!(
            "ai-strike: PICTURE t={t:.3} contacts={}",
            held.unwrap_or_default()
        );
    }
    let ground = |x: f64, z: f64| scene.ground(x, z);
    for site in state
        .targets
        .iter()
        .filter(|row| row.aircraft.is_none() && row.category & 0x1800 != 0 && row.hp > 0)
    {
        let range = horizontal(site.position, own);
        if range <= 30_000. {
            let eye = [site.position[0], site.position[1] + 10., site.position[2]];
            let clear = tore_sim::combat::live::terrain_hit(own, eye, &ground).is_none();
            if w.sight.insert(site.id, clear) != Some(clear) {
                println!(
                    "ai-strike: SIGHT t={t:.3} site={:#x} range_ft={range:.0} clear={}",
                    site.id,
                    if clear { "yes" } else { "no" }
                );
            }
        }
        if known.as_ref().is_some_and(|ids| ids.contains(&site.id)) && w.sites_seen.insert(site.id)
        {
            println!(
                "ai-strike: SEEN t={t:.3} site={:#x} range_ft={range:.0}",
                site.id
            );
        }
    }
}

/// `SHOT`: a round (not a gun's) a ground site launched at the striker, on
/// the tick it is first seen: the site, its range to the striker, and
/// whether the striker knew the site then (it is among the rings of its
/// ground picture, and not only because this launch gave a hidden team
/// away; slice H4). An ambush is a shot from a site it did not know (Mi-24
/// ambush layout, 2026-10-28).
fn observe_shots(scene: &Scene, w: &mut Watch, t: f64) {
    let state = &scene.world.combat.state;
    let Some((own, _)) = scene.striker_at(w.plane) else {
        return;
    };
    for p in &state.projectiles {
        if p.gun_round.is_some() || p.target != Some(w.plane.0) || w.shots_seen.contains(&p.id) {
            continue;
        }
        let Some(site) = state
            .targets
            .iter()
            .find(|row| row.id == p.owner && row.aircraft.is_none() && row.category & 0x1800 != 0)
        else {
            continue;
        };
        w.shots_seen.insert(p.id);
        if w.first_shot.is_none() {
            w.first_shot = Some((t, site.id));
        }
        // A hidden team that this very launch gave away (the picture's
        // unmasked tick is at or after the launch, on the AI's clock) was not
        // known when it fired.
        let launch = scene
            .world
            .ai_wings
            .as_ref()
            .map_or(0, |wings| wings.mission().tick())
            .saturating_sub(p.age + 1);
        let known = scene.actor(w.plane).and_then(|actor| {
            let picture = actor.ground_picture()?;
            let rings =
                tore_sim::ai::helicopter_threat::known_rings(picture, actor.identity().side);
            Some(
                rings.iter().any(|r| r.id == site.id)
                    && picture.unmasked_at(site.id).is_none_or(|at| at < launch),
            )
        });
        println!(
            "ai-strike: SHOT t={t:.3} site={:#x} range_ft={:.0} known={}",
            site.id,
            horizontal(site.position, own),
            match known {
                Some(true) => "yes",
                Some(false) => "no",
                None => "none",
            }
        );
    }
}

/// A gun run's bursts (slice ST-T2): each burst is one `RELEASE` (its first
/// round, with the burst's own aim draw and `burst=B rounds_left=N`) and one
/// `IMPACT` once its rounds have ended: `miss_ft` from the target to the
/// middle of the rounds that reached the ground, `rounds`, `ground` (how
/// many reached it), `near` (how many ended within 50 ft of the target where
/// it stood as each ended), `hits` (how many struck it: a round inside its
/// volume) and `damage` (the hit points they took).
fn observe_guns(scene: &Scene, w: &mut Watch, t: f64, target_at: Option<Vector>) {
    let state = &scene.world.combat.state;
    let key = scene.actor(w.plane).and_then(hooks::strafe_burst);
    let fresh: Vec<(u32, Vector, usize, Option<String>)> = state
        .projectiles
        .iter()
        .filter(|p| p.owner == w.plane.0 && !w.seen.contains(&p.id))
        .filter(|p| p.gun_round.is_some() || p.weapon.as_ref().is_some_and(live::is_gun))
        .map(|p| {
            (
                p.id,
                p.position,
                p.station,
                p.weapon.as_ref().map(|wp| wp.source.clone()),
            )
        })
        .collect();
    for (id, position, station, name) in fresh {
        w.seen.insert(id);
        let Some(key) = key else {
            continue;
        };
        if !w.bursts.contains_key(&key) {
            w.releases += 1;
            w.last_release = Some(t);
            let (alt, speed, range) = match (scene.striker_at(w.plane), target_at) {
                (Some((p, s)), Some(tp)) => (
                    p[1] - scene.ground(p[0], p[2]),
                    s / FPS_PER_KNOT,
                    horizontal(p, tp),
                ),
                _ => (f64::NAN, f64::NAN, f64::NAN),
            };
            let aim = scene.actor(w.plane).and_then(hooks::aim).map_or(
                String::new(),
                |(offset, inside, pass)| {
                    format!(
                        " pass={pass} aim_ft={offset:.0} inside={}",
                        if inside { "yes" } else { "no" }
                    )
                },
            );
            let left = scene
                .actor(w.plane)
                .and_then(|a| {
                    a.stations()
                        .iter()
                        .find(|s| usize::from(s.station.0) == station)
                })
                .map_or("unknown".to_owned(), |s| match s.store.rounds {
                    Rounds::Finite(n) => n.to_string(),
                    Rounds::Unlimited => "unlimited".to_owned(),
                });
            let lead = scene.actor(w.plane).map_or(0., hooks::lead);
            let dive = hooks::dive_deg(scene.striker_velocity(w.plane));
            println!(
                "ai-strike: RELEASE t={t:.3} n={} weapon={} station={station} alt_ft={alt:.0} speed_kt={speed:.0} range_ft={range:.0}{aim} lead_ft={lead:.0} dive_deg={dive:.0} burst={} rounds_left={left}",
                w.releases,
                name.as_deref().unwrap_or("gun"),
                key.1,
            );
            w.bursts.insert(
                key,
                Burst {
                    n: w.releases,
                    fired: 0,
                    ends: Vec::new(),
                    near: 0,
                    hits: 0,
                    damage: 0,
                    last_fired: t,
                    reported: false,
                },
            );
        }
        let burst = w.bursts.get_mut(&key).expect("inserted");
        burst.fired += 1;
        burst.last_fired = t;
        w.gun_rounds.insert(id, (key, position));
    }
    // Rounds that ended this tick.
    let gone: Vec<u32> = w
        .gun_rounds
        .keys()
        .copied()
        .filter(|id| !state.projectiles.iter().any(|p| p.id == *id))
        .collect();
    let standing = scene.target_row().map(|r| r.position);
    for id in gone {
        let (key, last) = w.gun_rounds.remove(&id).expect("tracked");
        let Some(burst) = w.bursts.get_mut(&key) else {
            continue;
        };
        // Against the target where it stands as the round ends.
        if standing.is_some_and(|tp| horizontal(last, tp) <= NEAR_FT) {
            burst.near += 1;
        }
        if last[1] - scene.ground(last[0], last[2]) <= ROUND_GROUND_FT {
            burst.ends.push(last);
        }
    }
    // Hits on the target since the last tick, to the latest burst (the
    // striker is the only one shooting at it).
    let tick = state.tick();
    if let Some(burst) = w.bursts.values_mut().max_by_key(|b| b.n) {
        for hit in state
            .history
            .iter()
            .filter(|h| h.target == w.target && h.tick > w.hits_tick)
        {
            burst.hits += 1;
            burst.damage += hit.applied;
        }
    }
    w.hits_tick = tick;
    for (id, (_, last)) in w.gun_rounds.iter_mut() {
        if let Some(p) = state.projectiles.iter().find(|p| p.id == *id) {
            *last = p.position;
        }
    }
    // Bursts whose rounds have all ended.
    let flying: std::collections::BTreeSet<(u32, u32)> =
        w.gun_rounds.values().map(|(key, _)| *key).collect();
    for (key, burst) in w.bursts.iter_mut() {
        if burst.reported || flying.contains(key) || t - burst.last_fired < BURST_DONE_SECONDS {
            continue;
        }
        burst.reported = true;
        w.impacts += 1;
        w.last_impact = Some(t);
        let middle = (!burst.ends.is_empty()).then(|| {
            let k = burst.ends.len() as f64;
            std::array::from_fn::<f64, 3, _>(|i| burst.ends.iter().map(|e| e[i]).sum::<f64>() / k)
        });
        let (near, hits, damage) = (burst.near, burst.hits, burst.damage);
        match (middle, target_at) {
            (Some(m), Some(tp)) => println!(
                "ai-strike: IMPACT t={t:.3} n={} miss_ft={:.1} at={:.0},{:.0},{:.0} rounds={} ground={} near={near} hits={hits} damage={damage}",
                burst.n,
                horizontal(m, tp),
                m[0],
                m[1],
                m[2],
                burst.fired,
                burst.ends.len(),
            ),
            _ => println!(
                "ai-strike: IMPACT t={t:.3} n={} miss_ft=nan rounds={} ground={} near={near} hits={hits} damage={damage}",
                burst.n,
                burst.fired,
                burst.ends.len()
            ),
        }
    }
}

/// The seat designates the target on the A-G page, as a pilot does: the
/// nearest contact to the target on the page, else after a few seconds the
/// Easy targeting cheat's Ctrl+X (an aircraft with no ground radar). Returns
/// the designated id, if any.
fn designate(
    scene: &mut Scene,
    pass: &Pass,
    plane: PlaneId,
    prefer: Option<u16>,
) -> AppResult<Option<u32>> {
    scene.queue(SeatCommand::Combat(Command::ToggleAirGround));
    let easy = scene.world.combat.state.cheats.easy_targeting;
    let mut found = None;
    for tick in 0..(DESIGNATE_SECONDS * 120.) as u64 {
        pass.fly(scene, 0.);
        let own = scene
            .world
            .combat
            .state
            .ownship(plane.0)
            .ok_or("no ownship")?;
        let hostile = |id| {
            scene.world.combat.state.targets.iter().any(|target| {
                target.id == id
                    && target.hp > 0
                    && target.side != live::NO_SIDE
                    && target.side != own.side
            })
        };
        if let Some(id) = own.designated().filter(|id| hostile(*id)) {
            let class = scene
                .world
                .combat
                .state
                .targets
                .iter()
                .find(|t| t.id == id)
                .map_or(0, |t| t.category);
            if prefer.is_none_or(|wanted| class & wanted != 0) {
                found = Some(id);
                break;
            }
        }
        let aim = pass.aim;
        let d = |p: [f64; 3]| (p[0] - aim[0]).hypot(p[2] - aim[2]);
        // The contact nearest the target, of the preferred class if one is
        // listed (a class order is given on a SAM).
        let nearest = own
            .sensors
            .ground()
            .iter()
            .filter(|g| hostile(g.contact.id) && prefer.is_none_or(|class| g.class & class != 0))
            .min_by(|a, b| d(a.contact.position).total_cmp(&d(b.contact.position)))
            .map(|g| g.contact.id);
        if let Some(id) = nearest {
            scene.queue(SeatCommand::Combat(Command::DesignateTarget(id)));
        } else if tick > 4 * 120 && tick % 60 == 0 {
            scene.world.combat.state.cheats.easy_targeting = true;
            scene.queue(SeatCommand::Combat(Command::NearestGround));
        }
        scene.step(false)?;
    }
    scene.world.combat.state.cheats.easy_targeting = easy;
    Ok(found)
}

pub(super) fn run(o: &Options, resources: &BTreeMap<String, Vec<u8>>) -> AppResult<()> {
    let cfg = &o.ai;
    let skill = Skill::ALL[o.skill.unwrap_or(1) as usize];
    let ordering = cfg.order.is_some();
    let mut spec = MissionSpec::new(&o.theater, o.aircraft);
    spec.ground_target = Some(o.stem.clone());
    spec.aaa = Defense::from_level(o.defenses.0).ok_or("defense levels are 0 to 3")?;
    spec.sam = Defense::from_level(o.defenses.1).ok_or("defense levels are 0 to 3")?;
    spec.surface_seed = o.seed;
    spec.wings[0].skill = skill;
    if let Some(condition) = o.condition {
        spec.condition = condition;
    }
    // `--render-scene` needs a seat's plane to present the picture from: a
    // companion lead the seat takes, flying well clear of the striker.
    let companion = (cfg.render_scene.is_some() || cfg.console_scene.is_some())
        && !ordering
        && cfg.handback.is_none();
    spec.wings[0].count = if ordering {
        1 + usize::from(cfg.order_wingmen.unwrap_or(1))
    } else {
        1
    };
    if companion && cfg.quick_wing {
        return Err("--render-scene and --quick-wing both use friendly wing 2: pick one".into());
    }
    // A Quick Mission's own strike wing: friendly wing 2, which the
    // mission's role rule sends when the aircraft is a bomber.
    let quick = cfg.quick_wing;
    if quick {
        spec.wings[1] = tore_world::mission::WingSpec {
            aircraft: o.aircraft,
            count: 2,
            skill,
        };
    }
    if cfg.clear_air_at.is_some() {
        spec.objectives[1] = tore_sim::ai::engagement::GroupObjective::Strike;
        spec.wings[3].count = 1;
        spec.objectives[3] = tore_sim::ai::engagement::GroupObjective::Hold;
    }
    let buddy_aircraft = buddy_option()?;
    if buddy_aircraft.is_some() && (ordering || cfg.handback.is_some() || quick || companion) {
        return Err(
            "--buddy is a run of its own: no --order, --handback, --quick-wing or --render-scene"
                .into(),
        );
    }
    if let Some(buddy) = buddy_aircraft {
        spec.wings[1] = WingSpec {
            aircraft: buddy,
            count: 1,
            ..spec.wings[0]
        };
        spec.cheats.easy_targeting = true;
    }
    if companion {
        // Friendly wing 2, one aircraft, the seat's.
        spec.wings[1].aircraft = o.aircraft;
        spec.wings[1].count = 1;
    }
    if let Some(altitude_ft) = cfg.mission_altitude {
        spec.start = tore_world::mission::Start::Airborne { altitude_ft };
    }
    spec.validate()?;
    let mut world = World::new(&spec, resources, Seating::Open)?;
    if let Some(why) = &world.terrain.surface.unresolved {
        return Err(format!("the ground target stands nowhere: {why}").into());
    }
    let targets =
        tore_world::ai_wings::outcome::ground_targets(&world.terrain.surface, &world.combat.state);
    let target = match o.unit {
        Some(id) => id,
        None => *targets
            .get(o.target)
            .ok_or_else(|| format!("{} has {} ground targets", o.stem, targets.len()))?,
    };
    // The striker is plane 0, or the wingman (plane 1) when a seat leads.
    let striker = PlaneId(if quick { 1 } else { u32::from(ordering) });
    let lead = PlaneId(0);
    let striker_seat = if ordering { WINGMAN_SEAT } else { SEAT };

    // Take the striker on a seat to fit its store and put it on the pass: the
    // AI's own aircraft take a loadout that way for the player-side kinds too.
    world.take_plane(striker_seat, striker)?;
    let station = fit_store(&mut world, striker.0, o, resources)?;
    let mut scene = Scene {
        world,
        out: TickOutput::default(),
        commands: Vec::new(),
        station,
        target,
        owner: striker.0,
        record: None,
        scenes: Default::default(),
        motors: Default::default(),
    };
    let pass = pass(&scene, o)?;
    let mut buddy = match buddy_aircraft {
        Some(_) => {
            let plane = scene
                .world
                .roster
                .planes()
                .iter()
                .find(|p| MissionSpec::wing_index(p.slot.wing) == 1 && p.slot.member == 0)
                .map(|p| p.id)
                .ok_or("no buddy plane")?;
            scene.world.take_plane(WINGMAN_SEAT, plane)?;
            Some(Buddy {
                plane,
                commands: Vec::new(),
                designated: None,
            })
        }
        None => None,
    };
    let row = scene.target_row().ok_or("the target has no row")?;
    let (target_at, hp) = (row.position, row.hp);
    {
        let own = scene
            .world
            .combat
            .state
            .ownship(striker.0)
            .ok_or("no ownship")?;
        let s = &own.configuration().stations[station];
        println!(
            "ai-strike: setup aircraft={:?} load={} station={station} count={} rounds={} target={target:#010x} name={} target_at={:.0},{:.0},{:.0} hp={hp} skill={} profile={} handback={} order={} mode={}",
            o.aircraft,
            s.weapon.source,
            s.count,
            own.rounds(station),
            scene
                .world
                .combat
                .ground_name(target)
                .unwrap_or("unnamed")
                .replace(' ', "_"),
            target_at[0],
            target_at[1],
            target_at[2],
            o.skill.unwrap_or(1),
            cfg.profile.map_or("none", Profile::label),
            cfg.handback.map_or("none".to_owned(), |s| format!("{s}")),
            cfg.order.map_or("none", |(name, _)| name),
            cfg.mode.map_or("none", DeliveryMode::label),
        );
        for &(id, bearing, range) in &cfg.place_sites {
            place_site(&mut scene, id, bearing, range, target_at)?;
        }
        for &(id, feet) in &cfg.site_sights {
            site_sight(&mut scene, id, feet)?;
        }
        if let Some((radius, step)) = cfg.terrain_map {
            print_terrain_map(&scene, target_at, radius, step);
        }
        if let Some(radius) = cfg.terrain_nodes {
            print_terrain_nodes(&scene, target_at, radius);
        }
        // The mission's hostile SAM and gun sites, once (slice ST-H): where
        // they stand against the target and the start, for reading a
        // defended run.
        let start = pass.start;
        for site in scene
            .world
            .combat
            .state
            .targets
            .iter()
            .filter(|row| row.category & 0x1800 != 0 && row.aircraft.is_none() && row.hp > 0)
        {
            println!(
                "ai-strike: SITE id={:#010x} name={} class={} hp={} target_ft={:.0} start_ft={:.0}",
                site.id,
                scene
                    .world
                    .combat
                    .ground_name(site.id)
                    .unwrap_or("unnamed")
                    .replace(' ', "_"),
                if site.category & 0x1000 != 0 {
                    "SAM"
                } else {
                    "AAA"
                },
                site.hp,
                horizontal(site.position, target_at),
                horizontal(site.position, start),
            );
        }
    }
    let mut human = false;
    if ordering {
        // The wingman starts 600 ft right of the lead and 300 ft behind; the
        // seat then flies the lead.
        place(&mut scene.world, striker, &pass, -300., 600.);
        scene.world.give_back_plane(striker_seat)?;
        for member in 2..=cfg.order_wingmen.unwrap_or(1) {
            let plane = PlaneId(u32::from(member));
            scene.world.take_plane(striker_seat, plane)?;
            place(
                &mut scene.world,
                plane,
                &pass,
                -300. * f64::from(member),
                -600.,
            );
            scene.world.give_back_plane(striker_seat)?;
        }
        scene.world.take_plane(SEAT, lead)?;
        place(&mut scene.world, lead, &pass, 0., 0.);
        scene.owner = lead.0;
        scene.station = 0;
        human = true;
    } else if quick {
        // The wing stays where the mission put it. Taking a plane to fit its
        // store rebuilt its AI actor without the role the mission gave it
        // (a takeover does not keep it yet, slice ST-K), so the mission's
        // role rule runs again.
        scene.world.give_back_plane(SEAT)?;
        if let Some(wings) = scene.world.ai_wings.as_mut() {
            wings.apply_strike_roles(
                &[tore_sim::ai::engagement::GroupObjective::Inherit; 6],
                true,
            );
        }
    } else if cfg.handback.is_some() {
        place(&mut scene.world, striker, &pass, 0., 0.);
        human = true;
    } else {
        place(&mut scene.world, striker, &pass, 0., cfg.offset_ft);
        scene.world.give_back_plane(SEAT)?;
        if companion {
            // The companion (friendly wing 2) on the second seat, well clear
            // of the pass; nobody flies it.
            scene.world.take_plane(WINGMAN_SEAT, COMPANION)?;
            place(
                &mut scene.world,
                COMPANION,
                &pass,
                -COMPANION_BEHIND_FT,
                COMPANION_RIGHT_FT,
            );
        }
    }
    if cfg.order_wingmen.is_some()
        && let Some(wings) = scene.world.ai_wings.as_ref()
    {
        for member in 1..=cfg.order_wingmen.unwrap_or(1) {
            let id = u32::from(member);
            if let (Some(actor), Some(config)) =
                (wings.mission().actor(id), wings.configuration(id))
            {
                for station in actor.stations() {
                    if let Some(store) = config.stations.get(usize::from(station.station.0)) {
                        println!(
                            "ai-strike: INVENTORY id={id} station={} weapon={} rounds={:?}",
                            station.station.0, store.weapon.source, station.store.rounds
                        );
                    }
                }
            }
        }
    }
    let mut watch = Watch::new(striker, target);
    let mut sead_done = false;
    if let Some(b) = &buddy {
        watch.lasers.push((b.plane.0, None));
    }
    // A buddy run lets the AI choose its store: a laser store needs a spot.
    let forced_station = (buddy.is_none() && !cfg.choose).then_some(station);
    // With a seat leading, the order gives the wingman its target; without
    // one the driver does.
    let note = if ordering {
        "ordered"
    } else if quick {
        "quick-wing"
    } else {
        hooks::assign(
            &mut scene.world,
            striker,
            target,
            forced_station,
            cfg.profile,
        )
    };
    println!(
        "ai-strike: PROFILE t=0.000 name=none requested={} note={note}",
        cfg.profile.map_or("none", Profile::label)
    );
    if quick && let Some(actor) = scene.actor(striker) {
        println!(
            "ai-strike: ROLE t=0.000 plane={} role={}",
            striker.0,
            match actor.strike_role() {
                StrikeRole::Strike => "strike",
                StrikeRole::Cap => "cap",
            }
        );
    }
    if let Some((p, _)) = scene.striker_at(striker) {
        println!(
            "ai-strike: start t=0.000 plane={} range_ft={:.0} alt_ft={:.0} speed_kt={:.0}",
            striker.0,
            horizontal(p, target_at),
            p[1] - scene.ground(p[0], p[2]),
            o.speed_kt
        );
    }

    // The human leg: designate, select the store, fly the pass, then hand
    // back (`--handback`) or give the order (`--order`).
    let mut leg_t = 0.;
    if human {
        let flier = if ordering { lead } else { striker };
        // A class order is given on a SAM (Attack SAMs).
        let prefer = (cfg.order.map(|(_, order)| order) == Some(PlayerOrder::EngageClass))
            .then_some(
                cfg.order_class
                    .unwrap_or(tore_sim::sensors::track::class::SAM),
            );
        let designated = designate(&mut scene, &pass, flier, prefer)?;
        if cfg.order_class.is_some() && designated.is_none() {
            return Err(
                "the order probe could not designate a hostile contact of the requested class"
                    .into(),
            );
        }
        match designated {
            Some(id) => {
                scene.target = id;
                watch.target = id;
                println!(
                    "ai-strike: DESIGNATE t={:.3} target={id:#010x} {}",
                    scene.seconds(),
                    scene.world.combat.ground_name(id).unwrap_or("unnamed")
                );
            }
            None => println!("ai-strike: DESIGNATE t={:.3} target=none", scene.seconds()),
        }
    }
    let mut scene = scene.recording(o)?;
    let mut ordered = false;
    let mut rendered = false;
    let mut console_due = cfg.console_at.clone();
    console_due.sort_by(f64::total_cmp);
    let mut first_release: Option<f64> = None;
    let mut handed = !human || ordering;
    let mut handed_at: Option<f64> = None;
    let mut kept_line: Option<String> = None;
    let started = scene.seconds();
    for tick in 0..(o.seconds * 120.) as u64 {
        if cfg
            .clear_air_at
            .is_some_and(|at| tick == (at * 120.) as u64)
        {
            let enemies: Vec<u32> = scene
                .world
                .ai_wings
                .as_ref()
                .into_iter()
                .flat_map(|w| w.slots())
                .filter(|s| s.side == tore_sim::ai::launch::Side::Enemy)
                .map(|s| s.id)
                .collect();
            for target in &mut scene.world.combat.state.targets {
                if enemies.contains(&target.id) {
                    target.hp = 0;
                }
            }
            println!(
                "ai-strike: CLEAR_AIR t={:.3} count={}",
                tick as f64 / 120.,
                enemies.len()
            );
        }
        if human {
            leg_t = tick as f64 / 120.;
            pass.fly(&mut scene, leg_t);
            if !ordering && !handed {
                scene.select();
                if let Some(mode) = cfg.mode
                    && scene.selected()
                    && scene.commands.is_empty()
                    && scene
                        .world
                        .combat
                        .state
                        .ownship(striker.0)
                        .is_some_and(|own| own.delivery_mode != mode)
                {
                    scene.queue(SeatCommand::Combat(Command::ToggleSeekerMode));
                }
                // `--laser`: on by hand (a laser that is on by the automatic
                // rule takes two presses: off by hand, then on by hand).
                if cfg.laser
                    && scene.selected()
                    && scene.commands.is_empty()
                    && scene
                        .world
                        .combat
                        .state
                        .ownship(striker.0)
                        .is_some_and(|own| own.laser.manual != Some(true))
                {
                    scene.queue(SeatCommand::Combat(Command::ToggleLaser));
                }
            }
            if ordering
                && !ordered
                && leg_t >= cfg.order_at
                && let Some((name, order)) = cfg.order
            {
                scene.queue(hooks::order_command(order));
                ordered = true;
                println!(
                    "ai-strike: ORDER t={:.3} kind={name} designated={}",
                    scene.seconds(),
                    scene
                        .world
                        .combat
                        .state
                        .ownship(lead.0)
                        .and_then(|own| own.designated())
                        .map_or("none".to_owned(), |id| format!("{id:#010x}"))
                );
            }
        }
        if let (Some(at), false) = (cfg.handback, handed)
            && leg_t >= at
        {
            let (designated, selected, mode, laser) = {
                let own = scene
                    .world
                    .combat
                    .state
                    .ownship(striker.0)
                    .ok_or("no ownship to hand back")?;
                (
                    own.designated(),
                    own.selected,
                    own.delivery_mode,
                    own.laser.manual,
                )
            };
            scene.world.give_back_plane(SEAT)?;
            handed = true;
            human = false;
            handed_at = Some(scene.seconds());
            println!(
                "ai-strike: HANDBACK t={:.3} designated={} selected={selected} mode={} laser={}",
                scene.seconds(),
                designated.map_or("none".to_owned(), |id| format!("{id:#010x}")),
                mode.label(),
                hooks::hand_words(laser)
            );
            match scene.actor(striker).and_then(hooks::takeover) {
                Some((target, station, mode)) => println!(
                    "ai-strike: TAKEOVER t={:.3} target={target:#010x} station={} mode={}",
                    scene.seconds(),
                    station.map_or("ai".to_owned(), |s| s.to_string()),
                    mode.label()
                ),
                None => println!("ai-strike: TAKEOVER t={:.3} none", scene.seconds()),
            }
        }
        // `--take-back`: the seat takes the aircraft again and the run ends
        // with what the human finds.
        if let (Some(after), Some(at)) = (cfg.take_back, handed_at)
            && scene.seconds() >= at + after
            && scene.actor(striker).is_some_and(AiActor::alive)
        {
            scene.world.take_plane(SEAT, striker)?;
            let own = scene
                .world
                .combat
                .state
                .ownship(striker.0)
                .ok_or("no ownship after the take")?;
            println!(
                "ai-strike: TAKEBACK t={:.3} designated={} selected={} mode={} laser={} rounds={}",
                scene.seconds(),
                own.designated()
                    .map_or("none".to_owned(), |id| format!("{id:#010x}")),
                own.selected,
                own.delivery_mode.label(),
                hooks::hand_words(own.laser.manual),
                own.rounds(station)
            );
            break;
        }
        if handed_at.is_some()
            && let Some(words) = hooks::kept_words(&scene.world.combat.state, striker)
            && kept_line.as_deref() != Some(words.as_str())
        {
            println!("ai-strike: KEPT t={:.3} {words}", scene.seconds());
            kept_line = Some(words);
        }
        // The buddy flies its scripted pass, designates and lases; the
        // striker is ordered onto what it designated.
        if let Some(b) = buddy.as_mut() {
            let t = tick as f64 / 120.;
            place_at(
                &mut scene.world,
                b.plane,
                &pass,
                [pass.speed * t + FEET_PER_NM, 3_000., 1_000.],
            );
            let own = scene.world.combat.state.ownship(b.plane.0);
            let designated = own.and_then(|own| own.designated());
            let laser = own.map(|own| own.laser);
            match designated {
                None if tick % 60 == 0 => {
                    b.commands.push(SeatCommand::Combat(Command::NearestGround))
                }
                None => {}
                Some(id) => {
                    if b.designated.is_none() {
                        b.designated = Some(id);
                        scene.target = id;
                        watch.target = id;
                        let note = hooks::assign(
                            &mut scene.world,
                            striker,
                            id,
                            forced_station,
                            cfg.profile,
                        );
                        println!(
                            "ai-strike: DESIGNATE t={:.3} target={id:#010x} buddy={} note={note}",
                            scene.seconds(),
                            b.plane.0
                        );
                    }
                    if laser.is_some_and(|l| !l.wanted()) && tick % 60 == 30 {
                        b.commands.push(SeatCommand::Combat(Command::ToggleLaser));
                    }
                }
            }
        }
        // `--sead-at S`: the stand-in for a SEAD flight (slice H4); or
        // `--sead-after-shot S`, S seconds after a named site first fires.
        let after_shot = cfg.sead_after_shot.and_then(|after| {
            watch
                .first_shot
                .filter(|(_, site)| cfg.sead_sites.is_empty() || cfg.sead_sites.contains(site))
                .map(|(at, _)| at + after)
        });
        if let Some(at) = cfg.sead_at.or(after_shot)
            && !sead_done
            && scene.seconds() >= at
        {
            sead_done = true;
            let target = scene.target;
            let mut killed = 0;
            for row in scene.world.combat.state.targets.iter_mut().filter(|row| {
                row.category & 0x1800 != 0
                    && row.aircraft.is_none()
                    && row.hp > 0
                    && row.id != target
                    && (cfg.sead_sites.is_empty() || cfg.sead_sites.contains(&row.id))
            }) {
                row.hp = 0;
                killed += 1;
            }
            println!("ai-strike: SEAD t={:.3} killed={killed}", scene.seconds());
        }
        let trigger = human && cfg.trigger && scene.selected();
        step(&mut scene, human, trigger, buddy.as_mut(), companion)?;
        for reply in &scene.out.orders {
            let (result, message) = match &reply.outcome {
                OrderOutcome::Given { message } => ("given", message),
                OrderOutcome::Refused { message } => ("refused", message),
                OrderOutcome::Failed { message } => ("failed", message),
            };
            println!(
                "ai-strike: ORDER-REPLY t={:.3} result={result} message=\"{message}\"",
                scene.seconds()
            );
            // What the wingman holds now: the order it took, if it took one.
            if let Some(actor) = scene.actor(striker) {
                let object =
                    |id: Option<u32>| id.map_or("none".to_owned(), |id| format!("{id:#010x}"));
                println!(
                    "ai-strike: ORDER-STATE t={:.3} strike_order={} class_order={} lase_order={}",
                    scene.seconds(),
                    object(actor.strike_order()),
                    actor
                        .ground_picture()
                        .map_or(0, |picture| picture.class_order().len()),
                    object(actor.lase_order())
                );
            }
        }
        observe(&mut scene, &mut watch);
        let t = scene.seconds();
        if let (Some(at), Some(path)) = (cfg.render_at, &o.render)
            && t >= at
            && t - 1. / 120. < at
        {
            render(&scene, resources, striker, path)?;
        }
        // `--console-scene`: the sensor console's frames at their seconds.
        if let Some(dir) = &cfg.console_scene {
            while console_due.first().is_some_and(|at| t >= *at) {
                let at = console_due.remove(0);
                super::console_render::render(&mut scene, COMPANION, striker, target, at, dir)?;
            }
        }
        // `--render-scene`: one frame a moment after the first release.
        if let (Some(dir), Some(at)) = (&cfg.render_scene, first_release.or(watch.last_release))
            && !rendered
            && t >= at + RENDER_AFTER_SECONDS
        {
            first_release = Some(at);
            rendered = true;
            let name = format!(
                "{}-{}-{}.png",
                o.stem.to_ascii_lowercase(),
                cfg.profile.map_or("free", Profile::label),
                o.skill.unwrap_or(1)
            );
            let target_at = scene.target_row().map_or(target_at, |r| r.position);
            super::ai_render::render(&mut scene, COMPANION, striker, target_at, &dir.join(name))?;
        }
        // Done when the striker is gone or the last round has landed and the
        // striker has had time to leave.
        let over = [
            watch.lost,
            watch.last_impact.filter(|_| {
                !(watch.busy()
                    || (cfg.follow_threat && watch.keeping_out())
                    || (cfg.follow_egress && !watch.egress))
            }),
            watch.egress_at.filter(|_| {
                cfg.follow_egress && !(watch.busy() || (cfg.follow_threat && watch.keeping_out()))
            }),
        ]
        .into_iter()
        .flatten()
        .fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))));
        if over.is_some_and(|at| t > at + SETTLE_SECONDS) {
            break;
        }
    }
    let t = scene.seconds();
    let hp_end = scene.target_row().map(|r| r.hp);
    println!(
        "ai-strike: summary t={t:.3} ran_s={:.1} profile={} releases={} impacts={} kills={} egress={} lost={} activity={} target_hp={}/{} closest_ft={} store_rounds_left={} gun_rounds={} bursts={} fall_s={:.1} blind_s={:.1} min_agl_ft={} break_s={:.1} reach_margin_ft={}",
        t - started,
        watch.profile.map_or("none", Profile::label),
        watch.releases,
        watch.impacts,
        watch.kills,
        if watch.egress { "yes" } else { "no" },
        if watch.lost.is_some() { "yes" } else { "no" },
        watch.activity.map_or("human", Activity::label),
        hp_end.map_or("gone".to_owned(), |hp| hp.to_string()),
        hp,
        watch
            .closest
            .map_or("none".to_owned(), |(r, _)| format!("{r:.0}")),
        scene
            .actor(striker)
            .and_then(|actor| actor.stations().get(station))
            .map_or("human".to_owned(), |s| match s.store.rounds {
                Rounds::Finite(n) => n.to_string(),
                Rounds::Unlimited => "unlimited".to_owned(),
            }),
        watch.gunship_rounds,
        watch.gunship_bursts,
        watch.fall_ticks as f64 / 120.,
        watch.blind_ticks as f64 / 120.,
        if watch.min_agl_ft.is_finite() {
            format!("{:.0}", watch.min_agl_ft)
        } else {
            "none".to_owned()
        },
        watch.break_ticks as f64 / 120.,
        if watch.reach_margin_ft.is_finite() {
            format!("{:.0}", watch.reach_margin_ft)
        } else {
            "none".to_owned()
        },
    );
    if watch.releases == 0 {
        println!("ai-strike: no release");
    }
    if cfg.order_wingmen.is_some() {
        for member in 1..=cfg.order_wingmen.unwrap_or(1) {
            let id = u32::from(member);
            let tally = scene.world.combat.state.ledger.total(|key| key.owner == id);
            println!(
                "ai-strike: WINGMAN id={id} shots={} hits={} damage={}",
                tally.launched, tally.hit, tally.damage
            );
        }
    }
    if scene.record.is_some() {
        scene.finish()?;
    }
    println!("ai-strike: done");
    Ok(())
}

/// One offscreen picture (slice ST-F): the striker from just outside it,
/// looking past it down at its target, with the ground units, the rounds in flight (tracers)
/// and the effects of this tick. A development aid, as `--surface-scene`.
fn render(
    scene: &Scene,
    resources: &BTreeMap<String, Vec<u8>>,
    plane: PlaneId,
    path: &std::path::Path,
) -> AppResult<()> {
    let world = &scene.world;
    let actor = scene
        .actor(plane)
        .ok_or("the AI does not fly the striker")?;
    let state = actor.flight().clone();
    let target = scene.target_row().map_or(state.position, |r| r.position);
    let airframe = crate::aircraft::Airframe::load(resources, actor.identity().aircraft)?;
    let mut scenery = crate::scenery::Scenery::build(resources, &world.terrain)?;
    let art = crate::render_snapshot::CombatArt::load(resources)?;
    let mut gpu = pollster::block_on(crate::reel::Gpu::new(&scenery))?;
    // The camera: on the line from the target out through the striker,
    // 450 ft beyond it and 150 ft behind it on its track, looking at the
    // target, so the gunship stands in front of what it shoots at.
    let (sin, cos) = state.yaw.sin_cos();
    let out: Vector = std::array::from_fn(|i| state.position[i] - target[i]);
    let length = out.iter().map(|v| v * v).sum::<f64>().sqrt().max(1.);
    let mut camera = crate::camera::Camera::new();
    camera.position = [
        state.position[0] + out[0] / length * 450. - sin * 150.,
        state.position[1] + out[1] / length * 450.,
        state.position[2] + out[2] / length * 450. - cos * 150.,
    ];
    let d: Vector = std::array::from_fn(|i| target[i] - camera.position[i]);
    camera.yaw = d[0].atan2(d[2]) as f32;
    camera.pitch = d[1].atan2(d[0].hypot(d[2])) as f32;
    camera.zoom = (30_f64.to_radians().tan() / 22_f64.to_radians().tan()) as f32;
    camera.near_clip = 2.;
    scenery.resolve_palette(&world.terrain, camera.position[1]);
    scenery.set_origin(camera.position);
    let snapshot = world.combat.render_snapshot().clone();
    let standing = crate::render_snapshot::standing(&world.combat.state.targets);
    let geometry = std::sync::Arc::clone(
        scenery.static_geometry(&world.combat.state.targets, &world.combat.surface),
    );
    gpu.sim.airports(&gpu.device, &gpu.queue, &geometry);
    gpu.sim.surface_units(
        &gpu.device,
        &gpu.queue,
        &scenery.surface_vertices(&snapshot, &|id| standing.contains(&id), &camera),
    );
    let contrails = tore_sim::combat::smoke::Smoke::default();
    gpu.sim.smoke(
        &gpu.device,
        &gpu.queue,
        &art.smoke,
        [&world.combat.state.smoke, &contrails],
        &world.combat.state.devices,
    );
    gpu.sim.effects(
        &gpu.device,
        &gpu.queue,
        &art.effects,
        &snapshot.effects,
        &snapshot.marks,
    );
    gpu.sim.combat(
        &gpu.device,
        &gpu.queue,
        &crate::render_snapshot::combat_geometry(
            &snapshot,
            &art,
            &airframe,
            &state,
            &camera,
            &world.terrain,
            &scenery,
        ),
    );
    let vertices = airframe.vertices(&state, &camera, &world.terrain, &scenery);
    gpu.sim
        .aircraft(&gpu.device, &gpu.queue, &airframe, &vertices);
    let pixels = gpu.pixels(&camera, &world.terrain, &scenery)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(
        path,
        crate::replay::png::encode_rgba(crate::reel::WIDTH, crate::reel::HEIGHT, &pixels)?,
    )?;
    println!(
        "ai-strike: RENDER t={:.3} path={} range_ft={:.0} height_ft={:.0} bank_deg={:.1}",
        scene.seconds(),
        path.display(),
        horizontal(state.position, target),
        state.position[1] - target[1],
        state.bank.to_degrees(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(args: &[&str]) -> AppResult<Config> {
        let mut c = Config::new();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            match *arg {
                "--pilot" => c.pilot(it.next().unwrap())?,
                "--profile" => c.set_profile(it.next().unwrap())?,
                "--mode" => c.set_mode(it.next().unwrap())?,
                "--order" => c.set_order(it.next().unwrap())?,
                "--handback" => c.handback = Some(it.next().unwrap().parse()?),
                "--trigger" => c.trigger = true,
                "--laser" => c.laser = true,
                "--take-back" => c.take_back = Some(it.next().unwrap().parse()?),
                other => panic!("{other}"),
            }
        }
        Ok(c)
    }

    #[test]
    fn profiles_round_trip_and_unknown_ones_are_refused() {
        for profile in Profile::ALL {
            assert_eq!(Profile::parse(profile.label()).unwrap(), profile);
        }
        assert_eq!(Profile::parse("DIVE").unwrap(), Profile::Dive);
        assert!(Profile::parse("loft").is_err());
    }

    #[test]
    fn order_names_are_unique_and_parse() {
        let mut names: Vec<_> = ORDERS.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ORDERS.len());
        let c = config(&["--order", "Engage-My-Target"]).unwrap();
        assert_eq!(c.order.map(|(_, o)| o), Some(PlayerOrder::EngageMyTarget));
        for kind in ["engage-ir-laser", "engage-class", "lase-my-target"] {
            assert!(config(&["--order", kind]).is_ok(), "{kind}");
        }
        assert!(config(&["--order", "lase-your-target"]).is_err());
    }

    #[test]
    fn the_options_only_make_sense_with_the_ai_flying() {
        assert!(config(&[]).unwrap().check(None, false).is_ok());
        let c = config(&["--profile", "dive"]).unwrap();
        assert!(c.check(None, false).is_err());
        let c = config(&["--pilot", "ai", "--profile", "dive"]).unwrap();
        assert!(c.check(Some(3), false).is_ok());
        assert!(c.check(Some(4), false).is_err());
        assert!(c.check(None, true).is_err());
    }

    #[test]
    fn the_sead_stand_in_is_timed_one_way() {
        let mut c = config(&["--pilot", "ai"]).unwrap();
        c.sead_at = Some(60.);
        c.sead_after_shot = Some(4.);
        assert!(c.check(None, false).is_err(), "a clock and a shot together");
        c.sead_at = None;
        assert!(c.check(None, false).is_ok());
        c.sead_after_shot = Some(-1.);
        assert!(c.check(None, false).is_err(), "before the shot");
        c.sead_after_shot = Some(f64::NAN);
        assert!(c.check(None, false).is_err());
    }

    #[test]
    fn a_handback_and_an_order_are_separate_runs() {
        let c = config(&["--pilot", "ai", "--handback", "5", "--order", "bug-out"]).unwrap();
        assert!(c.check(None, false).is_err());
        let c = config(&["--pilot", "ai", "--mode", "lvl"]).unwrap();
        assert!(c.check(None, false).is_err());
        let c = config(&[
            "--pilot",
            "ai",
            "--handback",
            "5",
            "--mode",
            "lvl",
            "--trigger",
        ])
        .unwrap();
        assert!(c.check(None, false).is_ok());
        assert_eq!(c.mode, Some(DeliveryMode::Level));
        // The takeover legs (slice ST-K) need a handback too.
        let c = config(&["--pilot", "ai", "--laser"]).unwrap();
        assert!(c.check(None, false).is_err());
        let c = config(&["--pilot", "ai", "--take-back", "40"]).unwrap();
        assert!(c.check(None, false).is_err());
        let c = config(&[
            "--pilot",
            "ai",
            "--handback",
            "5",
            "--laser",
            "--take-back",
            "40",
        ])
        .unwrap();
        assert!(c.check(None, false).is_ok());
        assert_eq!((c.laser, c.take_back), (true, Some(40.)));
    }
}
