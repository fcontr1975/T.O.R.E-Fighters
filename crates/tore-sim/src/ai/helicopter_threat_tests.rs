//! A helicopter's threat awareness (docs/spec/ai-strike.md, "Helicopter
//! threat awareness"): rings, the clear line, the attack plan, the hold
//! point and its terrain masking, the ambush ring. Synthetic geometry only.
use super::*;
use crate::ai::awareness::ground::DefenseFacts;
use crate::ai::targeting::Side;

fn defense(id: u32, at: Vector, missile: f64, gun: f64, working: bool) -> KnownDefense {
    KnownDefense {
        id,
        side: Side(2),
        position: at,
        facts: DefenseFacts {
            missile_reach_ft: missile,
            rotor_reach_ft: missile,
            gun_reach_ft: gun,
            radar: true,
            working,
            ship: false,
            heading_deg: 0.,
        },
        briefed: false,
    }
}

/// The AT-2 as the plan sees it, with no flight time known (the fixed
/// advance).
fn at2() -> StandOff<'static> {
    StandOff {
        reach_ft: 18_000.,
        fire_ft: 10_000.,
        flight: None,
        guide_fps: 200.,
        shots: None,
    }
}

fn ring(id: u32, at: Vector, radius: f64) -> Ring {
    Ring {
        id,
        centre: at,
        radius_ft: radius,
    }
}

#[test]
fn a_ring_is_the_reach_that_can_hurt_plus_the_margin() {
    let defenses = [
        defense(1, [0.; 3], 20_000., 3_000., true),
        // A launcher whose battery radar is dead keeps only its guns.
        defense(2, [0.; 3], 20_000., 3_000., false),
        // A lone search radar reaches nothing.
        defense(3, [0.; 3], 0., 0., true),
    ];
    let rings = rings(defenses.iter());
    assert_eq!(rings.len(), 2);
    assert_eq!(rings[0].radius_ft, 20_000. + RING_MARGIN_FT);
    assert_eq!(rings[1].radius_ft, 3_000. + RING_MARGIN_FT);
}

#[test]
fn a_ring_covers_the_sites_real_launch_range_at_a_helicopter() {
    // A shoulder-launched site whose zone ends at 13,500 ft but whose launch
    // gate fires at a helicopter from 16,600 ft (the energy rules, the
    // host's bound in `rotor_reach_ft`): the ring is that reach plus the
    // margin, so a helicopter flying round it is never launched at
    // (John, 2026-10-29).
    let mut site = defense(1, [0.; 3], 13_500., 0., true);
    site.facts.rotor_reach_ft = 16_600.;
    let ring = rings(std::iter::once(&site))[0];
    assert_eq!(ring.radius_ft, 16_600. + RING_MARGIN_FT);
    // Every launch point lies inside the ring the helicopter plans with.
    for bearing in [0., 90., 200., 315.] {
        let [x, z] = along([0.; 3], bearing, 16_600.);
        assert!(inside(&[ring], [x, 300., z]).is_some());
    }
    // A reach under the zone (a long-range site at a low helicopter) keeps
    // the zone.
    site.facts.rotor_reach_ft = 9_000.;
    assert_eq!(
        rings(std::iter::once(&site))[0].radius_ft,
        13_500. + RING_MARGIN_FT
    );
}

#[test]
fn a_line_is_clear_outside_every_ring_and_may_always_lead_out_of_one() {
    let rings = [ring(1, [0., 0., 10_000.], 3_000.)];
    // Straight through the ring: blocked; beside it: clear.
    assert!(!clear(&rings, [0.; 3], [0., 0., 20_000.]));
    assert!(clear(&rings, [5_000., 0., 0.], [5_000., 0., 20_000.]));
    // From inside, outward: clear; deeper in: blocked.
    assert!(clear(&rings, [0., 0., 9_000.], [0., 0., 0.]));
    assert!(!clear(&rings, [0., 0., 8_000.], [0., 0., 10_500.]));
}

