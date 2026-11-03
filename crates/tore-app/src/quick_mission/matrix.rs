//! Headless sweep of the Quick Mission creator (`--validate-creator`).
//!
//! Every dropdown value, theater, player aircraft, weather choice, start mode,
//! runway, separation, wing size, wing aircraft, wing skill and group order is
//! driven through the same steps a flown mission takes: the creator's own
//! refusal check, the wing launch, the ground layout, the layout plan, the
//! altitude check, the combat build, the flight restart and the AI wing build.
//! A setup must either start with sane positions or be refused with a message;
//! an error the flight start would treat as fatal, a panic or an impossible
//! position is a problem. Nothing here opens a window or reads a display.
use super::*;
use crate::mission_layout::{GroundLayout, MissionLayout, place_on_runway};
use crate::{AppResult, ai_wings, aircraft::Airframe, combat};
use std::collections::BTreeMap;
use tore_formats::aircraft::AircraftId;

/// What one setup did when started.
enum Outcome {
    /// The creator or the start refused with a message shown to the player.
    Refused(String),
    /// The start hit an error the game would treat as fatal.
    Fatal(String),
    Started,
}

struct Matrix<'a> {
    data: &'a BTreeMap<String, Vec<u8>>,
    airframes: BTreeMap<&'static str, Airframe>,
    problems: Vec<String>,
    started: usize,
    refused: usize,
    refusals: BTreeMap<String, (usize, Vec<String>)>,
    clock: std::time::Instant,
}

