"""Tests for the AI strike driver's scenarios (tools/battery_scenarios/ai_strike.py): the output
parsing, the consistency and behaviour checks, the release aim checks and the battery selection
family (AI strike round, slices ST-D, ST-T, ST-T2 and ST-L)."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import battery_selection as bs  # noqa: E402
from battery_scenarios import ai_strike as s  # noqa: E402

SETUP = (
    "ai-strike: setup aircraft=A4E load=MK82.JT station=1 count=4 rounds=4 target=0x50000001 "
    "target_at=999570,7,789571 hp=200 skill={skill} profile={profile} handback=none order=none mode=none"
)


def run(skill=2, profile="dive", named=None, releases=(), impacts=(), kills=(), egress=None, lost=None,
        hp="200/200", extra=(), aim=(100.0, "yes"), release_fields=""):
    """One driver run's output. `releases` and `impacts` are times; an impact is (t, miss)."""
    name = named if named is not None else "none"
    out = [
        SETUP.format(skill=skill, profile=profile),
        f"ai-strike: PROFILE t=0.000 name={name} requested={profile} note=no-strike-behaviour",
        "ai-strike: start t=0.000 plane=0 range_ft=24304 alt_ft=5000 speed_kt=450",
        "ai-strike: ACTIVITY t=0.008 name=In formation",
        *extra,
    ]
    for n, t in enumerate(releases, 1):
        out.append(
            f"ai-strike: RELEASE t={t:.3f} n={n} weapon=MK82.JT station=1 alt_ft=4900 speed_kt=450 range_ft=13000"
            + (f" pass=1 aim_ft={aim[0]:.0f} inside={aim[1]}" if aim else "")
            + release_fields
        )
    for n, (t, miss) in enumerate(impacts, 1):
        out.append(f"ai-strike: IMPACT t={t:.3f} n={n} miss_ft={miss} at=1.0,2.0,3.0")
    for t in kills:
        out.append(f"ai-strike: KILL t={t:.3f} victim=0x50000001 category=0x0001 target=yes")
    if egress is not None:
        out.append(f"ai-strike: EGRESS t={egress:.3f} range_ft=6000 alt_ft=5000")
    if lost is not None:
        out.append(f"ai-strike: LOST t={lost:.3f} reason=destroyed")
    out.append(
        f"ai-strike: summary t=60.000 ran_s=60.0 profile={name} releases={len(releases)} impacts={len(impacts)} "
        f"kills={len(kills)} egress={'yes' if egress is not None else 'no'} lost={'yes' if lost is not None else 'no'} "
        f"activity=In formation target_hp={hp} closest_ft=200 store_rounds_left=3"
    )
    if not releases:
        out.append("ai-strike: no release")
    out.append("ai-strike: done")
    return "\n".join(out)


class ParsingTests(unittest.TestCase):
    def test_runs_split_at_done_and_fields_parse(self):
        runs = s.parse_runs(run(skill=1) + "\n" + run(skill=3, releases=[10.0], impacts=[(27.5, 88.5)]))
        self.assertEqual(len(runs), 2)
        self.assertEqual([r.skill for r in runs], [1, 3])
        self.assertEqual(runs[0].setup["load"], "MK82.JT")
        self.assertEqual(runs[1].of("IMPACT")[0]["miss_ft"], "88.5")
        self.assertEqual(runs[1].times("RELEASE"), [10.0])
        self.assertEqual(runs[1].summary["releases"], "1")

    def test_no_release_line_is_its_own_kind(self):
        (parsed,) = s.parse_runs(run())
        self.assertEqual([k for k, _ in parsed.lines].count("no-release"), 1)

    def test_a_quoted_value_keeps_its_spaces(self):
        fields = s.parse_line("ORDER-REPLY", 't=3.025 result=refused message="Wing order unavailable: no valid hostile target"')
        self.assertEqual(fields["result"], "refused")
        self.assertEqual(fields["message"], "Wing order unavailable: no valid hostile target")

    def test_other_output_is_ignored(self):
        text = "a2g-run: fitted station 1\nnoise\n" + run()
        self.assertEqual(len(s.parse_runs(text)), 1)
        self.assertEqual(s.parse_runs("nothing here"), [])

    def test_number_reads_nan_for_junk(self):
        self.assertEqual(s.number("12.5"), 12.5)
        self.assertNotEqual(s.number("nan"), s.number("nan"))
        self.assertNotEqual(s.number(None), s.number(None))


class ConsistencyTests(unittest.TestCase):
    def check(self, text):
        (parsed,) = s.parse_runs(text)
        return s.consistency_problems(parsed, "run")

    def test_a_run_that_released_nothing_is_consistent(self):
        self.assertEqual(self.check(run()), [])

    def test_a_full_strike_is_consistent(self):
        text = run(named="dive", releases=[10.0, 10.5], impacts=[(27.0, 40), (27.5, 60)], kills=[27.5], egress=33.0)
        self.assertEqual(self.check(text), [])

    def test_the_summary_must_count_the_lines(self):
        text = run(releases=[10.0], impacts=[(27.0, 40)]).replace("releases=1", "releases=2")
        self.assertTrue(any("releases=2" in p for p in self.check(text)))

    def test_an_impact_needs_a_release_before_it(self):
        text = run(releases=[30.0], impacts=[(27.0, 40)])
        self.assertTrue(any("impact before the first release" in p for p in self.check(text)))

    def test_egress_needs_a_release_before_it(self):
        text = run(egress=12.0)
        self.assertTrue(any("egress before" in p for p in self.check(text)))

    def test_losing_the_striker_is_reported_in_the_summary(self):
        self.assertEqual(self.check(run(lost=22.0)), [])
        text = run(lost=22.0).replace("lost=yes", "lost=no")
        self.assertTrue(any("lost=no" in p for p in self.check(text)))

    def test_silence_about_a_missing_release_is_a_problem(self):
        text = run().replace("ai-strike: no release\n", "")
        self.assertTrue(any("does not say so" in p for p in self.check(text)))

    def test_events_out_of_time_order_are_a_problem(self):
        text = run(releases=[20.0], impacts=[(21.0, 5)], egress=10.5)
        self.assertTrue(any("out of order" in p or "egress before" in p for p in self.check(text)))


class DriverTests(unittest.TestCase):
    def test_three_honest_runs_pass(self):
        text = "\n".join([run(2, "dive"), run(0, "popup"), run(3, "none")])
        self.assertEqual(s.driver_problems(text, 3, ["dive", "popup", None]), [])

    def test_a_run_that_names_the_forced_profile_passes(self):
        text = run(2, "dive", named="dive", releases=[10.0], impacts=[(27.0, 30)], egress=33.0)
        self.assertEqual(s.driver_problems(text, 1, ["dive"]), [])

    def test_a_wrong_profile_or_count_fails(self):
        self.assertTrue(any("2 runs" in p for p in s.driver_problems(run() + "\n" + run(), 3, [None] * 3)))
        text = run(2, "dive", named="popup")
        self.assertTrue(any("PROFILE name=popup" in p for p in s.driver_problems(text, 1, ["dive"])))
        self.assertTrue(any("asked popup" in p for p in s.driver_problems(run(2, "dive"), 1, ["popup"])))


def handback_run(trigger=True, mode="LVL", release=13.3, back=30.0, miss=359.5, designate="0x50000000"):
    lines = [
        SETUP.format(skill=1, profile="none"),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=no-strike-behaviour",
        "ai-strike: start t=0.000 plane=0 range_ft=24304 alt_ft=5000 speed_kt=450",
        f"ai-strike: DESIGNATE t=0.017 target={designate} T-80",
    ]
    releases = []
    if trigger:
        lines.append(f"ai-strike: RELEASE t={release} n=1 weapon=MK82.JT station=1 alt_ft=5000 speed_kt=449 range_ft=13371")
        lines.append(f"ai-strike: IMPACT t=29.942 n=1 miss_ft={miss} at=1.0,0.0,2.0")
        releases = [release]
    lines.append(f"ai-strike: HANDBACK t={back} designated=none selected=1 mode={mode}")
    lines.append(f"ai-strike: ACTIVITY t={back + 0.008} name=In formation")
    lines.append(
        f"ai-strike: summary t=40.0 ran_s=40.0 profile=none releases={len(releases)} impacts={len(releases)} kills=0 "
        f"egress=no lost=no activity=In formation target_hp=154/200 closest_ft=23 store_rounds_left=3"
    )
    if not releases:
        lines.append("ai-strike: no release")
    lines.append("ai-strike: done")
    return "\n".join(lines)


class HandbackTests(unittest.TestCase):
    def test_a_release_then_a_handoff_passes(self):
        self.assertEqual(s.handback_problems(handback_run() + "\n" + handback_run(trigger=False, back=6.0)), [])

    def test_a_release_after_the_handoff_fails(self):
        text = handback_run(release=31.0) + "\n" + handback_run(trigger=False)
        self.assertTrue(any("after the handback" in p for p in s.handback_problems(text)))

    def test_no_release_by_the_human_fails(self):
        text = handback_run(trigger=False) + "\n" + handback_run(trigger=False)
        self.assertTrue(any("released nothing" in p for p in s.handback_problems(text)))

    def test_the_wrong_mode_or_a_far_bomb_fails(self):
        text = handback_run(mode="CCIP") + "\n" + handback_run(trigger=False)
        self.assertTrue(any("handed back in CCIP" in p for p in s.handback_problems(text)))
        text = handback_run(miss=4000) + "\n" + handback_run(trigger=False)
        self.assertTrue(any("4000 ft from the target" in p for p in s.handback_problems(text)))

    def test_an_undesignated_target_fails(self):
        text = handback_run(designate="none") + "\n" + handback_run(trigger=False)
        self.assertTrue(any("designated nothing" in p for p in s.handback_problems(text)))


