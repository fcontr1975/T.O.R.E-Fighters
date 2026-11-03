"""Synthetic checks for family-constrained offline fitting."""
import copy
import math
import csv
import io
import unittest

from missile_energy_fit import CELLS, select, read_candidates


def row(name, reach, score=0.):
    return dict(source=name, score=score, fraction=.3, drag=1.,
                control_s=70., anchor_km=reach, tail_target_km=reach,
                min_launch_km=.5, floor_km=1.5,
                **{"class": "MediumAir"}, **dict.fromkeys(CELLS, reach))


def candidates():
    levels = {"AIM7.JT": 12., "AIM7E.JT": 10., "AA10.JT": 11., "R27ER.JT": 13.,
              "R27T.JT": 9., "R27ET.JT": 10., "R530.JT": 10.,
              "AIM120.JT": 14., "AA12.JT": 14., "MICA.JT": 14.,
              "AIM9M.JT": 5., "AIM9X.JT": 5., "AA11.JT": 5., "AA11B.JT": 5.,
              "AIM9B.JT": 3., "AA2.JT": 3., "AA8.JT": 3.}
    return [row(name, reach) for name, reach in levels.items()]


class FamilyFitTests(unittest.TestCase):
    def test_runner_command_with_comma_filter_is_not_a_data_row(self):
        candidate = row("AIM120.JT", 14.)
        stream = io.StringIO()
        writer = csv.DictWriter(stream, fieldnames=["source", "class"] +
                                [key for key in candidate if key not in {"source", "class"}])
        writer.writeheader()
        writer.writerow(candidate)
        text = ("['probe', '--fit-motors', 'AA10.JT,AA12.JT,AGM84E.JT']\n" +
                stream.getvalue() + "motor fits AIM120.JT\n")
        self.assertEqual(read_candidates(text), [candidate])

    def test_role_constraint_rejects_cheaper_but_reversed_fit(self):
        rows = candidates()
        rows.append(row("AIM120.JT", 8., -100.))
        result = select(rows)
        self.assertEqual(result["fits"]["AIM120.JT"]["r5t"], 14.)
        self.assertGreaterEqual(result["margins"]["AIM120.JT/AIM7.JT:5k"], 0.)
        self.assertGreater(result["margins"]["AIM7.JT/AIM7E.JT"], 0.)

    def test_equal_or_zero_family_cells_are_not_upgrade_wins(self):
        for reach in (0., 10.):
            rows = candidates()
            next(r for r in rows if r["source"] == "AIM7.JT")["r500b"] = reach
            with self.assertRaises(ValueError):
                select(rows)

    def test_impossible_role_ordering_is_reported(self):
        rows = candidates()
        next(r for r in rows if r["source"] == "R530.JT").update(dict.fromkeys(CELLS, 20.))
        with self.assertRaisesRegex(ValueError, "active-radar"):
            select(rows)

    def test_invalid_numbers_and_bounds_are_rejected(self):
        for key, value in (("score", math.nan), ("fraction", .51), ("drag", 3.1)):
            rows = copy.deepcopy(candidates())
            rows[0][key] = value
            with self.assertRaises(ValueError):
                select(rows)

    def test_alias_fit_is_explicit_and_order_does_not_change_selection(self):
        first = select(candidates())
        second = select(list(reversed(candidates())))
        self.assertEqual(first["fits"], second["fits"])
        self.assertEqual(first["margins"], second["margins"])
        self.assertEqual(first["fits"]["AA11.JT"]["fraction"],
                         first["fits"]["AA11B.JT"]["fraction"])


if __name__ == "__main__":
    unittest.main()
