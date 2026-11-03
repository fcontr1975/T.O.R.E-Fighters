# Parity plan

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

One page. What is specified, what is built, what is next. Milestones live in
[ROADMAP.md](ROADMAP.md); how agents work is in [AGENTS.md](../AGENTS.md).

**Missile energy overhaul, 2026-10-26.** New worlds use one finite-energy
model and physical decoy pursuit for supported powered guided weapons.
Public/class masses, sourced range anchors and measured A2A envelopes are
recorded with per-store motors and paired fitted conditions. John's round-3
decision removes the geometry drag multiplier while retaining the newer-Sparrow
refit and turning-player advice. Drag returns to the round-2 fits. Regenerated
measurements match round 2 outside Sparrow; Linux checks and all 35 battery
scenarios pass, with current prediction costs in the baseline.
Phase 3 remains stopped pending lead review; its old results are invalidated.
This is John's opinionated extension, not a retail parity claim.
[Behaviour](spec/missile-energy.md), [baseline](baselines/missile-energy.md).

**Air-to-ground weapons, 2026-10-11.** A player can now designate ground
targets (the A-G radar page, the FLIR, the targeting sight, the HARM page), use
CCIP and level-release bomb sights with John's "T" guide, fire rocket pods, guided
bombs, laser missiles, JDAMs and the ground missiles, and see the laser and the
release-range indicator. Every weapon class was flown headless on two carriers
([acceptance](baselines/air-to-ground.md), [spec](spec/air-to-ground.md)). Online
under protocol 23 (slice NA) networked clients see the new displays, the lobby
carries the Pave pods and `net-a2g-loopback` flies a lasing and a buddy bomber.
Open: an AGM-65A
range question, and the not-built items in the spec. The AI strike round (AI
wings attacking ground targets) is next and needs John's go-ahead.

