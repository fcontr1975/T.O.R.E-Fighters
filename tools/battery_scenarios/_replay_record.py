"""Recording scenarios: probes recorded, read back every way, damaged files rejected.

Each recording scenario runs one AI probe with `--record-mission` and
`--verify-render`, then reads the recording back with `--recording-info`,
`--recording-log`, `--recording-acmi` and `--recording-diff`, and checks every
file it wrote. Scenarios that ask for a repeat also run the probe twice more
(once recorded, once not) to prove that identical runs record identically and
that recording changes nothing the probe prints.
"""
from __future__ import annotations

import json
import re
from pathlib import Path

from battery import Scenario, Step
from battery_scenarios import _replay_checks as rc

TOOLS = "tools/battery_scenarios/_replay_tools.py"
PY = "python3"
ROOT = Path(__file__).resolve().parents[2]

# The imported aircraft by command-line name.
AIRCRAFT = ["f18", "rafale", "f14", "a4e", "x31", "mig29", "su27", "mig21", "su25", "mig23", "su35", "f22", "f22n", "faxx"]


def sections(output: str) -> dict[int, str]:
    """Splits a scenario's output into the main run (0) and each follow-up step."""
    parts = re.split(r"\n\$ then (\d+): [^\n]*\n", output)
    found = {0: parts[0]}
    for i in range(1, len(parts) - 1, 2):
        found[int(parts[i])] = parts[i + 1]
    return found


_NOISE = re.compile(r"^(\d{10}\.\d+Z|Session log|Stage:|T\.O\.R\.E -|T\.O\.R\.E-Fighters )|verify-render|^Recording|^Tacview")


def normalise(text: str) -> list[str]:
    return [line for line in text.splitlines() if line.strip() and not _NOISE.search(line)]


def semantic_log_problems(text: str) -> list[str]:
    """Impossible states in the state samples and comms events of log.jsonl."""
    problems: list[str] = []
    world = None
    dead_at: dict[int, float] = {}
    delivered: dict[tuple, float] = {}
    for line in text.splitlines():
        d = json.loads(line)
        kind = d["type"]
        if kind == "header":
            world = d.get("world", {})
        elif kind == "sample":
            flags = d.get("flags", [])
            hp, max_hp = d["hp"], d["max_hp"]
            who = f"aircraft {d['id']} at {d['t']}s"
            if "alive" in flags and hp <= 0:
                problems.append(f"{who} is alive with {hp} hit points")
            if "alive" not in flags and hp > 0:
                problems.append(f"{who} is dead with {hp} hit points")
            if hp > max_hp:
                problems.append(f"{who} has more hit points than its maximum")
            if d["fuel_lb"] < 0:
                problems.append(f"{who} has negative fuel")
            x, y, z = d["pos_ft"]
            if world and world.get("extent_ft"):
                ex, ez = world["extent_ft"]
                if not (0 <= x <= ex and 0 <= z <= ez):
                    problems.append(f"{who} is outside the world ({x:.0f}, {z:.0f})")
            if abs(d["g"]) > 20:
                problems.append(f"{who} shows {d['g']} G")
        elif kind == "event":
            k = d["kind"]
            f = d.get("fields", {})
            if k in ("combat.destroyed", "aircraft.crashed", "aircraft.ejected") and d.get("subject") is not None:
                dead_at.setdefault(d["subject"], d["t"])
            if k in ("comms.radio", "comms.tower", "comms.hud") and f.get("outcome") == "delivered":
                text_ = d.get("text", "")
                if not text_.strip() or (k != "comms.hud" and not f.get("speaker")):
                    problems.append(f"empty delivered line at {d['t']}s: {f}")
                # The same line twice in 0.2 s is a stutter only when one
                # trigger made it. Two different triggers are two real events:
                # an "I'm hit" call is made for every guided hit (docs/spec/
                # radio-chatter.md), so two missiles striking one aircraft a
                # few ticks apart, or a splash and a direct hit, call twice and
                # the variant roll can pick the same line (1 in 5).
                key = (f.get("speaker"), text_, f.get("trigger"))
                last = delivered.get(key)
                if last is not None and d["t"] - last < 0.2 and k != "comms.hud":
                    problems.append(f"{f.get('speaker')} said '{text_}' twice within 0.2 s at {d['t']}s")
                delivered[key] = d["t"]
                s = d.get("subject")
                if s in dead_at and d["t"] > dead_at[s] + 0.5 and k == "comms.radio":
                    problems.append(f"{f.get('speaker')} spoke at {d['t']}s after aircraft {s} was lost at {dead_at[s]}s")
        if len(problems) > 20:
            problems.append("... more problems suppressed")
            break
    return problems


