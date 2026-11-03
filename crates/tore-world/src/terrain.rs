//! Simulation half of the mission's world: the T2 grid and its height, surface
//! and water queries, the airport scene and its runway anchors, and the weather
//! clock. It holds no art, palette, render origin or per-frame state (those are
//! the app's scenery) and reads no environment variable. World units
//! are feet, X east, Y up, Z north.
use crate::{WorldResult, resources::ResourceSource};
use std::collections::{BTreeMap, BTreeSet};
use tore_formats::theater::{CELL_FEET, Environment, HEIGHT_FEET, Theater};

pub mod redrawn;

/// The free-flight start over the Ukraine theaters, before the ground clamp.
pub const UKRAINE_START: [f64; 3] = [1_070_000.0, 28_000.0, 590_000.0];

/// What the simulation queries about the mission's ground and sky. Built from
/// the imported resources alone, so a server can build it without any art.
pub struct Terrain {
    /// Shared foundation terrain triangles for contact and drawing.
    pub foundations: crate::grounding::pads::Mesh,
    pub theater: Theater,
    /// Exact selected MM identity, distinct from its referenced base grid.
    pub layout: String,
    /// The recovered weather choice the world was built with, if any; `None`
    /// keeps the mission's own `layer` line and time.
    #[allow(dead_code)] // Read by the mission recorder.
    pub condition: Option<usize>,
    pub environment: Environment,
    /// Immutable imported placement/airport geometry. Mutable health belongs to combat.
    pub airport_scene: tore_sim::airport::Scene,
    /// Exact static obstacles used before the final front and grounding pass.
    pub placement_objects: Vec<crate::surface::layout::Placed>,
    /// Integer runway-lane clearance boxes used by final vehicle placement.
    pub placement_runways: Vec<crate::surface::layout::Placed>,
    /// Each runway's taxi, takeoff, landing and parking points from its STRIP
    /// shape, by runway object id. A runway is absent when its shape lacks a
    /// point or a point is off the airport surface.
    pub airfield_anchors: BTreeMap<u32, tore_sim::ai::airfield::AirfieldAnchors>,
    /// Every source placement, including definitions the bounded SH projector cannot draw.
    pub static_manifest: Vec<(u32, tore_formats::mission::SourceKey, String, bool)>,
    /// The mission's own theater: its code and label (empty when the import
    /// has no label for it). It no longer lists every theater of the import.
    pub catalog: Vec<(String, String)>,
    /// Authoritative environment. One instance per world, so every camera,
    /// mirror and panel resolves the same instant.
    pub weather: tore_sim::environment::Environment,
    /// The mission's surface units: the layout's NTs and the ground target
    /// template's objects, with their ids, owners and sides. The airport
    /// scene holds a contact volume for each one whose shape reads.
    pub surface: crate::surface::Surface,
    /// Real-size scenery buildings over the city texture's light areas
    /// (`crate::city`, [`Overrides::city_fill`], the default): `None` when
    /// the world was built without the fill or its theater has no city art.
    pub city_fill: Option<std::sync::Arc<crate::city::CityFill>>,
    /// The redrawn airports that replace the retail airfields
    /// ([`Overrides::redrawn_airports`], the default); empty when the
    /// world was built with the retail airfields.
    pub redrawn: Vec<redrawn::Built>,
    /// Whether the world was built with the redrawn airports, so a mission
    /// recording can say which airfields it was flown on (a layout with
    /// no airport has an empty [`Self::redrawn`] either way).
    pub redrawn_airports: bool,
    /// Whether the redraw took in the runway pieces of the ground target's
    /// template too (B14, airfield scene 2): always with the redrawn
    /// airports, except in a replay of a recording flown on scene 1.
    pub redrawn_templates: bool,
    /// Single-field replacement of paired retail airports (airfield scene 3).
    pub merged_airports: bool,
    /// Ground-art cells repaired from their own ground texels, retaining source water cutouts.
    pub neutral_airfield_ground: BTreeSet<(i32, i32)>,
}

/// Launch settings that replace the mission's own weather start time, wind
/// and cloud deck. The app resolves them from `TORE_WEATHER_TIME`,
/// `TORE_WIND` and `TORE_CLOUD_ALTITUDE` (see
/// the app's `scenery::launch_overrides`) and passes them in, so the terrain
/// itself reads no environment variable.
#[derive(Clone, Debug, PartialEq)]
pub struct Overrides {
    /// Start time: hour and minute.
    pub time: Option<[i32; 2]>,
    /// Wind heading in degrees and speed in feet per second.
    pub wind: Option<[i32; 2]>,
    /// Scattered cloud deck in feet, 0 for none.
    pub cloud_altitude: Option<i32>,
    /// Draw and fly every airport from its type's real-size plan
    /// ([`redrawn`]). On by default everywhere (John, 2026-10-11); off
    /// builds the retail airfields, which the app allows in single player
    /// as a developer escape hatch (`TORE_REDRAWN_AIRPORTS=0`). A mission
    /// text never turns it off, so every game of a session and the server
    /// build the same airfields.
    pub redrawn_airports: bool,
    /// Fill the city texture tiles with real-size scenery buildings
    /// ([`crate::city`]). On by default everywhere (John, 2026-10-11,
    /// protocol 25); off builds no fill, which the app allows in single
    /// player and the offline tools as a developer escape hatch
    /// (`TORE_CITY_FILL=0`). Aircraft crash into the buildings and ground
    /// bursts knock them flat, so a mission text never turns it off: every
    /// machine of a session and the server build the same city.
    pub city_fill: bool,
    /// The city fill's per-theater building budget, `None` for the default
    /// ([`crate::city::fill::MAX_INSTANCES`]). A measuring aid the app sets
    /// from `TORE_CITY_FILL_CAP` for single player and the offline tools
    /// only: a mission text never sets it, so a session, the server and the
    /// bot always build the whole fill whatever the variable says, and a
    /// replay builds the default budget.
    pub city_fill_cap: Option<usize>,
}

impl Default for Overrides {
    /// No launch override, the redrawn airports and the city fill.
    fn default() -> Self {
        Self {
            time: None,
            wind: None,
            cloud_altitude: None,
            redrawn_airports: true,
            city_fill: true,
            city_fill_cap: None,
        }
    }
}

/// Fitted grounding: align the largest aggregate horizontal pavement layer,
/// not a terminal roof or the whole mesh's midpoint, with airport ground.
fn pavement_height(shape: &tore_formats::shape::Shape) -> f64 {
    let mut areas = BTreeMap::<u32, f64>::new();
    for face in &shape.faces {
        if face.positions.len() < 3 {
            continue;
        }
        let height = face.positions[0][2];
        if face.positions.iter().any(|p| (p[2] - height).abs() > 0.01) {
            continue;
        }
        let area = face
            .positions
            .iter()
            .zip(face.positions.iter().cycle().skip(1))
            .map(|(a, b)| f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]))
            .sum::<f64>()
            .abs()
            * 0.5;
        if area > 0. {
            *areas.entry(height.to_bits()).or_default() += area;
        }
    }
    areas
        .into_iter()
        .max_by(|a, b| {
            a.1.total_cmp(&b.1)
                .then_with(|| f32::from_bits(b.0).total_cmp(&f32::from_bits(a.0)))
        })
        .map_or(0., |(height, _)| f64::from(f32::from_bits(height)))
}

/// Spec-derived: the STRIP template roles in `docs/formats/native-strip.md`
/// ("Remaining template callback boundaries"). Box midpoints are feet in the
/// shape's frame ([right, up, forward]); `place` puts one in the world. None
/// unless every point the airfield sequences use is present.
fn airfield_anchors(
    boxes: &[tore_formats::shape::ContactBox],
    heading: f64,
    place: impl Fn([f64; 3]) -> [f64; 3],
) -> Option<tore_sim::ai::airfield::AirfieldAnchors> {
    // The native lookup returns the first box with an id.
    let point = |id: u8| {
        boxes
            .iter()
            .find(|b| b.id == id)
            .map(|b| place(b.midpoint().map(f64::from)))
    };
    let points = |first: u8| -> Option<[[f64; 3]; 4]> {
        Some([
            point(first)?,
            point(first + 1)?,
            point(first + 2)?,
            point(first + 3)?,
        ])
    };
    let mut parking = [[0.; 3]; 9];
    for (slot, place) in parking.iter_mut().enumerate() {
        *place = point(0x19 + slot as u8)?;
    }
    Some(tore_sim::ai::airfield::AirfieldAnchors {
        taxi_out: points(0x25)?,
        takeoff_spot: point(0x11)?,
        // Box 0x17's recorded orientation is zero on every reviewed STRIP, so
        // the runway heading is the placed airport heading.
        takeoff_heading: heading,
        landing_point: point(0x12)?,
        // `fitted`: box 0x18's heading is not decoded. The landing aim point
        // is behind the takeoff spot, sometimes on a parallel centerline.
        // The host uses the takeoff direction for landings.
        landing_heading: heading,
        taxi_in: points(0x29)?,
        parking,
        parking_heading: (heading + std::f64::consts::FRAC_PI_2).rem_euclid(std::f64::consts::TAU),
    })
}

fn anchor_points(
    anchors: &tore_sim::ai::airfield::AirfieldAnchors,
) -> impl Iterator<Item = [f64; 3]> + '_ {
    anchors
        .taxi_out
        .iter()
        .chain([&anchors.takeoff_spot, &anchors.landing_point])
        .chain(&anchors.taxi_in)
        .chain(&anchors.parking)
        .copied()
}

/// Names of the six recovered weather choices, in table order, as the
/// command line and recordings show them.
#[allow(dead_code)] // Read by the mission recorder.
pub const CONDITION_NAMES: [&str; 6] = ["clear", "cloudy", "foggy", "dawn", "sunset", "night"];

/// The launch settings a mission recording keeps for its world, resolved
/// once when it was flown. [`Terrain::for_recorded`] builds from these instead
/// of the environment variables and mission defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Recorded {
    /// Layout code without `.MM`, for example `UKR` or `~UKR1`.
    pub code: String,
    /// The weather choice, when the mission picked one.
    pub condition: Option<usize>,
    /// The resolved `.LAY` resource; `None` resolves it as a launch does.
    pub layer: Option<String>,
    /// Start time: hour and minute.
    pub time: [i32; 2],
    /// Explicit wind in degrees and feet per second, or `None` for the
    /// generated default.
    pub wind: Option<[i32; 2]>,
    /// Scattered cloud deck in feet, 0 for none.
    pub cloud_altitude: i32,
    pub weather_seed: i32,
    /// Rebuild with the redrawn airports: the recording's airfield scene
    /// (a recording from before the redraw has the retail airfields).
    pub redrawn_airports: bool,
    /// Rebuild with the template's runway piece redrawn too (airfield scene
    /// 2, B14); a scene 1 recording keeps it retail.
    pub redrawn_templates: bool,
    /// Single-field replacement of paired retail airports (airfield scene 3).
    pub merged_airports: bool,
    /// Rebuild with the city fill: the recording's city fill scene (a
    /// recording from before the fill was the default has none).
    pub city_fill: bool,
    /// The Quick Mission ground target the flight had, so the surface
    /// (template, defenses, seed, nationality, separation and so the starts)
    /// rebuilds exactly.
    pub target: Option<crate::surface::resolve::GroundTarget>,
    /// The enemy area defenses the flight had (no ground target, B23), so
    /// the same sites rebuild.
    pub area: Option<crate::surface::resolve::AreaDefense>,
}

/// The imported layout of one theater with the definitions and shapes its
/// placements name. Both halves of the airport scene build from it: the
/// terrain's objects and runways, and the scenery's static geometry.
pub struct Placements {
    pub layout: tore_formats::mission::Layout,
    pub definitions: BTreeMap<String, tore_formats::static_object::Definition>,
    pub shapes: BTreeMap<String, tore_formats::shape::Shape>,
    shape_scales: BTreeMap<String, f64>,
    /// NT records whose movement limit is zero, including fixed SAM sites.
    fixed_units: BTreeSet<String>,
    /// STRIP anchor 0x11 midpoint by object type, in the shape's frame.
    runway_anchors: BTreeMap<String, [f64; 3]>,
    strip_boxes: BTreeMap<String, Vec<tore_formats::shape::ContactBox>>,
    /// Main shapes the bounded projector could not read, with the reason. Such
    /// a placement stays in the manifest without geometry.
    pub unreadable: Vec<(String, String)>,
    /// The ground target template's placements, with their surface ids
    /// (`0x5000_0000` range), after the layout's.
    pub surface: Vec<(u32, tore_formats::mission::Placement)>,
    /// The grid cells the layout's ground texture placements stand on.
    tiles: Vec<(i64, i64)>,
}

/// Where one placed shape stands in the world.
pub struct Stance {
    pub scale: f64,
    /// Solid face/sprite bounds in scaled local axes (right, up, forward).
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub half: [f64; 3],
    pub heading: f64,
    pub pitch: f64,
    pub bank: f64,
    pub basis: tore_sim::attitude::Basis,
    pub support_origin: [f64; 3],
    /// The shape's origin, after a runway is grounded on its pavement.
    pub origin: [f64; 3],
    pub center: [f64; 3],
    pub runway: bool,
    /// Feet the slope rule raised (or lowered) the whole shape above the
    /// origin rule's stance (`grounding::slope_lift_ft`); 0 on the origin rule.
    pub slope_lift: f64,
}

/// A runway's length in feet from its shape's extent along the runway (`min_z`
/// to `max_z`, scaled) and the STRIP anchor 0x11 midpoint's position along it:
/// the run from the anchor to the far end when the anchor lies inside the
/// shape, else the whole extent.
pub fn runway_length_ft(min_z: f64, max_z: f64, anchor_z: Option<f64>) -> f64 {
    match anchor_z {
        Some(anchor) if anchor < max_z && anchor >= min_z => max_z - anchor,
        _ => ((max_z - min_z) * 0.5).max(1.0) * 2.0,
    }
}