def takeover_run(mode="LVL", target="0x50000000", laser="hand-on", lasing_stops=70.0, miss=8.0, rounds=3,
                 pass_name="level", release=22.8):
    lines = [
        SETUP.format(skill=1, profile="none").replace("A4E load=MK82.JT", "F16C load=GBU10.JT"),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=not-ai-flown",
        "ai-strike: start t=0.000 plane=0 range_ft=36456 alt_ft=8000 speed_kt=450",
        "ai-strike: DESIGNATE t=0.017 target=0x50000000 T-80",
        "ai-strike: HANDBACK t=3.017 designated=0x50000000 selected=1 mode=LVL laser=hand-on",
        f"ai-strike: TAKEOVER t=3.017 target={target} station=1 mode={mode}",
        f"ai-strike: KEPT t=3.017 mode=LVL laser={laser} lasing=yes selected=1",
        f"ai-strike: PROFILE t=3.025 name={pass_name}",
    ]
    releases = []
    if release is not None:
        lines += [
            f"ai-strike: RELEASE t={release} n=1 weapon=GBU10.JT station=1 alt_ft=8021 speed_kt=471 range_ft=17246 "
            "pass=1 aim_ft=0 inside=yes",
            f"ai-strike: IMPACT t=61.775 n=1 miss_ft={miss} at=1,2,3",
            "ai-strike: KILL t=61.775 victim=0x50000000 category=0x0400 target=yes",
            "ai-strike: EGRESS t=61.775 range_ft=15759 alt_ft=8060",
        ]
        releases = [release]
    lines += [
        f"ai-strike: KEPT t={lasing_stops} mode=LVL laser={laser} lasing=no selected=1",
        f"ai-strike: TAKEBACK t=72.0 designated=0x50000000 selected=1 mode=LVL laser=hand-on rounds={rounds}",
        f"ai-strike: summary t=72.0 ran_s=72.0 profile={pass_name} releases={len(releases)} impacts={len(releases)} "
        f"kills={len(releases)} egress={'yes' if releases else 'no'} lost=no activity=Attacking target_hp=0/200 "
        "closest_ft=191 store_rounds_left=human",
    ]
    if not releases:
        lines.append("ai-strike: no release")
    lines.append("ai-strike: done")
    return "\n".join(lines)


class TakeoverTests(unittest.TestCase):
    def test_the_ai_finishing_the_humans_pass_passes(self):
        self.assertEqual(s.takeover_problems(takeover_run()), [])

    def test_another_target_mode_or_pass_fails(self):
        self.assertTrue(any("took over" in p for p in s.takeover_problems(takeover_run(target="0x50000001"))))
        self.assertTrue(any("took over" in p for p in s.takeover_problems(takeover_run(mode="CCIP"))))
        self.assertTrue(any("level run" in p for p in s.takeover_problems(takeover_run(pass_name="dive"))))

    def test_no_release_or_a_far_bomb_fails(self):
        self.assertTrue(any("released nothing" in p for p in s.takeover_problems(takeover_run(release=None, rounds=4))))
        self.assertTrue(any("ft from the target" in p for p in s.takeover_problems(takeover_run(miss=900))))

    def test_a_changed_cockpit_or_a_dropped_laser_fails(self):
        self.assertTrue(any("changed the human" in p for p in s.takeover_problems(takeover_run(laser="auto"))))
        self.assertTrue(any("laser stopped" in p for p in s.takeover_problems(takeover_run(lasing_stops=40.0))))

    def test_the_rounds_found_must_be_one_fewer(self):
        self.assertTrue(any("rounds" in p for p in s.takeover_problems(takeover_run(rounds=4))))


def order_run(result="refused", designated="0x50000000", extra=True):
    lines = [
        SETUP.format(skill=1, profile="none").replace("order=none", "order=engage-my-target"),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=no-strike-behaviour",
        "ai-strike: start t=0.000 plane=1 range_ft=24611 alt_ft=5000 speed_kt=450",
        "ai-strike: DESIGNATE t=0.017 target=0x50000000 T-80",
        f"ai-strike: ORDER t=3.017 kind=engage-my-target designated={designated}",
    ]
    if extra:
        lines.append(f'ai-strike: ORDER-REPLY t=3.025 result={result} message="Wing order unavailable: no valid hostile target"')
    lines += [
        "ai-strike: summary t=20.0 ran_s=20.0 profile=none releases=0 impacts=0 kills=0 egress=no lost=no "
        "activity=In formation target_hp=200/200 closest_ft=8902 store_rounds_left=4",
        "ai-strike: no release",
        "ai-strike: done",
    ]
    return "\n".join(lines)


class OrderTests(unittest.TestCase):
    def test_a_refusal_or_an_acceptance_passes(self):
        self.assertEqual(s.order_problems(order_run("refused")), [])
        self.assertEqual(s.order_problems(order_run("given")), [])

    def test_no_reply_or_nothing_designated_fails(self):
        self.assertTrue(any("no reply" in p for p in s.order_problems(order_run(extra=False))))
        self.assertTrue(any("nothing designated" in p for p in s.order_problems(order_run(designated="none"))))

    def test_a_strange_reply_fails(self):
        self.assertTrue(any("order reply weird" in p for p in s.order_problems(order_run("weird"))))


class StrikeTests(unittest.TestCase):
    def good(self, skill=2, profile="dive", miss=100.0, **kw):
        return run(skill, profile, named=profile, releases=[10.0], impacts=[(27.0, miss)], egress=33.0, **kw)

    def wanted(self, skill=2, profile="dive", **extra):
        return [dict(label="r", profile=profile, skill=skill, miss=s.MISS_FT_MAX[skill], **extra)]

    def test_a_good_strike_passes(self):
        self.assertEqual(s.strike_problems(self.good(), self.wanted()), [])

    def test_the_driver_as_it_is_today_fails_the_behaviour_scenarios(self):
        problems = s.strike_problems(run(), self.wanted())
        self.assertTrue(any("no RELEASE" in p for p in problems), problems)
        self.assertTrue(any("flew PROFILE none" in p for p in problems), problems)

    def test_a_miss_beyond_the_skills_limit_fails(self):
        too_far = s.MISS_FT_MAX[3] + 1
        ring = dict(aim=(50.0, "no"))
        self.assertTrue(any("from the target" in p for p in s.strike_problems(self.good(3, miss=too_far, **ring), self.wanted(3))))
        self.assertEqual(s.strike_problems(self.good(0, miss=s.MISS_FT_MAX[3] + 1, aim=(400.0, "no")), self.wanted(0)), [])

    def test_an_inside_draw_is_held_to_the_circle_plus_gate_plus_scatter(self):
        inside = s.MISS_FT_INSIDE[2]
        self.assertEqual(s.strike_problems(self.good(2, miss=inside - 1), self.wanted(2)), [])
        problems = s.strike_problems(self.good(2, miss=inside + 1), self.wanted(2))
        self.assertTrue(any("from the target" in p for p in problems), problems)
        ring = s.strike_problems(self.good(2, miss=inside + 1, aim=(200.0, "no")), self.wanted(2))
        self.assertEqual(ring, [])

    def test_the_aim_must_lie_where_the_draw_says(self):
        r = s.CIRCLE_FT[1]
        ok = s.strike_problems(self.good(1, aim=(r - 5, "yes")), self.wanted(1))
        self.assertEqual(ok, [])
        out = s.strike_problems(self.good(1, aim=(r + 50, "yes")), self.wanted(1))
        self.assertTrue(any("outside the" in p and "circle" in p for p in out), out)
        ring_low = s.strike_problems(self.good(1, aim=(r - 50, "no")), self.wanted(1))
        self.assertTrue(any("ring" in p for p in ring_low), ring_low)
        ring_far = s.strike_problems(self.good(1, aim=(2 * r + 50, "no")), self.wanted(1))
        self.assertTrue(any("ring" in p for p in ring_far), ring_far)
        missing = s.strike_problems(self.good(1, aim=None), self.wanted(1))
        self.assertTrue(any("without its aim" in p for p in missing), missing)

    def test_a_defence_is_required_where_asked(self):
        defended = self.good(extra=("ai-strike: ACTIVITY t=12.000 name=Defending",))
        self.assertEqual(s.strike_problems(defended, self.wanted(defends=True)), [])
        self.assertTrue(any("never defended" in p for p in s.strike_problems(self.good(), self.wanted(defends=True))))

    def test_the_limits_are_the_ring_edge_plus_scatter_and_grow_as_skill_falls(self):
        self.assertEqual([round(s.MISS_FT_MAX[k]) for k in (0, 1, 2, 3)], [986, 813, 641, 476])
        self.assertEqual([round(s.CIRCLE_FT[k] / 3.28084) for k in (3, 2, 1, 0)], [10, 35, 60, 85])
        self.assertEqual([round(s.CIRCLE_FT[k]) for k in (3, 2, 1, 0)], [33, 115, 197, 279])
        self.assertTrue(s.MISS_FT_MAX[0] > s.MISS_FT_MAX[1] > s.MISS_FT_MAX[2] > s.MISS_FT_MAX[3])

    def test_a_lost_striker_or_no_egress_fails(self):
        lost = run(2, "dive", named="dive", releases=[10.0], impacts=[(27.0, 50)], egress=33.0, lost=40.0)
        self.assertTrue(any("was lost" in p for p in s.strike_problems(lost, self.wanted())))
        self.assertEqual(s.strike_problems(lost, self.wanted(may_lose=True)), [])
        no_egress = run(2, "dive", named="dive", releases=[10.0], impacts=[(27.0, 50)])
        self.assertTrue(any("no EGRESS" in p for p in s.strike_problems(no_egress, self.wanted())))

    def test_a_hurt_target_is_required_where_asked(self):
        wanted = [dict(label="r", profile="dive", skill=2, miss=None, hit=True)]
        hurt = self.good(hp="120/200")
        self.assertEqual(s.strike_problems(hurt, wanted), [])
        self.assertTrue(any("unhurt" in p for p in s.strike_problems(self.good(), wanted)))
        self.assertEqual(s.strike_problems(self.good(hp="gone/200"), wanted), [])

    def test_threat_decisions_are_wanted_in_order_and_a_known_ring_is_not_fired_into(self):
        held = [dict(label="held", decisions=["hold_off", "home"], no_release=True)]
        quiet = dict(breaks=0, released=False, killed=False)
        self.assertEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off", "threat_home"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_home", "threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(
            s.heli_defense_problems(heli_run(breaks=0, decisions=["threat_hold_off", "threat_home"]), held), [])
        sead = [dict(label="sead", decisions=["hold_off", "resume"], sead=True, release=True)]
        resumed = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"], sead=True)
        self.assertEqual(s.heli_defense_problems(resumed, sead), [])
        no_sead = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"])
        self.assertNotEqual(s.heli_defense_problems(no_sead, sead), [])

    def test_the_run_count_must_match(self):
        self.assertTrue(any("1 runs" in p for p in s.strike_problems(self.good(), self.wanted() * 2)))


