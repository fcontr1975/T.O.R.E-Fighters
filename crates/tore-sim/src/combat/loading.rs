//! Resolved FA HARDCanLoad/StoreWeight arithmetic; no string lookup in updates.
use super::invalid;
use tore_formats::{
    Result,
    aircraft::Hardpoint,
    weapons::{Countermeasures, Tank, Weapon},
};

/// T.O.R.E's physical store-pair policy, applied after retail compatibility
/// and weight arithmetic. Source records remain unchanged. John, 2026-10-25:
/// odd maxima above one round down; one becomes two, one on each side.
#[derive(Clone, Copy, Debug)]
pub struct StorePairs(bool);

impl StorePairs {
    pub fn of(rack: super::hardpoints::Rack, mount: crate::attitude::Vector) -> Self {
        Self(rack.two_sided(mount))
    }

    pub fn maximum(self, count: i32) -> i32 {
        if self.0 && count > 0 {
            if count == 1 { 2 } else { count - count % 2 }
        } else {
            count
        }
    }

    /// Preserve a saved single as a pair; larger odd quantities round down.
    /// Aircraft takeoff weight is checked after migration, not by this rule.
    pub fn saved_quantity(self, count: u16, maximum: i32) -> u16 {
        if self.0 {
            self.maximum(i32::from(count)).min(maximum.max(0)) as u16
        } else {
            count
        }
    }

    pub fn step(self, single_step: i32) -> i32 {
        if self.0 { 2 } else { single_step }
    }

