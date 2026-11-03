"""The missile defense regression must reject the original weak turn and push."""
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from battery_scenarios.ai_defense import defense_problems, head_break_problems, chaff_timing_problems, skill_pull_problems, radar_memory_problems, inert_defense_problems


class InertDefenseChecks(unittest.TestCase):
    def test_requires_a_previously_observed_threat_and_immediate_end(self):
        trace = "\n".join(f"missile-study: inert-defense {store} seed 0 t=45.000 active_before=true cleared=true"
                          for store in ("AIM7.JT", "AIM9M.JT"))
        self.assertEqual(inert_defense_problems(Path(), trace), [])
        for invalid in ("", trace.splitlines()[0], trace.replace("active_before=true", "active_before=false"),
                        trace.replace("cleared=true", "cleared=false")):
            self.assertTrue(inert_defense_problems(Path(), invalid))


class DefenseChecks(unittest.TestCase):
    def check_case(self, g=3.0, turn=60, wrong=0, headings=(270, 271, 272, 273), bank=60, devices=4, passed=False, late_release=False):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "study").mkdir()
            (work / "study/runs.csv").write_text(
                f"fired,target_turn_deg,devices,other_devices\n1,{turn},{devices},{wrong}\n")
            phase = "Break" if passed else "Beam"
            time = "None" if passed else "Some(3.0)"
            trace = "\n".join(
                f"missile-study: trace AIM9M seed 0 bank={bank} g={g} alt=5000 activity=Defending "
                f"defense=Some((Some(MotionSuggestion {{ maneuver: {phase}, heading_deg: {h}, "
                f"flight_path_pitch_deg: -5.0, reduce_heat: false }}), None, {time}, true))"
                for h in headings)
            if late_release:
                trace += "\nmissile-study: device AIM9M seed 0 t=6.0 kind=Flare defense=Some((None, None, true))"
            return defense_problems(work, trace)

    def test_positive_turn_passes(self):
        self.assertEqual(self.check_case(), [])

    def test_original_push_weak_turn_and_mixed_burst_fail(self):
        self.assertEqual(len(self.check_case(g=-1.5, turn=7, wrong=4)), 3)

    def test_notch_flip_fails(self):
        self.assertIn("beam/notch side flipped", self.check_case(headings=(270, 271, 90, 89)))

    def test_inverted_overshoot_fails(self):
        self.assertIn("defensive bank exceeded 90 degrees", self.check_case(bank=-105))

    def test_a_missing_estimate_is_not_confirmed_passage(self):
        self.assertEqual(self.check_case(passed=True), [])

    def test_a_queued_release_after_passage_fails(self):
        self.assertIn("device released after the threat passed", self.check_case(late_release=True))

    def test_exceeding_the_budget_fails(self):
        self.assertIn("single-threat defense exceeded its eight-device budget", self.check_case(devices=10))

    def test_low_skill_chaff_must_precede_the_break_and_keep_radar_heat(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "study").mkdir()
            runs = work / "study/runs.csv"
            header = "fired,target_turn_deg,devices,other_devices,first_device_s\n"
            runs.write_text(header + "1,30,4,0,2.00\n")
            trace = "\n".join(
                f"missile-study: trace AIM120 seed 0 t={t:.2f} bank=60 g=3 alt=5000 activity=Defending "
                f"defense=Some((Some(MotionSuggestion {{ maneuver: {phase}, heading_deg: 270, "
                "flight_path_pitch_deg: -5.0, reduce_heat: false }), None, Some(3.0), true))"
                for t, phase in [(2.0, "Notch"), (2.25, "Notch"), (4.0, "Break"), (4.25, "Break")])
            self.assertEqual(chaff_timing_problems(work, trace), [])
            self.assertIn("known radar threat requested an IR heat cut", chaff_timing_problems(work, trace.replace("reduce_heat: false", "reduce_heat: true")))
            runs.write_text(header + "1,30,4,0,4.00\n")
            self.assertIn("first chaff waited for the last-ditch break", chaff_timing_problems(work, trace))


class ReviewTwoChecks(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.work = Path(self.tmp.name)
        (self.work / "study").mkdir()
        (self.work / "study/runs.csv").write_text("fired,target_turn_deg,devices,other_devices\n1,40,6,0\n")

    def trace(self, phases, loads):
        return "\n".join(
            f"missile-study: trace AIM7 seed 0 t={i * .5:.2f} bank=75 g={g} alt=5000 activity=Defending "
            f"defense=Some((Some(MotionSuggestion {{ maneuver: {phase}, heading_deg: 270, reduce_heat: false }}), None, None, true))"
            for i, (phase, g) in enumerate(zip(phases, loads)))

    def test_slow_pull_is_rejected_even_if_it_eventually_reaches_full_g(self):
        check = skill_pull_problems("ace")
        slow = self.trace(["Break"] * 5, [1, 2, 3, 4, 5.4])
        self.assertTrue(any("did not build" in p for p in check(self.work, slow)))
        self.assertEqual(check(self.work, self.trace(["Break"] * 5, [1, 4, 5.4, 5.4, 5.4])), [])

    def test_missing_estimate_is_allowed_but_restarting_a_break_is_not(self):
        self.assertEqual(radar_memory_problems(self.work, self.trace(["Break"] * 5, [4] * 5)), [])
        interrupted = self.trace(["Break", "Break", "Beam", "Break", "Break"], [4] * 5)
        self.assertIn("radar break dropped and restarted", radar_memory_problems(self.work, interrupted))
        (self.work / "study/runs.csv").write_text("fired,target_turn_deg,devices,other_devices\n1,40,4,2\n")
        self.assertIn("released wrong-class devices", radar_memory_problems(self.work, interrupted))


class HeadOnChecks(unittest.TestCase):
    def test_a_straight_command_cannot_count_as_a_head_on_break(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "study").mkdir()
            (work / "study/runs.csv").write_text("fired,target_turn_deg,devices,other_devices\n1,10,4,0\n")
            trace = "\n".join(
                "missile-study: trace AIM9M seed 0 yaw=0 g=3 alt=5000 activity=Defending "
                "defense=Some((Some(MotionSuggestion { maneuver: Break, heading_deg: 0"
                for _ in range(4))
            self.assertEqual(head_break_problems(work, trace), ["head-on break straightened toward the missile"])
            self.assertEqual(head_break_problems(work, trace.replace("heading_deg: 0", "heading_deg: 90")), [])