#[test]
fn a_line_from_inside_a_ring_must_lead_straight_out_not_skirt_deeper_first() {
    // The Ukraine column's SA-14 of surface seed 16 in miniature: the
    // helicopter starts inside the ring and the target lies outside it, but
    // the line to the target passes nearer the site before it leaves.
    let rings = [ring(1, [0.; 3], 15_500.)];
    let own = [12_167., 0., 0.];
    let skirting = [-2_000., 0., 24_000.];
    assert!(horizontal(rings[0].centre, skirting) > 15_500.);
    assert!(!clear(&rings, own, skirting));
    // Away from the site it is clear, and a pop-up across it is not planned.
    assert!(clear(&rings, own, [30_000., 0., 4_000.]));
    assert_eq!(plan_attack(own, skirting, None, false, &rings), None);
}

#[test]
fn a_ring_learned_after_the_choice_blocks_the_pass_in_hand() {
    let own = [0., 1_500., 0.];
    let target = [0., 0., 24_000.];
    let at2 = Some(at2());
    assert!(holds(Plan::PopUp, own, target, at2, &[]));
    // A gun 2,650 ft beyond the target: its ring covers the target, so the
    // pop-up in hand no longer holds, though a stand-off from outside the
    // ring still stands (the old test kept the pop-up for that reason).
    let gun = [ring(1, [0., 0., 26_650.], 9_500.)];
    assert!(!holds(Plan::PopUp, own, target, at2, &gun));
    let plan = plan_attack(own, target, at2, false, &gun);
    assert!(matches!(plan, Some(Plan::StandOff { .. })), "{plan:?}");
    // The plan chosen with the ring known holds while it is flown.
    assert!(holds(plan.unwrap(), own, target, at2, &gun));
    // A straight stand-off whose carry-on would end in a new ring does not.
    let straight = Plan::StandOff {
        approach: None,
        range_ft: 10_800.,
    };
    assert!(holds(straight, own, target, at2, &[]));
    let near = [ring(2, [0., 0., 16_000.], 3_000.)];
    assert!(!holds(straight, own, target, at2, &near));
}

#[test]
fn the_plan_is_the_pop_up_when_clear_else_a_stand_off_from_outside_the_rings() {
    let own = [0., 1_500., 0.];
    let target = [0., 0., 24_000.];
    let at2 = Some(at2());
    // Nothing around: the pop-up, or the straight stand-off when asked.
    assert_eq!(plan_attack(own, target, at2, false, &[]), Some(Plan::PopUp));
    assert_eq!(
        plan_attack(own, target, at2, true, &[]),
        Some(Plan::StandOff {
            approach: None,
            range_ft: 0.6 * 18_000.
        })
    );
    // A gun ring on the target: no pop-up and no rockets, but the AT-2
    // reaches from outside it.
    let gun = [ring(1, [0., 0., 24_500.], 7_000.)];
    assert_eq!(plan_attack(own, target, None, false, &gun), None);
    assert!(matches!(
        plan_attack(own, target, at2, false, &gun),
        Some(Plan::StandOff { .. })
    ));
    // A ring across the straight line: the firing point slides round the
    // target, off the line, and the way to it is clear.
    let across = [ring(2, [0., 0., 15_500.], 4_000.)];
    match plan_attack(own, target, at2, true, &across) {
        Some(Plan::StandOff {
            approach: Some(point),
            range_ft,
        }) => {
            assert!(inside(&across, point).is_none());
            assert!(clear(&across, own, point));
            assert!((horizontal(point, target) - range_ft).abs() < 1.);
        }
        other => panic!("{other:?}"),
    }
    // A SAM that out-ranges the AT-2 covering everything: no attack.
    let sam = [ring(3, [3_000., 0., 24_000.], 32_000.)];
    assert_eq!(plan_attack(own, target, at2, true, &sam), None);
}

