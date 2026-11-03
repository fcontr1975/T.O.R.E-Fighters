"""Lane: takeoff to landing, flight model, weapons, countermeasures, damage.

Every scenario is one `tore-app` run. The headless ones use the flight probe
(`--headless-flight`), which prints `extremes:`, `landing:` and
`spin_recovery:` lines this file reads for impossible states. The windowed ones
(`window=True`) go through tools/agent-run.sh and are limited by the runner.
See docs/testing/lane-flight.md.
"""
import os
import csv
import re
import time
from pathlib import Path

from battery import Scenario, Step
from battery_scenarios._debrief import kill_hit_problems
from battery_scenarios._powered import powered_scenarios
from battery_scenarios._strips import ground_airport

# Original roster covered by the established fixed-wing maneuver battery.
AIRCRAFT = ["f18", "rafale", "f14", "a4e", "x31", "mig29", "su27", "mig21", "su25", "mig23", "su35", "f22", "f22n", "faxx"]
VARIETY_AIRCRAFT = ["c130", "ac130", "e3", "il76", "e2", "av8", "yak141", "v22", "ah64", "mi24", "ch47", "mig17", "f4b", "f4j", "f4e", "f4g", "a7", "f15", "f16c", "f104", "a10", "b747", "a310"]
# Aircraft whose PT disables spins (X-31 flag, F-22 family): they never enter one.
SPIN_IMMUNE = {"x31", "f22", "f22n", "faxx"}

# Ground-start airports per theater (`--ground-start 1..N`), counted from the imported maps.
AIRPORTS = {
    "APA": 20, "BAL": 34, "CUB": 23, "EGY": 25, "FRA": 32, "GRE": 13, "IRA": 21, "KURILE": 4,
    "LFA": 5, "NSK": 22, "PGU": 21, "SPA": 21, "TVIET": 10, "UKR": 14, "VLA": 26, "WTA": 16,
    "~APAF": 2, "~BAL0": 34, "~BAL1": 34, "~BAL2": 34, "~BAL3": 34, "~BAL4": 34, "~BAL5": 34,
    "~BAL6": 34, "~BAL7": 34, "~CUBF": 2, "~EGY1": 25, "~EGY2": 25, "~EGY3": 25, "~EGY4": 25,
    "~EGY5": 25, "~EGY6": 25, "~EGY7": 25, "~EGY8": 26, "~EGY9": 25, "~EGYF": 2, "~FRA0": 32,
    "~FRA1": 32, "~FRA2": 32, "~FRA3": 32, "~FRA4": 32, "~FRA5": 32, "~FRA6": 32, "~FRA7": 32,
    "~FRA8": 32, "~FRA9": 32, "~LFAF": 1, "~SPAF": 1, "~UKR1": 14, "~UKR2": 13, "~UKR3": 14,
    "~UKR4": 14, "~UKR5": 14, "~UKR6": 14, "~UKR7": 14, "~UKR8": 14, "~VLA1": 26, "~VLA2": 26,
    "~VLA3": 26, "~VLA4": 26, "~VLA5": 26, "~VLA6": 26, "~VLA7": 26, "~VLA8": 26, "~VLAF": 1,
}
BASE_THEATERS = [t for t in AIRPORTS if not t.startswith("~")]
VARIANT_THEATERS = [t for t in AIRPORTS if t.startswith("~")]

# Gun capacity and the other station capacities of each aircraft's default load,
# in the order `--weapon-slot 1..N` selects them (from the combat smoke). A trailing 0 is a
# retained selection station the variety import added: configured but never loaded, so the
# weapon cycle skips it (docs/formats/aircraft-ordnance.md). A rocket pod station counts its
# rockets, not its pods (ffc8fff2: two LAU-61 or B-8M1 pods hold 38, two B-13L pods 10).
# The A-4E's default load carries three tanks in place of retail's centreline Mk 82s, which
# stay selectable at 0 (35023ad1). F22.PT row 8 is the optional inboard wing pair, empty by
# default (30f69393); F22N.PT now shares that carrier and the bay classification.
STATIONS = {
    "f18": [570, 2, 4, 4, 2],
    "rafale": [250, 4, 2, 2, 2],
    "f14": [675, 4, 2, 2],
    "a4e": [400, 0, 38, 0],
    "x31": [740, 2, 2, 2, 0],
    "mig29": [150, 2, 2, 2],
    "su27": [150, 2, 2, 4, 2],
    "mig21": [200, 2, 2, 0],
    "su25": [250, 10, 4, 4, 0],
    "mig23": [200, 38, 2, 2, 0],
    "su35": [150, 1, 2, 4, 2],
    "f22": [750, 2, 2, 4, 0],
}
STATIONS["f22n"] = [750, 2, 2, 4, 0]
STATIONS["faxx"] = STATIONS["f22n"]


def _numbers(output: str, prefix: str) -> dict:
    """`key=value` pairs of the last output line that starts with `prefix`."""
    lines = [ln for ln in output.splitlines() if ln.startswith(prefix)]
    if not lines:
        return {}
    return dict(re.findall(r"(\w+)=(\S+)", lines[-1]))


def _plain_numbers(output: str) -> dict:
    """`key=value` pairs from the probe's plain result lines."""
    found = {}
    for line in output.splitlines():
        if line.startswith(("trace:", "landing", "spin_recovery", "systems:", "extremes:")):
            continue
        for key, value in re.findall(r"(\w+)=(\S+)", line):
            found[key] = value
    return found


def extremes_problems(output: str, engine_off_ok: bool = False, beyond_envelope_ok: bool = False) -> list[str]:
    """Impossible values in the headless flight probe's `extremes:` line.

    `beyond_envelope_ok` skips the speed and altitude limits, for the climb
    scenario that measures how far past its envelope an aircraft can go."""
    e = _numbers(output, "extremes:")
    if not e:
        return ["no extremes: line (the probe did not finish)"]
    problems = []
    try:
        if int(e["non_finite"]) != 0:
            problems.append(f"{e['non_finite']} non-finite samples")
        if float(e["fuel_rise_lb"]) > 0.01:
            problems.append(f"fuel rose by {e['fuel_rise_lb']} lb")
        if float(e["dead_stick_gain_ft"]) > 25 and not engine_off_ok:
            problems.append(f"energy gained with the engine off: {e['dead_stick_gain_ft']} ft")
        if float(e["dead_stick_gain_ft"]) > 200:
            problems.append(f"large energy gain with the engine off: {e['dead_stick_gain_ft']} ft")
        if float(e["max_speed_kt"]) > 1300 and not beyond_envelope_ok:
            problems.append(f"impossible speed {e['max_speed_kt']} kt")
        if float(e.get("speed_over_envelope_top", 0)) > 1.02 and not beyond_envelope_ok:
            problems.append(f"flew faster than the aircraft's own top speed: {e['speed_over_envelope_top']} of it")
        if not (-12 <= float(e["min_g"]) and float(e["max_g"]) <= 16):
            problems.append(f"impossible load {e['min_g']}..{e['max_g']} G")
        if float(e["max_altitude_ft"]) > 70000 and not beyond_envelope_ok:
            problems.append(f"impossible altitude {e['max_altitude_ft']} ft")
        if float(e["min_altitude_ft"]) < -100:
            problems.append(f"under the ground: {e['min_altitude_ft']} ft")
        if int(e.get("under_ground_ticks", 0)) > 0:
            problems.append(f"{e['under_ground_ticks']} ticks more than 30 ft below the surface (lowest {e.get('min_agl_ft')} ft)")
        for veil in ("max_blackout", "max_redout"):
            if not 0 <= float(e.get(veil, 0)) <= 1:
                problems.append(f"{veil} {e[veil]} outside 0..1")
        if float(e.get("max_blackout", 0)) > 0 and float(e["max_g"]) < 5:
            problems.append(f"the view greyed out at {e['max_g']} G")
        if float(e.get("max_redout", 0)) > 0 and float(e["min_g"]) > -2:
            problems.append(f"the view reddened at {e['min_g']} G")
        if float(e.get("energy_rate_over_thrust", 0)) > 1.0:
            problems.append(f"energy gained faster than the thrust allows: {e['energy_rate_over_thrust']} of thrust power")
        if float(e["max_pitch_rate_dps"]) > 120 or float(e["max_roll_rate_dps"]) > 720:
            problems.append(f"impossible rates pitch {e['max_pitch_rate_dps']} roll {e['max_roll_rate_dps']} deg/s")
    except (KeyError, ValueError) as error:
        problems.append(f"unreadable extremes line: {error}")
    return problems


# ---------------------------------------------------------------- takeoff


def loadout_problems(output: str) -> list[str]:
    """The default ground-start load must weigh no more than the aircraft's own
    maximum takeoff weight, with fuel within its tanks."""
    load = _numbers(output, "loadout:")
    if not load:
        return []
    problems = []
    try:
        if float(load["gross_lb"]) > float(load["max_takeoff_lb"]):
            problems.append(f"default load weighs {load['gross_lb']} lb, over the {load['max_takeoff_lb']} lb maximum takeoff weight")
        if float(load["fuel_lb"]) > float(load["internal_fuel_lb"]) + 0.5 or float(load["fuel_lb"]) < 0:
            problems.append(f"fuel {load['fuel_lb']} lb outside 0..{load['internal_fuel_lb']}")
        if float(load["carried_lb"]) < 0:
            problems.append(f"negative carried weight {load['carried_lb']} lb")
    except (KeyError, ValueError):
        problems.append("unreadable loadout line")
    return problems


def tank_problems(output: str) -> list[str]:
    """External tanks feed first, and no fuel figure goes negative or rises."""
    load = _numbers(output, "loadout:")
    end = _numbers(output, "fuel_end:")
    if not load or not end:
        return []
    try:
        external0, internal0 = float(load["external_fuel_lb"]), float(load["fuel_lb"])
        external, internal = float(end["external_lb"]), float(end["internal_lb"])
    except (KeyError, ValueError):
        return ["unreadable fuel lines"]
    problems = []
    if external < -0.05 or internal < -0.05:
        problems.append(f"negative fuel: internal {internal}, external {external}")
    if external > external0 + 0.05 or internal > internal0 + 0.05:
        problems.append("fuel rose during the run")
    if external > 0.05 and internal < internal0 - 0.05:
        problems.append(f"internal fuel used ({internal0} -> {internal}) while the tanks still held {external} lb")
    return problems


def check_takeoff(output: str) -> list[str]:
    problems = extremes_problems(output) + loadout_problems(output) + tank_problems(output)
    n = _plain_numbers(output)
    if "takeoff_complete=true" not in output:
        problems.append("never reached 100 ft (stuck on the runway or crashed)")
    if n.get("crashed") != "false":
        problems.append("crashed during takeoff")
    try:
        ticks = int(n["ticks"])
        if not 300 <= ticks <= 4500:
            problems.append(f"takeoff took {ticks} ticks")
        speed = float(n["speed_kt"])
        if not 60 <= speed <= 260:
            problems.append(f"liftoff speed {speed} kt")
    except (KeyError, ValueError):
        problems.append("no result line")
    return problems + liftoff_problems(output)


def liftoff_problems(output: str) -> list[str]:
    """The wheels leave the ground at the speed the weight-scaled model gives.
    Two checks. The imported 1 G edge bounds it (0.75 to 1.9 times, since the
    stall speed grows with the square root of the weight over the aircraft's
    fitted reference weight, docs/spec/takeoff-ground-contact.md). And it lands
    within 5 percent below to 12 percent above the loaded minimum speed for 1 G
    with full flaps that the probe prints as `min_level_flaps_kt`, which is
    the rule the model follows."""
    env = re.search(r"^envelope: .*? g1_kt=([\d.]+)\.\.[\d.]+.*? min_level_flaps_kt=([\d.]+)", output, re.M)
    lift = re.search(r"^liftoff: tick=\d+ speed_kt=([\d.]+) distance_ft=(\d+)", output, re.M)
    if not env or not lift:
        return ["no envelope: or liftoff: line"]
    edge, minimum, speed = float(env.group(1)), float(env.group(2)), float(lift.group(1))
    problems = []
    if not 0.75 * edge <= speed <= 1.9 * edge:
        problems.append(f"lifted off at {speed} kt with a 1 G stall edge of {edge} kt")
    if not 0.95 * minimum <= speed <= 1.12 * minimum:
        problems.append(f"lifted off at {speed} kt, the model's loaded minimum speed is {minimum} kt")
    return problems


# What John asked for on 2026-09-29 (unsourced real-world figures, a plausibility
# range only), in knots: liftoff at the default gross weight and approach at a
# landing weight of 65 percent internal fuel. `None` where there is no figure.
LIFTOFF_TARGET = {"su27": (135, 150), "su25": (130, 145), "f22": (130, 150), "f22n": (130, 150), "faxx": (130, 150)}
APPROACH_TARGET = {
    "f18": (135, 135), "rafale": (130, 140), "f14": (130, 140), "a4e": (130, 140), "mig29": (135, 145),
    "su27": (120, 135), "mig21": (160, 170), "su25": (125, 140), "mig23": (150, 165), "su35": (135, 145),
    "f22": (135, 145), "f22n": (135, 145), "faxx": (135, 145),
}
# The figures the model gives with the fitted per-aircraft stall references
# (`STALL_REFERENCE_FRACTIONS`, 2026-09-30), for the aircraft whose figure is not
# exactly inside John's range, kept as a regression window of plus or minus 6
# percent (docs/testing/lane-flight.md). All of them are within 10 percent of the
# range, so the range's own window already holds them; these record the values.
KNOWN_LIFTOFF = {"su27": 128.1, "su25": 120.2, "f22": 127.7, "f22n": 127.7, "faxx": 127.7}
KNOWN_APPROACH = {"f18": 135.1, "su27": 142.2, "f22": 148.5, "f22n": 148.5, "faxx": 148.5}
# Internal fuel of each aircraft's default load, pounds.
INTERNAL_FUEL_LB = {
    "f18": 11220, "rafale": 9900, "f14": 15741, "a4e": 4434, "x31": 9975, "mig29": 13000, "su27": 19000,
    "mig21": 4534, "su25": 6614, "mig23": 11704, "su35": 22000, "f22": 25000, "f22n": 25000, "faxx": 25000,
}


