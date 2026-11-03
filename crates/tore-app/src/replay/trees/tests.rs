//! Tree builders against synthetic records.
use super::*;
use tore_sim::ai::{
    controller::EmploymentCheck,
    engagement::{CandidateExplanation, Explanation, Ineligibility, Selection},
    experience::{ExperienceOrigin, ResolvedExperience},
    geometry::{RelativeAngles, TargetGeometry},
    tactics::{BehaviorChoice, QuadrantThresholds},
    thought::{
        ActorTrace, EngagementTrace, LockTrace, ManeuverPath, ManeuverTrace, MotionTrace,
        StationTrace, TargetTrace, WeaponTrace,
    },
    weapon_service::{
        Delay, LockStatus, ProjectilePacing, Rounds, ServiceInputs, StoreCapability, TargetClass,
        TargetId,
    },
};

const NM: f64 = FT_PER_NM;

fn who(id: u32) -> String {
    match id {
        0 => "You".into(),
        7 => "Friendly 1-2".into(),
        9 => "Enemy 2-2".into(),
        other => format!("aircraft {other}"),
    }
}

fn store(_: StationId) -> Option<String> {
    Some("AA-10".into())
}

/// A node by label, anywhere in the tree.
fn find<'a>(nodes: &'a [Node], label: &str) -> &'a Node {
    nodes
        .iter()
        .find(|n| n.label == label)
        .unwrap_or_else(|| panic!("no {label} in\n{}", render(nodes)))
}

fn pacing() -> ProjectilePacing {
    ProjectilePacing {
        burst_count: 1,
        burst_interval: Delay::seconds(1),
        reload: Delay::seconds(2),
        startup: Delay::seconds(0),
    }
}

fn station_view(station: u8) -> StationView {
    StationView {
        station: StationId(station),
        guided: true,
        capability: StoreCapability::AIR_TO_AIR_MISSILE,
        inhibited: false,
        rounds: Rounds::Finite(2),
        pointing_error_deg: 12.,
        employment_limit_deg: Some(30.),
        employment_fit: None,
        minimum_range_ft: 1_500.,
        maximum_range_ft: Some(5.8 * NM),
        energy_target: None,
        infrared_envelope: None,
        requires_radar: true,
        requires_sensor: false,
        employment_zone: None,
        mount: [0.; 3],
        damage_vs_category: 1.,
        store_speed: tore_sim::ai::ScalarSpeed(2_000.),
        tracking_delay: Delay::seconds(1),
        pacing: pacing(),
    }
}

#[track_caller]
fn draw(site: &'static str, value: u32, threshold: Option<u32>) -> Draw {
    Draw {
        site: Some(site),
        location: std::panic::Location::caller(),
        bound: 100,
        value,
        offset: 0,
        threshold,
    }
}

fn candidate(
    id: u32,
    priority: Option<Priority>,
    score_ft: f64,
    ineligible: Ineligibility,
) -> CandidateExplanation {
    CandidateExplanation {
        id,
        ineligible,
        priority,
        distance_ft: score_ft,
        not_aircraft_penalty: false,
        wing_attacking_penalty: false,
        wing_full_penalty: false,
        score_ft,
        chosen: id == 0,
    }
}

