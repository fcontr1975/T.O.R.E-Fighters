import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import battery  # noqa: E402
from battery import DriveError  # noqa: E402


def scenario(**kw):
    return battery.Scenario(name="t", lane="ai", args=[], **kw)


class ProfileTests(unittest.TestCase):
    def test_inherited_recordings_never_enter_a_scenario(self):
        from unittest.mock import patch

        for fallback in (False, True):
            with self.subTest(fallback=fallback), tempfile.TemporaryDirectory() as folder:
                source = Path(folder) / "source"
                dest = Path(folder) / "work" / "data"
                (source / "replays").mkdir(parents=True)
                (source / "replays" / "old.tore-replay").write_text("finished")
                (source / "replays" / "old.partial").write_text("unfinished")
                (source / "import").mkdir()
                (source / "import" / "fixture").write_text("synthetic import")
                (source / "settings.conf").write_text("settings")
                if fallback:
                    with patch.object(battery.subprocess, "run", side_effect=OSError):
                        battery.clone_profile(source, dest)
                else:
                    battery.clone_profile(source, dest)
                self.assertFalse((dest / "replays").exists())
                self.assertEqual((dest / "import" / "fixture").read_text(), "synthetic import")
                self.assertEqual((dest / "settings.conf").read_text(), "settings")
                self.assertEqual((source / "replays" / "old.tore-replay").read_text(), "finished")
                self.assertEqual((source / "replays" / "old.partial").read_text(), "unfinished")


class JudgeTests(unittest.TestCase):
    def judge(self, s, output, code=0, timed_out=False):
        with tempfile.TemporaryDirectory() as d:
            return battery.judge(s, output, code, timed_out, Path(d))

    def test_clean_run_passes(self):
        self.assertEqual(self.judge(scenario(), "all fine\ninfo: done\n"), [])

    def test_exit_code_and_timeout(self):
        self.assertTrue(self.judge(scenario(), "", code=101))
        self.assertIn("timed out", self.judge(scenario(timeout=5), "", code=None, timed_out=True)[0])

    def test_detector_can_accept_either_reported_or_clean_exit(self):
        for code in (0, 1):
            self.assertEqual(self.judge(scenario(expect_exit=None), "", code=code), [])
        self.assertTrue(self.judge(scenario(expect_exit=None), "", code=None, timed_out=True))

    def test_generic_problems(self):
        self.assertTrue(self.judge(scenario(), "thread 'main' panicked at src/x.rs:1"))
        self.assertTrue(self.judge(scenario(), "speed=NaN"))
        self.assertTrue(self.judge(scenario(), "x=inf y=1"))
        self.assertEqual(self.judge(scenario(allow_generic=["NaN in output"]), "speed=NaN"), [])

    def test_expect_forbid_check(self):
        self.assertTrue(self.judge(scenario(expect=[r"totals"]), "nothing"))
        self.assertEqual(self.judge(scenario(expect=[r"totals"]), "totals: 1"), [])
        self.assertTrue(self.judge(scenario(forbid=[r"dropped=[1-9]"]), "dropped=3"))
        self.assertEqual(self.judge(scenario(check=lambda o: ["custom"]), "x"), ["custom"])

    def test_check_work_sees_the_work_folder_and_output(self):
        with tempfile.TemporaryDirectory() as d:
            (Path(d) / "made.txt").write_text("x")
            s = scenario(check_work=lambda work, out: [] if (work / "made.txt").exists() and "ok" in out else ["bad"])
            self.assertEqual(battery.judge(s, "ok", 0, False, Path(d)), [])
            self.assertEqual(battery.judge(s, "no", 0, False, Path(d)), ["bad"])

    def test_follow_up_steps_append_their_output_and_check_exit_codes(self):
        import argparse

        with tempfile.TemporaryDirectory() as d:
            # The running Python stands in for the game and for other commands, so this runs on every platform.
            opts = argparse.Namespace(bin=sys.executable, timeout_scale=1.0)
            fail = [sys.executable, "-c", "raise SystemExit(1)"]
            steps = [
                battery.Step(["-c", "print('one')"]),
                battery.Step(fail, app=False, expect_exit=1),
                battery.Step(fail, app=False, expect_exit=None),
                battery.Step(fail, app=False),
            ]
            problems: list[str] = []
            out = battery.run_steps(scenario(then=steps), opts, {}, Path(d), "main", problems)
            self.assertIn(f"$ then 1: {sys.executable} -c print('one')", out)
            self.assertIn("one", out)
            self.assertEqual(problems, ["step 4 exit code 1, expected 0"])

    def test_env_values_can_name_the_work_folder_and_known_failures_are_flagged(self):
        import argparse
        import shutil

        with tempfile.TemporaryDirectory() as d:
            profile = Path(d) / "profile"
            profile.mkdir()
            opts = argparse.Namespace(
                bin=sys.executable, profile=str(profile), keep_data=True, timeout_scale=1.0, out=d,
            )
            run_dir = Path(d) / "run"
            slots = __import__("threading").Semaphore(1)
            sc = battery.Scenario(
                name="t", lane="ai", args=["-c", "import os; print(os.environ['MARK']); raise SystemExit(3)"], env={"MARK": "{work}/x"}, expect_exit=0,
                known_failure="on purpose",
            )
            result = battery.run_one(sc, opts, run_dir, slots)
            self.assertTrue(result.ok)
            self.assertIn("known failure (on purpose)", result.problems[0])
            log = (run_dir / result.log).read_text()
            self.assertIn(str(run_dir / "work" / "t") + "/x", log)
            sc2 = battery.Scenario(name="t2", lane="ai", args=["-c", "pass"], known_failure="on purpose")
            result2 = battery.run_one(sc2, opts, run_dir, slots)
            self.assertFalse(result2.ok)
            self.assertIn("now passes", result2.problems[0])
            shutil.rmtree(run_dir, ignore_errors=True)

    def test_scenario_names_are_unique_and_lanes_valid(self):
        names = [s.name for s in battery.load_scenarios()]
        self.assertEqual(len(names), len(set(names)))


