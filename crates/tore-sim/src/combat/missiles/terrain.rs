//! Fitted cruise terrain clearance, shared by flight and fly-out prediction.
//! Constants and limitations: docs/spec/missiles.md, "Cruise terrain clearance".
use super::*;

const LOOKAHEAD_FT: f64 = 2. * NMI;
const SAMPLE_FT: f64 = 256.;
const CLEARANCE_FT: f64 = 256.;
const SAMPLE_MARGIN_FT: f64 = 64.;
const TERMINAL_FT: f64 = 5120.;
const RESPONSE_SECONDS: f64 = 2.;

/// Prelaunch surface visibility. The old eight samples over a whole seeker
/// range could skip a ridge near the target. Keep the sight's 25 ft endpoint
/// allowance and bound each sample interval to 256 ft for normal map ranges.
pub fn launch_line_clear(from: Vector, to: Vector, ground: &dyn Fn(f64, f64) -> f64) -> bool {
    let delta = sub(to, from);
    let range = length(delta);
    let end = if range > 25. { 1. - 25. / range } else { 0. };
    let steps = (range * end / SAMPLE_FT).ceil().clamp(1., 4096.) as u32;
    (0..=steps).all(|i| {
        let share = end * f64::from(i) / f64::from(steps);
        let p: Vector = std::array::from_fn(|axis| from[axis] + delta[axis] * share);
        p[1] > ground(p[0], p[2])
    })
}

impl Motion {
    /// Keep the record's aim unless terrain requires a steeper path. This
    /// changes the steering command only, never position, speed or lock state.
    pub fn aim_over_terrain(
        &self,
        position: Vector,
        intercept: Vector,
        ground: &dyn Fn(f64, f64) -> f64,
    ) -> Vector {
        cruise_aim(self.cruise, self.velocity, position, intercept, ground)
    }
}

pub(super) fn cruise_aim(
    cruise: Option<Cruise>,
    velocity: Vector,
    position: Vector,
    intercept: Vector,
    ground: &dyn Fn(f64, f64) -> f64,
) -> Vector {
    let aim = cruise.map_or(intercept, |c| c.aim(position, intercept));
    if cruise.is_none() {
        return aim;
    }
    clearance_aim(velocity, position, intercept, aim, ground)
}

pub(super) fn energy_cruise_aim(
    cruise: Cruise,
    velocity: Vector,
    position: Vector,
    intercept: Vector,
    ground: &dyn Fn(f64, f64) -> f64,
) -> (Vector, bool) {
    let mut aim = cruise.aim(position, intercept);
    if length(sub(aim, intercept)) > 1e-6 {
        let dx = intercept[0] - position[0];
        let dz = intercept[2] - position[2];
        let distance = dx.hypot(dz).max(1.);
        let ahead = distance.min(LOOKAHEAD_FT);
        aim[0] = position[0] + dx * ahead / distance;
        aim[2] = position[2] + dz * ahead / distance;
    }
    let aim = clearance_aim(velocity, position, intercept, aim, ground);
    (aim, length(sub(aim, intercept)) > 1e-6)
}

