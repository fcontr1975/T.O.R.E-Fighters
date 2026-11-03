//! Layout: where the resolved surface stands (docs/spec/surface-defenses.md,
//! "Jitter", "Relocation", "Start placement", "SAM batteries", "Resupply").
//!
//! After resolution has decided which units a mission has, this places them:
//!
//! - **Relocation.** A ground target template not tied to a fixed theater
//!   feature moves as a rigid group, a whole-degree rotation about its
//!   targets' centroid and a translation, to a site that passes the site
//!   rules ([`Anchor`] says why one stays).
//! - **Jitter.** Defenses, vehicles, barrage zones and ships move a seeded
//!   amount inside the template's own frame, so no two flights look alike.
//! - **Batteries.** SA-2, SA-3, SA-6 and HAWK launchers are clustered into
//!   batteries around a search radar: an existing one adopted, or a radar
//!   element added.
//! - **Supply trucks.** Each manned `<sam>` and `<aaa>` slot and each
//!   template battery gets a truck.
//! - **Starts.** Red starts by the target, Blue the mission's separation
//!   from Red heading at it; the airfields a ground start may use follow
//!   the target.
//! - **Front line.** A few Blue and Red ground units stand in contact a
//!   few miles from the targets, so a surface battle is visible.
//! - **Area defenses.** With no ground target, the creator's AAA and SAM
//!   strengths scatter enemy SAM sites and guns over Red's side of the
//!   front ([`area`]).
//!
//! Everything comes from the mission's surface seed through
//! [`super::resolve::Stream`] (one stream per template, object and purpose)
//! and integer arithmetic on the retail integer data ([`trig`], [`ground`]),
//! so every machine places every unit on the same foot. Object sizes come
//! from the placed scale (`terrain::placed_shape_scale`) through
//! [`TypeInfo`]; nothing here assumes one.
pub mod area;
mod batteries;
mod final_starts;
pub mod footing;
mod front;
pub use final_starts::place as scene_starts;
pub mod ground;
mod sites;
mod starts;
mod supply_trucks;
pub mod trig;

pub use ground::{Cells, Footprint, Front, Ground, Placed, TypeInfo};
pub use sites::{audit, class_audit};

use super::{
    Origin, Surface, Unit, UnitId,
    catalog::Entry,
    resolve::{Purpose, Stream},
};
use std::collections::BTreeMap;
use std::sync::Arc;
use tore_formats::mission::{Placement, SourceKey};

/// Feet per nautical mile, whole (the spec's 1 nm = 6,076 ft).
pub const NM_FT: i64 = 6_076;

// Jitter (docs/spec/surface-defenses.md, "Jitter"): fitted, agent.
/// SAM and AAA: position disc radius, feet, and heading spread, degrees.
pub const DEFENSE_JITTER_FT: i64 = 1_500;
pub const DEFENSE_JITTER_DEG: i32 = 45;
/// The invisible barrage zone `A_M1939`: position disc, feet; no heading.
pub const BARRAGE_JITTER_FT: i64 = 2_000;
/// Tanks, AFVs, vehicles and troops without a route.
pub const VEHICLE_JITTER_FT: i64 = 600;
pub const VEHICLE_JITTER_DEG: i32 = 30;
/// A fleet in an anchored template: one common shift and a formation
/// rotation about its centroid.
pub const FLEET_SHIFT_FT: i64 = 6_000;
pub const FLEET_TURN_DEG: i32 = 20;
/// Bounded search for a valid fleet move within the existing shift/turn limits.
pub const FLEET_CANDIDATES: usize = 128;
/// Each ship after the fleet's move.
pub const SHIP_JITTER_FT: i64 = 300;
pub const SHIP_JITTER_DEG: i32 = 10;
/// Candidates per jittered object; the first that stands wins, else the
/// retail (or relocated) spot.
pub const JITTER_CANDIDATES: usize = 8;
/// Initial bounded ring search for support. Final footprint placement repairs
/// an invalid fallback after airport replacement without removing equipment.
pub const SUPPORT_CANDIDATES: usize = 128;
/// A placed unit keeps this far, plus its own radius, from every runway.
pub const RUNWAY_CLEARANCE_FT: i64 = 300;
/// Land units stand where the four corner elevations span at most this many
/// elevation units (256 ft each).
pub const LEVEL_UNITS: u8 = 1;

