//! Synthetic kernel/queue cost probe, not the eight-aircraft world acceptance.
use std::{hint::black_box, time::Instant};
use tore_sim::combat::missiles::{
    energy::{Body, Flight, Motor, Observation},
    energy_estimate::{Queue, Request, Trial},
};

fn body() -> Body {
    Body {
        mass: 150.,
        diameter: 0.18,
        boost: Motor {
            ticks: 360,
            thrust: 18000.,
            propellant: 30.,
        },
        sustain: Motor {
            ticks: 600,
            thrust: 6000.,
            propellant: 20.,
        },
        ignition: 0,
        life: 24000,
        defeat_mach: 0.,
        defeat_ticks: 30,
        guidance_ticks: 120 * 120,
        structural_g: 30.,
        lift_coefficient: 12.,
        induced_factor: 0.03,
        drag_scale: 1.,
        cruise_drag: false,
        minimum_speed: 120.,
        navigation_gain: 3.5,
        loft_height: 0.,
        loft_range: 0.,
        cruise_feet: None,
        launch_elevation: -std::f64::consts::FRAC_PI_2,
    }
}
fn trial(id: u64) -> Trial {
    let target = Observation {
        position: [(id % 8) as f64 * 300., 5000., 20000.],
        velocity: [
            if id.is_multiple_of(3) { 200. } else { 0. },
            0.,
            if id % 3 == 1 { -250. } else { 250. },
        ],
    };
    Trial::new(
        body(),
        Flight::launch([0., 5000., 0.], [0., 0., 300.], [0., 0., 1.]),
        target,
        30.,
        0,
    )
}
fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[((samples.len() - 1) as f64 * p).round() as usize]
}
fn main() {
    if std::env::args().any(|a| a == "--guidance") {
        println!("altitude_m,range_m,aspect,hit,seconds,closest_m,impact_mps");
        for altitude in [1500., 4500., 10668.] {
            for range in [10000., 20000., 30000., 40000., 60000.] {
                for (name, velocity) in [
                    ("head", [0., 0., -250.]),
                    ("beam", [250., 0., 0.]),
                    ("tail", [0., 0., 250.]),
                ] {
                    let result = Trial::new(
                        body(),
                        Flight::launch([0., altitude, 0.], [0., 0., 300.], [0., 0., 1.]),
                        Observation {
                            position: [0., altitude, range],
                            velocity,
                        },
                        30.,
                        0,
                    )
                    .finish(&|_, _| 0.);
                    println!(
                        "{altitude},{range},{name},{},{:.3},{:.3},{:.3}",
                        result.hit, result.seconds, result.closest, result.impact_speed
                    );
                }
            }
        }
        return;
    }
    let mut q = Queue::default();
    for id in 0..24 {
        q.submit(Request {
            id,
            observed_tick: 0,
            trial: trial(id),
        });
    }
    let mut cold = Vec::new();
    let mut steps = 0;
    let mut completion = 0;
    for tick in 0..1000 {
        let started = Instant::now();
        let used = black_box(q.advance(&|_, _| 0.));
        cold.push(started.elapsed().as_secs_f64() * 1000.);
        steps += used;
        if q.requests.iter().all(|r| r.trial.answer.is_some()) {
            completion = tick + 1;
            break;
        }
    }
    let mut warm = Vec::new();
    for _ in 0..1000 {
        let started = Instant::now();
        for id in 0..24 {
            black_box(q.answer(id));
        }
        warm.push(started.elapsed().as_secs_f64() * 1000.);
    }
    println!("workload,calls,p50_ms,p95_ms,max_ms,work_units,completion_world_ticks");
    let p50 = percentile(&mut cold, 0.5);
    let p95 = percentile(&mut cold, 0.95);
    println!(
        "synthetic_24_pending,{},{p50:.6},{p95:.6},{:.6},{steps},{completion}",
        cold.len(),
        cold.last().unwrap()
    );
    let p50 = percentile(&mut warm, 0.5);
    let p95 = percentile(&mut warm, 0.95);
    println!(
        "synthetic_24_cached,{},{p50:.6},{p95:.6},{:.6},0,0",
        warm.len(),
        warm.last().unwrap()
    );
    let start = Instant::now();
    for id in 0..24 {
        black_box(trial(id).finish(&|_, _| 0.));
    }
    println!(
        "unbounded_24_total_ms,{:.6}",
        start.elapsed().as_secs_f64() * 1000.
    );
}
