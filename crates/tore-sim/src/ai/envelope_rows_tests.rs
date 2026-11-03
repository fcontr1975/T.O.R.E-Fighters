//! The AI's envelope readings worked out from each row's band once
//! ([`crate::flight::RowSpeeds`]) against copies of the code that asked
//! every row again for each reading (optimization round 2).
use super::tests::{setup, synthetic_profile};
use super::*;
use crate::flight;
use tore_formats::aircraft::{AircraftId, Envelope};

fn speed_limits_reference(flight: &crate::flight::State) -> SpeedLimits {
    let envelopes = &flight.model().configuration().aerodynamics.envelopes;
    let altitude = flight.position[1];
    let mut minimum = f64::INFINITY;
    let mut maximum = f64::NEG_INFINITY;
    let mut corner = f64::NAN;
    let mut best_g = i32::MIN;
    for envelope in envelopes {
        let Some((low, high)) = envelope.speeds(altitude) else {
            continue;
        };
        minimum = minimum.min(low);
        maximum = maximum.max(high);
        if envelope.g > best_g {
            best_g = envelope.g;
            corner = low;
        }
    }
    if !minimum.is_finite() || !maximum.is_finite() || maximum <= minimum {
        // No envelope covers this altitude. Fall back to the documented
        // fixture band rather than producing a non-finite limit.
        return SpeedLimits {
            minimum: ScalarSpeed(FALLBACK_MINIMUM_FPS),
            maximum: ScalarSpeed(FALLBACK_MAXIMUM_FPS),
            corner: ScalarSpeed(FALLBACK_CORNER_FPS),
        };
    }
    // Fitted (agent decision, 2026-09-29, after the weight-scaled stall
    // speeds): the lowest slow edge over every row, 0 G and negative G
    // included, lies well under the speed at which the loaded aircraft
    // can hold 1 G. The AI never plans to fly slower than that, in its
    // current flap setting (`flight::State::minimum_level_speed`); on the
    // F/A-18D the rows alone gave 114 kt against the 140 kt it needs, and
    // finals built on it sank into the ground.
    let level = minimum_level_speed_reference(flight, altitude, flight.flaps);
    if level < maximum {
        minimum = minimum.max(level);
    }
    // Fitted (agent decision, 2026-09-29): nor faster than the fastest
    // row that still leaves the loaded aircraft TOP_SPEED_G_MARGIN of
    // lift. Near its top speed only the 1 G row covers the aircraft, and
    // the legacy flight model divides that 1 G by the loading, so a
    // loaded fighter at full power there sank steadily into flat ground
    // at 1,100 kt without being able to pull up (fuzz seeds 43, 183,
    // 266).
    let config = flight.model().configuration();
    let load = 1.0
        + (flight.fuel + flight.payload_lbs) / config.mass.empty_lbs
            * config.aerodynamics.loaded_elevator_percent
            / 100.0;
    let manoeuvring = envelopes
        .iter()
        .filter(|e| f64::from(e.g) / load >= TOP_SPEED_G_MARGIN)
        .filter_map(|e| e.speeds(altitude).map(|(_, high)| high))
        .fold(f64::NEG_INFINITY, f64::max);
    if manoeuvring > minimum {
        maximum = maximum.min(manoeuvring);
    }
    if !corner.is_finite() {
        corner = minimum;
    }
    SpeedLimits {
        minimum: ScalarSpeed(minimum),
        maximum: ScalarSpeed(maximum),
        corner: ScalarSpeed(corner.min(maximum)),
    }
}

pub fn minimum_level_speed_reference(actor: &flight::State, altitude_ft: f64, flaps: f64) -> f64 {
    let c = actor.model().configuration();
    let Some(one) = c.aerodynamics.envelopes.iter().find(|e| e.g == 1) else {
        return 0.;
    };
    let Some((clean, _)) = one.speeds(altitude_ft) else {
        return 0.;
    };
    let stall = clean * (1. - 0.25 * flaps);
    let loading = (actor.fuel + actor.carried_lbs()) / c.mass.empty_lbs;
    let divisor = 1. + loading * c.aerodynamics.loaded_elevator_percent / 100.;
    let next = c
        .aerodynamics
        .envelopes
        .iter()
        .filter(|e| e.g > 1)
        .filter_map(|e| e.speeds(altitude_ft).map(|speeds| (speeds.0, e.g)))
        .filter(|(low, _)| *low > stall)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    match next {
        Some((low, g)) => {
            let share = ((divisor - 1.) / f64::from(g - 1)).clamp(0., 1.);
            stall + share * (low - stall)
        }
        None => stall,
    }
}

