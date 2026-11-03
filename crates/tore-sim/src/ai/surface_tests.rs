//! The surface controller's timing against the record numbers
//! (docs/spec/surface-defenses.md, "Engagement"; plan 9.1 W3).
use super::*;
use crate::checkpoint::{Models, round_trip};

const TPS: u64 = 120;

/// SA-6: NPC search 40, unready 144, attack 60 quarter seconds; SA6.JT
/// `trackT` 20, salvo 1, `reloadT` 60.
fn sa6(skill: i32) -> Profile {
    Profile {
        timing: Timing::from_quarters(40, 144, 60, 20, 32_767, skill),
        arm: Arm::Missile {
            salvo: 1,
            gap: QUARTER_SECOND_TICKS,
            spacing: 60 * QUARTER_SECOND_TICKS,
        },
    }
}

/// A ZSU-23 with the tuning table's row: 99 rounds in 1.75 s, 1 s pause,
/// 120 s swap; NPC 20 / 80 / 40; `trackT` 4.
fn zsu23() -> Profile {
    Profile {
        timing: Timing::from_quarters(20, 80, 40, 4, 32_767, 1),
        arm: Arm::Gun {
            burst: 99,
            burst_ticks: 7 * QUARTER_SECOND_TICKS,
            pause: 4 * QUARTER_SECOND_TICKS,
            opening: 0,
            opening_gap: 0,
            swap: 120 * TPS,
        },
    }
}

/// A T-72's 2A46 with the tuning table's row: one round, then 7.25 s; NPC
/// 20 / 80 / 40; `trackT` 4.
fn t72() -> Profile {
    Profile {
        timing: Timing::from_quarters(20, 80, 40, 4, 32_767, 1),
        arm: Arm::Gun {
            burst: 1,
            burst_ticks: QUARTER_SECOND_TICKS,
            pause: 29 * QUARTER_SECOND_TICKS,
            opening: 0,
            opening_gap: 0,
            swap: 60 * TPS,
        },
    }
}

fn stock(loaded: u32, magazines: u32) -> Stock {
    Stock {
        loaded,
        reserve: Reserve::Magazines(magazines),
        supply: false,
    }
}

struct Run {
    controller: Controller,
    profile: Profile,
    tick: u64,
    loaded: u32,
    reserve: u32,
    magazine: u32,
    eligible: Vec<u32>,
    in_range: bool,
    gates: bool,
    blind: bool,
    hold: bool,
    lay: u64,
    fired: Vec<(u64, FireRequest)>,
    swaps: Vec<u64>,
}

impl Run {
    fn new(profile: Profile, loaded: u32, reserve: u32) -> Self {
        Self {
            controller: Controller::new(),
            profile,
            tick: 0,
            loaded,
            reserve,
            magazine: loaded,
            eligible: vec![7],
            in_range: true,
            gates: true,
            blind: false,
            hold: false,
            lay: 0,
            fired: Vec::new(),
            swaps: Vec::new(),
        }
    }
    fn step(&mut self) {
        let inputs = Inputs {
            tick: self.tick,
            hostile_in_range: self.in_range,
            eligible: &self.eligible,
            gates: self.gates,
            stock: stock(self.loaded, self.reserve),
            blind: self.blind,
            slow: false,
            hold: self.hold,
            lay: self.lay,
        };
        let outcome = self.controller.advance(&self.profile, &inputs);
        if let Some(fire) = outcome.fire {
            self.loaded -= fire.rounds;
            self.fired.push((self.tick, fire));
        }
        if outcome.swap {
            self.reserve -= 1;
            self.loaded = self.magazine;
            self.swaps.push(self.tick);
        }
        self.tick += 1;
    }
    fn run_until(&mut self, end: u64) {
        while self.tick < end {
            self.step();
        }
    }
    fn first_phase(&mut self, phase: Phase, limit: u64) -> u64 {
        while self.controller.phase() != phase {
            assert!(self.tick < limit, "never reached {phase:?}");
            self.step();
        }
        self.tick - 1
    }
    /// Steps until a tick fires rounds; that tick.
    fn first_fire(&mut self, limit: u64) -> u64 {
        let before = self.fired.len();
        while self.fired.len() == before {
            assert!(self.tick < limit, "never fired");
            self.step();
        }
        self.tick - 1
    }
}

