//! The cockpit readout: everything a seat's displays and cockpit sounds show
//! that comes from its ownship, its sensors, its threat service, the AI or the
//! mission, as plain data. D5b of the multiplayer plan; see
//! docs/ARCHITECTURE.md, "The flight screen draws a frame".
//!
//! The host computes it from the simulation ([`World::cockpit_readout`]) and a
//! launcher, the plane's position, attitude, speed and devices as the caller
//! presents them: single player builds it every rendered frame from the
//! interpolated flight, a server builds it at each snapshot from the tick's
//! flight, and a client takes the newest one from a snapshot. The displays read
//! this and never the ownship, so they draw the same from either source.
//!
//! It holds no references into combat. Its groups match the wire's
//! ([`docs/formats/net-protocol.md`](../../../docs/formats/net-protocol.md)):
//! each can be coded as changed or unchanged against a baseline. Every list is
//! bounded by a limit below; a longer one keeps the entries nearest the plane.
//! Contacts carry world positions, so a client draws them around its own
//! predicted aircraft. Static loadout and mission data (the weapon records,
//! the airport scene, the roster's sides and names) are not in it.

use crate::{ai_wings::AiWings, snapshot::Damage, target_window::TargetBrief, world::Cockpit};
use tore_formats::aircraft::AircraftId;
use tore_sim::{
    airport::Service,
    attitude::Vector,
    combat::{
        delivery::{Ccip, Delivery, DeliveryMode, LevelRelease, Warning},
        guided::GuidedSight,
        gunship::{self, Sight, SightNotice},
        gunship_impact::Impact,
        laser::{LaserSpot, LaserState, Mask},
        live::{self, Launcher, Readiness, SeekerTone, Target},
        missiles::{FiringBand, Guidance, LaunchMode, TargetRole, seeker},
        sight::{SightMode, SightSensor},
        threats::ThreatRecord,
    },
    sensors::{self, Channel, Contact, Mode, Plot, Strobe, Support, passive},
};

/// Radar and infrared contacts kept: the nearest ones.
pub const MAX_CONTACTS: usize = 128;
/// Visual contacts kept.
pub const MAX_VISUAL: usize = 64;
/// Ground page contacts kept: the page's own list limit.
pub const MAX_GROUND: usize = sensors::track::MAX_GROUND_CONTACTS;
/// Stale plots kept.
pub const MAX_PLOTS: usize = 64;
/// Received noise strobes kept.
pub const MAX_STROBES: usize = 16;
/// Map contacts kept.
pub const MAX_MAP: usize = 512;
/// Passive emitters the warning receiver lists.
pub const MAX_EMITTERS: usize = 32;
/// Missile threat records kept.
pub const MAX_THREATS: usize = 16;
/// Missiles aimed at the plane kept for the warning tone and the music.
pub const MAX_INBOUND: usize = 16;
/// AI aircraft listed as holding a lock or aiming a missile.
pub const MAX_AI_THREATS: usize = 16;
/// The plane's own guided air-to-ground weapons in flight listed with their
/// time to impact (plan section 9: at most 4).
pub const MAX_WEAPON_TTI: usize = 4;

/// The readout of one seat's plane at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct CockpitReadout {
    /// The plane it is for.
    pub plane: u32,
    /// The combat tick it shows: the warning tone's clock.
    pub tick: u64,
    /// Stores and the selected station.
    pub stores: Stores,
    /// The AC-130's gunsight; `None` on every other aircraft.
    pub gunsight: Option<GunsightReadout>,
    /// The targeting sight of an aircraft with a laser designator or a FLIR,
    /// the AC-130 excepted (its sight is `gunsight`). Host-built; on the
    /// wire since protocol 23 (the readout's Sight and Sight sensor groups).
    pub sight: Option<SightReadout>,
    /// The laser and the spot the plane's laser weapons home on: present on
    /// an aircraft with a laser designator, or while a buddy's spot is shown.
    /// Host-built; on the wire since protocol 23.
    pub laser: Option<LaserReadout>,
    /// The selected guided air-to-ground weapon's sight: its aim, range,
    /// reach and advisory (docs/spec/air-to-ground.md, "Guided bomb release
    /// range indicator"). Host-built; on the wire since protocol 23.
    pub guided: Option<GuidedSight>,
    /// The plane's own guided air-to-ground weapons in flight, oldest first,
    /// at most [`MAX_WEAPON_TTI`], with their time to impact. Host-built;
    /// on the wire since protocol 23.
    pub weapon_tti: Vec<WeaponTti>,
    /// The combat tick the plane's last guided air-to-ground weapon ended
    /// (hit or went away), for the targeting page's impact cue. Host-built;
    /// on the wire since protocol 23.
    pub guided_impact_tick: Option<u64>,
    /// The mounted seeker and its tone.
    pub seeker: SeekerReadout,
    /// What the weapon page and HUD estimate for the selected station.
    pub estimates: Estimates,
    /// The bomb or rocket sight, while an unguided bomb or a rocket pod is
    /// selected and armed.
    pub bomb_sight: Option<BombSight>,
    /// The designated, displayed and view targets.
    pub targets: Targets,
    /// Radar and infrared contacts, plots, strobes and trails.
    pub sensors: SensorReadout,
    /// Visual contacts.
    pub visual: Vec<Contact>,
    /// Map contacts.
    pub map: Vec<MapRow>,
    /// Warning receiver emitters and missile records, and what the warning
    /// tone hears.
    pub rwr: RwrReadout,
    /// Damage, faults and failure flags, and the plane's score.
    pub damage: DamageReadout,
    /// Chaff and flares left.
    pub countermeasures: Countermeasures,
    /// The tower's service and NAV mode.
    pub airport: AirportReadout,
    /// What the target window says of the displayed target beyond its row.
    pub target_window: Option<TargetBrief>,
    /// The situation music's inputs and the mission result.
    pub music: MusicReadout,
    /// The seat's share of the flight data link. Empty until the world fills
    /// it in ([`crate::World::cockpit_readout`]); not yet on the wire (slice G7).
    pub link: LinkReadout,
}

impl CockpitReadout {
    /// Any current observation of this object, on the active scope channel or
    /// visually. Channels are never collapsed into one another.
    pub fn observation(&self, id: u32) -> Option<&Contact> {
        self.sensors
            .contact(id)
            .or_else(|| self.visual.iter().find(|contact| contact.id == id))
            .or_else(|| self.sensors.ground_contact(id).map(|g| &g.contact))
    }
}

