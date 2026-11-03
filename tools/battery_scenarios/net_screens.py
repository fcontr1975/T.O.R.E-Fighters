"""Lane: net. The game's screens through a rejoin and a host migration (stage K, slice K7b), in the real window.

Two windowed scenarios, each opening one window at a time (`--windows 1`):

`net-window-rejoin`: the game joins a `tore-server` with `--connect` and flies; it is killed in flight (SIGKILL, so no
Leave); the server keeps its aircraft for it; the game started again with the same data folder finds the token it kept
in `rejoin-v1.conf`, sends it, and is welcomed back in its aircraft.

`net-window-migrate`: a hosting `tore-bot` (Lead) and two pilots that stand by (`tore-bot --standby on`) fly with the game,
whose "Let my game take over hosting" switch is off (so a pilot, never the game, takes over); the hosting bot is killed
in flight. The game's HUD says "Lost contact with the host. Moving the game to Pilot1..." and then "The game moved to
Pilot1."; the pictures the script writes show the HUD for a person to look at. It needs the hosting bot of slice K9.

`net-window-migrate-smoke`: the lead's smoke test of slice K10, the shape of John's three-machine check. The game hosts in
the window, listed on a `tore-master` on this machine; two pilots that stand by join it directly, a bot joins through the
master's relay and Viper keeps its rejoin token in a file. AI weapons are held so Viper survives the retained-aircraft
check. The hosting game is killed in flight (SIGKILL); a pilot takes the game over, the relayed bot keeps its channel, and Viper, killed after the migration and started again
through the master with its token, is welcomed back into the aircraft the new host kept for it.
"""
from __future__ import annotations

import os
import json
import re
import signal
import time
from pathlib import Path

from battery import Drive, DriveError, Scenario

from battery_scenarios.net import (
    GAME_FLAGS, LOCALHOST, NET_BAD, SNAPSHOTS_AGAIN_MS, fresh_data, guide_mission, log_must, migrate_problems,
    rejoin_problems, server_log, start_master, start_server, stop_server, weapons_hold,
    query_reservation,
)

# The game flies about four seconds and leaves (only reached when it is not killed first).
REJOIN_SCRIPT = "wait 1\nwaittick 600 90\nexit\n"
# The game flies about ten seconds, then pictures of the flight every half second for seven, then leaves. The driver
# kills the host nine seconds after the game is seated, so the loss is noticed (1.5 s) just after the pictures begin.
MIGRATE_SCRIPT = (
    "wait 1\nwaittick 1200 120\n"
    + "".join(f"shot SHOTS/migrate-{i:02d}.ppm\nwait 0.5\n" for i in range(1, 15))
    + "exit\n"
)


def game_log(d: Drive) -> str:
    """The game's own log files in the data folder (`logs/tore-DATE.log`)."""
    return "\n".join(p.read_text(errors="replace") for p in sorted((d.data / "logs").glob("tore-*.log")))


def wait_log(d: Drive, pattern: str, seconds: float, after: int = 0) -> bool:
    """Waits until the game's log holds `pattern` past its first `after` characters."""
    end = time.time() + seconds * d.scale
    while time.time() < end:
        d._check_time()
        if re.search(pattern, game_log(d)[after:]):
            return True
        time.sleep(0.25)
    return False


def kill_game(d: Drive, proc) -> None:
    """Kills a windowed game hard (SIGKILL, so no Leave). Under Hyprland `tools/agent-run.sh` starts the game from the
    compositor, so killing the wrapper would leave the game flying; the game's own log file is named with its process
    id, so the game itself is killed and the wrapper ends with it (slice K10)."""
    pids = set()
    for path in (d.data / "logs").glob("tore-*.log"):
        m = re.search(r"-(\d+)-\d+\.log$", path.name)
        if not m:
            continue
        pid = int(m.group(1))
        try:
            if Path(f"/proc/{pid}/comm").read_text().strip() == "tore-app":
                pids.add(pid)
        except OSError:
            pass
    if not pids:
        # Not Linux, or the log is not there: the wrapper is the game.
        proc.popen.kill()
    for pid in pids:
        os.kill(pid, signal.SIGKILL)
    proc.stopped = True
    proc.wait(15)


