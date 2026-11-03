//! Manual-derived flight views, with fitted camera geometry. Never changes simulation.
use crate::{attitude::Basis, camera::Camera, combat::Combat, flight};
use tore_sim::attitude::{Vector, dot};

// Preserve the existing capture interface: 0 front, 1 external, 2 oblique, 3 back, 4 up.
pub const TRACK: u8 = 5;
pub const THREAT: u8 = 6;
pub const WING: u8 = 7;
pub const TARGET: u8 = 8;
pub const TARGET_PLAYER: u8 = 9;
pub const FLY_BY: u8 = 10;
pub const MISSILE: u8 = 11;

/// Opinionated, requested by John on 2026-10-22: game seconds at the last
/// weapon pose before F12 returns to Forward. Shared by live and replay.
pub const WEAPON_HOLD_SECONDS: f64 = 1.5;
/// John requested a stop about 100 metres before impact on 2026-10-22.
pub const WEAPON_STOP_FEET: f64 = 328.;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct WeaponStop {
    pub eye: Vector,
    pub impact: Vector,
}

/// Presentation-only prediction: a retained aim, otherwise the first terrain
/// crossing within 328 feet along the current flight direction. The local
/// tangent is fitted; it does not simulate or steer a second projectile.
pub(crate) fn weapon_stop(
    body: Body,
    aim: Option<Vector>,
    ground: impl Fn(f64, f64) -> f64,
) -> Option<WeaponStop> {
    let impact = if let Some(aim) = aim {
        (distance(body.position, aim) <= WEAPON_STOP_FEET).then_some(aim)?
    } else {
        let at = |distance: f64| -> Vector {
            std::array::from_fn(|i| body.position[i] + body.basis.forward[i] * distance)
        };
        let above = |p: Vector| p[1] - ground(p[0], p[2]);
        let mut hit = None;
        let mut lo = 0.;
        for n in 1..=32 {
            let mut hi = WEAPON_STOP_FEET * f64::from(n) / 32.;
            if above(at(hi)) <= 0. {
                for _ in 0..12 {
                    let mid = (lo + hi) * 0.5;
                    if above(at(mid)) > 0. {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                hit = Some(at(hi));
                break;
            }
            lo = hi;
        }
        hit?
    };
    let mut eye = relation(body, impact).position;
    eye[1] = eye[1].max(ground(eye[0], eye[2]) + 10.);
    Some(WeaponStop { eye, impact })
}

pub(crate) fn stopped_weapon_camera(stop: WeaponStop, look_at: Vector) -> Camera {
    facing(
        stop.eye,
        look_at,
        direction(stop.eye, stop.impact, [0., 0., 1.]),
    )
}
pub(crate) fn weapon_hold_ticks() -> u64 {
    (WEAPON_HOLD_SECONDS / flight::DT).round() as u64
}

#[derive(Clone, Copy)]
struct WeaponHold {
    id: u32,
    position: Vector,
    yaw: f32,
    pitch: f32,
    ended: Option<u64>,
    stop: Option<WeaponStop>,
}
impl WeaponHold {
    fn camera(self) -> Camera {
        let mut camera = Camera::new();
        camera.position = self.position;
        camera.yaw = self.yaw;
        camera.pitch = self.pitch;
        if let Some(stop) = self.stop {
            camera = stopped_weapon_camera(stop, stop.impact);
        }
        camera
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reference {
    #[default]
    Player,
    Target,
    Missile,
    /// Any aircraft by id, the player's own included. It never selects the cockpit:
    /// the front, up and track views sit at that aircraft and hide it through
    /// the camera's hidden target, the back view looks from its pilot's eye
    /// over its airframe, and the missile view follows that aircraft's
    /// newest missile.
    Aircraft(u32),
}

/// How far the subject may get from the fly-by point before a new point is
/// chosen: 3 nautical miles in straight-line feet. Opinionated: John asked
/// on 2026-09-28 for the fly-by to reset after "like 3-4 miles"; 3 nmi is an
/// agent choice.
pub const FLY_BY_RESET: f64 = 18_228.;

pub fn key(key: &str) -> Option<u8> {
    Some(match key {
        "F1" => 0,
        "F2" => 3,
        "F3" => 4,
        "F4" => TRACK,
        "F5" => THREAT,
        "F6" => WING,
        "F7" => TARGET,
        "F8" => TARGET_PLAYER,
        "F9" => FLY_BY,
        "F10" => 1,
        "F12" => MISSILE,
        _ => return None,
    })
}

/// One aircraft, ground object or missile a view can follow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Body {
    id: u32,
    position: Vector,
    velocity: Vector,
    basis: Basis,
    missile: bool,
}
impl Body {
    pub(crate) fn new(id: u32, position: Vector, velocity: Vector, basis: Basis) -> Self {
        Self {
            id,
            position,
            velocity,
            basis,
            missile: false,
        }
    }
    pub(crate) fn missile(id: u32, position: Vector, velocity: Vector, basis: Basis) -> Self {
        Self {
            missile: true,
            ..Self::new(id, position, velocity, basis)
        }
    }
    /// An aircraft or ground object as a picture draws it, its velocity
    /// times `sign` (a replay played backwards moves the other way).
    pub(crate) fn posed(pose: &crate::snapshot::AircraftPose, sign: f64) -> Self {
        let [yaw, pitch, bank] = pose.attitude;
        Self::new(
            pose.id,
            pose.position,
            pose.velocity.map(|v| v * sign),
            Basis::new(yaw, pitch, bank),
        )
    }
    /// A weapon in flight as a picture draws it, its velocity times `sign`.
    pub(crate) fn weapon(p: &crate::snapshot::ProjectilePose, sign: f64) -> Self {
        let d = p.direction;
        let speed = f64::from(p.speed_f8) / 256. * sign;
        let basis = Basis::new(d[0].atan2(d[2]), d[1].atan2(d[0].hypot(d[2])), 0.);
        Self::missile(p.id, p.position, d.map(|v| v * speed), basis)
    }
    pub(crate) fn position(&self) -> Vector {
        self.position
    }
    pub(crate) fn basis(&self) -> Basis {
        self.basis
    }
}
/// A missile in flight: its body, who fired it and at what.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shot {
    body: Body,
    owner: u32,
    target: Option<u32>,
    incoming: bool,
    aim: Option<Vector>,
}
impl Shot {
    pub(crate) fn new(body: Body, owner: u32, target: Option<u32>, incoming: bool) -> Self {
        Self {
            body,
            owner,
            target,
            incoming,
            aim: None,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Scene {
    tick: u64,
    stops: Vec<(u32, WeaponStop)>,
    player: Body,
    target: Option<u32>,
    bodies: Vec<Body>,
    wings: Vec<(u32, bool, u8, u8)>,
    missiles: Vec<Shot>,
}
impl Scene {
    /// A scene from its parts, so a replay can build one from recorded poses.
    /// `target` is the selected target's id; `wings` lists each flying wing
    /// aircraft as (id, friendly, wing number, member number).
    pub(crate) fn from_parts(
        player: Body,
        target: Option<u32>,
        bodies: Vec<Body>,
        wings: Vec<(u32, bool, u8, u8)>,
        missiles: Vec<Shot>,
    ) -> Self {
        Self {
            tick: 0,
            stops: Vec::new(),
            player,
            target,
            bodies,
            wings,
            missiles,
        }
    }
    pub(crate) fn at_tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }
    /// Resolve the local camera's impact prediction against actual terrain.
    pub(crate) fn for_view(self, view: u8, terrain: &crate::terrain::Terrain) -> Self {
        if view == MISSILE {
            self.on_terrain(terrain)
        } else {
            self
        }
    }
    pub(crate) fn on_terrain(mut self, terrain: &crate::terrain::Terrain) -> Self {
        self.stops = self
            .missiles
            .iter()
            .filter_map(|shot| {
                let aim = shot
                    .aim
                    .or_else(|| shot.target.and_then(|id| self.body(id)).map(|b| b.position));
                weapon_stop(shot.body, aim, |x, z| terrain.surface(x, z).height)
                    .map(|stop| (shot.body.id, stop))
            })
            .collect();
        self
    }
    /// The scene of a picture alone, which is all a networked client has: the
    /// aircraft, ground objects and missiles it draws, so another human's
    /// plane is as much a subject of the target and wing views as an AI one.
    /// `wing_of` names the wing and member of an aircraft in the roster, and
    /// whether it is friendly; `sign` turns every velocity round (a replay
    /// played backwards). A replay builds its scene the same way. A surface
    /// unit that follows a route is a subject where the picture draws it
    /// (protocol 22): a missile a SAM launched names the unit as its owner,
    /// and the object view of the unit is its subject's.
    pub(crate) fn of_picture(
        player: Body,
        target: Option<u32>,
        picture: &crate::snapshot::RenderSnapshot,
        wing_of: impl Fn(u32) -> Option<(bool, u8, u8)>,
        sign: f64,
    ) -> Self {
        let posed: Vec<Body> = picture
            .targets
            .iter()
            .filter(|t| t.airborne || t.damage.hp > 0)
            .map(|t| Body::posed(t, sign))
            .collect();
        let moving = picture
            .surface
            .iter()
            .filter(|unit| !unit.wrecked && !posed.iter().any(|b| b.id == unit.id.0))
            .map(|unit| {
                let [yaw, pitch, bank] = unit.attitude;
                Body::new(
                    unit.id.0,
                    unit.position,
                    [0.; 3],
                    Basis::new(yaw, pitch, bank),
                )
            })
            .collect::<Vec<_>>();
        Self::from_parts(
            player,
            target,
            posed.into_iter().chain(moving).collect(),
            picture
                .targets
                .iter()
                .filter(|pose| pose.airborne && pose.damage.hp > 0)
                .filter_map(|pose| {
                    let (friendly, wing, member) = wing_of(pose.id)?;
                    Some((pose.id, friendly, wing, member))
                })
                .collect(),
            picture
                .projectiles
                .iter()
                .filter(|p| !p.gun)
                .map(|p| Shot::new(Body::weapon(p, sign), p.owner, p.target, p.incoming))
                .collect(),
        )
        .at_tick(picture.tick)
    }
    /// The scene of the flight screen `frame` is for when it has no combat
    /// to read, a networked client's: its plane is the scene's player, as in
    /// [`Scene::new`], and everything else comes from the frame's picture.
    pub fn from_frame(
        frame: &crate::frame::FlightFrame,
        player: &flight::State,
        wings: Option<&crate::ai_wings::AiWings>,
    ) -> Self {
        Self::of_picture(
            Body::new(
                frame.plane.0,
                player.view_position(),
                player.velocity,
                Basis::new(player.yaw, player.pitch, player.bank),
            ),
            frame.readout.targets.view.as_ref().map(|t| t.id),
            frame.picture,
            |id| {
                let slot = wings?.slot(id)?;
                Some((
                    slot.side == tore_sim::ai::launch::Side::Friendly,
                    slot.wing_number,
                    slot.member_number,
                ))
            },
            1.,
        )
    }
    /// The scene of the flight screen `frame` is for, in the flight state
    /// `player`: the frame's plane is the scene's player and the frame's view
    /// target, which its readout names, is the selected target.
    pub fn new(
        frame: &crate::frame::FlightFrame,
        player: &flight::State,
        combat: &Combat,
        wings: Option<&crate::ai_wings::AiWings>,
        presented: Option<&crate::combat_view::CombatView>,
    ) -> Self {
        // The wing aircraft that fly: a wing slot whose target row is
        // airborne with hit points left. Ids are looked up in a sorted list,
        // not searched for in every row for every slot.
        let flying_ids = || {
            let mut ids: Vec<u32> = combat
                .state
                .targets
                .iter()
                .filter(|t| t.airborne && t.hp > 0)
                .map(|t| t.id)
                .collect();
            ids.sort_unstable();
            ids
        };
        Self::from_parts(
            Self::player_body(frame, player),
            frame.readout.targets.view.as_ref().map(|t| t.id),
            combat
                .state
                .targets
                .iter()
                .filter(|t| t.airborne || t.hp > 0)
                .map(|t| {
                    let (position, angles) = match presented {
                        Some(view) => view.pose(combat, t),
                        None => (t.position, t.basis.angles()),
                    };
                    Body::new(
                        t.id,
                        position,
                        t.velocity,
                        Basis::new(angles[0], angles[1], angles[2]),
                    )
                })
                .collect(),
            wings.map_or_else(Vec::new, |w| {
                let flying = flying_ids();
                w.slots()
                    .iter()
                    .filter(|slot| flying.binary_search(&slot.id).is_ok())
                    .map(|slot| {
                        (
                            slot.id,
                            slot.side == tore_sim::ai::launch::Side::Friendly,
                            slot.wing_number,
                            slot.member_number,
                        )
                    })
                    .collect()
            }),
            Self::missile_shots(combat),
        )
        .at_tick(frame.picture.tick)
    }
    /// The part of [`Scene::new`] a camera that follows the player's own
    /// aircraft reads: the player and the missiles in flight (the rig keeps
    /// track of the newest of its own). No aircraft bodies, wings or target.
    /// A rig answers [`Rig::needs_bodies`] to say when this is enough.
    pub fn lean(
        frame: &crate::frame::FlightFrame,
        player: &flight::State,
        combat: &Combat,
    ) -> Self {
        Self::from_parts(
            Self::player_body(frame, player),
            None,
            Vec::new(),
            Vec::new(),
            Self::missile_shots(combat),
        )
        .at_tick(frame.picture.tick)
    }
    fn player_body(frame: &crate::frame::FlightFrame, player: &flight::State) -> Body {
        Body::new(
            frame.plane.0,
            player.view_position(),
            player.velocity,
            Basis::new(player.yaw, player.pitch, player.bank),
        )
    }
    fn missile_shots(combat: &Combat) -> Vec<Shot> {
        combat
            .state
            .projectiles
            .iter()
            .filter(|p| !tore_sim::combat::live::is_gun(combat.state.weapon(p)))
            .map(|p| {
                let direction = unit(p.direction, [0., 0., 1.]);
                let mut shot = Shot::new(
                    Body::missile(
                        p.id,
                        p.position,
                        direction.map(|v| v * f64::from(p.speed_f8) / 256.),
                        Basis::new(
                            direction[0].atan2(direction[2]),
                            direction[1].atan2(direction[0].hypot(direction[2])),
                            0.,
                        ),
                    ),
                    p.owner,
                    p.target.filter(|_| p.threatening()),
                    p.incoming.is_some() && p.threatening(),
                );
                shot.aim = combat
                    .state
                    .guided_flight(p.id)
                    .filter(|_| p.threatening())
                    .and_then(|f| f.point);
                shot
            })
            .collect()
    }
    /// Whether two scenes hold the same subjects to within `tolerance` of a
    /// foot, a foot a second or a unit of direction.
    #[cfg(test)]
    pub(crate) fn same_as(&self, other: &Self, tolerance: f64) -> bool {
        let close = |a: Vector, b: Vector| (0..3).all(|i| (a[i] - b[i]).abs() <= tolerance);
        let same = |a: &Body, b: &Body| {
            a.id == b.id
                && a.missile == b.missile
                && close(a.position, b.position)
                && close(a.velocity, b.velocity)
                && close(a.basis.right, b.basis.right)
                && close(a.basis.up, b.basis.up)
                && close(a.basis.forward, b.basis.forward)
        };
        same(&self.player, &other.player)
            && self.target == other.target
            && self.wings == other.wings
            && self.bodies.len() == other.bodies.len()
            && self
                .bodies
                .iter()
                .zip(&other.bodies)
                .all(|(a, b)| same(a, b))
            && self.missiles.len() == other.missiles.len()
            && self.missiles.iter().zip(&other.missiles).all(|(a, b)| {
                same(&a.body, &b.body)
                    && (a.owner, a.target, a.incoming) == (b.owner, b.target, b.incoming)
            })
    }
    fn body(&self, id: u32) -> Option<Body> {
        if id == self.player.id {
            Some(self.player)
        } else {
            self.bodies.iter().find(|b| b.id == id).copied()
        }
    }
    fn target(&self) -> Result<Body, &'static str> {
        self.target
            .and_then(|id| self.body(id))
            .ok_or("No current target for this view")
    }
    /// The reference's wingman in wing/member order: `current` itself while it
    /// still flies, otherwise the next one; with `advance`, the one after it.
    /// The order wraps back to the first member.
    fn wing(
        &self,
        reference: Body,
        current: Option<WingKey>,
        advance: bool,
    ) -> Result<(WingKey, Body), &'static str> {
        let id = if reference.missile {
            self.missiles
                .iter()
                .find(|p| p.body.id == reference.id)
                .map_or(self.player.id, |p| p.owner)
        } else {
            reference.id
        };
        let group = if id == self.player.id {
            Some((true, 1))
        } else {
            self.wings.iter().find(|s| s.0 == id).map(|s| (s.1, s.2))
        };
        let mut members: Vec<WingKey> = self
            .wings
            .iter()
            .filter(|s| s.0 != id && Some((s.1, s.2)) == group && self.body(s.0).is_some())
            .map(|s| (s.2, s.3, s.0))
            .collect();
        members.sort_unstable();
        let key = current
            .and_then(|current| {
                members.iter().find(|k| {
                    if advance {
                        **k > current
                    } else {
                        **k >= current
                    }
                })
            })
            .or(members.first())
            .copied()
            .ok_or("No wingman for this view")?;
        Ok((key, self.body(key.2).expect("filtered to scene bodies")))
    }
}

/// A wingman's place in view order: wing number, member number, id.
type WingKey = (u8, u8, u32);

#[derive(Clone)]
struct Saved {
    view: u8,
    reference: Reference,
    look: [f32; 2],
    zoom: f32,
    fly_by: Option<Vector>,
}
#[derive(Clone, Default)]
pub struct Rig {
    pub reference: Reference,
    /// The aircraft the rig's player is: the plane the screen presents, or
    /// aircraft 0 in a replay.
    player: u32,
    /// Whose missiles the missile reference follows: the player in flight; the
    /// selected aircraft in a replay.
    missile_owner: u32,
    last_missile: Option<u32>,
    weapon_hold: Option<WeaponHold>,
    selected_weapon: Option<u32>,
    /// Presentation-only aim memory, retained when guidance loses its target.
    weapon_targets: std::collections::BTreeMap<u32, u32>,
    fly_by: Option<Vector>,
    /// The wingman F6 follows, and whether the next camera moves to the next.
    wingman: Option<WingKey>,
    advance_wingman: bool,
    other: Option<Saved>,
    /// Reject a pending GPU image after V changes the saved camera.
    pub other_pending: bool,
}
impl Rig {
    /// A rig for the screen of the human who flies `plane`.
    pub fn for_plane(plane: u32) -> Self {
        Self {
            player: plane,
            missile_owner: plane,
            ..Self::default()
        }
    }
    /// Whether `shot` counts as a launch of the missile reference's owner.
    /// The player's own shots exclude the incoming fixtures, which can share
    /// the player's shot counter; another aircraft's shots at the player are
    /// incoming and count.
    fn owns(&self, shot: &Shot) -> bool {
        shot.owner == self.missile_owner && (self.missile_owner != self.player || !shot.incoming)
    }
    /// The missile reference follows `owner`'s newest launch from now on,
    /// forgetting another owner's last missile.
    pub fn follow_missiles_of(&mut self, owner: u32) {
        if self.missile_owner != owner {
            self.missile_owner = owner;
            self.last_missile = None;
        }
    }
    pub fn observe(&mut self, scene: &Scene) {
        self.weapon_targets
            .retain(|id, _| scene.missiles.iter().any(|p| p.body.id == *id));
        for shot in &scene.missiles {
            if let Some(target) = shot.target {
                self.weapon_targets.insert(shot.body.id, target);
            }
        }
        if let Some(id) = scene
            .missiles
            .iter()
            .filter(|p| self.owns(p))
            .map(|p| p.body.id)
            .max()
        {
            self.last_missile = Some(self.last_missile.map_or(id, |old| old.max(id)));
        }
    }
    /// Keep the camera's last known target without restoring a simulation lock.
    pub fn prepare(
        &mut self,
        mut scene: Scene,
        view: u8,
        terrain: &crate::terrain::Terrain,
    ) -> Scene {
        self.observe(&scene);
        if view == MISSILE {
            let aims: Vec<_> = scene
                .missiles
                .iter()
                .map(|p| {
                    self.weapon_targets
                        .get(&p.body.id)
                        .and_then(|id| scene.body(*id))
                        .map(|b| b.position)
                })
                .collect();
            for (shot, aim) in scene.missiles.iter_mut().zip(aims) {
                shot.aim = shot.aim.or(aim);
            }
        }
        scene.for_view(view, terrain)
    }
    pub fn select(&mut self, reference: Reference) {
        self.reference = reference;
        self.weapon_hold = None;
        self.selected_weapon = None;
        self.fly_by = None;
        self.wingman = None;
        self.advance_wingman = false;
    }
    pub fn weapon_stopped(&self) -> bool {
        self.weapon_hold.is_some_and(|h| h.stop.is_some())
    }
    pub fn holding_weapon(&self) -> bool {
        self.weapon_hold.is_some_and(|h| h.ended.is_some())
    }
    /// An explicit F12 during a hold can choose an older surviving shot.
    pub fn next_live_weapon(&mut self, scene: &Scene, previous: Option<u32>) {
        let owner = match self.reference {
            Reference::Target => scene.target,
            Reference::Aircraft(id) => Some(id),
            _ => Some(self.missile_owner),
        };
        let mut ids: Vec<_> = scene
            .missiles
            .iter()
            .filter(|p| Some(p.owner) == owner && (p.owner != self.player || !p.incoming))
            .map(|p| p.body.id)
            .collect();
        ids.sort_unstable();
        self.selected_weapon = ids
            .iter()
            .find(|&&id| previous.is_none_or(|old| id > old))
            .or(ids.first())
            .copied();
    }
    /// F6 pressed again: the next camera follows the next wingman.
    pub fn next_wingman(&mut self) {
        self.advance_wingman = true;
    }
    /// The exact projectile held by the missile reference, including when it
    /// has disappeared. Replay captions must describe the same camera subject.
    pub fn missile_subject(&self) -> Option<u32> {
        self.weapon_hold
            .map(|h| h.id)
            .or(self.selected_weapon)
            .or(self.last_missile)
    }

    /// The aircraft F6 last followed.
    pub fn wingman(&self) -> Option<u32> {
        self.wingman.map(|key| key.2)
    }
    pub fn cockpit(&self, view: u8) -> bool {
        self.reference == Reference::Player && matches!(view, 0 | 3 | 4 | TRACK)
    }
    /// Whether the main view `view` or the saved Other View reads anything
    /// of the scene but the player and the missiles: every reference but the
    /// player's own does, and so does every view past the basic five
    /// (0 to 4), which relate the player to a target, a wingman or a missile.
    /// When it says no, [`Scene::lean`] gives the cameras what they read.
    pub fn needs_bodies(&self, view: u8) -> bool {
        let reads = |reference: Reference, view: u8| reference != Reference::Player || view > 4;
        reads(self.reference, view)
            || self
                .other
                .as_ref()
                .is_some_and(|s| reads(s.reference, s.view))
    }
    pub fn save(&mut self, view: u8, look: [f32; 2], zoom: f32) {
        self.other_pending = false;
        self.other = Some(Saved {
            view,
            reference: self.reference,
            look,
            zoom,
            fly_by: self.fly_by,
        });
    }
    /// Whether the player's airframe is drawn: every view outside the
    /// cockpit, and the back view, which looks over the spine and tail.
    pub fn shows_player(&self, view: u8) -> bool {
        !self.cockpit(view) || view == 3
    }
    pub fn other_shows_player(&self) -> bool {
        // The default Other View is Back, which shows the spine and tail.
        self.other
            .as_ref()
            .is_none_or(|s| s.reference != Reference::Player || !matches!(s.view, 0 | 4 | TRACK))
    }
    pub fn other_view(&self) -> u8 {
        self.other.as_ref().map_or(3, |s| s.view)
    }
    /// The saved Other View's camera. A saved fly-by keeps its own point,
    /// which moves on by the same rule as the main view's, independently.
    pub fn other_camera(&mut self, scene: &Scene, base: Camera) -> Result<Camera, &'static str> {
        let saved = self.other.clone().unwrap_or(Saved {
            view: 3,
            reference: Reference::Player,
            look: [0.; 2],
            zoom: 1.,
            fly_by: None,
        });
        let mut rig = self.clone();
        rig.reference = saved.reference;
        rig.fly_by = saved.fly_by;
        let mut camera = rig.camera(saved.view, scene, base, saved.look, saved.zoom)?;
        if let Some(other) = &mut self.other {
            other.fly_by = rig.fly_by;
        }
        camera.weather_slot = 3;
        Ok(camera)
    }
    fn missile<'a>(&self, scene: &'a Scene) -> Result<&'a Shot, &'static str> {
        scene
            .missiles
            .iter()
            .find(|p| self.owns(p) && Some(p.body.id) == self.selected_weapon.or(self.last_missile))
            .ok_or("No last-launched missile for this view")
    }
    pub fn camera(
        &mut self,
        view: u8,
        scene: &Scene,
        mut camera: Camera,
        look: [f32; 2],
        zoom: f32,
    ) -> Result<Camera, &'static str> {
        if view != MISSILE {
            self.weapon_hold = None;
        } else if let Some(hold) = &mut self.weapon_hold
            && !scene
                .missiles
                .iter()
                .any(|p| p.body.id == hold.id && (p.owner != self.player || !p.incoming))
        {
            let ended = *hold.ended.get_or_insert(scene.tick);
            if scene.tick.saturating_sub(ended) < weapon_hold_ticks() {
                let mut held = hold.camera();
                held.keys = std::mem::take(&mut camera.keys);
                held.zoom = zoom;
                return Ok(held);
            }
            return Err("Followed weapon has ended");
        }
        self.observe(scene);
        let keys = std::mem::take(&mut camera.keys);
        let subject = match self.reference {
            Reference::Player => scene.player,
            Reference::Target => scene.target()?,
            Reference::Missile => self.missile(scene)?.body,
            Reference::Aircraft(id) => scene.body(id).ok_or("That aircraft is not in the scene")?,
        };
        if matches!(view, 0..=4) {
            if self.reference != Reference::Player {
                camera = body_camera(subject, view);
            }
            crate::look::apply(&mut camera, subject.position, look, matches!(view, 1 | 2));
        } else {
            match view {
                TRACK => {
                    let target = scene.target()?;
                    let delta = direction(subject.position, target.position, subject.basis.forward);
                    let yaw =
                        dot(delta, subject.basis.right).atan2(dot(delta, subject.basis.forward));
                    let pitch = dot(delta, subject.basis.up)
                        .clamp(-1., 1.)
                        .asin()
                        .clamp(0., std::f64::consts::FRAC_PI_2);
                    camera = body_camera(subject, 0);
                    crate::look::apply(
                        &mut camera,
                        subject.position,
                        [yaw as f32, pitch as f32],
                        false,
                    );
                }
                THREAT => {
                    let threat = scene
                        .missiles
                        .iter()
                        .filter(|p| {
                            !subject.missile
                                && (p.target == Some(subject.id)
                                    || (subject.id == scene.player.id && p.incoming))
                        })
                        .min_by(|a, b| {
                            distance(a.body.position, subject.position)
                                .total_cmp(&distance(b.body.position, subject.position))
                        })
                        .ok_or("No inbound missile for this view")?;
                    camera = relation(subject, threat.body.position);
                }
                WING => {
                    let advance = std::mem::take(&mut self.advance_wingman);
                    let (key, wingman) = scene.wing(subject, self.wingman, advance)?;
                    self.wingman = Some(key);
                    camera = relation(subject, wingman.position);
                }
                TARGET => camera = relation(subject, scene.target()?.position),
                TARGET_PLAYER => camera = relation(scene.target()?, subject.position),
                FLY_BY => {
                    let eye = match self.fly_by {
                        Some(eye) if distance(eye, subject.position) <= FLY_BY_RESET => eye,
                        _ => *self.fly_by.insert(fly_by_point(subject)),
                    };
                    camera = facing(eye, subject.position, subject.basis.forward);
                }
                MISSILE => {
                    let missile = if let Some(id) = self.selected_weapon {
                        scene
                            .missiles
                            .iter()
                            .find(|p| p.body.id == id)
                            .ok_or("No live weapon for this view")?
                    } else if matches!(self.reference, Reference::Target | Reference::Aircraft(_)) {
                        scene
                            .missiles
                            .iter()
                            .filter(|p| p.owner == subject.id)
                            .max_by_key(|p| p.body.id)
                            .ok_or(if self.reference == Reference::Target {
                                "Target has no live missile"
                            } else {
                                "That aircraft has no live missile"
                            })?
                    } else {
                        self.missile(scene)?
                    };
                    let target = missile.target.and_then(|id| scene.body(id)).map_or_else(
                        || {
                            std::array::from_fn(|i| {
                                missile.body.position[i] + missile.body.basis.forward[i] * 1000.
                            })
                        },
                        |b| b.position,
                    );
                    let mut stop = self
                        .weapon_hold
                        .filter(|h| h.id == missile.body.id)
                        .and_then(|h| h.stop)
                        .or_else(|| {
                            scene
                                .stops
                                .iter()
                                .find(|(id, _)| *id == missile.body.id)
                                .map(|(_, stop)| *stop)
                        });
                    if let Some(stop) = &mut stop {
                        stop.impact = missile.body.position;
                    }
                    camera = stop.map_or_else(
                        || relation(missile.body, target),
                        |stop| stopped_weapon_camera(stop, missile.body.position),
                    );
                    self.weapon_hold = Some(WeaponHold {
                        id: missile.body.id,
                        position: camera.position,
                        yaw: camera.yaw,
                        pitch: camera.pitch,
                        ended: None,
                        stop,
                    });
                }
                _ => return Err("Unknown flight view"),
            }
        }
        // Back from another aircraft looks from its pilot's eye and shows its
        // airframe, as the player's own Back does; a missile stays hidden.
        let hides = matches!(view, 0 | 4 | TRACK) || (view == 3 && subject.missile);
        if hides && self.reference != Reference::Player {
            if subject.missile {
                camera.hidden_projectile = Some(subject.id);
            } else {
                camera.hidden_target = Some(subject.id);
            }
        }
        camera.keys = keys;
        camera.zoom = zoom;
        Ok(camera)
    }
}
/// Where a fly-by watches `subject` from: three seconds of its velocity
/// ahead, 300 feet to its right and 100 feet above.
fn fly_by_point(subject: Body) -> Vector {
    std::array::from_fn(|i| {
        subject.position[i]
            + subject.velocity[i] * 3.
            + subject.basis.right[i] * 300.
            + if i == 1 { 100. } else { 0. }
    })
}
/// The external view of `body`, as F10 shows it: the replay's fallback.
pub(crate) fn outside(body: Body) -> Camera {
    body_camera(body, 1)
}
fn body_camera(body: Body, view: u8) -> Camera {
    let mut camera = Camera::new();
    camera.position = body.position;
    let [yaw, pitch, bank] = body.basis.angles();
    camera.yaw = yaw as f32;
    camera.pitch = pitch as f32;
    camera.roll = -bank as f32;
    match view {
        3 | 4 => {
            turn(&mut camera, body.basis, view);
            if view == 3 && !body.missile {
                camera.position = crate::mirrors::pilot_eye(body.position, body.basis);
            }
        }
        1 | 2 => {
            let angle = yaw + if view == 2 { 0.8 } else { 0. };
            let distance = if view == 2 { 130. } else { 180. };
            camera.position[0] -= angle.sin() * distance;
            camera.position[2] -= angle.cos() * distance;
            camera.position[1] += 60.;
            camera.yaw = angle as f32;
            camera.pitch = -0.3;
            camera.roll = 0.;
        }
        _ => {}
    }
    camera
}
/// Turns a cockpit camera for the back (3) or up (4) view about the
/// aircraft's own axes, so pitch and bank read correctly from any attitude
/// and a loop or barrel roll never flips the view.
pub(crate) fn turn(camera: &mut Camera, body: Basis, view: u8) {
    let turned = match view {
        3 => crate::mirrors::rear_basis(body),
        4 => body.rotated(body.right.map(|v| -v * UP_VIEW)),
        _ => return,
    };
    let [yaw, pitch, bank] = turned.angles();
    camera.yaw = yaw as f32;
    camera.pitch = pitch as f32;
    camera.roll = -bank as f32;
}
/// Up view elevation above the nose, in radians (about 46 degrees).
const UP_VIEW: f64 = 0.8;
fn distance(a: Vector, b: Vector) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}
fn unit(v: Vector, fallback: Vector) -> Vector {
    let length = v[0].hypot(v[1]).hypot(v[2]);
    if length < 1e-6 {
        fallback
    } else {
        v.map(|n| n / length)
    }
}
fn direction(from: Vector, to: Vector, fallback: Vector) -> Vector {
    unit(std::array::from_fn(|i| to[i] - from[i]), fallback)
}
fn facing(eye: Vector, target: Vector, fallback: Vector) -> Camera {
    let d = direction(eye, target, fallback);
    let mut c = Camera::new();
    c.position = eye;
    c.yaw = d[0].atan2(d[2]) as f32;
    c.pitch = d[1].atan2(d[0].hypot(d[2])) as f32;
    c
}
/// The replay's object view: the relation camera from `from` toward `to`,
/// at any range, with its eye kept at least 20 feet above `ground` (height
/// under an east and north position) so a view from a ground object up at
/// an aircraft never looks from under the terrain. The clearance is fitted.
pub(crate) fn object_camera(from: Body, to: Vector, ground: impl Fn(f64, f64) -> f64) -> Camera {
    let mut camera = relation(from, to);
    let floor = ground(camera.position[0], camera.position[2]) + 20.;
    if camera.position[1] < floor {
        camera.position[1] = floor;
        let d = direction(camera.position, to, from.basis.forward);
        camera = facing(camera.position, to, d);
    }
    camera
}
pub(crate) fn relation(subject: Body, target: Vector) -> Camera {
    let d = direction(subject.position, target, subject.basis.forward);
    let (back, up) = if subject.missile {
        (30., 10.)
    } else {
        (180., 60.)
    };
    let eye =
        std::array::from_fn(|i| subject.position[i] - d[i] * back + if i == 1 { up } else { 0. });
    // Aim through the subject so both endpoints share the center sightline.
    facing(eye, target, d)
}

