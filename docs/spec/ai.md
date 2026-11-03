# Aircraft and surface AI behavior

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Research and implementation, 2026-09-17. This is the main behavior
specification for AI work. It records the behavior established so far and the
inputs needed to reproduce it. It is partial, not a claim that the complete AI
is recovered or implemented.
The scope covers [all twelve ported aircraft](ai-experience.md#currently-ported-aircraft),
the wider aircraft families and surface objects. Experience values and family
bindings have one home in [AI experience](ai-experience.md).

Evidence locations are indexed by the behavior IDs below in the
[source map](../formats/ai.md#behavior-source-points). Build identities and
inspection results live in the [baseline](../baselines/ai-research.md).
Sequencing and completion criteria remain in [M1e](../ROADMAP.md#1e-ai).

## Evidence and implementation status

**Executable-confirmed** means the relevant arithmetic or condition was
inspected in the hashed FA executable. **Source-confirmed** means an active
statement in the shipped AI text establishes the rule. Source confirmation
does not claim full agreement with the compiled BI. Comments, disabled blocks
and suggestive names are not behavior evidence. Both are research evidence
for future spec-derived code, not instructions to copy original internals.

The API below is an **opinionated agent proposal**. The established slices
are implemented as isolated components in `tore-sim::ai` (see
[implementation status](#implementation-status)); Quick Mission connects them to live actors, with reviewed combat integration
gaps still remaining. Unknown behavior remains visibly unresolved, returning an
explicit unspecified-rule error, until researched or deliberately specified as
fitted/opinionated. Runtime hookup is partial.

## The information an aircraft uses

These quantities describe an assigned target, not a rule that gives the AI
knowledge of every object. Acquisition, tracking persistence and information
sharing are separate, still incomplete contracts.

| ID | Established input meaning | Evidence and remaining limits |
| --- | --- | --- |
| B01 | Target ahead means the larger of absolute heading-to-target error and pitch-to-target error is strictly less than 90 degrees. Off-beam is that larger error, expressed in degrees. | Executable-confirmed. This is not a circular cone measured by one 3D vector angle. |
| B02 | Target facing means its absolute horizontal bearing error toward this aircraft is at most 90 degrees. | Executable-confirmed. The evaluator uses the heading output, not the target's pitch error; equality differs from B01. |
| B03 | Target distance is spatial separation; horizontal distance removes altitude separation. Own `alt` is height above the queried surface. | Executable-confirmed, with the existing fixed8 feet contract. Keep AGL and absolute altitude distinct. Terrain/object query eligibility is not fully recovered. |
| B04 | Climbing is permitted by the script predicate when current speed is at least current minimum speed plus 75 ft/s (about 44 knots). Better-speed means own maximum speed exceeds the target's by at least 75 ft/s. | Executable-confirmed. AI speed is feet per second; the HUD shows the same value in knots (times 3600, divided by 6076). Minimum and maximum are the aircraft's loaded envelope limits at its current altitude, so the thresholds move with altitude and loading. Fitted (agent decision, 2026-09-29): the minimum is never below the speed at which the loaded aircraft can hold 1 G in its flap setting (`flight::State::minimum_level_speed`; the envelope rows alone reach down to the 0 G row's slow edge), and the maximum never above the fastest envelope row that still leaves 1.2 G after the loading (near the top speed only the 1 G row holds, and the legacy flight model divides it by the loading, so a loaded fighter at full power sank into flat ground). |
| B05 | Better thrust-to-weight means the own-minus-target performance evaluator is at least 10. `corner` and `cornerSpeed` return the same aircraft query. | Executable-confirmed. The thrust-to-weight scale is not established here; do not interpret 10 as a physical ratio of ten or an invented ten-percent bonus. |
| B06 | The fighter can distinguish human-controlled targets, fighter versus other aircraft, wing combat/approach state, and its own resolved experience. | Source use and executable accessors inspected. Human-control accessor reads the target's control flag. Network/control ownership and wing-state production remain open. |

Proposed host inputs must distinguish observed target pose from permitted
classification/performance metadata. FA's relative-performance evaluators read
the assigned target's aircraft state directly. The host may eventually expose
equivalent information through an explicit retail-awareness policy; this pass
does not establish how the target first became known. The current sensor service
is a reusable host component with its own documented authored detection rules.

## Fighter and strike behavior

The following rules apply to the shared family used by all twelve current
ports. They describe requested maneuvers. Achieved trajectories depend on the
aircraft's flight limits and on steering rules still being recovered.

### B10: Idle, hit and missile events

The source has separate reasons for nothing, evade, attack, radar launch,
infrared launch and hit. Nothing and hit end this fighter script invocation.
They do not prove that the aircraft stops moving, loses its route or ignores
damage. Default event/route behavior is outside those handlers.

On an infrared-launch reason, request a climb to 90-degree pitch at current
heading and maximum speed, with bank unconstrained. A 50% choice also sends a
wingman break request of +90 heading and +90 pitch. The comment calls this a
break toward the sun, but the active statements do not inspect sun position.
Do not add a sun-tracking dependency from that comment.

On a radar-launch reason, if target off-beam is at most 90 degrees, turn toward
the target bearing at the engagement pitch and corner speed. Otherwise choose
current heading plus or minus 90 degrees with equal probability. Both branches
send a wingman break request. The engagement-pitch evaluator is not simply the
line-of-sight pitch; it depends on relative altitude and aircraft limits.

These are source-confirmed choices after the engine supplies the reason.
Threat sensing, warning eligibility, the identity represented by the event's
target, and interruptions of an existing maneuver remain unknown. Device
release is a separate [experience-dependent response](ai-experience.md#other-experience-effects).

### B11: Evasion and last-ditch defense

For a non-aircraft target, the fighter source requests unchanged heading,
maximum speed and an altitude operand equal to the larger of current AGL
altitude and 20000. Do not describe this as a settled 20000-foot AGL climb:
the source mixes an AGL read with a command whose altitude frame still needs
confirmation.

Against aircraft, coordinated escape requires a target behind and facing,
wing-combat state and wing-approach state. A preliminary 70% skip leaves 30%
for this branch when all its conditions hold. The remaining choice is equally
split among cross-turn, left/right split and high/low split. Exact coordination
geometry and membership must be recovered from the wing services.

A target behind, facing, within 5000 feet and with heading difference at most
35 degrees enters last-ditch selection. Candidate actions include split-S,
horizontal scissors, reverse, overshoot, loop and horizontal/vertical jinks.
Some candidates redraw when altitude, speed or geometry makes them unsuitable.
The branch thresholds therefore are not unconditional maneuver frequencies.
At 30000 feet or more, the ordinary fallback requests flight away from the
target at maximum speed rather than jinking. Below that distance it selects a
vertical jink. All of these decisions are source-confirmed; the maneuvers'
complete shapes and interruption rules are still open.

### B12: Approach, tactical choice and pursuit

Air-to-air entry raises requested wing horizontal spacing to 5000 if it is
smaller. Spacing is in feet; B43 records the reviewed wing consumer and remaining geometry.
The source attempts wing splits only against an ahead fighter with nonzero
wing-approach value in the inclusive range 5000 through 30000. That value's
producer is unresolved; do not replace it with ordinary target distance yet.

A special approach to a human-controlled target requires separation from 5000
through 20000 feet, off-beam at most 20 degrees, heading difference at least
155 degrees and pitch difference at most 25 degrees. The `random 100 > 75`
skip means a nominal 76% entry threshold, not 75%. It then chooses offset pass
or overhead pass equally. This is source-confirmed; resulting pass geometry
requires the maneuver consumers.

After earlier approach choices, separation greater than 15000 feet selects
pursuit directly. Otherwise ahead/facing geometry feeds the
[experience-dependent tactical choices](ai-experience.md#fighter-tactical-choices).
The branch called best attack ordinarily selects pursuit. Against a fighter
behind and facing within 2000 feet, heading difference at most 35 degrees and
a 75% gate can instead choose last-ditch defense. The comment promising repeated
fast passes for skilled energy fighters does not establish such a behavior.

Pursuit requests target-relative offsets with the experience-dependent
variation already specified. Negative `homepos` speed operands select the distance-regulating rule in B15.
They encode a desired separation, not negative airspeed or a closure rate.

### B13: Basic maneuvers and command limits

Straight flight requests current heading, engagement pitch and maximum speed.
Straight climb requests +45-degree pitch, falling back to straight flight if
B04 rejects climbing. Straight dive requests -45 degrees and falls back to
straight flight below 3000 feet AGL. Left/right breaks request current heading
minus/plus 170 degrees, engagement pitch and corner speed. A turnaround chooses
one of those heading changes with equal probability. These choices are
source-confirmed, not evidence of instantaneous turns.

Executable-confirmed command input handling wraps heading into 0 through
359 degrees, bounds explicit bank to -180 through +180 degrees and supports an
unconstrained-bank mode. Motion construction bounds requested pitch to
-90 through +90 degrees. Ordinary positive speed requests are constrained by
minimum/maximum speed queries. The duration operand is clamped to 0 through
15; zero takes a geometric-completion path rather than meaning no action.
Nonzero duration is nominal simulation seconds, evaluated on a quarter-second
clock. A duration of 5 adds 20 quarter-second counts to the sampled clock;
expiration is eligible when the clock reaches that deadline. Clock phase can
make eligibility up to just under a quarter-second earlier than five seconds
after submission, and service scheduling can delay observation. This is a
simulation-time rule, not a wall-clock promise under pause or time compression.
Long-session deadline saturation remains an original edge case described in
the source notes, not a requirement to copy the timer representation.

Zero-duration motion chooses one completion axis from heading, pitch and, when
constrained, bank, using angular differences and aircraft rate queries. It does
not wait for all three axes to arrive. The ordinary selected-axis predicate
requires equality; pitch has additional early completion paths. In free
flight (not in an airfield or carrier takeoff/landing sequence), a requested
pitch above 25 degrees finishes when speed has fallen to minimum speed plus
25 ft/s (about 15 knots). This is separate from B04's entry permission for a
climb. While the terrain floor of B44 is active, any lower pitch goal counts
as complete at once. Goal kinds also include speed, altitude, horizontal and
spatial distance; a cancelled command completes on its next check.

Interruption and route resumption are executable-confirmed: a script issues
at most one motion per event, and that motion replaces the current maneuver
immediately. When a maneuver's command list runs out, the aircraft asks its
script for the next action; if the script declines, the next route leg is
taken from the current waypoint (B48). A wing member with a leader takes the
formation move instead. Wing commands cancel the current maneuver outright.
Script reasons are ranked in B47.

### B14: Surface attack and egress

The fighter source uses the ordinary straight attack run after a 25% alternative
selection fails. The ordinary run approaches until horizontal separation is
at most 2000 feet or the target ceases to satisfy the source target predicate.
The alternative branch can choose dive-bombing when altitude and horizontal
distance are each at least `max(10000 feet, twice turn radius)`, behind a 33%
gate. Otherwise it chooses a fast-high pass one-third of the time and pop-up
two-thirds of the time.

Pop-up approaches with a target-offset vertical operand of 500 until within
10000 feet, requests +25-degree angular offset, then turns toward the target
until within 1500 feet or the target is no longer ahead/valid. The fast-high
pass uses a vertical offset operand of 5000 and exits its approach within
2000 feet or when the target is no longer ahead/valid. Its comment assumes
air-to-ground missiles, but its weapon-availability check is commented out.
Do not claim that this script guarantees a compatible weapon before choosing
that maneuver. Actual firing eligibility belongs to weapon services.

Egress uses a jinking phase while within 10000 horizontal feet, then a climbing
departure until at least `max(30000 feet, twice turn radius)` away, before
turning back. This is horizontal separation, not a 30000-foot altitude goal.
B15 establishes the shared target-offset frame. The dive-bomb rollout, the
jink path and the pop-up and orbit maneuvers are recorded in [Strike
maneuvers, steering frame and orbit](../formats/ai.md#strike-maneuvers-steering-frame-and-orbit)
(static read, ST-R); only the interruption rules and the hit test remain open.

**T.O.R.E strike AI (opinionated, John, 2026-10-11).** Retail chooses these
passes by dice and moves each released AI bomb to within 100 ft of its target.
T.O.R.E does neither: the pass is a fixed function of perceived threat and
available speed, limited by the aircraft's bombing class and weapon, and the AI
flies the pass and releases from the player's own solutions with skill-scaled
precision. The B14 pass shapes above still apply to each pass. The full
behaviour, with labels, is in [AI strike](ai-strike.md#pass-choice).

## B15: Pursuit reference frame and speed regulation

Executable-confirmed for `homepos` with a valid target and positive duration.
Its three offset operands are distances in feet. The horizontal pair rotates
with target heading, while the vertical component remains world-vertical.
Target pitch and bank do not rotate that offset into a full aircraft body frame.
The point used for steering can first be adjusted by the selected weapon's
prediction helper for a hostile target. B44 records the recovered lead gates
and the remaining speed-estimator boundary; pure position pursuit is insufficient.

The negative speed operand's magnitude is a desired spatial separation from
the target's unoffset position. For example, `-750` requests regulation around
750 feet, independently of the offset steering point. Let `e` be actual spatial
separation minus that desired separation, and `v` be the target's scalar speed,
with negative target speed treated as zero. When both heading and pitch errors
to that target position are at most 90 degrees, choose:

| Separation error `e`, feet | Requested speed before aircraft limits |
| --- | --- |
| `e >= 5000` | Own maximum speed |
| `1000 < e < 5000` | `v + 100` |
| `250 < e <= 1000` | `v + 50` |
| `50 <= e <= 250` | `v + 25` |
| `-50 <= e < 50` | `v` |
| `-100 <= e < -50` | `v - 25` |
| `e < -100` | `v - 50` |

The speed increments are feet per second: +59, +30 and +15 knots above the
target's speed and 15 and 30 knots below it. Own maximum, minimum and corner
speed are the loaded envelope values at current altitude, corner capped at
the maximum. If either angular error is greater than 90 degrees, the request
instead uses corner speed. The ordinary regulating branch applies own maximum
and minimum speed queries, with a separate aircraft-state exemption from the
minimum. Achieved acceleration and turning remain properties of the motion
consumer, not instantaneous changes implied by this table.

The offset representation loses some precision at larger magnitudes. That
encoding is recorded in source notes; it is not a requirement to implement
packed offsets in Rust. Lateral/longitudinal sign conventions, weapon lead,
invalid-target continuation and the minimum-speed exemption need closure before
full trajectory acceptance.

## B20: Other aircraft families

These are active source behaviors, not acceptance of newly flyable aircraft.
Family binding and catalog counts remain in the experience spec.

| Family | Behavior present | Inputs and missing contract |
| --- | --- | --- |
| F-117 | Shares fighter air combat; ground pass requests a 5000 vertical offset and speed operand 300 while target remains ahead/valid | Target pose, maximum/minimum speed and attack path; offset axes are the [steering frame](../formats/ai.md#strike-maneuvers-steering-frame-and-orbit); release timing is the weapon service's gates ([weapons](../formats/weapons.md#ai-release-against-ground-targets)) |
| Helicopter | Shares IR/radar response entry, distinct pursuit and pop-up attack; missile response turns toward a target with off-beam below 90 degrees, otherwise chooses a +/-90-degree break | Helicopter motion limits and target geometry; do not reuse fighter aerobatic trajectories. T.O.R.E builds the break (beamed to the threat, at speed and height, with terrain masking) as [helicopter defence](ai-strike.md#helicopter-defence) |
| Bomber | Aircraft attack handler exits; ground attack uses a high pass; IR/radar reactions choose a +/-90-degree break | Target type, altitude, available stores, wing state and evasion limits; exit does not prove that defensive weapons cannot fire |
| AC-130 | Ground attack repeatedly requests a circle with speed halfway between corner and maximum | Target pose and orbit parameters; the six circle arguments (left, 4,000 ft radius, 2,000 ft up, 35 degrees of bank capped by `maxBank`, speed, 5 s) and the gun arcs are in [Strike maneuvers, steering frame and orbit](../formats/ai.md#strike-maneuvers-steering-frame-and-orbit); the hit test is open |
| Large/airliner | Handles hit and evade; hit requests a downward pitch from 0 through -24 degrees and speed operand 2000 | Damage event and aircraft limits; positive speed clamp prevents assuming literal 2000-unit achieved flight |
| MOTH | Fighter-derived air behavior with distinct ground attack and shorter egress operands | Exact special-aircraft configuration and maneuver consumers; no ordinary aircraft alias |

A helicopter does not fly the fighter reactions of B47 below: its break, its
speed and height and its use of terrain are in
[AI strike](ai-strike.md#helicopter-defence), a helicopter has a fuel schedule of
its own ([rotorcraft fuel](#rotorcraft-fuel-slices-h3-and-hf2)) and since slice H2 takes off
and lands vertically through the airfield sequences
([vertical takeoff and landing](ai-airfield.md#vertical-takeoff-and-landing)).

For the T.O.R.E strike behaviour of these families (heavies level-bomb from a
minimum of 5,000 ft AGL raised by threat, helicopters pop up, the AC-130
orbits with its radius and height taken from its guns' reach, and the bombing
class taken from the imported record rather than a list), see
[AI strike](ai-strike.md#bombing-class), [heavy bombers](ai-strike.md#heavy-bombers)
and [AC-130](ai-strike.md#ac-130).

## B30: Surface behavior

SARAN's hydrofoil program attacks ships only and has the approach/depart/turn
sequence in the [experience spec](ai-experience.md#wider-retail-behaviour-coverage).
Other surface behavior is primarily engine-side. `_GVProc` dispatches to a
surface event service and the shared non-player weapon service. Static objects,
carriers, deck personnel and ejectees also have distinct bindings. This proves
that copying the hydrofoil script cannot supply all surface AI.

Proposed surface inputs are mission role/order, route, speed/turn limits,
terrain or water constraints, damage, side, resolved ground experience,
observations, mount arcs, weapon inventory and supported target classes.
SAM, AAA, moving vehicles, ships and carriers need separate contracts for
target eligibility, tracking, firing and movement. Exact engagement ranges,
salvos, crew reaction delays, terrain avoidance and experience effects are
unknown in this pass. Do not apply aircraft experience tables to these objects
without a demonstrated consumer.

## B40: Weapons, orders and lifecycle boundaries

The aircraft scripts request motion and wing actions; the non-player weapon
service separately evaluates and services weapons. Target selection, emission
control, gun lead, missile release, support, countermeasure consumption and
rearming are not established by a maneuver name. An attack request may result
in movement without a legal shot.

A controller needs feedback for invalid/lost targets, unavailable weapons,
failed or completed movement, damage, fuel state and superseded orders. The
host already models several weapon inhibition reasons. Route following,
formation rejoin and damage disengagement have identified source entry points
but do not yet have complete AI contracts; takeoff, landing and low-fuel
recovery are specified in [AI airfield sequences](ai-airfield.md). No distance,
fuel percentage or priority is invented here.

## B41: Target retention, eligibility and ranking

An aircraft's weapon target selector can retain an existing valid target at
strictly less than 20000 feet spatial separation when its special retarget-policy
flag is clear. At exactly 20000 feet it proceeds to selection. This is a
retention shortcut, not a maximum sensor range or maximum missile range.
The corresponding non-aircraft actor shortcut also checks terrain blocking.
Aircraft still have a blocking check in the later firing service, so retained
target identity must not be treated as permission to shoot through terrain.
The meaning and producers of the special policy flag remain unresolved.

Candidate selection excludes self, invalid objects and disallowed object types,
then evaluates reaction policy and available seeker eligibility. Reaction
rejects same-side candidates and depends on target class, mission masks and
usable weapon inventory. This establishes equipment-dependent eligibility;
it does not establish that every candidate is a currently visible sensor contact.
The exact seeker detection, information-sharing and reacquisition rules remain
open. A separate policy route can select a mission/defense-priority candidate
without the ordinary distance ranking below.

For an ordinary eligible candidate, the aircraft selector minimizes a score
starting at spatial distance. It adds 10000 feet for each applicable condition:

- The candidate is not an aircraft.
- At least one member of the same wing is already attacking it.
- The existing wing attacker count meets or exceeds the assignment allowance.

The last two penalties can both apply. They discourage concentration of attacks;
they do not make an occupied target ineligible. The allowance helper can return
1, 2 or 100 according to policy flags, but the mission-facing meanings of those
flags still need tracing. Equal scores do not displace the current best candidate
in this pass. Do not expose original enumeration order as a gameplay requirement.
Surface actors have additional ranking terms and special zone behavior. The
three-penalty aircraft rule must not silently become their full selector.

## B42: Weapon preparation, search cadence and firing

Executable-confirmed service boundaries distinguish target search, preparation,
lock checking, firing and reload. A valid hostile target, an available compatible
station, weapon-specific lock checks and an unblocked firing path precede the
reviewed firing branch. Losing a target sends the service back toward search.
A failed lock causes a nominal one-second retry; no suitable station causes a
nominal two-second retry. The motion honours that retry as well: once no store
resolves, gun lead tracking keeps the aircraft until the retry ends instead of
handing it back to a missile tactic on the next tick the missile's zone test
passes (agent decision 2026-09-29; without it a fighter at a zone edge swapped
maneuvers every few ticks, its own stick input moving the zone test). Gun lead
tracking is a 1-second timed motion, and by B13 a timed motion runs to its
deadline unless an event replaces it, so a change of preferred store (gun to
missile) does not end it before its second is up (agent decision 2026-09-29;
a fighter whose best store alternated otherwise swapped maneuvers 30 times in
3 seconds). Pre-firing service states can add half a second to a
chosen delay with a 10% gate. These are eligible service times, subject to the
clock and scheduling limits in B13, not a guaranteed time to the first shot.

The imported NPC fields supply search and initial preparation delays. Among the
current twelve PT records, the no-target search delay is:

| Aircraft records | Nominal no-target retry |
| --- | --- |
| F18.PT, RAFALE.PT, F14.PT, F31.PT, SU25.PT | 3 seconds |
| A4E.PT, MIG29.PT, SU27.PT, MIG21.PT, MIG23.PT, SU35.PT, F22.PT | 5 seconds |

All twelve provide nominal preparation delays of 5 seconds for the ordinary
branch and 8 seconds for the branch selected by the unready flag. The flag's
producer and interruptions remain open; these are not experience levels.
A successful lock uses the weapon's own tracking delay before the firing state.
The service also maintains a 15-second preparation/lock window in the reviewed
transitions. This is not a universal missile cooldown or target-loss timeout.

Burst size, within-burst spacing, reload and startup pacing come from the
selected projectile and NPC policy, rather than one shared fighter firing rate.
The reviewed firing service invokes the shared launch service and subsequently
checks whether the station still resolves, so depleted stores can change its
state. B45 specifies ordinary ammunition accounting and the reviewed envelope
and support gates. Per-store profiles, full gun lead, burst randomization,
in-flight support transitions and missile limits remain in research.
No single fixed gun range or unlimited-ammunition behavior is implied.

The [gun-employment correction](ai-gun-employment.md) adds an authored bounded
retry after expiry, plus a separate live-gun ready/burst/recovery cycle. It
replaces sparse representative-group gun releases with individually accounted
rounds and physical barrel lead. These are authored host corrections, not claims
that the original used the same recovery or aiming policy.

API consequence: supply a typed weapon profile, compatible-station availability,
lock/blocking results, preparation status and per-station feedback. Search and
preparation clocks belong to persistent actor/service state. Do not redraw or
restart them on every 120 Hz call. The imported timing profile remains distinct
from aircraft experience, flight capability and mission role.

## B43: Wing commands and formation variation

The script's approach command requires the sender to be the wing leader and
at least one wingman to exist. It compares the first wingman's target with the
sender's current target, requests target assignment when they differ, then sends
the approach parameters. Break and approach parameters are bounded heading-offset
and pitch requests, delivered through wing events. This establishes a command
and recipient-state dependency, not instantaneous movement or guaranteed obedience.
The receiver behavior established in this pass is specified in B46; radio
acknowledgment and broader order handling remain separate questions.

Horizontal and vertical spacing requests are clamped to 512 through 20000 feet
and the reviewed script setters act only for a wing leader. Formation motion
combines a formation-table position scaled by those spacings with small changing
offsets. Its offset draws are -15 through +14 feet in the first component and
-50 through +49 feet in the other two components before position packing. The
next variation deadline advances by a randomly chosen 1 through 10 simulation
seconds from its previous deadline. It is not a new random offset every frame,
and delayed service can leave it catching up with an old deadline.

The formation point rotates horizontally with the leader's heading. The
formation request lasts a nominal 3 seconds and is re-issued whenever the
wingman has nothing else to do.

Formation geometry is executable-confirmed. Three formations exist: echelon,
line abreast and line astern. Slot positions are multiples of the wing's
horizontal spacing H and vertical stacking V, with lateral positive to the
leader's right, vertical positive above, and longitudinal negative behind:

| Formation | Wingman n position before variation |
| --- | --- |
| Echelon | Lateral: right for odd n, left for even n, by ceil(n/2) H; longitudinal ceil(n/2) H behind; vertical by a fixed per-slot pattern (1, -1, 2, -2, 3, 3, 4, 4, 5 times V for slots 1 through 9) |
| Line abreast | Lateral n H to the right; no longitudinal offset; vertical n V |
| Line astern | Lateral 0; longitudinal 2n H behind; vertical n V |

The table above records original geometry. **Opinionated, requested by John on
2026-09-18:** the host uses balanced line abreast instead: odd-numbered wingmen
stay right and even-numbered wingmen stay left, at ceil(n/2) H. There is no
longitudinal offset. Vertical stacking remains n V. At 512 ft spacing, slots
1 through 4 are respectively 512 ft right, 512 ft left, 1024 ft right and
1024 ft left. This preserves their echelon lateral positions during a formation
change. Routine changes use the transition procedure below.

The first wingman therefore flies one spacing right and one back in echelon,
one spacing right in line abreast, and two spacings back in line astern. The
player's horizontal spacing order toggles between 512 and 2048 ft (the manual's
500 and 2000); the stacking order cycles level, 512 ft high, 512 ft low.
Mission waypoints set the initial control, formation, spacing and stacking;
scripts may set spacing from 512 to 20000 ft. An idle AI aircraft that is not
a wingman and has no command forces loose control, line astern and at least
1024 ft spacing before holding heading.

Formation speed (mode 9) uses the B15 bands on the spatial distance to the
slot point: beyond 5000 ft own maximum; 1000 to 5000 ft leader speed plus
100 ft/s; 250 to 1000 plus 50; 50 to 250 plus 25; within 50 ft leader speed;
clamped to own minimum and maximum. The negative bands are reachable only
through a lead-projection branch whose entry condition is open.

### Leader succession

**Opinionated: the rule is John's of 2026-09-28, and at his decision of
2026-09-29 it replaces the bug bash's renumbering.** Each wing has a current
leader, its member 0 at the start of the mission. When the leader's aircraft is
destroyed or its pilot ejects, the lead passes on that tick to the
lowest-numbered living human in the wing, or, if there is none, to the
lowest-numbered living AI member. Every aircraft keeps its member number and
callsign; the followers re-form behind the new leader in member order, taking
formation slots 1, 2 and so on. A member that was following the lost leader in
to land stops, as the wing abort does. When the AI takes the lead nothing is
said; the radio call a human who takes the lead hears is in the
[architecture guide](../ARCHITECTURE.md#lead-succession). In a networked game
with revival the [lead hold](#lead-hold-in-games-with-revival) changes this
for a human lead.

### Lead hold in games with revival

**Opinionated: the rules are John's of 2026-10-09 (the lobby pass); the
ordering, the owner of a lead an AI lost while its humans wait, and the HUD
lines are agent decisions.** In a networked game, co-op or PvP, whose respawn
rule is not `none`, a human who leads a flight keeps its lead while dead,
waiting to revive, in the lobby with its seat held, away (the AI flying its
plane for it), or flying back after a revival. Single player, and a game with
`respawn none`, keep the rule above unchanged.

- **The owner.** A flight's lead belongs to the human who flies its leading
  plane: from the start, by taking it, or by succeeding a lost AI lead. When
  an AI lead is lost while every human in the flight waits to revive, the
  lowest of them owns the lead. "Lowest" is the member number of the human's
  current plane, then the seat, the order the rule above uses. Revival now
  retains that original number (John, 2026-10-23).
- **The stand-in.** While the owner does not fly, the flight is led by a
  stand-in chosen by the rule above (the lowest flying human, else the lowest
  flying AI member). Everything that follows a leader follows the stand-in.
  Under an AI stand-in the flight flies the
  [mission of opportunity](#mission-of-opportunity-after-a-lost-human-leader);
  a human stand-in may give wing orders. A stand-in that is lost passes the
  stand-in's place on the same way. A human stand-in reads "You lead the
  flight until Viper flies again." (the owner's player by callsign, the
  lobby pass's follow-up F1; the owner's plane by its radio name, "Red one",
  when the mission has no callsign for it), with no voice, instead of
  "You're the Wingleader now". A stand-in never
  becomes the owner while the owner is in the game.
- **The lead given back.** As soon as the owner's new plane flies in the
  flight, it leads: the mission of opportunity ends, the flight re-forms on
  it in member order, and each AI wingman flies back into formation, except
  one engaged in combat (holding a target), which finishes its fight first
  and then re-forms (John, 2026-10-09). The owner reads "You lead your flight
  again." with no voice. John accepts that the owner may be out of the fight
  while it revives or flies back.
- **The owner leaves the game** (it leaves, is kicked, or drops and is no
  longer kept by the host): the lead goes to the next human in the flight,
  who leads at once if flying (and hears "You're the Wingleader now"), or
  owns the held lead if waiting to revive. With no human left the current
  leader leads as an ordinary lead, and the AI's own succession goes on. A
  human who leaves the flight for the lobby with a living plane, or whose
  revival puts it in another flight, has left the flight the same way; one
  who leaves with a lost plane keeps its held seat and the lead.
- **AI-only flights** keep the rule above: the lead passes down the flight
  as aircraft are lost. A respawn joins behind the current lead in the lowest
  free formation slot and does not displace it. Agent decision applying John's
  numbering request of 2026-10-23: later succession uses the retained original
  member number, under the existing lowest-numbered-member rule.

### AI respawn in networked games

**Opinionated: the rules are John's of 2026-10-09 (the lobby pass); the
2,000 ft step back, the room rule and the retiring of AI wrecks are fitted
or agent decisions, recorded in the
[architecture guide](../ARCHITECTURE.md#death-revival-and-lives).** In a
networked game, co-op or PvP, whose respawn rule is not `none` and whose King
has AI respawn on (the default), a lost AI aircraft that no player holds comes
back. Single player never respawns.

- **When.** After the same delay as a human's revival, and while the original
  aircraft's lineage has lives left (lives count per original aircraft, since
  an AI aircraft has no player; unlimited by default). A delay of 0 respawns on
  the next tick. An aircraft the AI flies for an away or dropped player is that
  player's own revival, not an AI respawn.
- **Where.** At the flight's original spawn point (the aircraft's own start
  position and heading), not near the lead and not at the revival distance. If
  a living aircraft is within 2,000 ft of the point, the point steps back 1 nm
  along the reverse of the heading, up to five times, and never off the map.
- **What.** The same aircraft with full fuel, the loadout the lobby gave that
  slot (else the standard load) cut by the revival weapons setting, the
  flight's own skill and the flight's orders. It gets a fresh decision stream
  of its own.
- **Joining the flight.** It is the flight's last wingman: it takes the lowest
  formation slot no living wingman holds (never above slot 9) and flies to its
  lead; a flight in a mission of opportunity gives it free engagement like the
  rest. Its original member number and callsign are retained (John,
  2026-10-23); formation slots remain separate from those labels.
- **Silent.** No radio call and no HUD line for an AI respawn.

### Mission of opportunity after a lost human leader

**Opinionated: the rule is John's of 2026-09-30; every number and edge below
is an agent decision (fitted).** When a wing's human leader is lost and the
lead passes to an AI aircraft (no living human is left in the wing), the wing
"continues on with the mission of opportunity and visual based on last known
enemy locations, if they can't find anything then RTB's", and, asked what a
wing that knows no enemy does: "Ideally it's go after enemy first, if no enemy
or threat found, then waypoints, if no waypoints, then direct RTB." Before, the
new lead had no order or route and flew straight on until the world edge
removed it.

- **Taking it up.** On the tick the AI takes the lead, every AI member of the
  wing is given free engagement, and any recall the lost leader gave ends with
  it. The lost leader's task (a patrol around its start point, an escort of the
  human, an intercept of one aircraft) ends too. A wing under weapons hold or
  self-defense keeps those rules: it is not cleared to hunt, so it returns to
  base at once.
- **Engaging.** A hostile aircraft the new lead detects releases the wing to
  engage, as for any AI leader.
- **Searching.** With no hostile in contact, the lead searches by eye where
  the enemy was last seen. The wing pools what its members know: every hostile
  aircraft each member sees now or remembers in its awareness memory (its own
  sightings and sensor tracks), one point per enemy aircraft at its latest
  known position. The pooled points outlast each pilot's own memory. A
  destroyed aircraft is dropped, as from each pilot's memory. The lead flies to
  the nearest point, circles over it (the lost-contact search: straight to the
  point, then a level clockwise circle of 0.75 nm radius once within 1 nm) and
  counts it searched after **60 s** within **2 nm** of it; then the next
  nearest. A hostile a member sees now is searched first, and a newer sighting
  of an aircraft makes its point worth searching again. The wingmen stay in
  formation on the lead. The lead's own lost contact, if it has one, comes
  first, as before.
- **The search ends** when no point is left to search (or the wing knew of
  none), or after **10 minutes** without a hostile in contact (counted from
  the lead passing, or from the wing's last contact: any member seeing a
  hostile aircraft, or the lead having a target).
- **Waypoints.** The lead then flies the wing's remaining waypoints in order:
  the lost human's route from the waypoint it was flying to, or the wing's own
  route. It heads for each at the waypoint's altitude and cruise speed (the
  B48 route command, climbing or diving at most 20 degrees), and a waypoint is
  done once it is behind the aircraft (B48: the direction to it has turned to
  within 45 degrees of the opposite of where it lay when it became current).
  The wing still engages anything it detects, and a new contact reopens the
  search; the route resumes when that search is over. *Today no wing has
  waypoints:* Quick Mission has no mission route (the NAV page shows
  `NO WAYPOINTS`, [navigation selection](weapon-navigation-selection.md)) and
  AI wings fly none, so the wing goes straight home. The rule takes effect
  once mission routes are imported; the AI probe can hand the player's wing a
  route to test it.
- **Returning to base.** With no waypoint left, or none at all, the wing
  returns to base through the [ordinary return to base](#return-to-base-when-the-mission-is-over):
  each airborne AI member with a home runway lands there, and a lead with none
  flies home and holds. A member still waiting to take off stays parked, since
  it would only take off to land again (it does not hold up its side's own
  return to base either); one already taxiing goes on, takes off and lands.
  The wing does not take up the search again. Bingo fuel and damage send an
  aircraft home earlier, as always.
- **Unchanged.** A wing led by AI from the start keeps its own succession: when
  an AI leader is lost, the next member leads and nothing else changes. A wing
  that took up a mission of opportunity keeps it when its AI lead is lost in
  turn. Nothing is said on the radio.

### Traffic avoidance

**Opinionated, requested by John on 2026-09-29; the numbers are agent
decisions:** an AI aircraft flying on its own predicts its closest approach to
every other airborne aircraft, friend or foe. A conflict is a closest approach
within the next 6 s that misses by less than 300 ft plus half a second of the
closing speed (so a fast head-on pass is seen earlier and given more room).
Against the aircraft it is attacking only a miss under 150 ft counts, since it
closes on that aircraft on purpose. On the first conflict it takes up a heading
30 degrees off its current one, away from the other aircraft, or to the right
when the other is within 20 degrees of its nose (head-on, as the rules of the
air have both aircraft turn right). It keeps its pitch and speed, holds that
heading until 3 s after the last predicted conflict, and never changes side
while it holds, so it does not flap. The held heading belongs to the aircraft
whose conflict set it: once that aircraft is no longer a conflict, a conflict
with another aircraft sets a new heading, 30 degrees off the current one and
away from the new aircraft (B4, 2026-10-06: two flightmates that turned right
together for one head-on aircraft otherwise held that heading against each
other for half a minute and collided). Avoidance never overrides missile or gun
defense, an airfield sequence, or a formation procedure (station keeping, a
join or a rejoin): wingmen in formation are kept apart by their slots.

### Return to base when the mission is over

**Opinionated, requested by John on 2026-09-29:** once a side has seen hostile
aircraft and none is left alive, its AI aircraft return to base. Each airborne
AI aircraft on that side that is not already taking off, landing, recovering
from damage or bugged out, and has a home runway, is ordered to land there
through the ordinary landing sequence ([AI airfield sequences](ai-airfield.md));
its wingmen join the leader's landing as for any ordered landing. A wing with
no home runway has its leader fly the B48 return-to-base path to its home
position (the launch point) and hold there by flying back over it, the
wingmen staying in formation. Helicopters, the V-22 and the hovering jets land
too since slice H2, by the [vertical landing](ai-airfield.md#vertical-takeoff-and-landing). Nothing changes while any hostile aircraft is
alive or any aircraft on the side is still waiting or taking off (its landing
traffic would hold the runway), and a mission that never had a hostile
aircraft does not end this way.
A wing led by the player stays with the player, whose own mission may still be
open. A wing with unfinished ground work stays on its strike assignment until
its strike controller finishes; see [AI strike](ai-strike.md). A CAP assignment
continues patrolling after the last bandit dies; see
[CAP navigation](ai-awareness.md#assignment-delivery-and-observed-attack-reports).
These exceptions implement John's correction of 2026-10-15. Intercept and escort
wings without remaining ground work keep the return rule.

### Normal formation variation and transitions

**Opinionated, requested by John on 2026-09-18:** tighten normal vertical
wandering and use coordinated local movement for routine formation changes.
The following implementation values are **fitted, agent-authored**. The recovered
variation draws remain documented above; live flight scales the vertical draw
by 0.1, giving -5 through +4.9 ft. Lateral and longitudinal bounds remain -15
through +14 and -50 through +49 ft. All three requested offsets pass through a
3-second exponential smoothing filter. These bound requested wandering, not
achieved aircraft error. No position, attitude or velocity is directly changed.

A change of formation, spacing or stacking while close starts a Reposition
phase from the aircraft's actual leader-relative position. A replacement order
replans from that current position. Aircraft already separated continue their
safe rejoin toward the new destination instead. A routine new slot more than
1800 ft away does not itself trigger departure during Reposition.

If lateral and vertical differences are both under 75 ft, move directly toward
the new slot. Otherwise first establish aft clearance at the farther-aft of
current and destination positions; if the destination is ahead, add 512 ft of
aft clearance. Then move laterally/vertically, then forward to the slot. Stage
completion needs position error under 60 ft and speed relative to the moving
formation frame under 12 ft/s.
Desired repositioning velocity is error / 8 seconds, capped at 40 ft/s, added
to leader velocity and the velocity needed to follow its measured turn at the
aircraft's relative position. Bank requests are capped at 20 degrees in straight
flight. In a turn the cap allows the bank needed for the measured turn plus
10 degrees, bounded to 20 through 60 degrees. Afterburner stays off. Loaded
flight-model limits and terrain protection remain authoritative.

Screen requested paths against current traffic over 12 seconds with 350 ft
clearance. Previous-tick Reposition velocity requests provide shared intentions;
when those requests conflict, the higher actor ID yields. Physical traffic
clearance overrides that priority. A yielding aircraft holds its current relative
position. A lateral route blocked by physical traffic can retreat behind that
traffic before retrying. All decisions use one immutable traffic snapshot.
Leader hard maneuvering still permits trailing/separation, and actual predicted
clearance below 350 ft during Reposition triggers breakout early enough to
allow physical response lag. Other phases retain the 220 ft emergency threshold.
This is bounded local planning, not a guarantee that every formation is feasible.

### Physical departure and rejoin

**Opinionated, requested by John on 2026-09-18:** formation must give way to
safe departure and rejoin paths. A rejoin is a separate maneuver. Aircraft
account for neighbors, stabilize outside the formation, and abandon unsafe
approaches. No target rejoin time or random delay controls arrivals. All
movement obeys the [input-only contract](#input-only-aircraft-control).
This deliberately changes host behavior and is not a claim of retail parity.
The public procedures in AETCMAN 11-248 sections 9.15 and 9.26-9.27,
AETCMAN 11-251 section 6.38, and AFMAN 11-2F-16V3 section 3.7.2 informed
this design. Their aircraft-specific speeds are not universal game limits.

The following implementation rules and thresholds are **fitted, agent-authored**:

- Close formation retains signed longitudinal speed correction, leader speed
  plus slot error / 6 seconds, bounded to +/-100 ft/s and loaded speed limits.
  The steering point projects three seconds along the leader's full velocity,
  including climb and descent. This replaces horizontal-only prediction.
- Leader turn rate above 4 degrees/second, pitch beyond 20 degrees, follower
  heading mismatch above 40 degrees, or slot error above 1800 ft releases the
  rigid slot into a wider trailing approach. A hard maneuver alone is not an
  emergency breakout.
- All aircraft positions and velocities come from one immutable mission
  snapshot. Predict closest approach over 8 seconds. Below 220 ft predicted
  clearance, break out. Resume interception only above 500 ft predicted
  clearance and after at least 2 seconds in breakout. These are reaction and
  hysteresis margins, not guaranteed achieved separations.
- Compare escape heading offsets 0, +/-30, +/-60 and +/-90 degrees with
  non-descending current pitch and 15 degrees climb. Score the worst clearance
  across traffic using equal current/candidate velocity weighting to approximate
  response lag. Penalize absolute offset by 0.2 ft/degree; formation side adds
  only a 0.05 ft/degree preference. Terrain pitch protection still applies.
  Two additions (fitted, agent decision 2026-09-28, after the bug battery saw
  two breaking-out wingmen collide; corrected 2026-09-29 after review): a
  heading that still closes on an aircraft is scored on its true closest
  approach over the 8 seconds; only a heading that already opens from an
  aircraft within 220 ft is scored on its distance 2 seconds ahead, because
  from now every opening heading ties at the present distance; and a lower-ID
  aircraft that is itself breaking out is predicted along its chosen escape
  (the same half-and-half blend), as repositioning aircraft already yield to
  lower IDs' plans.
- Before an ordinary approach becomes an emergency, screen requested headings
  at 0, +/-15, +/-30 and +/-45 degrees against traffic over 10 seconds. Use the
  same equal current/candidate velocity weighting, cap clearance credit at
  500 ft, and penalize offset by 2 ft/degree. The chosen detour remains a
  physical steering request. This also avoids members already in formation.
- Approach gates sit 256 ft outward of the assigned lateral offset (floored
  at 256 ft), and 1200 ft behind the assigned aft offset, at least 1800 ft aft
  of the leader. This keeps inner capture paths inside the outer slots. Actual occupied side influences departure. During intercept, a lateral slot
  selects its assigned-side gate before inward capture. Aircraft
  within 450 ft laterally of a corridor yield to a nearer arrival within
  1800 ft of its gate, with stable actor IDs breaking distances within 100 ft.
  Capturing aircraft retain their reservation until close tracking or a safety
  abort. Stabilizing aircraft have priority over intercepting aircraft. A yielding aircraft aims 1800 ft farther aft.
  Aircraft already in close formation do not reserve an arrival gate.
- Intercept requests leader velocity plus a closing vector toward the gate.
  Both heading/pitch and speed follow this velocity vector, avoiding turns back
  toward an overshot waiting point. Closing magnitude is the minimum of
  distance / 6 seconds, sqrt(2 times
  12 ft/s² times distance), and 300 ft/s. The 12 ft/s² braking estimate is fitted,
  not extra braking force. Heading mismatch above 60 degrees instead requests
  the lower of corner and leader speed, within the loaded speed envelope.
- Within 450 ft of the gate, heading error below 15 degrees and relative speed
  below 70 ft/s allow stabilization. Heading error below 10 degrees and relative
  speed below 40 ft/s permit inward capture. The target moves from gate to slot
  over at least 12 seconds, advancing only below 15 degrees heading error and
  70 ft/s relative speed. Close tracking resumes within 200 ft of the slot and
  absolute closure below 40 ft/s.
- Maneuvering, a conflicting arrival, or closure above 120 ft/s within 1200 ft
  of the slot abandons stabilization/capture. Once the gate-to-slot target has
  fully advanced, drifting farther than 1800 ft from the slot also abandons
  capture and establishes another approach. Predicted collision clearance
  overrides every phase. There is no teleport, attitude correction or velocity
  replacement in any transition.
- Close tracking retains the 60-degree bank request limit. Other formation
  phases use the ordinary 75-degree request bound, still limited by loaded
  aircraft bank and G authority. Interception may request afterburner only
  beyond 6000 ft from its approach target, within 20 degrees heading alignment,
  with requested acceleration above 100 ft/s and fuel endurance above 180 seconds.
  Every other phase requests burner off; aircraft capability and fuel remain
  authoritative.

`Controller::formation_trace` exposes phase, phase duration, slot distance,
closure, altitude error, predicted clearance, yielding actor and steering point. It is a hidden
inspection/extension hook, not a normal-flight display. The host can optionally
record it alongside actual controls and achieved flight state; see
[development diagnostics](../DEVELOPMENT.md#formation-flight-traces).

This pass retains the host's direct leader/traffic awareness. Sensor-limited
visual reacquisition, radio permission and AI-leader cooperation remain future
work. Human-leader advisory requests are described in the [radio integration](#live-wing-command-and-radio-integration). A standing formation order currently permits
safe automatic rejoin. Prediction assumes locally constant traffic velocities;
escape scoring approximates response lag rather than simulating each candidate.
Terrain clearance uses existing steering protection, not terrain-aware route
planning. These limitations require live evaluation, especially steep/inverted
maneuvers and dissimilar aircraft. Original negative-band speed selection
remains unknown; the isolated recovered mode-9 evaluator is unchanged.

Presentation uses the same previous/current simulation interval and blend
fraction for the player's camera and nearby aircraft positions and attitudes.
This is a fitted host rendering rule, with no change to 120 Hz simulation.
Pause displays current poses; restart discards old presentation history.

Wing control has three levels: loose, medium and tight. The player's control
key toggles loose and medium. Engage my target, protect me, attack on contact
and the approach orders silently drop control to loose; engage from formation,
disengage, formation, spacing and stacking orders raise it to medium. With
loose control the leader automatically shares its target with wingmen in
formation, up to the waypoint's attacker cap of 1, 2 or unlimited (this is the
B41 allowance); with medium or tight control it does not. Other control
effects are open.

## B44: Steering execution and pursuit lead

Executable-confirmed: heading, flight-path pitch and body bank approach their
requested values over simulation time. They are not instantaneous attitude
assignments. The ordinary angular approach stops at the requested value instead
of overshooting it. Aircraft turn and roll capability come from the current
flight-performance lookup, with turn capability also depending on current speed.
Consequently, sharing F.BI does not give all twelve aircraft identical turns.

For the bank-dependent movement branch, heading authority decreases while the
aircraft rolls into a turn. Below seven eighths of the reference bank magnitude,
it scales with bank magnitude, with a floor of one quarter of base turn authority.
Bank in the opposing direction can suppress heading progress. Pitch authority
also depends on bank, with a quarter-rate floor. A reduced-rate command divides
heading and pitch authority by three; its roll rate is divided by three and
bounded to nominally 10 through 30 degrees per second. Other state branches
halve roll authority and cap it at 45 degrees per second. These are conditional
rules, not an experience multiplier or universal aircraft limits.

Requested flight-path pitch is bounded to -90 through +90 degrees. Body pitch
includes a separate offset from flight-path pitch, so nose direction and
velocity direction must remain distinct inputs.

Performance selection is executable-confirmed. There is no separate AI
performance table: the AI reads the same loaded control-limit block and G
limit the flight model uses, after damage, hit-point and load reductions.
Turn rate in degrees per second is 2500 times the current G limit (in G)
divided by speed in feet per second, with speed treated as at least 125 ft/s
and the result capped at 40 degrees per second. The constant is the original's
own; it is about 78 times the physical value, so AI aircraft turn far faster
than physics would allow at the same G. Turn radius is speed divided by that
rate in radians, capped at 32767 ft. For example a 7 G limit at 500 ft/s
gives 35 degrees per second and an 819 ft radius. Because the G limit is the
loaded one, damage, loading and the experience G adjustment change AI turning.
Roll rate is the aircraft's roll limit after those reductions, halved and
capped at 45 degrees per second in ordinary states; a reduced-rate command
divides it by three and bounds it to 10 through 30. Every ported fighter's
roll limit is 180 degrees per second or more, so they all roll at the 45
degree cap; a B-52 rolls at 15. The reference bank magnitude in the bank-dependent heading authority above
is the aircraft's own maximum bank, so authority scales below seven eighths of
maximum bank.

Terrain avoidance keeps an AI aircraft at least 300 ft (the aircraft's
minimum-altitude value; 300 in every inspected record) above the terrain
1000 ft ahead of it. Level flight is permitted at that clearance; a dive is
permitted only when 1.375 turn radii times the sine of the dive angle fits
inside the surplus above the clearance, tested in 5 degree steps; below the
clearance the floor becomes a climb of at least 5 degrees, steeper as the
deficit grows. The floor is re-evaluated once a second in ordinary flight and
four times a second when pitched below -10 degrees or within 3000 ft of the
ground. While it is active, pitch authority gains 20 degrees per second and
any lower pitch goal completes at once. Pursuit and route commands enable the
floor. A separate terrain event can replace the current command with a
3 second climb at the aircraft's maximum climb angle (80 degrees in inspected
records), but it is masked while motion commands run, so its delivery
frequency is open.

**Opinionated, requested by John on 2026-09-29 (numbers are agent
decisions):** the look-ahead scales with speed. The floor looks six seconds
of travel ahead, never less than the retail 1,000 ft, and uses the highest
ground at up to twelve evenly spaced points along that track, so a ridge
inside it is not skipped. It also asks at least for the flight-path climb that
clears that ground by the minimum altitude, because B44's arc test alone
reduces to a 5 degree climb at supersonic turn radii. Six seconds comes from
the worst case the bug battery met: a fighter banked 90 degrees rolls level in
2 s at the 45 degree per second cap, the pitch loop below the floor closes
over 1 s, and at 1,800 ft/s with the 2.2 G available near the top of its
envelope, raising the flight path 14 degrees for the steepest hillsides takes
about 3 s more. At 440 kt the look-ahead is 4,500 ft; below 100 kt the retail
1,000 ft applies. Before this, supersonic AI aircraft saw rising ground 0.6 s
before hitting it.

**Fitted (agent decision, 2026-10-20, fuzz seed 43):** above corner speed the
envelope, not the pilot, limits the pull, so the look-ahead also covers the
time the G left there takes to raise the flight path those 14 degrees: 3 s to
roll level and settle, plus the angle times the speed over 32.174 ft/s² times
the excess G above 1 (taken as at least 0.25), never less than six seconds and
never more than 20. The six-second estimate above undercounts that term: at
1,800 ft/s with 1.2 G of excess it is about 11 s, not 3. A loaded F-22N at
1,030 kt and 3,000 ft keeps only about 1.5 G; with six seconds (10,300 ft) it
saw a 17 degree escarpment too late and flew into it at full back stick. With
20 s it climbs over. Helicopters and the V-22 keep six seconds.

Other overrides: above its ceiling altitude the aircraft does not accept a
climbing pitch request. On the ground it holds its entry pitch, and may pitch
up only above minimum speed unless in the airborne part of a takeoff; the
ground turn rate is at least 35 degrees per second. In the holding, taxi, roll
and climb-out states the requested flight path is capped at the airport's
4 degree limit. Aircraft with
the gravity flag gain or lose 32 ft/s of speed per second times the sine of
their flight-path pitch, halved when more than 100 ft/s above maximum speed,
and never decelerate below minimum speed while climbing. The bank request is
bounded by the aircraft's maximum bank and a second term that is still
untraced. The airfield states are named and specified in
[AI airfield sequences](ai-airfield.md); there the ground pitch hold governs
rotation on the takeoff roll and the ground turn floor bounds taxi steering.

The weapon-dependent lead used by B15 starts from the target position. For the
predictive weapon branch, distances of 20000 feet or more bypass prediction.
Within that boundary the calculation uses distance, a weapon/launcher speed
estimate and target scalar speed and attitude. For an aircraft launcher within
1600 feet, prediction is reduced according to the larger absolute heading or
pitch difference between the two aircraft: zero at 10 degrees or less, linear
between 10 and 35 degrees, and full at 35 degrees or more. These are differences
between aircraft attitudes, not line-of-sight errors. A non-aircraft aim point
can receive a 20-foot upward offset. The long-range early exit bypasses that
offset too. The speed estimator and short prediction-time cutoff still need
physical-unit closure; do not substitute a generic intercept solver and label
it recovered behavior.

## B45: Seeker visibility, launch eligibility and ammunition

Executable-confirmed: seeker eligibility has separate geometric, signature,
launcher-support and release-service checks. Passing any one is insufficient.
The checked envelope belongs to the selected equipment profile; there is no
single range or cone shared by the twelve aircraft or by every carried weapon.

When range checking is enabled, the seeker checks spatial range against the
profile's inclusive minimum and maximum. It also checks target altitude relative
to the observer against the profile's inclusive vertical limits. Sentinel limits
can disable individual bounds. A signature-adjusted effective range must also
fit inside the maximum. For positive final signature percentage S, that range
is physical range times 100/S, subject to overflow protection; zero signature
fails against a finite maximum range. Thus being inside the nominal range alone does
not establish visibility. Aspect, look-down and a speed-sensitive rejection
branch modify eligibility. Their complete signature producers, configuration
flags and per-store numerical profiles remain unresolved.

Angles are measured in the observer/mount frame. In the forward vertical
hemisphere, absolute horizontal and vertical errors must each fit their own
inclusive limits. For vertical errors beyond 90 degrees, the reviewed rule
accepts horizontal error within the larger of the horizontal limit and 90 degrees,
or vertical error at least 180 degrees minus the vertical limit. Both angular
limits set to their unrestricted sentinel bypass angles, not range checking.

Every store carries two envelopes. The first is what its seeker can acquire,
used for sensor and target search; the second is what it may be employed
against, used for lock, launch, store choice and in-flight support. Each has
its own minimum and maximum range, relative altitude window and horizontal and
vertical angular limits. No air-to-air store carried by the twelve ported
aircraft restricts relative altitude; the AIM-54C and AAM-L cannot be launched
inside 30000 ft. Per-store values live in the imported weapon records, not in
this spec.

Signature producers are executable-confirmed. Detection range equals the
profile's maximum range times the final signature percentage over 100; a
signature above 100 does not extend reach beyond the profile maximum. The
starting percentage is the target's own stored signature for the observing
sensor type (visual, laser, infrared, radar). A passive emitter seeker ignores
stored signatures: 100 percent while the target radiates, otherwise 0. An
infrared sensor sees at least double, never below 200 percent, while the
target is in the hot-engine state or its recent heat window. A radar sensor's
percentage comes from the attitude and configuration model, then two
configuration bonuses of 33 and 25 points each floored at 100. Weather scales
everything; the naked eye is lifted to at least 75 percent within 200 ft and
blended back by 1500 ft; at night a lit target's visual and laser signature
divides by up to 5 between 1500 and 4500 ft. Each seeker's aspect penalty is
subtracted unless the observer is inside a 40 degree elevation, 140 degree
azimuth rear cone or pointing within 30 degrees of vertical; a 100 percent
rear-aspect penalty (the AA-2) cannot see a target from the front, while the
Sidewinder family carries 30 or 20 and radar missiles 0. Look-down rejection
reaches full strength at 45 degrees down and on the deck, vanishing at
5000 ft above ground. The closing-speed gate is unused by roster radars.

Given several usable stations against a target, the AI picks the highest
score: an angular term from the employment envelope (100 minus pointing error
in degrees for guided stores, twice 50 minus error for guns and unguided
stores), plus the store's hit chance against the target, plus 50 for a guided
store beyond 1500 ft, plus the store's damage against the target's category
divided by 25. Eligibility is one class bit: air-to-air missiles only against
aircraft, bombs, rockets and ground missiles only against surface targets,
guns against both. The hit-chance term is an opaque routine not yet stated.

A target must still exist and be usable. Equipment can additionally require a
live launcher, launcher emission, a compatible supporting seeker, or launcher
seeker visibility. Human-controlled aircraft have an additional launch-context
G check against the weapon's tracking limit; that particular gate does not apply
to ordinary AI. Support-required flags must not be flattened into a universal
one-missile support channel. The reviewed AI radar-on check requires the actor's
emission-enabled state and extends its emission-valid deadline to at least
10 seconds ahead. This is not proof of ten seconds of missile guidance after
radar shutdown. The corresponding player path checks its existing deadline.
In flight a guided weapon re-checks its target every update using only the
angular limits of its employment envelope, so it does not lose its target by
closing inside its own launch minimum range. An AI launcher's support does not
lapse on its own while its missile keeps asking for it; a human launcher's
support lapses when the pilot stops emitting. When any check fails the weapon
simply loses its target: it is not destroyed, flies on unguided until its
normal lifetime ends, and does not reacquire. A passive emitter weapon keeps
tracking a stationary emitter that has shut down but loses a moving one. The
existing [missiles spec](missiles.md) retains its requested reacquisition
design as an opinionated host rule, not recovered behavior. A 4000 ft
terminal branch remains open.

For the bomb-type branch when trajectory checking is requested, the predicted
impact must fall within the larger of 1000 feet and one eighth of current range
from the target. This is a release-solution tolerance, not guaranteed bomb
accuracy or a general missile launch radius. B42's AI service checks lock and
terrain before release. The lower-level release routine can instead clear an
invalid or terrain-blocked target and still release the weapon. Preserve the
difference between AI authorization to fire and a weapon's ability to leave
the station without a retained target.

An inhibited station refuses release. Ordinary finite ammunition is debited by
the equipment's actual-rounds-per-game-round amount. A positive remainder smaller
than that debit is allowed and is reduced to zero; zero ammunition refuses the
debit. An unlimited-store sentinel succeeds without decrementing. The global
unlimited-ammunition bypass found in this routine additionally requires human
control, so it is not an unconditional AI exemption. Pod and burst metadata can
make one release operation create multiple projectiles; projectile count is not
necessarily the ammunition debit. Exact special-store and complete burst policy
remain open.

The original debits ammunition before allocating the projectile. An allocation
failure can therefore return failure after a debit. A host implementation may
choose atomic allocation/debit as an explicit opinionated robustness policy,
but must not call that behavior recovered. Report inventory change separately
from projectile creation and target retention in the future service API.

## B46: Wing-command receiver contract

Executable-confirmed for the reviewed aircraft event receiver:

| Request | Receiver behavior |
| --- | --- |
| Break | For an eligible AI aircraft, constructs a nominal five-second motion request at corner speed, with heading relative to the receiver's own body heading, the supplied pitch, and automatic bank selection. A target is not required. |
| Approach | For an eligible AI aircraft with a target, forwards heading/pitch and a speed bounded by its own minimum/maximum. A zero speed request selects corner speed. No target means no approach motion is installed. |
| Horizontal/vertical spacing | Applies the respective formation setter. The sender-side limits remain in B43. |
| Formation selection | Applies the formation setter; the follow-up clears an active ordinary-state command in the reviewed state branch. It is not immediate relocation into a slot. |
| Wing control | Applies the control setter and resets the active command. |
| Target/order assignment | Distinguishes holding fire, restoring free selection, a class/policy request, and a concrete target. A concrete target changes the reaction/target state and establishes a nominal 20-second target-related deadline, with a time-adjustment branch. This is not yet proof that the target is forgotten at expiry. |

Player orders are executable-confirmed: break left and right are 175 and 170
degree heading changes, break low and high are 70 degree pitch changes, and
fly straight is a zero change, each a 5 second request at corner speed.
Approach orders steer 45 degrees left or right of, or 35 degrees below or
above, the target and complete when the wingman is within 2000 ft of its
approach point; the wingman then returns to formation unless assigned an
attack. Whether the approach point is the target itself or displaced is open.
The player's radio call is printed and voiced when it is sent, regardless of
whether the wingman can comply; spacing calls say "Tighten up" below 1000 ft
and "Combat spread" otherwise. Only target assignments get a wingman reply,
an engage reply (heard only with radio traffic enabled) or "Showtime!" for
protect me, from the first wingman. The engage reply is "Engaging" for a ground
or sea target and one of nine lines for an aircraft target or attack on
contact; see [engage replies](radio-chatter.md#engage-replies-correction-to-b46). Break, approach, formation, spacing and control
orders receive no spoken reply.

Disengage puts the wingman back in formation at once and stops it choosing a
new target until the next engage order. A wingman whose target is lost or
destroyed returns to formation by itself; a leader resumes its waypoint. There
is no separate rejoin order or rejoin distance. Bug out is ignored while the
wingman takes off, lands or is on the ground; otherwise it flies a private
route home, lands, parks for good and no longer answers orders
([AI airfield sequences](ai-airfield.md#orders-and-warnings)). A wing-order subcode with no
found sender assigns a target in the second attack state.

Break and approach do not install AI steering on a human-controlled recipient.
The common maneuver eligibility gate rejects original states 1..18 and 21..30;
accepted states 19..30 are normalized by a second helper before the new request.
The combined effect is that states 19 and 20 can transition, while 21..30 are
rejected on that path. Mission-facing names for all of these states remain
unknown, so do not relabel them as landing or refueling solely from their numbers.

The event handler's Boolean result is not an obedience or radio-acknowledgment
contract: some settings are applied while returning false, and a human break
or targetless approach can return true without installing motion. A host receiver
needs distinct applied, rejected, and no-motion outcomes. The 20 second
target deadline's expiry consumer, the approach steering point and
loose-versus-medium self-engagement remain open. The player-side voicing of
wingman replies is in [radio chatter](radio-chatter.md#engage-replies-correction-to-b46).

## Live wing command and radio integration

Implementation mode, 2026-09-18. Commands execute on delivery, independently
of playback. Receiver outcomes distinguish applied settings, installed motion,
rejection and no motion. The UI reports those outcomes per addressed flight;
an accepted assignment is not a claim that weapons have fired. Only an accepted
assignment by the first living wingman may produce its B46 reply. The player
call plays at once; the reply follows two seconds later through the shared
radio channel, as [radio chatter](radio-chatter.md#implementation-in-tore)
specifies, including its variants and radio silence. Commands remain scoped to a side and wing; an optional
actor recipient must belong to that flight. Dead actors do not receive orders.

The following are **opinionated, agent-authored** integration choices. New
player orders interrupt queued order audio so obsolete order calls do not play
after a cancellation; a reply already queued in the radio channel still plays. Radio is a separate FIFO, capped at 16 clips, with
one voice at a time and gain 0.4. Pause freezes it; leaving flight, restart or
muting effects clears it. Missing metadata or recordings produces silence while
text and commands continue. No synthetic speech or substitute phrase is used.
Original phrase/recording mappings are imported as bounded inert data from the
reviewed FA executable, and samples through the existing archive/PCM readers.
Unsupported executable layouts leave radio unavailable rather than guessed.
See [radio data contract](../formats/radio.md).

Player commands include all five B46 breaks, target engagement, disengage,
formation selection, horizontal spacing, stacking and loose/medium control.
Quick Mission initializes neutral formation permission on both sides. Formation
selection and disengage recall the addressed aircraft, canceling pursuit until
a new engagement order or newly perceived attack. The current authored rules
are in [formation and leader authorization](ai-awareness.md#formation-and-leader-authorization).
Attack on contact restores free selection. Engage from formation permits an
explicit target with medium control. Protect me establishes a persistent duty
to protect the player, including when no attacker is currently observed. Each
escort independently acquires aircraft and assesses their threat to the player;
perceived attack reports can raise priority without revealing hidden launchers.
The authored policy, pursuit limits and search rules are specified in
[mission roles and engagement](ai-awareness.md#mission-roles-and-rules-of-engagement).
Approaches use the designated target and each recipient's own bearing/elevation.
The moving target position is the **fitted** approach point because the original
point displacement is unknown. Player approaches assign that target for attack.
The requested 45/35-degree offset tapers linearly from full at 10000 ft to zero
at 2000 ft to avoid orbiting the target, a **fitted, agent-authored** rule.
Completion is within 2000 ft, not merely heading alignment. A destroyed or unavailable approach target cancels the approach and
returns the wingman to its standing formation. A new break, assignment, formation
or control command cancels the old approach.

Formation reports are **opinionated, agent-authored**, from actual guidance
transitions: breakout/separation, rejoining and completed capture. A wingman
still intercepting after 30 seconds with nonpositive closure requests a steadier
platform, without claiming it can never catch up. Reports have a 10-second
per-aircraft cooldown. They are text-only: original mappings for separated,
rejoining, unable to catch up and steady-platform requests remain **unknown**.
Wingman combat calls (contacts, launches, hits, kills, damage, deaths, launch
warnings and fuel) are voiced; see [radio chatter](radio-chatter.md#implementation-in-tore).
Next research is tracing the remaining say-event tables and their senders.
Human controls are never changed. AI leader cooperation remains future work.

Combat target permissions use actor-owned radar/visual contacts where fitted
sensors exist. Formation leader/traffic positions remain direct world snapshots;
these reports do not claim visual contact or sensor-based reacquisition. Actual
visual reacquisition would require timestamped actor-owned visual observations
of friendly leaders, contact-loss persistence, and a search/permission policy.
No new movement override or skill reaction delay is introduced.

## B47: Threat warnings, countermeasures and reason priority

The following is recovered behavior and the existing component contract. The
[M1 missile-awareness specification](ai-awareness.md#missile-awareness-and-defense)
supersede its warning eligibility, delay and selected reaction gates for the
M1 AI path. The replacement is implemented for the default missile rules;
B47 remains available for explicit compatibility behavior and component evidence.

Executable-confirmed. A missile launch warning is delivered only to the
aircraft the missile was fired at. Other aircraft, wingmen included, never
receive it, whatever their equipment. The warning identifies the missile, so
the aircraft knows who fired and whether the seeker is infrared or radar.

The warning is delayed. A human-flown target is warned one second after
launch. An AI aircraft in ordinary flight is warned six seconds after launch,
plus one second for every two statute miles between missile and target at
launch, capped at twenty seconds, plus an experience term of 6, 3, 1 or 0
seconds for Novice through Ace. An AI aircraft in the two attack states can
instead use a base delay of one second when already engaging the launcher
and three seconds otherwise, with the same distance and experience additions.
The reviewed delay arithmetic in the [B47 source map](../formats/ai.md#behavior-source-points)
establishes these as base terms, not total delays. The producers of those states
remain open.
The minimum delay is half a second.

An AI aircraft ignores launch warnings while taking off and during the later
landing states. In the first two approach states it abandons the approach and
returns to free flight. The AI's airfield phases map onto these states as
listed in [AI airfield sequences](ai-airfield.md#orders-and-warnings). An aircraft with no countermeasure dispenser station
does not react at all. A mission-authored hold time can suppress reactions
until a given time of day. A launch by an aircraft on the same side causes no
maneuver. The "SAM launch"/"AAM launch" radio call is sent only for an
opposite-side launcher; see [radio chatter](radio-chatter.md#sam-and-aam-launch-calls).

On a warning the aircraft rolls for countermeasures at 35, 50, 75 or 90
percent by experience level. On success it releases two or three devices a
quarter second apart: radar decoys after a radar warning, infrared decoys
after an infrared warning. It never substitutes the other device, and the
release stops as soon as the matching dispenser is empty. The weapon service
is postponed two seconds. Each device decoys each missile guiding on the
releasing aircraft whose seeker class matches, with an independent roll of
the missile's decoy susceptibility times the device's effectiveness, in
percent. Human aircraft with the unlimited-ammunition option do not consume
devices. The decoyed missile's remaining flight time is shortened by a rule
still to be stated.

No evasive maneuver is flown when the launcher is on the aircraft's own side
or is already the aircraft's current target. Otherwise a wing reaction is
sent and the fighter script runs with the infrared-launch or radar-launch
reason (B10); if the script requests nothing the aircraft reverses course,
left or right with equal probability, at corner speed.

Script reasons are ranked: hit, then infrared launch, then radar launch,
then attack, then evade, then idle. A script still in progress resumes when
the new reason is not higher than the one it was started with; a higher
reason restarts the script from the top. What a restart does to a motion
command already in flight is open.

## B48: Routes, fuel and recovery

Executable-confirmed, partially. A landing waypoint within 60000 ft of an
aircraft with an airport hands it to the airport landing sequence. Beyond
5000 ft from a waypoint, a wing leader varies its commanded altitude by up to
plus or minus 100 ft on each command. Landing waypoints are never commanded
below 2000 ft. Waypoint speed is clamped to the aircraft's minimum and
maximum. Each route command lasts a nominal 5 seconds before re-planning.
A waypoint completes when it is behind the aircraft; a goal-object waypoint
also requires the goal destroyed or the whole wing out of usable weapons for
its class; a ground waypoint requires the aircraft on the ground. With no
route the aircraft holds its current heading. An AI wingman whose leader is
taking off or landing, within 10000 ft of the leader and 40000 ft of the
leader's airport, joins the landing.

Fuel: with a home airport the aircraft computes time to reach it at a cruise
speed (minimum plus one fifth of the envelope, or half if that is under
75 ft/s) and its endurance at the lowest throttle that holds that speed. The
endurance counts all remaining fuel, external tanks included: every AI
aircraft starts with its tanks full, a default load's as well as a given
one's, and burns them first, as the player does (fixed 2026-10-20; before, a
default load's tank, such as the F/A-18's or F-16C's centreline tank, was
carried at full weight with no fuel in it). It is
out of fuel at zero, critical below four minutes of endurance, on bingo when
endurance is under time-to-home plus five minutes, and on caution at or
under time-to-home plus ten minutes. An AI wingman whose leader is AI-controlled
leaves for its home airport on bingo, flying a private landing route at 5000
to 10000 ft and cruise speed, and lands there. An aircraft whose internal
fuel reaches zero is lost. The route ends in a Land goal, so the aircraft lands
and stays parked; the takeoff and landing sequences are specified in
[AI airfield sequences](ai-airfield.md). With a home runway, leaders,
singletons and wingmen of a human leader land the same way (fitted).
When no hostile aircraft remains and no ground strike or CAP duty remains, the
same landing and return path end the mission
([return to base](#return-to-base-when-the-mission-is-over), opinionated).

### Rotorcraft fuel (slices H3 and HF2)

Fitted (agent decisions, 2026-10-12 and 2026-10-13, John's requests that the AI
helicopters get a real fuel schedule and bingo, that rotorcraft burn fuel by the power
their rotor draws, and that the helicopter bingo reserve be five minutes, like a
fighter's). B48 is written for fighters; a helicopter or the V-22 (the aircraft the
[rotor adapter](#rotorcraft-and-jet-lift-control) flies) gets the same machinery with
two differences, the cruise speed and the planning flow. The AV-8 and the Yak-141 keep
B48's fighter rules.

**The AI burns fuel, by the power its rotor draws.** The flight model debits the fuel
every second the engine runs. For a rotorcraft that is not the PT record's flow (1 lb/s
for the AH-64, V-22 and CH-47, 3 lb/s for the Mi-24: placeholders, see [aircraft
records](../formats/aircraft.md#rotorcraft-fuel-records)) but the turboshaft line of the
[flight model](../FLIGHT-MODEL.md#rotorcraft-fuel): about a fifth of the full flow at
idle, rising to full flow at rated power, so a hover burns what hovering draws and a
cruise less. The AI plans with one steady figure (`State::endurance_flow_lbs_per_second`):
since slice HF3 (opinionated, John 2026-10-13: "helicopter bingo planned at cruise flow")
the flow of level flight at its cruise home (below) at sea level, worked out once at
empty plus full fuel and scaled with the weight now (fitted); slice HF2 planned at the
hover flow. At the cruise home speed the cruise flow is about the hover flow or a little
more, so the plan matches the flight home rather than lengthening it. Fuel at zero is
out of fuel and the aircraft goes down.

| | AH-64 | V-22 | CH-47 | Mi-24 |
| --- | --- | --- | --- | --- |
| Internal fuel, lb | 2,000 (record) | 11,700 (MV-22B, slice HF3; the record's 2,000 was the AH-64's) | 7,340 (CH-47F, slice HF3; the record's 3,307 was the Mi-24's) | 3,307 (record) |
| Flow of the record, lb/s (no longer read) | 1 | 1 | 1 | 3 |
| Top speed of the envelope at sea level, kt | 173 | 301 | 196 | 196 |
| Cruise home (80 percent), kt | 138 | 241 (airplane mode) | 156 | 156 |
| Planning flow at full fuel, lb/s (cruise home, sea level) | 0.52 | 1.44 | 0.83 | 0.44 |
| Full tank at the planning flow, minutes | 64 | 135 | 148 | 126 |
| Hover flow, lb/s (HF2's planning figure) | 0.47 | 1.36 | 0.65 | 0.43 |
| Full tank at 100 kt level, minutes | 93 | 244 | 232 | 199 |

(The probe that prints these is `powered_probe`; the table is in the [rotorcraft fuel
baseline](../baselines/rotorcraft-fuel.md).)

- **Cruise.** The time home is the straight distance to the home field at 80 percent of
  the envelope's top speed (`route::rotorcraft_cruise_speed`), and bingo flies the way
  home at that speed. B48's cruise (the envelope's minimum plus a fifth) is 55 knots
  for the AH-64 and 80 for the Mi-24, the figure that, with ST-H's 15 minute home, made
  every Mi-24 bingo at the start. 80 percent is where a helicopter cruises in practice.
  **Fitted.**
- **Margins are B48's** since slice HF2 (**opinionated**, John 2026-10-13): critical
  under 4 minutes of fuel, bingo under the time home plus **5 minutes**, caution plus 10.
  Slice H3 had used 2, 3 and 7 minutes because the record's flow gave tanks of 18 to 55
  minutes.
- **A home out of reach is no home.** The schedule has a home only when a full tank
  (at the planning flow) could fly to it and leave the 5 minute reserve
  (`route::rotorcraft_home_in_range`). A wing the Quick Mission puts farther from its
  field than that would be bingo at the start and fly home instead of the fight it was
  placed in. With no home it has no bingo, caution or critical, as B48 has for an
  aircraft with no home airport, and flies its mission until its field comes within
  reach. It is lost when the tanks run dry. **Fitted;** the alternative (send it home
  at once) was weighed and left out because it would neuter enemy helicopter wings. The
  reach grew with the endurance: a field up to 114 nautical miles from an AH-64 is in
  reach (it was 53), 99 for the V-22 (53), 357 for the CH-47 (123) and 292 for the
  Mi-24 (36). Slice HF3's cruise planning and the V-22's real tank move these: at the
  cruise home speed a full tank less the reserve reaches about 135 nautical miles for the
  AH-64, about 300 for the Mi-24, about 370 for the CH-47 (its real tank since slice
  HF3) and about 520 for the V-22.
- **Not bingo at the start.** A friendly wing's field is normally a few minutes away
  (3.5 minutes at the AH-64's cruise in the probes) against 64 to 148 minutes of fuel.
- **What bingo does.** The same as for a fighter: the actor heads for its home runway
  at the cruise speed, and since slice H2 lands there (a helicopter vertically).

Tests: `route::tests` (the cruise, the margins, the range), `flight::powered::fuel_tests` (flow by power, endurance per type), `ai_wings::tests::a_helicopter_has_a_fuel_schedule_at_its_own_cruise_and_goes_home_on_bingo`
and the battery's `ai-fight-*-friendly-wings` (no helicopter bingo at the start, fuel
lines from `--probe-fuel-trace`), `ai-fight-heli-fuel-bingo-ah64` and `ai-fight-v22-fuel-bingo`
(`--probe-fuel`).

## Implementation status

Renderer-independent components live in `crates/tore-sim/src/ai/`. Each file
names the behavior IDs it implements; unresolved branches return
`AiError::UnspecifiedRule` rather than a default, so a component never invents
a number and presents it as recovered.

A live controller cannot stop flying at those branches. `ai::controller`
sequences the components and, wherever one reports an unresolved rule, applies
one named rule from `ai::fitted` and records it. Every such rule is fitted,
never recovered retail behavior; they are listed in
[behavior provenance](../behavior-provenance.md) and reachable per actor
through `Controller::fallbacks`.

| Component | Implements | Not implemented, returns unspecified |
| --- | --- | --- |
| `experience` | Explicit, editor and Quick Mission resolution, enemy-skill override, tactical tables, G adjustment | Nothing; template ground skill values are data, not code |
| `geometry` | B01 through B05 predicates and distances | Bearing when both aircraft share a horizontal position (reported as unknown) |
| `tactics` | B10 reactions, B11 evasion entry, B12 approach and best/random choice, pursuit offsets, B14 attack selection | Last-ditch candidate redraw, random-tactic menu contents, dive-bomb profile |
| `motion` | B13 request limits, maneuver builders, quarter-second deadline clock, free-flight pitch early completion | Zero-duration axis selection |
| `pursuit` | B15 frame and speed bands, B44 lead scaling and bypass | Speed estimator and prediction time |
| `targeting` | B41 retention, eligibility and three-penalty ranking | Priority route, surface selector |
| `steering` | B44 approach without overshoot, turn rate and radius from G and speed, roll caps, authority floors and mode limits, terrain floor and cadence, ceiling, ground and gravity overrides (the ground pitch hold and turn floor are used by `airfield`) | Base pitch rate, second bank-bound term; the authority curve shapes are labeled fitted |
| `weapon_service` | B42 phases and retries, timing profiles for all twelve aircraft, B45 ammunition debit, seeker envelopes by role, detection range and stated signature modifiers, class eligibility, store score, in-flight track check and AI support extension, device schedule | Missile burst pacing after a shot, hit-chance rule, signature producers that need sensor state |
| `gunnery` | [Live gun employment](ai-gun-employment.md): shared-physics lead, fixed-barrel release, individual-round burst/recovery and tracking | Original skill accuracy, timing and maneuvering-target tracking |
| `threat` | B47 warning delay, receiver gates, countermeasure gate and dispenser selection, decoy roll, script fallback reversal, reason ranking | Decoyed-missile time shortening, restart effect on an in-flight move |
| `helicopter_defense` | B20 helicopter break: plus or minus 90 degrees to the threat's bearing, the side by the smaller turn, terrain-masking search, speed and height kept ([AI strike](ai-strike.md#helicopter-defence)) | Retail's turn toward a target that is ahead; the threat's site when no site is known |
| `route` | B48 waypoint completion by octant, route command with landing hand-off, leader jitter and floors, join-landing, cruise speed, fuel states, wingman bingo route (also flown by leaders and singletons, fitted) | Original damage-triggered disengagement policy; authored response is in `damage` |
| `airfield` | [AI airfield sequences](ai-airfield.md): hold, turn and runway-free gates, taxi-out, line-up, roll, climb-out, marshal square, approach gates, final, rollout, taxi-back and parking slots; bug-out, bingo, ordered and join landings; player landing priority | Carrier variants; retail nose attitudes on final and rollout and the stopped pivot have fitted substitutes; the [vertical and STOVL variants](ai-airfield.md#vertical-takeoff-and-landing) (slice H2) fit the descent rate, the helicopter marshal height and the air taxi |
| `wing` | B43 spacing clamps, formation table and names, player spacing values, mode 9 speed, control side effects, target sharing cap; B46 receiver outcomes, player break/approach values, reply rules | Approach steering point, mode 9 negative-band entry |
| `fitted` | One named, documented fitted rule per unresolved branch a fighter/strike actor can reach, with its constants | Nothing; this file exists because the branches are unresolved |
| `controller` | `Controller::new` and `Controller::step`, persistent state, seeded draws at documented decision points only, reason ranking, target selection, tactical choice, motion resolution, weapon cadence, wing requests, fuel | Families other than fighter/strike are rejected, not served fighter behavior |
| `steering_adapter` | Motion intent to flight controls through the B44 rate limits, and the AI-only experience G adjustment | Ceiling test and ground contact are caller concerns; the control gains are fitted |
| `rotor_adapter` | [Rotorcraft control](#rotorcraft-and-jet-lift-control): steer and hover requests flown on the helicopters and the V-22 through the autopilot's rotor laws, the V-22 nacelle schedule | Nothing retail; every gain and number is fitted |
| `jet_lift` | [Jet-lift control](#rotorcraft-and-jet-lift-control): the AV-8's and Yak-141's hover, vertical climb and descent | Transition to wingborne flight and short takeoff (slice H2) |
| `mission` | Actor-owned sensors, stores, flight model and decision state; ammunition debited before a launch event | Missile physics deliberately not duplicated; the host realises each launch |
| `damage` | [Authored fault responses](systems-damage.md#ai-pilot-response-to-faults): recovery commitment, power protection/restart, maneuver limits and fire danger | Original AI procedures; unreachable or missing recovery runway and irrecoverable failures can still be fatal |
| `launch` | Quick Mission launch payload: side, wing, member, aircraft, resolved experience and the enemy-skill override | Nothing; loadout carriage stays with the host |
| `opportunity` | [Mission of opportunity after a lost human leader](#mission-of-opportunity-after-a-lost-human-leader): the wing's pooled last known enemy positions, the search order, dwell and time limit, the waypoint leg, and when it returns to base (opinionated, fitted numbers) | Nothing; the rule is an opinionated addition |

Fitted and opinionated choices are listed in each file's module comment and in
the [provenance summary](../behavior-provenance.md).

## Proposed host API

The following is an opinionated interface design for future Rust implementation.
Names and records in this section are proposed, not existing crate exports.
Use explicit units and typed decisions rather than script sensor-name strings
or executable addresses. Keep the simulation independent of rendering.

```rust,ignore
pub fn resolve_experience(
    request: ExperienceRequest,
    policy: &ExperienceAssignment,
    random: &mut DecisionRandom,
) -> Result<ResolvedExperience, AiError>;

impl Controller {
    pub fn new(identity: ActorIdentity, profile: BehaviorProfile,
               experience: ResolvedExperience, seed: u64)
        -> Result<Self, AiError>;

    pub fn step(&mut self, frame: &DecisionFrame<'_>)
        -> Result<IntentBatch, AiError>;
}

pub fn target_geometry(own: &OwnState, target: &KnownTarget)
    -> TargetGeometry;
pub fn choose_aircraft_behavior(context: &AircraftContext<'_>,
    random: &mut DecisionRandom) -> Result<BehaviorChoice, AiError>;
pub fn choose_surface_behavior(context: &SurfaceContext<'_>,
    random: &mut DecisionRandom) -> Result<BehaviorChoice, AiError>;
pub fn plan_maneuver(choice: &BehaviorChoice, context: &MotionContext<'_>)
    -> Result<MotionIntent, AiError>;
pub fn decide_weapons(context: &WeaponContext<'_>)
    -> Result<WeaponIntent, AiError>;
```

The last five functions describe separable calculation boundaries for tests;
they need not all become public crate exports. `Controller::step` is the main
per-actor entry point. It accepts one immutable snapshot per advancing 120 Hz
simulation tick. Weapon search/preparation timings are specified in B42; other internal decision
cadences remain separate policies that are not yet fully specified. It must not redraw a tactic every tick
merely because the host calls `step`. Pause supplies no advancing ticks.

### Records and required inputs

| Record/entry point | Must look at | Must produce or preserve |
| --- | --- | --- |
| `ExperienceRequest` / `resolve_experience` | Explicit per-object level versus wing/editor setting, side/domain assignment channel, documented assignment policy | Level 0..3 plus origin; no reroll during ordinary updates, no fallback from unknown Quick Mission policy to editor jitter |
| `ActorIdentity` / `BehaviorProfile` | Stable actor identity, exact aircraft/object type, side, wing/member, family and mission role | Separate identities for family, capability and role; reject unsupported bindings rather than substitute another aircraft |
| `DecisionFrame` | Simulation tick, own state, permitted targets, threat events, orders, wing state, environment and service feedback | All views from one simulation snapshot, with observation timestamps and absent values represented explicitly |
| `OwnState` | Pose, scalar/ground-relative speed as distinct quantities, AGL and absolute altitude, fuel, damage, stores and current flight limits | No renderer-derived state or shared player globals |
| `KnownTarget` / `target_geometry` | Observation source/time, pose when known, permitted class/control metadata, validity and observed destruction | Spatial/horizontal range, separate heading/pitch errors, ahead/facing/off-beam and explicit unknown geometry |
| `AircraftContext` / `choose_aircraft_behavior` | Current reason/order, experience, geometry, performance comparisons, wing state and active maneuver | Behavior choice and reason; conditional probability draws only at documented decision points |
| `SurfaceContext` / `choose_surface_behavior` | Object class, ground experience, role, route, mobility, sensor and weapon constraints | Class-specific intent; stationary actors may request weapons without motion |
| `MotionContext` / `plan_maneuver` | Behavior choice, aircraft or surface limits, terrain, target/reference frame and ongoing maneuver progress | Desired motion with explicit altitude/offset frame and completion rule; never direct position writes |
| `WeaponContext` / `decide_weapons` | Launcher ownership, target role, inventory, legal firing solution, range/arc, sensor support and current orders | Sensor/track and weapon/device requests; separate inventory debit, projectile creation and retained target in feedback (B45) |

Proposed frame events include `ThreatReported`, `Hit`, `OrderChanged`,
`TargetUnavailable` and `ActorRemoved`, each with tick and actor references.
A threat report records its source, guidance knowledge and whether a launcher
or projectile identity is actually known. Do not manufacture warning knowledge
from a raw global missile list. Event priority and coalescing remain policy
gaps; the old interpreter's numeric reason ranking is not an accepted host rule.

Use feet and feet/second at existing simulation boundaries, radians for host
geometry and explicit conversion for recovered degree rules. Use named altitude
and offset frames rather than a bare vertical number. Performance differences
whose retail scale is unresolved must not masquerade as physical measurements.
Unresolved steering, eligibility or interruption semantics return `UnspecifiedRule` in a
research harness. An implementation can later supply a documented fitted policy
as an explicit profile, never as an invisible default.

### Intents, feedback and persistence

`IntentBatch` separates motion, sensor/target selection, weapon/device requests,
wing requests and activity. A weapon request names actor, station, target and
request identity so retry/feedback cannot fire twice accidentally. Motion has
an identity and completion rule. Feedback distinguishes completion, rejection,
cancellation, lost reference and still-in-progress, with a reason.

Store active behavior, target/order identities, maneuver progress, resolved
experience, deterministic random state and last processed tick in controller
state. Include this state in future replay/save snapshots. A removed actor
cannot remain a usable target; restart creates fresh state from the recorded
initial configuration. These are host design requirements, not recovered
retail scheduling details. Destruction knowledge and physical sensor presence
remain separate, as in the existing sensor component.

### Runtime service boundaries

| Existing boundary | Connection and limitation |
| --- | --- |
| `sensors::Sensors::contacts`, `visual`, `observation`, `support` | Build permitted target views and track feedback from each actor's own sensor state. `Observable` is service input, not automatic AI knowledge. |
| `sensors::Sensors::designate` and `step` | Apply validated sensor requests and advance observations; current channel/track rules stay in the shared component. |
| `combat::live::State::readiness`, `mounted_solution`, `step` | AI projectiles reuse combat simulation with owned records. The AI boundary applies selected-store gates; full AI seeker lifecycle remains open. |
| `flight::State::step_surface` and selected aircraft model | Convert intent into controls and step the actor's model without post-step movement writes. `autopilot` supplies reusable steering ideas, not a claim of recovered combat steering. |
| `quick_mission::QuickMission::dummy_wings` | Replaced by `QuickMission::wing_launches`, which carries side, wing, member, aircraft and resolved experience through `ai::launch`. `dummy_wings` remains as the flattened legacy view so the fixture path is unchanged. |

## Input-only aircraft control

**Opinionated, requested by John on 2026-09-18:** every AI-controlled aircraft
must move only through the physical aircraft inputs it commands. This applies
to formation, route flight, combat and evasion. The aircraft flight model alone
updates attitude, speed, velocity, position and achieved telemetry. AI may not
replace those results to meet a desired heading, rate limit, slot or maneuver.
Spawn/restart initializes a pose; destroyed airframes use the shared combat
wreck simulation. Render interpolation never changes simulation state.

This intentionally supersedes the earlier B44 post-step attitude integration.
B44's original fast turn formulas remain research facts and isolated reference
services, not a reason to grant extra movement authority. Replaying a tick's
AI inputs from the same flight state and environment must reproduce the whole
resulting flight state exactly. Existing player flight adapter selection stays
distinct. Airborne AI starts on the legacy model; ground-started AI and actors
starting a landing sequence use the researched model for runway contact. Both
are covered by synthetic input replay tests. AI does not inherit the player
adapter setting, and the restricted native-table AI path is not introduced.

The following feedback controller is **fitted, agent-authored**. The original
input mapping is unknown. Unconstrained heading error divided by 1 second
requests turn rate, converted to bank using atan(speed times rate / 32.174),
with radians/second and ft/s, speed floored at 125. Requested bank is bounded
by the aircraft maximum, acos(1 / positive G limit floored at 1), and 60 degrees
in close formation or 75 degrees otherwise. Explicit maneuver bank requests keep
their aircraft bound. Desired roll rate is bank error / 0.7 seconds minus half
the measured roll rate, bounded by the B44 requested roll authority. Divide by
the model's current roll authority and clamp stick roll to [-1, 1].

Pitch feedback requests (cos(flight-path pitch) + speed times pitch error /
(3 seconds times 32.174)) / max(cos(bank), 0.25) G, clamped to the loaded
negative limit and AI positive G limit. On the researched model the AI positive
limit is never below what that model itself gives: 1 G anywhere inside the
1 G envelope, and its loaded low-speed ceiling (the ramp from 1 G at the
flap-adjusted stall to the next envelope row) where that is higher (fitted,
agent decision 2026-09-29; before this a loaded aircraft slowing on final was
held to 1 G divided by its loading, 0.77 G for a fuelled F/A-18D, and sank
into the ground short of the runway). Pitch error includes the terrain floor.
When an airborne aircraft's flight path is below the terrain floor, the
requested bank is zero while it stays below, so a steep turn cannot use the
lift needed to climb away from the ground, and the pitch error is closed over
1 second instead of 3, in the spirit of B44's extra pitch authority while the
floor is active (fitted, agent decision 2026-09-28, after the bug battery saw
undamaged AI aircraft fly into hills at full G while still turning toward a
target, or ease into a rising hillside at 2 G; the airfield terrain correction
already levels the wings the same way).
A striker's attack steers alone may ask for two more handlings (AI strike
round, slice ST-T2; fitted, agent; [how the AI flies a
pass](ai-strike.md#how-the-ai-flies-a-pass)). **Roll and pull**: the lift is
rolled onto the wanted direction and pulled, so a nose-down change of 30
degrees or more that needs more than a 1 G push rolls inverted and pulls
(retail's dive bomb roll-in), at the aircraft's full roll rate, with the load
held off while the lift is more than 45 degrees from its bank. **Planned dive**:
the terrain floor is the dive's planned pull-out (the physical pull-out arc at
the AI's G limit plus 1 s of sink, bottoming out 1,000 ft over the highest
ground ahead) instead of the floor above. Other motions use the plain handling
above except for the [missile defensive turn policy](#missile-defense-flown-by-ai).
Invert the model's loaded stick-to-G mapping, with low-speed authority floored
at 0.01 only for division, and clamp pitch input to [-1, 1]. Rudder stays zero.
For the researched model, this includes its flap-adjusted minimum speed,
continuous low-speed positive-G ceiling, current flap lift and carried mass.
Using the clean-aircraft mapping with flaps extended caused repeated floating
approaches. These are host flight-model inputs, not additional aircraft power.
The aircraft model may lag, depart, stall, overshoot or fail to achieve the
requested maneuver. Steep/inverted maneuver tracking remains approximate.
Throttle retains the fitted speed-error rule: current throttle plus speed
error / 100 ft/s, bounded to [0, 1]; requested speed uses the loaded envelope.
Fuel and engine state can prevent acceleration even at full throttle.

Imported AI stores use the same payload convention as player live combat:
external equipment plus non-internal remaining rounds times nonnegative source
weapon weight. A release subtracts only the mass of ammunition actually debited.
Remaining health still reduces requested G and roll authority linearly as a
fitted controller restriction, but never asks for less than 1.25 G, or the
healthy limit where that is lower: a hit aircraft must still hold its flight
path (fitted, review 2026-10-25; a fault-free but hit F/A-18 on a heavy final
was held below 1 G and flew into the ground short of the runway). Actual axis-specific faults and regional damage
also act through the shared flight model. [Fault responses](systems-damage.md#ai-pilot-response-to-faults)
end offensive engagement when recovery is needed and add protective power,
speed and maneuver requests. These never overwrite achieved flight motion.

### Rotorcraft and jet-lift control

*Built in slice H1 of the helicopter and VTOL AI pass (2026-10-12; AI work
requested by John). Fitted, agent decisions, unless a row says otherwise.*
The controller above flies a load factor on the stick and a speed on the
throttle. On a helicopter the stick tilts the rotor disk, the throttle is a
governed engine's power lever and the lift comes from the collective, which
that controller never moved: a helicopter could hardly climb or descend, slowed
until it sank when asked for less speed, and could not hover. The helicopters
(AH-64, Mi-24, CH-47) and the V-22 on the researched model are therefore flown
by their own adapter (`ai::rotor_adapter`), which takes the same steer requests
(heading, flight path, speed with Maximum resolved, explicit or free bank) and
hover requests the airfield sequences use. It flies through the player
autopilot's own laws (`autopilot::rotor_law`, the [hover hold and coupled
heading modes](autopilot.md)) with targets it commands, so the player's and the
AI's rotorcraft obey one set of gains. Every other aircraft keeps the
controller above unchanged.

| Rule | Value | Label |
| --- | --- | --- |
| Engines | The throttle is held at 1: governed engines, power never cut for speed | fitted (agent) |
| Forward flight | Climb on the collective, speed on the cyclic (the ground speed along the heading, the wind along it added so the airspeed is the one asked), pedals coordinating the turn. Climb = speed x sin(flight path), at most 3,000 ft/min either way (the helicopter defence's 10 degree path at 150 kt); pitch at most 15 degrees | fitted (agent); the law and its gains are the player A modes' |
| Bank | An explicit bank, or the heading error over 1 s as a turn rate, banked as atan(speed x rate / g) with speed floored at 40 kt; at most 60 degrees or the aircraft's maximum bank (45 left a Mi-24 at 158 kt circling a lead-in point it could not turn inside) | fitted (agent) |
| Height before speed | Below 1,000 ft over the ground, outside pointing: the cyclic is asked for at most 10 ft/s of speed change at a time; sinking more than 2 ft/s below the descent asked (or below level when a climb is asked; any sink once the collective is at full travel or the rotor droops under 95 percent) it stops accelerating, slows by up to 10 ft/s and eases the bank by up to half. A 180 degree turn at 200 ft asking 160 kt loses 15 to 20 ft (without it the AH-64 flew into the ground) | fitted (agent) |
| Terrain floor | The B44 floor on the flight path, as for every aircraft, but with a minimum height of 100 ft for a helicopter instead of the fighters' 300, so the 200 ft masking height of the helicopter defence and the stand-off attack can be flown. The V-22 keeps the fighters' 300 ft (slice HF1): it never masks, and at 250 kt in aeroplane mode it needs their margin over rising ground | B44 spec-derived; the 100 ft and the V-22's 300 ft fitted (agent) |
| Strike speed | The helicopter's strike legs, exit, hold-off and break ask for 80% of its envelope's maximum speed, never under its minimum: about 120 kt on the Mi-24 (maximum about 150 kt). See [helicopters](ai-strike.md#helicopters) | about 120 kt opinionated (John, 2026-10-13); the 80% share fitted (agent, slice HF1) |
| Collective floor | A helicopter in forward flight never flies the collective under 0.1 of its travel (the climb loop and its integrator are held there; the V-22 and pointing, which has its own 0.2, are not touched). Wound to the stop by a long descent at 120 kt (a wingman chasing its slot 400 ft lower asked 42 ft/s down for 10 s), a CH-47's rotor lost its control power: the nose swung up to 85 degrees away from the track while the aircraft kept flying straight, the speed along the nose read near zero, the speed loop pitched the nose down to 68 degrees chasing it and the CH-47 flew into the ground (battery `ai-fight-ch47-enemy-wing`, slice CH47, 2026-10-13). 0.1 still flies the CH-47 down at 1,000 ft/min at 120 kt | fitted (agent, slice CH47) |
| Formation descent limit | A helicopter or V-22 wingman chasing its formation slot is asked for at most 1,000 ft/min (16.7 ft/s) down, or its leader's own sink when that is more: the formation's flight path is raised to that at the wingman's speed (40 kt floor), climbs untouched (`formation::rotorcraft_formation_path_deg`, applied in the controller's formation motion). The fighters' slot geometry had asked a CH-47 wingman for 28 degrees down at 59 kt, up to 80 ft/s with the pointing regime engaged (battery `ai-fight-ch47-formation-descent` without the limit). 1,000 ft/min is an ordinary helicopter descent and what the CH-47 flies at 120 kt above its collective floor | opinionated (John, 2026-10-13, "helicopter limit on formation descents"); the figure fitted (agent, slice HF3) |
| Defence's speed rule | The helicopter defence comes down no faster than it can hold its maximum speed; the adapter holds the speed on the cyclic whatever the path, so the defence allows it 10 kt over the maximum before it levels off | fitted (agent) |
| Pointing | A requested path steeper than 12 degrees that also asks for more than 3,000 ft/min (a pop-up's pull, a rocket run's dive at speed) is flown with the nose on the path (pitch up to 30 degrees) and the speed on the collective (never under 0.2 of it); back to forward flight below 8 degrees | fitted (agent): rockets leave along the nose |
| Hover | Hover hold's cascade toward a commanded point, wheel height and heading: position to ground velocity (0.15 per second, at most the commanded closing speed) to tilt (at most 20 degrees) to attitude; height to climb (0.25 per second, within the commanded climb and sink) to collective; heading to pedals. An optional slow-flight velocity rides on top (air taxi) | fitted (agent); the gains are hover hold's |
| Hover entry | The cascade takes over at a hover request below 40 kt of ground speed (hover hold's band). Faster, it slows first on the forward law toward the point, the speed wanted being sqrt(2 x 4 ft/s² x distance ahead) | fitted (agent) |
| V-22 nacelles | A schedule against indicated airspeed: 87 degrees (the published helicopter preset) to 40 kt, 80 at 80, 75 at 110 (the wing's published stall), 60 at 125, 45 at 135, 30 at 145, 15 at 155, 0 from 165. Asked to slow, the nacelles come up as if 2 kt slower for each knot too fast. The demand stays inside the angles the corridor allows, never aft within 10 kt of the 200 kt aft lock, and the speed asked is paced to 10 kt inside the corridor at the present angle, so the protection never has to override it. 87 degrees in a hover | fitted (agent); the preset, the corridor and the stall speed published |
| V-22 law | From 75 degrees of nacelle up (down to 72 once in it) the forward law flies the collective as on a helicopter; below (wingborne), the climb on the pitch and the speed on the collective, its power lever, as the player A modes do, the climb error handed to that law four times over (a few feet a second of sink is a fraction of a degree of path at 280 ft/s). Wingborne the climb is speed x sin(flight path) with no 3,000 ft/min cap, and the pitch may reach 25 degrees: until slice HF1 the helicopter's cap held a V-22 at 240 kt to a 7 degree path, flatter than the terrain floor's 10 to 14 degrees over rising ground, and AI V-22s flew into hills 22 s after an airborne start and about 100 s after a ground start | fitted (agent) |
| Memory | The laws' integrators and the regime are the actor's own and are checkpointed | spec-derived (checkpoint rule) |

Measured on the PT records over flat ground (the [rotor
probe](../testing/lane-flight.md#rotor-probe) and its unit tests): every
rotorcraft holds a point within 3.1 ft in calm air and a 15 kt wind, moves its
point 500 ft in 31 to 33 s without overshoot, climbs from 20 to 500 ft at 25 to
27 ft/s and settles on 500 ft, flies 40, 80 and 120 kt climbing or descending
1,000 ft/min within 35 ft/min and 3.2 kt, and slows from 120 kt to a hover over a
point 3,000 ft ahead losing at most 5.4 ft, and turns 180 degrees at 200 ft asking
160 kt losing 15 to 20 ft. The V-22 converts from a hover to airplane mode in 25 s
on its way to 220 kt and back to a hover, within 88 ft of its height, with no
corridor override; converted at 240 kt and asked by the terrain floor for a 12
degree path, it climbs at more than 65 ft/s within 10 s (57 ft/s at most before
slice HF1). In Quick Missions AI V-22s in every wing fly 300 s from an airborne
start and 400 s from a ground start without a loss
(`ai-v22-quick-mission`).

**Jet lift** (`ai::jet_lift`, the AV-8 and Yak-141). Used only for the hovers,
vertical takeoffs and landings the airfield sequences ask for (slice H2);
wingborne flight stays on the controller above. Throttle for height (0.03 per
ft/s of climb error plus an integrator), the nozzle for the speed along the
heading (the wanted acceleration as an angle off vertical, from 60 degrees to
the braking stop), the bank (at most 10 degrees) for the speed across, the nose
held level, the heading on the pedals through the puffers. Fitted, agent. On the
PT records, clean, both hold a point exactly in calm air and set down at a
commanded 5 ft/s sink: the AV-8 touches at 4.9 ft/s, the Yak-141 at 7.5 ft/s
(the jets lose lift to suck-down near the ground). A loaded AV-8 cannot hover
(the manual's "never be able to fully hover"); the law then sinks at full
throttle and the caller must choose a runway: the airfield sequence takes the
retail short takeoff and the runway landing for a jet that needs more than 90%
throttle to hover (slice H2, [vertical takeoff and
landing](ai-airfield.md#vertical-takeoff-and-landing)).

## Missile defense flown by AI

John requested effective missile defense on 2026-10-25. The following policy
is **fitted, agent-authored**, not measured retail behavior. It uses only the
actor's observed threats. Missile energy is unknown in that observation API;
time to closest approach is a timing estimate, not an energy estimate.

After first observing a threat in an encounter, Novice, Average, Experienced
and Ace pilots wait
1.5, 0.75, 0.25 and 0 seconds respectively. Sensor warning delays still apply.
With more than 18 seconds to closest approach for radar, or 14 for IR, turn
cold and extend at maximum dry speed. At shorter times beam the missile, or
notch the supporting radar bearing when known. A drag enters the beam at 14
seconds for radar and 10 for IR. Do not return to drag during that encounter.
At 1.5, 2.5, 3.5 and 4.5 seconds respectively, commit to a hard break toward
the missile's side while it is closing, requesting at least a 90-degree
heading change. Within 15 degrees of the nose, keep the current turn side when
bank exceeds 5 degrees; otherwise keep the committed beam side, or turn right
if no side is established. This avoids straightening at a head-on missile.
A missing estimate or one non-closing closest-approach projection does not end
an established break. Keep its heading and queued pair while passage is uncertain.
Confirm passage from fresh, non-stale observations showing opening range over
at least 0.75 seconds, at least 500 ft of separation gained, and a current range
of at least 1,500 ft. A renewed closing estimate or decreasing range resets that
confirmation. A gap longer than 0.5 seconds between range observations also
restarts the confirmation window. Re-reading a frozen report is not new evidence. These thresholds
are fitted agent decisions. Confirmed passage ends the motion and bursts;
another closing missile retains priority. Remember each missile's break heading
across switches and reacquisition. An observed renewed closure can reactivate it.

Choose the shorter beam turn. Add up to 30 degrees of cost for pointing toward
other observed threats, and 360 degrees for insufficient terrain clearance
along the turn. Hold the chosen side as the bearing moves. Only an unsafe
terrain path with a safe opposite path can reverse it. Threats classified as passive
use alternating 45 degree jinks. Each leg lasts at least 4 seconds,
or the estimated 90 degree turn plus roll-in and a skill-dependent dwell of
3, 2.5, 2 or 1.5 seconds, whichever is longer. Do not reverse before reaching
within 15 degrees of the leg's heading.

Missile defensive fixed-wing turns use bank and positive pull. The usable positive G
limit is the existing aircraft/pilot limit multiplied by 0.85, 0.90, 1.0 or
1.0 by skill (lead decision, 2026-10-25), with a 1 G floor where available. Bank is bounded by the aircraft
and the coordinated bank for that load, up to 85 degrees. Defensive roll closes
bank error over 0.10 seconds, bounded by aircraft roll authority. For aircraft
with measured control acceleration, release roll at the calculated stopping
angle, including one tick of travel and a 0.5-degree margin. For lag-response
aircraft subtract the roll still expected during the response time before
commanding the remaining error. These are fitted agent control choices.
For defensive bank commands of at least 35 degrees, blend from 1 G to the
skill limit over the first 30 degrees of bank toward the turn, then request the
full limit. Do not scale that pull down to 85 percent for a descent target.
The aircraft still owns achieved G and roll. Small bank corrections use flight
path feedback. No negative-G push is requested. The terrain
floor overrides the turn and levels the wings for recovery. Before descending,
check the current and requested headings and intermediate turn directions over
5 seconds, retaining at least 1,000 ft above sampled ground. Repeated horizontal sample positions share a terrain read within that decision.
After selecting a beam side, only that side is checked unless it becomes unsafe;
then the opposite side is checked before switching. The active path is still
sampled every tick with the same points and clearance. This sampling
cannot guarantee clearance over unresolved narrow terrain. No defense unload
is currently needed. Ordinary pursuit and formation control are unchanged.

Radar beams dispense two chaff at most every 6, 4, 3 or 3 seconds by skill,
once the turn is established (at least 5 degrees of bank toward its requested
heading), already within 20 degrees of that heading, or within 6 seconds of
closest approach, whichever comes first. These are fitted turn-start thresholds;
the all-skill release rule is the lead's decision of 2026-10-25.
Beams and breaks cut afterburner and cap throttle at 65 percent for a visually
seen missile unless radar evidence has classified it as radar-guided. Known IR
threats also request this heat reduction. Below 1.3 times minimum speed, preserve
needed throttle. This visual-threat policy is the lead's decision in the second
review. A missile already associated with radar guidance never requests the cut.
IR flares begin inside 2, 3, 5 or 6 seconds to closest approach.

All missile defense shares an eight-device request budget per encounter. One
missile can spend at most six. Keep the final pair of the budget and each
dispenser for a different missile's last-ditch break. Its first break burst can
use that reserve before the ordinary beam cooldown expires, within the
eight-device total, but never less than two seconds after the preceding pair. This six-plus-two allocation is an agent decision
implementing the lead's second-missile reserve requirement. Subsequent break
pairs start at least 2.0 seconds apart for every skill (John, 2026-10-27),
including across a threat switch. Earlier skill-dependent reaction times and
ordinary beam intervals remain. Devices in a pair
are 0.25 seconds apart. Queued defensive releases survive uncertain passage
and a brief lost observation; confirmed passage cancels them. Compatibility
warning releases keep their independent schedule.

Known radar threats get only chaff, known IR threats only flares. Keep learned
classification for the encounter even while the missile is absent from the
threat picture, including electronic-to-visual handoff. Unclassified visual
threats get precautionary flares and the heat cut; this does not claim knowledge
of their seeker. Unclassified electronic warnings use chaff. Stale observations
request no new bursts.

Switches retain reaction progress, cooldown, budget and the completed drag phase.
A lost observation can support the committed motion and queued pair for two
seconds (240 simulation ticks). An explicit combat notice that a store is
inert ends that store's threat and defense immediately, including during this
grace. It does not clear other threats or reset encounter reaction progress,
release cooldown or the device budget. This follows John's 2026-10-27
[inert-store policy](missile-energy.md#motion); a missing observation alone
is not such a notice. Encounter memory, including class and per-missile break heading,
lasts until 30 seconds without a fresh observation of a missile aimed at this
aircraft, a fitted agent choice. The aircraft's own missiles in flight and
missiles seen going elsewhere do not keep an encounter open. A longer quiet
spell starts a new encounter. When defense ends, resume existing orders,
formation or combat without changing aggression or launch gates.

## Live integration and authored boundaries

Implementation repairs on 2026-09-17 connect pursuit, delayed warnings, weapon
ownership, release gates, countermeasures, wing ranking and wing orders. These
are behavior fixes, not evidence of retail combat parity. Validation lives in
[the AI baseline](../baselines/ai-research.md).

- Combat owns a destroyed airframe's existing ballistic fall. A dead controller
  never overwrites its position or velocity; restart creates a fresh bridge.
- Pursuit retains its chosen target-relative offset for the maneuver, while
  recomputing heading, pitch and regulating speed against the moving target each
  tick. Breaks and escapes keep their independent heading requests. The one-degree
  geometric completion tolerance is fitted, rather than B13's exact equality.
- A warning is queued until its launch-relative deadline and consumed once.
  Target equality stands in for the unknown attack-state producer, a fitted
  choice. Equal and lower-priority reasons preserve an active maneuver.
- Imported AI actors carry their own PT default weapons and ECM counts. Using
  the PT default loadout when Quick Mission provides no AI loadout is an
  opinionated agent choice. The player's wing instead shares its accepted
  [QMC load](quick-mission-menu.md#the-players-flight-loadout), including empty
  stations and fuel; human multiplayer flights follow the
  [same-wing rule](multiplayer.md#human-flight-loadouts). A projectile owns its selected weapon record; guns
  remain unguided. Live guns authorize individual rounds, then debit at physical
  emission after capacity and barrel checks. Their fitted tracking and authored
  burst policy are specified in [gun employment](ai-gun-employment.md). Synthetic
  fixtures retain their earlier prepaid representative-group path.
- Selected-store range, separate angular and relative-altitude limits, mount
  position, target class, required emission and supporting sensor gates precede
  release. The existing host envelope geometry remains fitted. Terrain checking
  shares the combat segment query, which samples eight intervals and refines a
  crossing ten times. Very narrow intervening terrain can remain unresolved by
  that host sampling rule. The score receives pointing error, never angular margin.
- Device debits and the defensive dispense schedule are unchanged. Energy
  rules evaluate physical devices once per missile per two-second episode and
  pursue a successful decoy, as specified in the
  [energy countermeasure contract](missile-energy.md#countermeasure-contract).
  The preserved Spec adapter rolls per device and clears guidance to coast for
  the record's remaining lifetime. Original lifetime shortening remains unknown.
  Model timing is available through the additive
  `State::missile_time_to_impact` interface for already perceived threats;
  this missile change does not replace the separately owned defensive policy.
- AI movement follows the [input-only control contract](#input-only-aircraft-control).
  B44 remains a reference for requested limits, not a post-step pose override.
- Wing-assignment penalties count other live attackers on the same side and in
  the same wing. Automatic requests are delivered after all actors decide, and
  player requests address friendly wing 1 only. Each recipient retains its own
  formation and spacing overrides. Combat spacing is requested only with a target.
  Disengage prevents self-selection until another target order.

AI missile steering still uses the compatibility path, with actor-owned
launcher emission support. Full AI seeker acquisition, activation and pitbull
are not implemented. B12's unknown wing-approach producer remains disabled.
The remaining original maneuver shapes and approach-order completion are open.
A status label alone is never evidence of a completed maneuver.

## Acceptance and next research boundary

First synthetic cases should test B01/B02 at 89, 90 and 91 degrees, separating
heading and pitch; B03 with equal horizontal positions and different altitude;
B04 at minimum speed plus 74/75/76; experience choice boundaries from the linked
spec; and B12's inclusive distance/angle and 76% entry boundaries. Source-only
branch tests are provisional until relevant BI correspondence is checked.

Add B15 cases at each side of -100, -50, 50, 250, 1000 and 5000 feet of
separation error, plus off-beam rejection and own-speed limiting. Test B41
retention at 19999/20000 feet and the cumulative wing-assignment penalties.
Test each B42 aircraft timing group, delayed lock retries, pause and advancing
simulation time, and B43 spacing clamps and persistent variation deadlines.

Test climb/dive fallback, distinct IR/radar requests, an unarmed aircraft's
attack request, non-aircraft targets, absent/stale observations and removed
actors. Test the host API proposal for actor isolation, deterministic restart,
no duplicate fire and no per-tick skill rerolls once it is implemented. These are synthetic regression scenarios, not visual or retail acceptance.

Add B44 tests for roll-in, opposing bank, rate limiting and the 1600/20000-foot
lead boundaries; B45 tests for inclusive envelope limits, zero signature, support
loss, inhibited/empty/unlimited stores and a final partial ammunition debit; B46
tests separating applied settings from motion installation and handler results.
The B01 through B05, B12, B15, B41, B42, B43, B44, B45 and B46 cases listed
above now exist as synthetic tests in `tore-sim::ai`; they test the specified
numbers, not flown acceptance.

Quick Mission skill handling is resolved in the experience spec. Next resolve
steering performance producers, terrain overrides and completion exceptions;
signature producers and per-store envelopes; in-flight support-loss and
reacquisition; and remaining wing orders and approach completion. The
consolidated backlog lives in [M1e](../ROADMAP.md#1e-ai). B44 through B46 close the reviewed connections
and explicitly identify the branches still preventing complete behavioral closure.
Surface engagement follows its separate source map. Broader maneuver and
family coverage remains open. This document can grow by complete behavior
sections without waiting for byte-level closure of the entire executable.

The user-requested [Dummy training mode](dummy-aircraft.md) bypasses the combat
controller and aerodynamic flight. Its constant 400-knot motion is an explicit
exception to the normal AI flight-model path described above.

## Pilot ejection

The [ejection specification](ejection.md#fitted-ai-decision) owns the requested
recovery calculation, healthy-aircraft safety guard and per-second chance.
The mission evaluates escape before weapon releases; a successful ejection
stops aircraft control while the detached pilot continues descending. Combat
retains ownership of the abandoned wreck. These additions are fitted and
opinionated where labelled, not recovered retail AI decision predicates.

## Defensive store jettison

John requested AI defensive jettison on 2026-10-15. The shared
[jettison contract](jettison.md#replay-network-and-ai) defines tank shedding,
merge range, skill-dependent strike-load retention and inert recording behavior.
Since John's 2026-10-20 revision a fight drops only the tanks; the
air-to-ground load goes only when the pilot must save the aircraft (damaged or
short of fuel going home, or losing a close defensive fight).