def drive_rejoin_window(d: Drive) -> None:
    """The window's rejoin: killed in flight, started again, welcomed back in plane 0."""
    port = d.port()
    server = start_server(d, port, weapons_hold(guide_mission(50)), empty_timeout=90)
    script = d.work / "rejoin.txt"
    script.write_text(REJOIN_SCRIPT)
    argv = [d.app, "--connect", f"{LOCALHOST}:{port}", "--callsign", "Viper", "--slot", "0", *GAME_FLAGS,
            "--input-script", script]
    first = d.start("game", argv, window=True)
    if not wait_log(d, r"Network: seated in plane 0", 120):
        raise DriveError("the game was never seated")
    d.sleep(3)
    kept = d.data / "rejoin-v1.conf"
    if not kept.exists() or "token " not in kept.read_text():
        raise DriveError("the game kept no token in rejoin-v1.conf")
    # Killed in flight: no Leave, no goodbye.
    d.log("killing the game (SIGKILL)")
    kill_game(d, first)
    if not server.wait_for(r"Viper dropped out: the AI flies plane 0, kept for it$", 40):
        d.problem("the server did not keep plane 0 for the dropped game")
    if not server.wait_for(r"Viper \(plane 0\) left: no packet for 5 seconds$", 20):
        d.problem("the server did not see the killed game go silent (did it leave cleanly instead?)")
    d.sleep(1)
    token_text = next(line for line in kept.read_text().splitlines() if line.startswith("token "))
    reservation = query_reservation(port, token_text)
    (d.work / "reservation-before-rejoin.json").write_text(json.dumps(reservation, indent=2) + "\n")
    if (not reservation or not reservation.get("held") or reservation.get("plane") != 0
            or not 0 < reservation.get("return_seconds", 0) <= 90):
        d.problem(f"the server did not confirm the slot inside its empty-mission deadline: {reservation}")
    before = len(game_log(d))
    second = d.start("game2", argv, window=True)
    second.finish(150, 0)
    log = game_log(d)[before:]
    for pattern, what in (
        (r"Network: Welcome back, Viper: your aircraft is waiting\.", "the host's welcome"),
        (r"Network: seated in plane 0", "the aircraft taken back"),
    ):
        if not re.search(pattern, log):
            d.problem(f"the game's log lacks {what}: /{pattern}/")
    if re.search(NET_BAD + r"|does not know your rejoin token", log):
        d.problem("a network problem or a token refused in the game's log")
    server.send("end")
    server.finish(40, 0)
    log_must(
        d,
        server_log(d),
        r"Viper dropped out: the AI flies plane 0, kept for it$",
        r"Viper rejoined with its token: plane 0 is waiting$",
        forbid=r"protocol error|bad packets|\bfault\b",
    )


def drive_migrate_window(d: Drive) -> None:
    """The window's migration: the hosting bot is killed in flight and the game's HUD says so."""
    fresh_data(d)
    # The game does not stand by: a pilot takes over, and the switch's own file is what says so.
    (d.data / "network-v1.conf").write_text("tore-network 1\ncallsign Viper\nmay-host no\n")
    port = d.port()
    mission = d.work / "mission.txt"
    mission.write_text(guide_mission(5))
    shots = d.work / "shots"
    shots.mkdir(exist_ok=True)
    script = d.work / "migrate.txt"
    script.write_text(MIGRATE_SCRIPT.replace("SHOTS", str(shots)))
    host = d.start(
        "host",
        [d.bot, "--host", mission, "--port", port, "--callsign", "Lead", "--slot", "0", "--seconds", 400,
         "--players", "4", "--wait-standbys", 1],
    )
    if not host.wait_for(r"^Lead: hosting ", 60):
        raise DriveError("the hosting bot never began to host")
    pilots = d.start(
        "pilots",
        [d.bot, "--connect", f"{LOCALHOST}:{port}", "--callsign", "Pilot", "--count", "2", "--slot", "1",
         "--seconds", 120, "--standby", "on"],
    )
    game = d.start(
        "game",
        [d.app, "--connect", f"{LOCALHOST}:{port}", "--callsign", "Viper", "--slot", "3", *GAME_FLAGS,
         "--input-script", script],
        window=True,
    )
    if not host.wait_for(r"^Lead: host: mission started", 150):
        raise DriveError("the hosting bot never started the mission (no standby ready?)")
    if not wait_log(d, r"Network: seated in plane \d+", 150):
        raise DriveError("the game was never seated")
    d.sleep(9)
    d.log("killing the host (SIGKILL)")
    host.stopped = True
    host.popen.kill()
    host.wait(10)
    game.finish(150, 0)
    log = game_log(d)
    for pattern, what in (
        (r"Network: Lost contact with the host\. Moving the game to Pilot\d\.\.\.", "the loss line"),
        (r"Network: The game moved to Pilot\d\.", "the moved line"),
    ):
        if not re.search(pattern, log):
            d.problem(f"the game's log lacks {what}: /{pattern}/")
    for pattern, what in (
        (NET_BAD + r"|No other game could take over", "a network problem or a session given up"),
        (r"Host: .*took the game over", "the game taking over (its switch is off)"),
    ):
        if re.search(pattern, log):
            d.problem(f"the game's log holds {what}: /{pattern}/")
    pilots.finish(150, 0)
    pilots.expect(r"^Pilot\d: migrate: snapshots again \d+ ms", "a pilot's snapshots again")
    pictures = sorted(shots.glob("migrate-*.ppm"))
    if len(pictures) < 10:
        d.problem(f"the script wrote {len(pictures)} pictures, expected 14")


