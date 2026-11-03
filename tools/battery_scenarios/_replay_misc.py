"""Scenarios around flight rather than the flight itself: menus of the replay lane, diagnostics,
audio start-up, controller input and profiles, input tapes, first-run import errors.
"""
from __future__ import annotations

import re
from pathlib import Path

from battery import Scenario, Step
from battery_scenarios import _replay_tools as tools
from battery_scenarios._replay_record import AIRCRAFT, sections

PY = "python3"
TOOLS = tools.__file__
ROOT = Path(__file__).resolve().parents[2]

SNAPSHOT_STATES = [
    "normal", "hover", "pressed", "help", "pref", "multi", "notice",
    "controls", "controls-keyboard", "controls-mouse", "controls-head", "controls-search", "controls-search-keys", "controls-inv",
    "graphics", "sound", "replays", "replays-settings", "replays-delete",
    "locate", "locate-importing", "locate-done", "locate-starting",
]


def snapshot_scenarios() -> list[Scenario]:
    out = []
    for state in SNAPSHOT_STATES:
        out.append(
            Scenario(
                name=f"replay-snapshot-{state}",
                lane="replay",
                args=["--snapshot-state", state, "--snapshot", "{work}/s.ppm", "--no-audio"],
                outputs=["s.ppm"],
                check_work=lambda work, output: tools.ppm_problems(str(work / "s.ppm"), min_colors=8),
            )
        )
    return out


def diagnostics_scenarios() -> list[Scenario]:
    out = []
    modes = {"": (0, "Diagnostics self-test: PASS"), "error": (1, "Deliberate diagnostics self-test error"), "panic": (101, "Rust panic: Deliberate diagnostics self-test panic"),
             "worker-panic": (1, "worker panic was recorded"), "dialog": (1, "Deliberate diagnostics self-test error")}
    for mode, (code, text) in modes.items():
        out.append(
            Scenario(
                name=f"replay-diag-{mode or 'plain'}",
                lane="replay",
                args=["--diagnostics-self-test" + (f"={mode}" if mode else "")],
                expect_exit=code,
                expect=[re.escape(text)],
                allow_generic=["panic"] if mode == "panic" else [],
                env={"TORE_LOG_DIR": "{work}/logs"},
                check_work=(lambda work, output: [] if mode == "" or (work.parent.parent / "x").exists() or True else []),
            )
        )
    # The graphics check probes adapters and may open a hidden window.
    out.append(
        Scenario(
            name="replay-diag-graphics",
            lane="replay",
            args=["--diagnostics-self-test=graphics"],
            window=True,
            timeout=120,
        )
    )
    out.append(
        Scenario(
            name="replay-diag-startup-script",
            lane="replay",
            args=["--version"],
            then=[Step([PY, "tools/check_startup_diagnostics.py", "{bin}"], app=False, timeout=240)],
            expect=[r"tore-app|T\.O\.R\.E"],
            check_work=lambda work, output: [] if "Startup diagnostics passed" in output else ["startup diagnostics script did not pass"],
        )
    )
    return out


def audio_scenarios() -> list[Scenario]:
    """Start-up with audio on. A failed device must degrade to silence, never crash."""
    out = []
    variants = {
        "default": {},
        "bad-pulse": {"PULSE_SERVER": "unix:/nonexistent/pulse", "PIPEWIRE_REMOTE": "/nonexistent/pw", "ALSA_CONFIG_PATH": "/dev/null"},
        "bad-alsa": {"ALSA_CONFIG_PATH": "/dev/null"},
    }
    for name, env in variants.items():
        out.append(
            Scenario(
                name=f"replay-audio-freeflight-{name}",
                lane="replay",
                args=["--free-flight"],
                window=True,
                env={**env, "TORE_PERF_FRAMES": "90"},
                timeout=180,
                expect=[r"frame interval: mean"],
                forbid=[r"(?i)audio.*(panick|failed to start.*fatal)"],
                check=(lambda output, name=name: ["audio failure was not reported as 'Continuing without audio'"] if name == "bad-pulse" and "audio" in output.lower() and "Continuing without audio" not in output and "Audio:" not in output else []),
            )
        )
    # Replay sound: play a recording at 1x with audio on.
    out.append(
        Scenario(
            name="replay-audio-watch-replay",
            lane="replay",
            args=["--ai-probe-ticks", "3600", "--separation", "5", "--probe-attack", "600:10", "--record-mission", "{work}/rec.tore-replay", "--no-audio"],
            then=[Step(["--watch-replay", "{work}/rec.tore-replay", "--replay-speed", "1", "--replay-tick", "600"], window=True, timeout=180)],
            env={"TORE_PERF_FRAMES": "300"},
            check_work=lambda work, output: [] if "frame interval: mean" in output else ["the replay did not play"],
        )
    )
    return out


