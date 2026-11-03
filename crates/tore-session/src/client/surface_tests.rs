//! A ground target online (protocol 22, slice N1): the host draws the
//! surface seed, every client builds the same surface and checks its digest
//! at the seat, moving units arrive as the fifth entity kind, and the
//! units' hit points, radars, rails and spares as Surface unit events, to
//! a late joiner too. Synthetic data only.

use super::tests::{Rig, level_script};
use super::*;
use tore_net::sim::LinkConfig;
use tore_sim::combat::missiles::TargetRole;
use tore_world::mission::Start;
use tore_world::surface::{SURFACE_UNIT_BASE, UnitId};
use tore_world::test_support::surface::{routed_resources, spec_with_target, target};

const TANK: u32 = SURFACE_UNIT_BASE;
const BOAT: u32 = SURFACE_UNIT_BASE + 1;
const STANDING: u32 = SURFACE_UNIT_BASE + 2;

/// The routed column of the synthetic import (`~QUCOL`: a tank on a route,
/// a boat, a tank that stands), with the seed left for the host to draw.
fn column() -> MissionSpec {
    let mut spec = spec_with_target(&target("QUCOL", 0, 0, 0));
    spec.wings[0].count = 2;
    spec.start = Start::Airborne {
        altitude_ft: 10_000,
    };
    spec
}

fn link() -> LinkConfig {
    LinkConfig::for_round_trip(Duration::from_millis(40), 0.1, 0., 0.)
}

fn hp(world: &World, id: u32) -> i32 {
    world
        .combat
        .state
        .targets
        .iter()
        .find(|t| t.id == id && t.role == TargetRole::Surface)
        .map(|t| t.hp)
        .expect("a surface row")
}

fn set_hp(rig: &mut Rig, id: u32, hp: i32) {
    let world = rig.host.world_for_test();
    let row = world
        .combat
        .state
        .targets
        .iter_mut()
        .find(|t| t.id == id)
        .expect("a surface row");
    row.hp = hp;
}

#[test]
fn a_ground_target_flies_online_with_one_surface_on_every_machine() {
    let mut rig = Rig::with_import(column(), link(), 21, routed_resources(), |_| {});
    // The host drew the seed before it sent the text (Q1 hook).
    let seed = rig.host.spec().surface_seed;
    assert_ne!(seed, 0, "the host draws a surface seed");
    assert!(
        rig.host
            .mission_text()
            .contains(&format!("surface-seed {seed}"))
    );
    rig.host.start_now();
    let player = rig.join(|_| {}, level_script());
    assert!(rig.run_until(Duration::from_secs(4), |r| r.seated(player)));
    let host_digest = rig.host.world().terrain.surface.digest();
    let ours = rig.players[player]
        .client
        .mission()
        .expect("loaded")
        .terrain
        .surface
        .digest();
    assert_eq!(ours, host_digest, "the client built the host's surface");
    assert!(
        !rig.players[player]
            .events
            .iter()
            .any(|e| matches!(e, ClientEvent::ContentRefused { .. })),
        "{:?}",
        rig.players[player].events
    );

    // The column moves: the client draws the host's Mover.
    rig.run(Duration::from_secs(4));
    let now = rig.net.now();
    let frame = rig.players[player].client.frame(now).expect("flying");
    let host_tank = rig
        .host
        .world()
        .combat
        .surface
        .unit(UnitId(TANK))
        .and_then(|u| u.mover)
        .expect("the tank moves on the host");
    let drawn = frame
        .picture
        .surface
        .iter()
        .find(|p| p.id.0 == TANK)
        .expect("the tank reaches the client");
    let moved = host_tank.position();
    let gap = (0..3)
        .map(|i| (drawn.position[i] - moved[i]).powi(2))
        .sum::<f64>()
        .sqrt();
    // About 100 ms behind the host at up to 50 ft/s.
    assert!(gap < 25., "drawn {:?} host {:?}", drawn.position, moved);
    assert!(frame.picture.surface.iter().any(|p| p.id.0 == BOAT));
    assert!(
        !frame.picture.surface.iter().any(|p| p.id.0 == STANDING),
        "a unit with no route is the mission's, never sent"
    );
    // The ground row follows it too, for the views and the target window.
    let row = frame.picture.targets.iter().find(|t| t.id == TANK).unwrap();
    assert!((row.position[2] - drawn.position[2]).abs() < 1e-9);

    // The standing tank is hurt and the boat sunk on the host.
    let initial = hp(rig.host.world(), STANDING);
    set_hp(&mut rig, STANDING, initial / 2);
    set_hp(&mut rig, BOAT, 0);
    rig.run(Duration::from_secs(2));
    let now = rig.net.now();
    let frame = rig.players[player].client.frame(now).expect("flying");
    let view = frame
        .surface_units
        .get(&STANDING)
        .expect("the hurt tank is told");
    assert_eq!(view.hp, initial / 2);
    assert!(
        frame
            .picture
            .surface
            .iter()
            .any(|p| p.id.0 == BOAT && p.wrecked),
        "the boat's wreck stands where it stopped"
    );
    let standing = frame
        .picture
        .targets
        .iter()
        .find(|t| t.id == STANDING)
        .unwrap();
    assert_eq!(standing.damage.hp, initial / 2);

    // A late joiner is told the same at its seat.
    let late = rig.join(|c| c.callsign = "Hawk".into(), level_script());
    assert!(rig.run_until(Duration::from_secs(4), |r| r.seated(late)));
    rig.run(Duration::from_secs(1));
    let now = rig.net.now();
    let frame = rig.players[late].client.frame(now).expect("flying");
    assert_eq!(
        frame.surface_units.get(&STANDING).map(|v| v.hp),
        Some(initial / 2)
    );
    assert!(frame.picture.surface.iter().any(|p| p.id.0 == TANK));
}

