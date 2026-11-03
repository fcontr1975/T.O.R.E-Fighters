//! When the window draws its next frame: the Graphics screen's frame rate
//! choice, and the throttles for a paused, unfocused or hidden window.
//!
//! Presentation only. The simulation is not paced here: every flight frame
//! still turns the wall time since the last frame into 120 Hz ticks (clamped
//! to 0.25 s, every owed tick run), and a networked flight's ticks are the
//! session's. Drawing fewer frames changes how often the picture is shown,
//! never what the simulation computes.
//!
//! - **V-sync** (the default, John 2026-10-19) presents with `Fifo`: the
//!   display paces the frames, one per refresh, and the next flight frame is
//!   asked for as soon as one is presented. `Fifo` is used rather than
//!   `Mailbox` plus a timer at the monitor's refresh because a timer is not
//!   locked to the display: its small drift against the refresh shows as a
//!   doubled or dropped frame every so often, which is the uneven pacing
//!   V-sync is chosen to avoid (agent decision, 2026-10-19).
//! - **60, 100, 200** present without waiting for the display (`Mailbox`, so
//!   nothing tears, else `Immediate`, else `Fifo`) and a timer spaces the
//!   frames: the event loop sleeps until each frame's time (`WaitUntil`),
//!   never spinning.
//! - `TORE_FRAME_LIMIT` overrides the saved choice for one run: `vsync`,
//!   `60`, `100`, `200`, or `off` for uncapped benchmark rendering
//!   (`Immediate`, else `Mailbox`, else `Fifo`, with no unfocused cap), which
//!   the menu does not offer. Scripted runs, performance measurements,
//!   captures and smoke tests draw uncapped unless the variable is set.
//!
//! - **The frame-rate counter** (Graphics > Display, or `TORE_SHOW_FPS=1`)
//!   measures real frames, so while it is shown every screen draws
//!   continuously at the chosen rate, menus, the lobby, loading screens and a
//!   paused flight included (John, 2026-10-19). Hidden, the low-power pacing
//!   below applies unchanged. See [`counted`] and [`too_soon`].
//!
//! Throttles that need no choice (agent rules from the 2026-10-19
//! optimization plan): a stopped single-player flight with a still camera
//! draws only when something happens (input, a menu animation, a notice
//! expiring); an unfocused window draws about ten frames a second; a hidden
//! window (occluded, or minimised to no size) draws nothing. Scripted runs,
//! performance measurements, captures and smoke tests are never throttled.
use std::time::{Duration, Instant};

use crate::graphics::FrameLimit;

/// Whether a `TORE_FRAME_LIMIT` value names a limit.
fn named(text: &str) -> bool {
    let text = text.trim();
    text == "off" || FrameLimit::parse(text).is_some()
}

/// The longest gap between frames of an unfocused window: about 10 a second.
pub const UNFOCUSED: Duration = Duration::from_millis(100);
/// A menu that animates redraws at about 60 a second, as it always has.
pub const MENU: Duration = Duration::from_millis(16);

/// The frame rate in effect: the saved choice, or the run's override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    VSync,
    /// Frames a second, spaced by a timer.
    Rate(u32),
    /// Uncapped, for benchmarks: `TORE_FRAME_LIMIT=off`.
    Off,
}

