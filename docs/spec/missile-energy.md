# Energy-based powered guided weapons

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode, 2026-10-27. **Energy is selected by new worlds. Current fit residuals and validation are recorded in the [baseline](../baselines/missile-energy.md).**
John requested energy-limited guided weapons, lead guidance, greater drag low
down, flare response to target heat/aspect, and one uniform model. His Phase 2
decisions require real-world data where available and physical decoy pursuit.
This document owns the new behaviour. The existing [missile specification](missiles.md)
continues to describe the compatibility and previous Spec flight paths.

## Scope and provenance

All currently supported powered guided weapons use the same force kernel:
air-to-air, surface-to-air, ship strike, powered air-to-ground, laser and command.
Unpowered guided bombs and unguided rocket pods retain their current models.
Weapon identities, supported roles and availability do not change merely because
a real-world missile of that name has different guidance. Aircraft flight
adapters remain distinct and unchanged.

Energy flight and the countermeasure consequences below are opinionated,
requested by John on 2026-10-25. The coefficients, response times and derivation
rules are agent-fitted game approximations, not recovered retail behaviour.
Public mass/dimension/range facts must carry per-field source references in the
data documentation. A published maximum is conditional on launch geometry,
not a guaranteed range against every target. Unknown aerodynamic quantities
remain labeled fitted or estimated.

## Motion

At fixed 120 Hz, the body inherits the launcher's world velocity once, plus its
ejection vector. A drop does not erase downward velocity at ignition. Motor
phases contain duration, thrust and propellant mass, with dry mass as a hard
lower bound. Thrust ends when the phase ends. There is no velocity gain budget,
absolute speed clamp, coast-speed floor or reset to a powered speed.

