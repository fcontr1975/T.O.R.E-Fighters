//! Running-flight respawn menus, using the flight Escape menu widget.
use crate::{
    AppResult,
    pause_menu::{Event, Look, PauseMenu},
};
use std::collections::BTreeMap;
use tore_formats::{aircraft::AircraftId, font::Font, ui::MenuNode, weapons::Weapon};
use tore_sim::combat::{live::TankStore, loadout::Loadout, sight::DesignatorStore};
use tore_world::{
    aircraft_type::AircraftType,
    mission::{LoadoutSpec, MissionSpec},
    world::revive::RevivalWeapons,
};

const LOOK: Look = Look {
    title: "RESPAWN LOADOUT",
    buttons: &["Done", "Cancel", "Mission..."],
    help: |_| vec![],
};
mod list;
use list::{Entry, List};

struct Picker {
    station: Option<usize>,
    list: List,
    candidates: Vec<Loadout>,
}
pub(crate) const AIRCRAFT: &str = "Aircraft...";
pub(crate) const LOADOUT: &str = "Loadout...";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    Hidden,
    Loadout,
    Aircraft,
}
impl Access {
    fn waiting(revival: Option<&tore_session::wire::messages::Revival>, choice: bool) -> Self {
        match revival {
            Some(r) if r.rule == tore_session::settings::Respawn::Revive && r.lives != Some(0) => {
                if choice {
                    Self::Aircraft
                } else {
                    Self::Loadout
                }
            }
            _ => Self::Hidden,
        }
    }

    fn menu(self, tree: &mut [MenuNode]) {
        if let Some(tab) = tree.first_mut() {
            tab.children
                .retain(|n| n.label != AIRCRAFT && n.label != LOADOUT);
            if self == Self::Aircraft {
                tab.children.push(row(AIRCRAFT.into()));
            }
            if self != Self::Hidden {
                tab.children.push(row(LOADOUT.into()));
            }
        }
    }
}
/// The internal fuel after a click on the fuel row: up a tenth of the tanks
/// and, from full, round to a tenth; or, backward, down a tenth and, from
/// the lowest, round to full.
fn turn_fuel(load: &Loadout, forward: bool) -> f64 {
    let full = load.internal_capacity_lbs;
    let tenth = full * 0.1;
    if forward {
        if load.fuel_lbs >= full {
            tenth
        } else {
            (load.fuel_lbs + tenth).min(full)
        }
    } else if load.fuel_lbs <= tenth + 1e-6 {
        full
    } else {
        (load.fuel_lbs - tenth).max(tenth)
    }
}
fn row(label: String) -> MenuNode {
    MenuNode {
        label,
        shortcut: String::new(),
        children: vec![],
    }
}

