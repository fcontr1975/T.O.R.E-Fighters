"""Lane: net. A ground target online (protocol 22, slice N1), over real UDP on 127.0.0.1.

`net-surface-loopback`: a `tore-server` flies the dedicated server guide's mission with a ground target in
Ukraine, the moving column `~QUCOL` at heavy AAA and SAM defenses, the enemy 5 nm from the column, and two bots
fly it. The server draws the surface seed before it sends the mission; every bot builds the same surface (one
digest), draws the column's moving tanks and is told what the defenses do (radars, rails, damage, bursts or
launches).

`net-surface-pvp`: the same mission in PvP with one bot on each side. Redfor defends the target: the Blue bot's
debrief has the Destroy line, the Red bot's the Protect line; Redfor's own radars never paint the Red bot (Blue's may). Blue's
capture converts into a format 4 replay with the ground target, its seed and the surface units named.

`net-redfor-ground-start`: the same PvP mission with `enemy-start ground auto` (slice CL-R): the Red bot takes
Enemy Wing 1's lead parked at the Redfor field the world picks and starts there on the ground (the bot's start
line), Blue in the air; no Redfor radar paints the Red bot on its own field.

`net-area-defenses`: the guide's mission with no ground target and heavy AAA and SAM strengths (B23): two bots
build the same surface with the enemy's area defenses (one digest) and nobody is refused.

All read the `tore-bot` surface line (`NAME: surface: digest D, moving M, ...`, every five seconds). See
docs/testing/lane-net.md.
"""
from __future__ import annotations

import re

from battery import Drive, Scenario
from battery_scenarios.net import NET_BAD, guide_mission, server_log, start_bots, start_server, weapons_hold

SURFACE_LINE = re.compile(
    r"^(?P<name>\w+): surface: digest (?P<digest>[0-9a-f]{16}), moving (?P<moving>\d+), wrecks (?P<wrecks>\d+), "
    r"bursts (?P<bursts>\d+), ends (?P<ends>\d+), states (?P<states>\d+), told (?P<told>\d+), "
    r"launches (?P<launches>\d+), flak (?P<flak>\d+), painted (?P<painted>\d+), own (?P<own>\d+), rounds (?P<rounds>\d+)$",
    re.M,
)


def surface_mission(separation_nm: int = 5) -> str:
    """The guide's mission (Ukraine) with the moving column as its ground target, heavily defended, the enemy
    `separation_nm` from Blue, at 5,000 feet so the guns reach."""
    mission = guide_mission(separation_nm=separation_nm)
    mission = re.sub(r"(?m)^start airborne \d+$", "start airborne 5000", mission)
    return mission + "ground-target QUCOL\ndefenses aaa heavy sam heavy\n"


def last_surface(text: str) -> dict[str, dict[str, str]]:
    """Each bot's last surface line, by callsign."""
    lines: dict[str, dict[str, str]] = {}
    for m in SURFACE_LINE.finditer(text):
        lines[m.group("name")] = m.groupdict()
    return lines


def surface_problems(text: str, names: list[str]) -> list[str]:
    """What the bots' surface lines say against the scenario's rules: every bot printed one, all with one digest,
    each drew the moving column."""
    problems = []
    seen = last_surface(text)
    for name in names:
        if name not in seen:
            problems.append(f"{name} printed no surface line")
    digests = {line["digest"] for line in seen.values()}
    if len(digests) > 1:
        problems.append(f"the bots built different surfaces: {sorted(digests)}")
    for name, line in seen.items():
        if int(line["moving"]) < 9:
            problems.append(f"{name} drew {line['moving']} moving units, not the column's nine tanks")
    return problems


