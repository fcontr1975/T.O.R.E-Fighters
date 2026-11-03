//! Offline physical envelopes and public-anchor calibration. Runtime never calls this search.
use std::{fs, path::Path};
use tore_formats::weapons::Weapon;
use tore_sim::{
    attitude::{Basis, dot},
    combat::{
        live::{self, ActorSupport, Launcher, Projectile, Target},
        missiles::{self, energy::FT, energy_estimate::Trial, prediction},
    },
};
fn question(w: &Weapon, alt: f64, aspect: usize, range: f64) -> Trial {
    let surface = missiles::surface_launched(&w.source);
    let strike = w.flags & 0x20000 != 0;
    let command = tore_sim::combat::guided::kind(w) == Some(missiles::Guidance::Command);
    let speed = if surface {
        0.
    } else if command {
        30.
    } else {
        300.
    };
    let launch_alt = if surface { 10. } else { alt };
    let target_alt = if strike { 10. } else { alt };
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
    let velocity = if strike {
        [0.; 3]
    } else {
        match aspect {
            3 => [0.; 3],
            0 => [0., 0., -250.],
            1 => [250., 0., 0.],
            _ => [0., 0., 250.],
        }
    };
    let o = missiles::seeker::Observation {
        id: 1,
        position: [0., target_alt / FT, range / FT],
        velocity: velocity.map(|v| v / FT),
        range: range / FT,
        quality: 1.,
        off_axis: 0.,
    };
    prediction::launch_question(
        w,
        l,
        o,
        if tore_sim::combat::guided::kind(w).is_some() {
            tore_sim::combat::guided::HIT_FT
        } else {
            (f64::from(w.damage.fuze_radius.max(0))) + 30.
        },
        [0.; 3],
    )
}
fn hit(w: &Weapon, alt: f64, aspect: usize, range: f64, drag: Option<f64>) -> bool {
    let mut q = question(w, alt, aspect, range);
    if let Some(d) = drag {
        q.body.drag_scale = d;
    }
    while q.answer.is_none() {
        q.step_over(&|_, _| 0., &|a, b| a[1] > 0. && b[1] > 0.);
    }
    q.answer.unwrap().hit
}
fn seeker_hit(w: &Weapon, alt: f64, aspect: usize, range: f64, drag: Option<f64>) -> bool {
    let Some(profile) = missiles::Profile::for_weapon(w) else {
        return false;
    };
    let q = question(w, alt, aspect, range);
    let start = q.flight.position.map(|v| v / FT);
    let heading = if aspect == 0 {
        std::f64::consts::PI
    } else if aspect == 1 {
        std::f64::consts::FRAC_PI_2
    } else {
        0.
    };
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
            return false;
        }
    }
    let mut motion = missiles::Motion::launch_rules(
        w,
        [
            0.,
            0.,
            if missiles::surface_launched(&w.source) {
                0.
            } else {
                300. / FT
            },
        ],
        start[1],
        missiles::Rules::Energy,
    );
    let energy = motion.energy.as_mut().unwrap();
    energy.body = q.body;
    energy.flight = q.flight;
    if let Some(d) = drag {
        energy.body.drag_scale = d;
    }
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
    for age in 0..q.body.life {
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
        if dot(gap, gap) <= radius * radius
            && age >= q.arming_age
            && !m.energy.unwrap().flight.defeated
        {
            return true;
        }
        if p.position[1] <= 0. || m.energy.unwrap().flight.lost {
            return false;
        }
    }
    false
}
fn maximum(w: &Weapon, alt: f64, aspect: usize, drag: Option<f64>, seeker: bool) -> f64 {
    let hit = |w, alt, aspect, range, drag| {
        if seeker {
            seeker_hit(w, alt, aspect, range, drag)
        } else {
            hit(w, alt, aspect, range, drag)
        }
    };
    let mut low = 0.;
    let mut high = 100.;
    let mut upper: f64 = 1_000_000.;
    let mut distances = Vec::new();
    let body = missiles::energy_profile::body(w);
    if body.loft_range > 0. {
        distances.extend(
            prediction::envelope_probe_scales(body)
                .iter()
                .map(|scale| body.loft_range / 0.6 * scale)
                .filter(|d| *d < 1_000_000.),
        );
    }
    // Search every increasing range, including minimum-range failures.
    while high < 1_000_000. {
        distances.push(high);
        high *= 1.5;
    }
    distances.sort_by(f64::total_cmp);
    distances.dedup();
    for distance in distances {
        if hit(w, alt, aspect, distance, drag) {
            low = distance;
            upper = 1_000_000.;
        } else if distance > low {
            upper = upper.min(distance);
        }
    }
    if low == 0. {
        // Minimum turn radius can leave a narrow beam-intercept interval
        // between the coarse probes. Retry only empty offline envelopes.
        let mut distance = 100.;
        while distance < 1_000_000. {
            if hit(w, alt, aspect, distance, drag) {
                low = distance;
                upper = 1_000_000.;
            } else if distance > low {
                upper = upper.min(distance);
            }
            distance *= 1.1;
        }
    }
    if low == 0. {
        return 0.;
    }
    high = upper;
    for _ in 0..10 {
        let mid = (low + high) * 0.5;
        if hit(w, alt, aspect, mid, drag) {
            low = mid
        } else {
            high = mid
        }
    }
    low
}
fn candidate_hit(
    w: &Weapon,
    body: missiles::energy::Body,
    anchor: [f64; 4],
    distance: f64,
) -> bool {
    let mut q = question(w, anchor[0] * FT, anchor[1] as usize, distance);
    q.body = body;
    q.target.velocity = match anchor[1] as usize {
        0 => [0., 0., -anchor[3]],
        1 => [anchor[3], 0., 0.],
        2 => [0., 0., anchor[3]],
        _ => [0.; 3],
    };
    while q.answer.is_none() {
        q.step_over(&|_, _| 0., &|a, b| a[1] > 0. && b[1] > 0.);
    }
    q.answer.unwrap().hit
}
fn candidate_maximum(w: &Weapon, body: missiles::energy::Body, anchor: [f64; 4]) -> f64 {
    candidate_maximum_precision(w, body, anchor, 12)
}
fn candidate_maximum_precision(
    w: &Weapon,
    body: missiles::energy::Body,
    anchor: [f64; 4],
    precision: usize,
) -> f64 {
    candidate_maximum_kind(w, body, anchor, precision, false)
}
fn candidate_maximum_kind(
    w: &Weapon,
    body: missiles::energy::Body,
    anchor: [f64; 4],
    precision: usize,
    powered: bool,
) -> f64 {
    let hits = |distance: f64| {
        if powered {
            let q = arrival_trial(w, body, anchor, distance / 1000.);
            q.answer.unwrap().hit
                && q.flight.age < body.ignition + body.boost.ticks + body.sustain.ticks
        } else {
            candidate_hit(w, body, anchor, distance)
        }
    };
    if precision < 12 {
        // Local bracketing for optimizer iterations. Every emitted candidate
        // is remeasured with the complete loft-aware scan below.
        let mut high = (anchor[2] * 1000.).clamp(100., 1_000_000.);
        let mut low = high;
        if hits(high) {
            while high < 1_000_000. {
                high = (high * 1.5).min(1_000_000.);
                if !hits(high) {
                    break;
                }
                low = high;
            }
        } else {
            loop {
                low /= 1.5;
                if low < 100. {
                    return 0.;
                }
                if hits(low) {
                    break;
                }
                high = low;
            }
        }
        for _ in 0..precision {
            let mid = (low + high) * 0.5;
            if hits(mid) {
                low = mid;
            } else {
                high = mid;
            }
        }
        return low / 1000.;
    }
    let mut low = 0.;
    let mut high: f64 = 1_000_000.;
    let mut distances = Vec::new();
    let mut d = 100.;
    while d < 1_000_000. {
        distances.push(d);
        d *= 1.5;
    }
    if body.loft_range > 0. {
        distances.extend(
            prediction::envelope_probe_scales(body)
                .iter()
                .map(|scale| body.loft_range / 0.6 * scale),
        );
    }
    distances.sort_by(f64::total_cmp);
    distances.dedup();
    for d in distances {
        if hits(d) {
            low = d;
            high = 1_000_000.;
        } else if d > low {
            high = high.min(d);
        }
    }
    if low == 0. {
        // Match the table writer's empty-envelope retry. A narrow valid
        // beam interval must not be rejected merely for missing coarse probes.
        let mut distance = 100.;
        while distance < 1_000_000. {
            if hits(distance) {
                low = distance;
                high = 1_000_000.;
            } else if distance > low {
                high = high.min(distance);
            }
            distance *= 1.1;
        }
    }
    if low == 0. {
        return 0.;
    }
    for _ in 0..precision {
        let mid = (low + high) * 0.5;
        if hits(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    low / 1000.
}
/// Fixed-budget balance fitting. The only adjustable parameters are fuel and
/// drag. Explicit thrust overrides, burn durations and control budgets stay fixed.
fn fit_class(
    dir: &Path,
    filter: Option<&str>,
    ground_roles: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if ground_roles {
        println!("source,class,fraction,drag,control_s,anchor_km,primary_km,score");
    } else {
        println!(
            "source,class,fraction,drag,control_s,anchor_km,tail_target_km,score,min_launch_km,floor_km,r500h,r500b,r500t,r5h,r5b,r5t,r15h,r15b,r15t,r35h,r35b,r35t"
        );
    }
    for m in tore_formats::missile_motors::MOTORS {
        if filter.is_some_and(|s| !s.split(',').any(|name| name == m.source))
            || m.control_class.tail_floor_km().is_none() != ground_roles
        {
            continue;
        }
        let w = Weapon::parse(m.source, &fs::read(dir.join(m.source))?)?;
        let original = missiles::energy_profile::body(&w);
        let minimum = w
            .seeker
            .zones
            .iter()
            .map(|z| f64::from(z.minimum_range) * FT / 1000.)
            .fold(0f64, f64::max);
        let floor = m
            .control_class
            .tail_floor_km()
            .map_or(0., |v| v.max(1.2 * minimum + 0.01));
        let scaled = |fuel: f64, drag: f64| {
            let mut b = original;
            let ratio = fuel / m.propellant_fraction;
            b.boost.propellant *= ratio;
            b.sustain.propellant *= ratio;
            if m.thrust.is_none() {
                b.boost.thrust *= ratio;
                b.sustain.thrust *= ratio;
            }
            b.drag_scale = drag;
            b
        };
        for drag_step in 2..=12 {
            let drag = f64::from(drag_step) * 0.25;
            let threshold = |anchor: [f64; 4]| {
                let mut lo = 0.12;
                let mut hi = 0.5;
                if candidate_hit(&w, scaled(lo, drag), anchor, anchor[2] * 1000.) {
                    return lo;
                }
                if candidate_hit(&w, scaled(hi, drag), anchor, anchor[2] * 1000.) {
                    for _ in 0..12 {
                        let mid = (lo + hi) * 0.5;
                        if candidate_hit(&w, scaled(mid, drag), anchor, anchor[2] * 1000.) {
                            hi = mid;
                        } else {
                            lo = mid;
                        }
                    }
                }
                hi
            };
            let primary_fuel = threshold(m.primary);
            let mut fuels = vec![0.12, primary_fuel, 0.5];
            if !ground_roles {
                let tail_fuel = threshold(m.secondary);
                fuels.extend([
                    tail_fuel,
                    0.25 * primary_fuel + 0.75 * tail_fuel,
                    0.5 * (primary_fuel + tail_fuel),
                    0.75 * primary_fuel + 0.25 * tail_fuel,
                ]);
            }
            fuels.sort_by(f64::total_cmp);
            fuels.dedup_by(|a, b| (*a - *b).abs() < 0.00001);
            for fuel in fuels {
                let b = scaled(fuel, drag);
                if motor_shape_error(m, b, m.exhaust_mps).is_some()
                    || cruise_arrival_error(&w, m, b).is_some()
                {
                    continue;
                }
                let primary = candidate_maximum(&w, b, m.primary);
                let mut score = 4. * (primary / m.primary[2] - 1.).abs() + 0.02 * drag.ln().powi(2);
                if ground_roles {
                    println!(
                        "{},{:?},{fuel:.9},{drag},{},{},{primary:.6},{score:.8}",
                        m.source,
                        m.control_class,
                        m.control_class.seconds(),
                        m.primary[2]
                    );
                    continue;
                }
                let tail = candidate_maximum(&w, b, m.secondary);
                if tail < floor {
                    continue;
                }
                score += 4. * (tail / m.secondary[2] - 1.).abs();
                let mut ranges = Vec::new();
                for alt in [500., 5000., 15000., 35000.] {
                    for aspect in 0..3 {
                        ranges.push(if alt == 35000. && aspect == 0 {
                            primary
                        } else if alt == 5000. && aspect == 2 {
                            tail
                        } else {
                            candidate_maximum(&w, b, [alt, f64::from(aspect), 0., 250.])
                        });
                    }
                }
                if ranges.contains(&0.) {
                    continue;
                }
                print!(
                    "{},{:?},{fuel:.9},{drag},{},{},{},{score:.8},{minimum},{floor}",
                    m.source,
                    m.control_class,
                    m.control_class.seconds(),
                    m.primary[2],
                    m.secondary[2]
                );
                for value in ranges {
                    print!(",{value:.6}");
                }
                println!();
            }
        }
        eprintln!("balance candidates {}", m.source);
    }
    Ok(())
}

const MOTOR_FIT_HEADER: &str = "source,class,fraction,drag,control_s,boost_s,sustain_s,exhaust_mps,boost_n,sustain_n,boost_share,anchor_km,tail_target_km,primary_km,score,min_launch_km,floor_km,r500h,r500b,r500t,r5h,r5b,r5t,r15h,r15b,r15t,r35h,r35b,r35t";

/// Authored game balance, never a reconstruction of a real motor.
/// Fuel 12..50%, drag .5..3 and the unchanged class control budget bound every fit.
/// Staged fits allocate total impulse between non-inverted motor phases with
/// launch acceleration, dry-mass load, coefficient and Mach-one guards below.
/// The older four-coordinate search only rescales existing phase proportions.
fn fit_motors<const N: usize>(
    dir: &Path,
    filter: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{MOTOR_FIT_HEADER}");
    let sources: Vec<_> = tore_formats::missile_motors::MOTORS
        .iter()
        .filter(|m| filter.is_none_or(|f| f.split(',').any(|n| n == m.source)))
        .collect();
    std::thread::scope(|scope| {
        let mut jobs = Vec::new();
        for worker in 0..4 {
            let sources = &sources;
            jobs.push(scope.spawn(move || -> Result<(), String> {
                for m in sources.iter().skip(worker).step_by(4) {
                    let text = fit_motor_store::<N>(dir, m, None, 8).map_err(|e| e.to_string())?;
                    std::io::Write::write_all(&mut std::io::stdout().lock(), text.as_bytes())
                        .map_err(|e| e.to_string())?;
                    eprintln!("motor fits {}", m.source);
                }
                Ok(())
            }));
        }
        for job in jobs {
            job.join()
                .map_err(|_| "fit worker panicked".to_string())??;
        }
        Ok::<_, String>(())
    })?;
    Ok(())
}
fn fit_motor_store<const N: usize>(
    dir: &Path,
    m: &tore_formats::missile_motors::MotorData,
    physical_seed: Option<[f64; 6]>,
    precision: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    use missiles::energy::{Body, DT, G};
    use std::fmt::Write;
    let mut output = String::new();
    let air = m.control_class.tail_floor_km().is_some();
    let w = Weapon::parse(m.source, &fs::read(dir.join(m.source))?)?;
    let base = missiles::energy_profile::body(&w);
    let minimum = w
        .seeker
        .zones
        .iter()
        .map(|z| f64::from(z.minimum_range) * FT / 1000.)
        .fold(0f64, f64::max);
    let floor = m
        .control_class
        .tail_floor_km()
        .map_or(0., |f| f.max(1.2 * minimum + 0.02));
    let max_burn_scale = (fit_burn_seconds(m, base)
        / (f64::from(base.boost.ticks + base.sustain.ticks) * DT))
        .min(16.);
    let build = |x: [f64; N]| -> (Body, f64, f64) {
        let fuel = 0.12 + 0.38 * x[0];
        let duration = (0.25f64.ln() + x[2] * (max_burn_scale / 0.25).ln()).exp();
        let impulse = (0.25f64.ln() + x[3] * 16f64.ln()).exp();
        let mut body = base;
        body.drag_scale = 0.5 + 2.5 * x[1];
        for (phase, nominal) in [
            (&mut body.boost, base.boost),
            (&mut body.sustain, base.sustain),
        ] {
            phase.propellant = nominal.propellant * fuel / m.propellant_fraction;
            if nominal.ticks > 0 {
                phase.ticks = (f64::from(nominal.ticks) * duration).round().max(1.) as u32;
                phase.thrust = nominal.thrust * fuel / m.propellant_fraction
                    * impulse
                    * f64::from(nominal.ticks)
                    / f64::from(phase.ticks);
            }
        }
        if N == 6 {
            let jet = powered_cruise(m, base) && base.boost.ticks == 0;
            let exhaust = if jet {
                5000. + 23000. * x[3]
            } else {
                600. + 5400. * x[3]
            };
            let impulse_total = base.mass * fuel * exhaust;
            let dry = base.mass * (1. - fuel);
            let minimum = (impulse_total / (dry * 53.5 * G)).max(0.5);
            let maximum = fit_burn_seconds(m, base);
            let duration = minimum * (maximum / minimum).powf(x[2]);
            let total = ((duration / DT).round().max((minimum / DT).ceil()).max(2.) as u32)
                .min((maximum / DT).floor() as u32);
            let mut share = if jet { 0. } else { 0.05 + 0.93 * x[5] };
            let boost = if jet {
                0
            } else {
                let shortest = impulse_total * share / (dry * 53.5 * G);
                let longest = (impulse_total * share
                    / (base.mass * (minimum_boost_g(m, base) + 0.01) * G))
                    .min(f64::from(total) * DT * share);
                ((shortest + (longest - shortest).max(0.) * x[4]) / DT + 1e-6)
                    .floor()
                    .max((shortest / DT).ceil())
                    .clamp(1., f64::from(total - 1)) as u32
            };
            if boost > 0 {
                share = share.max(f64::from(boost) / f64::from(total) + 1e-7);
            }
            body.boost = missiles::energy::Motor {
                ticks: boost,
                propellant: base.mass * fuel * share,
                thrust: if boost == 0 {
                    0.
                } else {
                    impulse_total * share / (f64::from(boost) * DT)
                },
            };
            body.sustain = missiles::energy::Motor {
                ticks: total - boost,
                propellant: base.mass * fuel * (1. - share),
                thrust: impulse_total * (1. - share) / (f64::from(total - boost) * DT),
            };
            body.life = body.life.max(body.ignition + total + 1200);
            return (body, fuel, exhaust / m.exhaust_mps);
        }
        (body, fuel, impulse)
    };
    let mut effective_tail = if air { m.secondary[2].max(floor) } else { 0. };
    if matches!(m.source, "AIM7.JT" | "AIM7E.JT") {
        let a = tore_formats::missile_motors::motor("AIM7.JT")
            .unwrap()
            .secondary[2];
        let b = tore_formats::missile_motors::motor("AIM7E.JT")
            .unwrap()
            .secondary[2];
        // Minimum equal-relative-error compromise for contradictory tail targets.
        effective_tail = 2. * a * b / (a + b) + if m.source == "AIM7.JT" { 0.02 } else { -0.02 };
    }
    let goals: &[(f64, f64)] = if physical_seed.is_some() {
        if m.source == "AA8.JT" {
            &[(1.25, 1.)]
        } else {
            &[(1., 1.)]
        }
    } else if m.source == "AA8.JT" {
        &[
            (1.25, 1.),
            (1.28, 0.95),
            (1.20, 0.95),
            (1.28, 1.05),
            (1., 1.),
        ]
    } else if air {
        &[(1., 1.), (0.92, 1.), (1.08, 1.), (1., 0.92), (1., 1.08)]
    } else if powered_cruise(m, base) {
        &[(1., 1.), (1.05, 1.)]
    } else {
        &[(1., 1.)]
    };
    let mut outputs: Vec<([f64; N], f64)> = Vec::new();
    for &(head_factor, tail_factor) in goals {
        for start in 0..if physical_seed.is_some() {
            1
        } else if N == 6 {
            10
        } else {
            4
        } {
            let seed = if start == 0 {
                [
                    ((m.propellant_fraction - 0.12) / 0.38).clamp(0., 1.),
                    ((base.drag_scale - 0.5) / 2.5).clamp(0., 1.),
                    (4f64.ln() / (max_burn_scale / 0.25).ln()).clamp(0., 1.),
                    0.5,
                ]
            } else if start == 1 {
                [0.55, 0.8, 0.25, 0.55]
            } else if start == 2 {
                [0.55, 1., 0., 0.2]
            } else {
                [0.3, 0.3, 0.8, 0.65]
            };
            let mut initial = [0.; N];
            initial[..4].copy_from_slice(&seed);
            if N == 6 {
                if start < 4 {
                    initial[2] = [0.55, 0.25, 0.8, 0.4][start];
                    initial[3] = [0.3, 0.15, 0.6, 0.85][start];
                    initial[4] = [0.5, 0.7, 0.3, 0.5][start];
                    initial[5] = [0.6, 0.8, 0.3, 0.1][start];
                } else if start >= 8 {
                    initial.copy_from_slice(&if start == 8 {
                        [0.5, 0.8, 0.15, 0.3, 0.8, 0.2]
                    } else {
                        [0.3, 0.2, 1., 0.3, 0.5, 0.3]
                    });
                } else if start >= 6 {
                    let seed = if start == 6 {
                        [1., 0., 1., 0.1, 0.95, 0.]
                    } else {
                        [0.7, 0., 0.98, 0.25, 0.8, 0.02]
                    };
                    initial.copy_from_slice(&seed);
                } else {
                    let seed = if powered_cruise(m, base) {
                        if start == 4 {
                            [1., 1., 0.92, 0.2, 0.9, 0.]
                        } else {
                            [0.7, 0.9, 0.9, 0.4, 0.8, 0.02]
                        }
                    } else if start == 4 {
                        [0., 0., 0.98, 0.15, 0.95, 0.8]
                    } else {
                        [0.2, 0.3, 0.95, 0.3, 0.8, 0.6]
                    };
                    initial.copy_from_slice(&seed);
                }
            }
            if let Some([fuel, drag, boost, sustain, coefficient, share]) = physical_seed {
                let jet = powered_cruise(m, base) && base.boost.ticks == 0;
                let impulse = base.mass * fuel * coefficient;
                let dry = base.mass * (1. - fuel);
                let minimum = (impulse / (dry * 53.5 * G)).max(0.5);
                let maximum = fit_burn_seconds(m, base);
                let total = boost + sustain;
                let shortest = impulse * share / (dry * 53.5 * G);
                let longest = (impulse * share
                    / (base.mass * (minimum_boost_g(m, base) + 0.01) * G))
                    .min(total * share);
                let values = [
                    (fuel - 0.12) / 0.38,
                    (drag - 0.5) / 2.5,
                    (total / minimum).ln() / (maximum / minimum).ln(),
                    if jet {
                        (coefficient - 5000.) / 23000.
                    } else {
                        (coefficient - 600.) / 5400.
                    },
                    if jet {
                        0.
                    } else {
                        (boost - shortest) / (longest - shortest).max(DT)
                    },
                    if jet { 0. } else { (share - 0.05) / 0.93 },
                ];
                initial.copy_from_slice(&values.map(|v| v.clamp(0., 1.)));
            }
            if precision == 0 {
                outputs.push((initial, 0.));
                continue;
            }
            while N == 4 && initial[3] > 0. {
                let (body, _, _) = build(initial);
                if [body.boost, body.sustain]
                    .iter()
                    .all(|p| p.thrust <= body.dry_mass() * 60. * G)
                {
                    break;
                }
                initial[3] = (initial[3] - 0.1).max(0.);
            }
            let evaluate = |x: [f64; N]| {
                let (b, _, impulse) = build(x);
                if [b.boost, b.sustain]
                    .iter()
                    .any(|p| p.thrust > b.dry_mass() * 60. * G)
                {
                    return 1e6;
                }
                let head = candidate_maximum_precision(&w, b, m.primary, precision);
                let head_error = (head / (m.primary[2] * head_factor) - 1.).powi(2);
                let mut value = 25. * head_error;
                if !air {
                    for fraction in [0.75, 0.9] {
                        if !candidate_hit(&w, b, m.primary, m.primary[2] * 1000. * fraction) {
                            value += 4.;
                        }
                    }
                }
                if air {
                    let tail = candidate_maximum_precision(&w, b, m.secondary, precision);
                    let tail_error = (tail / (effective_tail * tail_factor) - 1.).powi(2);
                    value = 30. * head_error.max(tail_error) + 5. * (head_error + tail_error);
                    value += 100. * (1. - tail / floor).max(0.).powi(2);
                    let (head_band, tail_band) = accepted_fit_bands(m.source);
                    for (reach, reference, band) in [
                        (head, m.primary[2], head_band),
                        (tail, m.secondary[2], tail_band),
                    ] {
                        let ratio = reach / reference;
                        // Seek a small interior margin so full-precision emission
                        // does not round a coarse optimizer boundary out of band.
                        value += 10000.
                            * ((band[0] + 0.002 - ratio).max(0.).powi(2)
                                + (ratio - band[1] + 0.002).max(0.).powi(2));
                    }
                }
                if m.control_class.air_target() {
                    value += 10000. * (mach_one_seconds(b).unwrap_or(20.) - 2.98).max(0.).powi(2);
                    if b.sustain.ticks > 0 {
                        value += 10000. * (1.02 - sustain_g(b)).max(0.).powi(2);
                    }
                }
                if powered_cruise(m, b) {
                    for distance in [m.primary[2] * 0.5, m.primary[2], head] {
                        let q = arrival_trial(&w, b, m.primary, distance);
                        let a = q.answer.unwrap();
                        let mach = dot(q.flight.velocity, q.flight.velocity).sqrt()
                            / missiles::energy::atmosphere(q.flight.position[1]).sound;
                        let burn = f64::from(b.ignition + b.boost.ticks + b.sustain.ticks) * DT;
                        value += 100. * ((a.seconds + DT) / burn - 1.).max(0.).powi(2)
                            + 1000. * (0.72 - mach).max(0.).powi(2)
                            + 100. * (mach - 0.94).max(0.).powi(2)
                            + if a.hit {
                                0.
                            } else {
                                0.02 + 10. * (a.closest / (m.primary[2] * 1000.)).powi(2)
                            };
                    }
                }
                if N == 4 && motor_shape_error(m, b, m.exhaust_mps * impulse).is_some() {
                    value += 100.;
                }
                value += 0.002
                    * (b.boost.thrust.max(b.sustain.thrust) / (b.dry_mass() * G) / 30.).powi(2);
                value
                    + 0.001
                        * (b.drag_scale.ln().powi(2)
                            + (x[2] - initial[2]).powi(2)
                            + (x[3] - 0.5).powi(2))
            };
            if physical_seed.is_some() {
                outputs.push((initial, evaluate(initial)));
            }
            let clamp = |x: [f64; N]| x.map(|v| v.clamp(0., 1.));
            let mut simplex: Vec<_> = std::iter::once(initial)
                .chain((0..N).map(|i| {
                    let mut x = initial;
                    let step = if physical_seed.is_some() { 0.025 } else { 0.18 };
                    x[i] = if x[i] > 0.8 { x[i] - step } else { x[i] + step };
                    clamp(x)
                }))
                .map(|x| (x, evaluate(x)))
                .collect();
            for iteration in 0..if physical_seed.is_some() { 320 } else { 160 } {
                simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
                if iteration > 30 && simplex[0].1 < 0.0003 {
                    break;
                }
                let centroid: [f64; N] = std::array::from_fn(|i| {
                    simplex[..N].iter().map(|p| p.0[i]).sum::<f64>() / N as f64
                });
                let point = |factor: f64| {
                    clamp(std::array::from_fn(|i| {
                        centroid[i] + factor * (centroid[i] - simplex[N].0[i])
                    }))
                };
                let reflected = point(1.);
                let score = evaluate(reflected);
                if score < simplex[0].1 {
                    let expanded = point(2.);
                    let e = evaluate(expanded);
                    simplex[N] = if e < score {
                        (expanded, e)
                    } else {
                        (reflected, score)
                    };
                } else if score < simplex[N - 1].1 {
                    simplex[N] = (reflected, score);
                } else {
                    let contracted = point(if score < simplex[N].1 { 0.5 } else { -0.5 });
                    let c = evaluate(contracted);
                    if c < score.min(simplex[N].1) {
                        simplex[N] = (contracted, c);
                    } else {
                        let best = simplex[0].0;
                        for p in &mut simplex[1..] {
                            p.0 = clamp(std::array::from_fn(|i| 0.5 * (p.0[i] + best[i])));
                            p.1 = evaluate(p.0);
                        }
                    }
                }
            }
            simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
            outputs.extend(simplex.into_iter().take(2));
        }
    }
    // Keep bounded low-energy endpoints too, rather than losing a feasible
    // endpoint when a local search follows a disconnected hit interval.
    if !air && N == 4 {
        let nominal_duration = (4f64.ln() / (max_burn_scale / 0.25).ln()).clamp(0., 1.);
        for seed in [
            [0., 1., nominal_duration, 0.5],
            [0., 1., 0., 0.],
            [1., 1., 0., 0.],
        ] {
            let mut x = [0.; N];
            x[..4].copy_from_slice(&seed);
            outputs.push((x, 0.));
        }
    }
    for (x, _) in outputs {
        let (b, fuel, impulse) = build(x);
        if [b.boost, b.sustain]
            .iter()
            .any(|p| p.thrust > b.dry_mass() * 60. * G)
        {
            continue;
        }
        if let Some(reason) = motor_shape_error(m, b, m.exhaust_mps * impulse)
            .or_else(|| cruise_arrival_error(&w, m, b))
        {
            eprintln!(
                "rejected {}: {} fuel={fuel:.6} drag={:.6} boost={:.3}/{:.3}s sustain={:.3}/{:.3}s coefficient={:.3}",
                m.source,
                reason,
                b.drag_scale,
                b.boost.thrust,
                f64::from(b.boost.ticks) * DT,
                b.sustain.thrust,
                f64::from(b.sustain.ticks) * DT,
                m.exhaust_mps * impulse
            );
            continue;
        }
        let primary = candidate_maximum(&w, b, m.primary);
        let tail = if air {
            candidate_maximum(&w, b, m.secondary)
        } else {
            0.
        };
        let score = if air {
            let head = (primary / m.primary[2] - 1.).abs();
            let tail = (tail / m.secondary[2] - 1.).abs();
            8. * head.max(tail) + head + tail
        } else {
            4. * (primary / m.primary[2] - 1.).abs()
                + [0.75, 0.9]
                    .into_iter()
                    .filter(|fraction| {
                        !candidate_hit(&w, b, m.primary, m.primary[2] * 1000. * fraction)
                    })
                    .count() as f64
                    * 4.
        };
        let mut ranges = vec![0.; 12];
        if air {
            for (i, (alt, aspect)) in [500., 5000., 15000., 35000.]
                .into_iter()
                .flat_map(|alt| (0..3).map(move |aspect| (alt, aspect)))
                .enumerate()
            {
                ranges[i] = if i == 9 {
                    primary
                } else if i == 5 {
                    tail
                } else {
                    candidate_maximum(&w, b, [alt, f64::from(aspect), 0., 250.])
                };
            }
            if tail < floor || ranges.contains(&0.) {
                eprintln!(
                    "rejected envelope {}: H={primary} T={tail} cells={ranges:?} fuel={fuel} drag={} boost={}/{} sustain={}/{} coefficient={} share={}",
                    m.source,
                    b.drag_scale,
                    b.boost.thrust,
                    f64::from(b.boost.ticks) * DT,
                    b.sustain.thrust,
                    f64::from(b.sustain.ticks) * DT,
                    m.exhaust_mps * impulse,
                    b.boost.propellant / (b.boost.propellant + b.sustain.propellant)
                );
                continue;
            }
        }
        write!(&mut output,
                "{},{:?},{fuel:.9},{:.9},{},{:.9},{:.9},{:.9},{:.9},{:.9},{:.9},{},{},{primary:.6},{score:.8},{minimum},{floor}",
                m.source,
                m.control_class,
                b.drag_scale,
                m.control_class.seconds(),
                f64::from(b.boost.ticks) * DT,
                f64::from(b.sustain.ticks) * DT,
                m.exhaust_mps * impulse,
                b.boost.thrust,
                b.sustain.thrust,
                b.boost.propellant / (b.boost.propellant + b.sustain.propellant),
                m.primary[2],
                if air { m.secondary[2] } else { 0. }
            ).unwrap();
        for r in ranges {
            write!(&mut output, ",{r:.6}").unwrap();
        }
        writeln!(&mut output).unwrap();
    }
    Ok(output)
}

/// Edge, just-beyond-edge and ten-percent-beyond-edge diagnostics in the shared kernel.
fn diagnostics(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "source,altitude_ft,aspect,range_km,edge_fraction,hit,seconds,defeated,control_expired,mach,available_g,required_g,g_deficit,closing_mps,max_g_deficit,ground_impact,life_expired"
    );
    for m in tore_formats::missile_motors::MOTORS {
        let w = Weapon::parse(m.source, &fs::read(dir.join(m.source))?)?;
        let b = missiles::energy_profile::body(&w);
        let air = m.control_class.tail_floor_km().is_some();
        let conditions: Vec<_> = if air {
            [500., 5000., 15000., 35000.]
                .into_iter()
                .flat_map(|alt| (0..3).map(move |aspect| [alt, f64::from(aspect), 0., 250.]))
                .collect()
        } else {
            vec![m.primary]
        };
        for condition in conditions {
            let edge = candidate_maximum(&w, b, condition);
            for ratio in [1., 1.001, 1.1] {
                let mut q = question(
                    &w,
                    condition[0] * FT,
                    condition[1] as usize,
                    edge * ratio * 1000.,
                );
                let mut last = missiles::energy::Forces::default();
                let mut deficit: f64 = 0.;
                while q.answer.is_none() {
                    let mut sample = q.flight;
                    last = sample.step_diagnostics(q.body, q.wind, Some(q.target));
                    deficit = deficit.max(last.g_deficit);
                    q.step(&|_, _| 0.);
                }
                let a = q.answer.unwrap();
                println!(
                    "{},{},{},{:.6},{ratio},{},{:.6},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{deficit:.6},{},{}",
                    m.source,
                    condition[0],
                    condition[1],
                    edge * ratio,
                    a.hit,
                    a.seconds,
                    q.flight.defeated,
                    q.flight.age > q.body.guidance_ticks,
                    last.mach,
                    last.available_g,
                    last.required_g,
                    last.g_deficit,
                    last.closing_mps,
                    q.flight.position[1] <= 0.,
                    q.flight.age >= q.body.life
                );
            }
        }
    }
    Ok(())
}
fn fit_pairs(
    dir: &Path,
    filter: Option<&str>,
    measure_only: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "source,fraction,drag_scale,guidance_seconds,primary_target_km,primary_km,secondary_target_km,secondary_km,tail5_km,smooth_relative"
    );
    for motor in tore_formats::missile_motors::MOTORS {
        if filter.is_some_and(|s| !s.split(',').any(|name| name == motor.source)) {
            continue;
        }
        let w = Weapon::parse(motor.source, &fs::read(dir.join(motor.source))?)?;
        let original = missiles::energy_profile::body(&w);
        let air = !missiles::surface_launched(&w.source) && w.flags & 0x20000 == 0;
        let fractions: Vec<_> = if measure_only {
            vec![]
        } else if motor.thrust.is_some() || !motor.fit_propellant {
            vec![motor.propellant_fraction]
        } else {
            (6..=25).map(|n| n as f64 * 0.02).collect()
        };
        let tail_anchor = [5000., 2., 0., 250.];
        let tail_floor = (motor.primary[2] * 0.08).clamp(2., 6.);
        let mut best = if measure_only {
            Some((
                motor.propellant_fraction,
                original,
                candidate_maximum(&w, original, motor.secondary),
                if air {
                    candidate_maximum(&w, original, tail_anchor)
                } else {
                    0.
                },
                0.,
            ))
        } else {
            None
        };
        for fraction in fractions {
            let drags: &[f64] = if matches!(motor.source, "SSN9.JT" | "AS16.JT") {
                &[0.25, 0.3, 0.4, 0.5, 0.75, 1., 1.5, 2., 2.5, 3.]
            } else {
                &[0.5, 0.75, 1., 1.5, 2., 2.5, 3.]
            };
            for &drag in drags {
                let mut body = original;
                if motor.thrust.is_none() {
                    let ratio = fraction / motor.propellant_fraction;
                    body.boost.propellant *= ratio;
                    body.boost.thrust *= ratio;
                    body.sustain.propellant *= ratio;
                    body.sustain.thrust *= ratio;
                }
                body.drag_scale = drag;
                body.guidance_ticks = body.life;
                let mut q = question(
                    &w,
                    motor.primary[0] * FT,
                    motor.primary[1] as usize,
                    motor.primary[2] * 1000.,
                );
                q.body = body;
                q.target.velocity = match motor.primary[1] as usize {
                    0 => [0., 0., -motor.primary[3]],
                    2 => [0., 0., motor.primary[3]],
                    _ => [0.; 3],
                };
                while q.answer.is_none() {
                    q.step_over(&|_, _| 0., &|a, b| a[1] > 0. && b[1] > 0.);
                }
                let answer = q.answer.unwrap();
                if !answer.hit {
                    continue;
                }
                // Controlled life fitted per missile to its head-on reference:
                // the documented anchor, with the selected physical motor.
                // Ground/ship cases use their stationary or receding reference.
                body.guidance_ticks = if motor.guidance_evidence.starts_with("NAVWEPS") {
                    original.guidance_ticks
                } else {
                    (answer.seconds * 120.).ceil() as u32 + 1
                };
                let secondary = candidate_maximum(&w, body, motor.secondary);
                let tail = if air {
                    candidate_maximum(&w, body, tail_anchor)
                } else {
                    0.
                };
                let penalty = 0.15 * drag.ln().powi(2) + 0.03 * ((fraction - 0.35) / 0.15).powi(2);
                let primary_error = if motor.guidance_evidence.starts_with("NAVWEPS") {
                    (candidate_maximum(&w, body, motor.primary) / motor.primary[2] - 1.).abs()
                } else {
                    0.
                };
                let score = if drag < 0.5 {
                    1. + (0.5 / drag).ln().powi(2)
                } else {
                    0.
                } + 4. * primary_error
                    + 4. * (secondary / motor.secondary[2] - 1.).abs()
                    + penalty
                    + if air {
                        10. * (1. - tail / tail_floor).max(0.)
                    } else {
                        0.
                    };
                if best.is_none_or(|(_, _, _, _, old)| score < old) {
                    best = Some((fraction, body, secondary, tail, score));
                }
            }
        }
        let (fraction, body, secondary, tail, _) =
            best.ok_or_else(|| format!("{}: no feasible primary", motor.source))?;
        let primary = candidate_maximum(&w, body, motor.primary);
        let mut smooth: f64 = 0.;
        for ratio in [0.98, 1.02] {
            for field in 0..3 {
                let mut nearby = body;
                match field {
                    0 => {
                        nearby.boost.propellant *= ratio;
                        nearby.boost.thrust *= ratio;
                        nearby.sustain.propellant *= ratio;
                        nearby.sustain.thrust *= ratio;
                    }
                    1 => nearby.drag_scale *= ratio,
                    _ => {
                        nearby.guidance_ticks =
                            (f64::from(nearby.guidance_ticks) * ratio).round() as u32
                    }
                }
                for (anchor, base) in [(motor.primary, primary), (motor.secondary, secondary)] {
                    let reach = candidate_maximum(&w, nearby, anchor);
                    smooth = smooth.max((reach / base - 1.).abs());
                }
            }
        }
        println!(
            "{},{fraction:.9},{:.8},{:.6},{},{primary:.3},{},{secondary:.3},{tail:.3},{smooth:.5}",
            motor.source,
            body.drag_scale,
            f64::from(body.guidance_ticks) / 120.,
            motor.primary[2],
            motor.secondary[2]
        );
    }
    Ok(())
}

