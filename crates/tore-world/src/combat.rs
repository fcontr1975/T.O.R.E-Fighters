//! Live range host, original geometry and sampled original effect art.
use crate::{
    WorldResult,
    ai_wings::{ENEMY_SIDE, FRIENDLY_SIDE},
    aircraft_type::AircraftType,
    resources::ResourceSource,
    snapshot::{
        AircraftPose, Damage, DebrisPose, Draw, EffectPose, Engine, MarkPose, PilotPose,
        ProjectilePose, RenderSnapshot,
    },
    terrain::Terrain,
    world::plane::{self, OwnshipTerms},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tore_formats::{aircraft::AircraftId, theater::CELL_FEET};
use tore_sim::flight;
use tore_sim::{
    attitude::{Basis, Vector},
    combat::live::{self, Event, Launcher, OwnshipInput},
};

#[derive(Default)]
pub struct FireInput {
    pub held: bool,
    inhibited: bool,
}
impl FireInput {
    pub fn space(&mut self, down: bool, repeat: bool, blocked: bool) {
        if !down {
            self.held = false;
            self.inhibited = false;
        } else if blocked {
            self.cancel();
            self.inhibited = true;
        } else if !repeat && !self.inhibited {
            self.held = true;
        }
    }
    pub fn cancel(&mut self) {
        self.inhibited |= self.held;
        self.held = false;
    }
}
/// One ownship's trigger: the keyboard and the controller hold it separately.
#[derive(Default)]
pub struct Trigger {
    pub input: FireInput,
    pub controller: FireInput,
}
impl Trigger {
    pub fn held(&self) -> bool {
        self.input.held || self.controller.held
    }
}
/// What the picture draws of a human-flown plane when someone else's screen
/// is presented, as its flight left it in the last step.
struct Pose {
    position: Vector,
    attitude: [f64; 3],
    velocity: Vector,
    devices: [f64; crate::snapshot::DEVICES],
    engine: Engine,
    wreck: Option<tore_sim::wreck::Phase>,
    crashed: bool,
    escape: Option<tore_sim::ejection::Escape>,
    crew_escape: Option<tore_sim::ejection::Escape>,
}
/// Yaw, pitch and bank for one drawn airborne target.
///
/// With `ai_poses` off this is the existing straight-flight fixture rule: the
/// heading comes from the velocity and the aircraft is drawn level. With it on
/// the target carries a real attitude written by the AI bridge, so the stored
/// basis is used instead.
pub fn target_pose(target: &live::Target, ai_poses: bool) -> [f64; 3] {
    if ai_poses || target.hp <= 0 {
        target.basis.angles()
    } else {
        [target.velocity[0].atan2(target.velocity[2]), 0., 0.]
    }
}
/// A target row's attitude worked out from its basis (`Basis::angles`),
/// with the bits of that basis.
type KeptAngles = Option<([u64; 9], [f64; 3])>;

/// [`target_pose`] with the kept angles of the row at `index`: a basis seen
/// before, bit for bit, gives the angles it gave then, without the
/// trigonometry.
fn target_pose_kept(
    target: &live::Target,
    ai_poses: bool,
    kept: &mut [KeptAngles],
    index: usize,
) -> [f64; 3] {
    if !(ai_poses || target.hp <= 0) {
        return target_pose(target, ai_poses);
    }
    let basis = target.basis;
    let bits: [u64; 9] =
        std::array::from_fn(|i| [basis.right, basis.up, basis.forward][i / 3][i % 3].to_bits());
    match kept.get_mut(index) {
        Some(Some((kept_bits, angles))) if *kept_bits == bits => *angles,
        Some(slot) => {
            let angles = basis.angles();
            *slot = Some((bits, angles));
            angles
        }
        None => basis.angles(),
    }
}

/// An AI aircraft's drawn stores and the inputs they were built from.
/// Configurations can be replaced at the same address during a handoff, so
/// addresses and aggregate counts cannot identify the drawn loadout.
struct StoresEntry {
    config: live::Configuration,
    loads: Vec<crate::snapshot::StationStores>,
    poses: Arc<Vec<crate::snapshot::StorePose>>,
}
impl StoresEntry {
    /// Compares every loadout input read by `snapshot::store_poses_of`.
    /// The unchanged path compares borrowed fields without building poses.
    fn matches(
        &self,
        config: &live::Configuration,
        loads: &[crate::snapshot::StationStores],
    ) -> bool {
        let old = &self.config;
        self.loads == loads
            && old.hardpoint_slots == config.hardpoint_slots
            && old.stations.len() == config.stations.len()
            && old.stations.iter().zip(&config.stations).all(|(a, b)| {
                a.mount == b.mount
                    && a.rack == b.rack
                    && a.count == b.count
                    && a.weapon.source == b.weapon.source
                    && a.weapon.shape == b.weapon.shape
                    && live::pod_rounds(&a.weapon) == live::pod_rounds(&b.weapon)
            })
            && old.tanks.len() == config.tanks.len()
            && old.tanks.iter().zip(&config.tanks).all(|(a, b)| {
                a.hardpoint == b.hardpoint
                    && a.mount == b.mount
                    && a.rack == b.rack
                    && a.quantity == b.quantity
                    && a.store.as_ref().map(|s| s.tank.fuel_weight)
                        == b.store.as_ref().map(|s| s.tank.fuel_weight)
            })
    }

    fn cached(
        stored: &mut BTreeMap<u32, Self>,
        id: u32,
        config: &live::Configuration,
        loads: Vec<crate::snapshot::StationStores>,
    ) -> Arc<Vec<crate::snapshot::StorePose>> {
        if let Some(entry) = stored.get(&id)
            && entry.matches(config, &loads)
        {
            return entry.poses.clone();
        }
        let poses = Arc::new(crate::snapshot::store_poses_of(config, &loads));
        stored.insert(
            id,
            Self {
                config: config.clone(),
                loads,
                poses: poses.clone(),
            },
        );
        poses
    }
}

/// The last two per-tick render snapshots. They are presentation output only
/// and never feed back into sensors or physics, except that an aircraft whose
/// AI stopped flying keeps the devices last drawn for it. The frame's blend of
/// the two is the app's (`CombatView::presented`).
#[derive(Default)]
struct RenderHistory {
    /// The plane the snapshots are drawn for; `None` while the history holds
    /// none, as after a reset or in an open mission with no human.
    plane: Option<u32>,
    /// The snapshot one tick earlier; none right after a restart.
    previous: Option<RenderSnapshot>,
    current: RenderSnapshot,
    /// Each target's index in `previous` and in `current`, as (id, index)
    /// sorted by id; of a repeated id the last row answers.
    places: [Vec<(u32, u32)>; 2],
    /// The snapshot that fell out of the history last, for the next to be
    /// written over.
    spare: Option<RenderSnapshot>,
    /// The AI aircraft's drawn stores, worked out again only when their
    /// loads or their loadout change. A restart forgets them.
    ai_stores: Mutex<BTreeMap<u32, StoresEntry>>,
    /// Each target row's drawn attitude as last worked out from its basis,
    /// with the basis it was worked out from (a parked aircraft's or a ground
    /// object's basis stays as it is, tick after tick).
    angles: Mutex<Vec<KeptAngles>>,
    /// How many times the history has started over.
    restarts: u64,
}
impl RenderHistory {
    /// Forgets both snapshots, as after a reset.
    fn restart(&mut self) {
        *self = Self {
            restarts: self.restarts + 1,
            ..Self::default()
        };
    }
    /// Each target of `snapshot` by id, written into `places`.
    fn place(places: &mut Vec<(u32, u32)>, snapshot: &RenderSnapshot) {
        places.clear();
        places.extend(
            snapshot
                .targets
                .iter()
                .enumerate()
                .map(|(index, pose)| (pose.id, index as u32)),
        );
        places.sort_unstable();
        let mut kept = 0;
        for at in 0..places.len() {
            if places
                .get(at + 1)
                .is_some_and(|next| next.0 == places[at].0)
            {
                continue;
            }
            places[kept] = places[at];
            kept += 1;
        }
        places.truncate(kept);
    }
    /// The snapshot to write the next one over: the one that fell out of the
    /// history, or an empty one.
    fn take_spare(&mut self) -> RenderSnapshot {
        self.spare.take().unwrap_or_default()
    }
    /// Replaces the current snapshot, keeping the previous one.
    fn set_current(&mut self, current: RenderSnapshot) {
        self.plane = Some(current.player.id);
        Self::place(&mut self.places[1], &current);
        self.spare = Some(std::mem::replace(&mut self.current, current));
    }
    /// The current snapshot becomes the previous one.
    fn advance(&mut self, next: RenderSnapshot) {
        self.plane = Some(next.player.id);
        let mut places = std::mem::take(&mut self.places[0]);
        Self::place(&mut places, &next);
        self.places[0] = std::mem::replace(&mut self.places[1], places);
        self.spare = self
            .previous
            .replace(std::mem::replace(&mut self.current, next));
    }
    fn current_target(&self, id: u32) -> Option<&AircraftPose> {
        Self::found(&self.places[1], id).map(|index| &self.current.targets[index])
    }
    fn previous_target(&self, id: u32) -> Option<&AircraftPose> {
        self.previous
            .as_ref()
            .zip(Self::found(&self.places[0], id))
            .map(|(snapshot, index)| &snapshot.targets[index])
    }
    fn found(places: &[(u32, u32)], id: u32) -> Option<usize> {
        places
            .binary_search_by_key(&id, |(id, _)| *id)
            .ok()
            .map(|at| places[at].1 as usize)
    }
}

pub struct Combat {
    pub state: live::State,
    contrail_offsets: Vec<Vector>,
    contrail_sortie: u64,
    pub contrails: tore_sim::combat::smoke::Smoke,
    /// Contrail puffs are made: off on a dedicated server, whose world no
    /// screen draws (optimization slice S11; John, 2026-10-19). The sortie
    /// number still sets every client's contrails, which each client makes
    /// for itself.
    contrail_puffs: bool,
    /// Each ownship's trigger, made on first use.
    triggers: BTreeMap<u32, Trigger>,
    /// The engine outlets of the ownships other than the first, by aircraft.
    ownship_contrails: BTreeMap<u32, Vec<Vector>>,
    /// How the last step left each ownship other than the first, for the
    /// picture.
    poses: BTreeMap<u32, Pose>,
    pub range: bool,
    /// Draw airborne targets with their own stored attitude instead of the
    /// straight-flight fixture pose. Enabled by an AI creator launch; disabled
    /// for explicit fixtures, which keep their level velocity-based pose.
    pub ai_poses: bool,
    /// Pilot-only tapes retain their existing clean-aircraft initial state.
    pub clean_recording: bool,
    /// An open mission: the AI flies every aircraft at the start, plane 0
    /// included as the first aircraft row, and humans add ownships only by
    /// handoff. Otherwise the host flies plane 0 as the first ownship from
    /// the start (single player, free flight and the range), and the combat
    /// tape, the command notes and [`Self::contrail_offsets`] follow it.
    open: bool,
    initial_ammo: Option<Vec<u16>>,
    render: RenderHistory,
    dummies: Vec<(usize, Vector)>,
    mission_spawns: Option<Vec<crate::ai_wings::MissionSpawn>>,
    /// The accepted Quick Mission layout, kept so restart rebuilds it exactly.
    pub mission_layout: Option<crate::mission_layout::MissionLayout>,
    /// The other aircraft types this mission loaded, in draw order. The app
    /// keeps the drawn model of each beside them.
    dummy_types: Vec<Arc<AircraftType>>,
    dummy_configs: Vec<live::Configuration>,
    airport_objects: Vec<tore_sim::airport::StaticObject>,
    /// The side of each scene object that has one (the surface's), applied
    /// whenever the scene's targets are registered. Setup.
    ground_sides: BTreeMap<u32, live::Side>,
    /// The explosion and crater of each surface unit whose record names
    /// them, applied with the sides. Setup.
    ground_looks: BTreeMap<u32, live::GroundLook>,
    /// The surface units' changing state; see [`crate::surface`].
    pub surface: crate::surface::SurfaceState,
    /// The parked aircraft, registered after the scene's objects whenever
    /// they are, with their short names for the target window. Setup.
    parked: Vec<(live::ParkedAircraft, String)>,
    /// The mission's ground targets that have a combat row (the surface's
    /// 0x80 objects), for the target window's objective line. Setup.
    surface_targets: std::collections::BTreeSet<u32>,
    /// The hidden shoulder-fired SAM teams: the surface's teams that are no
    /// mission target, marked on their listings whenever the scene's targets
    /// are registered (T.O.R.E opinionated, John 2026-10-29). Setup.
    ground_teams: std::collections::BTreeSet<u32>,
    /// The contact volume weapons meet for each scene object whose differs
    /// from its shape's box: a runway's pavement ([`weapon_contact`]). Setup.
    weapon_boxes: BTreeMap<u32, tore_sim::airport::OrientedBox>,
    /// The records of the combat tape being written, collected until the app
    /// drains them (`take_tape`); `None` when no tape is being recorded.
    /// Combat holds no file: the app owns the writer.
    tape: Option<Vec<crate::combat_tape::Entry>>,
    last_launcher: Option<Launcher>,
    /// Each plane's terrain-aware cruise fly-out time, kept a few ticks for
    /// the readout (review M1). Local presentation: nothing in flight reads
    /// it.
    terrain_tti: crate::readout::TerrainTti,
    /// Player commands since the mission recorder last looked. Nothing in
    /// flight reads them.
    notes: std::collections::VecDeque<CommandNote>,
    /// The highest the terrain reaches over each square, built from the
    /// world on the first step and handed to the state every step, so a
    /// round's or a missile's sight line far above the ground is not
    /// sampled. Setup, never checkpointed.
    ground_ceiling: Option<Arc<tore_sim::ground_ceiling::GroundCeiling>>,
}

/// What one combat tick did for the human-flown aircraft.
pub struct Stepped {
    /// The tick's events.
    pub events: Vec<Event>,
    /// The ownship terms each flight took, in aircraft id order.
    pub terms: Vec<(u32, OwnshipTerms)>,
}

/// A player command combat received, kept for the mission recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandNote {
    Command(live::Command),
    /// The trigger was let go while it was held.
    Release,
}
/// Command notes kept between drains; the oldest are dropped first.
const MAX_COMMAND_NOTES: usize = 64;

