# Mission replays

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Every flight records itself so it can be watched again from any viewpoint,
exported to Tacview, and turned into logs that explain what the AI, the
weapons, the flight model and the radio did, with exact numbers. This is an
**opinionated addition requested by John on 2026-09-26**; the original game
has no such feature. Its "Replay Last Mission" button means *fly the last
mission again* and is unrelated. Every design choice below is an agent
decision unless it says otherwise.

A recording keeps **what happened**, not the stick inputs. Replaying inputs
would need frame-exact determinism the game does not have, while a record of
states seeks instantly, plays backwards, feeds Tacview directly and keeps the
exact values that happened.

The code lives in the dependency-free `crates/tore-replay` crate: the data
model, the writer, the bounded reader and every export. It knows nothing
about the simulation or the renderer; the app converts its own state into
the crate's model once per tick.

The replay HUD camera uses the current locally saved HUD colour and brightness
when the viewer opens. Its existing tape whitening and softness still apply.
Lighting changes during a flight are not recorded, and
old recordings remain unchanged. [Cockpit lighting](spec/cockpit-lighting.md).

Ground-pad foundations change reconstructed terrain contact under protocol 63
(2026-10-21). Replay bytes and readers are unchanged. Existing recordings still
load, but their world uses the corrected foundation terrain; old simulation
checksums are not evidence of agreement with this version near changed sites.
See [grounding](formats/objects-and-shapes.md#grounding-every-object-stands-on-the-bottom-of-its-shape).

## Recording

### When

Every flight records itself; there is nothing to switch on. Recording starts
when a flight starts (Free Flight, a Quick Mission, the range), right after
the scene is placed, and the first frame is the scene before the first
tick. It stops when the flight ends: End mission (after the debrief is
taken, so the footer carries its result), Restart (the old recording
finishes and a new one starts), quitting the game, or re-importing media.
Alt+F4 is a normal quit and finishes the recording, including over flight
settings screens. A forced process kill instead leaves a recoverable `.partial`.
The session log names each file as it starts and when it is saved; nothing
is printed to the terminal. Captures, `--smoke-test` and frame timing runs
do not record. `TORE_RECORD_MISSIONS=0` turns recording off for a run and
`=1` forces it on, for example to time frames with recording on and off.

**Ctrl+B** in flight marks the moment in the recording ("Bookmark N saved"),
for "something odd just happened". It is the `bookmark` action ("Mark replay
moment", Game and menus), rebindable like any other. Fighters Anthology
gives Ctrl+B no command, so it follows the
[key placement rules](INPUT.md#key-placement) for T.O.R.E-only commands.
The debug panels' right-click takes no key.

### Where and what name

Recordings live in `replays/` in the app data folder (which
`TORE_DATA_DIR` overrides), named after the UTC date and time the flight
started, the map and the player's aircraft:
`2026-09-26_1540_UKR_F18.tore-replay`. A second flight starting in the same
minute gets `-2`, `-3` and so on. The time is UTC, like the header's; the
game has no time-zone data without a new dependency (agent decision,
2026-09-26). Map names drop the `~` of variant layouts, and the aircraft
part is the exact identity: the F/A-XX is `FAXX`, never the F-22N it
borrows from.

### Auto-delete

`replays-v1.conf`, beside the other settings files, holds the rules:
auto-delete on or off, keep the last N recordings (default 20) or delete
those older than N days (default 30), and the recordings marked Keep. It is
on by default with keep the last 20, so recordings never fill a disk
unnoticed (agent decision, 2026-09-26). The
[Replays screen](#auto-delete-settings) changes the rules and the Keep
marks. Cleanup runs when a recording finishes and when the Replays screen
opens. It deletes only files it
can prove it may: a recording's name and the format's magic bytes, not
marked Keep, not the recording in progress, and not a `.partial` file
written in the last ten minutes, which another running game may own.
Anything else in the folder is never touched.

### How

The recorder runs inside the flight's fixed 120 Hz tick. Right after the
AI's step, when combat and the AI have both written the tick's poses, it
reads the same render snapshot live flight draws (see the
[architecture](ARCHITECTURE.md#mission-recordings)) plus the flight data a
snapshot does not carry: airspeed, G, fuel, the pilot's controls, ground
contact, and whether the pilot is still flying it. It compares them with
the previous tick to find what
changed, and hooks later in the tick add the radio lines delivered, the
sounds released and cockpit messages. Encoding and writing happen on a
background thread fed by a queue two seconds deep. Live flight never waits
for the disk: if the queue is ever full, that tick's frame is dropped and
the next frame starts with a `system.gap` event naming the missing ticks.
A headless probe recording (`--record-mission`) has no frame rate to
protect, so it waits for the writer instead and never has gaps.
Events noted between ticks (a pause, a bookmark, a wing order) go on the
tick that was on screen. The commands a player gives are applied by the next
tick's command phase, and the recorder opens that tick only after the phase, so
it notes them, and the cockpit messages they give, on the tick that was on
screen.
Headless AI probes explicitly enable real AI poses, so their recordings include
bank and pitch instead of the old straight-flight fixture orientation.
`--verify-render` also compares each living AI aircraft's drawn attitude with
its simulated body axes before checking recording reconstruction.

Recording reads only state the tick already computed. The behaviour probes
give byte-identical output with recording on and off, and the simulation's
golden fingerprints are unchanged.

What it costs, measured on the three-minute headless probes on an 8-core
Apple Silicon Mac with nothing else running: with 9 aircraft the recorder
takes about 10 microseconds of the simulation's tick (5 of them for the
reasons and display trees; draining the communication journal takes well
under a microsecond), and the whole process uses about 35 microseconds more
CPU a tick with the writer thread, against a tick of 8,333 microseconds.
With 5 aircraft the recorder takes about 7 microseconds (3 for the reasons
and trees). A busy machine can starve the writer thread: once, with four
probe runs and another agent's builds at the same time, a probe recording
lost 57 ticks to a `system.gap`. Probe recordings now wait for the writer
instead, so that cannot recur there; live flight keeps dropping rather than
stalling, which only a machine far busier than the game itself would cause.

### Whose flight it is

A recording is made for one seat, and the plane that seat flies is the
recording's player. Single player is seat 0 flying plane 0, the lead of
Friendly Wing 1, and its recordings are unchanged. For any other seat the
recorder does the same things with that plane's id:

- The player's pose, flight state and controls are the seat's plane's, and the
  roster registers it as `You`, a human pilot.
- The pilot's commands, the view target, the seeker tone, the stall warning,
  the device sounds, the ejection sounds and the weapon release sounds are that
  plane's, and so is the telemetry sampled 30 times a second and the reasons
  that only the player has (the G-limit hit that broke the structure, the
  engine switched off by the pilot, the ejection handle).
- Every other aircraft, including every other human-flown plane, is recorded
  the way an AI aircraft is: registered by the roster (a human's entry says so,
  and is labelled `Seat 2` and so on), with its state, hits, departures and
  telemetry sampled 5 times a second. Its controls read as idle, because only
  the app's own seat sends input until the network stages.
- A radio or tower line that was addressed only to other seats is left out. A
  line queued for several seats stays, and each seat's own delivery is
  recorded once. Which seats heard a line is not stored: the recording is one
  seat's, so the field would say the same thing on every entry.
- The header names the player with `draw.player` when it is not plane 0, so
  the viewer's reconstruction shows the recorded seat's plane as the player.
  A recording of plane 0 has no such entry and is byte for byte what it was.

The viewer, the sound, the tracks, the panels and the Replays screen's details
follow `draw.player` too (John, 2026-10-05; slice F2-O2): the first aircraft
watched, the weather's player, the target the views follow, the listener's
cockpit, the telemetry and thinking panels' `You` and the details' aircraft
are that plane's. A recording with no `draw.player` is plane 0's, as it was.
A recording can also have no player at all: `draw.player` is then `4294967295`
(the observer screen's live recording, below), no aircraft is drawn as the
player's ownship, and the first aircraft is the one watched.

### What is recorded now

| Family | Events |
| --- | --- |
| Weapons | `weapon.launch` with range, aspect, off-boresight angle, closure, heights and speeds at release (a gun round or rocket, which has no target of its own, is aimed at the shooter's current target: the AI's, or the player's designated one; a bomb, rocket or air-to-ground missile also says `surface` and, for a ground target, the range to it, and a HARM or Maverick says `boresight` or `cued` in `mode`); `weapon.seeker_active`, `weapon.pitbull`; `weapon.track_lost` once per shot, with the range and why (decoyed, the seeker lost it, the target is gone, or it could not hold the target); `weapon.decoyed` for each missile that followed chaff or a flare, an AI aircraft's or the player's own, with the roll, its threshold and `roll_bound` (100 for legacy/IR rolls or 10000 for Energy chaff; absent older fields mean 100), and the missile's susceptibility and the device's effectiveness behind it, and the `number` of the chaff cartridge or flare that fooled it, the same number as its `combat.countermeasure` entry (a roll that failed shows in the missile's guidance tree; recordings made before 2026-10-10 have no number); `weapon.outcome` (hit, missed, spoofed, jammed) for every shot the debrief ledger closes, with why (the decoy and its roll, the jammer, or a track lost earlier) and, for a guided miss, how close it came; one per shot, except that a missile recorded as spoofed which flies on and damages an aircraft gets a second outcome, a hit with `replaces` spoofed, and the later one stands, as it does in the debrief; that hit's reason names the device, as in `it was decoyed by chaff #7 from Enemy 2-1 at 0:13.5 but flew on and struck anyway` (`#7` is the device's number; a recording without numbers says `chaff` alone) |
| Combat | `combat.hit` from each aircraft's hit points, with the attacker, damage, hit points after and the region hit (plus damaged systems for the player); `combat.destroyed` with the killer; `combat.ground_impact` once for every weapon burst on the ground, with its spot (`x_ft`, `y_ft`, `z_ft`) and, when a weapon vanished that tick, the shooter, `projectile` and `weapon` ([below](#ground-impact-events)); `combat.countermeasure` for every chaff cartridge and flare that leaves an aircraft, the player's and the AI's, with how many of that kind it has left and the aircraft's exact position, velocity and attitude as it left, which the viewer flies the device again from ([below](#chaff-and-flares)); `combat.countermeasures_cleared` when a range reset removes them all |
| Aircraft | `aircraft.crashed` (flying into the ground or a structure, or a destroyed aircraft's wreck coming down or exploding), `aircraft.ejected` with the hazard an AI pilot left for or your own ejection, `aircraft.pilot_killed`, `aircraft.took_off`, `aircraft.landed`, `aircraft.flameout`, `aircraft.fuel_out` |
| Flight | `flight.departure` (mode changes), `flight.stall` and `flight.spin` on and off; `flight.effect` when a flight-model effect starts or stops, with what it applied and why ([below](#flight-model-effects)); `flight.g_limit` when the stick reaches its stop, with the G the envelope offers, the limit applied, the G delivered and what set the limit; `flight.structural_failure` with the section, the G and why |
| AI | `ai.activity` (with how long the old activity lasted), `ai.target` (with priority and score), `ai.weapon_phase` (with the store and the weapon service's words), `ai.airfield_phase`, each with its reason ([below](#reasons-for-ai-decisions)); `ai.defense` when missile defense starts, changes maneuver, releases chaff or flares, or ends; also when anonymous gun/hit evidence starts, changes kind or clears, identifying which threat has motion priority, and when a launch warning arrives or is dropped; `ai.fallback` the first time each aircraft uses each fitted stand-in rule; `ai.strike` when a striker with laser stores and no laser support decides what to do with them ([below](#strike-decisions)); `ai.ejection` when the ejection check finds a hazard, the pilot ejects, a go-around replaces an ejection, or the hazard passes |
| Communication | Every entry of the [communication journal](#communication-journal) and of the AI message journal, with trigger, rolls, outcome and reason ([below](#communication-events)); and `comms.hud` for every cockpit message line: shown, a repeat that moved the line on screen to the bottom with a fresh timer, or pushed off the screen by newer lines |
| Data link | `datalink.member`, `datalink.lock`, `datalink.unlock`, `datalink.assign`, `datalink.clear`, `datalink.acknowledge` and `datalink.sort_warning`: what changed in each flight's shared picture, from the data link's journal ([below](#data-link-events)) |
| Display | Format 4 records human `aircraft.hud` and `aircraft.sight` at 24 Hz, including ground targets, live impacts and A-G contacts. Older sparse or absent data keeps its fallback ([HUD and sight events](#hud-and-sight-events)). |
| Air to ground | The bombs, rockets and guided weapons of the [air-to-ground round](spec/air-to-ground.md), in [their own section](#air-to-ground-events): `weapon.release` for each trigger call that lets go of bombs or rockets (a pack is one entry listing its rounds in order), `weapon.dud`, `weapon.cluster`, `weapon.guided` for a laser or GPS weapon's release and each change of guidance, `player.laser` and `player.designate`, and, so the viewer can [draw them](#laser-and-bomb-marks), `player.laser_spot` and `weapon.aim` (where a spot or a weapon's aim point is, at most ten times a second) and `weapon.impact` (where a bomb landed and how far from its aim). A bomb's fall is the projectile's ordinary track, one pose a tick |
| Surface units (format 3) | The surface world of a [ground target mission](spec/surface-defenses.md): `surface.burst` for each gun burst or missile salvo of a unit, `surface.phase` for the engagement it is in (both name the target as the object, and carry `surface` true when it is a ground unit or ship of the other side: surface units fight each other since 2026-10-11, and the unit fired at joins the registry), `surface.rearm` and `surface.refill` when a supply truck rearms a launcher or gives a gun a magazine (with what it holds after), `surface.radar` when a radar comes on, goes off or is shut down by an anti-radiation missile, and `surface.wreck` with the unit's fire (its width, sized by its hit points). The rounds and missiles are ordinary `weapon.launch` and `weapon.outcome` events owned by the unit's id, and an aircraft's loss to one says `it was shot down by an SA-6` |
| Audio | `audio.effect` (impacts, explosions, and each chaff cartridge's and flare's release sound, marked `own` when the player's own aircraft released it), `audio.release` (weapon release sounds), `audio.tone` (the seeker tone, its loudness and whether its weapon aims at the surface), `audio.stall_warning`, `audio.ejection` (warnings, seat, parachute, a wingman ejecting), `audio.device` (gear, flaps, hook, brake) and `audio.music` (every input of the situation music, the score they ask for, and why) |
| Player and system | `player.command` (combat commands and trigger releases), `player.view_target` (the target your target views follow, from the first frame and again whenever it changes, including when the sensors drop it and the views hold it by sight within visual range, and when it goes to none), `player.bookmark`, `system.pause`, `system.resume`, `system.time_scale`, `system.cheat`, `system.restart` (first in a recording that follows a restart), `system.end`, `system.gap`, and `system.note` when a tick held more than the format stores or a journal overflowed |
| Display trees | `ai.thought` for every AI aircraft, `flight.telemetry` for every aircraft that flies, `weapon.guidance` for every guided missile ([below](#display-trees)) |

Every second a frame carries a checksum of all aircraft's exact state,
which `--recording-diff` uses. Every gun round is its own launch and
outcome, as the debrief counts them, so the summary's shot table lists a
burst round by round.

The attacker on `combat.hit` comes from the debrief ledger's last shooter
(`Ledger::attacker`: that includes a shooter whose blast only reached a
bystander aircraft, who is named but credited with nothing),
and its projectile from the same tick's shot outcomes; two hits on one
aircraft in one tick can be credited to the later shooter. A headless probe
runs no weather, crew voice, music or cockpit messages, and its header says
so. What the records themselves cannot explain is listed with the
[AI thinking record](#ai-thinking-record), the
[telemetry record](FLIGHT-MODEL.md#telemetry-record) and the
[communication journal](#not-visible-yet).

### Ground impact events

`combat.ground_impact` is written once for every weapon burst on the ground:
a bomb, rocket, missile or gun round that lands, whatever explosion it shows.
It holds the spot (`x_ft`, `y_ft`, `z_ft`) and, from the weapon that vanished
nearest to the burst on that tick, the shooter (subject), `projectile` and
`weapon`; a burst with no weapon behind it (nothing vanished that tick) keeps
only the spot. Ground bursts have been recorded as a blast on the ground since
explosion types, and until 2026-10-11 the recorder looked only for the older
plain ground effect, so the event was almost never written. That is a fix to
what is written, not a format change: no new event, field or version, so
every recording still opens, and one made before the fix simply holds few or
none of these events. The text export reads it as "shot N from X hit the
ground". The event is not a hit on an aircraft or a ground object (those are
`combat.hit` and the `weapon.outcome` result).

### Chaff and flares

Combat notes every chaff cartridge and flare as it leaves an aircraft, the
player's own and the AI's, and the player's own decoy rolls, in two
write-only lists the recorder drains (`State::take_device_notes`,
`State::take_decoy_rolls`); the AI's rolls come from the bridge as before.
Each release becomes one `combat.countermeasure` entry holding the
releasing aircraft, the kind, its number among the flight's releases (which
chooses the device's look: its flicker, sideways throw, smoke and strips)
and the aircraft's position, velocity and right, up and forward vectors
exactly as they were. The player's releases between ticks land on the tick
on screen, like the key press that made them; the AI's on the tick whose AI
step released them. How many of that kind the aircraft has left comes from
combat for the player and from the AI's dispensers, counted back over its
releases in the same tick, for AI aircraft. A range reset that clears every
device is its own entry. The golden fingerprint
`combat/player-countermeasures` was taken before these lists existed, and
they leave it unchanged.

Each entry also says after which combat tick's step the device left, and
the viewer ([below](#what-is-drawn)) flies every device again from it, one
tick at a time over the same ground, so its flight, flicker, smoke and
strips are the ones combat drew. Every aircraft's frame also says whether
its afterburner flame is lit (the `flame` flag), which lights the scene
around it; unlike the `afterburner` flag, which is what its nozzle draws,
it follows the flight model for AI aircraft too. `--verify-render` checks
both: the flag with each aircraft, and every flare, smoke puff and chaff
cloud against the devices combat flew, bit for bit. Releases lost to a
`system.gap` are missing from a replay.

### Display trees

The AI tree includes a separate ground-attack section with its phase, segment,
pass, target object id, selected surface weapon and release gate. A planned
attack phase does not mean a weapon fired; launch events establish releases.


A display tree is what a debug panel shows, as numbered lines: a label, a
value with its unit, and a note that says why. The recorder builds three
kinds from the simulation's write-only records, and the debug panels will
build the same trees from a live flight. The builders are pure functions in
`tore-app/src/replay/trees.rs`; the layouts and rates are agent decisions
(2026-09-26).

| Channel | Subject | Recorded | And at once when |
| --- | --- | --- | --- |
| `ai.thought` | Each AI aircraft | 10 times a second while alive | Its activity, target, weapon phase, motion branch or maneuver changes |
| `flight.telemetry` | You, and each AI aircraft | You 30 times a second, AI 5 times, while alive | An effect starts or stops |
| `weapon.guidance` | Each guided missile | 10 times a second, and at launch | A decoy roll is made against it |

Each aircraft and missile keeps its own phase within the rate, so the work
spreads over the ticks. Numbers are rounded to what a panel shows (tenths of
a nautical mile, whole degrees and knots, tens of feet for heights and
pounds for thrust and drag); notes use clock times rather than running
counts, so an unchanged line costs nothing in the file. Measurements (angle
of attack, sideslip, Mach, dynamic pressure, height above ground) are
labelled as measurements and never given as a cause.

The **AI thinking** tree, top level first. Lines appear only when the
record has something to say; the controller's lines appear only when its
decision ran this tick, otherwise "Motion" says why not (an airfield
sequence, destroyed, a training target).

| Line | Value | Note and children |
| --- | --- | --- |
| Aircraft | Label | Aircraft, skill and where it came from, side, wing and place |
| Lookout | Current scan sector | Forward/right/rear/left/above/below; bounded visual candidates with range and rejection reason; current measured visual/radar/infrared sources are separate from target permission |
| Incoming fire | Evidence | Observed tracer, close pass or weapon hit, its observation time, selected response and whether missile defense takes priority; no shooter identity is invented |
| Mission | Role | Stance; holding formation, must rejoin; each remembered attack, acted on or ignored and why |
| Activity | Activity (`Activity`) | Why it last changed; `Since` the clock time it began |
| Target | Aircraft id (`Target`) | Its name; `Chosen by` (mission ranking, kept, nearest first, weapons held) and why; `Lost`; `Score` with its priority; `Runner-up` with its score, name and priority; the ranking's rules; who attacked it or its charge; `Not eligible`, up to three observed aircraft that could not be chosen and why |
| Geometry | Range, nm | `Off nose` (ahead or behind, facing or not), `Aspect`, `Closure`, `Height` above or below |
| Weapon | The chosen store and station | Target class; `Phase` (and the one before); `Fire?` with the first reason it did not fire and its numbers, for example "outside max range (6.1 nm > 5.8 nm)"; `Service`, the weapon service's words with its next deadline as a clock time; each store against this target, grouped, with its verdict and failing checks |
| Damage response | Reason for recovery | Known runway or home-position fallback, ground hold, protective power or restart request, flight-safety exception, mechanical throttle lock and requested maneuver limits. |
| Gunnery | Round requested or waiting | Fixed-barrel alignment, predicted miss before dispersion, flight time, ammunition before release, burst/recovery phase and deadline. Uses permitted target motion and the shared projectile model. |
| Defense | The missile | Its weapon and shooter; `Range`, `Maneuver` (drag, beam, notch, break, jink, complete after confirmed opening range, or not yet, with heading and pitch), `Countermeasures` (requested devices), `Time to impact`, `Maneuver time`, `Break threshold`, `Why now` |
| Motion | Branch (tactics, gun tracking, formation, missile defense, rejoining, search, ordered) | What the branch is doing; `Slot distance` and `Formation speed` in formation; `Choice` with every draw of the choice (for example "roll 37 < 50: best attack"), `Chosen at`, `Path`, `Situation` with the quadrant's percentages, any fitted rule, last-ditch candidate, engagement pitch and pursuit offsets; `Next choice`; `Rolls` made this tick outside a choice |
| Steering | | `Heading`, `Pitch`, `Bank` and `Speed` delivered, each with `Asked`, what the maneuver asked for; `G` delivered with its `Limit`; `Terrain floor` |
| Controls | | Pitch, roll, rudder, throttle, afterburner |
| Fuel | Pounds | State; `Endurance`, `Time home`, `Heading home` |
| Airfield | Phase | The gates (turn, runway, wing landed, leader landing); parking slot, a landing begun, why it left the sequence, a go-around |
| Ejection | Watching, ejected or go-around | The hazard and seconds to impact; the phase that guarded it; whether it was a catastrophe |
| Fitted rules | Names | Every stand-in rule used this tick |
| Recent | Count | The last four decision changes, each labelled with its clock time, with what changed and why |

The **telemetry** tree: `Aircraft` (label; aircraft and which flight-model
path moved it), `Altitude` above sea level, `AGL` (measured), `Air data`
(measurements: `TAS`, `Mach`, `AoA`, `Sideslip`, `q`), `Load` (`G`
delivered, `Asked`, `G limit` with what set it), `Rates` (roll, pitch, yaw),
`Thrust` (with `Lapse`, `Power available`, `Throttle`, `Afterburner`),
`Drag` (airframe, fuel and stores, pull, gear, flaps, airbrake, slip, and
damage when there is any), `Stall speed` (with `Authority`), `Fuel`,
`Contact` on the ground, and `Effects applied`, one line per effect the
step applied: its label, the factor, limit or state it applied, and a note
saying why ([effects](#flight-model-effects)). The labels `TAS`, `Mach`,
`AoA`, `Sideslip`, `AGL`, `G` and `G limit` are the ones the exports read.

The **guidance** tree: `Weapon` (name; guidance kind), `Shooter`, `Target`,
`Mode` (cued or boresight), `Seeker` (status; acquired, searching or not yet
enabled) with `Quality` and `Tracking`, `Time of flight`, `Speed`,
`Range to target`, `Closest approach` so far and when, `Intercept in`, and
`Decoy roll`, the latest roll against it and whether it was decoyed or
resisted.

### Reasons for AI decisions

Each decision change is recorded on the tick it happened, with a thought
tree, and with a sentence built from the records: for an activity, the
missile defense and its reasons, the tactic chosen, the store firing at
whom, why there is no firing solution yet, holding formation until
released, the search, the rejoin, the fuel state, or the airfield sequence;
for a target, the mission ranking with priority, score and the runner-up,
the nearest-first ranking, a target that left, or weapons held; for a
weapon phase, the weapon service's words. When the aircraft heard something
from another aircraft on the same tick (an attack report, a leader's
release to free selection, a wing order, a launch warning), the reason ends
with "after" and what it heard. Spacing and wing-control orders move the
slot, not the decision, so they are not cited.

### Strike decisions

`ai.strike` (slice ST-G, 2026-10-12) records one decision of an AI striker
that is not a change of activity, target or weapon phase: an aircraft that
carries laser stores and nothing else it can drop, has no designator of its
own or in its wing and finds no spot on its target, which says "No laze
support" and either drops its laser bomb by hand as an unguided bomb or goes
home ([no laze support](spec/ai-strike.md#no-laze-support)). Subject: the
aircraft. Object: the target it would attack. Fields: `decision` (the words
on the radio, `no laze support, dropping manual` or `no laze support, RTB`),
`reason` (a fixed key: `no_laze_manual` when the laser bomb falls unguided,
`no_laze_rtb` when no laser store can), `station` and `weapon` (the store).
One event for each decision: an aircraft that drops its bombs by hand and then
has only a laser missile left makes a second one, `no_laze_rtb`. The same
decision is a radio call (`comms.radio`, text only) at the same time. The
text export reads `Friendly 1-2 strike decision: no laze support, dropping
manual with GBU-10E on Enemy 2-1 [no_laze_manual]`, the summary counts the
reasons for each aircraft (`AI strike decisions: no_laze_manual x1`), Tacview
carries the event with the other `ai.` debug events, and the viewer's timeline
marks it in the colour of orders. The driver (`--pilot ai`) cannot record yet
(slice ST-RA); its `NOLAZE` line shows the same decision.

### Flight-model effects

`flight.effect` turns each effect of the [telemetry
record](FLIGHT-MODEL.md#telemetry-record) into an "on" event when it starts
and an "off" event when it stops, compared by `std::mem::discriminant` (and
the device, for held devices). An effect that is gone for less than a
quarter of a second (30 ticks) and comes back is the same episode, so
flicker makes no events. Effects of one moment (a touchdown, a lift-off, a
blast kick, the spin direction rule, a spin's end, an autopilot release, a
building rebound) have an "on" event marked `momentary` and no "off". Every
aircraft's first recorded tick lists the effects already in force. An AI
aircraft's blast kick lands before its own flight step, so its record shows
the rotation it caused (`Blast jolt`), not the kick.

### Communication events

| Journal entry | Event |
| --- | --- |
| Radio calls, wingman replies, AI chatter | `comms.radio` |
| Crew remarks and coaching checks | `comms.crew` |
| Tower lines and replies, and tower speech cut when the runway under your landing clearance is destroyed | `comms.tower`; the cut is `cancelled` because the runway is unusable |
| AI text lines not shown yet (queued, held, replaced) | `comms.hud`; a line the HUD showed is recorded once, by the HUD |
| Player orders | `comms.order`, then one `comms.delivery` per addressed wingman with its answer (applied, rejected or skipped) and why; an order that went out on the radio has the `radio` route, one the game refused before it went out has none |
| Music inputs | `audio.music` with `from`, `to`, why, and every input (`succeeded`, `ejected`, `launching`, `air_target`, `hit_recently`, `danger`, `home`, `deck`) as true or false |
| Attack evidence (AI message journal) | `comms.report` when queued, then one `comms.delivery` per recipient: delivered, ignored with why, or expired after 2 s without news |
| A neutral leader releasing itself to free selection | `comms.order` with the attack that triggered it |
| Wing orders between AI aircraft | `comms.request`, answered by one `comms.delivery` per recipient when there are several |
| Launch warnings | `ai.defense` when they arrive (the reaction and devices scheduled) or are dropped (and why) |

Each carries its `trigger`, the `rolls` it used, its `outcome` and
`reason`, how long it waited, whether you heard it, its audience, its
`route` (`radio`, `tower` or `direct`: how the cockpit plays it), and a
`message` number shared by every entry about the same line or message.
Radio call numbers are the channel's own; AI messages count from 2^32. An
escort's changed priority is not an event; it shows in the escort's thought
tree.

A line has one entry for each thing that happened to it, so it is
delivered, and heard, exactly once: the entry whose `outcome` is
`delivered` (`tore_replay::vocab::heard` says which). Its queued entry
comes before, and an `interrupted` one after, when your wing order voice
cut it off. The replay's sound, subtitles and Tacview messages use only
that entry. Two tower triggers are fixed names a replay acts on
(`tore_replay::vocab::trigger`): `player request` on the tower's answer to
your own request, and `landing clearance cancelled` on the cut; other
triggers are plain English.

The seeker tone (`audio.tone`) is recorded when it starts, stops or
changes, and again when its loudness (`strength`, 0 to 1, as the mixer
takes it) moves by 0.05 or more from the last entry, so a tone that swells
with the seeker's signal makes a few entries rather than one a tick. Its
name comes from `tore_replay::vocab::tone`, and `surface` says whether the
weapon aims at surface targets, which changes the sound of an infrared
lock. The music's inputs are journaled only when the flight has sound; a
headless probe has none.

### Data link events

The [flight data link](DATALINK.md) keeps a write-only journal of what changes
in each flight's picture, the way the communication journal does: bounded at
1,024 entries between drains, and it draws no random number, so the picture
is the same whether or not a recording drains it. The recorder drains it every
tick, after the communication journal, and writes one event for each entry.
The layout is an agent decision (slice G9, 2026-10-05); the names are in
`tore_replay::vocab`.

| Event | Who | What |
| --- | --- | --- |
| `datalink.member` | the plane | A plane joined the picture, the first tick the link saw it, with `radar` (true or false: its aircraft has a radar, so its scope marks the picture). Every plane in play has one, so a recording that has data link events lists each plane once near its start. |
| `datalink.lock` | the member, then the target as object | A member took a radar lock. |
| `datalink.unlock` | the member, then the target | A member let go of a lock: the target or the member is gone, or the lock was released. |
| `datalink.assign` | the lead, then the wingman as object | A lead gave a wingman a target: `target` (the aircraft) and `order` (the order's name, such as `EngageMyTarget`, as `comms.order` names it). |
| `datalink.clear` | the member whose assignment ended, then the target | `reason`: `order` (the lead ordered something else), `receiver lost`, `target lost` or `lead changed`. |
| `datalink.acknowledge` | the member, then the target | The wingman locked the target it was assigned, the first time only. |
| `datalink.sort_warning` | the human's plane, then the aircraft both hold | A human was told a flightmate (`other`) holds a lock on the same aircraft. The HUD line is its own `comms.hud` entry, and the beep is a cue. |

The assignment call's words are not a second line: they stay in the
communication journal's entry for the order (`comms.order`), so
`datalink.assign` and the order that gave it match by tick, the lead and the
wingman. The 4-times-a-second tracks and each
member's state are not recorded: they are what each aircraft's sensors held,
and the AI thinking record shows what an AI member did with them. A
journal that overflowed leaves a `system.note` saying how many entries were
lost, as the communication journal does.

What reads them: the debug log and the Tacview file's `Debug` events carry
every one in words; the summary has a **Data link** section (below); the
Comms panel lists them under the **Link** chip, except `datalink.member`, which
only the exports count; and `--recording-diff` compares them as a category.
A recording from before them has none and reads as it always did.

### Air-to-ground events

The air-to-ground round (slice RA, 2026-10-11) adds events and one weapon
class; the format stays 3 for a recording with surface content and 2 for one
without (decision 19 of the plan: a replay draws no HUD, so no new per-tick
track is needed). Everything is derived from state the tick already computed
and draws no random number, so recording changes nothing the mission does. The
names are in `tore_replay::vocab`; the layout is an agent decision.

| Event | Who | What |
| --- | --- | --- |
| `weapon.release` | the aircraft, then the ground target it had designated | One entry for each trigger call that lets go of bombs or rockets: `class` (bomb or rocket), `weapon`, `station`, `count` (the rounds of this call), `projectile` (the first) and `projectiles` (all of them in pack order, so a round's place in the list is its pack index), `delivery` (`ccip` or `level` for a bomb, by the sight in use; `rocket`; `guided` for a laser or GPS bomb; `unguided` for an aircraft with no bomb sight), `left` (rounds the station holds after, for a flown aircraft), `range_ft` to the designated target and, for a rocket, `salvo`: its number in the run of rockets fired from that station with gaps under a second. Each round is also an ordinary `weapon.launch`. |
| `weapon.dud` | the shooter | A bomb that landed before its fuze armed (the [dud rule](spec/air-to-ground.md#warnings)): `projectile`, `weapon`, where it landed, `arm_s`, `flown_s` and `reason`. Its `weapon.outcome` says missed and why. |
| `weapon.cluster` | the shooter | A cluster bomb burst on the ground: `projectile`, `weapon`, where, and the burst's `explosion` type. The submunition flashes are drawn from that burst's explosion, so a replay shows what the flight showed. |
| `weapon.guided` | the shooter, then the object an AT-2 or JDAM follows | A laser weapon, JDAM or AT-2 at its release (`released` true) and at each change of mode: `mode` is `waiting` (a laser weapon released with no spot, which may lock one), `guiding` or `blind` (guidance lost for good, or never had), with `from`, `guidance` (`laser`, `coordinate` or `command`), `spot_owner` and `own` (a laser weapon on its own aircraft's spot or a buddy's), the JDAM's stored point as `x_ft`, `y_ft`, `z_ft` with `stored` true, and a `reason` in words. Event kinds of the same family as the `weapon.guidance` display tree, but a separate log of what changed. |
| `player.laser` | the lasing aircraft, then the object under the spot | An aircraft's laser designator changed state or mask: `state` (`ready`, `lasing`, `masked`, `off`), `how` (automatic or by hand), `mask` (range, gimbal or terrain), `range_ft`, and the spot's position while it lases. While the AI flies an aircraft whose human gave it back (a drop, a leave, away, a kick), the cockpit the human left keeps its laser and designation, and the entries go on for that aircraft with `kept` true (slice FX-K). When the aircraft is destroyed or leaves the mission with its laser on, one `off` entry (`reason` "its aircraft is gone") ends it. |
| `player.orbit` | the recording's player | The AC-130's sensor console (slice nr-orbit, 2026-10-13): `on` true when the orbit starts (the console opens on a pin or a track, or a point is pinned in it), with the point the orbit circles as `x_ft`, `y_ft`, `z_ft` and the planned `radius_ft` and `height_ft` above it, again whenever that point has moved more than 50 ft (at most once a second), and `on` false when it ends (the console closes, or the point is let go and the console searches; slice nr-cona, 2026-10-14). The search itself shows as the autopilot's orbit mode in the flight telemetry ([the sensor console](spec/ac130-linked-guns.md#the-sensor-console-and-the-orbit)). |
| `player.laser_spot` | the lasing aircraft, then the object under the spot | Where the spot is while the laser is on (slice FX-R, 2026-10-11): `x_ft`, `y_ft`, `z_ft`. Written when the spot has moved more than 5 ft since the last entry and at least 12 ticks (a tenth of a second) after it, so a spot on a standing target makes none beyond the `player.laser` that turned it on, and one on a moving truck makes ten a second. The viewer follows the spot with them. It also follows the spot of a kept cockpit (`kept` true), so a laser bomb that goes on guiding after its pilot dropped out shows its spot. |
| `weapon.aim` | the shooter | Where a guided weapon in the guiding mode steers (FX-R): `projectile`, `spot_owner` for a laser weapon, `x_ft`, `y_ft`, `z_ft`; written like `player.laser_spot`. The last one before a weapon goes blind is the point it was homing on then. A JDAM's stored point is in its `weapon.guided`; it makes none. |
| `weapon.impact` | the shooter, then the ground target it had designated | A bomb or guided weapon reached the ground (FX-R), one for each bomb, duds included: `projectile`, `weapon`, `x_ft`, `y_ft`, `z_ft` (where it landed), `guided`, `mode` (a guided weapon's mode at the end), `aim_x_ft`, `aim_y_ft`, `aim_z_ft` and `miss_ft` (the point it was steering for and the distance from it, when it had one), `target_ft` (distance from the designated target) and a `reason` in words (`it landed 940 ft from the point it was steering for before it went blind, 950 ft from the designated target`). For a shooter whose human gave the aircraft back, `target_ft` reads the designation the human left and a laser weapon's `reason` for going blind reads the kept laser (FX-K). |
| `player.designate` | the aircraft, then the ground object | A flown aircraft designated a ground object or pinned a ground point, or gave one up: `channel` (`a-g radar`, `infrared`, `harm page`, `sight`, `visual`, `pin` or `cleared`), `from` (the object before) and, for a pin, its position. An air target never makes an entry. The same kept cockpit and `kept` flag as `player.laser`; a kept designation keeps the channel the human made it by, and one the AI's aircraft lost with its life is cleared with `reason` "its aircraft is gone". |

```mermaid
stateDiagram-v2
    [*] --> guiding: released with a usable spot or point
    [*] --> waiting: laser weapon released with no spot
    [*] --> blind: every spot out of reach, or no point in the release zone
    waiting --> guiding: locks a spot inside its nose cone
    guiding --> blind: the spot owner's laser stops, the spot leaves the cone, or the launcher or target is lost
    blind --> [*]
    guiding --> [*]: impact
```

The words follow the weapon to its end. A guided weapon's `weapon.outcome`
says what it was following (`it was guided on Enemy 1-2's laser`, `it flew to
the point stored at its release`, `it went blind when your laser went off`),
and the loss it caused names the weapon and its guidance, in both
`surface.wreck` and `combat.destroyed`: `T-80 #2 was destroyed by You because it
was hit by a GBU-10 guided on Enemy 1-2's laser`. When the killing round was
an unguided bomb, a rocket or an air-to-ground missile, the loss still gets
its `weapon`, `projectile` and `reason` (`it was hit by an AGM-88`). Names come
from the roster, so a buddy's spot reads as the buddy's label and the player's
own as `your laser`.

Weapon classes are `bomb`, `rocket`, `missile`, `gun` and `other`. Since this
slice an unguided rocket pod (several rockets, no seeker) is a **rocket**,
where recordings from before were `other`; a guided glide bomb (the laser
bombs and the JDAMs) is a **bomb**, where it was a missile; a converted
network capture classes them the same (slice NA). The Tacview file
types them `Weapon+Rocket` and `Weapon+Bomb`, and a replay of an older
recording reads them as it recorded them.

What reads them: the viewer draws them as [laser and bomb
marks](#laser-and-bomb-marks); the debug log carries every event; the summary lists the
releases, duds, cluster bursts and guided modes among its key events; Tacview
shows all of them as `Debug` events; `--recording-diff` compares them as part
of the event categories. The battery scenario `replay-a2g` records a bomb
pack, a dud, a rocket salvo, a cluster bomb, a laser bomb on its own spot, one
that loses its laser, a JDAM and two HARM launches, replays each tick for
tick against the picture the run drew, and checks the events.

### HUD and sight events

The [HUD camera](#hud-camera) and [Sensor View](#sensor-view) read the cockpit
facts recorded for their aircraft. Format 4 records human HUD and sight state
at a steady 24 Hz, every five simulation ticks, including idle and gunfire
(opinionated, John 2026-10-21). Free fighter sights are included. Older sparse
recordings still load. Recording reads already computed state and consumes no
random numbers. The field grammar and compatibility rules live in
[replay display records](formats/replay-display.md).

| Event | Who | What |
| --- | --- | --- |
| `aircraft.hud` | Aircraft, then displayed target | NAV, arm state, selected station and weapon, rounds, launch/seeker/delivery mode, readiness, firing estimates, radar, mount, and the displayed target's position and velocity. Ground objects use the same target box and weapon range data as live flight. |
| `aircraft.sight` | Aircraft, then tracked object | Look angles, zoom, mode, aim, rounds, linked and selected guns, readiness, trains, gun lock, gimbal-limit state, live pipper impacts, target mark, and A-G radar contacts. The console shows the target name, range and bearings and the lead gun's sector outline. |

The overlay holds between samples. Camera pointing interpolates across adjacent
samples, with mode/target/zoom changes and gaps kept discrete. The HUD camera
still presents at 24 fps. Legacy samples keep the old held look and calculated
pipper. The complete behaviour is in the
[sensor replay spec](spec/target-window.md#sensor-view-in-a-replay).

The player's own aircraft, another flown aircraft and every AI aircraft are
sampled. An ownship is read from the cockpit readout the live HUD draws. An
AI aircraft has no ownship, so its sample comes from its strike cockpit and
its weapon phase (the phase words are mapped onto the seeker's labels; its
hit percent and band stay 0, and an AI AC-130 has no gun trains, so the
replay does not invent pippers). A recording converted from a network
capture samples the local seat from the readout the client already received.
The weapon's source name is not on the wire, so the conversion fills it from
the mission loadout, and the radar range step is the game's default. Other
players' HUDs are not in that readout. Recording them would be a protocol
change, which this slice does not make. The host's own recording has every
ownship.

### Stores

F-22 bay magazines use store location byte `0x83`, with the same layout and
count event fields as other stores. Main and side stores retain their drawn
release centres. Old recordings keep their recorded carrier locations; no
file-schema change is required ([carriage contract](spec/stores-on-pylons.md#f-22-naval-stores)).

Each aircraft's pylons and the stores still on them
([stores on pylons](spec/stores-on-pylons.md)) are recorded as events
(slice BP3, John 2026-10-12), so the viewer draws them emptying as they did
in flight. There is no format change: they are new entries within format 2
and 3, which an older reader skips. Recording them reads the tick's picture
and draws no random number, so it changes nothing the mission does. The
names are in `tore_replay::vocab`, the encoding in `tore_replay::stores`;
the layout is an agent decision.

| Event | When | What |
| --- | --- | --- |
| `stores.layout` | The first tick an aircraft is recorded with any stores, and whenever its drawn stations change (a respawn with another loadout, a jettisoned gun pod) | `count`, the stations that follow |
| `stores.station` | Right after its layout, one per station in order | `slot`, `hardpoint`, the record's position `x_ft`, `y_ft`, `z_ft`, `location`, `paired`, `look` (store, pod or tank), `items` (whole stores on it), `loaded`, the weapon's `shape` when it has one, and a fitted pod's or tank's `length_ft` and `diameter_ft`; since slice PY2 (2026-10-13) the model's underside over its pylons where it has one, `over_ft`, `over_inboard_ft`, `over_outboard_ft`, `over_front_ft` and `over_rear_ft` (feet up), which hang each store under its pylon or rail, and for a wingtip station its tip edge, `tip_right_ft` and `tip_up_ft`. Everything the drawing needs, so the viewer needs no loadout; a recording without the `over` fields hangs its stores at the record's height |
| `stores.items` | Whenever a whole store leaves (a bomb, a missile, a pod's last rocket), a jettison or a rearm | `items`: the whole stores on each station of the layout, in order |

The viewer reads them once when the recording opens (again as a live or
growing one grows) and draws each aircraft's stores at the tick under the
playhead from the newest entry at or before it, so a seek or reverse play
shows what forward play showed. `--verify-render` compares the stores of
every tick as well. The single-player recorder writes them from the tick's
picture; a networked flight's capture conversion writes them from the
host's Stores events ([protocol 27](formats/net-protocol.md#protocol-27-stores))
over each aircraft's loadout, and an observer's live recording from the
picture its client drew. A recording made before them has none, so its
aircraft fly without stores, as they always did. On a fighter's flight the
events cost a few hundred bytes for the whole mission.

The viewer loads shapes from both the projectile catalog and every
`stores.station.shape`, including stores nobody fires. Saved and growing
recordings use the same rule. Those meshes pass through the same size and
fin-roll conversion as live flight, then the same pylon geometry and lighting.
Pods and tanks keep their fitted bodies.

Compatibility decision (agent, nr-repst): keep the recorded mount, location,
underside heights and tip edge, including shoulder, tunnel and separate-pylon
location bytes. Do not apply today's aircraft station table to old mounts:
the recorded projectile origins would no longer meet them. New recordings
already contain the positions produced by `placed_on` with the source
hardpoint index. All recordings use the shared current rack drawing rules;
this does not promise historical geometry for rules changed after recording.
No replay format or network protocol changes are needed.

## Network flights

A networked flight is not recorded live: the game on a client does not run the
host's AI and combat, which the recorder reads. Instead each networked session
keeps a **capture** beside the replays (`2026-10-05_1540_NET_HOST.tore-capture`,
[format](formats/net-protocol.md#captures)): everything the network brought,
so the client can be run again offline. When the flight ends the game
**converts the capture into a replay** in the background, with the same name
pattern as any recording (`2026-10-05_1540_UKR_F18.tore-replay`), and it shows
in the Replays screen the next time that opens. John asked for this on
2026-09-30; the details are agent decisions and the
[design](ARCHITECTURE.md#converting-a-capture-into-a-replay) has the reasoning.

```sh
tore-app --convert-capture replays/2026-10-05_1540_NET_HOST.tore-capture
tore-app --convert-capture CAPTURE --out MINE.tore-replay
```

The command needs the import the capture was made with, since it runs the
client again. It prints the replay's path and what it made again (smoke puffs, contrail
puffs and gun rounds), and says so when the capture is cut short.

What the replay is:

- **Smooth, with hindsight.** Every other aircraft, missile, debris piece and
  ejected pilot follows a smooth curve through every update the client
  received, drawn between the two updates around each tick. What the player
  saw live had guesses ahead of the newest update and, for aircraft far away
  (sent 4 times a second, twice before slice D12), a whole interval of delay; the replay has neither.
  It never shows anything past the last update of an entity, and does not draw
  across a silence of more than two seconds, so an aircraft can appear a
  moment after the player first saw it live.
- **The own aircraft follows the host.** At each of the host's exact states
  of your plane (about once a second) the replay is exactly the host's state;
  between them it is what your game predicted, with any error it made spread
  back so nothing jumps.
- **Events come from the host.** HUD lines, the radio, crew and tower lines
  you heard (with their voices), launches, chaff and flares, explosions, hits,
  craters, crash-site fires, destroyed ground objects, wingmen ejecting and gun
  bursts, on the ticks the host gave.
- **Every plane keeps its own id.** The header's `draw.player` (and
  `net.player_plane`) names the plane you flew, and the viewer follows it as
  the recording's player: its first selection, the weather's view, the
  panels' `You` and the sounds are that plane's (agent decision, 2026-10-05:
  the conversion used to swap your plane's id with plane 0's, until the
  viewer followed `draw.player`). With plane 0 as your plane the header has
  neither key and nothing differs from before.
- **Smoke, contrails and gun rounds are made again.** The host sends none of
  them, so the game regenerates them as a live client does and writes them the
  way a single-player recording does: the smoke of missile motors, of aircraft
  at half their hit points or less and of burning crash sites, the contrails
  above each aircraft's onset altitude, and the rounds of every gun burst the
  host reported, the player's own included. The viewer shows them like any
  other recording's. A gun round's line can differ from the host's within the
  gun's cone, and a burst that began before the replay's first moment is not
  there.
- **Diagnostics.** The replay carries the network figures the client logged:
  a `net.stats` event every second (round trip, loss, snapshot loss and spread,
  input margin, interpolation delay, corrections, mismatches, bytes each way),
  a `net.event` for each join, seating, refusal and drop, the connection in
  the header (`net.server`, `net.callsign`, `net.build`, `net.capture`,
  `net.flight`) and the flight's means in the footer (`net.rtt_ms_mean`,
  `net.loss_percent_mean` and the rest). The Replays screen shows the server
  and the mean round trip and loss on a **Network** row, and the debug log and
  summary list the events.

What is not in it, known limits:

- No full cockpit readout, radar or HUD, and no AI thinking trees, reasons or
  state checksums (the client does not have the host's AI). The limited
  view-target and viewed-AI facts retained since 2026-10-15 are documented
  with the [object banner](#selected-object-banner).
- Other aircraft carry no fuel, G or stick (they read 0 lb, 1 G and idle),
  and their airspeed is the speed device or the ground speed.
- A weapon's name and shape come from the host; its class is the mission's own
  weapon record when the mission knows it.
- The first moments before the first update are not in the replay; it starts at
  the first host tick a snapshot showed after you were seated.
- The `datalink.*` events are only those about the player's own flight: the
  host sends a seat the wire's `Link` events for its flight's members
  ([protocol](formats/net-protocol.md#data-link-stage-g)), and the conversion
  writes each with the function the single-player recorder calls too (built,
  slice G7: `tore_session::client::convert::datalink_event`). A player
  seated after the start has no `datalink.member` events, and the tracks are
  not recorded, as in single player.

A capture that was cut short (the game crashed or was closed) converts up to
its last whole record; the footer says `end=cut` and the byte it stopped at.
Converting a capture twice gives the same bytes.

## File format

### Layout

A recording is one file. While it is being written it ends in `.partial`;
finishing renames it to its final name. All integers are little endian.

| Part | Contents |
| --- | --- |
| Prelude, 12 bytes | `TOREREPL`, the format version (2 or 3, see [Versions and damage](#versions-and-damage)), two reserved bytes |
| Header chunk | The text header: `key=value` lines in UTF-8 |
| Data chunks | One to two seconds of frames each (120 by default) |
| Footer chunk | End tick, the writer's totals and the mission result |
| Index chunk | Where every data chunk starts, for seeking |
| Trailer, 16 bytes | Where the index chunk starts, then `TORE-IDX` |

Every chunk starts with a 32-byte header: the marker `TORC`, the chunk kind,
the body length, the frame count, the first tick, and an **FNV-1a 64**
checksum over the header and body. FNV-1a was chosen because it is fast,
needs no dependency and catches accidental damage; it is not a security
measure.

A data chunk's body is a list of **sections**, each written as an id, a
length and the bytes, so a reader skips sections it does not know:

| Section | Contents |
| --- | --- |
| Strings | Strings first used in this chunk |
| Entities | Aircraft and weapons registered in this chunk, and from format 3 the surface units |
| Frames | Aircraft, projectiles, debris, ejected pilots and surface damage, tick by tick; from format 3 also the surface units' poses, launcher and magazine changes and debris pieces |
| Spawns | Effects and smoke or contrail puffs released, tick by tick |
| Events | Everything that happened, tick by tick |
| Trees | Display tree samples (AI thinking, telemetry, missile guidance) |
| Checksums | The once-per-second state checksums |
| Rotors | Each rotorcraft's rotor speed, a share of its governed 100 percent: an entry names a frame and the aircraft whose speed changed on it. The chunk's first value of an aircraft is exact, later changes are quantized to a thousandth. Aircraft without a rotor read zero, and files from before it have no such section. Playback turns the drawn blades by this speed summed frame by frame from the recording's start, so a tick shows the same blade angle however the playhead reached it ([rotor presentation](spec/rotor-presentation.md#rotor-speed-and-blade-angle)) |
| Disk tilt | Each rotorcraft's rotor disk tilts from its shaft, in radians: longitudinal then lateral for the first rotor, then for the second (the CH-47's rear rotor, the V-22's right proprotor). An entry names a frame and the aircraft whose tilt changed on it. The chunk's first nonzero value of an aircraft is exact, later changes are quantized to 1/256 rad (the network entity record's step) and stored as absolute step counts, so nothing accumulates. Aircraft without a rotor read level, and files from before it have no such section, so their rotorcraft are drawn with level disks. Playback draws the disk tilt as recorded and blends it between ticks as live flight does |
| City | The city fill's background buildings knocked down on a frame (nr-fx3, 2026-10-13): an entry names a frame, whether every building stood again first (a restarted flight), and where each building that fell stands, its base centre in whole feet east and north, the first as it is and each later one as its step from the one before. Only changes are stored. Files from before it have no such section and replay with the city whole ([below](#versions-and-damage)) |

Spawns, events and trees sit in their own sections so the viewer can
rebuild smoke, and a reader can list every event, without decoding the
aircraft.

### Keyframes and changes

The first frame of every chunk is a **keyframe**: every value is stored
exactly, all 64 bits. Later frames store only changes, **predicted from the
values the reader will reconstruct**, so rounding never builds up over a
long flight. Smooth quantities (positions, angles, velocities, airspeed) are
predicted to keep moving as they did on the last tick; controls and devices
are predicted to stay where they were. Only the difference from the
prediction is written, as small whole numbers, and groups of values that did
not change cost nothing. An aircraft or projectile that appears in the
middle of a chunk also starts with an exact record.

Aircraft positions start from their velocity, and projectile positions from
their direction and speed, so even the first changed frame costs little.
Angles are compared the short way round, so turning through north or
rolling through inverted is a small step.

A value that is not a finite number (or is absurdly large) cannot be
predicted. That aircraft or projectile is stored exactly on that tick and
the next, so the log shows the bad value bit for bit.

Strings are stored once. Each is defined in the chunk where it is first
used, and referenced by number afterwards. Aircraft and weapon identities
(for example `F18.PT`, shown as `F/A-18D`, never aliased) are registered
once in the same way.

### Units and axes

Feet, feet per second, radians, pounds and ticks of 1/120 second. X is east,
Y is height above mean sea level, Z is north. Attitude is yaw, pitch and
bank: yaw 0 faces north and grows clockwise, pitch is positive nose up, bank
is positive right wing down.

### What each tick stores

| Item | Values |
| --- | --- |
| Aircraft | Position, attitude, velocity, airspeed, G, the 22 animated devices (including actual nozzle pitch/yaw, nacelle conversion, collective, gun mount poses and linked gun membership), engine heat, flags (engine, afterburner, airborne, on the ground, crashed, wreck gone, alive, ejected, and animated: whether anything moves the devices, since straight-flight fixtures keep the model's neutral pose), wreck phase, fuel (internal and external tanks, see [Versions and damage](#versions-and-damage)), pilot controls, the auxiliary body rates that thrust-vectoring paddles and plumes follow, hit points, regional damage and the failed structural section |
| Projectiles | Owner, weapon, target, position, previous position, direction, speed, tracer, inbound on the player, age, and the seeker's state |
| Debris and ejected pilots | Position and attitude or heading |
| Effects and puffs | Only those released this tick; the viewer ages them itself. A hit, kill or ground strike stores its [explosion type](spec/explosions.md) in its effect code (64 to 87 hit, 96 to 119 kill, 128 to 151 ground, the type less 15 added); a crater is 160 plus its size and lasts for good; a crash-site fire is code 7; the crash-site column is puff kind 3, stored in its own byte; the viewer drifts every puff with the recorded wind, and each column puff also by the cone its release point gives it; replayed flare smoke drifts with the wind as in flight. Older builds read these codes as unknown kinds and skip them; recordings made before explosion types draw and sound their plain hit, kill or ground effect. An effect not drawn at its type's size keeps its scale in the effect scales section (id 10, [below](#versions-and-damage)) |
| Surface objects | Hit points, when they change |
| Surface units (format 3) | Each unit that follows a route: position and attitude every tick, and whether it is a wreck. A launcher's rails (every rail of the unit, when any changes) and a gun's spare magazines, when they change. The piece a parked aircraft's debris is. Standing units are in the rebuilt scenery and cost nothing |
| Events and display trees | See [the vocabulary](../crates/tore-replay/src/vocab.rs) |
| Checksum | Once per second: a hash of every aircraft's exact state |

### Precision

Quantized values are never further from the truth than half a step. A
10-minute test flight with smooth and violent maneuvers measured exactly
these bounds on every tick, with no drift.

| Quantity | Step | Largest error |
| --- | --- | --- |
| Positions of aircraft, projectiles, debris, pilots, effects and puffs | 1/32 ft | 1/64 ft (about 5 mm) |
| Attitude, headings | 2^-20 of a turn (0.00034 deg) | 0.00017 deg |
| Projectile direction | 2^-20 of a turn per angle | under 0.0005 deg |
| Velocity | 1/64 ft/s | 1/128 ft/s |
| Airspeed, speed device, projectile speed | 1/64 ft/s | 1/128 ft/s |
| G | 1/1024 G | 1/2048 G |
| Devices from 0 to 1, engine heat | 1/255 | 1/510, kept within 0 to 1 |
| Elevator, aileron and rudder | 1/127 | 1/254, kept within -1 to 1 |
| Pilot controls | 1/1024 | 1/2048 |
| Auxiliary body rates | 1/4096 rad/s | 1/8192 rad/s |
| Rotor speed (a share of the governed 100 percent, in its own section) | 1/1000 | 1/2000 |
| Rotor disk tilt (in its own section) | 1/256 rad | 1/512 rad |
| Fuel | 1/16 lb | 1/32 lb |
| Ids, flags, hit points, regional damage, wreck phase, seeker, events, trees, checksums | exact | none |

Keyframes are exact, so a value that has not changed since the chunk began
reads back bit for bit.

### Size

On a synthetic 60-second flight with 17 aircraft, contrails, AI thinking 10
times a second, player telemetry 30 times a second and thrust vectoring on
the violent flights, the file took 13.5 bytes per aircraft per tick (11.7
bytes for the aircraft alone). That is about **16.5 MB for a 10-minute
mission with 17 aircraft**. Real AI control inputs are noisier than the
test's, so expect somewhat more.

The headless AI probes, three minutes each with every reason event and
display tree, measured on 2026-09-26: 5 aircraft take 320 to 500 KB a
minute, and 7 to 9 aircraft 630 to 760 KB a minute, so a 10-minute mission
with 9 aircraft is about 7.6 MB. The reasons and trees add 100 to 220 KB a
minute for 5 aircraft and 260 to 380 KB for 7 to 9, most of it each tree's
full copy at the start of every one-second chunk.

### Limits

The writer refuses input beyond these limits with a clear message and writes
nothing for that frame; the recording carries on. The reader refuses files
beyond them.

| Limit | Value |
| --- | --- |
| Aircraft per frame | 64 |
| Projectiles per frame | 8,192 (1,024 before 2026-10-09): above the simulation's 5,000 in flight, so no round is cut; older builds refuse a frame with more than 1,024 |
| Debris pieces per frame | 256 |
| Ejected pilots per frame | 64 |
| New effects per frame | 64 |
| New puffs per frame | 4,096 |
| Surface changes per frame | 4,096 |
| Surface unit poses per frame, launcher and magazine changes per frame (format 3) | 4,096 each |
| Registered surface units (format 3) | 8,192 |
| Events per frame, fields per event | 1,024, 64 |
| Ids in one value | 1,024 |
| Display trees per frame | 256 |
| Nodes per tree, depth | 4,096, 32 levels |
| Strings in the table | 65,536 (later strings are stored inline) |
| Bytes per string | 1,024 |
| Chunk body | 16 MiB (a frame too large for one chunk is refused) |
| Frames per chunk | 240 |
| Registered aircraft, registered weapons | 4,096 each |
| Header | 64 KiB, 256 extra entries |
| Events in one file | 4,000,000 |
| File | 1 GiB (the writer stops accepting frames about 20 MiB short of it, keeping room to finish) |

### Versions and damage

- **Live recordings use format 4** for the fixed 24 Hz cockpit samples
  ([display format](formats/replay-display.md)). The reader also accepts
  formats 1, 2 and 3. Legacy writers still select format 3 for surface content
  and format 2 otherwise. Format 4 retains format 3's world and track grammar.
- Format 3 adds, for the surface-defenses round (slice RP1):
  - **The ground target** in the header, `world.ground_target=` followed by
    `stem,aaa,sam,seed,nationality,night_stealth,jitter,relocate,separation_nm`
    (for example `QUCOL,3,3,1,10,0,1,1,5`; the flags are 0 or 1). It is
    everything the world needs to resolve the same surface again: template,
    defense levels, seed, the enemy's nationality, the night rule, whether
    positions jitter and the template relocates, and the enemy distance that
    sets the starts. A tenth value `1` marks the [Quick Mission front
    line](spec/surface-defenses.md#the-front-line) (Blue and Red ground units
    in contact); it is left out when there is none, which is how every
    recording made before the front line reads. A build from before the
    front line (slice CL-FL) reads only nine values and refuses a recording
    with the tenth as damaged ("world.ground_target is not a ground target")
    rather than as a newer build's; the format number stays 3, so nothing
    earlier can tell it apart, and such a build cannot open a Quick Mission
    recording with a ground target made since, since the front line is on by
    default. The viewer rebuilds the terrain with it, so the
    template, its defenses, the added trucks and radars, the parked aircraft
    and the starts are the flight's.
  - **The area defenses** (B23, 2026-10-12), `world.area_defense=` followed
    by `aaa,sam,seed,nationality,night_stealth` (for example `3,3,17,10,0`),
    written only for a Quick Mission with no ground target whose AAA or SAM
    strength is above none, which makes the file format 3. The viewer
    rebuilds the same [area defenses](spec/surface-defenses.md#area-defenses)
    from it. A build from before it keeps the unknown entry aside and
    replays without the sites; `--recording-info` prints an `Area` line.
    Since 2026-10-13 (nr-b23b) the counts are rolled per slot and each SAM
    unit has a supply truck, and the header adds `world.area_rolled=1`. The
    five-field value does not change shape: a recording without the mark was
    made by the first build and rebuilds that build's fixed counts (2, 4 and
    6 SAM sites, 4, 8 and 12 gun sites, no trucks), so it replays as made. A
    build from before the mark keeps it as an unknown entry.
  - **The airfield scene**, `world.airfield_scene=N`, written only when N is
    not 0. 0 is the retail airfields, which every recording before the field
    has; 1 is the [redrawn airports](formats/redrawn-airports.md), the
    default since 2026-10-11 (protocol 24), with a ground target template's
    own runway piece still retail; 2 (since 2026-10-12, slice BG) is the
    redraw with that piece redrawn too; 3 replaces paired theater airfields
    with one long-runway field and neutral ground art. New flights record 3. A
    recording of a flight over the redrawn airports requires format 3 or newer.
    A flight with `TORE_REDRAWN_AIRPORTS=0` writes scene 0. The viewer rebuilds the
    scene the header names, whatever `TORE_REDRAWN_AIRPORTS` says: an old
    recording replays on the retail airfields, a new one on the redraw. A
    scene this build does not know (a later build's) opens with the retail
    airfields and a warning in the session log. A change to the plans that
    moves anything an aircraft touches takes a new number.
  - **The city fill scene**, `world.city_fill_scene=N`, written only when
    N is not 0. 0 is no city fill, which every recording before the field
    has; 1 is the [city fill](features.md) of `tore_world::city`, the
    default since 2026-10-11 (protocol 25), so a recording of a world with a
    city fill requires format 3 or newer. A world without one writes scene 0: a
    flight with `TORE_CITY_FILL=0`, or a theater whose fill has no
    buildings. A build from before the field (slice
    CL-CF) keeps the unknown entry aside and replays without the fill,
    with no warning. The viewer
    rebuilds what the header names, whatever `TORE_CITY_FILL` says: an old
    recording replays without the fill, a new one with it. A scene this
    build does not know opens without the fill and a warning in the session
    log. A change to the fill that moves anything an aircraft can hit takes
    a new number.
  - **Surface poses**: every unit that follows a route (columns, convoys,
    ships under way) as a list in each frame, keyed by its fixed id, with
    position and attitude predicted like an aircraft's, so a unit driving
    straight costs a zero residual and a unit that stands costs nothing. The
    wrecked flag flips when its hit points run out. These are the poses
    `RenderSnapshot.surface` holds; the viewer draws them with the unit's
    shape and wreck look.
  - **Launcher and magazine changes**: whenever a rail changes (a launch, a
    truck's rearm) every rail of that unit is recorded with the missiles it
    holds, and a gun's spare magazines whenever they change. A unit not
    listed has its full load. The viewer draws the rails from them.
  - **Debris pieces**: for each debris piece owned by a surface object, its
    piece index (`_B` or `_D`), as `DebrisPose.variant` holds it. Since
    nr-fx2 (2026-10-13) a destroyed parked aircraft throws none: it stays as
    a burnt hulk the viewer draws from the rebuilt scenery wherever the
    recording says it was destroyed, older recordings included (their
    recorded fragment is ignored).
  - **Flak**: its own effect code, `32` plus the explosion type less 15, so
    the KS-12's type 27 and the KS-19's type 28 are recorded as flak and not
    as an ordinary hit. A format 2 recording stores a burst as a hit, and the
    viewer lights and puffs a type 27 hit as flak; it has no way to tell a
    type 28 one.
  - **Surface unit registry**: a unit's id, short name (`SA-6`), label
    (`SA-6 #18`), side, hit points and position, in the entities section.
    The logs and exports name a surface shooter or victim by it, and a
    wreck's fire is sized by its hit points. Since B16 (2026-10-12, no
    format change) every destroyed building burns and about half the
    vehicles do (a fixed pick, the same in the recording as in flight), so
    `surface.wreck` says `burning` false for the others; the fires, a
    building's scorch and the craters on bombed runways are ordinary recorded
    marks, and the viewer draws each destroyed building's ruin from the
    rebuilt scenery, older recordings included. Since nr-fx2 a building's or
    fixed object's fire is recorded as lasting for good (the sim's own mark
    life) and the viewer's cap of 64 counts only fires with a limited life,
    as the simulation's does; a recording made from a networked game records
    every fire for good (the client was never told when one ends). A parked
    aircraft's fire lasts 15 minutes on a scorch, with no crater. The fires
    of knocked-down city fill buildings are drawn from the fill's own
    flattened set, which the city section keeps since nr-fx3 (below).
  - **Events**: `surface.burst`, `surface.phase`, `surface.rearm`,
    `surface.refill`, `surface.radar` and `surface.wreck` (see
    [the vocabulary](../crates/tore-replay/src/vocab.rs)). Unknown event kinds
    were always allowed, so an older reader would skip them too.
  - **Surface against surface** (2026-10-11, no format change): a burst or
    engagement at a ground unit or ship has that unit as its object and the
    `surface` field true; the unit joins the registry when first named, and
    its `surface.wreck` names the unit that destroyed it. Older recordings
    have none, as no unit fired at the ground before.
- **Effect scales** (slice BF, 2026-10-12, no format number change): the
  size each spawned effect is drawn at, percent of its explosion type's
  ([size by weapon](spec/explosions.md#size-by-weapon)), in a new chunk
  section, id 10, written only for the frames that spawn an effect not drawn
  at 100: for each such frame its gap from the last, how many, and for each
  its index among the frame's spawned effects and its scale byte (0 a dud's
  inert puff). Both formats carry it. An older build skips the unknown
  section, as it always has, and draws every explosion full size; a
  recording made before it has none, and this build draws its explosions
  full size too. A crater spawn's scale is its crater's drawn size, and a
  crater of size 0 (code 160) is a gun round's pock mark, which an older
  build draws as nothing. Craters and pock marks have no cap (John,
  2026-10-12); a recording costs about 24 bytes a mark. The motor flame of a missile or rocket is not recorded:
  the viewer dates each round's motor from its recorded age and the
  weapon's record in the import, so a recording of any age draws the flame
  only while the motor burns.
- **City** (nr-fx3, John 2026-10-13: replays track destroyed city fill
  buildings; no format number change): the city fill buildings a ground
  burst knocked down, in a new chunk section, id 11, written only for the
  frames on which the set changed: each building by **where it stands**
  (base centre, whole feet), not by its index in the fill, so a viewer whose
  rebuilt fill numbers its buildings differently (other import resources, a
  later build) still finds them; a place where the rebuilt fill has no
  building is skipped. The viewer reads every entry when it opens the file,
  and sets the fill to the set at the tick it shows, whether it got there by
  playing, seeking or playing backwards, so the flattened blocks, their
  rubble and their [fires](spec/surface-defenses.md#city-fill-buildings)
  show as in flight (agent decisions: places, not indices; changes only, not
  a set per chunk). A restarted flight's city stands again, marked on the
  entry. A recording converted from a network capture carries them too:
  the host's City flattened events (protocol 25) are written by place from
  the client's own fill, and buildings already flat when the seat joined
  stand flat from the replay's first tick. An older build skips the unknown
  section and shows the city whole; a recording made before it has none and
  shows the city whole here too. `--recording-info` prints a `City` line
  (how many were knocked down, in how many changes, from which tick). A
  raid that flattens 500 buildings costs about 4 KB. `TORE_REPLAY_CITY=0`
  replays a recording as an older build would, without them (a developer
  aid). Each entry's tick is also when its buildings began to burn: the
  viewer ages every burning building's smoke column from the tick it fell
  on (nr-fx4, below), so no extra data is stored.
- The air-to-ground round (slice RA) adds events and a weapon class and **no
  format change**: a recording is still format 3 with surface content and
  format 2 without, and both versions open unchanged. See
  [Air-to-ground events](#air-to-ground-events).
- Format 2 adds four powered-lift actuator positions, six signed gun mount
  coordinates and discrete linked gun membership to each aircraft record. Nozzle pitch, nacelle conversion and collective use the existing
  0-to-1 device precision; nozzle yaw uses the signed precision. Playback
  interpolates these actual positions and restores them for aircraft animation.
  Gun headings are divided by PI and elevations by PI/2. Linked membership
  is an exact integer from 0 to 7 and changes discretely; malformed masks
  are rejected.
  Format 1 records remain readable with their eleven original device slots and
  eleven neutral extension values. The writer produces format 2; refreshed export
  goldens identify the new format.
- The format version is in the prelude. A reader refuses a newer version
  with a plain message. Within a version, unknown sections, unknown chunk
  kinds and unknown header keys are skipped or kept, never fatal.
- A file cut short (a crash, or a recording still in progress) opens as
  **incomplete** and keeps every whole chunk; at most the last second is
  lost. The writer asks the system to put the file on disk every 30 seconds
  of flight, so a power cut can lose up to that much.
- A chunk that fails its checksum is skipped and reported; the chunks
  around it still play. Strings first defined in a lost chunk read as a
  placeholder.
- A jump in ticks, for example if the recorder falls behind, starts a new
  chunk and reads back as a gap.
- Replays converted from a network capture use the world's format too: their
  `net.*` events, header entries and result entries and `weapon.gun_burst`
  events are new names, which a reader that does not know them ignores.
- New kinds of data arrive as new entries, fields and flag bits within the
  version: released chaff and flares (`combat.countermeasure`), the
  `flame` flag, the player's view target (`player.view_target`) and the
  data link's events (`datalink.*`) came this way. A reader ignores flag bits it does not know,
  and a recording from before them has no releases and no lit flames: its
  chaff and flares were short effects, which the viewer does not draw, as
  flight no longer draws them.
- The viewer's [laser and bomb marks](#laser-and-bomb-marks) came this way
  (2026-10-11): `player.laser_spot`, `weapon.aim` and `weapon.impact` are new
  entries inside format 3 (2 without surface content). A recording from
  before them opens as it did and draws the marks its other air-to-ground
  events allow.
- The HUD and the targeting sight (nr-replayhud, John 2026-10-16) came this
  way too: `aircraft.hud` and `aircraft.sight` are new kinds inside the same
  format. A recording from before them opens as it did, and the HUD camera
  and Sensor View keep the picture they derived. A build from before them
  skips the unknown kinds.
- The stores on pylons (2026-10-12, slice BP3) came this way:
  `stores.layout`, `stores.station` and `stores.items` are new entries
  inside formats 2 and 3 ([stores](#stores)). A recording from before them
  opens as it did and draws no stores; a build from before them skips them.
- The `kept` flag (2026-10-11, slice FX-K) is new on `player.laser`,
  `player.laser_spot` and `player.designate`: it marks entries of an aircraft
  the AI flies with the cockpit a human left. Older recordings simply stop
  those entries when the human gave the aircraft back.
- A sample's fuel counts the internal tanks and, since 2026-09-29, the external
  tanks too (requested by John on 2026-09-29). New recordings say so with the
  header setting `fuel=internal and external tanks`, which the summary and the
  log's units echo (`fuel ... (used N lb, external tanks included)`). Older
  recordings have no such setting and hold internal fuel only; they read as they
  always did. A jettisoned tank's fuel leaves the total, so it counts as used.

## Exports

All exports read a recording and write text. They work on finished and
incomplete recordings alike.

### Debug log (JSONL)

One JSON object per line, for tools and for Claude:

| Line type | Contents |
| --- | --- |
| `header` | Game version, world, settings, tick range, completeness, damage notes, and the units of every field |
| `aircraft`, `weapon` | The registered identities |
| `event` | Every event with its fields, at its exact tick |
| `sample` | Each aircraft's state, once a second by default: position, attitude in degrees, velocity, airspeed and knots, G, fuel, heat, hit points, damage, flags, devices and controls |
| `tree` | Each display tree sample that differs from the previous one |
| `anomaly` | Every anomaly flag (below) |
| `footer` | End tick, result, completeness |

Options: a tick or seconds range, a list of aircraft ids (events with no
aircraft are always kept), the sample rate, and whether to include trees.
Numbers that are not finite are written as `null`, and an anomaly line
names them.

### Summary

A plain-English text file: the mission, conditions, result and length; each
aircraft's airborne time, highest and lowest G, lowest height above sea
level and above the ground (when telemetry records it), stalls, spins, fuel
used, shots, hits, kills (hits and kills on surface units count too: a shot's
`weapon.outcome` hit on anything but an aircraft, and each `surface.wreck`'s
killer), chaff and flares released, final state and time in each AI activity; a table
of every shot (launch geometry, time of flight, peak speed, closest approach
to the intended target, outcome and why); the communication transcript
with triggers, outcomes and reasons, including the music's changes; the
data link's changes when the recording has any (a line of counts for members,
locks, assignments, acknowledgements, cleared assignments and sort warnings,
then each assignment, acknowledgement, clearing and sort warning; locks are
counted and left to the log); a
timeline of key events (G-limit hits, chaff and flare releases and decoys among them; AI decisions
and effect changes stay in the log and the bookmarks); each bookmark with
the events of the ten seconds around it and every aircraft's state at that
moment; and the anomaly flags.

### Anomaly flags

Computed at export, so flight pays nothing. Each is a suspect worth a look,
not a verdict. Thresholds are **fitted**, agent decisions (2026-09-26), and
adjustable.

| Flag | Raised when |
| --- | --- |
| Non-finite | Any number in a state, event, tree or the header is not finite |
| Teleport | One tick's move differs from what the average velocity explains by more than 50 ft |
| Attitude jump | The nose or wings turn more than 20 degrees in one tick (2,400 degrees a second) |
| G excess | A live aircraft pulls more than 9.5 G or less than -4.5 G |
| Control oscillation | A stick axis reverses 8 times within 2 seconds, each swing at least 0.5 |
| AI stuck | An AI aircraft stays in one activity longer than 5 minutes |
| AI flipping | An AI aircraft changes activity, or target, 6 times within 2 seconds (decisions flipping every few ticks; an ordinary dogfight makes 6 changes in 5 to 10 seconds) |
| Track lost early | A guided weapon loses its target within 2 seconds of launch, other than to chaff or a flare (a decoy is the shot's outcome) |
| Fuel exhausted | An aircraft runs out of fuel |
| Order rejected | A recipient rejects an order, request or report |
| Call dropped, call suppressed | A call is dropped, or a spoken call (radio, crew, tower) is suppressed by a cooldown, limit or radio silence. The HUD's rate limit for AI lines, a cockpit message pushed off the screen by newer lines, and the AI's rules for which radio events become calls are routine and not flagged |
| Long wait | A call waits in a queue more than 3 seconds |
| Repeated call | The same speaker makes the same call 3 times within 10 seconds, each saying counted once (not its queued or cut-off entry) |
| Below terrain | Telemetry puts a live aircraft more than 5 ft below the ground |
| Crash undamaged | An AI aircraft crashes with no damage: it flew into the ground |

### Tacview

A `.txt.acmi` file in Tacview's ACMI 2.2 text format, sampled 10 times a
second by default (adjustable). Gun rounds are left out by default.

Each theater has a fitted GPS center and separate geographic spacing along
the map's east and north axes, calibrated from named airfields. Aircraft,
weapons, decoys and pilots all use that same mapping. The native U/V components
remain the game's original east/north coordinates in meters for flight-distance
calculations. Render Yaw follows the geographic mapping; native Heading and HDG
retain the game's heading. Unchanged values are omitted after the first line.

The [geographic alignment spec](spec/tacview-geography.md) defines the conversion.
This is an approximate map calibration, not surveyed GPS terrain.

| Game | Tacview |
| --- | --- |
| Height above sea level (ft) | `Alt` (m) |
| Bank, positive right wing down | `Roll`, the same sign: Tacview's roll is positive when rolling to the right, so a right turn shows a positive roll |
| Pitch, positive nose up | `Pitch` |
| Yaw, clockwise from map north | Geographic `Yaw` for rendering, native `Heading` and `HDG` for game telemetry |
| Airspeed | `TAS` (m/s) |
| Telemetry tree lines Mach, AoA, Sideslip, AGL | `Mach`, `AOA`, `AOS`, `AGL` (m) |
| Pilot throttle, afterburner or lit flame flag | `Throttle`, `Afterburner` |
| Gear, flaps, air brake, hook devices | `LandingGear`, `Flaps`, `AirBrakes`, `Tailhook` |
| Fuel (lb), G | `FuelWeight` (kg), `VerticalGForce` |

| Object | Tacview type and properties |
| --- | --- |
| Aircraft | `Air+FixedWing`; `Name` is the exact display name (for example `F/A-18D`), `Pilot` and `CallSign` the label, `Group` the wing, `Coalition` and `Color` the side (friendly blue, enemy red, neutral green) |
| Missiles, bombs, rockets | `Weapon+Missile`, `Weapon+Bomb`, `Weapon+Rocket`, with `Parent` set to the launcher |
| Gun rounds (optional) | `Projectile+Bullet` |
| Flares and chaff | `Misc+Decoy+Flare`, `Misc+Decoy+Chaff`, with `Parent` set to the releasing aircraft, where they left it, removed when a flare burns out (30 s) or chaff drifts away (20 s); an older recording's flare and chaff effects the same way without a parent |
| Ejected pilots | `Ground+Light+Human+Air+Parachutist`, with `Parent` set to the aircraft |

Object ids are hexadecimal and never zero: aircraft, projectiles, decoys and
parachutes each have their own range. Objects are removed with `-id` when
they leave the recording (a wreck when it is gone).

Events: `Destroyed` for kills, `Message` for each radio, crew or tower line
the player heard, once, when it was delivered, `Bookmark` for the player's
bookmarks, `TakenOff` and `Landed`, and `Debug` (shown with Tacview's
`/Debug:on`) for AI decisions with their reasons, orders and answers, comms
entries with an outcome or reason, data link changes, flight-model effects
starting and stopping, and G-limit hits. Commas in text are escaped as Tacview requires;
line breaks become spaces.

**Reference time.** The recording's date at the mission's local time of day,
shifted by the calibrated center longitude at 15 degrees an hour.

**Calibration.** All 16 base theaters have measured centers and geographic
spacing. The [calibration baseline](baselines/theater-georeference.md) lists
the public coordinate sources, airport matches, errors and reproducible tool.
Variant layouts use their base theater's calibration. A center override moves
the map without changing that theater's spacing. Unknown theaters use 0 N, 0 E
and the physical flat-earth fallback.

**Known limit.** Original map distortions and some inconsistent named placements
leave substantial landmark errors, particularly in France and Pakistan. The
Kuril and Falkland fits have only two reference airports and no independent
accuracy check. Tacview itself has not been used for a visual map-overlay check.

### Comparing two recordings

Reports header differences, differences in registered aircraft and weapons,
the **first second where the state checksums differ** (and the last that
matched), the **first tick where any aircraft's state differs** and which
aircraft, largest difference first, and, per category (kills, launches,
hits, chaff and flares, shot outcomes, AI decisions, comms, data link, flight
events, player and system events), the counts and the first event that differs or, when the events
match, the first difference in timing.

The checksum is FNV-1a 64 over every aircraft's exact state in id order,
computed by the app from live values once a second. States are compared
after decoding with tolerances of one and a half steps, so two recordings of
the same flight never differ just because their keyframes fall on different
ticks.

## Replays screen

**Replays** on the main menu's top bar, after Multi, opens the Replays
screen: every recording in the replays folder, newest first, the selected
one's details, and buttons to watch, keep, delete and export it. It is drawn
like the Controls screen, with the imported font, colours, title strip,
list rows and footer buttons. The entry and the screen are an opinionated
addition requested by John on 2026-09-26; their layout, wording and
behaviour are agent decisions (2026-09-26). The retail "Replay Last
Mission" button is unrelated and stays disabled. The entry shares Multi's
baseline and imported font in every input state, as specified in
[main-menu presentation](spec/main-menu-presentation.md#replays-entry).

### The list

| Column | Shows |
| --- | --- |
| Markers | A padlock when the recording is kept; a warning triangle when it did not finish (the game crashed or was closed while recording) or cannot be read |
| Started (UTC) | The date and time the flight started, in UTC, from the file name |
| Theater | The theater's name |
| Aircraft | The player's aircraft by its exact name, for example F/A-18D Hornet |
| Length | Minutes and seconds, with hours from an hour; `?` for an unfinished recording until its details are read |
| Result | Success or Failure for a Quick Mission; Ended, Quit or Restarted for Free Flight; Incomplete or Unreadable |

A second flight started in the same minute (`-2`, `-3` and so on) counts
as the newer one. Listing reads only each file's header, seek index and
footer. The list's title gives the number of recordings and their total
size.

### Details

The panel on the right describes the selected recording: its start, the
kind of mission, the theater, the weather and the local start time, the
player's aircraft, the wings and aircraft on each side, the length, the
result with how the flight ended and the pilot's fate, the player's kills,
the bookmarks with their times into the recording, the size and file name,
whether it finished normally, and whether it is kept. Wings, aircraft
counts, bookmarks and an unfinished recording's length need the whole file,
so a background thread reads them; the panel shows "Reading..." until they
arrive. The foot of the panel names the folder, where the exports go too.

### Buttons

| Button | Does |
| --- | --- |
| Watch | Opens the recording in the [viewer](#viewer); leaving the viewer comes back to this screen, with the list read again |
| Keep | Marks the recording kept, or not; its box is ticked while kept. Auto-delete never removes a kept recording. Saved at once in `replays-v1.conf` |
| Delete | Asks first, then deletes the file. A kept recording can be deleted too, with a warning. Exports made from it stay |
| Tacview | Writes `NAME.txt.acmi` beside the recording, the file `--recording-acmi` writes, and shows its path |
| Debug log | Writes `summary.txt` and `log.jsonl` into a `NAME-log` folder beside the recording, as `--recording-log` does, and shows its path |
| Auto-delete | Opens the auto-delete settings |
| Back | Returns to the main menu |

With no recordings, only Auto-delete and Back are available. Exports run on
a background thread, one at a time, so the menu keeps responding: the status
line shows "Writing ..." with the seconds so far, then the path written, or
why it failed. A recording being exported cannot be deleted until the
export finishes. Closing the screen lets a running export finish; the
session log records the result.

### Auto-delete settings

A panel over the list, with the [auto-delete](#auto-delete) rules:

- **Auto-delete:** On or Off.
- **Rule:** Keep a number, or Delete by age.
- **Keep the last:** 5, 10, 20, 50 or 100 recordings.
- **Delete older than:** 7, 14, 30 or 90 days.

Choosing a number also chooses its rule. A value written into the settings
file by hand that is not one of these shows as an extra choice. Every change
is saved at once. The panel says what the rule does and how many recordings
the next cleanup would delete, but deletes nothing itself: cleanup runs when
a flight ends and when this screen opens, and the status line then says how
many recordings it removed. Kept recordings, `.partial` files written in the
last ten minutes and files that are not recordings are never touched.

### Keys and mouse

| Input | Action |
| --- | --- |
| Up / Down | Previous or next recording; Down from the last goes to the buttons, Up from the buttons back to the list |
| PageUp / PageDown, Home / End | A page up or down, the first or the last recording |
| Enter, or a double-click | Watch the selected recording; on a button, press it |
| Delete or Backspace | Delete the selected recording, after the confirmation |
| Tab / Shift+Tab | Through the list and the available buttons |
| Left / Right | Between the buttons, and between Delete and Cancel; in the settings panel, the previous or next choice |
| Mouse wheel | Scroll the list three rows a notch; in the settings panel, move between rows |
| Esc | Close the settings panel or the confirmation, otherwise back to the main menu |

A held key repeats movement only, never Enter or Delete. Controller menu
buttons act as the arrow keys, Enter and Esc. `--snapshot-state replays`,
`replays-settings` and `replays-delete` draw the screen over a synthetic
list, for checks without recordings.

## Viewer

The viewer plays a recording back from any viewpoint. It draws the same
picture live flight drew, through the same drawing helpers and renderer
calls, rebuilt from the recording alone. Its interface, keys and layout are
agent design decisions (2026-09-26); the numbers below labelled fitted are
agent choices too.

### Opening a recording

- From the Replays screen: select a recording and press Watch (or
  double-click it). A recording that cannot be opened stays on the Replays
  screen, with the reason on its status line.
- From the command line: `tore-app --watch-replay FILE`.
- Esc opens the viewer's [pause menu](#pause-menu); its End Replay returns
  to the Replays screen, with the list read again, including when the
  viewer was started from the command line. The game's own world and
  aircraft come back as they were.

- **Live**, from the lobby of a multiplayer game while its mission flies: press
  **Watch** (see [watching a mission live](#watching-a-mission-live)).

The viewer builds the recorded world (map, weather choice, time of day,
wind and cloud deck) from the header, so a replay looks the same whatever
the viewer's own settings are. It loads the recorded player's aircraft and a
model for every other aircraft type the flight drew, and every weapon shape
the recording names.

### Watching a mission live

A player with no plane can watch the mission as it is flown (stage F phase 2,
slice F2-O2, [the design](ARCHITECTURE.md#the-observer-view)): in the lobby of
a game whose mission flies, the Loadout button reads **Watch**. The game
records what the host sends an observer into a recording of its own, which
grows as the mission flies, and the viewer plays it in a **live mode**. It is
the same viewer: every key, view, drone, label, panel, trail and the pause
menu work as on any replay, on the aircraft of both sides in their real plane
ids, and the player has no plane of its own.

- The transport bar reads **LIVE** while the playhead follows the newest
  frame, a second behind it. **Space**, **J** and the other transport keys,
  the bar and **Left** leave live; the last ten minutes can be scrubbed, and
  **End** returns to live. The playhead never passes the newest frame.
- With a delay set by the King (PvP only), the stream and the live view are
  that many seconds behind the host.
- The view asks the host to send what is near its camera at the full rate:
  the selected aircraft (Tab chooses another), or where the free drone is.
  Aircraft far from the camera move 4 times a second, as for any observer.
- **Esc** opens the pause menu, whose first row is **Stop Watching**: it tells
  the host and returns to the lobby. A mission that ends, or a host that stops
  the watch, returns to the lobby too, and Messages says why.
- **Your own aircraft.** When you go away in a flight (a menu open, the window
  out of focus or no controller for the host's `idle-ai` time, 5 minutes by
  default) and the AI flies your aircraft, the game opens this view on it, over
  your flight, with your aircraft selected. No flight input takes it back (a
  flight sim: nobody should fly AFK), so every key and the mouse are the
  viewer's here. You come back on purpose, from **Esc**, whose first rows are
  **Take Back Flight** (your own reserved aircraft, while it is alive),
  **Spawn in Aircraft** (when none is left: dimmed, with the reason, if the
  revival rules allow nothing) and **Leave Game** (a game you host asks you
  to choose it twice when no standby could take the game over)
  ([the design](ARCHITECTURE.md#the-observer-screen-for-an-away-players-own-plane-f2-o3)).
- Not there yet: smoke, contrails and gun rounds, radio and HUD lines, the AI's
  thinking, and each aircraft's fuel, G and controls (the observer stream does
  not carry them). The recording is a file in `observer/` in the data folder
  while the watch lasts (about 16 MB for ten minutes of 30 aircraft) and is
  removed when it ends; it is not listed on the Replays screen.

### Playing

Playback starts from the beginning at normal speed. The playhead can sit
between ticks, so slow motion is smooth, and every picture depends only on
where the playhead is: playing backwards shows exactly what playing forwards
showed at the same moment.

- **Speeds:** 1/8x, 0.25x, 0.5x, 0.75x, 1x, 2x, 4x, 8x and 16x, forwards or
  backwards. Play starts at 1x, fast forward at 2x; pressing reverse, fast
  forward, J or L again doubles the speed up to 16x. From slow motion they
  go back to their starting speed.
- **Steps and jumps:** one tick at a time while paused, 5 seconds or 30
  seconds either way, the start, the end, and the previous or next timeline
  marker. Playing on from either end starts over from the other.
- **Timeline markers:** launches (yellow), kills (red), orders (blue) and
  bookmarks (green), from the recording's events.

### Transport bar

Along the bottom of the view: start, step back, reverse, pause, play, fast
forward, step forward and end; the speed; the time and the recording's
length; the camera, which a click steps through the flight views, the
object view once something to look at is chosen, and the two drone modes.
Right-click the camera button to choose a specific view directly, including
follow/free drone. Right-clicking the scene still opens its object menu.
The selected-aircraft button moves to the next aircraft;
and Hide. Above the buttons is the timeline: click it to jump, drag along it
to scrub. The lit button shows what playback is doing. The bar uses the
Controls screen's colours, font and button style in the 640x480 interface
layer; the layer's top half is pinned to the top of the view and its bottom
half to the bottom, so the bar sits on the bottom edge of any window.

Drone commands are configurable under **Controls > Replay drone**: mode cycling,
direct follow/free selection, six movement directions, boost, mouse-look hold
and movement-speed steps. Defaults below remain unchanged. Keyboard and mouse
assignments are separate from flight controls. Assigned drone keys take precedence
over built-in replay shortcuts. Buttons and directional controls
on connected devices can also be assigned. Releases, focus loss, controller
disconnect and opening the pause menu clear held movement. Other replay shortcuts
remain built in.

### Selected object banner

The bottom banner is on by default, directly above the transport bar. This is
an **opinionated addition requested by John on 2026-10-15**. The layout and
selection rules below are agent decisions. Esc > Pref > **Object banner?**
switches it independently; H also hides it with the rest of the interface.
There is no new key binding. `--replay-ui banner` selects the banner alone
alongside the transport bar, and `--replay-ui none` leaves it off.

The banner follows the selected aircraft, the F6 wingman, the F12 weapon, or
the Alt/Ctrl view reference. An object view shows the explicit ground/weapon
**View from here** subject, otherwise its **Look at this** object. A drone
opened beside a weapon keeps that weapon selected until another object or
drone mode is selected. An absent object has no banner, so a finished weapon
never leaves its last speed on screen. The camera's aircraft fallback shows
that aircraft's banner.

| Object | Banner facts |
| --- | --- |
| Aircraft, including helicopters | Recorded label, exact type and side; airspeed in kt, recorded Mach when present, altitude in ft MSL, heading; recorded AI activity and mission role, or `player` for a human (crashed, ejected and destroyed states take priority); target name and straight-line range when its position is known |
| Missiles | Recorded weapon name, owner and side; speed in kt, altitude and direction of travel; recorded seeker state, or air-to-ground guidance mode; current target and range |
| Bombs and rockets | The same weapon identity and motion; a bomb says `falling` while present unless a recorded guidance mode gives more detail; target from its live state, guidance event or release aim |
| Moving vehicles and ships | Name and side; speed from consecutive recorded positions, heading and elevation; the most engaged recorded weapon station's phase and target, if any |
| Stationary SAM/AAA sites | Name, side and elevation; the most engaged recorded station's phase and target. Speed is omitted without a moving track |
| Other selectable scenery | Its existing replay name and elevation. Action, target, speed and orientation are omitted |

Heading is **true**, clockwise from the recorded world's north, with `T`
after the whole degree (000 through 359). This is the live HUD/view compass
convention; no magnetic variation is applied. A vertically falling weapon
has no heading. AGL and climb rate are omitted to keep the motion row short.
Values describe the current simulation tick (120 Hz); display trees keep
their recorded sampling rate. Pause, reverse, scrub and speed changes all
read the same tick and latest prior events, never the last displayed frame.

A dash means no recorded value or no target. The viewer does not invent a
missile boost/terminal phase or an AI order from its movement. Unguided
bomb/rocket targets are release aims, not locks that follow a moving target.
Multi-station sites show fire, track, pause, prepare, then other phases in
that priority order, with deterministic station-order ties.

The single-player recorder, file version, sections, event vocabulary and
network protocol are unchanged. Existing state, delta-coded display trees
and change events supply these facts. Old format 2/3 recordings still load.
Single-player recordings gain no bytes.

Client capture conversion now retains the small subset of cockpit facts
already on the wire: the player's view target, and the viewed AI aircraft's
activity and whether that AI targets this seat. It emits existing
`player.view_target`, `ai.activity` and `ai.target` events **only when these
facts change**, with the optional `source=net.readout` field. Initial known
facts are emitted too. Losing the viewed AI clears its activity to `-` and
its target to absent; two seconds without usable cockpit target facts clears
these scoped facts. A snapshot that omits an unchanged readout keeps them
current. Other cockpit groups need not be complete. An AI targeting another aircraft has an unknown target,
not an invented identity. No new wire fields, sections or version are needed.
Protocol-compatible captures can be converted again to gain these fields; old replay files
remain unchanged. Host AI thinking for other aircraft and full ground engagement phases do not
reach this client path and stay a dash. Surface gun bursts do transmit a
target: their existing `weapon.gun_burst` event now keeps that object, with
`source=net.surface` and `last_tick` through the last shot known to have fired.
The banner says `firing` with that target only over this bounded interval.
For round k the wire specifies `ceil(k * span / rounds)` ticks after the
start; an early-stop event's fired count limits the interval. It does not
invent a search phase after firing. Weapon tracks already retain targets. New event bytes scale with decisions/selection changes,
not the 120 Hz replay frame count.

The banner uses the replay's imported font, title/pale text and dark transport
background. Up to four rows occupy `rows * (font height + 3) + 8` logical
pixels above y=430 in the 640x480 layer. Text fits within 616 pixels; long
names are shortened with the replay's existing ellipsis helper. Cockpit
messages, subtitles and debug panels move above that space. Esc hides the
banner while its menu is open.

`--replay-banner-check FILE [--out DIRECTORY]` is a CPU-only developer check.
It checks every selectable identity, including weapons shorter-lived than a
second, and samples continuing tracks once a second. With `--out`, it saves
the actual banner/transport/message composition on a plain background at
3840x2160, 1920x1080, 800x600 and 640x360. It preserves existing image files.
This checks layout without pretending to validate the GPU scene.

### Keys and mouse

The viewer's keys are built in, listed in the
[controls master list](CONTROLS.md#built-in-controls-outside-the-tables) and
drawn on the Replay sheet of the [keyboard map](tore-keyboard-map.html#replay).
They follow the video-editor convention for J, K and L. F11 is not a view,
as in flight.

| Input | Action |
| --- | --- |
| Space | Play or pause |
| J / K / L | Play backwards / pause / play forwards; again for twice the speed |
| Up / Down | Next faster or slower speed in the same direction |
| Left / Right | 5 seconds back or forward; one tick while paused; with Shift, 30 seconds |
| Home / End | Start or end |
| PageUp / PageDown | Previous or next marker |
| Tab / Shift+Tab | Next or previous aircraft |
| F1 to F10, F12 | The flight views, on the selected aircraft; F6 again for the next wingman |
| Alt / Ctrl + F1 to F10, F12 | The same views from the selected aircraft's target / its newest missile; Alt+F4 still quits |
| O / Shift+O | [Object view](#object-view): look at the next or previous object present now |
| V / Shift+V | The [HUD camera](#hud-camera) / the [Sensor View](#sensor-view) on the selected aircraft |
| 1 / 2 / 3 / 4 | In the Sensor View: white hot / black hot / TV / night vision |
| Shift+F5 / F6 / F7 / F8 | The Sensor View in white hot / black hot / TV / night vision, from any view (the same keys as flight's AC-130 console) |
| Backslash | In the Sensor View: the next picture |
| Keypad 5 or Shift+/ | Recenter the look, keeping the view and zoom |
| + / - (keypad too) | Zoom in or out, 1.1 times a press, 0.5x to 4x |
| Backquote | Drone following the selected aircraft, then flying free, then back |
| W A S D, E / Q | Drone: move along the view and sideways, climb / descend; Shift is four times faster |
| Mouse wheel | Scrolls a panel or menu under the pointer; otherwise drone speed, or zoom in a flight view |
| Right-drag | Look around, or turn the drone |
| N / T / C | Name labels / mission timer / [Comms panel](#the-comms-panel) |
| I / F / G | [AI thinking / telemetry](#debug-panels) of the selected aircraft / guidance of its newest missile in flight; again to close |
| M / X | The [right-click menu](#the-right-click-menu) on the selected aircraft / close every panel |
| Right-click | The right-click menu on the aircraft, weapon or ground object under the pointer |
| R / Shift+R | Trails on or off / next trail length |
| B | [Laser and bomb marks](#laser-and-bomb-marks) on or off (on at first, for a recording that has any) |
| H | Hide or show the whole interface and the pointer |
| P | Save the view without the interface as a PNG |
| Esc | The [pause menu](#pause-menu); again, or Resume replay, to close it |

### Pause menu

Esc opens flight's paused Escape menu over the replay, drawn the same way
(tab strip, rows, bottom buttons, keyboard help) and worked the same way:
the arrows, Tab, Enter and Space, or clicks, with matching press and
release. An open right-click menu closes first. Opening it pauses playback
and hides the transport bar under its buttons; closing it plays on in the
speed and direction it had, or stays paused if it was paused. While it is
open it takes every key and click, so the bar, drones, right-drag and the
viewer's keys do nothing, and the pointer shows even with the interface
hidden. Closing it leaves a hidden interface hidden.

The menu is built from the imported `FMENUD.MNU` tree, keeping the retail
labels of the rows that mean something in a replay, in retail order. A
replay draws no cockpit, HUD or instrument windows and its debug panels are
always there, so the other Pref rows and tabs are left out. Without the
imported tree the same rows are authored.

| Row | What it does |
| --- | --- |
| ? > End Replay | Back to the Replays screen, as Esc used to be |
| ? > Exit to Desktop (Alt-F4) | Quits the game, as in flight |
| Control | The controls screen over the paused replay, as the Control tab opens it in flight; key, mouse, controller and head-tracking changes save and apply as they do there. The arrows step past the tab to the next one, so the menu is on ? or Pref when the screen closes and the next arrow does not open it again |
| Pref > Graphics... | The Graphics options screen over the paused replay; changes apply to the view at once |
| Pref > Sound... | The Sound/Music Prefs screen over the paused replay, as in flight |
| Pref > Time > Paused | Closes the menu, leaving the replay paused if it was playing and playing if it was paused |
| Pref > Time > Slow-motion, 1x, 2x, 4x, 8x | Playback at 0.5x to 8x forwards once the menu closes; the menu stays open, as flight's time rows do |
| Pref > Name labels? / Mission timer? / Flight path trails? / Laser and bomb marks? / Comms panel? / Object banner? | The same switches as N, T, R, B and C, plus the object banner, with On or Off beside them |
| Resume replay | Closes the menu |
| Keyboard shortcuts | The replay's keys, as in the table above |

Closing Graphics, Sound or the controls screen returns to the menu. The
replay's own keys are built in, so the controls screen changes the flight
controls, not the viewer's keys. The title reads REPLAY
PAUSED. The pause menu is opinionated, requested by John on 2026-09-28,
who chose the End Replay and Exit to Desktop labels and asked for the
controls screen in it; the four switch rows'
labels, the title, the help text, hiding the bar and what the Time rows do
are agent choices (2026-09-28).

### Cameras

- **Flight views on any aircraft.** Tab picks the aircraft; F1 to F12 give
  the same views as in flight, from that aircraft, following the
  [view rules](spec/flight-views.md#mission-replay): front, up and track sit
  at the aircraft and hide it (there is no cockpit in a replay); back looks
  from its pilot's eye over its spine and tails, as the player's own F2
  does; threat, wing, target and fly-by views work from it. Unmodified F12
  cycles every active non-gun weapon in stable projectile-id order, including
  missiles, bombs and rockets, retaining the selection as others launch.
  It sits 30 feet behind and 10 feet above the weapon and draws its lit body.
  Within 328 feet of its target or predicted surface impact, the eye stops and
  watches the weapon fly on. The first crossing is recovered from recorded
  history, so a direct seek or reverse playback gives the same eye; rewinding
  before the crossing restores the moving chase. The eye stays at least 10 feet
  above terrain. A miss keeps the eye fixed until expiry. See the
  [prediction and fallback rules](spec/flight-views.md#weapon-chase-captures).
  On expiry it holds the last look for 1.5 seconds of recorded game time,
  then shows Forward on the selected aircraft, like live F12. The world keeps
  moving. It retains the weapon identity so seeking backward into the hold
  restores that pose and seeking before impact restores the weapon. Pause and
  playback speed apply to the hold; view keys interrupt it immediately. Press F12 again to pick
  another active weapon. Weapon views
  face their retained aircraft or standing ground target, otherwise their flight
  direction. F6
  pressed again moves to the next wingman and names it ("Wingman view: Enemy
  1-2", the recorded label). Alt with a view key uses the aircraft's target
  as the reference and Ctrl its newest missile in flight (its shots at the
  player included); an unmodified key goes back to the aircraft itself. F7
  from the aircraft carries flight's bearing compass along the top of the
  view, in the recorded aircraft's HUD font and colour, the strip across the
  middle half of the view since a replay has no instrument windows; it hides
  with the interface. Keypad 5 and Shift+/ recenter a right-drag look, and
  + and - zoom as in flight. The fly-by picks a new point once the aircraft
  is 3 nmi from it, as in flight, and playing backwards puts the point ahead
  of the reversed motion. The target is that aircraft's own target when the
  recording has its AI target changes, otherwise the player's recorded view
  target (`player.view_target`), so the player's target views hold a dropped
  target by sight and let it go exactly as they did in flight. A recording
  from before view targets were recorded uses the player's designated
  target, which it notes with every command the player gives. A view that cannot be shown (no target, no wingman, no missile) says why
  once and shows the aircraft from outside (F12 uses Forward), coming back when it can be shown
  again (John's choice, 2026-09-28: the replay does not refuse the key or
  return to F1 as flight does); once the aircraft has left the recording (a
  wreck that exploded), the camera stays where it was. The viewer opens in
  the external view of the player.
The reel director also exposes the F12 chase placement as `view = "weapon"`.
Its `anchor` is the firing aircraft; it follows that aircraft's newest launch
and returns to Forward immediately at expiry, retaining its editorial capture
behavior without the interactive F12 approach stop or post-impact hold. The separate `seeker` view stays available as
a nose camera with the weapon hidden. See [capture options](../tools/reel/README.md#weapon-cameras).

- **Active weapons in the object list.** Empty-space right-click lists aircraft
  and active missiles, bombs and rockets. It refreshes during playback and seeking,
  removes expired weapons, and preserves focus by object identity. Choosing a
  weapon opens View from here. Guns are excluded. F12 includes the same weapon classes.
  These replay choices were requested by John on 2026-10-01.
- <a id="object-view"></a>**Object view.** A camera from any object to any
  other, at any range: aircraft, ground objects of the recorded world
  (buildings, runways, and a ground target's units, until they are destroyed)
  and weapons in flight (missiles, bombs and rockets, not gun rounds). A
  surface unit that follows a route is where the recording had it on the tick
  and faces the way it faces, so the view can ride with a tank in a column or
  look at a ship under way; the recording's registry names it (`T-80 #7`),
  and a unit it names by id only is "Ground object" and its number (slice
  RP1, agent decision). Requested by John on
  2026-09-28; the keys, menu items, order and fallback below are agent
  decisions. It starts from the selected aircraft, or from a ground object or
  weapon chosen with **View from here** on the right-click menu; **Look at
  this** chooses what it looks at. View from here with nothing else chosen
  yet looks at a weapon's own target while it flies, otherwise the nearest
  other aircraft. O and Shift+O step what it looks at
  forwards and backwards through everything present on the current tick, in
  a fixed order: aircraft, then weapons, then ground objects, each by id,
  leaving out the object it starts from. Any of these switches to the view.
  The camera is flight's relation camera: 180 feet behind the starting
  object and 60 above (30 and 10 for a weapon), on the line to the other
  object, so both sit on the centre sightline; its eye stays at least 20
  feet above the ground (fitted), so a view from a building up at an
  aircraft never looks from under the terrain. The camera button reads
  "Object: You > AIM-54 from You", and a line under the messages at the top
  gives both names and the range in nautical miles. When either end is
  missing it says why once and shows the starting object from outside, or
  keeps the last camera when that is gone too; it waits for the object
  rather than moving to another, and comes back when both are present again,
  for example after stepping back before a missile hit. F-keys leave it as
  usual, Tab selects an aircraft to start from, and the camera button steps
  to it after F12 once something to look at has been chosen.
- <a id="hud-camera"></a>**HUD camera** (V; the right-click menu's **HUD
  camera** on an aircraft, and the camera button's menu). The selected
  aircraft's HUD as a 1980s or 1990s HUD video recorder put it on tape:
  the forward view from the pilot's eye with the HUD drawn as the recorded
  aircraft's HUD shows it, and no cockpit, canopy or panel. Opinionated,
  requested by John on 2026-10-16 ("like an old style VHS recording of the
  HUD ... chromatic aberration, colour graded", then "zoomed in a bit ...
  a nice VHS texture smoothing"); everything below is an agent decision,
  tuned on offscreen renders so the HUD's numbers stay readable.
  - **The HUD** is the live weapon HUD, drawn by the same code flight uses,
    from the aircraft's [`aircraft.hud`](#hud-and-sight-events) sample at
    that tick (nr-replayhud, John 2026-10-16). Armed, it shows the mode the
    pilot had (air to air, the gun, or air to ground), the selected weapon
    and its count, the seeker word, the target box on the recorded target,
    the range, closure and aspect worked out from that target's pose, the
    missile's seeker circle and launch zone, the gun funnel, and a bomb's
    CCIP or level T with the time to impact counted from the recorded
    landing. The laser rows (`LSR 2.4`, `LSR RDY`, `MASK`, `BUD`) still come
    from the [laser and bomb events](#air-to-ground-events). `nav` true draws
    the NAV bank scale, as flight does. A recording with no `aircraft.hud`,
    or a weapon the import does not have, keeps the earlier picture: the
    flight data, the derived target box and gun pipper, and the laser rows.
  - **Framing.** The HUD's drawn area (the same 640 x 480 raster on every
    aircraft) fills 86% of the picture's height: zoom 1.6, a 39 degree
    field instead of the front view's 60. + and - still zoom.
  - **The tape** (`crt.rs` `VHS`, `crt.wgsl`): each line low-passed as tape
    keeps it, brightness over 1.7 px and colour over 6.5 px and 2.5 px late
    (pixels at 1080 rows), each line blended with the next; lifted blacks
    (6.5%), crushed highlights (92%) with a soft shoulder and a bloom; 62% of
    the colour with a green-cyan cast; a faint edge ring; red and blue
    pulled apart 2.4 px toward the edges; grain and blotchy chroma noise;
    240-line scanlines with the other field a little darker; a 30% vignette;
    each line slipping up to 0.45 px and the frame up to 0.5 px; the
    head-switching tear along the bottom 1.6%; a faint tracking band rolling
    up the picture for 1.5 s in three of every ten 7.5 s windows; and a
    dropout streak in 2.5% of fields. The recorder's lines are burned in:
    REC with its light, `HUD`, the tape counter (mission time), the aircraft
    and its type, and `SP`. The symbols are recorded a little bolder and
    whiter with a soft dark halo so they read over a bright sky.
  - Every part of the look comes from the replay tick, 60 fields a second,
    not the wall clock: a still or a paused picture is always the same, at
    any speed, while scrubbing and in `--replay-stills`.
  - **24 pictures a second.** Opinionated, requested by John on 2026-10-19
    ("it should run at 24 fps capped (because it's a VHS tape)"). The tape
    layer (the world, the HUD, the recorder's lines and the tape effects) is
    made 24 times a second on a fixed grid and shown again between those
    pictures; the transport bar, labels, panels, cursor and menus are drawn
    every frame at the window's own rate and stay responsive. Playback speed
    and timing are unchanged: this is a presentation rate, not a playback
    rate, and the other views keep the frame rate from the Graphics screen.
    A paused replay whose moment, aircraft, zoom and trails and marks
    choices have not changed makes no new picture at all, so a stopped tape
    costs the interface and one CRT pass a frame (the picture is a function of
    the tick, so a paused tape is a still). With the Escape menu or a screen
    open the picture is still made at 24 a second, so a Graphics change shows.
    Before this a frame spent about 40 ms of CPU at 4K placing the HUD on the
    tape layer with a full-picture smoothing and blur; that now only works
    where the HUD has symbols and the recorder's lines are kept until their
    words change. The pictures are bit for bit the ones made before.
    `crates/tore-app/src/replay/tape_pacing.rs` has the rule, the renderer
    keeps the picture (`Renderer::repeat_console`).
- <a id="sensor-view"></a>**Sensor View** (Shift+V; the right-click menu's
  **Sensor view** on an aircraft that has one). The [AC-130 sensor
  console](spec/target-window.md#sensor-console) from the selected
  aircraft's targeting sight, through the same renderer, picture modes and
  CRT. Offered only when the aircraft has a sight
  ([who has a sight](spec/air-to-ground.md#who-has-a-sight)): the AC-130's
  console; its own laser or FLIR record (the F-16C's and A-10's Pave Spike,
  the Su-25's and V-22's laser, the Rafale's AAS-38, the internal FLIRs);
  or a loaded Pave Spike, which the recording does not list, so an aircraft
  with no sight of its own counts as carrying one when the recording shows
  its laser. Otherwise the key says so in the message line and the view
  stays. Opinionated (John, 2026-10-16); the layout and rules are agent
  decisions, spelled out in [the target window
  spec](spec/target-window.md#sensor-view-in-a-replay).
  - **What the recording holds and what is worked out.** When the recording
    holds an [`aircraft.sight`](#hud-and-sight-events) sample and the sight
    is a gunship, a pin, a track or a return, the camera uses that azimuth,
    elevation and zoom. Format 4 also records a free fighter sight. Older
    free fighter sights and recordings with no sight event keep the derived
    line, in this order: the aircraft's own laser
    spot, its designation (the object where it is now, or the pinned point),
    its guided weapon's aim, the ground object its recorded target names,
    the AC-130's orbit centre (`player.orbit`), where its rounds burst on
    the ground from 2 s before to 3 s after the tick
    (`combat.ground_impact`), its latest bomb landing, else the sight's
    default look. That derived picture says which (`LOS FROM LASER SPOT`,
    ... `NO LOS RECORDED`). + and - still zoom. The picture mode (white hot,
    black hot, TV, night vision) is the viewer's own choice, not recorded:
    it starts in white hot and 1 to 4, Backslash or Shift+F5 to Shift+F8
    change it; the choice stays for the session.
  - **The AC-130** shows its console exactly as in flight. With a recorded
    sight the rounds, the selected gun, the readiness, the A-G radar (on or
    off, and the range step) and the target lines come from the sample, and
    format 4 uses the live pipper impacts and radar contact dots. The target
    block includes clock bearing, and the gimbal graphic includes the lead
    gun's sector. Older samples recompute the pipper from trains and terrain. The status line then names the gun, not the derived line. An
    older recording, or an AI gunship with no trains, still shows the rounds
    as `----` (or the rounds without a pipper) and says where the line of
    sight came from. Older recordings have no radar contact dots.
  - **Other aircraft** show their own symbology on the same console: the
    aircraft and its type (not the pod or sensor's name: John, 2026-10-16),
    `LASE` (boxed and
    blinking while lasing), `LSR RDY`, `MASK` or `OFF` with the laser's range
    in feet and nautical miles, `BUDDY 1-2 2.8NM` when its weapon follows
    another aircraft's spot, the weapon it last released, `TTI 0:22` (`X2`
    for several, `IMPACT` for a second), the designated target with range
    and bearing, the landing's words for twenty seconds, and marks on the
    picture: a laser spot (a buddy's in a dashed ring), the designated
    target's box, the weapon's aim point and a landing's X.
  - The camera sits at the sensor's eye (the AC-130's dome, 4 ft under the
    aircraft for a pod, the centre for a built-in sensor), held inside the
    sight's gimbal; at its edge `GIMBAL LIMIT` flashes.
- Both views follow Tab's selection, work paused, at every speed and while
  scrubbing, and leave to any other view with its key. On an aircraft that
  is not in the recording now they say so and show it from outside. The
  offscreen director draws them as views 4 (HUD camera) and 5 to 8 (the
  Sensor View in white hot, black hot, TV and night vision), so
  `--replay-stills` renders them (`--reel-render` too).
- **Drone.** Backquote switches to a drone that follows the selected
  aircraft at a fixed offset in world axes, so the aircraft stays where it
  was framed while the drone travels with it; Backquote again lets it fly
  free where it is, and again returns to the flight view. The drone starts
  where the camera was or, when that is far away, 250 feet behind, 90 feet
  to the right of and 60 feet above the aircraft: off its path, so it does
  not fly through the flares and smoke the aircraft leaves. W A S D move along
  the view and sideways, E and Q climb and descend (as in the terrain
  viewer), Shift is four times faster, and the wheel sets the speed from 20
  to 5,000 feet per second (250 at first, 25% a notch). The drone stays 10
  feet above the ground and moves in real time, so shots can be framed while
  playback is paused. Speed range and offsets are fitted.
- Right-drag looks around, at the mouse-look sensitivity set on the
  Controls screen. The object view and flight's relation views keep their
  own direction, as in flight.

### What is drawn

Everything live flight draws outside the cockpit, rebuilt for the tick
under the playhead:

- Aircraft, fixtures, weapons, tracers, debris and effects from the
  recorded frames, blended between ticks the way live flight blends its
  last two ticks. Frames are decoded a second at a time from the nearest
  keyframe; the eight most recently used seconds stay decoded, so playing
  either way reads each second once.
- Ejected pilots, with the imported ejection art.
- Buildings and airport objects, minus those the recording shows destroyed
  by that tick. A wreck shows its destroyed look. A ground target's template,
  trucks, radars and parked aircraft are rebuilt from the header
  ([format 3](#versions-and-damage)); its launchers show the rails the
  recording left them, its columns and ships drive their recorded poses, and
  the wreck smoke, gun flashes, launch flashes and flak light and puffs
  rebuild from the recorded rounds, flak and hit points as flight draws them.
  A wreck's smoke starts on the tick the unit died, wherever the playhead was
  put.
- Smoke and contrails rebuilt from their release ticks with the
  simulation's lifetimes, rise and caps (playing forward, only the puffs
  released since the last tick drawn are read and the expired dropped; a seek
  or reverse play reads the whole window, and the result is the same); effects
  from their start ticks.
  Craters and crash-site fires, which can last the whole mission, are read
  once when the recording opens and kept to the simulation's caps.
- The city fill as the recording left it at the tick shown (nr-fx3): its
  knocked-down buildings flat and burning, read once from the city section
  when the recording opens; seeking or playing backwards stands them up
  again. Each burning building's smoke column is as tall and dense as it
  was at that tick, however the playhead got there: its age is the tick
  shown less the tick the building fell on, so a seek, a jump back and a
  play-through draw the same smoke (nr-fx4, John 2026-10-13). At night every explosion and fire also glows, to the limit of the
  night scene, as in flight ([night glow](spec/explosions.md#night-glow)).
- Chaff clouds and burning flares with their smoke trails and glare,
  flown again from their recorded releases
  ([chaff and flares](#chaff-and-flares)), and the light that flares and
  lit afterburners throw on aircraft, ground, water, clouds and smoke, as
  live flight draws them through the same renderer calls. Nothing is alive
  a little over 33 seconds after the last release, so each stretch that
  follows releases is flown from its start, with a copy kept every second
  (up to 512 of them near where the playhead has been) so a jump or
  reverse play steps at most a second.
- The player's wing vapor, rebuilt by stepping the vapor history over the
  last 250 ticks of recorded poses from one of live flight's own commit
  ticks, so it matches live flight exactly once two seconds of history
  exist (playing forward it is stepped on pose by pose instead, to the same
  history; a seek or reverse play rebuilds it). Recordings keep attitudes, not turn rates, so the roll rate that
  shortens the vapor is worked out from the attitudes a tick apart
  (fitted).
- The weather, re-stepped one tick at a time as live flight steps it, with
  a snapshot every second (the environment's own state every ten seconds)
  so a seek is at most a second of stepping. Live flight steps the weather
  from the view camera; the viewer steps it from the recorded player's
  forward view, so each tick's weather is the same however it is watched
  (fitted: only the sun-whitening smoothing can differ from what the pilot
  saw). The palette is then resolved for the viewer camera's height. A
  headless probe never steps the weather, so its recordings keep the launch
  sky.

- Each aircraft's pylons and the stores still on them, from the recorded
  [stores](#stores) at that tick; none in a recording from before them.

There is no cockpit, instrument panel or mirror in a replay; the HUD shows
only in the [HUD camera](#hud-camera).

### Interface parts

- **Name labels** (N, on at first): each aircraft's label over it in its
  side's colour (friendly blue, enemy red, neutral green, grey when
  unknown), drawn at the view's full resolution with a dark shadow; the
  selected aircraft's is in brackets. Aircraft over 100 nautical miles away,
  wrecks on the ground and the aircraft the camera sits in have none. Labels
  that would print over one another (a formation seen from a distance) are
  lifted so each stays readable; the order follows the aircraft's number
  (agent decision, 2026-09-28).
- **Mission timer** (T, on at first): mission time as `mm:ss.t` and the tick.
- **Cockpit messages:** printed as flight prints them, in the HUD's colour
  and font at its size, centered, newest lowest, just above the transport
  bar where flight has the window's edge: each line for five seconds from
  its tick, a repeat moving to the bottom with a fresh five seconds, at most
  seven lines. They go with the subtitles' switch.
- **Subtitles:** radio, tower and crew lines the player heard, each once
  from its delivery, for four seconds from their tick, newest lowest, up to
  three, above the band the seven message lines can fill.
- **Comms panel** (C): every recorded comms and audio entry up to the
  playhead, each line once with its final outcome, filtered by kind and
  aircraft; see [the Comms panel](#the-comms-panel). While it is open the
  subtitles are not drawn, since it lists the same lines.
- **Flight path trails** (R, off at first): each aircraft's and guided
  weapon's path over the last 30 seconds (Shift+R: 10, 30, 60, 120 or 300)
  as a thin line in its side's colour, a weapon's paler than its owner's.
  They are drawn in the 3D view, so terrain and aircraft hide them, from
  path samples ten times a second plus where the aircraft is drawn now.
  Colours and lengths are fitted.
- **Laser and bomb marks** (B, on at first): see [below](#laser-and-bomb-marks).
- **Hide UI** (H or the Hide button): hides the bar, labels, subtitles,
  timer, debug panels, the right-click menu, the pointer and the laser and
  bomb marks (their lines, shapes and words: the square on the player's
  designated target goes too); trails stay as chosen, and playback and camera
  keys keep working. H brings the
  interface back; the pause menu still opens over the hidden interface.
- **Screenshots** (P): the 3D view without any interface, at the view's
  size (up to 1920x1080), saved as
  `screenshots/<recording>-tick<tick>.png` under the app data folder. The
  PNG writer is built in: uncompressed image data in stored deflate blocks,
  so a full-HD picture is about 6 MB.

Trails, fallen buildings and the weather need the whole recording read
once. A background pass does that when the viewer opens, a second at a
time; until it has reached a moment, trails there are shorter and the sky
waits at the last moment it has reached.

### Laser and bomb marks

John asked on 2026-10-11 for the replay to draw laser spots, "useful to
troubleshoot failed drops". B switches the marks (Pref > Laser and bomb
marks? in the pause menu; `--replay-ui marks`). They are on at first, and
drawn only for a recording that has [air-to-ground events](#air-to-ground-events);
an older recording, or any other, shows nothing and the toast says so. They are
in the 3D view like trails: lines and shapes a fixed size on screen at any
range, hidden by terrain and aircraft, and the words beside them print with
the name labels. The code is `tore-app/src/replay/ground_marks.rs`; the
colours, shapes and words are agent choices (2026-10-11).

| Mark | What it shows |
| --- | --- |
| Laser spot | A cross in a ring where the spot is while its aircraft lases, with a line from the aircraft and `YOUR LASER` or `FRIENDLY 1-2 LASER`. Green is the recording player's own laser, cyan a friendly buddy's, red or green any other side's. The spot follows `player.laser_spot` entries, so it moves with a moving target |
| Masked laser | A dim, hollow ring at the last spot with a dashed line and `MASKED: GIMBAL` (or range, terrain), while the laser is wanted but cannot see the point. It goes with `off` or `ready` |
| Designation | A square on the ground object an aircraft has designated (or its pinned point), in the aircraft's side colour, with `YOUR TARGET (A-G RADAR)`; it goes when the designation is given up |
| Guided weapon | A line from the weapon to the point it steers for and a shape there (a ring for a spot, a diamond for a JDAM's stored point), in the colour of the spot it follows; `GUIDING ON YOUR LASER`, `GUIDING TO STORED POINT`, or yellow `WAITING: NO LASER SPOT YET` for a laser weapon with none yet |
| Blind weapon | Orange, with a dashed line to the point it last steered for and the reason it gave: `BLIND: YOUR LASER WENT OFF`, `BLIND: THE LASER SPOT LEFT ITS 45 DEGREE CONE`, `BLIND: EVERY LASER SPOT WAS OUT OF ITS REACH AT RELEASE` |
| Landing | A magenta X in a ring for twenty seconds after a bomb lands, a dashed line to the point it steered for and `IMPACT 42 FT FROM AIM, 1,300 FT FROM TARGET` (`BLIND IMPACT` after it went blind, `DUD` for one that landed before its fuze armed) |

So a failed drop reads from the pictures: the weapon's line ends on the spot
while the spot is where the pilot wanted it, or it turns orange when a laser
goes off, or the landing sits far from both marks. Seen from the air the
spot, the aircraft's line and the weapon's line are all there; the object
view (O) on the weapon follows it down.

What the recording must hold: the spot's position at any moment, and the
aim of each weapon. `player.laser` only says when a laser changes state, so
the recorder (slice FX-R) also writes `player.laser_spot` and `weapon.aim`
as the spot and the point move, and `weapon.impact` when a bomb lands. They
are new events inside the format (a reader that does not know them skips
them), so recordings made before them still open. They show the spot where
its laser came on and a weapon's aim where it began guiding, but nothing
after, and no landings.

With the interface hidden (H) the lines and shapes stay and their words
go. `TORE_REPLAY_MARKS=1` draws the marks in the promo [reel
renderer](#promo-reel-capture), which otherwise never shows them, so the
marks can be checked without a window. `TORE_REPLAY_TRAILS=1` does the same
for the flight path trails (R). Both are drawn relative to the render origin,
as the rest of the combat geometry is, so they sit on their aircraft however
far from the map's corner the flight is.

For a device-free weapon sound check, set `TORE_WEAPON_SOUND_LOG=1` when
running `--recording-log FILE --out DIR`. With an imported profile in
`TORE_DATA_DIR`, this also writes `weapon-sound.tsv`: arrival tick, emission
tick, sound, shooter, gun flag, arrival distance, gain before preference
volumes and whether the imported sound exists. Rows are arriving other-shooter
reports and immediate own releases. `store-sound.tsv` checks all virtual bomb
and R-27 variants against the loaded weapon and PCM catalogs. It uses the
viewer's source lookup and the mixer's shared emission selector and wavefront. The listener follows the recorded player;
`TORE_WEAPON_SOUND_SUBJECT=ID` follows another aircraft externally. The audit
reads the entire recording regardless of log export filters. It neither plays
sound nor changes the recording.

### Sound

John asked on 2026-09-26 to hear the communication in a replay: the
voices, tones and effects played back at normal speed. At exactly 1x
forwards the viewer plays what the player heard, through the same sound
calls live flight makes, so it sounds as the flight did. How it is
scheduled is an agent design (2026-09-26); the code is
`tore-app/src/replay/sound.rs`.

- **Radio, crew and tower lines** the player heard, each once and in
  recorded order, queued behind the line before as in flight: radio calls,
  crew remarks, tower lines, the tower's reply to the player's own request
  (which replaces waiting tower speech), and recordings played straight
  into the cockpit, such as the death scream. Each plays the way its
  recorded route plays in flight: the radio queue, the tower's own queue,
  or straight in. The recording lists everything that happened to a line,
  so only its delivery speaks: an entry recorded as not heard, or as
  queued, held back, dropped or cut off, stays silent. Cockpit messages
  have no voice.
- **Chaff and flare releases** sound as combat's other traveling sounds do,
  from where they left their aircraft. The player's own plays at once and
  centered while the camera sits in the player's aircraft, as it does in
  the cockpit.
- **The player's wing orders:** every order that went out on the radio
  plays its voice, which cuts off waiting wingman speech as in flight. An
  order the wing refused has no voice but still cuts the speech off, as in
  flight; one the game refused before it went out (no AI wing, no airport
  to land at) does nothing. Orders between AI aircraft are silent.
- **Cockpit sounds:** the seeker tone at its recorded loudness, the stall
  warning, gear, flap, hook and air brake sounds, the ejection warnings,
  seat and parachute, and a friendly wingman ejecting. They are the
  player's cockpit, whichever aircraft the camera follows. Queued tower
  speech stops when the player's aircraft is lost, and when the runway
  under the player's landing clearance is destroyed, as in flight.
- **Traveling sound:** impacts, explosions and the player's weapon
  releases, aircraft and missiles passing the camera, and sonic booms,
  through the flight's [distance, delay and stereo model](audio.md#traveling-sound)
  with the viewer's camera as the listener. The model steps once for every
  recorded tick played, so sound takes as long to arrive as in flight. A
  camera inside the player's cockpit hears the player's releases at once,
  as the cockpit does. A cut to another view, another aircraft or the drone
  starts the pass detector afresh, so a cut never sounds like something
  flying past.
- **Other shooters:** every aircraft and surface weapon launch is resolved
  through the imported weapon catalog, including gun rounds. The viewer's
  camera hears all nearby shooters, with the same distance, canopy, burst and
  voice limits as live play and multiplayer clients. See the
  [sound spec](spec/sound.md#other-aircrafts-launches). Existing recordings
  already contain these launches, so their format does not change. Converted
  network captures also use newly born reconstructed gun rounds when individual
  launch events are absent. Old network captures cannot recover flak reports
  that the host never transmitted.

- **Engine:** the engine and afterburner loops of the aircraft the camera
  follows, from its recorded engine, afterburner and throttle, and its
  start or stop sound when its engine lights or stops while it is watched.
  Following another aircraft swaps the loops without cutting off speech.

What stops it:

- **Pause** freezes everything playing, a line mid-word included, and
  playing on resumes it. Playback pauses itself at the end, so sound
  freezes there too.
- **Any other speed, reverse, dragging the timeline, or a jump** of the
  playhead (the arrows, a marker, Home, End, a click on the timeline, a
  step while paused) cancels waiting speech and silences tones, loops and
  traveling sound. Back at 1x forwards, sound starts afresh from the
  playhead: the seeker tone, stall warning and engine sounding at that
  moment come back at once, and when the playhead sits exactly on a tick,
  that tick's cues play too.
- **Leaving the viewer** stops everything.

Fitted details, agent decisions (2026-09-26):

- The seeker tone's loudness follows the seeker's signal quality or the
  estimated hit chance in flight. The recording keeps it in steps of 0.05
  (see [Communication events](#communication-events)), so the replay's
  tone can be up to 0.05 from the flight's between steps. A recording made
  before the loudness was kept names only the tone, and the replay uses
  the live strength at 50 percent: radar lock 0.7, radar search 0.425,
  infrared lock 0.575 and infrared search 0.2875, times the seeker volume.
- An infrared lock on a surface weapon plays a different tone. The
  recording says whether the weapon aims at the surface; in a recording
  made before it did, the replay takes the lock to be of the same kind as
  the search tone before it.
- The engine loop plays while the recorded engine runs and the aircraft
  is neither destroyed nor abandoned. AI aircraft are recorded as they are
  drawn, engine always running and afterburner never lit, so a watched AI
  aircraft hums at its recorded throttle.
- Only missiles with a reviewed profile pass the camera audibly, as in
  flight.

Not in a replay:

- **Music.** The recording keeps every change in the inputs that choose a
  score (`audio.music`, recorded when the flight had sound), but the
  replay does not hand them to the music yet, so a replay has no music.
- Sounds that leave no record: the mixer's own choices, such as speech
  dropped from a full queue, and whatever a headless probe does not run
  (crew voice, music and cockpit messages).

Scheduling is checked without listening. The tests play a synthetic
recording in frames of every length, at every speed and through jumps and
pauses. `TORE_REPLAY_SOUND_LOG=1` writes every cue the viewer schedules,
with its tick, to the session log (traveling sound only on ticks with an
impact, explosion or release, and loops only when they change), and
`TORE_REPLAY_SOUND_FILE=FILE cargo test --locked -p tore-app replay::sound -- --ignored --nocapture`
prints the same for a whole recording played at 1x, without a display.
Nobody has listened to a replay yet.

### Captures and timing

For checking the viewer without a keyboard:

```sh
tore-app --watch-replay FILE --capture-replay OUT.ppm --replay-tick N \
    [--flight-view 0..11] [--replay-aircraft ID] [--replay-drone] \
    [--replay-look-at aircraft:ID|ground:ID|weapon:ID] \
    [--replay-ui labels,timer,trails,marks,banner,comms,subtitles] [--replay-clean] \
    [--replay-menu '?'|pref|time|help|graphics|sound|controls]
```

The capture waits for the background pass, draws the frame at tick `N`
with the interface as chosen, writes a PPM like `--capture-flight` and
exits. A path ending in `.png` instead saves the 3D view without the
interface through the same code and PNG writer as P. `--replay-clean`
starts with the interface hidden, as H hides it. `--replay-look-at` starts
in the [object view](#object-view) from the selected aircraft toward that
aircraft, ground object or weapon (a weapon by its projectile id, as the
debug log's launch events give it).
`--flight-view 13` starts in the [HUD camera](#hud-camera) and `14` in the
[Sensor View](#sensor-view); with `--capture-replay OUT.ppm` the capture then
saves the tape's picture (the world, the HUD and the tape effects, without
the transport bar and labels) instead of the plain 3D view.
`--replay-speed S` starts playing at a ladder speed, negative for reverse,
and with `TORE_PERF_FRAMES=N` the viewer reports frame timings like live
flight. `--replay-panels thought,telemetry,guidance,comms,menu` opens
[debug panels](#debug-panels), or the right-click menu, on the selected
aircraft for the capture. `--replay-menu PAGE` opens the
[pause menu](#pause-menu) at its `?` tab, Pref, Pref > Time or the keyboard
help, or opens Graphics, Sound or the controls screen over it; `?` needs quoting in most shells.

Measured on the development Mac (Apple M3, 1440x1080 view, release build,
other work running on the machine) with a synthetic ten-minute recording of
17 aircraft, trails and labels on: 33 ms a frame at 16x forwards, 36 ms at
16x backwards and 39 ms at 1x, against 38 ms for live free flight measured
at the same time. The GPU dominates: rebuilding the moment took 6 to 7 ms of
each frame, most of it the same aircraft drawing live flight does, and the
interface about 1 ms. While the weather snapshots are first being built,
about 1,200 ticks a frame, frames take about 2 ms more.

### Known limits

- No cockpit or instruments and no music yet (the HUD only in the HUD
  camera), and the keys cannot be rebound. Replay sound has not been checked
  by ear.
- A recording from before `aircraft.hud` and `aircraft.sight` (nr-replayhud)
  still plays. Its HUD camera keeps the derived box and pipper, and its
  Sensor View keeps the derived line of sight and the `LOS FROM` note.
  The picture mode is the viewer's choice either way. Radar contact dots
  are present in format 4 cockpit samples. Another player's HUD is not in a client capture (the
  wire carries the local seat's readout only); the host's recording has
  every ownship. An AI aircraft's hit percent and firing band are not
  estimated, and an AI AC-130's gun trains are not recorded, so its pippers
  stay empty.
- An aircraft recorded without its PT name (the air-to-ground driver's
  `--buddy` runs and its A-10 runs record the player's type by name only)
  is known by its exact type name since nr-replayviews (agent fix,
  `convert::by_label`), so it draws as itself and its sight is found; an
  older viewer drew it as an F/A-18D.
- Checked by eye with synthetic recordings and headless AI probe
  recordings; a recording of a flight flown by hand has not been watched
  yet.
- Gun rounds have no trails.
- The object view's ground objects are the recorded world's objects
  (Ukraine has 257 before a ground target adds its own), so after the
  aircraft and weapons O steps through all of them before it wraps round. It
  has no look-around, as flight's relation views have none.
- The game keeps the newest 64 explosions and drops the oldest when a 65th
  starts, even if it is still playing; a replay keeps every explosion that is
  still playing and shows the newest 64. A burst of more than 64 at once (a
  Mk 82 splashing a whole airfield) therefore leaves a replay one older
  explosion on screen than the game had for the next few seconds, and
  `--verify-render` of such a flight reports it as `effects live, recorded`.
- Seeking to a moment shows no muzzle flash or launch flash for a shot fired
  on that very tick (a flash needs the tick before to compare), and a flak
  burst or wreck column that began before the playhead is rebuilt as flight
  had it.
- A ground target's wreck fires and their smoke come back from the recorded
  craters, fires, hit points and the registry's hit points. A unit the
  recording never named (one that never fired and was not destroyed) is not
  in the registry, and a recording of a world from before format 3 has none
  of this: its columns stay where the scenery put them.
- A burning city fill building's smoke column is computed from the tick
  its building fell on (nr-fx4; it ran on the viewer's own clock before),
  so after any seek every burning block shows the column it would have had
  playing straight through, at the same height and density. The same holds
  for a destroyed unit's wreck column (from its recorded death tick) and
  for crash-site, building, vehicle and hulk fire columns (puffs rebuilt
  from their recorded release ticks). Where a recording has no tick to
  start from, the column starts warm: a unit that was already dead when the
  recording began (its column starts at the first recorded tick), and a
  building that was already flat when a network seat joined (it stands flat
  from the replay's first tick, so a seek within the first minute after
  that shows a young column the live game would have shown grown). A flame
  still flickers on the viewer's clock, so two stills of one tick differ in
  about a hundred flame pixels.

## Debug panels

Right-click an aircraft or a missile, in a replay or in live flight, to see
why it does what it does: an AI aircraft's thinking, any aircraft's
flight-model telemetry, a missile's guidance, and every radio call, order
and sound with its outcome and reason. This is an **opinionated addition
requested by John on 2026-09-26**. The panels' layout, keys, colours and
wording, the menu's items and the pick distance are agent decisions
(2026-09-26).

### Opening them

- **In a replay:** right-click an aircraft, a missile or a name label: press
  and release the right button without moving more than 4 pixels. Dragging
  with it still looks around. Keys act on the selected aircraft: I its AI
  thinking, F its telemetry, G the guidance of its newest missile in flight
  (each again to close), C the Comms panel, M the right-click menu, X closes
  every panel.
- **In live flight:** Escape, then Pref, then **Debug panels?** (off by
  default, saved with the other flight preferences). While it is on, the
  mission timer shows at the top of the view and a right-click without
  dragging opens the same menu; a right-drag is still mouse look when mouse
  look is on (Controls, Mouse tab). With mouse look off and the right button
  bound to an action, the button keeps its binding and the menu does not
  open from the mouse. Letter keys stay flight keys, so the panels open from
  the menu. The flight carries on while the menu is open: the menu's keys
  (the arrows, Tab, Home, End, PageUp, PageDown, Enter, Space and Esc) work
  it until it closes, and every other key still flies. Opening the Escape
  menu closes it.

### The right-click menu

The menu picks through the camera that drew the frame: the aircraft or
missile drawn nearest the pointer, within 14 interface pixels scaled with
the view (about 32 pixels on a 1080-line view), or the aircraft whose name
label is under the pointer. It is the same on any window shape, windowed,
fullscreen or letterboxed. On a panel it is the panel's aircraft or
missile.

| Right-click on | Items |
| --- | --- |
| An aircraft | Follow (chase view): F10 on it. Cockpit view: the front view from it, the player's own cockpit in live flight. Drone here (replay): the follow drone beside it. View from here and Look at this (replay): the [object view](#object-view) from it or toward it. AI thinking (aircraft the AI flies). Telemetry. Comms for this aircraft. Name labels. Flight path trails (replay) |
| A missile | Guidance. Drone here (replay): a free drone beside it. View from here and Look at this (replay). Go to the shooter. Name labels. Flight path trails (replay) |
| A ground object (replay) | View from here and Look at this. Name labels. Flight path trails |
| Empty space | Every aircraft in the picture, grouped by side and wing, to jump to: a replay selects it, live flight puts the chase view on it. Then the switches |

Up, Down and Tab (Shift+Tab back), Home, End, PageUp and PageDown move;
Enter, Space or Right choose; Esc or Left close. The pointer moves the
highlight, a press and release on the same item chooses it, a press outside
closes the menu, and the wheel scrolls a long list.

### The panels

Up to two panels show at once, one on each side, plus the Comms panel along
the bottom. They end above the band the seven cockpit message lines can
fill, in the HUD's font: over the window's edge in flight, over the
transport bar in a replay, so the messages never print over a panel. A new panel takes a free side, or the side whose unpinned panel
is older; with both pinned it says so and opens nothing. Each has **Pin**
and a close button (x) in its title. An unpinned AI thinking or Telemetry
panel follows the selected aircraft (in live flight, the aircraft the
camera is on); a pinned one stays on its aircraft; a Guidance panel always
stays on its missile. The mouse wheel over a panel scrolls it.

A tree panel draws one display tree as an indented list: each label, its
value with its unit in a column beside it, and the value's "because" line
beneath in grey. Numbers read to the precision a pilot would use: whole
feet, pounds and knots with thousands separators, tenths of a mile, degree
and percent, hundredths of a G, a second and a ratio (`x0.84`). Aircraft
named in a tree read as their labels. The line under the title says when
the sample was taken; in a replay it is the latest one at or before the
playhead, and one older than 2 seconds (an aircraft that has gone) shows in
amber.

| Panel | Tree | Shows |
| --- | --- | --- |
| AI thinking | `ai.thought` | What an AI aircraft considered: mission, activity, target and why, geometry, weapon and whether it may fire, defense, motion, steering, controls and fuel. See the [AI thinking record](#ai-thinking-record) |
| Telemetry | `flight.telemetry` | Air data, load, power, drag, and each effect the flight model applied with its because line |
| Guidance | `weapon.guidance` | A guided weapon's seeker and steering |

### The Comms panel

Every comms and audio entry up to the playhead, newest at the bottom, whole
rows only. Entries that share a message number make one row, shown once
with its latest outcome: a line queued and then delivered, dropped or cut
off, or an order, request or report and each recipient's answer. A row
sits where its newest entry is, so a line that waited in the queue appears
when it was said, and before that moment it reads as queued. Each row
gives the time, the kind (RADIO, CREW, TOWER, HUD, ORDER, REQUEST, REPORT,
ANSWER, TONE, STALL, MUSIC, EFFECT, RELEASE, EJECT, DEVICE, and for the data
link LOCK, UNLOCK, ASSIGN, CLEAR, ACK, SORT), who said what
to whom, and the outcome in brackets: green when it went out or was acted
on, amber while it waits or when some recipients took it and some did not,
red when it was held back, dropped, refused or cut off, grey when there was
nothing to act on (a check that found nothing to say, or a line said on a
radio the player does not hear). Beneath come the reasons, the trigger,
the rolls, the wait, the line's earlier states with their times, anything
else recorded with it, and one line per recipient with its answer and
why. The same sound repeated within half a second, such as a gun burst's
release sounds, is one row with its count. An entry without a message
number, such as a cockpit message or a tone, is a row of its own.

The chips filter by kind: **Radio**; **Orders** (orders, requests, reports
and each recipient's answer); **Tower**; **Crew** (crew remarks and cockpit
messages); **Tones** (seeker tones, warnings, music and sound effects); **Link** (the
data link's locks, assignments and sort warnings). The
aircraft chip steps through the aircraft, keeping the entries it sent,
received or is named in; "Comms for this aircraft" sets it. The filters are
kept when the panel closes. The list follows the playhead in either
direction at any speed; the wheel scrolls back, a note says so, and wheeling
down to the newest entry follows again.

### Where the data comes from

- **A replay** reads trees and entries from the recording at the playhead.
  Trees are decoded one chunk (a second) at a time and the last six chunks
  stay decoded, so playing either way decodes each second once. Everything
  shown depends only on the playhead, so playing backwards shows what
  playing forwards showed.
- **Live flight** shows what the mission recording writes, as it writes it:
  while the panels show, the recorder hands them each display tree it
  builds and every comms and audio entry (the newest 512). The trees come
  from the same builders, fed with the same records at the same moments,
  including the history a tree reads (when an activity began and why, the
  recent changes, a missile's closest approach), so a live panel shows
  exactly what a replay of the flight will show, a tenth of a second or so
  behind for an AI aircraft's thinking. Names come from the roster a
  recording registers. With recording off (`TORE_RECORD_MISSIONS=0`, and
  captures, which do not record unless it is `1`) the panels say so and
  stay empty.
- A recording made before display trees were recorded has none, and its
  panels say that nothing was recorded.

The panels only read. Behaviour probes give identical output with the
panels' code in place, and nothing a panel or the menu does reaches the
simulation: the menu moves the camera and opens panels.

### Captures

`--replay-panels thought,telemetry,guidance,comms,menu` opens panels, or the
menu, on the selected aircraft for `--capture-replay` with a `.ppm` path (a
`.png` capture is the clean view, without any interface). For flight,
`--debug-panels` turns the Pref row on for one run and `--flight-panels`
with the same list also opens them for `--capture-flight`: AI thinking and
the menu on the first aircraft the AI flies, telemetry on the player,
guidance on the newest missile in flight, and the Comms panel. Captures do
not record, so they show trees and entries only with
`TORE_RECORD_MISSIONS=1`, and a capture's single frame comes before the
first tick has been recorded.

### Known limits

- The panels sit in the 640x480 interface layer, centred and scaled with the
  view, so on a wide window the sides of the view stay free.
- Live flight shows no drone or trails, and the panels open from the menu
  only.
- Radio calls in recordings made before the communication journal was
  recorded name AI speakers by their words, not their aircraft, so the
  aircraft filter misses them.

### AI thinking record

Every AI aircraft writes down why it did what it did, every tick, so the AI
thinking panel, recordings and debug logs can give real reasons with real
numbers. The record is **write-only**: no decision ever reads it. It holds
copies of values the AI worked out anyway, plus explanations recomputed from
the same rules. The golden behaviour fingerprints prove the AI flies exactly
as it did before the record existed, including one test that reads every
record and drains the journal after every tick of two full synthetic
missions. What the record contains is an agent decision. The code lives in
`tore_sim::ai::thought`.

There are four parts.

**The controller record** (`Controller::trace()`) is what one aircraft's
decision loop considered on its latest tick. A paused or repeated tick leaves
it as it was.

| Part | What it says |
| --- | --- |
| Path | Whether the decision ran, or the aircraft was already destroyed. |
| Events | Hits, targets that disappeared, and every missile warning: the delay before the aircraft notices it, the tick it takes effect, and on arrival the countermeasure roll, the devices released, the reaction and whether it made a radio call. Then the most urgent reason and whether it restarted the aircraft's plan or let it carry on. |
| Fuel | Fuel, endurance, time to reach home, the fuel state and whether the aircraft is heading home. |
| Target | The previous target and how the new one was chosen: weapons held, the mission's choice (with why it was refused, if it was), kept because it is inside 20,000 ft, or ranked by distance and penalties with the winning score. |
| Geometry | Range, horizontal range and the angles to the target. |
| Weapons | Every carried store with the first rule it breaks: switched off, needs radar that is off, needs a sensor track, wrong kind of target, empty, or outside its envelope. Outside the envelope, every reason: too close, too far, too far off the nose, outside its zone. Usable stores carry their hit chance and score. Then the store chosen, whether the lock holds and which condition failed, exactly what the weapon service was told, its phase before and after, its outcome and its next deadline. |
| Motion | Which branch flew the aircraft: missile defense, rejoining a charge or patrol, searching along a bearing, an ordered approach or maneuver, investigating an old contact, formation (slot point, aim, speed, bank), carrying on with the current maneuver, not yet due for a new choice, or a new choice. Also the quarter-second clock and when the next choice is due. |
| Maneuver | For a new choice: going home, a missile reaction, no target, target straight above or below, evasion or the ordinary approach; the tactical situation, the quadrant (ahead or behind, facing or not) and its experience percentages, the choice, any fitted rule that stood in for an unknown one, the last-ditch move and the motion request. |
| Resolve | Where the pitch came from (asked for, or the fitted engagement pitch with its inputs), the speed, the pursuit offsets, steering point and regulated speed, and how the maneuver ends. |

Beside it, `Controller::draws()` lists every random draw of the tick: what
the draw decided (for example "best attack" or "countermeasure roll"), the
source line, the value, the threshold it was compared with and whether it
passed. `last_batch()`, `weapon_phase()`, `weapon_deadline()`,
`active_maneuver()` and `search_contact()` show the current state.

**The actor record** (`AiActor::trace()`) is what the mission decided around
the controller on its latest tick.

| Part | What it says |
| --- | --- |
| Path | Destroyed, a training target, a takeoff or landing sequence, or the ordinary decision. |
| Airfield gates | Turn to go, runway free, earlier wing members down, a free parking slot, and whether a joining wingman's leader is landing. |
| Ejection | The hazard found and seconds to impact, whether a takeoff or landing phase judged it, whether it was a catastrophe or earned a go-around, and whether the pilot ejected. |
| Airfield | Whether a landing started, why the aircraft left a sequence (its leader stopped landing, or a missile threat), and what the sequence saw and commanded. |
| Dropped warnings | Missile warnings thrown away while taking off or landing, or by a training target. |
| Forgotten attacks | Remembered attacks dropped after 240 ticks without news, or because the attacker is gone. |
| Targets and stores | What the aircraft's own sensors allowed it to target, its stores' view of the target, and which aircraft the stores were aimed at. |
| Engagement | Whether it is holding formation, the role and stance in force, each remembered attack and why any is ignored, the choice, and a ranking of up to 16 aircraft: why any cannot be a target, its priority, its distance and penalties, and its score. |
| Rejoin and search | An escort flying back inside its leash or a patrol returning to its region, a bearing to search along, and an old contact to investigate. |
| Bingo | A bingo landing ordered this tick. |
| Controls | The maneuver flown, the terrain floor, the steering adapter's whole output (the controls, its fallbacks and the attitude it asked for) and the attitude and speed the flight model delivered. |

`AiActor::route_draws()` lists the draws of the private route home. In the
app, `AiWings::decoy_rolls()` lists each missile's roll against a released
decoy in the latest step, with its draw and threshold (the bridge clears
its decoy generator's draw log at the start of each step, which never
touches the generator's state), `AiWings::decoy_draws()` the draws
themselves, and `AiWings::station_weapon()` the weapon on a station.

**Missile defense** (`AiActor::defense_decision()`) now also explains itself:
the maneuver the evidence calls for (notch or jink) and its heading and
pitch even when it is not flown yet, whether a dive is safe, and each reason
behind maneuvering or releasing chaff and flares now.

**The message journal** (`AiMission::take_journal()`, or
`AiWings::take_ai_journal()` in the app) lists the messages between
aircraft. The host drains it once per tick; if it is never drained it keeps
the latest 4,096 entries and counts the rest. Each entry has the tick, the
sender, the content and every recipient's outcome with its reason.

| Message | Outcomes |
| --- | --- |
| Attack evidence, from the aircraft that saw the attack to itself, its wing leader and its escorts | Queued for the next tick, delivered, ignored (with why: a report about someone else, an unknown reporter, a recipient that left, holding formation since a recall, or a projectile already known at the recall) and forgotten after 240 ticks. A refresh of evidence already held is not a new message. |
| A neutral leader releasing its wing after a perceived attack | The attack that triggered it and the leader's own outcome. |
| Wing orders over the wing channel (break, approach, spacing, formation, wing control, target assignment, land) | Each recipient's outcome: applied, rejected with the reason, accepted without motion, or motion installed. |
| An escort's target or priority changing | Journaled once per change. |
| Missile warnings, sent by the launcher | Due at a tick, received with the reaction, or dropped with the reason. |

What cannot be explained yet:

- The record explains this game's rules, not the original's. Where the
  original's rule is unknown, the record names the fitted rule that stood in
  for it (see [behavior provenance](behavior-provenance.md)).
- The weapon lock is a simple host test (target ahead, not behind terrain),
  so a lock failure can say only which of those failed.
- The formation guidance explains itself only through its phase and trace;
  its internal plan is not recorded.
- The ejection monitor's own once-a-second roll is inside the ejection
  code, so the record says the pilot ejected but not the roll.
- Player orders, radio calls and cockpit voices belong to the
  [communication journal](#communication-journal), not to this record.
- The flight model's reasons are a separate record; this one stops at the
  controls the flight model received.

## Communication journal

Every radio call, wingman reply, crew remark, tower line, AI text line and
player order is written to a journal as it happens: when, who said it to
whom, the words and recordings, what triggered it, any random roll it used
with its threshold, and what became of it, with the reason. Lines that were
never said are kept too, with the rule that held them back. The journal is
**write-only**: no rule reads it and writing it never draws a random number,
so what is said and when is the same with or without it. Tests run the same
scripted flight twice, draining the journal every tick and never, and hear
the same lines at the same ticks with the same rolls left over. The shape of
the records is an agent decision (2026-09-26).

The code is `tore-world/src/comms/journal.rs`. The mission recorder drains
it every tick and records each entry as an event
([communication events](#communication-events)); a flight that is not
recorded never drains it, and it simply stays within its bound.

### What an entry holds

| Field | Meaning |
| --- | --- |
| Time | Simulation seconds on the producer's clock; the recorder stamps its own tick |
| Call number | Shared by every entry about one line: queued, then delivered, dropped, cancelled or cut off |
| Source | `RADIO`, `REPLY` (a wingman answering the player's order), `CREW`, `TOWER`, `HUD` (the AI's text lines), `ORDER`, `CHATTER` (an AI radio event before it becomes a call) or `MUSIC` |
| Speaker and audience | The aircraft id (the recording's player, plane 0 in single player), the name as printed (`Red two`, `RIO`, `YOU`), and who it was addressed to: the cockpit, the flight leader, the flight, the player, the airport frequency, or the wingmen an order addressed |
| Words | The text, the recording stems, the route (radio, airport or direct) and whether radio silence may drop it |
| Trigger | A typed cause with its numbers, for example "infrared missile release at aircraft 3", "hit by aircraft 5 (aircraft, gun rounds)", "fuel state bingo", "situation Defensive, target 5000 ft: break" |
| Rolls | Each draw in the order made: "roll 37 < 50: Fox call", "roll 12 < 40: Splash one with the type name", "roll 13 mod 8 = 5: hit call", including draws a held call still used up |
| Outcome and reason | See the next table |

| Outcome | Meaning |
| --- | --- |
| `queued` | Waiting in the channel until it is due, or in the tower's own queue until it expires |
| `delivered` | Printed and played, with the seconds it waited; a tower notice also carries when it entered the tower's queue. An AI text line: shown on the HUD |
| `dropped` | Radio silence dropped routine chatter when it was sent, or a full queue pushed out its oldest line |
| `suppressed` | A rule held it back, with the time left on a cooldown or limit |
| `unheard` | Said, but the player's radio does not receive it: another flight, an enemy flight, the player is down, or the speaker has no radio identity |
| `replaced` | A newer notice about the same thing took its place, or an unread AI text line was overwritten |
| `expired` | A tower notice older than 15 seconds |
| `cancelled` | Taken off a queue: the tower answered the player's request, the player's aircraft was lost, the runway became unusable, the player left the approach, the wingman went down, or the aircraft entered an airfield sequence or left formation |
| `interrupted` | Delivered, then cut off by the player's wing order voice |
| `answered` | An order's answers, one per addressed wingman, and who replied |
| `rejected` | An order nobody received, and why |
| `silent` | A crew coaching check that found nothing to say, with the rule and the rolls it drew |
| `noted` | A state change: the crew's comment gate, or the music's inputs |

Shared names follow `tore_replay::vocab::outcome`; `unheard`, `interrupted`,
`answered`, `silent` and `noted` are the journal's own.

### Reasons, by producer

- **The channel.** Radio silence, the 64-call queue, the wait before
  delivery, airport calls cancelled, and lines cut off by a wing order
  voice.
- **Radio calls.** The listener rule; a release with no target; the 4 s bomb
  and gun cooldowns; unguided hits limited to one per shooter every 8 s and
  one every 4 s overall; "I'm hit" from gun rounds once per aircraft every
  8 s; ground kills after a bomb kill (4 s); friendly fire beyond 52,800 ft
  and its 6 s cooldown. Rolls: the Fox chance under 50, "Splash one" at 40
  or more, contact size words, and every variant.
- **Crew.** Changes in whether the crew may comment (aircraft lost, the
  eject warning, radio silence, not in free flight, or why nobody can coach
  a single-seat player: no wingman, wingman down, a different target, or
  beyond 15,000 ft). Each coaching check: the situation, the previous one,
  the range, the rule that chose the line or why none did, the next check
  time and the rolls. G strain and the -1 G crossing count, fuel states,
  and missile warnings, including a missile held back by the shared 6 s
  limit, which is then never called.
- **Tower.** Each trigger (runway free or occupied, airborne, climb-out,
  landing clearance, wind, landing score, welcome, a wingman's airfield
  phase or go-around), the 15 s lifetime, the 24-notice queue, and
  notices coalesced by aircraft and topic.
- **AI chatter.** Why a new target makes no contact report: only the first
  two aircraft of a flight report, the 15 s cooldown, never the same target
  twice, the 20 s block after accepting an attack order, or a target that is
  not a living airborne aircraft. Events pushed out of the 64-event queue.
- **Player orders.** Every addressed wingman's answer: applied (with or
  without motion), rejected with the receiver's reason, rejected because its
  sensors cannot see the target, or skipped (bugged out, flown by a human,
  already landed, taking off or landing, no base); the silent side orders
  with their outcomes; a refusal before anyone received it (no wingmen,
  all bugged out, no valid hostile target, no airport selected); and which
  wingman replied and with what, or why nobody did.
- **AI text lines.** Formation reports queued, held by the 10 s limit per
  aircraft, replaced, pushed out of the 16-report queue, cancelled, and
  shown; the activity line held by its 2 s limit, overwritten unread, and
  shown.
- **Situation music.** When an input changes: the score the inputs ask for
  and why each input is on (the designated enemy inside or beyond
  40,000 ft, a hit within 30 s, an AI aircraft aiming at the player within
  4 s, missiles guided at the player, success, home, deck, takeoff,
  ejection).

### Repeats and bounds

A gun fires one release per round, so a rule is listed once per window:
once per cooldown or limit for each aircraft, and at most every 4 s for a
rule without one. Crew gates are listed when they change; the 3 s channel
hold is not listed as a gate, because every delivered line shows it. At
most 1,024 entries wait between drains; a host that never drains keeps the
newest and counts the rest as lost.

### Not visible yet

- **The mixer's own decisions** inside the audio device: speech dropped
  because its queue is full or sound effects are off or paused, when a
  delivered line actually starts after the lines ahead of it, and which
  situation score really plays (its lockout, the once-per-flight scores,
  the Valkyries toggle and failed-load retries). The journal records what
  the inputs ask for.
- **The order voice without audio.** With no audio device nothing is cut
  off, so no line is marked `interrupted`.
- **Headless AI probes** run no crew voice, music or HUD delivery.

### Draining and host hooks

- Drain with `Comms::take_journal` once a tick. `radio_calls::step` moves
  the wing's journal (`AiWings::take_journal`) into the channel's each
  tick, so one call gets everything except the music's entries, which come
  in `flight_music::Step::journal`.
- `Entry::describe` prints one plain-English line; `Cause`, `Reason` and
  `Outcome` print their own text for event fields.
- Host hooks, all in place in `main.rs`:
  `Comms::cut_off(now, Reason::OrderVoice)` where the order voice
  interrupts wing speech, `Comms::cancel_airport` and `Entry::tower_reply`
  for tower replies, `Entry::clearance_cancelled` where a destroyed runway
  under the landing clearance cuts tower speech, `Entry::order_refused` for
  orders refused before delivery (no AI wing, no landing site, an error),
  and `Comms::send` from the mission core's result tracker for the mission result calls with their trigger.
  Every order the wing received is journaled with the `radio` route: the
  host plays its voice, even an empty one.
- The recorder drains it with `Recorder::drain_comms` after the tick's
  radio is delivered (live flight and `--record-mission` probes alike),
  notes entries its bound threw away, and takes the music's entries with
  `Recorder::comms`.

## Command line

These commands read recordings of what happened and need no game media,
except `--convert-capture`, which runs a client again and needs the import.
They are unrelated to `--record-input`, `--replay-input`, `--record-combat`
and `--replay-combat`, which store controls or combat inputs and simulate
them again.

| Command | What it does |
| --- | --- |
| `--recording-info FILE` | Prints who, where and when, the weather, length, size, settings, result, every aircraft and weapon, events by kind and any damage |
| `--recording-log FILE [--out DIR] [--from S] [--to S] [--ids 0,7] [--rate HZ]` | Writes `log.jsonl` and `summary.txt` to DIR (default: a `-log` folder beside the recording). `--from` and `--to` are mission seconds, `--ids` limits the log to those aircraft, `--rate` sets aircraft samples per second (default 1) |
| `--recording-acmi FILE [--out FILE] [--rate HZ] [--guns]` | Writes a Tacview file (default: `.txt.acmi` beside the recording), 10 samples a second by default; `--guns` adds gun rounds |
| `--recording-diff A B` | Prints how two recordings differ: header, identities, the first second their checksums differ, the first tick any aircraft's state differs, and event counts by family |
| `--convert-capture CAPTURE [--out REPLAY]` | Turns a networked flight's capture into a replay, beside the capture (or at REPLAY, which must not exist), one for each flight in it; needs the import the capture was made with. See [Network flights](#network-flights) |
| `--watch-replay FILE` | Opens the [viewer](#viewer) on a recording (this one needs the game media and a display); with `--capture-replay OUT.ppm --replay-tick N` it writes one frame and exits, see [captures](#captures-and-timing) |
| `--surface-objective THEATER STEM --record PATH [--verify-render] [--run-on] [--vulnerable]` | The ground target objective run ([development](DEVELOPMENT.md)) recorded as a format 3 file, with the same `--verify-render` check: its last line reads `surface-objective: verify-render: PASS ticks=... missing=0 differing=0`. `--run-on` keeps it running to `--seconds` after every target is down (trucks rearm), `--vulnerable` lets the defenses shoot the player down. The `replay-surface` battery scenario runs it |
| `--a2g-run ... --record PATH [--verify-render]` | The air-to-ground driver ([development](DEVELOPMENT.md)) recorded the same way, with the same check: its last line reads `a2g-run: verify-render: PASS ticks=... missing=0 differing=0`. The `replay-a2g` battery scenario runs it |
| `--ai-probe-ticks N --record-mission PATH [--verify-render]` | Records a headless AI probe to PATH (never overwritten) without changing its output. `--verify-render` then rebuilds every tick from the file, compares it with the picture the probe drew, and prints one line: `AI probe verify-render: PASS ticks=... missing=0 differing=0 device_ticks=...`, or the first difference; `device_ticks` counts the ticks with chaff or flares in the air, each checked against the devices the probe flew |

For example, after John says "look at the replay from 3:40 pm" (15:40 UTC
in the file name):

```sh
tore-app --recording-info replays/2026-09-26_1540_UKR_F18.tore-replay
tore-app --recording-log replays/2026-09-26_1540_UKR_F18.tore-replay --from 200 --to 260
```

[Development](DEVELOPMENT.md#mission-recordings-for-debugging) has the
headless workflow.

## For developers

- Write with `tore_replay::Writer` (`create`, `register_aircraft`,
  `register_weapon`, `register_surface_unit`, `push`, `finish`); a writer
  dropped without `finish` keeps its `.partial` file and flushes what it
  holds. `Writer::version` says which format the header's world chose
  (`World::needs_surface_format`); a format 2 writer refuses surface tracks,
  flak and the registry with a plain message.
- Read with `tore_replay::Recording` (`open`, `frame`, `frames`,
  `decode_chunk`, `tree`, `spawns`, `live_puffs`, `live_effects`, `events`).
  `Recording::peek` reads only the header, seek index and footer, for
  listings; an unfinished file peeks with no footer.
- In the app, `replay/recorder.rs` captures a flight (`start_tick`,
  `begin`, `note` and the other noting methods, `drain_comms`, `end`,
  `finish`), `replay/library.rs` owns the folder and auto-delete (`list`,
  `plan`, `cleanup`, `order_key`), `replay/screen.rs` is the Replays screen,
  and `replay/cli.rs` the command line, whose `log` and `acmi` the screen's
  export buttons call on a background thread. `Tick::journal` hands `begin`
  the AI message journal the host drained for the tick.
- `replay/recorder/surface.rs` records the surface world of a format 3
  recording from state the tick already computed: the routed units' poses
  (`RenderSnapshot.surface`), launcher and magazine changes
  (`SurfaceState.units[..].mounts`), the debris pieces, the registry (names
  from `Surface::unit` and `Combat::ground_name`, hit points and positions
  from the combat rows), and the events from the controllers' trace
  (`Trace::Shot`, `Phase`, `Rearm`, `Refill`, `Radar`, `Shutdown`) and from
  the surface rows whose hit points ran out. The aircraft's loss to a unit
  reads `it was shot down by an SA-6` from `outcome::shot_down_by`.
  `replay/tracks.rs` collects the launcher changes and the tick each unit died
  for the viewer (`stock_at`, `deaths`).
- `replay/recorder/why.rs` turns the AI's and the flight model's records
  into reason events and display trees, with its rates, triggers and
  debouncing; `replay/recorder/journal.rs` turns the two journals into
  events. `replay/trees.rs` holds the pure tree builders (`ai_thought`,
  `flight_telemetry`, `weapon_guidance`) and the shared wording
  (`effect_line`, `draw_text`, `service_text` and the labels), for a live
  panel as much as for the recorder. The recorder reads only through
  shared references; draining the two write-only journals is its one
  change, and a test flies the same synthetic mission with and without
  recording and finds the same flight, decision and weapon state.
- A network capture converts with `tore_session::client::convert` (`observe`,
  then `Conversion::write`); the game's `replay/net_convert.rs` names the file and
  builds the header's world, and `tore_session::fixture::bot_fight` (feature
  `test-support`) makes a synthetic capture for tests.
- Export with `tore_replay::export` (`write_jsonl`, `write_summary`,
  `detect`, `write_acmi`, `compare`, `write_diff`).
- Event kinds, field names, tree channels, well-known tree labels, units
  and outcomes live in `tore_replay::vocab`, with each kind's fields listed.
- The app converts between its per-tick `RenderSnapshot` and the recording
  in `tore-app/src/replay/convert.rs`: `aircraft_state`, `projectile_state`,
  `debris_states`, `escapee_state` and `EffectWatch` (effects are recorded
  once, when they start) on the way in; `snapshot` on the way out, which
  draws the same picture through the same helpers, with the effects
  `playing_effects` finds by replaying the simulation's 64-effect cap over
  the recorded starts (an effect the game cut short at the cap stays gone,
  even after others run out); `difference` names the
  first thing two snapshots disagree on beyond the precision above. The
  draw rules that hold for a whole flight (which models are loaded, which
  aircraft draw with them) are header extras, read with
  `Presentation::from_header`. Ground objects are not aircraft: only their
  hit points are recorded, except the units that follow a route (poses), the
  launchers (rails) and a parked aircraft's debris piece (format 3).
- `replay::identity::terrain` rebuilds the recorded world from the header's
  resolved settings (layout, weather choice and layer, start time, wind,
  cloud deck and, in format 3, the ground target, the airfield scene and the
  city fill scene) without reading `TORE_WEATHER_TIME`, `TORE_WIND`,
  `TORE_CLOUD_ALTITUDE`, `TORE_REDRAWN_AIRPORTS` or `TORE_CITY_FILL`;
  `replay::identity::of` captures them from a live terrain
  (`RETAIL_AIRFIELD_SCENE` 0, `REDRAWN_AIRFIELD_SCENE` 1,
  `NO_CITY_FILL_SCENE` 0, `CITY_FILL_SCENE` 1). The viewer then builds its `Scenery` from that terrain.
- The viewer lives in `tore-app/src/replay/`: `viewer.rs` (the screen:
  loading, cameras, keys, drawing), `host.rs` (the `Screen::Replay`
  plumbing in the app), `panels.rs` (the debug panels and the chunk cache
  of recorded trees), `context_menu.rs` (the right-click menu, picking and
  the click-or-drag rule), `live.rs` (the panels and menu in live flight), `playback.rs` (any tick's picture, smoke and wing
  vapor), `clock.rs` (playhead, speeds, steps, markers), `tracks.rs` (the
  background pass: trail samples, building hit points, the player's path
  for the weather), `weather.rs` (weather snapshots), `sound.rs` (which
  recorded sounds play and when, as plain-data cues), `drone.rs`,
  `trails.rs`, `overlay.rs` (the interface and its pointer handling) and
  `png.rs`. Tests use a synthetic recording (`replay/fixture.rs`); a longer
  demonstration recording over the imported Ukraine map, for looking at the
  viewer by eye, is written by
  `TORE_REPLAY_DEMO=FILE cargo test --locked -p tore-app replay::demo -- --ignored`
  (`TORE_REPLAY_DEMO_MINUTES` and `TORE_REPLAY_DEMO_AIRCRAFT` size it).
- Tests use synthetic recordings only; golden outputs live in
  `crates/tore-replay/tests/golden` and are rewritten with
  `TORE_UPDATE_GOLDEN=1 cargo test --locked -p tore-replay`.

## Promo reel capture

`--reel-render REPLAY FRAME_PLAN OUTPUT.mkv` renders exact replay ticks without
a window. Director-only HUD and cockpit views
use recorded aircraft data, source cockpit art and recorded target selection.
The gun pipper uses the existing ballistic solver with recorded visual target
observations. The director can also capture the actual replay transport,
timeline, labels and subtitles, with the declared playback speed and pause
state. A row's FOV drives the engine's cockpit zoom, which scales scene, HUD and
art together; an optional thirteenth column fixes the eye at a recorded tick.
`--hud-layer` also writes the visible HUD symbols as a lossless layer, and each
capture writes its speech as a stem beside the effects. `--reel-music DIR`
exports the imported score phrases. The [TOML reel recipe](../tools/reel/README.md)
drives these commands. Ordinary replay controls and recording behavior remain
unchanged.

For a still with the ordinary object-view camera and its banner, use
`--replay-stills REPLAY FRAME_PLAN OUT_DIR --look-at weapon:ID` (or
`aircraft:ID` / `ground:ID`). Use a plan row with view 3 and an aircraft
anchor that exists at the chosen tick. This offscreen command needs a GPU;
`--replay-banner-check` does not. It leaves existing director plans unchanged.

## Player feedback flags

On a dedicated server with feedback enabled, **Escape > ? > Feedback...**
opens a text form. It flags the moment the form opens, even if the player
spends time typing while the multiplayer flight continues. Submit records the
text, player, aircraft, opening tick and time, and submission tick and time.
The host records it and returns it to the sender's local replay capture.
Single player and player-hosted games do not show this button.

A recording containing feedback is flagged. Pink timeline markers show the
opening ticks. **Feedback (N)** in the viewer opens a list. Click a row to seek
there and read the text, or use Up/Down and Enter. PageUp/PageDown or the
mouse wheel scroll long reports. Escape closes the list.
The replay viewer cannot submit new feedback.

`--recording-log FILE --out FOLDER` now also writes `feedback.jsonl`, one JSON
object per flag. An unflagged recording produces an empty file. The export
contains the source file, tick, mission time in seconds, UTC wall-clock
milliseconds, submit tick and time, callsign, seat, aircraft, theater and text.
[The event layout](formats/replay-feedback.md) documents the fields and why
this remains compatible with replay format 4. The complete behavior is in
[the feedback specification](spec/replay-feedback.md).

Dedicated servers can save authoritative recordings with their mission logs.
Their disk worker never waits on the flight tick. Its 60-second unwritten
budget absorbs multi-second disk stalls; those buffered frames can be lost
in a crash. Exhausting that budget stops recording with a logged warning
and keeps a recoverable incomplete file;
[the server guide](DEDICATED-SERVER.md#recording-and-feedback) explains the
bounds, unchanged sync cadence and shutdown behavior.
They have no local cockpit display or heard radio track. See
[server recording and retention](DEDICATED-SERVER.md#recording-and-feedback).

## Human target selection

Human Enter and Apostrophe picks are recorded as explicit visual target identities
(protocol 71, 2026-10-23), with the resulting target state in mission recordings.
The replay viewer's keys retain their existing meanings. Combat tapes accept
`designate-visual-id:ID`; older `designate`, `designate-previous`,
`designate-visual` and `designate-id:ID` commands retain their legacy semantics.
Old mission recordings remain readable; replay container and pose layouts did
not change. Network captures still require their matching protocol build for
conversion. New sensor checkpoints require the matching protocol. See
[target selection](spec/radar.md#target-selection-keys).

HUD range samples carry optional `min_ft` for the contiguous Energy hit region advised at the sampled range. Older samples without it retain their decoded minimum fallback. `max_ft`, percentage and IN RNG describe the same advised region.
