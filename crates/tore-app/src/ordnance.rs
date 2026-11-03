//! Retail art and recovered card/control geometry over a transactional loadout.
use crate::rocker::Rocker;
use crate::{
    AppResult,
    menu::{Action, Canvas, Sprite, text_width},
    quick_mission::fit,
};
use std::collections::BTreeMap;
use std::time::Instant;
use tore_formats::{Pic, weapons::Weapon};
use tore_sim::{
    combat::live::OwnshipInput,
    combat::loadout::{EditableStation, Loadout, novelty, supported},
    combat::sight::{DESIGNATOR_PODS, DesignatorStore},
    models::FlightModel,
};
type Rect = (i32, i32, i32, i32);
const CATALOG: Rect = (64, 103, 235, 272);
const CATALOG_AREA: usize = 15;
const CARD_WIDTH: i32 = 109;
const CARD_HEIGHT: i32 = 23;
const STATIONS_PER_PAGE: usize = 6;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CatalogEntry {
    Weapon(usize),
    Tank(usize),
    Designator(usize),
}
/// Shared weapon-name ordering, applied once when a catalog is loaded.
pub(crate) fn sort_weapons(weapons: &mut [Weapon]) {
    weapons.sort_by(|a, b| a.name.cmp(&b.name).then(a.source.cmp(&b.source)));
}

/// Shared display order for the pre-flight catalog and respawn station picker.
/// The weapon catalog is already name-sorted; tanks retain their imported order.
pub(crate) fn catalog_entries(
    weapons: &[Weapon],
    tanks: &[tore_sim::combat::live::TankStore],
    pods: &[DesignatorStore],
    category: usize,
) -> Vec<CatalogEntry> {
    let weapons = weapons
        .iter()
        .enumerate()
        .filter(|(_, w)| usize::from(w.flags & 0x10000 == 0) == category)
        .map(|(index, _)| CatalogEntry::Weapon(index));
    if category == 1 {
        (0..tanks.len())
            .map(CatalogEntry::Tank)
            .chain((0..pods.len()).map(CatalogEntry::Designator))
            .chain(weapons)
            .collect()
    } else {
        weapons.collect()
    }
}
#[derive(Clone, Copy)]
enum RetainedKind {
    Weapon,
    Tank,
}
#[derive(Clone, Copy)]
enum DragSource {
    Catalog(usize),
    Station(usize),
    TankCatalog(usize),
    TankStation(usize),
    DesignatorCatalog(usize),
    DesignatorStation(usize),
}
struct Drag {
    source: DragSource,
    origin: (f64, f64),
    moved: bool,
}
/// The loadout screen's display name for a weapon: the long store name, when
/// it fits the card in the screen's font. The simulation's weapon rows carry
/// the same names, so the mission is built with this too.
pub fn weapon_label<'a>(
    data: &'a BTreeMap<String, Vec<u8>>,
    font: &'a Sprite,
) -> impl Fn(&mut Weapon) + 'a {
    move |w: &mut Weapon| {
        if tore_formats::weapons::corrections::identity(&w.source).is_some() {
            return;
        }
        if let Some(bytes) = data.get(&w.source)
            && let Ok(brf) = tore_formats::aircraft::Brf::parse(bytes)
            && let Ok(names) = brf.strings("si_names")
            && let Some(long) = names.get(1)
            && text_width(font, long) <= 111
        {
            w.name = long.clone();
        }
    }
}
/// The font the loadout screen measures names in.
pub fn label_font() -> Sprite {
    crate::menu::flat_font([232, 233, 230])
}
/// Said on the lobby's page when Cheat loading is asked for.
pub const LOBBY_CHEAT_NOTICE: &str = "Cheat loading is not allowed in a multiplayer game.";
pub struct Ordnance {
    pub loadout: Loadout,
    pub visible: bool,
    /// Every imported weapon; the catalog applies flight support and Cheat rules.
    weapons: Vec<Weapon>,
    catalog: Vec<Weapon>,
    tanks: Vec<tore_sim::combat::live::TankStore>,
    tank_catalog: Vec<tore_sim::combat::live::TankStore>,
    /// The laser designator pod (Pave Spike) the import has, and the ones
    /// the catalog lists.
    designators: Vec<DesignatorStore>,
    designator_catalog: Vec<DesignatorStore>,
    sprites: BTreeMap<String, Sprite>,
    category: usize,
    pages: [usize; 2],
    selected: Option<CatalogEntry>,
    station: usize,
    station_page: usize,
    retained_kind: BTreeMap<usize, RetainedKind>,
    hover: Option<usize>,
    pressed: Option<usize>,
    right_pressed: Option<usize>,
    /// Page rocker, then fuel rocker.
    rockers: [Rocker; 2],
    pointer: Option<(f64, f64)>,
    drag: Option<Drag>,
    controls: Vec<(usize, Rect)>,
    pub message: Option<String>,
    menu: bool,
    /// The page is open from a multiplayer lobby (EF8): Fly reads Accept and
    /// sends the loadout for the player's own slot, Select Plane reads
    /// Cancel, and Cheat loading is not offered unless the King's `loadouts`
    /// setting allows it ([`Ordnance::lobby_cheat`]; the host refuses it
    /// otherwise).
    pub lobby: bool,
    /// The King's loadout rule allows Cheat loading on a lobby page
    /// (`loadouts any`, slice F2-L); set through [`Ordnance::set_cheat_rule`].
    lobby_cheat: bool,
    /// The weapons not every player has (stage L, slice L4), by record name
    /// (`AIM9X.JT`), each with the host's words for why it cannot be
    /// loaded. Used on a lobby page only; empty otherwise.
    gaps: BTreeMap<String, String>,
    /// The page's frame clock for the pod station's blink: when it opened.
    opened: Instant,
    /// A fixed clock in milliseconds, for previews and tests: the blink
    /// then holds one phase and the page asks for no redraws.
    frozen_ms: Option<u64>,
}
impl Ordnance {
    pub fn new(mut loadout: Loadout, data: &BTreeMap<String, Vec<u8>>) -> AppResult<Self> {
        let background = Pic::parse(
            data.get("ORD_AIR3.PIC")
                .ok_or("missing ordnance background; re-import media")?,
        )?;
        let palette: [[u8; 3]; 256] = background
            .palette
            .clone()
            .try_into()
            .map_err(|_| "ordnance palette missing")?;
        let mut sprites = BTreeMap::new();
        for (name, bytes) in data.iter().filter(|(n, _)| {
            n.ends_with(".PIC")
                && (n.starts_with('$')
                    || n.starts_with("ACTION0")
                    || n.starts_with("ACTDFT0")
                    || n.starts_with("ROCKER0")
                    || [
                        "ORD_AIR3.PIC",
                        "DIAL00.PIC",
                        "DIAL04.PIC",
                        "DIAL11.PIC",
                        "DIAL13.PIC",
                        "LIGHTON.PIC",
                        "LIGHTOFF.PIC",
                        "ACTDFLT.PIC",
                        "PANELFNT.PIC",
                        "ARMFONT.PIC",
                        "SMLFONT.PIC",
                        "MENUFONT.PIC",
                        "FONTACT.PIC",
                        "FNTWPNB.PIC",
                        "FNTWPNY.PIC",
                    ]
                    .contains(&n.as_str()))
        }) {
            let p = Pic::parse(bytes)?;
            sprites.insert(
                name.clone(),
                Sprite {
                    width: p.width,
                    height: p.height,
                    rgba: p.rgba(&palette),
                    glyphs: p.glyphs,
                },
            );
        }
        for (name, color) in [
            ("QUICKFONT", [232, 233, 230]),
            ("WPNBLUE", [42, 124, 217]),
            ("WPNYELLOW", [255, 211, 38]),
        ] {
            sprites.insert(name.into(), crate::menu::flat_font(color));
        }
        let mut weapons: Vec<_> = data
            .iter()
            .filter(|(n, _)| n.ends_with(".JT") && (!n.starts_with('~') || novelty(n)))
            .filter_map(|(name, b)| Weapon::parse(name, b).ok())
            .collect();
        let display = weapon_label(data, &sprites["QUICKFONT"]);
        for w in &mut weapons {
            display(w);
        }
        for station in &mut loadout.configuration.stations {
            display(&mut station.weapon);
        }
        drop(display);
        sort_weapons(&mut weapons);
        let tanks = data
            .iter()
            .filter(|(name, _)| name.ends_with(".GAS"))
            .map(|(name, bytes)| tore_sim::combat::live::TankStore::parse(name, bytes))
            .collect::<tore_formats::Result<Vec<_>>>()?;
        let designators = DESIGNATOR_PODS
            .iter()
            .filter_map(|name| DesignatorStore::parse(name, data.get(*name)?).ok())
            .collect::<Vec<_>>();
        let mut ordnance = Self {
            loadout,
            visible: true,
            weapons,
            catalog: vec![],
            tanks,
            tank_catalog: vec![],
            designators,
            designator_catalog: vec![],
            sprites,
            category: 0,
            pages: [0; 2],
            selected: None,
            station: 0,
            station_page: 0,
            retained_kind: BTreeMap::new(),
            hover: None,
            pressed: None,
            right_pressed: None,
            rockers: Default::default(),
            pointer: None,
            drag: None,
            controls: vec![],
            message: None,
            menu: false,
            lobby: false,
            lobby_cheat: false,
            gaps: BTreeMap::new(),
            opened: Instant::now(),
            frozen_ms: None,
        };
        ordnance.station = ordnance.display_order().first().copied().unwrap_or(0);
        ordnance.remember_occupants();
        ordnance.rebuild_catalog();
        Ok(ordnance)
    }
    /// Sets the King's loadout rule on a lobby page: `allowed` is `loadouts
    /// any`. Under `own` the page refuses Cheat loading, and a loading that
    /// is a Cheat one is dropped to unloaded stations with Cheat off (the
    /// host drops a kept cheat loadout to the standard load in the same
    /// way). Returns true when the rule took a Cheat loading away.
    pub fn set_cheat_rule(&mut self, allowed: bool) -> bool {
        self.lobby_cheat = allowed;
        if allowed || !self.loadout.cheat {
            return false;
        }
        self.remember_occupants();
        self.loadout.quantities.fill(0);
        let _ = self.loadout.clear_tanks();
        self.loadout.cheat = false;
        self.rebuild_catalog();
        self.message = Some(
            "The King turned Cheat loading off: your stations are unloaded. Choose again.".into(),
        );
        true
    }
    /// Takes the weapons not every player has (stage L, slice L4):
    /// `refusal` gives the host's words for a weapon record when it is in a
    /// gap now (`Client::gap_refusal`). The page asks it about every weapon
    /// it offers; the catalog dims those and loading one says why it cannot
    /// be. Only the lobby's page uses it.
    pub fn set_gaps(&mut self, refusal: impl Fn(&str) -> Option<String>) {
        // The laser designator pods are weapon items too (protocol 23).
        self.gaps = self
            .weapons
            .iter()
            .map(|weapon| weapon.source.clone())
            .chain(self.designators.iter().map(|pod| pod.source.clone()))
            .filter_map(|source| {
                let words = refusal(&source)?;
                Some((source, words))
            })
            .collect();
    }
    /// The host's words for a weapon the lobby's page cannot load, when it
    /// is in a gap.
    pub fn gap_words(&self, source: &str) -> Option<&str> {
        if !self.lobby {
            return None;
        }
        self.gaps.get(source).map(String::as_str)
    }
    /// A weapon in a gap leaves the loadout as it was and puts the host's
    /// words on the page. True when it was refused.
    fn refuse_gap(&mut self, source: &str) -> bool {
        let Some(words) = self.gap_words(source).map(str::to_owned) else {
            return false;
        };
        self.message = Some(words);
        true
    }
    /// Whether the lobby's page may use Cheat loading.
    pub fn cheat_allowed(&self) -> bool {
        !self.lobby || self.lobby_cheat
    }
    fn rebuild_catalog(&mut self) {
        let load = &self.loadout;
        self.catalog = self
            .weapons
            .iter()
            .filter(|w| supported(&w.source))
            // The novelty bombs show only under Cheat loading (decision 16).
            .filter(|w| !novelty(&w.source) || load.cheat)
            .filter(|w| w.source != "SUU16.JT" || !load.configuration.gun_pods.is_empty())
            .filter(|w| {
                load.cheat
                    || (0..load.hardpoints.len()).any(|i| (1..32767).contains(&load.capacity(i, w)))
            })
            .cloned()
            .collect();
        self.tank_catalog = self
            .tanks
            .iter()
            .filter(|tank| {
                (0..load.tank_hardpoints.len()).any(|slot| load.tank_capacity(slot, tank) > 0)
            })
            .cloned()
            .collect();
        // Laser designator pods go on pod stations (decision 7); Cheat loading
        // lists both. The lobby's page offers them too since protocol 23 (a
        // loadout row names the pod's SEE record; slice NA).
        self.designator_catalog = self
            .designators
            .iter()
            .filter(|pod| {
                load.cheat
                    || (0..load.hardpoints.len())
                        .any(|slot| load.designator_capacity(slot, pod) > 0)
            })
            .cloned()
            .collect();
        self.pages = [0; 2];
        self.selected = None;
    }
    fn remember_occupants(&mut self) {
        for row in self.loadout.editable_stations() {
            if self.loaded_tank(&row).is_some() {
                self.retained_kind.insert(row.hardpoint, RetainedKind::Tank);
            } else if self.loaded_weapon(&row).is_some() || self.loaded_designator(&row).is_some() {
                self.retained_kind
                    .insert(row.hardpoint, RetainedKind::Weapon);
            }
        }
    }
    fn empty_kind(&self, row: &EditableStation) -> RetainedKind {
        if let Some(selected) = self.selected {
            match selected {
                CatalogEntry::Tank(index)
                    if row.tank.is_some_and(|slot| {
                        self.loadout.tank_capacity(slot, &self.tank_catalog[index]) > 0
                    }) =>
                {
                    return RetainedKind::Tank;
                }
                CatalogEntry::Weapon(index)
                    if row.weapon.is_some_and(|slot| {
                        self.loadout.capacity(slot, &self.catalog[index]) > 0
                    }) =>
                {
                    return RetainedKind::Weapon;
                }
                // A pod hangs on the weapon row.
                CatalogEntry::Designator(index)
                    if row.weapon.is_some_and(|slot| {
                        self.loadout
                            .designator_capacity(slot, &self.designator_catalog[index])
                            > 0
                    }) =>
                {
                    return RetainedKind::Weapon;
                }
                _ => {}
            }
        }
        self.retained_kind
            .get(&row.hardpoint)
            .copied()
            .unwrap_or(if row.weapon.is_none() {
                RetainedKind::Tank
            } else {
                RetainedKind::Weapon
            })
    }
    fn station_row(&self, index: usize) -> Option<EditableStation> {
        self.loadout.editable_stations().into_iter().nth(index)
    }
    fn loaded_weapon(&self, row: &EditableStation) -> Option<usize> {
        row.weapon.filter(|slot| {
            self.loadout
                .quantities
                .get(*slot)
                .is_some_and(|quantity| *quantity > 0)
        })
    }
    /// The weapon slot of `row` when a laser designator pod hangs on it.
    fn loaded_designator(&self, row: &EditableStation) -> Option<usize> {
        row.weapon
            .filter(|slot| self.loadout.designator(*slot).is_some())
    }
    fn loaded_tank(&self, row: &EditableStation) -> Option<usize> {
        row.tank.filter(|slot| {
            self.loadout
                .configuration
                .tanks
                .get(*slot)
                .is_some_and(|tank| tank.quantity > 0)
        })
    }
    /// The stations in the order the page lists them (gun, centreline, then
    /// inboard to outboard); each entry is the station's source-order index,
    /// which is what clicks, the wire and saved loads keep using.
    fn display_order(&self) -> Vec<usize> {
        self.loadout.station_display_order()
    }
    /// The page that lists station `index`.
    fn page_of(&self, index: usize) -> usize {
        self.display_order()
            .iter()
            .position(|&station| station == index)
            .map_or(0, |at| at / STATIONS_PER_PAGE)
    }
    fn reveal_station(&mut self) {
        self.station_page = self.page_of(self.station);
    }
    /// Milliseconds on the page's frame clock.
    fn clock_ms(&self) -> u64 {
        self.frozen_ms
            .unwrap_or_else(|| self.opened.elapsed().as_millis() as u64)
    }
    fn cannot_load(&mut self) -> Action {
        self.message = Some("This store cannot be loaded at this station.".into());
        Action::None
    }
    fn change_station(&mut self, index: usize, direction: i32, one: bool) -> Action {
        let Some(row) = self.station_row(index) else {
            return Action::None;
        };
        if let Some(slot) = self.loaded_tank(&row) {
            return self.edit_loadout(|load| load.change_tank(slot, direction));
        }
        // One pod to an aircraft: minus takes it off, plus has nothing to add.
        if let Some(slot) = self.loaded_designator(&row) {
            return if direction < 0 {
                self.edit_loadout(|load| load.unload_designator(slot))
            } else {
                Action::None
            };
        }
        let weapon = self.loaded_weapon(&row).or_else(|| {
            matches!(self.empty_kind(&row), RetainedKind::Weapon)
                .then_some(row.weapon)
                .flatten()
        });
        if let Some(slot) = weapon {
            return self.edit_loadout(|load| {
                if one {
                    let capacity = load.capacity(slot, &load.configuration.stations[slot].weapon);
                    // One store, or one pair on a paired station.
                    let step = load.store_pairs(slot).step(1);
                    if direction > 0 && i32::from(load.quantities[slot]) + step <= capacity {
                        load.quantities[slot] += step as u16;
                    }
                    load.refresh_equipment()
                } else {
                    load.change(slot, direction);
                    Ok(())
                }
            });
        }
        if matches!(self.empty_kind(&row), RetainedKind::Tank)
            && let Some(slot) = row.tank
        {
            return self.edit_loadout(|load| load.change_tank(slot, direction));
        }
        Action::None
    }
    fn load_selected(&mut self, index: usize) -> Action {
        let Some(row) = self.station_row(index) else {
            return Action::None;
        };
        let Some(selected) = self.selected else {
            return Action::None;
        };
        match selected {
            CatalogEntry::Tank(index) => {
                let Some(slot) = row.tank else {
                    return self.cannot_load();
                };
                let tank = self.tank_catalog[index].clone();
                self.edit_loadout(|load| load.select_tank(slot, tank))
            }
            CatalogEntry::Weapon(index) => {
                let Some(slot) = row.weapon else {
                    return self.cannot_load();
                };
                let weapon = self.catalog[index].clone();
                if self.refuse_gap(&weapon.source) {
                    return Action::None;
                }
                self.edit_loadout(|load| load.select(slot, weapon))
            }
            CatalogEntry::Designator(index) => {
                let Some(slot) = row.weapon else {
                    return self.cannot_load();
                };
                let pod = self.designator_catalog[index].clone();
                if self.refuse_gap(&pod.source) {
                    return Action::None;
                }
                self.edit_loadout(|load| load.load_designator(slot, pod))
            }
        }
    }

    fn drop_store(&mut self, source: DragSource, target: usize) -> Action {
        let Some(row) = self.station_row(target) else {
            return Action::None;
        };
        match source {
            DragSource::Catalog(index) => {
                let Some(slot) = row.weapon else {
                    return self.cannot_load();
                };
                self.selected = Some(CatalogEntry::Weapon(index));
                let weapon = self.catalog[index].clone();
                if self.refuse_gap(&weapon.source) {
                    return Action::None;
                }
                self.edit_loadout(|load| load.select(slot, weapon))
            }
            DragSource::TankCatalog(index) => {
                let Some(slot) = row.tank else {
                    return self.cannot_load();
                };
                self.selected = Some(CatalogEntry::Tank(index));
                let tank = self.tank_catalog[index].clone();
                self.edit_loadout(|load| load.select_tank(slot, tank))
            }
            DragSource::DesignatorCatalog(index) => {
                let Some(slot) = row.weapon else {
                    return self.cannot_load();
                };
                self.selected = Some(CatalogEntry::Designator(index));
                let pod = self.designator_catalog[index].clone();
                if self.refuse_gap(&pod.source) {
                    return Action::None;
                }
                self.edit_loadout(|load| load.load_designator(slot, pod))
            }
            DragSource::DesignatorStation(index) => {
                let Some(pod) = self
                    .station_row(index)
                    .and_then(|row| row.weapon)
                    .and_then(|slot| self.loadout.designator(slot).cloned())
                else {
                    return Action::None;
                };
                let Some(target) = row.weapon else {
                    return self.cannot_load();
                };
                self.selected = None;
                // The aircraft's one pod moves to the new station.
                self.edit_loadout(|load| load.load_designator(target, pod))
            }
            DragSource::Station(index) => {
                let Some(source) = self.station_row(index).and_then(|row| row.weapon) else {
                    return Action::None;
                };
                let Some(target) = row.weapon else {
                    return self.cannot_load();
                };
                self.selected = None;
                self.edit_loadout(|load| load.transfer(source, target))
            }
            DragSource::TankStation(index) => {
                let Some(source) = self.station_row(index).and_then(|row| row.tank) else {
                    return Action::None;
                };
                let Some(target) = row.tank else {
                    return self.cannot_load();
                };
                self.selected = None;
                self.edit_loadout(|load| load.transfer_tank(source, target))
            }
        }
    }
    fn drag_thumbnail(&self, source: DragSource) -> Option<&Sprite> {
        match source {
            DragSource::Catalog(index) => thumbnail(&self.sprites, &self.catalog[index]),
            DragSource::TankCatalog(index) => {
                tank_thumbnail(&self.sprites, &self.tank_catalog[index])
            }
            DragSource::DesignatorCatalog(index) => {
                designator_thumbnail(&self.sprites, &self.designator_catalog[index])
            }
            DragSource::DesignatorStation(index) => self
                .station_row(index)
                .and_then(|row| row.weapon)
                .and_then(|slot| self.loadout.designator(slot))
                .and_then(|pod| designator_thumbnail(&self.sprites, pod)),
            DragSource::Station(index) => self
                .station_row(index)
                .and_then(|row| row.weapon)
                .and_then(|slot| {
                    thumbnail(
                        &self.sprites,
                        &self.loadout.configuration.stations[slot].weapon,
                    )
                }),
            DragSource::TankStation(index) => self
                .station_row(index)
                .and_then(|row| row.tank)
                .and_then(|slot| self.loadout.configuration.tanks[slot].store.as_ref())
                .and_then(|tank| tank_thumbnail(&self.sprites, tank)),
        }
    }
    fn page_entries(&self) -> Vec<CatalogEntry> {
        catalog_entries(
            &self.catalog,
            &self.tank_catalog,
            &self.designator_catalog,
            self.category,
        )
    }

