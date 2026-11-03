//! The enemy distances the creator offers: the checked-in table's choices
//! for the theater, the ground target and the start
//! (`tore_world::mission_layout::distance_table`, John 2026-10-28). Nothing
//! here loads terrain or starts a thread; a lookup is a few comparisons.
use super::*;
use tore_world::mission_layout::distance_table;

impl QuickMission {
    /// The table's mask for the draft: one bit per [`SEPARATION_NM`] choice.
    fn distance_mask(&self) -> u16 {
        let Some(code) = self.theater_codes.get(self.draft.values[13]) else {
            return distance_table::ALL;
        };
        let target = self.draft.values[30]
            .checked_sub(1)
            .and_then(|at| ground_targets(code).get(at))
            .copied();
        distance_table::allowed(code, target, self.ground_start())
    }
    /// The chosen ground target's note in the table: what its layout
    /// always leaves out (`distance_table::note`).
    pub(super) fn target_note(&self) -> Option<&'static str> {
        let code = self.theater_codes.get(self.draft.values[13])?;
        let stem = ground_targets(code).get(self.draft.values[30].checked_sub(1)?)?;
        distance_table::note(code, stem)
    }
    /// The offered choices (indices into [`SEPARATION_NM`]), ascending.
    pub(super) fn offered_distances(&self) -> Vec<usize> {
        let mask = self.distance_mask();
        (0..SEPARATION_NM.len())
            .filter(|i| mask & (1 << i) != 0)
            .collect()
    }
    /// The offered choice after (`forward`) or before the current one,
    /// wrapping round, as a click or a right-click steps a field.
    pub(super) fn next_distance(&self, forward: bool) -> usize {
        let offered = self.offered_distances();
        let current = self.draft.values[17];
        let found = if forward {
            offered.iter().find(|i| **i > current).or(offered.first())
        } else {
            offered
                .iter()
                .rev()
                .find(|i| **i < current)
                .or(offered.last())
        };
        found.copied().unwrap_or(current)
    }
    /// Moves a distance the table leaves out to the largest offered one at
    /// or below it, or the smallest above it when none is below. The
    /// read-only creator shows the King's choice as it is.
    pub(super) fn fit_distance(&mut self) {
        if self.view.is_none() {
            self.draft.values[17] =
                distance_table::clamp_index(self.distance_mask(), self.draft.values[17]);
        }
    }
    /// The offered distances, for the snapshot's `quick-snapshot` line.
    pub fn placement_snapshot_report(&self) -> String {
        let enabled = self
            .offered_distances()
            .into_iter()
            .map(|i| SEPARATION_NM[i].to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "start {} enabled-distances {enabled}",
            if self.ground_start() {
                "ground"
            } else {
                "airborne"
            }
        )
    }
    /// Draws the layout's seed for the next flight (the map and the lobby
    /// show and send it), once, when the mission lays out a surface.
    pub(super) fn reserve_placement_seed(&mut self) {
        if self.lays_out_surface() && !self.seed_pending {
            if self.view.is_none() {
                self.surface_seed = super::fresh_seed();
            }
            self.seed_pending = true;
        }
    }
    /// Snapshot and command-line launch entry point: the draft as the
    /// creator would leave it before its first frame, its distance offered
    /// and its seed drawn.
    pub fn settle_distance(&mut self) {
        self.fit_distance();
        if self.resources.is_some() {
            self.reserve_placement_seed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creator() -> QuickMission {
        crate::quick_mission::tests::setup()
    }

    fn select(q: &mut QuickMission, code: &str) {
        let index = q.theater_catalog.iter().position(|c| c == code).unwrap();
        q.theater(index);
    }

    #[test]
    fn the_selector_lists_and_steps_only_offered_distances() {
        let mut q = creator();
        for code in tore_formats::quick_template::tables::THEATERS {
            select(&mut q, code);
            let mask = distance_table::allowed(code, None, false);
            assert_eq!(
                q.offered_distances(),
                (0..11).filter(|i| mask & (1 << i) != 0).collect::<Vec<_>>()
            );
            assert_eq!(q.selector_values(17).len(), q.offered_distances().len());
            assert!(
                q.offered_distances().contains(&q.draft.values[17]),
                "{code}"
            );
            // Stepping forward through the field visits every offered
            // distance once and comes back.
            let first = q.draft.values[17];
            let mut seen = vec![first];
            loop {
                let next = q.next_distance(true);
                q.apply(17, next);
                if q.draft.values[17] == first {
                    break;
                }
                seen.push(q.draft.values[17]);
            }
            seen.sort_unstable();
            assert_eq!(seen, q.offered_distances(), "{code}");
            let back = q.next_distance(false);
            q.apply(17, back);
            assert!(q.offered_distances().contains(&q.draft.values[17]));
        }
    }

    #[test]
    fn a_theater_change_moves_the_distance_down_to_an_offered_one() {
        let mut q = creator();
        let row = distance_table::THEATERS
            .iter()
            .find(|row| row.airborne & (1 << 10) == 0)
            .expect("a theater without 300 nm");
        // 300 nm where everything is offered: a ground target's ground start.
        let open = distance_table::THEATERS
            .iter()
            .find(|row| row.airborne & (1 << 10) != 0)
            .map(|row| row.code);
        if let Some(code) = open {
            select(&mut q, code);
            q.apply(17, 10);
            assert_eq!(q.draft.values[17], 10);
        } else {
            q.draft.values[17] = 10;
        }
        select(&mut q, row.code);
        let top = 31 - u32::from(row.airborne).leading_zeros();
        assert_eq!(q.draft.values[17], top as usize, "{}", row.code);
        // Never above what was chosen: 1 nm stays 1 nm.
        q.apply(17, 0);
        assert_eq!(q.draft.values[17], 0);
    }
}