/// Stores and selection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stores {
    /// The selected station.
    pub selected: u8,
    /// Arming: a weapon selected and armed, not NAV or safe.
    pub armed: bool,
    pub launch_mode: LaunchMode,
    /// Rounds at each station; bit 0x8000 marks a failed station.
    pub ammo: Vec<u16>,
    /// Which stations held something at the start of the mission, a bit each.
    pub loaded: u32,
    /// Authoritative normalized heading/elevation pairs for the gunship's mounts.
    pub gun_aim: [f64; 6],
    pub gun_group: u8,
}
impl Stores {
    /// The rounds at `station`.
    pub fn rounds(&self, station: usize) -> u16 {
        self.ammo.get(station).copied().unwrap_or(0) & 0x7fff
    }
    /// Whether `station` has failed.
    pub fn failed(&self, station: usize) -> bool {
        self.ammo
            .get(station)
            .is_some_and(|ammo| ammo & 0x8000 != 0)
    }
    /// Whether `station` was loaded at the start of the mission, whatever it
    /// holds now.
    pub fn was_loaded(&self, station: usize) -> bool {
        self.loaded & (1 << station) != 0
    }
    /// The selected station's index.
    pub fn selected(&self) -> usize {
        usize::from(self.selected)
    }
}

/// The AC-130 gunsight as the sim holds it (`tore_sim::combat::gunship::State`,
/// docs/spec/ac130-linked-guns.md): what the target camera's gunsight page
/// and the aim-point boxes draw. On the wire since protocol 21.
#[derive(Clone, Debug, PartialEq)]
pub struct GunsightReadout {
    /// What the sight holds: free slew, a pinned ground point or a tracked
    /// object.
    pub sight: Sight,
    /// Body-relative [heading, elevation] of the line of sight, radians.
    pub look: [f64; 2],
    /// The sight is travelling back to the default view.
    pub returning: bool,
    /// The point the guns train on.
    pub aim: Option<Vector>,
    /// Each gun's pipper (the linked guns and the candidate), by source slot.
    pub impacts: [Option<Impact>; 3],
    /// The combat tick a round fired with these impacts would leave on.
    pub impacts_tick: u64,
    /// Each gun's readiness, by source slot (TERRAIN MASK included).
    pub status: [Readiness; 3],
    /// The last notice for the activity line and the tick it was raised.
    pub notice: Option<SightNotice>,
    /// The zoom step in use, 1 to 6.
    pub zoom: u8,
    /// With the gun lock on (slice nr-cona), where the guns are laid: the
    /// locked object or point. The sight slews on its own meanwhile.
    pub gun_lock: Option<Vector>,
}
impl GunsightReadout {
    /// The readout of a gunship group.
    pub fn of(group: &gunship::State) -> Self {
        Self {
            sight: group.sight,
            look: group.look,
            returning: group.returning,
            aim: group.aim,
            impacts: group.impacts,
            impacts_tick: group.impacts_tick,
            status: group.status,
            notice: group.notice,
            zoom: group.zoom(),
            gun_lock: group.gun_lock.map(|lock| lock.point),
        }
    }
}

/// The targeting sight as the sim holds it (`tore_sim::combat::sight::Sight`,
/// docs/spec/air-to-ground.md, "The targeting sight"): what the targeting
/// page and the HUD laser box draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SightReadout {
    /// Free slew, a pinned ground point or a tracked object.
    pub mode: SightMode,
    /// Body-relative [heading, elevation] of the line of sight, radians.
    pub look: [f64; 2],
    /// The sight is travelling back to its default look.
    pub returning: bool,
    /// The point the sight marks.
    pub aim: Option<Vector>,
    /// The last notice for the activity line and the tick it was raised.
    pub notice: Option<SightNotice>,
    /// The zoom step in use, 1 to 6.
    pub zoom: u8,
    /// The sensor: laser or FLIR, reach, gimbal, eye and default look.
    pub sensor: SightSensor,
}
impl SightReadout {
    pub fn of(sight: &tore_sim::combat::sight::Sight) -> Self {
        Self {
            mode: sight.mode,
            look: sight.look,
            returning: sight.returning,
            aim: sight.aim,
            notice: sight.notice,
            zoom: sight.zoom(),
            sensor: sight.sensor,
        }
    }
    /// Slewed off the sensor's default look (the targeting page shows then).
    pub fn slewed(&self) -> bool {
        self.look != self.sensor.default_look
    }
}

/// A laser spot as the plane sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpotReadout {
    pub spot: LaserSpot,
    /// The plane's own spot (LASE, `LSR`, the box's L) or a buddy's (BUDDY,
    /// `BUD`, the box's B); the owner's callsign names the buddy.
    pub own: bool,
    /// From the plane to the spot, feet.
    pub range_ft: f64,
}

/// The plane's laser and the spots it may use (docs/spec/air-to-ground.md,
/// "The laser" and "Buddy lasing").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaserReadout {
    /// The plane carries a laser designator.
    pub designator: bool,
    /// Off, ready (LSR RDY), lasing (LASE) or masked (MASK).
    pub state: LaserState,
    pub mask: Option<Mask>,
    /// Ctrl+L: on or off by hand, or the automatic rule.
    pub manual: Option<bool>,
    /// From the sight's eye to its point, feet (0 without a point).
    pub range_ft: f64,
    /// The laser's reach now, feet (halved through cloud or fog).
    pub limit_ft: f64,
    /// The spot the plane's laser weapons home on: its own while lasing,
    /// else a buddy's.
    pub spot: Option<SpotReadout>,
    /// A buddy's spot shown beside the plane's own (the second BUDDY row).
    pub buddy: Option<SpotReadout>,
}

/// One of the plane's guided air-to-ground weapons in flight
/// (docs/spec/air-to-ground.md, "Laser on the HUD": `TTI 0:12`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponTti {
    /// Its projectile number.
    pub projectile: u32,
    /// The station it left.
    pub station: u8,
    /// Laser, coordinate (JDAM) or command (AT-2).
    pub kind: Guidance,
    /// It is homing now (a blind or waiting one falls or flies straight).
    pub guiding: bool,
    /// Seconds to impact, when they can be worked out.
    pub seconds: Option<f64>,
    /// Where it is.
    pub position: Vector,
}