impl Matrix<'_> {
    fn airframe(&mut self, id: AircraftId) -> AppResult<&Airframe> {
        let key = id.selection_key();
        if !self.airframes.contains_key(key) {
            self.airframes.insert(key, Airframe::load(self.data, id)?);
        }
        Ok(&self.airframes[key])
    }

    fn describe(quick: &QuickMission, world: &Terrain) -> String {
        let v = &quick.draft.values;
        format!(
            "theater={} player={} weather={} alt={} sep={} start={}/{} enemy-start={}/{} wings=[{}x{}:{}, {}x{}:{}, {}x{}:{} | {}x{}:{}, {}x{}:{}, {}x{}:{}] orders={:?} ai={} guns_only={}",
            world.layout,
            quick.aircraft_files.get(v[6]).map_or("?", String::as_str),
            v[15],
            v[14],
            v[17],
            v[33],
            v[34],
            v[ENEMY_START],
            v[ENEMY_AIRPORT],
            v[4],
            v[5],
            v[6],
            v[7],
            v[8],
            v[9],
            v[10],
            v[11],
            v[12],
            v[21],
            v[22],
            v[23],
            v[24],
            v[25],
            v[26],
            v[27],
            v[28],
            v[29],
            quick.group_objectives,
            quick.ai_mission,
            quick.guns_only(),
        )
    }

    /// The flown-mission start, step by step.
    fn start(&mut self, quick: &QuickMission, world: &Terrain, full: bool) -> Outcome {
        if let Some(message) = quick.unsupported() {
            return Outcome::Refused(message);
        }
        let id = quick.player().expect("checked by unsupported");
        let data = self.data;
        let hornet = match self.airframe(id) {
            Ok(h) => h,
            Err(e) => return Outcome::Fatal(format!("aircraft load: {e}")),
        };
        let load = match tore_sim::combat::loadout::Loadout::new(&hornet.profile, |name| {
            data.get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing loadout resource {name}")))
        }) {
            Ok(l) => l,
            Err(e) => return Outcome::Fatal(format!("loadout: {e}")),
        };
        let altitude = [5000., 10000., 20000., 40000.][quick.draft.values[14]];
        // A spec the creator makes must be one the text form carries: the
        // ground target, defenses and enemy nationality included.
        match quick.mission_spec() {
            Ok(spec) => {
                if let Err(e) = spec.validate() {
                    return Outcome::Fatal(format!("the spec is invalid: {e}"));
                }
                // The text form takes the sixteen theaters; a developer layout
                // is not hosted (the lobby refuses it).
                match MissionSpec::from_text(&spec.to_text()) {
                    _ if spec.theater.starts_with('~') => {}
                    Ok(read) if read == spec => {}
                    Ok(_) => {
                        return Outcome::Fatal("the spec's text reads back differently".into());
                    }
                    Err(e) => return Outcome::Fatal(format!("the spec's text is refused: {e}")),
                }
            }
            Err(e) => return Outcome::Fatal(format!("no spec: {e}")),
        }
        // `start ground auto`: the runway the world picks (the same call the
        // mission build makes).
        let ground_object = match (quick.ground_runway(), quick.ground_auto()) {
            (Some(object), _) => Some(object),
            (None, true) => {
                match crate::mission_layout::auto_runway(world, quick.player_wing_size()) {
                    Ok(object) => Some(object),
                    Err(e) => return Outcome::Refused(e.to_string()),
                }
            }
            _ => None,
        };
        let wings = match quick.wing_launches(None) {
            Ok(w) => w,
            Err(e) => return Outcome::Refused(e.to_string()),
        };
        let parked = quick.player_wing_size();
        let mut start = hornet.start(world);
        let ground_layout = match ground_object {
            Some(object) => {
                let result = start
                    .enable_research(1)
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        crate::mission_layout::ground_layout(world, object, parked)
                            .map_err(|e| e.to_string())
                    })
                    .and_then(|layout| {
                        place_on_runway(world, &mut start, &layout, 0)
                            .map(|()| layout)
                            .map_err(|e| e.to_string())
                    });
                match result {
                    Ok(layout) => Some(layout),
                    Err(message) => return Outcome::Refused(message),
                }
            }
            None => None,
        };
        let layout = MissionLayout::plan(
            world,
            &start,
            ground_layout.clone(),
            &wings,
            quick.separation_feet(),
            quick.situation(),
        );
        if let Some(problem) = tore_world::mission_layout::availability::surface_problem(
            &world.surface,
            quick.separation_nm() as u32,
            ground_layout.is_none(),
            false,
        ) {
            return Outcome::Refused(problem);
        }
        if layout.enemy.shortened() {
            return Outcome::Refused("This separation does not fit the theater and start.".into());
        }
        let ground = f64::from(world.height(start.position[0] as f32, start.position[2] as f32));
        let airborne_wings = wings.iter().any(|wing| {
            !wing.is_empty()
                && (layout.ground.is_none() || wing.wing.side.is_enemy() || wing.wing.index != 0)
        });
        if (ground_object.is_none() || airborne_wings) && altitude < ground + 100. {
            return Outcome::Refused(format!(
                "Airborne altitude must exceed {:.0} feet here. Choose a higher altitude.",
                ground + 100.
            ));
        }
        if !full {
            return Outcome::Started;
        }
        // As a flown launch builds it: combat, then the art that draws it.
        let (mut combat, mut view) =
            match combat::Combat::with_loadout(hornet, &load).and_then(|c| {
                let view = crate::combat_view::CombatView::new(&c, c.own_id(), data)?;
                Ok((c, view))
            }) {
                Ok(built) => built,
                Err(e) => return Outcome::Fatal(format!("combat build: {e}")),
            };
        if let Err(e) = combat.add_airport_targets(&world.airport_scene) {
            return Outcome::Fatal(format!("airport targets: {e}"));
        }
        if let Err(e) = view.mission_aircraft(&mut combat, &wings, &layout, data) {
            return Outcome::Refused(e.to_string());
        }
        // FreeFlight: the same start again, then the reset and the AI build.
        let mut flight = hornet.start(world);
        flight.position[1] = altitude;
        flight.fuel = load.fuel_lbs;
        if layout.player_turn != 0. {
            flight.yaw += layout.player_turn;
        }
        let parked_layout = layout.ground.clone();
        if ground_object.is_some() {
            let Some(ground) = &parked_layout else {
                return Outcome::Fatal("ground start lost its layout".into());
            };
            if let Err(e) = flight.enable_research(1) {
                return Outcome::Fatal(format!("research flight: {e}"));
            }
            flight.position[0] = ground.slots[0][0];
            flight.position[2] = ground.slots[0][2];
            flight.yaw = ground.heading;
        }
        if let Err(e) = combat.reset(&mut flight) {
            return Outcome::Fatal(format!("combat reset: {e}"));
        }
        combat.raise_airborne_spawns(world);
        combat.apply_startup_weapons();
        if let Some(ground) = &parked_layout
            && let Err(e) = place_on_runway(world, &mut flight, ground, 0)
        {
            return Outcome::Fatal(format!("runway placement at flight start: {e}"));
        }
        let airfields = ai_wings::Airfields::from_world(
            world,
            parked_layout.as_ref().map(GroundLayout::departure),
        );
        let mut bridge = match ai_wings::AiWings::build_mission(
            &wings,
            &combat.state.targets,
            quick.guns_only(),
            data,
            &airfields,
        ) {
            Ok(b) => b,
            Err(e) => return Outcome::Fatal(format!("AI wing build: {e}")),
        };
        bridge.apply_mission_preset(quick.ai_mission, flight.position);
        bridge.apply_group_objectives(&quick.group_objectives, flight.position);
        bridge.apply_group_survival(&quick.group_must_survive);
        bridge.mirror_pose_out(&mut combat.state.targets);
        if let Some(problem) =
            check_scene(world, quick, &flight, &combat, &bridge, &wings, altitude)
        {
            return Outcome::Fatal(problem);
        }
        Outcome::Started
    }

    fn stage(&self, name: &str) {
        println!(
            "creator matrix: {name} ({} started, {} refused, {} problems so far, {:.0}s)",
            self.started,
            self.refused,
            self.problems.len(),
            self.clock.elapsed().as_secs_f64()
        );
    }

    /// A setup through the whole start.
    fn run(&mut self, quick: &QuickMission, world: &Terrain, what: &str) {
        self.go(quick, world, what, true);
    }

    /// A setup through the creator's checks and layout only, which is quick.
    fn plan(&mut self, quick: &QuickMission, world: &Terrain, what: &str) {
        self.go(quick, world, what, false);
    }

    fn go(&mut self, quick: &QuickMission, world: &Terrain, what: &str, full: bool) {
        let described = Self::describe(quick, world);
        if let Some(id) = (3..quick.draft.values.len()).find(|id| {
            !(*id == 34
                && (!quick.ground_start()
                    || quick.airport_names[quick.draft.values[13]].is_empty()))
                && !(*id == ENEMY_AIRPORT
                    && (!quick.enemy_ground_start()
                        || quick.enemy_airport_names[quick.draft.values[13]].is_empty()))
                && matches!(quick.value(*id).as_str(), "Unavailable" | "")
        }) {
            self.problems
                .push(format!("{what}: field {id} shows no label: {described}"));
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.start(quick, world, full)
        }));
        match result {
            Ok(Outcome::Started) => self.started += 1,
            Ok(Outcome::Refused(message)) => {
                self.refused += 1;
                if message.trim().is_empty() {
                    self.problems
                        .push(format!("{what}: empty refusal: {described}"));
                }
                let entry = self.refusals.entry(message).or_default();
                entry.0 += 1;
                if entry.1.len() < 4 {
                    let short = described
                        .split(" wings=")
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    entry.1.push(short);
                }
            }
            Ok(Outcome::Fatal(message)) => {
                self.problems
                    .push(format!("{what}: {message}: {described}"));
            }
            Err(_) => self.problems.push(format!("{what}: PANIC: {described}")),
        }
    }
}