# The aim lies inside every skill's circle (Ace 33 ft, John's tighter precision).
def gunship_run(skill=2, bursts=8, aim=20.0, inside="yes", slant=10400.0, bank=-22.0, killed=True, lost=None):
    """One AC-130 run of the driver: an orbit with `bursts` bursts."""
    out = [
        "ai-strike: setup aircraft=Ac130 load=C_25.JT station=0 count=3000 rounds=3000 target=0x50000001 "
        f"name=T-90 target_at=999570,7,789571 hp=200 skill={skill} profile=none handback=none order=none mode=none",
        "ai-strike: PROFILE t=0.000 name=none requested=none note=chosen",
        "ai-strike: start t=0.000 plane=0 range_ft=18228 alt_ft=6000 speed_kt=220",
        "ai-strike: PROFILE t=0.008 name=orbit",
    ]
    for n in range(1, bursts + 1):
        out.append(
            f"ai-strike: BURST t={20 + 2 * n:.3f} n={n} aim_ft={aim:.0f} inside={inside} radius_ft=8520 "
            f"height_ft=5965 slant_ft={slant:.0f} bank_deg={bank:.1f} speed_kt=191 rounds={32 * n}"
        )
    if killed:
        out.append("ai-strike: KILL t=120.000 victim=0x50000001 category=0x0400 target=yes")
    if lost is not None:
        out.append(f"ai-strike: LOST t={lost:.3f} reason=destroyed")
    out.append(
        f"ai-strike: summary t=240.000 ran_s=240.0 profile=orbit releases=0 impacts=0 kills={1 if killed else 0} "
        f"egress=no lost={'yes' if lost is not None else 'no'} activity=Attacking target_hp={'0' if killed else '120'}/200 "
        f"closest_ft=8834 store_rounds_left=900 gun_rounds={32 * bursts} bursts={bursts}"
    )
    out.append("ai-strike: no release")
    out.append("ai-strike: done")
    return "\n".join(out)


class GunshipTests(unittest.TestCase):
    want = [dict(label="ac130", skill=2)]

    def test_an_honest_orbit_passes(self):
        self.assertEqual(s.gunship_problems(gunship_run(), self.want), [])

    def test_an_aim_outside_the_circle_or_the_ring_fails(self):
        r = s.CIRCLE_FT[2]
        self.assertNotEqual(s.gunship_problems(gunship_run(aim=r + 50), self.want), [])
        self.assertEqual(s.gunship_problems(gunship_run(aim=r + 50, inside="no"), self.want), [])
        self.assertNotEqual(s.gunship_problems(gunship_run(aim=2 * r + 50, inside="no"), self.want), [])

    def test_out_of_reach_or_overbanked_once_settled_fails(self):
        self.assertNotEqual(s.gunship_problems(gunship_run(slant=14000.0), self.want), [])
        self.assertNotEqual(s.gunship_problems(gunship_run(bank=-35.0), self.want), [])
        # The first bursts settle the orbit: only the later ones are held to it.
        self.assertEqual(s.gunship_problems(gunship_run(bursts=s.ORBIT_SETTLE_BURSTS, slant=14000.0), self.want), [])

    def test_no_kill_a_loss_or_no_burst_fails(self):
        self.assertNotEqual(s.gunship_problems(gunship_run(killed=False), self.want), [])
        self.assertNotEqual(s.gunship_problems(gunship_run(lost=100.0), self.want), [])
        self.assertNotEqual(s.gunship_problems(gunship_run(bursts=0), self.want), [])

    def test_threat_decisions_are_wanted_in_order_and_a_known_ring_is_not_fired_into(self):
        held = [dict(label="held", decisions=["hold_off", "home"], no_release=True)]
        quiet = dict(breaks=0, released=False, killed=False)
        self.assertEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off", "threat_home"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_home", "threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(
            s.heli_defense_problems(heli_run(breaks=0, decisions=["threat_hold_off", "threat_home"]), held), [])
        sead = [dict(label="sead", decisions=["hold_off", "resume"], sead=True, release=True)]
        resumed = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"], sead=True)
        self.assertEqual(s.heli_defense_problems(resumed, sead), [])
        no_sead = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"])
        self.assertNotEqual(s.heli_defense_problems(no_sead, sead), [])

    def test_the_run_count_must_match(self):
        self.assertNotEqual(s.gunship_problems(gunship_run(), self.want * 2), [])


def heli_run(profile="popup", breaks=6, offset=90, kt=130, pitch=-10, blocked=0, released=True, killed=True,
             lost=None, min_agl=1400, aircraft="Mi24", quick=False, start_alt=1500, decisions=(), sead=False,
             egress=False, shots=(), ahead=False, reach_margin=1200):
    """One helicopter run of the driver: `breaks` BREAK lines (one a second), the THREAT
    `decisions` (slice H4), a pop-up that may release and kill, and an EGRESS (slice HF1).
    `shots` are (time, known) pairs for the SHOT lines (a site's launch, `known` yes, no or none);
    the decisions come at t=30, 31, ... so a shot before a decision has a time under it. `ahead`
    adds a move-ahead plan's AHEAD line; `reach_margin` is the summary's `reach_margin_ft`."""
    out = [
        f"ai-strike: setup aircraft={aircraft} load=AT2.JT station=1 count=4 rounds=4 target=0x50000001 name=T-90 "
        "target_at=999570,7,789571 hp=200 skill=2 profile=none handback=none order=none mode=none",
        "ai-strike: PROFILE t=0.000 name=none requested=none note=chosen",
        *(["ai-strike: ROLE t=0.000 plane=1 role=strike"] if quick else []),
        f"ai-strike: start t=0.000 plane=0 range_ft=24304 alt_ft={start_alt} speed_kt=120",
        f"ai-strike: PROFILE t=0.008 name={profile}",
    ]
    for n in range(breaks):
        out.append(
            f"ai-strike: BREAK t={20 + n}.000 heading=350 track=345 bearing=22 offset={offset} masked_s=0 "
            f"blocked_s={blocked} pitch={pitch} agl_ft=1400 speed_kt={kt} bank=-30"
        )
    for at, known in shots:
        out.append(f"ai-strike: SHOT t={at:.3f} site=0x5000000f range_ft=11000 known={known}")
    if ahead:
        out.append("ai-strike: AHEAD t=1.008 range_ft=14000 fire_in_s=60.0 point=988868,778720 aim_moves_ft=3000 "
                   "from_ft=12000")
    for n, decision in enumerate(decisions):
        out.append(f"ai-strike: THREAT t={30 + n}.000 decision={decision} target=0x50000001 ring=0x5000000b "
                   'words="threat on target, holding off"')
    if sead:
        out.append("ai-strike: SEAD t=60.000 killed=3")
    if released:
        out.append("ai-strike: RELEASE t=70.000 n=1 weapon=AT2.JT station=1 alt_ft=1600 speed_kt=123 range_ft=9200"
                   " pass=1 aim_ft=0 inside=yes")
        out.append("ai-strike: IMPACT t=77.000 n=1 miss_ft=6.0 at=1.0,2.0,3.0")
    if killed:
        out.append("ai-strike: KILL t=77.000 victim=0x50000001 category=0x0400 target=yes")
    if egress:
        out.append("ai-strike: EGRESS t=90.000 range_ft=11000 alt_ft=250")
    if lost is not None:
        out.append(f"ai-strike: LOST t={lost:.3f} reason=destroyed crashed=yes")
    n = 1 if released else 0
    out.append(
        f"ai-strike: summary t=100.000 ran_s=100.0 profile={profile} releases={n} impacts={n} "
        f"kills={1 if killed else 0} egress={'yes' if egress else 'no'} lost={'yes' if lost is not None else 'no'} activity=Defending "
        f"target_hp={'0' if killed else '200'}/200 closest_ft=4773 store_rounds_left=3 min_agl_ft={min_agl} "
        f"break_s={float(breaks)} reach_margin_ft={reach_margin}"
    )
    if not released:
        out.append("ai-strike: no release")
    out.append("ai-strike: done")
    return "\n".join(out)


