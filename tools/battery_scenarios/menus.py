"""Lane: menu screens, the Quick Mission creator, loadout and GUI captures.

Headless scenarios (CPU snapshots, creator probes, mission starts through the
headless AI probe) run in parallel; the windowed ones (real flight captures at
several window sizes) go through tools/agent-run.sh and are kept few.
"""
import re
import json
import shutil
from pathlib import Path

from battery import Scenario, Step
from battery_scenarios._strips import ground_airport

AIRCRAFT = ["f18", "rafale", "f14", "a4e", "x31", "mig29", "su27", "mig21", "su25", "mig23", "su35", "f22", "f22n", "faxx"]

# Every menu state `--snapshot-state` accepts (normal mode).
NORMAL_STATES = [
    "loading", "loading-thunderbirds",
    "respawn-loadout", "respawn-aircraft", "respawn-aircraft-scrolled",
    "respawn-aircraft-filtered", "respawn-station-scrolled", "respawn-station-filtered",
    "update-available", "update-focus", "updates-off",
    "normal", "hover", "pressed", "help", "pref", "multi", "notice", "controls", "controls-keyboard",
    "controls-mouse", "controls-head", "controls-search", "controls-search-keys", "controls-inv", "graphics", "graphics-middle",
    "graphics-bottom", "graphics-scale", "graphics-shadows", "sound",
    "replays", "replays-settings", "replays-delete", "locate", "locate-importing", "locate-done",
    "direct-help", "internet-help", "lobby-help", "lobby-internet", "lobby-internet-help", "lobby-internet-settings", "lobby-internet-players",
    "direct", "direct-games", "direct-trying", "direct-refused", "direct-options",
    "internet", "internet-games", "internet-joining", "internet-options", "internet-unreachable",
    # Stage K (slice K7b): the Rejoin mark, the Host row, standby marks, Release and a reserved slot.
    "direct-rejoin", "internet-rejoin", "lobby-host-row", "lobby-standby", "lobby-release", "lobby-reserved",
    "lobby-king", "lobby-joiner", "lobby-unable", "lobby-flying", "lobby-server", "lobby-kick",
    "lobby-leave", "lobby-ready",
    # Stage F phase 2 (slice F2-L): the Settings panel's pages, the Players panel, slot locks, Watch, PvP's head line.
    "lobby-settings", "lobby-settings-revival", "lobby-settings-scoring", "lobby-settings-realism",
    "lobby-settings-joiner", "lobby-settings-pvp", "lobby-settings-flying", "lobby-players", "lobby-players-house",
    "lobby-locks", "lobby-watch", "lobby-pvp",
    # Slice J6: a relayed player selected, a player away from its aircraft.
    "lobby-relay", "lobby-away",
    # Stage L (slice L4, L5): Messages saying how a player's game differs from the host's.
    "lobby-gaps",
    # Lobby pass (slice L2): the map's arrow bars and the ready hint for the King, a joiner, PvP and co-op.
    "lobby-scroll", "lobby-scroll-top", "lobby-king-hint", "lobby-joiner-noslot-hint", "lobby-joiner-hint",
    "lobby-joiner-ready-hint", "lobby-pvp-hint", "lobby-coop-hint",
    # Lobby pass (slice L3): PvP slot colours, the side boxes in each state, the Players tint.
    "lobby-pvp-colours", "lobby-pvp-colours-red", "lobby-pvp-bluefor", "lobby-pvp-redfor", "lobby-pvp-open",
    "lobby-pvp-full", "lobby-pvp-locked", "lobby-balanced", "lobby-pvp-balanced-wait", "lobby-pvp-unable",
    # Force Start (nr-lobbyui): the King's button, and the countdown.
    "lobby-force", "lobby-force-count",
]
# Quick Mission mode states, the loadout page states and the debrief pages.
QUICK_STATES = [
    "normal", "first-load", "aircraft", "theaters", "help", "ordnance", "ordnance-tanks", "ordnance-empty", "ordnance-drag",
    "ordnance-message", "ordnance-message-long", "ordnance-laser", "lobby-ordnance-laser",
    "ordnance-pod", "ordnance-pod-yellow", "ordnance-authored-tank", "debrief", "debrief-2", "debrief-3", "debrief-4",
    "debrief-5", "debrief-success", "objectives", "ground-start", "airports",
    "ground-start-auto", "ground-target", "ground-target-last",
    "objective-1", "objective-2", "objective-3", "objective-4", "objective-5", "objective-6",
    # Stage L (slice L4): the lobby's creator and Load Ordnance with items not every player has dimmed.
    "lobby-creator-gaps", "lobby-creator-gap-notice", "lobby-creator-gap-theaters", "lobby-ordnance-gaps",
    # Lobby pass (slice L4): the mission page read-only, for a player who is not the King.
    "lobby-creator-view", "lobby-creator-view-gaps", "lobby-creator-view-click", "lobby-creator-view-changed",
    "lobby-creator-view-flying", "lobby-creator-view-locked", "lobby-creator-view-target",
    # Slice CL-R: the enemy's ground start rows and list, and the creator's Map (airborne, no target).
    "enemy-ground-start", "enemy-ground-start-auto", "enemy-airports", "map", "map-lobby",
    # Slice CL-R2: the map while it is still being built, at its widest zoom (the theater just fills the plot) and its narrowest.
    "map-building", "map-wide", "map-close",
]
THEATERS = ["BAL", "CUB", "EGY", "LFA", "FRA", "GRE", "IRA", "KURILE", "TVIET", "SPA", "APA", "PGU", "NSK", "WTA", "UKR", "VLA"]
VARIANT_THEATERS = [f"~{code}{n}" for code in ("UKR", "VLA") for n in range(1, 9)] + ["~UKRF", "~VLAF", "~WTAF"]


def read_ppm(path: Path):
    data = path.read_bytes()
    parts = data.split(b"\n", 3)
    if len(parts) < 4 or parts[0] != b"P6":
        return None
    width, height = (int(v) for v in parts[1].split())
    return width, height, parts[3]


