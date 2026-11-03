//! Building a mission: [`World::new`] turns a [`MissionSpec`] and the imported
//! resources into a running [`World`], with no window, screen or environment
//! variable in the way. The game's creator builds through it, and so will a
//! dedicated server. See docs/ARCHITECTURE.md, "A mission with no window".

use super::{AiSetup, Cockpit, Restarted, Setup, World};
use crate::{
    WorldResult, ai_wings,
    aircraft_type::{AircraftType, load_type},
    combat::Combat,
    comms, crew_voice,
    mission::{MissionSpec, Start},
    mission_layout,
    resources::ResourceSource,
    seats::{PlaneId, Roster},
};
use std::sync::Arc;
use tore_formats::{aircraft::AircraftId, weapons::Weapon};
use tore_sim::combat::loadout::Loadout;

/// Who flies what when the mission is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seating {
    /// Single player: seat 0 flies plane 0, the lead of friendly wing 1, with
    /// the loadout the spec gives it; the AI flies every other plane.
    SinglePlayer,
    /// A networked mission: the AI flies every plane, plane 0 included, with
    /// its standard stores, and there is no seat. Humans take planes and give
    /// them back by handoff ([`World::take_plane`], [`World::give_back_plane`])
    /// at any time, and the mission steps with no human at all. Every AI
    /// aircraft flies the hybrid model.
    Open,
}

/// What a caller can supply while a mission is built. The default supplies
/// nothing: every aircraft type loads through [`load_type`] and weapons keep
/// the names they are read with.
#[derive(Default)]
pub struct Hooks<'a> {
    /// Diagnostic probes may inspect retained invalid placements. Never enabled
    /// by the creator, server, lobby or a normal flight.
    pub inspect_invalid_placement: bool,
    /// The player's aircraft type, when the caller has loaded it already (the
    /// game loads it with its drawn half).
    pub player: Option<Arc<AircraftType>>,
    /// Loads each other aircraft type the mission flies, the first time it is
    /// needed. The game loads the drawn model beside the type and keeps it.
    pub load: Option<&'a mut dyn FnMut(AircraftId) -> WorldResult<Arc<AircraftType>>>,
    /// Tidies a weapon's display name as the game's loadout screen does, for
    /// every weapon of the player's load.
    pub weapon_label: Option<&'a dyn Fn(&mut Weapon)>,
    /// Jitter and relocation of the ground target, both on in a mission
    /// (`None`); development tools turn relocation off to fly the retail spot.
    pub ground_variation: Option<crate::surface::layout::Variation>,
}

/// A mission just built: the world, and what starting it reported.
pub struct Built {
    pub world: World,
    pub restarted: Restarted,
}

impl World {
    /// Builds the mission `spec` describes from the imported `resources`: the
    /// terrain for the theater and weather, the aircraft types, the layout,
    /// combat with the player's loadout and the other aircraft, the AI wings,
    /// and the start. The world is ready to step.
    pub fn new(
        spec: &MissionSpec,
        resources: &dyn ResourceSource,
        seating: Seating,
    ) -> WorldResult<World> {
        Ok(Self::build(spec, resources, seating, &mut Hooks::default())?.world)
    }

