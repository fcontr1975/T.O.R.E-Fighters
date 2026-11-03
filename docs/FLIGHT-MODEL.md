# Shared aircraft flight model

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Loaded aircraft weight uses the public/class masses of powered guided stores,
as requested by John on 2026-10-25. The per-store differences are listed in
[missile energy data](formats/missile-energy-data.md#loaded-aircraft-mass-changes).
This changes payload, takeoff weight and weight-dependent flight response;
it does not change which aircraft flight adapter is selected.

The [F/A-XX concept](spec/fa-xx.md) shares the F-22 flight response in all
three adapters. Its split flap rudder animation represents the existing yaw
authority; separate differential drag and fin removal effects are not simulated.

`tore-sim` is a renderer-independent, deterministic 120 Hz flight kernel. The
working **hybrid** path combines recovered aircraft data and native helper rules
with fitted continuous dynamics where the original engine contract is still
incomplete. It is a usable free-flight model, not a reproduction of the original
program's internal trajectory arithmetic or a real-aircraft engineering model.

The opt-in **airborne native research** path now connects the joined native
service to the live loop for both aircraft. Inside that research path only, the
clock, device and fuel boundaries are explicitly authored and the flight stops at
unsupported contact; ordinary free flight has working authored ground contact.
This is separate from legacy and hybrid.
[Acceptance and limits](baselines/native-live-flight.md).

Store loading follows the [even-paired-load contract](spec/stores-on-pylons.md#even-paired-loads).
Its rounded quantities feed the existing total payload mass and load-dependent
drag. There is no per-wing store drag torque or lateral store-CG model; paired
loads start with equal masses at mirrored positions.

## Next scheduled work

Sequencing lives in [the parity plan](parity-plan.md). The
[environment/systems plan](research/native-environment-systems-plan.md) is a
frozen archive as of 2026-09-15: use it for its recovered contact, asset,
lifecycle/event and environmental research, not for sequencing. AI movement has
an [input-only controller](spec/ai.md#input-only-aircraft-control). The first
[cache/preference checkpoint](baselines/native-land-foundation.md) and
[vertical geometry checkpoint](baselines/native-land-geometry.md) are diagnostic
only; the restricted research path still stops at unsupported contact.
Maneuver audio/rumble and final
[flight-response acceptance](research/flight-response-plan.md) follow that continuation.
[Provenance policy](behavior-provenance.md) records where each component came
from, spec-derived, native, fitted or opinionated, and keeps user-directed
changes distinct. Those labels describe origin; none of them is an acceptance
gate. Lifecycle and contact work inside the research path continues.

## Run and reproduce

Use the installed local media through the portable extraction entry point:

```sh
python3 tools/extract_assets.py --aircraft f18 --exclude-archive 'disc1/LHX/*' --out .local/aircraft/f18 --validate-flight
python3 tools/extract_assets.py --aircraft rafale --exclude-archive 'disc1/LHX/*' --out .local/aircraft/rafale --validate-flight
cargo run --locked -p tore-app -- --free-flight --researched-flight
```

`--validate-flight` extracts the selected aircraft, hashes source/output files,
and runs the same headless suite against each distinct extracted PT version.
It requires a full aircraft extraction, not `--include`, preview or
`--native-flight`. A failed extraction or scenario returns a nonzero exit code.
The source-specific LHX exclusion skips unrelated bundled archives; omit it for
media without that directory. See [EXTRACTION](EXTRACTION.md) for source paths.

The app defaults to the researched hybrid adapter, requested by John on
2026-09-16. `--researched-flight` remains an explicit alias; `--legacy-flight`
selects the previous compatibility model. Selection persists across new free
flights in that process. It also works
with `--headless-flight 7200 --maneuver loop`. Thirty-seven aircraft identities have flight configurations; the
[roster guide](aircraft-import.md) lists their identities. Select Rafale with
`--aircraft rafale` or Quick Mission. Each uses its source-referenced cockpit family and
its own reviewed exterior device mapping. No F18.SH animation addresses are applied to another shape. The additional
models own their configurations and use the [documented shared fit](spec/additional-aircraft.md).
Select them with `--aircraft f14|a4e|x31`; see [validation](baselines/aircraft-fa-expansion.md).
The seven [roster additions](spec/roster-aircraft.md) own separate configurations
with the same shared fit and their own source roll rates. See their
[validation](baselines/aircraft-roster-expansion.md).

For already-extracted files:

```sh
cargo run --locked -p tore-sim --example flight_suite -- .local/aircraft/f18/FA_2.LIB/F18.PT .local/aircraft/rafale/FA_2.LIB/RAFALE.PT
```

The suite exercises 13 scenarios per aircraft: level, loop, left/right banked
pull, stall, spin entry/recovery, approach landing, gear-up impact, braking roll,
controlled takeoff, constant wind, water impact and hard landing. It uses an
explicit flat runway, not a fabricated theater runway. Every step is replayed
from cloned initial state and checked for equality/finite motion. Additional
checks cover full loops, bank symmetry/AoA, wind advection, departure/recovery,
contact outcomes and fuel consumption. Synthetic unit tests cover pause/render
rate independence, wind, mass validation and ground behavior without retail data.

## Variety aircraft and powered lift

The [variety flight contract](spec/variety-flight.md) covers seventeen additional
conventional aircraft and six powered-lift aircraft. Every exact identity owns
its source mass, fuel, thrust, controls and envelopes. The four F-4 variants
share algorithms while preserving their own source configuration. Conventional
variety aircraft use the established hybrid solver. The fitted top speeds, the
level-speed fraction (drag reaches full thrust at 96 percent of the top speed,
which keeps the transports and airliners out of the overspeed shake) and the
ground-contact rules (the complete deployed gear, including the AV8's central
pieces) have one home in that contract.

The six powered-lift aircraft (AV8, YAK141, V22, AH64, MI24, CH47) fly their own
physics since the VTOL and helicopter overhaul, specified in
[powered-lift flight](spec/powered-lift-flight.md): a six-degree-of-freedom rigid
body whose angular rates are state, with rotors (momentum theory, a rotor-speed
state, translational lift, ground effect, autorotation, the vortex ring state,
torque, retreating blade stall), vectored nozzles with puffer jets, nacelles
with an always-on conversion corridor and an angle-of-attack wing. They share
the conventional solver's envelope, drag, systems and contact pieces, so a
fixed-wing aircraft is untouched (every golden fingerprint is unchanged). The
module map is in [architecture](ARCHITECTURE.md#powered-lift-flight-modules).

Powered controls operate only in the hybrid adapter; the legacy adapter and the
restricted native research path stay separate. All of the powered-lift state
(body rates, rotor speed, induced velocity, disk tilt, engine output, trim,
stability level, warnings and the corridor's held demand) is tick-owned and in the
exact flight state, so snapshots, checkpoints, prediction and seat handoff
continue the same flight. The unit tests are synthetic; the source-backed
probes read user-owned PT files at runtime. `variety_flight` checks conventional
flight or hover, vertical landing, departure, stick and yaw response and forward
conversion; `powered_probe` (helicopters and the V-22) and `jet_probe` (the
jets, against the conventional model on the same PT) print the tables recorded in
the [overhaul baseline](baselines/vtol-overhaul.md):

```sh
cargo run --locked -p tore-sim --example variety_flight -- PATH/FA_2.LIB/AV8.PT PATH/FA_2.LIB/AH64.PT
cargo run --release --locked -p tore-sim --example powered_probe -- PATH/FA_2.LIB/AH64.PT PATH/FA_2.LIB/V22.PT
cargo run --release --locked -p tore-sim --example jet_probe -- PATH/FA_2.LIB/AV8.PT --compare PATH/FA_2.LIB/F16C.PT
```

## Data and implementation boundaries

| Component | Runtime behavior and provenance |
| --- | --- |
| Aircraft selection | Exact reviewed PT identities for the existing roster and the [23-aircraft variety batch](spec/aircraft-variety.md). F18.PT remains the F/A-18D; RAFALE.PT remains the Rafale C. F18C, RAFALEE and RAFALEF are not aliases. |
| Envelopes and mass | Original G polygons, empty weight, internal fuel, military/AB thrust, consumption and drag/loading fields. Scalars resolve once when creating state; envelope intersection no longer allocates per update. |
| Attitude and momentum | Shared orthonormal basis, independent velocity and nose direction, full vertical/inverted flight. Float integration, aerodynamic alignment, trim AoA, atmosphere lapse, response and drag normalization are fitted. |
| Controls | Original roll-rate maximum in the hybrid path; G authority from aircraft envelopes/loading. Response filtering, pitch/yaw coupling and actuator travel are fitted. |
| Departure | Source warning/stall transitions and spin entry, with connected severity/control/lift attenuation. Initial stall classification uses a fitted below-clean-envelope gate. Hybrid spin torque, damping, surface response and threshold recovery are fitted; PT maximum yaw rate bounds angular velocity. The original two reviewed aircraft use spinExit −2; X-31 additionally disables spin entry. |
| Propulsion/fuel/devices | Per-aircraft military/AB thrust and fuel consumption; engine/fuel/throttle gates; gear, flaps, brake, hook and burner state. Lapse, exhaust ramp and three-second actuator travel remain fitted. |
| Ground contact | Native landing limits classify touchdown. Hybrid accepts only caller-declared landable ground with gear deployed and within limits; water and unsafe touchdowns crash, except that on a landable surface a touchdown over the landing limits but inside a hard-landing band (2 times sink and side speed, 1.5 times forward speed, roll and pitch) damages the gear and survives. One shared touchdown rule (`touchdown::assess`) also lets helicopters, the V-22 in helicopter mode and hovering AV-8/Yak-141 land on open ground or flat roofs within sink, ground speed and slope limits, used by contact, the AI ejection check and the AI go-around check ([rule](spec/touchdown.md)). Aircraft-owned CG clearance, load-scaled tire scrub/friction, smooth wheel unloading and crash severity policy are fitted ([takeoff rules](spec/takeoff-ground-contact.md)). |
| Wind | Explicit world wind in ft/s via `research::Surface`; airborne aerodynamic forces use air-relative velocity and position uses ground velocity. Hybrid aerodynamics also use full wind during rollout; the [MTOW crosswind/tailwind rule](spec/runway-wind.md) affects tire grip and difficulty cues, with static parked hold. Synthetic advection test holds airspeed unchanged and checks 400-foot drift over ten seconds at 40 ft/s. |
| Payload | `State::set_payload` validates finite, nonnegative mass against current fuel + empty weight and MTOW. Added mass affects loading/forces. The flight model reads it through `State::carried_lbs`, which the Ignore weapon weights cheat reduces to external fuel only ([cheats](spec/cheats.md)); Pull extra G raises the positive G limit to 9. Loadout, station release, external fuel burn and [player/AI jettison](spec/jettison.md) update the carried load used by the shared loading and drag model. Individual store-specific drag coefficients are not imported. |
| Time/replay | Fixed 120 Hz simulation; authored fractional bridge to native 256-unit time; seeded native RNG helper. Same-host deterministic replay is tested. Original scheduler/global RNG draw ordering and cross-architecture bit identity are not established. |

The legacy path remains available for comparison. Native integer matrix, force,
contact-query and scheduler research remains in `tore-formats::flight_model`;
those helpers are not falsely presented as a complete native integrator.

## What “working” covers

Both aircraft complete the same tested free-flight and ground scenarios using
their own extracted data. The suite is an engineering regression gate, not an
original-game trajectory oracle. Behavior a player would still find missing
includes carrier and arresting-gear dynamics and the remaining damage/equipment
state. Terrain and object contact is authored rather than recovered, and exact
integer update scheduling and RNG consumption order are original implementation
details rather than parity targets. Fitted behavior is localized in
`tore-sim`, separate from bounded readers and static native translations.

Current theaters supply height only through the app's existing callback; they do
not yet identify validated runway surfaces for the hybrid model. Consequently,
landing acceptance is demonstrated headlessly on explicit runways, and arbitrary
app terrain is not silently treated as a safe runway. That boundary is a
deliberate design choice (opinionated), not a hold waiting on recovered source
airfield/object collision mapping; ground, terrain and object contact was
reclassified opinionated on 2026-09-15.

See [acceptance evidence](baselines/shared-flight-model.md),
[native research](formats/native-flight.md), and [remaining work](research/progress.md).

## Independent aircraft models and future gauges

F/A-18D and Rafale C now own separate `models/f18.rs` and `models/rafale_c.rs`
implementations of `FlightModel`. `AircraftModel` dispatches by reviewed aircraft
identity, and each instance owns its full validated `Configuration` (including `Tuning`). Trim/AoA and thrust-lapse
laws are implemented separately; roll/pitch response, alignment, rudder and tire
coefficients are obtained from the selected model. Original PT envelopes, thrust,
mass, fuel and departure data remain aircraft-specific. Initial fitted numbers
preserve the previous tested baseline; identical initial coefficients do not
establish that the real aircraft behave alike. Changing one model's tuning does
not mutate the other. Common numerical integration, vector math and recovered
helper algorithms remain reusable components.

For code mods, copy a model module, implement `FlightModel`, add a registry/enum
entry, and review the new aircraft's PT identity/schema and extraction roots.
Use `AircraftModel::set_configuration` for complete per-instance edits or
`set_tuning` for fitted coefficients alone. Both validate before replacement. There is no arbitrary-code
plugin loader or external tuning-file format yet. Adding a new native layout
still requires bounded-reader review and its own acceptance baseline.

`telemetry::AirData::sample` provides a gauge-independent snapshot from flight
state and explicit terrain, wind, temperature and static pressure:

- TAS (3D air-relative speed), horizontal ground speed, EAS, Mach and dynamic pressure.
- Geometric MSL altitude, terrain-relative AGL, vertical speed, true heading,
  attitude, AoA, sideslip and load factor, with units in field names.
- ASL is explicitly an alias for the same mean-sea-level datum here, not another
  independently integrated height. AGL can be negative for below-terrain diagnostics.
- IAS, CAS, pressure altitude and indicated/barometric altitude are `Option`
  channels currently unavailable. They are not filled with TAS or geometric MSL.
  Pitot/static calibration, pressure setting, instrument error/lag and failures
  belong in future sensor components. AoA/sideslip are unavailable at zero airspeed.

Steam gauges can consume this snapshot without reading aircraft-specific structs.
The HUD now consumes the shared sample for TAS, AGL and vertical speed using
explicit wind, terrain and standard atmosphere; other panel channels retain
existing integration. This is not a fully modeled pitot/static system. Gauge animation/filtering must not modify authoritative flight state.

The optional engineering atmosphere helper follows the published
[NASA Glenn three-zone atmosphere approximation](https://www1.grc.nasa.gov/beginners-guide-to-aeronautics/earth-atmosphere-equation-metric/).
Mach uses temperature-dependent sound speed, following
[NASA's speed-of-sound relation](https://www.grc.nasa.gov/WWW/k-12/VirtualAero/BottleRocket/airplane/sound.html).
This telemetry atmosphere is separate from aircraft-specific fitted engine lapse;
it does not silently replace retail weather or claim precision ISA altimetry.

## Complete typed model ownership, 2026-09-14

`models/f18.rs` and `models/rafale_c.rs` each construct and own a validated,
immutable `Arc<Configuration>` from their own reviewed PT. The shared type and
import mapping live in `models/config.rs`; sharing a schema does not share aircraft
values. No retail values are embedded in code. Missing required fields fail import
instead of silently becoming zero. There is no longer a string-indexed parameter
cache or a second copy of aircraft limits in `Research`.

| Configuration group | Contents / runtime use |
| --- | --- |
| `mass` | Empty weight, internal fuel capacity, maximum takeoff weight; initial fuel, force acceleration, loading and payload validation |
| `propulsion` | Military/afterburner thrust and fuel rates; force and consumption calculations |
| `aerodynamics` | Altitude/speed/G polygons, loaded drag/elevator factors, G drag, source roll limit |
| `native` | Departure/spin parameters, landing limits, device drag, velocity limits and extended-warning flag |
| `equipment` | Fitted gear/flap/brake/hook travel, exhaust and control response, throttle response/AB threshold, ground clearance |
| `tuning` | Aircraft-owned fitted trim, thrust lapse, roll/pitch response, alignment, rudder and tire/braking coefficients |

Both legacy and research modes consume this configuration. Research mode adds
recovered departure/contact behavior. Hornet/Rafale legacy mode retains its
configured 1.8 rad/s fitted roll cap and legacy contact/drag treatment. The three
new ports use their PT roll response and low-speed auxiliary rotation in both
host adapters, as specified in [additional aircraft](spec/additional-aircraft.md).
Auxiliary rotation is separate from aerodynamic control and remains available
below stall speed when powered. It does not add lift or redirect thrust.

Configure before creating a flight (this Rust API assumes a parsed `Aircraft`):

```rust
use tore_sim::{flight::State, models::{AircraftModel, FlightModel}};

let mut model = AircraftModel::for_aircraft(&aircraft)?;
let mut config = model.configuration().clone();
config.mass.max_takeoff_lbs += 100.; // Example code mod, not a source-data correction.
model.set_configuration(config)?;
let mut flight = State::from_model(model, [0., 5000., 0.]);
flight.enable_research(1)?; // Optional; seed must satisfy NativeRng's contract.
flight.step(&keys, |_, _| 0.);
```

`State::new(&aircraft, position)` is the fallible import-and-start convenience API.
The app validates its Hornet model at asset load and clones it for each free flight.
Updates no longer accept another `Aircraft`; research activation also uses the
already selected model. Read it through `flight.model().configuration()`.
Configurations are readonly once attached to a flight. Clone/edit/validate a model
before starting a new flight, avoiding inconsistent mid-flight fuel/load limits.
Cloning models and presentation states shares immutable data without copying
polygons; replacement gives only the edited model a new allocation.

Live fuel, payload, actuator positions, velocity, attitude, stall/spin progression,
clock and RNG belong to `State`/`Research`, not configuration. Environment inputs
remain explicit. Telemetry and future gauges keep their independent typed API.

“Complete” here means the configuration used by the current adapter plus the
already recovered native profile. This refactor adds no new native decoding.
Native velocity limits and rudder/bay/wheel drag are retained for diagnostic
helpers; the continuous adapter still uses fitted force/contact coupling. Other
unresolved PT fields remain in raw imported `Aircraft` data for research, including
unimplemented equipment, damage and loadout behavior. It is not whole-game or
whole-tick parity. External mod-file serialization/loading remains future work.

## Aircraft import and expansion

The [aircraft import and acceptance guide](aircraft-import.md) joins extraction,
existing flight/presentation/systems coverage and all per-aircraft acceptance
gates. F-14D, A-4E and X-31 now have initial FA-only ports; see
[acceptance](baselines/aircraft-fa-expansion.md).

## Autopilot

The [autopilot specification](spec/autopilot.md) defines heading/altitude hold
and waypoint guidance through ordinary stick commands at 120 Hz. Steering gains
are fitted, throttle remains manual, and mode changes are recorded in pilot
input tapes. The optional navigation target accepts a waypoint number and world X/Z in feet; routes
and target-selection recording remain future work. Flight adapters stay distinct.

## Flight response contracts, 2026-09-15

The adapter response pass has [component/regression evidence](baselines/flight-response.md);
native steps 2–3 remain open.
`State::g` / `AirData::load_factor_g` report aerodynamic specific force projected
on body-up, excluding gravity/contact. Internal filtered lift remains separate.
`State::maneuver` is the last authoritative fixed-tick snapshot: commanded G,
attenuated lift G, achieved G, applied body roll/pitch/yaw rates in rad/s, rudder
command/filtered deflection/effective control, optional departure mode and severity.
It is not interpolated by presentation; consumers must not combine it with a
render-interpolated AirData sample as if both represented the same tick.
The existing `roll_rate`/`pitch_rate` fields are control-response state; use the
snapshot for actual rotation, including alignment, ground rotation and blended spin motion.

Rudder yaw now consumes the fitted filtered deflection, preserving a smooth
release. Each aircraft's own tuning includes `sideslip_drag=0.5`: drag/weight is
that coefficient times squared lateral airspeed fraction times low-speed authority.
This symmetric continuous loss is authored, not the native display-slip drag law.
[Lateral flight](spec/lateral-flight.md) adds fitted slip side force and roll coupling,
and the requested 0 to 50 mph nosewheel schedule. The sim exposes its available
steering fraction for the NSW HUD readout, independently of pedal position.
Roll retains its single response filter and existing source/hybrid versus fitted/
legacy cap. No new native rudder-to-roll law is asserted.

Hybrid spin entry retains source warning eligibility, direction selection and
the X-31 disable flag, with continuous fitted torque onset replacing the old
rudder/pitch switches. Speed deficit and back-stick smoothly increase driving
torque; opposite-rudder braking remains unchanged. Spin
motion and recovery use [input-driven angular dynamics](spec/spin-transitions.md):
continuous rudder torque, rotation/airflow-dependent control response, direct
proportional elevator pitch and aerodynamic damping. There is no timed spin
buildup or recovery ramp. Wrong rudder can increase rotation; early opposite
rudder can arrest it even below clean stall speed. In that case the spin clears
but the aircraft remains stalled. Normal flight requires sufficient speed,
airflow inside the 25-degree cone and residual yaw within normal rudder authority.
Small remaining angular velocity is retained and damped after clearance.

Analog input remains continuous through torque and elevator response, including
small deflections. Source direction selection still uses its integer input domain. Source recovery predicates remain unchanged in the restricted
research adapter. Hybrid normal stall classification remains a fitted
clean-envelope speed gate; severity and source warning timing are unchanged.

Native pitch/roll fall, tumble, full current-G/difficulty/device classification,
original spin movement/display composition and scheduling remain open. Legacy
still has fitted low-speed lift loss with no native warning/spin state machine;
its maneuver departure channel is `None`. This does not change adapter selection.

Run `cargo run --locked -p tore-sim --example response_probe -- PATH/F18.PT
PATH/RAFALE.PT` for both-adapter response/loop checks; set `TORE_RESPONSE_TRACE`
to an ignored local directory for per-tick evidence. The existing extraction
`--validate-flight` suite continues to cover both identities in hybrid mode.

### Provenance of the response pass

**Native:** source timer, attenuation and spin predicate arithmetic, with the
specific connections listed above. **Fitted:** response time constants, trim/
alignment, clean-envelope stall gate, severity reference speed, continuous spin
motion and `sideslip_drag=0.5`. These fitted choices were authored by the
implementation; they are not source facts or user-directed flight-law changes.
**Diagnostic instrumentation:** achieved-G and applied-body-rate snapshots.
**User-directed addition:** planned sustained rumble; native audio dispatch and
its future haptic mapping remain separate work.

Native tumble/fall/spin now has a joined diagnostic stage in
`tore-formats::flight_model::departure_stage`, with explicit envelope roles and
source movement composition. Both reviewed PTs pass its imported-table probes.
Neither live adapter calls it. The native
movement-state and whole-tick connection research steps remain open; see the
[source continuation](formats/native-flight.md#native-tumble-continuation-2026-09-15).

The [departure-stage evidence](baselines/native-departure-stage.md) and
[primary-control/movement evidence](baselines/native-movement-control.md) record
component checkpoints. The current `native_flight` diagnostic now feeds native
loaded controls, departure, force/velocity and movement/contact results back
into the next service for both PTs. It includes rudder/steering, auxiliary rates,
passive fall, sampled loading/damage/device effects and returned native events.
[Current acceptance and commands](baselines/native-flight-diagnostic.md).

The next continuation now provides restricted airborne live activation with
`--native-flight-tables DIR`, where DIR contains extracted sine/atan tables.
The translated control/departure/force/movement service is authoritative; the
existing host clock, input, device and fuel producers remain explicit adaptations.
Terrain contact stops this restricted research path and environmental turbulence
is disabled inside it; ordinary free flight has working authored ground contact.
Native query producers, engine/device/fuel/damage lifecycles, setup refresh cadence
and event execution remain open research items; scheduler and RNG ordering are
original implementation details, not parity targets. Legacy and hybrid retain
their existing behavior. [Live commands and validation](baselines/native-live-flight.md).
Retail comparison remains an unavailable evidence item, not an implementation
prerequisite. Audio/rumble follows the scheduled native work.

A-4 hybrid roll now uses the [90% reported Skyhawk peak target](spec/additional-aircraft.md#a-4-roll-tuning).
The evidence, variant limits, fitted acceleration and low-speed scaling are
specified there. Legacy A-4 retains the FA control values.

AI aircraft command stick, throttle and afterburner inputs into their own flight model.
Formation departures and rejoins use the [physical rejoin procedure](spec/ai.md#physical-departure-and-rejoin);
its clearance and braking predictions never override achieved motion.
That model alone advances attitude, velocity, position, fuel and telemetry;
there is no post-step AI movement override. Imported external stores contribute
payload mass and releases reduce it. The controller and its fitted limits are
specified in [input-only AI control](spec/ai.md#input-only-aircraft-control).
[Gun tracking](spec/ai-gun-employment.md#physical-bursts-and-aiming) adds fitted
lead-rate inputs through this same adapter, retaining physical limits and defense
priority. It cannot rotate the barrel or steer a projectile after release.
AI [damage responses](spec/systems-damage.md#ai-pilot-response-to-faults) constrain
requested power and maneuvering through these controls. Actual faults, jammed
actuators and engine progression remain in the shared systems model. Damaged
pilots can request recovery but cannot repair a component or override physics.
Ground-started AI aircraft use the researched model for runway contact.
An airborne AI actor switches to that model when its landing sequence begins.
AI helicopters (AH-64, Mi-24, CH-47) and the AI V-22 fly the researched model, and
so their powered-lift physics, from the mission start under every AI flight model,
single player's `standard` included: an intended default change of 2026-10-12
(opinionated: John's request for AI helicopter and VTOL hover, takeoff and landing;
the mechanism the lead's decision, slice H1), because on the legacy model a
rotorcraft flies as an aeroplane and cannot hover. They are flown by their own
[rotorcraft control](spec/ai.md#rotorcraft-and-jet-lift-control), which commands the
collective, cyclic, pedals and V-22 nacelles. The vectoring jets keep the AI flight
model's choice. The player's adapter flags (`--legacy-flight` and the others) do
not apply to AI aircraft and are unchanged.
The control mapping accounts for current flap lift and low-speed G authority.
[Takeoff, landing and return to base](spec/ai-airfield.md) define the sequence.
The three player adapters stay distinct. Exact input-replay validation is
recorded in the [AI baseline](baselines/ai-research.md).

## Rotorcraft fuel

Fitted (agent decision, John's requests of 2026-10-13). A helicopter or the V-22 does
not burn the PT record's fuel flow times the throttle, as every other aircraft does.
The records' flows (1 lb/s for the AH-64, V-22 and CH-47, 3 lb/s for the Mi-24) are
whole-pound placeholders that would empty the Mi-24 in 18 minutes
([evidence](formats/aircraft.md#rotorcraft-fuel-records)). The burn follows the shaft
power the drive delivers, the turboshaft line:

```text
flow = full_flow x (0.22 + 0.78 x power / rated_power)        lb/s
full_flow = rated shaft hp x 0.6 lb/(hp h) / 3600
```

- `power` is the drive's own lagged engine output, so a hover burns what the rotor
  draws to hover, level flight at 60 to 100 knots burns less (translational lift), a
  climb or a fast dash more, and an engine on the ground with the collective down only
  the idle share. Engines off burn nothing. The rated power is the one each aircraft's
  rotor model already carries (`models/variety/lift.rs`), so the burn scales with the
  aircraft.
- **0.6 lb/(hp h)** is 0.55 for turboshafts of this class (T700, TV3-117, T55 run 0.45
  to 0.60) over the 8 percent of the engines' power that transmission and installation
  take before the rotors. **0.22** is the idle share (a turboshaft burns about a fifth to
  a quarter of its full flow at flight idle). Both fitted.
- **Results** (`powered_probe`, hover and level trims at 1,000 ft at the record weights; the
  full table is in the [baseline](baselines/rotorcraft-fuel.md)): hover 0.47 lb/s for the
  AH-64 (71 minutes), 0.43 for the Mi-24 (128 minutes, about 200 at 100 knots). The
  CH-47, with the CH-47F's weights and 7,340 lb of fuel since slice HF3, burns 0.67 lb/s
  at a hover (182 minutes) and 0.83 at its 156 knot cruise home (148 minutes; the real
  CH-47F's 400 nautical miles at 157 knots is about 2.5 hours). The real aircraft burn about 0.3 lb/s (AH-64D cruise, 1,000 to 1,200 lb/h)
  and 0.5 to 0.6 (Mi-24). The V-22, with the MV-22B's weights and 11,700 lb of fuel since
  slice HF3 ([record correction](formats/aircraft.md#rotorcraft-fuel-records)), burns
  1.37 lb/s at a hover (142 minutes), 1.16 at 220 knots in airplane mode at 1,000 ft
  (168 minutes) and 1.04 at 240 knots at 15,000 ft (187 minutes); the real MV-22B's
  879 nautical mile range at about 240 knots is about three and a half hours.
- **The player's gauge** falls at these rates in a helicopter or the V-22: the internal
  fuel row of the systems page and the weight (a lighter aircraft hovers on less power).
  Jets, transports and the vectoring jets keep the record's flow and the afterburner
  rule.
- **Planning** (the AI's bingo line and the crew voice's low-fuel call) uses one steady
  figure, so the line does not swing with every climb and descent. Since slice HF3
  (opinionated, John 2026-10-13: "helicopter bingo planned at cruise flow") it is the
  flow of level flight at the AI's cruise home, 80 percent of the envelope's top speed
  (the V-22 on its downstops), at sea level and empty plus full internal fuel, trimmed
  once when the model is built and scaled in proportion to the weight now (fitted:
  profile and parasite power, which the weight barely moves, dominate at cruise). Slice
  HF2 used the hover flow. At those speeds the cruise is fast, so the cruise flow is
  about the hover flow or a little more (AH-64 0.52 against 0.47 lb/s, Mi-24 0.44 against
  0.43, CH-47 0.83 against 0.65, V-22 1.44 against 1.36): the plan matches the flight
  home the AI actually flies. A model with no level trim there plans at the hover flow.
- The Unlimited fuel cheat and the fuel starvation rules are unchanged.

## Envelope limits and loading

The aircraft's PT file holds a speed and altitude polygon for each whole G. The
1 G polygon is the aircraft's absolute limit (manual p. 90): its left edge is the
stall speed, its top is the ceiling and its right edge is the top speed. The
hybrid adapter turns them into the highest and lowest G it will deliver:

- The rows that hold the current speed and altitude set the range, from -1 to the
  largest of their G.
- **Loading divisor (fitted).** Fuel and stores make the aircraft heavier:
  `loading = (fuel + carried weight) / empty weight` and the divisor is
  `1 + loading * loadedElevator / 100`, with `loadedElevator` from the PT. Both
  limits are divided by it, so a full aircraft pulls fewer G than an empty one.
- **1 G floor (fitted, 2026-09-29).** Inside a row's polygon the upper limit never
  falls below 1 G. Without the floor the outermost band, where only the 1 G row
  holds, gave a loaded aircraft less than 1 G and it sank at full power near its
  top speed or its ceiling, and an AI wingman flew into the ground. The AI's own
  G limit (`ai/steering_adapter.rs`) applies the same floor and the same ceiling rule, so
  it never asks for G the aircraft cannot give. Both rules are hybrid adapter rules; the
  legacy compatibility model keeps the old limits.
- **Fast-side hold (fitted, agent decision 2026-10-08).** On the fast side of
  the envelope the higher rows end one after another before the 1 G row's top
  speed, so the available G steps down with speed. Past the fast edge of the
  last row above 1 G (in practice the 2 G row) only the 1 G row used to hold,
  and a full pull gave 1 G: no pull at all. Transports and airliners have few
  rows and reach that band in level flight at full power, so John reported
  (2026-10-08) heavies going from pulling 2 G to nothing well below overspeed,
  and an AI E-3 flying into the ground. The 1 G row's fast edge is the
  aircraft's structural speed limit, which the overspeed rule below governs,
  not a manoeuvring limit. So from that last fast edge to the top speed, and on
  past it, the aircraft keeps the G of the row whose fast edge reaches furthest
  (the higher G on a tie), before the loading divisor, and the 1 G floor
  applies there too. Near the ceiling, where no row above 1 G reaches the
  altitude, the hold past the 1 G edge is 1 G. Inside any row above 1 G nothing changes: a fighter's
  staircase is untouched, and only its last few percent below top speed (and
  overspeed) keep 2 G instead of 1 G. The powered-lift solver and the
  autopilot's stick scaling apply the same hold. The AI adapter
  (`ai/steering_adapter.rs`) still computes 1 G there; with the flight model
  delivering the hold it asks for full stick and gets the held G, which is
  enough to recover (see the regression tests below). Regression:
  `flight::tests::a_heavy_keeps_its_pull_*`, `the_fast_side_hold_leaves_a_fighter_staircase_alone`,
  `an_ai_heavy_recovers_from_a_fast_descent_*` and the `flight-variety-gcurve-*`
  battery scenarios.
- **Above the ceiling (fitted, 2026-09-29).** Above the top of the 1 G polygon the
  air is too thin to lift the weight (manual p. 90), so the upper limit is
  multiplied by the ratio of the air density there to the density at the ceiling
  (standard atmosphere, held at its 100,000 ft value above that, so the thinning stays finite). A zoom climb can carry an aircraft a little past its
  ceiling but it cannot stay there.
- Outside every row at that speed the limit is 1 G divided by the loading
  divisor, except past the right edge, where the fast-side hold above keeps the
  last higher row's pull so a pilot can still pull out of an overspeed dive. Overspeed is `opinionated` (requested by John,
  2026-10-01; timing and odds are requested values, [overspeed](spec/overspeed.md)). The manual says that below about
  36,000 ft, beyond the structural limit, "air resistance begins to weaken the airframe and the
  wings will eventually tear off" and gives no numbers. Here the view shakes from 95% of the top speed
  at the aircraft's altitude, rising to a clear maximum at 100%. Continuous time
  above the limit is safe through 5 seconds, followed by 25 percent loss rolls
  at 6, 7, 8 and 9 seconds and guaranteed destruction at 10 seconds. Returning
  to the limit resets the timer. Loss uses the ordinary destroyed path with cause `overspeed`.
  Above the ceiling the envelope has no speed range and the rule does not apply.
  The AH-64 and Mi-24 on the hybrid adapter are judged against their never-exceed
  speeds (197 and 190 kt) instead, because their envelope row is a performance
  figure ([overspeed](spec/overspeed.md)).
- The autopilot makes no promise outside the envelope ([autopilot](spec/autopilot.md)).

### Weight-scaled stall speed

`opinionated`, requested by John on 2026-09-29; the reference weights and the
numbers are agent decisions ([rules](spec/takeoff-ground-contact.md#weight-scaled-stall-speed)).
The polygon's slow edges for the 0 G to 2 G rows are the aircraft's speeds at its
reference weight (its empty weight times a fitted fraction, one per aircraft) and grow
with the square root of its weight over that (fuel, stores, ordnance), fading out at
the 4 G row so that the G limit above about twice the stall speed is the imported one (the
loaded-elevator divisor already carries the weight there), in the hybrid adapter only.
The fast edge, the overspeed rule and the ceiling rules do not change; full flaps
still take 25 percent off the 1 G stall speed; the loaded-elevator divisor stays.
`--retail-stall-speeds` (developer switch) restores the imported speeds at every
weight. The rule is the same for all fourteen aircraft, but the reference fraction is
`fitted` per aircraft (2026-09-30, to John's unsourced liftoff and approach ranges; a lower
fraction means faster liftoff and approach): F/A-18D 1.33, Rafale C 1.38, F-14D 0.85,
A-4E 1.20, X-31 1.00, MiG-29 1.20, Su-27 0.86, MiG-21 0.73, Su-25 0.55, MiG-23 0.78,
Su-35 0.95 and the F-22 family 0.42
(`STALL_REFERENCE_FRACTIONS` in `tore-sim/src/models/mod.rs`, read through
`AircraftModel::stall_reference_fraction` for the player and the AI alike).

### Limits of the world and the ground

An `opinionated` rule requested by John on 2026-09-29 with agent-decided numbers: an
aircraft is lost 105 nautical miles beyond the nearest point of the map, with a
turn-back warning from 100 ([world edge](spec/world-edge.md)). Like overspeed it ends
in the ordinary structural-failure or hit-point path, and a loss records its cause.

On the ground the gear cannot be raised while the main wheels carry weight, as in
retail: the gear key does nothing and T.O.R.E shows `Ground sensor preventing gear
retraction` on each refused press. Once the wheels leave the ground the key works as
before ([gear on the ground](spec/gear-on-the-ground.md)).

## Telemetry record

`State::trace()` returns a `FlightTrace` (`crates/tore-sim/src/flight/trace.rs`): what
the last fixed step used and applied, with the inputs that caused each effect.
It feeds the telemetry panel and replay logs, an opinionated addition requested
by John on 2026-09-26; the record's shape is an agent choice. It is plain `Copy`
data with no text. The step fills it with copies of values it computes anyway;
only the drag breakdown is recomputed from the same inputs for display. Nothing
in the simulation reads it, and it takes no part in `State` equality, so state
comparisons and input-replay checks never see it. The golden fingerprints are
identical with the trace read after every tick.

The record is reset when `step_surface` begins and stamped with the tick when it
ends. Turbulence, missile-blast kicks and No crashes building rebounds that the
host applies between steps join the record of the step before them. Units are
feet, ft/s, lb (mass), lbf (force), G, radians and rad/s; runway wind is in
knots. Stick arrays are [pitch, roll, yaw]; rate and control-scale arrays are
[roll, pitch, yaw]. Mission recordings read the record after the whole tick,
turbulence included, and keep it as the
[telemetry tree](REPLAYS.md#display-trees) and as `flight.effect` events when an
effect starts or stops ([flight-model effects](REPLAYS.md#flight-model-effects)).

| Field | What it holds |
| --- | --- |
| `tick`, `path` | Tick after the step, and which code moved the aircraft: legacy, hybrid, native, the wreck component, or a stop (native fault, fatal system failure) |
| `autopilot` | Engaged mode, the pilot's stick, the stick the flight model received, and why it let go |
| `controls` | Stick requested and delivered by the control system, hydraulic pressure, and control-run damage (per-axis authority and bias, linkage, instability) |
| `throttle_lock` | Jammed throttle position and the lever input it ignored |
| `adapter.air` | Altitude, airspeed used, surface wind removed, ground speed, wheels on the ground |
| `adapter.regional` | Left wing, right wing and tail damage, the penalties they produce, and the stick after them |
| `adapter.runway_wind`, `parked_*` | Crosswind, tailwind and headwind against the weight-class limits, the fade-in with ground speed, the tire-grip fraction; parked attitude and position holds |
| `adapter.devices` | Gear, flaps, airbrake and hook: switch, position, and whether no hydraulics or a jam blocked them |
| `adapter.power` | Engine, fuel starvation, afterburner and why it stayed dark, throttle, fuel flow, Unlimited fuel, rated thrust, model thrust lapse, power available, thrust |
| `adapter.envelope` | Clean and effective stall speed (weight-scaled, with the scale itself), flaps, top speed, missing 1 G envelope, authority, the envelope rows holding the speed and their G, loading and its divisor, Pull extra G, the low-speed ceiling ramp, final G limits, stick and stick G |
| `adapter.lift` | Transonic drag percentage, flap lift, wing damage, commanded G, spin lift factor, lift target and lagged lift |
| `adapter.departure` | Hybrid only: mode and spin direction before and after, spin drive, the spin direction rule (direction, random draw, roll rate and bank as the rule read them, entered), how a spin ended, spin rate and maximum, cleared on the ground |
| `adapter.scaling` | Stall severity and its control and lift scaling, spin blend and control effectiveness, final control scale |
| `adapter.rotation` | Roll law (fitted lag or the aircraft's control profile, with limits and authority), pitch targets, model trim, low-speed trim blend, trim used, alignment rate, and yaw from turn, rudder, auxiliary control, ground steering and spin |
| `adapter.forces` | Weight, carried and payload mass, Ignore weapon weights, drag (total, before damage, before the hybrid cap, the cap, and parts: airframe, fuel and stores, G pull, gear, flaps, airbrake, slip, damage percent), achieved G, support, wheel load, speed before the 6,000 ft/s cap |
| `contact` | Hybrid ground contact (airborne, surface dropped, lift-off, unsafe touchdown with each reason and the values checked against the landing limits, or rolling with tire grip, scrub, brake or rolling deceleration, brake hold and any graded touchdown); legacy floor crash or bounce |
| `jolt`, `blast`, `turbulence`, `rebound` | Blast rates rotating the aircraft this step; a blast kick, turbulence or a building rebound applied after it |

`FlightTrace::effects()` lists the effects that changed something this step,
each with its factor or limit and its causes: autopilot steering and release,
hydraulic loss, control response, throttle jam, regional damage, runway wind,
parking holds, held devices, fuel starvation, engine off or reduced power,
afterburner blocked, Unlimited fuel, Ignore weapon weights, a missing 1 G
envelope, flap stall speed, low-speed authority, speed outside every envelope
row, loaded G limits, Pull extra G, the low-speed ceiling, flap lift, wing
damage, spin lift loss, stall scaling, spin control loss, the spin direction
rule, spin endings, departures cleared on the ground, low-speed trim, ground
steering, gear drag off on the wheels, scaled device drag, the drag cap, the
speed cap, lift-off, surface drops, touchdowns and their grade, unsafe
touchdowns, legacy floor contact, tire forces, blast kicks, jolts, turbulence and
building rebounds. Values every step has (G limits, thrust, drag parts) stay in
the sections. Compare `std::mem::discriminant` values to notice an effect start
or stop.

What the record cannot explain:

- **Model response curves.** `AircraftModel::response` supplies the trim angle
  of attack and the thrust lapse. The record shows their values, not why.
- **Recovered helpers.** The transonic drag percentage, stall severity and its
  control and lift attenuation, and the landing-limit class are shown as values
  with their inputs, not their internal arithmetic.
- **Fitted internals.** Spin torque, damping and airflow stability show only as
  drive, spin rate and blend; the control profile's rate approach and powered
  auxiliary control only as authority and rates; unstable controls only as the
  gap between requested and delivered stick.
- **Turbulence.** The disturbance applied is recorded; its strength, timing and
  random draws belong to `turbulence::Turbulence` in the host.
- **Other paths.** The restricted native adapter records only that it ran, with
  the control response and throttle lock it received. Wreck motion, ejection and
  system progression (temperatures, leaks, flameout timers) are outside it.
- **Measurements are never causes.** Angle of attack, sideslip and Mach come from
  `telemetry::AirData`, derived after the step; no effect cites them.

The record adds 1,504 bytes to `State` (1,728 to 3,232 bytes on macOS aarch64).

## Creator ground initialization

The player's whole wing starts on the ground. The player uses the takeoff
anchor and wingmen queue on the taxiway near it, with a staggered runway
fallback when the points are unusable. Wingmen wait until the player is airborne.
The player ground-start path requires the existing researched adapter. It retains
selected fuel and payload, resolves a source runway departure point, and adds the
aircraft model's wheel/CG clearance to the shared runway surface height. All
velocity components and rotation rates start at zero; gear/flaps are fully deployed,
brakes applied and engine at idle. The adapter starts supported, so initialization
is not misclassified as a gear-up or hard touchdown. Legacy/native adapters reject
this setup rather than changing mode. Ground motion and takeoff then use the same
existing simulation. [UI and fitted defaults](spec/quick-mission-menu.md#player-ground-start).

Ground crosswind difficulty uses the imported maximum takeoff weight, not the
current fuel/load mass. The user-specified noticeable/rough/limit thresholds and
universal ten-knot tailwind limit replace the earlier ground-speed wind ramp.
The fitted tire-grip transition ends at five knots ground speed; parked aircraft
retain position and attitude with brakes either applied or released. Full wind
still determines aerodynamic airspeed, including headwind during takeoff. Thresholds classify difficulty
and drive drift, without automatically crashing or denying control.
[Exact rules and provenance](spec/runway-wind.md).

Hybrid flaps now reduce the low-speed stall reference and add lift as well as
airflow-dependent drag. The lowest positive-G band is interpolated through
rotation and liftoff. Tire forces decrease as the wheels unload, and positive
height gains no longer require six feet/second of upward velocity. The existing
binary flap control is retained. Low-speed nose alignment now blends a modest,
stick-dependent trim target into the airborne response without a wheel-release
angle jump. [Rules and fitted constants](spec/takeoff-ground-contact.md).

## Ownship systems damage

The shared 120 Hz ownship state now carries oil, hydraulic fluid, engine health,
thermal progression, control/device faults and fuel-tank contents. Flight uses
the resulting power and control limits. External fuel burns before internal fuel
and reduces loaded mass as consumed. Autopilot refuses damaged controls. The
legacy, hybrid and restricted research adapters remain separately selectable.
The original event identities are reviewed; progression constants are fitted
and documented in [systems damage](spec/systems-damage.md). No autonomous
aircraft decisions were changed.

Partial wing/tail damage now reduces lift and control authority, adds drag and
creates asymmetric roll/yaw bias using the same regional fractions as the
mesh tear model. All airframe damage visuals are temporarily hidden below 100%,
while the physical penalties remain active. Source-eligible cumulative faults prevent heavy combat damage
from leaving all flight systems healthy. Coefficients and thresholds are in the
[systems contract](spec/systems-damage.md#regional-structural-damage).

## Destroyed aircraft

Airborne destruction hands motion to a shared fixed-tick wreck component. It
retains momentum, tumbles under aerodynamic torque and drag, and carries captured
surviving-engine thrust until fuel depletion, ground contact or an airburst.
Individual shutdowns make multi-engine thrust asymmetric. The player cannot
control the wreck. [Requested timing and fitted physics](spec/destroyed-aircraft.md)
apply independently of the selected living flight adapter.

Player ground crashes and falling-wreck impacts now end in a guaranteed explosion.
Nose/cockpit loss kills the pilot, but surviving engine thrust remains active
until impact or an airburst. Pilot death switches presentation to the exterior
view without affecting the fixed-tick wreck physics.

## Pilot escape

[Ejection](spec/ejection.md) detaches pilot motion from the abandoned aircraft.
All existing flight adapters retain their selection; the shared fixed-step
escape component runs independently of the existing wreck component. The AI
recovery estimate uses imported G envelopes, remaining authority and terrain.
Its thresholds are fitted, with John's healthy-aircraft guard above 200 AGL
and the [airfield catastrophe guard](spec/ai-airfield.md#ejection-during-an-airfield-sequence).


AI exterior rendering now uses each actor's actual gear, flap, hook, brake,
bay, exhaust and control-surface positions. Device poses interpolate between
simulation ticks with the same fraction as aircraft positions. Straight-flight
fixtures retain their existing fixed devices. This is a presentation correction,
not an extra flight-model or ground-height offset.