    pub fn valid(self, count: u16) -> bool {
        !self.0 || count.is_multiple_of(2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Projectile {
        flags: u32,
        signature: u8,
        pod_count: i16,
        phoenix_family: bool,
    },
    Tank {
        flags: u8,
    },
    Equipment {
        flags: u8,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Store {
    pub kind: StoreKind,
    pub weight: i32,
}
impl Store {
    pub fn weapon(w: &Weapon) -> Self {
        Self {
            kind: StoreKind::Projectile {
                flags: w.flags,
                signature: w.seeker.signature,
                pod_count: w.burst.projectiles_in_pod,
                phoenix_family: w.source.starts_with("AIM54") || w.source.starts_with("AAML"),
            },
            weight: w.weight,
        }
    }
    pub fn tank(t: Tank) -> Self {
        Self {
            kind: StoreKind::Tank { flags: t.flags },
            weight: i32::from(t.empty_weight).wrapping_add(t.fuel_weight),
        }
    }
    /// A sensor carried as a store (a laser designator pod): equipment, as
    /// retail loads a SEE record on a hardpoint.
    pub fn sensor(flags: u8, weight: i32) -> Self {
        Self {
            kind: StoreKind::Equipment { flags },
            weight,
        }
    }
    pub fn countermeasures(c: Countermeasures) -> Self {
        Self {
            kind: StoreKind::Equipment { flags: c.flags },
            weight: i32::from(c.weight),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Station {
    pub flags: u16,
    pub capacity: i16,
    pub weight_class: u8,
}
impl Station {
    pub fn from_source(h: &Hardpoint) -> Result<Self> {
        Ok(Self {
            flags: h.flags as u16,
            capacity: i16::try_from(h.count).map_err(|_| invalid("station count exceeds word"))?,
            weight_class: u8::try_from(h.weight_class)
                .map_err(|_| invalid("station weight class exceeds byte"))?,
        })
    }
    /// FA 0x452980. Caller resolves the exact native default-name comparison
    /// once when constructing a compatibility matrix. Return is native capacity,
    /// not a complete mission/year/stock availability decision.
    pub fn allowed_count(self, store: Store, matches_default: bool) -> i32 {
        let h = self.flags;
        let allowed = if h & 8 != 0 {
            matches_default
        } else if matches_default {
            true
        } else {
            match store.kind {
                StoreKind::Projectile {
                    flags,
                    signature,
                    pod_count,
                    phoenix_family,
                } => {
                    if flags & 2 == 0
                        || (pod_count > 1 && h & 0x400 == 0)
                        || (h & 0x1000 != 0 && flags & 0x10000 != 0 && flags & 0x20000 == 0)
                    {
                        false
                    } else {
                        match signature {
                            1 | 2 => h & 0x80 != 0,
                            3 if phoenix_family => h & 0x10 != 0,
                            3 if flags & 0x200 != 0 => h & 0x40 != 0,
                            3 => h & 0x20 != 0,
                            _ => h & 0x100 != 0,
                        }
                    }
                }
                StoreKind::Tank { flags } => (h & 1 == 0 || flags & 1 != 0) && h & 0x200 != 0,
                StoreKind::Equipment { flags } => (h & 1 == 0 || flags & 1 != 0) && h & 0x400 != 0,
            }
        };
        if !allowed {
            return 0;
        }
        let weight = store.weight as i16;
        if self.weight_class == 0 || weight == 0 {
            i32::from(self.capacity)
        } else {
            (i32::from(self.weight_class) * 100 / i32::from(weight)).min(i32::from(self.capacity))
        }
    }
}

impl Station {
    /// Loadout cheat capacity. Fixed stations and the station's own default
    /// keep the normal rule. Otherwise the store gets the raw station capacity,
    /// except a projectile without flag 2 on a station with another default.
    /// Multiplayer participants, which also keep the normal rule, do not exist.
    pub fn cheat_count(self, store: Store, matches_default: bool, has_default: bool) -> i32 {
        if self.flags & 8 != 0 || matches_default {
            return self.allowed_count(store, matches_default);
        }
        match store.kind {
            StoreKind::Projectile { flags, .. } if flags & 2 == 0 && has_default => 0,
            _ => i32::from(self.capacity),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::hardpoints::{Location, Rack};

    #[test]
    fn store_pair_maxima_and_saved_quantities_keep_single_stations_unchanged() {
        for (location, paired, mount, applies) in [
            (Location::Wing, true, [8., -2., 0.], true),
            (Location::Wingtip, true, [20., 0., 0.], true),
            (Location::Shoulder, true, [8., -2., 0.], true),
            (Location::Separate, true, [8., -2., 0.], true),
            (Location::Wing, false, [8., -2., 0.], false),
            (Location::Wing, true, [0., -2., 0.], false),
            (Location::Centerline, false, [0., -2., 0.], false),
            (Location::Fuselage, true, [3., -2., 0.], true),
            (Location::Magazine, true, [3., -2., 0.], true),
            (Location::InternalBay, true, [3., -2., 0.], true),
            (Location::Tunnel, true, [3., -2., 0.], true),
            (Location::Tunnel, true, [0., -2., 0.], false),
            (Location::Fuselage, false, [3., -2., 0.], false),
        ] {
            let rule = StorePairs::of(
                Rack {
                    location: location.byte(),
                    paired,
                    ..Rack::default()
                },
                mount,
            );
            for (old, new) in [
                (0, 0),
                (1, 2),
                (2, 2),
                (3, 2),
                (4, 4),
                (5, 4),
                (32765, 32764),
                (32766, 32766),
            ] {
                assert_eq!(rule.maximum(old), if applies { new } else { old });
                assert_eq!(rule.step(1), if applies { 2 } else { 1 });
                assert_eq!(
                    rule.saved_quantity(old as u16, new),
                    if applies { new as u16 } else { old as u16 }
                );
            }
        }
    }

    #[test]
    fn even_paired_loads_have_equal_mass_zero_lateral_moment_and_balanced_release() {
        for location in [
            Location::Wing,
            Location::Wingtip,
            Location::Fuselage,
            Location::Tunnel,
            Location::Magazine,
            Location::InternalBay,
        ] {
            let rack = Rack {
                location: location.byte(),
                paired: true,
                ..Rack::default()
            };
            let size = crate::combat::store_sizes::StoreSize::body([8., 1.]);
            // Single odd maximum, two odd maxima, mixed even/odd, plus centreline.
            for counts in [vec![3], vec![3, 3], vec![4, 5, 1]] {
                let mut moment = 0.;
                let mut mass = [0.; 2];
                for (row, old) in counts.into_iter().enumerate() {
                    let mount = [8. + row as f64 * 4., -2., 0.];
                    let count = StorePairs::of(rack, mount).maximum(old) as u16;
                    let sides = rack.sides(mount, count);
                    assert_eq!(sides[0], sides[1]);
                    for side in 0..2 {
                        for slot in 0..sides[side] {
                            mass[side] += 300.;
                            moment += 300. * rack.store_point(mount, count, side, slot, size)[0];
                        }
                    }
                    for remaining in (1..=count).rev() {
                        let before = rack.sides(mount, remaining);
                        let side = usize::from(
                            rack.release_mount(mount, u32::from(remaining - 1))[0] < 0.,
                        );
                        assert!(
                            before[side] >= before[1 - side],
                            "fire the heavier side, left on a tie"
                        );
                        let after = rack.sides(mount, remaining - 1);
                        assert_eq!(before[side], after[side] + 1);
                        assert_eq!(before[1 - side], after[1 - side]);
                        assert!(after[0].abs_diff(after[1]) <= 1);
                    }
                }
                // A centreline store adds mass, but no lateral moment.
                let center = Rack::default();
                assert_eq!(StorePairs::of(center, [0.; 3]).maximum(3), 3);
                assert_eq!(mass[0], mass[1]);
                assert!(moment.abs() < 1e-9);
            }
        }
    }
    #[test]
    fn native_defaults_fixed_stations_pod_masks_and_weight_caps() {
        let missile = Store {
            kind: StoreKind::Projectile {
                flags: 2,
                signature: 2,
                pod_count: 1,
                phoenix_family: false,
            },
            weight: 300,
        };
        let s = Station {
            flags: 0x80,
            capacity: 4,
            weight_class: 10,
        };
        assert_eq!(s.allowed_count(missile, false), 3);
        assert_eq!(Station { flags: 8, ..s }.allowed_count(missile, false), 0);
        assert_eq!(Station { flags: 8, ..s }.allowed_count(missile, true), 3);
        let pod = Store {
            kind: StoreKind::Projectile {
                flags: 2,
                signature: 0,
                pod_count: 20,
                phoenix_family: false,
            },
            weight: 300,
        };
        assert_eq!(Station { flags: 0x100, ..s }.allowed_count(pod, false), 0);
        assert_eq!(Station { flags: 0x500, ..s }.allowed_count(pod, false), 3);
    }
    #[test]
    fn cheat_capacity_ignores_compatibility_and_weight_except_fixed_stations() {
        let missile = |flags| Store {
            kind: StoreKind::Projectile {
                flags,
                signature: 2,
                pod_count: 1,
                phoenix_family: false,
            },
            weight: 300,
        };
        let bombs_only = Station {
            flags: 0x100,
            capacity: 4,
            weight_class: 10,
        };
        assert_eq!(bombs_only.allowed_count(missile(2), false), 0);
        assert_eq!(bombs_only.cheat_count(missile(2), false, true), 4);
        let fixed = Station {
            flags: 8,
            ..bombs_only
        };
        assert_eq!(fixed.cheat_count(missile(2), false, true), 0);
        assert_eq!(bombs_only.cheat_count(missile(0), false, true), 0);
        assert_eq!(bombs_only.cheat_count(missile(0), false, false), 4);
    }
    #[test]
    fn special_radar_family_and_tanks_keep_distinct_source_rules() {
        let s = Station {
            flags: 0x20,
            capacity: 2,
            weight_class: 0,
        };
        let w = Store {
            kind: StoreKind::Projectile {
                flags: 2,
                signature: 3,
                pod_count: 1,
                phoenix_family: true,
            },
            weight: 100,
        };
        assert_eq!(s.allowed_count(w, false), 0);
        assert_eq!(Station { flags: 0x10, ..s }.allowed_count(w, false), 2);
        let tank = Store::tank(Tank {
            empty_weight: 100,
            fuel_weight: 900,
            flags: 1,
        });
        assert_eq!(
            Station {
                flags: 0x201,
                weight_class: 10,
                ..s
            }
            .allowed_count(tank, false),
            1
        );
        assert_eq!(Station { flags: 0x400, ..s }.allowed_count(tank, false), 0);
    }
}
