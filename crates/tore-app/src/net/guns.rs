//! The gun rounds a networked client draws: its own at once from its trigger
//! and its predicted aircraft, and every other aircraft's from the host's gun
//! burst events (slice D8c; docs/ARCHITECTURE.md, "Hits and lag compensation").
//!
//! The host sends a burst as an event (shooter, gun station, first tick, and
//! the burst again with its length once it is over), never its rounds. The
//! rounds are made again here with the simulation's own rules
//! ([`tore_sim::combat::gun_round`]): the muzzle, the spread, the launch speed,
//! the way a round flies and the cadence the gun releases them on. They are
//! cosmetic. They hit nothing on the client, and the host decides every hit.
//!
//! Nothing here changes the simulation. The rounds are added to the picture as
//! ordinary [`ProjectilePose`]s with `gun: true` and the tracer flag, so the
//! drawing that already shows a host's rounds shows them.
//!
//! An AC-130's guns are trained (docs/spec/ac130-linked-guns.md): a round
//! of one leaves the tip of its barrel along the barrel, as the host's does
//! ([`gunship::muzzle`], [`gunship::direction`]), with the barrel's train as
//! the picture draws it (the snapshot's gun devices). The seat's own linked
//! guns all fire at once from its trigger, each on its own cadence.
//!
//! *Agent decisions:* the spread of a round comes from a number only this
//! side knows (the host seeds it with the round's projectile number, which no
//! message carries), so a round's line can differ from the host's within the
//! gun's half-degree cone; another aircraft's tracer pattern runs on from the
//! rounds its earlier bursts made (every third round, as the host's does),
//! starting at the first burst this client has seen of that gun; other
//! aircraft's rounds are placed from the shooter's pose drawn now, taken back
//! along its velocity to the round's release tick; a burst the host has not
//! closed yet is drawn as running on, and the rounds it overran are taken
//! back when the closing event comes (the picture's delay keeps that from
//! showing in practice); an AC-130's rounds take the train its pose had at
//! the round's release tick, from the poses the client has drawn (the
//! nearest two, blended), and the seat's own take the train its aircraft is
//! drawn with now, the host's newest readout's; the seat's own linked guns
//! fire while the readout says each may (armed, loaded and not blocked by
//! the airframe), solved or not, as the host's do.
use crate::snapshot::{AircraftPose, DEVICES, GUN_AIM, ProjectilePose, RenderSnapshot};
use std::{
    collections::{BTreeMap, VecDeque},
    f64::consts::{FRAC_PI_2, PI},
};
use tore_formats::aircraft::AircraftId;
use tore_session::wire::events::{ReceivedEvent, WireEvent};
use tore_sim::{
    attitude::{Basis, Vector},
    combat::{
        gun_round::{self, Cadence, Round},
        gunship,
        live::{self, Launcher, Readiness, Station},
    },
};
use tore_world::{readout::CockpitReadout, seats::SeatCommand};

/// Ticks a frame catches the rounds up by at most: a stall is not caught up
/// on.
const MAX_CATCH_UP: u64 = 24;
/// Rounds alive at once, the host's own cap.
const MAX_ROUNDS: usize = live::MAX_PROJECTILES;
/// Ticks of an open burst kept without its closing event (20 seconds): a
/// burst the host never closed does not fire for ever.
const OPEN_BURST_TICKS: u64 = 2_400;
/// How far back a burst's rounds are still made (2 seconds): older ones are
/// out of the air by now, whatever the gun.
const BACKLOG_TICKS: u64 = 240;
/// Ticks the shooter's own rounds are remembered for counting its rounds
/// left (4 seconds).
const FIRED_KEPT_TICKS: u64 = 480;

/// Projectile numbers for the rounds drawn here: far above the host's, so
/// neither a camera that follows a missile nor the regenerated smoke mixes
/// one up with a host's.
const OWN_IDS: u32 = 0xF000_0000;
const OTHER_IDS: u32 = 0xE000_0000;

/// An AC-130's gun train in the snapshot's device units, by gun slot:
/// heading over pi, elevation over a right angle.
pub type GunAim = [[f64; 2]; 3];

/// What a client frame brings that the rounds need.
pub struct Inputs<'a> {
    /// The newest predicted tick, which is the host tick the aircraft's last
    /// predicted step belongs to.
    pub tick: u64,
    /// The host tick the picture shows.
    pub render_tick: f64,
    /// Whether the seat's trigger is held now.
    pub trigger: bool,
    /// Whether the gun key (Tab) is held now.
    pub gun_key: bool,
    /// The seat's plane.
    pub plane: u32,
    /// The seat's aircraft as drawn: position, nose and speed.
    pub own: Launcher,
    /// The seat's AC-130 gun train as its aircraft is drawn (zero on any
    /// other aircraft).
    pub gun_aim: GunAim,
    /// What the newest readout says of the selected station, when one has
    /// come.
    pub stores: Option<Stores>,
    /// The seat's loadout.
    pub config: &'a live::Configuration,
    /// The events released since the last frame.
    pub events: &'a [ReceivedEvent],
}

/// What a cockpit readout says of the weapon selected: the page the gun
/// fires from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stores {
    pub selected: usize,
    /// The readout's readiness for the selected weapon is Ready: armed, with
    /// rounds, nothing in the way.
    pub ready: bool,
    /// Rounds at the selected station.
    pub rounds: u16,
    /// The host tick the readout shows.
    pub tick: u64,
    /// An AC-130's gun group, when the seat flies one.
    pub group: Option<Group>,
    /// The gun Tab fires while a store that is not a gun is selected: its
    /// station, whether the system is armed and its rounds.
    pub key_gun: Option<(usize, bool, u16)>,
}

/// What a readout says of an AC-130's guns, by gun slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    /// The gun's station in the loadout, when it is installed.
    pub stations: [Option<usize>; 3],
    /// The guns linked to fire together.
    pub linked: [bool; 3],
    /// The gun may fire: its readiness is READY or only advisory
    /// ([`Readiness::gun_may_fire`]).
    pub may_fire: [bool; 3],
    /// Rounds at the gun.
    pub rounds: [u16; 3],
}

impl Stores {
    /// The weapon page of `readout`, for an aircraft loaded as `config`.
    pub fn of(readout: &CockpitReadout, config: &live::Configuration) -> Self {
        let selected = readout.stores.selected();
        let group = gunship::State::new(config)
            .zip(readout.gunsight.as_ref())
            .map(|(group, sight)| Group {
                stations: group.stations,
                linked: std::array::from_fn(|slot| readout.stores.gun_group & (1 << slot) != 0),
                may_fire: sight.status.map(Readiness::gun_may_fire),
                rounds: group
                    .stations
                    .map(|station| station.map_or(0, |s| readout.stores.rounds(s))),
            });
        Self {
            selected,
            ready: readout.estimates.readiness == Readiness::Ready,
            rounds: readout.stores.rounds(selected),
            tick: readout.tick,
            group,
            key_gun: config
                .stations
                .get(selected)
                .filter(|station| !live::is_gun(&station.weapon))
                .and_then(|_| live::key_gun_station(config, |i| readout.stores.rounds(i)))
                .map(|index| (index, readout.stores.armed, readout.stores.rounds(index))),
        }
    }

    /// The stations the trigger fires, each with its gun slot on an AC-130
    /// and whether it may fire and its rounds: the linked guns when the
    /// selected station is one of an AC-130's, as the host fires them, else
    /// the selected station.
    fn firing(&self) -> Vec<(usize, Option<usize>, bool, u16)> {
        match self.group {
            Some(group) if group.stations.contains(&Some(self.selected)) => (0..3)
                .filter(|slot| group.linked[*slot])
                .filter_map(|slot| {
                    group.stations[slot].map(|station| {
                        (
                            station,
                            Some(slot),
                            group.may_fire[slot],
                            group.rounds[slot],
                        )
                    })
                })
                .collect(),
            _ => vec![(self.selected, None, self.ready, self.rounds)],
        }
    }
}

/// The seat's trigger as the host reads it: the keyboard's Space key, which
/// arrives as a seat command (`TriggerKey`, and `ReleaseTrigger` to let go),
/// and the controller's button, which is the controls' trigger. Both are
/// held by the world's own rule ([`crate::combat::FireInput`]), so the
/// trigger this says is held is the one the host's step will find held.
#[derive(Default)]
pub struct Fire {
    key: crate::combat::FireInput,
    pad: crate::combat::FireInput,
    /// Tab, the gun key (`GunKeyDown` and `GunKeyUp`; a trigger release lets
    /// go of it too, as the host's does).
    gun: bool,
}

impl Fire {
    /// One turn's commands, in order, and the controller button; whether the
    /// trigger is held after them.
    pub fn turn(&mut self, commands: &[SeatCommand], pad: bool) -> bool {
        for command in commands {
            match *command {
                SeatCommand::TriggerKey {
                    down,
                    repeat,
                    blocked,
                } => self.key.space(down, repeat, blocked),
                SeatCommand::ReleaseTrigger => {
                    self.key.cancel();
                    self.pad.cancel();
                    self.gun = false;
                }
                SeatCommand::Combat(live::Command::GunKeyDown) => self.gun = true,
                SeatCommand::Combat(live::Command::GunKeyUp) => self.gun = false,
                _ => {}
            }
        }
        self.pad.space(pad, false, false);
        self.key.held || self.pad.held
    }

    /// Whether the gun key is held after the last turn.
    pub fn gun_key(&self) -> bool {
        self.gun
    }
}

/// What the client knows of the mission.
pub struct Around<'a> {
    /// The mission's ground height by x and z.
    pub ground: &'a dyn Fn(f64, f64) -> f64,
    /// The stations of an aircraft type's usual loadout.
    pub stations: &'a dyn Fn(AircraftId) -> &'a [Station],
    /// The mission's surface units and their arms, which every client
    /// builds from the mission (protocol 22); `None` where no surface
    /// rounds are made (a recorded flight).
    pub surface: Option<&'a tore_world::surface::Surface>,
}

/// A surface unit's gun burst, as its events tell it (protocol 22).
#[derive(Clone, Debug, PartialEq)]
struct SurfaceBurst {
    unit: u32,
    mount: usize,
    target: Option<u32>,
    /// The first round's direction, azimuth from +z towards +x and
    /// elevation, radians.
    aim: [f64; 2],
    /// The host tick of its first round.
    first: u64,
    /// Its schedule: `rounds` over `span` ticks.
    rounds: u32,
    span: u64,
    /// The rounds it fired when it ended short of its schedule.
    cut: Option<u32>,
    /// Rounds made so far.
    made: u32,
    /// The lead's azimuth and elevation at the first round, from the
    /// target as this client draws it: later rounds turn from `aim` as the
    /// lead turns from it.
    lead: Option<[f64; 2]>,
}

/// A gun burst of another aircraft, as its events tell it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Burst {
    shooter: u32,
    station: usize,
    /// The host tick of its first round.
    first: u64,
    /// The host tick of its last round, once the host has closed it.
    last: Option<u64>,
    /// The rounds the gun had let go before it, which sets its tracers.
    base: u64,
    /// The tick of each round made so far.
    made: Vec<u64>,
}

