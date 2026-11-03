//! The template's site: whether it is anchored, where it relocates, and the
//! jitter of its units (docs/spec/surface-defenses.md, "Jitter" and
//! "Relocation").
use super::{
    Anchor, Inputs, Obstacles, Placed, Pose, TypeInfo, disc, ground::CELL_FT, set_pose, spread,
    stands, trig, xz, *,
};
use crate::surface::{GroupTransform, IdRange, Surface, Unit, UnitKind};
use std::collections::BTreeMap;
use tore_formats::surface_unit::class;

/// The stream ordinal of the template's own draws (the site, the fleet,
/// the starts), apart from every object ordinal.
pub(super) const SITE_ORDINAL: u32 = u32::MAX;
pub(super) const FLEET_ORDINAL: u32 = u32::MAX - 1;
pub(super) const START_ORDINAL: u32 = u32::MAX - 2;
pub(super) const FRONT_ORDINAL: u32 = u32::MAX - 3;

/// How an object moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Rule {
    /// Structures, strips, routed units, radar sites and parked aircraft:
    /// they anchor the site and never jitter.
    Fixed,
    Defense,
    Barrage,
    Vehicle,
    Ship,
}

impl Rule {
    pub(super) fn of(unit: &Unit) -> Self {
        if unit.route.is_some() || unit.kind == UnitKind::Structure {
            Self::Fixed
        } else if unit.resource.eq_ignore_ascii_case(BARRAGE_ZONE) {
            Self::Barrage
        } else if unit.class & class::SHIP != 0 {
            Self::Ship
        } else if unit.class & (class::SAM | class::AAA) != 0 {
            Self::Defense
        } else if unit.class & class::STRUCTURE != 0 {
            // An NT structure (the GCI radar site) stays like a building.
            Self::Fixed
        } else {
            Self::Vehicle
        }
    }
    /// Position disc, feet, and heading spread, degrees.
    fn jitter(self) -> (i64, i32) {
        match self {
            Self::Fixed => (0, 0),
            Self::Defense => (DEFENSE_JITTER_FT, DEFENSE_JITTER_DEG),
            Self::Barrage => (BARRAGE_JITTER_FT, 0),
            Self::Vehicle => (VEHICLE_JITTER_FT, VEHICLE_JITTER_DEG),
            Self::Ship => (SHIP_JITTER_FT, SHIP_JITTER_DEG),
        }
    }
}