def _in_target(value: float, target: tuple, known: float | None) -> str | None:
    lo, hi = target
    if 0.9 * lo <= value <= 1.1 * hi:
        return None
    if known is not None and 0.94 * known <= value <= 1.06 * known:
        return None
    return f"{value} kt, outside {lo} to {hi} kt and its recorded {known} kt"


def check_liftoff_speed(output: str, ac: str) -> list[str]:
    problems = check_takeoff(output)
    lift = re.search(r"^liftoff: tick=\d+ speed_kt=([\d.]+)", output, re.M)
    if lift and ac in LIFTOFF_TARGET:
        p = _in_target(float(lift.group(1)), LIFTOFF_TARGET[ac], KNOWN_LIFTOFF.get(ac))
        if p:
            problems.append("liftoff at " + p)
    return problems


def check_approach_speed(output: str, ac: str) -> list[str]:
    """The scripted approach at a landing weight: flown at 1.3 times the flapped,
    weight-scaled stall speed (never under 1.05 times the loaded 1 G minimum),
    lands, stops and does not crash."""
    problems = check_landing(output)
    m = re.search(r"approach_kt=([\d.]+)", output)
    if not m:
        return problems + ["no approach speed"]
    if ac in APPROACH_TARGET:
        p = _in_target(float(m.group(1)), APPROACH_TARGET[ac], KNOWN_APPROACH.get(ac))
        if p:
            problems.append("approach at " + p)
    return problems


# The G a held pull reaches at 450 knots and 5,000 ft with the imported speeds
# (`--retail-stall-speeds`, 2026-09-29). The weight-scaled stall speed keeps the
# combat-speed G: the scaling fades out by the 4 G row (docs/spec/takeoff-ground-contact.md).
# The Su-25 was re-recorded on 2026-10-13 (4.99 to 5.41) when its record lost
# the Su-24's mass and thrust (docs/spec/aircraft-records.md).
PULL_G_RETAIL = {
    "f18": 7.64, "rafale": 7.39, "f14": 6.18, "a4e": 6.13, "x31": 7.25, "mig29": 7.15, "su27": 7.54,
    "mig21": 6.47, "su25": 5.41, "mig23": 6.77, "su35": 6.96, "f22": 6.91, "f22n": 6.91, "faxx": 6.91,
}


def check_combat_g(output: str, ac: str) -> list[str]:
    problems = extremes_problems(output)
    e = _numbers(output, "extremes:")
    try:
        got = float(e["max_g"])
    except (KeyError, ValueError):
        return problems + ["no extremes: line"]
    want = PULL_G_RETAIL[ac]
    if not 0.97 * want <= got <= 1.03 * want:
        problems.append(f"the held pull at 450 kt reached {got} G, the imported speeds give {want} G")
    return problems


def combat_g_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-combatg-{ac}",
            lane="flight",
            args=["--headless-flight", "600", "--maneuver", "pull", "--aircraft", ac, "--no-audio"],
            check=lambda output, ac=ac: check_combat_g(output, ac),
        )
        for ac in AIRCRAFT
    ]


def check_retail_liftoff(output: str) -> list[str]:
    """`--retail-stall-speeds`: the imported polygon speeds at every weight."""
    problems = check_takeoff(output)
    if "stall_scale=1.000" not in output:
        problems.append("the retail switch did not set the stall scale to 1")
    return problems


def liftoff_scenarios() -> list[Scenario]:
    out = []
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"flight-liftoff-{ac}",
                lane="flight",
                args=["--theater", "UKR", "--ground-start", "1", "--headless-flight", "9000", "--maneuver", "takeoff", "--aircraft", ac, "--no-audio"],
                check=lambda output, ac=ac: check_liftoff_speed(output, ac),
            )
        )
        out.append(
            Scenario(
                name=f"flight-liftoff-retail-{ac}",
                lane="flight",
                args=["--theater", "UKR", "--ground-start", "1", "--headless-flight", "9000", "--maneuver", "takeoff", "--retail-stall-speeds", "--aircraft", ac, "--no-audio"],
                check=check_retail_liftoff,
            )
        )
        out.append(
            Scenario(
                name=f"flight-approach-{ac}",
                lane="flight",
                args=[
                    "--theater", "UKR", "--ground-start", "1", "--headless-flight", "60000", "--maneuver", "land",
                    "--flight-fuel", str(round(0.65 * INTERNAL_FUEL_LB[ac])), "--aircraft", ac, "--no-audio",
                ],
                check=lambda output, ac=ac: check_approach_speed(output, ac),
                timeout=300,
            )
        )
    return out


def _theater_tag(theater: str) -> str:
    return theater.lstrip("~").lower() + ("v" if theater.startswith("~") else "")


def takeoff_scenarios() -> list[Scenario]:
    out = []
    seen = set()

    def add(ac: str, theater: str, n: int) -> None:
        name = f"flight-takeoff-{ac}-{_theater_tag(theater)}-{n}"
        if name in seen:
            return
        seen.add(name)
        out.append(
            Scenario(
                name=name,
                lane="flight",
                args=["--theater", theater, "--ground-start", str(n), "--headless-flight", "9000", "--maneuver", "takeoff", "--aircraft", ac, "--no-audio"],
                check=check_takeoff,
                timeout=120,
            )
        )

    # Every aircraft from the first and the middle airport of every base theater.
    for ac in AIRCRAFT:
        for theater in BASE_THEATERS:
            add(ac, theater, 1)
            add(ac, theater, max(1, AIRPORTS[theater] // 2))
    # The Hornet from every airport of every base theater.
    for theater in BASE_THEATERS:
        for n in range(1, AIRPORTS[theater] + 1):
            add("f18", theater, n)
    # The Hornet from the first airport of every imported variant.
    for theater in VARIANT_THEATERS:
        add("f18", theater, 1)
    return out


# ---------------------------------------------------------------- landing


def _landing(output: str) -> tuple[dict, float]:
    landing = _numbers(output, "landing:")
    start = re.search(r"runway_length_ft=(\d+)", output)
    return landing, float(start.group(1)) if start else 0.0


SHORT_STRIP_FT = 2000.0  # the game's line: no tower, clearance or ILS below it


def ils_problems(output: str) -> list[str]:
    """The player's ILS read along a scripted approach that flew its glide path:
    it must be active and stay near the bars (the pilot script is not perfect,
    and a crosswind lets it drift a little)."""
    m = re.search(r"ils_probe: samples=(\d+) active=(\d+)(?: max_abs_glide_deg=([\d.]+) max_abs_localizer_deg=([\d.]+))?", output)
    if not m:
        return []
    # Since f7d16d85 the tower gives no ILS to a runway under 2,000 ft (the 22
    # airstrips of 1,074 ft): none must be offered there, and the rest of the
    # landing's checks still apply.
    start = re.search(r"runway_length_ft=(\d+)", output)
    if start and float(start.group(1)) < SHORT_STRIP_FT:
        if int(m.group(2)) != 0:
            return ["the tower offered an ILS on a short strip"]
        return []
    if int(m.group(2)) == 0:
        return ["the ILS never became active on the approach"]
    glide, localizer = float(m.group(3)), float(m.group(4))
    problems = []
    if glide > 0.8:
        problems.append(f"the flown approach strayed {glide} degrees from the ILS glide path")
    if localizer > 1.5:
        problems.append(f"the flown approach strayed {localizer} degrees from the localizer")
    return problems


def check_landing(output: str, overrun_ok_ft: float = 0.0) -> list[str]:
    """`overrun_ok_ft` allows a longer roll (a tailwind landing)."""
    problems = extremes_problems(output) + ils_problems(output)
    landing, length = _landing(output)
    if not landing:
        return problems + ["no landing: line"]
    # The scripted landing floats to about 1,500 ft past the threshold, so only
    # a long runway is expected to hold it; a short strip may overrun.
    if length >= 5500:
        if landing.get("touchdown") != "true":
            problems.append("never touched down")
        if landing.get("crashed") != "false":
            problems.append(f"crashed on a {length:.0f} ft runway: {landing.get('unsafe')}")
        if landing.get("stopped") != "true":
            problems.append("did not come to a stop")
        elif float(landing.get("runway_left_ft", "-1")) < -overrun_ok_ft:
            problems.append(f"rolled {landing['runway_left_ft']} ft off the end of a {length:.0f} ft runway")
        if landing.get("unsafe", "none") != "none":
            problems.append(f"unsafe touchdown: {landing['unsafe']}")
        if landing.get("touchdown") == "true":
            if float(landing["touchdown_sink_fps"]) > 12:
                problems.append(f"hard touchdown, {landing['touchdown_sink_fps']} ft/s")
            if abs(float(landing["touchdown_bank_deg"])) > 5:
                problems.append(f"touchdown bank {landing['touchdown_bank_deg']} deg")
            if landing.get("left_runway") != "false" and overrun_ok_ft == 0:
                problems.append(f"rolled off the runway surface at {landing.get('left_runway_kt')} kt")
    else:
        # A short strip: landing, overrunning off the end or a crash off the
        # runway are all fine, but never a gear failure with the gear down.
        if "gear_up=true" in landing.get("unsafe", "none"):
            problems.append("gear-up crash on an approach flown with the gear down")
    return problems


def check_landing_gear_up(output: str) -> list[str]:
    problems = extremes_problems(output)
    landing, _ = _landing(output)
    if landing.get("crashed") != "true" or "gear_up=true" not in landing.get("unsafe", ""):
        problems.append(f"a gear-up landing did not crash for the gear: {landing.get('unsafe')}")
    return problems


def check_landing_off_runway(output: str) -> list[str]:
    problems = extremes_problems(output)
    landing, _ = _landing(output)
    if landing.get("crashed") != "true" or "not_landable=true" not in landing.get("unsafe", ""):
        problems.append(f"touching down beside the runway did not crash: {landing.get('unsafe')}")
    return problems


def check_landing_hard(output: str) -> list[str]:
    problems = extremes_problems(output)
    landing, _ = _landing(output)
    if landing.get("touchdown") != "true" and landing.get("crashed") != "true":
        problems.append("a steep touchdown neither landed nor crashed")
    return problems


def landing_scenarios() -> list[Scenario]:
    out = []
    seen = set()

    def add(ac: str, theater: str, n: int, tag: str = "", maneuver: str = "land", check=check_landing, env=None) -> None:
        name = f"flight-land{tag}-{ac}-{_theater_tag(theater)}-{n}"
        if name in seen:
            return
        seen.add(name)
        out.append(
            Scenario(
                name=name,
                lane="flight",
                args=["--theater", theater, "--ground-start", str(n), "--headless-flight", "60000", "--maneuver", maneuver, "--aircraft", ac, "--no-audio"],
                check=check,
                env=env or {},
                timeout=180,
            )
        )

    # The Hornet at every airport of every base theater.
    for theater in BASE_THEATERS:
        for n in range(1, AIRPORTS[theater] + 1):
            add("f18", theater, n)
    # Every other aircraft at the first airport of every base theater.
    for ac in AIRCRAFT:
        if ac != "f18":
            for theater in BASE_THEATERS:
                add(ac, theater, 1)
    # Variants: the Hornet at the first airport of each.
    for theater in VARIANT_THEATERS:
        add("f18", theater, 1)
    # Unsafe approaches, every aircraft at UKR airport 1.
    for ac in AIRCRAFT:
        add(ac, "UKR", 1, "-gearup", "land-gear-up", check_landing_gear_up)
        add(ac, "UKR", 1, "-offrunway", "land-off-runway", check_landing_off_runway)
        add(ac, "UKR", 1, "-hard", "land-hard", check_landing_hard)
    # Wind: head, tail and crosswinds of 20 to 60 ft/s (12 to 36 knots).
    for ac in AIRCRAFT:
        for heading, speed in [(0, 30), (180, 30), (90, 30), (270, 20), (45, 60)]:
            # Heading 0 is a tailwind on this runway: a longer roll is fair.
            check = (lambda out: check_landing(out, 400.0)) if heading == 0 else check_landing
            add(ac, "UKR", 1, f"-wind{heading}-{speed}", "land", check, {"TORE_WIND": f"{heading},{speed}"})
    return out


# --------------------------------------------------------------- manoeuvres

# The manoeuvres that hold the controls until the end: a rolling or banked pull
# with no pitch hold can fly into the ground, which is the script's doing.
MAY_CRASH = {"roll", "spin", "bank-left", "bank-right"}


def make_check_maneuver(maneuver: str):
    def check(output: str) -> list[str]:
        problems = extremes_problems(output, engine_off_ok=maneuver in {"stall", "spin"})
        n = _plain_numbers(output)
        if maneuver not in MAY_CRASH and n.get("crashed") != "false":
            problems.append("crashed in a manoeuvre that should stay in the air")
        try:
            ticks = int(n["ticks"])
            if maneuver in {"level", "pull", "stall"} and ticks != 7200:
                problems.append(f"ended at tick {ticks}")
            if maneuver == "loop" and n.get("loop_completed") != "true":
                problems.append("did not complete the loop")
        except KeyError:
            problems.append("no result line")
        return problems

    return check


def maneuver_scenarios() -> list[Scenario]:
    out = []
    adapters = [("default", []), ("legacy", ["--legacy-flight"]), ("researched", ["--researched-flight"])]
    for ac in AIRCRAFT:
        for maneuver in ["level", "pull", "loop", "roll", "stall", "spin", "bank-left", "bank-right"]:
            for label, flags in adapters:
                out.append(
                    Scenario(
                        name=f"flight-{maneuver}-{ac}-{label}",
                        lane="flight",
                        args=["--headless-flight", "7200", "--aircraft", ac, "--maneuver", maneuver, "--no-audio", *flags],
                        check=make_check_maneuver(maneuver),
                        timeout=120,
                    )
                )
    return out


def check_spin_recovery(ac: str):
    def check(output: str) -> list[str]:
        problems = extremes_problems(output, engine_off_ok=True)
        s = _numbers(output, "spin_recovery:")
        if not s:
            return problems + ["no spin_recovery: line"]
        entered = s.get("entered_tick") != "never"
        if ac in SPIN_IMMUNE:
            if entered:
                problems.append("spin-immune aircraft entered a spin")
            return problems
        if not entered:
            problems.append("could not be put into a spin with wrong rudder and full back stick")
            return problems
        if s.get("recovered_tick") == "never":
            problems.append("the manual's spin recovery did not recover the aircraft")
        if s.get("crashed") != "false":
            problems.append("crashed in the spin recovery")
        if float(s["revolutions_during_recovery"]) > 3:
            problems.append(f"needed {s['revolutions_during_recovery']} revolutions to recover")
        if float(s["altitude_lost_ft"]) > 8000:
            problems.append(f"lost {s['altitude_lost_ft']} ft in the spin")
        return problems

    return check


def spin_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-spinrecover-{ac}",
            lane="flight",
            args=["--headless-flight", "12000", "--aircraft", ac, "--maneuver", "spin-recover", "--no-audio"],
            check=check_spin_recovery(ac),
            timeout=120,
        )
        for ac in AIRCRAFT
    ]


