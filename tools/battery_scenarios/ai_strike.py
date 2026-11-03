"""Lane: flight. The AI strike driver (AI strike round, slice ST-D;
`.local/ai-a2g/plan.md`, docs/testing/lane-flight.md "AI strike driver").

`--a2g-run THEATER STEM --pilot ai` lets the AI fly the striker and prints
`ai-strike:` lines (PROFILE, RELEASE, IMPACT miss_ft, KILL, EGRESS, LOST) from
what the simulation reports. Two kinds of scenario use it:

* Driver scenarios, which pass today and keep passing as the strike behaviour
  lands: they check that the driver runs, that its report is consistent with
  itself (the summary counts the lines, a release comes before its impact),
  that a human-flown run is handed to the AI, and that a wing order goes out.
  `ai-strike-driver`, `ai-strike-handback`, `ai-strike-order`; the order scenarios `ai-wing-ground-order`
  and `ai-wing-lase` (slice ST-O).
* Takeover (slice ST-K): `ai-strike-takeover`, a human's laser bomb pass in LVL continued by the AI
  after a handback and taken back afterwards.
* Behaviour scenarios, one per weapon class (slice ST-T, the strike tactics):
  `ai-strike-bombs`, `ai-strike-rockets`, `ai-strike-maverick`,
  `ai-strike-jdam`, `ai-sead-harm`, `ai-strike-defended`. Each run names the
  pass it flew, releases, and ends the round within its skill's limits; the
  unguided ones also print the pass's drawn aim offset on the RELEASE line,
  which must lie inside the release circle when the draw says so and in the
  ring R to 2R otherwise.

* Lasing scenarios (slice ST-L, docs/spec/ai-strike.md "Lasing"): `ai-strike-laser-self` (an F-16C's
  Pave Spike lases its own GBU-10 through the whole fall in an offset turn, then exits; an Su-25's
  built-in cone holds its nose on the target instead) and `ai-strike-laser-buddy` (an AI lases for
  a player's GBU-10; an AI F-14 with only a GBU-10 and no designator drops on a player's spot; the
  same F-14 with nobody lasing does not choose the bomb). The runs print `PHASE`, `HOLD` and
  `LASER` lines for them.
* The committed-to-release window (John, 2026-10-28): `ai-strike-commit-aaa`, gunfire inside the
  last 5 s of a straight Mk 82 pass waits for the release (`COMMIT ... held_defence=yes`), and the
  same pass with a gun moved out under the run line breaks into Defend before the window opens.

`parse_runs` reads the output; the check functions are tested in
tools/test_battery_ai_strike.py.
"""
from __future__ import annotations

import dataclasses
import math
import re

from battery import Scenario, Step
from battery_scenarios.a2g_guided import IMPACT as A2G_IMPACT, KILL as A2G_KILL, RELEASE as A2G_RELEASE, SIGHT as A2G_SIGHT, TARGET as A2G_TARGET
from battery_scenarios.a2g_missiles import STRAIGHT_FLUSH

LINE = re.compile(r"^ai-strike: (\S+)(?: (.*))?$", re.M)
FIELD = re.compile(r'(\w+)=("[^"]*"|\S+)')

# John's release circle (2026-10-11, docs/spec/ai-strike.md#skill-and-release-precision), by skill
# 0 (Novice), 1 (Average), 2 (Experienced), 3 (Ace): 85, 60, 35 and 10 m, in feet (279, 197, 115
# and 33).
CIRCLE_M = {0: 85.0, 1: 60.0, 2: 35.0, 3: 10.0}
CIRCLE_FT = {skill: metres / 0.3048 for skill, metres in CIRCLE_M.items()}
# The release gate (`strike.rs` GATE_SHARE and GATE_FLOOR_FT): the cue may be this far from the aim
# point at the release, so the bomb's mean impact may sit this far outside the drawn aim.
GATE_FT = {skill: max(0.1 * r, 10.0) for skill, r in CIRCLE_FT.items()}
# The bomb's own scatter, on top of the aim offset: half what it was at 3 degrees (800 ft), as John
# halved the scatter to 1.5 degrees. The worst of the specced cases is the level Mk 82 (about 385
# ft across the track); the dive is about 155 ft.
SCATTER_FT = 400.0
# Horizontal miss, in feet, a bomb pass may end within. The seeded aim offset is inside the circle
# 90% of passes and inside the ring R to 2R on the other 10%: a run whose RELEASE lines all say
# `inside=yes` is held to the circle plus the gate plus the scatter, any other to the ring's outer
# edge plus them.
MISS_FT_MAX = {skill: 2 * r + GATE_FT[skill] + SCATTER_FT for skill, r in CIRCLE_FT.items()}
MISS_FT_INSIDE = {skill: r + GATE_FT[skill] + SCATTER_FT for skill, r in CIRCLE_FT.items()}
# The release floor under threat Clear (`strike.rs` FLOOR_CLEAR_FT).
FLOOR_CLEAR_FT = 3000.0
# The driver rounds the aim offset to whole feet.
AIM_SLACK_FT = 1.0
# A rocket or a guided weapon is judged by the target, not a miss distance.
SKILLS = (0, 1, 2, 3)
EVENT_KINDS = ("RELEASE", "IMPACT", "KILL", "EGRESS", "LOST")


@dataclasses.dataclass
class Run:
    """The `ai-strike:` lines of one driver run, in order, as (kind, fields)."""

    lines: list[tuple[str, dict[str, str]]]

    def of(self, kind: str) -> list[dict[str, str]]:
        return [f for k, f in self.lines if k == kind]

    def first(self, kind: str) -> dict[str, str] | None:
        found = self.of(kind)
        return found[0] if found else None

    @property
    def setup(self) -> dict[str, str]:
        return self.first("setup") or {}

    @property
    def summary(self) -> dict[str, str]:
        return self.first("summary") or {}

    @property
    def skill(self) -> int:
        return int(self.setup.get("skill", "-1"))

    def times(self, kind: str) -> list[float]:
        return [float(f["t"]) for f in self.of(kind) if "t" in f]


def parse_line(kind: str, rest: str | None) -> dict[str, str]:
    """`key=value` fields; a quoted value keeps its spaces and loses its quotes."""
    fields = {}
    for key, value in FIELD.findall(rest or ""):
        fields[key] = value[1:-1] if value.startswith('"') else value
    return fields


def parse_runs(output: str) -> list[Run]:
    """The runs in `output`: each ends at its `ai-strike: done`."""
    runs: list[Run] = []
    current: list[tuple[str, dict[str, str]]] = []
    for m in LINE.finditer(output):
        kind, rest = m.group(1), m.group(2)
        if kind == "done":
            runs.append(Run(current))
            current = []
        elif kind == "no":
            current.append(("no-release", {}))
        else:
            current.append((kind, parse_line(kind, rest)))
    if current:
        runs.append(Run(current))
    return runs


def number(value: str | None) -> float:
    try:
        return float(value) if value is not None else math.nan
    except ValueError:
        return math.nan


def consistency_problems(run: Run, label: str) -> list[str]:
    """What must hold of any run, whatever the AI does."""
    problems: list[str] = []
    kinds = [k for k, _ in run.lines]
    for once in ("setup", "start", "summary"):
        if kinds.count(once) != 1:
            problems.append(f"{label}: {kinds.count(once)} {once} lines, want 1")
    if "setup" in kinds and "PROFILE" not in kinds:
        problems.append(f"{label}: no PROFILE line")
    s = run.summary
    if not s:
        return problems
    counts = {
        "releases": len(run.of("RELEASE")),
        "impacts": len(run.of("IMPACT")),
        "kills": len(run.of("KILL")),
    }
    for key, count in counts.items():
        if s.get(key) != str(count):
            problems.append(f"{label}: summary says {key}={s.get(key)}, the lines show {count}")
    for key, kind in (("egress", "EGRESS"), ("lost", "LOST")):
        want = "yes" if run.of(kind) else "no"
        if s.get(key) != want:
            problems.append(f"{label}: summary says {key}={s.get(key)}, the lines show {want}")
    if counts["releases"] == 0 and "no-release" not in kinds:
        problems.append(f"{label}: no release, but the run does not say so")
    if counts["releases"] and "no-release" in kinds:
        problems.append(f"{label}: released, yet says no release")
    if counts["impacts"] > counts["releases"]:
        problems.append(f"{label}: {counts['impacts']} impacts for {counts['releases']} releases")
    releases, impacts = run.times("RELEASE"), run.times("IMPACT")
    if impacts and (not releases or min(impacts) < min(releases)):
        problems.append(f"{label}: an impact before the first release")
    egress = run.times("EGRESS")
    if egress and (not releases or min(egress) < min(releases)):
        problems.append(f"{label}: egress before the first release")
    if len(run.of("LOST")) > 1:
        problems.append(f"{label}: lost more than once")
    stamps = [float(f["t"]) for k, f in run.lines if k in EVENT_KINDS and "t" in f]
    if stamps != sorted(stamps):
        problems.append(f"{label}: events out of order in time: {stamps}")
    return problems


def driver_problems(output: str, want: int, forced: list[str | None]) -> list[str]:
    """`ai-strike-driver`: `want` runs, each consistent, each echoing the profile it was asked
    to fly and a PROFILE line that is either the forced one or an honest `none`."""
    runs = parse_runs(output)
    if len(runs) != want:
        return [f"{len(runs)} runs finished, want {want}"]
    problems: list[str] = []
    for i, (run, profile) in enumerate(zip(runs, forced)):
        label = f"run {i + 1}"
        problems += consistency_problems(run, label)
        if run.setup.get("profile") != (profile or "none"):
            problems.append(f"{label}: setup says profile={run.setup.get('profile')}, asked {profile}")
        named = run.first("PROFILE") or {}
        if named.get("requested") != (profile or "none"):
            problems.append(f"{label}: PROFILE requested={named.get('requested')}, asked {profile}")
        if named.get("name") not in ("none", profile):
            problems.append(f"{label}: PROFILE name={named.get('name')}, want {profile} or none")
        if run.setup.get("hp", "0") == "0":
            problems.append(f"{label}: the target has no hit points")
    return problems


def handback_problems(output: str) -> list[str]:
    """`ai-strike-handback`: run 1 holds the trigger in LVL, so a bomb leaves the human before the
    handoff; run 2 hands back with the bomb selected and nothing released."""
    runs = parse_runs(output)
    if len(runs) != 2:
        return [f"{len(runs)} runs finished, want 2"]
    problems: list[str] = []
    for i, run in enumerate(runs):
        label = f"run {i + 1}"
        problems += consistency_problems(run, label)
        back = run.first("HANDBACK")
        if back is None:
            problems.append(f"{label}: no HANDBACK line")
            continue
        if back.get("mode") != "LVL":
            problems.append(f"{label}: handed back in {back.get('mode')}, asked LVL")
        if back.get("selected") in (None, "0"):
            problems.append(f"{label}: handed back with station {back.get('selected')} selected, want the bomb's")
        if run.of("DESIGNATE") == [] or run.first("DESIGNATE").get("target") == "none":
            problems.append(f"{label}: the human designated nothing")
        after = [t for t in run.times("ACTIVITY") if t >= float(back["t"])]
        if not after:
            problems.append(f"{label}: no AI activity at or after the handback")
    first = runs[0]
    released = first.times("RELEASE")
    back = first.first("HANDBACK")
    if not released:
        problems.append("run 1: the human released nothing before the handback")
    elif back and released[0] >= float(back["t"]):
        problems.append("run 1: the first release came after the handback")
    for impact in first.of("IMPACT")[:1]:
        miss = number(impact.get("miss_ft"))
        if not miss <= 1500:
            problems.append(f"run 1: the human's bomb ended {impact.get('miss_ft')} ft from the target")
    if runs[1].of("RELEASE") and runs[1].times("RELEASE")[0] < float(runs[1].first("HANDBACK")["t"]):
        problems.append("run 2: the human released, but the trigger was never pressed")
    return problems


# A laser bomb the AI guides on the human's laser ends this close to the target, feet.
TAKEOVER_MISS_FT = 500.0


