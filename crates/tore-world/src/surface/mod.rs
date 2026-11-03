//! Surface units: the ships, SAM sites, AAA guns, vehicles, men and buildings
//! that stand on the ground or sea of a mission, as resolved from the theater
//! layout and, with a Quick Mission ground target, from its retail template.
//! Behaviour and numbers: docs/spec/surface-defenses.md; record layouts:
//! docs/formats/surface-units.md and docs/formats/quick-templates.md.
//!
//! [`Surface`] is the resolved, immutable picture every machine builds alike
//! from the same mission text and the same retail data: one [`Unit`] per
//! surface object with its fixed id ([`UnitId`]), owner, side, type and
//! destroyed look, plus the parked aircraft, supply trucks and batteries
//! later slices place. [`Surface::digest`] fingerprints it so two machines
//! can prove they built the same one. The terrain holds it
//! ([`crate::terrain::Terrain::surface`]) and builds its scene from it;
//! combat registers each unit as a target with its side, and keeps the
//! units' changing state ([`SurfaceUnitState`]) in its checkpointed state.
//!
//! This module resolves and identifies; [`movement`] moves the units that
//! follow a route. Nothing here fires.
pub mod aim;
pub mod catalog;
mod checkpoint;
pub mod emitters;
pub mod fire;
pub mod layout;
pub mod movement;
pub mod muzzle;
pub mod parked;
pub mod resolve;
pub mod rotor_reach;
pub mod supply;
pub mod units;
pub mod wreck;

pub use units::{
    Battery, BatteryState, BatterySystem, Engager, GroupTransform, MountStock, ParkedAircraft,
    RadarState, StartPoints, SupplyTruck, SurfaceState, SurfaceUnitState,
};

use crate::ai_wings::{ENEMY_SIDE, FRIENDLY_SIDE};
use std::collections::BTreeMap;
use tore_formats::quick_template::{Placeholder, Route};
use tore_sim::combat::live::{NO_SIDE, Side};

/// First id of the theater layout's objects: `0x4000_0000` plus the
/// placement's ordinal in the `.MM` file (unchanged from the airport scene,
/// so `GroundDestroyed`, rejoin lists and replays keep working).
pub const LAYOUT_OBJECT_BASE: u32 = 0x4000_0000;
/// First id of a ground target template's objects: `0x5000_0000` plus the
/// object's ordinal in the template, taken before the defense rolls, so a
/// removed slot leaves a gap and every machine numbers the rest alike.
pub const SURFACE_UNIT_BASE: u32 = 0x5000_0000;
/// First id of the supply trucks a later slice adds (`0x5800_0000 + n`, `n` in
/// the order of the unit each serves, ascending unit id).
pub const SUPPLY_TRUCK_BASE: u32 = 0x5800_0000;
/// First id of the battery radar elements a later slice adds
/// (`0x5C00_0000 + n`, `n` in battery order, ascending lowest launcher id).
pub const BATTERY_RADAR_BASE: u32 = 0x5C00_0000;
/// First id of the front line's units (`0x5E00_0000 + n`, `n` in the order
/// the layout adds them: Blue's, then Red's).
pub const FRONT_LINE_BASE: u32 = 0x5E00_0000;
/// First id of the enemy area defenses of a mission with no ground target
/// (`0x5F00_0000 + n`, `n` in placement order: SAM sites, then guns; B23).
pub const AREA_DEFENSE_BASE: u32 = 0x5F00_0000;
/// One past the last surface unit id.
pub const SURFACE_UNIT_END: u32 = 0x6000_0000;
/// First id of the shots surface units fire (SAMs and shells), a counter of
/// its own in combat. AI aircraft shots start at `1 << 24` and would need
/// 16.7 million shots to reach it.
pub use tore_sim::combat::live::SURFACE_PROJECTILE_ID_BASE;
/// A live supply truck resupplies friendly units within this horizontal
/// distance: 0.1 mile (John, 2026-10-10).
pub const RESUPPLY_RADIUS_FT: f64 = 528.0;

/// A surface object's fixed id, shared by the combat target, the scene, the
/// network and replays. See the ranges above.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitId(pub u32);

/// Which reserved range an id belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdRange {
    /// A theater layout object.
    Layout,
    /// A ground target template object.
    Template,
    /// A supply truck added to a defended group.
    SupplyTruck,
    /// A battery radar element added to a SAM battery.
    BatteryRadar,
    /// A unit of the Quick Mission front line, where Blue and Red ground
    /// forces stand in contact.
    FrontLine,
    /// An enemy area defense of a mission with no ground target: a SAM
    /// launcher or a gun standing on Red's side of the front.
    AreaDefense,
    /// Not a surface id.
    Other,
}