class HelicopterTests(unittest.TestCase):
    want = [dict(label="mi24", profile="popup", breaks=5, release=True, kill=True)]

    def test_an_honest_defence_passes(self):
        self.assertEqual(s.heli_defense_problems(heli_run(), self.want), [])

    def test_the_break_must_be_a_break(self):
        for bad in (heli_run(breaks=2), heli_run(offset=45), heli_run(kt=60), heli_run(pitch=-25),
                    heli_run(blocked=2)):
            self.assertNotEqual(s.heli_defense_problems(bad, self.want), [])
        self.assertEqual(s.heli_defense_problems(heli_run(offset=60), self.want), [])
        self.assertEqual(s.heli_defense_problems(heli_run(offset=120), self.want), [])

    def test_the_pass_the_ai_chose_is_named(self):
        self.assertNotEqual(s.heli_defense_problems(heli_run(profile="standoff"), self.want), [])

    def test_the_helicopter_lives_and_stays_off_the_ground(self):
        self.assertNotEqual(s.heli_defense_problems(heli_run(lost=80.0), self.want), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(min_agl=40), self.want), [])

    def test_a_release_and_a_kill_are_asked_for_only_when_wanted(self):
        self.assertNotEqual(s.heli_defense_problems(heli_run(released=False, killed=False), self.want), [])
        quiet = [dict(label="stand-off", profile="popup", breaks=5, release=False)]
        self.assertEqual(s.heli_defense_problems(heli_run(released=False, killed=False), quiet), [])

    def test_threat_decisions_are_wanted_in_order_and_a_known_ring_is_not_fired_into(self):
        held = [dict(label="held", decisions=["hold_off", "home"], no_release=True)]
        quiet = dict(breaks=0, released=False, killed=False)
        self.assertEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off", "threat_home"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_home", "threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(
            s.heli_defense_problems(heli_run(breaks=0, decisions=["threat_hold_off", "threat_home"]), held), [])
        sead = [dict(label="sead", decisions=["hold_off", "resume"], sead=True, release=True)]
        resumed = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"], sead=True)
        self.assertEqual(s.heli_defense_problems(resumed, sead), [])
        no_sead = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"])
        self.assertNotEqual(s.heli_defense_problems(no_sead, sead), [])

    def test_a_leave_is_the_answer_to_a_shot_from_a_site_it_did_not_know(self):
        """John, 2026-10-29: `threat_leave` is the ambush by an unknown shooter. The ambush run needs a
        SHOT with known=no before the leave; a leave after only shots from sites it knew is a fault in
        every run."""
        ambush = [dict(label="ambushed", decisions=["leave", "resume"], breaks=2, release=True, kill=True,
                       unknown_shooter=True)]
        plain = [dict(label="ambushed", decisions=["leave", "resume"], breaks=2, release=True, kill=True)]
        leave_resume = ["threat_leave", "threat_resume"]
        unseen = heli_run(decisions=leave_resume, shots=[(29.0, "no")])
        self.assertEqual(s.heli_defense_problems(unseen, ambush), [])
        self.assertEqual(s.heli_defense_problems(unseen, plain), [])
        # Shots from a site it knew, or with no picture at all, are no ambush.
        for known in ("yes", "none"):
            problems = s.heli_defense_problems(heli_run(decisions=leave_resume, shots=[(29.0, known)]), ambush)
            self.assertTrue(any("threat_leave" in p and "knew" in p for p in problems), (known, problems))
            self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=leave_resume, shots=[(29.0, known)]), plain), [])
        # One known shot among others is not enough to cancel the unknown one before the leave.
        mixed = heli_run(decisions=leave_resume, shots=[(28.0, "yes"), (29.0, "no")])
        self.assertEqual(s.heli_defense_problems(mixed, ambush), [])
        # The unknown shot has to come before the leave, not after it.
        late = heli_run(decisions=leave_resume, shots=[(40.0, "no")])
        self.assertNotEqual(s.heli_defense_problems(late, ambush), [])
        # No shot at all: a gun's ambush has no SHOT line. Only the run that asks for a missile ambush
        # needs one.
        gun = heli_run(decisions=leave_resume)
        self.assertEqual(s.heli_defense_problems(gun, plain), [])
        self.assertTrue(any("no SHOT" in p for p in s.heli_defense_problems(gun, ambush)))
        # A second leave counts from the first: it needs a shot of its own from a site it did not know.
        twice = ["threat_leave", "threat_resume", "threat_leave", "threat_resume"]
        both = heli_run(decisions=twice, shots=[(29.0, "no"), (31.0, "no")])
        self.assertEqual(s.heli_defense_problems(both, ambush), [])
        second_known = heli_run(decisions=twice, shots=[(29.0, "no"), (31.0, "yes")])
        self.assertTrue(any("t=32.0" in p and "knew" in p for p in s.heli_defense_problems(second_known, ambush)))
        self.assertNotEqual(s.heli_defense_problems(second_known, plain), [])
        # Runs with no leave are untouched by shots from sites it knew.
        held = [dict(label="held", decisions=["hold_off", "home"], no_release=True)]
        shot_at = heli_run(breaks=0, released=False, killed=False, decisions=["threat_hold_off", "threat_home"],
                           shots=[(40.0, "yes")])
        self.assertEqual(s.heli_defense_problems(shot_at, held), [])

    def test_the_run_count_must_match(self):
        self.assertNotEqual(s.heli_defense_problems(heli_run(), self.want * 2), [])

    def test_a_quick_mission_wing_of_helicopters_is_given_the_strike_role_and_starts_low(self):
        text = heli_run(quick=True) + "\n" + heli_run(quick=True, aircraft="Ah64")
        self.assertEqual(s.quick_heli_problems(text), [])
        self.assertNotEqual(s.quick_heli_problems(heli_run(quick=True) + "\n" + heli_run(quick=False)), [])
        self.assertNotEqual(s.quick_heli_problems(heli_run(quick=True, start_alt=5000) * 1 + "\n" + heli_run(quick=True)), [])
        self.assertNotEqual(
            s.quick_heli_problems(heli_run(quick=True, released=False, killed=False) + "\n" + heli_run(quick=True)), []
        )

    def test_the_scenarios_use_the_checks_and_the_defended_runs_are_unforced(self):
        scenarios = {sc.name: sc for sc in s.scenarios()}
        defended = scenarios["ai-strike-heli-defended"]
        argsets = [defended.args] + [step.args for step in defended.then]
        self.assertEqual(len(argsets), 9)
        self.assertTrue(all("mi24" in a for a in argsets))
        self.assertNotIn("--profile", [x for a in argsets for x in a], "the AI chooses its pass")
        heli = scenarios["ai-strike-heli"]
        self.assertNotIn("--profile", [x for a in [heli.args] + [st.args for st in heli.then] for x in a])
        held = ["threat_hold_off", "threat_home"]
        # Heavy AAA on the moving column: the move-ahead ambush (2026-10-29) fires from outside every
        # known gun's reach and hurts the tank, with no hold-off.
        # The ambush runs (2026-10-29): a shot from a site it did not know, the leave, the resume.
        ambushed = dict(decisions=["threat_leave", "threat_resume"], shots=[(29.0, "no")])
        # The SA-14 team on the target (seed 4, since the hidden teams and the move-ahead of
        # 2026-10-29): its launch is an ambush, a move-ahead resume, the second team's ambush, home.
        # The decisions come at t=30 to 35, the leaves at 31 and 34, each after a shot of its own.
        seed4 = dict(decisions=["threat_hold_off", "threat_leave", "threat_resume", "threat_hold_off",
                                "threat_leave", "threat_home"], shots=[(29.0, "no"), (33.0, "no")], ahead=True)
        honest = [heli_run(profile="standoff", breaks=0, ahead=True),
                  heli_run(profile="standoff", breaks=0),
                  heli_run(breaks=0, released=False, killed=False, decisions=held),
                  heli_run(profile="standoff", breaks=6, released=False, killed=False, **seed4),
                  heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"], sead=True),
                  heli_run(breaks=8, **ambushed),
                  heli_run(breaks=8, **ambushed),
                  heli_run(profile="standoff", breaks=3, released=False, killed=False, decisions=held),
                  heli_run(breaks=8, released=False, killed=False, decisions=["threat_leave"],
                           shots=[(20.0, "no"), (30.0, "yes")])]
        self.assertEqual(defended.check("\n".join(honest)), [])
        # The heavy-AAA run must plan a move-ahead, call no hold-off, and stay out of every known reach.
        for wrong in (dict(ahead=False), dict(decisions=["threat_hold_off"]), dict(reach_margin=-50),
                      dict(released=False)):
            heavy = list(honest)
            heavy[0] = heli_run(**{**dict(profile="standoff", breaks=0, ahead=True), **wrong})
            self.assertNotEqual(defended.check("\n".join(heavy)), [], wrong)
        # Seed 4 (2026-10-29): each leave answers a launch by a team it did not know; the second leave
        # after only a known team's shot (the first team again, past its unmasking) is a fault, and so
        # is the old hold-off and home with no ambush or move-ahead.
        for wrong in (dict(shots=[(29.0, "no"), (33.0, "yes")]), dict(decisions=held, shots=[]),
                      dict(ahead=False)):
            seed4_wrong = list(honest)
            seed4_wrong[3] = heli_run(profile="standoff", breaks=6, released=False, killed=False,
                                      **{**seed4, **wrong})
            self.assertNotEqual(defended.check("\n".join(seed4_wrong)), [], wrong)
        # Slice HF1: the ambushed runs must leave and resume after it.
        unambushed = list(honest)
        unambushed[5] = heli_run(breaks=8, decisions=["threat_hold_off", "threat_resume"])
        self.assertNotEqual(defended.check("\n".join(unambushed)), [])
        # 2026-10-29: and the leave must answer a shot from a site it did not know.
        for known in ("yes", "none"):
            known_site = list(honest)
            known_site[6] = heli_run(breaks=8, decisions=["threat_leave", "threat_resume"], shots=[(29.0, known)])
            self.assertNotEqual(defended.check("\n".join(known_site)), [], known)
        gunned = list(honest)
        gunned[5] = heli_run(breaks=8, decisions=["threat_leave", "threat_resume"])
        self.assertNotEqual(defended.check("\n".join(gunned)), [])
        # Both ambush runs lay the site out the same way: placed and destroyed after its shot. The game
        # hides the shoulder-fired team itself (2026-10-29), so no listing stand-in.
        for args in argsets[5:7]:
            for flag in ("--place-site", "--sead-site", "--sead-after-shot", "--follow-threat"):
                self.assertIn(flag, args)
            self.assertNotIn("--site-sight", args, "the game hides the team, not the driver")
            self.assertNotIn("--sead-at", args, "the SEAD stand-in goes by the shot, not the clock")
        quick = scenarios["qmc-strike-heli"]
        self.assertEqual(len([quick.args] + [st.args for st in quick.then]), 2)