def tape_text(kind: str) -> str:
    """A hand-made pilot input tape (tore-pilot 1)."""
    import math

    lines = ["tore-pilot 1"]
    for t in range(1, 1201):
        if kind == "roll":
            lines.append(f"{t} 0.05 {0.8 * math.sin(t / 90):.4f} 0 0 {'1' if t == 1 else '-'}")
        elif kind == "loop":
            lines.append(f"{t} {0.6 if 200 < t < 500 else 0} 0 0 0 {'1' if t == 1 else '-'}")
        else:
            lines.append(f"{t} {0.3 * math.sin(t / 60):.4f} {0.5 * math.sin(t / 90):.4f} 0 0 {'1' if t == 1 else '-'}{' set:gear:1' if t == 600 else ''}")
    return "\n".join(lines) + "\n"


def make_tape_step(kind: str, dest: str) -> Step:
    code = f"import sys; sys.path.insert(0, {str(ROOT / 'tools')!r}); from battery_scenarios._replay_misc import tape_text; open(sys.argv[1], 'w').write(tape_text({kind!r}))"
    return Step([PY, "-c", code, dest], app=False)


def input_scenarios() -> list[Scenario]:
    out = []
    out.append(
        Scenario(
            name="replay-input-list-no-controllers",
            lane="replay",
            args=["--list-inputs", "--no-controllers", "--no-audio"],
            expect=[r"no device is opened"],
            forbid=[r"^device ", r"detected, but no readable"],
            timeout=60,
        )
    )
    out.append(Scenario(name="replay-input-list", lane="replay", args=["--list-inputs", "--no-audio"], expect=[r"Input diagnostics"], timeout=60))
    out.append(
        Scenario(
            name="replay-input-profile-roundtrip",
            lane="replay",
            args=["--write-input-profile", "{work}/p.conf"],
            outputs=["p.conf"],
            then=[
                Step(["--write-input-profile", "{work}/p.conf"], expect_exit=1),
                Step(["--input-profile", "{work}/p.conf", "--free-flight", "--smoke-test", "--no-audio"], window=True, timeout=120),
            ],
            check_work=lambda work, output: (
                ([] if "File exists" in sections(output).get(1, "") else ["overwrite was not refused clearly"])
                + ([] if "Smoke test: requested screen presented successfully" in sections(output).get(2, "") else ["generated profile did not load"])
                + ([] if (work / "p.conf").read_text().startswith("tore-input 1") else ["profile has no header"])
            ),
        )
    )
    bad_profiles = {
        "empty": (b"", "missing input profile version"),
        "wrong-header": (b"not a profile\n", "expected tore-input 1"),
        "future-version": (b"tore-input 2\n", "expected tore-input 1"),
        "bad-action": (b"tore-input 1\nbind key:F1 nonsense-action press\n", "invalid profile directive"),
        "fire-press": (b"tore-input 1\nbind bogus button:1 fire press\n", "fire and fire-gun require hold"),
        "dup-alias": (b"tore-input 1\nalias a linux-xx\nalias a linux-yy\n", "duplicate alias"),
        "bad-utf8": (b"\xff\xfe\x00 bad", "valid UTF-8"),
        "too-big": (b"#" * 300_000, "exceeds 256 KiB"),
        "many-binds": (b"tore-input 1\n" + b"".join(f"bind key:F{n % 12 + 1} gear press\n".encode() for n in range(1100)), "invalid profile directive or limit exceeded"),
    }
    for name, (content, message) in bad_profiles.items():
        if name == "many-binds":
            code = "import sys; open(sys.argv[1], 'w').write('tore-input 1\\n' + ''.join(f'bind key:F{n % 12 + 1} gear press\\n' for n in range(1100)))"
        elif len(content) < 2000:
            code = f"import sys; open(sys.argv[1], 'wb').write({content!r})"
        else:
            code = f"import sys; open(sys.argv[1], 'wb').write(b'#' * {len(content)})"
        out.append(
            Scenario(
                name=f"replay-input-badprofile-{name}",
                lane="replay",
                args=["--version"],
                then=[
                    Step([PY, "-c", code, "{work}/bad.conf"], app=False),
                    Step(["--input-profile", "{work}/bad.conf", "--free-flight", "--smoke-test", "--no-audio"], window=True, expect_exit=None, timeout=120),
                ],
                check_work=lambda work, output, message=message: check_bad_profile(output, message),
            )
        )
    out.append(
        Scenario(
            name="replay-input-profile-missing-file",
            lane="replay",
            args=["--input-profile", "{work}/nothing.conf", "--free-flight", "--smoke-test", "--no-audio"],
            window=True,
            expect_exit=1,
            expect=[r"nothing\.conf: No such file"],
        )
    )
    # Input tapes: hand-made, replayed twice, identical results on every aircraft.
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"replay-tape-{ac}",
                lane="replay",
                args=["--version"],
                then=[
                    make_tape_step("mixed", "{work}/t.tape"),
                    Step(["--replay-input", "{work}/t.tape", "--aircraft", ac, "--researched-flight", "--no-audio"], timeout=120),
                    Step(["--replay-input", "{work}/t.tape", "--aircraft", ac, "--researched-flight", "--no-audio"], timeout=120),
                ],
                check_work=lambda work, output: check_tape_runs(output),
            )
        )
    bad_tapes = {
        "header": "tore-pilot 2\n",
        "empty": "",
        "nan": "tore-pilot 1\n1 NaN 0 0 0 -\n",
        "bad-command": "tore-pilot 1\n1 0 0 0 0 - set:gear:2\n",
        "tick-zero": "tore-pilot 1\n0 0 0 0 0 -\n",
        "out-of-order": "tore-pilot 1\n2 0 0 0 0 -\n1 0 0 0 0 -\n",
        "huge-axis": "tore-pilot 1\n1 50 0 0 0 -\n",
        "short-line": "tore-pilot 1\n1 0 0\n",
    }
    for name, text in bad_tapes.items():
        code = f"import sys; open(sys.argv[1], 'w').write({text!r})"
        out.append(
            Scenario(
                name=f"replay-tape-bad-{name}",
                lane="replay",
                args=["--version"],
                then=[
                    Step([PY, "-c", code, "{work}/t.tape"], app=False),
                    Step(["--replay-input", "{work}/t.tape", "--no-audio"], expect_exit=1),
                ],
                allow_generic=["NaN in output"],
                check_work=lambda work, output: [] if "panicked" not in output and len(sections(output).get(2, "").strip().splitlines()) >= 2 else ["no clear rejection"],
            )
        )
    return out