def check_stall_recovery(output: str) -> list[str]:
    problems = extremes_problems(output, engine_off_ok=True)
    s = _numbers(output, "stall_recovery:")
    if not s:
        return problems + ["no stall_recovery: line"]
    if s.get("alert_tick") == "never":
        problems.append("a full-back-stick pull at low speed never raised a stall alert")
    elif s.get("recovered_tick") == "never":
        problems.append("nose down with the afterburner in did not clear the stall")
    if s.get("crashed") != "false":
        problems.append("crashed in the stall recovery")
    return problems


def stall_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-stallrecover-{ac}",
            lane="flight",
            args=["--headless-flight", "20000", "--aircraft", ac, "--maneuver", "stall-recover", "--no-audio"],
            check=check_stall_recovery,
            timeout=120,
        )
        for ac in AIRCRAFT
    ]


# ------------------------------------------------------------ system faults


def check_faults(output: str) -> list[str]:
    problems = extremes_problems(output, engine_off_ok=True)
    if not _numbers(output, "systems:"):
        problems.append("no systems: line")
    return problems


def fault_scenarios() -> list[Scenario]:
    out = []
    # Every fault index 0..44 on three different aircraft, in a sustained pull.
    for ac in ["f18", "su25", "f22"]:
        for index in range(45):
            out.append(
                Scenario(
                    name=f"flight-fault{index:02d}-{ac}",
                    lane="flight",
                    args=["--headless-flight", "5400", "--aircraft", ac, "--maneuver", "pull", "--flight-fault", f"240:{index}", "--no-audio"],
                    check=check_faults,
                    timeout=120,
                )
            )
    # Every fault at once, and one after another, in every aircraft.
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"flight-faultall-{ac}",
                lane="flight",
                args=["--headless-flight", "7200", "--aircraft", ac, "--maneuver", "level", *[a for i in range(45) for a in ("--flight-fault", f"240:{i}")], "--no-audio"],
                check=check_faults,
                timeout=120,
            )
        )
        out.append(
            Scenario(
                name=f"flight-faultseq-{ac}",
                lane="flight",
                args=["--headless-flight", "7200", "--aircraft", ac, "--maneuver", "loop", *[a for i in range(45) for a in ("--flight-fault", f"{120 + 4 * i}:{i}")], "--no-audio"],
                check=check_faults,
                timeout=120,
            )
        )
    return out


# ------------------------------------------------------------------ combat


def check_combat_smoke(output: str) -> list[str]:
    problems = []
    passes = len(re.findall(r"\bPASS\b", output))
    if passes < 8:
        problems.append(f"only {passes} PASS lines")
    for line in output.splitlines():
        if "FAIL" in line and "PASS" not in line:
            problems.append(line.strip()[:200])
    return problems


def check_wreck_contacts(output: str) -> list[str]:
    problems = check_combat_smoke(output)
    if "wreck collision: weapons=2 hits=2 second_kills=0 PASS" not in output:
        problems.append("gun and AIM-120 wreck-contact checks did not both pass")
    return problems


# Aircraft with no reviewed guided air-to-air missile in the default load.
NO_MISSILES = {"a4e", "mig23"}
# Aircraft whose missile acceptance run is too slow for the battery.
SLOW_ACCEPTANCE = {"f14", "su35"}


def make_check_missile_acceptance(ac: str):
    def check(output: str) -> list[str]:
        if "weapon,mode,launcher_fps" not in output:
            return ["no acceptance table"]
        rows = [ln for ln in output.splitlines() if ".JT," in ln]
        if not rows:
            return [] if ac in NO_MISSILES else ["no acceptance rows"]
        return check_missile_rows(rows)

    return check


def check_missile_rows(rows: list[str]) -> list[str]:
    problems = []
    for row in rows:
        cols = row.split(",")
        # weapon, mode, range, target, motion, fraction, feet, outcome, seconds, closest
        if len(cols) < 10:
            problems.append(f"short row: {row[:100]}")
            continue
        try:
            if float(cols[8]) < 0:
                problems.append(f"negative flight time: {row[:100]}")
        except ValueError:
            problems.append(f"unreadable row: {row[:100]}")
        # Hits, guidance expiry and inhibited shots are the recorded outcomes
        # (docs/baselines/missiles.md); anything else is new.
        if cols[7] != "hit" and cols[7] != "expiry" and not cols[7].startswith("inhibit:"):
            problems.append(f"unknown outcome {cols[7]!r}: {row[:100]}")
    if not any(r.split(",")[7] == "hit" for r in rows):
        problems.append("no missile hit anything")
    return problems


def check_combat_evidence(output: str) -> list[str]:
    """The smoke's per-slot tapes replay to exactly the live state."""
    problems = check_combat_smoke(output)
    if "serialized combat replay" not in output:
        problems.append("no combat tape was written and replayed")
    if "diverged" in output:
        problems.append("a combat tape replay diverged from the live state")
    return problems


def check_missile_sag(output: str) -> list[str]:
    rows = [line.split(",") for line in output.splitlines() if line.startswith("sag,") and ".JT," in line]
    problems = []
    if len(rows) != 128:
        problems.append(f"expected 128 sag measurements, got {len(rows)}")
    ranges = {"AIM7.JT": 40, "AIM9M.JT": 12, "AIM120.JT": 50, "SEA_SPAR.JT": 12}
    checked = set()
    for row in rows:
        if len(row) != 8:
            problems.append(f"short sag row: {row}")
            continue
        try:
            _, weapon, height, distance, miss, end, predicted, ticks = row
            if float(distance) != ranges.get(weapon):
                continue
            checked.add((weapon, float(height)))
            if float(miss) >= 1 or end != "passed" or predicted != "true" or int(ticks) <= 0:
                problems.append(f"coasting intercept failed: {','.join(row)}")
        except ValueError:
            problems.append(f"unreadable sag row: {row}")
    expected = {(weapon, height) for weapon in ranges for height in (300, 10000)}
    if checked != expected:
        problems.append(f"missing sag cases: {sorted(expected - checked)}")
    return problems


def check_missile_energy(work, _output):
    """Actual episode logs must not compound; scripted worker counts must agree."""
    problems = []
    try:
        runs = work / "energy" / "runs.csv"
        if runs.read_bytes() != (work / "energy-serial" / "runs.csv").read_bytes():
            problems.append("energy study differs between serial and parallel workers")
        with runs.open() as stream:
            rows = list(csv.DictReader(stream))
        if len(rows) != 240:
            problems.append(f"expected 240 energy shots, got {len(rows)}")
        for row in rows:
            devices, rolls = int(row["devices"]), int(row["rolls"])
            limit = (devices + 7) // 8 if row["profile"] == "cm-hammer" else devices
            if rolls > limit:
                problems.append(f"repeated episode rolls: {row['missile']} {rolls} from {devices} devices")
                break
        outcomes = {row["outcome"] for row in rows}
        if "hit" not in outcomes or not outcomes.intersection({"ground", "energy", "seeker-lost", "expired"}):
            problems.append("energy study lacks both an intercept and a physical/seeker miss")
        if not any(row["range"] == "long" and row["hit"] == "0" and row["control_loss_s"] for row in rows):
            problems.append("long energy shots never lost physical control")
        if not any(int(row["rolls"]) for row in rows):
            problems.append("energy study did not exercise a decoy roll")
        with (work / "spec-compat" / "runs.csv").open() as stream:
            legacy = list(csv.DictReader(stream))
        if len(legacy) != 8 or not any(int(row["rolls"]) for row in legacy):
            problems.append("historical high-susceptibility study did not finish eight decoy trials")
    except (OSError, ValueError, KeyError) as error:
        problems.append(f"unreadable energy study: {error}")
    return problems



def check_missile_defeat(work: Path, _output: str) -> list[str]:
    """Post-burn defeat must be observed and never followed by a scored hit."""
    import csv
    try:
        first = (work / "defeat" / "runs.csv").read_bytes()
        second = (work / "defeat-serial" / "runs.csv").read_bytes()
        rows = list(csv.DictReader(first.decode().splitlines()))
        near = (work / "defeat-near" / "runs.csv").read_bytes()
        near_serial = (work / "defeat-near-serial" / "runs.csv").read_bytes()
        rows += list(csv.DictReader(near.decode().splitlines()))
        defeated = [row for row in rows if row["defeat_s"]]
        problems = []
        if near != near_serial:
            problems.append("inert near-pass differs between serial and parallel workers")
        if not any(row["closest_inert_ft"] and
                   float(row["closest_inert_ft"]) <= float(row["inert_radius_ft"])
                   for row in defeated):
            problems.append("no inert body passed within its target's fuze/contact radius")
        if any(int(row["inert_damage"]) for row in defeated):
            problems.append("an inert body damaged its target")
        if first != second:
            problems.append("defeat study differs between serial and parallel workers")
        if not defeated:
            problems.append("defeat study never reached the post-burn Mach threshold")
        if any(row["hit"] != "0" for row in defeated):
            problems.append("a defeated store scored a hit")
        return problems
    except (OSError, KeyError, ValueError) as error:
        return [f"unreadable defeat study: {error}"]


def check_countermeasure_cadence(output):
    """One deterministic AI flight, actual fixed-tick cartridge release times."""
    ticks = [int(t) for t in re.findall(r"missile-study: device AIM9M\.JT seed 0 .*?release_tick=(\d+) kind=Flare", output)]
    if len(ticks) < 4:
        return [f"cadence fixture produced fewer than two pairs: {ticks}"]
    problems = []
    if any(b - a != 30 for a, b in zip(ticks[::2], ticks[1::2])):
        problems.append(f"second cartridge is not 30 ticks after its pair start: {ticks}")
    if any(b - a < 240 for a, b in zip(ticks[::2], ticks[2::2])):
        problems.append(f"AI pair starts are less than 240 ticks apart: {ticks}")
    if len(ticks) > 6:
        problems.append(f"single-threat device budget exceeded: {ticks}")
    return problems


