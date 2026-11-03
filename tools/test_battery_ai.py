"""Tests for the AI lane's output checks (tools/battery_scenarios/ai.py)."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from battery_scenarios import ai  # noqa: E402

CLEAN = """AI probe: aircraft=F/A-18D actors=3 ticks=7200 enemy_skill=None mission=free
t=0 (0.0s) Enemy 1-1 F18: alive=true agl=5000 kt=450 x=1 z=2 hdg=197 terrain_agl=5000
t=1289 (10.7s) destroyed: Enemy 1-1
t=1289 (10.7s) Enemy 1-1 F18: alive=false agl=4185 kt=430 x=1 z=2 hdg=199 terrain_agl=4185
actor=1 Friendly 1-2 F18 activity=In formation alive=true rounds=582 x=1.0 y=2.0 z=3.0 hdg=4.0
actor=2 Enemy 1-1 F18 activity=Destroyed alive=false rounds=582 x=1.0 y=2.0 z=3.0 hdg=4.0
AI probe debrief: FAILURE [Destroy { destroyed: 1, total: 2 }] elapsed=60s player[Alive damage=0% kills=[1, 0, 0, 0, 0, 0, 0, 0, 0, 0] ff=0 a2a=1/1 dmg=140 gun=0/0 a2g=0/0 bomb=0/0 enemy_aam=0/0 enemy_gun=0/0] wingman[-]
AI probe radio: calls=2 heard=2
  5.5s YOU: 'Fox one' ["^FOXONE"]
  10.8s YOU: 'Splash' ["^SPLASH"]
