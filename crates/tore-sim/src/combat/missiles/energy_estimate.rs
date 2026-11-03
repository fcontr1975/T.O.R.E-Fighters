//! Bounded, resumable trial flight. Uses the live energy kernel, never wall time.
use super::energy::{Body, DT, Flight, Motor, Observation};
use crate::attitude::{Vector, dot};

/// Global quarter-step units: integration costs four, a terrain sample one.
/// The 4096 integration-equivalent budget is not multiplied by aircraft count.
pub const STEP_BUDGET: usize = 16_384;
/// A cruise scan plus a refined terrain crossing costs at most 75 units.
const MAX_STEP_WORK: usize = 80;
/// Sized for large missions: every AI station with a target, every SAM rail
/// and every missile in flight is one client. The step budget, not this cap,
/// bounds the work per tick; a full table silently starves later requesters.
pub const MAX_REQUESTS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Answer {
    pub hit: bool,
    pub seconds: f64,
    pub closest: f64,
    pub impact_speed: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trial {
    pub body: Body,
    pub flight: Flight,
    pub target: Observation,
    pub wind: Vector,
    pub radius: f64,
    pub arming_age: u32,
    pub start_age: u32,
    pub closest: f64,
    pub answer: Option<Answer>,
}
impl Trial {
    pub fn new(
        body: Body,
        flight: Flight,
        target: Observation,
        radius: f64,
        arming_age: u32,
    ) -> Self {
        Self {
            body,
            flight,
            target,
            wind: [0.; 3],
            radius,
            arming_age,
            start_age: flight.age,
            closest: f64::MAX,
            answer: None,
        }
    }
    /// One fixed simulation step. Terrain and contact use a swept segment.
    pub fn step(&mut self, ground: &impl Fn(f64, f64) -> f64) {
        self.step_over(ground, &|_, _| false);
    }
    pub fn step_over(
        &mut self,
        ground: &impl Fn(f64, f64) -> f64,
        clear: &impl Fn(Vector, Vector) -> bool,
    ) -> usize {
        if self.answer.is_some() {
            return 0;
        }
        let previous = self.flight.position;
        let gap: Vector = std::array::from_fn(|i| self.target.position[i] - previous[i]);
        let forces = self
            .flight
            .step_terrain(self.body, self.wind, Some(self.target), ground);
        for i in 0..3 {
            self.target.position[i] += self.target.velocity[i] * DT;
        }
        let after: Vector =
            std::array::from_fn(|i| self.target.position[i] - self.flight.position[i]);
        let segment: Vector = std::array::from_fn(|i| after[i] - gap[i]);
        let t = (-dot(gap, segment) / dot(segment, segment).max(1e-12)).clamp(0., 1.);
        let nearest: Vector = std::array::from_fn(|i| gap[i] + t * segment[i]);
        self.closest = self.closest.min(dot(nearest, nearest).sqrt());
        let contact = super::super::live::segment_sphere(gap, after, self.radius)
            .filter(|_| self.flight.age > self.arming_age);
        let samples = std::cell::Cell::new(0usize);
        let sampled = |x, z| {
            samples.set(samples.get() + 1);
            ground(x, z)
        };
        let ground_hit = if clear(previous, self.flight.position) {
            None
        } else {
            super::super::live::terrain_hit(previous, self.flight.position, &sampled)
        };
        let hit = !self.flight.defeated
            && contact.is_some_and(|contact| ground_hit.is_none_or(|fraction| contact < fraction));
        if hit || ground_hit.is_some() || self.flight.lost || self.flight.age >= self.body.life {
            self.answer = Some(Answer {
                hit,
                seconds: f64::from(self.flight.age - self.start_age) * DT,
                closest: self.closest,
                impact_speed: dot(self.flight.velocity, self.flight.velocity).sqrt(),
            });
        }
        4 + forces.terrain_samples + samples.get()
    }
    pub fn finish(mut self, ground: &impl Fn(f64, f64) -> f64) -> Answer {
        while self.answer.is_none() {
            self.step(ground);
        }
        self.answer.unwrap()
    }
}

/// Stable request ID is supplied by the owning world (actor/store or missile).
/// A request is immutable while it runs; replacing it explicitly starts anew.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub id: u64,
    pub observed_tick: u64,
    pub trial: Trial,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Queue {
    pub requests: Vec<Request>,
    pub cursor: usize,
    pub refused: u64,
}
impl Queue {
    pub fn submit(&mut self, request: Request) -> bool {
        if let Some(existing) = self.requests.iter_mut().find(|r| r.id == request.id) {
            *existing = request;
        } else {
            if self.requests.len() == MAX_REQUESTS {
                self.refused += 1;
                return false;
            }
            self.requests.push(request);
            self.requests.sort_by_key(|r| r.id);
        }
        true
    }
    pub fn answer(&self, id: u64) -> Option<(u64, Answer)> {
        let request = self.requests.iter().find(|r| r.id == id)?;
        Some((request.observed_tick, request.trial.answer?))
    }
    pub fn remove(&mut self, id: u64) {
        self.requests.retain(|r| r.id != id);
    }
    /// A fixed work limit makes pending-answer timing checkpointable and
    /// independent of CPU speed, cache warmth, frame rate or worker count.
    pub fn advance(&mut self, ground: &impl Fn(f64, f64) -> f64) -> usize {
        self.advance_over(ground, &|_, _| false)
    }
    pub fn advance_over(
        &mut self,
        ground: &impl Fn(f64, f64) -> f64,
        clear: &impl Fn(Vector, Vector) -> bool,
    ) -> usize {
        self.advance_prioritized(ground, clear, &[])
    }
    pub fn advance_prioritized(
        &mut self,
        ground: &impl Fn(f64, f64) -> f64,
        clear: &impl Fn(Vector, Vector) -> bool,
        priority: &[u64],
    ) -> usize {
        let first = if priority.is_empty() {
            0
        } else {
            self.advance_budget(ground, clear, priority, STEP_BUDGET / 4)
        };
        first + self.advance_budget(ground, clear, &[], STEP_BUDGET - first)
    }
    fn advance_budget(
        &mut self,
        ground: &impl Fn(f64, f64) -> f64,
        clear: &impl Fn(Vector, Vector) -> bool,
        only: &[u64],
        budget: usize,
    ) -> usize {
        if self.requests.is_empty() {
            return 0;
        }
        let count = self.requests.len();
        let start = self.cursor % count;
        // Completed answers stay available to the publisher. Build the active
        // ring once, rather than walking every completed slot for every step.
        let mut active = [0usize; MAX_REQUESTS];
        let mut len = 0;
        for offset in 0..count {
            let index = (start + offset) % count;
            if self.requests[index].trial.answer.is_none()
                && (only.is_empty() || only.contains(&self.requests[index].id))
            {
                active[len] = index;
                len += 1;
            }
        }
        let mut at = 0;
        let mut used = 0;
        while used + MAX_STEP_WORK <= budget && len > 0 {
            let index = active[at];
            let request = &mut self.requests[index];
            let spent = request.trial.step_over(ground, clear);
            debug_assert!(spent <= MAX_STEP_WORK);
            used += spent;
            self.cursor = index + 1;
            if request.trial.answer.is_some() {
                active.copy_within(at + 1..len, at);
                len -= 1;
            } else {
                at += 1;
            }
            if len > 0 {
                at %= len;
            }
        }
        used
    }
}

crate::checkpoint_struct!(Motor {
    ticks,
    thrust,
    propellant
});
crate::checkpoint_struct!(Body {
    mass,
    diameter,
    boost,
    sustain,
    ignition,
    life,
    guidance_ticks,
    defeat_mach,
    defeat_ticks,
    structural_g,
    lift_coefficient,
    induced_factor,
    drag_scale,
    cruise_drag,
    minimum_speed,
    navigation_gain,
    loft_height,
    loft_range,
    cruise_feet,
    launch_elevation
});
crate::checkpoint_struct!(Observation { position, velocity });
crate::checkpoint_struct!(Answer {
    hit,
    seconds,
    closest,
    impact_speed
});
crate::checkpoint_struct!(Trial {
    body,
    flight,
    target,
    wind,
    radius,
    arming_age,
    start_age,
    closest,
    answer
});
crate::checkpoint_struct!(Request {
    id,
    observed_tick,
    trial
});
crate::checkpoint_struct!(Queue {
    requests,
    cursor,
    refused
});

#[cfg(test)]
mod tests {
    use super::super::energy::tests::body;
    use super::*;
    fn trial() -> Trial {
        Trial::new(
            body(),
            Flight::launch([0., 5000., 0.], [0., 0., 300.], [0., 0., 1.]),
            Observation {
                position: [1000., 5000., 15000.],
                velocity: [100., 0., -250.],
            },
            30.,
            0,
        )
    }
    #[test]
    fn queued_prediction_matches_unbounded_and_survives_checkpoint() {
        let mut q = Queue::default();
        for id in 0..24 {
            assert!(q.submit(Request {
                id,
                observed_tick: 10,
                trial: trial()
            }));
        }
        assert!((1..=STEP_BUDGET).contains(&q.advance(&|_, _| 0.)));
        let mut q =
            crate::checkpoint::round_trip(&q, &crate::checkpoint::Models::default()).unwrap();
        let expected = trial().finish(&|_, _| 0.);
        assert!(expected.hit, "{expected:?}");
        for _ in 0..5000 {
            assert!(q.advance(&|_, _| 0.) <= STEP_BUDGET);
        }
        for id in 0..24 {
            assert_eq!(q.answer(id), Some((10, expected)));
        }
        assert_eq!(q.advance(&|_, _| 0.), 0);
    }
    #[test]
    fn a_tail_chase_runs_out_of_energy_sooner_in_dense_air() {
        let shot = |altitude| {
            Trial::new(
                body(),
                Flight::launch([0., altitude, 0.], [0., 0., 300.], [0., 0., 1.]),
                Observation {
                    position: [0., altitude, 20000.],
                    velocity: [0., 0., 250.],
                },
                30.,
                0,
            )
            .finish(&|_, _| 0.)
        };
        assert!(!shot(1500.).hit);
        assert!(shot(10668.).hit);
    }
    #[test]
    fn proven_clear_terrain_shortcut_keeps_the_same_trajectory_and_contact() {
        let expected = trial().finish(&|_, _| 0.);
        let mut q = trial();
        while q.answer.is_none() {
            q.step_over(&|_, _| 0., &|a, b| a[1] > 0. && b[1] > 0.);
        }
        assert_eq!(q.answer, Some(expected));
    }
    #[test]
    fn saturated_queue_refuses_without_dropping_existing_work() {
        let mut q = Queue::default();
        for id in 0..MAX_REQUESTS {
            assert!(q.submit(Request {
                id: id as u64,
                observed_tick: 0,
                trial: trial()
            }));
        }
        assert!(!q.submit(Request {
            id: 999,
            observed_tick: 0,
            trial: trial()
        }));
        assert_eq!(q.requests.len(), MAX_REQUESTS);
    }
}