// Relocation (docs/spec/surface-defenses.md, "Relocation"): fitted, agent;
// the radius and depth numbers are "default, pending John" (decision 12.1).
/// Distance of the moved targets' centroid from the retail spot, nm.
pub const RELOCATE_MIN_NM: i64 = 3;
pub const RELOCATE_MAX_NM: i64 = 30;
/// Candidate sites per template; the first that passes wins.
pub const RELOCATE_CANDIDATES: u32 = 32;
/// Every relocated object stays this many grid cells inside the map.
pub const RELOCATE_EDGE_CELLS: i64 = 2;
/// Clearance of every relocated object from the theater layout's objects,
/// feet, and from its runways, nm.
pub const RELOCATE_OBJECT_CLEARANCE_FT: i64 = 2_000;
pub const RELOCATE_RUNWAY_CLEARANCE_NM: i64 = 3;
/// The moved centroid keeps within this depth of the retail spot along the
/// front, nm, on the same side of its midpoint.
pub const RELOCATE_DEPTH_NM: i64 = 15;
/// A defense, vehicle or truck that fails at its moved spot gets this many
/// tries within [`LOCAL_TRY_FT`] before the candidate is rejected.
pub const LOCAL_TRIES: usize = 8;
pub const LOCAL_TRY_FT: i64 = 2_000;

// Anchoring (docs/spec/surface-defenses.md, "Relocation"): a rule over the
// template's contents.
/// A template whose centroid is this close to a theater runway stays, nm.
pub const ANCHOR_RUNWAY_NM: i64 = 1;
/// A template with a target this close to a theater layout object stays.
pub const ANCHOR_TOWN_FT: i64 = 2_000;

// Starts (docs/spec/surface-defenses.md, "Start placement"): John's rule,
// 2026-10-10 (request item 7): Red within 5 nm of the target, defending it;
// Blue the mission's separation from Red. The spread and the airfield
// distance are "default, pending John" (decisions 12.1 and 12.6).
/// Red's start: within this of the targets' centroid, nm.
pub const RED_START_NM: i64 = 5;
/// Blue's start: spread off the bearing toward Blue's side, degrees.
pub const BLUE_START_SPREAD_DEG: i32 = 30;
/// A start stays this many grid cells inside the map (the enemy placement
/// rule's margin, `mission_layout::MAP_MARGIN_CELLS`).
pub const START_MARGIN_CELLS: i64 = 1;
/// A ground start's airfield is at least this far from the target, nm.
pub const AIRFIELD_MIN_NM: i64 = 15;

// Batteries (docs/spec/surface-defenses.md, "SAM batteries"): "default,
// pending John" (decision 12.8).
/// Launchers of one system and side within this of each other, feet, form
/// one battery (single linkage).
pub const BATTERY_CLUSTER_FT: i64 = NM_FT;
/// An existing radar of the system's element type this close to the
/// battery's centroid is adopted, feet: 3.5 nm (lead's call, 2026-10-10,
/// so retail North Vietnam's GCIs pair with their SA-2 sites; the plan's
/// first default was 2 nm).
pub const BATTERY_ADOPT_FT: i64 = 7 * NM_FT / 2;
/// An added radar stands this far from the launchers' centroid, feet; an
/// SA-2 site's outside its six-rail ring.
pub const BATTERY_RADAR_FT: [i64; 2] = [600, 1_000];
pub const SA2_RADAR_FT: [i64; 2] = [1_000, 1_500];

// Supply trucks (docs/spec/surface-defenses.md, "Resupply"): "default,
// pending John" (decision 12.5).
/// An added truck stands this far from the unit it serves, feet.
pub const TRUCK_FT: [i64; 2] = [200, 400];
/// The truck a manned `<sam>` slot or a battery gets, and the one a manned
/// `<aaa>` slot gets.
pub const SAM_TRUCK: &str = "MISTRK.NT";
pub const AAA_TRUCK: &str = "TRUCK.NT";

