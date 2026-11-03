# Radar warning receiver presentation

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

## Scope

This specification defines the cockpit RWR presentation and its warning
tones. The simulation owns
emitter reception, missile observation and threat classification. The
instrument displays one actor-owned snapshot and cannot create or alter threat
knowledge.

The RWR retains its original frame, crosshair, two range rings, own-aircraft
center mark and `JAM` label. The top of the scope is directly ahead, the bottom
is behind, the right is 90 degrees and the left is 270 degrees. Maximum
reception remains subject to installed equipment and simulation rules.

## Range

The RWR and the radar scope share one range setting, stepped along the radar
ladder of 5, 10, 25, 50, 100 and 150 nautical miles described in the
[radar specification](radar.md#scope-range-and-targeting-modes). The RWR scale
is that setting capped at 50 nautical miles: it reads 5, 10, 25 or 50 in step
with the radar, and stays at 50 while the radar is at 100 or 150. The
upper-right label, the plotting scale and the out-of-scale markers all use this
capped value. The default setting is 10, so with no saved preference both
instruments read 10 at flight start. Normal sessions save and restore the
shared setting with the other display preferences.

The RWR window's `-` and `+` buttons, the radar window's range buttons and the
keyboard scope range keys (unless the RCS page is the most recently opened
instrument) all step the shared setting one radar ladder position, stopping at
5 and 150. The RWR buttons therefore also change the radar range. Above 50 a
press moves only the radar: `+` at 50 selects 100 with the RWR still at 50, and
`-` at 150 selects 100 with no visible RWR change.

## Symbols and warnings

Draw a ground emitter as a square. Draw a friendly aircraft as an outlined
diamond and an enemy aircraft as a filled diamond. Draw a missile with a dot.
An emitter painting this aircraft is bright. An emitter known to be tracking or
firing at this aircraft flashes. These states require supplied evidence. A
passively received emitter with no lock evidence remains in its ordinary state.
A current supported-radar incoming warning marks its source as firing only when
exactly one independently identified emitter lies within 2 degrees of the
supporting-radar bearing. Ambiguous and unidentified sources remain steady.
Prelaunch painting/tracking states have no fabricated producer.

Show `R` in the lower-right corner for radar warning state and `I` beside it for
infrared warning state. An ordinary detection is dim, a seeker tracking this
aircraft is bright, and an incoming missile warning flashes. `R` describes
radar guidance and `I` describes infrared guidance. Unknown or passive guidance
does not receive a false guidance label.

A missile known from this receiver's evidence to threaten this aircraft
flashes. Other detected missiles remain steady, including friendly missiles and
enemy missiles whose target is unknown. Threat state is receiver-specific. A
missile may therefore flash for one aircraft and remain steady or absent for
another.

The flash phase uses simulation time. Symbols are visible for 60 ticks and
hidden for 60 ticks at 120 Hz, producing one complete cycle per second. Pause
freezes the phase. Render frequency does not affect it.

## Plotting and lifecycle

Plot each contact at its heading-relative bearing. When measured range is
available and falls within the selected scale, radial position is proportional
to that range. A bearing-only observation appears as a short radial tick at the
outer ring. A ranged contact beyond the selected scale appears as a forked
clipped marker at the outer ring. These markers are deliberately distinct and
neither implies a false in-range position.

During the two-second lost-observation grace, retain the last permitted bearing
and range as a dim, steady marker. Stop flashing as soon as current targeting
evidence ends. Remove the marker when the shared threat record expires or the
missile is destroyed, impacts or otherwise leaves its lifecycle.

The RWR window being closed or set to a shorter scale does not affect simulation
knowledge. A failed RWR panel draws no electronic presentation. Receiver failure
does not erase a missile independently seen by a pilot or AI, although the
failed panel cannot present that observation. A failed receiver does not
silence the [warning tones](#warning-tones).

## Warning tones

The RWR has three recordings in the user's FA_2.LIB. At most one warning tone
sounds at a time. It starts from the beginning of its recording when its
condition starts, repeats the recording back to back while the condition
holds, and stops the moment the condition ends. The highest row that applies
wins:

| Rank | Condition | Recording | Recording length | What it sounds like |
| ---: | --- | --- | ---: | --- |
| 1 | A radar-guided missile is in flight with the player's aircraft as its target | `&RWRLOCK.5K` | 1.47 s | Fast, uneven beeps, about 0.25 s apart |
| 2 | An infrared-guided missile is in flight with the player's aircraft as its target | `&RWRLOCK.5K` | 1.47 s | Same recording as rank 1 |
| 3 | An enemy holds a radar-guided missile lock on the player's aircraft | `&RWRDTCT.5K` | 3.18 s | Slow beep groups, about 0.8 s apart |
| 4 | An enemy holds an infrared-guided missile lock on the player's aircraft | `&RWRIR.5K` | 1.07 s | Six even beeps, about 0.18 s apart |

- **Guidance.** Radar-guided means seeker class 3 and infrared-guided means
  class 2, the classes the
  [cockpit voice warnings](cockpit-voice.md#missile-warnings) use. Missiles of
  any other class never sound a tone.
- **Missile in flight.** The same count that drives the
  [flight music](flight-music.md) danger rule: a missile joins or leaves it the
  moment it acquires or drops the player as its target, and the count is
  rebuilt every 2 seconds. An AIM-120 more than 30,380 ft (5.0 nm) from the
  player is not counted, so its tone begins when it closes inside that range.
  A missile decoyed onto chaff or a flare no longer targets the player and
  stops sounding.
- **Lock.** An AI aircraft or ground unit whose target is the player, with a
  class 2 or 3 missile selected, has passed its seeker lock check with a clear
  line of sight and is waiting out the weapon's tracking delay or firing. Each
  refresh keeps the warning alive for four quarter-second clock steps, so the
  tone ends 0.75 to 1 second after the last refresh. Searching and preparing
  before lock sound nothing.
- **Changing condition.** Moving to another row stops the old tone and starts
  the new one from its beginning, even between ranks 1 and 2, which share a
  recording.
- **Own seeker tone.** While any warning tone sounds, the player's own seeker
  tones (the IR growl and the radar tracking and lock tones in the
  [sound spec](sound.md)) are silent. They start again from the beginning
  when the warning ends.
- **`&RWRMISS.5K`** is in the archive but the reviewed build never plays it.

| Level component | Value |
| --- | --- |
| Base level | 200 on the 0 to 255 sound scale |
| RWR slider | Multiplies by its percentage; default 50 |
| Overall slider | Multiplies by its percentage **twice**; default 75 |
| Arithmetic | `200 * RWR / 100`, then `* Overall / 100`, then `* Overall / 100`, dropping fractions at each step |
| Level at defaults | 56 |
| Level at RWR 100, Overall 100 | 200 |
| Level at RWR 100, Overall 75 | 112 |
| RWR slider at 0 | Silent |
| Slider change while sounding | Applies at once |
| Distance, direction, view | None: same level, centered in both channels, in cockpit and external views |

The tones follow the player's own aircraft, whichever aircraft the view shows,
and need that aircraft to exist and be active. They do not depend on the
receiver being installed or working, on the RWR window being open or on its
range, or on the threat appearing on the scope.

Pause mutes every effect, the tone included. Game time stops, so no warning
expires while paused, and a tone whose condition still holds is heard again on
resume. The fade that accompanies the `&HRTBEAT.11K` heartbeat applies to all
effects and also fades this tone, down to silence at its strongest.

The manual describes one tone for radar and one for infrared, slow when a
seeker tracks you and fast when a missile is inbound. The executable is the
authority here: infrared lock uses a fast recording, and both inbound classes
share `&RWRLOCK.5K`.

### Mapping to TORE's RWR states

| Tone condition | Existing TORE state | Gap |
| --- | --- | --- |
| Ranks 1 and 2, missile in flight | The flight music danger rule already lists live projectiles marked incoming whose target is the player, with the AIM-120 rule. The RWR `Incoming` indicator is a different, receiver-evidence state with a stale grace, so it must not drive the tone. | Split that list by the weapon's seeker signature, 3 or 2. |
| Ranks 3 and 4, lock | The RWR `Tracking` indicator and `Painting` emitter state exist with no producer. The AI controller's weapon phase already reports `Tracking` (lock held, waiting the tracking delay) and `Fire`. | A per-actor feed: target is the player, phase `Tracking` or `Fire`, the seeker signature of the store being locked, held 1 s. TORE's AI keeps no selected station, per the flight music code. TORE's surface units run their own engagement controller ([surface defenses](surface-defenses.md#engagement)) and feed the same list. |
| Level, pause, centering | Host mixer and the Sound/Music Prefs RWR and Overall words | None beyond applying Overall twice. |

### Implementation in TORE

Implementation mode, 2026-09-28. `rwr_tone.rs` chooses the tone every fixed
step and the mixer loops it, centred, restarting on a change of tone.

| Component | Behaviour in TORE | Provenance |
| --- | --- | --- |
| Ranks 1 and 2 | Live projectiles marked incoming whose target is the player, by the weapon's seeker class, with the AIM-120 rule, checked every step rather than every 2 seconds | spec-derived; step rate fitted |
| Energy inert stores | Incoming indications and rank 1/2 tones follow the shared [Energy defeat rule](missile-energy.md#motion); independent lock warnings remain. | opinionated, John's Phase 2.6 decision |
| Ranks 3 and 4 | AI aircraft on the enemy side whose target is the player, weapon phase tracking or firing, and whose chosen station carries a class 2 or 3 weapon; held four quarter-second clock steps past the last refresh | spec-derived |
| Level | 0.4 times the original's ratio of this tone to a full-level effect at the default settings, then RWR and OVERALL relative to their defaults, OVERALL twice | spec-derived ratio; absolute level fitted |
| Own seeker | Silent while a warning sounds; it restarts from the beginning afterwards | spec-derived |
| Ejection and death | No tone once the player has ejected or the pilot is dead | fitted, retail unknown |
| Ground units | A SAM launcher's or ship's missile controller whose target is the player and which holds its lock (tracking, firing, or between salvos) feeds the lock list with its missile's seeker class, so ranks 3 and 4 sound as for an AI aircraft; a SAM battery's lock is its radar's. A battery firing on its optical backup (radar dead) sounds no lock. Guns never do (no missile seeker class). | spec-derived (retail shares the weapons procedure, [RWR formats](../formats/rwr.md#lock-producer-stage)); the lock held between salvos defined (John, 2026-10-10) |
| Ground emitters | A surface radar on the air (see [surface defenses](surface-defenses.md#rwr-emitters-and-radar-state)) is received as the Ground square. The surface controllers also report which emitters paint the player (tracking or firing at it, held between salvos); the instrument draws their squares bright and steady. A networked client shows none until the wire carries the list (multiplayer slice). | spec-derived; painting display built (slice P1b, 2026-10-10) |
| Replays | The tone is not recorded, so a replay is silent here | fitted gap |

### Unknown

| Missing fact | Next research step |
| --- | --- |
| How often an AI refreshes the lock warning, so whether the rank 3 and 4 tone is continuous or can gap during a long lock | Trace the call rate of the AI weapons procedure per object |
| Whether a missile that hits or is destroyed stops its tone at once or up to 2 s later | Check every projectile removal path for a target clear |
| Whether remote human players' locks sound a tone in multiplayer | Review the network paths for writers of the lock warning |
| Whether tones stop when the player ejects or is destroyed | Trace the player's object and state after ejection and destruction |
| What drives the heartbeat fade | Sound or G-effects research on `&HRTBEAT.11K` |

## Provenance

The 1999 EA/Jane's *Fighters Anthology* manual at
`.local/missile-update/manual.pdf`, SHA-256
`1a082378a8e8cd163ed6b398efcc1df80b67c2f104f6b90ac0733c88d58e26c3`,
is the source for the scope orientation, range label, 50 NM maximum, `JAM`,
square, diamond and missile-dot symbols, bright tracking state, flashing firing
or missile state, and the `R` and `I` warning meanings. These appear on printed
pages 94 and 95, PDF pages 98 and 99. Printed page 131, PDF page 135,
distinguishes seeker tracking from an inbound missile and repeats that an
incoming radar missile flashes `R` and its dot, while an incoming infrared
missile flashes `I`.

The shared range with its 50-mile RWR cap is opinionated, requested by John on
2026-09-23, and matches retail screenshots he supplied in which both the RWR
and the radar read 10 at flight start. Stepping the full radar ladder from the
RWR buttons, so that presses above 50 change only the radar, is an agent
decision: it keeps one control rule for both windows instead of a separate RWR
step list.

The exact 60-tick visible and 60-tick hidden cadence, actor-owned threat feed,
bearing-only tick, forked out-of-scale marker, stale appearance and treatment of
unknown guidance are opinionated development rules shared with the
[air-to-air awareness specification](ai-awareness.md#shared-rwr-missile-information-and-display).
The manual establishes flashing but does not establish its exact cadence or
these missing-data presentations.

The warning tones are native: recovered by static disassembly of FA.EXE 1.02F,
SHA-256 `e31560c2a6d6adb4aa1493f0308f6ae5640f67a4e886dbdf5887489e6e99244c`,
with FA_2.LIB SHA-256
`fb8b30216e739292489d4872cc440debec334e14f8b9a3d0e340092445246198`.
Addresses and evidence are in the [RWR tone notes](../formats/rwr.md). The
described beep spacing is a rough measurement of the decoded recordings, a
listening aid rather than a decoded field. The manual's warning tone text is on
printed page 131, PDF page 135.

## Acceptance

Synthetic presentation tests cover the RWR scale at every radar setting,
including the 50-mile cap at 100 and 150, the RWR buttons stepping the shared
range, the 10-mile start, cardinal bearings, ranged, bearing-only and
out-of-scale plots, manual emitter shapes, steady and flashing
missiles, both phases at their exact tick boundaries, stale contacts, `R` and
`I` states, jammer state and receiver failure. Synthetic tone tests cover the
rank order, the start, loop and stop of each recording, the restart between
ranks 1 and 2, the 4-step lock hold at its boundary, silence of the player's
seeker tones during a warning, the level at the table's slider settings, and a
failed receiver still sounding. A display smoke test must also
confirm the assembled instrument with original runtime art and font assets.