#[test]
fn sa6_times_match_the_record() {
    let mut run = Run::new(sa6(1), 3, 0);
    // Nothing eligible at first: a search, then a retry 10 s later.
    run.eligible.clear();
    run.step();
    assert_eq!(run.controller.phase(), Phase::Search);
    assert_eq!(run.controller.deadline(), Some(10 * TPS));
    run.eligible = vec![7];
    run.run_until(10 * TPS);
    assert_eq!(run.controller.phase(), Phase::Search);
    run.step();
    assert_eq!(run.controller.phase(), Phase::Prepare);
    // The first engagement's preparation is the unready 36 s.
    assert_eq!(run.controller.deadline(), Some(10 * TPS + 36 * TPS));
    let track = run.first_phase(Phase::Track, 100 * TPS);
    assert_eq!(track, 46 * TPS);
    // 5 s of lock, then the launch.
    let fire = run.first_fire(100 * TPS);
    assert_eq!(fire, 51 * TPS);
    assert_eq!(run.fired.len(), 1);
    assert_eq!(run.fired[0].0, 51 * TPS);
    assert!(run.fired[0].1.burst_start);
    // 15 s between salvos while the lock holds.
    run.run_until(80 * TPS);
    let times: Vec<u64> = run.fired.iter().map(|(t, _)| *t).collect();
    assert_eq!(times, vec![51 * TPS, 66 * TPS]);
    run.run_until(90 * TPS);
    // Three rails, then Empty.
    assert_eq!(run.fired.len(), 3);
    assert_eq!(run.controller.phase(), Phase::Empty);
}

#[test]
fn a_second_engagement_prepares_with_the_ordinary_time() {
    let mut run = Run::new(sa6(1), 3, 0);
    run.first_fire(100 * TPS);
    // The target leaves; a new one appears.
    run.eligible = vec![9];
    run.step();
    assert_eq!(run.controller.phase(), Phase::Prepare);
    assert_eq!(run.controller.target(), Some(9));
    assert_eq!(run.controller.deadline(), Some(run.tick - 1 + 15 * TPS));
}

#[test]
fn experience_scales_search_and_preparation_only() {
    let novice = sa6(0).timing;
    let ace = sa6(3).timing;
    assert_eq!(novice.unready, 54 * TPS);
    assert_eq!(ace.unready, 36 * TPS * 70 / 100);
    assert_eq!(novice.track, ace.track);
    assert_eq!(novice.search, 15 * TPS);
    assert_eq!(launch_range_percent(0), 80);
    assert_eq!(harm_shutdown_percent(2), 60);
    assert!((aim_error_deg(3, true) - 0.2).abs() < 1e-12);
    assert!((aim_error_deg(0, false) - 1.5).abs() < 1e-12);
}

#[test]
fn small_arms_aim_wider_than_any_cannon_and_tighten_with_experience() {
    // 4 times the visual guns' error at each experience (fitted).
    for (skill, expected) in [(0, 6.0), (1, 4.0), (2, 2.8), (3, 2.0)] {
        let squad = small_arms_aim_error_deg(skill);
        assert!((squad - expected).abs() < 1e-12, "skill {skill}: {squad}");
        assert!(squad > aim_error_deg(skill, false));
        assert!(squad > aim_error_deg(skill, true));
    }
    let errors: Vec<f64> = (0..4).map(small_arms_aim_error_deg).collect();
    assert!(errors.windows(2).all(|pair| pair[0] > pair[1]));
}

#[test]
fn tank_guns_aim_wider_at_aircraft_and_wider_still_at_a_fast_crossing_one() {
    for skill in 0..4 {
        let hover = tank_air_aim_error_deg(skill, 0.);
        let visual = aim_error_deg(skill, false);
        assert!((hover - visual - TANK_AIR_RANGING_ERROR_DEG).abs() < 1e-12);
        assert!(hover > visual && hover < small_arms_aim_error_deg(skill));
        // A 450 kt jet crossing 1,000 ft away turns the sight line at about
        // 44 degrees a second: the lag alone is several degrees.
        let crossing = tank_air_aim_error_deg(skill, 44.);
        assert!((crossing - hover - 44. * TANK_AIR_TRACKING_LAG_S).abs() < 1e-12);
        assert!(crossing > 5.);
        assert_eq!(
            tank_air_aim_error_deg(skill, -3.),
            tank_air_aim_error_deg(skill, 3.)
        );
    }
}

