"""Lane: flight. The air-to-ground missiles (docs/spec/air-to-ground.md, "Missiles and the HARM
boresight cue", "Weapon classes" 8 to 11), slice W2, through `--a2g-run`'s missile kinds: every
designation, mode change and launch goes through the player's own controls and the trigger.

`a2g-maverick` over the Ukraine column `~QUCOL`: an A-10 turns the A-G page on (Ctrl+R), T
designates a tank, the AGM-65G locks it in CUED and kills it; with nothing designated the Maverick
is in BORE, its seeker settles on the strongest return near the bore (the hottest, centre weighted)
and the missile leaves locked on that one (John accepted plan decision 10); an AV-8B designates
with its FLIR (the IR page, M) and an A-10 with Enter (the visible ground contact nearest the
centre) for an AGM-65A, each with a kill (the AGM-65A's with a hit). The imported A-10 has no infrared sensor, so the FLIR run is the AV-8B's.
An A-10's AGM-65A also hits from 6 nmi on the A-G page after coasting into its dive,
checking that steering compensates for the downward sag (FX-M).

`a2g-harm-bore` over the Vladivostok radar site `~QVRDRI` (seed 4, heavy defenses): an F-4G with
AGM-88s and nothing designated sees the SA-6 battery's Straight Flush on the HARM cue (John,
2026-10-10: "act kind of like boresite on a2a weapons") and fires at it in BORE. An ace radar
shuts down when the HARM comes within 10 nm, and the missile keeps homing on it after its radar
stops (retail data, docs/formats/radar.md "HARM page") and destroys it. A novice radar stays on.

`a2g-harm-page`: the same F-4G on the HARM page (M), T cycles the page's emitters to the Straight
Flush, the designation turns the cue to CUED and the HARM leaves at it.

`a2g-antiship` over the Ukraine surface fleet `~QUSFLT`: an F/A-18D designates the Krivak on the
A-G page and its AGM-84A flies out midcourse, goes active and hits it; a Rafale's AM-39 does the
same. A surface radar weapon needs the A-G page on at release (retail data).

`a2g-sarh` over `~QUCOL`: a MiG-23's AS-7 rides the A-G track to a kill; with the A-G page turned
off two seconds after the launch the missile loses its target for good and flies on unguided
(retail data: AS-7 and AS-15 need A-G for the whole flight).
"""
import re

from battery import Scenario, Step

DESIGNATED = re.compile(r"^a2g-run: designated t=\S+ (0x[0-9a-f]+) presses (\d+)$", re.M)
READY = re.compile(
    r"^a2g-run: ready t=\S+ mode (\S+) candidate (\S+) designated (\S+) seeker (\S+) target (\S+)$", re.M
)
LAUNCH = re.compile(
    r"^a2g-run: launch t=(\S+) id (\d+) target (\S+) mode (\S+) seeker (\S+) acquired (\w+) unguided (\w+) "
    r"rounds \d+$",
    re.M,
)
CUE = re.compile(
    r"^a2g-run: harm-cue t=\S+ painted-by \S+ emitters (\d+) candidate (\S+) cued (\d) list (\S+)$", re.M
)
BORE = re.compile(r"^a2g-run: bore-cone t=\S+ returns (\d+) list (\S+)$", re.M)
FLIGHT = re.compile(r"^a2g-run: flight t=(\S+) id \d+ range \S+ altitude \S+ (\w+) target (\S+) unguided (\d)$", re.M)
END = re.compile(
    r"^a2g-run: end t=(\S+) id \d+ at \S+ \S+ \S+ miss (\S+) last .* target-hp (-?\d+)/(\d+)$", re.M
)
SHUTDOWN = re.compile(r"^a2g-run: shutdown t=(\S+) unit (0x[0-9a-f]+) missile \d+ shuts (\d)$", re.M)
BREAK = re.compile(r"^a2g-run: break t=(\S+) air-ground off$", re.M)
KILL = re.compile(r"^a2g-run: kill (0x[0-9a-f]+) class \S+$", re.M)

STRAIGHT_FLUSH = "0x50000009"
KRIVAK = "0x50000003"


def runs(output: str) -> list[str]:
    """The output of each `--a2g-run`, in order."""
    return [p for p in re.split(r"^a2g-run: done$", output, flags=re.M) if "a2g-run:" in p]


def _launch(run: str, label: str, problems: list[str]):
    m = LAUNCH.search(run)
    if m is None:
        why = re.search(r"^a2g-run: no launch readiness (.*)$", run, re.M)
        problems.append(f"{label}: no launch ({why.group(1) if why else 'unknown'})")
    return m


