//! Test-only pre-extraction observation reference, copied from c1d1e736.
//! Keep it independent of observation::observe_ownship and weapon_observation:
//! agreement between serial and worker executions alone cannot validate that
//! their shared extraction preserved the previous behavior.
use super::observation::ground_observer;
use super::*;

pub(super) fn observe(
    state: &State,
    own: &mut Ownship,
    k: usize,
    launcher: Launcher,
    rows: &[OwnRow],
    ground: &impl Fn(f64, f64) -> f64,
) {
    let obscured = |from: Vector, to: Vector| terrain_hit(from, to, ground).is_some();
    // Shared observations are produced before this tick's firing decision,
    // so the scope, the target view and weapon support all agree.
    own.sensors.controls = launcher.controls;
    let observables: Vec<Observable> = state
        .targets
        .iter()
        .chain(peers(rows, k))
        .map(observable_of)
        .collect();
    let observer = Observer {
        position: launcher.position,
        basis: launcher.basis,
        radar_powered: launcher.radar && launcher.alive,
        radar_failed: own.radar_failed,
        infrared_failed: own.infrared_failed || !launcher.alive,
        visual_failed: own.visual_failed || !launcher.alive,
    };
    let height = |x: f64, z: f64| ground(x, z);
    let environment = sensors::Environment {
        ground: &height,
        obscured: &obscured,
    };
    own.sensors.keep_selection = state.cheats.easy_targeting;
    let in_view = own.designated().or(own.sight_hold);
    // Added with the ground pages (slice G1), after the copy was taken.
    own.sensors
        .set_harm_seeker(super::ground_sensing::harm_seeker(own));
    own.sensors.step(&observer, &observables, &environment);
    let ground_selection = own
        .sensors
        .selected()
        .is_some_and(|id| own.sensors.contact(id).is_none());
    let facts = state.ground_facts();
    let objects: Vec<sensors::GroundObject> =
        if own.sensors.ground_page().is_some() || ground_selection {
            state
                .targets
                .iter()
                .chain(peers(rows, k))
                .map(|t| facts.ground_object(t))
                .collect()
        } else {
            Vec::new()
        };
    own.sensors.step_ground(
        &ground_observer(own, launcher, &observer),
        &objects,
        &environment,
        facts.weather,
    );
    if let Some(id) = own.designated() {
        own.hud_selection = Some(id);
    } else if !state.cheats.easy_targeting {
        // A dropped target is gone; nothing is remembered for later.
        own.hud_selection = None;
    }
    // The views keep a dropped target while it is within visual range, in
    // any direction (John, 2026-09-28); beyond it the target is gone for good.
    let visual_range = own
        .sensors
        .profiles
        .visual
        .as_ref()
        .filter(|_| !observer.visual_failed)
        .map_or(0., |visual| visual.search.maximum_ft);
    own.sight_hold = own.designated().or(in_view.filter(|id| {
        state.targets.iter().any(|t| {
            t.id == *id
                && t.body_present()
                && (0..3)
                    .map(|i| (t.position[i] - launcher.position[i]).powi(2))
                    .sum::<f64>()
                    .sqrt()
                    <= visual_range
        })
    }));
    own.emitters = passive::emitters(
        &observer,
        &observables,
        own.sensors.contacts(),
        &environment,
    );
    own.bore_observation = None;
    // Added with the air-to-ground missiles (slice W2), after the copy was
    // taken: the HARM cue, the HARM and Maverick BORE and the A-G track.
    own.harm_cue = super::agm::harm_emitters(
        own,
        &launcher,
        state.targets.iter().chain(peers(rows, k)),
        &obscured,
    );
    let index = own.selected;
    let w = &own.config.stations[index].weapon;
    if let Some(profile) =
        missiles::Profile::for_weapon(w).filter(|_| state.weapon_rules.uses_seeker())
    {
        if !profile.guidance_available(launcher.radar_power) || !profile.supports_boresight() {
            own.cockpit.launch_mode = LaunchMode::Cued;
        }
        if profile.guidance_available(launcher.radar_power)
            && own.armed
            && own.designated().is_none()
            && profile.supports_boresight()
        {
            own.cockpit.launch_mode = LaunchMode::Boresight;
        }
        if (profile.guidance == Guidance::Infrared || profile.holds_on_angles())
            && own.designated().is_some()
        {
            own.cockpit.launch_mode = LaunchMode::Cued;
        }
        let assigned = if own.launch_mode == LaunchMode::Cued {
            own.designated()
        } else {
            None
        };
        let key = (index, own.launch_mode, assigned);
        if own.mounted_key != Some(key) {
            own.mounted = Seeker::new(assigned);
            own.mounted_key = Some(key);
        }
        if !profile.guidance_available(launcher.radar_power) {
            own.mounted = Seeker {
                status: Status::Unguided,
                ..Default::default()
            };
            own.mounted_key = None;
        }
        if own.armed
            && profile.guidance_available(launcher.radar_power)
            && launcher.alive
            && own.hp > 0
            && own.rounds(index) > 0
            && own.ammo[index] & 0x8000 == 0
        {
            let bore = own.launch_mode == LaunchMode::Boresight;
            let cap = bore.then(|| profile.search_cap());
            // Mounted IR may choose a stronger return. Released missiles keep identity.
            if bore && (profile.guidance == Guidance::Infrared || profile.holds_on_angles()) {
                own.mounted.target = None;
                own.mounted.acquired = false;
                own.mounted.missing = 0;
            }
            let view = seeker::View {
                position: launcher.position,
                basis: launcher.basis,
                cap,
                obscured: &obscured,
            };
            let observations: Vec<_> = state
                .targets
                .iter()
                .chain(peers(rows, k))
                .filter(|t| t.body_present())
                .filter(|t| bore || assigned == Some(t.id))
                .filter_map(|t| {
                    if state.weapon_rules == Rules::Energy {
                        seeker::observe_energy(w, profile, &view, t)
                    } else {
                        seeker::observe(w, profile, &view, t)
                    }
                })
                .filter(|o| {
                    !bore
                        || profile.guidance != Guidance::Active
                        || own.config.sensors.radar.as_ref().is_some_and(|r| {
                            o.range <= launcher.controls.range_nmi() * missiles::NMI
                                && o.range <= r.track.maximum_ft
                        })
                })
                .collect();
            if bore {
                own.bore_observation = if profile.holds_on_angles() {
                    super::agm::harm_candidate(&observations, &own.painted_by, w, &launcher)
                } else {
                    observations
                        .iter()
                        .min_by(|a, b| seeker::compare_returns(a, b, profile))
                        .copied()
                };
            }
            if bore && profile.guidance == Guidance::Active {
                // The HUD estimate never pre-locks or assigns an active-radar shot.
                own.mounted = Seeker::default();
            } else if own.launch_mode == LaunchMode::Cued
                && matches!(profile.guidance, Guidance::Active | Guidance::Supported)
            {
                let supported: Vec<_> = observations
                    .into_iter()
                    .filter(|o| {
                        super::agm::ownship_supports(
                            own,
                            Some(profile),
                            w,
                            &launcher,
                            o.id,
                            state.targets.iter().find(|t| t.id == o.id),
                        )
                    })
                    .collect();
                own.mounted.step(
                    missiles::Profile {
                        guidance: Guidance::Supported,
                        ..profile
                    },
                    &supported,
                );
            } else if bore && profile.holds_on_angles() {
                let candidate: Vec<_> = own.bore_observation.into_iter().collect();
                own.mounted.step(profile, &candidate);
            } else {
                own.mounted.step(profile, &observations);
            }
        } else {
            own.mounted = Seeker::new(assigned);
        }
    } else {
        own.bore_observation = None;
        own.mounted = Seeker::default();
        own.mounted_key = None;
    }
    if let Some(o) = original_weapon_observation(state, own, launcher) {
        if state.tick.is_multiple_of(60)
            || own
                .range_estimate
                .is_none_or(|e| e.station != index || e.target != o.id || e.mode != own.launch_mode)
        {
            own.range_estimate = missiles::Profile::for_weapon(w).map(|profile| {
                let maximum = missiles::maximum_range(
                    w,
                    launcher.position,
                    launcher.basis.forward,
                    launcher.velocity,
                    o.position,
                    o.velocity,
                    profile.guidance_ticks,
                );
                RangeEstimate {
                    station: index,
                    target: o.id,
                    mode: own.launch_mode,
                    minimum: f64::from(w.seeker.zones[1].minimum_range),
                    maximum,
                    favorable: missiles::firing_band(
                        w,
                        launcher.position,
                        launcher.basis,
                        launcher.velocity,
                        o,
                        maximum,
                        profile.guidance_ticks,
                        own.launch_mode == LaunchMode::Boresight,
                    ),
                }
            });
        }
    } else {
        own.range_estimate = None;
    }
}

