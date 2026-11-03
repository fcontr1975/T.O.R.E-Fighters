//! Start geometry after airport replacement, with integer runway ranking.
use super::Ground;
use crate::surface::{StartPoints, Surface};
use tore_sim::airport::Scene;

/// Rank canonical airfields by their authored integer placement origins.
/// Template strips participate too. Floating point runway rendering geometry
/// never feeds the surface digest.
pub fn place(surface: &Surface, ground: &Ground<'_>, scene: &Scene) -> Option<StartPoints> {
    let mut ranked = super::starts::place(surface, ground)?;
    for ids in [&mut ranked.blue_airfields, &mut ranked.red_airfields] {
        let mut seen = std::collections::BTreeSet::new();
        for id in ids.iter_mut() {
            *id = scene.canonical_id(*id);
        }
        ids.retain(|id| seen.insert(*id));
    }
    Some(ranked)
}
