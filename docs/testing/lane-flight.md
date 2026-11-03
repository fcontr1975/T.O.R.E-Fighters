# Flight lane

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

The flight lane flies the player's aircraft from ground start to touchdown in all fourteen
flyable identities (`f18`, `rafale`, `f14`, `a4e`, `x31`, `mig29`, `su27`, `mig21`, `su25`,
`mig23`, `su35`, `f22`, `f22n`, `faxx`), and checks the weapons, countermeasures, systems and
instruments that go with it. It was built in the 2026-09-28 overnight bug battery. The scenario
file is [`tools/battery_scenarios/flight.py`](../../tools/battery_scenarios/flight.py); how the
runner works is in the [testing overview](README.md). The `render-*` capture scenarios live in
[`tools/battery_scenarios/render.py`](../../tools/battery_scenarios/render.py).

`flight-missile-energy` runs 240 headless imported shots through the Energy
model twice, with four workers and with one. It checks identical rows, real
hits and misses, and an upper bound of one roll per two-second hammered
episode. Eight historical Spec AIM9B shots also cover raw susceptibility above
100, which that preserved percentage-only API must clamp before evaluating.
These are regressions for repeated rolls and comparison compatibility, not
fixed Monte Carlo hit-rate targets. The broad calibration/study results are in the
[missile energy baseline](../baselines/missile-energy.md).

The `flight-missile-defeat` scenario repeats 32 long AIM120/AA8 shots
with four and one workers, requires an observed post-burn defeat and rejects
any defeated shot that scores. Two additional stationary-target AIM120 shots
use the `defeat-edge` range band, just beyond the shared kernel's last live
intercept, and repeat with one and four workers. At least one inert body must
pass within the target's fuze/contact radius without damage. The CSV columns
`defeat_s`, `closest_inert_ft`, `inert_radius_ft` and `inert_damage` distinguish
kinetic defeat from ordinary control-time expiry and expose that near pass.
Synthetic Rust tests cover
exact before/at/after contact ordering, direct/proximity/ground damage,
powered cruise, mid-countdown checkpointing, countdown cache buckets with a
defeat deadline, and same-tick warning/AI defense removal with the body retained.

The offline missile_energy_ranges example accepts a local JT directory.
`--verify` checks the shared motor-shape and powered cruise-arrival guards,
primary and secondary reference bands, class floors, decoded
minimum margins, family and radar-role ordering, long-low escape and local
perturbations. Named residual exceptions have explicit bounds; unlisted stores
use the ordinary tolerance. `--fit-class` exports fixed-budget fuel/drag
candidates for the high head and low tail conditions. `--fit-ground` fits ground
references. `--fit-motors` and `--fit-staged` also explore bounded per-store
motor durations and thrust, never control time. `tools/missile_energy_fit.py`
selects families and radar roles jointly; synthetic Python tests cover selection,
bounds and parsing. These are fitted game coefficients, not engineering data.
`--diagnostics` opts into G demand/capacity/deficit and closing-speed math,
which normal motion and prediction skip, and reports defeat
at the edge, 0.1% beyond and 10% beyond. The near-edge miss reports whether
defeat, control expiry, terrain contact or another event stopped the flight;
a continued hit is reported as a sampled-boundary limitation. --measure-pairs is historical comparison only.
Timer-fitting tools require explicit --legacy-fit-pair/--legacy-fit-sparrow
names and print a warning. No search edits source or runs during gameplay.