def outcome_problems(shot: int, results: list[dict]) -> list[str]:
    """A shot has one outcome. The one exception: a missile recorded as spoofed
    that flies on and damages an aircraft is recorded again as a hit that
    names the spoof it replaces (the debrief ledger withdraws the spoof and
    counts the hit, docs/spec/debrief.md)."""
    kinds = [(r.get("result"), r.get("replaces")) for r in results]
    if len(kinds) == 1:
        if kinds[0][1] is not None:
            return [f"shot {shot} outcome replaces {kinds[0][1]} but none came before it"]
        return []
    if kinds == [("spoofed", None), ("hit", "spoofed")]:
        return []
    return [f"shot {shot} has {len(kinds)} outcomes: " + ", ".join(
        k if r is None else f"{k} replacing {r}" for k, r in kinds)]


def decoy_problems(events: list[dict]) -> list[str]:
    """A decoyed missile names the chaff or flare that fooled it by the release
    number its `combat.countermeasure` entry carries. Recordings without numbers
    stay valid: only a number that is there is checked, and it must belong to a
    device of that kind released by then (by the aircraft the entry names, for
    `weapon.decoyed`). A tick lists its weapon events before the devices
    released on it, so a device counts from the tick it left."""
    problems: list[str] = []
    released: dict[tuple, int] = {}
    for e in events:
        f = e.get("fields", {})
        if e["kind"] == "combat.countermeasure" and f.get("number") is not None:
            released.setdefault((f["number"], e.get("subject"), f.get("decoy")), e["tick"])
    for e in events:
        f = e.get("fields", {})
        if e["kind"] == "weapon.decoyed" and f.get("number") is not None:
            left = released.get((f["number"], e.get("object"), f.get("decoy")))
            if left is None or left > e["tick"]:
                problems.append(
                    f"shot {f.get('projectile')} was decoyed by {f.get('decoy')} #{f['number']} "
                    f"of aircraft {e.get('object')}, which was never released")
        elif e["kind"] == "weapon.outcome" and f.get("replaces") is not None:
            m = re.search(r"decoyed by (chaff|flare) #(\d+) from ", f.get("reason", ""))
            if m and not any(
                n == int(m.group(2)) and d == m.group(1) and tick <= e["tick"]
                for (n, _, d), tick in released.items()
            ):
                problems.append(
                    f"shot {f.get('projectile')} late hit names {m.group(1)} #{m.group(2)}, which was never released")
    return problems


def invariant_problems(text: str) -> list[str]:
    """Event-stream invariants: order, ownership, one outcome per shot, decoy counts, no revivals."""
    problems: list[str] = []
    lines = [json.loads(line) for line in text.splitlines()]
    events = [e for e in lines if e["type"] == "event"]
    ids = {a["id"] for a in lines if a["type"] == "aircraft"}
    last = -1
    for e in events:
        if e["tick"] < last:
            problems.append(f"events run backwards at tick {e['tick']} (after {last})")
            break
        last = e["tick"]
    footer = [e for e in lines if e["type"] == "footer"]
    if footer and any(e["tick"] > footer[0]["end_tick"] for e in events):
        problems.append("an event is later than the recording's end")
    for e in events:
        s = e.get("subject")
        if s is not None and s < 1000 and s not in ids:
            problems.append(f"{e['kind']} names unknown aircraft {s}")
            break
    dead: dict[int, float] = {}
    for e in events:
        if e["kind"] in ("combat.destroyed", "aircraft.crashed") and e.get("subject") is not None:
            dead.setdefault(e["subject"], e["t"])
    launched = set()
    outcomes: dict[int, list[dict]] = {}
    left: dict[tuple, int] = {}
    for e in events:
        f = e.get("fields", {})
        if e["kind"] == "weapon.launch":
            launched.add(f["projectile"])
            if e["subject"] in dead and e["t"] > dead[e["subject"]] + 0.01:
                problems.append(f"aircraft {e['subject']} fired at {e['t']}s after it was lost at {dead[e['subject']]}s")
        elif e["kind"] == "weapon.outcome":
            outcomes.setdefault(f["projectile"], []).append(f)
        elif e["kind"] == "combat.countermeasure" and f.get("left") is not None:
            key = (e.get("subject"), f.get("decoy"))
            if f["left"] < 0 or f["left"] > left.get(key, 10**9):
                problems.append(f"aircraft {key[0]} {key[1]} count went to {f['left']} at {e['t']}s")
            left[key] = f["left"]
    problems += decoy_problems(events)
    for shot, results in outcomes.items():
        problems += outcome_problems(shot, results)
        if shot not in launched:
            problems.append(f"shot {shot} has an outcome but no launch")
    revived: set[int] = set()
    zero: set[int] = set()
    for e in lines:
        if e["type"] == "sample":
            if e["hp"] <= 0:
                zero.add(e["id"])
            elif e["id"] in zero and e["id"] not in revived:
                revived.add(e["id"])
                problems.append(f"aircraft {e['id']} came back to life at {e['t']}s")
    return problems[:20]