/// The invisible barrage-zone unit (retail `A_M1939.NT`).
pub const BARRAGE_ZONE: &str = "A_M1939.NT";

// Front line (docs/spec/surface-defenses.md, "The front line"): John,
// 2026-10-11 asked for opposing ground forces in contact; the numbers are
// fitted, agent.
/// The front's centre stands this far from the targets' centroid, nm: the
/// first band, then the second when no site in the first passes.
pub const FRONT_BANDS_NM: [[i64; 2]; 2] = [[5, 10], [10, 20]];
/// Every template unit, every leg of a routed unit's path, both airborne
/// starts and every ground-start runway are at least this far from the
/// front's centre, nm, so no front unit reaches the objective and no start
/// sits in the fighting.
pub const FRONT_CLEAR_NM: i64 = 4;
/// Candidate centres per band.
pub const FRONT_CANDIDATES: usize = 24;
/// The two lines face each other this far apart, feet: inside a tank gun's
/// reach, with room for the 3,000 ft of light-gun accuracy.
pub const FRONT_GAP_FT: i64 = 2_400;
/// Units of one line stand this far apart, feet.
pub const FRONT_SPACING_FT: i64 = 500;
/// Each front unit moves a seeded amount inside this disc, feet, and turns
/// by up to [`FRONT_TURN_DEG`].
pub const FRONT_JITTER_FT: i64 = 150;
pub const FRONT_TURN_DEG: i32 = 15;
/// The height above the ground the lines' sight is measured at, feet: the
/// surface weapons' own eye height.
pub const FRONT_EYE_FT: i64 = 10;
/// The equipment group Blue's front draws from: the Western list (group 0).
pub const FRONT_BLUE_GROUP: usize = 0;

// Area defenses (docs/spec/surface-defenses.md, "Area defenses"): John,
// 2026-10-12 (B23) asked for enemy AAA and SAMs with no objective, on Red's
// side of the front 10 to 60 nm deep with extra cover near Red airfields,
// counted from the creator's strengths; the numbers are fitted, agent.
/// Depth of a band site past the front's midpoint toward Red, nm.
pub const AREA_DEPTH_NM: [i64; 2] = [10, 60];
/// When the band holds no place for a site (an island theater's band is
/// mostly sea), it stands this deep instead, nm.
pub const AREA_FALLBACK_DEPTH_NM: [i64; 2] = [10, 120];
/// Red airfields this deep or less, nm, get the airfield cover.
pub const AREA_AIRFIELD_DEPTH_NM: i64 = 100;
/// Site slots of each kind, like the ten `<sam>` and ten `<aaa>` slots of
/// 79 of the 95 templates; each is manned with the strength's chance (0, 25,
/// 60 or 100 percent), so a heavy mission stands all ten sites of a kind
/// and a light one a quarter of them on average (nr-b23b, 2026-10-13; the
/// count is fitted, agent).
pub const AREA_SLOTS: usize = 10;
/// The first build's fixed SAM sites and AAA sites by strength (none,
/// light, moderate, heavy), kept for a recording made then.
pub const AREA_SAM_SITES: [usize; 4] = [0, 2, 4, 6];
pub const AREA_AAA_SITES: [usize; 4] = [0, 4, 8, 12];
/// Launchers or guns per site, all of one type.
pub const AREA_SITE_UNITS: usize = 2;
/// Every this many sites of a kind, the first included, one guards a Red
/// airfield instead of standing in the band.
pub const AREA_AIRFIELD_EVERY: usize = 3;
/// An airfield site stands this far from its runway's origin, nm.
pub const AREA_AIRFIELD_NM: [i64; 2] = [2, 5];
/// Sites keep this far apart, nm.
pub const AREA_SITE_SPACING_NM: i64 = 3;
/// Candidate centres per site; the first that holds every unit wins.
pub const AREA_CANDIDATES: usize = 96;
/// A site's units stand in this ring around its centre, feet, at least
/// [`AREA_MEMBER_GAP_FT`] apart beyond their footprints.
pub const AREA_MEMBER_FT: [i64; 2] = [150, 600];
pub const AREA_MEMBER_GAP_FT: i64 = 150;
/// Every area unit's centre keeps this far from every layout object
/// (towns, buildings, airfield objects), feet. At most the ground index's
/// reach, so the test stays a lookup.
pub const AREA_OBJECT_CLEARANCE_FT: i64 = 2_000;
/// And this far from every runway, feet (1 nm: past the aprons and
/// taxiways the redrawn airports add).
pub const AREA_RUNWAY_CLEARANCE_FT: i64 = NM_FT;
/// Every site stays this many grid cells inside the map.
pub const AREA_EDGE_CELLS: i64 = 2;

