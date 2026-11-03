#!/usr/bin/env python3
"""Pick the battery scenarios that a change can affect.

`python3 tools/battery.py --changed [REF] [--budget SECONDS]` uses this module.
The idea is the per-change tier of the testing policy (docs/testing/README.md,
"Testing tiers"): instead of the whole battery, run a handful of scenarios that
would notice the change, and fit them to a wall-clock budget.

Two reviewed tables do the work:

* FAMILIES names groups of scenarios (by name pattern) and says why they belong
  together, for example "flight-stall" or "ai-airfield".
* RULES maps source paths to the families they can affect. The first rule that
  matches a file wins, so the test-only rules sit at the top.

A test (tools/test_battery_selection.py) checks that every family matches at
least one scenario, every scenario belongs to some family, and every tracked
source file under `crates/` and `tools/` has a rule, so the map cannot drift
quietly when someone adds a file or a scenario. A changed file that no rule
matches still selects something (every family, trimmed to the budget) and is
reported as unmapped.

The budget fit keeps at least one scenario of every selected family, then adds
more, fastest first, in rounds: round one is the preferred aircraft and theater
of each kind of scenario, round two a second aircraft or theater. It stops when
the estimated wall-clock time (scenario durations from the newest full battery
run, packed onto the requested number of parallel jobs) would pass the budget.
"""
from __future__ import annotations

import dataclasses
import fnmatch
import heapq
import json
import re
import subprocess
from pathlib import Path
from typing import Iterable, Optional, Sequence

# --------------------------------------------------------------------------
# Scenario families: name -> (why they belong together, name patterns)
# --------------------------------------------------------------------------

FAMILIES: dict[str, tuple[str, tuple[str, ...]]] = {
    "vegetation": ("CPU vegetation camera, crown and streaming coverage", ("vegetation-cpu-cameras", "vegetation-pass5-cameras", "vegetation-stream-convergence", "vegetation-top-crowns", "vegetation-cache-roundtrip", "vegetation-grass-cameras", "vegetation-theater-limits", "vegetation-tile-seams", "vegetation-material-fringe")),
    "render-night-light-slots": (
        "headless persistent fire reservation through effect and scene light budgets",
        ("render-night-light-slots",),
    ),
    "terrain-colors": ("retail ground palette families in every theater", ("render-terrain-colors",)),
    "render-night-effect-lights": (
        "fire and flare comparisons and sustained explosion lighting at night",
        ("render-night-effect-lights",),
    ),
    # Flight lane.
    "flight-animation": ("reviewed CPU control-surface and device poses", ("flight-animation-*",)),
    "flight-maneuvers": (
        "flight model in the air: level, pull, loop, roll, bank, sprint, climb, overspeed, G, autopilot",
        (
            "flight-level-*", "flight-pull-*", "flight-loop-*", "flight-roll-*", "flight-bank-*",
            "flight-sprint-*", "flight-climb-*", "flight-overspeed-*", "flight-combatg-*",
            "flight-autopilot-*", "flight-waypoint-*", "flight-lateral-rudder-*", "flight-variety-*",
        ),
    ),
    "flight-powered": (
        "the VTOL and helicopter overhaul's six powered-lift aircraft flown by scripted tapes: hover, hover hold, "
        "transition and conversion, the V-22 corridor, autorotation, the vortex ring state, Easy flight physics",
        ("flight-powered-*",),
    ),
    "flight-stall": (
        "stall, spin and liftoff speeds, and the recoveries",
        ("flight-stall-*", "flight-stallrecover-*", "flight-spin-*", "flight-spinrecover-*", "flight-liftoff-*"),
    ),
    "flight-takeoff": (
        "ground start, takeoff roll, gear contact, climb-out, loadouts at takeoff",
        (
            "flight-takeoff-*", "flight-groundstart-*", "flight-belly-*", "flight-climbout-*",
            "flight-loadout-*", "replay-script-takeoff-*", "flight-lateral-nosewheel-*",
        ),
    ),
    "flight-landing": ("approach and landing", ("flight-land-*", "flight-approach-*")),
    "flight-combat": (
        "guns, missiles, F-22A/F-22N bays and live fire, jettison, countermeasures, combat smoke and evidence",
        (
            "flight-fight-*", "flight-attack-*", "flight-livefire-*", "flight-combatevidence-*",
            "flight-combatsmoke-*", "flight-missileacceptance-*", "flight-missile-sag", "flight-missile-energy", "flight-missile-defeat", "flight-jettison-*", "flight-bay-*",
            "flight-wreckcontact-*", "flight-ac130-*",
            "flight-countermeasures-*",  # Includes the fixed-tick AI cadence regression.
            "replay-combat-smoke-*", "menus-combat-smoke-*",
        ),
    ),
    "flight-damage": (
        "damage, faults, ejection, fuel, devices, cheats",
        (
            "flight-damage-*", "flight-eject-*", "flight-ejectionpose-*", "flight-fault*",
            "flight-panelfault*", "flight-fuelout-*", "flight-devices-*", "flight-cheat-*",
        ),
    ),
    "flight-environment": (
        "weather, time of day, terrain contact, world edge",
        (
            "flight-environment-*", "flight-weather*", "flight-hour*", "flight-edge-*",
            "flight-terrain-*", "flight-validate-*", "menus-validate-weather",
        ),
    ),
    "instruments": (
        "cockpit panels, HUD and sensors",
        ("flight-panel-*", "flight-window-*", "flight-sensor-*", "replay-sensor-*", "menus-panel-*"),
    ),
    # AI lane.
    "ai-fights": (
        "AI against AI, one against one up to fifteen against fifteen, guns, skills, threats",
        (
            "ai-fight-*", "ai-pair-*", "ai-big-*", "ai-guns-*", "ai-separation-*", "ai-skill-*",
            "ai-threat-*", "ai-missile-launch-regression", "ai-missile-envelope-gaps", "ai-missile-player-cue", "ai-missile-turning-cue", "ai-long-1v1", "ai-long-5v5", "ai-long-15v15*", "ai-long-guns-*", "ai-long-hold-*",
        ),
    ),
    "ai-damage": ("AI with faults and damage", ("ai-damaged-*", "ai-fault-*", "ai-record-fault")),
    "ai-missions": (
        "mission presets and objectives, fuzzed mission setups",
        ("ai-mission-*", "ai-objective-*", "ai-fuzz-*"),
    ),
    "ai-orders": ("wing orders", ("ai-order-*", "ai-orders-*")),
    "ai-datalink": ("the flight data link's picture and what reads it", ("ai-datalink-*",)),
    "link-cues": (
        "the flight data link's cues, flown by hand with wingmen",
        ("replay-script-link-*",),
    ),
    "ai-airfield": (
        "AI takeoff, landing, return to base, ILS approaches on every theater",
        (
            "ai-takeoff-*", "ai-theater-*", "ai-ground-*", "ai-rtb-*", "ai-ils-*",
            "ai-long-ground-land", "ai-lost-lead-*", "ai-airfield-taxi-*", "ai-merged-airfield-*", "ai-enemy-ground-start-*",
            "ai-heli-*", "ai-stovl-*", "ai-v22-*", "ai-records-*",
        ),
    ),
    "ai-lead": (
        "lead succession and the wing after the leader or the human is lost",
        ("ai-lost-lead-*", "ai-regress-wingman-dead-leader", "ai-known-*", "ai-ground-fight-*", "ai-ground-takeoff-*"),
    ),
    "ai-regression": (
        "regressions, determinism, recordings and roster probes",
        ("ai-regress-*", "ai-determinism-*", "ai-record-*", "ai-roster-*", "ai-probe-*"),
    ),
    "radio": (
        "scenarios whose output is checked for radio calls",
        (
            "ai-fight-1v1-*", "ai-fight-2v2-*", "ai-fight-4v4-*", "ai-order-*", "ai-ground-*", "ai-rtb-*",
            "replay-rec-fight-*", "replay-rec-order-*", "replay-rec-mission-*", "replay-live-audio-*",
        ),
    ),
    # Menus lane.
    "replay-feedback": ("dedicated feedback form, recording lifecycle and writer recovery", ("net-replay-feedback-*",)),
    "menus-screens": ("menu screens captured on the CPU", ("menus-snap-*", "menus-flight-map-*", "replay-snapshot-*", "menus-replays-bar-*", "menus-updates-bar-*")),
    "menus-creator": (
        "the Quick Mission creator, start-up runs and loadout pages, including even store-pair maxima and saved-load migration",
        ("qmc-wing-loadout", "menus-start-*", "menus-loadout-*", "menus-tanks-*", "menus-ordnance-*", "menus-derived-stations", "menus-validate-creator", "menus-snap-quick-*"),
    ),
    "menus-validate": (
        "text, maps, weather, creator and ILS validators",
        ("menus-validate-*", "replay-validate-*"),
    ),
    "airports": (
        "airports in the creator, the tower lists and ILS surveys",
        (
            "menus-validate-creator", "menus-validate-ils*", "menus-snap-quick-airports", "airport-allegiance",
            "menus-snap-quick-ground-start", "menus-snap-quick-field-*", "ai-ils-*",
            "airfield-taxi-audit", "airfield-parked-audit", "merged-airport-audit", "ai-airfield-taxi-*",
        ),
    ),
    "surface": (
        "surface units: ground target templates resolved, base-layout air defenses and their sides, final-front audit cache",
        ("surface-*", "bridge-audit", "bridge-end-ruins", "a2g-bridge-destruction"),
    ),
    "a2g": (
        "air-to-ground: the A-G, IR and HARM pages and designation (--a2g-radar), and the weapons flown through the real trigger by --a2g-run",
        ("a2g-*",),
    ),
    "ai-strike": (
        "the AI strike driver (--a2g-run --pilot ai) and the AI's air-to-ground behaviour (slice ST-D onward)",
        ("ai-strike-*", "ai-sead-*", "ai-wing-*", "qmc-strike-*", "ai-jettison"),
    ),
    "windowed-menus": ("captured windows: launches, terrain, previews, sizes", ("menus-window-*", "menus-window-frame-log")),
    "flight-views": ("camera views and their rendering", ("replay-view-*", "render-*", "flight-target-camera-*", "flight-f12-hold")),
    # Replay lane.
    "replay-recording": (
        "recording, reading and corrupting replay files",
        ("replay-banner-*", "replay-stores-*", "replay-rec-*", "replay-surface", "replay-a2g", "replay-city-strike", "replay-city-smoke-seek", "replay-taped-views", "replay-weapon-chase", "replay-hud-state", "replay-corrupt-*", "replay-watch-*", "ai-record-*", "ai-determinism-recordings"),
    ),
    "replay-cli": (
        "command-line errors, speeds, ticks, small tools",
        (
            "replay-cli-*", "replay-tape-bad-*", "replay-help", "replay-version", "replay-speed-*", "replay-ticks-*",
            "replay-aircraft-*", "replay-drone-*", "replay-screen-*", "replay-menu-*", "replay-ui-*",
            "replay-panels",
        ),
    ),
    "replay-input": (
        "keys, mouse and scripted hand flying, input profiles",
        ("replay-input-*", "replay-keys-*", "replay-script-*"),
    ),
    "combat-tapes": (
        "recorded combat input replayed into the simulation",
        ("replay-tape-*", "!replay-tape-bad-*"),
    ),
    "replay-live": (
        "live flights, restarts, panels and audio",
        ("replay-live-*", "replay-quick-*", "replay-flight-*", "replay-audio-*"),
    ),
    "replay-settings": (
        "settings files, import errors, diagnostics",
        ("replay-settings-*", "replay-import-*", "replay-diag-*"),
    ),
    # Net lane: real UDP on this machine, a driver per scenario.
    "net-check": ("the dedicated server's start-up and --check", ("net-server-check",)),
    "net-fly": (
        "a server and bots over UDP: join, fly, chat, console, observe, scores, the King, PvP, a delayed observer, "
        "revival, the lead hold, the idle AI, Autobalance, debrief, clean exit",
        (
            "net-server-fight", "net-server-chat", "net-server-kick", "net-server-observe", "net-server-scores",
            "net-server-min-players", "net-server-king", "net-server-pvp", "net-server-hunt", "net-server-delay", "net-server-revive", "net-window-respawn-menu",
            "net-server-results", "net-server-away", "net-server-rejoin", "net-server-replies",
            "net-server-datalink", "net-server-datalink-lead", "net-server-smoke-pvp", "net-server-smoke-coop",
            "net-server-rate", "net-host-rate", "net-server-autobalance", "net-server-ai-respawn",
            "net-server-revive-150", "net-server-side-boxes", "net-server-lead-hold",
            "net-server-objective-respawn", "net-server-lineage-join", "net-server-lead-hold-callsign",
            "net-server-slow-load",
            "net-host-slow-build",
        ),
    ),
    "net-convert": (
        "a networked flight's capture converted into a replay",
        ("net-convert-*",),
    ),
    "net-discovery": ("finding games on the local network", ("net-discovery",)),
    "net-content": (
        "stage L's content and gaps: a bot whose import lacks an aircraft joins, is unable with the words, the gap "
        "and its end are logged",
        ("net-content-missing",),
    ),
    "net-builds": (
        "a 1.0 import made in the run against the profile's 1.02F: the same items, no build or difference line, a flight (slow: "
        "it imports the disc)",
        ("net-content-builds",),
    ),
    "net-master": (
        "the master server on this machine: mixed released/current populations, its limits under the flood tool, a server listing itself",
        ("net-master-*",),
    ),
    "net-listing": ("a dedicated server broadcasting itself on a master on this machine", ("net-master-listing", "net-window-master-setting")),
    "net-introduce": (
        "a bot joining a listed dedicated server through an introduction from a master on this machine",
        ("net-master-introduce",),
    ),
    "net-relay": (
        "a bot joining a listed dedicated server through the relay of a master on this machine",
        ("net-master-relay",),
    ),
    "net-migrate": (
        "host migration on this machine (stage K, slice K9): a hosting bot killed in a fight or leaving on purpose, "
        "pilots that stand by taking the game over, and host selection's reach and upload tests",
        ("net-migrate-kill", "net-migrate-handover", "net-reach-upload"),
    ),
    "net-migrate-relay": (
        "a listed hosting bot killed with a relayed bot in the game: the listing and the relay channel follow the new host",
        ("net-migrate-relay",),
    ),
    "net-window": (
        "the game itself over the network, including menu disconnect, partial debriefs and seat reuse",
        ("net-window-*",),
    ),
    "net-surface": (
        "a ground target online (protocol 22): a server and bots over the defended moving column, co-op and PvP, a Redfor ground start, and the area defenses of a mission with no target",
        ("net-surface-*", "net-redfor-*", "net-area-*"),
    ),
    "net-a2g": (
        "the air-to-ground round online (protocol 23): a lasing bot and a buddy bomber with a HARM over a radar site; "
        "a bomber dropped mid-pass whose attack the AI finishes (slice ST-K)",
        ("net-a2g-*", "net-ai-strike-*"),
    ),
}

