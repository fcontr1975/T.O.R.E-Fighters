//! The host's answers on a striker's guided shots from farther out (John,
//! 2026-10-29): the energy model's exact-point answer under the energy
//! rules, the zone or the sight's bar under the others, cached by store,
//! band and range step. A synthetic store; no retail data.
use super::*;
use crate::test_support::surface::{spec_with_target, surface_resources, target};
use crate::world::{Seating, World};
use tore_sim::ai::strike::reach;

/// A guided store with a motor (a laser missile), 12,000 ft of zone, off
/// a synthetic striker's second station.
fn missile() -> Weapon {
    let mut spec = spec_with_target(&target("QUCITY", 0, 0, 9));
    spec.wings[0].count = 2;
    let world = World::new(&spec, &surface_resources(), Seating::SinglePlayer).unwrap();
    let wings = world.ai_wings.as_ref().unwrap();
    let config = wings.configs.values().next().unwrap();
    let mut w = config.stations[1].weapon.clone();
    w.source = "SYNTH_AGM.JT".into();
    w.seeker.signature = 1;
    w.flags |= 0x400 | 0x20000;
    w.flags &= !0x10;
    w.movement.ignite_t = 0;
    w.movement.fuel_t = 40;
    w.seeker.zones[1].minimum_range = 1_000;
    w.seeker.zones[1].maximum_range = 12_000;
    w.movement.maximum_speed = 1_200;
    w
}

fn table(w: &Weapon, kind: StoreKind, rules: missiles::Rules) -> (Arc<ShotCache>, StrikerShots) {
    let cache = Arc::new(ShotCache::default());
    let shots = StrikerShots::new(&cache, rules, std::iter::once((StationId(1), kind, w))).unwrap();
    (cache, shots)
}

/// A launch on the bands' own values (750 ft is the third 250 ft band).
const LAUNCH: ShotLaunch = ShotLaunch {
    target_ft: 0.,
    height_ft: 750.,
    speed_fps: 200.,
};

/// Under the energy rules the answer is the energy model's exact-point
/// answer for that launch, the question the AI's release gate asks; it is
/// cached by store, band and range step, so the same band and step asked
/// again is the same answer at no cost, and a nearby launch in the same band
/// shares it.
#[test]
fn an_energy_answer_is_the_exact_point_answer_cached_by_band_and_step() {
    let w = missile();
    let (cache, shots) = table(&w, StoreKind::Laser, missiles::Rules::Energy);
    let mut hits = 0;
    for step in 1..=24 {
        let range = f64::from(step) * reach::SHOT_STEP_FT;
        let answer = shots.shot(StationId(1), LAUNCH, range);
        let launcher = live::Launcher {
            radar_power: true,
            position: [0., 750., 0.],
            basis: tore_sim::attitude::Basis::new(0., 0., 0.),
            speed_fps: 200.,
            velocity: [0., 0., 200.],
            bay_ready: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: Default::default(),
        };
        let observed = seeker::Observation {
            id: u32::MAX,
            position: [0., 0., range],
            velocity: [0.; 3],
            quality: 1.,
            range: range.hypot(750.),
            off_axis: 0.,
        };
        let inside = range.hypot(750.) >= 1_000. && range.hypot(750.) <= 12_000.;
        let exact = live::State::energy_trial(&w, launcher, observed, [0.; 3], &|_, _| -8.)
            .filter(|a| a.hit && inside)
            .map(|a| a.seconds);
        assert_eq!(answer, exact, "{range}");
        hits += usize::from(answer.is_some());
    }
    assert!(hits > 0, "the synthetic missile hits somewhere in its zone");
    let asked = cache.len();
    assert_eq!(asked, 24);
    // The same step again, and a launch in the same bands: no new answer.
    let near = ShotLaunch {
        target_ft: 40.,
        height_ft: 690.,
        speed_fps: 210.,
    };
    for step in 1..=24 {
        let range = f64::from(step) * reach::SHOT_STEP_FT;
        assert_eq!(
            shots.shot(StationId(1), near, range),
            shots.shot(StationId(1), LAUNCH, range)
        );
    }
    assert_eq!(cache.len(), asked);
    // Another station is not the store's.
    assert_eq!(shots.shot(StationId(2), LAUNCH, 5_000.), None);
}

/// Under the other rules a seeker missile reaches its zone 1, and a guided
/// missile on the sight no farther than its bar, where the AI's gate lets
/// it go.
#[test]
fn under_the_other_rules_the_zone_or_the_bar_bounds_the_answer() {
    let w = missile();
    let (_, seeker) = table(&w, StoreKind::Seeker, missiles::Rules::Spec);
    assert_eq!(
        seeker.shot(StationId(1), LAUNCH, 500.),
        None,
        "inside its minimum"
    );
    let at = |range: f64| range.hypot(750.) / 1_200.;
    assert_eq!(seeker.shot(StationId(1), LAUNCH, 6_000.), Some(at(6_000.)));
    assert_eq!(
        seeker.shot(StationId(1), LAUNCH, 11_500.),
        Some(at(11_500.))
    );
    assert_eq!(
        seeker.shot(StationId(1), LAUNCH, 12_000.),
        None,
        "past its zone"
    );
    let (_, sight) = table(&w, StoreKind::Laser, missiles::Rules::Spec);
    let bar = 12_000. - guided::LEVEL_DEPTH * 11_000.;
    assert!(sight.shot(StationId(1), LAUNCH, bar - 100.).is_some());
    assert_eq!(sight.shot(StationId(1), LAUNCH, bar + 500.), None);
}

/// Unguided stores have no table: their passes have no stand-off.
#[test]
fn unguided_stores_have_no_table() {
    let w = missile();
    let cache = Arc::new(ShotCache::default());
    for kind in [StoreKind::Bomb, StoreKind::Rocket, StoreKind::Gun] {
        assert!(
            StrikerShots::new(
                &cache,
                missiles::Rules::Energy,
                std::iter::once((StationId(1), kind, &w))
            )
            .is_none()
        );
    }
}
