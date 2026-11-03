//! Whole-degree sine and cosine as integers, so every platform rotates a
//! template, places a start and measures a box the same way to the foot
//! (docs/spec/surface-defenses.md, "Relocation"). Floating point sine and
//! cosine are not bit-identical across platforms; this table is.
//!
//! Values are scaled by 2^30 (the plan named 2^16; at 2^16 one table step is
//! 3.7 ft over a 40 nm lever, at 2^30 it is under a thousandth of a foot).

/// The scale of [`sin`] and [`cos`]: 1.0 is `1 << 30`.
pub const ONE: i64 = 1 << 30;

/// `sin(d)` for whole degrees 0 to 90, scaled by 2^30, rounded to nearest.
const QUARTER: [i64; 91] = [
    0, 18739379, 37473049, 56195305, 74900443, 93582766, 112236583, 130856211, 149435979,
    167970228, 186453311, 204879599, 223243478, 241539355, 259761657, 277904834, 295963357,
    313931728, 331804471, 349576144, 367241333, 384794656, 402230767, 419544355, 436730145,
    453782903, 470697435, 487468587, 504091252, 520560366, 536870912, 553017922, 568996477,
    584801711, 600428808, 615873009, 631129609, 646193961, 661061475, 675727625, 690187940,
    704438018, 718473518, 732290163, 745883746, 759250125, 772385229, 785285058, 797945680,
    810363241, 822533958, 834454122, 846120104, 857528349, 868675383, 879557810, 890172315,
    900515665, 910584710, 920376381, 929887697, 939115760, 948057759, 956710970, 965072759,
    973140576, 980911966, 988384560, 995556083, 1002424350, 1008987269, 1015242840, 1021189159,
    1026824413, 1032146887, 1037154959, 1041847103, 1046221891, 1050277989, 1054014162, 1057429273,
    1060522280, 1063292242, 1065738315, 1067859754, 1069655912, 1071126243, 1072270298, 1073087729,
    1073578288, 1073741824,
];

/// `degrees` folded into 0 to 359.
pub fn wrap(degrees: i32) -> i32 {
    degrees.rem_euclid(360)
}

/// A heading folded into -179 to 180, the range the retail `angle` words
/// use.
pub fn signed(degrees: i32) -> i32 {
    let d = wrap(degrees);
    if d > 180 { d - 360 } else { d }
}

/// `sin(degrees)`, scaled by [`ONE`].
pub fn sin(degrees: i32) -> i64 {
    let d = wrap(degrees) as usize;
    match d {
        0..=90 => QUARTER[d],
        91..=180 => QUARTER[180 - d],
        181..=270 => -QUARTER[d - 180],
        _ => -QUARTER[360 - d],
    }
}

/// `cos(degrees)`, scaled by [`ONE`].
pub fn cos(degrees: i32) -> i64 {
    sin(degrees + 90)
}

/// `value / ONE`, rounded half away from zero.
pub fn unscale(value: i64) -> i64 {
    let half = ONE / 2;
    if value >= 0 {
        (value + half) / ONE
    } else {
        -((-value + half) / ONE)
    }
}

/// `length` feet along the compass `bearing` (degrees clockwise from north,
/// X east and Z north), as whole feet `[x, z]`.
pub fn along(bearing: i32, length: i64) -> [i64; 2] {
    [
        unscale(length * sin(bearing)),
        unscale(length * cos(bearing)),
    ]
}

/// Whole-foot vector for an authored fractional-degree lane. Linear
/// interpolation of the integer table avoids platform trigonometry; the
/// error is below one foot over a 12,000-foot runway.
pub fn along_millidegrees(bearing: i32, length: i64) -> [i64; 2] {
    let degrees = bearing.div_euclid(1000);
    let fraction = i64::from(bearing.rem_euclid(1000));
    let interpolate = |table: fn(i32) -> i64| {
        (table(degrees) * (1000 - fraction) + table(degrees + 1) * fraction) / 1000
    };
    [
        unscale(length * interpolate(sin)),
        unscale(length * interpolate(cos)),
    ]
}

/// `[x, z]` turned clockwise (seen from above) by `degrees`, whole feet.
/// A unit facing north turned by `d` faces heading `d`.
pub fn rotate(point: [i64; 2], degrees: i32) -> [i64; 2] {
    let (s, c) = (sin(degrees), cos(degrees));
    [
        unscale(point[0] * c + point[1] * s),
        unscale(point[1] * c - point[0] * s),
    ]
}

/// The point in a frame turned to `heading`: `[right, forward]`, scaled by
/// [`ONE`] (not rounded, so box tests stay exact).
pub fn to_local_scaled(point: [i64; 2], heading: i32) -> [i64; 2] {
    let (s, c) = (sin(heading), cos(heading));
    [point[0] * c - point[1] * s, point[0] * s + point[1] * c]
}

/// The whole-degree compass bearing (0 to 359) closest to the direction of
/// `vector` `[x, z]`; 0 for the zero vector. Ties go to the lower bearing.
pub fn bearing(vector: [i64; 2]) -> i32 {
    if vector == [0, 0] {
        return 0;
    }
    let mut best = (i128::MIN, 0);
    for d in 0..360 {
        let dot =
            i128::from(vector[0]) * i128::from(sin(d)) + i128::from(vector[1]) * i128::from(cos(d));
        if dot > best.0 {
            best = (dot, d);
        }
    }
    best.1
}

/// The integer square root: the largest `r` with `r * r <= n`.
pub fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    // Newton's method from a power of two above the root.
    let mut x = 1u128 << (128 - n.leading_zeros()).div_ceil(2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_matches_floating_point_within_half_a_foot_over_forty_miles() {
        let lever = 40 * 6076_i64;
        for d in -720..720 {
            let [x, z] = along(d, lever);
            let r = f64::from(d).to_radians();
            assert!((x as f64 - lever as f64 * r.sin()).abs() <= 0.5, "{d}");
            assert!((z as f64 - lever as f64 * r.cos()).abs() <= 0.5, "{d}");
        }
    }

    #[test]
    fn rotation_turns_clockwise_and_keeps_length() {
        assert_eq!(rotate([0, 1000], 90), [1000, 0]);
        assert_eq!(rotate([1000, 0], 90), [0, -1000]);
        assert_eq!(rotate([0, 1000], 180), [0, -1000]);
        assert_eq!(rotate([300, -400], 0), [300, -400]);
        for d in 0..360 {
            let [x, z] = rotate([30_000, 40_000], d);
            let length = ((x * x + z * z) as f64).sqrt();
            assert!((length - 50_000.).abs() <= 1.0, "{d}: {length}");
        }
    }

    #[test]
    fn bearings_round_trip_and_headings_fold() {
        for d in 0..360 {
            assert_eq!(bearing(along(d, 100_000)), d);
        }
        assert_eq!(signed(190), -170);
        assert_eq!(signed(-180), 180);
        assert_eq!(wrap(-1), 359);
    }

    #[test]
    fn integer_square_roots_floor() {
        for n in [0u128, 1, 2, 3, 4, 15, 16, 17, 1 << 40, (1 << 62) + 12345] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n, "{n}");
        }
    }

    #[test]
    fn rounding_is_half_away_from_zero() {
        assert_eq!(unscale(ONE / 2), 1);
        assert_eq!(unscale(-ONE / 2), -1);
        assert_eq!(unscale(ONE / 2 - 1), 0);
    }
}