/// The weapon chase camera. Zero offsets select F12's fitted placement;
/// otherwise X is sideways, Y world-up and Z forward along the target line.
pub(crate) fn weapon_camera(subject: Body, target: Vector, offset: Vector) -> Camera {
    if offset == [0.; 3] {
        return relation(subject, target);
    }
    let d = direction(subject.position, target, subject.basis.forward);
    let right = unit([d[2], 0., -d[0]], subject.basis.right);
    let eye = std::array::from_fn(|i| {
        subject.position[i]
            + right[i] * offset[0]
            + d[i] * offset[2]
            + if i == 1 { offset[1] } else { 0. }
    });
    facing(eye, target, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn body(id: u32, position: Vector) -> Body {
        Body {
            id,
            position,
            velocity: [0., 0., 200.],
            basis: Basis::new(0., 0., 0.),
            missile: false,
        }
    }
    fn scene() -> Scene {
        Scene {
            tick: 0,
            stops: Vec::new(),
            player: body(0, [0., 1000., 0.]),
            target: Some(1),
            bodies: vec![body(1, [1000., 1000., 0.]), body(2, [0., 1000., 1000.])],
            wings: vec![(1, false, 1, 1), (2, true, 1, 1)],
            missiles: vec![],
        }
    }
    fn camera(rig: &mut Rig, mode: u8, scene: &Scene) -> Camera {
        rig.camera(mode, scene, body_camera(scene.player, mode), [0.; 2], 1.)
            .unwrap()
    }
    fn shot(id: u32, owner: u32, target: Option<u32>, position: Vector) -> Shot {
        Shot {
            body: Body {
                missile: true,
                ..body(id, position)
            },
            owner,
            target,
            incoming: owner != 0,
            aim: None,
        }
    }
    fn near(a: Vector, b: Vector) {
        assert!(distance(a, b) < 0.001, "{a:?} != {b:?}");
    }
    /// The lean scene of `scene`: its player and missiles alone.
    fn lean_of(scene: &Scene) -> Scene {
        Scene::from_parts(
            scene.player,
            None,
            Vec::new(),
            Vec::new(),
            scene.missiles.clone(),
        )
    }
    /// Every field of a camera, as text.
    fn shown(camera: &Result<Camera, &'static str>) -> String {
        match camera {
            Ok(c) => format!(
                "{:?} {:?} {:?} {} {} {} {} {} {} {} {:?} {} {}",
                c.weather_slot,
                c.hidden_target,
                c.hidden_projectile,
                c.yaw,
                c.pitch,
                c.roll,
                c.view_fraction,
                c.zoom,
                c.near_clip,
                c.thermal,
                c.keys,
                c.sensor,
                c.position
                    .map(f64::to_bits)
                    .map(|b| b.to_string())
                    .join(","),
            ),
            Err(reason) => (*reason).to_owned(),
        }
    }
    #[test]
    fn a_lean_scene_serves_every_camera_that_follows_the_players_own_aircraft() {
        let mut full = scene();
        full.player.velocity = [30., -4., 250.];
        full.player.basis = Basis::new(0.7, 0.2, -0.4);
        full.missiles = vec![
            shot(40, 0, Some(1), [100., 1000., 100.]),
            shot(44, 9, Some(0), [-300., 900., 500.]),
            shot(41, 0, None, [100., 1200., 100.]),
        ];
        let lean = lean_of(&full);
        let saved = [None, Some(3), Some(4), Some(0)];
        for other in saved {
            for view in 0..=4u8 {
                for look in [[0.; 2], [0.4, -0.2], [-1.2, 0.9]] {
                    let (mut with_all, mut with_lean) = (Rig::for_plane(0), Rig::for_plane(0));
                    if let Some(saved_view) = other {
                        for rig in [&mut with_all, &mut with_lean] {
                            rig.save(saved_view, [0.2, 0.1], 1.5);
                        }
                    }
                    assert!(!with_lean.needs_bodies(view));
                    let base = |scene: &Scene| body_camera(scene.player, view);
                    let (a, b) = (
                        with_all.camera(view, &full, base(&full), look, 1.25),
                        with_lean.camera(view, &lean, base(&lean), look, 1.25),
                    );
                    assert_eq!(shown(&a), shown(&b), "view {view}");
                    let (a, b) = (
                        with_all.other_camera(&full, Camera::new()),
                        with_lean.other_camera(&lean, Camera::new()),
                    );
                    assert_eq!(shown(&a), shown(&b), "other {other:?}");
                    with_all.observe(&full);
                    with_lean.observe(&lean);
                    assert_eq!(with_all.missile_subject(), with_lean.missile_subject());
                    assert_eq!(with_all.missile_subject(), Some(41));
                }
            }
        }
    }
    #[test]
    fn a_view_of_anything_but_the_player_needs_the_other_bodies() {
        let mut rig = Rig::for_plane(0);
        assert!(!rig.needs_bodies(0) && !rig.needs_bodies(4));
        for view in [TRACK, THREAT, WING, TARGET, TARGET_PLAYER, FLY_BY, MISSILE] {
            assert!(rig.needs_bodies(view), "view {view}");
        }
        for reference in [
            Reference::Target,
            Reference::Missile,
            Reference::Aircraft(3),
        ] {
            rig.select(reference);
            assert!(rig.needs_bodies(0), "{reference:?}");
        }
        rig.select(Reference::Player);
        rig.save(WING, [0.; 2], 1.);
        assert!(rig.needs_bodies(0), "a saved Other View of a wingman");
    }
    #[test]
    fn relations_reverse_endpoints_and_show_the_reference_subject() {
        let scene = scene();
        let mut rig = Rig::default();
        let c = camera(&mut rig, TARGET, &scene);
        near(c.position, [-180., 1060., 0.]);
        assert!((c.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let c = camera(&mut rig, TARGET_PLAYER, &scene);
        near(c.position, [1180., 1060., 0.]);
        assert!((c.yaw + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        near(camera(&mut rig, WING, &scene).position, [0., 1060., -180.]);
        assert!(!rig.cockpit(TARGET));
    }
    #[test]
    fn tracking_respects_eye_line_and_banked_aircraft_axes() {
        let mut scene = scene();
        let mut rig = Rig::default();
        let c = camera(&mut rig, TRACK, &scene);
        assert!((c.yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(rig.cockpit(TRACK));
        scene.bodies[0].position = [0., 0., 1000.];
        assert_eq!(camera(&mut rig, TRACK, &scene).pitch, 0.);
        scene.player.basis = Basis::new(0., 0., std::f64::consts::FRAC_PI_2);
        let up = scene.player.basis.up;
        scene.bodies[0].position =
            std::array::from_fn(|i| scene.player.position[i] + up[i] * 1000.);
        let c = camera(&mut rig, TRACK, &scene);
        let forward = Basis::new(c.yaw.into(), c.pitch.into(), -f64::from(c.roll)).forward;
        near(forward, up);
    }
    #[test]
    fn fly_by_stays_fixed_until_reselected_and_saved_copy_stays_independent() {
        let mut scene = scene();
        let mut rig = Rig::default();
        let first = camera(&mut rig, FLY_BY, &scene);
        near(first.position, [300., 1100., 600.]);
        rig.other_pending = true;
        rig.save(FLY_BY, [0.; 2], 2.);
        assert!(!rig.other_pending);
        scene.player.position[2] = 500.;
        let later = camera(&mut rig, FLY_BY, &scene);
        assert_eq!(first.position, later.position);
        assert_ne!(first.yaw, later.yaw);
        rig.select(Reference::Player);
        let next = camera(&mut rig, FLY_BY, &scene);
        assert_eq!(next.position[2], 1100.);
        let stored = rig.other_camera(&scene, Camera::new()).unwrap();
        assert_eq!(stored.position, first.position);
        assert_eq!(stored.zoom, 2.);
        assert_eq!(stored.weather_slot, 3);
    }
    #[test]
    fn fly_by_moves_on_once_the_subject_is_three_miles_from_its_point() {
        let mut scene = scene();
        let mut rig = Rig::default();
        let first = camera(&mut rig, FLY_BY, &scene).position;
        near(first, [300., 1100., 600.]);
        // Straight along its path: 18,227 feet from the point, it stays.
        let place = |scene: &mut Scene, feet: f64| {
            let d = FLY_BY_RESET + feet;
            // Further up its path, `d` feet in a straight line from the point.
            let dz = (d * d - 300f64.powi(2) - 100f64.powi(2)).sqrt();
            scene.player.position = [0., 1000., first[2] + dz];
        };
        place(&mut scene, -1.);
        assert_eq!(camera(&mut rig, FLY_BY, &scene).position, first);
        // Just over: a new point by the same rule from where it is now.
        place(&mut scene, 1.);
        let moved = camera(&mut rig, FLY_BY, &scene).position;
        let p = scene.player.position;
        near(moved, [p[0] + 300., p[1] + 100., p[2] + 600.]);
        // It then keeps the new point.
        scene.player.position[2] += 500.;
        assert_eq!(camera(&mut rig, FLY_BY, &scene).position, moved);
        // Paused, nothing moves, so neither does the point.
        assert_eq!(camera(&mut rig, FLY_BY, &scene).position, moved);
    }
    #[test]
    fn saved_fly_by_moves_on_by_itself() {
        let mut scene = scene();
        let mut rig = Rig::default();
        let first = camera(&mut rig, FLY_BY, &scene).position;
        rig.save(FLY_BY, [0.; 2], 1.);
        assert_eq!(
            rig.other_camera(&scene, Camera::new()).unwrap().position,
            first
        );
        // Far away, the saved copy picks its own new point and keeps it.
        scene.player.position = [0., 1000., 30_000.];
        let saved = rig.other_camera(&scene, Camera::new()).unwrap().position;
        near(saved, [300., 1100., 30_600.]);
        scene.player.position[2] += 400.;
        assert_eq!(
            rig.other_camera(&scene, Camera::new()).unwrap().position,
            saved
        );
        // The main view has not moved on yet: it resets on its own next frame,
        // from where the subject is then, and the saved copy is untouched.
        assert_eq!(rig.fly_by, Some(first));
        let main = camera(&mut rig, FLY_BY, &scene).position;
        near(main, [300., 1100., 31_000.]);
        assert_eq!(
            rig.other_camera(&scene, Camera::new()).unwrap().position,
            saved
        );
    }
    #[test]
    fn back_from_another_aircraft_sits_at_its_pilot_eye_and_shows_it() {
        let mut scene = scene();
        scene.missiles = vec![shot(10, 1, Some(0), [500., 1000., 0.])];
        let mut rig = Rig::default();
        rig.select(Reference::Target);
        let c = camera(&mut rig, 3, &scene);
        near(c.position, [1000., 1007., 10.]);
        assert_eq!(c.hidden_target, None);
        assert!((c.yaw.abs() - std::f32::consts::PI).abs() < 1e-6);
        // Front, up and track still hide it.
        for view in [0, 4, TRACK] {
            let mut rig = Rig::default();
            rig.select(Reference::Aircraft(1));
            let c = rig
                .camera(view, &scene, Camera::new(), [0.; 2], 1.)
                .unwrap_or_else(|_| panic!("{view}"));
            assert_eq!(c.hidden_target, Some(1), "{view}");
        }
        // A missile's back view stays at the missile and hides it.
        rig.follow_missiles_of(1);
        rig.select(Reference::Missile);
        let c = camera(&mut rig, 3, &scene);
        assert_eq!(c.position, [500., 1000., 0.]);
        assert_eq!(c.hidden_projectile, Some(10));
    }
    #[test]
    fn missile_reference_follows_the_chosen_owner_newest_shot() {
        let mut scene = scene();
        scene.missiles = vec![
            shot(10, 1, Some(0), [0., 1000., 50.]),
            shot(11, 1, Some(0), [0., 1000., 100.]),
            shot(12, 2, Some(0), [0., 1000., 150.]),
            shot(13, 0, Some(1), [0., 1000., 200.]),
        ];
        let mut rig = Rig::default();
        // The player's by default, as in flight.
        rig.select(Reference::Missile);
        assert_eq!(camera(&mut rig, 0, &scene).position, [0., 1000., 200.]);
        // Another aircraft's newest, incoming at the player or not.
        rig.follow_missiles_of(1);
        rig.select(Reference::Missile);
        assert_eq!(camera(&mut rig, 0, &scene).position, [0., 1000., 100.]);
        assert_eq!(camera(&mut rig, 0, &scene).hidden_projectile, Some(11));
        // Once it is gone, no older shot stands in.
        scene.missiles.retain(|p| p.body.id != 11);
        assert!(rig.camera(0, &scene, Camera::new(), [0.; 2], 1.).is_err());
        rig.follow_missiles_of(2);
        assert_eq!(camera(&mut rig, 0, &scene).position, [0., 1000., 150.]);
    }
    #[test]
    fn object_camera_faces_the_far_end_at_any_range_and_stays_above_ground() {
        let level = Basis::new(0., 0., 0.);
        let from = Body::new(4, [0., 5_000., 0.], [0.; 3], level);
        // 50 nmi east and a mile up.
        let to = [50. * 6_076., 10_280., 0.];
        let c = object_camera(from, to, |_, _| 0.);
        let forward = Basis::new(c.yaw.into(), c.pitch.into(), 0.).forward;
        close(forward, direction(c.position, to, [0.; 3]));
        close(forward, direction(from.position, to, [0.; 3]));
        // From a ground object up at an aircraft, the eye stays 20 feet above
        // the terrain and still faces the aircraft.
        let ground = Body::new(5, [0., 100., 0.], [0.; 3], level);
        let above = [0., 20_000., 1_000.];
        let c = object_camera(ground, above, |_, _| 100.);
        assert!((c.position[1] - 120.).abs() < 1e-9, "{:?}", c.position);
        let forward = Basis::new(c.yaw.into(), c.pitch.into(), 0.).forward;
        close(forward, direction(c.position, above, [0.; 3]));
        // A missile end sits 30 feet back and 10 up.
        let missile = Body::missile(9, [0., 5_000., 0.], [0.; 3], level);
        let level_east = [50. * 6_076., 5_000., 0.];
        near(
            object_camera(missile, level_east, |_, _| 0.).position,
            [-30., 5_010., 0.],
        );
    }
    #[test]
    fn threat_chooses_nearest_inbound_not_nearest_outgoing_round() {
        let mut scene = scene();
        let mut rig = Rig::default();
        scene.missiles = vec![
            shot(10, 5, Some(0), [100., 1000., 0.]),
            shot(11, 6, Some(0), [0., 1000., 50.]),
            shot(12, 0, Some(1), [0., 1000., 1.]),
        ];
        let c = camera(&mut rig, THREAT, &scene);
        assert_eq!(c.yaw, 0.);
        near(c.position, [0., 1060., -180.]);
    }
    #[test]
    fn approach_stops_at_328_feet_and_keeps_the_eye_above_terrain() {
        let mut subject = shot(10, 0, Some(1), [0., 100., 0.]).body;
        assert!(weapon_stop(subject, Some([0., 100., 328.01]), |_, _| 0.).is_none());
        let stop = weapon_stop(subject, Some([0., 100., 328.]), |_, _| 0.).unwrap();
        assert_eq!(stop.eye, [0., 110., -30.]);
        let raised = weapon_stop(subject, Some([0., 100., 328.]), |_, z| {
            if z < 0. { 150. } else { 0. }
        })
        .unwrap();
        assert_eq!(raised.eye[1], 160.);
        subject.basis = Basis::new(0., -std::f64::consts::FRAC_PI_2, 0.);
        subject.position[1] = 328.;
        let ground = weapon_stop(subject, None, |_, _| 0.).unwrap();
        near(ground.impact, [0.; 3]);
        subject.position[1] = 328.01;
        assert!(weapon_stop(subject, None, |_, _| 0.).is_none());
        subject.basis = Basis::new(0., 0., 0.);
        assert!(weapon_stop(subject, None, |_, _| 0.).is_none());
    }

    #[test]
    fn stopped_eye_tracks_weapon_then_holds_last_impact_and_can_be_cancelled() {
        let mut scene = scene();
        scene.missiles = vec![shot(10, 0, Some(2), [0., 1000., 672.])];
        let stop =
            weapon_stop(scene.missiles[0].body, Some([0., 1000., 1000.]), |_, _| 0.).unwrap();
        scene.stops = vec![(10, stop)];
        let mut rig = Rig::default();
        let first = camera(&mut rig, MISSILE, &scene);
        assert!(rig.weapon_stopped());
        scene.stops.clear();
        scene.missiles[0].body.position = [50., 1000., 990.];
        let near = camera(&mut rig, MISSILE, &scene);
        assert_eq!(near.position, first.position);
        assert_ne!(near.yaw, first.yaw);
        // A miss does not restart the chase. The camera continues turning.
        scene.missiles[0].body.position = [200., 1000., 1500.];
        let missed = camera(&mut rig, MISSILE, &scene);
        assert_eq!(missed.position, first.position);
        scene.missiles.clear();
        scene.tick = 120;
        let held = camera(&mut rig, MISSILE, &scene);
        assert_eq!(
            (held.position, held.yaw, held.pitch),
            (missed.position, missed.yaw, missed.pitch)
        );
        scene.tick = 299;
        assert_eq!(camera(&mut rig, MISSILE, &scene).position, first.position);
        scene.tick = 300;
        assert!(
            rig.camera(MISSILE, &scene, Camera::new(), [0.; 2], 1.)
                .is_err()
        );
        rig.select(Reference::Player);
        assert!(!rig.weapon_stopped());
    }

    #[test]
    fn weapon_hold_uses_game_ticks_and_freezes_pose_while_world_moves() {
        for reference in [Reference::Player, Reference::Missile, Reference::Target] {
            let mut scene = scene();
            let mut rig = Rig::default();
            rig.select(reference);
            let owner = if reference == Reference::Target { 1 } else { 0 };
            scene.missiles = vec![shot(10, owner, Some(2), [0., 1000., 100.])];
            scene.tick = 99;
            let last = camera(&mut rig, MISSILE, &scene);
            scene.missiles.clear();
            scene.tick = 100;
            for tick in [100, 100, 101, 160, 279] {
                scene.tick = tick;
                scene.player.position[0] += 500.;
                scene.bodies.clear(); // Target destruction cannot cancel the hold.
                let held = camera(&mut rig, MISSILE, &scene);
                assert_eq!(held.position, last.position);
                assert_eq!((held.yaw, held.pitch), (last.yaw, last.pitch));
            }
            scene.tick = 280;
            assert!(
                rig.camera(MISSILE, &scene, Camera::new(), [0.; 2], 1.)
                    .is_err()
            );
            rig.select(Reference::Player);
            assert!(!rig.holding_weapon());
            assert_ne!(camera(&mut rig, 0, &scene).position, last.position);
        }
    }

    #[test]
    fn weapon_hold_does_not_jump_to_new_launch_and_f12_can_choose_survivor() {
        let mut scene = scene();
        let mut rig = Rig::for_plane(42); // A multiplayer seat has its own camera.
        scene.player.id = 42;
        scene.missiles = vec![shot(10, 42, None, [0., 1000., 100.])];
        scene.missiles[0].incoming = false;
        let last = camera(&mut rig, MISSILE, &scene);
        scene.missiles = vec![shot(11, 42, None, [1000., 1000., 100.])];
        scene.missiles[0].incoming = false;
        scene.tick = 1;
        assert_eq!(camera(&mut rig, MISSILE, &scene).position, last.position);
        rig.select(Reference::Player);
        rig.next_live_weapon(&scene, Some(10));
        assert_ne!(camera(&mut rig, MISSILE, &scene).position, last.position);
        scene.missiles.clear();
        camera(&mut rig, MISSILE, &scene);
        rig.select(Reference::Player); // F10, death or restart cancels immediately.
        assert!(!rig.holding_weapon());
        assert!(rig.camera(1, &scene, Camera::new(), [0.; 2], 1.).is_ok());
    }

    #[test]
    fn last_missile_keeps_its_target_and_does_not_revert_to_older_shots() {
        let mut scene = scene();
        let mut rig = Rig::default();
        scene.missiles = vec![
            shot(10, 0, Some(1), [0., 1000., 50.]),
            shot(11, 0, Some(2), [0., 1000., 100.]),
        ];
        let c = camera(&mut rig, MISSILE, &scene);
        near(c.position, [0., 1010., 70.]);
        scene.target = Some(1);
        assert_eq!(camera(&mut rig, MISSILE, &scene).yaw, 0.);
        scene.missiles.pop();
        // A diagnostic incoming round can share the player shot counter.
        scene.missiles.push(shot(11, 0, Some(0), [0., 1000., 200.]));
        scene.missiles.last_mut().unwrap().incoming = true;
        assert_eq!(camera(&mut rig, MISSILE, &scene).position, c.position);
        scene.tick = weapon_hold_ticks();
        assert!(
            rig.camera(MISSILE, &scene, Camera::new(), [0.; 2], 1.)
                .is_err()
        );
        assert_eq!(rig.last_missile, Some(11));
    }
    #[test]
    fn relative_cameras_hide_only_the_reference_body_and_unmodified_restores_cockpit() {
        let scene = scene();
        let mut rig = Rig::default();
        rig.select(Reference::Target);
        let c = camera(&mut rig, 0, &scene);
        assert_eq!(c.hidden_target, Some(1));
        assert_eq!(c.position, [1000., 1000., 0.]);
        assert!(!rig.cockpit(0));
        rig.select(Reference::Player);
        assert!(rig.cockpit(0));
        assert_eq!(camera(&mut rig, 0, &scene).hidden_target, None);
    }
    #[test]
    fn absent_and_coincident_subjects_are_bounded_and_saved_view_is_live() {
        let mut scene = scene();
        let mut rig = Rig::default();
        scene.bodies[0].position = scene.player.position;
        let c = camera(&mut rig, TARGET, &scene);
        assert!(
            c.position.iter().all(|v| v.is_finite())
                && [c.yaw, c.pitch].iter().all(|v| v.is_finite())
        );
        rig.save(TARGET, [0.; 2], 1.5);
        scene.bodies[0].position = [1000., 1000., 0.];
        assert!(
            (rig.other_camera(&scene, Camera::new()).unwrap().yaw - std::f32::consts::FRAC_PI_2)
                .abs()
                < 1e-6
        );
        scene.target = None;
        assert!(rig.other_camera(&scene, Camera::new()).is_err());
        for mode in [TRACK, THREAT, TARGET, TARGET_PLAYER, MISSILE] {
            assert!(
                rig.camera(mode, &scene, Camera::new(), [0.; 2], 1.)
                    .is_err()
            );
        }
        scene.wings.clear();
        assert!(
            rig.camera(WING, &scene, Camera::new(), [0.; 2], 1.)
                .is_err()
        );
    }
    #[test]
    fn any_aircraft_can_be_the_subject_of_every_view() {
        let level = Basis::new(0., 0., 0.);
        let scene = Scene::from_parts(
            Body::new(0, [0., 1000., 0.], [0., 0., 200.], level),
            Some(2),
            vec![
                Body::new(1, [1000., 1000., 0.], [0., 0., 200.], level),
                Body::new(2, [0., 1000., 1000.], [0., 0., 200.], level),
                Body::new(3, [1000., 1000., -500.], [0., 0., 200.], level),
            ],
            vec![(1, false, 1, 1), (3, false, 1, 2)],
            vec![
                Shot::new(
                    Body::missile(10, [0., 1000., 400.], [0.; 3], level),
                    1,
                    None,
                    false,
                ),
                Shot::new(
                    Body::missile(11, [0., 1000., 500.], [0.; 3], level),
                    1,
                    Some(2),
                    false,
                ),
                Shot::new(
                    Body::missile(12, [1000., 1000., 500.], [0.; 3], level),
                    2,
                    Some(1),
                    false,
                ),
            ],
        );
        let mut rig = Rig::default();
        rig.select(Reference::Aircraft(1));
        // Cockpit-like views sit at the aircraft and hide it; never the cockpit.
        let c = camera(&mut rig, 0, &scene);
        assert_eq!(c.position, [1000., 1000., 0.]);
        assert_eq!(c.hidden_target, Some(1));
        assert!(!rig.cockpit(0) && !rig.cockpit(TRACK));
        assert_eq!(camera(&mut rig, TRACK, &scene).hidden_target, Some(1));
        assert_eq!(camera(&mut rig, 1, &scene).hidden_target, None);
        // Its newest missile, toward that missile's target.
        near(
            camera(&mut rig, MISSILE, &scene).position,
            [0., 1010., 470.],
        );
        // The missile aimed at it, and its wingman.
        near(
            camera(&mut rig, THREAT, &scene).position,
            [1000., 1060., -180.],
        );
        near(
            camera(&mut rig, WING, &scene).position,
            [1000., 1060., 180.],
        );
        // The player by id, and an aircraft no longer in the scene.
        rig.select(Reference::Aircraft(0));
        let c = camera(&mut rig, 0, &scene);
        assert_eq!(c.position, [0., 1000., 0.]);
        assert_eq!(c.hidden_target, Some(0));
        rig.select(Reference::Aircraft(9));
        assert!(rig.camera(0, &scene, Camera::new(), [0.; 2], 1.).is_err());
        rig.select(Reference::Aircraft(3));
        assert!(
            rig.camera(MISSILE, &scene, Camera::new(), [0.; 2], 1.)
                .is_err()
        );
        rig.save(0, [0.; 2], 1.);
        assert!(rig.other_shows_player());
    }
    fn view_basis(c: &Camera) -> Basis {
        Basis::new(c.yaw.into(), c.pitch.into(), -f64::from(c.roll))
    }
    fn close(a: Vector, b: Vector) {
        assert!(dot(a, b) > 0.99999, "{a:?} != {b:?}");
    }
    #[test]
    fn back_and_up_views_turn_about_the_aircraft_axes() {
        let (sin, cos) = UP_VIEW.sin_cos();
        for (yaw, pitch, bank) in [
            (0.3, 0.5, 0.7),
            (1., -0.4, -1.2),
            (2., 1.4, 2.5),
            (4., 0., std::f64::consts::PI),
        ] {
            let body = Basis::new(yaw, pitch, bank);
            let mut c = Camera::new();
            // Back: facing the tail, with the aircraft's own up, so a climb
            // lowers the view and the horizon tilts against the bank.
            turn(&mut c, body, 3);
            let back = view_basis(&c);
            close(back.forward, body.forward.map(|v| -v));
            close(back.up, body.up);
            turn(&mut c, body, 4);
            let up = view_basis(&c);
            close(
                up.forward,
                std::array::from_fn(|i| body.forward[i] * cos + body.up[i] * sin),
            );
            close(up.right, body.right);
        }
    }
    #[test]
    fn up_view_stays_with_the_aircraft_through_a_loop_and_a_barrel_roll() {
        let (sin, cos) = UP_VIEW.sin_cos();
        let steps = 720;
        for roll_rate in [0., 1.] {
            let mut body = Basis::new(0.5, 0., 0.);
            let mut previous: Option<Vector> = None;
            for _ in 0..steps {
                let step = std::f64::consts::TAU / f64::from(steps);
                body = body.rotated(body.right.map(|v| -v * step));
                body = body.rotated(body.forward.map(|v| v * step * roll_rate));
                let mut c = Camera::new();
                turn(&mut c, body, 4);
                let forward = view_basis(&c).forward;
                close(
                    forward,
                    std::array::from_fn(|i| body.forward[i] * cos + body.up[i] * sin),
                );
                // No jump from one step to the next, even across vertical.
                if let Some(previous) = previous {
                    assert!(dot(previous, forward) > 0.999, "view flipped");
                }
                previous = Some(forward);
            }
        }
    }
    #[test]
    fn wingman_view_cycles_in_member_order_and_moves_on_when_one_is_lost() {
        let mut scene = scene();
        scene.bodies.push(body(3, [0., 1000., 2000.]));
        scene.bodies.push(body(4, [0., 1000., 3000.]));
        scene.wings = vec![
            (1, false, 1, 1),
            (2, true, 1, 2),
            (3, true, 1, 4),
            (4, true, 1, 3),
        ];
        let mut rig = Rig::default();
        camera(&mut rig, WING, &scene);
        assert_eq!(rig.wingman(), Some(2));
        let mut order = Vec::new();
        for _ in 0..4 {
            rig.next_wingman();
            camera(&mut rig, WING, &scene);
            order.push(rig.wingman().unwrap());
        }
        assert_eq!(order, [4, 3, 2, 4]);
        // Without a press the view stays on the same wingman.
        camera(&mut rig, WING, &scene);
        assert_eq!(rig.wingman(), Some(4));
        // Lost: the next member in order, wrapping to the first.
        scene.bodies.retain(|b| b.id != 4);
        scene.wings.retain(|w| w.0 != 4);
        camera(&mut rig, WING, &scene);
        assert_eq!(rig.wingman(), Some(3));
        scene.bodies.retain(|b| b.id != 3);
        camera(&mut rig, WING, &scene);
        assert_eq!(rig.wingman(), Some(2));
        scene.bodies.retain(|b| b.id != 2);
        assert!(
            rig.camera(WING, &scene, Camera::new(), [0.; 2], 1.)
                .is_err()
        );
        rig.select(Reference::Player);
        assert_eq!(rig.wingman(), None);
    }
    #[test]
    fn back_view_shows_the_player_airframe_and_other_cockpit_views_do_not() {
        let rig = Rig::default();
        assert!(rig.shows_player(3));
        for view in [0, 4, TRACK] {
            assert!(!rig.shows_player(view));
        }
        for view in [1, THREAT, WING, TARGET, TARGET_PLAYER, FLY_BY, MISSILE] {
            assert!(rig.shows_player(view));
        }
    }
    #[test]
    fn default_other_view_is_back_and_retains_flight_keys() {
        let scene = scene();
        let mut rig = Rig::default();
        let mut base = body_camera(scene.player, 3);
        base.keys.insert("ArrowDown".into());
        let c = rig.other_camera(&scene, base).unwrap();
        assert!((c.yaw - std::f32::consts::PI).abs() < 1e-6);
        assert!(rig.other_shows_player());
        assert!(c.keys.contains("ArrowDown"));
        rig.save(0, [0.; 2], 1.);
        assert!(!rig.other_shows_player());
        rig.save(1, [0.; 2], 1.);
        assert!(rig.other_shows_player());
    }
}