PY = sys.executable


def drive_opts(directory, scale=1.0):
    import argparse

    profile = Path(directory) / "profile"
    profile.mkdir(exist_ok=True)
    return argparse.Namespace(
        bin=PY, server_bin=PY, bot_bin=PY, profile=str(profile), keep_data=True, timeout_scale=scale, out=directory,
    )


def run_driver_scenario(driver, timeout=30, scale=1.0, **kw):
    import threading

    with tempfile.TemporaryDirectory() as d:
        opts = drive_opts(d, scale)
        sc = battery.Scenario(name="drv", lane="net", args=[], driver=driver, timeout=timeout, **kw)
        run_dir = Path(d) / "run"
        result = battery.run_one(sc, opts, run_dir, threading.Semaphore(1))
        return result, (run_dir / result.log).read_text()


class DriverTests(unittest.TestCase):
    """Scenarios that own several processes (the net lane); the running Python stands in for the programs."""

    def test_output_is_labelled_and_checked_like_a_single_run(self):
        def driver(d):
            one = d.start("one", [d.app, "-c", "print('hello from one')"])
            two = d.start("two", [d.server, "-c", "print('hello from two')"])
            one.finish(20)
            two.finish(20)
            one.expect(r"^hello from one$")
            two.forbid(r"one")

        result, log = run_driver_scenario(driver, expect=[r"^\[two\] hello from two$"], forbid=[r"NaN"])
        self.assertTrue(result.ok, result.problems)
        self.assertIn("[one] hello from one", log)
        self.assertIn("[driver] start one:", log)
        self.assertEqual(result.exit_code, 0)

    def test_the_generic_checks_see_every_process(self):
        def driver(d):
            d.start("a", [d.app, "-c", "print('speed=NaN')"]).finish(20)

        result, _ = run_driver_scenario(driver)
        self.assertFalse(result.ok)
        self.assertTrue(any("NaN" in p for p in result.problems), result.problems)

    def test_exit_codes_expectations_and_problems_fail_the_scenario(self):
        def driver(d):
            d.start("bad", [d.app, "-c", "raise SystemExit(3)"]).finish(20)
            d.start("ok", [d.app, "-c", "raise SystemExit(3)"]).finish(20, expect_exit=3)
            d.start("any", [d.app, "-c", "raise SystemExit(9)"]).finish(20, expect_exit=None)
            d.start("quiet", [d.app, "-c", "print('x')"]).expect(r"never printed")

        result, _ = run_driver_scenario(driver)
        self.assertFalse(result.ok)
        self.assertIn("bad exit code 3, expected 0", result.problems)
        self.assertTrue(any(p.startswith("quiet: missing") for p in result.problems), result.problems)
        self.assertEqual(len([p for p in result.problems if p.startswith(("ok ", "any "))]), 0)

    def test_the_console_and_waits(self):
        code = "import sys\nprint('ready', flush=True)\nfor line in sys.stdin:\n    print('got', line.strip(), flush=True)\n    if line.strip() == 'quit': break\n"

        def driver(d):
            server = d.start("server", [d.server, "-u", "-c", code], stdin=True)
            if not server.wait_for(r"^ready$", 20):
                raise DriveError("no ready")
            server.send("hello")
            self.assertTrue(server.wait_for(r"^got hello$", 20))
            self.assertFalse(server.wait_for(r"never", 0.2))
            server.send("quit")
            self.assertEqual(server.finish(20), 0)

        result, _ = run_driver_scenario(driver)
        self.assertTrue(result.ok, result.problems)

    def test_a_process_left_running_is_a_problem_and_is_stopped(self):
        procs = []

        def driver(d):
            proc = d.start("stray", [d.app, "-c", "import time; time.sleep(60)"])
            procs.append(proc.popen)
            d.sleep(0.2)

        result, _ = run_driver_scenario(driver)
        self.assertFalse(result.ok)
        self.assertIn("stray was still running when the driver finished", result.problems)
        self.assert_gone(procs[0])

    def test_a_process_the_driver_stopped_is_not_a_problem(self):
        def driver(d):
            proc = d.start("stray", [d.app, "-c", "import time; time.sleep(60)"])
            d.sleep(0.2)
            proc.stop(grace=5)

        result, _ = run_driver_scenario(driver)
        self.assertTrue(result.ok, result.problems)

    def test_a_process_that_will_not_exit_is_stopped_and_reported(self):
        def driver(d):
            self.assertIsNone(d.start("slow", [d.app, "-c", "import time; time.sleep(60)"]).finish(0.3))

        result, _ = run_driver_scenario(driver)
        self.assertIn("slow did not exit within 0s", result.problems)

    def test_the_timeout_stops_every_process_and_the_driver(self):
        import time

        procs = []

        def driver(d):
            procs.append(d.start("a", [d.app, "-c", "import time; time.sleep(60)"]).popen)
            procs.append(d.start("b", [d.app, "-c", "import time; time.sleep(60)"]).popen)
            d.sleep(60)

        started = time.time()
        result, _ = run_driver_scenario(driver, timeout=1)
        self.assertLess(time.time() - started, 20)
        self.assertFalse(result.ok)
        self.assertIsNone(result.exit_code)
        self.assertTrue(any("timed out" in p for p in result.problems), result.problems)
        for popen in procs:
            self.assert_gone(popen)

    def test_a_driver_that_fails_or_gives_up_is_reported(self):
        def broken(d):
            raise ValueError("bad regex")

        def gives_up(d):
            raise DriveError("the server never came up")

        result, log = run_driver_scenario(broken)
        self.assertFalse(result.ok)
        self.assertTrue(any("the driver failed: ValueError: bad regex" in p for p in result.problems), result.problems)
        self.assertIn("Traceback", log)
        result, _ = run_driver_scenario(gives_up)
        self.assertIn("the server never came up", result.problems)

    def test_a_window_needs_a_windowed_scenario(self):
        def driver(d):
            d.start("w", [d.app, "-c", "pass"], window=True)

        result, _ = run_driver_scenario(driver)
        self.assertTrue(any("must set window=True" in p for p in result.problems), result.problems)

    def test_the_work_folder_and_the_data_folder_are_given(self):
        def driver(d):
            self.assertTrue(d.data.is_dir())
            self.assertEqual(d.data.parent, d.work)
            proc = d.start("env", [d.app, "-c", "import os; print(os.environ['TORE_DATA_DIR'])"])
            proc.finish(20)
            self.assertIn(str(d.data), proc.text())

        result, _ = run_driver_scenario(driver)
        self.assertTrue(result.ok, result.problems)

    def test_ports_are_free_and_never_handed_out_twice(self):
        import socket

        ports = [battery.free_port() for _ in range(25)]
        self.assertEqual(len(set(ports)), 25)
        for port in ports[:5]:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
                s.bind(("127.0.0.1", port))

    def test_missing_server_and_bot_programs_are_named(self):
        import argparse

        sc = battery.Scenario(name="a", lane="net", args=[], driver=lambda d: None, uses=("server", "bot"))
        plain = battery.Scenario(name="b", lane="ai", args=[])
        opts = argparse.Namespace(server_bin="/nonexistent/tore-server", bot_bin=PY)
        self.assertEqual(battery.missing_binaries([sc, plain], opts), ["/nonexistent/tore-server"])
        self.assertEqual(battery.missing_binaries([plain], opts), [])

    def test_a_driver_that_calls_itself_inconclusive_runs_once_more(self):
        calls = []

        def driver(d):
            calls.append(1)
            if len(calls) == 1:
                d.problem("the second kill never came")
                d.inconclusive_because("the server ran slowly")

        result, log = run_driver_scenario(driver)
        self.assertEqual(len(calls), 2)
        self.assertTrue(result.ok, result.problems)
        self.assertIn("RETRY: the first run was inconclusive (the server ran slowly)", log)

    def test_two_inconclusive_runs_are_reported_as_inconclusive_not_as_a_plain_failure(self):
        calls = []

        def driver(d):
            calls.append(1)
            d.problem("the second kill never came")
            d.inconclusive_because("the server ran slowly")

        result, _ = run_driver_scenario(driver)
        self.assertEqual(len(calls), 2)
        self.assertFalse(result.ok)
        self.assertTrue(result.problems[0].startswith("inconclusive: the server ran slowly"), result.problems)
        self.assertIn("the second kill never came", result.problems)

    def test_a_driver_that_fails_without_saying_so_is_not_run_again(self):
        calls = []

        def driver(d):
            calls.append(1)
            d.problem("a real fault")

        result, _ = run_driver_scenario(driver)
        self.assertEqual(len(calls), 1)
        self.assertEqual(result.problems, ["a real fault"])

    def assert_gone(self, popen):
        import time

        # Polling collects an exited child, so this works the same on Windows.
        for _ in range(100):
            if popen.poll() is not None:
                return
            time.sleep(0.05)
        self.fail(f"process {popen.pid} is still running")


if __name__ == "__main__":
    unittest.main()
