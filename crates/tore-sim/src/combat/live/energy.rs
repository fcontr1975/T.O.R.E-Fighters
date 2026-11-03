//! Bounded prediction integration. Frame readers only consume published work.
use super::*;
use missiles::{energy::FT, energy_estimate::Trial, prediction};

fn key(owner: u32, station: usize) -> u64 {
    (u64::from(owner) << 16) | (station.min(65535) as u64)
}

fn raw_observation(
    own: &Ownship,
    launcher: Launcher,
    targets: &[Target],
) -> Option<seeker::Observation> {
    if let Some(point) = own.guided_sight.and_then(|s| s.aim) {
        let delta = missiles::sub(point, launcher.position);
        return Some(seeker::Observation {
            id: u32::MAX,
            position: point,
            velocity: own
                .guided_sight
                .filter(|s| s.kind == Guidance::Command)
                .and_then(|_| own.cockpit.target_object())
                .and_then(|id| targets.iter().find(|t| t.id == id))
                .map_or([0.; 3], |t| t.velocity),
            quality: 1.,
            range: missiles::length(delta),
            off_axis: 0.,
        });
    }
    if own.launch_mode == LaunchMode::Boresight {
        return own.bore_observation;
    }
    own.mounted.observation.or_else(|| {
        let id = own.designated()?;
        let o = own.sensors.observation(id)?;
        let delta = missiles::sub(o.position, launcher.position);
        Some(seeker::Observation {
            id,
            position: o.position,
            velocity: o.velocity,
            quality: 1.,
            range: missiles::length(delta),
            off_axis: dot(unit(delta), launcher.basis.forward)
                .clamp(-1., 1.)
                .acos(),
        })
    })
}
fn prediction_radius(w: &Weapon) -> f64 {
    if super::super::guided::kind(w).is_some() {
        super::super::guided::HIT_FT
    } else {
        f64::from(w.damage.fuze_radius.max(0)) + AIRCRAFT_RADIUS_FT
    }
}
fn powered(w: &Weapon) -> bool {
    w.movement.fuel_t > w.movement.ignite_t
}
impl State {
    fn prediction_request_start(&self) -> Option<(std::time::Instant, Option<u64>)> {
        self.measure_prediction_cost.then(|| {
            (
                std::time::Instant::now(),
                self.prediction_cpu_clock.and_then(|clock| clock()),
            )
        })
    }
    fn prediction_request_finish(&mut self, start: Option<(std::time::Instant, Option<u64>)>) {
        if let Some((wall, cpu)) = start {
            self.prediction_request_ns +=
                wall.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            if let Some(elapsed) = cpu
                .zip(self.prediction_cpu_clock.and_then(|clock| clock()))
                .and_then(|(a, b)| b.checked_sub(a))
            {
                self.prediction_request_cpu_ns += elapsed;
            }
        }
    }

    /// Motion of a designated command target, already owned by the strike cockpit.
    pub fn guided_point_velocity(&self, owner: u32, kind: Guidance) -> Vector {
        if kind != Guidance::Command {
            return [0.; 3];
        }
        self.strike_cockpit(owner)
            .and_then(|c| c.target_object())
            .and_then(|id| self.targets.iter().find(|t| t.id == id))
            .map_or([0.; 3], |t| t.velocity)
    }

    /// Optional model-based timing for a threat already perceived by the caller.
    /// Pending and defeated missiles have no answer. This does not disclose new threats.
    pub fn missile_time_to_impact(&self, projectile: u32, target: u32) -> Option<f64> {
        let p = self
            .projectiles
            .iter()
            .find(|p| p.id == projectile && p.target == Some(target) && p.threatening())?;
        let flight = p.guidance.as_ref()?;
        flight.solution.map(|s| s.seconds)
    }

