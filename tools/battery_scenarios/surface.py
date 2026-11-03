"""Lane: ai. Surface units: ground target resolution, layout and base-layout air defenses.

`surface-resolve-all` runs `--surface-dump --all`: every offered Quick Mission
ground target template of every theater resolved at every defense level with
three seeds, each theater's base layout, and each template placed in its
theater's scene at heavy defenses. It checks the counts against the retail
survey (template objects, `<sam>` and `<aaa>` slots, targets per template,
base-layout SAM and AAA by side), the 0, 25, 60 and 100 percent rolls, and that
a second run repeats every digest.

`surface-relocate-sweep` runs `--surface-dump --sweep`: every offered template
placed at heavy defenses with seeds 1 to 20. Every placement passes the site
rules, the anchored templates are exactly the expected ones and never move, at
least 60 percent of the other placements relocate, the batteries and trucks
keep their rules, each base layout forms its batteries, and a second run
repeats the digests.

`surface-class-sweep` runs `--surface-dump --classes`: the terrain class rule
(land units and structures on land, ships on water) over every offered
template placed at heavy defenses with seeds 1 to 20 and over each theater's
base layout. A unit the layout moved, a supply truck or a battery radar it
added must stand on its class; a unit still at its retail spot is the data's
and is not judged (docs/spec/surface-defenses.md, "Validity").

`surface-start-placement` runs `--surface-dump --starts`: one mission per
theater with a ground target and a 50 nm enemy distance. Red starts within
5 nm of the target; Blue starts 50 nm from Red toward its own side, heading
at the target; both on the map. See
docs/spec/surface-defenses.md.

`surface-area-sweep` runs `--surface-dump --area-sweep`: the enemy area
defenses of a Quick Mission with no ground target (B23) in every theater at
every strength with seeds 1 and 2. Every unit keeps the area rules (Red's side,
land, level, clear of layout objects and runways), every theater but the
Kurils (islands, mostly sea) stands every site, and a second run repeats the
digests. `surface-area-engage` flies past the first area site of a Ukraine
mission (heavy, seed 1, an SA-14) at 4,000 ft and expects it to launch.

`surface-grounding` (B13, 2026-10-12; slice GROUND, 2026-10-13) runs `--surface-dump
--grounding`, the grounding audit: every OT, NT and PT the import and the retail media hold, stood on flat
ground through the placement path and drawn through the scenery's own code (faces, the hit box, and a man's
sprites or cards at 300 ft and boosted at 6,000 ft). Everything but runways, roads and ships stands on the
bottom of its shape within 0.05 ft (the factory hall FACTD lifted 42.7 ft, the hangar 26.7 ft, the airfield
squad TROOPS 3.3 ft), that bottom agrees with the shape's F2 ground offset within 3 ft, and roads
and ships are not lifted. Bridges have their own shared-bottom and corridor audit.

`surface-small-arms-airfield` flies a 400 kt, 300 ft pass over an airfield target's squads (B17, BD2): one
squad at each experience, the squads around it at average and at ace, and a 1,000 ft, a 600 kt and a 1,500 ft
side pass. Their rounds carry tracers (every third round) and they only rarely hit a jet passing through
(about one round in a hundred; the riflemen lead it poorly, John 2026-10-12).

`surface-tank-aa` (John, 2026-10-24) puts one T-72 of Iraq's withdrawing column against aircraft: a helicopter
hovering 24 degrees above it is never taken and costs no round (the tank gun's 20 degree elevation), one hovering
low 0.5 km out draws its first round 4 s after the lock (the record's 1 s and the tank's 3 s lay on an aircraft),
every round at a 450 kt jet 200 ft over it leaves at 20 degrees or less, and a jet 2,500 ft over it draws none.

`surface-squad-killable` kills one airfield squad (BD2) with the A-4E's gun in a strafing pass, with an F/A-18's
Mk 82 in a dive, and with the trace's own kill at 23 s: every one is a kill of the squad's id, and a squad
killed in the middle of a pass fires no more.

`surface-flak-team` (nr-flak2, John 2026-10-14) runs `--flak-pass`, which flies a flight of two or four on a
scripted line over a team of heavy flak guns (KS-12 and KS-19 within a nautical mile of each other) or a lone
gun, every other armed unit laid down: North Vietnam's storage template, whose four guns form one team. A flight
of four heading in is engaged by the whole team (heading-in trigger), the shells spread over every member, the
lead lives, each gun's eight-shell opening barrage is 0.75 s a shell and spread round the flight, and every flak
knock is half the old one. Eight aircraft grazing the team's longest reach with one of them in range (under a
quarter) draw no fire; with two of them in range (a quarter) they are engaged; a four-ship grazing it with one
member in reach (a quarter, but under the minimum of two) draws none. Two players in seats heading in
both draw fire, and a lone KS-19 (a team of one) still fires at a flight.

The engagement scenarios run `--surface-trace`, which flies the player on a
scripted straight line past one surface unit and prints what its controller
does (phases, shots, bursts, radar, HARM rolls, battery changes), the RWR's
ground squares, locks and tone, decoy rolls and a summary line. They read the
retail records the import does not keep yet from the retail media, as the dump
does. Numbers they check are the record's and the AAA tuning table's
(docs/spec/surface-defenses.md).
"""
import math
import re

from battery import Scenario, Step

# Per template: (objects, targets before the rolls, <sam> slots, <aaa> slots),
# from the retail survey of the extracted FA_2.LIB (surface-AI round,
# 2026-10-10). Targets count the parked aircraft flagged as targets.
SURVEY = {
    "QBNOTH": (0, 0, 0, 0), "QBFLT": (20, 1, 0, 0), "QBAIR": (66, 7, 10, 10), "QBBRD": (34, 1, 10, 10),
    "QBXING": (48, 6, 10, 10), "QBACOL": (36, 8, 10, 10), "QBFAIR": (55, 9, 10, 10), "QBSPPY": (45, 10, 10, 10),
    "QBSHAR": (47, 1, 10, 10), "QCNOTH": (0, 0, 0, 0), "QCFAIR": (62, 5, 10, 10), "QCSCUD": (66, 6, 10, 10),
    "QCSUB": (36, 4, 10, 10), "QCLST": (32, 12, 10, 10), "QCCARG": (6, 4, 0, 0), "QCCMHQ": (51, 4, 10, 10),
    "QENOTH": (0, 0, 0, 0), "QESFLT": (7, 3, 0, 0), "QESAIR": (32, 4, 10, 10), "QELAIR": (45, 8, 10, 10),
    "QECMHQ": (31, 5, 10, 10), "QERDRI": (32, 7, 10, 10), "QEARMOR": (28, 5, 10, 10), "QECDEF": (34, 3, 10, 10),
    "QLFNOTH": (0, 0, 0, 0), "QLFCARG": (27, 3, 10, 10), "QLFPATR": (24, 4, 10, 10), "QLFSAM": (75, 6, 10, 10),
    "QLFFAIR": (60, 5, 10, 10), "QLFSTOR": (64, 17, 10, 10), "QLFCMHQ": (65, 1, 10, 10), "QFNOTH": (0, 0, 0, 0),
    "QFFLT": (14, 1, 0, 0), "QFSAIR": (37, 4, 10, 10), "QFLAIR": (44, 6, 10, 10), "QFSUP": (29, 5, 10, 10),
    "QFRDRI": (36, 8, 10, 10), "QFCMHQ": (31, 6, 10, 10), "QFFACT": (38, 9, 10, 10), "QGRNOTH": (0, 0, 0, 0),
    "QGRSAIR": (85, 10, 10, 10), "QGRPATR": (4, 4, 0, 0), "QGRRDR": (113, 4, 10, 10), "QGRCARG": (10, 3, 0, 0),
    "QGRSTOR": (116, 7, 10, 10), "QIRNOTH": (0, 0, 0, 0), "QIRRDR": (87, 6, 10, 10), "QIRFAIR": (85, 6, 10, 10),
    "QIRPOW": (47, 5, 10, 10), "QIRCCC": (105, 6, 10, 10), "QIRARM": (74, 7, 10, 10), "QIRSCUD": (94, 4, 10, 10),
    "QIRCWP": (65, 6, 10, 10), "QIRRETR": (94, 8, 10, 10), "QKNOTH": (0, 0, 0, 0), "QKSFLT": (5, 1, 0, 0),
    "QKLFLT": (16, 1, 0, 0), "QKSCFT": (26, 6, 10, 10), "QKSUB": (15, 4, 6, 5), "QKPLNGR": (22, 4, 10, 8),
    "QKSILO": (34, 7, 10, 10), "QKARMOR": (28, 8, 10, 10), "QTNOTH": (0, 0, 0, 0), "QTBARG": (49, 5, 4, 5),
    "QTCARGO": (17, 3, 5, 0), "QTBRDG": (46, 1, 6, 4), "QTBUNK": (63, 6, 7, 8), "QTCOMM": (36, 2, 6, 9),
    "QTSTRG": (36, 6, 6, 6), "QTTRUCK": (48, 13, 6, 4), "QTAAA": (66, 8, 5, 0), "QTSAM": (69, 4, 0, 4),
    "QSPNOTH": (0, 0, 0, 0), "QSPFAIR": (82, 8, 10, 10), "QSPSAM": (98, 9, 10, 10), "QSPASA": (70, 12, 10, 10),
    "QSPFRU": (86, 5, 10, 10), "QSPSUP": (50, 15, 10, 10), "QSPCMHQ": (61, 4, 10, 10), "QAPNOTH": (0, 0, 0, 0),
    "QAPFAIR": (59, 5, 10, 10), "QAPBLK": (27, 2, 10, 10), "QAPPATR": (55, 5, 10, 10), "QAPHELO": (67, 8, 10, 10),
    "QAPSAM": (76, 4, 10, 10), "QAPCMHQ": (99, 15, 10, 10), "QPGNOTH": (0, 0, 0, 0), "QPGPATR": (24, 4, 10, 10),
    "QPGFAIR": (74, 4, 10, 10), "QPGSAM": (127, 9, 10, 10), "QPGSRUN": (103, 9, 10, 10), "QPGRDR": (70, 5, 10, 10),
    "QPGWSHP": (36, 2, 10, 10), "QNSNOTH": (0, 0, 0, 0), "QNSFAIR": (91, 7, 10, 10), "QNSARM": (86, 8, 10, 10),
    "QNSFOA": (97, 8, 10, 10), "QNSBORD": (70, 10, 10, 10), "QNSCOL": (46, 8, 10, 10), "QNSSUP": (90, 14, 10, 10),
    "QWTNOTH": (0, 0, 0, 0), "QWTFAIR": (66, 9, 10, 10), "QWTPATR": (6, 6, 0, 0), "QWTHYDO": (11, 5, 0, 0),
    "QWTWARS": (11, 2, 0, 0), "QWTCARG": (9, 3, 0, 0), "QWTLAND": (75, 13, 10, 10), "QUNOTH": (0, 0, 0, 0),
    "QUSFLT": (5, 1, 0, 0), "QULFLT": (16, 1, 0, 0), "QUCITY": (39, 39, 12, 11), "QUFACT": (19, 3, 4, 3),
    "QUSTRIP": (58, 10, 10, 10), "QUCOL": (29, 3, 10, 10), "QUNUKE": (34, 6, 15, 11), "QUBRI": (15, 1, 3, 7),
    "QVNOTH": (0, 0, 0, 0), "QVSFLT": (7, 3, 0, 0), "QVSAIR": (43, 4, 10, 10), "QVLAIR": (55, 11, 10, 10),
    "QVCMHQ": (27, 6, 10, 10), "QVARMOR": (28, 8, 10, 10), "QVRDRI": (30, 5, 10, 10), "QVSUP": (29, 5, 10, 10),
}