def _end(run: str, label: str, problems: list[str]):
    m = END.search(run)
    if m is None:
        problems.append(f"{label}: the missile's end was not seen")
    return m


def _killed(run: str, target: str, label: str, problems: list[str]):
    end = _end(run, label, problems)
    if end and int(end.group(3)) > 0:
        problems.append(f"{label}: the target has {end.group(3)} hit points left, want 0")
    if target not in KILL.findall(run):
        problems.append(f"{label}: no kill of {target}")


def _cued_kill(run: str, label: str, problems: list[str], kill: bool = True) -> None:
    """A designation through the page, a CUED lock on it, the launch at it and its kill (or, with `kill`
    false, a hit)."""
    designated = DESIGNATED.search(run)
    if designated is None:
        problems.append(f"{label}: nothing designated")
        return
    target = designated.group(1)
    ready = READY.search(run)
    if ready is None or ready.group(1) != "CUED" or ready.group(3) != target:
        problems.append(f"{label}: not ready in CUED on {target}: {ready.group(0) if ready else None}")
    launch = _launch(run, label, problems)
    if launch and (launch.group(3) != target or launch.group(4) != "CUED"):
        problems.append(f"{label}: launched at {launch.group(3)} in {launch.group(4)}, want {target} in CUED")
    if kill:
        _killed(run, target, label, problems)
    else:
        end = _end(run, label, problems)
        if end and int(end.group(3)) >= int(end.group(4)):
            problems.append(f"{label}: the target was not hit ({end.group(3)}/{end.group(4)})")


def maverick_problems(output: str) -> list[str]:
    parts = runs(output)
    if len(parts) != 5:
        return [f"{len(parts)} runs finished, want 5"]
    problems: list[str] = []
    _cued_kill(parts[0], "A-10 AGM-65G on A-G", problems)
    ready = READY.search(parts[0])
    if ready and ready.group(4) != "Locked":
        problems.append(f"A-10 AGM-65G on A-G: the seeker reads {ready.group(4)} at release, want Locked")

    bore, label = parts[1], "A-10 AGM-65G in BORE"
    if DESIGNATED.search(bore):
        problems.append(f"{label}: something was designated")
    ready = READY.search(bore)
    cone = BORE.search(bore)
    if ready is None or ready.group(1) != "BORE" or ready.group(3) != "none":
        problems.append(f"{label}: not ready in BORE without a designation: {ready.group(0) if ready else None}")
    elif cone is None:
        problems.append(f"{label}: no bore returns printed")
    else:
        hottest = cone.group(2).split(",")[0].split(":")[0]
        if ready.group(2) != hottest or ready.group(5) != hottest:
            problems.append(f"{label}: the candidate {ready.group(2)} (seeker {ready.group(5)}) is not the strongest "
                            f"bore return {hottest}")
        launch = _launch(bore, label, problems)
        if launch and (launch.group(3) != hottest or launch.group(6) != "true"):
            problems.append(f"{label}: launched at {launch.group(3)} acquired {launch.group(6)}, want {hottest} locked")
        _killed(bore, hottest, label, problems)

    _cued_kill(parts[2], "AV-8B AGM-65G on the FLIR", problems)
    # The AGM-65A's warhead leaves a T-80 with a tenth of its hit points.
    _cued_kill(parts[3], "A-10 AGM-65A by Enter", problems, kill=False)
    label = "A-10 AGM-65A at 6 nmi on A-G"
    _cued_kill(parts[4], label, problems, kill=False)
    end = END.search(parts[4])
    if end and not 0 <= float(end.group(2)) <= 40:
        problems.append(f"{label}: ended {end.group(2)} ft from the target, want at most 40")
    return problems