/// A tank gun's lay on an aircraft ([`Inputs::lay`]): the first round waits
/// for the lock and the lay inside the gates, later rounds follow the gun's
/// cycle, and a target that leaves the gates and comes back is laid on
/// again. Without a lay the lock held outside the gates fires at once.
#[test]
fn a_lay_holds_the_first_round_until_the_target_has_been_in_the_gates_that_long() {
    let lay = 3 * TPS;
    let mut run = Run::new(t72(), 22, 2);
    run.lay = lay;
    let track = run.first_phase(Phase::Track, 60 * TPS);
    let fire = run.first_fire(60 * TPS);
    assert_eq!(fire, track + TPS + lay, "1 s lock and the 3 s lay");
    let next = run.first_fire(60 * TPS);
    assert_eq!(
        next,
        fire + 30 * QUARTER_SECOND_TICKS,
        "the 7.5 s cycle, no new lay"
    );
    // Out of the gates for 2 s after the pause, then back.
    run.gates = false;
    run.run_until(next + 30 * QUARTER_SECOND_TICKS + 2 * TPS);
    assert_eq!(run.controller.phase(), Phase::Track);
    run.gates = true;
    let back = run.tick;
    let again = run.first_fire(back + 60 * TPS);
    assert_eq!(again, back + TPS + lay, "laid on again");
    // No lay: the lock held while the target was out of the gates fires the
    // moment it is back.
    let mut run = Run::new(t72(), 22, 2);
    run.gates = false;
    let track = run.first_phase(Phase::Track, 60 * TPS);
    run.run_until(track + 5 * TPS);
    run.gates = true;
    let back = run.tick;
    assert_eq!(run.first_fire(back + 60 * TPS), back);
}

#[test]
fn failing_gates_hold_the_lock_for_15_seconds() {
    let mut run = Run::new(sa6(1), 3, 0);
    run.gates = false;
    let track = run.first_phase(Phase::Track, 100 * TPS);
    run.run_until(track + 15 * TPS);
    assert_eq!(run.controller.phase(), Phase::Track);
    run.step();
    // The search finds the same target again at once and prepares anew.
    assert_eq!(run.controller.phase(), Phase::Prepare);
    assert_eq!(run.controller.deadline(), Some(track + 15 * TPS + 15 * TPS));
    assert!(run.fired.is_empty());
}

#[test]
fn a_lost_target_returns_to_search_and_idle_follows_after_30_seconds() {
    let mut run = Run::new(sa6(1), 3, 0);
    run.first_phase(Phase::Track, 100 * TPS);
    run.eligible.clear();
    run.in_range = false;
    run.step();
    assert_eq!(run.controller.phase(), Phase::Search);
    let lost = run.tick - 1;
    let idle = run.first_phase(Phase::Idle, lost + 40 * TPS);
    // The last tick with a hostile in range was the one before the loss.
    assert_eq!(idle, lost - 1 + 30 * TPS);
}

#[test]
fn zsu23_bursts_pauses_empties_and_swaps() {
    let mut run = Run::new(zsu23(), 2_000, 2);
    let fire = run.first_fire(60 * TPS);
    // 20 s unready preparation, 1 s lock.
    assert_eq!(fire, 21 * TPS);
    run.first_phase(Phase::Pause, 60 * TPS);
    let burst: u32 = run.fired.iter().map(|(_, f)| f.rounds).sum();
    assert_eq!(burst, 99);
    // Every round within the 1.75 s burst.
    assert!(run.fired.iter().all(|(t, _)| *t < fire + 210));
    // The next burst starts 1.75 + 1 s after the first.
    let next = run.first_fire(60 * TPS);
    assert_eq!(next, fire + 210 + 120);
    // Fire on until the magazine runs out: 2,000 rounds, then a 120 s swap.
    let reload = run.first_phase(Phase::Reload, 200 * TPS);
    let fired: u32 = run.fired.iter().map(|(_, f)| f.rounds).sum();
    assert_eq!(fired, 2_000);
    run.run_until(reload + 120 * TPS + 1);
    assert_eq!(run.swaps, vec![reload + 120 * TPS]);
    assert_eq!(run.loaded, 2_000);
    // Two magazines in reserve: after the third empties the gun falls silent.
    run.run_until(reload + 600 * TPS);
    assert_eq!(run.swaps.len(), 2);
    assert_eq!(run.controller.phase(), Phase::Empty);
    let total: u32 = run.fired.iter().map(|(_, f)| f.rounds).sum();
    assert_eq!(total, 6_000);
    // A truck in range (the resupply slice's flag) starts a swap again.
    let before = run.tick;
    let inputs = Inputs {
        tick: run.tick,
        hostile_in_range: true,
        eligible: &[7],
        gates: true,
        stock: Stock {
            loaded: 0,
            reserve: Reserve::Magazines(0),
            supply: true,
        },
        blind: false,
        slow: false,
        hold: false,
        lay: 0,
    };
    run.controller.advance(&run.profile, &inputs);
    assert_eq!(run.controller.phase(), Phase::Reload);
    assert_eq!(run.controller.deadline(), Some(before + 120 * TPS));
}