# The hosting game flies until the driver kills it; pictures every two seconds once it flies.
SMOKE_SCRIPT = (
    "wait 1\nwaittick 600 120\n"
    + "".join(f"shot SHOTS/host-{i:02d}.ppm\nwait 2\n" for i in range(1, 31))
    + "exit\n"
)


def drive_migrate_smoke(d: Drive) -> None:
    """The hosting game killed in flight, one player relayed, one retained-aircraft rejoin."""
    fresh_data(d)
    master, mport = start_master(d)
    port = d.port()
    name = "Migrate smoke"
    mission = d.work / "mission.txt"
    mission.write_text(weapons_hold(guide_mission(5)))
    shots = d.work / "shots"
    shots.mkdir(exist_ok=True)
    script = d.work / "smoke.txt"
    script.write_text(SMOKE_SCRIPT.replace("SHOTS", str(shots)))
    game = d.start(
        "game",
        [d.app, "--host", mission, "--port", port, "--name", name, "--list", "--master", f"{LOCALHOST}:{mport}",
         "--callsign", "Host", "--slot", "0", *GAME_FLAGS, "--input-script", script],
        window=True,
    )
    if not wait_log(d, r"Network: seated in plane 0", 150):
        raise DriveError("the hosting game was never seated")
    if not master.wait_for(r'^listed id=[0-9a-f]{16} from=127\.0\.0\.1:\d+ name="Migrate smoke"', 60):
        raise DriveError("the master never listed the hosted game")
    pilots = d.start(
        "pilots",
        [d.bot, "--connect", f"{LOCALHOST}:{port}", "--callsign", "Pilot", "--count", "2", "--slot", "1",
         "--seconds", 150, "--standby", "on"],
    )
    relay = d.start(
        "relay",
        [d.bot, "--master", f"{LOCALHOST}:{mport}", "--listing", name, "--path", "relay", "--callsign", "Relay",
         "--slot", "3", "--seconds", 150],
    )
    # Viper may not host but answers the host's reach tests, as a game with its switch off does (a bot with no
    # --standby runs no peers router, and a player who never reaches the candidates leaves none eligible).
    token_file = d.work / "viper.token"
    viper = d.start(
        "viper",
        [d.bot, "--connect", f"{LOCALHOST}:{port}", "--callsign", "Viper", "--slot", "4", "--seconds", 150,
         "--token-file", token_file, "--standby", "off"],
    )
    for who in (r"Pilot\d", "Relay", "Viper"):
        proc = relay if who == "Relay" else viper if who == "Viper" else pilots
        if not proc.wait_for(rf"^{who}: seat \d+, plane \d+, at tick \d+$", 120):
            raise DriveError(f"{who} was never seated")
    if not pilots.wait_for(r"^Pilot\d: standby: checkpoint \{ tick: \d+, restored: true \}$", 90):
        raise DriveError("no pilot's standby became ready")
    # Keep the close formation, but hold AI weapons so combat cannot kill
    # Viper before the token rejoin checks its retained aircraft.
    d.sleep(15)
    d.log("killing the hosting game (SIGKILL)")
    kill_game(d, game)
    if not relay.wait_for(r"^Relay: migrate: snapshots again \d+ ms", 30):
        d.problem("the relayed bot's snapshots never came again after the host was killed")
    if not viper.wait_for(r"^Viper: migrate: snapshots again \d+ ms", 30):
        d.problem("Viper's snapshots never came again after the host was killed")
    took = re.search(r"^(Pilot\d): migrate: taking the game over$", pilots.text(), re.M)
    if not took:
        raise DriveError("no pilot took the game over")
    new = took.group(1)
    d.sleep(3)
    # Viper drops out of the new host's game and comes back with its token through the master.
    d.log("killing Viper (SIGKILL)")
    viper.stopped = True
    viper.popen.kill()
    viper.wait(10)
    d.sleep(8)
    viper2 = d.start(
        "viper2",
        [d.bot, "--master", f"{LOCALHOST}:{mport}", "--listing", name, "--callsign", "Viper", "--slot", "4",
         "--seconds", 20, "--token-file", token_file],
    )
    viper2.finish(120, 0)
    # Viper asks for slot 4, Friendly Wing 2's lead. Since the guide's mission made that wing Player Choice
    # (`wing-choice friendly 2`, 2026-10-16), a late join there replaces the AI aircraft with a new plane at
    # revival placement (docs/MULTIPLAYER.md, "AI aircraft and Player Choice"), so the plane kept for Viper is the
    # one it was first seated in, not plane 4 itself.
    first = re.search(r"(?m)^Viper: seat \d+, plane (\d+), at tick \d+$", viper.text())
    kept = int(first.group(1)) if first else 4
    for problem in rejoin_problems(viper2.text(), "Viper", kept):
        d.problem(problem)
    pilots.finish(240, 0)
    relay.finish(240, 0)
    together = pilots.text() + "\n" + relay.text() + "\n" + viper.text()
    for problem in migrate_problems(together, ["Pilot1", "Pilot2", "Relay", "Viper"], None, SNAPSHOTS_AGAIN_MS * d.scale):
        d.problem(problem)
    relay.expect(r"^Relay: joined through the Internet Lobby, path relay$", "the relayed join")
    relay.forbid(r"The relay closed|No other game could take over", "a lost relay")
    if not re.search(rf"^{new}: listing: resumed from the old host's part$", pilots.text(), re.M):
        d.problem("the new host never resumed the listing from the old host's part")
    if not re.search(rf"^{new}: host: Viper .*no packet for 5 seconds", pilots.text(), re.M):
        d.problem("the new host never saw Viper drop out")
    for text in (pilots.text(), relay.text(), viper2.text()):
        if re.search(NET_BAD, text):
            d.problem("a network problem: " + re.search(NET_BAD, text).group(0))
    master.send("quit")
    master.finish(20, 0)
    master.expect(r"^relay moved listing=[0-9a-f]{16} from=127\.0\.0\.1:\d+ to=127\.0\.0\.1:\d+ channels=\d+$",
                  "the relay channel moved with the listing")
    if len(sorted(shots.glob("host-*.ppm"))) < 3:
        d.problem("the hosting game wrote fewer than 3 pictures of its flight")