/// The plane's own guided weapons in flight, oldest first, at most
/// [`MAX_WEAPON_TTI`].
fn weapon_tti(state: &live::State, plane: u32) -> Vec<WeaponTti> {
    state
        .guided_flights()
        .filter(|(_, flight)| flight.owner == plane)
        .filter_map(|(id, flight)| {
            let p = state.projectiles.iter().find(|p| p.id == id)?;
            Some(WeaponTti {
                projectile: id,
                station: u8::try_from(flight.station).unwrap_or(u8::MAX),
                kind: flight.kind,
                guiding: flight.guiding(),
                seconds: flight.seconds,
                position: p.position,
            })
        })
        .take(MAX_WEAPON_TTI)
        .collect()
}

/// The mounted seeker.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SeekerReadout {
    pub status: seeker::Status,
    pub target: Option<u32>,
    pub observation: Option<seeker::Observation>,
    /// The tone the seeker plays now; none when it is silent.
    pub tone: Option<SeekerTone>,
}

/// The weapon estimates for the selected station, from the launcher the
/// readout was built for.
#[derive(Clone, Debug, PartialEq)]
pub struct Estimates {
    pub readiness: Readiness,
    /// The selected weapon's guidance is available (the radar is on for a
    /// radar missile).
    pub guidance_available: bool,
    /// A lock could be taken now.
    pub can_lock: bool,
    /// What the HUD shows of the target or the boresight return.
    pub observation: Option<seeker::Observation>,
    /// Start of the advised contiguous region, feet. None while unknown.
    pub min_range: Option<f64>,
    /// End of the advised contiguous region, feet.
    pub max_range: Option<f64>,
    /// The favourable firing band, feet.
    pub band: Option<FiringBand>,
    pub in_range: bool,
    /// Estimated hit percentage.
    pub hit_percent: u8,
    /// Seconds the missile would fly to the observed target.
    pub solution_seconds: Option<f64>,
    /// The HARM boresight cue while an anti-radiation missile is armed.
    /// Host-built; a networked client reads it from the readout's HARM cue
    /// group (protocol 23).
    pub harm: Option<HarmCue>,
}

/// Emitters the HARM boresight cue lists at most.
pub const MAX_HARM_CUE: usize = live::MAX_HARM_CUE;

/// The HARM boresight cue (opinionated, John, 2026-10-10: "act kind of like
/// boresite on a2a weapons where it shows a radiation source you can shoot
/// at"): every emitter the armed anti-radiation missile's seeker receives
/// inside its cone, nearest the bore first, and the one the trigger fires at.
/// docs/spec/air-to-ground.md, "Missiles and the HARM boresight cue".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HarmCue {
    /// At most [`MAX_HARM_CUE`], nearest the bore first.
    pub emitters: Vec<HarmEmitter>,
    /// The emitter the trigger fires at: the BORE candidate (painting first,
    /// then nearest the bore, inside the launch cone), or the designated one
    /// in CUED.
    pub candidate: Option<u32>,
    /// A designation holds the missile in CUED.
    pub cued: bool,
}

/// One emitter of the HARM cue, drawn as the RWR's ground square.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HarmEmitter {
    pub id: u32,
    pub position: Vector,
    /// Its radar paints the plane: drawn bright.
    pub painting: bool,
    /// It emits now; a cued target whose radar went off is drawn dim.
    pub emitting: bool,
}

impl HarmCue {
    /// The cue of an ownship, `None` unless an anti-radiation missile is
    /// armed.
    pub fn of(own: &live::Ownship, readiness: Readiness) -> Option<Self> {
        let w = &own.configuration().stations.get(own.selected)?.weapon;
        tore_sim::combat::missiles::Profile::for_weapon(w).filter(|p| p.holds_on_angles())?;
        if !own.armed || matches!(readiness, Readiness::Safe | Readiness::LauncherLost) {
            return None;
        }
        let cued = own.launch_mode == LaunchMode::Cued;
        let candidate = if cued {
            own.designated()
        } else {
            own.bore_observation.map(|o| o.id)
        };
        Some(Self {
            emitters: own
                .harm_cue()
                .iter()
                .take(MAX_HARM_CUE)
                .map(|e| HarmEmitter {
                    id: e.observation.id,
                    position: e.observation.position,
                    painting: e.painting,
                    emitting: e.emitting,
                })
                .collect(),
            candidate,
            cued,
        })
    }
}

/// What the HUD's bomb or rocket sight shows for the selected station
/// (docs/spec/air-to-ground.md, "The HUD in air-to-ground modes"): the
/// delivery solution combat worked out on the last tick
/// ([`tore_sim::combat::delivery`]). Host-built; a networked client reads it
/// from the readout's Bomb sight group (protocol 23).
#[derive(Clone, Debug, PartialEq)]
pub struct BombSight {
    /// CCIP or LVL; always CCIP for rockets.
    pub mode: DeliveryMode,
    /// A rocket pod: ARM and the rocket pipper.
    pub rocket: bool,
    /// The impact point and its range arc, when the round comes down.
    pub ccip: Option<Ccip>,
    /// The level release, in LVL with a ground target.
    pub level: Option<LevelRelease>,
    /// LOW, FRAG and OVERFLY, in that order.
    pub warnings: Vec<Warning>,
    /// Seconds until the plane's last released bomb lands.
    pub falling: Option<f64>,
}
impl BombSight {
    pub fn of(delivery: &Delivery) -> Self {
        Self {
            mode: delivery.mode,
            rocket: delivery.rocket,
            ccip: delivery.ccip,
            level: delivery.level,
            warnings: delivery.warnings.clone(),
            falling: delivery.falling,
        }
    }
    /// The first warning, for the readiness slot.
    pub fn warning(&self) -> Option<Warning> {
        self.warnings.first().copied()
    }
}

/// Where a target is and how hurt: what the HUD square, the target window and
/// its camera and the flight views read of it. The aircraft's hit points are
/// above zero: a destroyed target is neither displayed nor viewed.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetRow {
    pub id: u32,
    /// Exact aircraft identity; `None` for a ground object.
    pub aircraft: Option<AircraftId>,
    pub position: Vector,
    /// Ground-relative velocity, feet per second.
    pub velocity: Vector,
    /// Hit points, the initial ones and the damage in each section.
    pub damage: Damage,
}
impl TargetRow {
    pub fn of(target: &Target) -> Self {
        Self {
            id: target.id,
            aircraft: target.aircraft,
            position: target.position,
            velocity: target.velocity,
            damage: Damage {
                hp: target.hp,
                initial_hp: target.initial_hp,
                sections: target.localized_damage.amounts,
                structural: target.localized_damage.structural_section,
            },
        }
    }
    /// Hit points left.
    pub fn hp(&self) -> i32 {
        self.damage.hp
    }
}

