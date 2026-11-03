//! Exhaustive imported-roster check of the physical store-pair load rule.
use super::*;

pub(super) fn audit(
    standard: &Loadout,
    aircraft: &Aircraft,
    data: &BTreeMap<String, Vec<u8>>,
) -> AppResult<()> {
    let records = tore_sim::combat::hardpoints::default_records(aircraft);
    let mut cheat = standard.clone();
    cheat.cheat = true;
    for row in standard.editable_stations() {
        if let Some(slot) = row.weapon {
            let station = &standard.configuration.stations[slot];
            let policy = standard.store_pairs(slot);
            if policy.step(1) != 2 {
                continue;
            }
            if !policy.valid(station.count) || !policy.valid(standard.quantities[slot]) {
                return Err("default store load is not paired".into());
            }
            let source = &records[row.hardpoint];
            println!(
                "store-pair source: {} row={} maxItems={} weight_limit_lb={} default={} count={}",
                aircraft.id.selection_key(),
                row.hardpoint,
                standard.hardpoints[slot].count,
                standard.hardpoints[slot].weight_class * 100,
                station.weapon.source,
                station.count
            );
            if source.store.as_deref() == Some(&station.weapon.source)
                && source.count != i32::from(station.count)
            {
                println!(
                    "store-pair default: {} row={} {} {}->{} (human and AI)",
                    aircraft.id.selection_key(),
                    row.hardpoint,
                    station.weapon.source,
                    source.count,
                    station.count
                );
            }
            // Include imported setup-only definitions, not just live weapons.
            for (name, bytes) in data.iter().filter(|(name, _)| name.ends_with(".JT")) {
                let weapon = Weapon::parse(name, bytes)?;
                let cheat_old = cheat.capacity_before_store_pairs(slot, &weapon);
                let cheat_new = cheat.capacity(slot, &weapon);
                if cheat_old != cheat_new {
                    println!(
                        "store-pair cheat maximum: {} row={} {} {}->{}",
                        aircraft.id.selection_key(),
                        row.hardpoint,
                        name,
                        cheat_old,
                        cheat_new
                    );
                }
                let old = standard.capacity_before_store_pairs(slot, &weapon);
                let new = standard.capacity(slot, &weapon);
                if new != old {
                    println!(
                        "store-pair maximum: {} row={} {} {}->{} weight={} live={}",
                        aircraft.id.selection_key(),
                        row.hardpoint,
                        name,
                        old,
                        new,
                        weapon.weight,
                        SUPPORTED_WEAPONS.contains(&name.as_str())
                    );
                    if new != policy.maximum(old) || new % 2 != 0 {
                        return Err("store maximum differs from pair policy".into());
                    }
                    let mut draft = standard.clone();
                    draft.select(slot, weapon)?;
                    draft.change(slot, -1);
                    if draft.quantities[slot] != new as u16 - 2 {
                        return Err("store quantity does not step by a pair".into());
                    }
                    // Old singles become pairs; larger odd counts round down. A lobby request
                    // with the old odd count must not bypass validation.
                    let mut saved = LoadoutSpec::of(&draft);
                    saved.stations[slot].count = old as u16;
                    saved.stations[slot].quantity = old as u16;
                    let migrated = saved.apply(standard.clone(), data, None)?;
                    if migrated.quantities[slot] != policy.saved_quantity(old as u16, new)
                        || migrated.configuration.stations[slot].count != new as u16
                    {
                        return Err("old odd paired load migration differs".into());
                    }
                    if saved.check_for_plane(aircraft, data, false).is_ok() {
                        return Err("lobby accepted an odd paired load".into());
                    }
                }
            }
        }
        if let Some(slot) = row.tank {
            let station = &standard.configuration.tanks[slot];
            let policy = loading::StorePairs::of(station.rack, station.mount);
            if !policy.valid(station.quantity) {
                return Err("default paired tanks are not paired".into());
            }
            for (name, bytes) in data.iter().filter(|(name, _)| name.ends_with(".GAS")) {
                let store = TankStore::parse(name, bytes)?;
                let cheat_old = cheat.tank_capacity_before_store_pairs(slot, &store);
                let cheat_new = cheat.tank_capacity(slot, &store);
                if cheat_old != cheat_new {
                    println!(
                        "store-pair cheat maximum: {} row={} {} {}->{}",
                        aircraft.id.selection_key(),
                        row.hardpoint,
                        name,
                        cheat_old,
                        cheat_new
                    );
                }
                let old = standard.tank_capacity_before_store_pairs(slot, &store);
                let new = standard.tank_capacity(slot, &store);
                if old != new {
                    println!(
                        "store-pair maximum: {} row={} {} {}->{} weight={} live=true",
                        aircraft.id.selection_key(),
                        row.hardpoint,
                        name,
                        old,
                        new,
                        i32::from(store.tank.empty_weight) + store.tank.fuel_weight
                    );
                    let mut draft = standard.clone();
                    draft.select_tank(slot, store)?;
                    let mut saved = LoadoutSpec::of(&draft);
                    saved
                        .tanks
                        .as_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|tank| usize::from(tank.hardpoint) == row.hardpoint)
                        .unwrap()
                        .quantity = old as u16;
                    let migrated = saved.apply(standard.clone(), data, None)?;
                    if migrated.configuration.tanks[slot].quantity
                        != policy.saved_quantity(old as u16, new)
                    {
                        return Err("saved paired tank migration differs".into());
                    }
                    if saved.check_for_plane(aircraft, data, false).is_ok() {
                        return Err("lobby accepted an odd paired tank load".into());
                    }
                }
            }
        }
    }
    println!(
        "store-pair audit: {} defaults, maxima, steps, saved loads and lobby checked",
        aircraft.id.selection_key()
    );
    if aircraft.id == AircraftId::F18 {
        overweight_saved_load(standard, data)?;
    }
    Ok(())
}

