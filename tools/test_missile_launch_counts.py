"""Exercise the actual battery driver with synthetic natural-fight output."""
import os
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from battery_scenarios import ai


class LaunchCountTests(unittest.TestCase):
    def check_driver(self, counts=None, refused=0, starved=0):
        counts = counts or dict(UKR=15, BAL=15, SPA=15, GRE=15, big=30)
        problems = []
        calls = []
        with TemporaryDirectory() as directory:
            root = Path(directory)
            executable = root / "target/release/examples" / (
                "missile_energy_cost.exe" if os.name == "nt" else "missile_energy_cost")
            executable.parent.mkdir(parents=True)
            executable.touch()
            (root / "menu-synthetic.pack").touch()

            def run(name, argv, timeout):
                calls.append((name, argv, timeout))
                size = 23 if name == "big" else 8
                text = (f"guided_launches={counts[name]} ai={size} max_clients=40 "
                        f"refused={refused} starved={starved}")
                return SimpleNamespace(text=lambda: text)

            driver = SimpleNamespace(data=root, run=run, problem=problems.append)
            with patch.object(ai, "ROOT", root):
                ai.prediction_launch_regression(driver)
        self.assertEqual([call[0] for call in calls], ["UKR", "BAL", "SPA", "GRE", "big"])
        self.assertIn("--big", calls[-1][1])
        return problems

    def test_balance_increase_uses_phase25_instead_of_the_old_symmetric_ceiling(self):
        self.assertEqual(self.check_driver(), [])

    def test_each_fight_keeps_the_pre_overhaul_floor(self):
        for name, below_floor in [("UKR", 7), ("BAL", 8), ("SPA", 8), ("GRE", 8), ("big", 16)]:
            with self.subTest(name=name):
                counts = dict(UKR=15, BAL=15, SPA=15, GRE=15, big=30)
                counts[name] = below_floor
                self.assertTrue(any(f"{name}:" in p and "pre-overhaul Spec" in p
                                    for p in self.check_driver(counts)))

    def test_both_phase25_bounds_still_apply(self):
        for name in ["UKR", "BAL", "SPA", "GRE", "big"]:
            for value in ([18, 34] if name == "big" else [9, 17]):
                with self.subTest(name=name, count=value):
                    counts = dict(UKR=15, BAL=15, SPA=15, GRE=15, big=30)
                    counts[name] = value
                    problems = self.check_driver(counts)
                    self.assertEqual(len(problems), 1)
                    self.assertIn("Phase 2.5", problems[0])

    def test_refused_or_starved_requests_fail_even_with_valid_counts(self):
        for refused, starved in [(1, 0), (0, 1)]:
            problems = self.check_driver(refused=refused, starved=starved)
            self.assertEqual(len(problems), 5)
            self.assertTrue(all("admission failed" in p for p in problems))


if __name__ == "__main__":
    unittest.main()