/// A round of another aircraft, and the burst and tick it was let go at.
struct Other {
    /// The round's own number, which stays with it as the rounds before it
    /// leave the air (a recorded flight keeps a round's identity).
    serial: u32,
    shooter: u32,
    station: usize,
    burst: u64,
    release: u64,
    /// Its place in a surface burst (protocol 22); 0 for an aircraft's.
    index: u32,
    round: Round,
}

/// The rounds a client draws.
#[derive(Default)]
pub struct Guns {
    /// The seat's trigger schedule for each gun station it has fired, kept
    /// (as the host keeps them) so a gun's tracers run on.
    cadences: BTreeMap<usize, Cadence>,
    /// The stations the trigger fired last frame.
    firing: Vec<usize>,
    /// The seat's rounds, each with a number that stays with it while the
    /// rounds before it leave the air.
    own: Vec<(u32, Round)>,
    /// The number the seat's next round gets.
    own_serial: u32,
    /// The tick the seat's rounds have been made to.
    own_tick: u64,
    /// The ticks and stations the seat's own rounds left, for counting the
    /// rounds each gun has left against a readout older than they are.
    fired: VecDeque<(u64, usize)>,
    bursts: Vec<Burst>,
    /// The rounds each other aircraft's gun has let go, by shooter and station,
    /// as far as its bursts have told.
    ordinals: BTreeMap<(u32, usize), u64>,
    others: Vec<Other>,
    /// The host tick other aircraft's rounds have been made to.
    others_tick: u64,
    next_id: u32,
    /// The number the next round of another aircraft gets.
    next_serial: u32,
    /// Each AC-130's gun train as the pictures drew it, by host tick, for
    /// the train at a round's release.
    trains: BTreeMap<u32, VecDeque<(u64, GunAim)>>,
    /// Surface units' bursts (protocol 22).
    surface_bursts: Vec<SurfaceBurst>,
    /// The rounds each surface gun has let go, by unit and hardpoint, as
    /// far as its bursts have told: its tracer pattern runs on.
    surface_ordinals: BTreeMap<(u32, usize), u64>,
    /// The host tick each surface round's life ends at, by its number.
    surface_ends: BTreeMap<u32, u64>,
    /// Recent surface bursts at a target: (host tick, unit, hardpoint,
    /// target), what the client aims the units' pictures with
    /// (docs/spec/surface-defenses.md, "Aiming").
    surface_sights: Vec<(u64, u32, usize, u32)>,
}

impl Guns {
    /// The surface bursts of the last 5 s at a target, as the units'
    /// pictures aim with them on a client: (host tick, unit, hardpoint,
    /// target).
    pub fn surface_sights(&self) -> &[(u64, u32, usize, u32)] {
        &self.surface_sights
    }

    /// Makes and flies the rounds up to `input`'s ticks and adds every round
    /// alive to `picture`.
    pub fn step(&mut self, input: &Inputs<'_>, around: &Around<'_>, picture: &mut RenderSnapshot) {
        let own_events = |event: &&ReceivedEvent| matches!(&event.event, WireEvent::GunBurst { shooter, .. } if *shooter != input.plane);
        for received in input.events.iter().filter(own_events) {
            let WireEvent::GunBurst {
                shooter,
                station,
                length,
            } = &received.event
            else {
                continue;
            };
            self.note_burst(
                *shooter,
                usize::from(*station),
                u64::from(received.tick),
                *length,
            );
        }
        for received in input.events {
            self.note_surface(received);
        }
        self.step_own(input, around);
        self.step_others(input, around, picture);

        let pose = |id: u32, owner: u32, round: &Round| ProjectilePose {
            id,
            owner,
            weapon: round.weapon.clone(),
            shape: None,
            gun: true,
            tracer: round.tracer,
            position: round.position,
            previous: round.previous,
            direction: round.direction,
            target: None,
            incoming: false,
            speed_f8: round.speed_f8,
            powered: false,
        };
        // A round keeps its number for its whole flight, so whatever reads
        // the picture (the muzzle flashes) sees a new number only for a new
        // round.
        let own = self
            .own
            .iter()
            .map(|(serial, round)| pose(own_id(*serial), input.plane, round));
        let others = self
            .others
            .iter()
            .map(|other| pose(other_id(other.serial), other.shooter, &other.round));
        picture.projectiles.extend(own.chain(others));
    }

    /// A recorded flight's rounds (docs/ARCHITECTURE.md, "Converting a
    /// capture into a replay"): every aircraft's bursts, the player's too,
    /// made and flown to host tick `tick`, one tick at a time. `events` are
    /// the gun burst events of the tick, `picture` holds every aircraft
    /// drawn at it (the player's in its targets), and the rounds are read
    /// with [`Guns::rounds`]. A replay has no trigger, so only bursts make
    /// rounds.
    pub fn step_recorded(
        &mut self,
        events: &[ReceivedEvent],
        tick: u64,
        around: &Around<'_>,
        picture: &RenderSnapshot,
    ) {
        for received in events {
            if let WireEvent::GunBurst {
                shooter,
                station,
                length,
            } = &received.event
            {
                self.note_burst(
                    *shooter,
                    usize::from(*station),
                    u64::from(received.tick),
                    *length,
                );
            }
        }
        self.step_others_to(tick, around, picture);
    }

    /// The rounds of other aircraft in the air as [`Guns::step_recorded`]
    /// left them: a number that stays with the round, its shooter, how many
    /// ticks ago it was let go (`tick` is the tick it was stepped to) and the
    /// round.
    pub fn rounds(&self, tick: u64) -> impl Iterator<Item = (u32, u32, u64, &Round)> {
        self.others.iter().map(move |other| {
            (
                other_id(other.serial),
                other.shooter,
                tick.saturating_sub(other.release),
                &other.round,
            )
        })
    }

    /// A surface unit's burst event (protocol 22): it opens a burst with its
    /// schedule, or cuts the one that began at the same tick short.
    fn note_surface(&mut self, received: &ReceivedEvent) {
        let first = u64::from(received.tick);
        match &received.event {
            WireEvent::SurfaceBurst {
                unit,
                mount,
                target,
                aim,
                rounds,
                span,
            } => {
                if let Some(target) = target {
                    self.surface_sights.retain(|(tick, ..)| {
                        tick + tore_world::surface::aim::BURST_HOLD_TICKS >= first
                    });
                    self.surface_sights
                        .push((first, *unit, usize::from(*mount), *target));
                }
                self.surface_bursts.push(SurfaceBurst {
                    unit: *unit,
                    mount: usize::from(*mount),
                    target: *target,
                    aim: aim.map(tore_session::wire::entity::radians),
                    first,
                    rounds: u32::from(*rounds).max(1),
                    span: u64::from(*span).max(1),
                    cut: None,
                    made: 0,
                    lead: None,
                });
            }
            WireEvent::SurfaceBurstEnd { unit, mount, fired } => {
                let mount = usize::from(*mount);
                if let Some(burst) = self
                    .surface_bursts
                    .iter_mut()
                    .find(|b| b.unit == *unit && b.mount == mount && b.first == first)
                {
                    burst.cut = Some(u32::from(*fired));
                    // Rounds made past the cut that it never fired.
                    let fired = u32::from(*fired);
                    if burst.made > fired {
                        let made = burst.made;
                        burst.made = fired;
                        let ordinal = self.surface_ordinals.entry((*unit, mount)).or_default();
                        *ordinal = ordinal.saturating_sub(u64::from(made - fired));
                        let overran: Vec<u32> = self
                            .others
                            .iter()
                            .filter(|o| {
                                o.shooter == *unit
                                    && o.station == mount
                                    && o.burst == first
                                    && o.index >= fired
                            })
                            .map(|o| o.serial)
                            .collect();
                        self.others.retain(|o| !overran.contains(&o.serial));
                    }
                }
            }
            _ => {}
        }
    }

    /// Every surface burst's rounds due at host tick `tick`, flown on to
    /// `now`, as [`Guns::make_others`] makes an aircraft's.
    fn make_surface(&mut self, tick: u64, now: u64, around: &Around<'_>, picture: &RenderSnapshot) {
        let Some(surface) = around.surface else {
            self.surface_bursts.clear();
            return;
        };
        let mut bursts = std::mem::take(&mut self.surface_bursts);
        for burst in &mut bursts {
            loop {
                let limit = burst.cut.unwrap_or(burst.rounds).min(burst.rounds);
                if burst.made >= limit {
                    break;
                }
                let release = surface_release(burst.first, burst.made, burst);
                if release > tick {
                    break;
                }
                burst.made += 1;
                let key = (burst.unit, burst.mount);
                let ordinal = {
                    let n = self.surface_ordinals.entry(key).or_default();
                    *n += 1;
                    *n - 1
                };
                if now - release > BACKLOG_TICKS || self.others.len() >= MAX_ROUNDS {
                    continue;
                }
                let Some((round, end)) = surface_round(
                    surface,
                    burst,
                    release,
                    now,
                    ordinal,
                    around,
                    picture,
                    &mut self.next_id,
                ) else {
                    continue;
                };
                self.next_serial = self.next_serial.wrapping_add(1);
                if let Some(end) = end {
                    self.surface_ends.insert(self.next_serial, end);
                }
                self.others.push(Other {
                    serial: self.next_serial,
                    shooter: burst.unit,
                    station: burst.mount,
                    burst: burst.first,
                    release,
                    index: burst.made - 1,
                    round,
                });
            }
        }
        // A burst stays until its schedule is over and its rounds are made.
        bursts.retain(|b| {
            let limit = b.cut.unwrap_or(b.rounds).min(b.rounds);
            b.made < limit || b.first + b.span + BACKLOG_TICKS >= now
        });
        bursts.retain(|b| b.first + b.span + OPEN_BURST_TICKS > now);
        self.surface_bursts = bursts;
    }

    /// A burst event of another aircraft: it opens a burst, or closes the open
    /// one that began at the same tick (a closing event of a burst never
    /// opened here makes the whole burst).
    fn note_burst(&mut self, shooter: u32, station: usize, first: u64, length: Option<u32>) {
        let last = length.map(|n| first + u64::from(n.max(1)) - 1);
        match self
            .bursts
            .iter_mut()
            .find(|b| b.shooter == shooter && b.station == station && b.first == first)
        {
            Some(burst) => {
                burst.last = burst.last.or(last);
                // Rounds made while the burst was open that it never fired.
                if let Some(last) = burst.last {
                    burst.made.retain(|tick| *tick <= last);
                    self.ordinals
                        .insert((shooter, station), burst.base + burst.made.len() as u64);
                    self.others.retain(|other| {
                        !(other.shooter == shooter
                            && other.station == station
                            && other.burst == first
                            && other.release > last)
                    });
                }
            }
            None => self.bursts.push(Burst {
                shooter,
                station,
                first,
                last,
                base: self
                    .ordinals
                    .get(&(shooter, station))
                    .copied()
                    .unwrap_or_default(),
                made: Vec::new(),
            }),
        }
    }