/// The contact volume weapons meet for a runway object (`_STRIPProc`), `None`
/// for any other object, which keeps its shape's box. A runway's shape box
/// spans its whole airfield, aprons, parked aircraft and buildings included
/// (the Falklands field's is 7,900 by 8,100 ft and 92 ft tall), so rounds and
/// bombs meet a slab at the pavement instead: the shape's footprint, from 2
/// ft below the runway plane to 1 ft above it (lead ruling, 2026-10-10).
/// Units and aircraft standing on or beside the field are reached; the
/// landable surface, aircraft contact and every other use of the shape box
/// are unchanged. A composite runway shape's own structures (towers, hangars
/// drawn in it) no longer stop weapons.
pub fn weapon_contact(
    object: &tore_sim::airport::StaticObject,
    scene: &tore_sim::airport::Scene,
) -> Option<tore_sim::airport::OrientedBox> {
    if !object.runway {
        return None;
    }
    let runway = scene.runways.iter().find(|r| r.object == object.id)?;
    let b = object.bounds;
    let pavement = runway.support_height(b.center[0], b.center[2])?;
    let up = tore_sim::attitude::Basis::new(b.heading, b.pitch, b.bank).up;
    let base = [b.center[0], pavement, b.center[2]];
    Some(tore_sim::airport::OrientedBox {
        center: std::array::from_fn(|i| base[i] - up[i] * 0.5),
        half: [b.half[0], 1.5, b.half[2]],
        ..b
    })
}

/// Lowest airborne mission start above sea level, feet (John, 2026-09-29).
pub const SPAWN_MIN_MSL_FT: f64 = 5000.;
/// Lowest airborne mission start above the ground under it, feet.
pub const SPAWN_MIN_AGL_FT: f64 = 1000.;
/// Where a helicopter of the AI's own wings starts above the ground under
/// it, feet (opinionated, agent, slice ST-H): the AI cannot yet make a
/// helicopter climb or descend by much, so it starts at the height it
/// attacks from (the pop-up's 1,500 ft) and is not raised to
/// [`SPAWN_MIN_MSL_FT`]. A helicopter in the player's wing starts with the
/// player.
pub const HELICOPTER_START_AGL_FT: f64 = 1500.;

/// Where an airborne mission start at `y` feet settles over ground at
/// `ground`: raised to [`SPAWN_MIN_MSL_FT`] above sea level and
/// [`SPAWN_MIN_AGL_FT`] above the ground (John, 2026-09-29), except a
/// helicopter of the AI's own wings, which starts [`HELICOPTER_START_AGL_FT`]
/// above the ground, up or down (slice ST-H).
pub fn settled_spawn_height(y: f64, ground: f64, helicopter: bool) -> f64 {
    if helicopter {
        ground + HELICOPTER_START_AGL_FT
    } else {
        y.max(SPAWN_MIN_MSL_FT.max(ground + SPAWN_MIN_AGL_FT))
    }
}

pub fn launcher(s: &flight::State) -> Launcher {
    Launcher {
        position: s.position,
        basis: Basis::new(s.yaw, s.pitch, s.bank),
        speed_fps: s.speed,
        velocity: s.velocity,
        bay_ready: !s.bay_available() || s.bay >= 0.95,
        // Selecting the passive infrared channel stops radar transmission
        // without changing the radar power switch itself.
        radar_power: s.radar,
        radar: s.radar && s.engine && s.sensors.channel == tore_sim::sensors::Channel::Radar,
        jammer: s.jammer && s.engine,
        alive: !s.crashed,
        body_present: !s.wreck_gone(),
        controls: s.sensors,
    }
}
/// The longest part of a gun round's rewind beyond the seat's interpolation
/// delay, ticks (250 ms): a player on a slow link is judged against where the
/// targets were at most this long plus the delay ago.
pub const REWIND_BEYOND_DELAY_TICKS: u16 = 30;

/// The rewind the gun rounds a seat fires on input tick `tick` carry: the
/// ticks between that tick and the host tick its screen showed, of which the
/// part beyond the interpolation delay is at most
/// [`REWIND_BEYOND_DELAY_TICKS`] and the whole at most
/// [`live::rewind::MAX_REWIND_TICKS`]. No view, or a view of a tick not
/// before this one, is no rewind.
pub fn gun_rewind(tick: u64, view: Option<crate::seats::SeatView>) -> u16 {
    let Some(view) = view else {
        return 0;
    };
    let behind = tick.saturating_sub(view.tick);
    let cap = (u16::from(view.interpolation_delay) + REWIND_BEYOND_DELAY_TICKS)
        .min(live::rewind::MAX_REWIND_TICKS);
    behind.min(u64::from(cap)) as u16
}