/// Positions and populations that must hold after a start.
fn check_scene(
    world: &Terrain,
    quick: &QuickMission,
    flight: &tore_sim::flight::State,
    combat: &combat::Combat,
    bridge: &ai_wings::AiWings,
    wings: &[WingLaunch],
    altitude: f64,
) -> Option<String> {
    let bounds = crate::mission_layout::map_bounds(world);
    let inside = |x: f64, z: f64| {
        (bounds.min[0]..=bounds.max[0]).contains(&x) && (bounds.min[1]..=bounds.max[1]).contains(&z)
    };
    let p = flight.position;
    if p.iter().any(|v| !v.is_finite()) || !inside(p[0], p[2]) {
        return Some(format!("player start off the map or not finite at {p:?}"));
    }
    let floor = f64::from(world.height(p[0] as f32, p[2] as f32));
    if p[1] < floor - 1. {
        return Some(format!(
            "player start under the ground: y={:.0} floor={floor:.0}",
            p[1]
        ));
    }
    if !quick.ground_start() && (p[1] - altitude).abs() > 1. {
        return Some(format!(
            "airborne start at {:.0} ft, chose {altitude:.0}",
            p[1]
        ));
    }
    let expected: usize = wings.iter().map(|w| w.members.len()).sum();
    if bridge.len() != expected {
        return Some(format!(
            "AI wings hold {} aircraft, the setup asks for {expected}",
            bridge.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for t in &combat.state.targets {
        if t.role != tore_sim::combat::missiles::TargetRole::Aircraft {
            continue;
        }
        if !seen.insert(t.id) {
            return Some(format!("duplicate target id {}", t.id));
        }
        if t.position.iter().any(|v| !v.is_finite()) || !inside(t.position[0], t.position[2]) {
            return Some(format!(
                "aircraft {} starts off the map: {:?}",
                t.id, t.position
            ));
        }
        let floor = f64::from(world.height(t.position[0] as f32, t.position[2] as f32));
        if t.airborne && t.position[1] < floor {
            return Some(format!(
                "aircraft {} starts under the ground: {:?}",
                t.id, t.position
            ));
        }
        if t.hp <= 0 {
            return Some(format!("aircraft {} starts dead", t.id));
        }
    }
    None
}

/// Runs the whole sweep and fails, listing every problem, if any setup broke.
pub fn validate(data: &BTreeMap<String, Vec<u8>>, options: Options) -> AppResult<()> {
    let mut m = Matrix {
        data,
        airframes: BTreeMap::new(),
        problems: Vec::new(),
        started: 0,
        refused: 0,
        refusals: BTreeMap::new(),
        clock: std::time::Instant::now(),
    };
    let mut quick = QuickMission::new(AircraftId::F18, options, data);
    let selectable = quick.aircraft_files.len();
    // The player's theater list is the sixteen base theaters and nothing else.
    let listed = quick.options.fields[13].len();
    if listed != 16 || quick.theater_codes.iter().any(|c| c.starts_with('~')) {
        m.problems.push(format!(
            "the theater list offers {listed} entries, expected the 16 base theaters"
        ));
    }
    // The sweep below also covers the imported `~` layout variants, added the
    // way `--theater ~CODE` adds one; players never see them.
    quick.add_developer_theaters(data, None);
    println!(
        "creator matrix: {selectable} player aircraft, {listed} listed theaters, {} layouts",
        quick.theater_codes.len()
    );

    // Field tables: every dropdown has values and the default is one of them.
    for id in 0..quick.draft.values.len() {
        let len = quick.values(id).len();
        let value = quick.draft.values[id];
        // Fields 0 to 2 are not editable rows (the retail setup never shows them).
        if id <= 2 {
            continue;
        }
        if len == 0 && id != 34 && id != ENEMY_AIRPORT {
            m.problems
                .push(format!("field {id} has no dropdown values"));
        } else if len > 0 && value >= len {
            m.problems.push(format!(
                "field {id} default {value} is outside its {len} values"
            ));
        }
    }
    // Each theater's ground target list is the retail template list, entry for
    // entry, and the defense strengths are the creator's four words.
    for (index, templates) in tore_formats::quick_template::tables::TEMPLATES
        .iter()
        .enumerate()
    {
        let code = tore_formats::quick_template::tables::THEATERS[index];
        let listed = quick.options.targets[index].len();
        if listed != templates.len() {
            m.problems.push(format!(
                "{code}: the creator lists {listed} ground targets, the templates {}",
                templates.len()
            ));
        }
        if tore_world::mission::ground_targets(code).len() + 1 != listed {
            m.problems
                .push(format!("{code}: the spec offers a different target count"));
        }
    }
    let words = |id: usize| -> Vec<String> {
        quick.options.fields[id]
            .iter()
            .map(|word| word.trim().to_lowercase())
            .collect()
    };
    for id in [31, 32] {
        if words(id) != ["not", "lightly", "moderately", "heavily"] {
            m.problems.push(format!(
                "field {id} reads {:?}, expected the retail strengths",
                quick.options.fields[id]
            ));
        }
    }
    for group in 0..OBJECTIVE_COUNT {
        if QuickMission::objective_choices(group).len() < 3 {
            m.problems.push(format!("group {group} has too few orders"));
        }
    }

    m.stage("start of theater sweep");
    // Loop 1: every theater layout, F/A-18D, airborne at every separation and
    // altitude, then a ground start on every runway with wings of one to five.
    for code in quick.theater_codes.clone() {
        let world = match crate::scenery::launch_terrain(data, &code, None) {
            Ok(w) => w,
            Err(e) => {
                m.problems
                    .push(format!("theater {code} would not load: {e}"));
                continue;
            }
        };
        let index = quick.theater_codes.iter().position(|c| *c == code).unwrap();
        quick.apply(13, index);
        quick.apply(33, 0);
        quick.apply(4, 2);
        // The ground-start list is the player's side's and neutral fields
        // only (slice AL1): none the world holds for the other side.
        for object in quick.offered_airports(index) {
            let scene = &world.airport_scene;
            let serves = scene
                .runway(*object)
                .and_then(|r| scene.airports.iter().find(|a| a.id == r.airport))
                .is_some_and(|a| a.serves(false));
            if !serves {
                m.problems.push(format!(
                    "theater {code} offers runway {object:#x}, not a Blue or neutral field"
                ));
            }
        }
        // The enemy's list is Redfor's and the neutral fields (slice CL-R).
        for object in quick.offered_enemy_airports(index) {
            let scene = &world.airport_scene;
            let serves = scene
                .runway(*object)
                .and_then(|r| scene.airports.iter().find(|a| a.id == r.airport))
                .is_some_and(|a| a.serves(true));
            if !serves {
                m.problems.push(format!(
                    "theater {code} offers the enemy runway {object:#x}, not a Redfor or neutral field"
                ));
            }
        }
        for separation in 0..SEPARATION_NM.len() {
            for altitude in 0..4 {
                quick.apply(17, separation);
                quick.apply(14, altitude);
                if altitude == 1 && (separation == 0 || separation + 1 == SEPARATION_NM.len()) {
                    m.run(&quick, &world, "airborne theater sweep");
                } else {
                    m.plan(&quick, &world, "airborne theater layout");
                }
            }
        }
        quick.apply(17, Draft::default().values[17]);
        quick.apply(14, Draft::default().values[14]);
        let runways = quick.airport_objects[index].len();
        quick.apply(33, 1);
        if runways == 0 {
            m.run(&quick, &world, "ground start without runways");
        }
        for runway in 0..runways {
            quick.apply(34, runway);
            for size in 1..=5 {
                quick.apply(4, size);
                if runway == 0 && (size == 1 || size == 5) {
                    m.run(&quick, &world, "ground runway sweep");
                } else {
                    m.plan(&quick, &world, "ground runway layout");
                }
            }
        }
        quick.apply(4, 2);
        quick.apply(33, 0);
        // Every ground target of the theater with a spread of defenses, then
        // a ground start the world places from the target.
        if !code.starts_with('~') {
            for target in 1..quick.values(30).len() {
                quick.apply(30, target);
                for (aaa, sam) in [(0, 0), (1, 3), (2, 2), (3, 1), (3, 3)] {
                    quick.apply(31, aaa);
                    quick.apply(32, sam);
                    if target == 1 && (aaa, sam) == (3, 3) {
                        m.run(&quick, &world, "ground target sweep");
                    } else {
                        m.plan(&quick, &world, "ground target layout");
                    }
                }
                if target == 1 && runways > 0 {
                    quick.apply(33, 1);
                    m.run(&quick, &world, "ground auto sweep");
                    quick.apply(34, 1);
                    m.plan(&quick, &world, "ground target, named runway");
                    quick.apply(33, 0);
                }
            }
            quick.apply(30, 0);
        }
    }

    m.stage("start of weather sweep");
    // Loop 2: every weather choice on every source theater, both start modes.
    let base: Vec<String> = quick.theater_codes.iter().take(16).cloned().collect();
    for code in base {
        let index = quick.theater_codes.iter().position(|c| *c == code).unwrap();
        for weather in 0..6 {
            let Some(condition) = condition(weather) else {
                continue;
            };
            let world = match crate::scenery::launch_terrain(data, &code, Some(condition)) {
                Ok(w) => w,
                Err(e) => {
                    m.problems.push(format!(
                        "theater {code} weather {weather} would not load: {e}"
                    ));
                    continue;
                }
            };
            quick.apply(13, index);
            quick.apply(15, weather);
            quick.apply(33, 0);
            m.run(&quick, &world, "weather airborne");
            if !quick.airport_objects[index].is_empty() {
                quick.apply(33, 1);
                quick.apply(34, 0);
                m.run(&quick, &world, "weather ground");
            }
            quick.apply(33, 0);
        }
        quick.apply(15, Draft::default().values[15]);
    }

    m.stage("start of aircraft sweep");
    // Loop 3: every player aircraft, airborne and on the first runway of two
    // theaters, against each enemy aircraft.
    for code in ["UKR"] {
        let index = quick.theater_codes.iter().position(|c| c == code).unwrap();
        let world = crate::scenery::launch_terrain(data, code, None)?;
        quick.apply(13, index);
        for player in 0..selectable {
            quick.apply(6, player);
            for enemy in 0..quick.wing_files.len() {
                for field in [23, 26, 29] {
                    quick.apply(field, enemy);
                }
                for start in [0, 1] {
                    quick.apply(33, start);
                    quick.apply(34, 0);
                    m.run(&quick, &world, "player and enemy aircraft");
                }
            }
        }
        quick.apply(33, 0);
        for field in [23, 26, 29, 9, 12] {
            quick.apply(field, 0);
        }
        quick.apply(
            6,
            quick
                .aircraft_files
                .iter()
                .position(|f| f == AircraftId::F18.selection_key())
                .unwrap_or(0),
        );
    }

    m.stage("start of wing sweep");
    // Loop 4: every wing's count, skill and aircraft, one wing at a time.
    let world = crate::scenery::launch_terrain(data, "UKR", None)?;
    let index = quick.theater_codes.iter().position(|c| c == "UKR").unwrap();
    quick.apply(13, index);
    for field in [4, 7, 10, 21, 24, 27] {
        let baseline = quick.draft.values[field..field + 3].to_vec();
        for count in 0..=5 {
            for skill in 0..quick.values(field + 1).len() {
                quick.apply(field, count);
                quick.apply(field + 1, skill);
                m.run(&quick, &world, "wing count and skill sweep");
            }
        }
        quick.apply(field, 2);
        // Friendly wing 1 shares the player's list; the other five wings hold
        // only what the AI can fly.
        let choices = if field == 4 {
            selectable
        } else {
            quick.wing_files.len()
        };
        for aircraft in 0..choices {
            quick.apply(field + 2, aircraft);
            m.run(&quick, &world, "wing aircraft sweep");
        }
        for (offset, value) in baseline.into_iter().enumerate() {
            quick.apply(field + offset, value);
        }
    }

    m.stage("start of wing product and orders");
    // Loop 5: the full wing-count product on two skills, then every order for
    // every group, then every mission preset, with all six wings populated.
    let mut counts = [0usize; 6];
    let fields = [4, 7, 10, 21, 24, 27];
    let mut seen = 0;
    loop {
        for (field, count) in fields.iter().zip(counts) {
            quick.apply(*field, count);
        }
        // Sample the product (every 211th) to keep the sweep quick.
        if seen % 211 == 0 {
            m.run(&quick, &world, "wing count product");
        }
        seen += 1;
        let mut i = 0;
        while i < 6 {
            counts[i] += 1;
            if counts[i] <= 5 {
                break;
            }
            counts[i] = 0;
            i += 1;
        }
        if i == 6 {
            break;
        }
    }
    for field in fields {
        quick.apply(field, 2);
    }
    for group in 0..OBJECTIVE_COUNT {
        for (_, objective) in QuickMission::objective_choices(group) {
            for survive in [false, true] {
                quick.group_objectives[group] = objective;
                quick.group_must_survive[group] = survive;
                m.run(&quick, &world, "group orders");
            }
        }
        quick.group_objectives[group] = GroupObjective::Inherit;
        quick.group_must_survive[group] = false;
    }
    for preset in crate::ai_wings::Preset::ALL {
        quick.ai_mission = preset;
        m.run(&quick, &world, "mission preset");
    }
    quick.ai_mission = crate::ai_wings::Preset::Free;

    m.stage("start of remaining fields");
    // Loop 6: every other dropdown value once (nationalities, ordnance load,
    // and the fields the loops above do not vary).
    for id in 0..quick.draft.values.len() {
        if matches!(id, 4..=14 | 17 | 21..=29 | 33..=36) || quick.values(id).is_empty() {
            continue;
        }
        let saved = quick.draft.values[id];
        for value in 0..quick.values(id).len() {
            quick.apply(id, value);
            m.run(&quick, &world, &format!("field {id}"));
        }
        quick.apply(id, saved);
    }
    // With no target the strengths stay: they are the enemy's area
    // defenses (B23), and the mission says so.
    quick.apply(31, 2);
    quick.apply(32, 2);
    let area = quick
        .mission_spec()
        .is_ok_and(|spec| spec.ground_target.is_none() && spec.area_defenses());
    if (quick.draft.values[31], quick.draft.values[32]) != (2, 2) || !area {
        m.problems
            .push("the strengths with no ground target are not the area defenses".into());
    }
    quick.apply(31, 0);
    quick.apply(32, 0);

    println!(
        "creator matrix: {} setups started, {} refused with a message, {} problems",
        m.started,
        m.refused,
        m.problems.len()
    );
    for (message, (count, examples)) in &m.refusals {
        println!("  refused x{count}: {message}");
        for example in examples {
            println!("      e.g. {example}");
        }
    }
    for problem in m.problems.iter().take(60) {
        println!("  PROBLEM {problem}");
    }
    if m.problems.is_empty() {
        Ok(())
    } else {
        Err(format!("creator matrix found {} problems", m.problems.len()).into())
    }
}

/// Draws the creator with every dropdown value chosen, every selector open at
/// every page, and the help and notice overlays, checking that nothing panics
/// and that no picture is blank. Legibility is judged from the snapshots.
pub fn render(
    data: &BTreeMap<String, Vec<u8>>,
    options: Options,
    sprites: &BTreeMap<String, Sprite>,
    world: &Terrain,
) -> AppResult<()> {
    let mut quick = QuickMission::new(AircraftId::F18, options, data);
    let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
    let mut problems: Vec<String> = Vec::new();
    let mut drawn = 0usize;
    let mut draw = |quick: &mut QuickMission, what: String, problems: &mut Vec<String>| {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            quick.render(&mut pixels, sprites, world);
        }));
        drawn += 1;
        if result.is_err() {
            problems.push(format!("{what}: render panicked"));
            return;
        }
        let colours: std::collections::BTreeSet<[u8; 3]> = pixels
            .chunks_exact(4)
            .step_by(7)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        if colours.len() < 40 {
            problems.push(format!(
                "{what}: picture is blank ({} colours)",
                colours.len()
            ));
        }
    };
    let fields: Vec<usize> = (3..quick.draft.values.len()).collect();
    for id in fields.iter().copied() {
        // The defenses need a target to hold a strength.
        if matches!(id, 31 | 32) {
            quick.apply(30, 1);
        }
        let saved = quick.draft.values[id];
        let count = quick.values(id).len();
        for value in 0..count {
            quick.apply(id, value);
            draw(
                &mut quick,
                format!("field {id} value {value}"),
                &mut problems,
            );
        }
        quick.apply(id, saved);
        // The selector popup, page by page.
        quick.open(id);
        let pages = quick.selector_values(id).len().div_ceil(ROWS).max(1);
        for page in 0..pages {
            quick.scroll = page * ROWS;
            quick.cursor = quick
                .cursor
                .min(quick.selector_values(id).len().saturating_sub(1));
            draw(
                &mut quick,
                format!("selector {id} page {page}"),
                &mut problems,
            );
        }
        quick.cancel();
        if matches!(id, 31 | 32) {
            quick.apply(30, 0);
        }
    }
    for id in OBJECTIVE_BASE..OBJECTIVE_BASE + OBJECTIVE_COUNT {
        quick.open(id);
        draw(&mut quick, format!("group selector {id}"), &mut problems);
        quick.cancel();
    }
    for group in 0..OBJECTIVE_COUNT {
        for (index, (_, objective)) in QuickMission::objective_choices(group)
            .into_iter()
            .enumerate()
        {
            quick.group_objectives[group] = objective;
            draw(
                &mut quick,
                format!("group {group} order {index}"),
                &mut problems,
            );
        }
        quick.group_objectives[group] = GroupObjective::Inherit;
    }
    // Worst-case text: the longest labels in every row, saved as pictures when
    // TORE_CREATOR_DUMP names a folder, for a look at overflow and overlap.
    if let Some(folder) = std::env::var_os("TORE_CREATOR_DUMP").map(std::path::PathBuf::from) {
        std::fs::create_dir_all(&folder)?;
        let longest = |quick: &QuickMission, id: usize| {
            (0..quick.values(id).len())
                .max_by_key(|v| quick.values(id)[*v].len())
                .unwrap_or(0)
        };
        for (name, theater) in [
            ("ukraine", "UKR"),
            ("egypt", "EGY"),
            ("vietnam", "TVIET"),
            ("kurile", "KURILE"),
        ] {
            let index = quick
                .theater_codes
                .iter()
                .position(|c| c == theater)
                .unwrap_or(0);
            quick.apply(13, index);
            for field in [4, 7, 10, 21, 24, 27] {
                quick.apply(field, 5);
                let skill = longest(&quick, field + 1);
                quick.apply(field + 1, skill);
                let aircraft = longest(&quick, field + 2);
                quick.apply(field + 2, aircraft);
            }
            for id in [14, 15, 16, 17, 18, 19, 3, 20, 30, 31, 32] {
                let value = longest(&quick, id);
                quick.apply(id, value);
            }
            quick.apply(33, 1);
            let airport = longest(&quick, 34);
            quick.apply(34, airport);
            for group in 0..OBJECTIVE_COUNT {
                let choices = QuickMission::objective_choices(group);
                let (_, objective) = choices
                    .iter()
                    .max_by_key(|(label, _)| label.len())
                    .cloned()
                    .unwrap();
                quick.group_objectives[group] = objective;
                quick.group_must_survive[group] = true;
            }
            let mut shot = vec![0u8; WIDTH * HEIGHT * 4];
            quick.render(&mut shot, sprites, world);
            let mut out = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
            for p in shot.chunks_exact(4) {
                out.extend_from_slice(&p[..3]);
            }
            std::fs::write(folder.join(format!("worst-{name}.ppm")), out)?;
        }
    }
    // Every bitmap font drawing a sample with accented letters, saved when
    // TORE_CREATOR_DUMP names a folder, to see that no cell is a blank or a box.
    if let Some(folder) = std::env::var_os("TORE_CREATOR_DUMP").map(std::path::PathBuf::from) {
        let mut shot = vec![0u8; WIDTH * HEIGHT * 4];
        for pixel in shot.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[60, 60, 70, 255]);
        }
        let mut y = 4;
        for (name, font) in sprites.iter().filter(|(_, f)| f.glyphs.len() == 256) {
            let height = font.glyphs.iter().map(|g| g[2]).max().unwrap_or(0) as i32;
            if y + height + 2 > HEIGHT as i32 {
                break;
            }
            Canvas(&mut shot).text(
                font,
                "Ber\u{eb}zovka \u{fc}\u{e9}\u{f1}\u{df}\u{e0} ABC",
                4,
                y,
                None,
            );
            Canvas(&mut shot).text(&sprites["QUICKFONT"], name, 400, y, None);
            y += height + 3;
        }
        std::fs::create_dir_all(&folder)?;
        let mut out = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
        for p in shot.chunks_exact(4) {
            out.extend_from_slice(&p[..3]);
        }
        std::fs::write(folder.join("fonts.ppm"), out)?;
    }
    // The debrief with the biggest numbers it can be handed, and with distinct
    // spoofed and jammed counts (so a swapped row would show), on every page,
    // for a look at columns that run into each other.
    {
        use crate::debrief::{Debrief, Objective, Outcome, Pilot, Report, Status};
        use tore_sim::combat::ledger::Tally;
        let folder = std::env::var_os("TORE_CREATOR_DUMP").map(std::path::PathBuf::from);
        for (name, tally) in [
            (
                "big",
                Tally {
                    launched: 99_999,
                    hit: 88_888,
                    damage: 1_234_567,
                    missed: 5,
                    spoofed: 6,
                    jammed: 7,
                },
            ),
            (
                "distinct",
                Tally {
                    launched: 100,
                    hit: 10,
                    damage: 55,
                    missed: 20,
                    spoofed: 30,
                    jammed: 40,
                },
            ),
        ] {
            let pilot = Pilot {
                status: Status::Ejected,
                damage: 1.,
                landing_grade: Some(100),
                cause: None,
                kills: [999; 10],
                friendly_fire: 999,
                air_to_air: tally,
                air_to_ground: tally,
                gun: tally,
                bombs: tally,
                enemy_aam: tally,
                enemy_sam: tally,
                enemy_gun: tally,
                enemy_aaa: tally,
                shot_down_by: None,
            };
            let report = Report {
                outcome: Outcome::Success,
                objectives: vec![
                    Objective::Destroy {
                        destroyed: 29,
                        total: 29,
                    },
                    Objective::Protect {
                        protected: 29,
                        total: 29,
                    },
                ],
                elapsed_seconds: 359_999,
                player: pilot.clone(),
                wingman: Some(pilot),
            };
            for page in 1..=5usize {
                let mut debrief = Debrief::new(report.clone(), data, Some("DEBSCV.PIC"))?;
                debrief.page = page - 1;
                let mut shot = vec![0u8; WIDTH * HEIGHT * 4];
                let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    debrief.render(&mut shot);
                }))
                .is_err();
                if panicked {
                    problems.push(format!("debrief page {page} ({name}) panicked"));
                }
                if let Some(folder) = &folder {
                    std::fs::create_dir_all(folder)?;
                    let mut out = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
                    for p in shot.chunks_exact(4) {
                        out.extend_from_slice(&p[..3]);
                    }
                    std::fs::write(folder.join(format!("debrief-{name}-{page}.ppm")), out)?;
                }
            }
        }
    }
    quick.help = true;
    draw(&mut quick, "help".into(), &mut problems);
    quick.help = false;
    quick.notice = Some(NO_FIELD_NOTICE.into());
    draw(&mut quick, "notice".into(), &mut problems);
    quick.notice = None;
    // A ground target with defenses, and the ground start the world places.
    quick.apply(30, 1);
    quick.apply(31, 2);
    quick.apply(32, 3);
    draw(&mut quick, "ground target".into(), &mut problems);
    quick.apply(33, 1);
    draw(&mut quick, "ground start auto".into(), &mut problems);
    quick.apply(33, 0);
    quick.apply(30, 0);
    println!(
        "creator render sweep: {drawn} pictures, {} problems",
        problems.len()
    );
    for problem in problems.iter().take(40) {
        println!("  PROBLEM {problem}");
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("creator render sweep found {} problems", problems.len()).into())
    }
}

