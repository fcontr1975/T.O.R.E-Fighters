# In-flight map

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation specification, 2026-09-21. This is an opinionated addition requested
by John, not a claim of retail behavior. Shift+M opens the map. All object
categories have right-side map toggles, without settings in the Escape menu.
Buildings default off, as requested by John on 2026-09-21. Known
locations and aircraft-detected signatures appear together. Unidentified
contacts use placeholders. Original map art and fonts are reused where available.

## Display and controls

Agent decisions: a north-up map follows the player, initially showing 100
nautical miles across its 458-pixel-wide plotting area on a 640 by 480
reference canvas (the nearest zoom step when the theater is narrower).
The map zooms and scrolls exactly as the Quick Mission [mission map](quick-mission-menu.md#mission-map)
does (slice CL-MAP, John 2026-10-12, reusing the creator map's view code):

- **Zoom** steps are the in-game 800, 400, 200, 100, 50 and 25 nautical miles
  across, less any that are wider than the theater, plus the widest step of
  all, the one at which the theater just fills the plot. Plus and minus step
  through them; a vertical retail rocker (the debrief's) in the side rail does
  the same, its top half zooming in and its bottom half out, with ZOOM IN and
  ZOOM OUT beside it.
- **Scrolling** uses a horizontal bar under the plot and a vertical bar beside
  it (arrow buttons move the view an eighth of its size, the thumb drags, a
  click on the track pages by seven eighths), as well as the arrow keys,
  which pan by one quarter of the displayed width. The bars are drawn flat in
  the menu greys; neither retail's in-game map nor its creator has art for
  them. `opinionated`, John 2026-10-12.
- **No black border at any zoom.** The view never shows beyond the theater:
  at the widest zoom the theater fills the plot and every pan is held inside
  its edges, so a map picture always fills the plot. While following, the
  view is centred on the player as far as that rule allows, so near an edge
  the player's symbol is off centre.
- **Following.** Scrolling by hand (the arrow keys, the bars) stops the map
  following the player; Home resumes following. Zooming keeps whichever mode
  it is in.

Shift+M or Escape closes the map. Flight continues at the existing 120 Hz rate;
opening the map does not pause, steer, designate or fire. Ordinary flight
controls remain available. Map clicks never operate the instruments underneath.
Heading, speed in knots, altitude in feet, a distance bar and player marker
remain visible. One nautical mile is 6,076 feet, matching the sensor component.
Crowded symbols move in 34-pixel steps up to 102 pixels with a leader to their
actual position; overlapping labels are suppressed. Theater edges are dark,
not repeated terrain. Markers outside the view are hidden.

**Off the map.** An object that lies beyond the theater (a plane flown past
the edge, a contact, a start, the objective area) is drawn pinned at the map's
edge, at the point of the edge nearest it, with the small helper words
**OFF MAP** beside it, so the player sees which way it lies. The symbol is
held just inside the plot's frame. It shows only while that part of the edge
is in view; scrolling or zooming out to it brings it into view. `opinionated`,
John 2026-10-12.

## Mission brief tab

Opinionated addition requested by John, 2026-10-22. The page has **Map** and
**Mission** tabs. Agent presentation decisions: the tabs sit below the existing
plot so its size, scale and controls stay unchanged; Enter or controller A
switches tabs. Clicking a tab selects it. Map zoom, pan, following and category
choices survive a tab switch. Shift+M, Escape or controller B closes either tab.
Tab keeps its gun binding. The page never pauses a multiplayer flight.

The Mission tab reads the Quick Mission creator's shared sentence builder,
including filled variables, wing types, counts, skill, objectives, survival,
theater, condition, starts, separation, ground target and defenses. It uses the
creator's font, field colours and bevels, with no editable fields. Single player
keeps the sentences of the accepted setup that built the flight, including the
creator-only situation text. Multiplayer reconstructs the host's MissionSpec,
including Player Choice wings and their AI defaults, plus the session settings.
This also applies to dedicated servers. The same read-only creator wording marks
fields the network mission does not carry as the King's. Free flight explicitly
says that no Quick Mission setup is available.

Agent layout decisions: read each side's sentences down one column, then the
mission settings, then multiplayer rules. The rules show mode, respawn, lives
(revivals), revive delay, time and kill limits, friendly fire, AI and minimum
players. Twenty-four rows fit; longer text wraps and scrolls. Up/Down or the
controller D-pad scrolls one row, Page Up/Down moves 24 rows, Home/End reaches
the first/last page, and the mouse wheel scrolls. The red retail scroll thumb
can be dragged; clicking its track moves a page. Scrolling stops at the ends.
Controller A/B and the D-pad operate the page without also changing aircraft
gear, brakes, instruments or the gunship console. Other flight controls stay live.

John's corrected scope, 2026-10-22: **Mission...** appears only on the in-flight
spawn/respawn loadout and Player Choice aircraft pages. It is also a selectable
row on each page, reachable with keyboard or controller navigation; M opens it
directly. This opens the same brief with its own Close button. Escape,
controller B, Enter or controller A returns to the unchanged loadout page.
The pre-flight creator's Load Ordnance and lobby Loadout pages have no new
button. The replay viewer has no in-flight map, so it has no new brief tab.

## Knowledge and detection

All imported runways are known map locations. Other entities require a current
observation from the selected operating radar/IR channel or the visual sensor.
The map uses observed positions, never hidden live positions. Visual observation
identifies the entity; radar/IR alone gives an unknown aircraft or surface marker.
This identification rule is an agent decision. A broad surface placeholder covers
unclassified ground objects, vessels, AAA and SAMs rather than inventing a type.
Visually identified surface objects use their imported names where available.
Unknown contacts do not expose aircraft identity, object name or allegiance.
Lost contacts disappear at the next simulation step. Destroyed objects retain a
marker only while detected, marked DESTROYED. Since 0.2.2 a hidden shoulder-fired
SAM team reaches the map only once this pilot has spotted it
([hidden shoulder-fired teams](air-to-ground.md#hidden-shoulder-fired-teams);
T.O.R.E opinionated, John 2026-10-29).

Surface units known by sight show what they are: a visually identified SAM or
AAA unit, tank, truck, radar or ship takes the `MCICONS.PIC` symbol of its kind
on the tile of its side, the sheet's blue row for the viewer's own side and its
red row for the other (agent decision, 2026-10-10; which symbol means which was
read from the sheet: gun, launcher, tank, truck, radar building, ship, generic
cube). A unit with no side keeps the grey tile that claims nothing. Radar-only
contacts stay the yellow unknown placeholders and expose no allegiance. The
symbol and side come from the mission's surface units by id; nothing the player
has not seen is shown.

Airfields show their side (opinionated, John 2026-10-12: "if there are known
allegiances for airports (set by the mission) those should be visible in the
map"). Each airfield symbol sits on the tile of its airport's side against the
viewer's own: the blue row for a friendly field, the red row for a hostile one
and the grey tile for a neutral field, the same look as a surface unit. The
side is the one the world flies with, the
[airport allegiance](airports.md#allegiance) the layout gives every airport. A
mission does not change it: Quick Mission and networked missions only choose
which fields a ground start may use, so no field is ever unknown to the pilot
and the map reads it without any detection. The scene records it from Blue's
point of view; a Redfor pilot (a networked game's Redfor seat) sees friendly
and hostile swapped, as the tower and the wing's landing orders already do.
The airfields a start names (BLUE START, RED START) are drawn as the start
symbols described under [Mission markup](#mission-markup), on the same
viewer-relative tile; the creator's mission map draws its airfields on blue,
red or grey tiles by the Blue pilot's view.

## Category controls

The right rail has six buttons: Aircraft, Airfields, Buildings, Surface,
Emitters and Mission. All start on except Buildings. The player is always
visible. Clicking
a button changes display only; it never changes detection or weapon support.
A press and release on the same button is required. Closing/reopening retains
the selections; restarting a flight restores the defaults.

Agent grouping decision: Buildings covers an explicit allowlist of imported
structural object resources, including city blocks, houses, hangars, barracks,
bunkers, factories, storage, towers, bridges and fuel structures. It also hides
unidentified returns belonging to those resources. It does not identify them to
the player. Runways use Airfields, while other ground objects, unclassified
surface contacts, vessels and defenses use Surface. SAM sites and radar
installations are excluded from the building allowlist. No category is inferred
from a damage-class number. This is an authored display grouping, not a recovered
retail filter contract. Surface subcategories remain grouped until their metadata
supports a narrower classification.

The rail occupies x=500..628 on the reference canvas. Six 120 by 30 buttons
start at (504,76), spaced 44 pixels apart; the zoom rocker stands below them at
(582,372). The plotting area is (12,30,458,390), with the scroll bars at y=422
and x=472.

Surface detection is fitted: reuse the installed sensor's existing search
geometry, signature/range rules, terrain masking, power and damage gates, solely
for map observations. Surface observations confer no air-to-air designation or
weapon support. The airborne scope and its selection rules stay unchanged.
No SAM/AAA spawning, targeting or autonomous behavior is introduced. A mission
must actually contain an entity before it can be detected or plotted.
Bearing-only passive signals get an UNKNOWN EMITTER bearing ray, never an exact
position. The ray is a direction guide, not a measured range.

## Mission markup

Opinionated, requested by John on 2026-10-12: "make sure the inflight map also
shows the lines, and approximation where objectives are (similar to the mission
screen). Essentially this should be map mark up specific to the mission." The
**Mission** layer draws, for a Quick Mission or a networked mission, what the
creator's [mission map](quick-mission-menu.md#mission-map) shows, built by the
same function from the same mission data:

- **The approximate objective area**: the hashed 5 nautical mile circle whose
  centre is 0.5 to 2.5 nautical miles off the targets' centroid, drawn in the
  mission map's look (yellow hatching and ring with dark edges, at least 7
  pixels in radius) and labelled OBJECTIVE AREA. The exact targets are never
  drawn by this layer; a target the player's sensors have found appears
  through the ordinary contact layers (Surface), as before. A mission without
  a ground target has no area.
- **The starts**: BLUE START and RED START, each the airfield's name for a
  ground start (the airfield symbol is then not drawn again) or the airborne
  point with its heading, plus BLUE WING n or RED WING n for another wing that
  starts more than 3 nautical miles from its side's start. The creator's map
  shows both starts to everyone, so the flight map does too. The words stay
  BLUE START and RED START, but the tile is the viewer's: the viewer's own
  side's starts (and its WING n markers) are on the friendly blue tile and the
  other side's on the hostile red one, as for airfields and surface units
  (John 2026-10-12, agent wording). A Blue pilot sees BLUE START blue and RED
  START red; a Redfor pilot sees RED START blue and BLUE START red. The
  creator's map is the Blue pilot's view, where the two colourings coincide.
  Each machine of a networked game
  builds the same mission, so each sees the same markup.
- **The route**: a thin line from the viewer's own side's start to the edge of
  the objective area, in the map's one line style (a one-pixel ink line over a
  dark edge, as the leaders and bearing rays are). The creator's mission map
  drew no lines, so this route is new; it is drawn there too, from the Blue
  start, so the two maps look alike. A start inside the objective area has no
  route to draw (Red begins within 5 nautical miles of the target).
- The markup is the mission as it was built, not live positions: it does not
  move when the aircraft do, it is the same on every machine and in every
  rebuild of the same mission, and a restart shows it again. A free flight has
  none, and its Mission button does nothing. The replay viewer has no in-flight
  map, so a replay shows no markup.

The Mission button toggles the layer like the other categories, and closing and
reopening the map keeps the choice. Provenance: the creator map's numbers are
`fitted` and `opinionated`, as recorded there; the route and the off-map words
are `opinionated`.

## Visual provenance and unknowns

The supplied screenshot is a visual reference for the gray frame, terrain,
labels, distance scale and original square symbols. It does not establish
retail detection, projection or timing rules. The map uses the loaded theater's
original briefing image. Its full raster maps
onto the T2 grid extents, with image north at the top and world positive Z north.
This registration is fitted, not a recovered retail projection. T2 cell colors
are the fallback, with water index 255 displayed as RGB 65,119,137.
MCICONS.PIC supplies player, runway and identified object symbols when imported.
Original aircraft fonts supply labels. Missing artwork has geometric fallbacks
so existing caches remain usable. Icon choices and crop coordinates are agent
presentation decisions following visual inspection of the sheet.

[Implementation validation](../baselines/flight-map.md).

Exact retail map projection, category classification, identification ranges and
contact persistence are unknown. Next research: inspect bounded map data and
manual descriptions only when those player-visible details are needed.