impl Combat {
    /// The sortie number that sets each aircraft's contrail height. A host
    /// sends it in the Mission message so a client draws contrails at the same
    /// heights.
    pub fn contrail_sortie(&self) -> u64 {
        self.contrail_sortie
    }
    /// Whether the step makes contrail puffs ([`Self::contrails`]).
    pub fn contrail_puffs(&self) -> bool {
        self.contrail_puffs
    }
    /// Makes contrail puffs, or stops and drops them: a dedicated server,
    /// whose world no screen draws, makes none. The sortie number, which sets
    /// where every client draws its own, is kept either way. A checkpoint
    /// carries the switch, so a standby's copy steps as the host's does.
    pub fn set_contrail_puffs(&mut self, on: bool) {
        self.contrail_puffs = on;
        if !on {
            self.contrails = Default::default();
        }
    }
    /// The aircraft a host with one flight flies: its first ownship. For free
    /// flight, the range, the AI probe and the loadout screen, which always
    /// have one; it panics with none, so the build, the tick and the handoff
    /// never call it and ask for a plane's ownship by id instead.
    pub fn own_id(&self) -> u32 {
        self.state.own().aircraft
    }
    /// The combat state of the aircraft a host with one flight flies; see
    /// [`Self::own_id`].
    pub fn own(&self) -> &live::Ownship {
        self.state.own()
    }
    /// See [`Self::own`].
    pub fn own_mut(&mut self) -> &mut live::Ownship {
        self.state.own_mut()
    }
    /// What the flown aircraft's cockpit shows; see [`Self::own_id`].
    pub fn own_view(&self) -> live::OwnshipView<'_> {
        self.state.own_view()
    }
    /// The host's own plane while it has an ownship: plane 0 in single player,
    /// free flight and the range, which the combat tape, the command notes and
    /// [`Self::contrail_offsets`] follow. `None` in an open mission, where
    /// every ownship is a plane a human took, and once plane 0 is given back.
    pub fn host_plane(&self) -> Option<u32> {
        const HOST: u32 = 0;
        (!self.open && self.state.ownship(HOST).is_some()).then_some(HOST)
    }
    /// Whether this is an open mission's combat: the AI flew every aircraft at
    /// the start, plane 0 included.
    pub fn is_open(&self) -> bool {
        self.open
    }
    /// The id of the first aircraft row a reset adds: 1 after the host's plane
    /// 0, or 0 in an open mission, where plane 0 is a row too.
    fn first_row(&self) -> u32 {
        u32::from(!self.open)
    }
    /// The cockpit readout of `plane` for `launcher`, or `None` when it has no
    /// ownship. `wings` supplies the AI's part of the readout and `cockpit`
    /// what the plane keeps outside combat (the tower and the mission result);
    /// without them those parts are empty. See [`crate::readout`].
    pub fn cockpit_readout(
        &self,
        plane: u32,
        launcher: Launcher,
        wings: Option<&crate::ai_wings::AiWings>,
        cockpit: Option<&crate::world::Cockpit>,
    ) -> Option<crate::readout::CockpitReadout> {
        self.cockpit_readout_with_terrain(plane, launcher, wings, cockpit, None)
    }

    /// The mission readout with terrain available for launch checks and
    /// cruise fly-out time, without changing the energy-only range cue.
    pub fn cockpit_readout_with_terrain(
        &self,
        plane: u32,
        launcher: Launcher,
        wings: Option<&crate::ai_wings::AiWings>,
        cockpit: Option<&crate::world::Cockpit>,
        ground: Option<&dyn Fn(f64, f64) -> f64>,
    ) -> Option<crate::readout::CockpitReadout> {
        let mut readout = crate::readout::build_with_terrain_kept(
            &self.state,
            plane,
            launcher,
            wings,
            cockpit,
            ground,
            Some(&self.terrain_tti),
        )?;
        if let Some(brief) = readout.target_window.as_mut()
            && brief.objective.is_none()
        {
            brief.objective = self.ground_objective(plane, brief.id);
        }
        // Surface units' missile locks sound the same tone as the AI's.
        let locks = &mut readout.rwr.locks;
        locks.extend(self.surface.locks_on(plane));
        locks.truncate(crate::readout::MAX_AI_THREATS);
        // The radars painting the plane show bright on its RWR.
        readout.rwr.painting = self.surface.painting(plane);
        Some(readout)
    }
    /// What the mission asks of `plane` about target `id` when it is one of
    /// the ground target's objects: the window says "Obj:" for those too. The
    /// enemy side's planes (a Redfor player in a multiplayer game) must keep
    /// them, every other plane destroys them.
    pub fn ground_objective(
        &self,
        plane: u32,
        id: u32,
    ) -> Option<crate::target_window::TargetObjective> {
        use crate::target_window::TargetObjective;
        self.surface_targets.contains(&id).then(|| {
            let redfor = self
                .state
                .ownship(plane)
                .is_some_and(|own| own.side == ENEMY_SIDE);
            if redfor {
                TargetObjective::Survive
            } else {
                TargetObjective::Destroy
            }
        })
    }
    /// Player airborne startup convention: canonical gun selected and armed.
    pub fn apply_startup_weapons(&mut self) {
        let (guns_only, unlimited_ammo) = (
            self.state.cheats.guns_only,
            self.state.cheats.unlimited_ammo,
        );
        apply_startup_weapon_state(self.state.own_mut(), guns_only, unlimited_ammo);
    }
    pub fn uses_normal_startup_defaults(&self) -> bool {
        !self.range && self.tape.is_none() && !self.clean_recording
    }
    /// Registers the terrain's airport scene as combat targets, each with the
    /// side its surface gives it, and starts the surface units' state.
    pub fn add_scene_targets(&mut self, terrain: &Terrain) -> WorldResult<()> {
        let sides = terrain.surface.object_sides.clone();
        // Which wrecks burn (B16, docs/spec/surface-defenses.md, "Wreck
        // fires"): a fixed pick per object and mission for vehicles.
        let fires = crate::surface::wreck::scene_fires(&terrain.surface, &terrain.airport_scene);
        let burns = |id: u32| fires.get(&id).is_some_and(|class| class.burns());
        // A building's or fixed object's fire burns for the whole mission;
        // a vehicle's or ship's lasts 15 minutes (nr-fx2).
        let lasting = |id: u32| {
            matches!(
                fires.get(&id),
                Some(
                    crate::surface::wreck::FireClass::Building
                        | crate::surface::wreck::FireClass::Hard
                )
            )
        };
        let runways: std::collections::BTreeSet<u32> = terrain
            .airport_scene
            .objects
            .iter()
            .filter(|object| object.runway)
            .map(|object| object.id)
            .collect();
        // A burning building scorches the ground round its foot: a crater
        // mark at least as wide as its plan.
        let scorch: BTreeMap<u32, u8> = terrain
            .airport_scene
            .objects
            .iter()
            .filter(|object| {
                matches!(
                    fires.get(&object.id),
                    Some(
                        crate::surface::wreck::FireClass::Building
                            | crate::surface::wreck::FireClass::Hard
                    )
                )
            })
            .map(|object| {
                (
                    object.id,
                    crate::surface::wreck::scorch_size(object.bounds.half),
                )
            })
            .collect();
        let scorched = |id: u32, crater: u8| scorch.get(&id).map_or(crater, |s| crater.max(*s));
        let mut looks: BTreeMap<u32, live::GroundLook> = terrain
            .surface
            .units
            .iter()
            .filter_map(|unit| {
                Some((
                    unit.id.0,
                    live::GroundLook {
                        explosion: unit.explosion?,
                        crater: scorched(unit.id.0, unit.crater?),
                        burns: burns(unit.id.0),
                        lasting: lasting(unit.id.0),
                        // A template's own runway is one of its units.
                        pavement: runways.contains(&unit.id.0),
                    },
                ))
            })
            .collect();
        // Every other building and fixed object of the scene explodes as
        // before (the large ground blast) and burns on its scorch; a runway
        // takes the craters of the weapons that hit it.
        for object in &terrain.airport_scene.objects {
            looks.entry(object.id).or_insert(live::GroundLook {
                explosion: tore_sim::combat::blast::GROUND_OBJECT,
                crater: scorched(object.id, 0),
                burns: burns(object.id),
                lasting: lasting(object.id),
                pavement: object.runway,
            });
        }
        let parked = terrain
            .surface
            .parked_scene
            .iter()
            .map(|pose| (pose.target(), pose.name.clone()))
            .collect();
        let previous = (
            std::mem::replace(&mut self.ground_sides, sides),
            std::mem::replace(&mut self.ground_looks, looks),
            std::mem::replace(&mut self.parked, parked),
        );
        // The barrage zones have no shape: they join combat with a volume of
        // their own (docs/spec/surface-defenses.md, "Barrage zone").
        let shapeless = crate::surface::fire::shapeless_targets(&terrain.surface);
        let scene = if shapeless.is_empty() {
            std::borrow::Cow::Borrowed(&terrain.airport_scene)
        } else {
            let mut scene = terrain.airport_scene.clone();
            scene.objects.extend(shapeless);
            std::borrow::Cow::Owned(scene)
        };
        if let Err(error) = self.add_airport_targets(&scene) {
            (self.ground_sides, self.ground_looks, self.parked) = previous;
            return Err(error);
        }
        self.surface = terrain.surface.fresh_state();
        self.surface_targets =
            crate::ai_wings::outcome::ground_targets(&terrain.surface, &self.state)
                .into_iter()
                .collect();
        self.ground_teams = terrain
            .surface
            .arsenal
            .units
            .iter()
            .filter(|arms| arms.team && !self.surface_targets.contains(&arms.unit.0))
            .map(|arms| arms.unit.0)
            .collect();
        mark_teams(&mut self.state, &self.ground_teams);
        Ok(())
    }
    /// Registers `scene`'s objects as combat targets with the sides the last
    /// [`Self::add_scene_targets`] gave (none before one: neutral), then that
    /// call's parked aircraft.
    pub fn add_airport_targets(&mut self, scene: &tore_sim::airport::Scene) -> WorldResult<()> {
        scene.validate().map_err(std::io::Error::other)?;
        // A new layout replaces static identities atomically in the staged state.
        let mut staged = self.state.clone();
        staged.remove_ground_targets();
        let weapon_boxes: BTreeMap<u32, tore_sim::airport::OrientedBox> = scene
            .objects
            .iter()
            .filter_map(|object| Some((object.id, weapon_contact(object, scene)?)))
            .collect();
        for object in &scene.objects {
            Self::register_airport_object(
                &mut staged,
                object,
                weapon_boxes.get(&object.id).copied(),
                self.ground_side(object.id),
                self.ground_looks.get(&object.id).copied(),
            )?;
        }
        for (parked, _) in &self.parked {
            staged.add_parked_aircraft(parked)?;
        }
        mark_teams(&mut staged, &self.ground_teams);
        self.state = staged;
        self.airport_objects = scene.objects.clone();
        self.weapon_boxes = weapon_boxes;
        Ok(())
    }
    /// The side scene object `id` fights for, neutral without one.
    pub fn ground_side(&self, id: u32) -> live::Side {
        self.ground_sides
            .get(&id)
            .copied()
            .unwrap_or(tore_sim::combat::live::NO_SIDE)
    }
    fn register_airport_object(
        state: &mut live::State,
        object: &tore_sim::airport::StaticObject,
        contact: Option<tore_sim::airport::OrientedBox>,
        side: live::Side,
        look: Option<live::GroundLook>,
    ) -> WorldResult<()> {
        state.add_ground_target(
            object.id,
            contact.unwrap_or(object.bounds),
            object.hit_points,
            object.category,
            side,
        )?;
        if let Some(look) = look {
            state.set_ground_look(object.id, look);
        }
        if let Some(target) = state.targets.iter_mut().find(|t| t.id == object.id) {
            target.signature.radar = object.radar_signature;
            target.signature.infrared = object.infrared_signature;
        }
        // What the ground pages read of its type (docs/formats/radar.md).
        state.set_ground_listing(
            object.id,
            live::GroundListing {
                flags: object.type_flags,
                max_visible: object.max_visible.clamp(0, i32::from(u16::MAX)) as u16,
                team: false,
            },
        );
        Ok(())
    }
    pub fn ground_name(&self, id: u32) -> Option<&str> {
        self.airport_objects
            .iter()
            .find(|o| o.id == id)
            .map(|o| o.name.as_str())
            .or_else(|| {
                self.parked
                    .iter()
                    .find(|(parked, _)| parked.id == id)
                    .map(|(_, name)| name.as_str())
            })
    }
    pub fn new(h: &AircraftType, data: &dyn ResourceSource, range: bool) -> WorldResult<Self> {
        let config = live::Configuration::from_source(&h.profile, |name| {
            data.get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing live-fire resource {name}")))
        })?;
        Self::configured(h, range, config, None)
    }
    pub fn with_loadout(
        h: &AircraftType,
        load: &tore_sim::combat::loadout::Loadout,
    ) -> WorldResult<Self> {
        load.validate()?;
        Self::configured(
            h,
            false,
            load.configuration.clone(),
            Some(load.ammunition()?),
        )
    }
    /// Combat for an open mission: no ownship and no host's aircraft. The
    /// mission's aircraft, plane 0 first, join as rows with
    /// [`Self::mission_aircraft`] and take their places at [`Self::reset`];
    /// humans join by handoff with [`Self::add_ownship`].
    pub fn open() -> Self {
        Self {
            open: true,
            ..Self::with_state(live::State::open_mission(), Vec::new(), false, None)
        }
    }
    fn configured(
        h: &AircraftType,
        range: bool,
        config: live::Configuration,
        initial_ammo: Option<Vec<u16>>,
    ) -> WorldResult<Self> {
        Ok(Self::with_state(
            live::State::new(config, true)?,
            h.contrail_offsets.clone(),
            range,
            initial_ammo,
        ))
    }
    fn with_state(
        mut state: live::State,
        contrail_offsets: Vec<Vector>,
        range: bool,
        initial_ammo: Option<Vec<u16>>,
    ) -> Self {
        // New worlds select the approved energy rules. The lower-level State
        // constructor remains Spec for historical tools and unmarked tapes.
        state.weapon_rules = tore_sim::combat::missiles::Rules::Energy;
        Self {
            contrail_offsets,
            contrail_sortie: 0,
            contrails: Default::default(),
            contrail_puffs: true,
            state,
            dummies: Vec::new(),
            mission_spawns: None,
            mission_layout: None,
            dummy_types: Vec::new(),
            dummy_configs: Vec::new(),
            airport_objects: Vec::new(),
            ground_sides: BTreeMap::new(),
            ground_looks: BTreeMap::new(),
            surface: Default::default(),
            parked: Vec::new(),
            surface_targets: Default::default(),
            ground_teams: Default::default(),
            weapon_boxes: BTreeMap::new(),
            triggers: BTreeMap::new(),
            ownship_contrails: BTreeMap::new(),
            poses: BTreeMap::new(),
            range,
            ai_poses: false,
            clean_recording: false,
            open: false,
            initial_ammo,
            render: RenderHistory::default(),
            tape: None,
            last_launcher: None,
            terrain_tti: Default::default(),
            notes: Default::default(),
            ground_ceiling: None,
        }
    }
    /// Everything combat draws for this tick, as plain data, for the screen
    /// of the human who flies `plane`: that plane is the picture's player, in
    /// the flight state `player`, and every other human-flown plane, the
    /// first one too, is an ordinary target drawn from the pose its last step
    /// left it in. `wings` supplies the AI aircraft's devices and ejected
    /// pilots. An aircraft whose AI is not alive, or that has no AI, keeps the
    /// devices last drawn for it. Panics when `plane` has no ownship.
    pub fn snapshot(
        &self,
        plane: u32,
        player: &flight::State,
        wings: Option<&crate::ai_wings::AiWings>,
    ) -> RenderSnapshot {
        self.snapshot_reusing(
            plane,
            player,
            wings,
            RenderSnapshot::default(),
            &self.render.ai_stores,
            &self.render.angles,
        )
    }
    /// [`Self::snapshot`] written over `spare`, a snapshot nobody needs any
    /// more, whose lists and strings it reuses where it can, so a tick builds
    /// few new ones. Every field of the result is written, so what `spare`
    /// held makes no difference. `ai_stores` keeps the AI aircraft's drawn
    /// stores while their loads stay as they were, and `angles` each target
    /// row's attitude while its basis stays as it was; empty ones make the
    /// snapshot work them all out.
    fn snapshot_reusing(
        &self,
        plane: u32,
        player: &flight::State,
        wings: Option<&crate::ai_wings::AiWings>,
        mut spare: RenderSnapshot,
        ai_stores: &Mutex<BTreeMap<u32, StoresEntry>>,
        angles: &Mutex<Vec<KeptAngles>>,
    ) -> RenderSnapshot {
        let ownship = self.dummies.is_empty();
        let own_id = plane;
        let own = self
            .state
            .ownship(plane)
            .expect("the presented plane has an ownship");
        // The other human-flown planes, in aircraft id order, as the last step
        // left them.
        let others: Vec<(&live::Ownship, &Pose)> = self
            .state
            .ownships()
            .iter()
            .filter(|other| other.aircraft != own_id)
            .filter_map(|other| Some((other, self.poses.get(&other.aircraft)?)))
            .collect();
        let model = |id: u32| {
            id.checked_sub(self.first_row())
                .and_then(|index| self.dummies.get(index as usize))
                .map(|(model, _)| self.dummy_types[*model].profile.id)
        };
        let draw = |id: u32| {
            if ownship {
                Draw::Ownship
            } else if let Some((own, _)) = others.iter().find(|(own, _)| own.aircraft == id) {
                Draw::Model(own.configuration().aircraft)
            } else {
                model(id).map_or(Draw::Hidden, Draw::Model)
            }
        };
        // What each AI actor adds to its aircraft's pose, found in one walk
        // of the roster and looked up by id: the devices an actor that flies
        // is drawn with (the last ones drawn hold once it stops), a
        // rotorcraft's rotors for their sound and their drawing, a lit
        // afterburner for the flame light, and the stores each station holds
        // now (B3).
        struct Actor {
            id: u32,
            devices: Option<[f64; crate::snapshot::DEVICES]>,
            rotor: Option<Engine>,
            burning: bool,
            stores: Option<Arc<Vec<crate::snapshot::StorePose>>>,
        }
        let mut actors: Vec<Actor> = Vec::new();
        if let Some(wings) = wings {
            for actor in wings.mission().actors() {
                let alive = actor.alive();
                let flight = actor.flight();
                actors.push(Actor {
                    id: actor.id(),
                    devices: alive.then(|| crate::snapshot::devices(flight)),
                    rotor: if alive {
                        flight.rotor_speed_percent().map(|percent| {
                            let (rotor_turns, rotor_tilt) = crate::snapshot::rotor_pose(flight);
                            Engine {
                                rotor: percent / 100.,
                                rotor_turns,
                                rotor_tilt,
                                ..Engine::default()
                            }
                        })
                    } else {
                        None
                    },
                    burning: alive && flight.afterburner_active(),
                    stores: None,
                });
            }
            actors.sort_by_key(|actor| actor.id);
            // The cache is held only while it is checked and filled, so
            // snapshots made at once for several seats do not wait on one
            // another.
            let mut stored = ai_stores.lock().unwrap_or_else(|e| e.into_inner());
            for (id, config, loads) in ai_station_stores(wings) {
                let entry = StoresEntry::cached(&mut stored, id, config, loads);
                if let Ok(index) = actors.binary_search_by_key(&id, |actor| actor.id) {
                    actors[index].stores = Some(entry);
                }
            }
        }
        let actor_of = |id: u32| {
            actors
                .binary_search_by_key(&id, |actor| actor.id)
                .ok()
                .map(|index| &actors[index])
        };
        // The last devices drawn hold once an aircraft stops flying.
        let simulated = |id: u32| {
            actor_of(id)
                .and_then(|actor| actor.devices)
                .or_else(|| self.render.current_target(id).and_then(|pose| pose.devices))
        };
        let gun_devices = |mut devices: [f64; crate::snapshot::DEVICES], own: &live::Ownship| {
            if let Some(gunship) = &own.gunship {
                devices[crate::snapshot::GUN_AIM..crate::snapshot::GUN_GROUP]
                    .copy_from_slice(&gunship.normalized_devices());
                devices[crate::snapshot::GUN_GROUP] = f64::from(gunship.mask());
            }
            devices
        };
        let config = own.configuration();
        let capacity = config.damage_capacity;
        let player_engine = Engine {
            lit: player.engine && player.fuel > 0.,
            afterburner: player.afterburner_active(),
            rates: player.auxiliary_rates,
            rotor: player.lift_controls.drive.rotor_speed,
            rotor_turns: crate::snapshot::rotor_pose(player).0,
            rotor_tilt: crate::snapshot::rotor_pose(player).1,
            flame: player.afterburner_active() && player.escape.is_none() && own.hp > 0,
        };
        // Fixtures copy the player's state with their own crash flag.
        let fixture_afterburner = ownship
            && if player.crashed {
                let mut alive = player.clone();
                alive.crashed = false;
                alive.afterburner_active()
            } else {
                player_engine.afterburner
            };
        let model_engine = Engine {
            lit: true,
            afterburner: false,
            rates: [0.; 3],
            rotor: 0.,
            rotor_turns: 0.,
            rotor_tilt: [[0.; 2]; 2],
            flame: false,
        };
        let pilot = |owner: u32, escape: &tore_sim::ejection::Escape| PilotPose {
            owner,
            position: escape.position,
            heading: escape.heading,
            phase: escape.phase,
            crew: false,
        };
        let crew = |owner: u32, escape: &tore_sim::ejection::Escape| PilotPose {
            crew: true,
            ..pilot(owner, escape)
        };
        let player_pose = AircraftPose {
            id: own_id,
            aircraft: Some(config.aircraft),
            draw: Draw::Ownship,
            position: player.position,
            attitude: [player.yaw, player.pitch, player.bank],
            velocity: player.velocity,
            devices: Some(gun_devices(crate::snapshot::devices(player), own)),
            engine: player_engine,
            damage: Damage {
                hp: own.hp,
                initial_hp: capacity,
                // The exact amounts the drawn fractions divide.
                sections: own.damage_amounts(),
                structural: own.damage_section(),
            },
            airborne: true,
            wreck: player.wreck.as_ref().map(|wreck| wreck.phase),
            crashed: player.crashed,
            stores: own_stores(own),
        };
        // Every combat target in target order, then the other human-flown
        // planes. A slot of the spare keeps its list of stores, which the new
        // pose's stores are written over.
        let mut targets = std::mem::take(&mut spare.targets);
        let mut kept_angles = angles.lock().unwrap_or_else(|e| e.into_inner());
        kept_angles.resize(self.state.targets.len(), None);
        let mut at = 0;
        for (index, t) in self.state.targets.iter().enumerate() {
            if targets.len() == at {
                targets.push(AircraftPose::default());
            }
            // Written field by field into the slot, which keeps its list of
            // stores: every field is named, so none is left from before.
            let AircraftPose {
                id,
                aircraft,
                draw: drawn,
                position,
                attitude,
                velocity,
                devices,
                engine,
                damage,
                airborne,
                wreck,
                crashed,
                stores,
            } = &mut targets[at];
            match actor_of(t.id).and_then(|actor| actor.stores.as_deref()) {
                Some(poses) => stores.clone_from(poses),
                None => stores.clear(),
            }
            *id = t.id;
            *aircraft = t.aircraft;
            *drawn = draw(t.id);
            *position = t.position;
            *attitude = target_pose_kept(t, self.ai_poses, &mut kept_angles, index);
            *velocity = t.velocity;
            *devices = simulated(t.id);
            *engine = Engine {
                flame: t.airborne && t.hp > 0 && actor_of(t.id).is_some_and(|actor| actor.burning),
                ..if ownship {
                    Engine {
                        afterburner: fixture_afterburner && t.hp > 0,
                        ..player_engine
                    }
                } else {
                    match actor_of(t.id).and_then(|actor| actor.rotor) {
                        Some(rotor) => Engine {
                            rotor: rotor.rotor,
                            rotor_turns: rotor.rotor_turns,
                            rotor_tilt: rotor.rotor_tilt,
                            ..model_engine
                        },
                        None => model_engine,
                    }
                }
            };
            *damage = Damage {
                hp: t.hp,
                initial_hp: t.initial_hp,
                sections: t.localized_damage.amounts,
                structural: t.localized_damage.structural_section,
            };
            *airborne = t.airborne;
            *wreck = t.wreck.as_ref().map(|wreck| wreck.phase);
            *crashed = t.hp <= 0;
            at += 1;
        }
        drop(kept_angles);
        for (own, pose) in &others {
            let pose = AircraftPose {
                id: own.aircraft,
                aircraft: Some(own.configuration().aircraft),
                draw: Draw::Model(own.configuration().aircraft),
                position: pose.position,
                attitude: pose.attitude,
                velocity: pose.velocity,
                devices: Some(gun_devices(pose.devices, own)),
                engine: Engine {
                    flame: pose.engine.flame && own.hp > 0,
                    ..pose.engine
                },
                damage: Damage {
                    hp: own.hp,
                    initial_hp: own.configuration().damage_capacity,
                    sections: own.damage_amounts(),
                    structural: own.damage_section(),
                },
                airborne: true,
                wreck: pose.wreck,
                crashed: pose.crashed,
                stores: own_stores(own),
            };
            match targets.get_mut(at) {
                Some(slot) => *slot = pose,
                None => targets.push(pose),
            }
            at += 1;
        }
        targets.truncate(at);
        // The rounds in flight, each written over a slot of the spare so that
        // its record name and shape reuse the strings already there.
        let mut projectiles = std::mem::take(&mut spare.projectiles);
        projectiles.resize_with(self.state.projectiles.len(), Default::default);
        for (slot, p) in projectiles.iter_mut().zip(&self.state.projectiles) {
            let weapon = self.state.weapon(p);
            let mut name = std::mem::take(&mut slot.weapon);
            name.clone_from(&weapon.source);
            let mut shape = std::mem::take(&mut slot.shape);
            shape.clone_from(&weapon.shape);
            *slot = ProjectilePose {
                id: p.id,
                owner: p.owner,
                weapon: name,
                shape,
                gun: live::is_gun(weapon),
                tracer: p.tracer,
                position: p.position,
                previous: p.previous,
                direction: p.direction,
                target: p.target.filter(|_| p.threatening()),
                incoming: p.incoming.is_some() && p.threatening(),
                speed_f8: p.speed_f8,
                powered: self.state.motor_powered_with(weapon, p),
            };
        }
        let mut effects = std::mem::take(&mut spare.effects);
        effects.clear();
        effects.extend(self.state.effects.iter().map(|e| EffectPose {
            kind: e.kind,
            position: e.position,
            ticks: e.ticks,
            blast: e.blast,
            scale: e.scale,
        }));
        let mut marks = std::mem::take(&mut spare.marks);
        marks.clear();
        marks.extend(
            self.state
                .marks
                .iter()
                .map(|m| MarkPose::of(m, self.state.tick())),
        );
        // A piece of debris takes its owner's structural section from the
        // owner's row, the first of its id: found by id, not by a walk of
        // every row for every piece.
        let rows_by_id = (!self.state.debris.is_empty()).then(|| {
            let mut rows: Vec<(u32, usize)> = self
                .state
                .targets
                .iter()
                .enumerate()
                .map(|(index, target)| (target.id, index))
                .collect();
            rows.sort_unstable();
            rows
        });
        let first_row_of = |id: u32| {
            let rows = rows_by_id.as_deref()?;
            let at = rows.partition_point(|(row, _)| *row < id);
            rows.get(at)
                .filter(|(row, _)| *row == id)
                .map(|(_, index)| &self.state.targets[*index])
        };
        let mut debris = std::mem::take(&mut spare.debris);
        debris.clear();
        debris.extend(self.state.debris.iter().map(|piece| DebrisPose {
            owner: piece.owner,
            draw: if piece.owner == own_id {
                Draw::Ownship
            } else {
                draw(piece.owner)
            },
            position: piece.position,
            attitude: piece.basis.angles(),
            variant: if piece.owner == own_id {
                player.damage_variant
            } else if let Some((own, _)) =
                others.iter().find(|(own, _)| own.aircraft == piece.owner)
            {
                own.damage_section().map(|section| section as usize)
            } else if self.state.is_parked(piece.owner) {
                // A parked aircraft's piece: 0 is its `_B` shape,
                // 1 its `_D` (docs/spec/surface-defenses.md).
                Some(piece.variant)
            } else {
                first_row_of(piece.owner)
                    .and_then(|target| target.localized_damage.structural_section)
                    .map(|section| section as usize)
            },
        }));
        let mut pilots = std::mem::take(&mut spare.pilots);
        pilots.clear();
        pilots.extend(
            player
                .escape
                .iter()
                .map(|escape| pilot(own_id, escape))
                .chain(others.iter().filter_map(|(own, pose)| {
                    pose.escape
                        .as_ref()
                        .map(|escape| pilot(own.aircraft, escape))
                }))
                .chain(
                    wings
                        .into_iter()
                        .flat_map(|wings| wings.escapees())
                        .map(|(owner, escape)| pilot(owner, escape)),
                )
                // The second crew members of the ejected two-seaters follow
                // every pilot, in the same order.
                .chain(player.crew_escape.iter().map(|escape| crew(own_id, escape)))
                .chain(others.iter().filter_map(|(own, pose)| {
                    pose.crew_escape
                        .as_ref()
                        .map(|escape| crew(own.aircraft, escape))
                }))
                .chain(
                    wings
                        .into_iter()
                        .flat_map(|wings| wings.crew_escapees())
                        .map(|(owner, escape)| crew(owner, escape)),
                ),
        );
        let mut models = std::mem::take(&mut spare.models);
        models.clear();
        models.extend(self.dummy_types.iter().map(|h| h.profile.id));
        // The type of every other human-flown plane, for the app to draw.
        for (own, _) in &others {
            if !models.contains(&own.configuration().aircraft) {
                models.push(own.configuration().aircraft);
            }
        }
        RenderSnapshot {
            tick: self.state.tick(),
            player: player_pose,
            targets,
            projectiles,
            effects,
            marks,
            debris,
            pilots,
            models,
            surface: self.surface_poses(),
        }
    }
    /// once a mission's AI has placed its aircraft.
    pub fn restart_render(
        &mut self,
        plane: u32,
        player: &flight::State,
        wings: Option<&crate::ai_wings::AiWings>,
    ) {
        self.render.restart();
        let current = self.snapshot(plane, player, wings);
        self.render.set_current(current);
    }
    /// Ends a simulation tick: the current snapshot becomes the previous one.
    pub fn advance_render(
        &mut self,
        plane: u32,
        player: &flight::State,
        wings: Option<&crate::ai_wings::AiWings>,
    ) {
        let spare = self.render.take_spare();
        let next = self.snapshot_reusing(
            plane,
            player,
            wings,
            spare,
            &self.render.ai_stores,
            &self.render.angles,
        );
        // Debug builds make the snapshot again from nothing and compare.
        #[cfg(debug_assertions)]
        {
            let fresh = self.snapshot_reusing(
                plane,
                player,
                wings,
                RenderSnapshot::default(),
                &Mutex::default(),
                &Mutex::default(),
            );
            if next != fresh {
                // A NaN is never equal to itself; the text of both says if
                // anything else differs.
                assert_eq!(
                    format!("{next:?}"),
                    format!("{fresh:?}"),
                    "a reused snapshot"
                );
            }
        }
        self.render.advance(next);
    }
    /// Retakes the current snapshot after a command changed the scene between
    /// ticks, so the change shows at once as it always has.
    pub fn refresh_render(
        &mut self,
        plane: u32,
        player: &flight::State,
        wings: Option<&crate::ai_wings::AiWings>,
    ) {
        let spare = self.render.take_spare();
        let current = self.snapshot_reusing(
            plane,
            player,
            wings,
            spare,
            &self.render.ai_stores,
            &self.render.angles,
        );
        self.render.set_current(current);
    }
    /// An AI aircraft that flew into the ground with hit points left gets a
    /// crash site where it hit, as a shot-down wreck does when it lands.
    /// Presentation only; call after each AI step.
    pub fn ai_crashes(&mut self, wings: &crate::ai_wings::AiWings, world: &Terrain) {
        let crashed: Vec<_> = wings
            .mission()
            .actors()
            .iter()
            .filter(|actor| actor.flight().crashed && actor.flight().escape.is_none())
            .filter(|actor| {
                self.state
                    .targets
                    .iter()
                    .any(|t| t.id == actor.id() && t.hp > 0)
            })
            .map(|actor| (actor.id(), actor.flight().position))
            .collect();
        for (id, position) in crashed {
            let water = world.over_water(position[0], position[2]);
            self.state.aircraft_crashed(id, position, water);
        }
    }
    /// Developer preview (`TORE_EFFECT_PREVIEW=hill`): the open ground
    /// nearest `from` that is one even slope of 12 to 20 degrees across 2,400
    /// feet (a plane within 25 ft, on land), as its centre and the heading
    /// that faces uphill. The preview then starts 2,600 feet short of it, so
    /// its craters lie on the hillside.
    pub fn preview_hillside(world: &Terrain, from: [f64; 2]) -> Option<([f64; 3], f64)> {
        const STEP: f64 = 250.;
        const REACH: f64 = 600.;
        let height = |x: f64, z: f64| f64::from(world.height(x as f32, z as f32));
        let size = [
            world.theater.cols as f64 * f64::from(CELL_FEET),
            world.theater.rows as f64 * f64::from(CELL_FEET),
        ];
        let mut best: Option<(f64, [f64; 3], f64)> = None;
        let mut x = REACH * 2.;
        while x < size[0] - REACH * 2. {
            let mut z = REACH * 2.;
            while z < size[1] - REACH * 2. {
                let distance = (x - from[0]).hypot(z - from[1]);
                if distance < best.map_or(f64::MAX, |b| b.0) && !world.over_water(x, z) {
                    let centre = height(x, z);
                    let gx = (height(x + REACH, z) - height(x - REACH, z)) / (2. * REACH);
                    let gz = (height(x, z + REACH) - height(x, z - REACH)) / (2. * REACH);
                    let angle = gx.hypot(gz).atan().to_degrees();
                    let flat = (12. ..=20.).contains(&angle)
                        && [-2., -1., 0., 1., 2.].iter().all(|&i| {
                            [-2., -1., 0., 1., 2.].iter().all(|&j| {
                                let (dx, dz) = (i * REACH, j * REACH);
                                (height(x + dx, z + dz) - (centre + gx * dx + gz * dz)).abs() < 25.
                            })
                        });
                    if flat {
                        best = Some((distance, [x, centre, z], gx.atan2(gz)));
                    }
                }
                z += STEP;
            }
            x += STEP;
        }
        best.map(|(_, site, yaw)| (site, yaw))
    }
    /// Developer preview (`TORE_EFFECT_PREVIEW=1`): every explosion type
    /// 15 to 38 halfway through its animation in a row 2,500 feet ahead,
    /// surface types on the ground, and a burning crash site with its
    /// column and three weapon craters on the ground 4,000 feet ahead.
    pub fn preview_effects(&mut self, s: &flight::State, world: &Terrain) {
        use tore_sim::combat::{blast, smoke};
        let (sin, cos) = s.yaw.sin_cos();
        let (forward, right) = ([sin, 0., cos], [cos, 0., -sin]);
        let at = |ahead: f64, across: f64| -> Vector {
            let x = s.position[0] + forward[0] * ahead + right[0] * across;
            let z = s.position[2] + forward[2] * ahead + right[2] * across;
            [x, f64::from(world.height(x as f32, z as f32)), z]
        };
        // `hill` shows craters alone: six of them in two rows on the slope.
        let hill = std::env::var_os("TORE_EFFECT_PREVIEW").is_some_and(|mode| mode == "hill");
        if hill {
            for (n, (ahead, across, size)) in [
                (2200., -220., 18),
                (2600., -220., 9),
                (3000., -220., 18),
                (2200., 220., 18),
                (2600., 220., 3),
                (3000., 220., 9),
            ]
            .into_iter()
            .enumerate()
            {
                self.state.marks.push(blast::Mark {
                    position: at(ahead, across),
                    kind: blast::MarkKind::Crater(size),
                    ticks: blast::FOREVER,
                    born: 0,
                    scale: tore_sim::combat::blast_scale::FULL,
                    serial: u64::MAX - n as u64,
                });
            }
            return;
        }
        for kind in blast::FIRST..=blast::LAST {
            let row = blast::explosion(kind).expect("table type");
            let mut position = at(2500., (f64::from(kind) - 26.5) * 120.);
            if !row.surface {
                position[1] = s.position[1];
            }
            self.state.effects.push(live::Effect {
                position,
                kind: live::EffectKind::Destroyed,
                ticks: u16::from(row.seconds) * 60,
                blast: Some(kind),
                scale: tore_sim::combat::blast_scale::FULL,
            });
        }
        let site = at(4000., 0.);
        self.state.aircraft_crashed(u32::MAX, site, false);
        for (across, size) in [(-900., 3), (-600., 9), (900., 18)] {
            self.state.marks.push(blast::Mark {
                position: at(4000., across),
                kind: blast::MarkKind::Crater(size),
                ticks: blast::FOREVER,
                born: 0,
                scale: tore_sim::combat::blast_scale::FULL,
                serial: u64::MAX - size as u64,
            });
        }
        for _ in 0..1200 {
            self.state
                .smoke
                .step([([site[0], site[1] + 20., site[2]], smoke::Kind::Burning)]);
        }
    }
    /// The plane the render history is drawn for, `None` while it holds
    /// nothing.
    pub fn render_plane(&self) -> Option<u32> {
        self.render.plane
    }
    /// Empties the render history, as when no human is left to draw for. An
    /// empty one stays as it is.
    pub fn clear_render(&mut self) {
        if self.render.plane.is_some() {
            self.render.restart();
        }
    }
    /// The latest tick's snapshot, uninterpolated.
    pub fn render_snapshot(&self) -> &RenderSnapshot {
        &self.render.current
    }
    /// The snapshot one tick before [`Self::render_snapshot`]; none right
    /// after a restart.
    pub fn previous_snapshot(&self) -> Option<&RenderSnapshot> {
        self.render.previous.as_ref()
    }
    /// How many times the render history has started over, at a reset or a
    /// restart. A frame's tick fraction belongs to one history only.
    pub fn render_restarts(&self) -> u64 {
        self.render.restarts
    }
    /// One target in the latest snapshot.
    pub fn current_target(&self, id: u32) -> Option<&AircraftPose> {
        self.render.current_target(id)
    }
    /// One target in the snapshot before the latest.
    pub fn previous_target(&self, id: u32) -> Option<&AircraftPose> {
        self.render.previous_target(id)
    }
    /// The other aircraft types this mission loaded, in draw order.
    pub fn dummy_types(&self) -> &[Arc<AircraftType>] {
        &self.dummy_types
    }
    /// The live-fire configuration of each of [`Self::dummy_types`].
    pub fn dummy_configurations(&self) -> &[live::Configuration] {
        &self.dummy_configs
    }
    /// Where the player's aircraft's engines exhaust.
    pub fn contrail_offsets(&self) -> &[Vector] {
        &self.contrail_offsets
    }
    /// A ground object of the airport scene.
    pub fn ground_object(&self, id: u32) -> Option<&tore_sim::airport::StaticObject> {
        self.airport_objects.iter().find(|o| o.id == id)
    }
    /// Populate all six creator wings, retaining their sides for placement.
    /// `load` supplies the type of each aircraft the wings fly, the first
    /// time it is needed.
    pub fn mission_aircraft(
        &mut self,
        wings: &[tore_sim::ai::launch::WingLaunch],
        layout: &crate::mission_layout::MissionLayout,
        data: &dyn ResourceSource,
        load: &mut dyn FnMut(AircraftId) -> WorldResult<Arc<AircraftType>>,
    ) -> WorldResult<()> {
        self.mission_dummies(
            &tore_sim::ai::launch::legacy_pairs(wings),
            layout.enemy.distance_ft,
            data,
            load,
        )?;
        // An open mission's wings hold plane 0 as friendly wing 1's first
        // member; single player's player leads that wing outside them.
        self.mission_spawns = Some(crate::ai_wings::mission_spawns_for(
            wings,
            &layout.spawn_plan(),
            !self.open,
        ));
        self.mission_layout = Some(layout.clone());
        Ok(())
    }

    /// Adds straight-flight fixtures of the given aircraft. `load` supplies
    /// the type of each aircraft the first time it is needed; the caller
    /// keeps whatever draws it.
    pub fn mission_dummies(
        &mut self,
        wings: &[(AircraftId, usize)],
        separation: f64,
        data: &dyn ResourceSource,
        load: &mut dyn FnMut(AircraftId) -> WorldResult<Arc<AircraftType>>,
    ) -> WorldResult<()> {
        for (id, count) in wings {
            if *count == 0 {
                continue;
            }
            let model = if let Some(i) = self.dummy_types.iter().position(|h| h.profile.id == *id) {
                i
            } else {
                let h = load(*id)?;
                self.dummy_configs
                    .push(live::Configuration::from_source(&h.profile, |name| {
                        data.get(name)
                            .cloned()
                            .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
                    })?);
                self.dummy_types.push(h);
                self.dummy_types.len() - 1
            };
            for _ in 0..*count {
                let n = self.dummies.len();
                // Fitted stagger: first contact straight ahead, successive pairs
                // 500 feet to either side, 500 feet deeper per pair.
                let side = if n == 0 {
                    0.
                } else if n % 2 == 1 {
                    1.
                } else {
                    -1.
                };
                let row = n.div_ceil(2) as f64;
                self.dummies
                    .push((model, [side * row * 500., 0., separation + row * 500.]));
            }
        }
        Ok(())
    }
    /// A command for the first ownship, as a host with one flight gives it.
    pub fn command(&mut self, command: live::Command, l: Launcher) {
        let aircraft = self.own_id();
        self.command_for(aircraft, command, l);
    }
    /// A command for one ownship. The combat tape and the command notes follow
    /// the host's plane only ([`Self::host_plane`]).
    pub fn command_for(&mut self, aircraft: u32, command: live::Command, l: Launcher) {
        let first = Some(aircraft) == self.host_plane();
        if first && self.tape.is_some() {
            self.record_tape(crate::combat_tape::command_name(command), l);
        }
        if first {
            self.note(CommandNote::Command(command));
        }
        self.state.command(aircraft, command, l);
        if first {
            self.last_launcher = Some(l);
        }
    }
    fn note(&mut self, note: CommandNote) {
        if self.notes.len() == MAX_COMMAND_NOTES {
            self.notes.pop_front();
        }
        self.notes.push_back(note);
    }
    /// Player commands since the last call, oldest first. For mission
    /// recordings; flight never reads them.
    pub fn take_notes(&mut self) -> Vec<CommandNote> {
        self.notes.drain(..).collect()
    }
    /// Starts collecting the combat tape's records.
    pub fn start_tape(&mut self) {
        self.tape.get_or_insert_with(Vec::new);
    }
    /// Stops collecting; records not yet taken are dropped.
    pub fn stop_tape(&mut self) {
        self.tape = None;
    }
    /// A tape is being collected.
    pub fn recording_tape(&self) -> bool {
        self.tape.is_some()
    }
    /// Adds one record to the tape, when one is being collected.
    pub fn record_tape(&mut self, action: impl Into<String>, launcher: Launcher) {
        if let Some(tape) = &mut self.tape {
            tape.push(crate::combat_tape::Entry {
                action: action.into(),
                launcher,
            });
        }
    }
    /// The records collected since the last call, oldest first. Write-only:
    /// nothing in combat reads them.
    pub fn take_tape(&mut self) -> Vec<crate::combat_tape::Entry> {
        self.tape.as_mut().map(std::mem::take).unwrap_or_default()
    }
    /// The first ownship lets go of its trigger.
    pub fn cancel(&mut self) {
        let aircraft = self.own_id();
        self.cancel_for(aircraft);
    }
    /// One ownship lets go of its trigger and drops its queued rounds.
    pub fn cancel_for(&mut self, aircraft: u32) {
        let first = Some(aircraft) == self.host_plane();
        if first
            && self.tape.is_some()
            && let Some(l) = self.last_launcher
        {
            self.record_tape("release", l);
        }
        let trigger = self.trigger(aircraft);
        let held = trigger.held();
        trigger.input.cancel();
        trigger.controller.cancel();
        if first && held {
            self.note(CommandNote::Release);
        }
        self.state.release(aircraft);
    }
    /// One ownship's trigger.
    pub fn trigger(&mut self, aircraft: u32) -> &mut Trigger {
        self.triggers.entry(aircraft).or_default()
    }
    /// The first ownship's trigger.
    pub fn own_trigger(&mut self) -> &mut Trigger {
        let aircraft = self.own_id();
        self.trigger(aircraft)
    }
    /// Puts another human-flown aircraft into combat with the stores, damage
    /// and countermeasures its ownship carries; `contrail_offsets` are where
    /// its engines exhaust. The aircraft must not have an ownship yet, and its
    /// id must not be an AI target's.
    pub fn add_ownship(
        &mut self,
        ownship: live::Ownship,
        contrail_offsets: Vec<Vector>,
    ) -> WorldResult<()> {
        let aircraft = ownship.aircraft;
        if self.state.targets.iter().any(|t| t.id == aircraft) {
            return Err(format!("aircraft {aircraft} is already an AI target").into());
        }
        self.state
            .add_ownship(ownship)
            .map_err(std::io::Error::other)?;
        self.ownship_contrails.insert(aircraft, contrail_offsets);
        Ok(())
    }
    /// Takes an aircraft out of combat when its human gives it back, with its
    /// ownship: stores, damage and countermeasures as they stand. Any
    /// ownship can go, the host's plane 0 and the last one too.
    pub fn remove_ownship(&mut self, aircraft: u32) -> Option<live::Ownship> {
        self.triggers.remove(&aircraft);
        self.ownship_contrails.remove(&aircraft);
        self.poses.remove(&aircraft);
        self.state.remove_ownship(aircraft)
    }
    /// Starts combat over from the start `s`. With a host, its plane 0 gets
    /// a fresh ownship on the flight `s`, which takes the ownship's payload
    /// and systems; the mission's aircraft take their places around `s`. In
    /// an open mission `s` is only where the lead of Friendly Wing 1 starts:
    /// the mission's aircraft, plane 0 first, take their places around it,
    /// every ownship goes and the render history is left empty, as no human
    /// flies yet.
    pub fn reset(&mut self, s: &mut flight::State) -> WorldResult<()> {
        self.render.restart();
        self.contrails = Default::default();
        self.contrail_sortie = self.contrail_sortie.wrapping_add(1);
        let l = launcher(s);
        self.record_tape(if self.range { "reset" } else { "reset-scene" }, l);
        self.last_launcher = Some(l);
        self.terrain_tti.clear();
        let weapon_rules = self.state.weapon_rules;
        let energy_seed = self.state.energy_seed();
        let friendly_fire = self.state.friendly_fire;
        let host = if self.open {
            self.state = live::State::open_mission();
            None
        } else {
            let own = self
                .state
                .ownship(0)
                .ok_or("the host's plane 0 has no ownship to restart")?;
            let aircraft = own.aircraft;
            self.state = live::State::for_ownship(
                aircraft,
                own.side,
                own.configuration().clone(),
                s.native.is_none() && !self.clean_recording,
            )?;
            Some(aircraft)
        };
        self.state.weapon_rules = weapon_rules;
        self.state.set_energy_seed(energy_seed);
        self.state.friendly_fire = friendly_fire;
        if weapon_rules == tore_sim::combat::missiles::Rules::Compatibility {
            self.record_tape("compatibility-weapons", l);
        }
        // The keyboard trigger starts over; the controller's is only let go.
        let controller = host.and_then(|aircraft| self.triggers.remove(&aircraft));
        self.triggers.clear();
        self.ownship_contrails.clear();
        self.poses.clear();
        if let Some(aircraft) = host {
            self.reset_host(aircraft, s, controller.map(|trigger| trigger.controller))?;
        }
        let first_row = self.first_row();
        for (index, (model, offset)) in self.dummies.iter().enumerate() {
            let fixture_position = std::array::from_fn(|i| {
                l.position[i] + l.basis.right[i] * offset[0] + l.basis.forward[i] * offset[2]
            });
            let (position, basis) = self
                .mission_spawns
                .as_ref()
                .map(|spawns| spawns[index].pose(l.position, l.basis))
                .unwrap_or((fixture_position, l.basis));
            // A mission's aircraft fly for their wing's side; straight-flight
            // fixtures are the player's opponents.
            let side = self.mission_spawns.as_ref().map_or(ENEMY_SIDE, |spawns| {
                if spawns[index].opposing {
                    ENEMY_SIDE
                } else {
                    FRIENDLY_SIDE
                }
            });
            self.state
                .add_dummy(&self.dummy_configs[*model], position, basis, side);
            debug_assert_eq!(
                self.state.targets.last().unwrap().id,
                index as u32 + first_row
            );
        }
        // Aircraft are spawned first, preserving their roster ordering.
        for object in &self.airport_objects {
            let side = self.ground_side(object.id);
            let look = self.ground_looks.get(&object.id).copied();
            let contact = self.weapon_boxes.get(&object.id).copied();
            Self::register_airport_object(&mut self.state, object, contact, side, look)?;
        }
        mark_teams(&mut self.state, &self.ground_teams);
        for (parked, _) in &self.parked {
            self.state.add_parked_aircraft(parked)?;
        }
        self.surface.reset();
        if let Some(aircraft) = host {
            self.restart_render(aircraft, s, None);
        }
        Ok(())
    }
    /// The host's half of [`Self::reset`]: its plane's fresh ownship loaded,
    /// its trigger, and the flight `s` it flies taking the ownship's payload
    /// and systems.
    fn reset_host(
        &mut self,
        aircraft: u32,
        s: &mut flight::State,
        controller: Option<FireInput>,
    ) -> WorldResult<()> {
        let own = self
            .state
            .ownship_mut(aircraft)
            .ok_or("the host's plane has an ownship")?;
        if let Some(ammo) = &self.initial_ammo {
            own.ammo.clone_from(ammo);
        }
        own.start_load();
        let mut controller = controller.unwrap_or_default();
        controller.cancel();
        self.triggers.insert(
            aircraft,
            Trigger {
                input: FireInput::default(),
                controller,
            },
        );
        let own = self
            .state
            .ownship(aircraft)
            .ok_or("the host's plane has an ownship")?;
        s.set_payload(own.payload_lbs())?;
        s.systems = tore_sim::aircraft_systems::Systems::new(
            own.configuration().engines,
            own.external_fuel_lbs(),
        );
        s.damage_fraction = 0.;
        s.damage_variant = None;
        s.damage_regions = [0.; live::DAMAGE_SECTIONS];
        s.bay = 0.;
        s.bay_open = false;
        s.bay_auto_open = false;
        if self.range {
            self.state.range_target(aircraft, launcher(s));
        }
        Ok(())
    }
    /// Opinionated (requested by John, 2026-09-29): an airborne mission
    /// aircraft never starts inside the terrain. After [`Self::reset`] placed
    /// the wings, any airborne aircraft lower than [`SPAWN_MIN_MSL_FT`] above
    /// sea level or [`SPAWN_MIN_AGL_FT`] above the ground under it is raised
    /// to the higher of the two. Runway starts are untouched, and so is a
    /// helicopter of the player's wing; a helicopter of the AI's other wings
    /// starts [`HELICOPTER_START_AGL_FT`] above the ground (slice ST-H).
    /// Returns how many aircraft were raised.
    pub fn raise_airborne_spawns(&mut self, world: &Terrain) -> usize {
        let Some(spawns) = self.mission_spawns.as_ref() else {
            return 0;
        };
        let mut raised = 0;
        for (index, spawn) in spawns.iter().enumerate() {
            if spawn.runway_order.is_some() {
                continue;
            }
            let Some(target) = self.state.targets.get_mut(index) else {
                continue;
            };
            let [x, y, z] = target.position;
            let ground = f64::from(world.height(x as f32, z as f32));
            let helicopter =
                !spawn.with_player && target.aircraft.is_some_and(AircraftId::is_helicopter);
            let settled = settled_spawn_height(y, ground, helicopter);
            if (settled - y).abs() > f64::EPSILON {
                target.position[1] = settled;
                raised += usize::from(settled > y);
            }
        }
        raised
    }
    /// One combat tick for the first ownship's aircraft, as a host with one
    /// flight steps it.
    /// The highest `world`'s terrain reaches over each square, built on
    /// first use and kept: the world's terrain never changes in a mission.
    pub(crate) fn ground_ceiling(
        &mut self,
        world: &Terrain,
    ) -> Arc<tore_sim::ground_ceiling::GroundCeiling> {
        self.ground_ceiling
            .get_or_insert_with(|| Arc::new(world.ground_ceiling()))
            .clone()
    }

    pub fn step(&mut self, s: &mut flight::State, world: &Terrain) -> WorldResult<Vec<Event>> {
        let aircraft = self.own_id();
        Ok(self.step_all(&mut [(aircraft, s)], world)?.events)
    }
    /// One combat tick for every human-flown aircraft given, each with its
    /// flight, in aircraft id order: every launcher goes into one combat step,
    /// then each flight takes what combat did to its aircraft: system faults,
    /// payload, bay, radar and jammer, damage and the crash, through the
    /// shared plane step's write-backs (`world::plane`). A flight whose
    /// aircraft has no ownship is left alone. The combat tape and the command
    /// notes follow the first ownship only.
    pub fn step_all(
        &mut self,
        flights: &mut [(u32, &mut flight::State)],
        world: &Terrain,
    ) -> WorldResult<Stepped> {
        self.step_all_rewound(flights, &[], world)
    }
    /// The contrail puffs of the step: each engine outlet of every aircraft
    /// flying at or above its contrail height, then the puffs moved on.
    fn step_contrails(
        &mut self,
        world: &Terrain,
        first: Option<u32>,
        flights: &[(u32, &mut flight::State)],
    ) {
        use tore_sim::combat::smoke::contrail_altitude_ft;
        self.contrails.wind = world.wind();
        let sortie = self.contrail_sortie;
        let mut outlets = Vec::new();
        let mut add = |id: u32, position: Vector, basis: Basis, offsets: &[Vector]| {
            if position[1] < contrail_altitude_ft(sortie, id) {
                return;
            }
            for (engine, offset) in offsets.iter().enumerate() {
                let point = std::array::from_fn(|i| {
                    position[i]
                        + basis.right[i] * offset[0]
                        + basis.up[i] * offset[1]
                        + basis.forward[i] * offset[2]
                });
                outlets.push((u64::from(id) * 2 + engine as u64, point));
            }
        };
        let offsets_of = |aircraft: u32| -> &[Vector] {
            if Some(aircraft) == first {
                &self.contrail_offsets
            } else {
                self.ownship_contrails
                    .get(&aircraft)
                    .map_or(&[][..], Vec::as_slice)
            }
        };
        for (aircraft, s) in flights.iter() {
            let Some(own) = self.state.ownship(*aircraft) else {
                continue;
            };
            let height = f64::from(world.height(s.position[0] as f32, s.position[2] as f32));
            if !s.crashed && s.engine && s.fuel > 0. && own.hp > 0 && !s.supported_at(height) {
                add(
                    *aircraft,
                    s.position,
                    launcher(s).basis,
                    offsets_of(*aircraft),
                );
            }
        }
        for target in self.state.targets.iter().filter(|t| t.airborne && t.hp > 0) {
            if let Some(id) = target.aircraft {
                if let Some(own) = self
                    .state
                    .ownships()
                    .iter()
                    .find(|own| own.configuration().aircraft == id)
                {
                    add(
                        target.id,
                        target.position,
                        target.basis,
                        offsets_of(own.aircraft),
                    );
                } else if let Some(other) = self.dummy_types.iter().find(|h| h.profile.id == id) {
                    add(
                        target.id,
                        target.position,
                        target.basis,
                        &other.contrail_offsets,
                    );
                }
            }
        }
        self.contrails.step([]);
        self.contrails.contrails(outlets);
    }
    /// [`Self::step_all`] with lag compensation: `rewinds` gives, by
    /// aircraft, the rewind in ticks ([`gun_rewind`]) of the gun rounds its
    /// seat fires this tick. An aircraft not listed fires rounds with none.
    pub fn step_all_rewound(
        &mut self,
        flights: &mut [(u32, &mut flight::State)],
        rewinds: &[(u32, u16)],
        world: &Terrain,
    ) -> WorldResult<Stepped> {
        flights.sort_by_key(|(aircraft, _)| *aircraft);
        let first = self.host_plane();
        let mut inputs = Vec::new();
        let mut events = Vec::new();
        for (aircraft, s) in flights.iter() {
            let aircraft = *aircraft;
            if self.state.ownship(aircraft).is_none() {
                continue;
            }
            let l = launcher(s);
            let held = self.triggers.get(&aircraft).is_some_and(Trigger::held);
            if Some(aircraft) == first {
                self.last_launcher = Some(l);
                self.record_tape(if held { "fire" } else { "tick" }, l);
            }
            let energy = self.state.weapon_rules == tore_sim::combat::missiles::Rules::Energy;
            if let Some(own) = self.state.ownship_mut(aircraft) {
                if energy {
                    own.heat = crate::ai_wings::energy_heat(s, own.heat);
                }
                own.note_loaded();
            }
            if s.airburst()
                && let Some(event) = self.state.ownship_airburst(aircraft, s.position)
            {
                events.push(event);
            }
            if s.ground_impact()
                && let Some(event) = self.state.ownship_ground_impact(
                    aircraft,
                    s.position,
                    world.over_water(s.position[0], s.position[2]),
                )
            {
                events.push(event);
            }
            inputs.push(OwnshipInput {
                aircraft,
                held,
                launcher: l,
            });
        }
        self.state.smoke.wind = world.wind();
        self.state.devices.wind = world.wind();
        let ceiling = self.ground_ceiling(world);
        self.state.set_ground_ceiling(Some(ceiling));
        // Stop wreck emissions before advancing smoke on the impact/airburst tick.
        events.extend(self.state.step_rewound(
            &inputs,
            rewinds,
            |x, z| f64::from(world.height(x as f32, z as f32)),
            |x, z| world.over_water(x, z),
        ));
        // The city fill's buildings within a ground burst's flattening reach
        // fall (scenery only: nothing is scored).
        for burst in self.state.ground_bursts.drain(..) {
            if let Some(fill) = &world.city_fill {
                fill.flatten(
                    burst.position,
                    crate::city::flatten_radius(burst.radius, burst.percent),
                );
            }
        }
        for (aircraft, s) in flights.iter_mut() {
            let aircraft = *aircraft;
            let Some(own) = self.state.ownship(aircraft) else {
                continue;
            };
            plane::take_system_hits(
                s,
                &OwnshipTerms::of(own, self.state.tick()),
                own.configuration(),
            );
            let mut uncredited = false;
            if s.crashed
                && let Some(event) = self.state.systems_destroyed(aircraft)
            {
                events.push(event);
                // Lost to its own structure (overspeed, the map edge).
                uncredited = s.systems.structure.cause.is_some();
            }
            // A loss with no shooter credits nobody, even after an earlier hit,
            // as for an AI aircraft (lead's decision, 2026-09-29).
            if uncredited {
                self.state.ledger.lose_without_credit(aircraft);
            }
        }
        if self.contrail_puffs {
            self.step_contrails(world, first, flights);
        }
        let mut terms = Vec::with_capacity(flights.len());
        for (aircraft, s) in flights.iter_mut() {
            let aircraft = *aircraft;
            let Some(own) = self.state.ownship(aircraft) else {
                continue;
            };
            let own_terms = OwnshipTerms::of(own, self.state.tick());
            plane::take_combat(s, aircraft, &own_terms, own.configuration(), &events)?;
            terms.push((aircraft, own_terms));
            self.poses.insert(aircraft, Pose::of(s));
        }
        // A station that ran dry hands the selection on, but never while the
        // trigger is held: the next store must not fire from the same press.
        for input in &inputs {
            let aircraft = input.aircraft;
            let unlimited_ammo = self.state.cheats.unlimited_ammo;
            let dry = self
                .state
                .ownship(aircraft)
                .is_some_and(|own| own.armed && !own.carries(own.selected, unlimited_ammo));
            if dry && !self.triggers.get(&aircraft).is_some_and(Trigger::held) {
                self.command_for(aircraft, live::Command::AdvanceFromEmpty, input.launcher);
            }
        }
        Ok(Stepped { events, terms })
    }
}