    pub fn pointer(&mut self, p: Option<(f64, f64)>) {
        self.pointer = p;
        if let (Some(drag), Some(p)) = (&mut self.drag, p) {
            drag.moved |= (p.0 - drag.origin.0).powi(2) + (p.1 - drag.origin.1).powi(2) >= 9.;
        }
        if p.is_none() {
            self.drag = None;
            self.pressed = None;
            self.right_pressed = None;
        }
        self.hover = p.and_then(|p| {
            self.controls
                .iter()
                .rev()
                .find(|(_, r)| {
                    p.0 >= r.0 as f64
                        && p.0 < (r.0 + r.2) as f64
                        && p.1 >= r.1 as f64
                        && p.1 < (r.1 + r.3) as f64
                })
                .map(|(i, _)| *i)
        });
    }
    pub fn dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| drag.moved)
    }
    /// The page (1, 2) and fuel (5, 6) rockers act on press and tilt while
    /// held, as retail rockers do.
    pub fn down(&mut self) -> Action {
        self.right_pressed = None;
        self.pressed = self.hover;
        if let Some(id @ (1 | 2 | 5 | 6)) = self.hover.filter(|_| !self.menu) {
            return self.rock(id, true);
        }
        let source = match self.hover {
            Some(id @ 100..=107) => self.card_index(id).map(|entry| match entry {
                CatalogEntry::Weapon(index) => DragSource::Catalog(index),
                CatalogEntry::Tank(index) => DragSource::TankCatalog(index),
                CatalogEntry::Designator(index) => DragSource::DesignatorCatalog(index),
            }),
            Some(id @ 200..=231) => self.station_row(id - 200).and_then(|row| {
                if self.loaded_tank(&row).is_some() {
                    Some(DragSource::TankStation(id - 200))
                } else if self.loaded_designator(&row).is_some() {
                    Some(DragSource::DesignatorStation(id - 200))
                } else {
                    self.loaded_weapon(&row)
                        .map(|_| DragSource::Station(id - 200))
                }
            }),
            _ => None,
        };
        self.drag = source.zip(self.pointer).map(|(source, origin)| Drag {
            source,
            origin,
            moved: false,
        });
        Action::None
    }
    /// Tilts rocker control `id` and applies it. Page turns sound the rocker;
    /// fuel keeps its own cue.
    fn rock(&mut self, id: usize, held: bool) -> Action {
        self.rockers[usize::from(id > 2)].push(matches!(id, 2 | 6), held, Instant::now());
        match self.activate(id) {
            Action::Click => Action::RockerDown,
            action => action,
        }
    }
    pub fn cancel(&mut self) {
        self.pressed = None;
        self.right_pressed = None;
        for rocker in &mut self.rockers {
            if rocker.held() {
                rocker.release(Instant::now());
            }
        }
        self.hover = None;
        self.pointer = None;
        self.drag = None;
        self.menu = false;
    }
    pub fn up(&mut self) -> Action {
        let p = self.pressed.take();
        if let Some(id @ (1 | 2 | 5 | 6)) = p {
            let rocker = &mut self.rockers[usize::from(id > 2)];
            if rocker.held() {
                rocker.release(Instant::now());
                return Action::RockerUp;
            }
        }
        if let Some(drag) = self.drag.take().filter(|drag| drag.moved) {
            self.message = None;
            if let Some(target) = self.hover.filter(|id| (200..232).contains(id)) {
                self.station = target - 200;
                let station = self.station;
                return self.drop_store(drag.source, station);
            }
            if self
                .hover
                .is_some_and(|id| id == CATALOG_AREA || (100..108).contains(&id))
            {
                match drag.source {
                    DragSource::Station(source) => {
                        let Some(slot) = self.station_row(source).and_then(|row| row.weapon) else {
                            return Action::None;
                        };
                        self.selected = None;
                        self.station = source;
                        return self.edit_loadout(|load| {
                            load.quantities[slot] = 0;
                            load.refresh_equipment()
                        });
                    }
                    DragSource::TankStation(source) => {
                        let Some(slot) = self.station_row(source).and_then(|row| row.tank) else {
                            return Action::None;
                        };
                        self.selected = None;
                        self.station = source;
                        return self.edit_loadout(|load| load.unload_tank(slot));
                    }
                    DragSource::DesignatorStation(source) => {
                        let Some(slot) = self.station_row(source).and_then(|row| row.weapon) else {
                            return Action::None;
                        };
                        self.selected = None;
                        self.station = source;
                        return self.edit_loadout(|load| load.unload_designator(slot));
                    }
                    _ => {}
                }
            }
            return Action::None;
        }
        if let (Some(source), Some(target)) = (p, self.hover) {
            if (100..108).contains(&source) && (200..232).contains(&target) {
                self.select_card(source);
                return self.activate(target);
            }
            if source == target {
                return self.activate(target);
            }
        }
        Action::None
    }
    fn select_card(&mut self, id: usize) {
        self.selected = self.card_index(id);
    }
    fn card_index(&self, id: usize) -> Option<CatalogEntry> {
        self.page_entries()
            .get(self.pages[self.category] * 8 + id - 100)
            .copied()
    }
    pub fn right(&mut self, down: bool) -> Action {
        if down {
            self.pressed = None;
            self.drag = None;
            self.right_pressed = self.hover;
            return Action::None;
        }
        let pressed = self.right_pressed.take();
        if let Some(id) = self
            .hover
            .filter(|i| Some(*i) == pressed && (200..232).contains(i))
        {
            self.station = id - 200;
            self.change_station(self.station, -1, false)
        } else {
            Action::None
        }
    }
    /// Sound follows a completed edit, independent of the input gesture.
    fn edit_loadout(
        &mut self,
        edit: impl FnOnce(&mut Loadout) -> tore_formats::Result<()>,
    ) -> Action {
        self.remember_occupants();
        let before: Vec<_> = self
            .loadout
            .configuration
            .stations
            .iter()
            .zip(&self.loadout.quantities)
            .map(|(station, count)| (station.weapon.source.clone(), station.weapon.flags, *count))
            .collect();
        let tank_before: Vec<_> = self
            .loadout
            .configuration
            .tanks
            .iter()
            .map(|s| (s.store.as_ref().map(|t| t.source.clone()), s.quantity))
            .collect();
        let pods_before = self.loadout.configuration.designator_pods.clone();
        let rollback = self.loadout.clone();
        let fuel = self.loadout.fuel_lbs;
        self.message = None;
        if let Err(error) = edit(&mut self.loadout) {
            self.loadout = rollback;
            self.message = Some(error.to_string());
            return Action::None;
        }
        self.remember_occupants();
        let tank_after: Vec<_> = self
            .loadout
            .configuration
            .tanks
            .iter()
            .map(|s| (s.store.as_ref().map(|t| t.source.clone()), s.quantity))
            .collect();
        let weapon_added = self
            .loadout
            .configuration
            .stations
            .iter()
            .zip(&self.loadout.quantities)
            .zip(&before)
            .any(|((station, count), (source, _, old_count))| {
                *count > *old_count || (*count > 0 && station.weapon.source != *source)
            });
        // A pod hung or taken off sounds as a weapon does.
        if pods_before != self.loadout.configuration.designator_pods && !weapon_added {
            return Action::OrdnanceWeapon;
        }
        if tank_before != tank_after && !weapon_added {
            return Action::OrdnanceFuel;
        }
        for ((station, count), (source, old_flags, old_count)) in self
            .loadout
            .configuration
            .stations
            .iter()
            .zip(&self.loadout.quantities)
            .zip(before)
        {
            if *count != old_count || station.weapon.source != source {
                let flags = if *count == 0 {
                    old_flags
                } else {
                    station.weapon.flags
                };
                return if flags & 0x80 != 0 {
                    Action::OrdnanceAmmunition
                } else {
                    Action::OrdnanceWeapon
                };
            }
        }
        let tank_after: Vec<_> = self
            .loadout
            .configuration
            .tanks
            .iter()
            .map(|s| (s.store.as_ref().map(|t| t.source.clone()), s.quantity))
            .collect();
        if tank_before != tank_after {
            return Action::OrdnanceFuel;
        }
        if self.loadout.fuel_lbs != fuel {
            Action::OrdnanceFuel
        } else {
            Action::None
        }
    }
    fn activate(&mut self, id: usize) -> Action {
        self.message = None;
        match id {
            1 => self.pages[self.category] = self.pages[self.category].saturating_sub(1),
            2 => {
                self.pages[self.category] = (self.pages[self.category] + 1)
                    .min(self.page_entries().len().saturating_sub(1) / 8)
            }
            3 | 4 => {
                self.category = id - 3;
                self.selected = None;
            }
            16 => {
                // Compatibility for old headless snapshot fixtures, no visible button.
                self.category = 1;
                self.selected = None;
            }
            5 | 6 => {
                return self.edit_loadout(|load| {
                    load.fuel(id == 5);
                    Ok(())
                });
            }
            7 => {
                if let Err(e) = self.loadout.validate() {
                    self.message = Some(e.to_string());
                } else {
                    return Action::MissionFly;
                }
            }
            8 => {
                self.visible = false;
                self.cancel();
            }
            9 | 10 => {
                return self.change_station(self.station, if id == 9 { 1 } else { -1 }, false);
            }
            17 | 18 => {
                let pages = self
                    .loadout
                    .editable_stations()
                    .len()
                    .div_ceil(STATIONS_PER_PAGE)
                    .max(1);
                self.station_page = if id == 17 {
                    self.station_page.saturating_sub(1)
                } else {
                    (self.station_page + 1).min(pages - 1)
                };
                self.station = self
                    .display_order()
                    .get(self.station_page * STATIONS_PER_PAGE)
                    .copied()
                    .unwrap_or(0);
                self.selected = None;
            }
            11 => self.menu = !self.menu,
            12 => {
                self.remember_occupants();
                self.loadout.quantities.fill(0);
                self.loadout.configuration.designator_pods.clear();
                let _ = self.loadout.clear_tanks();
                self.menu = false;
            }
            13 if !self.cheat_allowed() => {
                self.menu = false;
                self.message = Some(LOBBY_CHEAT_NOTICE.into());
            }
            13 => {
                // Toggling unloads every station, then rebuilds the catalog.
                self.remember_occupants();
                self.loadout.quantities.fill(0);
                self.loadout.configuration.designator_pods.clear();
                let _ = self.loadout.clear_tanks();
                self.loadout.cheat = !self.loadout.cheat;
                self.rebuild_catalog();
                self.menu = false;
                self.message = Some(
                    if self.loadout.cheat {
                        "Cheat loading on."
                    } else {
                        "Cheat loading off."
                    }
                    .into(),
                );
            }
            14 => {
                self.message =
                    Some("Airbase aircraft cycling is not available in this setup.".into())
            }
            100..=107 => {
                self.select_card(id);
                let source = match self.selected {
                    Some(CatalogEntry::Weapon(index)) => Some(self.catalog[index].source.clone()),
                    Some(CatalogEntry::Designator(index)) => {
                        Some(self.designator_catalog[index].source.clone())
                    }
                    _ => None,
                };
                if let Some(source) = source {
                    self.refuse_gap(&source);
                }
            }
            200..=231 => {
                self.station = id - 200;
                let Some(row) = self.station_row(self.station) else {
                    return Action::None;
                };
                if self.loaded_weapon(&row).is_some()
                    || self.loaded_tank(&row).is_some()
                    || self.loaded_designator(&row).is_some()
                {
                    self.selected = None;
                    return self.change_station(self.station, 1, true);
                }
                return self.load_selected(self.station);
            }
            _ => return Action::None,
        }
        Action::Click
    }
    pub fn key(&mut self, key: &str) -> Action {
        if self.drag.is_some() {
            if key == "Escape" {
                self.cancel();
            }
            return Action::None;
        }
        match key {
            "Escape" => {
                if self.menu {
                    self.menu = false;
                } else {
                    self.visible = false;
                    self.cancel();
                }
                Action::None
            }
            "Enter" => self.activate(7),
            "+" | "=" => self.activate(9),
            "-" => self.activate(10),
            "ArrowRight" => self.rock(2, false),
            "ArrowLeft" => self.rock(1, false),
            "a" | "A" => self.activate(3),
            "s" | "S" => self.activate(4),
            "Tab" => {
                let order = self.display_order();
                if let Some(at) = order.iter().position(|&index| index == self.station) {
                    self.station = order[(at + 1) % order.len()];
                    self.reveal_station();
                } else if let Some(&first) = order.first() {
                    self.station = first;
                    self.reveal_station();
                }
                self.selected = None;
                Action::None
            }
            _ => Action::None,
        }
    }
    /// Reproducible presentation fixtures for the existing menu snapshot command.
    pub fn preview(&mut self, state: &str) {
        match state {
            "ordnance-empty" => {
                self.loadout.quantities.fill(0);
                let _ = self.loadout.clear_tanks();
            }
            "ordnance-tanks" => {
                self.activate(16);
            }
            "ordnance-drag" => {
                self.pointer(Some((100., 120.)));
                self.down();
                self.pointer(Some((325., 190.)));
            }
            "ordnance-message" => {
                self.activate(13);
            }
            "ordnance-laser" | "lobby-ordnance-laser" => {
                // A laser bomb on the first station that takes one, on the
                // air-to-surface page (the laser warning and sensor helper).
                self.category = 1;
                if let Some(weapon) = self
                    .catalog
                    .iter()
                    .find(|w| tore_sim::combat::laser::is_laser_weapon(w))
                    .cloned()
                {
                    for slot in 0..self.loadout.quantities.len() {
                        if self.loadout.select(slot, weapon.clone()).is_ok() {
                            break;
                        }
                    }
                }
            }
            "ordnance-pod" | "ordnance-pod-yellow" => {
                // The laser pod on the first station that takes it, with the
                // blink held on its red or its yellow phase (a fixed clock).
                self.category = 1;
                if let Some(pod) = self.designator_catalog.first().cloned() {
                    for slot in 0..self.loadout.quantities.len() {
                        if self.loadout.load_designator(slot, pod.clone()).is_ok() {
                            break;
                        }
                    }
                }
                self.frozen_ms = Some(if state == "ordnance-pod" {
                    0
                } else {
                    POD_BLINK_MS
                });
            }
            "ordnance-gun-pod" => {
                // The SUU-16 gun pod on the first station that takes it (the
                // F-4B's centreline, in place of its Sidewinders), on the
                // first page, where the catalog lists a gun.
                self.category = 0;
                if let Some(pod) = self
                    .catalog
                    .iter()
                    .find(|w| w.source == "SUU16.JT")
                    .cloned()
                {
                    for slot in 0..self.loadout.quantities.len() {
                        if self.loadout.select(slot, pod.clone()).is_ok() {
                            break;
                        }
                    }
                    // The catalog page that lists the pod.
                    let at = self.page_entries().iter().position(|entry| {
                        matches!(entry, CatalogEntry::Weapon(i) if self.catalog[*i].source == "SUU16.JT")
                    });
                    if let Some(at) = at {
                        self.pages[0] = at / 8;
                    }
                }
            }
            "ordnance-authored-tank" => {
                // The first authored tank (an F-16C's 370 gallon one, say) on
                // the first tank station that takes it, on the air-to-surface
                // page, where the catalog lists tanks first.
                self.category = 1;
                if let Some(tank) = self
                    .tank_catalog
                    .iter()
                    .find(|tank| {
                        tore_formats::aircraft::stations::TANKS
                            .iter()
                            .any(|(name, ..)| *name == tank.source)
                    })
                    .cloned()
                {
                    for slot in 0..self.loadout.configuration.tanks.len() {
                        if self.loadout.select_tank(slot, tank.clone()).is_ok()
                            && self.loadout.configuration.tanks[slot].quantity > 0
                        {
                            break;
                        }
                    }
                }
            }
            "ordnance-tip-amraam" => {
                // An AIM-120 on the wingtip pair (the F-16C's), on the
                // air-to-air page, with that station selected.
                self.category = 0;
                if let Some(weapon) = self
                    .catalog
                    .iter()
                    .find(|w| w.source == "AIM120.JT")
                    .cloned()
                {
                    let rows = self.loadout.editable_stations();
                    if let Some((index, slot)) = rows.iter().enumerate().find_map(|(i, r)| {
                        r.weapon.filter(|_| r.location == 5).map(|slot| (i, slot))
                    }) && self.loadout.select(slot, weapon).is_ok()
                    {
                        self.station = index;
                        self.reveal_station();
                    }
                }
            }
            "ordnance-wing-amraam" => {
                // An AIM-120 on the Eagle's wing pylons (the appended row 7,
                // a tank or a missile a side), on the air-to-air page, with
                // that station selected.
                self.category = 0;
                if let Some(weapon) = self
                    .catalog
                    .iter()
                    .find(|w| w.source == "AIM120.JT")
                    .cloned()
                {
                    let rows = self.loadout.editable_stations();
                    if let Some((index, slot)) = rows.iter().enumerate().find_map(|(i, r)| {
                        r.weapon.filter(|_| r.hardpoint == 7).map(|slot| (i, slot))
                    }) && self.loadout.select(slot, weapon).is_ok()
                    {
                        self.station = index;
                        self.reveal_station();
                    }
                }
            }
            "lobby-ordnance-gaps" => {
                // The catalog's first and third weapons are in a gap, and
                // the first is the one the player just tried.
                let names: Vec<String> = self.catalog.iter().map(|w| w.name.clone()).collect();
                let sources: Vec<String> = self.catalog.iter().map(|w| w.source.clone()).collect();
                for index in [0, 2] {
                    if let (Some(source), Some(name)) = (sources.get(index), names.get(index)) {
                        self.gaps.insert(
                            source.clone(),
                            format!("Not everyone has the {name}: Hawk's game has no {name}."),
                        );
                    }
                }
                if let Some(words) = sources.first().and_then(|s| self.gap_words(s)) {
                    self.message = Some(words.to_owned());
                }
            }
            "ordnance-message-long" => {
                self.message = Some(
                    "This is a long ordnance message preview. It stays on one line, with a background sized to the displayed text and an ellipsis when the message reaches the available screen width."
                        .into(),
                );
            }
            _ => {}
        }
    }
    /// Draws the screen; true while a rocker is still moving.
    pub fn render(&mut self, pixels: &mut [u8]) -> bool {
        let now = Instant::now();
        let animating = self
            .rockers
            .iter_mut()
            .fold(false, |moving, rocker| rocker.advance(now) | moving);
        pixels.copy_from_slice(&self.sprites["ORD_AIR3.PIC"].rgba);
        self.controls.clear();
        self.controls.push((CATALOG_AREA, CATALOG));
        let mut c = Canvas(pixels);
        let font = &self.sprites["QUICKFONT"];
        let menu_font = &self.sprites["MENUFONT.PIC"];
        for (label, x) in [("?", 87), ("Weapons", 103), ("Airbase", 178)] {
            c.centered_text(menu_font, label, (x, 38, text_width(menu_font, label), 20));
        }
        self.controls
            .extend([(11, (103, 35, 75, 24)), (14, (178, 35, 90, 24))]);
        let title = self.loadout.aircraft.label();
        c.text(
            &self.sprites["ARMFONT.PIC"],
            title,
            466 - text_width(&self.sprites["ARMFONT.PIC"], title) / 2,
            101,
            None,
        );
        let entries = self.page_entries();
        let page = self.pages[self.category];
        for (row, index) in entries.iter().skip(page * 8).take(8).enumerate() {
            // Imported black wells are 113 pixels wide at x=66/185.
            let x = 68 + (row % 2) as i32 * 119;
            let y = 108 + (row / 2) as i32 * 68;
            self.controls.push((100 + row, (x - 4, y - 5, 115, 68)));
            match *index {
                CatalogEntry::Tank(index) => tank_card(
                    &mut c,
                    &self.sprites,
                    &self.tank_catalog[index],
                    x,
                    y,
                    self.selected == Some(CatalogEntry::Tank(index)),
                    true,
                ),
                CatalogEntry::Designator(index) => {
                    designator_card(
                        &mut c,
                        &self.sprites,
                        &self.designator_catalog[index],
                        (x, y),
                        self.selected == Some(CatalogEntry::Designator(index)),
                        if self.selected == Some(CatalogEntry::Designator(index)) {
                            OUTLINE_YELLOW
                        } else {
                            OUTLINE_RED
                        },
                        true,
                    );
                    // What not every player has is dimmed on the lobby's page.
                    if self
                        .gap_words(&self.designator_catalog[index].source)
                        .is_some()
                    {
                        dim(&mut c, (x - 3, y - 5, 113, 66));
                    }
                }
                CatalogEntry::Weapon(index) => {
                    card(
                        &mut c,
                        &self.sprites,
                        &self.catalog[index],
                        x,
                        y,
                        self.selected == Some(CatalogEntry::Weapon(index)),
                        true,
                    );
                    // What not every player has is dimmed on the lobby's page.
                    if self.gap_words(&self.catalog[index].source).is_some() {
                        dim(&mut c, (x - 3, y - 5, 113, 66));
                    }
                }
            }
        }

        let stations = self.loadout.editable_stations();
        let station_pages = stations.len().div_ceil(STATIONS_PER_PAGE).max(1);
        self.station_page = self.station_page.min(station_pages - 1);
        if station_pages > 1 {
            c.centered_text(
                font,
                &format!("Stations {} of {}", self.station_page + 1, station_pages),
                (350, 38, 147, 20),
            );
            for (id, label, x) in [(17, "Prev", 501), (18, "Next", 551)] {
                let hit = c.action_button(
                    &self.sprites,
                    label,
                    (x, 35, 47),
                    false,
                    self.pressed == Some(id),
                );
                self.controls.push((id, hit));
            }
        }
        let order = self.display_order();
        let blink_yellow = pod_blink_yellow(self.clock_ms());
        for (visible, (index, row)) in order
            .iter()
            .map(|&index| (index, &stations[index]))
            .skip(self.station_page * STATIONS_PER_PAGE)
            .take(STATIONS_PER_PAGE)
            .enumerate()
        {
            let x = 350 + (visible % 2) as i32 * 119;
            let y = 121 + (visible / 2) as i32 * 71;
            self.controls.push((200 + index, (x - 1, y - 6, 115, 71)));
            let location = [
                "Centerline",
                "Fuselage",
                "Internal Gun",
                "Internal Bay",
                "Wing",
                "Wingtip",
            ]
            .get(usize::from(row.location))
            .copied()
            .unwrap_or("Station");
            c.text(font, &fit(font, location, 111), x + 1, y, None);
            if let Some(slot) = self.loaded_tank(row) {
                let tank = &self.loadout.configuration.tanks[slot];
                if let Some(store) = &tank.store {
                    tank_card(
                        &mut c,
                        &self.sprites,
                        store,
                        x + 3,
                        y + 14,
                        self.station == index,
                        false,
                    );
                    c.text(
                        font,
                        &format!(
                            "{} loaded (max {})",
                            tank.quantity,
                            self.loadout.tank_capacity(slot, store)
                        ),
                        x + 3,
                        y + 52,
                        None,
                    );
                }
            } else if let Some(pod) = self
                .loaded_designator(row)
                .and_then(|slot| self.loadout.designator(slot))
            {
                designator_card(
                    &mut c,
                    &self.sprites,
                    pod,
                    (x + 3, y + 14),
                    self.station == index,
                    // A pod on a station blinks its box between red and yellow.
                    if blink_yellow {
                        OUTLINE_YELLOW
                    } else {
                        OUTLINE_RED
                    },
                    false,
                );
                c.text(font, "1 loaded (max 1)", x + 3, y + 52, None);
            } else if let Some(slot) = self.loaded_weapon(row) {
                let station = &self.loadout.configuration.stations[slot];
                let quantity = self.loadout.quantities[slot];
                card(
                    &mut c,
                    &self.sprites,
                    &station.weapon,
                    x + 3,
                    y + 14,
                    self.station == index,
                    false,
                );
                let amount = if station.internal {
                    format!(
                        "{quantity} (max {})",
                        self.loadout.capacity(slot, &station.weapon)
                    )
                } else {
                    format!(
                        "{quantity} loaded (max {})",
                        self.loadout.capacity(slot, &station.weapon)
                    )
                };
                c.text(font, &amount, x + 3, y + 52, None);
            } else {
                card_outline(&mut c, x + 3, y + 14, false);
            }
        }
        let total = self.loadout.total_lbs();
        for (y, value) in [
            (351, self.loadout.maximum_lbs),
            (365, total),
            (381, self.loadout.maximum_lbs - total),
        ] {
            let text = format!("{} lbs", grouped(value));
            c.centered_text(
                font,
                &text,
                (
                    467 - text_width(font, &text),
                    y,
                    text_width(font, &text),
                    12,
                ),
            );
        }
        c.centered_text(
            font,
            &format!("{} lbs", grouped(self.loadout.fuel_lbs)),
            (487, 354, 56, 14),
        );
        // The original background already supplies the percent sign.
        c.centered_text(
            font,
            &format!(
                "{:.0}",
                100. * self.loadout.fuel_lbs / self.loadout.internal_capacity_lbs
            ),
            (487, 375, 27, 14),
        );
        c.centered_text(
            font,
            &format!("{} of {}", page + 1, entries.len().div_ceil(8).max(1)),
            (248, 388, 48, 15),
        );
        for (rocker, at) in self.rockers.iter().zip([(260, 408), (547, 356)]) {
            let sprite = &self.sprites[rocker.sprite()];
            c.blit(sprite, at, 0, sprite.width, 1.);
        }
        self.controls.extend([
            (1, (260, 408, 18, 17)),
            (2, (260, 425, 18, 17)),
            (5, (547, 356, 18, 17)),
            (6, (547, 373, 18, 17)),
            (3, (68, 390, 160, 30)),
            (4, (68, 420, 160, 31)),
        ]);
        let dial = &self.sprites[if self.category == 0 {
            "DIAL13.PIC"
        } else {
            "DIAL11.PIC"
        }];
        c.blit(dial, (148, 393), 0, dial.width, 1.);
        for (category, y) in [(0, 394), (1, 422)] {
            let lamp = &self.sprites[if self.category == category {
                "LIGHTON.PIC"
            } else {
                "LIGHTOFF.PIC"
            }];
            c.blit(lamp, (115, y), 0, lamp.width, 1.);
        }
        for (id, label, r) in [
            (
                7,
                if self.lobby { "Accept" } else { "Fly" },
                (493, 414, 80, 24),
            ),
            (
                8,
                if self.lobby { "Cancel" } else { "Select Plane" },
                (363, 414, 100, 24),
            ),
        ] {
            let hit = c.action_button(
                &self.sprites,
                label,
                (r.0, r.1, r.2),
                id == 7,
                self.pressed == Some(id),
            );
            self.controls.push((id, hit));
        }
        if self.menu {
            c.rect((103, 60, 230, 50), [210, 214, 211, 255]);
            c.text(&self.sprites["MENUFONT.PIC"], "Unload All", 108, 64, None);
            c.text(
                &self.sprites["MENUFONT.PIC"],
                if self.loadout.cheat {
                    "Cheat  On"
                } else if !self.cheat_allowed() {
                    "Cheat  Off (not allowed)"
                } else {
                    "Cheat  Off"
                },
                108,
                89,
                None,
            );
            self.controls = vec![
                (11, (103, 35, 75, 24)),
                (12, (103, 60, 230, 25)),
                (13, (103, 85, 230, 25)),
            ];
        }
        // The page's words, one band by priority: what the player just did
        // (the message), then the laser warning. The sensor helper line has
        // its own place below the Select Plane and Fly buttons, so it shows
        // with either (John, 2026-10-16; it used to share the lobby's strip
        // under the title bar and gave way there).
        let warning = laser_warning(&self.loadout);
        let identity = match self.selected {
            Some(CatalogEntry::Weapon(index)) => {
                tore_formats::weapons::corrections::identity(&self.catalog[index].source)
                    .map(|(_, long)| long)
            }
            _ => None,
        };
        if let Some(message) = &self.message {
            notice(&mut c, font, message, self.lobby);
        } else if let Some(words) = warning {
            band(&mut c, font, words, self.lobby, WARNING_INK);
        } else if let Some(name) = identity {
            notice(&mut c, font, name, self.lobby);
        }
        if let Some(words) = sensor_helper(&self.loadout, !self.designator_catalog.is_empty()) {
            helper_band(&mut c, font, &words);
        }
        if let Some(drag) = self.drag.as_ref().filter(|drag| drag.moved)
            && let Some((x, y)) = self.pointer
            && let Some(sprite) = self.drag_thumbnail(drag.source)
        {
            c.blit(
                sprite,
                (
                    x as i32 - sprite.width as i32 / 2,
                    y as i32 - sprite.height as i32 / 2,
                ),
                0,
                sprite.width,
                1.,
            );
        }
        // A pod on a station blinks on the live clock, so the page asks for
        // frames while one hangs; a fixed clock (previews, tests) does not.
        animating
            || (self.frozen_ms.is_none() && !self.loadout.configuration.designator_pods.is_empty())
    }
}
/// Where the page's notice sits: single player's low on the left, over the
/// bottom of the catalog; the lobby's in the strip of backdrop between the
/// title bar and the two panels, which covers no label (EF-F; the low place
/// hid the bottom weapon rows' names).
const NOTICE_Y: i32 = 335;
const LOBBY_NOTICE_Y: i32 = 64;