# Defense slots that are themselves targets: `~QUCITY` flags 11 <AAA> and 12
# <SAM> slots, so with defenses at none it has 16 targets.
DEFENSE_TARGETS = {"QUCITY": 23}

# Base-layout SAM and AAA units, (Redfor, Blue), from the same survey; all
# 321 go active on both sides (John, 2026-10-10).
BASE_DEFENSES = {
    "BAL": (10, 20), "CUB": (19, 4), "EGY": (0, 0), "LFA": (5, 2), "FRA": (0, 0),
    "GRE": (10, 14), "IRA": (13, 10), "KURILE": (2, 0), "TVIET": (98, 0), "SPA": (8, 14),
    "APA": (12, 9), "PGU": (13, 9), "NSK": (14, 16), "WTA": (5, 14), "UKR": (0, 0), "VLA": (0, 0),
}

ALL_LINE = re.compile(
    r"^surface-all: (\S+) (\S+) level (\d) seed (\d+) objects (\d+) sam-slots (\d+) aaa-slots (\d+) "
    r"targets (\d+) manned (\d+) removed (\d+) units (\d+) parked (\d+) left-out (\d+) digest (0x[0-9a-f]+)$",
    re.M,
)
BASE_LINE = re.compile(r"^surface-base: (\S+) units (\d+) sam-aaa-red (\d+) sam-aaa-blue (\d+) ", re.M)


def resolve_all_problems(output: str) -> list[str]:
    """Counts against the survey, the roll shares, and digests that repeat."""
    problems: list[str] = []
    first, _, again = output.partition("$ then 1:")
    digests: dict[tuple, str] = {}
    shares = {1: [0, 0], 2: [0, 0]}
    for m in ALL_LINE.finditer(first):
        stem, level, seed = m.group(2), int(m.group(3)), int(m.group(4))
        objects, sam, aaa, targets, manned, removed = (int(m.group(i)) for i in range(5, 11))
        digests[(stem, level, seed)] = m.group(14)
        expect = SURVEY.get(stem)
        if expect is None:
            problems.append(f"{stem}: not in the retail survey")
            continue
        want_objects, want_targets, want_sam, want_aaa = expect
        if (objects, sam, aaa) != (want_objects, want_sam, want_aaa):
            problems.append(f"{stem}: {objects} objects, {sam}/{aaa} slots; survey {want_objects}, {want_sam}/{want_aaa}")
        if manned + removed != sam + aaa:
            problems.append(f"{stem} level {level} seed {seed}: {manned} manned and {removed} removed of {sam + aaa}")
        if level == 3 and (targets, manned, removed) != (want_targets, sam + aaa, 0):
            problems.append(f"{stem} heavy: {targets} targets, {manned} manned; survey {want_targets}, all {sam + aaa}")
        if level == 0:
            want = want_targets - DEFENSE_TARGETS.get(stem, 0)
            if (targets, manned) != (want, 0):
                problems.append(f"{stem} none: {targets} targets, {manned} manned; expected {want}, 0")
        if level in shares:
            shares[level][0] += manned
            shares[level][1] += sam + aaa
    missing = sorted(set(SURVEY) - {stem for stem, _, _ in digests})
    if missing:
        problems.append(f"templates never resolved: {missing}")
    for level, percent in ((1, 25.0), (2, 60.0)):
        manned, slots = shares[level]
        if slots and abs(100.0 * manned / slots - percent) > 3.0:
            problems.append(f"level {level}: {100.0 * manned / slots:.1f} percent of {slots} slots manned, retail {percent:.0f}")
    bases = {m.group(1): (int(m.group(3)), int(m.group(4))) for m in BASE_LINE.finditer(first)}
    for theater, want in BASE_DEFENSES.items():
        if bases.get(theater) != want:
            problems.append(f"{theater} base layout SAM and AAA (Redfor, Blue) {bases.get(theater)}, survey {want}")
    # The second run (seed 1 only) gives the same digests.
    repeats = 0
    for m in ALL_LINE.finditer(again):
        key = (m.group(2), int(m.group(3)), int(m.group(4)))
        repeats += 1
        if digests.get(key) != m.group(14):
            problems.append(f"{key}: digest {digests.get(key)} then {m.group(14)}")
    if repeats != len(SURVEY) * 4:
        problems.append(f"second run resolved {repeats} times, expected {len(SURVEY) * 4}")
    return problems


TRACE = re.compile(r"^surface-trace: t=([0-9.]+) (.*)$", re.M)
SUMMARY = re.compile(r"^surface-trace: summary (.*)$", re.M)
SUBJECT = re.compile(r"^surface-trace: subject (0x[0-9a-f]+) (\S+) side (\d+)", re.M)
RADAR = re.compile(r"^surface-trace: radar (0x[0-9a-f]+)$", re.M)


def objective_audit_problems(output: str) -> list[str]:
    parts = runs(output)
    if len(parts) != 2:
        return ["expected cached and direct objective audit runs"]

    def rows(text):
        return [line for line in text.splitlines() if line.startswith(("objective-case:", "finding\t"))]

    if rows(parts[0]) != rows(parts[1]):
        return ["cached and direct objective audit cases/findings/digests differ"]
    findings = [line.split("\t") for line in parts[0].splitlines() if line.startswith("finding\t")]
    # Independent ceilings accept improvements, while new regressions fail.
    ceilings = {
        "land-on-water": 0, "outside-map": 0, "footprint-outside-map": 0,
        "footprint-terrain-class": 0, "vehicle-footprint-slope": 0,
        "vehicle-runway-clearance": 0, "vehicle-object-clearance": 0,
    }
    counted = 0
    for rule, ceiling in ceilings.items():
        count = sum(row[7].startswith(("0x580", "0x5c0")) and row[-1] == rule for row in findings)
        counted += count
        if count > ceiling:
            return [f"generated support {rule}: {count} exceeds ceiling {ceiling}"]
    pattern = r"^objective-audit: cases (\d+) findings \d+ errors (\d+)$"
    summaries = re.findall(pattern, parts[0], re.M)
    if len(summaries) != 1 or summaries[0][0] != "264":
        return ["missing complete 264-case audit summary"]
    if re.findall(pattern, parts[1], re.M) != summaries:
        return ["cached and direct audit summaries differ or are incomplete"]
    # The remaining contracts (determinism, final scene, runway datums and
    # relocation) all have zero allowance. Slope headroom cannot mask them.
    if int(summaries[0][1]) != counted:
        return ["placement contract failures outside the allowed categories"]

    return []


def objective_cache_kuril_problems(output: str) -> list[str]:
    """Final front availability can change while the raw front stays identical."""
    parts = runs(output)
    if len(parts) != 3:
        return ["expected the Kuril matrix and two independent scene builds"]
    if not re.search(r"^objective-audit: cases 4224 findings \d+ errors 0$", parts[0], re.M):
        return ["incomplete Kuril audit or new placement contract failures"]
    for direct, (distance, aaa, sam, seed) in zip(parts[1:], [(5, 0, 2, 2), (50, 3, 0, 3)]):
        pattern = (rf"^objective-case: KURILE QKSILO separation {distance} aaa {aaa} sam {sam} seed {seed} "
                   r"units \d+ parked \d+ findings \d+ digest (\S+)$")
        cached = re.findall(pattern, parts[0], re.M)
        built = re.findall(r"^surface: digest (\S+)$", direct, re.M)
        if len(cached) != 1 or built != cached:
            return [f"Kuril final-front cache differs from direct build at {distance} nm, AAA {aaa}, SAM {sam}, seed {seed}"]
    return []


def runs(output: str) -> list[str]:
    """The output of each run: the main command, then each `then` step."""
    return re.split(r"^\$ then \d+:.*$", output, flags=re.M)


def summary(run: str) -> dict[str, str]:
    m = SUMMARY.search(run)
    if not m:
        return {}
    words = m.group(1).split()
    return dict(zip(words[::2], words[1::2]))


def events(run: str) -> list[tuple[float, str]]:
    return [(float(m.group(1)), m.group(2)) for m in TRACE.finditer(run)]


def subject(run: str) -> str:
    m = SUBJECT.search(run)
    return m.group(1) if m else "?"


def first(run: str, pattern: str) -> float | None:
    for t, text in events(run):
        if re.search(pattern, text):
            return t
    return None


def near(a: float | None, b: float, tolerance: float = 0.02) -> bool:
    return a is not None and abs(a - b) <= tolerance


def sa6_engage_problems(output: str) -> list[str]:
    """SA-6 (NPC 10 s search, 36 s first preparation; record 5 s lock, 15 s
    between salvos): the phases at the record times, a launch, the RWR's
    ground square, the lock tone and then the missile tone."""
    problems: list[str] = []
    run = runs(output)[0]
    unit = subject(run)
    # A launcher in a battery fights through its battery's controller.
    prepare = first(run, rf"^(phase {unit} w0|battery \d+) Prepare")
    track = first(run, rf"^(phase {unit} w0|battery \d+) Track")
    shots = [t for t, text in events(run) if text.startswith(f"shot {unit} ") and " rounds 1 " in text]
    if prepare is None or not near(track, prepare + 36.0):
        problems.append(f"preparation {prepare} to lock {track}: expected 36 s")
    if not shots or not near(shots[0], (track or 0) + 5.0):
        problems.append(f"first launch {shots[:1]} not 5 s after the lock at {track}")
    if len(shots) < 2 or not near(shots[1] - shots[0], 15.0):
        problems.append(f"launches {shots}: expected 15 s apart")
    s = summary(run)
    for key in ("ground-square", "radar-lock-tone", "inbound-tone"):
        if s.get(key) != "true":
            problems.append(f"{key} {s.get(key)}")
    lock = first(run, r"^rwr .*tone Some\(RadarLock\)")
    inbound = first(run, r"^rwr .*tone Some\(RadarInbound\)")
    if lock is None or inbound is None or lock >= inbound:
        problems.append(f"lock tone at {lock} should come before the missile tone at {inbound}")
    return problems


