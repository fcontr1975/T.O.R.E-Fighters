"""Lane: AI fights, wings, damage and missiles (headless probes).

Every scenario is one `--ai-probe-ticks` run (or the roster and matrix probes).
`probe_problems` reads the probe's own lines and reports impossible states: the
probe's `AI probe anomaly:` lines (written by crates/tore-app/src/probe_invariants.rs
from every tick), actors whose final state disagrees with the events the probe
printed, debrief kill counts that disagree with the fight, dropped launches,
open ground hazards and radio lines that repeat. See docs/testing/lane-ai.md.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Callable, Optional

from battery import ROOT, Scenario, Step

from battery_scenarios import _ai_fuzz
from battery_scenarios._debrief import kill_hit_problems
from battery_scenarios._strips import ground_airport

AIRCRAFT = ["f18", "rafale", "f14", "a4e", "x31", "mig29", "su27", "mig21", "su25", "mig23", "su35", "f22", "f22n", "faxx"]
SKILLS = ["novice", "average", "experienced", "ace"]
MISSIONS = ["free", "cap", "intercept", "escort", "self-defense", "hold"]
GEOMETRIES = ["head", "side", "rear"]
ADAPTERS = ["legacy", "researched"]
# Base theaters (--validate-maps lists them); every one has airports 1 and 3.
THEATERS = ["APA", "BAL", "CUB", "EGY", "FRA", "GRE", "IRA", "KURILE", "LFA", "NSK", "PGU", "SPA", "TVIET", "UKR", "VLA", "WTA"]
# Known-good airport for ground starts in the default theater (Ukraine).
GROUND_AIRPORT = "2"

ORDER_NAMES = ["attack-on-contact", "engage-my-target", "land-selected", "bug-out"]
ACTOR = re.compile(r"^actor=(\d+) (\S+ \d-\d) (\S+) activity=(.*?) alive=(true|false) rounds=(\d+) ")
INVARIANTS = re.compile(r"^AI probe invariants: .*anomalies=(\d+)", re.M)
ANOMALY = re.compile(r"^AI probe anomaly: (.*)$", re.M)
TOTALS = re.compile(r"^AI probe totals: wings=(\d+) ticks=(\d+) shots=(\d+) dropped=(\d+) warnings=(\d+) live_projectiles=(\d+) player_hp=(-?\d+)", re.M)
DEBRIEF = re.compile(r"^AI probe debrief: (\S+) \[(.*?)\] elapsed=(\d+)s player\[(\S+) damage=(\d+)% kills=\[([\d, ]*)\] ff=(\d+)", re.M)
ATTACK = re.compile(r"^AI probe attack: clicks=.*? missiles=(\d+) .*?player_kills=(\d+) hits=(\d+) destroyed=(\d+) lost_friendly=(\d+)/(\d+) lost_enemy=(\d+)/(\d+) player_alive=(true|false) .*?ejections=(\d+) ", re.M)
RADIO_LINE = re.compile(r"^  (\d+\.\d)s (.+?): '(.*)' \[(.*)\]$", re.M)
# The hit calls of `crates/tore-world/src/radio_calls.rs` (HIT_BY_AIRCRAFT,
# HIT_BY_AAA, HIT_BY_OTHER). The radio chatter spec calls one on every missile
# hit (gun hits are limited in the game), so several hits within a few seconds
# may repeat the same words: fuzz seed 163 took three missile hits in 3 s and
# all three drew "Get this guy off me".
HIT_REACTION_CODES = frozenset(
    ["^IMHIT1", "^IMDMGE1", "^OFFME", "^SCORCH", "^HEAT", "^IMHIT2", "^IMDMGE2", "^IMAAA", "^EATLD"]
)
EVENT = re.compile(r"^t=(\d+) \(([\d.]+)s\) (?:destroyed: (\S+ \d-\d)|(\S+ \d-\d) pilot ejected)", re.M)
HAZARDS = re.compile(r"^AI probe liftoff gaps: \[(.*?)\] hazards_open=(\d+)", re.M)
PHASES = re.compile(r"^AI probe phases: (\S+ \d-\d): (.*)$", re.M)
# The AI probe's line for a wing whose human leader was lost (John's decision
# of 2026-09-30, docs/spec/ai.md "Mission of opportunity after a lost human leader").
OPPORTUNITY_HOME = re.compile(r"^t=\d+ \([\d.]+s\) mission of opportunity: (\S+ \d-\d) leads, returning to base \((.*)\)$", re.M)


# Anomaly kinds that are real but wait on a decision (docs/testing/lane-ai.md,
# "Found, not fixed" and "Needs a decision"): a fighter at the edge of a
# missile's employment zone alternates every few ticks between its maneuver and
# gun tracking, and AI aircraft collide in the air (leaders of different wings
# have no deconfliction; wingmen breaking out of formation can turn into each
# other). Scenarios pass `strict=True` to fail on them anyway.
KNOWN_ANOMALIES = ("activity flapping", "pitch stick oscillating", "mid-air collision")


def probe_problems(
    output: str,
    *,
    strict: bool = False,
    allow_anomalies: tuple[str, ...] = (),
    ground: bool = False,
    need_takeoff: bool = False,
    need_landing: bool = False,
    player_flies: bool = False,
) -> list[str]:
    """Impossible or inconsistent states in one AI probe's output. With
    `player_flies` the scripted player must end alive: only for probes with no
    enemy near, where its takeoff and climb-out are all that can bring it down."""
    problems: list[str] = []
    if player_flies and re.search(r"^player crashed=true", output, re.M):
        crash = PLAYER_DOWN.search(output)
        problems.append("the scripted player crashed" + (f" at tick {crash.group(1)}" if crash else ""))
    inv = INVARIANTS.search(output)
    if not inv:
        problems.append("no 'AI probe invariants' line (probe did not finish?)")
    allowed = allow_anomalies + (() if strict else KNOWN_ANOMALIES)
    for line in ANOMALY.findall(output):
        if not any(kind in line for kind in allowed):
            problems.append(f"anomaly: {line[:200]}")
    actors: dict[str, tuple[str, bool, int]] = {}
    for raw in output.splitlines():
        m = ACTOR.match(raw)
        if not m:
            continue
        _, label, _, activity, alive, rounds = m.groups()
        actors[label] = (activity, alive == "true", int(rounds))
        if alive == "true" and activity == "Destroyed":
            problems.append(f"{label} alive but its activity says Destroyed")
        if alive == "false" and activity != "Destroyed":
            problems.append(f"{label} dead but its activity says {activity}")
    problems.extend(ground_collisions(output, actors))
    for m in EVENT.finditer(output):
        label = m.group(3) or m.group(4)
        if label in actors and actors[label][1]:
            what = "destroyed" if m.group(3) else "ejected"
            problems.append(f"{label} {what} at {m.group(2)}s but alive at the end")
    for line in re.findall(r"^AI probe (?:UNDERGROUND|OFF-MAP) start: .*$", output, re.M):
        problems.append(line)
    totals = TOTALS.search(output)
    if totals and int(totals.group(4)) > 0:
        problems.append(f"dropped launches: {totals.group(4)}")
    problems.extend(kill_hit_problems(output))
    debrief = DEBRIEF.search(output)
    attack = ATTACK.search(output)
    if debrief and attack:
        kills = sum(int(k) for k in debrief.group(6).split(",") if k.strip())
        # The combat counter counts only destructions by the player's own
        # weapons; the debrief also credits aircraft the player damaged that
        # then went down (ejection, crash), so it may be larger, never smaller.
        if kills < int(attack.group(2)):
            problems.append(f"debrief player kills {kills} but combat counted {attack.group(2)}")
        lost_enemy, enemies = int(attack.group(7)), int(attack.group(8))
        objective = re.search(r"Destroy \{ destroyed: (\d+), total: (\d+) \}", debrief.group(2))
        # Only free engagement makes every enemy aircraft the objective.
        if objective and re.search(r"^AI probe: .* mission=free$", output, re.M) and not GROUP.search(output) and (int(objective.group(1)), int(objective.group(2))) != (lost_enemy, enemies):
            problems.append(
                f"debrief objective {objective.group(1)}/{objective.group(2)} but {lost_enemy}/{enemies} enemies lost"
            )
        player_alive = attack.group(9) == "true"
        if player_alive != (debrief.group(4) != "Dead") and debrief.group(4) not in ("Ejected", "Captured", "Rescued"):
            problems.append(f"debrief says player {debrief.group(4)} but probe says alive={player_alive}")
    hazards = HAZARDS.search(output)
    if ground and hazards and int(hazards.group(2)) > 0:
        problems.append(f"ground hazards still open at the end: {hazards.group(2)}")
    if ground:
        # A wingman still waiting to take off when its wing gave up its
        # mission of opportunity and went home stays parked (docs/spec/ai.md).
        stood_down = bool(OPPORTUNITY_HOME.search(output))
        for label, phases in PHASES.findall(output):
            names = [p.split("@")[0] for p in phases.split()]
            if stood_down and set(names) == {"Waiting"}:
                continue
            if need_takeoff and "ClimbOut" not in names and "Airborne" not in names:
                problems.append(f"{label} never took off: {phases[:160]}")
            if need_landing and label in actors and actors[label][1] and not any(n in names for n in ("Rollout", "Landed", "Parked", "TaxiIn")):
                problems.append(f"{label} never landed: {phases[:160]}")
    problems.extend(radio_problems(output))
    problems.extend(objective_problems(output))
    problems.extend(threshold_problems(output))
    return problems


# How long a theater's land-pair probe runs, where the default 900 s is too
# long. The scripted player cruises on along the runway heading while its
# wingman lands, and the probe needs no enemy near it (`player_flies`). From
# Kimpo (NSK 9) that heading runs north into North Korea: the theater
# layout's own Redfor SA-6 and ZSU-23 stand 71 nm up it and shot the player
# down at 631 s (first SA-6 launch 570 s at 8.6 nm, checked 2026-10-20). The
# wingman has rolled out by 485 s, so the probe ends at 540 s.
LAND_PAIR_TICKS = {"NSK": 64800}

THRESHOLD = re.compile(r"^t=\d+ \(([\d.]+)s\) THRESHOLD (\S+ \d-\d+) wheels=(-?\d+) ft", re.M)
# An AI final crossing the threshold this low is landing short, this high
# is landing long (the ILS path crosses it about 52 ft up).
# Longest time from the first gate to final before a wing counts as hanging
# at the gates (the 16 theaters' landing pairs take about 250 to 470 s).
GATES_MAX_S = 540
THRESHOLD_LOW_FT = 10
THRESHOLD_HIGH_FT = 300


def threshold_problems(output: str) -> list[str]:
    problems = []
    for seconds, label, wheels in THRESHOLD.findall(output):
        if not THRESHOLD_LOW_FT <= int(wheels) <= THRESHOLD_HIGH_FT:
            problems.append(f"{label} crossed the threshold with its wheels {wheels} ft up at {seconds}s")
    return problems


DEATH = re.compile(r"^t=\d+ \(([\d.]+)s\) (\S+ \d-\d) \S+: (?:phase=\S+ activity=\"[^\"]*\" )?alive=false agl=(-?\d+) kt=(\d+)", re.M)
DAMAGED = re.compile(r"^AI damage: actor=(\d+) ", re.M)


def ground_collisions(output: str, actors: dict) -> list[str]:
    """AI aircraft that flew into the ground undamaged: they died on the
    ground with no weapon kill, no ejection, no structural damage and no system fault."""
    destroyed = set(re.findall(r"destroyed: (\S+ \d-\d)", output))
    ejected = set(re.findall(r"(\S+ \d-\d) pilot ejected", output))
    damaged = set()
    for line in output.splitlines():
        match = DAMAGED.match(line)
        if match is None:
            continue
        fraction = re.search(r"\bdamage_fraction=([0-9.eE+-]+)", line)
        faults = re.search(r"\bfaults=\[([^\]]*)\]", line)
        if (fraction and float(fraction[1]) > 0) or (faults and faults[1].strip()):
            damaged.add(match[1])
    ids = {m.group(2): m.group(1) for m in (ACTOR.match(line) for line in output.splitlines()) if m}
    problems = []
    for m in DEATH.finditer(output):
        at, label, agl = m.group(1), m.group(2), int(m.group(3))
        if agl < 40 and label not in destroyed and label not in ejected and ids.get(label) not in damaged:
            problems.append(f"{label} flew into the ground undamaged at {at}s")
    return problems


GROUP = re.compile(r'^AI probe group: (\d) objective="(.*)" survive=(true|false)$', re.M)
OUTCOME = re.compile(r"^AI probe debrief: (SUCCESS|FAILURE) \[(.*?)\] elapsed=\S+ player\[(\S+) .*?ff=(\d+)", re.M)


def objective_problems(output: str) -> list[str]:
    """The debrief's objectives and outcome against the aircraft the probe
    reports alive, by the rules in docs/spec/debrief.md (Outcome and
    objectives). Checked only when `--probe-group` set the player's group."""
    groups = {int(g): (label, survive == "true") for g, label, survive in GROUP.findall(output)}
    debrief = OUTCOME.search(output)
    if 1 not in groups or not debrief:
        return []
    alive: dict[str, bool] = {}
    for line in output.splitlines():
        m = ACTOR.match(line)
        if m:
            alive[m.group(2)] = m.group(5) == "true"
    alive["Friendly 1-1"] = debrief.group(3) == "Alive"

    def members(side: str, wing: int) -> list[str]:
        return [label for label in alive if label.startswith(f"{side} {wing}-")]

    label = groups[1][0]
    target = re.match(r"Primary target: enemy group (\d)", label)
    targets = members("Enemy", int(target.group(1))) if target else [l for l in alive if l.startswith("Enemy")]
    protect = set()
    guard = re.match(r"Protect friendly group (\d)", label)
    if guard:
        protect.update(members("Friendly", int(guard.group(1))))
    for group, (_, survive) in groups.items():
        if survive and group <= 3:
            protect.update(members("Friendly", group))
    problems = []
    destroyed = sum(1 for t in targets if not alive[t])
    found = re.search(r"Destroy \{ destroyed: (\d+), total: (\d+) \}", debrief.group(2))
    if targets and (not found or (int(found.group(1)), int(found.group(2))) != (destroyed, len(targets))):
        problems.append(f"debrief targets {found.group(0) if found else 'missing'} but {destroyed} of {len(targets)} are down")
    kept = sum(1 for p in protect if alive[p])
    found = re.search(r"Protect \{ protected: (\d+), total: (\d+) \}", debrief.group(2))
    if protect and (not found or (int(found.group(1)), int(found.group(2))) != (kept, len(protect))):
        problems.append(f"debrief protected {found.group(0) if found else 'missing'} but {kept} of {len(protect)} survive")
    success = destroyed == len(targets) and kept == len(protect) and debrief.group(4) == "0"
    if (debrief.group(1) == "SUCCESS") != success:
        problems.append(f"debrief says {debrief.group(1)} but targets {destroyed}/{len(targets)}, protected {kept}/{len(protect)}")
    return problems