/// One template object (a unit or a parked aircraft) as the site rules see
/// it.
#[derive(Clone, Debug)]
pub(super) struct Object {
    pub ordinal: u32,
    pub item: Item,
    /// Where the template puts it, x and z feet.
    pub retail: [i64; 2],
    pub heading: i32,
    pub rule: Rule,
    /// Surface class is independent of motion: a carrier and its deck aircraft
    /// may be fixed within a group but still need water when that group moves.
    pub water: bool,
    pub info: TypeInfo,
    pub target: bool,
    pub routed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Item {
    Unit(usize),
    Parked(usize),
}

impl Object {
    /// Targets and fixed objects must pass a candidate site as they are;
    /// the others may move locally.
    fn must_pass(&self) -> bool {
        self.target || self.rule == Rule::Fixed
    }
}

/// The template's objects, ascending ordinal.
pub(super) fn objects(surface: &Surface, types: &mut dyn FnMut(&str) -> TypeInfo) -> Vec<Object> {
    let mut out = Vec::new();
    for (i, unit) in surface.units.iter().enumerate() {
        if unit.id.range() != IdRange::Template {
            continue;
        }
        out.push(Object {
            ordinal: super::ordinal_of(unit),
            item: Item::Unit(i),
            retail: xz(unit.position),
            heading: unit.angles[0],
            // A carrier with aircraft on its deck carries them: it stays
            // where they stand (their deck spots are measured from it).
            rule: if surface.parked.iter().any(|p| p.deck == Some(unit.id)) {
                Rule::Fixed
            } else {
                Rule::of(unit)
            },
            water: unit.class & class::SHIP != 0,
            info: types(&unit.resource),
            target: unit.is_target(),
            routed: unit.route.is_some(),
        });
    }
    for (i, parked) in surface.parked.iter().enumerate() {
        out.push(Object {
            ordinal: parked.id.0 - crate::surface::SURFACE_UNIT_BASE,
            item: Item::Parked(i),
            retail: xz(parked.position),
            heading: parked.angles[0],
            rule: Rule::Fixed,
            water: parked.deck.is_some(),
            info: types(&parked.resource),
            target: parked.target,
            routed: false,
        });
    }
    out.sort_by_key(|o| o.ordinal);
    out
}

/// Why the template stays at its retail spot, if it does. A rule over its
/// contents, tested in this order.
pub(super) fn anchor(objects: &[Object], ground: &Ground<'_>) -> Option<Anchor> {
    if objects.iter().any(|o| o.routed) {
        return Some(Anchor::Route);
    }
    if objects.iter().any(|o| o.info.strip) {
        return Some(Anchor::Strip);
    }
    if let Some(c) = centroid(objects.iter().map(|o| o.retail))
        && ground.near_runway(c, ANCHOR_RUNWAY_NM * NM_FT)
    {
        return Some(Anchor::Runway);
    }
    if objects.iter().any(|o| o.info.bridge_or_road) {
        return Some(Anchor::BridgeOrRoad);
    }
    // Runways are theater layout objects too (`~QPGSRUN`'s target stands
    // 1,000 ft from a dirt strip).
    if objects.iter().filter(|o| o.target).any(|o| {
        ground.near_object(o.retail, ANCHOR_TOWN_FT) || ground.near_runway(o.retail, ANCHOR_TOWN_FT)
    }) {
        return Some(Anchor::Town);
    }
    None
}

/// The pivot of the group's rotation and the point the starts follow: the
/// targets' centroid, or every object's when none is a target.
pub(super) fn pivot(objects: &[Object]) -> Option<[i64; 2]> {
    centroid(objects.iter().filter(|o| o.target).map(|o| o.retail))
        .or_else(|| centroid(objects.iter().map(|o| o.retail)))
}

/// Where `point` goes under `transform`.
pub(super) fn apply(transform: &GroupTransform, point: [i64; 2]) -> [i64; 2] {
    if transform.is_identity() {
        return point;
    }
    let pivot = transform.pivot.map(i64::from);
    let turned = trig::rotate(
        [point[0] - pivot[0], point[1] - pivot[1]],
        transform.rotation_deg,
    );
    [
        pivot[0] + turned[0] + i64::from(transform.translation[0]),
        pivot[1] + turned[1] + i64::from(transform.translation[1]),
    ]
}

/// Whether `object` may stand at `at` on a relocated site (the site rules,
/// stricter than jitter's): two cells inside the map, land (or water, open
/// sea where the retail spot was) and level, clear of the theater's objects
/// and runways.
fn site_ok(ground: &Ground<'_>, object: &Object, at: [i64; 2], open_sea: bool) -> bool {
    if !ground.inside(at, RELOCATE_EDGE_CELLS * CELL_FT) {
        return false;
    }
    // Fixed land structures retain the coarse site rule. Vehicle footing is
    // checked over its footprint, then repaired after airport replacement.
    if !object.water && object.rule == Rule::Fixed && !ground.level(at, LEVEL_UNITS) {
        return false;
    }
    let empty = Obstacles {
        fixed: &[],
        strips: &[],
    };
    if !stands(
        ground,
        &empty,
        at,
        object.info.footprint.radius(),
        object.water,
    ) || (object.water && open_sea && !ground.open_sea(at))
    {
        return false;
    }
    !(ground.near_object(at, RELOCATE_OBJECT_CLEARANCE_FT)
        || ground.near_runway(at, RELOCATE_RUNWAY_CLEARANCE_NM * NM_FT))
}

/// A relocated site: the transform and the local moves of the objects that
/// needed one, by ordinal.
pub(super) struct Site {
    pub transform: GroupTransform,
    pub nudges: BTreeMap<u32, [i64; 2]>,
}

/// The first of [`RELOCATE_CANDIDATES`] seeded sites where every object
/// passes the site rules, or `None` (the retail spot stays).
pub(super) fn relocate(
    objects: &[Object],
    ground: &Ground<'_>,
    seed: u32,
    stem: &str,
    pivot: [i64; 2],
) -> Option<Site> {
    let open_sea: Vec<bool> = objects
        .iter()
        .map(|o| o.water && ground.open_sea(o.retail))
        .collect();
    let mut draws = Stream::new(seed, stem, SITE_ORDINAL, Purpose::Group);
    for k in 0..RELOCATE_CANDIDATES {
        let (shift, _) = ring(
            &mut draws,
            [RELOCATE_MIN_NM * NM_FT, RELOCATE_MAX_NM * NM_FT],
        );
        let rotation = draws.below(360) as i32;
        let moved = [pivot[0] + shift[0], pivot[1] + shift[1]];
        if let Some(front) = &ground.front
            && !front.same_band(moved, pivot, RELOCATE_DEPTH_NM * NM_FT)
        {
            continue;
        }
        let (Ok(px), Ok(pz), Ok(tx), Ok(tz)) = (
            i32::try_from(pivot[0]),
            i32::try_from(pivot[1]),
            i32::try_from(shift[0]),
            i32::try_from(shift[1]),
        ) else {
            continue;
        };
        let transform = GroupTransform {
            rotation_deg: rotation,
            translation: [tx, tz],
            pivot: [px, pz],
        };
        let mut nudges = BTreeMap::new();
        let passed = objects.iter().zip(&open_sea).all(|(object, &sea)| {
            let at = apply(&transform, object.retail);
            if site_ok(ground, object, at, sea) {
                return true;
            }
            if object.must_pass() {
                return false;
            }
            // Local tries: their own stream per candidate, so an object's
            // tries never depend on which other objects were tested.
            let mut tries = Stream::new(
                seed ^ k.wrapping_mul(0x9E37_79B9),
                stem,
                object.ordinal,
                Purpose::Group,
            );
            for _ in 0..LOCAL_TRIES {
                let d = disc(&mut tries, LOCAL_TRY_FT);
                if site_ok(ground, object, [at[0] + d[0], at[1] + d[1]], sea) {
                    nudges.insert(object.ordinal, d);
                    return true;
                }
            }
            false
        });
        if passed {
            return Some(Site { transform, nudges });
        }
    }
    None
}

/// What placing the template leaves for the batteries and trucks: its
/// fixed objects and strips as obstacles.
pub(super) struct TemplateObstacles {
    pub fixed: Vec<Placed>,
    pub strips: Vec<Placed>,
}

/// Anchors or relocates the template, then jitters its units; parked
/// aircraft move with the group. Writes positions, headings, the transform
/// and the anchor into `surface`.
pub(super) fn place_template(
    surface: &mut Surface,
    inputs: &mut Inputs<'_, '_>,
) -> TemplateObstacles {
    let ground = inputs.ground;
    let objects = objects(surface, &mut *inputs.types);
    let site = surface.template.as_ref().expect("a template site");
    let (seed, stem, variation) = (
        site.settings.seed,
        site.stem.clone(),
        site.settings.variation,
    );
    let anchored = anchor(&objects, ground);
    let mut transform = GroupTransform::IDENTITY;
    let mut nudges = BTreeMap::new();
    if anchored.is_none()
        && variation.relocate
        && let Some(pivot) = pivot(&objects)
        && let Some(found) = relocate(&objects, ground, seed, &stem, pivot)
    {
        transform = found.transform;
        nudges = found.nudges;
    }
    if let Some(site) = surface.template.as_mut() {
        site.anchor = anchored;
    }
    surface.transform = transform;
    let world = |object: &Object| {
        let at = apply(&transform, object.retail);
        let d = nudges.get(&object.ordinal).copied().unwrap_or([0, 0]);
        Pose {
            at: [at[0] + d[0], at[1] + d[1]],
            heading: object.heading + transform.rotation_deg,
        }
    };
    // The fixed objects stand where the group puts them; template strips
    // are runways to the units placed beside them.
    let mut fixed = Vec::new();
    let mut strips = Vec::new();
    for object in objects.iter().filter(|o| o.rule == Rule::Fixed) {
        let pose = world(object);
        let placed = Placed {
            id: object.ordinal,
            at: pose.at,
            heading: pose.heading,
            footprint: object.info.footprint,
        };
        if object.info.strip {
            strips.push(placed);
        } else {
            fixed.push(placed);
        }
    }
    let obstacles = Obstacles {
        fixed: &fixed,
        strips: &strips,
    };
    // A fleet in a template that stays moves as one before each ship's
    // own jitter.
    let fleet = if variation.jitter && transform.is_identity() {
        fleet(&objects, ground, &obstacles, seed, &stem)
    } else {
        None
    };
    // Reserve each object's fallback as well as each accepted pose. A later
    // unit may exhaust its jitter search, so an earlier unit must not take
    // its fallback spot. Authored overlaps remain explicit audit findings.
    let bases: Vec<Pose> = objects
        .iter()
        .map(|object| {
            let base = world(object);
            if object.rule == Rule::Ship {
                fleet.as_ref().map_or(base, |fleet| fleet.apply(base))
            } else {
                base
            }
        })
        .collect();
    let mut occupied: Vec<([i64; 2], i64)> = objects
        .iter()
        .zip(&bases)
        .map(|(object, pose)| (pose.at, object.info.footprint.radius()))
        .collect();
    let mut poses = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        let base = bases[index];
        let others: Vec<_> = occupied
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                *i != index && !matches!(objects[*i].rule, Rule::Fixed | Rule::Barrage)
            })
            .map(|(_, p)| *p)
            .collect();
        let pose = if variation.jitter && object.rule != Rule::Fixed {
            jitter(
                object, base, &transform, &nudges, ground, &obstacles, &others, seed, &stem,
            )
        } else {
            base
        };
        occupied[index].0 = pose.at;
        poses.push((object.item, pose));
    }
    for (item, pose) in poses {
        match item {
            Item::Unit(i) => set_pose(&mut surface.units[i], pose),
            Item::Parked(i) => {
                let parked = &mut surface.parked[i];
                parked.position[0] = i32::try_from(pose.at[0]).unwrap_or(parked.position[0]);
                parked.position[2] = i32::try_from(pose.at[1]).unwrap_or(parked.position[2]);
                parked.angles[0] = trig::signed(pose.heading);
            }
        }
    }
    TemplateObstacles { fixed, strips }
}