    /// The seat's own rounds, from its trigger, up to `input.tick`.
    fn step_own(&mut self, input: &Inputs<'_>, around: &Around<'_>) {
        // The first frame, a restart and a long stall have nothing to catch
        // up on.
        if self.own_tick == 0 || input.tick < self.own_tick {
            self.own_tick = input.tick;
        }
        self.own_tick = self.own_tick.max(input.tick.saturating_sub(MAX_CATCH_UP));
        // The gun stations the trigger fires, with what each may do.
        let mut firing: Vec<_> = input
            .stores
            .map(|stores| stores.firing())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(index, slot, ready, rounds)| {
                let station = input
                    .config
                    .stations
                    .get(index)
                    .filter(|station| live::is_gun(&station.weapon))?;
                // Rounds left: the readout's, less what has left since it
                // was made.
                let since = input.stores.map_or(0, |stores| {
                    self.fired
                        .iter()
                        .filter(|(tick, s)| *s == index && *tick > stores.tick)
                        .count()
                });
                Some((
                    index,
                    slot,
                    station,
                    ready,
                    usize::from(rounds).saturating_sub(since),
                    // The gun selected fires from Tab as it does from the
                    // trigger.
                    input.trigger || input.gun_key,
                ))
            })
            .collect();
        // Tab with another store selected: the aircraft's gun has a pass of
        // its own, armed or not as the readout says (the host also holds it
        // back for a failed station, which the client cannot see).
        if input.gun_key
            && let Some((index, armed, rounds)) = input.stores.and_then(|s| s.key_gun)
            && let Some(station) = input.config.stations.get(index)
        {
            let since = input.stores.map_or(0, |stores| {
                self.fired
                    .iter()
                    .filter(|(tick, s)| *s == index && *tick > stores.tick)
                    .count()
            });
            firing.push((
                index,
                None,
                station,
                armed,
                usize::from(rounds).saturating_sub(since),
                true,
            ));
        }
        // A station the trigger no longer fires lets go of its trigger, as
        // changing weapon does.
        for index in &self.firing {
            if !firing.iter().any(|(station, ..)| station == index)
                && let Some(cadence) = self.cadences.get_mut(index)
            {
                cadence.discard();
            }
        }
        self.firing = firing.iter().map(|(index, ..)| *index).collect();
        let mut left: Vec<usize> = firing.iter().map(|f| f.4).collect();
        for tick in self.own_tick + 1..=input.tick {
            let back = (input.tick - tick) as f64 / 120.;
            self.own
                .retain_mut(|(_, round)| round.step(tick, &around.ground));
            // In slot order, as the host steps the linked guns.
            for (k, (index, slot, station, ready, _, held)) in firing.iter().enumerate() {
                let fired = self.cadences.entry(*index).or_default().step(
                    &station.weapon,
                    *held && input.own.alive,
                    *ready && left[k] > 0 && self.own.len() < MAX_ROUNDS,
                    tick,
                );
                let Some(fired) = fired else { continue };
                left[k] -= 1;
                self.fired.push_back((tick, *index));
                let launcher = Launcher {
                    position: std::array::from_fn(|i| {
                        input.own.position[i] - input.own.velocity[i] * back
                    }),
                    ..input.own
                };
                let (muzzle, direction) = gun_line(launcher, station, *slot, input.gun_aim);
                let seed = [self.next_id, input.plane, *index as u32];
                self.next_id = self.next_id.wrapping_add(1);
                if let Some(mut round) = Round::release(
                    &station.weapon,
                    muzzle,
                    direction,
                    launcher.speed_fps,
                    seed,
                    tick,
                    fired.tracer,
                ) && round.step(tick, &around.ground)
                {
                    self.own_serial = self.own_serial.wrapping_add(1);
                    self.own.push((self.own_serial, round));
                }
            }
        }
        self.own_tick = input.tick;
        while self
            .fired
            .front()
            .is_some_and(|(tick, _)| tick + FIRED_KEPT_TICKS < input.tick)
        {
            self.fired.pop_front();
        }
    }

    /// Other aircraft's rounds, from their bursts, up to the host tick the
    /// picture shows.
    fn step_others(&mut self, input: &Inputs<'_>, around: &Around<'_>, picture: &RenderSnapshot) {
        self.step_others_to(input.render_tick.max(0.) as u64, around, picture);
    }

    fn step_others_to(&mut self, now: u64, around: &Around<'_>, picture: &RenderSnapshot) {
        if self.others_tick == 0 || now < self.others_tick {
            self.others_tick = now;
        }
        self.others_tick = self.others_tick.max(now.saturating_sub(MAX_CATCH_UP));
        self.note_trains(now, picture);
        for tick in self.others_tick + 1..=now {
            let ends = &self.surface_ends;
            self.others.retain_mut(|other| {
                ends.get(&other.serial).is_none_or(|end| tick < *end)
                    && other.round.step(tick, &around.ground)
            });
            self.make_others(tick, now, around, picture);
            self.make_surface(tick, now, around, picture);
        }
        let alive: std::collections::BTreeSet<u32> = self.others.iter().map(|o| o.serial).collect();
        self.surface_ends.retain(|serial, _| alive.contains(serial));
        self.others_tick = now;
        // Bursts that are over, or were never closed, are done with.
        self.bursts.retain(|burst| match burst.last {
            Some(last) => last + BACKLOG_TICKS >= now,
            None => burst.first + OPEN_BURST_TICKS > now,
        });
    }

    /// Remembers the gun train of every AC-130 `picture` draws at host tick
    /// `now`, for the rounds let go at the ticks before it.
    fn note_trains(&mut self, now: u64, picture: &RenderSnapshot) {
        for pose in &picture.targets {
            let Some(aim) = drawn_train(pose) else {
                continue;
            };
            let train = self.trains.entry(pose.id).or_default();
            // A picture that went back (a restart, a seek) starts again.
            if train.back().is_some_and(|(tick, _)| *tick > now) {
                train.clear();
            }
            match train.back_mut() {
                Some((tick, last)) if *tick == now => *last = aim,
                _ => train.push_back((now, aim)),
            }
        }
        for train in self.trains.values_mut() {
            while train
                .front()
                .is_some_and(|(tick, _)| tick + BACKLOG_TICKS + MAX_CATCH_UP < now)
            {
                train.pop_front();
            }
        }
        self.trains.retain(|_, train| !train.is_empty());
    }

    /// The gun train `shooter`'s pose had at host tick `tick`: the nearest
    /// two the pictures drew, blended, or the nearest one.
    fn train_at(&self, shooter: u32, tick: u64) -> Option<GunAim> {
        let train = self.trains.get(&shooter)?;
        let after = train.iter().position(|(t, _)| *t >= tick);
        Some(match after {
            Some(0) => train[0].1,
            Some(k) => {
                let ((t0, a), (t1, b)) = (train[k - 1], train[k]);
                let f = (tick - t0) as f64 / (t1 - t0) as f64;
                std::array::from_fn(|slot| {
                    std::array::from_fn(|i| a[slot][i] + (b[slot][i] - a[slot][i]) * f)
                })
            }
            None => train.back()?.1,
        })
    }

    /// The rounds of every burst due at host tick `tick`, flown on to `now`.
    fn make_others(&mut self, tick: u64, now: u64, around: &Around<'_>, picture: &RenderSnapshot) {
        let mut bursts = std::mem::take(&mut self.bursts);
        for burst in &mut bursts {
            let Some(pose) = picture.targets.iter().find(|pose| pose.id == burst.shooter) else {
                continue;
            };
            let Some(aircraft) = pose.aircraft else {
                continue;
            };
            let stations = (around.stations)(aircraft);
            let Some(station) = stations
                .get(burst.station)
                .filter(|station| live::is_gun(&station.weapon))
                .or_else(|| stations.iter().find(|s| live::is_gun(&s.weapon)))
            else {
                continue;
            };
            loop {
                let n = burst.made.len() as u64;
                let release = gun_round::release_tick(&station.weapon, burst.first, n);
                if release > tick || burst.last.is_some_and(|last| release > last) {
                    break;
                }
                burst.made.push(release);
                self.ordinals
                    .insert((burst.shooter, burst.station), burst.base + n + 1);
                if now - release > BACKLOG_TICKS || self.others.len() >= MAX_ROUNDS {
                    continue;
                }
                let gun = gun_slot(pose, station).map(|slot| {
                    let train = self
                        .train_at(burst.shooter, release)
                        .or_else(|| drawn_train(pose))
                        .unwrap_or_default();
                    (slot, train)
                });
                if let Some(round) = other_round(
                    pose,
                    station,
                    gun,
                    release,
                    now,
                    gun_round::tracer(&station.weapon, burst.base + n),
                    around,
                    &mut self.next_id,
                ) {
                    self.next_serial = self.next_serial.wrapping_add(1);
                    self.others.push(Other {
                        serial: self.next_serial,
                        shooter: burst.shooter,
                        station: burst.station,
                        burst: burst.first,
                        release,
                        index: 0,
                        round,
                    });
                }
            }
        }
        self.bursts = bursts;
    }
}

/// The host tick round `k` of a surface burst leaves at: the first tick at
/// or after `first + k * span / rounds`, the host controller's schedule
/// ([`tore_sim::ai::surface::Controller::burst`]).
fn surface_release(first: u64, k: u32, burst: &SurfaceBurst) -> u64 {
    let n = u64::from(burst.rounds.max(1));
    first + (u64::from(k) * burst.span).div_ceil(n)
}

/// Azimuth (from +z towards +x) and elevation of `d`, radians.
fn azimuth_elevation(d: Vector) -> [f64; 2] {
    [d[0].atan2(d[2]), d[1].atan2(d[0].hypot(d[2]))]
}

/// The unit vector at azimuth and elevation `[a, e]`.
fn from_azimuth_elevation([a, e]: [f64; 2]) -> Vector {
    [e.cos() * a.sin(), e.sin(), e.cos() * a.cos()]
}

