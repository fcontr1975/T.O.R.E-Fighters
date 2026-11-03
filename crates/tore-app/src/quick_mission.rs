//! Retail option values with an explicit editable setup; simulation capabilities
//! are validated separately. Popup frame/focus feedback are authored presentation.
use crate::{
    camera::Camera,
    menu::{Action, Canvas, HEIGHT, Sprite, WIDTH, text_width},
    mission_layout::{FEET_PER_NM, RETAIL_SEPARATIONS, SEPARATION_NM},
    terrain::Terrain,
};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use tore_formats::{aircraft::AircraftId, ui::creator::Options};
use tore_session::wire::messages::ItemKind;
use tore_sim::ai::{
    AiError,
    engagement::GroupObjective,
    experience::EnemySkillOverride,
    launch::{Side, WingId, WingLaunch, WingSelection, resolve_wings},
};
use tore_world::mission::{
    ALTITUDES_FT, Condition, Defense, EnemyStart, MissionSpec, Situation, Skill, Start, WingSpec,
    ground_targets,
};
pub mod allegiance;
mod distances;
pub mod loadout_memory;
pub mod map;
pub mod matrix;
pub mod remember;
/// The aircraft fields of the five wings the AI flies. Field 6 is friendly
/// wing 1's, which is also the player's own aircraft.
const AI_AIRCRAFT_FIELDS: [usize; 5] = [9, 12, 23, 26, 29];
/// The creator's aircraft list cut down to what the AI can fly.
fn ai_catalog(files: &[String], names: &[String]) -> (Vec<String>, Vec<String>) {
    files
        .iter()
        .zip(names)
        .filter(|(file, _)| AircraftId::parse(file).is_ok_and(AircraftId::ai_flyable))
        .map(|(file, name)| (file.clone(), name.clone()))
        .unzip()
}
type Rect = (i32, i32, i32, i32);
/// What a mission holds that the creator's lists cannot, by field: how the
/// read-only creator draws it, and the words that say what it is.
type Unshown = BTreeMap<usize, (String, String)>;
const POPUP: Rect = (185, 100, 270, 370);
const ROWS: usize = 15;
const ROW_BASE: usize = 100;
const OK: usize = 1;
const CANCEL: usize = 2;
/// The top bar's Map entry, after Aircraft (slice CL-R).
const MAP: usize = 62;
/// The room after a top-bar label before the next entry's label starts: the
/// ordnance bar's Weapons entry is 75 wide and starts its label at 103, the
/// next entry's at 178 (measured, 13 to 14 pixels after the word).
const BAR_GAP: i32 = 14;
/// The top bar's entries start at x=103 and sit at y 35 to 59, their labels
/// at y=38 to 58, centred by the visible glyph bounds of the entry's own
/// neighbour "Aircraft".
const BAR_ENTRY: (i32, i32) = (35, 24);
const POP_OK: usize = 70;
const POP_CANCEL: usize = 71;
const UP: usize = 72;
const DOWN: usize = 73;
const OBJECTIVE_BASE: usize = 80;
const OBJECTIVE_COUNT: usize = 6;
const SURVIVAL_BASE: usize = OBJECTIVE_BASE + OBJECTIVE_COUNT;
/// The draft's fields: 3 to 34 are the retail setup's (33 and 34 the
/// player's Start and Airport), 35 and 36 the enemy's Start and Airport
/// (slice CL-R, John 2026-10-11: Redfor ground starts that match the
/// creator's styles).
const FIELDS: usize = 44;
pub const AI_AIRCRAFT: usize = 43;
const CHOICE_BASE: usize = 37;
/// The enemy's Start field (Airborne or Ground), the Redfor mirror of 33.
const ENEMY_START: usize = 35;
/// The enemy's Airport field, the Redfor mirror of 34.
const ENEMY_AIRPORT: usize = 36;
/// The lowest y of the Enemy start row, so the Enemy airport row below it
/// (at 405) stays above the OK and Cancel buttons at 419.
const ENEMY_ROWS_LAST_Y: i32 = 391;
/// The first row of the Airport list when a ground target is set: the world
/// picks the runway from the target (`start ground auto`).
const AUTO_AIRPORT: &str = "Automatic (near the target)";
/// Said when Ground is touched in a theater with no airfield of the pilot's
/// side (slice AL1): the start stays airborne.
pub const NO_FIELD_NOTICE: &str = "No airfield of your side in this theater, so you start airborne. Ground start needs a friendly or neutral airfield.";
/// The short note beside a Start locked to Airborne for that reason.
const NO_FIELD_NOTE: &str = "(no friendly airfield)";
/// Said when the enemy's Ground is touched in a theater with no Redfor or
/// neutral airfield (slice CL-R): the enemy stays airborne.
pub const NO_ENEMY_FIELD_NOTICE: &str = "No Redfor airfield in this theater, so the enemy starts airborne. An enemy ground start needs a Redfor or neutral airfield.";
/// The short note beside an Enemy start locked to Airborne for that reason.
const NO_ENEMY_FIELD_NOTE: &str = "(no Redfor airfield)";
/// Refused at OK or Accept: an enemy ground start parks enemy wing 1.
const EMPTY_ENEMY_WING: &str =
    "Enemy wing 1 has no aircraft to park. Give it aircraft or start the enemy Airborne.";

/// One airport of a theater layout as the creator sees it for a ground start:
/// its runway object, name, whether it is a short strip and its layout owner
/// (`Some(true)` Redfor, `Some(false)` Blue, `None` without an owner field).
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub id: u32,
    pub name: String,
    pub short: bool,
    /// A STOL runway (`tore_sim::stol`): labelled so in the picker, and
    /// listed after the longer fields.
    pub stol: bool,
    pub redfor: Option<bool>,
}
impl Field {
    /// The field belongs to the side opposite to a pilot of `redfor`.
    pub fn enemy_of(&self, redfor: bool) -> bool {
        self.redfor == Some(!redfor)
    }
    /// A ground start for a pilot of this side: no field of the other side
    /// (slice AL1, John 2026-10-10). A neutral field is offered to both
    /// sides. A short strip is offered too, labelled STOL (John,
    /// 2026-10-11, replacing his 2026-09-30 rule that left them out).
    pub fn offered_to(&self, redfor: bool) -> bool {
        !self.enemy_of(redfor)
    }
    /// The name the picker shows: with `(STOL)` after a STOL field's.
    pub fn label(&self) -> String {
        tore_sim::stol::label(&self.name, self.stol)
    }
}
/// The tint of a list row the lobby's creator dims: what not everyone has.
const GAP_TEXT: [u8; 3] = [104, 110, 112];
/// The same dimming on a field's grey, where [`GAP_TEXT`] would not show.
const GAP_FIELD_TEXT: [u8; 3] = [158, 165, 167];
/// The fields the lobby's mission does not carry (the friendly nationality):
/// the read-only creator says they are the King's. The enemy nationality
/// (field 20) is the mission's, since it picks the target's units, and so is
/// the situation (field 16), since it places the starts.
const NOT_CARRIED: [usize; 1] = [3];
/// What those fields read in the read-only creator.
const AS_THE_KINGS: &str = "as the King's";
/// Said when a field of the read-only creator is clicked.
pub const VIEW_REFUSAL: &str = "Only the King changes the mission.";
/// Said when the King opens the creator while the mission flies.
pub const VIEW_FLYING: &str = "The mission can change only in the lobby.";
/// Said when the King opens the creator on a server whose mission is locked
/// (`king-mission locked`).
pub const VIEW_FIXED: &str = "This server's mission is fixed.";
/// Said when the read-only creator opens.
pub const VIEW_NOTICE: &str = "View only: this is the lobby's mission. Back returns to the lobby.";
/// Said when the read-only creator redraws from a new mission.
pub const VIEW_CHANGED: &str = "The King changed the mission.";

/// Why the lobby's creator is read-only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewKind {
    /// The player is not the King (or the lobby is a server's, which has none).
    Reader,
    /// The King, while the mission flies.
    Flying,
    /// The King, on a dedicated server whose file locks the mission.
    Fixed,
}