/// Up to [`JITTER_CANDIDATES`] seeded spots for one object, in the
/// template's own frame and then through the group's move; the first that
/// stands wins, else `base`.
#[allow(clippy::too_many_arguments)]
fn jitter(
    object: &Object,
    base: Pose,
    transform: &GroupTransform,
    nudges: &BTreeMap<u32, [i64; 2]>,
    ground: &Ground<'_>,
    obstacles: &Obstacles<'_>,
    occupied: &[([i64; 2], i64)],
    seed: u32,
    stem: &str,
) -> Pose {
    let (radius, turn) = object.rule.jitter();
    let mut offsets = Stream::new(seed, stem, object.ordinal, Purpose::Offset);
    let mut headings = Stream::new(seed, stem, object.ordinal, Purpose::Heading);
    let size = object.info.footprint.radius();
    let ship = object.rule == Rule::Ship;
    let nudge = nudges.get(&object.ordinal).copied().unwrap_or([0, 0]);
    // The ship's base already carries the fleet's move: jitter about it.
    let fleet_shift = if ship {
        let plain = apply(transform, object.retail);
        [
            base.at[0] - plain[0] - nudge[0],
            base.at[1] - plain[1] - nudge[1],
        ]
    } else {
        [0, 0]
    };
    for _ in 0..JITTER_CANDIDATES {
        let d = disc(&mut offsets, radius);
        let dh = if turn > 0 {
            spread(&mut headings, turn)
        } else {
            0
        };
        let local = [object.retail[0] + d[0], object.retail[1] + d[1]];
        let moved = apply(transform, local);
        let at = [
            moved[0] + nudge[0] + fleet_shift[0],
            moved[1] + nudge[1] + fleet_shift[1],
        ];
        if clear_of_units(at, size, occupied) && stands(ground, obstacles, at, size, ship) {
            return Pose {
                at,
                heading: base.heading + dh,
            };
        }
    }
    base
}

