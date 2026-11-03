#!/usr/bin/env python3
"""Select fixed-budget two-condition game fits jointly. See docs/formats/missile-energy-data.md."""
import argparse
import csv
import itertools
import math
import re
import json
from pathlib import Path

CELLS = [f"r{height}{aspect}" for height in ("500", "5", "15", "35") for aspect in "hbt"]
TAILS = ["r5t", "r15t", "r35t"]
FAMILIES = [("AIM7.JT", "AIM7E.JT"), ("R27ER.JT", "AA10.JT"), ("R27ET.JT", "R27T.JT")]
MODERN = ["AIM9M.JT", "AIM9X.JT", "AA11.JT"]
EARLY = ["AIM9B.JT", "AA2.JT", "AA8.JT"]


def beats(newer, older, cells):
    """Require a positive, strictly ordered envelope with 10 m search margin."""
    return all(older[key] > 0 and newer[key] > older[key] + 0.01 for key in cells)


def select(rows):
    by_store = {}
    for row in rows:
        if not all(math.isfinite(value) for key, value in row.items()
                   if key not in {"source", "class"}):
            raise ValueError(f"Non-finite candidate: {row['source']}")
        if not 0.12 <= row["fraction"] <= 0.5 or not 0.5 <= row["drag"] <= 3:
            raise ValueError(f"Out-of-bounds candidate: {row['source']}")
        if row["class"] in {"ShortIr", "MediumAir", "LongAir"} and (
                any(row[key] <= 0 for key in CELLS) or row["r5t"] < row["floor_km"]):
            continue
        # Retain the established finite deck escape regression for modern IR.
        if row["source"] in MODERN and row["r500t"] >= 11.99:
            continue
        by_store.setdefault(row["source"], []).append(row)
    required = {name for pair in FAMILIES for name in pair} | set(MODERN + EARLY) | {
        "AA11B.JT", "AIM120.JT", "AA12.JT", "MICA.JT", "R530.JT"}
    missing = required - by_store.keys()
    if missing:
        raise ValueError(f"Missing candidates: {sorted(missing)}")
    best = {name: min(options, key=lambda row: row["score"])
            for name, options in by_store.items()}
    margins = {}

    def family_options(newer, older, key_store):
        result = []
        for key in by_store[key_store]:
            peers = by_store[older if key_store == newer else newer]
            pairs = [(key, peer) if key_store == newer else (peer, key)
                     for peer in peers]
            feasible = [(a, b) for a, b in pairs if beats(a, b, CELLS)]
            if not feasible:
                continue
            def cost(pair):
                a, b = pair
                if newer == "AIM7.JT":
                    # Equal priority to the two contradictory historical tails.
                    tail = max(abs(a["r5t"] / a["tail_target_km"] - 1),
                               abs(b["r5t"] / b["tail_target_km"] - 1))
                    head = abs(a["r35h"] / a["anchor_km"] - 1) + abs(b["r35h"] / b["anchor_km"] - 1)
                    return 8 * tail + 4 * head
                return a["score"] + b["score"]
            pair = min(feasible, key=cost)
            result.append((cost(pair), {newer: pair[0], older: pair[1]}, key))
        if not result:
            raise ValueError(f"No feasible family fit: {newer}/{older}")
        return result

    # The newer Sparrow already dominates its older family member, so its
    # H5/T5 values bound both. ER is constrained against AA10 but is not one
    # of the older stores in John's cross-role comparison.
    groups = [
        family_options("AIM7.JT", "AIM7E.JT", "AIM7.JT"),
        family_options("R27ER.JT", "AA10.JT", "AA10.JT"),
        [(row["score"], {"R530.JT": row}, row) for row in by_store["R530.JT"]],
    ]
    # Only group cost and its required H5/T5 envelope affect other groups.
    # Remove a group option when a cheaper one requires no more active reach.
    groups = [[option for option in group if not any(
        other[0] <= option[0] and other[2]["r5h"] <= option[2]["r5h"]
        and other[2]["r5t"] <= option[2]["r5t"]
        and (other[0], other[2]["r5h"], other[2]["r5t"]) !=
            (option[0], option[2]["r5h"], option[2]["r5t"])
        for other in group)] for group in groups]
    active = ["AIM120.JT", "AA12.JT", "MICA.JT"]
    cache = {}
    joint = None
    for combo in itertools.product(*groups):
        h = max(option[2]["r5h"] for option in combo)
        t = max(option[2]["r5t"] for option in combo)
        bounds = (h, t)
        if bounds not in cache:
            choices = []
            for name in active:
                eligible = [row for row in by_store[name]
                            if row["r5h"] >= h + 0.02 and row["r5t"] >= t + 0.01]
                if not eligible:
                    break
                choices.append(min(eligible, key=lambda row: row["score"]))
            cache[bounds] = choices if len(choices) == len(active) else None
        choices = cache[bounds]
        if choices is None:
            continue
        score = sum(option[0] for option in combo) + sum(row["score"] for row in choices)
        if joint is None or score < joint[0]:
            joint = (score, combo, choices)
    if joint is None:
        raise ValueError("No feasible active-radar versus older-radar fit")
    for _, chosen, _ in joint[1]:
        best.update(chosen)
    best.update(zip(active, joint[2]))
    infrared_family = family_options("R27ET.JT", "R27T.JT", "R27ET.JT")
    best.update(min(infrared_family, key=lambda option: option[0])[1])
    for newer, older in FAMILIES:
        margins[f"{newer}/{older}"] = min(best[newer][key] / best[older][key] - 1 for key in CELLS)
    for newer, older in itertools.product(active, ["AIM7E.JT", "AIM7.JT", "AA10.JT", "R530.JT"]):
        margins[f"{newer}/{older}:5k"] = min(best[newer][key] / best[older][key] - 1 for key in ["r5h", "r5t"])

    # Compress modern candidates with identical eligibility sets. Bit-set
    # intersection replaces repeatedly rescanning every older candidate.
    early_rows = {name: sorted(by_store[name], key=lambda row: row["score"]) for name in EARLY}
    modern_rows = []
    for name in MODERN:
        unique = {}
        for row in by_store[name]:
            masks = tuple(sum(1 << i for i, old in enumerate(early_rows[older])
                              if beats(row, old, TAILS)) for older in EARLY)
            if not all(masks):
                continue
            if masks not in unique or row["score"] < unique[masks]["score"]:
                unique[masks] = row
        modern_rows.append([(row, masks) for masks, row in unique.items()])
    choice = None
    for modern in itertools.product(*modern_rows):
        early = []
        for i, name in enumerate(EARLY):
            mask = modern[0][1][i] & modern[1][1][i] & modern[2][1][i]
            if not mask:
                break
            early.append(early_rows[name][(mask & -mask).bit_length() - 1])
        if len(early) != len(EARLY):
            continue
        selected = [entry[0] for entry in modern]
        score = sum(row["score"] for row in selected + early)
        if choice is None or score < choice[0]:
            choice = (score, selected, early)
    if choice is None:
        raise ValueError("No feasible short-IR joint fit")
    _, modern, early = choice
    best.update(zip(MODERN + EARLY, modern + early))
    for newer, older in itertools.product(MODERN, EARLY):
        margins[f"{newer}/{older}"] = min(
            best[newer][key] / best[older][key] - 1 for key in TAILS)
    # Fictional R73 class shares the exact physical fit.
    best["AA11B.JT"] = next(
        row for row in by_store["AA11B.JT"]
        if all(abs(row.get(key, 0) - best["AA11.JT"].get(key, 0)) < 1e-6
               for key in ["drag", "fraction", "boost_s", "sustain_s", "exhaust_mps", "boost_share"]))
    return {"fits": best, "margins": margins}



def read_candidates(text):
    """Read CSV rows from either plain output or the throttled runner's log."""
    lines = text.splitlines()
    header = next(line for line in lines if line.startswith("source,class,"))
    rows = list(csv.DictReader([header] + [
        line for line in lines if re.match(r"^[A-Z0-9_]+\.JT,", line)]))
    for row in rows:
        for key in row.keys() - {"source", "class"}:
            row[key] = float(row[key])
    return rows


def main():
    parser = argparse.ArgumentParser(
        description="Select family-constrained game fits from --fit-class CSV/log. "
                    "Writes evidence JSON, never runtime data or source.")
    parser.add_argument("candidates", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    rows = read_candidates(args.candidates.read_text())
    result = select(rows)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Selected {len(result['fits'])} fits; family margins: {result['margins']}")


if __name__ == "__main__":
    main()