def drive_master_setting(d: Drive) -> None:
    """Edit the real Options field, browse and host on its master, then restore Default."""
    master, mport = start_master(d)
    server = start_server(d, d.port(), broadcast="on", master=f"{LOCALHOST}:{mport}")
    dead_port = d.port()
    (d.data / "network-v1.conf").write_text(
        f"tore-network 1\ncallsign Viper\nport {d.port()}\nmaster {LOCALHOST}:{dead_port}\n"
        "port-forward no\n"
    )
    shots = d.work / "shots"
    shots.mkdir(exist_ok=True)

    def click(x: int, y: int) -> str:
        return f"movemenu {x} {y}\nwait 0.4\nclick\nwait 0.4\n"

    def keys(*names: str) -> str:
        return "".join(f"key {name}\n" for name in names)

    def master_field() -> str:
        return click(430, 430) + keys("Tab", "Tab", "Tab", "End")

    script = d.work / "master-setting.txt"
    script.write_text(
        "wait 15\n" + click(150, 48) + click(150, 90)
        + "wait 12\nsnapshot SHOTS/unreachable.ppm\n"
        + master_field() + keys(*(["Backspace"] * len(str(dead_port))), "b", "a", "d", "Enter")
        + "snapshot SHOTS/invalid.ppm\n"
        + keys("End", "Backspace", "Backspace", "Backspace", *str(mport))
        + "snapshot SHOTS/custom-options.ppm\n" + keys("Enter")
        + "wait 12\nsnapshot SHOTS/custom-listed.ppm\n"
        + click(80, 432) + "wait 20\nsnapshot SHOTS/custom-hosted.ppm\n"
        + keys("Escape") + "wait 1\n" + keys("Tab", "Enter") + "wait 4\n"
        + master_field() + keys("Tab", "Enter")
        + "snapshot SHOTS/default-draft.ppm\n" + keys("Escape")
        + "wait 1\nsnapshot SHOTS/default-cancelled.ppm\n"
        + master_field() + click(495, 237)
        + keys("Tab")  # Mouse focused Default; forward is next. Click OK to accept.
        + click(225, 420) + "wait 2\nsnapshot SHOTS/default-restored.ppm\nexit\n"
    )
    script.write_text(script.read_text().replace("SHOTS", str(shots)))
    # Main-menu bar coordinates vary by background; keep this UI script deterministic.
    game = d.start(
        "game", [d.app, *GAME_FLAGS, "--background", "CHOOSEV", "--input-script", script], window=True,
    )
    if not wait_log(d, "Internet Lobby: 1 game is listed on the Internet Lobby.", 100):
        d.problem("the edited master did not list the dedicated server")
    else:
        expected = f"master {LOCALHOST}:{mport}\n"
        if expected not in (d.data / "network-v1.conf").read_text():
            d.problem("the accepted custom master was not saved")
    hosted = r'^listed id=[0-9a-f]{16} from=127\.0\.0\.1:\d+ name="Viper\'s game"'
    if not master.wait_for(hosted, 60):
        d.problem("New did not register on the edited master")
    else:
        # No --master here: the saved setting must select the same master.
        browsed = d.run("saved-browse", [d.app, "--browse", "3"], timeout=30)
        browsed.expect(r'^"Viper\'s game"', "the game hosted from the edited lounge")
        browsed.expect(r'^"T\.O\.R\.E server"', "the dedicated server on the same master")
    game.finish(180, 0)
    log = game_log(d)
    for text in (
        f"The Internet Lobby at {LOCALHOST}:{dead_port} does not answer.",
        f"Asking the Internet Lobby at {LOCALHOST}:{mport} for games...",
        "Asking the Internet Lobby at master.jroverton.com:26901 for games...",
    ):
        if text not in log:
            d.problem(f"game log missing: {text}")
    if re.search(r"^master ", (d.data / "network-v1.conf").read_text(), re.M):
        d.problem("Default did not persist the public default")
    for name in ("unreachable", "invalid", "custom-options", "custom-listed", "custom-hosted",
                 "default-draft", "default-cancelled", "default-restored"):
        if not (shots / f"{name}.ppm").exists():
            d.problem(f"missing capture {name}")
    stop_server(d, server)
    master.send("quit")
    master.finish(20, 0)


