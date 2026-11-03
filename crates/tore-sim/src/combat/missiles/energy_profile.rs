//! Resolves authored physical metadata and explicit fitted class defaults.
//! No weapon-name flight branches. Calibration is recorded in the feature baseline.
use super::energy::{Body, FT, Motor};
use tore_formats::missile_motors::{ControlClass, DEFEAT_MACH, DEFEAT_TICKS};
use tore_formats::weapons::{SeekerKind, Weapon};

pub fn body(w: &Weapon) -> Body {
    let reference = tore_formats::missile_data::reference(&w.source);
    let surface = super::surface_launched(&w.source);
    let infrared = w.seeker_kind() == SeekerKind::Infrared;
    let mass = reference.map_or_else(
        || {
            if w.weight > 1 {
                f64::from(w.weight) * 0.45359237
            } else if infrared {
                15.
            } else {
                250.
            }
        },
        |r| r.mass_kg,
    );
    let diameter = reference.map_or_else(
        || (0.127 * (mass / 86.).cbrt()).clamp(0.07, 0.6),
        |r| r.diameter_m,
    );
    let range = reference.map_or(f64::from(w.seeker.zones[1].maximum_range) * FT, |r| {
        r.range_km * 1000.
    });
    let strike = w.flags & 0x20000 != 0;
    let cruise = strike && w.flags & super::CRUISE_FLAG != 0 && w.movement.fuel_t > 200;
    let fraction = if cruise { 0.20 } else { 0.35 };
    let fuel = mass * fraction;
    let burn = if cruise {
        3.
    } else {
        (0.2 * (range / 1000.).sqrt()).clamp(2., 6.)
    };
    let sustain = if cruise {
        (1.3 * range / 250.).clamp(30., 1800.)
    } else {
        (0.15 * range / 1000.).clamp(0., 25.)
    };
    let boost_ticks = (burn * 120.).round() as u32;
    let sustain_ticks = (sustain * 120.).round() as u32;
    let boost_fuel = if sustain_ticks == 0 {
        fuel
    } else {
        fuel * 0.65
    };
    let sustain_fuel = fuel - boost_fuel;
    let radar = matches!(
        w.seeker_kind(),
        SeekerKind::ActiveRadar | SeekerKind::SemiActiveRadar
    );
    // Unlisted synthetic/imported stores use role defaults, never range anchors.
    let class = tore_formats::missile_motors::motor(&w.source).map_or(
        if cruise {
            ControlClass::Cruise
        } else if super::Guidance::air_to_ground(w) == Some(super::Guidance::Command) {
            ControlClass::Command
        } else if strike {
            ControlClass::FastGround
        } else if surface && infrared {
            ControlClass::SurfaceIr
        } else if surface {
            ControlClass::SurfaceRadar
        } else if infrared {
            ControlClass::ShortIr
        } else {
            ControlClass::MediumAir
        },
        |m| m.control_class,
    );
    let mut body = Body {
        launch_elevation: if surface {
            12f64.to_radians()
        } else {
            -std::f64::consts::FRAC_PI_2
        },
        mass,
        diameter,
        boost: Motor {
            ticks: boost_ticks,
            propellant: boost_fuel,
            thrust: boost_fuel * 2200. / burn,
        },
        sustain: Motor {
            ticks: sustain_ticks,
            propellant: sustain_fuel,
            thrust: if sustain > 0. {
                sustain_fuel * if cruise { 10000. } else { 2200. } / sustain
            } else {
                0.
            },
        },
        ignition: if surface {
            0
        } else {
            u32::from(w.movement.ignite_t) * 30
        },
        life: (u32::from(w.movement.remove_t) * 30).max(if cruise {
            boost_ticks + sustain_ticks + 1200
        } else {
            0
        }),
        guidance_ticks: (class.seconds() * 120.) as u32,
        defeat_mach: if class.air_target() { DEFEAT_MACH } else { 0. },
        defeat_ticks: DEFEAT_TICKS,
        structural_g: if strike {
            12.
        } else if mass > 400. {
            20.
        } else {
            30.
        },
        lift_coefficient: 12.,
        induced_factor: 0.03,
        drag_scale: tore_formats::missile_data::fallback(&w.source).map_or(1., |r| r.drag_scale),
        cruise_drag: cruise,
        minimum_speed: if cruise
            || super::Guidance::air_to_ground(w) == Some(super::Guidance::Command)
        {
            70.
        } else {
            120.
        },
        navigation_gain: 3.5,
        loft_height: if !surface && !strike && radar && range >= 37040. {
            (range * 0.08).min(6000.)
        } else {
            0.
        },
        loft_range: range * 0.6,
        cruise_feet: strike.then(|| super::Cruise::for_weapon(w)).flatten(),
    };
    body.life = body.life.max((2. * range / 300. * 120.).ceil() as u32);
    if let Some(reference) = reference {
        body.drag_scale = reference.drag_scale;
        if let Some(g) = reference.structural_g {
            body.structural_g = g;
        }
        if reference.imaging && !strike {
            body.structural_g = 40.;
            body.navigation_gain = 4.;
        }
    }
    if let Some(m) = tore_formats::missile_motors::motor(&w.source) {
        body.guidance_ticks = (m.control_class.seconds() * 120.).round() as u32;
        if let Some(seconds) = m.ignition_seconds {
            body.ignition = (seconds * 120.).round() as u32;
        }
        let fuel = mass * m.propellant_fraction;
        let phase = |seconds: f64, share: f64, thrust: Option<f64>| Motor {
            ticks: (seconds * 120.).round() as u32,
            propellant: fuel * share,
            thrust: thrust.unwrap_or_else(|| {
                if seconds > 0. {
                    fuel * share * m.exhaust_mps / seconds
                } else {
                    0.
                }
            }),
        };
        body.boost = phase(m.boost_seconds, m.boost_share, m.thrust.map(|v| v[0]));
        body.sustain = phase(
            m.sustain_seconds,
            1. - m.boost_share,
            m.thrust.map(|v| v[1]),
        );
        body.life = body
            .life
            .max(body.ignition + body.boost.ticks + body.sustain.ticks + 1200);
    }
    body.life = body.life.max(body.guidance_ticks + 1200);
    body
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub body: Body,
    pub flight: super::energy::Flight,
    pub target: Option<super::energy::Observation>,
    /// Current qualified observation, as distinct from a frozen memory waypoint.
    pub observed: bool,
    pub wind: crate::attitude::Vector,
    pub decoy: super::decoys::Memory,
    pub tracking: super::energy_seeker::Tracking,
}
crate::checkpoint_struct!(Motion {
    body,
    flight,
    target,
    observed,
    wind,
    decoy,
    tracking
});

