//! Which RWR warning tone the player hears, from authoritative combat and AI
//! state. Read-only and presentation only: nothing here feeds back into the
//! simulation. Rules and numbers: docs/spec/rwr.md#warning-tones.
use crate::situation::AIM120_IGNORE_FT;
use tore_sim::ai::weapon_service::quarter_clock;
use tore_world::readout::CockpitReadout;

/// A warning tone, highest priority first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tone {
    /// A radar-guided missile in flight at the player.
    RadarInbound,
    /// An infrared-guided missile in flight at the player.
    InfraredInbound,
    /// An enemy holds a radar-guided missile lock on the player.
    RadarLock,
    /// An enemy holds an infrared-guided missile lock on the player.
    InfraredLock,
}
impl Tone {
    pub fn recording(self) -> &'static str {
        match self {
            Tone::RadarInbound | Tone::InfraredInbound => "&RWRLOCK.5K",
            Tone::RadarLock => "&RWRDTCT.5K",
            Tone::InfraredLock => "&RWRIR.5K",
        }
    }
}

/// A lock warning lasts four quarter-second clock steps past its last refresh.
const LOCK_HOLD_QUARTERS: u64 = 4;

/// The lock warnings' memory between ticks.
#[derive(Clone, Debug, Default)]
pub struct Warnings {
    /// Quarter-clock count of the latest radar and infrared lock refresh.
    radar_lock: Option<u64>,
    infrared_lock: Option<u64>,
}

impl Warnings {
    /// One fixed step. `inbound` says whether a radar-guided and an
    /// infrared-guided missile are in flight at the player ([`inbound`]);
    /// `locks` are the seeker classes of the missile locks enemies hold on
    /// the player this tick (2 infrared, 3 radar). `gone` while the player's
    /// aircraft is out of the fight: ejected or destroyed.
    pub fn step(
        &mut self,
        tick: u64,
        inbound: [bool; 2],
        locks: &[u8],
        gone: bool,
    ) -> Option<Tone> {
        let quarter = quarter_clock(tick);
        for class in locks {
            match class {
                3 => self.radar_lock = Some(quarter),
                2 => self.infrared_lock = Some(quarter),
                _ => {}
            }
        }
        let held = |at: Option<u64>| at.is_some_and(|at| quarter < at + LOCK_HOLD_QUARTERS);
        if gone {
            None
        } else if inbound[0] {
            Some(Tone::RadarInbound)
        } else if inbound[1] {
            Some(Tone::InfraredInbound)
        } else if held(self.radar_lock) {
            Some(Tone::RadarLock)
        } else if held(self.infrared_lock) {
            Some(Tone::InfraredLock)
        } else {
            None
        }
    }
}

/// Whether a radar-guided (class 3) and an infrared-guided (class 2) missile
/// is in flight with the readout's plane as its target. An AIM-120 farther
/// than 30,380 ft from `player`, the plane's position, is not counted, as for
/// the flight music.
pub fn inbound(readout: &CockpitReadout, player: [f64; 3]) -> [bool; 2] {
    [3, 2].map(|class| {
        readout.rwr.inbound.iter().any(|missile| {
            missile.seeker_class == class
                && !(missile.aim120 && distance(missile.position, player) > AIM120_IGNORE_FT)
        })
    })
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clearing_inert_inbound_cues_stops_the_launch_warning_immediately() {
        for (inbound, tone) in [
            ([true, false], Tone::RadarInbound),
            ([false, true], Tone::InfraredInbound),
        ] {
            let mut warnings = Warnings::default();
            assert_eq!(warnings.step(100, inbound, &[], false), Some(tone));
            assert_eq!(warnings.step(101, [false; 2], &[], false), None);
        }
    }
    #[test]
    fn lock_warnings_last_four_quarter_steps_past_the_last_refresh() {
        let mut w = Warnings::default();
        let none = [false; 2];
        // Refreshed at tick 30 (quarter 1): heard through quarter 4.
        assert_eq!(w.step(30, none, &[3], false), Some(Tone::RadarLock));
        assert_eq!(w.step(149, none, &[], false), Some(Tone::RadarLock));
        assert_eq!(w.step(150, none, &[], false), None);
        // Radar outranks infrared; infrared alone plays its own recording.
        assert_eq!(w.step(200, none, &[2, 3], false), Some(Tone::RadarLock));
        let mut w = Warnings::default();
        assert_eq!(w.step(200, none, &[2], false), Some(Tone::InfraredLock));
        assert_eq!(Tone::InfraredLock.recording(), "&RWRIR.5K");
    }
    #[test]
    fn missiles_in_flight_outrank_locks_and_nothing_sounds_once_gone() {
        let mut w = Warnings::default();
        assert_eq!(
            w.step(0, [false, true], &[3], false),
            Some(Tone::InfraredInbound)
        );
        assert_eq!(
            w.step(1, [true, true], &[], false),
            Some(Tone::RadarInbound)
        );
        assert_eq!(
            Tone::InfraredInbound.recording(),
            Tone::RadarInbound.recording()
        );
        assert_eq!(w.step(2, [true, false], &[3], true), None);
    }
}