def radio_problems(output: str) -> list[str]:
    """Radio lines that flood: the same speaker and words three times within five
    seconds. Hit calls are exempt: the game calls one on every missile hit."""
    problems = []
    recent: dict[tuple[str, str], list[float]] = {}
    for at, speaker, words, codes in RADIO_LINE.findall(output):
        called = re.findall(r'"(\^[^"]+)"', codes)
        if called and all(code in HIT_REACTION_CODES for code in called):
            continue
        times = recent.setdefault((speaker, words), [])
        times.append(float(at))
        window = [t for t in times if float(at) - t <= 5.0]
        if len(window) >= 3:
            problems.append(f"radio repeats {speaker}: '{words}' {len(window)} times in 5 s")
            times.clear()
    return problems


def checker(**kw) -> Callable[[str], list[str]]:
    return lambda output: probe_problems(output, **kw)


DATALINK_LINE = re.compile(r"^t=(\d+) data link: (member|lock|unlock) plane=(\d+)(?: radar=(true|false)| target=(\d+))$", re.M)


ASSIGNMENT_LINE = re.compile(
    r"^t=(\d+) data link: (assign|clear|acknowledge) plane=(\d+) target=(\d+)(?: by=(\d+) order=(\w+)| why=([a-z ]+))?$", re.M
)


def datalink_assign_check(plane: int, target: int, lead: int, cleared_by_order_at: int) -> Callable[[str], list[str]]:
    """The assignment lines of the flight data link (slice G3a): the lead's
    Engage my target gives `plane` the target, the plane acknowledges it at most
    once and only after the assignment, and the later Attack on contact clears
    it for the reason `order`. Every assignment has its giver and order, and no
    clear or acknowledgement comes without an assignment."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        held: dict[int, int] = {}
        acknowledged: set[int] = set()
        assigned = cleared = False
        for tick, kind, who, aimed, by, order, why in ASSIGNMENT_LINE.findall(output):
            who, aimed = int(who), int(aimed)
            if kind == "assign":
                if who in held:
                    problems.append(f"t={tick}: plane {who} assigned over an assignment")
                held[who] = aimed
                acknowledged.discard(who)
                assigned |= (who, aimed, int(by), order) == (plane, target, lead, "EngageMyTarget")
            elif held.get(who) != aimed:
                problems.append(f"t={tick}: {kind} for plane {who} target {aimed} that it was not assigned")
            elif kind == "acknowledge":
                if who in acknowledged:
                    problems.append(f"t={tick}: plane {who} acknowledged twice")
                acknowledged.add(who)
            else:
                held.pop(who)
                cleared |= who == plane and why == "order" and int(tick) >= cleared_by_order_at
        if not assigned:
            problems.append(f"no assignment of target {target} to plane {plane} by plane {lead}")
        if not cleared:
            problems.append(f"plane {plane}'s assignment was not cleared by the order at tick {cleared_by_order_at}")
        return problems

    return check


def datalink_order_link_check(wingman: int, target: int, lead: int) -> Callable[[str], list[str]]:
    """An assignment reaches an AI wingman through the picture (slice G3b): the
    wingman has no sensors of its own (`--probe-blind-wing`), the player's lock
    puts the bandit in the flight's picture, and the lead's Engage my target is
    taken (the assignment line) where it used to answer "cannot see the target".
    The wingman then flies after the track, acquiring, and, never holding the
    bandit itself, never locks it and never fires."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        assigned = [
            (int(who), int(aimed), int(by))
            for _, kind, who, aimed, by, order, _ in ASSIGNMENT_LINE.findall(output)
            if kind == "assign" and order == "EngageMyTarget"
        ]
        if (wingman, target, lead) not in assigned:
            problems.append(f"no assignment of target {target} to blind plane {wingman} by plane {lead}")
        if re.search(r"order=\S+ reply=.*[1-9]\d* rejected", output):
            problems.append("the order was rejected: " + (re.search(r"order=\S+ reply=.*", output).group(0)[:120]))
        locks = [t for t, kind, plane, _, _ in DATALINK_LINE.findall(output) if kind == "lock" and int(plane) == wingman]
        if locks:
            problems.append(f"the blind wingman {wingman} locked something at t={locks[0]}")
        shots = re.search(r"AI probe totals: .*?shots=(\d+)", output)
        if not shots or shots.group(1) != "0":
            problems.append(f"shots fired: {shots.group(1) if shots else 'unknown'}")
        activity = None
        for raw in output.splitlines():
            m = ACTOR.match(raw)
            if m and m.group(1) == str(wingman):
                activity = m.group(4)
        if activity not in ("Acquiring", "Pursuing"):
            problems.append(f"blind wingman {wingman} ends '{activity}', not on the track")
        return problems

    return check


def order_sort_check(wingmen: list[int], enemies: list[int], lead: int) -> Callable[[str], list[str]]:
    """The lead's Sort order (slice G3c, Alt+A): every wingman is assigned an
    enemy aircraft by the lead with the order Sort, the first wingmen each take
    a different one while there are enough, no enemy takes more than two, and
    the pilot's line says how many were assigned."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        given: dict[int, int] = {}
        for _, kind, who, aimed, by, order, _ in ASSIGNMENT_LINE.findall(output):
            if kind != "assign":
                continue
            if order != "Sort" or int(by) != lead:
                problems.append(f"plane {who} was assigned by plane {by} with order {order}, not by {lead} with Sort")
            given[int(who)] = int(aimed)
        if sorted(given) != sorted(wingmen):
            problems.append(f"wingmen assigned: {sorted(given)}, expected {sorted(wingmen)}")
        for who, aimed in given.items():
            if aimed not in enemies:
                problems.append(f"plane {who} was given {aimed}, which is not one of the enemy aircraft {enemies}")
        counts = [list(given.values()).count(enemy) for enemy in enemies]
        if max(counts, default=0) > 2:
            problems.append(f"more than two wingmen on one bandit: {dict(zip(enemies, counts))}")
        firsts = [given.get(who) for who in sorted(wingmen)[: len(enemies)]]
        if len(set(firsts)) != len(firsts):
            problems.append(f"the first {len(enemies)} wingmen did not each take a different bandit: {firsts}")
        if not re.search(rf'order=Sort reply="Sort: {len(wingmen)} assigned', output):
            problems.append("the pilot was not told how many wingmen were assigned")
        return problems

    return check


def datalink_lead_sort_check(wing_size: int, per_side: int, player: int = 0) -> Callable[[str], list[str]]:
    """An AI lead's share and sort (stage G4), in a fight of `per_side` aircraft
    a side flown in wings of `wing_size` numbered in sequence, the friendly side
    first (the scripted player is plane `player`, which leads the first wing, so
    it is never assigned anything). Every assignment is from a lead to a member
    of its own wing, with the order EngageMyTarget (the share) or Sort, and
    never to the player; its target is on the other side; the share gives
    at most one wingman a target in a tick (the lead and one wingman make the
    two-attacker allowance); a sort puts at most two wingmen on one bandit; and
    one lead sorts at most once in 30 seconds (3,600 ticks). At least one sort
    must have dealt two or more wingmen a bandit each."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        batches: dict[tuple[int, int, str], list[tuple[int, int]]] = {}
        for tick, kind, who, aimed, by, order, _ in ASSIGNMENT_LINE.findall(output):
            if kind != "assign" or int(by) == player:
                continue
            tick, who, aimed, by = int(tick), int(who), int(aimed), int(by)
            if order not in ("Sort", "EngageMyTarget"):
                problems.append(f"t={tick}: plane {who} was given {aimed} by AI lead {by} with order {order}")
            if who == player or who // wing_size != by // wing_size or who == by:
                problems.append(f"t={tick}: plane {by} gave {aimed} to plane {who}, which is not one of its wingmen")
            if aimed // per_side == by // per_side:
                problems.append(f"t={tick}: plane {by} gave plane {who} the friendly aircraft {aimed}")
            batches.setdefault((tick, by, order), []).append((who, aimed))
        for plane in re.findall(r"^t=\d+ data link: assign plane=(\d+) target=\d+ by=(\d+) order=\w+$", output, re.M):
            if int(plane[0]) == player:
                problems.append(f"the scripted player was assigned a target by plane {plane[1]}")
        sorts: dict[int, list[int]] = {}
        dealt = False
        for (tick, by, order), given in sorted(batches.items()):
            if order == "EngageMyTarget":
                if len(given) > 1:
                    problems.append(f"t={tick}: lead {by} shared its target with {len(given)} wingmen: the allowance is two attackers")
                continue
            sorts.setdefault(by, []).append(tick)
            dealt |= len(given) >= 2
            counts = [aimed for _, aimed in given]
            for aimed in set(counts):
                if counts.count(aimed) > 2:
                    problems.append(f"t={tick}: lead {by} put {counts.count(aimed)} wingmen on bandit {aimed}")
        for by, ticks in sorts.items():
            for first, second in zip(ticks, ticks[1:]):
                if second - first < 3600:
                    problems.append(f"lead {by} sorted at t={first} and again at t={second}, inside 30 seconds")
        if not dealt:
            problems.append("no AI lead sorted two or more wingmen onto bandits")
        return problems

    return check


def datalink_picture_check(planes: int, designated: int) -> Callable[[str], list[str]]:
    """The flight data link's picture, as the probe prints it (`data link:`
    lines, from `--probe-player-lock` and `--probe-data-link`): every plane is
    announced once with its radar flag, locks and unlocks pair up per plane, the
    player's designation becomes a lock on the aircraft it named, and the
    probe's own checks still pass."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        radars: dict[int, str] = {}
        held: dict[int, int] = {}
        player_locked = False
        for tick, kind, plane, radar, target in DATALINK_LINE.findall(output):
            plane = int(plane)
            if kind == "member":
                if plane in radars:
                    problems.append(f"plane {plane} announced twice")
                radars[plane] = radar
            elif kind == "lock":
                if plane in held:
                    problems.append(f"t={tick}: plane {plane} locked over a lock")
                held[plane] = int(target)
                player_locked |= plane == 0 and int(target) == designated
            elif held.pop(plane, None) != int(target):
                problems.append(f"t={tick}: plane {plane} dropped a lock it did not hold")
        if sorted(radars) != list(range(planes)):
            problems.append(f"members announced: {sorted(radars)}, expected planes 0..{planes - 1}")
        # Every aircraft ported so far has a radar.
        for plane, radar in sorted(radars.items()):
            if radar != "true":
                problems.append(f"plane {plane} reports no radar")
        if not player_locked:
            problems.append(f"the player never locked aircraft {designated}")
        return problems

    return check


def datalink_player_lock_check(wingman: int, locked: int, takes: int) -> Callable[[str], list[str]]:
    """The AI counts a human's locked target (slice G2): the player locks
    aircraft `locked`, and its wingman `wingman`, ordered to attack later, ranks
    that bandit with the wing-attacker penalty and takes the other one,
    `takes`. The wingman's first lock must be on `takes` and it must never lock
    `locked`. The two scenarios mirror each other, so the choice follows the
    player's lock and not the geometry."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        wingman_locks = [
            (int(tick), int(target))
            for tick, kind, plane, _, target in DATALINK_LINE.findall(output)
            if kind == "lock" and int(plane) == wingman
        ]
        player = [int(target) for _, kind, plane, _, target in DATALINK_LINE.findall(output) if kind == "lock" and int(plane) == 0]
        if locked not in player:
            problems.append(f"the player never locked aircraft {locked}")
        if not wingman_locks:
            problems.append(f"wingman plane {wingman} never locked anything")
        elif wingman_locks[0][1] != takes:
            problems.append(f"wingman plane {wingman} first locked {wingman_locks[0][1]} at t={wingman_locks[0][0]}, expected {takes}")
        if any(target == locked for _, target in wingman_locks):
            problems.append(f"wingman plane {wingman} locked the player's bandit {locked}")
        return problems

    return check


