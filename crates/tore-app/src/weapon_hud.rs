//! Manual-style weapon symbols with fitted layout and simulation-owned state.
use crate::{
    aim_box, combat, flight,
    frame::FlightFrame,
    hud::{self, Paint},
};
use tore_formats::font::Font;
use tore_formats::text::GlyphCodes;
use tore_sim::{
    attitude::{Basis, Vector, dot},
    combat::{
        gunsight, live,
        missiles::{self, Guidance, LaunchMode, seeker::Status},
    },
};
use tore_world::readout::CockpitReadout;

mod agm;
mod bombs;
mod guided;
pub mod laser;

/// Reserve the weapon readout area even for a safe gun, keeping flight text clear.
pub fn active(frame: &FlightFrame) -> bool {
    let Some(station) = frame.config.stations.get(frame.readout.stores.selected()) else {
        return false;
    };
    let weapon = &station.weapon;
    live::is_gun(weapon)
        || (frame.readout.stores.armed && missiles::Profile::for_weapon(weapon).is_some())
        || frame.readout.guided.is_some()
        || frame.readout.bomb_sight.is_some()
}

fn debug_point(point: (f64, f64), size: [f64; 2]) -> (f64, f64) {
    let scale = (size[0] / 640.).min(size[1] / 480.);
    (
        (point.0 - size[0] + 258. * scale) / scale,
        (point.1 - 8. * scale) / scale,
    )
}
pub fn mode_hit(point: (f64, f64), size: [f64; 2]) -> bool {
    let (x, y) = debug_point(point, size);
    (4. ..130.).contains(&x) && (4. ..18.).contains(&y)
}
pub fn release_hit(point: (f64, f64), size: [f64; 2]) -> bool {
    let (x, y) = debug_point(point, size);
    (132. ..246.).contains(&x) && (4. ..18.).contains(&y)
}
fn diamond_visible(locked: bool, radar_in_range: bool, tick: u64) -> bool {
    locked && (!radar_in_range || tick % 60 < 30)
}
fn projected(direction: Vector, s: &flight::State, zoom: f64) -> Option<(f64, f64)> {
    let bearing = direction[0].atan2(direction[2]) - s.yaw;
    let elevation = direction[1].atan2(direction[0].hypot(direction[2]));
    hud::project(s.pitch, s.bank, bearing, elevation, zoom)
}
/// The boundary uses the exact independent heading/elevation tests of the seeker.
fn boundary(basis: Basis, horizontal: f64, vertical: f64) -> Vec<Vector> {
    let mut points = Vec::new();
    for edge in 0..4 {
        for n in 0..=24 {
            let t = f64::from(n) / 24.;
            let (h, v) = match edge {
                0 => (-horizontal + 2. * horizontal * t, -vertical),
                1 => (horizontal, -vertical + 2. * vertical * t),
                2 => (horizontal - 2. * horizontal * t, vertical),
                _ => (-horizontal, vertical - 2. * vertical * t),
            };
            points.push(std::array::from_fn(|i| {
                basis.forward[i] * h.cos() * v.cos()
                    + basis.right[i] * h.sin() * v.cos()
                    + basis.up[i] * v.sin()
            }));
        }
    }
    points
}
#[allow(clippy::too_many_arguments)]
pub fn draw(
    pixels: &mut [u8],
    frame: &FlightFrame,
    font: &Font,
    color: [u8; 3],
    zoom: f64,
    target_friendly: bool,
    target_cue: bool,
    aim: Option<&aim_box::Marks>,
    gun_key: bool,
) {
    let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Hud);
    let s = frame.presented();
    let ro = &frame.readout;
    if target_cue {
        // The AC-130's box follows the aim point; every other aircraft's cue
        // follows the displayed target.
        match aim {
            Some(aim) => draw_aim(pixels, frame, aim, color, zoom),
            None => draw_target(pixels, frame, color, zoom, target_friendly),
        }
    }
    draw_assigned(pixels, frame, color, zoom);
    if ro.airport.nav_mode {
        Paint {
            pixels,
            clip: hud::HUD_CLIP,
            color: [color[0], color[1], color[2], 255],
        }
        .text(font, "NAV", 207, 259);
        return;
    }
    // The laser's box and text, whatever is selected (the sight designates
    // before and after a weapon is chosen).
    laser::draw(pixels, s, ro, font, color, zoom);
    let Some(station) = frame.config.stations.get(ro.stores.selected()) else {
        return;
    };
    if live::is_gun(&station.weapon) {
        draw_gun(pixels, frame, font, color, zoom, ro.stores.selected());
        return;
    }
    // Tab held with another store selected: the gun's sight and count take
    // the weapon area while the key is down (agent decision, FX-T).
    if gun_key && let Some(index) = live::key_gun_station(frame.config, |i| ro.stores.rounds(i)) {
        draw_gun(pixels, frame, font, color, zoom, index);
        return;
    }
    if !active(frame) {
        return;
    }
    let w = &station.weapon;
    if let Some(sight) = &ro.guided {
        let mut paint = Paint {
            pixels,
            clip: hud::HUD_CLIP,
            color: [color[0], color[1], color[2], 255],
        };
        guided::draw(&mut paint, frame, font, w, sight, zoom);
        return;
    }
    if let Some(sight) = &ro.bomb_sight {
        bombs::draw(pixels, frame, font, color, zoom, w, sight);
        return;
    }
    let Some(profile) = missiles::Profile::for_weapon(w) else {
        return;
    };
    let l = combat::launcher(s);
    let mut paint = Paint {
        pixels,
        clip: hud::HUD_CLIP,
        color: [color[0], color[1], color[2], 255],
    };
    let bore = ro.estimates.guidance_available && ro.stores.launch_mode == LaunchMode::Boresight;
    // The HARM cue shows its emitters in place of the bore circle.
    let harm = agm::is_harm(profile);
    let cap = if bore {
        profile.search_cap()
    } else {
        std::f64::consts::PI
    };
    let zone = &w.seeker.zones[0];
    let cone = if !ro.estimates.guidance_available || harm {
        Vec::new()
    } else if bore {
        let angle = cap
            .min(missiles::half_angle(zone.heading))
            .min(missiles::half_angle(zone.pitch));
        (0..=96)
            .map(|i| {
                let a = f64::from(i) * std::f64::consts::TAU / 96.;
                std::array::from_fn(|j| {
                    l.basis.forward[j] * angle.cos()
                        + (l.basis.right[j] * a.cos() + l.basis.up[j] * a.sin()) * angle.sin()
                })
            })
            .collect()
    } else {
        boundary(
            l.basis,
            missiles::half_angle(zone.heading),
            missiles::half_angle(zone.pitch),
        )
    };
    for pair in cone.windows(2) {
        if let (Some(a), Some(b)) = (projected(pair[0], s, zoom), projected(pair[1], s, zoom)) {
            paint.line(a, b);
        }
    }
    let ring = projected(l.basis.forward, s, zoom);
    if let Some((x, y)) = ring {
        if bore && !harm {
            let radius = 240. * 3f64.sqrt() * cap.tan() * zoom;
            paint.text(font, "BORE", x as i32 - 12, (y - radius + 5.) as i32);
        }
        for i in 0..32 {
            let a = f64::from(i) * std::f64::consts::TAU / 32.;
            let b = f64::from(i + 1) * std::f64::consts::TAU / 32.;
            paint.line(
                (x + 9. * a.cos(), y + 9. * a.sin()),
                (x + 9. * b.cos(), y + 9. * b.sin()),
            );
        }
    }
    if harm {
        agm::draw_harm(&mut paint, frame, font, zoom, ring);
    }
    let observed = ro.estimates.observation;
    let in_range = ro.estimates.in_range;
    if let Some(observation) = observed {
        let position = observation.position;
        if let Some((x, y)) = projected(missiles::sub(position, s.position), s, zoom) {
            let radar_in_range =
                matches!(profile.guidance, Guidance::Active | Guidance::Supported) && in_range;
            if diamond_visible(
                bore || matches!(ro.seeker.status, Status::Locked | Status::Pitbull),
                bore || radar_in_range,
                ro.sensors.tick,
            ) {
                for (a, b) in [
                    ((0., -7.), (7., 0.)),
                    ((7., 0.), (0., 7.)),
                    ((0., 7.), (-7., 0.)),
                    ((-7., 0.), (0., -7.)),
                ] {
                    paint.line((x + a.0, y + a.1), (x + b.0, y + b.1));
                }
            }
        }
        let range = missiles::length(missiles::sub(position, s.position));
        let min = ro
            .estimates
            .min_range
            .unwrap_or(f64::from(w.seeker.zones[1].minimum_range));
        let max = ro.estimates.max_range.unwrap_or(0.);
        // Retail reference: range scale just inside the altitude tape.
        let text_width = |text: &str| {
            text.glyph_codes()
                .map(|c| font.glyphs[c as usize].advance)
                .sum::<usize>() as i32
        };
        let right = 390;
        let x = f64::from(right);
        let (top, bottom) = (230., 282.);
        paint.line((x, top), (x, bottom));
        paint.line((x - 5., top), (x, top));
        paint.line((x - 5., bottom), (x, bottom));
        let maximum = if max > min {
            format!("{:.1}", max / missiles::NMI)
        } else {
            "--".into()
        };
        let minimum = format!("{:.1}", min / missiles::NMI);
        paint.text(font, &maximum, right - text_width(&maximum), 218);
        paint.text(font, &minimum, right - text_width(&minimum), 284);
        if max > min
            && let Some(band) = ro.estimates.band
        {
            let scale_y =
                |range: f64| bottom - (bottom - top) * ((range - min) / (max - min)).clamp(0., 1.);
            let upper = scale_y(band.maximum);
            let lower = scale_y(band.minimum);
            paint.line((x - 6., upper), (x, upper));
            paint.line((x - 6., lower), (x, lower));
        }
        if max > min && (!bore || ro.sensors.tick % 60 < 30) {
            let y = bottom - (bottom - top) * ((range - min) / (max - min)).clamp(0., 1.);
            paint.line((x - 8., y - 3.), (x - 2., y));
            paint.line((x - 2., y), (x - 8., y + 3.));
            paint.line((x - 8., y + 3.), (x - 8., y - 3.));
        }
    }
    paint.text(font, "ARM", 207, 259);
    paint.text(
        font,
        &format!("{} {}", ro.stores.rounds(ro.stores.selected()), w.hud_name),
        207,
        271,
    );
    let ready = ro.estimates.readiness;
    let percent = if ro.estimates.max_range.is_some() {
        format!("{}%", ro.estimates.hit_percent)
    } else {
        "--%".into()
    };
    paint.text(font, &percent, 207, 283);
    if in_range && ro.sensors.tick % 60 < 30 {
        let width: usize = percent
            .glyph_codes()
            .map(|c| font.glyphs[c as usize].advance)
            .sum();
        paint.text(font, "IN RNG", 211 + width as i32, 283);
    }
    if !matches!(
        ready,
        live::Readiness::Ready | live::Readiness::TargetDestroyed
    ) {
        paint.text(font, ready.label(), 207, 295);
    }
    if matches!(profile.guidance, Guidance::Active | Guidance::Supported)
        && let Some(o) = observed
    {
        let closure = missiles::closure(s.position, s.velocity, o.position, o.velocity) / 1.68781;
        paint.text(font, &format!("R {:.1}", o.range / missiles::NMI), 402, 259);
        paint.text(font, &format!("C {closure:+.0}"), 402, 271);
        let aspect = if missiles::length(o.velocity) > 1e-9 {
            let forward = tore_sim::attitude::unit(o.velocity);
            let los = tore_sim::attitude::unit(missiles::sub(s.position, o.position));
            let angle = dot(forward, los).clamp(-1., 1.).acos().to_degrees();
            let side = if forward[2] * los[0] - forward[0] * los[2] >= 0. {
                "R"
            } else {
                "L"
            };
            format!("A {angle:.0}{side}")
        } else {
            "A --".into()
        };
        paint.text(font, &aspect, 402, 283);
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum TargetCue {
    Square((f64, f64)),
    Chevron {
        point: (f64, f64),
        direction: (f64, f64),
    },
}

/// The rear hemisphere keeps the bearing's sign instead of perspective-flipping.
fn target_cue(direction: Vector, basis: Basis, zoom: f64) -> Option<TargetCue> {
    if !direction.iter().all(|v| v.is_finite()) || dot(direction, direction) < 1e-12 {
        return None;
    }
    let x = dot(direction, basis.right);
    let y = -dot(direction, basis.up);
    let z = dot(direction, basis.forward);
    if z > 1e-9 {
        let focal = 240. * 3f64.sqrt() * zoom;
        let point = (320. + focal * x / z, 240. + focal * y / z);
        if (184. ..=456.).contains(&point.0)
            && (106. ..=f64::from(hud::AIM_BOTTOM - 10)).contains(&point.1)
        {
            return Some(TargetCue::Square(point));
        }
    }
    let length = x.hypot(y);
    let (dx, dy) = if length < 1e-9 {
        (1., 0.)
    } else {
        (x / length, y / length)
    };
    let tx = if dx.abs() < 1e-12 {
        f64::INFINITY
    } else {
        136. / dx.abs()
    };
    let ty = if dy.abs() < 1e-12 {
        f64::INFINITY
    } else if dy > 0. {
        (f64::from(hud::AIM_BOTTOM - 10) - 240.) / dy
    } else {
        -134. / dy
    };
    let distance = tx.min(ty);
    Some(TargetCue::Chevron {
        point: (320. + dx * distance, 240. + dy * distance),
        direction: (dx, dy),
    })
}
/// Whether the HUD draws the selected target's square itself, rather than an
/// edge arrow.
pub fn target_in_hud(frame: &FlightFrame, zoom: f64) -> bool {
    frame
        .readout
        .targets
        .display
        .as_ref()
        .is_some_and(|target| point_in_hud(frame, target.position, zoom))
}
/// Whether a world point falls inside the HUD's own square cue region, where
/// the HUD draws a box itself rather than an edge arrow.
pub fn point_in_hud(frame: &FlightFrame, position: Vector, zoom: f64) -> bool {
    let s = frame.presented();
    matches!(
        target_cue(
            missiles::sub(position, s.position),
            Basis::new(s.yaw, s.pitch, s.bank),
            zoom,
        ),
        Some(TargetCue::Square(_))
    )
}
fn draw_target(pixels: &mut [u8], frame: &FlightFrame, color: [u8; 3], zoom: f64, friendly: bool) {
    let s = frame.presented();
    let Some(target) = &frame.readout.targets.display else {
        return;
    };
    replay_target(pixels, s, target.position, color, zoom, friendly);
}

/// The AC-130's aim-point box: the target cue, in the style of what the sight
/// holds, at the aim point.
fn draw_aim(
    pixels: &mut [u8],
    frame: &FlightFrame,
    aim: &aim_box::Marks,
    color: [u8; 3],
    zoom: f64,
) {
    cue(pixels, frame.presented(), aim.aim, color, zoom, aim.style);
}

/// Same target cue, with position supplied by a recorded AI target selection.
pub(crate) fn replay_target(
    pixels: &mut [u8],
    s: &flight::State,
    position: Vector,
    color: [u8; 3],
    zoom: f64,
    friendly: bool,
) {
    cue(
        pixels,
        s,
        position,
        color,
        zoom,
        aim_box::Style::Tracked { friendly },
    );
}

fn cue(
    pixels: &mut [u8],
    s: &flight::State,
    position: Vector,
    color: [u8; 3],
    zoom: f64,
    style: aim_box::Style,
) {
    let Some(cue) = target_cue(
        missiles::sub(position, s.position),
        Basis::new(s.yaw, s.pitch, s.bank),
        zoom,
    ) else {
        return;
    };
    let mut paint = Paint {
        pixels,
        clip: (174, 96, 292, hud::AIM_BOTTOM - 96),
        color: [color[0], color[1], color[2], 255],
    };
    match cue {
        TargetCue::Square((x, y)) => {
            draw_target_box(&mut paint, (x, y), style);
        }
        TargetCue::Chevron {
            point: (x, y),
            direction: (dx, dy),
        } => {
            for side in [-1., 1.] {
                paint.line(
                    (x - dx * 8. - dy * side * 4., y - dy * 8. + dx * side * 4.),
                    (x, y),
                );
            }
        }
    }
}
/// The target to wear the assignment brackets now: the one the lead assigned
/// to the plane, while the plane has not locked it and the blink is lit.
/// Locking it ends the brackets; the
/// sensors know a tick before the picture marks the assignment acknowledged.
fn bracketed(readout: &CockpitReadout) -> Option<u32> {
    let given = readout.link.assignment()?;
    let locked = given.acknowledged || readout.sensors.acquired == Some(given.target);
    (!locked && crate::scope::link_blink_on(readout.sensors.tick)).then_some(given.target)
}

/// Where the bracketed target is: the displayed target's row when it is the
/// one, else the plane's own observation of it, else the link's track.
fn assigned_position(readout: &CockpitReadout, target: u32) -> Option<Vector> {
    readout
        .targets
        .display
        .as_ref()
        .filter(|row| row.id == target)
        .map(|row| row.position)
        .or_else(|| readout.observation(target).map(|contact| contact.position))
        .or_else(|| {
            readout
                .link
                .tracks
                .iter()
                .find(|track| track.target == target)
                .map(|track| track.position)
        })
}

/// Four corner brackets round the target the lead assigned, bigger than the
/// target box and open at the sides' middles, so the two shapes never read
/// alike. Blinks once a second until the plane locks the target.
fn draw_assigned(pixels: &mut [u8], frame: &FlightFrame, color: [u8; 3], zoom: f64) {
    let readout = &frame.readout;
    let Some(target) = bracketed(readout) else {
        return;
    };
    let Some(position) = assigned_position(readout, target) else {
        return;
    };
    let s = frame.presented();
    let Some(TargetCue::Square(point)) = target_cue(
        missiles::sub(position, s.position),
        Basis::new(s.yaw, s.pitch, s.bank),
        zoom,
    ) else {
        return;
    };
    let mut paint = Paint {
        pixels,
        clip: (174, 96, 292, hud::AIM_BOTTOM - 96),
        color: [color[0], color[1], color[2], 255],
    };
    for (a, b) in bracket_lines(point) {
        paint.line(a, b);
    }
}

/// The eight strokes of the assignment brackets about `(x, y)`: a corner at
/// each of (+-11, +-11), with two arms of five pixels reaching inward along
/// the sides.
fn bracket_lines((x, y): (f64, f64)) -> [((f64, f64), (f64, f64)); 8] {
    const HALF: f64 = 11.;
    const ARM: f64 = 5.;
    let mut lines = [((0., 0.), (0., 0.)); 8];
    for (index, (sx, sy)) in [(-1., -1.), (1., -1.), (1., 1.), (-1., 1.)]
        .into_iter()
        .enumerate()
    {
        let corner = (x + sx * HALF, y + sy * HALF);
        lines[index * 2] = (corner, (corner.0 - sx * ARM, corner.1));
        lines[index * 2 + 1] = (corner, (corner.0, corner.1 - sy * ARM));
    }
    lines
}

fn draw_target_box(paint: &mut Paint<'_>, (x, y): (f64, f64), style: aim_box::Style) {
    for (a, b) in aim_box::box_strokes(style) {
        paint.line((x + a.0, y + a.1), (x + b.0, y + b.1));
    }
}

/// The gun's readout and LCOS pipper for the gun at station `index`, the
/// selected one or the one Tab fires.
fn draw_gun(
    pixels: &mut [u8],
    frame: &FlightFrame,
    font: &Font,
    color: [u8; 3],
    zoom: f64,
    index: usize,
) {
    let s = frame.presented();
    let ro = &frame.readout;
    let mut paint = Paint {
        pixels,
        clip: hud::HUD_CLIP,
        color: [color[0], color[1], color[2], 255],
    };
    let station = &frame.config.stations[index];
    paint.text(
        font,
        &format!("{} {}", ro.stores.rounds(index), station.weapon.hud_name),
        207,
        271,
    );
    paint.text(
        font,
        if ro.stores.armed { "LCOS" } else { "SAFE" },
        207,
        259,
    );
    let l = combat::launcher(s);
    if !ro.stores.armed
        || !l.alive
        || ro.stores.rounds(index) == 0
        || (index == ro.stores.selected()
            && ro.estimates.readiness == live::Readiness::StationFailed)
    {
        return;
    }
    let observation = ro
        .targets
        .designated
        .and_then(|id| ro.observation(id))
        .filter(|c| !c.destroyed);
    let radar = observation
        .filter(|c| {
            c.channel == tore_sim::sensors::Channel::Radar
                && ro.sensors.operating(tore_sim::sensors::Channel::Radar)
                && l.radar
        })
        .map(|c| gunsight::TargetObservation {
            position: c.position,
            velocity: c.velocity,
        });
    let solution = gunsight::solve(&station.weapon, &l, station.gun_mount(), radar)
        .ok()
        .flatten();
    let Some(solution) = solution else {
        paint.text(font, "NO SOL", 207, 283);
        return;
    };
    paint.text(
        font,
        if solution.radar { "RADAR" } else { "1000 FT" },
        207,
        283,
    );
    let range = observation.map(|c| missiles::length(missiles::sub(c.position, s.position)));
    if let Some(range) = range {
        paint.text(font, &format!("R {:.2}", range / missiles::NMI), 402, 259);
    }
    draw_gun_solution(pixels, s, color, zoom, solution, range);
}

/// Director presentation of the existing AI gun solution. No invented radar,
/// ammunition or readiness readouts: only the computed pipper and range arc.
pub(crate) fn replay_gunsight(
    pixels: &mut [u8],
    state: &flight::State,
    color: [u8; 3],
    zoom: f64,
    gun: &tore_formats::weapons::Weapon,
    mount: Vector,
    target: gunsight::TargetObservation,
) {
    if let Ok(Some(solution)) =
        gunsight::solve_observed(gun, &combat::launcher(state), mount, Some(target))
    {
        let range = missiles::length(missiles::sub(target.position, state.position));
        draw_gun_solution(pixels, state, color, zoom, solution, Some(range));
    }
}

fn draw_gun_solution(
    pixels: &mut [u8],
    s: &flight::State,
    color: [u8; 3],
    zoom: f64,
    solution: gunsight::Solution,
    range: Option<f64>,
) {
    let mut paint = Paint {
        pixels,
        clip: hud::HUD_CLIP,
        color: [color[0], color[1], color[2], 255],
    };
    let Some((x, y)) = projected(missiles::sub(solution.point, s.position), s, zoom) else {
        return;
    };
    if !(184. ..=456.).contains(&x) || !(106. ..=f64::from(hud::AIM_BOTTOM - 10)).contains(&y) {
        return;
    }
    paint.clip = (174, 96, 292, hud::AIM_BOTTOM - 96);
    let arc = range.map_or(0., |r| {
        gunsight::range_arc_fraction(r, solution.maximum_range_ft)
    });
    for segment in 0..64 {
        let a = f64::from(segment) * std::f64::consts::TAU / 64. - std::f64::consts::FRAC_PI_2;
        let b = f64::from(segment + 1) * std::f64::consts::TAU / 64. - std::f64::consts::FRAC_PI_2;
        paint.line(
            (x + 9. * a.cos(), y + 9. * a.sin()),
            (x + 9. * b.cos(), y + 9. * b.sin()),
        );
        if f64::from(segment) / 64. < arc {
            for radius in [10., 11.] {
                paint.line(
                    (x + radius * a.cos(), y + radius * a.sin()),
                    (x + radius * b.cos(), y + radius * b.sin()),
                );
            }
        }
    }
    paint.rect(x.round() as i32, y.round() as i32, 2, 2);
}

/// Diagnostic state and controls are composed at the window's upper right
/// when Pref > Weapon diagnostics? is on; the host gates drawing and clicks.
pub fn debug(
    pixels: &mut [u8],
    frame: &FlightFrame,
    state: &live::State,
    font: &Font,
    color: [u8; 3],
) {
    let s = frame.presented();
    let ro = &frame.readout;
    let mut paint = Paint {
        pixels,
        clip: (0, 0, 250, 96),
        color: [0, 0, 0, 190],
    };
    paint.rect(0, 0, 250, 96);
    paint.color = [color[0], color[1], color[2], 255];
    paint.text(
        font,
        if ro.airport.nav_mode {
            "NAV"
        } else if frame.config.stations.get(ro.stores.selected()).is_none() {
            "UNARMED"
        } else if live::is_gun(&frame.config.stations[ro.stores.selected()].weapon) {
            "GUN"
        } else if state.weapon_rules == missiles::Rules::Compatibility {
            "COMPATIBILITY"
        } else {
            ro.stores.launch_mode.label()
        },
        4,
        4,
    );
    paint.text(font, "[RELEASE LOCK]", 132, 4);
    paint.text(
        font,
        if ro.stores.armed {
            ro.seeker.status.label()
        } else {
            "SAFE"
        },
        4,
        18,
    );
    let details = if let Some(o) = ro.estimates.observation {
        let closing = missiles::closure(s.position, s.velocity, o.position, o.velocity) / 1.68781;
        format!("R {:.1}NM C {closing:+.0}KT", o.range / missiles::NMI)
    } else {
        "R -- C --".into()
    };
    paint.text(font, &details, 4, 32);
    let time = ro
        .estimates
        .solution_seconds
        .map_or_else(|| "EST --".into(), |seconds| format!("EST {seconds:.1}S"));
    paint.text(font, &time, 4, 44);
    if let Some(o) = ro.estimates.observation
        && missiles::length(o.velocity) > 1e-9
    {
        let aspect = dot(
            tore_sim::attitude::unit(o.velocity),
            tore_sim::attitude::unit(missiles::sub(s.position, o.position)),
        )
        .clamp(-1., 1.)
        .acos()
        .to_degrees();
        paint.text(font, &format!("ASP {aspect:.0}"), 132, 44);
    }
    for (row, shot) in state
        .projectiles
        .iter()
        .filter(|p| p.guidance.is_some() && p.threatening())
        .rev()
        .take(3)
        .enumerate()
    {
        let f = shot.guidance.as_ref().unwrap();
        let weapon = state.weapon(shot);
        let lifetime = shot.motion.and_then(|m| m.energy).map_or(
            shot.guidance_ticks.unwrap_or(f.profile.guidance_ticks),
            |e| u64::from(e.body.life),
        );
        let remaining = lifetime.saturating_sub(shot.age) as f64 / 120.;
        let motor = match shot.motion.map_or_else(
            || missiles::phase(&weapon.movement, shot.age),
            |m| m.engine_phase(&weapon.movement, shot.age),
        ) {
            tore_sim::combat::EnginePhase::BeforeIgnition => "WAIT",
            tore_sim::combat::EnginePhase::Powered => "BURN",
            tore_sim::combat::EnginePhase::Coast => "COAST",
        };
        paint.text(
            font,
            &format!(
                "#{} {} {} {motor} {remaining:.0}S",
                shot.id,
                weapon.hud_name,
                if f.seeker.status == Status::Search && f.profile.guidance != Guidance::Active {
                    "SEARCH"
                } else {
                    f.seeker.status.label()
                }
            ),
            4,
            58 + row as i32 * 11,
        );
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_empty_loadout_can_draw_before_the_readout_arrives() {
        let state = tore_world::test_support::combat_fixture(false);
        let mut config = state.own().configuration().clone();
        config.stations.clear();
        let flight =
            flight::State::new(&tore_world::test_support::profile(), [0., 5000., 0.]).unwrap();
        let picture = crate::snapshot::RenderSnapshot::default();
        let frame = FlightFrame {
            seat: tore_world::seats::SeatId(0),
            plane: tore_world::seats::PlaneId(0),
            flight: &flight,
            previous: &flight,
            presented: std::borrow::Cow::Borrowed(&flight),
            picture: &picture,
            smoke: [&Default::default(), &Default::default()],
            devices: &Default::default(),
            config: &config,
            readout: tore_world::frame::ReadoutSlot::ready(
                tore_world::readout::build(&state, 0, combat::launcher(&flight), None, None)
                    .unwrap(),
            ),
            tick_cues: &[],
        };
        let font = Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 6,
                    pixels: vec![(0, 0)],
                })
                .collect(),
        };
        let mut pixels = vec![0; 640 * 480 * 4];
        assert!(!active(&frame));
        draw(
            &mut pixels,
            &frame,
            &font,
            [0, 255, 0],
            1.,
            false,
            false,
            None,
            false,
        );
        debug(&mut pixels, &frame, &state, &font, [0, 255, 0]);
        assert!(pixels.iter().any(|v| *v != 0));
    }

    #[test]
    fn friendly_target_box_adds_only_a_centered_x() {
        let render = |friendly| {
            let mut pixels = vec![0; 640 * 480 * 4];
            draw_target_box(
                &mut Paint {
                    pixels: &mut pixels,
                    clip: hud::HUD_CLIP,
                    color: [0, 255, 0, 255],
                },
                (320., 240.),
                aim_box::Style::Tracked { friendly },
            );
            pixels
        };
        let ordinary = render(false);
        let friendly = render(true);
        for y in 233usize..=247 {
            for x in 313usize..=327 {
                let alpha = (y * 640 + x) * 4 + 3;
                let dx = x as i32 - 320;
                let dy = y as i32 - 240;
                let cross = dx.abs() <= 3 && dy.abs() <= 3 && dx.abs() == dy.abs();
                if cross {
                    assert_eq!(ordinary[alpha], 0);
                    assert_eq!(friendly[alpha], 255);
                } else {
                    assert_eq!(ordinary[alpha], friendly[alpha]);
                }
            }
        }
    }

    #[test]
    fn target_square_and_chevron_follow_full_three_dimensional_attitude() {
        let basis = Basis::new(0., 0., 0.);
        assert_eq!(
            target_cue([0., 0., 1000.], basis, 1.),
            Some(TargetCue::Square((320., 240.)))
        );
        for (point, expected) in [
            ([1000., 0., 1000.], (456., 240.)),
            ([-1000., 0., 1000.], (184., 240.)),
            ([0., 1000., 1000.], (320., 106.)),
            ([0., -1000., 1000.], (320., f64::from(hud::AIM_BOTTOM - 10))),
            ([0., 0., -1000.], (456., 240.)),
            ([1000., 0., -1000.], (456., 240.)),
        ] {
            let Some(TargetCue::Chevron { point, .. }) = target_cue(point, basis, 1.) else {
                panic!("missing off-HUD chevron");
            };
            assert_eq!(point, expected);
        }
        for (yaw, pitch, bank) in [
            (1., 0.8, 1.4),
            (2., -1.5, 3.0),
            (0., 0., std::f64::consts::FRAC_PI_2),
        ] {
            let body = Basis::new(yaw, pitch, bank);
            assert!(matches!(
                target_cue(body.forward.map(|v| v * 1000.), body, 1.),
                Some(TargetCue::Square(_))
            ));
            let direction = std::array::from_fn(|i| {
                body.forward[i] * 1000. + body.right[i] * 500. + body.up[i] * 100.
            });
            let Some(TargetCue::Chevron { point, direction }) = target_cue(direction, body, 1.)
            else {
                panic!("rotated cue");
            };
            assert!((point.0 - 456.).abs() < 1e-8 && point.1 < 240.);
            assert!(direction.0 > 0. && direction.1 < 0.);
        }
        assert_eq!(target_cue([0.; 3], basis, 1.), None);
        assert_eq!(target_cue([f64::NAN, 0., 1.], basis, 1.), None);
    }
    #[test]
    fn target_cue_crosses_hud_edge_without_reversing_or_clamping_a_box() {
        let basis = Basis::new(0., 0., 0.);
        for zoom in [0.5, 1., 2., 4.] {
            let edge = 136. / (240. * 3f64.sqrt() * zoom);
            assert!(matches!(
                target_cue([edge - 1e-6, 0., 1.], basis, zoom),
                Some(TargetCue::Square(_))
            ));
            assert!(matches!(
                target_cue([edge + 1e-6, 0., 1.], basis, zoom),
                Some(TargetCue::Chevron { .. })
            ));
        }
    }

    #[test]
    fn radar_ready_diamond_blinks_twice_per_simulation_second() {
        for tick in 0..120 {
            assert_eq!(diamond_visible(true, true, tick), tick % 60 < 30);
            assert!(diamond_visible(true, false, tick));
            assert!(!diamond_visible(false, true, tick));
        }
    }
    #[test]
    fn search_boundary_projects_with_zoom_and_hit_region_tracks_aspect() {
        let b = Basis::new(0., 0., 0.);
        let cone = boundary(b, 3f64.to_radians(), 3f64.to_radians());
        for d in cone {
            let h = d[0].atan2(d[2]).abs().to_degrees();
            let v = d[1].atan2(d[0].hypot(d[2])).abs().to_degrees();
            assert!(h <= 3. + 1e-9 && v <= 3. + 1e-9);
        }
        for size in [[640., 480.], [1920., 1080.], [1080., 1920.]] {
            let scale = (size[0] / 640f64).min(size[1] / 480.);
            assert!(mode_hit((size[0] - 248. * scale, 18. * scale), size));
            assert!(release_hit((size[0] - 108. * scale, 18. * scale), size));
            assert!(!mode_hit((size[0] - 108. * scale, 18. * scale), size));
        }
        for size in [[640, 480], [1920, 1080], [1080, 1920]] {
            let mut canvas = crate::flight_canvas::FlightCanvas::default();
            canvas.size = size;
            for camera_zoom in [0.5, 1., 2., 4.] {
                let zoom = f64::from(canvas.hud_zoom(camera_zoom));
                let offset = hud::project(0., 0., 3f64.to_radians(), 0., zoom).unwrap().0 - 320.;
                let scale = (f64::from(size[0]) / 640.).min(f64::from(size[1]) / 480.)
                    * crate::flight_canvas::HUD_SCALE;
                let expected = f64::from(size[1]) / 2.
                    * 3f64.sqrt()
                    * 3f64.to_radians().tan()
                    * f64::from(camera_zoom);
                assert!((offset * scale - expected).abs() < 0.001);
            }
        }
        let at = |z| hud::project(0., 0., 3f64.to_radians(), 0., z).unwrap().0 - 320.;
        assert!((at(2.) - 2. * at(1.)).abs() < 1e-9);
    }

    type Stroke = ((f64, f64), (f64, f64));

    /// A fixture readout whose link has the lead's assignment of target 7.
    fn assigned_readout(radar: bool, acknowledged: bool) -> CockpitReadout {
        let (combat, flight) = tore_world::combat::fixtures::loaded([1, 1]);
        let mut readout = combat
            .cockpit_readout(
                combat.own_id(),
                crate::combat::launcher(&flight),
                None,
                None,
            )
            .expect("the fixture's plane has an ownship");
        readout.link = tore_world::readout::LinkReadout {
            radar,
            assigned: Some(tore_world::readout::LinkAssigned {
                target: 7,
                by: 0,
                acknowledged,
            }),
            tracks: vec![tore_world::readout::LinkTrack {
                target: 7,
                position: [10., 20., 30.],
                velocity: [0.; 3],
                source: tore_world::datalink::TrackSource::Flight,
            }],
            ..Default::default()
        };
        readout
    }

    #[test]
    fn the_brackets_blink_once_a_second_until_the_plane_locks_the_target() {
        let mut readout = assigned_readout(true, false);
        let lit = |readout: &mut CockpitReadout, tick: u64| {
            readout.sensors.tick = tick;
            bracketed(readout)
        };
        // Half a second on, half a second off, at 120 ticks a second.
        assert_eq!(lit(&mut readout, 0), Some(7));
        assert_eq!(lit(&mut readout, 59), Some(7));
        assert_eq!(lit(&mut readout, 60), None);
        assert_eq!(lit(&mut readout, 119), None);
        assert_eq!(lit(&mut readout, 120), Some(7));
        // The plane's own lock ends them at once, before the picture catches up.
        readout.sensors.acquired = Some(7);
        assert_eq!(lit(&mut readout, 0), None);
        // A lock on something else does not.
        readout.sensors.acquired = Some(8);
        assert_eq!(lit(&mut readout, 0), Some(7));
        // The picture's acknowledgement ends them too, and so does having none.
        readout.sensors.acquired = None;
        readout.link.assigned.as_mut().unwrap().acknowledged = true;
        assert_eq!(lit(&mut readout, 0), None);
        readout.link.assigned = None;
        assert_eq!(lit(&mut readout, 0), None);
    }

    #[test]
    fn an_aircraft_with_no_radar_keeps_its_brackets() {
        // John, 2026-10-05: only the radar scope's marks go without a radar.
        let mut readout = assigned_readout(false, false);
        readout.sensors.tick = 0;
        assert_eq!(bracketed(&readout), Some(7));
    }

    #[test]
    fn the_brackets_find_the_target_where_the_plane_or_the_link_has_it() {
        let mut readout = assigned_readout(true, false);
        // Only the link's track holds it.
        assert_eq!(assigned_position(&readout, 7), Some([10., 20., 30.]));
        // The plane's own observation is better than the link's.
        let mut contact = tore_sim::sensors::Contact {
            id: 7,
            channel: tore_sim::sensors::Channel::Radar,
            bearing_rad: 0.,
            elevation_rad: 0.,
            distance_ft: 1.,
            position: [1., 2., 3.],
            velocity: [0.; 3],
            track_eligible: true,
            destroyed: false,
        };
        readout.sensors.contacts = vec![contact];
        assert_eq!(assigned_position(&readout, 7), Some([1., 2., 3.]));
        contact.id = 8;
        readout.sensors.contacts = vec![contact];
        readout.link.tracks.clear();
        assert_eq!(assigned_position(&readout, 7), None);
    }

    #[test]
    fn the_brackets_are_not_the_target_box() {
        let render = |lines: &[Stroke]| {
            let mut pixels = vec![0; 640 * 480 * 4];
            let mut paint = Paint {
                pixels: &mut pixels,
                clip: hud::HUD_CLIP,
                color: [0, 255, 0, 255],
            };
            for (a, b) in lines {
                paint.line(*a, *b);
            }
            pixels
                .chunks_exact(4)
                .enumerate()
                .filter(|(_, px)| px[3] != 0)
                .map(|(i, _)| (i % 640, i / 640))
                .collect::<std::collections::BTreeSet<_>>()
        };
        let brackets = render(&bracket_lines((320., 240.)));
        let mut pixels = vec![0; 640 * 480 * 4];
        draw_target_box(
            &mut Paint {
                pixels: &mut pixels,
                clip: hud::HUD_CLIP,
                color: [0, 255, 0, 255],
            },
            (320., 240.),
            aim_box::Style::Tracked { friendly: false },
        );
        let boxed: std::collections::BTreeSet<(usize, usize)> = pixels
            .chunks_exact(4)
            .enumerate()
            .filter(|(_, px)| px[3] != 0)
            .map(|(i, _)| (i % 640, i / 640))
            .collect();
        assert!(brackets.is_disjoint(&boxed), "the shapes touch");
        // Four corners, each of two arms; the middle of each side is open.
        for (x, y) in [(320, 229), (320, 251), (309, 240), (331, 240)] {
            assert!(!brackets.contains(&(x, y)), "({x}, {y}) is open");
        }
        for (x, y) in [(309, 229), (331, 229), (331, 251), (309, 251)] {
            assert!(brackets.contains(&(x, y)), "({x}, {y}) is a corner");
        }
        let extent = |axis: fn(&(usize, usize)) -> usize| {
            let values: Vec<usize> = brackets.iter().map(axis).collect();
            (*values.iter().min().unwrap(), *values.iter().max().unwrap())
        };
        assert_eq!(extent(|p| p.0), (309, 331));
        assert_eq!(extent(|p| p.1), (229, 251));
    }
}