def sa2_low_problems(output: str) -> list[str]:
    """Under the SA-2's 3,000 ft seeker floor: its radar is on and the RWR shows
    it, but it never launches."""
    problems: list[str] = []
    run = runs(output)[0]
    m = RADAR.search(run)
    radar = m.group(1) if m else subject(run)
    if first(run, rf"^radar {radar} on") is None:
        problems.append("the SA-2's radar never came on")
    s = summary(run)
    if s.get("ground-square") != "true":
        problems.append("no RWR ground square for the SA-2")
    if s.get("missiles") != "0":
        problems.append(f"{s.get('missiles')} missiles launched under the floor")
    return problems


def chaff_flare_problems(output: str) -> list[str]:
    """SA-6 and SA-7 launches against a jet dispensing chaff and flares: the
    radar missile rolls against chaff, the infrared one against flares, and
    some are decoyed."""
    problems: list[str] = []
    run = runs(output)[0]
    rolls = [text for _, text in events(run) if text.startswith("decoy ")]
    if not any(r.startswith("decoy Chaff") and " SA6.JT " in r for r in rolls):
        problems.append("no chaff roll against an SA-6")
    if not any(r.startswith("decoy Flare") and " SA7.JT " in r for r in rolls):
        problems.append("no flare roll against an SA-7")
    if any(r.startswith("decoy Chaff") and (" SA7.JT " in r or " SA13.JT " in r or " SA14.JT " in r) for r in rolls):
        problems.append("chaff rolled against an infrared missile")
    if any(r.startswith("decoy Flare") and " SA6.JT " in r for r in rolls):
        problems.append("a flare rolled against a radar missile")
    if not any(r.endswith("decoyed true") for r in rolls):
        problems.append("nothing was decoyed")
    return problems


def harm_problems(output: str) -> list[str]:
    """An AGM-88 fired from the jet in BORE (the HARM boresight cue, no
    designation) at an SA-6 battery's Straight Flush: an ace radar shuts down
    and the battery goes Blind, but the HARM keeps homing on the radar after it
    stops emitting (retail data, docs/formats/radar.md "HARM page") and
    destroys it, so the battery stays Blind; a killed radar blinds it for good."""
    problems: list[str] = []
    harm, killed = (runs(output) + ["", ""])[:2]
    m = RADAR.search(harm)
    radar = m.group(1) if m else "?"
    launched = first(harm, rf"^harm launched at {radar} mode BORE$")
    if launched is None:
        problems.append("the HARM was not fired in BORE at the radar")
    shut = first(harm, rf"^harm-roll {radar} .* shutdown$")
    if shut is None:
        problems.append("no HARM shutdown roll succeeded")
    else:
        if not near(first(harm, rf"^radar {radar} off"), shut):
            problems.append("the radar did not go off with the shutdown")
        if not near(first(harm, r"^battery \d+ Blind"), shut):
            problems.append("the battery did not go Blind with the shutdown")
        dead = first(harm, rf"^destroyed {radar}$")
        if dead is None or dead <= shut:
            problems.append(f"the HARM did not keep homing and destroy the radar after the shutdown ({dead})")
        back = [t for t, text in events(harm) if text == f"radar {radar} on" and t > shut]
        if back:
            problems.append(f"the radar came back at {back[0]} after its shutdown")
        after = [text for t, text in events(harm) if dead is not None and t > dead and text.startswith("battery ")]
        if after:
            problems.append(f"the battery left Blind after its radar died: {after[:2]}")
    kill = first(killed, r"^killed ")
    blind = first(killed, r"^battery \d+ Blind")
    if kill is None or not near(blind, kill):
        problems.append(f"killing the radar at {kill} did not blind the battery ({blind})")
    after = [text for t, text in events(killed) if kill is not None and t > kill and text.startswith("battery ")]
    if after:
        problems.append(f"the battery left Blind after its radar died: {after[:2]}")
    if any(t > (kill or 0) for t, text in events(killed) if text.startswith("shot ")):
        problems.append("a launch after the radar died")
    return problems


def battery_sa2_problems(output: str) -> list[str]:
    """A North Vietnam SA-2 site with its GCI radar as a battery: the radar is
    the RWR's square, never the launcher; with the radar killed in daylight it
    launches on its optical backup with no lock tone; at night it does not."""
    problems: list[str] = []
    live, optical, night = (runs(output) + ["", "", ""])[:3]
    unit = subject(live)
    m = RADAR.search(live)
    radar = m.group(1) if m else "?"
    squares = [text for _, text in events(live) if text.startswith("rwr ground-squares")]
    if not any(f'"{radar}"' in text for text in squares):
        problems.append(f"the radar {radar} never showed on the RWR")
    if any(f'"{unit}"' in text for text in squares):
        problems.append(f"the launcher {unit} showed on the RWR")
    if int(summary(live).get("missiles", "0")) == 0 or summary(live).get("radar-lock-tone") != "true":
        problems.append(f"the battery did not lock and launch: {summary(live)}")
    if int(summary(live).get("hits", "0")) == 0:
        problems.append("live SA-2 battery registered no hits")
    s = summary(optical)
    if int(s.get("missiles", "0")) == 0:
        problems.append("no optical launch in daylight")
    if s.get("lock-tone") != "false":
        problems.append("a lock tone from a blind battery")
    if first(optical, r"^battery \d+ .*optical") is None:
        problems.append("the battery never went to its optical backup")
    if summary(night).get("missiles") != "0" or first(night, r"^battery \d+ Blind") is None:
        problems.append(f"at night a blind SA-2 battery fired or did not go Blind: {summary(night)}")
    return problems


def zsu23_problems(output: str) -> list[str]:
    """ZSU-23 (AAA tuning row: 99-round bursts at 3,400 rpm, a 2,000-round
    magazine, every third round a tracer): bursts, tracers, hits, magazine."""
    problems: list[str] = []
    run = runs(output)[0]
    unit = subject(run)
    bursts = [text for _, text in events(run) if text.startswith(f"burst {unit} ")]
    full = [b for b in bursts if " rounds 99 " in b]
    if not full:
        problems.append(f"no full 99-round burst: {bursts[:3]}")
    for b in full:
        rpm = float(b.rsplit("rpm ", 1)[1])
        if abs(rpm - 3400) > 34:
            problems.append(f"burst rate {rpm} rpm, table 3,400")
    s = summary(run)
    rounds, tracers = int(s.get("rounds", "0")), int(s.get("tracers", "0"))
    if abs(tracers - rounds / 3) > 1:
        problems.append(f"{tracers} tracers in {rounds} rounds, every third")
    if int(s.get("hits", "0")) == 0:
        problems.append("no hits registered")
    m = re.search(rf"^surface-trace: stock {unit} m0 loaded (\d+) reserve (\d+)$", run, re.M)
    if not m or int(m.group(1)) + rounds != 2000 or m.group(2) != "2":
        problems.append(f"magazine: {m.group(0) if m else None} after {rounds} rounds of 2,000")
    return problems


# Tanks against aircraft (John, 2026-10-24: "max elevation of fire set to like
# 20 degrees up"; tanks "should be a surprise if they kill you"): the T-72's
# ceiling, its 1 s lock plus 3 s lay, and the jet passes the check flies.
TANK_MAX_ELEVATION_DEG = 20.0
TANK_LAY_S = 4.0  # the record's 1 s lock and the 3 s lay before the first round
TANK_EYE_FT = 10.0  # a unit's sight above its reference point
TANK_JET_FROM_FT = 3 * 6076.0
TANK_JET_FPS = 450 * 1.6878
TANK_JET_ALTITUDE_FT = 200.0


def tank_aa_problems(output: str) -> list[str]:
    """A T-72 against aircraft: a helicopter hovering 24 degrees up is not
    engaged and costs no round; one hovering low 0.5 km out draws fire 4 s
    after the lock (lock and lay); every round at a 450 kt jet 200 ft over it
    leaves at 20 degrees or less; a jet passing 2,500 ft over it draws none."""
    problems: list[str] = []
    found = runs(output)
    if len(found) != 4:
        return [f"expected 4 runs (high hover, low hover, low jet, high jet), got {len(found)}"]
    high, low, jet, over = found
    for name, run in (("high hover", high), ("low hover", low), ("low jet", jet), ("high jet", over)):
        if "T72.JT" not in run:
            problems.append(f"{name}: the subject does not fire T72.JT")
    unit = subject(high)
    s = summary(high)
    if s.get("rounds") != "0":
        problems.append(f"high hover: {s.get('rounds')} rounds at a helicopter 24 degrees up")
    if first(high, rf"^phase {unit} w0 (Prepare|Track)") is not None:
        problems.append("high hover: the tank took a target above its gun's elevation")
    if not re.search(rf"^surface-trace: stock {unit} m0 loaded 22 reserve 2$", high, re.M):
        problems.append("high hover: the magazine is not full")
    unit = subject(low)
    track, shot = first(low, rf"^phase {unit} w0 Track"), first(low, rf"^shot {unit} ")
    if track is None or shot is None:
        problems.append(f"low hover: track {track}, first round {shot}")
    elif not near(shot - track, TANK_LAY_S, 0.05):
        problems.append(f"low hover: first round {shot - track:.2f} s after the lock, want {TANK_LAY_S}")
    unit = subject(jet)
    shots = [t for t, text in events(jet) if text.startswith(f"shot {unit} ")]
    if not shots:
        problems.append("low jet: no round fired")
    for t in shots:
        horizontal = abs(TANK_JET_FROM_FT - TANK_JET_FPS * t)
        elevation = math.degrees(math.atan2(TANK_JET_ALTITUDE_FT - TANK_EYE_FT, horizontal))
        if elevation > TANK_MAX_ELEVATION_DEG + 0.5:
            problems.append(f"low jet: a round at {t:.2f} s, {elevation:.1f} degrees up")
    if summary(over).get("rounds") != "0":
        problems.append(f"high jet: {summary(over).get('rounds')} rounds at a jet always above 20 degrees")
    return problems


SMALL_ARMS_PLAYER_HP = 232  # an F/A-18's hit points
# What small arms may do to a jet (fitted bounds around the measured 0.3 to 1.3
# percent of rounds on a pass straight over a squad at 400 kt and 300 ft, John
# 2026-10-12: "RARELY hit an aircraft passing through"; the visual-gun aim
# error hit 15 to 26 of 40 rounds, the first nerf 5 to 13).
SMALL_ARMS_MAX_HITS_PER_SQUAD = 3  # of about 40 rounds
SMALL_ARMS_MAX_HIT_SHARE = 0.03  # of every round fired by the four single squads
SMALL_ARMS_MAX_FIELD_HP_LOSS = 0.25  # of a jet, one low pass over a whole field


