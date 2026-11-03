//! World-owned prediction jobs. Pending work and published observation ages
//! are simulation state, so frame readers cannot spend or change the budget.
use super::{
    energy::{Body, DT, Observation},
    energy_estimate::{Answer, Queue, Request, Trial},
};
use crate::attitude::{Vector, cross, dot, unit};

pub const ENVELOPE_BIT: u64 = 1 << 63;
pub const MISSILE_BIT: u64 = 1 << 62;

/// Probe both sides of the data-driven loft transition. A farther lofted
/// intercept can succeed after an unlofted shot has already become unreachable.
/// Coarse doubling alone can miss that disconnected successful interval.
pub fn envelope_probe_scales(body: Body) -> &'static [f64] {
    if body.loft_height > 0. {
        &[
            0.0625, 0.125, 0.25, 0.5, 0.6, 0.600001, 0.625, 0.65, 0.675, 0.7, 0.75, 1., 1.5, 2., 4.,
        ]
    } else {
        &[0.0625, 0.125, 0.25, 0.5, 0.75, 1., 1.5, 2., 4.]
    }
}

fn norm(v: Vector) -> f64 {
    dot(v, v).sqrt()
}
fn sub(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] - b[i])
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Published {
    pub at: u64,
    pub question: Trial,
    pub answer: Answer,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Client {
    pub id: u64,
    pub target: u32,
    pub touched: u64,
    pub submitted: u64,
    pub question: Trial,
    pub pending: bool,
    pub waiting_since: u64,
    pub result: Option<Published>,
    pub player: bool,
    pub observed: Option<ObservedMotion>,
    pub pending_turn: TurnRates,
    pub result_turn: TurnRates,
}

/// Rates are measured from consecutive observations, never inferred from a
/// future target pose. They qualify player advice only, not AI permission.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TurnRates {
    pub launcher: Vector,
    pub axis: Vector,
    pub target: Vector,
    pub valid: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObservedMotion {
    pub at: u64,
    pub question: Trial,
    pub rates: TurnRates,
}
fn angular_rate(a: Vector, b: Vector, seconds: f64) -> Option<Vector> {
    if norm(a) < 1e-9 || norm(b) < 1e-9 {
        return Some([0.; 3]);
    }
    let a = unit(a);
    let b = unit(b);
    let normal = cross(a, b);
    let sine = norm(normal);
    let angle = sine.atan2(dot(a, b));
    if angle / seconds > 20f64.to_radians() {
        return None;
    }
    Some(normal.map(|v| v * angle / seconds / sine.max(1e-12)))
}
fn rotate(v: Vector, rate: Vector, seconds: f64) -> Vector {
    let speed = norm(rate);
    if speed < 1e-9 {
        return v;
    }
    let axis = rate.map(|v| v / speed);
    let (sin, cos) = (speed * seconds).sin_cos();
    let side = cross(axis, v);
    let along = dot(axis, v);
    std::array::from_fn(|i| v[i] * cos + side[i] * sin + axis[i] * along * (1. - cos))
}
fn arc(position: Vector, velocity: Vector, rate: Vector, seconds: f64) -> Vector {
    let speed = norm(rate);
    if speed < 1e-9 {
        return std::array::from_fn(|i| position[i] + velocity[i] * seconds);
    }
    let axis = rate.map(|v| v / speed);
    let along = dot(axis, velocity);
    let side = cross(axis, velocity);
    let (sin, cos) = (speed * seconds).sin_cos();
    std::array::from_fn(|i| {
        position[i]
            + axis[i] * along * seconds
            + (velocity[i] - axis[i] * along) * sin / speed
            + side[i] * (1. - cos) / speed
    })
}
fn player_compatible(
    result: Published,
    rates: TurnRates,
    current: Trial,
    observed: TurnRates,
    tick: u64,
) -> bool {
    let ticks = tick.saturating_sub(result.at);
    if !rates.valid || !observed.valid || ticks > 120 || current.flight.age != 0 {
        return false;
    }
    for (a, b) in [
        (rates.launcher, observed.launcher),
        (rates.axis, observed.axis),
        (rates.target, observed.target),
    ] {
        if norm(sub(a, b)) > 2f64.to_radians() {
            return false;
        }
    }
    let seconds = ticks as f64 * DT;
    let mut moved = result.question;
    moved.flight.position = arc(
        moved.flight.position,
        moved.flight.velocity,
        rates.launcher,
        seconds,
    );
    moved.flight.velocity = rotate(moved.flight.velocity, rates.launcher, seconds);
    moved.flight.axis = rotate(moved.flight.axis, rates.axis, seconds);
    moved.target.position = arc(
        moved.target.position,
        moved.target.velocity,
        rates.target,
        seconds,
    );
    moved.target.velocity = rotate(moved.target.velocity, rates.target, seconds);
    compatible(moved, current, 0)
}

/// Contiguous hit regions a range search keeps, nearest first. A farther
/// region beyond the last kept one is not advised (fitted, agent).
pub const MAX_REGIONS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Envelope {
    /// The advised contiguous region, meters. A stored search result holds
    /// its first region here; [`Envelope::advised`] picks one for a range.
    pub minimum: f64,
    pub maximum: f64,
    /// Favorable firing band, inside the first region only.
    pub favorable: Option<(f64, f64)>,
    /// Every sampled contiguous hit region, ascending, meters.
    pub regions: [(f64, f64); MAX_REGIONS],
    pub count: u8,
}
impl Envelope {
    fn from_regions(
        regions: [(f64, f64); MAX_REGIONS],
        count: u8,
        favorable: Option<(f64, f64)>,
    ) -> Self {
        let (minimum, maximum) = if count > 0 { regions[0] } else { (0., 0.) };
        Self {
            minimum,
            maximum,
            favorable,
            regions,
            count,
        }
    }
    pub fn regions(&self) -> &[(f64, f64)] {
        &self.regions[..usize::from(self.count).min(MAX_REGIONS)]
    }
    pub fn contains(self, range: f64) -> bool {
        self.maximum > self.minimum && (self.minimum..=self.maximum).contains(&range)
    }
    /// The one region to advise at `range` (lead decision, 2026-10-27): the
    /// region containing it; else the nearest region below it, which closing
    /// flies into; else, below every region, the first. The favorable band
    /// stays only while its own first region is advised.
    pub fn advised(self, range: f64) -> Self {
        let regions = self.regions();
        let Some(index) = regions
            .iter()
            .position(|&(low, high)| (low..=high).contains(&range))
            .or_else(|| regions.iter().rposition(|&(_, high)| high < range))
            .or((!regions.is_empty()).then_some(0))
        else {
            return self;
        };
        let (minimum, maximum) = regions[index];
        Self {
            minimum,
            maximum,
            favorable: self.favorable.filter(|_| index == 0),
            ..self
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RangeSnapshot {
    pub at: u64,
    pub question: Trial,
    pub envelope: Envelope,
}
/// What a boundary refinement in a range search is for.
const EDGE_RISING: u8 = 0;
const EDGE_FALLING: u8 = 1;
const EDGE_SPLIT_FALLING: u8 = 2;
const EDGE_SPLIT_RISING: u8 = 3;
/// Interior gaps the favorable-band pass may split out of the first region.
const MAX_SPLITS: u8 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct RangeSearch {
    pub id: u64,
    pub target: u32,
    pub touched: u64,
    pub submitted: u64,
    pub waiting_since: u64,
    pub template: Trial,
    pub bearing: Vector,
    pub cap: f64,
    pub distance: f64,
    /// Decoded launch minimum, independent of moving target geometry.
    pub minimum: f64,
    /// Start of the hit region the outward scan is inside, if any.
    pub open: Option<f64>,
    /// The previous scan sample and whether it hit.
    pub previous_hit: Option<bool>,
    pub previous_distance: f64,
    /// The scan sample a refinement interrupted; the scan resumes after it.
    pub resume: f64,
    /// Which boundary the current refinement brackets (`EDGE_*`).
    pub edge: u8,
    /// Interior-gap split: the missed sample and the region's old far edge.
    pub split: (f64, f64),
    pub splits: u8,
    /// Closed regions found so far, ascending.
    pub regions: [(f64, f64); MAX_REGIONS],
    pub count: u8,
    /// Refinement bracket: the lower and upper distance of a hit/miss pair.
    pub low: f64,
    pub high: f64,
    pub phase: u8,
    pub iteration: u8,
    pub run: Option<(f64, f64)>,
    pub best: Option<(f64, f64)>,
    pub result: Option<Envelope>,
    /// Earlier envelope and its original observation. It can remain visible
    /// during a restarted search only while its age and geometry qualify.
    pub previous: Option<RangeSnapshot>,
    pub done: bool,
}
impl RangeSearch {
    pub fn new(id: u64, target: u32, tick: u64, template: Trial) -> Self {
        Self::new_with_minimum(id, target, tick, template, 100.)
    }
    pub fn new_with_minimum(
        id: u64,
        target: u32,
        tick: u64,
        template: Trial,
        minimum: f64,
    ) -> Self {
        let minimum = minimum.max(1.);
        let b = template.body;
        let life = f64::from(b.life.saturating_sub(template.start_age)) * DT;
        let impulse = b.boost.thrust * f64::from(b.boost.ticks) * DT
            + b.sustain.thrust * f64::from(b.sustain.ticks) * DT;
        let cap = (norm(template.flight.velocity)
            + impulse / b.dry_mass()
            + norm(template.target.velocity))
            * life
            + 0.5 * super::energy::G * life * life
            + template.radius;
        Self {
            id,
            target,
            touched: tick,
            submitted: tick,
            waiting_since: tick,
            template,
            bearing: unit([
                template.target.position[0] - template.flight.position[0],
                0.,
                template.target.position[2] - template.flight.position[2],
            ]),
            cap,
            minimum,
            open: None,
            previous_hit: None,
            previous_distance: 0.,
            resume: 0.,
            edge: EDGE_RISING,
            split: (0., 0.),
            splits: 0,
            regions: [(0., 0.); MAX_REGIONS],
            count: 0,
            distance: minimum
                .max(template.radius + 1.)
                .max((template.target.position[1] - template.flight.position[1]).abs() + 0.01)
                .min(cap),
            low: 0.,
            high: cap,
            phase: 0,
            iteration: 0,
            run: None,
            best: None,
            result: None,
            previous: None,
            done: false,
        }
    }
    fn trial(&self) -> Trial {
        let mut trial = self.template;
        let vertical = trial.target.position[1] - trial.flight.position[1];
        let horizontal = (self.distance * self.distance - vertical * vertical)
            .max(0.)
            .sqrt();
        trial.target.position[0] = trial.flight.position[0] + self.bearing[0] * horizontal;
        trial.target.position[2] = trial.flight.position[2] + self.bearing[2] * horizontal;
        trial
    }
    fn publish(&mut self, favorable: Option<(f64, f64)>) {
        self.result = Some(Envelope::from_regions(self.regions, self.count, favorable));
    }
    fn refine(&mut self, edge: u8, low: f64, high: f64) {
        self.phase = 1;
        self.edge = edge;
        self.iteration = 0;
        self.low = low;
        self.high = high;
        self.distance = (low + high) * 0.5;
    }
    fn start_band_pass(&mut self) {
        self.phase = 2;
        self.iteration = 0;
        self.run = None;
        self.best = None;
        self.distance = self.regions[0].0;
    }
    /// The next outward sample: a bounded 1.25 ratio grid that keeps the
    /// shared loft-transition samples and the current shot itself.
    fn advance_scan(&mut self) {
        if usize::from(self.count) >= MAX_REGIONS || self.distance >= self.cap {
            if let Some(start) = self.open.take()
                && usize::from(self.count) < MAX_REGIONS
            {
                self.regions[usize::from(self.count)] = (start, self.distance);
                self.count += 1;
            }
            if self.count == 0 {
                self.result = Some(Envelope::default());
                self.done = true;
            } else {
                self.publish(None);
                self.start_band_pass();
            }
            return;
        }
        let anchor = (self.template.body.loft_range / 0.6).max(1600.);
        let shot = norm(sub(
            self.template.target.position,
            self.template.flight.position,
        ));
        let mut next = (self.distance * 1.25).min(self.cap);
        for distance in envelope_probe_scales(self.template.body)
            .iter()
            .map(|scale| anchor * scale)
            .chain([shot])
        {
            if distance > self.distance * (1. + 1e-9) {
                next = next.min(distance);
            }
        }
        self.distance = next;
    }
    fn resume_scan(&mut self, hit: bool) {
        self.phase = 0;
        self.previous_hit = Some(hit);
        self.previous_distance = self.resume;
        self.distance = self.resume;
        self.advance_scan();
    }
    fn accept(&mut self, answer: Answer) {
        match self.phase {
            0 => {
                let here = self.distance;
                match (self.open, answer.hit) {
                    (None, true) if self.previous_hit == Some(false) => {
                        // A region starts between a miss and this hit.
                        self.resume = here;
                        self.refine(EDGE_RISING, self.previous_distance, here);
                        return;
                    }
                    (None, true) => self.open = Some(here),
                    (Some(_), false) => {
                        // The first miss after a hit closes this region.
                        self.resume = here;
                        self.refine(EDGE_FALLING, self.previous_distance, here);
                        return;
                    }
                    _ => {}
                }
                self.previous_hit = Some(answer.hit);
                self.previous_distance = here;
                self.advance_scan();
            }
            1 => {
                // The hit side of the bracket is low for a falling edge and
                // high for a rising one.
                let falling = matches!(self.edge, EDGE_FALLING | EDGE_SPLIT_FALLING);
                if answer.hit == falling {
                    self.low = self.distance;
                } else {
                    self.high = self.distance;
                }
                self.iteration += 1;
                if self.iteration < 10 {
                    self.distance = (self.low + self.high) * 0.5;
                    return;
                }
                match self.edge {
                    EDGE_RISING => {
                        self.open = Some(self.high);
                        self.resume_scan(true);
                    }
                    EDGE_FALLING => {
                        let start = self.open.take().unwrap_or(self.low);
                        self.regions[usize::from(self.count)] = (start, self.low);
                        self.count += 1;
                        self.publish(None);
                        self.resume_scan(false);
                    }
                    EDGE_SPLIT_FALLING => {
                        // A gap inside the first region: its near part ends
                        // here, at once; the far part is refined next.
                        self.regions[0].1 = self.low;
                        self.publish(None);
                        let (gap, end) = self.split;
                        self.refine(EDGE_SPLIT_RISING, gap, end);
                    }
                    _ => {
                        let tail = (self.high, self.split.1);
                        let last = usize::from(self.count).min(MAX_REGIONS - 1);
                        for index in (1..=last).rev() {
                            self.regions[index] = self.regions[index - 1];
                        }
                        self.regions[1] = tail;
                        self.count = (self.count + 1).min(MAX_REGIONS as u8);
                        self.publish(None);
                        self.start_band_pass();
                    }
                }
            }
            _ => {
                let (start, end) = self.regions[0];
                if !answer.hit && self.iteration > 0 && self.splits < MAX_SPLITS {
                    // The interior pass can expose a gap narrower than the
                    // scan grid. Split the first region around it.
                    self.splits += 1;
                    self.split = (self.distance, end);
                    let before = start + (end - start) * f64::from(self.iteration - 1) / 16.;
                    self.refine(EDGE_SPLIT_FALLING, before, self.distance);
                    return;
                }
                let relative = (self.distance / end.max(1.)).clamp(0., 1.);
                let life = f64::from(self.template.body.life) * DT;
                let score = 95.
                    * (1. - 0.75 * relative * relative)
                    * (1. - 0.6 * answer.seconds / life).clamp(0., 1.);
                if answer.hit && score >= 70. {
                    self.run = Some(self.run.map_or((self.distance, self.distance), |(lo, _)| {
                        (lo, self.distance)
                    }));
                    let run = self.run.unwrap();
                    if run.1 > run.0 && self.best.is_none_or(|best| run.1 - run.0 > best.1 - best.0)
                    {
                        self.best = Some(run);
                    }
                } else {
                    self.run = None;
                }
                self.iteration += 1;
                if self.iteration == 17 {
                    self.done = true;
                    self.publish(self.best);
                } else {
                    self.distance = start + (end - start) * f64::from(self.iteration) / 16.;
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Service {
    pub queue: Queue,
    pub clients: Vec<Client>,
    pub ranges: Vec<RangeSearch>,
    /// Admission rotates across waiting point and range clients, never actor order.
    pub admission_cursor: usize,
}
impl Service {
    pub fn request(&mut self, id: u64, target: u32, tick: u64, question: Trial, refresh: u64) {
        // Same-aircraft rails can share a qualified recent answer. They already
        // use the same geometric compatibility tolerance; duplicating identical
        // flyouts on every rail delays fresh answers in a turning fight.
        let reused = (id & MISSILE_BIT == 0 && question.flight.age == 0)
            .then(|| {
                self.clients
                    .iter()
                    .filter(|c| c.id != id && c.id >> 16 == id >> 16 && c.target == target)
                    .filter_map(|c| c.result)
                    .filter(|r| {
                        (tick.saturating_sub(r.at) < refresh || r.question == question)
                            && compatible(r.question, question, tick.saturating_sub(r.at))
                    })
                    .max_by_key(|r| r.at)
            })
            .flatten();
        if let Some(result) = reused {
            self.queue.remove(id);
            let replacement = Client {
                id,
                target,
                touched: tick,
                submitted: result.at,
                question,
                pending: false,
                waiting_since: tick,
                result: Some(result),
                player: false,
                observed: None,
                pending_turn: TurnRates::default(),
                result_turn: TurnRates::default(),
            };
            if let Some(client) = self.clients.iter_mut().find(|c| c.id == id) {
                *client = replacement;
            } else {
                self.clients.push(replacement);
            }
            return;
        }
        if let Some(client) = self.clients.iter_mut().find(|c| c.id == id) {
            client.touched = tick;
            client.player = false;
            if client.target != target || client.question.body != question.body {
                client.target = target;
                client.pending = false;
                client.result = None;
                self.queue.remove(id);
            }
            if client.pending && !self.queue.requests.iter().any(|r| r.id == id) {
                client.question = question;
                client.submitted = tick;
            }
            // The world terrain is fixed. Identical physical inputs need no
            // new flyout merely because a refresh interval passed.
            if client.result.is_some_and(|r| r.question == question) {
                return;
            }
            // A published answer that no longer matches the question cannot be
            // read (see `answer`), so waiting for the refresh interval only
            // leaves the cue and the AI gate dark. Short shots go stale within
            // a few ticks of launcher motion.
            let stale = client
                .result
                .is_some_and(|r| !compatible(r.question, question, tick.saturating_sub(r.at)));
            if !client.pending
                && (client.result.is_none()
                    || stale
                    || tick.saturating_sub(client.submitted) >= refresh)
            {
                client.question = question;
                client.submitted = tick;
                client.pending = true;
                client.waiting_since = tick;
            }
            return;
        }
        self.clients.push(Client {
            id,
            target,
            touched: tick,
            submitted: tick,
            question,
            pending: true,
            waiting_since: tick,
            result: None,
            player: false,
            observed: None,
            pending_turn: TurnRates::default(),
            result_turn: TurnRates::default(),
        });
    }
    pub fn request_player(&mut self, id: u64, target: u32, tick: u64, question: Trial) {
        let before = self
            .clients
            .iter()
            .find(|c| c.id == id && c.target == target)
            .and_then(|c| c.observed);
        let rates = before
            .and_then(|old| {
                let ticks = tick.checked_sub(old.at)?;
                if ticks == 0 || ticks > 12 {
                    return None;
                }
                let dt = ticks as f64 * DT;
                Some(TurnRates {
                    launcher: angular_rate(
                        old.question.flight.velocity,
                        question.flight.velocity,
                        dt,
                    )?,
                    axis: angular_rate(old.question.flight.axis, question.flight.axis, dt)?,
                    target: angular_rate(
                        old.question.target.velocity,
                        question.target.velocity,
                        dt,
                    )?,
                    valid: true,
                })
            })
            .unwrap_or_default();
        self.request(id, target, tick, question, 30);
        let client = self.clients.iter_mut().find(|c| c.id == id).unwrap();
        client.player = true;
        client.observed = Some(ObservedMotion {
            at: tick,
            question,
            rates,
        });
        if client.pending && client.submitted == tick {
            client.pending_turn = rates;
        }
    }
    /// Display-only qualification. AI callers must keep using `answer`.
    pub fn player_answer(
        &self,
        id: u64,
        target: u32,
        question: Trial,
        tick: u64,
    ) -> Option<Published> {
        if let Some(answer) = self.answer(id, target, question, tick) {
            return Some(answer);
        }
        let c = self
            .clients
            .iter()
            .find(|c| c.id == id && c.target == target && c.player)?;
        let result = c.result?;
        player_compatible(result, c.result_turn, question, c.observed?.rates, tick)
            .then_some(result)
    }
    pub fn request_range(&mut self, id: u64, target: u32, tick: u64, question: Trial) {
        self.request_range_with_minimum(id, target, tick, question, 100.);
    }
    pub fn request_range_with_minimum(
        &mut self,
        id: u64,
        target: u32,
        tick: u64,
        question: Trial,
        minimum: f64,
    ) {
        let id = id | ENVELOPE_BIT;
        if let Some(range) = self.ranges.iter_mut().find(|r| r.id == id) {
            range.touched = tick;
            // A range does not depend on the present distance. Keep the
            // normalized geometry while a long cold search is still running.
            let same_model = range.target == target
                && range.minimum == minimum.max(1.)
                && envelope_model_matches(range.template, question);
            // Finish a cold search before resampling moving geometry. Otherwise
            // a sustained turn can restart it forever without one publication.
            if same_model
                && (range.result.is_none()
                    || (tick.saturating_sub(range.submitted) <= 1200
                        && envelope_compatible(range.template, question)))
            {
                return;
            }
            let previous = same_model
                .then_some(
                    range
                        .result
                        .map(|envelope| RangeSnapshot {
                            at: range.submitted,
                            question: range.template,
                            envelope,
                        })
                        .or(range.previous),
                )
                .flatten();
            self.queue.remove(id);
            *range = RangeSearch::new_with_minimum(id, target, tick, question, minimum);
            range.previous = previous;
            return;
        }
        self.ranges.push(RangeSearch::new_with_minimum(
            id, target, tick, question, minimum,
        ));
    }
    /// Waiting requests are retained even when all active slots are occupied.
    /// Each admitted flyout finishes before its slot is reused. A persistent
    /// round-robin cursor gives every waiting client a turn under saturation.
    fn admit(&mut self) {
        let count = self.clients.len() + self.ranges.len();
        if count == 0 {
            return;
        }
        let mut players: Vec<_> = self
            .clients
            .iter()
            .filter(|c| c.player && c.pending)
            .collect();
        // Multiple human seats share this service. A refreshed early seat
        // must not take a reserved slot ahead of an older waiting question.
        players.sort_by_key(|c| (c.waiting_since, c.id));
        for c in players {
            if self.queue.requests.len() >= super::energy_estimate::MAX_REQUESTS {
                break;
            }
            if !self.queue.requests.iter().any(|r| r.id == c.id) {
                self.queue.submit(Request {
                    id: c.id,
                    observed_tick: c.submitted,
                    trial: c.question,
                });
            }
        }
        let mut background = self
            .queue
            .requests
            .iter()
            .filter(|r| !self.clients.iter().any(|c| c.id == r.id && c.player))
            .count();
        let start = self.admission_cursor % count;
        for offset in 0..count {
            // Player requests were admitted above. Stop at the background cap
            // so the cursor remains after the last admitted client instead of
            // wrapping back to the start through requests we cannot admit.
            if self.queue.requests.len() >= super::energy_estimate::MAX_REQUESTS
                || background >= super::energy_estimate::MAX_REQUESTS - 8
            {
                break;
            }
            let index = (start + offset) % count;
            let request = if index < self.clients.len() {
                let c = &self.clients[index];
                c.pending.then_some(Request {
                    id: c.id,
                    observed_tick: c.submitted,
                    trial: c.question,
                })
            } else {
                let r = &self.ranges[index - self.clients.len()];
                (!r.done).then(|| Request {
                    id: r.id,
                    observed_tick: r.submitted,
                    trial: r.trial(),
                })
            };
            if let Some(request) = request
                && !self.queue.requests.iter().any(|r| r.id == request.id)
            {
                let player = self.clients.iter().any(|c| c.id == request.id && c.player);
                if player || background < super::energy_estimate::MAX_REQUESTS - 8 {
                    self.queue.submit(request);
                    background += usize::from(!player);
                }
            }
            self.admission_cursor = (index + 1) % count;
        }
    }
    pub fn answer(&self, id: u64, target: u32, current: Trial, tick: u64) -> Option<Published> {
        let client = self
            .clients
            .iter()
            .find(|c| c.id == id && c.target == target)?;
        let mut result = client.result?;
        if !compatible(result.question, current, tick.saturating_sub(result.at)) {
            return None;
        }
        result.answer.seconds = (result.answer.seconds
            - f64::from(
                current
                    .flight
                    .age
                    .saturating_sub(result.question.flight.age),
            ) * DT)
            .max(0.);
        // Conservatively withhold a cached hit on/after the earliest defeat
        // if the present subsonic countdown continues. A fresh flyout may
        // later establish acceleration above threshold, but stale advice cannot.
        if current.body.defeat_mach > 0.
            && current.flight.subsonic_ticks > 0
            && result.answer.hit
            && result.answer.seconds
                >= f64::from(
                    current
                        .body
                        .defeat_ticks
                        .saturating_sub(current.flight.subsonic_ticks),
                ) * DT
        {
            return None;
        }
        Some(result)
    }
    pub fn envelope(&self, id: u64, target: u32, current: Trial, tick: u64) -> Option<Envelope> {
        let range = self
            .ranges
            .iter()
            .find(|r| r.id == (id | ENVELOPE_BIT) && r.target == target)?;
        if !envelope_model_matches(range.template, current) {
            return None;
        }
        range
            .result
            .map(|envelope| RangeSnapshot {
                at: range.submitted,
                question: range.template,
                envelope,
            })
            .into_iter()
            .chain(range.previous)
            .find(|snapshot| {
                let ticks = tick.saturating_sub(snapshot.at);
                // Carry the observation forward along both observed
                // velocities, as launch answers do, so straight flight
                // keeps its range while turns and reversals still drop it.
                let mut moved = snapshot.question;
                let seconds = ticks as f64 * DT;
                for i in 0..3 {
                    moved.flight.position[i] += moved.flight.velocity[i] * seconds;
                    moved.target.position[i] += moved.target.velocity[i] * seconds;
                }
                ticks <= 1200 && envelope_geometry(moved, current, 15., 10.)
            })
            .map(|snapshot| snapshot.envelope)
    }
    pub fn advance(&mut self, tick: u64, ground: &impl Fn(f64, f64) -> f64) -> usize {
        self.advance_over(tick, ground, &|_, _| false)
    }
    pub fn advance_over(
        &mut self,
        tick: u64,
        ground: &impl Fn(f64, f64) -> f64,
        clear: &impl Fn(Vector, Vector) -> bool,
    ) -> usize {
        for c in &mut self.clients {
            c.player &= c.observed.is_some_and(|o| o.at == tick);
        }
        self.admit();
        let priority: Vec<_> = self
            .clients
            .iter()
            .filter(|c| c.player && c.touched == tick)
            .map(|c| c.id)
            .collect();
        let used = self.queue.advance_prioritized(ground, clear, &priority);
        for client in &mut self.clients {
            if client.pending
                && let Some((at, answer)) = self.queue.answer(client.id)
            {
                client.pending = false;
                client.result_turn = client.pending_turn;
                client.result = Some(Published {
                    at,
                    question: client.question,
                    answer,
                });
                self.queue.remove(client.id);
            }
        }
        for range in &mut self.ranges {
            if !range.done
                && let Some((_, answer)) = self.queue.answer(range.id)
            {
                range.accept(answer);
                range.waiting_since = tick;
                self.queue.remove(range.id);
            }
        }
        for client in &self.clients {
            if tick.saturating_sub(client.touched) > 240 {
                self.queue.remove(client.id);
            }
        }
        for range in &self.ranges {
            if tick.saturating_sub(range.touched) > 240 {
                self.queue.remove(range.id);
            }
        }
        self.clients
            .retain(|c| tick.saturating_sub(c.touched) <= 240);
        self.ranges
            .retain(|c| tick.saturating_sub(c.touched) <= 240);
        used
    }
}

/// A published answer is explicitly an observation-snapshot estimate, not a
/// proof about future manoeuvres. Reject material changes instead of spending
/// unbounded synchronous work in a frame reader.
fn compatible(a: Trial, b: Trial, ticks: u64) -> bool {
    let ra = sub(a.target.position, a.flight.position);
    let rb = sub(b.target.position, b.flight.position);
    if a == b {
        return true;
    }
    let launch = a.flight.age == 0 && b.flight.age == 0;
    // Launch questions have no missile age. Advance both aircraft from the
    // observation tick, while in-flight answers use the actual missile age.
    let elapsed = if launch {
        ticks as f64
    } else {
        f64::from(b.flight.age.saturating_sub(a.flight.age))
    } * DT;
    let projected: Vector = std::array::from_fn(|i| {
        ra[i]
            + 0.5
                * (a.target.velocity[i] + b.target.velocity[i]
                    - a.flight.velocity[i]
                    - b.flight.velocity[i])
                * elapsed
    });
    let position: Vector = std::array::from_fn(|i| {
        a.flight.position[i] + 0.5 * (a.flight.velocity[i] + b.flight.velocity[i]) * elapsed
    });
    let tolerance = 30f64.max(norm(ra) * 0.02);
    (a == b || !launch || ticks <= 600)
        && a.body == b.body
        && a.flight.defeated == b.flight.defeated
        && a.flight.lost == b.flight.lost
        // Six-tick (50 ms) buckets retain useful in-flight answers while
        // the exact permanent defeat state above still invalidates at once.
        && a.flight.subsonic_ticks / 6 == b.flight.subsonic_ticks / 6
        && a.radius == b.radius
        && a.arming_age == b.arming_age
        && a.wind == b.wind
        && b.flight.age >= a.flight.age
        && b.flight.age - a.flight.age <= if a.body.cruise_drag { 3600 } else { 600 }
        && a.body.motor(a.flight.age).map(|(motor, _)| motor)
            == b.body.motor(b.flight.age).map(|(motor, _)| motor)
        && (a.body.mass_at(a.flight.age) - b.body.mass_at(b.flight.age)).abs() <= a.body.mass * 0.02
        && dot(a.flight.axis, b.flight.axis) >= 1f64.to_radians().cos()
        && norm(sub(projected, rb)) <= tolerance
        && norm(sub(position, b.flight.position)) <= tolerance
        && (position[1] - b.flight.position[1]).abs() <= 30.
        && norm(sub(a.flight.velocity, b.flight.velocity)) <= 10.
        && norm(sub(a.target.velocity, b.target.velocity)) <= 10.
}

fn envelope_model_matches(a: Trial, b: Trial) -> bool {
    a.body == b.body && a.radius == b.radius && a.arming_age == b.arming_age && a.wind == b.wind
}

fn envelope_compatible(a: Trial, b: Trial) -> bool {
    envelope_geometry(a, b, 5., 3.)
}
fn envelope_geometry(a: Trial, b: Trial, heading_degrees: f64, bearing_degrees: f64) -> bool {
    // The launch axis only shapes the first second of a range-scale flight;
    // a 1 degree tolerance restarted every search during gentle turns.
    envelope_model_matches(a, b)
        && norm(sub(a.flight.position, b.flight.position)) <= 1000.
        && dot(a.flight.axis, b.flight.axis) >= heading_degrees.to_radians().cos()
        && (norm(a.flight.velocity) - norm(b.flight.velocity)).abs() <= 10.
        && dot(unit(a.flight.velocity), unit(b.flight.velocity))
            >= heading_degrees.to_radians().cos()
        && norm(sub(a.target.velocity, b.target.velocity)) <= 25.
        && (a.flight.position[1] - b.flight.position[1]).abs() <= 150.
        && (a.target.position[1] - b.target.position[1]).abs() <= 150.
        && dot(
            unit([
                a.target.position[0] - a.flight.position[0],
                0.,
                a.target.position[2] - a.flight.position[2],
            ]),
            unit([
                b.target.position[0] - b.flight.position[0],
                0.,
                b.target.position[2] - b.flight.position[2],
            ]),
        ) >= bearing_degrees.to_radians().cos()
}

pub fn launch_question(
    w: &tore_formats::weapons::Weapon,
    launcher: super::super::live::Launcher,
    target: super::seeker::Observation,
    radius_ft: f64,
    wind: Vector,
) -> Trial {
    use super::energy::FT;
    let motion = super::Motion::launch_rules(
        w,
        launcher.velocity,
        launcher.position[1],
        super::Rules::Energy,
    );
    let energy = motion
        .energy
        .expect("a powered guided weapon has energy motion");
    let mut flight = energy.flight;
    flight.position = launcher.position.map(|v| v * FT);
    flight.axis = energy.body.launch_axis(launcher.basis.forward);
    let mut trial = Trial::new(
        energy.body,
        flight,
        Observation {
            position: target.position.map(|v| v * FT),
            velocity: target.velocity.map(|v| v * FT),
        },
        radius_ft * FT,
        u32::from(w.damage.fuze_arm_t) * 30,
    );
    trial.wind = wind.map(|v| v * FT);
    trial
}

crate::checkpoint_struct!(Published {
    at,
    question,
    answer
});
crate::checkpoint_struct!(Client {
    id,
    target,
    touched,
    submitted,
    question,
    pending,
    waiting_since,
    result,
    player,
    observed,
    pending_turn,
    result_turn
});
crate::checkpoint_struct!(TurnRates {
    launcher,
    axis,
    target,
    valid
});
crate::checkpoint_struct!(ObservedMotion {
    at,
    question,
    rates
});
crate::checkpoint_struct!(Envelope {
    minimum,
    maximum,
    favorable,
    regions,
    count
});
crate::checkpoint_struct!(RangeSnapshot {
    at,
    question,
    envelope
});
crate::checkpoint_struct!(RangeSearch {
    id,
    target,
    touched,
    submitted,
    waiting_since,
    template,
    bearing,
    cap,
    distance,
    minimum,
    open,
    previous_hit,
    previous_distance,
    resume,
    edge,
    split,
    splits,
    regions,
    count,
    low,
    high,
    phase,
    iteration,
    run,
    best,
    result,
    previous,
    done
});
crate::checkpoint_struct!(Service {
    queue,
    clients,
    ranges,
    admission_cursor
});

#[cfg(test)]
mod tests {
    use super::super::energy::{Flight, Observation};
    use super::*;
    fn question() -> Trial {
        Trial::new(
            super::super::energy::tests::body(),
            Flight::launch([0., 5000., 0.], [0., 0., 300.], [0., 0., 1.]),
            Observation {
                position: [0., 5000., 15000.],
                velocity: [0., 0., -250.],
            },
            30.,
            0,
        )
    }
    #[test]
    fn changed_defeat_policy_or_countdown_invalidates_a_cached_answer() {
        let mut q = question();
        q.flight.age = 1200;
        q.body.defeat_mach = 1.;
        assert!(compatible(q, q, 0));
        let mut changed = q;
        changed.body.defeat_mach = 0.9;
        assert!(!compatible(q, changed, 0));
        changed = q;
        changed.flight.subsonic_ticks = 29;
        assert!(!compatible(q, changed, 0));
        changed = q;
        changed.flight.defeated = true;
        changed.flight.lost = true;
        assert!(!compatible(q, changed, 0));
    }
    #[test]
    fn countdown_buckets_reuse_answers_but_never_a_hit_on_or_after_defeat() {
        let mut q = question();
        q.body.defeat_mach = 1.;
        q.flight.age = 1200;
        q.flight.subsonic_ticks = 24;
        let mut service = Service::default();
        service.request(7, 2, 0, q, 15);
        let published = Published {
            at: 0,
            question: q,
            answer: Answer {
                hit: true,
                seconds: 0.02,
                closest: 0.,
                impact_speed: 300.,
            },
        };
        service.clients[0].result = Some(published);
        let mut current = q;
        current.flight.age += 1;
        current.flight.subsonic_ticks += 1;
        assert!(compatible(q, current, 1), "same 50 ms bucket");
        assert!(service.answer(7, 2, current, 1).is_some());
        service.clients[0].result.as_mut().unwrap().answer.seconds = 0.1;
        assert!(
            service.answer(7, 2, current, 1).is_none(),
            "cached contact falls after the earliest defeat"
        );
        current.flight.defeated = true;
        current.flight.lost = true;
        assert!(
            service.answer(7, 2, current, 1).is_none(),
            "permanent defeat is never bucketed"
        );
    }
    fn turning_question(rate: f64, tick: u64) -> Trial {
        let mut q = question();
        let angle = rate * tick as f64 * DT;
        let radius = 300. / rate;
        q.flight.position[0] = radius * (1. - angle.cos());
        q.flight.position[2] = radius * angle.sin();
        q.flight.velocity = [300. * angle.sin(), 0., 300. * angle.cos()];
        q.flight.axis = [angle.sin(), 0., angle.cos()];
        q.target.position[2] -= 250. * tick as f64 * DT;
        q
    }
    #[test]
    fn turning_player_stays_answered_with_thirty_competing_jobs() {
        for degrees in [3f64, 10.] {
            let mut service = Service::default();
            let mut answered = 0;
            for tick in 0..1200 {
                for id in 1..=30 {
                    let mut q = question();
                    q.target.position[2] = 100000. + tick as f64 + id as f64 * 100.;
                    service.request(id << 16, 2, tick, q, 30);
                }
                let q = turning_question(degrees.to_radians(), tick);
                service.request_player(0, 2, tick, q);
                assert!(
                    service.advance(tick, &|_, _| 0.) <= super::super::energy_estimate::STEP_BUDGET
                );
                answered += usize::from(service.player_answer(0, 2, q, tick).is_some());
            }
            assert!(answered >= 1080, "{degrees} degrees/s: {answered}/1200");
            let copy =
                crate::checkpoint::round_trip(&service, &crate::checkpoint::Models::default())
                    .unwrap();
            assert_eq!(service, copy);
        }
    }
    #[test]
    fn turn_projected_player_advice_never_reaches_the_strict_answer() {
        // AI launch permission reads `answer`. Under a sustained turn and
        // competing load, the player's advice must rely on projection on some
        // ticks, and on exactly those ticks the strict answer stays empty.
        let mut service = Service::default();
        let (mut strict, mut projected) = (0, 0);
        for tick in 0..1200 {
            for id in 1..=30 {
                let mut q = question();
                q.target.position[2] = 100000. + tick as f64 + id as f64 * 100.;
                service.request(id << 16, 2, tick, q, 30);
            }
            let q = turning_question(10f64.to_radians(), tick);
            service.request_player(0, 2, tick, q);
            service.advance(tick, &|_, _| 0.);
            let advice = service.player_answer(0, 2, q, tick);
            if let Some(answer) = service.answer(0, 2, q, tick) {
                strict += 1;
                assert_eq!(advice, Some(answer), "tick {tick}");
            } else if advice.is_some() {
                projected += 1;
            }
        }
        assert!(
            strict > 0 && projected > 0,
            "{strict} strict, {projected} projected"
        );
    }
    #[test]
    fn player_turn_projection_never_relaxes_ai_permission_or_accepts_a_reversal() {
        let rate = 10f64.to_radians();
        let old = turning_question(rate, 1);
        let now = turning_question(rate, 61);
        let rates = TurnRates {
            launcher: [0., rate, 0.],
            axis: [0., rate, 0.],
            target: [0.; 3],
            valid: true,
        };
        let result = Published {
            at: 1,
            question: old,
            answer: old.finish(&|_, _| 0.),
        };
        assert!(result.answer.hit);
        assert!(player_compatible(result, rates, now, rates, 61));
        assert!(
            !compatible(old, now, 60),
            "AI must not fire from the turned snapshot"
        );
        let mut reversal = now;
        reversal.flight.axis = reversal.flight.axis.map(|v| -v);
        assert!(!player_compatible(result, rates, reversal, rates, 61));
        let mut wrong = now;
        wrong.target.position[0] += 1000.;
        assert!(!player_compatible(result, rates, wrong, rates, 61));
        let changed = TurnRates {
            target: [0., rate, 0.],
            ..rates
        };
        assert!(!player_compatible(result, rates, now, changed, 61));
        assert!(!player_compatible(result, rates, now, rates, 122));
    }
    #[test]
    fn reserved_player_admission_serves_oldest_questions_across_seats() {
        let mut s = Service::default();
        let q = question();
        for id in 100..348 {
            s.request(id, 2, 0, q, 30);
        }
        s.admit();
        for id in 1..=10 {
            s.request_player(id, 2, 11 - id, q);
        }
        s.admit();
        for id in 3..=10 {
            assert!(s.queue.requests.iter().any(|r| r.id == id));
        }
        assert!(!s.queue.requests.iter().any(|r| r.id == 1 || r.id == 2));
        // A completed early seat refreshing cannot jump the older waiter.
        s.queue.remove(10);
        s.clients
            .iter_mut()
            .find(|c| c.id == 10)
            .unwrap()
            .waiting_since = 20;
        s.admit();
        assert!(s.queue.requests.iter().any(|r| r.id == 2));
        assert!(!s.queue.requests.iter().any(|r| r.id == 10));
    }
    #[test]
    fn a_late_player_has_reserved_admission_and_background_still_progresses() {
        let mut s = Service::default();
        let q = question();
        for id in 1..=300 {
            s.request(id << 16, 2, 0, q, 30);
        }
        s.advance(0, &|_, _| 0.);
        s.request_player(0, 2, 1, q);
        s.advance(1, &|_, _| 0.);
        assert!(
            s.queue.requests.iter().any(|r| r.id == 0)
                || s.clients.iter().any(|c| c.id == 0 && c.result.is_some())
        );
        let before = s
            .queue
            .requests
            .iter()
            .find(|r| r.id != 0)
            .unwrap()
            .trial
            .flight
            .age;
        s.advance(2, &|_, _| 0.);
        assert!(
            s.queue
                .requests
                .iter()
                .any(|r| r.id != 0 && r.trial.flight.age > before)
        );
    }
    #[test]
    fn delayed_launch_answer_projects_both_aircraft_and_range_rejects_reversal() {
        let q = question();
        let mut current = q;
        for i in 0..3 {
            current.flight.position[i] += q.flight.velocity[i] * 2.;
            current.target.position[i] += q.target.velocity[i] * 2.;
        }
        assert!(compatible(q, current, 240));
        assert!(!compatible(q, current, 0));
        let mut service = Service::default();
        service.request_range(1, 2, 0, q);
        service.ranges[0].result = Some(Envelope {
            minimum: 100.,
            maximum: 10000.,
            favorable: None,
            ..Default::default()
        });
        assert!(service.envelope(1, 2, q, 10).is_some());
        current.flight.axis = [0., 0., -1.];
        current.flight.velocity = [0., 0., -300.];
        service.request_range(1, 2, 240, current);
        assert!(service.envelope(1, 2, current, 240).is_none());
        assert!(service.envelope(1, 2, q, 1201).is_none());
    }
    fn run_search(search: &mut RangeSearch, hit: impl Fn(f64) -> bool) {
        for _ in 0..400 {
            if search.done {
                return;
            }
            let h = hit(search.distance);
            search.accept(Answer {
                hit: h,
                seconds: 3.,
                closest: if h { 0. } else { 100. },
                impact_speed: 300.,
            });
        }
        panic!("range search did not finish: {search:?}");
    }
    #[test]
    fn range_search_keeps_each_region_and_advises_the_one_for_the_range() {
        let mut q = question();
        q.body.loft_height = 1000.;
        q.body.loft_range = 6000.;
        let mut search = RangeSearch::new(1, 2, 0, q);
        run_search(&mut search, |d| {
            (1000. ..=5000.).contains(&d) || (6100. ..=6500.).contains(&d)
        });
        let envelope = search.result.unwrap();
        let regions = envelope.regions();
        assert_eq!(regions.len(), 2, "{envelope:?}");
        assert!(regions[0].0 >= 1000. && (4995. ..=5000.).contains(&regions[0].1));
        assert!(
            (6100. ..6105.).contains(&regions[1].0) && (6495. ..=6500.).contains(&regions[1].1)
        );
        // The stored result still names the first region.
        assert_eq!((envelope.minimum, envelope.maximum), regions[0]);
        // Inside the first region: advised and in range.
        assert!(envelope.advised(3000.).contains(3000.));
        // In the gap: the nearest region below, not in range.
        let gap = envelope.advised(5500.);
        assert_eq!((gap.minimum, gap.maximum), regions[0]);
        assert!(!gap.contains(5500.));
        // In the far region: that region, in range, without the first
        // region's favorable band.
        let far = envelope.advised(6300.);
        assert_eq!((far.minimum, far.maximum), regions[1]);
        assert!(far.contains(6300.) && far.favorable.is_none());
        // Beyond every region: the nearest below; below them all: the first.
        let beyond = envelope.advised(9000.);
        assert_eq!((beyond.minimum, beyond.maximum), regions[1]);
        assert!(!beyond.contains(9000.));
        let near = envelope.advised(500.);
        assert_eq!((near.minimum, near.maximum), regions[0]);
        assert!(!near.contains(500.));
    }
    #[test]
    fn a_narrow_gap_at_the_shot_splits_regions_and_respects_decoded_minimum() {
        let mut q = question();
        q.target.position = [
            q.flight.position[0],
            q.flight.position[1],
            q.flight.position[2] + 3500.,
        ];
        let mut search = RangeSearch::new_with_minimum(1, 2, 0, q, 1200.);
        let mut sampled = Vec::new();
        for _ in 0..400 {
            if search.done {
                break;
            }
            assert!(search.distance >= 1200.);
            sampled.push(search.distance);
            let hit = !(3490. ..=3510.).contains(&search.distance) && search.distance <= 6000.;
            search.accept(Answer {
                hit,
                seconds: 1.,
                closest: 0.,
                impact_speed: 300.,
            });
        }
        assert!(search.done);
        let envelope = search.result.unwrap();
        let regions = envelope.regions();
        assert_eq!(regions.len(), 2, "{envelope:?}");
        assert_eq!(regions[0].0, 1200.);
        assert!(regions[0].1 < 3490. && regions[1].0 > 3510., "{regions:?}");
        assert!((5990. ..=6000.).contains(&regions[1].1));
        assert!(envelope.advised(2500.).contains(2500.));
        // The gap point itself advises the near region and is not in range.
        let gap = envelope.advised(3500.);
        assert_eq!(gap.maximum, regions[0].1);
        assert!(!gap.contains(3500.));
        // A far hit beyond the gap is advised and in range.
        assert!(envelope.advised(5000.).contains(5000.));
        assert!(!envelope.advised(1000.).contains(1000.));
    }
    #[test]
    fn published_jobs_survive_checkpoint_and_readers_do_not_advance_them() {
        let q = question();
        let expected = q.finish(&|_, _| 0.);
        let mut service = Service::default();
        service.request(1, 2, 0, q, 30);
        service.advance(1, &|_, _| 0.);
        let mut copy =
            crate::checkpoint::round_trip(&service, &crate::checkpoint::Models::default()).unwrap();
        for tick in 2..200 {
            assert_eq!(
                service.advance(tick, &|_, _| 0.),
                copy.advance(tick, &|_, _| 0.)
            );
        }
        assert_eq!(service, copy);
        for _ in 0..100 {
            assert_eq!(service.answer(1, 2, q, 0).unwrap().answer, expected);
        }
        assert_eq!(service, copy);
        service.request(1, 2, 400, q, 30);
        assert!(
            service.queue.requests.is_empty(),
            "unchanged input reuses its completed result"
        );
        let mut turned = q;
        turned.flight.axis = [1., 0., 0.];
        assert!(service.answer(1, 2, turned, 0).is_none());
        let mut far = q;
        far.target.position[2] += 1000.;
        assert!(service.answer(1, 2, far, 0).is_none());
        assert!(service.answer(1, 3, q, 0).is_none());
    }
    /// A short shot's launch question changes faster than the refresh
    /// interval. The published answer must be renewed as soon as it no
    /// longer matches, or the cue and the AI launch gate stay dark.
    #[test]
    fn saturation_serves_every_client_even_when_early_clients_refresh() {
        let mut service = Service::default();
        let count = super::super::energy_estimate::MAX_REQUESTS + 64;
        let mut seen = vec![false; count];
        for tick in 0..4000 {
            for (id, seen) in seen.iter_mut().enumerate() {
                let mut q = question();
                q.body.life = 30;
                q.target.position[2] += tick as f64;
                service.request(id as u64, id as u32 + 2, tick, q, 1);
                *seen |= service.answer(id as u64, id as u32 + 2, q, tick).is_some();
            }
            service.advance(tick, &|_, _| 0.);
            if seen.iter().all(|v| *v) {
                break;
            }
        }
        assert!(
            seen.iter().all(|v| *v),
            "starved {}",
            seen.iter().filter(|v| !**v).count()
        );
        assert_eq!(service.clients.len(), count);
    }
    #[test]
    fn neighboring_rails_reuse_only_a_recent_compatible_answer_for_the_same_target() {
        let mut q = question();
        q.body.life = 1;
        let mut service = Service::default();
        service.request(1, 2, 0, q, 30);
        service.advance(0, &|_, _| 0.);
        let mut nearby = q;
        nearby.flight.position[0] += 2.;
        service.request(2, 2, 1, nearby, 30);
        assert!(service.answer(2, 2, nearby, 0).is_some());
        assert!(service.queue.requests.is_empty());
        service.request(3, 3, 1, nearby, 30);
        assert!(service.answer(3, 3, nearby, 0).is_none());
        service.request(4, 2, 40, nearby, 30);
        assert!(
            service.answer(4, 2, nearby, 0).is_none(),
            "old geometry is not a fresh neighboring-rail answer"
        );
    }
    #[test]
    fn a_moving_short_range_launch_question_stays_answered() {
        let mut q = question();
        q.target.position[2] = 3000.;
        let mut service = Service::default();
        let mut answered = 0;
        for tick in 0..480u64 {
            let t = tick as f64 * DT;
            let mut current = q;
            for i in 0..3 {
                current.flight.position[i] += q.flight.velocity[i] * t;
                current.target.position[i] += q.target.velocity[i] * t;
            }
            service.request(1, 2, tick, current, 30);
            service.advance(tick, &|_, _| 0.);
            answered += usize::from(service.answer(1, 2, current, tick).is_some());
        }
        assert!(answered >= 432, "answered {answered} of 480 ticks");
    }
    #[test]
    fn a_fast_turn_finishes_cold_search_but_hides_incompatible_ranges() {
        let mut service = Service::default();
        let mut shown = 0;
        for tick in 0..480u64 {
            let heading = (45f64 * tick as f64 * DT).to_radians();
            let mut q = question();
            q.flight.axis = [heading.sin(), 0., heading.cos()];
            q.flight.velocity = q.flight.axis.map(|v| v * 300.);
            service.request_range(1, 2, tick, q);
            assert!(
                service.advance(tick, &|_, _| 0.) <= super::super::energy_estimate::STEP_BUDGET
            );
            shown += usize::from(service.envelope(1, 2, q, tick).is_some());
        }
        assert!(
            shown < 240,
            "stale turning ranges shown {shown} of 480 ticks"
        );
        assert!(
            service
                .ranges
                .iter()
                .any(|r| r.result.is_some() || r.previous.is_some()),
            "cold search never published"
        );
        let mut other = question();
        other.wind[0] = 30.;
        service.request_range(1, 3, 480, other);
        assert!(
            service.envelope(1, 3, other, 0).is_none(),
            "a different target/model must not inherit the old cue"
        );
    }
    #[test]
    fn a_turning_launcher_keeps_its_range_envelope() {
        let mut service = Service::default();
        let mut shown = 0;
        for tick in 0..1200u64 {
            let heading = (3f64 * tick as f64 * DT).to_radians();
            let mut q = question();
            q.flight.axis = [heading.sin(), 0., heading.cos()];
            q.flight.velocity = q.flight.axis.map(|v| v * 300.);
            service.request_range(1, 2, tick, q);
            service.advance(tick, &|_, _| 0.);
            shown += usize::from(service.envelope(1, 2, q, tick).is_some());
        }
        assert!(shown >= 600, "envelope shown {shown} of 1200 ticks");
    }
    /// A straight run restarts the range search every 1000 m of travel. The
    /// last range must stay on the HUD while the replacement search runs.
    #[test]
    fn a_straight_run_keeps_its_range_envelope_through_search_restarts() {
        for closing in [-250., 0., 250.] {
            let mut service = Service::default();
            let mut first = None;
            let mut shown = 0;
            for tick in 0..3600u64 {
                let t = tick as f64 * DT;
                let mut q = question();
                q.flight.position[2] = 300. * t;
                q.target.velocity = [0., 0., closing];
                q.target.position[2] = 25000. + closing * t;
                service.request_range(1, 2, tick, q);
                service.advance(tick, &|_, _| 0.);
                let visible = service.envelope(1, 2, q, tick).is_some();
                first = first.or(visible.then_some(tick));
                shown += usize::from(first.is_some() && visible);
            }
            let first = first.expect("range published");
            assert!(
                shown as u64 >= 3600 - first,
                "closing {closing}: range shown {shown} of {} ticks",
                3600 - first
            );
        }
    }
    #[test]
    fn a_stable_inflight_estimate_ages_with_observed_relative_motion() {
        let mut q = question();
        q.flight.age = 1200;
        q.start_age = 1200;
        let mut current = q;
        current.flight.age += 300;
        for i in 0..3 {
            current.flight.position[i] += q.flight.velocity[i] * 2.5;
            current.target.position[i] += q.target.velocity[i] * 2.5;
        }
        assert!(compatible(q, current, 0));
        current.flight.position[0] += 1000.;
        assert!(!compatible(q, current, 0));
        current = q;
        current.flight.age = 1801;
        assert!(!compatible(q, current, 0));
    }
    #[test]
    fn changing_surface_range_keeps_the_target_above_the_same_ground() {
        let mut q = question();
        q.target.position = [0., 10., 10000.];
        let mut search = RangeSearch::new(1, 2, 0, q);
        search.distance = 20000.;
        let far = search.trial();
        assert_eq!(far.target.position[1], 10.);
        assert!((norm(sub(far.target.position, far.flight.position)) - 20000.).abs() < 1e-8);
    }
    #[test]
    fn range_publishes_maximum_before_the_optional_band_finishes() {
        let mut search = RangeSearch::new(1, 2, 0, question());
        search.open = Some(100.);
        search.previous_hit = Some(true);
        search.phase = 1;
        search.edge = EDGE_FALLING;
        search.iteration = 9;
        search.low = 1000.;
        search.high = 2000.;
        search.resume = 2000.;
        search.distance = 1500.;
        search.accept(Answer {
            hit: true,
            seconds: 3.,
            closest: 0.,
            impact_speed: 700.,
        });
        assert_eq!(search.result.unwrap().maximum, 1500.);
        assert!(!search.done);
        run_search(&mut search, |d| d <= 1500.);
        let envelope = search.result.unwrap();
        assert_eq!(envelope.regions(), &[(100., 1500.)]);
        assert!(envelope.favorable.is_some());
    }
}