fn notice(c: &mut Canvas, font: &Sprite, message: &str, lobby: bool) {
    band(c, font, message, lobby, [235, 225, 179]);
}
/// The laser warning's ink: a dim amber, quieter than a message's cream
/// (opinionated, John 2026-10-12 asked for a subtle warning; the colour is
/// the lead's default).
const WARNING_INK: [u8; 3] = [196, 138, 52];
/// The sensor helper line's ink: a light blue-grey, brighter than the first
/// quiet [150, 170, 180] so it stands out (John, 2026-10-16: "a little
/// brighter"; the value is an agent decision).
const HELPER_INK: [u8; 3] = [208, 228, 238];
/// The sensor helper line sits below the right panel's Select Plane and Fly
/// buttons, on the backdrop under the panel's frame (John, 2026-10-16): its
/// top edge, the centre it is drawn about (the panel's) and the widest it
/// may be, which stays clear of the left panel and the screen edge.
const HELPER_Y: i32 = 458;
const HELPER_CENTER_X: i32 = 466;
const HELPER_MAX_WIDTH: i32 = 308;
/// The leftmost the strip may start (right of the left panel's frame) and the
/// right edge it keeps clear of.
const HELPER_LEFT: i32 = 322;
const HELPER_RIGHT: i32 = 632;
/// One line of the sensor helper on its dark strip, centred under the right
/// panel.
fn helper_band(c: &mut Canvas, font: &Sprite, message: &str) {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = fit(font, &line, HELPER_MAX_WIDTH - 8);
    if line.is_empty() {
        return;
    }
    let width = text_width(font, &line) + 8;
    let x =
        (HELPER_CENTER_X - width / 2).clamp(HELPER_LEFT, (HELPER_RIGHT - width).max(HELPER_LEFT));
    c.rect(
        (x, HELPER_Y, width, font.height as i32 + 4),
        [35, 44, 46, 255],
    );
    c.text(font, &line, x + 4, HELPER_Y + 2, Some(HELPER_INK));
}
/// One line of the page's words on its dark strip, in `ink`: low on the
/// left in single player, or in the lobby's strip under the title bar
/// (`high`).
fn band(c: &mut Canvas, font: &Sprite, message: &str, high: bool, ink: [u8; 3]) {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let line = fit(font, &line, 572);
    if line.is_empty() {
        return;
    }
    let y = if high { LOBBY_NOTICE_Y } else { NOTICE_Y };
    c.rect(
        (30, y, text_width(font, &line) + 8, font.height as i32 + 4),
        [35, 44, 46, 255],
    );
    c.text(font, &line, 34, y + 2, Some(ink));
}
/// Said when a laser weapon is loaded and nothing on the aircraft can lase
/// (slice BQ, B24; wording the lead's default). The player may still fly: a
/// flight mate's laser spot guides it (buddy lasing, docs/spec/air-to-ground.md).
pub const NO_LASER_WARNING: &str = "No laser designator: needs buddy lase";
/// The warning when laser ordnance (a loaded laser bomb or laser missile,
/// signature 1 with the designator flag) has no designator on the aircraft:
/// no built-in laser, and no pod on this station or another.
pub fn laser_warning(loadout: &Loadout) -> Option<&'static str> {
    let laser_loaded = loadout
        .configuration
        .stations
        .iter()
        .zip(&loadout.quantities)
        .any(|(station, quantity)| {
            *quantity > 0 && tore_sim::combat::laser::is_laser_weapon(&station.weapon)
        });
    let can_lase = loadout
        .configuration
        .sight_sensor()
        .is_some_and(|sensor| sensor.laser);
    (laser_loaded && !can_lase).then_some(NO_LASER_WARNING)
}
/// One line on what the aircraft's own sensors do for its weapons, from its
/// data (slice BQ, B24; wording the lead's default): a built-in or fitted
/// laser designator or FLIR, or, with neither, that a Pave pod can be hung
/// (`pods`: the page offers one). `None` when there is nothing to say.
pub fn sensor_helper(loadout: &Loadout, pods: bool) -> Option<String> {
    use tore_sim::combat::sight::Gimbal;
    let Some(sensor) = loadout.configuration.sight else {
        return pods.then(|| "No built-in designator: hang a Pave Spike pod to lase.".to_string());
    };
    let (what, needs) = if sensor.laser {
        ("laser designator", "laser ordnance")
    } else {
        ("FLIR", "IR weapons")
    };
    Some(if sensor.gimbal == Gimbal::Hemisphere {
        // Shortened on John's request, 2026-10-16.
        format!(
            "Aircraft {} pod equipped",
            if sensor.laser { "laser" } else { "FLIR" }
        )
    } else {
        format!("Built-in {what}: no pod needed for {needs}.")
    })
}
/// Darkens a block of the picture: a weapon the lobby's page cannot load.
fn dim(c: &mut Canvas, (x, y, w, h): Rect) {
    let (width, height) = (crate::menu::WIDTH as i32, crate::menu::HEIGHT as i32);
    for yy in y.max(0)..(y + h).min(height) {
        for xx in x.max(0)..(x + w).min(width) {
            let at = (yy as usize * crate::menu::WIDTH + xx as usize) * 4;
            for channel in &mut c.0[at..at + 3] {
                *channel = (u16::from(*channel) * 2 / 5) as u8;
            }
        }
    }
}
fn grouped(value: f64) -> String {
    let digits = format!("{value:.0}");
    let mut result = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0
            && ch.is_ascii_digit()
            && (digits.len() - i).is_multiple_of(3)
            && digits.as_bytes()[i - 1].is_ascii_digit()
        {
            result.push(',');
        }
        result.push(ch);
    }
    result
}
fn card(
    c: &mut Canvas,
    sprites: &BTreeMap<String, Sprite>,
    w: &Weapon,
    x: i32,
    y: i32,
    selected: bool,
    catalog: bool,
) {
    card_outline(c, x, y, selected);
    if let Some(p) = thumbnail(sprites, w) {
        c.blit(
            p,
            (
                x + (CARD_WIDTH - p.width as i32) / 2,
                y - 1 + (CARD_HEIGHT - p.height as i32) / 2,
            ),
            0,
            p.width,
            1.,
        );
    }
    let font = &sprites[if selected { "WPNYELLOW" } else { "QUICKFONT" }];
    let mut name = if tore_formats::weapons::corrections::identity(&w.source).is_some() {
        w.name.split(" (").next().unwrap_or(&w.name).to_owned()
    } else {
        w.name.clone()
    };
    while text_width(font, &name) > 111 && !name.is_empty() {
        name.pop();
    }
    c.text(font, &name, x, y + 25, None);
    if catalog {
        let font = &sprites["WPNBLUE"];
        c.text(
            font,
            &format!("{:.0} lbs", w.carried_weight_lb()),
            x,
            y + 37,
            None,
        );
        let guidance = guidance(w);
        c.text(
            font,
            guidance,
            x + 111 - text_width(font, guidance),
            y + 37,
            None,
        );
    }
}
/// The seeker word on a catalog card, from the retail list ("passive rdr,
/// active rdr, IR, laser, optical, unguided, SARH"): unguided records carry no
/// guided flag; a guided record is named for its seeker signature, and
/// signature 0 is "optical" (the JDAMs and the AT-2).
fn guidance(w: &Weapon) -> &'static str {
    if w.flags & 1 == 0 {
        return "unguided";
    }
    use tore_formats::weapons::SeekerKind;
    match w.seeker_kind() {
        SeekerKind::Infrared => "IR",
        SeekerKind::SemiActiveRadar => "SARH",
        SeekerKind::ActiveRadar => "active rdr",
        SeekerKind::PassiveRadar => "passive rdr",
        SeekerKind::Other => match w.seeker.signature {
            0 => "optical",
            1 => "laser",
            _ => "",
        },
    }
}
/// A laser designator pod's card: its name, its weight and "laser" in the
/// seeker corner (A2G decision 7).
fn designator_card(
    c: &mut Canvas,
    sprites: &BTreeMap<String, Sprite>,
    pod: &DesignatorStore,
    (x, y): (i32, i32),
    selected: bool,
    edge: [u8; 4],
    catalog: bool,
) {
    outline(c, x, y, edge);
    if let Some(icon) = designator_thumbnail(sprites, pod) {
        c.blit(
            icon,
            (
                x + (CARD_WIDTH - icon.width as i32) / 2,
                y - 1 + (CARD_HEIGHT - icon.height as i32) / 2,
            ),
            0,
            icon.width,
            1.,
        );
    }
    let font = &sprites[if selected { "WPNYELLOW" } else { "QUICKFONT" }];
    let mut name = pod.name.clone();
    while text_width(font, &name) > 111 && !name.is_empty() {
        name.pop();
    }
    c.text(font, &name, x, y + 25, None);
    if catalog {
        let font = &sprites["WPNBLUE"];
        c.text(font, &format!("{} lbs", pod.weight_lbs), x, y + 37, None);
        c.text(
            font,
            "laser",
            x + 111 - text_width(font, "laser"),
            y + 37,
            None,
        );
    }
}
fn designator_thumbnail<'a>(
    sprites: &'a BTreeMap<String, Sprite>,
    pod: &DesignatorStore,
) -> Option<&'a Sprite> {
    sprites.get(&format!("${}.PIC", pod.source.trim_end_matches(".SEE")))
}
fn tank_card(
    c: &mut Canvas,
    sprites: &BTreeMap<String, Sprite>,
    tank: &tore_sim::combat::live::TankStore,
    x: i32,
    y: i32,
    selected: bool,
    catalog: bool,
) {
    card_outline(c, x, y, selected);
    if let Some(icon) = tank_thumbnail(sprites, tank) {
        c.blit(
            icon,
            (
                x + (CARD_WIDTH - icon.width as i32) / 2,
                y - 1 + (CARD_HEIGHT - icon.height as i32) / 2,
            ),
            0,
            icon.width,
            1.,
        );
    }
    let font = &sprites[if selected { "WPNYELLOW" } else { "QUICKFONT" }];
    let mut name = tank.name.clone();
    while text_width(font, &name) > 111 && !name.is_empty() {
        name.pop();
    }
    c.text(font, &name, x, y + 25, None);
    if catalog {
        c.text(
            &sprites["WPNBLUE"],
            &format!("{:.0} lbs", tank.full_mass_lbs()),
            x,
            y + 37,
            None,
        );
    }
}
fn tank_thumbnail<'a>(
    sprites: &'a BTreeMap<String, Sprite>,
    tank: &tore_sim::combat::live::TankStore,
) -> Option<&'a Sprite> {
    let stem = tank.source.trim_end_matches(".GAS");
    sprites
        .get(&format!("${stem}.PIC"))
        .or_else(|| nearest_tank_picture(sprites, stem))
}
/// The picture of the retail tank closest in size, for a tank with no
/// picture of its own: the authored tanks (`F370`, `F610`, `F600`, `M396`,
/// `R528`; `aircraft_stations`) have no retail art, and the page draws them
/// with the retail tank graphic like the other fuel tanks (John, 2026-10-16;
/// the nearest-size rule is an agent decision). The stem is the record name
/// without `.GAS`, whose digits are its gallons; ties go to the larger tank.
fn nearest_tank_picture<'a>(
    sprites: &'a BTreeMap<String, Sprite>,
    stem: &str,
) -> Option<&'a Sprite> {
    let gallons = |stem: &str| -> Option<i32> {
        let digits = stem.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
            .then(|| digits.parse().ok())
            .flatten()
    };
    let wanted = gallons(stem)?;
    sprites
        .iter()
        .filter_map(|(name, sprite)| {
            // A retail tank picture is `$F` and its gallons, `$F350.PIC`:
            // not the `$FAB500.PIC` bomb or another store's.
            let size = name
                .strip_prefix("$F")?
                .strip_suffix(".PIC")
                .filter(|size| size.chars().all(|c| c.is_ascii_digit()))?
                .parse::<i32>()
                .ok()?;
            Some((size, sprite))
        })
        .min_by_key(|(size, _)| ((size - wanted).abs(), -size))
        .map(|(_, sprite)| sprite)
}
fn thumbnail<'a>(sprites: &'a BTreeMap<String, Sprite>, weapon: &Weapon) -> Option<&'a Sprite> {
    let source =
        tore_formats::weapons::corrections::donor(&weapon.source).unwrap_or(&weapon.source);
    sprites.get(&format!("${}.PIC", source.trim_end_matches(".JT")))
}
const OUTLINE_YELLOW: [u8; 4] = [201, 179, 49, 255];
const OUTLINE_RED: [u8; 4] = [94, 30, 20, 255];
/// How long each colour of a pod station's blink shows: two full red and
/// yellow cycles a second (opinionated, John 2026-10-16).
pub const POD_BLINK_MS: u64 = 250;
/// Whether the pod station's outline is yellow (else red) `ms` milliseconds
/// into the page's frame clock: red first, each colour [`POD_BLINK_MS`].
pub fn pod_blink_yellow(ms: u64) -> bool {
    (ms / POD_BLINK_MS) % 2 == 1
}
fn card_outline(c: &mut Canvas, x: i32, y: i32, selected: bool) {
    outline(
        c,
        x,
        y,
        if selected {
            OUTLINE_YELLOW
        } else {
            OUTLINE_RED
        },
    );
}
fn outline(c: &mut Canvas, x: i32, y: i32, edge: [u8; 4]) {
    c.rect((x, y - 1, CARD_WIDTH, 1), edge);
    c.rect((x, y + CARD_HEIGHT - 2, CARD_WIDTH, 1), edge);
    c.rect((x, y - 1, 1, CARD_HEIGHT), edge);
    c.rect((x + CARD_WIDTH - 1, y - 1, 1, CARD_HEIGHT), edge);
}

