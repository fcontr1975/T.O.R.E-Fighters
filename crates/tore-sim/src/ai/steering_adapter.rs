//! AI intent-to-input feedback controller. The aircraft model alone advances
//! attitude, velocity, position and telemetry; no AI movement override exists.
//! B44 remains a requested-envelope/reference service, not an achieved-motion
//! constraint. The fitted controller is specified in docs/spec/ai.md under
//! "Input-only aircraft control". Player adapters are unchanged.

use super::controller::MotionIntent;
use super::fitted::{self, Fallback};
use super::motion::Bank;
use super::steering::{
    self, AuthorityModifiers, AxisRates, SteeringRequest, SteeringState, TurnDirection,
};
use super::strike::Handling;
use super::{AiError, Experience, Result, ScalarSpeed, SpeedLimits, experience};
use crate::flight::State;
use crate::models::FlightModel;
use tore_input::{PilotCommand, PilotInput, Switch};

/// Fitted heading-error response horizon; see the input-only control spec.
pub const BANK_COMMAND_LEAD_SECONDS: f64 = 1.0;

/// Fitted: the pitch-error response horizon while the flight path is below
/// the terrain floor, in seconds; see the input-only control spec.
pub const TERRAIN_PITCH_SECONDS: f64 = 1.0;

/// Fitted: the speed error, in ft/s, that moves the throttle from closed to
/// open in one tick. Rule: the throttle setting is the current setting plus
/// the speed error divided by this reference, clamped to 0 through 1. A
/// hundred feet per second is an agent choice, about 60 knots, which is a
/// large speed error for a fighter, so ordinary corrections are gentle and a
/// large one saturates. The spec gives no AI throttle rule at all.
pub const THROTTLE_REFERENCE_ERROR_FPS: f64 = 100.0;

/// Controls plus what the adapter had to fall back on.
#[derive(Clone, Debug, PartialEq)]
pub struct AdapterOutput {
    pub input: PilotInput,
    pub fallbacks: Vec<Fallback>,
    /// The rate-limited attitude the B44 rules asked for this tick, kept so a
    /// caller can compare requested against achieved.
    pub requested: SteeringState,
}

/// Per-actor steering state. One per AI aircraft, never shared.
///
/// The adapter holds no control history: every tick reads the actor's actual
/// attitude out of its flight state, so a lagging or damaged aircraft cannot
/// accumulate an invisible error here. The recorded request exists for
/// reporting and tests only and never feeds back into a command.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlAdapter {
    last_requested: Option<SteeringState>,
    gun_aim: Option<super::gunnery::Aim>,
    /// A striker's attack handling this step (slice ST-T2); plain for every
    /// other motion.
    strike_handling: Handling,
    /// Per-step defensive handling, cleared before every actor update.
    defense: Option<(Experience, bool)>,
}

impl ControlAdapter {
    pub fn new() -> Self {
        Self {
            last_requested: None,
            gun_aim: None,
            strike_handling: Handling::PLAIN,
            defense: None,
        }
    }

    /// Feed forward the changing observed lead while gun tracking is active.
    pub fn set_gun_aim(&mut self, aim: Option<super::gunnery::Aim>) {
        self.gun_aim = aim;
    }

    /// How this step's motion is flown: a striker's attack segments roll and
    /// pull ([`Handling::pull`]); everything else is plain.
    pub fn set_strike_handling(&mut self, handling: Handling) {
        self.strike_handling = handling;
    }

    /// Positive-G defense and optional IR heat reduction for this step only.
    pub fn set_defense(&mut self, defense: Option<(Experience, bool)>) {
        self.defense = defense;
    }

    pub fn strike_handling(&self) -> Handling {
        self.strike_handling
    }

    /// The attitude the previous call asked for, or `None` before the first.
    pub fn last_requested(&self) -> Option<SteeringState> {
        self.last_requested
    }