/// A fighter outside its missile's range, chasing the player it was
/// assigned, with a runner-up and an aircraft it may not attack.
fn records() -> (ControllerTrace, ActorTrace) {
    let controller = ControllerTrace {
        tick: Some(1_200),
        path: StepPath::Decided,
        target: TargetTrace {
            previous: Some(0),
            path: TargetPath::Mission {
                requested: Some(0),
                refused: None,
            },
            chosen: Some(0),
            search_ended: false,
        },
        geometry: Some(TargetGeometry {
            spatial_distance_feet: 6.1 * NM,
            horizontal_distance_feet: 6.0 * NM,
            angles: Some(RelativeAngles {
                heading_error_deg: -12.,
                pitch_error_deg: 3.,
                off_beam_deg: 12.,
                ahead: true,
                facing: true,
                heading_difference_deg: 160.,
                pitch_difference_deg: 3.,
            }),
        }),
        weapons: Some(WeaponTrace {
            class: TargetClass::Air,
            stations: vec![StationTrace {
                index: 0,
                station: StationId(3),
                verdict: Some(StationVerdict::OutsideEnvelope),
                employment: Some(EmploymentCheck {
                    range_ft: 6.1 * NM,
                    error_deg: 12.,
                    below_minimum_range: false,
                    beyond_maximum_range: true,
                    beyond_angle_limit: false,
                    outside_zone: false,
                }),
                hit_chance: None,
                score: None,
            }],
            chosen: None,
            lock: LockTrace {
                locked: false,
                has_target: true,
                has_station: false,
                target_ahead: Some(true),
                terrain_blocked: false,
            },
            inputs: ServiceInputs {
                target: Some(TargetId(0)),
                station: None,
                unready: false,
                lock: LockStatus::Failed,
                path_blocked: false,
                pacing: pacing(),
            },
            phase_before: Phase::Search,
            outcome: Some(ServiceOutcome::NoStationRetry { deadline: 48 }),
            burst_pacing_restart: false,
            phase_after: Phase::Search,
            deadline: Some(48),
        }),
        motion: MotionTrace {
            branch: MotionBranch::NewManeuver,
            next_choice_at: Some(44),
            ..MotionTrace::default()
        },
        ..ControllerTrace::default()
    };
    let actor = ActorTrace {
        tick: Some(1_200),
        path: ActorPath::Controller,
        stations: vec![station_view(3)],
        engagement: Some(EngagementTrace {
            neutral: false,
            role: Role::FreeEngagement,
            stance: Stance::EngageAssigned,
            reports: Vec::new(),
            selection: Some(Selection {
                id: 0,
                priority: Priority::Assigned,
            }),
            must_rejoin: false,
            explanation: Explanation {
                best_priority: Some(Priority::Assigned),
                chosen: Some(Selection {
                    id: 0,
                    priority: Priority::Assigned,
                }),
                candidates: vec![
                    candidate(
                        0,
                        Some(Priority::Assigned),
                        18_400.,
                        Ineligibility::default(),
                    ),
                    candidate(7, Some(Priority::Free), 26_100., Ineligibility::default()),
                    candidate(
                        9,
                        None,
                        4_000.,
                        Ineligibility {
                            same_side: true,
                            ..Ineligibility::default()
                        },
                    ),
                ],
                ..Explanation::default()
            },
        }),
        ..ActorTrace::default()
    };
    (controller, actor)
}

fn choice() -> Choice {
    Choice {
        tick: 1_190,
        maneuver: ManeuverTrace {
            path: ManeuverPath::Approach,
            situation: None,
            quadrant: Some(Quadrant::AheadFacing),
            thresholds: Some(QuadrantThresholds {
                best_attack_percent: 50,
                random_tactic_percent: 30,
            }),
            choice: Some(BehaviorChoice::Pursuit),
            fallback: None,
            last_ditch: None,
            request: None,
        },
        resolve: None,
        draws: vec![draw("best attack", 37, Some(50)), draw("offset", 12, None)],
    }
}

fn thought<'a>(
    controller: &'a ControllerTrace,
    actor: &'a ActorTrace,
    choice: Option<&'a Choice>,
    recent: &'a [Change],
) -> Thought<'a> {
    Thought {
        tick: 1_200,
        label: "Enemy 2-1",
        name: "MiG-29",
        side: "enemy",
        wing: 2,
        member: 1,
        leader: true,
        experience: Some(ResolvedExperience {
            level: tore_sim::ai::Experience::Experienced,
            origin: ExperienceOrigin::QuickMission {
                selected: tore_sim::ai::Experience::Experienced,
            },
        }),
        activity: Activity::Acquiring,
        activity_since: 696,
        activity_reason: "You chosen; no firing solution yet",
        target: Some(0),
        weapon_phase: Phase::Search,
        neutral: false,
        airfield: None,
        defense: None,
        threat: None,
        controller,
        actor,
        draws: &[],
        choice,
        relative: Some(Relative {
            aspect_deg: 35.2,
            closure_kt: 820.4,
            height_ft: -2_304.,
        }),
        own: Own {
            position: [0.; 3],
            speed_fps: 700.,
            g: 5.94,
            g_limit: Some(6.),
            fuel_lb: 2_140.4,
            afterburner: false,
        },
        fallbacks: &[],
        recent,
        who: &who,
        store: &store,
    }
}

