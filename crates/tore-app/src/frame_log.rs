//! Opt-in host diagnostics. No simulation state or adapter policy is changed.
//! Off paths check one atomic flag, without clock reads, TLS access or allocation.
use std::{
    cell::RefCell,
    fmt::Write,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(false);
thread_local! { static LOG: RefCell<Option<Box<Recorder>>> = const { RefCell::new(None) }; }
pub const DEFAULT_MS: f64 = 50.;
const MAX_LINES: u32 = 10;
const PHASES: usize = 19;
const NAMES: [&str; PHASES] = [
    "simulation",
    "network",
    "terrain",
    "vegetation",
    "cities_scenery",
    "aircraft_stores",
    "surface_units",
    "effects_smoke",
    "sky_stars",
    "hud",
    "panels_mirrors",
    "overlays_text",
    "encoding",
    "submit",
    "acquire",
    "present",
    "render_other",
    "gpu_readback",
    "canvas_upload",
];
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Phase {
    Simulation,
    Network,
    Terrain,
    Vegetation,
    Cities,
    Aircraft,
    Surface,
    Effects,
    Sky,
    Hud,
    Panels,
    Ui,
    Encoding,
    Submit,
    Acquire,
    Present,
    Render,
    Readback,
    Upload,
}
const COUNTS: usize = 15;
const COUNT_NAMES: [&str; COUNTS] = [
    "ticks",
    "terrain_chunks",
    "vegetation_placed",
    "aircraft_contacts",
    "surface_vertices",
    "effect_instances",
    "smoke_instances",
    "mirror_views",
    "panel_views",
    "world_draw_calls",
    "city_draw_calls",
    "vegetation_draw_calls",
    "effect_draw_calls",
    "routed_surface_units",
    "network_prediction_tick_advance",
];
#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Count {
    Ticks,
    Terrain,
    Vegetation,
    Aircraft,
    Surface,
    Effects,
    Smoke,
    Mirrors,
    Panels,
    WorldDraws,
    CityDraws,
    VegetationDraws,
    EffectDraws,
    SurfaceUnits,
    NetworkTicks,
}

#[inline]
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub fn threshold(text: &str) -> Result<Option<f64>, &'static str> {
    match text {
        "off" | "0" => Ok(None),
        "on" | "true" | "" => Ok(Some(DEFAULT_MS)),
        _ => text
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v > 0. && *v <= 60_000.)
            .map(Some)
            .ok_or(
                "frame log threshold must be a finite number above 0 and at most 60000 ms, or off",
            ),
    }
}
pub fn configure(ms: Option<f64>) {
    LOG.with_borrow_mut(|slot| *slot = ms.map(|ms| Box::new(Recorder::new(ms, Instant::now()))));
    ENABLED.store(ms.is_some(), Ordering::Relaxed);
    if let Some(ms) = ms {
        log::info!(
            "Frame log enabled: threshold_ms={ms:.3}, maximum 10 slow lines/s; exclusive main-thread ms; AI included in simulation/network; GPU is asynchronous previous work"
        );
    } else {
        log::info!("Frame log disabled");
    }
}
pub fn toggle() {
    configure(if enabled() { None } else { Some(DEFAULT_MS) });
}

/// Exclusive wall time: nested scopes charge their own category, not the parent.
/// The totals baseline avoids an allocated scope stack and supports repeated phases.
pub struct Span(Option<(Instant, Phase, f64)>);
#[inline]
pub fn span(phase: Phase) -> Span {
    if !enabled() {
        return Span(None);
    }
    Span(LOG.with_borrow(|r| r.as_ref().map(|r| (Instant::now(), phase, r.total()))))
}
impl Drop for Span {
    fn drop(&mut self) {
        if let Some((start, phase, before)) = self.0 {
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            LOG.with_borrow_mut(|r| {
                if let Some(r) = r {
                    r.charge(phase, before, elapsed);
                }
            });
        }
    }
}
#[inline]
pub fn count(which: Count, n: usize) {
    if enabled() {
        LOG.with_borrow_mut(|r| {
            if let Some(r) = r {
                r.counts[which as usize] += n as u64;
            }
        });
    }
}