    /// [`World::new`], with the caller's [`Hooks`], returning what the start
    /// reported too. A start runs once, here: a caller that goes on to call
    /// [`World::restart`] starts the mission again.
    pub fn build(
        spec: &MissionSpec,
        resources: &dyn ResourceSource,
        seating: Seating,
        hooks: &mut Hooks<'_>,
    ) -> WorldResult<Built> {
        let open = seating == Seating::Open;
        spec.validate()?;
        if open {
            check_open(spec)?;
        } else if !spec.plane_loadouts.is_empty() {
            return Err("Plane loadouts are a multiplayer mission's: single player loads plane 0 with `loadout`.".into());
        } else if !spec.friendly_fire || spec.cheat_loadouts {
            // Stage F phase 2's settings: single player's spec never carries
            // them, so its build is unchanged.
            return Err(
                "`friendly-fire off` and `loadouts any` are a multiplayer mission's settings."
                    .into(),
            );
        }
        // The ground target's template joins the theater's own surface units.
        let mut target = crate::surface::resolve::GroundTarget::from_spec(spec);
        if let (Some(target), Some(variation)) = (target.as_mut(), hooks.ground_variation) {
            target.variation = variation;
        }
        // With no target, the creator's strengths scatter the enemy's area
        // defenses (B23).
        let area = crate::surface::resolve::AreaDefense::from_spec(spec);
        let terrain = crate::terrain::Terrain::for_mission_surface(
            resources,
            &spec.theater,
            Some(spec.condition.index()),
            &spec.weather,
            target.as_ref(),
            area.as_ref(),
        )?;
        if let Some(problem) = mission_layout::availability::surface_problem(
            &terrain.surface,
            spec.separation_nm,
            matches!(spec.start, Start::Airborne { .. }),
            hooks.inspect_invalid_placement,
        ) {
            return Err(problem.into());
        }
        let player = match hooks.player.clone() {
            Some(player) => player,
            None => load_type(resources, spec.player())?,
        };

        // The player's load: the spec's, or the aircraft's standard one.
        let label = hooks.weapon_label;
        let standard = Loadout::new(&player.profile, |name| {
            resources
                .get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing loadout resource {name}")))
        })?;
        let load = match &spec.loadout {
            Some(load) => load.apply(standard, resources, label)?,
            None => {
                let mut load = standard;
                if let Some(label) = label {
                    for station in &mut load.configuration.stations {
                        label(&mut station.weapon);
                    }
                }
                if spec.guns_only {
                    load.restrict_to_guns();
                }
                load
            }
        };
        if spec.guns_only
            && load
                .configuration
                .stations
                .iter()
                .zip(&load.quantities)
                .any(|(s, n)| !load.aircraft.mounts_gun(&s.weapon.source) && *n > 0)
        {
            return Err("Guns only is selected. Unload other weapons or return to setup and change the restriction.".into());
        }

        let altitude = f64::from(spec.start.altitude_ft());
        let selected_ground = match spec.start {
            Start::GroundAuto { .. } => Some(mission_layout::auto_runway(
                &terrain,
                if spec.fixture_wings {
                    1
                } else {
                    spec.player_wing_size()
                },
            )?),
            _ => spec.ground_runway(),
        };
        if selected_ground.is_some() && !spec.researched_flight {
            return Err("Ground start requires the researched flight model. Choose Airborne for this adapter.".into());
        }
        // Enemy Wing 1's ground start (slice CL-R): a Redfor field's runway.
        let enemy_ground =
            mission_layout::enemy_runway(&terrain, spec.enemy_start, spec.enemy_wing_size())?;
        if enemy_ground.is_some() && enemy_ground == selected_ground {
            return Err(
                "The enemy cannot start on the player's runway. Choose another airfield.".into(),
            );
        }
        let wings = if open {
            spec.open_wing_launches()
        } else {
            spec.wing_launches()
        }
        .map_err(|error| error.to_string())?;
        // A ground start parks the player's whole wing on the runway; the
        // straight-flight fixtures keep only the player there.
        let parked = if spec.fixture_wings {
            1
        } else {
            spec.player_wing_size()
        };
        let mut start = player.start(&terrain);
        let ground_layout = match selected_ground {
            Some(object) => {
                start.enable_research(1)?;
                let layout = mission_layout::ground_layout(&terrain, object, parked)?;
                mission_layout::place_on_runway(&terrain, &mut start, &layout, 0)?;
                Some(layout)
            }
            None => None,
        };
        let enemy_layout = match enemy_ground {
            Some(object) => Some(
                mission_layout::ground_layout(&terrain, object, spec.enemy_wing_size())
                    .map_err(|error| format!("Enemy wing 1's ground start: {error}"))?,
            ),
            None => None,
        };
        let layout = mission_layout::MissionLayout::plan(
            &terrain,
            &start,
            ground_layout,
            &wings,
            spec.separation_feet(),
            spec.situation,
        )
        .with_enemy_ground(enemy_layout);
        if layout.enemy.shortened() {
            return Err(mission_layout::availability::distance_problem(spec.separation_nm).into());
        }
        let ground = f64::from(terrain.height(start.position[0] as f32, start.position[2] as f32));
        // Only aircraft that start in the air need the altitude to clear the
        // ground; parked wingmen do not.
        let airborne_wings = if spec.fixture_wings {
            spec.fixture_pairs().iter().any(|(_, count)| *count > 0)
        } else {
            wings.iter().any(|wing| {
                let parked = wing.wing.index == 0
                    && if wing.wing.side.is_enemy() {
                        layout.enemy_ground.is_some()
                    } else {
                        layout.ground.is_some()
                    };
                !wing.is_empty() && !parked
            })
        };
        if (selected_ground.is_none() || airborne_wings) && altitude < ground + 100. {
            return Err(format!(
                "Airborne altitude must exceed {:.0} feet here. Choose a higher altitude.",
                ground + 100.
            )
            .into());
        }

        // Combat, with the other aircraft of the mission.
        let fuel = load.fuel_lbs;
        let mut combat = if open {
            Combat::open()
        } else {
            Combat::with_loadout(&player, &load)?
        };
        combat.add_scene_targets(&terrain)?;
        // The King's friendly fire (stage F phase 2): kept across every
        // restart of combat.
        combat.state.set_energy_seed(u64::from(spec.surface_seed));
        if !spec.friendly_fire {
            combat.state.friendly_fire = tore_sim::combat::live::FriendlyFire::Off;
        }
        let mut default_load = |id| load_type(resources, id);
        let loader: &mut dyn FnMut(AircraftId) -> WorldResult<Arc<AircraftType>> =
            match hooks.load.as_deref_mut() {
                Some(load) => load,
                None => &mut default_load,
            };
        if spec.fixture_wings {
            combat.mission_layout = Some(layout.clone());
            combat.mission_dummies(
                &spec.fixture_pairs(),
                layout.enemy.distance_ft,
                resources,
                loader,
            )?;
        } else {
            combat.mission_aircraft(&wings, &layout, resources, loader)?;
        }
        // The players' loadouts of an open mission, checked against the
        // aircraft each plane flies (the types are loaded by now), under the
        // mission's loadout rule.
        let mut loadouts = std::collections::BTreeMap::new();
        if !open && spec.share_flight_loadout {
            let planes = spec.open_planes();
            let player_wing = planes[0].wing;
            for plane in planes {
                if plane.id != 0 && plane.wing == player_wing && plane.aircraft == player.profile.id
                {
                    loadouts.insert(plane.id, load.clone());
                }
            }
        }
        for (plane, load) in &spec.plane_loadouts {
            let aircraft = spec
                .open_planes()
                .into_iter()
                .find(|p| p.id == *plane)
                .ok_or_else(|| format!("The mission has no plane {plane} to load."))?
                .aircraft;
            let kind = combat
                .dummy_types()
                .iter()
                .find(|kind| kind.profile.id == aircraft)
                .ok_or_else(|| format!("The mission holds no aircraft type for plane {plane}."))?;
            let checked = load
                .check_in(&kind.profile, resources, spec)
                .map_err(|error| format!("Plane {plane}'s loadout: {error}"))?;
            loadouts.insert(*plane, checked);
        }

        let setup = Setup {
            mission: Some((altitude, fuel)),
            ground_start: selected_ground,
            enemy_ground_start: enemy_ground,
            researched_flight: spec.researched_flight,
            native_tables: None,
            ai: (!spec.fixture_wings).then_some(AiSetup {
                wings,
                guns_only: spec.guns_only,
                preset: spec.preset,
                flight_model: spec.ai_flight_model,
                group_objectives: spec.objectives,
                group_must_survive: spec.must_survive,
                ground_target: spec.ground_target.is_some(),
                loadouts,
            }),
        };

        // Single player's first cockpit is a free start that `restart`
        // places; an open mission starts with none.
        let (roster, cockpits, comms) = if open {
            (
                Roster::open([]),
                Vec::new(),
                comms::Comms::with_seats(1, []),
            )
        } else {
            let flight = player.start(&terrain);
            let airport_service = tore_sim::airport::Service::new(&terrain.airport_scene)
                .map_err(std::io::Error::other)?;
            (
                Roster::single_player(comms::crew(&player.profile), []),
                vec![Cockpit {
                    plane: PlaneId(0),
                    previous_flight: flight.clone(),
                    flight,
                    turbulence: Default::default(),
                    turbulence_rng: tore_formats::flight_model::clock_rng::NativeRng::seeded(1)?,
                    airport_service,
                    airport_nav_mode: false,
                    airfield_radio: Default::default(),
                    crew_voice: crew_voice::CrewVoice::new(&player.profile),
                    result: Default::default(),
                    overspeed_message_at: None,
                    edge_message_at: None,
                }],
                comms::Comms::new(1),
            )
        };
        let mut world = World {
            choices: if open {
                super::choices::Resources::load(resources, spec)
            } else {
                Default::default()
            },
            setup,
            roster,
            cockpits,
            terrain,
            combat,
            ai_wings: None,
            comms,
            wing_status: Default::default(),
            datalink: Default::default(),
            score: None,
            revival: Default::default(),
            radio: Default::default(),
            phrases: comms::phrases(resources),
        };
        let restarted = world.restart(&player, resources)?;
        // Cheats reach the mission as the Settings command does. The default
        // is not sent, so a setting the build made (the AI's guns only) stands.
        if spec.cheats != tore_sim::cheats::Cheats::default() {
            world.apply_mission_command(&super::MissionCommand::Settings(super::Settings {
                cheats: spec.cheats,
            }))?;
        }
        Ok(Built { world, restarted })
    }
}

/// What an open mission refuses: it needs the AI (no straight-flight
/// fixtures), nobody flies from the start (no player loadout: the players'
/// loadouts are by plane, `plane-loadout`), and a networked
/// mission flies the hybrid model for humans and for every AI aircraft
/// (John, 2026-09-28). Agent decision (D3c): refused rather than overridden,
/// so a mission file says what it flies.
fn check_open(spec: &MissionSpec) -> WorldResult<()> {
    if spec.fixture_wings {
        return Err(
            "An open mission needs the AI: straight-flight fixture wings cannot fly it.".into(),
        );
    }
    if spec.loadout.is_some() {
        return Err("An open mission has no player loadout: every plane starts on the AI with its standard stores.".into());
    }
    if !spec.researched_flight {
        return Err("An open mission flies the hybrid model: `flight-model human legacy` is single player's.".into());
    }
    if spec.ai_flight_model != ai_wings::AiFlightModel::AllHybrid {
        return Err("An open mission flies every AI aircraft on the hybrid model: `flight-model ai standard` is single player's.".into());
    }
    Ok(())
}