The force calculation uses SI units, converting feet with exactly 0.3048 m/ft.
Dry standard air uses temperature and pressure layers at geopotential heights
0, 11, 20, 32, 47, 51 and 71 km, lapse rates -6.5, 0, 1, 2.8, 0, -2.8 and
-2 K/km. Sea level is 288.15 K and 101325 Pa, air gas constant 287.05287,
gamma 1.4 and gravity 9.80665 m/s². Convert geometric height using earth radius
6,356,766 m. Density is p/(RT), sound speed sqrt(1.4RT). The evaluated atmosphere
is bounded to geometric heights -1..85 km. Altitude means MSL, not terrain
clearance. [NASA standard atmosphere](https://ntrs.nasa.gov/api/citations/20050207438/downloads/20050207438.pdf?attachment=true).

With air-relative speed V and frontal reference area S=pi*d²/4, dynamic
pressure q=rho*V²/2. Zero-lift drag is q*S*Cd. Cd interpolates linearly at
Mach 0/0.8/1/1.2/2/3/5 through 0.30/0.30/0.65/0.55/0.36/0.30/0.26.
Cruise-class coefficients are 0.20/0.24/0.65/0.60/0.45/0.38/0.34. Endpoint
values hold outside the curve. Per-profile fitted scale is explicit data.
[Drag equation and reference-area convention](https://www1.grc.nasa.gov/beginners-guide-to-aeronautics/drag-equation/).

John's round-3 decision removes the supplemental geometry-based drag factor.
Drag uses the round-2 force law with the current guarded motor/drag fits. Altitude affects density and sound speed through the standard
atmosphere above; there is no additional altitude fade or span/diameter factor.
The supplementary [span reference data](../formats/missile-energy-data.md#span-reference-data)
do not affect simulation. Body diameter still supplies the original frontal
reference area. Thrust, fuel mass, phase duration, loft and controlled life
continue to affect each trajectory.

Lateral acceleration cannot exceed min(structural_G*g, q*S*CLmax/m). The initial
effective body-area CLmax is 12, induced-drag coefficient k is 0.03, and
induced drag is k*lift_force²/(q*S). Gravity compensation and loft demands use
this same limit and consume energy. These are fitted effective coefficients,
not wind-tunnel measurements. The [induced-drag equation](https://www1.grc.nasa.gov/beginners-guide-to-aeronautics/induced-drag-coefficient/)
motivates the law; it does not establish the selected k.

The lateral update rotates air-relative velocity without adding speed. Axial
thrust and drag update speed, then gravity changes world velocity; position
uses the average of old/new world velocities. Drag cannot reverse velocity in
one numerical step. This first-order force split with midpoint position is a
fitted integration choice. Coasting total mechanical energy decreases in still
air; a dive may increase kinetic energy at the expense of altitude.

John's 2026-10-27 policy applies to air-to-air and surface-launched air-target
stores. After ignition plus both motor durations have elapsed, air-relative
Mach strictly below 1.0 for 30 consecutive 120 Hz ticks defeats the store.
Mach at or above 1.0 resets that counter. The threshold is the single global
data knob `DEFEAT_MACH`; 0.9 and 1.1 are documented tuning alternatives,
not additional runtime modes. Powered flight and the ignition delay never
accumulate defeat ticks. There is no automatic defeat for a G deficit.
Available G, required unclipped G, their positive deficit and closing speed
are diagnostics only, evaluated only by an explicit diagnostic integration
request. Ordinary prediction steps do not calculate them.

Defeat is permanent: guidance and all direct, proximity, ground and collateral
damage stop. The body falls ballistically under drag and gravity until ground
contact or removal age. The policy is evaluated before each motion/contact
step: contact in the tick of defeat, or any later tick, cannot score, including
a contact at fraction zero. Contact in an earlier tick can score. Forecast
and live flight share this policy and its checkpointed state. This is distinct
from ordinary seeker loss and from control-time expiry.

John's Phase 2.6 decision treats an inert body as no longer threatening in the
same simulation tick. Defeated stores and stores that have passed their decoy
are removed from missile threat snapshots, AI defense inputs, RWR launch and
incoming warnings, voice inputs and HUD target/TTI cues. There is no lost-contact
grace for an inert store. The physical body remains visible and continues its
ballistic flight. Ordinary control expiry alone does not make a live fuze inert.
Other threats and ordinary radar-lock warnings remain independent.

Control budgets are explicit store classes, never fitted against range:
short IR 20 seconds, medium air 70, long air 180, cruise 600, short cruise 300,
boost-glide ground 600, fast ground
90, short ground 30, command 30, surface IR 25, surface radar 70 and long surface radar 120.
Medium/long air also explicitly contain the corresponding R-27 IR variants.
AIM9B retains its cited 20 seconds; every other allocation is an opinionated
game choice using John's approved class budgets. Exact allocations live in
`missile_motors::MOTORS`. At control expiry guidance is permanently lost,
but an otherwise undefeated body can still collide. Forecasts count controlled
intercepts only. Ground-target classes use path, support, terrain and control
completion, without a Mach or low-speed defeat rule. Cruise drag and powered subsonic flight remain intact. John approved shared
shorter cruise classes in Phase 2.8; the agent assigned AM39 and AGM84E to
ShortCruise 300 seconds before fitting. AS16 uses BoostGlide 600 seconds and
normal drag, retaining its boost-glide flight rather than a cruise classification.

A missile never acquires, guides toward, fuzes on, or damages its firing aircraft,
including blast damage. This permanent exclusion is John's 2026-10-26 decision;
bomb launch grace remains separate.

## Guidance

Proportional navigation uses only observed position and velocity. With relative
position r, velocity vr, LOS rotation omega=(r cross vr)/|r|², closing speed
Vc=-dot(vr,unit(r)) and flight direction f, command N*max(Vc,0)*(omega cross f),
projected perpendicular to the flight direction, plus gravity compensation.
N is 3, 3.5 or 4 by data class. Commands respond with a 0.08 s time constant and
are limited again by current available lift. [Homing guidance principles](https://secwww.jhuapl.edu/techdigest/content/techdigest/pdf/V29-N01/29-01-Palumbo_Principles_Rev2018.pdf).

Opening targets or heading error above 30 degrees use a bounded capture command
V*heading_error/0.5 s toward the observed LOS. There is a stable reversal axis
for opposite headings. Capture and PN share the same physical lift and drag.
Long-range radar profiles may request a loft waypoint; climb slope is limited
to 0.36, about 20 degrees. Profile data specify loft height and entry distance. Initial loft weight is
smoothstep(clamp(initial range / entry distance - 0.5, 0, 1)). Its terminal
factor is smoothstep(clamp((current range / entry distance - 0.4)/0.3, 0, 1)).
Their product blends the vertical aim toward the slope-limited loft waypoint,
observed target velocity toward a stationary waypoint, and PN toward capture.
Smoothstep(x) is x*x*(3-2*x). This agent-fitted path law avoids an abrupt
launch-range branch and fades to terminal guidance at 0.4 of entry distance. No A2A ground avoidance or
forced miss occurs. A sufficiently late turn/dive can cause physical ground
impact, overshoot or energy exhaustion; a short-range shot can still hit.

Seeker gimbal, instantaneous field of view, rate, acquisition dwell, support,
active-seeker activation and memory are separate from physical steering. SARH
requires its own launcher's support. Active seekers use observed launcher
updates during silent midcourse. Once the terminal seeker is enabled, lost
FOV or notch rejection freezes the last measured point; fresh launcher coordinates
cannot bypass that rejection. Ordinary seeker reacquisition remains allowed. Energy memory never substitutes
a constant-speed lead computed from a stationary or accelerating launch.
An in-flight TTI requires a current qualified observation, not just that frozen
memory point. Laser/command loss and HARM retention policies
remain as specified by the existing A-G guidance contracts.

## Countermeasure contract

John's Phase 2 decision supersedes the proposed inert one-second cleanup:
successful decoy transfers guidance to a physical flare/chaff object. Its
position and motion are deterministic simulation state and are checkpointed.
Flares eject and fall under gravity/drag until burnout; chaff slows toward the
air mass. Rendered-device capacity cannot delete a guiding missile's decoy.

The missile uses the same finite-energy guidance against that device. It cannot
fuze on a device. It can still fuze on a real aircraft while pursuing the decoy,
including a late flare beside the original target. After passing the decoy it
permanently disables its fuze and ends guidance (John, 2026-10-25). It then
coasts/falls until ground collision or lifetime,
without reacquiring any aircraft or device. Motor thrust is not replenished.

One roll per missile/releaser/device-class in a two-second episode. Monotonic
episode IDs prevent an older still-visible cloud from reopening a roll after
a newer episode has been considered. Extra
cartridges in that episode consume ammunition and appear physically but do not
compound the odds. IR heat factors are 0.1 engine-off, 0.5 idle, 1 military and 1.5 afterburner;
unknown engine state uses 1. Aspect factors are 1 tail, 0.7 beam and 0.4 front.
These are fitted relative signals, with no claimed cooling-time measurement.
IR odds use target heat, exhaust aspect, received flare-to-
target signal ratio and seeker generation. Imaging seekers strongly resist
flares. A beam turn reduces exhaust view; it is not an IR Doppler notch.
Radar uses target radial separation from terrain clutter for both SARH and
active seekers. Chaff and look-down notch improve seduction; active radar may
also lose a return in clutter without chaff. Clamp the full probability product,
preserving raw susceptibility values 150 and 200. The evaluator uses raw susceptibility times dispenser effectiveness,
then generation and signal factors. IR retains its whole-percent evaluator. IR factors are 1.0 for rear-aspect seekers, 0.6 for conventional
all-aspect and 0.08 for imaging, with ratio capped at 3 and probability at
95% (10% for imaging). Radar uses 0.35 for active seekers, 1.0 for records
whose susceptibility exceeds 100, otherwise 0.7, multiplied by
0.25 + 2.75*clutter_notch. John requested a global 30% chaff buff on
2026-10-27 and doubled it on 2026-10-28. Multiply the unfloored product by the
single global `CHAFF_EFFECTIVENESS_MULTIPLIER = 2.60`, then cap at 95% (lead
decision, 2026-10-27). Evaluate the result in basis points (0.01
percentage-point units), so a zero-notch AA12 at effectiveness 35 rolls 3.98%
(1.53% before any buff) and AIM120 3.19%, and a base below 1% is buffed too. IR odds, episode eligibility,
seed streams and notch thresholds do not change. The multiplier applies to
all Energy radar stores; explicit Spec/Compatibility modes retain their rules.
Recorded rolls carry their bound (100 or 10000) so thresholds remain readable.
The susceptibility-based early-SARH grouping is fitted and awaits per-profile review.

Each physical flare pair leaves 15 ft behind and 2 ft below the aircraft,
with aircraft velocity and 15 ft/s downward ejection. The two seeded lateral
throws are 20 to 30 ft in opposite directions with a 0.25 s exponential
response, matching the visible pair while retaining independent simulation
ownership. Only the strongest visible member supplies the episode signal.
Flare ignition delays seduction by 0.1 s. Flare terminal fall speed is
100 ft/s, chaff velocity decays toward wind plus 4 ft/s down with a 0.12 s
time constant. First-second devices may seduce; captured devices continue
independently of the scene registry. Flare/chaff burnout is 30/20 seconds.

Seeker slew is 15 degrees/s with a 4-degree instantaneous half-angle for
rear-only IR, 90/10 for imaging, 35/5 otherwise. Imported mechanical angles
remain. Unreviewed SAM aspect fields retain the existing game aspect
allowance; reviewed A2A rear-only overrides remain exact. This is a game-policy
continuation, not a claim that every real SAM has an all-aspect seeker. Look-down clutter scales from zero to full at five degrees depression.
Doppler separation within 60 ft/s produces notch strength; 24 ticks at strength
at least 0.75 loses the return, and 30 ticks below 0.5 restores eligibility.
These are fitted classes; the cited AIM9B 11 deg/s and 2-degree half-angle
override them. Seeker head movement is clamped to its mechanical gimbal.
Neither a clipped signal nor a decoy outside instantaneous FOV can roll.

## Usable aspect envelopes

John's 2026-10-27 ordnance-fix decision prioritizes usable envelopes over the
old two-condition range fits, with the smallest practical overshoot and retained
role/generation ordering and class floors. The agent fits AA8, AA12 and AEMP1
through explicit motor and drag data. No control time, defeat threshold, lift
coefficient, seeker setting or shared guidance law is adjusted to make a shot
pass. At shooter/target speeds 200/250 and 300/250 m/s, AA8 must have a
nonempty co-altitude hit band at 0/45/90/135/180-degree horizontal aspects from
500 through 45,000 ft. A low-speed or large vertical-separation shot can still
be unreachable. AA12 and AEMP1 must retain nonempty bands in the tested
5,000-to-30,000-ft climb sectors. These are game balance acceptance conditions,
not guaranteed hits everywhere inside a maximum; [measured limits and accepted
range exceptions](../baselines/missile-energy.md) remain explicit.

## Estimates and acceptance

Flight, range, TTI and AI launch decisions use the same model or a bounded
cache built from it. No obsolete straight-speed/coast-floor shortcut may
authorize a new-model intercept. Keep observed support/seeker gates separate
from physical reach. Player range and lock remain advisory at release.
The AI defensive schedule is specified in [AI defense](ai.md#missile-defense-flown-by-ai).

A range search scans outward from the decoded launch minimum to the reach
cap and keeps up to four contiguous sampled hit regions, nearest first. The
first sample, when it hits, is the first region's lower edge; every later
miss-to-hit and hit-to-miss transition is refined with ten bisections and
recorded on its hit side. The ordered scan uses a 1.25 range ratio and
includes shared loft-transition samples and the current shot range. Each
region is published as soon as it closes, so the first region appears as
early as before and farther regions follow. The 17-sample favorable-band pass
covers the first region only; a gap it exposes splits that region in two.
This is a fitted sampled approximation, not a mathematical guarantee against
arbitrarily narrow unsampled holes.

The player cue advises one region for the present range (lead decision,
2026-10-27), for A2A and A2G stores alike: the region containing the range;
otherwise the nearest region below it, which closing flies into; otherwise,
below every region, the first region. HUD minimum, maximum, percentage and
scale use that region, and the favorable band shows only with the first
region. IN RNG requires both membership in the advised region and the
exact-point hit answer, so a range inside a gap is never IN RNG while a hit in
a farther region is. The AI reads only its exact-point answer, for A2A
launches and guided A2G releases, so a range maximum never grants launch
permission through a gap. Empty or pending advice does not invent a maximum.
Readouts and replays carry the advised region's minimum as well as maximum.

A world-owned resumable queue admits at most 256 active flyouts. Waiting point
and envelope requests are retained and admitted in round-robin rotation;
player point questions enter first, with eight of the 256 slots reserved from
background admission and up to one quarter of the same work budget spent on
player jobs first. Unused priority work returns to the common queue; a large mission's AI stations, SAM rails and missiles in flight
must not starve each other out of a full table. A fixed 16,384 quarter-step work units per world tick pays for
integration (four units) and terrain samples (one unit). This permits at most
4096 integration steps, with terrain work reducing that count. It is not multiplied by the number of aircraft, and it does
not depend on wall-clock timing or render frequency. Ground-ceiling shortcuts
skip only provably clear terrain. Requests, cursor, partial searches and
published snapshots are checkpointed. Frame readers never run a flyout.

TTI snapshots refresh after 15 ticks, launch points after 30, and either
is resubmitted at once when its published answer no longer matches the
current question (a stale answer cannot be read, so waiting would leave the
cue and the AI launch gate dark through most of a short-range fight). Identical
physical inputs reuse the completed answer without another flyout; each
service belongs to one world's fixed terrain. Neighboring rails on the same
actor may reuse a recent compatible answer for the same target and body,
within the unchanged point tolerances. Different targets and stale geometry
never share an answer. This avoids duplicate rail flyouts delaying launch
cues in turning fights. Changed identity,
body, wind, arming or radius invalidates a result. Point compatibility allows
30 m or 2% relative-position error, 10 m/s velocity error, 30 m altitude and
one degree axis error, at most 600 ticks of in-flight age (3600 for stable
cruise profiles). In-flight snapshots
are aged along the mean observed velocities before comparing positions, so a
stable cruise TTI is not discarded solely because the missile moved forward.
A changed motor phase, more than 2% mass change, or a material trajectory
change rejects it. The low-Mach countdown is compared in six-tick (50 ms)
buckets, while lost/defeated states are compared exactly. A cached hit on or
after the earliest defeat time implied by the current countdown is withheld;
only a new flyout can establish that later acceleration makes it reachable. Launch snapshots advance launcher and target positions by the elapsed world
ticks since submission, using the same mean observed velocities. Their missile
age stays zero; launch answers expire after 600 ticks unless the inputs are
identical. AI launch gates retain those strict point tests. Player IN RNG and
A-G advice can also use a turn-qualified answer up to 120 ticks old. Angular
velocities are measured from consecutive observations 1..12 ticks apart,
limited to 20 degrees/s. Both positions follow constant-rate circular arcs;
launcher velocity, target velocity and launch axis rotate at their measured
rates. The same strict point tolerances apply to the residual after projection.
A rate change above 2 degrees/s rejects the answer. Identity/model checks still
apply, and no synchronous flyout runs in a frame reader. These are agent-fitted
advisory tolerances, not a relaxation of AI launch permission. Envelope searches
allow 150 m altitude, 1 km origin displacement and three degrees bearing drift,
10 m/s launcher speed and five degrees launcher axis and velocity direction,
and 25 m/s target velocity. A restarted search can show the previous envelope
only while its own observation is no more than 1,200 ticks old and geometry
still matches after both observed positions are carried forward along their
observed velocities for the elapsed ticks, so a straight run keeps its range
cue while each 1,000 m restart searches. Display-only angular limits are 15 degrees of axis/velocity
direction and 10 degrees of bearing, allowing a gentle turn while the tighter
5/3-degree replacement search finishes. Other limits stay the same. These
are agent-fitted display tolerances; they never authorize a shot. A cold search finishes
its first maximum before restarting for changed geometry, so continuous turns
cannot prevent the first publication. Target, body, arming, radius or wind
changes invalidate it immediately. A reversal removes the old head-on range
immediately. AI launch permission requires a qualified exact-point hit forecast. IN RNG also requires membership in the displayed contiguous range region. These tolerances are agent-fitted cache approximations, not
exact proofs about the current pose. Turns may leave a cue pending. AI cannot
fire from a pending/failed point answer, and ordinary sensor/support and
minimum-range launch gates remain. A surface seeker does not acquire a blind
zone near impact from that launch minimum or the site-relative decoded altitude
band. Those employment limits are applied at launch. Long range queries do not block the world.

Range search samples around the physical reference anchor and both sides of
the data-driven loft transition, brackets the last successful interval and
bisects ten times. A lofted shot can succeed beyond a shorter failing unlofted
shot, so the maximum does not imply that every shorter shot will hit. The point
forecast still governs IN RNG and AI launch permission. It publishes that maximum before
finishing the 17-sample favorable band. The fitted band uses ideal observation
quality, range fraction and flight-time/lifetime, threshold 70 of 95; it is not
a measured kill probability. A-G predictions retain the real target while the
same kernel constructs terrain/cruise waypoints. Bomb reach remains on the
bomb model. `State::missile_time_to_impact` adds timing for already perceived
threats without changing the AI defensive schedule or renaming its inputs.

The [calibration baseline](../baselines/missile-energy.md) gives all 25 A2A
records and SAM/A-G references. New worlds select Energy. The lower-level combat State constructor retains
Spec for historical tools and tapes. Second-review correction progress is in
[the validation baseline](../baselines/missile-energy.md#validation-status);
the invalidated Phase 3 study remains stopped. Estimate overhead must stay
about 2 ms per world tick with eight AI aircraft and sixteen missiles, reporting
cold/warm p50/p95. If the gate cannot be met, report options
instead of enabling a synchronous fallback.

Explicit old weapon modes remain. Unmarked combat tapes 2/3 select
Compatibility, 4..8 select Spec. New Energy tapes carry an explicit model
fingerprint marker and seed after the existing header. An old reader rejects that new
marker rather than silently using old physics. No version constant is changed
here; the lead owns the coordinated version bump. The fingerprint includes
physical data, motor/fuel, explicit class and control-time inputs, the global
defeat threshold/dwell and drag-curve knots,
and explicit semantic revisions for derivation, integration/atmosphere,
guidance, seeker, decoy, prediction, fuze/contact and inert-threat publication rules. Any behavior change
must update its rule revision; comments, citation wording in motor data and
calibration targets do not invalidate a tape. It does not hash source files.

Mission recordings play recorded poses. New weapon-launch events carry the
actual boost/sustain intervals and powered flags, so flame playback does not
reinterpret them through future imported motor timers. Old events without
that metadata keep the historical timer path. Live motor/flame/BURN cues read
the energy body's phases. No renderer implementation changed.