def takeover_problems(output: str) -> list[str]:
    """`ai-strike-takeover` (slice ST-K, docs/spec/ai-strike.md "Takeover and handback"): a human sets
    up a laser bomb pass in LVL with the laser on by hand and gives the aircraft back mid-pass; the AI
    continues the same pass on the same target with the same store, in LVL with the laser on, and the
    bomb guides home; the human takes the aircraft back and finds its cockpit as left, one bomb fewer."""
    runs = parse_runs(output)
    if len(runs) != 1:
        return [f"{len(runs)} runs finished, want 1"]
    run = runs[0]
    problems = consistency_problems(run, "run 1")
    back = run.first("HANDBACK")
    over = run.first("TAKEOVER")
    if back is None or over is None:
        return problems + ["no HANDBACK or TAKEOVER line"]
    t_back = number(back.get("t"))
    if back.get("mode") != "LVL" or back.get("laser") != "hand-on":
        problems.append(f"handed back in {back.get('mode')} with the laser {back.get('laser')}, asked LVL hand-on")
    if back.get("designated") in (None, "none"):
        problems.append("the human designated nothing")
    if (over.get("target"), over.get("station"), over.get("mode")) != (
        back.get("designated"), back.get("selected"), "LVL"
    ):
        problems.append(
            f"the AI took over target={over.get('target')} station={over.get('station')} mode={over.get('mode')}, "
            f"the human left {back.get('designated')} station {back.get('selected')} LVL"
        )
    passes = [f.get("name") for f in run.of("PROFILE") if number(f.get("t")) >= t_back]
    if passes[:1] != ["level"]:
        problems.append(f"the AI flew {passes[:1] or 'nothing'}, want a level run")
    releases = [t for t in run.times("RELEASE") if t >= t_back]
    if not releases:
        problems.append("the AI released nothing after the handback")
    impacts = run.of("IMPACT")
    first_impact = number(impacts[0].get("t")) if impacts else math.inf
    if not impacts:
        problems.append("the bomb never ended")
    elif not number(impacts[0].get("miss_ft")) <= TAKEOVER_MISS_FT:
        problems.append(f"the laser bomb ended {impacts[0].get('miss_ft')} ft from the target")
    kept = run.of("KEPT")
    if not kept:
        problems.append("no KEPT line: the AI flew without the human's cockpit")
    for line in kept:
        if line.get("mode") != "LVL" or line.get("laser") != "hand-on" or line.get("selected") != back.get("selected"):
            problems.append(f"the AI changed the human's cockpit: {line}")
            break
        if line.get("lasing") != "yes" and number(line.get("t")) < first_impact:
            problems.append(f"the laser stopped at t={line.get('t')}, before the bomb ended")
            break
    take = run.first("TAKEBACK")
    if take is None:
        return problems + ["no TAKEBACK line"]
    if (take.get("mode"), take.get("laser"), take.get("selected")) != ("LVL", "hand-on", back.get("selected")):
        problems.append(f"the human found mode={take.get('mode')} laser={take.get('laser')} station={take.get('selected')}")
    want = number(run.setup.get("rounds")) - len(run.of("RELEASE"))
    if number(take.get("rounds")) != want:
        problems.append(f"the human found {take.get('rounds')} rounds, want {want:.0f}")
    return problems


def order_problems(output: str) -> list[str]:
    """`ai-strike-order`: the seat designates, gives the order, and the world answers it."""
    runs = parse_runs(output)
    if len(runs) != 1:
        return [f"{len(runs)} runs finished, want 1"]
    run = runs[0]
    problems = consistency_problems(run, "run 1")
    order = run.first("ORDER")
    if order is None:
        return problems + ["no ORDER line"]
    if order.get("designated") in (None, "none"):
        problems.append("the order went out with nothing designated")
    reply = run.first("ORDER-REPLY")
    if reply is None:
        problems.append("the world gave no reply to the order")
    elif reply.get("result") not in ("given", "refused", "failed"):
        problems.append(f"order reply {reply.get('result')}")
    elif not reply.get("message"):
        problems.append("the order's reply has no words")
    if run.setup.get("order") in (None, "none"):
        problems.append("setup does not name the order")
    return problems


def ground_order_problems(output: str, runs_wanted: list[dict]) -> list[str]:
    """The wing orders onto the ground (slice ST-O). Each wanted run has a `label` and a `kind`:
    `strike` (Alt+E, Alt+R, Alt+F: the wingman holds the object as its order and releases),
    `class` (Alt+W: it holds a class order and releases), `lase` (it holds the lase order),
    `refused` (no usable surface store: the order is refused, nothing is held, nothing released)
    or `free` (attack on contact: taken, nothing held)."""
    runs = parse_runs(output)
    if len(runs) != len(runs_wanted):
        return [f"{len(runs)} runs finished, want {len(runs_wanted)}"]
    problems: list[str] = []
    for run, want in zip(runs, runs_wanted):
        label, kind = want["label"], want["kind"]
        problems += consistency_problems(run, label)
        order, reply, state = run.first("ORDER"), run.first("ORDER-REPLY"), run.first("ORDER-STATE")
        if order is None or reply is None or state is None:
            problems.append(f"{label}: missing ORDER, ORDER-REPLY or ORDER-STATE line")
            continue
        if order.get("designated") in (None, "none"):
            problems.append(f"{label}: the order went out with nothing designated")
        message = reply.get("message", "")
        held = {k: state.get(k, "none") for k in ("strike_order", "lase_order")}
        classes = int(state.get("class_order", "0"))
        if kind == "refused":
            if "no usable surface store" not in message:
                problems.append(f"{label}: reply {message!r}, want the no usable surface store refusal")
            if held["strike_order"] != "none" or held["lase_order"] != "none" or classes:
                problems.append(f"{label}: a refused order left an order standing: {state}")
            if run.of("RELEASE"):
                problems.append(f"{label}: the refusing wingman released")
            continue
        if reply.get("result") != "given" or "1 applied" not in message:
            problems.append(f"{label}: reply {reply.get('result')} {message!r}, want 1 applied")
        if kind == "strike":
            if held["strike_order"] == "none" or classes or held["lase_order"] != "none":
                problems.append(f"{label}: want one strike order held, got {state}")
        elif kind == "class":
            if classes < 1 or held["strike_order"] != "none":
                problems.append(f"{label}: want a class order held, got {state}")
        elif kind == "lase":
            # The lase duty (slice ST-L, wired at ST-INT) orders the wingman onto the target it
            # lases, so a strike order on that same object may stand beside the lase order.
            if held["lase_order"] == "none" or held["strike_order"] not in ("none", held["lase_order"]) or classes:
                problems.append(f"{label}: want the lase order held, got {state}")
            if want.get("flies", False):
                problems += lase_flown_problems(run, label, float(order.get("t", "nan")))
        elif kind == "free":
            if held["strike_order"] != "none" or held["lase_order"] != "none" or classes:
                problems.append(f"{label}: attack on contact holds no ground order, got {state}")
        if kind in ("strike", "class") and want.get("release", True) and not run.of("RELEASE"):
            problems.append(f"{label}: the wingman never released")
    return problems


def lase_flown_problems(run: Run, label: str, ordered_at: float) -> list[str]:
    """Lase my target flown (ST-INT): the wingman enters the lase hold within 2 s of the order,
    its laser comes up Lasing and stays so, it flies at least two HOLD lines inside the laser's
    reach and outside 2 nm (the orbit is 3 nm), and it drops nothing."""
    problems: list[str] = []
    plane = (run.first("start") or {}).get("plane")
    lase_at = [float(p["t"]) for p in run.of("PHASE") if p.get("name") == "Lase" and "t" in p]
    if not lase_at:
        problems.append(f"{label}: the wingman never entered the lase hold")
    elif not lase_at[0] - ordered_at <= 2.0:
        problems.append(f"{label}: the lase hold began {lase_at[0] - ordered_at:.1f} s after the order, want 2 s")
    lasers = [x for x in run.of("LASER") if x.get("plane") == plane and number(x.get("t")) >= ordered_at]
    if not any(x.get("state") == "Lasing" for x in lasers):
        problems.append(f"{label}: the wingman's laser never came up after the order")
    elif lasers[-1].get("state") != "Lasing":
        problems.append(f"{label}: the wingman's spot was lost ({lasers[-1].get('state')} {lasers[-1].get('mask')})")
    holds = run.of("HOLD")
    if len(holds) < 2:
        problems.append(f"{label}: {len(holds)} HOLD lines, want at least two")
    for h in holds[1:]:
        r = number(h.get("range_ft"))
        if not 2 * 6076.0 <= r < LASER_REACH_FT:
            problems.append(f"{label}: the hold at t={h.get('t')} was {r:.0f} ft out, want 2 nm to the laser's reach")
    if run.of("RELEASE"):
        problems.append(f"{label}: the lasing wingman released")
    return problems


def clear_air_strike_problems(output: str, kill: bool = True) -> list[str]:
    """`kill=False` (the Mk 82 run) asks for a bomb on its target instead of a building kill: one
    Mk 82 rarely destroys a 500 hp HQ building, and the Maverick wingman often kills the striker's
    bunker first, so a kill there depended on which target a long pass happened to pick."""
    runs = parse_runs(output)
    if len(runs) != 1:
        return [f"expected one strike run, got {len(runs)}"]
    run = runs[0]
    problems = consistency_problems(run, "HQ strike")
    if not any(number(r.get("t")) > 10 for r in run.of("RELEASE")):
        problems.append("no release after the air threat was removed")
    if kill and not any(k.get("victim", "").startswith("0x500000") for k in run.of("KILL")):
        problems.append("the strike did not destroy a building at the HQ site")
    limit = MISS_FT_MAX.get(run.skill)
    if not kill and limit is not None and not any(
        number(i.get("miss_ft")) <= limit for i in run.of("IMPACT")
    ):
        problems.append(f"no bomb landed within {limit:.0f} ft of its target")
    if not run.of("EGRESS"):
        problems.append("the strike did not egress")
    return problems


def quick_wing_problems(output: str) -> list[str]:
    """`qmc-strike-wing`: a Quick Mission's friendly wing 2 with a ground target set. Run 1 is
    bombers (an A-4E, bomber class): the mission gives them the Strike role and they release.
    Run 2 is fighters (an F-16C): they keep CAP and release nothing."""
    runs = parse_runs(output)
    if len(runs) != 2:
        return [f"{len(runs)} runs finished, want 2"]
    problems: list[str] = []
    for run, role, label in zip(runs, ("strike", "cap"), ("bombers", "fighters")):
        problems += consistency_problems(run, label)
        found = run.first("ROLE")
        if found is None:
            problems.append(f"{label}: no ROLE line")
        elif found.get("role") != role:
            problems.append(f"{label}: the wing was given role {found.get('role')}, want {role}")
        released = bool(run.of("RELEASE"))
        if role == "strike" and not released:
            problems.append(f"{label}: the Strike wing never released")
        # The wing picks its own target; the driver measures to the one it struck (nr-cue: it used to
        # measure to the driver's, 1,830 ft away, and the bomb looked 1,800 ft off).
        limit = MISS_FT_MAX.get(run.skill)
        for impact in run.of("IMPACT") if role == "strike" and limit is not None else []:
            if not number(impact.get("miss_ft")) <= limit:
                problems.append(f"{label}: impact {impact.get('miss_ft')} ft from its target, want at most {limit:.0f}")
        if role == "cap" and released:
            problems.append(f"{label}: the CAP wing released on the ground")
    return problems


# The helicopter break (docs/spec/ai-strike.md, "Helicopter defence"): the offsets the search may take
# off the threat's bearing, the slowest it flies a break, the steepest flight path it asks for, the
# lowest the helicopter may be in a run it lives through.
BREAK_OFFSETS = (60.0, 90.0, 120.0)
BREAK_KT_MIN = 75.0
BREAK_PITCH_MAX_DEG = 10.0
HELI_AGL_MIN_FT = 150.0


def leave_problems(run: Run, label: str, unknown_shooter: bool) -> list[str]:
    """The ambush rule (John, 2026-10-29): `threat_leave` is the answer to a shot from a site the
    helicopter did not know, so each `threat_leave` follows a `SHOT ... known=no` fired since the
    previous `threat_leave` (or the start). A leave that follows only shots from sites it knew
    (`known=yes`, or `none` for no picture) is a fault in every run; with `unknown_shooter` wanted,
    a leave with no shot at all before it is one too (a gun's ambush has no SHOT line, so only the
    run that lays out a missile ambush asks for this)."""
    problems: list[str] = []
    shots = [(float(f["t"]), f.get("known", "none")) for f in run.of("SHOT") if "t" in f]
    previous = float("-inf")
    for leave in (f for f in run.of("THREAT") if f.get("decision") == "threat_leave"):
        at = float(leave.get("t", "nan"))
        since = [known for t, known in shots if previous < t <= at]
        if since and "no" not in since:
            problems.append(f"{label}: threat_leave at t={at:.1f} after only shots from sites it knew ({since})")
        elif unknown_shooter and not since:
            problems.append(f"{label}: threat_leave at t={at:.1f} with no SHOT from an unknown site before it")
        previous = at
    if unknown_shooter and not any(f.get("known") == "no" for f in run.of("SHOT")):
        problems.append(f"{label}: no SHOT with known=no, the ambush was not by a site it did not know")
    return problems