/// The targets of the seat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targets {
    /// The designated target's identity.
    pub designated: Option<u32>,
    /// The target the HUD square and target window follow: the designated
    /// one, or with Easy targeting the last one after the sensors lose it.
    pub display: Option<TargetRow>,
    /// The target the flight views follow: the displayed one, or a dropped
    /// selection the pilot can still see (the sight hold).
    pub view: Option<TargetRow>,
}

/// Where a trail's samples are in [`SensorReadout::trail_points`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trail {
    pub id: u32,
    pub start: u16,
    pub len: u8,
}

/// Radar and infrared.
#[derive(Clone, Debug, PartialEq)]
pub struct SensorReadout {
    /// The sensors' own step count: the scope's noise phase and the blink
    /// clock of the HUD.
    pub tick: u64,
    pub selected: Option<u32>,
    pub acquired: Option<u32>,
    /// The track status of the selected target.
    pub selected_support: Option<Support>,
    /// Whether the radar, infrared and visual channels are installed.
    pub available: [bool; 3],
    /// Whether they are installed, powered and undamaged.
    pub operating: [bool; 3],
    /// The radar's track range in nautical miles: inside it the scope says TWS.
    pub radar_track_nmi: Option<f64>,
    /// Contacts of the active channel.
    pub contacts: Vec<Contact>,
    /// The last observations of lost contacts.
    pub plots: Vec<Plot>,
    /// Received noise as the scope shows it, fading included.
    pub strobes: Vec<Strobe>,
    /// Which samples of `trail_points` belong to which contact.
    pub trails: Vec<Trail>,
    /// The positions the contacts were recorded at, oldest first.
    pub trail_points: Vec<Vector>,
    /// The A-G flag (Ctrl+R): the radar shows the air-to-ground page.
    /// Host-built; on the wire since protocol 23 (the Sensors group's end).
    pub air_ground: bool,
    /// An anti-radiation weapon is aboard: the HARM page can be selected.
    /// Host-built, as `air_ground`.
    pub harm: bool,
    /// The ground page's list (A-G, the IR page's ground objects or the HARM
    /// page's emitters), nearest first, at most [`MAX_GROUND`]. Host-built;
    /// on the wire since protocol 23 (the readout's ground contacts list).
    pub ground: Vec<sensors::GroundContact>,
}
impl SensorReadout {
    const fn slot(channel: Channel) -> Option<usize> {
        match channel {
            Channel::Radar => Some(0),
            Channel::Infrared => Some(1),
            Channel::Visual => Some(2),
            Channel::Harm => None,
        }
    }
    /// Whether the channel is installed.
    pub fn available(&self, channel: Channel) -> bool {
        Self::slot(channel).map_or(self.harm, |slot| self.available[slot])
    }
    /// Whether the channel is installed, powered and undamaged.
    pub fn operating(&self, channel: Channel) -> bool {
        Self::slot(channel).map_or(self.harm, |slot| self.operating[slot])
    }
    /// A ground page contact by identity.
    pub fn ground_contact(&self, id: u32) -> Option<&sensors::GroundContact> {
        self.ground.iter().find(|g| g.contact.id == id)
    }
    /// The mode a channel would report for `controls`; the label follows the
    /// player's own control, never a step behind it.
    pub fn mode_for(&self, channel: Channel, controls: &sensors::Controls) -> Option<Mode> {
        match channel {
            Channel::Infrared | Channel::Visual => Some(Mode::Infrared),
            Channel::Harm => Some(Mode::Harm),
            Channel::Radar => self.radar_track_nmi.map(|track| {
                if self.air_ground {
                    Mode::AirGround
                } else if controls.range_nmi() <= track {
                    Mode::Tws
                } else {
                    Mode::Rws
                }
            }),
        }
    }
    /// The recorded positions of contact `id`, oldest first.
    pub fn trail(&self, id: u32) -> &[Vector] {
        self.trails
            .iter()
            .find(|trail| trail.id == id)
            .map_or(&[][..], |trail| {
                let start = usize::from(trail.start);
                &self.trail_points[start..start + usize::from(trail.len)]
            })
    }
    /// A contact of the active channel by identity.
    pub fn contact(&self, id: u32) -> Option<&Contact> {
        self.contacts.iter().find(|contact| contact.id == id)
    }
}

/// One map contact.
#[derive(Clone, Debug, PartialEq)]
pub struct MapRow {
    pub contact: Contact,
    /// A visual return identified it.
    pub identified: bool,
    pub airborne: bool,
    /// What an identified aircraft is.
    pub aircraft: Option<AircraftId>,
}

/// A missile aimed at the plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InboundMissile {
    pub id: u32,
    pub position: Vector,
    /// The seeker class of the weapon: 2 infrared, 3 radar.
    pub seeker_class: u8,
    /// An AIM-120, which the tone and the music ignore beyond a range.
    pub aim120: bool,
}

/// The warning receiver.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RwrReadout {
    /// Passive emitters received this step.
    pub emitters: Vec<passive::Emitter>,
    /// The missile threat service's records.
    pub missiles: Vec<ThreatRecord>,
    /// Missiles in flight aimed at the plane, for the warning tone.
    pub inbound: Vec<InboundMissile>,
    /// The seeker classes of the missile locks the AI holds on the plane.
    pub locks: Vec<u8>,
    /// The surface emitters (radars) whose controllers track or fire at the
    /// plane, by emitter id: the RWR draws their squares bright. Host-built
    /// (`Combat::cockpit_readout`); a networked client has none until the
    /// wire carries it (protocol 22).
    pub painting: Vec<u32>,
}

/// Damage, faults and failures.
#[derive(Clone, Debug, PartialEq)]
pub struct DamageReadout {
    pub hp: i32,
    pub damage: i32,
    pub subsystem_counts: [u8; 45],
    pub last_subsystem: Option<u8>,
    pub radar_failed: bool,
    pub visual_failed: bool,
    pub infrared_failed: bool,
    pub rwr_failed: bool,
    pub ecm_failed: bool,
    /// Rounds fired, hits and kills.
    pub shots: u32,
    pub hits: u32,
    pub kills: u32,
}

/// Chaff and flares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Countermeasures {
    pub chaff: u8,
    pub flares: u8,
}