    /// Turn one resolved maneuver into controls for this actor's own flight
    /// model. `terrain_pitch_floor_deg` is the active B44 floor when one
    /// applies. Returns the controls and every fitted fallback applied.
    ///
    /// `g_limit` and `roll_limit_deg_per_s` are the loaded limits the flight
    /// model supplies, with the documented fitted health reduction and the
    /// AI experience adjustment of [`ai_g_limits`] already applied by the
    /// caller. `maximum_bank_deg` is the aircraft's own maximum bank: it
    /// bounds the bank request and is the B44 reference bank.
    #[allow(clippy::too_many_arguments)]
    pub fn controls(
        &mut self,
        state: &State,
        intent: &MotionIntent,
        limits: &SpeedLimits,
        g_limit: f64,
        roll_limit_deg_per_s: f64,
        maximum_bank_deg: f64,
        terrain_pitch_floor_deg: Option<f64>,
        dt_s: f64,
    ) -> Result<AdapterOutput> {
        if !dt_s.is_finite() || dt_s < 0.0 {
            return Err(AiError::InvalidInput(
                "time step must be finite and non-negative",
            ));
        }
        finite(intent.heading_deg, "requested heading must be finite")?;
        finite(
            intent.flight_path_pitch_deg,
            "requested flight-path pitch must be finite",
        )?;
        finite(state.speed, "own speed must be finite")?;
        finite(state.throttle, "own throttle must be finite")?;

        let g_limit = self.defense.map_or(g_limit, |(skill, _)| {
            (g_limit * super::defense::load_fraction(skill))
                .max(1.0)
                .min(g_limit)
        });
        let current = attitude(state)?;
        let speed = ScalarSpeed(state.speed);

        // B44 leaves the base pitch rate unresolved; the documented fitted
        // stand-in is the only source for it, and its use is always reported.
        let fallbacks = vec![Fallback::BasePitchRate];
        let pitch_deg_per_s = fitted::base_pitch_rate_deg_per_s(g_limit, speed)?;
        let rates = AxisRates::from_limits(g_limit, speed, roll_limit_deg_per_s, pitch_deg_per_s)?;

        let reference_bank_deg = steering::reference_bank_deg(maximum_bank_deg)?;
        let heading_request_deg = steering::wrap_heading(intent.heading_deg)?;
        let on_ground = state.research.as_ref().is_some_and(|r| r.on_ground);
        let heading_reference = if self.strike_handling.track_heading
            && !on_ground
            && state.velocity[0].hypot(state.velocity[2]) > 1.
        {
            state.velocity[0].atan2(state.velocity[2]).to_degrees()
        } else {
            current.heading_deg
        };
        let turn = TurnDirection::toward(heading_reference, heading_request_deg);
        let bank_request_deg = match intent.bank {
            Bank::Explicit(deg) => f64::from(deg).clamp(-maximum_bank_deg, maximum_bank_deg),
            Bank::Unconstrained => {
                let rate = heading_error_deg(heading_reference, heading_request_deg).to_radians()
                    / if self.gun_aim.is_some() {
                        0.6
                    } else {
                        BANK_COMMAND_LEAD_SECONDS
                    }
                    + self.gun_aim.map_or(0., |a| a.heading_rate);
                let bank_limit = maximum_bank_deg
                    .min(if self.defense.is_some() {
                        85.0
                    } else if intent.formation_flight {
                        60.0
                    } else {
                        75.0
                    })
                    .min((1.0 / g_limit.max(1.0)).acos().to_degrees());
                (state.speed.max(125.0) * rate / 32.174)
                    .atan()
                    .to_degrees()
                    .clamp(-bank_limit, bank_limit)
            }
        };

        // Contact is tracked by the hybrid adapter; without it the adapter
        // cannot see the ground from this signature and reports airborne.
        // Fitted (agent decision, 2026-09-28, overnight bug battery): an
        // airborne aircraft whose flight path is below the B44 terrain floor
        // levels its wings, so a steep turn cannot use the lift it needs to
        // climb away from the ground, and closes the pitch error over
        // [`TERRAIN_PITCH_SECONDS`] rather than three seconds (B44: pitch
        // authority gains 20 degrees per second while the floor is active).
        // The airfield terrain correction already levels the wings. Without
        // it AI aircraft flew into the ground at full G while still turning
        // toward a target, or eased into a rising hillside at 2 G.
        let below_terrain_floor = !on_ground
            && terrain_pitch_floor_deg.is_some_and(|floor| current.flight_path_pitch_deg < floor);
        let bank_request_deg = if below_terrain_floor {
            0.0
        } else {
            bank_request_deg
        };
        // A striker's roll and pull (slice ST-T2): the lift is rolled onto
        // the wanted direction and pulled, so a big nose-down change rolls
        // inverted and pulls as retail's dive bomb does. Plain otherwise.
        let pull = (self.strike_handling.pull && !on_ground)
            .then(|| {
                roll_and_pull(
                    state,
                    current.bank_deg,
                    intent.heading_deg,
                    intent
                        .flight_path_pitch_deg
                        .max(terrain_pitch_floor_deg.unwrap_or(-90.0)),
                    g_limit,
                )
            })
            .flatten();
        let bank_request_deg = match pull {
            Some(p) if !below_terrain_floor => p.bank_deg,
            _ => bank_request_deg,
        };
        let request = SteeringRequest {
            heading_deg: heading_request_deg,
            flight_path_pitch_deg: intent.flight_path_pitch_deg,
            bank_deg: bank_request_deg.clamp(-maximum_bank_deg, maximum_bank_deg),
            reference_bank_deg,
            // The ceiling test belongs to the maneuver layer, which owns the
            // aircraft's ceiling; it is not visible from this signature.
            at_ceiling: false,
            terrain_pitch_floor_deg,
            on_ground,
            mode: intent.mode,
        };
        let requested = current.step(&request, rates, dt_s)?;

        // B44 limits the desired roll rate, never the achieved trajectory.
        let effective = steering::effective_rates(
            request.mode,
            rates,
            current.bank_deg,
            reference_bank_deg,
            turn,
            AuthorityModifiers {
                terrain_floor_active: terrain_pitch_floor_deg.is_some(),
                on_ground,
            },
        )?;

        // Fitted feedback controller. These are control requests only: the
        // aircraft model owns every achieved attitude, velocity and position.
        let config = state.model().configuration();
        let (clean_stall, maximum) = config
            .aerodynamics
            .envelopes
            .iter()
            .find(|e| e.g == 1)
            .and_then(|e| e.speeds(state.position[1]))
            .unwrap_or((900.0, 1000.0));
        let stall = if state.research.is_some() {
            clean_stall * (1.0 - 0.25 * state.flaps)
        } else {
            clean_stall
        };
        let authority = (state.speed / stall.max(1.0)).powi(2).clamp(0.0, 1.0);
        let bank_error = heading_error_deg(current.bank_deg, bank_request_deg).to_radians();
        // A roll and pull rolls at the aircraft's full roll rate: the B44
        // state rates halve it for maneuvering, too slow to roll inverted
        // and pull in a dive (slice ST-T2, fitted, agent).
        let roll_authority = if pull.is_some() || self.defense.is_some() {
            rates.roll_deg_per_s.max(effective.roll_deg_per_s)
        } else {
            effective.roll_deg_per_s
        };
        let desired_roll = if self.defense.is_some() {
            if let Some(profile) = config.controls {
                let axis = if state.research.is_some() {
                    profile.hybrid_roll.unwrap_or(profile.roll)
                } else {
                    profile.roll
                };
                let stopping = state.roll_rate.powi(2)
                    / (2.0 * f64::from(axis.deceleration).to_radians())
                    + state.roll_rate.abs() * dt_s
                    + 0.5_f64.to_radians();
                if bank_error * state.roll_rate > 0.0 && bank_error.abs() <= stopping
                    || bank_error.abs() < 0.5_f64.to_radians()
                {
                    0.0
                } else {
                    bank_error / 0.10
                }
            } else {
                // Cancel the remaining roll of the aircraft's lag response,
                // then close the bank error quickly without an inverted overshoot.
                (bank_error - state.roll_rate * config.tuning.roll_response_seconds) / 0.10
            }
        } else {
            bank_error / 0.7 - state.roll_rate * 0.5
        }
        .clamp(-roll_authority.to_radians(), roll_authority.to_radians());
        let roll_limit = if let Some(profile) = config.controls {
            let axis = if state.research.is_some() {
                profile.hybrid_roll.unwrap_or(profile.roll)
            } else {
                profile.roll
            };
            let bound = if desired_roll < 0.0 {
                -f64::from(axis.minimum)
            } else {
                f64::from(axis.maximum)
            };
            bound.to_radians() * (state.speed / (2.0 * stall.max(1.0))).clamp(0.0, 1.0)
        } else if state.research.is_some() {
            config
                .aerodynamics
                .roll_limit_rad_per_second
                .clamp(0.1, 6.0)
                * authority
        } else {
            config.tuning.legacy_roll_limit_rad_per_second * authority
        };
        let (mut low, mut high) = (-1.0_f64, 1.0_f64);
        let mut inside_envelope = false;
        for envelope in &config.aerodynamics.envelopes {
            if let Some((min, max)) = envelope.speeds(state.position[1])
                && state.speed >= min
                && state.speed <= max
            {
                low = low.min(f64::from(envelope.g));
                high = high.max(f64::from(envelope.g));
                inside_envelope = true;
            }
        }
        // The same two hybrid-only rules as the flight model (docs/FLIGHT-MODEL.md,
        // "Envelope limits and loading"), so the AI never asks for G the aircraft
        // cannot give: lift thins above the 1 G ceiling, and inside the envelope
        // 1 G is always kept.
        let hybrid = state.research.is_some();
        if hybrid
            && let Some(one_g) = config.aerodynamics.envelopes.iter().find(|e| e.g == 1)
            && one_g.speeds(state.position[1]).is_none()
        {
            let top = one_g.points.iter().map(|p| p[1]).fold(f64::MIN, f64::max);
            if state.position[1] > top {
                high *= crate::flight::ceiling_lift_ratio(state.position[1], top);
            }
        }
        let loading = 1.0
            + (state.fuel + state.carried_lbs()) / config.mass.empty_lbs
                * config.aerodynamics.loaded_elevator_percent
                / 100.0;
        low /= loading;
        high /= loading;
        // The flight model keeps 1 G anywhere inside the 1 G envelope.
        if hybrid && inside_envelope {
            high = high.max(1.0);
        }
        if state.systems.has(30) {
            low = low.max(-g_limit);
        }
        let mut flap_gain = 1.0;
        if state.research.is_some() {
            if let Some(continuous) = crate::flight::low_speed_positive_g_ceiling(
                config,
                state.position[1],
                state.speed,
                stall,
                false,
            ) {
                high = continuous / loading;
            }
            // Invert the same flap lift used by the researched flight model.
            // Otherwise a clean-aircraft elevator request adds excess lift
            // and the aircraft floats above the runway on every attempt.
            let drag = tore_formats::flight_model::drag_percent(
                (state.speed * 256.0) as i32,
                (state.position[1] * 256.0) as i32,
                maximum.round().clamp(1.0, f64::from(i16::MAX)) as i16,
            )
            .unwrap_or_else(|_| ((state.speed / maximum.max(1.0)) * 100.0).round() as i32)
            .clamp(0, 100) as f64;
            let lift = config.aerodynamics.flaps_lift_f8
                * if on_ground {
                    drag / 200.0
                } else {
                    1.0 - state.gear + drag / 100.0 * state.gear
                };
            flap_gain += state.flaps * lift / 256.0;
        }
        let pitch_goal = intent
            .flight_path_pitch_deg
            .max(terrain_pitch_floor_deg.unwrap_or(-90.0))
            .clamp(-90.0, 90.0);
        let pitch_error = (pitch_goal - current.flight_path_pitch_deg).to_radians();
        let vertical_g = current.flight_path_pitch_deg.to_radians().cos()
            + state.speed
                * (pitch_error
                    / if below_terrain_floor {
                        TERRAIN_PITCH_SECONDS
                    } else if self.gun_aim.is_some() {
                        0.8
                    } else {
                        3.0
                    }
                    + self.gun_aim.map_or(0., |a| a.pitch_rate))
                / 32.174;
        // Defense pulls to its skill limit once lift is on the turn side.
        // Pitch feedback cannot reduce a committed break's lateral lift.
        // Terrain recovery still takes priority.
        let desired_g = if self.defense.is_some() {
            if !below_terrain_floor && bank_request_deg.abs() >= 35.0 {
                // Pull to the skill limit as soon as lift is on the turn side.
                // Roll-in blends from 1 G to full pull over the first 30 degrees.
                let lift_on_turn =
                    (state.bank.to_degrees() * bank_request_deg.signum() / 30.0).clamp(0.0, 1.0);
                (1.0 + (g_limit - 1.0) * lift_on_turn).clamp(0.0, g_limit.max(0.0))
            } else {
                (vertical_g / state.bank.cos().max(0.05)).clamp(0.0, g_limit.max(0.0))
            }
        } else {
            (vertical_g / state.bank.cos().max(0.25)).clamp(low, g_limit.max(low))
        };

        // The roll and pull asks for its own load, applied as the lift comes
        // onto its bank: none while the lift is more than 45 degrees off it,
        // so a roll-out does not pull the nose across the track.
        let desired_g = match pull {
            Some(p) if !below_terrain_floor => {
                let off = heading_error_deg(current.bank_deg, p.bank_deg).to_radians();
                let aligned = ((off.cos() - ROLL_AND_PULL_ALIGN_COS)
                    / (1.0 - ROLL_AND_PULL_ALIGN_COS))
                    .clamp(0.0, 1.0);
                (p.load_g * aligned).clamp(low, g_limit.max(low))
            }
            _ => desired_g,
        };
        // The flight model also scales the lift by the airframe's damage
        // (a damaged wing or tail, `regional_effects`; a wing fault halves
        // it) and the elevator by the tail's damage. Inverted here as the
        // flap lift is (agent decision, 2026-10-21): without it a damaged
        // aircraft's pull fell short of the load it asked for, and on final
        // an F/A-18D with a quarter of its tail damaged sank 50 ft under
        // the glide path and touched down at the threshold.
        let regional = crate::aircraft_systems::regional_effects(state.damage_regions);
        let damaged_lift = regional.lift * if state.systems.has(25) { 0.5 } else { 1.0 };
        let delta = desired_g / (authority * flap_gain * damaged_lift).max(0.01) - 1.0;
        let pitch_input = delta
            / if delta > 0.0 {
                (high - 1.0).max(0.01)
            } else {
                (1.0 - low).max(0.01)
            }
            / regional.authority[0].max(0.01);

        let cool_engine = self.defense.is_some_and(|(_, cool)| cool);
        let throttle = throttle_command(state.throttle, speed, intent.speed, limits)?;
        let throttle = if cool_engine && speed.0 >= limits.minimum.0 * 1.3 {
            throttle.min(0.65)
        } else {
            throttle
        };
        let input = PilotInput {
            pitch: if dt_s > 0.0 {
                pitch_input.clamp(-1.0, 1.0)
            } else {
                0.0
            },
            roll: if dt_s > 0.0 {
                (desired_roll / roll_limit.max(0.01)).clamp(-1.0, 1.0)
            } else {
                0.0
            },
            // The AI steers on bank and pitch only; B44 gives the rudder no
            // steering role, so the yaw axis is left alone.
            yaw: 0.0,
            throttle_rate: 0.0,
            throttle: Some(throttle),
            commands: vec![PilotCommand::Set(
                Switch::Burner,
                intent.afterburner && !cool_engine,
            )],
            ..PilotInput::default()
        }
        .bounded();

        self.last_requested = Some(requested);
        Ok(AdapterOutput {
            input,
            fallbacks,
            requested,
        })
    }
}

