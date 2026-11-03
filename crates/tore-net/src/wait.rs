//! Waiting for a real-time deadline, for a loop that drives a host on a fixed
//! clock: the dedicated server's main loop and the game's host thread (moved
//! here from `tore-server` in slice EF3, unchanged).
//!
//! Sleeping is coarse: a sleep can wake late (on Windows by about half a
//! millisecond, on macOS by far more), so the wait sleeps until a little
//! before the deadline and spins for the rest. The
//! host catches up any tick a late wake-up missed (agent decision; the margins
//! are fitted). Slice EF-X measured the wait on the CI runners: on Linux and
//! Windows it wakes for a 120 Hz tick a few microseconds late, at most 1.2 ms;
//! on macOS, whose timer coalescing lets a background process's sleep slip by
//! up to 75 ms, 1 to 8 ms late on average and up to 36 ms (docs/ARCHITECTURE.md,
//! "Sleep and wait accuracy on each system").

use std::time::Duration;

use crate::datagram::RealClock;

/// How long before a deadline the wait stops sleeping and spins. The
/// standard library sleeps on a high-resolution waitable timer on Windows 10
/// (1803) and later, so a short sleep there overshoots by about half a
/// millisecond (0.53 ms typical, 0.59 to 0.77 ms at the 99th percentile on
/// the Windows 11 CI machine, 2026-10-24) rather than the 15.6 ms of the
/// default timer resolution; the margin covers that. It was 2 ms, which kept
/// a hosting game or a server on Windows spinning about a fifth of a core
/// while a mission flew, for no earlier wake (agent decision, fitted; see
/// docs/ARCHITECTURE.md, "Sleep and wait accuracy on each system").
/// Elsewhere a sleep overshoots by a fraction of a millisecond.
#[cfg(windows)]
pub const SPIN_MARGIN: Duration = Duration::from_micros(750);
#[cfg(not(windows))]
pub const SPIN_MARGIN: Duration = Duration::from_micros(400);

/// The longest a host's loop goes without polling the host, its sockets and
/// its commands while a mission is flying and a connection is open (the
/// flying host's per-tick work is the tick, and its packets want a prompt
/// read).
pub const MAX_NAP: Duration = Duration::from_millis(4);

/// How long a host with no connection at all sleeps between polls when it is
/// not flying: a lobby join, keepalive or console command is read at most
/// this much later than before (it is [`MAX_NAP`] for a host in use).
pub const EMPTY_NAP: Duration = Duration::from_millis(20);

/// What a host loop is doing, which decides how it naps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pace {
    /// A mission is flying, so the host has tick deadlines.
    pub flying: bool,
    /// At least one connection is open.
    pub connections: bool,
}

/// When the loop wakes next, and how much of the wait it spends spinning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Wake {
    /// The time to wake at, on the loop's clock.
    pub at: Duration,
    /// The spin margin for [`wait_until`]: the margin for a real tick
    /// deadline, none for a housekeeping nap, which has no use for the
    /// precision.
    pub spin_margin: Duration,
}

impl Wake {
    /// Wakes no later than `due`, a housekeeping deadline (a status line, a
    /// log entry), which is never worth a spin; an earlier tick deadline is
    /// kept as it was.
    pub fn limit(&mut self, due: Duration) {
        if due < self.at {
            self.at = due;
            self.spin_margin = Duration::ZERO;
        }
    }
}

/// Plans a host loop's next wake from the host's own `next_wake` (a duration
/// from `now`): while flying, the next tick's deadline is spun to with
/// `spin_margin`, and naps cap at [`MAX_NAP`] when a connection is open;
/// when not flying, the loop honours `next_wake` (which is at most the host's
/// idle wake) and sleeps to it without a spin, or sleeps up to
/// [`EMPTY_NAP`] when no connection is open either. Tick pacing and
/// catch-up are the host's, not the wait's, and do not change.
pub fn plan_wake(now: Duration, next_wake: Duration, pace: Pace, spin_margin: Duration) -> Wake {
    if pace.flying {
        if !pace.connections || next_wake <= MAX_NAP {
            return Wake {
                at: now + next_wake,
                spin_margin,
            };
        }
        return Wake {
            at: now + MAX_NAP,
            spin_margin: Duration::ZERO,
        };
    }
    let nap = if pace.connections {
        next_wake
    } else {
        next_wake.max(EMPTY_NAP)
    };
    Wake {
        at: now + nap,
        spin_margin: Duration::ZERO,
    }
}

/// Time and sleeping, so tests can fake both.
pub trait Sleep {
    /// Time since the clock was made.
    fn now(&self) -> Duration;
    /// Sleeps at least `duration`, and possibly longer.
    fn sleep(&mut self, duration: Duration);
    /// Gives the processor a hint while spinning.
    fn spin(&mut self) {
        std::hint::spin_loop();
    }
}