/// The tower's service and NAV mode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AirportReadout {
    /// NAV is selected instead of a weapon.
    pub nav_mode: bool,
    /// The plane's conversation with the tower: the selected airport, the
    /// clearance and which runways are usable. Absent outside a mission core.
    pub service: Option<Service>,
}

/// The situation music's inputs that only the host knows, and the mission
/// result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MusicReadout {
    /// The designated target when it is a live enemy aircraft: its identity
    /// and where it is.
    pub designated_enemy: Option<(u32, Vector)>,
    /// AI aircraft that have the plane as their target and a guided
    /// air-to-air missile ready.
    pub aiming: Vec<u32>,
    /// The mission's result was reached.
    pub succeeded: bool,
    /// The home condition was reached this flight (latched).
    pub home: bool,
}

/// Link marks kept: one per target a flightmate has locked or been assigned.
pub const MAX_LINK_MARKS: usize = 32;

/// The seat's share of the flight data link, as the displays draw it
/// (docs/DATALINK.md, "What the player sees"). It is worked out from the
/// picture by [`crate::datalink::DataLink::readout`] and holds only what the
/// seat's displays need.
///
/// `radar` is the aircraft's radar flag. John, 2026-10-05: an aircraft with no
/// radar has no radar scope, so the scope's link marks are the one cue it lacks
/// ([`LinkReadout::on_scope`]); the target window, the HUD and the sort warning
/// keep every cue.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkReadout {
    /// The plane's aircraft has a radar, and so a radar scope to mark.
    pub radar: bool,
    /// The target the lead assigned to this plane.
    pub assigned: Option<LinkAssigned>,
    /// Hostile aircraft the side's members hold, nearest first, up to
    /// [`crate::datalink::SEAT_TRACKS`].
    pub tracks: Vec<LinkTrack>,
    /// What flightmates have locked or been assigned, by target, in target
    /// order. Worked out from the locks and assignments at once, not from
    /// the tracks, which are published four times a second (agent decision).
    pub marks: Vec<LinkMark>,
    /// The state of the flightmates other than the plane.
    pub mates: Vec<LinkMate>,
}

impl LinkReadout {
    /// Whether the plane has a radar scope to draw the link's marks on.
    pub fn on_scope(&self) -> bool {
        self.radar
    }
    /// The marks on `target`.
    pub fn mark(&self, target: u32) -> Option<&LinkMark> {
        self.marks.iter().find(|mark| mark.target == target)
    }
    /// The flightmate `plane`.
    pub fn mate(&self, plane: u32) -> Option<&LinkMate> {
        self.mates.iter().find(|mate| mate.plane == plane)
    }
    /// The assignment the plane holds.
    pub fn assignment(&self) -> Option<&LinkAssigned> {
        self.assigned.as_ref()
    }
}

/// The target the lead gave this plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkAssigned {
    pub target: u32,
    /// The plane that gave it.
    pub by: u32,
    /// The plane has held a lock on the target since.
    pub acknowledged: bool,
}

/// A hostile aircraft the side's members hold, with where it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkTrack {
    pub target: u32,
    pub position: Vector,
    pub velocity: Vector,
    /// Whose sensors hold it.
    pub source: crate::datalink::TrackSource,
}

/// A flight member by flight and place: the flight's number and the member's
/// number from zero ("Blue 1" is flight 1, member 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberRef {
    pub flight: u8,
    pub member: u8,
}

/// Who has locked a target or been given it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkMark {
    pub target: u32,
    /// The flightmates (not the plane) holding a lock on it, a bit for each
    /// member number from zero.
    pub lockers: u16,
    /// A plane of another flight of the side holding a lock on it, the
    /// lowest plane id: the lock reaches this plane over the battle net.
    pub net_lock: Option<MemberRef>,
    /// The flightmates (not the plane) the lead assigned it to, a bit for
    /// each member number from zero.
    pub assigned_to: u16,
}

impl LinkMark {
    /// The members set in `mask`, as the numbers a pilot reads: from one.
    pub fn numbers(mask: u16) -> Vec<u8> {
        (0..16u8)
            .filter(|bit| mask & (1 << bit) != 0)
            .map(|bit| bit + 1)
            .collect()
    }
}

/// One flightmate's coarse state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkMate {
    pub plane: u32,
    /// Its place in the flight from zero.
    pub member: u8,
    pub fuel: crate::datalink::Fuel,
    pub weapons: crate::datalink::Weapons,
    pub damage: crate::datalink::Damage,
}

/// The nearest `keep` entries of `rows` by `distance`, in their own order.
fn nearest<T>(rows: &mut Vec<T>, keep: usize, distance: impl Fn(&T) -> f64) {
    if rows.len() <= keep {
        return;
    }
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|a, b| distance(&rows[*a]).total_cmp(&distance(&rows[*b])));
    order.truncate(keep);
    order.sort_unstable();
    let mut index = 0;
    let mut wanted = order.into_iter().peekable();
    rows.retain(|_| {
        let keep = wanted.peek() == Some(&index);
        if keep {
            wanted.next();
        }
        index += 1;
        keep
    });
}

fn away(a: Vector, b: Vector) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// The readout of `plane` from combat, for `launcher`. `wings` is the AI, for
/// its locks, aims and the target window; `cockpit` holds what the plane keeps
/// outside combat (the tower and the mission result). `None` when the plane
/// has no ownship.
pub fn build(
    state: &live::State,
    plane: u32,
    launcher: Launcher,
    wings: Option<&AiWings>,
    cockpit: Option<&Cockpit>,
) -> Option<CockpitReadout> {
    build_with_terrain(state, plane, launcher, wings, cockpit, None)
}

/// How long a plane's terrain-aware cruise fly-out time is kept, ticks (8
/// a second). Agent design: the HUD shows it to a tenth of a second, and the
/// fly-out samples the terrain on every step (review M1).
pub const TERRAIN_TTI_TICKS: u64 = 15;

/// The terrain-aware fly-out time of each plane's selected cruise weapon,
/// kept for [`TERRAIN_TTI_TICKS`] while the station, target and launch mode
/// stay the same, so a readout built every rendered frame and at every
/// snapshot flies it at most eight times a second. Presentation only:
/// nothing in the simulation reads it.
#[derive(Debug, Default)]
pub struct TerrainTti(std::sync::Mutex<std::collections::BTreeMap<u32, TtiEntry>>);

#[derive(Clone, Copy, Debug, PartialEq)]
struct TtiEntry {
    tick: u64,
    station: usize,
    target: Option<u32>,
    mode: LaunchMode,
    seconds: Option<f64>,
}