/// The runway length of an airport object type (a STRIP definition such as
/// `AIRPORT.OT`), read from the imported resources, or `None` when the type is
/// not an airport or its shape cannot be read. The same length the airport
/// scene gives the runway of a placement of this type, without building the
/// theater: the Quick Mission creator uses it to keep short strips off its
/// ground-start list (`tore_sim::airport::SHORT_STRIP_FT`).
pub fn strip_length_ft(resources: &dyn ResourceSource, object_type: &str) -> Option<f64> {
    let definition =
        tore_formats::static_object::Definition::parse(resources.get(object_type)?).ok()?;
    if !definition.callbacks.iter().any(|name| name == "_STRIPProc") {
        return None;
    }
    let shape_bytes = resources.get(definition.main_shape.as_ref()?)?;
    let shape = tore_formats::shape::Shape::scenery(shape_bytes).ok()?;
    let scale = placed_shape_scale(&definition, shape_bytes).ok()?;
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for point in shape.faces.iter().flat_map(|face| &face.positions) {
        // The shape's third coordinate is the runway's forward axis.
        min = min.min(f64::from(point[1]));
        max = max.max(f64::from(point[1]));
    }
    if !min.is_finite() || !max.is_finite() {
        return None;
    }
    let anchor = tore_formats::shape::contact_boxes(shape_bytes)
        .ok()
        .flatten()
        .and_then(|boxes| boxes.iter().find(|b| b.id == 0x11).map(|b| b.midpoint()[2]))
        .map(f64::from);
    Some(runway_length_ft(min * scale, max * scale, anchor))
}

/// The size a placed object is drawn at, against the retail shape scale.
///
/// Retail draws every shape at `2^(e-8)` feet per unit (the SH header
/// exponent `e`), about three times real size, on a map whose positions are
/// real feet. This game draws aircraft at real size, so placed objects are
/// drawn at real size too (John, 2026-10-10: "runways and buildings and
/// aircraft are all the same realistic scale"), except the objects whose size
/// is part of the map. See docs/formats/objects-and-shapes.md, "Placed object
/// scale".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlacedSize {
    /// Runways and strips (`_STRIPProc`), bridges and roads: a runway's length
    /// is a real map length and a bridge spans a river on the terrain, so
    /// they keep the retail shape scale.
    MapTied,
    /// Every other placed object (buildings, theater objects, city blocks,
    /// surface units): a third of the retail shape scale, its real size.
    RealSize,
}

/// The fitted factor that brings a retail shape to real size: one third, the
/// aircraft renderer's factor. Measured: at a third, an F/A-18D is 56 ft
/// long, a Krivak 405 ft, a T-72 30 ft and a Nimitz 1,092 ft, their real
/// sizes.
pub const REAL_SIZE_FACTOR: f64 = 1.0 / 3.0;

/// Map-tied types that the definition cannot name itself: the retail
/// bridges and roads (FA_2.LIB). Their OBJECT records are `_OBJProc` objects
/// like any building, and no flag or class word sets them apart (a bridge
/// end has flags `$901`, a crane the `$20921` of a bridge middle, a road the
/// `$0` of a tree), so they are listed by resource name. Runways need no
/// entry: their definition names `_STRIPProc`.
pub const MAP_TIED_TYPES: [&str; 16] = [
    "BR1END.OT",
    "BR1MID.OT",
    "BR2END.OT",
    "BR2MID.OT",
    "BR3END.OT",
    "BR3MID.OT",
    "BRD1.OT",
    "BRD2.OT",
    "BRD3.OT",
    "BRD4.OT",
    "BRDEND.OT",
    "BRDMID.OT",
    "ROAD.OT",
    "ROAD2.OT",
    "ROAD4.OT",
    "ROADC.OT",
];

impl PlacedSize {
    /// The size rule of a placed type, from its definition.
    pub fn of(definition: &tore_formats::static_object::Definition) -> Self {
        let strip = definition.callbacks.iter().any(|name| name == "_STRIPProc");
        let listed = MAP_TIED_TYPES
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&definition.resource_name));
        if strip || listed {
            Self::MapTied
        } else {
            Self::RealSize
        }
    }
    /// The factor on the retail shape scale.
    pub fn factor(self) -> f64 {
        match self {
            Self::MapTied => 1.,
            Self::RealSize => REAL_SIZE_FACTOR,
        }
    }
    /// World feet of a length a record gives in retail feet at the shape
    /// scale: an NT mount position, a shape's F2 ground offset.
    pub fn feet(self, retail_feet: f64) -> f64 {
        retail_feet * self.factor()
    }
}

/// Feet per shape unit of a placed object: the reviewed SH header exponent
/// (`2^(e-8)`) times the [`PlacedSize`] factor of its definition. Every
/// placed object's drawn size, contact box, collision and hit box (through
/// [`Placements::stance`]) and a runway's length come from this one value.
pub fn placed_shape_scale(
    definition: &tore_formats::static_object::Definition,
    shape_bytes: &[u8],
) -> WorldResult<f64> {
    Ok(tore_formats::shape::object_scale(shape_bytes)? * PlacedSize::of(definition).factor())
}

/// A theater's layout, loaded once, for resolving and placing many ground
/// targets on it without rebuilding the scene: the relocation sweep and the
/// preview tools. A mission builds through [`Terrain::for_mission_with`],
/// which places the same way.
pub struct SurfaceSite {
    sources: Placements,
    code: String,
}

impl SurfaceSite {
    pub fn load(resources: &dyn ResourceSource, code: &str) -> WorldResult<Self> {
        let code = code.trim_end_matches(".MM").to_owned();
        Ok(Self {
            sources: Placements::load(resources, &code)?,
            code,
        })
    }

    /// The surface `target` resolves and places to on `theater` (this
    /// layout's grid), as a mission's terrain would hold it.
    pub fn place(
        &self,
        resources: &dyn ResourceSource,
        theater: &Theater,
        target: Option<&crate::surface::resolve::GroundTarget>,
    ) -> WorldResult<crate::surface::Surface> {
        self.place_with(resources, theater, target, None)
    }

    /// [`Self::place`] with the enemy area defenses of a mission with no
    /// ground target.
    pub fn place_with(
        &self,
        resources: &dyn ResourceSource,
        theater: &Theater,
        target: Option<&crate::surface::resolve::GroundTarget>,
        area: Option<&crate::surface::resolve::AreaDefense>,
    ) -> WorldResult<crate::surface::Surface> {
        use crate::surface::{
            catalog::Catalog,
            layout::{self, Inputs},
        };
        let mut surface = Terrain::resolve_surface(resources, &self.sources.layout, target, area)?;
        let mut types = type_infos(resources, &self.sources);
        let ground = surface_ground(theater, &self.sources, &mut types);
        let mut catalog = Catalog::new(resources);
        let mut added = |name: &str| {
            placeable(resources, name)
                .then(|| catalog.entry(name).ok())
                .flatten()
        };
        let layout_name = format!("{}.MM", self.code);
        layout::place(
            &mut surface,
            &mut Inputs {
                ground: &ground,
                types: &mut types,
                added: &mut added,
                layout: &layout_name,
            },
        );
        Ok(surface)
    }

    /// Read-only placement geometry for development audits. Uses the same
    /// terrain classes, tile coverage and authored footprints as placement.
    pub fn inspection_ground<'t>(
        &self,
        resources: &dyn ResourceSource,
        theater: &'t Theater,
    ) -> crate::surface::layout::Ground<'t> {
        surface_ground(
            theater,
            &self.sources,
            &mut type_infos(resources, &self.sources),
        )
    }

    /// The completed airport scene's footprints, after airport replacement.
    /// Terrain classes and texture coverage retain the placement query's rule.
    pub fn inspection_scene_ground<'t>(
        &self,
        terrain: &'t Terrain,
    ) -> crate::surface::layout::Ground<'t> {
        use crate::surface::layout::{Footprint, Ground, Placed};
        let placed = |object: &tore_sim::airport::StaticObject| {
            let b = &object.bounds;
            Placed {
                id: object.id,
                at: [b.center[0].round() as i64, b.center[2].round() as i64],
                heading: b.heading.to_degrees().round() as i32,
                footprint: Footprint {
                    min: [-(b.half[0].ceil() as i64), -(b.half[2].ceil() as i64)],
                    max: [b.half[0].ceil() as i64, b.half[2].ceil() as i64],
                },
            }
        };
        let (_runways, objects): (Vec<_>, Vec<_>) = terrain
            .airport_scene
            .objects
            .iter()
            .filter(|o| {
                o.runway || crate::surface::UnitId(o.id).range() == crate::surface::IdRange::Layout
            })
            .partition(|o| o.runway);
        Ground::new(
            &terrain.theater,
            objects.into_iter().map(placed).collect(),
            terrain.placement_runways.clone(),
            self.front(),
        )
        .with_tiles(self.sources.tiles.iter().copied())
    }

    /// Verify that reuse across separations preserves the final front. Equal raw
    /// placements alone are insufficient: replacement airports change obstacles.
    /// This repeats the production front pass against its exact integer geometry.
    pub fn inspection_front_matches(
        &self,
        resources: &dyn ResourceSource,
        terrain: &Terrain,
        raw: &crate::surface::Surface,
    ) -> bool {
        use crate::surface::{
            IdRange, Surface, UnitId,
            catalog::Catalog,
            layout::{self, Ground, Inputs},
        };
        let is_front = |id: UnitId| id.range() == IdRange::FrontLine;
        let front_note = |n: &str| {
            n.starts_with("no front line:") || n.starts_with("front clearance unresolved:")
        };
        let mut candidate = terrain.surface.clone();
        candidate.template.clone_from(&raw.template);
        candidate.units.retain(|u| !is_front(u.id));
        candidate
            .units
            .extend(raw.units.iter().filter(|u| is_front(u.id)).cloned());
        candidate.placements.retain(|(id, _)| !is_front(*id));
        candidate.placements.extend(
            raw.placements
                .iter()
                .filter(|(id, _)| is_front(*id))
                .cloned(),
        );
        candidate
            .object_sides
            .retain(|id, _| !is_front(UnitId(*id)));
        candidate.object_sides.extend(
            raw.object_sides
                .iter()
                .filter(|(id, _)| is_front(UnitId(**id)))
                .map(|(id, side)| (*id, *side)),
        );
        candidate.layout_notes.retain(|n| !front_note(n));
        candidate
            .layout_notes
            .extend(raw.layout_notes.iter().filter(|n| front_note(n)).cloned());
        let ground = Ground::new(
            &terrain.theater,
            terrain.placement_objects.clone(),
            terrain.placement_runways.clone(),
            self.front(),
        )
        .with_tiles(self.sources.tiles.iter().copied());
        let mut types = type_infos(resources, &self.sources);
        let mut catalog = Catalog::new(resources);
        let mut added = |name: &str| {
            placeable(resources, name)
                .then(|| catalog.entry(name).ok())
                .flatten()
        };
        layout::recheck_front(
            &mut candidate,
            &mut Inputs {
                ground: &ground,
                types: &mut types,
                added: &mut added,
                layout: &format!("{}.MM", self.code),
            },
        );
        let units = |surface: &Surface| {
            surface
                .units
                .iter()
                .filter(|u| is_front(u.id))
                .cloned()
                .map(|mut u| {
                    // Contact-volume availability is set by the later grounding pass.
                    u.in_scene = false;
                    u
                })
                .collect::<Vec<_>>()
        };
        let notes = |surface: &Surface| {
            surface
                .layout_notes
                .iter()
                .filter(|n| front_note(n))
                .cloned()
                .collect::<Vec<_>>()
        };
        units(&candidate) == units(&terrain.surface) && notes(&candidate) == notes(&terrain.surface)
    }

    /// A resource's footprint at its actual placed scale, for audits.
    pub fn inspection_type(
        &self,
        resources: &dyn ResourceSource,
        name: &str,
    ) -> crate::surface::layout::TypeInfo {
        placed_type_info(resources, &self.sources, name)
    }

    /// The layout's front (the centroids of its Blue and Red placements).
    pub fn front(&self) -> Option<crate::surface::layout::Front> {
        layout_front(&self.sources.layout)
    }

    /// [`Terrain::audit_surface`] for a surface this site placed.
    pub fn audit(
        &self,
        resources: &dyn ResourceSource,
        theater: &Theater,
        surface: &crate::surface::Surface,
    ) -> Vec<String> {
        let mut types = type_infos(resources, &self.sources);
        let ground = surface_ground(theater, &self.sources, &mut types);
        crate::surface::layout::audit(surface, &ground, &mut types)
    }

    /// The SAM sites and gun sites `area` rolls on this site's layout,
    /// before any is left out for want of a place.
    pub fn area_site_counts(&self, area: &crate::surface::resolve::AreaDefense) -> (usize, usize) {
        crate::surface::layout::area::site_counts(area, &format!("{}.MM", self.code))
    }

    /// [`crate::surface::layout::area::audit`] for a surface this site
    /// placed: the area defenses' own rules.
    pub fn audit_area(
        &self,
        resources: &dyn ResourceSource,
        theater: &Theater,
        surface: &crate::surface::Surface,
    ) -> Vec<String> {
        let mut types = type_infos(resources, &self.sources);
        let ground = surface_ground(theater, &self.sources, &mut types);
        crate::surface::layout::area::audit(surface, &ground, &mut types)
    }

    /// [`crate::surface::layout::class_audit`] for a surface this site
    /// placed: every unit on the terrain class its kind needs.
    pub fn audit_classes(
        &self,
        resources: &dyn ResourceSource,
        theater: &Theater,
        surface: &crate::surface::Surface,
        retail: Option<&crate::surface::Surface>,
    ) -> Vec<String> {
        let mut types = type_infos(resources, &self.sources);
        let ground = surface_ground(theater, &self.sources, &mut types);
        crate::surface::layout::class_audit(surface, &ground, retail)
    }
}

/// Every placed type's layout facts, read once each.
fn type_infos<'r>(
    resources: &'r dyn ResourceSource,
    sources: &'r Placements,
) -> impl FnMut(&str) -> crate::surface::layout::TypeInfo + 'r {
    let mut infos = BTreeMap::new();
    move |name: &str| {
        *infos
            .entry(name.to_owned())
            .or_insert_with(|| placed_type_info(resources, sources, name))
    }
}

/// The ground the surface layout places on: the grid, the theater layout's
/// objects and runways with their footprints, and the front between the
/// centroids of its Blue and Red placements.
fn surface_ground<'t>(
    theater: &'t Theater,
    sources: &Placements,
    types: &mut dyn FnMut(&str) -> crate::surface::layout::TypeInfo,
) -> crate::surface::layout::Ground<'t> {
    use crate::surface::layout::{Ground, Placed};
    let mut objects = Vec::new();
    let mut runways = Vec::new();
    for placement in &sources.layout.placements {
        let Some(id) = crate::surface::UnitId::layout(placement.key.ordinal) else {
            continue;
        };
        let at = [
            i64::from(placement.position[0]),
            i64::from(placement.position[2]),
        ];
        let info = types(&placement.object_type);
        let placed = Placed {
            id: id.0,
            at,
            heading: placement.angles[0],
            footprint: info.footprint,
        };
        if info.strip {
            runways.push(placed);
        } else {
            objects.push(placed);
        }
    }
    Ground::new(theater, objects, runways, layout_front(&sources.layout))
        .with_tiles(sources.tiles.iter().copied())
}

