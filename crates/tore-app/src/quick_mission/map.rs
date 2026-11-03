//! The creator's Map (slice CL-R, John 2026-10-11): "a map button that
//! shows a screen like the in-game map and shows the bluefor and redfor
//! starting locations", with the objective as an approximate area, "so the
//! player knows roughly where to go without giving the targets away".
//!
//! The positions are the flight's own: the map builds the mission the
//! creator would fly ([`World::build`]) and reads where it put the player,
//! each AI wing and the ground target. It draws them with
//! [`crate::flight_map::PlanView`], the in-game map's look.
use super::QuickMission;
use crate::flight_map::{Plan, PlanMarker, PlanSide, PlanView};
use crate::menu::Sprite;
use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};
use tore_formats::font::Font;
use tore_sim::ai::launch::Side;
use tore_sim::sensors::FEET_PER_NAUTICAL_MILE as NMI;
use tore_world::ai_wings::AiFlightModel;
use tore_world::mission::MissionSpec;
use tore_world::terrain::Terrain;
use tore_world::world::{Hooks, Seating, World};

/// The objective area's radius, nm. `fitted` (agent decision, slice CL-R,
/// 2026-10-11), for John's opinionated rule of the same day: the objective
/// shows as an area, not the targets' positions.
pub const OBJECTIVE_RADIUS_NM: f64 = 5.;
/// The most the area's centre is moved off the targets' centroid, nm, so the
/// targets are inside the area but not at its centre. `fitted`, as above.
pub const OBJECTIVE_OFFSET_NM: f64 = 2.5;
/// A wing that starts within this many nm of the side's start is not drawn
/// again. `fitted` (agent decision, slice CL-R, 2026-10-11).
const SAME_PLACE_NM: f64 = 3.;
/// Said on the lobby map, which uses the creator's audited placement.
pub const LOBBY_MAP_NOTE: &str =
    "The host keeps this mission's checked placement. The target area is approximate.";

/// The open map: the mission's terrain, what it shows, its view and font.
pub struct Planning {
    terrain: Terrain,
    plan: Plan,
    view: PlanView,
    font: Font,
    title: String,
    close: (i32, i32, i32, i32),
    hover: bool,
    pressed: bool,
}

impl Planning {
    pub fn pointer(&mut self, point: Option<(f64, f64)>) {
        self.view.pointer(point);
        let (x, y, w, h) = self.close;
        self.hover = point.is_some_and(|(px, py)| {
            px >= f64::from(x)
                && py >= f64::from(y)
                && px < f64::from(x + w)
                && py < f64::from(y + h)
        });
    }
    pub fn down(&mut self) {
        // The scroll bars and the zoom rocker take a press first.
        self.pressed = !self.view.down() && self.hover;
    }
    /// The Close button was clicked: pressed and released on it.
    pub fn up(&mut self) -> bool {
        if self.view.up() {
            self.pressed = false;
            return false;
        }
        std::mem::take(&mut self.pressed) && self.hover
    }
    pub fn key(&mut self, key: &str) {
        self.view.key(key);
    }
    /// Draws the map; true while the zoom rocker is still tilting.
    pub fn render(&mut self, pixels: &mut [u8], sprites: &BTreeMap<String, Sprite>) -> bool {
        let animating = self.view.advance(std::time::Instant::now());
        self.close = self.view.draw(
            pixels,
            &self.terrain,
            &self.font,
            sprites,
            &self.plan,
            &self.title,
            self.pressed && self.hover,
        );
        animating
    }
    /// What the map shows, for tests and the snapshot's log.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }
}

/// A map being built on its own thread: the mission it was started for (a
/// result for any other is stale and ignored), and where the answer arrives.
pub struct Building {
    spec: MissionSpec,
    result: mpsc::Receiver<Result<Planning, String>>,
}

/// How a build under way stands.
pub enum Progress {
    Working,
    Done(Box<Planning>),
    Failed(String),
}