impl TerrainTti {
    /// Forgets every kept time (a restart or a loaded checkpoint).
    pub fn clear(&mut self) {
        self.0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn seconds(
        &self,
        plane: u32,
        own: &live::Ownship,
        at: &live::LauncherView<'_>,
        own_tick: u64,
    ) -> Option<f64> {
        let key = TtiEntry {
            tick: own_tick,
            station: own.selected,
            target: at.weapon_observation().map(|o| o.id),
            mode: own.launch_mode,
            seconds: None,
        };
        let mut kept = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = kept.get(&plane)
            && (entry.station, entry.target, entry.mode) == (key.station, key.target, key.mode)
            && (entry.tick..entry.tick + TERRAIN_TTI_TICKS).contains(&own_tick)
        {
            return entry.seconds;
        }
        let seconds = at.terrain_solution().map(|s| s.seconds);
        kept.insert(plane, TtiEntry { seconds, ..key });
        seconds
    }
}

/// Build the same readout with fresh launch visibility and terrain-aware
/// cruise fly-out time. The launch range scale stays energy-only.
pub fn build_with_terrain(
    state: &live::State,
    plane: u32,
    launcher: Launcher,
    wings: Option<&AiWings>,
    cockpit: Option<&Cockpit>,
    ground: Option<&dyn Fn(f64, f64) -> f64>,
) -> Option<CockpitReadout> {
    build_with_terrain_kept(state, plane, launcher, wings, cockpit, ground, None)
}

/// [`build_with_terrain`], keeping the terrain-aware fly-out time in `kept`
/// ([`TerrainTti`]).
pub fn build_with_terrain_kept(
    state: &live::State,
    plane: u32,
    launcher: Launcher,
    wings: Option<&AiWings>,
    cockpit: Option<&Cockpit>,
    ground: Option<&dyn Fn(f64, f64) -> f64>,
    kept: Option<&TerrainTti>,
) -> Option<CockpitReadout> {
    let view = state.view(plane)?;
    let own = view.ownship();
    let display = view.display_target();
    let here = launcher.position;

    // One view of the launcher, which works out the observation and the
    // firing solution once for all of these.
    let mut at = view.at(launcher);
    if let Some(ground) = ground {
        at = at.with_terrain(ground);
    }
    let estimates = Estimates {
        readiness: at.readiness(),
        guidance_available: view.guidance_available(launcher),
        can_lock: at.can_lock(),
        observation: at.weapon_observation(),
        min_range: at.estimated_min_range(),
        max_range: at.estimated_max_range(),
        band: at.favorable_firing_band(),
        in_range: at.in_estimated_range(),
        hit_percent: at.estimated_hit_percent(),
        solution_seconds: match kept {
            Some(kept) if at.flies_terrain_solution() => {
                kept.seconds(plane, own, &at, state.tick())
            }
            _ => at.terrain_solution().map(|s| s.seconds),
        },
        harm: HarmCue::of(own, at.readiness()),
    };

    let sensors = &own.sensors;
    let mut contacts = sensors.contacts().to_vec();
    nearest(&mut contacts, MAX_CONTACTS, |c| c.distance_ft);
    let mut trails = Vec::new();
    let mut trail_points = Vec::new();
    for contact in &contacts {
        let samples = sensors.trail(contact.id);
        if !samples.is_empty() {
            trails.push(Trail {
                id: contact.id,
                start: trail_points.len() as u16,
                len: samples.len() as u8,
            });
            trail_points.extend(samples.iter().map(|sample| sample.position));
        }
    }
    let mut plots = sensors.plots().to_vec();
    nearest(&mut plots, MAX_PLOTS, |p| p.distance_ft);
    let mut strobes = sensors.display_strobes();
    strobes.truncate(MAX_STROBES);
    let mut visual = sensors.visual().to_vec();
    nearest(&mut visual, MAX_VISUAL, |c| c.distance_ft);
    let mut map: Vec<MapRow> = sensors
        .map_contacts()
        .iter()
        .map(|observed| MapRow {
            contact: observed.contact,
            identified: observed.identified,
            airborne: observed.airborne,
            aircraft: observed
                .identified
                .then(|| view.contact(observed.contact.id))
                .flatten()
                .and_then(|target| target.aircraft),
        })
        .collect();
    nearest(&mut map, MAX_MAP, |row| away(row.contact.position, here));
    let sensor_readout = SensorReadout {
        tick: sensors.tick(),
        selected: sensors.selected(),
        acquired: sensors.acquired(),
        selected_support: sensors.selected().map(|id| sensors.track_status(id)),
        available: [Channel::Radar, Channel::Infrared, Channel::Visual]
            .map(|channel| sensors.available(channel)),
        operating: [Channel::Radar, Channel::Infrared, Channel::Visual]
            .map(|channel| sensors.operating(channel)),
        radar_track_nmi: sensors
            .profiles
            .radar
            .as_ref()
            .map(|radar| radar.track.maximum_nmi()),
        contacts,
        plots,
        strobes,
        trails,
        trail_points,
        air_ground: sensors.air_ground(),
        harm: sensors.available(Channel::Harm),
        ground: sensors.ground().iter().take(MAX_GROUND).copied().collect(),
    };

    let mut emitters = own.emitters.clone();
    nearest(&mut emitters, MAX_EMITTERS, |e| {
        e.distance_nmi.unwrap_or(f64::MAX)
    });
    let mut missiles: Vec<ThreatRecord> = own.missile_threats.records().copied().collect();
    missiles.truncate(MAX_THREATS);
    let mut inbound: Vec<InboundMissile> = state
        .projectiles
        .iter()
        .filter(|p| p.incoming.is_some() && p.target == Some(plane) && p.threatening())
        .map(|p| {
            let weapon = state.weapon(p);
            InboundMissile {
                id: p.id,
                position: p.position,
                seeker_class: weapon.seeker.signature,
                aim120: weapon.source.eq_ignore_ascii_case("AIM120.JT"),
            }
        })
        .collect();
    nearest(&mut inbound, MAX_INBOUND, |m| away(m.position, here));
    let mut locks = wings.map_or_else(Vec::new, |w| w.locks_on(plane));
    locks.truncate(MAX_AI_THREATS);

    // Live, the other side, and an aircraft. Without AI wings every
    // non-friendly target (range and fixture aircraft) counts as the other
    // side.
    let designated_enemy = view
        .designated()
        .and_then(|id| view.contact(id))
        .filter(|t| t.hp > 0 && t.role == TargetRole::Aircraft)
        .filter(|t| {
            wings.map_or(!own.friendlies.contains(&t.id), |w| {
                w.slot(t.id).is_some_and(|slot| slot.side.is_enemy())
            })
        })
        .map(|t| (t.id, t.position));
    let mut aiming = wings.map_or_else(Vec::new, |w| w.aiming_at(plane));
    aiming.truncate(MAX_AI_THREATS);
    let status = cockpit.map(|cockpit| cockpit.result.status());

    Some(CockpitReadout {
        plane,
        tick: state.tick(),
        stores: Stores {
            selected: own.selected as u8,
            armed: own.armed,
            launch_mode: own.launch_mode,
            ammo: own.ammo.clone(),
            loaded: (0..own.ammo.len())
                .filter(|station| own.was_loaded(*station))
                .fold(0, |mask, station| mask | 1 << station),
            gun_aim: own
                .gunship
                .as_ref()
                .map_or([0.; 6], |guns| guns.normalized_devices()),
            gun_group: own.gunship.as_ref().map_or(0, |guns| guns.mask()),
        },
        gunsight: own.gunship.as_ref().map(GunsightReadout::of),
        sight: own.sight.as_ref().map(SightReadout::of),
        laser: laser_readout(state, own, here),
        guided: own.guided_sight,
        weapon_tti: weapon_tti(state, plane),
        guided_impact_tick: state.guided_impact(plane),
        bomb_sight: own
            .delivery
            .as_ref()
            .filter(|d| own.armed && d.station == own.selected)
            .map(BombSight::of),
        seeker: SeekerReadout {
            status: own.mounted.status,
            target: own.mounted.target,
            observation: own.mounted.observation,
            tone: at.seeker_tone(),
        },
        estimates,
        targets: Targets {
            designated: view.designated(),
            display: display.map(TargetRow::of),
            view: view.view_target().map(TargetRow::of),
        },
        sensors: sensor_readout,
        visual,
        map,
        rwr: RwrReadout {
            emitters,
            missiles,
            inbound,
            locks,
            painting: Vec::new(),
        },
        damage: DamageReadout {
            hp: own.hp,
            damage: own.damage,
            subsystem_counts: own.subsystem_counts,
            last_subsystem: own.last_subsystem.map(|i| i as u8),
            radar_failed: own.radar_failed,
            visual_failed: own.visual_failed,
            infrared_failed: own.infrared_failed,
            rwr_failed: own.rwr_failed,
            ecm_failed: own.ecm_failed,
            shots: own.shots,
            hits: own.hits,
            kills: own.kills,
        },
        countermeasures: Countermeasures {
            chaff: own.chaff,
            flares: own.flares,
        },
        airport: AirportReadout {
            nav_mode: cockpit.is_some_and(|cockpit| cockpit.airport_nav_mode),
            service: cockpit.map(|cockpit| cockpit.airport_service.clone()),
        },
        target_window: display
            .zip(wings)
            .map(|(target, wings)| TargetBrief::of(wings, plane, target.id)),
        music: MusicReadout {
            designated_enemy,
            aiming,
            succeeded: status.as_ref().is_some_and(|status| status.succeeded),
            home: status.as_ref().is_some_and(|status| status.home),
        },
        link: LinkReadout::default(),
    })
}

/// The plane's laser readout: its own laser, and a buddy's spot when a laser
/// weapon is selected and the spot lies inside that weapon's seeker range
/// (zone 0 maximum, 8.23 nmi for the GBU-10; the display rule is fitted).
fn laser_readout(state: &live::State, own: &live::Ownship, here: Vector) -> Option<LaserReadout> {
    let designator = own
        .configuration()
        .sight_sensor()
        .is_some_and(|sensor| sensor.laser);
    let seeker_range = own
        .laser_weapon_selected()
        .then(|| own.configuration().stations.get(own.selected))
        .flatten()
        .map(|station| f64::from(station.weapon.seeker.zones[0].maximum_range.max(0)));
    let shown = |spot: LaserSpot, own: bool| SpotReadout {
        spot,
        own,
        range_ft: away(spot.position, here),
    };
    let buddy = seeker_range
        .and_then(|reach| {
            state
                .buddy_spot(own.aircraft, here)
                .filter(|spot| away(spot.position, here) <= reach)
        })
        .map(|spot| shown(spot, false));
    let spot = own
        .laser
        .spot(own.aircraft, own.side)
        .map(|spot| shown(spot, true));
    (designator || buddy.is_some()).then_some(LaserReadout {
        designator,
        state: own.laser.state,
        mask: own.laser.mask,
        manual: own.laser.manual,
        range_ft: own.laser.range_ft,
        limit_ft: own.laser.limit_ft,
        spot: spot.or(buddy),
        buddy: spot.and(buddy),
    })
}

/// Bits, counted plain: every field at its natural width with one tag bit for
/// each option, and no coding at all.
pub trait PlainBits {
    fn plain_bits(&self) -> usize;
}
const VECTOR: usize = 3 * 64;
fn option<T>(value: &Option<T>, bits: impl Fn(&T) -> usize) -> usize {
    1 + value.as_ref().map_or(0, bits)
}
impl PlainBits for Contact {
    fn plain_bits(&self) -> usize {
        // id, channel, bearing, elevation, distance, position, velocity,
        // eligible and destroyed.
        32 + 2 + 3 * 64 + 2 * VECTOR + 2
    }
}
impl PlainBits for TargetRow {
    fn plain_bits(&self) -> usize {
        // id, aircraft, position, velocity, hit points, initial, six
        // sections, structural.
        32 + option(&self.aircraft, |_| 16) + 2 * VECTOR + 2 * 32 + 6 * 32 + 4
    }
}
impl CockpitReadout {
    /// The plain bits of each group, in the order of the fields: header,
    /// stores, gunsight, seeker, estimates, targets, sensors, visual, map,
    /// rwr, damage, countermeasures, airport, target window, music, and the
    /// air-to-ground round's parts together (protocol 23).
    pub fn group_bits(&self) -> [(&'static str, usize); 16] {
        let observation = |_: &seeker::Observation| 32 + 2 * VECTOR + 3 * 64;
        // Mode, its point or id, look, returning, aim, three impacts (kind,
        // point, seconds, range), their tick, three readiness codes, the
        // notice and the zoom step.
        let gunsight = option(&self.gunsight, |g| {
            2 + match g.sight {
                Sight::Free => 0,
                Sight::Pinned(_) => VECTOR,
                Sight::Tracked(_) => 32,
            } + 2 * 64
                + 1
                + option(&g.aim, |_| VECTOR)
                + g.impacts
                    .iter()
                    .map(|i| option(i, |_| 2 + VECTOR + 2 * 64))
                    .sum::<usize>()
                + 64
                + 3 * 5
                + option(&g.notice, |_| 1 + 64)
                + 3
        });
        let stores = 8 + 1 + 1 + 16 * self.stores.ammo.len() + 32;
        let seeker = 4
            + option(&self.seeker.target, |_| 32)
            + option(&self.seeker.observation, observation)
            + option(&self.seeker.tone, |_| 64 + 3);
        let estimates = 5
            + 1
            + 1
            + option(&self.estimates.observation, observation)
            + option(&self.estimates.max_range, |_| 64)
            + option(&self.estimates.band, |_| 128)
            + 1
            + 8
            + option(&self.estimates.solution_seconds, |_| 64);
        let targets = option(&self.targets.designated, |_| 32)
            + option(&self.targets.display, PlainBits::plain_bits)
            + option(&self.targets.view, PlainBits::plain_bits);
        let sensors = &self.sensors;
        let sensor_bits = 64
            + option(&sensors.selected, |_| 32)
            + option(&sensors.acquired, |_| 32)
            + option(&sensors.selected_support, |_| 4)
            + 6
            + option(&sensors.radar_track_nmi, |_| 64)
            + sensors
                .contacts
                .iter()
                .map(PlainBits::plain_bits)
                .sum::<usize>()
            // id, channel, bearing, elevation, distance, position, age
            + sensors.plots.len() * (32 + 2 + 3 * 64 + VECTOR + 32)
            // id, bearing, elevation, received, width, floor
            + sensors.strobes.len() * (32 + 5 * 64)
            + sensors.trails.len() * (32 + 16 + 8)
            + sensors.trail_points.len() * VECTOR;
        let visual = self.visual.iter().map(PlainBits::plain_bits).sum::<usize>();
        let map = self
            .map
            .iter()
            .map(|row| row.contact.plain_bits() + 2 + option(&row.aircraft, |_| 16))
            .sum::<usize>();
        let rwr = &self.rwr;
        let rwr_bits =
            // id, bearing, distance, symbol, strength
            rwr.emitters.len() * (32 + 64 + 1 + 64 + 2 + 64)
            // id, source, tick, bearing, position, velocity, class, flags,
            // radar bearing
            + rwr.missiles.len() * (32 + 2 + 64 + 64 + 1 + VECTOR + 1 + VECTOR + 3 + 3 + 1 + 64)
            + rwr.inbound.len() * (32 + VECTOR + 8 + 1)
            + rwr.locks.len() * 8;
        let damage = 32 + 32 + 45 * 8 + 9 + 5 + 3 * 32;
        // NAV, then the selected airport and the clearance (airport, runway,
        // end). The runways that are down are not counted: the service keeps
        // them in a map of the scene's objects.
        let airport = 1 + self
            .airport
            .service
            .as_ref()
            .map_or(0, |_| (1 + 32) + (1 + 32 + 32 + 1));
        // id, objective, pilot kind, activity, skill, aims at the viewer
        let window = option(&self.target_window, |_| 32 + 2 + 2 + 6 + 8 + 1);
        let music = option(&self.music.designated_enemy, |_| 32 + VECTOR)
            + 32 * self.music.aiming.len()
            + 2;
        // The targeting sight (mode, its point or id, look, returning, aim,
        // notice, zoom) and its sensor (laser, reach, gimbal, eye, default
        // look); the laser (designator, state, mask, setting, ranges) and its
        // spots (owner, side, point, target, own, range); the bomb sight
        // (mode, rocket, CCIP, level release, warnings, fall); the guided
        // sight (kind, aim, buddy, ranges, reach, IN RNG, advisory); the
        // HARM cue's emitters; the weapons in flight; the ground pages.
        let spot = |_: &SpotReadout| 32 + 32 + VECTOR + 33 + 1 + 64;
        let a2g = option(&self.sight, |s| {
            2 + match s.mode {
                SightMode::Free => 0,
                SightMode::Pinned(_) => VECTOR,
                SightMode::Tracked(_) => 32,
            } + 2 * 64
                + 1
                + option(&s.aim, |_| VECTOR)
                + option(&s.notice, |_| 2 + 64)
                + 3
                + 1
                + 64
                + 1
                + 2 * 64
                + VECTOR
                + 2 * 64
        }) + option(&self.laser, |l| {
            1 + 2 + 2 + 2 + 2 * 64 + option(&l.spot, spot) + option(&l.buddy, spot)
        }) + option(&self.bomb_sight, |b| {
            1 + 1
                + option(&b.ccip, |_| 3 * VECTOR + 4 * 64 + 4)
                + option(&b.level, |_| VECTOR + 7 * 64)
                + 3
                + option(&b.falling, |_| 64)
        }) + option(&self.guided, |g| {
            3 + option(&g.aim, |_| VECTOR)
                + 1
                + option(&g.ground_range_ft, |_| 64)
                + option(&g.slant_range_ft, |_| 64)
                + option(&g.reach, |_| 128)
                + 1
                + 3
                + option(&g.level, |_| 2 * 64)
        }) + option(&self.estimates.harm, |h| {
            option(&h.candidate, |_| 32) + 1 + h.emitters.len() * (32 + VECTOR + 2)
        }) + option(&self.guided_impact_tick, |_| 64)
            + self.weapon_tti.len() * (32 + 8 + 3 + 1 + 65 + VECTOR)
            + 2
            + self
                .sensors
                .ground
                .iter()
                .map(|g| g.contact.plain_bits() + 16 + 2)
                .sum::<usize>();
        [
            ("header", 32 + 64),
            ("stores", stores),
            ("gunsight", gunsight),
            ("seeker", seeker),
            ("estimates", estimates),
            ("targets", targets),
            ("sensors", sensor_bits),
            ("visual", visual),
            ("map", map),
            ("rwr", rwr_bits),
            ("damage", damage),
            ("countermeasures", 16),
            ("airport", airport),
            ("target window", window),
            ("music", music),
            ("air to ground", a2g),
        ]
    }
}
impl PlainBits for CockpitReadout {
    fn plain_bits(&self) -> usize {
        self.group_bits().iter().map(|(_, bits)| bits).sum()
    }
}