/// A fleet's common move: a shift and a turn about its ships' centroid.
struct Fleet {
    centre: [i64; 2],
    shift: [i64; 2],
    turn: i32,
}

impl Fleet {
    fn apply(&self, pose: Pose) -> Pose {
        let turned = trig::rotate(
            [pose.at[0] - self.centre[0], pose.at[1] - self.centre[1]],
            self.turn,
        );
        Pose {
            at: [
                self.centre[0] + turned[0] + self.shift[0],
                self.centre[1] + turned[1] + self.shift[1],
            ],
            heading: pose.heading + self.turn,
        }
    }
}

/// The fleet move of a template that stays: up to [`FLEET_CANDIDATES`]
/// seeded shifts and turns; the first that keeps every ship's footprint
/// on the map, on water and clear of fixed objects wins. `None` without ships or when none fits.
fn fleet(
    objects: &[Object],
    ground: &Ground<'_>,
    obstacles: &Obstacles<'_>,
    seed: u32,
    stem: &str,
) -> Option<Fleet> {
    let ships: Vec<&Object> = objects.iter().filter(|o| o.rule == Rule::Ship).collect();
    let centre = centroid(ships.iter().map(|o| o.retail))?;
    let mut draws = Stream::new(seed, stem, FLEET_ORDINAL, Purpose::Offset);
    for _ in 0..FLEET_CANDIDATES {
        let shift = disc(&mut draws, FLEET_SHIFT_FT);
        let turn = spread(&mut draws, FLEET_TURN_DEG);
        let fleet = Fleet {
            centre,
            shift,
            turn,
        };
        if ships.iter().all(|ship| {
            let at = fleet.apply(Pose {
                at: ship.retail,
                heading: 0,
            });
            stands(ground, obstacles, at.at, ship.info.footprint.radius(), true)
        }) {
            return Some(fleet);
        }
    }
    None
}