def recorded_probe_checks(work: Path, output: str, *, same_run: bool) -> list[str]:
    """Everything a recording of a probe run should satisfy, given the standard steps."""
    problems: list[str] = []
    s = sections(output)
    main = s[0]
    m = re.search(r"AI probe verify-render: (PASS|FAIL)[^\n]*", main)
    if not m:
        problems.append("no verify-render line")
    elif m.group(1) != "PASS" or "missing=0 differing=0" not in m.group(0):
        problems.append(f"verify-render did not pass cleanly: {m.group(0)}")
    info = s.get(1, "")
    if "State       finished normally" not in info:
        problems.append("recording-info does not say the recording finished normally")
    for bad in ("Problem ", "did not finish", "unknown:"):
        if bad in info:
            problems.append(f"recording-info reports: {bad}")
    n_info = len(re.findall(r"^Aircraft +\d+ ", info, re.M))
    actors = re.search(r"AI probe: aircraft=\S+ actors=(\d+)", main)
    if actors and n_info != int(actors.group(1)) + 1:
        problems.append(f"recording lists {n_info} aircraft but the probe had {actors.group(1)} actors plus the player")
    outcome = re.search(r"AI probe debrief: (SUCCESS|FAILURE)", main)
    result = re.search(r"^Result .*outcome=(\w+)", info, re.M)
    if outcome and result and outcome.group(1).lower() != result.group(1):
        problems.append(f"probe debrief says {outcome.group(1)} but the recording's result says {result.group(1)}")
    problems += rc.file_problems(work, "log/log.jsonl", rc.check_jsonl)
    problems += rc.file_problems(work, "log/summary.txt", lambda t: rc.check_summary(t, expect_aircraft=n_info or None))
    problems += rc.file_problems(work, "a.acmi", rc.check_acmi)
    log = work / "log" / "log.jsonl"
    if log.exists():
        try:
            problems += semantic_log_problems(log.read_text())
            problems += invariant_problems(log.read_text())
        except (ValueError, KeyError) as e:
            problems.append(f"log.jsonl could not be read for state checks: {e!r}")
    if log.exists():
        problems += rc.info_vs_log(info, log.read_text())
    acmi = work / "a.acmi"
    if acmi.exists() and log.exists():
        problems += rc.acmi_vs_log(acmi.read_text(), log.read_text())
    if acmi.exists() and n_info:
        ids = set(re.findall(r"^(1[0-9a-f]{10}),T=", acmi.read_text(), re.M))
        if len(ids) != n_info:
            problems.append(f"Tacview file has {len(ids)} aircraft objects, the recording has {n_info}")
    if "Verdict: the recordings match" not in s.get(4, ""):
        problems.append("recording-diff of a recording against itself does not say they match")
    if same_run:
        if "Verdict: the recordings match" not in s.get(6, ""):
            problems.append("two identical runs made recordings that differ")
        if normalise(s.get(5, "")) != normalise(main):
            problems.append("the same probe run twice printed different output")
        bare, recorded = normalise(s.get(7, "")), normalise(main)
        if bare != recorded:
            first = next((i for i, (x, y) in enumerate(zip(bare, recorded)) if x != y), min(len(bare), len(recorded)))
            problems.append(f"recording changed the probe's output (first difference at line {first})")
    return problems


def record_scenario(
    name: str,
    probe: list[str],
    *,
    timeout: float = 240.0,
    same_run: bool = True,
    expect: list[str] | None = None,
    extra_check=None,
    notes: str = "",
) -> Scenario:
    """One probe recorded and read back; with `same_run`, run twice more to prove determinism."""
    steps = [
        Step(["--recording-info", "{work}/a.tore-replay"]),
        Step(["--recording-log", "{work}/a.tore-replay", "--out", "{work}/log", "--rate", "5"]),
        Step(["--recording-acmi", "{work}/a.tore-replay", "--out", "{work}/a.acmi", "--guns"]),
        Step(["--recording-diff", "{work}/a.tore-replay", "{work}/a.tore-replay"]),
    ]
    if same_run:
        steps += [
            Step([*probe, "--record-mission", "{work}/b.tore-replay", "--verify-render", "--no-audio"], timeout=timeout),
            Step(["--recording-diff", "{work}/a.tore-replay", "{work}/b.tore-replay"]),
            Step([*probe, "--no-audio"], timeout=timeout),
        ]

    def check(work: Path, output: str) -> list[str]:
        problems = recorded_probe_checks(work, output, same_run=same_run)
        if extra_check:
            problems += extra_check(work, output)
        return problems

    return Scenario(
        name=name,
        lane="replay",
        args=[*probe, "--record-mission", "{work}/a.tore-replay", "--verify-render", "--no-audio"],
        timeout=timeout,
        expect=[r"AI probe totals:", *(expect or [])],
        outputs=["a.tore-replay"],
        then=steps,
        check_work=check,
        notes=notes,
    )