def harm_bore_problems(output: str) -> list[str]:
    parts = runs(output)
    if len(parts) != 2:
        return [f"{len(parts)} runs finished, want 2"]
    problems: list[str] = []
    for run, label, ace in ((parts[0], "F-4G HARM, ace radar", True), (parts[1], "F-4G HARM, novice radar", False)):
        if DESIGNATED.search(run):
            problems.append(f"{label}: something was designated")
        ready = READY.search(run)
        if ready is None or ready.group(1) != "BORE" or ready.group(3) != "none":
            problems.append(f"{label}: not ready in BORE without a designation: {ready.group(0) if ready else None}")
        elif ready.group(2) != STRAIGHT_FLUSH:
            problems.append(f"{label}: the cue's candidate is {ready.group(2)}, want the Straight Flush")
        cues = CUE.findall(run)
        if not cues or int(cues[-1][0]) < 2 or cues[-1][1] != STRAIGHT_FLUSH or cues[-1][2] != "0":
            problems.append(f"{label}: the HUD cue should list several emitters with the radar as the BORE "
                            f"candidate: {cues[-1:]}")
        elif STRAIGHT_FLUSH not in cues[-1][3]:
            problems.append(f"{label}: the radar is not on the cue's list")
        launch = _launch(run, label, problems)
        if launch and (launch.group(3) != STRAIGHT_FLUSH or launch.group(4) != "BORE"):
            problems.append(f"{label}: launched at {launch.group(3)} in {launch.group(4)}, want the radar in BORE")
        shut = [s for s in SHUTDOWN.findall(run) if s[1] == STRAIGHT_FLUSH and s[2] == "1"]
        if ace and not shut:
            problems.append(f"{label}: the ace radar did not shut down")
        if not ace and shut:
            problems.append(f"{label}: a novice radar shut down")
        if ace and shut:
            off = float(shut[0][0])
            after = [f for f in FLIGHT.findall(run) if float(f[0]) > off]
            if not after or any(f[1] != "Locked" or f[2] != STRAIGHT_FLUSH for f in after):
                problems.append(f"{label}: the HARM did not keep homing after the shutdown: {after[:3]}")
        _killed(run, STRAIGHT_FLUSH, label, problems)
    return problems


def harm_page_problems(output: str) -> list[str]:
    parts = runs(output)
    if len(parts) != 1:
        return [f"{len(parts)} runs finished, want 1"]
    run, label, problems = parts[0], "F-4G HARM page", []
    if not re.search(r"^a2g-run: missile AGM88\.JT kind harm .* channel Harm ", run, re.M):
        problems.append(f"{label}: the HARM page was not selected")
    designated = DESIGNATED.search(run)
    if designated is None or designated.group(1) != STRAIGHT_FLUSH:
        problems.append(f"{label}: T did not designate the Straight Flush: {designated.group(0) if designated else None}")
    cues = CUE.findall(run)
    if not cues or cues[-1][1] != STRAIGHT_FLUSH or cues[-1][2] != "1":
        problems.append(f"{label}: the cue should be CUED on the radar at release: {cues[-1:]}")
    _cued_kill(run, label, problems)
    return problems


def antiship_problems(output: str) -> list[str]:
    parts = runs(output)
    if len(parts) != 2:
        return [f"{len(parts)} runs finished, want 2"]
    problems: list[str] = []
    for run, label in ((parts[0], "F/A-18D AGM-84A"), (parts[1], "Rafale AM-39")):
        designated = DESIGNATED.search(run)
        if designated is None or designated.group(1) != KRIVAK:
            problems.append(f"{label}: the Krivak was not designated on the A-G page")
        launch = _launch(run, label, problems)
        if launch and (launch.group(3) != KRIVAK or launch.group(5) != "Midcourse"):
            problems.append(f"{label}: launched at {launch.group(3)} seeker {launch.group(5)}, want the Krivak midcourse")
        if not any(f[1] == "Pitbull" and f[2] == KRIVAK for f in FLIGHT.findall(run)):
            problems.append(f"{label}: the seeker never went active on the Krivak")
        end = _end(run, label, problems)
        if end and int(end.group(3)) >= int(end.group(4)):
            problems.append(f"{label}: the Krivak was not hit ({end.group(3)}/{end.group(4)})")
    return problems


def sarh_problems(output: str) -> list[str]:
    parts = runs(output)
    if len(parts) != 2:
        return [f"{len(parts)} runs finished, want 2"]
    problems: list[str] = []
    held, broken = parts
    _cued_kill(held, "MiG-23 AS-7 on the A-G track", problems)
    if any(f[3] != "0" for f in FLIGHT.findall(held)):
        problems.append("MiG-23 AS-7 on the A-G track: the missile went unguided")
    label = "MiG-23 AS-7, A-G off after launch"
    designated = DESIGNATED.search(broken)
    brk = BREAK.search(broken)
    if brk is None:
        problems.append(f"{label}: the A-G page was not turned off")
    else:
        at = float(brk.group(1))
        after = [f for f in FLIGHT.findall(broken) if float(f[0]) > at + 0.5]
        if not after or any(f[1] != "Lost" or f[3] != "1" or f[2] != "none" for f in after):
            problems.append(f"{label}: the missile kept guiding after the track dropped: {after[:2]}")
    end = _end(broken, label, problems)
    if end and int(end.group(3)) < int(end.group(4)):
        problems.append(f"{label}: the target was hit ({end.group(3)}/{end.group(4)})")
    if designated and designated.group(1) in KILL.findall(broken):
        problems.append(f"{label}: the designated target was killed")
    return problems