def small_arms_problems(output: str) -> list[str]:
    """Airfield small arms (B17 and BD2, John, 2026-10-12): a squad's rounds
    carry a tracer on every third round; a squad at each experience rarely
    hits a 400 kt jet at 300 ft (and never more than a few rounds of 40); a
    whole field's squads together cost a jet a small part of its hit points
    in a low pass; nothing to speak of at 1,000 ft, at 600 kt or 1,500 ft to
    the side."""
    problems: list[str] = []
    found = runs(output)
    if len(found) != 9:
        return [f"expected 9 runs (4 experiences, 2 fields, 3 passes), got {len(found)}"]
    rounds_all = hits_all = 0
    for skill, run in enumerate(found[:4]):
        unit = subject(run)
        if "SMLARMS.JT" not in run:
            problems.append(f"skill {skill}: the subject {unit} does not fire SMLARMS")
        s = summary(run)
        rounds, hits = int(s.get("rounds", "0")), int(s.get("hits", "0"))
        tracers = int(s.get("tracers", "0"))
        if rounds < 20:
            problems.append(f"skill {skill}: only {rounds} rounds fired")
            continue
        if abs(tracers - rounds / 3) > 2:
            problems.append(f"skill {skill}: {tracers} tracers in {rounds} rounds, every third")
        if hits > SMALL_ARMS_MAX_HITS_PER_SQUAD:
            problems.append(f"skill {skill}: {hits} of {rounds} rounds hit, above {SMALL_ARMS_MAX_HITS_PER_SQUAD}")
        rounds_all += rounds
        hits_all += hits
    if rounds_all and hits_all / rounds_all > SMALL_ARMS_MAX_HIT_SHARE:
        problems.append(f"the four squads hit {hits_all} of {rounds_all} rounds, above {SMALL_ARMS_MAX_HIT_SHARE:.0%}")
    for name, run in (("average field", found[4]), ("ace field", found[5])):
        field = summary(run)
        rounds, tracers = int(field.get("rounds", "0")), int(field.get("tracers", "0"))
        hp = int(field.get("player-hp", "0"))
        if rounds < 80 or abs(tracers - rounds / 3) > 6:
            problems.append(f"{name}: {tracers} tracers in {rounds} rounds")
        lost = 1 - hp / SMALL_ARMS_PLAYER_HP
        if lost > SMALL_ARMS_MAX_FIELD_HP_LOSS:
            problems.append(f"{name}: the jet lost {lost:.0%} of its hit points in one pass, above {SMALL_ARMS_MAX_FIELD_HP_LOSS:.0%}")
        rounds_all += rounds
        hits_all += int(field.get("hits", "0"))
    if hits_all == 0:
        problems.append("small arms never hit: they must still be a nuisance to a low jet")
    for name, run in (("1,000 ft", found[6]), ("600 kt", found[7]), ("1,500 ft to the side", found[8])):
        s = summary(run)
        if int(s.get("hits", "0")) > 2:
            problems.append(f"{name}: {s.get('hits')} hits of {s.get('rounds')} rounds, expected almost none")
    return problems


KILL_LINE = re.compile(r"^a2g-run: kill (0x[0-9a-f]+) class (0x[0-9a-f]+)$", re.M)
SQUAD_ID = "0x50000021"


def squad_killable_problems(output: str) -> list[str]:
    """An airfield squad (TROOPS, 75 hit points) can be killed (BD2, John
    2026-10-12): by the A-4E's gun in a strafing pass, by an F/A-18's Mk 82,
    and (in the trace) it fires no more once it has been killed. Each kill is
    reported with the squad's id and the men's class word 0x0040."""
    problems: list[str] = []
    found = runs(output)
    if len(found) != 3:
        return [f"expected 3 runs (gun, bomb, trace kill), got {len(found)}"]
    for name, run in zip(("gun", "bomb"), found):
        kills = [(m.group(1), m.group(2)) for m in KILL_LINE.finditer(run)]
        if (SQUAD_ID, "0x0040") not in kills:
            problems.append(f"{name}: the squad {SQUAD_ID} was not killed ({kills})")
        if f"a2g-run: target {SQUAD_ID} hp 0/75" not in run:
            problems.append(f"{name}: the squad's hit points did not reach 0 of 75")
    run = found[2]
    killed = first(run, rf"^killed {SQUAD_ID}$")
    if not near(killed, 23.0, 0.05):
        problems.append(f"the trace kill at {killed}, expected 23 s")
        return problems
    before = [t for t, text in events(run) if text.startswith(f"burst {SQUAD_ID} ") and t < killed]
    after = [t for t, text in events(run) if text.startswith(f"burst {SQUAD_ID} ") and t > killed]
    if len(before) < 2:
        problems.append(f"the squad fired {len(before)} bursts before it was killed, expected two")
    if after:
        problems.append(f"the squad fired {len(after)} bursts after it was killed (first at {after[0]} s)")
    if int(summary(run).get("rounds", "0")) != 5 * len(before):
        problems.append(f"{summary(run).get('rounds')} rounds fired, expected {5 * len(before)} (all before the kill)")
    return problems


def stock(run: str, unit: str) -> list[tuple[int, str]]:
    """The final (loaded, reserve) of each of a unit's hardpoints."""
    return [
        (int(m.group(1)), m.group(2))
        for m in re.finditer(rf"^surface-trace: stock {unit} m\d+ loaded (\d+) reserve (\S+)$", run, re.M)
    ]


def resupply_problems(output: str) -> list[str]:
    """Resupply (John, 2026-10-10) on the Ukraine factory's seed 2 group, whose
    SA-6 battery and ZSU-23 each have a supply truck within 0.1 mile.

    Run 1: the SA-6 battery starts with empty rails; its MISTRK refills them all
    after the 420 s rearm timer, and the battery then fires. Run 2: the trucks
    are destroyed at 100 s; the rails stay empty and nothing fires. Run 3: the
    ZSU-23 starts with no spare magazines and an empty magazine; the truck adds
    one every 120 s. Run 4: the same gun with its magazine empty, the jet in
    range: it swaps in 120 s, fires the whole magazine, and swaps again 120 s
    after that, from the reserve the truck has refilled. Run 5: no truck, no
    magazine and no reserve: Empty, silent."""
    problems: list[str] = []
    rearm, cut, refill, swaps, silent = (runs(output) + [""] * 5)[:5]
    unit = subject(rearm)
    if not re.search(r"^surface-trace: trucks (0x[0-9a-f]+ ?)+$", rearm, re.M):
        problems.append("the SA-6 has no truck within reach")
    when = [t for t, text in events(rearm) if text == f"rearm {unit}"]
    if len(when) != 1 or not near(when[0], 420.0, 0.05):
        problems.append(f"SA-6 rearm at {when}: expected one, at 420 s")
    launch = first(rearm, rf"^shot {unit} ")
    if launch is None or launch < 420.0:
        problems.append(f"SA-6 launch at {launch}: expected after the rearm")
    if int(summary(rearm).get("missiles", "0")) != 3:
        problems.append(f"SA-6 fired {summary(rearm).get('missiles')} of its 3 missiles after the rearm")
    if stock(rearm, unit) != [(0, "0")]:
        problems.append(f"SA-6 stock after its three missiles {stock(rearm, unit)}")

    s = summary(cut)
    if first(cut, r"^killed truck") is None or s.get("rearms") != "0" or s.get("missiles") != "0":
        problems.append(f"with the trucks destroyed: {s}")
    if stock(cut, subject(cut)) != [(0, "0")]:
        problems.append(f"with the trucks destroyed the rails were {stock(cut, subject(cut))}")

    gun = subject(refill)
    when = [t for t, text in events(refill) if text.startswith(f"refill {gun} ")]
    if len(when) != 2 or not near(when[0], 120.0, 0.05) or not near(when[1], 240.0, 0.05):
        problems.append(f"ZSU-23 reserve refills at {when}: expected 120 s and 240 s")
    if stock(refill, gun) != [(0, "2")]:
        problems.append(f"ZSU-23 stock {stock(refill, gun)}: the magazine is not topped up, the reserve holds two")

    gun = subject(swaps)
    reloads = [t for t, text in events(swaps) if re.match(rf"phase {gun} w0 Reload", text)]
    done = [t for t, text in events(swaps) if text.startswith(f"swap {gun} ")]
    if len(reloads) != 2 or len(done) != 2 or any(not near(b - a, 120.0, 0.02) for a, b in zip(reloads, done)):
        problems.append(f"ZSU-23 swaps {reloads} to {done}: expected 120 s each")
    s = summary(swaps)
    if s.get("rounds") != "2000" or s.get("refills") != "1":
        problems.append(f"ZSU-23 fired {s.get('rounds')} rounds with {s.get('refills')} refills: expected 2000 and 1")
    if stock(swaps, gun) != [(2000, "1")]:
        problems.append(f"ZSU-23 stock {stock(swaps, gun)}: expected a full magazine and one spare")

    gun = subject(silent)
    s = summary(silent)
    if first(silent, rf"^phase {gun} w0 Empty") is None or s.get("rounds") != "0" or s.get("swaps") != "0":
        problems.append(f"ZSU-23 with no truck and no reserve did not stay silent: {s}")
    return problems


def flak_problems(output: str) -> list[str]:
    """KS-19 over North Vietnam's AAA emplacement: an eight-shell opening
    barrage, single shells after, flak bursts high and no tracers; at 3,000 ft,
    under the 4,000 ft floor, it holds fire."""
    problems: list[str] = []
    high, low = (runs(output) + ["", ""])[:2]
    unit = subject(high)
    shots = [text for _, text in events(high) if text.startswith(f"shot {unit} ")]
    opening = [s for s in shots if " opening" in s]
    if len(opening) != 8 or shots[:8] != opening:
        problems.append(f"opening barrage {len(opening)} shells, expected the first 8")
    if len(shots) <= 8:
        problems.append("no shells after the opening barrage")
    s = summary(high)
    if s.get("tracers") != "0":
        problems.append(f"{s.get('tracers')} flak tracers")
    if int(s.get("flak-bursts", "0")) == 0 or s.get("min-burst-ft") == "none" or float(s["min-burst-ft"]) < 4000:
        problems.append(f"flak bursts {s.get('flak-bursts')} lowest {s.get('min-burst-ft')} ft")
    if any(text.startswith(f"shot {subject(low)} ") for _, text in events(low)):
        problems.append("the KS-19 fired below its 4,000 ft floor")
    return problems


