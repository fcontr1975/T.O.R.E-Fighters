# Air-to-air awareness, memory and mission rules

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

## Scope and status

Development specification for M1 air-to-air awareness, memory, search, missile
defense, RWR missile presentation and mission engagement. Visual awareness,
aircraft memory and contact-loss search are implemented in the existing aircraft
runtime. Missile defense and shared AI/RWR missile information are implemented
for reviewed missile profiles. Mission roles, engagement rules and per-group
Quick Mission objective and survival selectors are connected to both sides' aircraft. Delivery sequencing is defined in
[M1e](../ROADMAP.md#m1-air-to-air-awareness-delivery).

The requirements are **opinionated** gameplay behavior. Existing equipment,
flight limits and missile guidance contracts retain their documented provenance.
This specification does not establish original-game parity.

The [visual-contact and incoming-fire investigation](../baselines/visual-awareness-under-fire.md)
found shared limits in neutral leader release, rear lookout and gun/hit defense.
Its [repair contract](visual-awareness-under-fire.md) is implemented, including
lookout, mission-permitted contact release and anonymous incoming-fire defense.

### Component provenance

| Component | Provenance |
| --- | --- |
| Skill-scaled 3 to 5 mile visual awareness, memory durations, Ace recall and Novice single-target restriction | Opinionated product requirements |
| Visible searching, mission-based engagement and escort priorities | Opinionated product requirements |
| Guidance-dependent warning onset, visual-only passive-missile detection, jink/notch/dive and countermeasure capability | Opinionated product requirements |
| Skill-dependent defensive commitment and re-engagement; shared AI/RWR missile information and blinking incoming threats | Opinionated product requirements |
| Nautical-mile interpretation, intermediate visual ranges, cone geometry, intermediate-skill capacity, sensor refresh and lifecycle knowledge policy | Opinionated agent-authored defaults |
| Search motion, warning delivery scheduling, threat classification and persistence, maneuver selection, burst scheduling, timing margins and escort limits | Opinionated agent-authored defaults; numeric tuning remains subject to acceptance |
| RWR snapshot allowance, visual-threat display integration, plotting and blink cadence | Opinionated agent-authored defaults |
| Missing visibility effects, missile notch rejection and passive countermeasure susceptibility | Unknown where not specified; fitted rules require documented constants before implementation acceptance |

Agent-authored defaults below define the initial development configuration. They
are not recovered original-game values.

## Integration baseline

The [AI spec](ai.md) and [experience spec](ai-experience.md) describe existing
rules. Four resolved skills exist: Novice, Average, Experienced and Ace. Use the
resolved pilot skill, after existing assignment rules, not the menu selection.

The runtime provides actor-owned sensors, weapons, flight state and controllers.
`ai::awareness` holds current observations separately from frozen aircraft memory.
`ai::mission` feeds only current observations into combat selection and supplies
lost hostile snapshots to the controller's search input. Visual attention uses
its own skill-scaled cone. Radar and infrared use the shared sensor component;
terrain blocks both sensor and visual observations. Production sensor import
failures are reported instead of using the sensorless fixture path.

`controller::Activity` exposes searching, acquiring and rejoining through the
existing Target window. SEARCHING also describes an airborne leader following
its current heading before first acquisition; wingmen retain formation activity.
Mission route import remains unavailable. Explicit protect/destroy assignments
feed `ai::engagement`; the original `MissionDefensePriority` branch remains a
research gap rather than a runtime requirement. Search returns to the assigned
protection, patrol center or existing formation/heading behavior.

The current host supplies terrain but no weather visibility limit to AI
perception. Cloud/night visibility remains a fitted clear-air assumption, not
complete environmental visibility. Initial integration and validation limits
are recorded in the [awareness baseline](../baselines/ai-awareness.md).

### Evidence references

The local 1999 EA/Jane's Fighters Anthology manual,
`.local/missile-update/manual.pdf`, SHA-256
`1a082378a8e8cd163ed6b398efcc1df80b67c2f104f6b90ac0733c88d58e26c3`. Printed
pages 94 and 110 describe radar-emission detection, up to 50 NM; page 97
separates RWS search from TWS targeting; page 101 explicitly lists SEARCHING,
ACQUIRING and PURSUING activities. Printed page 131, PDF page 135, distinguishes
enemy seeker tracking from an inbound missile. Page 131 was visually inspected;
the other cited pages were reviewed through extracted text. This establishes
manual-described behavior, not executable timing or a claim that every TWS/RWS
selection generates a warning. Executable build provenance for existing rules
remains in the [AI baseline](../baselines/ai-research.md).

## Visual awareness and memory

Use nautical miles and the existing 6,076 feet per NM convention. Visual range,
retention duration and memory capacity depend on resolved pilot skill.

| Resolved skill | Visual range, NM | Memory after last observation | Remembered aircraft capacity |
| --- | --- | --- | --- |
| Novice | 3 | 15 seconds | One selected hostile |
| Average | 3.5 | 90 seconds | All previously observed aircraft |
| Experienced | 4 | 120 seconds | All previously observed aircraft |
| Ace | 5 | Until destruction or mission end | All previously observed aircraft |

Memory capacity for Average, Experienced and Ace is bounded by the mission
aircraft count.

Each visual attention sample is a circular cone with a 60-degree half-angle,
identical at every skill, with skill changing range only. Measure 3D angle from
the gaze direction and spatial distance; include exact angle/range boundaries.
The [pilot lookout](visual-awareness-under-fire.md#seeing-an-aircraft-and-choosing-to-engage)
combines the forward cone, a timed scan and attention to a last measured point. This is separate from B01's ahead/off-beam combat predicate. Terrain
blocks visual acquisition. Reuse available environmental visibility limits,
capped by the skill range; any missing cloud/night occlusion remains explicitly
fitted and must not be described as complete weather visibility. Do not alter
the player's sensors or imported equipment profiles. AI attention filters the
visual channel separately from radar and infrared hardware.

A remembered record stores stable identity, last observed world position,
velocity, observation tick and observation source. Relative bearing is
recomputed from that frozen position and own movement. A lost contact never
receives hidden position, heading or velocity updates. Aim search at its last
observed position without indefinite velocity extrapolation. Ace's unlimited
duration is perfect recall of an old observation, not perfect tracking. No
future target turn becomes known without a new observation.

A valid current visual, radar or infrared observation refreshes the remembered
record and timer. Keep source timestamps separately. Radar observations can
sustain awareness outside the visual cone. Scope history and a mere selected
marker do not count as current observations. At loss, preserve the last measured
record; expire it exactly at the configured age using simulation ticks: Novice
1,800, Average 10,800, Experienced 14,400. Pause does not age memory.
Reacquisition refreshes it.

Novice may perceive several current contacts but remembers only its chosen
hostile. Changing target discards the old memory. On a kill or expiry it cannot
select an old forgotten aircraft. It must freshly detect another aircraft on its
own scope, or visually inside its 3 NM cone. A wing order or warning may cue a
search but must not silently refill Novice's target memory.

Destroyed actors become ineligible immediately. Simulation lifecycle removal
clears their memory records at every skill, including an unobserved kill. This
is a bounded knowledge shortcut, exposes no killer or survivor positions, and
must not synthesize a radio kill confirmation. Do not attack detectable wrecks.
Clear all records on mission restart and never reuse an old record for a new
actor ID.

Memory authorizes investigation, never a weapon solution. Firing and missile
support still require the existing appropriate live sensor/seeker geometry,
weapon envelope, ammunition and terrain checks. Target retention inside B41's
20,000 feet cannot bypass the new awareness or mission eligibility gates.

## Searching and player-visible activity

Required states and transitions:

- SEARCHING: no current eligible observation, or investigating a remembered
  location. Fly toward that location while scanning; with no memory, follow the
  assigned patrol/intercept route or stay with the protected formation.
- ACQUIRING: a current eligible contact is chosen but weapon acquisition is
  incomplete. Maneuver and request the appropriate sensor designation.
- PURSUING and ATTACKING: retain existing tactical/weapon services, with a
  current permissible observation and a mission-eligible target.
- DEFENDING or EVADING: an actionable threat interrupts ordinary engagement;
  after it clears, reassess the mission and available observations.
- REJOINING or RETURNING: engagement is no longer permitted, memory expired,
  escort separation exceeded its limit, or existing fuel/survival rules require
  withdrawal. Never call this LANDING before an actual landing phase.

Turn toward the last observed position; within 1 NM of it, request a level orbit
through the existing steering adapter. Stop investigating on memory expiry, a
higher-priority threat, mission completion or the escort leash. For an Ace, cap
one uninterrupted investigation at 120 seconds before returning to its mission,
while retaining the record. This prevents unlimited recall from causing
unlimited abandonment of an escort.

The fitted search orbit is clockwise, level at entry altitude, with a 0.75 NM
(4,557 feet) radius. Correct radial error proportionally over 0.25 NM (1,519
feet), clamped to 45 degrees toward or away from the center. Measure orbit
radius horizontally. Correct altitude toward entry altitude over one orbit
radius of horizontal look-ahead, with pitch limited to plus or minus 45 degrees.
Reevaluate guidance each simulation tick using bounded 3-second motion requests
at corner speed through the existing flight adapter. Approach
the last observation directly outside the 1 NM entry radius. These are
agent-authored search constants; the manual's SEARCHING label does not establish
orbit geometry or scan timing.

Expose these states from the simulation to the existing Target window activity
line. Keep the existing tactical goal codes; SEARCHING is an activity, not a new
unsupported goal letter. Preserve the distinction between attacking the player
and attacking somebody else. Populate the objective row only from player-relative mission requirements. The display reads state and cannot drive decisions.

Optional development visualization: cone extent, last observation point, age,
source and decision reason. Keep this in a debug overlay. Target-view activity
is required; the overlay is optional and cones are not part of normal gameplay.

## Missile awareness and defense

Radar search, contact selection and aircraft tracking do not themselves tell the
AI that a missile has been fired. Passive reception may still reveal an emitter
for ordinary awareness; it never manufactures a missile threat. Warning onset
comes from the actual missile lifecycle or an incoming missile seen by the
pilot. This warning contract replaces B47 launch-warning eligibility and delays
in the M1 AI path. The recovered [B47
behavior](ai.md#b47-threat-warnings-countermeasures-and-reason-priority) remains
the reference for the existing implementation, not the acceptance target for
this replacement.

Use the [missile specification's guidance
categories](missiles.md#four-game-guidance-types). A denotes active radar with
an initially silent midcourse phase. S denotes continuously supported radar. E
denotes passive emitter homing and is distinct from an A missile before pitbull.
Preserve the existing weapon classifications.

| Guidance | When the AI learns of the incoming missile | Initial device response |
| --- | --- | --- |
| A: active radar | No automatic launch warning during silent midcourse. Warn when its active seeker acquires this aircraft and enters PITBULL. | Chaff |
| S: supported radar | Warn immediately on a valid supported launch against this aircraft; the launcher must maintain the required track for guided flight. | Chaff |
| I: infrared | No automatic launch or seeker warning. Respond only after visually detecting the incoming missile. | Flares, including the documented precaution for unclassified visual threats |
| E: passive emitter / other passive guidance | No automatic launch or seeker warning. Respond only after visually detecting the incoming missile. | Precautionary flares unless independent classification identifies a radar threat |

Immediate means the next fixed simulation tick, at most 1/120 second. Do not add
the old B47 launch-distance, flight-state or experience delays to these warning
events. An A missile already active at boresight launch can warn immediately if
it acquires this aircraft; an active seeker searching empty space does not alert
every aircraft. Use the missile's actual active/target state, not a second AI
calculation of activation range. Merely entering ACTIVE SEARCH does not
establish a missile targeting warning. The missile spec defines PITBULL as
successful active-seeker acquisition; use that event for both cued and boresight
shots. Reacquisition cannot create duplicate launch events or infinite bursts.

Visual missile detection uses the pilot's skill-scaled visual range and cone,
with terrain and available visibility limits, including exact boundaries. No
rear-hemisphere or beyond-range launch event may reveal an IR/passive missile.
Visual detection can also reveal a radar missile before its electronic warning.
Motor burnout alone does not make the missile invisible; seeing a launch flash
without seeing the incoming missile is not sufficient.

The perception service may test world geometry to produce a sighting, but the
controller receives only the observed missile position/motion, tick and source.
Classify a visually observed missile as incoming when measured motion is closing
and its current straight-line closest approach is within 1,000 feet of own
aircraft over the next 15 seconds. Use two successive visual samples to estimate
motion; a single silhouette does not reveal its intended target. Once judged
incoming, a missile that is still sighted stays incoming for the same 2-second
grace after its last incoming sample (fitted, agent decision 2026-09-28: a
closest approach hovering at 1,000 feet otherwise flipped the judgment, and the
aircraft's defense, on every tick). Validate these
authored thresholds against guided crossing approaches before acceptance. Do not
use the hidden missile target ID to grant visual knowledge. Visual-only sighting
does not reveal seeker class or launcher identity automatically.

Keep perceived missile threats separate from aircraft target memory. A Novice's
one remembered hostile does not prevent reacting to several visible missiles,
and a missile sighting does not reveal a forgotten launcher. Retain a lost
visual threat for 2 seconds using its last observation, then clear it unless
sighted again; passive missiles cannot refresh this record remotely. Electronic
threat reports may refresh while supported guidance or an active seeker remains
directed at the aircraft. On signal loss retain the last warning for the same
2-second grace, without live geometry, then reassess. Destruction, impact and
expiry retire the missile's threat record. These bounded lifecycle shortcuts
convey no surviving aircraft positions. Aircraft memory timers above are
unchanged.

### Maneuvers, countermeasures and skill

The current fitted [missile defense policy](ai.md#missile-defense-flown-by-ai)
defines the drag, beam/notch and last-ditch sequence, positive-G steering,
terrain guard, side commitment, skill timing and finite device schedule.
It supersedes the initial two-second alternating jink and steep pitch-target
dive. Visual detection alone does not identify guidance: an unclassified
visual missile prompts precautionary flares rather than a mixed burst.

Awareness, maneuver commitment and device release remain separate decisions.
A defensive maneuver does not automatically trigger devices. Empty dispensers
do not prevent maneuvering. Weapons-hold orders prohibit offensive fire, not
defense. Prioritize the smallest positive time to closest approach from current
observed motion, with bearing-only warnings ahead of ranged threats whose
approach time is unknown, and keep the current threat on a tie. Changing the
selected threat retains the aircraft's device cooldown and routine budget.

The diagnostic maneuver-time estimate uses half the load-limited coordinated
turn rate at current speed, bounded by aircraft bank, and half the available
pitch acceleration above 1 G. Roll-in allowance is requested bank plus current
bank magnitude divided by loaded roll rate, plus 0.7 seconds. These fitted
estimates are not trajectory predictions and do not expose missile energy.
The defense policy uses observed closest approach and its documented thresholds.

After the perceived threat clears, resume the mission's priority selection. Ace must preserve a valid offensive solution through a defensive maneuver when
feasible, and promptly re-engage when defense permits. Own missile support is a
tactical constraint, never a reason to ignore an imminent impact. If survival
requires breaking supported guidance, do so and let the existing missile
support-loss rules apply. Attack the launcher only if independently identified,
currently observed or legitimately remembered and permitted by the mission
rules. A silent or anonymous missile cannot reveal its attacker. Escorts return
to protecting their charge instead of automatically chasing revenge. Novice must
still obey its single-target memory and fresh-reacquisition rule when its former
target is gone.

Active missile seekers use the shared Advanced notch preset: a 60 ft/s radial
speed half-width and a 0.45 center range factor at full ground clutter. Clutter
uses the target's actual height above terrain and the downward sight angle.
This is a fitted shared default, not weapon-specific recovered rejection.
Supported missiles lose measured steering when their launcher's own radar loses
support; they may reacquire before guidance expiry. A notch can shorten detection
range or break support; it never guarantees missile defeat. Passive E countermeasure
susceptibility also remains weapon-specific and unknown where not specified.
Ground/emitter homing can be tested with fixtures. A2G AI is out of scope.

### Shared RWR missile information and display

RWR supplies missile information to AI and displays missile contacts for the
player. Known incoming threats blink for the receiving aircraft. The
[manual-based RWR specification](rwr.md) defines symbols, indicators and layout.

Create one actor-owned threat-information service in `tore-sim`, consumed by
both AI and the player's RWR presentation. No AI-only omniscient missile list,
and no UI-generated threat decisions. Each record identifies the observed
missile, evidence source, observation tick, bearing, any permitted range/motion
estimate, known guidance class if available, and whether it is known to threaten
this receiver. Include freshness and loss state. Launcher identity remains
optional and must not be inferred from hidden missile ownership.

RWR information policy: once an S launch warning or A pitbull warning is
eligible, the shared RWR service supplies missile bearing, range and
relative-motion snapshots every 30 ticks (0.25 seconds), with an initial
snapshot on warning receipt. This is an explicitly opinionated gameplay
allowance for ranged RWR missile plots and skill-dependent response assessment,
not a claim about real passive receiver ranging or recovered retail precision.
Only eligible known radar threats receive it; no midcourse A, unseen I/E, hidden
launcher pose or future guidance trajectory is exposed. Controller and display
use the same snapshot and its age. Do not look up the missile's live world pose
between updates. Bearing-only observations remain valid when a ranged report is
absent; they use the conservative defense rule above. Validate this initial
exact-sample approximation in play before considering fitted estimation error.

Own launched missiles also appear as steady dots, as described by the manual.
The shared service supplies own-ordnance snapshots at the same 30-tick cadence,
an opinionated telemetry allowance that does not reveal another aircraft's
silent weapons. Unrelated active missile emitters provide bearing-only records
within 50 NM unless independently seen. Only a directed S warning or acquired
A threat supplies the authored radar threat range/motion snapshot. The player's
visual receiver uses the 5 NM cone as an agent-authored host default; AI uses
its resolved skill. RWR failure suppresses electronic reception but not vision.

Visual I/E observations feed the same threat service as visual evidence, never
as electronic detections. Show a visually acquired incoming missile on the
combined RWR threat display too, with its visual source retained internally.
This does not grant a passive missile warning before it is seen. Its
range/motion and incoming judgment come from the visual observation rules; no
hidden seeker/target metadata is added. Radar-emitter reception by itself can
identify an emitting missile only if that classification is established; it
cannot assert who the missile targets. Unknown emitter plots stay unknown.

Presentation contract:

- A known missile targeting this aircraft blinks as a missile dot. A detected
  missile not known to threaten this aircraft stays steady. Never blink solely
  because it belongs to the enemy, is selected, or exists in the world.
- S against this aircraft starts blinking with the immediate launch warning.
  A starts blinking at actual pitbull against this aircraft. I/E appear only
  after visual detection and blink only when the observed incoming test supports
  a threat to this aircraft. The visual threat judgment is an authored estimate,
  not access to the passive missile's actual target assignment.
- The same missile may blink for one aircraft and be steady or absent for
  another, according to each receiver's knowledge. No wing-wide propagation of
  missile position snapshots is implied.
- Use heading-relative bearings and the existing 5/10/20/30/50 NM display scales.
  Plot measured range only when present. Unknown-range bearings
  use a distinct rim tick, and a ranged threat outside the selected scale uses
  a clipped rim marker. Neither is drawn as a falsely ranged dot. Display scale
  and whether the RWR window is open do not change AI threat knowledge.
- Blink cadence: 60 simulation ticks on, 60 off, one full cycle
  per second at 120 Hz. Pause freezes phase; rendering rate does not affect it.
  A blink's off phase never removes the threat from AI state.
- During the 2-second lost-observation grace, show the last bearing/range as
  stale with a steady marker; cease blinking when current targeting evidence
  ends. The AI may finish its conservative defense during that grace, but it
  receives no new measurements. Remove expired, impacted or destroyed contacts
  and clear all records on restart.
- Respect RWR failure for electronic observations and the failed player panel.
  A failed receiver does not blind the pilot to visually seen missiles, and
  that visual evidence can still drive AI defense. No separate AI exemption.

Player RWR missile plots and warning onset are in scope. Human audible-warning
changes are out of scope. Any audio/visual timing mismatch must be recorded as
an integration limitation.

## Mission roles and rules of engagement

### Formation and leader authorization

Quick Mission starts every wing on both sides in formation with neutral
engagement permission, including wings assigned free fire, intercept or escort.
Mission objectives describe duties and remain separate from permission to begin
combat. Neutral aircraft run their sensors and retain observations, but neither
a detected hostile nor a remembered target independently starts a pursuit before
permission. The AI leader can now grant permission on an eligible current contact. This startup and
recall policy is opinionated behavior requested on 2026-09-22.

Accepted engage-target, attack-on-contact, engage-from-formation and protect-me
orders authorize only the addressed recipients. Protect me establishes its duty
without requiring a current attacker. Rejected commands, break maneuvers, spacing,
stacking and control changes do not authorize engagement. Formation selection
also acts as a return-to-formation order: cancel pursuit, target designation,
ordered approach and investigation, then stay neutral. Disengage uses the same
neutral recall. Mission objectives and aircraft memory survive the recall.

An accepted player recall also cancels the addressed aircraft's queued gun
shots. Already emitted projectiles retain their normal lifecycle. The current
host debits a gun burst when it accepts the launch event; canceled queued rounds
are not refunded and are not counted as failed launches. This ammunition
accounting is a fitted limitation of the burst scheduler.

AI-led wings remain neutral until their leader issues an engagement order.
The M1 automatic leader responds to a newly perceived attack on itself, a wing
member or an assigned protected aircraft by releasing its wing to perform their
assigned duties. A current hostile contact can also trigger release when its
mission policy permits engagement, provided the leader has not been explicitly
recalled. This is an agent-authored extension of the original attack-only rule.
The automatic order applies only to still-neutral wing members, preserving any
individual assignment already accepted. Reports reach leaders on the next tick; the leader's order is
delivered after that tick's decisions and takes effect on the following tick.
The human-led wing never receives an automatic AI-leader release. A recalled
individual remains neutral when the rest of its wing is already engaged.

Missile evasion and countermeasures, and [gun/hit defense](visual-awareness-under-fire.md#breaking-and-returning-to-the-mission), remain available during neutral flight.
A newly perceived identified attacker can also authorize immediate individual
self-defense. After recall, already known missiles and pre-order attack reports
cannot restart offensive combat; a new perceived attack or a new accepted lead
engagement order is required. An old incoming missile can still require evasion.
Track attack identity separately from repeated warning updates so an ongoing
RWR warning is not mistaken for a second launch. Weapons-hold mission assignments
still prohibit offensive fire, including automatic leader release. Restart
restores neutral formation permission regardless of prior combat orders.

### Roles and contact priority

Separate role (what to accomplish), stance (when allowed to engage), and current
activity (what the aircraft is doing). Skill controls awareness and execution;
it does not rewrite the mission. Supported stances are weapons hold,
self-defense, protect-assigned-aircraft and engage-assigned-hostiles. Weapons
hold prohibits offensive firing but permits evasion and countermeasures;
self-defense permits action against a perceived immediate threat. Explicit hold
orders take precedence over automatic target selection. Keep existing survival
and fuel limits authoritative.

| M1 role | Priority among known, eligible contacts | When to stop pursuit |
| --- | --- | --- |
| Free engagement / combat air patrol | Immediate threats, then assigned hostiles, using existing distance/wing assignment ranking within a priority | Target knowledge expires, patrol boundary or survival limit |
| Intercept | Assigned hostile aircraft, with self-defense interruption | Objective destroyed, order canceled, or survival limit |
| Escort / protect | Identified attacks threatening the protected aircraft, then known hostile escorts obstructing that defense, then detected aircraft threatening its protection zone, then other assigned threats | Threat clears and a higher duty applies, or escort leash exceeded |
| Self-defense / disengage | Immediate threats to self; otherwise return to assigned flight | Threat clears; do not chase incidental enemies |

Before each role's ranking, handle immediate inbound threats to self using the
existing defense service. Escort response must restore protection after the immediate threat clears. Enemy escorts use the
same rules to protect their own assigned aircraft.

Escorts assess their own current radar, infrared and visual contacts every
simulation tick. Their installed radar searches while powered, using its normal
physical coverage, terrain masking, interference and acquisition limits. The
default radar channel and 10 NM TWS setting retain search and fire-control
capability; that display scale does not limit physical detection. Protection
does not require waiting for a hostile to fire first.

A detected hostile aircraft threatens a charge if it is within 10 NM spatial
distance of that charge, or if it is within 30 NM and its observed relative
course will enter the 10 NM protection zone within 60 seconds. Use the observed
heading, pitch and speed and the assigned friendly charge's velocity to project
constant relative motion. Beyond the immediate zone, require closing motion;
a passing or departing aircraft whose projected course misses the zone does
not qualify. Assess each live assigned charge independently. These distances
and the projection horizon are agent-authored, opinionated threat-assessment
settings, not recovered original behavior. The protection zone admits only
independently detected hostiles; it grants no additional sensor coverage.

Confirmed identified attacks outrank these prospective threats. An unknown
missile warning cues a bearing search when there is no eligible observed
aircraft, without identifying its launcher. A qualifying observed threat can
be intercepted while its launcher identity remains unknown. Lost aircraft can
be investigated using their frozen observation and the same protection-zone
assessment, subject to skill memory and the escort leash; firing still requires
reacquisition. Incoming missiles aimed at the escort itself interrupt this duty
through the shared defense service. A warning for a charge does not make that
missile an incoming threat to the escort or trigger wasteful defensive bursts
on the charge's behalf.

Mission context must identify protected aircraft, destroy objectives, escort
relationships and any patrol region. Allegiance or proximity alone does not
identify an escort or objective. For M1, supply these explicit assignments from
Quick Mission presets and synthetic scenarios, independently of the
[missions and campaigns](../ROADMAP.md#milestone-3-missions-and-campaigns)
importer. Preserve the existing default Quick Mission behavior until these
assignments and defaults are deliberately connected.

Escort reporting policy: protected aircraft share perceived missile threats and
observed attacks with their assigned escorts on the next tick. Share the
observed threat bearing and only identities actually known, not the hidden enemy
world pose. A missile warning alone cannot identify its launcher as an aircraft
target. Escorts must independently acquire the threat before firing. This narrow
report is separate from any future wing-wide contact sharing and does not defeat
Novice's single-target rule. Treat perceived missile/attack events as threat
evidence; do not read an opponent's private controller goal. Mission-known
escort identity is metadata, not detection.

Escort pursuit limit: 10 NM from the protected aircraft, with re-engagement
allowed inside 8 NM to avoid oscillating at one boundary. Outside the leash,
evade immediate attacks and rejoin; do not initiate another chase. Within one
priority keep the current eligible target to limit switching; a new
higher-priority threat may preempt it. Use B41 ranking within a newly selected
priority and stable actor ID for exact ties. Patrol extent comes from the
assignment; an absent region does not authorize unlimited patrol.

### Assignment delivery and observed attack reports

Explicit accepted target orders replace the aircraft's current assignment with
an intercept of that target. Protect Me creates a persistent escort assignment
for the player. Return-to-formation and disengage recall the addressed aircraft
without rewriting its mission. Routine break and spacing commands do not
rewrite the mission or change engagement permission.
Target selection may retain a currently detected assigned aircraft outside its
weapon range so it can approach. Actual firing still requires a valid weapon
solution, including range, direction, support and terrain checks.

Attack reports expire exactly 240 ticks after their last observation. An actor
receives its own observations immediately; assigned same-side escorts receive
copies on the next simulation tick. Reports preserve their original observation
time and do not refresh themselves by forwarding. Unknown attackers remain
unknown. A world-relative bearing may direct a level search at corner speed,
using one-second bounded motion requests, but supplies no range, aircraft memory
or weapon target. This cue ends on expiry, a real target, recovery, own missile
defense or the escort leash.

A supporting radar can be associated with an attacker only when exactly one
independently observed hostile RF emitter lies within 2 degrees of the received
supporting-radar bearing. Active-missile radar bearings never identify the
launcher. A visible departing missile or tracer can identify a shooter only
during its first 30 ticks, with exactly one independently observed, visually
eligible hostile aircraft within 1,000 feet of the observed departure point.
Incoming trajectory evidence is required; hidden projectile target/owner IDs
cannot establish that association. These are fitted identification rules.

Escort distance is horizontal. Escort leaders follow the assigned live friendly
charge using the existing delta guidance; members retain their own wing leader
only when that leader has the same protection assignment. Leaders use existing
delta slots 1, 4 and 7 by wing index. Outside the 10 NM leash, rejoin the charge;
resume engagement at or inside 8 NM. Rejoin guidance uses corner speed with
pitch limited to plus or minus 20 degrees in three-second bounded requests.
Immediate missile defense and fuel recovery take precedence. CAP returns toward
its assigned center outside the patrol radius when it has no permitted target.
An observed intruder inside the station can be engaged while the patroller is
outside it; returning to station must not override that pursuit. Before first
contact an AI leader also navigates its station, with its unreleased wingmen
in formation. An explicit recall still suppresses this autonomous navigation.
After a target leaves the permitted region or is lost, the wing resumes patrol.
Clearing the sky does not complete a CAP assignment; fuel, damage and explicit
return orders still send it home. This is spec-derived implementation of John's
CAP correction (2026-10-15), with the station selection below fitted by the agent.
Full route navigation remains outside this slice.

<a id="quick-mission-objective-stamps"></a>

### Quick Mission objective selectors

Each of the three friendly and three enemy groups has an objective selector in
a briefing sentence, using the same font, inline field boxes, spacing and popup
controls as the other Quick Mission parameters. A primary assignment reads
`Your primary target is enemy group 1.` or `Wing 2's primary target is friendly
group 1.` Free fire reads `Your flight will use free fire.` Click the highlighted
field to choose; right-click cycles backward. Tab and arrow navigation reach all
six objective fields and the six survival fields.

| Objective | Assignment |
| --- | --- |
| Use mission setting | Inherit the selected mission preset; normal default is free engagement |
| Free fire (any opposing group) | Engage observed eligible hostiles without a designated primary group |
| Combat air patrol | Patrol a 10 NM horizontal circle centered on that wing leader's launch position (fitted, agent decision, 2026-10-15); a human-led wing uses its human launch position |
| Primary target: opposing group 1, 2 or 3 | Assign every aircraft in that group as a destroy objective; unrelated groups are not primary objectives |
| Escort another same-side group | Protect every aircraft in the chosen group |
| Self-defense | Engage only independently identified immediate attackers |
| Weapons hold | No offensive fire; defense and countermeasures remain available |

Group references are resolved to actual launch identities. Friendly group 1
includes the player. Same-side intercept and opposing-side/self-group escort
assignments are invalid and cannot manufacture targets. An inactive referenced
group resolves to an empty assignment. Its aircraft must never be substituted
from a different group. Inactive groups retain their selected stamps for later
editing. Mission restart rebuilds the same group assignments with fresh memory.
The settings are session-local; campaign/save persistence remains separate.

Each group also has a separate `survival is required/optional` field. Required
marks every actual group member as a must-survive mission object. Friendly group
1 includes the player. An inactive group contributes no object IDs; its setting
is retained when its aircraft count changes. Both sides can carry their own
survival requirements, independently of their combat orders. The flag supplies
mission metadata and presentation; it does not silently replace an intercept,
escort or free-fire order. Mission result/scoring evaluation remains separate.

`--ai-mission free|cap|intercept|escort|self-defense|hold` selects the inherited
Quick Mission preset. Explicit group selections override it. Intercept targets the
first enemy aircraft; its other aircraft protect it. Escort assigns friendly
AI to protect the player, the enemy principal to intercept the player, and
enemy escorts to protect that principal. These are authored M1 presets using
current fighter aircraft, not additional bomber or transport behavior families.
Hostile escort relationships are explicit metadata derived from assignments;
they affect priority only after observation and a perceived threat report.

Every AI aircraft inherits its group's resolved duty on both sides. Shift-4
shows a single player-relative requirement for the selected aircraft:

- `Obj: Survive` for a friendly assigned to the player's protection, or marked
  must-survive in the mission requirements.
- `Obj: Destroy` for an enemy explicitly assigned as the player's destroy
  objective. Merely appearing in another group's orders does not qualify.
- No objective label for other contacts, including unrelated enemy groups and
  free-fire contacts without a specific requirement.

Live activity, skill and tactical goal remain separate from the objective row. Enemy-side survival
requirements cannot appear as a player-side survival objective. The player's
group setting supplies objectives and AI-wingman orders; it does not automate
human controls or restrict the player's trigger. Dummy aircraft retain straight
flight regardless of their stored objective.

## Acceptance criteria

Use synthetic fixtures and fixed 120 Hz headless simulation. Required cases:

- All four skills at exact range/cone boundaries, behind the cone, terrain
  occlusion and reacquisition. Skill changes no radar equipment capability.
- Memory just before/at expiry, paused time, source refresh, restart, destroyed
  IDs and Novice switching. A target turning unseen must not update its record.
- An Ace remembers several aircraft after minutes; a Novice kills its target,
  ignores a forgotten off-scope aircraft and reacquires only on fresh detection.
- Search visibly changes activity in Target view. Acquisition, attack, defense
  and rejoin transitions represent actual simulation state.
- Radar selection/tracking alone produces no missile alert. A is silent before
  actual pitbull; S warns on launch; I/E never create automatic warnings. Cover
  boresight activation/acquisition, reacquisition, destroyed missiles and launches
  by both player and AI, without duplicate events or changes to player audio.
- Visual missile range/cone/terrain boundaries, motor burnout, crossing and
  receding missiles, lost sightings and the 2-second grace. Hidden seeker class,
  target ID and launcher pose never leak into the controller's observation.
- Shared RWR/AI records agree on identity, source, freshness and warning onset.
  Verify ranged and bearing-only plots, one-second blinking, steady non-threats,
  target-specific visibility, out-of-scale markers, pause, panel closure, receiver
  failure and stale/removal behavior. Hidden I/E and pre-pitbull A never leak
  through the RWR feed. Confirm actual live display with a rendering smoke test.
- Jink legs, correct S/A notch-source bearings, unsafe-dive rejection, empty
  dispensers, class-appropriate bursts, cooldown and simultaneous threats. Measure actual
  support/seeker loss and decoy outcomes separately from maneuver requests.
  Warning receipt and memory alone must never authorize offensive firing.
- Escort chooses a known threat to its charge over a nearer unrelated hostile,
  recognizes explicitly assigned hostile escorts only when observed, handles
  warning-only bearings, respects hold orders and returns at its leash.
- Reliable distant-threat fixtures show skill-dependent reaction and drag without devices,
  then beam and break at the fitted thresholds. Verify
  threshold equality, too-late fallback, bearing-only warnings, uncertain/lost
  observations, low-energy turns and safe re-engagement. Compare decisions and
  feasible trajectories, not a requirement that Ace always survives or wins.
- Mixed roles, sides and skills, simultaneous threats, empty stores, fuel
  withdrawal and preserved missile support. Full seeker activation/pitbull
  integration is required before calling radar-missile encounters accepted.
- Twelve ported aircraft by four resolved skills, mixed-aircraft encounters,
  identical-seed restart/replay, render-rate independence and a measured
  30-aircraft encounter. Preserve all flight adapters and `--fixture-wings`.

## Exclusions and unresolved contracts

Surface AI, air-to-ground tactics, new aircraft behavior families, full campaign
objectives/import and general contact-sharing doctrine are outside M1 scope.

Before implementing the affected behavior, resolve and document:

- Environmental visibility where cloud/night occlusion is incomplete.
- Weapon-specific tuning beyond the shared active-seeker notch preset.
- Passive-guidance countermeasure susceptibility where no weapon rule exists.
- Defensive maneuver-time estimates validated against the selected flight model.

Validate authored tuning through the acceptance scenarios. Unresolved original
behavior remains unknown and is not a requirement for byte-level reconstruction.