// Semantic revisions, updated only when behavior changes. Order: derivation,
// integration/atmosphere, guidance, seeker, decoy, prediction, fuze/contact, inert threat publication.
// Numeric inputs and drag curves are hashed directly; comments and evidence
// wording are not model inputs. Changes to rules must increment their revision.
const RULES: [u64; 8] = [5, 4, 3, 1, 4, 7, 3, 1];
/// Stable tape identity for numeric inputs, curves and semantic rule revisions.
pub fn fingerprint() -> u64 {
    fingerprint_model(
        tore_formats::missile_data::fingerprint(),
        tore_formats::missile_motors::MOTORS,
        RULES,
    )
}
fn fingerprint_model(
    mut hash: u64,
    motors: &[tore_formats::missile_motors::MotorData],
    rules: [u64; 8],
) -> u64 {
    let mut add = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
    };
    add(&super::decoys::CHAFF_EFFECTIVENESS_MULTIPLIER
        .to_bits()
        .to_le_bytes());
    for rule in rules {
        add(&rule.to_le_bytes());
    }
    for curve in super::energy::DRAG_CURVES {
        for value in curve {
            add(&value.to_le_bytes());
        }
    }
    add(&DEFEAT_MACH.to_le_bytes());
    add(&DEFEAT_TICKS.to_le_bytes());
    for m in motors {
        add(&[m.control_class as u8]);
        add(m.source.as_bytes());
        add(&[0]);
        for value in [
            m.propellant_fraction,
            m.boost_seconds,
            m.sustain_seconds,
            m.boost_share,
            m.exhaust_mps,
            m.control_class.seconds(),
        ] {
            add(&value.to_le_bytes());
        }
        add(&[u8::from(m.ignition_seconds.is_some())]);
        if let Some(seconds) = m.ignition_seconds {
            add(&seconds.to_le_bytes());
        }
        add(&[u8::from(m.thrust.is_some())]);
        if let Some(thrust) = m.thrust {
            for value in thrust {
                add(&value.to_le_bytes());
            }
        }
    }
    hash
}
#[cfg(test)]
mod fingerprint_tests {
    use super::*;
    #[test]
    fn semantic_identity_covers_inputs_and_fuze_but_not_provenance_text() {
        let mut motors = tore_formats::missile_motors::MOTORS.to_vec();
        let a = fingerprint_model(1, &motors, RULES);
        motors[0].motor_evidence = "a corrected citation";
        assert_eq!(a, fingerprint_model(1, &motors, RULES));
        motors[0].control_class = ControlClass::LongAir;
        assert_ne!(a, fingerprint_model(1, &motors, RULES));
        let mut rules = RULES;
        rules[6] += 1;
        assert_ne!(
            a,
            fingerprint_model(1, tore_formats::missile_motors::MOTORS, rules)
        );
        assert_ne!(
            a,
            fingerprint_model(2, tore_formats::missile_motors::MOTORS, RULES)
        );
    }
}