pub struct Menu {
    pub brief_open: bool,
    widget: PauseMenu,
    tree: Vec<MenuNode>,
    pub load: Loadout,
    aircraft: Vec<AircraftId>,
    weapons: Vec<Weapon>,
    tanks: Vec<TankStore>,
    pods: Vec<DesignatorStore>,
    picker: Option<Picker>,
    rule: RevivalWeapons,
    pub message: Option<String>,
}
pub enum Outcome {
    None,
    Click,
    Cancel,
    Done(AircraftId, LoadoutSpec),
}
impl Menu {
    pub fn new(
        aircraft: AircraftId,
        mut allowed: Vec<AircraftId>,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
        rule: RevivalWeapons,
        gap: impl Fn(&str) -> bool,
    ) -> AppResult<Self> {
        let load = Self::standard(aircraft, data, spec)?;
        // The creator and lobby sort by display name, then exact selection key.
        allowed.sort_by(|a, b| {
            a.label()
                .cmp(b.label())
                .then(a.selection_key().cmp(b.selection_key()))
        });
        let mut weapons: Vec<_> = data
            .iter()
            .filter(|(name, _)| tore_sim::combat::loadout::supported(name) && !gap(name))
            .filter_map(|(name, bytes)| Weapon::parse(name, bytes).ok())
            .collect();
        let font = crate::ordnance::label_font();
        let display = crate::ordnance::weapon_label(data, &font);
        for weapon in &mut weapons {
            display(weapon);
        }
        crate::ordnance::sort_weapons(&mut weapons);
        let tanks = data
            .iter()
            .filter(|(name, _)| name.ends_with(".GAS") && !gap(name))
            .filter_map(|(name, bytes)| TankStore::parse(name, bytes).ok())
            .collect();
        let pods = tore_sim::combat::sight::DESIGNATOR_PODS
            .iter()
            .filter(|name| !gap(name))
            .filter_map(|name| DesignatorStore::parse(name, data.get(*name)?).ok())
            .collect();
        let mut menu = Self {
            brief_open: false,
            widget: PauseMenu::default(),
            tree: vec![],
            load,
            aircraft: allowed,
            weapons,
            tanks,
            pods,
            picker: None,
            rule,
            message: None,
        };
        menu.refresh();
        Ok(menu)
    }
    fn standard(
        aircraft: AircraftId,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> AppResult<Loadout> {
        let kind = AircraftType::load(data, aircraft)?;
        let mut load = Loadout::new(&kind.profile, |name| {
            data.get(name)
                .cloned()
                .ok_or_else(|| std::io::Error::other(format!("missing {name}")))
        })?;
        load.cheat = spec.cheat_loadouts;
        if spec.guns_only {
            load.restrict_to_guns();
        }
        Ok(load)
    }
    fn restore_selection(
        &mut self,
        selected: &LoadoutSpec,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) {
        let restored = AircraftType::load(data, self.load.aircraft)
            .and_then(|kind| selected.check_in(&kind.profile, data, spec));
        match restored {
            Ok(mut load) => {
                load.cheat = spec.cheat_loadouts;
                self.load = load;
            }
            Err(error) => self.message = Some(format!("Using standard load: {error}")),
        }
        self.refresh();
    }

    fn restore_choice(
        &mut self,
        aircraft: AircraftId,
        selected: &LoadoutSpec,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) {
        if aircraft != self.load.aircraft {
            if !self.aircraft.contains(&aircraft) {
                return;
            }
            match Self::standard(aircraft, data, spec) {
                Ok(load) => self.load = load,
                Err(error) => {
                    self.message = Some(error.to_string());
                    return;
                }
            }
        }
        self.restore_selection(selected, data, spec);
    }

    fn refresh(&mut self) {
        let rows = self.load.editable_stations();
        let mut loads: Vec<MenuNode> = self
            .load
            .station_display_order()
            .into_iter()
            .map(|index| {
                let station = &rows[index];
                let heading = [
                    "Centerline",
                    "Fuselage",
                    "Internal Gun",
                    "Internal Bay",
                    "Wing",
                    "Wingtip",
                ]
                .get(usize::from(station.location))
                .copied()
                .unwrap_or("Station");
                let tank = station
                    .tank
                    .and_then(|slot| self.load.configuration.tanks.get(slot))
                    .filter(|tank| tank.quantity > 0);
                let pod = station.weapon.and_then(|slot| self.load.designator(slot));
                let store = if let Some(tank) = tank {
                    format!(
                        "{} x{}",
                        tank.store.as_ref().map_or("Empty", |s| s.name.as_str()),
                        tank.quantity
                    )
                } else if let Some(pod) = pod {
                    format!("{} x1", pod.name)
                } else if let Some(slot) = station.weapon {
                    let current = &self.load.configuration.stations[slot];
                    format!(
                        "{} x{}",
                        current.weapon.hud_name,
                        self.rule.cut(&current.weapon, self.load.quantities[slot])
                    )
                } else if let Some(slot) = station.tank {
                    let tank = &self.load.configuration.tanks[slot];
                    format!(
                        "{} x{}",
                        tank.store.as_ref().map_or("Empty", |s| s.name.as_str()),
                        tank.quantity
                    )
                } else {
                    "Empty".into()
                };
                row(format!("{heading}: {store} >"))
            })
            .collect();
        loads.push(row(format!(
            "Internal fuel: {:.0}%",
            100. * self.load.fuel_lbs / self.load.internal_capacity_lbs.max(1.)
        )));
        loads.push(row("Standard load".into()));
        loads.push(row("Done".into()));
        loads.push(row("Mission...".into()));
        self.tree = vec![MenuNode {
            label: "Loadout".into(),
            shortcut: String::new(),
            children: loads,
        }];
        if !self.aircraft.is_empty() {
            let mut rows = self
                .aircraft
                .iter()
                .map(|id| {
                    row(format!(
                        "{}{}",
                        if *id == self.load.aircraft { "* " } else { "" },
                        id.label()
                    ))
                })
                .collect::<Vec<_>>();
            rows.push(row("Mission...".into()));
            self.tree.push(MenuNode {
                label: "Aircraft".into(),
                shortcut: String::new(),
                children: rows,
            });
        }
    }
    fn check(
        &self,
        candidate: &Loadout,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> AppResult<()> {
        let kind = AircraftType::load(data, candidate.aircraft)?;
        LoadoutSpec::of(candidate).check_in(&kind.profile, data, spec)?;
        Ok(())
    }
    fn select(
        &mut self,
        index: usize,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> Outcome {
        if self.tree[self.widget.root]
            .children
            .get(index)
            .is_some_and(|row| row.label == "Mission...")
        {
            self.brief_open = true;
            return Outcome::Click;
        }
        self.message = None;
        if self.widget.root == 1 {
            if let Some(id) = self.aircraft.get(index).copied() {
                match Self::standard(id, data, spec) {
                    Ok(load) => self.load = load,
                    Err(e) => self.message = Some(e.to_string()),
                }
                self.picker = None;
                self.widget.show_tab(0);
            }
        } else {
            let order = self.load.station_display_order();
            if index == order.len() + 2 {
                return self.done(data, spec);
            }
            let mut candidate = self.load.clone();
            if index == order.len() + 1 {
                match Self::standard(self.load.aircraft, data, spec) {
                    Ok(load) => candidate = load,
                    Err(e) => self.message = Some(e.to_string()),
                }
            } else if index == order.len() {
                candidate.fuel_lbs = turn_fuel(&candidate, true);
            } else if let Some(&station) = order.get(index) {
                self.open_station(station, data, spec);
                return Outcome::Click;
            }

            match self.check(&candidate, data, spec) {
                Ok(()) => self.load = candidate,
                Err(e) => self.message = Some(e.to_string()),
            }
        }
        self.refresh();
        Outcome::Click
    }
    /// The right button on a row: the Internal fuel row steps back a tenth,
    /// wrapping from the lowest to full, as the left button steps up and
    /// wraps from full to a tenth. The station rows open a list, the
    /// aircraft and bottom buttons are commands; none of them turn.
    fn select_back(
        &mut self,
        index: usize,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> Outcome {
        if self.widget.root == 1 || self.picker.is_some() {
            return Outcome::None;
        }
        if index != self.load.station_display_order().len() {
            return Outcome::None;
        }
        self.message = None;
        let mut candidate = self.load.clone();
        candidate.fuel_lbs = turn_fuel(&candidate, false);
        match self.check(&candidate, data, spec) {
            Ok(()) => self.load = candidate,
            Err(e) => self.message = Some(e.to_string()),
        }
        self.refresh();
        Outcome::Click
    }
    fn open_station(
        &mut self,
        station: usize,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) {
        use crate::ordnance::{CatalogEntry, catalog_entries};
        let editable = self.load.editable_stations()[station];
        let mut entries = Vec::new();
        let mut candidates = Vec::new();
        let mut offer = |mut load: Loadout, label: String, search: String, group: &'static str| {
            if load.refresh_equipment().is_ok() && self.check(&load, data, spec).is_ok() {
                entries.push(Entry {
                    label,
                    search,
                    group,
                });
                candidates.push(load);
            }
        };
        let mut empty = self.load.clone();
        if let Some(slot) = editable.weapon {
            empty.quantities[slot] = 0;
            let _ = empty.unload_designator(slot);
        }
        if let Some(slot) = editable.tank {
            let _ = empty.unload_tank(slot);
        }
        offer(empty, "Empty".into(), "Empty".into(), "");
        for category in [0, 1] {
            let group = if category == 0 {
                "Air-to-air"
            } else {
                "Air-to-surface"
            };
            for entry in catalog_entries(&self.weapons, &self.tanks, &self.pods, category) {
                match entry {
                    CatalogEntry::Weapon(i) => {
                        if let Some(slot) = editable.weapon {
                            let weapon = &self.weapons[i];
                            let mut next = self.load.clone();
                            if next.select(slot, weapon.clone()).is_ok()
                                && self.rule.cut(weapon, next.quantities[slot]) > 0
                            {
                                let maximum = next.quantities[slot];
                                let quantities: Vec<u16> = if maximum <= 20 {
                                    (1..=maximum).collect()
                                } else {
                                    (1..=10)
                                        .map(|step| (u32::from(maximum) * step / 10) as u16)
                                        .collect()
                                };
                                let names = data
                                    .get(&weapon.source)
                                    .and_then(|b| tore_formats::aircraft::Brf::parse(b).ok())
                                    .and_then(|b| b.strings("si_names").ok())
                                    .unwrap_or_default()
                                    .join(" ");
                                let search = format!(
                                    "{} {} {} {names}",
                                    weapon.name, weapon.hud_name, weapon.source
                                );
                                for quantity in quantities {
                                    let mut amount = next.clone();
                                    amount.quantities[slot] = quantity;
                                    offer(
                                        amount,
                                        format!(
                                            "{} ({}) x{}",
                                            weapon.name,
                                            weapon.hud_name,
                                            self.rule.cut(weapon, quantity)
                                        ),
                                        search.clone(),
                                        group,
                                    );
                                }
                            }
                        }
                    }
                    CatalogEntry::Tank(i) => {
                        if let Some(slot) = editable.tank {
                            let tank = &self.tanks[i];
                            let mut next = self.load.clone();
                            if next.select_tank(slot, tank.clone()).is_ok() {
                                for quantity in 1..=next.configuration.tanks[slot].quantity {
                                    let mut amount = next.clone();
                                    amount.configuration.tanks[slot].quantity = quantity;
                                    offer(
                                        amount,
                                        format!("{} x{quantity}", tank.name),
                                        format!("{} {}", tank.name, tank.source),
                                        group,
                                    );
                                }
                            }
                        }
                    }
                    CatalogEntry::Designator(i) => {
                        if let Some(slot) = editable.weapon {
                            let pod = &self.pods[i];
                            let mut next = self.load.clone();
                            if next.load_designator(slot, pod.clone()).is_ok() {
                                offer(
                                    next,
                                    format!("{} x1", pod.name),
                                    format!("{} {}", pod.name, pod.source),
                                    group,
                                );
                            }
                        }
                    }
                }
            }
        }
        self.picker = Some(Picker {
            station: Some(station),
            list: List::new(entries),
            candidates,
        });
    }
    fn choose(
        &mut self,
        index: usize,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> Outcome {
        if let Some(picker) = &self.picker
            && picker.station.is_some()
        {
            if let Some(candidate) = picker.candidates.get(index) {
                match self.check(candidate, data, spec) {
                    Ok(()) => {
                        self.load = candidate.clone();
                        log::info!(
                            "Respawn: selected station {}: {:?}",
                            picker.station.unwrap(),
                            LoadoutSpec::of(&self.load)
                        );
                    }
                    Err(error) => {
                        self.message = Some(error.to_string());
                        return Outcome::Click;
                    }
                }
            }
            self.picker = None;
            self.refresh();
            return Outcome::Click;
        }
        self.select(index, data, spec)
    }
    fn show_tab(&mut self, index: usize) {
        self.widget.show_tab(index);
        self.picker = (index == 1).then(|| Picker {
            station: None,
            candidates: vec![],
            list: List::new(
                self.aircraft
                    .iter()
                    .map(|id| Entry {
                        label: format!(
                            "{}{}",
                            if *id == self.load.aircraft { "* " } else { "" },
                            id.label()
                        ),
                        search: format!(
                            "{} {} {}",
                            id.label(),
                            id.label().replace(' ', ""),
                            id.selection_key()
                        ),
                        group: "",
                    })
                    .collect(),
            ),
        });
    }
    pub fn text(&mut self, text: &str) {
        if let Some(picker) = &mut self.picker {
            picker.list.text(text);
        }
    }
    pub fn searching(&self) -> bool {
        self.picker.as_ref().is_some_and(|p| p.list.searching)
    }
    pub fn moved(&mut self, point: Option<(f64, f64)>) {
        if let Some(picker) = &mut self.picker {
            picker.list.moved(point);
        }
        if point.is_none() {
            self.widget.cancel_press();
        }
    }
    pub fn wheel(&mut self, notches: i32) {
        if let Some(picker) = &mut self.picker {
            picker.list.wheel(notches);
        } else {
            self.widget.key(
                if notches > 0 { "ArrowUp" } else { "ArrowDown" },
                &self.tree,
            );
        }
    }
    // Keep tabs and bottom buttons, but let the picker own its entire viewport.
    fn shell(&self) -> Vec<MenuNode> {
        let mut tree = self.tree.clone();
        if self.picker.is_some() {
            tree[self.widget.root].children.clear();
        }
        tree
    }
    fn done(&mut self, data: &BTreeMap<String, Vec<u8>>, spec: &MissionSpec) -> Outcome {
        match self.check(&self.load, data, spec) {
            Ok(()) => Outcome::Done(self.load.aircraft, LoadoutSpec::of(&self.load)),
            Err(e) => {
                self.message = Some(e.to_string());
                Outcome::Click
            }
        }
    }
    fn event(
        &mut self,
        event: Event,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> Outcome {
        match event {
            Event::Select(index) => self.select(index, data, spec),
            Event::Button(0) => self.done(data, spec),
            Event::Button(2) => {
                self.brief_open = true;
                Outcome::Click
            }
            Event::Button(_) => Outcome::Cancel,
            Event::Tab(index) => {
                self.show_tab(index);
                Outcome::Click
            }
            Event::Switched(index) => {
                self.show_tab(index);
                Outcome::Click
            }
            Event::None => Outcome::None,
            Event::Click => Outcome::Click,
        }
    }
    pub fn key(
        &mut self,
        key: &str,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> Outcome {
        if key == "Escape" {
            if let Some(picker) = &mut self.picker {
                if !picker.list.query.is_empty() {
                    picker.list.clear();
                } else if picker.station.is_some() {
                    self.picker = None;
                } else {
                    return Outcome::Cancel;
                }
                return Outcome::Click;
            }
            return Outcome::Cancel;
        }
        if let Some(picker) = &mut self.picker {
            if matches!(key, "ArrowLeft" | "ArrowRight") {
                let tab = (self.widget.root + 1) % self.tree.len();
                self.show_tab(tab);
                return Outcome::Click;
            }
            if picker.list.searching || !matches!(key, "m" | "M") {
                return if let Some(index) = picker.list.key(key) {
                    self.choose(index, data, spec)
                } else {
                    Outcome::None
                };
            }
        }
        if key == "m" || key == "M" {
            self.brief_open = true;
            return Outcome::Click;
        }
        let event = self.widget.key(key, &self.tree);
        self.event(event, data, spec)
    }
    pub fn pointer(
        &mut self,
        point: Option<(f64, f64)>,
        down: bool,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> Outcome {
        if let Some(picker) = &mut self.picker
            && let Some(index) = picker.list.pointer(point, down)
        {
            return self.choose(index, data, spec);
        }
        let event = self.widget.pointer(&self.shell(), &LOOK, point, down);
        self.event(event, data, spec)
    }
    /// The right button, which turns the fuel row back.
    pub fn right_pointer(
        &mut self,
        point: Option<(f64, f64)>,
        down: bool,
        data: &BTreeMap<String, Vec<u8>>,
        spec: &MissionSpec,
    ) -> Outcome {
        match self.widget.right_pointer(&self.shell(), &LOOK, point, down) {
            Some(index) => self.select_back(index, data, spec),
            None => Outcome::None,
        }
    }
    pub fn preview_aircraft(&mut self) {
        if self.tree.len() > 1 {
            self.show_tab(1);
        }
    }

    pub fn draw(
        &self,
        pixels: &mut [u8],
        font: &Font,
        prompt: &str,
        sprites: &BTreeMap<String, crate::menu::Sprite>,
    ) {
        self.widget
            .draw(pixels, font, &self.shell(), &LOOK, &|_| None);
        if let Some(picker) = &self.picker {
            let title = picker.station.map_or_else(
                || "AIRCRAFT".into(),
                |station| {
                    let row = self
                        .load
                        .station_display_order()
                        .iter()
                        .position(|i| *i == station)
                        .unwrap_or(0);
                    format!(
                        "{} > STORES",
                        self.tree[0].children[row]
                            .label
                            .split(':')
                            .next()
                            .unwrap_or("Station")
                    )
                },
            );
            picker.list.draw(pixels, font, sprites, &title);
        }
        let prompt = prompt.strip_prefix("Press Enter").map_or_else(
            || prompt.to_owned(),
            |rest| format!("Close this page, then Enter / A{rest}"),
        );
        crate::menu::Canvas(pixels).rect((10, 398, 620, 20), [24, 34, 45, 230]);
        if self.message.is_some() {
            crate::menu::Canvas(pixels).rect((10, 422, 620, 22), [24, 34, 45, 230]);
        }
        let mut paint = crate::hud::Paint {
            pixels,
            clip: (0, 0, 640, 480),
            color: [230, 240, 245, 255],
        };
        paint.text(font, &prompt, 16, 404);
        if let Some(message) = &self.message {
            paint.text(font, message, 16, 426);
        }
    }
}

impl crate::App {
    pub(crate) fn waiting_to_respawn(&self) -> bool {
        self.net_flight.is_some()
            && self
                .net
                .as_ref()
                .is_some_and(|s| s.client.revival().is_some())
    }

    pub(crate) fn respawn_brief_open(&self) -> bool {
        self.net_flight
            .as_ref()
            .and_then(|f| f.respawn_menu.as_ref())
            .is_some_and(|m| m.brief_open)
    }
    fn close_respawn_brief(&mut self) {
        if let Some(menu) = self
            .net_flight
            .as_mut()
            .and_then(|f| f.respawn_menu.as_mut())
        {
            menu.brief_open = false;
        }
    }

    pub(crate) fn respawn_open(&self) -> bool {
        self.net_flight
            .as_ref()
            .is_some_and(|f| f.respawn_menu.is_some())
    }
    fn respawn_access(&self) -> Access {
        let Some(session) = self.net.as_ref().filter(|_| self.net_flight.is_some()) else {
            return Access::Hidden;
        };
        let Some(spec) = session.client.spec() else {
            return Access::Hidden;
        };
        let Some((_, plane)) = session.client.seat() else {
            return Access::Hidden;
        };
        let Some(entry) = self.world.roster.plane(plane) else {
            return Access::Hidden;
        };
        Access::waiting(
            session.client.revival().map(|kept| &kept.revival),
            spec.player_choice[MissionSpec::wing_index(entry.slot.wing)],
        )
    }

    pub(crate) fn refresh_respawn_menu(&mut self) {
        let access = self.respawn_access();
        if let Some(flight) = &mut self.net_flight {
            access.menu(&mut flight.menu);
            if access != Access::Hidden && !flight.respawn_seen {
                self.open_respawn_menu(false);
            }
        }
    }

    pub(crate) fn open_respawn_menu(&mut self, aircraft_tab: bool) {
        let access = self.respawn_access();
        if access == Access::Hidden || (aircraft_tab && access != Access::Aircraft) {
            return;
        }
        let Some(session) = &self.net else { return };
        let Some(spec) = session.client.spec() else {
            return;
        };
        let Some((_, plane)) = session.client.seat() else {
            return;
        };
        let Some(entry) = self.world.roster.plane(plane) else {
            return;
        };
        let Some(aircraft) = self.world.aircraft_of(plane) else {
            return;
        };
        let choice = spec.player_choice[MissionSpec::wing_index(entry.slot.wing)];
        let allowed = if choice {
            AircraftId::SELECTABLE
                .into_iter()
                .filter(|id| {
                    session
                        .client
                        .gap_refusal(
                            tore_session::wire::messages::ItemKind::Aircraft,
                            id.selection_key(),
                        )
                        .is_none()
                        && AircraftType::load(&*self.theater_resources, *id).is_ok()
                })
                .collect()
        } else {
            vec![]
        };
        let value = session
            .client
            .lobby()
            .and_then(|l| {
                l.settings
                    .iter()
                    .find(|(n, _)| *n == tore_session::settings::number::REVIVE_WEAPONS)
            })
            .map_or(0, |(_, v)| *v);
        let rule = RevivalWeapons::from_value(value).unwrap_or_default();
        let menu = Menu::new(
            aircraft,
            allowed,
            &self.theater_resources,
            spec,
            rule,
            |name| {
                session
                    .client
                    .gap_refusal(tore_session::wire::messages::ItemKind::Weapon, name)
                    .is_some()
            },
        );
        let menu = menu.map(|mut menu| {
            let lobby = session.client.lobby();
            let kept = self
                .net_flight
                .as_ref()
                .and_then(|f| f.respawn_choice.as_ref())
                .map(|(id, load)| (*id, load))
                .or_else(|| {
                    session
                        .revival_draft
                        .as_ref()
                        .filter(|(mission, id, _)| {
                            lobby.is_some_and(|l| l.mission == *mission)
                                && (choice || *id == aircraft)
                        })
                        .map(|(_, id, load)| (*id, load))
                })
                .or_else(|| {
                    lobby
                        .and_then(|l| l.me())
                        .and_then(|p| p.slot)
                        .and_then(|slot| spec.plane_loadouts.get(&slot))
                        .map(|load| (aircraft, load))
                });
            if let Some((id, kept)) = kept {
                menu.restore_choice(id, kept, &self.theater_resources, spec);
            }
            if aircraft_tab {
                menu.preview_aircraft();
            }
            menu
        });
        let flight = self.net_flight.as_mut().expect("checked");
        flight.respawn_seen = true;
        match menu {
            Ok(menu) => {
                log::info!(
                    "Respawn: opened {} for {:?}",
                    if aircraft_tab { "Aircraft" } else { "Loadout" },
                    menu.load.aircraft
                );
                flight.respawn_menu = Some(menu);
                self.flight_ui.menu = false;
            }
            Err(error) => self.flight_ui.message(error.to_string()),
        }
        self.camera.keys.clear();
        self.mouse_look = None;
        self.input.release_keys();
        self.release_trigger();
        self.input.context(self.input_paused(), self.focused);
    }
    fn respawn_outcome(&mut self, outcome: Outcome) -> crate::menu::Action {
        let Some(flight) = &mut self.net_flight else {
            return crate::menu::Action::None;
        };
        match outcome {
            Outcome::None => return crate::menu::Action::None,
            Outcome::Cancel => {
                log::info!("Respawn: cancelled edits");
                flight.respawn_menu = None;
            }
            Outcome::Done(aircraft, loadout) => {
                log::info!("Respawn: saved choice {aircraft:?}");
                if let Some(session) = &mut self.net
                    && let Some(lobby) = session.client.lobby()
                {
                    session.revival_draft = Some((lobby.mission, aircraft, loadout.clone()));
                }
                flight.respawn_choice = Some((aircraft, loadout));
                flight.respawn_menu = None;
            }
            Outcome::Click => {
                if flight
                    .respawn_menu
                    .as_ref()
                    .is_some_and(|menu| menu.brief_open)
                {
                    self.flight_ui.map.brief = None;
                }
            }
        }
        self.camera.keys.clear();
        self.mouse_look = None;
        self.input.release_keys();
        self.release_trigger();
        self.input.context(self.input_paused(), self.focused);
        crate::menu::Action::Click
    }
    pub(crate) fn respawn_key(&mut self, key: &str) -> crate::menu::Action {
        if self.respawn_brief_open() {
            if key == "Enter"
                || self
                    .flight_ui
                    .map
                    .brief
                    .as_mut()
                    .is_some_and(|b| b.key(key))
            {
                self.close_respawn_brief();
            }
            return crate::menu::Action::None;
        }

        let Some(spec) = self.net.as_ref().and_then(|s| s.client.spec()) else {
            return crate::menu::Action::None;
        };
        let Some(menu) = self
            .net_flight
            .as_mut()
            .and_then(|f| f.respawn_menu.as_mut())
        else {
            return crate::menu::Action::None;
        };
        let outcome = menu.key(key, &self.theater_resources, spec);
        self.respawn_outcome(outcome)
    }
    /// The right button over the respawn menu: the fuel row turns back.
    pub(crate) fn respawn_right_pointer(
        &mut self,
        point: Option<(f64, f64)>,
        down: bool,
    ) -> crate::menu::Action {
        if self.respawn_brief_open() {
            return crate::menu::Action::None;
        }
        let Some(spec) = self.net.as_ref().and_then(|s| s.client.spec()) else {
            return crate::menu::Action::None;
        };
        let Some(menu) = self
            .net_flight
            .as_mut()
            .and_then(|f| f.respawn_menu.as_mut())
        else {
            return crate::menu::Action::None;
        };
        let outcome = menu.right_pointer(point, down, &self.theater_resources, spec);
        self.respawn_outcome(outcome)
    }
    pub(crate) fn respawn_pointer(
        &mut self,
        point: Option<(f64, f64)>,
        down: bool,
    ) -> crate::menu::Action {
        if self.respawn_brief_open() {
            if self
                .flight_ui
                .map
                .brief
                .as_mut()
                .is_some_and(|b| b.pointer(point, down))
            {
                self.close_respawn_brief();
            }
            return crate::menu::Action::None;
        }
        let Some(spec) = self.net.as_ref().and_then(|s| s.client.spec()) else {
            return crate::menu::Action::None;
        };
        let Some(menu) = self
            .net_flight
            .as_mut()
            .and_then(|f| f.respawn_menu.as_mut())
        else {
            return crate::menu::Action::None;
        };
        let outcome = menu.pointer(point, down, &self.theater_resources, spec);
        self.respawn_outcome(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escape_entries_last_through_the_delay_and_follow_slot_and_lives_rules() {
        use std::time::Duration;
        use tore_session::{client::revival::Kept, settings::Respawn, wire::messages::Revival};
        let mut kept = Kept {
            revival: Revival {
                rule: Respawn::Revive,
                lives: Some(2),
                wait_seconds: 30,
                why: None,
            },
            received: Duration::ZERO,
        };
        let mut tree = vec![MenuNode {
            label: "Game".into(),
            shortcut: String::new(),
            children: vec![row("Mission...".into())],
        }];
        for seconds in [0, 25, 30, 300] {
            let left = kept.seconds_left(Duration::from_secs(seconds));
            assert_eq!(
                tore_session::client::revival::may_fly_again(&kept.revival, left),
                seconds >= 30
            );
            for choice in [false, true] {
                let access = Access::waiting(Some(&kept.revival), choice);
                access.menu(&mut tree);
                access.menu(&mut tree);
                let labels: Vec<_> = tree[0].children.iter().map(|r| r.label.as_str()).collect();
                assert_eq!(
                    labels,
                    if choice {
                        vec!["Mission...", AIRCRAFT, LOADOUT]
                    } else {
                        vec!["Mission...", LOADOUT]
                    }
                );
                let mut ui = crate::flight_ui::FlightUi::default();
                ui.key("Escape", false, false, false, &tree);
                ui.key("ArrowUp", false, false, false, &tree);
                assert_eq!(
                    ui.key("Enter", false, false, false, &tree),
                    crate::flight_ui::Command::RespawnLoadout
                );
                if choice {
                    ui.key("ArrowUp", false, false, false, &tree);
                    assert_eq!(
                        ui.key("Enter", false, false, false, &tree),
                        crate::flight_ui::Command::RespawnAircraft
                    );
                }
            }
        }
        for rule in [Respawn::None, Respawn::AiSlot] {
            kept.revival.rule = rule;
            assert_eq!(Access::waiting(Some(&kept.revival), true), Access::Hidden);
        }
        kept.revival.rule = Respawn::Revive;
        kept.revival.lives = Some(0);
        assert_eq!(Access::waiting(Some(&kept.revival), true), Access::Hidden);
        Access::waiting(None, true).menu(&mut tree);
        assert_eq!(tree[0].children.len(), 1);
    }

    #[test]
    fn saved_aircraft_and_stores_survive_reopening_and_cancel_keeps_the_saved_choice() {
        let (data, spec) = setup();
        let open = || {
            Menu::new(
                AircraftId::F18,
                vec![AircraftId::F18, AircraftId::Ac130],
                &data,
                &spec,
                RevivalWeapons::Missiles,
                |_| false,
            )
            .unwrap()
        };
        let mut menu = open();
        menu.preview_aircraft();
        menu.select(
            menu.aircraft
                .iter()
                .position(|id| *id == AircraftId::Ac130)
                .unwrap(),
            &data,
            &spec,
        );
        menu.select(menu.load.station_display_order().len(), &data, &spec);
        menu.select(0, &data, &spec);
        let Outcome::Done(aircraft, selected) = menu.done(&data, &spec) else {
            panic!("valid choice")
        };
        assert_eq!(aircraft, AircraftId::Ac130);
        for _ in 0..3 {
            let mut reopened = open();
            reopened.restore_choice(aircraft, &selected, &data, &spec);
            assert_eq!(reopened.load.aircraft, aircraft);
            assert_eq!(LoadoutSpec::of(&reopened.load), selected);
            reopened.select(reopened.load.station_display_order().len(), &data, &spec);
            assert!(matches!(
                reopened.key("Escape", &data, &spec),
                Outcome::Cancel
            ));
        }
        let mut fixed = Menu::new(
            AircraftId::F18,
            vec![],
            &data,
            &spec,
            RevivalWeapons::Missiles,
            |_| false,
        )
        .unwrap();
        fixed.restore_choice(aircraft, &selected, &data, &spec);
        assert_eq!(fixed.load.aircraft, AircraftId::F18);
    }

    #[test]
    fn all_revival_weapon_rules_keep_their_cut_on_reopen() {
        let (data, spec) = setup();
        for rule in [
            RevivalWeapons::Missiles,
            RevivalWeapons::NoMissiles,
            RevivalWeapons::Guns,
            RevivalWeapons::HalfGuns,
        ] {
            let mut menu =
                Menu::new(AircraftId::F18, vec![], &data, &spec, rule, |_| false).unwrap();
            let selected = LoadoutSpec::of(&menu.load);
            let rows: Vec<_> = menu.tree[0]
                .children
                .iter()
                .map(|r| r.label.clone())
                .collect();
            menu.restore_choice(AircraftId::F18, &selected, &data, &spec);
            assert_eq!(
                menu.tree[0]
                    .children
                    .iter()
                    .map(|r| r.label.clone())
                    .collect::<Vec<_>>(),
                rows
            );
            menu.check(&menu.load, &data, &spec).unwrap();
            for (station, quantity) in menu
                .load
                .configuration
                .stations
                .iter()
                .zip(&menu.load.quantities)
            {
                let cut = rule.cut(&station.weapon, *quantity);
                if tore_sim::combat::live::is_gun(&station.weapon) {
                    assert_eq!(
                        cut,
                        if rule == RevivalWeapons::HalfGuns {
                            quantity.div_ceil(2)
                        } else {
                            *quantity
                        }
                    );
                } else if matches!(rule, RevivalWeapons::Guns | RevivalWeapons::HalfGuns) {
                    assert_eq!(cut, 0);
                } else if rule == RevivalWeapons::Missiles {
                    assert_eq!(cut, *quantity);
                }
            }
        }
    }

    #[test]
    fn aircraft_results_are_complete_in_creator_order_and_search_names_and_designations() {
        let (data, spec) = setup();
        let mut menu = Menu::new(
            AircraftId::F18,
            AircraftId::SELECTABLE.to_vec(),
            &data,
            &spec,
            RevivalWeapons::Missiles,
            |_| false,
        )
        .unwrap();
        menu.preview_aircraft();
        let mut expected = AircraftId::SELECTABLE.to_vec();
        expected.sort_by_key(|id| (id.label(), id.selection_key()));
        assert_eq!(menu.aircraft, expected);
        let list = &mut menu.picker.as_mut().unwrap().list;
        for i in 0..expected.len() {
            assert_eq!(list.key("Enter"), Some(i));
            list.key("ArrowDown");
        }
        list.key("Tab");
        list.text("HoRnEt");
        assert_eq!(
            list.key("Enter").map(|i| expected[i]),
            Some(AircraftId::F18)
        );
        list.clear();
        list.text("16");
        assert_eq!(
            list.key("Enter").map(|i| expected[i]),
            Some(AircraftId::F16C)
        );
        list.clear();
        list.text("F-16C");
        assert_eq!(
            list.key("Enter").map(|i| expected[i]),
            Some(AircraftId::F16C)
        );
        list.clear();
        list.text("does not exist");
        assert_eq!(list.key("Enter"), None);
    }
    #[test]
    fn escape_clears_search_then_closes_station_then_discards_and_m_types_when_focused() {
        let (data, spec) = setup();
        let mut menu = Menu::new(
            AircraftId::F18,
            vec![],
            &data,
            &spec,
            RevivalWeapons::Missiles,
            |_| false,
        )
        .unwrap();
        let before = LoadoutSpec::of(&menu.load);
        menu.key("Enter", &data, &spec);
        assert!(menu.picker.is_some());
        menu.key("Tab", &data, &spec);
        menu.key("m", &data, &spec);
        menu.text("m");
        assert!(!menu.brief_open);
        assert_eq!(menu.picker.as_ref().unwrap().list.query, "m");
        assert!(matches!(menu.key("Escape", &data, &spec), Outcome::Click));
        assert_eq!(menu.picker.as_ref().unwrap().list.query, "");
        assert!(matches!(menu.key("Escape", &data, &spec), Outcome::Click));
        assert!(menu.picker.is_none());
        assert!(matches!(menu.key("Escape", &data, &spec), Outcome::Cancel));
        assert_eq!(LoadoutSpec::of(&menu.load), before);
    }
    #[test]
    fn every_offered_quantity_is_valid_and_revival_cuts_and_gaps_are_preserved() {
        let (data, spec) = setup();
        for aircraft in [AircraftId::F18, AircraftId::Ac130] {
            for rule in [
                RevivalWeapons::Missiles,
                RevivalWeapons::NoMissiles,
                RevivalWeapons::Guns,
                RevivalWeapons::HalfGuns,
            ] {
                let mut menu = Menu::new(aircraft, vec![], &data, &spec, rule, |name| {
                    name == "AIM9M.JT"
                })
                .unwrap();
                for station in menu.load.station_display_order() {
                    menu.open_station(station, &data, &spec);
                    let picker = menu.picker.as_ref().unwrap();
                    assert!(!picker.candidates.is_empty());
                    let slot = menu.load.editable_stations()[station].weapon.unwrap();
                    for candidate in &picker.candidates {
                        menu.check(candidate, &data, &spec).unwrap();
                        let weapon = &candidate.configuration.stations[slot].weapon;
                        let quantity = candidate.quantities[slot];
                        if quantity > 0 {
                            assert_ne!(weapon.source, "AIM9M.JT");
                            assert!(rule.cut(weapon, quantity) > 0);
                        }
                    }
                }
            }
        }
    }
    #[test]
    fn station_order_matches_shared_catalog_for_aircraft_and_capacity_variants() {
        use crate::ordnance::{CatalogEntry, catalog_entries};
        let (mut data, spec) = setup();
        tore_world::test_support::resources::add_a2g_stores(&mut data);
        for aircraft in [AircraftId::F18, AircraftId::Ac130] {
            for cheat in [false, true] {
                let mut spec = spec.clone();
                spec.cheat_loadouts = cheat;
                let mut menu = Menu::new(
                    aircraft,
                    vec![],
                    &data,
                    &spec,
                    RevivalWeapons::Missiles,
                    |_| false,
                )
                .unwrap();
                let order: Vec<_> = [0, 1]
                    .into_iter()
                    .flat_map(|category| {
                        catalog_entries(&menu.weapons, &menu.tanks, &menu.pods, category)
                    })
                    .filter_map(|entry| match entry {
                        CatalogEntry::Weapon(i) => Some(menu.weapons[i].source.clone()),
                        _ => None,
                    })
                    .collect();
                for station in menu.load.station_display_order() {
                    menu.open_station(station, &data, &spec);
                    let slot = menu.load.editable_stations()[station].weapon.unwrap();
                    let mut previous = None;
                    for load in &menu.picker.as_ref().unwrap().candidates {
                        if load.quantities[slot] == 0 || load.designator(slot).is_some() {
                            continue;
                        }
                        let source = &load.configuration.stations[slot].weapon.source;
                        let at = order.iter().position(|s| s == source).unwrap();
                        assert!(previous.is_none_or(|before| before <= at));
                        previous = Some(at);
                    }
                }
            }
        }
    }

    #[test]
    #[ignore = "requires the isolated imported TORE_DATA_DIR profile"]
    fn imported_respawn_catalogs_keep_shared_order_for_three_aircraft() {
        use crate::ordnance::{CatalogEntry, catalog_entries};
        let dir = crate::assets::data_directory().unwrap();
        let assets = crate::assets::Assets::load(&dir).unwrap();
        let data = &assets.theater_resources;
        for aircraft in [AircraftId::F18, AircraftId::Rafale, AircraftId::F14] {
            let spec = MissionSpec::new("UKR", aircraft);
            let mut menu = Menu::new(
                aircraft,
                vec![],
                data,
                &spec,
                RevivalWeapons::Missiles,
                |_| false,
            )
            .unwrap();
            let order: Vec<_> = [0, 1]
                .into_iter()
                .flat_map(|category| {
                    catalog_entries(&menu.weapons, &menu.tanks, &menu.pods, category)
                })
                .map(|entry| match entry {
                    CatalogEntry::Weapon(i) => menu.weapons[i].source.clone(),
                    CatalogEntry::Tank(i) => menu.tanks[i].source.clone(),
                    CatalogEntry::Designator(i) => menu.pods[i].source.clone(),
                })
                .collect();
            let mut offered = 0;
            for station in menu.load.station_display_order() {
                menu.open_station(station, data, &spec);
                let editable = menu.load.editable_stations()[station];
                let mut previous = None;
                for load in &menu.picker.as_ref().unwrap().candidates {
                    menu.check(load, data, &spec).unwrap();
                    let source = editable
                        .tank
                        .and_then(|slot| {
                            let tank = &load.configuration.tanks[slot];
                            tank.store
                                .as_ref()
                                .filter(|_| tank.quantity > 0)
                                .map(|t| &t.source)
                        })
                        .or_else(|| {
                            editable.weapon.and_then(|slot| {
                                if let Some(pod) = load.designator(slot) {
                                    Some(&pod.source)
                                } else if load.quantities[slot] > 0 {
                                    Some(&load.configuration.stations[slot].weapon.source)
                                } else {
                                    None
                                }
                            })
                        });
                    if let Some(source) = source {
                        let at = order.iter().position(|s| s == source).unwrap();
                        assert!(
                            previous.is_none_or(|before| before <= at),
                            "{aircraft:?} station {station}: {source}"
                        );
                        previous = Some(at);
                        offered += 1;
                    }
                }
            }
            assert!(offered > 0, "{aircraft:?}");
        }
    }

    fn setup() -> (BTreeMap<String, Vec<u8>>, MissionSpec) {
        (
            tore_world::test_support::resources::gunship_resources(),
            MissionSpec::new(
                tore_world::test_support::resources::THEATER,
                AircraftId::F18,
            ),
        )
    }
    #[test]
    fn mission_brief_is_reachable_from_both_tabs_and_preserves_loadout() {
        let (data, spec) = setup();
        let mut menu = Menu::new(
            AircraftId::F18,
            vec![AircraftId::F18, AircraftId::Ac130],
            &data,
            &spec,
            RevivalWeapons::Missiles,
            |_| false,
        )
        .unwrap();
        let before = LoadoutSpec::of(&menu.load);
        for tab in [0, 1] {
            menu.widget.show_tab(tab);
            let last = menu.tree[tab].children.len() - 1;
            for _ in 0..last {
                menu.key("ArrowDown", &data, &spec);
            }
            menu.key("Enter", &data, &spec);
            assert!(menu.brief_open);
            assert_eq!(LoadoutSpec::of(&menu.load), before);
            menu.brief_open = false;
        }
        menu.key("m", &data, &spec);
        assert!(menu.brief_open);
        assert_eq!(LoadoutSpec::of(&menu.load), before);
    }
    #[test]
    fn respawn_overlay_uses_source_station_order_and_escape_discards() {
        let (data, spec) = setup();
        let mut menu = Menu::new(
            AircraftId::F18,
            vec![AircraftId::F18, AircraftId::Ac130],
            &data,
            &spec,
            RevivalWeapons::Missiles,
            |_| false,
        )
        .unwrap();
        assert_eq!(
            menu.tree[0].children.len(),
            menu.load.station_display_order().len() + 4
        );
        let before = LoadoutSpec::of(&menu.load);
        assert!(matches!(menu.key("Escape", &data, &spec), Outcome::Cancel));
        assert_eq!(LoadoutSpec::of(&menu.load), before);
        menu.widget.show_tab(1);
        menu.select(
            menu.aircraft
                .iter()
                .position(|id| *id == AircraftId::Ac130)
                .unwrap(),
            &data,
            &spec,
        );
        assert_eq!(menu.load.aircraft, AircraftId::Ac130);
        assert!(matches!(
            menu.done(&data, &spec),
            Outcome::Done(AircraftId::Ac130, _)
        ));
    }
    #[test]
    fn the_fuel_row_turns_up_on_a_click_and_down_on_a_right_click_wrapping_both_ways() {
        let (data, spec) = setup();
        let mut menu = Menu::new(
            AircraftId::F18,
            vec![],
            &data,
            &spec,
            RevivalWeapons::Guns,
            |_| false,
        )
        .unwrap();
        let row = menu.load.station_display_order().len();
        let full = menu.load.internal_capacity_lbs;
        let tenth = full * 0.1;
        menu.load.fuel_lbs = tenth;
        // Down from the lowest wraps to full, then a tenth at a time.
        assert!(matches!(
            menu.select_back(row, &data, &spec),
            Outcome::Click
        ));
        assert!((menu.load.fuel_lbs - full).abs() < 1e-6);
        menu.select_back(row, &data, &spec);
        assert!((menu.load.fuel_lbs - (full - tenth)).abs() < 1e-6);
        // Up goes to full and wraps to the lowest.
        menu.select(row, &data, &spec);
        assert!((menu.load.fuel_lbs - full).abs() < 1e-6);
        menu.select(row, &data, &spec);
        assert!((menu.load.fuel_lbs - tenth).abs() < 1e-6);
        // A part tenth steps down no lower than a tenth.
        menu.load.fuel_lbs = tenth * 1.5;
        menu.select_back(row, &data, &spec);
        assert!((menu.load.fuel_lbs - tenth).abs() < 1e-6);
        // The station rows, the aircraft list and the buttons do not turn.
        let fuel = menu.load.fuel_lbs;
        assert!(matches!(menu.select_back(0, &data, &spec), Outcome::None));
        assert!(matches!(
            menu.select_back(row + 1, &data, &spec),
            Outcome::None
        ));
        assert_eq!(menu.load.fuel_lbs, fuel);
        menu.widget.show_tab(1);
        assert!(matches!(menu.select_back(row, &data, &spec), Outcome::None));
    }
    #[test]
    fn respawn_overlay_store_cycles_keep_the_host_loading_rules() {
        let (data, spec) = setup();
        let mut menu = Menu::new(
            AircraftId::F18,
            vec![],
            &data,
            &spec,
            RevivalWeapons::Guns,
            |_| false,
        )
        .unwrap();
        for index in 0..menu.load.station_display_order().len() {
            for _ in 0..5 {
                menu.select(index, &data, &spec);
                menu.check(&menu.load, &data, &spec).unwrap();
            }
        }
        let mut heavy = menu.load.clone();
        heavy.maximum_lbs = 0.;
        assert!(
            heavy
                .validate()
                .unwrap_err()
                .to_string()
                .contains("too heavy")
        );
    }
}
