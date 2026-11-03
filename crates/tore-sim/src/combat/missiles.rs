//! Spec-derived missile profiles and fitted motion, docs/spec/missiles.md.
//! Independent from renderer, host clock and the explicit compatibility adapter.
use super::{EnginePhase, altitude_performance};
use crate::attitude::{Basis, Vector, dot, unit};
use tore_formats::weapons::{Movement, Weapon, Zone};

pub mod decoys;
pub mod energy;
pub mod energy_estimate;
pub mod energy_profile;
pub mod energy_seeker;
pub mod prediction;
mod terrain;
pub use terrain::launch_line_clear;

pub const HZ: u64 = 120;
pub const DT: f64 = 1. / HZ as f64;
pub const NMI: f64 = 6076.;
pub const DWELL: u32 = 30;
pub const MEMORY: u32 = 240;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rules {
    Compatibility,
    #[default]
    Spec,
    /// Finite-energy rules selected by new worlds; historical tools retain Spec.
    Energy,
}
impl Rules {
    pub fn uses_seeker(self) -> bool {
        self != Self::Compatibility
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LaunchMode {
    #[default]
    Cued,
    Boresight,
}
impl LaunchMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cued => "CUED",
            Self::Boresight => "BORESIGHT",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Guidance {
    Supported,
    Active,
    Infrared,
    Emitter,
    /// Homes on a laser spot: the GBU-10 family, GBU-27, GBU-28, AS-14,
    /// AS-30 and AT-12 (signature 1 with the designator flag 0x400; retail
    /// data). The spot is the aircraft's own, else a buddy's (decision 6).
    Laser,
    /// Flies to the ground point designated at release: the GBU-29, GBU-29/P
    /// and GBU-30 JDAMs. Retail calls them "optical" and homes them on the
    /// release target; GPS coordinates are opinionated (John, 2026-10-10,
    /// decision 4).
    Coordinate,
    /// Steered at the designated object while the launcher sees it: the AT-2
    /// Swatter (guided signature 0 with the launcher flag 0x100).
    Command,
}
impl Guidance {
    /// The air-to-ground guidance of a weapon record, from its seeker
    /// signature and type flags (docs/formats/missiles.md, "Laser and
    /// designator" and "Optical"): laser weapons, the guided signature-0
    /// bombs (no bomb flag 0x10, no launcher, support or designator flag) and
    /// the guided signature-0 weapons the launcher steers (0x100). These
    /// weapons have no [`Profile`]: their flight is `live`'s guided flight.
    pub fn air_to_ground(w: &Weapon) -> Option<Self> {
        const GUIDED: u32 = 0x1;
        const LAUNCHER: u32 = 0x100;
        const SUPPORT: u32 = 0x200;
        const DESIGNATOR: u32 = 0x400;
        if super::laser::is_laser_weapon(w) {
            return Some(Self::Laser);
        }
        if w.seeker.signature != 0 || w.flags & GUIDED == 0 || w.flags & 0x10 != 0 {
            return None;
        }
        if w.flags & (SUPPORT | DESIGNATOR) != 0 {
            None
        } else if w.flags & LAUNCHER != 0 {
            Some(Self::Command)
        } else {
            Some(Self::Coordinate)
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetRole {
    Aircraft,
    Surface,
}
/// The surface-to-air records surface units fire that fly supported by their
/// launcher's radar (docs/spec/surface-defenses.md, "SAM missiles"): radar
/// records (seeker 3) with the support flag 0x200, as the missile inventory
/// proposes, plus SA-19 and SA-N-11, which lack the flag but arm the HAWK,
/// Roland, 2S6 and Kirov (default, pending John). Target role aircraft.
/// ASROC stays held (no submarine is ever a target); the SS-N-9 is
/// [`SURFACE_TO_SURFACE`].
pub const SURFACE_SUPPORTED: [&str; 13] = [
    "SA2A.JT",
    "SA3.JT",
    "SA6.JT",
    "SA15.JT",
    "SA19.JT",
    "R440.JT",
    "MIS.JT",
    "SAN3.JT",
    "SAN4.JT",
    "SAN7.JT",
    "SAN9.JT",
    "SAN11.JT",
    "SEA_SPAR.JT",
];
/// The surface-to-air records with infrared seekers (seeker 2), which home on
/// their own: shoulder-launched and vehicle SAMs.
pub const SURFACE_INFRARED: [&str; 6] = [
    "FIM92.JT", "SA7.JT", "SA9.JT", "SA13.JT", "SA14.JT", "SA16.JT",
];
/// The surface-to-surface missiles surface units fire: the Sarancha's SS-N-9,
/// the only missile record a surface unit carries whose target flag is the
/// surface one (0x20000) and not the air one (0x10000). Target role
/// surface; supported radar by the record's support flag 0x200, as the AS-7
/// and AS-15 (retail data). Its launcher aims it at ships only
/// ([`ANTI_SHIP`]). Surface units fight each other since John's request of
/// 2026-10-11; docs/spec/surface-defenses.md, "Surface against surface".
pub const SURFACE_TO_SURFACE: [&str; 1] = ["SSN9.JT"];
/// The surface-to-surface missiles a surface unit aims at ships only: the
/// SS-N-9 is an anti-ship missile (its name in the spec's SAM table; agent
/// decision, 2026-10-11).
pub const ANTI_SHIP: [&str; 1] = ["SSN9.JT"];
/// The reviewed air-to-ground missiles: target role surface. Infrared
/// (AGM-65G, AGM-65A, the AGM-84E SLAM by its data signature 2),
/// anti-radiation (AGM-45, AGM-88), active radar (AGM-84A, AM-39, AS-16) and
/// supported radar (AS-7, AS-15 by its data signature 3 with 0x200; the manual
/// calls it laser). docs/spec/air-to-ground.md, "Weapon classes" 8 to 11.
pub const SURFACE_ROLE: [&str; 10] = [
    "AGM65G.JT",
    "AGM65A.JT",
    "AGM84E.JT",
    "AGM45.JT",
    "AGM88.JT",
    "AGM84A.JT",
    "AM39.JT",
    "AS16.JT",
    "AS7.JT",
    "AS15.JT",
];

/// The air-to-ground weapons whose seeker has a picture, by record name: the
/// infrared AGMs (class 8, AGM-65G, AGM-65A and the AGM-84E SLAM), the
/// laser bombs (class 4, the GBU-10 family, Pave Way and GBU-28), the laser
/// missiles (class 12, AS-14, AS-30 and AT-12) and the AT-2 (retail's
/// "optical", command guided from the launcher's sight). The target camera
/// follows the player's last one in flight (docs/spec/target-window.md,
/// "Seeker view"). Left out, by design: the anti-radiation, active radar and
/// supported radar missiles (no picture) and the JDAMs (they fly to a point
/// and carry no camera; John 2026-10-12: "JDAM no"). Opinionated: the pick was
/// the agent's on John's "every weapon with a seeker view" (2026-10-12), John
/// added the AT-2 the same day and the laser bombs on 2026-10-13 ("nosecone
/// (GBU's, Mavericks, AT-12's etc) views"); the data has no such flag, and no
/// TV or IR guided bomb (GBU-15, Walleye) exists in it.
pub const SEEKER_VIEW: [&str; 14] = [
    "AGM65G.JT",
    "AGM65A.JT",
    "AGM84E.JT",
    "GBU10.JT",
    "GBU10A.JT",
    "PAVEWAY.JT",
    "GBU12.JT",
    "GBU16.JT",
    "PAVEWA3.JT",
    "GBU28.JT",
    "AS14.JT",
    "AS30.JT",
    "AT12.JT",
    "AT2.JT",
];

/// These records are not player stores. Keep their existing launch-origin
/// qualification and sensor minimum while revising aircraft release rules.
pub fn surface_launched(source: &str) -> bool {
    SURFACE_SUPPORTED.contains(&source)
        || SURFACE_INFRARED.contains(&source)
        || SURFACE_TO_SURFACE.contains(&source)
}

/// The seeker-view weapons that home on a laser spot rather than on an
/// object: the laser bombs and the laser missiles (class 4 and 12). Their
/// picture marks the spot (the laser, docs/spec/air-to-ground.md).
pub const SPOT_HOMING: [&str; 10] = [
    "GBU10.JT",
    "GBU10A.JT",
    "PAVEWAY.JT",
    "GBU12.JT",
    "GBU16.JT",
    "PAVEWA3.JT",
    "GBU28.JT",
    "AS14.JT",
    "AS30.JT",
    "AT12.JT",
];

/// Whether the weapon record named `source` homes on a laser spot
/// ([`SPOT_HOMING`]).
pub fn homes_on_spot(source: &str) -> bool {
    SPOT_HOMING.contains(&source)
}

/// Whether the weapon record named `source` has a seeker picture for the
/// target camera ([`SEEKER_VIEW`]).
pub fn has_seeker_view(source: &str) -> bool {
    SEEKER_VIEW.contains(&source)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub role: TargetRole,
    pub guidance: Guidance,
    pub activation_ft: Option<f64>,
    pub guidance_ticks: u64,
    pub memory_ticks: u32,
    pub radar_emissions: bool,
    pub jammer_emissions: bool,
}
impl Profile {
    pub fn validate(self) -> tore_formats::Result<()> {
        let activation_valid = match (self.guidance, self.activation_ft) {
            (Guidance::Active, Some(value)) => value.is_finite() && value > 0.,
            (Guidance::Active, None) | (_, Some(_)) => false,
            (_, None) => true,
        };
        if !activation_valid || self.guidance_ticks == 0 || self.memory_ticks == 0 {
            return Err(super::invalid(
                "invalid missile profile timing or activation",
            ));
        }
        Ok(())
    }

    /// Reviewed profile eligibility and fitted activation distances in nmi.
    /// Guidance itself comes only from `Weapon::seeker_kind`.
    fn reviewed_activation(source: &str) -> Option<Option<f64>> {
        if SURFACE_SUPPORTED.contains(&source)
            || SURFACE_TO_SURFACE.contains(&source)
            || SURFACE_INFRARED.contains(&source)
        {
            return Some(None);
        }
        Some(match source {
            "AIM120.JT" | "MICA.JT" | "AA12.JT" => Some(5.),
            "AAML.JT" | "AGM84A.JT" | "AM39.JT" => Some(8.),
            "AIM54C.JT" => Some(10.),
            "AEMP1.JT" => Some(3.),
            "AS16.JT" => Some(2.),
            "R27ER.JT" | "R27T.JT" | "R27ET.JT" | "R530.JT" | "AS7.JT" | "AS15.JT" | "AA10.JT"
            | "AIM7.JT" | "AIM7E.JT" | "AA11.JT" | "AA11B.JT" | "AA2.JT" | "AA8.JT"
            | "AIM9M.JT" | "AIM9X.JT" | "R550.JT" | "AGM65G.JT" | "AGM65A.JT" | "AGM84E.JT"
            | "AIM9B.JT" | "AGM45.JT" | "AGM88.JT" => None,
            _ => return None,
        })
    }

    /// Whether the weapon record named `source` has a reviewed profile, as
    /// [`Profile::for_weapon`] decides: for presentation that knows a
    /// weapon only by name, such as a mission replay's sound.
    pub fn reviewed(source: &str) -> bool {
        Self::reviewed_activation(source).is_some()
    }

    /// Explicit reviewed identities only. This does not expand store allowlists.
    pub fn for_weapon(w: &Weapon) -> Option<Self> {
        let active = Self::reviewed_activation(&w.source)?;
        use tore_formats::weapons::SeekerKind;
        let guidance = match w.seeker_kind() {
            SeekerKind::SemiActiveRadar => Guidance::Supported,
            SeekerKind::ActiveRadar => Guidance::Active,
            SeekerKind::Infrared => Guidance::Infrared,
            SeekerKind::PassiveRadar => Guidance::Emitter,
            SeekerKind::Other => return None,
        };
        let active = if guidance == Guidance::Active {
            Some(active?)
        } else {
            None
        };
        let role = if SURFACE_ROLE.contains(&w.source.as_str())
            || SURFACE_TO_SURFACE.contains(&w.source.as_str())
        {
            TargetRole::Surface
        } else {
            TargetRole::Aircraft
        };
        Some(Self {
            role,
            guidance,
            activation_ft: active.map(|v| v * NMI),
            guidance_ticks: u64::from(w.movement.remove_t) * 30,
            memory_ticks: MEMORY,
            radar_emissions: guidance == Guidance::Emitter,
            // Fitted conservative receiver policy, no evidence for jammer homing.
            jammer_emissions: false,
        })
    }
    pub fn independent(self) -> bool {
        self.guidance != Guidance::Supported
    }
    pub fn guidance_available(self, radar_power: bool) -> bool {
        radar_power || self.guidance == Guidance::Infrared
    }
    /// Air-to-air independent missiles fire in BORE; of the air-to-ground
    /// ones the infrared missiles (the Maverick boresight, John accepted plan
    /// decision 10) and the anti-radiation missiles (the HARM boresight cue,
    /// John, 2026-10-10) do. docs/spec/air-to-ground.md, "Missiles and the
    /// HARM boresight cue".
    pub fn supports_boresight(self) -> bool {
        match self.role {
            TargetRole::Aircraft => self.independent(),
            TargetRole::Surface => matches!(self.guidance, Guidance::Infrared | Guidance::Emitter),
        }
    }
    /// A surface-target radar weapon (AGM-84A, AM-39, AS-16, AS-7, AS-15):
    /// retail releases it only with the A-G page on, and it takes its target
    /// from the A-G track (retail data, docs/formats/radar.md, "A-G page").
    pub fn needs_air_ground(self) -> bool {
        self.role == TargetRole::Surface
            && matches!(self.guidance, Guidance::Active | Guidance::Supported)
    }
    /// A surface-target supported radar weapon (AS-7, AS-15, flag 0x200): the
    /// launcher's A-G track must hold it, with the target inside the weapon's
    /// zone 1 from the launcher, for the whole flight; once lost it flies
    /// unguided (retail data). The player's new designation retargets it in
    /// flight when the new target is inside its zone-1 cone, and blinds it
    /// otherwise (`PROJRetargetMissiles`, docs/formats/missiles.md, "Target
    /// switch"). The air-to-air supported missiles keep today's rules.
    pub fn rides_air_ground(self) -> bool {
        self.role == TargetRole::Surface && self.guidance == Guidance::Supported
    }
    /// An anti-radiation surface missile in flight keeps homing on its
    /// acquired object after its radar stops, on angles only, while the object
    /// stays inside the weapon's zone-1 cone (retail data, docs/formats/radar.md,
    /// "HARM page").
    pub fn holds_on_angles(self) -> bool {
        self.role == TargetRole::Surface && self.guidance == Guidance::Emitter
    }
    pub fn accepts(self, target: &super::live::Target) -> bool {
        self.role == target.role && (self.role == TargetRole::Surface || target.airborne)
    }
    /// The bore circle: 5 degrees, except that an anti-radiation surface
    /// missile's BORE looks through its whole seeker cone (zone 0, 45 degrees
    /// on the AGM-88 and AGM-45), as its HUD cue shows every emitter there.
    pub fn search_cap(self) -> f64 {
        if self.holds_on_angles() {
            std::f64::consts::PI
        } else {
            5f64.to_radians()
        }
    }
}

pub fn phase(m: &Movement, age: u64) -> EnginePhase {
    if age < u64::from(m.ignite_t) * 30 {
        EnginePhase::BeforeIgnition
    } else if age < u64::from(m.fuel_t) * 30 {
        EnginePhase::Powered
    } else {
        EnginePhase::Coast
    }
}
pub fn removed(m: &Movement, age: u64) -> bool {
    age >= u64::from(m.remove_t) * 30
}

/// 0x7fff means unrestricted on that axis; other limits remain independent.
/// This fitted spherical-angle interpretation avoids tan(180) collapsing a cone.
pub fn half_angle(raw: i16) -> f64 {
    if raw == i16::MAX {
        std::f64::consts::PI
    } else {
        (f64::from(raw.max(0)) / 182.).to_radians()
    }
}
pub fn geometry(
    z: &Zone,
    position: Vector,
    basis: Basis,
    target: Vector,
    cap: Option<f64>,
) -> bool {
    let d = sub(target, position);
    let distance = length(d);
    let forward = dot(d, basis.forward);
    let heading = dot(d, basis.right).atan2(forward).abs();
    let elevation = dot(d, basis.up)
        .atan2(forward.hypot(dot(d, basis.right)))
        .abs();
    let limit = |raw| half_angle(raw).min(cap.unwrap_or(std::f64::consts::PI));
    let inside_circle =
        cap.is_none_or(|angle| distance > 0. && forward / distance >= angle.cos() - 1e-12);
    inside_circle
        && distance >= f64::from(z.minimum_range)
        && distance <= f64::from(z.maximum_range)
        && d[1] >= f64::from(z.minimum_altitude)
        && d[1] <= f64::from(z.maximum_altitude)
        && heading <= limit(z.heading) + 1e-12
        && elevation <= limit(z.pitch) + 1e-12
}

/// Fitted active missile radar response using the shared Advanced notch preset:
/// 60 ft/s half width and 0.45 centre range factor in full ground clutter.
/// The continuous range penalty feeds normal seeker memory, so entering the
/// notch does not immediately destroy or permanently defeat the missile.
pub fn active_radar_visible(
    w: &Weapon,
    position: Vector,
    basis: Basis,
    target: &super::live::Target,
    target_height_agl_ft: f64,
) -> bool {
    let nominal = f64::from(w.seeker.zones[0].maximum_range);
    // A surface-target seeker looks for ground objects and ships: the air
    // radar's clutter notch does not apply (fitted, docs/spec/air-to-ground.md,
    // "Weapon classes" 10).
    if Profile::for_weapon(w).is_some_and(|p| p.role == TargetRole::Surface) {
        let sighting = crate::sensors::detection::Sighting::new(position, &basis, target.position);
        let signature = target.signature.effective_radar(
            &target.basis,
            sub(position, target.position),
            target.configuration,
        );
        let range =
            crate::sensors::detection::effective_range_ft(nominal, signature / 100., 1., 1., 1.);
        return signature > 0. && sighting.distance_ft <= range;
    }
    let sighting = crate::sensors::detection::Sighting::new(position, &basis, target.position);
    let signature = target.signature.effective_radar(
        &target.basis,
        sub(position, target.position),
        target.configuration,
    );
    let clutter =
        crate::sensors::detection::clutter_exposure(&sighting, target_height_agl_ft.max(0.));
    let notch = crate::sensors::detection::notch_factor(
        &crate::sensors::Preset::Advanced.notch(),
        clutter,
        crate::sensors::detection::notch_speed_fps(&sighting, target.velocity),
    );
    let range =
        crate::sensors::detection::effective_range_ft(nominal, signature / 100., 1., notch, 1.);
    signature > 0. && sighting.distance_ft <= range
}
pub fn sub(a: Vector, b: Vector) -> Vector {
    std::array::from_fn(|i| a[i] - b[i])
}
pub fn length(v: Vector) -> f64 {
    dot(v, v).sqrt()
}
pub fn closure(position: Vector, velocity: Vector, target: Vector, target_velocity: Vector) -> f64 {
    dot(sub(velocity, target_velocity), unit(sub(target, position)))
}

/// Record flag: the weapon sags under gravity while its motor is unlit.
pub const SAG_FLAG: u32 = 0x4;
/// Record flag: the release ejects the weapon downward.
pub const EJECT_FLAG: u32 = 0x8;
/// Record flag: the weapon flies the record's cruise altitude profile.
pub const CRUISE_FLAG: u32 = 0x20;
/// Sag acceleration and cap, and the ejection kick, feet per second.
pub const SAG_FPS2: f64 = 32.;
pub const SAG_MAX_FPS: f64 = 80.;
pub const EJECT_FPS: f64 = 32.;
/// Record cruise distances and altitudes count 256-foot steps.
pub const CRUISE_UNIT_FT: f64 = 256.;
/// Fitted: an air-to-air lob climbs toward its cruise altitude over this
/// much horizontal distance, 2 nmi, instead of pointing straight at it.
pub const LOB_LOOKAHEAD_FT: f64 = 2. * NMI;

/// The record's cruise altitude profile: two distance stages, each holding
/// an altitude above the target, then direct homing. docs/spec/missiles.md#drop-launch-sag-and-cruise-profile
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cruise {
    /// Each stage's horizontal distance from the target and altitude above
    /// it, feet, outer stage first. Inside the inner distance it homes direct.
    pub stages: [(f64, f64); 2],
    /// Air-to-air lob: climb to the cruise altitude rather than aim at it.
    pub lob: bool,
}
impl Cruise {
    pub fn for_weapon(w: &Weapon) -> Option<Self> {
        let [d1, a1, d2, a2] = w.movement.cruise.map(|v| f64::from(v) * CRUISE_UNIT_FT);
        (w.flags & CRUISE_FLAG != 0 && d1.max(d2) > 0.).then(|| Self {
            stages: [(d1, a1), (d2, a2)],
            lob: Profile::for_weapon(w).is_some_and(|p| p.role == TargetRole::Aircraft),
        })
    }
    /// Where a missile at `position` steers for `intercept`.
    pub fn aim(self, position: Vector, intercept: Vector) -> Vector {
        let horizontal = [intercept[0] - position[0], 0., intercept[2] - position[2]];
        let distance = length(horizontal);
        let [(outer, outer_above), (inner, inner_above)] = self.stages;
        if distance < inner {
            return intercept;
        }
        let above = if distance < outer {
            inner_above
        } else {
            outer_above
        };
        let altitude = intercept[1] + above;
        if !self.lob || distance < 1. {
            return [intercept[0], altitude, intercept[2]];
        }
        let reach = distance.min(LOB_LOOKAHEAD_FT) / distance;
        [
            position[0] + horizontal[0] * reach,
            altitude,
            position[2] + horizontal[2] * reach,
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub velocity: Vector,
    pub gain: f64,
    pub budget: f64,
    /// Downward sag while the motor is unlit, feet per second, applied to
    /// position apart from the flight path. Lighting the motor clears it.
    pub sink: f64,
    pub sags: bool,
    pub cruise: Option<Cruise>,
    /// Fitted: steering lays the missile's actual path, its velocity with
    /// the sag, on the aim point instead of its velocity alone, so the sag
    /// cannot carry it under the aim point. Every sagging missile opts in,
    /// as does surface guidance. See [`Motion::path`].
    pub holds_path: bool,
    pub energy: Option<energy_profile::Motion>,
}
impl Motion {
    pub fn new(m: &Movement, velocity: Vector, altitude: f64) -> Self {
        let maximum = altitude_performance(
            m.maximum_speed,
            m.performance_at_0,
            m.performance_at_20,
            (altitude * 256.) as i32,
        );
        Self {
            velocity,
            gain: 0.,
            budget: (f64::from(maximum) - f64::from(m.initial_speed)).max(0.),
            sink: 0.,
            sags: false,
            cruise: None,
            holds_path: false,
            energy: None,
        }
    }
    /// A release of `w`: the record's ejection, sag and cruise profile.
    pub fn launch(w: &Weapon, velocity: Vector, altitude: f64) -> Self {
        Self {
            sink: if w.flags & EJECT_FLAG != 0 {
                EJECT_FPS
            } else {
                0.
            },
            sags: w.flags & SAG_FLAG != 0,
            cruise: Cruise::for_weapon(w),
            holds_path: w.flags & SAG_FLAG != 0
                || Profile::for_weapon(w).is_some_and(|p| p.role == TargetRole::Surface)
                || Guidance::air_to_ground(w).is_some(),
            ..Self::new(&w.movement, velocity, altitude)
        }
    }
    pub fn launch_rules(w: &Weapon, velocity: Vector, altitude: f64, rules: Rules) -> Self {
        let mut motion = Self::launch(w, velocity, altitude);
        if rules == Rules::Energy && w.movement.fuel_t > w.movement.ignite_t {
            let mut velocity = velocity.map(|v| v * energy::FT);
            if !surface_launched(&w.source) {
                velocity[1] -= motion.sink * energy::FT;
            }
            motion.energy = Some(energy_profile::Motion {
                body: energy_profile::body(w),
                flight: energy::Flight::launch(
                    [0., altitude * energy::FT, 0.],
                    velocity,
                    [0., 0., 1.],
                ),
                target: None,
                observed: false,
                wind: [0.; 3],
                decoy: Default::default(),
                tracking: Default::default(),
            });
            motion.velocity = velocity.map(|v| v / energy::FT);
            motion.sink = 0.;
            motion.sags = false;
        }
        motion
    }
    pub fn prepare(&mut self, position: Vector, axis: Vector, age: u64, wind: Vector) {
        if let Some(e) = &mut self.energy {
            e.flight.position = position.map(|v| v * energy::FT);
            if age == 0 {
                e.flight.axis = e.body.launch_axis(axis);
            }
            e.flight.age = age.min(u64::from(u32::MAX - 1)) as u32;
            e.wind = wind.map(|v| v * energy::FT);
            e.target = None;
            e.observed = false;
        }
    }
    pub fn track(&mut self, position: Vector, velocity: Vector) {
        if let Some(e) = &mut self.energy {
            e.observed = true;
            e.target = Some(energy::Observation {
                position: position.map(|v| v * energy::FT),
                velocity: velocity.map(|v| v * energy::FT),
            });
        }
    }
    pub fn engine_phase(&self, m: &Movement, age: u64) -> EnginePhase {
        self.energy.map_or_else(
            || phase(m, age),
            |e| {
                if age < u64::from(e.body.ignition) {
                    EnginePhase::BeforeIgnition
                } else if e.body.motor(age as u32).is_some() {
                    EnginePhase::Powered
                } else {
                    EnginePhase::Coast
                }
            },
        )
    }
    /// The way the missile is travelling, for steering. Its velocity, and
    /// for a missile that holds its path the sag as well: the sag moves it
    /// straight down apart from its velocity, so a missile that only pointed
    /// its velocity at a target would sink under its line of sight and can
    /// meet the ground short of a low target (docs/spec/missiles.md,
    /// "Drop launch, sag and cruise profile").
    pub fn path(&self) -> Vector {
        if self.holds_path {
            [
                self.velocity[0],
                self.velocity[1] - self.sink,
                self.velocity[2],
            ]
        } else {
            self.velocity
        }
    }
    /// Where the missile steers for `intercept`.
    pub fn aim(&self, position: Vector, intercept: Vector) -> Vector {
        self.cruise
            .map_or(intercept, |c| c.aim(position, intercept))
    }
    /// Steering rotates inherited velocity and applies fitted maneuver loss. No change of rail direction
    /// means full inherited climb/side-slip survives the unsteered boost.
    pub fn turn(&mut self, old: Vector, new: Vector) {
        if self.energy.is_some() {
            return;
        }
        let axis = crate::attitude::cross(old, new);
        let angle = length(axis).atan2(dot(old, new));
        if angle > 1e-12 {
            let b = Basis {
                right: [1., 0., 0.],
                up: [0., 1., 0.],
                forward: [0., 0., 1.],
            }
            .rotated(unit(axis).map(|v| v * angle));
            let loss = (-0.03 * angle * angle / DT).exp();
            self.velocity = std::array::from_fn(|i| {
                (b.right[i] * self.velocity[0]
                    + b.up[i] * self.velocity[1]
                    + b.forward[i] * self.velocity[2])
                    * loss
            });
        }
    }
    pub fn step(&mut self, m: &Movement, age: u64, forward: Vector) -> Vector {
        self.step_over_terrain(m, age, forward, &|_, _| 0.)
    }
    pub fn step_over_terrain(
        &mut self,
        m: &Movement,
        age: u64,
        forward: Vector,
        ground: &impl Fn(f64, f64) -> f64,
    ) -> Vector {
        if let Some(e) = &mut self.energy {
            let before = e.flight.position;
            e.flight.step_terrain(e.body, e.wind, e.target, &|x, z| {
                ground(x / energy::FT, z / energy::FT) * energy::FT
            });
            self.velocity = e.flight.velocity.map(|v| v / energy::FT);
            return std::array::from_fn(|i| (e.flight.position[i] - before[i]) / energy::FT);
        }
        match phase(m, age) {
            EnginePhase::Powered => {
                let gain =
                    (f64::from(m.acceleration.max(0)) * DT).min((self.budget - self.gain).max(0.));
                self.gain += gain;
                for (v, f) in self.velocity.iter_mut().zip(forward) {
                    *v += f * gain;
                }
            }
            EnginePhase::Coast => {
                let speed = length(self.velocity);
                let next = (speed - f64::from(m.deceleration.max(0)) * DT)
                    .max(f64::from(m.final_speed.max(0)).min(speed));
                if speed > 0. {
                    self.velocity = self.velocity.map(|v| v * next / speed);
                }
            }
            EnginePhase::BeforeIgnition => {}
        }
        let mut delta = self.velocity.map(|v| v * DT);
        if phase(m, age) == EnginePhase::Powered {
            self.sink = 0.;
        } else if self.sags {
            self.sink = (self.sink + SAG_FPS2 * DT).min(SAG_MAX_FPS);
        }
        delta[1] -= self.sink * DT;
        delta
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solution {
    pub point: Vector,
    pub seconds: f64,
}
/// Opinionated HUD estimate, not a calibrated probability or guidance permission.
pub fn estimated_hit_percent(
    observation: seeker::Observation,
    solution: Option<Solution>,
    zone: &Zone,
    lifetime_seconds: f64,
    bore_cap: Option<f64>,
) -> u8 {
    let Some(solution) = solution else {
        return 0;
    };
    let min = f64::from(zone.minimum_range);
    let max = f64::from(zone.maximum_range);
    if max <= min || !(min..=max).contains(&observation.range) || lifetime_seconds <= 0. {
        return 0;
    }
    let r = ((observation.range - min) / (max - min)).clamp(0., 1.);
    let centring = bore_cap.map_or(1., |cap| seeker::centre_weight(observation.off_axis, cap));
    let margin = (1. - 0.6 * solution.seconds / lifetime_seconds).clamp(0., 1.);
    (95. * observation.quality.clamp(0., 1.) * centring * (1. - 0.75 * r * r) * margin)
        .round()
        .clamp(0., 95.) as u8
}

/// Constant-speed lead from observed motion only. No target state lookup.
pub fn lead(position: Vector, speed: f64, target: Vector, velocity: Vector) -> Solution {
    let r = sub(target, position);
    let a = dot(velocity, velocity) - speed * speed;
    let b = 2. * dot(r, velocity);
    let c = dot(r, r);
    let mut seconds = 0.;
    if a.abs() < 1e-9 {
        if b < -1e-9 {
            seconds = -c / b;
        }
    } else {
        let discriminant = b * b - 4. * a * c;
        if discriminant >= 0. {
            seconds = [
                (-b - discriminant.sqrt()) / (2. * a),
                (-b + discriminant.sqrt()) / (2. * a),
            ]
            .into_iter()
            .filter(|t| *t >= 0. && t.is_finite())
            .min_by(f64::total_cmp)
            .unwrap_or(0.);
        }
    }
    Solution {
        point: std::array::from_fn(|i| target[i] + velocity[i] * seconds),
        seconds,
    }
}

/// Correct inherited slip/climb using flight-path error, not nose error alone.
pub fn commanded_heading(forward: Vector, velocity: Vector, desired: Vector) -> Vector {
    if length(velocity) < 1. {
        return desired;
    }
    let flight_path = unit(velocity);
    unit(std::array::from_fn(|i| {
        forward[i] + desired[i] - flight_path[i]
    }))
}

/// Same exact angular limit in prediction and live guidance.
pub fn steer(m: &Movement, age: u64, forward: Vector, desired: Vector) -> Vector {
    let angle = dot(forward, desired).clamp(-1., 1.).acos();
    // Fins are locked until the motor lights.
    let rate = match phase(m, age) {
        EnginePhase::BeforeIgnition => 0,
        EnginePhase::Powered => m.powered_turn_rate,
        EnginePhase::Coast => m.unpowered_turn_rate,
    };
    let step = (f64::from(rate.max(0)) * std::f64::consts::TAU / 65520. * DT).min(angle);
    if angle < 1e-12 || step <= 0. {
        return forward;
    }
    let mut axis = crate::attitude::cross(forward, desired);
    if length(axis) < 1e-9 {
        axis = crate::attitude::cross(
            forward,
            if forward[1].abs() < 0.9 {
                [0., 1., 0.]
            } else {
                [1., 0., 0.]
            },
        );
    }
    let axis = unit(axis);
    let tangent = crate::attitude::cross(axis, forward);
    unit(std::array::from_fn(|i| {
        forward[i] * step.cos() + tangent[i] * step.sin()
    }))
}

/// How often, in ticks, the fly-out of [`intercept`] checks whether the
/// target has fallen out of reach.
const REACH_CHECK_TICKS: u64 = 16;

/// Fitted 120 Hz flyout with observed constant-velocity target and shared physics.
#[allow(clippy::too_many_arguments)]
pub fn intercept(
    m: &Movement,
    motion: Motion,
    position: Vector,
    forward: Vector,
    target: Vector,
    velocity: Vector,
    age: u64,
    lifetime: u64,
) -> Option<Solution> {
    intercept_with_terrain(
        m, motion, position, forward, target, velocity, age, lifetime, None,
    )
}

/// The answers of [`intercept_kept`] kept per thread: the launch-time
/// fly-out of the selected weapon, asked by the release check, the cockpit
/// readout and the mission recorder of the same state.
const KEPT_SOLUTIONS: usize = 4;

/// Everything a launch-time fly-out of [`intercept`] depends on, compared bit
/// for bit, so a kept answer is only ever returned for the same question.
#[derive(Clone, Copy, PartialEq)]
struct FlyOut {
    movement: Movement,
    motion: [[u64; 3]; 2],
    flags: [bool; 2],
    cruise: Option<([[u64; 2]; 2], bool)>,
    vectors: [[u64; 3]; 4],
    age: u64,
    lifetime: u64,
    energy: Option<energy_profile::Motion>,
}

type KeptSolutions = (Vec<(FlyOut, Option<Solution>)>, usize);

thread_local! {
    static SOLUTIONS: std::cell::RefCell<KeptSolutions> =
        const { std::cell::RefCell::new((Vec::new(), 0)) };
}

/// [`intercept`], kept: the same question asked again on this thread, by the
/// release check, the cockpit readout or the recorder, reads the answer
/// worked out the first time instead of flying it again. The answer is the
/// same bit for bit, since the fly-out reads nothing but its arguments.
/// Interpolated frame launchers remain distinct questions; never substitute
/// a simulation-tick launcher to make their answers share this cache.
#[allow(clippy::too_many_arguments)]
pub fn intercept_kept(
    m: &Movement,
    motion: Motion,
    position: Vector,
    forward: Vector,
    target: Vector,
    velocity: Vector,
    age: u64,
    lifetime: u64,
) -> Option<Solution> {
    let bits = |v: Vector| v.map(f64::to_bits);
    // Named field by field, so a field added to the motion joins the key.
    let Motion {
        velocity: speed,
        gain,
        budget,
        sink,
        sags,
        cruise,
        holds_path,
        energy,
    } = motion;
    let key = FlyOut {
        movement: *m,
        motion: [bits(speed), [gain, budget, sink].map(f64::to_bits)],
        flags: [sags, holds_path],
        cruise: cruise.map(|Cruise { stages, lob }| {
            (
                stages.map(|(distance, above)| [distance, above].map(f64::to_bits)),
                lob,
            )
        }),
        vectors: [bits(position), bits(forward), bits(target), bits(velocity)],
        age,
        lifetime,
        energy,
    };
    if let Some(kept) = SOLUTIONS.with(|kept| {
        kept.borrow()
            .0
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, solution)| *solution)
    }) {
        return kept;
    }
    let solution = intercept(
        m, motion, position, forward, target, velocity, age, lifetime,
    );
    SOLUTIONS.with(|kept| {
        let (entries, next) = &mut *kept.borrow_mut();
        if entries.len() < KEPT_SOLUTIONS {
            entries.push((key, solution));
        } else {
            entries[*next] = (key, solution);
            *next = (*next + 1) % KEPT_SOLUTIONS;
        }
    });
    solution
}

/// The same fly-out with cruise terrain clearance. The energy-only launch
/// envelope calls `intercept`; time-to-impact uses the actual terrain.
#[allow(clippy::too_many_arguments)]
pub fn intercept_with_terrain(
    m: &Movement,
    mut motion: Motion,
    mut position: Vector,
    mut forward: Vector,
    mut target: Vector,
    velocity: Vector,
    age: u64,
    lifetime: u64,
    ground: Option<&dyn Fn(f64, f64) -> f64>,
) -> Option<Solution> {
    let end = lifetime.min(u64::from(m.remove_t) * 30);
    let mut aim = target;
    for tick in age..end {
        if tick == age || tick.is_multiple_of(12) {
            aim = lead(position, length(motion.velocity), target, velocity).point;
        }
        let previous = sub(target, position);
        // The most the gap can still close is fixed by the missile's speed
        // now, the motor's unspent push and the target's known straight line:
        // a target farther off than that, plus the 25 feet that count as a
        // hit, is never reached, and the rest of the fly-out could only say
        // no. See `out_of_reach`.
        if (tick - age).is_multiple_of(REACH_CHECK_TICKS)
            && out_of_reach(m, &motion, previous, velocity, end - tick, tick)
        {
            #[cfg(test)]
            note_flown(tick - age);
            return None;
        }
        let desired = commanded_heading(
            forward,
            motion.path(),
            unit(sub(
                ground.map_or_else(
                    || motion.aim(position, aim),
                    |g| motion.aim_over_terrain(position, aim, g),
                ),
                position,
            )),
        );
        let next = steer(m, tick, forward, desired);
        motion.turn(forward, next);
        forward = next;
        let delta = motion.step(m, tick, forward);
        for i in 0..3 {
            position[i] += delta[i];
            target[i] += velocity[i] * DT;
        }
        let relative = sub(target, position);
        let segment = sub(relative, previous);
        let u = (-dot(previous, segment) / dot(segment, segment).max(1e-12)).clamp(0., 1.);
        let closest = std::array::from_fn(|i| previous[i] + segment[i] * u);
        if length(closest) <= 25. {
            #[cfg(test)]
            note_flown(tick - age + 1);
            return Some(Solution {
                point: target,
                seconds: (tick - age + 1) as f64 * DT,
            });
        }
    }
    #[cfg(test)]
    note_flown(end.saturating_sub(age));
    None
}

/// Whether the rest of a fly-out, `ticks` more ticks from `age`, provably
/// never brings the missile within the 25 feet of the target that count as a
/// hit, with `gap` the target's offset from the missile now and
/// `target_velocity` its constant velocity.
///
/// Measured along the line to the target now, the gap after any number of
/// ticks is at least its length now, plus the target's own movement along
/// that line, less the most the missile can have moved: so the gap can only
/// close by the sum, over the ticks, of how much the missile's greatest
/// possible speed outruns the target's speed along the line. Each tick's
/// speed is bounded by phase: before ignition it never rises (a turn only
/// costs speed); while the motor burns it rises by no more than the push
/// still unspent; once the motor is out it falls by the slowdown each tick
/// to the final speed, as [`Motion::step`] flies it; the sag or ejection
/// speed counts on top. The closest approach of each tick's segment, which
/// is what [`intercept`] tests, lies between the ends' distances along the
/// line, so it is bounded the same way. The margin covers rounding in the
/// fly-out.
fn out_of_reach(
    m: &Movement,
    motion: &Motion,
    gap: Vector,
    target_velocity: Vector,
    ticks: u64,
    age: u64,
) -> bool {
    let range = length(gap);
    if range <= 25. {
        return false;
    }
    let line = gap.map(|v| v / range);
    // How fast the target draws away along the line (negative: closing),
    // less the most the missile can drop toward it by sag or ejection.
    let sink = if motion.sags {
        motion.sink.abs().max(SAG_MAX_FPS)
    } else {
        motion.sink.abs()
    };
    let away = dot(target_velocity, line) - sink;
    // The ticks left in each phase.
    let end = age + ticks;
    let ignition = u64::from(m.ignite_t) * 30;
    let burnout = u64::from(m.fuel_t) * 30;
    let unlit = ignition.min(end).saturating_sub(age);
    let burning = burnout.min(end).saturating_sub(age.max(ignition));
    let coasting = ticks - unlit - burning;
    let speed = length(motion.velocity);
    let push = if burning > 0 {
        (motion.budget - motion.gain).max(0.)
    } else {
        0.
    };
    let top = speed + push;
    let outrun = |speed: f64| (speed - away).max(0.);
    // Coasting from `top`: max(top - k slow, min(final, top)) on tick k.
    let slow = f64::from(m.deceleration.max(0)) * DT;
    let floor = f64::from(m.final_speed.max(0)).min(top);
    let falling = top - floor.max(away);
    let fall = if falling <= 0. || coasting == 0 {
        0.
    } else if slow <= 0. {
        falling * coasting as f64
    } else {
        // The ticks on which the falling part is still positive, and their
        // sum; two slowdowns cover rounding in the count.
        let count = ((falling / slow).floor() + 1.).min(coasting as f64);
        count * falling - slow * count * (count - 1.) * 0.5 + 2. * slow
    };
    let closing = (unlit as f64 * outrun(speed)
        + burning as f64 * outrun(top)
        + coasting as f64 * outrun(floor)
        + fall)
        * DT;
    let travel = (top + sink.abs() + length(target_velocity)) * ticks as f64 * DT;
    range > 25. + closing + 1e-6 * (range + travel) + 1e-3
}

#[cfg(test)]
thread_local! {
    static FLOWN_TICKS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn note_flown(ticks: u64) {
    FLOWN_TICKS.with(|flown| flown.set(flown.get() + ticks));
}
/// The ticks of missile fly-out [`intercept`] has run on this thread so far:
/// a count of the work the firing estimates cost, for tests to bound, since
/// the time it takes varies with the machine. Compiled for tests only.
#[cfg(test)]
pub fn flown_ticks() -> u64 {
    FLOWN_TICKS.with(|flown| flown.get())
}

/// Player launch angles/altitude, without advisory minimum or fixed maximum.
pub fn launch_geometry(w: &Weapon) -> Zone {
    Zone {
        minimum_range: 0,
        maximum_range: i32::MAX,
        ..w.seeker.zones[1]
    }
}

/// How many ticks a fly-out of `intercept` runs at most: the guidance time or
/// the weapon's removal time, whichever is first.
fn lifetime_ticks(m: &Movement, lifetime: u64) -> u64 {
    lifetime.min(u64::from(m.remove_t) * 30)
}

/// The farthest a missile launched with `motion` can possibly fly in a whole
/// fly-out of `intercept` (feet), however it steers. Each tick's speed is
/// bounded by the speed the motor could have reached with every bit of thrust
/// spent in the line (steering only loses speed), the coast slowdown applied
/// as it is flown, and the largest downward sag or ejection speed on top.
fn travel_ceiling(m: &Movement, motion: &Motion, lifetime: u64) -> f64 {
    let sink = SAG_MAX_FPS.max(EJECT_FPS).max(motion.sink);
    let mut speed = length(motion.velocity);
    let mut gain = motion.gain;
    let mut total = 0.;
    for tick in 0..lifetime_ticks(m, lifetime) {
        match phase(m, tick) {
            EnginePhase::Powered => {
                let step =
                    (f64::from(m.acceleration.max(0)) * DT).min((motion.budget - gain).max(0.));
                gain += step;
                speed += step;
            }
            EnginePhase::Coast => {
                speed = (speed - f64::from(m.deceleration.max(0)) * DT)
                    .max(f64::from(m.final_speed.max(0)).min(speed));
            }
            EnginePhase::BeforeIgnition => {}
        }
        total += (speed + sink) * DT;
    }
    total
}

/// Whether a target `range` feet away is provably out of the reach of a
/// fly-out that covers at most `travel` feet between the missile and the
/// target together: `intercept` only succeeds once the closest approach is
/// within 25 feet, and each tick closes the gap by no more than the two
/// objects' own movement. The margin covers rounding in the fly-out.
fn unreachable(range: f64, travel: f64) -> bool {
    range > 25. + travel + 1e-6 * (range + travel) + 1e-3
}

/// Max useful launch distance bounded by available motion/time, not nominal max.
#[allow(clippy::too_many_arguments)]
pub fn maximum_range(
    w: &Weapon,
    position: Vector,
    forward: Vector,
    velocity: Vector,
    target: Vector,
    target_velocity: Vector,
    lifetime: u64,
) -> f64 {
    let minimum = f64::from(w.seeker.zones[1].minimum_range.max(0));
    let motion = Motion::launch(w, velocity, position[1]);
    let seconds = lifetime.min(u64::from(w.movement.remove_t) * 30) as f64 * DT;
    // A conservative search ceiling: both objects' maximum possible travel.
    // It is only a bound for the solve, never the displayed range itself.
    let travel_bound = (length(velocity) + motion.budget + length(target_velocity)) * seconds + 25.;
    let cap = if Profile::for_weapon(w).is_some_and(|p| p.guidance == Guidance::Active) {
        travel_bound
    } else {
        // IR/emitter/supported seekers must measure before independent steering.
        travel_bound.min(f64::from(w.seeker.zones[0].maximum_range))
    };
    if cap <= minimum {
        return 0.;
    }
    let bearing = unit(sub(target, position));
    // A target the missile cannot cover in its whole flight needs no fly-out:
    // `intercept` could only run to the end and say no. See `unreachable`.
    let ceiling = travel_ceiling(
        &w.movement,
        &Motion::launch(w, velocity, position[1]),
        lifetime,
    );
    let travel =
        ceiling + length(target_velocity) * lifetime_ticks(&w.movement, lifetime) as f64 * DT;
    let reaches = |range: f64| {
        if unreachable(range, travel) {
            return false;
        }
        intercept(
            &w.movement,
            Motion::launch(w, velocity, position[1]),
            position,
            forward,
            std::array::from_fn(|i| position[i] + bearing[i] * range),
            target_velocity,
            0,
            lifetime,
        )
        .is_some()
    };
    if reaches(cap) {
        return cap;
    }
    let step = (cap - minimum) / 16.;
    for sample in (0..16).rev() {
        let mut low = minimum + f64::from(sample) * step;
        if !reaches(low) {
            continue;
        }
        let mut high = low + step;
        for _ in 0..10 {
            let middle = (low + high) * 0.5;
            if reaches(middle) {
                low = middle;
            } else {
                high = middle;
            }
        }
        return low;
    }
    0.
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FiringBand {
    pub minimum: f64,
    pub maximum: f64,
}
/// Fitted favorable interval, not a guaranteed kill or calibrated no-escape zone.
#[allow(clippy::too_many_arguments)]
pub fn firing_band(
    w: &Weapon,
    position: Vector,
    basis: Basis,
    velocity: Vector,
    observation: seeker::Observation,
    maximum: f64,
    lifetime: u64,
    bore: bool,
) -> Option<FiringBand> {
    let minimum = f64::from(w.seeker.zones[1].minimum_range.max(0));
    if maximum <= minimum {
        return None;
    }
    let profile = Profile::for_weapon(w)?;
    let lower = minimum + 0.1 * (maximum - minimum);
    let step = (maximum - lower) / 16.;
    let bearing = unit(sub(observation.position, position));
    let mut zone = w.seeker.zones[1];
    zone.maximum_range = maximum.floor() as _;
    let mut start = None;
    let mut best: Option<FiringBand> = None;
    let bore_cap = bore.then(|| profile.search_cap());
    let life = lifetime_ticks(&w.movement, lifetime);
    let life_seconds = life as f64 * DT;
    for sample in 0..=16 {
        let range = lower + f64::from(sample) * step;
        let target = std::array::from_fn(|i| position[i] + bearing[i] * range);
        let observed = seeker::Observation {
            position: target,
            range,
            ..observation
        };
        let score_of =
            |solution| estimated_hit_percent(observed, solution, &zone, life_seconds, bore_cap);
        // Only a score of 70 or more counts, and a later interception scores
        // no higher than an earlier one. So the fly-out need only run as long
        // as an interception can still score 70, which is exact: a shot that
        // would hit later scores below 70 either way.
        let late = |ticks: u64| {
            score_of(Some(Solution {
                point: target,
                seconds: ticks as f64 * DT,
            }))
        };
        let horizon = if life == 0 || late(1) < 70 {
            0
        } else {
            let (mut good, mut bad) = (1, life + 1);
            while bad - good > 1 {
                let middle = good + (bad - good) / 2;
                if late(middle) >= 70 {
                    good = middle;
                } else {
                    bad = middle;
                }
            }
            good
        };
        let solution =
            if horizon > 0 && geometry(&launch_geometry(w), position, basis, target, None) {
                intercept(
                    &w.movement,
                    Motion::launch(w, velocity, position[1]),
                    position,
                    basis.forward,
                    target,
                    observation.velocity,
                    0,
                    horizon,
                )
            } else {
                None
            };
        let score = score_of(solution);
        if score >= 70 {
            let first = *start.get_or_insert(range);
            if range > first && best.is_none_or(|b| range - first > b.maximum - b.minimum) {
                best = Some(FiringBand {
                    minimum: first,
                    maximum: range,
                });
            }
        } else {
            start = None;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_laser_weapons_have_a_seeker_view_and_the_jdams_do_not() {
        // John 2026-10-13: GBUs, Mavericks and AT-12s have nose views; the
        // JDAMs stay out (2026-10-12).
        for laser in SPOT_HOMING {
            assert!(has_seeker_view(laser) && homes_on_spot(laser), "{laser}");
        }
        for jdam in ["GBU29.JT", "GBU29P.JT", "GBU30.JT", "GBU38.JT"] {
            assert!(!has_seeker_view(jdam) && !homes_on_spot(jdam), "{jdam}");
        }
        assert!(has_seeker_view("AGM65G.JT") && !homes_on_spot("AGM65G.JT"));
        assert!(!has_seeker_view("MK82.JT") && !has_seeker_view("AGM88.JT"));
    }

    fn movement() -> Movement {
        Movement {
            minimum_speed: 0,
            corner_speed: 0,
            maximum_speed: 1000,
            acceleration: 1000,
            deceleration: 100,
            initial_speed: 0,
            final_speed: 200,
            launch_retard: 100,
            ignite_t: 4,
            fuel_t: 12,
            remove_t: 80,
            powered_turn_rate: 1000,
            unpowered_turn_rate: 1000,
            performance_at_0: 100,
            performance_at_20: 100,
            cruise: [0; 4],
            jink: [0; 3],
        }
    }
    #[test]
    fn inherited_vector_finite_boost_and_closure() {
        let m = movement();
        let mut motion = Motion::new(&m, [40., 60., 600.], 10000.);
        for age in 0..360 {
            motion.step(&m, age, [0., 0., 1.]);
        }
        assert!(length(sub(motion.velocity, [40., 60., 1600.])) < 1e-8);
        assert_eq!(motion.gain, 1000.);
        assert!(
            (closure([0.; 3], motion.velocity, [0., 0., 10000.], [0., 0., -300.]) - 1900.).abs()
                < 1e-8
        );
        assert!(
            (closure([0.; 3], motion.velocity, [0., 0., 10000.], [0., 0., 300.]) - 1300.).abs()
                < 1e-8
        );
        assert_eq!(phase(&m, 119), EnginePhase::BeforeIgnition);
        assert_eq!(phase(&m, 120), EnginePhase::Powered);
        assert_eq!(phase(&m, 360), EnginePhase::Coast);
        motion.step(&m, 360, [0., 0., 1.]);
        assert!(length(motion.velocity) < length([40., 60., 1600.]));
    }
    /// The AIM54C.JT motor and turn numbers, with the retail release flags.
    fn phoenix() -> (Movement, Motion) {
        let m = Movement {
            maximum_speed: 5866,
            acceleration: 733,
            deceleration: 146,
            final_speed: 1026,
            ignite_t: 8,
            fuel_t: 564,
            remove_t: 1132,
            powered_turn_rate: 10920,
            unpowered_turn_rate: 8190,
            performance_at_0: 75,
            cruise: [78, 20, 78, 20],
            ..movement()
        };
        let motion = Motion {
            sink: EJECT_FPS,
            sags: true,
            cruise: Some(Cruise {
                stages: [(78. * 256., 20. * 256.); 2],
                lob: true,
            }),
            ..Motion::new(&m, [0., 0., 900.], 20000.)
        };
        (m, motion)
    }
    #[test]
    fn drop_falls_unsteered_until_the_motor_lights() {
        let (m, mut motion) = phoenix();
        let mut position = [0., 20000., 0.];
        let forward = [0., 0., 1.];
        // A target up and to the side cannot turn the unlit missile.
        assert_eq!(steer(&m, 0, forward, unit([1., 1., 1.])), forward);
        for age in 0..240 {
            let delta = motion.step(&m, age, forward);
            for i in 0..3 {
                position[i] += delta[i];
            }
        }
        // 32 ft/s kick, 32 ft/s² sag capped at 80 ft/s: about 124 ft in 2 s.
        assert!((20000. - position[1] - 124.).abs() < 1., "{}", position[1]);
        assert_eq!(motion.sink, SAG_MAX_FPS);
        assert_eq!(motion.velocity, [0., 0., 900.]);
        // Lighting the motor ends the sag at once and frees the fins.
        let delta = motion.step(&m, 240, forward);
        assert_eq!(motion.sink, 0.);
        assert!(delta[1].abs() < 1e-12);
        assert_ne!(steer(&m, 240, forward, unit([1., 1., 1.])), forward);
        // A rail missile's kick is gone on its first powered step.
        let mut rail = Motion {
            sink: EJECT_FPS,
            sags: true,
            ..Motion::new(&movement(), [0., 0., 900.], 0.)
        };
        rail.step(
            &Movement {
                ignite_t: 0,
                ..movement()
            },
            0,
            forward,
        );
        assert_eq!(rail.sink, 0.);
        // Burnout brings the sag back.
        let mut coast = Motion {
            sags: true,
            ..Motion::new(&m, [0., 0., 900.], 0.)
        };
        coast.step(&m, 564 * 30, forward);
        assert!((coast.sink - SAG_FPS2 * DT).abs() < 1e-12);
    }
    #[test]
    fn path_accounts_for_sink_only_when_holding_the_path() {
        let mut motion = Motion {
            sink: 80.,
            ..Motion::new(&movement(), [40., -60., 953.], 1024.)
        };
        assert_eq!(motion.path(), [40., -60., 953.]);
        motion.holds_path = true;
        assert_eq!(motion.path(), [40., -140., 953.]);
        motion.sink = 0.;
        assert_eq!(motion.path(), motion.velocity);
    }

    /// Coast along a commanded dive line from cruise height with full sag.
    /// Return the closest approach and the lowest altitude before arrival.
    fn sag_dive(holds_path: bool) -> (f64, f64) {
        let m = Movement {
            ignite_t: 0,
            fuel_t: 0,
            final_speed: 953,
            unpowered_turn_rate: 8190,
            ..movement()
        };
        let mut position = [0., 1024., 0.];
        let target = [0., 0., 7424.];
        let desired_path = unit(sub(target, position));
        let mut forward = desired_path;
        let mut motion = Motion {
            sink: 80.,
            sags: true,
            holds_path,
            ..Motion::new(&m, forward.map(|v| v * 953.), position[1])
        };
        let mut closest = length(sub(target, position));
        let mut lowest = position[1];
        for age in 0..12 * HZ {
            // Keep the commanded line fixed to isolate path correction from
            // pursuit. The live Maverick test covers a changing sight line.
            let desired = commanded_heading(forward, motion.path(), desired_path);
            let next = steer(&m, age, forward, desired);
            motion.turn(forward, next);
            forward = next;
            let delta = motion.step(&m, age, forward);
            position = std::array::from_fn(|i| position[i] + delta[i]);
            closest = closest.min(length(sub(target, position)));
            lowest = lowest.min(position[1]);
            if closest <= 25. || position[2] >= target[2] {
                break;
            }
        }
        (closest, lowest)
    }

    #[test]
    fn a_coasting_dive_holds_its_path_against_the_sag() {
        let (miss, low) = sag_dive(false);
        assert!(
            low < -100.,
            "velocity-only steering dips underground: {low}"
        );
        assert!(miss > 25., "velocity-only steering misses: {miss}");
        let (miss, low) = sag_dive(true);
        assert!(miss <= 25., "path holding reaches the ground point: {miss}");
        assert!(low >= -25., "path holding stays above the ground: {low}");
    }

    #[test]
    fn cruise_stages_hold_altitude_above_the_target_then_home_direct() {
        let harpoon = Cruise {
            stages: [(78. * 256., 4. * 256.), (20. * 256., 12. * 256.)],
            lob: false,
        };
        let target = [0., 0., 0.];
        let aim = |z: f64| harpoon.aim([0., 500., z], target);
        assert_eq!(aim(30000.), [0., 1024., 0.]);
        assert_eq!(aim(10000.), [0., 3072., 0.]);
        assert_eq!(aim(5000.), target);
        // Distance is horizontal only.
        assert_eq!(harpoon.aim([0., 90000., 19968.], target), [0., 1024., 0.]);
        // A lob climbs over the look-ahead, not along the whole distance.
        let lob = Cruise {
            lob: true,
            ..harpoon
        };
        let point = lob.aim([0., 0., 60760.], target);
        assert!((point[2] - (60760. - LOB_LOOKAHEAD_FT)).abs() < 1e-9);
        assert_eq!(point[1], 1024.);
        assert_eq!(lob.aim([0., 0., 5000.], target), target);
    }
    #[test]
    fn phoenix_lob_climbs_cruises_and_dives_onto_the_target() {
        let (m, mut motion) = phoenix();
        let target = [0., 20000., 30. * NMI];
        let mut position = [0., 20000., 0.];
        let mut forward = [0., 0., 1.];
        let mut highest: f64 = 0.;
        let mut lowest_drop: f64 = position[1];
        for age in 0..u64::from(m.remove_t) * 30 {
            let desired = commanded_heading(
                forward,
                motion.velocity,
                unit(sub(motion.aim(position, target), position)),
            );
            let next = steer(&m, age, forward, desired);
            motion.turn(forward, next);
            forward = next;
            let delta = motion.step(&m, age, forward);
            for i in 0..3 {
                position[i] += delta[i];
            }
            if age < 240 {
                lowest_drop = lowest_drop.min(position[1]);
            }
            highest = highest.max(position[1]);
            if length(sub(target, position)) < 100. {
                break;
            }
        }
        assert!(lowest_drop < 19880.);
        // It levels near 5,120 ft above the target and still arrives.
        assert!((24900. ..25400.).contains(&highest), "{highest}");
        assert!(length(sub(target, position)) < 100.);
        // The climbing prediction reaches the same target.
        let (m, motion) = phoenix();
        assert!(
            intercept(
                &m,
                motion,
                [0., 20000., 0.],
                [0., 0., 1.],
                target,
                [0.; 3],
                0,
                1132 * 30
            )
            .is_some()
        );
    }
    #[test]
    fn prediction_leads_crossing_and_bounds_impossible_shots() {
        let mut m = movement();
        // Crossing interception now needs enough actual turn authority.
        m.powered_turn_rate = 6000;
        m.unpowered_turn_rate = 6000;
        let motion = Motion::new(&m, [0., 0., 600.], 10000.);
        let solution = intercept(
            &m,
            motion,
            [0.; 3],
            [0., 0., 1.],
            [0., 0., 3000.],
            [300., 0., 0.],
            0,
            2400,
        )
        .unwrap();
        assert!(solution.point[0] > 0.);
        assert!(
            intercept(
                &m,
                motion,
                [0.; 3],
                [0., 0., 1.],
                [0., 0., 3000.],
                [0., 0., 10000.],
                0,
                2400
            )
            .is_none()
        );
        assert!(
            intercept(
                &m,
                motion,
                [0.; 3],
                [0., 0., 1.],
                [0., 0., 3000.],
                [0.; 3],
                0,
                30
            )
            .is_none()
        );
        assert!(removed(&m, 65536 * 30));
        let mut early = m;
        early.remove_t = 8;
        assert!(removed(&early, 240));
        assert_eq!(phase(&early, 240), EnginePhase::Powered);
    }
    /// A kept fly-out answers only the very same question, bit for bit: the
    /// same question again reads the kept answer, and a question that differs
    /// in any argument, by as little as a sign of zero or the last bit of a
    /// position, or in the motor record or the motion, is flown afresh. Five
    /// questions in turn outrun the four kept and still answer correctly.
    #[test]
    fn a_kept_fly_out_answers_only_the_same_question() {
        SOLUTIONS.with(|kept| *kept.borrow_mut() = (Vec::new(), 0));
        let m = movement();
        let (phoenix, lob) = phoenix();
        let motion = Motion::new(&m, [0., 0., 600.], 10000.);
        let ask = |m: &Movement, motion: Motion, target: Vector, velocity: Vector, cached: bool| {
            let args = ([0.; 3], [0., 0., 1.], target, velocity, 0, 2400);
            let before = SOLUTIONS.with(|kept| kept.borrow().clone());
            let kept = intercept_kept(m, motion, args.0, args.1, args.2, args.3, args.4, args.5);
            assert_eq!(SOLUTIONS.with(|kept| *kept.borrow() == before), cached);
            let flown = intercept(m, motion, args.0, args.1, args.2, args.3, args.4, args.5);
            assert_eq!(kept, flown, "{target:?} {velocity:?}");
            kept
        };
        let near = [0., 0., 3000.];
        let first = ask(&m, motion, near, [0., 0., 300.], false);
        assert!(first.is_some());
        assert_eq!(ask(&m, motion, near, [0., 0., 300.], true), first);
        let mut slower = m;
        slower.acceleration -= 1;
        let next = f64::from_bits(near[2].to_bits() + 1);
        for _ in 0..2 {
            ask(&m, motion, near, [0., -0., 300.], false);
            ask(&m, motion, [0., 0., next], [0., 0., 300.], false);
            ask(&slower, motion, near, [0., 0., 300.], false);
            ask(
                &m,
                Motion {
                    sags: true,
                    ..motion
                },
                near,
                [0., 0., 300.],
                false,
            );
            ask(&phoenix, lob, [0., 0., 60000.], [0., 0., -900.], false);
            ask(&m, motion, near, [0., 0., 300.], false);
        }
    }
    /// Three readers of each tick share one fly-out, including a cached miss.
    /// This measures a synthetic same-state workload, not a gameplay hit rate.
    #[test]
    fn repeated_readers_hit_the_cache_without_flying_again() {
        SOLUTIONS.with(|kept| *kept.borrow_mut() = (Vec::new(), 0));
        let m = movement();
        let motion = Motion::new(&m, [0., 0., 600.], 10000.);
        let mut hits = 0;
        for tick in 0..120 {
            let target = [0., 0., 3000. + f64::from(tick)];
            let expected = intercept(&m, motion, [0.; 3], [0., 0., 1.], target, [0.; 3], 0, 2400);
            assert!(expected.is_some());
            for reader in 0..3 {
                let before = flown_ticks();
                let got =
                    intercept_kept(&m, motion, [0.; 3], [0., 0., 1.], target, [0.; 3], 0, 2400);
                assert_eq!(got, expected);
                let cached = flown_ticks() == before;
                assert_eq!(cached, reader > 0);
                hits += usize::from(cached);
            }
        }
        assert_eq!(hits, 240);
        eprintln!("same-state cache: {hits}/360 hits (66.67%)");
        let target = [0., 0., 1e9];
        assert!(
            intercept_kept(&m, motion, [0.; 3], [0., 0., 1.], target, [0.; 3], 0, 2400).is_none()
        );
        SOLUTIONS.with(|kept| assert!(kept.borrow().0.iter().any(|(_, answer)| answer.is_none())));
        let before = flown_ticks();
        assert!(
            intercept_kept(&m, motion, [0.; 3], [0., 0., 1.], target, [0.; 3], 0, 2400).is_none()
        );
        assert_eq!(flown_ticks(), before);
    }
    #[test]
    fn independent_angles_and_wide_sentinel() {
        let z = Zone {
            heading: 32767,
            pitch: 182 * 90,
            minimum_range: 0,
            maximum_range: 10000,
            minimum_altitude: -10000,
            maximum_altitude: 10000,
        };
        let b = Basis::new(0., 0., 0.);
        assert!(geometry(&z, [0.; 3], b, [0., 0., -100.], None));
        let point = |angle: f64| {
            [
                1000. * angle.to_radians().sin(),
                0.,
                1000. * angle.to_radians().cos(),
            ]
        };
        assert!(geometry(&z, [0.; 3], b, point(3.), Some(3f64.to_radians())));
        assert!(!geometry(
            &z,
            [0.; 3],
            b,
            point(3.001),
            Some(3f64.to_radians())
        ));
    }
}

pub mod seeker;
#[derive(Clone, Debug, PartialEq)]
pub struct Flight {
    pub unguided: bool,
    pub launch_origin: Vector,
    /// SAM qualification metadata; aircraft-launched missiles no longer use it.
    pub qualified_target: Option<u32>,
    pub profile: Profile,
    pub mode: LaunchMode,
    pub seeker: seeker::Seeker,
    pub enabled: bool,
    pub last_intercept: Option<Vector>,
    pub solution: Option<Solution>,
}
impl Flight {
    pub fn eligible(&self, w: &Weapon, target: &super::live::Target) -> bool {
        self.profile.accepts(target)
            && (!surface_launched(&w.source)
                || self.qualified_target == Some(target.id)
                || length(sub(target.position, self.launch_origin))
                    >= f64::from(w.seeker.zones[1].minimum_range.max(0)))
    }

    pub fn new(
        profile: Profile,
        mode: LaunchMode,
        target: Option<u32>,
        launch_origin: Vector,
    ) -> Self {
        Self {
            unguided: false,
            launch_origin,
            qualified_target: None,
            profile,
            mode,
            seeker: seeker::Seeker::new(target),
            enabled: profile.guidance != Guidance::Active || mode == LaunchMode::Boresight,
            last_intercept: None,
            solution: None,
        }
    }

    /// Seed a cued release exclusively from the firing actor's observation.
    /// Later support may refresh this intercept, but hidden target state never
    /// enters the midcourse solution.
    pub fn from_supported_launch(
        profile: Profile,
        mode: LaunchMode,
        observation: seeker::Observation,
        launch_origin: Vector,
    ) -> Self {
        let mut flight = Self::new(profile, mode, Some(observation.id), launch_origin);
        flight.qualified_target = Some(observation.id);
        flight.last_intercept = Some(observation.position);
        flight.seeker.target = Some(observation.id);
        flight
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "missiles_checkpoint.rs"]
mod checkpoint;