/// Integer clearance boxes for the actual drawn runway lanes, excluding aprons.
/// Redrawn merged fields translate on a 2048-foot grid. Recover that grid offset,
/// then transform the plan's whole-foot dimensions with the placement trig table.
fn placement_runways(
    sources: &Placements,
    built: &[redrawn::Built],
    mut fallback: Vec<crate::surface::layout::Placed>,
) -> Vec<crate::surface::layout::Placed> {
    use crate::surface::layout::{Footprint, Placed, trig};
    for field in built {
        if field.runways.is_empty() {
            continue;
        }
        let Some((_, p)) = sources
            .placed()
            .filter_map(Result::ok)
            .find(|(id, _)| *id == field.strip_id)
        else {
            continue;
        };
        let heading = p.angles[0];
        let anchor = sources
            .runway_anchors
            .get(&p.object_type)
            .copied()
            .unwrap_or([0.; 3]);
        let d = trig::rotate(
            [anchor[0].round() as i64, anchor[2].round() as i64],
            heading,
        );
        let mut origin = [
            i64::from(p.position[0]) + d[0],
            i64::from(p.position[2]) + d[1],
        ];
        if let Some(tile_id) = field.merged_from
            && let Some((_, tile)) = sources
                .placed()
                .filter_map(Result::ok)
                .find(|(id, _)| *id == tile_id)
        {
            // Compact paired fields are centered between the two authored
            // origins, with the frame 6000 feet behind that midpoint.
            let forward = trig::along(heading, 6000);
            origin = [
                (i64::from(p.position[0]) + i64::from(tile.position[0]) + 1) / 2 - forward[0],
                (i64::from(p.position[2]) + i64::from(tile.position[2]) + 1) / 2 - forward[1],
            ];
        }
        for (axis, world_axis) in [(0, 0), (1, 2)] {
            origin[axis] += ((field.frame.origin[world_axis] - origin[axis] as f64) / 2048.).round()
                as i64
                * 2048;
        }
        fallback.retain(|r| r.id != field.strip_id && Some(r.id) != field.merged_from);
        for runway in &field.runways {
            let turn = runway.heading.round() as i32;
            let length = runway.length.ceil() as i64;
            let milli = (runway.heading * 1000.).round() as i32;
            // The clearance box uses whole degrees like all placement boxes.
            // Expand it to enclose a fractional-degree lane, not rotate its
            // center away from the actual pavement.
            let error = i64::from((milli - turn * 1000).abs());
            let width = (runway.width / 2.).ceil() as i64;
            let side_pad = (length * error * 18 + 1_999_999) / 2_000_000;
            let end_pad = (width * error * 18 + 999_999) / 1_000_000;
            let half_width = width + side_pad;
            let along = trig::along_millidegrees(milli, length / 2);
            let local = [
                runway.from[0].round() as i64 + along[0],
                runway.from[1].round() as i64 + along[1],
            ];
            let delta = trig::rotate(local, heading);
            fallback.push(Placed {
                id: field.strip_id,
                at: [origin[0] + delta[0], origin[1] + delta[1]],
                heading: heading + turn,
                footprint: Footprint {
                    min: [-half_width, -length / 2 - end_pad],
                    max: [half_width, (length + 1) / 2 + end_pad],
                },
            });
        }
    }
    fallback
}

/// The front of a theater layout: the centroids of its Blue-side and
/// Red-side placements, `None` without both.
fn layout_front(layout: &tore_formats::mission::Layout) -> Option<crate::surface::layout::Front> {
    use crate::surface::layout::{Front, centroid};
    let at =
        |p: &tore_formats::mission::Placement| [i64::from(p.position[0]), i64::from(p.position[2])];
    let side = |red: bool| {
        centroid(
            layout
                .placements
                .iter()
                .filter(move |p| p.redfor() == Some(red))
                .map(at),
        )
    };
    match (side(false), side(true)) {
        (Some(blue), Some(red)) if blue != red => Some(Front { blue, red }),
        _ => None,
    }
}

/// The definition of a placed type: its record, or for the TORE-defined
/// HAWK radar element the Straight Flush's under its own shape.
fn placed_definition(
    resources: &dyn ResourceSource,
    object_type: &str,
) -> Option<tore_formats::static_object::Definition> {
    if object_type == crate::surface::catalog::HAWK_RADAR {
        return crate::surface::catalog::hawk_radar_definition(resources);
    }
    tore_formats::static_object::Definition::parse(resources.get(object_type)?).ok()
}

/// Whether the scene can place a unit of `object_type` the layout adds: its
/// record reads and its main shape is in the import.
fn placeable(resources: &dyn ResourceSource, object_type: &str) -> bool {
    placed_definition(resources, object_type).is_some_and(|definition| {
        definition
            .main_shape
            .as_deref()
            .is_none_or(|shape| resources.get(shape).is_some())
    })
}

/// What the surface layout needs to know about a placed type: its
/// horizontal footprint from its shape's integer vertex bounds at the placed
/// scale ([`placed_shape_scale`]), and whether it is a strip, bridge or road
/// piece. A type without a readable shape has an empty footprint.
fn placed_type_info(
    resources: &dyn ResourceSource,
    sources: &Placements,
    object_type: &str,
) -> crate::surface::layout::TypeInfo {
    use crate::surface::layout::{Footprint, TypeInfo};
    let owned;
    let definition = match sources.definitions.get(object_type) {
        Some(definition) => definition,
        None => match placed_definition(resources, object_type) {
            Some(definition) => {
                owned = definition;
                &owned
            }
            None => return TypeInfo::default(),
        },
    };
    let name = definition.display_name.to_ascii_lowercase();
    let mut info = TypeInfo {
        footprint: Footprint::default(),
        strip: definition.callbacks.iter().any(|c| c == "_STRIPProc"),
        bridge_or_road: name.contains("bridge") || name.contains("road"),
    };
    let Some(shape_name) = &definition.main_shape else {
        return info;
    };
    let Some(bytes) = resources.get(shape_name) else {
        return info;
    };
    let parsed;
    let shape = match sources.shapes.get(object_type) {
        Some(shape) => shape,
        None => match tore_formats::shape::Shape::scenery(bytes) {
            Ok(shape) => {
                parsed = shape;
                &parsed
            }
            Err(_) => return info,
        },
    };
    let Ok(scale) = placed_shape_scale(definition, bytes) else {
        return info;
    };
    // Shape units are whole numbers: right and forward are the first two
    // coordinates of a face position.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let sprites = shape.billboards.iter().flat_map(|sprite| {
        let w = f64::from(sprite.size[0]) * 0.5;
        let c = sprite.center.map(f64::from);
        [[c[0] - w, c[1] - w], [c[0] + w, c[1] + w]]
    });
    for point in shape
        .faces
        .iter()
        .flat_map(|face| {
            face.positions
                .iter()
                .map(|p| [f64::from(p[0]), f64::from(p[1])])
        })
        .chain(sprites)
    {
        for axis in 0..2 {
            lo[axis] = lo[axis].min(point[axis]);
            hi[axis] = hi[axis].max(point[axis]);
        }
    }
    if lo.iter().chain(&hi).all(|v| v.is_finite()) {
        info.footprint = Footprint {
            min: lo.map(|v| (v * scale).floor() as i64),
            max: hi.map(|v| (v * scale).ceil() as i64),
        };
    }
    info
}

impl Placements {
    pub fn load(resources: &dyn ResourceSource, code: &str) -> WorldResult<Self> {
        let layout_name = format!("{code}.MM");
        let layout = tore_formats::mission::Layout::parse(
            &layout_name,
            resources
                .get(&layout_name)
                .ok_or_else(|| format!("missing airport layout {layout_name}"))?,
        )?;
        let mut out = Self::empty();
        out.tiles = resources
            .get(&layout_name)
            .and_then(|bytes| tore_formats::theater::Environment::parse(bytes).ok())
            .map(|e| {
                e.textures
                    .values()
                    .map(|t| (i64::from(t.col), i64::from(t.row)))
                    .collect()
            })
            .unwrap_or_default();
        for placement in &layout.placements {
            out.add_type(resources, &layout_name, &placement.object_type)?;
        }
        out.layout = layout;
        Ok(out)
    }

    /// No placements and no types.
    fn empty() -> Self {
        Self {
            layout: tore_formats::mission::Layout {
                resource: String::new(),
                map: None,
                sides: Default::default(),
                placements: Vec::new(),
            },
            definitions: BTreeMap::new(),
            shapes: BTreeMap::new(),
            shape_scales: BTreeMap::new(),
            fixed_units: BTreeSet::new(),
            runway_anchors: BTreeMap::new(),
            strip_boxes: BTreeMap::new(),
            unreadable: Vec::new(),
            surface: Vec::new(),
            tiles: Vec::new(),
        }
    }

    /// The definitions and shapes of `types` with no placements, for an
    /// audit that stands each type itself ([`Self::stance`]); a type whose
    /// record does not read is skipped.
    pub fn of_types<'a>(
        resources: &dyn ResourceSource,
        types: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let mut out = Self::empty();
        for object_type in types {
            let _ = out.add_type(resources, "audit", object_type);
        }
        out
    }

    /// The layout's placements and the surface's template placements, as the
    /// terrain built its scene from them: what the scenery draws.
    pub fn for_terrain(
        resources: &dyn ResourceSource,
        terrain: &Terrain,
        code: &str,
    ) -> WorldResult<Self> {
        let mut out = Self::load(resources, code)?;
        redrawn::reapply(resources, &terrain.redrawn, &mut out)?;
        // Vehicle footing can move a base-layout NT too. Drawing must use the
        // same final pose as contact and simulation.
        for p in &mut out.layout.placements {
            if let Some(id) = crate::surface::UnitId::layout(p.key.ordinal)
                && let Some(unit) = terrain.surface.unit(id)
            {
                p.position = unit.position;
                p.angles = unit.angles;
            }
        }
        out.add_surface(resources, &terrain.surface)?;
        Ok(out)
    }

    /// Adds the surface's template placements and reads their types.
    pub fn add_surface(
        &mut self,
        resources: &dyn ResourceSource,
        surface: &crate::surface::Surface,
    ) -> WorldResult<()> {
        for (id, placement) in &surface.placements {
            self.add_type(resources, &placement.key.layout, &placement.object_type)?;
            self.surface.push((id.0, placement.clone()));
        }
        Ok(())
    }

    /// Every placement with its object id: the layout's (`0x4000_0000` plus
    /// ordinal), then the template's.
    pub fn placed(
        &self,
    ) -> impl Iterator<Item = WorldResult<(u32, &tore_formats::mission::Placement)>> + '_ {
        self.layout
            .placements
            .iter()
            .map(|placement| {
                crate::surface::UnitId::layout(placement.key.ordinal)
                    .map(|id| (id.0, placement))
                    .ok_or_else(|| "airport object ID overflow".into())
            })
            .chain(
                self.surface
                    .iter()
                    .map(|(id, placement)| Ok((*id, placement))),
            )
    }

    /// Reads a placed type's definition and main shape, once.
    fn add_type(
        &mut self,
        resources: &dyn ResourceSource,
        source: &str,
        object_type: &str,
    ) -> WorldResult<()> {
        if self.definitions.contains_key(object_type) {
            return Ok(());
        }
        if object_type.ends_with(".NT")
            && let Some(bytes) = resources.get(object_type)
            && let Ok(unit) = tore_formats::surface_unit::SurfaceUnit::parse(bytes)
            && unit.movement.max_speed == 0
        {
            self.fixed_units.insert(object_type.to_owned());
        }
        let definition = if object_type == crate::surface::catalog::HAWK_RADAR {
            crate::surface::catalog::hawk_radar_definition(resources).ok_or_else(|| {
                format!("{source}: missing the HAWK radar's record or shape; re-import media")
            })?
        } else {
            tore_formats::static_object::Definition::parse(resources.get(object_type).ok_or_else(
                || format!("{source}: missing placed definition {object_type}; re-import media"),
            )?)?
        };
        if let Some(main_shape) = &definition.main_shape {
            let shape_bytes = resources.get(main_shape).ok_or_else(|| {
                format!(
                    "{source}: missing shape {main_shape} referred by {object_type}; re-import media"
                )
            })?;
            let parsed = tore_formats::shape::Shape::scenery(shape_bytes);
            match parsed {
                Ok(shape) => {
                    if definition.callbacks.iter().any(|name| name == "_STRIPProc")
                        && let Some(boxes) = tore_formats::shape::contact_boxes(shape_bytes)?
                        && let Some(anchor) = boxes.iter().find(|b| b.id == 0x11)
                    {
                        self.runway_anchors
                            .insert(object_type.to_owned(), anchor.midpoint().map(f64::from));
                        self.strip_boxes
                            .insert(object_type.to_owned(), boxes.clone());
                    }
                    self.shape_scales.insert(
                        object_type.to_owned(),
                        placed_shape_scale(&definition, shape_bytes)?,
                    );
                    self.shapes.insert(object_type.to_owned(), shape);
                }
                Err(error) => self
                    .unreadable
                    .push((main_shape.clone(), error.to_string())),
            }
        }
        self.definitions.insert(object_type.to_owned(), definition);
        Ok(())
    }

    /// The scale `object_type`'s shape is drawn at (1 when unknown).
    pub fn shape_scale(&self, object_type: &str) -> f64 {
        self.shape_scales.get(object_type).copied().unwrap_or(1.0)
    }

    /// One foundation eligibility path for world construction, drawing and
    /// audits. Mobile NT records never acquire permanent terrain foundations.
    pub fn foundation(
        &self,
        p: &tore_formats::mission::Placement,
    ) -> Option<crate::grounding::pads::Pad> {
        if !p.object_type.ends_with(".OT") && !self.fixed_units.contains(&p.object_type) {
            return None;
        }
        crate::grounding::pads::Pad::of(
            self.definitions.get(&p.object_type)?,
            self.shapes.get(&p.object_type)?,
            self.shape_scale(&p.object_type),
            p,
        )
    }

    /// The shared stance on actual terrain, used by both drawing and contact.
    /// Whole assemblies keep their internal offsets and authored height.
    pub fn stance_on_terrain(
        &self,
        placement: &tore_formats::mission::Placement,
        height: impl Fn(f64, f64) -> f64,
    ) -> Option<Stance> {
        let ground = height(
            f64::from(placement.position[0]),
            f64::from(placement.position[2]),
        );
        let mut stance = self.stance(placement, ground)?;
        let definition = self.definitions.get(&placement.object_type)?;
        if crate::grounding::footing(definition) == crate::grounding::Footing::Bridge {
            let datum = crate::grounding::bridges::datum(self, placement, &height)?;
            let lift = datum + f64::from(placement.position[1]) - stance.origin[1];
            stance.origin[1] += lift;
            stance.center[1] += lift;
            stance.slope_lift = lift;
        } else if crate::grounding::footing(definition) == crate::grounding::Footing::Bottom {
            let shape = self.shapes.get(&placement.object_type)?;
            // The lift is measured from the ground, then the authored height
            // stays above it.
            let mut on_ground = stance.origin;
            on_ground[1] -= f64::from(placement.position[1]);
            if let Some(lift) = crate::grounding::slope_lift_ft(
                shape,
                stance.scale,
                stance.basis,
                on_ground,
                height,
            ) {
                stance.origin[1] += lift;
                stance.center[1] += lift;
                stance.slope_lift = lift;
            }
        }
        Some(stance)
    }

    /// Where `placement` stands on ground `ground` feet high, or `None` when it
    /// has no drawable shape or the shape has no finite extent.
    pub fn stance(
        &self,
        placement: &tore_formats::mission::Placement,
        ground: f64,
    ) -> Option<Stance> {
        let definition = self.definitions.get(&placement.object_type)?;
        let shape = self.shapes.get(&placement.object_type)?;
        let heading = f64::from(placement.angles[0]).to_radians();
        let runway = definition.callbacks.iter().any(|name| name == "_STRIPProc");
        // The source runway plane stays at authored ground. The renderer
        // applies a bounded static-surface depth bias without changing contact.
        let support_height = ground;
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        // Contact bounds exclude thin lines; the origin below still uses the
        // complete drawn shape for grounding and compact-object slope lift.
        for point in crate::grounding::placement_solid_points(definition, shape) {
            let mapped = [
                f64::from(point[0]),
                f64::from(point[2]),
                f64::from(point[1]),
            ];
            for axis in 0..3 {
                min[axis] = min[axis].min(mapped[axis]);
                max[axis] = max[axis].max(mapped[axis]);
            }
        }
        if min.iter().any(|value| !value.is_finite()) {
            return None;
        }
        // `placed_shape_scale` drives both visual and contact scale.
        let scale = self
            .shape_scales
            .get(&placement.object_type)
            .copied()
            .unwrap_or(1.0);
        for axis in 0..3 {
            min[axis] *= scale;
            max[axis] *= scale;
        }
        let half = std::array::from_fn(|axis| ((max[axis] - min[axis]) * 0.5).max(1.0));
        let pitch = f64::from(placement.angles[1]).to_radians();
        let bank = f64::from(placement.angles[2]).to_radians();
        let basis = tore_sim::attitude::Basis::new(heading, pitch, bank);
        let local_center = std::array::from_fn::<_, 3, _>(|axis| (min[axis] + max[axis]) * 0.5);
        let support_origin = [
            f64::from(placement.position[0]),
            support_height + f64::from(placement.position[1]),
            f64::from(placement.position[2]),
        ];
        // Every object stands on the bottom of its shape (crate::grounding);
        // a runway on its pavement, ships and roads on their origin.
        let grounding_offset = if runway {
            -pavement_height(shape) * scale
        } else {
            crate::grounding::placement_lift_ft(definition, shape, scale)
        };
        let origin = std::array::from_fn::<_, 3, _>(|axis| {
            support_origin[axis] + basis.up[axis] * grounding_offset
        });
        let center = std::array::from_fn(|axis| {
            origin[axis]
                + basis.right[axis] * local_center[0]
                + basis.up[axis] * local_center[1]
                + basis.forward[axis] * local_center[2]
        });
        Some(Stance {
            scale,
            min,
            max,
            half,
            heading,
            pitch,
            bank,
            basis,
            support_origin,
            origin,
            center,
            runway,
            slope_lift: 0.,
        })
    }
}

