"""Missile defense regressions flown through the real AI and flight models."""
from __future__ import annotations

import csv
import re
from pathlib import Path

from battery import Scenario, Step

TRACE = re.compile(r"^missile-study: trace .*? g=(-?[\d.]+).*?activity=Defending defense=Some\(\(Some\(MotionSuggestion \{ maneuver: (\w+), heading_deg: (-?[\d.e+]+)", re.M)


def defense_problems(work: Path, output: str, *, minimum_turn: float = 20) -> list[str]:
    problems = []
    samples = TRACE.findall(output)
    if len(samples) < 4:
        problems.append("too few defensive flight samples")
    if any(float(g) <= 0 for g, _, _ in samples):
        problems.append("defense flew non-positive G")
    previous = None
    last_time = None
    for line in output.splitlines():
        if not line.startswith("missile-study: trace "):
            continue
        sample = TRACE.search(line)
        time = re.search(r" t=([\d.]+)", line)
        if time and last_time is not None and float(time[1]) < last_time:
            previous = None
        if time:
            last_time = float(time[1])
        if not sample:
            continue
        _, phase, heading = sample.groups()
        if previous:
            prior_phase, prior_heading = previous
            delta = abs((float(heading) - prior_heading + 180) % 360 - 180)
            if prior_phase == phase and phase in ("Beam", "Notch") and delta > 120:
                problems.append("beam/notch side flipped")
        previous = phase, float(heading)
    runs = work / "study" / "runs.csv"
    if runs.exists():
        with runs.open() as source:
            rows = list(csv.DictReader(source))
        if not rows or any(row["fired"] != "1" for row in rows):
            problems.append("study did not fire every shot")
        if any(float(row["target_turn_deg"]) < minimum_turn for row in rows):
            problems.append(f"defensive heading change below {minimum_turn:g} degrees")
        if any(int(row["other_devices"]) for row in rows):
            problems.append("released wrong-class devices")
    else:
        problems.append("missing study runs.csv")
    for line in output.splitlines():
        if "activity=Defending" not in line:
            continue
        bank = re.search(r" bank=(-?[\d.]+)", line)
        if bank and abs(float(bank[1])) > 90.0:
            problems.append("defensive bank exceeded 90 degrees")
            break
    if re.search(r"^missile-study: device .*defense=Some\(\(.*true\)\)$", output, re.M):
        problems.append("device released after the threat passed")
    if runs.exists() and any(int(row["devices"]) + int(row["other_devices"]) > 8 for row in rows):
        problems.append("single-threat defense exceeded its eight-device budget")
    return problems


def head_break_problems(work: Path, output: str) -> list[str]:
    problems = defense_problems(work, output, minimum_turn=5)
    first = re.search(r"^missile-study: trace .*? yaw=(-?[\d.]+).*?activity=Defending defense=Some\(\(Some\(MotionSuggestion \{ maneuver: Break, heading_deg: (-?[\d.e+]+)", output, re.M)
    if first is None:
        problems.append("no head-on last-ditch break observed")
    elif abs((float(first[2]) - float(first[1]) + 180) % 360 - 180) < 75:
        problems.append("head-on break straightened toward the missile")
    return problems


def chaff_timing_problems(work: Path, output: str) -> list[str]:
    problems = defense_problems(work, output, minimum_turn=5)
    runs = work / "study/runs.csv"
    if not runs.exists():
        return problems
    with runs.open() as source:
        rows = list(csv.DictReader(source))
    if not rows or not rows[0]["first_device_s"]:
        return problems + ["no chaff release observed"]
    first_device = float(rows[0]["first_device_s"])
    samples = []
    for line in output.splitlines():
        match = re.search(r" t=([\d.]+).*maneuver: (\w+)", line)
        if match:
            samples.append((float(match[1]), match[2]))
    after_release = next((phase for at, phase in samples if first_device <= at <= first_device + 0.26), None)
    if after_release != "Notch":
        problems.append("first chaff waited for the last-ditch break")
    if any("reduce_heat: true" in line and float(at[1]) >= first_device
           for line in output.splitlines() if (at := re.search(r" t=([\d.]+)", line))):
        problems.append("known radar threat requested an IR heat cut")
    return problems


def skill_pull_problems(skill: str):
    def check(work: Path, output: str) -> list[str]:
        problems = defense_problems(work, output, minimum_turn=5)
        rows = []
        for line in output.splitlines():
            match = re.search(r" t=([\d.]+).* g=(-?[\d.]+).*activity=Defending.*maneuver: (Beam|Notch|Break),", line)
            if match:
                rows.append((float(match[1]), float(match[2])))
        minimum = {"novice": 3.4, "average": 3.6, "experienced": 4.9, "ace": 4.9}[skill]
        if not rows or max(g for t, g in rows if t <= rows[0][0] + 1.25) < minimum:
            problems.append(f"{skill} did not build {minimum:g} G within 1.25 s of turn entry")
        return problems
    return check


def radar_memory_problems(work: Path, output: str) -> list[str]:
    problems = defense_problems(work, output, minimum_turn=0)
    shots = []
    last = float("inf")
    for line in output.splitlines():
        time = re.search(r"^missile-study: trace .* t=([\d.]+)", line)
        if not time:
            continue
        at = float(time[1])
        if at < last:
            shots.append([])
        last = at
        maneuver = re.search(r"maneuver: (\w+), heading_deg: (-?[\d.e+]+)", line)
        shots[-1].append((maneuver[1], float(maneuver[2])) if maneuver else (None, None))
    for rows in shots:
        indices = [i for i, row in enumerate(rows) if row[0] == "Break"]
        if not indices:
            continue
        if any(rows[i][0] != "Break" for i in range(indices[0], indices[-1])):
            problems.append("radar break dropped and restarted")
        heading = rows[indices[0]][1]
        if any(abs((rows[i][1] - heading + 180) % 360 - 180) > 1 for i in indices):
            problems.append("committed radar break heading changed")
    return problems