def fox_three_problems(work: Path, output: str) -> list[str]:
    """Match a real AIM-120 release to its delivered, recorded launch call."""
    log = work / "log" / "log.jsonl"
    if not log.exists():
        return ["missing Fox regression event log"]
    rows = [json.loads(line) for line in log.read_text().splitlines()]
    weapons = {row["id"] for row in rows if row["type"] == "weapon"
               and row["source"] == "AIM120.JT"}
    launches = [r for r in rows if r["type"] == "event" and r["kind"] == "weapon.launch"
                and r.get("fields", {}).get("weapon") in weapons]
    if not launches:
        return ["no AIM-120 launch recorded"]
    calls = [r for r in rows if r["type"] == "event" and r["kind"] == "comms.radio"
             and r.get("fields", {}).get("outcome") == "delivered"]
    for shot in launches:
        for call in calls:
            if (call.get("subject") == shot.get("subject")
                    and 0 <= call["t"] - shot["t"] <= 2
                    and "^FOXTHR" in call.get("fields", {}).get("stems", "").split()):
                return []
    return ["AIM-120 release has no matching delivered ^FOXTHR recording"]


def missile_release_problems(work: Path, output: str, *, close: bool) -> list[str]:
    problems = fox_three_problems(work, output)
    rows = [json.loads(line) for line in (work / "log" / "log.jsonl").read_text().splitlines()]
    launches = [r for r in rows if r.get("kind") == "weapon.launch" and r.get("subject") == 0]
    if not launches:
        return problems + ["manual player shot did not leave the rail"]
    if "readiness=Ready" not in output:
        problems.append("manual release was not ready")
    if close:
        if "initial_range_ft=1215.2" not in output:
            problems.append("close target was not placed at 0.2 nm")
        if not any(r.get("kind") == "weapon.pitbull" and r.get("subject") == 0
                   and r.get("fields", {}).get("range_ft", 12000) < 2000 for r in rows):
            problems.append("close shot did not acquire with its own seeker inside minimum range")
        if not any(r.get("kind") == "weapon.outcome" and r.get("subject") == 0
                   and r.get("fields", {}).get("result") in ("hit", "miss", "ground", "expired") for r in rows):
            problems.append("close shot has no recorded physical outcome")
    else:
        calls = [r for r in rows if r.get("kind") == "comms.radio"
                 and r.get("subject") == 0 and "^FOXTHR" in r.get("fields", {}).get("stems", "")]
        if not calls:
            problems.append("targetless launch has no recorded Fox three")
    return problems


def head_ir_problems(work: Path, output: str) -> list[str]:
    rows = [json.loads(line) for line in (work / "log" / "log.jsonl").read_text().splitlines()]
    weapons = {r["id"] for r in rows if r.get("type") == "weapon" and r.get("source") == "AIM9M.JT"}
    launches = [r for r in rows if r.get("kind") == "weapon.launch" and r.get("subject") == 0
                and r.get("fields", {}).get("weapon") in weapons]
    problems = []
    if not launches:
        problems.append("head-on AIM-9M did not leave the rail")
    elif not any(r.get("fields", {}).get("aspect_deg", 0) > 170
                 and 2.5 * 6076 < r.get("fields", {}).get("range_ft", 0) < 3.1 * 6076 for r in launches):
        problems.append("AIM-9M release was not head-on near 3 nm")
    if not any(r.get("kind") == "weapon.decoyed" and r.get("subject") == 0
               and r.get("fields", {}).get("decoy") == "flare" for r in rows):
        problems.append("seeded head-on AIM-9M was not seduced by the target's flare")
    if any(r.get("kind") == "combat.hit" and r.get("subject") == 0
           and r.get("fields", {}).get("weapon") in weapons for r in rows):
        problems.append("decoyed head-on AIM-9M unexpectedly hit after device passage")
    if not any(r.get("kind") == "audio.tone" and r.get("fields", {}).get("tone") == "infrared lock" for r in rows):
        problems.append("head-on IR lock tone was not recorded")
    return problems