impl Terrain {
    /// Shared drawing/contact stance. Runway planes, road grades and ship
    /// waterlines keep the imported datum even when a nearby foundation's
    /// skirt changes the terrain under their placement anchor.
    pub fn placement_stance(
        &self,
        sources: &Placements,
        p: &tore_formats::mission::Placement,
    ) -> Option<Stance> {
        let foundation = matches!(
            crate::grounding::footing(sources.definitions.get(&p.object_type)?),
            crate::grounding::Footing::Bottom | crate::grounding::Footing::Bridge
        );
        sources.stance_on_terrain(p, |x, z| {
            f64::from(if foundation {
                self.height(x as f32, z as f32)
            } else {
                self.raw_height(x as f32, z as f32)
            })
        })
    }

    /// The terrain with the mission's own weather and no launch overrides.
    pub fn for_theater(resources: &dyn ResourceSource, code: &str) -> WorldResult<Self> {
        Self::for_mission(resources, code, None, &Overrides::default())
    }

    /// `condition` selects one of the six recovered weather choices; without it
    /// the mission's own `layer` line and time are used unchanged. `overrides`
    /// replace the start time, wind and cloud deck.
    pub fn for_mission(
        resources: &dyn ResourceSource,
        code: &str,
        condition: Option<usize>,
        overrides: &Overrides,
    ) -> WorldResult<Self> {
        Self::build(resources, code, condition, None, overrides, None, None)
    }

    /// [`Self::for_mission`] with the Quick Mission ground target, whose
    /// template's units join the surface and the airport scene.
    pub fn for_mission_with(
        resources: &dyn ResourceSource,
        code: &str,
        condition: Option<usize>,
        overrides: &Overrides,
        target: Option<&crate::surface::resolve::GroundTarget>,
    ) -> WorldResult<Self> {
        Self::build(resources, code, condition, None, overrides, target, None)
    }

    /// [`Self::for_mission_with`] with the enemy area defenses of a Quick
    /// Mission with no ground target (B23).
    pub fn for_mission_surface(
        resources: &dyn ResourceSource,
        code: &str,
        condition: Option<usize>,
        overrides: &Overrides,
        target: Option<&crate::surface::resolve::GroundTarget>,
        area: Option<&crate::surface::resolve::AreaDefense>,
    ) -> WorldResult<Self> {
        Self::build(resources, code, condition, None, overrides, target, area)
    }

    /// Rebuilds the world a mission recording was flown in from its recorded,
    /// resolved launch settings: layout, weather choice and layer, start time,
    /// wind, cloud deck and ground target. No override applies, so a replay
    /// looks the same whatever the viewer's settings are.
    #[allow(dead_code)] // Used by the mission replay viewer.
    pub fn for_recorded(resources: &dyn ResourceSource, recorded: &Recorded) -> WorldResult<Self> {
        Self::build(
            resources,
            &recorded.code,
            recorded.condition,
            Some(recorded),
            &Overrides::default(),
            recorded.target.as_ref(),
            recorded.area.as_ref(),
        )
    }

    fn build(
        resources: &dyn ResourceSource,
        code: &str,
        condition: Option<usize>,
        recorded: Option<&Recorded>,
        overrides: &Overrides,
        target: Option<&crate::surface::resolve::GroundTarget>,
        area: Option<&crate::surface::resolve::AreaDefense>,
    ) -> WorldResult<Self> {
        let probe = std::env::var_os("TORE_LOADING_PROFILE").is_some();
        let probe_started = probe.then(std::time::Instant::now);
        let required = |n: &str| {
            resources
                .get(n)
                .ok_or_else(|| format!("Missing {n}; re-import media with --import"))
        };
        let layout = format!("{}.MM", code.trim_end_matches(".MM"));
        let base =
            tore_formats::theater::base_theater(&layout).ok_or("unknown retail map layout")?;
        let mut environment = Environment::parse(required(&layout)?)?;
        if tore_formats::theater::base_theater(&environment.map) != Some(base) {
            return Err("layout and terrain identities disagree".into());
        }
        let grid = resources
            .get(&environment.map)
            .or_else(|| resources.get(&format!("{base}.T2")))
            .ok_or("missing base terrain grid")?;
        let mut theater = Theater::parse(grid)?;
        // The label comes from this mission's own theater only, so a bad
        // grid of another theater cannot fail this mission and a manifest
        // holds one grid, not sixteen.
        let id = code.trim_end_matches(".MM");
        let mut catalog = Vec::new();
        if let Some(label) = resources.theater_label(id)? {
            theater.name.clone_from(&label);
            catalog.push((id.to_owned(), label));
        }
        if theater.cols < 2 || theater.rows < 2 {
            return Err("unsupported theater dimensions".into());
        }
        let (layer, launch) = match condition {
            Some(index) => {
                let choice = tore_sim::environment::CONDITIONS
                    .get(index)
                    .ok_or("weather condition outside source table")?;
                (
                    tore_sim::environment::layer_resource(index, &format!("{base}.T2"))?,
                    Some([
                        choice.seconds_of_day / 3600,
                        choice.seconds_of_day / 60 % 60,
                    ]),
                )
            }
            None => (environment.layer.clone(), environment.time),
        };
        // A recording names the layer it resolved, so a later change to the
        // choice table cannot change an old replay's sky.
        let layer = recorded.and_then(|r| r.layer.clone()).unwrap_or(layer);
        let module = tore_formats::weather::Module::parse(required(&layer)?)?;
        let [hour, minute] = match recorded {
            Some(recorded) => recorded.time,
            None => overrides.time.or(launch).unwrap_or([12, 0]),
        };
        let wind = match recorded {
            Some(recorded) => recorded.wind,
            None => overrides.wind.or(environment.wind),
        };
        let mut configuration = tore_sim::environment::Configuration::new(
            module,
            hour,
            minute,
            condition.map_or_else(|| environment.layer_parameter.unwrap_or(0), |i| i as i32),
            wind,
        )?;
        if let Some(recorded) = recorded {
            configuration = configuration.with_weather_seed(recorded.weather_seed)?;
        }
        let weather = tore_sim::environment::Environment::new(configuration);
        if weather.sample(0.).is_none() {
            return Err("mission weather layer covers no altitude at its launch time".into());
        }
        // Preserve the resolved launch identity for validation and restart.
        environment.layer = layer;
        environment.layer_parameter = Some(weather.configuration().parameter());
        environment.time = Some([hour, minute]);
        let cloud_altitude = if let Some(recorded) = recorded {
            recorded.cloud_altitude
        } else if let Some(value) = overrides.cloud_altitude {
            if !(0..=400_000).contains(&value) {
                return Err("cloud altitude outside 0..400000 feet".into());
            }
            value
        } else if let Some(choice) = condition {
            tore_sim::clouds::generated_altitude(
                choice,
                &mut tore_formats::flight_model::clock_rng::NativeRng::seeded(1)?,
            )?
        } else {
            environment.clouds.unwrap_or(0)
        };
        if !(0..=400_000).contains(&cloud_altitude) {
            return Err("mission cloud altitude outside supported range".into());
        }
        environment.clouds = Some(cloud_altitude);
        let mut out = Self {
            foundations: Default::default(),
            theater,
            layout,
            condition,
            environment,
            airport_scene: tore_sim::airport::Scene::default(),
            placement_objects: Vec::new(),
            placement_runways: Vec::new(),
            airfield_anchors: BTreeMap::new(),
            static_manifest: Vec::new(),
            catalog,
            weather,
            surface: Default::default(),
            city_fill: None,
            redrawn: Vec::new(),
            redrawn_airports: false,
            redrawn_templates: false,
            merged_airports: false,
            neutral_airfield_ground: BTreeSet::new(),
        };
        out.redrawn_airports = recorded.map_or(overrides.redrawn_airports, |r| r.redrawn_airports);
        out.redrawn_templates =
            out.redrawn_airports && recorded.is_none_or(|r| r.redrawn_templates);
        let mut plans = redrawn::Plans::new(out.redrawn_airports)?;
        plans.retail_templates = !out.redrawn_templates;
        out.merged_airports = out.redrawn_airports && recorded.is_none_or(|r| r.merged_airports);
        plans.legacy_pairs = !out.merged_airports;
        out.build_airport_scene(
            resources,
            code.trim_end_matches(".MM"),
            target,
            area,
            &plans,
        )?;
        // Replace source land artwork, retaining the original coast masks in
        // the environment. Scenery repairs opaque markings while retaining those masks.
        for built in &out.redrawn {
            for p in &built.replaced_centres {
                let key = (
                    ((p[0] / f64::from(CELL_FEET)).floor() as i32) & !3,
                    ((p[2] / f64::from(CELL_FEET)).floor() as i32) & !3,
                );
                if out.environment.textures.contains_key(&key) {
                    out.neutral_airfield_ground.insert(key);
                }
            }
        }
        if let Some(started) = probe_started {
            eprintln!(
                "loading stage Terrain: {:.3} ms",
                started.elapsed().as_secs_f64() * 1000.
            );
        }
        if recorded.map_or(overrides.city_fill, |r| r.city_fill) {
            let cap = match recorded {
                Some(_) => None,
                None => overrides.city_fill_cap,
            };
            let city_started = probe.then(std::time::Instant::now);
            out.city_fill = crate::city::CityFill::for_terrain(
                resources,
                &out,
                cap.unwrap_or(crate::city::fill::MAX_INSTANCES),
            )
            .map(std::sync::Arc::new);
            if let Some(started) = city_started {
                let instances = out
                    .city_fill
                    .as_ref()
                    .map(|fill| fill.layout.instances.len())
                    .unwrap_or(0);
                eprintln!(
                    "loading stage CityFill: {:.3} ms instances={instances}",
                    started.elapsed().as_secs_f64() * 1000.
                );
            }
        }
        Ok(out)
    }