/// Fitted (slice ST-T2, agent): the roll and pull turns the flight path
/// toward the wanted direction at the angle between them over this many
/// seconds.
pub const ROLL_AND_PULL_SECONDS: f64 = 1.0;
/// Fitted: a nose-down change that needs more than this push, in G, rolls
/// inverted and pulls; a smaller one unloads or pushes wings level. Once
/// inverted it stays so while any push is needed.
pub const ROLL_AND_PULL_INVERT_G: f64 = 1.0;
/// Fitted: only a change of flight path of at least this many degrees rolls
/// inverted; a smaller one is pushed (a steep dive's corrections).
pub const ROLL_AND_PULL_INVERT_DEG: f64 = 30.0;
/// Fitted: below this load, in G, the roll and pull holds the wings level.
pub const ROLL_AND_PULL_SLACK_G: f64 = 0.02;
/// Fitted: an upright bank is worked out as if at least this much lift were
/// wanted upward, so a small lift is not banked steeply to the side.
pub const ROLL_AND_PULL_UPRIGHT_G: f64 = 0.5;
/// Fitted: the steepest upright bank the roll and pull flies; the load is
/// the wanted lift along that bank.
pub const ROLL_AND_PULL_UPRIGHT_BANK_DEG: f64 = 85.0;
/// Fitted: the cosine of the bank error (45 degrees) beyond which the roll
/// and pull holds its load off while it rolls; the load grows to the full
/// request as the lift comes onto its bank.
pub const ROLL_AND_PULL_ALIGN_COS: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// A roll and pull's bank and load.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RollAndPull {
    pub bank_deg: f64,
    /// The load along the lift, G; negative is a push.
    pub load_g: f64,
}