class ScenarioTests(unittest.TestCase):
    def setUp(self):
        self.scenarios = {sc.name: sc for sc in s.scenarios()}

    def test_every_scenario_is_live(self):
        for name, sc in self.scenarios.items():
            self.assertEqual(sc.known_failure, "", name)

    def test_the_plan_s_scenarios_exist(self):
        want = {"ai-strike-bombs", "ai-strike-rockets", "ai-strike-maverick", "ai-strike-jdam", "ai-sead-harm",
                "ai-strike-defended", "ai-strike-heli", "ai-strike-ac130", "ai-strike-laser-self",
                "ai-strike-laser-buddy"}
        self.assertTrue(want <= set(self.scenarios))

    def test_every_scenario_flies_the_ai(self):
        """Each run has the AI fly the striker, except the lasing runs where a player drops on an
        AI buddy's spot (`--buddy-ai`)."""
        for name, sc in self.scenarios.items():
            for args in [sc.args] + [step.args for step in sc.then]:
                self.assertEqual(args[0], "--a2g-run", name)
                if "--pilot" in args:
                    self.assertEqual(args[args.index("--pilot") + 1], "ai", name)
                else:
                    self.assertIn("--buddy-ai", args, name)

    def test_the_bomb_scenario_forces_each_pass_once_and_covers_every_skill(self):
        sc = self.scenarios["ai-strike-bombs"]
        argsets = [sc.args] + [step.args for step in sc.then]
        profiles = [a[a.index("--profile") + 1] for a in argsets]
        skills = {int(a[a.index("--skill") + 1]) for a in argsets}
        self.assertEqual(sorted(set(profiles), key=profiles.index)[:5], ["straight", "popup", "fasthigh", "dive", "level"])
        self.assertEqual(skills, {0, 1, 2, 3})
        self.assertEqual(len(argsets), len(s.BOMB_RUNS))

    def test_the_names_are_in_the_selection_family(self):
        patterns = bs.FAMILIES["ai-strike"][1]
        import fnmatch
        for name in self.scenarios:
            self.assertTrue(any(fnmatch.fnmatch(name, p) for p in patterns), name)

    def test_the_checks_accept_an_honest_run_of_the_driver_scenarios(self):
        self.assertEqual(self.scenarios["ai-strike-driver"].check(
            "\n".join([run(2, "dive"), run(0, "popup"), run(3, "none")])), [])
        self.assertEqual(self.scenarios["ai-strike-handback"].check(
            handback_run() + "\n" + handback_run(trigger=False, back=6.0)), [])
        self.assertEqual(self.scenarios["ai-strike-order"].check(order_run()), [])

    def test_the_family_scenarios_fly_the_helicopters_and_the_gunship(self):
        heli = self.scenarios["ai-strike-heli"]
        aircraft = [a[a.index("--aircraft") + 1] for a in [heli.args] + [st.args for st in heli.then]]
        self.assertEqual(sorted(set(aircraft)), ["ah64", "mi24"])
        gun = self.scenarios["ai-strike-ac130"]
        self.assertTrue(all(a[a.index("--aircraft") + 1] == "ac130" for a in [gun.args] + [st.args for st in gun.then]))
        self.assertEqual(gun.check("\n".join(gunship_run(k) for k in (0, 2, 3))), [])

    def test_the_pending_checks_fail_on_the_driver_as_it_is_today(self):
        today = "\n".join(run(skill, profile) for skill, profile in s.BOMB_RUNS)
        self.assertNotEqual(self.scenarios["ai-strike-bombs"].check(today), [])


def ground_order_run(kind="strike", result="given", message="Engage my target: 1 applied, 0 rejected, 0 without motion",
                     state="strike_order=0x50000001 class_order=0 lase_order=none", released=True, designated="0x50000001",
                     extra=()):
    lines = [
        SETUP.format(skill=1, profile="none").replace("order=none", "order=engage-my-target"),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=ordered",
        "ai-strike: start t=0.000 plane=1 range_ft=24611 alt_ft=5000 speed_kt=450",
        f"ai-strike: ORDER t=3.017 kind=engage-my-target designated={designated}",
        f'ai-strike: ORDER-REPLY t=3.025 result={result} message="{message}"',
        f"ai-strike: ORDER-STATE t=3.025 {state}",
        *extra,
    ]
    releases = 1 if released else 0
    if released:
        lines.append("ai-strike: RELEASE t=30.000 n=1 weapon=MK82.JT station=1 alt_ft=4900 speed_kt=450 range_ft=9000"
                     " pass=1 aim_ft=100 inside=yes")
        lines.append("ai-strike: IMPACT t=40.000 n=1 miss_ft=300 at=1.0,2.0,3.0")
    lines.append(f"ai-strike: summary t=60.0 ran_s=60.0 profile=none releases={releases} impacts={releases} kills=0 egress=no "
                 "lost=no activity=In formation target_hp=200/200 closest_ft=8902 store_rounds_left=4")
    if not released:
        lines.append("ai-strike: no release")
    lines.append("ai-strike: done")
    return "\n".join(lines)


LASE_STATE = "strike_order=0x50000001 class_order=0 lase_order=0x50000001"


def lase_flown(phase_at=3.025, lased=True, lost=False, ranges=(21000, 19700, 20500, 21500)):
    """The lines of a wingman flying Lase my target (slice ST-L's duty, wired at ST-INT)."""
    lines = []
    if phase_at is not None:
        lines.append(f"ai-strike: PHASE t={phase_at:.3f} name=Lase")
    lines.append("ai-strike: LASER t=0.025 plane=1 state=Off mask=none range_ft=0")
    if lased:
        lines.append("ai-strike: LASER t=3.033 plane=1 state=Lasing mask=none range_ft=22920")
    lines += [f"ai-strike: HOLD t={5 + 5 * i:.3f} range_ft={r} above_ft=6500 speed_kt=200 bank_deg=12"
              for i, r in enumerate(ranges)]
    if lost:
        lines.append("ai-strike: LASER t=40.000 plane=1 state=Masked mask=Gimbal range_ft=21000")
    return tuple(lines)


def lase_run(**kw):
    released = kw.pop("released", False)
    return ground_order_run(state=kw.pop("state", LASE_STATE), released=released, extra=lase_flown(**kw))


class GroundOrderTests(unittest.TestCase):
    def check(self, text, kind):
        return s.ground_order_problems(text, [dict(label="run", kind=kind)])

    def test_a_taken_strike_order_holds_the_object_and_releases(self):
        self.assertEqual(self.check(ground_order_run(), "strike"), [])
        self.assertTrue(any("never released" in p for p in self.check(ground_order_run(released=False), "strike")))
        none_held = ground_order_run(state="strike_order=none class_order=0 lase_order=none")
        self.assertTrue(any("want one strike order" in p for p in self.check(none_held, "strike")))

    def test_a_class_order_holds_its_targets(self):
        held = ground_order_run(state="strike_order=none class_order=4 lase_order=none")
        self.assertEqual(self.check(held, "class"), [])
        self.assertTrue(any("class order" in p for p in self.check(ground_order_run(), "class")))

    def test_a_refusal_holds_nothing_and_releases_nothing(self):
        refused = ground_order_run(result="given", message="Wing order unavailable: no usable surface store",
                                   state="strike_order=none class_order=0 lase_order=none", released=False)
        self.assertEqual(self.check(refused, "refused"), [])
        self.assertTrue(any("refusal" in p for p in self.check(ground_order_run(), "refused")))

    def test_the_lase_order_is_held_with_at_most_its_own_target_as_a_strike(self):
        held = ground_order_run(state="strike_order=none class_order=0 lase_order=0x50000001", released=False)
        self.assertEqual(self.check(held, "lase"), [])
        self.assertEqual(self.check(ground_order_run(state=LASE_STATE, released=False), "lase"), [])
        other = ground_order_run(state="strike_order=0x50000002 class_order=0 lase_order=0x50000001", released=False)
        self.assertTrue(any("lase order" in p for p in self.check(other, "lase")))
        self.assertTrue(any("lase order" in p for p in self.check(ground_order_run(), "lase")))

    def flown(self, text):
        return s.ground_order_problems(text, [dict(label="run", kind="lase", flies=True)])

    def test_the_lase_order_is_flown_as_the_lase_hold(self):
        self.assertEqual(self.flown(lase_run()), [])
        self.assertTrue(any("never entered" in p for p in self.flown(lase_run(phase_at=None))))
        self.assertTrue(any("began" in p for p in self.flown(lase_run(phase_at=8.0))))
        self.assertTrue(any("never came up" in p for p in self.flown(lase_run(lased=False))))
        self.assertTrue(any("spot was lost" in p for p in self.flown(lase_run(lost=True))))
        self.assertTrue(any("ft out" in p for p in self.flown(lase_run(ranges=(21000, 70000)))))
        self.assertTrue(any("HOLD lines" in p for p in self.flown(lase_run(ranges=(21000,)))))
        self.assertTrue(any("released" in p for p in self.flown(lase_run(released=True))))

    def test_attack_on_contact_holds_no_order(self):
        free = ground_order_run(state="strike_order=none class_order=0 lase_order=none", released=False)
        self.assertEqual(self.check(free, "free"), [])
        self.assertNotEqual(self.check(ground_order_run(), "free"), [])

    def test_the_scenarios_use_these_checks(self):
        scenarios = {sc.name: sc for sc in s.scenarios()}
        self.assertIn("ai-wing-ground-order", scenarios)
        self.assertIn("ai-wing-lase", scenarios)
        self.assertEqual(len(s.ORDER_RUNS), 1 + len(scenarios["ai-wing-ground-order"].then))
        self.assertEqual(scenarios["ai-wing-lase"].check(lase_run()), [])
        self.assertNotEqual(scenarios["ai-wing-lase"].check(lase_run(phase_at=None)), [])