LAND_ORDER = re.compile(r'^t=(\d+) order=LandAtSelected reply="(.*)"$', re.M)
PLAYER_DOWN = re.compile(r"^t=(\d+) \([\d.]+s\) player: .*crashed=true", re.M)
LANDED_PHASES = ("Rollout", "Landed", "Parked", "TaxiIn")


def land_order_checker(whole: Callable[[str], list[str]], *, player_lost: bool = False) -> Callable[[str], list[str]]:
    """A probe that orders the wing to land. The scripted player is the wing's
    leader and must still be flying when it gives the order (the probe's pilot
    survives its takeoff and climb-out, docs/testing/lane-ai.md), so the order
    must be accepted and `whole` (the landing checks) must pass. A player who
    crashed first would hand the lead to the wing's next member and refuse the
    order ("you are not leading your wing"): that is reported, never excused.

    With `player_lost` the probe loses the player on purpose (`--probe-lose-player`)
    and lead succession applies (approved by John 2026-09-28): the order is
    refused, the new lead flies a mission of opportunity (John, 2026-09-30),
    and as these probes have no enemy it must return to base at once and land
    there, and `whole` must pass as well."""

    def check(output: str) -> list[str]:
        order = LAND_ORDER.search(output)
        down = PLAYER_DOWN.search(output)
        gone = bool(order and down and int(down.group(1)) <= int(order.group(1)))
        refused = bool(order and "not leading your wing" in order.group(2))
        problems: list[str] = []
        if not player_lost:
            if down:
                problems.append(f"the scripted player crashed at tick {down.group(1)}, so the wing was not led by a living player")
            if not order:
                problems.append("the land order was never given")
            elif refused:
                problems.append("the land order was refused although the player led the wing")
            problems.extend(whole(output))
            return problems
        if gone and not refused:
            problems.append("a player who had already crashed still gave the land order and it was accepted")
        if refused and not gone:
            problems.append("the land order was refused although the player still led the wing")
        if not gone:
            problems.append("the probe was to lose the player before the land order and did not")
        problems.extend(whole(output))
        if gone:
            home = OPPORTUNITY_HOME.search(output)
            if not home:
                problems.append("the AI that took the lead from the lost player never returned to base")
            elif home.group(2) != "no enemy position known":
                problems.append(f"the new lead returned to base for the wrong reason: {home.group(2)}")
            else:
                phases = dict(PHASES.findall(output)).get(home.group(1), "")
                if not any(f"{name}@" in phases for name in LANDED_PHASES):
                    problems.append(f"the new lead {home.group(1)} returned to base but never landed: {phases[:160]}")
        return problems

    return check


OPPORTUNITY = re.compile(r"^t=\d+ \([\d.]+s\) mission of opportunity: (\S+ \d-\d) leads, (.*)$", re.M)


def lost_lead_route_checker(output: str) -> list[str]:
    """The player is lost with no enemy known: the new lead flies both of the
    wing's waypoints in order, then returns to base and lands."""
    problems = probe_problems(output, ground=True, need_takeoff=True)
    notes = [doing for _, doing in OPPORTUNITY.findall(output)]
    wanted = [
        "flying its waypoints, 2 left (no enemy position known)",
        "flying its waypoints, 1 left (no enemy position known)",
        "returning to base (waypoints flown)",
    ]
    if notes != wanted:
        problems.append(f"mission of opportunity went {notes}, expected {wanted}")
    home = OPPORTUNITY_HOME.search(output)
    phases = dict(PHASES.findall(output)).get(home.group(1), "") if home else ""
    if not any(f"{name}@" in phases for name in LANDED_PHASES):
        problems.append(f"the new lead never landed: {phases[:160]}")
    return problems


def rerun_same(args: list[str]) -> Callable[[str], list[str]]:
    """Determinism: run the same probe again on a fresh profile copy and compare."""

    def strip(text: str) -> list[str]:
        # Log lines carry wall-clock stamps; everything else must match.
        return [line for line in text.splitlines() if not re.match(r"^\d+\.\d+Z ", line)]

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        profile = Path(os.environ.get("TORE_BATTERY_PROFILE", ROOT / ".local" / "bugbash-data"))
        binary = os.environ.get("TORE_BATTERY_BIN", str(ROOT / "target" / "debug" / "tore-app"))
        with tempfile.TemporaryDirectory(prefix="bb-ai-det-") as tmp:
            data = Path(tmp) / "data"
            if subprocess.run(["cp", "-a", "--reflink=auto", str(profile), str(data)]).returncode != 0:
                shutil.copytree(profile, data)
            env = dict(os.environ, TORE_DATA_DIR=str(data), TORE_NO_ERROR_DIALOG="1")
            again = subprocess.run([binary, *args], env=env, capture_output=True, text=True, errors="replace", timeout=900)
        a, b = strip(output), strip(again.stdout + again.stderr)
        if a != b:
            first = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
            left = a[first] if first < len(a) else "<end>"
            right = b[first] if first < len(b) else "<end>"
            problems.append(f"not deterministic at line {first}: {left[:120]!r} vs {right[:120]!r}")
        return problems

    return check


def recordings_match(args: list[str]) -> Callable[[str], list[str]]:
    """Determinism of recordings: record the probe twice on fresh profile
    copies and require `--recording-diff` to say they match."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        profile = Path(os.environ.get("TORE_BATTERY_PROFILE", ROOT / ".local" / "bugbash-data"))
        binary = os.environ.get("TORE_BATTERY_BIN", str(ROOT / "target" / "debug" / "tore-app"))
        with tempfile.TemporaryDirectory(prefix="bb-ai-rec-") as tmp:
            paths = []
            for n in range(2):
                data = Path(tmp) / f"data{n}"
                if subprocess.run(["cp", "-a", "--reflink=auto", str(profile), str(data)]).returncode != 0:
                    shutil.copytree(profile, data)
                path = Path(tmp) / f"run{n}.tore-replay"
                env = dict(os.environ, TORE_DATA_DIR=str(data), TORE_NO_ERROR_DIALOG="1")
                subprocess.run([binary, *args, "--record-mission", str(path)], env=env, capture_output=True, timeout=900)
                paths.append(str(path))
            diff = subprocess.run([binary, "--recording-diff", *paths], capture_output=True, text=True, errors="replace", timeout=300)
        if "Verdict: the recordings match" not in diff.stdout:
            tail = [line for line in diff.stdout.splitlines() if line.strip()][-3:]
            problems.append(f"two recordings of the same probe differ: {' / '.join(tail)[:200]}")
        return problems

    return check


TOUCHDOWN = re.compile(
    r"^t=(\d+) \(([\d.]+)s\) TOUCHDOWN (\S+ \d-\d) sink_fps=([\d.-]+) ground_kt=([\d.-]+) phase=(\w+) "
    r"lift=(\w+) nacelle=([\d.-]+) gear_damaged=(true|false) crashed=(true|false)$",
    re.M,
)
VERTICAL_LANDING = re.compile(
    r"^t=(\d+) \(([\d.]+)s\) VERTICAL LANDING (\S+ \d-\d) lift=(\w+) phase=(\w+) kt=(\d+) nacelle=([\d.-]+)",
    re.M,
)
# The shared touchdown rule's clean limits (crates/tore-sim/src/touchdown.rs): sink, and the ground
# speed of a helicopter or the V-22 and of a hovering jet.
TOUCHDOWN_SINK_FPS = 10.0
TOUCHDOWN_KT = {"Rotor": 20.0, "Jet": 10.0}
ROTOR_CYCLE = ["VerticalHold", "VerticalClimb", "Airborne", "HoverApproach", "VerticalDescent", "Settle", "AirTaxi",
               "Parked"]
ROTOR_LANDING = ["HoverApproach", "VerticalDescent", "Settle", "AirTaxi", "Parked"]


def vertical_problems(output: str, runs_wanted: list[dict]) -> list[str]:
    """The vertical takeoff and landing scenarios (slice H2): each probe run (split at its `AI probe:`
    line) passes the probe's own checks, and each wingman named in `members` went through the wanted
    airfield phases in order (`phases`), never through the `forbid` ones, and touched down at most
    `touchdowns` times (or exactly, with `touchdowns_exact`) within the shared touchdown rule's clean
    limits for its lift (a jet's runway landing is judged by its runway rule), without gear damage.
    `converted` asks that each landing began with the V-22's nacelles at or under that angle (in
    aeroplane mode) and ended at 75 or more."""
    parts = re.split(r"(?m)^(?=AI probe: aircraft=)", output)
    runs = [part for part in parts if part.startswith("AI probe: aircraft=")]
    if len(runs) != len(runs_wanted):
        return [f"{len(runs)} probe runs finished, want {len(runs_wanted)}"]
    problems: list[str] = []
    for run, want in zip(runs, runs_wanted):
        label = want["label"]
        problems += [f"{label}: {p}" for p in probe_problems(run, ground=want.get("ground", True),
                                                               need_takeoff=want.get("takeoff", True))]
        phases = {member: [p.split("@")[0] for p in line.split()] for member, line in PHASES.findall(run)}
        for member in want["members"]:
            names = phases.get(member)
            if names is None:
                problems.append(f"{label}: no phases for {member}")
                continue
            rest = iter(names)
            if not all(any(w == n for n in rest) for w in want.get("phases", [])):
                problems.append(f"{label}: {member} flew {names}, want {want['phases']} in that order")
            for bad in want.get("forbid", []):
                if bad in names:
                    problems.append(f"{label}: {member} went through {bad}: {names}")
            downs = [m for m in TOUCHDOWN.finditer(run) if m.group(3) == member]
            need = want.get("touchdowns", 0)
            if want.get("touchdowns_exact") and len(downs) != need:
                problems.append(f"{label}: {member} touched down {len(downs)} times, want {need}")
            elif len(downs) < need:
                problems.append(f"{label}: {member} touched down {len(downs)} times, want at least {need}")
            for m in downs:
                at = f"{label}: {member} touchdown at t={m.group(2)}s"
                sink, knots, lift = float(m.group(4)), float(m.group(5)), m.group(7)
                if m.group(10) == "true" or m.group(9) == "true":
                    problems.append(f"{at} crashed={m.group(10)} gear_damaged={m.group(9)}")
                if lift in TOUCHDOWN_KT and not (sink <= TOUCHDOWN_SINK_FPS and knots <= TOUCHDOWN_KT[lift]):
                    problems.append(f"{at}: {sink} ft/s at {knots} kt, over the touchdown rule's {lift} limits")
                if "converted" in want and float(m.group(8)) < 75.0:
                    problems.append(f"{at} with the nacelles at {m.group(8)} degrees, not in helicopter mode")
            if "converted" in want:
                starts = [m for m in VERTICAL_LANDING.finditer(run) if m.group(3) == member]
                if not starts or float(starts[0].group(7)) > want["converted"]:
                    got = starts[0].group(7) if starts else "none"
                    problems.append(f"{label}: {member} began its landing with the nacelles at {got}, want "
                                    f"{want['converted']} or less (aeroplane mode)")
    return problems


def vertical_scenario(name: str, runs: list[tuple[list[str], int, dict]], notes: str) -> Scenario:
    """Probe runs (args, ticks, wanted) checked by [`vertical_problems`], the first as the scenario's
    command and the rest as its steps."""
    def command(args: list[str], ticks: int) -> list[str]:
        return ["--ai-probe-ticks", str(ticks), *args, "--no-audio"]

    (first, ticks, _), rest = runs[0], runs[1:]
    return Scenario(
        name=name,
        lane="ai",
        args=command(first, ticks),
        timeout=2400,
        then=[Step(command(args, t), timeout=2400) for args, t, _ in rest],
        expect=[r"AI probe totals:"],
        check=lambda output: vertical_problems(output, [want for _, _, want in runs]),
        notes=notes,
    )


def v22_problems(output: str, *, ground: bool) -> list[str]:
    """`ai-v22-quick-mission` (slice HF1): one probe run passes the probe's own checks, nobody fired
    (the V-22s are in free flight, not a fight) and every AI V-22 is alive at the end."""
    problems = probe_problems(output, ground=ground, need_takeoff=ground)
    totals = TOTALS.search(output)
    if totals and int(totals.group(3)) > 0:
        problems.append(f"{totals.group(3)} shots fired: the run is meant to be free flight")
    v22s = [m for m in (ACTOR.match(line) for line in output.splitlines()) if m and m.group(3) == "V22"]
    if not v22s:
        problems.append("no AI V-22 in the run")
    for m in v22s:
        if m.group(5) != "true":
            problems.append(f"{m.group(2)} V-22 lost ({m.group(4)})")
    return problems


def v22_quick_mission() -> Scenario:
    """Slice HF1: AI V-22s in a Quick Mission from the airborne start (the player's F/A-18D wing on the
    ground at Odesa, a friendly wing 2 and the enemy wing of V-22s in the air, the enemy starting 50 nm
    north toward rising ground) and from the ground (the player's V-22 wing of three at Odesa, a
    friendly wing 2 of V-22s, the enemy V-22s parked at Kharkiv): 300 s of flight (400 s from the
    ground) and no V-22 lost."""
    common = ["--ground-start", "9", "--probe-friendly-aircraft", "v22", "--probe-enemy-aircraft", "v22",
              "--separation", "50", "--no-audio"]
    airborne = ["--ai-probe-ticks", "36000", "--aircraft", "f18", *common]
    ground = ["--ai-probe-ticks", "48000", "--aircraft", "v22", "--maneuver", "takeoff", "--probe-wing-size",
              "3", "--enemy-ground-start", "6", *common]

    def check(output: str) -> list[str]:
        parts = re.split(r"(?m)^(?=AI probe: aircraft=)", output)
        runs = [part for part in parts if part.startswith("AI probe: aircraft=")]
        if len(runs) != 2:
            return [f"{len(runs)} probe runs finished, want 2"]
        return ([f"airborne: {p}" for p in v22_problems(runs[0], ground=False)]
                + [f"ground: {p}" for p in v22_problems(runs[1], ground=True)])

    return Scenario(
        name="ai-v22-quick-mission",
        lane="ai",
        args=airborne,
        timeout=1800,
        then=[Step(ground, timeout=2400)],
        expect=[r"AI probe totals:"],
        check=check,
        notes="AI V-22s in every wing of a Quick Mission fly 300 s from an airborne start (the "
              "enemy pair 50 nm north of Odesa heads into rising ground, where it used to fly into a "
              "hill 22 s in) and 400 s from a ground start (the player's V-22 wing at Odesa and the "
              "enemy pair at Kharkiv take off vertically; the enemy used to hit the ground about 100 s "
              "in): no V-22 lost, no shot fired.",
    )


def vertical_scenarios() -> list[Scenario]:
    """Slice H2: vertical takeoffs and landings (docs/spec/ai-airfield.md, "Vertical takeoff and
    landing")."""
    wing = ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", "3", "--maneuver", "takeoff", "--separation",
            "200", "--probe-wing-only"]
    members = ["Friendly 1-2", "Friendly 1-3"]
    out = []
    # A ground-started wing of three of each helicopter type: the player lifts off on the probe's
    # rotor script, the two AI wingmen lift off one by one where they stand (no taxi), fly out, are
    # ordered to land at the departure field, come in, land vertically, sit 5 s, air taxi to a slot,
    # set down and shut down.
    out.append(vertical_scenario(
        "ai-heli-ground-start-wing",
        [([*wing, "--aircraft", kind, "--probe-wing-order", "6000:land-selected"], 108000,
          dict(label=kind, members=members, phases=ROTOR_CYCLE, forbid=["Taxi", "TakeoffRoll"], touchdowns=2,
               touchdowns_exact=True))
         for kind in ("ah64", "mi24", "ch47", "v22")],
        notes="Each helicopter type (AH-64, Mi-24, CH-47, V-22) as a ground-started wing of three at "
              "Simferopol: vertical takeoffs one by one from where they stand, a land order at 50 s, the "
              "helicopter approach and vertical landing at the landing point, the air taxi to a parking "
              "slot; every touchdown within the shared touchdown rule.",
    ))
    # Home: an airborne helicopter wing bugs out and goes home to land (the ST-H stopgap made "home"
    # a heading and a hold).
    out.append(vertical_scenario(
        "ai-heli-land-home",
        [(["--aircraft", kind, "--probe-wing-size", "3", "--probe-wing-only", "--separation", "200",
           "--probe-wing-order", "600:bug-out"], 108000,
          dict(label=kind, members=members, phases=ROTOR_LANDING, ground=False, takeoff=False, touchdowns=2,
               touchdowns_exact=True))
         for kind in ("mi24", "ah64")],
        notes="An airborne Mi-24 wing, then an AH-64 wing, bugs out and goes home: each wingman flies "
              "the vertical landing at its home field and parks; the second comes in once the first is "
              "down.",
    ))
    # The V-22 converts from aeroplane mode for the landing.
    out.append(vertical_scenario(
        "ai-v22-conversion-land",
        [(["--aircraft", "v22", "--probe-wing-size", "2", "--probe-wing-only", "--separation", "200",
           "--probe-wing-order", "600:bug-out"], 90000,
          dict(label="v22", members=["Friendly 1-2"], phases=ROTOR_LANDING, ground=False, takeoff=False,
               touchdowns=2, touchdowns_exact=True, converted=30.0))],
        notes="An airborne V-22 wingman cruising in aeroplane mode (nacelles down) bugs out 5 s in: it "
              "converts to helicopter mode on the way, lands vertically with the nacelles at 75 degrees "
              "or more and parks.",
    ))
    # The V-22 in free flight (slice HF1): wingborne, its climb was held to the collective's 50 ft/s,
    # a 7 degree path at 240 kt, and it flew into rising ground (22 s from the airborne enemy start
    # north of Odesa, about 100 s after the enemy ground start at Kharkiv). V-22s in every wing of a
    # Quick Mission, airborne and from the ground, fly 300 and 400 s without crashing.
    out.append(v22_quick_mission())
    # The AV-8 and Yak-141: light enough to hover (guns only), the AV-8 taxis to the spot and takes off
    # vertically, comes home and lands vertically; the Yak-141 on full fuel needs 99% thrust to hover,
    # so it makes the short takeoff and lands on the runway; the AV-8 with its stores makes the short
    # takeoff too. The 2026-10-08 finding (a second wingman that never left the taxiway) is the
    # take-off part of every run.
    out.append(vertical_scenario(
        "ai-stovl-ground-start-wing",
        [
            ([*wing, "--aircraft", "av8", "--probe-ai-guns-only", "--probe-wing-order", "18000:land-selected"],
             126000,
             dict(label="av8 light", members=members,
                  phases=["Taxi", "LineUp", "VerticalHold", "VerticalClimb", "Transition", "Airborne",
                          "HoverApproach", "VerticalDescent", "Settle", "TaxiClear"],
                  forbid=["TakeoffRoll", "Final"], touchdowns=1)),
            ([*wing, "--aircraft", "yak141", "--probe-ai-guns-only", "--probe-wing-order", "18000:land-selected"],
             126000,
             dict(label="yak141 full fuel", members=members,
                  phases=["Taxi", "LineUp", "TakeoffRoll", "ClimbOut", "Airborne", "Approach", "Final", "Rollout"],
                  forbid=["VerticalHold", "HoverApproach"], touchdowns=1)),
            ([*wing, "--aircraft", "av8"], 24000,
             dict(label="av8 with stores", members=members,
                  phases=["Taxi", "LineUp", "TakeoffRoll", "ClimbOut", "Airborne"], forbid=["VerticalHold"])),
        ],
        notes="AV-8 and Yak-141 ground-started wings of three at Simferopol: a light AV-8 takes off and "
              "lands vertically, a full Yak-141 and a loaded AV-8 make the retail short takeoff (nozzles "
              "60, 40, 20) and the Yak-141 lands on the runway; every wingman leaves the taxiway.",
    ))
    return out