/// A tiny fixed-seed generator so the input fuzz is the same on every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const FUZZ_KEYS: [&str; 18] = [
    "Tab",
    "ArrowDown",
    "ArrowUp",
    "ArrowLeft",
    "ArrowRight",
    "Enter",
    "Escape",
    "PageDown",
    "PageUp",
    "Home",
    "End",
    " ",
    "+",
    "-",
    "=",
    "a",
    "s",
    "x",
];

/// Drives the creator and the loadout page with a fixed random stream of key
/// presses, pointer moves, clicks and right clicks. Nothing may panic, the
/// creator's fields must stay inside their lists, and the loadout must stay
/// inside every station's capacity.
pub fn fuzz(
    data: &BTreeMap<String, Vec<u8>>,
    options: Options,
    sprites: &BTreeMap<String, Sprite>,
    world: &Terrain,
) -> AppResult<()> {
    let mut problems: Vec<String> = Vec::new();
    let mut events = 0usize;
    let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
    // The creator.
    for seed in 1..=6u64 {
        let mut quick = QuickMission::new(AircraftId::F18, options.clone(), data);
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
        for step in 0..6000 {
            events += 1;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match rng.below(8) {
                    0..=2 => {
                        let key = FUZZ_KEYS[rng.below(FUZZ_KEYS.len())];
                        quick.key(key, rng.below(4) == 0);
                    }
                    3 | 4 => quick.pointer(Some((rng.below(640) as f64, rng.below(480) as f64))),
                    5 => {
                        quick.down();
                    }
                    6 => {
                        quick.up();
                    }
                    _ => {
                        let down = rng.below(2) == 0;
                        quick.right(down);
                    }
                }
                if step % 25 == 0 {
                    quick.render(&mut pixels, sprites, world);
                }
            }));
            if result.is_err() {
                problems.push(format!("creator fuzz seed {seed} step {step}: panicked"));
                break;
            }
            if let Some(id) = (3..quick.draft.values.len()).find(|id| {
                let len = quick.values(*id).len();
                len > 0 && quick.draft.values[*id] >= len
            }) {
                problems.push(format!(
                    "creator fuzz seed {seed} step {step}: field {id} left its list"
                ));
                break;
            }
            if quick.draft.values[4] == 0 {
                problems.push(format!(
                    "creator fuzz seed {seed} step {step}: no player in wing 1"
                ));
                break;
            }
        }
    }
    // The loadout page, every aircraft.
    for id in AircraftId::SELECTABLE {
        let airframe = Airframe::load(data, id)?;
        let load = tore_sim::combat::loadout::Loadout::new(&airframe.profile, |name| {
            data.get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
        })?;
        let mut ui = crate::ordnance::Ordnance::new(load, data)?;
        let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ id.pt().len() as u64 ^ (events as u64));
        for step in 0..3000 {
            events += 1;
            ui.visible = true;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match rng.below(9) {
                    0..=2 => {
                        ui.key(FUZZ_KEYS[rng.below(FUZZ_KEYS.len())]);
                    }
                    3..=5 => ui.pointer(Some((rng.below(640) as f64, rng.below(480) as f64))),
                    6 => {
                        ui.down();
                    }
                    7 => {
                        ui.up();
                    }
                    _ => {
                        let down = rng.below(2) == 0;
                        ui.right(down);
                    }
                }
                if step % 25 == 0 {
                    ui.render(&mut pixels);
                }
            }));
            if result.is_err() {
                problems.push(format!("{id:?} loadout fuzz step {step}: panicked"));
                break;
            }
            let load = &ui.loadout;
            let bad = load
                .configuration
                .stations
                .iter()
                .zip(&load.quantities)
                .enumerate()
                .find(|(i, (s, n))| i32::from(**n) > load.capacity(*i, &s.weapon));
            if bad.is_some()
                || !load.fuel_lbs.is_finite()
                || !(0. ..=load.internal_capacity_lbs).contains(&load.fuel_lbs)
                || load.quantities.iter().any(|n| *n > 32766)
            {
                problems.push(format!(
                    "{id:?} loadout fuzz step {step}: quantities {:?} fuel {}",
                    load.quantities, load.fuel_lbs
                ));
                break;
            }
        }
    }
    println!(
        "creator input fuzz: {events} events, {} problems",
        problems.len()
    );
    for problem in problems.iter().take(40) {
        println!("  PROBLEM {problem}");
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("creator input fuzz found {} problems", problems.len()).into())
    }
}