AI probe attack: clicks=1 steps=1 presses=1 missiles=1 gun_bursts=0 gun_rounds=0 player_hits=1 player_kills=1 hits=1 destroyed=1 lost_friendly=0/1 lost_enemy=1/2 player_alive=true player_damaged=0 ejections=0 perceived=1
AI probe totals: wings=3 ticks=7200 shots=3 dropped=0 warnings=0 live_projectiles=0 player_hp=232 target_hp=[1] checksum=0
AI probe invariants: actors=3 samples=7200 anomalies=0 peak_heading_rate=1.0 peak_bank_rate=1.0 peak_kt=450 peak_alt_ft=5000 kinds=[]
"""


class ProbeCheckTests(unittest.TestCase):
    def test_clean_output_passes(self):
        self.assertEqual(ai.probe_problems(CLEAN), [])

    def test_a_kill_needs_a_recorded_hit(self):
        # The spoofed-missile accounting bug: a kill credited, no hit tallied.
        text = CLEAN.replace("a2a=1/1 dmg=140", "a2a=0/1 dmg=0")
        self.assertTrue(any("1 kills but only 0 recorded hits" in p for p in ai.probe_problems(text)))
        # Hits of any weapon class count, in either column.
        text = CLEAN.replace("a2a=1/1 dmg=140", "a2a=0/1 dmg=0").replace("bomb=0/0", "bomb=1/1")
        self.assertEqual(ai.probe_problems(text), [])
        text = CLEAN.replace("wingman[-]", "wingman[Alive damage=0% kills=[0, 1, 0] ff=1 a2a=1/1 dmg=9 gun=0/0 a2g=0/0 bomb=0/0 enemy_aam=0/0 enemy_gun=0/0]")
        self.assertTrue(any("wingman with 2 kills but only 1" in p for p in ai.probe_problems(text)))

    def test_missing_invariants_line(self):
        text = CLEAN.replace("AI probe invariants:", "AI probe other:")
        self.assertTrue(any("invariants" in p for p in ai.probe_problems(text)))

    def test_anomaly_lines_and_allowances(self):
        text = CLEAN + "AI probe anomaly: t=5 (0.0s) outside the world: Enemy 1-1 x=-5 z=1\n"
        self.assertTrue(ai.probe_problems(text))
        self.assertEqual(ai.probe_problems(text, allow_anomalies=("outside the world",)), [])

    def test_labels_and_final_state_must_agree(self):
        text = CLEAN.replace("activity=Destroyed alive=false", "activity=Searching alive=false")
        self.assertTrue(any("dead but" in p for p in ai.probe_problems(text)))
        text = CLEAN.replace("activity=Destroyed alive=false", "activity=Destroyed alive=true")
        problems = ai.probe_problems(text)
        self.assertTrue(any("alive but" in p for p in problems))
        self.assertTrue(any("destroyed at 10.7s but alive" in p for p in problems))

    def test_dropped_launches_and_objective_count(self):
        self.assertTrue(ai.probe_problems(CLEAN.replace("dropped=0", "dropped=2")))
        text = CLEAN.replace("destroyed: 1, total: 2", "destroyed: 2, total: 2")
        self.assertTrue(any("objective" in p for p in ai.probe_problems(text)))
        # Other missions choose their own objective group.
        self.assertEqual(ai.probe_problems(text.replace("mission=free", "mission=intercept")), [])

    def test_debrief_kills_never_below_the_combat_count(self):
        text = CLEAN.replace("kills=[1, 0,", "kills=[0, 0,")
        self.assertTrue(any("kills" in p for p in ai.probe_problems(text)))
        # Credited kills beyond the combat count (an ejection after damage)
        # are fine when each has its recorded hit.
        text = CLEAN.replace("kills=[1, 0,", "kills=[2, 0,").replace("a2a=1/1 dmg=140", "a2a=2/2 dmg=240")
        self.assertEqual(ai.probe_problems(text), [])

    def test_undamaged_ground_collision(self):
        text = CLEAN.replace(
            "t=1289 (10.7s) destroyed: Enemy 1-1\n", ""
        ).replace("alive=false agl=4185 kt=430", "alive=false agl=8 kt=0")
        self.assertTrue(any("flew into the ground" in p for p in ai.probe_problems(text)))
        damaged = text + "AI damage: actor=2 faults=[(4, 1)] return=None\n"
        self.assertFalse(any("flew into the ground" in p for p in ai.probe_problems(damaged)))
        structural = text + "AI damage: actor=2 faults=[] damage_fraction=0.241\n"
        self.assertFalse(any("flew into the ground" in p for p in ai.probe_problems(structural)))
        tiny = text + "AI damage: actor=2 faults=[] damage_fraction=1e-9\n"
        self.assertFalse(any("flew into the ground" in p for p in ai.probe_problems(tiny)))
        healthy = text + "AI damage: actor=2 faults=[] damage_fraction=0\n"
        self.assertTrue(any("flew into the ground" in p for p in ai.probe_problems(healthy)))

    def test_repeated_radio_line(self):
        text = CLEAN + "".join(f"  {20 + i}.0s Red two: 'Contact' []\n" for i in range(3))
        self.assertTrue(any("radio repeats" in p for p in ai.probe_problems(text)))

    def test_hit_calls_may_repeat(self):
        hits = "".join(f"  {20 + i}.0s YOU: 'Get this guy off me' [\"^OFFME\"]\n" for i in range(3))
        self.assertFalse(any("radio repeats" in p for p in ai.probe_problems(CLEAN + hits)))
        calls = "".join(f"  {20 + i}.0s YOU: 'Fox one' [\"^FOXONE\"]\n" for i in range(3))
        self.assertTrue(any("radio repeats" in p for p in ai.probe_problems(CLEAN + calls)))

    def test_scenarios_are_unique_and_named_for_the_lane(self):
        names = [s.name for s in ai.scenarios()]
        self.assertEqual(len(names), len(set(names)))
        self.assertTrue(all(n.startswith("ai-") for n in names))
        self.assertGreater(len(names), 300)

class ShortStripTests(unittest.TestCase):
    """John, 2026-09-30: no ground start on a short strip, and since slice AL1 none at a Redfor
    field, so no AI scenario or fuzz seed uses one."""

    def test_a_short_strip_or_enemy_field_moves_to_the_next_airport_that_is_not(self):
        from battery_scenarios._strips import SHORT_STRIPS, ground_airport, is_enemy_field, is_short_strip

        self.assertEqual(sum(len(v) for v in SHORT_STRIPS.values()), 22)
        self.assertEqual(ground_airport("APA", 1), 1)
        self.assertEqual(ground_airport("APA", 3), 7)
        self.assertEqual(ground_airport("UKR", 1), 2)
        self.assertEqual(ground_airport("UKR", 3), 8)
        self.assertEqual(ground_airport("NSK", 1), 9)
        # No Blue field: the Falklands' one is a short strip, North Vietnam has none.
        self.assertIsNone(ground_airport("LFA", 1))
        self.assertIsNone(ground_airport("TVIET", 1))
        self.assertIsNone(ground_airport("CUB", 3))
        self.assertEqual(ground_airport("~APAF", 2), 2)
        self.assertTrue(is_short_strip("CUB", 13) and not is_short_strip("CUB", 1))
        self.assertTrue(is_enemy_field("UKR", 1) and not is_enemy_field("UKR", 2))

    def test_no_scenario_and_no_fuzz_seed_starts_on_a_short_strip_or_enemy_field(self):
        from battery_scenarios import _ai_fuzz
        from battery_scenarios._strips import is_ground_start

        def theater_and_airport(args):
            if "--ground-start" not in args:
                return None
            theater = args[args.index("--theater") + 1] if "--theater" in args else "UKR"
            return theater, int(args[args.index("--ground-start") + 1])

        from battery_scenarios._strips import is_enemy_field, is_short_strip

        def redfor_start(theater, airport):
            return is_enemy_field(theater, airport) and not is_short_strip(theater, airport)

        for s in ai.scenarios():
            found = theater_and_airport(s.args)
            if found and "--probe-redfor" in s.args:
                # The Redfor seat (slice CL-R) starts at a Redfor field instead.
                self.assertTrue(redfor_start(*found), f"{s.name}: the Redfor seat starts on a short strip or Blue field")
            elif found:
                self.assertTrue(is_ground_start(*found), f"{s.name} starts on a short strip or enemy field")
            if "--enemy-ground-start" in s.args:
                theater = s.args[s.args.index("--theater") + 1] if "--theater" in s.args else "UKR"
                airport = int(s.args[s.args.index("--enemy-ground-start") + 1])
                self.assertTrue(redfor_start(theater, airport), f"{s.name}: the enemy starts on a short strip or Blue field")
        for seed in _ai_fuzz.SEEDS:
            args, _, _ = _ai_fuzz.config(seed)
            found = theater_and_airport(args)
            if found:
                self.assertTrue(is_ground_start(*found), f"fuzz seed {seed} starts on a short strip or enemy field")


class PlayerFliesTests(unittest.TestCase):
    """Probes with no enemy near require the scripted player to survive its climb-out."""

    CRASHED = "player crashed=true gear=false x=1.0 y=2.0 z=3.0 hdg=8.0\n"
    DOWN = "t=2017 (16.8s) player: on_ground=false gear=false crashed=true agl=85 kt=202 x=1 z=2 hdg=8 terrain_agl=85\n"

    def test_a_crashed_player_fails_only_when_asked(self):
        text = CLEAN + self.DOWN + self.CRASHED
        self.assertEqual(ai.probe_problems(text), [])
        problems = ai.probe_problems(text, player_flies=True)
        self.assertEqual(problems, ["the scripted player crashed at tick 2017"])

    def test_a_flying_player_passes(self):
        text = CLEAN + "player crashed=false gear=false x=1.0 y=2.0 z=3.0 hdg=8.0\n"
        self.assertEqual(ai.probe_problems(text, player_flies=True), [])


class LandOrderTests(unittest.TestCase):
    """The scripted player leads a living wing: its land order is accepted and
    the landing is checked. A probe that loses the player on purpose
    (`player_lost`) follows lead succession: the order is refused and the new AI
    lead flies a mission of opportunity, which with no enemy is a return to base
    (John, 2026-09-30)."""

    DOWN = "t=2017 (16.8s) player: on_ground=false gear=false crashed=true agl=85 kt=202 x=1 z=2 hdg=8 terrain_agl=85\n"
    REFUSED = 't=12000 order=LandAtSelected reply="Wing order unavailable: you are not leading your wing"\n'
    ACCEPTED = 't=12000 order=LandAtSelected reply="Land at Simferopol: 1 landing"\n'
    HOME = "t=2017 (16.8s) mission of opportunity: Friendly 1-2 leads, returning to base (no enemy position known)\n"
    LANDED = "AI probe phases: Friendly 1-2: Waiting@0.0s ClimbOut@57.5s Airborne@83.7s Approach@83.7s Final@377.4s Rollout@409.3s Parked@729.0s\n"
    FLYING = "AI probe phases: Friendly 1-2: Waiting@0.0s ClimbOut@57.5s Airborne@83.7s\n"

    def check(self, text, whole=None, lost=True):
        return ai.land_order_checker(whole or (lambda output: ["did not land"]), player_lost=lost)(text)

    def flying(self, text, whole=None):
        return self.check(text, whole, lost=False)

    def passes(self, output):
        return []

    def test_a_crashed_player_is_refused_and_the_new_lead_lands_at_home(self):
        self.assertEqual(self.check(CLEAN + self.DOWN + self.REFUSED + self.HOME + self.LANDED, whole=self.passes), [])

    def test_the_landing_checks_apply_after_the_refusal(self):
        self.assertEqual(self.check(CLEAN + self.DOWN + self.REFUSED + self.HOME + self.LANDED), ["did not land"])

    def test_the_new_lead_must_return_to_base(self):
        problems = self.check(CLEAN + self.DOWN + self.REFUSED + self.FLYING, whole=self.passes)
        self.assertTrue(any("never returned to base" in p for p in problems), problems)

    def test_the_new_lead_must_land(self):
        problems = self.check(CLEAN + self.DOWN + self.REFUSED + self.HOME + self.FLYING, whole=self.passes)
        self.assertTrue(any("never landed" in p for p in problems), problems)

    def test_the_new_lead_has_no_enemy_to_search_for(self):
        text = CLEAN + self.DOWN + self.REFUSED + self.HOME.replace("no enemy position known", "search time up") + self.LANDED
        self.assertTrue(any("wrong reason" in p for p in self.check(text, whole=self.passes)))

    def test_the_lead_may_no_longer_fly_off_the_map(self):
        text = CLEAN + self.DOWN + self.REFUSED + self.HOME + self.LANDED + "AI probe anomaly: t=5 (0.0s) outside the world: Friendly 1-2 x=-5 z=1\n"
        whole = ai.checker(ground=True, need_takeoff=True)
        self.assertTrue(any("outside the world" in p for p in self.check(text, whole=whole)))

    def test_a_wingman_still_waiting_when_the_wing_goes_home_stays_parked(self):
        parked = "AI probe phases: Friendly 1-4: Waiting@0.0s\n"
        text = CLEAN + self.DOWN + self.LANDED + parked
        self.assertTrue(any("1-4 never took off" in p for p in ai.probe_problems(text, ground=True, need_takeoff=True)))
        self.assertEqual(ai.probe_problems(text + self.HOME, ground=True, need_takeoff=True), [])

    def test_the_lost_lead_flies_the_route_then_lands(self):
        route = (
            "t=7119 (59.3s) mission of opportunity: Friendly 1-2 leads, flying its waypoints, 2 left (no enemy position known)\n"
            "t=25608 (213.4s) mission of opportunity: Friendly 1-2 leads, flying its waypoints, 1 left (no enemy position known)\n"
            "t=43672 (363.9s) mission of opportunity: Friendly 1-2 leads, returning to base (waypoints flown)\n"
        )
        self.assertEqual(ai.lost_lead_route_checker(CLEAN + self.DOWN + route + self.LANDED), [])
        problems = ai.lost_lead_route_checker(CLEAN + self.DOWN + self.HOME + self.LANDED)
        self.assertTrue(any("expected" in p for p in problems), problems)

    def test_a_crashed_player_must_not_be_obeyed(self):
        self.assertTrue(any("already crashed" in p for p in self.check(CLEAN + self.DOWN + self.ACCEPTED)))

    def test_a_lost_player_probe_must_lose_the_player(self):
        problems = self.check(CLEAN + self.ACCEPTED, whole=self.passes)
        self.assertTrue(any("did not" in p for p in problems), problems)

    def test_a_flying_player_is_obeyed_and_the_landing_is_checked(self):
        self.assertEqual(self.flying(CLEAN + self.ACCEPTED, whole=self.passes), [])
        self.assertEqual(self.flying(CLEAN + self.ACCEPTED), ["did not land"])

    def test_a_crashed_player_is_reported_not_excused_when_it_should_fly(self):
        text = CLEAN + self.DOWN + self.REFUSED + self.HOME + self.LANDED
        problems = self.flying(text, whole=self.passes)
        self.assertTrue(any("scripted player crashed" in p for p in problems), problems)
        self.assertTrue(any("refused although the player led" in p for p in problems), problems)

    def test_an_order_that_was_never_given_is_reported(self):
        self.assertTrue(any("never given" in p for p in self.flying(CLEAN, whole=self.passes)))

    def test_the_flying_player_must_not_be_refused(self):
        self.assertTrue(any("still led the wing" in p for p in self.check(CLEAN + self.REFUSED, whole=self.passes)))


class DataLinkPictureTests(unittest.TestCase):
    MEMBERS = "".join(f"t=1 data link: member plane={n} radar=true\n" for n in range(3))
    LOCKED = "t=660 data link: lock plane=0 target=2\n"

    def check(self, text, **kw):
        planes = kw.pop("planes", 3)
        return ai.datalink_picture_check(planes=planes, designated=2)(CLEAN + text)

    def test_a_full_picture_passes(self):
        text = self.MEMBERS + self.LOCKED + "t=900 data link: unlock plane=0 target=2\n"
        self.assertEqual(self.check(text), [])

    def test_every_plane_is_announced_once(self):
        problems = self.check(self.MEMBERS.replace("plane=2", "plane=1") + self.LOCKED)
        self.assertTrue(any("twice" in p for p in problems), problems)
        self.assertTrue(any("announced" in p for p in problems), problems)

    def test_every_plane_reports_a_radar(self):
        problems = self.check(self.MEMBERS.replace("plane=0 radar=true", "plane=0 radar=false") + self.LOCKED)
        self.assertTrue(any("no radar" in p for p in problems), problems)

    def test_the_designation_must_become_a_lock(self):
        problems = self.check(self.MEMBERS)
        self.assertTrue(any("never locked" in p for p in problems), problems)

    def test_a_lock_must_pair_with_its_unlock(self):
        problems = self.check(self.MEMBERS + self.LOCKED + "t=9 data link: unlock plane=1 target=2\n")
        self.assertTrue(any("did not hold" in p for p in problems), problems)
        problems = self.check(self.MEMBERS + self.LOCKED + self.LOCKED)
        self.assertTrue(any("over a lock" in p for p in problems), problems)


if __name__ == "__main__":
    unittest.main()