def unlocked_release_problems(work: Path, output: str, *, mode: str) -> list[str]:
    rows = [json.loads(line) for line in (work / "log" / "log.jsonl").read_text().splitlines()]
    launches = [r for r in rows if r.get("kind") == "weapon.launch" and r.get("subject") == 0]
    if not launches:
        return ["manual player missile did not launch"]
    problems = []
    shots = {r.get("fields", {}).get("projectile") for r in launches}
    guidance = [{n["label"]: n.get("value") for n in r.get("nodes", [])}
                for r in rows if r.get("type") == "tree"
                and r.get("channel") == "weapon.guidance" and r.get("subject") in shots]
    if mode in ("maddog", "unlocked-ir"):
        if not guidance or guidance[0].get("Mode") != "boresight" or guidance[0].get("Target") is not None:
            problems.append("unlocked shot did not record independent targetless search")
    if mode == "maddog":
        problems += fox_three_problems(work, output)
        if not any(g.get("Seeker") == "PITBULL" for g in guidance):
            problems.append("recorded guidance tree never shows PITBULL")
        if not any(r.get("kind") == "weapon.pitbull" and r.get("subject") == 0 for r in rows):
            problems.append("maddog failed to acquire the off-bore target")
    elif mode == "unlocked-ir":
        if "mode=Cued" not in output or "locked: true" in output:
            problems.append("IR shot was not cued without seeker lock")
    elif mode == "max-range":
        if "readiness=MaximumRange" not in output:
            problems.append("shot did not retain MAX RANGE advisory")
        intended = {r.get("fields", {}).get("projectile"): r.get("object") for r in launches}
        if not intended or None in intended or any(target is None for target in intended.values()):
            problems.append("beyond-energy shot has no recorded designated target")
        if any(r.get("kind") == "combat.hit" and r.get("subject") == 0
               and r.get("fields", {}).get("projectile") in intended for r in rows):
            problems.append("beyond-energy shot produced an unintended hit")
    if mode != "maddog" and not any(r.get("kind") == "weapon.outcome"
            and r.get("subject") == 0 for r in rows):
        problems.append("shot has no recorded physical outcome")
    return problems


def other_weapon_sound_check(work: Path, output: str) -> list[str]:
    import csv
    path = work / "sound-log" / "weapon-sound.tsv"
    if not path.exists():
        return ["missing weapon sound audit"]
    rows = list(csv.DictReader(path.open(), delimiter="\t"))
    heard = [r for r in rows if r["selected"] == "true" and r["event"].startswith("audio.other.")]
    problems = []
    if not heard:
        problems.append("no other shooter audible")
    previous = {}
    for row in sorted(heard, key=lambda r: int(r.get("emitted_tick", r["tick"]))):
        if row["clip_present"] != "true" or not 0 < float(row["gain"]) <= 0.104001:
            problems.append("missing recording or invalid cockpit gain")
        if row["gun"] == "true":
            key = (row["shooter"], row["sound"])
            tick = int(row.get("emitted_tick", row["tick"]))
            if tick - previous.get(key, -30) < 30:
                problems.append("gun sound repeated per round")
            previous[key] = tick
        if float(row["distance_ft"]) > 100 and int(row["tick"]) <= int(row.get("emitted_tick", row["tick"])):
            problems.append("remote weapon sound did not travel")
    if "guns" in work.name and not previous:
        problems.append("no audible gun burst")
    return problems


