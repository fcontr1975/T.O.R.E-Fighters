//! Physical countermeasure targets, independent of presentation pool limits.
//! SI units. One cartridge is one logical target and one episode contribution.
use super::energy::{DT, Observation};
use crate::attitude::{Vector, dot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Flare,
    Chaff,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Device {
    pub id: u64,
    pub owner: u32,
    pub kind: Kind,
    pub episode: u64,
    pub position: Vector,
    pub previous: Vector,
    pub velocity: Vector,
    pub age: u32,
    pub alive: bool,
    pub effectiveness: u8,
    pub side: Vector,
    pub resting: bool,
}
impl Device {
    pub fn release(
        id: u64,
        owner: u32,
        kind: Kind,
        episode: u64,
        position: Vector,
        velocity: Vector,
    ) -> Self {
        Self {
            id,
            owner,
            kind,
            episode,
            position,
            previous: position,
            velocity,
            age: 0,
            alive: true,
            effectiveness: 35,
            side: [0.; 3],
            resting: false,
        }
    }
    /// The same release and throw as the visible pair, retained independently
    /// when presentation evicts an older device from its bounded pool.
    pub fn from_release(
        id: u64,
        owner: u32,
        kind: Kind,
        release: super::super::countermeasures::Release,
        side_index: u8,
    ) -> Self {
        let ft = super::energy::FT;
        let position = std::array::from_fn(|i| {
            (release.position[i] - 15. * release.basis.forward[i] - 2. * release.basis.up[i]) * ft
        });
        let velocity = std::array::from_fn(|i| {
            (release.velocity[i]
                - if kind == Kind::Flare {
                    15. * release.basis.up[i]
                } else {
                    0.
                })
                * ft
        });
        let mut device = Self::release(id, owner, kind, id, position, velocity);
        if kind == Kind::Flare {
            let seed = mix(mix(id) ^ u64::from(side_index));
            let draw = (mix(seed ^ mix(0)) >> 11) as f64 / ((1u64 << 53) as f64);
            let distance = (20. + 10. * draw) * if side_index == 0 { ft } else { -ft };
            device.side = release.basis.right.map(|v| v * distance);
        }
        device
    }
    pub fn observation(self) -> Observation {
        Observation {
            position: self.position,
            velocity: self.velocity,
        }
    }
    pub fn lifetime(self) -> u32 {
        match self.kind {
            Kind::Flare => 3600,
            Kind::Chaff => 2400,
        }
    }
    pub fn strength(self) -> f64 {
        if !self.alive {
            return 0.;
        }
        let remaining = self.lifetime().saturating_sub(self.age);
        match self.kind {
            Kind::Flare => (f64::from(remaining) / 360.).min(1.),
            Kind::Chaff => (f64::from(remaining) / 600.).min(1.),
        }
    }
    pub fn step(&mut self, wind: Vector, ground: &impl Fn(f64, f64) -> f64) {
        if !self.alive {
            return;
        }
        self.previous = self.position;
        if self.resting {
            self.age += 1;
            self.alive = self.age < self.lifetime();
            return;
        }
        let air: Vector = std::array::from_fn(|i| self.velocity[i] - wind[i]);
        let damping = match self.kind {
            Kind::Flare => {
                1. / (1.
                    + (32.174 * super::energy::FT) / (30.48 * 30.48) * dot(air, air).sqrt() * DT)
            }
            Kind::Chaff => (-DT / 0.12).exp(),
        };
        self.velocity = std::array::from_fn(|i| wind[i] + air[i] * damping);
        match self.kind {
            Kind::Flare => self.velocity[1] -= 32.174 * super::energy::FT * DT,
            Kind::Chaff => self.velocity[1] -= 1.2192 * (1. - damping),
        }
        let time = f64::from(self.age) * DT;
        let throw = (-time / 0.25).exp() - (-(time + DT) / 0.25).exp();
        for i in 0..3 {
            self.position[i] += self.velocity[i] * DT + self.side[i] * throw;
        }
        self.age += 1;
        let rest = ground(self.position[0], self.position[2])
            + if self.kind == Kind::Flare {
                1.5 * super::energy::FT
            } else {
                0.
            };
        if self.position[1] <= rest {
            self.position[1] = rest;
            self.velocity = [0.; 3];
            self.resting = true;
        }
        self.alive = self.age < self.lifetime();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pursuit {
    pub device: Device,
    pub passed: bool,
}
impl Pursuit {
    /// Relative swept closest approach detects passage even when a fast
    /// missile crosses the entire device position between fixed ticks.
    pub fn after_motion(&mut self, previous: Vector, position: Vector) -> f64 {
        if self.passed || !self.device.alive {
            self.passed = true;
            return 0.;
        }
        let before: Vector = std::array::from_fn(|i| self.device.previous[i] - previous[i]);
        let after: Vector = std::array::from_fn(|i| self.device.position[i] - position[i]);
        let delta: Vector = std::array::from_fn(|i| after[i] - before[i]);
        // The relative range has passed its minimum during this step.
        self.passed = dot(before, delta) < 0. && dot(after, delta) >= 0.;
        if self.passed {
            (-dot(before, delta) / dot(delta, delta).max(1e-12)).clamp(0., 1.)
        } else {
            1.
        }
    }
    pub fn target(self) -> Option<Observation> {
        (!self.passed && self.device.alive).then(|| self.device.observation())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Episode {
    pub owner: u32,
    pub kind: Kind,
    pub start_tick: u64,
    pub id: u64,
}
impl Episode {
    pub fn includes(self, owner: u32, kind: Kind, tick: u64) -> bool {
        self.owner == owner
            && self.kind == kind
            && tick >= self.start_tick
            && tick - self.start_tick < 240
    }
}

/// First-second seduction targets; captured devices live in missile state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scene {
    pub devices: Vec<Device>,
    pub episodes: Vec<Episode>,
}
impl Scene {
    pub fn release(&mut self, mut device: Device, tick: u64) {
        if let Some(episode) = self
            .episodes
            .iter()
            .find(|e| e.includes(device.owner, device.kind, tick))
        {
            device.episode = episode.id;
        } else {
            self.episodes
                .retain(|e| tick.saturating_sub(e.start_tick) < 240);
            self.episodes.push(Episode {
                owner: device.owner,
                kind: device.kind,
                start_tick: tick,
                id: device.id,
            });
            device.episode = device.id;
        }
        if self.devices.len() == 1024 {
            self.devices.remove(0);
        }
        self.devices.push(device);
    }
    pub fn effectiveness(&mut self, owner: u32, kind: Kind, value: u8) {
        if let Some(id) = self
            .devices
            .iter()
            .rev()
            .find(|d| d.owner == owner && d.kind == kind)
            .map(|d| d.id)
        {
            for device in self.devices.iter_mut().filter(|d| d.id == id) {
                device.effectiveness = value;
            }
        }
    }
    pub fn step(&mut self, wind: Vector, ground: &impl Fn(f64, f64) -> f64) {
        for device in &mut self.devices {
            device.step(wind, ground);
        }
        self.devices
            .retain(|device| device.alive && device.age < 120);
    }
}

/// A missile remembers one episode for its owned target and seeker class.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Memory {
    pub last_episode: Option<u64>,
    pub pursuit: Option<Pursuit>,
}
impl Memory {
    pub fn eligible(&self, episode: u64) -> bool {
        // Scene episode IDs are the monotonically increasing global release count.
        // An older still-visible cloud cannot reopen a consumed episode when a
        // newer episode fails its roll. Reacquired stale clouds are ineligible.
        self.pursuit.is_none() && self.last_episode.is_none_or(|last| episode > last)
    }
    pub fn evaluate(
        &mut self,
        device: Device,
        probability: f64,
        seed: u64,
        missile: u32,
    ) -> Option<bool> {
        if !self.eligible(device.episode) {
            return None;
        }
        self.last_episode = Some(device.episode);
        let draw = mix(seed ^ mix(u64::from(missile)) ^ mix(device.episode)) >> 11;
        let decoyed = (draw as f64) / ((1u64 << 53) as f64) < probability.clamp(0., 1.);
        if decoyed {
            self.pursuit = Some(Pursuit {
                device,
                passed: false,
            });
        }
        Some(decoyed)
    }
}

pub fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

/// Ratios are received signal ratios, after range, visibility and seeker FOV.
pub fn infrared_probability(
    susceptibility: u8,
    effectiveness: u8,
    ratio: f64,
    generation: f64,
    imaging: bool,
) -> f64 {
    let p = f64::from(susceptibility) * f64::from(effectiveness) / 10000.
        * generation
        * ratio.clamp(0., 3.);
    p.clamp(0., if imaging { 0.10 } else { 0.95 })
}
/// John's game-balance adjustment, applied to every Energy radar store: 1.30
/// on 2026-10-27, doubled to 2.60 on 2026-10-28.
pub const CHAFF_EFFECTIVENESS_MULTIPLIER: f64 = 2.60;

/// Radar draws use basis points; IR retains its existing whole-percent evaluator.
pub fn chance_units(kind: Kind, probability: f64) -> (u16, u16) {
    let bound = if kind == Kind::Chaff { 10000 } else { 100 };
    let scaled = probability.clamp(0., 1.) * f64::from(bound);
    let threshold = if kind == Kind::Chaff {
        scaled.round()
    } else {
        scaled.floor()
    } as u16;
    (threshold, bound)
}

pub fn radar_probability(
    susceptibility: u8,
    effectiveness: u8,
    notch: f64,
    generation: f64,
) -> f64 {
    // John's multiplier acts on the unfloored chance (lead decision,
    // 2026-10-27); the basis-point draw keeps fractions of a percent, so a
    // base below 1% is buffed too.
    let base = f64::from(susceptibility) * f64::from(effectiveness) / 10000.
        * generation
        * (0.25 + 2.75 * notch.clamp(0., 1.));
    (base * CHAFF_EFFECTIVENESS_MULTIPLIER).clamp(0., 0.95)
}

/// Terrain Doppler separation uses target ground velocity, not closing speed.
/// `look_down` is the sensor's independently established clutter strength.
pub fn notch(target_velocity: Vector, line: Vector, look_down: f64) -> f64 {
    let radial = dot(target_velocity, line).abs();
    (1. - radial / (60. * 0.3048)).clamp(0., 1.) * look_down.clamp(0., 1.)
}

crate::checkpoint_enum!(Kind {Flare=0,Chaff=1});
crate::checkpoint_struct!(Device {
    id,
    owner,
    kind,
    episode,
    position,
    previous,
    velocity,
    age,
    alive,
    effectiveness,
    side,
    resting
});
crate::checkpoint_struct!(Scene { devices, episodes });
crate::checkpoint_struct!(Pursuit { device, passed });
crate::checkpoint_struct!(Episode {
    owner,
    kind,
    start_tick,
    id
});
crate::checkpoint_struct!(Memory {
    last_episode,
    pursuit
});

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chaff_buff_scales_the_unfloored_chance_and_does_not_change_ir_units() {
        // AA12-like 50 x 35 active seeker at zero notch: 1.53125% before.
        let base = 50. * 35. / 10000. * 0.35 * 0.25;
        let buffed = radar_probability(50, 35, 0., 0.35);
        assert!((buffed - base * 2.60).abs() < 1e-12);
        assert_eq!(chance_units(Kind::Chaff, buffed), (398, 10000));
        // AIM120-like 40 x 35: 1.225% becomes about 3.19%.
        assert_eq!(
            chance_units(Kind::Chaff, radar_probability(40, 35, 0., 0.35)),
            (319, 10000)
        );
        // A base below 1% is buffed as well: 0.7% becomes 1.82%.
        assert_eq!(
            chance_units(Kind::Chaff, radar_probability(40, 20, 0., 0.35)),
            (182, 10000)
        );
        // Flares keep the whole-percent evaluator.
        assert_eq!(chance_units(Kind::Flare, base), (1, 100));
        assert_eq!(radar_probability(200, 100, 1., 1.), 0.95);
        for generation in [0.35, 0.7, 1.] {
            for notch in [0., 0.25, 0.5, 1.] {
                let old = 0.10 * generation * (0.25 + 2.75 * notch);
                assert!(
                    (radar_probability(50, 20, notch, generation) - (old * 2.60).min(0.95)).abs()
                        < 1e-12
                );
            }
        }
    }
    #[test]
    fn physical_flare_matches_the_visible_release_without_sharing_its_pool() {
        use super::super::super::countermeasures::{Devices, Release};
        use crate::attitude::Basis;
        let release = Release {
            position: [100., 1000., -200.],
            velocity: [0., 0., 700.],
            basis: Basis::new(0.2, 0.1, 0.3),
        };
        let mut visible = Devices::default();
        visible.release_flare(release);
        let mut physical = Device::from_release(1, 0, Kind::Flare, release, 0);
        for _ in 0..120 {
            visible.step(&|_, _| 0.);
            physical.step([0.; 3], &|_, _| 0.);
            let expected = visible.flares.front().unwrap().position;
            for (actual, expected) in physical.position.into_iter().zip(expected) {
                assert!((actual / super::super::energy::FT - expected).abs() < 1e-6);
            }
        }
    }
    #[test]
    fn changing_the_explicit_seed_changes_episode_draws_reproducibly() {
        let d = Device::release(1, 1, Kind::Flare, 1, [0., 1000., 0.], [0.; 3]);
        let count = (0..1000)
            .filter(|seed| Memory::default().evaluate(d, 0.35, *seed, 7) == Some(true))
            .count();
        assert!((300..400).contains(&count), "{count}");
        for seed in 0..100 {
            assert_eq!(
                Memory::default().evaluate(d, 0.35, seed, 7),
                Memory::default().evaluate(d, 0.35, seed, 7)
            );
        }
    }
    #[test]
    fn episode_does_not_compound_and_preserves_large_susceptibilities() {
        let device = Device::release(1, 2, Kind::Flare, 10, [0., 1000., 0.], [0., 0., 200.]);
        let mut memory = Memory::default();
        assert_eq!(memory.evaluate(device, 0., 123, 7), Some(false));
        assert_eq!(
            memory.evaluate(Device { id: 2, ..device }, 1., 123, 7),
            None
        );
        assert_eq!(
            memory.evaluate(
                Device {
                    id: 3,
                    episode: 11,
                    ..device
                },
                1.,
                123,
                7
            ),
            Some(true)
        );
        assert_eq!(
            memory.evaluate(
                Device {
                    id: 4,
                    episode: 12,
                    ..device
                },
                1.,
                123,
                7
            ),
            None
        );
        assert_eq!(infrared_probability(200, 35, 1., 1., false), 0.7);
        assert!((infrared_probability(150, 35, 1., 1., false) - 0.525).abs() < 1e-12);
        let e = Episode {
            owner: 2,
            kind: Kind::Flare,
            start_tick: 10,
            id: 1,
        };
        assert!(e.includes(2, Kind::Flare, 249));
        assert!(!e.includes(2, Kind::Flare, 250));
    }
    #[test]
    fn overlapping_episodes_cannot_alternate_into_repeated_rolls() {
        let mut scene = Scene::default();
        let mut memory = Memory::default();
        let mut rolls = 0;
        for tick in 0..1000u64 {
            if tick.is_multiple_of(30) {
                scene.release(
                    Device::release(tick / 30 + 1, 2, Kind::Chaff, 0, [0., 1000., 0.], [0.; 3]),
                    tick,
                );
            }
            if let Some(d) = scene
                .devices
                .iter()
                .rev()
                .find(|d| memory.eligible(d.episode))
                .copied()
            {
                rolls += usize::from(memory.evaluate(d, 0., 123, 7).is_some());
            }
            scene.step([0.; 3], &|_, _| 0.);
        }
        assert_eq!(rolls, 5);
        assert!(!memory.eligible(1));
    }
    #[test]
    fn weaker_target_signal_increases_flare_odds_and_imaging_resists() {
        let tail = infrared_probability(50, 35, 1., 0.6, false);
        let beam = infrared_probability(50, 35, 1. / 0.7, 0.6, false);
        let cold = infrared_probability(50, 35, 1. / 0.07, 0.6, false);
        assert!(tail < beam && beam < cold);
        assert!(infrared_probability(50, 35, 1. / 0.07, 0.08, true) < tail);
    }
    #[test]
    fn radar_notch_is_relative_to_stationary_clutter() {
        assert_eq!(notch([250., 0., 0.], [0., 0., 1.], 1.), 1.);
        assert_eq!(notch([250., 0., 0.], [0., 0., 1.], 0.), 0.);
        assert_eq!(notch([0., 0., 250.], [0., 0., 1.], 1.), 0.);
        assert!(radar_probability(100, 35, 1., 0.35) > radar_probability(100, 35, 0., 0.35));
    }
    #[test]
    fn devices_move_physically_and_passing_ends_pursuit_without_a_device_fuze() {
        let mut d = Device::release(1, 2, Kind::Chaff, 1, [0., 1000., 0.], [0., 0., 250.]);
        for _ in 0..120 {
            d.step([10., 0., 0.], &|_, _| 0.);
        }
        assert!(d.velocity[2] < 0.1 && d.velocity[0] > 9.9 && d.velocity[1] < 0.);
        let mut p = Pursuit {
            device: d,
            passed: false,
        };
        let before = [d.position[0], d.position[1], d.position[2] - 5.];
        let after = [d.position[0], d.position[1], d.position[2] + 5.];
        p.after_motion(before, after);
        assert!(p.passed && p.target().is_none());
        // Fuzes are deliberately not evaluated here. Live aircraft contact
        // remains authoritative even while this physical decoy is pursued.
        let copy =
            crate::checkpoint::round_trip(&p, &crate::checkpoint::Models::default()).unwrap();
        assert_eq!(copy, p);
    }
}