/// Called only after the pacer accepts a redraw. Network work between redraws
/// stays in the current interval; the rest of that gap is idle/event work.
pub fn begin(
    screen: &'static str,
    view: u8,
    theater: &str,
    role: &'static str,
    focused: bool,
    compression: f64,
) {
    if !enabled() {
        return;
    }
    LOG.with_borrow_mut(|slot| {
        if let Some(r) = slot {
            let now = Instant::now();
            if r.last.is_none() {
                r.phases.fill(0.);
                r.counts.fill(0);
                r.minute = now;
                r.rate = Rate::new(now);
            }
            r.idle = r.last.map_or(0., |last| {
                (now.duration_since(last).as_secs_f64() * 1000. - r.total()).max(0.)
            });
            r.start = Some(now);
            r.screen = screen;
            r.role = role;
            r.focused = focused;
            r.view = view;
            r.theater.clear();
            r.theater.push_str(theater);
            r.compression = compression;
        }
    });
}
/// Replay uses its own theater, camera and playhead speed, not the parked flight's.
pub fn replay_context(view: u8, theater: &str, speed: f64) {
    if !enabled() {
        return;
    }
    LOG.with_borrow_mut(|slot| {
        if let Some(r) = slot {
            r.view = view;
            r.theater.clear();
            r.theater.push_str(theater);
            r.compression = speed;
        }
    });
}
pub fn finish(gpu: Option<f64>) {
    if !enabled() {
        return;
    }
    LOG.with_borrow_mut(|r| {
        if let Some(r) = r {
            let now = Instant::now();
            if let Some(start) = r.start.take() {
                let wall = now.duration_since(r.last.unwrap_or(start)).as_secs_f64() * 1000.;
                r.record(now, wall, gpu);
            }
            r.last = Some(now);
            r.phases.fill(0.);
            r.counts.fill(0);
        }
    });
}