impl Limit {
    /// The limit for this run from the saved `choice` and the
    /// `TORE_FRAME_LIMIT` text `variable`: the limit the variable names, or
    /// the choice. An `automated` run (an input script, a performance
    /// measurement, a capture or a smoke test) draws uncapped unless the
    /// variable names a limit: those runs keep the presentation they were
    /// written against, and agent windows on a hidden workspace get no
    /// compositor frame callbacks, which V-sync would wait for. An unknown
    /// value is ignored.
    pub fn with(choice: FrameLimit, automated: bool, variable: Option<&str>) -> Self {
        let named = variable.map(str::trim).and_then(|text| match text {
            "off" => Some(Self::Off),
            _ => FrameLimit::parse(text).map(Self::of),
        });
        match named {
            Some(limit) => limit,
            None if automated => Self::Off,
            None => Self::of(choice),
        }
    }
    /// [`Limit::with`] the environment's `TORE_FRAME_LIMIT`.
    pub fn for_run(choice: FrameLimit, automated: bool) -> Self {
        let variable = std::env::var("TORE_FRAME_LIMIT").ok();
        if let Some(text) = &variable
            && !named(text)
        {
            log::warn!("TORE_FRAME_LIMIT={text} ignored: use off, vsync, 60, 100 or 200");
        }
        Self::with(choice, automated, variable.as_deref())
    }
    /// Whether `TORE_FRAME_LIMIT` replaces the saved choice for this run.
    pub fn overridden() -> bool {
        std::env::var("TORE_FRAME_LIMIT").is_ok_and(|text| named(&text))
    }
    fn of(choice: FrameLimit) -> Self {
        choice.rate().map_or(Self::VSync, Self::Rate)
    }
    /// The surface's present mode from those the adapter offers. `Fifo` is
    /// always offered.
    pub fn present_mode(self, modes: &[wgpu::PresentMode]) -> wgpu::PresentMode {
        use wgpu::PresentMode::{Fifo, Immediate, Mailbox};
        let order: &[wgpu::PresentMode] = match self {
            Self::VSync => &[Fifo],
            Self::Rate(_) => &[Mailbox, Immediate],
            Self::Off => &[Immediate, Mailbox],
        };
        order
            .iter()
            .copied()
            .find(|mode| modes.contains(mode))
            .unwrap_or(Fifo)
    }
    /// The timer's spacing between frames; None when presentation paces.
    pub fn period(self) -> Option<Duration> {
        match self {
            Self::Rate(n) => Some(Duration::from_secs_f64(1. / f64::from(n.max(1)))),
            Self::VSync | Self::Off => None,
        }
    }
}

/// The wait before drawing again after a frame whose surface gave no image
/// (lost, outdated or timed out), for the `failures`-th such frame in a row:
/// 16 ms, doubling to at most a quarter of a second. Such a frame never asks
/// for its own redraw, so a surface that keeps failing cannot spin the loop.
pub fn surface_retry(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(4);
    Duration::from_millis(16 << doublings)
}

/// When to try drawing again after a frame whose surface gave no image: the
/// `soonest` the frame rate and the window allow (None while it is hidden,
/// which waits for it to show), but not before `retry` from `now`.
pub fn after_surface_failure(
    soonest: Option<Instant>,
    retry: Duration,
    now: Instant,
) -> Option<Instant> {
    soonest.map(|at| at.max(now + retry))
}

/// What the screen wants after the frame it just drew.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// Nothing moves: draw again only when an event asks.
    Nothing,
    /// Nothing moves until this time (a notice expiring), or an event.
    At(Instant),
    /// A menu animation: again after [`MENU`].
    Menu,
    /// A live view (flight, the viewer, a replay): as often as the frame rate
    /// allows.
    Live,
}

/// What a screen wants after its frame once the frame-rate counter is
/// considered: with the counter shown every screen is a live view, drawn as
/// often as the frame rate allows, so the counter measures real continuous
/// frames; hidden, the screen's own want stands. A hidden window still draws
/// nothing (see [`Pacer::next`]).
pub fn counted(want: Want, counter: bool) -> Want {
    if counter { Want::Live } else { want }
}

/// Whether a redraw request that arrives at `now` should wait for the timer
/// (`next_frame`) instead of drawing at once. With the counter shown and a
/// capped rate, an event (a moving mouse, a key) must not add frames to the
/// ones the timer draws, or the counter would read above the cap. V-sync is
/// paced by presentation and the uncapped limit has no cap to keep. An
/// unfocused window keeps its ten frames a second the same way: the pointer
/// passing over it, or a menu redrawn for news, waits for the frame already
/// due (at most [`UNFOCUSED`] away) rather than drawing extra ones; this needs
/// no counter and holds under V-sync too. Automated runs are never held.
pub fn too_soon(
    limit: Limit,
    counter: bool,
    window: Window,
    next_frame: Option<Instant>,
    now: Instant,
) -> bool {
    let capped = counter && limit.period().is_some();
    let away = !window.focused && limit != Limit::Off;
    !window.automated && (capped || away) && next_frame.is_some_and(|at| now < at)
}

/// The window's state for pacing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub focused: bool,
    /// Occluded, or minimised to no size.
    pub hidden: bool,
    /// A scripted or measured run, a capture or a smoke test: never throttled.
    pub automated: bool,
}

/// The frame timer for the capped rates.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pacer {
    /// When the last timed frame was due.
    due: Option<Instant>,
    /// When the last frame finished.
    last: Option<Instant>,
    /// What [`Pacer::next`] chose after it: a time, or None to wait for an
    /// event.
    chosen: Option<Instant>,
}