impl ViewKind {
    /// What a click on a field says.
    pub fn refusal(self) -> &'static str {
        match self {
            Self::Reader => VIEW_REFUSAL,
            Self::Flying => VIEW_FLYING,
            Self::Fixed => VIEW_FIXED,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub values: [usize; FIELDS],
}
impl Default for Draft {
    fn default() -> Self {
        let mut values = [0; FIELDS];
        for (id, value) in [
            (4, 1),
            (5, 1),
            (15, 1),
            (16, 1),
            (17, 2),
            (19, 1),
            (21, 2),
            (22, 2),
            (AI_AIRCRAFT, 1),
        ] {
            values[id] = value;
        }
        Self { values }
    }
}
impl Draft {
    /// The draft of a mission spec, read against `lists` (the creator's own
    /// lists of theaters and aircraft, which the draft indexes). It fails
    /// when the spec holds something the lists cannot show: an aircraft this
    /// game has no entry for, a count past the list's end, a theater or a
    /// distance or an airport the creator does not offer. The fields the
    /// spec does not carry (the nationalities, the situation, the load) keep
    /// the creator's own values for the theater. The player's and the
    /// enemy's ground starts read back into Start and Airport (33 and 34)
    /// and Enemy start and Enemy airport (35 and 36).
    pub fn from_spec(spec: &MissionSpec, lists: &QuickMission) -> Result<Draft, String> {
        let (draft, unshown) = lists.read_spec(spec)?;
        match unshown.into_values().next() {
            Some((_, words)) => Err(format!("This creator cannot show {words}.")),
            None => Ok(draft),
        }
    }
}
pub struct QuickMission {
    /// Results of the mission just flown, shown over the creator until OK.
    pub debrief: Option<crate::debrief::Debrief>,
    /// The debrief is a multiplayer flight's, which has no creator to go
    /// back to: closing it returns to the main menu.
    pub debrief_to_menu: bool,
    pub ordnance: Option<crate::ordnance::Ordnance>,
    pub hover: Option<usize>,
    pressed: Option<usize>,
    right_pressed: Option<usize>,
    /// The selector's PREV/NEXT page rocker.
    rocker: crate::rocker::Rocker,
    pub focus: usize,
    pub selection: usize,
    pub aircraft_selection: usize,
    pub aircraft_names: Vec<String>,
    pub aircraft_files: Vec<String>,
    /// The aircraft the AI can fly (`AircraftId::ai_flyable`): the choices of
    /// the five wing fields that are not the player's (9, 12, 23, 26, 29). A
    /// draft value in those fields indexes this list, not the player's.
    pub wing_names: Vec<String>,
    pub wing_files: Vec<String>,
    pub draft: Draft,
    /// Friendly groups 1 through 3, then enemy groups 1 through 3.
    pub group_objectives: [GroupObjective; OBJECTIVE_COUNT],
    /// Whole-group survival requirement, independent of its combat orders.
    pub group_must_survive: [bool; OBJECTIVE_COUNT],
    /// Mission-wide setting used when a group inherits its objective.
    pub ai_mission: crate::ai_wings::Preset,
    options: Options,
    theater_codes: Vec<String>,
    theater_catalog: Vec<String>,
    start_modes: Vec<String>,
    /// Per theater, the ground-start airports the pilot's side is offered
    /// (names and runway objects, in layout order), from [`Self::fields`].
    airport_names: Vec<Vec<String>>,
    airport_objects: Vec<Vec<u32>>,
    airport_aliases: Vec<BTreeMap<u32, u32>>,
    legacy_fields: Vec<Vec<Field>>,
    /// Per theater, the airports offered to the enemy's Enemy Wing 1 for an
    /// enemy ground start (fields 35 and 36): the Red and neutral fields (STOL
    /// strips last, labelled), whichever side the creator's pilot flies for.
    enemy_airport_names: Vec<Vec<String>>,
    enemy_airport_objects: Vec<Vec<u32>>,
    /// Per theater, every airport of the layout with its owner, short
    /// strips and the other side's fields included: what the offered lists
    /// are cut from and what a refused `--ground-start` names.
    fields: Vec<Vec<Field>>,
    /// The creator's pilot flies for Redfor, so Red fields are its own.
    /// Single player and the lobby's creator are Blue; a Redfor pilot's
    /// picker ([`Self::set_player_redfor`]) lists the Red and neutral fields.
    player_redfor: bool,
    selector: Option<usize>,
    cursor: usize,
    scroll: usize,
    controls: Vec<(usize, Rect)>,
    pub notice: Option<String>,
    pub help: bool,
    pub shift: bool,
    /// The creator is open from a multiplayer lobby (EF8): its OK button
    /// reads Accept and sends the mission to the game, and only what a host
    /// supports can be accepted. Both starts work as in single player
    /// (agent decision, slice CL-R, 2026-10-11).
    pub lobby: bool,
    /// The lobby's items not every player has (stage L, slice L4), each with
    /// the host's words for why it cannot be chosen: aircraft by their
    /// selection key, theaters by their code. Empty outside a lobby.
    gaps: BTreeMap<(ItemKind, String), String>,
    /// The creator is the lobby's mission read-only: every field is drawn as
    /// the King set it, none can be changed, and OK reads Back.
    view: Option<ViewKind>,
    /// What the read-only creator shows where its lists have no entry for
    /// the mission's value: an aircraft this game does not have (by field),
    /// a wing count past the list's end. Empty outside the read-only page.
    shown: BTreeMap<usize, String>,
    /// The seed of the surface layout, rolled when the creator's OK starts a
    /// flight with a ground target and kept until the next one, so a restart
    /// flies the same defenses. 0 until then.
    surface_seed: u32,
    /// The surface seed was drawn ahead of the next flight (by the map, or
    /// given with `--surface-seed`): OK keeps it instead of drawing anew, so
    /// the flight flies the layout the map showed.
    seed_pending: bool,
    /// The imported game data, which the map builds the mission from
    /// ([`Self::set_resources`]). `None` in tests.
    resources: Option<Arc<BTreeMap<String, Vec<u8>>>>,
    /// The map is open over the creator (slice CL-R).
    map: Option<Box<map::Planning>>,
    /// The map being built on its own thread (slice CL-R2).
    map_build: Option<map::Building>,
    /// The snapshot that shows the build's note keeps the map from opening.
    map_hold: bool,
}
/// What the creator keeps for the lobby's Cancel to put back: the draft and
/// everything beside it that Accept would send.
#[derive(Clone)]
pub struct Saved {
    draft: Draft,
    group_objectives: [GroupObjective; OBJECTIVE_COUNT],
    group_must_survive: [bool; OBJECTIVE_COUNT],
    ai_mission: crate::ai_wings::Preset,
}
/// Said when the lobby opens the creator. Start is no longer locked to
/// Airborne there: agent decision, slice CL-R, 2026-10-11, as John's
/// 2026-09-28 rule "everyone starts airborne" is superseded by his
/// 2026-10-11 request for Redfor ground starts that match the creator.
pub const LOBBY_NOTICE: &str = "Multiplayer: Accept sends this mission to the lobby.";
/// Maps a creator condition onto the six recovered source weather choices.
/// The two lists are both recovered but the engine holds no table joining
/// them, so this match is by label: dawn, clear, cloudy, foggy, sunset and
/// night each name one choice. The editor omits the duplicate overcast label.
pub fn condition(value: usize) -> Option<usize> {
    Some(match value {
        0 => 3,
        1 => 0,
        2 => 1,
        3 => 2,
        4 => 4,
        5 => 5,
        _ => return None,
    })
}

/// The ground-start choices among a theater's `fields` for a pilot of one
/// side: names (STOL fields labelled) and runway objects, in layout order
/// with the STOL fields after the others, so the default start is a field
/// any aircraft can take off from whenever the side has one.
fn offered(fields: &[Field], redfor: bool) -> (Vec<String>, Vec<u32>) {
    let side = || fields.iter().filter(|field| field.offered_to(redfor));
    side()
        .filter(|field| !field.stol)
        .chain(side().filter(|field| field.stol))
        .map(|field| (field.label(), field.id))
        .unzip()
}

impl QuickMission {
    pub fn new(id: AircraftId, mut options: Options, data: &BTreeMap<String, Vec<u8>>) -> Self {
        // Keep the imported option inventory intact; only the editor list drops
        // the duplicate. Draft indices below refer to this six-row list.
        options.fields[15]
            .retain(|label| !label.trim_end_matches('.').eq_ignore_ascii_case("overcast"));
        for field in [5, 8, 11, 22, 25, 28] {
            options.fields[field].truncate(4);
            options.fields[field].push("Dummy (400 KTS)".into());
        }
        // The retail list ends at 50; 75, 100, 150, 200 and 300 are host entries
        // in the retail label style (John, 2026-09-23). Every entry is read as
        // nautical miles, as the manual states.
        options.fields[17].truncate(RETAIL_SEPARATIONS);
        for nm in &SEPARATION_NM[options.fields[17].len()..] {
            options.fields[17].push(format!("{nm} miles"));
        }
        // Metadata for the full retail catalog is also cached. Only expose the
        // exact aircraft identities whose flight profiles were imported.
        let mut catalog: Vec<(String, String)> = AircraftId::SELECTABLE
            .into_iter()
            .filter(|id| {
                data.get(id.pt())
                    .and_then(|bytes| tore_formats::aircraft::Aircraft::parse(bytes).ok())
                    .is_some_and(|aircraft| aircraft.id == id.source())
            })
            .map(|id| (id.selection_key().to_string(), id.label().to_string()))
            .collect();
        catalog.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        let (aircraft_files, aircraft_names): (Vec<_>, Vec<_>) = catalog.into_iter().unzip();
        let selected = aircraft_files
            .iter()
            .position(|n| n == id.selection_key())
            .unwrap_or(0);
        let (wing_files, wing_names) = ai_catalog(&aircraft_files, &aircraft_names);
        let wing_selected = wing_files
            .iter()
            .position(|n| n == id.selection_key())
            .unwrap_or(0);
        let mut draft = Draft::default();
        draft.values[6] = selected;
        for i in AI_AIRCRAFT_FIELDS {
            draft.values[i] = wing_selected;
        }
        // Only the sixteen base theaters are offered to the player. The
        // imported `~` layout variants are incomplete (mostly one or two
        // airports); they stay reachable for probes through `--theater ~CODE`
        // (see `choose_theater_code`), but never appear in the list.
        let theater_codes: Vec<String> = source_theaters()
            .iter()
            .map(|code| code.to_string())
            .collect();
        options.fields[13].truncate(16);
        let theater_catalog: Vec<String> = tore_formats::theater::THEATERS
            .iter()
            .map(|(code, _)| code.to_string())
            .collect();
        let mut definitions = BTreeMap::new();
        let fields: Vec<Vec<Field>> = theater_codes
            .iter()
            .map(|code| Self::airports_in(code, data, &mut definitions))
            .collect();
        let airport_aliases: Vec<_> = theater_codes
            .iter()
            .map(|code| Self::aliases_in(code, data))
            .collect();
        let legacy_fields = fields.clone();
        let fields: Vec<Vec<Field>> = fields
            .into_iter()
            .zip(&airport_aliases)
            .map(|(fields, aliases)| {
                fields
                    .into_iter()
                    .filter(|f| !aliases.contains_key(&f.id))
                    .collect()
            })
            .collect();
        let (airport_names, airport_objects) =
            fields.iter().map(|list| offered(list, false)).unzip();
        let (enemy_airport_names, enemy_airport_objects) =
            fields.iter().map(|list| offered(list, true)).unzip();
        let mut quick = Self {
            debrief: None,
            debrief_to_menu: false,
            ordnance: None,
            start_modes: vec!["Airborne".into(), "Ground".into()],
            airport_names,
            airport_objects,
            airport_aliases,
            legacy_fields,
            enemy_airport_names,
            enemy_airport_objects,
            fields,
            player_redfor: false,
            hover: None,
            pressed: None,
            right_pressed: None,
            rocker: Default::default(),
            focus: 6,
            selection: 0,
            aircraft_selection: selected,
            aircraft_names,
            aircraft_files,
            wing_names,
            wing_files,
            draft,
            group_objectives: [GroupObjective::Inherit; OBJECTIVE_COUNT],
            group_must_survive: [false; OBJECTIVE_COUNT],
            ai_mission: crate::ai_wings::Preset::Free,
            options,
            theater_codes,
            theater_catalog,
            selector: None,
            cursor: 0,
            scroll: 0,
            controls: vec![],
            notice: None,
            help: false,
            shift: false,
            lobby: false,
            gaps: BTreeMap::new(),
            view: None,
            shown: BTreeMap::new(),
            surface_seed: 0,
            seed_pending: false,
            resources: None,
            map: None,
            map_build: None,
            map_hold: false,
        };
        // The enemy is the first theater's from the start, as a theater
        // change makes it for every other.
        quick.draft.values[20] = enemy_nationality(quick.base_theater_index());
        quick
    }
    /// Gives the creator the imported game data, which its map builds the
    /// mission from.
    pub fn set_resources(&mut self, resources: Arc<BTreeMap<String, Vec<u8>>>) {
        self.resources = Some(resources);
    }
    /// The map is open over the creator.
    pub fn map_open(&self) -> bool {
        self.map.is_some()
    }
    /// What the open map shows.
    pub fn map_plan(&self) -> Option<&crate::flight_map::Plan> {
        self.map.as_ref().map(|map| map.plan())
    }
    /// The runway objects the creator offers a Blue pilot's ground start in
    /// theater `code`, in list order: what the distance table generator
    /// measures a no-target ground start from.
    pub fn player_ground_runways(code: &str, data: &BTreeMap<String, Vec<u8>>) -> Vec<u32> {
        let aliases = Self::aliases_in(code, data);
        let fields: Vec<Field> = Self::airports_in(code, data, &mut BTreeMap::new())
            .into_iter()
            .filter(|f| !aliases.contains_key(&f.id))
            .collect();
        offered(&fields, false).1
    }
    /// Every airport of one theater layout, in layout order, with whether it
    /// is a short strip and its owner. `fitted`, agent decision 2026-09-30,
    /// for John's decision of the same day: an airport whose runway is under
    /// `tore_sim::airport::SHORT_STRIP_FT` is no ground start, for the player
    /// or any wing; nor, since slice AL1 (John, 2026-10-10), is a field of
    /// the other side ([`Field::offered_to`]). `definitions` caches, per
    /// object type, whether it is an airport and its runway length.
    fn aliases_in(code: &str, data: &BTreeMap<String, Vec<u8>>) -> BTreeMap<u32, u32> {
        let name = format!("{code}.MM");
        data.get(&name)
            .and_then(|b| tore_formats::mission::Layout::parse(&name, b).ok())
            .map(|l| tore_world::terrain::redrawn::pair_aliases(&l))
            .unwrap_or_default()
    }
    fn canonical_runway(&self, theater: usize, object: u32) -> u32 {
        self.airport_aliases
            .get(theater)
            .and_then(|a| a.get(&object))
            .copied()
            .unwrap_or(object)
    }
    fn airports_in(
        code: &str,
        data: &BTreeMap<String, Vec<u8>>,
        definitions: &mut BTreeMap<String, (bool, Option<f64>)>,
    ) -> Vec<Field> {
        let name = format!("{code}.MM");
        let mut fields = Vec::new();
        if let Some(bytes) = data.get(&name)
            && let Ok(layout) = tore_formats::mission::Layout::parse(&name, bytes)
        {
            for p in layout.placements {
                let (airport, length) =
                    *definitions.entry(p.object_type.clone()).or_insert_with(|| {
                        let airport = data
                            .get(&p.object_type)
                            .and_then(|b| tore_formats::static_object::Definition::parse(b).ok())
                            .is_some_and(|d| {
                                d.main_shape.is_some()
                                    && d.callbacks.iter().any(|c| c == "_STRIPProc")
                            });
                        let length = airport
                            .then(|| tore_world::terrain::strip_length_ft(data, &p.object_type))
                            .flatten();
                        (airport, length)
                    });
                if airport {
                    let redfor = p.redfor();
                    fields.push(Field {
                        id: 0x4000_0000 + p.key.ordinal,
                        name: p.name.unwrap_or(p.object_type),
                        // A strip whose shape cannot be measured stays listed.
                        short: length.is_some_and(tore_sim::airport::short_strip_length),
                        stol: length.is_some_and(tore_sim::stol::length),
                        redfor,
                    });
                }
            }
        }
        fields
    }
    /// Every airport of the theater at `index` (in [`Self::theater_codes`]
    /// order), short strips and both sides' fields included.
    pub fn theater_fields(&self, index: usize) -> &[Field] {
        self.fields.get(index).map_or(&[], Vec::as_slice)
    }
    /// The ground-start airports offered in the theater at `index`, by
    /// runway object, in the order the picker lists them.
    pub fn offered_airports(&self, index: usize) -> &[u32] {
        self.airport_objects.get(index).map_or(&[], Vec::as_slice)
    }
    /// The airports offered for an enemy ground start in the theater at
    /// `index`, by runway object, in the order the Enemy airport list
    /// shows them: the Red and neutral fields, STOL strips last.
    pub fn offered_enemy_airports(&self, index: usize) -> &[u32] {
        self.enemy_airport_objects
            .get(index)
            .map_or(&[], Vec::as_slice)
    }
    /// The creator's pilot flies for Redfor (`true`) or Blue: the airport
    /// lists become that side's and the neutral fields, and a ground start
    /// at a field the new side cannot use starts over at its first field.
    /// The enemy's lists (fields 35 and 36) are always Redfor's and stay.
    pub fn set_player_redfor(&mut self, redfor: bool) {
        self.player_redfor = redfor;
        let (names, objects) = self.fields.iter().map(|list| offered(list, redfor)).unzip();
        self.airport_names = names;
        self.airport_objects = objects;
        self.draft.values[34] = 0;
        if self.ground_unavailable() {
            self.draft.values[33] = 0;
        }
    }
    /// The selected theater has no airfield the pilot's side may start
    /// from, so Ground is not offered and the start is airborne.
    pub fn ground_unavailable(&self) -> bool {
        self.airport_objects
            .get(self.draft.values[13])
            .is_none_or(Vec::is_empty)
    }
    /// The selected theater has no Redfor or neutral airfield, so the
    /// enemy's Ground is not offered and the enemy starts airborne.
    pub fn enemy_ground_unavailable(&self) -> bool {
        self.enemy_airport_objects
            .get(self.draft.values[13])
            .is_none_or(Vec::is_empty)
    }
    /// Developer option: add one imported `~` layout variant (or, with
    /// `all`, every one) after the base theaters so a probe can run the
    /// creator on it. The player's theater list never contains them.
    pub fn add_developer_theaters(&mut self, data: &BTreeMap<String, Vec<u8>>, only: Option<&str>) {
        let catalog = tore_formats::theater::map_catalog(data).unwrap_or_default();
        let mut definitions = BTreeMap::new();
        for (code, label) in catalog
            .into_iter()
            .filter(|(code, _)| code.starts_with('~'))
        {
            if only.is_some_and(|only| only != code) || self.theater_codes.contains(&code) {
                continue;
            }
            let fields = Self::airports_in(&code, data, &mut definitions);
            let aliases = Self::aliases_in(&code, data);
            self.legacy_fields.push(fields.clone());
            let fields: Vec<_> = fields
                .into_iter()
                .filter(|f| !aliases.contains_key(&f.id))
                .collect();
            self.airport_aliases.push(aliases);
            let (names, ids) = offered(&fields, self.player_redfor);
            self.airport_names.push(names);
            self.airport_objects.push(ids);
            let (names, ids) = offered(&fields, true);
            self.enemy_airport_names.push(names);
            self.enemy_airport_objects.push(ids);
            self.fields.push(fields);
            self.theater_codes.push(code.clone());
            self.theater_catalog.push(code);
            self.options.fields[13].push(label);
        }
    }
    /// Select a theater by code, as `--theater CODE` does. A `~` variant is
    /// added on demand (developer option); an unknown code selects the first
    /// theater.
    pub fn choose_theater_code(&mut self, code: &str, data: &BTreeMap<String, Vec<u8>>) {
        if code.starts_with('~') {
            self.add_developer_theaters(data, Some(code));
        }
        let index = self
            .theater_catalog
            .iter()
            .position(|c| c == code)
            .unwrap_or(0);
        self.theater(index);
    }
    pub fn theater(&mut self, index: usize) {
        if self.selection != index {
            self.draft.values[34] = 0;
            self.draft.values[ENEMY_AIRPORT] = 0;
        }
        self.selection = index;
        if let Some(code) = self.theater_catalog.get(index) {
            self.draft.values[13] = self
                .theater_codes
                .iter()
                .position(|c| c == code)
                .unwrap_or(0);
        }
        // A theater with no Redfor field takes the enemy back to Airborne.
        if self.enemy_ground_unavailable() {
            self.draft.values[ENEMY_START] = 0;
        }
        self.nationalities();
        self.fit_distance();
    }
    fn nationalities(&mut self) {
        self.draft.values[3] = 0;
        self.draft.values[20] = enemy_nationality(self.base_theater_index());
    }
    /// Every imported string the creator can show, with where it comes from,
    /// for the `--validate-text` scan.
    pub fn imported_strings(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (id, list) in self.options.fields.iter().enumerate() {
            for text in list {
                out.push((format!("creator field {id}"), text.clone()));
            }
        }
        for text in &self.aircraft_names {
            out.push(("creator aircraft".into(), text.clone()));
        }
        for (theater, fields) in self.theater_codes.iter().zip(&self.fields) {
            for field in fields {
                out.push((format!("airport in {theater}"), field.label()));
            }
        }
        for (id, list) in self.options.targets.iter().enumerate() {
            for text in list {
                out.push((format!("ground target list {id}"), text.clone()));
            }
        }
        out
    }
    pub fn player(&self) -> Option<AircraftId> {
        self.aircraft_files
            .get(self.draft.values[6])
            .and_then(|n| AircraftId::parse(n).ok())
    }
    /// Rebuilds the AI's aircraft list from the player's.
    #[cfg(test)]
    fn refresh_wing_catalog(&mut self) {
        (self.wing_files, self.wing_names) = ai_catalog(&self.aircraft_files, &self.aircraft_names);
    }
    /// The aircraft the draft chose in an aircraft field (6, 9, 12, 23, 26 or
    /// 29), if it resolves to an import.
    fn wing_aircraft(&self, field: usize) -> Option<AircraftId> {
        let files = if field == 6 {
            &self.aircraft_files
        } else {
            &self.wing_files
        };
        files
            .get(self.draft.values[field])
            .and_then(|n| AircraftId::parse(n).ok())
    }
    /// The value that picks `id` in one of the AI's wing fields (9, 12, 23,
    /// 26 and 29), or `None` when the AI cannot fly it or it is not imported.
    pub fn ai_choice(&self, id: AircraftId) -> Option<usize> {
        self.wing_files.iter().position(|n| n == id.selection_key())
    }
    /// Why no AI wingman can share friendly wing 1 with the player: the
    /// player's aircraft is one the AI cannot fly. The words end "so
    /// friendly wing 1 ...". A helicopter's wingmen take off from a ground
    /// start too since slice H2 (vertical takeoffs).
    fn wingmen_refusal(&self) -> Option<String> {
        let player = self.player()?;
        if !player.ai_flyable() {
            return Some(format!("The AI cannot fly the {} yet", player.label()));
        }
        None
    }
    pub fn guns_only(&self) -> bool {
        self.draft.values[19] == 0
    }
    fn base_theater_index(&self) -> usize {
        self.base_theater_index_of(self.draft.values[13])
    }
    /// The base theater's place in the source list for the theater at
    /// `index` in the creator's list.
    fn base_theater_index_of(&self, index: usize) -> usize {
        let base = self
            .theater_codes
            .get(index)
            .and_then(|code| tore_formats::theater::base_theater(code));
        source_theaters()
            .iter()
            .position(|code| Some(*code) == base)
            .unwrap_or(0)
    }
    /// The theater code the creator has selected, from its own catalog. A
    /// flight's world lists only its own theater, so the code never comes
    /// from there.
    pub fn theater_code(&self) -> Option<&str> {
        self.theater_catalog
            .get(self.theater_index())
            .map(String::as_str)
    }
    pub fn theater_index(&self) -> usize {
        self.theater_codes
            .get(self.draft.values[13])
            .and_then(|code| self.theater_catalog.iter().position(|c| c == code))
            .unwrap_or(0)
    }
    fn values(&self, id: usize) -> Cow<'_, [String]> {
        if id == AI_AIRCRAFT {
            return Cow::Owned(vec!["Off".into(), "On".into()]);
        }
        if (CHOICE_BASE..CHOICE_BASE + 6).contains(&id) {
            return Cow::Owned(vec!["Fixed".into(), "Player choice".into()]);
        }
        Cow::Borrowed(if id == 6 {
            &self.aircraft_names
        } else if AI_AIRCRAFT_FIELDS.contains(&id) {
            &self.wing_names
        } else if id == 33 || id == ENEMY_START {
            &self.start_modes
        } else if id == 34 || id == ENEMY_AIRPORT {
            let names = if id == 34 {
                &self.airport_names[self.draft.values[13]]
            } else {
                &self.enemy_airport_names[self.draft.values[13]]
            };
            if self.has_target() {
                // The world's own pick first, then the theater's airports.
                let mut rows = Vec::with_capacity(names.len() + 1);
                rows.push(AUTO_AIRPORT.to_owned());
                rows.extend(names.iter().cloned());
                return Cow::Owned(rows);
            }
            names
        } else if id == 30 {
            &self.options.targets[self.base_theater_index()]
        } else {
            &self.options.fields[id]
        })
    }
    /// A friendly ground target is chosen (field 30 is not "none").
    fn has_target(&self) -> bool {
        self.draft.values[30] != 0
    }
    fn value(&self, id: usize) -> String {
        if self.view.is_some() {
            if self.lobby && NOT_CARRIED.contains(&id) {
                return AS_THE_KINGS.into();
            }
            if let Some(text) = self.shown.get(&id) {
                return text.clone();
            }
        }
        if let Some(group) = id
            .checked_sub(OBJECTIVE_BASE)
            .filter(|group| *group < OBJECTIVE_COUNT)
        {
            return self.objective_label(group);
        }
        if let Some(group) = id
            .checked_sub(SURVIVAL_BASE)
            .filter(|group| *group < OBJECTIVE_COUNT)
        {
            return if self.group_must_survive[group] {
                "required"
            } else {
                "optional"
            }
            .into();
        }
        self.values(id)
            .get(self.draft.values[id])
            .cloned()
            .unwrap_or_else(|| "Unavailable".into())
    }
    pub(crate) fn objective_choices(group: usize) -> Vec<(String, GroupObjective)> {
        let side = if group < 3 {
            Side::Friendly
        } else {
            Side::Enemy
        };
        let wing = group % 3;
        let opposing = if side == Side::Friendly {
            Side::Enemy
        } else {
            Side::Friendly
        };
        let mut choices = vec![
            ("Use mission setting".into(), GroupObjective::Inherit),
            (
                "Free fire (any opposing group)".into(),
                GroupObjective::Free,
            ),
            ("Combat air patrol".into(), GroupObjective::Cap),
        ];

        for index in 0..3 {
            let target = WingId::new(opposing, index).expect("fixed Quick Mission wing");
            choices.push((
                format!(
                    "Primary target: {} group {}",
                    if opposing == Side::Enemy {
                        "enemy"
                    } else {
                        "friendly"
                    },
                    index + 1
                ),
                GroupObjective::Intercept(target),
            ));
        }
        for index in 0..3 {
            if index != wing {
                let target = WingId::new(side, index as u8).expect("fixed Quick Mission wing");
                choices.push((
                    format!(
                        "Protect {} group {}",
                        if side == Side::Friendly {
                            "friendly"
                        } else {
                            "enemy"
                        },
                        index + 1
                    ),
                    GroupObjective::Escort(target),
                ));
            }
        }
        choices.extend([
            ("Self-defense".into(), GroupObjective::SelfDefense),
            ("Weapons hold".into(), GroupObjective::Hold),
        ]);
        // Friendly wings other than the player's may strike the ground
        // target (docs/spec/ai-strike.md, "Who strikes"); it takes effect
        // once a ground target is chosen. Redfor has no ground objectives.
        // Last in the list, so the other choices keep their numbers
        // (`--probe-group` names a choice by its place).
        if side == Side::Friendly && wing > 0 {
            choices.push(("Strike the ground target".into(), GroupObjective::Strike));
        }
        choices
    }
    fn objective_prefix(&self, group: usize) -> String {
        let primary = matches!(self.group_objectives[group], GroupObjective::Intercept(_));
        match (group, primary) {
            (0, true) => "Your primary target is ".into(),
            (0, false) => "Your flight will ".into(),
            (_, true) => format!("Wing {}'s primary target is ", group % 3 + 1),
            (_, false) => format!("Wing {} will ", group % 3 + 1),
        }
    }
    fn objective_label(&self, group: usize) -> String {
        match self.group_objectives[group] {
            GroupObjective::Inherit => match self.ai_mission {
                crate::ai_wings::Preset::Free => "use free fire (mission)".into(),
                crate::ai_wings::Preset::Cap => "patrol (mission)".into(),
                crate::ai_wings::Preset::SelfDefense => "defend itself (mission)".into(),
                crate::ai_wings::Preset::Hold => "hold fire (mission)".into(),
                _ => "follow mission orders".into(),
            },
            GroupObjective::Free => "use free fire".into(),
            GroupObjective::Cap => "patrol the area".into(),
            GroupObjective::Intercept(wing) => format!(
                "{} group {}",
                if wing.side == Side::Enemy {
                    "enemy"
                } else {
                    "friendly"
                },
                wing.display_number()
            ),
            GroupObjective::Escort(wing) => format!(
                "protect {} group {}",
                if wing.side == Side::Friendly {
                    "friendly"
                } else {
                    "enemy"
                },
                wing.display_number()
            ),
            GroupObjective::SelfDefense => "defend itself".into(),
            GroupObjective::Hold => "hold fire".into(),
            GroupObjective::Strike => "strike the ground target".into(),
        }
    }
    fn selector_values(&self, id: usize) -> Vec<String> {
        if let Some(group) = id
            .checked_sub(OBJECTIVE_BASE)
            .filter(|g| *g < OBJECTIVE_COUNT)
        {
            Self::objective_choices(group)
                .into_iter()
                .map(|(label, _)| label)
                .collect()
        } else {
            let values = self.values(id);
            if id == 17 {
                // Only the distances the table offers here are listed.
                return self
                    .offered_distances()
                    .into_iter()
                    .filter_map(|index| values.get(index).cloned())
                    .collect();
            }
            values.into_owned()
        }
    }
    /// The six wing rows as the AI launch payload: side, aircraft, wing skill
    /// and one entry per member (`docs/spec/ai-experience.md`, "Experience
    /// channels"). Wing counts live in fields 4, 7, 10 (friendly) and 21, 24,
    /// 27 (enemy); each wing's skill is the next field and its aircraft the one
    /// after that. Friendly wing 1 loses one slot to the player. A wing whose
    /// aircraft choice does not resolve to a supported import is skipped, which
    /// `unsupported` reports separately before a mission may start.
    pub fn wing_launches(
        &self,
        enemy_override: Option<EnemySkillOverride>,
    ) -> Result<Vec<WingLaunch>, AiError> {
        let mut selections = Vec::with_capacity(6);
        for (field, side, index) in [
            (4, Side::Friendly, 0),
            (7, Side::Friendly, 1),
            (10, Side::Friendly, 2),
            (21, Side::Enemy, 0),
            (24, Side::Enemy, 1),
            (27, Side::Enemy, 2),
        ] {
            let Some(aircraft) = self.wing_aircraft(field + 2) else {
                continue;
            };
            selections.push(WingSelection {
                wing: WingId::new(side, index)?,
                aircraft,
                count: self.draft.values[field].saturating_sub(usize::from(field == 4)),
                skill_level: self.draft.values[field + 1] as i32,
            });
        }
        resolve_wings(&selections, enemy_override)
    }
    /// The creator's draft as a mission with stable names: the theater code,
    /// the condition, the start, the six wings by their aircraft keys, the
    /// standing orders and the groups' objectives. What belongs to the
    /// session rather than the creator (the flight models, the enemy skill
    /// override, the weather overrides, the cheats, the loadout) keeps the
    /// spec's defaults for the caller to set.
    pub fn mission_spec(&self) -> Result<MissionSpec, String> {
        let v = &self.draft.values;
        let player = self.player().ok_or("Choose an imported aircraft to fly.")?;
        let theater = self
            .theater_codes
            .get(v[13])
            .ok_or("Choose a theater.")?
            .clone();
        let mut spec = MissionSpec::new(&theater, player);
        spec.condition = condition(v[15])
            .and_then(Condition::from_index)
            .ok_or("Choose one of the six available weather conditions.")?;
        let altitude_ft = *ALTITUDES_FT
            .get(v[14])
            .ok_or("Choose one of the four altitudes.")?;
        spec.start = match self.ground_runway() {
            Some(runway) => Start::Ground {
                runway,
                altitude_ft,
            },
            None if self.ground_auto() => Start::GroundAuto { altitude_ft },
            None => Start::Airborne { altitude_ft },
        };
        spec.separation_nm = self.separation_nm() as u32;
        spec.situation = self.situation();
        spec.preset = self.ai_mission;
        spec.guns_only = self.guns_only();
        for (index, field) in [4, 7, 10, 21, 24, 27].into_iter().enumerate() {
            // A wing whose aircraft choice does not resolve keeps the
            // player's type; `unsupported` refuses to fly one with aircraft.
            let aircraft = self.wing_aircraft(field + 2).unwrap_or(player);
            spec.wings[index] = WingSpec {
                aircraft,
                count: v[field].max(usize::from(index == 0)),
                skill: Skill::from_level(v[field + 1] as i32).ok_or("Choose a pilot skill.")?,
            };
        }
        spec.player_choice = std::array::from_fn(|i| self.lobby && v[CHOICE_BASE + i] != 0);
        spec.enemy_start = self.enemy_start();
        // A strike needs a ground target to strike; without one the wing
        // flies the mission's own orders.
        let has_target = self.has_target();
        spec.objectives = self.group_objectives.map(|objective| {
            if objective == GroupObjective::Strike && !has_target {
                GroupObjective::Inherit
            } else {
                objective
            }
        });
        spec.must_survive = self.group_must_survive;
        // The ground target, by its template's stem. The defenses go with
        // it, and with no target they are the enemy's area defenses (B23,
        // John 2026-10-12).
        spec.aaa = Defense::from_level(v[31]).ok_or("Choose an AAA strength.")?;
        spec.sam = Defense::from_level(v[32]).ok_or("Choose a SAM strength.")?;
        if self.has_target() {
            spec.ground_target = Some(
                ground_targets(&theater)
                    .get(v[30] - 1)
                    .ok_or("Choose one of this theater's ground targets.")?
                    .to_string(),
            );
        }
        spec.enemy_nationality = u8::try_from(v[20])
            .ok()
            .filter(|n| *n < tore_world::mission::NATIONALITIES)
            .ok_or("Choose one of the enemy nationalities.")?;
        spec.surface_seed = self.surface_seed;
        Ok(spec)
    }
    /// The creator's ground target (field 30) by its template's stem, with
    /// the AAA and SAM levels (fields 31 and 32, each 0 to 3) and the layout's
    /// seed: what `--ground-target STEM --defenses AAA SAM --surface-seed N`
    /// set for a launched flight or a capture.
    pub fn choose_ground_target(
        &mut self,
        stem: &str,
        (aaa, sam): (usize, usize),
        seed: u32,
    ) -> Result<(), String> {
        let theater = self
            .theater_codes
            .get(self.draft.values[13])
            .ok_or("Choose a theater.")?
            .clone();
        let stem = stem.trim_start_matches('~').to_ascii_uppercase();
        let index = ground_targets(&theater)
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(&stem))
            .ok_or_else(|| {
                format!(
                    "{theater} has no ground target {stem}: {}",
                    ground_targets(&theater).join(" ")
                )
            })?;
        self.draft.values[30] = index + 1;
        self.draft.values[31] = aaa;
        self.draft.values[32] = sam;
        self.surface_seed = seed;
        // A given seed is the next flight's (and the map's).
        self.seed_pending = seed != 0;
        self.fit_distance();
        Ok(())
    }
    /// Developer launch/capture settings for area defenses with no target.
    pub fn choose_area_defenses(&mut self, (aaa, sam): (usize, usize), seed: u32) {
        self.apply(30, 0);
        self.draft.values[31] = aaa;
        self.draft.values[32] = sam;
        self.surface_seed = seed;
        self.seed_pending = seed != 0;
    }
    /// The flight lays out a surface from the seed: a ground target, or with
    /// none an AAA or SAM strength above none (the area defenses).
    fn lays_out_surface(&self) -> bool {
        self.has_target() || self.draft.values[31] != 0 || self.draft.values[32] != 0
    }
    /// Draws the surface layout's seed for the flight about to start, when it
    /// has a ground target or area defenses to lay out. Kept until the next flight, so a
    /// restart reuses it. `fitted`: any nonzero 32-bit value; the clock and
    /// the process make it, as nothing here has to be reproducible.
    pub fn roll_surface_seed(&mut self) {
        // The map drew this flight's already: keep it (slice CL-R).
        if std::mem::take(&mut self.seed_pending) && self.lays_out_surface() {
            return;
        }
        self.surface_seed = if self.lays_out_surface() {
            fresh_seed()
        } else {
            0
        };
    }
    /// The aircraft/count pairs the current mission spawner still takes. This
    /// is the launch payload with side, member and experience dropped; it stays
    /// for the existing spawner call and must not grow new callers.
    ///
    /// `fitted`: an out-of-range wing skill or count would make the payload an
    /// error, and this infallible signature has nowhere to put one, so it
    /// launches nothing. Rule: the decoded setup screen offers four
    /// experience levels plus Dummy and counts 0 through 5 per wing (`docs/formats/quick-mission.md`),
    /// so the menu cannot reach that state; the empty result is a visible
    /// failure rather than a silent clamp if it ever does.
    #[cfg(test)]
    pub fn dummy_wings(&self) -> Vec<(AircraftId, usize)> {
        self.wing_launches(None)
            .map(|wings| tore_sim::ai::launch::legacy_pairs(&wings))
            .unwrap_or_default()
    }
    pub fn ground_start(&self) -> bool {
        self.draft.values[33] == 1
    }
    /// The runway the draft chose by name. With a ground target the Airport
    /// list's first row is the world's own pick (`ground_auto`), and the
    /// airports follow it.
    pub fn ground_runway(&self) -> Option<u32> {
        self.ground_start()
            .then(|| {
                let at = self.draft.values[34].checked_sub(usize::from(self.has_target()))?;
                self.airport_objects[self.draft.values[13]].get(at).copied()
            })
            .flatten()
    }
    /// A ground start whose runway the world picks from the ground target
    /// (`start ground auto`): the default when a target is set and no runway
    /// was picked.
    pub fn ground_auto(&self) -> bool {
        self.ground_start() && self.has_target() && self.draft.values[34] == 0
    }
    pub fn choose_ground_runway(&mut self, object: u32) -> Result<(), String> {
        let object = self.canonical_runway(self.draft.values[13], object);
        if let Some(field) = self
            .fields
            .get(self.draft.values[13])
            .and_then(|list| list.iter().find(|field| field.id == object))
        {
            let name = &field.name;
            if field.enemy_of(self.player_redfor) {
                return Err(format!(
                    "{name} is an enemy airfield: no ground start there. Choose one of your side's airfields or Airborne."
                ));
            }
        }
        let index = self.airport_objects[self.draft.values[13]]
            .iter()
            .position(|id| *id == object)
            .ok_or_else(|| "No imported runway matches the chosen airport".to_string())?;
        self.apply(33, 1);
        self.apply(34, index + usize::from(self.has_target()));
        Ok(())
    }
    /// The enemy starts on the ground (field 35 is Ground): Enemy Wing 1
    /// is parked at a Redfor or neutral airfield.
    pub fn enemy_ground_start(&self) -> bool {
        self.draft.values[ENEMY_START] == 1
    }
    /// The runway the draft chose by name for the enemy's ground start. As
    /// with [`Self::ground_runway`], a ground target puts the world's own
    /// pick first in the Enemy airport list and the airports follow it.
    pub fn enemy_ground_runway(&self) -> Option<u32> {
        self.enemy_ground_start()
            .then(|| {
                let at =
                    self.draft.values[ENEMY_AIRPORT].checked_sub(usize::from(self.has_target()))?;
                self.enemy_airport_objects[self.draft.values[13]]
                    .get(at)
                    .copied()
            })
            .flatten()
    }
    /// An enemy ground start whose runway the world picks from the ground
    /// target (`enemy-start ground auto`): the default when a target is set
    /// and no runway was picked.
    pub fn enemy_ground_auto(&self) -> bool {
        self.enemy_ground_start() && self.has_target() && self.draft.values[ENEMY_AIRPORT] == 0
    }
    /// Where the draft starts Enemy Wing 1, as the mission carries it.
    pub fn enemy_start(&self) -> EnemyStart {
        match self.enemy_ground_runway() {
            Some(runway) => EnemyStart::Ground { runway },
            None if self.enemy_ground_auto() => EnemyStart::GroundAuto,
            None => EnemyStart::Airborne,
        }
    }
    /// Starts the enemy on the ground at the airport of runway `object`, as
    /// the Enemy airport list would. A Blue field is refused with the reason,
    /// as [`Self::choose_ground_runway`] does; a STOL strip is allowed.
    pub fn choose_enemy_runway(&mut self, object: u32) -> Result<(), String> {
        let object = self.canonical_runway(self.draft.values[13], object);
        if let Some(field) = self
            .fields
            .get(self.draft.values[13])
            .and_then(|list| list.iter().find(|field| field.id == object))
        {
            let name = &field.name;
            if !field.offered_to(true) {
                return Err(format!(
                    "{name} is a Blue airfield: no enemy ground start there. Choose a Redfor airfield or start the enemy Airborne."
                ));
            }
        }
        let index = self.enemy_airport_objects[self.draft.values[13]]
            .iter()
            .position(|id| *id == object)
            .ok_or_else(|| "No imported runway matches the chosen enemy airport".to_string())?;
        self.apply(ENEMY_START, 1);
        self.apply(ENEMY_AIRPORT, index + usize::from(self.has_target()));
        Ok(())
    }
    /// The chosen enemy separation in nautical miles (manual p.19). An index
    /// outside the table falls back to the 5 mile default instead of failing.
    pub fn separation_nm(&self) -> f64 {
        SEPARATION_NM
            .get(self.draft.values[17])
            .copied()
            .unwrap_or(SEPARATION_NM[Draft::default().values[17]])
    }
    pub fn separation_feet(&self) -> f64 {
        self.separation_nm() * FEET_PER_NM
    }
    /// The creator's situation (field 16); neutral for an index outside
    /// the list.
    pub fn situation(&self) -> Situation {
        Situation::from_index(self.draft.values[16]).unwrap_or_default()
    }
    /// Aircraft in the player's wing, the player included.
    pub fn player_wing_size(&self) -> usize {
        self.draft.values[4].clamp(1, 5)
    }
    pub fn unsupported(&self) -> Option<String> {
        if self.player().is_none() {
            return Some(
                "This aircraft is available for setup only. Choose an imported aircraft from the player list to fly."
                    .into(),
            );
        }
        if self.ground_start() && self.ground_runway().is_none() && !self.ground_auto() {
            return Some(
                "No imported runways are available in this theater. Choose Airborne.".into(),
            );
        }
        if self.enemy_ground_start()
            && self.enemy_ground_runway().is_none()
            && !self.enemy_ground_auto()
        {
            return Some(
                "No imported Redfor runways are available in this theater. Start the enemy Airborne."
                    .into(),
            );
        }
        let v = &self.draft.values;
        if self.enemy_ground_start() && v[21] == 0 {
            return Some(EMPTY_ENEMY_WING.into());
        }
        // Only a neutral field is offered to both sides.
        if let Some(runway) = self.ground_runway()
            && self.enemy_ground_runway() == Some(runway)
        {
            return Some(
                "You and the enemy cannot start on the same airfield. Choose another airport for one of you."
                    .into(),
            );
        }
        for field in [4, 7, 10, 21, 24, 27] {
            if v[field] > 0 && self.wing_aircraft(field + 2).is_none() {
                return Some(
                    "Choose a supported imported aircraft for every populated wing.".into(),
                );
            }
        }
        if !self.lobby
            && v[4] > 1
            && let Some(why) = self.wingmen_refusal()
        {
            return Some(format!(
                "{why}, so friendly wing 1 can only hold you. Set friendly wing 1 to one aircraft."
            ));
        }
        if condition(v[15]).is_none() {
            return Some("Choose one of the six available weather conditions.".into());
        }
        None
    }
    /// A sample of gaps for the previews: the second and fourth aircraft and
    /// the third theater, each as Hawk's game lacking it.
    fn preview_gaps(&mut self) {
        let mut gaps = BTreeMap::new();
        for index in [1, 3] {
            if let (Some(key), Some(label)) = (
                self.aircraft_files.get(index),
                self.aircraft_names.get(index),
            ) {
                gaps.insert(
                    (ItemKind::Aircraft, key.clone()),
                    format!("Not everyone can fly the {label}: Hawk's game has no {label}."),
                );
            }
        }
        if let Some(code) = self.theater_codes.get(2) {
            gaps.insert(
                (ItemKind::Theater, code.clone()),
                format!("Not everyone has {code}: Hawk's game has no {code}."),
            );
        }
        self.gaps = gaps;
    }
    /// What the lobby's Cancel puts back.
    pub fn save(&self) -> Saved {
        Saved {
            draft: self.draft.clone(),
            group_objectives: self.group_objectives,
            group_must_survive: self.group_must_survive,
            ai_mission: self.ai_mission,
        }
    }
    pub fn restore(&mut self, saved: Saved) {
        self.draft = saved.draft;
        self.group_objectives = saved.group_objectives;
        self.group_must_survive = saved.group_must_survive;
        self.ai_mission = saved.ai_mission;
        self.aircraft_selection = self.draft.values[6];
        self.selection = self.theater_index();
        self.fit_distance();
    }
    /// A spec's draft, and the values its lists cannot hold, by field, in
    /// words (see [`Draft::from_spec`]). The creator's draft is untouched.
    fn read_spec(&self, spec: &MissionSpec) -> Result<(Draft, Unshown), String> {
        let mut draft = Draft::default();
        let mut unshown = BTreeMap::new();
        let v = &mut draft.values;
        let theater = self
            .theater_codes
            .iter()
            .position(|code| *code == spec.theater)
            .ok_or_else(|| format!("This creator has no theater {}.", spec.theater))?;
        v[13] = theater;
        v[3] = 0;
        v[20] = if usize::from(spec.enemy_nationality) < self.options.fields[20].len() {
            usize::from(spec.enemy_nationality)
        } else {
            unshown.insert(
                20,
                (
                    spec.enemy_nationality.to_string(),
                    format!("enemy nationality {}", spec.enemy_nationality),
                ),
            );
            enemy_nationality(self.base_theater_index_of(theater))
        };
        v[31] = spec.aaa.level();
        v[32] = spec.sam.level();
        if let Some(stem) = &spec.ground_target {
            let offered = self.options.targets[self.base_theater_index_of(theater)].len();
            match ground_targets(&spec.theater)
                .iter()
                .position(|offer| offer == stem)
                .map(|at| at + 1)
                .filter(|at| *at < offered)
            {
                Some(at) => v[30] = at,
                None => {
                    unshown.insert(30, (stem.clone(), format!("the ground target {stem}")));
                }
            }
        }
        // The two ground starts, after the target: with one, each Airport
        // list begins with the world's own pick.
        let player_runway = match spec.start {
            Start::Airborne { .. } => None,
            Start::Ground { runway, .. } => Some(Some(runway)),
            Start::GroundAuto { .. } => Some(None),
        };
        if let Some(runway) = player_runway {
            self.read_ground_start(
                &mut draft,
                &mut unshown,
                (33, 34),
                runway,
                self.offered_airports(theater),
            );
        }
        let enemy_runway = match spec.enemy_start {
            EnemyStart::Airborne => None,
            EnemyStart::Ground { runway } => Some(Some(runway)),
            EnemyStart::GroundAuto => Some(None),
        };
        if let Some(runway) = enemy_runway {
            self.read_ground_start(
                &mut draft,
                &mut unshown,
                (ENEMY_START, ENEMY_AIRPORT),
                runway,
                self.offered_enemy_airports(theater),
            );
        }
        let v = &mut draft.values;
        v[15] = (0..6)
            .find(|index| condition(*index).and_then(Condition::from_index) == Some(spec.condition))
            .ok_or("This creator cannot show the mission's weather.")?;
        v[14] = ALTITUDES_FT
            .iter()
            .position(|feet| *feet == spec.start.altitude_ft())
            .ok_or("This creator has no such altitude.")?;
        v[17] = SEPARATION_NM
            .iter()
            .position(|nm| *nm == f64::from(spec.separation_nm))
            .ok_or_else(|| format!("This creator has no {} mile distance.", spec.separation_nm))?;
        v[16] = spec.situation.index();
        v[19] = usize::from(!spec.guns_only);
        for (index, field) in [4, 7, 10, 21, 24, 27].into_iter().enumerate() {
            v[CHOICE_BASE + index] = usize::from(spec.player_choice[index]);
            let wing = spec.wings[index];
            if wing.count < self.options.fields[field].len() {
                v[field] = wing.count;
            } else {
                v[field] = usize::from(index == 0);
                unshown.insert(
                    field,
                    (
                        wing.count.to_string(),
                        format!("a wing of {} aircraft", wing.count),
                    ),
                );
            }
            v[field + 1] = usize::try_from(wing.skill.level()).unwrap_or(0);
            let aircraft_field = field + 2;
            let files = if aircraft_field == 6 {
                &self.aircraft_files
            } else {
                &self.wing_files
            };
            match files
                .iter()
                .position(|key| key == wing.aircraft.selection_key())
            {
                Some(at) => v[aircraft_field] = at,
                None => {
                    v[aircraft_field] = 0;
                    unshown.insert(
                        aircraft_field,
                        (
                            wing.aircraft.label().to_owned(),
                            format!("the {}", wing.aircraft.label()),
                        ),
                    );
                }
            }
        }
        Ok((draft, unshown))
    }
    /// Reads a ground start into its Start and Airport fields (33 and 34,
    /// or 35 and 36): `runway` is the chosen runway, `None` the world's own
    /// pick, and `offered` the Airport list's runways. A runway the list
    /// does not hold, or an automatic pick the list cannot show (no ground
    /// target, or no airfield to pick from), goes to `unshown`.
    fn read_ground_start(
        &self,
        draft: &mut Draft,
        unshown: &mut Unshown,
        (start, airport): (usize, usize),
        runway: Option<u32>,
        offered: &[u32],
    ) {
        let v = &mut draft.values;
        let target = usize::from(v[30] != 0);
        v[start] = 1;
        v[airport] = 0;
        let runway = runway.map(|id| self.canonical_runway(v[13], id));
        match runway {
            Some(runway) => match offered.iter().position(|id| *id == runway) {
                Some(at) => v[airport] = at + target,
                None => {
                    let name = self
                        .theater_fields(v[13])
                        .iter()
                        .find(|field| field.id == runway)
                        .map_or_else(|| format!("runway {runway}"), |field| field.name.clone());
                    let words = format!("a ground start at {name}");
                    unshown.insert(airport, (name, words));
                }
            },
            None if target == 0 => {
                unshown.insert(
                    airport,
                    (
                        AUTO_AIRPORT.into(),
                        "an automatic ground start with no ground target".into(),
                    ),
                );
            }
            None if offered.is_empty() => {
                unshown.insert(
                    airport,
                    (
                        AUTO_AIRPORT.into(),
                        "an automatic ground start in a theater with no airfield for it".into(),
                    ),
                );
            }
            None => {}
        }
    }
    /// Puts a mission spec in the creator: the draft, the groups' orders and
    /// the mission-wide setting. With `exact`, a spec the lists cannot show
    /// is refused (an editable creator would send the wrong mission back);
    /// without it those values are kept to be drawn as they are (the
    /// read-only creator). Nothing changes when it fails.
    pub fn load_spec(&mut self, spec: &MissionSpec, exact: bool) -> Result<(), String> {
        let (draft, shown) = if exact {
            (Draft::from_spec(spec, self)?, Unshown::new())
        } else {
            self.read_spec(spec)?
        };
        self.draft = draft;
        self.shown = shown
            .into_iter()
            .map(|(field, (text, _))| (field, text))
            .collect();
        self.surface_seed = spec.surface_seed;
        self.seed_pending = spec.surface_seed != 0;
        self.group_objectives = spec.objectives;
        self.group_must_survive = spec.must_survive;
        self.ai_mission = spec.preset;
        self.aircraft_selection = self.draft.values[6];
        self.selection = self.theater_index();
        Ok(())
    }
    /// Opens the creator on the lobby's mission `spec`: the King's, which
    /// edits it (`view` is `None`), or read-only (`view` says why). The
    /// King's draft is the lobby's mission, not the King's last one.
    pub fn open_lobby_mission(
        &mut self,
        spec: &MissionSpec,
        view: Option<ViewKind>,
    ) -> Result<(), String> {
        self.load_spec(spec, view.is_none())?;
        self.enter_lobby();
        if let Some(kind) = view {
            self.view = Some(kind);
            self.focus = OK;
            self.notice = Some(match kind {
                ViewKind::Reader => VIEW_NOTICE.into(),
                ViewKind::Flying => format!("{VIEW_NOTICE} {VIEW_FLYING}"),
                ViewKind::Fixed => format!("{VIEW_NOTICE} {VIEW_FIXED}"),
            });
        }
        Ok(())
    }
    /// `spec` (the creator's mission, [`QuickMission::lobby_spec`]) laid over
    /// `lobby`, the mission the lobby has now: what the creator edits comes
    /// from `spec`, the rest (the cheats, the weather overrides, the flight
    /// models) stays as the lobby has it, so Accept does not undo it.
    pub fn lay_over(lobby: &MissionSpec, spec: MissionSpec) -> MissionSpec {
        let mut over = lobby.clone();
        over.theater = spec.theater;
        over.condition = spec.condition;
        over.start = spec.start;
        over.enemy_start = spec.enemy_start;
        over.separation_nm = spec.separation_nm;
        over.situation = spec.situation;
        over.preset = spec.preset;
        over.guns_only = spec.guns_only;
        over.wings = spec.wings;
        over.player_choice = spec.player_choice;
        let planes = over.open_planes();
        let choices = over.player_choice;
        over.plane_aircraft.retain(|plane, _| {
            planes
                .iter()
                .any(|p| p.id == *plane && choices[MissionSpec::wing_index(p.wing)])
        });
        over.objectives = spec.objectives;
        over.must_survive = spec.must_survive;
        // The creator owns the target, defenses and the seed it audited.
        over.surface_seed = spec.surface_seed;
        over.ground_target = spec.ground_target;
        over.aaa = spec.aaa;
        over.sam = spec.sam;
        over.enemy_nationality = spec.enemy_nationality;
        // A lobby's mission carries no loadouts: each player arms their own.
        over.loadout = None;
        over.plane_loadouts.clear();
        over
    }
    /// The read-only creator redraws from the King's new mission and says so.
    pub fn reload_view(&mut self, spec: &MissionSpec) -> Result<(), String> {
        self.load_spec(spec, false)?;
        self.cancel();
        self.notice = Some(VIEW_CHANGED.into());
        Ok(())
    }
    /// The creator is the lobby's mission read-only.
    pub fn is_view(&self) -> bool {
        self.view.is_some()
    }
    /// Opens the creator for a multiplayer lobby (EF8), and the notice says
    /// what Accept does. Start and Enemy start keep their ground choices:
    /// agent decision, slice CL-R, 2026-10-11 (John's 2026-09-28 rule
    /// "everyone starts airborne" is superseded by his 2026-10-11 request
    /// for Redfor ground starts that match the creator's styles).
    pub fn enter_lobby(&mut self) {
        self.lobby = true;
        self.drop_map();
        self.cancel();
        self.focus = 3;
        self.notice = Some(LOBBY_NOTICE.into());
    }
    /// Closes the lobby's creator.
    pub fn leave_lobby(&mut self) {
        self.lobby = false;
        self.view = None;
        self.shown.clear();
        // A map left open, or still building, when the host started the
        // flight must not reappear over the next creator.
        self.drop_map();
        self.cancel();
        self.notice = None;
        self.gaps.clear();
    }
    /// Takes the items not every player has (stage L, slice L4): `refusal`
    /// gives the host's words for an item of a kind and key when it is in a
    /// gap now (`Client::gap_refusal`). The creator asks it about each
    /// aircraft and theater it offers; the list dims those and a choice of
    /// one says why it cannot be made. Only the lobby's creator uses it.
    pub fn set_gaps(&mut self, refusal: impl Fn(ItemKind, &str) -> Option<String>) {
        let mut gaps = BTreeMap::new();
        for key in &self.aircraft_files {
            if let Some(words) = refusal(ItemKind::Aircraft, key) {
                gaps.insert((ItemKind::Aircraft, key.clone()), words);
            }
        }
        for code in self.theater_codes.iter().filter(|c| !c.starts_with('~')) {
            if let Some(words) = refusal(ItemKind::Theater, code) {
                gaps.insert((ItemKind::Theater, code.clone()), words);
            }
        }
        self.gaps = gaps;
    }
    /// Why the choice of value `index` in `field` cannot be made, in the
    /// host's words, when the creator is the lobby's and the aircraft or
    /// theater is in a gap.
    fn gap_in(&self, field: usize, index: usize) -> Option<&str> {
        // Ground in a theater with no airfield of the pilot's side is dimmed
        // and refused the same way, with its own words (slice AL1).
        if field == 33 && index == 1 && self.ground_unavailable() {
            return Some(NO_FIELD_NOTICE);
        }
        // And the enemy's Ground with no Redfor or neutral field (slice CL-R).
        if field == ENEMY_START && index == 1 && self.enemy_ground_unavailable() {
            return Some(NO_ENEMY_FIELD_NOTICE);
        }
        if !self.lobby {
            return None;
        }
        let (kind, key) = if field == 6 {
            (ItemKind::Aircraft, self.aircraft_files.get(index)?)
        } else if AI_AIRCRAFT_FIELDS.contains(&field) {
            (ItemKind::Aircraft, self.wing_files.get(index)?)
        } else if field == 13 {
            (ItemKind::Theater, self.theater_codes.get(index)?)
        } else {
            return None;
        };
        self.gaps.get(&(kind, key.clone())).map(String::as_str)
    }
    /// A choice in a gap leaves the draft as it was and puts the host's
    /// words in the notice. True when it was refused.
    fn refuse_gap(&mut self, field: usize, index: usize) -> bool {
        let Some(words) = self.gap_in(field, index).map(str::to_owned) else {
            return false;
        };
        self.notice = Some(words);
        true
    }
    /// What the host would refuse in the draft for a gap: the first of the
    /// player's aircraft, each populated wing's and the theater that is in
    /// one, in the host's words.
    fn gap_problem(&self) -> Option<String> {
        let v = &self.draft.values;
        let wings = [4, 7, 10, 21, 24, 27]
            .into_iter()
            .filter(|field| v[*field] > 0 || *field == 4)
            .map(|field| (field + 2, v[field + 2]));
        std::iter::once((6, v[6]))
            .chain(wings)
            .chain(std::iter::once((13, v[13])))
            .find_map(|(field, index)| self.gap_in(field, index))
            .map(str::to_owned)
    }
    /// What a host will not take from the creator, in words (EF8): what
    /// single player cannot fly either, and a developer theater layout,
    /// which the mission's text form cannot name.
    pub fn lobby_problem(&self) -> Option<String> {
        if let Some(problem) = self.unsupported() {
            return Some(problem);
        }
        if self
            .theater_codes
            .get(self.draft.values[13])
            .is_some_and(|code| code.starts_with('~'))
        {
            return Some(
                "A developer theater layout cannot be hosted. Choose one of the sixteen theaters."
                    .into(),
            );
        }
        self.gap_problem()
    }
    /// The mission a host takes from this draft: [`QuickMission::mission_spec`]
    /// with both starts as chosen (agent decision, slice CL-R, 2026-10-11),
    /// with what the lobby refuses reported first.
    pub fn lobby_spec(&self) -> Result<MissionSpec, String> {
        if let Some(problem) = self.lobby_problem() {
            return Err(problem);
        }
        let spec = self.mission_spec()?;
        // Keep the seed whose placements and distance choices were checked.
        // The host reads the mission as text: what the text cannot carry
        // would silently change on the way.
        match MissionSpec::from_text(&spec.to_text()) {
            Ok(read) if read == spec => Ok(spec),
            Ok(_) => Err("This mission cannot be sent to a host as it is.".into()),
            Err(error) => Err(error.to_string()),
        }
    }
    fn apply(&mut self, id: usize, value: usize) {
        if self.resources.is_some() && matches!(id, 13 | 30) {
            self.surface_seed = 0;
            self.seed_pending = false;
        }
        let had_target = self.has_target();
        self.draft.values[id] = value;
        self.draft.values[4] = self.draft.values[4].max(1);
        if id == 13 {
            self.draft.values[34] = 0;
            self.draft.values[ENEMY_AIRPORT] = 0;
            self.draft.values[30] = 0;
            self.nationalities();
        }
        // The Airport lists gain their first row (the world's pick) with a
        // target, so their index starts over when a target comes or goes.
        if id == 30 && had_target != self.has_target() {
            self.draft.values[34] = 0;
            self.draft.values[ENEMY_AIRPORT] = 0;
        }
        // Retail clears both defenses with the target; here they stay, as
        // the enemy's area defenses (B23, agent decision 2026-10-12).
        self.aircraft_selection = self.draft.values[6];
        self.selection = self.theater_index();
        self.notice = None;
        // No airfield of the pilot's side in the theater: Ground is not
        // offered and the start stays airborne (slice AL1).
        if self.draft.values[33] == 1 && self.ground_unavailable() {
            self.draft.values[33] = 0;
            self.draft.values[34] = 0;
            self.notice = Some(NO_FIELD_NOTICE.into());
        }
        // No Redfor or neutral airfield: the enemy starts airborne too.
        if self.enemy_ground_start() && self.enemy_ground_unavailable() {
            self.draft.values[ENEMY_START] = 0;
            self.draft.values[ENEMY_AIRPORT] = 0;
            self.notice = Some(NO_ENEMY_FIELD_NOTICE.into());
        }
        // The AI cannot fly the player's aircraft yet: friendly wing 1 holds
        // just the player (a lobby's wing 1 is for humans, so it keeps its size).
        if !self.lobby
            && self.draft.values[4] > 1
            && let Some(why) = self.wingmen_refusal()
        {
            self.draft.values[4] = 1;
            self.notice = Some(format!("{why}, so friendly wing 1 is just you."));
        }
        // A distance the theater, target and start do not offer moves to
        // the nearest one they do (John, 2026-10-28).
        self.fit_distance();
    }
    /// An Airport field (34 or 36) whose Start is Airborne: it is not drawn,
    /// so it cannot be focused or touched.
    fn hidden_airport(&self, id: usize) -> bool {
        (id == 34 && !self.ground_start()) || (id == ENEMY_AIRPORT && !self.enemy_ground_start())
    }
    /// Reuse the creator's aircraft dialog for a multiplayer slot.
    pub fn open_slot_aircraft(&mut self, aircraft: AircraftId) {
        if let Some(index) = self
            .aircraft_files
            .iter()
            .position(|name| AircraftId::parse(name).ok() == Some(aircraft))
        {
            self.draft.values[6] = index;
        }
        self.open(6);
    }

    pub fn slot_aircraft_result(&self) -> Option<AircraftId> {
        if self.selector.is_some() {
            None
        } else {
            self.wing_aircraft(6)
        }
    }

    fn open(&mut self, id: usize) {
        self.selector = Some(id);
        self.cursor = if let Some(group) = id
            .checked_sub(OBJECTIVE_BASE)
            .filter(|group| *group < OBJECTIVE_COUNT)
        {
            Self::objective_choices(group)
                .iter()
                .position(|(_, objective)| *objective == self.group_objectives[group])
                .unwrap_or(0)
        } else if id == 17 {
            self.offered_distances()
                .iter()
                .position(|index| *index == self.draft.values[17])
                .unwrap_or(0)
        } else {
            self.draft.values[id]
        };
        self.scroll = self.cursor / ROWS * ROWS;
        self.hover = None;
        self.pressed = None;
        self.right_pressed = None;
        self.help = false;
    }
    /// A mission to read in the previews: an aircraft from each end of the
    /// lists, three wings to a side, orders and a survival flag.
    fn preview_spec(&self) -> crate::AppResult<MissionSpec> {
        let pick = |files: &[String], at: usize| {
            files
                .get(at % files.len().max(1))
                .and_then(|key| AircraftId::parse(key).ok())
                .ok_or("The preview needs imported aircraft.")
        };
        let player = pick(&self.aircraft_files, 1)?;
        let theater = self
            .theater_codes
            .get(2)
            .ok_or("The preview needs a theater.")?;
        let mut spec = MissionSpec::new(theater, player);
        spec.condition = Condition::Dawn;
        spec.start = Start::Airborne {
            altitude_ft: 20_000,
        };
        spec.separation_nm = 50;
        spec.preset = crate::ai_wings::Preset::Cap;
        let wings = [
            (player, 3, Skill::Average),
            (pick(&self.wing_files, 3)?, 2, Skill::Experienced),
            (pick(&self.wing_files, 0)?, 0, Skill::Novice),
            (pick(&self.wing_files, 2)?, 4, Skill::Ace),
            (pick(&self.wing_files, 4)?, 2, Skill::Novice),
            (pick(&self.wing_files, 5)?, 1, Skill::Dummy),
        ];
        for (slot, (aircraft, count, skill)) in spec.wings.iter_mut().zip(wings) {
            *slot = WingSpec {
                aircraft,
                count,
                skill,
            };
        }
        spec.objectives[0] = GroupObjective::Intercept(WingId::new(Side::Enemy, 0)?);
        spec.objectives[1] = GroupObjective::Escort(WingId::new(Side::Friendly, 0)?);
        spec.objectives[3] = GroupObjective::Free;
        spec.must_survive[0] = true;
        spec.must_survive[3] = true;
        Ok(spec)
    }
    /// The read-only creator as a player who is not the King sees it.
    fn preview_view(&mut self, name: &str) -> crate::AppResult<()> {
        let mut spec = self.preview_spec()?;
        if name.ends_with("-target") {
            // The King's mission names a ground target and its defenses.
            spec.ground_target = ground_targets(&spec.theater).first().map(|s| s.to_string());
            spec.aaa = Defense::Moderate;
            spec.sam = Defense::Heavy;
        }
        let gaps = name.ends_with("-gaps");
        if gaps {
            // This game lacks the last aircraft: the enemy's third wing
            // flies it, and its field shows it dimmed.
            spec.wings[5].aircraft = self
                .wing_files
                .last()
                .and_then(|key| AircraftId::parse(key).ok())
                .ok_or("The preview needs imported aircraft.")?;
            let (file, label) = (
                self.wing_files.pop().unwrap_or_default(),
                self.wing_names.pop().unwrap_or_default(),
            );
            self.aircraft_files.retain(|f| *f != file);
            self.aircraft_names.retain(|n| *n != label);
        }
        let kind = if name.ends_with("-flying") {
            ViewKind::Flying
        } else if name.ends_with("-locked") {
            ViewKind::Fixed
        } else {
            ViewKind::Reader
        };
        self.open_lobby_mission(&spec, Some(kind))?;
        if gaps {
            self.preview_gaps();
        }
        match name {
            "lobby-creator-view-click" => {
                self.activate(14);
            }
            "lobby-creator-view-changed" => {
                spec.separation_nm = 100;
                spec.wings[3].count = 5;
                self.reload_view(&spec)?;
            }
            _ => {}
        }
        Ok(())
    }
    pub fn preview_selector(&mut self, name: &str) -> crate::AppResult<()> {
        match name {
            "normal" | "ordnance" => {}
            // A first load: every aircraft slot random (slice BQ, B22), from a
            // fixed seed so the picture is repeatable.
            "first-load" => self.randomize_aircraft(0x2026_1012, false),
            // The creator opened from a multiplayer lobby (EF8), and after the
            // host refused an Accept.
            "lobby-creator" => self.enter_lobby(),
            "lobby-creator-choice" => {
                self.enter_lobby();
                self.draft.values[CHOICE_BASE] = 1;
                self.draft.values[CHOICE_BASE + 3] = 1;
                self.draft.values[AI_AIRCRAFT] = 0;
            }
            "lobby-creator-refused" => {
                self.enter_lobby();
                self.notice = Some(
                    "The mission could not be built: Choose a theater with enough room for the enemy separation."
                        .into(),
                );
            }
            // The lobby's creator with items not every player has (stage L,
            // slice L4): the aircraft list dimmed, and the notice a choice of
            // one leaves.
            "lobby-creator-gaps" | "lobby-creator-gap-notice" => {
                self.enter_lobby();
                self.preview_gaps();
                self.open(6);
                if name == "lobby-creator-gap-notice" {
                    self.cursor = (0..self.aircraft_files.len())
                        .find(|i| self.gap_in(6, *i).is_some())
                        .unwrap_or(0);
                    self.scroll = self.cursor / ROWS * ROWS;
                    self.activate(POP_OK);
                }
            }
            "lobby-creator-gap-theaters" => {
                self.enter_lobby();
                self.preview_gaps();
                self.open(13);
            }
            "lobby-creator-view"
            | "lobby-creator-view-gaps"
            | "lobby-creator-view-click"
            | "lobby-creator-view-changed"
            | "lobby-creator-view-flying"
            | "lobby-creator-view-locked" => self.preview_view(name)?,
            "lobby-ordnance" | "lobby-ordnance-refused" | "lobby-ordnance-cheat" => {}
            "lobby-ordnance-gaps" | "lobby-ordnance-laser" => {
                if let Some(ordnance) = &mut self.ordnance {
                    ordnance.preview(name);
                }
            }
            "ordnance-tanks"
            | "ordnance-empty"
            | "ordnance-drag"
            | "ordnance-message"
            | "ordnance-message-long"
            | "ordnance-laser"
            | "ordnance-pod"
            | "ordnance-pod-yellow"
            | "ordnance-gun-pod"
            | "ordnance-tip-amraam"
            | "ordnance-wing-amraam"
            | "ordnance-authored-tank" => {
                if let Some(ordnance) = &mut self.ordnance {
                    ordnance.preview(name);
                }
            }
            "objectives" => {
                self.draft.values[7] = 2;
                self.draft.values[24] = 2;
                self.group_objectives[0] = GroupObjective::Intercept(WingId::new(Side::Enemy, 0)?);
                self.group_objectives[1] = GroupObjective::Free;
                self.group_must_survive[1] = true;
                self.group_objectives[2] = GroupObjective::Escort(WingId::new(Side::Friendly, 1)?);
                self.group_objectives[3] =
                    GroupObjective::Intercept(WingId::new(Side::Friendly, 0)?);
                self.group_objectives[4] = GroupObjective::Free;
                self.group_objectives[5] = GroupObjective::Escort(WingId::new(Side::Enemy, 1)?);
            }
            "aircraft" => self.open(6),
            "theaters" => self.open(13),
            // Debrief pages are prepared by the snapshot host.
            state if state.starts_with("debrief") => {}
            "help" => self.help = true,
            // A ground target with its defenses (the theater's first target,
            // AAA moderately, SAMs heavily), its last, and a ground start the
            // world places from the target.
            "ground-target" | "ground-target-last" | "ground-start-auto" => {
                let last = self.values(30).len().saturating_sub(1);
                self.apply(
                    30,
                    if name == "ground-target-last" {
                        last
                    } else {
                        1
                    },
                );
                self.apply(31, if name == "ground-target-last" { 1 } else { 2 });
                self.apply(32, if name == "ground-target-last" { 2 } else { 3 });
                if name == "ground-start-auto" {
                    self.apply(33, 1);
                }
            }
            "lobby-creator-view-target" => self.preview_view(name)?,
            "ground-start" => self.apply(33, 1),
            "airports" => {
                self.apply(33, 1);
                self.open(34);
            }
            // The enemy's ground start (slice CL-R): Ground on its first
            // Redfor field, the world's pick from the theater's first target,
            // and the Enemy airport list open.
            "enemy-ground-start" => self.apply(ENEMY_START, 1),
            "enemy-ground-start-auto" => {
                self.apply(30, 1);
                self.apply(ENEMY_START, 1);
            }
            // The Map over the creator (slice CL-R), and over the lobby's
            // editable creator, which shows the target's retail spot.
            "map" => {
                self.open_map();
                self.wait_for_map();
            }
            // The widest zoom, where the theater just fills the plot, and the
            // narrowest, to show that no black border appears at either.
            "map-wide" | "map-close" => {
                self.open_map();
                self.wait_for_map();
                if let Some(map) = &mut self.map {
                    let key = if name == "map-wide" { "-" } else { "+" };
                    for _ in 0..8 {
                        map.key(key);
                    }
                }
            }
            // The note shown while the map is still being built.
            "map-building" => {
                self.map_hold = true;
                self.open_map();
            }
            "map-lobby" => {
                self.enter_lobby();
                self.open_map();
                self.wait_for_map();
            }
            "enemy-airports" => {
                self.apply(ENEMY_START, 1);
                self.open(ENEMY_AIRPORT);
            }
            name if name.starts_with("objective-") => {
                let group = name[10..]
                    .parse::<usize>()
                    .ok()
                    .filter(|group| (1..=OBJECTIVE_COUNT).contains(group))
                    .ok_or("objective snapshots use objective-1 through objective-6")?;
                self.open(OBJECTIVE_BASE + group - 1);
            }
            _ => {
                let id=name.strip_prefix("field-").and_then(|v|v.parse::<usize>().ok()).filter(|v|(3..FIELDS).contains(v)).ok_or("snapshot states: normal, aircraft, objectives, ground-start, ground-start-auto, ground-target, ground-target-last, airports, enemy-ground-start, enemy-ground-start-auto, enemy-airports, map, map-building, map-wide, map-close, map-lobby, objective-1 through objective-6, theaters, help, first-load, field-3 through field-36")?;
                self.open(id);
            }
        }
        Ok(())
    }
    pub fn pointer(&mut self, p: Option<(f64, f64)>) {
        if let Some(d) = &mut self.debrief {
            d.pointer(p);
            return;
        }
        if let Some(map) = &mut self.map {
            map.pointer(p);
            return;
        }
        if let Some(o) = self.ordnance.as_mut().filter(|o| o.visible) {
            o.pointer(p);
            return;
        }
        self.hover = p.and_then(|p| {
            self.controls
                .iter()
                .rev()
                .find(|(_, r)| inside(p, *r))
                .map(|(i, _)| *i)
        });
    }
    pub fn down(&mut self) -> Action {
        self.right_pressed = None;
        if let Some(d) = &mut self.debrief {
            return d.down();
        }
        if let Some(o) = self.ordnance.as_mut().filter(|o| o.visible) {
            return o.down();
        }
        if let Some(map) = &mut self.map {
            map.down();
            return Action::None;
        }
        self.pressed = self.hover;
        // The selector's rocker pages on press and tilts while held.
        match self.hover.filter(|_| self.selector.is_some()) {
            Some(id @ (UP | DOWN)) => self.rock(id, true),
            _ => Action::None,
        }
    }
    fn rock(&mut self, id: usize, held: bool) -> Action {
        self.rocker
            .push(id == DOWN, held, std::time::Instant::now());
        self.activate(id);
        Action::RockerDown
    }
    pub fn up(&mut self) -> Action {
        if let Some(d) = &mut self.debrief {
            return d.up().unwrap_or_else(|| self.close_debrief());
        }
        if let Some(o) = self.ordnance.as_mut().filter(|o| o.visible) {
            return o.up();
        }
        if let Some(map) = &mut self.map {
            if map.up() {
                self.close_map();
                return Action::Click;
            }
            return Action::None;
        }
        let p = self.pressed.take();
        if matches!(p, Some(UP | DOWN)) && self.rocker.held() {
            self.rocker.release(std::time::Instant::now());
            return Action::RockerUp;
        }
        if let Some(i) = p.filter(|p| Some(*p) == self.hover) {
            self.activate(i)
        } else {
            Action::None
        }
    }
    pub fn right(&mut self, down: bool) -> Action {
        if let Some(d) = &mut self.debrief {
            self.right_pressed = None;
            return d.right(down);
        }
        if let Some(o) = self.ordnance.as_mut().filter(|o| o.visible) {
            self.right_pressed = None;
            return o.right(down);
        }
        if self.map.is_some() {
            self.right_pressed = None;
            return Action::None;
        }
        if self.selector.is_some() || self.help {
            self.right_pressed = None;
            return Action::None;
        }
        if down {
            self.pressed = None;
            self.right_pressed = self.hover.filter(|id| {
                (3..FIELDS).contains(id)
                    || (OBJECTIVE_BASE..SURVIVAL_BASE + OBJECTIVE_COUNT).contains(id)
            });
            return Action::None;
        }
        let Some(id) = self
            .right_pressed
            .take()
            .filter(|id| Some(*id) == self.hover)
        else {
            return Action::None;
        };
        if let Some(kind) = self.view {
            self.focus = id;
            self.notice = Some(kind.refusal().into());
            return Action::Click;
        }
        if self.hidden_airport(id) {
            return Action::None;
        }
        if (SURVIVAL_BASE..SURVIVAL_BASE + OBJECTIVE_COUNT).contains(&id) {
            self.group_must_survive[id - SURVIVAL_BASE] ^= true;
            self.focus = id;
            return Action::Click;
        }
        if let Some(group) = id
            .checked_sub(OBJECTIVE_BASE)
            .filter(|group| *group < OBJECTIVE_COUNT)
        {
            let choices = Self::objective_choices(group);
            let current = choices
                .iter()
                .position(|(_, objective)| *objective == self.group_objectives[group])
                .unwrap_or(0);
            let previous = (current + choices.len() - 1) % choices.len();
            self.group_objectives[group] = choices[previous].1;
            self.focus = id;
            return Action::Click;
        }
        if id == 17 {
            self.focus = id;
            let previous = self.next_distance(false);
            self.apply(17, previous);
            return Action::Click;
        }
        let n = self.values(id).len();
        let minimum = usize::from(id == 4);
        if n <= minimum {
            return Action::None;
        }
        let current = self.draft.values[id];
        let previous = if current <= minimum || current >= n {
            n - 1
        } else {
            current - 1
        };
        self.focus = id;
        if !self.refuse_gap(id, previous) {
            self.apply(id, previous);
        }
        Action::Click
    }
    /// Leaves the debrief for the creator, keeping the mission just flown, or
    /// for the main menu after a multiplayer flight.
    fn close_debrief(&mut self) -> Action {
        self.debrief = None;
        self.hover = None;
        self.pressed = None;
        if std::mem::take(&mut self.debrief_to_menu) {
            return Action::Back;
        }
        Action::Click
    }
    /// Forgets the map: the open one, a build in progress and the hold the
    /// snapshots put on it.
    fn drop_map(&mut self) {
        self.map = None;
        self.map_build = None;
        self.map_hold = false;
    }
    /// Closes the map; the creator is as it was.
    fn close_map(&mut self) {
        self.map = None;
        self.hover = None;
        self.pressed = None;
    }
    pub fn cancel(&mut self) {
        if let Some(d) = &mut self.debrief {
            d.cancel();
        }
        if let Some(o) = &mut self.ordnance {
            o.cancel();
        }
        self.pressed = None;
        self.right_pressed = None;
        if self.rocker.held() {
            self.rocker.release(std::time::Instant::now());
        }
        self.hover = None;
        self.selector = None;
        self.help = false;
    }
    /// A press in the read-only creator: Back closes it, and a field says
    /// that it cannot be changed. Nothing else happens.
    fn activate_view(&mut self, kind: ViewKind, id: usize) -> Action {
        match id {
            0 => self.help = !self.help,
            61 => return Action::Exit,
            // The map changes nothing, so the read-only page has it too.
            MAP => self.open_map(),
            OK | CANCEL => {
                self.map_build = None;
                return Action::Back;
            }
            3..=43 | 60 | OBJECTIVE_BASE..=91 => {
                self.focus = id;
                self.notice = Some(kind.refusal().into());
            }
            _ => return Action::None,
        }
        Action::Click
    }
    fn activate(&mut self, id: usize) -> Action {
        if let Some(kind) = self.view {
            return self.activate_view(kind, id);
        }
        if let Some(field) = self.selector {
            match id {
                POP_OK => {
                    let values = self.selector_values(field);
                    if values.is_empty() {
                        return Action::None;
                    }
                    if let Some(group) = field
                        .checked_sub(OBJECTIVE_BASE)
                        .filter(|group| *group < OBJECTIVE_COUNT)
                    {
                        self.group_objectives[group] =
                            Self::objective_choices(group)[self.cursor].1;
                        self.notice = None;
                    } else if field == 17 {
                        // The list holds the offered distances only.
                        if let Some(index) = self.offered_distances().get(self.cursor) {
                            self.apply(17, *index);
                        }
                    } else if self.gap_in(field, self.cursor).is_some() {
                        // Not everyone has it (stage L): the choice stays as
                        // it was and the notice says who lacks it.
                        self.refuse_gap(field, self.cursor);
                    } else {
                        self.apply(field, self.cursor);
                    }
                    self.selector = None;
                    self.focus = field;
                }
                POP_CANCEL => {
                    self.selector = None;
                    self.focus = field;
                }
                UP => self.scroll = self.scroll.saturating_sub(ROWS),
                DOWN => {
                    self.scroll = (self.scroll + ROWS)
                        .min(self.selector_values(field).len().saturating_sub(1) / ROWS * ROWS)
                }
                ROW_BASE.. => {
                    let index = self.scroll + id - ROW_BASE;
                    if index < self.selector_values(field).len() {
                        self.cursor = index;
                    }
                }
                _ => return Action::None,
            }
            self.hover = None;
            return Action::Click;
        }
        match id {
            0 => {
                self.help = !self.help;
            }
            60 => {
                self.notice=Some("Aircraft era filters are not available yet. The list shows only supported imported aircraft.".into());
            }
            61 => return Action::Exit,
            MAP => self.open_map(),
            OK => {
                if let Some(message) = self.unsupported() {
                    self.notice = Some(message);
                } else {
                    // A new flight, a new layout of its defenses.
                    self.roll_surface_seed();
                    self.map_build = None;
                    return Action::Mission;
                }
            }
            CANCEL => {
                self.map_build = None;
                return Action::Back;
            }
            3..=43 => {
                if self.hidden_airport(id) {
                    return Action::None;
                }
                self.focus = id;
                if matches!(id, 6 | 9 | 12 | 13 | 23 | 26 | 29 | 34 | ENEMY_AIRPORT) ^ self.shift {
                    self.open(id);
                } else {
                    let n = self.values(id).len();
                    if id == 17 {
                        let next = self.next_distance(true);
                        self.apply(17, next);
                    } else if n > 0 {
                        let next = (self.draft.values[id] + 1) % n;
                        if !self.refuse_gap(id, next) {
                            self.apply(id, next);
                        }
                    }
                }
            }
            OBJECTIVE_BASE..=85 => {
                self.focus = id;
                self.open(id);
            }
            SURVIVAL_BASE..=91 => {
                self.focus = id;
                self.group_must_survive[id - SURVIVAL_BASE] ^= true;
            }
            _ => return Action::None,
        }
        Action::Click
    }
    pub fn key(&mut self, key: &str, shift: bool) -> Action {
        if let Some(d) = &mut self.debrief {
            return d.key(key).unwrap_or_else(|| self.close_debrief());
        }
        if let Some(o) = self.ordnance.as_mut().filter(|o| o.visible) {
            return o.key(key);
        }
        if let Some(map) = &mut self.map {
            if matches!(key, "Escape" | "Enter" | " ") {
                self.close_map();
                return Action::Click;
            }
            map.key(key);
            return Action::None;
        }
        self.shift = shift;
        // Enter closes the read-only creator, as Back does.
        if self.view.is_some() && key == "Enter" {
            self.map_build = None;
            return Action::Back;
        }
        if key == "Escape" {
            // Escape first gives up the map being built.
            if self.map_build.take().is_some() {
                return Action::None;
            }
            // The lobby's notice is not in the way of Cancel (EF8).
            if self.selector.is_some() || self.help || (self.notice.is_some() && !self.lobby) {
                self.cancel();
                self.notice = None;
                return Action::None;
            }
            return Action::Back;
        }
        if let Some(field) = self.selector {
            let n = self.selector_values(field).len();
            if n == 0 {
                return Action::None;
            }
            match key {
                "ArrowDown" => self.cursor = (self.cursor + 1) % n,
                "ArrowUp" => self.cursor = (self.cursor + n - 1) % n,
                "Home" => self.cursor = 0,
                "End" => self.cursor = n - 1,
                "PageDown" | "PageUp" => {
                    let down = key == "PageDown";
                    self.cursor = if down {
                        (self.cursor + ROWS).min(n - 1)
                    } else {
                        self.cursor.saturating_sub(ROWS)
                    };
                    self.scroll = self.cursor / ROWS * ROWS;
                    self.rocker.push(down, false, std::time::Instant::now());
                    return Action::RockerDown;
                }
                "Enter" => return self.activate(POP_OK),
                _ => {}
            }
            self.scroll = self.cursor / ROWS * ROWS;
            return Action::None;
        }
        match key {
            "Tab" | "ArrowDown" | "ArrowUp" => {
                let backwards = shift || key == "ArrowUp";
                let focus_order: Vec<_> = (1..FIELDS)
                    .filter(|id| !self.hidden_airport(*id) && (self.lobby || *id < CHOICE_BASE))
                    .chain(OBJECTIVE_BASE..SURVIVAL_BASE + OBJECTIVE_COUNT)
                    .collect();
                self.focus = if let Some(position) =
                    focus_order.iter().position(|field| *field == self.focus)
                {
                    let next = if backwards {
                        (position + focus_order.len() - 1) % focus_order.len()
                    } else {
                        (position + 1) % focus_order.len()
                    };
                    focus_order[next]
                } else if backwards {
                    *focus_order.last().unwrap()
                } else {
                    focus_order[0]
                };
                self.hover = Some(self.focus);
            }
            "Enter" | " " => return self.activate(self.focus),
            _ => {}
        }
        Action::None
    }
    pub fn render(
        &mut self,
        pixels: &mut [u8],
        sprites: &BTreeMap<String, Sprite>,
        _world: &Terrain,
    ) -> bool {
        if self.resources.is_some() {
            self.reserve_placement_seed();
        }
        let mut animating = self.rocker.advance(std::time::Instant::now());
        // A map built in the background opens the moment it is done.
        self.poll_map();
        if let Some(d) = &mut self.debrief {
            return d.render(pixels);
        }
        if let Some(o) = self.ordnance.as_mut().filter(|o| o.visible) {
            return o.render(pixels);
        }
        if let Some(map) = &mut self.map {
            animating |= map.render(pixels, sprites);
            return animating;
        }
        pixels.copy_from_slice(&sprites["QUIKMIS3.PIC"].rgba);
        self.controls.clear();
        let mut c = Canvas(pixels);
        let font = &sprites["QUICKFONT"];
        // The top bar: Aircraft, then Map (slice CL-R) in the same face, on
        // the same baseline and starting a label's gap after it; each hit
        // box runs from its label to the next one's.
        let face = &sprites["MENUFONT.PIC"];
        let mut entries = Vec::new();
        let mut x = 103;
        for (id, label) in [(60, "Aircraft"), (MAP, "Map")] {
            let width = text_width(face, label);
            c.centered_text_by(face, label, "Aircraft", (x, 38, width, 20));
            entries.push((id, (x, BAR_ENTRY.0, width + BAR_GAP, BAR_ENTRY.1)));
            x += width + BAR_GAP;
        }
        self.controls.push((0, (84, 35, 18, 24)));
        self.controls.extend(entries);
        c.text(font, "FRIENDLY SITUATION", 116, 104, None);
        c.text(font, "ENEMY SITUATION", 429, 104, None);
        for sentence in self.sentences(font) {
            if sentence.wrap {
                for (text, id, x, y) in sentence.words(font, 605) {
                    c.text(font, &text, x, y, None);
                    if let Some(id) = id {
                        let r = (
                            x - 1,
                            y - 1,
                            text_width(font, &text) + 2,
                            font.glyphs.iter().map(|g| g[2]).max().unwrap_or(9) as i32 + 2,
                        );
                        self.controls.push((id, r));
                        c.rect(
                            r,
                            if self.hover == Some(id) {
                                [127, 139, 144, 255]
                            } else {
                                [101, 107, 109, 255]
                            },
                        );
                        bevel(&mut c, r, false);
                        c.text(font, &text, x, y, None);
                    }
                }
            } else {
                let parts: Vec<_> = sentence
                    .parts
                    .iter()
                    .map(|(text, id)| (text.as_str(), *id))
                    .collect();
                self.line(&mut c, font, sentence.x, sentence.y, &parts);
                let note = if sentence.parts.iter().any(|p| p.1 == Some(33))
                    && self.ground_unavailable()
                {
                    Some((33, NO_FIELD_NOTE))
                } else if sentence.parts.iter().any(|p| p.1 == Some(ENEMY_START))
                    && self.enemy_ground_unavailable()
                {
                    Some((ENEMY_START, NO_ENEMY_FIELD_NOTE))
                } else {
                    None
                };
                if let Some((id, note)) = note.filter(|_| self.view.is_none()) {
                    let x = self
                        .controls
                        .iter()
                        .rev()
                        .find(|(field, _)| *field == id)
                        .map_or(sentence.x + 105, |(_, r)| r.0 + r.2 + 6);
                    c.text(font, note, x, sentence.y, Some(GAP_FIELD_TEXT));
                }
            }
        }
        if self.view.is_some() {
            // Read-only: one button, and the page says so in its header.
            self.button(&mut c, sprites, OK, "Back", (492, 419, 85, 24));
            let face = &sprites["MENUFONT.PIC"];
            let words = "View only";
            c.text(face, words, 604 - text_width(face, words), 38, None);
        } else {
            self.button(
                &mut c,
                sprites,
                OK,
                if self.lobby { "Accept" } else { "OK" },
                (387, 419, 85, 24),
            );
            self.button(&mut c, sprites, CANCEL, "Cancel", (492, 419, 85, 24));
        }
        // The creator stays live while the map is built: it says so.
        let building = self.map_building().then_some(map::BUILDING_NOTE.to_owned());
        animating |= building.is_some();
        // The chosen target's fixed note (what its layout always leaves
        // out), from the distance table, when nothing else is said.
        let note = self.target_note().map(str::to_owned);
        if let Some(message) = self.notice.as_ref().or(building.as_ref()).or(note.as_ref()) {
            if self.lobby {
                lobby_notice(&mut c, &sprites["SMLFONT.PIC"], message);
            } else {
                notice(&mut c, &sprites["SMLFONT.PIC"], message);
            }
        }
        if self.help {
            c.rect((84, 60, 180, 25), [212, 215, 218, 255]);
            c.text(&sprites["MENUFONT.PIC"], "Exit to Desktop", 89, 64, None);
            self.controls = vec![(0, (84, 35, 18, 24)), (61, (84, 60, 180, 25))];
        }
        if let Some(field) = self.selector {
            let selector_values = self.selector_values(field);
            self.controls.clear();
            // Reuse the original metal panel texture, inset list wells and rocker.
            let background = &sprites["QUIKMIS3.PIC"];
            for y in 0..POPUP.3 {
                for x in 0..POPUP.2 {
                    let source = (((140 + y % 240) as usize * WIDTH) + (50 + x % 240) as usize) * 4;
                    c.rect(
                        (POPUP.0 + x, POPUP.1 + y, 1, 1),
                        background.rgba[source..source + 4].try_into().unwrap(),
                    );
                }
            }
            bevel(&mut c, POPUP, false);
            for row in 0..ROWS {
                let y = 116 + row as i32 * 18;
                c.rect((207, y, 226, 14), [12, 16, 16, 255]);
                bevel(&mut c, (207, y, 226, 14), true);
                let Some(text) = selector_values.get(self.scroll + row) else {
                    continue;
                };
                stripe(
                    &mut c,
                    (209, y + 1, 11, 12),
                    self.cursor == self.scroll + row,
                );
                // What not every player has is dimmed in the lobby's creator.
                let dim = self.gap_in(field, self.scroll + row).map(|_| GAP_TEXT);
                c.text(font, &fit(font, text, 208), 223, y + 2, dim);
            }
            for row in 0..ROWS.min(selector_values.len().saturating_sub(self.scroll)) {
                self.controls
                    .push((ROW_BASE + row, (207, 116 + row as i32 * 18, 226, 14)));
            }
            c.text(font, "PAGE", 277, 394, None);
            c.rect((307, 389, 50, 17), [12, 16, 16, 255]);
            bevel(&mut c, (307, 389, 50, 17), true);
            c.text(
                font,
                &format!(
                    "{} of {}",
                    self.scroll / ROWS + 1,
                    selector_values.len().div_ceil(ROWS).max(1)
                ),
                314,
                394,
                None,
            );
            c.text(font, "PREV", 380, 394, None);
            c.text(font, "NEXT", 380, 417, None);
            let rocker = &sprites[self.rocker.sprite()];
            c.blit(rocker, (410, 393), 0, rocker.width, 1.);
            self.controls
                .extend([(UP, (410, 393, 18, 17)), (DOWN, (410, 410, 18, 17))]);
            if selector_values.is_empty() {
                c.text(font, "No available choices.", 209, 118, None);
            }
            self.button(&mut c, sprites, POP_OK, "OK", (217, 437, 85, 24));
            self.button(&mut c, sprites, POP_CANCEL, "Cancel", (312, 437, 85, 24));
        }
        animating
    }
    /// One source of sentence text and variable values for setup and flight.
    pub(crate) fn sentences(&self, font: &Sprite) -> Vec<crate::mission_brief::Sentence> {
        use crate::mission_brief::Sentence;
        let mut rows = Vec::new();
        let mut line = |x, y, parts: &[(&str, Option<usize>)]| {
            rows.push(Sentence {
                x,
                y,
                wrap: false,
                parts: parts
                    .iter()
                    .map(|(text, id)| {
                        (
                            id.map(|i| self.value(i)).unwrap_or_else(|| (*text).into()),
                            *id,
                        )
                    })
                    .collect(),
            });
        };
        if self.lobby {
            line(35, 120, &[("AI aircraft: ", None), ("", Some(AI_AIRCRAFT))]);
        }

        line(35, 137, &[("Friendly forces are ", None), ("", Some(3))]);
        line(340, 137, &[("Enemy forces are ", None), ("", Some(20))]);
        for (x, start) in [(35, 4), (340, 21)] {
            for wing in 0..3 {
                let id = start + wing * 3;
                let y = 153 + wing as i32 * 14;
                let heading = format!("Wing {}: ", wing + 1);
                let group = wing + usize::from(start == 21) * 3;
                let mut parts = vec![
                    (heading.as_str(), None),
                    ("", Some(id)),
                    (" ", None),
                    ("", Some(id + 1)),
                    (" ", None),
                ];
                if self.lobby {
                    parts.push(("", Some(CHOICE_BASE + group)));
                    parts.push((
                        if self.draft.values[CHOICE_BASE + group] != 0 {
                            " AI: "
                        } else {
                            " "
                        },
                        None,
                    ));
                }
                parts.push(("", Some(id + 2)));
                line(x, y, &parts);
            }
            for wing in 0..3 {
                let group = wing + usize::from(start == 21) * 3;
                let prefix = self.objective_prefix(group);
                line(
                    x,
                    201 + wing as i32 * 14,
                    &[
                        (&prefix, None),
                        ("", Some(OBJECTIVE_BASE + group)),
                        (".", None),
                    ],
                );
            }
        }
        for (x, side_offset) in [(35, 0), (340, 3)] {
            for wing in 0..3 {
                let group = side_offset + wing;
                let prefix = if group == 0 {
                    "Your group's survival is ".to_owned()
                } else {
                    format!("Wing {}'s survival is ", wing + 1)
                };
                line(
                    x,
                    249 + wing as i32 * 14,
                    &[
                        (&prefix, None),
                        ("", Some(SURVIVAL_BASE + group)),
                        (".", None),
                    ],
                );
            }
        }
        for (y, parts) in [
            (301, vec![("You are flying over ", None), ("", Some(13))]),
            (
                315,
                vec![
                    (
                        if self.ground_start() {
                            "Airborne wings at "
                        } else {
                            "You are at "
                        },
                        None,
                    ),
                    ("", Some(14)),
                    (" feet. It is ", None),
                    ("", Some(15)),
                ],
            ),
            (329, vec![("Your situation is ", None), ("", Some(16))]),
            (
                343,
                vec![
                    ("You are ", None),
                    ("", Some(17)),
                    (" from enemy forces.", None),
                ],
            ),
            (371, vec![("You are carrying ", None), ("", Some(18))]),
            (385, vec![("Air combat is with ", None), ("", Some(19))]),
        ] {
            line(35, y, &parts);
        }
        line(35, 357, &[("Start: ", None), ("", Some(33))]);
        if self.ground_start() {
            line(35, 399, &[("Airport: ", None), ("", Some(34))]);
        }
        let target = Sentence {
            x: 340,
            y: 301,
            wrap: true,
            parts: [
                ("Friendly ground target is ", None),
                ("", Some(30)),
                ("", Some(31)),
                ("defended by AAA and ", None),
                ("", Some(32)),
                ("defended by SAMs.", None),
            ]
            .iter()
            .map(|(text, id)| {
                (
                    id.map(|i| self.value(i)).unwrap_or_else(|| (*text).into()),
                    *id,
                )
            })
            .collect(),
        };
        let enemy_y = (target.last_y(font, 605) + 28).min(ENEMY_ROWS_LAST_Y);
        line(
            340,
            enemy_y,
            &[("Enemy start: ", None), ("", Some(ENEMY_START))],
        );
        if self.enemy_ground_start() {
            line(
                340,
                enemy_y + 14,
                &[("Enemy airport: ", None), ("", Some(ENEMY_AIRPORT))],
            );
        }
        rows.push(target);
        rows
    }
    /// Reconstruct from the accepted mission, never from the current draft.
    pub(crate) fn brief_sentences(
        &self,
        spec: &MissionSpec,
        settings: Option<&tore_session::settings::Store>,
        font: &Sprite,
    ) -> Result<Vec<crate::mission_brief::Sentence>, String> {
        let empty = BTreeMap::new();
        let mut creator = Self::new(
            spec.player(),
            self.options.clone(),
            self.resources.as_deref().unwrap_or(&empty),
        );
        creator.choose_theater_code(&spec.theater, self.resources.as_deref().unwrap_or(&empty));
        creator.load_spec(spec, false)?;
        creator.view = Some(ViewKind::Reader);
        creator.lobby = settings.is_some();
        if let Some(settings) = settings {
            creator.draft.values[AI_AIRCRAFT] = usize::from(settings.ai());
        }
        Ok(creator.sentences(font))
    }
    fn line(
        &mut self,
        c: &mut Canvas,
        font: &Sprite,
        x: i32,
        y: i32,
        parts: &[(&str, Option<usize>)],
    ) {
        let mut x = x;
        for (text, id) in parts {
            let text = id
                .map(|i| self.value(i))
                .unwrap_or_else(|| text.to_string());
            if id.is_some() {
                x += 2;
            }
            let text = fit(font, &text, if x < 320 { 299 - x } else { 605 - x });
            let width = text_width(font, &text);
            if let Some(id) = id {
                let r = (
                    x - 1,
                    y - 1,
                    width + 2,
                    font.glyphs.iter().map(|g| g[2]).max().unwrap_or(9) as i32 + 2,
                );
                self.controls.push((*id, r));
                // A dimmed field sits in a darker well, as the field is the
                // same grey as the tint the lists dim with.
                let dim = self.view_tint(*id).is_some();
                c.rect(
                    r,
                    match (dim, self.hover == Some(*id)) {
                        (_, true) => [127, 139, 144, 255],
                        (true, false) => [58, 63, 65, 255],
                        (false, false) => [101, 107, 109, 255],
                    },
                );
                bevel(c, r, false);
            }
            let tint = id.and_then(|id| self.view_tint(id));
            c.text(font, &text, x, y, tint);
            x += width + if id.is_some() { 2 } else { 0 };
        }
    }
    /// The read-only creator dims what the mission does not carry and what
    /// this game lacks (an aircraft or a theater not everyone has); the tint
    /// the lobby's lists use for it.
    fn view_tint(&self, id: usize) -> Option<[u8; 3]> {
        // A Start locked to Airborne for want of a field of the side is
        // greyed (the read-only page draws the King's choice as it is).
        if self.view.is_none()
            && ((id == 33 && self.ground_unavailable())
                || (id == ENEMY_START && self.enemy_ground_unavailable()))
        {
            return Some(GAP_FIELD_TEXT);
        }
        self.view?;
        let lacks = self.shown.contains_key(&id) && (id == 6 || AI_AIRCRAFT_FIELDS.contains(&id));
        let gap = self
            .draft
            .values
            .get(id)
            .is_some_and(|index| self.gap_in(id, *index).is_some());
        (NOT_CARRIED.contains(&id) || lacks || gap).then_some(GAP_FIELD_TEXT)
    }
    fn button(
        &mut self,
        c: &mut Canvas,
        sprites: &BTreeMap<String, Sprite>,
        id: usize,
        label: &str,
        r: Rect,
    ) {
        let hit = c.action_button(
            sprites,
            label,
            (r.0, r.1, r.2),
            matches!(id, OK | POP_OK),
            self.pressed == Some(id),
        );
        self.controls.push((id, hit));
    }
}
pub(crate) fn bevel(c: &mut Canvas, (x, y, w, h): Rect, inset: bool) {
    let light = [115, 120, 119, 255];
    let dark = [24, 27, 27, 255];
    let (top, bottom) = if inset { (dark, light) } else { (light, dark) };
    c.rect((x, y, w, 1), top);
    c.rect((x, y, 1, h), top);
    c.rect((x, y + h - 1, w, 1), bottom);
    c.rect((x + w - 1, y, 1, h), bottom);
}
fn stripe(c: &mut Canvas, (x, y, w, h): Rect, selected: bool) {
    // Fitted diagonal status marker, using the reference's blue/gold treatment.
    let colors = if selected {
        [[230, 181, 39, 255], [73, 53, 17, 255]]
    } else {
        [[180, 193, 215, 255], [58, 94, 168, 255]]
    };
    for yy in 0..h {
        for xx in 0..w {
            c.rect(
                (x + xx, y + yy, 1, 1),
                colors[((xx - yy).rem_euclid(6) / 3) as usize],
            );
        }
    }
}
/// The enemy nationality field's value for a base theater (the first field,
/// the friendly nationality, is always 0).
fn enemy_nationality(base_theater: usize) -> usize {
    tore_formats::quick_template::tables::ENEMY_NATIONALITY[base_theater]
}
/// A fresh nonzero seed: the clock, the process and a counter, mixed.
fn fresh_seed() -> u32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut z = nanos
        ^ (u64::from(std::process::id()) << 32)
        ^ COUNT.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) as u32).max(1)
}
fn source_theaters() -> [&'static str; 16] {
    tore_world::mission::THEATERS
}
fn inside(p: (f64, f64), r: Rect) -> bool {
    p.0 >= r.0 as f64 && p.1 >= r.1 as f64 && p.0 < (r.0 + r.2) as f64 && p.1 < (r.1 + r.3) as f64
}
pub(crate) fn fit(font: &Sprite, text: &str, width: i32) -> String {
    let mut s = text.to_string();
    if text_width(font, &s) > width {
        while !s.is_empty() && text_width(font, &format!("{s}...")) > width {
            s.pop();
        }
        s.push_str("...");
    }
    s
}
pub fn notice(c: &mut Canvas, font: &Sprite, text: &str) {
    c.rect((30, 335, 580, 65), [35, 44, 46, 255]);
    let mut line = String::new();
    let mut y = 340;
    for word in text.split_whitespace() {
        let next = format!("{line}{word} ");
        if text_width(font, &next) > 565 {
            c.text(font, &line, 36, y, Some([235, 225, 179]));
            line.clear();
            y += 13;
        }
        line.push_str(word);
        line.push(' ');
    }
    c.text(font, &line, 36, y, Some([235, 225, 179]));
}
/// The lobby creator's notice (EF8): a box at the lower left, beside the
/// buttons, of up to three lines, so it does not cover the Start line that
/// the notice is about. A longer message ends in an ellipsis.
fn lobby_notice(c: &mut Canvas, font: &Sprite, text: &str) {
    const WIDTH: i32 = 340;
    const LINES: usize = 3;
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let next = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if text_width(font, &next) > WIDTH - 12 && !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            line = word.to_owned();
        } else {
            line = next;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.len() > LINES {
        lines.truncate(LINES);
        let last = lines[LINES - 1].clone();
        lines[LINES - 1] = fit(font, &format!("{last} ..."), WIDTH - 12);
    }
    let height = 6 + 13 * lines.len() as i32;
    c.rect((30, 438 - height, WIDTH, height), [35, 44, 46, 255]);
    for (i, line) in lines.iter().enumerate() {
        c.text(
            font,
            line,
            36,
            438 - height + 4 + 13 * i as i32,
            Some([235, 225, 179]),
        );
    }
}
pub fn hud(
    pixels: &mut [u8],
    sprites: &BTreeMap<String, Sprite>,
    camera: &Camera,
    world: &Terrain,
) {
    pixels.fill(0);
    let mut c = Canvas(pixels);
    let font = &sprites["SMLFONT.PIC"];
    c.rect((0, 0, WIDTH as i32, 37), [18, 26, 30, 230]);
    c.text(
        font,
        &format!(
            "{}   TERRAIN VIEWER     ALT {:.0} ft",
            world.theater.name.to_ascii_uppercase(),
            camera.position[1]
        ),
        12,
        8,
        Some([230, 234, 222]),
    );
    c.text(
        font,
        "Arrows: move   Shift: fast   Q/E: down/up   WASD: look   Esc: return",
        12,
        24,
        Some([210, 218, 220]),
    );
    c.rect((8, HEIGHT as i32 - 25, 365, 19), [18, 26, 30, 225]);
    c.text(
        font,
        "Retail terrain / midday preview - environment parity in progress",
        13,
        HEIGHT as i32 - 20,
        Some([210, 218, 220]),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tore_sim::ai::launch::legacy_pairs;
    use tore_world::mission::default_enemy_nationality;
    #[test]
    fn a_multiplayer_debrief_returns_to_the_menu_and_a_single_player_one_to_the_creator() {
        let mut q = setup();
        assert_eq!(q.close_debrief(), Action::Click);
        q.debrief_to_menu = true;
        assert_eq!(q.close_debrief(), Action::Back);
        // Once only: the next debrief is a single-player one again.
        assert_eq!(q.close_debrief(), Action::Click);
    }
    #[test]
    fn the_selected_theater_code_comes_from_the_creator_not_the_flight_world() {
        let mut q = setup();
        for index in [0, 5, 13] {
            q.theater(index);
            assert_eq!(
                q.theater_code(),
                Some(q.theater_catalog[q.theater_index()].as_str()),
                "theater {index}"
            );
        }
    }
    /// Three Quick Missions in a row, each in a different theater. After the
    /// first, the flight world's catalog lists only that theater. The next
    /// mission still comes from the creator's catalog, which is the crash of
    /// 2026-10-17 (index 13 into a list of 1).
    #[test]
    fn three_quick_missions_in_a_row_build_different_theaters() {
        let mut resources = tore_world::test_support::resources::resources();
        for code in ["KURILE", "NSK"] {
            let grid = resources["UKR.T2"].clone();
            resources.insert(format!("{code}.T2"), grid);
            resources.insert(
                format!("{code}.MM"),
                format!("textFormat\nmap {code}.T2\nlayer CLEAR.LAY 0\ntime 12 0\n").into_bytes(),
            );
        }
        let mut quick = setup();
        // One aircraft. The extra wings are not what this test is about.
        for field in [7, 10, 21, 24, 27] {
            quick.draft.values[field] = 0;
        }
        for code in ["UKR", "KURILE", "NSK"] {
            quick.choose_theater_code(code, &resources);
            assert_eq!(quick.theater_code(), Some(code));
            let index = quick.theater_index();
            // A built world's catalog has length 1. These later theaters sit
            // past that, which is the index the old selection used.
            if code != "UKR" {
                assert!(index >= 1, "{code} index {index}");
            }
            let spec = quick
                .mission_spec()
                .unwrap_or_else(|error| panic!("{code}: {error}"));
            assert_eq!(spec.theater, code);
            let world = tore_world::world::World::new(
                &spec,
                &resources,
                tore_world::world::Seating::SinglePlayer,
            )
            .unwrap_or_else(|error| panic!("{code}: {error}"));
            assert_eq!(world.terrain.catalog.len(), 1, "{code}");
            assert_eq!(world.terrain.catalog[0].0, code);
        }
    }
    #[test]
    fn variant_selection_retains_layout_and_uses_base_country_and_target_lists() {
        let mut q = setup();
        q.theater_codes.push("~UKR1".into());
        q.theater_catalog.push("~UKR1".into());
        q.options.fields[13].push("Ukraine (UKR1)".into());
        q.airport_names.push(vec!["Variant runway".into()]);
        q.airport_objects.push(vec![0x4000_0003]);
        q.enemy_airport_names.push(vec![]);
        q.enemy_airport_objects.push(vec![]);
        q.theater(16);
        assert_eq!(q.theater_index(), 16);
        assert_eq!(q.value(13), "Ukraine (UKR1)");
        let base = source_theaters()
            .iter()
            .position(|name| *name == "UKR")
            .unwrap();
        assert_eq!(q.base_theater_index(), base);
        assert_eq!(&q.values(30)[..], &q.options.targets[base][..]);
        assert_eq!(&q.values(34)[..], ["Variant runway"]);
        q.apply(33, 1);
        assert_eq!(q.ground_runway(), Some(0x4000_0003));
        q.theater(0);
        assert_eq!(q.theater_index(), 0);
        assert_eq!(q.draft.values[34], 0);
    }
    #[test]
    fn the_player_list_has_only_base_theaters_and_a_variant_is_a_developer_addition() {
        let mut q = setup();
        assert_eq!(q.options.fields[13].len(), 16);
        assert!(q.theater_codes.iter().all(|c| !c.starts_with('~')));
        // No imported layouts in this fixture, so nothing is added; an
        // unknown code falls back to the first theater.
        q.choose_theater_code("~NOPE", &BTreeMap::new());
        assert_eq!(q.theater_index(), 0);
        assert_eq!(q.options.fields[13].len(), 16);
        q.choose_theater_code("UKR", &BTreeMap::new());
        assert_eq!(q.theater_codes[q.draft.values[13]], "UKR");
    }
    pub(super) fn setup() -> QuickMission {
        // The retail list sizes: a target list per theater (the templates'),
        // and the four strengths of the AAA and SAM fields.
        let mut options = Options {
            fields: vec![vec!["value".into(); 60]; 33],
            targets: tore_formats::quick_template::tables::TEMPLATES
                .iter()
                .map(|stems| {
                    (0..stems.len())
                        .map(|i| {
                            if i == 0 {
                                "none".into()
                            } else {
                                format!("target {i}")
                            }
                        })
                        .collect()
                })
                .collect(),
        };
        for id in [31, 32] {
            options.fields[id] = ["not", "lightly", "moderately", "heavily"]
                .map(String::from)
                .to_vec();
        }
        options.fields[15] = [
            "dawn",
            "clear",
            "cloudy",
            "overcast.",
            "foggy",
            "sunset",
            "night",
        ]
        .map(String::from)
        .to_vec();
        options.fields[4] = (0..6).map(|i| i.to_string()).collect();
        let mut q = QuickMission::new(AircraftId::F18, options, &BTreeMap::new());
        q.aircraft_names = vec!["Hornet".into(), "Rafale".into(), "Other".into()];
        q.aircraft_files = vec!["F18.PT".into(), "RAFALE.PT".into(), "OTHER.PT".into()];
        q.refresh_wing_catalog();
        q
    }
    /// A creator offering every selectable aircraft.
    pub(super) fn full_catalog() -> QuickMission {
        let mut q = setup();
        q.aircraft_files = AircraftId::SELECTABLE
            .map(|id| id.selection_key().to_string())
            .to_vec();
        q.aircraft_names = AircraftId::SELECTABLE
            .map(|id| id.label().to_string())
            .to_vec();
        q.refresh_wing_catalog();
        q.draft.values[6] = 0;
        for field in AI_AIRCRAFT_FIELDS {
            q.draft.values[field] = 0;
        }
        q
    }
    #[test]
    fn the_ais_wing_choices_list_every_aircraft_the_player_can_fly() {
        let q = full_catalog();
        for field in AI_AIRCRAFT_FIELDS {
            assert_eq!(q.values(field).len(), AircraftId::SELECTABLE.len());
            for id in AircraftId::SELECTABLE {
                assert!(
                    q.values(field).contains(&id.label().to_string()),
                    "{id:?} missing from field {field}"
                );
            }
        }
        // The armed helicopters since slice ST-H, the transport, the tiltrotor
        // and the vectored-thrust jets since slice H3.
        for id in [
            AircraftId::Ah64,
            AircraftId::Mi24,
            AircraftId::Ch47,
            AircraftId::V22,
            AircraftId::Av8,
            AircraftId::Yak141,
            AircraftId::Mig29,
        ] {
            assert!(q.ai_choice(id).is_some(), "{id:?}");
        }
        // The player's list is whole.
        assert_eq!(q.values(6).len(), AircraftId::SELECTABLE.len());
    }
    #[test]
    fn a_wing_field_names_the_aircraft_of_the_ais_list_not_the_players() {
        let mut q = full_catalog();
        // Every aircraft is the AI's since slice H3, so its list is cut here
        // (the first entry gone) to make it differ from the player's, as it
        // does whenever `AircraftId::ai_flyable` keeps something back.
        q.wing_files.remove(0);
        q.wing_names.remove(0);
        for field in [9, 12, 23, 26, 29] {
            let index = q.ai_choice(AircraftId::Mig17).unwrap();
            q.apply(field, index);
            assert_eq!(q.wing_aircraft(field), Some(AircraftId::Mig17));
        }
        // The same number in the player's field is a different aircraft.
        let index = q.ai_choice(AircraftId::Mig17).unwrap();
        q.apply(6, index);
        assert_ne!(q.player(), Some(AircraftId::Mig17));
        let spec = q.mission_spec().unwrap();
        assert_eq!(spec.wings[2].aircraft, AircraftId::Mig17);
        assert_eq!(spec.wings[5].aircraft, AircraftId::Mig17);
    }
    #[test]
    fn the_vertical_lifters_lead_wingmen_in_the_air_and_off_the_ground() {
        // The CH-47, the V-22, the AV-8 and the Yak-141 are offered to the AI
        // (slice H3), and since slice H2 their wingmen take off from a ground
        // start too (vertical or short takeoffs), as the AH-64's and Mi-24's.
        let mut q = full_catalog();
        q.airport_names[0] = vec!["First Field".into()];
        q.airport_objects[0] = vec![0x4000_0000];
        for id in [
            AircraftId::Ch47,
            AircraftId::V22,
            AircraftId::Av8,
            AircraftId::Yak141,
        ] {
            q.apply(33, 0);
            q.apply(6, 0);
            q.apply(4, 3);
            let index = q
                .aircraft_files
                .iter()
                .position(|f| f == id.selection_key())
                .unwrap();
            q.apply(6, index);
            assert_eq!(q.player(), Some(id));
            q.apply(4, 3);
            assert_eq!(q.draft.values[4], 3, "{id:?} airborne");
            assert!(q.unsupported().is_none());
            q.apply(33, 1);
            assert_eq!(q.draft.values[4], 3, "{id:?} ground");
            assert!(q.unsupported().is_none(), "{id:?} ground");
        }
    }
    #[test]
    fn a_helicopter_leads_ai_wingmen_in_the_air_and_off_the_ground() {
        let mut q = full_catalog();
        q.airport_names[0] = vec!["First Field".into()];
        q.airport_objects[0] = vec![0x4000_0000];
        for id in [AircraftId::Ah64, AircraftId::Mi24] {
            // Airborne, the AI flies the helicopter in friendly wing 1.
            q.apply(33, 0);
            q.apply(4, 0);
            let index = q
                .aircraft_files
                .iter()
                .position(|f| f == id.selection_key())
                .unwrap();
            q.apply(6, index);
            assert_eq!(q.player(), Some(id));
            q.apply(4, 3);
            assert_eq!(q.draft.values[4], 3, "{id:?}");
            assert!(q.unsupported().is_none());
            // A ground start keeps the wing (slice H2: the AI takes a
            // helicopter off the ground vertically).
            q.apply(33, 1);
            assert_eq!(q.draft.values[4], 3, "{id:?}");
            assert!(q.unsupported().is_none(), "{id:?}");
            // The AI's other wings take the helicopter at either start.
            for field in [9, 12] {
                q.apply(field, q.ai_choice(id).unwrap());
                assert_eq!(q.wing_aircraft(field), Some(id));
            }
        }
        // A fighter takes its wingmen off the ground as ever.
        q.apply(6, 0);
        q.apply(4, 3);
        assert_eq!(q.draft.values[4], 3);
        assert!(q.unsupported().is_none());
    }
    fn right_click(q: &mut QuickMission, id: usize) -> Action {
        q.hover = Some(id);
        assert!(matches!(q.right(true), Action::None));
        q.right(false)
    }
    #[test]
    fn group_objectives_are_independent_and_inactive_groups_keep_their_choice() {
        let mut q = setup();
        q.draft.values[7] = 0;
        q.open(OBJECTIVE_BASE + 1);
        q.cursor = QuickMission::objective_choices(1)
            .iter()
            .position(|(_, objective)| *objective == GroupObjective::Hold)
            .unwrap();
        q.activate(POP_OK);
        assert_eq!(q.group_objectives[1], GroupObjective::Hold);
        assert!(
            q.group_objectives
                .iter()
                .enumerate()
                .all(|(index, objective)| index == 1 || *objective == GroupObjective::Inherit)
        );
        q.draft.values[7] = 3;
        assert_eq!(q.group_objectives[1], GroupObjective::Hold);
    }

    #[test]
    fn survival_requirement_is_independent_of_orders_and_inactive_group_count() {
        let mut q = setup();
        let group = 1;
        q.draft.values[7] = 0;
        q.group_objectives[group] = GroupObjective::Intercept(WingId::new(Side::Enemy, 0).unwrap());
        assert!(matches!(q.activate(SURVIVAL_BASE + group), Action::Click));
        assert!(q.group_must_survive[group]);
        assert_eq!(q.value(SURVIVAL_BASE + group), "required");
        q.draft.values[7] = 3;
        assert!(q.group_must_survive[group]);
        assert_eq!(
            q.group_objectives[group],
            GroupObjective::Intercept(WingId::new(Side::Enemy, 0).unwrap())
        );
        assert!(matches!(
            right_click(&mut q, SURVIVAL_BASE + group),
            Action::Click
        ));
        assert!(!q.group_must_survive[group]);
        assert_eq!(q.value(SURVIVAL_BASE + group), "optional");
    }

    #[test]
    fn strike_is_a_choice_for_friendly_ai_wings_and_needs_a_ground_target() {
        for group in 0..OBJECTIVE_COUNT {
            let offered = QuickMission::objective_choices(group)
                .iter()
                .any(|(_, objective)| *objective == GroupObjective::Strike);
            assert_eq!(offered, matches!(group, 1 | 2), "group {group}");
        }
        let mut q = lobby_creator();
        q.group_objectives[1] = GroupObjective::Strike;
        q.apply(30, 0);
        assert_eq!(
            q.mission_spec().unwrap().objectives[1],
            GroupObjective::Inherit,
            "no ground target, no strike"
        );
        q.apply(30, 1);
        assert_eq!(
            q.mission_spec().unwrap().objectives[1],
            GroupObjective::Strike
        );
        assert_eq!(q.objective_label(1), "strike the ground target");
    }

    #[test]
    fn objective_choices_restrict_targets_by_side_and_exclude_self_escort() {
        for group in 0..OBJECTIVE_COUNT {
            let side = if group < 3 {
                Side::Friendly
            } else {
                Side::Enemy
            };
            let own_index = (group % 3) as u8;
            let choices = QuickMission::objective_choices(group);
            let intercepts: Vec<_> = choices
                .iter()
                .filter_map(|(_, objective)| match objective {
                    GroupObjective::Intercept(wing) => Some(*wing),
                    _ => None,
                })
                .collect();
            assert_eq!(intercepts.len(), 3);
            assert!(intercepts.iter().all(|wing| wing.side != side));
            let escorts: Vec<_> = choices
                .iter()
                .filter_map(|(_, objective)| match objective {
                    GroupObjective::Escort(wing) => Some(*wing),
                    _ => None,
                })
                .collect();
            assert_eq!(escorts.len(), 2);
            assert!(
                escorts
                    .iter()
                    .all(|wing| wing.side == side && wing.index != own_index)
            );
        }
    }

    #[test]
    fn objective_popup_and_right_click_cycle_only_the_addressed_group() {
        let mut q = setup();
        q.open(OBJECTIVE_BASE);
        q.cursor = 2;
        q.activate(POP_OK);
        assert_eq!(q.group_objectives[0], GroupObjective::Cap);
        assert!(matches!(right_click(&mut q, OBJECTIVE_BASE), Action::Click));
        assert_eq!(q.group_objectives[0], GroupObjective::Free);
        assert_eq!(q.group_objectives[1], GroupObjective::Inherit);
    }

    #[test]
    fn inherited_stamp_names_the_actual_mission_setting() {
        let mut q = setup();
        assert_eq!(q.objective_label(0), "use free fire (mission)");
        q.ai_mission = crate::ai_wings::Preset::Escort;
        assert_eq!(q.objective_label(5), "follow mission orders");
        q.ai_mission = crate::ai_wings::Preset::SelfDefense;
        assert_eq!(q.objective_label(2), "defend itself (mission)");
        q.group_objectives[2] = GroupObjective::Intercept(
            WingId::new(Side::Enemy, 2).expect("fixed Quick Mission wing"),
        );
        assert_eq!(q.objective_label(2), "enemy group 3");
    }
    #[test]
    fn all_six_skill_selectors_launch_dummy_members() {
        let mut q = setup();
        for field in [4, 7, 10, 21, 24, 27] {
            assert_eq!(q.values(field + 1)[4], "Dummy (400 KTS)");
            q.draft.values[field] = 2;
            q.draft.values[field + 1] = 4;
            q.draft.values[field + 2] = 0;
        }
        let wings = q
            .wing_launches(Some(EnemySkillOverride::AllAverage))
            .unwrap();
        assert_eq!(wings.len(), 6);
        assert_eq!(wings[0].count(), 1); // Human slot is excluded.
        assert!(wings.iter().all(|w| w.dummy));
        q.draft.values[22] = 3;
        assert!(!q.wing_launches(None).unwrap()[3].dummy);
    }

    #[test]
    fn right_click_reverses_values_wraps_and_keeps_player_count_positive() {
        let mut q = setup();
        q.apply(4, 2);
        assert!(matches!(right_click(&mut q, 4), Action::Click));
        assert_eq!(q.draft.values[4], 1);
        right_click(&mut q, 4);
        assert_eq!(q.draft.values[4], 5);
        q.apply(15, 1);
        right_click(&mut q, 15);
        assert_eq!(q.value(15), "dawn");
        right_click(&mut q, 15);
        assert_eq!(q.value(15), "night");
        q.activate(15); // Existing forward cycle reverses the last change.
        assert_eq!(q.value(15), "dawn");
        q.options.fields[14].clear();
        assert!(matches!(right_click(&mut q, 14), Action::None));
    }
    #[test]
    fn right_click_cannot_activate_actions_or_fields_behind_a_popup() {
        let mut q = setup();
        let before = q.draft.values;
        for id in [0, OK, CANCEL, 60, 61, POP_OK, POP_CANCEL, ROW_BASE] {
            assert!(matches!(right_click(&mut q, id), Action::None));
            assert_eq!(q.draft.values, before);
        }
        q.hover = Some(15);
        q.right(true);
        q.hover = Some(14);
        assert!(matches!(q.right(false), Action::None));
        q.hover = Some(15);
        q.right(true);
        q.cancel();
        q.hover = Some(15);
        assert!(matches!(q.right(false), Action::None));
        q.open(15);
        assert!(matches!(right_click(&mut q, 15), Action::None));
        assert_eq!(q.draft.values, before);
        q.cancel();
        q.help = true;
        assert!(matches!(right_click(&mut q, 15), Action::None));
        assert_eq!(q.draft.values, before);
        q.cancel();
        // With no target the defenses open like any field (B23: they are
        // the enemy's area defenses), and a cancel changes nothing.
        for field in [31, 32] {
            q.open(field);
            assert_eq!(q.selector, Some(field));
            q.cancel();
            assert_eq!(q.draft.values, before);
        }
        q.focus = 32;
        q.key(" ", false);
        q.activate(15);
        assert_ne!(q.draft.values[15], before[15]);
    }
    #[test]
    fn ground_start_airport_picker_cancels_and_resets_on_theater_change() {
        let mut q = setup();
        q.airport_names[0] = vec!["First Field".into(), "Second Field".into()];
        q.airport_objects[0] = vec![0x40000000, 0x40000003];
        assert!(!q.ground_start());
        q.activate(33);
        assert!(q.ground_start());
        q.activate(34);
        assert_eq!(q.selector, Some(34));
        q.activate(ROW_BASE + 1);
        q.activate(POP_CANCEL);
        assert_eq!(q.ground_runway(), Some(0x40000000));
        q.activate(34);
        q.activate(ROW_BASE + 1);
        q.activate(POP_OK);
        assert_eq!(q.ground_runway(), Some(0x40000003));
        // A theater with no field of the side takes the draft back to
        // Airborne and says why (slice AL1).
        q.apply(13, 1);
        assert_eq!(q.draft.values[34], 0);
        assert!(!q.ground_start() && q.ground_runway().is_none());
        assert_eq!(q.notice.as_deref(), Some(NO_FIELD_NOTICE));
        assert!(q.unsupported().is_none());
    }
    #[test]
    fn the_lobbys_creator_offers_both_starts_and_accepts_only_what_a_host_takes() {
        let mut q = setup();
        set_fields(
            &mut q,
            0,
            &[
                (0, "Blue Field", false, Some(false)),
                (1, "Red Field", false, Some(true)),
            ],
        );
        q.theater_codes = vec!["UKR".into(), "~UKR1".into()];
        // Single player keeps its own Start and its OK.
        q.activate(33);
        assert!(q.ground_start() && !q.lobby);
        // The lobby keeps a ground draft (agent decision, slice CL-R,
        // 2026-10-11) and says what Accept does.
        let saved = q.save();
        q.enter_lobby();
        assert!(q.lobby && q.ground_start());
        assert_eq!(q.notice.as_deref(), Some(LOBBY_NOTICE));
        // Start, Airport, Enemy start and Enemy airport all work there.
        q.notice = None;
        assert_eq!(right_click(&mut q, 33), Action::Click);
        assert!(!q.ground_start());
        assert_eq!(q.activate(33), Action::Click);
        assert!(q.ground_start() && q.notice.is_none());
        assert_eq!(q.activate(34), Action::Click);
        assert_eq!(q.selector, Some(34));
        q.activate(POP_CANCEL);
        assert_eq!(q.activate(ENEMY_START), Action::Click);
        assert!(q.enemy_ground_start());
        assert_eq!(q.activate(ENEMY_AIRPORT), Action::Click);
        assert_eq!(q.selector, Some(ENEMY_AIRPORT));
        q.activate(POP_OK);
        // The mission keeps both starts and goes to the host as text.
        let spec = q.lobby_spec().expect("a mission");
        assert_eq!(
            spec.start,
            Start::Ground {
                runway: 0x4000_0000,
                altitude_ft: 5_000
            }
        );
        assert_eq!(
            spec.enemy_start,
            EnemyStart::Ground {
                runway: 0x4000_0001
            }
        );
        assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
        // A developer layout is refused before Accept, with the ground
        // targets; it has no field of either side, so both starts go back
        // to Airborne.
        q.apply(13, 1);
        assert!(!q.ground_start() && !q.enemy_ground_start());
        assert!(q.lobby_problem().unwrap().contains("developer theater"));
        assert!(q.lobby_spec().is_err());
        q.apply(13, 0);
        // A ground target is a mission a host takes, and both starts can
        // be the world's own pick from it.
        q.apply(30, 1);
        q.apply(31, 1);
        assert!(q.lobby_problem().is_none());
        let spec = q.lobby_spec().expect("a mission with a target");
        assert_eq!(spec.ground_target.as_deref(), Some("QUSFLT"));
        assert_eq!((spec.aaa, spec.sam), (Defense::Light, Defense::None));
        q.apply(33, 1);
        q.apply(ENEMY_START, 1);
        let spec = q.lobby_spec().expect("both starts automatic");
        assert_eq!(spec.start, Start::GroundAuto { altitude_ft: 5_000 });
        assert_eq!(spec.enemy_start, EnemyStart::GroundAuto);
        assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
        // Accept sends what the lobby's mission is laid over with.
        let lobby = MissionSpec::new("UKR", AircraftId::F18);
        let sent = QuickMission::lay_over(&lobby, spec);
        assert_eq!(
            (sent.start, sent.enemy_start),
            (
                Start::GroundAuto { altitude_ft: 5_000 },
                EnemyStart::GroundAuto
            )
        );
        q.apply(30, 0);
        // OK is Accept: it still answers Mission, which the lobby takes.
        assert_eq!(q.activate(OK), Action::Mission);
        // Esc leaves with the notice up, and Cancel puts the draft back.
        assert_eq!(q.key("Escape", false), Action::Back);
        q.apply(4, 3);
        q.restore(saved);
        assert!(q.ground_start() && !q.enemy_ground_start());
        assert_ne!(q.draft.values[4], 3);
        q.leave_lobby();
        assert!(!q.lobby && q.notice.is_none());
    }
    // ---- the ground target and its defenses (surface-AI round, slice Q1) ----

    #[test]
    fn every_theaters_target_list_is_its_template_list_and_each_target_round_trips() {
        use tore_formats::quick_template::tables::{ENEMY_NATIONALITY, TEMPLATES, THEATERS};
        let mut q = lobby_creator();
        for (index, theater) in THEATERS.into_iter().enumerate() {
            let at = q.theater_codes.iter().position(|c| c == theater).unwrap();
            q.apply(13, at);
            assert_eq!(q.values(30).len(), TEMPLATES[index].len(), "{theater}");
            assert_eq!(q.draft.values[30], 0, "a theater starts with no target");
            for (target, stem) in TEMPLATES[index].iter().enumerate().skip(1) {
                q.apply(30, target);
                q.apply(31, target % 4);
                q.apply(32, (target + 1) % 4);
                let spec = q.mission_spec().unwrap();
                assert_eq!(spec.ground_target.as_deref(), Some(*stem), "{theater}");
                assert_eq!(spec.aaa.level(), target % 4);
                assert_eq!(spec.sam.level(), (target + 1) % 4);
                assert_eq!(
                    usize::from(spec.enemy_nationality),
                    ENEMY_NATIONALITY[index]
                );
                // The host takes it as text, and a creator reads it back.
                let sent = q.lobby_spec().unwrap();
                assert_eq!(MissionSpec::from_text(&sent.to_text()).unwrap(), sent);
                let draft = q.draft.values;
                q.load_spec(&sent, true).unwrap();
                assert_eq!(q.draft.values[30..=32], draft[30..=32], "{theater} {stem}");
                assert_eq!(q.lobby_spec().unwrap(), sent);
            }
            // The "nothing" entry is no target.
            q.apply(30, 0);
            assert_eq!(q.mission_spec().unwrap().ground_target, None);
        }
    }

    #[test]
    fn the_creator_shows_the_retail_strength_words_and_the_text_form_the_plain_ones() {
        let mut q = lobby_creator();
        q.apply(30, 1);
        for (level, retail, plain) in [
            (0, "not", "none"),
            (1, "lightly", "light"),
            (2, "moderately", "moderate"),
            (3, "heavily", "heavy"),
        ] {
            q.apply(31, level);
            q.apply(32, level);
            assert_eq!(
                (q.value(31).as_str(), q.value(32).as_str()),
                (retail, retail)
            );
            let text = q.mission_spec().unwrap().to_text();
            if level > 0 {
                assert!(
                    text.contains(&format!("defenses aaa {plain} sam {plain}")),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn the_defenses_stay_without_a_target_as_the_area_defenses() {
        let mut q = lobby_creator();
        // With no target the strengths cycle (B23): they set the enemy's
        // area defenses.
        assert_eq!(q.activate(31), Action::Click);
        assert_eq!(q.draft.values[31], 1);
        q.apply(32, 3);
        let spec = q.mission_spec().unwrap();
        assert_eq!(
            (spec.ground_target.as_deref(), spec.aaa, spec.sam),
            (None, Defense::Light, Defense::Heavy)
        );
        assert!(spec.area_defenses());
        assert!(spec.to_text().contains("\ndefenses aaa light sam heavy\n"));
        // With a target they are the target's.
        q.apply(30, 2);
        let spec = q.mission_spec().unwrap();
        assert!(spec.ground_target.is_some() && !spec.area_defenses());
        assert_eq!((spec.aaa, spec.sam), (Defense::Light, Defense::Heavy));
        // Back to none, and across a theater change, they stay.
        q.apply(30, 0);
        assert_eq!((q.draft.values[31], q.draft.values[32]), (1, 3));
        q.apply(13, 3);
        assert_eq!(q.draft.values[30..=32], [0, 1, 3]);
        // A spec read back into the creator keeps them.
        let spec = q.mission_spec().unwrap();
        let draft = Draft::from_spec(&spec, &q).unwrap();
        assert_eq!(draft.values[31..=32], [1, 3]);
    }

    #[test]
    fn the_flight_draws_the_surface_seed_and_a_restart_keeps_it() {
        let mut q = lobby_creator();
        q.lobby = false;
        // No target, no layout to seed: the mission text is what it was.
        assert_eq!(q.activate(OK), Action::Mission);
        assert_eq!(q.mission_spec().unwrap().surface_seed, 0);
        assert!(!q.mission_spec().unwrap().to_text().contains("surface-seed"));
        // Area defenses with no target draw one (B23).
        q.apply(32, 1);
        assert_eq!(q.activate(OK), Action::Mission);
        assert_ne!(q.mission_spec().unwrap().surface_seed, 0);
        q.apply(32, 0);
        q.apply(30, 1);
        assert_eq!(q.activate(OK), Action::Mission);
        let seed = q.mission_spec().unwrap().surface_seed;
        assert_ne!(seed, 0);
        // The restart builds the same spec again; the next flight rolls anew.
        assert_eq!(q.mission_spec().unwrap().surface_seed, seed);
        assert!(
            q.mission_spec()
                .unwrap()
                .to_text()
                .contains(&format!("surface-seed {seed}\n"))
        );
        let seeds: std::collections::BTreeSet<u32> = (0..50)
            .map(|_| {
                q.roll_surface_seed();
                q.surface_seed
            })
            .collect();
        assert!(seeds.len() > 40 && !seeds.contains(&0), "{seeds:?}");
        // The host keeps the seed whose placements and distances were checked.
        q.surface_seed = 99;
        q.lobby = true;
        assert_eq!(q.lobby_spec().unwrap().surface_seed, 99);
    }

    #[test]
    fn the_enemy_nationality_is_the_missions_and_a_theaters_own_is_not_written() {
        let mut q = lobby_creator();
        let default = q.mission_spec().unwrap();
        assert_eq!(
            default.enemy_nationality,
            default_enemy_nationality(&default.theater)
        );
        assert!(!default.to_text().contains("enemy-nationality"));
        q.apply(20, 5);
        let chosen = q.lobby_spec().unwrap();
        assert_eq!(chosen.enemy_nationality, 5);
        assert!(chosen.to_text().contains("enemy-nationality 5\n"));
        // A creator opened on that mission has it, and the read-only one
        // shows it instead of "as the King's".
        let mut other = lobby_creator();
        other.load_spec(&chosen, true).unwrap();
        assert_eq!(other.draft.values[20], 5);
        other.lobby = false;
        other
            .open_lobby_mission(&chosen, Some(ViewKind::Reader))
            .unwrap();
        assert_eq!(other.value(20), "value");
        assert!(other.view_tint(20).is_none());
        assert!(NOT_CARRIED.iter().all(|field| *field != 20));
        // Laid over the lobby's mission, it goes with the rest.
        let lobby = MissionSpec::new("UKR", AircraftId::F18);
        assert_eq!(QuickMission::lay_over(&lobby, chosen).enemy_nationality, 5);
    }

    #[test]
    fn a_ground_start_with_a_target_is_auto_until_a_runway_is_picked() {
        let mut q = setup();
        q.airport_names[0] = vec!["First Field".into(), "Second Field".into()];
        q.airport_objects[0] = vec![0x4000_0000, 0x4000_0003];
        q.apply(33, 1);
        // With no target the list is the airports, the first one chosen.
        assert_eq!(&q.values(34)[..], ["First Field", "Second Field"]);
        assert_eq!(q.ground_runway(), Some(0x4000_0000));
        assert!(!q.ground_auto());
        assert!(matches!(
            q.mission_spec().unwrap().start,
            Start::Ground { .. }
        ));
        // A target brings the world's own pick in front, and it is the default.
        q.apply(30, 1);
        assert_eq!(q.values(34)[0], AUTO_AIRPORT);
        assert_eq!(q.values(34).len(), 3);
        assert!(q.ground_auto() && q.ground_runway().is_none());
        assert_eq!(q.value(34), AUTO_AIRPORT);
        assert!(q.unsupported().is_none());
        let spec = q.mission_spec().unwrap();
        assert_eq!(spec.start, Start::GroundAuto { altitude_ft: 5_000 });
        assert!(spec.to_text().contains("start ground auto\n"));
        // Picking an airport by name keeps it.
        q.apply(34, 2);
        assert_eq!(q.ground_runway(), Some(0x4000_0003));
        assert!(!q.ground_auto());
        assert_eq!(
            q.mission_spec().unwrap().start,
            Start::Ground {
                runway: 0x4000_0003,
                altitude_ft: 5_000
            }
        );
        q.choose_ground_runway(0x4000_0000).unwrap();
        assert_eq!(q.ground_runway(), Some(0x4000_0000));
        assert_eq!(
            q.draft.values[34], 1,
            "the first airport follows the auto row"
        );
        // Dropping the target puts the list back to the airports.
        q.apply(30, 0);
        assert_eq!(q.draft.values[34], 0);
        assert_eq!(q.ground_runway(), Some(0x4000_0000));
        // A theater with no airports of the side has no ground start, even
        // on auto with a target: the world would find no field (slice AL1).
        q.apply(13, 3);
        q.airport_names[3].clear();
        q.airport_objects[3].clear();
        q.apply(33, 1);
        assert!(!q.ground_start());
        assert_eq!(q.notice.as_deref(), Some(NO_FIELD_NOTICE));
        q.apply(30, 1);
        assert!(!q.ground_auto() && q.unsupported().is_none());
        q.apply(13, 0);
        q.apply(33, 1);
        q.apply(30, 1);
        assert!(q.ground_auto());
        // The lobby keeps the automatic start (slice CL-R).
        q.lobby = true;
        assert_eq!(
            q.lobby_spec().unwrap().start,
            Start::GroundAuto { altitude_ft: 5_000 }
        );
    }

    #[test]
    fn the_read_only_creator_shows_the_target_and_its_defenses_and_changes_none() {
        let mut spec = MissionSpec::new("EGY", AircraftId::F18);
        spec.ground_target = Some(ground_targets("EGY")[2].to_string());
        spec.aaa = Defense::Heavy;
        spec.sam = Defense::Light;
        let mut q = viewer(&spec);
        assert_eq!(q.draft.values[30..=32], [3, 3, 1]);
        assert_eq!(
            [q.value(30), q.value(31), q.value(32)],
            ["target 3", "heavily", "lightly"]
        );
        for field in [30, 31, 32] {
            q.notice = None;
            assert_eq!(q.activate(field), Action::Click);
            assert_eq!(q.notice.as_deref(), Some(VIEW_REFUSAL));
            assert!(q.selector.is_none());
        }
        assert_eq!(q.draft.values[30..=32], [3, 3, 1]);
        assert_eq!(q.lobby_spec().unwrap(), spec);
        // A target this creator's list has no entry for is not silently lost.
        let mut odd = spec.clone();
        odd.ground_target = Some("QEARMOR".into());
        let mut short = lobby_creator();
        short.options.targets[2].truncate(3);
        assert!(short.load_spec(&odd, true).unwrap_err().contains("QEARMOR"));
    }

    /// The words the host would give for the Rafale and for a theater.
    fn some_gaps(q: &mut QuickMission) {
        let theater = q.theater_codes[1].clone();
        q.set_gaps(|kind, key| match (kind, key) {
            (ItemKind::Aircraft, "RAFALE.PT") => {
                Some("Not everyone can fly the Rafale: Hawk's game has no Rafale.".into())
            }
            (ItemKind::Theater, code) if code == theater => Some(format!(
                "Not everyone has {code}: Hawk's game has no {code}."
            )),
            _ => None,
        });
    }
    #[test]
    fn the_lobbys_creator_refuses_a_choice_in_a_gap_and_keeps_the_old_one() {
        let mut q = setup();
        // Single player knows no gaps: the same list and clicks choose it.
        some_gaps(&mut q);
        assert!(q.gap_in(6, 1).is_none());
        q.open(6);
        q.cursor = 1;
        q.activate(POP_OK);
        assert_eq!(q.draft.values[6], 1);
        assert!(q.notice.is_none());
        q.apply(6, 0);

        q.enter_lobby();
        some_gaps(&mut q);
        // Rafale is the second aircraft, in a gap, for every aircraft field
        // and for the theater at its own index.
        for field in [6, 9, 12, 23, 26, 29] {
            assert!(q.gap_in(field, 1).is_some(), "{field}");
            assert!(q.gap_in(field, 0).is_none() && q.gap_in(field, 2).is_none());
        }
        assert!(q.gap_in(13, 1).is_some() && q.gap_in(13, 0).is_none());
        assert!(q.gap_in(15, 1).is_none(), "other fields have no gaps");
        // Choosing it in the list leaves the choice as it was, closes the
        // list and says who lacks it.
        q.open(6);
        q.cursor = 1;
        q.activate(POP_OK);
        assert_eq!(q.draft.values[6], 0);
        assert_eq!(q.aircraft_selection, 0);
        assert!(q.selector.is_none());
        assert_eq!(
            q.notice.as_deref(),
            Some("Not everyone can fly the Rafale: Hawk's game has no Rafale.")
        );
        // What everyone has is chosen as before, and the notice clears.
        q.open(6);
        q.cursor = 2;
        q.activate(POP_OK);
        assert_eq!(q.draft.values[6], 2);
        assert!(q.notice.is_none());
        // The shifted click that steps to the next value, and the right
        // click that steps back, refuse the same way.
        q.apply(9, 0);
        q.shift = true;
        q.focus = 9;
        q.activate(9);
        assert_eq!(q.draft.values[9], 0);
        assert!(q.notice.as_deref().unwrap().contains("Rafale"));
        q.notice = None;
        q.apply(9, 2);
        right_click(&mut q, 9);
        assert_eq!(q.draft.values[9], 2);
        assert!(q.notice.as_deref().unwrap().contains("Rafale"));
        // A theater in a gap is refused in its list too.
        q.notice = None;
        q.open(13);
        q.cursor = 1;
        q.activate(POP_OK);
        assert_eq!(q.draft.values[13], 0);
        assert!(q.notice.as_deref().unwrap().starts_with("Not everyone has"));
        // Leaving the lobby's creator forgets them.
        q.leave_lobby();
        assert!(q.gaps.is_empty() && q.gap_in(6, 1).is_none());
    }
    #[test]
    fn accept_checks_the_whole_mission_for_gaps_and_ignores_a_wing_nobody_flies() {
        let mut q = setup();
        q.enter_lobby();
        some_gaps(&mut q);
        assert!(q.lobby_problem().is_none());
        // The player's own aircraft.
        q.draft.values[6] = 1;
        assert!(q.lobby_problem().unwrap().contains("Rafale"));
        assert!(q.lobby_spec().is_err());
        q.draft.values[6] = 0;
        // A wing that flies, and a wing with no aircraft in it (count 0),
        // which the host does not count either.
        q.draft.values[9] = 1;
        assert_eq!(q.draft.values[7], 0);
        assert!(q.lobby_problem().is_none());
        q.draft.values[23] = 1;
        assert!(q.draft.values[21] > 0);
        assert!(q.lobby_problem().unwrap().contains("Rafale"));
        q.draft.values[23] = 0;
        // The theater.
        q.draft.values[13] = 1;
        assert!(q.lobby_problem().unwrap().starts_with("Not everyone has"));
        q.draft.values[13] = 0;
        assert!(q.lobby_spec().is_ok());
        // OK is Accept and reaches the lobby only with a clean draft; the
        // refusal is the host's own line in the notice.
        q.draft.values[6] = 1;
        assert_eq!(q.activate(OK), Action::Mission);
        assert!(q.lobby_spec().is_err());
        // Single player's creator never asks.
        q.leave_lobby();
        assert!(q.lobby_problem().is_none());
        assert!(q.lobby_spec().is_ok());
    }
    #[test]
    fn the_creators_gap_words_come_from_the_clients_refusals_for_what_it_lists() {
        let mut q = setup();
        q.enter_lobby();
        let mut asked = Vec::new();
        let asked_cell = std::cell::RefCell::new(&mut asked);
        q.set_gaps(|kind, key| {
            asked_cell.borrow_mut().push((kind, key.to_owned()));
            None
        });
        // Every aircraft and every base theater, nothing else.
        assert_eq!(
            asked
                .iter()
                .filter(|(kind, _)| *kind == ItemKind::Aircraft)
                .count(),
            q.aircraft_files.len()
        );
        assert_eq!(
            asked
                .iter()
                .filter(|(kind, _)| *kind == ItemKind::Theater)
                .count(),
            q.theater_codes.len()
        );
        assert!(asked.iter().all(|(kind, _)| *kind != ItemKind::Weapon));
        assert!(q.gaps.is_empty());
    }
    /// CL-R2: the map is built on a thread of its own. A result for settings
    /// that have since changed is dropped and the map is asked for again; one
    /// for the settings in force is taken; Escape and Cancel give the build up.
    #[test]
    fn a_background_map_build_is_taken_dropped_when_stale_and_given_up_on_leaving() {
        let mut q = setup();
        let spec = q.map_mission().unwrap().0;
        let (send, receive) = std::sync::mpsc::channel();
        q.map_build = Some(map::Building::from_channel(spec.clone(), receive));
        assert!(q.map_building());
        // Nothing has come: the creator carries on, building.
        q.poll_map();
        assert!(q.map_building() && q.notice.is_none());
        // The answer for the settings in force is taken (here a refusal).
        send.send(Err("The map could not be built: test".into()))
            .unwrap();
        q.poll_map();
        assert!(!q.map_building());
        assert_eq!(
            q.notice.as_deref(),
            Some("The map could not be built: test")
        );
        // The settings change while it builds: the answer is stale, so it is
        // dropped and the build is asked for again (this creator has no game
        // data, which is what the second try says).
        q.notice = None;
        let (send, receive) = std::sync::mpsc::channel();
        q.map_build = Some(map::Building::from_channel(spec, receive));
        q.apply(15, 2);
        send.send(Err("old settings".into())).unwrap();
        q.poll_map();
        assert_eq!(
            q.notice.as_deref(),
            Some("The map needs the imported game data.")
        );
        // Escape gives the build up first, and Cancel leaves it behind.
        for leave in [0, 1] {
            let spec = q.map_mission().unwrap().0;
            let (_send, receive) = std::sync::mpsc::channel();
            q.map_build = Some(map::Building::from_channel(spec, receive));
            if leave == 0 {
                assert_eq!(q.key("Escape", false), Action::None);
            } else {
                assert_eq!(q.activate(CANCEL), Action::Back);
            }
            assert!(!q.map_building(), "leave {leave}");
        }
    }
    /// Leaving the lobby's creator (the host started the flight) forgets the
    /// map: a build in progress must not open a map over the next creator,
    /// and the hold a snapshot puts on it goes too.
    #[test]
    fn leaving_the_lobby_drops_the_map_and_its_build() {
        let mut q = setup();
        q.enter_lobby();
        let spec = q.map_mission().unwrap().0;
        let (_send, receive) = std::sync::mpsc::channel();
        q.map_build = Some(map::Building::from_channel(spec, receive));
        q.map_hold = true;
        q.leave_lobby();
        assert!(!q.map_open() && !q.map_building() && !q.map_hold);
        // The build cannot come back as a map or a notice.
        q.poll_map();
        assert!(!q.map_open() && !q.map_building() && q.notice.is_none());
    }
    /// Checking a finished build against the draft draws no surface seed, so
    /// an abandoned build cannot change the next flight's layout or start a
    /// second build.
    #[test]
    fn taking_a_finished_map_build_draws_no_surface_seed() {
        let mut q = setup();
        q.apply(30, 1);
        let spec = q.map_mission().unwrap().0;
        assert!(q.seed_pending);
        // The flight takes the pending seed, as OK does.
        q.roll_surface_seed();
        assert!(!q.seed_pending);
        let seed = q.surface_seed;
        let (send, receive) = std::sync::mpsc::channel();
        q.map_build = Some(map::Building::from_channel(spec, receive));
        send.send(Err("The map could not be built: test".into()))
            .unwrap();
        q.poll_map();
        assert_eq!((q.surface_seed, q.seed_pending), (seed, false));
        assert!(!q.map_building());
        assert_eq!(
            q.notice.as_deref(),
            Some("The map could not be built: test")
        );
    }
    /// A build that dies without answering is reported, not waited on.
    #[test]
    fn a_map_build_that_dies_says_so() {
        let mut q = setup();
        let spec = q.map_mission().unwrap().0;
        let (send, receive) = std::sync::mpsc::channel::<Result<map::Planning, String>>();
        q.map_build = Some(map::Building::from_channel(spec, receive));
        drop(send);
        q.poll_map();
        assert!(!q.map_building());
        assert_eq!(q.notice.as_deref(), Some("The map could not be built."));
    }
    /// A theater's airports for the creator: `(ordinal, name, short, owner)`.
    fn fields(list: &[(u32, &str, bool, Option<bool>)]) -> Vec<Field> {
        list.iter()
            .map(|(ordinal, name, short, redfor)| Field {
                id: 0x4000_0000 + ordinal,
                name: (*name).into(),
                short: *short,
                stol: *short,
                redfor: *redfor,
            })
            .collect()
    }

    /// Gives the theater at `index` these airports, as an import would:
    /// both sides' offered lists are cut from them.
    fn set_fields(q: &mut QuickMission, index: usize, list: &[(u32, &str, bool, Option<bool>)]) {
        q.fields[index] = fields(list);
        let (names, objects) = offered(&q.fields[index], true);
        q.enemy_airport_names[index] = names;
        q.enemy_airport_objects[index] = objects;
        q.set_player_redfor(q.player_redfor);
    }
    /// Red, Blue and neutral fields and a Red STOL strip.
    fn mixed_fields(q: &mut QuickMission) {
        set_fields(
            q,
            0,
            &[
                (0, "Red Field", false, Some(true)),
                (1, "Blue Field", false, Some(false)),
                (2, "Open Field", false, None),
                (3, "Red Strip", true, Some(true)),
            ],
        );
    }

    #[test]
    fn the_enemy_airport_list_is_redfors_with_the_world_pick_first_with_a_target() {
        let mut q = setup();
        mixed_fields(&mut q);
        assert_eq!(
            &q.values(ENEMY_AIRPORT)[..],
            ["Red Field", "Open Field", "Red Strip (STOL)"]
        );
        assert_eq!(&q.values(34)[..], ["Blue Field", "Open Field"]);
        assert_eq!(
            q.offered_enemy_airports(0),
            [0x4000_0000, 0x4000_0002, 0x4000_0003]
        );
        // The pilot's side does not change the enemy's list.
        q.set_player_redfor(true);
        assert_eq!(
            &q.values(ENEMY_AIRPORT)[..],
            ["Red Field", "Open Field", "Red Strip (STOL)"]
        );
        q.set_player_redfor(false);
        // Airborne by default, the mission's default.
        assert_eq!(&q.values(ENEMY_START)[..], ["Airborne", "Ground"]);
        assert!(!q.enemy_ground_start());
        assert_eq!(q.enemy_start(), EnemyStart::Airborne);
        assert_eq!(q.mission_spec().unwrap().enemy_start, EnemyStart::Airborne);
        assert!(!q.mission_spec().unwrap().to_text().contains("enemy-start"));
        // Ground: the first Redfor field, and the list chooses another.
        q.activate(ENEMY_START);
        assert!(q.enemy_ground_start());
        assert_eq!(
            q.mission_spec().unwrap().enemy_start,
            EnemyStart::Ground {
                runway: 0x4000_0000
            }
        );
        q.activate(ENEMY_AIRPORT);
        assert_eq!(q.selector, Some(ENEMY_AIRPORT));
        q.activate(ROW_BASE + 1);
        q.activate(POP_OK);
        assert_eq!(q.enemy_ground_runway(), Some(0x4000_0002));
        let spec = q.mission_spec().unwrap();
        assert_eq!(MissionSpec::from_text(&spec.to_text()).unwrap(), spec);
        // A target puts the world's own pick first, and it is the default.
        q.apply(30, 1);
        assert_eq!(q.values(ENEMY_AIRPORT)[0], AUTO_AIRPORT);
        assert_eq!(q.values(ENEMY_AIRPORT).len(), 4);
        assert_eq!(q.value(ENEMY_AIRPORT), AUTO_AIRPORT);
        assert!(q.enemy_ground_auto() && q.enemy_ground_runway().is_none());
        assert_eq!(q.enemy_start(), EnemyStart::GroundAuto);
        assert!(q.unsupported().is_none());
        assert!(
            q.mission_spec()
                .unwrap()
                .to_text()
                .contains("enemy-start ground auto\n")
        );
        q.choose_enemy_runway(0x4000_0000).unwrap();
        assert_eq!(q.draft.values[ENEMY_AIRPORT], 1, "after the auto row");
        assert_eq!(
            q.enemy_start(),
            EnemyStart::Ground {
                runway: 0x4000_0000
            }
        );
        // Dropping the target puts the list back to the airports.
        q.apply(30, 0);
        assert_eq!(q.draft.values[ENEMY_AIRPORT], 0);
        assert_eq!(q.enemy_ground_runway(), Some(0x4000_0000));
        // The player's own start is untouched by all of it.
        assert!(!q.ground_start());
    }

    #[test]
    fn the_map_entry_says_why_it_cannot_open_and_changes_nothing() {
        let mut q = setup();
        let before = q.draft.values;
        // No imported data in a test: the notice says so, the creator stays.
        assert_eq!(q.activate(MAP), Action::Click);
        assert!(!q.map_open());
        assert_eq!(
            q.notice.as_deref(),
            Some("The map needs the imported game data.")
        );
        assert_eq!(q.draft.values, before);
        // The read-only page has the Map too: it is not refused as a change.
        let spec = MissionSpec::new("UKR", AircraftId::F18);
        let mut view = viewer(&spec);
        view.notice = None;
        assert_eq!(view.activate(MAP), Action::Click);
        assert_ne!(view.notice.as_deref(), Some(VIEW_REFUSAL));
    }

    #[test]
    fn a_seed_drawn_ahead_of_the_flight_is_the_one_ok_flies() {
        let mut q = setup();
        let stem = ground_targets(&q.theater_codes[0])[0].to_string();
        q.choose_ground_target(&stem, (1, 1), 4242).unwrap();
        // OK keeps the seed the map (or `--surface-seed`) drew, once.
        assert_eq!(q.activate(OK), Action::Mission);
        assert_eq!(q.mission_spec().unwrap().surface_seed, 4242);
        assert_eq!(q.activate(OK), Action::Mission);
        assert_ne!(q.mission_spec().unwrap().surface_seed, 4242);
        // Without a target the kept strengths are area defenses (B23): a
        // seed is drawn. With neither there is nothing to keep.
        q.apply(30, 0);
        q.roll_surface_seed();
        assert_ne!(q.surface_seed, 0);
        q.seed_pending = true;
        q.apply(31, 0);
        q.apply(32, 0);
        q.roll_surface_seed();
        assert_eq!((q.surface_seed, q.seed_pending), (0, false));
    }

    #[test]
    fn an_enemy_ground_start_refuses_a_blue_field_and_takes_a_stol_strip() {
        let mut q = setup();
        mixed_fields(&mut q);
        let error = q.choose_enemy_runway(0x4000_0001).unwrap_err();
        assert_eq!(
            error,
            "Blue Field is a Blue airfield: no enemy ground start there. Choose a Redfor airfield or start the enemy Airborne."
        );
        let error = q.choose_enemy_runway(0x4000_0999).unwrap_err();
        assert!(error.contains("No imported runway matches"), "{error}");
        assert!(!q.enemy_ground_start(), "a refusal changes nothing");
        // A Red STOL strip is a ground start too (John, 2026-10-11).
        q.choose_enemy_runway(0x4000_0003).unwrap();
        assert_eq!(q.enemy_ground_runway(), Some(0x4000_0003));
        // A neutral field is offered to both sides.
        q.choose_enemy_runway(0x4000_0002).unwrap();
        assert_eq!(q.enemy_ground_runway(), Some(0x4000_0002));
        q.choose_ground_runway(0x4000_0002).unwrap();
        // But not to both at once.
        assert!(q.unsupported().unwrap().contains("same airfield"));
        assert_eq!(q.activate(OK), Action::Click);
        q.choose_ground_runway(0x4000_0001).unwrap();
        assert!(q.unsupported().is_none());
    }

    #[test]
    fn an_enemy_ground_start_needs_aircraft_in_enemy_wing_1() {
        let mut q = setup();
        mixed_fields(&mut q);
        q.apply(ENEMY_START, 1);
        q.apply(21, 0);
        assert_eq!(q.unsupported().as_deref(), Some(EMPTY_ENEMY_WING));
        assert_eq!(q.activate(OK), Action::Click);
        assert_eq!(q.notice.as_deref(), Some(EMPTY_ENEMY_WING));
        q.lobby = true;
        assert_eq!(q.lobby_spec().unwrap_err(), EMPTY_ENEMY_WING);
        q.lobby = false;
        // Airborne, an empty enemy wing 1 is fine, as before.
        q.apply(ENEMY_START, 0);
        assert!(q.unsupported().is_none());
        q.apply(ENEMY_START, 1);
        q.apply(21, 2);
        assert!(q.unsupported().is_none());
        assert_eq!(q.activate(OK), Action::Mission);
    }

    #[test]
    fn a_theater_with_no_redfor_field_keeps_the_enemy_airborne_and_says_why() {
        let mut q = setup();
        set_fields(&mut q, 0, &[(0, "Blue Field", false, Some(false))]);
        set_fields(&mut q, 1, &[(0, "Red Field", false, Some(true))]);
        assert!(q.enemy_ground_unavailable() && !q.ground_unavailable());
        // Ground is dimmed in the Enemy start list and refused with the
        // notice; the field is greyed and the enemy stays airborne.
        assert_eq!(q.gap_in(ENEMY_START, 1), Some(NO_ENEMY_FIELD_NOTICE));
        assert!(q.gap_in(ENEMY_START, 0).is_none());
        q.activate(ENEMY_START);
        assert!(!q.enemy_ground_start());
        assert_eq!(q.notice.as_deref(), Some(NO_ENEMY_FIELD_NOTICE));
        q.apply(ENEMY_START, 1);
        assert!(!q.enemy_ground_start());
        assert_eq!(q.view_tint(ENEMY_START), Some(GAP_FIELD_TEXT));
        assert_eq!(q.mission_spec().unwrap().enemy_start, EnemyStart::Airborne);
        // A theater change starts the Enemy airport over, and onto a
        // theater without a Redfor field takes the enemy back to Airborne.
        q.apply(13, 1);
        assert!(q.view_tint(ENEMY_START).is_none());
        q.apply(ENEMY_START, 1);
        q.apply(30, 1);
        q.apply(ENEMY_AIRPORT, 1);
        q.apply(13, 0);
        assert!(!q.enemy_ground_start());
        assert_eq!(q.draft.values[ENEMY_AIRPORT], 0);
        assert_eq!(q.notice.as_deref(), Some(NO_ENEMY_FIELD_NOTICE));
        // The same through `--theater`.
        q.theater(1);
        q.apply(ENEMY_START, 1);
        q.apply(30, 1);
        q.apply(ENEMY_AIRPORT, 1);
        q.theater(0);
        assert!(!q.enemy_ground_start());
        assert_eq!(q.draft.values[ENEMY_AIRPORT], 0);
    }

    #[test]
    fn the_picker_offers_the_pilots_side_and_neutral_fields_and_defaults_to_one() {
        let mut q = setup();
        q.fields[0] = fields(&[
            (0, "Red Field", false, Some(true)),
            (1, "Blue Field", false, Some(false)),
            (2, "Open Field", false, None),
            (3, "Blue Strip", true, Some(false)),
        ]);
        q.set_player_redfor(false);
        q.apply(33, 1);
        // Blue: its own and the neutral field, never the enemy's, and the
        // default is its first, not the layout's first (an enemy field). Its
        // STOL strip comes last, labelled (John, 2026-10-11).
        assert_eq!(
            &q.values(34)[..],
            ["Blue Field", "Open Field", "Blue Strip (STOL)"]
        );
        assert_eq!(q.ground_runway(), Some(0x4000_0001));
        let error = q.choose_ground_runway(0x4000_0000).unwrap_err();
        assert!(error.contains("Red Field is an enemy airfield"), "{error}");
        assert_eq!(q.ground_runway(), Some(0x4000_0001));
        // A Redfor pilot sees the Red field and the neutral one.
        q.set_player_redfor(true);
        q.apply(33, 1);
        assert_eq!(&q.values(34)[..], ["Red Field", "Open Field"]);
        assert_eq!(q.ground_runway(), Some(0x4000_0000));
        assert!(q.choose_ground_runway(0x4000_0001).is_err());
        q.choose_ground_runway(0x4000_0002).unwrap();
        assert_eq!(q.ground_runway(), Some(0x4000_0002));
    }

    #[test]
    fn a_theater_with_no_field_of_the_side_starts_airborne_and_says_why() {
        let mut q = setup();
        q.fields[0] = fields(&[
            (0, "Red Field", false, Some(true)),
            // A STOL strip is a ground start (John, 2026-10-11), so only
            // the other side's fields leave Blue without one.
            (1, "Red Strip", true, Some(true)),
        ]);
        q.set_player_redfor(false);
        assert!(q.ground_unavailable());
        // Ground is dimmed in the Start list and refused with the notice,
        // by a click and by the list alike; the start stays airborne.
        assert_eq!(q.gap_in(33, 1), Some(NO_FIELD_NOTICE));
        assert!(q.gap_in(33, 0).is_none());
        q.activate(33);
        assert!(!q.ground_start());
        assert_eq!(q.notice.as_deref(), Some(NO_FIELD_NOTICE));
        q.apply(33, 1);
        assert!(!q.ground_start());
        assert_eq!(q.notice.as_deref(), Some(NO_FIELD_NOTICE));
        // The Start field is greyed.
        assert_eq!(q.view_tint(33), Some(GAP_FIELD_TEXT));
        assert!(matches!(
            q.mission_spec().unwrap().start,
            Start::Airborne { .. }
        ));
        // A theater change onto it takes a ground draft back to Airborne.
        q.fields[1] = fields(&[(0, "Blue Field", false, Some(false))]);
        q.set_player_redfor(false);
        q.apply(13, 1);
        q.apply(33, 1);
        assert!(q.ground_start());
        q.apply(13, 0);
        assert!(!q.ground_start());
        assert_eq!(q.notice.as_deref(), Some(NO_FIELD_NOTICE));
        // Redfor has a field there.
        q.set_player_redfor(true);
        assert!(!q.ground_unavailable() && q.gap_in(33, 1).is_none());
        q.apply(33, 1);
        assert_eq!(q.ground_runway(), Some(0x4000_0000));
    }

    /// John, 2026-10-11: a short runway is labelled STOL and is a ground
    /// start like any other; it is listed after the longer fields, so the
    /// default stays a field any aircraft can take off from.
    #[test]
    fn a_stol_strip_is_labelled_and_offered_last() {
        let mut q = setup();
        q.fields[0] = fields(&[
            (3, "Goose Green", true, None),
            (0, "Long Field", false, None),
        ]);
        q.set_player_redfor(false);
        assert_eq!(&q.values(34)[..], ["Long Field", "Goose Green (STOL)"]);
        q.apply(33, 1);
        assert_eq!(q.ground_runway(), Some(0x4000_0000));
        q.choose_ground_runway(0x4000_0003).unwrap();
        assert!(q.ground_start());
        assert_eq!(q.ground_runway(), Some(0x4000_0003));
        // A theater whose only field of the side is a STOL strip still
        // offers Ground there.
        q.fields[0] = fields(&[(3, "San Carlos", true, Some(false))]);
        q.set_player_redfor(false);
        assert_eq!(&q.values(34)[..], ["San Carlos (STOL)"]);
        // Something that is no runway keeps the old message.
        let error = q.choose_ground_runway(0x4000_0999).unwrap_err();
        assert!(error.contains("No imported runway matches"), "{error}");
    }
    #[test]
    fn tab_reaches_ground_controls_without_focusing_hidden_airport() {
        let mut q = setup();
        q.airport_names[0] = vec!["First Field".into()];
        q.airport_objects[0] = vec![0x4000_0000];
        q.focus = 32;
        q.key("Tab", false);
        assert_eq!(q.focus, 33);
        q.key("Tab", false);
        assert_eq!(q.focus, ENEMY_START);
        q.key("Tab", false);
        assert_eq!(q.focus, OBJECTIVE_BASE);
        for expected in OBJECTIVE_BASE + 1..SURVIVAL_BASE + OBJECTIVE_COUNT {
            q.key("Tab", false);
            assert_eq!(q.focus, expected);
        }
        q.key("Tab", false);
        assert_eq!(q.focus, 1);
        q.apply(33, 1);
        q.focus = 33;
        q.key("Tab", false);
        assert_eq!(q.focus, 34);
        q.key("Tab", false);
        assert_eq!(q.focus, ENEMY_START);
        q.key("Tab", false);
        assert_eq!(q.focus, OBJECTIVE_BASE);
        q.key("ArrowUp", false);
        assert_eq!(q.focus, ENEMY_START);
        // The Enemy airport is reached once the enemy starts on the ground.
        q.enemy_airport_names[0] = vec!["Red Field".into()];
        q.enemy_airport_objects[0] = vec![0x4000_0001];
        q.apply(ENEMY_START, 1);
        q.key("Tab", false);
        assert_eq!(q.focus, ENEMY_AIRPORT);
        q.key("Tab", false);
        assert_eq!(q.focus, OBJECTIVE_BASE);
        q.focus = 1;
        q.key("Tab", true);
        assert_eq!(q.focus, SURVIVAL_BASE + OBJECTIVE_COUNT - 1);
        q.key("ArrowDown", false);
        assert_eq!(q.focus, 1);
    }
    #[test]
    fn selectors_page_and_cancel_without_changing_the_draft() {
        let mut q = setup();
        q.activate(13);
        assert_eq!(q.selector, Some(13));
        q.activate(DOWN);
        assert_eq!(q.scroll, 15);
        q.activate(ROW_BASE);
        assert_eq!(q.cursor, 15);
        q.activate(POP_CANCEL);
        assert_eq!(q.draft.values[13], 0);
        q.open(13);
        q.key("PageDown", false);
        assert_eq!((q.scroll, q.cursor), (15, 15));
        q.key("Enter", false);
        assert_eq!(q.draft.values[13], 15);
        q.aircraft_names.clear();
        q.aircraft_files.clear();
        q.open(6);
        assert_eq!(q.activate(POP_OK), Action::None);
        assert!(q.player().is_none());
    }
    #[test]
    fn catalog_metadata_and_variant_aliases_do_not_create_imported_aircraft() {
        let options = setup().options;
        let data = ["F18C.PT", "RAFALEE.PT", "RAFALEF.PT", "OTHER.PT", "F18.PT"]
            .into_iter().map(|name| (name.to_string(), format!(
                "[brent's_relocatable_format]\n:ot_names\nstring \"Plane\"\nstring \"Planes\"\nstring \"{name}\"\nend\n"
            ).into_bytes())).collect();
        let q = QuickMission::new(AircraftId::F18, options, &data);
        assert!(q.aircraft_files.is_empty());
    }
    #[test]
    fn draft_cancel_and_pointer_release_are_transactional() {
        let mut q = setup();
        q.open(6);
        q.key("ArrowDown", false);
        assert_eq!(q.draft.values[6], 0);
        q.key("Escape", false);
        assert_eq!(q.draft.values[6], 0);
        q.open(6);
        q.key("ArrowDown", false);
        q.key("Enter", false);
        assert_eq!(q.player(), Some(AircraftId::Rafale));
        q.controls = vec![(4, (10, 10, 20, 20))];
        q.pointer(Some((15., 15.)));
        q.down();
        q.pointer(None);
        assert_eq!(q.up(), Action::None);
        assert_eq!(q.draft.values[4], 1);
    }
    #[test]
    fn source_dependencies_and_theater_identity_are_preserved() {
        let mut q = setup();
        q.apply(4, 0);
        assert_eq!(q.draft.values[4], 1);
        q.draft.values[31] = 3;
        q.draft.values[32] = 3;
        q.apply(30, 0);
        // The strengths stay without a target (B23, area defenses).
        assert_eq!(&q.draft.values[30..33], &[0, 3, 3]);
        for i in 0..16 {
            q.theater(i);
            assert_eq!(q.theater_index(), i);
        }
        q.apply(13, 3);
        assert_eq!(q.draft.values[20], 57);
        assert_eq!(q.draft.values[3], 0);
        q.apply(13, 9);
        assert_eq!(q.draft.values[20], 37);
    }
    #[test]
    fn all_wings_launch_with_the_selected_identities() {
        let mut q = setup();
        for field in [4, 7, 10, 21, 24, 27] {
            q.apply(field, 5);
        }
        q.apply(23, 1);
        assert!(q.unsupported().is_none());
        let wings = q.dummy_wings();
        assert_eq!(wings.iter().map(|(_, n)| n).sum::<usize>(), 29);
        assert_eq!(wings[0], (AircraftId::F18, 4));
        assert_eq!(wings[3], (AircraftId::Rafale, 5));
        q.apply(23, 2);
        assert!(q.unsupported().unwrap().contains("populated wing"));
    }
    #[test]
    fn wing_skill_selections_reach_the_launch_payload() {
        use tore_sim::ai::Experience;
        use tore_sim::ai::experience::ExperienceOrigin;
        let mut q = setup();
        // Every wing populated, with a different skill per wing row.
        for field in [4, 7, 10, 21, 24, 27] {
            q.apply(field, 5);
        }
        for (field, level) in [(5, 0), (8, 1), (11, 2), (22, 3), (25, 0), (28, 1)] {
            q.apply(field, level);
        }
        let wings = q.wing_launches(None).unwrap();
        assert_eq!(wings.len(), 6);
        let levels = [
            Experience::Novice,
            Experience::Average,
            Experience::Experienced,
            Experience::Ace,
            Experience::Novice,
            Experience::Average,
        ];
        for (wing, level) in wings.iter().zip(levels) {
            assert_eq!(wing.selected_level, level);
            assert!(!wing.is_empty());
            for member in &wing.members {
                assert_eq!(member.experience.level, level);
                assert_eq!(
                    member.experience.origin,
                    ExperienceOrigin::QuickMission { selected: level }
                );
            }
            assert!(wing.leader().is_some_and(|m| m.member == 0));
        }
        // Sides and the player's slot in friendly wing 1 are preserved.
        assert_eq!(wings[0].count(), 4);
        assert_eq!(wings[3].count(), 5);
        let sides: Vec<bool> = wings.iter().map(|w| w.wing.side.is_enemy()).collect();
        assert_eq!(sides, [false, false, false, true, true, true]);
        assert_eq!(
            wings.iter().map(|w| w.wing.index).collect::<Vec<_>>(),
            [0, 1, 2, 0, 1, 2]
        );
    }
    #[test]
    fn the_enemy_skill_override_changes_enemy_wings_only() {
        use tore_sim::ai::Experience;
        use tore_sim::ai::experience::ExperienceOrigin;
        let mut q = setup();
        for field in [4, 7, 10, 21, 24, 27] {
            q.apply(field, 5);
        }
        for field in [5, 8, 11, 22, 25, 28] {
            q.apply(field, 3);
        }
        let wings = q
            .wing_launches(Some(EnemySkillOverride::AllNovice))
            .unwrap();
        for wing in &wings {
            assert_eq!(wing.selected_level, Experience::Ace);
            let enemy = wing.wing.side.is_enemy();
            for member in &wing.members {
                if enemy {
                    assert_eq!(member.experience.level, Experience::Novice);
                    assert_eq!(member.experience.origin, ExperienceOrigin::EnemyOverride);
                } else {
                    assert_eq!(member.experience.level, Experience::Ace);
                    assert_eq!(
                        member.experience.origin,
                        ExperienceOrigin::QuickMission {
                            selected: Experience::Ace
                        }
                    );
                }
            }
        }
        // The legacy pairs are unchanged by the override.
        assert_eq!(legacy_pairs(&wings), q.dummy_wings());
    }
    #[test]
    fn separation_lists_eleven_nautical_choices_and_never_panics() {
        let mut q = setup();
        let labels = q.values(17).to_vec();
        assert_eq!(labels.len(), 11);
        assert_eq!(
            &labels[6..],
            [
                "75 miles",
                "100 miles",
                "150 miles",
                "200 miles",
                "300 miles"
            ]
        );
        // A theater and target offering every distance (the table's).
        let open = tore_world::mission_layout::distance_table::TARGETS
            .iter()
            .find(|row| row.airborne == tore_world::mission_layout::distance_table::ALL)
            .expect("a target offering every distance");
        let theater = q
            .theater_catalog
            .iter()
            .position(|c| c == open.theater)
            .unwrap();
        q.theater(theater);
        let target = ground_targets(open.theater)
            .iter()
            .position(|stem| *stem == open.stem)
            .unwrap();
        q.apply(30, target + 1);
        for (index, nm) in SEPARATION_NM.into_iter().enumerate() {
            q.apply(17, index);
            assert_eq!(q.separation_nm(), nm);
            assert_eq!(q.separation_feet(), nm * 6_076.12);
        }
        // Default is 5 miles; an index past the table falls back to it.
        assert_eq!(Draft::default().values[17], 2);
        q.draft.values[17] = 99;
        assert_eq!(q.separation_feet(), 5. * FEET_PER_NM);
        // Clicking cycles through the host entries and wraps.
        q.draft.values[17] = 5;
        for expected in [
            "75 miles",
            "100 miles",
            "150 miles",
            "200 miles",
            "300 miles",
        ] {
            q.activate(17);
            assert_eq!(q.value(17), expected);
        }
        q.activate(17);
        assert_eq!(q.draft.values[17], 0);
        for expected in [
            "300 miles",
            "200 miles",
            "150 miles",
            "100 miles",
            "75 miles",
        ] {
            right_click(&mut q, 17);
            assert_eq!(q.value(17), expected);
        }
    }

    #[test]
    fn placeholders_cannot_silently_launch_as_a_supported_mission() {
        let mut q = setup();
        assert!(q.unsupported().is_none());
        assert_eq!(q.dummy_wings().iter().map(|(_, n)| n).sum::<usize>(), 2);
        q.apply(21, 0);
        assert!(q.unsupported().is_none());
        q.apply(6, 2);
        assert!(q.unsupported().unwrap().contains("setup only"));
        q.apply(6, 0);
        // Every editor row maps to its intended source weather, including night.
        assert_eq!(
            &q.values(15)[..],
            ["dawn", "clear", "cloudy", "foggy", "sunset", "night"]
        );
        for (value, source) in [3, 0, 1, 2, 4, 5].into_iter().enumerate() {
            assert_eq!(condition(value), Some(source));
            q.apply(15, value);
            assert!(q.unsupported().is_none(), "condition {value}");
        }
        q.apply(15, 6);
        assert!(q.unsupported().unwrap().contains("six available"));
        q.apply(15, 1);
        // A ground target and its defenses are a mission the game flies.
        q.apply(30, 1);
        q.apply(31, 2);
        q.apply(32, 3);
        assert!(q.unsupported().is_none());
    }
    // ---- the lobby's mission read back, and read-only (lobby pass, L4) ----

    /// A creator as a lobby opens it: every aircraft offered.
    #[test]
    fn mission_brief_uses_the_creators_sentences_for_single_and_multiplayer_specs() {
        let font = crate::menu::flat_font([232, 233, 230]);
        let source = full_catalog();
        let mut seed = 59;
        let mut spec = made_spec(&source, &mut seed);
        spec.player_choice[0] = true;
        spec.ground_target = ground_targets(&spec.theater)
            .first()
            .map(|s| (*s).to_string());
        spec.aaa = Defense::Heavy;
        spec.sam = Defense::Moderate;
        for multiplayer in [false, true] {
            let mut settings =
                tore_session::settings::Store::defaults(tore_session::settings::Mode::Pvp);
            settings
                .apply(&[(tore_session::settings::number::AI, 0)])
                .unwrap();
            let mut creator = full_catalog();
            creator.load_spec(&spec, false).unwrap();
            creator.view = Some(ViewKind::Reader);
            creator.lobby = multiplayer;
            creator.draft.values[AI_AIRCRAFT] = 0;
            let expected = creator.sentences(&font);
            let actual = source
                .brief_sentences(&spec, multiplayer.then_some(&settings), &font)
                .unwrap();
            assert_eq!(actual, expected);
            let field = |id| {
                actual
                    .iter()
                    .flat_map(|s| &s.parts)
                    .find(|p| p.1 == Some(id))
                    .unwrap()
                    .0
                    .clone()
            };
            assert_eq!(field(31), "heavily");
            assert_eq!(field(32), "moderately");
            if multiplayer {
                assert_eq!(field(CHOICE_BASE), "Player choice");
                assert_eq!(field(AI_AIRCRAFT), "Off");
            }
        }
    }
    fn lobby_creator() -> QuickMission {
        let mut q = full_catalog();
        q.lobby = true;
        q
    }
    /// A small deterministic generator for sweeping specs.
    fn next(state: &mut u64) -> usize {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*state >> 33) as usize
    }
    /// A spec the creator could have made: any theater, weather, altitude,
    /// distance, orders, wing sizes, skills and aircraft.
    fn made_spec(q: &QuickMission, state: &mut u64) -> MissionSpec {
        let id = |files: &[String], at: usize| AircraftId::parse(&files[at % files.len()]).unwrap();
        let theater = q.theater_codes[next(state) % 16].clone();
        let mut spec = MissionSpec::new(&theater, id(&q.aircraft_files, next(state)));
        spec.condition = Condition::ALL[next(state) % 6];
        spec.start = Start::Airborne {
            altitude_ft: ALTITUDES_FT[next(state) % ALTITUDES_FT.len()],
        };
        spec.separation_nm = SEPARATION_NM[next(state) % SEPARATION_NM.len()] as u32;
        spec.preset = [
            crate::ai_wings::Preset::Free,
            crate::ai_wings::Preset::Cap,
            crate::ai_wings::Preset::SelfDefense,
            crate::ai_wings::Preset::Hold,
        ][next(state) % 4];
        spec.guns_only = next(state).is_multiple_of(2);
        for index in 0..6 {
            let aircraft = if index == 0 {
                spec.wings[0].aircraft
            } else {
                id(&q.wing_files, next(state))
            };
            spec.wings[index] = WingSpec {
                aircraft,
                count: (next(state) % 6).max(usize::from(index == 0)),
                skill: Skill::ALL[next(state) % 5],
            };
            // A strike needs a ground target, which these specs have none of
            // (the creator's own test of it is above).
            let choices: Vec<_> = QuickMission::objective_choices(index)
                .into_iter()
                .filter(|(_, objective)| *objective != GroupObjective::Strike)
                .collect();
            spec.objectives[index] = choices[next(state) % choices.len()].1;
            spec.must_survive[index] = next(state).is_multiple_of(2);
        }
        spec
    }
    #[test]
    fn a_mission_read_into_the_creator_comes_back_as_the_same_mission() {
        let mut q = lobby_creator();
        let mut state = 7;
        for case in 0..400 {
            let spec = made_spec(&q, &mut state);
            let draft = Draft::from_spec(&spec, &q).unwrap_or_else(|e| panic!("{case}: {e}"));
            q.load_spec(&spec, true).unwrap();
            assert_eq!(q.draft.values, draft.values, "case {case}");
            let back = q
                .lobby_spec()
                .unwrap_or_else(|e| panic!("case {case}: {e}"));
            assert_eq!(back, spec, "case {case}");
        }
    }
    #[test]
    fn every_value_of_every_choice_the_creator_offers_round_trips() {
        let mut q = lobby_creator();
        let mut state = 99;
        let mut spec = made_spec(&q, &mut state);
        for theater in q.theater_codes.clone() {
            spec.theater = theater;
            q.load_spec(&spec, true).unwrap();
            assert_eq!(q.lobby_spec().unwrap(), spec);
        }
        for condition in Condition::ALL {
            spec.condition = condition;
            q.load_spec(&spec, true).unwrap();
            assert_eq!(q.lobby_spec().unwrap(), spec);
        }
        for feet in ALTITUDES_FT {
            spec.start = Start::Airborne { altitude_ft: feet };
            q.load_spec(&spec, true).unwrap();
            assert_eq!(q.lobby_spec().unwrap(), spec);
        }
        for nm in SEPARATION_NM {
            spec.separation_nm = nm as u32;
            q.load_spec(&spec, true).unwrap();
            assert_eq!(q.lobby_spec().unwrap(), spec);
        }
        for wing in 0..6 {
            for skill in Skill::ALL {
                for count in 0..=5 {
                    spec.wings[wing].skill = skill;
                    spec.wings[wing].count = count.max(usize::from(wing == 0));
                    q.load_spec(&spec, true).unwrap();
                    assert_eq!(q.lobby_spec().unwrap(), spec);
                }
            }
            for (_, objective) in QuickMission::objective_choices(wing) {
                if objective == GroupObjective::Strike {
                    continue;
                }
                spec.objectives[wing] = objective;
                q.load_spec(&spec, true).unwrap();
                assert_eq!(q.lobby_spec().unwrap(), spec);
            }
        }
        for id in AircraftId::SELECTABLE {
            spec.wings[0].aircraft = id;
            q.load_spec(&spec, true).unwrap();
            assert_eq!(q.lobby_spec().unwrap(), spec);
            if id.ai_flyable() {
                spec.wings[3].aircraft = id;
                q.load_spec(&spec, true).unwrap();
                assert_eq!(q.lobby_spec().unwrap(), spec);
            }
        }
    }
    #[test]
    fn both_ground_starts_read_back_into_the_creator_and_the_view_shows_them() {
        let mut q = lobby_creator();
        set_fields(
            &mut q,
            0,
            &[
                (0, "Blue Field", false, Some(false)),
                (1, "Open Field", false, None),
                (2, "Red Field", false, Some(true)),
            ],
        );
        let code = q.theater_codes[0].clone();
        let mut spec = MissionSpec::new(&code, AircraftId::F18);
        spec.wings[3].count = 2;
        q.load_spec(&spec, true).unwrap();
        assert_eq!(q.lobby_spec().unwrap(), spec);
        assert!(!q.ground_start() && !q.enemy_ground_start());
        // A runway each: Blue's list is Blue Field, Open Field; Redfor's is
        // Open Field, Red Field.
        let mut ground = spec.clone();
        ground.start = Start::Ground {
            runway: 0x4000_0001,
            altitude_ft: 10_000,
        };
        ground.enemy_start = EnemyStart::Ground {
            runway: 0x4000_0002,
        };
        let draft = Draft::from_spec(&ground, &q).unwrap();
        assert_eq!(draft.values[33..=36], [1, 1, 1, 1]);
        q.load_spec(&ground, true).unwrap();
        assert_eq!(q.ground_runway(), Some(0x4000_0001));
        assert_eq!(q.enemy_ground_runway(), Some(0x4000_0002));
        assert_eq!(q.lobby_spec().unwrap(), ground);
        // With a target, the world's own picks are the first rows, and a
        // chosen runway follows them.
        let mut auto = ground.clone();
        auto.ground_target = Some(ground_targets(&code)[0].to_string());
        auto.start = Start::GroundAuto {
            altitude_ft: 10_000,
        };
        auto.enemy_start = EnemyStart::GroundAuto;
        assert_eq!(
            Draft::from_spec(&auto, &q).unwrap().values[33..=36],
            [1, 0, 1, 0]
        );
        q.load_spec(&auto, true).unwrap();
        assert!(q.ground_auto() && q.enemy_ground_auto());
        assert_eq!(q.lobby_spec().unwrap(), auto);
        auto.enemy_start = EnemyStart::Ground {
            runway: 0x4000_0001,
        };
        assert_eq!(Draft::from_spec(&auto, &q).unwrap().values[36], 1);
        // The read-only creator draws the King's choices.
        q.open_lobby_mission(&ground, Some(ViewKind::Reader))
            .unwrap();
        assert_eq!(
            [q.value(33), q.value(34), q.value(35), q.value(36)],
            ["Ground", "Open Field", "Ground", "Red Field"]
        );
        assert!(q.view_tint(35).is_none());
        // A runway the lists do not offer is refused by the draft and drawn
        // by the view; so is an automatic pick with no target to pick from.
        let mut odd = spec.clone();
        odd.enemy_start = EnemyStart::Ground {
            runway: 0x4000_0000,
        };
        let words = Draft::from_spec(&odd, &q).unwrap_err();
        assert!(words.contains("Blue Field"), "{words}");
        q.load_spec(&odd, false).unwrap();
        assert_eq!(q.value(36), "Blue Field");
        let mut lone = spec;
        lone.enemy_start = EnemyStart::GroundAuto;
        let words = Draft::from_spec(&lone, &q).unwrap_err();
        assert!(words.contains("automatic"), "{words}");
    }
    #[test]
    fn a_mission_the_lists_cannot_show_is_refused_by_the_draft_and_drawn_by_the_view() {
        let mut q = lobby_creator();
        let mut spec = MissionSpec::new("UKR", AircraftId::F18);
        spec.wings[3] = WingSpec {
            aircraft: AircraftId::F14,
            count: 1,
            skill: Skill::Ace,
        };
        // This game has no F-14.
        q.aircraft_files
            .retain(|key| key != AircraftId::F14.selection_key());
        q.aircraft_names
            .retain(|name| name != AircraftId::F14.label());
        q.refresh_wing_catalog();
        let words = Draft::from_spec(&spec, &q).unwrap_err();
        assert!(words.contains(AircraftId::F14.label()), "{words}");
        let before = q.draft.clone();
        assert!(q.load_spec(&spec, true).is_err());
        assert_eq!(q.draft.values, before.values, "nothing changed");
        // A theater the creator never offers cannot be shown at all.
        let mut odd = spec.clone();
        odd.theater = "NOWHERE".into();
        assert!(q.load_spec(&odd, false).is_err());
        // The view draws the aircraft it lacks, and counts past the list.
        spec.wings[4].count = 99;
        q.load_spec(&spec, false).unwrap();
        q.view = Some(ViewKind::Reader);
        assert_eq!(q.value(23), AircraftId::F14.label());
        assert_eq!(q.value(24), "99");
        assert!(q.view_tint(23).is_some() && q.view_tint(24).is_none());
        let mut wide = spec;
        wide.wings[4].count = 3;
        q.load_spec(&wide, false).unwrap();
        assert_eq!(q.value(24), "value", "a count the list holds is the list's");
        assert_eq!(q.draft.values[24], 3);
    }
    /// Opens the read-only creator on `spec`.
    fn viewer(spec: &MissionSpec) -> QuickMission {
        let mut q = lobby_creator();
        q.lobby = false;
        q.open_lobby_mission(spec, Some(ViewKind::Reader)).unwrap();
        q
    }
    #[test]
    fn the_read_only_creator_changes_nothing_and_says_why() {
        let mut state = 5;
        let spec = {
            let q = lobby_creator();
            made_spec(&q, &mut state)
        };
        let mut q = viewer(&spec);
        assert!(q.is_view() && q.lobby);
        assert!(q.notice.as_deref().is_some_and(|n| n.contains("View only")));
        let before = (q.draft.clone(), q.group_objectives, q.group_must_survive);
        let fields: Vec<usize> = (3..FIELDS)
            .chain([60])
            .chain(OBJECTIVE_BASE..SURVIVAL_BASE + OBJECTIVE_COUNT)
            .collect();
        for id in &fields {
            q.notice = None;
            q.hover = Some(*id);
            q.pressed = Some(*id);
            assert_eq!(q.up(), Action::Click, "left click on {id}");
            assert_eq!(q.notice.as_deref(), Some(VIEW_REFUSAL), "left {id}");
            q.notice = None;
            if (3..FIELDS).contains(id) || *id >= OBJECTIVE_BASE {
                q.hover = Some(*id);
                assert_eq!(right_click(&mut q, *id), Action::Click);
                assert_eq!(q.notice.as_deref(), Some(VIEW_REFUSAL), "right {id}");
            }
            assert!(q.selector.is_none(), "{id} opened a pop-up");
        }
        for key in ["Tab", "ArrowDown", "ArrowUp", " ", "Home", "x"] {
            q.key(key, false);
        }
        assert_eq!(
            (
                q.draft.clone().values,
                q.group_objectives,
                q.group_must_survive
            ),
            (before.0.values, before.1, before.2)
        );
        assert_eq!(q.lobby_spec().unwrap(), spec, "the mission is as it was");
    }
    #[test]
    fn the_read_only_creator_closes_with_back_enter_or_escape_and_has_no_cancel() {
        let spec = MissionSpec::new("UKR", AircraftId::F18);
        let mut q = viewer(&spec);
        q.hover = Some(OK);
        q.pressed = Some(OK);
        assert_eq!(q.up(), Action::Back, "the OK place reads Back");
        assert_eq!(q.activate(CANCEL), Action::Back);
        assert_eq!(q.key("Enter", false), Action::Back);
        assert_eq!(q.key("Escape", false), Action::Back);
        // Enter on a focused field closes it too; Space says it is read-only.
        q.focus = 7;
        assert_eq!(q.key("Enter", false), Action::Back);
        assert_eq!(q.key(" ", false), Action::Click);
        assert_eq!(q.notice.as_deref(), Some(VIEW_REFUSAL));
        // The help menu still works.
        assert_eq!(q.activate(0), Action::Click);
        assert!(q.help);
        assert_eq!(q.activate(61), Action::Exit);
    }
    #[test]
    fn the_kings_view_while_the_mission_flies_says_it_changes_only_in_the_lobby() {
        let spec = MissionSpec::new("UKR", AircraftId::F18);
        let mut q = lobby_creator();
        q.open_lobby_mission(&spec, Some(ViewKind::Flying)).unwrap();
        q.activate(17);
        assert_eq!(q.notice.as_deref(), Some(VIEW_FLYING));
    }
    #[test]
    fn the_kings_view_on_a_locked_server_says_the_missions_fixed() {
        let spec = MissionSpec::new("UKR", AircraftId::F18);
        let mut q = lobby_creator();
        q.open_lobby_mission(&spec, Some(ViewKind::Fixed)).unwrap();
        assert!(q.notice.as_deref().unwrap().ends_with(VIEW_FIXED));
        q.activate(17);
        assert_eq!(q.notice.as_deref(), Some(VIEW_FIXED));
        assert_eq!(q.activate(CANCEL), Action::Back);
    }
    #[test]
    fn what_the_mission_does_not_carry_reads_as_the_kings_and_a_new_mission_redraws() {
        let mut spec = MissionSpec::new("EGY", AircraftId::F18);
        let mut q = viewer(&spec);
        for field in NOT_CARRIED {
            assert_eq!(q.value(field), AS_THE_KINGS);
            assert!(q.view_tint(field).is_some());
        }
        assert!(
            q.view_tint(17).is_none(),
            "a carried field is drawn as it is"
        );
        assert_eq!(q.separation_nm(), 5.);
        spec.separation_nm = 100;
        spec.guns_only = true;
        q.activate(7);
        q.reload_view(&spec).unwrap();
        assert_eq!(q.separation_nm(), 100.);
        assert!(q.guns_only());
        assert_eq!(q.notice.as_deref(), Some(VIEW_CHANGED));
        assert!(q.is_view());
        // Leaving the lobby ends the view and what it kept.
        q.leave_lobby();
        assert!(!q.is_view() && q.shown.is_empty());
        assert_eq!(
            q.value(3),
            "value",
            "the nationalities read as the creator's own"
        );
    }
    #[test]
    fn what_this_game_lacks_is_dimmed_in_the_read_only_creator() {
        let spec = MissionSpec::new("UKR", AircraftId::F18);
        let mut q = viewer(&spec);
        let key = q.aircraft_files[q.draft.values[6]].clone();
        let theater = q.theater_codes[q.draft.values[13]].clone();
        q.set_gaps(|kind, k| {
            ((kind, k) == (ItemKind::Aircraft, key.as_str())
                || (kind, k) == (ItemKind::Theater, theater.as_str()))
                .then(|| "Not everyone has it.".to_owned())
        });
        assert!(q.view_tint(6).is_some(), "the aircraft");
        assert!(q.view_tint(13).is_some(), "the theater");
        assert!(q.view_tint(14).is_none());
    }
    #[test]
    fn the_kings_creator_opens_on_the_lobbys_mission_not_the_kings_last_draft() {
        let mut q = lobby_creator();
        // The King's last draft: something else entirely.
        q.apply(13, 5);
        q.apply(17, 6);
        q.apply(4, 3);
        let stale = q.lobby_spec().unwrap();
        let mut state = 21;
        let lobby = made_spec(&q, &mut state);
        assert_ne!(lobby, stale);
        let saved = q.save();
        q.open_lobby_mission(&lobby, None).unwrap();
        assert!(!q.is_view() && q.lobby);
        assert_eq!(q.lobby_spec().unwrap(), lobby);
        // A change of the draft is what Accept sends, laid over the lobby's.
        q.apply(17, 2);
        q.leave_lobby();
        q.restore(saved);
        assert_eq!(
            q.lobby_spec().unwrap(),
            stale,
            "the King's own draft is back"
        );
    }
    #[test]
    fn accept_lays_the_drafts_mission_over_the_lobbys_so_the_cheats_stay() {
        let mut lobby = MissionSpec::new("UKR", AircraftId::F18);
        lobby.cheats.unlimited_fuel = true;
        lobby.friendly_fire = false;
        let mut edited = lobby.clone();
        edited.cheats = Default::default();
        edited.friendly_fire = true;
        edited.separation_nm = 50;
        edited.guns_only = true;
        let sent = QuickMission::lay_over(&lobby, edited);
        assert!(sent.cheats.unlimited_fuel && !sent.friendly_fire);
        assert_eq!((sent.separation_nm, sent.guns_only), (50, true));
    }
    #[test]
    fn placement_reserves_one_seed_and_lobby_load_and_accept_keep_it() {
        let mut q = lobby_creator();
        q.apply(30, 1);
        q.surface_seed = 99;
        q.seed_pending = true;
        q.reserve_placement_seed();
        assert_eq!(q.surface_seed, 99);
        q.roll_surface_seed();
        assert_eq!(q.surface_seed, 99);
        q.reserve_placement_seed();
        assert_ne!(q.surface_seed, 99);
        let spec = q.mission_spec().unwrap();
        let seed = spec.surface_seed;
        q.load_spec(&spec, true).unwrap();
        assert_eq!(q.surface_seed, seed);
        assert!(q.seed_pending);
        assert_eq!(
            QuickMission::lay_over(&spec, q.lobby_spec().unwrap()).surface_seed,
            seed
        );
    }
}