fn g_limits_reference(actor: &AiActor) -> (f64, f64) {
    let envelopes = &actor.flight.model().configuration().aerodynamics.envelopes;
    let altitude = actor.flight.position[1];
    let speed = actor.flight.speed;
    let mut available = 1;
    for envelope in envelopes {
        if let Some((low, high)) = envelope.speeds(altitude)
            && speed >= low
            && speed <= high
            && envelope.g > available
        {
            available = envelope.g;
        }
    }
    let config = actor.flight.model().configuration();
    let loading = (actor.flight.fuel + actor.flight.payload_lbs) / config.mass.empty_lbs;
    let mut loaded = f64::from(available.max(1))
        / (1.0 + loading * config.aerodynamics.loaded_elevator_percent / 100.0);
    // The hybrid flight model keeps 1 G anywhere inside the 1 G envelope
    // and ramps from 1 G at the stall to the next envelope row
    // (docs/FLIGHT-MODEL.md, "Envelope limits and loading"), so the AI
    // may ask for what the model gives there, as its control adapter
    // already assumes. Before this a loaded aircraft slowing on final was
    // held below 1 G, could not hold its path and sank into the ground.
    if actor.flight.research.is_some() {
        if let Some(ramp) = hybrid_ramp_g_reference(actor, speed) {
            loaded = loaded.max(ramp);
        }
        if envelopes.iter().any(|e| {
            e.g == 1
                && e.speeds(altitude)
                    .is_some_and(|(low, high)| speed >= low && speed <= high)
        }) {
            loaded = loaded.max(1.0);
        }
    }
    let (positive, negative) = ai_g_limits(
        actor.controller.experience().level,
        loaded,
        -loaded / 2.0,
        actor.identity.human_controlled,
    );
    (
        health_scaled_positive_g(positive, actor.control_health()),
        negative * actor.control_health(),
    )
}

fn hybrid_ramp_g_reference(actor: &AiActor, speed: f64) -> Option<f64> {
    let config = actor.flight.model().configuration();
    let altitude = actor.flight.position[1];
    let (clean_stall, _) = config
        .aerodynamics
        .envelopes
        .iter()
        .find(|e| e.g == 1)?
        .speeds(altitude)?;
    let stall = clean_stall * (1.0 - 0.25 * actor.flight.flaps);
    let loading = 1.0
        + (actor.flight.fuel + actor.flight.carried_lbs()) / config.mass.empty_lbs
            * config.aerodynamics.loaded_elevator_percent
            / 100.0;
    crate::flight::low_speed_positive_g_ceiling(config, altitude, speed, stall, false)
        .map(|g| g / loading)
}

fn approach_minimum_reference(actor: &AiActor) -> f64 {
    if actor.flight.research.is_none() {
        return 0.0;
    }
    let config = actor.flight.model().configuration();
    let altitude = actor.flight.position[1];
    let Some((clean_stall, maximum)) = config
        .aerodynamics
        .envelopes
        .iter()
        .find(|e| e.g == 1)
        .and_then(|e| e.speeds(altitude))
    else {
        return 0.0;
    };
    let stall = clean_stall * 0.75;
    let loading = 1.0
        + (actor.flight.fuel + actor.flight.carried_lbs()) / config.mass.empty_lbs
            * config.aerodynamics.loaded_elevator_percent
            / 100.0;
    let mut speed = stall;
    while speed < maximum {
        match crate::flight::low_speed_positive_g_ceiling(config, altitude, speed, stall, false) {
            Some(g) if g / loading < APPROACH_G_MARGIN => speed += 5.0,
            _ => return speed,
        }
    }
    maximum
}

fn random_envelopes(next: &mut impl FnMut() -> f64) -> Vec<Envelope> {
    // Always a 1 G row, as a model's configuration requires.
    let low = -(next() * 4.) as i32;
    let rows = 2 - low + (next() * 12.) as i32;
    (0..rows)
        .map(|row| Envelope {
            g: low + row,
            points: (0..3 + (next() * 18.) as usize)
                .map(|_| {
                    [
                        (next() * 1600.).round(),
                        // Shared heights, so vertices land on the altitudes asked.
                        (next() * 12.).round() * 5_000.,
                    ]
                })
                .collect(),
        })
        .collect()
}