impl Pacer {
    /// When to draw the next frame after one finished at `now`; None waits
    /// for an event (input, a resize, the window showing again).
    pub fn next(
        &mut self,
        limit: Limit,
        want: Want,
        window: Window,
        now: Instant,
    ) -> Option<Instant> {
        self.last = Some(now);
        self.chosen = None;
        if window.hidden && !window.automated {
            self.due = None;
            return None;
        }
        let at = match want {
            Want::Nothing => {
                self.due = None;
                return None;
            }
            Want::At(at) => {
                self.due = None;
                at
            }
            Want::Menu => {
                self.due = None;
                now + MENU
            }
            Want::Live => match limit.period() {
                None => now,
                Some(period) => {
                    // Evenly spaced from the last due time; a late frame
                    // starts a new count rather than rushing to catch up.
                    let at = self
                        .due
                        .map(|due| due + period)
                        .filter(|at| *at >= now)
                        .unwrap_or(now);
                    self.due = Some(at);
                    at
                }
            },
        };
        let at = throttle(limit, window, at, now);
        self.chosen = Some(at);
        Some(at)
    }
    /// How much of the time from `since` (when the last frame started) to
    /// `now` the frame did not wait for by choice: the gap less the wait the
    /// pacer asked for after it (until its timer, a throttle's slot, or, for
    /// a view waiting on events, all of it). A slow frame or a loop that wakes
    /// late shows here; an unfocused window's tenth of a second, a capped
    /// rate's slot or a still picture's wait does not.
    pub fn busy(&self, since: Instant, now: Instant) -> Duration {
        let gap = now.saturating_duration_since(since);
        let Some(last) = self.last.filter(|last| *last >= since) else {
            return gap;
        };
        let until = self.chosen.map_or(now, |at| at.min(now));
        gap.saturating_sub(until.saturating_duration_since(last))
    }
    /// The soonest an event may have a waiting view drawn again at `now`:
    /// at once, or when the frame rate or the unfocused rate next allows.
    /// None while the window is hidden.
    pub fn soonest(&self, limit: Limit, window: Window, now: Instant) -> Option<Instant> {
        if window.hidden && !window.automated {
            return None;
        }
        let at = limit
            .period()
            .zip(self.due)
            .map(|(period, due)| due + period)
            .filter(|at| *at > now)
            .unwrap_or(now);
        let last = self.last.unwrap_or(now);
        Some(throttle(limit, window, at, last))
    }
}

/// `at`, or later for an unfocused window: [`UNFOCUSED`] after `last`.
fn throttle(limit: Limit, window: Window, at: Instant, last: Instant) -> Instant {
    if !window.focused && !window.automated && limit != Limit::Off {
        at.max(last + UNFOCUSED)
    } else {
        at
    }
}

/// What a stopped flight's picture depends on besides events: the camera's
/// pose and lens. A change between frames means the view is still moving
/// (look-around easing, a view change settling), so it keeps drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub position: [f64; 3],
    pub angles: [f32; 3],
    pub view_fraction: f32,
    pub zoom: f32,
    pub view: u8,
}

impl Pose {
    pub fn of(camera: &crate::camera::Camera, view: u8) -> Self {
        Self {
            position: camera.position,
            angles: [camera.yaw, camera.pitch, camera.roll],
            view_fraction: camera.view_fraction,
            zoom: camera.zoom,
            view,
        }
    }
}

/// Whether a flight frame may wait for events: the flight is stopped (single
/// player, paused or under a menu) and nothing on screen moves by itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stillness {
    last: Option<Pose>,
}