def heli_defense_problems(output: str, runs_wanted: list[dict]) -> list[str]:
    """`ai-strike-heli-defended`: for each run (`label`, `profile` or None for any, `breaks` BREAK
    lines at least, `release` wanted, `no_release`, `hurt`, `kill`, `egress`, `decisions` the THREAT reasons
    wanted in this order, `no_decisions` none made, `sead` the SEAD stand-in fired, `unknown_shooter` every
    `threat_leave` follows a SHOT from a site it did not know, `ahead` a move-ahead plan made,
    `outside_reach` never inside the reach of a defence it knew, the summary's `reach_margin_ft`), the AI
    named its pass, flew the helicopter
    break (each BREAK at an offset of 60, 90 or 120 degrees, at 75 knots or more, no steeper than 10
    degrees, never blocked by the ground), made its threat decisions (slice H4: hold off outside a
    known ring, resume, switch targets, go home, leave an ambush's ring), released as wanted, was not
    lost and did not fly lower than 150 ft above the ground."""
    runs = parse_runs(output)
    if len(runs) != len(runs_wanted):
        return [f"{len(runs)} runs finished, want {len(runs_wanted)}"]
    problems: list[str] = []
    for run, want in zip(runs, runs_wanted):
        label = want["label"]
        problems += consistency_problems(run, label)
        named = (run.of("PROFILE") or [{}])[-1]
        if want.get("profile") and named.get("name") != want["profile"]:
            problems.append(f"{label}: flew PROFILE {named.get('name')}, want {want['profile']}")
        breaks = run.of("BREAK")
        if len(breaks) < want.get("breaks", 0):
            problems.append(f"{label}: {len(breaks)} BREAK lines, want at least {want['breaks']}")
        for line in breaks:
            at = f"{label}: BREAK at t={line.get('t')}"
            if number(line.get("offset")) not in BREAK_OFFSETS:
                problems.append(f"{at} is {line.get('offset')} degrees off the bearing, want one of {BREAK_OFFSETS}")
            if not number(line.get("speed_kt")) >= BREAK_KT_MIN:
                problems.append(f"{at} at {line.get('speed_kt')} kt, want {BREAK_KT_MIN:.0f} or more")
            if not abs(number(line.get("pitch"))) <= BREAK_PITCH_MAX_DEG:
                problems.append(f"{at} asks for a flight path of {line.get('pitch')} degrees")
            if number(line.get("blocked_s")) != 0:
                problems.append(f"{at} is blocked by the ground for {line.get('blocked_s')} s")
        made = [line.get("decision") for line in run.of("THREAT")]
        wanted = [f"threat_{d}" for d in want.get("decisions", [])]
        rest = iter(made)
        if not all(any(d == m for m in rest) for d in wanted):
            problems.append(f"{label}: threat decisions {made}, want {wanted} in that order")
        if want.get("no_decisions") and made:
            problems.append(f"{label}: threat decisions {made}, want none")
        if want.get("ahead") and not run.of("AHEAD"):
            problems.append(f"{label}: no move-ahead plan (AHEAD line)")
        if want.get("outside_reach") and not number(run.summary.get("reach_margin_ft")) >= 0:
            problems.append(f"{label}: came {run.summary.get('reach_margin_ft')} ft inside the reach of a "
                            "defence it knew")
        problems += leave_problems(run, label, want.get("unknown_shooter", False))
        if want.get("sead") and not run.of("SEAD"):
            problems.append(f"{label}: the SEAD stand-in never fired")
        if want.get("release") and not (run.of("RELEASE") and run.of("IMPACT")):
            problems.append(f"{label}: no release and impact")
        if want.get("egress") and not run.of("EGRESS"):
            problems.append(f"{label}: never egressed")
        if want.get("no_release") and run.of("RELEASE"):
            problems.append(f"{label}: released into a ring it knew ({run.first('RELEASE')})")
        if want.get("hurt") and not _target_hurt(run):
            problems.append(f"{label}: the target is unhurt ({run.summary.get('target_hp')})")
        if want.get("kill") and not any(k.get("target") == "yes" for k in run.of("KILL")):
            problems.append(f"{label}: the target was not killed ({run.summary.get('target_hp')})")
        if run.of("LOST"):
            problems.append(f"{label}: the helicopter was lost ({run.first('LOST')})")
        low = number(run.summary.get("min_agl_ft"))
        if not low >= HELI_AGL_MIN_FT:
            problems.append(f"{label}: flew down to {run.summary.get('min_agl_ft')} ft above the ground")
    return problems


def quick_heli_problems(output: str) -> list[str]:
    """`qmc-strike-heli`: a Quick Mission's friendly wing 2 of two helicopters with a ground target
    set (an Mi-24 pair, then an AH-64 pair): the mission gave them the Strike role, started them at
    1,500 ft over the ground, and they released. A loss to the target's defences is allowed."""
    runs = parse_runs(output)
    if len(runs) != 2:
        return [f"{len(runs)} runs finished, want 2"]
    problems: list[str] = []
    for run, label in zip(runs, ("mi24 pair", "ah64 pair")):
        problems += consistency_problems(run, label)
        found = run.first("ROLE")
        if found is None:
            problems.append(f"{label}: no ROLE line")
        elif found.get("role") != "strike":
            problems.append(f"{label}: the wing was given role {found.get('role')}, want strike")
        height = number((run.first("start") or {}).get("alt_ft"))
        if not abs(height - 1500.0) <= 100.0:
            problems.append(f"{label}: started {height:.0f} ft over the ground, want 1,500")
        if not run.of("RELEASE"):
            problems.append(f"{label}: the Strike wing never released")
    return problems


def strike_problems(output: str, runs_wanted: list[dict]) -> list[str]:
    """The behaviour scenarios: for each run (a dict with `label`, `profile`, `skill`, and
    optional `miss` (bomb miss limit) or `weapon_hit`), the AI named the pass it flew, released,
    ended the round near the target, left, and was not lost."""
    runs = parse_runs(output)
    if len(runs) != len(runs_wanted):
        return [f"{len(runs)} runs finished, want {len(runs_wanted)}"]
    problems: list[str] = []
    for run, want in zip(runs, runs_wanted):
        label = want["label"]
        problems += consistency_problems(run, label)
        # The pass the AI reports flying: the last PROFILE line (the first is the driver's echo).
        named = (run.of("PROFILE") or [{}])[-1]
        profile = want.get("profile")
        if profile and named.get("name") != profile:
            problems.append(f"{label}: flew PROFILE {named.get('name')}, asked {profile}")
        if not run.of("RELEASE"):
            problems.append(f"{label}: no RELEASE")
        if not run.of("IMPACT"):
            problems.append(f"{label}: no IMPACT")
        limit = want.get("miss")
        if limit is not None and want.get("skill") in CIRCLE_FT:
            problems += aim_problems(run, label, want["skill"])
            if run.of("RELEASE") and all(r.get("inside") == "yes" for r in run.of("RELEASE")):
                limit = min(limit, MISS_FT_INSIDE[want["skill"]])
            # A rocket's or gun's own spread in place of the bomb's scatter.
            limit += want.get("spread", SCATTER_FT) - SCATTER_FT
        for impact in run.of("IMPACT") if limit is not None else []:
            miss = number(impact.get("miss_ft"))
            if not miss <= limit:
                problems.append(f"{label}: impact {impact.get('miss_ft')} ft from the target, want at most {limit:.0f}")
        problems += maneuver_problems(run, label, want)
        lost_allowed = run.of("LOST") and want.get("may_lose")
        # A helicopter leaves too slowly for the driver's one-mile EGRESS inside its settle time.
        egress_wanted = want.get("egress", True) and not want.get("no_egress")
        if limit is not None and egress_wanted and run.of("RELEASE") and not run.of("EGRESS") and not lost_allowed:
            problems.append(f"{label}: no EGRESS after the release")
        if run.of("LOST") and not want.get("may_lose"):
            problems.append(f"{label}: the striker was lost ({run.first('LOST')})")
        if want.get("defends") and not any(a.get("name") == "Defending" for a in run.of("ACTIVITY")):
            problems.append(f"{label}: never defended against the defences")
        if want.get("hit") and not _target_hurt(run):
            problems.append(f"{label}: the target is unhurt ({run.summary.get('target_hp')})")
    return problems


# The committed-to-release window (docs/spec/ai-strike.md, John 2026-10-28): it opens 5 s or less
# from the release (the driver prints the estimate to 0.01 s), and the striker answers a held defence
# on the tick after the release (1/120 s); half a second is room for the controller's hand-over.
COMMIT_WINDOW_S = 5.0
COMMIT_REACT_S = 0.5


def _phase_at(run: Run, t: float) -> str | None:
    """The strike phase the last PHASE line before `t` named."""
    phase = None
    for p in run.of("PHASE"):
        if number(p.get("t")) < t:
            phase = p.get("name")
    return phase


def commit_held_problems(run: Run, label: str) -> list[str]:
    """`ai-strike-commit-aaa`, a held run: a COMMIT window opens at most 5 s from the release; gunfire
    reaches the striker while it is open (`held_defence=yes`, `held_at` inside the window); the run is
    not broken (no Defend between the opening and the release); the bomb leaves inside the window; and
    the striker defends right after the release."""
    problems: list[str] = []
    commits = run.of("COMMIT")
    opened = next((c for c in commits if c.get("open") == "yes"), None)
    if opened is None:
        return [f"{label}: the committed-to-release window never opened"]
    t_open = number(opened.get("t"))
    release_s = number(opened.get("release_s"))
    if not 0.0 <= release_s <= COMMIT_WINDOW_S:
        problems.append(f"{label}: the window opened {opened.get('release_s')} s from the release, want 0 to 5")
    closed = next((c for c in commits if c.get("open") == "no" and number(c.get("t")) >= t_open), None)
    if closed is None:
        return problems + [f"{label}: the window opened at t={t_open:.3f} and never closed"]
    t_close = number(closed.get("t"))
    held_at = number(closed.get("held_at"))
    if closed.get("held_defence") != "yes":
        problems.append(f"{label}: no defence came while the window was open (held_defence={closed.get('held_defence')})")
    elif not t_open <= held_at <= t_close:
        problems.append(f"{label}: the held defence at t={closed.get('held_at')} lies outside the window "
                        f"{t_open:.3f} to {t_close:.3f}")
    inside = [t for t in run.times("RELEASE") if t_open <= t <= t_close]
    if not inside:
        problems.append(f"{label}: no RELEASE inside the window {t_open:.3f} to {t_close:.3f} "
                        f"(releases at {run.times('RELEASE')})")
        return problems
    released = inside[0]
    if held_at > released:
        problems.append(f"{label}: the defence at t={held_at:.3f} came after the release at t={released:.3f}")
    broke = [number(p.get("t")) for p in run.of("PHASE")
             if p.get("name") == "Defend" and t_open <= number(p.get("t")) < released]
    if broke:
        problems.append(f"{label}: the run broke into Defend at t={broke[0]:.3f}, inside the window")
    defended = [number(p.get("t")) for p in run.of("PHASE")
                if p.get("name") == "Defend" and released <= number(p.get("t")) <= released + COMMIT_REACT_S]
    if not defended:
        problems.append(f"{label}: no Defend within {COMMIT_REACT_S} s after the release at t={released:.3f}")
    return problems


def commit_breaks_problems(run: Run, label: str) -> list[str]:
    """`ai-strike-commit-aaa`, the control: the same guns reach the striker before the window opens,
    so the attack breaks into Defend as before, with no window open and nothing released before it."""
    problems: list[str] = []
    defends = [number(p.get("t")) for p in run.of("PHASE") if p.get("name") == "Defend"]
    if not defends:
        return [f"{label}: the striker never defended"]
    t_defend = defends[0]
    if _phase_at(run, t_defend) != "Attack":
        problems.append(f"{label}: the first Defend at t={t_defend:.3f} came in {_phase_at(run, t_defend)}, "
                        "want it to break an attack run")
    early = [number(c.get("t")) for c in run.of("COMMIT") if c.get("open") == "yes" and number(c.get("t")) <= t_defend]
    if early:
        problems.append(f"{label}: the window opened at t={early[0]:.3f}, before the Defend at t={t_defend:.3f}; "
                        "the control must break outside it")
    if any(t <= t_defend for t in run.times("RELEASE")):
        problems.append(f"{label}: released before the Defend at t={t_defend:.3f}")
    return problems


def commit_problems(output: str, runs_wanted: list[dict]) -> list[str]:
    """`ai-strike-commit-aaa`: each wanted run is `held` (gunfire inside the window waits for the
    release) or `breaks` (gunfire before it breaks the run)."""
    runs = parse_runs(output)
    if len(runs) != len(runs_wanted):
        return [f"{len(runs)} runs finished, want {len(runs_wanted)}"]
    problems: list[str] = []
    for run, want in zip(runs, runs_wanted):
        label = want["label"]
        problems += consistency_problems(run, label)
        if want["kind"] == "held":
            problems += commit_held_problems(run, label)
        else:
            problems += commit_breaks_problems(run, label)
    return problems