/// Which of the seeded variations a layout applies. All are on for every
/// mission; the preview tools turn them off to show the retail spot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variation {
    pub jitter: bool,
    pub relocate: bool,
    /// The Quick Mission front line: Blue and Red ground units in contact.
    pub front: bool,
}

impl Variation {
    pub const ON: Self = Self {
        jitter: true,
        relocate: true,
        front: true,
    };
    pub const OFF: Self = Self {
        jitter: false,
        relocate: false,
        front: false,
    };
}

impl Default for Variation {
    fn default() -> Self {
        Self::ON
    }
}

/// Why a template stays at its retail spot and only jitters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// One of its objects has a route: routes follow roads.
    Route,
    /// It brings its own runway or strip piece.
    Strip,
    /// Its centroid is within [`ANCHOR_RUNWAY_NM`] of a theater runway.
    Runway,
    /// It contains a bridge or road piece.
    BridgeOrRoad,
    /// A target stands within [`ANCHOR_TOWN_FT`] of a theater layout
    /// object: built into a town, harbor or base.
    Town,
}

impl Anchor {
    pub fn name(self) -> &'static str {
        match self {
            Self::Route => "route",
            Self::Strip => "strip",
            Self::Runway => "runway",
            Self::BridgeOrRoad => "bridge-or-road",
            Self::Town => "town",
        }
    }
}

/// What the layout reads besides the surface.
pub struct Inputs<'g, 'a> {
    pub ground: &'g Ground<'a>,
    /// The footprint and kind of a placed type, by resource.
    pub types: &'g mut dyn FnMut(&str) -> TypeInfo,
    /// The record of a type the layout adds (a supply truck or a radar
    /// element), or `None` when the import cannot place it.
    pub added: &'g mut dyn FnMut(&str) -> Option<Arc<Entry>>,
    /// The theater layout's resource (`CUB.MM`): the key of its batteries'
    /// streams.
    pub layout: &'g str,
}

/// A placed position and heading in the world, whole feet and degrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pose {
    pub at: [i64; 2],
    pub heading: i32,
}

/// A unit the layout adds: a supply truck or a battery radar element.
pub(crate) struct Added {
    pub unit: Unit,
    /// The resource its placement belongs to: the template's or the
    /// layout's.
    pub source: String,
}

/// Places `surface` (see the module comment): moves the template's units
/// and parked aircraft, forms the batteries, adds radars and trucks and
/// places the starts. The units' positions and angles, the scene
/// placements, the sides, `transform`, `batteries`, `trucks` and `starts`
/// are filled in.
pub fn place(surface: &mut Surface, inputs: &mut Inputs<'_, '_>) {
    // Where every unit stood as resolved: the battery rules cluster
    // template launchers in the template's own frame.
    let frames: BTreeMap<UnitId, [i64; 2]> = surface
        .units
        .iter()
        .map(|unit| (unit.id, xz(unit.position)))
        .collect();
    let mut fixed = Vec::new();
    let mut strips = Vec::new();
    if surface.template.is_some() {
        let placed = sites::place_template(surface, inputs);
        fixed = placed.fixed;
        strips = placed.strips;
    }
    let obstacles = Obstacles {
        fixed: &fixed,
        strips: &strips,
    };
    let mut added = Vec::new();
    let mut notes = Vec::new();
    // Area defenses join the surface before the batteries form, so a SAM
    // site gets its radar.
    let (area, area_sams) = area::add(surface, inputs, &obstacles, &mut notes);
    commit(surface, area);
    let mut batteries =
        batteries::form(surface, inputs, &frames, &obstacles, &mut added, &mut notes);
    let trucks = supply_trucks::add(
        surface,
        inputs,
        &mut batteries,
        &area_sams,
        &obstacles,
        &mut added,
        &mut notes,
    );
    surface.batteries = batteries;
    let starts = starts::place(surface, inputs.ground);
    surface.starts = starts;
    front::add(surface, inputs, &obstacles, &mut added, &mut notes);
    finish(surface, added, trucks);
    surface.layout_notes = notes;
}