def quick_wing_run(role="strike", released=True):
    lines = [
        SETUP.format(skill=1, profile="none"),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=quick-wing",
        f"ai-strike: ROLE t=0.000 plane=1 role={role}",
        "ai-strike: start t=0.000 plane=1 range_ft=240000 alt_ft=5000 speed_kt=450",
    ]
    if released:
        lines.append("ai-strike: RELEASE t=200.000 n=1 weapon=MK82.JT station=1 alt_ft=4900 speed_kt=450 range_ft=9000"
                     " pass=1 aim_ft=100 inside=yes")
        lines.append("ai-strike: IMPACT t=210.000 n=1 miss_ft=300 at=1.0,2.0,3.0")
    lines.append(f"ai-strike: summary t=300.0 ran_s=300.0 profile=none releases={int(released)} impacts={int(released)} "
                 "kills=0 egress=no lost=no activity=In formation target_hp=200/200 closest_ft=8902 store_rounds_left=4")
    if not released:
        lines.append("ai-strike: no release")
    lines.append("ai-strike: done")
    return "\n".join(lines)


class QuickWingTests(unittest.TestCase):
    def test_bombers_strike_and_fighters_do_not(self):
        text = quick_wing_run("strike", True) + "\n" + quick_wing_run("cap", False)
        self.assertEqual(s.quick_wing_problems(text), [])

    def test_a_bomber_wing_that_stays_cap_or_a_fighter_wing_that_strikes_fails(self):
        wrong_role = quick_wing_run("cap", False) + "\n" + quick_wing_run("cap", False)
        self.assertTrue(any("role cap, want strike" in p for p in s.quick_wing_problems(wrong_role)))
        bombs = quick_wing_run("strike", True) + "\n" + quick_wing_run("strike", True)
        self.assertTrue(any("want cap" in p for p in s.quick_wing_problems(bombs)))
        idle = quick_wing_run("strike", False) + "\n" + quick_wing_run("cap", False)
        self.assertTrue(any("never released" in p for p in s.quick_wing_problems(idle)))
def laser_run(plane="0", spot="0", phases=True, lased=True, blind="0.0", fall="49.5", miss=8.4, kill=True,
              holds=((25.0, "15258", "-72"), (40.0, "13360", "46")), exit_at=70.783, lase_at=21.275,
              weapon="GBU10.JT", guided="Guiding", cone=False, let_go=True):
    """One driver run of an AI laser strike: the pass, the release, the hold, the landing, the exit."""
    lines = [
        SETUP.format(skill=2, profile="none").replace("A4E", "F16C").replace("MK82.JT", "GBU10.JT"),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=chosen",
        f"ai-strike: start t=0.000 plane={plane} range_ft=36456 alt_ft=15000 speed_kt=480",
        "ai-strike: PROFILE t=0.008 name=level",
        "ai-strike: PHASE t=0.008 name=Attack",
        f"ai-strike: LASER t=0.025 plane={plane} state=Lasing mask=none range_ft=39405",
    ]
    if phases:
        lines.append(f"ai-strike: PHASE t={lase_at} name=Lase")
    lines.append(f"ai-strike: RELEASE t=21.275 n=1 weapon={weapon} station=2 alt_ft=14733 speed_kt=508 range_ft=18535 "
                 f"pass=1 aim_ft=0 inside=yes guided={guided} spot_owner={spot}")
    if not cone:
        lines += [f"ai-strike: HOLD t={t:.3f} range_ft={r} above_ft=14721 speed_kt=400 bank_deg={b}" for t, r, b in holds]
    if not lased:
        lines.append(f"ai-strike: LASER t=40.000 plane={plane} state=Masked mask=Gimbal range_ft=12000")
    lines.append(f"ai-strike: PHASE t={exit_at} name=Exit")
    lines.append(f"ai-strike: IMPACT t=70.783 n=1 miss_ft={miss} at=1.0,2.0,3.0")
    if kill:
        lines.append("ai-strike: KILL t=70.783 victim=0x50000001 category=0x0400 target=yes")
    lines.append("ai-strike: EGRESS t=70.783 range_ft=20766 alt_ft=14785")
    if let_go:
        lines.append(f"ai-strike: LASER t=70.800 plane={plane} state=Ready mask=none range_ft=25504")
    lines.append(
        f"ai-strike: summary t=90.0 ran_s=90.0 profile=level releases=1 impacts=1 kills={1 if kill else 0} egress=yes "
        f"lost=no activity=Attacking target_hp={'0' if kill else '200'}/200 closest_ft=11722 store_rounds_left=1 "
        f"fall_s={fall} blind_s={blind}"
    )
    lines.append("ai-strike: done")
    return "\n".join(lines)


class LaserSelfTests(unittest.TestCase):
    def check(self, pod=None, cone=None):
        return s.laser_self_problems((pod or laser_run()) + "\n" + (cone or laser_run(cone=True, exit_at=62.0)))

    def test_an_honest_pair_of_runs_passes(self):
        self.assertEqual(self.check(), [])

    def test_threat_decisions_are_wanted_in_order_and_a_known_ring_is_not_fired_into(self):
        held = [dict(label="held", decisions=["hold_off", "home"], no_release=True)]
        quiet = dict(breaks=0, released=False, killed=False)
        self.assertEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off", "threat_home"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_home", "threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(s.heli_defense_problems(heli_run(decisions=["threat_hold_off"], **quiet), held), [])
        self.assertNotEqual(
            s.heli_defense_problems(heli_run(breaks=0, decisions=["threat_hold_off", "threat_home"]), held), [])
        sead = [dict(label="sead", decisions=["hold_off", "resume"], sead=True, release=True)]
        resumed = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"], sead=True)
        self.assertEqual(s.heli_defense_problems(resumed, sead), [])
        no_sead = heli_run(breaks=0, decisions=["threat_hold_off", "threat_resume"])
        self.assertNotEqual(s.heli_defense_problems(no_sead, sead), [])

    def test_the_run_count_must_match(self):
        self.assertTrue(any("1 runs" in p for p in s.laser_self_problems(laser_run())))

    def test_the_hold_must_begin_at_the_release_and_last_to_the_landing(self):
        self.assertTrue(any("never held" in p for p in self.check(laser_run(phases=False))))
        self.assertTrue(any("hold began" in p for p in self.check(laser_run(lase_at=30.0))))
        self.assertTrue(any("before the bomb landed" in p for p in self.check(laser_run(exit_at=50.0))))

    def test_a_cone_may_leave_early_but_a_pod_may_not(self):
        self.assertEqual(self.check(cone=laser_run(cone=True, exit_at=40.0)), [])

    def test_the_hold_must_be_a_turn_inside_the_lasers_reach(self):
        flat = ((25.0, "15258", "2"), (40.0, "13360", "1"))
        self.assertTrue(any("not a turn" in p for p in self.check(laser_run(holds=flat))))
        far = ((25.0, "15258", "-72"), (40.0, "70000", "46"))
        self.assertTrue(any("out of the laser's reach" in p for p in self.check(laser_run(holds=far))))
        self.assertTrue(any("fewer than two" in p for p in self.check(laser_run(holds=()))))

    def test_a_lost_spot_or_a_blind_bomb_fails(self):
        self.assertTrue(any("spot was lost" in p for p in self.check(laser_run(lased=False))))
        self.assertTrue(any("with no spot" in p for p in self.check(laser_run(blind="2.2"))))

    def test_the_bomb_must_be_the_aircrafts_own_and_land_on_the_tank(self):
        self.assertTrue(any("on its own" in p for p in self.check(laser_run(spot="1"))))
        self.assertTrue(any("want a GBU-10" in p for p in self.check(laser_run(weapon="AGM65G.JT"))))
        self.assertTrue(any("ft from the target" in p for p in self.check(laser_run(miss=900.0))))
        self.assertTrue(any("unhurt" in p for p in self.check(laser_run(kill=False))))
        self.assertTrue(any("fell only" in p for p in self.check(laser_run(fall="9.0"))))