#[test]
fn a_thought_tree_says_why_the_weapon_has_not_fired_with_its_numbers() {
    let (controller, actor) = records();
    let choice = choice();
    let recent = [Change {
        tick: 696,
        what: "In formation -> Acquiring".into(),
        why: "the leader released the wing".into(),
    }];
    let nodes = ai_thought(&thought(&controller, &actor, Some(&choice), &recent));
    let text = render(&nodes);
    assert_eq!(
        find(&nodes, node::ACTIVITY).value,
        Value::Text("Acquiring".into())
    );
    assert_eq!(find(&nodes, "Since").value, Value::Text("0:05.8".into()));
    let target = find(&nodes, node::TARGET);
    assert_eq!(
        (target.value.clone(), target.note.as_str()),
        (Value::Id(0), "You")
    );
    assert_eq!(
        find(&nodes, "Chosen by").value,
        Value::Text("mission ranking".into())
    );
    let score = find(&nodes, "Score");
    assert_eq!(score.value, Value::Num(18_400.));
    assert!(score.note.contains("priority assigned"), "{text}");
    let runner_up = find(&nodes, "Runner-up");
    assert_eq!(runner_up.value, Value::Num(26_100.));
    assert_eq!(runner_up.note, "Friendly 1-2: free");
    assert_eq!(
        find(&nodes, "Enemy 2-2").value,
        Value::Text("not eligible: same side".into())
    );
    assert_eq!(find(&nodes, "Geometry").value, Value::Num(6.1));
    assert_eq!(find(&nodes, "Height").value, Value::Num(-2_300.));
    let fire = find(&nodes, "Fire?");
    assert_eq!(fire.value, Value::Text("not yet".into()));
    assert_eq!(fire.note, "outside max range (6.1 nm > 5.8 nm)");
    // The service's retry as a mission clock time: quarter 48 is 12 s.
    assert_eq!(
        find(&nodes, "Service").value,
        Value::Text("no usable store; retry at 0:12.0".into())
    );
    assert_eq!(find(&nodes, "AA-10").note, "station 3; too far");
    let choice = find(&nodes, "Choice");
    assert_eq!(choice.value, Value::Text("Pursuit".into()));
    assert_eq!(
        choice.note,
        "roll 37 < 50: best attack; draw 12 of 0..99: offset"
    );
    assert_eq!(
        find(&nodes, "Situation").note,
        "best attack 50%, random tactic 30%"
    );
    assert_eq!(
        find(&nodes, "Next choice").value,
        Value::Text("0:11.0".into())
    );
    assert_eq!(find(&nodes, "Fuel").value, Value::Num(2_140.));
    assert_eq!(find(&nodes, "0:05.8").note, "the leader released the wing");
    assert!(
        find(&nodes, "Aircraft")
            .note
            .contains("Experienced (the Quick Mission wing setting)")
    );
    // Depths grow one level at a time from the top.
    assert_eq!(nodes[0].depth, 0);
    for pair in nodes.windows(2) {
        assert!(pair[1].depth <= pair[0].depth + 1, "{text}");
    }
}

#[test]
fn a_stale_controller_record_is_not_presented_as_this_ticks_decision() {
    let (mut controller, mut actor) = records();
    controller.tick = Some(1_000);
    actor.path = ActorPath::Airfield;
    let nodes = ai_thought(&thought(&controller, &actor, None, &[]));
    assert_eq!(
        find(&nodes, "Motion").value,
        Value::Text("not deciding".into())
    );
    assert_eq!(
        find(&nodes, "Motion").note,
        "a takeoff or landing sequence flies it"
    );
    assert!(
        nodes
            .iter()
            .all(|n| n.label != "Geometry" && n.label != "Fire?")
    );
    // The weapon phase still shows, from the controller's own state.
    assert_eq!(find(&nodes, "Phase").value, Value::Text("Search".into()));
}

#[test]
fn a_blocked_lock_a_lock_and_a_shot_read_differently() {
    let (mut controller, actor) = records();
    let weapons = controller.weapons.as_mut().unwrap();
    weapons.chosen = Some(StationId(3));
    weapons.lock = LockTrace {
        locked: false,
        has_target: true,
        has_station: true,
        target_ahead: Some(true),
        terrain_blocked: true,
    };
    let views = &actor.stations;
    let at = |q: u64| clock(q * 30);
    assert_eq!(
        fire_reason(weapons, Some(0), views, &at).as_deref(),
        Some("no lock: terrain blocks the line of fire")
    );
    weapons.lock.locked = true;
    weapons.outcome = Some(ServiceOutcome::Locked { fire_at: 50 });
    assert_eq!(
        fire_reason(weapons, Some(0), views, &at).as_deref(),
        Some("locked; fires at 0:12.5 after its tracking delay")
    );
    weapons.outcome = Some(ServiceOutcome::Fire(
        tore_sim::ai::weapon_service::FireRequest {
            actor: tore_sim::ai::weapon_service::ActorId(1),
            station: StationId(3),
            target: TargetId(0),
            request_id: tore_sim::ai::weapon_service::RequestId(1),
        },
    ));
    assert_eq!(fire_reason(weapons, Some(0), views, &at), None);
}

