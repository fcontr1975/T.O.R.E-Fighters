//! Placement retries at every host mission boundary, using synthetic resources.
use super::*;
use tore_world::test_support::surface::{placement_retry_resources, spec_with_target, target};

fn config() -> HostConfig {
    HostConfig {
        entropy: tore_net::Entropy::Seeded(19),
        ..HostConfig::new(BuildId {
            compatibility: 0,
            version: "seed-test".into(),
            commit: "synthetic".into(),
            release: false,
        })
    }
}

fn assert_chosen(host: &Host, seed: u32) {
    assert_eq!(host.spec.surface_seed, seed);
    assert!(host.spec_text.contains(&format!("surface-seed {seed}")));
    let reads = ResourceReads::new(&host.resources);
    let direct = World::new(&host.spec, &reads, Seating::Open).unwrap();
    assert_eq!(
        host.world.terrain.surface.digest(),
        direct.terrain.surface.digest()
    );
    assert_eq!(
        host.manifest,
        reads.manifest(),
        "failed attempts must not pollute the accepted manifest"
    );
    assert!(host.logs.iter().any(|log| matches!(log, HostLog::Note { text, .. } if text.contains(&format!("Surface placement seed {seed} selected")))));
}

#[test]
fn a_drawn_seed_that_fails_is_retried_before_the_host_publishes_its_mission() {
    let requested = spec_with_target(&target("QUCITY", 0, 3, 0));
    let config = config();
    let staged = king::with_settings(
        requested.clone(),
        &crate::settings::Store::from_config(&config),
    );
    let initial = with_surface_seed(staged, session_id(config.entropy), 1);
    let (resources, chosen) = placement_retry_resources(initial.surface_seed);
    let error = World::new(&initial, &resources, Seating::Open)
        .err()
        .expect("fixture must refuse its first seed");
    assert!(
        error
            .to_string()
            .starts_with("Vehicle placement unavailable:"),
        "{error}"
    );
    let host = Host::new(requested, Arc::new(resources), config).unwrap();
    assert_chosen(&host, chosen);
}

struct OneRound(Option<MissionSpec>);
impl sequence::MissionSequence for OneRound {
    fn next(&mut self) -> Option<Result<sequence::Round, String>> {
        self.0.take().map(|spec| {
            Ok(sequence::Round {
                spec,
                settings: vec![],
                time_limit: None,
            })
        })
    }
    fn attempts(&self) -> usize {
        1
    }
    fn notice(&mut self) -> Option<String> {
        None
    }
    fn loaded(&mut self) {}
    fn failed(&mut self, why: &str) {
        panic!("rotation should recover with another seed: {why}");
    }
}

#[test]
fn king_changes_restart_and_rotation_keep_the_successful_seed() {
    let (resources, chosen) = placement_retry_resources(1);
    let bad = spec_with_target(&target("QUCITY", 0, 3, 1));
    let mut valid = bad.clone();
    valid.surface_seed = chosen;
    let mut host = Host::new(valid, Arc::new(resources), config()).unwrap();
    host.change_mission(ConnectionId(0), &bad.to_text())
        .unwrap();
    assert_chosen(&host, chosen);
    let number = host.number;
    host.spec.surface_seed = 1; // Exercise a restart whose requested layout is refused.
    host.next_mission();
    assert_chosen(&host, chosen);
    assert_eq!(host.number, number.wrapping_add(1));
    host.set_mission_sequence(Box::new(OneRound(Some(bad))));
    assert!(host.sequence_mission());
    assert_chosen(&host, chosen);
    assert!(!host.logs.iter().any(|l| matches!(l, HostLog::Fault { .. })));
}

#[test]
fn a_distance_the_table_leaves_out_moves_before_the_host_builds() {
    use tore_world::mission_layout::distance_table;
    // The synthetic Ukraine is far smaller than the real one, so the moved
    // 200 nm cannot fit it either: the build's refusal names the distance
    // the table moved 300 nm to, which is what the host built.
    let mut spec = MissionSpec::new("UKR", tore_formats::aircraft::AircraftId::F18);
    spec.wings[3].count = 1;
    spec.separation_nm = 300;
    assert_eq!(distance_table::allowed_for(&spec) & (1 << 10), 0);
    let error = match Host::new(
        spec,
        Arc::new(tore_world::test_support::resources::resources()),
        config(),
    ) {
        Ok(_) => panic!("200 nm cannot fit the synthetic map"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("Separation 200 nm"), "{error}");
}