/// The template's fixed objects and strips, as obstacles for what the
/// layout places next to them.
pub(crate) struct Obstacles<'a> {
    pub fixed: &'a [Placed],
    pub strips: &'a [Placed],
}

/// Whether a unit of `radius` feet may stand at `at`: inside the grid, on
/// land (water for a ship) at its centre and the four corners of its
/// footprint, level ground for land units, clear of every theater layout
/// object and the template's fixed objects, and [`RUNWAY_CLEARANCE_FT`]
/// clear of every runway.
pub(crate) fn stands(
    ground: &Ground<'_>,
    obstacles: &Obstacles<'_>,
    at: [i64; 2],
    radius: i64,
    ship: bool,
) -> bool {
    if !ground.inside(at, 0) {
        return false;
    }
    let corners = [
        at,
        [at[0] - radius, at[1] - radius],
        [at[0] + radius, at[1] - radius],
        [at[0] - radius, at[1] + radius],
        [at[0] + radius, at[1] + radius],
    ];
    if corners
        .iter()
        .any(|c| !ground.inside(*c, 0) || ground.water(*c) != ship)
    {
        return false;
    }
    // Preliminary conservative grade check. The completed scene applies the
    // actual oriented footprint and the unit type's limit.
    if !ship
        && !footing::slope_ok(
            ground,
            Placed {
                id: 0,
                at,
                heading: 0,
                footprint: Footprint {
                    min: [-radius, -radius],
                    max: [radius, radius],
                },
            },
            10,
        )
    {
        return false;
    }
    let runway = RUNWAY_CLEARANCE_FT + radius;
    !(ground.near_object(at, radius)
        || obstacles.fixed.iter().any(|o| o.within(at, radius))
        || ground.near_runway(at, runway)
        || obstacles.strips.iter().any(|o| o.within(at, runway)))
}

/// Occupied solid unit footprints for support placement. Pavement, roads and
/// the invisible barrage controller are not bodies to avoid here.
pub(crate) fn occupied<'a>(
    units: impl Iterator<Item = &'a Unit>,
    types: &mut dyn FnMut(&str) -> TypeInfo,
) -> Vec<([i64; 2], i64)> {
    units
        .filter_map(|unit| {
            let info = types(&unit.resource);
            (!info.strip && !info.bridge_or_road && unit.resource != BARRAGE_ZONE)
                .then_some((xz(unit.position), info.footprint.radius()))
        })
        .collect()
}

pub(crate) fn clear_of_units(at: [i64; 2], radius: i64, occupied: &[([i64; 2], i64)]) -> bool {
    occupied
        .iter()
        .all(|(other, size)| distance2(at, *other) >= (radius + size).pow(2))
}

/// A point uniformly inside a disc of `radius` feet (rejection sampling).
pub(crate) fn disc(stream: &mut Stream, radius: i64) -> [i64; 2] {
    if radius <= 0 {
        return [0, 0];
    }
    let span = 2 * radius as u64 + 1;
    for _ in 0..64 {
        let x = stream.below(span) as i64 - radius;
        let z = stream.below(span) as i64 - radius;
        if x * x + z * z <= radius * radius {
            return [x, z];
        }
    }
    [0, 0]
}

/// A point uniformly (by area) in the ring between `min` and `max` feet,
/// with its bearing.
pub(crate) fn ring(stream: &mut Stream, [min, max]: [i64; 2]) -> ([i64; 2], i32) {
    let (lo, hi) = ((min * min) as u64, (max * max) as u64);
    let radius = trig::isqrt(u128::from(lo + stream.below(hi - lo + 1))) as i64;
    let bearing = stream.below(360) as i32;
    (trig::along(bearing, radius), bearing)
}