#[test]
fn envelope_readings_from_row_bands_match_asking_every_row_again() {
    let mut seed = 0x51f1_5e3a_77c0_1d2b_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1_u64 << 53) as f64
    };
    let mut readings = 0;
    let mut random = 0;
    for (case, id) in AircraftId::ALL.into_iter().cycle().take(240).enumerate() {
        let profile = synthetic_profile(id);
        let position = [0., 1_000., 0.];
        let Ok(state) = flight::State::new(&profile, position) else {
            continue;
        };
        let mut s = setup(1, 1, 0, position, 0.);
        s.flight = state;
        s.home_airport = None;
        let Ok(mut actor) = AiActor::new(s) else {
            continue;
        };
        let envelopes = random_envelopes(&mut next);
        if case % 2 == 1 {
            let mut configuration = actor.flight().model().configuration().clone();
            configuration.aerodynamics.envelopes = envelopes.clone();
            actor
                .flight_mut()
                .model_mut()
                .set_configuration(configuration)
                .unwrap();
        }
        if case % 3 != 0 {
            let _ = actor.flight_mut().enable_research(1);
        }
        if case % 2 == 1
            && actor
                .flight()
                .model()
                .configuration()
                .aerodynamics
                .envelopes
                == envelopes
        {
            random += 1;
        }
        for _ in 0..40 {
            let flight = actor.flight_mut();
            flight.position[1] = if next() < 0.3 {
                (next() * 12.).round() * 5_000.
            } else {
                next() * 65_000.
            };
            flight.speed = next() * 1_800.;
            flight.flaps = [0., 0.5, 1., next()][(next() * 4.) as usize];
            flight.fuel = next() * 20_000.;
            let flight = actor.flight();
            assert_eq!(
                format!("{:?}", speed_limits_of(flight)),
                format!("{:?}", speed_limits_reference(flight)),
                "{id:?} case {case}"
            );
            for flaps in [0., 0.3, 1.] {
                let altitude = flight.position[1];
                assert_eq!(
                    flight.minimum_level_speed(altitude, flaps).to_bits(),
                    minimum_level_speed_reference(flight, altitude, flaps).to_bits()
                );
            }
            assert_eq!(
                format!("{:?}", actor.g_limits()),
                format!("{:?}", g_limits_reference(&actor)),
                "{id:?} case {case}"
            );
            assert_eq!(
                actor.approach_minimum_fps().to_bits(),
                approach_minimum_reference(&actor).to_bits(),
                "{id:?} case {case}"
            );
            readings += 1;
        }
    }
    assert!(readings > 4_000, "{readings}");
    // The random polygons are the model's own envelope block (the hybrid
    // adapter scales the others from them).
    assert!(random >= 30, "{random}");
}

#[test]
fn kept_row_bands_follow_a_replaced_configuration_at_the_same_altitude() {
    let mut seed = 0x0dd1_5eed_c0ff_ee11_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1_u64 << 53) as f64
    };
    let position = [0., 15_000., 0.];
    let mut state = flight::State::new(&synthetic_profile(AircraftId::F18), position).unwrap();
    for round in 0..300 {
        // Asked at one altitude, then the configuration is replaced (its
        // old allocation freed and perhaps used again) and asked again there.
        let fresh = |state: &flight::State| {
            flight::RowSpeeds::new(
                &state.model().configuration().aerodynamics.envelopes,
                state.position[1],
            )
            .bands()
            .collect::<Vec<_>>()
        };
        assert_eq!(
            format!(
                "{:?}",
                flight::RowSpeeds::of(&state).bands().collect::<Vec<_>>()
            ),
            format!("{:?}", fresh(&state)),
            "round {round}"
        );
        let mut configuration = state.model().configuration().clone();
        configuration.aerodynamics.envelopes = random_envelopes(&mut next);
        state.model_mut().set_configuration(configuration).unwrap();
        assert_eq!(
            format!(
                "{:?}",
                flight::RowSpeeds::of(&state).bands().collect::<Vec<_>>()
            ),
            format!("{:?}", fresh(&state)),
            "round {round}"
        );
        assert_eq!(
            format!("{:?}", speed_limits_of(&state)),
            format!("{:?}", speed_limits_reference(&state)),
            "round {round}"
        );
        if round % 7 == 0 {
            state.position[1] = (next() * 12.).round() * 5_000.;
        }
    }
}