impl UnitId {
    /// The id of the theater layout's placement `ordinal`.
    pub fn layout(ordinal: u32) -> Option<Self> {
        Self::within(LAYOUT_OBJECT_BASE, SURFACE_UNIT_BASE, ordinal)
    }
    /// The id of the template's object `ordinal`.
    pub fn template(ordinal: u32) -> Option<Self> {
        Self::within(SURFACE_UNIT_BASE, SUPPLY_TRUCK_BASE, ordinal)
    }
    /// The id of the `n`th added supply truck.
    pub fn supply_truck(n: u32) -> Option<Self> {
        Self::within(SUPPLY_TRUCK_BASE, BATTERY_RADAR_BASE, n)
    }
    /// The id of the `n`th added battery radar.
    pub fn battery_radar(n: u32) -> Option<Self> {
        Self::within(BATTERY_RADAR_BASE, FRONT_LINE_BASE, n)
    }
    /// The id of the `n`th front line unit.
    pub fn front_line(n: u32) -> Option<Self> {
        Self::within(FRONT_LINE_BASE, AREA_DEFENSE_BASE, n)
    }
    /// The id of the `n`th area defense unit.
    pub fn area_defense(n: u32) -> Option<Self> {
        Self::within(AREA_DEFENSE_BASE, SURFACE_UNIT_END, n)
    }
    fn within(base: u32, end: u32, n: u32) -> Option<Self> {
        base.checked_add(n).filter(|id| *id < end).map(Self)
    }
    pub fn range(self) -> IdRange {
        match self.0 {
            LAYOUT_OBJECT_BASE..SURFACE_UNIT_BASE => IdRange::Layout,
            SURFACE_UNIT_BASE..SUPPLY_TRUCK_BASE => IdRange::Template,
            SUPPLY_TRUCK_BASE..BATTERY_RADAR_BASE => IdRange::SupplyTruck,
            BATTERY_RADAR_BASE..FRONT_LINE_BASE => IdRange::BatteryRadar,
            FRONT_LINE_BASE..AREA_DEFENSE_BASE => IdRange::FrontLine,
            AREA_DEFENSE_BASE..SURFACE_UNIT_END => IdRange::AreaDefense,
            _ => IdRange::Other,
        }
    }
}

/// The combat side of an owner: the nationality byte's bit 0x80 is Redfor
/// (the AI wings' enemy side), clear is Blue (their friendly side), and an
/// object with no owner field is neutral scenery.
pub fn side_of_owner(redfor: Option<bool>) -> Side {
    match redfor {
        Some(true) => ENEMY_SIDE,
        Some(false) => FRIENDLY_SIDE,
        None => NO_SIDE,
    }
}

/// An NT mount's position (`pos.x/y/z`, hull relative) in world feet, at the
/// size the unit is drawn. The record gives retail feet at the shape scale
/// (a Krivak's mounts lie inside its hull only there); surface units are
/// drawn at real size ([`crate::terrain::PlacedSize::RealSize`]), so the
/// muzzles and aim points scale with them. Axes stay in the record's order.
pub fn mount_position_ft(mount: &tore_formats::surface_unit::Mount) -> [f64; 3] {
    mount
        .position
        .map(|v| crate::terrain::PlacedSize::RealSize.feet(f64::from(v)))
}

/// A surface unit shape's F2 ground offset (the shape's bottom, read as feet
/// at the shape scale) in world feet at the size the unit is drawn, or `None`
/// when the shape names none.
pub fn ground_offset_ft(shape_bytes: &[u8]) -> crate::WorldResult<Option<f64>> {
    Ok(tore_formats::shape::contact_offset(shape_bytes)?
        .map(|v| crate::terrain::PlacedSize::RealSize.feet(f64::from(v))))
}

/// Where a unit came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The theater layout's placement `ordinal`.
    Layout { ordinal: u32 },
    /// The ground target template's object `ordinal`; `placeholder` is the
    /// slot it filled, when it was one.
    Template {
        ordinal: u32,
        placeholder: Option<Placeholder>,
    },
    /// Added by the layout rules (a supply truck or a battery radar).
    Added,
}