def family_matches(family: str, name: str) -> bool:
    """True when `name` matches one of the family's patterns and none of its "!" exclusions."""
    patterns = FAMILIES[family][1]
    if any(fnmatch.fnmatchcase(name, p[1:]) for p in patterns if p.startswith("!")):
        return False
    return any(fnmatch.fnmatchcase(name, p) for p in patterns if not p.startswith("!"))


#: Scenarios cheap enough to stand for a lane when only the lane's scenario file changed.
LANE_SMOKE = {
    "flight": ("flight-maneuvers", "flight-stall", "flight-takeoff"),
    "ai": ("ai-fights", "ai-regression"),
    "menus": ("menus-screens", "menus-validate"),
    "replay": ("replay-recording", "replay-cli"),
    "net": ("net-check", "net-discovery"),
}

ALL_FAMILIES = tuple(FAMILIES)

# --------------------------------------------------------------------------
# Source paths -> families
# --------------------------------------------------------------------------


@dataclasses.dataclass(frozen=True)
class Rule:
    pattern: str
    families: tuple[str, ...]
    why: str
    windowed: bool = False  # the change touches rendering or windowed input: windows are worth opening
    unit_tests: tuple[str, ...] = ()  # Python test modules under tools/


def _r(pattern: str, families: Sequence[str], why: str, windowed: bool = False, unit_tests: Sequence[str] = ()) -> Rule:
    return Rule(pattern, tuple(families), why, windowed, tuple(unit_tests))


FLIGHT_CORE = ("flight-maneuvers", "flight-stall", "flight-takeoff", "flight-landing", "ai-fights")
AI_CORE = ("ai-fights", "ai-missions", "ai-orders", "ai-airfield", "ai-lead", "ai-regression", "ai-datalink")
MAIN_FAMILIES = (
    "replay-cli", "menus-creator", "flight-maneuvers", "flight-takeoff", "flight-landing", "ai-fights",
    "ai-airfield", "ai-lead", "ai-orders", "ai-regression", "flight-powered",
)
NET_FAMILIES = ("net-fly", "net-window", "net-convert", "net-surface", "net-a2g")
# What a change to host migration reaches besides the rest of the net lane (slice K9).
NET_MIGRATE = NET_FAMILIES + ("net-migrate", "net-migrate-relay")
RENDER_FAMILIES = ("windowed-menus", "flight-views", "instruments")
MENU_FAMILIES = ("menus-screens", "menus-creator", "menus-validate")