#[test]
fn the_stand_off_advance_scales_with_the_stores_time_of_flight() {
    use crate::ai::strike::FlightTimes;
    let flight = FlightTimes {
        points: [(4_500., 7.), (9_000., 14.), (13_500., 20.), (18_000., 26.)],
    };
    let slow = StandOff {
        flight: Some(flight),
        guide_fps: 200.,
        ..at2()
    };
    // From 10,800 ft the round flies 16.4 s: the helicopter carries on that
    // long at its guiding speed, plus the margin.
    let expect = 16.4 * 200. + ADVANCE_MARGIN_FT;
    assert!((slow.advance_ft(10_800.) - expect).abs() < 1e-6);
    // A longer flight carries it farther, and so does a faster helicopter.
    assert!(slow.advance_ft(16_000.) > slow.advance_ft(10_800.));
    let fast = StandOff {
        guide_fps: 250.,
        ..slow
    };
    assert!((fast.advance_ft(10_800.) - (16.4 * 250. + ADVANCE_MARGIN_FT)).abs() < 1e-6);
    // Never under the old fixed advance, and that with no flight time known.
    assert_eq!(slow.advance_ft(500.), STANDOFF_ADVANCE_FT);
    assert_eq!(at2().advance_ft(10_800.), STANDOFF_ADVANCE_FT);
    // Past the last point, at the last stretch's pace.
    assert!((flight.seconds_to(22_500.) - 32.).abs() < 1e-9);

    // The firing point leaves room for the advance outside every ring: a
    // ring 4,000 ft short of the target on the straight line clears the old
    // 2,500 ft carry from the 10,800 ft firing point, not the flight's.
    let own = [0., 1_500., 0.];
    let target = [0., 0., 24_000.];
    let near = [ring(2, [0., 0., 20_000.], 3_000.)];
    assert_eq!(
        plan_attack(own, target, Some(at2()), true, &near),
        Some(Plan::StandOff {
            approach: None,
            range_ft: 0.6 * 18_000.
        })
    );
    let plan = plan_attack(own, target, Some(slow), true, &near);
    match plan {
        Some(Plan::StandOff {
            approach: Some(point),
            range_ft,
        }) => {
            let [ex, ez] = along(
                target,
                bearing(target, point),
                range_ft - slow.advance_ft(range_ft),
            );
            assert!(clear(&near, point, [ex, own[1], ez]));
            assert!(holds(plan.unwrap(), own, target, Some(slow), &near));
        }
        other => panic!("{other:?}"),
    }
    // The straight shot the old advance took no longer holds.
    let straight = Plan::StandOff {
        approach: None,
        range_ft: 0.6 * 18_000.,
    };
    assert!(holds(straight, own, target, Some(at2()), &near));
    assert!(!holds(straight, own, target, Some(slow), &near));
}

#[test]
fn a_known_site_that_lies_along_the_threat_is_the_shooter() {
    let site = ring(7, [0., 0., 10_000.], 15_000.);
    let rings = [site, ring(AMBUSH_ID, [0., 0., -5_000.], AMBUSH_RADIUS_FT)];
    let own = [0., 300., 0.];
    // Toward the site, within 20 degrees: the site it knew.
    assert_eq!(known_shooter(&rings, own, Some(0.)).map(|r| r.id), Some(7));
    assert_eq!(
        known_shooter(&rings, own, Some(345.)).map(|r| r.id),
        Some(7)
    );
    // From elsewhere (the ambush ring is no site): a shooter it did not know.
    assert_eq!(known_shooter(&rings, own, Some(90.)), None);
    assert_eq!(known_shooter(&rings, own, Some(180.)), None);
    // Too far out for the site to have fired, whatever the bearing.
    let far = [0., 300., 10_000. - 15_000. - RING_MARGIN_FT - 1.];
    assert_eq!(known_shooter(&rings, far, Some(0.)), None);
    // With no bearing: the ring it is inside, else nothing.
    assert_eq!(known_shooter(&rings, own, None).map(|r| r.id), Some(7));
    assert_eq!(known_shooter(&rings, [0., 0., 30_000.], None), None);
    // On top of the site the bearing says nothing.
    assert_eq!(
        known_shooter(&rings, [100., 0., 10_000.], Some(200.)).map(|r| r.id),
        Some(7)
    );
}

#[test]
fn the_hold_point_leaves_room_for_its_circle_outside_every_ring() {
    let rings = [
        ring(1, [0., 0., 24_000.], 20_000.),
        ring(2, [6_000., 0., 4_000.], 3_000.),
    ];
    let own = [0., 1_500., 6_000.];
    let centre = hold_centre(own, &rings, None);
    for r in &rings {
        assert!(
            horizontal(r.centre, centre) >= r.radius_ft + HOLD_RADIUS_FT - 1.,
            "{centre:?} against {r:?}"
        );
    }
    // Already clear: it holds where it is.
    let far = [0., 1_500., -20_000.];
    assert_eq!(hold_centre(far, &rings, None), far);
}