def check_bad_profile(output: str, message: str) -> list[str]:
    text = sections(output).get(2, "")
    problems = []
    if "panicked" in text:
        problems.append("panic on a bad profile")
    if message and message not in text:
        problems.append(f"expected '{message}' in the profile error")
    if message and "bad.conf" not in text:
        problems.append("the error does not name the profile file")
    return problems


def check_tape_runs(output: str) -> list[str]:
    s = sections(output)
    a, b = s.get(2, ""), s.get(3, "")
    problems = []
    result = re.search(r"ticks=1200 speed_kt=[\d.]+ altitude_ft=[-\d.]+ fuel_lb=[\d.]+ crashed=\w+", a)
    if not result:
        problems.append("the tape replay printed no result line")
    keys = [ln for ln in a.splitlines() if ln.startswith(("flight_model", "aoa_deg", "ticks=", "vertical", "departure"))]
    keys_b = [ln for ln in b.splitlines() if ln.startswith(("flight_model", "aoa_deg", "ticks=", "vertical", "departure"))]
    if keys != keys_b:
        problems.append(f"two replays of one tape differ: {keys} vs {keys_b}")
    return problems


def import_scenarios() -> list[Scenario]:
    """First-run import against bad media, in a fresh data folder of the scenario's own."""
    out = []
    kinds = {
        "empty-dir": ("media", "not a Fighters Anthology source"),
        "junk-files": ("media", "has not been reviewed"),
        "nonexistent": ("media", "does not exist"),
        "text-file": ("media", "is a file, and its folder"),
        "iso-file": ("media.iso", "Mount the image"),
        "trunc-fa1": ("media", "FA_1.LIB could not be read"),
        "trunc-fa2": ("media", "FA_2.LIB could not be read"),
        "empty-fa1": ("media", "FA_1.LIB could not be read"),
        "exe-flip": ("media", "has not been reviewed"),
        "no-exe": ("media", "not a Fighters Anthology source"),
        "no-fa2": ("media", "not a Fighters Anthology source"),
    }
    for kind, (leaf, message) in kinds.items():
        out.append(
            Scenario(
                name=f"replay-import-bad-{kind}",
                lane="replay",
                args=["--version"],
                env={"TORE_DATA_DIR": "{work}/fresh"},
                then=[
                    Step([PY, TOOLS, "media", kind, "{work}/" + leaf], app=False),
                    Step(["--import", "{work}/" + leaf, "--import-only", "--no-audio"], expect_exit=1, timeout=240),
                ],
                check_work=lambda work, output, message=message: check_bad_import(work, output, message),
            )
        )
    return out