/// What a unit can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    /// An NT with a weapon or sensor mount: it searches and fires (later
    /// slices).
    Active,
    /// An unarmed NT: targetable and movable, never fires.
    Passive,
    /// A static object (OT): a building, bunker, road or strip piece.
    Structure,
}

/// What a destroyed unit looks like (docs/spec/surface-defenses.md,
/// "Destroyed looks"). Metadata only: the presentation slice draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DestroyedLook {
    /// Ships: swap to the `_A` shape and burn (retail shapes).
    DamagedShape(String),
    /// Ground vehicles, SAM launchers, AAA guns: replaced by the named wreck
    /// object, `DEST.OT` (fitted: the swap rule is untraced).
    Wreck(String),
    /// Bunkers with a damaged variant: swap to that object (`~BNK5.OT`).
    DamagedObject(String),
    /// Other buildings: the scenery draws the damage branch of their shape,
    /// a generated ruin (bridge ends included), or (pavement, rocks) their
    /// intact look; they never vanish (B16).
    Removed,
    /// Men whose shape holds a damaged model (the airfield squad TROOPS):
    /// that model, the men lying on the ground (retail shape, drawn when
    /// they are killed; John, 2026-10-13). Nothing burns.
    Bodies(String),
    /// Other men and invisible barrage zones: gone.
    Vanish,
}

/// One resolved surface unit. Positions and angles are the retail integer
/// words (feet; `pos` y 0 is ground level), exactly as resolved, so every
/// machine has the same values.
#[derive(Clone, Debug, PartialEq)]
pub struct Unit {
    pub id: UnitId,
    pub origin: Origin,
    /// Archive resource of its type: `SA6.NT`, `BNK6.OT`.
    pub resource: String,
    pub kind: UnitKind,
    /// The OBJECT block's class word (`tore_formats::surface_unit::class`).
    pub class: u16,
    /// Its type's short display name.
    pub name: String,
    /// Creator nationality index with the side in bit 0x80, when it has an
    /// owner field.
    pub nationality: Option<i32>,
    pub side: Side,
    pub position: [i32; 3],
    pub angles: [i32; 3],
    /// Mission `flags`; 0x80 marks a destroy target.
    pub flags: i32,
    /// Experience 0 (novice) to 3 (ace). Template objects carry their own;
    /// base-layout units take 1, as the generator's `themGroundSkill 1`
    /// (fitted).
    pub skill: i32,
    pub react: Option<[u32; 3]>,
    pub search_dist: Option<i32>,
    pub start_time: Option<i32>,
    /// The route the unit drives or sails, in template feet.
    pub route: Option<Route>,
    pub hit_points: i32,
    pub look: DestroyedLook,
    /// The NT's `expType` and `craterSize`; `None` for static objects, whose
    /// explosion stays the fitted ground-object one.
    pub explosion: Option<u8>,
    pub crater: Option<u8>,
    /// TRUCK or MISTRK: refills friendly rails and magazines nearby.
    pub supply_truck: bool,
    /// The scene holds a contact volume and drawn geometry for it, so combat
    /// can hit it. `false` while its shape is one the reader cannot draw yet.
    pub in_scene: bool,
}

impl Unit {
    /// Flag 0x80: a destroy target.
    pub fn is_target(&self) -> bool {
        self.flags & tore_formats::quick_template::TARGET_FLAG != 0
    }
}

/// A unit's pose for drawing a moving or wrecked unit. Filled by the
/// movement and presentation slices.
#[derive(Clone, Debug, PartialEq)]
pub struct SurfacePose {
    pub id: UnitId,
    pub position: [f64; 3],
    /// Heading, pitch and bank in radians.
    pub attitude: [f64; 3],
    /// The shape drawn: the main shape, or the destroyed look's.
    pub shape: Option<String>,
    pub wrecked: bool,
}

/// A template object left out of the world, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftOut {
    pub ordinal: u32,
    pub resource: String,
    pub why: &'static str,
}

/// The ground target a mission resolved: which template, under which
/// settings, and what the rolls did.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateSite {
    /// `QUCOL` for `~QUCOL.M`.
    pub stem: String,
    pub settings: resolve::GroundTarget,
    /// The equipment group the enemy nationality draws from.
    pub group: usize,
    /// `quickpos`, the engagement anchor of the "nothing" templates.
    pub quickpos: Option<[i32; 3]>,
    /// Template objects, before any roll.
    pub objects: usize,
    /// Ordinals of the defense slots whose roll failed.
    pub removed: Vec<u32>,
    pub left_out: Vec<LeftOut>,
    /// Why the template stays at its retail spot (the layout slice), `None`
    /// when it may relocate.
    pub anchor: Option<layout::Anchor>,
}