/// Source-cache regression probe. No GPU, native execution, or invented targets.
/// Headless source-backed regression for explicit tank selections and restart.
/// It covers the reviewed F-14 tank contract only: `requested` is the aircraft
/// named with `--aircraft`, if any, and any other aircraft is refused rather
/// than silently probing the F-14. `--validate-ordnance` covers every aircraft.
pub fn validate_tanks(
    data: &BTreeMap<String, Vec<u8>>,
    requested: Option<tore_formats::aircraft::AircraftId>,
) -> AppResult<()> {
    use tore_world::mission::LoadoutSpec;
    if let Some(id) = requested
        && id != tore_formats::aircraft::AircraftId::F14
    {
        return Err(format!(
            "--validate-tanks probes the F-14 tank contract only, not --aircraft {}; use --validate-ordnance for every aircraft's tanks",
            id.selection_key()
        )
        .into());
    }
    let profile = tore_formats::aircraft::Aircraft::parse(
        data.get("F14.PT")
            .ok_or("tank probe missing F14.PT; re-import media")?,
    )?;
    let standard = Loadout::new(&profile, |name| {
        data.get(name)
            .cloned()
            .ok_or_else(|| std::io::Error::other(format!("tank probe missing {name}")))
    })?;
    let slot = standard
        .configuration
        .tanks
        .iter()
        .position(|tank| tank.hardpoint == 5)
        .ok_or("tank probe missing F14 source tank station")?;
    let store = standard.configuration.tanks[slot]
        .store
        .clone()
        .ok_or("tank probe missing source tank type")?;
    if store.source != "F250.GAS"
        || standard.configuration.tanks[slot].quantity != 2
        || standard.fuel_lbs != 15741.
        || standard.external_fuel_lbs() != 3300.
        || standard.tank_shell_lbs() != 396.
    {
        return Err("F14 tank probe source differs from the reviewed 2 x F250 contract".into());
    }
    for quantity in [0, 2] {
        let mut draft = standard.clone();
        draft.clear_tanks()?;
        if quantity > 0 {
            draft.select_tank(slot, store.clone())?;
        }
        draft.validate()?;
        let accepted = LoadoutSpec::of(&draft);
        for _ in 0..2 {
            let restarted = accepted.apply(standard.clone(), data, None)?;
            restarted.validate()?;
            if restarted.fuel_lbs != 15741.
                || restarted.external_fuel_lbs() != f64::from(quantity) * 1650.
                || restarted.tank_shell_lbs() != f64::from(quantity) * 198.
                || restarted.total_lbs()
                    != standard.total_lbs() - 3696. + f64::from(quantity) * 1848.
            {
                return Err(format!(
                    "F14 tank selection {quantity} changed during accepted-load restart"
                )
                .into());
            }
        }
        println!(
            "F14 tank selection {quantity}: internal {:.0} external {:.0} shells {:.0} total {:.0}; restart passed",
            draft.fuel_lbs,
            draft.external_fuel_lbs(),
            draft.tank_shell_lbs(),
            draft.total_lbs()
        );
    }
    let mut edited = standard.clone();
    edited.change_tank(slot, -1)?;
    if edited.configuration.tanks[slot].quantity != 0 {
        return Err("F14 tank minus must remove the pair".into());
    }
    edited.change_tank(slot, 1)?;
    if edited.configuration.tanks[slot].quantity != 2 {
        return Err("F14 tank plus must restore the pair".into());
    }
    let mut single = LoadoutSpec::of(&standard);
    single
        .tanks
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|tank| tank.hardpoint == 5)
        .unwrap()
        .quantity = 1;
    let migrated = single.apply(standard.clone(), data, None)?;
    migrated.validate()?;
    if migrated.configuration.tanks[slot].quantity != 2
        || migrated.external_fuel_lbs() != 3300.
        || migrated.tank_shell_lbs() != 396.
    {
        return Err("F14 saved single tank did not become the full pair".into());
    }
    println!(
        "F14 saved single tank: 1 to 2, external 3300 shells 396; pair editing and migration passed"
    );
    let mut fuel = tore_sim::aircraft_systems::Fuel::new(standard.configuration.external_fuel_lbs);
    let mut internal = standard.fuel_lbs;
    fuel.consume(&mut internal, 3300.);
    if fuel.external_lbs() != 0.
        || internal != 15741.
        || f64::from(standard.configuration.external_equipment_lbs) - fuel.used_lbs()
            != f64::from(standard.configuration.fixed_external_equipment_lbs) + 396.
    {
        return Err("F14 dry tanks lost shell mass or consumed internal fuel prematurely".into());
    }
    let mut old = LoadoutSpec::of(&standard);
    old.tanks = None;
    if old.apply(standard, data, None)?.external_fuel_lbs() != 3300. {
        return Err("older mission lost its source-default tank selection".into());
    }
    println!(
        "F14 tanks: full selection, removal, empty shells, old defaults and accepted restart passed"
    );
    Ok(())
}

pub fn validate_sources(
    data: &BTreeMap<String, Vec<u8>>,
    world: &crate::terrain::Terrain,
    scenery: &crate::scenery::Scenery,
) -> AppResult<()> {
    // Run editor/loadout checks for the complete roster before unrelated flight
    // appearance probes, so their failures cannot hide preflight coverage.
    for id in tore_formats::aircraft::AircraftId::SELECTABLE {
        let airframe = crate::aircraft::Airframe::load(data, id)?;
        let load = Loadout::new(&airframe.profile, |name| {
            data.get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
        })?;
        let mut combat = crate::combat::Combat::with_loadout(&airframe, &load)?;
        let mut view = crate::combat_view::CombatView::new(&combat, combat.own_id(), data)?;
        view.mission_dummies(&mut combat, &[(id, 29)], 5280., data)?;
        combat.reset(&mut airframe.start(world))?;
        validate_guns_only(&load, &airframe, &combat.state.targets, data, world)?;
        validate_dragging(&load, data)?;
        validate_removed_stores(&load, &airframe, world)?;
        println!(
            "{}: guns-only across all six wings, player/wing restart, standard loads, ordnance dragging and removed stores passed",
            id.label()
        );
    }
    for id in tore_formats::aircraft::AircraftId::SELECTABLE {
        let airframe = crate::aircraft::Airframe::load(data, id)?;
        let mut load = Loadout::new(&airframe.profile, |n| {
            data.get(n)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing {n}")))
        })?;
        load.validate()?;
        let mut normal = crate::combat::Combat::new(&airframe, data, false)?;
        let mut normal_view = crate::combat_view::CombatView::new(&normal, normal.own_id(), data)?;
        let mut flight = airframe.start(world);
        let mtow = flight.model().configuration().mass.max_takeoff_lbs;
        let wind_limits =
            tore_sim::runway_wind::limits(mtow).ok_or("invalid imported MTOW wind class")?;
        println!(
            "{id:?} runway wind: MTOW={mtow:.0} lb noticeable={} rough={} limit={} kt tailwind=10 kt",
            wind_limits.noticeable_knots, wind_limits.rough_knots, wind_limits.limit_knots
        );

        normal.reset(&mut flight)?;
        if normal.state.own().ammo != load.ammunition()?
            || normal.range
            || !normal.state.targets.is_empty()
        {
            return Err("normal start lost default weapons or spawned a range target".into());
        }
        normal.state.own_mut().ammo.fill(0);
        normal.reset(&mut flight)?;
        if normal.state.own().ammo != load.ammunition()? {
            return Err("normal restart lost weapons".into());
        }
        normal.clean_recording = true;
        normal.reset(&mut flight)?;
        if flight.payload_lbs != 0.
            || normal
                .state
                .own()
                .configuration()
                .stations
                .iter()
                .zip(&normal.state.own().ammo)
                .any(|(station, count)| !station.internal && *count != 0)
        {
            return Err("pilot-only recording gained external stores".into());
        }
        normal.clean_recording = false;
        normal_view.mission_dummies(&mut normal, &[(id, 29)], 5280., data)?;
        normal.reset(&mut flight)?;
        let positions: Vec<_> = normal.state.targets.iter().map(|t| t.position).collect();
        if positions.len() != 29 {
            return Err("mission wing count mismatch".into());
        }
        let camera = airframe.panel_camera(&flight, 3);
        let geometry = normal_view.dummy_geometry(&normal, &camera, world, scenery);
        if geometry.len() != 1 || geometry[0].0.profile.id != id || geometry[0].1.is_empty() {
            return Err("dummy model identity or geometry mismatch".into());
        }
        let intact = airframe.vertices(&flight, &camera, world, scenery);
        // Exercise the shared effects against every imported mesh and gun,
        // including variants, rather than inferring coverage from F/A-18D.
        // A surviving aircraft is drawn intact (damage still drives flight and
        // systems); only a destroyed one shows its regional body variant.
        for region in [3usize, 4, 5] {
            for fraction in [0.1, 0.4, 0.8, 1.0] {
                flight.damage_fraction = fraction;
                flight.damage_variant = (fraction >= 1.).then_some(region);
                flight.damage_regions = [0.; tore_sim::combat::live::DAMAGE_SECTIONS];
                flight.damage_regions[region] = fraction;
                let mesh = airframe.vertices(&flight, &camera, world, scenery);
                let distinct = mesh != intact;
                let rotor_wing_unmapped = matches!(
                    id,
                    tore_formats::aircraft::AircraftId::Ah64
                        | tore_formats::aircraft::AircraftId::Mi24
                        | tore_formats::aircraft::AircraftId::Ch47
                ) && [3, 4].contains(&region);
                if fraction >= 1. && !distinct && rotor_wing_unmapped {
                    println!(
                        "{id:?}: destroyed wing region {region} has no applicable fitted wing geometry; rotorcraft visual damage remains partial"
                    );
                }
                if mesh.is_empty()
                    || mesh.iter().any(|v| !v.is_finite())
                    || (distinct != (fraction >= 1.) && !(fraction >= 1. && rotor_wing_unmapped))
                {
                    return Err(format!("{id:?}: damage region {region} at {fraction} has no finite geometry, or a survivor changed shape, or the wreck did not").into());
                }
            }
        }
        let mut gun_checks = 0;
        if id != tore_formats::aircraft::AircraftId::Ac130 {
            for station in load
                .configuration
                .stations
                .iter()
                .enumerate()
                .filter(|(_, station)| id.guns().contains(&station.weapon.source.as_str()))
                .map(|(slot, _)| slot)
            {
                let mut gun = crate::combat::Combat::new(&airframe, data, false)?;
                let gun_view = crate::combat_view::CombatView::new(&gun, gun.own_id(), data)?;
                gun.state.own_mut().armed = true;
                gun.state.own_mut().selected = station;
                let gun_flight = airframe.start(world);
                let launcher = crate::combat::launcher(&gun_flight);
                for _ in 0..120 {
                    gun.state.step(
                        &[OwnshipInput {
                            aircraft: 0,
                            held: true,
                            launcher,
                        }],
                        |_, _| 0.,
                    );
                    if !gun.state.projectiles.is_empty()
                        && id != tore_formats::aircraft::AircraftId::F4J
                    {
                        break;
                    }
                }
                if id == tore_formats::aircraft::AircraftId::F4J {
                    if gun.state.own().rounds(station) == 0
                        || gun.state.own().rounds(station) >= 599
                    {
                        return Err(
                            "source F4J gun pod did not sustain fire from its 600-unit budget"
                                .into(),
                        );
                    }
                    let retained = gun.state.own().payload_lbs();
                    gun.state.own_mut().ammo[station] = 0;
                    if gun.state.own().payload_lbs() != retained {
                        return Err("source F4J empty gun pod lost its hardware mass".into());
                    }
                }
                let round = gun
                    .state
                    .projectiles
                    .first()
                    .ok_or("imported gun failed to fire")?;
                if !tore_sim::combat::live::is_gun(round.weapon(gun.state.own().configuration())) {
                    return Err(
                        format!("{id:?}: imported gun is missing shared gun behavior").into(),
                    );
                }
                let gun_station =
                    &gun.state.own().configuration().stations[gun.state.own().selected];
                let pipper = tore_sim::combat::gunsight::solve(
                    &gun_station.weapon,
                    &launcher,
                    gun_station.gun_mount(),
                    None,
                )?
                .ok_or_else(|| format!("{id:?}: imported gun has no 1000-foot sight solution"))?;
                if !pipper.point.iter().all(|v| v.is_finite())
                    || (pipper.range_ft - 1000.).abs() > 0.01
                    || pipper.maximum_range_ft <= 100.
                {
                    return Err(format!("{id:?}: invalid imported gun sight/range").into());
                }
                gun.refresh_render(gun.own_id(), &gun_flight, None);
                let tracer =
                    gun_view.vertices(&gun, &airframe, &gun_flight, &camera, world, scenery);
                if !tracer.vertices.chunks_exact(10).any(|v| v[5] == -8.) {
                    return Err(
                        format!("{id:?}: imported gun has no luminous tracer geometry").into(),
                    );
                }
                gun_checks += 1;
            }
        }
        // Only a destroyed aircraft draws its damaged body (survivors stay intact).
        flight.damage_fraction = 1.;
        flight.damage_variant = Some(tore_sim::combat::live::DamageSection::LeftWing as usize);
        flight.damage_regions = [0.; tore_sim::combat::live::DAMAGE_SECTIONS];
        flight.damage_regions[tore_sim::combat::live::DamageSection::LeftWing as usize] = 1.;
        let damaged = airframe.vertices(&flight, &camera, world, scenery);
        let fragment = airframe.fragment_vertices(&flight, &camera, world, scenery);
        if tore_sim::combat::debris::damage_variant(
            id,
            tore_sim::combat::live::DamageSection::LeftWing as usize,
        )
        .is_some()
            && (fragment.is_empty() || fragment == damaged)
        {
            return Err("detached model missing or substituted".into());
        }
        if (intact == damaged
            && !matches!(
                id,
                tore_formats::aircraft::AircraftId::Ah64
                    | tore_formats::aircraft::AircraftId::Mi24
                    | tore_formats::aircraft::AircraftId::Ch47
            ))
            || damaged.is_empty()
            || damaged
                .chunks_exact(10)
                .any(|v| !v.iter().all(|x| x.is_finite()))
        {
            return Err("damaged body did not produce distinct valid geometry".into());
        }
        flight.damage_fraction = 0.;
        flight.damage_variant = None;
        flight.damage_regions = [0.; tore_sim::combat::live::DAMAGE_SECTIONS];
        normal.state.targets[0].hp = 0;
        normal.refresh_render(normal.own_id(), &flight, None);
        normal.step(&mut flight, world)?;
        normal.advance_render(normal.own_id(), &flight, None);
        if normal_view.dummy_geometry(&normal, &camera, world, scenery)[0]
            .1
            .is_empty()
        {
            return Err("falling wreck disappeared".into());
        }
        normal.reset(&mut flight)?;
        if !normal.state.smoke.puffs.is_empty()
            || !normal.state.debris.is_empty()
            || flight.damage_fraction != 0.
        {
            return Err("reset retained damage appearance".into());
        }
        if normal
            .state
            .targets
            .iter()
            .map(|t| t.position)
            .collect::<Vec<_>>()
            != positions
            || normal.state.targets.iter().any(|t| t.hp <= 0)
        {
            return Err("restart did not restore dummy formation".into());
        }
        load.fuel(false);
        for i in 0..load.quantities.len() {
            load.change(i, -1);
        }
        let mut alternatives = 0;
        for (name, bytes) in data
            .iter()
            .filter(|(n, _)| tore_sim::combat::loadout::supported(n))
        {
            let weapon = Weapon::parse(name, bytes)?;
            if let Some(i) = (0..load.quantities.len()).find(|i| {
                !load.configuration.stations[*i].internal && load.capacity(*i, &weapon) > 0
            }) {
                let mut candidate = load.clone();
                candidate.select(i, weapon)?;
                if candidate.total_lbs() > candidate.maximum_lbs {
                    if candidate.validate().is_ok() {
                        return Err("overweight store selection was accepted".into());
                    }
                    continue;
                }
                candidate.validate()?;
                let mut combat = crate::combat::Combat::with_loadout(&airframe, &candidate)?;
                let mut flight = airframe.start(world);
                flight.fuel = candidate.fuel_lbs;
                combat.reset(&mut flight)?;
                if combat.state.own().ammo != candidate.ammunition()?
                    || combat.range
                    || (flight.payload_lbs - combat.state.own().payload_lbs()).abs() > 0.01
                {
                    return Err("custom loadout initialization mismatch".into());
                }
                combat.state.own_mut().ammo.fill(0);
                combat.reset(&mut flight)?;
                if combat.state.own().ammo != candidate.ammunition()? {
                    return Err("restart lost accepted loadout".into());
                }
                alternatives += 1;
            }
        }
        if alternatives == 0 && load.configuration.stations.iter().any(|s| !s.internal) {
            return Err("no supported compatible external loadout exercised".into());
        }
        load.quantities.fill(0);
        load.fuel_lbs = 0.;
        load.refresh_equipment()?;
        load.validate()?;
        let mut combat = crate::combat::Combat::with_loadout(&airframe, &load)?;
        let mut flight = airframe.start(world);
        flight.fuel = 0.;
        combat.reset(&mut flight)?;
        if combat.state.own().ammo.iter().any(|n| *n != 0) {
            return Err("empty loadout gained ammunition".into());
        }
        println!(
            "{}: {alternatives} supported store/placement cases, edited fuel, empty stations, normal weapons, guns-only for all six wings and restart, {gun_checks} applicable forward gun sights/tracers, regional damage stages, 29 dummy models and restart passed",
            id.label()
        );
    }
    Ok(())
}