def uncued_sarh_problems(output: str) -> list[str]:
    problems = []
    if "unsupported release" not in output:
        problems.append("no unsupported manual trigger")
    if not re.search(r"a2g-run: launch .* target none mode BORE.* acquired false unguided false", output):
        problems.append("SARH did not launch with no target and a recoverable seeker")
    flights = FLIGHT.findall(output)
    if not any(f[1] == "Search" and f[2] == "none" and f[3] == "0" for f in flights):
        problems.append("no recorded unsupported straight-flight interval")
    if not any(f[1] == "Locked" and f[2] != "none" and f[3] == "0" for f in flights):
        problems.append("later A-G illumination did not acquire a target")
    return problems


def terrain_cruise_problems(output: str) -> list[str]:
    problems: list[str] = []
    _cued_kill(output, "Maverick ridge clearance", problems)
    heights = [int(h) for h in re.findall(r"^a2g-run: flight .* altitude (\d+) ", output, re.M)]
    if max(heights, default=0) < 1900:
        problems.append("the cruise missile did not climb above 1,900 ft over the ridge")
    end = END.search(output)
    if end and float(end.group(2)) > 40:
        problems.append(f"ridge shot ended {end.group(2)} ft from the target, want at most 40")
    return problems


def other_missile_problems(output: str) -> list[str]:
    parts = runs(output)
    if len(parts) != 3:
        return [f"{len(parts)} runs finished, want 3"]
    problems: list[str] = []
    _cued_kill(parts[0], "F-15 SLAM", problems)
    launch = _launch(parts[1], "A-4 Shrike", problems)
    if launch and launch.group(3) != STRAIGHT_FLUSH:
        problems.append("Shrike did not select the Straight Flush")
    _killed(parts[1], STRAIGHT_FLUSH, "A-4 Shrike", problems)
    _cued_kill(parts[2], "MiG-29 AS-15", problems)
    return problems