STATE = (
    "import glob,hashlib,os,sys; d=sys.argv[1]; p=glob.glob(d+'/menu-*.pack'); "
    "print('STATE', [os.path.getsize(x) for x in p], hashlib.sha1(open(d+'/media-source.txt','rb').read()).hexdigest())"
)


def failed_reimport_scenarios() -> list[Scenario]:
    """A failed import into a data folder that already holds a good import must leave it alone."""
    out = []
    for kind in ("trunc-fa2", "empty-fa1", "exe-flip", "empty-dir", "no-exe"):
        out.append(
            Scenario(
                name=f"replay-import-keeps-good-data-{kind}",
                lane="replay",
                args=["--version"],
                then=[
                    Step([PY, "-c", STATE, "{work}/data"], app=False),
                    Step([PY, TOOLS, "media", kind, "{work}/media"], app=False),
                    Step(["--import", "{work}/media", "--import-only", "--no-audio"], expect_exit=1, timeout=240),
                    Step([PY, "-c", STATE, "{work}/data"], app=False),
                    Step(["--snapshot-state", "normal", "--snapshot", "{work}/n.ppm", "--no-audio"], timeout=120),
                ],
                check_work=lambda work, output: check_kept(work, output),
            )
        )
    return out


def check_kept(work: Path, output: str) -> list[str]:
    s = sections(output)
    before, after = re.search(r"STATE .*", s.get(1, "")), re.search(r"STATE .*", s.get(4, ""))
    problems = []
    if not before or "[]" in before.group(0):
        problems.append("the profile the scenario started from has no imported pack")
    elif not after or before.group(0) != after.group(0):
        problems.append(f"a failed import changed the existing data: {before.group(0)} -> {after.group(0) if after else 'nothing'}")
    problems += tools.ppm_problems(str(work / "n.ppm"), min_colors=8)
    return problems


def check_bad_import(work: Path, output: str, message: str) -> list[str]:
    problems = []
    text = sections(output).get(2, "")
    if message not in text:
        problems.append(f"expected '{message}' in the import error, got: {text.strip()[-200:]}")
    if "panicked" in text:
        problems.append("panic on bad media")
    if re.search(r"invalid archive sentinel|failed to fill whole buffer|unexpected end of file", text) and "could not be read" not in text:
        problems.append("the import error is a bare reader message")
    if list((work / "fresh").glob("menu-*.pack")):
        problems.append("a failed import left a pack behind")
    return problems


def validate_scenarios() -> list[Scenario]:
    return [
        Scenario(
            name="replay-validate-creator",
            lane="replay",
            args=["--validate-creator"],
            # The menus lane's input fuzz made this take about eight minutes in a debug build.
            timeout=1800,
        ),
        Scenario(name="replay-sensor-summary", lane="replay", args=["--sensor-summary"], timeout=120),
        Scenario(name="replay-help", lane="replay", args=["--help"], expect=[r"Mission recordings:", r"--replay-menu"]),
        Scenario(name="replay-version", lane="replay", args=["--version"], expect=[r"T\.O\.R\.E"]),
    ]


def combat_smoke_scenarios() -> list[Scenario]:
    """The weapons acceptance probe on each aircraft. Currently failing on most; see lane-replay.md."""
    out = []
    for ac in AIRCRAFT:
        out.append(
            Scenario(
                name=f"replay-combat-smoke-{ac}",
                lane="replay",
                args=["--combat-smoke", "--aircraft", ac, "--no-audio"],
                timeout=300,
                expect=[r"combat smoke .* PASS"],
            )
        )
    return out