fn source_station_point(
    ui: &mut Ordnance,
    slot: usize,
    pixels: &mut [u8],
) -> AppResult<(f64, f64)> {
    let row = ui
        .loadout
        .editable_stations()
        .iter()
        .position(|row| row.weapon == Some(slot))
        .ok_or("weapon station has no editable source row")?;
    ui.station = row;
    ui.reveal_station();
    ui.render(pixels);
    // The page draws in display order (gun, centreline, then outward), so
    // the card's place on the six-card page is that order, not the source
    // index. A Rafale centreline tank shifts every later card.
    let listed = ui
        .display_order()
        .iter()
        .position(|&index| index == row)
        .ok_or("weapon station is not listed")?;
    let visible = listed % STATIONS_PER_PAGE;
    Ok((
        360. + (visible % 2) as f64 * 119.,
        145. + (visible / 2) as f64 * 71.,
    ))
}
fn source_catalog_drag(
    ui: &mut Ordnance,
    slot: usize,
    weapon: &Weapon,
    pixels: &mut [u8],
) -> AppResult<()> {
    let index = ui
        .catalog
        .iter()
        .position(|candidate| candidate.source == weapon.source)
        .ok_or("imported station weapon missing from catalog")?;
    ui.category = usize::from(weapon.flags & 0x10000 == 0);
    let entry = ui
        .page_entries()
        .iter()
        .position(|i| *i == CatalogEntry::Weapon(index))
        .unwrap();
    ui.pages[ui.category] = entry / 8;
    let destination = source_station_point(ui, slot, pixels)?;
    let row = entry % 8;
    ui.pointer(Some((
        80. + (row % 2) as f64 * 120.,
        120. + (row / 2) as f64 * 68.,
    )));
    ui.down();
    ui.pointer(Some(destination));
    ui.up();
    if i32::from(ui.loadout.quantities[slot]) != ui.loadout.capacity(slot, weapon) {
        return Err("catalog drag did not refill the imported station".into());
    }
    Ok(())
}
/// The record names the Load Ordnance page's weapon catalog offers for
/// `load`, normal or under Cheat loading, in the page's order. The ordnance
/// audit checks the lists against the station rules (A2G decision 16).
pub fn catalog_sources(
    load: &Loadout,
    data: &BTreeMap<String, Vec<u8>>,
    cheat: bool,
) -> AppResult<Vec<String>> {
    let mut load = load.clone();
    load.cheat = cheat;
    let ui = Ordnance::new(load, data)?;
    Ok(ui.catalog.iter().map(|w| w.source.clone()).collect())
}

/// The designator pod names the page's weapon catalog offers for `load`,
/// normal or under Cheat loading.
pub fn designator_sources(
    load: &Loadout,
    data: &BTreeMap<String, Vec<u8>>,
    cheat: bool,
) -> AppResult<Vec<String>> {
    let mut load = load.clone();
    load.cheat = cheat;
    let ui = Ordnance::new(load, data)?;
    Ok(ui
        .designator_catalog
        .iter()
        .map(|pod| pod.source.clone())
        .collect())
}

/// Check an isolated weapon drag into an empty destination. Catalog refills
/// and saved-load migration happen before this snapshot and can change totals.
fn check_station_transfer(
    before: &Loadout,
    after: &Loadout,
    source: usize,
    target: usize,
) -> AppResult<u16> {
    use tore_world::mission::LoadoutSpec;
    let weapon = &before.configuration.stations[source].weapon;
    let capacity = before.capacity(target, weapon) as u16;
    let paired = before.store_pairs(source).step(1) == 2 || before.store_pairs(target).step(1) == 2;
    let available = before.quantities[source].min(capacity);
    let moved = if paired {
        if available >= 2 { 2 } else { 0 }
    } else {
        let step = match capacity {
            301.. => 100,
            101..=300 => 10,
            _ => 1,
        };
        available.min(step)
    };
    let mut expected = LoadoutSpec::of(before);
    if expected.stations[target].quantity != 0 {
        return Err("station transfer probe needs an empty weapon destination".into());
    }
    if moved > 0 {
        expected.stations[source].quantity -= moved;
        expected.stations[target] = tore_world::mission::StationLoad {
            weapon: weapon.source.clone(),
            count: capacity,
            quantity: moved,
        };
    }
    let actual = LoadoutSpec::of(after);
    if actual != expected || (after.total_lbs() - before.total_lbs()).abs() > 0.01 {
        return Err(format!(
            "station transfer mismatch: {} {} slots {source}->{target}, expected {moved} moved; expected {expected:?}, got {actual:?}",
            before.aircraft.label(), weapon.source,
        ).into());
    }
    Ok(moved)
}

fn validate_dragging(load: &Loadout, data: &BTreeMap<String, Vec<u8>>) -> AppResult<()> {
    let mut ui = Ordnance::new(load.clone(), data)?;
    let mut pixels = vec![0; crate::menu::WIDTH * crate::menu::HEIGHT * 4];
    let Some(source) = load
        .configuration
        .stations
        .iter()
        .position(|station| !station.internal)
    else {
        println!(
            "{}: external weapon drag not applicable",
            load.aircraft.label()
        );
        return Ok(());
    };
    let weapon = &load.configuration.stations[source].weapon;
    // Newly exposed mounts are genuinely empty. Load one through the actual
    // catalog gesture before exercising removal, rather than pretending it was installed.
    if ui.loadout.quantities[source] == 0 {
        source_catalog_drag(&mut ui, source, weapon, &mut pixels)?;
    }
    let from = source_station_point(&mut ui, source, &mut pixels)?;
    ui.pointer(Some(from));
    ui.down();
    ui.pointer(Some((290., 370.)));
    ui.up();
    if ui.loadout.quantities[source] != 0 {
        return Err("catalog drop did not empty the imported station".into());
    }
    source_catalog_drag(&mut ui, source, weapon, &mut pixels)?;
    // Isolate weapon conservation from deliberate replacement of a tank.
    ui.loadout.clear_tanks()?;
    let filled = ui.loadout.clone();
    let mut checked = 0;
    let mut skipped = 0;
    for target in (0..load.quantities.len())
        .filter(|i| *i != source && (1..32767).contains(&load.capacity(*i, weapon)))
    {
        ui.loadout = filled.clone();
        ui.loadout.quantities[target] = 0;
        ui.loadout.refresh_equipment()?;
        let before = ui.loadout.clone();
        let from = source_station_point(&mut ui, source, &mut pixels)?;
        let source_page = ui.station_page;
        let to = source_station_point(&mut ui, target, &mut pixels)?;
        // Only the visible page has hit targets. Report skipped pairs rather
        // than allowing the first cross-page target to end the aircraft's sweep.
        if ui.station_page != source_page {
            skipped += 1;
            continue;
        }
        ui.pointer(Some(from));
        ui.down();
        ui.pointer(Some(to));
        ui.up();
        let moved = check_station_transfer(&before, &ui.loadout, source, target)?;
        let row = |slot| {
            load.configuration
                .hardpoint_slots
                .iter()
                .position(|candidate| *candidate == Some(slot))
                .unwrap()
        };
        println!(
            "station transfer {} {}: row {} (paired={}) -> row {} (paired={}, capacity={}): [{}, 0] -> [{}, {}], moved={moved}; passed",
            load.aircraft.selection_key(),
            weapon.source,
            row(source),
            before.store_pairs(source).step(1) == 2,
            row(target),
            before.store_pairs(target).step(1) == 2,
            before.capacity(target, weapon),
            before.quantities[source],
            ui.loadout.quantities[source],
            ui.loadout.quantities[target],
        );
        checked += 1;
    }
    println!(
        "{}: {checked} station transfers checked, {skipped} cross-page transfers skipped",
        load.aircraft.label()
    );
    Ok(())
}

fn validate_removed_stores(
    load: &Loadout,
    airframe: &crate::aircraft::Airframe,
    world: &crate::terrain::Terrain,
) -> AppResult<()> {
    let stations = load.quantities.len();
    let mut cases: Vec<Vec<u16>> = (0..stations)
        .map(|removed| {
            let mut q = load.quantities.clone();
            q[removed] = 0;
            q
        })
        .collect();
    cases.push(vec![0; stations]);
    // Everything but the internal guns, the usual "strip the stores" edit.
    cases.push(
        load.configuration
            .stations
            .iter()
            .zip(&load.quantities)
            .map(|(s, n)| if s.internal { *n } else { 0 })
            .collect(),
    );
    for quantities in cases {
        let mut edited = load.clone();
        edited.quantities = quantities.clone();
        edited.refresh_equipment()?;
        let expected_ammo = edited.ammunition()?;
        edited.validate()?;
        let mut combat = crate::combat::Combat::with_loadout(airframe, &edited)?;
        let mut flight = airframe.start(world);
        for _ in 0..2 {
            combat.reset(&mut flight)?;
            combat.apply_startup_weapons();
            if combat.state.own().ammo != expected_ammo {
                return Err(format!(
                    "{:?}: removed stores came back at launch or restart: {:?} instead of {expected_ammo:?}",
                    airframe.profile.id, combat.state.own().ammo
                )
                .into());
            }
            let readout = combat
                .cockpit_readout(
                    combat.own_id(),
                    crate::combat::launcher(&flight),
                    None,
                    None,
                )
                .expect("the ordnance check's plane has an ownship");
            let listed = crate::combat_view::readout(
                &combat,
                combat.state.own().configuration(),
                &readout,
                &flight,
                flight.sensors,
                1.,
            )
            .weapons;
            let carried: std::collections::BTreeSet<&str> = combat
                .state
                .own()
                .configuration()
                .stations
                .iter()
                .enumerate()
                .filter(|(i, _)| quantities[*i] > 0)
                .map(|(_, s)| s.weapon.hud_name.as_str())
                .collect();
            if listed.iter().any(|(name, count, selected)| {
                !carried.contains(name.as_str()) && (*count > 0 || *selected)
            }) || listed.iter().map(|(_, count, _)| *count).sum::<u32>()
                != expected_ammo.iter().map(|n| u32::from(*n)).sum::<u32>()
            {
                return Err(format!(
                    "{:?}: the weapons window lists {listed:?} for {quantities:?}",
                    airframe.profile.id
                )
                .into());
            }
            if combat.state.own().armed && expected_ammo[combat.state.own().selected] == 0 {
                return Err("an empty station started selected".into());
            }
            let external: f64 = combat
                .state
                .own()
                .configuration()
                .stations
                .iter()
                .zip(&quantities)
                .filter(|(s, _)| !s.internal && s.weapon.source != "SUU16.JT")
                .map(|(s, n)| s.weapon.carried_weight_lb() * f64::from(*n))
                .sum::<f64>()
                + f64::from(combat.state.own().configuration().external_equipment_lbs);
            if (flight.payload_lbs - external).abs() > 0.5 {
                return Err("flight payload does not match the edited stores".into());
            }
            combat.state.own_mut().ammo.fill(1);
        }
    }
    Ok(())
}