    /// The airport scene: every placement's contact volume, and each runway's
    /// surface, approach line and airfield anchors. The scenery builds the
    /// matching geometry from the same [`Placements`].
    fn build_airport_scene(
        &mut self,
        resources: &dyn ResourceSource,
        code: &str,
        target: Option<&crate::surface::resolve::GroundTarget>,
        area: Option<&crate::surface::resolve::AreaDefense>,
        plans: &redrawn::Plans,
    ) -> WorldResult<()> {
        use tore_sim::airport::{
            Airport, Allegiance, OrientedBox, Runway, SourceKey, StaticObject,
        };
        let mut sources = Placements::load(resources, code)?;
        let original_front = layout_front(&sources.layout);
        self.surface = Self::resolve_surface(resources, &sources.layout, target, area)?;
        self.place_surface(resources, &sources, code);
        // The template's placements join before the redraw, which redraws
        // a template's own runway piece too (B14).
        sources.add_surface(resources, &self.surface)?;
        // Experiment AP1: redrawn airports move and add their buildings
        // after the surface has placed, so its layout is the retail one.
        let mut parked = std::mem::take(&mut self.surface.parked);
        let built = redrawn::apply(
            resources,
            &mut sources,
            plans,
            &mut parked,
            |x, z| f64::from(self.height(x as f32, z as f32)),
            Some(self),
        );
        self.surface.parked = parked;
        self.redrawn = built?;
        // Keep the target identity while moving a template's airport buildings.
        for (id, p) in &sources.surface {
            if let Some((_, old)) = self
                .surface
                .placements
                .iter_mut()
                .find(|(old, _)| old.0 == *id)
            {
                *old = p.clone();
            }
            if let Some(unit) = self.surface.units.iter_mut().find(|u| u.id.0 == *id) {
                unit.position = p.position;
                unit.angles = p.angles;
            }
        }
        // Airport replacement changes obstacles and target positions. Resolve
        // the front and vehicle footing before the shared grounding pass.
        {
            use crate::surface::{
                catalog::Catalog,
                layout::{self, Ground, Inputs, Placed},
            };
            let mut types = type_infos(resources, &sources);
            let movable: BTreeSet<_> = self
                .surface
                .units
                .iter()
                .filter(|u| layout::footing::vehicle(u))
                .map(|u| u.id.0)
                .collect();
            let mut objects = Vec::new();
            let mut runways = Vec::new();
            for (id, p) in sources.placed().filter_map(Result::ok) {
                let info = types(&p.object_type);
                let placed = Placed {
                    id,
                    at: [i64::from(p.position[0]), i64::from(p.position[2])],
                    heading: p.angles[0],
                    footprint: info.footprint,
                };
                if info.strip {
                    runways.push(placed);
                } else if !movable.contains(&id) && !info.bridge_or_road {
                    objects.push(placed);
                }
            }
            let runways = placement_runways(&sources, &self.redrawn, runways);
            self.placement_objects = objects.clone();
            self.placement_runways = runways.clone();
            let ground = Ground::new(&self.theater, objects, runways, original_front)
                .with_tiles(sources.tiles.iter().copied());
            let mut catalog = Catalog::new(resources);
            let mut added = |name: &str| {
                placeable(resources, name)
                    .then(|| catalog.entry(name).ok())
                    .flatten()
            };
            layout::finish_layout(
                &mut self.surface,
                &mut Inputs {
                    ground: &ground,
                    types: &mut types,
                    added: &mut added,
                    layout: &format!("{code}.MM"),
                },
            );
        }
        // Refresh the scene inputs without retaining any previous front units.
        sources.surface.retain(|(id, _)| {
            crate::surface::UnitId(*id).range() != crate::surface::IdRange::FrontLine
        });
        for (id, p) in &self.surface.placements {
            sources.add_type(resources, &p.key.layout, &p.object_type)?;
            if let Some((_, old)) = sources.surface.iter_mut().find(|(old, _)| *old == id.0) {
                *old = p.clone();
            } else {
                sources.surface.push((id.0, p.clone()));
            }
        }
        for p in &mut sources.layout.placements {
            if let Some(u) = self
                .surface
                .units
                .iter()
                .find(|u| u.id == crate::surface::UnitId::layout(p.key.ordinal).unwrap())
            {
                p.position = u.position;
                p.angles = u.angles;
            }
        }
        // Keep the existing airport planes authoritative. A foundation beside
        // a runway must not put its buildings and vehicles on another surface.
        let airfields: Vec<_> = sources
            .placed()
            .filter_map(|placed| {
                let (id, p) = placed.ok()?;
                let stance = sources.stance(
                    p,
                    self.raw_height(p.position[0] as f32, p.position[2] as f32)
                        .into(),
                )?;
                if !stance.runway {
                    return None;
                }
                if self.redrawn.iter().any(|b| b.merged_from == Some(id)) {
                    return None;
                }
                Some(redrawn::Built::for_runway(&self.redrawn, id).map_or(
                    OrientedBox {
                        center: [stance.center[0], stance.support_origin[1], stance.center[2]],
                        half: stance.half,
                        heading: stance.heading,
                        pitch: stance.pitch,
                        bank: stance.bank,
                    },
                    |b| b.surface,
                ))
            })
            .collect();
        let (bridge_pads, bridge_banks) = crate::grounding::bridges::graded_foundations(
            &sources,
            |x, z| self.raw_height(x, z),
            |bounds| !airfields.iter().any(|field| bounds.meets_airfield(field)),
        );
        let pads = sources
            .placed()
            .filter_map(|placed| {
                let (_, p) = placed.ok()?;
                sources.foundation(p)
            })
            .chain(bridge_pads.iter().copied())
            .collect();
        self.foundations = crate::grounding::pads::Mesh::build_with_banks(
            pads,
            self.theater.cols,
            self.theater.rows,
            |x, z| self.raw_height(x, z),
            |pad| {
                airfields
                    .iter()
                    .find(|field| pad.meets_airfield(field))
                    .map(|field| field.center[1])
                    .or_else(|| {
                        crate::grounding::bridges::support(pad, &bridge_pads, |x, z| {
                            self.raw_height(x, z)
                        })
                    })
            },
            &bridge_banks,
        );
        let mut objects = Vec::new();
        let mut slope_lifts = BTreeMap::new();
        let mut runways = Vec::new();
        let mut airports = Vec::new();
        let mut aliases = BTreeMap::new();
        let mut airport_ordinals = BTreeMap::new();
        for placed in sources.placed() {
            let (id, p) = placed?;
            if sources.definitions[&p.object_type]
                .callbacks
                .iter()
                .any(|c| c == "_STRIPProc")
            {
                airport_ordinals.insert(id, airport_ordinals.len() as u32 + 1);
            }
        }
        for built in &self.redrawn {
            if let Some(old) = built.merged_from {
                aliases.insert(old, built.strip_id);
                aliases.insert(airport_ordinals[&old], airport_ordinals[&built.strip_id]);
            }
        }
        let mut anchors = BTreeMap::new();
        for placed in sources.placed() {
            let (id, placement) = placed?;
            if aliases.contains_key(&id) {
                continue;
            }
            let definition = &sources.definitions[&placement.object_type];
            self.static_manifest.push((
                id,
                placement.key.clone(),
                placement.object_type.clone(),
                sources.shapes.contains_key(&placement.object_type),
            ));
            if !sources.shapes.contains_key(&placement.object_type) {
                continue;
            }
            let ground = f64::from(
                self.raw_height(placement.position[0] as f32, placement.position[2] as f32),
            );
            let Some(stance) = self.placement_stance(&sources, placement) else {
                continue;
            };
            let Stance {
                min,
                max,
                half,
                heading,
                pitch,
                bank,
                basis,
                support_origin,
                center,
                runway,
                slope_lift,
                ..
            } = stance;
            slope_lifts.insert(id, slope_lift);
            let redraw = redrawn::Built::for_runway(&self.redrawn, id);
            // A redrawn airport's contact box is its own whole airfield.
            let bounds = redraw.map_or(
                OrientedBox {
                    center,
                    half,
                    heading,
                    pitch,
                    bank,
                },
                |built| built.surface,
            );
            objects.push(StaticObject {
                id,
                source: SourceKey {
                    layout: placement.key.layout.clone(),
                    ordinal: placement.key.ordinal,
                },
                name: placement
                    .name
                    .clone()
                    .unwrap_or_else(|| definition.display_name.clone()),
                object_type: placement.object_type.clone(),
                bounds,
                hit_points: definition.hit_points.unwrap_or(100),
                runway,
                category: definition.category,
                radar_signature: f64::from(definition.radar_signature),
                infrared_signature: f64::from(definition.infrared_signature),
                type_flags: definition.flags,
                max_visible: definition.max_visible,
            });
            if runway {
                let airport_id = airport_ordinals[&id];
                // The whole airport mesh includes aprons and parallel strips.
                // Use source anchor0x11 for the fitted primary approach line,
                // rather than steering onto the overall mesh's midpoint.
                let mut approach_center = center;
                // Even a fallback centerline belongs to the plane through the
                // placement origin, not the whole airport mesh's vertical center.
                if basis.up[1].abs() > 1e-6 {
                    approach_center[1] = support_origin[1]
                        - (basis.up[0] * (center[0] - support_origin[0])
                            + basis.up[2] * (center[2] - support_origin[2]))
                            / basis.up[1];
                }
                let anchor = sources.runway_anchors.get(&placement.object_type);
                let mut length_ft = runway_length_ft(min[2], max[2], anchor.map(|a| a[2]));
                let mut runway_heading = heading;
                if let Some(anchor) = anchor
                    && anchor[2] < max[2]
                    && anchor[2] >= min[2]
                {
                    let local = [anchor[0], 0.0, (anchor[2] + max[2]) * 0.5];
                    approach_center = std::array::from_fn(|axis| {
                        support_origin[axis]
                            + basis.right[axis] * local[0]
                            + basis.forward[axis] * local[2]
                    });
                }
                // A redrawn second tile flies from its own main runway.
                if let Some(line) = redraw.and_then(|built| built.runway_line) {
                    approach_center = line.center;
                    runway_heading = line.heading;
                    length_ft = line.length;
                }
                // A redrawn airport's anchors are its plan's, or none.
                let found = match redraw {
                    Some(built) => built.anchors,
                    None => sources
                        .strip_boxes
                        .get(&placement.object_type)
                        .and_then(|boxes| {
                            airfield_anchors(boxes, heading, |local| {
                                std::array::from_fn(|axis| {
                                    support_origin[axis]
                                        + basis.right[axis] * local[0]
                                        + basis.forward[axis] * local[2]
                                })
                            })
                        }),
                };
                if let Some(found) = found {
                    anchors.insert(id, found);
                }
                runways.push(Runway {
                    object: id,
                    airport: airport_id,
                    name: placement
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Runway {airport_id}")),
                    surface: bounds,
                    approach_center,
                    // ILS datum remains authored airport ground, independent of rendering bias.
                    elevation_ft: redraw
                        .filter(|b| b.merged_from.is_some())
                        .map_or(ground, |b| b.surface.center[1]),
                    heading: runway_heading,
                    length_ft,
                });
                airports.push(Airport {
                    id: airport_id,
                    name: placement
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Airport {airport_id}")),
                    runway_objects: vec![id],
                    // The runway's layout owner (slice AL1, John 2026-10-10):
                    // Blue's field is friendly, Redfor's hostile, and one
                    // with no owner field neutral. Neutral fields grant
                    // permission to both sides (agent decision, AL1).
                    allegiance: Allegiance::of_owner(placement.redfor()),
                    neutral_permission: true,
                });
            }
        }
        // A unit is in the scene, and so a combat target, when its shape gave
        // it a contact volume.
        let placed: BTreeSet<u32> = objects.iter().map(|o: &StaticObject| o.id).collect();
        for unit in &mut self.surface.units {
            unit.in_scene = placed.contains(&unit.id.0);
        }
        // A unit that follows a route takes its hit box from the scene's.
        let mut courses = std::mem::take(&mut self.surface.courses);
        for (id, course) in &mut courses {
            if let Some(object) = objects.iter().find(|object| object.id == id.0) {
                let ground = f64::from(self.height(course.start[0] as f32, course.start[2] as f32));
                // The hit box rides on the unit's own shape; a standing
                // unit's slope lift is the mover's to find as it settles.
                let lift = slope_lifts.get(&id.0).copied().unwrap_or(0.);
                course.fit(&object.bounds, ground + lift);
                if let Some(shape) = sources.shapes.get(&object.object_type) {
                    let scale = sources
                        .shape_scales
                        .get(&object.object_type)
                        .copied()
                        .unwrap_or(1.0);
                    course.footing = crate::grounding::UnitFooting::of(shape, scale);
                }
            }
        }
        // Units that share a route drive side by side, not on one line.
        crate::surface::movement::set_lanes(&mut courses);
        self.surface.courses = courses;
        // The parked aircraft stand on the ground or their carrier's deck.
        let (parked, unreadable) =
            crate::surface::parked::place(resources, &self.surface, &|x, z| {
                f64::from(self.height(x as f32, z as f32))
            });
        self.surface.parked_scene = parked;
        self.surface.unreadable.extend(unreadable);
        self.airport_scene = tore_sim::airport::Scene {
            objects,
            runways,
            airports,
            aliases,
        };
        // Every point must stand on the airport's landable surface.
        let scene = &self.airport_scene;
        anchors.retain(|_, found: &mut tore_sim::ai::airfield::AirfieldAnchors| {
            anchor_points(found).all(|p| scene.runway_surface(p[0], p[2]).is_some())
        });
        self.airfield_anchors = anchors;
        // Airport replacement can move target structures and parked aircraft.
        // The mission must follow their final positions and integer runway origins.
        let mut types = type_infos(resources, &sources);
        let mut ground = surface_ground(&self.theater, &sources, &mut types);
        ground.front = original_front;
        for (id, p) in &sources.surface {
            let info = types(&p.object_type);
            if info.strip {
                ground.runways.push(crate::surface::layout::Placed {
                    id: *id,
                    at: [i64::from(p.position[0]), i64::from(p.position[2])],
                    heading: p.angles[0],
                    footprint: info.footprint,
                });
            }
        }
        self.surface.starts =
            crate::surface::layout::scene_starts(&self.surface, &ground, &self.airport_scene);
        // What the armed units fight with, read once from their records.
        let arsenal = crate::surface::fire::Arsenal::load(&self.surface, resources, &|x, z| {
            f64::from(self.height(x as f32, z as f32))
        });
        self.surface.arsenal = arsenal;
        self.airport_scene.validate().map_err(|error| error.into())
    }