/// The mission's resolved surface. See the module comment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Surface {
    /// Every surface unit, ascending id: the base layout's NTs, then the
    /// template's objects.
    pub units: Vec<Unit>,
    /// The template's aircraft, parked on the ground or a carrier deck,
    /// ascending id.
    pub parked: Vec<ParkedAircraft>,
    /// Where each parked aircraft stands and what it is, once the terrain is
    /// known ([`parked::place`]); not in the digest, which covers what it is
    /// built from.
    pub parked_scene: Vec<parked::ParkedPose>,
    /// Supply trucks: those already standing in the layout and the template,
    /// and those the layout rules add.
    pub trucks: Vec<SupplyTruck>,
    /// SAM batteries (formed by the layout slice).
    pub batteries: Vec<Battery>,
    /// The template's rigid move (identity until the layout slice relocates
    /// it).
    pub transform: GroupTransform,
    /// The units that follow a route, by id: what [`movement`] needs to drive
    /// them. Routed templates are never relocated or jittered, so the routes
    /// are in the same frame as the units.
    pub courses: BTreeMap<UnitId, movement::Course>,
    pub template: Option<TemplateSite>,
    /// The enemy area defenses' settings, for a Quick Mission with no ground
    /// target and a strength above none (B23); the layout places them.
    pub area: Option<resolve::AreaDefense>,
    /// The side of every scene object by id: units and owned layout
    /// placements alike. Absent ids are neutral.
    pub object_sides: BTreeMap<u32, Side>,
    /// The template's objects as placements for the scene, with their ids.
    pub placements: Vec<(UnitId, tore_formats::mission::Placement)>,
    /// Types whose record could not be read, with the reason; such a layout
    /// placement stays plain scenery.
    pub unreadable: Vec<(String, String)>,
    /// Why the mission's ground target stands nowhere: its template is not in
    /// the import. The mission flies without it.
    pub unresolved: Option<String>,
    /// What the armed units, radars and batteries fight with, read from their
    /// records when the terrain builds (not part of the digest: it follows
    /// from the units and the import).
    pub arsenal: fire::Arsenal,
    /// Where the two sides start with a ground target (the layout slice).
    pub starts: Option<StartPoints>,
    /// What the layout could not add (a battery radar or a supply truck
    /// whose type the import lacks), for the log and the dump.
    pub layout_notes: Vec<String>,
}