def buddy_head(ordered="true", callsign="Friendly 2-1", spot="1", own="Off", miss="8.5", hp="0", buddy_lased=True):
    lines = [
        "a2g-run: setup aircraft F18 station 2 GBU10.JT count 2 rounds 2 target 0x50000001 at 999570 7 789571 hp 200",
        f"a2g-run: buddy-ai-ordered t=0.258 plane=1 ok={ordered} target 0x50000000",
    ]
    if buddy_lased:
        lines.append(f'a2g-run: buddy-lasing t=0.275 range 50089 callsign "{callsign}"')
    lines += [
        f"a2g-run: release-sight t=23.758 kind Laser range 17072 reach 11995..18770 in-reach yes advisory none buddy yes laser {own} bar 16060 along 1011 seconds none steer none",
        f"a2g-run: release t=23.767 id=0 kind Laser mode Guiding spot-owner {spot} height 11998 speed 680.8 stored 999588 10 788231",
        f"a2g-run: impact t=73.150 id=0 mode Guiding at 1000295 14 786039 target-miss {miss} point-miss 8.5 target-hp {hp} flown 49.4",
        "a2g-run: summary released yes rounds 1",
        "a2g-run: kill 0x50000000 class 0x0400",
        f"a2g-run: target 0x50000000 hp {hp}/200",
        "a2g-run: done",
    ]
    return "\n".join(lines)


def buddy_drop(buddy="1", spot="1", lase_first=True, own_lased=False, phase_lase=False, miss=8.7, blind="0.0"):
    lines = [
        SETUP.format(skill=2, profile="none").replace("A4E", "F14").replace("MK82.JT", "GBU10.JT"),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=chosen",
        "ai-strike: start t=0.000 plane=0 range_ft=36456 alt_ft=12000 speed_kt=450",
        "ai-strike: PHASE t=0.008 name=Wait",
        f"ai-strike: DESIGNATE t=0.008 target=0x50000000 buddy={buddy} note=chosen",
    ]
    if lase_first:
        lines.append(f"ai-strike: LASER t=0.258 plane={buddy} state=Lasing mask=none range_ft=32628")
    if own_lased:
        lines.append("ai-strike: LASER t=2.000 plane=0 state=Lasing mask=none range_ft=30000")
    lines += ["ai-strike: PHASE t=2.008 name=Attack"]
    if phase_lase:
        lines.append("ai-strike: PHASE t=26.583 name=Lase")
    lines += [
        f"ai-strike: RELEASE t=26.583 n=1 weapon=GBU10.JT station=2 alt_ft=12009 speed_kt=418 range_ft=15771 pass=1 aim_ft=0 inside=yes guided=Guiding spot_owner={spot}",
        f"ai-strike: IMPACT t=77.025 n=1 miss_ft={miss} at=1.0,2.0,3.0",
        "ai-strike: KILL t=77.025 victim=0x50000000 category=0x0400 target=yes",
        "ai-strike: EGRESS t=77.025 range_ft=30760 alt_ft=14061",
        f"ai-strike: summary t=97.0 ran_s=97.0 profile=level releases=1 impacts=1 kills=1 egress=yes lost=no activity=Pursuing target_hp=0/200 closest_ft=905 store_rounds_left=1 fall_s=50.4 blind_s={blind}",
        "ai-strike: done",
    ]
    return "\n".join(lines)


def buddy_none(decision="manual", words=None, guided="Waiting", spot="none", miss=91.7, profile="dive",
               skill=2, releases=True, weapon="GBU10.JT", store="GBU-10E", nolaze=True, home=False):
    """The F-14 with a laser store and nobody lasing (ST-G): it says "No laze support" and either
    drops the laser bomb by hand (`manual`) or goes home (`rtb`)."""
    manual = decision == "manual"
    reason = "no_laze_manual" if manual else "no_laze_rtb"
    words = words or ("No laze support, dropping manual" if manual else "No laze support, RTB")
    lines = [
        SETUP.format(skill=skill, profile="none").replace("A4E", "F14").replace("MK82.JT", weapon),
        "ai-strike: PROFILE t=0.000 name=none requested=none note=chosen",
        "ai-strike: start t=0.000 plane=0 range_ft=36456 alt_ft=12000 speed_kt=450",
        "ai-strike: PHASE t=0.008 name=Wait",
    ]
    if nolaze:
        lines.append(
            f'ai-strike: NOLAZE t=20.008 decision={decision} reason={reason} weapon={store} station=2 '
            f'target=0x50000001 words="{words}"'
        )
    if manual:
        lines += [
            f"ai-strike: PROFILE t=20.008 name={profile}",
            "ai-strike: PHASE t=20.008 name=Attack",
        ]
        if releases:
            lines += [
                f"ai-strike: RELEASE t=46.867 n=1 weapon={weapon} station=2 alt_ft=6849 speed_kt=492 range_ft=3372 "
                f"pass=1 aim_ft=90 inside=yes lead_ft=430 dive_deg=63 guided={guided} spot_owner={spot}",
                f"ai-strike: IMPACT t=55.467 n=1 miss_ft={miss} at=1.0,2.0,3.0",
                "ai-strike: KILL t=55.467 victim=0x50000001 category=0x0400 target=yes",
                "ai-strike: EGRESS t=60.700 range_ft=6799 alt_ft=5944",
            ]
    else:
        lines += ["ai-strike: ACTIVITY t=10.008 name=In formation", "ai-strike: PHASE t=10.008 name=Home"]
        if releases:
            lines.append("ai-strike: RELEASE t=15.000 n=1 weapon=AS14.JT station=2 alt_ft=12000 speed_kt=418 range_ft=15771")
    n = 1 if manual and releases else 0
    lines.append(
        f"ai-strike: summary t=100.0 ran_s=100.0 profile={profile} releases={n} impacts={n} kills={n} egress={'yes' if n else 'no'} "
        f"lost=no activity=Attacking target_hp={'0' if n else '200'}/200 closest_ft=14233 store_rounds_left=1 fall_s=0.0 blind_s=0.0"
    )
    if not n:
        lines.append("ai-strike: no release")
    lines.append("ai-strike: done")
    return "\n".join(lines)


class LaserBuddyTests(unittest.TestCase):
    def check(self, head=None, drop=None, none=None):
        return s.laser_buddy_problems("\n".join([head or buddy_head(), drop or buddy_drop(), none or buddy_none()]))

    def test_an_honest_trio_passes(self):
        self.assertEqual(self.check(), [])

    def test_the_players_run_must_finish(self):
        self.assertTrue(any("did not finish" in p for p in s.laser_buddy_problems(buddy_drop() + "\n" + buddy_none())))

    def test_the_ai_must_be_ordered_lase_and_be_named(self):
        self.assertTrue(any("not ordered" in p for p in self.check(buddy_head(ordered="false"))))
        self.assertTrue(any("never lased" in p for p in self.check(buddy_head(buddy_lased=False))))
        self.assertTrue(any("Friendly 2-1" in p for p in self.check(buddy_head(callsign=""))))

    def test_the_players_bomb_must_home_on_the_ais_spot(self):
        self.assertTrue(any("aircraft 1's spot" in p for p in self.check(buddy_head(spot="0"))))
        self.assertTrue(any("buddy's spot" in p for p in self.check(buddy_head(own="Lasing"))))
        self.assertTrue(any("came down" in p for p in self.check(buddy_head(miss="400"))))
        self.assertTrue(any("not destroyed" in p for p in self.check(buddy_head(hp="120"))))

    def test_the_ais_bomb_must_home_on_the_players_spot(self):
        self.assertTrue(any("player's (1)" in p for p in self.check(drop=buddy_drop(spot="0"))))
        self.assertTrue(any("not up before" in p for p in self.check(drop=buddy_drop(lase_first=False))))
        self.assertTrue(any("no designator" in p for p in self.check(drop=buddy_drop(own_lased=True))))
        self.assertTrue(any("held a spot" in p for p in self.check(drop=buddy_drop(phase_lase=True))))
        self.assertTrue(any("landed" in p for p in self.check(drop=buddy_drop(miss=900.0))))
        self.assertTrue(any("with no spot" in p for p in self.check(drop=buddy_drop(blind="3.0"))))

    def test_with_nobody_lasing_the_bomb_is_dropped_by_hand_and_said_so(self):
        self.assertTrue(any("0 NOLAZE" in p for p in self.check(none=buddy_none(nolaze=False))))
        self.assertTrue(any("want manual" in p for p in self.check(none=buddy_none(decision="rtb"))))
        self.assertTrue(any("said" in p for p in self.check(none=buddy_none(words="Bombs away"))))
        self.assertTrue(any("want waiting" in p for p in self.check(none=buddy_none(guided="Guiding", spot="0"))))
        self.assertTrue(any("no manual drop" in p for p in self.check(none=buddy_none(releases=False))))
        self.assertTrue(any("landed" in p for p in self.check(none=buddy_none(miss=900.0))))
        self.assertTrue(any("bomb pass" in p for p in self.check(none=buddy_none(profile="none"))))


def no_laze_output(skills=(0, 1, 2), rtb=None):
    runs = [buddy_none(skill=k) for k in skills]
    runs.append(rtb or buddy_none(decision="rtb", releases=False, weapon="AS14.JT", store="AS-14"))
    return "\n".join(runs)


class NoLazeTests(unittest.TestCase):
    """`ai-strike-no-laze` (slice ST-G): three manual drops and one RTB."""

    def test_an_honest_set_passes(self):
        self.assertEqual(s.no_laze_problems(no_laze_output()), [])

    def test_the_runs_are_counted(self):
        self.assertTrue(any("runs finished" in p for p in s.no_laze_problems(buddy_none())))

    def test_the_manual_drop_must_be_a_bomb_pass_from_the_ballistic_solution(self):
        bad = no_laze_output(skills=(0, 1, 2)).replace("guided=Waiting", "guided=Guiding", 1)
        self.assertTrue(any("want waiting" in p for p in s.no_laze_problems(bad)))
        self.assertTrue(any("ran at skill" in p for p in s.no_laze_problems(no_laze_output(skills=(0, 1, 1)))))

    def test_the_rtb_must_say_so_and_not_release(self):
        silent = buddy_none(decision="rtb", releases=False, weapon="AS14.JT", nolaze=False)
        self.assertTrue(any("0 NOLAZE" in p for p in s.no_laze_problems(no_laze_output(rtb=silent))))
        released = buddy_none(decision="rtb", releases=True, weapon="AS14.JT")
        self.assertTrue(any("released after" in p for p in s.no_laze_problems(no_laze_output(rtb=released))))
        wrong = buddy_none(decision="rtb", releases=False, weapon="AS14.JT").replace("rtb reason", "manual reason", 1)
        self.assertTrue(any("want rtb" in p for p in s.no_laze_problems(no_laze_output(rtb=wrong))))