fn original_weapon_observation(
    state: &State,
    own: &Ownship,
    launcher: Launcher,
) -> Option<seeker::Observation> {
    if !own.armed || (state.weapon_rules.uses_seeker() && !own.guidance_available(launcher)) {
        return None;
    }
    if own.launch_mode == LaunchMode::Boresight {
        let w = &own.config.stations[own.selected].weapon;
        let profile = missiles::Profile::for_weapon(w)?;
        return own.bore_observation.filter(|o| {
            profile.guidance != Guidance::Infrared
                || (missiles::geometry(
                    &missiles::launch_geometry(w),
                    launcher.position,
                    launcher.basis,
                    o.position,
                    None,
                ) && missiles::intercept(
                    &w.movement,
                    Motion::launch(w, launcher.velocity, launcher.position[1]),
                    launcher.position,
                    launcher.basis.forward,
                    o.position,
                    o.velocity,
                    0,
                    profile.guidance_ticks,
                )
                .is_some())
        });
    }
    own.mounted.observation.or_else(|| {
        let id = own.designated()?;
        let w = &own.config.stations[own.selected].weapon;
        if state.weapon_rules.uses_seeker()
            && missiles::Profile::for_weapon(w)
                .is_some_and(|p| view(state, own).contact(id).is_none_or(|t| !p.accepts(t)))
        {
            return None;
        }
        let contact = own.sensors.observation(id)?;
        let delta = missiles::sub(contact.position, launcher.position);
        Some(seeker::Observation {
            id: contact.id,
            position: contact.position,
            velocity: contact.velocity,
            quality: 1.,
            range: missiles::length(delta),
            off_axis: dot(unit(delta), launcher.basis.forward)
                .clamp(-1., 1.)
                .acos(),
        })
    })
}