#[test]
fn the_hold_point_slides_round_the_site_behind_a_ridge() {
    // A ridge east of the site's south line: the point due south of the site
    // is in the open, the point 30 degrees round toward the east (or 60)
    // lies behind the ridge.
    let site = [0., 0., 0.];
    let rings = [ring(1, site, 10_000.)];
    let ground = |x: f64, z: f64| {
        let bearing = x.atan2(z).to_degrees().rem_euclid(360.);
        let distance = x.hypot(z);
        if (100.0..170.).contains(&bearing) && (8_000.0..9_000.).contains(&distance) {
            3_000.
        } else {
            0.
        }
    };
    let own = [0., 1_500., -9_000.];
    let open = hold_centre(own, &rings, None);
    assert!(!masked(site, open, &ground));
    let hidden = hold_centre(own, &rings, Some(&ground));
    assert!(masked(site, hidden, &ground), "{hidden:?}");
    assert!(horizontal(site, hidden) >= 10_000. + HOLD_RADIUS_FT - 1.);
}

#[test]
fn an_ambush_ring_lies_toward_the_threat_and_is_deterministic() {
    let own = [1_000., 500., 2_000.];
    let east = ambush_ring(own, Some(90.));
    assert_eq!(east.id, AMBUSH_ID);
    assert!((east.centre[0] - (1_000. + AMBUSH_OFFSET_FT)).abs() < 1e-6);
    assert!((east.centre[2] - 2_000.).abs() < 1e-6);
    assert_eq!(east.radius_ft, AMBUSH_RADIUS_FT);
    assert_eq!(ambush_ring(own, None).centre, own);
    assert_eq!(ambush_ring(own, Some(90.)), east);
}

#[test]
fn the_decisions_have_fixed_reasons_and_words() {
    for (decision, reason) in [
        (Decision::HoldOff, "threat_hold_off"),
        (Decision::Resume, "threat_resume"),
        (Decision::Retarget, "threat_retarget"),
        (Decision::Home, "threat_home"),
        (Decision::Leave, "threat_leave"),
    ] {
        assert_eq!(decision.reason(), reason);
        assert!(!decision.words().is_empty());
    }
}

// The stand-off's search out to the store's reliable reach (T.O.R.E
// opinionated change, John 2026-10-29: "If an attack aircraft can hit
// outside of range safely of known threats AI should do that").

/// The host's answers as a test gives them: a hit, at 700 ft/s, where
/// `hits` says so.
struct Table(Box<dyn Fn(f64) -> bool + Send + Sync>);

impl crate::ai::strike::ShotTable for Table {
    fn shot(&self, _: StationId, _: ShotLaunch, range_ft: f64) -> Option<f64> {
        (self.0)(range_ft).then_some(range_ft / 700.)
    }
}

fn shots(hits: impl Fn(f64) -> bool + Send + Sync + 'static) -> Shots {
    Shots(Some(std::sync::Arc::new(Table(Box::new(hits)))))
}

/// The AT-2 with the energy rules' flight times (about 25 s to 13,500 ft)
/// at a guiding speed of 219 ft/s (120 kt and the adapter's 10), and the
/// host's answers from a 700 ft unmask.
fn at2_with(shots: &Shots) -> StandOff<'_> {
    StandOff {
        reach_ft: 18_000.,
        fire_ft: 10_000.,
        flight: Some(FlightTimes {
            points: [
                (4_500., 7.7),
                (9_000., 14.6),
                (13_500., 24.7),
                (18_000., 34.8),
            ],
        }),
        guide_fps: 219.,
        shots: Some(StandOffShots {
            shots,
            station: StationId(1),
            launch: ShotLaunch {
                target_ft: 0.,
                height_ft: 700.,
                speed_fps: 203.,
            },
        }),
    }
}

/// 30,000 ft south of a target at the origin, masked at 200 ft.
const OWN: Vector = [0., 200., -30_000.];
const TARGET: Vector = [0.; 3];