fn clearance_aim(
    velocity: Vector,
    position: Vector,
    intercept: Vector,
    aim: Vector,
    ground: &dyn Fn(f64, f64) -> f64,
) -> Vector {
    let dx = intercept[0] - position[0];
    let dz = intercept[2] - position[2];
    let distance = dx.hypot(dz);
    if distance < 1. {
        return aim;
    }
    // A flat surface is not an intervening obstacle. Preserve low sea
    // cruise and terminal profiles there, including existing sag behavior.
    let base = ground(position[0], position[2]).min(ground(intercept[0], intercept[2]));
    let ahead = distance.min(LOOKAHEAD_FT);
    let aim_distance = (aim[0] - position[0]).hypot(aim[2] - position[2]);
    let profile_slope = (aim[1] - position[1]) / aim_distance.max(1.);
    let mut slope = profile_slope;
    let samples = (ahead / SAMPLE_FT).ceil() as u32;
    let response = velocity[0].hypot(velocity[2]) * RESPONSE_SECONDS;
    for i in 0..=samples {
        let along = ahead * f64::from(i) / f64::from(samples);
        let x = position[0] + dx * along / distance;
        let z = position[2] + dz * along / distance;
        let clearance =
            (CLEARANCE_FT + SAMPLE_MARGIN_FT) * ((distance - along) / TERMINAL_FT).clamp(0., 1.);
        let terrain = ground(x, z);
        if terrain <= base + 1. {
            continue;
        }
        let height = terrain + clearance;
        // Lead a climb by two seconds for the finite turn rate. Descents
        // retain the actual sample distance, so a passed ridge releases
        // the constraint naturally. The target endpoint has no clearance.
        let run = if height > position[1] {
            (along - response).max(SAMPLE_FT)
        } else {
            along.max(SAMPLE_FT)
        };
        slope = slope.max((height - position[1]) / run);
    }
    if slope <= profile_slope {
        aim
    } else {
        [
            position[0] + dx * ahead / distance,
            position[1] + slope * ahead,
            position[2] + dz * ahead / distance,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_visibility_catches_a_ridge_between_the_old_eight_probes() {
        let from = [0., 1000., 0.];
        let to = [0., 10., 20000.];
        let ridge = |_: f64, z: f64| {
            if (19100. ..=19700.).contains(&z) {
                150.
            } else {
                0.
            }
        };
        assert!(crate::combat::live::terrain_hit(from, to, &ridge).is_none());
        assert!(!launch_line_clear(from, to, &ridge));
        assert!(launch_line_clear(from, to, &|_, _| 0.));
        assert!(
            launch_line_clear(from, [0., 0., 20000.], &|_, _| 0.),
            "target ground endpoint is excluded"
        );
    }

    fn flight() -> (Movement, Motion) {
        let m = Movement {
            minimum_speed: 0,
            corner_speed: 1000,
            maximum_speed: 1000,
            acceleration: 0,
            deceleration: 0,
            initial_speed: 1000,
            final_speed: 1000,
            launch_retard: 100,
            ignite_t: 0,
            fuel_t: 240,
            remove_t: 240,
            powered_turn_rate: 10920,
            unpowered_turn_rate: 10920,
            performance_at_0: 100,
            performance_at_20: 100,
            cruise: [29, 4, 29, 4],
            jink: [0; 3],
        };
        let mut motion = Motion::new(&m, [0., 0., 1000.], 1024.);
        motion.cruise = Some(Cruise {
            stages: [(7424., 1024.); 2],
            lob: false,
        });
        (m, motion)
    }

    fn ridge(_: f64, z: f64) -> f64 {
        (1. - ((z - 14000.) / 3000.).abs()).max(0.) * 2200.
    }

    #[test]
    fn cruise_climbs_over_a_ridge_then_resumes_and_prediction_matches_flight() {
        let (m, initial) = flight();
        let target = [0., 10., 30000.];
        let start = [0., 1034., 0.];
        let forward = [0., 0., 1.];
        let predicted = intercept_with_terrain(
            &m,
            initial,
            start,
            forward,
            target,
            [0.; 3],
            0,
            7200,
            Some(&ridge),
        )
        .unwrap();
        let fly = |avoid: bool| {
            let mut p = start;
            let mut direction = forward;
            let mut motion = initial;
            let mut crest_clearance = f64::INFINITY;
            let mut highest: f64 = p[1];
            for tick in 0..7200 {
                let aim = if avoid {
                    motion.aim_over_terrain(p, target, &ridge)
                } else {
                    motion.aim(p, target)
                };
                let next = steer(
                    &m,
                    tick,
                    direction,
                    commanded_heading(direction, motion.path(), unit(sub(aim, p))),
                );
                motion.turn(direction, next);
                direction = next;
                let delta = motion.step(&m, tick, direction);
                for i in 0..3 {
                    p[i] += delta[i];
                }
                highest = highest.max(p[1]);
                if (13500. ..=14500.).contains(&p[2]) {
                    crest_clearance = crest_clearance.min(p[1] - ridge(p[0], p[2]));
                }
                if p[1] <= ridge(p[0], p[2]) {
                    return (false, tick + 1, highest, crest_clearance);
                }
                if length(sub(p, target)) <= 25. {
                    return (true, tick + 1, highest, crest_clearance);
                }
            }
            (false, 7200, highest, crest_clearance)
        };
        assert!(!fly(false).0, "unmodified cruise hits the ridge");
        let result = fly(true);
        assert!(result.0, "{result:?}");
        assert!(
            result.2 > 2456.,
            "climbs above ridge plus clearance: {result:?}"
        );
        assert!(result.3 >= 256., "crest clearance: {result:?}");
        assert!(
            (predicted.seconds - result.1 as f64 * DT).abs() <= DT,
            "{predicted:?} {result:?}"
        );
        assert_eq!(result, fly(true), "deterministic replay");
    }

    #[test]
    fn no_cruise_and_clear_flat_profiles_keep_their_aim() {
        let (_, mut motion) = flight();
        let position = [0., 1034., 0.];
        let target = [0., 10., 30000.];
        assert_eq!(
            motion.aim(position, target),
            motion.aim_over_terrain(position, target, &|_, _| 0.)
        );
        motion.cruise.as_mut().unwrap().stages = [(7424., 256.); 2];
        assert_eq!(
            motion.aim(position, target),
            motion.aim_over_terrain(position, target, &|_, _| 0.),
            "flat sea retains the low record profile"
        );
        motion.cruise = None;
        assert_eq!(
            motion.aim(position, target),
            motion.aim_over_terrain(position, target, &ridge)
        );
    }

    #[test]
    fn clearance_tapers_to_allow_terminal_impact_and_applies_to_lobs_too() {
        let (_, mut motion) = flight();
        let target = [0., 10., 30000.];
        let terminal = [0., 100., 29500.];
        assert_eq!(
            motion.aim_over_terrain(terminal, target, &|_, _| 0.),
            target
        );
        motion.cruise.as_mut().unwrap().lob = true;
        let position = [0., 1034., 6000.];
        assert!(
            motion.aim_over_terrain(position, target, &ridge)[1] > motion.aim(position, target)[1]
        );
    }

    /// Review M1 timing probe: one HUD terrain-aware fly-out over hills, as
    /// `LauncherView::terrain_solution` flies it, against the energy-only
    /// one. `cargo test --release -p tore-sim terrain_fly_out_cost --
    /// --ignored --nocapture`.
    #[test]
    #[ignore = "timing probe"]
    fn terrain_fly_out_cost() {
        let (m, initial) = flight();
        let hills = |x: f64, z: f64| 600. + 500. * (x / 2500.).sin() * (z / 3100.).sin();
        let start = [0., 1500., 0.];
        for (name, range) in [("in reach", 30_000.), ("out of reach", 400_000.)] {
            let target = [0., hills(0., range), range];
            let time = |ground: Option<&dyn Fn(f64, f64) -> f64>| {
                let begin = std::time::Instant::now();
                let mut answer = None;
                for _ in 0..20 {
                    answer = intercept_with_terrain(
                        &m,
                        initial,
                        start,
                        [0., 0., 1.],
                        target,
                        [0.; 3],
                        0,
                        7200,
                        ground,
                    );
                }
                (
                    begin.elapsed().as_secs_f64() * 1e6 / 20.,
                    answer.map(|s| s.seconds),
                )
            };
            let flat = time(None);
            let terrain = time(Some(&hills));
            println!(
                "m1 {name}: energy-only {:.0} us {:?}, terrain {:.0} us {:?}",
                flat.0, flat.1, terrain.0, terrain.1
            );
        }
    }
}