# The first matching rule wins. Patterns are fnmatch on the repository-relative path, where "*" also
# crosses "/".
RULES: tuple[Rule, ...] = (
    _r("crates/vendor/naga/*", RENDER_FAMILIES, "patched shader translator", windowed=True),
    _r("crates/tore-app/src/terrain_seams*", ("vegetation", "flight-views"), "terrain seam masks, colours and vegetation belts", windowed=True),
    _r("crates/tore-app/src/terrain_audit.rs", ("terrain-colors",), "CPU retail tile import audit"),
    _r("crates/tore-app/src/vegetation*", ("vegetation", "flight-views"), "vegetation placement and atlas rendering", windowed=True),
    _r("crates/tore-app/src/grass_blades*", ("vegetation", "flight-views"), "procedural grass geometry and GPU bindings", windowed=True),
    _r("crates/tore-app/assets/vegetation/*", ("vegetation", "flight-views"), "generated vegetation art", windowed=True),
    _r("tools/slice_vegetation_pass5.py", ("vegetation",), "supplied Sahara and top-view slicer", unit_tests=("test_slice_vegetation",)),
    _r("tools/slice_vegetation.py", ("vegetation",), "generated sprite slicer", unit_tests=("test_slice_vegetation",)),
    _r("tools/battery_scenarios/vegetation.py", ("vegetation",), "CPU vegetation regression coverage"),
    # Documentation and tests change no scenario output.
    _r("docs/*", (), "documentation"),
    _r("*.md", (), "documentation"),
    _r("crates/*/tests/*", (), "integration tests only"),
    _r("crates/*/examples/*", (), "examples are not run by the battery"),
    _r("crates/*/src/golden_tests/*", (), "golden tests only"),
    _r("crates/*/src/golden_tests.rs", (), "golden tests only"),
    _r("crates/*_tests.rs", (), "unit tests only"),
    _r("crates/*/tests.rs", (), "unit tests only"),
    _r("crates/*_tests/*", (), "unit tests only"),
    _r("crates/*/test_support.rs", (), "test support only"),
    _r("crates/*/test_support/*", (), "test support only"),
    _r("crates/*/assets/*", (), "bundled art and notices", windowed=True),
    # Exact checkpoints (docs/formats/checkpoint.md): coders only read state, and only tests restore a
    # checkpoint until host migration (stage K) uses one, so no scenario's output can change.
    _r("crates/*_checkpoint.rs", (), "checkpoint coders"),
    _r("crates/*/checkpoint.rs", (), "checkpoint traits and container"),
    _r("crates/*/checkpoint_*.rs", (), "checkpoint shared coders, records, scenarios and tests"),
    # Build configuration can change anything.
    _r("Cargo.toml", ALL_FAMILIES, "workspace manifest"),
    _r("Cargo.lock", ALL_FAMILIES, "dependency versions"),
    _r("rust-toolchain.toml", ALL_FAMILIES, "compiler version"),
    _r("crates/*/Cargo.toml", ALL_FAMILIES, "crate manifest"),
    _r("crates/*/build.rs", ALL_FAMILIES, "build script"),
    # A shared executor serves both the world and CPU render preparation.
    _r("crates/tore-workers/*", ALL_FAMILIES, "shared scoped worker execution", windowed=True),
    # tore-sim: flight model.
    _r("crates/tore-sim/src/models/variety/*", FLIGHT_CORE + ("flight-damage", "flight-powered"), "the powered-lift aircraft's parameters"),
    _r("crates/tore-sim/src/models/*", FLIGHT_CORE + ("flight-damage",), "per-aircraft flight tables"),
    _r("crates/tore-sim/src/flight.rs", FLIGHT_CORE + ("flight-powered",), "flight model; the powered-lift aircraft's start and step"),
    _r("crates/tore-sim/src/flight/powered/*", FLIGHT_CORE + ("flight-powered", "flight-damage"), "the powered-lift flight: rotors, jets, tiltrotor, stability, starts, hover hold's flight"),
    _r("crates/tore-sim/src/flight/*", FLIGHT_CORE, "flight model"),
    _r("crates/tore-sim/src/research.rs", FLIGHT_CORE + ("flight-powered",), "researched flight model; the off-airfield touchdown rule"),
    _r("crates/tore-sim/src/touchdown.rs", FLIGHT_CORE + ("flight-powered", "flight-damage"), "the shared touchdown rule"),
    _r("crates/tore-sim/src/native.rs", FLIGHT_CORE, "native flight research path"),
    _r("crates/tore-sim/src/native_objects.rs", FLIGHT_CORE, "native flight research path"),
    _r("crates/tore-sim/src/attitude.rs", FLIGHT_CORE, "attitude"),
    _r("crates/tore-sim/src/g_effects.rs", ("flight-maneuvers", "flight-damage", "ai-fights"), "G effects"),
    _r("crates/tore-sim/src/turbulence.rs", ("flight-environment", "flight-maneuvers"), "turbulence"),
    _r("crates/tore-sim/src/runway_wind.rs", ("flight-takeoff", "flight-landing", "flight-environment"), "runway wind"),
    _r("crates/tore-sim/src/autopilot.rs", ("flight-maneuvers", "flight-landing", "ai-fights", "flight-powered"), "autopilot; hover hold and the powered-lift A modes"),
    _r("crates/tore-sim/src/autopilot/*", ("flight-powered", "ai-strike", "ai-fights"), "the rotor laws shared by hover hold, the powered-lift A modes and the AI"),
    _r("crates/tore-sim/src/telemetry.rs", ("flight-maneuvers", "instruments"), "telemetry"),
    _r("crates/tore-sim/src/cheats.rs", ("flight-damage", "flight-maneuvers", "flight-powered"), "cheats; Easy flight physics"),
    # tore-sim: AI.
    _r("crates/tore-sim/src/ai/surface*", ("surface",), "the surface engagement controller"),
    _r("crates/tore-sim/src/ai/rotor_*", ("flight-powered", "ai-strike", "ai-fights"), "the AI's rotorcraft control and its probe", unit_tests=("test_battery_flight",)),
    _r("crates/tore-sim/src/ai/jet_lift.rs", ("flight-powered",), "the AI's jet-lift control"),
    _r("crates/tore-sim/src/ai/*", AI_CORE + ("radio", "ai-strike"), "AI"),
    # tore-sim: the flight data link's radar table.
    _r("crates/tore-sim/src/datalink.rs", ("ai-datalink", "ai-orders", "ai-fights"), "data link radar table"),
    _r("crates/tore-sim/src/datalink/*", ("ai-datalink", "ai-orders", "ai-fights"), "data link sort"),
    # tore-sim: combat, systems, sensors.
    _r("crates/tore-sim/src/combat.rs", ("flight-combat", "flight-damage", "ai-fights", "ai-damage"), "combat"),
    _r("crates/tore-sim/src/combat/live/parked*", ("surface", "flight-combat"), "parked aircraft targets"),
    _r("crates/tore-sim/src/combat/live/ground_sensing*", ("a2g", "flight-combat"), "what the ground pages read of the targets"),
    _r("crates/tore-sim/src/combat/live/agm*", ("a2g", "surface", "flight-combat"), "the air-to-ground missiles: A-G track, HARM cue, retarget"),
    _r("crates/tore-sim/src/combat/hardpoints*", ("a2g", "flight-combat", "ai-strike"), "where stores leave their stations (B3)"),
    _r("crates/tore-sim/src/combat/bomb_flight*", ("a2g", "ai-strike", "surface"), "shared bomb integration and prediction"),
    _r("crates/tore-sim/src/combat/ballistics*", ("a2g", "surface", "flight-combat"), "bomb motion and the impact predictor"),
    _r("crates/tore-sim/src/combat/guided*", ("a2g", "flight-combat"), "guided air-to-ground weapons: glide, cones and reach"),
    _r("crates/tore-sim/src/combat/live/guided*", ("a2g", "flight-combat", "ai-strike"), "the guided air-to-ground weapons' flight and sight, for a human's cockpit and an AI striker's"),
    _r("crates/tore-sim/src/combat/live/cockpit*", ("a2g", "flight-combat", "ai-strike"), "the strike cockpit every human and AI striker shares, and the AI's release gates"),
    _r("crates/tore-sim/src/combat/delivery*", ("a2g", "flight-combat"), "the bomb and rocket sights' delivery solution"),
    _r("crates/tore-sim/src/combat/live/delivery*", ("a2g", "flight-combat"), "the bomb and rocket sights in the live adapter"),
    _r("crates/tore-sim/src/combat/laser*", ("a2g", "flight-combat", "ai-strike"), "the laser designator, its spot and states"),
    _r("crates/tore-sim/src/combat/live/sighting*", ("a2g", "flight-combat", "ai-strike"), "the targeting sight and laser step, and the spots every aircraft's laser weapons home on"),
    _r("crates/tore-sim/src/combat/missiles/*", ("flight-combat", "ai-fights", "surface", "a2g", "ai-strike"), "shared energy flight, seekers and bounded predictions"),
    _r("crates/tore-formats/src/missile_data.rs", ("flight-combat", "surface", "a2g"), "authored physical weapon data"),
    _r("crates/tore-formats/src/missile_motors.rs", ("flight-combat", "surface", "a2g"), "motor fits, class control budgets and post-burn defeat policy"),
    _r("crates/tore-sim/src/combat/missiles.rs", ("flight-combat", "flight-damage", "ai-fights", "ai-damage", "surface", "a2g", "ai-strike"), "missile motion and prediction, and the terrain line a command launch and a laser spot need"),
    _r("crates/tore-sim/src/combat/*", ("flight-combat", "flight-damage", "ai-fights", "ai-damage"), "combat"),
    _r("crates/tore-sim/src/aircraft_systems/*", ("flight-damage", "flight-combat", "ai-damage", "instruments"), "aircraft systems"),
    _r("crates/tore-sim/src/ejection.rs", ("flight-damage",), "ejection"),
    _r("crates/tore-sim/src/sensors.rs", ("instruments", "flight-combat", "ai-fights", "a2g"), "sensors"),
    _r("crates/tore-sim/src/sensors/track/ground*", ("a2g", "instruments"), "the ground pages and their tests"),
    _r("crates/tore-sim/src/sensors/*", ("instruments", "flight-combat", "ai-fights", "a2g"), "sensors"),
    _r("crates/tore-sim/src/wreck.rs", ("flight-damage", "flight-environment"), "wrecks"),
    # tore-sim: world.
    _r("crates/tore-sim/src/airport.rs", ("airports", "flight-takeoff", "flight-landing", "ai-airfield", "menus-creator"), "airports"),
    _r("crates/tore-sim/src/stol.rs", ("airports", "menus-creator"), "STOL runway labels"),
    _r("crates/tore-sim/src/environment.rs", ("flight-environment", "flight-maneuvers", "terrain-colors"), "environment"),
    _r("crates/tore-sim/src/environment/*", ("flight-environment", "flight-maneuvers", "terrain-colors"), "environment"),
    _r("crates/tore-sim/src/clouds.rs", ("flight-environment",), "clouds"),
    _r("crates/tore-sim/src/vapor.rs", ("flight-environment",), "vapor"),
    _r("crates/tore-sim/src/acoustics.rs", ("replay-live",), "acoustics"),
    _r("crates/tore-sim/src/lib.rs", FLIGHT_CORE + ("ai-airfield",), "tore-sim public surface"),
    _r("crates/tore-sim/*", FLIGHT_CORE + ("ai-airfield", "flight-combat"), "tore-sim, unmapped file"),
    # tore-world.
    _r("crates/tore-world/src/ai_wings.rs", AI_CORE + ("radio", "ai-strike"), "wing AI"),
    _r("crates/tore-world/src/ai_wings/outcome.rs", AI_CORE + ("radio", "surface", "ai-strike"), "the mission result rule: the surface objectives join it"),
    _r("crates/tore-world/src/ai_wings/*", AI_CORE + ("radio", "ai-strike"), "wing AI"),
    _r("crates/tore-world/src/datalink.rs", ("ai-datalink", "ai-orders", "ai-fights"), "data link picture"),
    _r("crates/tore-world/src/datalink/*", ("ai-datalink", "ai-orders", "ai-fights", "link-cues", "ai-strike"), "data link picture; the order calls"),
    _r("crates/tore-world/src/airfield_radio.rs", ("ai-airfield", "radio", "airports"), "tower radio"),
    _r("crates/tore-world/src/comms.rs", ("radio", "replay-recording", "ai-fights"), "radio and crew calls"),
    _r("crates/tore-world/src/comms/*", ("radio", "replay-recording", "ai-fights"), "radio and crew calls"),
    _r("crates/tore-world/src/radio_calls.rs", ("radio", "replay-recording", "ai-fights", "ai-strike"), "radio calls"),
    _r("crates/tore-world/src/crew_voice.rs", ("radio", "replay-live"), "crew voice"),
    _r("crates/tore-world/src/situation.rs", ("radio", "replay-live"), "situation audio"),
    _r("crates/tore-world/src/combat.rs", ("flight-combat", "ai-fights", "ai-damage", "combat-tapes", "surface"), "combat in the world"),
    _r("crates/tore-world/src/combat_tape.rs", ("combat-tapes", "flight-combat"), "combat tapes"),
    _r("crates/tore-world/src/terrain/redrawn.rs", ("airports", "ai-airfield", "surface"), "redrawn airports: plans, AI points, ILS lines"),
    _r("crates/tore-world/src/terrain/redrawn/*", ("airports", "ai-airfield", "surface"), "redrawn airports: plans, AI points, ILS lines"),
    _r("crates/tore-world/airports/*", ("airports", "ai-airfield", "surface"), "redrawn airport plans"),
    _r("crates/tore-world/src/grounding/*", ("surface", "airports", "flight-environment"), "shared foundation terrain"),
    _r("crates/tore-world/src/grounding.rs", ("surface", "airports", "ai-airfield", "replay-recording"), "ground-object footing and contact geometry"),
    _r("crates/tore-world/src/terrain.rs", ("airports", "flight-environment", "ai-airfield", "menus-creator", "surface"), "terrain and airport lists"),
    _r("crates/tore-world/src/surface/*", ("surface", "airports", "flight-combat", "ai-fights", "net-surface"), "surface units: resolution, ids and sides; what the network sends of them"),
    _r("crates/tore-world/src/debrief.rs", ("menus-screens", "ai-fights", "surface") + NET_FAMILIES, "debrief evaluator; the multiplayer results rows; the surface tallies"),
    _r("crates/tore-world/src/debrief/*", ("menus-screens", "ai-fights") + NET_FAMILIES, "the multiplayer results rows' tests"),
    _r("crates/tore-world/src/seats.rs", ("ai-lead", "ai-fights"), "seats"),
    _r("crates/tore-world/src/frame.rs", ("flight-views", "instruments", "replay-recording", "link-cues"), "the flight frame"),
    _r("crates/tore-world/src/readout.rs", ("instruments", "flight-combat", "replay-live", "link-cues"), "the cockpit readout"),
    _r("crates/tore-world/src/snapshot.rs", ("replay-recording", "ai-fights"), "snapshots"),
    _r("crates/tore-world/src/mission_layout/*", ("surface", "ai-missions", "menus-creator"), "runtime placement availability"),
    _r("crates/tore-world/src/mission_layout.rs", ("ai-missions", "menus-creator"), "mission layout"),
    _r(
        "crates/tore-world/src/mission.rs", ("ai-missions", "menus-creator", "net-fly", "ai-strike"),
        "mission spec; its friendly-fire and loadouts lines and the loadout rule are a networked mission's",
    ),
    _r("crates/tore-world/src/resources.rs", ("ai-missions", "menus-creator", "airports"), "mission resource reads"),
    _r(
        "crates/tore-world/src/content.rs", ("net-content", "net-builds"),
        "the content digests (stage L): computed by the lobby and the dedicated server, which later slices wire in; "
        "no scenario reads them yet",
    ),
    _r("crates/tore-world/src/target_window.rs", ("instruments", "link-cues"), "target window"),
    _r("crates/tore-world/src/aircraft_type.rs", ("ai-fights", "menus-creator"), "aircraft types"),
    # Stage F phase 2's shared types (F2-0); single player never uses them.
    _r("crates/tore-world/src/score.rs", NET_FAMILIES, "score facts (networked games)"),
    _r("crates/tore-world/src/world/revive.rs", NET_FAMILIES + ("ai-lead",), "revival (networked games)"),
    _r(
        "crates/tore-world/src/world/handoff.rs",
        AI_CORE + ("replay-recording", "ai-strike", "net-a2g"),
        "the handoff between the AI and a human: the takeover of a ground attack (slice ST-K)",
    ),
    _r("crates/tore-world/src/world/replies.rs", NET_FAMILIES + ("radio",), "wingmen's replies"),
    _r("crates/tore-world/src/world.rs", AI_CORE + ("replay-recording", "ai-strike"), "mission world"),
    _r("crates/tore-world/src/world/*", AI_CORE + ("replay-recording", "ai-strike"), "mission world"),
    _r("crates/tore-world/*", AI_CORE + ("replay-recording",), "tore-world, unmapped file"),
    # tore-formats.
    _r("crates/tore-formats/src/flight_model.rs", FLIGHT_CORE, "flight model data"),
    _r("crates/tore-formats/src/flight_model/*", FLIGHT_CORE, "flight model data"),
    _r("crates/tore-formats/src/aircraft*", FLIGHT_CORE + ("menus-creator",), "aircraft data"),
    _r("crates/tore-formats/src/ejection.rs", ("flight-damage",), "ejection data"),
    _r("crates/tore-formats/src/strip.rs", ("airports", "flight-takeoff", "flight-landing", "ai-airfield"), "airport strips"),
    _r("crates/tore-formats/src/strip/*", ("airports", "flight-takeoff", "flight-landing", "ai-airfield"), "airport strips"),
    _r("crates/tore-formats/src/theater.rs", ("airports", "flight-environment", "ai-airfield", "menus-creator"), "theaters"),
    _r("crates/tore-formats/src/mission*.rs", ("ai-missions", "menus-creator", "menus-validate"), "missions"),
    _r("crates/tore-formats/src/radio.rs", ("radio", "replay-recording"), "radio data"),
    _r("crates/tore-formats/src/weather.rs", ("flight-environment",), "weather data"),
    _r("crates/tore-formats/src/weather/*", ("flight-environment",), "weather data"),
    _r("crates/tore-formats/src/weapons.rs", ("flight-combat", "menus-creator", "ai-fights"), "weapon data"),
    _r("crates/tore-formats/src/chat.rs", ("menus-validate",), "quick chat messages (CHAT.TXT)"),
    _r("crates/tore-formats/src/text.rs", ("menus-validate", "menus-screens"), "text decoding"),
    _r("crates/tore-formats/src/ui.rs", MENU_FAMILIES, "menu data", windowed=True),
    _r("crates/tore-formats/src/ui/*", MENU_FAMILIES, "menu data", windowed=True),
    _r("crates/tore-formats/src/font.rs", MENU_FAMILIES, "fonts", windowed=True),
    _r("crates/tore-formats/src/pic.rs", MENU_FAMILIES, "pictures", windowed=True),
    _r("crates/tore-formats/src/hud.rs", ("instruments", "menus-screens"), "HUD data", windowed=True),
    _r("crates/tore-formats/src/shape.rs", RENDER_FAMILIES, "shapes", windowed=True),
    _r("crates/tore-formats/src/parked_aircraft.rs", ("surface",), "parked aircraft records and gear"),
    _r("crates/tore-formats/src/carrier.rs", ("surface",), "carrier parts and decks"),
    _r("crates/tore-formats/src/static_object.rs", RENDER_FAMILIES, "static objects", windowed=True),
    _r("crates/tore-formats/src/music.rs", ("replay-live",), "music"),
    _r("crates/tore-formats/src/pcm.rs", ("replay-live",), "sound samples"),
    _r("crates/tore-formats/src/esa.rs", ("flight-environment",), "terrain data"),
    _r("crates/tore-formats/src/executable.rs", ("menus-validate", "replay-settings"), "importer data"),
    _r("crates/tore-formats/src/module.rs", ("menus-validate", "replay-settings"), "importer data"),
    _r("crates/tore-formats/src/dcl.rs", ("menus-validate", "replay-settings"), "importer decompression"),
    _r("crates/tore-formats/src/surface_set.rs", ("menus-validate", "replay-settings", "net-builds"), "what the import keeps for ground targets"),
    _r("crates/tore-formats/src/surface_set/*", ("menus-validate", "replay-settings", "net-builds"), "what the import keeps for ground targets"),
    _r("crates/tore-formats/src/lib.rs", ("menus-validate", "flight-maneuvers", "ai-fights"), "tore-formats public surface"),
    _r("crates/tore-formats/*", ("menus-validate", "menus-creator", "flight-maneuvers", "ai-fights"), "tore-formats, unmapped file"),
    # tore-input and friends.
    _r("crates/tore-input/src/pilot.rs", ("flight-maneuvers", "replay-input", "combat-tapes", "flight-powered"), "pilot controls; the powered-lift commands"),
    _r("crates/tore-input/src/recording.rs", ("replay-input", "replay-recording", "flight-powered"), "input recording; the tapes the powered-lift scenarios replay"),
    _r("crates/tore-input/*", ("replay-input", "flight-maneuvers"), "input bindings", windowed=True),
    _r("crates/tore-input-native/*", ("replay-input", "replay-settings"), "input devices", windowed=True),
    _r("crates/tore-replay/*", ("replay-recording", "ai-regression", "net-convert"), "replay format"),
    _r("crates/tore-diagnostics-native/*", ("replay-settings",), "diagnostics"),
    _r("crates/tore-extract/*", ("replay-settings", "menus-validate"), "extractor"),
    # Stage D crates. Until networked scenarios exist, only the import reaches a battery scenario.
    _r(
        "crates/tore-import/src/source.rs", ("menus-validate", "replay-settings", "net-builds"),
        "the import's source entry (build and importer); nothing in single player reads it",
    ),
    _r(
        "crates/tore-import/src/import.rs", ("menus-validate", "replay-settings", "net-builds"),
        "what the import selects and writes (net-content-builds imports the 1.0 disc)",
    ),
    _r(
        "crates/tore-import/src/surface.rs", ("menus-validate", "replay-settings", "net-builds"),
        "the ground target data marker and lookups",
    ),
    _r(
        "crates/tore-import/src/pack.rs", ("menus-validate", "replay-settings", "net-builds"),
        "the pack markers every start checks",
    ),
    _r("crates/tore-import/*", ("menus-validate", "replay-settings"), "importer and data folder"),
    _r("crates/tore-codec/*", NET_FAMILIES, "network encoding"),
    _r("crates/tore-net/src/master/*", ("net-master", "net-migrate-relay"), "the master server's wire and the browse client"),
    _r(
        "crates/tore-net/src/peers*", NET_MIGRATE,
        "the peers router in front of a joined socket, its reach answerer and reach tests (stage K, slice K6)",
    ),
    _r("crates/tore-net/*", NET_FAMILIES + ("net-discovery", "net-introduce", "net-relay"), "network transport"),
    _r("crates/tore-session/src/settings.rs", NET_FAMILIES + ("net-discovery",), "the King's settings registry"),
    _r("crates/tore-session/src/client/scores.rs", NET_FAMILIES, "the scores a game keeps and their words (tore-bot prints them)"),
    _r(
        "crates/tore-session/src/client/revival.rs", NET_FAMILIES,
        "the revival a game keeps, its words and the spawned planes (tore-bot prints them)",
    ),
    _r("crates/tore-session/src/host/revive.rs", NET_FAMILIES, "death and revival on the host"),
    _r(
        "crates/tore-session/src/host/away*", NET_FAMILIES,
        "the AI flying an idle player's aircraft on the host, and its tests (tore-bot --away)",
    ),
    _r(
        "crates/tore-session/src/host/path_tests.rs", NET_FAMILIES,
        "the lobby's player list carries each player's connection path, and the host's log says it (slice J6)",
    ),
    _r(
        "crates/tore-session/src/client/away*", NET_FAMILIES,
        "a game's Away and Back and the plane the AI flies for it, and their tests (tore-bot --away)",
    ),
    _r("crates/tore-session/src/client/results.rs", NET_FAMILIES, "the results a game keeps and their words (tore-bot prints them)"),
    _r("crates/tore-session/src/host/results*", NET_FAMILIES, "the results message at a mission's end, and its tests"),
    _r(
        "crates/tore-session/src/host/content*", NET_FAMILIES + ("net-content", "net-builds"),
        "stage L on the host: players' content, the gaps, the refusals and words, the content log, and their tests",
    ),
    _r(
        "crates/tore-session/src/client/content.rs", ("net-content", "net-builds"),
        "a player's own words about its import and the lobby's build and gap lines (tore-bot prints them)",
    ),
    _r(
        "crates/tore-session/src/host/king*",
        NET_FAMILIES + ("net-discovery",),
        "the King's lobby: the crown, settings, slot locks, visibility (tore-bot --king)",
    ),
    # Stage K (slice K0): the host steps its world through the journal, so every flight a host
    # flies passes through it; the seams refuse their requests until their slices land.
    _r("crates/tore-session/src/journal*", NET_MIGRATE, "the journal the host steps its world through (stage K)"),
    _r("crates/tore-session/src/wire/migration*", NET_MIGRATE, "stage K's message bodies and their tests"),
    _r("crates/tore-session/src/standby/*", NET_MIGRATE, "the standby's side of host migration (stage K)"),
    _r(
        "crates/tore-session/src/host/journal*", NET_MIGRATE,
        "the host's journal, which every tick the host steps passes, and its tests (stage K)",
    ),
    _r("crates/tore-session/src/host/standby*", NET_MIGRATE, "the host's standby stream and its tests (stage K)"),
    _r("crates/tore-session/src/host/resume*", NET_MIGRATE, "takeover and resume on the host and their tests (stage K)"),
    _r(
        "crates/tore-session/src/host/listing_part_tests.rs", NET_MIGRATE,
        "the listing part through the standby stream to a host that takes over (stage K, slice K7a)",
    ),
    _r(
        "crates/tore-session/src/host/rejoin*", NET_MIGRATE,
        "rejoin tokens and reservations, the session part that carries them, a slot's reservation in every lobby state "
        "(stage K), and their tests",
    ),
    _r(
        "crates/tore-session/src/host/succession*", NET_MIGRATE,
        "candidates and host selection on the host, the candidates part, and their tests (stage K, slice K6)",
    ),
    _r(
        "crates/tore-session/src/host/*state.rs", NET_MIGRATE,
        "the session's state parts and their coders beside the state they code (stage K)",
    ),
    _r("crates/tore-session/src/client/migrate*", NET_MIGRATE, "the client's side of host migration and its tests (stage K)"),
    _r(
        "crates/tore-session/src/client/rejoin*", NET_MIGRATE,
        "the client's side of rejoin: the token it keeps, its store and the Rejoin (stage K; tore-bot --token-file)",
    ),
    _r(
        "crates/tore-session/src/client/candidate*", NET_MIGRATE,
        "the client's side of host selection: its report, CPU measure, reach work and upload burst (stage K, slice K6)",
    ),
    _r(
        "crates/tore-session/src/client/migration_seams_tests.rs", NET_MIGRATE,
        "stage K's seams on the network simulator (slice K0's tests)",
    ),
    _r(
        "crates/tore-session/src/client/radar_page_tests.rs", NET_FAMILIES,
        "the cockpit readout a game shows through stalls on a slow round trip (bug B1; a real-data test for the full run)",
    ),
    _r("crates/tore-session/src/client/convert*", ("net-convert",), "capture conversion"),
    _r("crates/tore-session/src/client/seen.rs", ("net-convert",), "capture conversion"),
    _r("crates/tore-session/src/client/capture.rs", ("net-convert",), "captures"),
    _r("crates/tore-session/src/client/prediction.rs", ("net-convert",), "the own plane's prediction, which the conversion traces"),
    _r("crates/tore-session/src/fixture.rs", ("net-convert",), "the synthetic fight other crates' tests convert"),
    _r("crates/tore-session/src/bin/tore-bot.rs", NET_MIGRATE, "the bot, which keeps the capture the scenario converts and hosts and stands by in the migration scenarios"),
    _r(
        "crates/tore-session/src/bin/tore-bot/*", NET_MIGRATE,
        "the bot's hosting and standby (slice K9): the migration scenarios run it",
    ),
    _r(
        "crates/tore-session/*",
        NET_FAMILIES + ("net-discovery", "net-introduce", "net-relay", "net-content"),
        "network sessions, the host and tore-bot",
    ),
    _r(
        "crates/tore-server/*", ("net-check", "net-fly", "net-discovery", "net-listing", "net-content"),
        "dedicated server",
    ),
    _r("crates/tore-master/*", ("net-master", "net-migrate-relay"), "the master server, its configuration and its flood tool"),
    _r("crates/tore-realtime-native/*", ALL_FAMILIES, "host and shared-worker scheduling on macOS", windowed=True),
    _r("crates/tore-app/src/loading.rs", ("menus-screens", "menus-creator", "net-fly", "net-content", "replay-recording"), "flight loading workers and screen"),
    # tore-app: rendering (windowed).
    _r("crates/tore-app/src/*.wgsl", RENDER_FAMILIES, "shaders", windowed=True),
    _r("crates/tore-app/src/*renderer*.rs", RENDER_FAMILIES, "renderers", windowed=True),
    _r("crates/tore-app/src/scenery*", RENDER_FAMILIES, "scenery drawing", windowed=True),
    _r("crates/tore-app/src/instruments*", ("instruments", "windowed-menus", "link-cues"), "cockpit instruments", windowed=True),
    _r("crates/tore-app/src/hud*", ("instruments", "windowed-menus"), "HUD", windowed=True),
    _r("crates/tore-app/src/weapon_hud.rs", ("instruments", "windowed-menus", "link-cues"), "weapon HUD", windowed=True),
    _r("crates/tore-app/src/weapon_hud/guided.rs", ("a2g", "instruments"), "the guided weapons' release range scale"),
    _r("crates/tore-app/src/weapon_hud/agm*", ("a2g", "instruments"), "the air-to-ground missile HUD cues"),
    _r("crates/tore-app/src/weapon_hud/bombs*", ("a2g", "instruments", "windowed-menus"), "the HUD's bomb and rocket sights", windowed=True),
    _r("crates/tore-app/src/pylons.rs", RENDER_FAMILIES, "stores drawn on pylons", windowed=True),
    _r("crates/tore-app/src/stores_preview.rs", (), "the stores preview (a development aid)"),
    _r("crates/tore-app/src/render_snapshot.rs", RENDER_FAMILIES + ("menus-screens",), "snapshot drawing", windowed=True),
    _r("crates/tore-app/src/graphics*", RENDER_FAMILIES + ("menus-screens",), "graphics options", windowed=True),
    _r("crates/tore-app/src/flight_views.rs", ("flight-views", "windowed-menus"), "flight views", windowed=True),
    _r("crates/tore-app/src/view_compass.rs", ("flight-views",), "compass", windowed=True),
    _r("crates/tore-app/src/look.rs", ("flight-views",), "look controls", windowed=True),
    _r("crates/tore-app/src/camera.rs", RENDER_FAMILIES, "camera", windowed=True),
    _r("crates/tore-app/src/flight_canvas.rs", RENDER_FAMILIES, "flight canvas", windowed=True),
    _r("crates/tore-app/src/flight_map.rs", RENDER_FAMILIES + ("menus-screens", "menus-creator"), "flight map", windowed=True),
    _r("crates/tore-app/src/flight_map/*", RENDER_FAMILIES + ("menus-screens", "menus-creator"), "flight map view", windowed=True),
    _r("crates/tore-app/src/mission_brief.rs", ("menus-screens", "menus-creator") + RENDER_FAMILIES, "mission brief", windowed=True),
    _r("crates/tore-app/src/flight_map_preview.rs", ("menus-screens",), "flight map preview"),
    _r("crates/tore-app/src/target_select.rs", ("instruments", "net-window"), "human target selection", windowed=True),
    _r("crates/tore-app/src/scope.rs", RENDER_FAMILIES + ("link-cues",), "radar scope", windowed=True),
    _r("crates/tore-app/src/canvas_present.rs", RENDER_FAMILIES, "presentation", windowed=True),
    _r("crates/tore-app/src/static_art.rs", RENDER_FAMILIES, "art", windowed=True),
    _r("crates/tore-app/src/*_art.rs", RENDER_FAMILIES, "art", windowed=True),
    _r("crates/tore-app/src/aircraft_animation_probe*", ("flight-animation",), "CPU animation witnesses"),
    _r("crates/tore-app/src/*animation.rs", RENDER_FAMILIES + ("flight-animation",), "animation", windowed=True),
    _r("crates/tore-app/src/engine_material.rs", RENDER_FAMILIES, "materials", windowed=True),
    _r("crates/tore-app/src/f14_geometry.rs", ("flight-views",), "F-14 source geometry repairs", windowed=True),
    _r("crates/tore-app/src/surface_lighting.rs", RENDER_FAMILIES, "lighting", windowed=True),
    _r("crates/tore-app/src/lens_flare.rs", RENDER_FAMILIES, "lens flare", windowed=True),
    _r("crates/tore-app/src/celestial.rs", RENDER_FAMILIES, "sun and moon", windowed=True),
    _r("crates/tore-app/src/clouds.rs", RENDER_FAMILIES, "cloud drawing", windowed=True),
    _r("crates/tore-app/src/ocean.rs", RENDER_FAMILIES, "ocean", windowed=True),
    _r("crates/tore-app/src/weather.rs", RENDER_FAMILIES + ("flight-environment",), "weather drawing", windowed=True),
    _r("crates/tore-app/src/target_preview.rs", RENDER_FAMILIES, "target preview", windowed=True),
    _r("crates/tore-app/src/attitude.rs", RENDER_FAMILIES, "attitude drawing", windowed=True),
    _r("crates/tore-app/src/combat_view.rs", ("flight-combat", "windowed-menus", "link-cues"), "combat view", windowed=True),
    _r("crates/tore-app/src/pause_menu.rs", ("windowed-menus", "replay-live"), "pause menu", windowed=True),
    # tore-app: menus.
    _r("crates/tore-app/src/menu.rs", MENU_FAMILIES, "menus", windowed=True),
    _r("crates/tore-app/src/updates*", ("menus-screens",), "main-menu release checks and fixtures"),
    _r("crates/tore-app/src/rocker.rs", MENU_FAMILIES, "menu rockers", windowed=True),
    _r("crates/tore-app/src/locate.rs", ("menus-screens", "replay-settings"), "locate screen", windowed=True),
    _r("crates/tore-app/src/quick_mission.rs", MENU_FAMILIES + ("airports",), "Quick Mission creator", windowed=True),
    _r("crates/tore-app/src/quick_mission/*", MENU_FAMILIES + ("airports",), "Quick Mission creator", windowed=True),
    _r("crates/tore-app/src/ordnance_audit.rs", ("menus-creator",), "ordnance availability probe"),
    _r("crates/tore-app/src/ordnance.rs", ("menus-creator", "flight-combat"), "ordnance page", windowed=True),
    _r("crates/tore-app/src/controls_editor.rs", ("menus-screens", "replay-input"), "controls screen", windowed=True),
    _r("crates/tore-app/src/sound_screen.rs", ("menus-screens", "replay-settings"), "sound screen", windowed=True),
    _r("crates/tore-app/src/sound_prefs.rs", ("menus-screens", "replay-settings"), "sound preferences"),
    _r("crates/tore-app/src/preferences.rs", ("menus-screens", "replay-settings"), "preferences"),
    _r("crates/tore-app/src/debrief.rs", ("menus-screens", "ai-fights"), "debrief", windowed=True),
    _r("crates/tore-app/src/startup.rs", ("menus-creator", "replay-settings"), "start-up"),
    _r("crates/tore-app/src/assets.rs", ("menus-validate", "replay-settings"), "importer results"),
    _r("crates/tore-app/src/version.rs", ("replay-cli",), "version string"),
    _r("crates/tore-app/src/mirrors.rs", ("menus-screens",), "mirrors"),
    _r("crates/tore-app/src/input_catalog.rs", ("replay-input", "menus-screens"), "input catalog", windowed=True),
    # tore-app: input and flight shell (windowed).
    _r("crates/tore-app/src/input.rs", ("replay-input", "flight-maneuvers"), "input handling", windowed=True),
    _r("crates/tore-app/src/input_script.rs", ("replay-input", "replay-live"), "input scripts", windowed=True),
    _r("crates/tore-app/src/target_info.rs", ("replay-input", "replay-live"), "friend-or-foe cues", windowed=True),
    _r("crates/tore-app/src/flight_ui.rs", ("replay-live", "replay-input", "instruments"), "flight screen", windowed=True),
    _r("crates/tore-app/src/flight.rs", ("replay-live", "replay-input", "flight-maneuvers", "flight-takeoff"), "flight screen", windowed=True),
    _r("crates/tore-app/src/flight_watch.rs", ("replay-recording", "replay-live"), "replay watching", windowed=True),
    _r("crates/tore-app/src/audio*", ("replay-live", "radio"), "audio"),
    _r("crates/tore-app/src/flight_music.rs", ("replay-live",), "flight music"),
    _r("crates/tore-app/src/rwr_tone.rs", ("replay-live",), "RWR tones"),
    # tore-app: headless probes and tools.
    _r("crates/tore-app/src/flight_probe.rs", ("flight-maneuvers", "flight-stall", "flight-takeoff", "flight-landing"), "headless flight probe"),
    _r("crates/tore-app/src/powered_hud.rs", ("instruments", "windowed-menus", "flight-powered"), "the powered-lift HUD cluster", windowed=True),
    _r("crates/tore-app/src/variety_rotors.rs", ("flight-animation", "flight-powered"), "rotor, nacelle and nozzle drawing", windowed=True),
    _r("crates/tore-app/src/ai_roster_probe.rs", ("ai-regression", "ai-fights"), "AI roster probe"),
    _r("crates/tore-app/src/probe_invariants.rs", AI_CORE, "AI probe checks"),
    _r("crates/tore-app/src/formation_trace.rs", ("ai-fights", "ai-orders"), "formation trace"),
    _r("crates/tore-app/src/combat_smoke.rs", ("flight-combat",), "combat smoke"),
    _r("crates/tore-app/src/gunsight_probe.rs", ("flight-combat",), "AC-130 gunsight probe"),
    _r("crates/tore-app/src/gunsight_splash.rs", ("flight-combat",), "AC-130 mixed-group splash probe"),
    _r("crates/tore-app/src/missile_sag.rs", ("flight-combat",), "missile sag regression"),
    _r("crates/tore-app/src/missile_acceptance.rs", ("flight-combat",), "missile acceptance"),
    _r("crates/tore-app/src/tape_file.rs", ("combat-tapes", "flight-combat", "replay-cli"), "tape files"),
    _r("crates/tore-app/src/navigation.rs", ("flight-landing", "airports", "ai-airfield", "menus-creator"), "navigation and airport lists"),
    _r("crates/tore-app/src/airfield_audit.rs", ("airports", "ai-airfield"), "airfield audit"),
    _r("crates/tore-app/src/ils_survey.rs", ("airports", "flight-landing", "ai-airfield"), "ILS survey"),
    _r("crates/tore-app/src/bridge_audit.rs", ("surface",), "bridge placement, imported damage assets and captures"),
    _r("crates/tore-app/src/surface_dump/*", ("surface",), "objective placement and mission start audits"),
    _r("tools/objectives_audit.py", ("surface",), "objective audit runner"),
    _r("crates/tore-app/src/surface_dump.rs", ("surface",), "the surface dump and template sheets"),
    _r("crates/tore-app/src/surface_trace.rs", ("surface",), "the surface trace"),
    _r("crates/tore-app/src/flak_pass.rs", ("surface",), "the flak team probe (--flak-pass)"),
    _r("crates/tore-app/src/surface_drive.rs", ("surface",), "the surface movement drive"),
    _r("crates/tore-app/src/surface_battle.rs", ("surface",), "the surface battle drive and survey"),
    _r("crates/tore-app/src/surface_parked.rs", ("surface",), "the parked aircraft drive"),
    _r("crates/tore-app/src/city_fires.rs", ("surface", "replay-recording"), "the burning city fill buildings' fires and smoke (--city-fires); a replay's knocked-down buildings (nr-fx3)"),
    _r("crates/tore-app/src/terrain_colors.rs", ("terrain-colors",), "headless theater palette regression"),
    _r("crates/tore-app/src/night_glow.rs", ("render-night-light-slots",), "headless light-slot stability"),
    _r("crates/tore-app/src/night_glow*", ("surface", "replay-recording", "render-night-effect-lights"), "the night glow of explosions and fires and their surface light", windowed=True),
    _r("crates/tore-app/src/city_fill_dump.rs", ("surface",), "the city fill dump, and --city-fires, which surface-city-fires runs"),
    _r("crates/tore-app/src/effect_renderer.rs", ("render-night-effect-lights",), "effect sprites and sustained light", windowed=True),
    _r("crates/tore-app/src/impact_preview.rs", ("render-night-effect-lights",), "night lighting capture fixtures", windowed=True),
    _r("crates/tore-app/src/blast_preview.rs", ("replay-cli",), "the explosion and cluster burst preview (offscreen renders)"),
    _r("crates/tore-app/src/a2g_radar.rs", ("a2g",), "the ground pages drive"),
    _r("crates/tore-app/src/a2g_run/ai.rs", ("a2g", "ai-strike"), "the AI strike driver"),
    _r("crates/tore-app/src/a2g_run.rs", ("a2g", "ai-strike"), "the air-to-ground attack driver and its shared option parser"),
    _r("crates/tore-app/src/a2g_run/guided.rs", ("a2g", "ai-strike"), "the guided attacks, which the AI strike buddy run shares"),
    _r("crates/tore-app/src/a2g_run*", ("a2g",), "the air-to-ground attack driver"),
    _r("crates/tore-app/src/surface_scene.rs", ("surface", "replay-recording"), "surface scene bomb placement"),
    _r("crates/tore-app/src/surface_objective.rs", ("surface", "replay-recording"), "the ground target objective drive, which also records the run"),
    _r("crates/tore-app/src/diagnostics.rs", ("replay-settings",), "diagnostics"),
    _r("crates/tore-app/src/performance.rs", ("flight-maneuvers",), "performance counters"),
    _r("crates/tore-app/src/replay/net_convert.rs", ("net-convert",), "converting a capture into a replay"),
    _r("crates/tore-app/src/replay/net_effects.rs", ("net-convert",), "the smoke, contrails and gun rounds a converted capture carries"),
    _r("crates/tore-app/src/replay/*", ("replay-recording", "ai-regression", "replay-live"), "recording and replay screens"),
    _r("crates/tore-app/src/net/rejoin_check.rs", ("net-fly", "net-window"), "server-confirmed return-slot checks and timeouts", unit_tests=("test_battery_net",)),
    _r("crates/tore-app/src/net/observe.rs", ("net-window", "replay-live"), "the observer screen's recording and watch; the window scenario watches a server", windowed=True),
    _r("crates/tore-app/src/net/hosting*", ("net-window",), "the game's host thread; the hosted-game scenario reaches it", windowed=True),
    _r("crates/tore-app/src/net/keepalive_tests.rs", (), "the joined game's keepalive tests (real time, cargo test only)"),
    _r("crates/tore-app/src/net/standby_tests.rs", (), "the game's standby and takeover tests (real time on loopback, cargo test only)"),
    _r(
        "crates/tore-app/src/net/standby.rs", ("net-window",),
        "a joined game's standby and the migration lines (stage K); a windowed game joins and stands by",
        windowed=True,
    ),
    _r("crates/tore-app/src/net/join_tests.rs", (), "the join through the master's tests (real time on loopback, cargo test only)"),
    _r(
        "crates/tore-app/src/net/rejoin_game_tests.rs", (),
        "the game's rejoin token kept and sent again (real time on loopback, cargo test only; stage K, slice K7b)",
    ),
    _r(
        "crates/tore-app/src/net/rejoin_store.rs", ("net-window",),
        "the game's rejoin tokens in rejoin-v1.conf (stage K, slice K7b); a windowed game that joins keeps its token in it",
        windowed=True,
    ),
    _r("crates/tore-app/src/widgets/*", ("menus-screens", "net-window"), "the multiplayer widget kit; the Direct Connection and Internet Lobby screens draw it (menus-snap-direct*, menus-snap-internet*)", windowed=True),
    _r("crates/tore-app/src/multiplayer_menu.rs", ("menus-screens", "net-window"), "multiplayer exit menu and safe return to main", windowed=True),
    _r("crates/tore-app/src/direct_screen/*", ("menus-screens", "net-window"), "the Direct Connection screen; its snapshot states are menus-snap-direct*", windowed=True),
    _r("crates/tore-app/src/internet_screen/*", ("menus-screens", "net-window", "net-listing"), "the Internet Lobby screen; its snapshot states are menus-snap-internet*; `--browse` is judged by net-master-listing", windowed=True),
    _r("crates/tore-app/src/lobby_screen/*", ("menus-screens", "net-window"), "the lobby screen; its snapshot states are menus-snap-lobby*", windowed=True),
    _r("crates/tore-app/src/net/lobby_chat.rs", ("menus-screens", "net-window"), "the lobby's chat box and line; the lobby screen draws it (menus-snap-lobby*)", windowed=True),
    _r("crates/tore-app/src/net/scoreboard.rs", ("net-window",), "K's score board in a networked flight", windowed=True),
    _r(
        "crates/tore-app/src/net/away.rs", ("net-window",),
        "the game's away detection, Back at the first flight input and the banner (unit tests; a networked flight "
        "reaches it)", windowed=True,
    ),
    _r("crates/tore-app/src/net/search.rs", ("net-discovery",), "the local-network game search; --find-games, which net-discovery runs, and the Direct Connection screen use it"),
    _r("crates/tore-app/src/net/lookup.rs", ("net-window",), "the typed-address lookup thread; the Direct Connection screen reaches it", windowed=True),
    _r("crates/tore-app/src/net/browse.rs", ("net-listing", "menus-screens"), "the Internet Lobby's browse loop; `tore-app --browse`, which net-master-listing runs, and the Internet Lobby screen use it"),
    _r("crates/tore-app/src/net/telemetry.rs", ("net-window",), "the anonymous statistics: the install id, the notice and a player's Report; a game listed from the command line and the Internet Lobby screen reach them", windowed=True),
    _r("crates/tore-app/src/net/settings.rs", ("net-window",), "the remembered multiplayer settings; --connect and --host reach them", windowed=True),
    _r("crates/tore-app/src/net/*", ("net-window",), "the game's network play: joining, hosting, chat, debrief, files", windowed=True),
    # main.rs holds the command line and the probes, including the AI probe's scripted pilot, so it reaches
    # every kind of headless run.
    _r("crates/tore-app/src/main.rs", MAIN_FAMILIES, "command line, probes and start-up wiring"),
    _r("crates/tore-app/*", MAIN_FAMILIES, "tore-app, unmapped file"),
    # Tools.
    _r("tools/battery_selection.py", (), "the selection map", unit_tests=("test_battery_selection",)),
    _r("tools/test_battery_selection.py", (), "the selection map's tests", unit_tests=("test_battery_selection",)),
    _r("tools/quick_check.py", (), "the quick check", unit_tests=("test_quick_check",)),
    _r("tools/test_quick_check.py", (), "the quick check's tests", unit_tests=("test_quick_check",)),
    _r("tools/battery.py", ("flight-maneuvers", "net-check"), "the battery runner (one cheap scenario of each kind runs end to end)", unit_tests=("test_battery", "test_battery_selection", "test_battery_net")),
    _r("tools/test_battery.py", (), "the battery runner's tests", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/animation.py", ("flight-animation",), "CPU animation regressions", unit_tests=("test_battery_animation",)),
    _r("tools/battery_scenarios/flight.py", LANE_SMOKE["flight"], "flight scenarios", unit_tests=("test_battery_flight",)),
    _r("tools/battery_scenarios/_powered.py", ("flight-powered",), "the powered-lift flight scenarios", unit_tests=("test_battery_flight",)),
    _r("tools/test_battery_flight.py", (), "flight scenario tests", unit_tests=("test_battery_flight",)),
    _r("tools/battery_scenarios/render.py", ("flight-views",), "render capture scenarios", windowed=True, unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/ai.py", LANE_SMOKE["ai"], "AI scenarios", unit_tests=("test_battery_ai",)),
    _r("tools/battery_scenarios/surface.py", ("surface",), "surface unit scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/surface_move.py", ("surface",), "surface movement scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/surface_battle.py", ("surface",), "surface units fighting each other", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/surface_parked.py", ("surface",), "parked aircraft scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/surface_fires.py", ("surface",), "burning city fill scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/replay_city.py", ("replay-recording",), "the recorded city strike scenario", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/replay_views.py", ("replay-recording", "flight-views"), "replay cameras and live F12 hold", unit_tests=("test_battery", "test_replay_checks")),
    _r("tools/battery_scenarios/a2g_radar.py", ("a2g",), "ground page scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/a2g_release_origin.py", ("a2g",), "release origin scenario", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/bomb_flight.py", ("a2g",), "recorded bomb flight acceptance", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/a2g_bombs.py", ("a2g",), "bomb and rocket motion scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/a2g_cluster.py", ("a2g",), "cluster bomb scenarios", unit_tests=("test_battery", "test_battery_a2g")),
    _r("tools/battery_scenarios/a2g_catalog.py", ("a2g", "menus-creator"), "A2G catalog scenario", unit_tests=("test_battery", "test_battery_a2g")),
    _r("tools/battery_scenarios/a2g_guided.py", ("a2g",), "guided bomb and laser missile scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/a2g_missiles.py", ("a2g",), "air-to-ground missile scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/a2g_sights.py", ("a2g",), "bomb and rocket sight scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/surface_objective.py", ("surface",), "ground target objective scenarios", unit_tests=("test_battery",)),
    _r("tools/battery_scenarios/_ai_fuzz.py", LANE_SMOKE["ai"], "AI fuzz scenarios", unit_tests=("test_battery_ai",)),
    _r("tools/_ai_fuzz_cmd.py", LANE_SMOKE["ai"], "AI fuzz command", unit_tests=("test_battery_ai",)),
    _r("tools/battery_scenarios/_debrief.py", LANE_SMOKE["ai"] + LANE_SMOKE["flight"], "the debrief kill and hit check shared by the AI and flight lanes", unit_tests=("test_battery_ai", "test_battery_flight")),
    _r("tools/battery_scenarios/_strips.py", LANE_SMOKE["ai"] + ("flight-takeoff",), "short strip helper", unit_tests=("test_battery_ai", "test_battery_flight")),
    _r("tools/test_battery_ai.py", (), "AI scenario tests", unit_tests=("test_battery_ai",)),
    _r("tools/battery_scenarios/menus.py", LANE_SMOKE["menus"], "menu scenarios", unit_tests=("test_battery_menus",)),
    _r("tools/test_battery_menus.py", (), "menu scenario tests", unit_tests=("test_battery_menus",)),
    _r("tools/battery_scenarios/ai_defense.py", ("ai-regression",), "AI missile defensive flying", unit_tests=("test_battery_ai_defense",)),
    _r("tools/test_battery_ai_defense.py", (), "AI missile defense regression checks", unit_tests=("test_battery_ai_defense",)),
    _r("tools/battery_scenarios/ai_strike.py", ("ai-strike",), "AI strike driver scenarios", unit_tests=("test_battery_ai_strike",)),
    _r("tools/test_battery_ai_strike.py", (), "AI strike scenario tests", unit_tests=("test_battery_ai_strike",)),
    _r("tools/test_battery_a2g.py", (), "air-to-ground scenario tests", unit_tests=("test_battery_a2g",)),
    _r("tools/battery_scenarios/scripts/*", ("replay-input", "flight-takeoff"), "scripts for windowed runs", windowed=True, unit_tests=("test_replay_checks",)),
    _r("tools/battery_scenarios/_replay_*.py", LANE_SMOKE["replay"], "replay scenarios", unit_tests=("test_replay_checks",)),
    _r("tools/battery_scenarios/replay.py", LANE_SMOKE["replay"], "replay scenarios", unit_tests=("test_replay_checks",)),
    _r("tools/test_replay_checks.py", (), "replay check tests", unit_tests=("test_replay_checks",)),
    _r("tools/battery_scenarios/replay_feedback.py", ("replay-feedback",), "feedback window flows", unit_tests=("test_battery_selection",)),
    _r("crates/tore-app/src/feedback.rs", ("replay-feedback",), "feedback form"),
    _r("crates/tore-session/src/recording/*", ("replay-feedback", "net-convert"), "authoritative replay recording"),
    _r("crates/tore-replay/src/feedback.rs", ("replay-feedback",), "feedback event layout"),
    _r("tools/battery_scenarios/net.py", LANE_SMOKE["net"], "net scenarios", unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/net_timer.py", ("net-window",), "mission countdown and deadline captures", windowed=True),
    _r("crates/tore-app/src/mission_timer.rs", ("net-window",), "mission countdown presentation", windowed=True),
    _r("tools/battery_scenarios/net_lobby.py", ("net-window",), "the lobby panels scenario", windowed=True, unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/net_observe.py", ("net-window",), "the observer screen scenario", windowed=True, unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/net_datalink.py", ("net-window", "link-cues"), "the data link's cues in a multiplayer flight (stage G, slice G10)", windowed=True, unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/net_accept.py", ("net-window",), "stage F phase 2's acceptance scenarios: the away menu's Spawn in Aircraft and a host's Leave Game (slice F2-X)", windowed=True, unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/target_select.py", ("instruments", "net-window"), "human targeting live checks", unit_tests=("test_target_select",)),
    _r("tools/battery_scenarios/net_surface.py", ("net-surface",), "a ground target online (protocol 22, slice N1)", unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/net_a2g.py", ("net-a2g",), "the air-to-ground round online (protocol 23, slice NA)", unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/net_debrief.py", ("net-window",), "partial reports, revival and retained AI/human scores", windowed=True),
    _r("tools/battery_scenarios/net_screens.py", ("net-window",), "the game's rejoin and its HUD through a host migration (stage K, slice K7b)", windowed=True),
    _r("tools/test_battery_net.py", (), "net scenario tests", unit_tests=("test_battery_net",)),
    _r("tools/battery_scenarios/*", ALL_FAMILIES, "battery scenarios, unmapped file", unit_tests=("test_battery",)),
    _r("tools/agent-run.sh", ("windowed-menus",), "the windowed-run wrapper", windowed=True),
    _r("tools/calibrate_theaters.py", ("replay-cli",), "Tacview geography calibration", unit_tests=("test_calibrate_theaters",)),
    _r("tools/test_*.py", (), "tool tests"),
    _r("tools/*", (), "tooling outside the battery"),
    _r(".githooks/*", (), "git hook"),
    _r(".github/*", (), "CI"),
    _r(".claude/*", (), "agent settings"),
    _r("assets/*", (), "repository assets"),
)

#: Files no rule mentions select every family (trimmed by the budget); see `Plan.unmapped`.
_UNMAPPED_WHY = "no rule for this file, so anything might be affected"


def rule_for(path: str) -> Optional[Rule]:
    for rule in RULES:
        if fnmatch.fnmatchcase(path, rule.pattern):
            return rule
    return None


# --------------------------------------------------------------------------
# Durations and scheduling
# --------------------------------------------------------------------------

DEFAULT_SECONDS = 10.0
DEFAULT_BUDGET = 120.0
DEFAULT_PER_FAMILY = 12


def default_jobs() -> int:
    """Half the cores, between 4 and 12: the quick tier leaves the rest of the machine usable."""
    import os

    return max(4, min(12, (os.cpu_count() or 8) // 2))
FULL_RUN_COVERAGE = 0.9


def load_durations(battery_dir: Path, names: Iterable[str]) -> dict[str, float]:
    """Seconds per scenario from the newest full battery run, gaps filled from newer partial runs.

    A run is "full" when it covers at least 90 percent of the scenarios now defined. Scenarios the full
    run lacks take their time from any newer run that has them, newest first; the rest are missing from the
    result (callers use DEFAULT_SECONDS).
    """
    wanted = set(names)
    runs = sorted((p for p in battery_dir.glob("*/results.json")), key=lambda p: p.parent.name, reverse=True)
    newer: list[dict[str, float]] = []
    for path in runs:
        try:
            rows = json.loads(path.read_text())
            seconds = {r["name"]: float(r["seconds"]) for r in rows}
        except (OSError, ValueError, KeyError, TypeError):
            continue
        if len(wanted & seconds.keys()) >= FULL_RUN_COVERAGE * len(wanted):
            merged = {n: s for n, s in seconds.items() if n in wanted}
            for partial in newer:  # newest first: fill only what the full run lacks
                for n, s in partial.items():
                    if n in wanted and n not in merged:
                        merged[n] = s
            return merged
        newer.append(seconds)
    merged = {}
    for partial in newer:  # no full run anywhere: best effort
        for n, s in partial.items():
            if n in wanted and n not in merged:
                merged[n] = s
    return merged


def makespan(seconds: Sequence[float], workers: int) -> float:
    """Wall-clock time to run jobs of these lengths on `workers` slots, longest first."""
    if not seconds:
        return 0.0
    workers = max(1, workers)
    load = [0.0] * min(workers, len(seconds))
    heapq.heapify(load)
    for s in sorted(seconds, reverse=True):
        heapq.heappush(load, heapq.heappop(load) + s)
    return max(load)


def estimate_wall(chosen: Sequence["Pick"], jobs: int, windows: int) -> float:
    """Estimated wall clock: every job shares `jobs` slots, and windowed ones also fit `windows` at once."""
    everything = makespan([p.seconds for p in chosen], jobs)
    windowed = makespan([p.seconds for p in chosen if p.scenario.windowed], windows)
    return max(everything, windowed)


# --------------------------------------------------------------------------
# Choosing
# --------------------------------------------------------------------------

AIRCRAFT_ORDER = ("f18", "rafale", "f14", "a4e", "x31", "mig29", "su27", "mig21", "su25", "mig23", "su35", "f22", "f22n", "faxx")
THEATER_ORDER = ("ukr", "apa", "bal", "cub", "egy", "fra", "gre", "ira", "kurile", "lfa", "nsk", "pgu", "spa", "tviet", "vla", "wta")
_AC_RANK = {a: i for i, a in enumerate(AIRCRAFT_ORDER)}
_TH_RANK = {t: i for i, t in enumerate(THEATER_ORDER)}


_THEATER_TOKEN = re.compile(r"^(" + "|".join(THEATER_ORDER) + r")(\d*|f)v?$")


def variant_key(name: str) -> tuple[str, Optional[str], Optional[str], int]:
    """Splits a scenario name into a shape (aircraft, theater and numbers blanked) and its variants.

    `flight-takeoff-f18-ukr-1` and `flight-takeoff-rafale-apa-10` share the shape
    `flight-takeoff-<ac>-<th>-<n>`; so do `flight-land-f18-bal3v-1` (a terrain-and-airport token) and
    `flight-fault07-f18` and `flight-fault31-f18` (`flight-fault<n>-<ac>`).
    """
    parts: list[str] = []
    aircraft = theater = None
    number = 0
    for token in name.split("-"):
        if token in _AC_RANK:
            aircraft = aircraft or token
            parts.append("<ac>")
            continue
        m = _THEATER_TOKEN.match(token)
        if m:
            theater = theater or m.group(1)
            parts.append("<th>")
            digits = re.search(r"\d+", token[len(m.group(1)):])
            if digits and not number:
                number = int(digits.group(0))
            continue
        digits = re.search(r"\d+", token)
        if digits and not number:
            number = int(digits.group(0))
        parts.append(re.sub(r"\d+", "<n>", token))
    return "-".join(parts), aircraft, theater, number


@dataclasses.dataclass
class Pick:
    scenario: object  # battery.Scenario
    seconds: float
    families: list[str]
    tier: int  # 0: preferred variant of its kind, 1: a second aircraft or theater, ...
    why: str = ""


@dataclasses.dataclass
class Plan:
    base: str = ""
    changed: list[str] = dataclasses.field(default_factory=list)
    by_rule: dict[str, list[str]] = dataclasses.field(default_factory=dict)  # why -> files
    families: dict[str, list[str]] = dataclasses.field(default_factory=dict)  # family -> reasons
    unmapped: list[str] = dataclasses.field(default_factory=list)
    unit_tests: list[str] = dataclasses.field(default_factory=list)
    picks: list[Pick] = dataclasses.field(default_factory=list)
    needs_window: list[str] = dataclasses.field(default_factory=list)  # families with only windowed scenarios
    windows_used: bool = False
    budget: float = 0.0
    jobs: int = 1
    windows: int = 1
    estimate: float = 0.0
    over_budget: bool = False
    skipped_for_budget: int = 0
    candidates: int = 0

    @property
    def scenarios(self) -> list:
        return [p.scenario for p in self.picks]


def _rank_variants(group: list, max_tier: int) -> list[tuple[object, int]]:
    """Orders one kind of scenario by preference and diversity; returns (scenario, tier) for the first few.

    The first is the preferred aircraft and theater (F/A-18D, Ukraine, lowest airport number). Each next one
    prefers a different aircraft and a different theater from those already taken.
    """
    rest = list(group)
    seen_ac: set = set()
    seen_th: set = set()
    out: list[tuple[object, int]] = []
    for tier in range(max_tier + 1):
        if not rest:
            break

        def order(item):
            _, ac, th, number, seconds_hint = item
            novelty = (ac is not None and ac not in seen_ac) + (th is not None and th not in seen_th)
            ac_rank = _AC_RANK.get(ac, 99)
            th_rank = _TH_RANK.get(th, 99)
            return (-novelty, max(ac_rank, th_rank), ac_rank + th_rank, number, seconds_hint)

        best = min(rest, key=order)
        rest.remove(best)
        out.append((best[0], tier))
        seen_ac.add(best[1])
        seen_th.add(best[2])
    return out


def choose(
    families: dict[str, list[str]],
    scenarios: Sequence,
    durations: dict[str, float],
    budget: float,
    jobs: int,
    windows: int,
    allow_windows: bool,
    max_tier: int = 1,
    per_family: int = DEFAULT_PER_FAMILY,
) -> tuple[list[Pick], list[str], int, int]:
    """Returns (picks, families that need a window, candidates considered, candidates skipped for the budget)."""
    seconds_of = lambda s: durations.get(s.name, DEFAULT_SECONDS)  # noqa: E731
    members: dict[str, list] = {}
    needs_window: list[str] = []
    for fam in families:
        matching = [s for s in scenarios if family_matches(fam, s.name)]
        runnable = [s for s in matching if allow_windows or not s.windowed]
        if matching and not runnable:
            needs_window.append(fam)
        members[fam] = runnable

    # Tier of each candidate: its rank inside its kind (same shape of name), among this run's candidates.
    candidates: dict[str, object] = {}
    for fam_members in members.values():
        for s in fam_members:
            candidates[s.name] = s
    groups: dict[str, list] = {}
    for s in candidates.values():
        key, ac, th, number = variant_key(s.name)
        groups.setdefault(key, []).append((s, ac, th, number, seconds_of(s)))
    tier_of: dict[str, int] = {}
    for group in groups.values():
        for s, tier in _rank_variants(group, max_tier):
            tier_of[s.name] = tier

    picks: dict[str, Pick] = {}
    count: dict[str, int] = {fam: 0 for fam in members}

    def add(s, fam: str) -> None:
        if s.name in picks:
            if fam not in picks[s.name].families:
                picks[s.name].families.append(fam)
                count[fam] += 1
            return
        picks[s.name] = Pick(s, seconds_of(s), [fam], tier_of.get(s.name, max_tier + 1))
        count[fam] += 1

    # 1. At least one scenario of every family: the cheapest of its preferred variants (or one already chosen).
    for fam, fam_members in members.items():
        if not fam_members:
            continue
        shared = [s for s in fam_members if s.name in picks]
        if shared:
            add(min(shared, key=seconds_of), fam)
            continue
        preferred = [s for s in fam_members if tier_of.get(s.name) == 0] or fam_members
        add(min(preferred, key=seconds_of), fam)

    # 2. Fill the budget: preferred variants first, then a second aircraft or theater; fastest first inside
    #    a round. No family gets more than `per_family`, and no extra scenario may take more than a third of
    #    the budget by itself (one long run would leave every other slot idle for the whole budget).
    skipped = 0
    longest = budget / 3
    order = sorted(
        (s for s in candidates.values() if tier_of.get(s.name, max_tier + 1) <= max_tier and s.name not in picks),
        key=lambda s: (tier_of[s.name], seconds_of(s), s.name),
    )
    for s in order:
        owners = [fam for fam, fam_members in members.items() if s in fam_members and count[fam] < per_family]
        if not owners:
            continue
        if seconds_of(s) > longest:
            skipped += 1
            continue
        trial = list(picks.values()) + [Pick(s, seconds_of(s), [], tier_of[s.name])]
        if estimate_wall(trial, jobs, windows) <= budget:
            for fam in owners:
                add(s, fam)
        else:
            skipped += 1
    return list(picks.values()), needs_window, len(candidates), skipped


def plan_for(
    changed: Sequence[str],
    scenarios: Sequence,
    durations: dict[str, float],
    budget: float,
    jobs: int,
    windows: int = 3,
    windowed: str = "auto",
    base: str = "",
    max_tier: int = 1,
    per_family: int = DEFAULT_PER_FAMILY,
) -> Plan:
    """`windowed` is "auto" (only when a changed file touches rendering or windowed input), "yes" or "no"."""
    plan = Plan(base=base, changed=list(changed), budget=budget, jobs=jobs, windows=windows)
    want_windows = windowed == "yes"
    for path in changed:
        rule = rule_for(path)
        if rule is None:
            plan.unmapped.append(path)
            plan.by_rule.setdefault(_UNMAPPED_WHY, []).append(path)
            for fam in ALL_FAMILIES:
                plan.families.setdefault(fam, []).append(f"{path} (unmapped)")
            continue
        plan.by_rule.setdefault(rule.why, []).append(path)
        for fam in rule.families:
            plan.families.setdefault(fam, []).append(f"{path} ({rule.why})")
        for module in rule.unit_tests:
            if module not in plan.unit_tests:
                plan.unit_tests.append(module)
        if rule.windowed and windowed == "auto":
            want_windows = True
    if windowed == "no":
        want_windows = False
    plan.windows_used = want_windows
    if plan.families:
        picks, needs_window, plan.candidates, plan.skipped_for_budget = choose(
            plan.families, scenarios, durations, budget, jobs, windows, want_windows, max_tier, per_family
        )
        plan.needs_window = needs_window
        plan.picks = sorted(picks, key=lambda p: -p.seconds)  # longest first packs best
        plan.estimate = estimate_wall(plan.picks, jobs, windows)
        plan.over_budget = plan.estimate > budget
    return plan


def format_plan(plan: Plan, names_per_family: Optional[int] = 8) -> str:
    """The plan as text; `names_per_family` limits how many scenario names are listed per family (None: all)."""
    lines = [f"Quick battery selection (changes since {plan.base or 'the given base'}):"]
    if not plan.changed:
        lines.append("  nothing changed, nothing to select")
        return "\n".join(lines)
    lines.append(f"  {len(plan.changed)} changed file(s):")
    for why, files in sorted(plan.by_rule.items()):
        shown = ", ".join(files[:4]) + (f", and {len(files) - 4} more" if len(files) > 4 else "")
        lines.append(f"    {why}: {shown}")
    if plan.unmapped:
        lines.append(f"  NOT IN THE MAP (every family selected, trimmed to the budget): {', '.join(plan.unmapped)}")
    if plan.unit_tests:
        lines.append(f"  unit tests for the battery's own files: {', '.join(plan.unit_tests)}")
    if not plan.families:
        lines.append("  no scenario can be affected by these files (documentation, tests or tooling only)")
        return "\n".join(lines)
    lines.append(f"  {len(plan.families)} scenario family(ies) affected:")
    for fam, reasons in plan.families.items():
        chosen = [p for p in plan.picks if fam in p.families]
        if fam in plan.needs_window:
            state = "only windowed scenarios here (use --with-windows yes)"
        else:
            state = f"{len(chosen)} chosen"
        lines.append(f"    {fam}: {state}  [{FAMILIES[fam][0]}; because of {reasons[0]}" + (f" +{len(reasons) - 1}" if len(reasons) > 1 else "") + "]")
    lines.append(
        f"  chosen {len(plan.picks)} of {plan.candidates} candidate scenarios"
        f" ({'windows allowed' if plan.windows_used else 'headless only'}), "
        f"estimated {plan.estimate:.0f} s wall at {plan.jobs} jobs (budget {plan.budget:.0f} s)"
        f", {sum(p.seconds for p in plan.picks):.0f} s of scenario time"
    )
    if plan.over_budget:
        lines.append("  over budget: one scenario of every family is kept even when that costs more than the budget")
    if plan.skipped_for_budget:
        lines.append(f"  {plan.skipped_for_budget} more candidate(s) left out to stay inside the budget")
    shown: set[str] = set()
    lines.append("  scenarios (seconds each):")
    for fam in plan.families:
        chosen = [p for p in plan.picks if fam in p.families and p.scenario.name not in shown]
        shown.update(p.scenario.name for p in chosen)
        if not chosen:
            continue
        chosen.sort(key=lambda p: p.scenario.name)
        names = [f"{p.scenario.name}{'*' if p.scenario.windowed else ''} {p.seconds:.0f}" for p in chosen]
        more = ""
        if names_per_family is not None and len(names) > names_per_family:
            more = f", and {len(names) - names_per_family} more"
            names = names[:names_per_family]
        lines.append(f"    {fam} ({sum(p.seconds for p in chosen):.0f} s, {len(chosen)}): " + ", ".join(names) + more)
    if any(p.scenario.windowed for p in plan.picks):
        lines.append("  * opens a window (through tools/agent-run.sh)")
    return "\n".join(lines)


# --------------------------------------------------------------------------
# Git
# --------------------------------------------------------------------------


def _git(root: Path, *args: str) -> str:
    done = subprocess.run(["git", *args], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    if done.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)}: {done.stderr.strip()}")
    return done.stdout


def default_base(root: Path) -> str:
    """The merge base with `multiplayer`, or HEAD~1 when on it (or when there is no such branch)."""
    def rev(ref: str) -> Optional[str]:
        try:
            return _git(root, "rev-parse", "--verify", "--quiet", ref + "^{commit}").strip() or None
        except RuntimeError:
            return None

    head = rev("HEAD")
    for trunk in ("multiplayer", "origin/multiplayer"):
        tip = rev(trunk)
        if not tip:
            continue
        if tip == head:
            return "HEAD~1" if rev("HEAD~1") else "HEAD"
        return _git(root, "merge-base", "HEAD", trunk).strip()
    return "HEAD~1" if rev("HEAD~1") else "HEAD"


def changed_files(root: Path, base: str, head: Optional[str] = None) -> list[str]:
    """Paths changed from `base` to `head` (default: the working tree, plus untracked files)."""
    if head:
        names = _git(root, "diff", "--name-only", base, head).splitlines()
    else:
        names = _git(root, "diff", "--name-only", base).splitlines()
        names += _git(root, "ls-files", "--others", "--exclude-standard").splitlines()
    return sorted({n for n in names if n})


def tracked_files(root: Path) -> list[str]:
    return _git(root, "ls-files").splitlines()