#[test]
fn a_safe_far_firing_range_is_chosen_when_the_near_ones_are_not() {
    // A gun ring of 7,000 ft about the target (the heavy-AAA column's
    // shape): from the usual 10,800 ft the AT-2's 18.6 s of flight carries
    // the helicopter to 5,700 ft from the target, inside it, and from the
    // fire range likewise. The AT-2 flies faster than the helicopter, so the
    // advance's end moves out as the range grows: 13,500 ft is the nearest
    // grid range whose advance ends outside the ring (7,091 ft).
    let table = shots(|_| true);
    let st = at2_with(&table);
    let rings = [ring(1, TARGET, 7_000.)];
    for usual in [STANDOFF_SHARE * 18_000., FIRE_RANGE_SHARE * 10_000.] {
        assert!(usual - st.advance_ft(usual) < 7_000., "{usual}");
    }
    let plan = plan_attack(OWN, TARGET, Some(st), true, &rings);
    let Some(Plan::StandOff {
        approach: Some(point),
        range_ft,
    }) = plan
    else {
        panic!("{plan:?}");
    };
    assert_eq!(range_ft, 13_500.);
    // Straight in from the south, the firing point and the advance's end
    // both outside the ring.
    assert!((point[0]).abs() < 1e-6 && (point[2] + 13_500.).abs() < 1e-6);
    assert!(range_ft - st.advance_ft(range_ft) >= 7_000.);
    assert!(holds(plan.unwrap(), OWN, TARGET, Some(st), &rings));
    // The same every time.
    assert_eq!(plan_attack(OWN, TARGET, Some(st), true, &rings), plan);
    // Without the host's answers it looks no farther than the usual ranges:
    // it holds off, as before.
    let blind = StandOff { shots: None, ..st };
    assert_eq!(plan_attack(OWN, TARGET, Some(blind), true, &rings), None);
}

#[test]
fn it_holds_off_only_when_no_range_within_reach_is_safe() {
    let rings = [ring(1, TARGET, 7_000.)];
    // Its shots hit to 13,000 ft only: 13,500 ft would be clear but is not
    // in reach, and no nearer range is clear. It holds off.
    let short = shots(|r| r <= 13_000.);
    assert_eq!(
        plan_attack(OWN, TARGET, Some(at2_with(&short)), true, &rings),
        None
    );
    // A ring of 9,000 ft: even the farthest reliable range (17,000 ft, one
    // step inside the 17,986 ft the zone allows from 700 ft) ends its
    // advance at 8,870 ft. It holds off.
    let full = shots(|_| true);
    let wide = [ring(1, TARGET, 9_000.)];
    assert_eq!(
        plan_attack(OWN, TARGET, Some(at2_with(&full)), true, &wide),
        None
    );
    // With the reach and the 7,000 ft ring, it attacks.
    assert!(plan_attack(OWN, TARGET, Some(at2_with(&full)), true, &rings).is_some());
}

#[test]
fn a_range_whose_shot_strikes_the_ground_is_never_planned() {
    // The AT-2 from a low hover strikes the ground over a band of mid
    // ranges (docs/baselines/missile-energy.md). Here the band covers
    // 9,000 to 11,500 ft, both usual ranges among them. A small ring well
    // off the line (rings in play) leaves the usual firing points clear,
    // but their shots would not hit: the plan takes the nearest reliable
    // range past the band, 12,500 ft (12,000 and 13,000 ft hit too).
    let banded = shots(|r| !(9_000. ..=11_500.).contains(&r));
    let aside = [ring(1, [20_000., 0., 0.], 3_000.)];
    let plan = plan_attack(OWN, TARGET, Some(at2_with(&banded)), true, &aside);
    assert!(
        matches!(
            plan,
            Some(Plan::StandOff {
                approach: Some(_),
                range_ft: 12_500.
            })
        ),
        "{plan:?}"
    );
    // With no ring at all the run is as before (the ST-F stand-off straight
    // in at the usual range; its release gate waits for a shot that hits).
    assert_eq!(
        plan_attack(OWN, TARGET, Some(at2_with(&banded)), true, &[]),
        Some(Plan::StandOff {
            approach: None,
            range_ft: STANDOFF_SHARE * 18_000.
        })
    );
    // A band past the ring: 13,500 to 14,500 ft strike the ground, and
    // 15,000 ft has the band one step nearer; 15,500 ft is the plan.
    let far_band = shots(|r| !(13_500. ..=14_500.).contains(&r));
    let rings = [ring(1, TARGET, 7_000.)];
    let plan = plan_attack(OWN, TARGET, Some(at2_with(&far_band)), true, &rings);
    assert!(
        matches!(
            plan,
            Some(Plan::StandOff {
                approach: Some(_),
                range_ft: 15_500.
            })
        ),
        "{plan:?}"
    );
}