# The AC-130's orbit from its guns' reach (docs/spec/ai-strike.md#ac-130): the slant range is planned
# at 80% of the 13,000 ft reach (90% at most when the bank cap grows the radius); a moving target
# stretches it a little either way. The bank never passes the AC-130's 30 degrees.
GUN_REACH_FT = 13000.0
ORBIT_SLANT_FT = (8000.0, GUN_REACH_FT)
ORBIT_BANK_MAX_DEG = 30.5
# Bursts the orbit takes to settle before its geometry is held to the limits.
ORBIT_SETTLE_BURSTS = 5


def gunship_problems(output: str, runs_wanted: list[dict]) -> list[str]:
    """`ai-strike-ac130`: for each run (a dict with `label` and `skill`), the AC-130 flew its orbit,
    fired its guns in bursts whose walked-in aim lies inside the skill circle (the ring R to 2R for a
    ring draw), kept the target inside its guns' reach and its bank under 30 degrees once settled,
    killed the target, and was not lost."""
    runs = parse_runs(output)
    if len(runs) != len(runs_wanted):
        return [f"{len(runs)} runs finished, want {len(runs_wanted)}"]
    problems: list[str] = []
    for run, want in zip(runs, runs_wanted):
        label = want["label"]
        problems += consistency_problems(run, label)
        named = (run.of("PROFILE") or [{}])[-1]
        if named.get("name") != "orbit":
            problems.append(f"{label}: flew PROFILE {named.get('name')}, want orbit")
        bursts = run.of("BURST")
        if not bursts:
            problems.append(f"{label}: no BURST")
            continue
        r = CIRCLE_FT[want["skill"]]
        for burst in bursts:
            aim = number(burst.get("aim_ft"))
            edge = r if burst.get("inside") == "yes" else 2 * r
            if not aim <= edge + AIM_SLACK_FT:
                problems.append(f"{label}: burst {burst.get('n')} aimed {burst.get('aim_ft')} ft out, beyond {edge:.0f}")
        for burst in bursts[ORBIT_SETTLE_BURSTS:]:
            slant = number(burst.get("slant_ft"))
            if not ORBIT_SLANT_FT[0] <= slant <= ORBIT_SLANT_FT[1]:
                problems.append(f"{label}: burst {burst.get('n')} from {burst.get('slant_ft')} ft slant, outside the guns' reach")
            if not abs(number(burst.get("bank_deg"))) <= ORBIT_BANK_MAX_DEG:
                problems.append(f"{label}: burst {burst.get('n')} at {burst.get('bank_deg')} degrees of bank")
        if not number(run.summary.get("gun_rounds")) > 0:
            problems.append(f"{label}: no gun rounds")
        if not any(k.get("target") == "yes" for k in run.of("KILL")):
            problems.append(f"{label}: the target was not killed ({run.summary.get('target_hp')})")
        if run.of("LOST"):
            problems.append(f"{label}: the gunship was lost ({run.first('LOST')})")
    return problems


def maneuver_problems(run: Run, label: str, want: dict) -> list[str]:
    """The ST-T2 maneuver checks a run may ask for: `min_dive` (every RELEASE in a dive at least
    this steep, degrees), `max_alt` / `min_alt` (every RELEASE's height above the ground),
    `first_pass` (the first RELEASE on pass 1), `moving` (True: the RELEASE lays a lead on the
    target; False: none) and `bursts` (at least this many gun bursts)."""
    problems: list[str] = []
    releases = run.of("RELEASE")
    for release in releases:
        dive, alt = number(release.get("dive_deg")), number(release.get("alt_ft"))
        if "min_dive" in want and not dive >= want["min_dive"]:
            problems.append(f"{label}: released in a {release.get('dive_deg')} degree dive, want at least {want['min_dive']}")
        if "max_alt" in want and not alt <= want["max_alt"]:
            problems.append(f"{label}: released at {release.get('alt_ft')} ft, want at most {want['max_alt']}")
        if "min_alt" in want and not alt >= want["min_alt"]:
            problems.append(f"{label}: released at {release.get('alt_ft')} ft, want at least {want['min_alt']}")
        lead = number(release.get("lead_ft"))
        if want.get("moving") is True and not lead > 0:
            problems.append(f"{label}: no lead on the moving target ({release.get('lead_ft')})")
        if want.get("moving") is False and not lead == 0:
            problems.append(f"{label}: a lead of {release.get('lead_ft')} ft on a standing target")
    if want.get("first_pass") and releases and releases[0].get("pass") != "1":
        problems.append(f"{label}: first release on pass {releases[0].get('pass')}, want pass 1")
    if "bursts" in want and len([r for r in releases if "burst" in r]) < want["bursts"]:
        problems.append(f"{label}: {len(releases)} gun bursts, want at least {want['bursts']}")
    if want.get("damaged"):
        # A gun run walks its rounds onto the target (ST-G): some hit, and it is hurt.
        hits = sum(int(i.get("hits", "0")) for i in run.of("IMPACT"))
        damage = sum(int(i.get("damage", "0")) for i in run.of("IMPACT"))
        if not hits > 0 or not damage > 0:
            problems.append(f"{label}: the bursts hit {hits} times for {damage} hit points")
        if not _target_hurt(run):
            problems.append(f"{label}: the target is unhurt ({run.summary.get('target_hp')})")
    return problems


def aim_problems(run: Run, label: str, skill: int) -> list[str]:
    """Each RELEASE's drawn aim offset against the skill's circle R: inside R when the draw says
    `inside=yes`, in the ring R to 2R when it says `no` (docs/spec/ai-strike.md)."""
    problems: list[str] = []
    r = CIRCLE_FT[skill]
    for release in run.of("RELEASE"):
        aim = number(release.get("aim_ft"))
        inside = release.get("inside")
        if inside == "yes" and not aim <= r + AIM_SLACK_FT:
            problems.append(f"{label}: aim {release.get('aim_ft')} ft is outside the {r:.0f} ft circle it was drawn in")
        elif inside == "no" and not r - AIM_SLACK_FT <= aim <= 2 * r + AIM_SLACK_FT:
            problems.append(f"{label}: aim {release.get('aim_ft')} ft is outside the ring {r:.0f} to {2 * r:.0f} ft")
        elif inside not in ("yes", "no"):
            problems.append(f"{label}: a RELEASE without its aim draw")
    return problems


def _target_hurt(run: Run) -> bool:
    now, _, start = run.summary.get("target_hp", "").partition("/")
    return now == "gone" or (now.lstrip("-").isdigit() and start.isdigit() and int(now) < int(start))


# A guided bomb lands on its spot: the aim is the target itself.
GUIDED_MISS_FT = 150.0
# Seconds a guided weapon may fly with the spot it follows not lasing (rounding of the ticks around
# the release and the impact).
BLIND_S_MAX = 1.0
# Seconds a laser bomb must have fallen for the hold to have mattered: an exit instead of the hold
# loses the spot long before this.
FALL_S_MIN = 20.0
# The longest a designator pod's laser can reach, feet (10 nm).
LASER_REACH_FT = 60760.0


def _phases(run: Run) -> list[tuple[float, str]]:
    return [(float(f["t"]), f["name"]) for k, f in run.lines if k == "PHASE"]


def _lasers(run: Run, plane: str) -> list[tuple[float, str, str]]:
    return [(float(f["t"]), f["state"], f.get("mask", "none")) for k, f in run.lines
            if k == "LASER" and f.get("plane") == plane]


def laser_self_run_problems(run: Run, label: str, *, cone: bool) -> list[str]:
    """One AI laser strike on its own designator: the hold from the release to the landing, the spot
    kept (a pod), the target destroyed, then the exit."""
    problems = consistency_problems(run, label)
    release, impact = run.first("RELEASE"), run.first("IMPACT")
    plane = (run.first("start") or {}).get("plane", "0")
    if release is None or impact is None:
        return problems + [f"{label}: no release or no impact"]
    if not release.get("weapon", "").startswith("GBU10"):
        problems.append(f"{label}: released {release.get('weapon')}, want a GBU-10")
    if release.get("guided") != "Guiding" or release.get("spot_owner") != plane:
        problems.append(f"{label}: the bomb was {release.get('guided')} on aircraft "
                        f"{release.get('spot_owner')}'s spot, want guiding on its own ({plane})")
    t_release, t_impact = float(release["t"]), float(impact["t"])
    phases = _phases(run)
    names = [n for _, n in phases]
    if "Lase" not in names:
        problems.append(f"{label}: never held the spot (phases {names})")
    else:
        t_lase = next(t for t, n in phases if n == "Lase")
        if abs(t_lase - t_release) > 1.0:
            problems.append(f"{label}: the hold began at {t_lase}, the release was at {t_release}")
        exits = [t for t, n in phases if n == "Exit" and t >= t_lase]
        if not exits:
            problems.append(f"{label}: never exited after the hold")
        elif not cone and exits[0] < t_impact - 1.0:
            # A cone cannot follow a target it is flying over (its hold ends inside 2,000 ft): the
            # bomb keeps following the spot's last point, as a masked laser does.
            problems.append(f"{label}: left the hold at {exits[0]}, {t_impact - exits[0]:.1f} s before the bomb landed")
    holds = [f for k, f in run.lines if k == "HOLD"]
    if not cone:
        if len(holds) < 2:
            problems.append(f"{label}: fewer than two hold readings")
        if not any(abs(number(h.get("bank_deg"))) > 10 for h in holds):
            problems.append(f"{label}: the hold was not a turn: {[h.get('bank_deg') for h in holds]}")
        far = [h for h in holds if not number(h.get("range_ft")) < LASER_REACH_FT]
        if far:
            problems.append(f"{label}: held out of the laser's reach: {far[0]}")
        lost = [(t, state, mask) for t, state, mask in _lasers(run, plane)
                if t_release <= t <= t_impact and state != "Lasing"]
        if lost:
            problems.append(f"{label}: the spot was lost in the fall: {lost[0]}")
        if not number(run.summary.get("blind_s")) <= BLIND_S_MAX:
            problems.append(f"{label}: the bomb flew {run.summary.get('blind_s')} s with no spot")
    if not number(run.summary.get("fall_s")) >= FALL_S_MIN:
        problems.append(f"{label}: the bomb fell only {run.summary.get('fall_s')} s")
    if not number(impact.get("miss_ft")) <= GUIDED_MISS_FT:
        problems.append(f"{label}: landed {impact.get('miss_ft')} ft from the target, want at most {GUIDED_MISS_FT:.0f}")
    if not _target_hurt(run):
        problems.append(f"{label}: the target is unhurt ({run.summary.get('target_hp')})")
    if not run.of("EGRESS"):
        problems.append(f"{label}: no EGRESS")
    if run.of("LOST"):
        problems.append(f"{label}: the striker was lost")
    return problems


def laser_self_problems(output: str) -> list[str]:
    """`ai-strike-laser-self`: an F-16C's Pave Spike (a hemisphere pod) and an Su-25's built-in
    designator (a cone), each on a GBU-10 of its own."""
    runs = parse_runs(output)
    if len(runs) != 2:
        return [f"{len(runs)} runs finished, want 2"]
    return (laser_self_run_problems(runs[0], "F-16C Pave Spike", cone=False)
            + laser_self_run_problems(runs[1], "Su-25 built-in designator", cone=True))