def barrage_problems(output: str) -> list[str]:
    """North Vietnam SAM sites' barrage zone: it fires about a third of its
    bursts (random fire 33 percent) and sleeps once the jet leaves."""
    problems: list[str] = []
    run = runs(output)[0]
    unit = subject(run)
    tried = sum(1 for _, text in events(run) if text.startswith(f"phase {unit} w0 Fire"))
    fired = sum(1 for _, text in events(run) if text.startswith(f"burst {unit} "))
    if tried < 20 or not 0.1 <= fired / tried <= 0.6:
        problems.append(f"{fired} of {tried} bursts fired, retail 33 percent")
    if first(run, rf"^phase {unit} w0 Idle") is None:
        problems.append("the zone never went back to sleep")
    if summary(run).get("ground-square") != "false":
        problems.append("the barrage zone showed on the RWR")
    return problems


def base_defenses_problems(output: str) -> list[str]:
    """Cuba's base layout: an enemy SA-2 engages a Blue jet, and Blue's M163s
    engage a Redfor one, whose RWR shows their radar."""
    problems: list[str] = []
    red, blue = (runs(output) + ["", ""])[:2]
    m = SUBJECT.search(red)
    if not m or m.group(3) != "2" or int(summary(red).get("missiles", "0")) == 0:
        problems.append(f"the Redfor SA-2 did not engage: {summary(red)}")
    m = SUBJECT.search(blue)
    if not m or m.group(3) != "1" or int(summary(blue).get("rounds", "0")) == 0:
        problems.append(f"the Blue M163 did not engage a Redfor jet: {summary(blue)}")
    # Guantanamo's M163 stands low on a slope: its radar must still show
    # (reception ends at its antenna, above the ground).
    if summary(blue).get("ground-square") != "true":
        problems.append("the Blue M163's radar never showed on the Redfor jet's RWR")
    return problems


FLAK_PASS = re.compile(r"^flak-pass: t=([0-9.]+) (.*)$", re.M)
FLAK_MEMBER = re.compile(
    r"^flak-pass: member (\d+)( \(lead\))? shells (\d+) opening (\d+) first-shell (\S+) hp (\d+) -> (\d+) "
    r"lost (-?\d+) destroyed (\S+) jolts (\d+) knock ([0-9.]+)$",
    re.M,
)
FLAK_SUMMARY = re.compile(r"^flak-pass: summary first-shell (\S+) shells \[([0-9, ]*)\] .* lead-survived (\w+)$",
                          re.M)


def flak_pass_shots(run: str) -> list[tuple[float, str, bool]]:
    """Each shell the followed team fired: time, gun, opening barrage."""
    return [(t, gun, opening) for t, gun, _, opening in flak_pass_targets(run)]


def flak_pass_targets(run: str) -> list[tuple[float, str, str, bool]]:
    """Each shell the followed team fired: time, gun, target, opening."""
    out = []
    for m in FLAK_PASS.finditer(run):
        text = m.group(2)
        if text.startswith("shot "):
            words = text.split()
            out.append((float(m.group(1)), words[1], words[4], text.endswith(" opening")))
    return out


def flak_team_problems(output: str) -> list[str]:
    """Flak teams (John, 2026-10-14): fire spread over the group, held at a
    group skirting the team, half the knock, a slower opening barrage."""
    problems: list[str] = []
    heading_in, skirt, quarter, small, seats, lone = (runs(output) + [""] * 6)[:6]

    # A flight of four heading at the four-gun team.
    team_lines = [m.group(2) for m in FLAK_PASS.finditer(heading_in) if m.group(2).startswith("flak-team ")]
    if not any("heading-in engaged" in line for line in team_lines):
        problems.append(f"the team never engaged the flight heading in: {team_lines[:3]}")
    members = FLAK_MEMBER.findall(heading_in)
    shells = [int(m[2]) for m in members]
    if len(shells) != 4 or min(shells, default=0) < 2:
        problems.append(f"shells per member {shells}: every member should draw fire")
    elif shells[0] * 10 > sum(shells) * 4:
        problems.append(f"the lead drew {shells[0]} of {sum(shells)} shells (more than 40 percent)")
    summary_line = FLAK_SUMMARY.search(heading_in)
    if not summary_line or summary_line.group(3) != "true":
        problems.append("the lead did not survive the pass")
    for m in members:
        jolts, knock = int(m[9]), float(m[10])
        if abs(knock - 0.25 * jolts) > 0.011:
            problems.append(f"member {m[0]} knock {knock} for {jolts} flak jolts, expected a quarter each")
    by_gun: dict[str, list[float]] = {}
    for t, gun, opening in flak_pass_shots(heading_in):
        if opening:
            by_gun.setdefault(gun, []).append(t)
    if len(by_gun) != 4:
        problems.append(f"{len(by_gun)} guns fired an opening barrage, expected 4")
    for gun, times in by_gun.items():
        gaps = {round(b - a, 2) for a, b in zip(times, times[1:])}
        if len(times) != 8 or gaps != {0.75}:
            problems.append(f"{gun} opening barrage {len(times)} shells, gaps {sorted(gaps)}")
    # The opening barrages are spread round the members in reach (2026-10-14).
    spread = {gun: {target for _, g, target, opening in flak_pass_targets(heading_in) if g == gun and opening}
              for gun in by_gun}
    if max((len(t) for t in spread.values()), default=0) < 3:
        problems.append(f"opening barrages not spread over the flight: {spread}")

    # Eight aircraft grazing the team's longest reach, one of them in range
    # (12 percent): the team holds.
    if flak_pass_shots(skirt):
        problems.append(f"{len(flak_pass_shots(skirt))} shells at a flight skirting the team")
    skirt_lines = [m.group(2) for m in FLAK_PASS.finditer(skirt) if m.group(2).startswith("flak-team ")]
    if not any("in-range 1/8 holding" in line for line in skirt_lines) or \
            any("engaged" in line for line in skirt_lines):
        problems.append(f"the skirting flight was not held with one in range: {skirt_lines[:4]}")

    # Two of the eight in range (25 percent of the group within the team's
    # longest reach): engaged and fired at.
    quarter_lines = [m.group(2) for m in FLAK_PASS.finditer(quarter) if m.group(2).startswith("flak-team ")]
    if not any("in-range 2/8 engaged" in line for line in quarter_lines) or not flak_pass_shots(quarter):
        problems.append(f"a quarter of the group in range was not engaged: {quarter_lines[:4]}")

    # A four-ship grazing the edge with one member in reach: a quarter, but
    # under the minimum of two (John, 2026-10-14): held.
    small_lines = [m.group(2) for m in FLAK_PASS.finditer(small) if m.group(2).startswith("flak-team ")]
    if flak_pass_shots(small) or not any("in-range 1/4 holding" in line for line in small_lines) or \
            any("engaged" in line for line in small_lines):
        problems.append(f"a four-ship with one member in reach was not held: {small_lines[:4]}")

    # Two players in seats heading in: both draw fire.
    seat_shells = [int(m[2]) for m in FLAK_MEMBER.findall(seats)]
    if len(seat_shells) != 2 or min(seat_shells, default=0) < 2:
        problems.append(f"shells per seated player {seat_shells}")

    # A lone KS-19, a team of one.
    if not flak_pass_shots(lone):
        problems.append("the lone KS-19 never fired")
    return problems


def flak_pass(*args: str) -> list[str]:
    return ["--flak-pass", "TVIET", "QTSTRG", "--defenses", "3", "0", "--solo-team", "--from", "20",
            "--seconds", "200", "--rng", "1", *args]


TRACE_NOTES = ("Reads the surface records the import does not keep yet from the retail media "
               "(TORE_GAME_DIR, the remembered source or the gameassets link).")


def trace(*args: str) -> list[str]:
    return ["--surface-trace", *args]
# Templates that stay at their retail spot, by the anchoring rule over their
# contents (docs/spec/surface-defenses.md, "Relocation"), measured on the
# retail data (surface-AI round, slice L1, 2026-10-10): the plan's 31 with
# ~QPGSRUN by its target beside a dirt strip, plus ~QTCARGO and ~QUFACT
# (routes, lead ruling) and ~QCCMHQ (its centroid 0.6 nm from a dirt strip).
ANCHORED = {
    "QAPFAIR": "runway", "QAPHELO": "runway", "QBAIR": "runway", "QBBRD": "bridge-or-road",
    "QBFAIR": "strip", "QCCMHQ": "runway", "QCFAIR": "runway", "QELAIR": "runway",
    "QESAIR": "runway", "QFFACT": "strip", "QFLAIR": "runway", "QFSAIR": "runway",
    "QGRSAIR": "strip", "QIRFAIR": "runway", "QKPLNGR": "runway", "QLFFAIR": "runway",
    "QNSFAIR": "runway", "QPGFAIR": "runway", "QPGSRUN": "town", "QSPFAIR": "runway",
    "QTBARG": "town", "QTBRDG": "bridge-or-road", "QTBUNK": "strip", "QTCARGO": "route",
    "QTSTRG": "town", "QTTRUCK": "bridge-or-road", "QUBRI": "bridge-or-road", "QUCITY": "town",
    "QUCOL": "route", "QUFACT": "route", "QUSTRIP": "strip", "QVLAIR": "runway",
    "QVSAIR": "runway", "QWTFAIR": "runway",
}

# Base-layout batteries per theater (SA-2, SA-3, SA-6, HAWK), from the plan's
# list (section 3.11): Cuba 5 SA-2 and 2 SA-6, North Vietnam 13 SA-2, the
# Baltics' 4 HAWK in two sites, Panama 3 SA-6 and a HAWK pair, the SA-6 sites
# of Iraq, Pakistan, the Persian Gulf, South Korea and Taiwan.
BASE_BATTERIES = {
    "BAL": (0, 0, 0, 2), "CUB": (5, 0, 2, 0), "IRA": (0, 0, 4, 0), "TVIET": (13, 0, 0, 0),
    "SPA": (0, 0, 2, 0), "APA": (0, 0, 3, 1), "PGU": (0, 0, 3, 0), "NSK": (0, 0, 4, 0),
    "WTA": (0, 0, 2, 0),
}

SWEEP_LINE = re.compile(
    r"^surface-sweep: (\S+) (\S+) seed (\d+) anchor (\S+) moved-ft (\d+) rotation (\d+) units (\d+) "
    r"trucks (\d+) batteries (\d+) problems (\d+) digest (0x[0-9a-f]+)$",
    re.M,
)
BASE_BATTERY_LINE = re.compile(
    r"^surface-base-batteries: (\S+) batteries (\d+) sa2 (\d+) sa3 (\d+) sa6 (\d+) hawk (\d+) adopted (\d+) "
    r"added (\d+) launchers (\d+) in-batteries (\d+) ",
    re.M,
)