/// The site rules a placed template breaks, for the relocation sweep: every
/// relocated object against [`site_ok`]'s rules (with its local move), the
/// distance and front rules on the moved pivot. Empty when it passes or did
/// not relocate.
pub fn audit(
    surface: &Surface,
    ground: &Ground<'_>,
    types: &mut dyn FnMut(&str) -> TypeInfo,
) -> Vec<String> {
    let mut problems = Vec::new();
    let transform = surface.transform;
    if transform.is_identity() {
        return problems;
    }
    let pivot = transform.pivot.map(i64::from);
    let shift = transform.translation.map(i64::from);
    let d2 = shift[0] * shift[0] + shift[1] * shift[1];
    let (lo, hi) = (RELOCATE_MIN_NM * NM_FT, RELOCATE_MAX_NM * NM_FT);
    if d2 < lo * lo || d2 > hi * hi {
        problems.push(format!(
            "moved {} ft, outside {lo} to {hi}",
            trig::isqrt(d2 as u128)
        ));
    }
    let moved = [pivot[0] + shift[0], pivot[1] + shift[1]];
    if let Some(front) = &ground.front
        && !front.same_band(moved, pivot, RELOCATE_DEPTH_NM * NM_FT)
    {
        problems.push("left its depth band along the front".into());
    }
    let jitter_slack = |rule: Rule| match rule {
        Rule::Fixed => 0,
        _ => rule.jitter().0 + FLEET_SHIFT_FT + LOCAL_TRY_FT,
    };
    for unit in surface.template_units() {
        let rule = Rule::of(unit);
        let at = xz(unit.position);
        let info = types(&unit.resource);
        let size = info.footprint.radius();
        let ship = unit.class & class::SHIP != 0;
        let level = if ship || unit.resource == BARRAGE_ZONE {
            true
        } else if footing::vehicle(unit) {
            footing::slope_ok(
                ground,
                Placed {
                    id: unit.id.0,
                    at,
                    heading: unit.angles[0],
                    footprint: info.footprint,
                },
                footing::slope_limit(unit),
            )
        } else {
            ground.level(at, LEVEL_UNITS)
        };
        let fails = if !ground.inside(at, RELOCATE_EDGE_CELLS * CELL_FT - jitter_slack(rule)) {
            Some("off the map edge")
        } else if ground.water(at) != ship {
            Some(if ship {
                "a ship on land"
            } else {
                "a land unit on water"
            })
        } else if !level {
            Some("on a slope")
        } else if ground.near_runway(at, RUNWAY_CLEARANCE_FT + size) {
            Some("on a runway")
        } else if rule == Rule::Fixed && ground.near_object(at, RELOCATE_OBJECT_CLEARANCE_FT) {
            Some("on a theater object")
        } else if ground.near_object(at, size) {
            Some("inside a theater object")
        } else {
            None
        };
        if let Some(why) = fails {
            problems.push(format!("{:#010x} {} {why}", unit.id.0, unit.resource));
        }
    }
    problems
}