/// The same fixed-stream fuzz for the preference and input screens: the
/// graphics, sound, controls and replay screens take keys, wheel turns, moves
/// and clicks and must neither panic nor draw a blank picture.
pub fn fuzz_screens(
    data: &BTreeMap<String, Vec<u8>>,
    menu: &mut crate::menu::Menu,
) -> AppResult<()> {
    let hornet = Airframe::load(data, AircraftId::F18)?;
    let font = &hornet.font;
    let mut problems: Vec<String> = Vec::new();
    let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
    let mut events = 0usize;
    let blank = |pixels: &[u8]| {
        pixels
            .chunks_exact(4)
            .step_by(11)
            .map(|p| [p[0], p[1], p[2]])
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            < 6
    };
    // The flight menu (Escape), keyboard help, map and cheats, with the
    // flight's own key handling.
    for seed in 1..=3u64 {
        let mut rng = Rng(0x7777_1234_abcd_ef01 ^ seed);
        let mut ui = crate::flight_ui::FlightUi::default();
        ui.menu = false;
        for step in 0..6000 {
            events += 1;
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match rng.below(8) {
                    0..=3 => {
                        ui.key(
                            FUZZ_KEYS[rng.below(FUZZ_KEYS.len())],
                            rng.below(4) == 0,
                            rng.below(6) == 0,
                            rng.below(8) == 0,
                            &hornet.flight_menu,
                        );
                    }
                    4 | 5 => {
                        let down = rng.below(2) == 0;
                        ui.pointer(
                            &hornet.flight_menu,
                            Some((rng.below(640) as f64, rng.below(480) as f64)),
                            down,
                        );
                    }
                    6 => {
                        ui.key("Escape", false, false, false, &hornet.flight_menu);
                    }
                    _ => ui.cancel_press(),
                }
                if step % 40 == 0 {
                    ui.draw(&mut pixels, font, &hornet.flight_menu);
                }
            }))
            .is_ok();
            if !ok {
                problems.push(format!("flight menu seed {seed} step {step}: panicked"));
                break;
            }
        }
    }
    // The main menu and its bars.
    let mut rng = Rng(0x0fed_cba9_8765_4321);
    for step in 0..6000 {
        events += 1;
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match rng.below(8) {
                0..=2 => {
                    menu.state
                        .key(FUZZ_KEYS[rng.below(FUZZ_KEYS.len())], rng.below(4) == 0);
                }
                3 | 4 => {
                    menu.state
                        .pointer(Some((rng.below(640) as f64, rng.below(480) as f64)));
                }
                5 => menu.state.down(),
                6 => {
                    menu.state.up();
                }
                _ => menu.state.cancel(),
            }
            if step % 40 == 0 {
                menu.render();
            }
        }))
        .is_ok();
        if !ok {
            problems.push(format!("main menu step {step}: panicked"));
            break;
        }
    }
    for seed in 1..=4u64 {
        let mut rng = Rng(0x1234_5678_9abc_def1 ^ seed);
        let point = |rng: &mut Rng| Some((rng.below(640) as f64, rng.below(480) as f64));
        // Graphics.
        let mut graphics = crate::graphics_screen::Editor::new(
            crate::graphics::Options::default(),
            [true, true, true, seed % 2 == 0],
            "Main menu",
        );
        // Half the runs know the window, so the resolution line draws too.
        graphics.set_window((seed % 2 == 1).then_some([3840, 2160]));
        for step in 0..3000 {
            events += 1;
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match rng.below(6) {
                    0..=2 => {
                        graphics.key(FUZZ_KEYS[rng.below(FUZZ_KEYS.len())], rng.below(4) == 0);
                    }
                    3 => {
                        graphics.wheel(rng.below(5) as i32 - 2);
                    }
                    _ => {
                        let down = rng.below(2) == 0;
                        let at = point(&mut rng);
                        graphics.moved(at);
                        graphics.pointer(at, down);
                    }
                }
                if step % 30 == 0 {
                    graphics.draw(&mut pixels, font);
                }
            }))
            .is_ok();
            if !ok {
                problems.push(format!("graphics screen seed {seed} step {step}: panicked"));
                break;
            }
        }
        graphics.draw(&mut pixels, font);
        if blank(&pixels) {
            problems.push(format!("graphics screen seed {seed}: blank after input"));
        }
        // What the screen applies is what it saves, and never a choice the
        // adapter cannot do (8x when unsupported).
        let path = std::env::temp_dir().join(format!(
            "tore-graphics-fuzz-{}-{seed}.conf",
            std::process::id()
        ));
        let applied = graphics.apply(Some(&path));
        if seed % 2 != 0 && applied.anti_aliasing == crate::graphics::AntiAliasing::X8 {
            problems.push(format!(
                "graphics screen seed {seed}: applied unsupported 8x"
            ));
        }
        if crate::graphics::Options::load(&path) != applied {
            problems.push(format!(
                "graphics screen seed {seed}: saved options differ from applied"
            ));
        }
        let _ = std::fs::remove_file(&path);
        // Sound.
        let mut sound = crate::sound_screen::Screen::new(
            crate::sound_prefs::Settings::default(),
            seed % 2 == 0,
        );
        for step in 0..3000 {
            events += 1;
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match rng.below(7) {
                    0..=2 => {
                        sound.key(FUZZ_KEYS[rng.below(FUZZ_KEYS.len())], rng.below(4) == 0);
                    }
                    3 => {
                        sound.wheel(rng.below(5) as i32 - 2);
                    }
                    4 => {
                        sound.moved(point(&mut rng));
                    }
                    _ => {
                        let down = rng.below(2) == 0;
                        sound.button(point(&mut rng), down);
                    }
                }
                if step % 30 == 0 {
                    sound.animate();
                    sound.draw(&mut pixels, &menu.sprites);
                }
            }))
            .is_ok();
            if !ok {
                problems.push(format!("sound screen seed {seed} step {step}: panicked"));
                break;
            }
        }
        // Controls, on a synthetic pad.
        let pad = crate::controls_editor::preview_device();
        let defaults = crate::input::gamepad_defaults(&pad);
        let profile = tore_input::Profile {
            bindings: defaults.bindings,
            modifiers: defaults.modifiers,
            gamepad_defaults: true,
            ..Default::default()
        };
        let mut controls = crate::controls_editor::Editor::new(profile, vec![pad], "Main menu");
        for step in 0..4000 {
            events += 1;
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match rng.below(8) {
                    0..=3 => {
                        let key = FUZZ_KEYS[rng.below(FUZZ_KEYS.len())];
                        controls.key(key, rng.below(4) == 0, rng.below(6) == 0, rng.below(8) == 0);
                    }
                    4 => {
                        controls.wheel(rng.below(5) as i32 - 2);
                    }
                    5 => {
                        controls.text_input(["a", "fire", "x", " ", "1"][rng.below(5)]);
                    }
                    _ => {
                        let down = rng.below(2) == 0;
                        controls.pointer(point(&mut rng), down);
                    }
                }
                if step % 40 == 0 {
                    controls.draw(&mut pixels, font);
                }
            }))
            .is_ok();
            if !ok {
                problems.push(format!("controls screen seed {seed} step {step}: panicked"));
                break;
            }
        }
        controls.cancel_capture();
        controls.draw(&mut pixels, font);
        if blank(&pixels) {
            problems.push(format!("controls screen seed {seed}: blank after input"));
        }
        // Replays.
        let mut replays = crate::replay::screen::Replays::preview("Main menu");
        for step in 0..3000 {
            events += 1;
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                match rng.below(7) {
                    0..=2 => {
                        replays.key(
                            FUZZ_KEYS[rng.below(FUZZ_KEYS.len())],
                            rng.below(4) == 0,
                            rng.below(5) == 0,
                        );
                    }
                    3 => {
                        replays.wheel(rng.below(5) as i32 - 2);
                    }
                    _ => {
                        let down = rng.below(2) == 0;
                        replays.pointer(point(&mut rng), down);
                    }
                }
                if step % 30 == 0 {
                    replays.draw(&mut pixels, font);
                }
            }))
            .is_ok();
            if !ok {
                problems.push(format!("replay screen seed {seed} step {step}: panicked"));
                break;
            }
        }
    }
    println!(
        "screen input fuzz: {events} events, {} problems",
        problems.len()
    );
    for problem in problems.iter().take(40) {
        println!("  PROBLEM {problem}");
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("screen input fuzz found {} problems", problems.len()).into())
    }
}

