# Stores on pylons

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation specification, 2026-10-12 (slice BP, bug B3). John reported that
bombs and missiles left from one fixed point ahead and to the right of the
view, and asked for them to drop from their hardpoints, with the pylons and
loaded stores drawn on the aircraft ("I'd love positions and drawn pylons").
Slice PY2 (2026-10-13) made every wing pylon 2 ft deep, put fuselage and
centreline stores on 4 inch rails and wingtip missiles on the tip itself, hung fuselage pairs one
behind another and moved the A-4E's Mk 82s to a centreline rack (John's
morning list, items 25 to 27). Slice PY3 (2026-10-13 evening, John's evening
list, items 7 and 9 and a follow-up) halved the pylons (1 ft wing pylons,
2 inch rails), replaced the deeper pylons beside nacelles with a sideways
move of the station, hung two stores on a pylon off the sides of a small
bar, rolled the weapon shapes so their fins stand in an X, and gave the
A-4E the real aircraft's three tanks. Slice F14 (2026-10-14, John's
annotated picture of the F-14 from below) moved the F-14's stores to the
real aircraft's stations: tanks under the intake nacelles, the Phoenixes
closer together in the tunnel with the front pair back where the tanks
hung, and the AIM-120 under the glove pylon beside the AIM-9 on its
shoulder rail ([the F-14](#the-f-14)).

## Even paired loads

**Opinionated, requested by John on 2026-10-25 (issue #6), expanded by his
review follow-up.** Every point that carries stores on both sides of the
aircraft loads complete pairs. This includes wings, wingtips, shoulder rails,
fuselage pairs, tunnel pairs and paired internal bays, even while bay doors
hide their stores. Use the resolved carrier's pairing and nonzero lateral
position, the same `Rack::two_sided` predicate as drawing and release.
Centreline and genuinely single-sided points keep their counts. Original
PT/JT records are not rewritten.

After existing compatibility and weight arithmetic, a positive odd maximum
above 1 rounds down (3 to 2, 5 to 4); a maximum of 1 becomes 2. Zero stays
zero. Quantity editing, including clicking a loaded station, steps by 2.
Moving between a pair and a single point also moves 2, or nothing if either
end cannot supply or accept a pair. The same conversion builds human and
AI default loads; AI tactics do not change. Existing singleton designator
and gun-pod equipment paths keep their separate rules.

A maximum of one becoming two is an explicit minimum-pair exception to the
old row weight class. Both stores' full weight, including tank fuel, counts
toward aircraft takeoff weight. This can require removing other stores or
fuel. All other odd maxima round down after the weight calculation.

**Saved preflight loads:** a saved single on a paired point becomes a pair
(1 to 2), as John requested. Larger odd quantities round down (3 to 2).
Clamp to the current maximum; zero stays zero. Layout maxima use the same
rule. A saved single is not silently removed to avoid overweight.

Quick Mission restores a structurally valid migrated load into the editor
even if overweight. The existing weight display shows the excess. Fly refuses
with `This plane is too heavy to take off. Remove stores or fuel.` until the
player changes stores or fuel. Invalid records still fall back to the normal
load. Non-interactive single-player mission construction also applies the
migration, then refuses an overweight start with that same error through
`Combat::with_loadout`. It does not drop the pair, reduce fuel, substitute a
default load or begin simulation. Network requests must already obey current
counts and aircraft weight limits; old odd requests are refused. An odd
count on a two-sided point is refused with `Paired stores must be loaded in
pairs.` (or `Paired tanks ...`); any other layout or quantity mismatch is
reported without attributing it to pairing.

In-flight ammunition, checkpoints and recorded store layouts are not
migrated: firing one store legitimately leaves an odd count. Each even load
starts with equal side counts and mass at mirrored positions. The existing
release rule remains left, right, left, right for the selected point, so the
heavier remaining side of that point fires next after its first shot.
Jettison removes actual remaining stores. The flight adapter still uses total
payload mass and total load-dependent drag; no lateral store-CG or per-side
drag torque model is introduced.

Retail has aggregate source-row capacities and an alternating per-row release
offset. John confirms it did not draw these pylons and loaded stores.
T.O.R.E's old right-extra layout independently applied to every row caused
the visible pile-up. Even preflight loads are a deliberate departure from
retail quantity editing, not a correction to the original records.
[Source-row research](../formats/aircraft-ordnance.md#paired-point-counts-and-physics)
owns that evidence and its limitations. The
[roster validation](../baselines/even-paired-loads.md) lists every changed
maximum and reports the default-load audit.

## What the player sees

- **Every store leaves from where it hangs.** A missile, bomb or rocket
  appears exactly where it was drawn a moment before, in the aircraft's
  frame: under its own pylon or rail, at its place on a rack, wingtip
  missiles on the tip, internal bay stores below the bay. Nothing jumps at
  release.
- **Wing pylons are 1 ft deep.** A wing store hangs with its top 1 ft
  under the wing above it.
- **Fuselage and centreline stores sit on short rails**, their tops about
  2 inches under the body, as launchers and conformal stations are.
- **A store beside a nacelle or a body's flank moves aside** (up to 2 ft)
  rather than hanging deeper, so no pylon is deeper than those above.
- **Two stores on one pylon hang beside it**, one each side of a small bar
  at the pylon's foot, as a wingtip missile sits on the tip, not under a
  wide beam.
- **Fins stand in an X.** Every weapon is turned about its long axis so its
  fins point 45 degrees away from the pylon, the wing, the body and the store
  beside it, and it keeps that turn after release.
- **Wingtip missiles sit on the tip.** No pylon or rail is drawn under the
  tip: the missile's centre line lies on the tip's edge, as an F-16's or
  F/A-18's tip AIM-9 does.
- **The F-14 carries its stores at its real stations**: its tanks under
  the intake nacelles, four Phoenixes close together in the tunnel between
  them (two ahead, two behind), and on each glove pylon an AIM-120 (or
  AIM-7 or AIM-54) under it with the AIM-9 on its shoulder rail beside it
  ([the F-14](#the-f-14)).
- **Fuselage pairs hang one behind another**, as the F-14's Phoenixes and
  the F-15's and F-4's Sparrows do, and the front pair leaves first.
- **Paired stations alternate.** Two AIM-9s on the wingtips leave left, then
  right; four Mavericks on a wing pair leave left, right, left, right. The
  player, the AI and every seat of a network game release the same way, so a
  remote player's missile appears at that player's pylon.
- **Pylons and loaded stores are drawn** on every aircraft that carries a
  combat loadout, in outside views and the flown aircraft's own views of
  itself, and they empty as stores leave. A pylon stays when its stores are
  gone. In the cockpit view the stores hide with the rest of the flown
  aircraft's model.
- The bomb's fall, sag and drop after release (the weapons pass behaviour) are
  unchanged; only the point they start from moves.

## Rules

| Rule | Value | Provenance |
| --- | --- | --- |
| Station position | The source hardpoint record's x (right) and z (forward), a third of a foot a unit, from the aircraft's reference point ([table](../formats/aircraft-ordnance.md#hardpoint-positions)); its height is the pylon's (below) | spec-derived (the height: opinionated, PY2) |
| Pylon depth | A wing store's top (its lugs, the top of its body) hangs 1 ft under the model's underside over its pylon; a fuselage or centreline store's 2 inches (0.17 ft) under it, on a launcher rail. Stores beside a rack's bar keep the same depth; a triple rack's third hangs under the bar. The underside is the lowest face of the clean model up to 12 ft above the record, straight above the pylon or, where nothing is straight above, up to 3 ft inboard. A station with nothing of the model above it keeps the record's height | opinionated, John 2026-10-13 (morning: 2 ft wing pylons and 4 inch rails; evening: "pylons need to be about half the size they are now"); the samples are agent decisions |
| Moved aside | A station whose store would cut through a part of the model beside it (a face within 0.6 ft either side of the pylon reaching more than 0.25 ft below the store's top: a nacelle, a gear pod, a body's flank) moves sideways in 0.25 ft steps, out before in at each distance, up to 2 ft, to the first place it clears with model straight above. A store that cannot clear within 2 ft hangs under the lowest surface across its body, as PY2 hung every store (no default load needs it). The move is part of the station's position, so it releases from there too. On the default loads: F/A-18D fuselage AIM-120s 0.25 ft in (3.08 ft out), Rafale fuselage Mavericks 0.25 ft in (3.08), AV-8 fuselage Mavericks 0.25 ft out (3.25), Su-25 B-13 pods 0.5 ft out (5.5), (the A-10's rows are placed by hand, [the A-10](#the-a-10)). The F-14's stations are placed by hand instead ([the F-14](#the-f-14)) | agent decision, 2026-10-13 (John: short pylons; a store inside the launching aircraft's hit box is not a reason to hang it deeper, slice SD) |
| Under the chord | A wing station with nothing of the model straight above it (nor up to 3 ft inboard) slides forward or aft, up to 12 ft, to under the wing's chord at its distance out, nearest the record: 2.4 ft behind the leading edge, so a launch rail about as long as an AA-11's pylon (4.3 ft) starts at the leading edge and the missile's nose reaches ahead of it, as on real underwing launchers; on a chord shorter than 4.8 ft, at its middle (John 2026-10-14: "center the rail to the leading edge"). A safety net: no default load of the 22 aircraft the stores preview draws needs it. It was first written for the Yak-141's outer row (retail records it 16.67 ft out at 0 ft forward, ahead of the swept wing, so its AA-11s hung in space), which now has its own stations ([the Yak-141](#the-yak-141)); it stays for any other row recorded off the wing | agent decision, 2026-10-14 (fitted) |
| Real stations (A-10) | The A-10's rows 6 and 7 hang on the wing at 10.75 and 14.75 ft out, not under the main gear pods ([the A-10](#the-a-10)) | opinionated, John 2026-10-14; the spans are read from his marks |
| Real stations (Su-27, Su-35) | The Flankers' tunnel, intake and wing rows hang at the real aircraft's stations, one missile a pylon, from the per-aircraft table ([the Su-27 and Su-35](#the-su-27-and-su-35)) | opinionated, John 2026-10-14 ("make them look realistic"); the positions are agent decisions read from the models |
| Real stations (Yak-141) | The Yak-141's two wing rows hang at its inner and outer underwing pylons, from the per-aircraft table, not the records' positions ([the Yak-141](#the-yak-141)) | opinionated, John 2026-10-14; the positions are agent decisions read from the model |
| Real stations (F-14) | The F-14's four weapon and tank records hang at the real aircraft's stations, from a per-aircraft table, not the records' positions: no move aside there ([the F-14](#the-f-14)) | opinionated, John 2026-10-14; the positions are agent decisions read from the model |
| Shoulder rail | A store on a pylon's shoulder rail (the F-14's AIM-9) hangs 1.25 ft outboard of that pylon's centre line, its body 0.05 ft above the pylon's foot (the lugs, the top, of whatever store hangs under the pylon), and leaves from there: the rail juts off the pylon's side just above the main store, so the shoulder store's body is above the whole body of any store under the pylon, real loads or not (AIM-120, AIM-7, AIM-54, bombs, tanks, GBU-10). 1.25 ft keeps an AIM-9's fins (2.07 ft across at 45 degrees) clear of the pylon and of the fins of an AIM-120, AIM-7 or AIM-54 under it | opinionated, John 2026-10-14 ("the AIM-9 rail in real life is a jut off the main pylon slightly elevated above the stores on the main wing pylon"); the spacing and the 0.05 ft gap are agent decisions (fitted) |
| Wingtip | A wingtip store's centre line lies on the tip's edge: as far out as the model's outermost point beside the record (within 4 ft inboard and 0.3 ft outboard of it, 2.5 ft fore and aft, 3 ft up or down), at the middle height of that edge. No pylon or rail is drawn. With no model read, the record's position | opinionated, John 2026-10-13 ("the missile's centre line sits on the wingtip geometry, no gap"); the search box is an agent decision |
| Where the underside is read | When the loadout is built, from the aircraft shape the simulation already reads (`<ID>.SH`, at the debris scale): over the record, over a two-pylon wing's inboard and outboard pylons (1.4 ft either side), and 7 ft ahead of and behind the record; a tandem pylon's surface is interpolated between the record and the sample on its side. The heights travel with the station (`Rack::over`), into checkpoints and replays | agent decision |
| Released from | The point the store hangs at: its side, its pylon, its place on the pylon's rack and the pylon's height (`Rack::store_point`), for the player, the AI, every network seat and replays. The bomb sight predicts from the same point | opinionated, John 2026-10-13 ("a store never jumps at release") |
| Store size | Length, the height of its lugs above its origin (half its body diameter) and its width across its fins, from the [real sizes](#store-sizes) table by shape name; a pod's or tank's fitted body; 8 by 0.9 ft for a store not listed. The origin is taken as the store's middle | agent decision |
| Paired record | Hardpoint flag 0x4: the record's position is the right pylon, its mirror the left | spec-derived ([release state](../formats/weapons.md#release-state)) |
| Also paired | Every Wing and Wingtip record without flag 0x4 (the Mi-24's AT-2 rails) | agent decision, 2026-10-12 |
| A one-sided wing row by the centreline | A Wing record without flag 0x4 less than 3 ft out (the A-4E's Mk 82 row, 2.7 ft out, which retail releases from one side) is a centreline rack at x 0 ([A-4E](#the-a-4e)) | agent decision, 2026-10-13 |
| Fuselage pairs one behind another | A Fuselage record whose side holds two stores, or four or more, has two pylons on that side, one ahead of the other: their stores' middles are half a store length and 0.5 ft either side of the record's position, so they clear by 1 ft. The front pylon's store leaves first, so four leave left front, right front, left rear, right rear. Four or more a side: two side by side on each pylon's rack | opinionated, John 2026-10-13 ("fuselage missile pairs one behind another: fix"); the gap and the front-first order are agent decisions (the real aircraft's firing order is not published; front first keeps a rear missile from passing under a loaded front one) |
| Fuselage pair on the centreline | A paired Fuselage weapon record at x 0 holding two or more (the F-4E's and F-4G's four Sparrows) hangs 3 ft either side of the centreline, in the Sparrow wells | agent decision, 2026-10-13 (fitted) |
| Never paired | An internal gun, whatever its flags; a record on the centreline (nothing to mirror) | agent decision |
| Which side | The mirror (left) when the whole releases left on the station after this one (rounds over `actualRoundsPerGame`) is odd. Even wing loads start symmetric and release left then right; a partially spent point can hold one extra on the right | fitted release order based on the documented retail per-row alternation; no claim of a retail drawn-store split |
| Unlimited ammunition cheat | The count never moves, so the side follows the release's projectile number (alternates) | agent decision |
| Who | The player, the AI (strike stores, missiles and seeker missiles alike) and every network seat. Retail releases a distant AI aircraft from its centre; T.O.R.E releases every aircraft from its pylon | agent decision |
| Guns | Keep their muzzle (gun mount, AC-130 barrel) | unchanged |
| Bomb packs | Each bomb of a pack keeps its pack stagger, added to the pylon's side | unchanged |
| Twin rails on a wing | A Wing record whose side holds two stores (a row of four on a paired record) hangs them on one pylon at the record's position, on a twin-rail launcher: one each side of its bar ([drawing](#drawing-opinionated-john-2026-10-12)), as the real F/A-18's wing pylons carry two missiles each. The racks are mirrored left and right, so on both wings the outboard store leaves first: four Mavericks leave left outboard, right outboard, left inboard, right inboard. A side moved aside for a part beside it is checked 1.5 ft either side of the pylon (two stores side by side) rather than 0.6 ft. Four or more on a side (beyond retail's maxima) hang on two pylons 1.4 ft either side of the record, each with its rack. Three on a side hang on one pylon's triple ejector rack, and Fuselage, Centerline and Wingtip records keep one pylon a side | opinionated, John 2026-10-14 ("the F-18s need the dual pylons for the wings that take 4 weapons at a station", with a picture of a real F/A-18; on 2026-10-12, "four stores on a wing row: dual pylons on the wing", BP2 had read two pylons a wing); the 1.5 ft and the rule for other counts are agent decisions |
| Rockets from a two-pylon side | Leave from the pod that is firing (the outboard pod first) | agent decision |

**The bomb sight aims from the pylon the next store leaves** (John,
2026-10-12, slice BP3). The CCIP pipper, the level T and the rocket pipper
predict the release from the pylon of the selected station the next store
will leave, by the same rule the release uses (`Station::release_point`):
the side from the whole releases left on the station once that store has
gone, odd for the mirror (left), even for the record's own position (right),
and on a twin rail the side of the bar that store hangs from. So with four
Mk 82s on a wing row the pipper shows the left outboard bomb's fall, then
the right outboard, then the inboard ones. With unlimited ammunition the release's projectile number picks the
side, as the release does. The AI's release gate reads the same solution from
the pylon its next store leaves, so an AI bomb is judged from where it
drops (agent decision: the AI's next side is worked out from its weapon
service's rounds before the debit, which is what the release then debits).

| Sight | Predicts from | Provenance |
| --- | --- | --- |
| CCIP (bombs) | The next store's pylon, pack offset 0 | opinionated, John 2026-10-12 |
| Level T (bombs) | The CCIP's release point, so the same pylon | opinionated, John 2026-10-12 |
| Rocket pipper | The next rocket's pod | opinionated, John 2026-10-12 |
| AI release gate | The AI's next store's pylon | agent decision |
| Guided weapon sights, gun sights | Unchanged (the aircraft, the gun's muzzle) | unchanged |

## Drawing (opinionated, John 2026-10-12)

In a replay, carried weapons use their own shapes even if that weapon type
was never fired. The [replay stores contract](../REPLAYS.md#stores) describes
shape loading and how old recordings retain their station placement.

Retail draws no stores on aircraft (the A2G pass recorded this as decision 20,
"stores on pylons stay undrawn"); John reversed that decision on 2026-10-12.
Every number below is fitted (agent decision).

| Item | Rule |
| --- | --- |
| Stations drawn | External ones (Centerline, Fuselage, Wing, Wingtip) that held something at mission start; not guns or internal bays |
| Store look | The weapon's own shape (the same mesh it flies with after release) refitted to the real store's size ([store sizes](#store-sizes)). Rocket pods: a grey-green eight-sided pod of the real pod's size (LAU-61 5.5 by 1.38 ft, LAU-10 8.3 by 1.25, B-8M1 8.9 by 1.71, B-13L 11.7 by 1.35; any other 8.5 by 1.4). Fuel tanks: a light grey pod 13 ft by 2 ft at 2,000 lb of fuel, scaled by the cube root of its fuel (a 330 US gallon tank is 13.3 by 2.05 ft; the real one is about 12.9 by 2.1). A store with no shape: a plain body 8 ft by 0.9 ft |
| Stores on one pylon | One at the pylon point. Two hang off the sides of a small bar at the pylon's foot, one each side, their centre lines level with the bar's middle and at its ends, as a wingtip missile sits on the tip (John 2026-10-13: "the panel should be a small bar on the pylon and ordnance hangs to the side"): out from the pylon's middle by the pylon's half thickness (0.14 ft), a 0.1 ft gap and the store's body radius, or 0.36 of its width across the fins if more, so two sets of fins at 45 degrees clear each other (AIM-9 0.75 ft, Mk 82 0.69 ft, Maverick 0.85 ft). Three as a triple ejector rack: one each side of the bar and the third under it. Four as two pairs one behind another (a multiple ejector rack), the front pair first. More: triple racks one behind another, 1.05 store lengths apart. The layout keeps the loaded count's slots; the highest slot empties first. The simulation places them ([released from](#rules)) and the drawing hangs them there |
| Pylon | A grey slab 0.28 ft thick, from the top of the store's body (its lugs, not its fin tips) or a rack's bar up into the model's underside (1 ft, see [pylon depth](#pylon-depth)), 0.02 ft into it (retail wings are single faces, so more would show on top). Its length is 0.45 store lengths, at least twice its depth and at most 0.7 store lengths (1.2 to 8 ft), cut short where the underside ends (a pylon does not run out ahead of a leading edge), and its top follows the underside from end to end. The underside is the lowest face of the clean model up to 12 ft above the lugs, straight above or up to 3 ft inboard, preferring a nearby short attachment over a distant overhead surface ([Apache correction](#apache-stub-wing-pylon-attachment)) |
| Rack | A bar 0.25 ft deep across the pylon's foot, reaching out to the centre lines of the stores beside it, 0.7 of the pylon's length (plus the racks one behind another) |
| Fin roll | Every weapon shape is rolled about its body's centre line so its fins stand in an X, 45 degrees from straight out and straight up and down, so no fin points into the pylon, the wing or body above, or the store beside it (John 2026-10-13: "ordnance needs to be rotated 45 degrees to make sure fins don't clip and are perpendicular to pylons and the fuselage as best as possible"). The roll is measured per retail shape, not assumed: the fin tips' clock about the body (the corners more than 1.6 body half widths out, their angles averaged over a quarter turn). A "+" shape (clock 0) turns 45 degrees; a shape already in an X (clock about 45) is not turned; a shape whose tips agree on no clock (agreement under 0.5 of 1) is not turned. One roll serves every mount (under a pylon, beside a bar, on a wingtip, on a rail), since an X is the same seen from below or from the side. The rolled shape is the one drawn everywhere the store is: on the pylon, in flight after release (so it does not turn at release), for the player, AI aircraft, network peers, replays and previews. See [fin roll by shape](#fin-roll-by-shape). Opinionated, John 2026-10-13; the measuring rule and one roll per shape are agent decisions |
| Wingtip | Nothing drawn under a wingtip store ([rules](#rules)) |
| Shoulder rail | The pylon the store rides beside, from its foot into the surface above (drawn again for the shoulder store, so it stands when the station under it was left empty), and an arm 0.15 ft deep jutting off the pylon's side, above the main store, out along the shoulder store's top to its centre line, 0.6 store lengths long (3 to 6 ft), drawn as pylon structure. The F-15C's twin shoulder rails on one pylon (two Sidewinders, 1.75 ft either side of it) use the same arm, 3.5 ft wide across the pylon and 3 to 6 ft long, over both missiles. The 2026-10-20 attempts to narrow it to the F/A-18 bar's size, and then to half the width, were undone the same day: John moved the whole pylon inboard instead, below the paired pylon as on the real F-15 |
| Edge rail | A wing station's store with nothing of the model above it, beside a wing's edge less than 4 ft inboard, hangs on a rail from 0.3 ft inside the tip's edge out over the store, as deep as both the tip's edge and the store's upper half, 0.6 store lengths long (3 to 9 ft). With no tip beside it, a 0.35 ft rail |
| Colours | Pylons and racks grey (0.5, 0.52, 0.54), tanks light grey, pods grey-green; weapon shapes keep their palette colours |
| Lighting | Exactly the airframe's: every pylon, rack, fitted body and weapon face carries a palette entry (a fitted colour takes the aircraft palette's nearest entry) with its fog mode, so the weather's time-of-day palette darkens it at night and at dusk, fog and haze fall on it, and the same sun, moon and ambient light it, as they do the untextured faces of the aircraft it hangs on; a released weapon in flight is lit the same way. Before 2026-10-13 they carried fixed colours and showed bright at night (John's report, an A-10 at night). The stepped compatibility lighting (`TORE_WEATHER_SMOOTH=0`), which picks a light row per airframe face, is not applied to stores | opinionated, John 2026-10-13; the nearest-entry rule is an agent decision |

Where the stores come from:

- **Single player, the host, AI aircraft:** the render snapshot carries each
  aircraft's stations and their stores left ([`AircraftPose::stores`]).
- **Network clients (protocol 27, slice BP3, John 2026-10-12):** a client
  draws every aircraft's pylons and stores from its own copy of the
  aircraft's loadout (the session's copy of the mission, which every game
  builds the same), as the loadout built it until the host says otherwise.
  The host sends a **Stores** event for an aircraft when what it draws
  changes (a release that empties a whole store, a jettison, a rearm): the
  aircraft's drawn stations with the whole stores still on each. A player
  seated later, a resumed seat after a migration and an observer are told
  every aircraft not as built when they start. The client puts each told
  state at its tick, so a station empties when the picture reaches the
  release, the same moment the store appears as a weapon. The seat's own
  aircraft is still drawn from its cockpit readout, which is newer. A
  Stores event is about 14 bytes for a fighter's six stations and costs
  nothing while nothing changes ([the protocol](../formats/net-protocol.md#protocol-27-stores)).
- **Mission replays (slice BP3, John 2026-10-12):** a recording keeps each
  aircraft's drawn stations the first time it is recorded and their whole
  stores whenever one empties, as `stores.*` events; the viewer draws each
  aircraft's stores at the tick it shows, forward, backward and after a
  seek. A converted network capture and an observer's live recording keep
  them too. A recording made before them has none, so its aircraft carry no
  stores ([replays](../REPLAYS.md#stores)).

## Store sizes

John, 2026-10-12: "are the weapon renders from retail accurate? They seem big
and bulky. Check weapon renders to make sure they are realistic sizes."
Measured with `tore-app --stores-sizes` (every weapon shape and every
aircraft, as drawn) and `--stores-size-sheet` (silhouettes, retail under
refitted, on a one-foot grid):

- **The aircraft are drawn at their real size**, within about 10 percent:
  F/A-18D 56.0 ft long (real 56.1) and 36.3 ft across the tips (37.5), F-16C
  49.0 by 34.0 (49.4 by 32.7 with tip missiles), F-14 60.0 (62.75), A-10 48.7
  by 54.0 (53.3 by 57.5), Su-25 50.0 (50.9), MiG-29 56.0 (56.8), AV-8 50.0
  (46.3), A-4E 40.0 (40.1).
- **The retail weapon shapes are about the right length but 1.3 to 2 times
  too thick.** Shape coordinates are whole source units of a third of a foot,
  so the thinnest body a shape can draw is 0.67 ft across; an AIM-9 is
  0.42 ft. Bodies and fins are drawn larger still: a Mk 82's body 1.33 ft
  (real 0.9) with 2.0 ft fins (1.27), a Maverick's 1.33 ft (1.0) with 3.3 ft
  wings (2.36), an AIM-54's 2.0 ft (1.25). Lengths are within 10 percent,
  except the dispensers: CBU-87 and CBU-97 drawn 11 and 13 ft (real 7.7),
  Mk 20 10.7 ft (7.7), RBK-250 and RBK-500 11 ft (6.4 and 8.2); the FAB-250
  is drawn short (5.0 ft, real 6.4). There is no unit mix-up: missiles, bombs
  and aircraft all use the same third of a foot, and no shape header carries a
  different scale exponent.
- **The fitted pylon bodies were too long or too fat** for some pods (a
  LAU-61 drawn 8.5 ft, real 5.5).

**Refit (opinionated, John 2026-10-12; rule an agent decision).** Every
listed shape is refitted, on the pylon and in flight, so a store does not
change size when it leaves (the in-flight shapes are wrong by the same
measure). Each axis is mapped separately: the length scales about the
store's origin (its release point) to the real length; across, the body
(the second thinnest of nine cross-sections between a quarter and three
quarters of the length) scales to the real diameter and the fin tips to the
real span, linearly between, so a thin fin stays thin. A shape several
weapons share takes the size of the store it depicts. Unlisted shapes
(surface-launched missiles, the AT-12, the AAM-L) keep their retail size.

| Shape | Weapons | Length ft | Body ft | Span ft | Source |
| --- | --- | ---: | ---: | ---: | --- |
| `SIDEM.SH` | AIM-9M | 9.35 | 0.42 | 2.07 | US Navy fact file; AIM-9L/M 2.85 m, 127 mm, 0.63 m |
| `SIDEB.SH` | AIM-9B | 9.3 | 0.42 | 1.83 | AIM-9B 2.83 m, 127 mm, 0.56 m |
| `AM9X.SH` | AIM-9X | 9.9 | 0.42 | 1.47 | Raytheon; 3.02 m, 127 mm, 0.45 m |
| `AMRAM.SH` | AIM-120 | 12.0 | 0.58 | 1.75 | US Air Force fact sheet; 3.66 m, 178 mm, AIM-120A/B 0.53 m |
| `SPAR.SH` | AIM-7, AIM-7E (and the SS-N-9 and Sea Sparrow records) | 12.0 | 0.67 | 3.3 | US Navy; 3.66 m, 203 mm, 1.0 m |
| `PHENX.SH` | AIM-54 | 13.0 | 1.25 | 3.0 | US Navy; 3.96 m, 380 mm, 0.91 m |
| `MICA.SH` | MICA | 10.2 | 0.53 | 1.83 | MBDA; 3.1 m, 160 mm, 0.56 m |
| `M55.SH` | R-550 Magic | 9.0 | 0.52 | 2.17 | Matra; 2.75 m, 157 mm, 0.66 m |
| `M53.SH` | Super 530D | 12.5 | 0.86 | 2.03 | Matra; 3.8 m, 263 mm, 0.62 m |
| `APHID.SH` | AA-8 (R-60) | 6.9 | 0.39 | 1.28 | Vympel; 2.09 m, 120 mm, 0.39 m |
| `ARCHR.SH`, `AA11B.SH` | AA-11 (R-73) | 9.5 | 0.56 | 1.67 | Vympel; 2.9 m, 170 mm, 0.51 m |
| `ALAMO.SH` | AA-10 (R-27) | 14.8 | 0.75 | 3.2 | Vympel R-27ET; 4.5 m, 230 mm, 0.97 m tail |
| `AMSKI.SH` | AA-12 (R-77) | 11.8 | 0.66 | 2.2 | Vympel; 3.6 m, 200 mm, 0.68 m grid fins |
| `ATOLL.SH` | AA-2 (K-13) | 9.2 | 0.42 | 1.73 | 2.8 m, 127 mm, 0.53 m |
| `ACRID.SH` | AA-6 (R-40) | 20.0 | 1.02 | 4.76 | about 6.1 m, 310 mm, 1.45 m |
| `AMOS.SH` | AA-9 (R-33) | 13.6 | 1.25 | 3.87 | Vympel; 4.15 m, 380 mm, 1.18 m |
| `STGR.SH` | FIM-92 | 5.0 | 0.23 | 0.3 | 1.52 m, 70 mm |
| `MAVRK.SH` | AGM-65A, AGM-65G | 8.2 | 1.0 | 2.36 | US Air Force fact sheet; 2.49 m, 305 mm, 0.72 m |
| `HARM.SH` | AGM-88 (and the AGM-45, 10 ft real, which retail draws with this shape) | 13.7 | 0.83 | 3.7 | US Navy; 4.17 m, 254 mm, 1.12 m |
| `HARPN.SH` | AGM-84A | 12.6 | 1.12 | 3.0 | US Navy; air-launched 3.84 m, 343 mm, 0.91 m |
| `SLAM.SH` | AGM-84E | 14.6 | 1.12 | 3.0 | US Navy; 4.45 m |
| `KERRY.SH` | AS-7 (Kh-23) | 11.6 | 0.9 | 2.58 | 3.53 m, 275 mm, 0.785 m |
| `KEDGE.SH` | AS-14 (Kh-29) | 12.8 | 1.25 | 3.6 | 3.9 m, 380 mm, 1.1 m |
| `KIKBK.SH` | AS-16 (Kh-15) | 15.7 | 1.49 | 3.0 | 4.78 m, 455 mm, 0.92 m |
| `AS12.SH` | AS.15TT | 7.1 | 0.62 | 1.85 | Aerospatiale; 2.16 m, 187 mm, 0.56 m |
| `AS30.SH` | AS.30L | 12.0 | 1.12 | 3.3 | 3.65 m, 342 mm, 1.0 m |
| `EXO.SH` | AM-39 Exocet | 15.4 | 1.15 | 3.6 | MBDA; 4.69 m, 348 mm, 1.1 m |
| `SWATR.SH` | AT-2 | 3.8 | 0.48 | 2.17 | 1.16 m, 148 mm, 0.66 m |
| `MK82.SH`, `MK82A.SH` | Mk 82, Mk 82 P3, Mk 82 AIR | 7.3 | 0.9 | 1.27 | US Navy; 2.22 m, 273 mm; fin span about 15 in (approximate) |
| `SNAKE.SH` | Mk 82 Snakeye | 7.4 | 0.9 | 1.27 carried, 4.0 released | as Mk 82; retail draws its retarding fins open, as they are after release; the open span is approximate |
| `MK84.SH` | Mk 84 | 10.8 | 1.5 | 2.1 | 3.28 m, 458 mm; fin span about 25 in (approximate) |
| `MK20.SH` | Mk 20 Rockeye | 7.7 | 1.1 | 1.45 | 2.34 m, 335 mm |
| `CB87.SH`, `CB89.SH` | CBU-87, CBU-97 | 7.7 | 1.3 | 1.9 | SUU-65 dispenser 2.34 m, 396 mm; fin span approximate |
| `PAVE.SH` | GBU-10 | 14.2 | 1.5 | 5.4 | 4.32 m, 1.65 m wings |
| `GB27.SH` | GBU-27 (and the GBU-29 and GBU-30 retail draws with it) | 13.9 | 1.2 | 5.4 | 4.24 m, BLU-109 370 mm |
| `GB28.SH` | GBU-28 | 19.2 | 1.21 | 2.2 | 5.84 m, 370 mm; fins approximate |
| `F250.SH`, `RBK2.SH` | FAB-250, RBK-250 | 6.4 | 1.07 | 1.35 | M-62 1.96 m, 325 mm |
| `F500.SH` | FAB-500 | 7.9 | 1.3 | 1.6 | M-62 2.4 m, 400 mm |
| `F1000.SH` | FAB-1000 | 9.4 | 2.07 | 2.6 | about 2.87 m, 630 mm |
| `RBK5.SH` | RBK-500 | 8.2 | 1.48 | 1.8 | 2.5 m, 450 mm |

Numbers are public data (manufacturers' and services' published figures, as
collected by Wikipedia and FAS Military Analysis Network), rounded; spans
marked approximate are estimates. Pod sizes are in the drawing table above.

## Fin roll by shape

Measured with `tore-app --stores-sizes` (the `fins` lines) on the retail
shapes:

| Fins | Shapes | Roll |
| --- | --- | --- |
| "+" (clock 0) | AIM-9 (`SIDEM`, `SIDEB`), AIM-120 (`AMRAM`), AIM-7 (`SPAR`), AIM-54 (`PHENX`), AA-2, AA-6, AA-8, AA-10, AA-11 (`ARCHR`), AA-9, Maverick (`MAVRK`), HARM, Harpoon, SLAM, AS-7, AS-14, AS-16, AT-2, Mk 82, Mk 84, GBU-10 (`PAVE`), FAB-250, FAB-1000, and the surface-launched missiles | 45 degrees |
| Already an X (clock 44 to 48) | AIM-9X (`AM9X`), MICA, R-550 (`M55`), AS.30L, AS.15TT (`AS12`), Exocet, CBU-87, CBU-97, Mk 20, GBU-27, GBU-28, RBK-250, RBK-500, Snakeye (fins drawn open in an X), FIM-92, AAM-L | none |
| No clear pattern | AA-11 (`AA11B`, 0.3), Super 530D (`M53`, 0.4), AA-12 (`AMSKI`, grid fins), the rocket shape; no fin tips found: FAB-500, Mk 82 AIR (`MK82A`) and a few surface shapes | none |

The unturned shapes with no clear pattern are the ones whose fins may still
touch a pylon or a neighbour; at the refitted real spans none showed a fin
through a pylon in the close-ups.

## Pylon depth

Before PY2 a store hung at its hardpoint record's height, so a pylon was as
deep as the gap from the record to the model: about 2 ft on the F/A-18D's
wing, 3.3 ft on the F-16C's and MiG-29's, 4 to 5 ft on the AV-8's and
Su-25's, and 7 ft under the F-14's body. Real pylons are 15 to 30 inches;
John asked for 2 ft everywhere, then the same day for short rails about
4 inches deep at fuselage and centreline stations and wingtip missiles on
the tip itself (2026-10-13). Every store now hangs that far under the
surface over it and leaves from there, so the release origins moved with
it: up by a few feet on most aircraft, by up to 4 ft under the F-14, and
wingtip missiles in to the tip's edge (1 ft on the F/A-18D and F-16C,
1.7 ft on the Rafale). In the evening John asked for pylons about half that
size (PY3): 1 ft on wings and 2 inches at fuselage and centreline rails, so
every store's release origin moved up again by 1 ft (wing) or 2 inches
(body), and more where PY2 had hung a store clear of a nacelle.

Measured with `tore-app --stores-preview` (the depth from each store's lugs
straight up to the drawn model, feet; default loads):

| Aircraft | Stations | Depth |
| --- | --- | --- |
| F/A-18D, F-16C, F-14, A-10, Su-25, MiG-29, AV-8, A-4E, F-15, F-4B, F-4E, F-4J, Rafale | Wing stations | 1.0 |
| F/A-18D, F-14, F-15, F-4B, F-4E, F-4J, AV-8, Su-25, Rafale | Fuselage stations (moved aside where noted in [the rules](#rules)) | 0.17 (0.02 to 0.15 where the body's underside slopes along the store) |
| F-4B, A-4E (Mk 82s loaded there) | Centreline pairs beside the bar | 0.17 |
| F/A-18D, F-16C, Rafale | Wingtip missiles | on the tip's edge (0.0 to 0.1) |

PY2's deeper pylons (Su-25 B-13 pods 3.2 ft, AV-8 fuselage Mavericks 3.7 ft,
F-14 front Phoenixes 3.4 ft) are gone: those stations moved aside instead
([moved aside](#rules)). The record's own fore and aft position is kept
everywhere, and its lateral position except the moves listed there and the
F-14's stations ([the F-14](#the-f-14)).

## The A-4E

Retail's default A-4E load is four Mk 82s on a one-sided wing row 2.7 ft
out, four 150-gallon tanks on the next wing row (5.7 ft out, paired) and two
LAU-61 pods on the outer wing row. The real A-4E has five stations: a
centreline one and two a wing, and carries its tanks on the centreline and
the inner wing pylons. With BP2's two pylons a wing, the four tanks (two a
wing) and the Mk 82s (paired, two a wing) crowded the same stretch of wing.

PY2 (agent decision, 2026-10-13) kept retail's quantities and moved the
Mk 82 row, which retail releases from one side next to the fuselage, to the
centreline: a rack of two pairs one behind another on a short rail under the
belly, the front pair first, right then left.

**Default load since PY3 (opinionated, John 2026-10-13: "A-4E: the real
three tanks (centreline + inner wings)").** A 150 gallon tank on the
centreline row and one on each inner wing pylon (5.7 ft out, one pylon a
wing), the LAU-61 pods on the outer row as before. External fuel is 2,970 lb
instead of retail's 3,960 (about 990 lb less, one tank). The centreline row
can carry a tank or the Mk 82s, not both, so the Mk 82s are no longer loaded
by default; they stay the row's weapon in the ordnance screen (four on the
centreline rack), as do the other stores each row accepts, and each row
keeps retail's capacity (the inner row can still take four tanks). Only what
starts loaded changes (`hardpoints::default_records`, applied by the live
configuration). The development driver `--a2g-run --load MK82` still loads
the four Mk 82s on the centreline row retail defaults them to, taking that
row's tank off.

## The F-14

John, 2026-10-14, on a picture of the F-14 from below: "default tank
pylons are actually under engine intake nacelles", "AIM-54s grouped closer
together and front missiles tucked backward where tanks are", "AIM-120
default pylon tucked under wing flaring next to AIM-9". Retail's records
put the tanks and all four Phoenixes 3 ft either side of the centreline,
the two tanks between the front and rear Phoenix pairs, and the AIM-120s
6 ft out, beside the nacelles rather than under the glove.

The real F-14's stations are the guide: 1A/1B and 8A/8B on the glove
pylons (an AIM-9 on the pylon's shoulder rail, an AIM-7, AIM-54 or AIM-120
under it), 2 and 7 under the intake nacelles (the tanks), 3 to 6 the
tunnel pallets between the nacelles (two ahead, two behind). T.O.R.E's F-14
model has its tunnel ceiling 2.7 ft under the reference out to 3 ft, the
nacelles from 3.5 to 6.5 ft out (their bottom ramps from 2.7 ft down at the
intake to 4 ft down from the reference point back), and the glove from
6.5 ft out about 0.6 ft up.

| Record (F14.PT row) | Retail position, ft out / forward | T.O.R.E station | How it hangs |
| --- | --- | --- | --- |
| 4, AIM-54 x4 (tunnel) | 3.0 / 8.0 (moved to 2.5 by PY3) | 1.25 / 1.5 | Tandem pairs on 2 inch rails, the tunnel pallets closed up (John 2026-10-14): 0.25 ft from the rear pair's noses to the front pair's tails (other aircraft's fuselage pairs keep 1 ft), front pair 1.6 to 14.6 ft forward (about where the tanks hung, its noses behind the intakes), rear pair -11.6 to 1.4 ft, its tails 0.6 ft past the tunnel's end (about -11 ft; was 2.5 ft past); 2.5 ft apart side by side, so the two pairs' fins clear by about 0.4 ft |
| 5, 250 gallon tanks x2 | 3.0 / 6.67 | 4.75 / 6.67 | Under the nacelles on 2 inch rails, under the lowest part of the intake ramp along the tank (6.5 ft either side of the record), so the tank does not cut into the ramp; the pylon is deeper at the front, where the ramp rises |
| 6, AIM-120 x2 (glove pylon) | 6.0 / 4.67 | 7.5 / 4.67 | 1 ft under the glove, just outboard of the nacelle |
| 7, AIM-9 x2 | 8.67 / 1.67 | 8.75 / 6.0 | On the glove pylon's shoulder rail ([rules](#rules)), its nose level with the AIM-120's |

Every load the ordnance screen allows on these rows hangs at the same
stations: the tunnel row's other stores (AIM-7s, bombs, AA-10s and the
rest) as tandem pairs or single stores 1.25 ft out, the glove row's under
the glove pylon, the AIM-9 row's (AIM-9B, AIM-9X, AA-2, AA-8, AA-11, R.550,
AT-12) on the shoulder rail. The release, the bomb sight and the AI fire
from those points, as everywhere ([released from](#rules)). Opinionated, John 2026-10-14;
the positions, the tandem gap and the shoulder rail's spacing are agent
decisions (fitted).

What still touches, measured with `tore-app --stores-preview` (its `clip`
lines: how far the deepest point of each drawn store reaches above the
model's lowest face straight below it), default load:

| Store | Before | After |
| --- | ---: | ---: |
| AIM-54 (tunnel) | 0.72 ft (into the nacelle beside it) | 0.27 ft (the tips of its upper fins into the tunnel ceiling) |
| AIM-120 (glove) | 0.21 ft (into the nacelle) | 0 |
| AIM-9 (shoulder) | 0 | 0.05 ft (since the rail sits above the main store: an upper tail fin tip into the glove) |
| Tanks | 0 (but inside the Phoenixes, same place) | 0 |

The Phoenix's fins are 3 ft across; turned to an X they reach 1.06 ft up
from its centre line, which on a 2 inch rail is 0.27 ft more than its body
radius and the rail together. Any finned missile on a 2 inch fuselage rail
does the same where its fins are wider than its body by more than the rail.
With bombs, tanks or a GBU-10 chosen for the glove pylon (beyond the real
F-14's loads) the shoulder AIM-9's body stays above them, though its lower
fins can touch a wide one. John accepts a little clipping (2026-10-14).

## The A-10

John, 2026-10-14, on a head-on picture: "the double pylons are attached to
the bottom of the gear/sensor things". Retail records row 6 (four
Mavericks, two a wing on a twin rail since nr-yak) 8.33 ft out, over the
main gear pod, so its pylon hung from the pod's underside, about 2 ft
below the other pylons. The real A-10's eleven stations are on the wings
and fuselage; the gear pods carry none (stations 3 and 9 are just outboard
of them, 2 and 10 between those and the outermost 1 and 11). From John's
marks:

| Record (A10.PT row) | Retail, ft out | T.O.R.E, ft out |
| --- | ---: | ---: |
| 5, twin rail inboard of the pod | 3.33 | unchanged (moved aside to 2.48) |
| 6, twin rail | 8.33 (moved aside to 7.83, under the pod) | 10.75, just outboard of the pod |
| 7, one a wing | 13.33 | 14.75 |
| 8, one a wing | 18.33 | unchanged |

Every pylon now hangs 1 ft under the wing; the `clip` lines read 0.
Opinionated, John 2026-10-14; the spans are agent decisions from his marks.

## The Su-27 and Su-35

John, 2026-10-14: the Su-27 and Su-35 should look realistic. The real
Flankers carry one missile a pylon: wingtip launchers, three underwing
pylons a wing (inner, middle, outer), a pylon under each intake duct and
two stations in the tunnel between the engines, one behind the other.
Retail's records (the same for both; SU35.PT is the canard Su-27M, the only
other Flanker in the game) put two AA-12s side by side on the centreline,
the next row 8 ft out beside the intake ducts, four wing missiles on one
row 17 ft out (two a wing, which nr-yak's rule hung on a twin rail), and
the wingtip rails.

| Record (row) | Retail position, ft out / forward | T.O.R.E stations | How |
| --- | --- | --- | --- |
| 5, centreline (Su-27 two AA-12s, Su-35 one AA-ML) | 0 / 4.0, side by side | The tunnel between the engines, centred 1 ft back | Two hang one behind the other on 2 inch rails, 0.25 ft apart (front 5 ft forward, rear 7 ft back); one hangs at the centre |
| 6, beside the intakes (AA-12s, AA-MLs) | 8.0 / 2.67 | Under each intake duct, 4.5 ft out, 2 ft forward | On 2 inch rails under the lowest part of the duct along the store, 6 ft either side (the Su-27's duct bottom slopes) |
| 7, wing (Su-27 four AA-11s, Su-35 four AA-12s) | 17.0 / -1.33 | The middle and outer underwing pylons, one missile each: Su-27 14 and 18 ft out, Su-35 13 and 17 ft out | A row with more stores than one real pylon a wing spreads to the neighbouring real pylon: two a wing are one on the middle pylon and one on the outer, the outer pair first; one a wing hangs on the outer pylon. Each pylon sits 0.75 ft further back for each foot out, along the swept leading edge; Su-27: middle 0.4 ft back, outer 3.4 ft back (rails about 1.5 ft behind the leading edge, clear of the model's drooped leading edge); Su-35: middle 0.6 ft forward, outer 2.4 ft back (rails at the leading edge) |
| 8, wingtip | 28.0 / -5.33 | Unchanged: on the tip's edge | |

The inner underwing pylons (about 10 ft out) have no retail row, so
nothing is drawn there. Every store the ordnance screen allows on these
rows hangs at the same stations. Clips measured with the stores preview's
`clip` lines, default loads:

| Store | Before | After |
| --- | ---: | ---: |
| Su-27 intake AA-12 | 1.30 ft (into the duct beside it) | 0 |
| Su-27 tunnel AA-12 | 0.54 ft | 0.54 ft (grid fins into the tunnel) |
| Su-27 wing AA-11 | 0.71 ft | 0.72 ft (the outer one's nose under the drooped leading edge) |
| Su-35 intake AA-ML | 0.38 ft | 0.38 ft |
| Su-35 wing AA-12 | 0 | 0 |

Opinionated, John 2026-10-14; positions are agent decisions (fitted).

## The Yak-141

John, 2026-10-14, with a picture of a Yak-141 from the side: "the wingtip
is actually a wing pylon, and both pylons are fully nestled under the
wing." Retail's records put the AA-10s 8.33 ft out at 0 ft forward and the
AA-11s 16.67 ft out at 0 ft forward. The model's wing is swept (its leading
edge about 0.8 ft forward at 7.5 ft out, 2 ft back at 11 ft out, 6.3 ft back
at 16.67 ft out; its trailing edge about 11 ft back; its tip about 18.5 ft
out), so the outer row was ahead of the wing and near the tip.

The real aircraft has two underwing pylons a wing, both well inboard under
the chord, the missiles' noses a little ahead of the leading edge. T.O.R.E
hangs the two rows there ([`station_move`](#source-notes)):

| Record (YAK141.PT row) | Retail position, ft out / forward | T.O.R.E station | Default store |
| --- | --- | --- | --- |
| 4, inner pylon | 8.33 / 0 | 7.5 / -2.8 | AA-10 (14.8 ft): rail 0.25 ft behind the leading edge, nose 4.6 ft ahead of it, tail 0.8 ft ahead of the trailing edge |
| 5, outer pylon | 16.67 / 0 | 11.0 / -4.4 | AA-11 (9.5 ft): rail 0.25 ft behind the leading edge, nose 2.35 ft ahead of it, tail 1.8 ft ahead of the trailing edge |

Both hang 1 ft under the wing, 3.5 ft apart. Every store the ordnance screen
allows on these rows (one a wing on each) hangs and leaves from the same
stations, centred where the default store is. Nothing of the default load
reaches into the airframe (the preview's `clip` lines read 0). Wide stores
on both rows of one wing (two GBU-10s, 5.4 ft wings) would touch wing tip
to wing tip. Opinionated, John 2026-10-14; the positions are agent decisions
(fitted).

## Performance

The stores join their aircraft's existing vertex batch (no extra draw call or
pipeline). A pylon is 48 vertices, a fitted pod or tank 144, a weapon shape its
own triangles; the model surfaces pylons meet are looked up once per point and
aircraft type. On a
16-aircraft formation each carrying six stations this is about 10,000 extra
vertices a frame.

## Source notes

- Hardpoint record layout and location names: [aircraft ordnance](../formats/aircraft-ordnance.md).
- Retail release position and the 0x4 mirror rule: [weapons formats](../formats/weapons.md#release-state)
  (FA.EXE 1.02F `HARDPos` `0x4532a0`, `HARDUnrotatedHardPos` `0x453220`).
- Code: `tore_sim::combat::hardpoints` (the rule, twin rails on a wing (`Rack::pylons`, `Rack::rack_offset`), tandem fuselage pairs,
  `station_move` and `placed_on` (the F-14's, Yak-141's and Flankers' stations, the shoulder rail, separate wing pylons),
  `Rack::store_point`, `slot_offset` and `shoulder` (the bar), `placed` and `Underside::nudge`
  (moving aside), the 1 ft pylon, `default_records` (the A-4E)), `tore-app/src/store_sizes.rs`
  (`fin_clock`, `carried_roll`, the fin roll in `variants`), `tore_sim::combat::store_sizes` (the sizes
  stores hang by), `Station::release_point`, `combat::live::cockpit`
  (`release_round`), `tore_world::ai_wings` (AI missiles),
  `tore_world::snapshot::store_poses`, `tore-app/src/pylons.rs` (drawing), `tore-app/src/store_sizes.rs` (real sizes), `tore_world::snapshot::pod_body`.
- Scenarios: `a2g-release-origin`, `net-a2g-stores` (a client sees another
  player's and an AI's stores empty) and `replay-a2g` (the stores recorded
  and replayed) in the battery.

## The F-15C

John, 2026-10-15: move the wing pylons aft and put the four fuselage
missiles on the lower outer intake corners, two in tandem on each side.
This layout is **opinionated**, requested by John. The exact coordinates
are **fitted**, agent decisions from the model and his supplied photograph,
not recovered original-game behaviour.

`F15.PT` is the roster's only F-15, the F-15C. No F-15E, ACTIVE or
conformal-tank station record exists in the supported roster.

| Source row | Before, feet out / forward | Fitted station, feet out / forward | Rule |
| --- | --- | --- | --- |
| 4, fuselage missiles | 8.33 / 0 | 5 / -1 | Lower outer intake corners; four missiles form two tandem pairs with the existing 1 ft nose-to-tail gap |
| 5, wing stores | 16.67 / 0 | 11.0 / 1.5 (16.67 / -5.5 until 2026-10-20) | Under the wing chord; four Sidewinders keep one small twin-rail bar per wing |

The duct bottom at 5 ft out is about 4.0 to 4.3 ft below the model's
reference point between 14 ft forward and 6 ft aft. The old source height
is only 3.33 ft below it, inside the duct. This row therefore reads the
underside starting 8 ft below the reference, rather than from that source
height. Its rails remain 2 inches deep; the upper fin tips may enter the
body slightly, as accepted for fuselage rails. The four missile release
origins are 5.5 ft forward and 7.5 ft aft, at 5 ft either side. The meshes
are not all centred on those origins: the AIM-120 extends from -2.67 to
9.33 ft relative to its origin, the Sparrow from -6 to 6 ft. Thus the
front AIM-120 spans 2.83 to 14.83 ft forward, the rear -10.17 to 1.83 ft;
the front Sparrow -0.5 to 11.5 ft, the rear -13.5 to -1.5 ft. Both keep
the existing 1 ft nose-to-tail gap. The front pair leaves first.

(Superseded on 2026-10-20: the pylon is now 11 ft out and forward, see
[the F-15C wing pylon](#the-f-15c-wing-pylon-moves-inboard-2026-10-20).)
At the 16.67 ft span the model chord runs approximately from 0 to 9 ft
aft. The release origin is 5.5 ft aft. The Sidewinder mesh's centre
is 1.21 ft forward of that origin, so the drawn pylon centre is 4.29 ft
aft, near mid-chord. Pylon depth remains 1 ft, the twin bar
and 45-degree fin roll remain unchanged, and tanks selected on this row
use the same aft position.

The source has no centreline carrier row. T.O.R.E now appends the
[derived tank stations](#derived-f-15c-and-f-16c-stations) requested by John.
The original missile rows retain their loading rules; their tanks still
replace weapons on those rows. The new wing tank row is independent.

The same table builds weapons, optional loads and tank positions. Flight,
network seats and newly recorded store layouts use those positions;
existing replays retain the positions saved in their recordings.

## The F-15C wing pylons, 2026-10-20

John, 2026-10-20: "there is a wing pylon that only accepts tanks; that should
be fixed (on the F-15)". The appended wing row (row 7, stations 2 and 8) took
only the 610 gallon tank (station mask 0x205), so the Load Ordnance page showed
its box empty and refused every weapon, although the real Eagle's wing pylon
carries air-to-air missiles on its launcher as well as the tank.
**Opinionated**, requested by John; the list is an agent decision.

| | Before | After |
| --- | --- | --- |
| Station mask | 0x205: tank | 0x2a5: tank, infrared and active radar missiles |
| Normal loading takes | One 610 (or 150, 250, 350, 500) gallon tank a pylon, two in all | The same tanks, or an AIM-120, AIM-9B, AIM-9M or AIM-9X a pylon (two in all, 90 class), never both on one row |
| Refused | Every weapon | AIM-7 and AIM-7E, AA-11, bombs, rocket pods, air-to-ground missiles and pods. Cheat loading keeps its unrestricted rule |

The row is exclusive like the F-22's inboard pair: choosing a tank empties the
missiles and choosing a missile empties the tank, through the same selector.
A missile hangs where the tank would, under the pylon, with the two Sidewinders
of row 5 on its shoulder rails above, so a full Eagle carries 4 fuselage
missiles, 4 shoulder Sidewinders and 2 more missiles or 2 tanks on the wing
pylons. The row gains a weapon slot (a fifth station after the centreline
bomb row), empty by default. A loadout saved before it has one station fewer
and still restores, the new station empty (`stations::later_weapon_rows`).
The station list, the checkpoint's station entries and a lobby loadout's
station count for the F-15C change with it; see the report for the protocol.

### The F-15C wing pylon moves inboard (2026-10-20)

John, 2026-10-20: "the wing pylons were below the paired pylon; the whole
thing needs to be moved inward, kind of like the real F-15". The wing pylon
(rows 5 and 7: shoulder Sidewinders, the tank or the AIM-120/AIM-9 under
it) was 16.67 ft out, at the wing's outer third. It is 11 ft out
(`F15_WING_OUT`, **fitted**, from John's marks on the model's 22.8 ft
semispan; the real Eagle's pylon is about 9 to 10 ft of 21 ft out), its top on the wing's underside there. John then asked for it forward,
"almost to the tip of the front of the wing": `F15_WING_FORWARD` puts row 5 1.5
ft ahead of the reference point (row 7's tank pylon 1.2 ft ahead of that, 2.7
ft). The model's wing at 11 ft out reaches 5.25 ft forward, so the pylon's
front (4.8 ft for the Sidewinder pylon, 5.2 ft for the tank's) stands just behind
the leading edge and the Sidewinders' noses (7.4 ft) a little ahead of it; the
tank's nose is about 5 ft ahead of the edge. Row 5 and 7 stand 7 ft further
forward than before (fitted). Both wings are symmetric.
Store release points move with it (stores leave from where they are
drawn); recorded replays keep the poses they recorded.

## The F-16C and fleet rail audit

John, 2026-10-15: put the F-16 underwing pylons under the wing, farther
aft, and review every aircraft's rails for obvious overhang. The F-16
changes are **opinionated**, requested by John; exact origins below are
**fitted**, agent decisions using the model chord and drawn weapon extents.
`F16C.PT` is the roster's only F-16 record.

| Source row | Real station grouping | Before origin, feet out / forward | After origin | Behaviour |
| --- | --- | --- | --- | --- |
| 5 | Centreline 5 | 0 / 0 | Unchanged | One tank, under the fuselage |
| 6 | Middle underwing 3/7 | 10.67 / -2.33 | 10.67 / -3 | One pylon per wing; paired stores retain the small twin bar |
| 7 | Outer underwing 2/8 | 14.33 / -3.67 | 14.33 / -7 | One pylon per wing, farther aft under the chord |
| 8 | Wingtip 1/9 | Source 18 / -4.67, fitted to the tip at 17 ft out | Unchanged | Stores sit on the model's tip launcher; AIM-9 or [AIM-120](#the-f-16c-wingtip-aim-120), one a tip |

The inner wet pair, real stations 4/6, has no source row. The
[derived station addition](#derived-f-15c-and-f-16c-stations) supplies it.
Tank selections on existing compatible rows retain their capacities and
replace the weapons on that row.

At 10.67 ft out the model wing runs from about 2.25 ft forward to 7 ft
aft. The Maverick's drawn centre is 0.34 ft ahead of its release origin,
so its pylon is about 2.66 ft aft, near the middle of that chord. At
14.33 ft out the chord runs from 0.75 to 7 ft aft. The AIM-120's drawn
centre is 3.33 ft ahead of its release origin, so the moved pylon lies
3.67 ft aft, near the middle. Its nose remains about 3.08 ft ahead of
the local leading edge: a 12 ft missile is longer than the 6.25 ft chord.
The pylon itself stays under the wing.

A geometry audit must measure **drawn mesh extents**, not assume that each
mesh is centred on the release origin. `--stores-preview` with
`TORE_STORES_PREVIEW_CPU=1 TORE_STORES_PREVIEW_AUDIT=1` prints each source
row, actual carried-store origins, local model chord, drawn hanger front
and drawn store front, without a GPU. It covers the selectable roster,
including internal/equipment-only records and optional weapon rows.
The quarter-foot chord samples are a screening tool. They do not establish
real aircraft carriage clearance, variable-sweep clearance or certification.
Long stores and tip launchers can extend ahead of a wing. Uncertain
real-world station mappings require references and a design decision,
not a blanket aft shift.

### The F-16C wingtip AIM-120

**Opinionated**, John, 2026-10-16: "On the F-16, the wingtip station
typically can have AIM-120s. Can we make sure that is enabled for the
wingtip?" The real F-16C's tip launchers (stations 1 and 9, LAU-129) carry
the AIM-120 as well as the AIM-9. The rule below is an **agent decision**.

Why it was refused: source row 8 is a wingtip station with mask 0x85
(infrared missiles only) and weight class 4 (400 lb, sized for two
190 lb AIM-9s). The AIM-120 is a 345 lb active-radar missile, so the
station mask turned it away, and a weight class of 4 would have allowed only
one. Raising the class would have changed the other infrared counts too
(a 230 lb AA-11 would go from one to two).

What changed: on the F-16C only, normal loading gives the AIM-120 on the
wingtip row the row's own count, **2**, which is one a tip, exactly what the
row holds for an AIM-9 today. The rule is
`aircraft::stations::extra_weapon` (read by `Loadout::capacity`, which every
caller shares: the Load Ordnance page and its drag and drop, the saved-load
and mission-text checks, the multiplayer lobby's loadout check and the AI
wingmen who copy their lead's loadout). It applies only to an F-16C record
whose guarded station rows were appended (so the reviewed source signature
matched, the same guard as the [derived stations](#derived-f-15c-and-f-16c-stations)).
Nothing else moves:

- every other store keeps the source mask and weight rule on that row (AIM-9
  two, AA-11 one, radar missiles other than the AIM-120 none, bombs none);
- the other F-16C rows, their order, station numbers and default load, and
  old saved loads, are unchanged (an old load simply has the AIM-9 there);
- Cheat loading already gave the row's raw capacity;
- the F/A-18D's tips stay AIM-9 only, as the real aircraft's.

A tip AIM-120 is placed by the same [wingtip rule](#rules) as the AIM-9: its
centre line on the tip's edge (17 ft out), no pylon. The 12 ft missile is
longer than the tip's 9 ft chord, so its nose stands about 3.2 ft ahead of
the tip's leading edge and its tail ends at the tip's trailing edge (the
outer-wing AIM-120 already shows about 3.1 ft ahead). The model's tip
overlaps the drawn missile by 0.9 ft at its worst vertex, against 0.4 ft
for the AIM-9; the audit's clip report shows both. Left and right alternate
as for any paired row, the missile leaves from the tip edge, and it flies the
AIM-120's own guided record.

Checked by `--validate-ordnance` (the F-16C tip block of `ordnance_audit.rs`:
capacity of every supported weapon, load, save, restore, two shots from the tip
edges), `--validate-wing-loadout` (wingmen and the lobby check) and a Load
Ordnance snapshot (`--snapshot-state ordnance-tip-amraam`).

Other aircraft with a wingtip row: the F/A-18D, Rafale C and X-31 tip rows have
the same infrared-only mask and stay as they are; the Su-27's tip row already
accepts the AIM-120 under the original mask, and the Su-35's is a fixed
station. These are listed for John, not changed.

### Mi-24 lower wing attachment

The fleet audit also found `MI24.PT` row 2 hanging its four AT-2s from the
rotor-height surface, at about +7.64 ft. Its source origin (8.33 ft out,
0 ft forward) is ahead of the stub wing, so the upward search sees the
rotor disk instead of triggering the generic off-wing fallback. The row
now sits 12 ft out and 5 ft aft, near the outer stub-wing ends, clear of
the model's fixed pods around 9.5 ft out. At that span the wing is about
1.3 ft below the reference. The existing twin bar hangs 1 ft below it;
its short AT-2 noses remain near the leading edge.

This is **fitted**, an agent correction under John's 2026-10-15 fleet
review request. The twin bar remains the game's simplified launcher;
the [US Army Mi-24 reference](https://odin.t2com.army.mil/WEG/Asset/Mi-24_%28Hind%29_Russian_Attack_Helicopter)
places the four AT-2s on two underwing rails. No rotor, flight controller,
weapon capacity or guidance behaviour changes.

### Apache stub-wing pylon attachment

**Fitted**, agent correction under John's 2026-10-21 pylon review request.
The renderer prefers a short attachment to the nearby wing over a distant
surface directly overhead. If its first surface is more than 2.5 ft above
the store lugs or rack bar, it checks up to 3 ft inboard in half-foot steps
and takes the first surface within that height. The 2.5 ft threshold is the
existing 1 ft wing depth plus the 1.5 ft surface-following tolerance. If
the mesh middle still sees only the distant surface, the hanger moves toward
the station origin in quarter-foot steps, up to 12 ft, stopping at the first
short attachment. This also handles the Apache's AT-12 (`SWNGR.SH`), whose
mesh middle is aft of the stub wing. The store itself never moves. When
there is no such nearby surface, the existing attachment remains. The rule
applies to every aircraft, including paired and empty pylons.

On `AH64.PT` row 6, the station sampler correctly finds the stub wing at
about -0.083 ft. Drawing starts higher, at the already placed missile, so
the rotor at about +6.33 ft enters the renderer's 12 ft upward search.
That directly overhead hit previously hid the nearby wing inboard, producing
a slab about 7.4 ft tall. The short attachment now meets the stub wing;
its top still sinks 0.02 ft into the model. Pylon length and surface-following
use the same selected surface. Store positions, release points, capacities,
weapon behaviour, simulation, protocol and replay formats are unchanged.

The supported default Apache load is two `LAU61.JT` rocket pods and two
`AIM9M.JT` missiles, plus its gun. Neither a Hellfire weapon nor an M299
four-rail launcher or M261 pod is supported by this loadout. This correction
does not relabel those weapons or add cosmetic missiles. Real Apache launcher
support remains separate work; these are fitted pylons for the game's
available stores, not a claim of exact real-aircraft carriage or retail parity.

The CPU stores preview checks all 37 selectable aircraft with default and
maximum current-candidate weapon loads, retaining installed tanks. This
includes the Mi-24, CH-47, V-22, AV-8, Yak-141, A-10, Su-25, A-7, C-130,
AC-130, Il-76, E-2 and E-3. Aircraft with no external weapon load stay clean;
the roster has no AH-1. `render-ah64-stub-wing-pylons` and
`render-ah64-stub-wing-at12` check the actual Apache hanger vertices. Synthetic
tests cover paired and emptied racks with a rotor above a stub wing, an
off-centre store mesh, and a genuine high wing without a lower surface.

### Derived smaller guided bomb models

The private GB12.SH, GB16.SH and GB38.SH aliases reuse donor geometry with
separate length, body diameter and fin-span refits. Pylon placement and
released-store rendering use the same size table. See the canonical
[smaller guided bomb specification](missiles.md#smaller-guided-bomb-stores)
for dimensions and provenance.

## Derived F-15C and F-16C stations

**Opinionated**, John, 2026-10-15: add the missing F-15C centreline and
independent wing tank carriers and the F-16C inner wet pair. These are
new gameplay capabilities, not recovered original-game behaviour.
Exact positions, load limits, tank shell masses and tank shapes are
**fitted agent decisions**. Defaults remain unchanged; all added rows
start empty. Retail files are never edited.

| Aircraft / appended row (zero based) | Real stations | Capacity | Origin, feet right / forward |
| --- | --- | --- | --- |
| F15.PT / 6 | Centreline 5 | One tank or up to three bombs, 4,500 lb | 0 / -3 |
| F15.PT / 7 | Wing pylons 2 and 8 | Two tanks, or two AIM-120 or Sidewinders (one a pylon), 9,000 lb total ([wing pylon missiles](#the-f-15c-wing-pylons-2026-10-20)) | +/-11.0 / 2.7 (16.67 / -4.3 until 2026-10-20) |
| F16C.PT / 9 | Inner wet 4 and 6 | Two stores, 6,000 lb total | +/-7 / -1 |

The F-15 can retain its four fuselage AIM-7/AIM-120 and four wing
Sidewinders while carrying three `F610.GAS` tanks. The Sidewinders sit
above the shared pylon foot on its two side rails, each 1.75 ft from the
pylon centre to clear the tank and the rolled fins. The F-16 row accepts
two `F370.GAS` tanks or compatible bombs/missiles, including Mk 82 and
AGM-65G, with one store per wing. It uses the existing loading masks,
not a separate release path. Tank and weapon selections exclude each
other on the F-16 wet row. The [2026-10-16 addition](#optional-carriers-2026-10-16)
enables F-15 centreline bombs as requested gameplay, with fitted limits.

A 610 US gallon tank contains 4,026 lb fuel and a fitted 432 lb shell;
a 370 US gallon tank contains 2,442 lb fuel and a fitted 262 lb shell.
Fuel density is the existing 6.6 lb/gal class convention. Shell mass
scales the existing 350 gallon class's 248 lb shell. The normal selector
limits the new tank identities to the above carriers; Cheat retains its
existing unrestricted rules. Existing tank types keep their old rules.
Full F-15 additions total 12,078 lb fuel plus 1,296 lb shells. Full F-16
additions total 4,884 lb fuel plus 524 lb shells, independent of its
existing centreline tank.

Heights sample the model underside with 1 ft wing pylons and the existing
2 inch centreline rail. The F-15 centreline search starts 8 ft below the
reference and the F-16 wet pair 6 ft below. Tank bodies keep the existing
volume-scaled shape rule; no exact real tank contour is claimed.
Weight, external-store drag, fuel-first consumption, Shift+K tank
jettison, Ctrl+Shift+K selected weapon jettison, AI mission loadouts and
recorded store poses use the shared configuration. No AI decision rule
changes. Older saved loads retain their weapon indices and leave added
rows empty.

The parser appends only when every original station field matches the
reviewed F15.PT or F16C.PT signature. Edited rows suppress the addition.
The nine serialized fuel banks stay unchanged: F-16 source row 9 uses
bank 0, unused by its fixed visual sensor. Duplicate bank allocations
are rejected. Equipment damage still refers to source rows, so a visual
sensor hit does not empty the added tanks. This added row has no new
retail subsystem-damage roll. Peers must agree on derived resources,
row definitions, fuel-bank assignment and placement. No protocol number
or checkpoint layout changes in this slice.

Real-carriage context: the [USAF F-15 fact sheet](https://www.af.mil/About-Us/Fact-Sheets/Display/Article/104501/f-15-eagle/level/f-15-eagle/)
describes three external tanks; [Kunsan maintenance](https://www.kunsan.af.mil/News/Article/2440224/maintainers-construct-external-fuel-tanks/)
identifies 370 gallon F-16 wing tanks. The requested 610 gallon Eagle
class is used as John's requested size, not a measurement from the game.

### Source limitations behind the optional carriers

The 2026-10-15 roster screen compared the 36 supported retail identities'
source carrier rows with documented real tank carriage. The [2026-10-16 addition](#optional-carriers-2026-10-16) implements selected
candidates below. The table records the source limitations,
not evidence of original-game behaviour or a certification review.

| Aircraft | Source limitation | Real-carriage evidence and next step |
| --- | --- | --- |
| MIG29.PT, Fulcrum-C | Rows 5 to 7 are three wing pairs; no centreline carrier | The [German MiG-29 flight manual](https://jasonblair.net/wp-content/uploads/2015/06/MIG29-Flight-Manual-Pt-1.pdf) describes a 1,500 litre centreline tank. C-variant gun-clearance restrictions remain unknown; the requested gameplay addition is specified below. |
| A10.PT | Rows 5 to 8 are wing weapon groups; no centreline carrier and no tank permission bit on those rows | [USAF](https://www.127wg.ang.af.mil/Media/Articles/Article/865880/selfridge-angb-home-of-the-drop-tank/) confirms external tanks for long-distance flight. [Eglin](https://www.eglin.af.mil/News/Article-Display/Article/814477/40th-fts-expands-a-10-fuel-limitations-in-combat/) distinguishes the later 600 gallon combat-clearance work. Review period A-10A stations and ferry restrictions. |
| RAFALE.PT, Rafale C | Centreline row 4 is occupied by AAS38.SEE and retained as equipment; wing rows 6/7 lack tank permission | [Dassault](https://www.dassault-aviation.com/en/defense/rafale/adapt-and-deliver/) specifies five heavy/wet stations. The addition below retains the sensor and adds an aft centreline tank carrier; exact period tank contours remain unknown. |
| F22.PT | Only internal carrier rows 5 to 7; none has tank permission | [USAF](https://www.af.mil/About-Us/Fact-Sheets/Display/Article/104506/f-22-raptor/) specifies two external wing fuel tanks. The addition below fits the prototype model; The naval donor now shares this carrier under the [F-22 naval stores contract](#f-22-naval-stores). |

F-14, F/A-18D, A-4E and F-4 family source groups already represent the
centreline or independent tank positions relevant to this screen. A-7,
AV-8, MiG-17, MiG-21, MiG-23, F-104 and Su-25 have tank-capable carrier
rows, although class sizes and grouped weapon/tank tradeoffs still need
variant-specific review. Su-27/Su-35 and experimental/rotorcraft entries
were not declared deficient merely because their row counts differ from
real station counts. A grouped row can represent several physical
stations. No claim of exhaustive real-world certification coverage is made.

## Optional carriers, 2026-10-16

Implementation specification, **opinionated**, requested by John on 2026-10-16.
These additions are not claims about original-game behaviour or real aircraft
certification. Limits, coordinates and tank shells below are **fitted agent
choices**. A-10 and all default loads remain unchanged.

- F15.PT centreline row 6 accepts one 610 gallon tank or up to three bombs,
  within 4,500 lb total. The F-15C's source rows carry only air-to-air
  missiles, so the rack's list is its own: the station mask 0x3c1 (tank 0x200,
  unguided and optical 0x100, laser 0x80, SARH 0x40) filtered to bombs and
  guided bombs only (`Loadout::capacity`). Normal loading accepts Mk 82 (3),
  Mk 82 high drag, Mk 82 three-pack, Mk 20, CBU-87, CBU-89, FAB-250, FAB-500,
  RBK-250, RBK-500, GBU-12, GBU-16, GBU-38 and GBU-30 (3 each), Mk 84,
  GBU-10, GBU-10A, Paveway, Paveway III, GBU-29 and GBU-29/P (2, by weight),
  and FAB-1000, GBU-28, Mk 82 pack and Mk 82 AIR (1, by weight). Missiles,
  rocket pods and designator pods are refused; Boulder and Moose load only
  under Cheat. A three-position rack has two bombs beside its bar and one
  below, between the fuselage missiles.
- F22.PT row 8 is a paired inboard wing carrier: two physical pylons represented
  by one screen row, four AIM-120 or AIM-9 missiles (two per pylon), or two
  600 gallon tanks (one per pylon), within 9,000 lb total. Tank and missile
  selections exclude each other. No bomb or air-to-ground missile permission.
- MIG29.PT row 8 adds the missing centreline carrier for one 1,500 litre tank,
  within 3,000 lb. Rows 5, 6 and 7 already represent three pylons per wing.
  No duplicate wing pylons are added. Later-variant wing-tank certification is
  not assumed for the Fulcrum-C. Cannon operation is unchanged, an opinionated
  omission of variant-specific tank/gun restrictions pending C-model evidence.
- RAFALE.PT row 9 adds one centreline carrier for a 2,000 litre tank, within
  4,000 lb. This uses one additional loadout row and supplies useful fuel
  capacity while preserving the existing AAS38 sensor equipment and wing loads.
- F22N.PT now shares the bay and wing carriage below. Its original source
  rows were external fuselage carriers; this is an explicit gameplay override.

| Aircraft / appended row (zero based) | Real stations | Capacity | Origin, feet right / forward | Surface read upward from |
| --- | --- | --- | --- | --- |
| F15.PT / 6 | Centreline 5 | One tank or three bombs, 4,500 lb | 0 / -3 | 8 ft below the origin |
| F22.PT / 8 | Inboard underwing pair | Two tanks or four missiles, 9,000 lb total | +/-12 / -6 | 6 ft below the origin |
| MIG29.PT / 8 | Centreline, between the engines | One tank, 3,000 lb | 0 / -2 | 8 ft below the origin |
| RAFALE.PT / 9 | Centreline, behind the AAS-38 | One tank, 4,000 lb | 0 / -4 | 8 ft below the origin |

The new rows also accept the retail tank records that fit the weight class
(F150, F250 and F350 on all three; F500 on the F-22 and Rafale), as the
F-15C and F-16C rows do. Tank counts are capped at one per physical pylon.

Authored records are F600.GAS (600 US gal, 3,960 lb fuel, 425 lb shell),
M396.GAS (1,500 litres rounded to 396 US gal, 2,614 lb fuel, 281 lb shell),
and R528.GAS (2,000 litres rounded to 528 US gal, 3,485 lb fuel, 374 lb shell).
Fuel is rounded to whole pounds using 6.6 lb/US gal; shells scale 248/350 lb
per gallon. These are fitted game classes, not measured tank masses. Each is
added only to a pack containing its carrier; explicit pack records win.
Normal loading restricts each authored tank to its requested carrier row.

Every source row must match its reviewed signature before appending rows.
Old saved loads preserve existing indices and leave new rows empty. The
nine fuel banks remain unchanged: Rafale row 9 uses bank 0, occupied by fixed
sensor equipment rather than fuel. MiG-29 and F-22 row 8 use bank 8.
Fuel consumption, tank and selected-store jettison, AI mission loadouts,
network configuration and replay poses use the shared loadout paths. Peers
must agree on the new resources, compatibility, capacities and station poses.
Current compatibility is covered by the [protocol version rules](../formats/net-protocol.md#versions).

Real-carriage context: [USAF F-22 fact sheet](https://www.af.mil/About-Us/Fact-Sheets/Display/Article/104506/f-22-raptor/)
identifies two external wing tanks. The [German MiG-29 flight manual](https://jasonblair.net/wp-content/uploads/2015/06/MIG29-Flight-Manual-Pt-1.pdf)
describes a 1,500 litre centreline tank, but is not C-model certification.
[Dassault](https://www.dassault-aviation.com/en/defense/rafale/adapt-and-deliver/)
identifies 14 Rafale hardpoints, five suitable for heavy stores and tanks.
John's requested configuration defines the gameplay scope where period or
variant-specific evidence is incomplete.

## The Phantoms' gun pod, 2026-10-16

Implementation specification, **opinionated**, requested by John on 2026-10-16:
"There is a gun pod missing from the Phantom B, at least as an option to load.
I think the other variants had gun pods available to them as well." The records,
the real-aircraft basis and every choice below that John did not name are
**agent decisions**, for his review. They are not claims about original-game
behaviour.

**What the data holds.** The import has one gun pod record, `SUU16.JT`
(SUU-16 gun pod, 1,200 rounds, 1,702 lb). There is no Mk 4, SUU-23 or other
pod record. The Phantoms' source stations:

| Phantom | Built-in gun | Gun pod before | Gun pod now |
| --- | --- | --- | --- |
| F-4B (`F4B.PT`) | none | none | SUU-16 on the centreline, optional (below) |
| F-4J (`F4J.PT`) | none | SUU-16 on its own fixed row 3 (loaded by default; the page can take it off, but the row takes nothing else) | unchanged; now drawn and fired from its nose |
| F-4E (`F4E.PT`) | M61 | none | none (open question, below) |
| F-4G (`F4.PT`) | M61 | none | none (as the F-4E) |

**The F-4B's centreline.** The B's source row 3 is its centreline station, which
carries four AIM-9Bs by default and takes the other single-rack stores (bombs,
rocket pods). The SUU-16 now loads there, one pod in place of whatever the row
carries, through the normal Load Ordnance selector (it is listed on the
air-to-air page with the other weapons; the card reads "SUU-16 Gun Pod, 1
loaded (max 1)"). A real Phantom's centreline is one station, so the pod
replaces the row's stores rather than adding a second centreline row: the
default load (four AIM-9Bs, four Mk 82s on the fuselage pairs, four on the
wing pair and two 350 gallon tanks) is unchanged, and taking the pod off
restores the row's stores. Carrying it, the B weighs 53,416 lb, against 52,394 lb
without it (the pod's 1,702 lb replaces four 170 lb Sidewinders; the same
weight accounting as the F-4J's pod). It starts empty-of-pod; saved loadouts
and mission files from before carry the row's own weapon and load as they did.

- **One pod a station**, Cheat loading too (`Loadout::capacity`), and only on the
  B's row 3 or a row whose source record names the pod (the F-4J's): the pod
  is refused on every other station.
- **Ammunition and mass** follow the F-4J's fitted interpretation: 600 logical
  units for the record's 1,200 rounds (two rounds a unit), the 1,702 lb pod
  counted in the weight whether or not it holds rounds, until it is removed or
  jettisoned. A pod row with a missile on it carries no pod: the entry in the
  configuration (`GunPod`) counts only while the station's weapon is the pod
  (`Configuration::gun_pod`).
- **The Phantom B's stand-in.** The real US Navy pod was the Mk 4 (a 20 mm
  M61A1 pod, 750 rounds); the data has no Mk 4 record, so the SUU-16 (the
  same gun in a ram-air driven pod, 1,200 rounds) stands in, with its record's
  numbers. Agent decision, 2026-10-16.
- **Selecting and firing.** The pod is a gun (`AircraftId::mounts_gun`, the
  Guns only rule): GUN on the selection ring picks it, Tab fires it, with the
  aircraft's own gun first where it has one (none on the B or J). The F-4B has
  no installed gun, so `AircraftId::F4B.guns()` stays empty and
  `gun_pods()` names the pod it can carry.
- **Where the rounds start.** At the nose of the pod as it hangs, under the
  centreline: the drawn pod's middle (`Station::release_point`) and half the
  pod's 15.7 ft length ahead (`Station::muzzle`, `gun_mount`), for the player,
  the AI, the gun sight and the AI's ground-gun cue alike. The F-4J's pod
  fired from its record's position, 3.3 ft ahead of and 6.7 ft below the
  aircraft's reference point; it now fires from the same place as the B's,
  on the belly (agent decision).
- **Drawing.** The pod is a store on its pylon like a rocket pod: a fitted body
  15.7 ft by 1.8 ft (the real SUU-16/A is about 16 ft by 22 inches; fitted,
  `store_sizes::pod_body`), 2 inches under the body (a centreline rail),
  drawn on both Phantoms that carry it (the F-4J's pod was not drawn before) and
  in outside views, replays and on every network client from the same
  `Stores` event. It stays while it has no rounds and leaves only when it is
  jettisoned (Ctrl+Shift+K, with the selected store).
- **External stores.** The pod counts as one external store of 1,702 lb in the
  radar signature, as the F-4J's does; a pod row holding missiles counts the
  missiles.
- **AI wingmen** carrying the lead's flight loadout carry the pod, fire it as
  a gun (600 units, from its nose) and keep it through a restart. No AI default
  load changes: AI Phantoms keep their source stores.

Weight, the nose position and the F-4J's centreline are checked by the
`--validate-ordnance` pod audit ("gun pod F4B.PT: ..."), the wing check in
`--validate-wing-loadout` and the pod snapshot (`ordnance-gun-pod`).

**Limitation.** The F-4J's centreline row (source row 4, empty by default) does
not take the pod: the J already has its own. A J carrying its pod and a centreline
store on row 4 draws both on the belly, one through the other (the real
aircraft's centreline is one station). Not prevented; recorded here.

**Open question for John: the F-4E and F-4G.** Both are built with an M61. Real
F-4Es could in principle carry a centreline SUU-23/A, but the evidence in hand
is thin and a second gun on an aircraft that has one adds little, so neither
gets a pod. Adding one needs a new centreline row (the E's centreline row 4 is
a tank row with no unguided-store bits), appended and signature-guarded like
the rows above; the SUU-16 would stand in for the SUU-23 (there is no SUU-23
record).

**Wire.** Peers must agree on which rows take the pod and its capacity (the F-4B's
row 3, one pod) and on the `Stores` event listing a gun pod station with one
store while it hangs. A host and client that disagree refuse or misdraw a
Phantom carrying a pod. The lead assigns the protocol change.

## F-22 naval stores

Implementation specification, **opinionated**, requested by John on 2026-10-21.
F-22N uses the F-22A's three bay rows and optional wing pair. This is the
requested game configuration, not a claim of retail parity or certification.
The reviewed F22N.PT signature is checked before correcting rows 5 to 7 to
internal bays (location 3, flags 0x5e0, 0x5e0, 0x5e4) and appending row 8.
Edited source records keep their own definitions. Gun position, sensors,
flight characteristics, hook and exact model identity stay aircraft-specific.
The existing F/A-XX consumer inherits its F22N.PT donor's corrected carriage.

Default loads remain 750 gun rounds, two AIM-120, two AGM-65G and four AIM-9X;
the wing row starts empty. Row 8 offers four AIM-120 or AIM-9 missiles, or two
600 gallon tanks, with the same 9,000 lb limit and mutually exclusive selection
as the F-22A. Existing saved loads keep their row indices and new rows empty.
Load Ordnance lists gun, left main bay, right main bay, paired side bays and
wing pair in that order. Each identity keeps its own imported aircraft art.
AI loadout construction uses the same corrected carrier definitions; autonomous
flight and weapon-selection policy are unchanged.

The following geometry is **fitted, agent-authored**. Both reviewed shapes have
the central belly at 3 ft below the reference point. Their distinct belly and
intake polygons are cut and animated using their own materials. Main bays retain
the source lateral boundaries: left x -8/3 to -2/3 ft, right x 2/3 to 7/3 ft,
forward -8 to 17 ft (extended aft to fit tandem stores). Their recess depth is
2.5 ft to contain the default loads.
Side openings are x +/-3 to +/-4.9 ft and forward -6 to 46/3 ft, cut from
the sloping intake skins. Each side has one outward-opening door and a lined
recess roof at 0.4 ft up on F-22A, 0.6 ft on F-22N. This side-bay fit is not
recovered original animation.

| Carrier | Magazine centre x / floor up / release origin forward, feet |
| --- | --- |
| Left main, row 5 | -5/3 / -3 / 1.4 |
| Right main, row 6 | 1.5 / -3 / 1.4 |
| F-22A paired sides, row 7 | +/-3.9 / -1.2 / 1.7 |
| F-22N paired sides, row 7 | +/-3.9 / -1.05 / 1.7 |
| Wing pair, row 8 | +/-12 / own model underside minus the 1 ft pylon and store radius / -6 |

A bay magazine places two stores front to back, separated by their length
plus 0.25 ft. Their common height is the floor plus half the larger of the
fin span and body diameter, plus 0.08 ft. The forward store leaves first;
the side pair alternates left then right. Stores keep their positions as
the load empties. This fits the default/full candidate loads;
larger alternative weapons, including normally compatible guided bombs,
retain their eligibility but have not received enclosure-fit acceptance.
The ordinary compatibility masks and quantities stay unchanged.

Bay weapons are drawn inside, without exterior pylons, and leave from exactly
the same centres. Fully closed doors occlude their drawing, including through
masked pixels in the imported skin. Their counts and positions remain in
network snapshots and replay records while hidden. The [bay trigger timing](aircraft-animation.md#f-22-main-weapon-bays)
applies to bay weapons only. Wing missiles and the gun fire with doors closed
and do not request opening. Main and side doors share that existing timing.
Jettison still permits selected unfixed weapons, including bay stores, and
retains the existing inert falling-store rule. Internal bay stores do not add
external-store radar signature; wing stores and tanks do. The open-bay radar
term remains unchanged. Network clients and replays carry the same store
counts and magazine layout. Protocol 64 rejects incompatible live peers;
existing replay records retain their recorded layouts and still load.