def relocate_sweep_problems(output: str) -> list[str]:
    """Site rules, anchors, relocation share, base batteries, repeated digests."""
    problems: list[str] = []
    first, _, again = output.partition("$ then 1:")
    digests: dict[tuple, str] = {}
    anchors: dict[str, str] = {}
    free = moved = 0
    for m in SWEEP_LINE.finditer(first):
        stem, seed, anchor, distance = m.group(2), int(m.group(3)), m.group(4), int(m.group(5))
        digests[(stem, seed)] = m.group(11)
        anchors.setdefault(stem, anchor)
        if anchors[stem] != anchor:
            problems.append(f"{stem}: anchor {anchor} with seed {seed}, {anchors[stem]} before")
        if anchor == "none":
            free += 1
            moved += distance > 0
        elif distance:
            problems.append(f"{stem}: anchored ({anchor}) but moved {distance} ft with seed {seed}")
    if len(anchors) != len(SURVEY) - 16:
        problems.append(f"{len(anchors)} templates swept, expected {len(SURVEY) - 16}")
    want = {stem: anchor for stem, anchor in ANCHORED.items()}
    got = {stem: anchor for stem, anchor in anchors.items() if anchor != "none"}
    if got != want:
        extra = sorted(set(got.items()) - set(want.items()))
        missing = sorted(set(want.items()) - set(got.items()))
        problems.append(f"anchored templates differ: extra {extra}, missing {missing}")
    if free and moved < 0.6 * free:
        problems.append(f"only {moved} of {free} free placements relocated")
    for line in re.findall(r"^surface-sweep-problem: .*$", first, re.M)[:20]:
        problems.append(line)
    bases = {m.group(1): m for m in BASE_BATTERY_LINE.finditer(first)}
    for theater, m in bases.items():
        counts = tuple(int(m.group(i)) for i in range(3, 7))
        if counts != BASE_BATTERIES.get(theater, (0, 0, 0, 0)):
            problems.append(f"{theater} base batteries (SA-2, SA-3, SA-6, HAWK) {counts}, expected {BASE_BATTERIES.get(theater, (0, 0, 0, 0))}")
        if m.group(9) != m.group(10):
            problems.append(f"{theater}: {m.group(10)} of {m.group(9)} battery launchers in batteries")
    if len(bases) != 16:
        problems.append(f"{len(bases)} base layouts, expected 16")
    repeats = 0
    for m in SWEEP_LINE.finditer(again):
        key = (m.group(2), int(m.group(3)))
        repeats += 1
        if digests.get(key) != m.group(11):
            problems.append(f"{key}: digest {digests.get(key)} then {m.group(11)}")
    if repeats != 2 * (len(SURVEY) - 16):
        problems.append(f"second run placed {repeats} times, expected {2 * (len(SURVEY) - 16)}")
    return problems


START_LINE = re.compile(
    r"^surface-start: (\S+) (\S+) red-target-nm (-?[\d.]+) blue-red-nm ([\d.]+) separation-nm (\d+) "
    r"side-off-deg ([\d.]+) heading-off-deg ([\d.]+) blue-on-map (\d) red-on-map (\d) front (\d)$",
    re.M,
)


def start_placement_problems(output: str) -> list[str]:
    """Red within 5 nm of the target; Blue the separation from Red toward its side, at the target; on the map."""
    problems: list[str] = []
    lines = list(START_LINE.finditer(output))
    if len(lines) != 16:
        problems.append(f"{len(lines)} theaters checked, expected 16")
    for m in lines:
        theater = m.group(1)
        red, blue, separation, side_off, heading_off = (float(m.group(i)) for i in (3, 4, 5, 6, 7))
        if not 0.0 <= red <= 5.01:
            problems.append(f"{theater}: Red starts {red} nm from the target")
        # A map too small for the separation shortens it; none of the 16 is.
        if abs(blue - separation) > 0.05:
            problems.append(f"{theater}: Blue starts {blue} nm from Red, separation {separation:.0}")
        # The spread is 30 degrees about a whole-degree bearing.
        if m.group(10) == "1" and side_off > 31.0:
            problems.append(f"{theater}: Blue starts {side_off} degrees off its side of the front")
        # Blue's heading is a whole degree.
        if heading_off > 0.6:
            problems.append(f"{theater}: Blue heads {heading_off} degrees off the target")
        if (m.group(8), m.group(9)) != ("1", "1"):
            problems.append(f"{theater}: a start is off the map")
    return problems


AREA_LINE = re.compile(
    r"^surface-area: (\S+) seed (\d+) level (\d) front (\d) units (\d+) of (\d+) sa-batteries (\d+) "
    r"depth-nm (\S+) problems (\d+) digest (0x[0-9a-f]+)$",
    re.M,
)
# Island theaters whose Red side is mostly sea: some sites find no land.
AREA_SHORT_OK = {"KURILE"}


def area_sweep_problems(output: str) -> list[str]:
    """Every theater places its area sites (all of them but the Kurils), and
    the second run (seed 1) repeats the digests."""
    problems: list[str] = []
    first_run, _, again = output.partition("$ then 1:")
    digests: dict[tuple, str] = {}
    for m in AREA_LINE.finditer(first_run):
        theater, seed, level = m.group(1), int(m.group(2)), int(m.group(3))
        front, units, wanted = int(m.group(4)), int(m.group(5)), int(m.group(6))
        digests[(theater, seed, level)] = m.group(10)
        if not front:
            problems.append(f"{theater}: no front")
        if units < wanted and theater not in AREA_SHORT_OK:
            problems.append(f"{theater} seed {seed} level {level}: {units} of {wanted} units")
        if units == 0:
            problems.append(f"{theater} seed {seed} level {level}: no area units")
    if len(digests) != 16 * 2 * 3:
        problems.append(f"{len(digests)} area builds, expected 96")
    repeats = 0
    for m in AREA_LINE.finditer(again):
        key = (m.group(1), int(m.group(2)), int(m.group(3)))
        repeats += 1
        if digests.get(key) != m.group(10):
            problems.append(f"{key}: digest {digests.get(key)} then {m.group(10)}")
    if repeats != 16 * 3:
        problems.append(f"second run built {repeats} times, expected 48")
    return problems


def area_resupply_problems(output: str) -> list[str]:
    """The first area SAM unit has a supply truck: with its rails drained it
    rearms in the run, and does not once its truck is killed."""
    runs_ = runs(output)
    problems: list[str] = []
    if not runs_:
        return ["no trace run"]
    if not re.search(r"^surface-trace: subject 0x5f000000 \S+ side 2 ", runs_[0], re.M):
        problems.append(f"subject {subject(runs_[0])} is not the first enemy area unit")
    if int(summary(runs_[0]).get("rearms", "0")) < 1:
        problems.append(f"drained area SAM never rearmed: rearms {summary(runs_[0]).get('rearms')}")
    if len(runs_) > 1 and int(summary(runs_[1]).get("rearms", "0")) != 0:
        problems.append("the area SAM rearmed after its truck was killed")
    return problems


def area_engage_problems(output: str) -> list[str]:
    """The first area unit is an enemy SAM and launches at the aircraft."""
    run = runs(output)[0]
    problems: list[str] = []
    if not re.search(r"^surface-trace: subject 0x5f000000 \S+ side 2 ", run, re.M):
        problems.append(f"subject {subject(run)} is not the first enemy area unit")
    s = summary(run)
    if int(s.get("missiles", "0")) < 1:
        problems.append(f"no launch: missiles {s.get('missiles')}")
    return problems


GROUNDING = re.compile(
    r"^grounding: (\S+) shape \S+ size (\w+) class \S+ footing (\w+) low (-?[\d.]+) high (-?[\d.]+) "
    r"below \d+% f2 (\S+) lift (-?[\d.]+) drawn (\S+) box (\S+) near (\S+) far (\S+) hp \S+$",
    re.M,
)


def grounding_problems(output: str) -> list[str]:
    """Every object stands on the bottom of its shape (docs/formats/objects-and-shapes.md, "Grounding")."""
    rows = {m.group(1): m for m in GROUNDING.finditer(output)}
    problems: list[str] = []
    for ext, want in [(".OT", 150), (".NT", 60), (".PT", 20)]:
        n = sum(1 for name in rows if name.upper().endswith(ext))
        if n < want:
            problems.append(f"{n} {ext} types surveyed, want at least {want}")
    for name, m in rows.items():
        foot, low, high, f2, lift = m.group(3), float(m.group(4)), float(m.group(5)), m.group(6), float(m.group(7))
        drawn, box, near, far = m.group(8), m.group(9), m.group(10), m.group(11)
        if foot in ("MapHeight", "Waterline") and lift != 0:
            problems.append(f"{name}: a {foot} type lifted {lift} ft")
        if foot != "Bottom":
            continue
        if abs(lift + low) > 0.11:
            problems.append(f"{name}: lifted {lift} ft, its lowest point is {low} ft")
        checks = [("faces", drawn), ("man near", near), ("man far", far)]
        if high - low >= 2 and box != "-" and float(box) < -0.05:
            problems.append(f"{name}: solid hit box bottom {box} ft below ground")
        for what, value in checks:
            if value != "-" and abs(float(value)) > 0.05:
                problems.append(f"{name}: {what} bottom {value} ft above the ground")
        if f2 != "-" and low < 0 and abs(float(f2) - low) > 3.0:
            problems.append(f"{name}: lowest point {low} ft is {abs(float(f2) - low):.1f} ft from its F2 offset {f2}")
    for name, want in [("FACTD.OT", 42.7), ("HANGR.OT", 26.7), ("FACT1.OT", 162.7), ("TROOPS.NT", 3.3),
                       ("GCI.NT", 18.7)]:
        m = rows.get(name)
        if m is None or abs(float(m.group(7)) - want) > 0.11:
            problems.append(f"{name}: lift {m and m.group(7)}, want {want} ft")
    for man in ("SOLDIER.NT", "TROOPS.NT"):
        m = rows.get(man)
        if m is None or "-" in (m.group(10), m.group(11)):
            problems.append(f"{man}: no man drawn near and far")
    return problems


def moving_sam_hit_problems(output: str) -> list[str]:
    found = runs(output)
    if not found:
        return ["missing moving-jet SAM trace"]
    return [f"moving jet received no SAM hits: {summary(run)}" for run in found
            if int(summary(run).get("hits", "0")) < 1]