struct Rate {
    start: Instant,
    emitted: u32,
    suppressed: u64,
}
impl Rate {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            emitted: 0,
            suppressed: 0,
        }
    }
    fn roll(&mut self, now: Instant) -> u64 {
        if now.duration_since(self.start) < Duration::from_secs(1) {
            return 0;
        }
        let n = self.suppressed;
        *self = Self::new(now);
        n
    }
    fn allow(&mut self) -> bool {
        if self.emitted < MAX_LINES {
            self.emitted += 1;
            true
        } else {
            self.suppressed += 1;
            false
        }
    }
}
/// Fixed histogram: 0.25 ms bins through 1 second, then powers of two.
/// Bounded memory even with uncapped rendering. Percentiles are upper bounds.
struct Summary {
    bins: [u64; 4017],
    frames: u64,
    max: f64,
    sum: f64,
    phase_sum: [f64; PHASES],
    phase_max: [f64; PHASES],
}
impl Default for Summary {
    fn default() -> Self {
        Self {
            bins: [0; 4017],
            frames: 0,
            max: 0.,
            sum: 0.,
            phase_sum: [0.; PHASES],
            phase_max: [0.; PHASES],
        }
    }
}
impl Summary {
    fn add(&mut self, wall: f64, phases: &[f64; PHASES]) {
        let bin = if wall <= 1000. {
            (wall * 4.).ceil() as usize
        } else {
            4000 + (wall / 1000.).log2().ceil() as usize
        };
        self.bins[bin.min(4016)] += 1;
        self.frames += 1;
        self.sum += wall;
        self.max = self.max.max(wall);
        for (i, value) in phases.iter().enumerate() {
            self.phase_sum[i] += value;
            self.phase_max[i] = self.phase_max[i].max(*value);
        }
    }
    fn percentile(&self, p: f64) -> f64 {
        let target = (self.frames as f64 * p).ceil() as u64;
        let mut count = 0;
        for (i, n) in self.bins.iter().enumerate() {
            count += n;
            if count >= target {
                return if i <= 4000 {
                    i as f64 / 4.
                } else {
                    1000. * 2f64.powi(i as i32 - 4000)
                };
            }
        }
        0.
    }
    fn line(&self) -> String {
        let mut order: Vec<_> = (0..PHASES).collect();
        order.sort_by(|a, b| self.phase_sum[*b].total_cmp(&self.phase_sum[*a]));
        let mut line = format!(
            "Frame log minute: frames={} mean_ms={:.2} p50_le_ms={:.2} p95_le_ms={:.2} p99_le_ms={:.2} max_ms={:.2} worst_phases(mean/max_ms)=",
            self.frames,
            self.sum / self.frames.max(1) as f64,
            self.percentile(0.5),
            self.percentile(0.95),
            self.percentile(0.99),
            self.max
        );
        for i in order.into_iter().take(5) {
            let _ = write!(
                line,
                "{}:{:.2}/{:.2},",
                NAMES[i],
                self.phase_sum[i] / self.frames.max(1) as f64,
                self.phase_max[i]
            );
        }
        line
    }
}
struct Recorder {
    threshold: f64,
    last: Option<Instant>,
    start: Option<Instant>,
    idle: f64,
    phases: [f64; PHASES],
    counts: [u64; COUNTS],
    screen: &'static str,
    role: &'static str,
    focused: bool,
    view: u8,
    theater: String,
    compression: f64,
    rate: Rate,
    minute: Instant,
    summary: Summary,
}
impl Recorder {
    fn new(threshold: f64, now: Instant) -> Self {
        Self {
            threshold,
            last: None,
            start: None,
            idle: 0.,
            phases: [0.; PHASES],
            counts: [0; COUNTS],
            screen: "unknown",
            role: "single",
            focused: false,
            view: 0,
            theater: String::new(),
            compression: 1.,
            rate: Rate::new(now),
            minute: now,
            summary: Summary::default(),
        }
    }
    fn total(&self) -> f64 {
        self.phases.iter().sum()
    }
    fn charge(&mut self, phase: Phase, before: f64, elapsed: f64) {
        self.phases[phase as usize] += (elapsed - (self.total() - before)).max(0.);
    }
    fn slow(&self, wall: f64) -> bool {
        wall > self.threshold
    }
    fn line(&self, wall: f64, gpu: Option<f64>) -> String {
        let mut line = format!(
            "Slow frame: wall_ms={wall:.2} idle_event_ms={:.2} unaccounted_ms={:.2} screen={} view={} theater={:?} role={} focused={} compression={} ms=[",
            self.idle,
            (wall - self.idle - self.total()).max(0.),
            self.screen,
            self.view,
            self.theater,
            self.role,
            self.focused,
            self.compression
        );
        for (name, ms) in NAMES.iter().zip(self.phases) {
            let _ = write!(line, "{name}:{ms:.2},");
        }
        line.push_str("] counts=[");
        for (name, n) in COUNT_NAMES.iter().zip(self.counts) {
            let _ = write!(line, "{name}:{n},");
        }
        line.push_str("] gpu_previous_ms=");
        if let Some(ms) = gpu {
            let _ = write!(line, "{ms:.2}");
        } else {
            line.push_str("unavailable");
        }
        line.push_str(" ai=in_simulation_or_network");
        line
    }
    fn record(&mut self, now: Instant, wall: f64, gpu: Option<f64>) {
        let suppressed = self.rate.roll(now);
        if suppressed > 0 {
            log::info!("Frame log: {suppressed} more slow frames (rate limited)");
        }
        self.summary.add(wall, &self.phases);
        if self.slow(wall) && self.rate.allow() {
            log::info!("{}", self.line(wall, gpu));
        }
        if now.duration_since(self.minute) >= Duration::from_secs(60) {
            log::info!("{}", self.summary.line());
            self.summary = Summary::default();
            self.minute = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thresholds_are_strict_and_reject_invalid_values() {
        assert_eq!(threshold("on"), Ok(Some(50.)));
        assert_eq!(threshold("0.1"), Ok(Some(0.1)));
        assert_eq!(threshold("off"), Ok(None));
        for value in ["NaN", "inf", "-1", "60001", "garbage"] {
            assert!(threshold(value).is_err());
        }
        let r = Recorder::new(50., Instant::now());
        assert!(!r.slow(50.));
        assert!(r.slow(50.001));
    }
    #[test]
    fn rate_limit_counts_and_rolls_without_a_slow_frame() {
        let now = Instant::now();
        let mut r = Rate::new(now);
        for _ in 0..10 {
            assert!(r.allow());
        }
        for _ in 0..7 {
            assert!(!r.allow());
        }
        assert_eq!(r.roll(now + Duration::from_millis(999)), 0);
        assert_eq!(r.roll(now + Duration::from_secs(1)), 7);
        assert!(r.allow());
        assert_eq!(r.roll(now + Duration::from_secs(3)), 0);
    }
    #[test]
    fn nested_scopes_charge_only_exclusive_time() {
        let mut r = Recorder::new(50., Instant::now());
        r.charge(Phase::Network, 0., 2.);
        let before = r.total();
        r.charge(Phase::Terrain, before, 3.);
        r.charge(Phase::Submit, r.total(), 4.);
        r.charge(Phase::Render, before, 10.);
        assert_eq!(r.phases[Phase::Render as usize], 3.);
        assert_eq!(r.total(), 12.);
        // Nested scopes in the same category do not double count either.
        let before = r.total();
        r.charge(Phase::Panels, before, 2.);
        r.charge(Phase::Panels, before, 6.);
        assert_eq!(r.phases[Phase::Panels as usize], 6.);
    }
    #[test]
    fn summary_percentiles_and_worst_phases() {
        let mut s = Summary::default();
        let mut p = [0.; PHASES];
        p[Phase::Acquire as usize] = 8.;
        for ms in 1..=100 {
            s.add(ms as f64, &p);
        }
        let line = s.line();
        assert!(line.contains(
            "frames=100 mean_ms=50.50 p50_le_ms=50.00 p95_le_ms=95.00 p99_le_ms=99.00 max_ms=100.00"
        ));
        assert!(line.contains("acquire:8.00/8.00"));
        let r = Recorder::new(50., Instant::now());
        let line = r.line(100., None);
        assert!(line.contains("gpu_previous_ms=unavailable"));
        assert!(line.contains("ticks:0"));
        assert!(!line.contains('\n'));
    }
}