def combat_scenarios() -> list[Scenario]:
    out = [Scenario(name="flight-wreckcontact-f18", lane="flight",
                    args=["--combat-smoke", "--aircraft", "f18", "--no-audio"],
                    check=check_wreck_contacts, timeout=300)]
    out.append(Scenario(name="flight-countermeasures-cadence", lane="flight",
                        args=["--missile-study", "{work}/cadence", "--missiles", "AIM9M",
                              "--profiles", "ai", "--ranges", "short", "--aspects", "head",
                              "--altitudes", "co", "--seeds", "1", "--threads", "1", "--ai",
                              "--skill", "ace", "--trace", "--missile-model", "energy"],
                        outputs=["cadence/runs.csv"], check=check_countermeasure_cadence,
                        timeout=180, notes="AI pairs span decoy episodes and retain the device budget"))
    study = ["--missiles", "AIM9M,AIM120", "--profiles", "straight,break,beam+cm-ai@off,dive,cm-hammer",
             "--ranges", "short,medium,long", "--aspects", "head,tail", "--altitudes", "low,high",
             "--seeds", "2", "--missile-model", "energy"]
    out.append(Scenario(name="flight-missile-energy", lane="flight",
                        args=["--missile-study", "{work}/energy", "--threads", "4"] + study,
                        then=[Step(args=["--missile-study", "{work}/energy-serial", "--threads", "1"] + study),
                              Step(args=["--missile-study", "{work}/spec-compat", "--threads", "1",
                                         "--missile-model", "spec", "--missiles", "AIM9B", "--profiles", "cm-hammer",
                                         "--ranges", "short", "--aspects", "tail", "--altitudes", "co", "--seeds", "8"])],
                        outputs=["energy/runs.csv", "energy-serial/runs.csv", "spec-compat/runs.csv"], check_work=check_missile_energy,
                        timeout=180, notes="Shared energy flight, physical decoy episodes and worker determinism"))
    defeat = ["--missiles", "AIM120,AA8", "--profiles", "straight,dive", "--ranges", "long",
              "--aspects", "head,tail", "--altitudes", "low,high", "--seeds", "2",
              "--missile-model", "energy"]
    out.append(Scenario(name="flight-missile-defeat", lane="flight",
                        args=["--missile-study", "{work}/defeat", "--threads", "4"] + defeat,
                        then=[Step(args=["--missile-study", "{work}/defeat-serial", "--threads", "1"] + defeat),
                              Step(args=["--missile-study", "{work}/defeat-near", "--threads", "4",
                                         "--missiles", "AIM120", "--profiles", "straight", "--ranges", "defeat-edge",
                                         "--aspects", "head", "--altitudes", "low", "--seeds", "2", "--missile-model", "energy"]),
                              Step(args=["--missile-study", "{work}/defeat-near-serial", "--threads", "1",
                                         "--missiles", "AIM120", "--profiles", "straight", "--ranges", "defeat-edge",
                                         "--aspects", "head", "--altitudes", "low", "--seeds", "2", "--missile-model", "energy"])],
                        outputs=["defeat/runs.csv", "defeat-serial/runs.csv", "defeat-near/runs.csv", "defeat-near-serial/runs.csv"],
                        check_work=check_missile_defeat, timeout=180,
                        notes="Post-burn inert defeat and serial/parallel equality"))
    out.append(Scenario(name="flight-missile-sag", lane="flight",
                        args=["--missile-sag", "--no-audio"],
                        check=check_missile_sag, timeout=120))
    for ac in AIRCRAFT:
        out.append(Scenario(name=f"flight-combatsmoke-{ac}", lane="flight", args=["--combat-smoke", "--aircraft", ac, "--no-audio"], check=check_combat_smoke, timeout=300))
        # The F-14 and Su-35 carry very long range missiles: their acceptance
        # tables take about 40 minutes each in a debug build, so they are left to
        # the slow set (below) with the other aircraft's release-build runs.
        if ac not in SLOW_ACCEPTANCE:
            out.append(Scenario(name=f"flight-missileacceptance-{ac}", lane="flight", args=["--missile-acceptance", "--aircraft", ac, "--no-audio"], check=make_check_missile_acceptance(ac), timeout=1200))
    # The same smoke with per-slot combat tapes written and replayed
    # (TORE_COMBAT_EVIDENCE takes a new folder each run, so it is named by the
    # time the scenario list was built).
    if os.environ.get("TORE_BATTERY_SLOW"):
        # Slow set, off by default: every aircraft's missile acceptance table.
        # Run it with an optimized build:
        #   cargo build --release -p tore-app
        #   TORE_BATTERY_SLOW=1 python3 tools/battery.py --scenario 'flight-slow-*' \
        #       --bin target/release/tore-app --jobs 4
        for ac in AIRCRAFT:
            out.append(
                Scenario(
                    name=f"flight-slow-missileacceptance-{ac}",
                    lane="flight",
                    args=["--missile-acceptance", "--aircraft", ac, "--no-audio"],
                    check=make_check_missile_acceptance(ac),
                    timeout=7200,
                    notes="slow: run with an optimized build and TORE_BATTERY_SLOW=1",
                )
            )
    stamp = time.strftime("%Y%m%d-%H%M%S")
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"flight-combatevidence-{ac}",
                lane="flight",
                args=["--combat-smoke", "--aircraft", ac, "--no-audio"],
                env={"TORE_COMBAT_EVIDENCE": f".local/battery/evidence/{stamp}-{ac}"},
                check=check_combat_evidence,
                timeout=300,
            )
        )
    out.append(Scenario(name="flight-sensor-summary", lane="flight", args=["--sensor-summary", "--no-audio"], expect=[r"radar .* search"], timeout=120))
    out.append(Scenario(name="flight-validate-weather", lane="flight", args=["--validate-weather", "--no-audio"], timeout=600))
    out.append(Scenario(name="flight-validate-maps", lane="flight", args=["--validate-maps", "--no-audio"], timeout=600))
    return out


# --------------------------------------------------------------- windowed


# Stations whose weapon is a surface missile or similar and refuses the aircraft
# fixture, so the trigger must not spend a round (docs/spec/missiles.md).
REFUSED_SLOTS = {
    ("f18", 3), ("f18", 4), ("f22", 3), ("f22n", 3), ("faxx", 3), ("mig23", 3), ("mig23", 4),
    ("rafale", 2), ("su25", 3), ("x31", 3),
}


def check_slot(ac: str, slot: int):
    capacity = STATIONS[ac]
    refused = (ac, slot) in REFUSED_SLOTS

    def check(output: str) -> list[str]:
        problems = []
        m = re.search(r"Combat probe: .* shots=(\d+) hits=(\d+) kills=(\d+) active=(\d+) ammo=\[([\d, ]+)\]", output)
        if not m:
            return ["no Combat probe line"]
        shots, hits, kills = (int(m.group(i)) for i in range(1, 4))
        ammo = [int(v) for v in m.group(5).split(",")]
        if len(ammo) != len(capacity):
            return [f"{len(ammo)} stations, expected {len(capacity)}"]
        for index, (left, cap) in enumerate(zip(ammo, capacity)):
            if left < 0 or left > cap:
                problems.append(f"station {index + 1} ammo {left} outside 0..{cap}")
        if capacity[slot - 1] == 0:
            # A never-loaded station cannot be selected: the run starts on NAV
            # and the probe fires whatever the cycle reaches, so this slot only
            # proves the harness does not hang and the station stays empty.
            if ammo[slot - 1] != 0:
                problems.append(f"the never-loaded station {slot} holds {ammo[slot - 1]}")
            used = sum(cap - left for left, cap in zip(ammo, capacity))
            if used != shots:
                problems.append(f"{shots} shots but the stations used {used}")
            if hits > shots or kills > hits:
                problems.append(f"impossible tally shots={shots} hits={hits} kills={kills}")
            return problems
        if refused and shots != 0:
            problems.append(f"a surface weapon fired at the practice aircraft ({shots} shots)")
        if not refused and shots == 0:
            problems.append("the trigger fired nothing")
        fired_slot = capacity[slot - 1] - ammo[slot - 1]
        if fired_slot != shots:
            problems.append(f"fired {shots} but the station lost {fired_slot}")
        for index, (left, cap) in enumerate(zip(ammo, capacity)):
            if index != slot - 1 and left != cap:
                problems.append(f"station {index + 1} changed ({cap}->{left}) while firing station {slot}")
        if hits > shots or kills > hits:
            problems.append(f"impossible tally shots={shots} hits={hits} kills={kills}")
        return problems

    return check


def slot_scenarios() -> list[Scenario]:
    out = []
    for ac, capacity in STATIONS.items():
        if ac == "faxx":
            continue
        for slot in range(1, len(capacity) + 1):
            out.append(
                Scenario(
                    name=f"flight-livefire-{ac}-slot{slot}",
                    lane="flight",
                    args=["--live-fire", "--aircraft", ac, "--weapon-slot", str(slot), "--combat-probe-ticks", "1200", "--capture-flight", "{work}/shot.ppm", "--no-audio"],
                    window=True,
                    check=check_slot(ac, slot),
                    timeout=180,
                )
            )
    for ac in ("f22", "f22n"):
        for row in (5, 7, 8):
            out.append(Scenario(
                name=f"flight-bay-fire-{ac}-row{row}", lane="flight",
                args=["--stores-preview", "{work}/frames"], window=True,
                env={"TORE_STORES_PREVIEW_AIRCRAFT": ac,
                     "TORE_STORES_PREVIEW_FULL": "1",
                     "TORE_STORES_PREVIEW_FIRE": str(row)},
                expect=[f"Stores firing: {ac} row={row} PASS"],
                outputs=[f"frames/{ac}-fire-row{row}-t000.png", f"frames/{ac}-fire-row{row}-t400.png"],
                timeout=300,
                notes="Actual one-press release, bay-only gate, hold and closing; main bay, side bay and wing, with rendered projectile frames.",
            ))
    return out


def check_jettison(ac: str, slot: int):
    capacity = STATIONS[ac]

    def check(output: str) -> list[str]:
        m = re.search(
            r"Combat probe: .* shots=(\d+) .* ammo=\[([\d, ]+)\] payload_start_lb=(\d+) payload_lb=(\d+) flight_payload_lb=(\d+) external_fuel_lb=(\d+)",
            output,
        )
        if not m:
            return ["no Combat probe line with payload figures"]
        shots = int(m.group(1))
        ammo = [int(v) for v in m.group(2).split(",")]
        start, payload, flight_payload = (int(m.group(i)) for i in (3, 4, 5))
        problems = []
        jettisoned = not (payload == start and ammo[slot - 1] == capacity[slot - 1] - shots)
        if not jettisoned:
            # An internal station (for example the Su-35's slot 5) cannot be
            # jettisoned: the load does not change and it still fires normally.
            if shots == 0 and (ac, slot) not in REFUSED_SLOTS:
                problems.append("a station neither jettisoned nor fired")
        else:
            if ammo[slot - 1] != 0:
                problems.append(f"station {slot} still holds {ammo[slot - 1]} after a jettison")
            if not payload < start:
                problems.append(f"jettison did not lighten the load: {start} -> {payload} lb")
        # The flight model carries the store weight less the fuel already used.
        if not payload - 30 <= flight_payload <= payload:
            problems.append(f"flight carries {flight_payload} lb but the stores weigh {payload} lb")
        others = [(index, left, cap) for index, (left, cap) in enumerate(zip(ammo, capacity)) if index != slot - 1]
        if jettisoned:
            # The emptied station cannot be selected, so the selection moves on
            # to the next loaded station and the probe's shots come from there:
            # every shot is accounted for by other stations' ammunition.
            used = sum(cap - left for _, left, cap in others)
            if used != shots:
                problems.append(f"{shots} shots but the other stations used {used} after station {slot} was jettisoned")
        else:
            for index, left, cap in others:
                if left != cap:
                    problems.append(f"station {index + 1} changed ({cap}->{left}) when station {slot} was jettisoned")
        return problems

    return check


def jettison_scenarios() -> list[Scenario]:
    out = []
    for ac, capacity in STATIONS.items():
        if ac == "faxx":
            continue
        for slot in range(2, len(capacity) + 1):
            if capacity[slot - 1] == 0:
                continue  # nothing is loaded there to jettison
            out.append(
                Scenario(
                    name=f"flight-jettison-{ac}-slot{slot}",
                    lane="flight",
                    args=["--live-fire", "--aircraft", ac, "--weapon-slot", str(slot), "--combat-command", "jettison", "--combat-probe-ticks", "300", "--capture-flight", "{work}/j.ppm", "--no-audio"],
                    window=True,
                    check=check_jettison(ac, slot),
                    timeout=180,
                )
            )
    return out


def check_countermeasures(output: str) -> list[str]:
    m = re.search(
        r"Countermeasure preview: ticks=\d+ flares=(\d+) burning=(\d+) puffs=(\d+) chaff=(\d+) carried_chaff=(\d+) carried_flares=(\d+) capacity_chaff=(\d+) capacity_flares=(\d+)",
        output,
    )
    if not m:
        return ["no Countermeasure preview line"]
    flares, burning, puffs, chaff, carried_chaff, carried_flares, cap_chaff, cap_flares = (int(m.group(i)) for i in range(1, 9))
    problems = []
    if burning > flares:
        problems.append("more flares burning than flares in the air")
    if carried_chaff > cap_chaff or carried_flares > cap_flares:
        problems.append("more countermeasures carried than the aircraft's capacity")
    # Five chaff releases and three flare releases were commanded.
    if carried_chaff != max(cap_chaff - 5, 0):
        problems.append(f"5 chaff releases but chaff {cap_chaff} -> {carried_chaff}")
    if carried_flares != max(cap_flares - 3, 0):
        problems.append(f"3 flare releases but flares {cap_flares} -> {carried_flares}")
    if cap_chaff >= 5 and chaff != 5:
        problems.append(f"{chaff} chaff cartridges in the air, expected 5")
    return problems


def countermeasure_scenarios() -> list[Scenario]:
    out = []
    for ac in AIRCRAFT:
        args = ["--live-fire", "--aircraft", ac]
        args += ["--combat-command", "chaff"] * 5
        args += ["--combat-command", "flare"] * 3
        args += ["--countermeasure-preview", "240", "--capture-flight", "{work}/cm.ppm", "--no-audio"]
        out.append(Scenario(name=f"flight-countermeasures-{ac}", lane="flight", args=args, window=True, check=check_countermeasures, timeout=180))
    return out


