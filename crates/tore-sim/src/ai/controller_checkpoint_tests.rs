//! Round trips of the controller's checkpoint coders, on scripted controllers
//! that fly the same kinds of scenes as the AI controller golden
//! (`golden_tests/ai.rs`): a leader, a wingman and an ace under orders and
//! mission overrides, with launch warnings, hits, lost targets, a gun and
//! missile stores; and a wingman repositioning around same-tick traffic.
//!
//! The golden's own fixtures are private to that file, so these scenes are
//! rebuilt here on flat ground (the golden's terrain function is not needed to
//! exercise the coders).

use super::super::{
    ActorIdentity, BehaviorFamily, BehaviorProfile, Controller, DecisionFrame, DefenseMotion,
    DefenseSource, FrameEvent, IntentBatch, LeaderView, MissionRole, OwnState, RouteView,
    SearchContact, StationView, TargetView, ThreatReport, WingView,
};
use crate::ai::experience::{ExperienceOrigin, ResolvedExperience};
use crate::ai::formation::Traffic;
use crate::ai::gunnery::{Aim, Solution, View};
use crate::ai::route::Position;
use crate::ai::targeting::Side;
use crate::ai::threat::{DispenserStore, FlightState, SeekerClass, TimeOfDay};
use crate::ai::weapon_service::{
    ActorId, Delay, ProjectilePacing, Rounds, StationId, StoreCapability,
};
use crate::ai::wing::{
    Formation, ReceiverOutcome, SpacingAxis, TargetId, TargetOrder, WingControl, WingRequest,
};
use crate::ai::{Experience, ScalarSpeed, SpeedLimits};
use crate::attitude::Basis;
use crate::checkpoint::{Coded, Models, from_bytes, round_trip, to_bytes};
use tore_formats::aircraft::AircraftId;

fn resolved(level: Experience) -> ResolvedExperience {
    ResolvedExperience {
        level,
        origin: ExperienceOrigin::QuickMission { selected: level },
    }
}