def picture_problems(output: str, marker: str, size=None, min_colors=24, max_flat=0.985) -> list[str]:
    """Reads the picture the run wrote (its path is printed after `marker`) and
    reports a missing, mis-sized, blank or single-colour picture."""
    m = re.search(marker + r"\s*(\S+)", output)
    if not m:
        return [f"no line naming the picture ({marker})"]
    path = Path(m.group(1))
    if not path.exists():
        return [f"picture not written: {path}"]
    ppm = read_ppm(path)
    if ppm is None:
        return [f"not a P6 picture: {path}"]
    width, height, pixels = ppm
    problems = []
    if len(pixels) != width * height * 3:
        problems.append(f"picture {width}x{height} has {len(pixels)} bytes")
        return problems
    if size and (width, height) != size:
        problems.append(f"picture is {width}x{height}, expected {size[0]}x{size[1]}")
    colors = {}
    step = max(1, (width * height) // 20000)
    for i in range(0, width * height, step):
        px = pixels[i * 3 : i * 3 + 3]
        colors[px] = colors.get(px, 0) + 1
    total = sum(colors.values())
    if len(colors) < min_colors:
        problems.append(f"picture has only {len(colors)} colours (blank or flat)")
    if max(colors.values()) / total > max_flat:
        problems.append("picture is almost one flat colour")
    return problems


def frame_stats(path: Path) -> dict:
    """Brightness and flatness numbers for a captured frame: mean luminance
    (0 to 255), the share of near-black pixels, and the share taken by the
    single most common colour (each channel cut to 4 bits). Sampled, so it is
    quick on a 1920 by 1080 frame."""
    ppm = read_ppm(path)
    if ppm is None:
        raise ValueError(f"not a P6 picture: {path}")
    width, height, pixels = ppm
    count = width * height
    step = max(1, count // 30000)
    total = black = n = 0
    colours: dict = {}
    for i in range(0, count, step):
        r, g, b = pixels[i * 3], pixels[i * 3 + 1], pixels[i * 3 + 2]
        lum = (299 * r + 587 * g + 114 * b) // 1000
        total += lum
        black += lum < 8
        key = (r >> 4, g >> 4, b >> 4)
        colours[key] = colours.get(key, 0) + 1
        n += 1
    # The upper and lower thirds separately, for a missing sky or missing ground.
    def band(lo, hi):
        rows = range(int(height * lo), int(height * hi), max(1, height // 60))
        vals = []
        for y in rows:
            for x in range(0, width, max(1, width // 60)):
                j = (y * width + x) * 3
                vals.append((299 * pixels[j] + 587 * pixels[j + 1] + 114 * pixels[j + 2]) // 1000)
        return sum(vals) / len(vals)
    return {
        "mean": total / n,
        "black": black / n,
        "top_colour": max(colours.values()) / n,
        "colours": len(colours),
        "upper": band(0.05, 0.35),
        "lower": band(0.65, 0.95),
    }


# Weather choices by number: clear, cloudy, foggy, dawn, sunset, night.
WEATHER_NAMES = ["clear", "cloudy", "foggy", "dawn", "sunset", "night"]


def scene_problems(condition: int, path_marker=r"Scene capture:"):
    """A check for a captured flight scene under weather choice `condition`:
    not blank, not a single colour, night dark, day bright, ground and sky
    both drawn (the frame is not black in a whole band)."""

    def check(output: str) -> list[str]:
        m = re.search(path_marker + r"\s*(\S+)", output)
        if not m or not Path(m.group(1)).exists():
            return ["no capture written"]
        st = frame_stats(Path(m.group(1)))
        problems = []
        name = WEATHER_NAMES[condition]
        if st["black"] > 0.5:
            problems.append(f"{name}: {st['black']:.0%} of the frame is black")
        if st["top_colour"] > 0.6 and condition != 2:
            problems.append(f"{name}: one colour fills {st['top_colour']:.0%} of the frame")
        if st["top_colour"] > 0.85:
            problems.append(f"{name}: one colour fills {st['top_colour']:.0%} of the frame")
        if st["colours"] < 60:
            problems.append(f"{name}: only {st['colours']} colours")
        if condition in (0, 1, 2) and st["mean"] < 70:
            problems.append(f"{name}: day frame is dark (mean {st['mean']:.0f})")
        if condition == 5 and st["mean"] > 70:
            problems.append(f"night: frame is bright (mean {st['mean']:.0f})")
        if condition in (3, 4) and not 15 < st["mean"] < 200:
            problems.append(f"{name}: mean brightness {st['mean']:.0f} is out of range")
        if st["upper"] < 3 and st["lower"] < 3:
            problems.append(f"{name}: upper and lower bands both black")
        return problems

    return check


def menu_picture(size=(640, 480)):
    # The dark input, graphics, replay and locate screens use few colours.
    return lambda output: picture_problems(output, r"(?:Menu|Locate) preview:", size, min_colors=8)


def replays_bar_picture(offset=0):
    """The shared 'l' in Multi and Replays must have identical ink placement.

    Compare only the blue letter core, so the artwork and hover fill behind
    it cannot hide a baseline regression. Coordinates are on the 640x480
    canvas; all five backgrounds use one of the three bar origins.
    """
    def check(output):
        problems = menu_picture()(output)
        if problems:
            return problems
        path = Path(re.search(r"Menu preview:\s*(\S+)", output).group(1))
        width, _, pixels = read_ppm(path)

        def ink(left):
            return {(x, y) for y in range(38, 57) for x in range(5)
                    if pixels[(y * width + left + x) * 3] < 80
                    and pixels[(y * width + left + x) * 3 + 2] > 100}

        multi, replays = ink(160 + offset), ink(210 + offset)
        if not multi or not replays:
            return ["Multi or Replays letter core is missing"]
        if multi != replays:
            return ["Replays baseline or letter weight differs from Multi"]
        return []

    return check


def map_plot_filled():
    """The creator's Map snapshot is a menu picture whose plot (x 12 to 470,
    y 30 to 420) has no void: the planning map's cell for "off the theater" is
    (18, 18, 18), and the map never zooms or pans far enough to show it
    (slice CL-R2, John 2026-10-11: "we don't see black space around it")."""
    base = menu_picture()

    def check(output: str) -> list[str]:
        problems = base(output)
        m = re.search(r"Menu preview:\s*(\S+)", output)
        if not m or not Path(m.group(1)).exists():
            return problems + ["no map picture written"]
        data = Path(m.group(1)).read_bytes()
        header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
        if not header:
            return problems + ["the map picture is not a binary PPM"]
        width, height = int(header.group(1)), int(header.group(2))
        pixels = data[header.end():]
        void = 0
        for y in range(30, 420):
            row = (y * width + 12) * 3
            for x in range(458):
                j = row + x * 3
                if pixels[j] == 18 and pixels[j + 1] == 18 and pixels[j + 2] == 18:
                    void += 1
        if void > 50:
            problems.append(f"{void} void pixels show in the map's plot")
        return problems

    return check


def ordnance_notes(warning: bool, helper: bool, lobby: bool = False):
    """The Load Ordnance page's laser warning (dim amber, in the message band) and sensor helper line
    (light blue-grey, below the Select Plane and Fly buttons since 2026-10-16), found by their exact
    ink: the tint times the font's own colour (slice BQ, B24; wording and colours the lead's defaults)."""
    base = menu_picture()
    amber, grey = (178, 126, 47), (189, 208, 215)

    def count(pixels: bytes, width: int, rows: range, ink) -> int:
        found = 0
        for y in rows:
            row = y * width * 3
            for x in range(width):
                j = row + x * 3
                if (pixels[j], pixels[j + 1], pixels[j + 2]) == ink:
                    found += 1
        return found

    def check(output: str) -> list[str]:
        problems = base(output)
        m = re.search(r"Menu preview:\s*(\S+)", output)
        if not m or not Path(m.group(1)).exists():
            return problems + ["no ordnance picture written"]
        data = Path(m.group(1)).read_bytes()
        header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
        if not header:
            return problems + ["the ordnance picture is not a binary PPM"]
        width, pixels = int(header.group(1)), data[header.end():]
        low = range(64, 86) if lobby else range(335, 355)
        got_warning = count(pixels, width, low, amber) > 30
        got_helper = count(pixels, width, range(458, 476), grey) > 30
        if got_warning != warning:
            problems.append(f"laser warning shown={got_warning}, expected {warning}")
        if got_helper != helper:
            problems.append(f"sensor helper shown={got_helper}, expected {helper}")
        return problems

    return check


def flight_picture(size):
    return lambda output: picture_problems(output, r"Scene capture:", size, min_colors=200)


def mission_brief_picture(output):
    # The opaque brief panel must survive automatic creator loading. A cockpit
    # screenshot is a failure even if it is a perfectly good flight picture.
    problems = picture_problems(output, r"Scene capture:", min_colors=8)
    match = re.search(r"Scene capture:\s*(\S+)", output)
    ppm = read_ppm(Path(match.group(1))) if match else None
    if ppm:
        width, height, pixels = ppm
        step = max(1, width * height // 20000)
        samples = [pixels[i * 3:i * 3 + 3] for i in range(0, width * height, step)]
        panel = sum(all(abs(c - expected) <= 2 for c, expected in zip(rgb, (58, 63, 65)))
                    for rgb in samples)
        if panel < len(samples) // 4:
            problems.append("Mission brief panel is missing from the flight capture")
    return problems


def flown_inventory(choice: str):
    """A Quick Mission launched with stores taken off must hold exactly what the
    Load Ordnance page left: every count zero and nothing listed for `none`, only
    the internal gun for `guns`."""

    def check(output: str) -> list[str]:
        m = re.search(r"Quick Mission launch:.* ammo=\[([0-9, ]*)\] listed=\[(.*)\]$", output, re.M)
        if not m:
            return ["no Quick Mission launch line with ammo and listed weapons"]
        ammo = [int(v) for v in m.group(1).split(",") if v.strip()]
        listed = m.group(2)
        problems = []
        if choice == "none":
            if any(ammo):
                problems.append(f"stores came back after removing them all: {ammo}")
            if listed.strip():
                problems.append(f"weapons window lists {listed} for an empty aircraft")
        else:
            if not ammo or ammo[0] == 0 or any(ammo[1:]):
                problems.append(f"gun only expected, ammo is {ammo}")
            if listed.count("(") != 1 or "true" not in listed:
                problems.append(f"weapons window should list only the selected gun: {listed}")
        return problems

    return check


AIRPORT_LINE = re.compile(r"^airport-allegiance: (\S+) (blue|red) (offered (\d+) default|none:)", re.M)
BASE_THEATERS = ("APA", "BAL", "CUB", "EGY", "FRA", "GRE", "IRA", "KURILE", "LFA", "NSK", "PGU", "SPA",
                 "TVIET", "UKR", "VLA", "WTA")


def airport_allegiance_problems(output: str) -> list[str]:
    """Every base theater has a line for each side, and only North Vietnam
    has no Blue field (slice AL1; the Falklands' San Carlos, a STOL strip, is
    a Blue ground start since slice CL-AP, John 2026-10-11)."""
    found = {(m.group(1), m.group(2)): m.group(3) for m in AIRPORT_LINE.finditer(output)}
    problems = []
    for theater in BASE_THEATERS:
        for side in ("blue", "red"):
            line = found.get((theater, side))
            if line is None:
                problems.append(f"no {side} line for {theater}")
                continue
            none = line == "none:"
            if none != (side == "blue" and theater == "TVIET"):
                problems.append(f"{theater} {side}: {line}")
    return problems


def no_ordnance_leak(output: str) -> list[str]:
    """The creator probe must report every removed-store case and no problems."""
    problems = []
    if "removed stores passed" not in output:
        problems.append("removed-store check did not run")
    if re.search(r"PROBLEM", output):
        problems.append("creator matrix listed problems")
    return problems


def wing_loadout_recording_problems(work: Path, output: str) -> list[str]:
    """Check the recorded tick-zero external inventory, independently of setup."""
    problems = []
    for label in ("before", "after", "ground"):
        path = work / "wing-loadout" / f"{label}.toreplay-log" / "log.jsonl"
        if not path.is_file():
            problems.append(f"missing {label} recording export")
            continue
        rows = {i: [] for i in range(4)}
        for line in path.read_text().splitlines():
            event = json.loads(line)
            if event.get("kind") == "stores.station" and event.get("tick") == 0:
                fields = event["fields"]
                if fields.get("items", 0) and event.get("subject") in rows:
                    rows[event["subject"]].append((fields["hardpoint"], fields.get("shape", fields["look"]), fields["items"]))
        expected = [(6, "MK82.SH", 2), (8, "SIDEM.SH", 2)]
        if rows[0] != expected:
            problems.append(f"{label} player stores differ: {rows[0]}")
        wing = [(6, "MAVRK.SH", 4), (7, "AMRAM.SH", 2), (8, "SIDEM.SH", 2), (5, "tank", 1)] if label == "before" else expected
        for plane in range(1, 4):
            if rows[plane] != wing:
                problems.append(f"{label} plane {plane} stores differ from expected policy: {rows[plane]}")
    return problems


def frame_log_check(work: Path, output: str) -> list[str]:
    logs = sorted((work / "frame-logs").glob("*.log"))
    if len(logs) != 1:
        return [f"expected one diagnostic session, got {len(logs)}"]
    text = logs[0].read_text()
    problems = []
    markers = ["Frame log enabled: threshold_ms=2.000", "Frame log enabled: threshold_ms=1.000",
               "Session system:", "Slow frame:", "Frame log disabled",
               "Frame log enabled: threshold_ms=50.000"]
    at = 0
    for marker in markers:
        found = text.find(marker, at)
        if found < 0:
            problems.append(f"missing ordered frame-log marker: {marker}")
        else:
            at = found + len(marker)
    for marker in ("screen=flight", "role=single", "compression=1", "gpu_previous_ms=",
                   "simulation:", "submit:", "acquire:", "present:", "mirror_views:"):
        if marker not in text:
            problems.append(f"missing frame-log field: {marker}")
    if "Slow frame:" in text.split("Frame log disabled")[-1].split("Frame log enabled:")[0]:
        problems.append("slow-frame logging continued while disabled")
    return problems


def scenarios() -> list[Scenario]:
    out: list[Scenario] = []
    out.append(Scenario(
        name="menus-window-frame-log", lane="menus", window=True,
        args=["--free-flight", "--no-audio", "--window-size", "960x720", "--frame-log", "1",
              "--input-script", str(Path(__file__).parent / "scripts" / "frame-log.txt")],
        env={"TORE_FRAME_LOG": "2", "TORE_LOG_DIR": "{work}/frame-logs", "TORE_SCRIPT_OUT": "{work}"},
        timeout=180, expect=[r"Input script: exit"],
        outputs=["debug-frame-log-off.ppm", "debug-frame-log-on.ppm"],
        check_work=frame_log_check,
        notes="CLI overrides environment; Escape Debug toggles slow-frame logging off and on through normal key handlers, with real session records.",
    ))
    out.append(Scenario(
        name="qmc-wing-loadout", lane="menus",
        args=["--validate-wing-loadout", "{work}/wing-loadout", "--no-audio"],
        expect=[r"qmc wing loadout: four F16 aircraft match, airborne, ground and restart passed", r"qmc wing loadout: one tank matches", r"qmc wing loadout: wingtip AIM-120 matches, restarts and passes the lobby check", r"qmc wing loadout: cheat Boulder carriage matches", r"qmc wing loadout: F-4B gun pod matches on 6 wingman starts, rounds and muzzle, airborne and restart passed"],
        outputs=[f"wing-loadout/{label}.toreplay" for label in ("before", "after", "ground")],
        then=[Step(["--recording-log", f"{{work}}/wing-loadout/{label}.toreplay"]) for label in ("before", "after", "ground")],
        check_work=wing_loadout_recording_problems,
        notes="QMC four-ship F-16 custom Mk82/AIM9 load, removed tanks and reduced fuel, recorded before/after at start; airborne, ground and restart equality; the F-4B's centreline gun pod shared with its wingmen (rounds, muzzle, restart).",
    ))
    out.append(Scenario(
        name="menus-derived-stations", lane="menus",
        args=["--validate-ordnance", "--no-audio"],
        expect=[
            r"derived stations F15.PT: 3 extra tanks, 12078 lb fuel, 1296 lb shells; feed, jettison, save and old load passed",
            r"derived stations F16C.PT: 2 extra tanks, 4884 lb fuel, 524 lb shells; feed, jettison, save and old load passed",
            r"derived stations F22.PT: 2 extra tanks, 7920 lb fuel, 850 lb shells; feed, jettison, save and old load passed",
            r"derived stations F22N.PT: 2 extra tanks, 7920 lb fuel, 850 lb shells; feed, jettison, save and old load passed",
            r"derived stations MIG29.PT: 1 extra tanks, 2614 lb fuel, 281 lb shells; feed, jettison, save and old load passed",
            r"derived stations RAFALE.PT: 1 extra tanks, 3485 lb fuel, 374 lb shells; feed, jettison, save and old load passed",
            r"F-16C wingtip AIM-120: two a load \(one a tip\), saved, restored and fired guided from each tip edge; pair-sized editing checked",
            r"derived weapons F15.PT row 6: MK82.JT:3, MK84.JT:2, .*GBU12.JT:3, .*GBU38.JT:3",
            r"(?m)derived weapons F22.PT row 8: AIM9X.JT:4, AIM120.JT:4, AIM9M.JT:4, AIM9B.JT:4$",
            r"gun pod F4B.PT: row 3 takes one SUU-16 in place of AIM9B.JT x4; weight, 600 logical units \(1,200 rounds\), Tab, Guns only, save, lobby rule, firing from the pod's nose and jettison passed",
        ],
        notes="Derived Eagle, Falcon, Raptor, Fulcrum-C and Rafale C rows through normal selection, capacity, fuel, jettison, mission and profile saves, and old-load restoration; the F-4B's centreline SUU-16 gun pod (weight, rounds, Tab, Guns only, save, lobby rule, fired from its nose, jettisoned).",
    ))
    out.append(
        Scenario(
            name="menus-ordnance-store-pairs",
            lane="menus",
            args=["--validate-ordnance", "--no-audio"],
            expect=[
                r"store-pair maximum: F18.PT row=5 AGM84A.JT 1->2",
                r"store-pair maximum: F15.PT row=4 MK84.JT 3->2",
                r"store-pair maximum: F22.PT row=7 MICA.JT 3->2",
                r"store-pair overweight save: .*non-interactive start refused with existing weight message",
                r"store-pair maximum: F18.PT row=6 AGM84A.JT 3->2",
                r"store-pair maximum: F18.PT row=7 AGM84A.JT 3->2",
                r"store-pair maximum: F18.PT row=7 AS16.JT 1->2",
                r"store-pair audit: F16C.PT defaults, maxima, steps, saved loads and lobby checked",
                r"ordnance audit: .* passed",
            ],
            notes="Issue 6: complete imported roster of even paired maxima, pair-sized edits, human/AI defaults, old odd save migration and strict lobby validation. Includes Harpoon 3 to 2 and Kickback 1 to 2.",
        )
    )
    out.append(
        Scenario(
            name="menus-ordnance-availability",
            lane="menus",
            args=["--validate-ordnance", "--no-audio"],
            expect=[r"ordnance audit: 36 aircraft, 14 recovered weapon stations, 3411 weapon placements, 180 tank placements, 78 weapon and 9 tank records; passed"],
            notes="All reviewed source stations, compatible supported weapons/tanks, empty defaults and accepted-load restoration. The audit fails when a supported weapon or tank record is missing or a pinned count moves (ordnance_audit.rs).",
        )
    )
    out.append(
        Scenario(
            name="menus-tanks-f14",
            lane="menus",
            args=["--validate-tanks", "--aircraft", "f14", "--no-audio"],
            expect=[
                r"F14 tank selection 0: internal 15741 external 0 shells 0 .*restart passed",
                r"F14 saved single tank: 1 to 2, external 3300 shells 396; pair editing and migration passed",
                r"F14 tank selection 2: internal 15741 external 3300 shells 396 .*restart passed",
                r"F14 tanks: full selection, removal, empty shells, old defaults and accepted restart passed",
            ],
            notes="F14 tank pairs: 0 or 2, full mass and fuel, pair-sized editing, old single migration and accepted-load restart without a display.",
        )
    )
    out.append(
        Scenario(
            name="menus-tanks-refuses-other-aircraft",
            lane="menus",
            args=["--validate-tanks", "--aircraft", "f18", "--no-audio"],
            timeout=120,
            expect_exit=1,
            expect=[r"--validate-tanks probes the F-14 tank contract only, not --aircraft F18.PT"],
            forbid=[r"F14 tank selection"],
            notes="The tank probe is the F-14's reviewed contract: another --aircraft is refused instead of silently probing the F-14.",
        )
    )
    out.append(
        Scenario(
            name="menus-validate-creator",
            lane="menus",
            args=["--validate-creator", "--no-audio"],
            timeout=3600,
            expect=[
                r"creator matrix: \d+ setups started",
                r"removed stores passed",
                r"station transfer SU35\.PT AAML\.JT: row 5 \(paired=false\) -> row 6 \(paired=true, capacity=2\): \[1, 0\] -> \[1, 0\], moved=0; passed",
            ],
            forbid=[r"PROBLEM"],
            check=no_ordnance_leak,
            notes="Loadouts for every aircraft incl. removed stores and exact transfer conservation, including the Su-35 single-to-pair refusal, then the whole Quick Mission creator matrix.",
        )
    )
    out.append(
        Scenario(
            name="menus-validate-text",
            lane="menus",
            args=["--validate-text", "--no-audio"],
            timeout=300,
            expect=[r"non-ASCII text: KURILE\.MM: name .?Ber\u00ebzovka", r"0 problems"],
            forbid=[r"\ufffd", r"PROBLEM"],
            notes="Every imported string decodes without U+FFFD and is drawable in the original fonts.",
        )
    )
    # The ILS at every airport: datum against the runway plane, the ideal path
    # reading zero with the right signs, 52 ft over the threshold (docs/testing/ils.md).
    out.append(
        Scenario(
            name="menus-validate-ils",
            lane="menus",
            args=["--validate-ils", "--no-audio"],
            timeout=300,
            expect=[r"ils-survey: 16 theaters, \d+ runway ends, glide 3 degrees, 0 problems", r"crossing_ft=52\.4"],
            forbid=[r"PROBLEM"],
            notes="Every airport of every base theater: ILS datum on the runway surface, ideal path reads zero.",
        )
    )
    for code in ("~UKR1", "~BAL3", "~EGY5", "~FRAF"):
        out.append(
            Scenario(
                name=f"menus-validate-ils-{code.strip('~').lower()}-variant",
                lane="menus",
                args=["--validate-ils", "--theater", code, "--no-audio"],
                timeout=300,
                expect=[r"ils-survey: 17 theaters, \d+ runway ends, glide 3 degrees, 0 problems"],
                forbid=[r"PROBLEM"],
            )
        )
    # Airport allegiance (slice AL1): every base theater for Blue and for
    # Redfor, the ground-start fields offered and checked against the world.
    out.append(
        Scenario(
            name="airport-allegiance",
            lane="menus",
            args=["--airport-allegiance", "--no-audio"],
            timeout=600,
            expect=[r"airport-allegiance: 16 theaters, blue none: TVIET, red none: -, 0 problems"],
            forbid=[r"PROBLEM"],
            check=airport_allegiance_problems,
            notes="Each theater's ground-start fields for Blue and for Redfor: none of the other side's, the default "
                  "one of them, auto and AI homes the same; North Vietnam has none for Blue.",
        )
    )
    out.append(Scenario(
        name="merged-airport-audit", lane="menus",
        args=["--airfield-audit", "--merged", "--no-audio"], timeout=600,
        expect=[r"merged-airport: fields=84 problems=0"],
        forbid=[r"flat=false|inside=false|four_slots=false|alias=false"],
        notes="Every paired theater field has one long runway, valid ground slots and legacy-ID redirects."))
    # The redrawn airports for the AI (slice CL-AP, John 2026-10-11): every
    # AI ground route of every runway object of every layout on the drawn
    # pavement, every runway end reachable from parking, every runway line
    # (the ILS) on a drawn runway; and every Quick Mission template aircraft
    # parked on a redrawn field on its pavement (docs/formats/redrawn-airports.md).
    out.append(
        Scenario(
            name="airfield-taxi-audit",
            lane="menus",
            args=["--airfield-audit", "--no-audio"],
            timeout=600,
            expect=[r"airfield-audit: 75 theaters, \d+ airfields, 1028 runway objects, \d+ routes, 0 problems"],
            forbid=[r"PROBLEM"],
            notes="AI taxi routes, runway-end reachability and runway lines at every redrawn airport of every layout.",
        )
    )
    out.append(
        Scenario(
            name="airfield-parked-audit",
            lane="menus",
            args=["--airfield-audit", "--parked", "--no-audio"],
            timeout=600,
            expect=[r"airfield-audit: parked 108 templates, \d+ aircraft on redrawn fields, 0 problems"],
            forbid=[r"PROBLEM"],
            notes="Every Quick Mission template aircraft parked on a redrawn airfield stands on its pavement.",
        )
    )
    out.append(Scenario(name="menus-validate-maps", lane="menus", args=["--validate-maps", "--no-audio"], timeout=600, expect=[r"Validated 75 retail map layouts"]))
    out.append(Scenario(name="menus-validate-weather", lane="menus", args=["--validate-weather", "--no-audio"], timeout=900, expect=[r"Weather sources validated"]))
    # `--combat-smoke` fails for the other thirteen aircraft (a stale radar-off
    # check since the missile guidance rework); see docs/testing/lane-menus.md.
    out.append(Scenario(name="menus-combat-smoke-mig29", lane="menus", args=["--aircraft", "mig29", "--combat-smoke", "--no-audio"], timeout=600, expect=[r"PASS"], forbid=[r"FAIL"]))

    # CPU snapshots of every menu state.
    for state in NORMAL_STATES:
        out.append(
            Scenario(
                name=f"menus-snap-{state}",
                lane="menus",
                args=["--snapshot", "{work}/shot.ppm", "--snapshot-state", state, "--no-audio"],
                timeout=120,
                expect=[r"(Menu|Locate) preview:"],
                check=menu_picture(),
            )
        )
    for background, offset in (("CHOOSEV", 0), ("CHOOSEU", 0), ("CHOOSEM", 0),
                               ("CHOOSEAC", -6), ("CHOOSE3", 109)):
        for state in ("normal", "bar-multi-hover", "bar-replays-hover",
                      "bar-replays-pressed", "bar-replays-focus", "multi"):
            out.append(Scenario(
                name=f"menus-replays-bar-{background.lower()}-{state}",
                lane="menus",
                args=["--background", background, "--snapshot", "{work}/shot.ppm",
                      "--snapshot-state", state, "--no-audio"],
                timeout=120,
                expect=[r"Menu preview:"],
                check=replays_bar_picture(offset),
            ))
    for background in ("CHOOSEV", "CHOOSEU", "CHOOSEM", "CHOOSEAC", "CHOOSE3"):
        for state in ("update-available", "update-focus"):
            out.append(Scenario(
                name=f"menus-updates-bar-{background.lower()}-{state}",
                lane="menus",
                args=["--background", background, "--snapshot", "{work}/shot.ppm",
                      "--snapshot-state", state, "--no-audio"],
                timeout=120,
                expect=[r"Menu preview:"],
                check=menu_picture(),
            ))
    for state in QUICK_STATES:
        out.append(
            Scenario(
                name=f"menus-snap-quick-{state}",
                lane="menus",
                args=["--quick-mission", "--snapshot", "{work}/shot.ppm", "--snapshot-state", state, "--no-audio"],
                timeout=120,
                expect=[r"(Menu|Locate) preview:"],
                check=map_plot_filled() if state.startswith("map") else menu_picture(),
            )
        )
    # Every creator popup (fields 3 to 36), with a ground start chosen so the
    # airport row exists (Simferopol: airport 1, Zaporizhzhya, is a Redfor
    # field since slice AL1 and no Blue ground start). The enemy's Start and
    # Airport (35 and 36, slice CL-R) are opened with Enemy Wing 1 parked at
    # Donets'k (airport 5), so the Enemy airport row exists too.
    for field in range(3, 37):
        enemy = ["--enemy-ground-start", "5"] if field >= 35 else []
        out.append(
            Scenario(
                name=f"menus-snap-quick-field-{field}",
                lane="menus",
                args=["--quick-mission", "--ground-start", "2", *enemy, "--snapshot", "{work}/shot.ppm", "--snapshot-state", f"field-{field}", "--no-audio"],
                timeout=120,
                expect=[r"Menu preview:"],
                check=menu_picture(),
            )
        )
    out.append(
        Scenario(
            name="menus-snap-quick-target-ground-distances", lane="menus",
            args=["--theater", "UKR", "--aircraft", "f18", "--ground-start", "2",
                  "--ground-target", "QUCITY", "--defenses", "0", "0", "--surface-seed", "7",
                  "--separation", "300", "--snapshot", "{work}/shot.ppm",
                  "--snapshot-state", "field-17", "--no-audio"],
            timeout=180,
            expect=[r"quick-snapshot: start ground enabled-distances 1,2,5,10,20,50,75,100,150,200,300$", r"Menu preview:",
                    r"creator map: BLUE START: ODESA at"],
            check=menu_picture(),
            then=[Step(args=("--theater", "UKR", "--aircraft", "f18", "--launch-quick-mission",
                             "--ground-start", "2", "--ground-target", "QUCITY", "--defenses", "0", "0",
                             "--surface-seed", "7", "--separation", "300", "--smoke-test", "--no-audio"),
                       window=True, timeout=180),
                  Step(args=("--theater", "UKR", "--aircraft", "f18", "--ground-start", "9",
                             "--ground-target", "QUCITY", "--defenses", "0", "0", "--surface-seed", "7",
                             "--snapshot", "{work}/explicit-airport.ppm", "--snapshot-state", "map", "--no-audio"), timeout=180)],
            notes="An explicit ground-start snapshot applies its runway after its target. All distances stay selectable, and 300 nm launches with the target's ground-start override.",
        )
    )
    # The creator's Map (slice CL-R) on the Ukraine with a ground target, a
    # Blue ground start at L'viv (airport 8) and Enemy Wing 1 parked at
    # Donets'k (airport 5): both starts are named and the objective is an
    # area, built from the mission the creator would fly.
    for state in ("map", "map-lobby", "map-wide", "map-close"):
        out.append(
            Scenario(
                name=f"menus-snap-quick-{state}-ukr-target-ground",
                lane="menus",
                args=["--theater", "UKR", "--quick-mission", "--ground-start", "8", "--enemy-ground-start", "5",
                      "--ground-target", "QUCITY", "--defenses", "2", "2", "--surface-seed", "12345",
                      "--snapshot", "{work}/shot.ppm", "--snapshot-state", state, "--no-audio"],
                timeout=180,
                expect=[r"creator map: BLUE START: L'VIV at", r"creator map: RED START: DONETS'K at",
                        r"creator map: objective area x=\d+ z=\d+ radius=30380", r"Menu preview:"],
                forbid=[r"the map did not open"],
                check=map_plot_filled(),
            )
        )
    # The in-flight map's mission markup (slice CL-MAP): windowless and no GPU.
    out.append(
        Scenario(
            name="menus-flight-map-preview",
            lane="menus",
            args=["--flight-map-preview", "{work}", "--no-audio"],
            timeout=300,
            expect=[r"BLUE START at \d+, \d+ nm", r"RED START at \d+, \d+ nm", r"objective Some\(",
                    *[rf"Flight map preview: \S+/{name}\.png \(void pixels \d+\)"
                      for name in ("follow", "wide", "objective", "close", "scrolled", "red", "layer-off", "off-map")]],
            outputs=["follow.png", "wide.png", "objective.png", "off-map.png"],
        )
    )
    out.append(
        Scenario(
            name="menus-flight-map-mission-brief",
            lane="menus",
            args=["--flight-map-preview", "{work}", "brief", "--no-audio"],
            timeout=300,
            expect=[rf"Mission brief preview: \S+/{kind}-{page}\.png"
                    for kind in ("single", "multiplayer")
                    for page in ("map", "mission", "mission-bottom", "respawn-brief")],
            outputs=["single-map.png", "single-mission.png", "multiplayer-map.png",
                     "multiplayer-mission.png", "multiplayer-mission-bottom.png", "multiplayer-respawn-brief.png"],
        )
    )
    out.append(
        Scenario(
            name="menus-snap-quick-unknown-state-refused",
            lane="menus",
            args=["--quick-mission", "--snapshot", "{work}/shot.ppm", "--snapshot-state", "bogus", "--no-audio"],
            timeout=120,
            expect_exit=1,
            expect=[r"snapshot states: normal, aircraft, objectives"],
        )
    )
    # The creator and the loadout page for every aircraft; the loadout page
    # is where taking stores off is done.
    for a in AIRCRAFT:
        for state in ("normal", "aircraft", "ordnance", "ordnance-empty"):
            out.append(
                Scenario(
                    name=f"menus-snap-quick-{state}-{a}",
                    lane="menus",
                    args=["--aircraft", a, "--quick-mission", "--snapshot", "{work}/shot.ppm", "--snapshot-state", state, "--no-audio"],
                    timeout=120,
                    expect=[r"(Menu|Locate) preview:"],
                    check=menu_picture(),
                )
            )
    # The ordnance page's laser warning and sensor helper (slice BQ, B24): a laser missile loaded on
    # the F/A-18 (pod stations, no sensor), the Su-25 (built-in laser) and the F-16C (fixed Pave Spike).
    for a, state, warning, helper, lobby in (
        ("f18", "ordnance", False, True, False),
        ("f18", "ordnance-laser", True, True, False),
        ("f18", "lobby-ordnance-laser", True, True, True),
        ("su25", "ordnance-laser", False, True, False),
        ("f16c", "ordnance-laser", False, True, False),
        ("mig29", "ordnance", False, True, False),
    ):
        out.append(
            Scenario(
                name=f"menus-snap-quick-{state}-notes-{a}",
                lane="menus",
                args=["--aircraft", a, "--quick-mission", "--snapshot", "{work}/shot.ppm", "--snapshot-state", state, "--no-audio"],
                timeout=120,
                expect=[r"Menu preview:"],
                check=ordnance_notes(warning, helper, lobby),
            )
        )
    # The page's authored tank graphic and the pod station's blink phases (John, 2026-10-16), on the
    # F-16C, whose inner wet pair takes the 370 gallon tank and whose pod station blinks.
    for a, state in (
        ("f16c", "ordnance-authored-tank"),
        ("f16c", "ordnance-tip-amraam"),
        # The Eagle's wing pylons take an AIM-120 in place of a tank (John, 2026-10-20).
        ("f15", "ordnance-wing-amraam"),
        ("f16c", "ordnance-pod"),
        ("f16c", "ordnance-pod-yellow"),
        # The F-4B's centreline gun pod (John, 2026-10-16), in place of its four Sidewinders.
        ("f4b", "ordnance-gun-pod"),
    ):
        out.append(
            Scenario(
                name=f"menus-snap-quick-{state}-{a}",
                lane="menus",
                args=["--aircraft", a, "--quick-mission", "--snapshot", "{work}/shot.ppm", "--snapshot-state", state, "--no-audio"],
                timeout=120,
                expect=[r"Menu preview:"],
                check=menu_picture(),
            )
        )
    # The creator on every base theater, with its theater popup open. The
    # imported `~` layout variants are not offered to players; `--theater ~CODE`
    # still opens the creator on one as a developer option, so each is checked
    # on the plain setup page.
    for code in THEATERS + VARIANT_THEATERS:
        variant = code.startswith("~")
        out.append(
            Scenario(
                name=f"menus-snap-quick-{'devtheater' if variant else 'theater'}-{code.strip('~').lower()}",
                lane="menus",
                args=["--theater", code, "--quick-mission", "--snapshot", "{work}/shot.ppm", "--snapshot-state", "normal" if variant else "theaters", "--no-audio"],
                timeout=120,
                expect=[r"(Menu|Locate) preview:"],
                check=menu_picture(),
            )
        )

    # Missions started through the headless AI probe: the same launch layout
    # code a flown Quick Mission uses. Player aircraft against three theaters
    # and every weather choice, then separations and wing sizes.
    probe = ["--ai-probe-ticks", "90", "--no-audio"]
    for a in AIRCRAFT:
        for theater in ("UKR", "KURILE", "PGU"):
            out.append(
                Scenario(
                    name=f"menus-start-{a}-{theater.lower()}",
                    lane="menus",
                    args=["--aircraft", a, "--theater", theater, "--probe-fight", "3:3", *probe],
                    timeout=300,
                    expect=[r"AI probe totals:"],
                    forbid=[r"OFF-MAP"],
                )
            )
    for weather in range(6):
        for theater in ("UKR", "NSK", "BAL"):
            out.append(
                Scenario(
                    name=f"menus-start-weather{weather}-{theater.lower()}",
                    lane="menus",
                    args=["--weather-condition", str(weather), "--theater", theater, "--probe-fight", "2:2", *probe],
                    timeout=300,
                    expect=[r"AI probe totals:"],
                )
            )
    for sep in (1, 2, 5, 10, 20, 50, 75, 100, 150, 200, 300):
        for theater in ("UKR", "CUB"):
            out.append(
                Scenario(
                    name=f"menus-start-separation{sep}-{theater.lower()}",
                    lane="menus",
                    args=["--separation", str(sep), "--theater", theater, "--probe-fight", "2:2", *probe],
                    timeout=300,
                    expect_exit=1 if sep == 300 else 0,
                    expect=[r"Separation .* does not fit"] if sep == 300 else [r"AI probe totals:", rf"AI probe layout:.*enemy_nm={sep}\.0 requested_nm={sep}\.0"],
                    forbid=[r"OFF-MAP"],
                )
            )
    for size in range(1, 6):
        out.append(
            Scenario(
                name=f"menus-start-wing{size}",
                lane="menus",
                args=["--probe-wing-size", str(size), "--probe-fight", "5:5", *probe],
                timeout=300,
                expect=[r"AI probe totals:"],
            )
        )

    # Stores taken off, as the Load Ordnance page leaves them: the aircraft still
    # flies and the gun still works with the externals off.
    for a in AIRCRAFT:
        for choice in ("none", "guns"):
            out.append(
                Scenario(
                    name=f"menus-loadout-{choice}-{a}",
                    lane="menus",
                    args=["--aircraft", a, "--loadout", choice, "--headless-flight", "600", "--no-audio"],
                    timeout=300,
                )
            )
    for a in ("f18", "f14", "su35"):
        for choice in ("none", "guns"):
            out.append(
                Scenario(
                    name=f"menus-window-weapons-page-{choice}-{a}",
                    lane="menus",
                    window=True,
                    args=["--aircraft", a, "--loadout", choice, "--capture-flight", "{work}/flight.ppm", "--instrument-page", "8", "--window-size", "960x720", "--no-audio"],
                    timeout=120,
                    expect=[r"Smoke test: requested screen presented successfully"],
                    check=flight_picture((960, 720)),
                )
            )

    # The real App path: a Quick Mission launched with the loadout page's edits.
    for a in AIRCRAFT:
        for choice in ("none", "guns"):
            out.append(
                Scenario(
                    name=f"menus-window-launch-{choice}-{a}",
                    lane="menus",
                    window=True,
                    args=["--aircraft", a, "--launch-quick-mission", "--loadout", choice, "--smoke-test", "--no-audio"],
                    timeout=180,
                    expect=[r"Quick Mission restart: PASS", r"Quick Mission launch:"],
                    check=flown_inventory(choice),
                )
            )

    # Ground starts through the real launch path, wings of three, every aircraft,
    # and the adapters that cannot ground start.
    for a in AIRCRAFT:
        out.append(
            Scenario(
                name=f"menus-window-launch-ground-{a}",
                lane="menus",
                window=True,
                args=["--aircraft", a, "--launch-quick-mission", "--ground-start", "2", "--probe-wing-size", "3", "--smoke-test", "--no-audio"],
                timeout=180,
                expect=[r"Quick Mission restart: PASS", r"Quick Mission launch: ground=Some\(\d+\).*supported=true.*parked_targets=2"],
            )
        )
    out.append(
        Scenario(
            name="menus-window-launch-legacy-adapter-airborne",
            lane="menus",
            window=True,
            args=["--legacy-flight", "--launch-quick-mission", "--smoke-test", "--no-audio"],
            timeout=180,
            expect=[r"Quick Mission restart: PASS", r"Quick Mission launch: ground=None"],
        )
    )
    out.append(
        Scenario(
            name="menus-window-launch-legacy-adapter-ground-refused",
            lane="menus",
            window=True,
            args=["--legacy-flight", "--launch-quick-mission", "--ground-start", "2", "--smoke-test", "--no-audio"],
            timeout=180,
            expect_exit=1,
            expect=[r"Ground start requires the researched flight model; choose Airborne for this adapter"],
        )
    )

    # Terrain, sky and lighting: real-window captures on every base theater in
    # clear weather and at night, four theaters in each other weather, and ground
    # starts. The frame must not be blank, one colour or black, day must be
    # bright, night dark, and the sky and ground both drawn.
    def terrain_scenario(theater, condition, ground, aircraft):
        name = f"menus-window-terrain-{theater.lower()}-{WEATHER_NAMES[condition]}-{'ground' if ground else 'air'}-{aircraft}"
        args = ["--theater", theater, "--weather-condition", str(condition), "--aircraft", aircraft]
        if ground:
            # The first airport Blue may start at: --ground-start refuses an
            # enemy field (6c83d6c1), and UKR's airport 1 is Redfor.
            airport = ground_airport(theater, 1)
            assert airport is not None, f"{theater} has no Blue ground start"
            args += ["--ground-start", str(airport)]
        return Scenario(
            name=name,
            lane="menus",
            window=True,
            args=[*args, "--capture-flight", "{work}/flight.ppm", "--window-size", "960x720", "--no-audio"],
            timeout=180,
            expect=[r"Smoke test: requested screen presented successfully"],
            check=lambda output, c=condition: scene_problems(c)(output) + flight_picture((960, 720))(output),
        )

    n = 0
    for theater in THEATERS:
        for condition in (0, 5):
            out.append(terrain_scenario(theater, condition, False, AIRCRAFT[n % 14]))
            n += 1
    for condition in (1, 2, 3, 4):
        for theater in ("UKR", "KURILE", "PGU", "VLA"):
            out.append(terrain_scenario(theater, condition, False, AIRCRAFT[n % 14]))
            n += 1
    # Ground starts in a temperate and a tropical theater. North Vietnam was
    # the tropical one until fields took their runway's side (6c83d6c1): Blue
    # has no field there, so Cuba (Blue's Guantanamo) stands in.
    for theater in ("UKR", "CUB"):
        for condition in range(6):
            out.append(terrain_scenario(theater, condition, True, AIRCRAFT[n % 14]))
            n += 1

    # Windowed captures through tools/agent-run.sh: a few per lane, quick ones.
    sizes = {"960x720": (960, 720), "1280x720": (1280, 720), "640x900": (640, 900)}
    for label, size in sizes.items():
        out.append(
            Scenario(
                name=f"menus-window-flight-{label}",
                lane="menus",
                window=True,
                args=["--capture-flight", "{work}/flight.ppm", "--window-size", label, "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                check=flight_picture(size),
            )
        )
        out.append(
            Scenario(
                name=f"menus-window-flight-menu-{label}",
                lane="menus",
                window=True,
                args=["--capture-flight", "{work}/flight.ppm", "--flight-menu", "--window-size", label, "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                check=flight_picture(size),
            )
        )
    for label in ("640x480", "641x481", "800x600", "1000x1000", "1600x900", "1920x1080"):
        width, height = (int(v) for v in label.split("x"))
        out.append(
            Scenario(
                name=f"menus-window-size-{label}",
                lane="menus",
                window=True,
                args=["--capture-flight", "{work}/flight.ppm", "--window-size", label, "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                check=flight_picture((width, height)),
            )
        )
    for label in ("480x320", "100x100", "3841x2161"):
        out.append(
            Scenario(
                name=f"menus-window-size-refused-{label}",
                lane="menus",
                args=["--capture-flight", "{work}/flight.ppm", "--window-size", label, "--no-audio"],
                timeout=60,
                expect_exit=1,
                expect=[r"window size outside 640x480\.\.3840x2160"],
            )
        )
    for label, clock, lighting in [("day", "12:00", "4,0,0,0"), ("night", "00:00", "4,3,0,0"), ("dusk", "19:24", "4,0,0,0")]:
        out.append(
            Scenario(
                name=f"menus-window-cockpit-lighting-{label}",
                lane="menus",
                window=True,
                args=["--aircraft", "f16c", "--capture-flight", "{work}/flight.ppm", "--window-size", "960x720", "--no-audio"],
                env={"TORE_WEATHER_TIME": clock, "TORE_COCKPIT_LIGHTING": lighting},
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                check=flight_picture((960, 720)),
            )
        )
    for a in AIRCRAFT:
        out.append(
            Scenario(
                name=f"menus-window-cockpit-{a}",
                lane="menus",
                window=True,
                args=["--aircraft", a, "--capture-flight", "{work}/flight.ppm", "--window-size", "960x720", "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                check=flight_picture((960, 720)),
            )
        )
    for view in range(12):
        out.append(
            Scenario(
                name=f"menus-window-view-{view}",
                lane="menus",
                window=True,
                args=["--capture-flight", "{work}/flight.ppm", "--flight-view", str(view), "--window-size", "960x720", "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                check=flight_picture((960, 720)),
            )
        )
    # Instrument windows are drawn on the CPU: every page for every aircraft,
    # and every panel-only systems fault (1 to 35) on the systems window.
    for a in AIRCRAFT:
        for page in range(10):
            out.append(
                Scenario(
                    name=f"menus-panel-{a}-page-{page}",
                    lane="menus",
                    args=["--aircraft", a, "--panel-snapshot", "{work}/panel.ppm", "--instrument-page", str(page), "--no-audio"],
                    timeout=120,
                    outputs=["panel.ppm"],
                )
            )
    for fault in range(1, 36):
        out.append(
            Scenario(
                name=f"menus-panel-fault-{fault}",
                lane="menus",
                args=["--panel-snapshot", "{work}/panel.ppm", "--instrument-page", "7", "--systems-preview", str(fault), "--no-audio"],
                timeout=120,
                outputs=["panel.ppm"],
            )
        )
    priority = ["nice", "-n", "10"] if shutil.which("nice") else []
    out.append(Scenario(
        name="menus-window-graphics-runtime", lane="menus", window=True,
        args=["--smoke-test", "--no-audio"], timeout=600,
        env={"CARGO_BUILD_JOBS": "8"},
        then=[Step([*priority, "cargo", "test", "--locked", "-p", "tore-app",
                    "gpu_runtime_graphics_changes_keep_sun_and_spotting_valid",
                    "--", "--ignored", "--nocapture", "--test-threads=1"],
                   app=False, timeout=600)],
        expect=[r"1 passed; 0 failed"],
        notes="Runtime MSAA transitions with sun visibility, spotting, scale, shadows and resized views under GPU validation.",
    ))
    # Every graphics option value, and the preview and panel switches, in a
    # real window.
    for option in (
        "--anti-aliasing off", "--anti-aliasing 2x", "--anti-aliasing 4x", "--anti-aliasing 8x",
        "--render-scale 75", "--render-scale 125", "--render-scale 150", "--render-scale 200",
        "--spotting-aid off", "--spotting-aid strong", "--terrain-filtering off", "--original-graphics",
        "--shadow-distance trees", "--shadow-distance 1", "--shadow-distance 2",
        "--shadow-distance 4", "--shadow-distance 6", "--shadow-distance 8",
    ):
        out.append(
            Scenario(
                name=f"menus-window-graphics{option.replace('--', '-').replace(' ', '-')}",
                lane="menus",
                window=True,
                args=[*option.split(), "--dummy-aircraft", "mig29,2", "--capture-flight", "{work}/flight.ppm", "--window-size", "960x720", "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                check=flight_picture((960, 720)),
            )
        )
    for name, extra in (
        ("map", ["--flight-map"]),
        ("mission-brief", ["--mission-brief", "--launch-quick-mission"]),
        ("panels", ["--dummy-aircraft", "mig29,2", "--flight-panels", "thought,telemetry,guidance,comms,menu", "--debug-panels"]),
        ("weapon-diagnostics", ["--weapon-diagnostics", "--dummy-aircraft", "mig29,1"]),
        ("small-layout", ["--instrument-layout", "small"]),
        ("damage", ["--damage-preview", "0.6"]),
        ("ejection", ["--ejection-preview", "seat"]),
        ("chaff", ["--combat-command", "chaff", "--countermeasure-preview", "60"]),
    ):
        out.append(
            Scenario(
                name=f"menus-window-preview-{name}",
                lane="menus",
                window=True,
                args=[*extra, "--capture-flight", "{work}/flight.ppm", "--window-size", "960x720", "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
                # A creator launch receives the display's physical HiDPI size.
                check=mission_brief_picture if name == "mission-brief" else flight_picture((960, 720)),
            )
        )
    for flag in ("--free-flight", "--launch-quick-mission", "--quick-mission", "--controls-menu"):
        out.append(
            Scenario(
                name=f"menus-window-smoke{flag.replace('--', '-')}",
                lane="menus",
                window=True,
                args=[flag, "--smoke-test", "--no-audio"],
                timeout=120,
                expect=[r"Smoke test: requested screen presented successfully"],
            )
        )
    return out