# ---------------------------------------------------------------- climb-out


def check_climbout(output: str) -> list[str]:
    """The scripted leader takes off, raises gear and flaps and cruises."""
    problems = []
    if "player crashed=false" not in output:
        problems.append("the player crashed or the probe did not finish")
    # An aircraft short of speed for its clean wing keeps its flaps down until it has it.
    if not re.search(r"player: airborne, gear (and flaps )?up", output):
        problems.append("never got airborne")
    if "flaps held for speed" in output and "player: flaps up" not in output:
        problems.append("the flaps were held for speed and never came up")
    if "levelling off at" not in output:
        problems.append("never reached cruise height")
    if not re.search(r"AI probe liftoff gaps: \[\] hazards_open=0", output):
        problems.append("ground hazards or liftoff gaps reported")
    if "OFF-MAP" in output:
        problems.append("a wing member started off the map")
    return problems


def climbout_scenarios() -> list[Scenario]:
    out = []
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"flight-climbout-{ac}",
                lane="flight",
                # Kherson: Krasnodar (airport 3) is a Redfor field (slice AL1).
                args=["--theater", "UKR", "--ground-start", "11", "--ai-probe-ticks", "16000", "--maneuver", "takeoff", "--probe-wing-size", "1", "--probe-wing-only", "--aircraft", ac, "--no-audio"],
                check=check_climbout,
                timeout=240,
            )
        )
    # A whole wing of five leaving the first Blue airport of every base
    # theater (slice AL1); the Falklands and North Vietnam have none.
    for theater in BASE_THEATERS:
        airport = ground_airport(theater, 1)
        if airport is None:
            continue
        out.append(
            Scenario(
                name=f"flight-climbout-wing5-{_theater_tag(theater)}",
                lane="flight",
                args=["--theater", theater, "--ground-start", str(airport), "--ai-probe-ticks", "9000", "--maneuver", "takeoff", "--probe-wing-size", "5", "--probe-wing-only", "--no-audio"],
                check=check_climbout,
                timeout=240,
            )
        )
    return out


# --------------------------------------------------------------- autopilot


def check_autopilot(output: str) -> list[str]:
    problems = extremes_problems(output)
    n = _plain_numbers(output)
    e = _numbers(output, "extremes:")
    try:
        if abs(float(n["bank_deg"])) > 2:
            problems.append(f"autopilot left {n['bank_deg']} deg of bank")
        if abs(float(n["altitude_ft"]) - 5000) > 100:
            problems.append(f"autopilot did not hold 5,000 ft: {n['altitude_ft']}")
        if float(e["min_altitude_ft"]) < 4850:
            problems.append(f"autopilot let the aircraft sink to {e['min_altitude_ft']} ft")
        if float(e["max_g"]) > 2:
            problems.append(f"autopilot pulled {e['max_g']} G")
    except (KeyError, ValueError):
        problems.append("no result line")
    return problems


def autopilot_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-autopilot-{ac}",
            lane="flight",
            args=["--headless-flight", "3600", "--aircraft", ac, "--maneuver", "autopilot", "--no-audio"],
            check=check_autopilot,
            timeout=120,
        )
        for ac in AIRCRAFT
    ]


# ---------------------------------------------------------------- ejection


def make_check_eject(maneuver: str):
    def check(output: str) -> list[str]:
        problems = extremes_problems(output, engine_off_ok=True)
        m = re.search(r"^ejection=(\w+) pilot_alive=(\w+) pilot_position=\[([^\]]+)\]", output, re.M)
        if not m:
            return problems + ["no ejection line (the seat never fired)"]
        phase, alive, position = m.group(1), m.group(2), [float(v) for v in m.group(3).split(",")]
        if phase != "Landed":
            problems.append(f"the pilot was still {phase} after the run")
        if maneuver == "eject" and alive != "true":
            problems.append("the pilot died ejecting at 5,000 ft in level flight")
        if abs(position[1]) > 60:
            problems.append(f"the pilot came to rest at {position[1]:.0f} ft above the ground")
        # A two-seater ejects its second crew member too (John, 2026-10-06):
        # a second chute, down by the end like the pilot's. A single seat
        # prints neither line.
        crew = re.search(r"^crew_ejection=(\w+) crew_position=\[([^\]]+)\]", output, re.M)
        two_seater = re.search(r"^two_seater=true", output, re.M) is not None
        if two_seater and not crew:
            problems.append("a two-seater ejected only its pilot")
        if crew and not two_seater:
            problems.append("a single seat ejected a second crew member")
        if crew:
            if crew.group(1) != "Landed":
                problems.append(f"the second crew member was still {crew.group(1)} after the run")
            height = float(crew.group(2).split(",")[1])
            if abs(height) > 60:
                problems.append(f"the second crew member came to rest at {height:.0f} ft above the ground")
        return problems

    return check


def eject_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-{maneuver}-{ac}",
            lane="flight",
            args=["--headless-flight", "30000", "--aircraft", ac, "--maneuver", maneuver, "--no-audio"],
            check=make_check_eject(maneuver),
            timeout=120,
        )
        for ac in AIRCRAFT
        for maneuver in ("eject", "eject-low")
    ]


# ----------------------------------------------------------------- weather


def frame_problems(output: str, night_ok: bool = False) -> list[str]:
    """Read the captured frame the app names and flag a blank or uniform picture."""
    m = re.search(r"Scene capture: (\S+)", output)
    if not m:
        return ["no Scene capture line"]
    path = m.group(1)
    if not os.path.isabs(path):
        path = os.path.join(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))), path)
    try:
        with open(path, "rb") as f:
            data = f.read()
    except OSError as error:
        return [f"capture unreadable: {error}"]
    header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
    if not header:
        return ["capture is not a P6 PPM"]
    width, height = int(header.group(1)), int(header.group(2))
    pixels = data[header.end():]
    if len(pixels) < width * height * 3:
        return ["capture is truncated"]
    step = 3 * 37
    sample = pixels[: width * height * 3 : step]
    if len(set(sample[i : i + 3] for i in range(0, len(sample) - 2, 3))) < 40:
        return ["the frame is a flat colour (blank render)"]
    mean = sum(sample) / len(sample)
    if mean < 2 and not night_ok:
        return [f"the frame is black (mean {mean:.1f})"]
    return []


def weather_scenarios() -> list[Scenario]:
    out = []
    for theater in BASE_THEATERS:
        for condition in range(6):
            out.append(
                Scenario(
                    name=f"flight-weather{condition}-{_theater_tag(theater)}",
                    lane="flight",
                    args=["--free-flight", "--theater", theater, "--weather-condition", str(condition), "--capture-flight", "{work}/frame.ppm", "--no-audio"],
                    window=True,
                    check=lambda out, night=(condition == 5): frame_problems(out, night),
                    timeout=180,
                )
            )
    return out


# ------------------------------------------------------------------ cheats


def make_check_cheat(cheat: str):
    def check(output: str) -> list[str]:
        problems = extremes_problems(output, engine_off_ok=True)
        n = _plain_numbers(output)
        e = _numbers(output, "extremes:")
        try:
            if cheat == "extra-g":
                if not 8.5 <= float(e["max_g"]) <= 9.5:
                    problems.append(f"Pull extra G reached {e['max_g']} G, expected about 9")
            elif cheat == "no-g-effects":
                if float(e["max_blackout"]) != 0 or float(e["max_redout"]) != 0:
                    problems.append("No redout or blackout on but the view still greyed or reddened")
            elif cheat == "no-spins":
                if n.get("spin_direction") != "0" or "Spinning" in n.get("departure_alert", ""):
                    problems.append("No spins on but the aircraft spun")
            elif cheat == "no-crashes":
                if n.get("crashed") != "false":
                    problems.append("No crashes on but the aircraft crashed")
            elif cheat == "unlimited-fuel":
                if abs(float(e["fuel_end_lb"]) - float(e["fuel_start_lb"])) > 0.01:
                    problems.append("Unlimited fuel on but the fuel went down")
        except (KeyError, ValueError):
            problems.append("no result line")
        return problems

    return check


def cheat_scenarios() -> list[Scenario]:
    out = []
    plan = [("extra-g", "pull"), ("no-g-effects", "pull"), ("no-spins", "spin"), ("no-crashes", "roll"), ("unlimited-fuel", "loop")]
    for ac in AIRCRAFT:
        for cheat, maneuver in plan:
            out.append(
                Scenario(
                    name=f"flight-cheat-{cheat}-{ac}",
                    lane="flight",
                    args=["--headless-flight", "7200", "--aircraft", ac, "--maneuver", maneuver, "--flight-cheat", cheat, "--no-audio"],
                    check=make_check_cheat(cheat),
                    timeout=120,
                )
            )
    return out


def check_unlimited_ammo(ac: str, slot: int):
    capacity = STATIONS[ac]

    def check(output: str) -> list[str]:
        m = re.search(r"Combat probe: .* shots=(\d+) hits=(\d+) kills=(\d+) active=(\d+) ammo=\[([\d, ]+)\]", output)
        if not m:
            return ["no Combat probe line"]
        shots = int(m.group(1))
        ammo = [int(v) for v in m.group(5).split(",")]
        problems = []
        if shots == 0:
            problems.append("nothing was fired")
        # The gun's count is set by the first round; a missile station keeps its full count.
        if slot > 1 and ammo[slot - 1] != capacity[slot - 1]:
            problems.append(f"Unlimited ammo on but station {slot} went {capacity[slot - 1]} -> {ammo[slot - 1]}")
        if slot == 1 and ammo[0] < capacity[0] - 1:
            problems.append(f"Unlimited ammo on but the gun went {capacity[0]} -> {ammo[0]}")
        return problems

    return check


def make_check_damage_cheat(mode: str):
    def check(output: str) -> list[str]:
        m = re.search(r"\| HP (\d+) SYS (\S+) ", output)
        if not m:
            return ["no status line with HP"]
        hp, sys = int(m.group(1)), m.group(2)
        problems = []
        if mode == "invulnerable":
            if hp == 0 or "LAUNCHER LOST" in output:
                problems.append(f"Invulnerable but the aircraft was destroyed (HP {hp})")
        elif mode == "normal":
            if sys != "--":
                problems.append(f"Normal damage but system faults appeared: SYS {sys}")
        elif mode == "realistic":
            if sys == "--" and hp < 232:
                problems.append("Realistic damage took hit points but caused no system fault")
        return problems

    return check


def cheat_combat_scenarios() -> list[Scenario]:
    out = []
    # The first air-to-air missile station of each aircraft (the Rafale's second
    # station is a surface missile that refuses the practice aircraft).
    for ac, missile_slot in [("f18", 2), ("rafale", 3), ("f14", 2), ("mig29", 2), ("su27", 2), ("f22", 2)]:
        for slot in (1, missile_slot):
            out.append(
                Scenario(
                    name=f"flight-cheat-unlimited-ammo-{ac}-slot{slot}",
                    lane="flight",
                    args=["--live-fire", "--aircraft", ac, "--weapon-slot", str(slot), "--flight-cheat", "unlimited-ammo", "--combat-probe-ticks", "1200", "--capture-flight", "{work}/c.ppm", "--no-audio"],
                    window=True,
                    check=check_unlimited_ammo(ac, slot),
                    timeout=180,
                )
            )
    for ac in ["f18", "su27", "f22"]:
        for mode, flags in [("invulnerable", ["--flight-cheat", "invulnerable"]), ("normal", []), ("realistic", ["--flight-cheat", "realistic-damage"])]:
            if mode == "realistic" and ac != "f18":
                continue
            out.append(
                Scenario(
                    name=f"flight-cheat-damage-{mode}-{ac}",
                    lane="flight",
                    args=["--live-fire", "--aircraft", ac, "--weapon-slot", "2", *flags, "--combat-command", "incoming", "--combat-command", "incoming", "--combat-probe-ticks", "1500", "--capture-flight", "{work}/c.ppm", "--no-audio"],
                    window=True,
                    check=make_check_damage_cheat(mode),
                    timeout=180,
                )
            )
    return out


def check_waypoint(output: str) -> list[str]:
    """The waypoint autopilot turns right toward waypoint 1 (60,000 ft out, 60
    degrees right of north) from a heading of 17 degrees and holds the altitude
    it captured. After 25 seconds it has turned well toward it without
    overshooting past the bearing."""
    problems = extremes_problems(output)
    f = _numbers(output, "final_position:")
    e = _numbers(output, "extremes:")
    try:
        heading = float(f["heading_deg"])
        if not 30 <= heading <= 75:
            problems.append(f"heading {heading:.1f} after 25 s, expected a turn from 17 toward 60 degrees")
        if float(e["max_altitude_ft"]) - float(e["min_altitude_ft"]) > 80:
            problems.append("the altitude hold wandered more than 80 ft")
    except (KeyError, ValueError):
        problems.append("no result line")
    return problems


def waypoint_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-waypoint-{ac}",
            lane="flight",
            args=["--headless-flight", "3000", "--aircraft", ac, "--maneuver", "waypoint", "--no-audio"],
            check=check_waypoint,
            timeout=120,
        )
        for ac in AIRCRAFT
    ]