fn limits() -> SpeedLimits {
    SpeedLimits {
        minimum: ScalarSpeed(220.),
        maximum: ScalarSpeed(1500.),
        corner: ScalarSpeed(700.),
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn wrap(degrees: f64) -> f64 {
    (degrees + 180.).rem_euclid(360.) - 180.
}

fn pacing(burst: u32, interval: Delay, reload: Delay) -> ProjectilePacing {
    ProjectilePacing {
        burst_count: burst,
        burst_interval: interval,
        reload,
        startup: Delay::seconds(0),
    }
}

/// A body that follows the controller's motion intents loosely, enough for
/// the scenes to go somewhere.
#[derive(Clone, Copy)]
struct PointMass {
    position: [f64; 3],
    heading: f64,
    pitch: f64,
    bank: f64,
    speed: f64,
}

impl PointMass {
    fn follow(&mut self, motion: Option<&super::super::MotionIntent>) {
        let dt = crate::flight::DT;
        if let Some(motion) = motion {
            let turn = wrap(motion.heading_deg - self.heading).clamp(-15. * dt, 15. * dt);
            self.heading = wrap(self.heading + turn);
            self.bank = (wrap(motion.heading_deg - self.heading) * 3.).clamp(-70., 70.);
            self.pitch += (motion.flight_path_pitch_deg.clamp(-30., 30.) - self.pitch)
                .clamp(-8. * dt, 8. * dt);
            self.speed += (motion.speed.0 - self.speed).clamp(-25. * dt, 25. * dt);
        }
        let forward = Basis::new(self.heading.to_radians(), self.pitch.to_radians(), 0.).forward;
        for (position, axis) in self.position.iter_mut().zip(forward) {
            *position += axis * self.speed * dt;
        }
    }

    fn own(&self, endurance_s: f64, radar: bool, damaged: bool) -> OwnState {
        OwnState {
            position: self.position,
            heading_deg: self.heading,
            flight_path_pitch_deg: self.pitch,
            body_pitch_offset_deg: 3.,
            bank_deg: self.bank,
            speed: ScalarSpeed(self.speed),
            limits: limits(),
            altitude_msl_ft: self.position[1],
            agl_ft: self.position[1],
            terrain_ahead_ft: 0.,
            terrain_climb_deg: -90.,
            minimum_altitude_ft: 300.,
            at_ceiling: false,
            on_ground: false,
            g_limit: if damaged { 4.5 } else { 7. },
            roll_limit_deg_per_s: 180.,
            maximum_bank_deg: 80.,
            alive: true,
            fuel_endurance_s: endurance_s,
            time_home_s: Some(self.position[0].hypot(self.position[2]) / 700.),
            rotorcraft: false,
            internal_fuel_lbs: endurance_s * 2.,
            radar_emitting: radar,
        }
    }
}

fn target_view(id: u32, side: u32, position: [f64; 3]) -> TargetView {
    TargetView {
        infrared_strength: 1.,
        id,
        side: Side(side),
        position,
        heading_deg: 180.,
        pitch_deg: 0.,
        speed: ScalarSpeed(750.),
        maximum_speed: ScalarSpeed(1500.),
        is_aircraft: true,
        is_fighter: true,
        human_controlled: false,
        valid: true,
        type_allowed: true,
        seeker_eligible: true,
        wing_attackers: 0,
        terrain_blocked: false,
        sensor_supported: true,
        link_track: false,
    }
}

fn station_view(
    station: u8,
    limit: Option<f64>,
    range: (f64, Option<f64>),
    mount: [f64; 3],
) -> StationView {
    StationView {
        station: StationId(station),
        guided: true,
        capability: StoreCapability::AIR_TO_AIR_MISSILE,
        inhibited: false,
        rounds: Rounds::Finite(4),
        pointing_error_deg: 0.,
        employment_limit_deg: limit,
        employment_fit: None,
        minimum_range_ft: range.0,
        maximum_range_ft: range.1,
        energy_target: None,
        infrared_envelope: None,
        requires_radar: false,
        requires_sensor: false,
        employment_zone: None,
        mount,
        damage_vs_category: 100.,
        store_speed: ScalarSpeed(2400.),
        tracking_delay: Delay::seconds(1),
        pacing: pacing(1, Delay::seconds(0), Delay::seconds(2)),
    }
}

/// Six stores: radar and sensor requirements, a gun at station 2, a surface
/// store, an inhibited one and an empty one.
fn stations() -> Vec<(StationView, u32)> {
    let mut radar = station_view(0, Some(40.), (1_500., Some(40_000.)), [4., -2., 0.]);
    radar.requires_radar = true;
    let mut infrared = station_view(1, Some(60.), (1_000., Some(15_000.)), [-4., -2., 0.]);
    infrared.requires_sensor = true;
    infrared.tracking_delay = Delay::quarters(2);
    let mut gun = station_view(2, Some(5.), (0., Some(6_000.)), [0., 0., 10.]);
    gun.guided = false;
    gun.capability = StoreCapability::GUN;
    gun.damage_vs_category = 10.;
    gun.store_speed = ScalarSpeed(3_000.);
    gun.tracking_delay = Delay::quarters(1);
    gun.pacing = pacing(10, Delay::quarters(1), Delay::quarters(2));
    let mut surface = station_view(3, Some(20.), (0., Some(30_000.)), [0.; 3]);
    surface.capability = StoreCapability::SURFACE_STORE;
    let mut inhibited = station_view(4, Some(60.), (0., Some(50_000.)), [0.; 3]);
    inhibited.inhibited = true;
    let mut empty = station_view(5, Some(60.), (0., Some(50_000.)), [0.; 3]);
    empty.rounds = Rounds::Finite(0);
    vec![
        (radar, 4),
        (infrared, 2),
        (gun, 400),
        (surface, 2),
        (inhibited, 4),
        (empty, 0),
    ]
}

/// Two circling and crossing hostiles, one that appears and is later
/// destroyed, a surface object and a friendly.
fn scripted_targets(tick: u64) -> Vec<TargetView> {
    let t = tick as f64 / 120.;
    let mut circling = target_view(
        11,
        2,
        [
            6_000. * (t * 0.05).sin(),
            20_000. + 1_500. * (t * 0.1).sin(),
            12_000. + 6_000. * (t * 0.05).cos(),
        ],
    );
    circling.heading_deg = wrap(90. - t * 0.05f64.to_degrees());
    let mut head_on = target_view(12, 2, [-4_000. + t * 150., 21_000., 24_000. - t * 700.]);
    head_on.heading_deg = 170.;
    head_on.sensor_supported = (tick / 400).is_multiple_of(2);
    let mut late = target_view(13, 2, [30_000. - t * 700., 19_000., 15_000.]);
    late.heading_deg = -90.;
    late.valid = tick < 1_800;
    let mut surface = target_view(14, 2, [5_000., 50., 30_000.]);
    surface.is_aircraft = false;
    surface.is_fighter = false;
    surface.speed = ScalarSpeed(0.);
    let friendly = target_view(5, 1, [3_000., 20_000., 2_000. + t * 700.]);
    let mut targets = vec![circling, head_on, surface, friendly];
    if tick >= 600 {
        targets.push(late);
    }
    targets
}

fn threat_report(missile: u32, seeker: SeekerClass, launcher: u32, tick: u64) -> ThreatReport {
    ThreatReport {
        missile_id: missile,
        seeker,
        launcher_id: launcher,
        launcher_same_side: false,
        distance_at_launch_ft: 18_000.,
        launch_tick: tick,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Leader,
    Wingman,
    Ace,
    /// A wingman with no hostiles that reslots and dodges same-tick traffic.
    Formation,
}

#[derive(Clone)]
struct Scripted {
    controller: Controller,
    ship: PointMass,
    stations: Vec<(StationView, u32)>,
    dispensers: Vec<DispenserStore>,
    events: Vec<(u64, FrameEvent)>,
    kind: Kind,
}

impl Scripted {
    #[allow(clippy::too_many_arguments)]
    fn new(
        kind: Kind,
        id: u32,
        wing: u8,
        member: u8,
        aircraft: AircraftId,
        level: Experience,
        seed: u64,
        position: [f64; 3],
        heading: f64,
    ) -> Self {
        let identity = ActorIdentity {
            actor: ActorId(id),
            side: Side(1),
            wing,
            member,
            leads: member == 0,
            aircraft,
            human_controlled: false,
        };
        let profile = BehaviorProfile {
            family: BehaviorFamily::FighterStrike,
            role: MissionRole::AirToAir,
        };
        Self {
            controller: Controller::new(identity, profile, resolved(level), seed).unwrap(),
            ship: PointMass {
                position,
                heading,
                pitch: 0.,
                bank: 0.,
                speed: 750.,
            },
            stations: stations(),
            dispensers: vec![
                DispenserStore {
                    class: SeekerClass::Infrared,
                    count: 6,
                },
                DispenserStore {
                    class: SeekerClass::Radar,
                    count: 6,
                },
            ],
            events: Vec::new(),
            kind,
        }
    }

    fn orders(&mut self, tick: u64) {
        let heading = self.ship.heading;
        let controller = &mut self.controller;
        let order = match tick {
            240 => Some(WingRequest::WingControl(WingControl::Tight)),
            480 => Some(WingRequest::Spacing {
                axis: SpacingAxis::Horizontal,
                feet: 3_000,
            }),
            720 => Some(WingRequest::Break {
                heading_offset_deg: 90,
                pitch_deg: 10,
            }),
            960 => Some(WingRequest::TargetAssignment(TargetOrder::ConcreteTarget(
                TargetId(12),
            ))),
            1_200 => Some(WingRequest::Approach {
                heading_deg: 45,
                pitch_deg: 5,
                speed: ScalarSpeed(0.),
            }),
            2_700 => Some(WingRequest::FormationSelection(Formation::LineAstern)),
            2_900 => Some(WingRequest::TargetAssignment(TargetOrder::HoldFire)),
            2_950 => Some(WingRequest::TargetAssignment(TargetOrder::FreeSelection)),
            _ => None,
        };
        if let Some(order) = order {
            controller.prepare_order(heading, limits());
            let outcome = controller.receive_order(order, tick);
            assert!(
                !matches!(outcome, Ok(ReceiverOutcome::Rejected(_))),
                "{outcome:?}"
            );
        }
        match tick {
            1_300 => controller.track_ordered_approach(11, 30., 5.),
            1_440 => controller.set_defense_motion(Some(DefenseMotion {
                source: DefenseSource::Missile,
                heading_deg: 200.,
                pitch_deg: -10.,
                rotorcraft: false,
                extend: false,
            })),
            1_560 => controller.set_defense_motion(None),
            1_680 => controller.set_search_contact(Some(SearchContact {
                id: 13,
                position: [9_000., 19_000., 15_000.],
                observed_tick: 1_600,
            })),
            1_800 => controller.set_search_contact(None),
            1_900 => controller.set_mission_target(Some(11)),
            2_000 => controller.set_mission_rejoin(Some([0., 20_000., 30_000.])),
            2_200 => controller.set_mission_rejoin(None),
            2_300 => controller.set_mission_target(None),
            2_400 => controller.set_mission_search_bearing(Some(135.)),
            2_600 => controller.set_mission_search_bearing(None),
            2_760 => controller.return_to_formation(),
            2_800 => controller.set_experience(resolved(Experience::Novice)),
            2_850 => controller.set_mission_hold_fire(true),
            2_875 => controller.set_mission_hold_fire(false),
            _ => {}
        }
    }

    /// One tick, the way the mission host drives a controller: the scratch
    /// fields are written first, then the step.
    fn advance(&mut self, tick: u64, leader: Option<PointMass>) -> IntentBatch {
        let hostile = self.kind != Kind::Formation;
        let targets = if hostile {
            scripted_targets(tick)
        } else {
            Vec::new()
        };
        if self.kind == Kind::Ace {
            self.orders(tick);
        }
        let events: Vec<FrameEvent> = self
            .events
            .iter()
            .filter(|(at, _)| *at == tick)
            .map(|(_, event)| *event)
            .collect();
        let own = self.ship.own(0., true, false);
        let aim = self
            .controller
            .target()
            .and_then(|id| targets.iter().find(|t| t.id == id))
            .or_else(|| {
                targets.iter().min_by(|a, b| {
                    distance(own.position, a.position)
                        .total_cmp(&distance(own.position, b.position))
                })
            });
        let views: Vec<StationView> = self
            .stations
            .iter()
            .map(|(station, rounds)| {
                let mut view = *station;
                if !matches!(view.rounds, Rounds::Finite(0)) {
                    view.rounds = Rounds::Finite(*rounds);
                }
                view.employment_fit = aim.and_then(|t| view.employment_error(&own, t));
                view.pointing_error_deg = view.employment_fit.unwrap_or(180.);
                view
            })
            .collect();
        let gun_rounds = self.stations[2].1;
        let gun_views: Vec<View> = aim
            .map(|target| {
                let range = distance(own.position, target.position);
                View {
                    station: StationId(2),
                    target: Some(target.id),
                    solution: (range < 12_000.).then_some(Solution {
                        aim: Aim {
                            direction: [0., 0., 1.],
                            heading_rate: 0.002,
                            pitch_rate: -0.001,
                        },
                        aligned: tick % 240 < 120,
                        miss_ft: 20.,
                        seconds: 1.5,
                        range_ft: range,
                    }),
                    rounds: 10,
                    period_ticks: 30,
                    target_speed: 750.,
                    ammunition: Rounds::Finite(gun_rounds),
                }
            })
            .into_iter()
            .collect();
        self.controller.set_gun_views(gun_views);
        let leader_view = leader.map(|ship| LeaderView {
            position: ship.position,
            velocity: Basis::new(ship.heading.to_radians(), ship.pitch.to_radians(), 0.)
                .forward
                .map(|axis| axis * ship.speed),
            heading_deg: ship.heading,
            speed: ScalarSpeed(ship.speed),
            target: None,
            recovering: false,
            on_ground: false,
        });
        if let Some(leader) = leader {
            // Same-tick traffic: the leader and, for a while, a second
            // aircraft repositioning across the wingman's path.
            let near = (255..700).contains(&tick);
            let mut traffic = vec![Traffic {
                id: 1,
                position: leader.position,
                velocity: leader_view.map_or([0.; 3], |l| l.velocity),
                phase: None,
                planned_velocity: None,
            }];
            if near {
                // Another repositioning aircraft whose plan crosses ours: its
                // physical track is clear, so only the plan makes us yield.
                let velocity = leader_view.map_or([0.; 3], |l| l.velocity);
                traffic.push(Traffic {
                    id: 2,
                    position: [
                        self.ship.position[0] + 900.,
                        self.ship.position[1],
                        self.ship.position[2],
                    ],
                    velocity,
                    phase: Some(crate::ai::formation::Phase::Reposition),
                    planned_velocity: Some([velocity[0] - 100., velocity[1], velocity[2]]),
                });
            }
            self.controller.set_formation_observation(traffic);
        }
        let spacing = if self.kind == Kind::Formation {
            match tick {
                0..250 => 2_048,
                250..900 => 4_096,
                _ => 1_024,
            }
        } else {
            2_048
        };
        let dispensers = self.dispensers.clone();
        let frame = DecisionFrame {
            tick,
            own: self.ship.own(
                3_000. - tick as f64 * 0.9,
                !(1_200..1_500).contains(&tick),
                events.iter().any(|event| matches!(event, FrameEvent::Hit)) || tick > 2_100,
            ),
            targets: &targets,
            events: &events,
            stations: &views,
            dispensers: &dispensers,
            wing: WingView {
                control: WingControl::Loose,
                formation: Formation::Echelon,
                horizontal_spacing_ft: spacing,
                vertical_spacing_ft: 512,
                slot: if self.kind == Kind::Leader { 1 } else { 2 },
                leader: leader_view,
                wingmen_in_formation: u32::from(self.kind == Kind::Leader),
                wing_combat: !targets.is_empty(),
                wing_approach: false,
                wing_approach_value_ft: None,
            },
            route: RouteView {
                home_airport: Some(Position { x: 0., z: 0. }),
                leader_is_ai: true,
            },
            now: TimeOfDay(tick),
            flight_state: FlightState::Free,
        };
        let batch = self.controller.step(&frame).unwrap();
        self.controller.terrain_floor(&frame).unwrap();
        for weapon in &batch.weapons {
            if let Some((_, rounds)) = self
                .stations
                .iter_mut()
                .find(|(station, _)| station.station == weapon.request.station)
            {
                *rounds = rounds.saturating_sub(1);
            }
        }
        if let Some(devices) = batch.devices
            && let Some(dispenser) = self
                .dispensers
                .iter_mut()
                .find(|dispenser| dispenser.class == devices.class)
        {
            dispenser.count = dispenser.count.saturating_sub(u32::from(devices.count));
        }
        self.ship.follow(batch.motion.as_ref());
        batch
    }
}

#[derive(Clone)]
struct Scene {
    crew: Vec<Scripted>,
    /// What a `Kind::Formation` wingman follows.
    lead: PointMass,
}

impl Scene {
    fn step(&mut self, tick: u64) -> Vec<IntentBatch> {
        let leader_ship = self.crew[0].ship;
        let lead = self.lead;
        let batches = self
            .crew
            .iter_mut()
            .enumerate()
            .map(|(index, scripted)| match scripted.kind {
                Kind::Wingman => scripted.advance(tick, Some(leader_ship)),
                Kind::Formation => scripted.advance(tick, Some(lead)),
                Kind::Leader | Kind::Ace => {
                    let _ = index;
                    scripted.advance(tick, None)
                }
            })
            .collect();
        self.lead.follow(None);
        batches
    }

    fn controllers(&self) -> Vec<Controller> {
        self.crew.iter().map(|s| s.controller.clone()).collect()
    }
}

/// The controller as a restore leaves it: per-tick scratch is written by the
/// next step, so a restored controller starts with none.
fn without_scratch(controller: &Controller) -> Controller {
    let mut controller = controller.clone();
    controller.set_gun_views(Vec::new());
    controller.set_formation_observation(Vec::new());
    controller
}

fn models() -> Models {
    Models::default()
}

/// Codes a controller, decodes a copy (which must code to the same bytes) and
/// requires it equal to the original but for the scratch.
fn check(controller: &Controller) -> Controller {
    let copy = round_trip(controller, &models()).unwrap();
    assert_eq!(copy, without_scratch(controller));
    copy
}

/// Restores `restored` into a copy of `scene` at `tick` and steps both on:
/// every batch and, at the end, every controller must be equal.
fn steps_on_identically(scene: &Scene, tick: u64, ticks: u64, restored: Vec<Controller>) {
    let mut original = scene.clone();
    let mut twin = scene.clone();
    for (scripted, controller) in twin.crew.iter_mut().zip(restored) {
        scripted.controller = controller;
    }
    for at in tick..tick + ticks {
        let left = original.step(at);
        let right = twin.step(at);
        assert_eq!(left, right, "the batches differ at tick {at}");
    }
    for (left, right) in original.crew.iter().zip(&twin.crew) {
        assert_eq!(left.controller, right.controller);
    }
}

fn crew_scene() -> Scene {
    let mut leader = Scripted::new(
        Kind::Leader,
        1,
        0,
        0,
        AircraftId::F18,
        Experience::Experienced,
        0x51,
        [0., 20_000., 0.],
        0.,
    );
    leader.events = vec![
        (
            900,
            FrameEvent::ThreatReported(threat_report(71, SeekerClass::Radar, 12, 890)),
        ),
        (1_100, FrameEvent::Hit),
        (1_800, FrameEvent::TargetUnavailable(13)),
    ];
    let mut wingman = Scripted::new(
        Kind::Wingman,
        2,
        0,
        1,
        AircraftId::Mig29,
        Experience::Novice,
        0x52,
        [-2_000., 19_500., -2_500.],
        0.,
    );
    wingman.events = vec![
        (
            1_500,
            FrameEvent::ThreatReported(threat_report(72, SeekerClass::Infrared, 11, 1_490)),
        ),
        (2_100, FrameEvent::Hit),
        (1_810, FrameEvent::ActorRemoved(13)),
    ];
    let mut ace = Scripted::new(
        Kind::Ace,
        3,
        1,
        0,
        AircraftId::Su27,
        Experience::Ace,
        0x53,
        [8_000., 18_000., -6_000.],
        20.,
    );
    ace.events = vec![
        (
            600,
            FrameEvent::ThreatReported(threat_report(73, SeekerClass::Radar, 13, 600)),
        ),
        (
            1_000,
            FrameEvent::ThreatReported(threat_report(74, SeekerClass::Infrared, 11, 995)),
        ),
    ];
    Scene {
        crew: vec![leader, wingman, ace],
        lead: PointMass {
            position: [0., 20_000., 0.],
            heading: 0.,
            pitch: 0.,
            bank: 0.,
            speed: 750.,
        },
    }
}

fn formation_scene() -> Scene {
    let wingman = Scripted::new(
        Kind::Formation,
        4,
        0,
        1,
        AircraftId::F18,
        Experience::Average,
        0x54,
        [-1_500., 20_000., -1_800.],
        0.,
    );
    // Slot 0 is only read through `crew[0]` by the other kinds.
    Scene {
        crew: vec![wingman],
        lead: PointMass {
            position: [0., 20_000., 0.],
            heading: 0.,
            pitch: 0.,
            bank: 0.,
            speed: 750.,
        },
    }
}

/// What the scenes covered, read from the controllers' debug text (the
/// guidance and the weapon service keep private state).
#[derive(Default)]
struct Seen {
    text: String,
}

impl Seen {
    fn add(&mut self, controller: &Controller) {
        self.text.push_str(&format!("{controller:?}\n"));
    }
    fn require(&self, needles: &[&str]) {
        for needle in needles {
            assert!(
                self.text.contains(needle),
                "no snapshot showed `{needle}`: the scenes no longer cover it"
            );
        }
    }
}

#[test]
fn a_fresh_controller_round_trips() {
    let scene = crew_scene();
    for controller in scene.controllers() {
        check(&controller);
    }
    let mut seen = Seen::default();
    seen.add(&scene.crew[0].controller);
}

#[test]
fn the_scripted_crew_round_trips_and_steps_on_identically() {
    let mut scene = crew_scene();
    let mut seen = Seen::default();
    // The three ticks the slice is accepted at; every controller of the
    // scene steps on identically from a restored copy.
    let twins = [600u64, 1_500, 2_800];
    let mut notable = false;
    for tick in 0..3_000u64 {
        if tick % 60 == 0 || twins.contains(&tick) || notable {
            let restored: Vec<Controller> = scene
                .controllers()
                .iter()
                .map(|controller| {
                    seen.add(controller);
                    check(controller)
                })
                .collect();
            if twins.contains(&tick) {
                steps_on_identically(&scene, tick, 240, restored);
            }
        }
        // Also look at the tick after a release, an order or a countermeasure
        // request, when the last batch holds one.
        notable = scene
            .step(tick)
            .iter()
            .any(|b| !b.weapons.is_empty() || b.devices.is_some() || !b.wing.is_empty());
    }
    seen.require(&[
        "gun_cycles: {2: Cycle",
        "active: Some(ActiveManeuver",
        "pending_warnings: [(",
        "search_contact: Some(",
        "defense_motion: Some(",
        "mission_target: Some(",
        "mission_rejoin: Some(",
        "mission_search_bearing: Some(",
        "ordered_approach: Some(",
        "target_order: Some(",
        "gun_tracking: Some(View",
        "weapons: [WeaponIntent",
        "devices: Some(DeviceIntent",
        "wing: [",
        "reason: Some(",
        "Prepare {",
        "Tracking {",
        "Reload",
    ]);
}

#[test]
fn a_wingman_reslotting_around_traffic_round_trips_and_steps_on_identically() {
    let mut scene = formation_scene();
    let mut seen = Seen::default();
    let mut twinned = 0;
    let mut reposition_twin = false;
    for tick in 0..1_600u64 {
        let reposition = format!("{:?}", scene.crew[0].controller).contains("reposition: Some(");
        if tick % 30 == 0 || (reposition && !reposition_twin) {
            let restored: Vec<Controller> = scene
                .controllers()
                .iter()
                .map(|controller| {
                    seen.add(controller);
                    check(controller)
                })
                .collect();
            if tick == 120 || tick == 1_290 || (reposition && !reposition_twin) {
                steps_on_identically(&scene, tick, 300, restored);
                twinned += 1;
                reposition_twin |= reposition;
            }
        }
        scene.step(tick);
    }
    assert!(twinned >= 3, "{twinned}");
    assert!(reposition_twin, "the scene never entered a reposition");
    seen.require(&[
        "reposition: Some(Reposition",
        "trace: Some(Trace",
        "yielding_to: Some(",
        "formation_configuration: Some(",
    ]);
}

#[test]
fn a_controller_rebuilt_from_its_coding_codes_the_same_bytes() {
    let mut scene = crew_scene();
    for tick in 0..1_500u64 {
        scene.step(tick);
    }
    for controller in scene.controllers() {
        let first = to_bytes(&controller, &models()).unwrap();
        let copy: Controller = from_bytes(&first, &models()).unwrap();
        assert_eq!(to_bytes(&copy, &models()).unwrap(), first);
    }
}

#[test]
fn scratch_and_trace_do_not_change_the_coding() {
    let mut scene = crew_scene();
    for tick in 0..900u64 {
        scene.step(tick);
    }
    for scripted in &scene.crew {
        let with = to_bytes(&scripted.controller, &models()).unwrap();
        let without = to_bytes(&without_scratch(&scripted.controller), &models()).unwrap();
        assert_eq!(with, without);
    }
}

#[test]
fn damaged_coding_is_refused_or_read_without_a_panic() {
    let mut scene = crew_scene();
    for tick in 0..1_500u64 {
        scene.step(tick);
    }
    let coded = to_bytes(&scene.crew[2].controller, &models()).unwrap();
    // Every truncation.
    for length in 0..coded.body.len() {
        let cut = Coded {
            body: coded.body[..length].to_vec(),
            records: coded.records.clone(),
        };
        assert!(
            from_bytes::<Controller>(&cut, &models()).is_err(),
            "a controller read from {length} of {} bytes",
            coded.body.len()
        );
    }
    // Flipped bits and random bytes: any answer but a panic.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..3_000 {
        let mut damaged = coded.clone();
        let flips = 1 + next() % 4;
        for _ in 0..flips {
            let at = (next() % damaged.body.len() as u64) as usize;
            damaged.body[at] ^= 1 << (next() % 8);
        }
        let _ = from_bytes::<Controller>(&damaged, &models());
    }
    for _ in 0..1_000 {
        let length = (next() % 4_096) as usize;
        let random = Coded {
            body: (0..length).map(|_| next() as u8).collect(),
            records: Vec::new(),
        };
        let _ = from_bytes::<Controller>(&random, &models());
    }
}

#[test]
fn the_small_controller_types_round_trip() {
    use super::super::{Completion, FallbackLog};
    use crate::ai::fitted::Fallback;
    use crate::ai::motion::{CompletionAxis, Deadline};
    let models = models();
    let same = |value: Completion| {
        assert_eq!(round_trip(&value, &models).unwrap(), value);
    };
    same(Completion::Deadline(Deadline(77)));
    same(Completion::Axis(CompletionAxis::Bank));
    let mut log = FallbackLog::default();
    for (n, fallback) in Fallback::ALL.into_iter().enumerate() {
        assert_eq!(round_trip(&fallback, &models).unwrap(), fallback);
        for _ in 0..=n {
            log.record(fallback);
        }
    }
    let copy = round_trip(&log, &models).unwrap();
    assert_eq!(copy, log);
    assert_eq!(copy.total(), 55);
}