def scenarios() -> list[Scenario]:
    moving_sams = []
    for weapon, theater, index in [("SA2A", "TVIET", "6"), ("SA3", "SPA", "0"), ("SA6", "CUB", "0")]:
        for altitude in [10000, 20000]:
            moving_sams.append(Scenario(
                name=f"surface-moving-hit-{weapon.lower()}-{altitude}", lane="ai", timeout=300,
                args=trace(theater, *(["QSPSAM"] if weapon == "SA3" else []), "--over", weapon, "--index", index, "--solo", "--altitude", str(altitude),
                           "--from", "20", "--seconds", "200", "--quiet-shots"),
                check=moving_sam_hit_problems,
                notes="Requires hits on a moving vulnerable jet, not just missile launches."))
    return moving_sams + [
        Scenario(
            name="bridge-audit", lane="ai",
            args=["--bridge-audit", "{work}/bridges", "UKR", "BAL", "--strict"], timeout=300,
            expect=[r"^bridge-audit: [1-9]\d* layouts [1-9]\d* placements 0 flagged 0 missing textures$"],
            notes="Imported bridge damage assets and shared deck/footing clearance, including bank cuts, 8 percent approaches, supported toes, sloping Ukraine crossings and bridge objectives.",
        ),
        Scenario(
            name="bridge-end-ruins", lane="flight", window=True,
            args=["--bridge-audit", "{work}/bridge-ends", "TVIET", "UKR", "--strict", "--capture",
                  "--id=400000eb,400000b7,400000bc,40000092"], timeout=300,
            expect=[r"^bridge-audit: [1-9]\d* layouts [1-9]\d* placements 0 flagged 0 missing textures$"],
            outputs=["bridge-ends/TVIET-400000eb-destroyed.png", "bridge-ends/TVIET-400000b7-destroyed.png",
                     "bridge-ends/TVIET-400000bc-destroyed.png", "bridge-ends/UKR-40000092-destroyed.png"],
            notes="All four END types change only at zero HP, retain geometry, and match replay HP; uses the ordinary GPU scenery path.",
        ),
        Scenario(
            name="a2g-bridge-destruction", lane="flight",
            args=["--a2g-run", "UKR", "QUBRI", "--aircraft", "a10", "--load", "AGM65G",
                  "--attack", "maverick", "--from", "3", "--heading", "90", "--altitude", "3000",
                  "--defenses", "0", "0", "--seconds", "35", "--record", "{work}/bridge.toreplay",
                  "--verify-render"], timeout=300,
            expect=[r"^a2g-run: target box center 979636 568 801861 half 128\.0 24\.0 2456\.0$",
                    r"^a2g-run: kill 0x50000000 class 0x0100$",
                    r"^a2g-run: target 0x50000000 hp 0/100$",
                    r"^a2g-run: verify-render: PASS ticks=[1-9]\d* missing=0 differing=0"],
            notes="A real Maverick release destroys the Ukraine bridge span; recorded render snapshots retain its damage and effects.",
        ),
        Scenario(
            name="surface-grounding", lane="ai",
            args=["--surface-dump", "--grounding"], timeout=300,
            expect=[r"^grounding: \d+ types, \d+ lifted, 0 problems$"],
            forbid=[r"^grounding: problem "],
            check=grounding_problems,
            notes="B13 and GROUND: every object, unit, man and parked aircraft stands on the bottom of its shape, "
                  "including thin frames below the solid hit box. Reads the records the import lacks from the retail media.",
        ),
        Scenario(
            name="surface-ground-pads", lane="ai",
            args=["--surface-dump", "--ground-pads"], timeout=1200,
            expect=[r"^ground-pads: worlds [1-9]\d* placements [1-9]\d* problems 0$"],
            notes="Every theater, alternate layout and objective, authored and relocated: flat shapes and broad fixed footprints share terrain support within 0.05 ft.",
        ),
        Scenario(
            name="surface-grounding-sites", lane="ai",
            args=["--surface-dump", "--grounding-sites"], timeout=600,
            expect=[r"^grounding-sites: 16 theaters, 0 unread resources, 0 problems$"] + [
                rf"^grounding-sites: {theater} missions [1-9]\d* templates [1-9]\d* "
                r"placements [1-9]\d{2,} exceptions \d+ unavailable \d+ before \d+ problems 0$"
                for theater in BASE_DEFENSES
            ],
            forbid=[r"^site-problem:"],
            notes="Authored theater and campaign mission objects plus every Quick Mission template, "
                  "at its authored site and seed 1 relocated site. Checks lines and slope clearance.",
        ),
        Scenario(
            name="surface-grounding-radar-render", lane="flight",
            args=["--surface-scene", "{work}/radar", "CUB", "QCLST", "--seconds", "0",
                  "--focus", "1", "--distance", "35", "--look", "30,-8", "--tag", "radar"],
            timeout=300, outputs=["radar/radar-look.png"],
            expect=[r"surface-scene: .*radar-look.png"],
            notes="Offscreen radar objective with its antenna frame, no window.",
        ),
        Scenario(
            name="surface-resolve-all", lane="ai",
            args=["--surface-dump", "--all"], timeout=300,
            expect=[r"^surface-all: 1488 resolutions, 124 templates, 0 errors$"],
            forbid=[r"^surface-all: error"],
            then=[Step(["--surface-dump", "--all", "--seeds", "1"], timeout=300)],
            check=resolve_all_problems,
            notes="Reads the templates from the retail media (TORE_GAME_DIR, the remembered source or the "
                  "gameassets link) until the import keeps them.",
        ),
        Scenario(
            name="surface-sa6-engage", lane="ai", timeout=300,
            args=trace("IRA", "--over", "SA6", "--altitude", "15000", "--from", "20", "--seconds", "150",
                       "--invulnerable"),
            forbid=[r"^surface-trace: t=\S+ player crashed"],
            check=sa6_engage_problems, notes=TRACE_NOTES,
        ),
        Scenario(
            name="surface-sa2-low", lane="ai", timeout=300,
            args=trace("TVIET", "--over", "SA2A", "--altitude", "1000", "--from", "20", "--seconds", "150",
                       "--invulnerable", "--quiet-shots"),
            forbid=[r"^surface-trace: t=\S+ player crashed"],
            check=sa2_low_problems, notes=TRACE_NOTES,
        ),
        Scenario(
            name="surface-chaff-flare", lane="ai", timeout=300,
            args=trace("UKR", "QUFACT", "--surface-seed", "2", "--unit", "0x5000000e", "--near", "2000",
                       "--altitude", "5000", "--speed", "300", "--from", "6", "--seconds", "120",
                       "--chaff", "1", "--flares", "1", "--invulnerable"),
            check=chaff_flare_problems, notes=TRACE_NOTES + " Seed 2 mans the factory's SA-6 and SA-7 slots.",
        ),
        Scenario(
            name="surface-harm", lane="ai", timeout=400,
            args=trace("VLA", "QVRDRI", "--surface-seed", "4", "--unit", "0x5000000d",
                       "--skill", "3", "--rng", "2", "--aircraft", "f16c", "--altitude", "20000",
                       "--from", "25", "--harm-at", "10", "--seconds", "200", "--invulnerable"),
            then=[Step(trace("VLA", "QVRDRI", "--surface-seed", "4", "--unit", "0x5000000d",
                             "--skill", "3", "--aircraft", "f16c", "--altitude", "20000", "--from", "25",
                             "--kill-at", "140", "--seconds", "220", "--invulnerable"), timeout=300)],
            check=harm_problems,
            notes=TRACE_NOTES + " The jet carries an AGM-88 and fires it in BORE through the trigger when the "
                  "HARM cue's candidate is the radar (slice W2). The radar's return 30 s after a shutdown that no "
                  "HARM follows is checked by the tore-world unit test.",
        ),
        Scenario(
            name="surface-battery-sa2", lane="ai", timeout=400,
            args=trace("TVIET", "--over", "SA2A", "--index", "6", "--solo", "--altitude", "20000",
                       "--from", "20", "--seconds", "200", "--quiet-shots"),
            then=[
                Step(trace("TVIET", "--over", "SA2A", "--index", "6", "--altitude", "20000",
                           "--from", "20", "--seconds", "200", "--invulnerable", "--quiet-shots",
                           "--kill-at", "1"), timeout=300),
                Step(trace("TVIET", "--over", "SA2A", "--index", "6", "--altitude", "20000",
                           "--from", "20", "--seconds", "200", "--invulnerable", "--quiet-shots",
                           "--kill-at", "1", "--condition", "night"), timeout=300),
            ],
            check=battery_sa2_problems,
            notes=TRACE_NOTES + " The North Vietnam SA-2 whose battery adopted a GCI.",
        ),
        Scenario(
            name="surface-zsu23-gun", lane="ai", timeout=300,
            args=trace("BAL", "--over", "ZSU23", "--altitude", "1500", "--speed", "250", "--from", "4",
                       "--seconds", "120", "--quiet-shots"),
            check=zsu23_problems, notes=TRACE_NOTES,
        ),
        Scenario(
            name="surface-small-arms-airfield", lane="ai", timeout=900,
            args=trace("APA", "QAPFAIR", "--over", "TROOPS", "--solo", "--skill", "0", "--rng", "11",
                       "--altitude", "300", "--speed", "400", "--from", "3", "--seconds", "40", "--quiet-shots"),
            then=[
                Step(trace("APA", "QAPFAIR", "--over", "TROOPS", "--solo", "--skill", str(skill), "--rng", "11",
                           "--altitude", "300", "--speed", "400", "--from", "3", "--seconds", "40",
                           "--quiet-shots"), timeout=300)
                for skill in (1, 2, 3)
            ] + [
                Step(trace("APA", "QAPFAIR", "--over", "TROOPS", "--solo", "--near", "15000", "--skill",
                           str(skill), "--rng", "11", "--altitude", "300", "--speed", "400", "--from", "3",
                           "--seconds", "60", "--quiet-shots"), timeout=300)
                for skill in (1, 3)
            ] + [
                Step(trace("APA", "QAPFAIR", "--over", "TROOPS", "--solo", "--skill", "3", "--rng", "11",
                           "--altitude", altitude, "--speed", speed, "--pass", side, "--from", "3",
                           "--seconds", "40", "--quiet-shots"), timeout=300)
                for altitude, speed, side in (("1000", "400", "0"), ("300", "600", "0"), ("300", "400", "1500"))
            ],
            check=small_arms_problems,
            notes=TRACE_NOTES + " A low pass over Afghanistan's airfield target (~QAPFAIR): one squad at each "
                  "experience, then every squad within 2.5 nm of it at average and ace experience, the other "
                  "units laid down (--solo); then ace passes at 1,000 ft, at 600 kt and 1,500 ft to the side.",
        ),
        Scenario(
            name="surface-tank-aa", lane="ai", timeout=600,
            args=trace("IRA", "QIRRETR", "--over", "T72", "--solo", "--skill", "1", "--rng", "11",
                       "--aircraft", "ah64", "--altitude", "1500", "--speed", "0", "--from", "0.54",
                       "--seconds", "60", "--quiet-shots"),
            then=[
                Step(trace("IRA", "QIRRETR", "--over", "T72", "--solo", "--skill", "1", "--rng", "11",
                           "--aircraft", "ah64", "--altitude", "300", "--speed", "0", "--from", "0.27",
                           "--seconds", "60", "--invulnerable"), timeout=300),
                Step(trace("IRA", "QIRRETR", "--over", "T72", "--solo", "--skill", "1", "--rng", "11",
                           "--aircraft", "f16c", "--altitude", "200", "--speed", "450", "--from", "3",
                           "--seconds", "45", "--invulnerable"), timeout=300),
                Step(trace("IRA", "QIRRETR", "--over", "T72", "--solo", "--skill", "3", "--rng", "11",
                           "--aircraft", "f16c", "--altitude", "2500", "--speed", "400", "--from", "3",
                           "--seconds", "50", "--quiet-shots"), timeout=300),
            ],
            check=tank_aa_problems,
            notes=TRACE_NOTES + " One T-72 of Iraq's withdrawing column (~QIRRETR), the other units laid down "
                  "(--solo): an AH-64 hovering 1,500 ft up 1 km out (24 degrees), then 300 ft up 0.5 km out; "
                  "an F-16C at 450 kt 200 ft straight over it; at 400 kt 2,500 ft over it (John, 2026-10-24).",
        ),
        Scenario(
            name="surface-squad-killable", lane="ai", timeout=600,
            args=["--a2g-run", "APA", "QAPFAIR", "--aircraft", "a4e", "--load", "LAU61", "--attack", "strafe",
                  "--unit", SQUAD_ID, "--from", "0.1", "--altitude", "400", "--dive", "10", "--speed", "350",
                  "--hold", "6", "--seconds", "60"],
            then=[
                Step(["--a2g-run", "APA", "QAPFAIR", "--aircraft", "f18", "--load", "MK82", "--attack", "ccip",
                      "--unit", SQUAD_ID, "--from", "3", "--altitude", "3000", "--dive", "30", "--speed", "400",
                      "--seconds", "60"], timeout=300),
                Step(trace("APA", "QAPFAIR", "--over", "TROOPS", "--solo", "--skill", "1", "--rng", "11",
                           "--altitude", "300", "--speed", "400", "--from", "3", "--seconds", "40",
                           "--kill-at", "23", "--quiet-shots"), timeout=300),
            ],
            check=squad_killable_problems,
            notes="An airfield squad (~QAPFAIR, 75 hit points, a 30 by 12 ft hit box) dies to a strafing pass "
                  "and to a bomb, is counted as a kill of the men's class, and fires no more once killed.",
        ),
        Scenario(
            name="surface-resupply", lane="ai", timeout=900,
            args=trace("UKR", "QUFACT", "--surface-seed", "2", "--unit", "0x5000000e", "--drain",
                       "--altitude", "15000", "--from", "62", "--seconds", "600", "--invulnerable"),
            then=[
                Step(trace("UKR", "QUFACT", "--surface-seed", "2", "--unit", "0x5000000e", "--drain",
                           "--kill-truck-at", "100", "--altitude", "15000", "--from", "62",
                           "--seconds", "600", "--invulnerable"), timeout=300),
                Step(trace("UKR", "QUFACT", "--surface-seed", "2", "--unit", "0x5000000c", "--drain",
                           "--drain-reserve", "--from", "200", "--seconds", "300", "--invulnerable",
                           "--quiet-shots"), timeout=300),
                Step(trace("UKR", "QUFACT", "--surface-seed", "2", "--unit", "0x5000000c", "--drain",
                           "--altitude", "3000", "--speed", "60", "--from", "4", "--seconds", "400",
                           "--invulnerable", "--quiet-shots"), timeout=300),
                Step(trace("UKR", "QUFACT", "--surface-seed", "2", "--unit", "0x5000000c", "--drain",
                           "--drain-reserve", "--kill-truck-at", "0", "--altitude", "3000", "--speed", "60",
                           "--from", "4", "--seconds", "400", "--invulnerable", "--quiet-shots"), timeout=300),
            ],
            check=resupply_problems,
            notes=TRACE_NOTES + " The factory's seed 2 group: an SA-6 battery and a ZSU-23, each with its "
                  "supply truck.",
        ),
        Scenario(
            name="surface-flak-tviet", lane="ai", timeout=400,
            args=trace("TVIET", "QTAAA", "--over", "KS19", "--aircraft", "a10", "--altitude", "15000",
                       "--speed", "300", "--from", "10", "--seconds", "120", "--invulnerable"),
            then=[Step(trace("TVIET", "QTAAA", "--over", "KS19", "--aircraft", "a10", "--altitude", "3000",
                             "--speed", "300", "--from", "10", "--seconds", "120", "--invulnerable"),
                       timeout=300)],
            check=flak_problems, notes=TRACE_NOTES,
        ),
        Scenario(
            name="surface-flak-team", lane="ai", timeout=600,
            args=flak_pass("--size", "4", "--flight", "4"),
            then=[
                Step(flak_pass("--size", "4", "--flight", "4", "--second", "4", "--pass", "48300"), timeout=300),
                Step(flak_pass("--size", "4", "--flight", "4", "--second", "4", "--pass", "46000"), timeout=300),
                Step(flak_pass("--size", "4", "--flight", "4", "--pass", "45500"), timeout=300),
                Step(flak_pass("--size", "4", "--flight", "2", "--seats"), timeout=300),
                Step(flak_pass("--size", "1", "--flight", "2"), timeout=300),
            ],
            forbid=[r"^flak-pass: t=\S+ member 0 destroyed"],
            check=flak_team_problems,
            notes=TRACE_NOTES + " Flak teams, spread fire, the 25 percent and heading-in triggers, half the "
                  "knock and the slower opening barrage (John, 2026-10-14).",
        ),
        Scenario(
            name="surface-barrage-zone", lane="ai", timeout=300,
            args=trace("TVIET", "QTSAM", "--over", "A_M1939", "--altitude", "3000", "--speed", "250",
                       "--from", "6", "--pass", "2000", "--seconds", "300", "--quiet-shots", "--invulnerable"),
            check=barrage_problems, notes=TRACE_NOTES,
        ),
        Scenario(
            name="surface-base-defenses", lane="ai", timeout=400,
            args=trace("CUB", "--over", "SA2A", "--altitude", "20000", "--from", "20", "--seconds", "150",
                       "--quiet-shots", "--invulnerable"),
            then=[Step(trace("CUB", "--over", "M163", "--player-side", "red", "--altitude", "1500",
                             "--speed", "300", "--from", "4", "--seconds", "90", "--quiet-shots",
                             "--invulnerable"), timeout=300)],
            check=base_defenses_problems, notes=TRACE_NOTES,
        ),
        Scenario(
            name="surface-relocate-sweep", lane="ai",
            args=["--surface-dump", "--sweep"], timeout=600,
            expect=[r"^surface-sweep: 2160 placements, \d+ relocated, 0 problems, 0 errors$"],
            forbid=[r"^surface-sweep: error"],
            then=[Step(["--surface-dump", "--sweep", "--seeds", "2"], timeout=300)],
            check=relocate_sweep_problems,
            notes="Reads the templates from the retail media until the import keeps them.",
        ),
        Scenario(
            name="surface-class-sweep", lane="ai",
            args=["--surface-dump", "--classes"], timeout=600,
            expect=[r"^surface-class-sweep: \d+ placements, 0 problems$"],
            forbid=[r"^surface-class-problem:"],
            notes="Reads the templates from the retail media until the import keeps them.",
        ),
        Scenario(
            name="surface-area-sweep", lane="ai",
            args=["--surface-dump", "--area-sweep", "--seeds", "2"], timeout=900,
            expect=[r"^surface-area-sweep: builds 96 problems 0 short \d+$"],
            forbid=[r"^surface-area-problem:"],
            then=[Step(["--surface-dump", "--area-sweep", "--seeds", "1"], timeout=600)],
            check=area_sweep_problems,
            notes="B23 area defenses. Reads the unit records from the retail media until the import keeps them.",
        ),
        Scenario(
            name="surface-area-engage", lane="ai", timeout=400,
            args=trace("UKR", "--area", "--defenses", "3", "3", "--surface-seed", "1", "--unit", "0x5f000000",
                       "--altitude", "4000", "--speed", "350", "--from", "8", "--seconds", "120",
                       "--invulnerable", "--quiet-shots"),
            forbid=[r"^surface-trace: t=\S+ player crashed"],
            check=area_engage_problems,
            notes=TRACE_NOTES + " A Quick Mission with no ground target and heavy strengths (B23).",
        ),
        Scenario(
            name="surface-area-resupply", lane="ai", timeout=900,
            args=trace("UKR", "--area", "--defenses", "0", "3", "--surface-seed", "1", "--unit", "0x5f000000",
                       "--drain", "--altitude", "15000", "--from", "62", "--seconds", "700", "--invulnerable"),
            then=[
                Step(trace("UKR", "--area", "--defenses", "0", "3", "--surface-seed", "1", "--unit", "0x5f000000",
                           "--drain", "--kill-truck-at", "0", "--altitude", "15000", "--from", "62",
                           "--seconds", "700", "--invulnerable"), timeout=300),
            ],
            check=area_resupply_problems,
            notes=TRACE_NOTES + " The first area SAM unit (heavy SAM strength, no guns) drained at the start "
                  "rearms from its supply truck in 300 to 600 s; with the truck killed at once it does not "
                  "(nr-b23b).",
        ),
        Scenario(
            name="surface-objectives-audit", lane="ai",
            args=["--surface-dump", "--objectives", "--fast", "--seeds", "1", "LFA", "UKR"],
            timeout=600,
            expect_exit=None,
            expect=[r"^objective-audit: cases 264 findings \d+ errors \d+$"],
            forbid=[r"^finding\t.*\t(?:objective-anchor-stale|nondeterministic|duplicate-id|object-count|missing-element|unavailable-grounding-geometry)"],
            then=[Step(args=("--surface-dump", "--objectives", "--fast", "--seeds", "1", "LFA", "UKR", "--uncached"), timeout=600, expect_exit=None)],
            check=objective_audit_problems,
            notes="All eleven separations, four defense levels, fleet/shore, airport and area layouts. Independent category ceilings accept improvements, reject placement regressions and compare cached/direct output.",
        ),
        Scenario(
            name="surface-objectives-cache-kuril", lane="ai",
            args=["--surface-dump", "--objectives", "--seeds", "3", "KURILE"],
            timeout=900,
            expect=[r"^objective-audit: cases 4224 findings \d+ errors 0$"],
            then=[
                Step(args=("--surface-dump", "KURILE", "QKSILO", "--defenses", "0", "2", "--surface-seed", "2", "--separation", "5")),
                Step(args=("--surface-dump", "KURILE", "QKSILO", "--defenses", "3", "0", "--surface-seed", "3", "--separation", "50")),
            ],
            check=objective_cache_kuril_problems,
            notes="The final front can appear or move at a new separation despite identical raw geometry. Compare both observed cache regressions with independent full scene builds.",
        ),
        Scenario(
            name="surface-start-placement", lane="ai",
            args=["--surface-dump", "--starts"], timeout=300,
            expect=[r"^surface-start: 16 theaters$"],
            check=start_placement_problems,
            notes="Reads the templates from the retail media until the import keeps them.",
        ),
    ]