#[test]
fn draws_read_as_rolls_against_their_thresholds() {
    assert_eq!(
        draw_text(&draw("best attack", 37, Some(50))),
        "roll 37 < 50: best attack"
    );
    assert_eq!(
        draw_text(&draw("decoy roll", 60, Some(14))),
        "roll 60 >= 14: decoy roll"
    );
    let mut signed = draw("offset", 5, None);
    signed.offset = -15;
    signed.bound = 30;
    assert_eq!(draw_text(&signed), "draw -10 of -15..14: offset");
}

fn air() -> tore_sim::telemetry::AirData {
    tore_sim::telemetry::AirData {
        tick: 10,
        altitude_msl_ft: 12_340.,
        altitude_agl_ft: 11_200.,
        true_airspeed_knots: 480.26,
        equivalent_airspeed_knots: 400.,
        ground_speed_knots: 470.,
        mach: 0.7812,
        vertical_speed_fpm: 0.,
        angle_of_attack_deg: Some(12.34),
        sideslip_deg: Some(0.41),
        heading_true_deg: 90.,
        pitch_deg: 2.,
        bank_deg: 60.,
        load_factor_g: 5.4,
        density_kg_m3: 0.8,
        dynamic_pressure_pa: 29_200.,
        static_pressure_pa: 60_000.,
        temperature_k: 260.,
        indicated_airspeed_knots: None,
        calibrated_airspeed_knots: None,
        indicated_altitude_ft: None,
        pressure_altitude_ft: None,
    }
}

fn damaged_trace() -> FlightTrace {
    use tore_sim::flight::trace::{AdapterTrace, EnvelopeTrace, LiftTrace, LowSpeedCeiling};
    let adapter = AdapterTrace {
        envelope: EnvelopeTrace {
            clean_stall_fps: 240.,
            stall_fps: 240.,
            authority: 1.,
            rows: 3,
            envelope_g: [-3., 9.],
            loading: 0.4,
            load_divisor: 1.2,
            loaded_positive_g: 7.5,
            low_speed_ceiling: Some(LowSpeedCeiling {
                limit_g: 8.16,
                from_fps: 240.,
                to_fps: 420.,
                to_g: 9.,
                fraction: 0.9,
            }),
            limits_g: [-2.5, 6.8],
            stick: 1.,
            stick_g: 6.8,
            ..EnvelopeTrace::default()
        },
        lift: LiftTrace {
            wing_damaged: true,
            flap_factor: 1.,
            spin_factor: 1.,
            commanded_g: 3.4,
            ..LiftTrace::default()
        },
        ..AdapterTrace::default()
    };
    FlightTrace {
        tick: 10,
        path: Path::Hybrid,
        adapter: Some(adapter),
        ..FlightTrace::default()
    }
}