def scenarios() -> list[Scenario]:
    return [
        Scenario(
            name="net-window-master-setting", lane="net", args=[], driver=drive_master_setting,
            uses=("server",), window=True, timeout=360,
            notes="Options validates and saves a private master, lists and hosts there, then restores Default",
        ),
        Scenario(
            name="net-window-rejoin", lane="net", args=[], driver=drive_rejoin_window, uses=("server",), window=True,
            timeout=360,
            notes="the game's rejoin in the window: it joins a server, is killed in flight (SIGKILL), the server keeps "
            "its aircraft, and the game started again sends the token it kept in rejoin-v1.conf and is welcomed back",
        ),
        Scenario(
            name="net-window-migrate", lane="net", args=[], driver=drive_migrate_window, uses=("bot",), window=True,
            timeout=480,
            notes="the game's HUD through a host migration: a hosting bot and two standby pilots fly with the game "
            "(its take-over switch off); the host is killed; the HUD says the game is moving and then that it moved "
            "(slice K7b; needs the hosting bot of K9)",
        ),
        Scenario(
            name="net-window-migrate-smoke", lane="net", args=[], driver=drive_migrate_smoke, uses=("bot",),
            window=True, timeout=600,
            notes="the lead's smoke test (slice K10): the game hosts, listed on a local master; it is killed in "
            "flight with AI weapons held; a standby pilot takes over, a relayed bot keeps its channel, and a bot "
            "killed after the migration "
            "rejoins its kept aircraft through the master with its token",
        ),
    ]