def inert_defense_problems(_work: Path, output: str) -> list[str]:
    ended = set(re.findall(r"^missile-study: inert-defense (\S+) .* active_before=true cleared=true$", output, re.M))
    return [f"{store}: no active defense ended on its inert tick"
            for store in ("AIM7.JT", "AIM9M.JT") if store not in ended]


def scenarios() -> list[Scenario]:
    turns = [Scenario(
        name=f"ai-regress-missile-positive-g-{model}", lane="ai",
        args=["--missile-study", "{work}/study", "--ai", "--trace", "--skill", "ace",
              "--ai-model", model, "--missiles", "AIM9M", "--ranges", "medium",
              "--aspects", "tail", "--altitudes", "co", "--seeds", "1", "--threads", "1"],
        timeout=180, outputs=["study/runs.csv", "study/cells.csv"],
        check_work=defense_problems,
        notes="Committed defensive turns keep positive G and dispense only flares against a visually observed IR missile.",
    ) for model in ("standard", "hybrid")]

    # Two IR seeds retain an actual decoy passage after the two-second cadence
    # change; the single seed-0 shot now hits and cannot exercise inert cleanup.
    turns.append(Scenario(
        name="ai-regress-missile-inert-end", lane="ai",
        args=["--missile-study", "{work}/study", "--ai", "--trace", "--skill", "ace",
              "--missile-model", "energy", "--missiles", "AIM7", "--ranges", "long",
              "--aspects", "tail", "--altitudes", "co", "--seeds", "1", "--threads", "1"],
        then=[Step(args=["--missile-study", "{work}/study-decoy", "--ai", "--trace", "--skill", "ace",
                         "--missile-model", "energy", "--missiles", "AIM9M", "--ranges", "medium",
                         "--aspects", "head", "--altitudes", "co", "--seeds", "2", "--threads", "1"])],
        timeout=180, outputs=["study/runs.csv", "study/cells.csv", "study-decoy/runs.csv"],
        check_work=inert_defense_problems,
        notes="Explicit combat notices end active AI defense on the tick of post-burn defeat and decoy passage.",
    ))
    turns.append(Scenario(
        name="ai-regress-missile-head-on-break", lane="ai",
        args=["--missile-study", "{work}/study", "--ai", "--trace", "--skill", "ace",
              "--missiles", "AIM9M", "--ranges", "medium", "--aspects", "head",
              "--altitudes", "co", "--seeds", "1", "--threads", "1"],
        timeout=180, outputs=["study/runs.csv", "study/cells.csv"],
        check_work=head_break_problems,
        notes="A missile directly ahead prompts a hard crossing break, not a straightening command.",
    ))
    for model in ("standard", "hybrid"):
        turns.append(Scenario(
            name=f"ai-regress-missile-bank-low-{model}", lane="ai",
            args=["--missile-study", "{work}/study", "--ai", "--trace", "--skill", "ace",
                  "--ai-model", model, "--missiles", "AIM120", "--ranges", "medium",
                  "--aspects", "tail", "--altitudes", "down", "--seeds", "1", "--threads", "1"],
            timeout=180, outputs=["study/runs.csv", "study/cells.csv"],
            check_work=defense_problems,
            notes="Radar warning ensures the 3,000 ft target defends; the overhead IR shot is not seen. Check upright bank and the end of the break.",
        ))
    for skill in ("novice", "average"):
        turns.append(Scenario(
            name=f"ai-regress-missile-chaff-{skill}", lane="ai",
            args=["--missile-study", "{work}/study", "--ai", "--trace", "--skill", skill,
                  "--missiles", "AIM120", "--ranges", "medium", "--aspects", "tail",
                  "--altitudes", "co", "--seeds", "1", "--threads", "1"],
            timeout=180, outputs=["study/runs.csv", "study/cells.csv"],
            check_work=chaff_timing_problems,
            notes="Chaff starts in the notch phase for lower skills; no IR throttle cut for a radar threat.",
        ))
    for skill in ("novice", "average", "experienced", "ace"):
        turns.append(Scenario(
            name=f"ai-regress-missile-pull-{skill}", lane="ai",
            args=["--missile-study", "{work}/study", "--ai", "--trace", "--skill", skill,
                  "--missiles", "AIM9M", "--ranges", "medium", "--aspects", "tail",
                  "--altitudes", "co", "--seeds", "1", "--threads", "1"],
            timeout=180, outputs=["study/runs.csv", "study/cells.csv"],
            check_work=skill_pull_problems(skill),
            notes="Each skill promptly reaches its useful G load while rolling upright.",
        ))
        turns.append(Scenario(
            name=f"ai-regress-missile-radar-memory-{skill}", lane="ai",
            args=["--missile-study", "{work}/study", "--ai", "--trace", "--skill", skill,
                  "--missiles", "AIM7", "--ranges", "short,medium,long", "--aspects", "head,beam,tail",
                  "--altitudes", "co", "--seeds", "2", "--threads", "1"],
            timeout=300, outputs=["study/runs.csv", "study/cells.csv"],
            check_work=radar_memory_problems,
            notes="AIM-7 motion-report gaps preserve the break and learned radar class; no flares.",
        ))
    return turns