#[test]
fn ks19_opens_with_eight_shells() {
    // KS-19: one round, 16-quarter pause, 8 startup shots, 60 s swap; NPC
    // 40 / 100 / 60, retarget 40.
    let profile = Profile {
        timing: Timing::from_quarters(40, 100, 60, 4, 40, 1),
        arm: Arm::Gun {
            burst: 1,
            burst_ticks: QUARTER_SECOND_TICKS,
            pause: 16 * QUARTER_SECOND_TICKS,
            opening: 8,
            opening_gap: 0,
            swap: 60 * TPS,
        },
    };
    assert_eq!(profile.timing.retarget, Some(10 * TPS));
    let mut run = Run::new(profile, 60, 2);
    run.first_fire(60 * TPS);
    run.first_phase(Phase::Pause, 60 * TPS);
    assert_eq!(run.fired.len(), 8);
    assert!(run.fired.iter().all(|(_, f)| f.opening && f.rounds == 1));
    let spacing: Vec<u64> = run.fired.windows(2).map(|w| w[1].0 - w[0].0).collect();
    assert!(spacing.iter().all(|s| *s == QUARTER_SECOND_TICKS));
    // Then one shell every 4.25 s after the barrage's 2 s.
    let next = run.first_fire(100 * TPS);
    assert_eq!(next, run.fired[0].0 + 2 * TPS + 4 * TPS);
    assert_eq!(run.fired.len(), 9);
    assert!(!run.fired[8].1.opening);
}

#[test]
fn a_held_controller_keeps_its_lock_and_fires_the_moment_it_is_released() {
    // A flak team waiting for a group: the gun locks on and holds, past the
    // gates' 15 s window, and fires on the first tick the hold lifts.
    let profile = Profile {
        timing: Timing::from_quarters(40, 100, 60, 4, 40, 1),
        arm: Arm::Gun {
            burst: 1,
            burst_ticks: QUARTER_SECOND_TICKS,
            pause: 50 * QUARTER_SECOND_TICKS,
            opening: 8,
            opening_gap: 3 * QUARTER_SECOND_TICKS,
            swap: 60 * TPS,
        },
    };
    let mut run = Run::new(profile, 60, 2);
    run.hold = true;
    run.first_phase(Phase::Track, 60 * TPS);
    for _ in 0..(40 * TPS) {
        run.step();
    }
    assert_eq!(run.controller.phase(), Phase::Track);
    assert!(run.fired.is_empty());
    run.hold = false;
    let released = run.tick;
    let first = run.first_fire(released + TPS);
    assert_eq!(first, released, "fires on the tick the hold lifts");
    // Held again mid-barrage: the rest of the burst is not fired.
    run.hold = true;
    for _ in 0..(10 * TPS) {
        run.step();
    }
    assert_eq!(run.fired.len(), 1);
    assert_eq!(run.controller.phase(), Phase::Track);
}