/// One round of a surface burst, let go at `release` and flown on to `now`,
/// with the host tick its life ends at (a little past the target's range,
/// as the host's): from the unit's mount (a moving unit's where the picture
/// draws it), along the first round's direction turned as the lead on the
/// target the picture draws has turned since the first round, a radar gun
/// observing every tick and a visual one every half second. The round's
/// first picture shows it leaving the muzzle.
#[allow(clippy::too_many_arguments)]
fn surface_round(
    surface: &tore_world::surface::Surface,
    burst: &mut SurfaceBurst,
    release: u64,
    now: u64,
    ordinal: u64,
    around: &Around<'_>,
    picture: &RenderSnapshot,
    next_id: &mut u32,
) -> Option<(Round, Option<u64>)> {
    use tore_world::surface::{UnitId, fire};
    let arms = surface.arsenal.arms(UnitId(burst.unit))?;
    let weapon = arms
        .weapons
        .iter()
        .find(|w| w.mounts.iter().any(|m| m.index == burst.mount))?;
    let arc = weapon.mounts.iter().find(|m| m.index == burst.mount)?;
    let moving = picture.surface.iter().find(|p| p.id.0 == burst.unit);
    let place = match moving {
        Some(pose) => {
            let [yaw, pitch, bank] = pose.attitude;
            fire::Place {
                origin: pose.position,
                eye: pose.position,
                basis: Basis::new(yaw, pitch, bank),
                heading: yaw,
                velocity: [0.; 3],
            }
        }
        None => arms.place(None),
    };
    let muzzle = place.muzzle(arc, burst.aim[0], around.ground);
    let radar = matches!(weapon.kind, fire::Kind::Gun { radar: true, .. });
    // The target as drawn now, taken back to the tick the gun observed it.
    let target = burst.target.and_then(|id| {
        std::iter::once(&picture.player)
            .chain(&picture.targets)
            .find(|pose| pose.id == id && pose.aircraft.is_some())
    });
    let lead = target.map(|pose| {
        let seen = if radar {
            release
        } else {
            burst.first + (release - burst.first) / VISUAL_REFRESH_TICKS * VISUAL_REFRESH_TICKS
        };
        let back = (now as f64 - seen as f64) / 120.;
        let age = (release - seen) as f64 / 120.;
        let seen_at: Vector = std::array::from_fn(|i| pose.position[i] - pose.velocity[i] * back);
        let observed = fire::observation(weapon, false, seen_at, pose.velocity, age);
        let point = fire::gun_aim_point(weapon, &place, muzzle, observed);
        let d: Vector = std::array::from_fn(|i| point[i] - muzzle[i]);
        (
            azimuth_elevation(d),
            d.iter().map(|v| v * v).sum::<f64>().sqrt(),
        )
    });
    let direction = match lead {
        Some(([a, e], _)) => {
            let [a0, e0] = *burst.lead.get_or_insert([a, e]);
            from_azimuth_elevation([burst.aim[0] + (a - a0), burst.aim[1] + (e - e0)])
        }
        None => from_azimuth_elevation(burst.aim),
    };
    let end = lead
        .and_then(|(_, distance)| {
            live::ticks_to_range(&weapon.record, fire::gun_end_range(distance))
        })
        .map(|ticks| release + ticks);
    let seed = [*next_id, burst.unit, burst.mount as u32];
    *next_id = next_id.wrapping_add(1);
    let speed = place.velocity.iter().map(|v| v * v).sum::<f64>().sqrt();
    let mut round = Round::release(
        &weapon.record,
        muzzle,
        direction,
        speed,
        seed,
        release,
        live::surface_tracer(&weapon.record, ordinal),
    )?;
    for tick in release..=now {
        if end.is_some_and(|end| tick >= end) || !round.step(tick, &around.ground) {
            return None;
        }
    }
    round.previous = muzzle;
    Some((round, end))
}

/// Ticks a visual surface gun holds its look at its target (0.5 s), as the
/// host's controller does.
const VISUAL_REFRESH_TICKS: u64 = 60;

/// The picture's number for the seat's round `serial`, in its own block.
fn own_id(serial: u32) -> u32 {
    OWN_IDS | (serial & !OWN_IDS)
}
/// The picture's number for another aircraft's round `serial`, below the
/// seat's block.
fn other_id(serial: u32) -> u32 {
    OTHER_IDS | (serial & 0x0FFF_FFFF)
}

/// One round of another aircraft, let go at `release` and flown on to `now`
/// from where the aircraft drawn at `now` was then. `gun` is an AC-130
/// gun's slot and its train at `release`.
#[allow(clippy::too_many_arguments)]
fn other_round(
    pose: &AircraftPose,
    station: &Station,
    gun: Option<(usize, GunAim)>,
    release: u64,
    now: u64,
    tracer: bool,
    around: &Around<'_>,
    next_id: &mut u32,
) -> Option<Round> {
    let back = (now - release) as f64 / 120.;
    let [yaw, pitch, bank] = pose.attitude;
    let speed = pose.velocity.iter().map(|v| v * v).sum::<f64>().sqrt();
    let launcher = Launcher {
        position: std::array::from_fn(|i| pose.position[i] - pose.velocity[i] * back),
        basis: Basis::new(yaw, pitch, bank),
        speed_fps: speed,
        velocity: pose.velocity,
        bay_ready: true,
        radar_power: false,
        radar: false,
        jammer: false,
        alive: true,
        body_present: true,
        controls: Default::default(),
    };
    let (slot, train) = gun.unzip();
    let (muzzle, direction) = gun_line(launcher, station, slot, train.unwrap_or_default());
    let seed = [*next_id, pose.id, 0];
    *next_id = next_id.wrapping_add(1);
    let mut round = Round::release(
        &station.weapon,
        muzzle,
        direction,
        speed,
        seed,
        release,
        tracer,
    )?;
    for tick in release..=now {
        if !round.step(tick, &around.ground) {
            return None;
        }
    }
    Some(round)
}

/// Where a round of `station` leaves an aircraft flying as `launcher`, and
/// the line it leaves along before its spread: an AC-130 gun (its `slot`)
/// from the tip of its barrel along the barrel, trained as `aim` says (the
/// host's [`gunship::muzzle`] and [`gunship::direction`]); any other gun
/// from its mount along the nose. A barrel with no train drawn (zero, which
/// no gun's arc reaches) points as its mesh lies, as the drawn barrel and
/// its flash do.
fn gun_line(
    launcher: Launcher,
    station: &Station,
    slot: Option<usize>,
    aim: GunAim,
) -> (Vector, Vector) {
    match slot {
        Some(slot) if aim[slot] != [0., 0.] => {
            let [heading, elevation] = aim[slot];
            let (heading, elevation) = (heading * PI, elevation * FRAC_PI_2);
            (
                gunship::muzzle(slot, launcher, heading, elevation),
                gunship::direction(launcher, heading, elevation),
            )
        }
        Some(slot) => crate::gun_flash::Mount {
            position: launcher.position,
            basis: launcher.basis,
            gun_aim: aim,
        }
        .muzzle(slot),
        None => (
            gunship::world_mount(launcher, station.mount),
            launcher.basis.forward,
        ),
    }
}

/// The slot of `station`'s gun on `pose`'s aircraft, when it is an AC-130's
/// trained gun.
fn gun_slot(pose: &AircraftPose, station: &Station) -> Option<usize> {
    (pose.aircraft == Some(AircraftId::Ac130))
        .then(|| {
            gunship::GUNS
                .iter()
                .position(|gun| station.weapon.source.eq_ignore_ascii_case(gun))
        })
        .flatten()
}