    /// Places the resolved surface on this terrain: the template's
    /// relocation and jitter, the batteries, the added trucks and radars and
    /// the starts ([`crate::surface::layout`]), from integer data only.
    fn place_surface(&mut self, resources: &dyn ResourceSource, sources: &Placements, code: &str) {
        use crate::surface::{
            catalog::Catalog,
            layout::{self, Inputs},
        };
        let mut types = type_infos(resources, sources);
        let Self {
            theater, surface, ..
        } = self;
        let ground = surface_ground(theater, sources, &mut types);
        let mut catalog = Catalog::new(resources);
        let mut added = |name: &str| {
            placeable(resources, name)
                .then(|| catalog.entry(name).ok())
                .flatten()
        };
        let layout_name = format!("{code}.MM");
        layout::place(
            surface,
            &mut Inputs {
                ground: &ground,
                types: &mut types,
                added: &mut added,
                layout: &layout_name,
            },
        );
    }

    /// The site rules the placed surface breaks, against the same ground
    /// the layout placed it on: empty for a template that stayed or a
    /// relocation that holds (the `surface-relocate-sweep` scenario).
    pub fn audit_surface(&self, resources: &dyn ResourceSource) -> WorldResult<Vec<String>> {
        let sources = Placements::load(resources, self.layout.trim_end_matches(".MM"))?;
        let mut types = type_infos(resources, &sources);
        let ground = surface_ground(&self.theater, &sources, &mut types);
        Ok(crate::surface::layout::audit(
            &self.surface,
            &ground,
            &mut types,
        ))
    }

    /// [`crate::surface::layout::class_audit`] against the ground this
    /// mission's surface was placed on.
    pub fn audit_classes(&self, resources: &dyn ResourceSource) -> WorldResult<Vec<String>> {
        let sources = Placements::load(resources, self.layout.trim_end_matches(".MM"))?;
        let mut types = type_infos(resources, &sources);
        let ground = surface_ground(&self.theater, &sources, &mut types);
        Ok(crate::surface::layout::class_audit(
            &self.surface,
            &ground,
            None,
        ))
    }

    /// The surface a layout and an optional ground target resolve to.
    fn resolve_surface(
        resources: &dyn ResourceSource,
        layout: &tore_formats::mission::Layout,
        target: Option<&crate::surface::resolve::GroundTarget>,
        area: Option<&crate::surface::resolve::AreaDefense>,
    ) -> WorldResult<crate::surface::Surface> {
        use crate::surface::{catalog::Catalog, resolve};
        let mut catalog = Catalog::new(resources);
        let base = resolve::layout(layout, &mut catalog);
        let mut unresolved = None;
        let template = match target {
            Some(target) => {
                let name = format!("~{}.M", target.stem.to_ascii_uppercase());
                match resources.get(&name) {
                    Some(bytes) => {
                        let template = tore_formats::quick_template::Template::parse(&name, bytes)?;
                        Some(resolve::template(
                            &template,
                            target,
                            &mut catalog,
                            layout.map.as_deref(),
                        )?)
                    }
                    // An import made before it kept the templates: the
                    // mission flies, and says why nothing stands there.
                    None => {
                        unresolved = Some(format!(
                            "the import has no ground target template {name}; re-import media"
                        ));
                        None
                    }
                }
            }
            None => None,
        };
        let mut surface = resolve::surface(base, template)?;
        surface.unresolved = unresolved;
        // Area defenses only without a ground target (never both).
        if surface.template.is_none() && surface.unresolved.is_none() {
            surface.area = area.cloned();
        }
        // The units that follow a route (Quick Mission columns and ships).
        for unit in surface.units.iter().filter(|unit| unit.route.is_some()) {
            if let Ok(entry) = catalog.entry(&unit.resource)
                && let Some(record) = &entry.unit
                && let Some(course) = crate::surface::movement::Course::of(unit, record)
            {
                surface.courses.insert(unit.id, course);
            }
        }
        Ok(surface)
    }

    /// The AI's view of one runway, with its airfield points when known.
    pub fn runway_view(&self, object: u32) -> Option<tore_sim::ai::airfield::RunwayView> {
        self.airport_scene.runway(object).map(|runway| {
            tore_sim::ai::airfield::RunwayView::from(runway)
                .with_anchors(self.airfield_anchors.get(&runway.object).copied())
        })
    }

    /// How far a point is beyond the edge of the theater's map rectangle, in
    /// nautical miles, measured from the nearest point of the rectangle; zero
    /// inside it. Terrain runs from 0 to `(cells - 1) * cell` on each axis.
    pub fn edge_distance_nm(&self, x: f64, z: f64) -> f64 {
        edge_distance_nm(self.theater.cols, self.theater.rows, x, z)
    }
    /// The mission's steady wind in world feet per second.
    pub fn wind(&self) -> [f64; 3] {
        self.weather.configuration().wind_world_fps()
    }

    /// Native T_Info/Collision publishes whether the winning terrain class is 1.
    /// This host samples the T2 cell under the aircraft; native object/carrier
    /// collision overrides and triangle-boundary parity remain unimplemented.
    pub fn turbulence_reduced_surface(&self, x: f64, z: f64) -> bool {
        let col = (x / f64::from(CELL_FEET))
            .floor()
            .clamp(0., (self.theater.cols - 1) as f64) as usize;
        let row = (z / f64::from(CELL_FEET))
            .floor()
            .clamp(0., (self.theater.rows - 1) as f64) as usize;
        self.theater.cell(col, row).class == 1
    }

    /// Whether the T2 cell under the point is water: terrain class 1, the
    /// class the original's collision query reports as water. Outside the grid
    /// the original's fallback cell is water too. See
    /// docs/formats/native-land-contact.md.
    pub fn over_water(&self, x: f64, z: f64) -> bool {
        let col = (x / f64::from(CELL_FEET)).floor();
        let row = (z / f64::from(CELL_FEET)).floor();
        if col < 0.
            || row < 0.
            || col >= self.theater.cols as f64
            || row >= self.theater.rows as f64
        {
            return true;
        }
        self.theater.cell(col as usize, row as usize).class == 1
    }

    /// Whether the terrain is drawn as open water under the point: the T2
    /// cell is water class, or its colour is the water cutout (255) that the
    /// renderer leaves to the ocean pass. The airport audit uses it to find
    /// pavement drawn over the sea.
    pub fn drawn_as_water(&self, x: f64, z: f64) -> bool {
        self.over_water(x, z) || {
            let col = (x / f64::from(CELL_FEET)).floor() as usize;
            let row = (z / f64::from(CELL_FEET)).floor() as usize;
            self.theater.cell(col, row).color == 255
        }
    }

    /// Explicit authored standard atmosphere, shared wind and rendered terrain.
    /// No weather-derived temperature/pressure is inferred from LAY colors.
    pub fn air_data(
        &self,
        state: &tore_sim::flight::State,
    ) -> tore_formats::Result<tore_sim::telemetry::AirData> {
        tore_sim::telemetry::AirData::sample(
            state,
            tore_sim::telemetry::EnvironmentReading {
                terrain_msl_ft: f64::from(
                    self.height(state.position[0] as f32, state.position[2] as f32),
                ),
                wind_world_fps: self.wind(),
                atmosphere: tore_sim::telemetry::Atmosphere::standard(state.position[1])?,
            },
        )
    }

    /// The terrain surface plus the environment's wind, for one fixed step.
    pub fn surface(&self, x: f64, z: f64) -> tore_sim::research::Surface {
        if let Some((_, height)) = self.airport_scene.runway_surface(x, z) {
            let mut surface = tore_sim::research::Surface::runway(height);
            surface.wind = self.wind();
            return surface;
        }
        let mut surface =
            tore_sim::research::Surface::terrain(f64::from(self.height(x as f32, z as f32)));
        surface.wind = self.wind();
        surface.water = self.over_water(x, z);
        surface.slope_deg = self.slope_degrees(x, z);
        surface
    }

    /// Exactly `self.surface(x, z).height`, without the wind, water and
    /// slope the full surface also works out. The AI reads only the height
    /// for its ground samples and sight lines.
    pub fn surface_height(&self, x: f64, z: f64) -> f64 {
        if let Some((_, height)) = self.airport_scene.runway_surface(x, z) {
            return height;
        }
        f64::from(self.height(x as f32, z as f32))
    }

    /// The surface under `(x, z)` for an aircraft whose lowest point is at
    /// `bottom` (feet): [`Self::surface`], or, when a standing object's flat
    /// top (`alive` ids: a building roof, a hangar, a bunker, a deck) or a
    /// city fill building's roof lies at or below `bottom`, that top as a
    /// level, non-landable, dry surface. Only aircraft that may touch down
    /// away from an airfield ask this (docs/spec/touchdown.md); a fixed-wing
    /// aircraft uses [`Self::surface`] and crashes into the object as ever.
    pub fn surface_over(
        &self,
        x: f64,
        z: f64,
        bottom: f64,
        alive: &BTreeSet<u32>,
    ) -> tore_sim::research::Surface {
        let mut surface = self.surface(x, z);
        let object = self
            .airport_scene
            .flat_top_below(x, z, bottom, |id| alive.contains(&id));
        let roof = self
            .city_fill
            .as_ref()
            .and_then(|fill| fill.roof_below(x, z, bottom));
        if let Some(top) = [object, roof]
            .into_iter()
            .flatten()
            .max_by(f64::total_cmp)
            // On an airfield only a real structure counts, so a low marking
            // or pad object never turns the runway into limited ground.
            .filter(|top| {
                *top > surface.height
                    + if surface.landable {
                        AIRFIELD_ROOF_RISE_FT
                    } else {
                        0.
                    }
            })
        {
            surface.height = top;
            surface.landable = false;
            surface.water = false;
            surface.slope_deg = 0.;
        }
        surface
    }

    /// How far the ground under `(x, z)` tilts from level, degrees: the
    /// slope of the terrain triangle there ([`Self::height`] is planar over
    /// each half of a cell).
    pub fn slope_degrees(&self, x: f64, z: f64) -> f64 {
        let cell = CELL_FEET;
        let fx = (x as f32 / cell).clamp(0.0, (self.theater.cols - 1) as f32 - 0.001);
        let fy = (z as f32 / cell).clamp(0.0, (self.theater.rows - 1) as f32 - 0.001);
        let (ix, iy) = (fx as usize, fy as usize);
        let (u, v) = (fx - ix as f32, fy - iy as f32);
        let h = |dx, dy| self.theater.cell(ix + dx, iy + dy).elevation as f32 * HEIGHT_FEET;
        let (dhx, dhz) = if u + v <= 1.0 {
            (h(1, 0) - h(0, 0), h(0, 1) - h(0, 0))
        } else {
            (h(1, 1) - h(0, 1), h(1, 1) - h(1, 0))
        };
        (f64::from(dhx).hypot(f64::from(dhz)) / f64::from(cell))
            .atan()
            .to_degrees()
    }