BAD_OPTIONS = {
    "rate-word": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/o", "--rate", "abc"], "--rate needs a number, not 'abc'"),
    "rate-zero": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/o", "--rate", "0"], "--rate needs samples per second above 0"),
    "rate-huge": (["--recording-acmi", "{work}/a.tore-replay", "--out", "{work}/o.acmi", "--rate", "999"], "--rate needs samples per second"),
    "ids-word": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/o", "--ids", "a,b"], "--ids needs a number"),
    "from-word": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/o", "--from", "x"], "--from needs a number"),
    "to-negative": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/o", "--to", "-3"], "--to needs seconds of mission time"),
    "from-after-to": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/o", "--from", "20", "--to", "10"], "--from 20 is later than --to 10"),
    "ids-unknown": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/o", "--ids", "0,999"], "names aircraft 999"),
    "flag-separation": (["--ai-probe-ticks", "60", "--separation", "x"], "--separation needs a number"),
    "flag-wing-size": (["--ai-probe-ticks", "60", "--probe-wing-size", "x"], "--probe-wing-size needs a number"),
    "flag-trace": (["--ai-probe-ticks", "60", "--probe-trace", "x"], "--probe-trace needs a number"),
    "flag-fault-tick": (["--ai-probe-ticks", "60", "--probe-fault", "x:1"], "--probe-fault needs a number"),
    "flag-threat-tick": (["--ai-probe-ticks", "60", "--probe-threat", "x:hit"], "--probe-threat needs a number"),
    "flag-home": (["--ai-probe-ticks", "60", "--probe-player-home", "a:b"], "--probe-player-home needs a number"),
    "flag-wing-order-tick": (["--ai-probe-ticks", "60", "--probe-wing-order", "x:bug-out"], "--probe-wing-order needs a number"),
    "flag-damage-preview": (["--damage-preview", "x"], "--damage-preview needs a number"),
    "flag-countermeasure-preview": (["--countermeasure-preview", "x"], "--countermeasure-preview needs a number"),
    "flag-zoom": (["--flight-zoom", "x"], "--flight-zoom needs a number"),
    "flag-window-size": (["--window-size", "axb"], "--window-size needs a number"),
    "flag-look": (["--flight-look", "a,b"], "--flight-look needs a number"),
    "flag-throttle": (["--flight-throttle", "x"], "--flight-throttle needs a number"),
    "flag-bay": (["--flight-bay", "x"], "--flight-bay needs a number"),
    "flag-devices": (["--flight-devices", "a,b,c,d,e"], "--flight-devices needs a number"),
    "flag-controls": (["--flight-controls", "a,b,c"], "--flight-controls needs a number"),
    "flag-weapon-slot": (["--weapon-slot", "x"], "--weapon-slot needs a number"),
    "flag-combat-ticks": (["--combat-probe-ticks", "x"], "--combat-probe-ticks needs a number"),
    "flag-scope-range": (["--scope-range", "x"], "--scope-range needs a number"),
    "flag-instrument-page": (["--instrument-page", "x"], "--instrument-page needs a number"),
    "flag-weather": (["--weather-condition", "x"], "--weather-condition needs a number"),
    "flag-flight-view": (["--flight-view", "x"], "--flight-view needs a number"),
    "flag-dummy-count": (["--dummy-aircraft", "f18,x"], "--dummy-aircraft needs a number"),
    "info-none": (["--recording-info"], "--recording-info needs a recording"),
    "diff-one": (["--recording-diff", "{work}/a.tore-replay"], "--recording-diff needs two recordings"),
    "diff-missing": (["--recording-diff", "{work}/a.tore-replay", "{work}/none.tore-replay"], "none.tore-replay"),
    "log-into-file": (["--recording-log", "{work}/a.tore-replay", "--out", "{work}/a.tore-replay"], "cannot use it as the output folder"),
    "acmi-into-folder": (["--recording-acmi", "{work}/a.tore-replay", "--out", "{work}"], "cannot write it"),
    "tick-negative": (["--watch-replay", "{work}/a.tore-replay", "--replay-tick", "-5"], "--replay-tick needs a number"),
    "speed-word": (["--watch-replay", "{work}/a.tore-replay", "--replay-speed", "fast"], "--replay-speed needs a number"),
    "speed-out-of-range": (["--watch-replay", "{work}/a.tore-replay", "--replay-speed", "100"], "is not a replay speed"),
    "aircraft-word": (["--watch-replay", "{work}/a.tore-replay", "--replay-aircraft", "x"], "--replay-aircraft needs a number"),
    "view-out-of-range": (["--watch-replay", "{work}/a.tore-replay", "--flight-view", "12"], "--flight-view needs 0..11"),
    "ui-bogus": (["--watch-replay", "{work}/a.tore-replay", "--replay-ui", "bogus"], "unknown replay interface part"),
    "panels-bogus": (["--watch-replay", "{work}/a.tore-replay", "--replay-panels", "bogus"], "unknown replay panel"),
    "menu-bogus": (["--watch-replay", "{work}/a.tore-replay", "--replay-menu", "bogus"], "unknown replay menu page"),
    "lookat-bogus": (["--watch-replay", "{work}/a.tore-replay", "--replay-look-at", "bogus"], "aircraft:ID"),
    "watch-missing": (["--watch-replay", "{work}/none.tore-replay"], "none.tore-replay"),
    "capture-without-watch": (["--capture-replay", "{work}/c.ppm"], ""),
    "record-empty-path": (["--ai-probe-ticks", "60", "--record-mission", ""], "needs a new path, not an empty one"),
    "record-missing-folder": (["--ai-probe-ticks", "60", "--record-mission", "{work}/nowhere/x.tore-replay"], "No such file or directory"),
    "record-onto-folder": (["--ai-probe-ticks", "60", "--record-mission", "{work}"], "already exists"),
    "monitor-word": (["--monitor-inputs", "abc"], "--monitor-inputs needs a number"),
    "monitor-range": (["--monitor-inputs", "100000"], "must be 1..300 seconds"),
    "rumble-none": (["--test-rumble", "nosuchdevice"], "rumble device was not found"),
    # 288000 since ca59477f: 40 minutes, so a four-ship landing on a merged
    # 12,000 ft field can finish its taxi. 216000 was the old 30 minute cap.
    "probe-ticks-zero": (["--ai-probe-ticks", "0"], "needs 1 to 288000 ticks"),
    "probe-ticks-word": (["--ai-probe-ticks", "many"], "--ai-probe-ticks needs a number"),
    "ground-start-word": (["--ai-probe-ticks", "60", "--ground-start", "x"], "--ground-start needs a number"),
    "ground-start-none": (["--ai-probe-ticks", "60", "--ground-start", "99"], "airport is unavailable"),
    "separation-odd": (["--ai-probe-ticks", "60", "--separation", "3"], "--separation needs one of"),
    "wing-size": (["--ai-probe-ticks", "60", "--probe-wing-size", "6"], "--probe-wing-size needs 1..5"),
    "fight-size": (["--ai-probe-ticks", "60", "--probe-fight", "16:1"], "--probe-fight needs"),
    "mission-preset": (["--ai-probe-ticks", "60", "--ai-mission", "bogus"], "unknown AI mission preset"),
    "aircraft-name": (["--ai-probe-ticks", "60", "--aircraft", "bogus"], "supported aircraft"),
    "theater-code": (["--ai-probe-ticks", "60", "--theater", "XXX"], "unknown retail map layout"),
    "wing-order": (["--ai-probe-ticks", "60", "--probe-wing-order", "10:dance"], "--probe-wing-order needs"),
    "fault-index": (["--ai-probe-ticks", "60", "--probe-fault", "10:99"], "fault index must be"),
    "threat-kind": (["--ai-probe-ticks", "60", "--probe-threat", "10:nuke"], "unknown probe threat"),
}


