import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from battery_scenarios import _replay_checks as rc  # noqa: E402

GOOD_ACMI = """FileType=text/acmi/tacview
FileVersion=2.2
0,ReferenceTime=2026-09-29T09:44:00Z
#0
10000000000,T=0.8|-0.6|1524|0|0|17|326|179|17,Type=Air+FixedWing,Name=F/A-18D
#1
10000000000,T=0.8|-0.6|1500|0|0|17|326|179|17
0,Event=Destroyed|10000000000|gone
-10000000000
"""


class AcmiTests(unittest.TestCase):
    def test_good_file_passes(self):
        self.assertEqual(rc.check_acmi(GOOD_ACMI), [])

    def test_header_time_and_lifetime_problems_are_found(self):
        self.assertTrue(rc.check_acmi("nonsense\n"))
        self.assertTrue(rc.check_acmi(GOOD_ACMI.replace("#1\n", "#-1\n#0.5\n#0.4\n")))
        self.assertTrue(rc.check_acmi(GOOD_ACMI + "10000000000,T=0|0|0|0|0|0|0|0|0\n"))
        self.assertTrue(rc.check_acmi(GOOD_ACMI.replace("1500", "nan")))
        self.assertTrue(rc.check_acmi(GOOD_ACMI.replace("-10000000000\n", "-10000000009\n")))
        self.assertTrue(rc.check_acmi(GOOD_ACMI.split("#0\n")[0]))

    def test_a_one_frame_flight_has_no_time_span_and_may_be_excused(self):
        one = "FileType=text/acmi/tacview\nFileVersion=2.2\n0,Title=t\n#0\n10000000000,T=0|0|1500|0|0|0|0|0|0,Name=F/A-18D\n"
        self.assertIn("ACMI has no time span", rc.check_acmi(one))
        self.assertEqual(rc.check_acmi(one, need_span=False), [])


class AcmiVsLogTests(unittest.TestCase):
    LOG = '{"type":"sample","t":1,"id":0,"pos_ft":[3280.84,1000,6561.68]}\n'

    def test_matching_positions_pass_and_wrong_ones_fail(self):
        good = "#1\n10000000000,T=1|2|304.8|0|0|0|1000|2000|0,Type=Air\n"
        self.assertEqual(rc.acmi_vs_log(good, self.LOG), [])
        bad = good.replace("304.8", "500")
        self.assertTrue(rc.acmi_vs_log(bad, self.LOG))
        self.assertTrue(rc.acmi_vs_log("#5\n10000000000,T=1|2|304.8|0|0|0|1000|2000|0\n", self.LOG))


class InfoVsLogTests(unittest.TestCase):
    def test_counts_must_agree(self):
        info = "Events      3 in all\n                  2 weapon.launch\n                  1 system.end\n"
        log = '{"type":"event","kind":"weapon.launch"}\n{"type":"event","kind":"weapon.launch"}\n{"type":"event","kind":"system.end"}\n'
        self.assertEqual(rc.info_vs_log(info, log), [])
        self.assertTrue(rc.info_vs_log(info, log.replace("system.end", "weapon.launch")))


class JsonlTests(unittest.TestCase):
    def test_lines_must_be_json_with_one_header(self):
        header = '{"type":"header","format":1}\n{"type":"aircraft","id":0}\n'
        self.assertEqual(rc.check_jsonl(header + '{"type":"sample","t":0}\n{"type":"sample","t":1}\n'), [])
        self.assertTrue(rc.check_jsonl(header + "{broken\n"))
        self.assertTrue(rc.check_jsonl(header + '{"type":"sample","t":2}\n{"type":"sample","t":1}\n'))
        self.assertTrue(rc.check_jsonl(header + '{"type":"aircraft","id":0}\n'))
        self.assertTrue(rc.check_jsonl(header + '{"type":"sample","t":NaN}\n'))


class SummaryTests(unittest.TestCase):
    def test_sections_are_required(self):
        text = "T.O.R.E mission summary\n\nAircraft\n--------\nYou (F/A-18D, F18.PT; friendly wing 1-1; Human)\n\nTimeline\n--------\n"
        self.assertEqual(rc.check_summary(text, expect_aircraft=1), [])
        self.assertTrue(rc.check_summary(text, expect_aircraft=2))
        self.assertTrue(rc.check_summary("nothing"))


if __name__ == "__main__":
    unittest.main()


class KilledProbeTests(unittest.TestCase):
    def test_a_partial_length_is_accepted_at_any_speed(self):
        from battery_scenarios import _replay_record as rr  # noqa: E402

        for length in ("0:41.2", "3:07.0", "12:00.0"):
            self.assertTrue(rr.has_length(f"State       INCOMPLETE\nLength      {length} (22440 frames)\n"), length)
        self.assertFalse(rr.has_length("State       INCOMPLETE\nResult      unknown\n"))
        self.assertFalse(rr.has_length("Length      none\n"))


