//! Offline physical envelopes and public-anchor calibration. Runtime never calls this search.
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::{fs, path::Path};
use tore_formats::weapons::Weapon;
use tore_sim::{
    attitude::{Basis, dot},
    combat::{
        live::{self, ActorSupport, Launcher, Projectile, Target},
        missiles::{self, energy::FT, energy_estimate::Trial, prediction},
    },
};
static OVERRIDES: OnceLock<std::collections::BTreeMap<String, f64>> = OnceLock::new();
static TRACE: AtomicBool = AtomicBool::new(false);
fn override_body(w: &Weapon, mut b: missiles::energy::Body) -> missiles::energy::Body {
    let Some(values) = OVERRIDES.get() else {
        return b;
    };
    let get = |key: &str, default: f64| values.get(key).copied().unwrap_or(default);
    b.drag_scale = get("drag", b.drag_scale);
    b.induced_factor = get("induced", b.induced_factor);
    b.navigation_gain = get("navigation", b.navigation_gain);
    b.lift_coefficient = get("lift", b.lift_coefficient);
    b.structural_g = get("g", b.structural_g);
    b.guidance_ticks = (get("control", f64::from(b.guidance_ticks) / 120.) * 120.).round() as u32;
    b.life = b.life.max(b.guidance_ticks + 1200);
    b.defeat_mach = get("defeat", b.defeat_mach);
    if ["fuel", "share", "boost_s", "sustain_s", "coefficient"]
        .iter()
        .any(|key| values.contains_key(*key))
    {
        let m = tore_formats::missile_motors::motor(&w.source).unwrap();
        let fuel = get("fuel", m.propellant_fraction);
        let share = get("share", m.boost_share);
        let coefficient = get("coefficient", m.exhaust_mps);
        b.boost.ticks = (get("boost_s", m.boost_seconds) * 120.).round() as u32;
        b.sustain.ticks = (get("sustain_s", m.sustain_seconds) * 120.).round() as u32;
        b.boost.propellant = b.mass * fuel * share;
        b.sustain.propellant = b.mass * fuel * (1. - share);
        for phase in [&mut b.boost, &mut b.sustain] {
            phase.thrust = if phase.ticks > 0 {
                phase.propellant * coefficient / (f64::from(phase.ticks) / 120.)
            } else {
                0.
            };
        }
        b.life = b
            .life
            .max(b.ignition + b.boost.ticks + b.sustain.ticks + 1200);
    }
    assert!(b.valid(), "invalid candidate override");
    b
}
fn trace_step(
    b: missiles::energy::Body,
    f: missiles::energy::Flight,
    target: Option<missiles::energy::Observation>,
    observed: bool,
    status: &str,
) {
    if !TRACE.load(Ordering::Relaxed) || !f.age.is_multiple_of(12) {
        return;
    }
    let mut probe = f;
    let forces = probe.step_diagnostics(b, [0.; 3], target);
    let speed = dot(f.velocity, f.velocity).sqrt();
    let error = target.map_or(0., |t| {
        let r = std::array::from_fn::<_, 3, _>(|i| t.position[i] - f.position[i]);
        (dot(r, f.velocity) / (dot(r, r).sqrt() * speed).max(1e-9))
            .clamp(-1., 1.)
            .acos()
            .to_degrees()
    });
    println!(
        "{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{},{},{},{}",
        f64::from(f.age) / 120.,
        f.position[0],
        f.position[1],
        f.position[2],
        f.velocity[0],
        f.velocity[1],
        f.velocity[2],
        forces.mach,
        forces.available_g,
        forces.required_g,
        forces.g_deficit,
        forces.thrust,
        forces.drag,
        error,
        f.controlled,
        f.lost,
        f.defeated,
        f.subsonic_ticks,
        observed,
        status
    );
}
fn question(w: &Weapon, c: Condition, range: f64) -> Trial {
    let speed = c.shooter;
    let launch_alt = c.launch_ft * FT;
    let target_alt = c.target_ft * FT;
    let delta = [0., target_alt - launch_alt, range];
    let pitch = delta[1].atan2(range);
    let l = Launcher {
        position: [0., launch_alt / FT, 0.],
        basis: Basis::new(0., pitch, 0.),
        velocity: [0., 0., speed / FT],
        speed_fps: speed / FT,
        bay_ready: true,
        radar_power: true,
        radar: true,
        jammer: false,
        alive: true,
        body_present: true,
        controls: Default::default(),
    };
    let angle = c.aspect.to_radians();
    let velocity = [c.speed * angle.sin(), 0., -c.speed * angle.cos()];
    let o = missiles::seeker::Observation {
        id: 1,
        position: [0., target_alt / FT, range / FT],
        velocity: velocity.map(|v| v / FT),
        range: range.hypot(target_alt - launch_alt) / FT,
        quality: 1.,
        off_axis: 0.,
    };
    let mut q = prediction::launch_question(
        w,
        l,
        o,
        if tore_sim::combat::guided::kind(w).is_some() {
            tore_sim::combat::guided::HIT_FT
        } else {
            (f64::from(w.damage.fuze_radius.max(0))) + 30.
        },
        [0.; 3],
    );
    q.body = override_body(w, q.body);
    q
}
fn seeker_run(w: &Weapon, c: Condition, range: f64, diagnostics: bool) -> Shot {
    let Some(profile) = missiles::Profile::for_weapon(w) else {
        return Shot::empty("held");
    };
    let q = question(w, c, range);
    let start = q.flight.position.map(|v| v / FT);
    let heading = std::f64::consts::PI - c.aspect.to_radians();
    let mut target = Target {
        aircraft: Some(tore_formats::aircraft::AircraftId::Mig29),
        role: profile.role,
        heat: missiles::seeker::Heat::Engine {
            on: true,
            throttle: 1.,
            afterburner: false,
        },
        radar_emitting: true,
        id: 1,
        position: q.target.position.map(|v| v / FT),
        velocity: q.target.velocity.map(|v| v / FT),
        basis: Basis::new(heading, 0., 0.),
        configuration: Default::default(),
        signature: Default::default(),
        jammer: None,
        jammer_active: false,
        airborne: true,
        on_ground: false,
        wreck: None,
        wreck_power: Default::default(),
        radius: 30.,
        hp: 100,
        initial_hp: 100,
        fragment_offsets: [[0.; 3]; 2],
        fragment_released: false,
        localized_damage: Default::default(),
        faults: Default::default(),
        category: 0x80,
        side: live::Side(2),
    };
    let observation = |t: &Target, p: [f64; 3]| missiles::seeker::Observation {
        id: t.id,
        position: t.position,
        velocity: t.velocity,
        quality: 1.,
        range: dot(
            std::array::from_fn::<_, 3, _>(|i| t.position[i] - p[i]),
            std::array::from_fn::<_, 3, _>(|i| t.position[i] - p[i]),
        )
        .sqrt(),
        off_axis: 0.,
    };
    if profile.guidance == missiles::Guidance::Infrared {
        let view = missiles::seeker::View {
            position: start,
            basis: Basis::new(
                0.,
                (q.target.position[1] - q.flight.position[1]).atan2(range),
                0.,
            ),
            cap: None,
            obscured: &|_, _| false,
        };
        if missiles::seeker::observe_energy(w, profile, &view, &target)
            .is_none_or(|o| o.quality < 0.25)
        {
            return Shot::empty("seeker_limit");
        }
    }
    let mut motion = missiles::Motion::launch_rules(
        w,
        [0., 0., c.shooter / FT],
        start[1],
        missiles::Rules::Energy,
    );
    let energy = motion.energy.as_mut().unwrap();
    energy.body = q.body;
    energy.flight = q.flight;
    let mut guidance = missiles::Flight::from_supported_launch(
        profile,
        missiles::LaunchMode::Cued,
        observation(&target, start),
        start,
    );
    if profile.guidance == missiles::Guidance::Infrared {
        guidance.seeker.acquired = true;
        guidance.seeker.dwell = missiles::DWELL;
    }
    let mut p = Projectile {
        id: 1,
        owner: 0,
        weapon: None,
        guidance: Some(guidance),
        motion: Some(motion),
        guidance_ticks: None,
        age: 0,
        incoming: None,
        station: 0,
        position: start,
        previous: start,
        direction: q.flight.axis,
        speed_f8: 0,
        launched_t: 0,
        target: Some(1),
        fall: Default::default(),
        gun_round: None,
        tracer: false,
    };
    let radius = q.radius / FT;
    let mut metrics = Metrics::default();
    let mut closest = f64::MAX;
    for age in 0..q.body.life {
        if diagnostics {
            metrics.sample(q.body, p.motion.unwrap().energy.unwrap().flight);
        }
        p.age = u64::from(age);
        p.previous = p.position;
        p.motion
            .as_mut()
            .unwrap()
            .prepare(p.position, p.direction, p.age, [0.; 3]);
        let support = ActorSupport {
            owner: 0,
            observation: Some(observation(&target, start)),
            supported: true,
            radar_position: start,
            radar_emitting: true,
        };
        live::guide_owned(
            &mut p,
            w,
            std::slice::from_ref(&target),
            std::iter::empty(),
            Some(&support),
            &|_, _| false,
            &|_, _| 0.,
        );
        if TRACE.load(Ordering::Relaxed) {
            let e = p.motion.unwrap().energy.unwrap();
            trace_step(
                e.body,
                e.flight,
                e.target,
                e.observed,
                &format!("{:?}", p.guidance.as_ref().unwrap().seeker.status),
            );
        }
        let before = std::array::from_fn::<_, 3, _>(|i| target.position[i] - p.position[i]);
        let m = p.motion.as_mut().unwrap();
        let delta = m.step_over_terrain(&w.movement, p.age, p.direction, &|_, _| 0.);
        for (i, d) in delta.into_iter().enumerate() {
            p.position[i] += d;
            target.position[i] += target.velocity[i] / 120.;
        }
        p.direction = m.energy.unwrap().flight.axis;
        let after = std::array::from_fn::<_, 3, _>(|i| target.position[i] - p.position[i]);
        let d = std::array::from_fn::<_, 3, _>(|i| after[i] - before[i]);
        let u = (-dot(before, d) / dot(d, d).max(1e-12)).clamp(0., 1.);
        let gap = std::array::from_fn::<_, 3, _>(|i| before[i] + d[i] * u);
        closest = closest.min(dot(gap, gap).sqrt() * FT);
        if dot(gap, gap) <= radius * radius
            && age >= q.arming_age
            && !m.energy.unwrap().flight.defeated
        {
            return Shot::finish(true, q.body, m.energy.unwrap().flight, closest, metrics);
        }
        if p.position[1] <= 0. || m.energy.unwrap().flight.lost {
            return Shot::finish(false, q.body, m.energy.unwrap().flight, closest, metrics);
        }
    }
    Shot::finish(
        false,
        q.body,
        p.motion.unwrap().energy.unwrap().flight,
        closest,
        metrics,
    )
}
#[derive(Clone, Copy)]
struct Condition {
    launch_ft: f64,
    target_ft: f64,
    aspect: f64,
    shooter: f64,
    speed: f64,
}
#[derive(Default, Clone, Copy)]
struct Metrics {
    supersonic: bool,
    powered_low: u32,
    low_after_supersonic: u32,
    min_after_super: Option<f64>,
}
impl Metrics {
    fn sample(&mut self, b: missiles::energy::Body, f: missiles::energy::Flight) {
        let mach =
            dot(f.velocity, f.velocity).sqrt() / missiles::energy::atmosphere(f.position[1]).sound;
        if mach >= 1. {
            self.supersonic = true;
        }
        if f.age >= b.ignition && f.age < b.ignition + b.boost.ticks + b.sustain.ticks {
            if mach < 1. {
                self.powered_low += 1;
            }
            if self.supersonic {
                self.min_after_super = Some(self.min_after_super.unwrap_or(mach).min(mach));
                if mach < 1. {
                    self.low_after_supersonic += 1;
                }
            }
        }
    }
}
#[derive(Clone)]
struct Shot {
    hit: bool,
    event: &'static str,
    seconds: f64,
    mach: Option<f64>,
    available_g: Option<f64>,
    closest: Option<f64>,
    ground: bool,
    defeated: bool,
    control_expired: bool,
    metrics: Metrics,
}
fn num(v: Option<f64>) -> String {
    v.filter(|x| x.is_finite())
        .map_or("null".into(), |x| format!("{x:.9}"))
}
impl Shot {
    fn empty(event: &'static str) -> Self {
        Self {
            hit: false,
            event,
            seconds: 0.,
            mach: None,
            available_g: None,
            closest: None,
            ground: false,
            defeated: false,
            control_expired: false,
            metrics: Metrics::default(),
        }
    }
    fn finish(
        hit: bool,
        b: missiles::energy::Body,
        f: missiles::energy::Flight,
        closest: f64,
        metrics: Metrics,
    ) -> Self {
        let ground = f.position[1] <= 0.;
        let expired = f.age > b.guidance_ticks;
        let event = if hit {
            "hit"
        } else if f.defeated {
            "post_burn_defeat"
        } else if ground {
            "ground"
        } else if expired {
            "control_expiry"
        } else if f.age >= b.life {
            "life_expiry"
        } else {
            "other"
        };
        let air = missiles::energy::atmosphere(f.position[1]);
        let v2 = dot(f.velocity, f.velocity);
        let g = (0.5 * air.density * v2 * b.area() * b.lift_coefficient
            / b.mass_at(f.age)
            / missiles::energy::G)
            .min(b.structural_g);
        Self {
            hit,
            event,
            seconds: f64::from(f.age) / 120.,
            mach: Some(v2.sqrt() / air.sound),
            available_g: Some(g),
            closest: Some(closest),
            ground,
            defeated: f.defeated,
            control_expired: expired,
            metrics,
        }
    }
    fn json(&self) -> String {
        format!(
            "{{\"hit\":{},\"event\":\"{}\",\"seconds\":{:.9},\"mach\":{},\"available_g\":{},\"closest_m\":{},\"ground\":{},\"defeated\":{},\"control_expired\":{},\"powered_subsonic_seconds\":{:.9},\"powered_subsonic_after_supersonic_seconds\":{:.9},\"minimum_powered_mach_after_supersonic\":{}}}",
            self.hit,
            self.event,
            self.seconds,
            num(self.mach),
            num(self.available_g),
            num(self.closest),
            self.ground,
            self.defeated,
            self.control_expired,
            f64::from(self.metrics.powered_low) / 120.,
            f64::from(self.metrics.low_after_supersonic) / 120.,
            num(self.metrics.min_after_super)
        )
    }
}
fn physical_run(w: &Weapon, c: Condition, range: f64, diagnostics: bool) -> Shot {
    let mut q = question(w, c, range);
    let mut metrics = Metrics::default();
    while q.answer.is_none() {
        if diagnostics {
            metrics.sample(q.body, q.flight);
        }
        trace_step(q.body, q.flight, Some(q.target), true, "ideal");
        q.step_over(&|_, _| 0., &|a, b| a[1] > 0. && b[1] > 0.);
    }
    Shot::finish(q.answer.unwrap().hit, q.body, q.flight, q.closest, metrics)
}
fn run(w: &Weapon, c: Condition, r: f64, seeker: bool, diag: bool) -> Shot {
    if seeker {
        seeker_run(w, c, r, diag)
    } else {
        physical_run(w, c, r, diag)
    }
}
fn edge(w: &Weapon, c: Condition, seeker: bool, factor: f64) -> String {
    if seeker && missiles::Profile::for_weapon(w).is_none() {
        return "null".into();
    }
    let body = missiles::energy_profile::body(w);
    let mut ranges = vec![1., 5., 10., 25., 50., 75., 1_000_000.];
    let mut d = 100.;
    while d < 1_000_000. {
        ranges.push(d);
        d *= factor;
    }
    if body.loft_range > 0. {
        ranges.extend(
            prediction::envelope_probe_scales(body)
                .iter()
                .map(|x| body.loft_range / 0.6 * x)
                .filter(|x| *x <= 1_000_000.),
        );
    }
    ranges.sort_by(f64::total_cmp);
    ranges.dedup();
    let mut samples: Vec<(f64, Shot)> = ranges
        .into_iter()
        .map(|r| (r, run(w, c, r, seeker, false)))
        .collect();
    // Refine every observed hit/miss transition, including disconnected islands.
    let mut additions = Vec::new();
    for pair in samples.windows(2) {
        if pair[0].1.hit == pair[1].1.hit {
            continue;
        }
        let mut lo = pair[0].0;
        let mut hi = pair[1].0;
        let left = pair[0].1.hit;
        while hi - lo > 0.5 {
            let mid = (lo + hi) * 0.5;
            let s = run(w, c, mid, seeker, false);
            if s.hit == left {
                lo = mid;
            } else {
                hi = mid;
            }
            additions.push((mid, s));
        }
    }
    samples.extend(additions);
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let last = samples.iter().rposition(|x| x.1.hit);
    let mut intervals = Vec::new();
    let mut start = None;
    for (r, s) in &samples {
        if s.hit && start.is_none() {
            start = Some(*r);
        }
        if !s.hit
            && let Some(lo) = start.take()
        {
            intervals.push(format!("[{lo:.9},{r:.9}]"));
        }
    }
    if let Some(lo) = start {
        intervals.push(format!("[{lo:.9},1000000.0]"));
    }
    let scan = samples
        .iter()
        .map(|(r, s)| format!("[{r:.9},{},\"{}\"]", s.hit, s.event))
        .collect::<Vec<_>>()
        .join(",");
    if let Some(i) = last {
        let r = samples[i].0;
        let hi = samples.get(i + 1).map(|x| x.0).unwrap_or(r);
        let good = run(w, c, r, seeker, true);
        let bad = run(w, c, hi, seeker, true);
        let counter = if seeker {
            Some(physical_run(w, c, hi, true))
        } else {
            None
        };
        let limit = if bad.event == "seeker_limit" || counter.as_ref().is_some_and(|s| s.hit) {
            "seeker_limit"
        } else {
            bad.event
        };
        format!(
            "{{\"maximum_m\":{r:.9},\"miss_m\":{hi:.9},\"limit\":\"{limit}\",\"edge_hit\":{},\"edge_miss\":{},\"miss_physical_counterfactual\":{},\"hit_intervals_m\":[{}],\"samples\":[{scan}]}}",
            good.json(),
            bad.json(),
            counter.map_or("null".into(), |s| s.json()),
            intervals.join(",")
        )
    } else {
        let all_reject = samples.iter().all(|x| x.1.event == "seeker_limit");
        let limit = if all_reject {
            "seeker_limit"
        } else {
            "no_sampled_hit"
        };
        format!(
            "{{\"maximum_m\":0,\"miss_m\":null,\"limit\":\"{limit}\",\"edge_hit\":null,\"edge_miss\":null,\"miss_physical_counterfactual\":null,\"hit_intervals_m\":[],\"samples\":[{scan}]}}"
        )
    }
}
const CASES: [(&str, f64, f64); 11] = [
    ("co500", 500., 500.),
    ("co5000", 5000., 5000.),
    ("co15000", 15000., 15000.),
    ("co25000", 25000., 25000.),
    ("co35000", 35000., 35000.),
    ("co45000", 45000., 45000.),
    ("up5_15", 5000., 15000.),
    ("up5_30", 5000., 30000.),
    ("down30_5", 30000., 5000.),
    ("down30_500", 30000., 500.),
    ("down15_500", 15000., 500.),
];
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--identity") {
        println!("{}", missiles::energy_profile::fingerprint());
        return Ok(());
    }
    if args.len() < 3
        || (args
            .get(4)
            .is_some_and(|s| matches!(s.as_str(), "point" | "cell" | "trace" | "advice"))
            && args.len() < 11)
    {
        return Err("usage: ordnance_envelopes JT_DIR OUT_DIR [SOURCE [point|cell|trace|advice LAUNCH_FT TARGET_FT ASPECT_DEG SHOOTER_MPS TARGET_MPS RANGE_M_OR_RATIO]]".into());
    }
    let mut overrides = std::collections::BTreeMap::new();
    if let Some(i) = args.iter().position(|s| s == "--override") {
        for pair in args
            .get(i + 1)
            .ok_or("--override requires key=value pairs")?
            .split(',')
        {
            let (key, value) = pair.split_once('=').ok_or("invalid override")?;
            if ![
                "fuel",
                "share",
                "boost_s",
                "sustain_s",
                "coefficient",
                "drag",
                "induced",
                "navigation",
                "lift",
                "g",
                "control",
                "defeat",
            ]
            .contains(&key)
            {
                return Err("unknown override".into());
            }
            let value = value.parse::<f64>()?;
            if !value.is_finite() || value < 0. {
                return Err("invalid override value".into());
            }
            overrides.insert(key.to_owned(), value);
        }
    }
    OVERRIDES.set(overrides).unwrap();
    let dir = Path::new(&args[1]);
    let out = Path::new(&args[2]);
    let filter = args.get(3).map(String::as_str).unwrap_or("");
    if args.get(4).is_some_and(|s| s == "cell") {
        let w = Weapon::parse(filter, &fs::read(dir.join(filter))?)?;
        let c = Condition {
            launch_ft: args[5].parse()?,
            target_ft: args[6].parse()?,
            aspect: args[7].parse()?,
            shooter: args[8].parse()?,
            speed: args[9].parse()?,
        };
        let factor: f64 = args[10].parse()?;
        if !factor.is_finite() || factor <= 1. || factor > 2. {
            return Err("scan ratio must be greater than 1 and at most 2".into());
        }
        println!(
            "{{\"physical\":{},\"seeker\":{}}}",
            edge(&w, c, false, factor),
            edge(&w, c, true, factor)
        );
        return Ok(());
    }
    if args.get(4).is_some_and(|s| s == "advice") {
        let w = Weapon::parse(filter, &fs::read(dir.join(filter))?)?;
        let c = Condition {
            launch_ft: args[5].parse()?,
            target_ft: args[6].parse()?,
            aspect: args[7].parse()?,
            shooter: args[8].parse()?,
            speed: args[9].parse()?,
        };
        let distance = args[10].parse()?;
        let q = question(&w, c, distance);
        let mut service = prediction::Service::default();
        let mut maximum_work = 0;
        let mut first_published = None;
        for tick in 0..20000 {
            // A held cue re-requests every tick, as the cockpit does.
            service.request(1, 1, tick, q, 30);
            service.request_range_with_minimum(
                1,
                1,
                tick,
                q,
                f64::from(w.seeker.zones[1].minimum_range) * FT,
            );
            maximum_work = maximum_work.max(service.advance(tick, &|_, _| 0.));
            if service.ranges[0].result.is_some() {
                first_published.get_or_insert(tick + 1);
            }
            if service.ranges[0].done {
                let slant = distance.hypot((c.target_ft - c.launch_ft) * FT);
                let e = service.ranges[0].result.unwrap().advised(slant);
                let hit = service.clients[0].result.unwrap().answer.hit;
                println!(
                    "{{\"minimum_m\":{},\"maximum_m\":{},\"regions\":{},\"first_ticks\":{},\"ticks\":{},\"max_work\":{},\"point_hit\":{},\"in_region\":{}}}",
                    e.minimum,
                    e.maximum,
                    e.count,
                    first_published.unwrap_or(tick + 1),
                    tick + 1,
                    maximum_work,
                    hit,
                    e.contains(slant)
                );
                return Ok(());
            }
        }
        return Err("range advice did not finish".into());
    }
    if args.get(4).is_some_and(|s| s == "trace") {
        let w = Weapon::parse(filter, &fs::read(dir.join(filter))?)?;
        let c = Condition {
            launch_ft: args[5].parse()?,
            target_ft: args[6].parse()?,
            aspect: args[7].parse()?,
            shooter: args[8].parse()?,
            speed: args[9].parse()?,
        };
        let distance = args[10].parse()?;
        let seeker = args.get(11).is_some_and(|s| s == "seeker");
        eprintln!(
            "trace metadata: source={} all_aspect={} seeker={:?} body={:?}",
            w.source,
            missiles::energy_seeker::all_aspect(&w),
            missiles::energy_seeker::Settings::for_weapon(&w),
            question(&w, c, distance).body
        );

        TRACE.store(true, Ordering::Relaxed);
        println!(
            "seconds,x_m,y_m,z_m,vx_mps,vy_mps,vz_mps,mach,available_g,required_g,g_deficit,thrust_n,drag_n,velocity_los_deg,controlled,lost,defeated,subsonic_ticks,observed,seeker_status"
        );
        println!("# result {}", run(&w, c, distance, seeker, true).json());
        return Ok(());
    }
    if args.get(4).is_some_and(|s| s == "point") {
        let w = Weapon::parse(filter, &fs::read(dir.join(filter))?)?;
        let c = Condition {
            launch_ft: args[5].parse()?,
            target_ft: args[6].parse()?,
            aspect: args[7].parse()?,
            shooter: args[8].parse()?,
            speed: args[9].parse()?,
        };
        let r = args[10].parse()?;
        println!(
            "{{\"physical\":{},\"seeker\":{}}}",
            physical_run(&w, c, r, true).json(),
            seeker_run(&w, c, r, true).json()
        );
        return Ok(());
    }
    let stores: Vec<_> = tore_formats::missile_motors::MOTORS
        .iter()
        .filter(|m| {
            m.control_class.tail_floor_km().is_some()
                && (filter.is_empty() || filter.split(',').any(|n| n == m.source))
        })
        .collect();
    std::thread::scope(|scope| {
        let mut threads = Vec::new();
        for worker in 0..16 {
            let stores = &stores;
            threads.push(scope.spawn(move||{
            for m in stores.iter().skip(worker).step_by(16) {
                let w=Weapon::parse(m.source,&fs::read(dir.join(m.source)).unwrap()).unwrap();
                let mut file=fs::File::create(out.join(format!("{}.jsonl",m.source))).unwrap();
                let held=missiles::Profile::for_weapon(&w).is_none();
                for (name,launch_ft,target_ft) in CASES {
                    for (shooter,speed) in [(300.,250.),(200.,250.),(300.,150.)] {
                        if speed==150. && !["co500","co15000","co35000"].contains(&name) {continue;}
                        for aspect in [0.,45.,90.,135.,180.] {
                            let c=Condition{launch_ft,target_ft,aspect,shooter,speed};
                            let physical=edge(&w,c,false,1.1);let seeker=edge(&w,c,true,1.1);
                            writeln!(file,"{{\"store\":\"{}\",\"held\":{held},\"case\":\"{name}\",\"shooter_altitude_ft\":{launch_ft},\"target_altitude_ft\":{target_ft},\"aspect_deg\":{aspect},\"shooter_speed_mps\":{shooter},\"target_speed_mps\":{speed},\"physical\":{physical},\"seeker\":{seeker}}}",m.source).unwrap();
                        }
                    }
                    eprintln!("{} {name} done",m.source);
                }
            }
        }));
        }
        for t in threads {
            t.join().unwrap();
        }
    });
    Ok(())
}