def scenarios() -> list[Scenario]:
    out: list[Scenario] = []
    for mode in ["maddog", "unlocked-ir", "max-range"]:
        out.append(record_scenario(
            f"replay-rec-missile-{mode}",
            ["--aircraft", "f14" if mode == "max-range" else "f18", "--ai-probe-ticks", "18000", "--probe-fight", "1:1",
             "--probe-enemy-aircraft", "mig29", "--probe-geometry", "rear",
             "--probe-missile-release", mode, "--scope-range", "150", "--ai-mission", "hold"],
            timeout=300, same_run=True,
            extra_check=lambda work, output, mode=mode: unlocked_release_problems(work, output, mode=mode),
            notes="Player release advisories and independent seeker flight, recorded and replayed without rendering",
        ))
    out.append(record_scenario(
        "replay-rec-fox-three",
        ["--aircraft", "f18", "--ai-probe-ticks", "7200", "--separation", "5",
         "--probe-attack", "600:10"],
        same_run=False, timeout=400, extra_check=fox_three_problems,
        notes="AIM-120 launch is recorded as ^FOXTHR and survives replay export",
    ))

    for close in [False, True]:
        mode = "close" if close else "empty"
        out.append(record_scenario(
            f"replay-rec-missile-release-{mode}",
            ["--aircraft", "f18", "--ai-probe-ticks", "1200",
             "--probe-missile-release", mode, "--ai-mission", "hold",
             *(["--probe-fight", "1:1", "--probe-geometry", "rear"] if close
               else ["--probe-wing-only", "--probe-wing-size", "1"])],
            timeout=180, same_run=True,
            extra_check=lambda work, output, close=close: missile_release_problems(work, output, close=close),
            notes="Manual missile release, recorded Fox call and deterministic playback; minimum range is advisory",
        ))

    out.append(record_scenario(
        "replay-rec-missile-head-ir",
        ["--aircraft", "f18", "--ai-probe-ticks", "1800", "--probe-fight", "1:1",
         "--probe-enemy-aircraft", "mig29", "--probe-geometry", "head",
         "--probe-missile-release", "head-ir", "--ai-mission", "hold"],
        timeout=180, same_run=True,
        expect=[r"missile release tone: Some.*locked: true", r"readiness=Ready"],
        extra_check=head_ir_problems,
        notes="AIM-9M head-on at a MiG-29 from 3 nm: tone, release, guidance and replay",
    ))

    # AI fights of every size.
    for f, e in [(1, 1), (2, 2), (3, 3), (5, 5), (8, 8), (15, 15), (1, 15), (15, 1), (4, 9)]:
        out.append(
            record_scenario(
                f"replay-rec-fight-{f}v{e}",
                ["--ai-probe-ticks", "2400", "--probe-fight", f"{f}:{e}", "--separation", "5"],
                same_run=f + e <= 16,
                timeout=500,
            )
        )

    # Every imported aircraft as the enemy and as the player.
    for ac in AIRCRAFT:
        out.append(
            record_scenario(
                f"replay-rec-enemy-{ac}",
                ["--ai-probe-ticks", "2400", "--probe-enemy-aircraft", ac, "--separation", "2", "--probe-attack", "300:10"],
                same_run=False,
            )
        )
        out.append(
            record_scenario(
                f"replay-rec-player-{ac}",
                ["--ai-probe-ticks", "2400", "--aircraft", ac, "--separation", "2", "--probe-attack", "300:10", "--probe-flight-model", "researched"],
                same_run=False,
            )
        )

    # Skills, geometries and missions.
    for skill in ("novice", "average", "experienced", "ace"):
        for geometry in ("head", "side", "rear"):
            out.append(
                record_scenario(
                    f"replay-rec-skill-{skill}-{geometry}",
                    ["--ai-probe-ticks", "2400", "--probe-enemy-skill", skill, "--probe-geometry", geometry, "--separation", "1", "--probe-attack", "300:10"],
                    same_run=False,
                )
            )
    for mission in ("free", "cap", "intercept", "escort", "self-defense", "hold"):
        out.append(
            record_scenario(
                f"replay-rec-mission-{mission}",
                ["--ai-probe-ticks", "3000", "--ai-mission", mission, "--separation", "5", "--probe-fight", "3:3"],
                same_run=False,
            )
        )

    for mode, extra in [("guns", ["--probe-ai-guns-only", "--probe-guns"]), ("missiles", [])]:
        out.append(Scenario(
            name=f"replay-rec-other-weapon-sound-{mode}", lane="replay",
            args=["--ai-probe-ticks", "7200", "--probe-fight", "2:2", "--separation", "1" if mode == "guns" else "5",
                  "--probe-wing-order", "1:attack-on-contact", *extra,
                  "--record-mission", "{work}/sound.tore-replay", "--no-audio"],
            timeout=300, env={"TORE_WEAPON_SOUND_LOG": "1"},
            then=[Step(["--recording-log", "{work}/sound.tore-replay", "--out", "{work}/sound-log", "--rate", "1"], timeout=300)],
            check_work=other_weapon_sound_check,
            expect=[r"Weapon sound audit:"],
        ))

    # Guns, missiles, chaff and flares, threats, faults.
    out.append(record_scenario("replay-rec-guns", ["--ai-probe-ticks", "3600", "--separation", "2", "--probe-guns", "--probe-ai-guns-only", "--probe-attack", "300:5"]))
    out.append(record_scenario("replay-rec-missiles", ["--ai-probe-ticks", "7200", "--separation", "5", "--probe-attack", "600:10"], timeout=400))
    for kind in ("hit", "gun", "aaa"):
        out.append(
            record_scenario(
                f"replay-rec-threat-{kind}",
                ["--ai-probe-ticks", "1800", "--probe-enemy-aircraft", "su27", "--probe-geometry", "rear", "--separation", "1", "--ai-mission", "hold", "--probe-threat", f"120:{kind}", "--probe-flight-model", "researched"],
                same_run=False,
            )
        )
    for fault in (0, 4, 7, 11, 12, 29, 30, 34, 44):
        out.append(record_scenario(f"replay-rec-fault-{fault}", ["--ai-probe-ticks", "1800", "--separation", "1", "--probe-fault", f"100:{fault}"], same_run=False))

    # Ground starts, takeoffs, wings, orders, at Blue fields (slice AL1):
    # Simferopol, Odesa and Kherson.
    for airport in (2, 9, 11):
        for wing in (1, 3, 5):
            out.append(
                record_scenario(
                    f"replay-rec-ground-{airport}-wing{wing}",
                    ["--ai-probe-ticks", "6000", "--ground-start", str(airport), "--maneuver", "takeoff", "--probe-wing-size", str(wing), "--probe-wing-only"],
                    same_run=False,
                    timeout=300,
                )
            )
    for order in ("bug-out", "attack-on-contact", "engage-my-target", "land-selected"):
        out.append(
            record_scenario(
                f"replay-rec-order-{order}",
                ["--ai-probe-ticks", "9000", "--ground-start", "2", "--maneuver", "takeoff", "--probe-wing-size", "3", "--probe-wing-order", f"3000:{order}"],
                same_run=False,
                timeout=400,
            )
        )

    # The flight data link's events (slice G9): the probe's own `data link:` lines
    # and the recording's `datalink.*` events say the same things.
    out.append(
        record_scenario(
            "replay-rec-datalink",
            [
                "--ai-probe-ticks", "2400", "--probe-wing-size", "2", "--separation", "5", "--probe-player-lock", "600:4",
                "--probe-wing-order", "700:engage-my-target@1", "--probe-wing-order", "900:attack-on-contact",
            ],
            expect=[r"data link: assign plane=1 target=4 by=0 order=EngageMyTarget"],
            extra_check=datalink_event_problems,
            notes="datalink.* events: every plane a member, the player's lock, the assignment and its clearing by order",
        )
    )

    out += failure_scenarios()
    return out