impl Pose {
    fn of(s: &flight::State) -> Self {
        Self {
            position: s.position,
            attitude: [s.yaw, s.pitch, s.bank],
            velocity: s.velocity,
            devices: crate::snapshot::devices(s),
            engine: Engine {
                lit: s.engine && s.fuel > 0.,
                afterburner: s.afterburner_active(),
                rates: s.auxiliary_rates,
                rotor: s.lift_controls.drive.rotor_speed,
                rotor_turns: crate::snapshot::rotor_pose(s).0,
                rotor_tilt: crate::snapshot::rotor_pose(s).1,
                flame: s.afterburner_active() && s.escape.is_none(),
            },
            wreck: s.wreck.as_ref().map(|wreck| wreck.phase),
            crashed: s.crashed,
            escape: s.escape.clone(),
            crew_escape: s.crew_escape.clone(),
        }
    }
}

pub(crate) fn apply_startup_weapon_state(
    own: &mut live::Ownship,
    guns_only: bool,
    unlimited_ammo: bool,
) {
    // The gun is the startup weapon. A gun station that carries nothing falls
    // back to the first station that does, and an aircraft with nothing
    // loaded starts on NAV, so an empty station never shows up armed.
    let stations = &own.configuration().stations;
    let gun = stations
        .iter()
        .position(|station| live::is_gun(&station.weapon))
        .filter(|index| own.carries(*index, unlimited_ammo));
    let choice = gun.or_else(|| {
        (0..stations.len()).find(|index| {
            own.carries(*index, unlimited_ammo) && own.station_allowed(*index, guns_only)
        })
    });
    match choice {
        Some(index) => {
            own.selected = index;
            own.armed = true;
        }
        None => own.armed = false,
    }
}