/// The lift placement that turns the flight path toward `heading_deg`,
/// `pitch_deg` (slice ST-T2, fitted, agent). The wanted acceleration across
/// the flight path is the speed times the angle to the wanted direction over
/// [`ROLL_AND_PULL_SECONDS`]; the lift must give it and hold off gravity's
/// part across the path. The lift's bank and size follow: a lift that would
/// point down by more than [`ROLL_AND_PULL_INVERT_G`] (any amount once
/// inverted), and more down than sideways, is flown inverted with a positive
/// pull, if the flight path must turn [`ROLL_AND_PULL_INVERT_DEG`] or more
/// (or is inverted already); an upward lift banks at most [`ROLL_AND_PULL_UPRIGHT_BANK_DEG`], worked out as if at least
/// [`ROLL_AND_PULL_UPRIGHT_G`] were wanted upward; a smaller downward need
/// is a push banked at most 45 degrees the other way; a lift under
/// [`ROLL_AND_PULL_SLACK_G`] keeps the wings level. `None` when the aircraft
/// does not move.
pub fn roll_and_pull(
    state: &State,
    bank_deg: f64,
    heading_deg: f64,
    pitch_deg: f64,
    g_limit: f64,
) -> Option<RollAndPull> {
    const G: f64 = 32.174;
    let v = state.velocity;
    let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !speed.is_finite() || speed < 1.0 {
        return None;
    }
    let v = v.map(|c| c / speed);
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let norm = |a: [f64; 3]| {
        let l = dot(a, a).sqrt();
        (l > 1e-9).then(|| a.map(|c| c / l))
    };
    // Lift directions: "up" across the path in the vertical plane (bank 0)
    // and "right" (bank +90, which turns toward a larger heading).
    let up = norm([-v[1] * v[0], 1.0 - v[1] * v[1], -v[1] * v[2]]).unwrap_or_else(|| {
        let h = state.yaw;
        [-h.sin() * v[1].signum(), 0.0, -h.cos() * v[1].signum()]
    });
    let right = [
        up[1] * v[2] - up[2] * v[1],
        up[2] * v[0] - up[0] * v[2],
        up[0] * v[1] - up[1] * v[0],
    ];
    let (h, p) = (
        heading_deg.to_radians(),
        pitch_deg.clamp(-90.0, 90.0).to_radians(),
    );
    let wanted = [h.sin() * p.cos(), p.sin(), h.cos() * p.cos()];
    let along = dot(wanted, v).clamp(-1.0, 1.0);
    let angle = along.acos();
    let across = norm(std::array::from_fn(|i| wanted[i] - along * v[i])).unwrap_or(up);
    let accel = speed * angle / ROLL_AND_PULL_SECONDS;
    // Lift = wanted acceleration minus gravity's part across the path.
    let gravity_across: [f64; 3] =
        std::array::from_fn(|i| -v[1] * v[i] * -G - if i == 1 { G } else { 0.0 });
    let lift: [f64; 3] = std::array::from_fn(|i| (accel * across[i] - gravity_across[i]) / G);
    let (lift_up, lift_right) = (dot(lift, up), dot(lift, right));
    let size = lift_up.hypot(lift_right);
    let inverted = bank_deg.abs() > 90.0;
    let threshold = if inverted {
        0.0
    } else {
        -ROLL_AND_PULL_INVERT_G
    };
    let (bank, load) = if size < ROLL_AND_PULL_SLACK_G {
        (0.0, lift_up)
    } else if lift_up < threshold
        && -lift_up >= lift_right.abs()
        && (inverted || angle.to_degrees() >= ROLL_AND_PULL_INVERT_DEG)
    {
        // A nose-down change: roll inverted and pull.
        (lift_right.atan2(lift_up).to_degrees(), size)
    } else if lift_up >= 0.0 {
        // Upright: bank toward the side wanted, the load holding the lift
        // that is wanted upward (a turn of at most the adapter's 75 degrees).
        let bank = lift_right
            .atan2(lift_up.max(ROLL_AND_PULL_UPRIGHT_G))
            .to_degrees()
            .clamp(
                -ROLL_AND_PULL_UPRIGHT_BANK_DEG,
                ROLL_AND_PULL_UPRIGHT_BANK_DEG,
            );
        let b = bank.to_radians();
        (bank, (lift_up * b.cos() + lift_right * b.sin()).max(0.0))
    } else {
        // A gentle push, nearly wings level: bank the other way, negative G.
        let bank = (-lift_right)
            .atan2(-lift_up)
            .to_degrees()
            .clamp(-45.0, 45.0);
        let b = bank.to_radians();
        (bank, lift_up * b.cos() - lift_right * b.sin())
    };
    Some(RollAndPull {
        bank_deg: bank,
        load_g: load.min(g_limit.max(1.0)),
    })
}

/// The AI-only experience G adjustment ("Other experience effects").
///
/// Novice and Average AI aircraft lose 1 G of positive limit, never below 2 G,
/// and 1 G of negative limit, never beyond -2 G. Experienced and Ace are
/// unchanged.
///
/// Returns the loaded limits unchanged for a human-flown aircraft. The
/// exemption is the object's human-control bit and no AI path sets it, so
/// player flight limits never pass through the adjustment.
pub fn ai_g_limits(
    level: Experience,
    positive_g: f64,
    negative_g: f64,
    human_controlled: bool,
) -> (f64, f64) {
    experience::adjusted_g_limits(level, positive_g, negative_g, human_controlled)
}

/// Fitted: the throttle setting for a commanded speed.
///
/// Rule: the commanded speed is held inside the aircraft's own minimum and
/// maximum, and the setting is the current setting plus the remaining speed
/// error divided by [`THROTTLE_REFERENCE_ERROR_FPS`], clamped to 0 through 1.
/// Reading the actual setting back each tick makes this self-correcting
/// without any stored integrator.
pub fn throttle_command(
    current_throttle: f64,
    speed: ScalarSpeed,
    requested: ScalarSpeed,
    limits: &SpeedLimits,
) -> Result<f64> {
    finite(requested.0, "requested speed must be finite")?;
    finite(limits.minimum.0, "minimum speed must be finite")?;
    finite(limits.maximum.0, "maximum speed must be finite")?;
    if limits.minimum.0 > limits.maximum.0 {
        return Err(AiError::InvalidInput(
            "minimum speed must not exceed maximum speed",
        ));
    }
    let target = requested.0.clamp(limits.minimum.0, limits.maximum.0);
    Ok((current_throttle + (target - speed.0) / THROTTLE_REFERENCE_ERROR_FPS).clamp(0.0, 1.0))
}

/// The actor's current attitude in the recovered degree domain.
///
/// Heading is the body heading, `yaw` measured from +Z toward +X. Flight-path
/// pitch is the direction of the velocity vector; body pitch minus it is the
/// B44 body pitch offset, which steering carries through unchanged. A stopped
/// aircraft has no velocity direction, so its nose direction is used and the
/// offset is zero.
fn attitude(state: &State) -> Result<SteeringState> {
    finite(state.yaw, "own heading must be finite")?;
    finite(state.pitch, "own pitch must be finite")?;
    finite(state.bank, "own bank must be finite")?;
    let body_pitch_deg = signed_angle_deg(state.pitch.to_degrees());
    let [vx, vy, vz] = state.velocity;
    let horizontal = vx.hypot(vz);
    let flight_path_pitch_deg =
        if vy.is_finite() && horizontal.is_finite() && horizontal + vy != 0.0 {
            vy.atan2(horizontal).to_degrees()
        } else {
            body_pitch_deg
        };
    Ok(SteeringState {
        heading_deg: steering::wrap_heading(state.yaw.to_degrees())?,
        flight_path_pitch_deg,
        bank_deg: signed_angle_deg(state.bank.to_degrees()),
        body_pitch_offset_deg: body_pitch_deg - flight_path_pitch_deg,
    })
}

/// Signed shorter path from `current` to `requested`, in -180 through 180.
fn heading_error_deg(current_deg: f64, requested_deg: f64) -> f64 {
    (requested_deg - current_deg + 180.0).rem_euclid(360.0) - 180.0
}

/// Wrap an angle into -180 through 180 degrees.
fn signed_angle_deg(degrees: f64) -> f64 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