def drive_loopback(d: Drive) -> None:
    """Two bots fly the defended column with a server; one surface everywhere, and the defenses reach them."""
    port = d.port()
    server = start_server(d, port, surface_mission())
    bots = start_bots(d, port, "bots", 80, "--count", "2", "--callsign", "Bot")
    bots.finish(140, 0)
    server.finish(40, 0)
    text = bots.text()
    for n in (1, 2):
        bots.expect(rf"^Bot{n}: seat \d+, plane \d+, at tick \d+$", "a seating")
        bots.expect(rf"^Bot{n}: debrief objective: (Destroyed|Failed to destroy)", "the ground target's Destroy line")
        bots.expect(rf"^Bot{n}: The connection ended: the player left\.$", "a clean leave")
    bots.forbid(NET_BAD, "a network problem")
    bots.forbid(r"places this mission's ground target differently", "a surface digest refusal")
    for problem in surface_problems(text, ["Bot1", "Bot2"]):
        d.problem(problem)
    seen = last_surface(text)
    fire = sum(int(line[k]) for line in seen.values() for k in ("bursts", "launches", "flak"))
    if fire == 0:
        d.problem("no surface burst, launch or flak reached either bot in 80 seconds over the defended column")
    told = sum(int(line["told"]) for line in seen.values())
    if told == 0:
        d.problem("no surface unit's state (radar, rails, damage) was told to either bot")
    server.forbid(NET_BAD, "a network problem")
    log = server_log(d)
    if not re.search(r"joined as Bot1", log) or not re.search(r"joined as Bot2", log):
        d.problem("the server log does not show both bots joining")


def drive_pvp(d: Drive) -> None:
    """PvP over the defended column: Redfor defends the target, Bluefor attacks it."""
    port = d.port()
    server = start_server(
        d, port, weapons_hold(surface_mission()), mode="pvp", kill_limit=5, kill_owner="total", time_limit=2,
    )
    replays = d.work / "replays"
    replays.mkdir(exist_ok=True)
    capture = replays / "2026-10-10_1700_NET_127001.tore-capture"
    blue = start_bots(d, port, "blue", 75, "--callsign", "Blue", "--slot", "0", "--capture", capture)
    red = start_bots(d, port, "red", 75, "--callsign", "Red", "--slot", "6")
    blue.finish(140, None)
    red.finish(140, None)
    server.finish(60, None)
    blue.expect(r"^Blue: seat \d+, plane 0, at tick \d+$", "a friendly plane")
    red.expect(r"^Red: seat \d+, plane 6, at tick \d+$", "an enemy plane, open in PvP")
    blue.expect(r"^Blue: debrief objective: (Destroyed|Failed to destroy)", "Bluefor's Destroy line")
    red.expect(r"^Red: debrief objective: (Protected|Failed to protect)", "Redfor's Protect line")
    for problem in surface_problems(blue.text() + red.text(), ["Blue", "Red"]):
        d.problem(problem)
    red_line = last_surface(red.text()).get("Red")
    # A radar warning receiver shows every radar painting the plane, Blue's included (Blue's front-line M163 can
    # track Red): only Redfor's own radars painting the Red bot are wrong (slice NET2).
    if red_line and int(red_line["own"]) > 0:
        d.problem(f"Redfor's own radars painted the Red bot in {red_line['own']} frames")
    for bot in (blue, red):
        bot.forbid(NET_BAD, "a network problem")
        bot.forbid(r"places this mission's ground target differently", "a surface digest refusal")
    server.forbid(NET_BAD, "a network problem")
    # Blue's capture converts into a format 4 replay with the surface tracks (replay slice RP1's hooks).
    if not capture.exists():
        d.problem("the Blue bot wrote no capture")
        return
    run = d.run("convert", [d.app, "--convert-capture", capture], timeout=180)
    run.expect(r"^Replay: .*\.tore-replay \(\d+ frames", "the replay's line")
    replay = next(replays.glob("*.tore-replay"), None)
    if replay is None:
        d.problem("the conversion wrote no replay beside the capture")
        return
    info = d.run("info", [d.app, "--recording-info", replay], timeout=60)
    info.expect(r"^State +finished normally", "a finished replay")
    info.expect(r"^Game +.*, format 4,", "a format 4 recording")
    info.expect(r"^Ground +target QUCOL with AAA 3 and SAM 3, surface seed [1-9]", "the ground target and its seed")
    info.expect(r"^Surface +[1-9]\d* units named", "the surface units named")
    info.forbid(r"INCOMPLETE|^Problem", "damage")