`--motor-shapes` reports initial thrust/weight, dry-mass load, declared and
actual impulse coefficient, burn/control ratio, sustain-entry thrust/weight,
standardized time to Mach 1, powered-hit limit and reference/edge
arrival speed and powered state. A missed reference is explicitly marked as
a miss, so its final speed is not an arrival measurement. `--refine-motor`
and `--measure-motor` accept SOURCE FUEL DRAG BOOST_S SUSTAIN_S COEFFICIENT
BOOST_SHARE for a bounded refinement or measurement of one candidate.
Both reapply the guard. The complete offline search retries an empty envelope
with factor-1.1 probes after the ordinary factor-1.5 sweep, to find narrow
beam-intercept intervals. Nonempty envelopes keep their original search;
this is still sampling, not proof that every success island was found.
Runtime advice separately keeps each sampled region and advises the one for the present range, as specified in the
[range contract](../spec/missile-energy.md#estimates-and-acceptance). Thresholds and class floors have one home in
[the data document](../formats/missile-energy-data.md#per-store-motors-and-class-control-budgets).
Run the synthetic guard and exception tests with
`cargo test --locked -p tore-sim --example missile_energy_ranges -- --test-threads=4`.

The `flight-countermeasures-cadence` scenario runs an isolated real-AI Ace
AIM9M encounter at short range, head-on, co-altitude and seed 0. It requires
at least two pairs, exactly 30 ticks within each pair, at least 240 ticks
between pair starts and no more than six releases for the single threat.
All-skill reaction, reserve and encounter transitions have synthetic tests.

The offline `ordnance_envelopes` example accepts `JT_DIR OUT_DIR [SOURCES]`.
Its default survey covers eleven altitude cases, five aspects, shooter speeds
200/300 m/s against 250 m/s, and a 150 m/s target subset at three altitudes.
It exports physical and live-seeker intervals, with every sampled transition
refined to 0.5 m. A finite scan can still miss a narrower island or hole.
Single-case modes use `SOURCE point|cell|trace|advice LAUNCH_FT TARGET_FT
ASPECT_DEG SHOOTER_MPS TARGET_MPS RANGE_M`; `cell` takes a scan ratio instead
of range and `trace` additionally takes `physical|seeker`. Range is horizontal;
the runtime `advice` result reports slant-range edges and fixed-queue work.
The trace emits Mach, G demand/capacity, motor/drag force and seeker state.
`--override key=value,...` is diagnostic only: fuel, share, boost_s, sustain_s,
coefficient, drag, lift, g, induced, navigation, control and defeat are supported.
Counterfactual control/defeat overrides must never be mistaken for shipping
policy or an approved calibration. No mode edits game data.

The synthetic `behavior_hash_requires_a_model_revision_for_rule_changes` test
pairs the model identity with a quantized trajectory/contact trace. A changed
trace requires investigating the behavior, incrementing the relevant semantic
revision, and reviewing the new pair. An injected PN perturbation verifies that
behavior changes independently of the declared identity. This finite set of
traces is a tripwire, not exhaustive proof for every branch or platform.

## Variety aircraft checks

`flight-variety-*` checks each of the 23 new identities through real application
asset loading and 1,200 hybrid flight ticks, with finite motion, no crash and
bounded fuel/energy output. It is separate from the original roster's fighter
maneuver and AI acceptance cases. Source-backed powered-lift checks use
`tore-sim`'s `variety_flight` example; synthetic tests cover hover limits,
conversion, control release and exact-state continuation. See
[the variety flight evidence](../baselines/variety-flight.md).

`flight-variety-gcurve-*` sweeps the full-stick G limit of the seven transports
and airliners from 100 to 750 kt at 5,000 ft (`--maneuver gcurve`). Once the
limit has passed 1.4 G it must not fall below 1.3 G, through the top speed and
into overspeed. Before the fast-side hold
([flight model](../FLIGHT-MODEL.md#envelope-limits-and-loading)) it fell to
about 1 G well below the top speed.

## Air-to-ground weapons

`a2g-*` (scenario files
[`tools/battery_scenarios/a2g_bombs.py`](../../tools/battery_scenarios/a2g_bombs.py),
[`a2g_sights.py`](../../tools/battery_scenarios/a2g_sights.py),
[`a2g_guided.py`](../../tools/battery_scenarios/a2g_guided.py) and
[`a2g_missiles.py`](../../tools/battery_scenarios/a2g_missiles.py)) runs
`--a2g-run THEATER STEM --aircraft ID --load REC --attack KIND`, which flies a scripted
straight pass at a ground target of a Quick Mission, invulnerable, and attacks through the
real player path: the weapon keys select the station and the trigger releases. A store the
aircraft does not carry is fitted to a bomb, pod or air-to-ground missile station for the run
(`--count N`). The missile kinds also select the page and designate through the player's keys
(`--designate ag|ir|harm|visual|none`, `--unit ID`), and `--render OUT.png` draws the HUD at
the release. Each
weapon slice adds its attack kind as a submodule of `crates/tore-app/src/a2g_run/`.

| Scenario | What it flies | What must hold |
| --- | --- | --- |
| `a2g-bomb-motion` | Over `~QUCOL`: an A-4E Mk 82 from a level pass at 5,000 ft and 450 kt, an F-4B Mk 82 from a 30 degree dive released 3,000 ft above the target, an A-4E Mk 82 AIR 7-pack and an A-4E Snakeye, each released when the bomb sight's mean impact reaches the target | The solution obeys the bomb equation (fall and throw within 0.01 s and 2 ft); every bomb bursts within 2 ft of its own path and within the 1.5 degree scatter's reach of the mean point, its scatter within 1.5 degrees; the 7-pack is seven bombs in one tick for one round at the retail stagger and 66 percent of the speed; the Snakeye falls as long as the slick bomb and throws 66 percent as far ([bomb motion](../spec/air-to-ground.md#bomb-motion)) |
| `a2g-ccip-dive` | `--attack ccip` over the TVIET truck park: an F/A-18D's Mk 82 in a 30 degree dive from 2,000 ft, released when the HUD's CCIP pipper covers the truck; then a longer, shallower pass from 3 nmi | The truck is destroyed; every logged sight follows the retail arc rule (full inside 0.4 of the 20,000 ft range, one step fewer each fourteenth of the rest, the bare ring from the maximum, on retail's approximate distance), its glyph is 0x20 plus the arc, IN RNG inside the range, and the arc never shrinks as the range closes ([CCIP](../spec/air-to-ground.md#ccip-the-dive-pipper)) |
| `a2g-ccip-terrain` | `--attack ccip` toward the Vladivostok SAM site of surface seed 4 (`--unit 0x50000009`, undefended), which stands on a 768 ft plateau while the ground 24,000 ft short of it is lower: an A-4E's Mk 82 in a 3 degree dive from 15,000 and from 18,000 ft (slice ST-G, realistic CCIP) | At the release the pipper lies within 20 ft of the target on the plateau (`target-gap`), and in at least one run retail's pipper (`retail-gap`, the ground under the aircraft for the whole fall) is more than 50 ft worse (100 and 145 ft seen); the bomb comes down on the plateau |
| `a2g-level-t` | `--attack level` over `~QUCOL`: an F-4B designates the column from the A-G page, selects its Mk 82 (LVL on selection) and holds the trigger through a level pass at 5,000 ft and 450 kt | Exactly one bomb, released within three ticks of ground travel past the cue with the cross-track miss inside the radius; the release time falls and the marker rides down before it; OVERFLY after; the burst inside the record's splash radius of the target plus the 1.5 degree scatter's reach (about 595 ft; the bomb is one random draw) ([the level T](../spec/air-to-ground.md#the-level-release-t-lvl)) |
| `a2g-rocket-ccip` | `--attack rocket`: an A-4E's LAU-61s in a 10 degree dive on a truck, fired once the rocket pipper says IN RNG, held a second | Rockets hit and destroy the truck; a rocket that bursts on the ground does so within a foot of the pipper of the tick it was fired on ([rockets](../spec/air-to-ground.md#rockets-and-guns)) |
| `a2g-warnings` | `--attack ccip` with an A-4E's Mk 82 from a 30 degree dive at 300 ft, a level pass at 400 ft and one at 1,500 ft | LOW and FRAG, FRAG only, and neither; the LOW bomb lands a dud (a puff, nothing destroyed), the others burst ([warnings](../spec/air-to-ground.md#warnings)) |
| `a2g-rocket-pod` | An A-4E's two LAU-61 pods tapped three times, held for a second and held until empty; two LAU-10 pods held for two seconds | 38 rockets in the pods, one a press; four or five in a second held, a quarter second apart on the game's quarter-second clock; all 38 when held on; the LAU-10 pods hold 8 and fire one however long the trigger is held ([rockets](../spec/air-to-ground.md#rockets)) |
| `a2g-rocket-smoke` | An A-4E's LAU-61 pod held for a second over `~QUCOL` | The `motors` line: rockets whose motors lit, and missile smoke puffs in the air while they fly (bug B6: rockets have no seeker and smoked nothing before; [smoke](../spec/damage-smoke.md)) |
| `a2g-gun-key` | `--a2g-run --attack strafe`: an A-4E with an Mk 82 selected, Tab held for two seconds, the trigger never pressed | the gun fires a burst and its ammunition falls round for round, about one round in three a tracer, the selection stays on the bomb, the bomb stays on its pylon ([the gun key](../spec/air-to-ground.md#the-gun-key)) |
| `a2g-cluster` | Over `~QUCOL` (scenario file [`a2g_cluster.py`](../../tools/battery_scenarios/a2g_cluster.py), attack kind `cluster`): an A-4E CBU-87 and Mk 20 from a level pass and an F-4B RBK-500 from a 30 degree dive | The burst is the type 18 family with no crater; a vehicle inside the record's splash (2,200, 1,600 and 2,500 ft) loses the record's damage times 70 percent, falling in a line to nothing at the edge, and one outside loses none; the 28 bomblet flashes the renderer derives are the same on every derivation, inside 1,200 ft of the burst and start within its first half second ([cluster flashes](../spec/ordnance-presentation.md#cluster-bomb-flashes)) |

`a2g-catalog` (menus lane, [`a2g_catalog.py`](../../tools/battery_scenarios/a2g_catalog.py))
runs `--validate-ordnance`, whose audit covers the air-to-ground records on the Load
Ordnance page and the Cheat-only novelty bombs: the normal and Cheat catalogs are checked
against the station rules on all 36 aircraft, and Boulder and Moose must have no capacity,
be refused and fail validation without Cheat loading ([novelty
rule](../spec/ordnance-presentation.md#novelty-bombs-are-cheat-only)).
| `a2g-laser-self` | `--attack laser` over `~QUCOL`: an F-16C designates a T-80 from the A-G page (T), its Pave Spike lases automatically with the GBU-10 selected, and the bomb is released from 15,000 ft at 480 kt on the HUD's IN RNG cue; again with Ctrl+L turning the laser off 20 s after the release | The bomb leaves at 90 percent of the speed, guiding on aircraft 0's spot from inside its reach, its time to impact counts down, and it destroys the tank; with the laser off it goes from guiding to blind, falls and misses, and the tank is unhurt ([the laser](../spec/air-to-ground.md#the-laser)) |
| `a2g-laser-buddy` | Two seats (Open seating): an Su-25 two miles behind designates with Ctrl+X (Easy targeting) and lases by hand; an F/A-18D, which has no designator, drops a GBU-10 from 12,000 ft | The release guides on the Su-25's spot (buddy yes, own laser off), lands within 40 ft of the tank and destroys it ([buddy lasing](../spec/air-to-ground.md#buddy-lasing)) |
| `a2g-jdam-cloud` | `--attack jdam --condition cloudy`: an F-16C's GBU-30 from 12,000 ft at a standing object of the TVIET truck park, then at a `~QUCOL` tank that drives on at 50 ft/s | The first lands on the stored point and destroys the object; the second lands within 40 ft of the stored point and at least 500 ft from the tank, which is unhurt ([guided bomb release range indicator](../spec/air-to-ground.md#guided-bomb-release-range-indicator)) |
| `a2g-bar` | `--attack jdam --release bar` then `--attack laser --release bar`: an F-16C's GBU-30 from 12,000 ft at 450 kt at a standing object of the truck park, then its GBU-10 on its own Pave Spike from 8,000 ft at 400 kt at a `~QUCOL` tank; the run turns onto the designated object and holds the trigger in LVL | One release each, within 60 ft of the T's bar (along track) and within 10 degrees of the stem; the bomb guides to the end, lands within 60 ft of the target and destroys it, its time to impact counts down ([the level T for guided bombs](../spec/air-to-ground.md#guided-bomb-release-range-indicator)) |
| `a2g-laser-missile` | `--attack laser`: an Su-25's AS-14 from 3,000 ft, on its built-in designator's spot | Released guiding on its own spot; ends guiding within 60 ft and destroys the tank ([weapon classes](../spec/air-to-ground.md#weapon-classes)) |
| `a2g-at2` | `--attack at2`: a Mi-24 300 ft up, 2 nmi out at 60 kt, launches an AT-2 at a `~QUCOL` tank (Easy targeting's Ctrl+X: it has no ground page); again turning 90 degrees away 2 s after the launch | The AT-2 is command guided and hits (the T-80 takes 120 of its 200 hit points); turning away makes it blind for good, it never guides again and misses ([weapon classes](../spec/air-to-ground.md#weapon-classes)) |
| `a2g-missile-burnout` | An A-10's AGM-65G from 6 nmi at 3,000 ft over `~QUCOL` | The `motors` line: one motor lit, smoked and burnt out while the missile still flew, which ends its smoke and its drawn flame (bug B7; [smoke](../spec/damage-smoke.md)) |
| `a2g-terrain-cruise` | F/A-18D AGM-65A at `VLA QVARMOR`, target `0x50000000`, 6 nmi, 1,100 ft above target ground, heading 60, 350 kt | Logged altitude rises above 1,900 ft over the ridge, then the missile resumes its profile and kills the tank within 40 ft. Without terrain clearance the logged maximum was 1,866 ft ([cruise clearance](../spec/missiles.md#cruise-terrain-clearance)) |
| `a2g-terrain-mask` | F/A-18D AGM-65G at `VLA QVSUP`, target `0x50000000`, 6 nmi, 300 ft above target ground, heading 180, 350 kt, 10 s | Terrain rises above the measured sight line near the target; TERRAIN MASK, no launch. This caught a ridge skipped by the old eight-probe visibility check ([terrain gates](../spec/air-to-ground.md#terrain-at-lock-and-launch)) |
| `a2g-maverick` | Over `~QUCOL`: an A-10's AGM-65G on a tank designated on the A-G page (Ctrl+R, T), the same with nothing designated (BORE), an AV-8B's AGM-65G on a tank designated on its FLIR (M, T), an A-10's AGM-65A on the tank Enter picks, and an AGM-65A from 6 nmi on the A-G page | CUED with the seeker locked on the designated tank, launched at it and killed; in BORE the candidate and the missile's target are the strongest return in the 5 degree bore circle, locked at release, killed; the FLIR run kills, both AGM-65A runs hit and the 6 nmi shot ends within 40 ft of the tank ([missiles](../spec/air-to-ground.md#missiles-and-the-harm-boresight-cue)) |
| `a2g-slam-shrike-as15` | SLAM on the column, Shrike on the Straight Flush, AS-15 on the A-G track | Each selected ground target is destroyed |
| `a2g-laser-as30l` | Su-25 AS-30L (AS30.JT) on its built-in designator | Guides on its own spot and destroys the tank |
| `a2g-harm-bore` | Over `~QVRDRI` (seed 4, heavy defenses): an F-4G's AGM-88 with nothing designated, at an SA-6 battery's Straight Flush at ace and at novice experience | The HUD cue lists several emitters with the radar as the BORE candidate; the HARM leaves in BORE at it; the ace radar shuts down within 10 nm and the HARM keeps homing, locked, and kills it; the novice radar stays on and dies |
| `a2g-harm-page` | The same F-4G on the HARM page (M), T to the Straight Flush | The designation turns the cue to CUED; the HARM leaves in CUED at the radar and kills it |
| `a2g-antiship` | Over `~QUSFLT`: an F/A-18D's AGM-84A and a Rafale's AM-39 at the Krivak designated on the A-G page | Launched midcourse at the Krivak, the seeker goes active on it and it is hit |
| `a2g-sarh` | Over `~QUCOL`: a MiG-23's AS-7 on a tank designated on the A-G page, then the same with the A-G page turned off two seconds after launch | On the track it kills its tank, never unguided; with the page off it loses its target for good (status Lost, unguided) and the tank is not hit |
| `a2g-sarh-uncued` | A MiG-23 AS-7 released at 0.5 seconds with no A-G track; the page opens at 3 seconds and designation follows | Initial targetless Search without permanent disablement, then Locked on later illumination; `--attack sarh-uncued` uses ordinary player controls |

## AI strike driver

Class-order probes accept `--order-class vehicles|tanks|structures|aaa|sams|ships`
and `--order-wingmen 1..9`. Order probes can use `--record PATH` without a GPU.
The `SELECTION` line identifies the actual ground target, side, class and weapon;
`WINGMAN` totals report each member's launches, hits and damage when the count is
explicit. These probes exercise the ordinary seat order path, including host
processing of client orders.

`--a2g-run THEATER STEM --pilot ai` (scenario file
[`tools/battery_scenarios/ai_strike.py`](../../tools/battery_scenarios/ai_strike.py), source
`crates/tore-app/src/a2g_run/ai.rs`) is the headless driver for the AI strike round (plan
slice ST-D). The player-side `--attack` kinds above fly the aircraft through the seat's
controls. This one lets the AI fly it, from the same mission step as the game, and only
watches. It exists so every behaviour slice of the round can test from the first day. Since
slice ST-T the driver orders the AI onto the target (an ordered ground target, which outranks any
briefing) and the AI flies its strike tactics ([AI strike](../spec/ai-strike.md)).

The mission is an open one (every plane on the AI), one friendly wing, no enemy. The striker
starts `--from NM` short of the target at `--altitude` ft and `--speed` kt on `--heading`
(and `--dive`), loaded as the player-side kinds load it (`--aircraft`, `--load`, `--station`,
`--count`). The target is the Nth ground target of the stem (`--target N`, or `--unit ID`),
with `--defenses AAA SAM` and `--surface-seed N` as before.

| Option | What it does |
| --- | --- |
| `--skill 0..3` | The striker's wing skill, Novice to Ace (Average by default) |
| `--profile straight\|popup\|fasthigh\|dive\|level\|orbit\|standoff\|laydown\|strafe` | **Forces** a pass, for coverage (`orbit` is the AC-130's, `standoff` a helicopter's stand-off attack, `laydown` a drag bomb's low level pass, `strafe` a gun run). The real AI chooses its pass from the perceived threat and its energy (John, 2026-10-11), not at random; the flag lets a test fly each pass once |
| `--handback S` | A human-flown run handed to the AI: a seat flies the striker on the scripted pass, designates the target on the A-G page (or Easy targeting's Ctrl+X for an aircraft with no ground page), selects the store, and gives the aircraft back `S` seconds into the pass. `--mode ccip\|lvl` sets the bomb sight in the human leg, `--trigger` holds the trigger there (pickle and hold) so a bomb leaves before the handoff, `--laser` turns the laser on by hand (Ctrl+L), and `--take-back S` has the seat take the aircraft again `S` seconds after the handoff and ends the run there. For the takeover tests |
| `--choose-store` | The AI chooses its own store instead of the fitted one (slice ST-L): a laser store only with a spot assured |
| `--buddy ID` | A second seat flies aircraft ID as a buddy (slice ST-L): a scripted pass 1 nm ahead of the striker, 3,000 ft right and 1,000 ft higher, which designates the nearest ground object with Ctrl+X and lases it by hand (Ctrl+L); the striker is ordered onto what it designated and chooses its own store. A striker with no designator drops on the buddy's spot (`spot_owner` on the RELEASE line). `--buddy` with `--buddy-ai` on the player-side `--attack laser` run is the other way round: the AI flies the buddy, is given the wing order's call and lases for the seat's bomb |
| `--order KIND` | Wing orders: a seat leads a two-ship wing on the pass, designates the target, and gives the wingman (the striker) the order `--order-at S` seconds in (5 by default). Kinds: `engage-my-target`, `engage-from-formation`, `attack-on-contact`, `protect-me`, `disengage`, `bug-out`, `land-selected`, `sort`, and the strike round's `engage-ir-laser` (Alt+F), `engage-class` (Alt+W) and `lase-my-target`. The order is the seat's own command: the world reads the ground target from the seat's designation and its targeting sight, and the wingman takes it or refuses. The driver does not give the wingman its target in an order run, the order does |
| `--quick-wing` | A Quick Mission's own strike wing: friendly wing 2 is two of `--aircraft`, nobody flies or orders it, and the mission's default role (bomber-class aircraft strike when a ground target is set) sends it. `ROLE` reports the role it was given |
| `--render OUT.png --render-at S` | One offscreen picture of the striker and its target `S` seconds into an AI run, the chase camera on the target line (slice ST-F) |
| `--offset FT` | The striker starts this far right of the pass line (negative: left), for the first-pass convergence runs |
| `--sead-at S` | A stand-in for a SEAD flight (slice H4): every hostile SAM and gun site but the target is destroyed `S` seconds into the run (`SEAD t= killed=N`), so a helicopter holding off can be seen to resume |
| `--sead-site ID` | With `--sead-at` or `--sead-after-shot`: the stand-in destroys only this site (repeatable, the id as the SITE lines name it), so a site the helicopter has not met yet stays in place, for laying out an ambush after a resume |
| `--sead-after-shot S` | The SEAD stand-in goes in by the shot, not the clock (2026-10-29): `S` seconds after the first round one of the `--sead-site` sites (any site when none is named) launches at the striker, they are destroyed (`SEAD t= killed=N`). For an ambusher whose second round, 10 s after its first, would hit as well. Not with `--sead-at` |
| `--place-site ID,BEARING,RANGE_FT` | Before the first tick the hostile SAM or gun site `ID` (as the SITE lines name it) moves to that compass bearing and range from the target: its surface unit, its arms, its combat row and hit box (the drawn scene keeps it where it was). Prints `PLACED`. Repeatable: each named site moves, in the order given. For laying out the threats a helicopter meets (slice HF1) |
| `--site-sight ID,FEET` | The hostile site `ID` is listed by an eye only from `FEET` of it (at least 256), in place of its type's 19,968 ft (`maxVisDist` 78 units of 256 ft). Prints `SIGHTLIMIT`. A site dug in and hidden well: the helicopter does not know it until it is near, and the site fires as always. Repeatable. Written for the layout of an ambush by a site the helicopter did not know (2026-10-29; why terrain cannot do it: [AI strike](../spec/ai-strike.md#helicopter-threat-awareness)); since the game hides shoulder-fired teams itself the scenarios no longer need it, and it stays as a tool to try a spotting distance or hide a vehicle site |
| `--terrain-map RADIUS_FT,STEP_FT` | Before the first tick prints the ground height on a grid round the target, north row first (`TERRAINMAP` and `TERRAIN` lines, whole feet; a step of at least 100 ft) |
| `--terrain-nodes RADIUS_FT` | Before the first tick prints the ground height at every node of the imported terrain lattice (8,192 ft apart; the ground between nodes is interpolated from them) within that radius of the target: a `TERRAINNODES` header with the first node's numbers and the grid size, then one `NODE` line per row from the south. For working out sight lines exactly |
| `--follow-egress` | After the last impact the run goes on, within `--seconds`, until the striker egresses, and settles from the EGRESS line, so its way out is seen (slice HF1) |
| `--follow-threat` | After the last impact the run goes on, within `--seconds`, while a helicopter keeps out of a ring it means to come back from (its latest THREAT is `threat_leave`, or it is holding off), so the resume is seen (slice H2); after a resume it goes on until the next release (slice HF1) |
| `--mission-altitude FT` | The Quick Mission altitude setting (5,000, 10,000, 20,000 or 40,000 ft), the airborne height a heavy bombs from |
| `--station N` one past the last | With `--load`, adds an internal bay there: a heavy with no ground station (the C-130) can carry bombs for the level run |
| `--render-scene DIR` | Draws one frame offscreen at 1080p (no window), from a chase camera off the striker's right wing, 0.4 s after its first release, as `STEM-PROFILE-SKILL.png`. A companion aircraft (friendly wing 2) sits on a second seat, far from the pass, to present the picture from |

Every line starts `ai-strike:` and is `key=value` words (the battery reads them with
`ai_strike.parse_runs`):

| Line | Meaning |
| --- | --- |
| `setup` | Aircraft, store, station, target (its id and `name`) and where it stands, hit points, skill, forced profile, handback, order, mode |
| `PROFILE name=P requested=R` | The first line echoes the request (`note=forced` or `note=chosen`); a later line names each pass the AI flies |
| `start` | The striker at tick 0: range to the target, height above the ground, speed |
| `DESIGNATE`, `HANDBACK`, `ORDER`, `ORDER-REPLY`, `ORDER-STATE` | The human leg: the target designated, what was set when the aircraft was handed back (designation, selected station, bomb sight mode, laser hand setting `hand-on`, `hand-off` or `auto`), the order given and the world's reply (given, refused or failed, with its words), and what the wingman holds after it (`strike_order`, the size of its `class_order`, `lase_order`) |
| `ROLE role=strike\|cap` | `--quick-wing`: the role the mission gave the wing |
| `TAKEOVER` | After a handback: the attack the AI continues (target, kept station or `ai`, bomb sight mode), or `none` |
| `KEPT` | While the AI flies a handed-back aircraft: the kept cockpit's mode, laser hand setting, whether it lases, and the selected station, each time one changes |
| `TAKEBACK` | A `--take-back`: what the human finds (designation, station, mode, laser hand setting, rounds left on the store) |
| `ACTIVITY name=...` | What the AI says it is doing, each time that changes |
| `PHASE name=...` | The striker's strike phase (Ingress, Attack, Lase, Exit, TurnBack, Wait, Defend, Home, HoldOff), each time it changes |
| `TARGET target=T phase=P` | The striker's strike target, each time it changes, with the phase it changed in: an early retarget (a target killed before the pass released, John, 2026-10-28) changes it inside `Attack` |
| `COMMIT open=yes\|no release_s=S held_defence=yes\|no held_at=T\|none` | The committed-to-release window (John, 2026-10-28) opened or closed: the estimated seconds to the release (`none` once the pass has ended or released, or while the cue is not closing on the aim) and, on closing, whether a missile or gunfire defence was pending while it was open (one the window held back) and when it was first seen (`held_at`, the run's seconds) |
| `THREAT decision=D target=T ring=R words="..."` | A helicopter's threat decision (slice H4): `threat_hold_off`, `threat_resume`, `threat_retarget`, `threat_home` or `threat_leave`, the target it concerns and the ring that blocked it (a defence's id, `0x0` for an ambush); the replay records each as an `ai.strike` event. Followed by a `RINGS` line |
| `RINGS n=N rings=id:radius:from_target,...` | The threat rings a helicopter knows (slice H4), with each `THREAT` line and each `PHASE` change of a helicopter: the defence's id, the ring's radius and its distance from the target, feet |
| `HOLDOFF` | Every ten seconds of a hold-off (slice H4): seconds held, distance from the hold point, height above the ground and range to the target |
| `NOLAZE decision=manual or rtb` | A striker with laser stores and no laser support decided (slice ST-G): `reason` (`no_laze_manual` or `no_laze_rtb`), the store, station and target, and the `words` on the radio. Once |
| `SITE` | Once, after `setup`: each standing hostile SAM or gun site of the mission (id, name, class `SAM` or `AAA`, hit points) and its horizontal distance from the target (`target_ft`) and from the start (`start_ft`), for reading a defended run (slice ST-H) |
| `BREAK` | While a helicopter flies its defence (slice ST-H), once a second: its heading and track, the threat's compass `bearing` (or `none`), the break's `offset` off that bearing (60, 90 or 120 degrees), the seconds of the next ten that terrain hides it from the site (`masked_s`) and that it could not climb clear of the ground (`blocked_s`), the flight path asked for (`pitch`), its height above the ground, its speed in knots and bank |
| `SHOT site=ID range_ft=R known=yes\|no` | A ground site launched a round (not a gun's) at the striker, `R` feet from it; `known` says whether the striker knew the site at that moment (the site's ring was in its ground picture, and not only because this launch gave a hidden shoulder-fired team away; `none` without a picture). An ambush in the sense of the threat rule is a shot from a site it did not know |
| `SEEN site=ID range_ft=R` | A hostile SAM or gun site entered the striker's ground picture for the first time (the first time its ring is known), `R` feet from it |
| `SIGHT site=ID range_ft=R clear=yes\|no` | The terrain line between the striker and a hostile site within 30,000 ft opened or closed (the line the eye uses: eight probes from the striker to the site), for reading why a site was or was not known when it fired |
| `PICTURE contacts=N` | The number of ground contacts the striker's picture holds, each time it changes (the 50 nearest the eye lists) |
| `SIGHTLIMIT id=ID sight_ft=F was_ft=W` | `--site-sight` set the site's listing distance |
| `HURT` | The striker lost hit points: the points left and its first, and the system faults the hits have caused (index and count) |
| `HOLD` | Every 5 s of a lase hold: range from the target, height above it, speed, bank |
| `LASER plane=P state=S mask=M` | The laser of the striker (and of the buddy), each time its state or mask changes (Off, Ready, Lasing, Masked: Range, Gimbal or Terrain) |
| `RELEASE` | A store left the striker: time, store, station, height, speed, range to the target, and the pass's number, its drawn aim offset from the target (`aim_ft`) and whether the draw lies inside the release circle (`inside=yes`) or in the ring R to 2R (`inside=no`), the lead laid on a moving target (`lead_ft`, 0 on a standing one) and the dive angle (`dive_deg`).; for a guided weapon, its mode and the owner of the spot it follows (`guided`, `spot_owner`). A gun burst is one `RELEASE` (its first round) with `burst=B rounds_left=N`, `aim_ft` being the burst's start offset (its rounds walk onto the target over the skill's time, slice ST-G) |
| `IMPACT miss_ft=F` | A released round ended; its horizontal distance from the target (the object the striker released at, where it stands when the round ends) in feet (the nearest crater or burst within 60 ft of its last place, else that place). For a gun burst, once its rounds have ended: the distance from the target to the middle of the rounds that reached the ground, with `rounds`, `ground`, `near` (rounds that ended within 50 ft of the target where it stood as each ended), `hits` (rounds that struck it, that is entered its volume) and `damage` (the hit points they took) |
| `BURST` | An AC-130 burst began (slice ST-F; gun rounds are not RELEASE lines): its number at the target, its walked-in aim offset (`aim_ft`) and whether the draw lies inside the circle, the orbit's horizontal radius, height above the target, slant range and bank, the speed and the gun rounds fired so far |
| `RENDER path=P` | `--render-scene` wrote its frame |
| `KILL` | A ground object the striker destroyed, and whether it was the target |
| `EGRESS` | After a release and once the rounds have landed, the striker is a nautical mile past its closest approach and opening |
| `LOST` | The striker was destroyed, crashed, ejected or left the AI (`crashed=yes` when its flight model says it hit the ground, which a shot-down aircraft's wreck also does) |
| `summary` | Counts of the above, the final activity, the target's hit points, the store's rounds left, the gun rounds and bursts (`gun_rounds`, `bursts`), and the seconds a laser weapon of the run flew guiding (`fall_s`) and how many of them the spot it followed was not lasing (`blind_s`), the lowest height above the ground it flew (`min_agl_ft`) and the seconds a helicopter flew its break (`break_s`); then `no release` if there was none, and `done` |

The run ends 20 s after the striker is lost or its last round has landed. The hooks the strike
slices fill are one `hooks` module in `ai.rs`: ST-T hands the AI its target (and the driver's
store) and forces the `--profile` pass and reports it, ST-K reads what a takeover kept, ST-O
adds order kinds.

| Scenario | What it flies | What must hold |
| --- | --- | --- |
| `ai-strike-driver` | An A-4E's Mk 82 on `~QUCOL` with the AI flying, at skills 2, 0 and 3, the first two with a forced profile | The report is consistent with itself (the summary counts the lines, a release comes before its impact and its egress), the setup and PROFILE lines echo the request. Passes before and after the strike behaviour |
| `ai-strike-handback` | A seat designates the column, selects the Mk 82 in LVL and holds the trigger; the bomb falls and is reported; the aircraft is handed to the AI at 30 s. A second run is handed back at 6 s with nothing released | HANDBACK shows LVL and the bomb's station, the human's release comes before the handoff and ends within 1,500 ft, the AI reports an activity after it |
| `ai-strike-takeover` | An F-16C's GBU-10 pass at the column from 8,000 ft: the seat designates, selects the bomb in LVL with the laser on by hand and hands back 3 s in, then takes the aircraft back 60 s after | TAKEOVER names the human's target, station and LVL; the AI flies a level run and releases; the bomb ends within 500 ft; every KEPT line keeps LVL, the hand setting and the station, and lases until the bomb ends; TAKEBACK finds LVL, the laser on by hand, the station and one bomb fewer |
| `ai-strike-order` | A seat leads a two-ship wing, designates the column, orders Engage my target | One ORDER line with a designation, and the world's reply (taken since slice ST-O) |
| `ai-wing-ground-order` | A seat leads a two-ship wing and orders the wingman onto the ground: Alt+E and Alt+R on the Ukraine column (an A-4E's Mk 82), Alt+F with the lead's Pave Spike on the factory `~QUFACT` (an F-16C's GBU-30), Alt+W with a SAM of the defended column designated (`--defenses 3 3`; the wingman's Mk 82 may be shot down, so it holds the class order but need not release), and Alt+E refused by an F-14 wingman with only air-to-air stores | Each takes or refuses as it should: the first four are "1 applied" and leave the wingman holding the object (a strike order) or its class (Alt+W), and the wingman releases (not required for the class order, which flies into the defences); the last is refused ("no usable surface store"), holds nothing and releases nothing |
| `ai-wing-lase` | The lead of two F-16Cs designates the factory and orders Lase my target, 60 s | "1 applied", the wingman holds the lase order (and the duty's strike order on the same object), enters the lase hold within 2 s (PHASE Lase), its laser comes up Lasing and is still lasing at the end, at least two HOLD lines between 2 nm and the laser's reach, no release |
| `qmc-strike-wing` | A Quick Mission with the factory `~QUFACT` as the ground target and a second friendly wing (not the column: its guns shoot the wing down on the way in): two A-4E (bomber class), then two F-16C (fighters) | The A-4E wing is given the Strike role by the mission's default and releases on the target, and its bomb ends within the skill's release limit of the target the wing struck (the wing picks its own, so the miss is measured to the object the striker released at, not to the driver's; before nr-cue it read 1,830 ft off the driver's target); the F-16C wing keeps CAP and releases nothing |
| `ai-strike-bombs` | An A-4E's Mk 82 on the Ukraine column `~QUCOL`: straight, pop-up, fast-high, dive and level forced once at skill 2, and skills 0, 1 and 3 on the dive | Each run names its pass and releases with its drawn aim inside the skill's release circle (or in the ring R to 2R when the draw says so), ends the bomb within the circle plus the release gate plus 400 ft of scatter (707, 617, 526 and 443 ft for skills 0 to 3; the ring's edge plus them, 986, 813, 641 and 476 ft, for a ring draw, see [AI strike](../spec/ai-strike.md#skill-and-release-precision)), shows an EGRESS and is not lost. Not the TVIET truck park: its radar-laid guns (26,200 and 44,250 ft reach) shoot an A-4E down at 5,000 ft within 25 s whatever pass it flies |
| `ai-strike-rockets` | An A-4E's LAU-61 on `~QUCOL`, straight runs at skills 1 and 2 (an Ace rarely meets its 10 m circle on a short straight run) | A rocket burst with the aim inside the circle, each rocket within the circle plus the gate plus 400 ft, an EGRESS, not lost |
| `ai-strike-maverick`, `ai-strike-jdam`, `ai-sead-harm` | An A-10's AGM-65G on `~QUCOL` (fast-high), an F-16C's GBU-30 on the factory `~QUFACT` (level; a JDAM flies to the point stored at release, so a moving column would be missed), an F-4G's AGM-88 on the `~QVRDRI` radar (fast-high) | The named pass, a release and an impact, the target hurt, the striker not lost |
| `ai-strike-heli` | On `~QUCOL` from 4 nm at 1,500 ft and 120 kt: a Mi-24's AT-2 (its own choice, the pop-up), an AH-64's LAU-61 at skills 2 and 3, followed out with `--follow-egress` for up to 300 s (slice HF1) | The named pass, a release and an impact; the AT-2 hurts the tank; the rockets' drawn aim lies in the circle (or the ring) and each rocket ends within the circle plus the gate plus 400 ft; not lost. The AH-64 runs must EGRESS (they turn away from the column and mask low, about 47 s after the release) and live through the 20 s after it; before slice HF1 the run ended 20 s after the impact and the AH-64, flying on over the column, was shot down 2 s later. The stand-off attack is no longer forced on a quiet column (slice ST-H): the AI chooses it under SAMs itself, in `ai-strike-heli-defended` |
| `ai-strike-heli-defended` | A Mi-24's AT-2 against defences (slices ST-H and H4, John's threat rule of 2026-10-12), 4 nm out at 1,500 ft unless named: heavy AAA on the column (skill 1); light AAA at the factory `~QUFACT` (skill 1) with its ZSU-23 moved 4,500 ft north of the factory (`--place-site 0x5000000c,0,4500`, 2026-10-29); light SAMs of seed 19 (a SAM that out-ranges the AT-2); light SAMs of seed 4 (an SA-14 team on the target, up to 400 s); the same with `--sead-at 60`; two ambushes on the factory `~QUFACT` (`--defenses 0 1`, up to 400 s with `--follow-threat`; slice HF1, a new layout on 2026-10-29): a lone SA-7 team (surface seed 7, `0x5000000f`, skill 2) and a lone SA-14 team (seed 8, `0x5000000e`, skill 3), each moved to bearing 190 and 7,000 ft from the factory (`--place-site`), hidden by the game's rule for shoulder-fired teams (since 2026-10-29 the helicopter spots one only within 3,500 ft at skill 2 and 4,000 ft at skill 3, or once it launches; before, the runs used `--site-sight 3000`) and destroyed 4 s after its first launch (`--sead-site`, `--sead-after-shot 4`); from 4 nm at 200 ft on seed 1 (the layout where the AI would choose the stand-off); light SAMs of seed 16 (`--defenses 0 1 --surface-seed 16`, from `ai-strike-heli` since 2026-10-29: it starts 12,100 ft from an SA-14 team, inside its ring) | Heavy AAA: an `AHEAD` line, a release and an impact, the tank hurt, no THREAT decision, and the summary's `reach_margin_ft` 0 or more (never inside the reach of a gun it knew). Since the [move-ahead ambush](../spec/ai-strike.md#move-ahead-ambush) of 2026-10-29 it plans on where the tank will be: from 1 s it sets up west of the road, the column turns south-east at about 62 s and spoils that plan, and it plans again and fires at 86 s from 13,900 ft; the AT-2 takes 120 of the T-90's 200 hit points and the exit turn comes about 960 ft into a ZSU-23 ring's 2,000 ft margin. Before, each stand-off planned on the tank's place was spoiled within seconds as the column drove on among the standing guns (every gun of `--defenses` stands), and at 113 s it held off without firing. Factory: the stand-off and a kill (it sets out on the pop-up and gives it up for the stand-off once it sees the ZSU-23 near the factory, 20 s in; at the gun's own 2,650 ft no firing point leaves room for the advance, hence the move). Seeds 19 and 1 (seed 1 names no pass since 0.2.2: it holds off from the start): THREAT `threat_hold_off` then `threat_home`, no release. Seed 4: THREAT `threat_hold_off`, `threat_leave`, `threat_resume`, `threat_hold_off`, `threat_leave` and `threat_home` in that order, an `AHEAD` line, each leave after a SHOT with `known=no`, and no release. Until 2026-10-29 it held off the SA-14 it knew and went home; since the game hides shoulder-fired teams, the team on the target launches at 29 s (`known=no`), the helicopter holds off its ring and breaks the round, the column drives out from under the ring and at 198 s a move-ahead shot stands clear of it, and the seed's other hidden SA-14 team launches at 204 s (`known=no`); it holds off again and goes home at 385 s. Seed 16: a SHOT with `known=no` (20 s in), THREAT `threat_leave`, at least 2 BREAK lines and no release; until 2026-10-29 it knew the SA-14 from the start and flew a stand-off from outside its ring that hurt the tank, but the game now hides the team, whose launch gives it away (the second round, 10 s later, is `known=yes`), and two hits send it home. SEAD: `threat_hold_off`, the SEAD line, `threat_resume`, a release and the tank hurt. Ambush runs: a SHOT with `known=no` before the `threat_leave` (since 2026-10-29 `threat_leave` is the answer to a shot from a site it did not know, and a leave after only `known=yes` or `none` shots fails in any run), then THREAT `threat_leave` (ring 0, the ambush) and `threat_resume` in that order, at least 2 BREAK lines, a release and the factory killed by the shot after the resume. The SA-7 launches at 32 s from 10,900 ft (the SA-14 at 20 s from 13,400 ft), the helicopter breaks, takes the first round (78 of 103 hit points) and flies on; the second round, 10 s after the first, would hit too and send it home, which the stand-in prevents. The resume comes at about 119 s (101 s) and the kill at about 226 s (223 s). The layout does not hang on the skill, the start or the seed; the site can move across a wide area and the spotting distance anywhere up to 11,000 ft (the spec, "The ambush layout of 2026-10-29", has the margins). Every BREAK at 60, 90 or 120 degrees off the threat, 75 knots or more, no steeper than 10 degrees, never blocked by the ground; the helicopter never lost and never lower than 150 ft above the ground. The switch to another target after the wait is covered by the unit tests: no driver layout has a second target outside every ring |
| `qmc-strike-heli` | A Quick Mission with the factory as the ground target and a second friendly wing of two AI helicopters, a Mi-24 pair then an AH-64 pair (slice ST-H) | ROLE strike from the bomber-class default, a start 1,500 ft over the ground, a release (the AT-2, then a rocket burst). The factory's defences may bring them down, so a loss is allowed |
| `ai-strike-ac130` | An AC-130 from 3 nm at 6,000 ft on `~QUCOL` for 240 s at skills 0, 2 and 3 (the TVIET truck park's radar-laid guns shoot it down on the way in) | PROFILE orbit; BURST lines whose walked-in aim lies inside the circle (the ring for a ring draw); after five bursts the slant range inside 8,000 to 13,000 ft and the bank at most 30 degrees; gun rounds fired; the tank killed; not lost |
| `ai-strike-laser-self` | An F-16C's Pave Spike and a GBU-10 on `~QUCOL` from 15,000 ft at 480 kt, and an Su-25's built-in designator and a GBU-10 from 8,000 ft at 400 kt, the AI flying both (slice ST-L) | The F-16C goes from the pass to the hold at the release (a PHASE Lase line within a second of it), flies an offset turn (a bank over 10 degrees) inside the laser's 10 nm reach, holds the spot on the tank for the whole 50 s fall (no LASER line leaves Lasing, `blind_s` at most 1) and exits only after the bomb has landed, within 150 ft of the tank, which is destroyed; the laser is not asked to be off afterwards (the second GBU-10 is still armed, so the automatic rule keeps it on). The Su-25's cone holds its nose on the tank and may leave inside 2,000 ft, the bomb following the spot's last point; it also destroys the tank |
| `ai-strike-laser-buddy` | Three runs (slice ST-L). A player's F/A-18D drops a GBU-10 with an AI F-16C as the buddy, given the wing order's call; an AI F-14 with a GBU-10 and no designator, with a player's F-16C lasing as the buddy and the store left to the AI; the same F-14 with nobody lasing | The AI lases, the sight at the release says IN RNG on a buddy's spot with no laser of its own, the bomb guides on aircraft 1's spot and destroys the tank, and the target page would name the lasing pilot `Friendly 2-1`. The F-14 waits at the choice, chooses the bomb once the player's spot is up, never lases or holds, drops on that spot and destroys the tank, the spot never lost. With nobody lasing it says "No laze support, dropping manual" (one NOLAZE line) and drops the bomb by hand as an unguided bomb (slice ST-G), released `Waiting` with no spot owner and within the circle limits of the target |
| `ai-strike-no-laze` | An AI F-14 with two GBU-10 and nobody lasing, at skills 0, 1 and 2, then with two AS-14 laser missiles (slice ST-G) | The bombs: one NOLAZE line `decision=manual reason=no_laze_manual` saying "No laze support, dropping manual", a bomb pass, a RELEASE `Waiting` with no spot owner and a skill aim and lead, an impact within the circle limits plus 150 ft, no lase hold. The missiles: `decision=rtb reason=no_laze_rtb` saying "No laze support, RTB", no pass, no release, PHASE Home |
| `ai-strike-commit-aaa` | The committed-to-release window (John, 2026-10-28; [AI strike](../spec/ai-strike.md), "Committed to the release, 5 s"). An A-4E's forced straight Mk 82 pass on `~QUCOL` from the default 4 nm, 5,000 ft and 450 kt, with `--defenses 2 0 --surface-seed 1`, at skills 2, 0 and 3 for 35 s: the window opens at about t=23.2, a gun's rounds reach the striker at about t=26.6 and the bomb leaves at about t=27.8. The control flies the skill 2 pass for 30 s with the column's nearest ZSU-23 moved 14,000 ft south under the run line (`--place-site 0x5000000a,180,14000`): it fires at about t=21.7, before the window would open | Held runs: a COMMIT line opens with `release_s` 0 to 5; the closing line says `held_defence=yes` with `held_at` inside the window; no PHASE Defend between the opening and the release; a RELEASE inside the window; PHASE Defend within 0.5 s after it. Control: the first PHASE Defend comes in Attack, with no window opened before it and nothing released, so the run breaks as it did before the change. The runs are deterministic (the same lines run after run); surface seeds 3 and 4 put the gun's first rounds just after the release, which is why the seed is fixed |
| `ai-strike-defended` | An F/A-18D's Mk 82 on `~QUCOL` with `--defenses 2 2`, its pass chosen by threat (a pop-up, SAMs cover the column) | Releases inside its circle and defends (an ACTIVITY Defending line). The column's guns can bring it down at the pop-up's low altitude, so a loss is allowed |
| `ai-strike-dive-steep` | An A-4E's Mk 82 dive on `~QUCOL` at every skill (slice ST-T2) | Released in a dive of at least 45 degrees (the inverted roll-in), above the 3,000 ft floor, with a lead on the moving column, within the circle limits |
| `ai-strike-strafe` | An A-10's GAU-8 gun run on the column's lighter vehicle (`--unit 0x50000003`, 150 hit points) at every skill (slice ST-G: the rounds walk onto the target) | At least three bursts, each burst's start offset inside the circle (or the ring), the burst's middle within the circle limits with 150 ft of gun spread in place of the bomb scatter, released under 4,000 ft, and the vehicle hit and hurt (the IMPACT lines' `hits` and `damage` above 0, its hit points down) at every skill. No egress required (the run ends near the target); a loss is allowed |
| `ai-strike-rockets-popup`, `ai-strike-rockets-dive` | An A-4E's LAU-61 pop-up and dive at skill 2 on `~QUCOL` | Rockets released, within the circle limits with 250 ft of rocket spread |
| `ai-strike-snakeye` | An A-4E's Mk 82 Snakeye laydown at skills 1 and 2 | Released under 1,000 ft with a lead on the column, within the circle limits |
| `ai-strike-heavy-level` | A C-130 with an added bay of eight Mk 82 on `~QUFACT`, `--mission-altitude 10000`, starting at 7,000 ft 8 nm out | Released at 9,500 ft or more (the floor is the mission height, not the 5,000 ft base), no lead on the standing factory |
| `ai-strike-offset` | Level and dive Mk 82 passes at skill 2 starting 1,000 and 3,000 ft off the attack line (`--offset`) | The first release on pass 1 (the sideways law converges on the first pass) |
| `ai-strike-lead-dive`, `ai-strike-lead-level` | John's comparison: the same A-4E Mk 82 dive (level) on the moving column `~QUCOL` and the standing factory `~QUFACT` at every skill | A lead on the moving target and none on the standing one, each within the circle limits; the miss and kill of each run read side by side |

The scenarios' parsing and checks have unit tests in `tools/test_battery_ai_strike.py`, and the
battery selection family `ai-strike` (`ai-strike-*`, `ai-sead-*`, `ai-wing-*`, `qmc-strike-*`) is chosen by changes to
`ai.rs`, `guided.rs`, the scenario file, the AI sources (`tore-sim/src/ai`, `tore-world/src/ai_wings`)
and the laser and sight step (`combat/laser.rs`, `combat/live/sighting.rs`).

## Rotor probe

`--rotor-probe` (slice H1) flies each imported powered-lift PT through scripted
requests to the AI's [rotorcraft and jet-lift control](../spec/ai.md#rotorcraft-and-jet-lift-control)
over flat ground at sea level, clean, at the Damper level, and prints one `ROTOR`
line per flight. The flights are `tore_sim::ai::rotor_probe`'s, the same ones the
unit tests in `flight/powered/ai_lift_tests.rs` assert on the synthetic PT fixtures.
The battery scenario `flight-powered-ai-probe` checks the lines against the unit
tests' bands.

| Line | Flight | Band |
| --- | --- | --- |
| `hold calm`, `hold wind15` | A hover held over a point at 200 ft for 60 s after 30 s to settle, calm and in a 15 kt wind across the heading | within 10 ft and 10 ft of height |
| `move500` | The point moved 500 ft diagonally | overshoot under 10 ft, arrives within 10 ft |
| `climb500` | A vertical climb from 20 to 500 ft at 25 ft/s | fastest 23 to 27 ft/s, top under 515 ft, ends within 10 ft |
| `forward <kt> <fpm>` | 40, 80 and 120 kt climbing and descending 1,000 ft/min | climb within 150 ft/min, speed within 5 kt, heading within 3 degrees |
| `descent_turn` | Four seconds banked left onto a slot, then a right turn onto heading 80 slowing from 130 to 60 kt while asked 12 degrees down (about 42 ft/s), 40 s from 3,000 ft (a wingman's formation chase, slice CH47) | the helicopters never fly the collective under 0.1 and the nose stays within 30 degrees of the track (the V-22, with its power lever, is not held to it) |
| `turn200` | 180 degrees at 200 ft asking 160 kt, more than these aircraft make | loses under 50 ft, turns round within 5 degrees in 60 s |
| `slow120` | From 120 kt at 1,000 ft to a hover over a point 3,000 ft ahead | loses under 50 ft, ends within 10 ft hovering |
| `convert` (V-22) | From a hover to airplane mode asking 220 kt, then a hover 15,000 ft ahead | nacelles down and over 200 kt, back over 80 degrees within 10 ft, within 300 ft of height, no corridor override |
| `jet hover` (AV-8, Yak-141) | A hover at 200 ft, then a vertical landing at a 5 ft/s sink | within 10 ft, touchdown under 8 ft/s within 15 ft of the point |

```sh
TORE_DATA_DIR=<a copy of your profile> cargo run --locked -p tore-app -- --rotor-probe --no-audio
```

## Powered-lift scenarios (VTOL overhaul)

`flight-powered-*` flies the six powered-lift aircraft (AV-8, Yak-141, V-22, AH-64,
Mi-24, CH-47) by short scripted pilot-input tapes, from
[`tools/battery_scenarios/_powered.py`](../../tools/battery_scenarios/_powered.py). The
app's `--replay-input TAPE --maneuver hover` puts an aircraft at rest in the air in its own
hover trim (hands off it holds, so a tape starts from a fixed point), and the tapes
(`tore-pilot 3`, written at run time) fly the rest. Each is open loop, so the checks are
bands read from the result line (`speed_kt`, `altitude_ft`, `crashed`, `heading_deg`,
`bank_deg`, `non_finite`); the numbers are in the
[overhaul baseline](../baselines/vtol-overhaul.md) and the physics in the
[spec](../spec/powered-lift-flight.md).

| Scenario | What it flies | What must hold |
| --- | --- | --- |
| `flight-powered-hover-<aircraft>` (all six) | 15 seconds hands off in the trimmed hover (the Yak-141 with 6,000 lb of fuel: full tanks leave it no hover at 5,000 ft) | Speed under 3 kt and height within 30 ft; a rotorcraft also burns 0.1 to 0.9 lb/s (slice HF2: by the power the rotor draws, not the record's 1 to 3 lb/s) |
| `flight-powered-hover-hold-<aircraft>` (AH-64, Mi-24, CH-47, V-22) | A hard stick pulse from the hover, hover hold engaged at 3.5 seconds, and the same pulse without the hold | The held run ends under 3 kt and within 50 ft of its height; the unheld run is drifting at over 15 kt, so the pulse is real. The jets refuse hover hold by design (unit test A2) |
| `flight-powered-transition-<aircraft>` (all six) | Hover to forward flight: forward cyclic trim taps at the Attitude level (helicopters), the conversion keys at 90 percent lever (V-22; 85 before slice HF3 gave the V-22 its real weight), the manual's three nozzle steps and the forward preset (jets) | A speed over each aircraft's minimum, a height inside its band, no crash |
| `flight-powered-corridor-v22` | Airplane-mode start at 175 kt, the nacelles asked toward helicopter mode for 20 seconds | The aircraft keeps its speed and height and never goes over the overspeed limit: the corridor protection moved the nacelles, the pilot did not |
| `flight-powered-autorotation-ah64` | Engine cut at the start speed, lever down, Attitude level | A steady descent of 1,500 to 3,500 ft in 50 seconds at 90 to 160 kt, no crash |
| `flight-powered-vrs-ah64` | A sinking hover, then full collective, with and without Easy flight physics | Without the cheat it sinks at least 100 ft further than with it: the vortex ring state shows |
| `flight-powered-easy-<aircraft>` (all six) | At stability Off, a 30 percent collective step from the hover (helicopters) or a roll pulse (jets), with and without Easy flight physics | The single-rotor helicopters turn the nose over 15 degrees without the cheat and under 5 with it; the jets bank over 15 degrees without it and under 12 with it; the CH-47 and V-22 have no torque and do not turn either way |
| `flight-powered-touchdown-offfield` | Six headless touchdowns from 60 ft over flat open ground with `--maneuver settle:SINK`, which holds the sink rate on the collective: AH-64 at 5, 15 and 25 ft/s, Mi-24 at 5, V-22 at 5 and 15 | The 5 ft/s landings sit undamaged; the 15 and 25 ft/s ones crash (off an airfield every touchdown over the limits is a crash) ([touchdown rule](../spec/touchdown.md)). The jets (a hands-off hover drifts past 10 kt) and fixed-wing off-airfield crashes are in the unit tests and the `land-off-runway` landing probes |

`mp-seat-ah64` (a human taking a rotorcraft seat mid-flight), which the design named, is
not a scenario: the exact-state continuation it relies on is covered by unit tests
(`powered_state_restoration_and_input_replay_continue_exactly` and the per-type restore
tests) and by the wire's round trips. No windowed scenario was added: the HUD cluster is
covered by `powered_hud` unit tests and `--hud-snapshot`, and the moving parts by the
`flight-animation-*` scenarios.

```sh
TORE_BATTERY_PROFILE="$PWD/.local/dev-profile" python3 tools/battery.py --scenario 'flight-variety-*' --profile .local/dev-profile --jobs 4
```

The profile must contain a current runtime import. Rendering cases use the
required agent window wrapper; their captures remain local.

## What it means for the game

Nothing in this lane crashed, produced a NaN, put an aircraft under the ground, let ammunition go
negative or over capacity, or left an aircraft stuck on a runway. Every aircraft takes off from
every airport, lands on every long runway, recovers from a stall, recovers from a spin unless its
data forbids spinning, and burns and runs out of fuel sensibly. The one real breakage was in the
test tools: the headless combat smoke probe had failed for thirteen of fourteen aircraft since
0.1.0 because it still expected rules the game has deliberately moved past. The probe is fixed
and now passes for all fourteen. Several behaviours are not defined anywhere in the specs and are
listed under "Needs a decision" below, with the evidence.

## Running it

```sh
cargo build --locked -p tore-app
python3 tools/battery.py --lane flight --jobs 6 --windows 2 --tag flight
python3 tools/battery.py --scenario 'flight-land-*' --jobs 6              # one family
python3 tools/battery.py --scenario 'flight-livefire-f18-*' --windows 2   # windowed
```

The whole lane is 3,285 scenarios, 396 of them windowed (through `tools/agent-run.sh`).
The headless ones take about a second each; the missile acceptance runs take up to five minutes
each and the windowed ones five to eight seconds. See "Runtime" at the end for the measured time.

## What is covered

| Family | Scenarios | What it checks |
| --- | --- | --- |
| `takeoff-*` | 772 | Ground start and takeoff for all 14 aircraft from the first and middle airport of every base theater, the Hornet from every airport of every base theater and the first airport of every imported variant: reaches 100 ft, no crash, sane liftoff speed and time, a default load no heavier than the aircraft's maximum takeoff weight, and external tanks that feed before the internal fuel. |
| `land-*` | 676 | The scripted approach and landing (see below): the Hornet at every airport of every base theater, every other aircraft at the first airport of every base theater, the Hornet at the first airport of every variant, all 14 with gear up (must crash for the gear), off the runway (must crash for not landing on a runway) and with no flare, and all 14 in five different winds. On runways of 5,500 ft or more: touches down, stops, no crash, no unsafe touchdown, does not leave the runway surface. |
| `level`, `pull`, `loop`, `roll`, `stall`, `spin`, `bank-left`, `bank-right` | 336 | Every manoeuvre in every aircraft on the default, `--legacy-flight` and `--researched-flight` adapters: no non-finite value, fuel never rises, no energy from nothing, load, speed, altitude and rates within limits, no veil without G. |
| `spinrecover-*`, `stallrecover-*` | 28 | The manual's spin and stall recovery procedures recover every aircraft that can enter a spin or stall (X-31 and the F-22 family cannot spin: their data says so). |
| `climb-*`, `sprint-*` | 28 | Climb ceiling behavior and full-afterburner level flight settling near the top speed. Time above the limit, rather than a peak-speed multiplier, determines overspeed failure. |
| `overspeed-*` | 42 | Every aircraft is held above its limit and lost at a 6 to 10 second boundary with cause overspeed; Invulnerable survives; an unattended dive ends in a loss ([overspeed](../spec/overspeed.md)). |
| `lateral-*` | 4 | Unpowered rudder turns the path and banks; low-speed nosewheel steering reaches 90 degrees for F/A-18D and Rafale C. |
| `target-camera-*` | 6 | Close and long-range target window, F7 and F8 captures for visual inspection. |
| `belly-*` | 28 | The gear key at 80 knots on the roll is refused by the ground sensor (one message, gear still down); the same key once airborne is a normal retraction ([gear on the ground](../spec/gear-on-the-ground.md)). The names keep "belly" from the slide they replaced. |
| `fault*` | 73 | Every system fault 0..44 on three aircraft in a pull, all 45 at once, and one after another, in every aircraft. |
| `combatsmoke-*`, `combatevidence-*`, `missileacceptance-*` | 40 | The headless combat smoke (default slots, five damage classes, jettison, radar power, incoming missiles, jammer), the same smoke with per-slot combat tapes written and replayed to the identical state, and the missile reach probes (the F-14 and Su-35 tables are left to the slow set, below). |
| `ac130-pin-orbit`, `ac130-fire-no-target`, `ac130-track-out-of-arc` | 3 | Headless AC-130 gunsight probes (`--gunsight-probe NAME --aircraft ac130`, `crates/tore-app/src/gunsight_probe.rs`) on flat ground with the imported guns, about half a second each. Orbit at 4,000 feet, pin the ground at its centre and fire for ten seconds (impacts within 150 feet of the pin, no barrel faster than 30 degrees a second, guns READY, never NO LINE OF FIRE; `--gunsight-dump PATH` writes the per-tick gun train as CSV). Fire with nothing held in the default view (all three guns fire, impacts around the aim point). Track a ground object past the wing line (CANNOT BEAR, barrels at the arc limit, camera still on the object, trigger still fires). |
| `ac130-orbit-pin`, `ac130-orbit-slew`, `ac130-orbit-track`, `ac130-orbit-close` | 4 | The AC-130's sensor console (slice nr-orbit; `--a2g-run UKR QUCOL --aircraft ac130 --load C_25.JT --attack orbit`, `crates/tore-app/src/a2g_run/orbit.rs`), about 50 s each. From 3 nm at 4,000 ft the console opens on a pin (also slewed) or the tracked lead tank, and nothing touches the stick or throttle: it enters the orbit; after entry never inside the minimum radius or below the minimum height, every gun and the camera bear at least 90% of the time, bank at most 31 degrees, bank rate at most 8 and pitch rate at most 4 degrees a second, no stall; the slewed pin and the moving tank move the point and the orbit follows; Ctrl+9 again at 120 s hands the aircraft back. |
| `ac130-console-search`, `ac130-console-pin-release` | 2 | The console's search and keys (slice nr-cona; the same run with `--orbit-sight free --console-keys T:KEY,...`, `crates/tore-app/src/a2g_run/orbit/console.rs`), about 25 to 30 s each. The console opens searching with nothing pinned; the keys go in as seat commands (`pin`, `heading+N`, `altitude+N`, `speed=P`, `speed+N`). Search: a 30 degree right turn and 2,000 ft climb, then a 90 degree left turn and a descent; each search settles within 1 degree of its heading and stays within 3 (no turn back), and at the end holds altitude within 200 ft, speed within 8 kt and wings within 3 degrees. Pin and release: P at 15 s joins and enters the orbit (90% bearing after entry), P at 240 s rolls out into the search, which settles the same way. Bank rate at most 8 and pitch rate at most 4 degrees a second, no stall. |
| `livefire-*`, `cheat-unlimited-ammo-*`, `cheat-damage-*`, `countermeasures-*` | about 90 | Windowed: fire every weapon slot of every aircraft (ammunition never negative or over capacity, drops by exactly what was fired, other stations untouched, surface weapons refuse the practice aircraft), Unlimited ammo, the three Damage modes, and chaff and flare counts against capacity. |
| `cheat-*` | 70 | Extra G reaches about 9 G, No redout or blackout, No spins, No crashes and Unlimited fuel on every aircraft. |
| `devices-*` | 14 | Gear, flaps, airbrake and hook stay within 0..1, never move against their command and take the aircraft's own deployment time; an aircraft with no hook does not lower one. |
| `eject-*`, `ejectionpose-*` | 70 | The seat and parachute at 5,000 ft and 250 ft in every aircraft; windowed frames of the three ejection poses. |
| `autopilot-*`, `waypoint-*`, `fuelout-*` | 42 | Heading and altitude hold from a 25 degree bank, waypoint steering, and running out of fuel. |
| `climbout-*` | 30 | The scripted leader's takeoff, gear and flaps up (the A-4E and Su-25 hold the flaps a moment for speed), and cruise, for every aircraft alone and for a wing of five from every base theater. |
| `edge-*`, `terrain-*` | 160 | Flying out over each of the four map edges at 20,000 ft for 200 s (still flying), `edge-lost-*` flying on until the aircraft is lost 105 nm past the map with cause out of bounds ([world edge](../spec/world-edge.md)), and a spin or roll at 90 ft over every theater. |
| `weather*-*`, `hour*-*`, `groundstart-*`, `damage-*`, `bay-*` | about 250 | Windowed frames of every weather condition in every base theater, every hour of the day in three theaters, every aircraft at a ground start, and the damage and F-22 bay fixtures, each checked for a blank or flat frame. |
| `loadout-*` | 56 | Round two: every aircraft with `--loadout none` and `--loadout guns` takes off and lands on a long runway like any other and carries less than its default load. |
| `jettison-*` | 38 | Round two, windowed: jettisoning every station of every aircraft empties exactly that station (an internal station, for example the Su-35 slot 5, refuses and still fires), lightens the load, keeps the flight model's carried weight in step (less fuel already burned) and touches no other station. |
| `fight-*`, `attack-*` | 56 | Round two: the player passive, then attacking through its own controls, in a 5 v 5 against ace or average AI from ahead and behind: the debrief's fate, damage, hit points, crash flag and hit and shot counts agree, and the AI invariants hold. |
| `environment-*` | 96 | The wind, air data and turbulence probe in every base theater at 100 and 5,000 ft in three winds. |
| `panel-*`, `panelfault*` | 175 | Every instrument page of every aircraft, and the Systems page under panel faults 1..35. |
| `window-seeker-*` | 8 | The seeker window: the target camera's place in the small and the large layout before a seeker launch, after it (pop-up at the top right, or left of the top-right window), with free slots, and after the landing ([spec](../spec/target-window.md#seeker-window)). |

Also run by hand and found clean: bad or out-of-range command lines (about twenty, all refused
with a message and no panic), extreme winds up to 200 ft/s (the scripted takeoff pilot crashes in the strongest headwinds only because it holds its rotation), starting at 90,000 ft, and every
theater and weather condition through `--validate-weather` and `--validate-maps`.

## The scripted pilots and probes

The headless flight probe (`--headless-flight`) gained the tools this lane needs, all described
in [DEVELOPMENT.md](../DEVELOPMENT.md#headless-flight-checks): an `extremes:` line that a script
reads for impossible states (non-finite values, fuel that rises, energy from nothing, speed
against the envelope, height above the terrain, the G veil), `--flight-trace`, `--flight-fault`,
`--flight-cheat`, `--flight-fuel`, `--flight-start`, and scripted pilots for spin recovery, stall
recovery, an approach and landing (with gear-up, hard and off-runway variants), autopilot,
waypoint, devices, climb, sprint and ejection. They are `fitted` test harnesses (agent decision,
2026-09-28) and are not game behaviour. The scripted landing floats about 1,500 ft past its aim
point, so it is only expected to stop on a runway of about 5,500 ft or more; a tailwind landing
is allowed a 400 ft longer roll.

## Bugs found

| Symptom | Cause | Fix |
| --- | --- | --- |
| `--combat-smoke` failed for 13 of 14 aircraft (only the MiG-29 passed), since 0.1.0. | Not the game: the probe expected rules that have since changed on purpose. Radar power off now allows an unguided radar-missile release (feature matrix, "Uncued launch with the onboard seeker enabled"); surface weapons refuse the practice aircraft (`WrongTarget`); a fixture flying at the player can win the race against a slow gun and collide with it; the A-4E's guns need far more than forty hits to destroy the player fixture. | The probe now checks those rules and names the slot, weapon and reason when it fails (commit 503b580). All fourteen pass and are in the battery. |
| A combat tape replayed with `--replay-combat` drifted its smoke differently from the live run (`TORE_COMBAT_EVIDENCE` smoke: "serialized live-fire replay diverged"). | The host sets the mission wind on the smoke and countermeasures before every step, but a tape does not record it, so the replay drifted them with no wind. The smoke also compared a live state without airfields to a replay that added them, and kept stepping a crashed flight. | The replay is given the theater's wind (`combat_tape.rs`); the smoke replays without airfields and starts its manual-command tape from a fresh flight. All fourteen aircraft roundtrip their tapes and are in the battery as `combatevidence-*` (commit "Give combat tape replays the mission wind and fix the smoke's tape roundtrip"). |
| A full-power climb carried aircraft far past their own ceiling (X-31 to 66,700 ft against 41,000) and they flew on there; the F-22 could hold level flight 6,000 ft above its ceiling. | The manual (p. 90) says the 1 G ceiling is where the air is too thin to lift the weight, but above the polygon the model kept the full 1 G. | Lift above the ceiling now falls with air density (`flight.rs`, fitted, in FLIGHT-MODEL.md); a zoom climb carries an aircraft at most 23 percent past it and it sinks back. Unit test `above_its_ceiling_an_aircraft_cannot_hold_level_flight`. |
| A loaded aircraft at full afterburner in level flight (autopilot altitude hold) sank into the ground near its top speed: F-22, MiG-21, X-31. | Loading divides the G limit, and on the outermost band of the envelope, where only the 1 G row holds, the divided limit is below 1 G. | Inside the envelope the limit never falls below 1 G, in the flight model and in the AI's own G limit (fitted, FLIGHT-MODEL.md). Unit test `a_loaded_aircraft_holds_level_flight_on_the_outer_band_of_its_envelope`. The sprint scenarios now hold altitude. |
| The RCS instrument window drew "NO EXPOSURE DATA" over the 270 bearing label. | Message placed at the left edge of the window. | Moved below the crosshair. The menus lane fixed the same line the same way; that version is the one merged. |

No other game defect was found in takeoff, landing, the manoeuvres, spin and stall recovery,
faults, ejection, devices, autopilot, fuel, weapons accounting, countermeasures, cheats or the
terrain checks. Three problems belong to other lanes and were handed on:

- **AI wingman flies into rising terrain behind a fast leader.** Still present after round two:
  `tore-app --theater UKR --aircraft f22 --ground-start 3 --ai-probe-ticks 9000 --maneuver takeoff --no-audio`
  ends with `HAZARD off landable surface: Friendly 1-2 agl=14 kt=809` at t=8544 and
  `wingman[Dead ...]`. It happens with any wingman type (`--probe-friendly-aircraft f18` too) but
  only with the F-22 as the scripted leader, which cruises fast. The wingman is in formation at
  full throttle, drifts 2,000 ft below the leader (which holds its MSL altitude), reaches about
  800 kt and hits terrain that rises 586 to 1,726 ft over the last 14,000 ft. It is not the
  envelope: the numbers did not change with the round-two flight-model fixes. The AI terrain floor
  (B44, `ai/steering.rs::terrain_pitch_floor`) looks only 1,000 ft ahead, under a second at 800 kt,
  so it cannot start a climb in time, and the formation code lets the wingman sink below its slot.
  Not changed: it is AI behaviour.
- **The debrief credited kills with no recorded hit (fixed 2026-10-09).** With
  `--aircraft faxx --probe-enemy-aircraft su27 --probe-enemy-skill average --ai-probe-ticks 20000 --probe-fight 5:5 --separation 10 --probe-attack 100:8`
  the debrief showed `kills=[1,...] a2a=0/2` while the probe saw one player hit (the Su-35
  showed `a2a=1/4 kills=[2]`). The cause: a flare or chaff spoofed the missile, the ledger closed
  it as Spoofed, and the missile flew on and struck the aircraft anyway. The damage, the kill and
  the last-attacker credit were applied, but the hit could not be tallied because the missile was
  already resolved. The ledger now remembers spoofed missiles and counts a late strike as the
  missile's hit (see [debrief kills](../spec/debrief.md#kills)). Before the fix the same
  mismatch showed in 33 of 186 AI-lane and combat scenarios (every default `ai-fight-NvN`, the
  MiG-29 vs F-14 skill pairs and 10 fuzz seeds) and the two lane fights above; the scenario check
  no longer exempts the Su-27 and Su-35, and `tools/battery_scenarios/_debrief.py` holds every
  AI probe debrief line to it, for the player and the wingman.
- **AI aircraft collide.** In the F-14 rear-geometry fight two AI enemies touch (56 ft apart)
  and both are lost; the AI invariant counts it. The fight scenarios do not count mid-air
  collisions between AI aircraft.

## Stall and liftoff speeds against the imported data

John doubted the F-22, Su-27 and Su-25 numbers on 2026-09-29: at 80 knots they were
inside their lift envelope, and he expected 130 to 150 knots to fly. This was checked
with the headless takeoff (`envelope:` and `liftoff:` lines, sea-level airports, calm
air, the default loadout, full flaps and afterburner, back stick 0.35 held from the
start). The model is doing what the imported data says. Nothing was changed.

**How the model gets its stall speed** (`flight.rs`, hybrid adapter). It takes the left
edge of the aircraft's imported 1 G speed and altitude polygon at the current altitude
(the game's "stall speed limit", manual p. 90). Full flaps lower it 25 percent (the
reviewed flap effect, [takeoff rules](../spec/takeoff-ground-contact.md)). Lift is
`(speed / stall)^2` of one G up to the stall speed. Between the stall speed and the next
G row's left edge the G limit ramps from 1 G up to that row, and loading divides it
(`1 + loading * loadedElevator / 100`, for example 1.35 for the F-22), so a loaded
aircraft gets a full 1 G only above the stall speed. That is the only weight term: the
polygon itself is fixed, and it is not scaled by the square root of the weight. Thrust
contributes its vertical share while the nose is up. There is no wing area or CLmax in
the model: the imported polygon stands for them.

**The imported data.** The sea-level vertex of each 1 G polygon is round in feet per
second: F/A-18D 200 (118.5 kt), Su-27 180 (106.7), Su-25 130 (77.0), F-22 120 (71.1). The
F-22's edge is 120 ft/s, low against the F/A-18D's 200; the game data gives the F-22 the low
figure, and F-22N and F/A-XX share it. The manual has no stall, takeoff or approach figure
for the F-22, Su-27 or Su-25 (its real-aircraft pages give approach speeds only for a few
other types; the 80 to 90 knot "stall speed" on pp. 70 to 71 is the STOVL vector-nozzle
procedure), so the polygons are the only per-aircraft data. The imported landing limit is
195.5 kt for every aircraft (a touchdown limit, not an approach speed).

| Aircraft | Gross lb | 1 G edge (kt) | With flaps (kt) | Min 1 G with flaps and load (kt) | Rotation (kt) | Liftoff (kt) | Liftoff run (ft) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| F/A-18D | 41,838 | 118.5 | 88.9 | 110.1 | 103.1 | 111.9 | 780 |
| Rafale C | 32,092 | 118.5 | 88.9 | 107.6 | 105.3 | 111.6 | 588 |
| F-14D | 64,511 | 100.7 | 75.5 | 88.5 | 85.7 | 91.3 | 607 |
| A-4E | 22,026 | 106.7 | 80.0 | 91.2 | 90.4 | 94.1 | 847 |
| X-31 | 28,620 | 130.4 | 97.8 | 115.0 | 114.2 | 120.5 | 597 |
| MiG-29 | 31,895 | 118.5 | 88.9 | 106.2 | 105.4 | 111.0 | 506 |
| Su-27 | 57,920 | 106.7 | 80.0 | 91.4 | 92.6 | 96.9 | 429 |
| MiG-21 | 18,018 | 118.5 | 88.9 | 97.2 | 96.2 | 100.9 | 662 |
| Su-25 | 69,875 | 77.0 | 57.8 | 70.8 | 68.4 | 73.4 | 513 |
| MiG-23 | 39,413 | 106.7 | 80.0 | 97.4 | 91.4 | 99.4 | 748 |
| Su-35 | 69,054 | 106.7 | 80.0 | 93.1 | 91.9 | 97.1 | 523 |
| F-22, F-22N, F/A-XX | 57,800 | 71.1 | 53.3 | 70.0 | 73.2 | 77.6 | 207 |

Sea level, so calibrated and true airspeed and ground speed are the same here. The
"Min 1 G" column is where the loaded G limit first reaches 1 G, and liftoff follows it
closely. The default loadout is the takeoff weight in the table; the stall speed does not
change with weight, only the loading term does.

**Against John's reference figures** (unsourced real-world numbers, a plausibility range
only): liftoff 135 to 150 kt for the Su-27, 130 to 145 kt for the Su-25 and 130 to 150 kt
for the F-22; approach 120 to 135, 125 to 140 and 135 to 145 kt. The model lifts them off at
97, 73 and 78 kt, 30 to 45 percent lower. The other types sit low the same way (the F/A-18D lifts
off at 112 kt and its 1 G edge is 118.5 kt, against a real approach of about 135 kt), so the
cause is the game's own polygons, whose left edges are below real stall speeds, and not a
fault in one aircraft's handling. The scripted landing's
approach speed is 1.3 times the 1 G edge (F-22 92 kt, Su-25 100, Su-27 139), a probe choice
and not an aircraft figure.

John decided the same day to have the model scale the stall speed with weight,
as a deliberate departure from the polygon data (next section). The
`flight-takeoff-*` scenarios check that liftoff follows the model's own
minimum speed for 1 G.

## Weight-scaled stall speed (2026-09-29)

`opinionated`, requested by John; rules in the
[takeoff spec](../spec/takeoff-ground-contact.md#weight-scaled-stall-speed). The polygon's
slow edges apply at a reference weight and grow with the square root of the weight over
it. The first version used the empty weight for every aircraft. It could not bring every
aircraft within 10 percent of John's figures: no single fraction of the empty weight (0.6
to 1.6 tried) does, because a fraction low enough to help the F-22 family and Su-25 puts the
F/A-18D, Rafale and A-4E 20 to 40 percent too high, and a high one does the reverse. Since
2026-09-30 (`fitted`, an agent decision, John's decisions 1 and 2 of the aircraft pass) each
aircraft has its own fraction of its empty weight, keyed by the aircraft's identity
(`STALL_REFERENCE_FRACTIONS`, `AircraftModel::stall_reference_fraction`, the table in the
[takeoff spec](../spec/takeoff-ground-contact.md#reference-fraction-per-aircraft-fitted)).
A lower fraction means faster liftoff and approach. The X-31, with no figure from John,
keeps 1.00.

Liftoff at the default loadout (calm, sea level, UKR airport 1, full flaps, afterburner,
back stick 0.35), with the imported speeds (`--retail-stall-speeds`), with one reference for
every aircraft (the empty weight, before 2026-09-30) and now, against John's figures where
he gave them:

| Aircraft | Gross lb | Scale now | Liftoff, imported (kt / ft) | Liftoff, one reference (kt / ft) | Liftoff now (kt / ft) | John's liftoff |
| --- | ---: | ---: | ---: | ---: | ---: | :---: |
| F/A-18D | 41,838 | 1.17 | 112 / 780 | 147 / 1,383 | 129 / 1,050 | - |
| Rafale C | 32,092 | 1.17 | 112 / 588 | 149 / 1,056 | 128 / 781 | - |
| F-14D | 64,511 | 1.38 | 91 / 607 | 114 / 948 | 123 / 1,107 | - |
| A-4E | 22,026 | 1.30 | 94 / 847 | 132 / 1,716 | 121 / 1,427 | - |
| X-31 | 28,620 | 1.33 | 121 / 597 | 156 / 1,001 | 156 / 1,001 | - |
| MiG-29 | 31,895 | 1.21 | 111 / 506 | 143 / 851 | 132 / 719 | - |
| Su-27 | 57,920 | 1.37 | 97 / 429 | 120 / 659 | 128 / 757 | 135 to 150 (5 percent low) |
| MiG-21 | 18,018 | 1.39 | 101 / 662 | 118 / 911 | 136 / 1,224 | - |
| Su-25 | 69,875 | 1.74 | 73 / 513 | 93 / 836 | 123 / 1,506 | 130 to 145 (5 percent low) |
| MiG-23 | 39,413 | 1.46 | 99 / 748 | 126 / 1,219 | 142 / 1,552 | - |
| Su-35 | 69,054 | 1.34 | 97 / 523 | 124 / 855 | 127 / 897 | - |
| F-22, F-22N, F/A-XX | 57,800 | 2.14 | 78 / 207 | 103 / 359 | 128 / 559 | 130 to 150 (2 percent low) |

Approach speed of the scripted landing (flaps down, 65 percent internal fuel, stores as
loaded; 1.3 times the flapped, scaled stall speed and never under 1.05 times the loaded
minimum for 1 G, capped at 166 kt, 0.85 times the landing limit) with the imported speeds
(full fuel, 1.3 times the polygon edge), with one reference and now, against the figures
John gave:

| Aircraft | Imported (kt) | One reference (kt) | Now (kt) | Figure (kt) | Off by now |
| --- | ---: | ---: | ---: | :---: | ---: |
| F/A-18D | 154 | 156 | 135 | 135 | in range |
| Rafale C | 154 | 158 | 135 | 130 to 140 | in range |
| F-14D | 131 | 125 | 135 | 130 to 140 | in range |
| A-4E | 139 | 148 | 136 | 130 to 140 | in range |
| X-31 | 166 | 166 | 166 (cap) | (research aircraft) | - |
| MiG-29 | 154 | 154 | 140 | 135 to 145 | in range |
| Su-27 | 139 | 132 | 142 | 120 to 135 | +5 percent |
| MiG-21 | 154 | 137 | 160 | 160 to 170 | in range |
| Su-25 | 100 | 97 | 131 | 125 to 140 | in range |
| MiG-23 | 139 | 134 | 152 | 150 to 165 | in range |
| Su-35 | 139 | 136 | 139 | 135 to 145 | in range |
| F-22, F-22N, F/A-XX | 92 | 102 | 148 | 135 to 145 | +2 percent |

Eight of the eleven figures are inside John's range and the other three are within 5
percent of it, against 2 of 11 with one reference. The F/A-18D's 135.1 kt against a single
figure of 135 counts as in. The Su-27 is the one aircraft the model cannot fit both ways:
John's liftoff (135 to 150 kt) and approach (120 to 135 kt) ranges put its approach at or
below its liftoff, and the scripted approach is about 1.1 times the liftoff at every
fraction, so 0.86 takes the middle (John's decision 2). The nine aircraft with no liftoff
figure are fitted to their approach only, and their liftoff comes out 4 to 15 percent under
it. The battery's windows for these figures are `KNOWN_LIFTOFF` and `KNOWN_APPROACH` in
`tools/battery_scenarios/flight.py`, re-recorded on 2026-09-30 (plus or minus 6 percent
around the recorded value, on top of John's range plus or minus 10 percent).

Each fraction was chosen at least 0.02 clear of the liftoff jumps that appear when the
model's G rows cross at a larger scale (the X-31 between 0.94 and 0.95, the MiG-21 between
0.68 and 0.69 and the Su-25 between 0.52 and 0.53, measured by sweeping every fraction 0.06
either side; the Su-25's 0.55 is the closest). Since the Su-25's record correction
(2026-10-13) its fraction is 0.51 on the corrected empty weight and its jump sits near
0.49; `KNOWN_LIFTOFF` is 120.2 kt and `PULL_G_RETAIL` 5.41 G for it.

**Combat-speed G (correction, 2026-09-29).** The first version scaled every row of the
polygon, which counted the weight twice (the edges, then the loaded-elevator divisor) and left a
fuelled F/A-18D pulling 5.1 G in the held pull at 450 kt and 5,000 ft (7.64 with the imported
speeds; the replay lane's scripted pull peaked at 3.8 G). Now the scale applies to the 0 G, 1 G and
2 G rows, fades linearly to nothing at the 4 G row and leaves the rows from 4 G up as imported
(`row_scale`, `STALL_SCALE_FADE_G`). The stall, liftoff and approach speeds are unchanged (they
come from the 1 G edge and the ramp to the 2 G edge), and above roughly twice the stall speed the G limit is the
imported one; below it the weight still costs G. The divisor stays: removing it would put the
F-22's liftoff back near 80 knots (97 kt for 1 G instead of 74).

Held pull at 450 kt and 5,000 ft (default loadout, `--maneuver pull`, max G reached), and the
G limit at the top row's own speed (corner speed) at 5,000 ft with the same weight
(`--maneuver gcurve`), the first (double counted) version, now, and with `--retail-stall-speeds`:

| Aircraft | Pull before | Pull now | Pull retail | Corner G before | Corner G now | Corner G retail |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| F/A-18D | 5.10 | 7.64 | 7.64 | 5.02 | 7.53 | 7.53 |
| Rafale C | 6.57 | 7.39 | 7.39 | 4.87 | 7.31 | 7.31 |
| F-14D | 6.18 | 6.18 | 6.18 | 4.40 | 6.15 | 6.15 |
| A-4E | 6.10 | 6.13 | 6.13 | 4.29 | 6.01 | 6.01 |
| X-31 | 5.64 | 7.25 | 7.25 | 4.82 | 7.22 | 7.22 |
| MiG-29 | 4.78 | 7.15 | 7.15 | 4.71 | 7.06 | 7.06 |
| Su-27 | 5.25 | 7.54 | 7.54 | 5.18 | 7.77 | 7.77 |
| MiG-21 | 6.16 | 6.47 | 6.47 | n/a | n/a | n/a |
| Su-25 | 4.99 | 4.99 | 4.99 | 4.11 | 4.93 | 4.93 |
| MiG-23 | 5.92 | 6.77 | 6.77 | 4.21 | 6.73 | 6.73 |
| Su-35 | 5.22 | 6.96 | 6.96 | n/a | n/a | n/a |
| F-22 family | 6.14 | 6.91 | 6.91 | 4.56 | 6.84 | 6.84 |

(The MiG-21 and Su-35 have no top row at 5,000 ft, so they have no corner figure there.) The
pull now equals the imported value for every aircraft and the corner G is within 2 percent
of it (the F/A-18D's small difference is the divisor at the moment of measuring); measured again
with the per-aircraft fractions of 2026-09-30, every held pull and every corner G is the same.
The G limit against speed at gross weight
(default loadout, UKR airport 1, calm, 5,000 ft, full back stick), imported speeds / now:

| Speed (kt) | F/A-18D | F-22 | Su-27 |
| ---: | :---: | :---: | :---: |
| 100 | 0.84 / 0.84 | 1.03 / 0.76 | 0.86 / 0.86 |
| 150 | 1.32 / 1.16 | 2.28 / 0.76 | 1.57 / 0.86 |
| 200 | 1.67 / 1.67 | 3.04 / 2.66 | 1.73 / 1.57 |
| 250 | 2.51 / 2.51 | 4.56 / 4.56 | 3.45 / 3.45 |
| 300 | 3.35 / 3.35 | 6.08 / 6.08 | 4.32 / 4.32 |
| 350 | 5.02 / 5.02 | 6.84 / 6.84 | 5.18 / 5.18 |
| 400 | 6.70 / 6.70 | 6.84 / 6.84 | 6.91 / 6.91 |
| 450 and up | 7.53 / 7.53 | 6.84 / 6.84 | 7.77 / 7.77 |

So the weight now costs G only at the slow speeds where the low G rows lie (up to about 200 to
250 kt), where a heavy aircraft really is lift-limited, and nothing at combat speed. With the
per-aircraft fractions the clean aircraft (no flaps, 5,000 ft, default load) first pulls 1 G at
150 kt (F/A-18D, Rafale, A-4E, Su-25), 175 kt (F-14D, MiG-29, Su-27, MiG-23, Su-35) or 200 kt
(X-31, MiG-21, F-22 family), in 25 kt steps, against 100 to 150 kt with the imported speeds: the
F-22 cannot hold level flight clean below about 200 kt, the direct cost of lifting off at 128 kt
(the flaps lower this by a quarter).

**Short strips.** The roll to liftoff at full afterburner is longer for every aircraft, most
for the heavy loadouts and for the ones whose fraction is under 1. From the Santa Fe start
(`--theater APA --ground-start 3`, a strip of about 1,074 ft, 1,020 ft of paved run from a solo
start slot; San Carlos is the same length) the liftoff runs are, with the imported speeds and now:
F/A-18D 823 to 1,098 ft, Rafale C 620 to 816, F-14D 650 to 1,164, A-4E 908 to 1,507, X-31 626 to
1,040, MiG-29 532 to 751, Su-27 454 to 790, MiG-21 702 to 1,278, Su-25 560 to 1,588, MiG-23 796 to
1,622, Su-35 555 to 940 and the F-22 family 222 to 584. So the F/A-18D, F-14D, A-4E, X-31, MiG-21,
Su-25 and MiG-23 no longer lift off within the paved strip (they roll on past its end); the
Rafale, MiG-29, Su-27, Su-35 and F-22 family still do. The AI takeoffs on those strips were already
known failures (lane-ai item 11). Since 2026-09-30 a Quick Mission never starts on such a strip
(lane-ai item 11); these developer `--headless-flight --ground-start` runs, which measure the
flight model and not a mission, still start there. Landing rollouts shorten slightly (the flare
from 60 ft touches down nearer the threshold), so the 5,532 ft UKR runway still works for all twelve.

**Other users of the stall speed.** The flight model, the stall warning and departure
behaviour, the autopilot, the ejection G check, the flight envelope window (the drawn slow
edges follow the weight), the scripted landing probe and the crew callouts all
read the scaled speeds. The HUD landing-speed brackets are not implemented in the game, so
there is nothing to update. The AI reads the same scaled speeds through the model's
configuration without any change to `tore-sim/src/ai`. The AI follow-up this rule needed is done: the AI lane retuned its speed
limits during the bug bash (`4e47944`, `0057684`: the minimum never below the 1 G level-flight
speed, the maximum never above the fastest row that leaves 1.2 G, a final lift margin of 1.15 G;
the twelve scenarios that failed first are listed in the
[battery report](../baselines/battery-2026-09-28.md), section 2). With the per-aircraft stall
references of the aircraft pass (2026-09-30) the whole AI lane passes. AI takeoffs are
unaffected (the roll only lasts longer).

## Needs a decision

None of these is defined in the specs, the manual text or the feature matrix, so none was changed.

1. **Overspeed** (decided). John requested the current five-second safe period,
   seeded 25 percent rolls at 6 to 9 seconds, and guaranteed loss at 10 seconds
   on 2026-10-01. The fixture holds 1.1 times the current limit to isolate timing;
   it is not a natural acceleration test. [Behavior](../spec/overspeed.md).

2. **The autopilot's altitude hold outside the envelope.** Beyond the top speed the aircraft cannot
   hold 1 G and sinks; the autopilot spec says "no guaranteed hold outside the flight envelope
   is implied", so this is as written, and inside the envelope the hold now works (see "Bugs
   found"). Overspeed now has a rule (item 1); the hold outside the envelope is unchanged.
3. **The map edge** (decided). John asked on 2026-09-29 for a warning at 100 nautical miles past the map
   and a loss at 105, AI aircraft lost without a kill ([world edge](../spec/world-edge.md),
   `opinionated`, distances are agent decisions). The `flight-edge-*` scenarios fly 200 seconds out
   and must stay flying; `flight-edge-lost-*` fly on until the loss.
4. **A ground-start airport is a flat square of 5,000 to 8,000 ft a side and all of it counts as
   runway.** The footprint (`footprint_half_ft` in the `landing_start:` line, for example 2,604
   by 3,000 ft either side of the centre at UKR airport 1) is the airport shape's bounds. A
   touchdown 1,300 ft to the side of the paved runway is graded as a normal landing, a takeoff
   roll can run past the runway's end on the flat square, and rolling off the paved strip is never
   penalised. The airport spec calls this "fitted contact" and says terrain outside is unchanged,
   so it is by design; whether a narrower landing area is wanted is a decision.
5. **The small airstrips (about 1,000 ft) accept any aircraft.** Changed (John, 2026-09-30): a Quick Mission ground start is no longer offered on a runway under 2,000 ft (the 22 strips), so no aircraft or wing starts there. Round two had searched the specs, the airport spec and the manual for a minimum runway length and found none, so the rule is John's, not retail's ([short strips](../spec/airports.md#short-strips)).
6. **Combat tapes do not record everything the host does.** A replayed tape shows the player alive
   after a flight that crashed, because the host turns a crashed flight into a dead player and the
   tape does not record that. Only the developer replay uses tapes; the mission recordings of
   [REPLAYS.md](../REPLAYS.md) are separate.
7. **Carrier hook and arresting gear.** The hook lowers on the five aircraft that have one (F/A-18D,
   F-14D, A-4E, F-22N and the F/A-XX concept) and does nothing else; carriers are listed as
   remaining work in the feature matrix.

## Needs a human eye or ear

- The `weather*` frames: the cloudy and foggy conditions are a solid white at the 5,000 ft
  start, with no ground visible at all. Check that being inside the cloud layer is what the
  retail game shows.
- The `hour*` frames of the day: hour 07 has a dawn glow, hours 08 to 19 are full day and night
  starts at 20; check the sunset transition looks right around 19 to 20.
- The `damage-*`, `ejectionpose-*`, `groundstart-*` and `bay-*` frames: the checks only reject
  blank pictures. The F-22 bay frames are taken from a distance where the doors are hard to see.
- Captures made with `--flight-devices` show "RADAR OFF" on the radar window because the capture
  path skips the combat service; live flight shows the scope. Cosmetic, in a diagnostic mode only.
- Aural checks (stall horn, RWR tones, gun and missile sounds) belong to the replay and sound work.

## The slow set and the missile acceptance tables

`--missile-sag` is a headless motion regression using imported AIM-7, AIM-9M,
AIM-120 and Sea Sparrow records, a continuously known stationary target, and
flat ground.
It reports the swept closest approach before the first pass, ground contact or
expiry, alongside the terrain-free fly-out prediction and coasting sag ticks.
Target heights are 300 and 10,000 ft AGL. Aircraft launches inherit 600 ft/s at
target height; the SAM starts stationary on a 20 ft rail aimed at the target.
This isolates motion, not seeker acquisition, launch permission or damage odds.
The `flight-missile-sag` battery scenario checks the coasting long-range rows.
See the [sag validation](../baselines/missile-sag.md) for before/after measurements.

`--missile-acceptance` prints one row per weapon, mode, launch motion, target motion and range
(hit, expiry or an inhibited shot). Every aircraft was run to completion in an optimized build
(`cargo build --release -p tore-app`, each run a few minutes; the F-14's long-range AIM-54 rows
take far longer) and every row was checked: a hit has a positive time, a sane average speed
(100 to 5,500 ft/s) and a travel that matches the target's motion (about the range for a stationary
target, less for an approaching one, more for a receding one); an inhibited shot never flies; the
only outcomes are hit, expiry and inhibit. No impossible row was found in any table. Patterns
worth knowing: every infrared missile has a block of inhibited rows (the seeker is still
acquiring when a cued shot is asked for) and eight boresight expiries (a blind shot at an
approaching target from a climbing or slipping launcher); the F-22A has ten more AIM-120
boresight expiries against a crossing target at 75 and 100 percent range than the F-22N and the
F/A-XX, which fits the F-22A's bay doors adding a second to the launch.

The slow set is off by default (the default battery leaves out the F-14 and Su-35 tables, which
take about 40 minutes each in a debug build):

```sh
CARGO_TARGET_DIR=target-release cargo build --release --locked -p tore-app
TORE_BATTERY_SLOW=1 python3 tools/battery.py --scenario 'flight-slow-*' \
    --bin target-release/release/tore-app --jobs 4
```

## Not covered

RWR reaction to AI radars and missiles, and system faults appearing on the panels during an AI
fight, have no headless or scripted path (the AI probe does not feed the player's RWR or panel
faults, and `--dummy-aircraft` cannot be combined with `--live-fire`), so they were not tested in
round two; the windowed `cheat-damage-*` scenarios cover system faults from incoming missiles and
the `--combat-command` sequences cover designation. Radar and infrared channel switching and
target cycling with several targets in view are exercised only by the probe-attack leader
(`attack-*`), not by a scripted key sequence.

## Render captures

`render-night-light-slots` is a CPU-only exception: it runs
`--night-light-slots` without a window or GPU. The synthetic regression checks
240 frames with eight sustained fires and up to 64 remote muzzle/impact
flashes, through both light budgets and the camera-relative upload. Run it
with `--scenario render-night-light-slots --no-unit-tests --with-windows no`.
The emission and point-light rules are in [night glow](../spec/explosions.md#night-glow).

`python3 tools/battery.py --scenario 'render-*' --windows 1` runs windowed captures
(through the wrapper, one window at a time) that reproduce drawing faults.
`render-ah64-stub-wing-pylons` and `render-ah64-stub-wing-at12` use the CPU stores
audit to check that both Apache hanger rows meet the low stub wing rather than the rotor disk. They
check actual rendered geometry and need an imported profile, with no window.

The two `render-capture-*-deterministic` cases repeat a bare flight or terrain
capture three times and compare complete PPM bytes. They fix launch time,
disable controllers and keep focus-related pause presentation stable.
The art inspection cases below check for blank frames; look at their PPMs:

| Scenario | What it shows | What to look for |
| --- | --- | --- |
| `render-f14-roots-top`, `render-f14-roots-side` | Fully swept F-14, top and side | Wing/tail attachments are symmetric, and the wing sits close to the outer root under the fixed glove. |
| `render-f14-exhaust-top` | Top view with afterburners | Exhaust collars and plume roots have equal widths and engine centerlines. |
| `render-f14-rear-belly` | Rear underside with afterburners | Aft belly and tail fairing seams are closed; no sky shows through. |
| `render-f14-nose-belly`, `render-f14-devices-belly` | Forward underside with gear/brakes stowed and deployed | Nose side and under-cockpit seams are closed; intakes and device movement remain visible. |
| `render-f14-hook-*` | Deployed hook from both sides, half retracted and stowed | The F-22N striped shank and hooked end keep their cutout outline. Retraction stays attached and rigid; zero extension is hidden. |
| `render-nosewheel-hud-inactive` | F/A-18 takeoff roll past the steering cutoff | NSW authority disappears from the HUD. |
| `render-mig21-upper-skin` | The MiG-21 in free flight, flight view 2, look `0,-12`, `TORE_WEATHER_TIME=11:00` | The upper wing is plain camouflage: no grey underside shape shows through it. |
| `render-runway-paint-calm` | The F/A-18 on a UKR ground start (airport 1), takeoff maneuver, 1,300 probe ticks, flight view 1, `TORE_WEATHER_TIME=07:15`, `TORE_WIND=0,0` | The runway paint and joints are whole, with no flicker or z-fighting. |

The synthetic GPU checks run separately with
`cargo test --locked -p tore-app gpu_airport_ -- --ignored`. They test that
pavement never hides an aircraft above it and that paint over pavement holds at
distant and moving cameras.

## Headless animation regressions

`flight-animation-*` scenarios run the actual drawing-path geometry probe for
individually reviewed aircraft, without opening a window. Combined rotor, flap/roll and
nozzle sweeps are required where their aircraft contracts specify them. They require reviewed
attachment scope and passing per-control gates, including signed motion,
root/skin coherence, wheel rigidity/separation, intermediate topology and hook
closing direction. They write user-owned OBJ and CPU contact-sheet evidence
under the ignored battery output directory. Apache and Chinook scenarios also
check the loaded fixed-gear state and ignored retraction commands before the
mesh sweep. Multi-axis artifacts are required separately, including both V-22
conversion/rotor and flaperon sweeps and all 125 Rafale control combinations.
F-14 isolated controls use the existing unswept 400-knot threshold; separate
coupled sweep/flap/roll and pitch/roll grids cover its operational interactions.
Its hook also samples positive near-stow before hiding. A-4E adds 202 samples
each for gear, hook and brake, plus coupled flap/roll and pitch/roll checks.
The original stowed A-4E hook stays visible at zero. Exact selection keys
keep F/A-XX separate from the F22N source donor in reports and scenario matching.

Run one repaired aircraft before expanding the audit:

```sh
python3 tools/battery.py --scenario flight-animation-a7 --profile .local/dev-profile
```

Expand the roster in `tools/battery_scenarios/animation.py` only after independent
source and pose-sheet review. This regression family does not replace that
review, validate GPU appearance, or establish retail animation parity. Current
coverage and remaining profiles are in the
[aircraft animation audit](../baselines/aircraft-animations.md).

## Runtime

The full lane, 3,089 scenarios with `--jobs 6 --windows 2`, took 1,804 seconds (30 minutes) on the
24-thread dev machine with other agents running (load average about 24). Round two's run had one
failure, the jettison check of the Su-35's internal slot 5, which was the check's mistake and is
fixed; that family passes 38 of 38, and the rest passed. The missile acceptance
runs (about five minutes each in a debug build) and the windowed frames set the length.

The `render-night-effect-lights` scenario renders fires of 24, 90 and 160 feet
beside a burning flare and bomb fireballs before and after their initial flash.
It checks capture completion; inspect the PNGs for lighting appearance.
`tore-app` tests assert fire size, linear fade, fixed source height and sustained
fireball light lifetime. See [capture commands](../DEVELOPMENT.md#explosion-inspection).


### Theater ground colours

`render-terrain-colors` is a headless scenario, with no window or GPU. It
compares the live terrain loader's ground palette with the family read from
each retail base MM at all six weather choices and three altitudes. It prints
mean land-cell RGB for each comparison and fails on any palette mismatch.
[Diagnostic and optional offscreen captures](../DEVELOPMENT.md#ground-colour-regression).

## Human air target selection

`flight-sensor-target-select` flies a four-against-four Quick Mission through
T, Shift+T, radar off, Enter and Apostrophe. `net-window-target-select` repeats
those inputs on a private server with a bot, including opening and closing
chat. Each saves before/after HUD and radar captures and checks the next key's
authoritative selection to verify host acceptance and visual retention.
The multiplayer fixture starts enemies 20 nmi out, uses their existing dummy
flight mode and waits for host tick 6,000 before visual selection. Captures can
block the client long enough to reset its network baseline, so the script
waits 0.5 seconds after each shot for fresh contact lists. This avoids treating
snapshot recovery as a target-selection regression.

Run both with `python3 tools/battery.py --scenario '*target-select' --jobs 4
--windows 1 --profile <private-import> --keep-data`. The battery wraps windows
with `tools/agent-run.sh`. See the
[target-selection specification](../spec/radar.md#target-selection-keys).