fn validate_guns_only(
    load: &Loadout,
    airframe: &crate::aircraft::Airframe,
    targets: &[tore_sim::combat::live::Target],
    data: &BTreeMap<String, Vec<u8>>,
    world: &crate::terrain::Terrain,
) -> AppResult<()> {
    use tore_sim::ai::{
        launch::{Side, WingId, WingSelection, resolve_wings},
        weapon_service::Rounds,
    };
    let selections: Vec<_> = (0..6)
        .map(|group| WingSelection {
            wing: WingId::new(
                if group < 3 {
                    Side::Friendly
                } else {
                    Side::Enemy
                },
                group % 3,
            )
            .unwrap(),
            aircraft: load.aircraft,
            count: if group == 0 { 4 } else { 5 },
            skill_level: 1,
        })
        .collect();
    let wings = resolve_wings(&selections, None)?;
    let expected_ammo = load.ammunition()?;
    // Rebuilding is also the live restart path. Standard loads after guns-only
    // must still come from the unmodified imported aircraft records.
    for guns_only in [true, true, false] {
        let bridge = crate::ai_wings::AiWings::build(&wings, targets, guns_only, data)?;
        if bridge.len() != 29 {
            return Err("guns-only validation lost wing members".into());
        }
        let tank_fuel: f64 = load.configuration.external_fuel_lbs.iter().sum();
        for actor in bridge.mission().actors() {
            if actor.stations().len() != load.configuration.stations.len() {
                return Err("guns-only validation lost stations".into());
            }
            // A default load's tanks start full and the systems know every
            // engine, as the player's do (an F/A-18 or F-16C centreline tank).
            let systems = &actor.flight().systems;
            if systems.external_lbs() != tank_fuel
                || systems.engine.count() != load.configuration.engines
            {
                return Err(format!(
                    "{:?} AI default load starts with {:.0} lb external fuel and {} engines, not {tank_fuel:.0} lb and {}",
                    load.aircraft,
                    systems.external_lbs(),
                    systems.engine.count(),
                    load.configuration.engines
                )
                .into());
            }
            for (index, (actual, original)) in actor
                .stations()
                .iter()
                .zip(&load.configuration.stations)
                .enumerate()
            {
                let count = if guns_only && !load.aircraft.mounts_gun(&original.weapon.source) {
                    0
                } else {
                    u32::from(expected_ammo[index])
                };
                if actual.rounds() != Rounds::Finite(count) {
                    return Err("wing weapon restriction mismatch".into());
                }
            }
        }
    }
    let mut guns = load.clone();
    guns.restrict_to_guns();
    let mut combat = crate::combat::Combat::with_loadout(airframe, &guns)?;
    let mut flight = airframe.start(world);
    for _ in 0..2 {
        combat.reset(&mut flight)?;
        if combat.state.own().ammo != guns.ammunition()? {
            return Err("player guns-only load lost at launch or restart".into());
        }
        combat.state.own_mut().ammo.fill(0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{HEIGHT, WIDTH};
    use tore_formats::aircraft::Hardpoint;

    #[test]
    fn the_tank_probe_covers_the_f14_and_refuses_any_other_aircraft() {
        use tore_formats::aircraft::AircraftId;
        let empty = BTreeMap::new();
        for id in [AircraftId::F18, AircraftId::A4E, AircraftId::Faxx] {
            let error = validate_tanks(&empty, Some(id)).unwrap_err().to_string();
            assert!(error.contains("F-14 tank contract only"), "{error}");
        }
        // The F-14, named or by default, goes on to read the F-14's record.
        for requested in [Some(AircraftId::F14), None] {
            let error = validate_tanks(&empty, requested).unwrap_err().to_string();
            assert!(error.contains("missing F14.PT"), "{error}");
        }
    }

    fn fixture() -> Ordnance {
        let mut config = tore_world::test_support::combat_fixture(true)
            .own()
            .configuration()
            .clone();
        config.stations[0].weapon.source = "AIM9M.JT".into();
        config.stations[0].weapon.flags |= 0x10002;
        config.stations[0].count = 4;
        let mut gun = config.stations[0].clone();
        gun.weapon.source = "M61.JT".into();
        gun.weapon.flags |= 0x80;
        gun.internal = true;
        gun.count = 500;
        config.stations.push(config.stations[0].clone());
        config.stations.push(gun);
        config.hardpoint_slots = vec![Some(0), Some(1), Some(2)];
        let catalog = vec![config.stations[0].weapon.clone()];
        let loadout = Loadout {
            aircraft: config.aircraft,
            configuration: config,
            quantities: vec![2, 0, 500],
            fuel_lbs: 1000.,
            internal_capacity_lbs: 1000.,
            empty_lbs: 10000.,
            maximum_lbs: 20000.,
            cheat: false,
            tank_hardpoints: vec![],
            hardpoints: (0..3)
                .map(|i| Hardpoint {
                    location: if i == 2 { 2 } else { 4 },
                    flags: if i == 2 { 8 } else { 0x20 },
                    position: [0; 3],
                    store: Some(if i == 2 { "M61.JT" } else { "AIM9M.JT" }.into()),
                    count: if i == 2 { 500 } else { 4 },
                    weight_class: 0,
                })
                .collect(),
        };
        let mut sprites = BTreeMap::new();
        sprites.insert(
            "ORD_AIR3.PIC".into(),
            Sprite {
                width: WIDTH,
                height: HEIGHT,
                rgba: [17, 19, 23, 255].repeat(WIDTH * HEIGHT),
                glyphs: vec![],
            },
        );
        for name in [
            "QUICKFONT",
            "MENUFONT.PIC",
            "ARMFONT.PIC",
            "WPNBLUE",
            "WPNYELLOW",
            "FONTACT.PIC",
        ] {
            sprites.insert(name.into(), crate::menu::flat_font([200, 200, 200]));
        }
        for name in [
            "ROCKER00.PIC",
            "ROCKER01.PIC",
            "ROCKER02.PIC",
            "ROCKER03.PIC",
            "ROCKER04.PIC",
            "DIAL11.PIC",
            "DIAL13.PIC",
            "LIGHTON.PIC",
            "LIGHTOFF.PIC",
            "ACTDFLT.PIC",
            "ACTDFT0L.PIC",
            "ACTDFT0M.PIC",
            "ACTDFT0R.PIC",
            "ACTION0L.PIC",
            "ACTION0M.PIC",
            "ACTION0R.PIC",
        ] {
            sprites.insert(
                name.into(),
                Sprite {
                    width: 1,
                    height: 1,
                    rgba: vec![0; 4],
                    glyphs: vec![],
                },
            );
        }
        let mut rgba = vec![0; 5 * 3 * 4];
        rgba[(5 + 2) * 4..(5 + 3) * 4].copy_from_slice(&[0, 255, 0, 255]);
        sprites.insert(
            "$AIM9M.PIC".into(),
            Sprite {
                width: 5,
                height: 3,
                rgba,
                glyphs: vec![],
            },
        );
        let mut ui = Ordnance {
            loadout,
            visible: true,
            weapons: catalog.clone(),
            catalog,
            tanks: vec![],
            tank_catalog: vec![],
            designators: vec![],
            designator_catalog: vec![],
            sprites,
            category: 0,
            pages: [0; 2],
            selected: None,
            station: 0,
            station_page: 0,
            retained_kind: BTreeMap::new(),
            hover: None,
            pressed: None,
            right_pressed: None,
            rockers: Default::default(),
            pointer: None,
            drag: None,
            controls: vec![],
            message: None,
            menu: false,
            lobby: false,
            lobby_cheat: false,
            gaps: BTreeMap::new(),
            opened: Instant::now(),
            frozen_ms: Some(0),
        };
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        ui
    }
    fn drag(ui: &mut Ordnance, from: (f64, f64), to: (f64, f64)) -> Action {
        ui.pointer(Some(from));
        ui.down();
        ui.pointer(Some(to));
        ui.up()
    }
    #[test]
    fn tank_page_removal_reload_and_unload_all_change_fuel_without_ammunition() {
        use tore_sim::combat::live::{TankStation, TankStore};
        let mut ui = fixture();
        let tank = TankStore {
            source: "SYNTHETIC.GAS".into(),
            name: "Synthetic tank".into(),
            tank: tore_formats::weapons::Tank {
                empty_weight: 198,
                fuel_weight: 1650,
                flags: 1,
            },
        };
        ui.loadout.configuration.tanks = vec![TankStation {
            hardpoint: 5,
            mount: [0.; 3],
            rack: Default::default(),
            store: Some(tank.clone()),
            quantity: 2,
        }];
        ui.loadout.configuration.hardpoint_slots.resize(6, None);
        ui.loadout.tank_hardpoints = vec![Hardpoint {
            location: 4,
            flags: 0x200,
            position: [0; 3],
            store: Some(tank.source.clone()),
            count: 2,
            weight_class: 0,
        }];
        ui.loadout.configuration.refresh_tanks().unwrap();
        ui.tanks = vec![tank];
        ui.rebuild_catalog();
        let weapons = ui.loadout.quantities.clone();
        ui.activate(16);
        ui.station = ui
            .loadout
            .editable_stations()
            .iter()
            .position(|row| row.hardpoint == 5)
            .unwrap();
        let row_id = 200 + ui.station;
        assert_eq!(ui.category, 1);
        ui.activate(10);
        ui.activate(10);
        assert_eq!(ui.loadout.external_fuel_lbs(), 0.);
        assert_eq!(ui.loadout.tank_shell_lbs(), 0.);
        assert_eq!(ui.loadout.quantities, weapons);
        ui.activate(100);
        ui.activate(row_id);
        assert_eq!(ui.loadout.external_fuel_lbs(), 3300.);
        assert_eq!(ui.loadout.tank_shell_lbs(), 396.);
        ui.activate(12);
        assert_eq!(ui.loadout.external_fuel_lbs(), 0.);
        assert_eq!(ui.loadout.tank_shell_lbs(), 0.);
        assert!(ui.loadout.quantities.iter().all(|n| *n == 0));
    }

    fn mixed_fixture() -> Ordnance {
        use tore_sim::combat::live::{TankStation, TankStore};
        let mut ui = fixture();
        let tank = TankStore {
            source: "SYNTHETIC.GAS".into(),
            name: "Synthetic tank".into(),
            tank: tore_formats::weapons::Tank {
                empty_weight: 198,
                fuel_weight: 1650,
                flags: 1,
            },
        };
        for hardpoint in [0, 1, 5] {
            let source = if hardpoint < 2 {
                ui.loadout.hardpoints[hardpoint].flags |= 0x200;
                ui.loadout.hardpoints[hardpoint].clone()
            } else {
                Hardpoint {
                    location: 1,
                    flags: 0x200,
                    position: [0; 3],
                    store: Some(tank.source.clone()),
                    count: 2,
                    weight_class: 0,
                }
            };
            ui.loadout.tank_hardpoints.push(source);
            ui.loadout.configuration.tanks.push(TankStation {
                hardpoint,
                mount: [0.; 3],
                rack: Default::default(),
                store: Some(tank.clone()),
                quantity: 0,
            });
        }
        ui.loadout.configuration.hardpoint_slots.resize(6, None);
        ui.loadout.configuration.refresh_tanks().unwrap();
        ui.tanks = vec![tank];
        let icon = &ui.sprites["$AIM9M.PIC"];
        let icon = Sprite {
            width: icon.width,
            height: icon.height,
            rgba: icon.rgba.clone(),
            glyphs: icon.glyphs.clone(),
        };
        ui.sprites.insert("$SYNTHETIC.PIC".into(), icon);
        ui.rebuild_catalog();
        ui
    }
    #[test]
    fn air_surface_catalog_puts_tanks_first_and_keeps_mixed_page_indices_distinct() {
        let mut ui = mixed_fixture();
        let mut surface = ui.weapons[0].clone();
        surface.source = "MK82.JT".into();
        surface.flags &= !0x10000;
        ui.weapons.push(surface);
        ui.loadout.cheat = true;
        let tank = ui.tanks[0].clone();
        ui.tanks = (0..10)
            .map(|index| {
                let mut tank = tank.clone();
                tank.source = format!("SYNTH{index}.GAS");
                tank
            })
            .collect();
        ui.rebuild_catalog();
        ui.activate(4);
        let entries = ui.page_entries();
        assert_eq!(
            &entries[..10],
            &(0..10).map(CatalogEntry::Tank).collect::<Vec<_>>()
        );
        assert_eq!(entries[10], CatalogEntry::Weapon(1));
        ui.pages[1] = 1;
        assert_eq!(ui.card_index(100), Some(CatalogEntry::Tank(8)));
        assert_eq!(ui.card_index(102), Some(CatalogEntry::Weapon(1)));
        ui.activate(100);
        assert_eq!(ui.selected, Some(CatalogEntry::Tank(8)));
        ui.activate(3);
        assert!(
            ui.page_entries()
                .iter()
                .all(|entry| matches!(entry, CatalogEntry::Weapon(_)))
        );
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        assert!(!ui.controls.iter().any(|(id, _)| *id == 16));
    }
    #[test]
    fn empty_quantity_restore_keeps_last_actual_tank_kind_after_catalog_switch() {
        let mut ui = mixed_fixture();
        ui.activate(4);
        ui.drop_store(DragSource::TankCatalog(0), 0);
        ui.selected = None;
        ui.station = 0;
        while ui.loadout.configuration.tanks[0].quantity > 0 {
            ui.activate(10);
        }
        ui.activate(3);
        ui.activate(9);
        assert_eq!(ui.loadout.configuration.tanks[0].quantity, 1);
        assert_eq!(ui.loadout.quantities[0], 0);
    }
    #[test]
    fn both_catalogs_leave_the_removed_tank_button_and_banner_as_original_background() {
        let mut ui = mixed_fixture();
        for category in [3, 4] {
            ui.activate(category);
            let mut pixels = vec![0; WIDTH * HEIGHT * 4];
            ui.render(&mut pixels);
            let background = &ui.sprites["ORD_AIR3.PIC"].rgba;
            for (x, y, width, height) in [(278, 35, 60, 24), (343, 69, 295, 20)] {
                for row in y..y + height {
                    let start = (row * WIDTH + x) * 4;
                    let end = start + width * 4;
                    assert_eq!(&pixels[start..end], &background[start..end]);
                }
            }
            assert!(!ui.controls.iter().any(|(id, _)| *id == 16));
        }
    }
    #[test]
    fn unified_source_rows_deduplicate_shared_mounts_and_keep_occupants_across_catalogs() {
        let mut ui = mixed_fixture();
        let rows = ui.loadout.editable_stations();
        assert_eq!(
            rows.iter().map(|row| row.hardpoint).collect::<Vec<_>>(),
            vec![0, 1, 2, 5]
        );
        assert_eq!(rows[0].weapon, Some(0));
        assert_eq!(rows[0].tank, Some(0));
        assert_eq!(rows[3].weapon, None);
        assert_eq!(rows[3].tank, Some(2));
        ui.loadout
            .select_tank(0, ui.tank_catalog[0].clone())
            .unwrap();
        let mut a = vec![0; WIDTH * HEIGHT * 4];
        let mut b = a.clone();
        ui.render(&mut a);
        let first: Vec<_> = ui
            .controls
            .iter()
            .filter(|(id, _)| (200..232).contains(id))
            .copied()
            .collect();
        ui.activate(16);
        ui.render(&mut b);
        assert_eq!(
            first,
            ui.controls
                .iter()
                .filter(|(id, _)| (200..232).contains(id))
                .copied()
                .collect::<Vec<_>>()
        );
        for y in 115..328 {
            assert_eq!(
                &a[(y * WIDTH + 349) * 4..(y * WIDTH + 584) * 4],
                &b[(y * WIDTH + 349) * 4..(y * WIDTH + 584) * 4]
            );
        }
        // Loaded tank changes as equipment even while the weapon catalog is open.
        ui.activate(3);
        ui.station = 0;
        let before = ui.loadout.configuration.tanks[0].quantity;
        ui.activate(10);
        assert_eq!(ui.loadout.configuration.tanks[0].quantity, before - 1);
        // A loaded weapon increments as a weapon while a tank catalog card is selected.
        ui.activate(16);
        ui.selected = Some(CatalogEntry::Tank(0));
        let gun = ui.loadout.quantities[2];
        ui.loadout.quantities[2] = gun - 1;
        ui.activate(202);
        assert_eq!(ui.loadout.quantities[2], gun);
    }
    #[test]
    fn tank_drag_replaces_weapon_transfers_one_unloads_and_rejects_wrong_station_type() {
        let mut ui = mixed_fixture();
        ui.activate(16);
        assert_eq!(
            ui.drop_store(DragSource::TankCatalog(0), 0),
            Action::OrdnanceFuel
        );
        assert_eq!(ui.loadout.quantities[0], 0);
        let loaded = ui.loadout.configuration.tanks[0].quantity;
        assert!(loaded > 0);
        assert_eq!(
            ui.drop_store(DragSource::TankStation(0), 1),
            Action::OrdnanceFuel
        );
        assert_eq!(ui.loadout.configuration.tanks[0].quantity, loaded - 1);
        assert_eq!(ui.loadout.configuration.tanks[1].quantity, 1);
        let quantities = ui.loadout.quantities.clone();
        let tank_count: Vec<_> = ui
            .loadout
            .configuration
            .tanks
            .iter()
            .map(|tank| tank.quantity)
            .collect();
        assert_eq!(ui.drop_store(DragSource::TankCatalog(0), 2), Action::None);
        assert_eq!(ui.loadout.quantities, quantities);
        assert_eq!(
            ui.loadout
                .configuration
                .tanks
                .iter()
                .map(|tank| tank.quantity)
                .collect::<Vec<_>>(),
            tank_count
        );
        assert_eq!(ui.drop_store(DragSource::Catalog(0), 3), Action::None);
        assert_eq!(ui.loadout.quantities, quantities);
        ui.activate(3);
        assert_eq!(
            ui.drop_store(DragSource::Catalog(0), 0),
            Action::OrdnanceWeapon
        );
        assert_eq!(ui.loadout.configuration.tanks[0].quantity, 0);
        assert!(ui.loadout.quantities[0] > 0);
        // Actual mouse dragging works from a tank shown in a weapon catalog.
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        assert_eq!(
            drag(&mut ui, (400., 215.), (75., 360.)),
            Action::OrdnanceFuel
        );
        assert_eq!(ui.loadout.configuration.tanks[1].quantity, 0);
        assert!(ui.loadout.quantities[0] > 0);
    }
    #[test]
    fn station_pages_never_overlap_weight_fields_and_tab_reveals_all_source_rows() {
        let mut ui = fixture();
        let station = ui.loadout.configuration.stations[0].clone();
        let hardpoint = ui.loadout.hardpoints[0].clone();
        ui.loadout.configuration.stations = vec![station; 9];
        ui.loadout.configuration.hardpoint_slots = (0..9).map(Some).collect();
        ui.loadout.hardpoints = vec![hardpoint; 9];
        ui.loadout.quantities = vec![0; 9];
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        let cards: Vec<_> = ui
            .controls
            .iter()
            .filter(|(id, _)| (200..232).contains(id))
            .copied()
            .collect();
        assert_eq!(cards.len(), 6);
        assert!(
            cards
                .iter()
                .all(|(_, rect)| rect.1 >= 115 && rect.1 + rect.3 <= 328)
        );
        assert!(ui.controls.iter().any(|(id, _)| *id == 18));
        ui.activate(18);
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        assert_eq!(
            ui.controls
                .iter()
                .filter(|(id, _)| (200..232).contains(id))
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![206, 207, 208]
        );
        ui.station = 5;
        ui.key("Tab");
        assert_eq!(ui.station, 6);
        assert_eq!(ui.station_page, 1);
        ui.station = 8;
        ui.key("Tab");
        assert_eq!(ui.station, 0);
        assert_eq!(ui.station_page, 0);
        ui.activate(16);
        assert_eq!(ui.station, 0);
    }
    /// Nine stations in source order whose lateral offsets run the other
    /// way: hardpoint 4 is the centreline and 7 the internal gun.
    fn crowded() -> Ordnance {
        let mut ui = fixture();
        let station = ui.loadout.configuration.stations[0].clone();
        let mut gun = station.clone();
        gun.internal = true;
        let mut stations = vec![station.clone(); 9];
        stations[7] = gun;
        let mut hardpoints = vec![ui.loadout.hardpoints[0].clone(); 9];
        for (index, hardpoint) in hardpoints.iter_mut().enumerate() {
            hardpoint.position = [
                (90 - 10 * index as i32) * if index % 2 == 0 { 1 } else { -1 },
                0,
                0,
            ];
        }
        hardpoints[4].location = 0;
        hardpoints[4].position = [0; 3];
        hardpoints[7].location = 2;
        ui.loadout.configuration.stations = stations;
        ui.loadout.configuration.hardpoint_slots = (0..9).map(Some).collect();
        ui.loadout.hardpoints = hardpoints;
        ui.loadout.quantities = vec![0; 9];
        ui.station = ui.display_order()[0];
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        ui
    }
    #[test]
    fn stations_list_gun_then_centerline_then_inboard_to_outboard() {
        let mut ui = crowded();
        // The gun, the centreline, then the rest by |x|: 90 - 10 i, so the
        // highest source row is the most inboard, mirrored sides alike.
        assert_eq!(ui.display_order(), [7, 4, 8, 6, 5, 3, 2, 1, 0]);
        // Station numbers are the source order's: the boxes' ids follow it,
        // wherever they are drawn.
        let ids = |ui: &Ordnance| {
            ui.controls
                .iter()
                .filter(|(id, _)| (200..232).contains(id))
                .map(|(id, _)| *id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&ui), [207, 204, 208, 206, 205, 203]);
        assert_eq!(ui.station, 7);
        // Tab walks the page's order and turns the page with it.
        for expected in [4, 8, 6, 5, 3] {
            ui.key("Tab");
            assert_eq!(ui.station, expected);
            assert_eq!(ui.station_page, 0);
        }
        ui.key("Tab");
        assert_eq!((ui.station, ui.station_page), (2, 1));
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        assert_eq!(ids(&ui), [202, 201, 200]);
        for _ in 0..3 {
            ui.key("Tab");
        }
        assert_eq!((ui.station, ui.station_page), (7, 0));
        // Prev and Next pages start on their first listed station.
        ui.activate(18);
        assert_eq!((ui.station, ui.station_page), (2, 1));
        ui.activate(17);
        assert_eq!((ui.station, ui.station_page), (7, 0));
    }
    /// A catalog drop must hit the card the page drew, which follows display
    /// order. Source station 0 is the last card here, on the second page.
    #[test]
    fn a_catalog_drop_empties_the_station_the_card_shows() {
        let mut ui = crowded();
        ui.loadout.quantities[0] = 2;
        let mut pixels = vec![0; WIDTH * HEIGHT * 4];
        let from = source_station_point(&mut ui, 0, &mut pixels).unwrap();
        ui.pointer(Some(from));
        ui.down();
        ui.pointer(Some((290., 370.)));
        ui.up();
        assert_eq!(
            ui.loadout.quantities[0], 0,
            "the drop hit the card at {from:?} but left the source station loaded"
        );
    }
    #[test]
    fn a_click_edits_the_station_the_box_shows_not_its_place_on_the_page() {
        let mut ui = crowded();
        ui.loadout.quantities[4] = 1;
        ui.render(&mut vec![0; WIDTH * HEIGHT * 4]);
        // The second box on the page is source station 4 (the centreline): a
        // click adds one to it and to nothing else.
        drag(&mut ui, (500., 145.), (501., 145.));
        assert_eq!(ui.station, 4);
        assert_eq!(ui.loadout.quantities, [0, 0, 0, 0, 2, 0, 0, 0, 0]);
        // A right click takes one off the same station and no other.
        ui.pointer(Some((500., 145.)));
        ui.right(true);
        ui.right(false);
        assert_eq!(ui.station, 4);
        assert_eq!(ui.loadout.quantities, [0, 0, 0, 0, 1, 0, 0, 0, 0]);
    }
    #[test]
    fn authored_tanks_without_art_use_the_nearest_retail_tank_graphic() {
        use tore_sim::combat::live::TankStore;
        // One picture per retail tank, told apart by width; the bomb picture
        // `$FAB500` is not a tank.
        let sprites: BTreeMap<String, Sprite> = [
            ("$F150.PIC", 15),
            ("$F250.PIC", 25),
            ("$F350.PIC", 35),
            ("$F500.PIC", 50),
            ("$FAB500.PIC", 99),
        ]
        .into_iter()
        .map(|(name, width)| {
            (
                name.to_owned(),
                Sprite {
                    width,
                    height: 1,
                    rgba: vec![0; width * 4],
                    glyphs: vec![],
                },
            )
        })
        .collect();
        let tank = |source: &str| TankStore {
            source: source.into(),
            name: source.into(),
            tank: tore_formats::weapons::Tank {
                empty_weight: 1,
                fuel_weight: 1,
                flags: 1,
            },
        };
        let width = |source: &str| tank_thumbnail(&sprites, &tank(source)).map(|s| s.width);
        // A retail tank keeps its own picture.
        assert_eq!(width("F250.GAS"), Some(25));
        // Every authored tank gets the graphic of the nearest size.
        for (source, expected) in [
            ("F370.GAS", 35),
            ("F610.GAS", 50),
            ("F600.GAS", 50),
            ("M396.GAS", 35),
            ("R528.GAS", 50),
        ] {
            assert_eq!(width(source), Some(expected), "{source}");
        }
        // Each authored record the game generates has one.
        for (name, ..) in tore_formats::aircraft::stations::TANKS {
            assert!(width(name).is_some(), "{name}");
        }
        // A name with no size has none to borrow.
        assert_eq!(width("SYNTHETIC.GAS"), None);
    }
    #[test]
    fn catalog_drag_draws_only_transparent_thumbnail_and_loads_station() {
        let mut ui = fixture();
        let mut before = vec![0; WIDTH * HEIGHT * 4];
        ui.render(&mut before);
        ui.pointer(Some((100., 120.)));
        ui.down();
        ui.pointer(Some((310., 280.)));
        let mut after = before.clone();
        ui.render(&mut after);
        let changed: Vec<_> = before
            .chunks_exact(4)
            .zip(after.chunks_exact(4))
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(changed, vec![280 * WIDTH + 310]);
        assert_eq!(
            &after[changed[0] * 4..changed[0] * 4 + 4],
            &[0, 255, 0, 255]
        );
        ui.pointer(Some((400., 215.)));
        ui.up();
        assert_eq!(ui.loadout.quantities, [2, 4, 500]);
        assert!(ui.drag.is_none());
    }
    #[test]
    fn station_transfer_and_catalog_unload_preserve_other_stations() {
        let mut ui = fixture();
        drag(&mut ui, (500., 145.), (400., 215.));
        assert_eq!(ui.loadout.quantities, [1, 1, 500]);
        // Unused space in the catalog is a valid unload destination.
        drag(&mut ui, (400., 215.), (280., 360.));
        assert_eq!(ui.loadout.quantities, [1, 0, 500]);
        // An occupied catalog card unloads too, without selecting that card.
        drag(&mut ui, (500., 145.), (100., 120.));
        assert_eq!(ui.loadout.quantities, [0, 0, 500]);
        assert!(ui.selected.is_none());
    }
    #[test]
    fn creator_transfer_check_accepts_pair_refusals_and_successful_drags() {
        for (source_pair, target_pair, supply, capacity, expected) in [
            (false, true, 1, 2, [1, 0, 500]), // Su-35's single to a pair.
            (true, false, 2, 1, [2, 0, 500]),
            (false, false, 1, 1, [0, 1, 500]),
            (false, true, 2, 2, [0, 2, 500]),
            (true, false, 4, 3, [2, 2, 500]),
            (true, true, 4, 4, [2, 2, 500]),
        ] {
            let mut ui = fixture();
            for (slot, paired) in [(0, source_pair), (1, target_pair)] {
                let station = &mut ui.loadout.configuration.stations[slot];
                station.rack.location = 4;
                station.rack.paired = paired;
                station.mount = [8., -2., 0.];
            }
            ui.loadout.quantities[0] = supply;
            ui.loadout.hardpoints[1].count = capacity;
            let before = ui.loadout.clone();
            let mut pixels = vec![0; WIDTH * HEIGHT * 4];
            let from = source_station_point(&mut ui, 0, &mut pixels).unwrap();
            let to = source_station_point(&mut ui, 1, &mut pixels).unwrap();
            drag(&mut ui, from, to);
            assert_eq!(ui.loadout.quantities, expected);
            assert_eq!(
                check_station_transfer(&before, &ui.loadout, 0, 1).unwrap(),
                expected[1]
            );
        }
    }

    #[test]
    fn creator_transfer_check_rejects_loss_duplication_wrong_identity_and_unrelated_edits() {
        let before = fixture().loadout;
        let mut after = before.clone();
        after.transfer(0, 1).unwrap();
        assert_eq!(check_station_transfer(&before, &after, 0, 1).unwrap(), 1);
        for quantities in [[0, 1, 500], [2, 1, 500], [1, 1, 499], [2, 0, 500]] {
            let mut bad = after.clone();
            bad.quantities = quantities.to_vec();
            assert!(check_station_transfer(&before, &bad, 0, 1).is_err());
        }
        let mut wrong_weapon = after.clone();
        wrong_weapon.configuration.stations[1].weapon.source = "OTHER.JT".into();
        assert!(check_station_transfer(&before, &wrong_weapon, 0, 1).is_err());
        let mut changed_fuel = after;
        changed_fuel.fuel_lbs -= 1.;
        assert!(check_station_transfer(&before, &changed_fuel, 0, 1).is_err());

        let mut single = before;
        single.quantities[0] = 1;
        single.configuration.stations[1].rack.paired = true;
        single.configuration.stations[1].rack.location = 4;
        single.configuration.stations[1].mount = [8., -2., 0.];
        assert_eq!(check_station_transfer(&single, &single, 0, 1).unwrap(), 0);
        let mut split = single.clone();
        split.quantities = vec![0, 1, 500];
        assert!(check_station_transfer(&single, &split, 0, 1).is_err());
    }
    const AIM9M_WORDS: &str = "Not everyone has the AIM-9M: Hawk's game has no AIM-9M.";
    #[test]
    fn the_lobbys_page_refuses_a_weapon_in_a_gap_and_single_players_never_asks() {
        // Single player: the same gaps change nothing.
        let mut sp = fixture();
        sp.set_gaps(|source| (source == "AIM9M.JT").then(|| AIM9M_WORDS.to_owned()));
        assert_eq!(sp.gap_words("AIM9M.JT"), None);
        drag(&mut sp, (100., 120.), (400., 215.));
        assert_eq!(sp.loadout.quantities, [2, 4, 500]);
        assert!(sp.message.is_none());

        let mut ui = fixture();
        ui.lobby = true;
        let asked = std::cell::RefCell::new(Vec::new());
        ui.set_gaps(|source| {
            asked.borrow_mut().push(source.to_owned());
            (source == "AIM9M.JT").then(|| AIM9M_WORDS.to_owned())
        });
        // It asked about every weapon the page holds.
        assert!(asked.borrow().contains(&"AIM9M.JT".to_owned()));
        assert_eq!(ui.gap_words("AIM9M.JT"), Some(AIM9M_WORDS));
        // Dragging it onto a station is refused, with the host's words, and
        // the loadout stays as it was.
        drag(&mut ui, (100., 120.), (400., 215.));
        assert_eq!(ui.loadout.quantities, [2, 0, 500]);
        assert_eq!(ui.message.as_deref(), Some(AIM9M_WORDS));
        // So is picking it and clicking a station, and picking it says so.
        ui.message = None;
        ui.activate(100);
        assert_eq!(ui.message.as_deref(), Some(AIM9M_WORDS));
        ui.activate(201);
        assert_eq!(ui.loadout.quantities, [2, 0, 500]);
        assert_eq!(ui.message.as_deref(), Some(AIM9M_WORDS));
        // What everyone has loads as before.
        ui.set_gaps(|_| None);
        ui.activate(201);
        assert_eq!(ui.loadout.quantities, [2, 4, 500]);
        assert!(ui.message.is_none());
    }
    #[test]
    fn the_lobbys_page_dims_a_weapon_in_a_gap_in_the_catalog_only() {
        let mut plain = fixture();
        plain.lobby = true;
        let mut dimmed = fixture();
        dimmed.lobby = true;
        dimmed.set_gaps(|source| (source == "AIM9M.JT").then(|| AIM9M_WORDS.to_owned()));
        let (mut before, mut after) =
            (vec![0u8; WIDTH * HEIGHT * 4], vec![0u8; WIDTH * HEIGHT * 4]);
        plain.render(&mut before);
        dimmed.render(&mut after);
        let changed: Vec<usize> = (0..WIDTH * HEIGHT)
            .filter(|i| before[i * 4..i * 4 + 4] != after[i * 4..i * 4 + 4])
            .collect();
        // The card's pixels are darker, and nothing outside the catalog's
        // first card changed (the stations on the right keep their picture).
        assert!(!changed.is_empty());
        for i in &changed {
            let (x, y) = (i % WIDTH, i / WIDTH);
            assert!(x < 190 && (100..180).contains(&y), "{x},{y}");
            assert!(after[i * 4] <= before[i * 4]);
        }
        // Single player's page, given the same gaps, draws as it always did.
        let mut sp = fixture();
        sp.set_gaps(|source| (source == "AIM9M.JT").then(|| AIM9M_WORDS.to_owned()));
        let mut single = vec![0u8; WIDTH * HEIGHT * 4];
        sp.render(&mut single);
        let mut untouched = vec![0u8; WIDTH * HEIGHT * 4];
        fixture().render(&mut untouched);
        assert!(single == untouched, "single player's page changed");
    }
    /// The pixels a notice changes, as the rows they span.
    fn notice_rows(lobby: bool) -> Option<(usize, usize)> {
        let width = crate::menu::WIDTH;
        let mut bare = fixture();
        bare.lobby = lobby;
        let mut said = fixture();
        said.lobby = lobby;
        said.message = Some(LOBBY_CHEAT_NOTICE.into());
        let mut plain = vec![0u8; width * crate::menu::HEIGHT * 4];
        let mut with = plain.clone();
        bare.render(&mut plain);
        said.render(&mut with);
        let rows: Vec<usize> = (0..crate::menu::HEIGHT)
            .filter(|row| {
                plain[row * width * 4..(row + 1) * width * 4]
                    != with[row * width * 4..(row + 1) * width * 4]
            })
            .collect();
        Some((*rows.first()?, *rows.last()?))
    }
    #[test]
    fn the_lobbys_notice_sits_above_the_panels_and_single_players_stays_low() {
        // Single player's notice is where it always was (over the catalog's
        // bottom edge).
        let (top, bottom) = notice_rows(false).expect("single player's notice draws");
        assert!(
            top >= NOTICE_Y as usize && bottom < NOTICE_Y as usize + 20,
            "{top} {bottom}"
        );
        // The lobby's is in the strip between the title bar and the panels,
        // clear of every weapon label.
        let (top, bottom) = notice_rows(true).expect("the lobby's notice draws");
        assert!(
            top >= LOBBY_NOTICE_Y as usize && bottom < 86,
            "{top} {bottom}"
        );
    }
    #[test]
    fn the_lobbys_page_refuses_cheat_loading_and_still_answers_fly_with_accept() {
        let mut ui = fixture();
        ui.lobby = true;
        // The Cheat row of the page's menu is not offered to a multiplayer
        // game: the notice says so and nothing is unloaded or turned on.
        let before = ui.loadout.quantities.clone();
        ui.activate(11);
        ui.activate(13);
        assert!(!ui.loadout.cheat);
        assert_eq!(ui.loadout.quantities, before);
        assert_eq!(ui.message.as_deref(), Some(LOBBY_CHEAT_NOTICE));
        // Accept is the page's Fly button; Cancel is Select Plane.
        assert_eq!(ui.activate(7), Action::MissionFly);
        ui.activate(8);
        assert!(!ui.visible);
        // Single player's page still toggles Cheat.
        let mut sp = fixture();
        sp.activate(13);
        assert!(sp.loadout.cheat);
    }
    #[test]
    fn the_lobbys_page_allows_cheat_loading_under_the_kings_any_rule_and_drops_it_under_own() {
        let mut ui = fixture();
        ui.lobby = true;
        // `loadouts any`: the Cheat row works as single player's does, and
        // its menu no longer says "not allowed".
        assert!(!ui.set_cheat_rule(true));
        assert!(ui.cheat_allowed());
        ui.activate(11);
        ui.activate(13);
        assert!(ui.loadout.cheat);
        assert_eq!(ui.message.as_deref(), Some("Cheat loading on."));
        // The King turns the rule back to `own`: the page unloads the Cheat
        // loading and turns it off, and refuses it again.
        ui.loadout.quantities = vec![1, 1, 1];
        assert!(ui.set_cheat_rule(false));
        assert!(!ui.loadout.cheat);
        assert!(ui.loadout.quantities.iter().all(|n| *n == 0));
        assert!(!ui.cheat_allowed());
        ui.activate(11);
        ui.activate(13);
        assert!(!ui.loadout.cheat);
        assert_eq!(ui.message.as_deref(), Some(LOBBY_CHEAT_NOTICE));
        // Setting the rule on a page with no Cheat loading changes nothing.
        assert!(!ui.set_cheat_rule(false));
        // Single player's page is not a lobby page: it always allows Cheat.
        assert!(fixture().cheat_allowed());
    }
    #[test]
    fn normal_catalog_hides_unimplemented_weapons_even_when_they_fit() {
        let mut ui = fixture();
        let mut missile = ui.weapons[0].clone();
        missile.source = "AIM120.JT".into();
        missile.flags = 0x10003;
        missile.seeker.signature = 3;
        ui.weapons.push(missile.clone());
        missile.source = "UNREVIEWED.JT".into();
        assert_eq!(ui.loadout.capacity(0, &missile), 4);
        ui.weapons.push(missile);
        ui.weapons
            .push(ui.loadout.configuration.stations[2].weapon.clone());
        ui.rebuild_catalog();
        assert_eq!(
            ui.catalog
                .iter()
                .map(|w| w.source.as_str())
                .collect::<Vec<_>>(),
            ["AIM9M.JT", "AIM120.JT", "M61.JT"]
        );
        // A compatible alternative can be selected, loaded and flown.
        ui.activate(101);
        ui.activate(201);
        assert_eq!(
            ui.loadout.configuration.stations[1].weapon.source,
            "AIM120.JT"
        );
        assert_eq!(ui.loadout.quantities[1], 4);
        assert_eq!(ui.activate(7), Action::MissionFly);
    }
    #[test]
    fn cheat_button_unloads_and_toggles_any_store_on_any_station() {
        let mut ui = fixture();
        let mut bomb = ui.weapons[0].clone();
        bomb.source = "MK82.JT".into();
        bomb.flags = 2;
        bomb.seeker.signature = 0;
        ui.weapons.push(bomb.clone());
        let mut unsupported = bomb.clone();
        unsupported.source = "MOTHB.JT".into();
        ui.weapons.push(unsupported.clone());
        let mut gun = bomb.clone();
        gun.source = "DEFA.JT".into();
        gun.flags = 0x80;
        ui.weapons.push(gun.clone());
        ui.rebuild_catalog();
        let names = |ui: &Ordnance| {
            ui.catalog
                .iter()
                .map(|w| w.source.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&ui), ["AIM9M.JT"]);
        ui.activate(13);
        assert!(ui.loadout.cheat);
        assert_eq!(ui.loadout.quantities, [0, 0, 0]);
        assert_eq!(names(&ui), ["AIM9M.JT", "MK82.JT", "DEFA.JT"]);
        // Even a cheat-compatible unimplemented weapon stays hidden.
        assert_eq!(ui.loadout.capacity(0, &unsupported), 4);
        // Cheat lists all supported weapons, including other aircraft's guns.
        assert!((0..3).all(|i| ui.loadout.capacity(i, &gun) == 0));
        assert_eq!(ui.loadout.capacity(0, &bomb), 4);
        assert_eq!(ui.loadout.capacity(2, &bomb), 0, "the fixed gun station");
        ui.activate(4);
        ui.activate(100);
        ui.activate(200);
        assert_eq!(
            ui.loadout.configuration.stations[0].weapon.source,
            "MK82.JT"
        );
        assert_eq!(ui.loadout.quantities, [4, 0, 0]);
        assert_eq!(ui.activate(7), Action::MissionFly);
        ui.pages = [1, 2];
        ui.activate(13);
        assert!(!ui.loadout.cheat);
        assert_eq!(ui.loadout.quantities, [0, 0, 0]);
        assert_eq!(names(&ui), ["AIM9M.JT"]);
        assert_eq!(ui.pages, [0; 2]);
        assert!(ui.selected.is_none());
    }
    #[test]
    fn filtered_empty_catalog_has_no_selectable_cards_or_extra_pages() {
        let mut ui = fixture();
        ui.weapons[0].source = "UNREVIEWED.JT".into();
        for cheat in [false, true] {
            ui.loadout.cheat = cheat;
            ui.rebuild_catalog();
            assert!(ui.catalog.is_empty());
            for category in [3, 4] {
                ui.activate(category);
                ui.activate(2);
                ui.activate(100);
                assert_eq!(ui.pages, [0; 2]);
                assert!(ui.selected.is_none());
                let mut pixels = vec![0; WIDTH * HEIGHT * 4];
                ui.render(&mut pixels);
                assert!(!ui.controls.iter().any(|(id, _)| (100..108).contains(id)));
            }
        }
    }
    #[test]
    fn invalid_same_station_outside_and_cancelled_drops_do_not_mutate_loads() {
        let mut ui = fixture();
        drag(&mut ui, (500., 145.), (400., 145.));
        assert!(ui.message.is_some());
        assert_eq!(ui.loadout.quantities, [2, 0, 500]);
        for destination in [(510., 145.), (600., 300.), (520., 430.)] {
            drag(&mut ui, (500., 145.), destination);
            assert_eq!(ui.loadout.quantities, [2, 0, 500]);
        }
        for cancel in 0..3 {
            ui.pointer(Some((500., 145.)));
            ui.down();
            ui.pointer(Some((100., 120.)));
            match cancel {
                0 => ui.cancel(),
                1 => {
                    ui.key("Escape");
                }
                _ => ui.pointer(None),
            }
            ui.pointer(Some((100., 120.)));
            ui.up();
            assert_eq!(ui.loadout.quantities, [2, 0, 500]);
            assert!(ui.visible);
        }
    }
    #[test]
    fn empty_station_has_only_red_card_outline_below_location_heading() {
        let mut ui = fixture();
        ui.loadout.quantities.fill(0);
        let mut pixels = vec![0; WIDTH * HEIGHT * 4];
        ui.render(&mut pixels);
        for (x, y) in [(353, 135), (472, 135), (353, 206)] {
            for yy in y - 1..y + 51 {
                for xx in x..x + 109 {
                    let edge =
                        yy <= y + 21 && (yy == y - 1 || yy == y + 21 || xx == x || xx == x + 108);
                    let expected = if edge {
                        [94, 30, 20, 255]
                    } else {
                        [17, 19, 23, 255]
                    };
                    let at = (yy * WIDTH + xx) * 4;
                    assert_eq!(pixels[at..at + 4], expected, "pixel {xx},{yy}");
                }
            }
        }
    }
    #[test]
    fn click_loading_and_right_click_decrement_still_work() {
        let mut ui = fixture();
        drag(&mut ui, (100., 120.), (101., 120.));
        assert_eq!(ui.selected, Some(CatalogEntry::Weapon(0)));
        drag(&mut ui, (400., 215.), (400., 215.));
        assert_eq!(ui.loadout.quantities, [2, 4, 500]);
        ui.right(true);
        ui.right(false);
        assert_eq!(ui.loadout.quantities, [2, 3, 500]);
    }
    #[test]
    fn taking_every_store_off_by_any_route_leaves_zeros_that_still_validate() {
        // The minus key on each station until it stops changing.
        let mut ui = fixture();
        for station in 0..ui.loadout.quantities.len() {
            ui.station = station;
            for _ in 0..40 {
                ui.key("-");
            }
        }
        assert_eq!(ui.loadout.quantities, [0, 0, 0]);
        ui.loadout.validate().unwrap();
        // Right-click on each station card.
        let mut ui = fixture();
        for station in 0..3 {
            for _ in 0..40 {
                ui.loadout.change(station, -1);
            }
        }
        assert_eq!(ui.loadout.quantities, [0, 0, 0]);
        // The menu's unload-everything button.
        let mut ui = fixture();
        ui.activate(12);
        assert_eq!(ui.loadout.quantities, [0, 0, 0]);
        ui.loadout.validate().unwrap();
        // Emptied stations keep their store type (the page shows an empty
        // box) but weigh and count nothing.
        assert_eq!(
            ui.loadout.configuration.stations[0].weapon.source,
            "AIM9M.JT"
        );
        assert_eq!(
            ui.loadout.total_lbs(),
            ui.loadout.empty_lbs + ui.loadout.fuel_lbs
        );
    }
    #[test]
    fn loaded_station_click_adds_exactly_one_without_replacing_or_overfilling() {
        let mut ui = fixture();
        let mut other = ui.catalog[0].clone();
        other.source = "AIM120.JT".into();
        ui.catalog.push(other);
        ui.selected = Some(CatalogEntry::Weapon(1));
        drag(&mut ui, (500., 145.), (501., 145.));
        assert_eq!(ui.loadout.quantities, [3, 0, 500]);
        assert_eq!(
            ui.loadout.configuration.stations[0].weapon.source,
            "AIM9M.JT"
        );
        drag(&mut ui, (500., 145.), (500., 145.));
        drag(&mut ui, (500., 145.), (500., 145.));
        assert_eq!(ui.loadout.quantities, [4, 0, 500]);
        // Click adds one even where keyboard/right-click steps are 100 rounds.
        ui.loadout.quantities[2] = 498;
        drag(&mut ui, (400., 145.), (400., 145.));
        assert_eq!(ui.loadout.quantities, [4, 0, 499]);
    }
    #[test]
    fn loaded_station_click_adds_a_whole_pair_on_every_two_sided_carrier() {
        for location in [
            4,
            5,
            1,
            tore_sim::combat::hardpoints::TUNNEL_BYTE,
            tore_sim::combat::hardpoints::MAGAZINE_BYTE,
        ] {
            let mut ui = fixture();
            let station = &mut ui.loadout.configuration.stations[0];
            station.rack = tore_sim::combat::hardpoints::Rack {
                location,
                paired: true,
                ..Default::default()
            };
            station.mount = [8., -2., 0.];
            assert_eq!(ui.loadout.quantities, [2, 0, 500]);
            drag(&mut ui, (500., 145.), (500., 145.));
            assert_eq!(ui.loadout.quantities, [4, 0, 500]);
            drag(&mut ui, (500., 145.), (500., 145.));
            assert_eq!(ui.loadout.quantities, [4, 0, 500], "never past capacity");
            ui.loadout.validate().unwrap();
        }
    }
    #[test]
    fn a_saved_single_restores_as_an_overweight_pair_until_the_player_adjusts_it() {
        let mut ui = fixture();
        let station = &mut ui.loadout.configuration.stations[0];
        station.rack.location = 1;
        station.rack.paired = true;
        station.mount = [3., -2., 0.];
        ui.loadout.quantities[0] = 1;
        let weight = f64::from(station.weapon.weight);
        assert!(weight > 0.);
        ui.loadout.maximum_lbs = ui.loadout.total_lbs() + weight / 2.;
        let saved = tore_world::mission::LoadoutSpec::of(&ui.loadout);
        let restored = saved
            .apply(ui.loadout.clone(), &BTreeMap::new(), None)
            .unwrap();
        restored.validate_for_editing().unwrap();
        assert_eq!(restored.quantities[0], 2);
        assert!(restored.total_lbs() > restored.maximum_lbs);
        ui.loadout = restored;
        assert_eq!(ui.activate(7), Action::Click);
        assert_eq!(
            ui.message.as_deref(),
            Some("This plane is too heavy to take off. Remove stores or fuel.")
        );
        ui.loadout.change(0, -1);
        assert_eq!(ui.activate(7), Action::MissionFly);
    }
    #[test]
    fn ordnance_sounds_follow_successful_edits_and_not_failed_or_empty_actions() {
        let mut ui = fixture();
        assert_eq!(
            drag(&mut ui, (500., 145.), (500., 145.)),
            Action::OrdnanceWeapon
        );
        ui.right(true);
        assert_eq!(ui.right(false), Action::OrdnanceWeapon);
        assert_eq!(ui.key("+"), Action::OrdnanceWeapon);
        assert_eq!(
            drag(&mut ui, (100., 120.), (400., 215.)),
            Action::OrdnanceWeapon
        );
        assert_eq!(drag(&mut ui, (500., 145.), (400., 215.)), Action::None);
        ui.loadout.quantities[1] = 0;
        assert_eq!(
            drag(&mut ui, (500., 145.), (400., 215.)),
            Action::OrdnanceWeapon
        );
        assert_eq!(
            drag(&mut ui, (400., 215.), (100., 120.)),
            Action::OrdnanceWeapon
        );
        assert_eq!(drag(&mut ui, (500., 145.), (400., 145.)), Action::None);
        assert!(ui.message.is_some());
        assert_eq!(drag(&mut ui, (500., 145.), (510., 145.)), Action::None);
        ui.loadout.quantities[2] = 499;
        assert_eq!(
            drag(&mut ui, (400., 145.), (400., 145.)),
            Action::OrdnanceAmmunition
        );
        assert_eq!(drag(&mut ui, (400., 145.), (400., 145.)), Action::None);
        assert_eq!(
            drag(&mut ui, (400., 145.), (100., 120.)),
            Action::OrdnanceAmmunition
        );
        ui.pointer(Some((400., 145.)));
        ui.right(true);
        assert_eq!(ui.right(false), Action::None);
        // The imported flag chooses the sound, including ammunition in a pod.
        ui.loadout.configuration.stations[0].weapon.flags |= 0x80;
        assert_eq!(
            drag(&mut ui, (500., 145.), (500., 145.)),
            Action::OrdnanceAmmunition
        );
    }
    #[test]
    fn fuel_sound_requires_an_actual_fuel_change() {
        let mut ui = fixture();
        assert_eq!(ui.activate(5), Action::None);
        assert_eq!(ui.activate(6), Action::OrdnanceFuel);
        assert_eq!(ui.activate(6), Action::OrdnanceFuel);
        assert_eq!(ui.activate(6), Action::None);
        assert_eq!(ui.activate(5), Action::OrdnanceFuel);
    }
    #[test]
    fn rockers_act_on_press_and_spring_back_on_release() {
        let mut ui = fixture();
        // The fuel rocker's bottom half removes fuel as soon as it is pressed.
        let fuel = ui.loadout.fuel_lbs;
        ui.pointer(Some((550., 380.)));
        assert_eq!(ui.down(), Action::OrdnanceFuel);
        assert!(ui.loadout.fuel_lbs < fuel);
        assert!(ui.rockers[1].held());
        assert_eq!(ui.up(), Action::RockerUp);
        assert!(!ui.rockers[1].held());
        assert!(ui.loadout.fuel_lbs < fuel);
        // The page rocker sounds its own press, even at the first page.
        ui.pointer(Some((265., 410.)));
        assert_eq!(ui.down(), Action::RockerDown);
        ui.pointer(Some((300., 300.)));
        assert_eq!(ui.up(), Action::RockerUp);
        assert_eq!(ui.pages, [0, 0]);
    }
    #[test]
    fn novelty_bombs_load_only_under_cheat_loading() {
        let mut ui = fixture();
        let mut bomb = ui.weapons[0].clone();
        bomb.source = "MK82.JT".into();
        bomb.flags = 0x22012;
        bomb.seeker.signature = 0;
        ui.weapons.push(bomb.clone());
        for name in ["~BLDR.JT", "~MOOSE.JT"] {
            let mut novelty = bomb.clone();
            novelty.source = name.into();
            ui.weapons.push(novelty);
        }
        let names = |ui: &Ordnance| {
            ui.catalog
                .iter()
                .map(|w| w.source.clone())
                .collect::<Vec<_>>()
        };
        // Normal loading: no station gives a novelty bomb capacity, so the
        // catalog never lists it, a selection is refused and a loadout that
        // carries one is invalid.
        ui.rebuild_catalog();
        assert!(
            names(&ui).iter().all(|n| !n.starts_with('~')),
            "{:?}",
            names(&ui)
        );
        let boulder = ui
            .weapons
            .iter()
            .find(|w| w.source == "~BLDR.JT")
            .unwrap()
            .clone();
        assert_eq!(ui.loadout.capacity(0, &boulder), 0);
        assert!(ui.loadout.clone().select(0, boulder.clone()).is_err());
        let mut carrying = ui.loadout.clone();
        carrying.cheat = true;
        carrying.select(0, boulder.clone()).unwrap();
        carrying.cheat = false;
        let error = carrying.validate().unwrap_err().to_string();
        assert!(error.contains("Cheat loading only"), "{error}");
        // Cheat loading lists both, with the other supported bombs.
        ui.activate(13);
        assert!(ui.loadout.cheat);
        let listed = names(&ui);
        assert!(listed.contains(&"~BLDR.JT".to_string()), "{listed:?}");
        assert!(listed.contains(&"~MOOSE.JT".to_string()), "{listed:?}");
        assert!(listed.contains(&"MK82.JT".to_string()), "{listed:?}");
        ui.loadout.select(0, boulder).unwrap();
        ui.loadout.validate().unwrap();
        // Switching Cheat off unloads every station and hides them again.
        ui.activate(13);
        assert!(!ui.loadout.cheat);
        assert_eq!(ui.loadout.quantities, [0, 0, 0]);
        ui.loadout.validate().unwrap();
        assert!(names(&ui).iter().all(|n| !n.starts_with('~')));
        // A record that is not Boulder or Moose never gets in: the other
        // novelty records stay out of the page's weapon list.
        assert!(!ui.weapons.iter().any(|w| w.source == "~VOMIT.JT"));
    }
    #[test]
    fn the_card_names_the_seeker_from_the_retail_list() {
        let mut w = fixture().weapons[0].clone();
        let mut word = |flags: u32, signature: u8| {
            w.flags = flags;
            w.seeker.signature = signature;
            guidance(&w)
        };
        // No guided flag: every bomb, rocket and gun.
        assert_eq!(word(0x22012, 0), "unguided");
        assert_eq!(word(0x82a052, 0), "unguided");
        assert_eq!(word(0x20846, 0), "unguided");
        // Guided signature 0 is "optical": the JDAMs (0x2a043) and the AT-2.
        assert_eq!(word(0x2a043, 0), "optical");
        assert_eq!(word(0x2216f, 0), "optical");
        assert_eq!(word(0x2a543, 1), "laser");
        assert_eq!(word(0x2a06f, 2), "IR");
        assert_eq!(word(0x2236f, 3), "SARH");
        assert_eq!(word(0x2a06f, 3), "active rdr");
        assert_eq!(word(0x2204f, 4), "passive rdr");
    }
    fn with_pod_station() -> Ordnance {
        let mut ui = fixture();
        // Station 1 hangs a pod: the retail pod-station flags and weight class.
        ui.loadout.hardpoints[1].flags = 0x1785;
        ui.loadout.hardpoints[1].weight_class = 32;
        ui.designators = vec![DesignatorStore {
            source: "PAVESPK.SEE".into(),
            name: "Pave Spike".into(),
            sensor: tore_sim::combat::sight::SightSensor {
                laser: true,
                range_ft: 60_760.,
                gimbal: tore_sim::combat::sight::Gimbal::Hemisphere,
                eye: [0., -5., 0.],
                default_look: [0., -0.17],
            },
            weight_lbs: 425,
            flags: 1,
        }];
        ui.rebuild_catalog();
        ui
    }
    #[test]
    fn a_designator_pod_is_listed_loaded_moved_and_taken_off_like_any_store() {
        let mut ui = with_pod_station();
        assert_eq!(ui.designator_catalog.len(), 1);
        // The air-to-surface page lists the pod after the tanks and before the
        // weapons; the air-to-air page lists none.
        ui.activate(4);
        assert_eq!(ui.page_entries()[0], CatalogEntry::Designator(0));
        ui.activate(3);
        assert!(
            ui.page_entries()
                .iter()
                .all(|entry| !matches!(entry, CatalogEntry::Designator(_)))
        );
        ui.activate(4);
        // Click the card and a station that takes it: the pod replaces the
        // weapon, which is unloaded, and the station shows the pod.
        ui.activate(100);
        assert_eq!(ui.selected, Some(CatalogEntry::Designator(0)));
        // Station 0 is an air-to-air rail: it cannot hang a pod.
        assert_eq!(ui.load_selected(0), Action::None);
        assert!(ui.loadout.designator(0).is_none());
        assert_eq!(ui.activate(201), Action::OrdnanceWeapon);
        assert!(ui.loadout.designator(1).is_some());
        assert_eq!(ui.loadout.quantities[1], 0);
        let row = ui.station_row(1).unwrap();
        assert_eq!(ui.loaded_designator(&row), Some(1));
        // One pod to an aircraft and nothing to add to it: a click does
        // nothing, and the card page can still be drawn.
        assert_eq!(ui.activate(201), Action::None);
        assert_eq!(ui.loadout.configuration.designator_pods.len(), 1);
        let mut pixels = vec![0; crate::menu::WIDTH * crate::menu::HEIGHT * 4];
        ui.render(&mut pixels);
        // Right-click takes it off; the weapon's quantity stays unloaded.
        ui.station = 1;
        assert_eq!(ui.change_station(1, -1, false), Action::OrdnanceWeapon);
        assert!(ui.loadout.designator(1).is_none());
        assert_eq!(ui.loadout.quantities[1], 0);
        // A weapon taking the station back removes the pod.
        ui.activate(101);
        ui.loadout
            .load_designator(1, ui.designators[0].clone())
            .unwrap();
        ui.loadout.quantities[1] = 1;
        ui.loadout.refresh_equipment().unwrap();
        assert!(ui.loadout.designator(1).is_none());
    }
    #[test]
    fn a_designator_pod_drags_between_stations_and_off_to_the_catalog() {
        let mut ui = with_pod_station();
        ui.activate(4);
        // A second pod station (flags and weight class as the first).
        ui.loadout.hardpoints[0].flags = 0x1785;
        ui.loadout.hardpoints[0].weight_class = 32;
        ui.drop_store(DragSource::DesignatorCatalog(0), 1);
        assert!(ui.loadout.designator(1).is_some());
        // Dragged to the other pod station, the aircraft's one pod moves.
        assert_eq!(
            ui.drop_store(DragSource::DesignatorStation(1), 0),
            Action::OrdnanceWeapon
        );
        assert!(ui.loadout.designator(0).is_some() && ui.loadout.designator(1).is_none());
        // Dragged to a station that cannot take it, nothing changes.
        assert_eq!(
            ui.drop_store(DragSource::DesignatorStation(0), 2),
            Action::None
        );
        assert!(ui.loadout.designator(0).is_some());
        assert!(
            ui.drag_thumbnail(DragSource::DesignatorStation(0))
                .is_none(),
            "no art in the fixture"
        );
        // Unload All and the Cheat toggle take the pod off.
        ui.activate(12);
        assert!(ui.loadout.configuration.designator_pods.is_empty());
        ui.drop_store(DragSource::DesignatorCatalog(0), 1);
        ui.activate(13);
        assert!(ui.loadout.configuration.designator_pods.is_empty());
        // Cheat loading lists the pod whatever the aircraft's stations.
        assert_eq!(ui.designator_catalog.len(), 1);
    }
    /// Protocol 23 (slice NA): the lobby's page offers the pods, a pod in a
    /// gap is dimmed and refused with the host's words, and the loadout the
    /// page sends names the pod's SEE record on its station.
    #[test]
    fn the_lobbys_page_offers_designator_pods_and_refuses_one_in_a_gap() {
        const WORDS: &str = "Not everyone has the Pave Spike: Hawk's game has no Pave Spike.";
        let mut ui = with_pod_station();
        ui.lobby = true;
        ui.activate(4);
        assert_eq!(ui.page_entries()[0], CatalogEntry::Designator(0));
        let asked = std::cell::RefCell::new(Vec::new());
        ui.set_gaps(|source| {
            asked.borrow_mut().push(source.to_owned());
            (source == "PAVESPK.SEE").then(|| WORDS.to_owned())
        });
        assert!(asked.borrow().contains(&"PAVESPK.SEE".to_owned()));
        // Picking it says why; loading it, by a click or a drag, is refused.
        ui.activate(100);
        assert_eq!(ui.message.as_deref(), Some(WORDS));
        ui.activate(201);
        assert!(ui.loadout.designator(1).is_none());
        ui.message = None;
        ui.drop_store(DragSource::DesignatorCatalog(0), 1);
        assert!(ui.loadout.designator(1).is_none());
        assert_eq!(ui.message.as_deref(), Some(WORDS));
        // Everyone has it: it hangs, and the loadout names it.
        ui.set_gaps(|_| None);
        ui.message = None;
        assert_eq!(ui.activate(201), Action::OrdnanceWeapon);
        assert!(ui.loadout.designator(1).is_some());
        let spec = tore_world::mission::LoadoutSpec::of(&ui.loadout);
        assert_eq!(spec.stations[1].weapon, "PAVESPK.SEE");
        assert_eq!(spec.stations[1].quantity, 0);
    }

    // Slice BQ, B24: the laser warning and the sensor helper line.
    fn laser_fixture() -> Ordnance {
        let mut ui = fixture();
        // Station 0 holds two laser bombs: signature 1 and the designator flag.
        let weapon = &mut ui.loadout.configuration.stations[0].weapon;
        weapon.source = "GBU10.JT".into();
        weapon.seeker.signature = 1;
        weapon.flags |= 0x400 | 1;
        ui.loadout.configuration.sight = None;
        ui.loadout.quantities[0] = 2;
        ui
    }
    fn sensor(
        laser: bool,
        gimbal: tore_sim::combat::sight::Gimbal,
    ) -> tore_sim::combat::sight::SightSensor {
        tore_sim::combat::sight::SightSensor {
            laser,
            range_ft: 60_760.,
            gimbal,
            eye: [0.; 3],
            default_look: [0., -0.17],
        }
    }
    const CONE: tore_sim::combat::sight::Gimbal = tore_sim::combat::sight::Gimbal::Cone {
        search: 1.0,
        track: 0.8,
    };
    #[test]
    fn laser_ordnance_with_nothing_to_lase_it_warns() {
        let mut ui = laser_fixture();
        assert_eq!(laser_warning(&ui.loadout), Some(NO_LASER_WARNING));
        // Unloaded, the bomb warns of nothing.
        ui.loadout.quantities[0] = 0;
        assert_eq!(laser_warning(&ui.loadout), None);
        // A weapon that is not a laser one never does: signature 1 without the
        // designator flag needs no designator, and another seeker with the
        // flag is not a laser seeker.
        let mut other = laser_fixture();
        other.loadout.configuration.stations[0].weapon.flags &= !0x400;
        assert_eq!(laser_warning(&other.loadout), None);
        let mut other = laser_fixture();
        other.loadout.configuration.stations[0]
            .weapon
            .seeker
            .signature = 2;
        assert_eq!(laser_warning(&other.loadout), None);
    }
    #[test]
    fn a_laser_on_the_aircraft_or_a_pod_on_any_station_silences_the_warning() {
        use tore_sim::combat::sight::Gimbal;
        // A built-in laser, or a fitted laser pod (the F-16C and A-10 rows).
        for gimbal in [CONE, Gimbal::Hemisphere] {
            let mut ui = laser_fixture();
            ui.loadout.configuration.sight = Some(sensor(true, gimbal));
            assert_eq!(laser_warning(&ui.loadout), None);
        }
        // A FLIR sees but does not lase.
        let mut ui = laser_fixture();
        ui.loadout.configuration.sight = Some(sensor(false, Gimbal::Hemisphere));
        assert_eq!(laser_warning(&ui.loadout), Some(NO_LASER_WARNING));
        // A pod hung on another station counts, and takes the warning back
        // when it comes off.
        let mut ui = with_pod_station();
        let weapon = &mut ui.loadout.configuration.stations[0].weapon;
        weapon.seeker.signature = 1;
        weapon.flags |= 0x400 | 1;
        ui.loadout.quantities[0] = 2;
        assert_eq!(laser_warning(&ui.loadout), Some(NO_LASER_WARNING));
        ui.activate(4);
        ui.activate(100);
        assert_eq!(ui.activate(201), Action::OrdnanceWeapon);
        assert!(ui.loadout.designator(1).is_some());
        assert_eq!(laser_warning(&ui.loadout), None);
        ui.station = 1;
        ui.change_station(1, -1, false);
        assert_eq!(laser_warning(&ui.loadout), Some(NO_LASER_WARNING));
    }
    #[test]
    fn the_helper_line_says_what_the_aircraft_brings_from_its_data() {
        use tore_sim::combat::sight::Gimbal;
        let mut ui = fixture();
        ui.loadout.configuration.sight = None;
        assert_eq!(sensor_helper(&ui.loadout, false), None);
        assert_eq!(
            sensor_helper(&ui.loadout, true).as_deref(),
            Some("No built-in designator: hang a Pave Spike pod to lase.")
        );
        for (laser, gimbal, words) in [
            (
                true,
                CONE,
                "Built-in laser designator: no pod needed for laser ordnance.",
            ),
            (false, CONE, "Built-in FLIR: no pod needed for IR weapons."),
            (true, Gimbal::Hemisphere, "Aircraft laser pod equipped"),
            (false, Gimbal::Hemisphere, "Aircraft FLIR pod equipped"),
        ] {
            ui.loadout.configuration.sight = Some(sensor(laser, gimbal));
            // Pods on offer or not, a sensor the aircraft has is what is said.
            for pods in [false, true] {
                assert_eq!(sensor_helper(&ui.loadout, pods).as_deref(), Some(words));
            }
        }
    }
    /// The first and last row of `ink` in the page's pixels, and whether any
    /// of it is there.
    fn ink_rows(pixels: &[u8], ink: [u8; 3]) -> Option<(usize, usize)> {
        // A tint is multiplied by the font's own colour (200 in the fixture).
        let ink = ink.map(|channel| ((u32::from(channel) * 200 + 127) / 255) as u8);
        let rows: Vec<usize> = (0..HEIGHT)
            .filter(|row| {
                pixels[row * WIDTH * 4..(row + 1) * WIDTH * 4]
                    .chunks_exact(4)
                    .any(|p| p[..3] == ink)
            })
            .collect();
        Some((*rows.first()?, *rows.last()?))
    }
    fn drawn(ui: &mut Ordnance) -> Vec<u8> {
        let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
        ui.render(&mut pixels);
        pixels
    }
    #[test]
    fn the_warning_takes_the_message_band_and_a_message_takes_it_back() {
        let mut ui = laser_fixture();
        let pixels = drawn(&mut ui);
        let (top, bottom) = ink_rows(&pixels, WARNING_INK).expect("the warning draws");
        assert!(
            top >= NOTICE_Y as usize && bottom < NOTICE_Y as usize + 20,
            "{top} {bottom}"
        );
        // A message the player just earned shows instead, never both.
        ui.message = Some(LOBBY_CHEAT_NOTICE.into());
        let pixels = drawn(&mut ui);
        assert_eq!(ink_rows(&pixels, WARNING_INK), None);
        // No laser bomb, no warning.
        let mut quiet = fixture();
        assert_eq!(ink_rows(&drawn(&mut quiet), WARNING_INK), None);
        // The player can still start with it up.
        // The warning never refuses Fly: the loadout's own validation is the
        // only thing that can, and it does not look at the designator.
        let mut ui = laser_fixture();
        let valid = ui.loadout.validate().is_ok();
        let action = ui.activate(7);
        assert_eq!(action == Action::MissionFly, valid);
        assert_ne!(ui.message.as_deref(), Some(NO_LASER_WARNING));
        ui.loadout.configuration.designator_pods.clear();
        assert_eq!(ui.loadout.validate().is_ok(), valid);
    }
    #[test]
    fn the_helper_sits_below_the_buttons_in_both_pages_and_beside_any_message() {
        use tore_sim::combat::sight::Gimbal;
        let mut ui = fixture();
        ui.loadout.configuration.sight = Some(sensor(false, CONE));
        let (top, bottom) = ink_rows(&drawn(&mut ui), HELPER_INK).expect("the helper draws");
        // Under the Select Plane and Fly buttons (they end at row 438) and on
        // the screen, not in the strip under the title bar.
        assert!(
            top >= HELPER_Y as usize + 2 && bottom < HEIGHT && top > 438,
            "{top} {bottom}"
        );
        // Single player: the warning low on the left, the helper below the
        // panel, both at once.
        let mut both = laser_fixture();
        both.loadout.configuration.sight = Some(sensor(false, Gimbal::Hemisphere));
        let pixels = drawn(&mut both);
        assert!(ink_rows(&pixels, WARNING_INK).is_some());
        assert!(ink_rows(&pixels, HELPER_INK).is_some());
        // The lobby's warning keeps the strip under the title bar and the
        // helper no longer gives way to it.
        both.lobby = true;
        let pixels = drawn(&mut both);
        let (top, bottom) = ink_rows(&pixels, WARNING_INK).expect("the lobby's warning draws");
        assert!(
            top >= LOBBY_NOTICE_Y as usize && bottom < 86,
            "{top} {bottom}"
        );
        let (top, _) = ink_rows(&pixels, HELPER_INK).expect("the helper stays");
        assert!(top > 438);
        both.loadout.quantities[0] = 0;
        let pixels = drawn(&mut both);
        assert_eq!(ink_rows(&pixels, WARNING_INK), None);
        assert!(ink_rows(&pixels, HELPER_INK).is_some());
    }
    /// The widest helper line never leaves the strip's bounds or the screen.
    #[test]
    fn the_longest_helper_line_stays_between_the_panels_and_the_screen_edge() {
        let ui = fixture();
        let font = &ui.sprites["QUICKFONT"];
        let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
        let mut canvas = Canvas(&mut pixels);
        helper_band(
            &mut canvas,
            font,
            "No built-in designator: hang a Pave Spike pod to lase.",
        );
        let strip: Vec<usize> = (0..WIDTH)
            .filter(|x| {
                (HELPER_Y as usize..HELPER_Y as usize + 15)
                    .any(|y| pixels[(y * WIDTH + x) * 4 + 3] != 0)
            })
            .collect();
        assert!(*strip.first().unwrap() >= HELPER_LEFT as usize);
        assert!(*strip.last().unwrap() < HELPER_RIGHT as usize);
    }
    #[test]
    fn a_pod_station_blinks_red_then_yellow_twice_a_second() {
        // Each colour 250 ms, red first: one full cycle every 500 ms.
        for (ms, yellow) in [
            (0, false),
            (249, false),
            (250, true),
            (499, true),
            (500, false),
            (749, false),
            (750, true),
            (1_000, false),
        ] {
            assert_eq!(pod_blink_yellow(ms), yellow, "{ms} ms");
        }
        // The station box's outline takes the phase; a frozen clock holds it
        // and asks for no redraws.
        let mut ui = with_pod_station();
        ui.activate(4);
        ui.activate(100);
        ui.activate(201);
        assert!(ui.loadout.designator(1).is_some());
        let order = ui.display_order();
        let slot = order.iter().position(|&index| index == 1).unwrap();
        let (x, y) = (
            350 + (slot as i32 % 2) * 119 + 3,
            121 + (slot as i32 / 2) * 71 + 14,
        );
        let edge = |ui: &mut Ordnance, ms: u64| {
            ui.frozen_ms = Some(ms);
            let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
            let animating = ui.render(&mut pixels);
            assert!(!animating, "a frozen clock needs no redraws");
            let at = ((y - 1) as usize * WIDTH + (x + 10) as usize) * 4;
            [pixels[at], pixels[at + 1], pixels[at + 2]]
        };
        assert_eq!(edge(&mut ui, 0), [94, 30, 20]);
        assert_eq!(edge(&mut ui, 250), [201, 179, 49]);
        assert_eq!(edge(&mut ui, 500), [94, 30, 20]);
        // On the live clock a hung pod keeps the page redrawing.
        ui.frozen_ms = None;
        assert!(ui.render(&mut vec![0u8; WIDTH * HEIGHT * 4]));
        // Without a pod the clock is idle.
        ui.loadout.configuration.designator_pods.clear();
        assert!(!ui.render(&mut vec![0u8; WIDTH * HEIGHT * 4]));
    }
    #[test]
    fn the_retired_pave_knife_loads_as_the_pave_spike_and_is_not_offered() {
        use tore_world::mission::LoadoutSpec;
        assert_eq!(
            tore_sim::combat::sight::pod_record("PAVEKNF.SEE"),
            "PAVESPK.SEE"
        );
        assert_eq!(
            tore_sim::combat::sight::pod_record("PAVESPK.SEE"),
            "PAVESPK.SEE"
        );
        assert_eq!(tore_sim::combat::sight::DESIGNATOR_PODS, ["PAVESPK.SEE"]);
        let ui = with_pod_station();
        let mut spec = LoadoutSpec::of(&ui.loadout);
        // The synthetic record's flags were not made for pods: Cheat loading.
        spec.cheat = true;
        spec.stations[1].weapon = "PAVEKNF.SEE".into();
        spec.stations[1].quantity = 0;
        let mut resources = std::collections::BTreeMap::new();
        resources.insert(
            "PAVESPK.SEE".to_owned(),
            tore_world::test_support::resources::laser_pod("PAVESPK.SEE"),
        );
        let applied = spec
            .apply(ui.loadout.clone(), &resources, None)
            .expect("an old Pave Knife row still loads");
        assert_eq!(applied.designator(1).unwrap().source, "PAVESPK.SEE");
    }
}
