//! Seeker-owned observations. Hidden physical state is consulted only here;
//! guidance receives measured returns, never an unobserved target position.
use super::*;
use crate::combat::live::Target;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Heat {
    #[default]
    Unknown,
    Engine {
        on: bool,
        throttle: f64,
        afterburner: bool,
    },
}
impl Heat {
    pub fn factor(self) -> f64 {
        match self {
            Self::Unknown => 1.,
            Self::Engine { on: false, .. } => 0.1,
            Self::Engine {
                afterburner: true, ..
            } => 1.5,
            Self::Engine { throttle, .. } => 0.5 + 0.5 * throttle.clamp(0., 1.),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    pub id: u32,
    pub position: Vector,
    pub velocity: Vector,
    pub quality: f64,
    pub off_axis: f64,
    pub range: f64,
}
/// Shared fitted aircraft IR response. Aspect changes detection distance, not
/// the lock threshold: a visible head-on fighter can actually acquire.
pub fn infrared_quality(
    all_aspect: bool,
    nominal: f64,
    strength: f64,
    facing: f64,
    distance: f64,
) -> f64 {
    if nominal <= 0. || strength <= 0. || (!all_aspect && facing > -std::f64::consts::FRAC_1_SQRT_2)
    {
        return 0.;
    }
    let aspect_range = if all_aspect {
        0.7 - 0.3 * facing.clamp(-1., 1.)
    } else {
        1.
    };
    let reach = nominal * aspect_range * strength.sqrt();
    if distance > reach {
        return 0.;
    }
    strength / (1. + (distance / reach).powi(2))
}

pub fn heat_quality(w: &Weapon, target: &Target, observer: Vector, nominal: f64) -> f64 {
    let direction = unit(sub(observer, target.position));
    let facing = dot(target.basis.forward, direction).clamp(-1., 1.);
    let strength = target.signature.infrared.max(0.) / 100. * target.heat.factor();
    // This pass tunes the reviewed A2A inventory. Preserve the separate SAM
    // profiles' prior heat response until their aspect contracts are reviewed.
    if tore_formats::weapons::corrections::infrared_aspect(&w.source).is_none() {
        let aspect = if facing < 0. {
            0.5 - 0.5 * facing
        } else {
            0.5 - 0.25 * facing
        };
        let heat = target.signature.infrared.max(0.) / 100. * aspect * target.heat.factor();
        let distance = length(sub(target.position, observer));
        return if nominal > 0. && distance <= nominal * heat.sqrt().min(1.) {
            (heat / (1. + (distance / nominal).powi(2))).max(0.)
        } else {
            0.
        };
    }

    infrared_quality(
        w.infrared_all_aspect(),
        nominal,
        strength,
        facing,
        length(sub(target.position, observer)),
    )
}
pub struct View<'a> {
    pub position: Vector,
    pub basis: Basis,
    pub cap: Option<f64>,
    pub obscured: &'a dyn Fn(Vector, Vector) -> bool,
}
pub fn observe(
    w: &Weapon,
    profile: Profile,
    view: &View<'_>,
    target: &Target,
) -> Option<Observation> {
    observe_model(w, profile, view, target, false)
}
pub fn observe_energy(
    w: &Weapon,
    profile: Profile,
    view: &View<'_>,
    target: &Target,
) -> Option<Observation> {
    observe_model(w, profile, view, target, true)
}
/// Signal changes acquisition distance, rather than making a sufficiently
/// nearby residual-heat target impossible to see at every distance.
pub fn energy_infrared_quality(
    all_aspect: bool,
    nominal: f64,
    strength: f64,
    facing: f64,
    distance: f64,
) -> f64 {
    if nominal <= 0. || strength <= 0. || (!all_aspect && facing > -std::f64::consts::FRAC_1_SQRT_2)
    {
        return 0.;
    }
    let aspect = 0.7 - 0.3 * facing.clamp(-1., 1.);
    let reach = nominal * (strength * aspect).sqrt();
    if distance > reach {
        0.
    } else {
        1. / (1. + (distance / reach).powi(2))
    }
}
fn observe_model(
    w: &Weapon,
    profile: Profile,
    view: &View<'_>,
    target: &Target,
    energy: bool,
) -> Option<Observation> {
    let mut zone = w.seeker.zones[0];
    let nominal = f64::from(zone.maximum_range);
    if energy && profile.guidance == Guidance::Infrared && profile.role == TargetRole::Aircraft {
        let facing = dot(
            target.basis.forward,
            unit(sub(view.position, target.position)),
        );
        let strength = target.signature.infrared.max(0.) / 100.
            * target.heat.factor()
            * (0.7 - 0.3 * facing.clamp(-1., 1.));
        zone.maximum_range = (nominal * strength.sqrt()).ceil() as i32;
    }
    // Minimum launch distance is an employment gate, not a near-target blind
    // zone for the energy seeker. Historical Spec SAMs retain their old rule.
    if energy || !surface_launched(&w.source) {
        zone.minimum_range = 0;
    }
    // The decoded altitude band of a surface seeker zone is the site's
    // look-up limit (SA-2: targets at least 3,000 ft above the site), an
    // employment gate the launch band already applies. Measured from the
    // climbing missile it blinds the seeker as soon as the missile nears the
    // target's altitude, and the energy steering has no ground-track fallback,
    // so every moving target was missed.
    if energy {
        zone.minimum_altitude = i32::MIN;
        zone.maximum_altitude = i32::MAX;
    }
    if !profile.accepts(target)
        || !geometry(&zone, view.position, view.basis, target.position, view.cap)
        || (view.obscured)(view.position, target.position)
    {
        return None;
    }
    let range = length(sub(target.position, view.position));
    let quality = match profile.guidance {
        Guidance::Infrared if profile.role == TargetRole::Surface => {
            let contrast = target.signature.infrared.max(0.) / 100.;
            if nominal <= 0. || range > nominal * contrast.sqrt().min(1.) {
                return None;
            }
            contrast / (1. + (range / nominal).powi(2))
        }
        Guidance::Infrared if energy => {
            let facing = dot(
                target.basis.forward,
                unit(sub(view.position, target.position)),
            );
            energy_infrared_quality(
                super::energy_seeker::all_aspect(w),
                nominal,
                target.signature.infrared.max(0.) / 100. * target.heat.factor(),
                facing,
                range,
            )
        }
        Guidance::Infrared => heat_quality(w, target, view.position, nominal),
        Guidance::Active | Guidance::Supported => {
            let signature = target.signature.effective_radar(
                &target.basis,
                sub(view.position, target.position),
                target.configuration,
            );
            let range_limit = crate::sensors::detection::effective_range_ft(
                nominal,
                signature / 100.,
                1.,
                1.,
                1.,
            );
            if signature <= 0. || range > range_limit {
                return None;
            }
            // Fitted return strength: directional size and inverse-square range.
            (signature / 100. / (1. + (range / nominal).powi(2))).max(0.)
        }
        Guidance::Emitter => {
            let radar = profile.radar_emissions && target.radar_emitting;
            let jammer = profile.jammer_emissions
                && target.jammer_active
                && target.jammer.as_ref().is_some_and(|j| j.radio_frequency);
            if !radar && !jammer {
                return None;
            }
            1.
        }
        // The air-to-ground kinds have no profile: their guidance is
        // `live::guided`'s, never this seeker's.
        Guidance::Laser | Guidance::Coordinate | Guidance::Command => return None,
    };
    (quality > 0.).then_some(Observation {
        id: target.id,
        position: target.position,
        velocity: target.velocity,
        quality,
        off_axis: dot(
            unit(sub(target.position, view.position)),
            view.basis.forward,
        )
        .clamp(-1., 1.)
        .acos(),
        range,
    })
}
/// Centre returns receive up to four times the weight of edge returns.
pub fn centre_weight(off_axis: f64, cap: f64) -> f64 {
    1. - 0.75 * (off_axis / cap).clamp(0., 1.).powi(2)
}
pub fn compare_returns(a: &Observation, b: &Observation, profile: Profile) -> std::cmp::Ordering {
    let score = |o: &Observation| o.quality * centre_weight(o.off_axis, profile.search_cap());
    let signal = if matches!(profile.guidance, Guidance::Infrared | Guidance::Active) {
        score(b).total_cmp(&score(a))
    } else {
        std::cmp::Ordering::Equal
    };
    signal
        .then(a.off_axis.total_cmp(&b.off_axis))
        .then(a.range.total_cmp(&b.range))
        .then(a.id.cmp(&b.id))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    Unguided,
    Midcourse,
    #[default]
    Search,
    Acquiring,
    Locked,
    Pitbull,
    Memory,
    Lost,
    Expired,
}
impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unguided => "DUMB",
            Self::Midcourse => "MIDCOURSE",
            Self::Search => "ACTIVE SEARCH",
            Self::Acquiring => "ACQUIRING",
            Self::Locked => "LOCK",
            Self::Pitbull => "PITBULL",
            Self::Memory => "MEMORY",
            Self::Lost => "LOST",
            Self::Expired => "EXPIRED",
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seeker {
    pub target: Option<u32>,
    pub candidate: Option<u32>,
    pub dwell: u32,
    pub missing: u32,
    pub acquired: bool,
    pub status: Status,
    pub quality: f64,
    pub observation: Option<Observation>,
}
impl Seeker {
    pub fn new(target: Option<u32>) -> Self {
        Self {
            target,
            ..Self::default()
        }
    }
    /// Caller supplies only eligible seeker returns. Acquired identity is never
    /// released on signal loss, including after the memory indication times out.
    pub fn step(&mut self, profile: Profile, observations: &[Observation]) {
        let retaining = matches!(self.status, Status::Locked | Status::Pitbull);
        let threshold = if profile.guidance == Guidance::Infrared {
            if retaining { 0.20 } else { 0.25 }
        } else {
            0.
        };
        let best = observations
            .iter()
            .filter(|o| self.target.is_none_or(|id| o.id == id) && o.quality >= threshold)
            .min_by(|a, b| compare_returns(a, b, profile))
            .copied();
        self.observation = best;
        self.quality = best.map_or(0., |o| o.quality);
        if let Some(o) = best {
            if self.candidate != Some(o.id) {
                self.candidate = Some(o.id);
                self.dwell = 0;
            }
            self.dwell = self.dwell.saturating_add(1);
            if self.dwell >= DWELL {
                self.target = Some(o.id);
                self.acquired = true;
                self.missing = 0;
                self.status = if profile.guidance == Guidance::Active {
                    Status::Pitbull
                } else {
                    Status::Locked
                };
                return;
            }
        } else {
            self.candidate = None;
            self.dwell = 0;
        }
        if self.acquired {
            self.missing = self.missing.saturating_add(1);
            self.status = if self.missing < profile.memory_ticks {
                Status::Memory
            } else {
                Status::Lost
            };
        } else {
            self.status = if best.is_some() {
                Status::Acquiring
            } else {
                Status::Search
            };
        }
    }
    pub fn tone(&self) -> f64 {
        let quality = self.quality.clamp(0., 1.);
        if matches!(self.status, Status::Memory | Status::Lost | Status::Expired) {
            0.15
        } else if matches!(self.status, Status::Locked) {
            0.4 + 0.6 * quality
        } else {
            0.15 + 0.55 * quality
        }
    }
}

// Exact checkpoints (docs/formats/checkpoint.md).
#[path = "seeker_checkpoint.rs"]
mod checkpoint;