class ShotOutcomeTests(unittest.TestCase):
    @staticmethod
    def log(*results):
        import json

        lines = [{"type": "aircraft", "id": 3}, {"type": "event", "tick": 1, "t": 0.01, "kind": "weapon.launch",
                                                  "subject": 3, "fields": {"projectile": 16777216}}]
        for i, (result, replaces) in enumerate(results):
            fields = {"projectile": 16777216, "result": result}
            if replaces:
                fields["replaces"] = replaces
            lines.append({"type": "event", "tick": 2 + i, "t": 0.02, "kind": "weapon.outcome", "subject": 3, "fields": fields})
        return "\n".join(json.dumps(line) for line in lines)

    def test_one_outcome_or_a_spoof_replaced_by_a_hit_pass(self):
        from battery_scenarios import _replay_record as rr  # noqa: E402

        self.assertEqual(rr.invariant_problems(self.log(("hit", None))), [])
        self.assertEqual(rr.invariant_problems(self.log(("spoofed", None), ("hit", "spoofed"))), [])

    def test_any_other_repeat_fails(self):
        from battery_scenarios import _replay_record as rr  # noqa: E402

        for results in (
            [("spoofed", None), ("hit", None)],
            [("hit", None), ("hit", None)],
            [("missed", None), ("hit", "spoofed")],
            [("hit", "spoofed"), ("spoofed", None)],
            [("spoofed", None), ("hit", "spoofed"), ("hit", "spoofed")],
            [("hit", "spoofed")],
        ):
            problems = rr.invariant_problems(self.log(*results))
            self.assertTrue(any("shot 16777216" in p for p in problems), results)
    def decoy_log(self, *extra):
        import json

        lines = [{"type": "aircraft", "id": 3}, {"type": "aircraft", "id": 0},
                 {"type": "event", "tick": 0, "t": 0.0, "kind": "weapon.launch",
                  "subject": 3, "fields": {"projectile": 16777216}},
                 {"type": "event", "tick": 1, "t": 0.01, "kind": "combat.countermeasure",
                  "subject": 0, "fields": {"decoy": "chaff", "number": 7}}]
        for tick, kind, subject, obj, fields in extra:
            event = {"type": "event", "tick": tick, "t": tick / 100, "kind": kind, "subject": subject,
                     "fields": {"projectile": 16777216, **fields}}
            if obj is not None:
                event["object"] = obj
            lines.append(event)
        return "\n".join(json.dumps(line) for line in lines)

    def test_a_numbered_decoy_must_be_a_device_that_was_released(self):
        from battery_scenarios import _replay_record as rr  # noqa: E402

        late = (4, "weapon.outcome", 3, 0, {
            "result": "hit", "replaces": "spoofed",
            "reason": "it was decoyed by chaff #7 from You at 0:13.5 but flew on and struck anyway"})
        spoof = (3, "weapon.outcome", 3, 0, {"result": "spoofed"})
        good = (2, "weapon.decoyed", 3, 0, {"decoy": "chaff", "number": 7})
        self.assertEqual(rr.invariant_problems(self.decoy_log(good, spoof, late)), [])
        # The same tick lists the decoy before the device that left on it.
        self.assertEqual(rr.invariant_problems(self.decoy_log(
            (1, "weapon.decoyed", 3, 0, {"decoy": "chaff", "number": 7}), spoof)), [])
        # A decoy earlier than the release is wrong.
        problems = rr.invariant_problems(self.decoy_log(
            (0, "weapon.decoyed", 3, 0, {"decoy": "chaff", "number": 7}), spoof))
        self.assertTrue(any("never released" in p for p in problems))
        # An older recording has no numbers: fine.
        old = (4, "weapon.outcome", 3, 0, {
            "result": "hit", "replaces": "spoofed",
            "reason": "it was spoofed but flew on and struck anyway"})
        self.assertEqual(rr.invariant_problems(self.decoy_log(
            (2, "weapon.decoyed", 3, 0, {"decoy": "chaff"}), spoof, old)), [])
        # A number nobody released, a wrong kind, or a wrong releaser fails.
        for bad in (
            (2, "weapon.decoyed", 3, 0, {"decoy": "chaff", "number": 8}),
            (2, "weapon.decoyed", 3, 0, {"decoy": "flare", "number": 7}),
            (2, "weapon.decoyed", 3, 3, {"decoy": "chaff", "number": 7}),
        ):
            problems = rr.invariant_problems(self.decoy_log(bad, spoof))
            self.assertTrue(any("never released" in p for p in problems), bad)
        wrong = (4, "weapon.outcome", 3, 0, {
            "result": "hit", "replaces": "spoofed",
            "reason": "it was decoyed by flare #7 from You but flew on and struck anyway"})
        problems = rr.invariant_problems(self.decoy_log(good, spoof, wrong))
        self.assertTrue(any("late hit names flare #7" in p for p in problems))


