//! Flight display projection. Source fonts; authored layout and symbology.
use crate::{flight::State, menu::Canvas};
use tore_formats::font::Font;
use tore_formats::text::GlyphCodes;

pub struct Paint<'a> {
    pub pixels: &'a mut [u8],
    pub clip: (i32, i32, i32, i32),
    pub color: [u8; 4],
}
impl Paint<'_> {
    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let (cx, cy, cw, ch) = self.clip;
        let (left, top) = (x.max(cx), y.max(cy));
        Canvas(self.pixels).rect(
            (
                left,
                top,
                (x + w).min(cx + cw) - left,
                (y + h).min(cy + ch) - top,
            ),
            self.color,
        );
    }
    pub fn line(&mut self, a: (f64, f64), b: (f64, f64)) {
        // Bound work even if a projected endpoint is very far offscreen.
        let n = (b.0 - a.0)
            .abs()
            .max((b.1 - a.1).abs())
            .ceil()
            .clamp(1., 4096.) as i32;
        for i in 0..=n {
            self.rect(
                (a.0 + (b.0 - a.0) * i as f64 / n as f64).round() as i32,
                (a.1 + (b.1 - a.1) * i as f64 / n as f64).round() as i32,
                1,
                1,
            );
        }
    }
    fn readout_box(&mut self, font: &Font, text: &str, x: i32, y: i32) {
        let width = text
            .glyph_codes()
            .map(|c| font.glyphs[c as usize].advance)
            .sum::<usize>() as i32;
        let (left, top, right, bottom) = (
            x - 4,
            y - 3,
            x + width.max(24) + 3,
            y + font.height as i32 + 2,
        );
        // Outline only: retain the world behind the original HUD glyphs.
        self.line((left as f64, top as f64), (right as f64, top as f64));
        self.line((right as f64, top as f64), (right as f64, bottom as f64));
        self.line((right as f64, bottom as f64), (left as f64, bottom as f64));
        self.line((left as f64, bottom as f64), (left as f64, top as f64));
        self.text(font, text, x, y);
    }
    pub fn text(&mut self, font: &Font, text: &str, mut x: i32, y: i32) {
        for ch in text.glyph_codes() {
            let g = &font.glyphs[ch as usize];
            for &(xx, yy) in &g.pixels {
                self.rect(x + xx as i32, y + yy as i32, 1, 1);
            }
            x += g.advance as i32;
        }
    }
}
// Authored F-16-style bank scale requested by the user. The graduated arc
// rotates past a fixed index, keeping full rolls readable through +/-180.
// Right-hand status column. Leave one row below BRAKE for steering authority
// and keep HOOK and the MSL caption clear of every status label.
const STATUS_LABEL_X: i32 = 388;
const TIME_LABEL_Y: i32 = 118;
const BAY_LABEL_Y: i32 = 129;
const NSW_LABEL_Y: i32 = 173;
const MSL_LABEL: (i32, i32) = (402, 201);
pub const HUD_CLIP: (i32, i32, i32, i32) = (174, 96, 292, 354);
pub const AIM_BOTTOM: i32 = 390;
const BANK_CENTER_Y: f64 = 135.;
const BANK_RADIUS: f64 = 223.;
fn bank_point(angle: f64, radius: f64) -> (f64, f64) {
    let a = angle.to_radians();
    (320. + radius * a.sin(), BANK_CENTER_Y + radius * a.cos())
}
fn bank_scale(p: &mut Paint<'_>, font: &Font, bank: f64) {
    for mark in (-180i32..180).step_by(10) {
        let angle = bank_tick_angle(mark, bank);
        if angle.abs() > 30. {
            continue;
        }
        let major = mark % 30 == 0;
        p.line(
            bank_point(angle, if major { 216. } else { 219. }),
            bank_point(angle, BANK_RADIUS),
        );
        if major {
            let text = mark.abs().to_string();
            let width = text
                .glyph_codes()
                .map(|c| font.glyphs[c as usize].advance)
                .sum::<usize>() as i32;
            let (x, y) = bank_point(angle, 239.);
            p.text(
                font,
                &text,
                x.round() as i32 - width / 2,
                y.round() as i32 - font.height as i32 / 2,
            );
        }
    }
    let tip_y = BANK_CENTER_Y + BANK_RADIUS + 1.;
    let base_y = tip_y + 7.;
    p.line((320., tip_y), (316., base_y));
    p.line((316., base_y), (324., base_y));
    p.line((324., base_y), (320., tip_y));
}
/// Where a bank mark sits on the arc. The scale turns with the horizon: in a
/// right bank the horizon turns counter-clockwise, carrying the zero mark to
/// the right of the fixed index.
fn bank_tick_angle(mark: i32, bank: f64) -> f64 {
    (bank.to_degrees() - f64::from(mark) + 180.).rem_euclid(360.) - 180.
}
/// The bank scale's gyro: the shown bank follows the aircraft through a
/// damped spring, so it glides after a quick roll and settles with a slight
/// overshoot instead of copying every frame. Opinionated, requested by John on
/// 2026-09-28; the constants are fitted.
#[derive(Clone, Copy, Debug, Default)]
pub struct BankGyro {
    shown: Option<f64>,
    rate: f64,
}
/// Natural frequency in radians per second, and damping ratio: about 0.14
/// seconds behind a steady roll, settling within 0.6 seconds, 5% overshoot.
const GYRO_FREQUENCY: f64 = 10.;
const GYRO_DAMPING: f64 = 0.7;
const GYRO_STEP: f64 = 1. / 480.;
impl BankGyro {
    /// Advance by `elapsed` seconds toward the aircraft's `bank`, and return
    /// the bank to show. The first call shows the bank as it is.
    pub fn follow(&mut self, bank: f64, elapsed: f64) -> f64 {
        use std::f64::consts::{PI, TAU};
        let Some(mut shown) = self.shown else {
            self.shown = Some(bank);
            return bank;
        };
        let steps = (elapsed.clamp(0., 0.25) / GYRO_STEP).ceil() as usize;
        let step = if steps == 0 {
            0.
        } else {
            elapsed.clamp(0., 0.25) / steps as f64
        };
        for _ in 0..steps {
            // The short way round, so full rolls pass through 180 smoothly.
            let error = (bank - shown + PI).rem_euclid(TAU) - PI;
            self.rate += (GYRO_FREQUENCY.powi(2) * error
                - 2. * GYRO_DAMPING * GYRO_FREQUENCY * self.rate)
                * step;
            shown = (shown + self.rate * step + PI).rem_euclid(TAU) - PI;
        }
        self.shown = Some(shown);
        shown
    }
}
pub fn heading(yaw: f64) -> f64 {
    yaw.to_degrees().rem_euclid(360.)
}
/// A world direction projected through the same 60-degree camera as the GPU.
/// Camera roll is -bank. Screen Y points down.
pub fn project(
    pitch: f64,
    bank: f64,
    bearing: f64,
    elevation: f64,
    zoom: f64,
) -> Option<(f64, f64)> {
    let (sb, cb) = bearing.sin_cos();
    let (se, ce) = elevation.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sr, cr) = bank.sin_cos();
    let x = sb * ce;
    let y = se * cp - ce * cb * sp;
    let z = ce * cb * cp + se * sp;
    if z < 0.05 {
        return None;
    }
    let scale = 240. * 3f64.sqrt() * zoom / z;
    Some((
        320. + (x * cr - y * sr) * scale,
        240. - (x * sr + y * cr) * scale,
    ))
}
/// Pixels per radian of pitch at the forward point: the derivative of
/// `project` at the centre, so the ladder's scale is the HUD's true scale there.
fn ladder_scale(zoom: f64) -> f64 {
    240. * 3f64.sqrt() * zoom
}
/// A literal attitude ruler (docs/spec/hud-layout.md, "Flight display"). Every
/// mark, the zero bar included, sits (rung elevation minus aircraft pitch) times
/// one fixed pixels-per-degree scale from the forward point (320,240) along the
/// bank normal, so spacing is equal everywhere and a rung moves exactly with
/// pitch. The bearing offset is the local zero-pitch perspective width, which
/// keeps rungs a finite width at the poles; the whole scale rotates with bank.
fn ladder_project(
    pitch: f64,
    bank: f64,
    bearing: f64,
    elevation: f64,
    zoom: f64,
) -> Option<(f64, f64)> {
    let relative = elevation - pitch;
    // Far beyond the visible window, and the bearing would fold behind us.
    if relative.abs() >= std::f64::consts::FRAC_PI_2 || bearing.abs() >= std::f64::consts::FRAC_PI_2
    {
        return None;
    }
    let scale = ladder_scale(zoom);
    let (across, up) = (bearing.tan() * scale, relative * scale);
    let (sr, cr) = bank.sin_cos();
    Some((320. + across * cr - up * sr, 240. - across * sr - up * cr))
}
/// The HUD's weapon status word (docs/spec/air-to-ground.md, "Status word and
/// mode choice"; retail printer 0x409030): NAV with NAV selected, LCOS or
/// SAFE for a gun, CCIP or LVL for an unguided bomb as its sight is set, ARM
/// for a rocket pod and every guided store (a guided bomb in LVL says LVL from
/// its own HUD code, `weapon_hud/guided.rs`).
pub fn status_word(
    nav: bool,
    gun: bool,
    armed: bool,
    bomb_sight: Option<&tore_world::readout::BombSight>,
) -> &'static str {
    if nav {
        "NAV"
    } else if gun {
        if armed { "LCOS" } else { "SAFE" }
    } else if let Some(sight) = bomb_sight.filter(|sight| !sight.rocket) {
        sight.mode.label()
    } else {
        "ARM"
    }
}
/// `2X`, `4X`, `8X` or `1/2X`: the compression level the HUD shows.
pub fn time_label(scale: f64) -> String {
    if scale < 1. {
        format!("1/{:.0}X", 1. / scale)
    } else {
        format!("{scale:.0}X")
    }
}
#[allow(clippy::too_many_arguments)]
pub fn draw(
    pixels: &mut [u8],
    s: &State,
    font: &Font,
    ground: f64,
    air: Option<&tore_sim::telemetry::AirData>,
    ladder: bool,
    weapons: bool,
    color: [u8; 3],
    zoom: f32,
    ils: Option<(&tore_sim::airport::Guidance, &str, &str)>,
    wind: Option<&tore_sim::runway_wind::Assessment>,
    // The gyro-smoothed bank for the bank scale, and the time compression rate.
    (gyro_bank, time_scale): (f64, f64),
) {
    let _frame_log = crate::frame_log::span(crate::frame_log::Phase::Hud);
    let mut p = Paint {
        pixels,
        clip: HUD_CLIP,
        color: [color[0], color[1], color[2], 255],
    };
    if s.systems.has(31) {
        p.text(font, "FLIGHT DATA FAILED", 264, 225);
        return;
    }
    let hdg = heading(s.yaw);
    // Heading strip and pointer, wrapped across north.
    for i in -4..=4 {
        let tick = (hdg / 10.).floor() * 10. + i as f64 * 10.;
        let x = 320. + (tick - hdg) * 2.;
        if (260. ..380.).contains(&x) {
            p.line((x, 146.), (x, 151.));
            p.text(
                font,
                &format!("{:02.0}", tick.rem_euclid(360.) / 10.),
                x as i32 - 5,
                133,
            );
        }
    }
    p.line((315., 153.), (320., 158.));
    p.line((320., 158.), (325., 153.));
    p.text(font, &format!("{:03}", hdg.round() as u32 % 360), 311, 162);
    // Ladder is a linear degree scale, clipped away from the fixed tapes.
    if ladder {
        p.clip = (250, 182, 140, 166);
        for degrees in (-90..=90).step_by(5) {
            let el = (degrees as f64).to_radians();
            for side in [-1., 1.] {
                let spans: &[(f64, f64)] = if degrees < 0 {
                    &[(0.7, 1.2), (1.6, 2.1), (2.5, 3.)]
                } else {
                    &[(0.7, 3.)]
                };
                for &(a, b) in spans {
                    if let (Some(a), Some(b)) = (
                        ladder_project(s.pitch, s.bank, (side * a).to_radians(), el, zoom as f64),
                        ladder_project(s.pitch, s.bank, (side * b).to_radians(), el, zoom as f64),
                    ) {
                        p.line(a, b);
                    }
                }
            }
            if degrees != 0
                && let Some((x, y)) =
                    ladder_project(s.pitch, s.bank, 3.5f64.to_radians(), el, zoom as f64)
            {
                p.text(font, &degrees.to_string(), x as i32, y as i32 - 4);
            }
        }
    }
    p.clip = HUD_CLIP;
    // Kinematic flight path marker; no fixed aircraft-datum bars.
    if s.speed > 10. {
        let gamma = s.velocity[1].atan2(s.velocity[0].hypot(s.velocity[2]));
        let bearing = s.velocity[0].atan2(s.velocity[2]) - s.yaw;
        if let Some((x, y)) = project(s.pitch, s.bank, bearing, gamma, zoom as f64)
            && (235. ..405.).contains(&x)
            && (155. ..380.).contains(&y)
        {
            for i in 0..36 {
                let t = i as f64 * std::f64::consts::TAU / 36.;
                p.rect((x + 4. * t.cos()) as i32, (y + 4. * t.sin()) as i32, 1, 1);
            }
            p.line((x - 11., y), (x - 4., y));
            p.line((x + 4., y), (x + 11., y));
            p.line((x, y - 8.), (x, y - 4.));
        }
    }
    let speed = air.map_or(s.speed / 1.68781, |d| d.true_airspeed_knots);
    p.text(font, "TAS", 211, 190);
    p.text(font, "MSL", MSL_LABEL.0, MSL_LABEL.1);
    p.readout_box(font, &format!("{speed:.0}"), 211, 235);
    p.readout_box(font, &format!("{:.0}", s.position[1]), 405, 235);
    p.text(font, &format!("{:.1}G", s.g), 235, 164);
    // A rotorcraft's throttle keys drive its collective, so the readout shows
    // that (VTOL overhaul design section 6).
    match crate::powered_hud::collective_label(s) {
        Some(collective) => {
            let (x, y) = crate::powered_hud::THROTTLE_READOUT;
            p.text(font, &collective, x, y);
        }
        None => p.text(font, &format!("{:.0}%", s.throttle * 100.), 235, 178),
    }
    if s.afterburner_active() {
        p.text(font, "AFT", 235, 150);
    }
    for (label, y) in status_labels(s) {
        p.text(font, label, STATUS_LABEL_X, y);
    }
    if let Some(label) = nosewheel_label(s.nosewheel_authority()) {
        p.text(font, &label, STATUS_LABEL_X, NSW_LABEL_Y);
    }
    // Manual p. 80: the time compression rate shows in the upper right corner.
    if (time_scale - 1.).abs() > 1e-9 {
        p.text(font, &time_label(time_scale), STATUS_LABEL_X, TIME_LABEL_Y);
    }
    if !weapons && ils.is_some_and(|(guidance, _, _)| guidance.active) {
        p.text(
            font,
            &format!(
                "AGL {:.0}",
                air.map_or(s.position[1] - ground, |d| d.altitude_agl_ft)
                    .max(0.)
            ),
            402,
            259,
        );
        p.text(
            font,
            &format!(
                "V/S {:+.0}",
                air.map_or(s.vertical_speed * 60., |d| d.vertical_speed_fpm)
            ),
            207,
            271,
        );
    }
    if s.autopilot.mode() != tore_sim::autopilot::Mode::Off {
        p.text(font, "AUTO", 211, 133);
        p.text(font, &s.autopilot.label(), 211, 145);
    }
    // The powered-lift cluster: rotor and torque, nozzle or nacelle angle,
    // the hover display, radar height and the stability level.
    if s.model().powered_lift().is_some() {
        let agl = air.map_or(s.position[1] - ground, |d| d.altitude_agl_ft);
        let cluster = crate::powered_hud::marks(s, agl, weapons);
        crate::powered_hud::draw(&mut p, font, &cluster);
    }
    if let Some(wind) = wind {
        p.text(
            font,
            &wind_label(wind),
            244,
            if ils.is_some() { 96 } else { 106 },
        );
    }
    if let Some((guidance, airport, runway)) = ils {
        let airport: String = airport
            .chars()
            .filter(|c| tore_formats::text::is_drawn(*c))
            .take(24)
            .collect();
        let runway: String = runway
            .chars()
            .filter(|c| tore_formats::text::is_drawn(*c))
            .take(16)
            .collect();
        p.text(font, &format!("ILS {airport}"), 252, 106);
        p.text(
            font,
            &format!("RWY {runway} {:.1}NM", guidance.range_ft / 6_076.12),
            252,
            118,
        );
        if guidance.active {
            let vertical_x = 320. - guidance.localizer_normalized * 42.;
            let horizontal_y = 240. + guidance.glide_normalized * 30.;
            p.line((vertical_x, 207.), (vertical_x, 273.));
            p.line((278., horizontal_y), (362., horizontal_y));
            p.rect(318, 238, 5, 5);
        } else {
            p.text(font, "ILS ARM", 300, 258);
        }
    }
    if s.stall_alert(ground).is_some() {
        p.text(font, "STALL", 301, 274);
    }
    if s.crashed {
        p.text(font, "CRASHED - ESC", 277, 306);
    } else if !s.engine {
        p.text(font, "ENGINE OFF", 283, 306);
    } else if !weapons {
        p.clip = HUD_CLIP;
        bank_scale(&mut p, font, gyro_bank);
    }
}