// Named, documented balance residuals. New profiles get no exemption.
// The normal band is approximately 10%, with 0.5 percentage point numerical
// allowance. Exceptions are bounded on the side of the actual compromise;
// a shortfall does not authorize a new overshoot.
fn accepted_fit_bands(source: &str) -> ([f64; 2], [f64; 2]) {
    match source {
        // John approved this guarded-motor compromise in Phase 2.7.
        "PL7.JT" => ([0.875, 1.125], [0.875, 1.125]),
        "AA10.JT" => ([0.86, 1.105], [0.895, 1.13]),
        "AA2.JT" => ([0.82, 1.105], [0.895, 1.18]),
        // John prioritized usable envelopes in the 2026-10-27 ordnance fix.
        // These bounded residuals are the agent-selected minimal guarded fits.
        "AA8.JT" => ([0.895, 1.56], [0.895, 1.85]),
        "AA12.JT" | "AEMP1.JT" => ([0.895, 1.25], [0.895, 1.105]),
        "AIM54C.JT" => ([0.895, 1.105], [0.895, 1.14]),
        "AIM7.JT" => ([0.82, 1.105], [0.895, 1.21]),
        "AIM7E.JT" => ([0.895, 1.105], [0.79, 1.105]),
        // These former overshoots now fit the normal band.
        "AIM9M.JT" | "AIM9X.JT" => ([0.895, 1.105], [0.895, 1.105]),
        _ => ([0.895, 1.105], [0.895, 1.105]),
    }
}
fn in_fit_band(actual: f64, target: f64, band: [f64; 2]) -> bool {
    actual.is_finite() && target > 0. && actual >= target * band[0] && actual <= target * band[1]
}
#[cfg(test)]
mod fit_gate_tests {
    use super::*;
    fn guarded_body() -> missiles::energy::Body {
        use missiles::energy::{Body, Motor};
        Body {
            mass: 100.,
            diameter: 0.2,
            boost: Motor {
                ticks: 240,
                thrust: 12000.,
                propellant: 10.,
            },
            sustain: Motor {
                ticks: 480,
                thrust: 1000.,
                propellant: 10.,
            },
            ignition: 0,
            life: 12000,
            guidance_ticks: 8400,
            defeat_mach: 1.,
            defeat_ticks: 30,
            structural_g: 40.,
            lift_coefficient: 12.,
            induced_factor: 0.03,
            drag_scale: 1.,
            cruise_drag: false,
            minimum_speed: 120.,
            navigation_gain: 3.,
            loft_height: 0.,
            loft_range: 0.,
            cruise_feet: None,
            launch_elevation: 0.,
        }
    }
    #[test]
    fn motor_guard_rejects_inversion_slow_launch_and_boundary_coefficients() {
        let m = tore_formats::missile_motors::motor("AIM120.JT").unwrap();
        let b = guarded_body();
        assert_eq!(motor_shape_error(m, b, 2200.), None);
        let mut bad = b;
        bad.sustain.thrust = 15000.;
        assert_eq!(
            motor_shape_error(m, bad, 2200.),
            Some("inverted boost/sustain thrust")
        );
        bad = b;
        bad.boost.thrust = 2000.;
        assert_eq!(
            motor_shape_error(m, bad, 2200.),
            Some("insufficient initial thrust/launch weight")
        );
        bad = b;
        bad.boost.thrust = 50000.;
        assert!(
            motor_shape_error(m, bad, 2200.)
                .unwrap()
                .contains("dry-mass")
        );
        assert!(
            motor_shape_error(m, b, 8800.)
                .unwrap()
                .contains("coefficient")
        );
        bad = b;
        bad.ignition = 360;
        assert!(motor_shape_error(m, bad, 2200.).unwrap().contains("Mach 1"));
    }
    #[test]
    fn air_guard_rejects_trickle_sustain_and_long_burn_without_changing_defeat() {
        let m = tore_formats::missile_motors::motor("AIM120.JT").unwrap();
        let mut b = guarded_body();
        b.sustain.thrust = 850.; // Enough on dry mass, insufficient at sustain entry.
        assert_eq!(
            motor_shape_error(m, b, 2200.),
            Some("air-target sustain below 1 g on current mass at phase start")
        );
        b.sustain.thrust = (b.mass - b.boost.propellant) * missiles::energy::G;
        assert_eq!(motor_shape_error(m, b, 2200.), None);
        b.sustain.ticks = 34 * 120; // 36 s total, against the 35 s class cap.
        assert_eq!(
            motor_shape_error(m, b, 2200.),
            Some("air-target burn exceeds half the class control time")
        );
        b = guarded_body();
        b.sustain = missiles::energy::Motor {
            ticks: 0,
            thrust: 0.,
            propellant: 0.,
        };
        assert_eq!(motor_shape_error(m, b, 2200.), None);
        let sam = tore_formats::missile_motors::motor("SA6.JT").unwrap();
        b = guarded_body();
        assert_eq!(motor_shape_error(sam, b, 2200.), None);
        b.ignition = 360;
        assert!(motor_shape_error(sam, b, 2200.).unwrap().contains("Mach 1"));
    }
    #[test]
    fn sustain_only_cruise_is_a_powered_engine_not_an_inverted_booster() {
        let m = tore_formats::missile_motors::motor("AGM84A.JT").unwrap();
        let mut b = guarded_body();
        b.cruise_drag = true;
        b.boost = missiles::energy::Motor {
            ticks: 0,
            thrust: 0.,
            propellant: 0.,
        };
        b.sustain = missiles::energy::Motor {
            ticks: 48000,
            thrust: 500.,
            propellant: 20.,
        };
        assert_eq!(actual_coefficient(b), 10000.);
        assert_eq!(motor_shape_error(m, b, 10000.), None);
        b.sustain.thrust = 100.;
        assert!(
            motor_shape_error(m, b, 10000.)
                .unwrap()
                .contains("initial thrust")
        );
    }
    #[test]
    fn an_unlisted_overshoot_is_rejected_even_when_it_is_finite() {
        assert!(!in_fit_band(120., 100., accepted_fit_bands("NEW.JT").0));
        assert!(in_fit_band(100., 100., accepted_fit_bands("NEW.JT").0));
        assert!(in_fit_band(12.8, 10., accepted_fit_bands("AA8.JT").0));
        assert!(in_fit_band(13.199, 10., accepted_fit_bands("AA8.JT").0));
        assert!(in_fit_band(15.46, 10., accepted_fit_bands("AA8.JT").0));
        assert!(!in_fit_band(15.61, 10., accepted_fit_bands("AA8.JT").0));
        assert!(in_fit_band(4.332, 2.377, accepted_fit_bands("AA8.JT").1));
        assert!(!in_fit_band(4.41, 2.377, accepted_fit_bands("AA8.JT").1));
        for source in ["AA12.JT", "AEMP1.JT"] {
            assert!(in_fit_band(62.17, 50., accepted_fit_bands(source).0));
            assert!(!in_fit_band(62.6, 50., accepted_fit_bands(source).0));
        }
        assert!(!in_fit_band(16., 10., accepted_fit_bands("AA8.JT").0));
        assert!(in_fit_band(15.695, 14., accepted_fit_bands("PL7.JT").0));
        assert!(!in_fit_band(16., 14., accepted_fit_bands("PL7.JT").0));
        assert!(in_fit_band(4.812, 5.474, accepted_fit_bands("PL7.JT").1));
        assert!(!in_fit_band(4.7, 5.474, accepted_fit_bands("PL7.JT").1));
    }
}
fn verify(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!("source,guidance_seconds,drag_scale,tail5_km,tail15_km,tail35_km,smooth_relative");
    let mut failures = Vec::new();
    let mut tails = std::collections::BTreeMap::new();
    for m in tore_formats::missile_motors::MOTORS {
        let w = Weapon::parse(m.source, &fs::read(dir.join(m.source))?)?;
        let body = missiles::energy_profile::body(&w);
        if !(0.5..=3.).contains(&body.drag_scale) || !(0.12..=0.5).contains(&m.propellant_fraction)
        {
            failures.push(format!("{}: fuel/drag outside approved bounds", m.source));
        }
        if let Some(reason) =
            motor_shape_error(m, body, m.exhaust_mps).or_else(|| cruise_arrival_error(&w, m, body))
        {
            failures.push(format!("{}: motor shape: {reason}", m.source));
        }
        let primary = candidate_maximum(&w, body, m.primary);
        if !in_fit_band(primary, m.primary[2], accepted_fit_bands(m.source).0) {
            failures.push(format!(
                "{}: unapproved primary residual: {primary:.3} vs {}",
                m.source, m.primary[2]
            ));
        }
        if missiles::surface_launched(&w.source) || w.flags & 0x20000 != 0 {
            continue;
        }
        let anchors = [
            [5000., 2., 0., 250.],
            [15000., 2., 0., 250.],
            [35000., 2., 0., 250.],
        ];
        let reach = anchors.map(|a| candidate_maximum(&w, body, a));
        let minimum = w
            .seeker
            .zones
            .iter()
            .map(|z| f64::from(z.minimum_range) * FT / 1000.)
            .fold(0f64, f64::max);
        let floor = m.control_class.tail_floor_km().expect("explicit A2A class");
        if reach[0] < floor
            || reach[0] <= 1.2 * minimum
            || reach[1] < reach[0] * 0.9
            || reach[2] < reach[1] * 0.9
        {
            failures.push(format!(
                "{}: inadequate or reversed tail reach {reach:?}",
                m.source
            ));
        }
        tails.insert(m.source, reach);
        if !in_fit_band(reach[0], m.secondary[2], accepted_fit_bands(m.source).1) {
            failures.push(format!(
                "{}: unapproved round-2 tail residual: {:.3} vs {}",
                m.source, reach[0], m.secondary[2]
            ));
        }
        if candidate_hit(&w, body, m.primary, primary * 1500.) {
            failures.push(format!(
                "{}: 150% measured head-on envelope still hits",
                m.source
            ));
        }
        let mut smooth: f64 = 0.;
        for ratio in [0.98, 1.02] {
            for field in 0..4 {
                let mut nearby = body;
                match field {
                    0 => {
                        nearby.boost.propellant *= ratio;
                        nearby.boost.thrust *= ratio;
                        nearby.sustain.propellant *= ratio;
                        nearby.sustain.thrust *= ratio;
                    }
                    1 => nearby.drag_scale *= ratio,
                    2 => {
                        nearby.guidance_ticks =
                            (f64::from(nearby.guidance_ticks) * ratio).round() as u32
                    }
                    _ => {
                        nearby.boost.ticks = (f64::from(nearby.boost.ticks) * ratio).round() as u32;
                        nearby.sustain.ticks =
                            (f64::from(nearby.sustain.ticks) * ratio).round() as u32;
                        nearby.boost.thrust /= ratio;
                        nearby.sustain.thrust /= ratio;
                    }
                }
                for (anchor, base) in [
                    (m.primary, primary),
                    (anchors[0], reach[0]),
                    (anchors[2], reach[2]),
                ] {
                    let value = candidate_maximum(&w, nearby, anchor);
                    smooth = smooth.max((value / base - 1.).abs());
                }
            }
        }
        if smooth > 0.10 {
            failures.push(format!("{}: discontinuous reach {smooth}", m.source));
        }
        println!(
            "{},{:.6},{:.4},{:.3},{:.3},{:.3},{smooth:.5}",
            m.source,
            f64::from(body.guidance_ticks) / 120.,
            body.drag_scale,
            reach[0],
            reach[1],
            reach[2]
        );
    }
    for modern in ["AIM9M.JT", "AIM9X.JT", "AA11.JT"] {
        for old in ["AIM9B.JT", "AA2.JT", "AA8.JT"] {
            if tails[modern].iter().zip(tails[old]).any(|(a, b)| *a <= b) {
                failures.push(format!(
                    "{modern} does not exceed {old} at every tail altitude"
                ));
            }
        }
    }
    // Within-family upgrades must win every measured altitude/aspect cell.
    for (new, old) in [
        ("AIM7.JT", "AIM7E.JT"),
        ("R27ER.JT", "AA10.JT"),
        ("R27ET.JT", "R27T.JT"),
    ] {
        let newer = Weapon::parse(new, &fs::read(dir.join(new))?)?;
        let older = Weapon::parse(old, &fs::read(dir.join(old))?)?;
        for altitude in [500., 5000., 15000., 35000.] {
            for aspect in [0., 1., 2.] {
                let condition = [altitude, aspect, 0., 250.];
                let a =
                    candidate_maximum(&newer, missiles::energy_profile::body(&newer), condition);
                let b =
                    candidate_maximum(&older, missiles::energy_profile::body(&older), condition);
                if a <= b {
                    failures.push(format!(
                        "{new} <= {old} at {altitude} ft aspect {aspect}: {a:.3}/{b:.3}"
                    ));
                }
            }
        }
    }
    // John's Phase 2.6 low-altitude role comparison, separate from the
    // high-head references and the within-family upgrade checks.
    for new in ["AIM120.JT", "AA12.JT", "MICA.JT"] {
        let newer = Weapon::parse(new, &fs::read(dir.join(new))?)?;
        for old in ["AIM7E.JT", "AIM7.JT", "AA10.JT", "R530.JT"] {
            let older = Weapon::parse(old, &fs::read(dir.join(old))?)?;
            for aspect in [0., 2.] {
                let condition = [5000., aspect, 0., 250.];
                let a =
                    candidate_maximum(&newer, missiles::energy_profile::body(&newer), condition);
                let b =
                    candidate_maximum(&older, missiles::energy_profile::body(&older), condition);
                if a < b {
                    failures.push(format!(
                        "5k role reversal {new}/{old} aspect {aspect}: {a}/{b}"
                    ));
                }
            }
        }
    }
    // Cross-family ordering is justified by the high-head reference, not by
    // generation alone: burn duration and drag can reverse other geometries.
    for new in ["AIM120.JT", "AA12.JT", "MICA.JT"] {
        let newer = Weapon::parse(new, &fs::read(dir.join(new))?)?;
        for old in ["AIM7E.JT", "AA6.JT", "R530.JT"] {
            let older = Weapon::parse(old, &fs::read(dir.join(old))?)?;
            let condition = [35000., 0., 0., 250.];
            if candidate_maximum(&newer, missiles::energy_profile::body(&newer), condition)
                <= candidate_maximum(&older, missiles::energy_profile::body(&older), condition)
            {
                failures.push(format!("high-head generation reversal {new}/{old}"));
            }
        }
    }
    for name in ["AIM9M.JT", "AIM9X.JT", "AA11.JT"] {
        let w = Weapon::parse(name, &fs::read(dir.join(name))?)?;
        if candidate_hit(
            &w,
            missiles::energy_profile::body(&w),
            [500., 2., 12., 250.],
            12000.,
        ) {
            failures.push(format!("{name}: 12 km deck drag did not escape"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}

fn fit_sparrow(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let w = Weapon::parse("AIM7.JT", &fs::read(dir.join("AIM7.JT"))?)?;
    let old = Weapon::parse("AIM7E.JT", &fs::read(dir.join("AIM7E.JT"))?)?;
    let original = missiles::energy_profile::body(&w);
    let mut cases = Vec::new();
    let older = missiles::energy_profile::body(&old);
    for altitude in [500., 5000., 15000., 35000.] {
        for aspect in 0..3 {
            let condition = [altitude, aspect as f64, 0., 250.];
            cases.push((condition, candidate_maximum(&old, older, condition)));
        }
    }
    println!("fraction,drag,ignition_seconds,guidance_seconds,min_advantage,score");
    for fraction in [0.35, 0.4, 0.45, 0.5] {
        for drag in [0.5, 0.75, 1., 1.5, 2., 2.5, 3.] {
            for ignition in [0f64, 0.25, 0.5, 1.] {
                let mut b = original;
                let scale = fraction
                    / tore_formats::missile_motors::motor("AIM7.JT")
                        .unwrap()
                        .propellant_fraction;
                b.boost.propellant *= scale;
                b.boost.thrust *= scale;
                b.sustain.propellant *= scale;
                b.sustain.thrust *= scale;
                b.ignition = (ignition * 120.) as u32;
                b.drag_scale = drag;
                b.guidance_ticks = b.life;
                let mut q = question(&w, 35000. * FT, 0, 70000.);
                q.body = b;
                while q.answer.is_none() {
                    q.step_over(&|_, _| 0., &|a, b| a[1] > 0. && b[1] > 0.);
                }
                let a = q.answer.unwrap();
                if !a.hit {
                    continue;
                }
                b.guidance_ticks = (a.seconds * 120.).ceil() as u32 + 1;
                let mut minimum = f64::INFINITY;
                for &(condition, reference) in &cases {
                    let reach = candidate_maximum(&w, b, condition);
                    // A zero/zero beam cell is a failure, never a NaN that
                    // f64::min silently ignores. Require an actual intercept.
                    let advantage = if reach == 0. {
                        0.
                    } else if reference == 0. {
                        f64::INFINITY
                    } else {
                        reach / reference
                    };
                    minimum = minimum.min(advantage);
                    if minimum < 1.05 {
                        break;
                    }
                }
                if minimum >= 1.05 {
                    let score = (fraction - 0.35).powi(2)
                        + 0.02 * drag.ln().powi(2)
                        + 0.01 * (1. - ignition).powi(2);
                    println!(
                        "{fraction},{drag},{ignition},{:.6},{minimum:.4},{score:.6}",
                        f64::from(b.guidance_ticks) / 120.
                    );
                }
            }
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let dir = Path::new(args.get(1).ok_or("JT directory required")?);
    if args
        .get(2)
        .is_some_and(|s| matches!(s.as_str(), "--refine-motor" | "--measure-motor"))
    {
        let source = args
            .get(3)
            .ok_or("--refine-motor SOURCE FUEL DRAG BOOST_S SUSTAIN_S COEFFICIENT BOOST_SHARE")?;
        let m = tore_formats::missile_motors::motor(source).ok_or("unknown motor source")?;
        let mut seed = [0f64; 6];
        for (i, v) in seed.iter_mut().enumerate() {
            *v = args.get(i + 4).ok_or("missing refinement value")?.parse()?;
        }
        if seed.iter().any(|v| !v.is_finite())
            || !(0.12..=0.5).contains(&seed[0])
            || !(0.5..=3.).contains(&seed[1])
            || seed[2] < 0.
            || seed[3] < 0.
            || seed[2] + seed[3] <= 0.
            || seed[4] <= 0.
            || !(0. ..=1.).contains(&seed[5])
        {
            return Err("invalid motor refinement seed".into());
        }
        println!("{MOTOR_FIT_HEADER}");
        print!(
            "{}",
            fit_motor_store::<6>(
                dir,
                m,
                Some(seed),
                if args[2] == "--measure-motor" { 0 } else { 12 }
            )?
        );
        return Ok(());
    }
    if args.get(2).is_some_and(|s| s == "--motor-shapes") {
        return motor_shapes(dir);
    }
    if args.get(2).is_some_and(|s| s == "--diagnostics") {
        return diagnostics(dir);
    }
    if args.get(2).is_some_and(|s| s == "--fit-staged") {
        return fit_motors::<6>(dir, args.get(3).map(String::as_str));
    }
    if args.get(2).is_some_and(|s| s == "--fit-motors") {
        return fit_motors::<4>(dir, args.get(3).map(String::as_str));
    }
    if args.get(2).is_some_and(|s| s == "--fit-ground") {
        return fit_class(dir, args.get(3).map(String::as_str), true);
    }
    if args.get(2).is_some_and(|s| s == "--fit-class") {
        return fit_class(dir, args.get(3).map(String::as_str), false);
    }
    if args
        .get(2)
        .is_some_and(|s| s == "--fit-pair" || s == "--fit-sparrow")
    {
        return Err("Timer fitting retired. Use --fit-class; historical reproduction only: --legacy-fit-pair or --legacy-fit-sparrow".into());
    }
    if args.get(2).is_some_and(|s| s.starts_with("--legacy-")) {
        eprintln!("LEGACY TIMER-FITTED CALIBRATION, NOT THE CURRENT MODEL");
    }
    if args.get(2).is_some_and(|s| s == "--legacy-fit-sparrow") {
        return fit_sparrow(dir);
    }
    if args.get(2).is_some_and(|s| s == "--verify") {
        return verify(dir);
    }
    if args.get(2).is_some_and(|s| s == "--point") {
        let source = args
            .get(3)
            .ok_or("--point SOURCE ALTITUDE_FT ASPECT RANGE_M")?;
        let alt = args.get(4).ok_or("altitude required")?.parse::<f64>()? * FT;
        let aspect = match args.get(5).map(String::as_str) {
            Some("head") => 0,
            Some("beam") => 1,
            Some("tail") => 2,
            Some("stationary") => 3,
            _ => return Err("head, beam, tail or stationary required".into()),
        };
        let range = args
            .get(6)
            .ok_or("range in meters required")?
            .parse::<f64>()?;
        if !alt.is_finite() || alt < 0. || !range.is_finite() || range <= 0. {
            return Err("finite nonnegative altitude and positive range required".into());
        }
        let w = Weapon::parse(source, &fs::read(dir.join(source))?)?;
        println!(
            "source={source} range_m={range} physical_hit={} seeker_hit={}",
            hit(&w, alt, aspect, range, None),
            seeker_hit(&w, alt, aspect, range, None)
        );
        return Ok(());
    }
    if args
        .iter()
        .any(|s| s == "--legacy-fit-pair" || s == "--measure-pairs")
    {
        fit_pairs(
            dir,
            args.get(3).map(String::as_str),
            args[2] == "--measure-pairs",
        )?;
        return Ok(());
    }
    println!(
        "source,altitude_ft,aspect,physical_km,seeker_valid_km,anchor_km,drag_scale,retail_lb,physical_lb,anchor_evidence"
    );
    for source in tore_formats::missile_data::REFERENCES
        .iter()
        .map(|r| r.source)
        .chain(
            tore_formats::missile_data::FALLBACKS
                .iter()
                .map(|r| r.source),
        )
    {
        let Ok(bytes) = fs::read(dir.join(source)) else {
            continue;
        };
        let w = Weapon::parse(source, &bytes)?;
        let body = missiles::energy_profile::body(&w);
        let row = tore_formats::missile_data::reference(source)
            .copied()
            .unwrap_or(tore_formats::missile_data::Reference {
                source,
                mass_kg: body.mass,
                diameter_m: body.diameter,
                range_km: f64::from(w.seeker.zones[1].maximum_range) * FT / 1000.,
                dimensions_evidence: "RETAIL-AMBIGUOUS",
                range_evidence: "RETAIL-AMBIGUOUS",
                drag_scale: body.drag_scale,
                imaging: false,
                structural_g: None,
                tracking_deg_s: None,
                fov_half_deg: None,
            });
        let surface = missiles::surface_launched(&w.source);
        let strike = w.flags & 0x20000 != 0;
        if w.movement.fuel_t <= w.movement.ignite_t {
            continue;
        }
        if args.iter().any(|a| a == "--data") {
            eprintln!(
                "{} flags={:x} rawfuel={} body={:?}",
                w.source,
                w.flags,
                w.movement.fuel_t,
                missiles::energy_profile::body(&w)
            );
            continue;
        }
        let drag = None;
        for altitude in if surface || strike {
            vec![if surface {
                (row.range_km * 200.).clamp(300., 3000.) / FT
            } else if tore_sim::combat::guided::kind(&w) == Some(missiles::Guidance::Command) {
                100. / FT
            } else {
                1500. / FT
            }]
        } else {
            vec![500., 5000., 15000., 35000.]
        } {
            for aspect in 0..if strike {
                1
            } else if surface {
                4
            } else {
                3
            } {
                let max = maximum(&w, altitude * FT, aspect, drag, false);
                let valid = if missiles::Profile::for_weapon(&w).is_some() {
                    format!(
                        "{:.3}",
                        maximum(&w, altitude * FT, aspect, drag, true) / 1000.
                    )
                } else {
                    String::new()
                };
                println!(
                    "{},{altitude:.0},{},{:.3},{valid},{},{:.8},{},{:.4},{}",
                    row.source,
                    ["head", "beam", "tail", "stationary"][aspect],
                    max / 1000.,
                    row.range_km,
                    body.drag_scale,
                    w.weight,
                    row.mass_kg / 0.45359237,
                    row.range_evidence
                );
            }
        }
    }
    Ok(())
}

fn powered_cruise(m: &tore_formats::missile_motors::MotorData, b: missiles::energy::Body) -> bool {
    matches!(
        m.control_class,
        tore_formats::missile_motors::ControlClass::Cruise
            | tore_formats::missile_motors::ControlClass::ShortCruise
    ) && b.cruise_drag
}
fn minimum_boost_g(m: &tore_formats::missile_motors::MotorData, b: missiles::energy::Body) -> f64 {
    use tore_formats::missile_motors::ControlClass::*;
    match m.control_class {
        ShortIr | MediumAir | LongAir | SurfaceIr | SurfaceRadar | SurfaceRadarLong => 8.,
        FastGround | BoostGlide => 4.,
        ShortGround => 1.,
        Command => 2.,
        Cruise | ShortCruise => {
            if b.boost.ticks == 0 {
                0.25
            } else {
                4.
            }
        }
    }
}
fn fit_burn_seconds(m: &tore_formats::missile_motors::MotorData, b: missiles::energy::Body) -> f64 {
    let available = f64::from(b.guidance_ticks - b.ignition) * missiles::energy::DT;
    if m.control_class.air_target() {
        available.min(m.control_class.seconds() * 0.5)
    } else if powered_cruise(m, b) {
        // Cover the last controlled contact tick, including the end-of-step age.
        available + 1.
    } else {
        available
    }
}
fn sustain_g(b: missiles::energy::Body) -> f64 {
    b.sustain.thrust / ((b.mass - b.boost.propellant) * missiles::energy::G)
}
fn actual_coefficient(b: missiles::energy::Body) -> f64 {
    use missiles::energy::DT;
    (b.boost.thrust * f64::from(b.boost.ticks) + b.sustain.thrust * f64::from(b.sustain.ticks)) * DT
        / (b.boost.propellant + b.sustain.propellant)
}
/// Authored interior guard, leaving headroom to the old 8,800 m/s / 60 G extremes.
/// Sustain-only air-launched cruise engines have no fictitious zero-thrust booster.
fn motor_shape_error(
    m: &tore_formats::missile_motors::MotorData,
    b: missiles::energy::Body,
    coefficient: f64,
) -> Option<&'static str> {
    use missiles::energy::G;
    if !b.valid() {
        return Some("invalid physical body");
    }
    let first = if b.boost.ticks == 0 {
        b.sustain.thrust
    } else {
        b.boost.thrust
    };
    let jet = powered_cruise(m, b) && b.boost.ticks == 0;
    let bounds = if jet { 4000. ..=32000. } else { 400. ..=6600. };
    if b.boost.ticks > 0 && b.boost.thrust < b.sustain.thrust {
        return Some("inverted boost/sustain thrust");
    }
    if first / (b.mass * G) < minimum_boost_g(m, b) {
        return Some("insufficient initial thrust/launch weight");
    }
    if m.control_class.air_target() {
        if f64::from(b.boost.ticks + b.sustain.ticks) * missiles::energy::DT
            > m.control_class.seconds() * 0.5 + 1e-9
        {
            return Some("air-target burn exceeds half the class control time");
        }
        if b.sustain.ticks > 0 && sustain_g(b) < 1. {
            return Some("air-target sustain below 1 g on current mass at phase start");
        }
    }
    if b.boost.thrust.max(b.sustain.thrust) / (b.dry_mass() * G) > 54. {
        return Some("dry-mass axial load lacks 10% headroom to 60 G");
    }
    if !bounds.contains(&coefficient) || !bounds.contains(&actual_coefficient(b)) {
        return Some("impulse coefficient outside interior guard");
    }
    if m.control_class.air_target() && mach_one_seconds(b).is_none_or(|t| t > 3.) {
        return Some("Mach 1 takes more than 3 s from 250 m/s at 5k ft");
    }
    None
}
fn cruise_arrival_error(
    w: &Weapon,
    m: &tore_formats::missile_motors::MotorData,
    b: missiles::energy::Body,
) -> Option<&'static str> {
    if !powered_cruise(m, b) {
        return None;
    }
    let edge = candidate_maximum(w, b, m.primary);
    for distance in [
        m.primary[2] * 0.5,
        m.primary[2] * 0.75,
        m.primary[2],
        edge * 0.75,
        edge,
    ] {
        let q = arrival_trial(w, b, m.primary, distance);
        let mach = dot(q.flight.velocity, q.flight.velocity).sqrt()
            / missiles::energy::atmosphere(q.flight.position[1]).sound;
        if !q.answer.unwrap().hit {
            return Some("cruise misses a reference/intermediate/edge point");
        }
        if q.flight.age >= b.ignition + b.boost.ticks + b.sustain.ticks {
            return Some("cruise coasts before its measured reach edge");
        }
        if !(0.7..=0.95).contains(&mach) {
            return Some("cruise reference/intermediate/edge speed outside Mach 0.70..0.95");
        }
    }
    None
}
/// Uniform offline acceleration and reference-arrival characterization.
fn mach_one_seconds(body: missiles::energy::Body) -> Option<f64> {
    use missiles::energy::{DT, Flight, atmosphere};
    let mut f = Flight::launch([0., 5000. * FT, 0.], [0., 0., 250.], [0., 0., 1.]);
    let target = Some(missiles::energy::Observation {
        position: [0., 5000. * FT, 100_000.],
        velocity: [0.; 3],
    });
    for _ in 0..2400 {
        f.step(body, [0.; 3], target);
        let speed = dot(f.velocity, f.velocity).sqrt();
        if speed >= atmosphere(f.position[1]).sound {
            return Some(f64::from(f.age) * DT);
        }
    }
    None
}
fn arrival_trial(
    w: &Weapon,
    body: missiles::energy::Body,
    condition: [f64; 4],
    range_km: f64,
) -> Trial {
    let mut q = question(
        w,
        condition[0] * FT,
        condition[1] as usize,
        range_km * 1000.,
    );
    q.body = body;
    q.target.velocity = match condition[1] as usize {
        0 => [0., 0., -condition[3]],
        1 => [condition[3], 0., 0.],
        2 => [0., 0., condition[3]],
        _ => [0.; 3],
    };
    while q.answer.is_none() {
        q.step(&|_, _| 0.);
    }
    q
}
fn motor_shapes(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use missiles::energy::{DT, G, atmosphere};
    println!(
        "source,class,mass_kg,fuel,drag,boost_n,boost_s,sustain_n,sustain_s,boost_g,dry_g,coefficient,actual_coefficient,mach1_s,reference_hit,reference_s,reference_mach,reference_powered,reach_km,edge_hit,edge_s,edge_mps,edge_mach,edge_powered,peak_mach,cruise_drag,control_s,burn_s,burn_fraction,sustain_start_g,powered_limit_km,cruise_half_mach,ignition_s,burnout_s"
    );
    for m in tore_formats::missile_motors::MOTORS {
        let w = Weapon::parse(m.source, &fs::read(dir.join(m.source))?)?;
        let b = missiles::energy_profile::body(&w);
        let reference = arrival_trial(&w, b, m.primary, m.primary[2]);
        let ref_mach = dot(reference.flight.velocity, reference.flight.velocity).sqrt()
            / atmosphere(reference.flight.position[1]).sound;
        let reach = candidate_maximum(&w, b, m.primary);
        let mut q = question(&w, m.primary[0] * FT, m.primary[1] as usize, reach * 1000.);
        q.body = b;
        q.target.velocity = match m.primary[1] as usize {
            0 => [0., 0., -m.primary[3]],
            1 => [m.primary[3], 0., 0.],
            2 => [0., 0., m.primary[3]],
            _ => [0.; 3],
        };
        let mut peak: f64 = 0.;
        while q.answer.is_none() {
            q.step(&|_, _| 0.);
            peak = peak.max(
                dot(q.flight.velocity, q.flight.velocity).sqrt()
                    / atmosphere(q.flight.position[1]).sound,
            );
        }
        let a = q.answer.unwrap();
        let v = dot(q.flight.velocity, q.flight.velocity).sqrt();
        let burnout = b.ignition + b.boost.ticks + b.sustain.ticks;
        let control_s = m.control_class.seconds();
        let burn_s = f64::from(b.boost.ticks + b.sustain.ticks) * DT;
        let burn_fraction = burn_s / control_s;
        let ignition_s = f64::from(b.ignition) * DT;
        let burnout_s = ignition_s + burn_s;
        let sustain_start_g = sustain_g(b);
        let powered_limit = if powered_cruise(m, b) {
            format!("{:.6}", candidate_maximum_kind(&w, b, m.primary, 12, true))
        } else {
            String::new()
        };
        let cruise_half_mach = if powered_cruise(m, b) {
            let q = arrival_trial(&w, b, m.primary, m.primary[2] * 0.5);
            format!(
                "{:.6}",
                dot(q.flight.velocity, q.flight.velocity).sqrt()
                    / atmosphere(q.flight.position[1]).sound
            )
        } else {
            String::new()
        };
        println!(
            "{},{:?},{:.6},{:.9},{:.9},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6},{},{},{:.6},{:.6},{},{:.6},{},{:.6},{:.6},{:.6},{},{peak:.6},{},{control_s},{burn_s:.6},{burn_fraction:.6},{sustain_start_g:.6},{powered_limit},{cruise_half_mach},{ignition_s:.6},{burnout_s:.6}",
            m.source,
            m.control_class,
            b.mass,
            m.propellant_fraction,
            b.drag_scale,
            b.boost.thrust,
            f64::from(b.boost.ticks) * DT,
            b.sustain.thrust,
            f64::from(b.sustain.ticks) * DT,
            b.boost.thrust / (b.mass * G),
            b.boost.thrust.max(b.sustain.thrust) / (b.dry_mass() * G),
            m.exhaust_mps,
            actual_coefficient(b),
            mach_one_seconds(b).map_or(String::new(), |s| format!("{s:.6}")),
            reference.answer.unwrap().hit,
            reference.answer.unwrap().seconds,
            ref_mach,
            reference.flight.age < burnout,
            reach,
            a.hit,
            a.seconds,
            v,
            v / atmosphere(q.flight.position[1]).sound,
            q.flight.age < burnout,
            b.cruise_drag
        );
    }
    Ok(())
}