class FoxLaunchTests(unittest.TestCase):
    def test_recorded_call_must_match_the_amraam_shooter_and_time(self):
        import json
        import tempfile
        from battery_scenarios._replay_record import fox_three_problems

        with tempfile.TemporaryDirectory() as directory:
            work = Path(directory)
            (work / "log").mkdir()
            rows = [
                {"type": "weapon", "id": 4, "source": "AIM120.JT"},
                {"type": "event", "kind": "weapon.launch", "subject": 0,
                 "t": 3, "fields": {"weapon": 4}},
                {"type": "event", "kind": "comms.radio", "subject": 0,
                 "t": 3.5, "fields": {"outcome": "delivered", "stems": "^FOXONE"}},
            ]

            def check():
                (work / "log" / "log.jsonl").write_text(
                    "\n".join(json.dumps(row) for row in rows))
                return fox_three_problems(work, "")

            self.assertTrue(check(), "the old Fox one bug must fail")
            rows[2]["fields"]["stems"] = "^FOXTHR"
            self.assertEqual(check(), [])
            rows[2]["subject"] = 1
            self.assertTrue(check(), "another shooter's Fox three is not evidence")
            rows[2]["subject"] = 0
            rows[2]["t"] = 10
            self.assertTrue(check(), "an unrelated later call is not evidence")


class UnlockedMissileTests(unittest.TestCase):
    def test_max_range_requires_launch_advisory_and_physical_outcome(self):
        import json
        import tempfile
        from battery_scenarios import _replay_record as rr
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            (work / "log").mkdir()
            log = work / "log" / "log.jsonl"
            log.write_text("")
            self.assertTrue(rr.unlocked_release_problems(work, "readiness=MaximumRange", mode="max-range"))
            rows = [{"kind": "weapon.launch", "subject": 0, "object": 1, "fields": {"projectile": 7}},
                    {"kind": "weapon.outcome", "subject": 0, "fields": {"projectile": 7}}]
            log.write_text("\n".join(json.dumps(r) for r in rows))
            self.assertEqual(rr.unlocked_release_problems(work, "readiness=MaximumRange", mode="max-range"), [])
            self.assertTrue(rr.unlocked_release_problems(work, "readiness=Ready", mode="max-range"))
            rows.append({"kind": "combat.hit", "subject": 0, "object": 0, "fields": {"projectile": 7}})
            log.write_text("\n".join(json.dumps(r) for r in rows))
            self.assertTrue(rr.unlocked_release_problems(work, "readiness=MaximumRange", mode="max-range"))
            rows[-1]["object"] = 1
            log.write_text("\n".join(json.dumps(r) for r in rows))
            self.assertTrue(rr.unlocked_release_problems(work, "readiness=MaximumRange", mode="max-range"))


class UncuedSurfaceSarhTests(unittest.TestCase):
    def test_probe_requires_search_then_guidance(self):
        from battery_scenarios.a2g_missiles import uncued_sarh_problems
        launch = "a2g-run: unsupported release t=0.500 readiness NoTarget\na2g-run: launch t=0.508 id 0 target none mode BORE seeker Search acquired false unguided false rounds 1\n"
        search = "a2g-run: flight t=1.000 id 0 range 20000 altitude 3000 Search target none unguided 0\n"
        lock = "a2g-run: flight t=4.000 id 0 range 17000 altitude 2800 Locked target 0x50000001 unguided 0\n"
        self.assertEqual(uncued_sarh_problems(launch + search + lock), [])
        self.assertTrue(uncued_sarh_problems(launch + search))
        self.assertTrue(uncued_sarh_problems(launch + lock))


class WeaponHoldTests(unittest.TestCase):
    def test_hold_checker_rejects_early_return_and_unstable_reverse(self):
        import tempfile
        from battery_scenarios.replay_views import HOLD_TICKS, hold_problems
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            ppm = lambda value: b"P6\n1 1\n255\n" + bytes([value] * 3)
            for tick in HOLD_TICKS:
                (work / f"replay-{tick}.ppm").write_bytes(ppm(tick % 255))
            (work / "reverse.ppm").write_bytes(ppm(2713 % 255))
            (work / "interrupted.ppm").write_bytes(ppm(0))
            output = "probe F12 stop: tick 2600 eye [0, 0, 0]\n"
            output += "\n".join(f"a2g-run: F12 tick {t} eye {[0, 0, int(t < 2600 or t == 2833)]}" for t in HOLD_TICKS)
            output += f"\nreplay F12 shot: {work}/reverse.ppm eye [0, 0, 0] yaw 0 pitch 0\nreplay F12 shot: {work}/replay-2713.ppm eye [0, 0, 0] yaw 0 pitch 0"
            self.assertEqual(hold_problems(work, output), [])
            early = output.replace("tick 2832 eye [0, 0, 0]", "tick 2832 eye [0, 0, 1]")
            self.assertTrue(any("180 game ticks" in p for p in hold_problems(work, early)))
            (work / "reverse.ppm").write_bytes(ppm(0))
            self.assertTrue(any("reverse" in p for p in hold_problems(work, output)))