    /// Earliest solid building contact. Runways remain a separate surface query.
    pub fn solid_contact(
        &self,
        from: [f64; 3],
        to: [f64; 3],
        alive: impl IntoIterator<Item = u32>,
    ) -> Option<(u32, f64)> {
        let mut alive: Vec<u32> = alive.into_iter().collect();
        alive.sort_unstable();
        self.airport_scene
            .objects
            .iter()
            .filter(|object| !object.runway && alive.binary_search(&object.id).is_ok())
            // A box the segment cannot reach at all is not tested exactly,
            // which needs the box's trigonometry.
            .filter(|object| !segment_misses_sphere(&object.bounds, from, to))
            .filter_map(|object| {
                object
                    .bounds
                    .segment_fraction(from, to)
                    .map(|at| (object.id, at))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
    }

    /// Whether the segment `from` to `to` enters a standing city fill
    /// building below its roof; always false without a fill.
    pub fn fill_contact(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        self.city_fill
            .as_ref()
            .is_some_and(|fill| fill.contact(from, to).is_some())
    }

    /// Where a free flight and the free camera start: over the Ukraine
    /// default in the Ukraine theaters and over the middle of the grid in
    /// the others, at 28,000 feet or 3,000 feet above the ground there,
    /// whichever is higher.
    pub fn free_flight_start(&self) -> [f64; 3] {
        let mut position = UKRAINE_START;
        if tore_formats::theater::base_theater(&self.layout) != Some("UKR") {
            position = [
                f64::from((self.theater.cols as f32 - 1.0) * CELL_FEET * 0.5),
                28000.0,
                f64::from((self.theater.rows as f32 - 1.0) * CELL_FEET * 0.5),
            ];
        }
        let ground = self.height(position[0] as f32, position[2] as f32);
        position[1] = position[1].max(f64::from(ground + 3000.0));
        position
    }

    /// Where an airborne mission's lead starts, and its heading in radians
    /// when the mission's ground target placed it (docs/spec/surface-defenses.md,
    /// "Start placement": Blue the enemy distance from Red, heading at the target);
    /// otherwise the free-flight start with no heading of its own.
    pub fn airborne_start(&self) -> ([f64; 3], Option<f64>) {
        let mut position = self.free_flight_start();
        let Some(starts) = &self.surface.starts else {
            return (position, None);
        };
        position[0] = f64::from(starts.blue[0]);
        position[2] = f64::from(starts.blue[1]);
        let ground = self.height(position[0] as f32, position[2] as f32);
        position[1] = 28_000f64.max(f64::from(ground + 3000.0));
        (
            position,
            Some(f64::from(starts.blue_heading_deg).to_radians()),
        )
    }

    /// The highest [`Self::height`] or [`Self::surface`] answers over each
    /// square of the terrain grid: the square's highest corner cell (heights
    /// blend them) or the highest runway surface over it.
    pub fn ground_ceiling(&self) -> tore_sim::ground_ceiling::GroundCeiling {
        let theater = &self.theater;
        let mut ceiling = tore_sim::ground_ceiling::GroundCeiling::from_grid(
            theater.cols,
            theater.rows,
            f64::from(CELL_FEET),
            |col, row| f64::from(f32::from(theater.cell(col, row).elevation) * HEIGHT_FEET),
        );
        for (&(x, z), triangles) in &self.foundations.cells {
            let top = triangles
                .iter()
                .flatten()
                .map(|p| f64::from(p[1]))
                .fold(f64::NEG_INFINITY, f64::max);
            ceiling.raise(
                [
                    x as f64 * f64::from(CELL_FEET),
                    z as f64 * f64::from(CELL_FEET),
                ],
                [
                    (x + 1) as f64 * f64::from(CELL_FEET),
                    (z + 1) as f64 * f64::from(CELL_FEET),
                ],
                top,
            );
        }
        for runway in &self.airport_scene.runways {
            if let Some((lo, hi, top)) = runway.surface_extent() {
                ceiling.raise(lo, hi, top);
            }
        }
        ceiling
    }

    pub fn height(&self, x: f32, z: f32) -> f32 {
        self.foundations
            .height(x, z)
            .unwrap_or_else(|| self.raw_height(x, z))
    }

    /// Imported terrain before static foundations, for audits and construction.
    pub fn raw_height(&self, x: f32, z: f32) -> f32 {
        let fx = (x / CELL_FEET).clamp(0.0, (self.theater.cols - 1) as f32 - 0.001);
        let fy = (z / CELL_FEET).clamp(0.0, (self.theater.rows - 1) as f32 - 0.001);
        let (ix, iy) = (fx as usize, fy as usize);
        let (u, v) = (fx - ix as f32, fy - iy as f32);
        let h = |dx, dy| self.theater.cell(ix + dx, iy + dy).elevation as f32 * HEIGHT_FEET;
        if u + v <= 1.0 {
            h(0, 0) + (h(1, 0) - h(0, 0)) * u + (h(0, 1) - h(0, 0)) * v
        } else {
            h(1, 1) + (h(0, 1) - h(1, 1)) * (1.0 - u) + (h(1, 0) - h(1, 1)) * (1.0 - v)
        }
    }
}
/// Distance beyond the edge of a `cols` by `rows` terrain grid, in nautical
/// miles, from the nearest point of the map rectangle (zero inside it).
pub fn edge_distance_nm(cols: usize, rows: usize, x: f64, z: f64) -> f64 {
    let cell = f64::from(tore_formats::theater::CELL_FEET);
    let extent = |cells: usize| cells.saturating_sub(1) as f64 * cell;
    let beyond = |value: f64, extent: f64| (0. - value).max(value - extent).max(0.);
    beyond(x, extent(cols)).hypot(beyond(z, extent(rows))) / FEET_PER_NAUTICAL_MILE
}

/// Whether the segment `from` to `to` is certainly clear of `bounds`: it
/// passes farther from the box's centre than the box's half diagonal (a
/// sphere that holds the box at any heading), by a margin far above rounding.
/// A segment this says nothing of may still miss the box, which the exact
/// test settles; a segment it clears can never touch it.
fn segment_misses_sphere(
    bounds: &tore_sim::airport::OrientedBox,
    from: [f64; 3],
    to: [f64; 3],
) -> bool {
    let along: [f64; 3] = std::array::from_fn(|i| to[i] - from[i]);
    let centre: [f64; 3] = std::array::from_fn(|i| bounds.center[i] - from[i]);
    let length_squared: f64 = along.iter().map(|v| v * v).sum();
    let at = if length_squared > 0. {
        (centre.iter().zip(&along).map(|(c, a)| c * a).sum::<f64>() / length_squared).clamp(0., 1.)
    } else {
        0.
    };
    let distance_squared: f64 = (0..3)
        .map(|i| {
            let d = centre[i] - along[i] * at;
            d * d
        })
        .sum();
    let reach = bounds.half.iter().map(|h| h * h).sum::<f64>().sqrt() + 1.;
    // Anything not finite is left to the exact test.
    distance_squared.is_finite() && reach.is_finite() && distance_squared > reach * reach
}

/// A roof on an airfield's box must stand this high above the runway plane to
/// be a landing surface of its own, feet. Fitted (agent decision).
const AIRFIELD_ROOF_RISE_FT: f64 = 8.;

/// Feet in a nautical mile.
pub const FEET_PER_NAUTICAL_MILE: f64 = 6_076.115_49;
/// The player is warned to turn back this far beyond the theater's edge, and any
/// aircraft is lost 5 nautical miles further out. `opinionated` (requested by
/// John, 2026-09-29); the distances and the unit are agent decisions.
pub const EDGE_WARNING_NM: f64 = 100.;
pub const EDGE_DESTROY_NM: f64 = 105.;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_runway_is_measured_from_the_anchor_to_the_far_end() {
        // The anchor inside the shape: the run from it to the far end.
        assert_eq!(runway_length_ft(-600., 600., Some(-537.)), 1_137.);
        // No anchor, or one outside the shape: the whole extent.
        assert_eq!(runway_length_ft(-600., 600., None), 1_200.);
        assert_eq!(runway_length_ft(-600., 600., Some(700.)), 1_200.);
        assert_eq!(runway_length_ft(-600., 600., Some(-700.)), 1_200.);
        // The far end itself is outside, the near end inside.
        assert_eq!(runway_length_ft(-600., 600., Some(600.)), 1_200.);
        assert_eq!(runway_length_ft(-600., 600., Some(-600.)), 1_200.);
        // The smallest extent the scene gives a runway is two feet.
        assert_eq!(runway_length_ft(0., 0.5, None), 2.);
    }
    #[test]
    fn strip_length_needs_an_airport_definition_in_the_resources() {
        assert_eq!(strip_length_ft(&BTreeMap::new(), "AIRPORT.OT"), None);
        let junk = BTreeMap::from([("AIRPORT.OT".to_string(), vec![0u8; 8])]);
        assert_eq!(strip_length_ft(&junk, "AIRPORT.OT"), None);
    }
    fn definition(resource: &str, callback: &str) -> tore_formats::static_object::Definition {
        tore_formats::static_object::Definition {
            display_name: String::new(),
            class_name: String::new(),
            resource_name: resource.to_owned(),
            main_shape: None,
            callbacks: vec![callback.to_owned()],
            hit_points: None,
            category: 0,
            radar_signature: 0,
            infrared_signature: 0,
            explosion: 0,
            crater: 0,
            flags: 0,
            max_visible: 0,
        }
    }

    /// A data module whose CODE section is a shape header with exponent `e`.
    fn shape_with_exponent(e: u16) -> Vec<u8> {
        let mut code = vec![0u8; 16];
        code[6..8].copy_from_slice(&e.to_le_bytes());
        let mut b = vec![0u8; 256 + code.len()];
        b[..2].copy_from_slice(b"MZ");
        b[60..64].copy_from_slice(&64u32.to_le_bytes());
        b[64..68].copy_from_slice(b"PL\0\0");
        b[68..70].copy_from_slice(&0x14cu16.to_le_bytes());
        b[70..72].copy_from_slice(&1u16.to_le_bytes());
        b[84..86].copy_from_slice(&32u16.to_le_bytes());
        b[120..124].copy_from_slice(b"CODE");
        for (off, v) in [(8, code.len()), (12, 4096), (16, code.len()), (20, 256)] {
            b[120 + off..124 + off].copy_from_slice(&(v as u32).to_le_bytes());
        }
        b[256..].copy_from_slice(&code);
        b
    }

    #[test]
    fn placed_objects_are_real_size_and_map_tied_ones_keep_the_shape_scale() {
        use PlacedSize::*;
        // Runways by their callback, whatever their name.
        assert_eq!(
            PlacedSize::of(&definition("STRIP.OT", "_STRIPProc")),
            MapTied
        );
        assert_eq!(
            PlacedSize::of(&definition("MYFIELD.OT", "_STRIPProc")),
            MapTied
        );
        // Bridges and roads by name, in any case.
        for name in MAP_TIED_TYPES {
            assert_eq!(PlacedSize::of(&definition(name, "_OBJProc")), MapTied);
        }
        assert_eq!(
            PlacedSize::of(&definition("brdmid.ot", "_OBJProc")),
            MapTied
        );
        // Buildings, city blocks and surface units are drawn at real size.
        for (name, callback) in [
            ("HANGR.OT", "_OBJProc"),
            ("CTYBKA.OT", "_OBJProc"),
            ("SA6.NT", "_GVProc"),
            ("NIMITZ.NT", "_CARRIERProc"),
        ] {
            assert_eq!(PlacedSize::of(&definition(name, callback)), RealSize);
        }
        // HANGR.SH (e 10): 4 ft a unit in retail, 4/3 here; RUNWAY.SH keeps 4.
        let e10 = shape_with_exponent(10);
        let hangar = placed_shape_scale(&definition("HANGR.OT", "_OBJProc"), &e10).unwrap();
        assert!((hangar - 4. / 3.).abs() < 1e-12);
        let runway = placed_shape_scale(&definition("STRIP.OT", "_STRIPProc"), &e10).unwrap();
        assert_eq!(runway, 4.);
        // A record length in retail feet follows the same factor.
        assert!((RealSize.feet(-225.) + 75.).abs() < 1e-12);
        assert_eq!(MapTied.feet(-225.), -225.);
    }

    #[test]
    fn the_map_edge_distance_is_measured_from_the_nearest_point_of_the_rectangle() {
        // 209 by 201 cells of 8,192 ft: the map runs 0..1,703,936 by 0..1,638,400.
        let d = |x: f64, z: f64| edge_distance_nm(209, 201, x, z);
        assert_eq!(d(0., 0.), 0.);
        assert_eq!(d(1_703_936., 1_638_400.), 0.);
        assert_eq!(d(800_000., 800_000.), 0.);
        let nm = FEET_PER_NAUTICAL_MILE;
        assert!((d(-100. * nm, 800_000.) - 100.).abs() < 1e-9);
        assert!((d(1_703_936. + 105. * nm, 0.) - 105.).abs() < 1e-9);
        assert!((d(800_000., 1_638_400. + 3. * nm) - 3.).abs() < 1e-9);
        // Past a corner the nearest point is the corner: 3-4-5 miles.
        assert!((d(-3. * nm, -4. * nm) - 5.).abs() < 1e-9);
    }
    use crate::test_support::terrain as world;
    #[test]
    fn authored_support_planes_do_not_drift_with_foundation_terrain() {
        let mut t = world();
        t.foundations = crate::grounding::pads::Mesh::build(
            vec![crate::grounding::pads::Pad {
                min: [1000., 1000.],
                max: [3000., 3000.],
                height: 0.,
            }],
            2,
            2,
            |x, z| t.raw_height(x, z),
        );
        let p = tore_formats::mission::Layout::parse(
            "T.MM",
            b"textFormat\nobj\n\ttype STRIP.OT\n\tpos 1100 0 1100\n\t.\n",
        )
        .unwrap()
        .placements
        .remove(0);
        let mut sources = Placements::empty();
        sources
            .definitions
            .insert("STRIP.OT".into(), definition("STRIP.OT", "_STRIPProc"));
        sources.shapes.insert(
            "STRIP.OT".into(),
            tore_formats::shape::Shape {
                faces: Vec::new(),
                lines: Vec::new(),
                state_words: Default::default(),
                billboards: vec![tore_formats::shape::Billboard {
                    center: [0., 0., 0.],
                    size: [300., 20.],
                    texture: String::new(),
                    uv: None,
                    fog: Default::default(),
                    address: 0,
                }],
            },
        );
        assert_ne!(t.height(1100., 1100.), t.raw_height(1100., 1100.));
        let stance = t.placement_stance(&sources, &p).unwrap();
        assert_eq!(
            stance.support_origin[1],
            f64::from(t.raw_height(1100., 1100.))
        );
    }

    #[test]
    fn only_fixed_surface_records_can_make_permanent_foundations() {
        let mut sources = Placements::empty();
        let p = tore_formats::mission::Layout::parse(
            "T.MM",
            b"textFormat\nobj\n\ttype TEST.NT\n\tpos 1000 0 1000\n\t.\n",
        )
        .unwrap()
        .placements
        .remove(0);
        sources
            .definitions
            .insert("TEST.NT".into(), definition("TEST.NT", "_NTProc"));
        sources.shapes.insert(
            "TEST.NT".into(),
            tore_formats::shape::Shape {
                faces: Vec::new(),
                lines: Vec::new(),
                state_words: Default::default(),
                billboards: vec![tore_formats::shape::Billboard {
                    center: [0., 0., 0.],
                    size: [300., 20.],
                    texture: String::new(),
                    uv: None,
                    fog: Default::default(),
                    address: 0,
                }],
            },
        );
        assert!(
            sources.foundation(&p).is_none(),
            "a mobile unit must not level its starting position"
        );
        sources.fixed_units.insert("TEST.NT".into());
        assert!(
            sources.foundation(&p).is_some(),
            "a fixed site uses the same footprint rule as a building"
        );
    }

    #[test]
    fn dominant_pavement_not_roof_controls_grounding() {
        use tore_formats::shape::{Face, FogMode, Shape};
        let face = |width: f32, length: f32, height: f32| Face {
            positions: vec![
                [0., 0., height],
                [width, 0., height],
                [width, length, height],
                [0., length, height],
            ],
            colors: vec![1; 4],
            fog: FogMode::Enabled,
            uv: vec![],
            texture: String::new(),
            subtype: 0x59,
            normal: None,
            address: 0,
        };
        let shape = Shape {
            billboards: Vec::new(),
            lines: vec![],
            faces: vec![
                face(40., 100., -1.),
                face(40., 100., -1.),
                face(50., 100., 20.),
            ],
            state_words: Default::default(),
        };
        assert_eq!(pavement_height(&shape), -1.);
        assert_eq!(
            pavement_height(&Shape {
                billboards: Vec::new(),
                lines: vec![],
                faces: vec![],
                state_words: Default::default()
            }),
            0.
        );
    }
    #[test]
    fn turbulence_surface_uses_class_not_color_or_height() {
        let mut w = world();
        assert!(!w.turbulence_reduced_surface(0., 0.));
        w.theater.cells[0].class = 1;
        assert!(w.turbulence_reduced_surface(0., 0.));
        w.theater.cells[0].color = 255;
        w.theater.cells[0].elevation = 200;
        assert!(w.turbulence_reduced_surface(0., 0.));
        assert!(!w.turbulence_reduced_surface(f64::from(CELL_FEET), 0.));
    }

    #[test]
    fn strip_boxes_become_world_airfield_points() {
        use tore_formats::shape::ContactBox;
        let at = |id: u8, x: i16, z: i16| ContactBox {
            flags: 0xc0,
            id,
            pairs: [[x, x], [0, 32], [z - 10, z + 10]],
        };
        let mut boxes: Vec<_> = (0x19..=0x21)
            .map(|id| at(id, 2688, -1266 + 100 * i16::from(id - 0x19)))
            .collect();
        boxes.extend([
            at(0x11, -1723, -952),
            at(0x12, -1723, -1110),
            at(0x25, 2208, -794),
            at(0x26, 1412, -1326),
            at(0x27, -308, -1326),
            at(0x28, -1723, -1326),
            at(0x29, -1723, 1912),
            at(0x2a, 288, 1912),
            at(0x2b, 288, -1326),
            at(0x2c, 2220, -1326),
        ]);
        // An east-facing field placed at (10000, 500, 20000): local forward is
        // world +X and local right is world -Z.
        let heading = std::f64::consts::FRAC_PI_2;
        let basis = tore_sim::attitude::Basis::new(heading, 0., 0.);
        let place = |local: [f64; 3]| -> [f64; 3] {
            std::array::from_fn(|axis| {
                [10_000., 500., 20_000.][axis]
                    + basis.right[axis] * local[0]
                    + basis.forward[axis] * local[2]
            })
        };
        let anchors = airfield_anchors(&boxes, heading, place).unwrap();
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 1e-6);
        assert!(near(
            anchors.takeoff_spot,
            [10_000. - 952., 500., 20_000. + 1723.]
        ));
        assert!(near(
            anchors.landing_point,
            [10_000. - 1110., 500., 20_000. + 1723.]
        ));
        assert!(near(
            anchors.taxi_out[0],
            [10_000. - 794., 500., 20_000. - 2208.]
        ));
        assert!(near(
            anchors.taxi_in[3],
            [10_000. - 1326., 500., 20_000. - 2220.]
        ));
        assert!(near(
            anchors.parking[8],
            [10_000. - 466., 500., 20_000. - 2688.]
        ));
        assert_eq!(anchors.takeoff_heading, heading);
        assert_eq!(anchors.landing_heading, heading);
        assert_eq!(anchors.parking_heading, std::f64::consts::PI);
        assert_eq!(anchor_points(&anchors).count(), 19);
        // Any missing point means the field has no usable anchors.
        boxes.retain(|b| b.id != 0x21);
        assert!(airfield_anchors(&boxes, heading, place).is_none());
    }

