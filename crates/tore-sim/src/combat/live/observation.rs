//! Independent ownship observation work. No shared projectile, event, random
//! stream or live peer state is available through this context.
use super::*;

pub(super) struct Context<'a, G> {
    /// The state's read-only facts for the ground pages: type facts, airfield
    /// sequences and weather. Never its projectiles or random stream.
    pub facts: &'a super::ground_sensing::GroundFacts<'a>,
    pub targets: &'a [Target],
    pub rows: &'a [OwnRow],
    pub weapon_rules: Rules,
    pub easy_targeting: bool,
    pub tick: u64,
    pub ground: &'a G,
}

pub(super) fn observe_ownship(
    own: &mut Ownship,
    own_index: usize,
    launcher: Launcher,
    context: &Context<'_, impl Fn(f64, f64) -> f64 + Sync>,
) {
    let obscured = |from: Vector, to: Vector| terrain_hit(from, to, context.ground).is_some();
    // Shared observations are produced before this tick's firing decision,
    // so the scope, the target view and weapon support all agree.
    own.sensors.controls = launcher.controls;
    let observer = Observer {
        position: launcher.position,
        basis: launcher.basis,
        radar_powered: launcher.radar && launcher.alive,
        radar_failed: own.radar_failed,
        infrared_failed: own.infrared_failed || !launcher.alive,
        visual_failed: own.visual_failed || !launcher.alive,
    };
    let height = |x: f64, z: f64| (context.ground)(x, z);
    let environment = sensors::Environment {
        ground: &height,
        obscured: &obscured,
    };
    // A hidden shoulder-fired team reaches this pilot's sensors, pages and
    // map only once spotted (T.O.R.E opinionated, John 2026-10-29).
    super::ground_sensing::spot_teams(own, &observer, context.targets, context.facts, &environment);
    let unspotted = super::ground_sensing::unspotted_teams(own, context.targets, context.facts);
    let shown = |t: &&Target| unspotted.binary_search(&t.id).is_err();
    let observables: Vec<Observable> = context
        .targets
        .iter()
        .filter(shown)
        .chain(peers(context.rows, own_index))
        .map(observable_of)
        .collect();
    let easy_targeting = own.easy_targeting(context.easy_targeting);
    own.sensors.keep_selection = easy_targeting;
    let in_view = own.designated().or(own.sight_hold);
    own.sensors
        .set_harm_seeker(super::ground_sensing::harm_seeker(own));
    own.sensors.step(&observer, &observables, &environment);
    // The ground pages, and a ground selection's keeping, read the rows only
    // when a page lists ground objects or the selection is not an air contact.
    let ground_selection = own
        .sensors
        .selected()
        .is_some_and(|id| own.sensors.contact(id).is_none());
    let objects: Vec<sensors::GroundObject> =
        if own.sensors.ground_page().is_some() || ground_selection {
            context
                .targets
                .iter()
                .filter(shown)
                .chain(peers(context.rows, own_index))
                .map(|t| context.facts.ground_object(t))
                .collect()
        } else {
            Vec::new()
        };
    own.sensors.step_ground(
        &ground_observer(own, launcher, &observer),
        &objects,
        &environment,
        context.facts.weather,
    );
    if let Some(id) = own.designated() {
        own.hud_selection = Some(id);
    } else if !easy_targeting {
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
        context.targets.iter().any(|t| {
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
    let surface_obscured =
        |from: Vector, to: Vector| !missiles::launch_line_clear(from, to, context.ground);
    own.harm_cue = super::agm::harm_emitters(
        own,
        &launcher,
        context.targets.iter().chain(peers(context.rows, own_index)),
        &surface_obscured,
    );
    let index = own.selected;
    let Some(station) = own.config.stations.get(index) else {
        own.mounted = Seeker::default();
        own.range_estimate = None;
        return;
    };
    let w = &station.weapon;
    if let Some(profile) =
        missiles::Profile::for_weapon(w).filter(|_| context.weapon_rules.uses_seeker())
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
        // A designation turns an infrared missile, and an anti-radiation
        // surface missile (the HARM cue's CUED), to CUED.
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
            let seeker_obscured = |from: Vector, to: Vector| {
                if profile.role == TargetRole::Surface {
                    surface_obscured(from, to)
                } else {
                    obscured(from, to)
                }
            };
            let view = seeker::View {
                position: launcher.position,
                basis: launcher.basis,
                cap,
                obscured: &seeker_obscured,
            };
            let observations: Vec<_> = context
                .targets
                .iter()
                .chain(peers(context.rows, own_index))
                .filter(|t| t.body_present())
                .filter(|t| bore || assigned == Some(t.id))
                .filter_map(|t| {
                    if context.weapon_rules == Rules::Energy {
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
                    // The HARM cue's candidate: painting first, then nearest
                    // the bore, inside the launch cone.
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
                            context.targets.iter().find(|t| t.id == o.id),
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
                // The mounted HARM seeker settles on the cue's candidate.
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
    if context.weapon_rules == Rules::Energy {
        return;
    }
    if let Some(o) = weapon_observation(own, launcher, context.weapon_rules, |id| {
        context
            .targets
            .iter()
            .chain(peers(context.rows, own_index))
            .find(|target| target.id == id)
    }) {
        if context.tick.is_multiple_of(60)
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

/// The observer the ground pages scan from. On the AC-130 the radar's scan
/// is centred on the gunsight's line of sight rather than the nose: the
/// gunship's radar works with its sensors toward the orbit's centre, so the
/// A-G page lists what the sensor turret looks at (agent decision, slice
/// nr-conb, 2026-10-14, for John's sensor console radar: "cycle targets in
/// line of view of the sensor"). Every other rule of the page is unchanged.
/// Other aircraft scan about the nose.
pub(super) fn ground_observer(own: &Ownship, launcher: Launcher, observer: &Observer) -> Observer {
    let Some(group) = &own.gunship else {
        return *observer;
    };
    let line = crate::combat::gunship::direction(launcher, group.look[0], group.look[1]);
    Observer {
        basis: crate::attitude::Basis::new(
            line[0].atan2(line[2]),
            line[1].atan2(line[0].hypot(line[2])),
            0.,
        ),
        ..*observer
    }
}