/// The gun train an AC-130's pose is drawn with.
fn drawn_train(pose: &AircraftPose) -> Option<GunAim> {
    (pose.aircraft == Some(AircraftId::Ac130)).then(|| {
        let devices = pose.devices.unwrap_or([0.; DEVICES]);
        std::array::from_fn(|slot| [devices[GUN_AIM + slot * 2], devices[GUN_AIM + slot * 2 + 1]])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_formats::weapons::Weapon;
    use tore_session::host::{BURST_SLACK_TICKS, round_interval};
    use tore_sim::combat::live::{Ownship, OwnshipInput, State};

    /// Where the guns are, feet right, up and forward of the aircraft.
    const MOUNT: Vector = [2., -1.5, 14.];
    /// The shooter, an aircraft of the mission that is not the seat's.
    const SHOOTER: u32 = 7;
    const TICKS: u64 = 700;
    /// Ticks the picture is behind the host: 100 ms.
    const DELAY: u64 = 12;

    /// The synthetic import's cannon, its burst record changed to `burst`
    /// (rounds in a burst, rounds per game round, burst time).
    fn gun(burst: (u8, u8, u8)) -> Weapon {
        let resources = tore_world::test_support::resources::resources();
        let mut weapon = Weapon::parse("M61.JT", &resources["M61.JT"]).unwrap();
        weapon.burst.game_rounds_in_burst = burst.0;
        weapon.burst.actual_rounds_per_game = burst.1;
        weapon.burst.game_burst_t = burst.2;
        weapon
    }

    fn configuration(weapon: &Weapon) -> live::Configuration {
        let mut config = tore_world::test_support::combat_fixture(false)
            .own()
            .configuration()
            .clone();
        config.stations[0].weapon = weapon.clone();
        config.stations[0].mount = MOUNT;
        config.stations[0].count = 2_000;
        config
    }

    /// The aircraft at combat tick `tick`: a banking turn, so the nose, the
    /// muzzle and the speed all change while the gun fires.
    fn launcher(tick: u64) -> Launcher {
        let seconds = tick as f64 / 120.;
        let (yaw, pitch, bank) = (0.4 + 0.05 * seconds, 0.04, 0.3);
        let basis = Basis::new(yaw, pitch, bank);
        let speed: f64 = 520. + 10. * seconds;
        let velocity = basis.forward.map(|v| v * speed);
        Launcher {
            position: std::array::from_fn(|i| [1_000., 9_000., -4_000.][i] + velocity[i] * seconds),
            basis,
            speed_fps: speed,
            velocity,
            bay_ready: true,
            radar_power: true,
            radar: true,
            jammer: false,
            alive: true,
            body_present: true,
            controls: tore_sim::sensors::Controls::default(),
        }
    }

    /// What the host did: the rounds in flight after each tick's step, in the
    /// order they were released, and the ticks and tracers of the releases.
    struct Host {
        flying: Vec<Vec<live::Projectile>>,
        fired: Vec<(u64, bool)>,
    }

    fn host(weapon: &Weapon, held: &dyn Fn(u64) -> bool) -> Host {
        let mut state = State::open_mission();
        state
            .add_ownship(
                Ownship::new(
                    SHOOTER,
                    live::DEFAULT_OWNSHIP_SIDE,
                    configuration(weapon),
                    true,
                )
                .unwrap(),
            )
            .unwrap();
        let mut run = Host {
            flying: Vec::new(),
            fired: Vec::new(),
        };
        let mut seen = 0;
        for tick in 0..TICKS {
            assert_eq!(state.tick(), tick);
            state.step(
                &[OwnshipInput {
                    aircraft: SHOOTER,
                    held: held(tick),
                    launcher: launcher(tick),
                }],
                |_, _| -100_000.,
            );
            let mut projectiles = state.projectiles.clone();
            projectiles.sort_by_key(|p| p.id);
            let before = seen;
            for p in projectiles.iter().filter(|p| p.id >= before) {
                run.fired.push((tick, p.tracer));
                seen = p.id + 1;
            }
            run.flying.push(projectiles);
        }
        run
    }

    fn ready() -> Option<Stores> {
        Some(Stores {
            selected: 0,
            ready: true,
            rounds: 2_000,
            tick: 0,
            group: None,
            key_gun: None,
        })
    }

    /// Tab with a bomb selected makes the client's own rounds as the trigger
    /// does with the gun selected: the same rounds on the same ticks, the
    /// same tracers; and a disarmed aircraft makes none.
    #[test]
    fn the_gun_key_makes_the_own_rounds_the_trigger_would() {
        let weapon = gun((4, 3, 2));
        let mut config = configuration(&weapon);
        let mut bomb = config.stations[0].clone();
        bomb.weapon.source = "MK82.JT".into();
        bomb.weapon.flags = 0x22012;
        config.stations.push(bomb);
        let ground = |_: f64, _: f64| -100_000.;
        let stations = |_: AircraftId| &[][..];
        let around = around(&ground, &stations);
        let run = |trigger: bool, gun_key: bool, stores: Option<Stores>| {
            let mut guns = Guns::default();
            let mut made = Vec::new();
            for tick in 0..TICKS {
                let mut picture = RenderSnapshot::default();
                guns.step(
                    &Inputs {
                        tick,
                        render_tick: tick as f64,
                        trigger: trigger && pulls(tick),
                        gun_key: gun_key && pulls(tick),
                        plane: SHOOTER,
                        own: launcher(tick),
                        gun_aim: GunAim::default(),
                        stores,
                        config: &config,
                        events: &[],
                    },
                    &around,
                    &mut picture,
                );
                made.push(
                    picture
                        .projectiles
                        .iter()
                        .map(|p| (p.tracer, p.position.map(f64::to_bits)))
                        .collect::<Vec<_>>(),
                );
            }
            (made, guns.fired.len())
        };
        let trigger = run(true, false, ready());
        assert!(trigger.1 > 20, "the reference fired: {}", trigger.1);
        let bomb_selected = |armed: bool| {
            Some(Stores {
                selected: 1,
                ready: true,
                rounds: 2,
                key_gun: Some((0, armed, 2_000)),
                ..ready().unwrap()
            })
        };
        let key = run(false, true, bomb_selected(true));
        assert_eq!(key, trigger, "Tab makes the trigger's rounds");
        // The trigger alone, with the bomb selected, makes none.
        assert_eq!(run(true, false, bomb_selected(true)).1, 0);
        // Disarmed: none.
        assert_eq!(run(false, true, bomb_selected(false)).1, 0);
    }

    fn around<'a>(
        ground: &'a dyn Fn(f64, f64) -> f64,
        stations: &'a dyn Fn(AircraftId) -> &'a [Station],
    ) -> Around<'a> {
        Around {
            ground,
            stations,
            surface: None,
        }
    }

    fn shooter_pose(tick: u64) -> AircraftPose {
        let at = launcher(tick);
        let [yaw, pitch, bank] = at.basis.angles();
        AircraftPose {
            id: SHOOTER,
            aircraft: Some(AircraftId::F18),
            position: at.position,
            attitude: [yaw, pitch, bank],
            velocity: at.velocity,
            airborne: true,
            ..AircraftPose::default()
        }
    }

    fn burst_event(number: u16, tick: u64, length: Option<u32>) -> ReceivedEvent {
        ReceivedEvent {
            number,
            tick: tick as u32,
            event: WireEvent::GunBurst {
                shooter: SHOOTER,
                station: 0,
                length,
            },
        }
    }

    /// The bursts the host would announce for `fired`: runs of rounds with no
    /// gap longer than the round interval plus the slack, as combat's burst
    /// tracker cuts them, with the tick each closing event arrives at.
    fn announced(weapon: &Weapon, fired: &[(u64, bool)]) -> Vec<(u64, u32, u64)> {
        let gap = round_interval(weapon) + BURST_SLACK_TICKS;
        let mut bursts: Vec<(u64, u64)> = Vec::new();
        for (tick, _) in fired {
            match bursts.last_mut() {
                Some((_, last)) if tick - *last <= gap => *last = *tick,
                _ => bursts.push((*tick, *tick)),
            }
        }
        bursts
            .into_iter()
            .map(|(first, last)| (first, (last - first + 1) as u32, last + gap + 1))
            .collect()
    }

    fn dist(a: Vector, b: Vector) -> f64 {
        (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
    }

    /// A trigger pulled in three bursts, the last one long enough to run on
    /// through the gun's own breaks.
    fn pulls(tick: u64) -> bool {
        (30..90).contains(&tick) || (200..230).contains(&tick) || (400..640).contains(&tick)
    }

    /// The guns the tests fire: slow and fast, whole and fractional ticks
    /// between rounds, and a repeat deadline with a wait.
    const BURSTS: [(u8, u8, u8); 4] = [(1, 2, 1), (4, 3, 2), (7, 3, 1), (2, 5, 3)];

    /// The seat's own rounds, drawn at once from its trigger and its aircraft,
    /// are the host's: the same count, released on the same ticks, with the
    /// same tracers, and with the host's seed for the spread, the same line.
    #[test]
    fn the_seats_own_rounds_are_the_hosts() {
        for burst in BURSTS {
            let weapon = gun(burst);
            let config = configuration(&weapon);
            let host = host(&weapon, &pulls);
            assert!(host.fired.len() > 10, "{burst:?} fired {:?}", host.fired);
            let ground = |_: f64, _: f64| -100_000.;
            let stations = |_: AircraftId| &[][..];
            let around = around(&ground, &stations);
            let mut guns = Guns::default();
            let mut released = Vec::new();
            for tick in 0..TICKS {
                let mut picture = RenderSnapshot::default();
                let inputs = Inputs {
                    tick,
                    render_tick: tick as f64,
                    trigger: pulls(tick),
                    gun_key: false,
                    plane: SHOOTER,
                    own: launcher(tick),
                    gun_aim: GunAim::default(),
                    stores: ready(),
                    config: &config,
                    events: &[],
                };
                // The first call only sets the clock.
                guns.step(&inputs, &around, &mut picture);
                let mine = &host.flying[tick as usize];
                assert_eq!(
                    picture.projectiles.len(),
                    mine.len(),
                    "{burst:?} tick {tick}: rounds in the air"
                );
                for (ours, theirs) in picture.projectiles.iter().zip(mine) {
                    assert!(ours.gun);
                    assert_eq!(ours.tracer, theirs.tracer, "{burst:?} tick {tick}");
                    assert!(
                        dist(ours.position, theirs.position) < 0.01,
                        "{burst:?} tick {tick}: {:?} against {:?}",
                        ours.position,
                        theirs.position
                    );
                    assert!(dist(ours.previous, theirs.previous) < 0.01);
                }
                if guns.fired.back().map(|(t, _)| t) == Some(&tick)
                    && released.last() != Some(&tick)
                {
                    released.push(tick);
                }
            }
            let ticks: Vec<u64> = host.fired.iter().map(|(tick, _)| *tick).collect();
            // Ticks of the first frames (tick 0 sets the clock) are not rounds.
            assert_eq!(released, ticks, "{burst:?}: release ticks");
        }
    }

    /// With a seed of its own for the spread (the host's is a projectile number
    /// only it knows) a round is never further from the host's than the gun's
    /// spread allows: half a degree between two lines in the cone, at most 26
    /// feet in a second of a 3,000 ft/s round.
    #[test]
    fn with_its_own_spread_a_round_is_within_the_cone_of_the_hosts() {
        let weapon = gun((4, 3, 2));
        let config = configuration(&weapon);
        let host = host(&weapon, &pulls);
        let ground = |_: f64, _: f64| -100_000.;
        let stations = |_: AircraftId| &[][..];
        let around = around(&ground, &stations);
        let mut guns = Guns {
            next_id: 100_000,
            ..Guns::default()
        };
        let mut worst: f64 = 0.;
        for tick in 0..TICKS {
            let mut picture = RenderSnapshot::default();
            guns.step(
                &Inputs {
                    tick,
                    render_tick: tick as f64,
                    trigger: pulls(tick),
                    gun_key: false,
                    plane: SHOOTER,
                    own: launcher(tick),
                    gun_aim: GunAim::default(),
                    stores: ready(),
                    config: &config,
                    events: &[],
                },
                &around,
                &mut picture,
            );
            let mine = &host.flying[tick as usize];
            assert_eq!(picture.projectiles.len(), mine.len());
            for (ours, theirs) in picture.projectiles.iter().zip(mine) {
                // The first second of the round's flight.
                if theirs.age <= 120 {
                    worst = worst.max(dist(ours.position, theirs.position));
                }
            }
        }
        assert!(worst > 0.5, "a different spread must differ ({worst})");
        assert!(worst < 27., "{worst} ft");
    }

    /// Another aircraft's rounds, made from its burst events alone, are the
    /// host's too: the same count, the same release ticks and, for the same
    /// spread seed, the same positions.
    #[test]
    fn another_aircrafts_rounds_come_from_its_burst_events() {
        for burst in BURSTS {
            let weapon = gun(burst);
            let config = configuration(&weapon);
            let host = host(&weapon, &pulls);
            let bursts = announced(&weapon, &host.fired);
            assert!(bursts.len() >= 3, "{burst:?}: {bursts:?}");
            let ground = |_: f64, _: f64| -100_000.;
            let stations = |id: AircraftId| {
                if id == AircraftId::F18 {
                    &config.stations[..]
                } else {
                    &[][..]
                }
            };
            let around = around(&ground, &stations);
            // The seat is another plane altogether.
            let own = Some(Stores {
                rounds: 0,
                ready: false,
                ..ready().unwrap()
            });
            let mut guns = Guns::default();
            let mut number = 0;
            // The picture is `DELAY` ticks behind the host, as a client's is.
            for tick in DELAY..TICKS + DELAY {
                let shown = tick - DELAY;
                let mut events = Vec::new();
                for (first, length, closed) in &bursts {
                    // An event is handed over once it has arrived and the
                    // picture has reached its tick.
                    if tick == *first + DELAY {
                        number += 1;
                        events.push(burst_event(number, *first, None));
                    }
                    if tick == (*closed).max(*first + DELAY) {
                        number += 1;
                        events.push(burst_event(number, *first, Some(*length)));
                    }
                }
                let mut picture = RenderSnapshot {
                    targets: vec![shooter_pose(shown)],
                    ..RenderSnapshot::default()
                };
                guns.step(
                    &Inputs {
                        tick,
                        render_tick: shown as f64,
                        trigger: false,
                        gun_key: false,
                        plane: 3,
                        own: launcher(tick),
                        gun_aim: GunAim::default(),
                        stores: own,
                        config: &config,
                        events: &events,
                    },
                    &around,
                    &mut picture,
                );
                let mine = &host.flying[shown as usize];
                assert_eq!(
                    picture.projectiles.len(),
                    mine.len(),
                    "{burst:?} tick {shown}: rounds in the air"
                );
                for (ours, theirs) in picture.projectiles.iter().zip(mine) {
                    assert!(ours.gun);
                    assert_eq!(ours.owner, SHOOTER);
                    assert_eq!(ours.tracer, theirs.tracer, "{burst:?} tick {shown}");
                    assert!(
                        dist(ours.position, theirs.position) < 0.01,
                        "{burst:?} tick {shown}: {:?} against {:?}",
                        ours.position,
                        theirs.position
                    );
                }
            }
        }
    }

    /// A burst the host has not closed yet is drawn as running on; the rounds
    /// that overran it vanish when the closing event comes.
    #[test]
    fn a_closing_event_takes_back_the_rounds_an_open_burst_overran() {
        let weapon = gun((4, 3, 2));
        let config = configuration(&weapon);
        let ground = |_: f64, _: f64| -100_000.;
        let stations = |_: AircraftId| &config.stations[..];
        let around = around(&ground, &stations);
        let mut guns = Guns::default();
        let live = |guns: &mut Guns, tick: u64, events: &[ReceivedEvent]| {
            let mut picture = RenderSnapshot {
                targets: vec![shooter_pose(tick)],
                ..RenderSnapshot::default()
            };
            guns.step(
                &Inputs {
                    tick,
                    render_tick: tick as f64,
                    trigger: false,
                    gun_key: false,
                    plane: 3,
                    own: launcher(tick),
                    gun_aim: GunAim::default(),
                    stores: None,
                    config: &config,
                    events,
                },
                &around,
                &mut picture,
            );
            picture.projectiles.len()
        };
        live(&mut guns, 99, &[]);
        // Rounds every 5 ticks from 100: at 100, 105, 110, 115 and 120.
        assert_eq!(live(&mut guns, 120, &[burst_event(1, 100, None)]), 5);
        // Closed as ending at 110: those at 115 and 120 never were.
        assert_eq!(live(&mut guns, 121, &[burst_event(2, 100, Some(11))]), 3);
        assert_eq!(live(&mut guns, 140, &[]), 3);
    }

    /// The seat's own plane in the host's burst events is not drawn again, and
    /// a burst of another aircraft that arrives late (its picture is already
    /// past the first rounds) is made up to where its rounds would be now.
    #[test]
    fn the_trigger_is_the_space_key_or_the_controller_as_the_world_holds_it() {
        let key = |down, repeat, blocked| SeatCommand::TriggerKey {
            down,
            repeat,
            blocked,
        };
        let mut fire = Fire::default();
        assert!(!fire.turn(&[], false));
        // Space goes down, and stays held while it repeats.
        assert!(fire.turn(&[key(true, false, false)], false));
        assert!(fire.turn(&[key(true, true, false)], false));
        assert!(!fire.turn(&[key(false, false, false)], false));
        // A press while blocked (a menu, a lost window) fires nothing and
        // stays blocked until it is let go.
        assert!(!fire.turn(&[key(true, false, true)], false));
        assert!(!fire.turn(&[key(true, false, false)], false));
        assert!(!fire.turn(&[key(false, false, false)], false));
        // The controller's button holds it as long as it is down, and a
        // release command lets go of both.
        assert!(fire.turn(&[], true));
        assert!(!fire.turn(&[SeatCommand::ReleaseTrigger], true));
        assert!(!fire.turn(&[], true));
        assert!(!fire.turn(&[], false));
        assert!(fire.turn(&[key(true, false, false)], false));
        assert!(!fire.turn(&[SeatCommand::ReleaseTrigger], false));
        // The gun key is its own: down and up, and a trigger release lets go.
        let tab = |down| {
            SeatCommand::Combat(if down {
                live::Command::GunKeyDown
            } else {
                live::Command::GunKeyUp
            })
        };
        let mut fire = Fire::default();
        assert!(!fire.gun_key());
        assert!(!fire.turn(&[tab(true)], false), "Tab is not the trigger");
        assert!(fire.gun_key());
        fire.turn(&[tab(false)], false);
        assert!(!fire.gun_key());
        fire.turn(&[tab(true)], false);
        fire.turn(&[SeatCommand::ReleaseTrigger], false);
        assert!(!fire.gun_key());
    }

    #[test]
    fn the_seats_own_burst_is_not_drawn_twice_and_a_late_burst_catches_up() {
        let weapon = gun((4, 3, 2));
        let config = configuration(&weapon);
        let ground = |_: f64, _: f64| -100_000.;
        let stations = |_: AircraftId| &config.stations[..];
        let around = around(&ground, &stations);
        let mut guns = Guns::default();
        let events = [burst_event(1, 100, None), burst_event(2, 100, Some(40))];
        let mut picture = RenderSnapshot {
            targets: vec![shooter_pose(100)],
            ..RenderSnapshot::default()
        };
        let mut input = Inputs {
            tick: 99,
            render_tick: 99.,
            trigger: false,
            gun_key: false,
            plane: SHOOTER,
            own: launcher(99),
            gun_aim: GunAim::default(),
            stores: None,
            config: &config,
            events: &[],
        };
        guns.step(&input, &around, &mut picture);
        input.tick = 140;
        input.render_tick = 140.;
        input.events = &events;
        guns.step(&input, &around, &mut picture);
        assert!(picture.projectiles.is_empty(), "the seat's own burst");

        // The same burst of another aircraft, seen 20 ticks after it began.
        input.plane = 3;
        let mut guns = Guns::default();
        input.tick = 99;
        input.render_tick = 99.;
        input.events = &[];
        guns.step(&input, &around, &mut picture);
        input.tick = 120;
        input.render_tick = 120.;
        input.events = &events;
        let mut picture = RenderSnapshot {
            targets: vec![shooter_pose(120)],
            ..RenderSnapshot::default()
        };
        guns.step(&input, &around, &mut picture);
        // A round every 5 ticks from tick 100 to tick 140, those from 100 to
        // 120 in the air: five of them.
        assert_eq!(picture.projectiles.len(), 5);
    }

    #[test]
    fn a_round_keeps_its_picture_number_while_the_rounds_before_it_leave() {
        let weapon = gun((4, 3, 2));
        let config = configuration(&weapon);
        let ground = |_: f64, _: f64| -100_000.;
        let stations = |_: AircraftId| &config.stations[..];
        let around = around(&ground, &stations);
        let mut guns = Guns::default();
        let events = [burst_event(1, 100, None), burst_event(2, 100, Some(40))];
        let mut seen: Vec<u32> = Vec::new();
        let mut gone: Vec<u32> = Vec::new();
        let mut previous: Vec<u32> = Vec::new();
        for tick in 99..2_000 {
            let input = Inputs {
                tick,
                render_tick: tick as f64,
                trigger: false,
                gun_key: false,
                plane: 3,
                own: launcher(tick),
                gun_aim: GunAim::default(),
                stores: None,
                config: &config,
                events: if tick == 101 { &events } else { &[] },
            };
            let mut picture = RenderSnapshot {
                targets: vec![shooter_pose(tick)],
                ..RenderSnapshot::default()
            };
            guns.step(&input, &around, &mut picture);
            let ids: Vec<u32> = picture.projectiles.iter().map(|p| p.id).collect();
            for id in &ids {
                assert!(!gone.contains(id), "round {id:x} came back at tick {tick}");
                if !seen.contains(id) {
                    seen.push(*id);
                }
            }
            gone.extend(previous.iter().filter(|id| !ids.contains(id)));
            previous = ids;
        }
        // Every round of the burst had a number of its own, and all have left.
        let rounds = (0..)
            .take_while(|n| gun_round::release_tick(&weapon, 100, *n) <= 139)
            .count();
        assert_eq!(seen.len(), rounds);
        assert!(previous.is_empty() && gone.len() == rounds);
        assert_eq!(own_id(5), OWN_IDS + 5);
        assert_eq!(other_id(0x1234_5678), OTHER_IDS + 0x0234_5678);
    }

    /// An AC-130 in a left orbit at 4,500 feet over flat ground, its three
    /// guns linked and trained by the host's own sight (the default view),
    /// firing a long burst: the rounds a client remakes leave the right
    /// barrels along their train and land where the host's land.
    mod ac130 {
        use super::*;
        use tore_world::{
            mission::{MissionSpec, Start},
            test_support::resources::{THEATER, gunship_resources},
            world::{Seating, World},
        };

        const HELD: std::ops::Range<u64> = 600..1_500;
        const RUN: u64 = 2_400;
        /// The round trip the readout's train is late by: 100 ms.
        const RTT: u64 = 12;
        const ALTITUDE: f64 = 4_500.;
        const SPEED: f64 = 400.;

        fn config() -> live::Configuration {
            let mut spec = MissionSpec::new(THEATER, AircraftId::Ac130);
            spec.start = Start::Airborne { altitude_ft: 5_000 };
            let world = World::new(&spec, &gunship_resources(), Seating::SinglePlayer).unwrap();
            world.combat.state.own().configuration().clone()
        }

        /// A coordinated 25 degree left turn (the yaw falls; a negative bank
        /// lifts the right wing).
        fn launcher(tick: u64) -> Launcher {
            let seconds = tick as f64 / 120.;
            let bank = -25_f64.to_radians();
            let rate = -32.174 * bank.abs().tan() / SPEED;
            let yaw0: f64 = 0.3;
            let yaw = yaw0 + rate * seconds;
            let basis = Basis::new(yaw, 0., bank);
            let r = SPEED / rate;
            Launcher {
                position: [
                    2_000. + r * (yaw0.cos() - yaw.cos()),
                    ALTITUDE,
                    -3_000. + r * (yaw.sin() - yaw0.sin()),
                ],
                basis,
                speed_fps: SPEED,
                velocity: basis.forward.map(|v| v * SPEED),
                bay_ready: true,
                radar_power: true,
                radar: true,
                jammer: false,
                alive: true,
                body_present: true,
                controls: tore_sim::sensors::Controls::default(),
            }
        }

        /// The host after each tick's step.
        struct Tick {
            flying: Vec<live::Projectile>,
            /// The train the step fired with, in device units.
            aim: GunAim,
            stores: Stores,
        }

        struct Host {
            ticks: Vec<Tick>,
            /// Release tick and station of every round, in order.
            fired: Vec<(u64, usize)>,
            /// Where each round, by id, came down.
            impacts: BTreeMap<u32, Vector>,
            /// Each round's station and release tick, by id.
            rounds: BTreeMap<u32, (usize, u64)>,
        }

        fn ground(_: f64, _: f64) -> f64 {
            0.
        }

        /// Where a round whose last tick ran from `previous` to `position`
        /// meets the ground, carried on along that tick's line.
        fn landing(previous: Vector, position: Vector) -> Vector {
            let d: Vector = std::array::from_fn(|i| position[i] - previous[i]);
            if d[1] >= 0. {
                return position;
            }
            let k = position[1] / -d[1];
            std::array::from_fn(|i| position[i] + d[i] * k)
        }

        fn host(config: &live::Configuration) -> Host {
            let mut state = State::open_mission();
            state
                .add_ownship(
                    Ownship::new(SHOOTER, live::DEFAULT_OWNSHIP_SIDE, config.clone(), true)
                        .unwrap(),
                )
                .unwrap();
            let group = state.own_mut().gunship.as_mut().expect("an AC-130");
            group.included = [true; 3];
            let stations = group.stations;
            let mut run = Host {
                ticks: Vec::new(),
                fired: Vec::new(),
                impacts: BTreeMap::new(),
                rounds: BTreeMap::new(),
            };
            let mut last: BTreeMap<u32, (Vector, Vector)> = BTreeMap::new();
            for tick in 0..RUN {
                state.step(
                    &[OwnshipInput {
                        aircraft: SHOOTER,
                        held: HELD.contains(&tick),
                        launcher: launcher(tick),
                    }],
                    ground,
                );
                let mut flying = state.projectiles.clone();
                flying.sort_by_key(|p| p.id);
                for p in &flying {
                    if let std::collections::btree_map::Entry::Vacant(round) =
                        run.rounds.entry(p.id)
                    {
                        round.insert((p.station, tick));
                        run.fired.push((tick, p.station));
                    }
                }
                for (id, (previous, position)) in &last {
                    if !flying.iter().any(|p| p.id == *id) {
                        run.impacts.insert(*id, landing(*previous, *position));
                    }
                }
                last = flying
                    .iter()
                    .map(|p| (p.id, (p.previous, p.position)))
                    .collect();
                let own = state.own();
                let group = own.gunship.as_ref().unwrap();
                let devices = group.normalized_devices();
                run.ticks.push(Tick {
                    flying,
                    aim: std::array::from_fn(|slot| [devices[slot * 2], devices[slot * 2 + 1]]),
                    stores: Stores {
                        selected: own.selected,
                        ready: false,
                        rounds: own.rounds(own.selected),
                        tick,
                        group: Some(Group {
                            stations,
                            linked: group.included,
                            may_fire: group.status.map(Readiness::gun_may_fire),
                            rounds: stations.map(|s| s.map_or(0, |s| own.rounds(s))),
                        }),
                        key_gun: None,
                    },
                });
            }
            run
        }

        /// The train as the wire carries it (the snapshot's devices and the
        /// readout's stores), at `steps` either way.
        fn quantized(aim: GunAim, steps: f64) -> GunAim {
            aim.map(|gun| gun.map(|v| (v * steps).round() / steps))
        }
        fn wire(aim: GunAim) -> GunAim {
            quantized(aim, tore_session::wire::entity::GUN_AIM_STEPS)
        }
        /// Before protocol 21's finer gun angles: 1/127.
        fn old_wire(aim: GunAim) -> GunAim {
            quantized(aim, 127.)
        }

        /// The client's impacts agree with the host's as well as two draws
        /// of the spread do: no worse round, and the stream's centre no
        /// further off, than with the host's train exactly, give or take
        /// `slack` feet.
        fn within(wired: &Agreement, tolerance: &Agreement, slack: f64) -> bool {
            (0..3).all(|slot| {
                wired.worst[slot] <= tolerance.worst[slot] + slack
                    && wired.centre[slot] <= tolerance.centre[slot] + slack
            })
        }

        /// What a client drew of each round, by picture id: its weapon, the
        /// line it was first drawn on, and where it came down.
        #[derive(Default)]
        struct Drawn {
            first: BTreeMap<u32, (String, Vector, Vector)>,
            order: Vec<u32>,
            last: BTreeMap<u32, (Vector, Vector)>,
            impacts: BTreeMap<u32, Vector>,
        }
        impl Drawn {
            fn note(&mut self, picture: &RenderSnapshot) {
                for p in &picture.projectiles {
                    if let std::collections::btree_map::Entry::Vacant(first) =
                        self.first.entry(p.id)
                    {
                        first.insert((p.weapon.clone(), p.previous, p.direction));
                        self.order.push(p.id);
                    }
                }
                let ids: Vec<u32> = picture.projectiles.iter().map(|p| p.id).collect();
                for (id, (previous, position)) in &self.last {
                    if !ids.contains(id) {
                        self.impacts.insert(*id, landing(*previous, *position));
                    }
                }
                self.last = picture
                    .projectiles
                    .iter()
                    .map(|p| (p.id, (p.previous, p.position)))
                    .collect();
            }
            /// The rounds of gun `slot` in the order they were drawn.
            fn of(&self, slot: usize) -> Vec<u32> {
                self.order
                    .iter()
                    .copied()
                    .filter(|id| self.first[id].0.eq_ignore_ascii_case(gunship::GUNS[slot]))
                    .collect()
            }
        }

        /// How a client's rounds compare with the host's, gun by gun: the
        /// rounds paired in order, the worst and mean miss between paired
        /// impacts, and how far apart the two streams' mean impact points
        /// are.
        #[derive(Debug)]
        struct Agreement {
            rounds: [usize; 3],
            worst: [f64; 3],
            mean: [f64; 3],
            centre: [f64; 3],
        }

        fn agreement(host: &Host, drawn: &Drawn, config: &live::Configuration) -> Agreement {
            let stations = gunship::State::new(config).unwrap().stations;
            let mut out = Agreement {
                rounds: [0; 3],
                worst: [0.; 3],
                mean: [0.; 3],
                centre: [0.; 3],
            };
            for (slot, gun) in stations.into_iter().enumerate() {
                let ours = drawn.of(slot);
                let theirs: Vec<u32> = host
                    .rounds
                    .iter()
                    .filter(|(_, (station, _))| Some(*station) == gun)
                    .map(|(id, _)| *id)
                    .collect();
                assert_eq!(ours.len(), theirs.len(), "gun {slot}: rounds drawn");
                let pairs: Vec<(Vector, Vector)> = ours
                    .iter()
                    .zip(&theirs)
                    .filter_map(|(a, b)| Some((*drawn.impacts.get(a)?, *host.impacts.get(b)?)))
                    .collect();
                assert!(!pairs.is_empty(), "gun {slot}: no impacts");
                let n = pairs.len() as f64;
                out.rounds[slot] = pairs.len();
                for (a, b) in &pairs {
                    out.worst[slot] = out.worst[slot].max(dist(*a, *b));
                    out.mean[slot] += dist(*a, *b) / n;
                }
                let centre = |k: usize| -> Vector {
                    std::array::from_fn(|i| pairs.iter().map(|p| [p.0, p.1][k][i]).sum::<f64>() / n)
                };
                out.centre[slot] = dist(centre(0), centre(1));
            }
            out
        }

        fn angle(a: Vector, b: Vector) -> f64 {
            let dot: f64 = (0..3).map(|i| a[i] * b[i]).sum();
            let norm = |v: Vector| v.iter().map(|x| x * x).sum::<f64>().sqrt();
            (dot / norm(a) / norm(b)).clamp(-1., 1.).acos().to_degrees()
        }

        /// Every round drawn left its gun's barrel tip along the barrel the
        /// host fired it with, not the nose: within the gun's spread and
        /// `slack` degrees, from within `reach` feet of the host's muzzle.
        fn check_barrels(
            host: &Host,
            drawn: &Drawn,
            config: &live::Configuration,
            slack: f64,
            reach: f64,
        ) {
            let stations = gunship::State::new(config).unwrap().stations;
            for (slot, gun) in stations.into_iter().enumerate() {
                let theirs: Vec<u64> = host
                    .fired
                    .iter()
                    .filter(|(_, station)| Some(*station) == gun)
                    .map(|(tick, _)| *tick)
                    .collect();
                for (id, release) in drawn.of(slot).iter().zip(theirs) {
                    let (_, muzzle, direction) = &drawn.first[id];
                    let at = launcher(release);
                    let [h, e] = host.ticks[release as usize].aim[slot];
                    let (h, e) = (h * PI, e * FRAC_PI_2);
                    let barrel = gunship::direction(at, h, e);
                    let tip = gunship::muzzle(slot, at, h, e);
                    assert!(
                        angle(*direction, barrel) < 0.25 + slack,
                        "gun {slot} round at {release}: {} degrees off the barrel",
                        angle(*direction, barrel)
                    );
                    assert!(angle(*direction, at.basis.forward) > 30.);
                    assert!(
                        dist(*muzzle, tip) < reach,
                        "gun {slot} round at {release}: {} ft from the muzzle",
                        dist(*muzzle, tip)
                    );
                }
            }
        }

        /// The seat's own rounds with `aim(tick)` as its drawn train and
        /// `stores(tick)` as its readout, drawn every tick.
        fn own(
            host: &Host,
            config: &live::Configuration,
            first_id: u32,
            late: u64,
            train: &dyn Fn(GunAim) -> GunAim,
        ) -> Drawn {
            let none = |_: AircraftId| &[][..];
            let around = around(&ground, &none);
            let mut guns = Guns {
                next_id: first_id,
                ..Guns::default()
            };
            let mut drawn = Drawn::default();
            for tick in 0..RUN {
                let seen = &host.ticks[tick.saturating_sub(late) as usize];
                let mut picture = RenderSnapshot::default();
                guns.step(
                    &Inputs {
                        tick,
                        render_tick: tick as f64,
                        trigger: HELD.contains(&tick),
                        gun_key: false,
                        plane: SHOOTER,
                        own: launcher(tick),
                        gun_aim: train(seen.aim),
                        stores: Some(seen.stores),
                        config,
                        events: &[],
                    },
                    &around,
                    &mut picture,
                );
                drawn.note(&picture);
            }
            drawn
        }

        /// A round takes the train its shooter was drawn with at its release
        /// tick, blended between the pictures either side of it, even when
        /// its burst arrives late.
        #[test]
        fn a_late_round_takes_the_train_drawn_at_its_release() {
            let pose = |aim: f64| {
                let mut devices = [0.; DEVICES];
                devices[GUN_AIM] = aim;
                AircraftPose {
                    id: SHOOTER,
                    aircraft: Some(AircraftId::Ac130),
                    devices: Some(devices),
                    ..AircraftPose::default()
                }
            };
            let mut guns = Guns::default();
            for (tick, aim) in [(100, -0.4), (110, -0.6)] {
                let picture = RenderSnapshot {
                    targets: vec![pose(aim)],
                    ..RenderSnapshot::default()
                };
                guns.note_trains(tick, &picture);
            }
            let heading = |tick| guns.train_at(SHOOTER, tick).unwrap()[0][0];
            assert!((heading(105) + 0.5).abs() < 1e-12);
            assert_eq!(heading(90), -0.4);
            assert_eq!(heading(120), -0.6);
            assert_eq!(guns.train_at(3, 105), None);
            // Another aircraft's pose keeps no train.
            let mut f18 = pose(-0.5);
            f18.aircraft = Some(AircraftId::F18);
            assert_eq!(drawn_train(&f18), None);
        }

        /// The seat's rounds with the host's own train, readout and round
        /// numbers are the host's rounds exactly, on every linked gun.
        #[test]
        fn the_seats_own_linked_guns_fire_the_hosts_rounds() {
            let config = config();
            let host = host(&config);
            let none = |_: AircraftId| &[][..];
            let around = around(&ground, &none);
            let mut guns = Guns::default();
            for tick in 0..RUN {
                let seen = &host.ticks[tick as usize];
                let mut picture = RenderSnapshot::default();
                guns.step(
                    &Inputs {
                        tick,
                        render_tick: tick as f64,
                        trigger: HELD.contains(&tick),
                        gun_key: false,
                        plane: SHOOTER,
                        own: launcher(tick),
                        gun_aim: seen.aim,
                        stores: Some(seen.stores),
                        config: &config,
                        events: &[],
                    },
                    &around,
                    &mut picture,
                );
                assert_eq!(
                    picture.projectiles.len(),
                    seen.flying.len(),
                    "tick {tick}: rounds in the air"
                );
                for (ours, theirs) in picture.projectiles.iter().zip(&seen.flying) {
                    assert_eq!(ours.tracer, theirs.tracer, "tick {tick}");
                    assert!(
                        dist(ours.position, theirs.position) < 0.01,
                        "tick {tick}: {:?} against {:?}",
                        ours.position,
                        theirs.position
                    );
                }
            }
            let per_gun = |station: Option<usize>| {
                host.fired
                    .iter()
                    .filter(|(_, s)| Some(*s) == station)
                    .count()
            };
            let stations = gunship::State::new(&config).unwrap().stations;
            assert!(
                per_gun(stations[0]) > 100 && per_gun(stations[1]) > 5 && per_gun(stations[2]) >= 2
            );
        }

        /// As a client draws them: the train is the readout's (a round trip
        /// late, at the wire's 1/127) and the spread its own. The rounds
        /// leave the right barrels and land as near the host's as the
        /// host's own spread lets two rounds land.
        #[test]
        fn the_seats_own_linked_guns_leave_their_barrels_and_land_with_the_hosts() {
            let config = config();
            let host = host(&config);
            // The tolerance: the host's train exactly, a spread of its own.
            let spread = own(&host, &config, 100_000, 0, &|aim| aim);
            let tolerance = agreement(&host, &spread, &config);
            let drawn = own(&host, &config, 100_000, RTT, &wire);
            check_barrels(&host, &drawn, &config, 0.05, 0.1);
            let late = agreement(&host, &drawn, &config);
            let old = agreement(
                &host,
                &own(&host, &config, 100_000, RTT, &old_wire),
                &config,
            );
            eprintln!(
                "own, spread only: {tolerance:?}\nown, wire train: {late:?}\nown, 1/127 train: {old:?}"
            );
            assert!(
                within(&late, &tolerance, 5.),
                "{late:?} against {tolerance:?}"
            );
            // The coarser train the wire carried before missed by more than
            // the spread does, the same way every round.
            assert!(
                !within(&old, &tolerance, 5.),
                "{old:?} against {tolerance:?}"
            );
        }

        /// Another player's AC-130 from its burst events and its drawn pose:
        /// the rounds leave its barrels along the train it is drawn with and
        /// land with the host's.
        #[test]
        fn another_ac130s_linked_guns_leave_their_barrels_and_land_with_the_hosts() {
            let config = config();
            let host = host(&config);
            let stations = gunship::State::new(&config).unwrap().stations;
            let mut events: Vec<(u64, ReceivedEvent)> = Vec::new();
            let mut number = 0;
            for station in stations.into_iter().flatten() {
                let weapon = &config.stations[station].weapon;
                let fired: Vec<(u64, bool)> = host
                    .fired
                    .iter()
                    .filter(|(_, s)| *s == station)
                    .map(|(tick, _)| (*tick, false))
                    .collect();
                for (first, length, closed) in announced(weapon, &fired) {
                    for (at, length) in [
                        (first + DELAY, None),
                        (closed.max(first + DELAY), Some(length)),
                    ] {
                        number += 1;
                        events.push((
                            at,
                            ReceivedEvent {
                                number,
                                tick: first as u32,
                                event: WireEvent::GunBurst {
                                    shooter: SHOOTER,
                                    station: station as u8,
                                    length,
                                },
                            },
                        ));
                    }
                }
            }
            let usual = |id: AircraftId| {
                if id == AircraftId::Ac130 {
                    &config.stations[..]
                } else {
                    &[][..]
                }
            };
            let around = around(&ground, &usual);
            let run = |train: &dyn Fn(GunAim) -> GunAim| {
                let mut guns = Guns::default();
                let mut drawn = Drawn::default();
                for tick in DELAY..RUN + DELAY {
                    let shown = tick - DELAY;
                    let now: Vec<ReceivedEvent> = events
                        .iter()
                        .filter(|(at, _)| *at == tick)
                        .map(|(_, event)| event.clone())
                        .collect();
                    let at = launcher(shown);
                    let mut devices = [0.; DEVICES];
                    for (slot, [h, e]) in train(host.ticks[shown as usize].aim)
                        .into_iter()
                        .enumerate()
                    {
                        devices[GUN_AIM + slot * 2] = h;
                        devices[GUN_AIM + slot * 2 + 1] = e;
                    }
                    let mut picture = RenderSnapshot {
                        targets: vec![AircraftPose {
                            id: SHOOTER,
                            aircraft: Some(AircraftId::Ac130),
                            position: at.position,
                            attitude: at.basis.angles(),
                            velocity: at.velocity,
                            devices: Some(devices),
                            airborne: true,
                            ..AircraftPose::default()
                        }],
                        ..RenderSnapshot::default()
                    };
                    guns.step(
                        &Inputs {
                            tick,
                            render_tick: shown as f64,
                            trigger: false,
                            gun_key: false,
                            plane: 3,
                            own: launcher(tick),
                            gun_aim: GunAim::default(),
                            stores: None,
                            config: &config,
                            events: &now,
                        },
                        &around,
                        &mut picture,
                    );
                    drawn.note(&picture);
                }
                drawn
            };
            let tolerance = agreement(&host, &run(&|aim| aim), &config);
            let drawn = run(&wire);
            // A round is placed back along the pose's velocity from where it
            // is drawn, which an orbit bends away from by a little.
            check_barrels(&host, &drawn, &config, 0.05, 2.);
            let wired = agreement(&host, &drawn, &config);
            let old = agreement(&host, &run(&old_wire), &config);
            eprintln!(
                "other, spread only: {tolerance:?}\nother, wire train: {wired:?}\nother, 1/127 train: {old:?}"
            );
            assert!(
                within(&wired, &tolerance, 5.),
                "{wired:?} against {tolerance:?}"
            );
            assert!(
                !within(&old, &tolerance, 5.),
                "{old:?} against {tolerance:?}"
            );
        }
    }

    /// A surface with one ZSU-23 (the table's Shilka on the synthetic
    /// cannon's record) at the middle of the map, heading north.
    fn shilka() -> (tore_world::surface::Surface, u32) {
        use tore_world::surface::{
            UnitId,
            fire::{Arms, MountArc, WeaponArms},
        };
        let unit = UnitId(tore_world::surface::SURFACE_UNIT_BASE + 4);
        let mut record = gun((10, 1, 8));
        record.source = "ZSU23.JT".into();
        let arc = MountArc {
            index: 0,
            rest: [0., 0.],
            limit: [0., 0.],
            offset: [0., 6., 2.],
            barrel: None,
        };
        let (weapon, stock) =
            WeaponArms::gun(record, "ZSU23", arc, 2, false, [4, 4, 4, 4], 0).expect("a table row");
        let mut surface = tore_world::surface::Surface::default();
        surface.arsenal.units.push(Arms {
            unit,
            position: [10_000., 100., 10_000.],
            heading: 0.,
            skill: 2,
            side: tore_sim::combat::live::Side(2),
            react: 0,
            search_limit: None,
            ship: false,
            weapons: vec![weapon],
            radar: None,
            battery: None,
            loads: vec![stock],
            npc: [4, 4, 4, 4],
            team: false,
        });
        (surface, unit.0)
    }

    /// Protocol 22: a surface unit's burst is remade from its event alone,
    /// owned by the unit, leaving its mount on the burst's schedule along
    /// the first round's line, turning with the target as the picture draws
    /// it, each round's first picture showing it at the muzzle, and the
    /// rounds past an early end taken back.
    #[test]
    fn a_surface_burst_is_remade_from_its_schedule_and_cut_short() {
        let (surface, unit) = shilka();
        let flat = |_: f64, _: f64| 0.;
        let no_stations = |_: AircraftId| -> &[Station] { &[] };
        let around = Around {
            ground: &flat,
            stations: &no_stations,
            surface: Some(&surface),
        };
        // The target: the seat's own jet, crossing east 4,000 ft north and
        // 3,000 ft up.
        let target = |tick: u64| AircraftPose {
            id: 0,
            aircraft: Some(AircraftId::F18),
            position: [10_000. - 2_000. + tick as f64 * 5., 3_100., 14_000.],
            velocity: [600., 0., 0.],
            ..AircraftPose::default()
        };
        // Azimuth 0 (north), elevation 0.6 rad, 2^-16 of a turn.
        let aim = [0, (0.6 / std::f64::consts::TAU * 65_536.).round() as u16];
        let mut guns = Guns::default();
        guns.note_surface(&ReceivedEvent {
            number: 1,
            tick: 100,
            event: WireEvent::SurfaceBurst {
                unit,
                mount: 0,
                target: Some(0),
                aim,
                rounds: 10,
                span: 60,
            },
        });
        let mut picture = RenderSnapshot::default();
        let mut directions = Vec::new();
        for now in 99..=130 {
            picture.tick = now;
            picture.player = target(now);
            let before: std::collections::BTreeSet<u32> =
                guns.others.iter().map(|o| o.serial).collect();
            guns.step_others_to(now, &around, &picture);
            for other in guns.others.iter().filter(|o| !before.contains(&o.serial)) {
                assert_eq!(other.shooter, unit, "owned by the unit");
                assert_eq!(other.release, now, "made at its release tick");
                let muzzle = [10_000., 106., 10_002.];
                for (at, want) in other.round.previous.iter().zip(muzzle) {
                    assert!((at - want).abs() < 1e-9, "at the muzzle");
                }
                directions.push(other.round.direction);
            }
        }
        // 10 rounds over 60 ticks: one every 6 ticks from 100, six by 130.
        let released: Vec<u64> = guns.others.iter().map(|o| o.release).collect();
        assert_eq!(released, [100, 106, 112, 118, 124, 130]);
        // The first leaves along the event's line (the gun's spread is a
        // quarter degree), the later ones turn east with the crossing jet.
        let first = directions[0];
        let along = [0f64.sin() * 0.6f64.cos(), 0.6f64.sin(), 0.6f64.cos()];
        let cos = (0..3).map(|i| first[i] * along[i]).sum::<f64>();
        assert!(cos > 0.5f64.to_radians().cos(), "{first:?}");
        assert!(directions[5][0] > first[0] + 0.01, "turned with the target");
        // The host ends the burst after 4 rounds: the rest are taken back
        // and no more are made.
        guns.note_surface(&ReceivedEvent {
            number: 2,
            tick: 100,
            event: WireEvent::SurfaceBurstEnd {
                unit,
                mount: 0,
                fired: 4,
            },
        });
        for now in 131..=170 {
            picture.tick = now;
            picture.player = target(now);
            guns.step_others_to(now, &around, &picture);
        }
        let released: Vec<u64> = guns.others.iter().map(|o| o.release).collect();
        assert!(released.iter().all(|r| *r <= 118), "{released:?}");
        assert_eq!(guns.surface_ordinals[&(unit, 0)], 4);
    }
}