/// The device labels of the status column shown for `s`, with their rows.
/// An aircraft whose gear is fixed down (the AH-64 and CH-47) shows no
/// `GEAR`: the cue says the gear is out of the airflow's way or not, and a
/// fixed gear never is (VTOL overhaul slice P7b).
fn status_labels(s: &State) -> Vec<(&'static str, i32)> {
    let gear = if s.model().fixed_gear() { 0. } else { s.gear };
    [
        ("GEAR", 140, gear),
        ("FLAP", 151, s.flaps),
        ("BRAKE", 162, s.brake),
        ("HOOK", 184, s.hook),
        // Manual p. 79: BAY shows while the weapons bay is open.
        (
            "BAY",
            BAY_LABEL_Y,
            if s.bay_available() { s.bay } else { 0. },
        ),
    ]
    .into_iter()
    .filter(|(_, _, value)| *value > 0.01)
    .map(|(label, y, _)| (label, y))
    .collect()
}

fn nosewheel_label(authority: f64) -> Option<String> {
    let percent = (authority.clamp(0., 1.) * 100.).round() as u32;
    (percent > 0).then(|| format!("NSW {percent}%"))
}
fn wind_label(wind: &tore_sim::runway_wind::Assessment) -> String {
    let tail = if wind.tailwind_knots >= 0.5 {
        format!(
            " TW {:.0}/10{}",
            wind.tailwind_knots,
            if wind.tailwind_knots >= 10. {
                " LIMIT"
            } else {
                ""
            }
        )
    } else {
        String::new()
    };
    let cross = if wind.crosswind_knots.abs() < 0.5 {
        0.
    } else {
        wind.crosswind_knots
    };
    format!(
        "XW {cross:+.0}/{:.0} {}{tail} KT",
        wind.limit_knots,
        wind.severity.label()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_status_word_follows_the_selected_store_and_the_bomb_sight() {
        use tore_sim::combat::delivery::DeliveryMode;
        let sight = |mode, rocket| tore_world::readout::BombSight {
            mode,
            rocket,
            ccip: None,
            level: None,
            warnings: Vec::new(),
            falling: None,
        };
        assert_eq!(status_word(true, true, true, None), "NAV");
        assert_eq!(status_word(false, true, true, None), "LCOS");
        assert_eq!(status_word(false, true, false, None), "SAFE");
        let ccip = sight(DeliveryMode::Ccip, false);
        let level = sight(DeliveryMode::Level, false);
        let rocket = sight(DeliveryMode::Ccip, true);
        assert_eq!(status_word(false, false, true, Some(&ccip)), "CCIP");
        assert_eq!(status_word(false, false, true, Some(&level)), "LVL");
        assert_eq!(status_word(false, false, true, Some(&rocket)), "ARM");
        assert_eq!(status_word(false, false, true, None), "ARM");
    }
    #[test]
    fn a_fixed_gear_shows_no_gear_label() {
        use tore_formats::aircraft::AircraftId;
        let state = |id: Option<AircraftId>| {
            let profile = id.map_or_else(tore_world::test_support::profile, |id| {
                tore_world::test_support::powered_profile(id)
            });
            let mut s = State::new(&profile, [0., 3_000., 0.]).unwrap();
            s.gear = 1.;
            s.gear_down = true;
            s
        };
        for id in [AircraftId::Ah64, AircraftId::Ch47] {
            let s = state(Some(id));
            assert!(s.model().fixed_gear(), "{id:?}");
            assert!(
                !status_labels(&s).iter().any(|(l, _)| *l == "GEAR"),
                "{id:?}"
            );
        }
        // Retracting gear keeps its cue: the Mi-24, the V-22, the AV-8 and a
        // fighter.
        for id in [
            Some(AircraftId::Mi24),
            Some(AircraftId::V22),
            Some(AircraftId::Av8),
            None,
        ] {
            assert_eq!(status_labels(&state(id))[0], ("GEAR", 140), "{id:?}");
        }
        let mut up = state(None);
        up.gear = 0.;
        assert!(status_labels(&up).is_empty());
    }

    #[test]
    fn inactive_nosewheel_authority_has_no_hud_label() {
        assert_eq!(nosewheel_label(0.), None);
        assert_eq!(nosewheel_label(0.004), None);
        assert_eq!(nosewheel_label(0.006).as_deref(), Some("NSW 1%"));
        assert_eq!(nosewheel_label(0.5).as_deref(), Some("NSW 50%"));
        assert_eq!(nosewheel_label(1.).as_deref(), Some("NSW 100%"));
    }
    #[test]
    fn no_status_label_overlaps_another_or_the_msl_caption() {
        // The labels sit at fixed rows, so every pair covers every combination
        // of shown labels. Each glyph is a solid 10 px tall cell, the height of
        // the imported HUD11 font, so this is stricter than the real ink.
        let (height, advance) = (10, 6);
        let width = |text: &str| text.chars().count() as i32 * advance;
        let mut boxes = vec![("MSL", MSL_LABEL.0, MSL_LABEL.1, width("MSL"))];
        for (name, y) in [
            ("1/2X", TIME_LABEL_Y),
            ("BAY", BAY_LABEL_Y),
            ("GEAR", 140),
            ("FLAP", 151),
            ("BRAKE", 162),
            ("NSW 100%", NSW_LABEL_Y),
            ("HOOK", 184),
        ] {
            boxes.push((name, STATUS_LABEL_X, y, width(name)));
        }
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let apart = a.1 + a.3 <= b.1
                    || b.1 + b.3 <= a.1
                    || a.2 + height <= b.2
                    || b.2 + height <= a.2;
                assert!(apart, "{} overlaps {}", a.0, b.0);
            }
        }
    }
    #[test]
    fn the_time_compression_label_reads_like_the_menu() {
        assert_eq!(time_label(0.5), "1/2X");
        assert_eq!(time_label(2.), "2X");
        assert_eq!(time_label(8.), "8X");
    }
    #[test]
    fn accented_letters_draw_on_their_code_page_437_cell() {
        // A font whose only marked cells are 'e' (0x65) and CP437 0x89, the
        // e with a diaeresis, each a different single pixel.
        let mut glyphs: Vec<tore_formats::font::Glyph> = (0..256)
            .map(|_| tore_formats::font::Glyph {
                advance: 4,
                pixels: vec![],
            })
            .collect();
        glyphs[0x65].pixels = vec![(0, 0)];
        glyphs[0x89].pixels = vec![(2, 0)];
        let font = tore_formats::font::Font { height: 1, glyphs };
        let mut pixels = vec![0u8; crate::menu::WIDTH * crate::menu::HEIGHT * 4];
        Paint {
            pixels: &mut pixels,
            clip: (0, 0, 640, 480),
            color: [255, 255, 255, 255],
        }
        .text(&font, "e\u{eb}", 0, 0);
        let lit: Vec<usize> = (0..12)
            .filter(|x| pixels[x * 4 + 3] != 0 && pixels[x * 4] == 255)
            .collect();
        // 'e' at x 0, then the diaeresis cell's pixel two columns into the second glyph.
        assert_eq!(lit, [0, 4 + 2]);
    }
    #[test]
    fn wind_readout_distinguishes_crosswind_tailwind_and_ignored_headwind() {
        use tore_sim::runway_wind::{FEET_PER_SECOND_PER_KNOT as K, assessment};
        assert_eq!(
            wind_label(&assessment(40_000., [31. * K, 0., 0.], 0.).unwrap()),
            "XW +31/30 LIMIT KT"
        );
        assert_eq!(
            wind_label(&assessment(40_000., [0., 0., 10. * K], 0.).unwrap()),
            "XW +0/30 CALM TW 10/10 LIMIT KT"
        );
        assert_eq!(
            wind_label(&assessment(40_000., [0., 0., -40. * K], 0.).unwrap()),
            "XW +0/30 CALM KT"
        );
    }

    #[test]
    fn bank_scale_tracks_aircraft_attitude_through_full_rolls() {
        assert_eq!(bank_tick_angle(30, 30f64.to_radians()), 0.);
        assert_eq!(bank_tick_angle(-30, (-30f64).to_radians()), 0.);
        assert_eq!(bank_tick_angle(0, 360f64.to_radians()), 0.);
        assert!((bank_tick_angle(-180, 179f64.to_radians()) + 1.).abs() < 1e-9);
        assert!((bank_tick_angle(-180, 181f64.to_radians()) - 1.).abs() < 1e-9);
        // A right bank carries the zero mark right of the index, as the
        // horizon turns counter-clockwise; a left bank carries it left.
        assert_eq!(bank_tick_angle(0, 30f64.to_radians()), 30.);
        assert!(bank_point(bank_tick_angle(0, 30f64.to_radians()), BANK_RADIUS).0 > 320.);
        assert!(bank_point(bank_tick_angle(0, (-30f64).to_radians()), BANK_RADIUS).0 < 320.);
        let left = bank_point(-30., BANK_RADIUS);
        let center = bank_point(0., BANK_RADIUS);
        let right = bank_point(30., BANK_RADIUS);
        assert!((right.0 - left.0 - 223.).abs() < 1e-9);
        assert!((center.1 - left.1 - 223. * (1. - 30f64.to_radians().cos())).abs() < 1e-9);
        assert_eq!(center.1, 358.);
    }
    #[test]
    fn bank_gyro_glides_and_settles() {
        let degrees = |r: f64| r.to_degrees();
        let mut gyro = BankGyro::default();
        assert_eq!(gyro.follow(0.3, 0.1), 0.3, "starts at the aircraft's bank");
        assert_eq!(gyro.follow(0.3, 0.), 0.3);
        // A sudden 30 degree roll: behind at first, a small overshoot, settled
        // within 0.6 seconds.
        let target = 0.3 + 30f64.to_radians();
        let mut peak = f64::MIN;
        let mut shown = Vec::new();
        for _ in 0..120 {
            let bank = gyro.follow(target, 1. / 120.);
            peak = peak.max(bank);
            shown.push(bank);
        }
        assert!(degrees(target - shown[5]) > 15., "{}", degrees(shown[5]));
        let overshoot = degrees(peak - target);
        assert!((0.5..2.).contains(&overshoot), "{overshoot}");
        assert!(
            shown[72..]
                .iter()
                .all(|b| degrees((b - target).abs()) < 0.6)
        );
        // Paused, it holds.
        assert_eq!(gyro.follow(-1., 0.), shown[119]);
    }
    #[test]
    fn bank_gyro_lags_a_steady_roll_and_passes_through_180() {
        let rate = 90f64.to_radians();
        let wrap = |a: f64| {
            (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
        };
        for hz in [30., 60., 144.] {
            let mut gyro = BankGyro::default();
            let mut bank = 0.;
            gyro.follow(bank, 0.);
            let mut previous = 0.;
            for _ in 0..(3. * hz) as usize {
                bank = wrap(bank + rate / hz);
                let shown = gyro.follow(bank, 1. / hz);
                // Never a jump, even crossing 180 degrees.
                assert!(wrap(shown - previous).abs() < 2. * rate / hz + 0.01);
                previous = shown;
            }
            // Behind a steady roll by 2 * damping / frequency seconds, less
            // the half frame each held input sample leads by.
            let lag = wrap(bank - previous) / rate;
            assert!((lag - (0.14 - 0.5 / hz)).abs() < 0.003, "{hz} Hz: {lag}");
        }
    }
    #[test]
    fn heading_wrap_and_horizon_projection() {
        assert!((heading(-0.1) - 354.270422).abs() < 0.00001);
        assert_eq!(project(0., 0., 0., 0., 1.), Some((320., 240.)));
        assert!(project(0.2, 0., 0., 0., 1.).unwrap().1 > 240.);
        assert!(project(0., 0., std::f64::consts::PI, 0., 1.).is_none());
        let a = project(0., 0., 0.1, 0., 1.).unwrap();
        let b = project(0., std::f64::consts::FRAC_PI_2, 0.1, 0., 1.).unwrap();
        assert!(a.0 > 320. && (b.0 - 320.).abs() < 0.001 && b.1 < 240.);
    }
    /// Offset of a screen point from the forward point along the bank normal
    /// (up the ladder) and along the bank tangent (across a rung).
    fn ladder_offsets(bank: f64, (x, y): (f64, f64)) -> (f64, f64) {
        let (dx, dy) = (x - 320., y - 240.);
        (
            -(dx * bank.sin() + dy * bank.cos()),
            dx * bank.cos() - dy * bank.sin(),
        )
    }
    const BANKS: [f64; 9] = [-180., -135., -90., -45., 0., 30., 45., 90., 180.];
    const RADIANS_TO_DEGREES: f64 = 180. / std::f64::consts::PI;
    const ZOOMS: [f64; 5] = [0.5, 1., 2., 4., 8.];

    #[test]
    fn ladder_is_literal_degrees_with_no_growth_or_compression() {
        // John, 2026-10-11: literal degrees, equal spacing, no acceleration.
        for pitch_deg in (-90..=90).step_by(5) {
            let pitch = (pitch_deg as f64).to_radians();
            for bank in BANKS.map(f64::to_radians) {
                for zoom in ZOOMS {
                    let per_degree = 240. * 3f64.sqrt() * zoom / RADIANS_TO_DEGREES;
                    for rung in (-90..=90).step_by(5) {
                        let relative = (rung - pitch_deg) as f64;
                        if relative.abs() >= 90. {
                            continue;
                        }
                        let at = ladder_project(pitch, bank, 0., (rung as f64).to_radians(), zoom)
                            .unwrap();
                        let (up, across) = ladder_offsets(bank, at);
                        assert!(
                            (up - relative * per_degree).abs() < 1e-6,
                            "rung {rung} pitch {pitch_deg} bank {bank} zoom {zoom}: {up}"
                        );
                        assert!(across.abs() < 1e-6);
                        // Equal spacing: the next rung is exactly five degrees on.
                        if let Some(next) =
                            ladder_project(pitch, bank, 0., ((rung + 5) as f64).to_radians(), zoom)
                        {
                            let (next_up, _) = ladder_offsets(bank, next);
                            assert!((next_up - up - 5. * per_degree).abs() < 1e-6);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn ladder_scale_is_the_true_projection_scale_at_the_centre() {
        // 240 * sqrt(3) pixels per radian at zoom 1: about 7.26 per degree.
        assert!((ladder_scale(1.) * 1f64.to_radians() - 7.2554).abs() < 1e-3);
        for zoom in ZOOMS {
            let step = 1e-6;
            let above = project(0., 0., 0., step, zoom).unwrap();
            assert!(((240. - above.1) / step - ladder_scale(zoom)).abs() < 1e-3 * zoom);
            // A rung N degrees from the nose is exactly N * scale away.
            for n in [1_f64, 5., 10., 45.] {
                let rung = ladder_project(0., 0., 0., n.to_radians(), zoom).unwrap();
                assert!((240. - rung.1 - n * ladder_scale(zoom) / RADIANS_TO_DEGREES).abs() < 1e-9);
            }
            // Within the visible window the true projection differs by a pixel or so.
            let edge = 11f64.to_radians();
            let true_edge = project(0., 0., 0., edge, 1.).unwrap().1;
            let ladder_edge = ladder_project(0., 0., 0., edge, 1.).unwrap().1;
            assert!((true_edge - ladder_edge).abs() < 2.5);
        }
    }

    #[test]
    fn ladder_rung_width_is_the_level_width_at_every_pitch() {
        for pitch in [-90_f64, -45., 0., 30., 85., 90.].map(f64::to_radians) {
            for bank in BANKS.map(f64::to_radians) {
                let left = ladder_project(pitch, bank, -3f64.to_radians(), pitch, 1.).unwrap();
                let right = ladder_project(pitch, bank, 3f64.to_radians(), pitch, 1.).unwrap();
                let (_, across_left) = ladder_offsets(bank, left);
                let (_, across_right) = ladder_offsets(bank, right);
                let width = ladder_scale(1.) * 2. * 3f64.to_radians().tan();
                assert!((across_right - across_left - width).abs() < 1e-6);
                assert!(ladder_offsets(bank, left).0.abs() < 1e-6);
            }
        }
        assert!(ladder_project(0., 0., std::f64::consts::PI, 0., 1.).is_none());
    }

    #[test]
    fn horizon_bar_is_on_the_same_scale_as_the_numbered_rungs() {
        for pitch_deg in [-60_f64, -10., -4., 0., 4., 10., 60.] {
            let pitch = pitch_deg.to_radians();
            for bank in BANKS.map(f64::to_radians) {
                for zoom in ZOOMS {
                    let per_degree = ladder_scale(zoom).to_radians();
                    let bar = ladder_project(pitch, bank, 0., 0., zoom).unwrap();
                    let (bar_up, _) = ladder_offsets(bank, bar);
                    assert!((bar_up + pitch_deg * per_degree).abs() < 1e-6);
                    // The bar is exactly 5 degrees from the 5 and -5 rungs.
                    for step in [-5_f64, 5.] {
                        let rung =
                            ladder_project(pitch, bank, 0., step.to_radians(), zoom).unwrap();
                        let (rung_up, _) = ladder_offsets(bank, rung);
                        assert!((rung_up - bar_up - step * per_degree).abs() < 1e-6);
                    }
                    // A level velocity vector (true projection) lies within the
                    // linear/true difference of the bar near the centre.
                    if pitch_deg.abs() <= 10. && zoom <= 1. {
                        let velocity = project(pitch, bank, 0., 0., zoom).unwrap();
                        let (velocity_up, _) = ladder_offsets(bank, velocity);
                        assert!((velocity_up - bar_up).abs() < 2.5, "{pitch_deg} {zoom}");
                    }
                }
            }
        }
    }

    #[test]
    fn ladder_has_visible_rungs_and_is_continuous_through_a_loop() {
        use crate::attitude::Basis;
        let mut basis = Basis::new(0., 0., 0.);
        let mut previous_pole: Option<(f64, f64)> = None;
        let mut previous_marks: Option<(f64, [(f64, f64); 3])> = None;
        for _ in 0..1440 {
            basis = basis.rotated(basis.right.map(|v| -v * std::f64::consts::TAU / 1440.));
            let [_, pitch, bank] = basis.angles();
            let visible = (-90..=90).step_by(5).any(|degree| {
                let elevation = (degree as f64).to_radians();
                [-3_f64, 3.].iter().all(|side| {
                    ladder_project(pitch, bank, side.to_radians(), elevation, 1.).is_some_and(
                        |(x, y)| (250. ..390.).contains(&x) && (182. ..348.).contains(&y),
                    )
                })
            });
            assert!(
                visible,
                "empty ladder at pitch {} bank {}",
                pitch.to_degrees(),
                bank.to_degrees()
            );
            // Away from the pole's bank flip, every mark moves smoothly: one
            // loop step is 0.25 degrees, 1.8 pixels of scale at zoom 1.
            let marks = [-5_f64, 0., 5.].map(|degrees| {
                ladder_project(pitch, bank, 0., degrees.to_radians(), 1.).unwrap_or((0., 0.))
            });
            if pitch.abs() < 80f64.to_radians() {
                if let Some((previous_bank, previous)) = previous_marks
                    && (bank - previous_bank).abs() < 0.01
                {
                    for (now, before) in marks.iter().zip(previous) {
                        assert!(
                            (now.0 - before.0).hypot(now.1 - before.1) < 2.2,
                            "mark jumped at pitch {}",
                            pitch.to_degrees()
                        );
                    }
                }
                previous_marks = Some((bank, marks));
            } else {
                previous_marks = None;
            }
            if pitch.abs() > 80f64.to_radians() {
                let pole = ladder_project(
                    pitch,
                    bank,
                    0.,
                    pitch.signum() * std::f64::consts::FRAC_PI_2,
                    1.,
                )
                .unwrap();
                if let Some(previous) = previous_pole {
                    assert!((pole.0 - previous.0).hypot(pole.1 - previous.1) < 2.);
                }
                previous_pole = Some(pole);
            } else {
                previous_pole = None;
            }
        }
    }

    #[test]
    fn rendered_ladder_marks_the_actual_pitch_at_the_forward_point() {
        let mut state = State::new(&tore_world::test_support::profile(), [0., 5000., 0.]).unwrap();
        let font = Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 6,
                    pixels: vec![],
                })
                .collect(),
        };
        for degrees in (-90..=90).step_by(5) {
            state.pitch = (degrees as f64).to_radians();
            let mut with = vec![0; 640 * 480 * 4];
            let mut without = with.clone();
            draw(
                &mut with,
                &state,
                &font,
                0.,
                None,
                true,
                false,
                [0, 255, 0],
                1.,
                None,
                None,
                (state.bank, 1.),
            );
            draw(
                &mut without,
                &state,
                &font,
                0.,
                None,
                false,
                false,
                [0, 255, 0],
                1.,
                None,
                None,
                (state.bank, 1.),
            );
            let visible = (297..316).any(|x| {
                let at = (240 * 640 + x) * 4;
                with[at..at + 4] != without[at..at + 4]
            });
            assert!(
                visible,
                "actual {degrees}-degree rung missing at the forward point"
            );
        }
    }

    #[test]
    fn rendered_horizon_bar_sits_on_the_literal_scale() {
        let mut state = State::new(&tore_world::test_support::profile(), [0., 5000., 0.]).unwrap();
        state.pitch = 4f64.to_radians();
        let font = Font {
            height: 8,
            glyphs: (0..256)
                .map(|_| tore_formats::font::Glyph {
                    advance: 6,
                    pixels: vec![],
                })
                .collect(),
        };
        let mut with = vec![0; 640 * 480 * 4];
        let mut without = with.clone();
        draw(
            &mut with,
            &state,
            &font,
            0.,
            None,
            true,
            false,
            [0, 255, 0],
            1.,
            None,
            None,
            (state.bank, 1.),
        );
        draw(
            &mut without,
            &state,
            &font,
            0.,
            None,
            false,
            false,
            [0, 255, 0],
            1.,
            None,
            None,
            (state.bank, 1.),
        );
        let horizon = ladder_project(state.pitch, 0., 0., 0., 1.)
            .unwrap()
            .1
            .round() as usize;
        let elsewhere = horizon + 9;
        let difference = |row: usize| {
            (298..306).any(|x| {
                let at = (row * 640 + x) * 4;
                with[at..at + 4] != without[at..at + 4]
            })
        };
        assert!(difference(horizon));
        assert!(!difference(elsewhere));
    }

    #[test]
    fn velocity_marker_matches_body_axes_through_banked_pulls() {
        use crate::attitude::{Basis, dot};
        for bank in [-2., -0.8, 0., 0.8, 2.] {
            let body = Basis::new(0.3, 0.4, bank);
            // Synthetic nose-up AoA plus side-slip, rotated with the aircraft.
            let velocity = std::array::from_fn(|i| {
                body.forward[i] * 700. - body.up[i] * 40. + body.right[i] * 15.
            });
            let gamma = velocity[1].atan2(velocity[0].hypot(velocity[2]));
            let bearing = velocity[0].atan2(velocity[2]) - 0.3;
            let (x, y) = project(0.4, bank, bearing, gamma, 1.).unwrap();
            let focal = 240. * 3f64.sqrt();
            let z = dot(velocity, body.forward);
            assert!((x - (320. + focal * dot(velocity, body.right) / z)).abs() < 1e-9);
            assert!((y - (240. - focal * dot(velocity, body.up) / z)).abs() < 1e-9);
        }
    }
}