impl Surface {
    pub fn unit(&self, id: UnitId) -> Option<&Unit> {
        self.units
            .binary_search_by_key(&id, |unit| unit.id)
            .ok()
            .map(|at| &self.units[at])
    }
    /// The side a scene object fights for; neutral when it has none.
    pub fn side_of(&self, object: u32) -> Side {
        self.object_sides.get(&object).copied().unwrap_or(NO_SIDE)
    }
    /// The destroy targets: template units and parked aircraft flagged 0x80
    /// that exist after resolution.
    pub fn targets(&self) -> impl Iterator<Item = UnitId> + '_ {
        self.units
            .iter()
            .filter(|unit| unit.is_target())
            .map(|unit| unit.id)
            .chain(self.parked.iter().filter(|p| p.target).map(|p| p.id))
    }
    /// The template's units, ascending id.
    pub fn template_units(&self) -> impl Iterator<Item = &Unit> + '_ {
        self.units
            .iter()
            .filter(|unit| unit.id.range() == IdRange::Template)
    }
    /// The base layout's units, ascending id.
    pub fn layout_units(&self) -> impl Iterator<Item = &Unit> + '_ {
        self.units
            .iter()
            .filter(|unit| unit.id.range() == IdRange::Layout)
    }
    /// A fresh changing state for every unit, as a mission starts.
    pub fn fresh_state(&self) -> SurfaceState {
        SurfaceState::new(
            self.digest(),
            self.units
                .iter()
                .map(|unit| SurfaceUnitState::new(unit.id))
                .collect(),
        )
    }

    /// FNV-1a 64 over everything resolved and placed: the template and its
    /// settings, the group transform, then every unit (id, type, position,
    /// angles, owner, side, flags, skill), every parked aircraft, supply
    /// truck and battery, in id order, and the starts. Integers are little endian, strings length
    /// prefixed, so every platform computes the same value. A machine whose
    /// digest differs from the host's built a different surface.
    pub fn digest(&self) -> u64 {
        let mut h = Digest::default();
        h.bytes(b"tore-surface-1");
        match &self.template {
            Some(site) => {
                h.u8(1);
                h.text(&site.stem);
                h.u32(site.settings.aaa as u32);
                h.u32(site.settings.sam as u32);
                h.u32(site.settings.seed);
                h.u32(site.settings.enemy_nationality as u32);
                h.u8(u8::from(site.settings.night_stealth));
                h.u32(site.settings.separation_nm);
                h.u8(u8::from(site.settings.variation.jitter)
                    | u8::from(site.settings.variation.relocate) << 1
                    | u8::from(site.settings.variation.front) << 2);
            }
            None => h.u8(0),
        }
        match &self.unresolved {
            Some(why) => {
                h.u8(1);
                h.text(why);
            }
            None => h.u8(0),
        }
        let GroupTransform {
            rotation_deg,
            translation,
            pivot,
        } = self.transform;
        h.i32(rotation_deg);
        h.i32s(&translation);
        h.i32s(&pivot);
        h.u32(self.units.len() as u32);
        for unit in &self.units {
            h.u32(unit.id.0);
            h.text(&unit.resource);
            h.i32s(&unit.position);
            h.i32s(&unit.angles);
            h.i32(unit.nationality.unwrap_or(-1));
            h.u32(unit.side.0);
            h.i32(unit.flags);
            h.i32(unit.skill);
        }
        h.u32(self.parked.len() as u32);
        for parked in &self.parked {
            h.u32(parked.id.0);
            h.text(&parked.resource);
            h.i32s(&parked.position);
            h.i32s(&parked.angles);
            h.u32(parked.side.0);
            h.u8(u8::from(parked.target));
            h.u32(parked.deck.map_or(0, |id| id.0));
        }
        h.u32(self.trucks.len() as u32);
        for truck in &self.trucks {
            h.u32(truck.id.0);
            h.u32(truck.serves.map_or(0, |id| id.0));
            h.u8(u8::from(truck.added));
        }
        h.u32(self.batteries.len() as u32);
        for battery in &self.batteries {
            h.u8(battery.system as u8);
            h.u32(battery.side.0);
            h.u32(battery.radar.0);
            h.u8(u8::from(battery.radar_added));
            h.u32(battery.launchers.len() as u32);
            for launcher in &battery.launchers {
                h.u32(launcher.0);
            }
            h.u32(battery.truck.map_or(0, |id| id.0));
        }
        match &self.starts {
            Some(starts) => {
                h.u8(1);
                h.i32s(&starts.target);
                h.i32s(&starts.red);
                h.i32s(&starts.blue);
                h.i32(starts.blue_heading_deg);
                for fields in [&starts.blue_airfields, &starts.red_airfields] {
                    h.u32(fields.len() as u32);
                    for id in fields {
                        h.u32(*id);
                    }
                }
            }
            None => h.u8(0),
        }
        // Fed only when set, so every digest without area defenses is the
        // one it always was.
        if let Some(area) = &self.area {
            h.bytes(b"area");
            h.u32(area.aaa as u32);
            h.u32(area.sam as u32);
            h.u32(area.seed);
            h.u32(area.enemy_nationality as u32);
            h.u8(u8::from(area.night_stealth));
            // Only the rolled counts say so: the first build's digest
            // (fixed counts) stays what it was.
            if area.rolled {
                h.bytes(b"rolled");
            }
        }
        h.finish()
    }
}

/// The digest's byte feed.
#[derive(Default)]
struct Digest(tore_codec::hash::Fnv1a64);

impl Digest {
    fn bytes(&mut self, data: &[u8]) {
        self.0.update(data);
    }
    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }
    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }
    fn i32(&mut self, value: i32) {
        self.bytes(&value.to_le_bytes());
    }
    fn i32s(&mut self, values: &[i32]) {
        for value in values {
            self.i32(*value);
        }
    }
    fn text(&mut self, text: &str) {
        self.u32(text.len() as u32);
        self.bytes(text.as_bytes());
    }
    fn finish(&self) -> u64 {
        self.0.finish()
    }
}

#[cfg(test)]
mod movement_tests;
#[cfg(test)]
mod objective_tests;
#[cfg(test)]
mod parked_tests;
#[cfg(test)]
mod tests;