def probe(name: str, args: list[str], *, ticks: int = 7200, timeout: float = 900, check=None, expect=None, outputs=None, notes: str = "") -> Scenario:
    return Scenario(
        name=f"ai-{name}",
        lane="ai",
        args=["--ai-probe-ticks", str(ticks), *args, "--no-audio"],
        timeout=timeout,
        expect=[r"AI probe totals:", *(expect or [])],
        check=check or checker(),
        outputs=outputs or [],
        notes=notes,
    )


def fight(f: int, e: int, *extra: str) -> list[str]:
    return ["--probe-fight", f"{f}:{e}", *extra]


# One AI aircraft's fuel judgement (`--probe-fuel-trace`, slice H3).
FUEL_LINE = re.compile(
    r"^AI fuel: tick=(\d+) actor=(\d+) aircraft=(\w+) fuel_lb=(\d+) endurance_s=(\d+) time_home_s=(\S+) "
    r"state=(?:Some\()?(\w+)\)? activity=(\w+)$",
    re.M,
)


def fuel_lines(output: str, aircraft: str) -> list[tuple[int, int, int, int, str, str, str]]:
    """(tick, actor, fuel lb, endurance s, time home s or 'none', state, activity) of `aircraft`."""
    return [
        (int(t), int(a), int(f), int(e), home, state, activity)
        for t, a, kind, f, e, home, state, activity in FUEL_LINE.findall(output)
        if kind == aircraft
    ]


# Slice HF2: the least a rotorcraft's planned endurance (seconds) may be at the start with a full tank.
# The record's flow gave 33 minutes (AH-64, V-22) to 18 (Mi-24). Since slice HF3 the AI plans at its level
# cruise flow (docs/baselines/rotorcraft-fuel.md: 64 to 148 minutes with a full tank and no stores; the
# stores the wings carry take a few minutes off), and the V-22 and CH-47 carry their real tanks (11,700 lb, the
# MV-22B's, and 7,340 lb, the CH-47F's; their records had copied the AH-64's and the Mi-24's): over two hours.
ROTORCRAFT_MIN_ENDURANCE_S = {"Ah64": 3300, "Mi24": 6000, "Ch47": 7800, "V22": 7200}


# One AI rotorcraft's flight and the rotor adapter's command (`--probe-rotor-trace`, slice HF1).
ROTOR_LINE = re.compile(
    r"^AI rotor: tick=(\d+) actor=(\d+) activity=(\w+) agl=\S+ kt=(\S+) vs=(\S+) .*? cmd_climb=(\S+) ",
    re.M,
)

# Slice HF3 (John, 2026-10-13): a helicopter wingman is asked for at most 1,000 ft/min down while it chases its
# formation slot, or its leader's own sink when that is more (tore_sim::ai::formation::
# ROTORCRAFT_FORMATION_DESCENT_FPS, 16.7 ft/s). A little over for rounding.
FORMATION_DESCENT_FPS = 1000 / 60 + 0.5


def formation_descent_check(output: str) -> list[str]:
    """Every `Formation` line's commanded climb is no steeper down than the limit or its wing's other
    rotorcraft's sink at that tick (the leader's), and there are formation lines to judge."""
    problems = probe_problems(output)
    by_tick: dict[int, list[tuple[int, str, float, float]]] = {}
    for tick, actor, activity, _kt, vs, climb in ROTOR_LINE.findall(output):
        if climb == "-":
            continue
        by_tick.setdefault(int(tick), []).append((int(actor), activity, float(vs), float(climb)))
    judged = 0
    steepest = 0.0
    for tick, lines in sorted(by_tick.items()):
        for actor, activity, _vs, climb in lines:
            if activity != "Formation":
                continue
            judged += 1
            others = [v for a, act, v, _ in lines if a != actor and act != "Formation"]
            allowed = max([FORMATION_DESCENT_FPS] + [-v + 0.5 for v in others])
            steepest = min(steepest, climb)
            if climb < -allowed:
                problems.append(f"tick {tick} actor {actor} was asked for {-climb:.1f} ft/s down in formation (limit {allowed:.1f})")
    if judged < 50:
        problems.append(f"only {judged} formation lines: the scenario proves nothing")
    return problems[:20]


def rotorcraft_fuel_check(aircraft: str, *, bingo: bool) -> Callable[[str], list[str]]:
    """Slice H3: the `aircraft` the AI flies (an AH-64, Mi-24, CH-47 or V-22) keeps a fuel schedule.
    At the first line none of them is bingo or critical (with the probe's full tanks every one is
    within reach of its home) and each has a time home at its own cruise; with `bingo` (a probe given
    little fuel by `--probe-fuel`) the friendly ones start on caution, then turn bingo, head home
    (`ReturningToBase`, or since slice H2 the landing: `HoldingMarshal`, `Landing`, `Landed`) and are
    not lost."""

    def check(output: str) -> list[str]:
        problems = probe_problems(output)
        lines = fuel_lines(output, aircraft)
        if not lines:
            return problems + [f"no AI fuel line for {aircraft}"]
        first = min(t for t, *_ in lines)
        for tick, actor, fuel, endurance, home, state, activity in lines:
            if tick == first and state in ("Bingo", "Critical") and not bingo:
                problems.append(f"actor {actor} ({aircraft}) was {state} at the start with {fuel} lb, home {home} s")
            if tick == first and home == "none":
                problems.append(f"actor {actor} ({aircraft}) had no time home at the start")
            if tick == first and not bingo and endurance < ROTORCRAFT_MIN_ENDURANCE_S[aircraft]:
                problems.append(f"actor {actor} ({aircraft}) planned only {endurance} s of fuel with {fuel} lb")
        if bingo:
            # Home: the way home, or (slice H2) the landing there, which a wing close to home
            # starts at once at its marshal.
            landing = ("ReturningToBase", "HoldingMarshal", "Landing", "Landed")
            homeward = [l for l in lines if l[5] == "Bingo" and l[6] in landing]
            if not homeward:
                problems.append(f"no {aircraft} turned bingo and headed home: " + ", ".join(sorted({l[5] + "/" + l[6] for l in lines})))
        return problems

    return check


# Failures that are understood and documented in docs/testing/lane-ai.md. Each
# turns red when it starts to pass, so the entry gets removed with the fix.
# (The 1,074 ft strip failures are gone: short strips are no ground start, John
# 2026-09-30, so no scenario or fuzz seed starts on one. See `_strips.py`.)
KNOWN_FAILURES = {
    "ai-theater-cub-takeoff-a1": "friendly wing with no route leaves the map on a Key West ground start (docs/testing/lane-ai.md, decision 5)",
    "ai-theater-vla-takeoff-a6": "friendly wing with no route leaves the map on a Spassk Dalniy ground start, the first Blue field since slice AL1 (docs/testing/lane-ai.md, decision 5)",
}


def prediction_envelope_gap_regression(d):
    executable = ROOT / "target" / "release" / "examples" / ("missile_energy_cost.exe" if os.name == "nt" else "missile_energy_cost")
    packs = sorted(d.data.glob("menu-*.pack"))
    if not executable.exists() or not packs:
        d.problem("release cost example and imported menu pack required")
        return
    output = d.run("gaps", [executable, packs[-1], "1200", "--gap-witness"], timeout=120).text()
    if "gap-witness: passed" not in output:
        d.problem("imported hit/miss/hit fixture did not verify the region advice policy")