def drive_redfor_ground_start(d: Drive) -> None:
    """PvP over the defended column with an enemy ground start (slice CL-R): Enemy Wing 1 parks at the Redfor field
    nearest the target at least 15 nm from it, and the Red bot that takes its lead starts there on the ground, its
    own side's field, while Blue starts in the air. The field's radars are Red's, so none of them paints the Red bot (a Blue radar may)."""
    port = d.port()
    mission = weapons_hold(surface_mission()) + "enemy-start ground auto\n"
    server = start_server(d, port, mission, mode="pvp", kill_limit=5, kill_owner="total", time_limit=2)
    blue = start_bots(d, port, "blue", 60, "--callsign", "Blue", "--slot", "0")
    red = start_bots(d, port, "red", 60, "--callsign", "Red", "--slot", "6")
    blue.finish(120, None)
    red.finish(120, None)
    server.finish(60, None)
    blue.expect(r"^Blue: seat \d+, plane 0, at tick \d+$", "a friendly plane")
    red.expect(r"^Red: seat \d+, plane 6, at tick \d+$", "Enemy Wing 1's lead, open in PvP")
    blue.expect(r"^Blue: start: airborne at \d+ ft$", "Blue's airborne start")
    red.expect(r"^Red: start: on the ground at .+, a Redfor field$", "Red's ground start at its own field")
    red.expect(r"^Red: debrief objective: (Protected|Failed to protect)", "Redfor's Protect line")
    for problem in surface_problems(blue.text() + red.text(), ["Blue", "Red"]):
        d.problem(problem)
    red_line = last_surface(red.text()).get("Red")
    if red_line and int(red_line["own"]) > 0:
        d.problem(f"Redfor's own radars painted the Red bot on its own side's field in {red_line['own']} frames")
    for bot in (blue, red):
        bot.forbid(NET_BAD, "a network problem")
        bot.forbid(r"places this mission's ground target differently", "a surface digest refusal")
    red.forbid(r"^Red: debrief .*[Ss]hot down by", "the Red bot shot down on its own field")
    server.forbid(NET_BAD, "a network problem")
    if not re.search(r"enemy wing 1 on a Redfor runway the world picks", server.text() + server_log(d)):
        d.problem("the server's mission line does not name the enemy ground start")


def drive_area_defenses(d: Drive) -> None:
    """B23: the guide's mission with no ground target and heavy AAA and SAM strengths. The server draws the
    surface seed; both bots build the same surface (one digest) with the enemy's area defenses in it."""
    port = d.port()
    mission = guide_mission() + "defenses aaa heavy sam heavy\n"
    server = start_server(d, port, mission)
    bots = start_bots(d, port, "bots", 40, "--count", "2", "--callsign", "Bot")
    bots.finish(120, 0)
    server.finish(40, 0)
    for n in (1, 2):
        bots.expect(rf"^Bot{n}: seat \d+, plane \d+, at tick \d+$", "a seating")
        bots.expect(rf"^Bot{n}: The connection ended: the player left\.$", "a clean leave")
    bots.forbid(NET_BAD, "a network problem")
    bots.forbid(r"places this mission's ground target differently", "a surface digest refusal")
    seen = last_surface(bots.text())
    for name in ("Bot1", "Bot2"):
        if name not in seen:
            d.problem(f"{name} printed no surface line")
    digests = {line["digest"] for line in seen.values()}
    if len(digests) > 1:
        d.problem(f"the bots built different surfaces: {sorted(digests)}")
    server.forbid(NET_BAD, "a network problem")


def scenarios() -> list[Scenario]:
    return [
        Scenario(
            name="net-surface-loopback", lane="net", args=[], driver=drive_loopback, uses=("server", "bot"),
            timeout=300,
        ),
        Scenario(
            name="net-surface-pvp", lane="net", args=[], driver=drive_pvp, uses=("server", "bot"), timeout=300,
        ),
        Scenario(
            name="net-area-defenses", lane="net", args=[], driver=drive_area_defenses, uses=("server", "bot"),
            timeout=300,
        ),
        Scenario(
            name="net-redfor-ground-start", lane="net", args=[], driver=drive_redfor_ground_start,
            uses=("server", "bot"), timeout=300,
        ),
    ]
