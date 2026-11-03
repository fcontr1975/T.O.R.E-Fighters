# Player countermeasures

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Energy rules use the shared [physical decoy and episode contract](missile-energy.md#countermeasure-contract).
The recovered controls, ammunition and dispenser schedule below remain; the
per-device fitted evaluation describes the preserved earlier weapon rules.
The hit-time jammer and damage asymmetry are unchanged by energy flight.

Research and implementation, 2026-09-26. Build: reviewed FA.EXE 1.02F, SHA-256
`e31560c2a6d6adb4aa1493f0308f6ae5640f67a4e886dbdf5887489e6e99244c`. Evidence
is static disassembly. Nothing was run and no retail session was observed.

## Recovered behaviour

- **Insert releases one chaff cartridge and Delete releases one flare**, one per
  press. The original reads keyboard scan codes without the extended bit, so
  keypad 0 and keypad period do the same whatever NumLock says. The full key
  table is in the [keyboard spec](keyboard.md).
- Each press takes one device from the first dispenser that still holds that
  class and reports what is left: `Chaff launched, 3 left` or
  `Flare launched, 3 left`. An empty dispenser reports `Out of chaff` or
  `Out of flares` and releases nothing.
- With **Unlimited ammo**, the count does not fall and the message repeats the
  same number.
- Counts start from the aircraft's ECM record and are shown on the WEAPONS
  window as `N CHAFF` and `N FLARE`
  ([weapons window](weapon-navigation-selection.md)). ECM damage can empty a
  dispenser ([systems damage](systems-damage.md)).
- Chaff can decoy radar-guided missiles; flares can decoy infrared missiles.
  Only a missile whose seeker is guiding on the releasing aircraft can be
  decoyed. It is decoyed with probability susceptibility × effectiveness / 100
  percent, both whole percentages from the weapon and ECM records. This is the
  rule the AI already uses ([missile defense](ai-awareness.md#missile-awareness-and-defense),
  [missiles](missiles.md)). A decoyed missile loses its target and coasts.

### Release sound

- **Every device released makes one sound:** `&CHAFF.5K` for a chaff cartridge,
  `&FLARE.5K` for a flare. The same holds for AI aircraft and for other players
  in multiplayer. An empty dispenser releases nothing and makes no sound.
- The chaff recording lasts 1.15 seconds and the flare recording 2.38 seconds.
- The level is 200 on the original's 0 to 255 scale, before the effects volume
  setting. A weapon launch plays at 255, so a release is about 2 dB quieter
  than a shot.
- The sound comes from the releasing aircraft and moves with it while it plays.
  It is at full level within 100 feet of the camera and fades in a straight line
  to silence at 4,000 feet. A release farther than 4,000 feet away is not
  played at all. Its stereo position follows its direction from the camera.
- In a cockpit view of the releasing aircraft, which for the player means their
  own cockpit, it plays at full level and centered, with no distance.

## Host rules

- A held key does not repeat. Nothing is released while paused, after the
  aircraft is destroyed or after ejection.
- The release shows the flare pair or chaff cloud described under
  [Presentation](#presentation), for the player and AI alike.
- Gamepad: hold View and press D-pad left for chaff, D-pad right for flare.
  `opinionated`, agent decision 2026-09-26.
- Each device the player or an AI aircraft releases plays its recording once.
  The original's 100 and 4,000 feet become the reference and maximum distances
  of T.O.R.E's [traveling sound](../audio.md#traveling-sound), and its level
  becomes a peak gain of 0.31, against the 0.4 of a weapon release.
  `spec-derived`.
- In the player's own cockpit, the player's release plays at once, centered, at
  that peak gain. In every other view it travels from where it was released.
  `spec-derived`.
- Known difference: the level falls with inverse distance, as every T.O.R.E
  traveling sound does, instead of the original's straight line. At 1,000 feet
  the original is at 77 percent of full level and T.O.R.E at 10 percent. The
  sound also stays where the device left the aircraft instead of moving with
  it. `opinionated`: John requested this acoustic model on 2026-09-23; using it
  for releases is an agent decision of 2026-09-26.

## Presentation

John asked on 2026-09-26 for flares that leave in pairs, burn orange and
yellow with a short smoke trail by day, show as a glaring ball at night and
light the aircraft and ground around them, and for chaff that shimmers in the
sun as many small strips. The same day he set each flare's life to 30 seconds
from release, with a fading, flickering last 3 seconds. This is
**opinionated** presentation. The values John gave are marked as his; every
other number is an agent decision (`fitted`). None of it changes counts,
messages, decoy odds or seekers.

### Flares

- **A flare leaves as a pair**, one thrown to each side. The pair counts as
  one flare, so counts, messages, AI budgets and decoy odds are unchanged.
  John chose this on 2026-09-26.
- Both leave 15 feet behind the aircraft's reference point and 2 feet below
  it, at the aircraft's speed, pushed 15 ft/s out of the belly.
- Each is thrown sideways along the wings **20 to 30 feet** (John's range,
  random per flare), 95 percent of the way within 0.75 seconds.
- Drag slows a flare hard. Behind a 400-knot jet it is more than 250 feet
  back after one second, and it settles toward a 100 ft/s fall. Drag acts on
  its motion through the air, so once the throw is spent the mission wind
  carries it (John, 2026-09-28). A flare resting on the ground stays put.
- **Life: 30 seconds from release, then it is gone** (John). It flickers by
  plus or minus 15 percent while it burns. **Over the last 3 seconds it dims to
  nothing while the flicker grows into a sputter** (John). A flare that
  reaches the ground rests 1.5 feet above it and burns there until its 30
  seconds are up.
- It looks like a white-yellow core 1.5 feet across, never less than 3 pixels,
  inside an orange-to-yellow flame 6 feet across that streams back along its
  motion.
- **Glare** is drawn over the finished image. By day it is a small halo about
  three core widths across. At night it is a halo about 6 percent of the
  screen height with four soft streaks. It weakens with distance (half at
  3,000 feet) and with haze. It shows only while the core is in view: no glare
  from a flare behind a hill or an aircraft, but glare from a visible flare can
  spill across the aircraft that released it.
- **Smoke** uses the white missile puff. A flare leaves one puff every 5 feet
  of its path, and at least one every 0.05 seconds while slow. Each puff drifts
  upward at 8 to 15 ft/s in a random direction **within 30 degrees of straight
  up** (John's 30-degree cone, read as 30 degrees either side of vertical),
  slowing over 1.5 seconds, and the mission wind carries it the whole time
  (John, 2026-09-28). It starts 2.5 feet in radius and grows 4 feet per
  second. Puffs thicken over their first 0.1 seconds, so the flame stays
  visible at the head of the trail. They are gone once the flare has moved
  **200 feet** past them (John), or after 3 seconds, whichever comes first. A
  burnt-out flare's remaining smoke finishes fading.

### Flare light

- A burning flare lights nearby aircraft, terrain, buildings, water, clouds,
  chaff and smoke with warm white light, linear RGB (1, 0.75, 0.45).
- It falls with the square of distance. At full strength it lights a surface
  facing it as brightly as full sun at about 63 feet, a quarter as brightly at
  126 feet, and not at all beyond 1,500 feet. A light of the same law
  stronger than a flare (a bomb's flash, a big fire) reaches farther, by the
  square root of its strength in flares (nr-fx3, [night
  glow](explosions.md#night-glow)); a flare's own reach is unchanged.
- At night eyes adapt to the dark, so the same flare counts up to four times
  as much. The boost blends in as the sun goes down.
- Night weather darkens the art itself. Under flare light, surfaces show the
  colors of the weather's brightest record instead, so a flare over desert
  shows sand, not black. Water shows a fixed dark sea color and a rippled
  reflection of each flare.
- The 16 flares that matter most at the camera light the scene at once,
  ranked by strength over distance squared.
- Smoke puffs take at most one full sun's worth of flare light, so the head
  of a trail glows without turning into a flat white sheet.
- Known differences: flare light casts no shadows and reaches every surface
  facing it, even through the aircraft between them. It needs the smooth
  lighting mode; the stepped "original graphics" mode keeps its palette
  lighting, with only the smoke glow.

### Chaff

- A cartridge becomes one cloud of 600 foil strips. It leaves from the same
  point as flares at the aircraft's speed, stops relative to the air almost
  at once (0.12-second time constant), then settles at 4 ft/s and drifts with
  the mission wind (John, 2026-09-28). A cloud that reaches the ground stays
  where it lies.
- The cloud's radius grows from 3 feet to about 35 feet within 1.5 seconds,
  then keeps spreading 1.5 feet per second. Each strip falls at its own 2 to
  6 ft/s, flutters, and spins 2 to 8 turns per second.
- A strip is 0.5 feet across and never drawn smaller than one pixel. Below a
  pixel it fades instead, so a distant cloud glitters faintly rather than
  flickering. Clouds fade out between 12,000 and 20,000 feet away.
- Strips are dim silver. **In sunlight a strip flashes white whenever its face
  mirrors the sun toward the camera, so the cloud shimmers.** At night strips
  catch only moonlight and flare light.
- A cloud lasts 20 seconds and fades over its last 5.
- Up to 64 chaff clouds and 128 flares exist at once; the oldest go first.

## Unknown

- **Dispenser damage messages.** The strings `CHAFF DISPENSER DAMAGED` and
  `FLARE DISPENSER DAMAGED` exist; when they are shown is not traced.
- **Original device art and motion.** The original loads `CHAFF.SH`,
  `FLARE.SH` and `FLARE.PIC` ([weapons notes](../formats/weapons.md)). How it
  draws a device, how it moves and how long it lasts are not traced. Next
  step: trace the device object's render and lifetime.
- **Which views count as cockpit views** for the sound rule. The evidence points
  to the cockpit views; which key selects each internal view case was not
  traced ([sound evidence](../formats/sound.md#countermeasure-release-sound)).

## Source notes

In-flight key dispatch `0x414690`: scan code 0x52 (Insert) branches at
`0x41493b` to `0x415c47`, 0x53 (Delete) at `0x414946` to `0x415c9b`. Both call
the release routine `0x4c39a0` (ECX 1 chaff, 0 flare), which returns the count
before release. A result of zero or less prints the "Out of" string
(`0x4ee4fc`, `0x4ee4d4`). Otherwise the count minus one is printed with
`0x4ee50c` or `0x4ee4e4`, unless global flag `0x4eb6f8` bit 0x8 is set, which
keeps the count. The routine scans the player's dispenser records and creates
one device object per call.

Creating a chaff or flare device requests its recording once, with the
releasing aircraft as the source. The AI device schedule and the network
release message reach the same request. Addresses, the request's arguments,
the distance and pan rules, and the recording hashes are in the
[sound format notes](../formats/sound.md#countermeasure-release-sound).