impl Stillness {
    /// Record this frame's pose; true when the frame showed a still picture:
    /// `stopped`, the pose as last frame's, and no `moving` cause (held look
    /// keys, a head tracker, an animating screen over the flight).
    pub fn still(&mut self, stopped: bool, pose: Pose, moving: bool) -> bool {
        let same = self.last == Some(pose);
        self.last = Some(pose);
        stopped && same && !moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::PresentMode::{AutoNoVsync, AutoVsync, Fifo, FifoRelaxed, Immediate, Mailbox};

    const FOCUSED: Window = Window {
        focused: true,
        hidden: false,
        automated: false,
    };
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn the_choice_and_the_variable_pick_the_limit() {
        let with = |choice, variable| Limit::with(choice, false, variable);
        assert_eq!(with(FrameLimit::VSync, None), Limit::VSync);
        assert_eq!(with(FrameLimit::Fps60, None), Limit::Rate(60));
        assert_eq!(with(FrameLimit::Fps100, None), Limit::Rate(100));
        assert_eq!(with(FrameLimit::Fps200, None), Limit::Rate(200));
        assert_eq!(with(FrameLimit::VSync, Some("off")), Limit::Off);
        assert_eq!(with(FrameLimit::VSync, Some(" 60\n")), Limit::Rate(60));
        assert_eq!(with(FrameLimit::Fps200, Some("vsync")), Limit::VSync);
        assert_eq!(with(FrameLimit::Fps100, Some("fast")), Limit::Rate(100));
        assert_eq!(with(FrameLimit::VSync, Some("")), Limit::VSync);
        // Automated runs are uncapped unless the variable names a limit.
        let run = |choice, variable| Limit::with(choice, true, variable);
        assert_eq!(run(FrameLimit::VSync, None), Limit::Off);
        assert_eq!(run(FrameLimit::Fps60, None), Limit::Off);
        assert_eq!(run(FrameLimit::Fps60, Some("fast")), Limit::Off);
        assert_eq!(run(FrameLimit::Fps60, Some("vsync")), Limit::VSync);
        assert_eq!(run(FrameLimit::VSync, Some("100")), Limit::Rate(100));
        assert!(named("off") && named("vsync") && named("200"));
        assert!(!named("144") && !named("unlimited") && !named(""));
    }

    #[test]
    fn each_limit_takes_its_present_mode() {
        let all = [
            Fifo,
            FifoRelaxed,
            Immediate,
            Mailbox,
            AutoVsync,
            AutoNoVsync,
        ];
        assert_eq!(Limit::VSync.present_mode(&all), Fifo);
        for rate in [60, 100, 200] {
            assert_eq!(Limit::Rate(rate).present_mode(&all), Mailbox);
            assert_eq!(
                Limit::Rate(rate).present_mode(&[Fifo, Immediate]),
                Immediate
            );
            assert_eq!(Limit::Rate(rate).present_mode(&[Fifo]), Fifo);
        }
        assert_eq!(Limit::Off.present_mode(&all), Immediate);
        assert_eq!(Limit::Off.present_mode(&[Fifo, Mailbox]), Mailbox);
        assert_eq!(Limit::Off.present_mode(&[Fifo]), Fifo);
        assert_eq!(Limit::VSync.present_mode(&[]), Fifo);
        assert_eq!(Limit::VSync.period(), None);
        assert_eq!(Limit::Off.period(), None);
        assert_eq!(Limit::Rate(100).period(), Some(ms(10)));
        assert_eq!(Limit::Rate(200).period(), Some(ms(5)));
    }

    #[test]
    fn vsync_and_off_ask_for_the_next_live_frame_at_once() {
        let now = Instant::now();
        for limit in [Limit::VSync, Limit::Off] {
            let mut pacer = Pacer::default();
            assert_eq!(pacer.next(limit, Want::Live, FOCUSED, now), Some(now));
        }
    }

    #[test]
    fn a_rate_spaces_frames_evenly_without_catching_up() {
        let start = Instant::now();
        let period = Duration::from_secs_f64(1. / 60.);
        let mut pacer = Pacer::default();
        let limit = Limit::Rate(60);
        // The first frame after a pause in timing goes at once.
        assert_eq!(pacer.next(limit, Want::Live, FOCUSED, start), Some(start));
        // Frames that finish early wait for their slot, which stays on the
        // grid however long each frame took.
        let mut due = start;
        for work in [2, 9, 15, 1] {
            let finished = due + ms(work);
            let next = pacer.next(limit, Want::Live, FOCUSED, finished).unwrap();
            assert_eq!(next, due + period);
            due = next;
        }
        // A frame later than its slot goes at once and restarts the grid.
        let late = due + period + ms(7);
        assert_eq!(pacer.next(limit, Want::Live, FOCUSED, late), Some(late));
        assert_eq!(
            pacer.next(limit, Want::Live, FOCUSED, late + ms(3)),
            Some(late + period)
        );
        // Sixty frames over a simulated second with light work: one second.
        let mut pacer = Pacer::default();
        let mut at = pacer.next(limit, Want::Live, FOCUSED, start).unwrap();
        for _ in 0..60 {
            at = pacer.next(limit, Want::Live, FOCUSED, at + ms(3)).unwrap();
        }
        let second = (at - start).as_secs_f64();
        assert!((second - 1.).abs() < 1e-6, "{second}");
    }

    #[test]
    fn menus_still_views_and_timers() {
        let now = Instant::now();
        let mut pacer = Pacer::default();
        for limit in [Limit::VSync, Limit::Rate(200), Limit::Off] {
            assert_eq!(
                pacer.next(limit, Want::Menu, FOCUSED, now),
                Some(now + MENU)
            );
            assert_eq!(pacer.next(limit, Want::Nothing, FOCUSED, now), None);
            let expiry = now + ms(1800);
            assert_eq!(
                pacer.next(limit, Want::At(expiry), FOCUSED, now),
                Some(expiry)
            );
        }
    }

    #[test]
    fn unfocused_draws_about_ten_a_second_unless_uncapped_or_automated() {
        let now = Instant::now();
        let away = Window {
            focused: false,
            ..FOCUSED
        };
        for limit in [Limit::VSync, Limit::Rate(60), Limit::Rate(200)] {
            let mut pacer = Pacer::default();
            assert_eq!(
                pacer.next(limit, Want::Live, away, now),
                Some(now + UNFOCUSED)
            );
            assert_eq!(
                pacer.next(limit, Want::Menu, away, now),
                Some(now + UNFOCUSED)
            );
            // A later timer keeps its own time.
            let later = now + ms(900);
            assert_eq!(pacer.next(limit, Want::At(later), away, now), Some(later));
            assert_eq!(pacer.next(limit, Want::Nothing, away, now), None);
        }
        let mut pacer = Pacer::default();
        assert_eq!(pacer.next(Limit::Off, Want::Live, away, now), Some(now));
        let scripted = Window {
            automated: true,
            ..away
        };
        assert_eq!(
            pacer.next(Limit::VSync, Want::Live, scripted, now),
            Some(now)
        );
    }

    #[test]
    fn a_hidden_window_draws_nothing_until_shown() {
        let now = Instant::now();
        let hidden = Window {
            hidden: true,
            ..FOCUSED
        };
        for limit in [Limit::VSync, Limit::Rate(60), Limit::Off] {
            let mut pacer = Pacer::default();
            for want in [Want::Live, Want::Menu, Want::At(now + ms(5)), Want::Nothing] {
                assert_eq!(pacer.next(limit, want, hidden, now), None);
            }
            // Shown again, the timed grid starts afresh.
            assert_eq!(
                pacer.next(limit, Want::Live, FOCUSED, now + ms(500)),
                Some(now + ms(500))
            );
        }
        let automated = Window {
            automated: true,
            ..hidden
        };
        assert_eq!(
            Pacer::default().next(Limit::VSync, Want::Live, automated, now),
            Some(now)
        );
    }

    #[test]
    fn events_wake_a_waiting_view_within_the_limits() {
        let now = Instant::now();
        let away = Window {
            focused: false,
            ..FOCUSED
        };
        let hidden = Window {
            hidden: true,
            ..FOCUSED
        };
        // Nothing drawn yet: at once.
        let pacer = Pacer::default();
        assert_eq!(pacer.soonest(Limit::Rate(60), FOCUSED, now), Some(now));
        assert_eq!(pacer.soonest(Limit::VSync, hidden, now), None);
        // A paused view drew at `now` and waits; an event 2 ms later may
        // draw at the next 100 Hz slot, at once with V-sync (the display
        // paces it), and 100 ms after that frame when unfocused.
        let mut pacer = Pacer::default();
        assert_eq!(
            pacer.next(Limit::Rate(100), Want::Live, FOCUSED, now),
            Some(now)
        );
        assert_eq!(
            pacer.next(Limit::Rate(100), Want::Nothing, FOCUSED, now),
            None
        );
        let mut pacer = Pacer::default();
        pacer.next(Limit::Rate(100), Want::Live, FOCUSED, now);
        let event = now + ms(2);
        assert_eq!(
            pacer.soonest(Limit::Rate(100), FOCUSED, event),
            Some(now + ms(10))
        );
        assert_eq!(
            pacer.soonest(Limit::Rate(100), FOCUSED, now + ms(30)),
            Some(now + ms(30))
        );
        assert_eq!(pacer.soonest(Limit::VSync, FOCUSED, event), Some(event));
        assert_eq!(
            pacer.soonest(Limit::VSync, away, event),
            Some(now + UNFOCUSED)
        );
        assert_eq!(pacer.soonest(Limit::Off, away, event), Some(event));
        assert_eq!(pacer.soonest(Limit::VSync, hidden, event), None);
    }

    #[test]
    fn the_counter_makes_every_screen_a_live_view_at_the_frame_rate() {
        let now = Instant::now();
        let wants = [
            Want::Nothing,
            Want::At(now + ms(900)),
            Want::Menu,
            Want::Live,
        ];
        for limit in [
            Limit::VSync,
            Limit::Rate(60),
            Limit::Rate(100),
            Limit::Rate(200),
        ] {
            // Shown: an idle menu (Nothing) and every other want draw again
            // as often as the limit allows, evenly on the timer grid.
            let mut pacer = Pacer::default();
            let mut at = now;
            for want in wants {
                let first = pacer.next(limit, counted(want, true), FOCUSED, at);
                assert!(first.is_some(), "{limit:?} {want:?}");
            }
            if let Some(period) = limit.period() {
                let mut pacer = Pacer::default();
                at = pacer
                    .next(limit, counted(Want::Nothing, true), FOCUSED, now)
                    .unwrap();
                for _ in 0..10 {
                    let next = pacer
                        .next(limit, counted(Want::Nothing, true), FOCUSED, at + ms(1))
                        .unwrap();
                    assert_eq!(next, at + period, "{limit:?}");
                    at = next;
                }
            } else {
                assert_eq!(
                    Pacer::default().next(limit, counted(Want::Nothing, true), FOCUSED, now),
                    Some(now)
                );
            }
            // Hidden: the screen keeps its own low-power want.
            let mut pacer = Pacer::default();
            assert_eq!(counted(Want::Nothing, false), Want::Nothing);
            assert_eq!(counted(Want::Menu, false), Want::Menu);
            assert_eq!(counted(Want::At(now), false), Want::At(now));
            assert_eq!(
                pacer.next(limit, counted(Want::Nothing, false), FOCUSED, now),
                None
            );
            assert_eq!(
                pacer.next(limit, counted(Want::Menu, false), FOCUSED, now),
                Some(now + MENU)
            );
        }
        // A counted screen over an unfocused or hidden window still obeys
        // those throttles.
        let away = Window {
            focused: false,
            ..FOCUSED
        };
        let hidden = Window {
            hidden: true,
            ..FOCUSED
        };
        let mut pacer = Pacer::default();
        assert_eq!(
            pacer.next(Limit::VSync, counted(Want::Nothing, true), away, now),
            Some(now + UNFOCUSED)
        );
        assert_eq!(
            pacer.next(Limit::VSync, counted(Want::Nothing, true), hidden, now),
            None
        );
    }

    #[test]
    fn a_redraw_request_waits_for_the_timer_under_a_cap_with_the_counter_or_unfocused() {
        let now = Instant::now();
        let soon = Some(now + ms(5));
        let automated = Window {
            automated: true,
            ..FOCUSED
        };
        for rate in [60, 100, 200] {
            let limit = Limit::Rate(rate);
            assert!(too_soon(limit, true, FOCUSED, soon, now));
            // The slot has come, or nothing is scheduled: draw.
            assert!(!too_soon(limit, true, FOCUSED, soon, now + ms(5)));
            assert!(!too_soon(limit, true, FOCUSED, None, now));
            // Hidden counter: requests are honoured as before.
            assert!(!too_soon(limit, false, FOCUSED, soon, now));
            assert!(!too_soon(limit, true, automated, soon, now));
        }
        for limit in [Limit::VSync, Limit::Off] {
            assert!(!too_soon(limit, true, FOCUSED, soon, now));
        }
        // Unfocused, any limit but uncapped holds a request for the frame
        // already due, counter or not; nothing due draws at once.
        let away = Window {
            focused: false,
            ..FOCUSED
        };
        for limit in [Limit::VSync, Limit::Rate(60), Limit::Rate(200)] {
            for counter in [false, true] {
                assert!(too_soon(limit, counter, away, soon, now));
                assert!(!too_soon(limit, counter, away, None, now));
                assert!(!too_soon(limit, counter, away, soon, now + ms(5)));
            }
        }
        assert!(!too_soon(Limit::Off, false, away, soon, now));
        let scripted = Window {
            automated: true,
            ..away
        };
        assert!(!too_soon(Limit::VSync, false, scripted, soon, now));
    }

    #[test]
    fn a_failing_surface_is_retried_with_a_growing_wait() {
        let waits: Vec<_> = (1..=7).map(surface_retry).collect();
        assert_eq!(waits, [16, 32, 64, 128, 256, 256, 256].map(ms));
        let now = Instant::now();
        // At the frame rate's slot, or the retry wait if that is later.
        assert_eq!(
            after_surface_failure(Some(now), ms(16), now),
            Some(now + ms(16))
        );
        assert_eq!(
            after_surface_failure(Some(now + UNFOCUSED), ms(16), now),
            Some(now + UNFOCUSED)
        );
        // Hidden: wait until the window shows.
        assert_eq!(after_surface_failure(None, ms(16), now), None);
    }

    #[test]
    fn only_time_the_pacer_did_not_ask_for_counts_as_busy() {
        let start = Instant::now();
        let away = Window {
            focused: false,
            ..FOCUSED
        };
        // An unfocused flight frame takes 6 ms and waits the throttle's
        // 100 ms: the next frame, woken 1 ms late, was busy for 7 ms only.
        let mut pacer = Pacer::default();
        let finished = start + ms(6);
        let at = pacer
            .next(Limit::VSync, Want::Live, away, finished)
            .unwrap();
        assert_eq!(at, finished + UNFOCUSED);
        assert_eq!(pacer.busy(start, at + ms(1)), ms(7));
        // A 150 ms frame with V-sync asks for the next at once: all busy.
        let mut pacer = Pacer::default();
        let finished = start + ms(150);
        pacer.next(Limit::VSync, Want::Live, FOCUSED, finished);
        assert_eq!(pacer.busy(start, finished + ms(2)), ms(152));
        // A capped rate's slot is a chosen wait.
        let mut pacer = Pacer::default();
        pacer.next(Limit::Rate(60), Want::Live, FOCUSED, start);
        let finished = start + ms(4);
        let at = pacer
            .next(Limit::Rate(60), Want::Live, FOCUSED, finished)
            .unwrap();
        assert_eq!(pacer.busy(start, at), ms(4));
        // A still picture waits for events: the whole wait is chosen, and an
        // event that comes before a timed slot ends it early.
        let mut pacer = Pacer::default();
        pacer.next(Limit::VSync, Want::Nothing, FOCUSED, start + ms(5));
        assert_eq!(pacer.busy(start, start + ms(5_000)), ms(5));
        pacer.next(
            Limit::Rate(60),
            Want::At(start + ms(900)),
            FOCUSED,
            start + ms(5),
        );
        assert_eq!(pacer.busy(start, start + ms(300)), ms(5));
        // A hidden window waits for events too.
        let hidden = Window {
            hidden: true,
            ..FOCUSED
        };
        pacer.next(Limit::VSync, Want::Live, hidden, start + ms(5));
        assert_eq!(pacer.busy(start, start + ms(2_000)), ms(5));
        // No frame paced since `since` (or none at all): the whole gap.
        assert_eq!(Pacer::default().busy(start, start + ms(120)), ms(120));
        assert_eq!(pacer.busy(start + ms(6), start + ms(120)), ms(114));
    }

    #[test]
    fn a_stopped_flight_is_still_once_its_camera_settles() {
        let mut still = Stillness::default();
        let pose = Pose {
            position: [1., 2., 3.],
            zoom: 1.,
            ..Pose::default()
        };
        // The first frame has nothing to compare with.
        assert!(!still.still(true, pose, false));
        assert!(still.still(true, pose, false));
        // Running, the flight is never still.
        assert!(!still.still(false, pose, false));
        // Look-around moves the camera: drawn until it stops.
        let looked = Pose {
            angles: [0.1, 0., 0.],
            ..pose
        };
        assert!(!still.still(true, looked, false));
        assert!(still.still(true, looked, false));
        // A view change or zoom counts as movement too.
        assert!(!still.still(true, Pose { view: 3, ..looked }, false));
        assert!(!still.still(
            true,
            Pose {
                zoom: 2.,
                view: 3,
                ..looked
            },
            false
        ));
        // Held look keys, a head tracker or an animating screen keep drawing.
        let settled = Pose {
            zoom: 2.,
            view: 3,
            ..looked
        };
        assert!(still.still(true, settled, false));
        assert!(!still.still(true, settled, true));
    }
}