fn finite(value: f64, what: &'static str) -> Result<()> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(AiError::InvalidInput(what))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::DecisionRandom;
    use crate::ai::controller::Completion;
    use crate::ai::motion::{CompletionAxis, Duration, MotionRequest, PitchRequest, SpeedRequest};
    use crate::ai::steering::{CommandMode, SECONDS_PER_TICK};
    use crate::flight::integration_tests::profile;

    const G_LIMIT: f64 = 7.0;
    const ROLL_LIMIT: f64 = 180.0;
    const MAXIMUM_BANK: f64 = 80.0;

    fn limits() -> SpeedLimits {
        SpeedLimits {
            minimum: ScalarSpeed(200.0),
            maximum: ScalarSpeed(1600.0),
            corner: ScalarSpeed(700.0),
        }
    }

    fn state() -> State {
        let mut s = State::new(&profile(), [0.0, 15000.0, 0.0]).unwrap();
        s.yaw = 0.0;
        s.pitch = 0.0;
        s.bank = 0.0;
        s.velocity = [0.0, 0.0, s.speed];
        s
    }

    fn intent(heading_deg: f64, pitch_deg: f64, speed: f64) -> MotionIntent {
        MotionIntent {
            formation_flight: false,
            afterburner: false,
            id: 1,
            request: MotionRequest::new(
                heading_deg as i32,
                PitchRequest::Explicit(pitch_deg as i32),
                Bank::Unconstrained,
                SpeedRequest::Explicit(ScalarSpeed(speed)),
                Duration::Timed(5),
            ),
            heading_deg,
            flight_path_pitch_deg: pitch_deg,
            speed: ScalarSpeed(speed),
            bank: Bank::Unconstrained,
            completion: Completion::Axis(CompletionAxis::Heading),
            steering_point: None,
            mode: CommandMode::Ordinary,
        }
    }

    fn controls(state: &State, intent: &MotionIntent, dt_s: f64) -> AdapterOutput {
        ControlAdapter::new()
            .controls(
                state,
                intent,
                &limits(),
                G_LIMIT,
                ROLL_LIMIT,
                MAXIMUM_BANK,
                None,
                dt_s,
            )
            .unwrap()
    }

    #[test]
    fn defensive_roll_brakes_before_crossing_ninety_degrees() {
        use crate::models::handling::Profile;
        use tore_formats::flight_model::normal_control::LoadedAxis;
        for hybrid in [false, true] {
            for direction in [-1.0, 1.0] {
                for (rate, acceleration, deceleration) in
                    [(225, 286, 571), (180, 214, 427), (345, 498, 996)]
                {
                    let mut model = state().import_model();
                    let mut config = model.configuration().clone();
                    config.controls = Some(Profile {
                        roll: LoadedAxis {
                            minimum: -rate,
                            maximum: rate,
                            acceleration,
                            deceleration,
                        },
                        hybrid_roll: None,
                        auxiliary: [LoadedAxis {
                            minimum: 0,
                            maximum: 0,
                            acceleration: 200,
                            deceleration: 400,
                        }; 3],
                    });
                    model.set_configuration(config).unwrap();
                    let mut s = State::from_model(model, [0.0, 3000.0, 0.0]);
                    if hybrid {
                        s.enable_research(1).unwrap();
                    }
                    s.yaw = 0.0;
                    s.pitch = 0.0;
                    s.bank = 0.0;
                    s.speed = 850.0;
                    s.velocity = [0.0, 0.0, s.speed];
                    let mut adapter = ControlAdapter::new();
                    adapter.set_defense(Some((Experience::Ace, false)));
                    let mut command = intent(direction * 90.0, -5.0, s.speed);
                    command.bank = Bank::Explicit((direction * 85.0) as i32);
                    let mut peak = 0.0_f64;
                    for _ in 0..960 {
                        command.heading_deg = s.yaw.to_degrees() + direction * 90.0;
                        let out = adapter
                            .controls(
                                &s,
                                &command,
                                &limits(),
                                G_LIMIT,
                                ROLL_LIMIT,
                                85.0,
                                None,
                                crate::flight::DT,
                            )
                            .unwrap();
                        s.step(&out.input, |_, _| 0.0);
                        peak = peak.max(s.bank.to_degrees().abs());
                        assert!(s.g >= 0.0 && !s.crashed);
                    }
                    assert!(
                        peak <= 90.0,
                        "hybrid={hybrid} direction={direction} rate={rate}: peak={peak}"
                    );
                }
            }
        }
    }

    #[test]
    fn defensive_bank_pulls_positive_g_and_skill_changes_the_flown_turn() {
        for hybrid in [false, true] {
            let mut turns = Vec::new();
            for skill in Experience::ALL {
                let mut s = state();
                if hybrid {
                    s.enable_research(1).unwrap();
                }
                s.speed = 850.0;
                s.velocity = [0.0, 0.0, s.speed];
                let mut command = intent(120.0, -5.0, 850.0);
                command.mode = CommandMode::OtherState;
                let mut adapter = ControlAdapter::new();
                adapter.set_defense(Some((skill, false)));
                // This synthetic airframe has a 6 G row, not the generic
                // test helper's 7 G request. Supply its actual loaded limit
                // as the mission does, so clipping cannot erase skill differences.
                let loaded_limit = 6.0 / (1.0 + (s.fuel + s.carried_lbs()) / 10000.0 * 0.4);
                let pilot_limit = ai_g_limits(skill, loaded_limit, -3.0, false).0;
                let mut peak_g = 0.0_f64;
                for _ in 0..600 {
                    let out = adapter
                        .controls(
                            &s,
                            &command,
                            &limits(),
                            pilot_limit,
                            ROLL_LIMIT,
                            MAXIMUM_BANK,
                            None,
                            crate::flight::DT,
                        )
                        .unwrap();
                    s.step(&out.input, |_, _| 0.0);
                    assert!(s.g > 0.0, "{skill:?} hybrid={hybrid}: {} G", s.g);
                    peak_g = peak_g.max(s.g);
                }
                let turn = heading_error_deg(0.0, s.yaw.to_degrees()).abs();
                assert!(turn > 20.0, "{skill:?} hybrid={hybrid}: {turn} degrees");
                let skill_limit = pilot_limit * super::super::defense::load_fraction(skill);
                assert!(
                    peak_g >= 0.95 * skill_limit,
                    "{skill:?} hybrid={hybrid}: peak {peak_g} G, limit {skill_limit}"
                );
                assert!(!s.crashed);
                turns.push(turn);
            }
            assert!(turns[3] > turns[0] * 1.3, "hybrid={hybrid}: {turns:?}");
        }
    }

    #[test]
    fn defensive_heat_reduction_preserves_low_speed_power_and_clears() {
        let mut s = state();
        s.speed = 850.0;
        s.velocity = [0.0, 0.0, s.speed];
        s.throttle = 1.0;
        let command = intent(90.0, 0.0, 1000.0);
        let mut adapter = ControlAdapter::new();
        adapter.set_defense(Some((Experience::Ace, true)));
        let run = |adapter: &mut ControlAdapter, s: &State| {
            adapter
                .controls(
                    s,
                    &command,
                    &limits(),
                    G_LIMIT,
                    ROLL_LIMIT,
                    MAXIMUM_BANK,
                    None,
                    crate::flight::DT,
                )
                .unwrap()
        };
        let cool = run(&mut adapter, &s);
        assert_eq!(cool.input.throttle, Some(0.65));
        assert!(
            cool.input
                .commands
                .contains(&PilotCommand::Set(Switch::Burner, false))
        );
        s.speed = 250.0;
        s.velocity = [0.0, 0.0, s.speed];
        assert_eq!(run(&mut adapter, &s).input.throttle, Some(1.0));
        adapter.set_defense(None);
        assert_eq!(
            run(&mut adapter, &s),
            controls(&s, &command, crate::flight::DT)
        );
    }

    #[test]
    fn defensive_turn_yields_to_terrain_recovery() {
        let mut s = state();
        s.bank = 70.0_f64.to_radians();
        s.pitch = -10.0_f64.to_radians();
        s.velocity = crate::attitude::Basis::new(s.yaw, s.pitch, s.bank)
            .forward
            .map(|v| v * s.speed);
        let command = intent(120.0, -5.0, s.speed);
        let mut adapter = ControlAdapter::new();
        adapter.set_defense(Some((Experience::Ace, false)));
        let out = adapter
            .controls(
                &s,
                &command,
                &limits(),
                G_LIMIT,
                ROLL_LIMIT,
                MAXIMUM_BANK,
                Some(10.0),
                crate::flight::DT,
            )
            .unwrap();
        assert!(out.input.roll < 0.0);
        assert!(out.input.pitch > 0.0);
    }

    #[test]
    fn bomb_tracking_keeps_a_correct_ground_track_when_the_nose_is_offset() {
        let mut s = state();
        s.yaw = 10_f64.to_radians();
        let command = intent(0., 0., s.speed);
        assert!(controls(&s, &command, SECONDS_PER_TICK).input.roll < 0.);
        let mut adapter = ControlAdapter::new();
        adapter.set_strike_handling(Handling {
            track_heading: true,
            ..Handling::PLAIN
        });
        let output = adapter
            .controls(
                &s,
                &command,
                &limits(),
                G_LIMIT,
                ROLL_LIMIT,
                MAXIMUM_BANK,
                None,
                SECONDS_PER_TICK,
            )
            .unwrap();
        assert!(output.input.roll.abs() < 1e-9);
    }

    #[test]
    fn researched_flaps_follow_a_shallow_descent_without_ballooning() {
        let mut aircraft = profile();
        aircraft.fields.get_mut("flapsLift").unwrap().value = "256".into();
        for envelope in &mut aircraft.envelopes {
            if envelope.g > 1 {
                envelope.points[0][0] = 300.0 + f64::from(envelope.g) * 20.0;
                envelope.points[1][0] = 350.0 + f64::from(envelope.g) * 20.0;
            }
        }
        let mut s = State::new(&aircraft, [0.0, 8_000.0, 0.0]).unwrap();
        s.enable_research(1).unwrap();
        s.yaw = 0.0;
        s.pitch = -1.5_f64.to_radians();
        s.bank = 0.0;
        s.speed = 220.0;
        s.velocity = crate::attitude::Basis::new(s.yaw, s.pitch, s.bank)
            .forward
            .map(|v| v * s.speed);
        s.gear_down = true;
        s.gear = 1.0;
        s.flaps_down = true;
        s.flaps = 1.0;
        let command = intent(0.0, -1.5, 220.0);
        let mut replay = s.clone();
        for _ in 0..30 * 120 {
            let output = controls(&s, &command, crate::flight::DT);
            s.step(&output.input, |_, _| 0.0);
            replay.step(&output.input, |_, _| 0.0);
        }
        assert_eq!(s, replay);
        let path = attitude(&s).unwrap().flight_path_pitch_deg;
        assert!(
            (path + 1.5).abs() < 0.5,
            "requested -1.5 degrees, achieved {path}"
        );
        assert!(s.position[1] < 7_900.0);
        assert!(!s.crashed);
    }

    #[test]
    fn a_damaged_airframe_still_holds_the_requested_descent() {
        // 2026-10-21 (ai-long-15v15): an F/A-18D with a quarter of its tail
        // damaged flew its final 50 ft under the glide path, because the
        // stick it was given ignored the damaged lift and elevator.
        let fly = |regions: [f64; 6], research: bool| {
            let mut s = State::new(&profile(), [0.0, 8_000.0, 0.0]).unwrap();
            if research {
                s.enable_research(1).unwrap();
                s.gear_down = true;
                s.gear = 1.0;
                s.flaps_down = true;
                s.flaps = 1.0;
            }
            s.damage_regions = regions;
            s.yaw = 0.0;
            s.pitch = -3_f64.to_radians();
            s.bank = 0.0;
            s.speed = if research { 230.0 } else { 400.0 };
            s.velocity = crate::attitude::Basis::new(s.yaw, s.pitch, s.bank)
                .forward
                .map(|v| v * s.speed);
            let command = intent(0.0, -3.0, s.speed);
            for _ in 0..30 * 120 {
                let output = controls(&s, &command, crate::flight::DT);
                s.step(&output.input, |_, _| 0.0);
            }
            assert!(!s.crashed);
            attitude(&s).unwrap().flight_path_pitch_deg
        };
        for research in [false, true] {
            let clean = fly([0.0; 6], research);
            for regions in [
                [0.0, 0.0, 0.0, 0.0, 0.0, 0.25],
                [0.0, 0.0, 0.0, 0.3, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.2, 0.2, 0.5],
            ] {
                let damaged = fly(regions, research);
                assert!(
                    (damaged - clean).abs() < 0.25 && (damaged + 3.0).abs() < 0.5,
                    "research={research} {regions:?}: clean {clean}, damaged {damaged}"
                );
            }
        }
    }

    #[test]
    fn above_the_ceiling_the_hybrid_request_uses_the_thinned_lift() {
        // The flight model gives less than 1 G above the aircraft's 1 G ceiling
        // (hybrid only), so a pull request must move the stick differently there than the
        // same request just below it. (The legacy model's limits are pinned in
        // `flight.rs`.)
        let stick = |altitude: f64, research: bool| {
            let mut s = State::new(&profile(), [0.0, altitude, 0.0]).unwrap();
            if research {
                s.enable_research(1).unwrap();
            }
            s.speed = 900.0;
            s.velocity = [0.0, 0.0, 900.0];
            controls(&s, &intent(0.0, 5.0, 900.0), crate::flight::DT)
                .input
                .pitch
        };
        let (below, above) = (stick(49_000.0, true), stick(70_000.0, true));
        assert!(above.is_finite() && above != below, "{above} {below}");
    }

    #[test]
    fn formation_heading_follows_bank_through_a_shallow_turn() {
        for direction in [-1.0, 1.0] {
            let mut s = state();
            s.speed = 800.;
            s.velocity = [0., 0., 800.];
            let mut command = intent(direction * 10., 0., 800.);
            command.formation_flight = true;
            command.mode = CommandMode::OtherState;
            let mut peak_bank = 0.0_f64;
            for _ in 0..2400 {
                let output = controls(&s, &command, crate::flight::DT);
                assert_eq!(output.input.yaw, 0.);
                s.step(&output.input, |_, _| 0.0);
                peak_bank = peak_bank.max(s.bank.abs().to_degrees());
            }
            assert!(peak_bank > 20. && peak_bank <= 60.);
            assert!(heading_error_deg(s.yaw.to_degrees(), direction * 10.).abs() < 0.1);
            assert!(s.bank.abs().to_degrees() < 0.1);
        }
    }

    #[test]
    fn a_commanded_heading_change_rolls_into_the_turn_on_the_correct_side() {
        let mut s = state();
        let right = controls(&s, &intent(90.0, 0.0, s.speed), SECONDS_PER_TICK);
        assert!(right.input.roll > 0.0, "{}", right.input.roll);
        assert!(right.requested.bank_deg > 0.0);

        let left = controls(&s, &intent(270.0, 0.0, s.speed), SECONDS_PER_TICK);
        assert!(left.input.roll < 0.0, "{}", left.input.roll);
        assert!(left.requested.bank_deg < 0.0);

        let straight = controls(&s, &intent(0.0, 0.0, s.speed), SECONDS_PER_TICK);
        assert_eq!(straight.input.roll, 0.0);

        // Across the 0/360 wrap the shorter path still decides the side.
        s.yaw = 350f64.to_radians();
        s.velocity = crate::attitude::Basis::new(s.yaw, 0.0, 0.0)
            .forward
            .map(|v| v * s.speed);
        let over_zero = controls(&s, &intent(10.0, 0.0, s.speed), SECONDS_PER_TICK);
        assert!(over_zero.input.roll > 0.0, "{}", over_zero.input.roll);

        s.yaw = 10f64.to_radians();
        s.velocity = crate::attitude::Basis::new(s.yaw, 0.0, 0.0)
            .forward
            .map(|v| v * s.speed);
        let under_zero = controls(&s, &intent(350.0, 0.0, s.speed), SECONDS_PER_TICK);
        assert!(under_zero.input.roll < 0.0, "{}", under_zero.input.roll);
    }

    #[test]
    fn the_approach_stops_at_the_request_instead_of_overshooting() {
        // B44: a step long enough to cover the whole difference lands exactly
        // on the request, and the deflection it asks for is still bounded.
        let mut s = state();
        s.yaw = 89f64.to_radians();
        s.bank = 10f64.to_radians();
        s.velocity = crate::attitude::Basis::new(s.yaw, 0.0, 0.0)
            .forward
            .map(|v| v * s.speed);
        let mut command = intent(90.0, 5.0, s.speed);
        command.bank = Bank::Explicit(20);
        let long = controls(&s, &command, 10.0);
        assert_eq!(long.requested.heading_deg, 90.0);
        assert_eq!(long.requested.flight_path_pitch_deg, 5.0);
        assert_eq!(long.requested.bank_deg, 20.0);
        assert!(long.input.roll.abs() <= 1.0 && long.input.pitch.abs() <= 1.0);

        // Repeating from the arrived attitude asks for nothing further.
        s.yaw = 90f64.to_radians();
        s.bank = 20f64.to_radians();
        s.pitch = 5f64.to_radians();
        s.velocity = crate::attitude::Basis::new(s.yaw, 5f64.to_radians(), 0.0)
            .forward
            .map(|v| v * s.speed);
        let arrived = controls(&s, &command, 10.0);
        assert_eq!(arrived.requested.heading_deg, 90.0);
        assert!(arrived.input.roll.abs() < 1e-9, "{}", arrived.input.roll);
        assert!(
            arrived.input.pitch > 0.0,
            "a banked aircraft still needs lift to hold its flight path"
        );
    }

    #[test]
    fn a_climb_commands_nose_up_and_a_dive_commands_nose_down() {
        let s = state();
        let climb = controls(&s, &intent(0.0, 20.0, s.speed), SECONDS_PER_TICK);
        assert!(climb.input.pitch > 0.0, "{}", climb.input.pitch);
        assert!(climb.requested.flight_path_pitch_deg > 0.0);

        let dive = controls(&s, &intent(0.0, -20.0, s.speed), SECONDS_PER_TICK);
        assert!(dive.input.pitch < 0.0, "{}", dive.input.pitch);
        assert!(dive.requested.flight_path_pitch_deg < 0.0);

        let level = controls(&s, &intent(0.0, 0.0, s.speed), SECONDS_PER_TICK);
        assert_eq!(level.input.pitch, 0.0);
    }

    #[test]
    fn throttle_follows_the_commanded_speed_within_the_limits() {
        let mut s = state();
        s.throttle = 0.5;
        let faster = controls(&s, &intent(0.0, 0.0, s.speed + 50.0), SECONDS_PER_TICK);
        assert!(faster.input.throttle.unwrap() > 0.5);
        let slower = controls(&s, &intent(0.0, 0.0, s.speed - 50.0), SECONDS_PER_TICK);
        assert!(slower.input.throttle.unwrap() < 0.5);
        let held = controls(&s, &intent(0.0, 0.0, s.speed), SECONDS_PER_TICK);
        assert!((held.input.throttle.unwrap() - 0.5).abs() < 1e-9);

        // A request outside the aircraft's own limits is held at the limit.
        let absurd = controls(&s, &intent(0.0, 0.0, 99_000.0), SECONDS_PER_TICK);
        assert_eq!(absurd.input.throttle, Some(1.0));
        let stopped = controls(&s, &intent(0.0, 0.0, 0.0), SECONDS_PER_TICK);
        let at_minimum =
            throttle_command(0.5, ScalarSpeed(s.speed), limits().minimum, &limits()).unwrap();
        assert_eq!(stopped.input.throttle, Some(at_minimum));
    }

    #[test]
    fn every_control_is_finite_and_bounded_over_a_randomised_sweep() {
        let mut random = DecisionRandom::seeded(0x005E_EDA1);
        let mut s = state();
        let mut adapter = ControlAdapter::new();
        for _ in 0..4000 {
            s.yaw = f64::from(random.below(360)).to_radians();
            s.pitch = (f64::from(random.below(181)) - 90.0).to_radians();
            s.bank = (f64::from(random.below(361)) - 180.0).to_radians();
            s.speed = f64::from(random.below(1800));
            s.throttle = f64::from(random.below(101)) / 100.0;
            s.velocity = crate::attitude::Basis::new(s.yaw, s.pitch, s.bank)
                .forward
                .map(|v| v * s.speed);
            let mut command = intent(
                f64::from(random.below(360)),
                f64::from(random.below(181)) - 90.0,
                f64::from(random.below(2000)),
            );
            command.mode = match random.below(3) {
                0 => CommandMode::Ordinary,
                1 => CommandMode::ReducedRate,
                _ => CommandMode::OtherState,
            };
            if random.chance(50) {
                command.bank = Bank::Explicit(random.below(361) as i32 - 180);
            }
            let floor = random
                .chance(50)
                .then(|| f64::from(random.below(19)) * 5.0 - 90.0);
            let dt = if random.chance(10) {
                0.0
            } else {
                SECONDS_PER_TICK
            };
            let out = adapter
                .controls(
                    &s,
                    &command,
                    &limits(),
                    G_LIMIT,
                    ROLL_LIMIT,
                    MAXIMUM_BANK,
                    floor,
                    dt,
                )
                .unwrap();
            for axis in [out.input.pitch, out.input.roll, out.input.yaw] {
                assert!(axis.is_finite() && (-1.0..=1.0).contains(&axis), "{axis}");
            }
            let throttle = out.input.throttle.unwrap();
            assert!(
                throttle.is_finite() && (0.0..=1.0).contains(&throttle),
                "{throttle}"
            );
            assert!(out.requested.heading_deg.is_finite());
            assert!(out.requested.flight_path_pitch_deg.abs() <= 90.0 + 1e-9);
        }
    }

    #[test]
    fn the_experience_g_adjustment_applies_to_ai_aircraft_only() {
        assert_eq!(
            ai_g_limits(Experience::Novice, 9.0, -3.0, false),
            (8.0, -2.0)
        );
        assert_eq!(
            ai_g_limits(Experience::Average, 7.5, -4.0, false),
            (6.5, -3.0)
        );
        // The 2 G and -2 G floors.
        assert_eq!(
            ai_g_limits(Experience::Novice, 2.5, -2.5, false),
            (2.0, -2.0)
        );
        assert_eq!(
            ai_g_limits(Experience::Average, 2.0, -2.0, false),
            (2.0, -2.0)
        );
        // Experienced and Ace are unchanged.
        assert_eq!(
            ai_g_limits(Experience::Experienced, 9.0, -3.0, false),
            (9.0, -3.0)
        );
        assert_eq!(ai_g_limits(Experience::Ace, 9.0, -3.0, false), (9.0, -3.0));
        // A human-flown aircraft keeps its full limits at every level.
        for level in Experience::ALL {
            assert_eq!(ai_g_limits(level, 9.0, -3.0, true), (9.0, -3.0));
        }
    }

    #[test]
    fn the_base_pitch_rate_fallback_is_reported() {
        let s = state();
        let out = controls(&s, &intent(90.0, 10.0, s.speed), SECONDS_PER_TICK);
        assert_eq!(out.fallbacks, vec![Fallback::BasePitchRate]);
    }

    #[test]
    fn the_same_state_and_intent_give_identical_output_twice() {
        let s = state();
        let command = intent(123.0, -12.0, 700.0);
        let mut adapter = ControlAdapter::new();
        let run = |adapter: &mut ControlAdapter| {
            adapter
                .controls(
                    &s,
                    &command,
                    &limits(),
                    G_LIMIT,
                    ROLL_LIMIT,
                    MAXIMUM_BANK,
                    Some(-10.0),
                    SECONDS_PER_TICK,
                )
                .unwrap()
        };
        let first = run(&mut adapter);
        let second = run(&mut adapter);
        assert_eq!(first, second);
        assert_eq!(
            first.input.pitch.to_bits(),
            second.input.pitch.to_bits(),
            "pitch is not bit-identical"
        );
        assert_eq!(first.input.roll.to_bits(), second.input.roll.to_bits());
        assert_eq!(
            first.input.throttle.unwrap().to_bits(),
            second.input.throttle.unwrap().to_bits()
        );
        // A fresh adapter produces the same output as a used one.
        assert_eq!(first, run(&mut ControlAdapter::new()));
    }

    #[test]
    fn invalid_inputs_are_rejected_rather_than_flown() {
        let s = state();
        let command = intent(90.0, 0.0, s.speed);
        let mut adapter = ControlAdapter::new();
        let call = |adapter: &mut ControlAdapter, bank, dt| {
            adapter.controls(&s, &command, &limits(), G_LIMIT, ROLL_LIMIT, bank, None, dt)
        };
        assert!(call(&mut adapter, 0.0, SECONDS_PER_TICK).is_err());
        assert!(call(&mut adapter, MAXIMUM_BANK, -1.0).is_err());
        assert!(call(&mut adapter, MAXIMUM_BANK, f64::NAN).is_err());
        assert!(adapter.last_requested().is_none());
        assert!(call(&mut adapter, MAXIMUM_BANK, SECONDS_PER_TICK).is_ok());
        assert!(adapter.last_requested().is_some());
    }

    #[test]
    fn below_the_terrain_floor_the_wings_level_before_the_turn() {
        // Battery finding (2026-09-28): an AI F-14 at 72 degrees of bank and
        // 20 degrees nose down kept turning at full G toward its target and
        // flew into the ground with the terrain floor asking for a climb.
        let banked_dive = |bank_deg: f64, path_deg: f64, altitude: f64| {
            let mut s = State::new(&profile(), [0.0, altitude, 0.0]).unwrap();
            s.yaw = 0.0;
            s.bank = bank_deg.to_radians();
            s.pitch = path_deg.to_radians();
            let path = path_deg.to_radians();
            s.velocity = [0.0, s.speed * path.sin(), s.speed * path.cos()];
            s
        };
        let command = intent(120.0, 10.0, 700.0);
        let s = banked_dive(70.0, -20.0, 1500.0);
        let run = |floor| {
            ControlAdapter::new()
                .controls(
                    &s,
                    &command,
                    &limits(),
                    G_LIMIT,
                    ROLL_LIMIT,
                    MAXIMUM_BANK,
                    floor,
                    SECONDS_PER_TICK,
                )
                .unwrap()
        };
        // Above the floor the turn continues; below it the wings come level.
        assert!(run(Some(-30.0)).input.roll > 0.0);
        assert!(run(Some(-5.0)).input.roll < 0.0);
        // Flown closed loop over flat ground, the aircraft recovers.
        let mut s = banked_dive(70.0, -20.0, 1500.0);
        let mut adapter = ControlAdapter::new();
        let mut lowest = f64::MAX;
        for _ in 0..1200 {
            let out = adapter
                .controls(
                    &s,
                    &command,
                    &limits(),
                    G_LIMIT,
                    ROLL_LIMIT,
                    MAXIMUM_BANK,
                    Some(if s.position[1] < 1800.0 { 0.0 } else { -90.0 }),
                    crate::flight::DT,
                )
                .unwrap();
            s.step(&out.input, |_, _| 0.0);
            lowest = lowest.min(s.position[1]);
        }
        assert!(!s.crashed, "flew into the ground");
        assert!(lowest > 300.0, "lowest {lowest:.0} ft");
        // Level at speed with the floor asking for 15 degrees of climb, the
        // pull uses most of the G limit instead of easing up over 3 s.
        let level = banked_dive(0.0, 0.0, 1500.0);
        let pull = ControlAdapter::new()
            .controls(
                &level,
                &intent(0.0, 0.0, level.speed),
                &limits(),
                G_LIMIT,
                ROLL_LIMIT,
                MAXIMUM_BANK,
                Some(15.0),
                SECONDS_PER_TICK,
            )
            .unwrap();
        let ease = ControlAdapter::new()
            .controls(
                &level,
                &intent(0.0, 15.0, level.speed),
                &limits(),
                G_LIMIT,
                ROLL_LIMIT,
                MAXIMUM_BANK,
                None,
                SECONDS_PER_TICK,
            )
            .unwrap();
        assert!(pull.input.pitch > ease.input.pitch * 2.0);
    }

    #[test]
    fn stepping_a_real_flight_state_turns_the_aircraft_toward_the_command() {
        let mut s = State::new(&profile(), [0.0, 15000.0, 0.0]).unwrap();
        s.yaw = 0.0;
        s.pitch = 0.0;
        s.bank = 0.0;
        s.velocity = [0.0, 0.0, s.speed];
        let command = intent(90.0, 0.0, s.speed);
        let mut adapter = ControlAdapter::new();
        let start = heading_error_deg(0.0, 90.0).abs();
        let mut banked = 0.0f64;
        for _ in 0..600 {
            let out = adapter
                .controls(
                    &s,
                    &command,
                    &limits(),
                    G_LIMIT,
                    ROLL_LIMIT,
                    MAXIMUM_BANK,
                    None,
                    crate::flight::DT,
                )
                .unwrap();
            s.step(&out.input, |_, _| 0.0);
            banked = banked.max(s.bank.to_degrees());
        }
        assert!(!s.crashed);
        let heading = s.yaw.to_degrees().rem_euclid(360.0);
        let error = heading_error_deg(heading, 90.0).abs();
        assert!(
            banked > 30.0,
            "the aircraft never rolled into the turn: {banked}"
        );
        assert!(
            error < start - 30.0,
            "heading {heading} did not close on 90 from an error of {start}"
        );
    }

    #[test]
    fn the_roll_and_pull_rolls_inverted_for_a_dive_and_pushes_for_a_small_change() {
        let s = state();
        // Level, wanting 60 degrees down: inverted, pulling.
        let dive = roll_and_pull(&s, 0.0, 0.0, -60.0, G_LIMIT).unwrap();
        assert!(dive.bank_deg.abs() > 170.0, "{dive:?}");
        assert!(dive.load_g > 1.0);
        // A few degrees down: wings level, unloaded or pushed, never rolled.
        let nudge = roll_and_pull(&s, 0.0, 0.0, -3.0, G_LIMIT).unwrap();
        assert!(
            nudge.bank_deg.abs() < 1.0 && nudge.load_g < 1.0,
            "{nudge:?}"
        );
        // Wanting a right turn: banked right, pulling.
        let turn = roll_and_pull(&s, 0.0, 20.0, 0.0, G_LIMIT).unwrap();
        assert!(turn.bank_deg > 30.0 && turn.load_g > 1.0, "{turn:?}");
        // Inverted already, a push still wanted: it stays inverted.
        let held = roll_and_pull(&s, 175.0, 0.0, -10.0, G_LIMIT).unwrap();
        assert!(held.bank_deg.abs() > 90.0, "{held:?}");
        // Straight and level as wanted: one G, wings level.
        let steady = roll_and_pull(&s, 0.0, 0.0, 0.0, G_LIMIT).unwrap();
        assert!((steady.load_g - 1.0).abs() < 1e-6 && steady.bank_deg.abs() < 1e-6);
    }

    #[test]
    fn a_roll_and_pull_noses_the_aircraft_into_a_steep_dive_quickly() {
        // Flown closed loop: the plain adapter's push and the roll and pull
        // from level toward a 60 degree dive at 15,000 ft. The roll and pull
        // gets there sooner, as retail's inverted pull does.
        let fly = |pull: bool| {
            let mut s = state();
            let mut adapter = ControlAdapter::new();
            adapter.set_strike_handling(if pull {
                Handling::PULL
            } else {
                Handling::PLAIN
            });
            let command = intent(0.0, -60.0, 700.0);
            for tick in 0..1200 {
                let out = adapter
                    .controls(
                        &s,
                        &command,
                        &limits(),
                        G_LIMIT,
                        ROLL_LIMIT,
                        MAXIMUM_BANK,
                        None,
                        crate::flight::DT,
                    )
                    .unwrap();
                s.step(&out.input, |_, _| 0.0);
                let v = s.velocity;
                if v[1].atan2(v[0].hypot(v[2])).to_degrees() < -50.0 {
                    return tick;
                }
            }
            1200
        };
        let (pulled, pushed) = (fly(true), fly(false));
        assert!(pulled < pushed, "pull {pulled} ticks, push {pushed} ticks");
        assert!(pulled < 6 * 120, "{pulled}");
    }

    #[test]
    fn plain_handling_flies_exactly_as_an_adapter_that_never_heard_of_it() {
        let s = state();
        let command = intent(40.0, -10.0, 650.0);
        let fresh = controls(&s, &command, crate::flight::DT);
        let mut adapter = ControlAdapter::new();
        adapter.set_strike_handling(Handling::PULL);
        adapter.set_strike_handling(Handling::PLAIN);
        let reset = adapter
            .controls(
                &s,
                &command,
                &limits(),
                G_LIMIT,
                ROLL_LIMIT,
                MAXIMUM_BANK,
                None,
                crate::flight::DT,
            )
            .unwrap();
        assert_eq!(fresh, reset);
    }
}