def prediction_launch_regression(d):
    """Same natural fight fixture as the independent review, no injected shots."""
    executable = ROOT / "target" / "release" / "examples" / ("missile_energy_cost.exe" if os.name == "nt" else "missile_energy_cost")
    if not executable.exists():
        d.problem("build --release -p tore-world --example missile_energy_cost before this scenario")
        return
    packs = sorted(d.data.glob("menu-*.pack"))
    if not packs:
        d.problem("no imported menu pack")
        return
    # John's Phase 2.6 decision: compare balance changes to the reviewed 2.5
    # counts, retaining the earlier Spec floor against admission regressions.
    cases = [(name, 14400, 8, 13, spec, [])
             for name, spec in [("UKR", 11), ("BAL", 12), ("SPA", 12), ("GRE", 12)]]
    cases.append(("big", 7200, 23, 26, 24, ["--big"]))
    for name, ticks, aircraft, phase25, spec, flags in cases:
        theater = "UKR" if name == "big" else name
        text = d.run(name, [executable, packs[-1], str(ticks), theater, "moving",
                            "AIM120.JT", "1", "--natural", *flags], timeout=240).text()
        match = re.search(rf"guided_launches=(\d+) ai={aircraft} .*refused=0 starved=0", text)
        if not match:
            d.problem(f"{name}: missing launch counts or prediction admission failed")
            continue
        launches = int(match[1])
        if not .7 * phase25 <= launches <= 1.3 * phase25:
            d.problem(f"{name}: guided launch count outside 30% of reviewed Phase 2.5 {phase25}")
        if launches < .7 * spec:
            d.problem(f"{name}: guided launch count below 70% of pre-overhaul Spec {spec}")


def prediction_player_cue_regression(d):
    executable = ROOT / "target" / "release" / "examples" / ("missile_energy_cost.exe" if os.name == "nt" else "missile_energy_cost")
    packs = sorted(d.data.glob("menu-*.pack"))
    if not executable.exists() or not packs:
        d.problem("release cost example and imported menu pack required")
        return
    for name, flags in [("4v4", []), ("big", ["--big"])]:
        output = d.run(name, [executable, packs[-1], "2400", "UKR", "moving", "AIM120.JT", "1", "--natural", "--player-cue", *flags], timeout=240).text()
        match = re.search(r"player_cue_samples=\[(\d+), (\d+), (\d+)\]", output)
        counts = list(map(int, match.groups())) if match else [0, 0, 0]
        if sum(counts[:2]) < 1800 or sum(counts[:2]) < .9 * sum(counts):
            d.problem(f"{name}: opening 20-second player cue must have answers on 75% of all ticks and 90% of observed-cue ticks: {counts}")


def prediction_turning_cue_regression(d):
    executable = ROOT / "target" / "release" / "examples" / ("missile_energy_cost.exe" if os.name == "nt" else "missile_energy_cost")
    packs = sorted(d.data.glob("menu-*.pack"))
    if not executable.exists() or not packs:
        d.problem("release cost example and imported menu pack required")
        return
    for rate in [3, 10]:
        output = d.run(str(rate), [executable, packs[-1], "2400", "UKR", "moving", "AIM120.JT", "1", "--natural", "--big", f"--player-turn={rate}"], timeout=240).text()
        match = re.search(r"player_cue_samples=\[(\d+), (\d+), (\d+)\]", output)
        c = list(map(int, match.groups())) if match else [0, 0, 0]
        clients = re.search(r"max_clients=(\d+)", output)
        if sum(c) < 600 or sum(c[:2]) < .9 * sum(c):
            d.problem(f"{rate} deg/s: need 600 observed ticks and 90% answered: {c}")
        if not clients or int(clients[1]) < 31:
            d.problem(f"{rate} deg/s: insufficient competing requests")