#[test]
fn an_opening_gap_spaces_the_barrage() {
    // Flak since 2026-10-14: the eight opening shells 0.75 s apart (three
    // quarters), a third of the old rate like the rest of its fire.
    let profile = Profile {
        timing: Timing::from_quarters(40, 100, 60, 4, 40, 1),
        arm: Arm::Gun {
            burst: 1,
            burst_ticks: QUARTER_SECOND_TICKS,
            pause: 50 * QUARTER_SECOND_TICKS,
            opening: 8,
            opening_gap: 3 * QUARTER_SECOND_TICKS,
            swap: 60 * TPS,
        },
    };
    let mut run = Run::new(profile, 60, 2);
    run.first_fire(60 * TPS);
    run.first_phase(Phase::Pause, 60 * TPS);
    assert_eq!(run.fired.len(), 8);
    let spacing: Vec<u64> = run.fired.windows(2).map(|w| w[1].0 - w[0].0).collect();
    assert!(
        spacing.iter().all(|s| *s == 3 * QUARTER_SECOND_TICKS),
        "{spacing:?}"
    );
    // The barrage spans 6 s, then the 12.5 s pause.
    let next = run.first_fire(100 * TPS);
    assert_eq!(next, run.fired[0].0 + 6 * TPS + 25 * TPS / 2);
}

#[test]
fn flak_moves_to_the_nearest_target_each_retarget_period() {
    let profile = Profile {
        timing: Timing::from_quarters(40, 100, 60, 4, 40, 1),
        arm: Arm::Gun {
            burst: 1,
            burst_ticks: QUARTER_SECOND_TICKS,
            pause: 16 * QUARTER_SECOND_TICKS,
            opening: 0,
            opening_gap: 0,
            swap: 60 * TPS,
        },
    };
    let mut run = Run::new(profile, 60, 2);
    run.eligible = vec![7, 9];
    run.first_fire(60 * TPS);
    run.eligible = vec![9, 7];
    // The retarget period (10 s from the lock) is checked as each pause ends.
    run.run_until(run.tick + 8 * TPS);
    assert_eq!(run.controller.target(), Some(7));
    run.run_until(run.tick + 8 * TPS);
    assert_eq!(run.controller.target(), Some(9));
    // A SAM keeps its target while it stays eligible.
    let mut sam = Run::new(sa6(1), 3, 0);
    sam.eligible = vec![7, 9];
    sam.first_fire(100 * TPS);
    sam.eligible = vec![9, 7];
    sam.run_until(sam.tick + 30 * TPS);
    assert_eq!(sam.controller.target(), Some(7));
}

#[test]
fn a_blind_battery_stops_and_searches_when_its_radar_returns() {
    let mut run = Run::new(sa6(1), 3, 0);
    run.first_phase(Phase::Track, 100 * TPS);
    run.blind = true;
    run.step();
    assert_eq!(run.controller.phase(), Phase::Blind);
    assert_eq!(run.controller.target(), None);
    run.run_until(run.tick + 60 * TPS);
    assert!(run.fired.is_empty());
    run.blind = false;
    run.step();
    // Back to Search and straight on to the ordinary preparation.
    assert_eq!(run.controller.phase(), Phase::Prepare);
    assert_eq!(run.controller.deadline(), Some(run.tick - 1 + 15 * TPS));
}

#[test]
fn optical_backup_doubles_preparation() {
    let mut controller = Controller::new();
    let profile = sa6(1);
    let inputs = Inputs {
        tick: 0,
        hostile_in_range: true,
        eligible: &[7],
        gates: true,
        stock: stock(3, 0),
        blind: false,
        slow: true,
        hold: false,
        lay: 0,
    };
    controller.advance(&profile, &inputs);
    assert_eq!(controller.phase(), Phase::Prepare);
    assert_eq!(controller.deadline(), Some(72 * TPS));
}

#[test]
fn the_controller_round_trips_through_a_checkpoint() {
    let mut run = Run::new(zsu23(), 2_000, 2);
    run.first_fire(60 * TPS);
    run.step();
    let copy = round_trip(&run.controller, &Models::default()).expect("it round-trips");
    assert_eq!(copy, run.controller);
    // The copy goes on exactly as the original.
    let mut twin = Run::new(zsu23(), run.loaded, 2);
    twin.controller = copy;
    twin.tick = run.tick;
    for _ in 0..600 {
        run.step();
        twin.step();
    }
    assert_eq!(
        run.fired[run.fired.len() - 3..],
        twin.fired[twin.fired.len() - 3..]
    );
}
