//! Transactional preflight loadout. Compatibility is translated; selectable live
//! weapons remain explicitly limited to the reviewed manual-weapons service.
use super::{live, loading};
use tore_formats::{
    Result,
    aircraft::{Aircraft, AircraftId, Hardpoint},
    weapons::Weapon,
};
/// One editable aggregate source row, independent of catalog category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditableStation {
    pub hardpoint: usize,
    pub location: u8,
    pub weapon: Option<usize>,
    pub tank: Option<usize>,
}
#[derive(Clone, Debug)]
pub struct Loadout {
    pub aircraft: AircraftId,
    pub configuration: live::Configuration,
    pub quantities: Vec<u16>,
    pub fuel_lbs: f64,
    pub internal_capacity_lbs: f64,
    pub empty_lbs: f64,
    pub maximum_lbs: f64,
    pub hardpoints: Vec<Hardpoint>,
    /// Original source hardpoints corresponding to the explicit tank station rows.
    pub tank_hardpoints: Vec<Hardpoint>,
    /// Loadout screen Cheat: any store on any station, up to its capacity.
    pub cheat: bool,
}
impl Loadout {
    pub fn editable_stations(&self) -> Vec<EditableStation> {
        let mut rows = std::collections::BTreeMap::new();
        for (hardpoint, slot) in self.configuration.hardpoint_slots.iter().enumerate() {
            if let Some(slot) = slot {
                rows.insert(
                    hardpoint,
                    EditableStation {
                        hardpoint,
                        location: self.hardpoints[*slot].location,
                        weapon: Some(*slot),
                        tank: None,
                    },
                );
            }
        }
        for (slot, tank) in self.configuration.tanks.iter().enumerate() {
            rows.entry(tank.hardpoint)
                .or_insert(EditableStation {
                    hardpoint: tank.hardpoint,
                    location: self.tank_hardpoints[slot].location,
                    weapon: None,
                    tank: None,
                })
                .tank = Some(slot);
        }
        rows.into_values().collect()
    }
    /// The order the Load Ordnance page lists its stations in: the internal
    /// gun first, then the centreline, then the other stations from the
    /// fuselage outward by their lateral offset (the source position's x,
    /// mirrored for the pair), ties by source row. Each entry indexes
    /// [`Self::editable_stations`]; station numbers, mission text and the wire
    /// keep the source order (display only, John 2026-10-16).
    pub fn station_display_order(&self) -> Vec<usize> {
        let rows = self.editable_stations();
        let mut order: Vec<usize> = (0..rows.len()).collect();
        let key = |row: &EditableStation| {
            let internal = row
                .weapon
                .and_then(|slot| self.configuration.stations.get(slot))
                .is_some_and(|station| station.internal);
            let group = if internal || row.location == 2 {
                0
            } else if row.location == 0 {
                1
            } else {
                2
            };
            let hardpoint = row
                .weapon
                .and_then(|slot| self.hardpoints.get(slot))
                .or_else(|| row.tank.and_then(|slot| self.tank_hardpoints.get(slot)));
            let lateral = hardpoint.map_or(i32::MAX, |h| h.position[0].abs());
            (group, lateral, row.hardpoint)
        };
        order.sort_by_key(|&index| key(&rows[index]));
        order
    }
    pub fn unload_tank(&mut self, slot: usize) -> Result<()> {
        let mut draft = self.clone();
        let tank = draft
            .configuration
            .tanks
            .get_mut(slot)
            .ok_or_else(|| super::invalid("Invalid tank station."))?;
        tank.quantity = 0;
        draft.refresh_equipment()?;
        *self = draft;
        Ok(())
    }
    /// Move one installed tank, leaving both loads unchanged if rejected.
    pub fn transfer_tank(&mut self, source: usize, target: usize) -> Result<()> {
        if source == target {
            return Ok(());
        }
        let from = self
            .configuration
            .tanks
            .get(source)
            .ok_or_else(|| super::invalid("Invalid source tank station."))?;
        if from.quantity == 0 {
            return Ok(());
        }
        let store = from
            .store
            .clone()
            .ok_or_else(|| super::invalid("Installed tank has no source type."))?;
        let to = self
            .configuration
            .tanks
            .get(target)
            .ok_or_else(|| super::invalid("Invalid destination tank station."))?;
        let capacity = self.tank_capacity(target, &store);
        if capacity <= 0 {
            return Err(super::invalid(
                "This tank cannot be loaded at this station.",
            ));
        }
        let existing = if to
            .store
            .as_ref()
            .is_some_and(|tank| tank.source == store.source)
        {
            to.quantity
        } else {
            0
        };
        if i32::from(existing) >= capacity {
            return Ok(());
        }
        let mut draft = self.clone();
        draft.configuration.tanks[target].store = Some(store);
        let step = self
            .tank_pairs(source)
            .step(self.tank_pairs(target).step(1)) as u16;
        if from.quantity < step || i32::from(existing + step) > capacity {
            return Ok(());
        }
        draft.configuration.tanks[target].quantity = existing + step;
        draft.configuration.tanks[source].quantity -= step;
        let hardpoint = draft.configuration.tanks[target].hardpoint;
        if let Some(Some(slot)) = draft.configuration.hardpoint_slots.get(hardpoint) {
            draft.quantities[*slot] = 0;
        }
        draft.refresh_equipment()?;
        *self = draft;
        Ok(())
    }
    pub fn ammunition(&self) -> Result<Vec<u16>> {
        self.configuration.ammunition(&self.quantities)
    }
    pub fn refresh_equipment(&mut self) -> Result<()> {
        // A designator pod leaves its station when a weapon or a tank takes
        // the station.
        let quantities = &self.quantities;
        let tanks = &self.configuration.tanks;
        let slots = &self.configuration.hardpoint_slots;
        self.configuration.designator_pods.retain(|pod| {
            quantities.get(pod.station).is_some_and(|q| *q == 0)
                && !tanks
                    .iter()
                    .any(|t| t.quantity > 0 && slots.get(t.hardpoint) == Some(&Some(pod.station)))
        });
        for pod in &mut self.configuration.gun_pods {
            pod.quantity = if self.configuration.stations[pod.station].weapon.source == "SUU16.JT" {
                self.quantities[pod.station]
            } else {
                0
            };
        }
        self.configuration.refresh_tanks()
    }
    pub fn new(a: &Aircraft, read: impl FnMut(&str) -> Result<Vec<u8>>) -> Result<Self> {
        let configuration = live::Configuration::from_source(a, read)?;
        let hardpoints = a
            .hardpoints
            .iter()
            .enumerate()
            .filter_map(|(index, h)| {
                configuration.hardpoint_slots[index].map(|slot| (slot, h.clone()))
            })
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect();
        Ok(Self {
            aircraft: a.id,
            quantities: configuration.stations.iter().map(|s| s.count).collect(),
            tank_hardpoints: configuration
                .tanks
                .iter()
                .map(|s| a.hardpoints[s.hardpoint].clone())
                .collect(),
            configuration,
            fuel_lbs: a.fields["internalFuel"].number()? as f64,
            internal_capacity_lbs: a.fields["internalFuel"].number()? as f64,
            empty_lbs: a.object["weight"].number()? as f64,
            maximum_lbs: a.fields["maxTakeoffWeight"].number()? as f64,
            hardpoints,
            cheat: false,
        })
    }
    pub fn tank_capacity(&self, slot: usize, tank: &live::TankStore) -> i32 {
        self.tank_pairs(slot)
            .maximum(self.tank_capacity_before_store_pairs(slot, tank))
    }
    /// Capacity before the physical store-pair policy, for the roster audit.
    pub fn tank_capacity_before_store_pairs(&self, slot: usize, tank: &live::TankStore) -> i32 {
        if !self.cheat
            && self.configuration.tanks.get(slot).is_none_or(|station| {
                !tore_formats::aircraft::stations::tank_allowed(
                    self.aircraft,
                    station.hardpoint,
                    &tank.source,
                )
            })
        {
            return 0;
        }
        self.tank_hardpoints
            .get(slot)
            .and_then(|h| {
                loading::Station::from_source(h).ok().map(|s| {
                    let matches_default = h.store.as_deref() == Some(tank.source.as_str());
                    // Preserve reviewed source-default quantities. The generic
                    // full-tank weight cap disagrees with A4E/F104 defaults;
                    // paired/weight consumer semantics remain unresolved.
                    if matches_default {
                        return h.count;
                    }
                    let store = loading::Store::tank(tank.tank);
                    if self.cheat {
                        s.cheat_count(store, matches_default, h.store.is_some())
                    } else {
                        s.allowed_count(store, matches_default)
                    }
                })
            })
            .unwrap_or(0)
            .clamp(0, 32766)
            .min(if self.cheat {
                32766
            } else {
                match (
                    self.aircraft,
                    self.configuration.tanks.get(slot).map(|s| s.hardpoint),
                ) {
                    (AircraftId::F15, Some(6)) => 1,
                    (AircraftId::F22 | AircraftId::F22n | AircraftId::Faxx, Some(8)) => 2,
                    _ => 32766,
                }
            })
    }
    /// Tanks start full. Filling an installed empty tank is a later UI feature.
    pub fn select_tank(&mut self, slot: usize, tank: live::TankStore) -> Result<()> {
        let capacity = self.tank_capacity(slot, &tank);
        if capacity <= 0 {
            return Err(super::invalid(
                "This tank cannot be loaded at this station.",
            ));
        }
        let mut configuration = self.configuration.clone();
        let station = configuration
            .tanks
            .get_mut(slot)
            .ok_or_else(|| super::invalid("Invalid tank station."))?;
        let hardpoint = station.hardpoint;
        station.store = Some(tank);
        station.quantity = capacity as u16;
        configuration.refresh_tanks()?;
        self.configuration = configuration;
        if let Some(Some(weapon)) = self.configuration.hardpoint_slots.get(hardpoint) {
            self.quantities[*weapon] = 0;
        }
        self.refresh_equipment()
    }
    pub fn change_tank(&mut self, slot: usize, direction: i32) -> Result<()> {
        let Some(station) = self.configuration.tanks.get(slot) else {
            return Ok(());
        };
        let Some(store) = &station.store else {
            return Ok(());
        };
        let capacity = self.tank_capacity(slot, store);
        let quantity = (i32::from(station.quantity)
            + direction.signum() * self.tank_pairs(slot).step(1))
        .clamp(0, capacity) as u16;
        let hardpoint = station.hardpoint;
        self.configuration.tanks[slot].quantity = quantity;
        if quantity > 0
            && let Some(Some(weapon)) = self.configuration.hardpoint_slots.get(hardpoint)
        {
            self.quantities[*weapon] = 0;
        }
        self.refresh_equipment()
    }
    pub fn clear_tanks(&mut self) -> Result<()> {
        for station in &mut self.configuration.tanks {
            station.quantity = 0;
        }
        self.refresh_equipment()
    }
    pub fn external_fuel_lbs(&self) -> f64 {
        self.configuration.external_fuel_lbs.iter().sum()
    }
    pub fn tank_shell_lbs(&self) -> f64 {
        self.configuration
            .tanks
            .iter()
            .filter_map(|s| {
                s.store
                    .as_ref()
                    .map(|store| f64::from(s.quantity) * f64::from(store.tank.empty_weight))
            })
            .sum()
    }
    fn clear_tank_on_weapon_slot(&mut self, slot: usize) -> Result<()> {
        for tank in &mut self.configuration.tanks {
            if self.configuration.hardpoint_slots.get(tank.hardpoint) == Some(&Some(slot)) {
                tank.quantity = 0;
            }
        }
        self.refresh_equipment()
    }
    pub fn capacity(&self, slot: usize, w: &Weapon) -> i32 {
        self.store_pairs(slot)
            .maximum(self.capacity_before_store_pairs(slot, w))
    }
    pub fn store_pairs(&self, slot: usize) -> loading::StorePairs {
        self.configuration.stations.get(slot).map_or_else(
            || loading::StorePairs::of(Default::default(), [0.; 3]),
            |s| loading::StorePairs::of(s.rack, s.mount),
        )
    }
    fn tank_pairs(&self, slot: usize) -> loading::StorePairs {
        self.configuration.tanks.get(slot).map_or_else(
            || loading::StorePairs::of(Default::default(), [0.; 3]),
            |s| loading::StorePairs::of(s.rack, s.mount),
        )
    }
    /// Migrate stored preflight loads. Live ammunition is never normalized:
    /// a single release may temporarily leave one extra store on a wing.
    pub fn normalize_saved_store_pairs(&mut self) -> Result<()> {
        for slot in 0..self.configuration.stations.len() {
            let policy = self.store_pairs(slot);
            let cap = self.capacity(slot, &self.configuration.stations[slot].weapon);
            let station = &mut self.configuration.stations[slot];
            station.count =
                policy.saved_quantity(policy.maximum(i32::from(station.count)) as u16, cap);
            self.quantities[slot] = policy.saved_quantity(self.quantities[slot], cap);
        }
        for slot in 0..self.configuration.tanks.len() {
            if let Some(store) = &self.configuration.tanks[slot].store {
                let cap = self.tank_capacity(slot, store);
                self.configuration.tanks[slot].quantity = self
                    .tank_pairs(slot)
                    .saved_quantity(self.configuration.tanks[slot].quantity, cap);
            }
        }
        self.refresh_equipment()
    }
    /// Retail arithmetic plus existing aircraft permissions, before pairing.
    pub fn capacity_before_store_pairs(&self, slot: usize, w: &Weapon) -> i32 {
        let row = self
            .configuration
            .hardpoint_slots
            .iter()
            .position(|s| *s == Some(slot));
        if !self.cheat {
            if self.aircraft == AircraftId::F15
                && row == Some(6)
                && !super::ballistics::is_bomb(w)
                && !super::guided::is_glide_bomb(w)
            {
                return 0;
            }
            // The F-22's inboard pair and the Eagle's wing pylons take a
            // tank or air-to-air missiles: the mask's other classes (the
            // Sparrow's, bombs) stay off them.
            if matches!(
                (self.aircraft, row),
                (
                    AircraftId::F22 | AircraftId::F22n | AircraftId::Faxx,
                    Some(8)
                ) | (AircraftId::F15, Some(7))
            ) && !matches!(
                w.source.as_str(),
                "AIM120.JT" | "AIM9B.JT" | "AIM9M.JT" | "AIM9X.JT"
            ) {
                return 0;
            }
        }
        // Guarded extra permissions on an existing source row (the F-16C's
        // wingtip AIM-120, one a tip): the row's own count, no weight cap.
        if !self.cheat
            && row.is_some_and(|row| {
                tore_formats::aircraft::stations::extra_weapon(
                    self.aircraft,
                    self.configuration.hardpoint_slots.len(),
                    row,
                    &w.source,
                )
            })
        {
            return self
                .hardpoints
                .get(slot)
                .map_or(0, |h| h.count)
                .clamp(0, 32766);
        }
        // R-27 carriage is an explicit family policy, not generic IR rail
        // compatibility. Cheat keeps its existing unrestricted behavior.
        if !self.cheat
            && matches!(
                w.source.as_str(),
                "AA10.JT" | "R27ER.JT" | "R27T.JT" | "R27ET.JT"
            )
        {
            let extended = matches!(w.source.as_str(), "R27ER.JT" | "R27ET.JT");
            if !matches!(
                self.aircraft,
                AircraftId::Su27 | AircraftId::Su35 | AircraftId::Mig29
            ) && (extended || self.aircraft != AircraftId::Yak141)
            {
                return 0;
            }
            // MIG29.PT is the Fulcrum-C: R/ER/T/ET on its inner pair only.
            // Source row 5 represents those two stations.
            if self.aircraft == AircraftId::Mig29
                && self
                    .configuration
                    .hardpoint_slots
                    .iter()
                    .position(|s| *s == Some(slot))
                    != Some(5)
            {
                return 0;
            }
            // R-27s use medium rails, never Flanker wingtip rails. IR heads
            // also need an unobstructed underwing view rather than a tunnel.
            let infrared = matches!(w.source.as_str(), "R27T.JT" | "R27ET.JT");
            if self.hardpoints.get(slot).is_none_or(|h| {
                h.location == 5
                    || (infrared && h.location != 4)
                    || (h.flags & 0x40 == 0 && h.store.as_deref() != Some("AA10.JT"))
            }) {
                return 0;
            }
        }
        // Boulder and Moose load only under Cheat loading.
        if novelty(&w.source) && !self.cheat {
            return 0;
        }
        // The reviewed pod contract is limited to source-installed gun-pod
        // stations. Unrelated rocket launchers keep their existing semantics.
        if w.source == "SUU16.JT"
            && !self
                .configuration
                .gun_pods
                .iter()
                .any(|pod| pod.station == slot)
        {
            return 0;
        }
        self.hardpoints
            .get(slot)
            .and_then(|h| {
                loading::Station::from_source(h).ok().map(|s| {
                    let store = loading::Store::weapon(w);
                    let matches_default = h.store.as_deref() == Some(&w.source)
                        || tore_formats::weapons::corrections::donor(&w.source)
                            .filter(|_| w.source.starts_with("GBU"))
                            .is_some_and(|donor| h.store.as_deref() == Some(donor));
                    if self.cheat {
                        s.cheat_count(store, matches_default, h.store.is_some())
                    } else {
                        s.allowed_count(store, matches_default)
                    }
                })
            })
            .unwrap_or(0)
            .max(0)
            // One pod a station, whatever the row's own capacity and weight
            // class allow (the F-4B's centreline row holds four Sidewinders).
            .min(if live::is_gun_pod(w) { 1 } else { i32::MAX })
    }
    /// How many of a designator pod station `slot` takes: one where the
    /// retail equipment rule allows it (a pod station, flag 0x400, within
    /// its weight class), else none. Cheat loading follows the cheat rule.
    pub fn designator_capacity(&self, slot: usize, pod: &super::sight::DesignatorStore) -> i32 {
        self.hardpoints
            .get(slot)
            .and_then(|h| {
                loading::Station::from_source(h).ok().map(|s| {
                    let store = loading::Store::sensor(pod.flags, pod.weight_lbs);
                    let matches_default = h
                        .store
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case(&pod.source));
                    if self.cheat {
                        s.cheat_count(store, matches_default, h.store.is_some())
                    } else {
                        s.allowed_count(store, matches_default)
                    }
                })
            })
            .unwrap_or(0)
            .clamp(0, 1)
    }
    /// Load a laser designator pod on station `slot` (decision 7), in place
    /// of its weapon and of a tank on the same hardpoint. An aircraft
    /// carries one pod: loading another moves it. Leaves the loadout as it
    /// was if the station cannot take it.
    pub fn load_designator(
        &mut self,
        slot: usize,
        pod: super::sight::DesignatorStore,
    ) -> Result<()> {
        if self.designator_capacity(slot, &pod) <= 0 {
            return Err(super::invalid(
                "This store cannot be loaded at this station.",
            ));
        }
        let mut draft = self.clone();
        draft.quantities[slot] = 0;
        for tank in &mut draft.configuration.tanks {
            if draft.configuration.hardpoint_slots.get(tank.hardpoint) == Some(&Some(slot)) {
                tank.quantity = 0;
            }
        }
        draft.configuration.designator_pods = vec![super::sight::DesignatorPod {
            station: slot,
            store: pod,
        }];
        draft.refresh_equipment()?;
        *self = draft;
        Ok(())
    }
    /// Take the designator pod off station `slot`, if it holds one.
    pub fn unload_designator(&mut self, slot: usize) -> Result<()> {
        self.configuration
            .designator_pods
            .retain(|pod| pod.station != slot);
        self.refresh_equipment()
    }
    /// The designator pod on station `slot`.
    pub fn designator(&self, slot: usize) -> Option<&super::sight::DesignatorStore> {
        self.configuration
            .designator_pods
            .iter()
            .find(|pod| pod.station == slot)
            .map(|pod| &pod.store)
    }
    pub fn select(&mut self, slot: usize, w: Weapon) -> Result<()> {
        let capacity = self.capacity(slot, &w);
        if capacity <= 0 || capacity >= 32767 {
            return Err(super::invalid(
                "This store cannot be loaded at this station.",
            ));
        }
        self.configuration.stations[slot].weapon = w;
        self.configuration.stations[slot].count = capacity as u16;
        self.quantities[slot] = capacity as u16;
        self.clear_tank_on_weapon_slot(slot)
    }
    pub fn change(&mut self, slot: usize, direction: i32) {
        if let Some(s) = self.configuration.stations.get(slot) {
            let cap = self.capacity(slot, &s.weapon);
            let step = self.store_pairs(slot).step(quantity_step(cap));
            self.quantities[slot] =
                (i32::from(self.quantities[slot]) + direction.signum() * step).clamp(0, cap) as u16;
            if self.quantities[slot] > 0 && self.configuration.stations[slot].count == 0 {
                self.configuration.stations[slot].count = cap as u16;
            }
            if self.quantities[slot] > 0 {
                let _ = self.clear_tank_on_weapon_slot(slot);
            } else {
                let _ = self.refresh_equipment();
            }
        }
    }
    /// Move one quantity step, preserving both loads if the destination rejects it.
    pub fn transfer(&mut self, source: usize, target: usize) -> Result<()> {
        if source == target || self.quantities.get(source).copied().unwrap_or(0) == 0 {
            return Ok(());
        }
        let weapon = &self.configuration.stations[source].weapon;
        let capacity = self.capacity(target, weapon);
        if capacity <= 0 || capacity >= 32767 {
            return Err(super::invalid(
                "This store cannot be loaded at this station.",
            ));
        }
        let existing = if self.configuration.stations[target].weapon.source == weapon.source {
            i32::from(self.quantities[target])
        } else {
            0
        };
        let step = self
            .store_pairs(source)
            .step(self.store_pairs(target).step(quantity_step(capacity)));
        let moved = step
            .min(i32::from(self.quantities[source]))
            .min((capacity - existing).max(0)) as u16;
        let moved = if (self.store_pairs(source).step(1) == 2
            || self.store_pairs(target).step(1) == 2)
            && !moved.is_multiple_of(2)
        {
            moved - 1
        } else {
            moved
        };
        if moved > 0 {
            self.configuration.stations[target].weapon = weapon.clone();
            self.configuration.stations[target].count = capacity as u16;
            self.quantities[target] = existing as u16 + moved;
            self.quantities[source] -= moved;
            self.clear_tank_on_weapon_slot(target)?;
        }
        Ok(())
    }
    pub fn restrict_to_guns(&mut self) {
        for (station, count) in self.configuration.stations.iter().zip(&mut self.quantities) {
            if !self.aircraft.mounts_gun(&station.weapon.source) {
                *count = 0;
            }
        }
        let _ = self.refresh_equipment();
    }
    pub fn fuel(&mut self, up: bool) {
        self.fuel_lbs =
            (self.fuel_lbs + if up { 500. } else { -500. }).clamp(0., self.internal_capacity_lbs);
    }
    pub fn total_lbs(&self) -> f64 {
        self.empty_lbs
            + self.fuel_lbs
            + f64::from(self.configuration.external_equipment_lbs)
            + self
                .configuration
                .stations
                .iter()
                .zip(&self.quantities)
                .filter(|(s, _)| !s.internal && s.weapon.source != "SUU16.JT")
                .map(|(s, n)| s.weapon.carried_weight_lb() * f64::from(*n))
                .sum::<f64>()
    }
    pub fn validate(&self) -> Result<()> {
        self.validate_for_editing()?;
        if self.total_lbs() > self.maximum_lbs {
            return Err(super::invalid(
                "This plane is too heavy to take off. Remove stores or fuel.",
            ));
        }
        Ok(())
    }
    /// A structurally valid load may open in the editor while overweight.
    /// Fly and non-interactive starts must still use `validate`.
    pub fn validate_for_editing(&self) -> Result<()> {
        self.ammunition()?;
        if self.quantities.len() != self.configuration.stations.len()
            || !self.fuel_lbs.is_finite()
            || !(0. ..=self.internal_capacity_lbs).contains(&self.fuel_lbs)
        {
            return Err(super::invalid("Invalid loadout fuel or station state."));
        }
        for (i, (s, n)) in self
            .configuration
            .stations
            .iter()
            .zip(&self.quantities)
            .enumerate()
        {
            if !self.store_pairs(i).valid(*n) || !self.store_pairs(i).valid(s.count) {
                return Err(super::invalid("Paired stores must be loaded in pairs."));
            }
            if *n > 0 && novelty(&s.weapon.source) && !self.cheat {
                return Err(super::invalid(
                    "This store is available with Cheat loading only.",
                ));
            }
            if i32::from(*n) > self.capacity(i, &s.weapon) {
                return Err(super::invalid("Station quantity exceeds capacity."));
            }
            if *n > 0 && !supported(&s.weapon.source) {
                return Err(super::invalid(
                    "This store is available for setup only; its flight behavior is not implemented yet.",
                ));
            }
        }
        if self.configuration.tanks.len() != self.tank_hardpoints.len() {
            return Err(super::invalid("Invalid tank station state."));
        }
        for pod in &self.configuration.designator_pods {
            if self.designator_capacity(pod.station, &pod.store) <= 0
                || self.quantities.get(pod.station).is_none_or(|q| *q > 0)
            {
                return Err(super::invalid(
                    "A designator pod cannot be loaded at this station.",
                ));
            }
        }
        for (index, tank) in self.configuration.tanks.iter().enumerate() {
            if !self.tank_pairs(index).valid(tank.quantity) {
                return Err(super::invalid("Paired tanks must be loaded in pairs."));
            }
            if tank.quantity > 0 {
                let store = tank
                    .store
                    .as_ref()
                    .ok_or_else(|| super::invalid("Tank quantity has no selected type."))?;
                if i32::from(tank.quantity) > self.tank_capacity(index, store) {
                    return Err(super::invalid("Tank quantity exceeds station capacity."));
                }
                if let Some(Some(slot)) = self.configuration.hardpoint_slots.get(tank.hardpoint)
                    && self.quantities[*slot] > 0
                {
                    return Err(super::invalid(
                        "A weapon and a tank cannot share the same station.",
                    ));
                }
            }
        }
        let mut derived = self.configuration.clone();
        for pod in &mut derived.gun_pods {
            pod.quantity = if derived.stations[pod.station].weapon.source == "SUU16.JT" {
                self.quantities[pod.station]
            } else {
                0
            };
        }
        derived.refresh_tanks()?;
        if derived.external_equipment_lbs != self.configuration.external_equipment_lbs
            || derived.external_fuel_lbs != self.configuration.external_fuel_lbs
        {
            return Err(super::invalid(
                "Tank fuel and mass disagree with installed quantities.",
            ));
        }
        Ok(())
    }
}
fn quantity_step(capacity: i32) -> i32 {
    if capacity > 300 {
        100
    } else if capacity > 100 {
        10
    } else {
        1
    }
}
/// Weapons connected to flight, by record name. Shared by launch validation
/// and the ordnance catalog, including Cheat loading; importing a definition
/// does not enable it. The ordnance audit requires every one to be imported.
pub const SUPPORTED_WEAPONS: &[&str] = &[
    "AA11.JT",
    "AA11B.JT",
    "AA12.JT",
    "AA2.JT",
    "AA8.JT",
    "AAML.JT",
    "AS7.JT",
    "B13.JT",
    "B8.JT",
    "GSH23.JT",
    "GSH301.JT",
    "GSH6_30.JT",
    "M61.JT",
    "DEFA.JT",
    "MK12.JT",
    "MK82.JT",
    "LAU61.JT",
    "AIM54C.JT",
    "AIM9X.JT",
    "AIM120.JT",
    "AIM9M.JT",
    "AGM65G.JT",
    "MICA.JT",
    "R530.JT",
    "R550.JT",
    "AA10.JT",
    "R27ER.JT",
    "R27T.JT",
    "R27ET.JT",
    "AGM88.JT",
    "AIM7.JT",
    "AIM7E.JT",
    "AIM9B.JT",
    "AT2.JT",
    "C_105.JT",
    "C_25.JT",
    "C_40.JT",
    "GAU12.JT",
    "GAU8.JT",
    "GSH30.JT",
    "SUU16.JT",
    "T12_4.JT",
    "T30_1.JT",
    // A2G round, X1: free-fall, retarded and pack bombs, cluster bombs and
    // the LAU-10 pod. The bomb motion, packs and pods are B1's.
    "MK84.JT",
    "FAB250.JT",
    "FAB500.JT",
    "FAB1000.JT",
    "MK82P3.JT",
    "MK82HD.JT",
    "MK82P.JT",
    "MK82AIR.JT",
    "CBU87.JT",
    "CBU89.JT",
    "MK20.JT",
    "RBK250.JT",
    "RBK500.JT",
    "LAU10.JT",
    // Guided air-to-ground weapons, slice W1 (docs/spec/air-to-ground.md,
    // "Weapon classes" 4, 5 and 12): the laser bombs, the JDAMs and the
    // laser missiles. The AT-2 above now flies command guidance.
    "GBU12.JT",
    "GBU16.JT",
    "GBU38.JT",
    "GBU10.JT",
    "GBU10A.JT",
    "PAVEWAY.JT",
    "PAVEWA3.JT",
    "GBU28.JT",
    "GBU29.JT",
    "GBU29P.JT",
    "GBU30.JT",
    "AS14.JT",
    "AS30.JT",
    "AT12.JT",
    // Air-to-ground missiles, slice W2 (docs/spec/air-to-ground.md, "Weapon
    // classes" 8 to 11): the second Maverick and the SLAM (infrared), the
    // Shrike (anti-radiation), Harpoon, Exocet and Kickback (active radar on
    // the A-G track) and the AS 15 (supported by the A-G track).
    "AGM65A.JT",
    "AGM84E.JT",
    "AGM45.JT",
    "AGM84A.JT",
    "AM39.JT",
    "AS16.JT",
    "AS15.JT",
];
/// The novelty bombs, Boulder and Moose: they fly as free-fall bombs but load
/// only with the Load Ordnance page's Cheat loading (A2G decision 16,
/// opinionated, John 2026-10-10). They are not in [`SUPPORTED_WEAPONS`], so
/// the ordnance audit and the normal catalog never count them.
pub const NOVELTY_WEAPONS: &[&str] = &["~BLDR.JT", "~MOOSE.JT"];
/// Whether `name` is a novelty store that needs Cheat loading.
pub fn novelty(name: &str) -> bool {
    NOVELTY_WEAPONS.contains(&name)
}
pub fn supported(name: &str) -> bool {
    SUPPORTED_WEAPONS.contains(&name) || novelty(name)
}