def scenarios() -> list[Scenario]:
    out: list[Scenario] = [Scenario(name="ai-missile-launch-regression", lane="ai", args=[],
        driver=prediction_launch_regression, timeout=1000,
        notes="Four 120-second natural 4v4 fights within 30% of reviewed Spec counts; 23+ AI admission stress. Build the release missile_energy_cost example first.")]
    out.append(Scenario(name="ai-missile-envelope-gaps", lane="ai", args=[], driver=prediction_envelope_gap_regression,
                        timeout=180, notes="R27ER hit/miss/hit points: the gap advises the first region without IN RNG; the far hit advises its own region."))
    out.append(Scenario(name="ai-missile-player-cue", lane="ai", args=[], driver=prediction_player_cue_regression, timeout=600,
        notes="Actual player HUD queries in the opening 20 seconds of 4v4 and 23-AI fights: answers on 75% of all ticks and 90% of observed-cue ticks."))
    out.append(Scenario(name="ai-missile-turning-cue", lane="ai", args=[], driver=prediction_turning_cue_regression, timeout=600,
        notes="3 and 10 deg/s scripted player turns in a natural 23-AI fight; observed HUD availability under real shared load."))
    attack = ["--probe-attack", "600:10"]

    # 1. Fight sizes, even and lopsided, the default F/A-18D against itself.
    sizes = [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (7, 7), (8, 8), (10, 10), (12, 12), (15, 15),
             (1, 15), (15, 1), (5, 10), (10, 5), (3, 12), (12, 3), (1, 5), (6, 2)]
    for f, e in sizes:
        out.append(probe(f"fight-{f}v{e}-default", fight(f, e, "--separation", "5", *attack)))
        out.append(probe(f"fight-{f}v{e}-noattack", fight(f, e, "--separation", "10"), ticks=9600))

    # 2. Every aircraft against every aircraft, two a side.
    for i, fa in enumerate(AIRCRAFT):
        for j, ea in enumerate(AIRCRAFT):
            skill = SKILLS[(i + j) % 4]
            out.append(probe(
                f"pair-{fa}-vs-{ea}",
                fight(2, 2, "--aircraft", fa, "--probe-friendly-aircraft", fa, "--probe-enemy-aircraft", ea,
                      "--probe-enemy-skill", skill, "--separation", "5", *attack),
                ticks=6000,
            ))

    # 3. Each aircraft as the enemy, bigger fights, researched adapter.
    for i, ea in enumerate(AIRCRAFT):
        fa = AIRCRAFT[(i + 5) % len(AIRCRAFT)]
        out.append(probe(
            f"big-{fa}-vs-{ea}-researched",
            fight(6, 6, "--aircraft", fa, "--probe-friendly-aircraft", fa, "--probe-enemy-aircraft", ea,
                  "--probe-flight-model", "researched", "--separation", "10", *attack),
            ticks=9600,
        ))

    # 4. Skills x geometry x adapter, one against one and three against three.
    for skill in SKILLS:
        for geo in GEOMETRIES:
            for adapter in ADAPTERS:
                out.append(probe(
                    f"skill-{skill}-{geo}-{adapter}-1v1",
                    ["--aircraft", "f22", "--probe-enemy-aircraft", "su27", "--probe-enemy-skill", skill,
                     "--probe-geometry", geo, "--probe-flight-model", adapter, "--separation", "2", *attack],
                    ticks=6000,
                ))
                out.append(probe(
                    f"skill-{skill}-{geo}-{adapter}-3v3",
                    fight(3, 3, "--aircraft", "mig29", "--probe-friendly-aircraft", "mig29", "--probe-enemy-aircraft", "f14",
                          "--probe-enemy-skill", skill, "--probe-geometry", geo, "--probe-flight-model", adapter,
                          "--separation", "5", *attack),
                    ticks=6000,
                ))

    # 5. Missions at three sizes, with and without the leader attacking.
    for mission in MISSIONS:
        for f, e in [(1, 1), (4, 4), (10, 10)]:
            out.append(probe(f"mission-{mission}-{f}v{e}", fight(f, e, "--ai-mission", mission, "--separation", "5", *attack)))
            out.append(probe(f"mission-{mission}-{f}v{e}-passive", fight(f, e, "--ai-mission", mission, "--separation", "10"), ticks=9600))

    # 6. Separations.
    for sep, ticks in [(1, 4800), (2, 6000), (10, 12000), (20, 18000), (50, 36000), (100, 48000)]:
        out.append(probe(f"separation-{sep}nm-3v3", fight(3, 3, "--separation", str(sep), *attack), ticks=ticks, timeout=1800))

    # 7. Guns only.
    for fa, ea in [("f18", "mig21"), ("a4e", "su25"), ("f14", "mig23"), ("x31", "rafale"), ("su35", "f22n")]:
        out.append(probe(f"guns-{fa}-vs-{ea}", fight(2, 2, "--aircraft", fa, "--probe-friendly-aircraft", fa, "--probe-enemy-aircraft", ea,
                                                       "--probe-guns", "--probe-ai-guns-only", "--separation", "2", *attack), ticks=9600))
        out.append(probe(f"guns-ai-only-{fa}-vs-{ea}", ["--aircraft", fa, "--probe-enemy-aircraft", ea, "--probe-ai-guns-only",
                                                        "--probe-geometry", "rear", "--separation", "1"], ticks=6000))
    # Four MiG-21s firing long bursts (fuzz seed 317): with 256 rounds in
    # flight at most they threw 29 away as "dropped launches" (lane doc).
    out.append(probe("guns-mig21-pack", ["--theater", "~FRA0", "--probe-fight", "1:4", "--aircraft", "a4e",
                                         "--probe-friendly-aircraft", "su25", "--probe-enemy-aircraft", "mig21",
                                         "--probe-enemy-skill", "experienced", "--ai-mission", "intercept", "--probe-geometry", "side",
                                         "--separation", "5", "--probe-flight-model", "researched", "--probe-attack", "720:20",
                                         "--probe-ai-guns-only", "--probe-wing-order", "6540:attack-on-contact",
                                         "--probe-wing-order", "1440:attack-on-contact", "--probe-wing-order", "3000:land-selected"],
                     ticks=14400))

    # 8. Fault injection into the first enemy, every index, and controlled threats.
    for index in range(45):
        out.append(probe(f"fault-{index:02d}", ["--probe-enemy-aircraft", "su27", "--probe-fault", f"240:{index}",
                                                 "--separation", "5", "--ai-mission", "hold"], ticks=4800))
    for threat in ["hit", "gun", "aaa"]:
        for geo in GEOMETRIES:
            out.append(probe(f"threat-{threat}-{geo}", ["--aircraft", "f22", "--probe-enemy-aircraft", "mig29", "--probe-geometry", geo,
                                                        "--probe-threat", f"120:{threat}", "--probe-threat", f"1200:{threat}",
                                                        "--separation", "1", "--ai-mission", "hold"], ticks=4800))
    out.append(probe("threat-many-hits", ["--probe-enemy-aircraft", "su35"] + sum((["--probe-threat", f"{120 + 60 * k}:hit"] for k in range(64)), []),
                     ticks=6000))

    # 9. Wing orders in the air.
    for order in ["bug-out", "land-selected", "attack-on-contact", "engage-my-target"]:
        for size in [2, 5]:
            # Engage-my-target needs the leader's designation, made at tick 600.
            at = 700 if order == "engage-my-target" else 600
            out.append(probe(f"order-{order}-wing{size}", ["--probe-wing-size", str(size), "--probe-wing-order", f"{at}:{order}",
                                                           "--separation", "5", "--probe-attack", "600:10"], ticks=12000,
                             expect=[r"order=\S+ (reply|refused)"]))

    # 9a. The sort order (stage G3c): the lead of four sorts its three wingmen
    # onto the two enemy aircraft 20 nm out: the first two take one each, the
    # third doubles up. The enemies are planes 6 and 7.
    out.append(probe("order-sort-wing4", ["--probe-wing-size", "4", "--separation", "20", "--probe-data-link",
                                          "--probe-wing-order", "400:sort"], ticks=2400,
                     expect=[r"data link: assign plane=1 target=\d+ by=0 order=Sort", r'order=Sort reply="Sort: 3 assigned'],
                     check=order_sort_check(wingmen=[1, 2, 3], enemies=[6, 7], lead=0)))

    # 9a2. AI leads share and sort (stage G4): in a fight of ten against ten the
    # AI leads of the other wings (planes 5, 10 and 15; the scripted player leads
    # plane 0's wing) deal their wingmen bandits when they take a new target.
    out.append(probe("datalink-lead-sort", ["--probe-fight", "10:10", "--separation", "20", "--probe-data-link"], ticks=9000,
                     expect=[r"data link: assign plane=\d+ target=\d+ by=(5|10|15) order=Sort"],
                     check=datalink_lead_sort_check(wing_size=5, per_side=10)))

    # 9b. The flight data link's picture (stage G0): members and their radar flag, the
    # player's lock from its designation, AI engagements, and a wing order to
    # one member (`@1`, the first wingman).
    out.append(probe("datalink-picture", ["--probe-wing-size", "2", "--separation", "5", "--probe-player-lock", "600:4",
                                          "--probe-wing-order", "700:engage-my-target@1"], ticks=2400,
                     expect=[r"data link: member plane=0 radar=true", r"data link: lock plane=0 target=4",
                             r"order=EngageMyTarget reply"],
                     check=datalink_picture_check(planes=6, designated=4)))
    # 9c. Assignments (stage G3a): an Engage my target to the first wingman
    # becomes its assignment (the probe prints `data link: assign`), and a later
    # Attack on contact clears it.
    out.append(probe("datalink-assign", ["--probe-wing-size", "2", "--separation", "5", "--probe-player-lock", "600:4",
                                         "--probe-wing-order", "700:engage-my-target@1",
                                         "--probe-wing-order", "900:attack-on-contact"], ticks=2400,
                     expect=[r"data link: assign plane=1 target=4 by=0 order=EngageMyTarget",
                             r"data link: clear plane=1 target=4 why=order"],
                     check=datalink_assign_check(plane=1, target=4, lead=0, cleared_by_order_at=900)))

    # 9c2. Assignments reach the AI (stage G3b): the first wingman has no
    # sensors of its own, so the player's lock puts the bandit in the picture
    # and the wingman takes the order on a flightmate's track, flies after it
    # and never fires on what only the link holds.
    out.append(probe("datalink-order-link", ["--probe-wing-size", "2", "--separation", "50", "--probe-blind-wing",
                                             "--probe-player-lock", "100:4",
                                             "--probe-wing-order", "400:engage-my-target@1"], ticks=2400,
                     expect=[r"data link: assign plane=1 target=4 by=0 order=EngageMyTarget",
                             r'order=EngageMyTarget reply="Engage my target: 1 applied, 0 rejected'],
                     check=datalink_order_link_check(wingman=1, target=4, lead=0)))

    # 9d. Humans in the engagement table (stage G2): the player locks one bandit of
    # a pair 20 nm out, and its wingman, ordered to attack once the lock is held,
    # takes the other (the player's lock counts as a wing member attacking it,
    # B41's penalty). Mirrored, so the geometry cannot explain the choice.
    for name, locked, takes in [("datalink-player-lock", 4, 5), ("datalink-player-lock-mirror", 5, 4)]:
        out.append(probe(name, ["--probe-wing-size", "2", "--separation", "20",
                                                              "--probe-player-lock", f"100:{locked}",
                                                              "--probe-wing-order", "400:attack-on-contact"], ticks=3000,
                         expect=[rf"data link: lock plane=0 target={locked}", rf"data link: lock plane=1 target={takes}"],
                         check=datalink_player_lock_check(wingman=1, locked=locked, takes=takes)))

    # 9z. AI helicopters in a Quick Mission (slice ST-H): the AH-64 and Mi-24 fly as the AI's
    # wings (friendly wings 2 and 3 and the enemy), and as the player's own AI wingmen on an airborne
    # start. They start at 1,500 ft over the ground (wings 2 and 3, enemies), keep no fuel schedule and
    # never enter an airfield sequence. The probes' own checks (no anomaly, no open hazard, no dead
    # aircraft with a live activity) are the test: a helicopter that dived or landed on a runway at
    # 120 knots shows as a ground hazard or a crash.
    out.append(probe("fight-heli-wings-vs-jets", fight(2, 2, "--aircraft", "f18", "--probe-friendly-aircraft", "mi24",
                                                  "--probe-enemy-aircraft", "su27", "--separation", "5", *attack),
                     ticks=7200, notes="Mi-24 friendly wings 2 and 3 with the player's F/A-18Ds against two Su-27s."))
    out.append(probe("fight-heli-vs-heli", fight(3, 3, "--aircraft", "f18", "--probe-friendly-aircraft", "ah64",
                                            "--probe-enemy-aircraft", "mi24", "--separation", "5", *attack),
                     ticks=9600, notes="AH-64 friendly wings against Mi-24 enemies."))
    out.append(probe("fight-heli-player-wing", ["--aircraft", "ah64", "--probe-wing-size", "3", "--separation", "10"],
                     ticks=9600, notes="A player in an AH-64 with two AI AH-64 wingmen on an airborne start."))
    out.append(probe("rtb-heli-land-order", ["--aircraft", "mi24", "--probe-wing-size", "3", "--separation", "100",
                                          "--probe-wing-order", "2000:land-selected"], ticks=14000,
                     notes="The land order to an airborne helicopter wing: with no airport selected it is "
                           "refused, and the wing flies on. Helicopters landing are ai-heli-land-home and "
                           "ai-heli-ground-start-wing (slice H2)."))

    # 9y. The CH-47, V-22, AV-8 and Yak-141 as the AI's aircraft (slice H3, John 2026-10-12): as the
    # player's AI wingmen, as friendly wings 2 (with fuel lines: the rotorcraft keep a fuel schedule
    # and are not bingo at the start), and as the enemy wing, airborne starts. The probes' own checks
    # (no anomaly, no open hazard, no undamaged ground impact) are the test. The CH-47 is unarmed: it
    # flies in formation and never strikes. Fuel lines come from `--probe-fuel-trace`.
    rotor_names = {"ch47": "Ch47", "v22": "V22", "mi24": "Mi24", "ah64": "Ah64"}
    for key in ["ch47", "v22", "av8", "yak141", "mi24", "ah64"]:
        fuel = ["--probe-fuel-trace", "20"] if key in rotor_names else []
        friendly_check = rotorcraft_fuel_check(rotor_names[key], bingo=False) if key in rotor_names else None
        out.append(probe(f"fight-{key}-friendly-wings", fight(7, 2, "--aircraft", "f18", "--probe-friendly-aircraft", key,
                                                         "--probe-enemy-aircraft", "su27", "--separation", "5", *attack, *fuel),
                         ticks=9600, check=friendly_check,
                         notes=f"The player's five F/A-18Ds and a friendly wing 2 of two {key} against two Su-27s."))
        if key in ("mi24", "ah64"):
            continue
        out.append(probe(f"fight-{key}-enemy-wing", fight(2, 2, "--aircraft", "f18", "--probe-enemy-aircraft", key,
                                                       "--separation", "5", *attack),
                         ticks=9600, notes=f"Two {key} as the enemy wing against the player's F/A-18D pair."))
        out.append(probe(f"fight-{key}-player-wing", ["--aircraft", key, "--probe-wing-size", "3", "--probe-enemy-aircraft", "su27",
                                                   "--separation", "10"],
                         ticks=9600, notes=f"A player in a {key} with two AI {key} wingmen on an airborne start."))
    # Rotorcraft fuel (slice H3): little fuel in the tanks, so the friendly helicopters start on caution,
    # turn bingo within the probe and head home (`ReturningToBase`). The enemy Su-27s have their own
    # fuel and the aircraft counted are the friendly rotorcraft alone. (The V-22 is not probed here: it
    # enters the aeroplane's airfield sequence when the fight ends, which is slice H2's to replace.)
    out.append(probe("fight-heli-fuel-bingo-ah64", ["--aircraft", "ah64", "--probe-wing-size", "3", "--probe-enemy-aircraft", "su27",
                                              "--separation", "10", "--probe-fuel", "250", "--probe-fuel-trace", "10"],
                     ticks=16800, check=rotorcraft_fuel_check("Ah64", bingo=True),
                     notes="The AI's AH-64 wingmen with 250 lb (about 9 minutes at the hover flow of the rotor's power, "
                           "slice HF2) start on caution, turn bingo under the time home plus the five minute reserve "
                           "within a minute and head home."))

    # Slice HF3: the V-22 with the MV-22B's fuel keeps a fuel schedule too: its AI wingmen with 700 lb (about nine
    # minutes at its cruise flow) turn bingo and head home.
    out.append(probe("fight-v22-fuel-bingo", ["--aircraft", "v22", "--probe-wing-size", "3", "--probe-enemy-aircraft", "su27",
                                        "--separation", "10", "--probe-fuel", "700", "--probe-fuel-trace", "10"],
                     ticks=16800, check=rotorcraft_fuel_check("V22", bingo=True),
                     notes="The AI's V-22 wingmen with 700 lb (about nine minutes at the level cruise flow, slice HF3) "
                           "turn bingo under the time home plus the five minute reserve and head home."))
    # Slice HF3: the CH-47 enemy wing of slice CH47 (whose wingman was asked for 42 ft/s down chasing its slot behind
    # a breaking lead) with the rotor trace: no formation request asks more than 1,000 ft/min down, or the leader's own sink.
    out.append(probe("fight-ch47-formation-descent", fight(2, 2, "--aircraft", "f18", "--probe-enemy-aircraft", "ch47",
                                                       "--separation", "5", *attack, "--probe-rotor-trace", "0.25"),
                     ticks=9600, check=formation_descent_check,
                     notes="Two CH-47s as the enemy wing: the wingman's formation descents stay at 1,000 ft/min or the "
                           "leader's sink (John, 2026-10-13)."))

    # 10. Ground starts: takeoff, formation, landing orders.
    for size in [1, 2, 3, 4, 5]:
        out.append(probe(f"ground-takeoff-wing{size}", ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", str(size), "--maneuver", "takeoff",
                                                         "--separation", "50"], ticks=6000 + 6000 * size,
                         check=checker(ground=True, need_takeoff=True)))
    for size in [2, 4]:
        out.append(probe(f"ground-land-selected-wing{size}", ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", str(size), "--maneuver", "takeoff",
                                                              "--probe-wing-order", f"{12000 * size // 2}:land-selected", "--separation", "200", "--probe-wing-only"],
                         # A wing's approach over the hills south of Simferopol takes about
                         # 400 s per aircraft (lane doc, "Needs a decision"), so only the
                         # pair is expected to be down inside the run. The scripted player
                         # survives its climb-out and leads the wing, so its order is
                         # accepted and the landing is checked (land_order_checker).
                         ticks=90000, timeout=1800,
                         check=land_order_checker(checker(ground=True, need_takeoff=True, need_landing=size == 2))))
        # Bug out is ignored while taking off (spec), so order it once the wing is up.
        out.append(probe(f"ground-bug-out-wing{size}", ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", str(size), "--maneuver", "takeoff",
                                                        "--probe-wing-order", f"{6000 * size}:bug-out", "--separation", "200", "--probe-wing-only"],
                         ticks=48000, timeout=1800, check=checker(ground=True, need_takeoff=True, player_flies=True)))
        out.append(probe(f"ground-fight-wing{size}", ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", str(size), "--maneuver", "takeoff",
                                                      "--separation", "20", "--probe-attack", "6000:10"], ticks=24000, timeout=1800,
                         check=checker(ground=True, need_takeoff=True)))
    out.append(probe("ground-idle-wing4", ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", "4", "--separation", "50", "--probe-wing-only"],
                     ticks=7200, check=checker(ground=True)))

    # 11. Long runs: 30 simulated minutes, looking for slow leaks and stuck aircraft.
    for name, args in [
        ("long-1v1", fight(1, 1, "--separation", "10", *attack)),
        ("long-5v5", fight(5, 5, "--separation", "20", *attack)),
        ("long-15v15", fight(15, 15, "--separation", "20", *attack)),
        ("long-hold-4v4", fight(4, 4, "--ai-mission", "hold", "--separation", "20")),
        ("long-guns-3v3", fight(3, 3, "--probe-guns", "--probe-ai-guns-only", "--separation", "5", *attack)),
        ("long-ground-land", ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", "4", "--maneuver", "takeoff",
                              "--probe-wing-order", "30000:land-selected", "--separation", "200", "--probe-wing-only"]),
    ]:
        # With no route an aircraft holds its heading (B48) and can fly off the
        # map in half an hour; recorded under "Needs a decision" in the lane doc.
        out.append(probe(name, args, ticks=216000, timeout=3600,
                         check=checker(ground="ground" in name, need_takeoff="ground" in name,
                                       allow_anomalies=("outside the world",))))

    # 12. Recordings: render verification and a readable log.
    for name, args in [
        ("record-2v2", fight(2, 2, "--separation", "5", *attack)),
        ("record-15v15", fight(15, 15, "--separation", "5", *attack)),
        ("record-guns", fight(2, 2, "--probe-guns", "--probe-ai-guns-only", "--separation", "2", *attack)),
        ("record-researched", fight(4, 4, "--probe-flight-model", "researched", "--separation", "5", *attack)),
        ("record-fault", ["--probe-fault", "300:11", "--separation", "2", *attack]),
        ("record-threat-aaa", ["--probe-threat", "120:aaa", "--probe-geometry", "rear", "--separation", "1", "--ai-mission", "hold"]),
        ("record-ground", ["--ground-start", GROUND_AIRPORT, "--probe-wing-size", "3", "--maneuver", "takeoff", "--separation", "20"]),
    ]:
        out.append(probe(name, [*args, "--record-mission", "{work}/a.tore-replay", "--verify-render"],
                         ticks=7200, expect=[r"AI probe verify-render: PASS"], outputs=["a.tore-replay"],
                         check=checker(ground="ground" in name)))

    # 13. Determinism: the same arguments twice give the same output.
    for name, args in [
        ("determinism-1v1", ["--ai-probe-ticks", "6000", *fight(1, 1, "--separation", "2", *attack), "--no-audio"]),
        ("determinism-8v8", ["--ai-probe-ticks", "6000", *fight(8, 8, "--separation", "5", *attack), "--no-audio"]),
        ("determinism-researched", ["--ai-probe-ticks", "6000", *fight(4, 4, "--probe-flight-model", "researched", *attack), "--no-audio"]),
        ("determinism-ground", ["--ai-probe-ticks", "9000", "--ground-start", GROUND_AIRPORT, "--probe-wing-size", "4", "--maneuver", "takeoff", "--no-audio"]),
    ]:
        out.append(Scenario(name=f"ai-{name}", lane="ai", args=args, timeout=1800,
                            expect=[r"AI probe totals:"], check=rerun_same(args)))

    recorded = ["--ai-probe-ticks", "4800", *fight(5, 5, "--separation", "5", *attack), "--no-audio"]
    out.append(Scenario(name="ai-determinism-recordings", lane="ai", args=recorded, timeout=1800,
                        expect=[r"AI probe totals:"], check=recordings_match(recorded)))

    # 13b. Regressions for fixed defects, checked strictly.
    out.append(probe("regress-visual-incoming-flap", fight(6, 6, "--aircraft", "f22", "--probe-friendly-aircraft", "f22",
                                                           "--probe-enemy-aircraft", "mig29", "--separation", "10", *attack),
                     ticks=14400, check=checker(strict=True),
                     notes="wingman flipped between missile defense and formation every tick (fixed 2026-09-28)"))
    def wingman_back_in_fight(output: str) -> list[str]:
        problems = probe_problems(output)
        # With no friendly left (the player destroyed, the friendly wingman
        # out), Enemy 1-2 has won and goes home to land, as winners do. Splash
        # damage (2026-10-10) changed this fight so that it can end that way.
        won = (re.search(r"destroyed: player$", output, re.M)
               and not re.search(r"^actor=\d+ Friendly \S+ \S+ activity=.*? alive=true", output, re.M))
        if not won and re.search(r"^actor=3 Enemy 1-2 \S+ activity=(Landing|Holding at marshal) alive=true", output, re.M):
            problems.append("Enemy 1-2 is still following its ejected leader in to land")
        return problems

    out.append(probe("regress-wingman-dead-leader", fight(2, 2, "--aircraft", "f18", "--probe-friendly-aircraft", "f18",
                                                          "--probe-enemy-aircraft", "su27", "--probe-enemy-skill", "ace",
                                                          "--separation", "5", *attack),
                     ticks=6000, check=wingman_back_in_fight,
                     notes="wingman kept landing after its damaged leader ejected (fixed 2026-09-28)"))
    out.append(probe("regress-envelope-edge-flap", fight(8, 8, "--separation", "5", *attack), ticks=7200,
                     check=checker(strict=True, allow_anomalies=("mid-air collision",)),
                     notes="Friendly 2-3 swapped between its missile tactic and gun tracking every few ticks (fixed 2026-09-29)"))
    # Formerly a known failure (lane doc): an F-22 test-harness leader
    # outruns its wingman, which chases at 800 kt, 900 ft above hills that
    # rose faster than the old 1,000 ft terrain look-ahead could see. Passes
    # with the six-second look-ahead (2026-09-29). Since slice AL1 the wing
    # leaves Ivano Frankivs'k under the Carpathians: Krasnodar (airport 3),
    # where it first failed, is a Redfor field and no Blue ground start.
    out.append(probe("known-f22-leader-wingman-ukr12", [
        "--theater", "UKR", "--aircraft", "f22", "--ground-start", "12",
        "--maneuver", "takeoff", "--probe-wing-size", "2", "--probe-wing-only"],
        ticks=9000, check=checker(ground=True, need_takeoff=True, player_flies=True)))
    out.append(probe("regress-gun-missile-flap-su35", ["--aircraft", "su35", "--probe-enemy-aircraft", "mig29",
                                                       "--probe-enemy-skill", "average", "--probe-fight", "5:5",
                                                       "--separation", "10", "--probe-attack", "100:8"],
                     ticks=20000, check=checker(strict=True, allow_anomalies=("mid-air collision",)),
                     notes="Enemy 1-5 swapped gun tracking and its missile tactic 30 times in 3 s (fixed 2026-09-29)"))
    out.append(probe("regress-gun-missile-flap-a4e", fight(2, 2, "--aircraft", "a4e", "--probe-friendly-aircraft", "a4e",
                                                            "--probe-enemy-aircraft", "mig29", "--probe-enemy-skill", "novice",
                                                            "--separation", "5", *attack),
                     ticks=6000, check=checker(strict=True, allow_anomalies=("mid-air collision",)),
                     notes="Enemy 1-2 swapped gun tracking and its missile tactic every few ticks (fixed 2026-09-29)"))
    # John, 2026-09-29: once no hostile aircraft remains the winning wing
    # returns to base and lands (lane doc, bb2 item 17).
    def winners_go_home(output: str) -> list[str]:
        problems = probe_problems(output)
        final = re.findall(r"^actor=\d+ (Friendly|Enemy) (\S+) \S+ activity=(.*?) alive=(true|false)", output, re.M)
        if any(side == "Friendly" and alive == "true" for side, _, _, alive in final):
            problems.append("the friendly wing survived, so the fight never ended")
        home = ("Returning to base", "Holding at marshal", "Landing", "Landed", "Taxiing")
        for side, member, activity, alive in final:
            if side == "Enemy" and alive == "true" and activity not in home:
                problems.append(f"Enemy {member} is still {activity} with no hostile aircraft left")
        return problems

    out.append(probe("rtb-after-win", fight(2, 4, "--aircraft", "mig21", "--probe-friendly-aircraft", "mig21",
                                            "--probe-enemy-aircraft", "f22", "--probe-enemy-skill", "ace",
                                            "--separation", "10", *attack),
                     ticks=72000, timeout=1800, check=winners_go_home,
                     notes="the winning wing lands at its home runway once no hostile aircraft remains (added 2026-09-29)"))
    # A 15-aircraft winning side returning to base on a 3 degree path in
    # several theaters: every one must land or still be in the sequence.
    for theater in ("UKR", "PGU", "FRA", "VLA"):
        out.append(probe(f"rtb-after-win-2v15-{theater.lower()}",
                         ["--theater", theater, *fight(2, 15, "--aircraft", "mig21", "--probe-friendly-aircraft", "mig21",
                                                       "--probe-enemy-aircraft", "f18", "--probe-enemy-skill", "ace",
                                                       "--separation", "10", *attack)],
                         ticks=144000, timeout=3600, check=winners_go_home,
                         notes="fifteen winners land on the 3 degree path (added 2026-09-29)"))
    out.append(probe("regress-decoy-over-100", fight(2, 2, "--aircraft", "su25", "--probe-friendly-aircraft", "su25",
                                                     "--probe-enemy-aircraft", "mig21", "--separation", "5", *attack),
                     ticks=6000, notes="mission aborted: decoy percentages exceed 100 (fixed 2026-09-28)"))
    out.append(probe("regress-terrain-f14-dive", ["--probe-fight", "3:3", "--aircraft", "mig29", "--probe-friendly-aircraft", "mig29",
                                                  "--probe-enemy-aircraft", "f14", "--probe-enemy-skill", "novice", "--probe-geometry", "head",
                                                  "--probe-flight-model", "legacy", "--separation", "5", *attack],
                     ticks=6000, notes="undamaged F-14 flew into the ground turning at 72 degrees of bank (fixed 2026-09-28)"))
    out.append(probe("regress-terrain-f18-hill", fight(1, 1, "--separation", "10", *attack), ticks=24000,
                     notes="undamaged F/A-18D eased into a hillside at 2 G (fixed 2026-09-28)"))

    # 15. Other theaters: fights, ground starts, takeoff and landing, and
    # every aircraft's ground-start takeoff at a few airports.
    for theater in THEATERS:
        where = ["--theater", theater]
        out.append(probe(f"theater-{theater.lower()}-fight-4v4", where + fight(4, 4, "--separation", "5", *attack)))
        out.append(probe(f"theater-{theater.lower()}-fight-8v8-noattack", where + fight(8, 8, "--separation", "10"), ticks=9600))
        # A short strip is no ground start (John, 2026-09-30), nor is a
        # Redfor field for the Blue wing (slice AL1): the next airport that
        # is one takes its place, and names the scenario. The Falklands and
        # North Vietnam have no Blue field, so no ground start there.
        airports = []
        for wanted in (1, 3):
            found = ground_airport(theater, wanted)
            if found is not None and found not in airports:
                airports.append(found)
        for airport in map(str, airports):
            out.append(probe(f"theater-{theater.lower()}-takeoff-a{airport}", where + [
                "--ground-start", airport, "--probe-wing-size", "3", "--maneuver", "takeoff", "--separation", "50"],
                ticks=24000, timeout=1800, check=checker(ground=True, need_takeoff=True)))
        if not airports:
            continue
        out.append(probe(f"theater-{theater.lower()}-land-pair", where + [
            "--ground-start", str(airports[0]), "--probe-wing-size", "2", "--maneuver", "takeoff",
            "--probe-wing-order", "18000:land-selected", "--separation", "200", "--probe-wing-only"],
            ticks=LAND_PAIR_TICKS.get(theater, 108000), timeout=2400,
            check=checker(ground=True, need_takeoff=True, need_landing=True, player_flies=True)))
    # Runway ends whose 3 degree path meets terrain in the last 5 nm
    # (`--validate-ils` prints them as `ils-terrain:`; docs/testing/ils.md):
    # a pair ordered to land must land without hitting the ground and must
    # not hang at the gates (added 2026-09-29).
    def lands_without_hanging(output: str) -> list[str]:
        problems = checker(ground=True, need_takeoff=True, need_landing=True)(output)
        for label, phases in re.findall(r"(\w+ \d-\d): ((?:\w+@[\d.]+s ?)+)", output):
            times = {p.split("@")[0]: float(p.split("@")[1][:-1]) for p in phases.split()}
            if "Approach" in times and "Final" in times and times["Final"] - times["Approach"] > GATES_MAX_S:
                problems.append(f"{label} flew the gates for {times['Final'] - times['Approach']:.0f} s")
        return problems

    # L'viv and Ivano Frankivs'k (UKR 8 and 12) are Blue fields. Amiens (FRA
    # 9), Burevestnik (KURILE 3), Hyon Ni (NSK 6), Donets'k and Kharkiv (UKR 5
    # and 6) are Redfor fields since slice AL1: the Blue wing neither starts
    # nor is ordered to land there, so the Redfor seat flies them
    # (`--probe-redfor`, slice CL-R): the scripted leader and its wingman are
    # Enemy Wing 1 of an open mission, parked there by an enemy ground start.
    for theater, airport, redfor in (
        ("UKR", "8", False), ("UKR", "12", False),
        ("FRA", "9", True), ("KURILE", "3", True), ("NSK", "6", True), ("UKR", "5", True), ("UKR", "6", True),
    ):
        out.append(probe(f"ils-terrain-{theater.lower()}-a{airport}", [
            "--theater", theater, "--ground-start", airport, "--probe-wing-size", "2", "--maneuver", "takeoff",
            "--probe-wing-order", "18000:land-selected", "--separation", "200", "--probe-wing-only",
            *(["--probe-redfor"] if redfor else [])],
            ticks=120000, timeout=2400, check=land_order_checker(lands_without_hanging)))
    # Redrawn airports (slice CL-AP, John 2026-10-11: "ensure AI actually taxi
    # on taxiways instead of grass"): a ground start, takeoff, land order and
    # taxi to parking with no AI ground tick off the drawn pavement, at a base
    # plan, the paired tiles (STRIP5A and STRIP6A fly the joined crossing
    # runway) and two lone tiles. `--probe-pavement` prints the count; any
    # off-pavement tick is an `off the pavement` anomaly in every ground probe.
    def on_pavement(landers: int):
        def check(output: str) -> list[str]:
            # The scripted player flies on alone for up to 30 minutes and is
            # not what these check, so it may come down.
            problems = checker(ground=True, need_takeoff=True, need_landing=True)(output)
            m = re.search(r"^AI probe pavement: ground_samples=(\d+) off_pavement=(\d+)$", output, re.M)
            if not m:
                problems.append("no 'AI probe pavement' line")
            elif int(m.group(1)) == 0 or int(m.group(2)) > 0:
                problems.append(f"pavement: {m.group(0)}")
            parked = len(re.findall(r"Parked@", output))
            if parked < landers:
                problems.append(f"{parked} of {landers} AI aircraft parked")
            return problems
        return check

    for name, theater, airport, extra, size, order, landers in (
        ("ukr-a8-wing4", "UKR", "8", [], "4", 24000, 3),
        ("fra-a24-tile", "FRA", "24", [], "2", 9000, 1),
        ("egy-a22-tile", "EGY", "22", [], "2", 9000, 1),
        ("nsk-a18-tile-f16", "NSK", "18", ["--aircraft", "f16c", "--probe-friendly-aircraft", "f16c"], "2", 9000, 1),
        ("vlaf-a1-lone", "~VLAF", "1", [], "2", 9000, 1),
        ("balf-a1-lone", "~BALF", "1", [], "2", 9000, 1),
    ):
        out.append(probe(f"airfield-taxi-{name}", [
            "--theater", theater, "--ground-start", airport, *extra, "--probe-wing-size", size, "--maneuver", "takeoff",
            "--probe-wing-order", f"{order}:land-selected", "--separation", "200", "--probe-wing-only", "--probe-pavement"],
            ticks=216000 if size == "4" else 108000, timeout=2400, check=on_pavement(landers)))
    # nr-apmerge: both old IDs of a merged field lead to the same pavement.
    # Southampton leaves room for the scripted Hornet leader's departure;
    # Spassk Dalniy's northern map edge is too close for a long straight jet flight.
    for aircraft in ("f18", "c130"):
        for size in (2, 4):
            theater = "FRA" if aircraft == "f18" else "VLA"
            airport = ("14" if size == 2 else "24") if aircraft == "f18" else ("6" if size == 2 else "22")
            order = 12000 * size // 2 if aircraft == "f18" else 36000
            out.append(probe(f"merged-airfield-{aircraft}-wing{size}", [
                "--theater", theater, "--ground-start", airport,
                "--aircraft", aircraft, "--probe-friendly-aircraft", aircraft,
                "--probe-wing-size", str(size), "--maneuver", "takeoff",
                "--probe-wing-order", f"{order}:land-selected", "--separation", "200",
                "--probe-wing-only", "--probe-pavement"],
                ticks=264000, timeout=2400, check=on_pavement(size-1),
                notes="Two/four aircraft at merged Southampton or Spassk Dalniy, including the old second-half ID."))
    # An enemy ground start in single player (slice CL-R): Enemy Wing 1 parks
    # at Kharkiv (UKR 6, a Redfor field) while the player's wing leaves Odesa;
    # the AI pair must start on the ground and be flying at the end.
    def enemy_wing_took_off(output: str) -> list[str]:
        problems = checker(ground=True)(output)
        starts = re.findall(r"^t=0 \(0\.0s\) (Enemy 1-\d) \S+: alive=true agl=(-?\d+) ", output, re.M)
        if len(starts) != 2 or any(int(agl) > 50 for _, agl in starts):
            problems.append(f"Enemy Wing 1 did not start parked: {starts}")
        for raw in output.splitlines():
            m = ACTOR.match(raw)
            if m and m.group(2).startswith("Enemy 1-"):
                y = re.search(r" y=(-?[\d.]+) ", raw)
                if m.group(5) != "true" or not y or float(y.group(1)) < 1500:
                    problems.append(f"{m.group(2)} is not flying at the end: {raw[:160]}")
        return problems

    out.append(probe("enemy-ground-start-ukr-a6", [
        "--theater", "UKR", "--ground-start", "9", "--enemy-ground-start", "6", "--probe-wing-size", "2",
        "--maneuver", "takeoff", "--separation", "50"],
        ticks=24000, timeout=1800, check=enemy_wing_took_off))
    # The new lead after a lost human (John, 2026-09-30). The scripted pilot
    # survives, so these probes lose the player on purpose with
    # `--probe-lose-player`, 60 s after takeoff at Kharkiv and 20 s after
    # takeoff at Simferopol, where the old pilot used to crash. The lead flies
    # the wing's waypoints, then goes home and lands. Kharkiv is a Redfor
    # field since slice AL1, so the route probe leaves Odesa (airport 9).
    out.append(probe("lost-lead-route-ukr-a9", [
        "--theater", "UKR", "--ground-start", "9", "--probe-wing-size", "2", "--maneuver", "takeoff",
        "--separation", "200", "--probe-wing-only", "--probe-lose-player", "7200",
        "--probe-wing-route", "8:8:8000", "--probe-wing-route", "0:16:8000"],
        ticks=120000, timeout=2400, check=lost_lead_route_checker))
    # With no route and a land order the player can no longer give, the new
    # lead returns to base at once; a wingman still waiting to take off in a
    # wing of four stays parked.
    for size in [2, 4]:
        out.append(probe(f"lost-lead-land-order-wing{size}", [
            "--ground-start", GROUND_AIRPORT, "--probe-wing-size", str(size), "--maneuver", "takeoff",
            "--probe-lose-player", "2400", "--probe-wing-order", f"{12000 * size // 2}:land-selected",
            "--separation", "200", "--probe-wing-only"],
            ticks=90000, timeout=1800,
            check=land_order_checker(checker(ground=True, need_takeoff=True, need_landing=size == 2), player_lost=True)))
    # Blue fields (slice AL1): Odesa, Ras Al Khaimah, Florennes and Kimpo.
    # Before it the UKR, FRA and NSK starts were Zaporizhzhya, Chateaudun and
    # Nuchon Ni, Redfor fields.
    for theater, airport in (("UKR", "9"), ("PGU", "2"), ("FRA", "4"), ("NSK", "9")):
        for aircraft in AIRCRAFT:
            out.append(probe(f"takeoff-{theater.lower()}-a{airport}-{aircraft}", [
                "--theater", theater, "--ground-start", airport, "--aircraft", aircraft,
                "--probe-friendly-aircraft", aircraft, "--probe-wing-size", "2", "--maneuver", "takeoff",
                "--separation", "100", "--probe-wing-only"],
                ticks=18000, timeout=1800, check=checker(ground=True, need_takeoff=True, player_flies=True)))
    # Corrected aircraft records (slice nr-rec, docs/spec/aircraft-records.md):
    # an aircraft whose record the game corrects takes off from Odesa, flies on
    # for 150 s and lands on a land order. The MiG-17F is not here: with its
    # default drop tanks it cannot climb away after liftoff, on the retail
    # record too (an open finding, not this correction).
    for aircraft in ("su25", "c130"):
        out.append(probe(f"records-land-{aircraft}", [
            "--theater", "UKR", "--ground-start", "9", "--aircraft", aircraft,
            "--probe-friendly-aircraft", aircraft, "--probe-wing-size", "2", "--maneuver", "takeoff",
            "--probe-wing-order", "18000:land-selected", "--separation", "200", "--probe-wing-only"],
            ticks=108000, timeout=2400,
            check=checker(ground=True, need_takeoff=True, need_landing=True, player_flies=True)))
    for theater in ("PGU", "VLA"):
        out.append(probe(f"long-15v15-{theater.lower()}", ["--theater", theater, *fight(15, 15, "--separation", "20", *attack)],
                         ticks=216000, timeout=3600, check=checker(allow_anomalies=("outside the world",))))

    # 19. Damaged aircraft for 20 minutes: faults with threats at the same
    # time, recovery landings, ejections. Weapons hold keeps the fight from
    # deciding it. The damaged first enemy must end landed, dead or ejected,
    # or flying free, never still on its way down after 20 minutes.
    def damaged_outcome(output: str) -> list[str]:
        problems = probe_problems(output, allow_anomalies=("outside the world",))
        final = re.search(r"^actor=(\d+) Enemy 1-1 \S+ activity=(.*?) alive=(true|false)", output, re.M)
        # Still flying its approach counts as flying: over the hills south of
        # Simferopol a slow damaged approach can take more than 20 minutes
        # (lane doc, "Needs a decision", item 6). Stuck on the ground does not,
        # but taxiing in after landing (its sequence clearing the runway) is
        # the recovery finishing.
        if final and final.group(3) == "true" and final.group(2) in ("Taxiing", "Waiting to take off"):
            taxiing_in = re.search(rf"^AI damage: actor={final.group(1)} .*landing=Some\(TaxiClear\)", output, re.M)
            if not (final.group(2) == "Taxiing" and taxiing_in):
                problems.append(f"damaged Enemy 1-1 still {final.group(2)} after 20 minutes")
        return problems

    for index in [1, 4, 7, 11, 12, 14, 19, 21, 25, 29, 30, 34]:
        for threat in ["hit", "gun"]:
            out.append(probe(f"damaged-fault{index:02d}-{threat}",
                             ["--probe-enemy-aircraft", AIRCRAFT[index % len(AIRCRAFT)], "--probe-fault", f"600:{index}",
                              "--probe-threat", f"600:{threat}", "--probe-threat", f"1800:{threat}",
                              "--probe-fault", f"2400:{(index + 7) % 45}", "--separation", "10", "--ai-mission", "hold"],
                             ticks=144000, timeout=3600, check=damaged_outcome))

    # 18. Player-wing orders given again and again in a fight.
    def orders_answered(expected: int) -> Callable[[str], list[str]]:
        def check(output: str) -> list[str]:
            problems = probe_problems(output)
            answered = len(re.findall(r"^t=\d+ order=\S+ (reply|refused)", output, re.M))
            if answered != expected:
                problems.append(f"{answered} of {expected} wing orders answered")
            return problems
        return check

    for size, fa, ea, ground in [(5, "f18", "mig29", False), (3, "su27", "f14", False), (4, "f18", "su35", True)]:
        orders = []
        start = 12000 if ground else 600
        for k in range(12):
            orders += ["--probe-wing-order", f"{start + k * 1200}:{ORDER_NAMES[k % 4]}"]
        where = ["--ground-start", GROUND_AIRPORT, "--maneuver", "takeoff", "--separation", "20"] if ground else ["--separation", "5"]
        out.append(probe(f"orders-cycle-{fa}-{ea}{'-ground' if ground else ''}",
                         ["--probe-wing-size", str(size), "--aircraft", fa, "--probe-friendly-aircraft", fa,
                          "--probe-enemy-aircraft", ea, *where, "--probe-attack", f"{start}:10", *orders],
                         ticks=start + 16000, timeout=1800, check=orders_answered(12)))

    # CAP must navigate before contact, on both sides, then fight intruders.
    def cap_orders(output: str, patrol_only: bool, expected: int = 2) -> list[str]:
        problems = probe_problems(output)
        rows = re.findall(r"AI probe CAP: actor=(\d+) first_target_tick=(None|Some\(\d+\)) shots=(\d+) max_station_nm=([\d.]+)", output)
        if len(rows) != expected:
            problems.append(f"expected {expected} CAP leaders, got {len(rows)}")
        for actor, first, shots, radius in rows:
            if patrol_only and float(radius) > 13:
                problems.append(f"CAP {actor} left its station by {radius} nm")
            if not patrol_only and (first == "None" or int(shots) == 0):
                problems.append(f"CAP {actor} failed to engage and fire: {first}, {shots} shots")
        return problems

    for label, separation, patrol_only in [("station", "150", True), ("engage", "5", False)]:
        out.append(probe(f"objective-cap-orders-{label}",
            fight(6, 1, "--probe-friendly-aircraft", "f15", "--probe-enemy-aircraft", "su27",
                  "--probe-group", "2:2", "--probe-group", "4:2", "--separation", separation),
            ticks=18000, check=lambda output, only=patrol_only: cap_orders(output, only),
            notes="Friendly F-15 and enemy Su-27 CAP leaders patrol their own stations before contact and engage intruders."))

    for label, args in [
        ("friendly", fight(6, 1, "--probe-friendly-aircraft", "f15", "--probe-enemy-aircraft", "il76",
                           "--probe-group", "2:2", "--probe-group", "4:8")),
        ("enemy", fight(1, 1, "--aircraft", "il76", "--probe-enemy-aircraft", "su27",
                        "--probe-group", "4:2")),
    ]:
        out.append(probe(f"objective-cap-intruder-{label}",
            [*args, "--separation", "50", "--record-mission", "{work}/cap.tore-replay"],
            ticks=72000, check=lambda output: cap_orders(output, False, 1),
            outputs=["cap.tore-replay"],
            notes="A transport enters the CAP station from 50 nm away; the patroller must acquire and fire without a player release order."))

    # 17. Creator objectives and required survival against the debrief.
    group_cases = [
        ("free", ["--probe-group", "1:1"]),
        ("cap", ["--probe-group", "1:2"]),
        ("target-e1", ["--probe-group", "1:3"]),
        ("target-e2", ["--probe-group", "1:4", "--probe-group", "3:0:survive"]),
        ("protect-f2", ["--probe-group", "1:6"]),
        ("protect-f3-survive", ["--probe-group", "1:7", "--probe-group", "3:0:survive"]),
        ("self-defense-survive", ["--probe-group", "1:8:survive"]),
        ("hold", ["--probe-group", "1:9"]),
        ("own-survive", ["--probe-group", "1:1:survive", "--probe-group", "2:0:survive"]),
        ("enemy-escort", ["--probe-group", "1:3", "--probe-group", "4:6", "--probe-group", "5:0:survive"]),
    ]
    for name, groups in group_cases:
        for f, e, attacking in [(6, 6, True), (12, 12, True), (11, 13, False), (15, 15, True)]:
            extra = attack if attacking else []
            out.append(probe(f"objective-{name}-{f}v{e}{'' if attacking else '-passive'}",
                             fight(f, e, "--separation", "5", *extra, *groups), ticks=12000))

    # 16. Seeded random configurations (tools/battery_scenarios/_ai_fuzz.py).
    for seed in _ai_fuzz.selected_seeds():
        args, ticks, info = _ai_fuzz.config(seed)
        out.append(probe(f"fuzz-{seed:04d}", args, ticks=ticks, timeout=2400,
                         check=fuzz_checker(seed, args, ticks),
                         notes=f"fuzz seed {seed}: {info}"))

    # 14. The fixed acceptance probes.
    out.append(Scenario(name="ai-roster-probe", lane="ai", args=["--ai-roster-probe-ticks", "3600", "--no-audio"], timeout=1800))
    out.append(Scenario(name="ai-probe-matrix", lane="ai",
                        args=["--probe-matrix", "{work}/matrix", "--ai-probe-ticks", "360", "--separation", "1", "--no-audio"],
                        timeout=3600, check=matrix_problems))
    for s in out:
        if s.name in KNOWN_FAILURES:
            s.known_failure = KNOWN_FAILURES[s.name]
    out.extend(vertical_scenarios())
    return out


def fuzz_checker(seed: int, args: list[str], ticks: int) -> Callable[[str], list[str]]:
    """The usual checks, each problem prefixed with the seed and command."""
    command = " ".join(["tore-app", "--ai-probe-ticks", str(ticks), *args, "--no-audio"])

    def check(output: str) -> list[str]:
        problems = probe_problems(output, ground="--ground-start" in args,
                                  allow_anomalies=("outside the world",))
        return [f"seed {seed} ({command}): {p}" for p in problems]

    return check


def matrix_problems(output: str) -> list[str]:
    problems = []
    cases = len(re.findall(r"^AI probe matrix: ", output, re.M))
    if cases < 1008:
        problems.append(f"probe matrix ran {cases} of 1008 cases")
    for line in ANOMALY.findall(output):
        problems.append(f"anomaly: {line[:200]}")
    return problems[:40]