/// A whole-degree turn in `-spread..=spread`.
pub(crate) fn spread(stream: &mut Stream, spread: i32) -> i32 {
    stream.below(2 * spread as u64 + 1) as i32 - spread
}

/// The stream of a layout draw: the template's when the unit is the
/// template's, else the theater layout's with seed 0 (base layouts look the
/// same in every flight).
pub(crate) fn stream_for(
    surface: &Surface,
    layout: &str,
    template_unit: bool,
    ordinal: u32,
    purpose: Purpose,
) -> Stream {
    match (&surface.template, template_unit) {
        (Some(site), true) => Stream::new(site.settings.seed, &site.stem, ordinal, purpose),
        _ => Stream::new(0, layout, ordinal, purpose),
    }
}

/// The ordinal behind a unit id, for its streams.
pub(crate) fn ordinal_of(unit: &Unit) -> u32 {
    match unit.origin {
        Origin::Layout { ordinal } | Origin::Template { ordinal, .. } => ordinal,
        Origin::Added => unit.id.0,
    }
}

pub(crate) fn xz(position: [i32; 3]) -> [i64; 2] {
    [i64::from(position[0]), i64::from(position[2])]
}

/// The integer mean of `points`, rounded toward negative infinity.
pub fn centroid(points: impl IntoIterator<Item = [i64; 2]>) -> Option<[i64; 2]> {
    let (mut sum, mut n) = ([0i64; 2], 0i64);
    for p in points {
        sum[0] += p[0];
        sum[1] += p[1];
        n += 1;
    }
    (n > 0).then(|| [sum[0].div_euclid(n), sum[1].div_euclid(n)])
}

pub(crate) fn distance2(a: [i64; 2], b: [i64; 2]) -> i64 {
    (a[0] - b[0]).pow(2) + (a[1] - b[1]).pow(2)
}

/// Sets a unit's world position (keeping its height word) and heading.
pub(crate) fn set_pose(unit: &mut Unit, pose: Pose) {
    unit.position[0] = i32::try_from(pose.at[0]).unwrap_or(unit.position[0]);
    unit.position[2] = i32::try_from(pose.at[1]).unwrap_or(unit.position[2]);
    unit.angles[0] = trig::signed(pose.heading);
}

/// A unit the layout adds, from its record, standing at `pose` with
/// `owner`'s nationality, side and skill.
pub(crate) fn added_unit(id: UnitId, entry: &Entry, pose: Pose, owner: &Unit) -> Unit {
    added_unit_for(id, entry, pose, owner.nationality, owner.side, owner.skill)
}

/// A unit the layout adds, from its record, standing at `pose` for the
/// given nationality, side and skill.
pub(crate) fn added_unit_for(
    id: UnitId,
    entry: &Entry,
    pose: Pose,
    nationality: Option<i32>,
    side: tore_sim::combat::live::Side,
    skill: i32,
) -> Unit {
    let kind = match entry.family {
        super::catalog::Family::Unit(kind) => kind,
        _ => super::UnitKind::Structure,
    };
    let mut unit = Unit {
        id,
        origin: Origin::Added,
        resource: entry.resource.clone(),
        kind,
        class: entry.class,
        name: entry.name.clone(),
        nationality,
        side,
        position: [0; 3],
        angles: [0; 3],
        // Never a destroy target.
        flags: 0,
        skill,
        react: None,
        search_dist: None,
        start_time: None,
        route: None,
        hit_points: entry.hit_points,
        look: entry.look.clone(),
        explosion: entry.explosion,
        crater: entry.crater,
        supply_truck: entry.supply_truck,
        in_scene: false,
    };
    set_pose(&mut unit, pose);
    unit
}