def scenarios() -> list[Scenario]:
    run = ["--a2g-run"]
    col = run + ["UKR", "QUCOL", "--speed", "350", "--seconds", "60"]
    site = run + ["VLA", "QVRDRI", "--surface-seed", "4", "--defenses", "3", "3", "--unit", STRAIGHT_FLUSH,
                  "--aircraft", "f4g", "--load", "AGM88", "--attack", "harm", "--from", "12", "--altitude", "15000",
                  "--speed", "450", "--seconds", "60"]
    # Finite-energy subsonic cruise needs more than the former 90-second window.
    fleet = run + ["UKR", "QUSFLT", "--unit", KRIVAK, "--attack", "antiship", "--from", "15", "--altitude", "5000",
                   "--speed", "450", "--seconds", "180"]
    return [
        Scenario(
            name="a2g-slam-shrike-as15", lane="flight",
            args=col + ["--aircraft", "f15", "--load", "AGM84E", "--station", "2", "--count", "2", "--attack", "maverick",
                        "--from", "6", "--altitude", "3000"],
            timeout=300,
            then=[
                Step(run + ["VLA", "QVRDRI", "--surface-seed", "4", "--defenses", "3", "3",
                            "--unit", STRAIGHT_FLUSH, "--aircraft", "a4e", "--load", "AGM45",
                            "--attack", "harm", "--from", "6", "--altitude", "5000",
                            "--speed", "450", "--seconds", "60"], timeout=300),
                Step(col + ["--aircraft", "mig29", "--load", "AS15", "--station", "2", "--count", "2", "--attack", "sarh",
                            "--from", "4", "--altitude", "3000"], timeout=300),
            ],
            check=other_missile_problems,
            notes="SLAM IR, Shrike anti-radiation and AS-15 SARH retain their ground guidance.",
        ),
        Scenario(
            name="a2g-sarh-uncued", lane="flight",
            args=col + ["--aircraft", "mig23", "--load", "AS7", "--attack", "sarh-uncued",
                        "--from", "4", "--altitude", "3000", "--speed", "400"],
            timeout=300, check=uncued_sarh_problems,
            notes="Player surface SARH launches with no A-G support and acquires when illumination begins later",
        ),
        Scenario(
            name="a2g-missile-burnout", lane="flight",
            args=col + ["--aircraft", "a10", "--load", "AGM65G", "--attack", "maverick", "--from", "6",
                        "--altitude", "3000"],
            timeout=300,
            expect=[r"^a2g-run: motors lit 1 burnt-out-in-flight 1 missile-smoke max-puffs [1-9]\d*$"],
            notes="Bug B7 (slice BF): a Maverick's motor lights, smokes and burns out before impact, which ends its "
                  "smoke and its drawn flame.",
        ),
        Scenario(
            name="a2g-terrain-cruise", lane="flight",
            args=run + ["VLA", "QVARMOR", "--aircraft", "f18", "--load", "AGM65A", "--attack", "maverick",
                        "--from", "6", "--altitude", "1100", "--heading", "60", "--speed", "350",
                        "--unit", "0x50000000", "--seconds", "90"],
            timeout=300,
            check=terrain_cruise_problems,
            notes="The Maverick climbs above 1,900 ft over the ridge, then resumes its profile and kills the tank.",
        ),
        Scenario(
            name="a2g-terrain-mask", lane="flight",
            args=run + ["VLA", "QVSUP", "--aircraft", "f18", "--load", "AGM65G", "--attack", "maverick",
                        "--from", "6", "--altitude", "300", "--heading", "180", "--speed", "350",
                        "--unit", "0x50000000", "--seconds", "10"],
            timeout=300,
            expect=[r"a2g-run: terrain setup .* mask [1-9][0-9]*$", r"a2g-run: no launch readiness TERRAIN MASK$"],
            forbid=[r"a2g-run: launch "],
            notes="A ridge near the target, skipped by the old eight-probe line, denies Maverick lock and launch.",
        ),
        Scenario(
            name="a2g-maverick", lane="flight",
            args=col + ["--aircraft", "a10", "--load", "AGM65G", "--attack", "maverick", "--from", "6",
                        "--altitude", "3000"],
            timeout=300,
            then=[
                Step(col + ["--aircraft", "a10", "--load", "AGM65G", "--attack", "maverick", "--designate", "none",
                            "--from", "4", "--altitude", "3000"], timeout=300),
                Step(col + ["--aircraft", "av8", "--load", "AGM65G", "--attack", "maverick", "--designate", "ir",
                            "--from", "4", "--altitude", "2000"], timeout=300),
                # View the column crosswise: the corrected bridge behind it is
                # now a valid visual pick from the old northbound approach.
                Step(col + ["--aircraft", "a10", "--load", "AGM65A", "--attack", "maverick", "--designate",
                            "visual", "--from", "3", "--altitude", "2000", "--heading", "90"], timeout=300),
                Step(col + ["--aircraft", "a10", "--load", "AGM65A", "--attack", "maverick",
                            "--from", "6", "--altitude", "3000"], timeout=300),
            ],
            check=maverick_problems,
            notes="Maverick CUED on the A-G page, in BORE, on the AV-8B's FLIR, by Enter, and AGM-65A at 6 nmi.",
        ),
        Scenario(
            name="a2g-harm-bore", lane="flight",
            args=site + ["--skill", "3"],
            timeout=300,
            then=[Step(site + ["--skill", "0"], timeout=300)],
            check=harm_bore_problems,
            notes="HARM fired in BORE at the SA-6 battery's radar: an ace radar shuts down and still dies.",
        ),
        Scenario(
            name="a2g-harm-page", lane="flight",
            args=site + ["--designate", "harm"],
            timeout=300,
            check=harm_page_problems,
            notes="HARM page (M), T to the Straight Flush, CUED launch.",
        ),
        Scenario(
            name="a2g-antiship", lane="flight",
            args=fleet + ["--aircraft", "f18", "--load", "AGM84A"],
            timeout=300,
            then=[Step(fleet + ["--aircraft", "rafale", "--load", "AM39"], timeout=300)],
            check=antiship_problems,
            notes="Harpoon and Exocet at the Krivak on the A-G track.",
        ),
        Scenario(
            name="a2g-sarh", lane="flight",
            args=col + ["--aircraft", "mig23", "--load", "AS7", "--attack", "sarh", "--from", "4",
                        "--altitude", "3000", "--speed", "400"],
            timeout=300,
            then=[Step(col + ["--aircraft", "mig23", "--load", "AS7", "--attack", "sarh", "--from", "4",
                              "--altitude", "3000", "--speed", "400", "--break-at", "2"], timeout=300)],
            check=sarh_problems,
            notes="AS-7 holding the A-G track, then losing it.",
        ),
    ]
