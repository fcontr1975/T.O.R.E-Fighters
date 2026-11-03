"""The battery must reject silent or damaging defeat runs, not merely finish."""
import tempfile
import unittest
from pathlib import Path

from battery import Scenario, judge
from battery_scenarios.flight import check_missile_defeat


class DefeatBatteryTests(unittest.TestCase):
    def check_rows(self, rows, serial=None):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for name, data in [("defeat", rows), ("defeat-serial", rows if serial is None else serial),
                               ("defeat-near", rows), ("defeat-near-serial", rows if serial is None else serial)]:
                (root / name).mkdir()
                (root / name / "runs.csv").write_text("hit,defeat_s,closest_inert_ft,inert_radius_ft,inert_damage\n" + data)
            scenario = Scenario(name="defeat-test", lane="flight", args=[], check_work=check_missile_defeat)
            return judge(scenario, "", 0, False, root)

    def test_requires_an_observed_inert_defeat(self):
        self.assertEqual(self.check_rows("0,12.5,50,100,0\n1,,,0,0\n"), [])
        self.assertTrue(self.check_rows("0,,,0,0\n1,,,0,0\n"))
        self.assertTrue(self.check_rows("1,12.5,50,100,0\n"))

    def test_requires_a_real_near_pass_without_damage(self):
        self.assertTrue(self.check_rows("0,12.5,500,100,0\n"))
        self.assertTrue(self.check_rows("0,12.5,50,100,1\n"))

    def test_rejects_worker_dependent_results(self):
        self.assertTrue(self.check_rows("0,12.5,50,100,0\n", "0,12.6,50,100,0\n"))


if __name__ == "__main__":
    unittest.main()