impl Building {
    /// A build whose answer the caller sends (tests).
    #[cfg(test)]
    pub(super) fn from_channel(
        spec: MissionSpec,
        result: mpsc::Receiver<Result<Planning, String>>,
    ) -> Self {
        Self { spec, result }
    }
    /// Starts building the map of `spec` on a thread of its own. The build is
    /// the whole mission ([`World::build`]), which takes seconds on a large
    /// theater, so it must not run on the thread that draws the menu.
    fn start(
        resources: Arc<BTreeMap<String, Vec<u8>>>,
        spec: MissionSpec,
        lobby: bool,
    ) -> Result<Self, String> {
        let (send, result) = mpsc::channel();
        let job = spec.clone();
        std::thread::Builder::new()
            .name("tore-map-build".into())
            .spawn(move || {
                // A closed creator drops the receiver; the answer is unwanted.
                let _ = send.send(build(&resources, &job, lobby));
            })
            .map_err(|error| format!("The map could not be started: {error}"))?;
        Ok(Self { spec, result })
    }
    /// The answer, if it has arrived.
    fn poll(&self) -> Progress {
        match self.result.try_recv() {
            Ok(Ok(planning)) => Progress::Done(Box::new(planning)),
            Ok(Err(words)) => Progress::Failed(words),
            Err(mpsc::TryRecvError::Empty) => Progress::Working,
            Err(mpsc::TryRecvError::Disconnected) => {
                Progress::Failed("The map could not be built.".into())
            }
        }
    }
    /// Waits for the answer (the headless snapshot has nothing else to do).
    fn wait(&self) -> Progress {
        match self.result.recv() {
            Ok(Ok(planning)) => Progress::Done(Box::new(planning)),
            Ok(Err(words)) => Progress::Failed(words),
            Err(_) => Progress::Failed("The map could not be built.".into()),
        }
    }
}