def laser_buddy_problems(output: str) -> list[str]:
    """`ai-strike-laser-buddy`: three runs. (1) An AI F-16C lases for a player's F/A-18D (the driver
    gives the order the wing order "Lase my target" gives), which drops a GBU-10 on its spot; the
    target page would name the F-16C by its callsign. (2) A player's F-16C lases by hand and an AI
    F-14 with only a GBU-10 and no designator chooses the bomb and drops it on that spot. (3) The
    same F-14 with nobody lasing does not choose the bomb."""
    head, sep, rest = output.partition("a2g-run: done")
    if not sep:
        return ["the player's run did not finish"]
    problems: list[str] = []
    label = "AI F-16C lases for a player's GBU-10"
    release, sight = A2G_RELEASE.search(head), A2G_SIGHT.search(head)
    impact, target = A2G_IMPACT.search(head), A2G_TARGET.search(head)
    ordered = re.search(r"^a2g-run: buddy-ai-ordered t=\S+ plane=(\d+) ok=(\w+)", head, re.M)
    lasing = re.search(r'^a2g-run: buddy-lasing t=\S+ range \d+ callsign "([^"]*)"', head, re.M)
    if ordered is None or ordered.group(2) != "true":
        problems.append(f"{label}: the AI was not ordered to lase ({ordered and ordered.group(0)})")
    if lasing is None:
        problems.append(f"{label}: the AI never lased")
    elif lasing.group(1) != "Friendly 2-1":
        problems.append(f"{label}: the target page would name the lasing pilot {lasing.group(1)!r}, want 'Friendly 2-1'")
    if release is None:
        problems.append(f"{label}: nothing released")
    elif (release.group(3), release.group(4)) != ("Guiding", "1"):
        problems.append(f"{label}: released {release.group(0)}, want guiding on aircraft 1's spot")
    if sight is None or (sight.group(4), sight.group(6), sight.group(7)) != ("yes", "yes", "Off"):
        problems.append(f"{label}: at release the sight said {sight and sight.group(0)}, want IN RNG on a buddy's spot with no laser of its own")
    if impact is None or float(impact.group(3)) > 40:
        problems.append(f"{label}: the bomb came down {impact and impact.group(3)} ft from the tank")
    if target is None or int(target.group(1)) != 0 or not A2G_KILL.search(head):
        problems.append(f"{label}: the tank was not destroyed")

    runs = parse_runs(rest)
    if len(runs) != 2:
        return problems + [f"{len(runs)} AI runs finished, want 2"]
    drop, none = runs
    label = "AI F-14 drops on a player's spot"
    problems += consistency_problems(drop, label)
    release, impact = drop.first("RELEASE"), drop.first("IMPACT")
    buddy = (drop.first("DESIGNATE") or {}).get("buddy")
    if buddy is None:
        problems.append(f"{label}: the player designated nothing")
    if release is None or impact is None:
        return problems + [f"{label}: no release or no impact"]
    if not release.get("weapon", "").startswith("GBU10"):
        problems.append(f"{label}: released {release.get('weapon')}, want a GBU-10")
    if release.get("guided") != "Guiding" or release.get("spot_owner") != buddy:
        problems.append(f"{label}: the bomb was {release.get('guided')} on aircraft {release.get('spot_owner')}'s spot, want the player's ({buddy})")
    t_release = float(release["t"])
    lased = [t for t, state, _ in _lasers(drop, buddy or "") if state == "Lasing"]
    if not lased or lased[0] > t_release:
        problems.append(f"{label}: the player's spot was not up before the release")
    if any(state == "Lasing" for _, state, _ in _lasers(drop, "0")):
        problems.append(f"{label}: the F-14 has no designator, yet its laser lased")
    if "Lase" in [n for _, n in _phases(drop)]:
        problems.append(f"{label}: a carrier with no designator held a spot")
    if not number(impact.get("miss_ft")) <= GUIDED_MISS_FT:
        problems.append(f"{label}: landed {impact.get('miss_ft')} ft from the target")
    if not _target_hurt(drop):
        problems.append(f"{label}: the target is unhurt")
    if not number(drop.summary.get("blind_s")) <= BLIND_S_MAX:
        problems.append(f"{label}: the bomb flew {drop.summary.get('blind_s')} s with no spot")

    label = "AI F-14 with nobody lasing"
    problems += consistency_problems(none, label)
    # No laze support (ST-G, John 2026-10-12): it says so and drops the laser bomb by hand as an
    # unguided bomb, which waits for a spot it never gets.
    problems += no_laze_manual_problems(none, label)
    return problems


def no_laze_manual_problems(run: Run, label: str) -> list[str]:
    """A laser bomb carrier with no designator and no spot: it announces "No laze support, dropping
    manual" (once, in the NOLAZE line the radio call and the replay event mirror) and drops the bomb
    on the target as an unguided bomb: CCIP/LVL on the ballistic solution, the unguided skill
    circle and scatter, the fall waiting for a spot that never comes."""
    problems: list[str] = []
    calls = run.of("NOLAZE")
    if len(calls) != 1:
        return [f"{label}: {len(calls)} NOLAZE lines, want one"]
    call = calls[0]
    if (call.get("decision"), call.get("reason")) != ("manual", "no_laze_manual"):
        problems.append(f"{label}: decided {call.get('decision')} / {call.get('reason')}, want manual")
    if call.get("words") != "No laze support, dropping manual":
        problems.append(f"{label}: said {call.get('words')!r}")
    if not str(call.get("weapon", "")).startswith("GBU"):
        problems.append(f"{label}: the store is {call.get('weapon')}, want a GBU")
    releases, impacts = run.of("RELEASE"), run.of("IMPACT")
    if not releases or not impacts:
        return problems + [f"{label}: no manual drop ({len(releases)} releases, {len(impacts)} impacts)"]
    release = releases[0]
    if release.get("guided") != "Waiting" or release.get("spot_owner") != "none":
        problems.append(f"{label}: released {release.get('guided')} on {release.get('spot_owner')}, want waiting with no spot")
    if "aim_ft" not in release or "lead_ft" not in release:
        problems.append(f"{label}: the release carries no skill aim ({release})")
    named = (run.of("PROFILE") or [{}])[-1].get("name")
    if named in (None, "none", "laydown"):
        problems.append(f"{label}: flew PROFILE {named}, want a bomb pass")
    miss = number(impacts[0].get("miss_ft"))
    limit = MISS_FT_MAX[run.skill] + 150.0
    if not miss <= limit:
        problems.append(f"{label}: the bomb landed {miss} ft from the target, want at most {limit:.0f}")
    if "Lase" in [n for _, n in _phases(run)]:
        problems.append(f"{label}: a carrier with no designator held a spot")
    return problems


def no_laze_rtb_problems(run: Run, label: str) -> list[str]:
    """A laser store that cannot fall unguided and no spot: "No laze support, RTB", no release."""
    problems: list[str] = []
    calls = run.of("NOLAZE")
    if len(calls) != 1:
        return [f"{label}: {len(calls)} NOLAZE lines, want one"]
    call = calls[0]
    if (call.get("decision"), call.get("reason")) != ("rtb", "no_laze_rtb"):
        problems.append(f"{label}: decided {call.get('decision')} / {call.get('reason')}, want rtb")
    if call.get("words") != "No laze support, RTB":
        problems.append(f"{label}: said {call.get('words')!r}")
    if run.of("RELEASE"):
        problems.append(f"{label}: released after deciding to go home")
    names = [n for _, n in _phases(run)]
    if "Home" not in names:
        problems.append(f"{label}: never went home ({names})")
    if "Attack" in names:
        problems.append(f"{label}: flew a pass with a laser missile and no spot ({names})")
    return problems


def no_laze_problems(output: str) -> list[str]:
    """`ai-strike-no-laze`: an AI F-14 with a GBU-10 and nobody lasing drops it by hand and kills the
    tank at three skills; an AI F-14 with an AS-14 laser missile says so and goes home."""
    runs = parse_runs(output)
    if len(runs) != 4:
        return [f"{len(runs)} runs finished, want 4"]
    problems: list[str] = []
    for run, skill in zip(runs[:3], (0, 1, 2)):
        label = f"F-14 GBU-10 skill {skill}"
        problems += consistency_problems(run, label)
        problems += no_laze_manual_problems(run, label)
        if run.skill != skill:
            problems.append(f"{label}: ran at skill {run.skill}")
    label = "F-14 AS-14"
    problems += consistency_problems(runs[3], label)
    problems += no_laze_rtb_problems(runs[3], label)
    return problems


def _runs(skills_and_profiles, miss=True, **extra) -> list[dict]:
    wanted = []
    for skill, profile in skills_and_profiles:
        wanted.append(
            dict(label=f"skill {skill} {profile or 'free'}", profile=profile, skill=skill,
                 miss=MISS_FT_MAX[skill] if miss else None, **extra)
        )
    return wanted


# The plan's bomb coverage: each pass forced once at the middle skill, and every skill on the dive.
BOMB_RUNS = [(2, p) for p in ("straight", "popup", "fasthigh", "dive", "level")] + [(s, "dive") for s in (0, 1, 3)]


# The runs of `ai-wing-ground-order`, in the order they are given.
ORDER_RUNS = [
    dict(label="Alt+E", kind="strike"),
    dict(label="Alt+R", kind="strike"),
    dict(label="Alt+F", kind="strike"),
    dict(label="Alt+W", kind="class", release=False),
    dict(label="refused", kind="refused"),
]

# ST-T2: a rocket's spread and a gun burst's (the middle of the rounds that reach the ground) on
# top of the aim, in place of the bomb's scatter: fitted from the driver runs (rockets 90 to 125 ft
# from a 90 ft aim at skill 2; bursts 85 to 120 ft from 85 to 102 ft aims).
ROCKET_SPREAD_FT = 250.0
GUN_SPREAD_FT = 150.0
# The A-10's strafing target: the column's lighter vehicle (150 hit points; the tanks have 200).
STRAFE_UNIT = "0x50000003"
# Static and moving targets for the lead comparison: the Ukraine column (moving, about 50 ft/s)
# and the Ukraine factory (standing).
LEAD_STEMS = (("QUCOL", True), ("QUFACT", False))


def pilot_run(theater: str, stem: str, *rest: str) -> list[str]:
    return ["--a2g-run", theater, stem, "--pilot", "ai", *rest]