    #[test]
    fn solid_contact_is_separate_from_runway_surface_and_respects_health_ids() {
        let mut world = world();
        world
            .airport_scene
            .objects
            .push(tore_sim::airport::StaticObject {
                id: 100,
                source: tore_sim::airport::SourceKey {
                    layout: "T.MM".into(),
                    ordinal: 0,
                },
                name: "Hangar".into(),
                object_type: "HANGR.OT".into(),
                bounds: tore_sim::airport::OrientedBox {
                    center: [50.0, 10.0, 50.0],
                    half: [10.0; 3],
                    heading: 0.0,
                    pitch: 0.0,
                    bank: 0.0,
                },
                hit_points: 100,
                runway: false,
                category: 0x2000,
                radar_signature: 1.0,
                infrared_signature: 0.0,
                type_flags: 0,
                max_visible: 0,
            });
        assert_eq!(
            world
                .solid_contact([0.0, 10.0, 50.0], [100.0, 10.0, 50.0], [100])
                .map(|hit| hit.0),
            Some(100)
        );
        assert!(
            world
                .solid_contact([0.0, 10.0, 50.0], [100.0, 10.0, 50.0], [])
                .is_none()
        );
        assert!(!world.surface(50.0, 50.0).landable);
    }

    /// `solid_contact` as it was: a set of ids, the exact test on each.
    fn solid_contact_reference(
        world: &Terrain,
        from: [f64; 3],
        to: [f64; 3],
        alive: impl IntoIterator<Item = u32>,
    ) -> Option<(u32, f64)> {
        let alive: BTreeSet<_> = alive.into_iter().collect();
        world
            .airport_scene
            .objects
            .iter()
            .filter(|object| !object.runway && alive.contains(&object.id))
            .filter_map(|object| {
                object
                    .bounds
                    .segment_fraction(from, to)
                    .map(|at| (object.id, at))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
    }

    #[test]
    fn solid_contact_finds_what_the_exact_test_on_every_live_object_found() {
        let mut world = world();
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1_u64 << 53) as f64
        };
        for id in 0..120 {
            world
                .airport_scene
                .objects
                .push(tore_sim::airport::StaticObject {
                    id,
                    source: tore_sim::airport::SourceKey {
                        layout: "T.MM".into(),
                        ordinal: id,
                    },
                    name: "Hangar".into(),
                    object_type: "HANGR.OT".into(),
                    bounds: tore_sim::airport::OrientedBox {
                        center: [next() * 4000., next() * 300., next() * 4000.],
                        half: [3. + next() * 60., 3. + next() * 40., 3. + next() * 90.],
                        heading: next() * 6.3,
                        pitch: (next() - 0.5) * 0.6,
                        bank: (next() - 0.5) * 0.6,
                    },
                    hit_points: 100,
                    runway: id % 17 == 0,
                    category: 0x2000,
                    radar_signature: 1.0,
                    infrared_signature: 0.0,
                    type_flags: 0,
                    max_visible: 0,
                });
        }
        let mut hits = 0;
        for _ in 0..4000 {
            let from = [next() * 4000., next() * 300., next() * 4000.];
            let to = if next() < 0.1 {
                from
            } else {
                [
                    from[0] + (next() - 0.5) * 900.,
                    from[1] + (next() - 0.5) * 200.,
                    from[2] + (next() - 0.5) * 900.,
                ]
            };
            let alive: Vec<u32> = (0..140).filter(|_| next() < 0.7).collect();
            let old = solid_contact_reference(&world, from, to, alive.iter().copied());
            let new = world.solid_contact(from, to, alive.iter().copied());
            assert_eq!(new, old, "{from:?} to {to:?}");
            hits += usize::from(new.is_some());
        }
        assert!(hits > 50, "only {hits} of the segments touched a box");
    }

    #[test]
    fn slope_water_and_roofs_reach_the_surface() {
        let mut w = world();
        // The fixture's four cells rise 1,024 and 2,048 ft across 8,192 ft in
        // both triangles: atan(2290 / 8192), about 15.6 degrees.
        let tilt = 1024f64.hypot(2048.) / f64::from(CELL_FEET);
        let tilt = tilt.atan().to_degrees();
        for (x, z) in [(100., 100.), (7_000., 7_000.), (3_000., 500.)] {
            assert!((w.slope_degrees(x, z) - tilt).abs() < 1e-6, "{x} {z}");
            assert!((w.surface(x, z).slope_deg - tilt).abs() < 1e-6);
        }
        for cell in &mut w.theater.cells {
            cell.elevation = 4;
        }
        assert_eq!(w.slope_degrees(100., 100.), 0.);
        // Water is the class-1 cell, and everything past the grid.
        assert!(!w.surface(100., 100.).water);
        w.theater.cells[0].class = 1;
        assert!(w.surface(100., 100.).water);
        assert!(w.surface(-5., 100.).water);
        w.theater.cells[0].class = 2;
        // A box roof is a level, dry, non-landable surface for an aircraft
        // above it, and nothing for one beside or below it.
        let ground = f64::from(w.height(2_000., 3_000.));
        w.airport_scene
            .objects
            .push(tore_sim::airport::StaticObject {
                id: 5,
                source: tore_sim::airport::SourceKey {
                    layout: "T.MM".into(),
                    ordinal: 0,
                },
                name: "Hangar".into(),
                object_type: "HANGR.OT".into(),
                bounds: tore_sim::airport::OrientedBox {
                    center: [2_000., ground + 20., 3_000.],
                    half: [60., 20., 60.],
                    heading: 0.,
                    pitch: 0.,
                    bank: 0.,
                },
                hit_points: 100,
                runway: false,
                category: 0x2000,
                radar_signature: 1.,
                infrared_signature: 0.,
                type_flags: 0,
                max_visible: 0,
            });
        let alive: BTreeSet<u32> = [5].into();
        let roof = w.surface_over(2_030., 2_980., ground + 45., &alive);
        assert_eq!(roof.height, ground + 40.);
        assert!(!roof.landable && !roof.water && roof.slope_deg == 0.);
        // Below the roof, off the box, or with the object destroyed: ground.
        assert_eq!(
            w.surface_over(2_030., 2_980., ground + 30., &alive).height,
            ground
        );
        assert_eq!(
            w.surface_over(2_100., 2_980., ground + 45., &alive).height,
            ground
        );
        assert_eq!(
            w.surface_over(2_030., 2_980., ground + 45., &BTreeSet::new())
                .height,
            ground
        );
    }

    #[test]
    fn height_matches_triangle_corners_and_center() {
        let w = world();
        assert_eq!(w.height(0.0, 0.0), 0.0);
        assert!((w.height(CELL_FEET / 2.0, CELL_FEET / 2.0) - 1536.0).abs() < 0.01);
    }

    /// Wherever the ceiling calls a sight line clear, every point of it is
    /// above the ground the AI is given, on and off the grid and over a
    /// sloped runway.
    #[test]
    fn the_ground_ceiling_never_clears_a_line_through_the_ground() {
        use tore_formats::theater::TerrainCell;
        use tore_sim::airport::{OrientedBox, Runway};
        let mut seed = 0x51_7cc1_b727_220a_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut w = world();
        let (cols, rows) = (12, 9);
        w.theater.cols = cols;
        w.theater.rows = rows;
        w.theater.cells = (0..cols * rows)
            .map(|_| TerrainCell {
                color: 100,
                class: 2,
                elevation: (next() % 32) as u8,
            })
            .collect();
        let surface = OrientedBox {
            center: [
                3.3 * f64::from(CELL_FEET),
                9_000.,
                4.1 * f64::from(CELL_FEET),
            ],
            half: [150., 20., 6_000.],
            heading: 0.6,
            pitch: 0.03,
            bank: 0.01,
        };
        w.airport_scene.runways.push(Runway {
            object: 1,
            airport: 1,
            name: "R".into(),
            surface,
            approach_center: surface.center,
            elevation_ft: 9_000.,
            heading: 0.6,
            length_ft: 12_000.,
        });
        let ceiling = w.ground_ceiling();
        // Only the runway, standing above the highest hill (31 steps of 256
        // feet), keeps this line from being clear.
        let over = |height: f64| {
            let c = surface.center;
            ceiling.clear_above([c[0] - 100., height, c[2]], [c[0] + 100., height, c[2]])
        };
        assert!(!over(8_500.));
        assert!(over(9_500.));
        let span = f64::from(CELL_FEET) * 14.;
        let mut cleared = 0;
        for _ in 0..20_000 {
            let mut point = || {
                [
                    (next() % 1_000_000) as f64 / 1e6 * span - f64::from(CELL_FEET),
                    (next() % 12_000) as f64,
                    (next() % 1_000_000) as f64 / 1e6 * span - f64::from(CELL_FEET),
                ]
            };
            let a = point();
            let mut b = point();
            // Mostly short lines, as a pilot sees a tracer nearby.
            if next() % 4 != 0 {
                b = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * 0.05);
            }
            if !ceiling.clear_above(a, b) {
                continue;
            }
            cleared += 1;
            for step in 0..=64 {
                let t = f64::from(step) / 64.;
                let p: [f64; 3] = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
                let terrain = f64::from(w.height(p[0] as f32, p[2] as f32));
                let ground = w.surface(p[0], p[2]).height;
                assert!(p[1] > terrain && p[1] > ground, "{a:?} {b:?} at {t}");
            }
        }
        assert!(
            cleared > 1_000,
            "too few lines cleared to mean anything: {cleared}"
        );
    }

    /// The AI's height-only ground answers exactly what the full surface
    /// does, bit for bit: on and off the grid, on a sloped runway, along its
    /// edges and beyond the map.
    #[test]
    fn the_surface_height_is_the_full_surface_height_bit_for_bit() {
        use tore_formats::theater::TerrainCell;
        use tore_sim::airport::{OrientedBox, Runway};
        let mut seed = 0x2f6b_93d1_04c7_e851_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut w = world();
        let (cols, rows) = (10, 8);
        w.theater.cols = cols;
        w.theater.rows = rows;
        w.theater.cells = (0..cols * rows)
            .map(|_| TerrainCell {
                color: 100,
                class: if next() % 5 == 0 { 1 } else { 2 },
                elevation: (next() % 40) as u8,
            })
            .collect();
        let surface = OrientedBox {
            center: [
                3.7 * f64::from(CELL_FEET),
                2_000.,
                4.2 * f64::from(CELL_FEET),
            ],
            half: [150., 20., 6_000.],
            heading: 0.6,
            pitch: 0.03,
            bank: 0.01,
        };
        w.airport_scene.runways.push(Runway {
            object: 1,
            airport: 1,
            name: "R".into(),
            surface,
            approach_center: surface.center,
            elevation_ft: 2_000.,
            heading: 0.6,
            length_ft: 12_000.,
        });
        let same = |x: f64, z: f64| {
            assert_eq!(
                w.surface_height(x, z).to_bits(),
                w.surface(x, z).height.to_bits(),
                "{x} {z}"
            );
        };
        let span = f64::from(CELL_FEET) * 12.;
        let mut on_runway = 0;
        for k in 0..40_000 {
            let unit = |value: u64| (value % 1_000_000) as f64 / 1e6;
            // Half over the whole map, half around the runway.
            let (x, z) = if k % 2 == 0 {
                (
                    unit(next()) * span - f64::from(CELL_FEET),
                    unit(next()) * span - f64::from(CELL_FEET),
                )
            } else {
                (
                    surface.center[0] + (unit(next()) - 0.5) * 14_000.,
                    surface.center[2] + (unit(next()) - 0.5) * 14_000.,
                )
            };
            same(x, z);
            on_runway += usize::from(w.airport_scene.runway_surface(x, z).is_some());
        }
        assert!(on_runway > 20, "too few samples on the runway: {on_runway}");
        // Grid points, cell edges and points just either side of them.
        for col in 0..=cols + 1 {
            for row in 0..=rows + 1 {
                let (x, z) = (
                    col as f64 * f64::from(CELL_FEET),
                    row as f64 * f64::from(CELL_FEET),
                );
                for (dx, dz) in [(0., 0.), (-1e-6, 0.), (0., 1e-6), (0.5, -0.5)] {
                    same(x + dx, z + dz);
                }
            }
        }
        // Along and across the runway's edges, a little inside and outside.
        let (sin, cos) = surface.heading.sin_cos();
        for step in -70..=70 {
            let along = f64::from(step) * 100.;
            for across in [-150.01, -150., -149.99, 0., 149.99, 150., 150.01] {
                for end in [0_f64, 5_999.99, 6_000., 6_000.01] {
                    let a = if end == 0. {
                        along
                    } else {
                        end.copysign(along)
                    };
                    same(
                        surface.center[0] + a * sin + across * cos,
                        surface.center[2] + a * cos - across * sin,
                    );
                }
            }
        }
        // Far beyond the map and odd values.
        for (x, z) in [(-1e7, -1e7), (1e7, 3e6), (f64::MAX, 0.), (0., -0.)] {
            same(x, z);
        }
    }
}