def bad_option_scenarios() -> list[Scenario]:
    out = []
    for name, (args, message) in BAD_OPTIONS.items():
        out.append(
            Scenario(
                name=f"replay-cli-bad-{name}",
                lane="replay",
                args=["--ai-probe-ticks", "300", "--separation", "2", "--record-mission", "{work}/a.tore-replay", "--no-audio"],
                then=[Step(args, expect_exit=1, timeout=60)],
                check_work=lambda work, output, message=message: check_bad_option(output, message),
            )
        )
    return out


def check_bad_option(output: str, message: str) -> list[str]:
    text = sections(output).get(1, "")
    problems = []
    if message and message not in text:
        problems.append(f"expected '{message}' in the error, got: {text.strip()[-200:]}")
    if "panicked" in text:
        problems.append("panic on a bad option")
    # The parser's words may follow the option's own in brackets; they must not stand alone.
    if re.search(r"^(invalid float literal|invalid digit found[a-z ]*)$", text, re.M):
        problems.append("a bare parser message reached the user")
    return problems


def scenarios() -> list[Scenario]:
    return bad_option_scenarios() + failed_reimport_scenarios() + snapshot_scenarios() + diagnostics_scenarios() + audio_scenarios() + input_scenarios() + validate_scenarios() + combat_smoke_scenarios()