def scenarios() -> list[Scenario]:
    qucol = ["--aircraft", "a4e", "--load", "MK82", "--seconds", "60"]
    smoke = pilot_run("UKR", "QUCOL", *qucol)
    hand = ["--aircraft", "a4e", "--load", "MK82", "--handback", "30", "--mode", "lvl", "--altitude", "5000",
            "--seconds", "80"]
    bombs = ["--aircraft", "a4e", "--load", "MK82", "--seconds", "120"]

    def forced(skill: int, profile: str, *rest: str, theater: str = "UKR", stem: str = "QUCOL") -> list[str]:
        return pilot_run(theater, stem, *rest, "--skill", str(skill), "--profile", profile)

    # The column, not the TVIET truck park: the park's radar-laid guns (26,200 and 44,250 ft
    # reach) shoot an A-4E down within 25 s at 5,000 ft whatever it flies.
    bomb_args = [forced(s, p, *bombs) for s, p in BOMB_RUNS]
    # The straight rocket run at two skills (the pop-up and dive have their own ST-T2 scenarios).
    # Not the Ace: on a short straight run its 10 m circle is rarely met before the run ends.
    rocket_runs = [(1, "straight"), (2, "straight")]
    rockets = ["--aircraft", "a4e", "--load", "LAU61", "--seconds", "120"]
    maverick = ["--aircraft", "a10", "--load", "AGM65G", "--seconds", "120"]
    # Helicopters (slice ST-F) start 4 nm out at 1,500 ft and 120 kt. The AT-2 is judged by the
    # target; the rockets by their miss, like a fighter's. The stand-off attack (flown under SAMs) is
    # forced on a quiet column: the column's SAMs bring a helicopter down before it can show it.
    heli = ["--altitude", "1500", "--speed", "120", "--seconds", "150"]
    at2 = ["--aircraft", "mi24", "--load", "AT2", *heli]
    heli_rockets = ["--aircraft", "ah64", "--load", "LAU61", *heli]
    heli_rockets_out = [*heli_rockets, "--seconds", "300", "--follow-egress"]
    # Light SAMs with the layout of surface seed 16 (an SA-14 team 12,100 ft from the Mi-24's start):
    # ai-strike-heli-defended, "mi24 ambushed by the SA-14 it starts inside". Seed 19, ST-H's layout,
    # holds a SAM that out-ranges the AT-2: since slice H4 the helicopter holds off there
    # (ai-strike-heli-defended).
    sams16 = ["--defenses", "0", "1", "--surface-seed", "16"]
    sams19 = ["--defenses", "0", "1", "--surface-seed", "19"]
    heli_runs = [
        (pilot_run("UKR", "QUCOL", *at2, "--skill", "2"),
         dict(label="mi24 at2 popup", profile="popup", skill=2, miss=None, hit=True)),
        # The rocket runs follow the helicopter out (slice HF1): it turns away from the column and
        # masks low; before, it flew on over the column and a site it had not seen shot it down 2 s
        # after the old run end.
        (pilot_run("UKR", "QUCOL", *heli_rockets_out, "--skill", "2"),
         dict(label="ah64 rockets skill 2", profile="popup", skill=2, miss=MISS_FT_MAX[2])),
        (pilot_run("UKR", "QUCOL", *heli_rockets_out, "--skill", "3"),
         dict(label="ah64 rockets skill 3", profile="popup", skill=3, miss=MISS_FT_MAX[3])),
    ]
    # The committed-to-release window (John, 2026-10-28; docs/spec/ai-strike.md "Committed to the
    # release, 5 s"): an A-4E's forced straight Mk 82 pass on the column under medium AAA, surface
    # seed 1. The window opens at t=23.1 to 23.4 and the release follows 4.5 s later; a gun's rounds
    # reach the striker at t=26.6, about 1.1 s before the release, at every skill. Before the window
    # that broke the run 0.2 s before its release (Vladivostok HQ). The control moves the column's
    # nearest ZSU-23 (0x5000000a, 3,660 ft from the target) 14,000 ft south, under the run line: it
    # fires at t=21.7, 1.5 s before the window would open and 6 s before the release, and the run
    # breaks into Defend as it did before the change.
    commit = ["--aircraft", "a4e", "--load", "MK82", "--profile", "straight", "--defenses", "2", "0",
              "--surface-seed", "1"]
    commit_runs = [
        *((pilot_run("UKR", "QUCOL", *commit, "--skill", str(k), "--seconds", "35"),
           dict(label=f"a4e held skill {k}", kind="held")) for k in (2, 0, 3)),
        (pilot_run("UKR", "QUCOL", *commit, "--skill", "2", "--seconds", "30",
                   "--place-site", "0x5000000a,180,14000"),
         dict(label="a4e breaks before the window", kind="breaks")),
    ]
    # The helicopter's defence (slice ST-H) and threat awareness (slice H4, John's rule of
    # 2026-10-12): the AI picks its own pass; a known SAM or gun ring it does not fly into (it attacks
    # from outside every ring, or holds off up to 3 minutes for SEAD, then switches targets or goes
    # home); a threat it did not know gets the break, then it leaves that ring and resumes once the
    # threat is quiet. Every run lives. The switch to another target outside the rings is the unit
    # tests' (no driver layout has a second target outside every ring).
    sams = ["--defenses", "0", "1"]

    def ambush_site(site: str) -> list[str]:
        """The ambusher's layout (2026-10-29): the shoulder-fired team `site` 7,000 ft south-south-west
        of the target (bearing 190), destroyed 4 s after its first launch. The game hides the team: the
        helicopter spots it only within 3,000 to 4,000 ft by its skill, or once it launches."""
        return ["--place-site", f"{site},190,7000", "--sead-site", site, "--sead-after-shot", "4"]

    defended_runs = [
        # Heavy AAA on the column (before H4 the Mi-24 flew the pop-up through it and was lost). Until
        # 2026-10-29 it fired from a point off the straight line and guided its AT-2 on into the 9,500 ft
        # ring of the column's ZSU-23 3,660 ft from the tank. Since the stand-off's advance is the AT-2's
        # flight at its speed and its range search reaches the AT-2's reliable 14,500 ft (John,
        # 2026-10-29), every firing point planned on the tank's place was spoiled within seconds as the
        # column drove on among the guns, which stand still (it does not drive with them: every gun of
        # `--defenses` stands; the column's nine tanks are its only movers), and it held off at 113 s.
        # Since the move-ahead ambush (John, 2026-10-29) it plans on where the tank will be: from 1 s it
        # sets up west of the road; the column turns south-east at about 62 s, which spoils that plan;
        # it plans again on the new course and fires at about 86 s from 13,900 ft, its advance outside
        # every gun ring it knows, and the AT-2 hurts the tank (120 of 200). Its exit turn takes it up to
        # about 960 ft into the 2,000 ft margin of a ZSU-23's ring, never into a gun's reach. No hold-off
        # is called.
        (pilot_run("UKR", "QUCOL", *at2, "--skill", "1", "--defenses", "3", "0"),
         dict(label="mi24 under heavy AAA", profile="standoff", no_decisions=True, ahead=True, release=True,
              hurt=True, outside_reach=True)),
        # Light AAA at the factory: it sets out on the pop-up, learns of a ZSU-23 near the factory 20 s
        # in and, its ring covering the factory, gives the pop-up up for a stand-off from outside the
        # ring ("not pressed through"). Until 0.2.2 it flew the pop-up on into the guns and was lost
        # guiding the energy rules' slower AT-2; the faster one had hidden it. The gun stood 2,650 ft
        # from the factory; since the stand-off's advance is the AT-2's flight (John, 2026-10-29) no
        # firing point leaves room for it outside that ring, so the run moves the gun 4,500 ft north of
        # the factory, where the stand-off still clears it.
        (pilot_run("UKR", "QUFACT", *at2, "--skill", "1", "--defenses", "1", "0",
                   "--place-site", "0x5000000c,0,4500"),
         dict(label="mi24 under light AAA at the factory", profile="standoff", release=True, kill=True)),
        # A SAM that out-ranges the AT-2 (the open ST-H question): hold off, then home.
        (pilot_run("UKR", "QUCOL", *at2, "--skill", "2", *sams19, "--seconds", "240"),
         dict(label="mi24 under a SAM that out-ranges the AT-2", decisions=["hold_off", "home"],
              no_release=True)),
        # An SA-14 on the target (0x50000009, 2,935 ft from the tank): hold off, then home, no release.
        # Since the game hides shoulder-fired teams and the move-ahead ambush (John, 2026-10-29) both
        # SA-14 teams of the seed are hidden, so the way there is an ambush and an attack: the team on
        # the target launches at 29 s from 15,900 ft (`SHOT ... known=no`), its launch gives it away, the
        # helicopter holds off its ring (`threat_hold_off`) and breaks the round (`threat_leave`, the
        # team known only by that launch) and takes one hit. The column drives out from under that
        # ring, and at 198 s, 11 s before the three minutes run out, a move-ahead shot from 14,500 ft
        # stands clear of it (`threat_resume`, an AHEAD line, 8,800 ft or more outside the team's
        # reach). On the way the seed's other hidden SA-14 team (0x5000000d), 16,500 ft off, launches
        # at 204 s (`known=no`): it holds off again, leaves that ambush and, with nothing clear of the
        # two rings when that hold's three minutes run out, goes home at 385 s without firing.
        (pilot_run("UKR", "QUCOL", *at2, "--skill", "2", *sams, "--surface-seed", "4", "--seconds", "400"),
         dict(label="mi24 holds off then goes home",
              decisions=["hold_off", "leave", "resume", "hold_off", "leave", "home"], ahead=True,
              no_release=True, unknown_shooter=True)),
        # The same with SEAD killing the sites a minute in: it resumes and fires.
        (pilot_run("UKR", "QUCOL", *at2, "--skill", "2", *sams, "--surface-seed", "4", "--seconds", "240",
                   "--sead-at", "60"),
         dict(label="mi24 resumes after SEAD", decisions=["hold_off", "resume"], sead=True, release=True,
              hurt=True)),
        # An ambush by a site it had not seen (slice H4's rule; restored in slice HF1, John
        # 2026-10-13; a new layout for John's rules of 2026-10-29, when `threat_leave` came to mean a
        # shot from an unknown site). A lone SA-7 of seed 7 (an SA-14 of seed 8, at skill 3, in the
        # second run) stands 7,000 ft south-south-west of the factory. Since 2026-10-29 the game hides a
        # shoulder-fired team (John): the helicopter's eye spots it only within 3,500 ft (skill 2;
        # 4,000 ft at skill 3), not 19,968 ft, and its launch gives it away. Before, the runs needed the
        # driver's `--site-sight` stand-in: terrain cannot hide a site here, because a site needs about
        # 20 s of clear line before its first launch and the eye lists it within 6 s of the line
        # opening. The site launches at the helicopter 10,900 ft away on its way in (`SHOT ... known=no`),
        # it breaks (`threat_leave`), takes the first round's hit, leaves the ambush ring, holds off and
        # resumes once the ring has been quiet a minute; the factory dies to the shot after the resume.
        # The SEAD stand-in destroys the site 4 s after its first launch (`--sead-after-shot`): its
        # second round, 10 s later, would hit too, take the helicopter below half its hit points and
        # send it home. The margins are in docs/spec/ai-strike.md, "The ambush layout of 2026-10-29".
        (pilot_run("UKR", "QUFACT", *at2, "--skill", "2", *sams, "--surface-seed", "7", "--seconds", "400",
                   "--follow-threat", *ambush_site("0x5000000f")),
         dict(label="mi24 ambushed by an SA-7", decisions=["leave", "resume"], breaks=2, release=True, kill=True,
              unknown_shooter=True)),
        (pilot_run("UKR", "QUFACT", *at2, "--skill", "3", *sams, "--surface-seed", "8", "--seconds", "400",
                   "--follow-threat", *ambush_site("0x5000000e")),
         dict(label="mi24 ambushed by an SA-14", decisions=["leave", "resume"], breaks=2, release=True, kill=True,
              unknown_shooter=True)),
        # From 4 nm at 200 ft on seed 1 the AI once chose the stand-off under the SAMs and turned
        # away at once; since a line out of a ring must lead straight out it holds off from the
        # start (no pass is named), with the same decisions.
        (pilot_run("UKR", "QUCOL", "--aircraft", "mi24", "--load", "AT2", "--altitude", "200", "--from", "4",
                   "--speed", "120", "--seconds", "200", "--skill", "2", *sams, "--surface-seed", "1"),
         dict(label="mi24 holds off the SAMs it would stand off under",
              decisions=["hold_off", "home"], no_release=True)),
        # Surface seed 16: the Mi-24 starts 12,100 ft from an SA-14 team, inside its ring. Until 0.2.2 it
        # flew the pop-up through the warnings and broke (slice ST-H); then, knowing the SA-14 from the
        # start, it flew the stand-off from outside the ring and hurt the tank (ai-strike-heli, "mi24 at2
        # under SAMs it starts inside"). Since the game hides shoulder-fired teams (John, 2026-10-29) it
        # does not know the team: the team launches 20 s in (`SHOT ... known=no`), the helicopter
        # breaks and leaves (`threat_leave`), the launch gives the team away, its second round 10 s
        # later is a known site's shot (`known=yes`), both rounds hit and it goes home without firing.
        # The line out of a ring it knows is the unit tests' (helicopter_threat_tests).
        (pilot_run("UKR", "QUCOL", *at2, "--skill", "2", *sams16),
         dict(label="mi24 ambushed by the SA-14 it starts inside", decisions=["leave"], breaks=2,
              no_release=True, unknown_shooter=True)),
    ]
    # The AC-130 (slice ST-F) on the moving armoured column: the TVIET truck park's radar-laid guns
    # (26,200 and 44,250 ft reach) shoot it down on its way in, so the quiet column it is.
    gunship_skills = (0, 2, 3)
    gunship = ["--aircraft", "ac130", "--load", "C_25.JT", "--altitude", "6000", "--speed", "220",
               "--from", "3", "--seconds", "240"]
    gunship_args = [pilot_run("UKR", "QUCOL", *gunship, "--skill", str(k)) for k in gunship_skills]
    jdam = ["--aircraft", "f16c", "--load", "GBU30", "--station", "2", "--count", "2", "--seconds", "120"]
    column = ["--aircraft", "a4e", "--load", "MK82", "--order-at", "3", "--seconds", "90"]
    order_args = {
        "engage-my-target": pilot_run("UKR", "QUCOL", *column, "--order", "engage-my-target"),
        "engage-from-formation": pilot_run("UKR", "QUCOL", *column, "--order", "engage-from-formation"),
        "engage-ir-laser": pilot_run("UKR", "QUFACT", "--aircraft", "f16c", "--load", "GBU30", "--station", "2",
                                     "--count", "2", "--order-at", "3", "--seconds", "90",
                                     "--order", "engage-ir-laser"),
        # The heavily defended column: the lead designates a SAM (the nearest one its radar lists), and the
        # wingman is given every one of its class. The defences may bring the wingman down.
        "engage-class": pilot_run("UKR", "QUCOL", "--defenses", "3", "3", "--aircraft", "a4e", "--load", "MK82",
                                  "--order-at", "3", "--seconds", "60", "--order", "engage-class"),
        "refused": pilot_run("UKR", "QUCOL", "--aircraft", "f14", "--station", "0", "--load", "AIM9M",
                             "--order-at", "3", "--seconds", "30", "--order", "engage-my-target"),
    }
    lase_args = pilot_run("UKR", "QUFACT", "--aircraft", "f16c", "--load", "GBU30", "--station", "2",
                          "--count", "2", "--order-at", "3", "--seconds", "60", "--order", "lase-my-target")
    # The lasing runs: a GBU-10 from 15,000 ft at 480 kt for the pod (an F-16C's Pave Spike), from
    # 8,000 ft at 400 kt for the cone (an Su-25's built-in designator).
    gbu10 = ["--load", "GBU10", "--station", "2", "--count", "2"]
    laser_pod = pilot_run("UKR", "QUCOL", "--aircraft", "f16c", *gbu10, "--altitude", "15000", "--speed", "480",
                          "--from", "6", "--seconds", "200", "--skill", "2")
    laser_cone = pilot_run("UKR", "QUCOL", "--aircraft", "su25", *gbu10, "--altitude", "8000", "--speed", "400",
                           "--from", "6", "--seconds", "200", "--skill", "2")
    # A player's F/A-18D drops on an AI F-16C's spot; an AI F-14 with a GBU-10 and no designator
    # drops on a player's F-16C's spot; with nobody lasing it chooses not to.
    ai_lases = ["--a2g-run", "UKR", "QUCOL", "--attack", "laser", "--aircraft", "f18", *gbu10, "--altitude", "12000",
                "--speed", "450", "--from", "6", "--seconds", "120", "--buddy", "f16c", "--buddy-ai"]
    ai_drops = pilot_run("UKR", "QUCOL", "--aircraft", "f14", *gbu10, "--altitude", "12000", "--speed", "450",
                         "--from", "6", "--seconds", "150", "--skill", "2", "--buddy", "f16c")
    ai_waits = pilot_run("UKR", "QUCOL", "--aircraft", "f14", *gbu10, "--altitude", "12000", "--speed", "450",
                         "--from", "6", "--seconds", "100", "--skill", "2", "--choose-store")
    a4e_bombs = ["--aircraft", "a4e", "--load", "MK82"]

    def run_list(entries):
        """(args, want) pairs to (first args, then steps, wants)."""
        return entries[0][0], [Step(a, timeout=400) for a, _ in entries[1:]], [w for _, w in entries]

    def want(skill, profile, **extra):
        label = extra.pop("label", f"skill {skill} {profile}")
        return dict(label=label, profile=profile, skill=skill, miss=MISS_FT_MAX[skill], **extra)

    steep = [(forced(s, "dive", *a4e_bombs, "--seconds", "150"),
              want(s, "dive", min_dive=45, min_alt=FLOOR_CLEAR_FT, moving=True)) for s in SKILLS]
    strafe = [(pilot_run("UKR", "QUCOL", "--unit", STRAFE_UNIT, "--aircraft", "a10", "--load", "GAU8",
                         "--seconds", "90", "--skill", str(s), "--profile", "strafe"),
               want(s, "strafe", spread=GUN_SPREAD_FT, bursts=3, no_egress=True, may_lose=True,
                    max_alt=4000, damaged=True)) for s in SKILLS]
    rockets_popup = [(forced(2, "popup", *rockets), want(2, "popup", spread=ROCKET_SPREAD_FT, no_egress=True))]
    rockets_dive = [(forced(2, "dive", *rockets), want(2, "dive", spread=ROCKET_SPREAD_FT, no_egress=True))]
    snakeye = [(forced(s, "laydown", "--aircraft", "a4e", "--load", "MK82HD", "--seconds", "120"),
                want(s, "laydown", max_alt=1000, moving=True)) for s in (1, 2)]
    heavy = [(pilot_run("UKR", "QUFACT", "--aircraft", "c130", "--load", "MK82", "--station", "0", "--count", "8",
                        "--mission-altitude", "10000", "--altitude", "7000", "--speed", "250", "--from", "8",
                        "--seconds", "360", "--skill", "2", "--profile", "level"),
              want(2, "level", min_alt=9500, moving=False, label="heavy level from the 10,000 ft mission height"))]
    offsets = [(forced(2, p, *a4e_bombs, "--offset", str(off), "--seconds", "150"),
                want(2, p, first_pass=True, label=f"{p} {off} ft off the line"))
               for p in ("level", "dive") for off in (1000, 3000)]

    def lead_runs(profile):
        return [(pilot_run("UKR", stem, *a4e_bombs, "--seconds", "240", "--skill", str(s), "--profile", profile),
                 want(s, profile, moving=moving, label=f"skill {s} {profile} {stem} {'moving' if moving else 'static'}"))
                for stem, moving in LEAD_STEMS for s in SKILLS]

    maneuvers = [
        ("ai-strike-dive-steep", steep, 900,
         "An A-4E's Mk 82 dive at every skill: rolled inverted onto a dive steeper than 45 degrees (retail's "
         "roll-in), released above the 3,000 ft floor with a lead on the moving column, within the circle."),
        ("ai-strike-strafe", strafe, 900,
         "An A-10's GAU-8 gun run on the column's lighter vehicle at every skill: shallow dive, each burst "
         "opens on its own seeded offset inside the circle (or the ring) and its rounds walk onto the target "
         "over the skill's walk-in time, the bursts' middle within the circle plus the gun's spread, break-off "
         "low; the 150 hit point vehicle is hit and hurt at every skill (the walk-in's rounds, John, 2026-10-12)."),
        ("ai-strike-rockets-popup", rockets_popup, 600,
         "An A-4E's LAU-61 pop-up at skill 2: the pull, the roll-in onto the column, rockets on the cue."),
        ("ai-strike-rockets-dive", rockets_dive, 600,
         "An A-4E's LAU-61 dive at skill 2 from 6,000 ft, rockets on the cue."),
        ("ai-strike-snakeye", snakeye, 600,
         "An A-4E's Mk 82 Snakeye laydown at skills 1 and 2: down to the low level height that clears the "
         "fuze and the fragments (under 1,000 ft), released level with a lead on the column."),
        ("ai-strike-heavy-level", heavy, 900,
         "A C-130 with a bay of Mk 82 on the factory: its release floor is the 10,000 ft Quick Mission "
         "altitude, not the 5,000 ft base."),
        ("ai-strike-offset", offsets, 900,
         "Level and dive Mk 82 passes starting 1,000 and 3,000 ft off the attack line: the sideways law lays "
         "the track on the aim and releases on the first pass."),
        ("ai-strike-lead-dive", lead_runs("dive"), 1200,
         "John's comparison: the same A-4E Mk 82 dive on the moving column and the standing factory at every "
         "skill; the moving target is led, the standing one not, both within the circle."),
        ("ai-strike-lead-level", lead_runs("level"), 1200,
         "The same comparison for the level run."),
    ]
    extra = []
    for name, entries, timeout, notes in maneuvers:
        first, steps, wants = run_list(entries)
        extra.append(Scenario(name=name, lane="flight", args=first, timeout=timeout, then=steps,
                              check=lambda out, wants=wants: strike_problems(out, wants), notes=notes))
    return extra + [
        Scenario(
            name="ai-strike-laser-self", lane="flight",
            args=laser_pod, timeout=600,
            then=[Step(laser_cone, timeout=600)],
            check=laser_self_problems,
            notes="An AI F-16C with a Pave Spike pod drops a GBU-10 on its own spot and holds the spot on the "
                  "tank through the 50 s fall in an offset turn (the target stays under the pod's wings, the "
                  "bomb never goes blind), then exits; an AI Su-25's built-in cone holds its nose on the tank "
                  "instead. Both destroy the tank.",
        ),
        Scenario(
            name="ai-strike-laser-buddy", lane="flight",
            args=ai_lases, timeout=600,
            then=[Step(ai_drops, timeout=600), Step(ai_waits, timeout=600)],
            check=laser_buddy_problems,
            notes="Buddy lasing both ways. An AI F-16C, given the wing order's call, lases for a player's F/A-18D "
                  "GBU-10 (the target page would name it Friendly 2-1). An AI F-14 with only a GBU-10 and no "
                  "designator chooses the bomb because a player's F-16C lases the tank, and drops it on that "
                  "spot; with nobody lasing it says \"No laze support, dropping manual\" and drops the bomb by "
                  "hand as an unguided bomb (slice ST-G).",
        ),
        Scenario(
            name="ai-strike-no-laze", lane="flight",
            args=pilot_run("UKR", "QUCOL", "--aircraft", "f14", *gbu10, "--altitude", "12000", "--speed", "450",
                           "--from", "6", "--seconds", "100", "--skill", "0", "--choose-store"),
            timeout=900,
            then=[
                Step(pilot_run("UKR", "QUCOL", "--aircraft", "f14", *gbu10, "--altitude", "12000", "--speed", "450",
                               "--from", "6", "--seconds", "100", "--skill", "1", "--choose-store"), timeout=600),
                Step(pilot_run("UKR", "QUCOL", "--aircraft", "f14", *gbu10, "--altitude", "12000", "--speed", "450",
                               "--from", "6", "--seconds", "100", "--skill", "2", "--choose-store"), timeout=600),
                Step(pilot_run("UKR", "QUCOL", "--aircraft", "f14", "--load", "AS14", "--station", "2",
                               "--count", "2", "--altitude", "12000", "--speed", "450", "--from", "6",
                               "--seconds", "60", "--skill", "2", "--choose-store"), timeout=300),
            ],
            check=no_laze_problems,
            notes="No laze support (slice ST-G, John 2026-10-12). An AI F-14 with only GBU-10s, no designator "
                  "and nobody lasing says \"No laze support, dropping manual\" and drops the laser bomb by hand "
                  "as an unguided bomb (CCIP/LVL on the ballistic solution, the unguided skill circle and "
                  "scatter, lead) at three skills (the Ace's 10 m circle is rarely met by the F-14's GBU-10 dive: open); with an AS-14 laser missile, which cannot fall unguided, it "
                  "says \"No laze support, RTB\" and goes home. The radio line and the replay's ai.strike "
                  "event mirror the NOLAZE line.",
        ),
        Scenario(
            name="ai-strike-driver", lane="flight",
            args=smoke + ["--skill", "2", "--profile", "dive"], timeout=300,
            then=[
                Step(smoke + ["--skill", "0", "--profile", "popup"], timeout=300),
                Step(smoke + ["--skill", "3"], timeout=300),
            ],
            check=lambda out: driver_problems(out, 3, ["dive", "popup", None]),
            notes="The AI flies an A-4E Mk 82 pass at QUCOL and the driver reports it, at three skills, "
                  "with and without a forced profile; passes before and after the strike behaviour lands.",
        ),
        Scenario(
            name="ai-strike-handback", lane="flight",
            args=pilot_run("UKR", "QUCOL", *hand, "--trigger"), timeout=300,
            then=[Step(pilot_run("UKR", "QUCOL", "--aircraft", "a4e", "--load", "MK82", "--handback", "6",
                                 "--mode", "lvl", "--altitude", "5000", "--seconds", "30"), timeout=300)],
            check=handback_problems,
            notes="A seat designates the column, selects the Mk 82 in LVL and holds the trigger (a bomb is "
                  "released and reported), then gives the aircraft to the AI; and a second run handed back "
                  "before the release. Both report the handoff and what the AI does after it.",
        ),
        Scenario(
            name="ai-strike-takeover", lane="flight",
            args=pilot_run("UKR", "QUCOL", "--aircraft", "f16c", "--load", "GBU10", "--handback", "3", "--mode",
                           "lvl", "--laser", "--take-back", "60", "--altitude", "8000", "--from", "6",
                           "--seconds", "120"),
            timeout=300,
            check=takeover_problems,
            notes="A seat flies an F-16C's GBU-10 pass at the column in LVL with the laser on by hand and gives the "
                  "aircraft back 3 s in; the AI flies the same level pass at the same target with the same store, "
                  "the laser holds the target until the bomb ends, and the seat takes the aircraft back to find "
                  "its cockpit as left, one bomb fewer (slice ST-K).",
        ),
        Scenario(
            name="ai-wing-a7-bomb-vehicles", lane="flight",
            args=pilot_run("KURILE", "QKPLNGR", "--aircraft", "a7", "--load", "MK82", "--station", "3",
                           "--choose-store", "--order", "engage-class", "--order-class", "vehicles",
                           "--order-at", "0", "--unit", "0x4000004f", "--altitude", "7800",
                           "--from", "1", "--skill", "3", "--sead-at", "0", "--seconds", "240"),
            timeout=600,
            expect=[r"Engage target class: 1 applied", r"PROFILE .*name=level", r"RELEASE .*weapon=MK82.JT",
                    r"SELECTION .*kind=Bomb side=2 class=0x0200", r"summary .*target_hp=0/50"],
            notes="The ordered A-7 with Mavericks replaced by bombs must release from a level pass, "
                  "using the unchanged shared bomb solution and Ace circle.",
        ),
        Scenario(
            name="ai-wing-a7-attack-vehicles", lane="flight",
            args=pilot_run("KURILE", "QKPLNGR", "--aircraft", "a7", "--load", "MK82",
                           "--choose-store", "--order", "engage-class", "--order-class", "vehicles",
                           "--order-wingmen", "2", "--order-at", "0", "--unit", "0x4000004f",
                           "--altitude", "10000", "--from", "1", "--skill", "3", "--sead-at", "0",
                           "--seconds", "240", "--record", "{work}/a7-attack-vehicles.tore-replay"),
            timeout=600,
            expect=[r"Engage target class: 2 applied", r"SELECTION .*kind=Seeker side=2 class=0x0200",
                    r"WINGMAN id=1 shots=[1-9]", r"WINGMAN id=2 shots=[1-9]",
                    r"WINGMAN id=[12] shots=[1-9][0-9]* hits=[1-9]", r"recorded .*a7-attack-vehicles"],
            notes="John's A-7E three-ship vehicle order near the Kuril airstrip. Starts near the order's "
                  "geometry at 10,000 ft; the two static ZSU sites are removed, as John had destroyed "
                  "them before his order. Default stores, real seat command, both wingmen release "
                  "and the hostile vehicle is hit. The full run is recorded without rendering.",
        ),
        Scenario(
            name="ai-strike-order", lane="flight",
            args=pilot_run("UKR", "QUCOL", "--aircraft", "a4e", "--load", "MK82", "--order", "engage-my-target",
                           "--order-at", "3", "--seconds", "30"),
            timeout=300,
            check=order_problems,
            notes="A seat leads a two-ship wing, designates the column and gives the wingman Engage my "
                  "target; the driver reports the order and the world's reply (taken since slice ST-O).",
        ),
        Scenario(
            name="ai-wing-ground-order", lane="flight",
            args=order_args["engage-my-target"], timeout=600,
            then=[Step(a, timeout=600) for k, a in order_args.items() if k != "engage-my-target"],
            check=lambda out: ground_order_problems(out, ORDER_RUNS),
            notes="A seat leads a two-ship wing and orders the wingman onto the ground: Alt+E and Alt+R "
                  "on the Ukraine armored column (an A-4E's Mk 82), Alt+F on the factory with the "
                  "lead's Pave Spike on it (an F-16C's JDAM), Alt+W on a SAM of the defended column (the "
                  "wingman takes the site's SAMs as a class order), and Alt+E refused by a wingman with only air-to-air "
                  "stores. Each reports the reply and what the wingman holds; the takers release.",
        ),
        Scenario(
            name="qmc-strike-a10-maverick-clear-air", lane="flight",
            args=pilot_run("VLA", "QVCMHQ", "--aircraft", "a10", "--load", "AGM65G",
                           "--quick-wing", "--choose-store", "--clear-air-at", "10", "--follow-egress", "--seconds", "480"),
            timeout=900,
            expect=[r"CLEAR_AIR .*count=1", r"RELEASE", r"IMPACT", r"EGRESS"],
            check=clear_air_strike_problems,
            notes="The A-10 chooses a Maverick for Vladivostok HQ and must finish the strike after the last bandit disappears.",
        ),
        Scenario(
            name="qmc-strike-a10-clear-air", lane="flight",
            args=pilot_run("VLA", "QVCMHQ", "--aircraft", "a10", "--load", "MK82",
                           "--quick-wing", "--clear-air-at", "10", "--follow-egress", "--seconds", "900"),
            timeout=900,
            expect=[r"CLEAR_AIR .*count=1", r"RELEASE", r"IMPACT", r"EGRESS"],
            check=lambda out: clear_air_strike_problems(out, kill=False),
            notes="An A-10 Quick Mission strike wing must attack Vladivostok HQ after the last bandit is removed at "
                  "ten seconds: the Mk 82 striker releases, lands within the miss limit of its target and egresses. "
                  "Its Maverick wingman can kill the striker's target first, and a gun hit can break a pass, so "
                  "the run has 900 s (2026-10-28: an energy-model change moved the first pass by 1 ft/s and "
                  "the 480 s run, which needed a lucky bunker kill, stopped passing).",
        ),
        Scenario(
            name="qmc-strike-wing", lane="flight",
            args=pilot_run("UKR", "QUFACT", "--aircraft", "a4e", "--load", "MK82", "--quick-wing",
                           "--seconds", "300"),
            timeout=900,
            then=[Step(pilot_run("UKR", "QUFACT", "--aircraft", "f16c", "--load", "GBU30", "--station", "2",
                                 "--quick-wing", "--seconds", "60"), timeout=300)],
            check=quick_wing_problems,
            notes="A Quick Mission with the Ukraine factory as the ground target and a second friendly wing "
                  "of two: A-4E bombers get the Strike role from the mission's default (retail's "
                  "bomber-class rule) and attack the column on their own; F-16C fighters keep CAP and "
                  "release nothing.",
        ),
        Scenario(
            name="ai-wing-lase", lane="flight",
            args=lase_args, timeout=300,
            check=lambda out: ground_order_problems(out, [dict(label="lase", kind="lase", flies=True)]),
            notes="The lead of two F-16Cs designates the factory and orders Lase my target: the wingman "
                  "has a designator, takes it and holds the lase order, enters the lase hold at once, "
                  "lases the factory and keeps the spot for the minute, orbiting inside the laser's "
                  "reach, and drops nothing (ST-INT wires the order to slice ST-L's duty).",
        ),
        Scenario(
            name="ai-strike-bombs", lane="flight",
            args=bomb_args[0], timeout=900,
            then=[Step(a, timeout=300) for a in bomb_args[1:]],
            check=lambda out: strike_problems(out, _runs(BOMB_RUNS, hit=False)),
            notes="An A-4E's Mk 82 on the Ukraine armored column: each pass (straight, pop-up, fast-high, "
                  "dive, level) forced once at skill 2 and skills 0, 1 and 3 on the dive. Each run names its "
                  "pass, releases with its drawn aim inside the release circle (or the ring R to 2R when the "
                  "draw says so), ends the bomb within the circle plus the gate plus 400 ft of scatter (the ring's edge plus "
                  "it for a ring draw), leaves and is not lost.",
        ),
        Scenario(
            name="ai-strike-rockets", lane="flight",
            args=forced(*rocket_runs[0], *rockets), timeout=600,
            then=[Step(forced(*r, *rockets), timeout=300) for r in rocket_runs[1:]],
            check=lambda out: strike_problems(out, _runs(rocket_runs, hit=False)),
            notes="An A-4E's LAU-61 pods on the column, straight runs at skills 1 and 2: a burst at the "
                  "record's cadence with the drawn aim inside the circle, each rocket within the circle "
                  "plus the gate plus 400 ft, and the striker leaves.",
        ),
        Scenario(
            name="ai-strike-maverick", lane="flight",
            args=pilot_run("UKR", "QUCOL", *maverick, "--skill", "2", "--profile", "fasthigh"),
            timeout=600,
            check=lambda out: strike_problems(out, _runs([(2, "fasthigh")], miss=False, hit=True)),
            notes="An A-10's Maverick on the Ukraine armored column, fast-high: fired on its lock and "
                  "the tank is hurt.",
        ),
        Scenario(
            name="ai-strike-jdam", lane="flight",
            args=pilot_run("UKR", "QUFACT", *jdam, "--skill", "2", "--profile", "level"),
            timeout=600,
            check=lambda out: strike_problems(out, _runs([(2, "level")], miss=False, hit=True)),
            notes="An F-16C's GBU-30 JDAM on the Ukraine factory, level: the stored point is the target "
                  "(no skill offset for a smart weapon) and the target is hurt. A static target: a JDAM "
                  "flies to the point stored at release, so a moving column would be missed.",
        ),
        Scenario(
            name="ai-sead-harm", lane="flight",
            args=pilot_run("VLA", "QVRDRI", "--surface-seed", "4", "--defenses", "3", "3", "--unit",
                           STRAIGHT_FLUSH, "--aircraft", "f4g", "--load", "AGM88", "--from", "12",
                           "--altitude", "15000", "--seconds", "120", "--skill", "2", "--profile", "fasthigh"),
            timeout=600,
            check=lambda out: strike_problems(out, _runs([(2, "fasthigh")], miss=False, hit=True)),
            notes="An F-4G's HARM on the heavily defended site's radar, fast-high: fired inside the AI's "
                  "100,000 ft HARM range while the radar emits, and the radar is hurt.",
        ),
        Scenario(
            name="ai-strike-heli", lane="flight",
            args=heli_runs[0][0], timeout=600,
            then=[Step(a, timeout=300) for a, _ in heli_runs[1:]],
            check=lambda out: strike_problems(out, [w for _, w in heli_runs]),
            notes="Helicopters strike the Ukraine armored column: a Mi-24's AT-2 from the pop-up, the "
                  "same target under light SAMs whose ring it starts in (it attacks from a stand-off "
                  "outside the ring and its AT-2 hurts the tank), an "
                  "AH-64's rocket burst from the pop-up at skills 2 and 3, the drawn aim inside the "
                  "circle and every rocket within the circle plus the gate plus 400 ft, followed out "
                  "(--follow-egress) until it egresses, turning away from the column. None is lost. "
                  "The stand-off attack is no longer forced on a quiet column: the AI chooses it under "
                  "SAMs itself (ai-strike-heli-defended).",
        ),
        Scenario(
            name="ai-strike-heli-defended", lane="flight",
            args=defended_runs[0][0], timeout=600,
            then=[Step(a, timeout=600) for a, _ in defended_runs[1:]],
            check=lambda out: heli_defense_problems(out, [w for _, w in defended_runs]),
            notes="A Mi-24 against defences (slices ST-H and H4, John's threat rule of 2026-10-12): "
                  "heavy AAA on the moving column (the move-ahead ambush of 2026-10-29: it plans on where "
                  "the tank will be, fires its AT-2 from outside every gun ring it knows, never comes "
                  "within a known gun's reach and hurts the tank), light AAA at the factory (the pop-up given up for a stand-off once the guns are known, "
                  "kill), a SAM that out-ranges the AT-2 and an "
                  "SA-14 team on the target (hidden: its launch is an ambush, it holds off, a move-ahead shot "
                  "is cut short by the seed's other hidden team's ambush, then it goes home), the SA-14 "
                  "with SEAD a minute in "
                  "(it resumes and fires), two ambushes on the factory by an SA-7 and an SA-14 team the "
                  "game hides until it is near or launches (a launch with known=no, then it breaks, "
                  "leaves the ring, holds off, resumes and kills the factory; the site is destroyed after "
                  "its first launch), and the stand-off layout of seed 1 (it holds "
                  "off). Each break is the retail plus or minus 90 degrees to the threat's bearing (or "
                  "the masking alternatives), kept at speed, within 10 degrees of level and clear of "
                  "the ground; the helicopter is never lost.",
        ),
        Scenario(
            name="qmc-strike-heli", lane="flight",
            args=pilot_run("UKR", "QUFACT", "--aircraft", "mi24", "--load", "AT2", "--quick-wing",
                           "--seconds", "300"),
            timeout=900,
            then=[Step(pilot_run("UKR", "QUFACT", "--aircraft", "ah64", "--load", "LAU61", "--quick-wing",
                                 "--seconds", "300"), timeout=900)],
            check=quick_heli_problems,
            notes="A Quick Mission with the Ukraine factory as the ground target and a second friendly "
                  "wing of two AI helicopters (an Mi-24 pair, then an AH-64 pair): the mission offers "
                  "them, gives them the Strike role by the bomber-class rule, starts them 1,500 ft over "
                  "the ground, and they fly the pop-up and release (AT-2, then a rocket burst). Defended "
                  "targets may bring them down, so a loss is allowed.",
        ),
        Scenario(
            name="ai-strike-ac130", lane="flight",
            args=gunship_args[0], timeout=600,
            then=[Step(a, timeout=300) for a in gunship_args[1:]],
            check=lambda out: gunship_problems(
                out, [dict(label=f"ac130 skill {k}", skill=k) for k in gunship_skills]),
            notes="An AC-130 circles the Ukraine armored column on its left-hand orbit from the guns' "
                  "reach and fires its linked guns in bursts (skills 0, 2 and 3): each burst's walked-in "
                  "aim inside the skill circle, the target inside the guns' 13,000 ft and the bank "
                  "under 30 degrees once settled, the tank killed, the gunship not lost.",
        ),
        Scenario(
            name="ai-strike-commit-aaa", lane="flight",
            args=commit_runs[0][0], timeout=600,
            then=[Step(a, timeout=300) for a, _ in commit_runs[1:]],
            check=lambda out: commit_problems(out, [w for _, w in commit_runs]),
            notes="The committed-to-release window (John, 2026-10-28): an A-4E's straight Mk 82 pass on "
                  "the column under its medium AAA (surface seed 1). The window opens 5 s out, a gun "
                  "reaches the striker about 1 s before the release, and the run holds: the bomb leaves "
                  "in the window and the striker defends on the next tick (skills 2, 0 and 3). The "
                  "control moves the column's nearest ZSU-23 14,000 ft out under the run line, so it "
                  "fires about 6 s before the release: the run breaks into Defend with no window open "
                  "and nothing released.",
        ),
        Scenario(
            name="ai-strike-defended", lane="flight",
            args=pilot_run("UKR", "QUCOL", "--aircraft", "f18", "--load", "MK82", "--defenses", "2", "2",
                           "--seconds", "180", "--skill", "2"),
            timeout=600,
            check=lambda out: strike_problems(out, _runs([(2, None)], hit=False, may_lose=True, defends=True)),
            notes="An F/A-18D's Mk 82 on the defended column, its pass chosen by threat (SAMs cover the "
                  "column: a pop-up): releases inside its circle and defends against the defences. The "
                  "column's own guns may still bring it down at the pop-up's low altitude, so a loss is "
                  "allowed; the run checks the release and the defence.",
        ),
    ]