#[test]
fn a_game_that_places_the_ground_target_otherwise_refuses_its_seat() {
    let mut rig = Rig::with_import(column(), link(), 22, routed_resources(), |_| {});
    rig.host.start_now();
    let player = rig.join(|_| {}, level_script());
    // This game's build places Red's start a foot away from the host's.
    rig.players[player]
        .client
        .set_mission_builder(Box::new(|spec, resources| {
            let mut world = World::new(spec, resources, Seating::Open)?;
            if let Some(starts) = &mut world.terrain.surface.starts {
                starts.red[0] += 1;
            }
            Ok(world)
        }));
    rig.run(Duration::from_secs(4));
    assert!(!rig.seated(player), "the seat is refused");
    let reason = rig.players[player]
        .events
        .iter()
        .find_map(|e| match e {
            ClientEvent::ContentRefused { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("the game refuses");
    assert!(reason.contains("ground target"), "{reason}");
    assert!(
        rig.logs
            .iter()
            .any(|log| matches!(log, crate::host::HostLog::ContentRefused { .. })),
        "the host hears of it"
    );
}

/// PvP with a ground target (L1 hook): a Redfor human takes an enemy plane
/// of the group the layout aims at Red's start, within 5 nm of the target,
/// so Redfor starts by the target it defends.
#[test]
fn a_redfor_player_starts_by_the_target_it_defends() {
    let mut spec = column();
    spec.wings[3].count = 2;
    spec.separation_nm = 20;
    let mut rig = Rig::with_import(spec, link(), 23, routed_resources(), |config| {
        config.open_planes = crate::host::OpenPlanes::All;
    });
    rig.host.start_now();
    let red = rig.join(|c| c.plane = Some(2), level_script());
    assert!(rig.run_until(Duration::from_secs(4), |r| r.seated(red)));
    let starts = rig
        .host
        .world()
        .terrain
        .surface
        .starts
        .clone()
        .expect("a ground target's starts");
    let flight = &rig.players[red]
        .client
        .prediction()
        .expect("flying")
        .plane()
        .flight;
    let from_red = (flight.position[0] - f64::from(starts.red[0]))
        .hypot(flight.position[2] - f64::from(starts.red[1]));
    // The enemy group's members fly within a mile or two of its point.
    assert!(from_red < 3. * 6_076., "{from_red} ft from Red's start");
    let from_blue = (flight.position[0] - f64::from(starts.blue[0]))
        .hypot(flight.position[2] - f64::from(starts.blue[1]));
    assert!(from_blue > 15. * 6_076., "{from_blue} ft from Blue's start");
}

#[test]
fn clients_build_the_retried_host_seed_without_retrying_locally() {
    use tore_world::test_support::surface::placement_retry_resources;
    let (resources, chosen) = placement_retry_resources(1);
    let mission = spec_with_target(&target("QUCITY", 0, 3, 1));
    let mut rig = Rig::with_import(mission, link(), 21, resources, |_| {});
    assert_eq!(rig.host.spec().surface_seed, chosen);
    rig.host.start_now();
    let player = rig.join(|_| {}, level_script());
    assert!(rig.run_until(Duration::from_secs(4), |r| r.seated(player)));
    assert_eq!(
        rig.players[player]
            .client
            .mission()
            .unwrap()
            .terrain
            .surface
            .digest(),
        rig.host.world().terrain.surface.digest()
    );
    assert!(
        !rig.players[player]
            .events
            .iter()
            .any(|e| matches!(e, ClientEvent::ContentRefused { .. }))
    );
}