/// Prints every leaf of the retail in-flight menu bar (`FMENUD.MNU`, the
/// manual's appendix D) with what activating it does in this game: a working
/// command or toggle, or the "not implemented yet" message the game shows for
/// an item it does not connect. `TORE_CREATOR_STAGE=menu` runs only this.
pub fn flight_menu_table(data: &BTreeMap<String, Vec<u8>>) -> AppResult<()> {
    let hornet = Airframe::load(data, AircraftId::F18)?;
    fn walk(
        nodes: &[tore_formats::ui::MenuNode],
        path: &str,
        out: &mut Vec<(String, String, String)>,
    ) {
        for node in nodes {
            let here = if path.is_empty() {
                node.label.clone()
            } else {
                format!("{path} > {}", node.label)
            };
            if node.children.is_empty() {
                let mut ui = crate::flight_ui::FlightUi::default();
                let command = ui.activate(&node.label, &node.shortcut);
                let notes = ui.take_notes();
                let missing = notes
                    .iter()
                    .any(|(text, _)| text.contains("not implemented yet"));
                let result = if missing {
                    "NOT IMPLEMENTED".to_string()
                } else {
                    let said = notes
                        .first()
                        .map(|(t, _)| format!(", says \"{t}\""))
                        .unwrap_or_default();
                    format!("{command:?}{said}")
                };
                out.push((here, node.shortcut.clone(), result));
            } else {
                walk(&node.children, &here, out);
            }
        }
    }
    let mut rows = Vec::new();
    walk(&hornet.flight_menu, "", &mut rows);
    let missing = rows.iter().filter(|r| r.2 == "NOT IMPLEMENTED").count();
    for (path, shortcut, result) in &rows {
        println!("flight menu | {path} | {shortcut} | {result}");
    }
    println!(
        "flight menu: {} items, {missing} not implemented",
        rows.len()
    );
    Ok(())
}