def commit_run(open_t=23.2, release_s="5.00", held="yes", held_at=26.625, release=27.758, defend=27.767,
               broke=None, released=True):
    """A held `ai-strike-commit-aaa` run: the window opens, a defence is held, the bomb leaves, Defend."""
    extra = ["ai-strike: PHASE t=0.008 name=Attack",
             f"ai-strike: COMMIT t={open_t:.3f} open=yes release_s={release_s} held_defence=no held_at=none"]
    if broke is not None:
        extra += [f"ai-strike: PHASE t={broke:.3f} name=Defend", f"ai-strike: PHASE t={broke + 1:.3f} name=Attack"]
    extra += [f"ai-strike: PHASE t={release:.3f} name=Exit",
              f"ai-strike: COMMIT t={release:.3f} open=no release_s=none held_defence={held} "
              f"held_at={held_at if held == 'yes' else 'none'}"]
    if defend is not None:
        extra.append(f"ai-strike: PHASE t={defend:.3f} name=Defend")
    return run(profile="straight", releases=[release] if released else [], extra=extra)


def control_run(defend=21.683, commit_at=None, release=None):
    extra = ["ai-strike: PHASE t=0.008 name=Attack"]
    if commit_at is not None:
        extra.append(f"ai-strike: COMMIT t={commit_at:.3f} open=yes release_s=5.00 held_defence=no held_at=none")
    extra.append(f"ai-strike: PHASE t={defend:.3f} name=Defend")
    return run(profile="straight", releases=[release] if release is not None else [], extra=extra)


class CommitTests(unittest.TestCase):
    HELD = [dict(label="held", kind="held")]
    BREAKS = [dict(label="control", kind="breaks")]

    def test_a_held_run_passes(self):
        self.assertEqual(s.commit_problems(commit_run(), self.HELD), [])

    def test_the_window_must_open_within_5_s(self):
        problems = s.commit_problems(commit_run(release_s="6.20"), self.HELD)
        self.assertTrue(any("want 0 to 5" in p for p in problems), problems)

    def test_no_held_defence_fails(self):
        problems = s.commit_problems(commit_run(held="no"), self.HELD)
        self.assertTrue(any("no defence came" in p for p in problems), problems)

    def test_a_defence_outside_the_window_fails(self):
        problems = s.commit_problems(commit_run(held_at=22.0), self.HELD)
        self.assertTrue(any("outside the window" in p for p in problems), problems)

    def test_the_release_must_be_inside_the_window(self):
        problems = s.commit_problems(commit_run(released=False), self.HELD)
        self.assertTrue(any("no RELEASE inside" in p for p in problems), problems)

    def test_a_break_inside_the_window_fails(self):
        problems = s.commit_problems(commit_run(broke=25.0), self.HELD)
        self.assertTrue(any("broke into Defend" in p for p in problems), problems)

    def test_the_striker_must_defend_right_after_the_release(self):
        problems = s.commit_problems(commit_run(defend=None), self.HELD)
        self.assertTrue(any("no Defend within" in p for p in problems), problems)
        problems = s.commit_problems(commit_run(defend=29.0), self.HELD)
        self.assertTrue(any("no Defend within" in p for p in problems), problems)

    def test_the_control_breaks_outside_the_window(self):
        self.assertEqual(s.commit_problems(control_run(), self.BREAKS), [])

    def test_a_control_with_the_window_open_or_a_release_first_fails(self):
        problems = s.commit_problems(control_run(commit_at=20.0), self.BREAKS)
        self.assertTrue(any("must break outside it" in p for p in problems), problems)
        problems = s.commit_problems(control_run(release=20.0), self.BREAKS)
        self.assertTrue(any("released before the Defend" in p for p in problems), problems)
        self.assertTrue(s.commit_problems(commit_run(), self.BREAKS))

    def test_the_scenario_exists_with_held_runs_and_a_control(self):
        scenario = next(sc for sc in s.scenarios() if sc.name == "ai-strike-commit-aaa")
        self.assertEqual(len(scenario.then), 3)
        self.assertIn("--place-site", scenario.then[-1].args)


class SelectionTests(unittest.TestCase):
    def families(self, path):
        return set(bs.rule_for(path).families)

    def test_the_driver_source_selects_the_strike_and_a2g_families(self):
        self.assertEqual(self.families("crates/tore-app/src/a2g_run/ai.rs"), {"a2g", "ai-strike"})
        self.assertEqual(self.families("crates/tore-app/src/a2g_run/guided.rs"), {"a2g", "ai-strike"})
        for path in ("crates/tore-sim/src/combat/laser.rs", "crates/tore-sim/src/combat/live/sighting.rs"):
            self.assertIn("ai-strike", self.families(path), path)
        self.assertEqual(self.families("crates/tore-app/src/a2g_run/level.rs"), {"a2g"})
        self.assertEqual(self.families("crates/tore-app/src/a2g_run.rs"), {"a2g", "ai-strike"})

    def test_the_new_scenarios_belong_to_the_strike_family(self):
        import fnmatch
        patterns = bs.FAMILIES["ai-strike"][1]
        for name in ("ai-wing-ground-order", "ai-wing-lase", "qmc-strike-wing"):
            self.assertTrue(any(fnmatch.fnmatch(name, p) for p in patterns), name)

    def test_the_ai_sources_select_the_strike_family(self):
        for path in ("crates/tore-sim/src/ai/strike.rs", "crates/tore-sim/src/ai/mission.rs",
                     "crates/tore-world/src/ai_wings/strike.rs", "crates/tore-world/src/ai_wings.rs"):
            self.assertIn("ai-strike", self.families(path), path)

    def test_the_scenario_file_and_its_tests_select_the_unit_tests(self):
        rule = bs.rule_for("tools/battery_scenarios/ai_strike.py")
        self.assertEqual(set(rule.families), {"ai-strike"})
        self.assertIn("test_battery_ai_strike", rule.unit_tests)
        self.assertEqual(bs.rule_for("tools/test_battery_ai_strike.py").families, ())

    def test_the_family_names_only_strike_scenarios(self):
        description, patterns = bs.FAMILIES["ai-strike"]
        self.assertIn("ai-strike-*", patterns)
        self.assertIn("ai-sead-*", patterns)
        self.assertTrue(description)


if __name__ == "__main__":
    unittest.main()


class ManeuverTests(unittest.TestCase):
    """ST-T2: the dive angle, release height, first pass, lead and burst checks."""

    def strike(self, fields, **extra):
        out = run(2, "dive", named="dive", releases=[10.0], impacts=[(27.0, 50.0)], egress=33.0,
                  release_fields=fields)
        want = [dict(label="r", profile="dive", skill=2, miss=s.MISS_FT_MAX[2], **extra)]
        return s.strike_problems(out, want)

    def test_a_steep_enough_dive_passes_and_a_shallow_one_fails(self):
        self.assertEqual(self.strike(" lead_ft=300 dive_deg=60", min_dive=45, moving=True), [])
        shallow = self.strike(" lead_ft=300 dive_deg=30", min_dive=45)
        self.assertTrue(any("degree dive" in p for p in shallow), shallow)

    def test_release_heights_are_bounded_where_asked(self):
        self.assertEqual(self.strike(" dive_deg=0", max_alt=5000, min_alt=4000), [])
        self.assertTrue(any("at most" in p for p in self.strike(" dive_deg=0", max_alt=1000)))
        self.assertTrue(any("at least 9500" in p for p in self.strike(" dive_deg=0", min_alt=9500)))

    def test_a_moving_target_needs_a_lead_and_a_standing_one_none(self):
        self.assertTrue(any("no lead" in p for p in self.strike(" lead_ft=0", moving=True)))
        self.assertTrue(any("standing target" in p for p in self.strike(" lead_ft=40", moving=False)))
        self.assertEqual(self.strike(" lead_ft=0", moving=False), [])

    def test_the_first_release_on_pass_one_and_gun_bursts(self):
        self.assertEqual(self.strike("", first_pass=True), [])
        self.assertTrue(any("bursts" in p for p in self.strike("", bursts=1)))
        self.assertEqual(self.strike(" burst=1 rounds_left=1179", bursts=1), [])

    def test_a_rocket_or_gun_spread_widens_the_miss_limit(self):
        far = s.MISS_FT_INSIDE[2] + 100
        out = run(2, "dive", named="dive", releases=[10.0], impacts=[(27.0, far)], egress=33.0)
        base = [dict(label="r", profile="dive", skill=2, miss=s.MISS_FT_MAX[2])]
        self.assertTrue(any("from the target" in p for p in s.strike_problems(out, base)))
        wide = [dict(base[0], spread=s.SCATTER_FT + 150)]
        self.assertEqual(s.strike_problems(out, wide), [])

    def test_the_maneuver_scenarios_exist(self):
        names = {sc.name for sc in s.scenarios()}
        for name in ("ai-strike-dive-steep", "ai-strike-strafe", "ai-strike-rockets-popup",
                     "ai-strike-rockets-dive", "ai-strike-snakeye", "ai-strike-heavy-level",
                     "ai-strike-offset", "ai-strike-lead-dive", "ai-strike-lead-level"):
            self.assertIn(name, names)