**Retail map detail, 2026-09-23.** John selected source textures, artwork,
scenery and variants. All sixteen base maps and 59 variants now load, with
Kurile's named artwork, full-resolution scenery pages and visible main-shape
placements. Generic land tiling and static variant composition are fitted.
Shaders and expanded landscapes are [future work](ROADMAP.md#future-terrain-enhancements).
[Evidence and limits](baselines/retail-terrain-review.md#implementation-validation).

Airports are connected for the sixteen base theaters: source static placements,
individual ground targets, runway support, tower text/verified recorded replies,
and automatic ILS at or below 4,000 feet above airport ground. Campaign overlays,
additional tower speech and further visual parity remain open. The creator now
supports player ground starts with named airport selection and preserved restart;
[validation](baselines/ground-start.md). [Implementation slices and acceptance](ROADMAP.md#airport-and-ground-object-expansion).

Parity is measured by **expression of feature**: does the player experience what
they experience in Fighters Anthology? A behaviour's provenance, `spec-derived`,
`native`, `fitted`, `opinionated`, records where it came from and never gates
acceptance.

Heading/altitude autopilot and toggleable waypoint guidance are implemented from
the requested USNF-ATF behavior. Waypoint mode currently falls back to heading
hold; mission route import is pending. NAV INFO can select eligible airports
by distance, with a separate empty mission mode. See [scope and provenance](spec/autopilot.md).

Takeoff/flap/contact and low-speed rotation corrections are implemented
([acceptance](baselines/takeoff-acceptance.md)). Cannons now release individual
bullets with intermittent tracers across the roster
([rules](spec/damage-smoke.md#individual-cannon-rounds)). Human testing should
focus on runway rotation, easing the stick after liftoff, continuous gun fire,
and the [expanded HUD with NAV/gun startup defaults](spec/hud-layout.md).

[Ejection](spec/ejection.md) now includes player confirmation, surviving-pilot
escape, original art/audio and fitted AI recovery decisions. Healthy AI aircraft
above 200 feet AGL cannot auto-eject. Campaign recovery and additional crew remain
open. See [validation](baselines/ejection.md).

The remaining [retail flight views](spec/flight-views.md) are implemented on
F4-F9/F12, with reference modifiers and V to save Other View. Camera placement
and missing-subject behavior remain fitted; [validation](baselines/flight-views.md).

## The decision this plan follows

**D30, 2026-09-15. Parity by expression of feature.** Recorded from John's
direction after eighteen plan revisions were built on a misreading of "replicate
only native functionality":

1. Parity is 1:1 gameplay parity by expression of feature, not a recreation of
   the original program's code, control flow, caches or RNG ordering.
2. "Native" is a provenance label only. It is never a requirement or a gate.
3. Ground, terrain and object contact is reclassified as **opinionated**,
   authored behaviour, no longer waiting on a recovered native producer. In
   practice this means both things: the ground contact that exists today is
   accepted as shipped, and contact is now a feature to design deliberately
   rather than a gap to be filled by recovery.
4. Research mode produces prose specs under [`spec/`](spec/). Implementation mode
   reads a spec and builds the behaviour idiomatically.
5. `spec-derived` is the default provenance for gameplay code. Fitted and
   opinionated components do not have to be replaced before acceptance.

> John first referred to this as D27. That ID was already taken by "NE-00.1p
> scheduler/clock ownership" in the frozen environment plan, whose log ends at
> D29, so the strategy was renumbered **D30** on 2026-09-16, the next free ID,
> which leaves every existing link and reference intact. See
> [the realignment report](doc-realignment-2026-09-15.md).

The [live map](spec/flight-map.md) is implemented as a requested addition: Shift-M,
right-side category toggles, buildings off by default, current detections and
unknown markers, without Escape-menu filters. Surface detection and cartography remain fitted.

## Specs

Behaviour specs live in [`spec/`](spec/). One file per feature a player would
name, with the numbers a player would notice.

| Spec | Covers | Status |
| --- | --- | --- |
| [Missiles](spec/missiles.md) | Four guidance types, 63-candidate inventory, pitbull, range, motor and guidance lifetime | Implemented for A2A stores and, since the air-to-ground round, for the surface seekers (Mavericks, SLAM, HARM, Harpoon, Exocet, Kickback, AS-7, AS-15) flown from ground designations; surface units fire 13 radar and 6 infrared SAM and ship missile profiles (2026-10-10); [missile validation](baselines/missiles.md), [air-to-ground acceptance](baselines/air-to-ground.md), [HUD cleanup and tone limits](baselines/hud-cleanup.md) |
| [Aircraft radar](spec/radar.md) | Twelve-aircraft radar stats, automatic range modes, installed visual and ECM records, and look-down evidence | Implemented as one shared component, with the retail air-to-ground, IR-ground and HARM pages added by the air-to-ground round; [component guide](radar.md), [validation](baselines/radar.md) |
| [Air-to-ground](spec/air-to-ground.md) | Ground designation, the A-G radar and HARM page, the targeting sight and laser, bomb sights and warnings, guided bombs, rocket pods, cluster bombs and the ground missiles: 40 records in twelve classes plus the Pave pods | Implemented and flown headless on two carriers per record (2026-10-11); not built: a JDAM release-at-the-bar mode, the HUD time-to-impact row, Tab gun fire and a viewer laser overlay; clients see the new displays under protocol 23 ([online](MULTIPLAYER.md#air-to-ground-online)); [acceptance](baselines/air-to-ground.md) |
| [Roster expansion](spec/roster-aircraft.md) | Seven REDFOR/F-22A initial player ports | [Acceptance and limits](baselines/aircraft-roster-expansion.md) |
| [Additional aircraft](spec/additional-aircraft.md) | F-14D, A-4E and X-31 source flight configuration and fitted presentation | Initial ports implemented; [acceptance](baselines/aircraft-fa-expansion.md) |
| [AI experience](spec/ai-experience.md) | Experience channels, Quick Mission and mission skill rules, enemy-skill override, tactical thresholds, G exemption, family scope | Specified; implemented as isolated `tore-sim::ai::experience`, Quick Mission hookup is partial |
| [AI behavior](spec/ai.md) | Fighter decisions, timing, pursuit, targeting, steering and terrain, seeker gates, ammunition, wing orders and formations, threat warnings and countermeasures, routes and fuel | Established rules implemented as isolated components with synthetic tests; open items in the [M1e backlog](ROADMAP.md#ai-backlog-2026-09-17); Quick Mission hookup is partial |
| [Surface defenses](spec/surface-defenses.md) | Quick Mission ground targets in 16 theaters: templates, rolls, jitter and relocation, starts, SAM batteries, AAA tuning, flak, resupply, parked aircraft, objectives, multiplayer and replays | Implemented (John's request, 2026-10-10); [acceptance](baselines/surface-defenses.md) |
| [Ocean](spec/ocean.md) | Short ripples, close pixelation and distance filtering; original textures/colors | Implemented; [acceptance](baselines/ocean.md) |
| [Terrain shorelines](spec/terrain-shorelines.md) | Beach/water coverage and absence of land-color strips | Implemented; validation in the viewer baseline |
| [Ground textures and map detail](spec/terrain-detail.md) | Source terrain scale, named artwork, scenery scope and shader boundaries | Source detail expansion implemented for 75 static layouts; fitted land tiling and variant composition, dynamic states remain open |

The research to build the first specs from already exists: recovered numbers are
in [`formats/`](formats/) (weather, native flight, quick mission, ordnance menu,
theater, weapons, aircraft) and measured results are in
[`baselines/`](baselines/). Writing a spec is mostly a matter of lifting the
player-visible numbers out of those files and leaving the byte layouts behind.

## What is built

| Area | State | Provenance | Evidence |
| --- | --- | --- | --- |
| In-flight situation music | Nine retail scores chosen by rank and situation, boundary-timed downgrades, once-per-flight success and home music | spec-derived selection, fitted TORE inputs and in-flight mission result | [flight music](spec/flight-music.md#current-tore-state) |
| Combat and passing sounds | Correct original IR growl, percentage/lock gain, delayed explosions, spatial pass cues and external booms | spec-derived recording identities, opinionated realism and fitted acoustics | [sound](spec/sound.md), [audio guide](audio.md) |
| Main menu and Choose Activity | Original art, fonts, sounds, five backgrounds | native assets, spec-derived layout | [main-menu](baselines/main-menu.md) |
| Quick Mission creator | Briefing screen, aircraft and theater selection, editable fields | mixed | [creator/ordnance](baselines/creator-ordnance.md) |
| Load Ordnance screen | Original art, supported weapon and fuel edits; normal catalog requires aircraft compatibility, Cheat shows all supported imports | mixed | [creator/ordnance](baselines/creator-ordnance.md), [catalog checks](baselines/ordnance-catalog.md) |
| Theaters | 16 base maps selectable (59 variants load through `--theater ~CODE`, developer only); named/numbered artwork, fitted generic land and full-size scenery textures render. Dynamic scenery and campaign progression remain open | native data, fitted rendering | [terrain review](baselines/retail-terrain-review.md) |
| Weather | Day/night palettes, horizon, sun/moon/stars, cloud sheets, fog maps | mixed | [weather](baselines/weather.md), [review](baselines/weather-review.md) |
| 36 retail aircraft plus F/A-XX | Source cockpits, reviewed equipment, initial exterior rigs and hybrid flight, including six powered-lift types on the VTOL overhaul's rotor, nozzle and nacelle physics; remaining moving surfaces and special-system acceptance are tracked separately | source-derived configuration and documented fitted flight/animation laws | [Variety matrix](spec/aircraft-variety.md), [flight evidence](baselines/variety-flight.md), [powered-lift flight](spec/powered-lift-flight.md) |
| Ground contact and landing | Runway contact, taxi, brakes, touchdown; taxiway-queued ground starts, airport/wing speech and AI takeoff, landing and return to base ([airfield spec](spec/ai-airfield.md)) | **opinionated** contact; spec-derived airfield sequence with fitted flight control | [land foundation](baselines/native-land-foundation.md) |
| Input | Keyboard, gamepad, joystick, profiles, rumble, rebinding; default keys follow the original's in-flight key table, including chaff and flares | spec-derived keyboard layout ([keyboard](spec/keyboard.md)), opinionated authored layer and T.O.R.E-only keys | [input](baselines/input.md) |
| Weapons | 135 definitions imported; development range with manual firing, damage fixtures, ECM | mixed | [weapons systems](baselines/weapons-systems.md), [manual weapons](baselines/manual-weapons.md) |
| Sensors | One shared radar, infrared and visual component across the expanded roster, with explicit absent equipment: imported capability profiles, contacts, one fire-control track, click selection, history, jammer noise, the RCS exposure page and radar weapon support | **opinionated** detection/notch/jamming/RCS tuning over spec-derived equipment data | [radar](baselines/radar.md) |
| Surface defenses | The Quick Mission ground target in every theater and the base layouts' air defenses fight: batteries, guns, flak, ships, columns, supply trucks, parked aircraft, wrecks, objectives and the debrief; online under protocol 22 and in replay format 3 | retail records and tables, spec-derived phases, fitted and opinionated (John, 2026-10-10) rules recorded per component | [spec](spec/surface-defenses.md), [acceptance](baselines/surface-defenses.md) |
| Combat AI | Spec-derived components in `tore-sim::ai` plus a per-actor controller, steering adapter and actor-owned mission runtime. Quick Mission launches independent AI wings by default; `--fixture-wings` keeps the straight-flight setup. Idle aircraft follow their own wing leader in delta formation. Surface units (SAM, AAA, flak, ships, armed vehicles) run their own controller since the 2026-10-10 surface round; the other aircraft families are not implemented, and AI wings do not attack ground targets | spec-derived, with named fitted rules where the spec leaves a branch open; each is recorded per actor | [AI research](baselines/ai-research.md), [delivery stages](ROADMAP.md#1e-ai), [provenance](behavior-provenance.md) |

Flight has three selectable paths and they stay distinct: the compatibility `--legacy-flight`, the
default hybrid `--researched-flight`, and the restricted `--native-flight-tables`
research path. Do not change the default without being asked.

## Next

**Surface objectives and air defenses, 2026-10-10 (built, awaiting the full
battery and John's in-game check).** John authorized surface AI on 2026-10-10.
The Quick Mission's ground target is live in all sixteen theaters with its
retail template, defended at the chosen AAA and SAM strength, nudged and
relocated per flight; SAM batteries, guns, flak, ships, columns, supply trucks
and simulated parked aircraft fight or burn as specified, the base layouts'
defenses wake on both sides, and the objective and debrief count it, single
player and online ([spec](spec/surface-defenses.md),
[acceptance](baselines/surface-defenses.md)). Next, each its own round: the
air-to-ground player weapons, then AI strike with the ground weapons pass, the
real-scale airfields and the city fill
([roadmap](ROADMAP.md#next-rounds-after-the-surface-round)). Runway width is
deferred by John, with the remaining work listed there.


Existing implementation track: **M1 air-to-air awareness and engagement**.
The retail terrain expansion above adds no autonomous behavior. The
[development specification](spec/ai-awareness.md) defines visual cones,
four-tier memory, searching in Target view, shared AI/RWR missile information,
blinking incoming-missile plots, skill-based defensive timing, jink/notch/dive,
chaff/flare responses and mission roles. Visual awareness, frozen memory and
search/Target-view activity, missile defense and shared RWR missile information
are implemented. Mission roles, six sentence-style Quick Mission objective/survival selectors
and player-relative Shift-4 Survive/Destroy labels are connected. Full campaign routes/scoring and human balance
review remain separate work. The rules are authored behavior, not a claim of retail
parity. [Stage validation](baselines/ai-awareness.md). The
[delivery sequence](ROADMAP.md#m1-air-to-air-awareness-delivery) builds
observations/memory first, then search, missile defense and RWR, mission
priorities and integrated combat acceptance. Existing actor-owned sensors,
stores, flight and controllers provide the foundation; seeker activation/pitbull
and broader mission integration remain partial. The acceptance target is the current
selectable roster at all four resolved skills. Surface AI and other behavior
families remain outside this scope. `--fixture-wings` and all three flight paths
remain available.

**Visual contact and incoming-fire repair is implemented.** The
[F-22/Su-27 investigation](baselines/visual-awareness-under-fire.md) led to
side/rear lookout, mission-permitted leader contact release and target-independent
gun/hit defense. The [contract](spec/visual-awareness-under-fire.md) records the
authored tuning; the [implementation baseline](baselines/incoming-fire-implementation.md)
records aircraft/skill/adapter coverage and remaining limits. Headless probes now
select opponents, geometry, skill and flight adapter, inject controlled threats,
and record actual AI attitude. Radar stealth and normal adapter defaults are
unchanged. Full weather visibility, cockpit blind sectors and retail timing
remain separate work; surface firing AI was built in its own round
(2026-10-10, below).

**AI gun employment implemented.** Expired attempts retry, and live guns use
physical bursts, observed-motion lead and barrel alignment with per-round debit.
[Validation](baselines/ai-gun-implementation.md) includes sustained Su-27 fights
at every skill with both flight models, non-stealth controls and shared-physics
regressions. [Authored rules and remaining approximations](spec/ai-gun-employment.md)
cover gun tracking and original skill/accuracy policy. Difficult turning fights
remain a tuning area, separate from the repaired permanent weapon timeout.

**AI fault response implemented.** The AI branch incorporates main's Realistic
NPC system damage. Pilots now commit to recovery and make protective control
requests from live faults. Dummies and health thresholds are unchanged.
[Rules and limitations](spec/systems-damage.md#ai-pilot-response-to-faults),
[rebase and validation](baselines/ai-damage-response.md).

1. **Missile tuning and remaining evidence.** Stages 1 through 5 shipped for
   current stores, with controlled emitter fixtures. Four guidance types, silent
   active flight, same-target reacquisition through guidance expiry, inherited
   velocity, BORESIGHT, HUD and fitted tone are implemented. The
   [baseline](baselines/missiles.md) records 3,920 reach cases and the remaining
   fitted tuning, unavailable human review and original evidence gaps. The
   air-to-ground round (2026-10-10 and 11) later added the ground weapons and
   their catalog entries; no combat AI was added, and the AI wings still do not
   attack the ground.
2. **Finish the shared aircraft radar tuning pass.** The component John
   requested on 2026-09-16 shipped on the same day: shared profiles and contact
   state using PT signatures with authored look-down, notch and jamming, the
   RCS/aspect display, persistent click selection, Y history and infrared A2A,
   one fire-control track only, and detectable destroyed aircraft. Target-view
   IFF is unchanged; the A-G, IR-ground and HARM pages came with the
   air-to-ground round. Stages 1 to 4 of the
   [component guide](radar.md#delivery-and-acceptance) are done. Stage 5 is
   partly done: all twelve aircraft produce the capability summary and pass their
   combat smokes, but no side-by-side tuning review of the twelve has happened,
   so the presets and matchups have not been played against each other yet.
   [Capability spec](spec/radar.md), [validation](baselines/radar.md). No AI
   work.
3. **Continue behaviour specs.** Start with the features that already have
   the most recovered numbers and the least prose: weather, then flight
   envelope/departure, then the quick-mission and ordnance screens. Each spec
   replaces a pile of source notes with one page a player would recognise.
4. **Ground contact and landing as an authored feature.** Contact is opinionated
   now. Make takeoff, landing, taxi and deck behaviour feel right and hold up in
   tests; stop waiting on a recovered contact producer.
5. **Maneuver audio and rumble**, then the remaining
   [flight-response](research/flight-response-plan.md) items.
6. **Continue F-14D, A-4E and X-31 acceptance.** Initial FA-only ports are implemented,
   requested by John on 2026-09-16. Resolve the [documented limits](baselines/aircraft-fa-expansion.md),
   especially original X-31 vector animation schedules and remaining pitch/yaw response
   tuning. PT roll response and low-speed auxiliary control are implemented.
7. **Remaining weather work**: wind, turbulence and vapor.

## Frozen

These were plans. They are now archives under [`research/`](research/), kept for
their recovered facts, dated checkpoints and evidence links. Their gates, status
columns and sequencing are not authoritative.

- [native environment and systems plan](research/native-environment-systems-plan.md)
- [progress log](research/progress.md)
- [weather plan](research/weather-plan.md)
- [flight response plan](research/flight-response-plan.md)
- [ordnance plan](research/ordnance-plan.md)
- [quick mission plan](research/quick-mission-plan.md)
- [menu parity matrix](research/menu-parity-matrix.md)
- [weapons implementation plan](research/weapons-plan.md)