def check_climb(output: str) -> list[str]:
    """A full-power climb and the dive after it. Overspeed loses an aircraft at
    sustained time above its own top speed (docs/spec/overspeed.md); the ceiling is
    enforced by the density rule in docs/FLIGHT-MODEL.md, so a zoom climb carries them at most a fifth past it. These limits are today's
    behaviour with a margin, so a change for the worse is caught; the speed limit
    is not a specification."""
    problems = extremes_problems(output, engine_off_ok=True, beyond_envelope_ok=True)
    c = _numbers(output, "climb:")
    if not c:
        return problems + ["no climb: line"]
    try:
        if float(c["ceiling_ft"]) <= 0:
            problems.append("the aircraft has no envelope ceiling")
        if float(c["over_ceiling"]) > 1.3:
            problems.append(f"climbed to {c['max_altitude_ft']} ft, {c['over_ceiling']} of its {c['ceiling_ft']} ft ceiling")
        if float(c["max_altitude_ft"]) < 0.6 * float(c["ceiling_ft"]):
            problems.append(f"could only climb to {c['max_altitude_ft']} ft of a {c['ceiling_ft']} ft ceiling")
    except (KeyError, ValueError):
        problems.append("no result line")
    return problems


def check_sprint(output: str) -> list[str]:
    """Full afterburner in level flight at 5,000 ft settles near the aircraft's
    own top speed instead of running away."""
    problems = extremes_problems(output, engine_off_ok=True, beyond_envelope_ok=True)
    e = _numbers(output, "extremes:")
    try:
        if float(e["speed_over_envelope_top"]) > 1.08:
            problems.append(f"level afterburner flight reached {e['speed_over_envelope_top']} times the envelope's top speed")
        # Near the top of its envelope only the 1 G row is left, and a loaded
        # aircraft cannot hold 1 G there: it sinks (see "Needs a decision"), so
        # the altitude hold is not checked once the speed is above 90% of it.
        if float(e["speed_over_envelope_top"]) <= 0.9 and float(e["max_altitude_ft"]) - float(e["min_altitude_ft"]) > 80:
            problems.append("the autopilot altitude hold wandered more than 80 ft in the sprint")
    except (KeyError, ValueError):
        problems.append("no result line")
    return problems


def climb_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-climb-{ac}",
            lane="flight",
            args=["--headless-flight", "216000", "--aircraft", ac, "--maneuver", "climb", "--no-audio"],
            check=check_climb,
            timeout=240,
        )
        for ac in AIRCRAFT
    ]


def sprint_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-sprint-{ac}",
            lane="flight",
            args=["--headless-flight", "36000", "--aircraft", ac, "--maneuver", "sprint", "--no-audio"],
            check=check_sprint,
            timeout=240,
        )
        for ac in AIRCRAFT
    ]


# -------------------------------------------------------------------- fuel


def check_fuel_out(output: str) -> list[str]:
    """Run out of fuel in level flight: the fuel stops at zero and the dead
    engine gives no energy."""
    problems = extremes_problems(output)
    e = _numbers(output, "extremes:")
    n = _plain_numbers(output)
    try:
        if abs(float(e["fuel_end_lb"])) > 1e-6:
            problems.append(f"25 lb of fuel did not run out in 100 seconds of level flight: {e['fuel_end_lb']} lb left")
        if float(n["fuel_lb"]) < 0:
            problems.append("negative fuel")
        if n.get("crashed") != "false":
            problems.append("crashed after running out of fuel in level flight at 5,000 ft")
    except (KeyError, ValueError):
        problems.append("no result line")
    return problems


def fuel_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-fuelout-{ac}",
            lane="flight",
            args=["--headless-flight", "12000", "--aircraft", ac, "--maneuver", "level", "--flight-fuel", "25", "--no-audio"],
            check=check_fuel_out,
            timeout=120,
        )
        for ac in AIRCRAFT
    ]


# ----------------------------------------------------------------- devices


def check_devices(output: str) -> list[str]:
    problems = extremes_problems(output)
    d = _numbers(output, "devices:")
    if not d:
        return problems + ["no devices: line"]
    if d["violations"] != "0":
        problems.append(f"device moved wrongly: {d['first_violation']}")
    travel = float(d["deployment_seconds"])
    for name in ("gear", "flaps", "brake", "hook"):
        if name == "hook" and d["hook_available"] != "true":
            if d["hook_down_s"] != "none":
                problems.append("an aircraft with no hook lowered one")
            continue
        for way in ("down", "up"):
            value = d[f"{name}_{way}_s"]
            if value == "none":
                problems.append(f"{name} never finished going {way}")
            elif abs(float(value) - travel) > 0.1:
                problems.append(f"{name} took {value} s going {way}, the aircraft's travel is {travel} s")
    return problems


def device_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-devices-{ac}",
            lane="flight",
            args=["--headless-flight", "3600", "--aircraft", ac, "--maneuver", "devices", "--no-audio"],
            check=check_devices,
            timeout=120,
        )
        for ac in AIRCRAFT
    ]


# ----------------------------------------------------------- damage visuals


def capture_scenarios() -> list[Scenario]:
    """Windowed captures of the damage, ejection and animation fixtures, each
    checked for a blank frame. The images themselves need a human eye."""
    out = []

    def add(name: str, args: list[str]) -> None:
        out.append(
            Scenario(
                name=f"flight-{name}",
                lane="flight",
                args=[*args, "--capture-flight", "{work}/frame.ppm", "--no-audio"],
                window=True,
                check=frame_problems,
                timeout=180,
            )
        )

    for label, distance in (("close", 500), ("long", 60000)):
        for view in (0, 8, 9):
            add(f"target-camera-{label}-view{view}", ["--free-flight", "--flight-view", str(view),
                "--hud-target-preview", f"0,0,{distance}"])
    for ac in AIRCRAFT:
        for fraction in ("0.5", "1"):
            add(f"damage-{ac}-{fraction}", ["--free-flight", "--aircraft", ac, "--flight-view", "1", "--damage-preview", fraction, "--damage-preview-section", "core"])
        for pose in ("seat", "freefall", "chute"):
            add(f"ejectionpose-{ac}-{pose}", ["--free-flight", "--aircraft", ac, "--flight-view", "1", "--ejection-preview", pose])
    for section in ("nose", "cockpit", "core", "left-wing", "right-wing", "tail"):
        for fraction in ("0.3", "0.7", "1"):
            add(f"damage-f18-{section}-{fraction}", ["--free-flight", "--aircraft", "f18", "--flight-view", "1", "--damage-preview", fraction, "--damage-preview-section", section])
    # Odesa (UKR 9), a Blue field: Zaporizhzhya (UKR 1) is Redfor since slice
    # AL1 and refuses a ground start ("an enemy airfield").
    for ac in AIRCRAFT:
        add(f"groundstart-{ac}", ["--free-flight", "--aircraft", ac, "--theater", "UKR", "--ground-start", "9", "--flight-view", "1", "--flight-look", "150,-15", "--flight-zoom", "1.5"])
    for ac in ("f22", "f22n"):
        for bay in ("0", "0.5", "1"):
            add(f"bay-{ac}-{bay}", ["--free-flight", "--aircraft", ac, "--flight-view", "1", "--flight-look", "150,-30", "--flight-bay", bay])
    return out


# ------------------------------------------------------------- world edges

# Terrain grid (columns, rows) of each base theater; the map runs 0 to
# (cells - 1) * 8,192 feet on each axis.
GRIDS = {
    "APA": (256, 256), "BAL": (256, 256), "CUB": (256, 256), "EGY": (208, 200), "FRA": (208, 200),
    "GRE": (256, 256), "IRA": (256, 256), "KURILE": (256, 256), "LFA": (256, 256), "NSK": (256, 256),
    "PGU": (256, 256), "SPA": (256, 256), "TVIET": (200, 200), "UKR": (208, 200), "VLA": (208, 200),
    "WTA": (256, 256),
}


NAUTICAL_MILE_FT = 6076.11549


def _edge_nm(theater: str, x: float, z: float) -> float:
    """Distance in nautical miles from the map rectangle, 0 inside it."""
    cols, rows = GRIDS[theater]
    dx = max(0.0, -x, x - (cols - 1) * 8192)
    dz = max(0.0, -z, z - (rows - 1) * 8192)
    return (dx * dx + dz * dz) ** 0.5 / NAUTICAL_MILE_FT


def check_edge(output: str) -> list[str]:
    """Flying straight out over a map edge at 20,000 ft for 200 seconds (well
    inside the 100 nautical mile warning line): the aircraft must keep flying
    with finite numbers and no loss."""
    problems = extremes_problems(output)
    n = _plain_numbers(output)
    if n.get("crashed") != "false":
        problems.append("crashed flying out over the map edge at 20,000 ft")
    if "final_position:" not in output:
        problems.append("no final position")
    if "loss: cause=none" not in output:
        problems.append("the short edge flight reported a loss")
    return problems


def check_edge_lost(output: str, theater: str) -> list[str]:
    """Flying on out to sea, the aircraft is lost 105 nautical miles past the
    map (docs/spec/world-edge.md): the same destroyed path as any loss, with
    the cause out of bounds."""
    problems = extremes_problems(output)
    if _plain_numbers(output).get("crashed") != "true":
        problems.append("the aircraft was not lost past the edge of the world")
    if "loss: cause=out of bounds" not in output:
        problems.append("the loss did not name out of bounds as the cause")
    m = re.search(r"final_position: x=(-?[\d.]+) z=(-?[\d.]+)", output)
    if not m:
        return problems + ["no final position"]
    # The wreck falls on from the loss point, so allow a few miles past 105.
    nm = _edge_nm(theater, float(m.group(1)), float(m.group(2)))
    if not 104.5 <= nm <= 112:
        problems.append(f"lost {nm:.1f} nautical miles past the map, not about 105")
    return problems