impl Sleep for RealClock {
    fn now(&self) -> Duration {
        RealClock::now(self)
    }
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Waits until `deadline` on `clock`: sleeps until `spin_margin` before it,
/// then spins.
pub fn wait_until<S: Sleep + ?Sized>(clock: &mut S, deadline: Duration, spin_margin: Duration) {
    loop {
        let now = clock.now();
        let Some(remaining) = deadline.checked_sub(now).filter(|d| !d.is_zero()) else {
            return;
        };
        if remaining > spin_margin {
            clock.sleep(remaining - spin_margin);
        } else {
            clock.spin();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A clock whose sleeps take exactly as long as asked plus an overshoot,
    /// and whose spins take a microsecond each.
    #[derive(Default)]
    struct Fake {
        now: Duration,
        overshoot: Duration,
        sleeps: Vec<Duration>,
        spins: u32,
    }

    impl Sleep for Fake {
        fn now(&self) -> Duration {
            self.now
        }
        fn sleep(&mut self, duration: Duration) {
            self.sleeps.push(duration);
            self.now += duration + self.overshoot;
        }
        fn spin(&mut self) {
            self.spins += 1;
            self.now += Duration::from_micros(1);
        }
    }

    #[test]
    fn the_wait_sleeps_most_of_the_way_then_spins() {
        let mut clock = Fake::default();
        wait_until(
            &mut clock,
            Duration::from_millis(8),
            Duration::from_micros(400),
        );
        assert_eq!(clock.sleeps, [Duration::from_micros(7_600)]);
        assert_eq!(clock.spins, 400);
        assert_eq!(clock.now, Duration::from_millis(8));
    }

    #[test]
    fn a_late_sleep_does_not_loop_and_a_past_deadline_returns_at_once() {
        let mut clock = Fake {
            overshoot: Duration::from_millis(15),
            ..Default::default()
        };
        wait_until(
            &mut clock,
            Duration::from_millis(8),
            Duration::from_millis(2),
        );
        assert_eq!((clock.sleeps.len(), clock.spins), (1, 0));
        let before = clock.now;
        wait_until(
            &mut clock,
            Duration::from_millis(1),
            Duration::from_millis(2),
        );
        assert_eq!(clock.now, before);
    }

    const MARGIN: Duration = Duration::from_micros(400);

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn plan(next_wake: Duration, flying: bool, connections: bool) -> Wake {
        plan_wake(
            ms(100),
            next_wake,
            Pace {
                flying,
                connections,
            },
            MARGIN,
        )
    }

    #[test]
    fn a_tick_deadline_is_spun_to_and_a_nap_is_not() {
        // The tick is 3 ms away: that is the deadline, spin to it.
        assert_eq!(
            plan(ms(3), true, true),
            Wake {
                at: ms(103),
                spin_margin: MARGIN
            }
        );
        // The tick is 8 ms away: nap 4 ms with no spin tail.
        assert_eq!(
            plan(ms(8), true, true),
            Wake {
                at: ms(104),
                spin_margin: Duration::ZERO
            }
        );
        // Exactly MAX_NAP away is the deadline.
        assert_eq!(plan(MAX_NAP, true, true).spin_margin, MARGIN);
    }

    #[test]
    fn a_flying_host_with_no_connections_waits_for_its_tick() {
        // Nothing needs a 4 ms read, so it wakes for the tick alone.
        assert_eq!(
            plan(ms(8), true, false),
            Wake {
                at: ms(108),
                spin_margin: MARGIN
            }
        );
    }

    #[test]
    fn a_lobby_honours_the_hosts_wake_without_a_spin() {
        assert_eq!(
            plan(ms(10), false, true),
            Wake {
                at: ms(110),
                spin_margin: Duration::ZERO
            }
        );
        // An earlier host wake (a timer) is honoured too.
        assert_eq!(plan(ms(2), false, true).at, ms(102));
        assert_eq!(plan(ms(2), false, true).spin_margin, Duration::ZERO);
    }

    #[test]
    fn an_empty_idle_host_sleeps_longer() {
        assert_eq!(
            plan(ms(10), false, false),
            Wake {
                at: ms(120),
                spin_margin: Duration::ZERO
            }
        );
        // The empty nap is a floor only for the idle wake, never longer than
        // the host asks when it asks for more.
        assert_eq!(plan(ms(30), false, false).at, ms(130));
    }

    #[test]
    fn a_housekeeping_deadline_cuts_a_wake_short_without_a_spin() {
        let mut wake = plan(ms(8), true, true);
        wake.limit(ms(102));
        assert_eq!(
            wake,
            Wake {
                at: ms(102),
                spin_margin: Duration::ZERO
            }
        );
        // A later deadline leaves the tick deadline and its spin alone.
        let mut wake = plan(ms(3), true, true);
        wake.limit(ms(150));
        assert_eq!(
            wake,
            Wake {
                at: ms(103),
                spin_margin: MARGIN
            }
        );
    }

    #[test]
    fn a_nap_wait_never_spins_and_a_tick_wait_does() {
        let mut clock = Fake::default();
        let nap = plan_wake(
            clock.now,
            ms(10),
            Pace {
                flying: false,
                connections: true,
            },
            MARGIN,
        );
        wait_until(&mut clock, nap.at, nap.spin_margin);
        assert_eq!((clock.sleeps.len(), clock.spins), (1, 0));
        let tick = plan_wake(
            clock.now,
            ms(3),
            Pace {
                flying: true,
                connections: true,
            },
            MARGIN,
        );
        wait_until(&mut clock, tick.at, tick.spin_margin);
        assert_eq!((clock.sleeps.len(), clock.spins), (2, 400));
    }

    #[test]
    fn the_real_clock_waits_at_least_until_the_deadline() {
        let mut clock = RealClock::new();
        let deadline = clock.now() + Duration::from_millis(3);
        wait_until(&mut clock, deadline, SPIN_MARGIN);
        assert!(clock.now() >= deadline);
    }
}