def datalink_event_problems(work: Path, output: str) -> list[str]:
    """The recorded `datalink.*` events of the data link probe: every plane joined
    once, the player's lock, the lead's assignment to the first wingman and its
    clearing by the later Attack on contact, and nothing out of order."""
    log = work / "log" / "log.jsonl"
    if not log.exists():
        return []
    events = [e for e in (json.loads(line) for line in log.read_text().splitlines()) if e["type"] == "event"]
    link = [e for e in events if e["kind"].startswith("datalink.")]
    problems: list[str] = []
    members = [e["subject"] for e in link if e["kind"] == "datalink.member"]
    if sorted(members) != sorted(set(members)) or sorted(members) != list(range(6)):
        problems.append(f"datalink.member events for planes {sorted(members)}, expected each of 0..5 once")
    if any(e["fields"].get("radar") is not True for e in link if e["kind"] == "datalink.member"):
        problems.append("a datalink.member event says no radar")
    if not any(e["kind"] == "datalink.lock" and e["subject"] == 0 and e["object"] == 4 for e in link):
        problems.append("no datalink.lock for the player on aircraft 4")
    assigns = [e for e in link if e["kind"] == "datalink.assign"]
    if not any(
        (e["subject"], e["object"], e["fields"].get("target"), e["fields"].get("order")) == (0, 1, 4, "EngageMyTarget")
        for e in assigns
    ):
        problems.append(f"no datalink.assign from plane 0 to plane 1 for target 4: {assigns}")
    clears = [e for e in link if e["kind"] == "datalink.clear" and e["subject"] == 1 and e["object"] == 4]
    if not any(e["fields"].get("reason") == "order" for e in clears):
        problems.append(f"plane 1's assignment was not cleared by an order: {clears}")
    # The probe's own lines say the same: one assign line for each assign event.
    lines = len(re.findall(r"^t=\d+ data link: assign ", sections(output)[0], re.M))
    if lines != len(assigns):
        problems.append(f"the probe printed {lines} assignments, the recording has {len(assigns)}")
    return problems


def check_rejected(output: str, message: str) -> list[str]:
    problems = []
    s = sections(output)
    for i in (2, 3, 4):
        text = s.get(i, "")
        if message not in text:
            problems.append(f"step {i}: expected the message '{message}', got: {text.strip()[-160:]}")
        if "panicked" in text:
            problems.append(f"step {i}: panic")
    return problems


def check_soft_damage(work: Path, output: str) -> list[str]:
    """Damaged but readable files: a warning or a clear message, never a panic or a bare error."""
    problems = []
    s = sections(output)
    if "Problem " not in s.get(2, ""):
        problems.append("recording-info does not report a problem with a damaged file")
    for i in (2, 3, 4, 5):
        text = s.get(i, "")
        if "panicked" in text:
            problems.append(f"step {i}: panic")
        if not text.strip():
            problems.append(f"step {i}: no output at all")
    return problems