def edge_scenarios() -> list[Scenario]:
    out = []
    for theater, (cols, rows) in GRIDS.items():
        width, depth = (cols - 1) * 8192, (rows - 1) * 8192
        for edge, x, z, heading in [
            ("west", 20000, depth // 2, 270),
            ("east", width - 20000, depth // 2, 90),
            ("south", width // 2, 20000, 180),
            ("north", width // 2, depth - 20000, 0),
        ]:
            out.append(
                Scenario(
                    name=f"flight-edge-{edge}-{_theater_tag(theater)}",
                    lane="flight",
                    args=["--theater", theater, "--headless-flight", "24000", "--flight-start", f"{x},{z},{heading},20000", "--no-audio"],
                    check=check_edge,
                    timeout=180,
                )
            )
            if theater == "UKR":
                out.append(
                    Scenario(
                        name=f"flight-edge-lost-invulnerable-{edge}-{_theater_tag(theater)}",
                        lane="flight",
                        args=["--theater", theater, "--headless-flight", "150000", "--flight-start", f"{x},{z},{heading},20000", "--flight-cheat", "invulnerable", "--no-audio"],
                        check=lambda output, theater=theater: check_edge_lost(output, theater),
                        timeout=300,
                    )
                )
            out.append(
                Scenario(
                    name=f"flight-edge-lost-{edge}-{_theater_tag(theater)}",
                    lane="flight",
                    args=["--theater", theater, "--headless-flight", "150000", "--flight-start", f"{x},{z},{heading},20000", "--no-audio"],
                    check=lambda output, theater=theater: check_edge_lost(output, theater),
                    timeout=300,
                )
            )
    return out


def check_overspeed_loss(output: str) -> list[str]:
    """A continuous 1.1 times the top speed fixture: lost at 6 to 10 seconds, with the
    cause overspeed (docs/spec/overspeed.md)."""
    problems = []
    if _plain_numbers(output).get("crashed") != "true":
        problems.append("the aircraft survived ten continuous seconds above its top speed")
    if "loss: cause=overspeed" not in output:
        problems.append("the loss did not name overspeed as the cause")
    timer = re.search(r"^overspeed_ticks=(\d+)$", output, re.M)
    if timer is None or int(timer.group(1)) not in (720, 840, 960, 1080, 1200):
        problems.append("loss was not on a 6 to 10 second overspeed boundary")
    return problems


def check_overspeed_invulnerable(output: str) -> list[str]:
    """The same continuous overspeed fixture with the Invulnerable cheat: not lost."""
    problems = []
    if _plain_numbers(output).get("crashed") != "false":
        problems.append("an invulnerable player was lost to overspeed")
    if "loss: cause=none" not in output:
        problems.append("an invulnerable player's overspeed reported a loss")
    return problems


def check_dive(output: str) -> list[str]:
    """A full afterburner dive from 40,000 ft with nobody pulling out ends on
    the ground, or in the time-based overspeed loss. Peak speed alone is no
    longer the failure trigger."""
    problems = extremes_problems(output, engine_off_ok=True, beyond_envelope_ok=True)
    if _plain_numbers(output).get("crashed") != "true":
        problems.append("an unattended 60 degree dive from 40,000 ft did not end in a loss")
    if "loss: cause=" not in output:
        problems.append("no loss line")
    return problems


def check_rudder_path(output: str) -> list[str]:
    values = _numbers(output, "lateral:")
    problems = extremes_problems(output, engine_off_ok=True)
    try:
        if float(values["velocity_x_fps"]) <= 10 or float(values["bank_deg"]) <= 0.5:
            problems.append("rudder did not turn the unpowered aircraft's path and bank it")
    except (KeyError, ValueError):
        problems.append("no lateral response report")
    return problems


def check_nosewheel(output: str) -> list[str]:
    values = _numbers(output, "lateral:")
    problems = extremes_problems(output, engine_off_ok=True)
    try:
        if abs(float(values["wheel_deg"]) - 90) > 0.01:
            problems.append("low-speed nosewheel did not reach 90 degrees")
        if _plain_numbers(output).get("crashed") != "false":
            problems.append("taxi steering crashed the aircraft")
    except (KeyError, ValueError):
        problems.append("no nosewheel report")
    return problems


def lateral_scenarios() -> list[Scenario]:
    return [Scenario(name=f"flight-lateral-{maneuver}-{ac}", lane="flight",
                     args=["--headless-flight", "360", "--maneuver", maneuver,
                           "--aircraft", ac, "--no-audio", *extra], check=check)
            for ac in ("f18", "rafale")
            for maneuver, extra, check in (
                ("rudder", [], check_rudder_path),
                ("nosewheel", ["--theater", "UKR", "--ground-start", "1"], check_nosewheel))]


def overspeed_scenarios() -> list[Scenario]:
    out = []
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"flight-overspeed-loss-{ac}",
                lane="flight",
                args=["--headless-flight", "1200", "--maneuver", "overspeed", "--aircraft", ac, "--no-audio"],
                check=check_overspeed_loss,
            )
        )
        out.append(
            Scenario(
                name=f"flight-overspeed-invulnerable-{ac}",
                lane="flight",
                args=["--headless-flight", "1200", "--maneuver", "overspeed", "--flight-cheat", "invulnerable", "--aircraft", ac, "--no-audio"],
                check=check_overspeed_invulnerable,
            )
        )
        out.append(
            Scenario(
                name=f"flight-overspeed-dive-{ac}",
                lane="flight",
                args=["--headless-flight", "9000", "--maneuver", "dive", "--aircraft", ac, "--no-audio"],
                check=check_dive,
            )
        )
    return out


def check_belly_early(output: str, ac: str) -> list[str]:
    """Gear up pressed at 80 knots with weight on the wheels (docs/spec/gear-on-the-ground.md):
    the ground sensor refuses it, the gear stays down and the aircraft is not lost.
    Every aircraft, however fast it is at 80 knots, is refused while it rolls."""
    problems = extremes_problems(output, engine_off_ok=True)
    n = _plain_numbers(output)
    if n.get("gear_pulled") != "true":
        return problems + ["the script never pulled the gear up"]
    if n.get("crashed") == "true":
        problems.append("the aircraft crashed on the runway")
    if n.get("ground_sensor_refusals") != "1":
        problems.append(f"expected one ground sensor message, saw {n.get('ground_sensor_refusals')}")
    if n.get("gear") != "1.00":
        problems.append(f"the gear did not stay down (gear={n.get('gear')})")
    return problems


def check_belly_airborne(output: str) -> list[str]:
    """Gear up once airborne is a normal retraction, with no ground sensor message."""
    problems = extremes_problems(output, engine_off_ok=True)
    n = _plain_numbers(output)
    if "takeoff_complete=true" not in output:
        problems.append("did not take off")
    if n.get("ground_sensor_refusals") != "0":
        problems.append("an airborne gear retraction was refused by the ground sensor")
    if n.get("gear_pulled") != "true":
        problems.append("the script never pulled the gear up")
    if n.get("gear") == "1.00":
        problems.append("the gear did not come up")
    return problems


def belly_scenarios() -> list[Scenario]:
    out = []
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"flight-belly-early-{ac}",
                lane="flight",
                args=["--theater", "UKR", "--ground-start", "1", "--headless-flight", "9000", "--maneuver", "takeoff-gear-early", "--aircraft", ac, "--no-audio"],
                check=lambda output, ac=ac: check_belly_early(output, ac),
            )
        )
        out.append(
            Scenario(
                name=f"flight-belly-airborne-{ac}",
                lane="flight",
                args=["--theater", "UKR", "--ground-start", "1", "--headless-flight", "9000", "--maneuver", "takeoff-gear-airborne", "--aircraft", ac, "--no-audio"],
                check=check_belly_airborne,
            )
        )
    return out


def check_terrain_crash(output: str) -> list[str]:
    """A spin or a roll at 90 ft over any theater hits the ground: the aircraft
    crashes, and never sinks through the surface."""
    problems = extremes_problems(output, engine_off_ok=True)
    n = _plain_numbers(output)
    e = _numbers(output, "extremes:")
    if n.get("crashed") != "true":
        problems.append("a spin or roll 90 ft above the ground did not crash")
    try:
        if int(e["under_ground_ticks"]) != 0:
            problems.append(f"{e['under_ground_ticks']} ticks more than 30 ft below the surface")
        if float(e["min_agl_ft"]) < -30:
            problems.append(f"went {e['min_agl_ft']} ft below the surface")
    except (KeyError, ValueError):
        problems.append("no terrain figures in the extremes line")
    return problems


def terrain_scenarios() -> list[Scenario]:
    out = []
    for theater, (cols, rows) in GRIDS.items():
        x, z = (cols - 1) * 4096, (rows - 1) * 4096
        for maneuver in ("spin", "roll"):
            out.append(
                Scenario(
                    name=f"flight-terrain-{maneuver}-{_theater_tag(theater)}",
                    lane="flight",
                    args=["--theater", theater, "--headless-flight", "24000", "--maneuver", maneuver, "--flight-start", f"{x},{z},300,90", "--no-audio"],
                    check=check_terrain_crash,
                    timeout=180,
                )
            )
    return out


# ------------------------------------------------------------- environment


def check_environment(output: str) -> list[str]:
    """The environment probe (wind, air data, turbulence) resolves cleanly."""
    problems = []
    if "air_data=Ok(" not in output:
        problems.append("air data did not resolve")
    if "Turbulence:" not in output:
        problems.append("no turbulence line")
    return problems


def environment_scenarios() -> list[Scenario]:
    out = []
    for theater in BASE_THEATERS:
        for wind in ("0,0", "90,20", "200,100"):
            for agl in (100, 5000):
                out.append(
                    Scenario(
                        name=f"flight-environment-{_theater_tag(theater)}-w{wind.replace(',', '_')}-{agl}",
                        lane="flight",
                        args=["--free-flight", "--theater", theater, "--aircraft", "f18", "--flight-probe-ticks", "600", "--no-audio"],
                        env={"TORE_ENVIRONMENT_PROBE": "1", "TORE_FLIGHT_AGL": str(agl), "TORE_WIND": wind},
                        check=check_environment,
                        timeout=120,
                    )
                )
    return out


# ------------------------------------------------------- stripped loadouts


def make_check_loadout(kind: str, ac: str, landing: bool):
    default_carried = {}

    def check(output: str) -> list[str]:
        problems = loadout_problems(output)
        load = _numbers(output, "loadout:")
        problems += check_landing(output) if landing else check_takeoff(output)
        if load:
            # Nothing loaded can carry no more than the aircraft's own gun and
            # tanks; the default load carries at least as much.
            if float(load["carried_lb"]) > float(load["internal_fuel_lb"]) * 0.5 + 4000:
                problems.append(f"--loadout {kind} still carries {load['carried_lb']} lb")
        return problems

    return check


def loadout_scenarios() -> list[Scenario]:
    """An aircraft with nothing loaded, or only its gun, must take off, fly and
    land like any other, and carry less."""
    out = []
    for ac in AIRCRAFT:
        for kind in ("none", "guns"):
            for maneuver, ticks in (("takeoff", "9000"), ("land", "60000")):
                out.append(
                    Scenario(
                        name=f"flight-loadout-{kind}-{maneuver}-{ac}",
                        lane="flight",
                        args=["--theater", "UKR", "--ground-start", "1", "--headless-flight", ticks, "--maneuver", maneuver, "--loadout", kind, "--aircraft", ac, "--no-audio"],
                        check=make_check_loadout(kind, ac, maneuver == "land"),
                        timeout=180,
                    )
                )
    return out


# ------------------------------------------------------ passive in a fight


PLAYER_LINE = re.compile(r"^AI probe debrief: .*?player\[(\w+) damage=(\d+)% .*?enemy_aam=(\d+)/(\d+) enemy_gun=(\d+)/(\d+)(?: cause=[^\]]*)?\]", re.M)


def check_passive_fight(output: str) -> list[str]:
    """The player flies straight and level through a 5 v 5 fight. Whatever
    happens, the pilot's fate, the damage, the hit points, the crash flag and
    the debrief must agree, and no AI or player invariant may break."""
    problems = []
    m = PLAYER_LINE.search(output)
    crashed = re.search(r"^player crashed=(\w+)", output, re.M)
    hp = re.search(r"player_hp=(-?\d+)", output)
    invariants = re.search(r"^AI probe invariants: .*anomalies=(\d+)", output, re.M)
    if not (m and crashed and hp and invariants):
        return ["missing debrief, player, hit point or invariant line"]
    status, damage = m.group(1), int(m.group(2))
    aam_hits, aam_shots, gun_hits, gun_shots = (int(m.group(i)) for i in range(3, 7))
    crashed, hp = crashed.group(1) == "true", int(hp.group(1))
    # Two AI aircraft touching in a crowded 5 v 5 is the AI lane's business (it
    # happens, for example, with the F-14 as the player and enemies from the
    # rear), so it is listed in the lane page and not counted here.
    anomalies = [a for a in re.findall(r"^AI probe anomaly: (.*)$", output, re.M) if "mid-air collision" not in a]
    if anomalies:
        problems.append(f"{len(anomalies)} probe anomalies: {anomalies[0][:120]}")
    problems.extend(kill_hit_problems(output))
    if aam_hits > aam_shots or gun_hits > gun_shots:
        problems.append(f"the debrief counts more hits than shots (missiles {aam_hits}/{aam_shots}, guns {gun_hits}/{gun_shots})")
    if status == "Dead":
        if not crashed or hp != 0 or damage != 100:
            problems.append(f"debrief says Dead but crashed={crashed}, hit points {hp}, damage {damage}%")
        if aam_hits + gun_hits == 0:
            problems.append("the player is dead but nothing hit it")
    elif status == "Alive":
        if crashed or hp <= 0 or damage >= 100:
            problems.append(f"debrief says Alive but crashed={crashed}, hit points {hp}, damage {damage}%")
    elif status == "Ejected":
        if not crashed:
            problems.append("ejected but the aircraft did not crash")
    else:
        problems.append(f"unknown player status {status}")
    if status == "Alive" and (aam_hits + gun_hits) > 0 and damage == 0:
        problems.append("the player was hit but the debrief shows no damage")
    return problems


def fight_scenarios() -> list[Scenario]:
    out = []
    for n, ac in enumerate(AIRCRAFT):
        enemy = ["f18", "su27", "mig29", "f22"][n % 4]
        for geometry in ("head", "rear"):
            out.append(
                Scenario(
                    name=f"flight-fight-{ac}-{geometry}",
                    lane="flight",
                    args=[
                        "--aircraft", ac, "--probe-enemy-aircraft", enemy, "--probe-enemy-skill", "ace",
                        "--probe-geometry", geometry, "--ai-probe-ticks", "20000", "--probe-fight", "5:5",
                        "--separation", "5", "--no-audio",
                    ],
                    check=check_passive_fight,
                    timeout=600,
                )
            )
    return out


ATTACK_LINE = re.compile(
    r"^AI probe attack: clicks=(\d+) steps=(\d+) presses=(\d+) missiles=(\d+) gun_bursts=(\d+) gun_rounds=(\d+) "
    r"player_hits=(\d+) player_kills=(\d+) .* player_alive=(\w+) player_damaged=(\d+)",
    re.M,
)


def check_attacking_fight(output: str, ac: str = "") -> list[str]:
    """The scripted leader designates, selects a weapon and fires through the
    player's own controls in a 5 v 5. What it did, what the debrief says it did
    and the end state must agree."""
    problems = check_passive_fight(output)
    a = ATTACK_LINE.search(output)
    d = re.search(r"player\[(\w+) damage=\d+% kills=\[([\d, ]+)\] .*? a2a=(\d+)/(\d+) dmg=(\d+) gun=(\d+)/(\d+)", output)
    if not (a and d):
        return problems + ["missing attack or debrief line"]
    clicks, steps, presses, missiles, bursts, rounds, hits, kills = (int(a.group(i)) for i in range(1, 9))
    alive = a.group(9) == "true"
    a2a_hits, a2a_shots, damage_dealt, gun_hits, gun_shots = (int(d.group(i)) for i in (3, 4, 5, 6, 7))
    if clicks < 1:
        problems.append("the leader never designated a target")
    # A kill needs a recorded hit by the player: check_passive_fight holds both
    # columns to it (docs/spec/debrief.md, "Kills"), with no aircraft exempt.
    if kills > hits:
        problems.append(f"the probe counts {kills} kills from {hits} hits")
    if a2a_hits > a2a_shots or gun_hits > gun_shots:
        problems.append("the debrief counts more hits than shots")
    if missiles + rounds > 0 and a2a_shots + gun_shots == 0:
        problems.append("the probe fired but the debrief shows no shots")
    if presses > 0 and missiles + bursts == 0:
        problems.append("the trigger was pressed and nothing left")
    if alive != (d.group(1) == "Alive"):
        problems.append(f"the probe says player_alive={alive} but the debrief says {d.group(1)}")
    if kills > 0 and damage_dealt == 0:
        problems.append("a kill with no damage dealt in the debrief")
    return problems


