//! Shared 120 Hz point-mass missile flight. All quantities in this module are SI.
//! The numbers and provenance belong to docs/spec/missile-energy.md.
use crate::attitude::{Vector, cross, dot, unit};

pub const DT: f64 = 1. / 120.;
pub const G: f64 = 9.80665;
pub const DRAG_CURVES: [[f64; 7]; 3] = [
    [0., 0.8, 1., 1.2, 2., 3., 5.],
    [0.30, 0.30, 0.65, 0.55, 0.36, 0.30, 0.26],
    [0.20, 0.24, 0.65, 0.60, 0.45, 0.38, 0.34],
];
pub const FT: f64 = 0.3048;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Air {
    pub density: f64,
    pub sound: f64,
}

/// Dry standard atmosphere, with geometric height converted to geopotential.
pub fn atmosphere(height: f64) -> Air {
    const R: f64 = 287.05287;
    const LAYERS: [(f64, f64, f64, f64); 7] = [
        (0., 288.15, 101325., -0.0065),
        (11000., 216.65, 22632.06, 0.),
        (20000., 216.65, 5474.889, 0.001),
        (32000., 228.65, 868.0187, 0.0028),
        (47000., 270.65, 110.9063, 0.),
        (51000., 270.65, 66.93887, -0.0028),
        (71000., 214.65, 3.95642, -0.002),
    ];
    let geometric = height.clamp(-1000., 85000.);
    let h = 6356766. * geometric / (6356766. + geometric);
    let &(base, temperature, pressure, lapse) = LAYERS
        .iter()
        .rev()
        .find(|row| h >= row.0)
        .unwrap_or(&LAYERS[0]);
    let t = temperature + lapse * (h - base);
    let p = if lapse == 0. {
        pressure * (-G * (h - base) / (R * temperature)).exp()
    } else {
        pressure * (t / temperature).powf(-G / (R * lapse))
    };
    Air {
        density: p / (R * t),
        sound: (1.4 * R * t).sqrt(),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Motor {
    pub ticks: u32,
    pub thrust: f64,
    pub propellant: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub mass: f64,
    pub diameter: f64,
    pub boost: Motor,
    pub sustain: Motor,
    pub ignition: u32,
    pub life: u32,
    pub guidance_ticks: u32,
    /// Zero disables post-burn defeat for ground-target classes.
    pub defeat_mach: f64,
    pub defeat_ticks: u32,
    pub structural_g: f64,
    pub lift_coefficient: f64,
    pub induced_factor: f64,
    pub drag_scale: f64,
    pub cruise_drag: bool,
    pub minimum_speed: f64,
    pub navigation_gain: f64,
    pub loft_height: f64,
    pub loft_range: f64,
    /// Imported surface cruise schedule; its units are explicitly feet.
    pub cruise_feet: Option<super::Cruise>,
    pub launch_elevation: f64,
}

impl Body {
    pub fn launch_axis(self, axis: Vector) -> Vector {
        if axis[1] >= self.launch_elevation.sin() {
            return axis;
        }
        let horizontal = (axis[0] * axis[0] + axis[2] * axis[2]).sqrt().max(1e-12);
        [
            axis[0] / horizontal * self.launch_elevation.cos(),
            self.launch_elevation.sin(),
            axis[2] / horizontal * self.launch_elevation.cos(),
        ]
    }

    pub fn dry_mass(self) -> f64 {
        self.mass - self.boost.propellant - self.sustain.propellant
    }
    pub fn valid(self) -> bool {
        [
            self.mass,
            self.diameter,
            self.structural_g,
            self.lift_coefficient,
            self.drag_scale,
            self.minimum_speed,
            self.navigation_gain,
        ]
        .into_iter()
        .all(|x| x.is_finite() && x > 0.)
            && self.induced_factor.is_finite()
            && self.induced_factor >= 0.
            && self.dry_mass() > 0.
            && self.life > 0
            && self.guidance_ticks > 0
            && self.defeat_mach.is_finite()
            && self.defeat_mach >= 0.
            && self.defeat_ticks > 0
            && [self.boost, self.sustain].into_iter().all(|m| {
                m.thrust.is_finite()
                    && m.thrust >= 0.
                    && m.propellant.is_finite()
                    && m.propellant >= 0.
                    && (m.ticks > 0 || (m.thrust == 0. && m.propellant == 0.))
            })
            && self.loft_height.is_finite()
            && self.loft_height >= 0.
            && self.loft_range.is_finite()
            && self.loft_range >= 0.
    }
    pub fn motor(self, age: u32) -> Option<(Motor, u32)> {
        let elapsed = age.checked_sub(self.ignition)?;
        if elapsed < self.boost.ticks {
            Some((self.boost, elapsed))
        } else if elapsed - self.boost.ticks < self.sustain.ticks {
            Some((self.sustain, elapsed - self.boost.ticks))
        } else {
            None
        }
    }
    pub fn mass_at(self, age: u32) -> f64 {
        let elapsed = age.saturating_sub(self.ignition);
        let used = |m: Motor, ticks: u32| {
            if m.ticks == 0 {
                0.
            } else {
                m.propellant * f64::from(ticks.min(m.ticks)) / f64::from(m.ticks)
            }
        };
        self.mass
            - used(self.boost, elapsed)
            - used(self.sustain, elapsed.saturating_sub(self.boost.ticks))
    }
    pub fn area(self) -> f64 {
        std::f64::consts::PI * self.diameter * self.diameter * 0.25
    }
    pub fn drag_coefficient(self, mach: f64) -> f64 {
        let mach_points = DRAG_CURVES[0];
        let values = DRAG_CURVES[if self.cruise_drag { 2 } else { 1 }];
        for i in 1..mach_points.len() {
            if mach < mach_points[i] {
                let t = ((mach - mach_points[i - 1]) / (mach_points[i] - mach_points[i - 1]))
                    .clamp(0., 1.);
                return (values[i - 1] + t * (values[i] - values[i - 1])) * self.drag_scale;
            }
        }
        values[6] * self.drag_scale
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    pub position: Vector,
    pub velocity: Vector,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flight {
    pub position: Vector,
    pub velocity: Vector,
    pub axis: Vector,
    pub demand: Vector,
    pub age: u32,
    pub subsonic_ticks: u32,
    /// Permanent post-burn Mach defeat. Unlike control expiry, disables damage.
    pub defeated: bool,
    pub controlled: bool,
    pub lost: bool,
    pub lofted: bool,
    pub loft_decided: bool,
    pub loft_weight: f64,
    pub terminal: bool,
    pub launch_height: f64,
    pub waypoint: Option<Vector>,
    pub path_active: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Forces {
    pub mass: f64,
    pub thrust: f64,
    pub drag: f64,
    pub induced_drag: f64,
    pub available_g: f64,
    pub applied_g: f64,
    pub required_g: f64,
    pub g_deficit: f64,
    pub closing_mps: f64,
    pub mach: f64,
    pub terrain_samples: usize,
}

fn norm(v: Vector) -> f64 {
    dot(v, v).sqrt()
}
fn sub(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] - b[i])
}
fn perpendicular(v: Vector, axis: Vector) -> Vector {
    let along = dot(v, axis);
    std::array::from_fn(|i| v[i] - along * axis[i])
}
fn limited(v: Vector, limit: f64) -> Vector {
    let scale = (limit / norm(v).max(1e-12)).min(1.);
    v.map(|x| x * scale)
}

impl Flight {
    pub fn launch(position: Vector, velocity: Vector, axis: Vector) -> Self {
        Self {
            position,
            velocity,
            axis: unit(axis),
            demand: [0.; 3],
            age: 0,
            subsonic_ticks: 0,
            defeated: false,
            controlled: false,
            lost: false,
            lofted: false,
            loft_decided: false,
            loft_weight: 0.,
            terminal: false,
            launch_height: position[1],
            waypoint: None,
            path_active: false,
        }
    }
    /// PN or capture against a measured target. No observations means ballistic.
    fn command(&mut self, body: Body, u: Vector, target: Observation) -> Vector {
        let mut r = sub(target.position, self.position);
        let mut vr = sub(target.velocity, self.velocity);
        let range = norm(r).max(1.);
        let mut path_weight = f64::from(self.path_active);
        if !self.terminal && body.loft_height > 0. {
            if !self.loft_decided {
                let x = (range / body.loft_range.max(1.) - 0.5).clamp(0., 1.);
                self.loft_weight = x * x * (3. - 2. * x);
                self.lofted = self.loft_weight > 0.;
                self.loft_decided = true;
            }
            if range <= body.loft_range * 0.4 {
                self.terminal = true;
            } else if self.lofted {
                let x = ((range / body.loft_range.max(1.) - 0.4) / 0.3).clamp(0., 1.);
                path_weight = self.loft_weight * x * x * (3. - 2. * x);
                let height = self.launch_height.max(target.position[1]) + body.loft_height;
                let horizontal = r[0].hypot(r[2]).max(1.);
                let climb =
                    (height - self.position[1]).clamp(-0.36 * horizontal, 0.36 * horizontal);
                r[1] += (climb - r[1]) * path_weight;
                vr = sub(
                    target.velocity.map(|v| v * (1. - path_weight)),
                    self.velocity,
                );
            }
        }
        let speed = norm(u);
        let axis = if speed > 1. { unit(u) } else { self.axis };
        let line = unit(r);
        let cosine = dot(axis, line).clamp(-1., 1.);
        let closing = -dot(vr, line);
        let capture = {
            let direction = perpendicular(line, axis);
            let direction = if norm(direction) < 1e-9 && cosine < 0. {
                // Stable reversal axis, including a vertical initial flight path.
                unit(cross(
                    axis,
                    if axis[1].abs() < 0.9 {
                        [0., 1., 0.]
                    } else {
                        [1., 0., 0.]
                    },
                ))
            } else {
                unit(direction)
            };
            direction.map(|x| x * speed * cosine.acos() / 0.5)
        };
        let steer = if closing > 0. && cosine > 30f64.to_radians().cos() {
            let omega = cross(r, vr).map(|x| x / dot(r, r).max(1.));
            let pn = cross(omega, axis).map(|x| x * body.navigation_gain * closing);
            std::array::from_fn(|i| pn[i] * (1. - path_weight) + capture[i] * path_weight)
        } else {
            capture
        };
        let gravity = perpendicular([0., -G, 0.], axis);
        std::array::from_fn(|i| steer[i] - gravity[i])
    }

    pub fn step(&mut self, body: Body, wind: Vector, target: Option<Observation>) -> Forces {
        self.step_terrain(body, wind, target, &|_, _| 0.)
    }
    pub fn step_terrain(
        &mut self,
        body: Body,
        wind: Vector,
        target: Option<Observation>,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> Forces {
        self.step_terrain_mode::<false>(body, wind, target, ground)
    }
    /// Explicit offline diagnostics; ordinary prediction does not evaluate them.
    pub fn step_diagnostics(
        &mut self,
        body: Body,
        wind: Vector,
        target: Option<Observation>,
    ) -> Forces {
        self.step_terrain_mode::<true>(body, wind, target, &|_, _| 0.)
    }
    fn step_terrain_mode<const DIAGNOSTICS: bool>(
        &mut self,
        body: Body,
        wind: Vector,
        target: Option<Observation>,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> Forces {
        let samples = std::cell::Cell::new(0usize);
        let measured_ground = |x: f64, z: f64| {
            samples.set(samples.get() + 1);
            ground(x * FT, z * FT) / FT
        };
        let target = target.map(|observed| {
            if body.cruise_feet.is_none() {
                return observed;
            }
            if self.waypoint.is_none() || self.age.is_multiple_of(12) {
                let (aim, active) = super::terrain::energy_cruise_aim(
                    body.cruise_feet.unwrap(),
                    self.velocity.map(|v| v / FT),
                    self.position.map(|v| v / FT),
                    observed.position.map(|v| v / FT),
                    &measured_ground,
                );
                self.waypoint = Some(aim.map(|v| v * FT));
                self.path_active = active;
            }
            if self.path_active {
                Observation {
                    position: self.waypoint.unwrap(),
                    velocity: [0.; 3],
                }
            } else {
                observed
            }
        });
        if target.is_none() {
            self.waypoint = None;
            self.path_active = false;
        }
        let mut forces =
            self.step_in::<DIAGNOSTICS>(body, wind, target, atmosphere(self.position[1]));
        forces.terrain_samples = samples.get();
        forces
    }

    fn step_in<const DIAGNOSTICS: bool>(
        &mut self,
        body: Body,
        wind: Vector,
        target: Option<Observation>,
        air: Air,
    ) -> Forces {
        if self.age >= body.guidance_ticks {
            self.lost = true;
        }
        let before = self.velocity;
        let relative = sub(before, wind);
        let speed = norm(relative);
        let axis = if speed > 1. {
            unit(relative)
        } else {
            self.axis
        };
        let mass = (body.mass_at(self.age) + body.mass_at(self.age + 1)) * 0.5;
        let qs = 0.5 * air.density * speed * speed * body.area();
        let max_accel = (qs * body.lift_coefficient / mass).min(body.structural_g * G);
        if speed >= body.minimum_speed {
            self.controlled = true;
        }
        // Sample before motion, so the 30th low-Mach sample disables every
        // contact in this tick, including a contact at fraction zero.
        if body.defeat_mach > 0. {
            let burnout = body.ignition + body.boost.ticks + body.sustain.ticks;
            if self.age >= burnout && speed / air.sound < body.defeat_mach {
                self.subsonic_ticks = self.subsonic_ticks.saturating_add(1);
                if self.subsonic_ticks >= body.defeat_ticks {
                    self.defeated = true;
                }
            } else {
                self.subsonic_ticks = 0;
            }
        }
        if self.defeated {
            self.lost = true;
        }
        let command = if !self.lost && self.age >= body.ignition {
            target.map(|o| self.command(body, relative, o))
        } else {
            None
        };
        let required_g = if DIAGNOSTICS {
            command.map_or(0., norm) / G
        } else {
            0.
        };
        let closing_mps = if DIAGNOSTICS {
            target.map_or(0., |o| {
                -dot(
                    sub(o.velocity, self.velocity),
                    unit(sub(o.position, self.position)),
                )
            })
        } else {
            0.
        };
        if let Some(command) = command {
            let command = limited(command, max_accel);
            for (demand, command) in self.demand.iter_mut().zip(command) {
                *demand += (command - *demand) * (DT / 0.08);
            }
        } else {
            self.demand = [0.; 3];
        }
        let lift = limited(perpendicular(self.demand, axis), max_accel);
        let acceleration = norm(lift);
        let induced = if qs > 1e-12 {
            body.induced_factor * (mass * acceleration).powi(2) / qs
        } else {
            0.
        };
        let drag = qs * body.drag_coefficient(speed / air.sound) + induced;
        let thrust = body.motor(self.age).map_or(0., |(motor, _)| motor.thrust);
        // Axial drag integration cannot reverse the velocity in one step.
        let damping = 1. / (1. + drag / (mass * speed.max(1e-6)) * DT);
        let angle = if speed > 1. {
            acceleration / speed * DT
        } else {
            0.
        };
        let (sin, cos) = angle.sin_cos();
        let normal = if acceleration > 1e-12 {
            lift.map(|x| x / acceleration)
        } else {
            [0.; 3]
        };
        let thrust_axis = if self.controlled { axis } else { self.axis };
        self.velocity = std::array::from_fn(|i| {
            wind[i]
                + (speed * (axis[i] * cos + normal[i] * sin) + thrust_axis[i] * thrust / mass * DT)
                    * damping
        });
        self.velocity[1] -= G * DT;
        for (i, value) in before.iter().enumerate() {
            self.position[i] += (value + self.velocity[i]) * 0.5 * DT;
        }
        if self.controlled && norm(sub(self.velocity, wind)) > 1. {
            self.axis = unit(sub(self.velocity, wind));
        }
        self.age += 1;
        Forces {
            mass,
            thrust,
            drag,
            induced_drag: induced,
            available_g: max_accel / G,
            applied_g: acceleration / G,
            required_g,
            g_deficit: if DIAGNOSTICS {
                (required_g - max_accel / G).max(0.)
            } else {
                0.
            },
            closing_mps,
            mach: speed / air.sound,
            terrain_samples: 0,
        }
    }
}

crate::checkpoint_struct!(Flight {
    position,
    velocity,
    axis,
    demand,
    age,
    subsonic_ticks,
    defeated,
    controlled,
    lost,
    lofted,
    loft_decided,
    loft_weight,
    terminal,
    launch_height,
    waypoint,
    path_active
});

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    pub fn body() -> Body {
        Body {
            mass: 150.,
            diameter: 0.18,
            boost: Motor {
                ticks: 360,
                thrust: 18000.,
                propellant: 30.,
            },
            sustain: Motor {
                ticks: 600,
                thrust: 6000.,
                propellant: 20.,
            },
            ignition: 0,
            life: 24000,
            defeat_mach: 0.,
            defeat_ticks: 30,
            guidance_ticks: 120 * 120,
            structural_g: 30.,
            lift_coefficient: 12.,
            induced_factor: 0.03,
            drag_scale: 1.,
            cruise_drag: false,
            minimum_speed: 120.,
            navigation_gain: 3.5,
            loft_height: 0.,
            loft_range: 0.,
            cruise_feet: None,
            launch_elevation: -std::f64::consts::FRAC_PI_2,
        }
    }
    #[test]
    fn diagnostics_are_opt_in_and_do_not_change_motion() {
        let b = body();
        let mut normal = Flight::launch([0., 1000., 0.], [0., 0., 500.], [0., 0., 1.]);
        let mut measured = normal;
        let target = Some(Observation {
            position: [200., 1100., 3000.],
            velocity: [0., 0., -250.],
        });
        for _ in 0..120 {
            let plain = normal.step(b, [0.; 3], target);
            let diagnostic = measured.step_diagnostics(b, [0.; 3], target);
            assert_eq!(normal, measured);
            assert_eq!(
                (plain.required_g, plain.g_deficit, plain.closing_mps),
                (0., 0., 0.)
            );
            assert!(diagnostic.required_g > 0. && diagnostic.closing_mps > 0.);
        }
    }
    #[test]
    fn defeat_boundary_is_strict_post_burn_persistent_and_checkpointed() {
        let b = Body {
            defeat_mach: 1.,
            defeat_ticks: 30,
            ..body()
        };
        let air = Air {
            density: 0.,
            sound: 300.,
        };
        let mut f = Flight::launch([0., 10000., 0.], [0.; 3], [0., 0., 1.]);
        // Even a previously controlled store cannot be kinetically defeated
        // during ignition or either motor phase.
        f.controlled = true;
        for age in 0..b.ignition + b.boost.ticks + b.sustain.ticks {
            f.age = age;
            f.velocity = [0., 0., 10.];
            f.step_in::<false>(b, [0.; 3], None, air);
            assert!(!f.defeated && !f.lost);
            assert_eq!(f.subsonic_ticks, 0);
        }
        for _ in 0..29 {
            f.velocity = [0., 0., 299.];
            f.step_in::<false>(b, [0.; 3], None, air);
            assert!(!f.defeated);
        }
        // Equality resets the consecutive-sample window.
        f.velocity = [0., 0., 300.];
        f.step_in::<false>(b, [0.; 3], None, air);
        assert_eq!(f.subsonic_ticks, 0);
        for _ in 0..15 {
            f.velocity = [0., 0., 299.];
            f.step_in::<false>(b, [0.; 3], None, air);
        }
        let models = crate::checkpoint::Models::default();
        let restored_body = crate::checkpoint::round_trip(&b, &models).unwrap();
        assert_eq!(b, restored_body);
        let mut copy = crate::checkpoint::round_trip(&f, &models).unwrap();
        for tick in 16..=31 {
            f.velocity = [0., 0., 299.];
            copy.velocity = f.velocity;
            assert_eq!(
                f.step_in::<false>(b, [0.; 3], None, air),
                copy.step_in::<false>(restored_body, [0.; 3], None, air)
            );
            assert_eq!(f, copy);
            assert_eq!(f.defeated, tick >= 30);
        }
        f.velocity = [0., 0., 1000.];
        f.step_in::<false>(b, [0.; 3], None, air);
        assert!(f.lost && f.defeated);
    }

    #[test]
    fn subsonic_cruise_keeps_power_and_stationary_target_control() {
        let b = Body {
            defeat_mach: 0.,
            cruise_drag: true,
            ..body()
        };
        let target = Some(Observation {
            position: [0., 1000., 10000.],
            velocity: [0.; 3],
        });
        let mut f = Flight::launch([0., 1000., 0.], [0., 0., 60.], [0., 0., 1.]);
        for _ in 0..120 {
            f.velocity = [0., 0., 60.];
            let forces = f.step_diagnostics(b, [0.; 3], target);
            assert!(forces.thrust > 0. && forces.mach < 1.);
            assert!(forces.required_g.is_finite() && forces.closing_mps > 0.);
            assert!(!f.lost && !f.defeated);
        }
    }

    #[test]
    fn battery_expiry_is_irreversible_and_leaves_ballistic_motion() {
        let mut b = body();
        b.guidance_ticks = 2;
        let mut f = Flight::launch([0., 3000., 0.], [0., 0., 400.], [0., 0., 1.]);
        let target = Some(Observation {
            position: [1000., 3000., 4000.],
            velocity: [0.; 3],
        });
        f.step(b, [0.; 3], target);
        assert!(!f.lost);
        f.step(b, [0.; 3], target);
        let before = f.position;
        f.step(b, [0.; 3], target);
        assert!(f.lost);
        assert_eq!(f.demand, [0.; 3]);
        assert_ne!(f.position, before);
        b.guidance_ticks = 12000;
        f.step(b, [0.; 3], target);
        assert!(f.lost);
        assert_eq!(f.demand, [0.; 3]);
    }
    #[test]
    fn atmosphere_matches_standard_and_has_continuous_layers() {
        let air = atmosphere(0.);
        assert!((air.density - 1.225).abs() < 0.0001);
        assert!((air.sound - 340.294).abs() < 0.01);
        assert!(atmosphere(5000. * FT).density > atmosphere(15000. * FT).density);
        assert!(atmosphere(15000. * FT).density > atmosphere(35000. * FT).density);
        for h in [11000., 20000., 32000., 47000., 51000., 71000.] {
            let geometric = 6356766. * h / (6356766. - h);
            assert!(
                (atmosphere(geometric - 0.01).density - atmosphere(geometric + 0.01).density).abs()
                    < 1e-5
            );
        }
    }
    #[test]
    fn motor_mass_and_impulse_are_finite() {
        let b = body();
        assert!(b.valid());
        let impulse: f64 = (0..2000)
            .map(|t| b.motor(t).map_or(0., |(m, _)| m.thrust) * DT)
            .sum();
        assert!((impulse - 84000.).abs() < 1e-6);
        assert_eq!(b.mass_at(0), 150.);
        assert_eq!(b.mass_at(360), 120.);
        assert_eq!(b.mass_at(960), 100.);
        assert_eq!(b.mass_at(20000), 100.);
    }
    #[test]
    fn drag_and_lift_follow_dynamic_pressure_and_turns_cost_energy() {
        let b = body();
        let mut low = Flight::launch([0., 1500., 0.], [0., 0., 500.], [0., 0., 1.]);
        low.age = 1000;
        let mut high = low;
        high.position[1] = 11000.;
        let mut turn = low;
        let straight = low.step(b, [0.; 3], None);
        let thin = high.step(b, [0.; 3], None);
        let pulled = turn.step(
            b,
            [0.; 3],
            Some(Observation {
                position: [2000., 1500., 2000.],
                velocity: [0.; 3],
            }),
        );
        assert!(straight.drag > thin.drag);
        assert!(straight.available_g > thin.available_g);
        assert!(pulled.induced_drag > 0. && pulled.applied_g <= b.structural_g);
        assert!(norm(turn.velocity) < norm(low.velocity));
    }
    #[test]
    fn coast_loses_total_energy_and_dive_may_gain_speed() {
        let b = body();
        let mut f = Flight::launch([0., 10000., 0.], [0., -200., 100.], [0., -1., 0.]);
        f.age = 1000;
        for _ in 0..120 {
            let e = 0.5 * dot(f.velocity, f.velocity) + G * f.position[1];
            f.step(b, [0.; 3], None);
            assert!(0.5 * dot(f.velocity, f.velocity) + G * f.position[1] < e);
        }
        let mut vacuum = Flight::launch([0., 10000., 0.], [0., -200., 0.], [0., -1., 0.]);
        vacuum.age = 1000;
        vacuum.step_in::<false>(
            b,
            [0.; 3],
            None,
            Air {
                density: 0.,
                sound: 300.,
            },
        );
        assert!(norm(vacuum.velocity) > 200.);
    }
    #[test]
    fn post_burn_defeat_does_not_prevent_a_powered_launch_and_is_permanent() {
        let b = body();
        let mut f = Flight::launch([0., 1000., 0.], [0.; 3], [0., 0., 1.]);
        for _ in 0..240 {
            f.step(b, [0.; 3], None);
        }
        assert!(f.controlled && !f.lost);
        f.velocity = [0., 0., 60.];
        let coast = Body {
            defeat_mach: 1.,
            boost: Motor::default(),
            sustain: Motor::default(),
            ..b
        };
        for _ in 0..30 {
            f.step(coast, [0.; 3], None);
        }
        assert!(f.lost);
        f.velocity = [0., 0., 1000.];
        f.step(b, [0.; 3], None);
        assert!(f.lost);
    }
    #[test]
    fn checkpoint_continues_the_same_flight() {
        let b = body();
        let target = Some(Observation {
            position: [500., 5000., 5000.],
            velocity: [0., 0., -250.],
        });
        let mut f = Flight::launch([0., 5000., 0.], [0., 0., 300.], [0., 0., 1.]);
        for _ in 0..110 {
            f.step(b, [0.; 3], target);
        }
        let mut copy =
            crate::checkpoint::round_trip(&f, &crate::checkpoint::Models::default()).unwrap();
        for _ in 0..200 {
            assert_eq!(f.step(b, [0.; 3], target), copy.step(b, [0.; 3], target));
        }
        assert_eq!(f, copy);
    }
}