/// The terrain class rule over everything the layout placed, moved or not:
/// a land unit, structure or parked aircraft stands on a land cell
/// (docs/spec/surface-defenses.md, "Validity"), a ship on a water cell, each
/// at its centre ([`Ground::water`]). `retail` is the same template placed
/// without jitter or relocation: a unit still where retail put it is the
/// data's and is not judged (a harbor's ship may sit on a shore cell of the
/// coarse grid); a unit the layout moved, a supply truck or a battery radar
/// it added must pass. Theater layout objects never move and are not judged.
/// Aircraft on a deck stand where their carrier does. Empty when the rule
/// holds.
pub fn class_audit(
    surface: &Surface,
    ground: &Ground<'_>,
    retail: Option<&Surface>,
) -> Vec<String> {
    let mut problems = Vec::new();
    let retail_at: BTreeMap<u32, [i64; 2]> = retail
        .map(|r| {
            r.units
                .iter()
                .map(|u| (u.id.0, xz(u.position)))
                .chain(r.parked.iter().map(|p| (p.id.0, xz(p.position))))
                .collect()
        })
        .unwrap_or_default();
    let moved = |id: u32, at: [i64; 2]| retail_at.get(&id) != Some(&at);
    for unit in &surface.units {
        let at = xz(unit.position);
        if unit.id.range() == IdRange::Layout || !moved(unit.id.0, at) {
            continue;
        }
        let ship = unit.class & class::SHIP != 0;
        if ground.water(at) != ship {
            problems.push(format!(
                "{:#010x} {} {} at {} {}",
                unit.id.0,
                unit.resource,
                if ship {
                    "a ship on land"
                } else {
                    "a land unit on water"
                },
                at[0],
                at[1]
            ));
        }
    }
    for parked in surface.parked.iter().filter(|p| p.deck.is_none()) {
        let at = xz(parked.position);
        if moved(parked.id.0, at) && ground.water(at) {
            problems.push(format!(
                "{:#010x} {} a parked aircraft on water at {} {}",
                parked.id.0, parked.resource, at[0], at[1]
            ));
        }
    }
    problems
}
