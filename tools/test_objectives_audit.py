"""The objective detector accepts improvement but rejects new categories."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from battery_scenarios.surface import objective_audit_problems, objective_cache_kuril_problems
from battery_selection import family_matches


def output(rule=None, count=0):
    lines = []
    for _ in range(count):
        lines.append(f"finding\tLFA\t1\tQLFCARG\t0\t0\t1\t0x58000000:TRUCK.NT\t0\t0\t0\t{rule}")
    lines.append(f"objective-audit: cases 264 findings {count} errors {count}")
    text = "\n".join(lines)
    return text + "\n$ then 1: direct\n" + text


class ObjectiveCeilingTests(unittest.TestCase):
    def test_zero_contract_failures_pass(self):
        self.assertEqual(objective_audit_problems(output()), [])

    def test_increases_and_new_categories_fail(self):
        self.assertTrue(objective_audit_problems(output("vehicle-footprint-slope", 1)))
        for rule in ("vehicle-runway-clearance", "vehicle-object-clearance", "land-on-water"):
            self.assertTrue(objective_audit_problems(output(rule, 1)))

    def test_unlisted_contract_failures_cannot_hide_in_the_total(self):
        text = output().replace("errors 0", "errors 1")
        self.assertTrue(objective_audit_problems(text))

    def test_direct_summary_must_be_complete_and_agree(self):
        text = output()
        self.assertTrue(objective_audit_problems(text.rsplit("objective-audit:", 1)[0]))
        self.assertTrue(objective_audit_problems(text.rsplit("errors 0", 1)[0] + "errors 1"))

    def test_missing_or_different_direct_run_fails(self):
        self.assertTrue(objective_audit_problems(""))
        self.assertTrue(objective_audit_problems(output() + "\nfinding\tdifferent"))


class KurilCacheTests(unittest.TestCase):
    def fixture(self):
        return (
            "objective-case: KURILE QKSILO separation 5 aaa 0 sam 2 seed 2 units 38 parked 0 findings 17 digest 0x111\n"
            "objective-case: KURILE QKSILO separation 50 aaa 3 sam 0 seed 3 units 50 parked 0 findings 26 digest 0x222\n"
            "objective-audit: cases 4224 findings 1 errors 0\n"
            "$ then 1: direct\nsurface: digest 0x111\n"
            "$ then 2: direct\nsurface: digest 0x222\n"
        )

    def test_both_final_front_outcomes_match_independent_builds(self):
        self.assertEqual(objective_cache_kuril_problems(self.fixture()), [])

    def test_stale_front_digest_fails(self):
        self.assertTrue(objective_cache_kuril_problems(self.fixture().replace("surface: digest 0x222", "surface: digest 0x333")))

    def test_incomplete_matrix_or_missing_direct_build_fails(self):
        self.assertTrue(objective_cache_kuril_problems(self.fixture().replace("cases 4224", "cases 4200")))
        self.assertTrue(objective_cache_kuril_problems(self.fixture().rsplit("$ then", 1)[0]))

    def test_new_scenario_is_in_surface_family(self):
        self.assertTrue(family_matches("surface", "surface-objectives-cache-kuril"))