/// A deterministic 64-bit mix (SplitMix64's finaliser).
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The objective area for targets whose centroid is `centroid` (east and
/// north feet): a circle of [`OBJECTIVE_RADIUS_NM`] whose centre is moved up
/// to [`OBJECTIVE_OFFSET_NM`] in a direction and by a distance drawn from
/// the theater, the target's stem and the surface seed, so the same mission
/// always shows the same circle. Returns the centre and the radius in feet.
pub fn objective_area(centroid: [f64; 2], theater: &str, stem: &str, seed: u32) -> ([f64; 2], f64) {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in theater.bytes().chain([0]).chain(stem.bytes()) {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let draw = mix(hash ^ u64::from(seed));
    let angle = (draw & 0xffff_ffff) as f64 / 4_294_967_296. * std::f64::consts::TAU;
    // At least a fifth of the offset, so the targets are never at the centre.
    let distance = (0.2 + 0.8 * ((draw >> 32) as f64 / 4_294_967_296.)) * OBJECTIVE_OFFSET_NM * NMI;
    (
        [
            centroid[0] + angle.sin() * distance,
            centroid[1] + angle.cos() * distance,
        ],
        OBJECTIVE_RADIUS_NM * NMI,
    )
}

/// The airfield a ground start stands on: the nearest runway's, with its
/// runway objects.
fn airfield_at(terrain: &Terrain, position: [f64; 3]) -> Option<(String, Vec<u32>)> {
    let scene = &terrain.airport_scene;
    let runway = scene.runways.iter().min_by(|a, b| {
        let d = |r: &tore_sim::airport::Runway| {
            (r.surface.center[0] - position[0]).hypot(r.surface.center[2] - position[2])
        };
        d(a).total_cmp(&d(b))
    })?;
    let airport = scene.airports.iter().find(|a| a.id == runway.airport)?;
    Some((airport.name.clone(), airport.runway_objects.clone()))
}

fn distance_nm(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[2] - b[2]) / NMI
}

/// What the creator says while the map is being built.
pub const BUILDING_NOTE: &str = "Building mission map...";

impl QuickMission {
    /// Starts building the map of the mission the draft describes, or says in
    /// the notice why it cannot. The map opens when the build is done
    /// ([`Self::poll_map`]); until then the creator stays usable and says
    /// "Building mission map...".
    pub(super) fn open_map(&mut self) {
        self.map_build = None;
        match self.start_map() {
            Ok(building) => self.map_build = Some(building),
            Err(words) => self.notice = Some(words),
        }
    }

    /// The mission the map shows for the draft as it stands now, and whether
    /// it is the lobby's or the read-only view's. Both keep the audited seed.
    ///
    /// Single player draws the flight's seed now, as OK would, and OK keeps
    /// it, so the flight flies the layout the map shows.
    pub(super) fn map_mission(&mut self) -> Result<(MissionSpec, bool), String> {
        let (mut spec, lobby) = self.map_spec()?;
        if spec.lays_out_surface() && !lobby && !self.seed_pending {
            self.surface_seed = super::fresh_seed();
            self.seed_pending = true;
            spec.surface_seed = self.surface_seed;
        }
        Ok((spec, lobby))
    }

    /// [`Self::map_mission`] without drawing a seed: with none pending, a
    /// single-player spec carries the seed last drawn. For comparing a
    /// finished build against the draft, which must change nothing.
    fn map_spec(&self) -> Result<(MissionSpec, bool), String> {
        if let Some(problem) = self.unsupported() {
            return Err(problem);
        }
        let mut spec = self.mission_spec()?;
        spec.ai_flight_model = AiFlightModel::Standard;
        let lobby = self.lobby || self.view.is_some();
        if spec.lays_out_surface() {
            spec.surface_seed = self.surface_seed;
        }
        Ok((spec, lobby))
    }

    fn start_map(&mut self) -> Result<Building, String> {
        let resources = self
            .resources
            .clone()
            .ok_or("The map needs the imported game data.")?;
        let (spec, lobby) = self.map_mission()?;
        Building::start(resources, spec, lobby)
    }

    /// True while a map is being built.
    pub fn map_building(&self) -> bool {
        self.map_build.is_some()
    }

    /// Takes the finished map, if the build is done: opens it, or says why it
    /// failed. A build for settings that have changed since is dropped and
    /// the map is built again for the settings now in force, so the map that
    /// opens is always the draft's.
    pub(super) fn poll_map(&mut self) {
        let Some(building) = &self.map_build else {
            return;
        };
        // The "map-building" snapshot holds the answer back to show the note.
        if self.map_hold {
            return;
        }
        let progress = building.poll();
        self.finish_map(progress);
    }

    /// Waits for the map being built and opens it (the headless snapshot).
    pub(super) fn wait_for_map(&mut self) {
        if let Some(building) = &self.map_build {
            let progress = building.wait();
            self.finish_map(progress);
        }
    }

    fn finish_map(&mut self, progress: Progress) {
        if matches!(progress, Progress::Working) {
            return;
        }
        let building = self.map_build.take();
        let Some(building) = building else {
            return;
        };
        // Compared without drawing a seed: a finished build must not change
        // the draft it is being checked against.
        let current = self.map_spec().is_ok_and(|(spec, _)| spec == building.spec);
        if !current {
            // The settings changed while it was built: this one is stale.
            // The player asked for a map, so ask again for the new settings.
            self.open_map();
            return;
        }
        match progress {
            Progress::Done(planning) => {
                self.cancel();
                self.map = Some(planning);
            }
            Progress::Failed(words) => self.notice = Some(words),
            Progress::Working => {}
        }
    }
}

/// Builds the mission of `spec` and reads the starts and the objective from
/// it: the map's whole content. Deterministic for the same `spec`.
fn build(
    resources: &BTreeMap<String, Vec<u8>>,
    spec: &MissionSpec,
    lobby: bool,
) -> Result<Planning, String> {
    let built = World::build(
        spec,
        resources,
        Seating::SinglePlayer,
        &mut Hooks::default(),
    )
    .map_err(|error| format!("The map could not be built: {error}"))?;
    let world = built.world;
    let font = resources
        .get("WIN11.FNT")
        .and_then(|bytes| Font::parse(bytes).ok())
        .ok_or("The map needs WIN11.FNT; re-import the game.")?;
    let plan = plan_of(&world, spec, lobby);
    let view = PlanView::new(&plan, &world.terrain);
    let title = world.terrain.theater.name.clone();
    Ok(Planning {
        terrain: world.terrain,
        plan,
        view,
        font,
        title,
        close: (0, 0, 0, 0),
        hover: false,
        pressed: false,
    })
}

/// What the map shows of a built mission (the creator's map: the starts, the
/// approximate objective and the line below the plot).
fn plan_of(world: &World, spec: &tore_world::mission::MissionSpec, lobby: bool) -> Plan {
    mission_plan(world, lobby, Some(spec.start.altitude_ft()))
}

/// The mission's markup, read from a world just built and not yet flown: the
/// Blue and Red starts (an airfield or an airborne point), the other wings'
/// starts where they differ, and the approximate objective area. The creator's
/// map draws it, and so does the in-flight map (slice CL-MAP, John
/// 2026-10-12: "map mark up specific to the mission"), so the two always show
/// the same mission the same way. Everything comes from the world's mission
/// data, which every machine of a networked game builds the same, so the
/// markup is the same on all of them, and in a replay.
///
/// `lobby` adds the checked-placement note to the markup. `altitude_ft` is the creator's start
/// altitude, for the line under the plot; the flight has no such line.
pub fn mission_plan(world: &World, lobby: bool, altitude_ft: Option<u32>) -> Plan {
    let mut plan = Plan::default();
    let terrain = &world.terrain;
    let ground_label = |side: &str, position: [f64; 3], plan: &mut Plan| -> String {
        match airfield_at(terrain, position) {
            Some((name, runways)) => {
                plan.start_fields.extend(runways);
                format!("{side} START: {}", name.to_ascii_uppercase())
            }
            None => format!("{side} START"),
        }
    };
    // The AI wings' leads, by side and wing.
    let mut leads: Vec<(Side, u8, [f64; 3], f64)> = Vec::new();
    if let Some(ai) = &world.ai_wings {
        for slot in ai.slots() {
            if leads
                .iter()
                .any(|(side, wing, ..)| *side == slot.side && *wing == slot.wing_number)
            {
                continue;
            }
            if let Some(actor) = ai.mission().actor(slot.id) {
                let flight = actor.flight();
                leads.push((slot.side, slot.wing_number, flight.position, flight.yaw));
            }
        }
    }
    leads.sort_by_key(|(side, wing, ..)| (*side == Side::Enemy, *wing));
    // Blue: the player's own start, which leads friendly wing 1 (an open
    // mission has no cockpit until a human takes a plane: the wing's lead).
    let mut blue_words = String::from("BLUE: AIRBORNE");
    let blue = world
        .cockpits
        .first()
        .map(|cockpit| (cockpit.flight.position, cockpit.flight.yaw))
        .or_else(|| {
            leads
                .iter()
                .find(|(side, wing, ..)| *side == Side::Friendly && *wing == 1)
                .map(|(_, _, position, heading)| (*position, *heading))
        });
    if let Some((position, heading)) = blue {
        let label = if world.setup.ground_start.is_some() {
            blue_words = "BLUE: ON THE GROUND".into();
            ground_label("BLUE", position, &mut plan)
        } else {
            if let Some(feet) = altitude_ft {
                blue_words = format!("BLUE: AIRBORNE AT {feet} FT");
            }
            "BLUE START".into()
        };
        plan.markers.push(PlanMarker {
            position,
            heading,
            side: PlanSide::Blue,
            label,
        });
    }
    let blue_start = plan.markers.first().map(|m| m.position);
    for (_, wing, position, heading) in leads.iter().filter(|(side, ..)| *side == Side::Friendly) {
        if *wing == 1 || blue_start.is_some_and(|at| distance_nm(at, *position) < SAME_PLACE_NM) {
            continue;
        }
        plan.markers.push(PlanMarker {
            position: *position,
            heading: *heading,
            side: PlanSide::Blue,
            label: format!("BLUE WING {wing}"),
        });
    }
    // Red: Enemy Wing 1's start (parked at its field for an enemy ground
    // start), else the first enemy wing's; the other wings where they differ.
    let mut red_words = String::from("RED: NONE");
    let mut red_start: Option<[f64; 3]> = None;
    for (_, wing, position, heading) in leads.iter().filter(|(side, ..)| *side == Side::Enemy) {
        let label = match red_start {
            None => {
                red_start = Some(*position);
                if *wing == 1 && world.setup.enemy_ground_start.is_some() {
                    red_words = "RED: ON THE GROUND".into();
                    ground_label("RED", *position, &mut plan)
                } else {
                    red_words = "RED: AIRBORNE".into();
                    "RED START".into()
                }
            }
            Some(at) if distance_nm(at, *position) < SAME_PLACE_NM => continue,
            Some(_) => format!("RED WING {wing}"),
        };
        plan.markers.push(PlanMarker {
            position: *position,
            heading: *heading,
            side: PlanSide::Red,
            label,
        });
    }
    // The objective as an area around the targets, never the targets.
    let surface = &terrain.surface;
    let has_target = surface.template.is_some() || surface.unresolved.is_some();
    if let Some(site) = &surface.template {
        let centroid = surface
            .starts
            .as_ref()
            .map(|starts| [f64::from(starts.target[0]), f64::from(starts.target[1])])
            .or_else(|| {
                let points: Vec<[f64; 2]> = surface
                    .units
                    .iter()
                    .filter(|u| u.is_target())
                    .map(|u| u.position)
                    .chain(
                        surface
                            .parked
                            .iter()
                            .filter(|p| p.target)
                            .map(|p| p.position),
                    )
                    .map(|p| [f64::from(p[0]), f64::from(p[2])])
                    .collect();
                (!points.is_empty()).then(|| {
                    let n = points.len() as f64;
                    [
                        points.iter().map(|p| p[0]).sum::<f64>() / n,
                        points.iter().map(|p| p[1]).sum::<f64>() / n,
                    ]
                })
            });
        // The draw hashes the theater's code, the template's stem and the
        // surface seed, all of which the mission itself carries.
        let theater = terrain.layout.trim_end_matches(".MM");
        if let (None, Some(centroid)) = (&surface.unresolved, centroid) {
            plan.objective = Some(objective_area(
                centroid,
                theater,
                &site.stem,
                site.settings.seed,
            ));
        }
    }
    if has_target && plan.objective.is_none() {
        plan.note = Some(
            "The ground target is not in this import, so the map has no objective area.".into(),
        );
    }
    if lobby && has_target && plan.note.is_none() {
        plan.note = Some(LOBBY_MAP_NOTE.into());
    }
    plan.summary = format!("{blue_words}   {red_words}");
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_objective_area_holds_the_targets_off_its_centre_and_is_the_same_each_time() {
        for (theater, stem, seed) in [
            ("UKR", "QUSFLT", 0),
            ("EGY", "QEARMOR", 7),
            ("UKR", "QUSFLT", 99),
        ] {
            let centroid = [1_000_000., 2_000_000.];
            let (center, radius) = objective_area(centroid, theater, stem, seed);
            assert_eq!(
                objective_area(centroid, theater, stem, seed),
                (center, radius)
            );
            assert_eq!(radius, 5. * NMI);
            let off = (center[0] - centroid[0]).hypot(center[1] - centroid[1]) / NMI;
            assert!((0.5..=2.5).contains(&off), "{theater} {stem} {seed}: {off}");
        }
        // Another seed or target moves it.
        let a = objective_area([0.; 2], "UKR", "QUSFLT", 1).0;
        let b = objective_area([0.; 2], "UKR", "QUSFLT", 2).0;
        let c = objective_area([0.; 2], "UKR", "QULFLT", 1).0;
        assert!(a != b && a != c);
    }

    /// The synthetic mission the markup tests build: a ground target with
    /// the starts that come with it, and an enemy wing.
    fn mission(seating: Seating) -> World {
        use tore_world::test_support::surface::{spec_with_target, surface_resources, target};
        let mut spec = spec_with_target(&target("QUCITY", 3, 3, 9));
        spec.wings[3].count = 2;
        World::new(&spec, &surface_resources(), seating).unwrap()
    }

    #[test]
    fn the_flights_markup_is_the_creators_map_of_the_same_mission() {
        // Single player and a networked game build the same mission; both
        // show the same circle, on the same stem, theater and seed, and the
        // same starts.
        let single = mission(Seating::SinglePlayer);
        let open = mission(Seating::Open);
        let creator = plan_of(
            &single,
            &MissionSpec::new("UKR", tore_formats::aircraft::AircraftId::F18),
            false,
        );
        let flight = mission_plan(&single, false, None);
        let networked = mission_plan(&open, false, None);
        let (center, radius) = flight.objective.expect("a ground target has an area");
        assert_eq!(radius, OBJECTIVE_RADIUS_NM * NMI);
        assert_eq!(creator.objective, flight.objective);
        assert_eq!(networked.objective, flight.objective);
        assert_eq!(creator.markers, flight.markers);
        assert_eq!(networked.markers.len(), flight.markers.len());
        // The circle is the hashed one: the same draw for the same mission,
        // holding the targets' centroid off its centre.
        let site = single.terrain.surface.template.as_ref().unwrap();
        let theater = single.terrain.layout.trim_end_matches(".MM");
        let starts = single.terrain.surface.starts.as_ref().unwrap();
        let centroid = [f64::from(starts.target[0]), f64::from(starts.target[1])];
        assert_eq!(
            (center, radius),
            objective_area(centroid, theater, &site.stem, site.settings.seed)
        );
        let off = (center[0] - centroid[0]).hypot(center[1] - centroid[1]) / NMI;
        assert!((0.5..=2.5).contains(&off), "{off}");
        // The markup holds the area, never a target's own position.
        assert!(
            flight
                .markers
                .iter()
                .all(|m| m.label.contains("START") || m.label.contains("WING"))
        );
        // The same mission built again gives the same markup.
        assert_eq!(
            mission_plan(&mission(Seating::SinglePlayer), false, None),
            flight
        );
    }

    #[test]
    fn both_starts_are_marked_and_an_open_mission_starts_blue_at_its_lead() {
        for seating in [Seating::SinglePlayer, Seating::Open] {
            let plan = mission_plan(&mission(seating), false, None);
            let labels: Vec<&str> = plan.markers.iter().map(|m| m.label.as_str()).collect();
            assert_eq!(labels.first(), Some(&"BLUE START"), "{seating:?}");
            assert!(labels.contains(&"RED START"), "{labels:?}");
            assert_eq!(plan.markers[0].side, PlanSide::Blue);
        }
        // The lobby's note is the creator's, only for the lobby.
        assert_eq!(
            mission_plan(&mission(Seating::SinglePlayer), true, None)
                .note
                .as_deref(),
            Some(LOBBY_MAP_NOTE)
        );
        assert_eq!(
            mission_plan(&mission(Seating::SinglePlayer), false, None).note,
            None
        );
    }
}
