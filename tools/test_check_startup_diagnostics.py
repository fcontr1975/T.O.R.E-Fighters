import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from check_startup_diagnostics import NATIVE_CRASHES, check


class StartupCheckTests(unittest.TestCase):
    def exercise(self, clear_error=False, metadata=True, windows=False, status=None, crash_note=None):
        runs = []

        def fake_run(command, **kwargs):
            env = kwargs["env"]
            self.assertEqual(env["TORE_NO_ERROR_DIALOG"], "1")
            self.assertNotIn("RUST_BACKTRACE", env)
            self.assertIn(" ", env["TORE_DATA_DIR"])
            self.assertTrue(kwargs["timeout"] > 0)
            mode = command[-1].partition("=")[2]
            runs.append(mode)
            logs = Path(env["TORE_LOG_DIR"])
            (logs / f"session-{len(runs)}.log").write_text(
                "version=1 commit=abc target=test Thread: worker Backtrace: test" if metadata else "no identity", encoding="utf-8")
            last_error = Path(env["TORE_DATA_DIR"]) / "last-error.txt"
            native = NATIVE_CRASHES.get(mode)
            if native and windows:
                name = native[1]
                report = (crash_note if crash_note is not None else
                          f"Stage: deliberate\nNative crash: {name}\nFaulting address: 0x1\n"
                          f"Faulting module: C:\\app+0x1\nThread: main (id 1)\nLast GPU step: none\n"
                          f"Session log: {logs}\n")
                last_error.write_text(report, encoding="utf-8")
                (logs / f"session-{len(runs)}.fatal.txt").write_text(report, encoding="utf-8")
                return subprocess.CompletedProcess(command, status if status is not None else native[0], "", "")
            if native:
                last_error.write_text(f"this test runs only on Windows\nSession log: {logs}", encoding="utf-8")
            elif mode:
                last_error.write_text(f"deliberate {mode}\nThread: worker\nBacktrace: test\nSession log: {logs}", encoding="utf-8")
            elif clear_error and last_error.exists():
                last_error.unlink()
            return subprocess.CompletedProcess(command, int(bool(mode)), "", "")

        with tempfile.TemporaryDirectory() as directory:
            with patch("check_startup_diagnostics.subprocess.run", side_effect=fake_run):
                with patch.dict(os.environ, {"RUST_BACKTRACE": "1"}):
                    check(Path("app"), Path(directory), windows)
        return runs

    def test_all_modes_and_waited_success_after_each_failure(self):
        self.assertEqual(self.exercise(), ["", "error", "", "panic", "", "worker-panic", "",
                                           "native-crash", "stack-overflow"])

    def test_windows_native_crashes_are_checked_and_survive_a_good_launch(self):
        self.assertEqual(self.exercise(windows=True), ["", "error", "", "panic", "", "worker-panic", "",
                                                       "native-crash", "", "stack-overflow", ""])

    def test_windows_wrong_exit_status_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "wanted"):
            self.exercise(windows=True, status=1)

    def test_windows_incomplete_crash_report_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "report missing"):
            self.exercise(windows=True, crash_note="Native crash: something\n")

    def test_missing_metadata_rejected(self):
        with self.assertRaisesRegex(RuntimeError, "metadata"):
            self.exercise(metadata=False)

    def test_erased_previous_error_rejected(self):
        with self.assertRaises((RuntimeError, FileNotFoundError)):
            self.exercise(clear_error=True)