/// Haptics follow confirmed ownship events. A distant target explosion is not
/// player damage; incoming fixture launches must not feel like own launches.
pub fn feedback(
    event: &Event,
    aircraft: u32,
    config: &live::Configuration,
) -> Option<tore_input::FeedbackEvent> {
    use tore_input::FeedbackEvent as F;
    match event {
        Event::Fired {
            aircraft: who,
            station,
        } if *who == aircraft => Some(if config.stations[*station].internal {
            F::GunFired
        } else {
            F::MissileLaunched
        }),
        Event::OwnshipDamaged { aircraft: who, .. } if *who == aircraft => Some(F::Damage),
        Event::OwnshipDestroyed { aircraft: who } if *who == aircraft => Some(F::Crash),
        _ => None,
    }
}

/// A flown aircraft's stores as drawn: each station's rounds left, and the
/// stations its mission started with (B3).
fn own_stores(own: &live::Ownship) -> Vec<crate::snapshot::StorePose> {
    crate::snapshot::store_poses_of(own.configuration(), &own_station_stores(own))
}

/// A flown aircraft's drawn stations and the whole stores on each (B3).
pub fn own_station_stores(own: &live::Ownship) -> Vec<crate::snapshot::StationStores> {
    crate::snapshot::station_stores(
        own.configuration(),
        |index| u32::from(own.rounds(index)),
        |index| own.was_loaded(index),
    )
}