def killed_probe_scenario() -> Scenario:
    """A recording whose writer is killed mid-flight must still read back as incomplete."""
    binary = "{bin}"  # the tore-app under test, filled in by battery.run_steps
    return Scenario(
        name="replay-rec-killed-probe",
        lane="replay",
        args=["--version"],
        then=[
            Step(
                [PY, TOOLS, "killrun", "6", binary, "--ai-probe-ticks", "90000", "--separation", "5", "--record-mission", "{work}/k.tore-replay", "--no-audio"],
                app=False,
                timeout=60,
            ),
            Step(["--recording-info", "{work}/k.tore-replay.partial"]),
            Step(["--recording-log", "{work}/k.tore-replay.partial", "--out", "{work}/log", "--rate", "2"]),
            Step(["--recording-acmi", "{work}/k.tore-replay.partial", "--out", "{work}/k.acmi"]),
        ],
        check_work=check_killed,
    )


def has_length(info: str) -> bool:
    """recording-info prints `Length      m:ss.s`. How much was written before the
    kill depends on how fast the machine runs the probe (0:xx on a slow one,
    minutes on a fast one), so only a printed length counts."""
    return re.search(r"^Length +\d+:\d\d", info, re.M) is not None


def check_killed(work: Path, output: str) -> list[str]:
    s = sections(output)
    problems = []
    if not (work / "k.tore-replay.partial").exists():
        problems.append("no .partial recording was left")
    if (work / "k.tore-replay").exists():
        problems.append("a killed run left a finished-looking recording")
    if "INCOMPLETE" not in s.get(2, ""):
        problems.append("recording-info does not call it incomplete")
    if not has_length(s.get(2, "")):
        problems.append("the partial recording has no length")
    problems += rc.file_problems(work, "log/log.jsonl", rc.check_jsonl)
    problems += rc.file_problems(work, "k.acmi", rc.check_acmi)
    return problems


def failure_scenarios() -> list[Scenario]:
    out: list[Scenario] = [killed_probe_scenario()]
    make = ["--ai-probe-ticks", "1200", "--separation", "2", "--record-mission", "{work}/src.tore-replay", "--no-audio"]

    out.append(
        Scenario(
            name="replay-rec-refuses-existing-path",
            lane="replay",
            args=[*make[:-3], "--record-mission", "{work}/a.tore-replay", "--no-audio"],
            outputs=["a.tore-replay"],
            then=[Step([*make[:-3], "--record-mission", "{work}/a.tore-replay", "--no-audio"], expect_exit=1)],
            check_work=lambda work, output: (
                [] if "already exists" in sections(output).get(1, "") else ["no clear 'already exists' message"]
            )
            + ([] if (work / "a.tore-replay").stat().st_size > 1000 else ["existing recording was damaged"]),
        )
    )

    hard = {
        "empty": "too short",
        "head100": "header",
        "text": "damaged recording",
        "random": "not a T.O.R.E recording",
        "bad-magic": "not a T.O.R.E recording",
        "bad-version": "unsupported recording",
    }
    for kind, message in hard.items():
        steps = [
            Step([PY, TOOLS, "mangle", kind, "{work}/src.tore-replay", "{work}/bad.tore-replay"], app=False),
            Step(["--recording-info", "{work}/bad.tore-replay"], expect_exit=1),
            Step(["--recording-log", "{work}/bad.tore-replay", "--out", "{work}/o-log"], expect_exit=1),
            Step(["--recording-acmi", "{work}/bad.tore-replay", "--out", "{work}/o.acmi"], expect_exit=1),
        ]
        out.append(
            Scenario(
                name=f"replay-corrupt-{kind}",
                lane="replay",
                args=make,
                then=steps,
                check_work=lambda work, output, message=message: check_rejected(output, message),
            )
        )
    for kind in ("trunc-third", "trunc-half", "no-tail", "no-last-byte", "flip", "zero-fill", "duplicate-tail"):
        steps = [
            Step([PY, TOOLS, "mangle", kind, "{work}/src.tore-replay", "{work}/bad.tore-replay"], app=False),
            Step(["--recording-info", "{work}/bad.tore-replay"], expect_exit=None),
            Step(["--recording-log", "{work}/bad.tore-replay", "--out", "{work}/o-log"], expect_exit=None),
            Step(["--recording-acmi", "{work}/bad.tore-replay", "--out", "{work}/o.acmi"], expect_exit=None),
            Step(["--recording-diff", "{work}/src.tore-replay", "{work}/bad.tore-replay"], expect_exit=None),
        ]
        out.append(
            Scenario(
                name=f"replay-corrupt-{kind}",
                lane="replay",
                args=make,
                then=steps,
                check_work=check_soft_damage,
            )
        )
    return out