/// Writes the placed units back: the template placements follow their
/// units, the added units join the units, the placements and the sides,
/// and the trucks list is rebuilt in id order.
fn finish(surface: &mut Surface, added: Vec<Added>, trucks: Vec<super::SupplyTruck>) {
    for (id, placement) in &mut surface.placements {
        if let Some(unit) = surface
            .units
            .binary_search_by_key(id, |u| u.id)
            .ok()
            .map(|at| &surface.units[at])
        {
            placement.position = unit.position;
            placement.angles = unit.angles;
        }
    }
    commit(surface, added);
    let mut all: Vec<super::SupplyTruck> = surface
        .trucks
        .iter()
        .filter(|t| !t.added)
        .cloned()
        .chain(trucks)
        .collect();
    all.sort_by_key(|t| t.id);
    surface.trucks = all;
}

/// The added units join the units, the placements and the sides, all kept
/// in id order.
fn commit(surface: &mut Surface, added: Vec<Added>) {
    if added.is_empty() {
        return;
    }
    for Added { unit, source } in added {
        if unit.side != tore_sim::combat::live::NO_SIDE {
            surface.object_sides.insert(unit.id.0, unit.side);
        }
        let ordinal = unit.id.0;
        surface.placements.push((
            unit.id,
            Placement {
                key: SourceKey {
                    layout: source,
                    ordinal,
                },
                section: None,
                object_type: unit.resource.clone(),
                position: unit.position,
                angles: unit.angles,
                source_nationality: unit.nationality,
                nationality2: false,
                nationality3: true,
                nationality: unit.nationality,
                flags: Some(unit.flags),
                speed: Some(0),
                name: None,
                alias: None,
                unknown: Vec::new(),
            },
        ));
        surface.units.push(unit);
    }
    surface.units.sort_by_key(|unit| unit.id);
    surface.placements.sort_by_key(|(id, _)| *id);
}

/// Rebuild the front against the completed objective, before scene grounding.
pub fn finish_layout(surface: &mut Surface, inputs: &mut Inputs<'_, '_>) {
    rebuild_front(surface, inputs, true);
}

/// Recheck a repaired scene at another separation without repeating vehicle repair.
/// The caller must restore the raw front first, including its fallback equipment.
pub fn recheck_front(surface: &mut Surface, inputs: &mut Inputs<'_, '_>) {
    rebuild_front(surface, inputs, false);
}

fn rebuild_front(surface: &mut Surface, inputs: &mut Inputs<'_, '_>, repair: bool) {
    use super::IdRange;
    let old_units: Vec<_> = surface
        .units
        .iter()
        .filter(|u| u.id.range() == IdRange::FrontLine)
        .cloned()
        .collect();
    let old_placements: Vec<_> = surface
        .placements
        .iter()
        .filter(|(id, _)| id.range() == IdRange::FrontLine)
        .cloned()
        .collect();
    surface.units.retain(|u| u.id.range() != IdRange::FrontLine);
    surface
        .placements
        .retain(|(id, _)| id.range() != IdRange::FrontLine);
    surface
        .object_sides
        .retain(|id, _| UnitId(*id).range() != IdRange::FrontLine);
    surface
        .layout_notes
        .retain(|n| !n.starts_with("no front line:"));
    if repair {
        footing::repair(surface, inputs.ground, inputs.types);
    }
    surface.starts = starts::place(surface, inputs.ground);
    let mut added = Vec::new();
    let mut notes = Vec::new();
    let fixed: Vec<_> = surface
        .units
        .iter()
        .filter_map(|unit| {
            let info = (inputs.types)(&unit.resource);
            (!info.strip && !info.bridge_or_road && unit.resource != BARRAGE_ZONE).then_some(
                Placed {
                    id: unit.id.0,
                    at: xz(unit.position),
                    heading: unit.angles[0],
                    footprint: info.footprint,
                },
            )
        })
        .collect();
    front::add(
        surface,
        inputs,
        &Obstacles {
            fixed: &fixed,
            strips: &[],
        },
        &mut added,
        &mut notes,
    );
    if added.is_empty() && !old_units.is_empty() {
        for u in &old_units {
            surface.object_sides.insert(u.id.0, u.side);
        }
        surface.units.extend(old_units);
        surface.placements.extend(old_placements);
        notes.push(
            "front clearance unresolved: retained original equipment after final-start recheck"
                .into(),
        );
    } else {
        commit(surface, added);
    }
    surface.layout_notes.extend(notes);
}

#[cfg(test)]
mod tests;