/// Each AI aircraft's loadout, drawn stations and whole stores on each, in
/// actor order (B3). An unlimited store shows its loaded quantity.
pub fn ai_station_stores(
    wings: &crate::ai_wings::AiWings,
) -> impl Iterator<
    Item = (
        u32,
        &live::Configuration,
        Vec<crate::snapshot::StationStores>,
    ),
> + '_ {
    wings.mission().actors().iter().filter_map(move |actor| {
        let config = wings.configuration(actor.id())?;
        let rounds = |index: usize| {
            let loaded = config.stations.get(index).map_or(0, |s| {
                u32::from(s.count) * tore_sim::combat::live::pod_rounds(&s.weapon)
            });
            actor
                .stations()
                .iter()
                .find(|spec| usize::from(spec.station.0) == index)
                .map_or(loaded, |spec| match spec.rounds() {
                    tore_sim::ai::weapon_service::Rounds::Finite(rounds) => rounds,
                    tore_sim::ai::weapon_service::Rounds::Unlimited => loaded,
                })
        };
        let loaded = |index: usize| config.stations.get(index).is_some_and(|s| s.count > 0);
        Some((
            actor.id(),
            config,
            crate::snapshot::station_stores(config, rounds, loaded),
        ))
    })
}

impl Combat {
    /// Every aircraft's drawn stations and the whole stores on each, by
    /// aircraft id (B3): the human-flown planes from their cockpits, the
    /// AI's from its weapon service. What the host tells clients (protocol
    /// 26) and what a recording keeps.
    pub fn station_stores(
        &self,
        wings: Option<&crate::ai_wings::AiWings>,
    ) -> BTreeMap<u32, Vec<crate::snapshot::StationStores>> {
        let mut out: BTreeMap<u32, Vec<crate::snapshot::StationStores>> = wings
            .into_iter()
            .flat_map(ai_station_stores)
            .map(|(id, _, loads)| (id, loads))
            .collect();
        for own in self.state.ownships() {
            out.insert(own.aircraft, own_station_stores(own));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inert_body_remains_visible_without_incoming_or_target_cues() {
        let (mut combat, flight) = fixtures::loaded([500, 3]);
        combat.state.weapon_rules = tore_sim::combat::missiles::Rules::Energy;
        let own = combat.own_id();
        combat.state.own_mut().selected = 1;
        let l = launcher(&flight);
        combat.state.command(own, live::Command::Incoming, l);
        let p = &mut combat.state.projectiles[0];
        // The rendering fixture's named missile starts as a gun record.
        // Give this capability test an actual synthetic guided weapon.
        let w = p.weapon.as_mut().unwrap();
        w.source = "AIM120.JT".into();
        w.flags = 0x1204f;
        w.seeker.signature = 3;
        p.target = Some(own);
        p.motion = Some(tore_sim::combat::missiles::Motion::launch_rules(
            p.weapon.as_ref().unwrap(),
            [0., 0., -1000.],
            p.position[1],
            tore_sim::combat::missiles::Rules::Energy,
        ));
        assert!(
            !combat
                .cockpit_readout(own, l, None, None)
                .unwrap()
                .rwr
                .inbound
                .is_empty()
        );
        let p = &mut combat.state.projectiles[0];
        p.motion
            .as_mut()
            .unwrap()
            .energy
            .as_mut()
            .unwrap()
            .flight
            .defeated = true;
        assert!(
            combat
                .cockpit_readout(own, l, None, None)
                .unwrap()
                .rwr
                .inbound
                .is_empty()
        );
        let picture = combat.snapshot(own, &flight, None);
        assert_eq!(picture.projectiles.len(), 1);
        assert!(!picture.projectiles[0].incoming);
        assert_eq!(picture.projectiles[0].target, None);
    }
    #[test]
    fn an_airborne_start_is_raised_clear_but_an_ai_helicopter_starts_at_its_own_height() {
        // The 5,000 ft floor above sea level and the 1,000 ft above the
        // ground (John, 2026-09-29): raised, never lowered.
        assert_eq!(settled_spawn_height(3_000., 0., false), 5_000.);
        assert_eq!(settled_spawn_height(3_000., 6_000., false), 7_000.);
        assert_eq!(settled_spawn_height(9_000., 6_000., false), 9_000.);
        // A helicopter of the AI's own wings: 1,500 ft above the ground,
        // from above or below, and not raised to the sea-level floor.
        assert_eq!(settled_spawn_height(9_000., 200., true), 1_700.);
        assert_eq!(settled_spawn_height(100., 200., true), 1_700.);
        assert_eq!(settled_spawn_height(5_000., 6_000., true), 7_500.);
    }

    /// An AI's hit on a human-flown aircraft is credited to the AI (John,
    /// 2026-09-29), but not when the aircraft is then lost with no shooter:
    /// to overspeed or the map edge (lead's decision, 2026-09-29).
    #[test]
    fn an_ai_hit_is_not_credited_for_a_loss_with_no_shooter() {
        use tore_sim::aircraft_systems::LossCause;
        let world = crate::test_support::terrain();
        let (mut c, mut f) = fixtures::loaded([500, 3]);
        let own = c.own_id();
        f.position = [2000., 8000., 2000.];
        let l = launcher(&f);
        c.state.command(own, live::Command::Incoming, l);
        let forward = l.basis.forward;
        let p = &mut c.state.projectiles[0];
        p.owner = 7;
        p.incoming = None;
        p.position = std::array::from_fn(|i| l.position[i] + forward[i] * 100.);
        p.previous = p.position;
        let full = c.state.own().hp;
        for _ in 0..120 {
            c.step(&mut f, &world).unwrap();
            if c.state.own().hp < full {
                break;
            }
        }
        assert!(
            c.state.own().hp < full && c.state.own().hp > 0,
            "the AI round hit"
        );
        assert_eq!(c.state.ledger.credit(own).map(|k| k.owner), Some(7));
        f.systems.destroy(LossCause::Overspeed);
        f.crashed = true;
        c.step(&mut f, &world).unwrap();
        assert_eq!(c.state.own().hp, 0);
        assert_eq!(c.state.ledger.credit(own), None);
        assert!(c.state.ledger.kills().iter().all(|k| k.victim != own));
    }

    /// A human-flown aircraft lost to overspeed or the map edge
    /// credits nobody, even when a shooter hit it earlier; an ordinary crash
    /// still goes to the last shooter (lead's decision, 2026-09-29).
    #[test]
    fn an_ownship_lost_with_no_shooter_credits_nobody() {
        use tore_sim::aircraft_systems::LossCause;
        use tore_sim::combat::ledger::Kill;
        let world = crate::test_support::terrain();
        for cause in [
            Some(LossCause::Overspeed),
            Some(LossCause::OutOfBounds),
            None,
        ]
        .into_iter()
        .enumerate()
        {
            let (case, cause) = cause;
            let (mut c, mut f) = fixtures::loaded([500, 3]);
            let own = c.own_id();
            let hit = Kill {
                owner: 7,
                victim: own,
                category: 0x8000,
                aircraft: true,
            };
            c.state.ledger.damaged(hit);
            if let Some(cause) = cause {
                f.systems.destroy(cause);
                f.crashed = true;
            } else {
                // An ordinary crash with no cause of its own.
                f.systems = tore_sim::aircraft_systems::Systems::new(
                    c.state.own().configuration().engines,
                    c.state.own().external_fuel_lbs(),
                );
                f.crashed = true;
            }
            c.step(&mut f, &world).unwrap();
            assert_eq!(c.state.own().hp, 0, "case {case}");
            if cause.is_none() {
                assert_eq!(c.state.ledger.credit(own), Some(hit));
            } else {
                assert_eq!(c.state.ledger.credit(own), None, "case {case}");
            }
        }
    }

    #[test]
    fn fire_requires_unmodified_press_and_release_after_interruption() {
        let mut f = FireInput::default();
        f.space(true, false, true);
        assert!(!f.held);
        f.space(true, true, false);
        assert!(!f.held);
        f.space(false, false, false);
        f.space(true, false, false);
        assert!(f.held);
        f.cancel();
        assert!(!f.held);
        f.space(true, true, false);
        assert!(!f.held);
        f.space(false, false, true);
        f.space(true, false, false);
        assert!(f.held);
    }
}

#[cfg(test)]
mod ai_pose_tests {
    use super::*;
    use crate::snapshot::blend;
    use tore_formats::aircraft::AircraftId;
    use tore_sim::{
        combat::missiles::{TargetRole, seeker::Heat},
        sensors,
    };

    fn target(velocity: Vector, basis: Basis) -> live::Target {
        live::Target {
            aircraft: Some(AircraftId::F18),
            role: TargetRole::Aircraft,
            heat: Heat::Unknown,
            radar_emitting: false,
            id: 1,
            position: [0.; 3],
            velocity,
            basis,
            configuration: sensors::Configuration::CLEAN,
            signature: sensors::SignatureProfile::default(),
            jammer: None,
            jammer_active: false,
            airborne: true,
            on_ground: false,
            radius: 28.,
            hp: 100,
            initial_hp: 100,
            fragment_offsets: [[0.; 3]; 2],
            wreck: None,
            wreck_power: tore_sim::wreck::Power::default(),
            fragment_released: false,
            localized_damage: live::LocalizedDamage::default(),
            faults: Default::default(),
            category: 0,
            side: tore_sim::combat::live::NO_SIDE,
        }
    }

    /// A target's drawn pose as a snapshot records it.
    fn drawn(t: &live::Target) -> AircraftPose {
        AircraftPose {
            id: t.id,
            position: t.position,
            attitude: target_pose(t, true),
            ..Default::default()
        }
    }

    #[test]
    fn formation_rendering_shares_the_camera_tick_fraction() {
        // A fixed slot must stay fixed at every render fraction, including
        // frames without a simulation tick. 800 ft/s used to produce a
        // 6.67 ft (2.03 m) sawtooth when only the camera was interpolated.
        for turning in [false, true] {
            let mut t = target([0., 0., 800.], Basis::new(0., 0., 0.));
            let mut camera_before = [0.; 3];
            for tick in 0..240 {
                let offset = [512., 0., -512.];
                t.position = std::array::from_fn(|i| camera_before[i] + offset[i]);
                let previous = drawn(&t);
                let heading = if turning { tick as f64 * 0.001 } else { 0. };
                let camera_after: Vector = std::array::from_fn(|i| {
                    camera_before[i] + Basis::new(heading, 0., 0.).forward[i] * 800. / 120.
                });
                t.position = std::array::from_fn(|i| camera_after[i] + offset[i]);
                t.basis = Basis::new(heading, 0.1, 0.3);
                let authoritative = t.position;
                for alpha in [0., 0.13, 0.5, 0.91, 1.] {
                    let pose = blend(Some(&previous), &drawn(&t), alpha);
                    for i in 0..3 {
                        let camera =
                            camera_before[i] + (camera_after[i] - camera_before[i]) * alpha;
                        assert!((pose.position[i] - camera - offset[i]).abs() < 1e-9);
                    }
                    assert!(pose.attitude.iter().all(|a| a.is_finite()));
                    assert_eq!(t.position, authoritative);
                }
                camera_before = camera_after;
            }
            assert_eq!(
                blend(None, &drawn(&t), 0.5).position,
                t.position,
                "restart must discard history"
            );
        }
    }

    #[test]
    fn target_presentation_blends_attitude_and_keeps_new_targets_current() {
        let mut t = target([0., 0., 800.], Basis::new(359_f64.to_radians(), 0., 0.));
        let previous = drawn(&t);
        t.basis = Basis::new(1_f64.to_radians(), 0., 0.);
        let pose = blend(Some(&previous), &drawn(&t), 0.5);
        assert!(
            pose.attitude[0].sin().abs() < 1e-9,
            "heading must take the short path"
        );
        t.id = 2;
        t.position = [100.; 3];
        let pose = blend(None, &drawn(&t), 0.5);
        assert_eq!(
            (pose.position, pose.attitude),
            (t.position, t.basis.angles())
        );
    }

    /// The fixture rule is unchanged with AI poses disabled: heading from the
    /// velocity, level wings, whatever attitude the row happens to carry.
    #[test]
    fn the_fixture_pose_ignores_the_stored_attitude() {
        let banked = Basis::new(1.5, 0.4, 0.9);
        let t = target([300., 0., 0.], banked);
        let pose = target_pose(&t, false);
        assert!((pose[0] - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(pose[1], 0.);
        assert_eq!(pose[2], 0.);
    }

    /// With the option on the AI's own attitude is drawn, so a banking AI
    /// aircraft looks like one.
    #[test]
    fn the_ai_pose_uses_the_stored_attitude() {
        let banked = Basis::new(1.5, 0.4, 0.9);
        let t = target([300., 0., 0.], banked);
        let pose = target_pose(&t, true);
        for (got, want) in pose.iter().zip(banked.angles()) {
            assert!(
                (got - want).abs() < 1e-9,
                "{pose:?} vs {:?}",
                banked.angles()
            );
        }
        assert!(pose[2].abs() > 0.5, "bank was discarded: {pose:?}");
    }
}

/// Synthetic combat scenes for tests here and in the app, with no art: a
/// state with a gun and a missile station, other aircraft, fixtures, weapons,
/// effects, debris and pilots, and the snapshots live flight would take of
/// them. The app's drawing tests (`combat_view`) build on it.
#[cfg(any(test, feature = "test-support"))]
pub mod fixtures {
    use super::*;
    use tore_formats::{aircraft::AircraftId, weapons::Weapon};
    use tore_sim::attitude::unit;
    use tore_sim::{
        combat::{
            FallState,
            debris::Piece,
            live::{DamageSection, EffectKind, LocalizedDamage},
            missiles::{TargetRole, seeker::Heat},
        },
        ejection::{Escape, Phase},
        sensors,
    };

    /// The player's gun and one missile station with a loaded shape.
    fn state() -> live::State {
        let mut config = crate::test_support::combat_fixture(false)
            .own()
            .configuration()
            .clone();
        config.stations[0].weapon.source = "M61.JT".into();
        let mut missile = config.stations[0].clone();
        missile.weapon.source = "SYNMSL.JT".into();
        missile.weapon.shape = Some("SYNMSL.SH".into());
        config.stations.push(missile);
        live::State::new(config, true).unwrap()
    }
    /// A player combat state with a gun and a missile, loaded as given, and
    /// the flight it was reset with. The app's weapon-list tests use it.
    pub fn loaded(ammo: [u16; 2]) -> (Combat, flight::State) {
        let mut c = combat(vec![], vec![]);
        let mut config = c.state.own().configuration().clone();
        config.stations[0].weapon.source = AircraftId::F18.gun().unwrap().into();
        c.state = live::State::new(config, true).unwrap();
        c.initial_ammo = Some(ammo.to_vec());
        let mut f = flight::State::new(&crate::test_support::profile(), [0.; 3]).unwrap();
        c.reset(&mut f).unwrap();
        c.apply_startup_weapons();
        (c, f)
    }
    pub fn combat(dummy_types: Vec<Arc<AircraftType>>, dummies: Vec<(usize, Vector)>) -> Combat {
        Combat {
            state: state(),
            contrail_offsets: Vec::new(),
            contrail_sortie: 0,
            contrails: Default::default(),
            contrail_puffs: true,
            triggers: BTreeMap::new(),
            ownship_contrails: BTreeMap::new(),
            poses: BTreeMap::new(),
            range: false,
            ai_poses: true,
            clean_recording: false,
            open: false,
            initial_ammo: None,
            render: RenderHistory::default(),
            dummies,
            mission_spawns: None,
            mission_layout: None,
            dummy_types,
            dummy_configs: Vec::new(),
            airport_objects: Vec::new(),
            ground_sides: BTreeMap::new(),
            ground_looks: BTreeMap::new(),
            surface: Default::default(),
            parked: Vec::new(),
            surface_targets: Default::default(),
            ground_teams: Default::default(),
            weapon_boxes: BTreeMap::new(),
            tape: None,
            last_launcher: None,
            terrain_tti: Default::default(),
            notes: Default::default(),
            ground_ceiling: None,
        }
    }
    /// Synthetic types for the three other aircraft the scene draws, in the
    /// order of the drawn models.
    pub fn types() -> Vec<Arc<AircraftType>> {
        [AircraftId::F18, AircraftId::Rafale, AircraftId::F14]
            .into_iter()
            .map(|id| Arc::new(AircraftType::synthetic(id, Vec::new())))
            .collect()
    }
    /// Sets the other aircraft types the mission holds, as `Combat::mission_dummies`
    /// would have loaded them.
    pub fn set_types(combat: &mut Combat, types: Vec<Arc<AircraftType>>) {
        combat.dummy_types = types;
    }
    /// Sets where the player's aircraft's engines exhaust.
    pub fn set_contrail_offsets(combat: &mut Combat, offsets: Vec<Vector>) {
        combat.contrail_offsets = offsets;
    }
    /// Makes two snapshots the render history.
    pub fn set_history(combat: &mut Combat, previous: RenderSnapshot, current: RenderSnapshot) {
        combat.render = RenderHistory::default();
        combat.render.set_current(previous);
        combat.render.advance(current);
    }

    fn aircraft(
        id: u32,
        kind: AircraftId,
        position: Vector,
        velocity: Vector,
        basis: Basis,
    ) -> live::Target {
        live::Target {
            aircraft: Some(kind),
            role: TargetRole::Aircraft,
            heat: Heat::Unknown,
            radar_emitting: false,
            id,
            position,
            velocity,
            basis,
            configuration: sensors::Configuration::CLEAN,
            signature: sensors::SignatureProfile::default(),
            jammer: None,
            jammer_active: false,
            airborne: true,
            on_ground: false,
            radius: 28.,
            hp: 100,
            initial_hp: 100,
            fragment_offsets: [[0.; 3]; 2],
            wreck: None,
            wreck_power: tore_sim::wreck::Power::default(),
            fragment_released: false,
            localized_damage: LocalizedDamage::default(),
            faults: Default::default(),
            category: 0,
            side: tore_sim::combat::live::NO_SIDE,
        }
    }
    fn damaged(amounts: [i32; 6], section: Option<DamageSection>) -> LocalizedDamage {
        LocalizedDamage {
            amounts,
            structural_variant: section.map(|s| usize::from(s as u8 > 2)),
            structural_section: section,
        }
    }
    fn wreck(id: u32, phase: tore_sim::wreck::Phase) -> Option<tore_sim::wreck::Wreck> {
        let mut wreck = tore_sim::wreck::Wreck::new(id, 100, [0.; 3]);
        wreck.phase = phase;
        Some(wreck)
    }
    fn shot(
        id: u32,
        owner: u32,
        station: usize,
        weapon: Option<Weapon>,
        [position, previous]: [Vector; 2],
        direction: Vector,
        tracer: bool,
    ) -> live::Projectile {
        live::Projectile {
            id,
            owner,
            weapon,
            guidance: None,
            motion: None,
            guidance_ticks: None,
            age: 3,
            incoming: (owner != 0).then_some(0),
            station,
            position,
            previous,
            direction: unit(direction),
            speed_f8: 900 * 256,
            launched_t: 0,
            target: Some(1),
            fall: FallState::default(),
            gun_round: None,
            tracer,
        }
    }
    fn piece(owner: u32, position: Vector, basis: Basis) -> Piece {
        Piece {
            owner,
            variant: 0,
            position,
            velocity: [0.; 3],
            basis,
        }
    }

    pub struct Scene {
        pub previous: Vec<live::Target>,
        pub current: Vec<live::Target>,
        pub devices: Vec<(
            u32,
            [f64; crate::snapshot::DEVICES],
            [f64; crate::snapshot::DEVICES],
        )>,
        pub projectiles: Vec<live::Projectile>,
        pub effects: Vec<live::Effect>,
        pub debris: Vec<Piece>,
    }
    pub fn scene(config: &live::Configuration) -> Scene {
        use AircraftId::{F14, F18, Rafale};
        use tore_sim::wreck::Phase::{Exploded, Falling};
        let b = Basis::new;
        let mut current: Vec<_> = [
            (
                1,
                F18,
                [12., 5003., 3006.5],
                [80., 20., 700.],
                [0.25, 0.06, 0.5],
            ),
            (
                2,
                Rafale,
                [405., 5099., 2507.],
                [30., -5., 650.],
                [0.05, -0.12, -0.35],
            ),
            (
                3,
                F14,
                [-590., 4905., 3510.],
                [50., 25., 600.],
                [0.12, 0.21, 0.12],
            ),
            (
                4,
                F18,
                [201., 4795., 4005.],
                [10., -60., 500.],
                [0.3, -0.5, 1.2],
            ),
            (
                5,
                Rafale,
                [-300., 5200., 2800.],
                [0., -40., 400.],
                [5.9, -0.3, 2.],
            ),
            (6, F14, [900., 5000., 3000.], [0.; 3], [1., 0., 0.]),
            (7, F18, [150., 5050., 2200.], [0., 0., 750.], [0., 0.02, 0.]),
            (
                20,
                F18,
                [-800., 5100., 2600.],
                [-20., 0., 640.],
                [6.2, 0.1, -0.6],
            ),
            (100, F18, [300., 4000., 3000.], [0.; 3], [0.7, 0., 0.]),
            (101, F18, [320., 4000., 3040.], [0.; 3], [1.7, 0., 0.]),
        ]
        .into_iter()
        .map(|(id, kind, position, velocity, [yaw, pitch, bank])| {
            aircraft(id, kind, position, velocity, b(yaw, pitch, bank))
        })
        .collect();
        // Flying on with a failed left wing.
        current[1].hp = 60;
        current[1].localized_damage = damaged([0, 0, 5, 80, 0, 0], Some(DamageSection::LeftWing));
        // Destroyed through the nose and falling.
        current[3].hp = 0;
        current[3].localized_damage = damaged([90, 0, 0, 0, 0, 0], Some(DamageSection::Nose));
        current[3].wreck = wreck(4, Falling);
        // Destroyed without a structural break, wreck already exploded.
        current[4].hp = 0;
        current[4].localized_damage = damaged([30, 30, 20, 10, 5, 5], None);
        current[4].wreck = wreck(5, Exploded);
        // Parked.
        current[5].airborne = false;
        current[5].on_ground = true;
        // No model slot, carrying a left-wing break for its debris.
        current[7].localized_damage = damaged([0, 0, 0, 80, 0, 0], Some(DamageSection::LeftWing));
        // Ground objects, one standing and one destroyed.
        for (target, hp) in current[8..].iter_mut().zip([50, 0]) {
            target.aircraft = None;
            target.role = TargetRole::Surface;
            target.airborne = false;
            target.hp = hp;
        }
        let mut previous: Vec<_> = current
            .iter()
            .filter(|t| t.id != 20)
            .cloned()
            .map(|mut t| {
                let shift = f64::from(t.id % 10);
                t.position = [
                    t.position[0] - 3. * shift,
                    t.position[1] + 0.5 * shift,
                    t.position[2] - 6. - shift,
                ];
                t.velocity[0] -= 10.;
                let [yaw, pitch, bank] = t.basis.angles();
                t.basis = Basis::new(yaw - 0.02 * shift, pitch + 0.01, bank - 0.05 * shift);
                t
            })
            .collect();
        // A heading that crosses north between ticks.
        previous[1].basis = Basis::new(6.26, -0.1, -0.3);
        previous.push(aircraft(
            21,
            F18,
            [0., 5000., 0.],
            [0., 0., 700.],
            b(0., 0., 0.),
        ));
        let devices = vec![
            (
                1,
                [
                    1., 0.5, 0., 0., 0., 0.2, 0.1, -0.3, 0.05, 700., 0.6, 0., 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0.,
                ],
                [
                    0.8, 0.4, 0.3, 0.1, 0., 0.6, -0.2, 0.2, -0.1, 720., 0.9, 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0., 0.,
                ],
            ),
            (
                2,
                [
                    0.3, 0.2, 0.5, 0., 0., 1., 0.2, 0.1, 0.3, 600., 0.8, 0., 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0.,
                ],
                [
                    0.2, 0.25, 0.6, 0., 0., 0.9, 0.15, 0.2, 0.25, 610., 0.85, 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0., 0.,
                ],
            ),
            (
                3,
                [
                    0., 0.2, 0.4, 1., 0., 0.5, 0.3, -0.2, 0.2, 800., 0.5, 0., 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0.,
                ],
                [
                    0.5, 0.1, 0.3, 1., 0., 0.7, 0.25, -0.1, 0.25, 860., 0.55, 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0., 0.,
                ],
            ),
            (
                5,
                [
                    0., 0., 0., 0., 0., 0.4, 0.2, 0.2, 0.1, 500., 0.4, 0., 0., 0., 0., 0., 0., 0.,
                    0., 0., 0., 0.,
                ],
                [
                    0., 0., 0., 0., 0., 0.3, 0.1, 0.25, 0.15, 480., 0.3, 0., 0., 0., 0., 0., 0.,
                    0., 0., 0., 0., 0.,
                ],
            ),
        ];
        let weapon = |source: &str, shape: Option<&str>| {
            let mut weapon = config.stations[1].weapon.clone();
            weapon.source = source.into();
            weapon.shape = shape.map(Into::into);
            Some(weapon)
        };
        let projectiles = vec![
            shot(
                10,
                0,
                0,
                None,
                [[3., 5001., 130.], [0., 5000., 100.]],
                [3., 1., 30.],
                true,
            ),
            shot(
                11,
                0,
                0,
                None,
                [[5., 5002., 160.], [2., 5001., 130.]],
                [3., 1., 30.],
                false,
            ),
            shot(
                12,
                0,
                0,
                None,
                [[9., 5003., 90.], [9., 5003., 90.]],
                [0., 0., 1.],
                true,
            ),
            shot(
                13,
                0,
                1,
                None,
                [[100., 5020., 900.], [98., 5019., 880.]],
                [0.1, 0.05, 0.99],
                false,
            ),
            shot(
                14,
                1,
                0,
                weapon("AIMSL.JT", Some("AIMSL.SH")),
                [[300., 5040., 1500.], [301., 5041., 1520.]],
                [-0.05, -0.05, -1.],
                false,
            ),
            shot(
                15,
                2,
                0,
                weapon("NOSHAPE.JT", None),
                [[-200., 4990., 1800.], [-199., 4990., 1790.]],
                [-0.1, 0., 1.],
                false,
            ),
            shot(
                16,
                3,
                0,
                weapon("GSH301.JT", None),
                [[-100., 5010., 1200.], [-95., 5011., 1180.]],
                [-0.25, -0.05, 1.],
                true,
            ),
            shot(
                17,
                0,
                1,
                None,
                [[50., 5015., 700.], [49., 5015., 690.]],
                [0.1, 0., 1.],
                false,
            ),
        ];
        let effect = |kind, position, ticks| live::Effect {
            position,
            kind,
            ticks,
            blast: None,
            scale: tore_sim::combat::blast_scale::FULL,
        };
        let effects = vec![
            effect(EffectKind::Flare, [10., 5050., 500.], 45),
            effect(EffectKind::Flare, [14., 5040., 520.], 12),
            effect(EffectKind::Chaff, [-20., 5060., 480.], 30),
            effect(EffectKind::Launch, [0., 5000., 300.], 40),
            effect(EffectKind::Hit, [30., 5000., 900.], 45),
            effect(EffectKind::Hit, [35., 5005., 910.], 23),
            effect(EffectKind::Hit, [40., 5010., 920.], 1),
            effect(EffectKind::Destroyed, [200., 4800., 4000.], 240),
            effect(EffectKind::Destroyed, [210., 4790., 4010.], 121),
            effect(EffectKind::Destroyed, [220., 4780., 4020.], 7),
            effect(EffectKind::Ground, [0., 4000., 1500.], 45),
            effect(EffectKind::Ground, [10., 4000., 1510.], 3),
            effect(EffectKind::DebrisImpact, [-30., 4000., 1600.], 45),
            effect(EffectKind::DebrisImpact, [-40., 4000., 1610.], 20),
        ];
        let debris = vec![
            piece(0, [50., 5010., 2900.], b(0.4, 0.2, 0.9)),
            piece(1, [20., 5000., 3000.], b(1., 0., 0.3)),
            piece(2, [410., 5090., 2510.], b(2., 0.4, -1.)),
            piece(4, [210., 4780., 4010.], b(0.1, -0.2, 0.5)),
            piece(20, [-790., 5100., 2610.], b(0.3, 0.3, 0.3)),
            piece(99, [0., 5000., 2500.], b(0., 0., 0.)),
        ];
        Scene {
            previous,
            current,
            devices,
            projectiles,
            effects,
            debris,
        }
    }
    /// The presented player state that fixture targets copy.
    pub fn player() -> flight::State {
        let mut s = flight::State::new(&crate::test_support::profile(), [0., 5000., 0.]).unwrap();
        s.bay = 0.35;
        s.speed = 820.;
        s.throttle = 0.95;
        s.burner = true;
        s.exhaust = 0.8;
        s.gear = 1.;
        s.flaps = 0.5;
        s.brake = 0.2;
        s.hook = 0.4;
        s.elevator = 0.3;
        s.aileron = -0.2;
        s.rudder = 0.1;
        s.auxiliary_rates = [0.1, 0.4, -0.3];
        s.damage_variant = Some(DamageSection::LeftWing as usize);
        s
    }
    pub fn pilots() -> Vec<Escape> {
        [
            Phase::Seat,
            Phase::Freefall,
            Phase::Inflating,
            Phase::Parachute,
            Phase::Landed,
            Phase::Impact,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, phase)| {
            let i = i as f64;
            let mut pilot = Escape::new(
                [100. * i, 5000. - 50. * i, 2000. + 30. * i],
                [0.; 3],
                Basis::new(0.4 * i, 0., 0.),
            );
            pilot.phase = phase;
            pilot
        })
        .collect()
    }
    /// Loads the scene as live flight would hold it: the previous and
    /// current tick's snapshots, each aircraft's devices as the AI simulated
    /// them on those ticks.
    pub fn load(combat: &mut Combat, scene: &Scene, ai_poses: bool, player: &flight::State) {
        let [previous, current] = snapshots(combat, scene, ai_poses, player);
        set_history(combat, previous, current);
    }
    /// The scene's previous and current tick as live flight snapshots them,
    /// with each aircraft's devices as the AI simulated them on those ticks.
    /// Leaves the current tick's scene in `combat.state`.
    pub fn snapshots(
        combat: &mut Combat,
        scene: &Scene,
        ai_poses: bool,
        player: &flight::State,
    ) -> [RenderSnapshot; 2] {
        combat.ai_poses = ai_poses;
        combat.render = RenderHistory::default();
        combat.state.targets.clone_from(&scene.previous);
        let mut previous = combat.snapshot(combat.own_id(), player, None);
        combat.state.targets.clone_from(&scene.current);
        combat.state.projectiles.clone_from(&scene.projectiles);
        combat.state.effects.clone_from(&scene.effects);
        combat.state.debris.clone_from(&scene.debris);
        let mut current = combat.snapshot(combat.own_id(), player, None);
        for &(id, before, after) in &scene.devices {
            for (snapshot, devices) in [(&mut previous, before), (&mut current, after)] {
                if let Some(pose) = snapshot.targets.iter_mut().find(|pose| pose.id == id) {
                    pose.devices = Some(devices);
                }
            }
        }
        [previous, current]
    }

    #[test]
    fn gun_mounts_and_membership_pass_from_combat_to_draw_clones() {
        let player = player();
        let mut combat = combat(types(), vec![]);
        let gunship = tore_sim::combat::gunship::State {
            stations: [Some(0), Some(1), None],
            included: [true, false, true],
            headings: [-std::f64::consts::FRAC_PI_2, -1.2, -1.8],
            elevations: [0.1, -0.2, 0.3],
            sight: tore_sim::combat::gunship::Sight::Tracked(99),
            status: [live::Readiness::Ready; 3],
            ..Default::default()
        };
        let expected = gunship.normalized_devices();
        combat.state.ownship_mut(0).unwrap().gunship = Some(gunship);
        let picture = combat.snapshot(0, &player, None);
        let devices = picture.player.devices.unwrap();
        assert_eq!(
            &devices[crate::snapshot::GUN_AIM..crate::snapshot::GUN_GROUP],
            &expected
        );
        assert_eq!(devices[crate::snapshot::GUN_GROUP], 5.);
        let mut drawn = player.clone();
        crate::snapshot::set_gun_devices(&mut drawn, &devices);
        assert_eq!(drawn.gun_aim.concat(), expected);
        assert_eq!(drawn.gun_group, 5);
        assert_eq!(drawn.throttle, player.throttle);
        assert_eq!(drawn.lift_controls, player.lift_controls);
        assert_eq!(player.gun_aim, [[0.; 2]; 3]);
        let mut previous = picture.clone();
        previous.player.devices.as_mut().unwrap()[crate::snapshot::GUN_GROUP] = 1.;
        let blended = crate::snapshot::interpolate(Some(&previous), &picture, 0.5);
        assert_eq!(
            blended.player.devices.unwrap()[crate::snapshot::GUN_GROUP],
            5.
        );
    }

    #[test]
    fn snapshots_route_each_aircraft_and_piece_to_the_model_that_draws_it() {
        use crate::snapshot::Draw;
        let player = player();
        let mut with_models = combat(types(), (0..7).map(|i| (i % 3, [0.; 3])).collect());
        let scene = scene(with_models.state.own().configuration());
        load(&mut with_models, &scene, true, &player);
        let snapshot = with_models.render_snapshot();
        let draw = |id| snapshot.target(id).unwrap().draw;
        assert_eq!(draw(1), Draw::Model(AircraftId::F18));
        assert_eq!(draw(2), Draw::Model(AircraftId::Rafale));
        assert_eq!(draw(3), Draw::Model(AircraftId::F14));
        assert_eq!(draw(20), Draw::Hidden, "no model slot");
        assert_eq!(
            snapshot.models,
            [AircraftId::F18, AircraftId::Rafale, AircraftId::F14]
        );
        let debris: Vec<_> = snapshot
            .debris
            .iter()
            .map(|piece| (piece.owner, piece.draw, piece.variant))
            .collect();
        assert_eq!(
            debris,
            [
                (0, Draw::Ownship, player.damage_variant),
                (1, Draw::Model(AircraftId::F18), None),
                (2, Draw::Model(AircraftId::Rafale), Some(3)),
                (4, Draw::Model(AircraftId::F18), Some(0)),
                (20, Draw::Hidden, Some(3)),
                (99, Draw::Hidden, None),
            ]
        );
        let mut fixture = combat(Vec::new(), Vec::new());
        load(&mut fixture, &scene, true, &player);
        let snapshot = fixture.render_snapshot();
        assert!(snapshot.targets.iter().all(|t| t.draw == Draw::Ownship));
        assert!(snapshot.debris.iter().all(|p| p.draw == Draw::Ownship));
        assert_eq!(snapshot.player.id, 0);
    }

    #[test]
    fn devices_hold_once_nothing_simulates_the_aircraft() {
        let player = player();
        let mut combat = combat(types(), (0..7).map(|i| (i % 3, [0.; 3])).collect());
        combat.state.targets = scene(combat.state.own().configuration()).current;
        combat.restart_render(combat.own_id(), &player, None);
        let devices = |combat: &Combat| combat.render_snapshot().target(1).unwrap().devices;
        assert_eq!(devices(&combat), None);
        let last = [0.5; crate::snapshot::DEVICES];
        combat
            .render
            .current
            .targets
            .iter_mut()
            .find(|pose| pose.id == 1)
            .unwrap()
            .devices = Some(last);
        combat.advance_render(combat.own_id(), &player, None);
        assert_eq!(devices(&combat), Some(last));
        combat.restart_render(combat.own_id(), &player, None);
        assert_eq!(devices(&combat), None, "a restart forgets them");
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "combat_checkpoint.rs"]
mod checkpoint;

// The seats' shared network picture (optimization slice S11).
#[path = "combat_wire.rs"]
mod wire;
pub use wire::{WirePicture, WirePilot};

/// Marks the hidden shoulder-fired teams' listings
/// ([`live::GroundListing::team`]); every other listing is left as it is.
fn mark_teams(state: &mut live::State, teams: &std::collections::BTreeSet<u32>) {
    for id in teams {
        if let Some(listing) = state.ground_listing(*id) {
            state.set_ground_listing(
                *id,
                live::GroundListing {
                    team: true,
                    ..listing
                },
            );
        }
    }
}