#[test]
fn telemetry_labels_measurements_and_gives_each_effect_its_factor_and_cause() {
    let trace = damaged_trace();
    let air = air();
    let nodes = flight_telemetry(&Telemetry {
        label: "You",
        name: "F/A-18D",
        trace: &trace,
        air: Some(&air),
        altitude_ft: 12_340.4,
        agl_ft: 11_200.4,
        g: 5.4,
        fuel_lb: 8_120.,
        rates: [0.5, 0.1, 0.],
        throttle: 0.9,
        afterburner: false,
    });
    let text = render(&nodes);
    // The exports read these labels and units.
    assert_eq!(find(&nodes, node::AGL).value, Value::Num(11_200.));
    assert_eq!(find(&nodes, node::AGL).unit, unit::FT);
    assert_eq!(find(&nodes, node::TAS).value, Value::Num(480.));
    assert_eq!(find(&nodes, node::MACH).value, Value::Num(0.78));
    assert_eq!(find(&nodes, node::AOA).value, Value::Num(12.3));
    assert_eq!(find(&nodes, node::SIDESLIP).value, Value::Num(0.4));
    assert_eq!(find(&nodes, node::LOAD).value, Value::Num(5.4));
    assert_eq!(find(&nodes, node::G_LIMIT).value, Value::Num(6.8));
    assert_eq!(find(&nodes, "Air data").note, "measurements, never causes");
    assert_eq!(find(&nodes, "q").value, Value::Num(610.));
    // Each effect names what it applied and why; a measurement is never a
    // cause.
    let effects = trace.effects();
    assert_eq!(
        find(&nodes, "Effects applied").value,
        Value::Int(effects.len() as i64)
    );
    let wing = find(&nodes, "Wing damaged");
    assert_eq!(
        (wing.value.clone(), wing.note.as_str()),
        (Value::Num(0.5), "the wing system failed")
    );
    let ceiling = find(&nodes, "Low-speed G ceiling");
    assert_eq!(ceiling.value, Value::Num(6.8));
    assert!(
        ceiling.note.starts_with("speed near the stall speed"),
        "{text}"
    );
    assert_eq!(find(&nodes, "Loaded G limits").value, Value::Num(1.2));
    for n in &nodes {
        let note = n.note.to_lowercase();
        assert!(
            !note.contains("angle of attack") && !note.contains("mach"),
            "a measurement given as a cause: {text}"
        );
    }
}

#[test]
fn every_effect_has_words_and_keys_tell_held_devices_apart() {
    use tore_sim::flight::trace::{Device, DeviceKind};
    let held = |device| Effect::DeviceHeld {
        device,
        state: Device {
            commanded: true,
            position: 0.3,
            blocked: Some(Block::NoHydraulics),
        },
    };
    let gear = held(DeviceKind::Gear);
    let flaps = held(DeviceKind::Flaps);
    assert_ne!(effect_key(&gear), effect_key(&flaps));
    assert_eq!(effect_key(&gear), effect_key(&held(DeviceKind::Gear)));
    let line = effect_line(&flaps);
    assert_eq!(line.label, "Flaps held");
    assert_eq!(line.factor, "stuck at 30%");
    assert_eq!(line.because, "no hydraulic pressure");
    for effect in damaged_trace().effects() {
        let line = effect_line(&effect);
        assert!(
            !line.label.is_empty() && !line.factor.is_empty(),
            "{effect:?}"
        );
    }
    assert!(momentary(&Effect::SurfaceDropped));
    assert!(!momentary(&Effect::WingDamaged));
}

#[test]
fn guidance_trees_carry_seeker_range_and_closest_approach() {
    let nodes = weapon_guidance(&Guidance {
        weapon: "AIM-120",
        shooter: 0,
        target: Some(9),
        mode: "cued",
        guidance: "active radar",
        seeker: Some(SeekerView {
            status: "LOCK",
            quality: 0.834,
            acquired: true,
            tracking: Some(9),
        }),
        enabled: true,
        age: 330,
        speed_fps: 2_531.8,
        range_ft: Some(4_200.4),
        closest: Some((4_200.4, 1_330)),
        intercept_s: Some(1.73),
        decoy: Some("resisted chaff from Enemy 2-2: roll 60 >= 14: decoy roll".into()),
        who: &who,
    });
    assert_eq!(find(&nodes, "Shooter").note, "You");
    assert_eq!(find(&nodes, node::TARGET).value, Value::Id(9));
    assert_eq!(find(&nodes, "Seeker").value, Value::Text("LOCK".into()));
    assert_eq!(find(&nodes, "Quality").value, Value::Num(0.83));
    assert_eq!(find(&nodes, "Time of flight").value, Value::Num(2.8));
    assert_eq!(find(&nodes, "Closest approach").note, "at 0:11.0");
    assert!(
        find(&nodes, "Decoy roll")
            .value
            .as_str()
            .unwrap()
            .starts_with("resisted")
    );
}

#[test]
fn rounding_stores_short_decimals_and_never_negative_zero() {
    assert_eq!(round(12.345, 1), 12.3);
    assert_eq!(round(2_345., -1), 2_350.);
    assert_eq!(round(-0.0001, 2).to_bits(), 0f64.to_bits());
    assert!(round(f64::NAN, 1).is_nan(), "kept for the anomaly check");
    assert_eq!(format!("{}", round(0.1 + 0.2, 2)), "0.3");
    assert_eq!(distance(6.1 * NM), "6.1 nm");
    assert_eq!(distance(1_500.), "1,500 ft");
    assert_eq!(clock(120 * 754 + 60), "12:34.5");
}