/// Exercise the actual non-interactive world start with an overweight migrated
/// save, not just the loadout validator. The editor keeps this same draft.
fn overweight_saved_load(standard: &Loadout, data: &BTreeMap<String, Vec<u8>>) -> AppResult<()> {
    let mut old = standard.clone();
    old.cheat = true;
    old.clear_tanks()?;
    old.quantities.fill(0);
    old.fuel_lbs = 0.;
    let weapon = Weapon::parse("GBU28.JT", &data["GBU28.JT"])?;
    for slot in 0..old.quantities.len() {
        if old.store_pairs(slot).step(1) == 2 && old.capacity(slot, &weapon) > 0 {
            old.select(slot, weapon.clone())?;
            old.quantities[slot] = 1;
        }
    }
    old.fuel_lbs = (old.maximum_lbs - old.total_lbs() - f64::from(weapon.weight) / 2.)
        .clamp(0., old.internal_capacity_lbs);
    let before = old.total_lbs();
    let saved = LoadoutSpec::of(&old);
    let migrated = saved.apply(standard.clone(), data, None)?;
    migrated.validate_for_editing()?;
    if before > old.maximum_lbs || migrated.total_lbs() <= migrated.maximum_lbs {
        return Err("saved-load overweight fixture does not cross the takeoff limit".into());
    }
    let mut mission = MissionSpec::new("UKR", AircraftId::F18);
    mission.loadout = Some(saved);
    let error = match tore_world::world::World::new(
        &mission,
        data,
        tore_world::world::Seating::SinglePlayer,
    ) {
        Ok(_) => return Err("non-interactive start accepted an overweight migrated load".into()),
        Err(error) => error.to_string(),
    };
    if error != "This plane is too heavy to take off. Remove stores or fuel." {
        return Err(format!("non-interactive overweight refusal differs: {error}").into());
    }
    println!(
        "store-pair overweight save: {before:.0}->{} lb, maximum {}; editor retains pairs, non-interactive start refused with existing weight message",
        migrated.total_lbs(),
        migrated.maximum_lbs
    );
    Ok(())
}