def attack_scenarios() -> list[Scenario]:
    out = []
    for n, ac in enumerate(AIRCRAFT):
        out.append(
            Scenario(
                name=f"flight-attack-{ac}",
                lane="flight",
                args=[
                    "--aircraft", ac, "--probe-enemy-aircraft", ["f18", "su27", "mig29", "f22"][n % 4],
                    "--probe-enemy-skill", "average", "--ai-probe-ticks", "20000", "--probe-fight", "5:5",
                    "--separation", "10", "--probe-attack", "100:8", "--no-audio",
                ],
                check=lambda out, ac=ac: check_attacking_fight(out, ac),
                timeout=600,
            )
        )
    return out


# -------------------------------------------------------- instrument panels


def panel_scenarios() -> list[Scenario]:
    out = []
    # Every instrument page of every aircraft, and the Systems page under each
    # panel fault the preview accepts (1..35).
    for ac in AIRCRAFT:
        for page in range(10):
            out.append(
                Scenario(
                    name=f"flight-panel-{ac}-page{page}",
                    lane="flight",
                    args=["--panel-snapshot", "{work}/p.ppm", "--instrument-page", str(page), "--aircraft", ac, "--no-audio"],
                    outputs=["p.ppm"],
                    timeout=120,
                )
            )
    for fault in range(1, 36):
        out.append(
            Scenario(
                name=f"flight-panelfault{fault:02d}",
                lane="flight",
                args=["--panel-snapshot", "{work}/p.ppm", "--instrument-page", "7", "--systems-preview", str(fault), "--flight-probe-ticks", "1200", "--no-audio"],
                outputs=["p.ppm"],
                expect=[r"Damage \d+% \| TEMP \d+% OIL \d+% HYD \d+% \| Power \d+%"],
                timeout=120,
            )
        )
    # The seeker window: a launch opens the target camera when every slot is
    # taken (small: top right; large: left of the top-right window), takes a
    # free slot otherwise, and the landing closes what the launch opened.
    # --window-layout-preview draws the whole layout without a GPU and prints
    # where the camera went (a 640x480 canvas).
    popups = {
        "small": r"x=536 y=8 w=96 h=95",
        "large": r"x=300 y=8 w=162 h=160",
    }
    for layout, popup in popups.items():
        words = layout.capitalize()
        modes = {
            "before": r"popup false, opened by launch false, target camera closed",
            "launch": r"popup true, opened by launch true, target camera " + popup,
            "free": r"popup false, opened by launch true, target camera x=\d+ y=\d+",
            "landed": r"popup false, opened by launch false, target camera closed",
        }
        for mode, expected in modes.items():
            out.append(
                Scenario(
                    name=f"flight-window-seeker-{layout}-{mode}",
                    lane="flight",
                    args=[
                        "--panel-snapshot", "{work}/w.ppm", "--window-layout-preview", mode,
                        "--instrument-layout", layout, "--window-size", "640x480",
                        "--aircraft", "f16c", "--no-audio",
                    ],
                    outputs=["w.ppm"],
                    expect=[rf"window layout {words} {mode}: pages \[[\d, ]+\], {expected}"],
                    timeout=120,
                )
            )
    return out


def daytime_scenarios() -> list[Scenario]:
    """One frame at every hour of the day in three theaters."""
    out = []
    for theater in ("UKR", "TVIET", "KURILE"):
        for hour in range(24):
            out.append(
                Scenario(
                    name=f"flight-hour{hour:02d}-{_theater_tag(theater)}",
                    lane="flight",
                    args=["--free-flight", "--theater", theater, "--capture-flight", "{work}/frame.ppm", "--no-audio"],
                    env={"TORE_WEATHER_TIME": f"{hour:02d}:00"},
                    window=True,
                    check=lambda out, night=(hour < 5 or hour >= 21): frame_problems(out, night),
                    timeout=180,
                )
            )
    return out


def check_variety_flight(output: str) -> list[str]:
    problems = extremes_problems(output)
    if "flight_model=hybrid" not in output or "crashed=false" not in output:
        problems.append("variety aircraft did not complete a live hybrid flight")
    if _plain_numbers(output).get("ticks") != "1200":
        problems.append("variety flight did not complete 1,200 ticks")
    return problems


def variety_scenarios() -> list[Scenario]:
    """Every new identity loads its own app assets and flies without a display.

    Hover/conversion numerical checks live in the shared powered-flight tests
    and the source-backed variety_flight example, not the jet-only maneuvers.
    """
    return [Scenario(
        name=f"flight-variety-{aircraft}", lane="flight",
        args=["--headless-flight", "1200", "--aircraft", aircraft,
              "--researched-flight", "--no-audio", "--no-controllers"],
        check=check_variety_flight, timeout=120,
    ) for aircraft in VARIETY_AIRCRAFT]


# Transports and airliners: their higher G rows end well short of the 1 G row's
# top speed, the band where they once could not pull at all (John, 2026-10-08).
HEAVY_AIRCRAFT = ["c130", "ac130", "e3", "il76", "e2", "b747", "a310"]


def check_heavy_gcurve(output: str) -> list[str]:
    """Once the full-stick limit has passed 1.4 G it never falls back near 1 G.

    The probe's `gcurve:` lines sweep 100 to 750 kt at 5,000 ft, so the sweep
    runs from the slow-side ramp through the top speed into overspeed. Before
    the fast-side hold the limit fell from about 1.5 to 2 G to 1.00 well below
    the top speed (docs/FLIGHT-MODEL.md, "Envelope limits and loading")."""
    limits = [(int(kt), float(g)) for kt, g in re.findall(r"^gcurve: kt=(\d+) limit_g=(\S+)", output, re.M)]
    if len(limits) < 20:
        return ["gcurve sweep missing"]
    problems = []
    pulled = False
    for kt, g in limits:
        pulled = pulled or g > 1.4
        if pulled and g < 1.3:
            problems.append(f"full-stick limit fell to {g:.2f} G at {kt} kt")
    if not pulled:
        problems.append("never reached 1.4 G")
    return problems


def heavy_gcurve_scenarios() -> list[Scenario]:
    return [Scenario(
        name=f"flight-variety-gcurve-{aircraft}", lane="flight",
        args=["--headless-flight", "10", "--maneuver", "gcurve", "--aircraft", aircraft,
              "--researched-flight", "--no-audio", "--no-controllers"],
        check=check_heavy_gcurve, timeout=120,
    ) for aircraft in HEAVY_AIRCRAFT]


# The AC-130 gunsight probes (`--gunsight-probe`, crates/tore-app/src/gunsight_probe.rs):
# scripted runs on flat ground through the live combat state with the imported guns.
# name -> the PASS lines a healthy run prints at least.
GUNSIGHT_PROBES = {"pin-orbit": 9, "fire-no-target": 9, "track-out-of-arc": 11, "splash-105": 7}


def check_gunsight(name: str, minimum: int):
    def check(output: str) -> list[str]:
        problems = [line.strip()[:200] for line in output.splitlines() if line.startswith("FAIL ")]
        passes = sum(1 for line in output.splitlines() if line.startswith("PASS "))
        if f"gunsight probe {name}: PASS" not in output:
            problems.append(f"the {name} probe did not finish with PASS")
        if passes < minimum:
            problems.append(f"only {passes} PASS lines, expected at least {minimum}")
        return problems

    return check


def gunsight_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-ac130-{name}",
            lane="flight",
            args=["--gunsight-probe", name, "--aircraft", "ac130", "--no-audio"],
            check=check_gunsight(name, minimum),
            timeout=300,
        )
        for name, minimum in GUNSIGHT_PROBES.items()
    ]


# The AC-130's sensor console and orbit hold (slice nr-orbit,
# crates/tore-app/src/a2g_run/orbit.rs): the console opens over the Ukraine
# armoured column and the autopilot flies the attack orbit with no stick or
# throttle. name -> (extra arguments, notes).
ORBIT_RUNS = {
    "orbit-pin": (
        ["--orbit-sight", "pin", "--seconds", "300"],
        "The console opens on a pin at the column: enters the orbit, never inside the minimum radius or "
        "below the minimum height after entry, every gun and the camera bear 90% of the time, smooth bank "
        "and pitch, no stall.",
    ),
    "orbit-slew": (
        ["--orbit-sight", "pin", "--orbit-slew", "150", "--seconds", "330"],
        "The console opens on a pin; at 150 s the pin is slewed right for 4 s and the orbit re-centres on "
        "it without exceeding the bank and pitch rates.",
    ),
    "orbit-track": (
        ["--orbit-sight", "track", "--seconds", "300"],
        "The console opens on the tracked lead tank of the moving column and follows it.",
    ),
    "orbit-console-radar": (
        ["--orbit-sight", "pin", "--console-range", "2", "--console-cycle", "70,90", "--seconds", "120"],
        "The console turns the A-G radar on as it opens; at 70 s and 90 s the console's T (slice nr-conb) "
        "picks the next radar contact the sensor sees within 25 nm, designates it, and the gunsight tracks "
        "it one zoom step wider than the step that fills the picture.",
    ),
    "orbit-console-search-t": (
        ["--orbit-sight", "free", "--console-range", "2", "--console-cycle", "20", "--seconds", "90"],
        "The console opens searching with nothing pinned and the A-G radar on; the console's T at 20 s "
        "designates the nearest radar contact the sensor sees, the gunsight tracks it and the autopilot "
        "leaves the search for the orbit about it (slices nr-conb and nr-cona).",
    ),
    # Slice nr-cona (John, 2026-10-14): the console opens with a free sight and
    # searches straight and level; its keys step the heading, altitude and
    # speed; P pins the ground under the crosshair (orbit) and lets it go
    # (search again).
    "console-search": (
        ["--orbit-sight", "free", "--seconds", "220", "--console-keys",
         "10:heading+6,10:altitude+4,10:speed+2,90:heading-18,90:altitude-2,90:speed=25"],
        "Opens searching with nothing pinned; a 30 degree right turn with a 2,000 ft climb, then a 90 degree "
        "left turn, each settling on its heading without turning back, holding altitude, speed and wings "
        "level, with smooth bank and pitch rates and no stall.",
    ),
    "console-pin-release": (
        ["--orbit-sight", "free", "--seconds", "330", "--console-keys", "20:pin,240:pin"],
        "Opens searching; P at 20 s pins the ground under the crosshair and the autopilot joins and enters "
        "the orbit (guns and camera bear 90% after entry); P at 240 s lets it go and the search rolls out "
        "wings level on the roll-out heading without turning back.",
    ),
    "console-gun-lock": (
        ["--orbit-sight", "free", "--seconds", "240", "--console-keys",
         "20:pin,90:gunlock,100:slew,180:gunlock"],
        "Gun lock (John, 2026-10-14): P at 20 s pins and the orbit is joined; the guns lock on the pin at "
        "90 s and the sensor slews right for 4 s at 100 s; while locked every pipper stays within 300 ft of "
        "the locked point, the sight looks over 1,000 ft away and the orbit keeps circling the locked point; "
        "unlocked at 180 s the pippers go back to the sight within 20 s.",
    ),
    "orbit-close": (
        ["--orbit-sight", "pin", "--orbit-close", "120", "--seconds", "130"],
        "Ctrl+9 again at 120 s closes the console and hands the aircraft back, flying, with no snap.",
    ),
}


def check_orbit(output: str) -> list[str]:
    problems = [line.strip()[:200] for line in output.splitlines() if line.startswith("FAIL ")]
    if "orbit probe: PASS" not in output:
        problems.append("the orbit probe did not finish with PASS")
    return problems


def orbit_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name=f"flight-ac130-{name}",
            lane="flight",
            args=["--a2g-run", "UKR", "QUCOL", "--aircraft", "ac130", "--load", "C_25.JT", "--attack", "orbit",
                  "--altitude", "4000", "--speed", "200", "--from", "3", *extra],
            check=check_orbit,
            timeout=600,
            notes=notes,
        )
        for name, (extra, notes) in ORBIT_RUNS.items()
    ]


def scenarios() -> list[Scenario]:
    return (
        takeoff_scenarios()
        + orbit_scenarios()
        + landing_scenarios()
        + maneuver_scenarios()
        + spin_scenarios()
        + stall_scenarios()
        + fault_scenarios()
        + combat_scenarios()
        + gunsight_scenarios()
        + slot_scenarios()
        + jettison_scenarios()
        + countermeasure_scenarios()
        + climbout_scenarios()
        + autopilot_scenarios()
        + eject_scenarios()
        + weather_scenarios()
        + cheat_scenarios()
        + panel_scenarios()
        + cheat_combat_scenarios()
        + device_scenarios()
        + edge_scenarios()
        + overspeed_scenarios()
        + lateral_scenarios()
        + liftoff_scenarios()
        + combat_g_scenarios()
        + belly_scenarios()
        + terrain_scenarios()
        + fight_scenarios()
        + attack_scenarios()
        + loadout_scenarios()
        + environment_scenarios()
        + capture_scenarios()
        + daytime_scenarios()
        + fuel_scenarios()
        + waypoint_scenarios()
        + climb_scenarios()
        + sprint_scenarios()
        + variety_scenarios()
        + heavy_gcurve_scenarios()
        + powered_scenarios()
    )