    pub fn request_energy_range(
        &mut self,
        owner: u32,
        station: usize,
        w: &Weapon,
        launcher: Launcher,
        observed: seeker::Observation,
    ) {
        if self.weapon_rules != Rules::Energy || !powered(w) {
            return;
        }
        let measured = self.prediction_request_start();
        let question = prediction::launch_question(
            w,
            launcher,
            observed,
            prediction_radius(w),
            self.devices.wind,
        );
        self.energy_predictions.request_range_with_minimum(
            key(owner, station),
            observed.id,
            self.tick,
            question,
            f64::from(w.seeker.zones[1].minimum_range) * FT,
        );
        self.prediction_request_finish(measured);
    }
    /// Admission starvation means a requested point has waited over ten
    /// simulated seconds without an active slot. Completion latency is separate.
    pub fn energy_prediction_admission_stats(&self) -> (u64, usize, u64) {
        let service = &self.energy_predictions;
        let waits = service
            .clients
            .iter()
            .filter(|c| c.pending && !service.queue.requests.iter().any(|r| r.id == c.id))
            .map(|c| self.tick.saturating_sub(c.waiting_since))
            .chain(
                service
                    .ranges
                    .iter()
                    .filter(|r| !r.done && !service.queue.requests.iter().any(|job| job.id == r.id))
                    .map(|r| self.tick.saturating_sub(r.waiting_since)),
            );
        (
            service.queue.refused,
            waits.clone().filter(|wait| *wait > 1200).count(),
            waits.max().unwrap_or(0),
        )
    }
    pub fn energy_prediction_counts(&self) -> (usize, usize, usize) {
        (
            self.energy_predictions.queue.requests.len(),
            self.energy_predictions.clients.len(),
            self.energy_predictions
                .ranges
                .iter()
                .filter(|r| r.result.is_some() || r.previous.is_some())
                .count(),
        )
    }
    pub(super) fn energy_guided_output(
        &self,
        owner: u32,
        station: usize,
        w: &Weapon,
        launcher: Launcher,
        mut sight: super::super::guided::GuidedSight,
    ) -> super::super::guided::GuidedSight {
        if self.weapon_rules != Rules::Energy || !powered(w) {
            return sight;
        }
        sight.energy_intercept = Some(false);
        sight.reach = None;
        let Some(point) = sight.aim else {
            sight.in_reach = false;
            return sight;
        };
        let observed = seeker::Observation {
            id: u32::MAX,
            position: point,
            velocity: self.guided_point_velocity(owner, sight.kind),
            quality: 1.,
            range: missiles::length(missiles::sub(point, launcher.position)),
            off_axis: 0.,
        };
        let question = prediction::launch_question(
            w,
            launcher,
            observed,
            prediction_radius(w),
            self.devices.wind,
        );
        let envelope = self
            .energy_predictions
            .envelope(key(owner, station), u32::MAX, question, self.tick)
            .map(|e| e.advised(observed.range * FT));
        // The AI's strike gate reads its exact-point answer only. AI cockpits
        // request no range search, so requiring envelope membership here
        // would leave every powered guided store permanently not due.
        let hit = self
            .energy_predictions
            .answer(key(owner, station), u32::MAX, question, self.tick)
            .is_some_and(|r| r.answer.hit);
        sight.energy_intercept = Some(hit);
        sight.in_reach &= hit;
        sight.reach = envelope.map(|e| super::super::guided::Reach {
            minimum_ft: e.minimum / FT,
            maximum_ft: e.maximum / FT,
        });
        sight
    }
    pub fn request_energy_solution(
        &mut self,
        owner: u32,
        station: usize,
        w: &Weapon,
        launcher: Launcher,
        observed: seeker::Observation,
    ) {
        if self.weapon_rules != Rules::Energy || !powered(w) {
            return;
        }
        let measured = self.prediction_request_start();
        let radius = prediction_radius(w);
        let question =
            prediction::launch_question(w, launcher, observed, radius, self.devices.wind);
        self.energy_predictions
            .request(key(owner, station), observed.id, self.tick, question, 30);
        if self.measure_prediction_cost {
            let result = self.energy_predictions.answer(
                key(owner, station),
                observed.id,
                question,
                self.tick,
            );
            let index = result.map_or(2, |r| usize::from(!r.answer.hit));
            self.prediction_gate_samples[index] += 1;
        }
        self.prediction_request_finish(measured);
    }
    /// One launch question flown to its answer at once, over `ground` (the
    /// terrain height at a point, feet), with `wind` (world feet per
    /// second): the same question, hit radius and
    /// kernel the prediction service answers for [`Self::energy_solution`],
    /// without its queue. `None` for a store with no motor, which the
    /// energy rules never launch. For callers that bound an envelope ahead
    /// of time (a helicopter's threat rings), not for the per-tick gates.
    pub fn energy_trial(
        w: &Weapon,
        launcher: Launcher,
        observed: seeker::Observation,
        wind: Vector,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> Option<missiles::energy_estimate::Answer> {
        if !powered(w) {
            return None;
        }
        let question =
            prediction::launch_question(w, launcher, observed, prediction_radius(w), wind);
        Some(question.finish(&|x, z| ground(x / FT, z / FT) * FT))
    }
    /// Player advisory only. AI employment reads `energy_solution` instead.
    pub(super) fn player_energy_answer(
        &self,
        owner: u32,
        station: usize,
        w: &Weapon,
        launcher: Launcher,
        observed: seeker::Observation,
    ) -> Option<missiles::energy_estimate::Answer> {
        if self.weapon_rules != Rules::Energy || !powered(w) {
            return None;
        }
        let question = prediction::launch_question(
            w,
            launcher,
            observed,
            prediction_radius(w),
            self.devices.wind,
        );
        self.energy_predictions
            .player_answer(key(owner, station), observed.id, question, self.tick)
            .map(|p| p.answer)
    }
    pub fn energy_solution(
        &self,
        owner: u32,
        station: usize,
        w: &Weapon,
        launcher: Launcher,
        observed: seeker::Observation,
    ) -> Option<missiles::Solution> {
        if !powered(w) {
            return None;
        }
        let radius = prediction_radius(w);
        let question =
            prediction::launch_question(w, launcher, observed, radius, self.devices.wind);
        let result = self.energy_predictions.answer(
            key(owner, station),
            observed.id,
            question,
            self.tick,
        )?;
        if !result.answer.hit {
            return None;
        }
        Some(missiles::Solution {
            seconds: result.answer.seconds,
            point: std::array::from_fn(|i| {
                observed.position[i] + observed.velocity[i] * result.answer.seconds
            }),
        })
    }
    pub(super) fn advance_energy_predictions(
        &mut self,
        rows: &[OwnRow],
        ground: &impl Fn(f64, f64) -> f64,
    ) {
        let cpu_start = if self.measure_prediction_cost {
            self.prediction_cpu_clock.and_then(|clock| clock())
        } else {
            None
        };
        let started = self.measure_prediction_cost.then(std::time::Instant::now);
        for row in rows {
            let own = &self.ownships[row.index];
            if !own.armed {
                continue;
            }
            let Some(station) = own.config.stations.get(own.selected) else {
                continue;
            };
            let w = &station.weapon;
            if !powered(w)
                || (missiles::Profile::for_weapon(w).is_none()
                    && super::super::guided::kind(w).is_none())
            {
                continue;
            }
            let Some(observed) = raw_observation(own, row.launcher, &self.targets) else {
                continue;
            };
            let radius = prediction_radius(w);
            let question =
                prediction::launch_question(w, row.launcher, observed, radius, self.devices.wind);
            let id = key(own.aircraft, own.selected);
            self.energy_predictions
                .request_player(id, observed.id, self.tick, question);
            self.energy_predictions.request_range_with_minimum(
                id,
                observed.id,
                self.tick,
                question,
                f64::from(w.seeker.zones[1].minimum_range) * FT,
            );
        }
        for projectile in &self.projectiles {
            let Some(energy) = projectile.motion.and_then(|m| m.energy) else {
                continue;
            };
            let Some(target) = energy.target.filter(|_| {
                energy.observed && energy.decoy.pursuit.is_none() && !energy.flight.lost
            }) else {
                continue;
            };
            let id = projectile
                .target
                .or_else(|| self.guided.get(&projectile.id).map(|_| u32::MAX));
            let Some(id) = id else {
                continue;
            };
            let w = self.weapon(projectile);
            let mut question = Trial::new(
                energy.body,
                energy.flight,
                target,
                (prediction_radius(w)) * FT,
                u32::from(w.damage.fuze_arm_t) * 30,
            );
            question.wind = energy.wind;
            self.energy_predictions.request(
                prediction::MISSILE_BIT | u64::from(projectile.id),
                id,
                self.tick,
                question,
                15,
            );
        }
        let ceiling = self.ground_ceiling.as_deref();
        self.energy_predictions.advance_over(
            self.tick,
            &|x, z| ground(x / FT, z / FT) * FT,
            &|a, b| ceiling.is_some_and(|c| c.clear_above(a.map(|v| v / FT), b.map(|v| v / FT))),
        );
        for row in rows {
            let own = &mut self.ownships[row.index];
            own.energy_hit = None;
            own.range_estimate = None;
            if !own.armed {
                continue;
            }
            let Some(w) = own.config.stations.get(own.selected).map(|s| &s.weapon) else {
                continue;
            };
            if !powered(w)
                || (missiles::Profile::for_weapon(w).is_none()
                    && super::super::guided::kind(w).is_none())
            {
                continue;
            }
            let Some(observed) = raw_observation(own, row.launcher, &self.targets) else {
                continue;
            };
            let question = prediction::launch_question(
                w,
                row.launcher,
                observed,
                prediction_radius(w),
                self.devices.wind,
            );
            let id = key(own.aircraft, own.selected);
            let answer =
                self.energy_predictions
                    .player_answer(id, observed.id, question, self.tick);
            if self.measure_prediction_cost && own.aircraft == 0 {
                self.prediction_player_samples[answer.map_or(2, |r| usize::from(!r.answer.hit))] +=
                    1;
            }
            own.energy_hit = answer.filter(|r| r.answer.hit).map(|_| observed.id);
            // One contiguous region advised for the present range: the one
            // containing it, else the nearest below, else the first.
            let advised = self
                .energy_predictions
                .envelope(id, observed.id, question, self.tick)
                .map(|e| e.advised(observed.range * FT));
            if let Some(sight) = &mut own.guided_sight {
                let hit = own.energy_hit == Some(observed.id)
                    && advised.is_some_and(|e| e.contains(observed.range * FT));
                sight.energy_intercept = Some(hit);
                sight.in_reach &= hit;
                sight.reach = advised.map(|e| super::super::guided::Reach {
                    minimum_ft: e.minimum / FT,
                    maximum_ft: e.maximum / FT,
                });
            }
            if let Some(envelope) = advised {
                own.range_estimate = Some(RangeEstimate {
                    station: own.selected,
                    target: observed.id,
                    mode: own.launch_mode,
                    minimum: envelope.minimum / FT,
                    maximum: envelope.maximum / FT,
                    favorable: envelope
                        .favorable
                        .map(|(minimum, maximum)| missiles::FiringBand {
                            minimum: minimum / FT,
                            maximum: maximum / FT,
                        }),
                });
            }
        }
        for projectile in &mut self.projectiles {
            let Some(energy) = projectile.motion.and_then(|m| m.energy) else {
                continue;
            };
            let guided = self.guided.get_mut(&projectile.id);
            let id = projectile
                .target
                .or_else(|| guided.as_ref().map(|_| u32::MAX));
            let result = energy.target.zip(id).and_then(|(target, id)| {
                if !energy.observed || energy.decoy.pursuit.is_some() || energy.flight.lost {
                    return None;
                }
                let client = self
                    .energy_predictions
                    .clients
                    .iter()
                    .find(|c| c.id == (prediction::MISSILE_BIT | u64::from(projectile.id)))?;
                let mut question = client.question;
                question.flight = energy.flight;
                question.target = target;
                question.wind = energy.wind;
                let result = self
                    .energy_predictions
                    .answer(client.id, id, question, self.tick)
                    .filter(|r| r.answer.hit)?;
                Some(missiles::Solution {
                    seconds: result.answer.seconds,
                    point: std::array::from_fn(|i| {
                        (target.position[i] + target.velocity[i] * result.answer.seconds) / FT
                    }),
                })
            });
            if let Some(flight) = &mut projectile.guidance {
                flight.solution = result;
            }
            if let Some(flight) = guided {
                flight.seconds = result.map(|r| r.seconds);
            }
        }
        if let Some(started) = started {
            self.prediction_cost_ns = started.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
                + std::mem::take(&mut self.prediction_request_ns);
            self.prediction_cpu_cost_ns = cpu_start.and_then(|from| {
                self.prediction_cpu_clock
                    .and_then(|clock| clock())
                    .and_then(|to| to.checked_sub(from))
                    .map(|elapsed| elapsed + std::mem::take(&mut self.prediction_request_cpu_ns))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn weapon(source: &str, signature: u8) -> Weapon {
        let mut w = super::super::tests::fixture(true)
            .own()
            .configuration()
            .stations[0]
            .weapon
            .clone();
        w.source = source.into();
        w.flags = 0x1204f;
        w.seeker.signature = signature;
        w.seeker.all_aspect = 1;
        w.seeker.chaff_flare_chance = 50;
        w.damage.fuze_arm_t = 0;
        w.damage.fuze_radius = 100;
        w
    }
    fn projectile(w: &Weapon, target: &Target) -> Projectile {
        let position = [0., target.position[1], 0.];
        let velocity = [0., 0., 300. / FT];
        let observation = seeker::Observation {
            id: target.id,
            position: target.position,
            velocity: target.velocity,
            quality: 1.,
            off_axis: 0.,
            range: missiles::length(missiles::sub(target.position, position)),
        };
        let profile = missiles::Profile::for_weapon(w).unwrap();
        let mut guidance =
            Flight::from_supported_launch(profile, LaunchMode::Cued, observation, position);
        guidance.enabled = true;
        guidance.seeker.acquired = true;
        guidance.seeker.observation = Some(observation);
        guidance.seeker.status = Status::Locked;
        let mut motion = Motion::launch_rules(w, velocity, position[1], Rules::Energy);
        motion.prepare(position, [0., 0., 1.], 0, [0.; 3]);
        Projectile {
            id: 7,
            owner: 0,
            weapon: Some(w.clone()),
            guidance: Some(guidance),
            motion: Some(motion),
            guidance_ticks: Some(profile.guidance_ticks),
            age: 0,
            incoming: None,
            station: 0,
            position,
            previous: position,
            direction: [0., 0., 1.],
            speed_f8: (300. / FT * 256.) as i32,
            launched_t: 0,
            target: Some(target.id),
            fall: FallState::default(),
            gun_round: None,
            tracer: false,
        }
    }
    #[test]
    fn an_ai_guided_sight_is_due_on_its_point_answer_without_a_range_search() {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        let w = weapon("R27ER.JT", 2);
        let launcher = Launcher {
            position: [0., 300., 0.],
            basis: Basis::new(0., 0., 0.),
            velocity: [0., 0., 30. / FT],
            speed_fps: 30. / FT,
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: Default::default(),
        };
        let point = [0., 0., 3000. / FT];
        let observed = seeker::Observation {
            id: u32::MAX,
            position: point,
            velocity: [0.; 3],
            quality: 1.,
            off_axis: 0.,
            range: missiles::length(missiles::sub(point, launcher.position)),
        };
        state.request_energy_solution(7, 0, &w, launcher, observed);
        let question = state.energy_predictions.clients[0].question;
        assert!(state.energy_predictions.ranges.is_empty());
        let sight = crate::combat::guided::GuidedSight {
            energy_intercept: None,
            kind: Guidance::Laser,
            aim: Some(point),
            buddy: false,
            ground_range_ft: Some(3000. / FT),
            slant_range_ft: Some(observed.range),
            reach: None,
            in_reach: true,
            advisory: None,
            level: None,
        };
        for hit in [false, true] {
            state.energy_predictions.clients[0].result = Some(prediction::Published {
                at: 0,
                question,
                answer: missiles::energy_estimate::Answer {
                    hit,
                    seconds: 20.,
                    closest: if hit { 0. } else { 1000. },
                    impact_speed: 200.,
                },
            });
            let out = state.energy_guided_output(7, 0, &w, launcher, sight);
            assert_eq!(out.energy_intercept, Some(hit));
            assert_eq!(super::super::guided_due(&out), hit);
        }
    }
    #[test]
    fn an_ai_point_miss_cannot_be_overruled_by_a_range_envelope() {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        let w = weapon("R27ER.JT", 2);
        let launcher = Launcher {
            position: [0., 5000., 0.],
            basis: Basis::new(0., 0., 0.),
            velocity: [0., 0., 300. / FT],
            speed_fps: 300. / FT,
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: Default::default(),
        };
        let observed = seeker::Observation {
            id: 42,
            position: [0., 5000., 35000. / FT],
            velocity: [0., 0., -250. / FT],
            quality: 1.,
            off_axis: 0.,
            range: 35000. / FT,
        };
        state.request_energy_solution(7, 0, &w, launcher, observed);
        let question = state.energy_predictions.clients[0].question;
        let mut range = prediction::RangeSearch::new(key(7, 0), 42, 0, question);
        range.result = Some(prediction::Envelope {
            minimum: 100.,
            maximum: 50000.,
            favorable: None,
            ..Default::default()
        });
        state.energy_predictions.ranges.push(range);
        for hit in [false, true, false] {
            state.energy_predictions.clients[0].result = Some(prediction::Published {
                at: 0,
                question,
                answer: missiles::energy_estimate::Answer {
                    hit,
                    seconds: 30.,
                    closest: if hit { 0. } else { 1000. },
                    impact_speed: 500.,
                },
            });
            assert_eq!(
                state
                    .energy_solution(7, 0, &w, launcher, observed)
                    .is_some(),
                hit
            );
        }
    }
    #[test]
    fn fitted_short_ir_has_no_empty_slow_shooter_coaltitude_sectors() {
        let mut body = missiles::energy_profile::body(&weapon("AA8.JT", 2));
        body.ignition = 0;
        for altitude in [500., 5000., 15000., 25000., 35000., 45000.] {
            for angle in [45_f64, 90., 135.] {
                assert!(
                    synthetic_sector_has_hit(
                        body,
                        altitude,
                        altitude,
                        angle,
                        200.,
                        &[1000., 2000., 3000., 4000., 5000., 6000.]
                    ),
                    "{altitude} ft, {angle} degrees"
                );
            }
        }
        assert_eq!(body.guidance_ticks, 2400);
        assert_eq!(body.defeat_mach, 1.);
    }

    #[test]
    fn fitted_medium_radar_stores_reach_every_sampled_climbing_sector() {
        for source in ["AA12.JT", "AEMP1.JT"] {
            let mut body = missiles::energy_profile::body(&weapon(source, 3));
            body.ignition = 0;
            for speed in [200., 300.] {
                for angle in [0., 45., 90., 135., 180.] {
                    assert!(
                        synthetic_sector_has_hit(
                            body,
                            5000.,
                            30000.,
                            angle,
                            speed,
                            &[3000., 5000., 8000., 12000., 18000., 25000.]
                        ),
                        "{source}, {speed} m/s, {angle} degrees"
                    );
                }
            }
            assert_eq!(body.guidance_ticks, 8400);
            assert_eq!(body.defeat_mach, 1.);
        }
    }

    fn synthetic_sector_has_hit(
        body: missiles::energy::Body,
        from: f64,
        to: f64,
        angle: f64,
        speed: f64,
        ranges: &[f64],
    ) -> bool {
        ranges.iter().any(|range| {
            let trial = Trial::new(
                body,
                missiles::energy::Flight::launch(
                    [0., from * FT, 0.],
                    [0., 0., speed],
                    [0., 0., 1.],
                ),
                missiles::energy::Observation {
                    position: [0., to * FT, *range],
                    velocity: [
                        250. * angle.to_radians().sin(),
                        0.,
                        -250. * angle.to_radians().cos(),
                    ],
                },
                40.,
                60,
            );
            trial.finish(&|_, _| 0.).hit
        })
    }
    #[test]
    fn inert_stores_clear_the_warning_picture_on_the_defeat_tick() {
        for passed_decoy in [false, true] {
            let mut state = super::super::tests::fixture(true);
            state.weapon_rules = Rules::Energy;
            state.energy_predictions_enabled = false;
            state.targets.clear();
            state.own_mut().config.sensors.signature.radar = 100.;
            let target = super::super::tests::target(0, [0., 1000. / FT, -2000. / FT], 10000, 0x80);
            let w = weapon("AIM120.JT", 3);
            let mut p = projectile(&w, &target);
            p.owner = 99;
            p.incoming = Some(0);
            p.previous = p.position;
            p.direction = [0., 0., -1.];
            let f = p.guidance.as_mut().unwrap();
            f.enabled = true;
            f.seeker.status = Status::Pitbull;
            f.seeker.candidate = Some(target.id);
            f.seeker.dwell = missiles::DWELL;
            p.motion.as_mut().unwrap().velocity = [0., 0., -200. / FT];
            let e = p.motion.as_mut().unwrap().energy.as_mut().unwrap();
            e.tracking.head = Some(p.direction);
            e.flight.position = p.position.map(|v| v * FT);
            e.flight.velocity = [0., 0., -200.];
            e.flight.axis = p.direction;
            e.flight.age = e.body.ignition + e.body.boost.ticks + e.body.sustain.ticks;
            e.flight.subsonic_ticks = 28;
            p.age = u64::from(e.flight.age);
            state.projectiles = vec![p];
            let l = Launcher {
                position: target.position,
                basis: Basis::new(0., 0., 0.),
                velocity: [0.; 3],
                speed_fps: 0.,
                bay_ready: true,
                radar_power: true,
                radar: true,
                jammer: false,
                alive: true,
                body_present: true,
                controls: Default::default(),
            };
            let input = [OwnshipInput {
                aircraft: 0,
                held: false,
                launcher: l,
            }];
            state.step_surface(&input, |_, _| 0., |_, _| false);
            assert!(
                state
                    .own()
                    .missile_threats
                    .records()
                    .any(|r| r.targeting_receiver)
            );
            assert!(state.projectiles[0].threatening());
            assert!(state.ended_missile_threats().is_empty());
            if passed_decoy {
                let e = state.projectiles[0]
                    .motion
                    .as_mut()
                    .unwrap()
                    .energy
                    .as_mut()
                    .unwrap();
                e.flight.subsonic_ticks = 0;
                e.decoy.pursuit = Some(missiles::decoys::Pursuit {
                    device: missiles::decoys::Device::release(
                        1,
                        0,
                        missiles::decoys::Kind::Chaff,
                        1,
                        e.flight.position,
                        [0.; 3],
                    ),
                    passed: true,
                });
            }
            state.step_surface(&input, |_, _| 0., |_, _| false);
            assert_eq!(state.projectiles.len(), 1, "the inert body must remain");
            assert!(!state.projectiles[0].threatening());
            assert_eq!(
                state.own().missile_threats.records().count(),
                0,
                "no grace period for an inert store"
            );
            assert!(state.missile_snapshots(&[(0, l)]).is_empty());
            assert_eq!(state.ended_missile_threats(), &[7]);
            let models = crate::checkpoint::Models::default();
            let saved = crate::checkpoint::to_bytes(&state, &models).unwrap();
            let mut restored: State = crate::checkpoint::from_bytes(&saved, &models).unwrap();
            assert_eq!(restored.ended_missile_threats(), &[7]);
            restored.projectiles.clear();
            restored.step_surface(&input, |_, _| 0., |_, _| false);
            assert!(restored.ended_missile_threats().is_empty());
            assert_eq!(state.missile_time_to_impact(7, 0), None);
        }
    }

    // Start with 28/29/30 prior low-Mach samples. Contact in the next tick
    // is respectively before/at/after defeat. Sweep direct and proximity cases.
    fn defeat_contact(prior: u32, offset: f64, ground_contact: bool) -> (bool, bool, bool) {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        state.energy_predictions_enabled = false;
        let mut target = super::super::tests::target(
            100,
            [
                offset,
                if ground_contact { 0. } else { 1000. / FT },
                1. / FT,
            ],
            10000,
            0x80,
        );
        target.radius = 1. / FT;
        target.side = Side(2);
        let mut w = weapon("AIM120.JT", 3);
        w.damage.fuze_radius = 10;
        let mut p = projectile(&w, &target);
        p.guidance = None;
        p.target = None;
        p.position[1] = if ground_contact {
            0.01
        } else {
            target.position[1]
        };
        let e = p.motion.as_mut().unwrap().energy.as_mut().unwrap();
        e.flight.position = p.position.map(|v| v * FT);
        e.flight.velocity = [0., if ground_contact { -200. } else { 0. }, 200.];
        e.flight.age = e.body.ignition + e.body.boost.ticks + e.body.sustain.ticks;
        e.flight.subsonic_ticks = prior;
        e.flight.defeated = prior >= e.body.defeat_ticks;
        e.flight.lost = e.flight.defeated;
        p.age = u64::from(e.flight.age);
        let mut trial = Trial::new(
            e.body,
            e.flight,
            missiles::energy::Observation {
                position: target.position.map(|v| v * FT),
                velocity: [0.; 3],
            },
            (target.radius + f64::from(w.damage.fuze_radius)) * FT,
            0,
        );
        trial.step(&|_, _| 0.);
        state.targets = vec![target];
        state.projectiles = vec![p];
        let events = state.step_surface(&[], |_, _| 0., |_, _| false);
        let hit = events.iter().any(|e| matches!(e, Event::Hit(100)));
        if prior >= 29 {
            assert_eq!(
                state.targets[0].hp, 10000,
                "inert body caused direct or collateral damage"
            );
            assert!(!hit);
            assert_eq!(state.ended_missile_threats(), &[7]);
            if ground_contact {
                assert!(state.projectiles.is_empty());
            } else {
                assert!(
                    state.projectiles[0]
                        .motion
                        .unwrap()
                        .energy
                        .unwrap()
                        .flight
                        .defeated
                );
            }
        }
        (
            hit,
            trial.answer.is_some_and(|a| a.hit),
            state.targets[0].hp < 10000,
        )
    }

    #[test]
    fn defeat_contact_order_matches_forecast_and_disables_direct_and_proximity_damage() {
        for offset in [0., 8.] {
            for prior in [28, 29, 30] {
                let (live, forecast, damaged) = defeat_contact(prior, offset, false);
                assert_eq!(live, prior == 28, "{prior} {offset}");
                assert_eq!(live, forecast);
                assert_eq!(live, damaged);
            }
        }
    }

    #[test]
    fn defeated_ground_contact_cannot_burst_or_damage_collateral() {
        for prior in [29, 30] {
            defeat_contact(prior, 20., true);
        }
    }

    #[test]
    fn defeat_contacts_are_identical_in_serial_and_parallel() {
        let serial: Vec<_> = (28..=30).map(|n| defeat_contact(n, 0., false)).collect();
        let parallel: Vec<_> = std::thread::scope(|scope| {
            let jobs: Vec<_> = (28..=30)
                .map(|n| scope.spawn(move || defeat_contact(n, 0., false)))
                .collect();
            jobs.into_iter().map(|j| j.join().unwrap()).collect()
        });
        assert_eq!(serial, parallel);
    }

    // Golden behaviour, separate from the input/revision identity. Quantize
    // sampled metres and seconds to avoid insignificant platform roundoff.
    // A failing trace requires investigation and a semantic revision bump,
    // then review of the new (identity, trace) pair, never just a new digest.
    fn behavior_trace(navigation_multiplier: f64, defeat_multiplier: f64) -> u64 {
        let mut hash = 0xcbf29ce484222325u64;
        let mut add = |value: f64| {
            for byte in ((value * 100.).round() as i64).to_le_bytes() {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        };
        for source in ["AIM9M.JT", "AIM120.JT", "AIM7.JT"] {
            let w = weapon(source, if source == "AIM9M.JT" { 2 } else { 3 });
            for altitude in [500., 5000., 35000.] {
                let mut body = missiles::energy_profile::body(&w);
                body.navigation_gain *= navigation_multiplier;
                let mut trial = Trial::new(
                    body,
                    missiles::energy::Flight::launch(
                        [0., altitude * FT, 0.],
                        [0., 0., 300.],
                        [0., 0., 1.],
                    ),
                    missiles::energy::Observation {
                        position: [300., altitude * FT, 4000.],
                        velocity: [80., 0., -150.],
                    },
                    10.,
                    12,
                );
                for tick in 0..2400 {
                    trial.step(&|_, _| 0.);
                    if tick % 120 == 0 {
                        for value in trial
                            .flight
                            .position
                            .into_iter()
                            .chain(trial.flight.velocity)
                        {
                            add(value);
                        }
                    }
                    if let Some(a) = trial.answer {
                        add(f64::from(a.hit));
                        add(a.seconds);
                        add(a.closest);
                        break;
                    }
                }
            }
        }
        // Defeat-policy trace: mid-coast, reset, boundary, and inert contact.
        let mut b = missiles::energy_profile::body(&weapon("AIM120.JT", 3));
        b.defeat_mach *= defeat_multiplier;
        let mut f = missiles::energy::Flight::launch([0., 1500., 0.], [0., 0., 200.], [0., 0., 1.]);
        f.age = b.ignition + b.boost.ticks + b.sustain.ticks;
        for tick in 0..65 {
            if tick == 15 {
                f.velocity = [0., 0., 500.];
            }
            if tick == 16 {
                f.velocity = [0., 0., 200.];
            }
            f.step(b, [0.; 3], None);
            add(f64::from(f.subsonic_ticks));
            add(f64::from(f.defeated));
            add(f64::from(f.lost));
        }
        for n in 28..=30 {
            let (live, forecast, damage) = defeat_contact(n, 0., false);
            add(f64::from(live));
            add(f64::from(forecast));
            add(f64::from(damage));
        }
        // Exercise live seeker/support, contact and launcher exclusion too.
        for owner in [0, 100] {
            let mut state = super::super::tests::fixture(true);
            state.weapon_rules = Rules::Energy;
            state.energy_predictions_enabled = false;
            let mut target = super::super::tests::target(100, [0., 10000., 3000.], 10000, 0x80);
            target.signature.infrared = 100.;
            let w = weapon("AIM9M.JT", 2);
            let mut p = projectile(&w, &target);
            p.owner = owner;
            state.targets = vec![target];
            state.projectiles = vec![p];
            for _ in 0..1200 {
                state.step_surface(&[], |_, _| 0., |_, _| false);
            }
            add(f64::from(state.targets[0].hp));
            add(state.projectiles.len() as f64);
        }
        hash
    }
    #[test]
    fn behavior_hash_requires_a_model_revision_for_rule_changes() {
        let identity = missiles::energy_profile::fingerprint();
        let trace = behavior_trace(1., 1.);
        // A real PN-law perturbation changes behaviour without changing data
        // or declared revisions. This is the defect this tripwire catches.
        assert_ne!(trace, behavior_trace(1.1, 1.));
        assert_ne!(trace, behavior_trace(1., 0.5));
        assert_eq!(identity, missiles::energy_profile::fingerprint());
        let fixture = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/golden_tests/missile-energy.txt"
        ))
        .unwrap();
        let expected: Vec<u64> = fixture
            .split_whitespace()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(
            [identity, trace].as_slice(),
            expected.as_slice(),
            "review behavior and bump its semantic revision before updating this pair"
        );
    }
    #[test]
    fn expired_guidance_does_not_remove_the_live_body() {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        state.energy_predictions_enabled = false;
        let target = super::super::tests::target(100, [0., 1000. / FT, 5000. / FT], 10000, 0x80);
        let w = weapon("AIM120.JT", 3);
        let mut p = projectile(&w, &target);
        p.motion
            .as_mut()
            .unwrap()
            .energy
            .as_mut()
            .unwrap()
            .body
            .guidance_ticks = 2;
        state.targets = vec![target];
        state.projectiles = vec![p];
        for _ in 0..10 {
            state.step_surface(&[], |_, _| 0., |_, _| false);
        }
        assert_eq!(state.projectiles.len(), 1);
        assert!(
            state.projectiles[0]
                .motion
                .unwrap()
                .energy
                .unwrap()
                .flight
                .lost
        );
        assert!(state.projectiles[0].guidance.as_ref().unwrap().unguided);
    }
    #[test]
    fn missile_never_fuzes_on_its_launcher_but_hits_another_aircraft() {
        for owner in [0, 100] {
            let mut state = super::super::tests::fixture(true);
            state.weapon_rules = Rules::Energy;
            state.energy_predictions_enabled = false;
            let mut target =
                super::super::tests::target(100, [0., 1000. / FT, 300. / FT], 10000, 0x80);
            target.side = Side(2);
            let w = weapon("AIM120.JT", 3);
            let mut p = projectile(&w, &target);
            p.owner = owner;
            p.guidance.as_mut().unwrap().unguided = true;
            state.targets = vec![target];
            state.projectiles = vec![p];
            let mut hit = false;
            for _ in 0..240 {
                hit |= state
                    .step_surface(&[], |_, _| 0., |_, _| false)
                    .iter()
                    .any(|e| matches!(e, Event::Hit(100)));
            }
            assert_eq!(hit, owner != 100, "owner={owner}");
        }
    }
    #[test]
    fn seeker_discards_its_launcher_including_a_preexisting_memory_cue() {
        let target = super::super::tests::target(100, [0., 1000. / FT, 300. / FT], 10000, 0x80);
        let w = weapon("AIM120.JT", 3);
        let mut p = projectile(&w, &target);
        p.owner = 100;
        for _ in 0..120 {
            guide_owned(
                &mut p,
                &w,
                std::slice::from_ref(&target),
                std::iter::empty(),
                None,
                &|_, _| false,
                &|_, _| 0.,
            );
            assert_ne!(p.target, Some(100));
            assert!(p.motion.unwrap().energy.unwrap().target.is_none());
            assert!(p.guidance.as_ref().unwrap().last_intercept.is_none());
        }
    }
    #[test]
    fn live_decoy_odds_respond_to_heat_aspect_and_imaging() {
        let mut target = super::super::tests::target(100, [0., 10000., 1000.], 10000, 0x80);
        target.signature.infrared = 100.;
        let mut scene = missiles::decoys::Scene::default();
        let release = super::super::super::countermeasures::Release {
            position: target.position,
            velocity: [0.; 3],
            basis: Basis::new(0., 0., 0.),
        };
        scene.release(
            missiles::decoys::Device::from_release(
                1,
                target.id,
                missiles::decoys::Kind::Flare,
                release,
                0,
            ),
            0,
        );
        for _ in 0..12 {
            scene.step([0.; 3], &|_, _| 0.);
        }
        let threshold = |target: &Target, source: &str| {
            let w = weapon(source, 2);
            let mut p = projectile(&w, target);
            energy_decoy(&mut p, &w, target, &scene, 123, &|_, _| false)
                .unwrap()
                .threshold
        };
        target.heat = Heat::Engine {
            on: true,
            throttle: 1.,
            afterburner: false,
        };
        let tail = threshold(&target, "AIM9M.JT");
        target.basis = Basis::new(std::f64::consts::FRAC_PI_2, 0., 0.);
        let beam = threshold(&target, "AIM9M.JT");
        target.heat = Heat::Engine {
            on: false,
            throttle: 0.,
            afterburner: false,
        };
        let cold = threshold(&target, "AIM9M.JT");
        assert!(tail < beam && beam < cold, "{tail}/{beam}/{cold}");
        assert!(threshold(&target, "AIM9X.JT") < tail);
        let w = weapon("AIM9M.JT", 2);
        let profile = missiles::Profile::for_weapon(&w).unwrap();
        let view = seeker::View {
            position: [0., 10000., 0.],
            basis: Basis::new(0., 0., 0.),
            cap: None,
            obscured: &|_, _| false,
        };
        assert!(
            seeker::observe_energy(&w, profile, &view, &target).is_some_and(|o| o.quality >= 0.25),
            "nearby cold target stays measurable"
        );
    }
    #[test]
    fn a_terminal_notch_cannot_be_bypassed_by_fresh_launcher_coordinates() {
        for source in ["AIM120.JT", "AIM7.JT"] {
            let mut w = weapon(source, 3);
            w.seeker.zones[0].maximum_range = 100_000;
            if source == "AIM7.JT" {
                w.flags |= 0x200;
            }
            let mut target =
                super::super::tests::target(100, [0., 1000. / FT, 5000. / FT], 10000, 0x80);
            target.velocity = [100. / FT, 0., 0.];
            let mut p = projectile(&w, &target);
            let remembered = [0., 1000. / FT, 4900. / FT];
            p.guidance.as_mut().unwrap().last_intercept = Some(remembered);
            p.motion
                .as_mut()
                .unwrap()
                .energy
                .as_mut()
                .unwrap()
                .tracking
                .notched = true;
            let support = ActorSupport {
                owner: 0,
                observation: Some(seeker::Observation {
                    id: target.id,
                    position: target.position,
                    velocity: target.velocity,
                    quality: 1.,
                    off_axis: 0.,
                    range: 5000. / FT,
                }),
                supported: true,
                radar_position: p.position,
                radar_emitting: true,
            };
            guide_owned(
                &mut p,
                &w,
                std::slice::from_ref(&target),
                std::iter::empty(),
                Some(&support),
                &|_, _| false,
                &|_, _| 0.,
            );
            let observation = p.motion.unwrap().energy.unwrap().target.unwrap();
            assert_eq!(observation.position, remembered.map(|v| v * FT), "{source}");
            assert_eq!(observation.velocity, [0.; 3], "{source}");
        }
    }
    #[test]
    fn an_energy_sam_does_not_go_blind_inside_its_launch_minimum() {
        let mut w = weapon("FIM92.JT", 2);
        w.seeker.zones[0].minimum_range = 7500;
        w.seeker.zones[0].maximum_range = 100_000;
        let target = super::super::tests::target(100, [0., 1000., 1000.], 10000, 0x80);
        let profile = missiles::Profile::for_weapon(&w).unwrap();
        let view = seeker::View {
            position: [0., 1000., 0.],
            basis: Basis::new(0., 0., 0.),
            cap: None,
            obscured: &|_, _| false,
        };
        assert!(seeker::observe_energy(&w, profile, &view, &target).is_some());
        assert!(seeker::observe(&w, profile, &view, &target).is_none());
        assert_eq!(w.seeker.zones[0].minimum_range, 7500);
    }
    /// The SA-2's decoded 3,000 ft zone floor is a look-up limit from the
    /// site. Measured from the climbing missile it hid every target within
    /// 3,000 ft of the missile's own altitude, so no moving target was hit.
    #[test]
    fn an_energy_sam_keeps_its_target_near_the_target_altitude() {
        let mut w = weapon("FIM92.JT", 2);
        w.seeker.zones[0].minimum_altitude = 3000;
        w.seeker.zones[0].maximum_range = 100_000;
        let target = super::super::tests::target(100, [0., 9000., 3000.], 10000, 0x80);
        let profile = missiles::Profile::for_weapon(&w).unwrap();
        let view = seeker::View {
            position: [0., 8000., 0.],
            basis: Basis::new(0., (1000f64).atan2(3000.), 0.),
            cap: None,
            obscured: &|_, _| false,
        };
        assert!(seeker::observe_energy(&w, profile, &view, &target).is_some());
        assert!(seeker::observe(&w, profile, &view, &target).is_none());
    }
    #[test]
    fn live_contact_tick_matches_the_shared_point_estimate() {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        state.energy_predictions_enabled = false;
        state.own_mut().side = Side(1);
        let mut target =
            super::super::tests::target(100, [0., 1000. / FT, 5000. / FT], 10000, 0x80);
        target.side = Side(2);
        target.radius = AIRCRAFT_RADIUS_FT;
        let w = weapon("AIM120.JT", 3);
        let p = projectile(&w, &target);
        let energy = p.motion.unwrap().energy.unwrap();
        let expected = Trial::new(
            energy.body,
            energy.flight,
            missiles::energy::Observation {
                position: target.position.map(|v| v * FT),
                velocity: [0.; 3],
            },
            (f64::from(w.damage.fuze_radius) + AIRCRAFT_RADIUS_FT) * FT,
            0,
        )
        .finish(&|_, _| 0.);
        assert!(expected.hit, "{expected:?}");
        state.set_actor_supports([ActorSupport {
            owner: 0,
            observation: Some(seeker::Observation {
                id: target.id,
                position: target.position,
                velocity: target.velocity,
                quality: 1.,
                off_axis: 0.,
                range: 5000. / FT,
            }),
            supported: true,
            radar_position: p.position,
            radar_emitting: true,
        }]);
        state.targets = vec![target];
        state.projectiles = vec![p];
        let mut contact = None;
        for tick in 1..=energy.body.life {
            if state
                .step_surface(&[], |_, _| 0., |_, _| false)
                .iter()
                .any(|e| matches!(e, Event::Hit(100)))
            {
                contact = Some(tick);
                break;
            }
        }
        assert_eq!(contact, Some((expected.seconds * 120.).round() as u32));
    }
    #[test]
    fn prediction_and_live_preserve_the_existing_arming_grace() {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        state.energy_predictions_enabled = false;
        state.own_mut().side = Side(1);
        let mut target = super::super::tests::target(100, [0., 1000. / FT, 50. / FT], 10000, 0x80);
        target.side = Side(2);
        target.radius = AIRCRAFT_RADIUS_FT;
        let mut w = weapon("AIM120.JT", 3);
        w.damage.fuze_arm_t = 1;
        let p = projectile(&w, &target);
        let energy = p.motion.unwrap().energy.unwrap();
        let mut q = Trial::new(
            energy.body,
            energy.flight,
            missiles::energy::Observation {
                position: target.position.map(|v| v * FT),
                velocity: [0.; 3],
            },
            prediction_radius(&w) * FT,
            30,
        );
        state.targets = vec![target];
        state.projectiles = vec![p];
        for _ in 0..30 {
            q.step(&|_, _| 0.);
            assert!(!q.answer.is_some_and(|a| a.hit));
            assert!(
                !state
                    .step_surface(&[], |_, _| 0., |_, _| false)
                    .iter()
                    .any(|e| matches!(e, Event::Hit(100)))
            );
        }
        assert!(
            q.closest < AIRCRAFT_RADIUS_FT * FT,
            "the unarmed body actually passed the aircraft"
        );
    }
    #[test]
    fn a_late_physical_decoy_does_not_disarm_the_real_aircraft_fuze() {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        state.own_mut().side = Side(1);
        let mut target = super::super::tests::target(100, [0., 1000. / FT, 50. / FT], 10000, 0x80);
        target.side = Side(2);
        target.radius = AIRCRAFT_RADIUS_FT;
        let w = weapon("AIM120.JT", 3);
        let mut p = projectile(&w, &target);
        p.target = None;
        p.motion
            .as_mut()
            .unwrap()
            .energy
            .as_mut()
            .unwrap()
            .decoy
            .pursuit = Some(missiles::decoys::Pursuit {
            device: missiles::decoys::Device::release(
                1,
                100,
                missiles::decoys::Kind::Chaff,
                1,
                // Far beyond the aircraft: the missile is still pursuing the
                // decoy, not in the step that passes it, when it meets the jet.
                [0., 1000., 2000.],
                [0.; 3],
            ),
            passed: false,
        });
        state.targets = vec![target];
        state.projectiles = vec![p];
        let mut hit = false;
        for _ in 0..30 {
            hit |= state
                .step_surface(&[], |_, _| 0., |_, _| false)
                .iter()
                .any(|event| matches!(event, Event::Hit(100)));
        }
        assert!(
            hit,
            "the aircraft is encountered while the missile still pursues the decoy"
        );
    }
    #[test]
    fn a_missile_that_passed_its_decoy_cannot_fuze_on_a_later_aircraft() {
        let mut state = super::super::tests::fixture(true);
        state.weapon_rules = Rules::Energy;
        state.own_mut().side = Side(1);
        // The aircraft sits on the coasting missile's sagging path about
        // 2.7 s after it passes the decoy, so only the disabled fuze spares it.
        let mut target =
            super::super::tests::target(100, [0., 1000. / FT, 1000. / FT], 10000, 0x80);
        target.side = Side(2);
        target.radius = AIRCRAFT_RADIUS_FT;
        let w = weapon("AIM120.JT", 3);
        let mut p = projectile(&w, &target);
        target.position = [0., 3052.5, 3251.4];
        p.target = None;
        p.motion
            .as_mut()
            .unwrap()
            .energy
            .as_mut()
            .unwrap()
            .decoy
            .pursuit = Some(missiles::decoys::Pursuit {
            device: missiles::decoys::Device::release(
                1,
                100,
                missiles::decoys::Kind::Chaff,
                1,
                [0., 1000., 10.],
                [0.; 3],
            ),
            passed: false,
        });
        state.targets = vec![target];
        state.projectiles = vec![p];
        let mut hit = false;
        let mut closest = f64::INFINITY;
        for _ in 0..600 {
            hit |= state
                .step_surface(&[], |_, _| 0., |_, _| false)
                .iter()
                .any(|event| matches!(event, Event::Hit(100)));
            if let Some(p) = state.projectiles.first() {
                closest = closest.min(missiles::length(missiles::sub(
                    p.position,
                    state.targets[0].position,
                )));
            }
        }
        assert!(!hit, "passing the decoy permanently disables the fuze");
        // Without that rule the coasting missile would have fuzed here.
        let reach = AIRCRAFT_RADIUS_FT + f64::from(w.damage.fuze_radius.max(0));
        assert!(
            closest < reach,
            "the passed missile met the aircraft at {closest:.0} ft"
        );
    }
    #[test]
    fn modern_ir_motors_catch_a_straight_fighter_astern_but_a_long_low_drag_escapes() {
        for source in ["AIM9M.JT", "AIM9X.JT", "AA11.JT"] {
            let w = weapon(source, 2);
            let body = missiles::energy_profile::body(&w);
            assert!(body.valid(), "{source}");
            let shot = |alt: f64, distance: f64| {
                missiles::energy_estimate::Trial::new(
                    body,
                    missiles::energy::Flight::launch(
                        [0., alt * FT, 0.],
                        [0., 0., 300.],
                        [0., 0., 1.],
                    ),
                    missiles::energy::Observation {
                        position: [0., alt * FT, distance],
                        velocity: [0., 0., 250.],
                    },
                    10.,
                    0,
                )
                .finish(&|_, _| 0.)
            };
            assert!(
                shot(15000., 2700.).hit,
                "{source}: 2.7 km astern at 15,000 ft"
            );
            assert!(
                shot(5000., 2700.).hit,
                "{source}: 2.7 km astern at 5,000 ft"
            );
            assert!(
                !shot(500., 12000.).hit,
                "{source}: a 12 km deck drag escapes"
            );
        }
    }
}
