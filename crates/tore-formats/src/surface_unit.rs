//! Bounded reader for NT active-object definitions: SAM launchers, AAA guns,
//! ships, vehicles, men and the GCI radar. An NT is the OBJECT and NPC blocks of
//! a PT with no PLANE block, followed by its hardpoint rows. The layout, the
//! field meanings and the census of all 84 retail records are in
//! docs/formats/surface-units.md. Nothing here runs a callback: `utilProc`
//! stays a symbol name.
use crate::{
    Result,
    aircraft::{Brf, Token, fields, schema},
    invalid,
};

/// `slewLimitH` and `slewLimitP` and `slewH` and `slewP` count 182 units per
/// degree: SA-6 pitch 12740 is 70 degrees, a Nimitz CIWIS arc 21840 is 120.
pub const ANGLE_UNITS_PER_DEGREE: i32 = 182;
/// `maxItems` of a gun hardpoint that never runs dry.
pub const UNLIMITED_ROUNDS: i32 = 32767;
/// The soft supply vehicles the retail equipment lists and templates draw:
/// `<vehicle>` picks TRUCK and TANKER, the Pakistan and Vietnam convoys name
/// MISTRK ("SAM-Carrying Truck").
pub const TRUCK_TYPES: [&str; 3] = ["TRUCK.NT", "TANKER.NT", "MISTRK.NT"];
/// The trucks that resupply a group (plan 2.11, John item 19): a live one
/// within 0.1 mile refills SAM rails and AAA magazines. TANKER carries fuel
/// and does not count.
pub const SUPPLY_TRUCKS: [&str; 2] = ["TRUCK.NT", "MISTRK.NT"];
/// The most hit points a shoulder-fired SAM team has: the men's and the
/// MANPADS teams' 5 (every vehicle and launcher has 100 or more).
pub const TEAM_MAX_HIT_POINTS: i32 = 5;
/// The fastest a shoulder-fired SAM team moves (`_maxSpeed`): the men's and
/// the MANPADS teams' 10 (SAM vehicles 50).
pub const TEAM_MAX_SPEED: i32 = 10;
/// The retail "Destroyed Vehicle" static object (shape `dest.SH`). The record
/// names no wreck; this is the only candidate in the archive (see the doc).
pub const DESTROYED_VEHICLE_OBJECT: &str = "DEST.OT";
/// Class words (`obj_class`), the same table the debrief sorts kills by.
pub mod class {
    pub const SHIP: u16 = 0x2000;
    pub const SAM: u16 = 0x1000;
    pub const AAA: u16 = 0x0800;
    pub const TANK: u16 = 0x0400;
    pub const VEHICLE: u16 = 0x0200;
    pub const STRUCTURE: u16 = 0x0100;
    pub const OTHER: u16 = 0x0040;
}
const MAX_MOUNTS: i32 = 64;
/// OBJECT plus NPC is 186 bytes in the type size; each hardpoint row is 24.
const TYPE_SIZE_BASE: i32 = 186;
const TYPE_SIZE_PER_MOUNT: i32 = 24;

/// A BRF number with its `^` feet marker kept apart from the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quantity {
    pub raw: i32,
    /// Written `^N` in the file. `_acc ^50` is 50 feet per second squared.
    pub scaled: bool,
}

/// OBJECT movement words. Units are not established beyond the waypoint speed
/// (feet per second, decision 12.2); read them raw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Movement {
    /// `_turnRate`, 182 units per degree per second (ships 910, tanks 2730).
    pub turn_rate: i32,
    pub min_speed: i32,
    pub corner_speed: i32,
    pub max_speed: i32,
    pub acceleration: Quantity,
    pub deceleration: Quantity,
}

/// NPC block: the search and preparation delays the AI service reads. Times
/// are quarter seconds (AI B42).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Npc {
    pub flags: u32,
    /// `ctName`: an AI script name (SARAN names `HYDRO.BI`).
    pub script: Option<String>,
    pub search_frequency: i32,
    pub unready_attack: i32,
    pub attack: i32,
    /// 32767 except KS-12 and KS-19 (40).
    pub retarget: i32,
    /// Nonzero only on the A_M1939 barrage zone (195).
    pub zone_dist: i32,
}

/// What a hardpoint holds, from the extension of its `defaultTypeName`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountKind {
    /// A `.JT` weapon record.
    Weapon,
    /// A `.SEE` sensor record (GCIR.SEE on the GCI, REDCR.SEE on Red Crown).
    Sensor,
    /// No default store.
    Empty,
}

/// Rounds on a mount.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ammo {
    /// Guns: `maxItems` 32767.
    Unlimited,
    /// Missile rails: the finite load (SA-2 1 per rail, SA-6 3, SA-15 8).
    Rounds(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// `name`: always 0 in the retail NTs.
    pub location: u8,
    /// 8 or 10 in the retail NTs; the bit 0x2 is not understood.
    pub flags: i32,
    /// `pos.x/y/z` in hull source units (feet at an unestablished scale).
    pub position: [i32; 3],
    /// `slewH`, `slewP`: where the mount points at rest, hull relative. A
    /// Nimitz stern CIWS reads 32760 (180 degrees).
    pub slew: [i32; 2],
    /// `slewLimitH`, `slewLimitP`: the half-arc either side of `slew`.
    pub slew_limit: [i32; 2],
    pub kind: MountKind,
    /// `defaultTypeName`, upper case ("SA6.JT").
    pub store: Option<String>,
    /// `maxItems`.
    pub count: i32,
    pub weight_class: i32,
}

impl Mount {
    pub fn ammo(&self) -> Ammo {
        if self.count >= UNLIMITED_ROUNDS || self.count < 0 {
            Ammo::Unlimited
        } else {
            Ammo::Rounds(self.count as u32)
        }
    }
    /// Rest direction, `[heading, pitch]` in degrees.
    pub fn slew_degrees(&self) -> [f64; 2] {
        degrees(self.slew)
    }
    /// Half-arc either side of the rest direction, `[heading, pitch]` degrees.
    pub fn limit_degrees(&self) -> [f64; 2] {
        degrees(self.slew_limit)
    }
}

fn degrees(units: [i32; 2]) -> [f64; 2] {
    units.map(|v| f64::from(v) / f64::from(ANGLE_UNITS_PER_DEGREE))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceUnit {
    /// Upper case archive name, "SA6.NT".
    pub resource: String,
    pub short_name: String,
    pub name: String,
    /// `obj_class`, see [`class`].
    pub class: u16,
    /// OBJECT `flags` (0x821 ground units, 0x131 ships, 0x2000821 flak guns).
    pub object_flags: u32,
    /// `utilProc`: `_GVProc`, `_OBJProc`, `_CARRIERProc` and a few own procs.
    pub callback: String,
    /// Main shape, upper case. `None` for the invisible A_M1939 zone.
    pub shape: Option<String>,
    /// `shadowShape` (AA85B.SH on the flak guns).
    pub shadow_shape: Option<String>,
    /// The damaged look. The record names none; every ship has one by the
    /// retail naming rule, the main shape's stem plus `_A.SH` (`nimz.SH` to
    /// `NIMZ_A.SH`). Ground units and men have none (their wreck is
    /// [`DESTROYED_VEHICLE_OBJECT`]). The import test checks each exists.
    pub damaged_shape: Option<String>,
    pub hit_points: i32,
    /// `damage[0..4]`: 255 on every NT, meaning unresolved.
    pub damage: [i32; 5],
    /// `sigs[0..4]`: `[3]` radar, `[2]` infrared.
    pub signatures: [i32; 5],
    /// `maxVisDist`, raw (78 ground, 391 SA-2 and ships).
    pub max_visible: i32,
    pub year: i32,
    /// `expType`: 21 ground, 35 ship, 15 men.
    pub explosion: u8,
    pub crater: u8,
    pub damage_type: i32,
    pub debris_damaged: [i32; 3],
    pub debris_destroyed: [i32; 3],
    pub movement: Movement,
    pub npc: Npc,
    /// Every hardpoint in file order, sensors included; the index is the
    /// mount's identity for ammunition and reload state.
    pub mounts: Vec<Mount>,
    /// The first `.SEE` hardpoint's record, upper case.
    pub sensor: Option<String>,
}

impl SurfaceUnit {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let brf = Brf::parse(bytes)?;
        let root = brf.block("")?;
        let (o, n) = (schema::OBJECT.len(), schema::NPC.len());
        if root.len() != o + n {
            return Err(invalid("unsupported NT BRF layout"));
        }
        let object = fields(&root[..o], schema::OBJECT)?;
        let npc = fields(&root[o..], schema::NPC)?;
        if object["structType"].number()? != 3 {
            return Err(invalid("NT structType must be 3"));
        }
        let count = npc["numHards"].number()?;
        if !(0..=MAX_MOUNTS).contains(&count) {
            return Err(invalid("invalid NT hardpoint count"));
        }
        if object["typeSize"].number()? != TYPE_SIZE_BASE + TYPE_SIZE_PER_MOUNT * count {
            return Err(invalid("NT type size does not match its hardpoint count"));
        }
        let number = |key: &str| -> Result<i32> {
            let field = &object[key];
            if field.scaled {
                return Err(invalid("scaled NT statistic unsupported"));
            }
            field.number()
        };
        let names = single_block(&brf, &object["ot_names"], 3, "identity")?;
        let resource = names[2].to_ascii_uppercase();
        if !resource.ends_with(".NT") {
            return Err(invalid("NT identity must name its NT resource"));
        }
        let shape = optional_resource(&brf, &object["shape"])?;
        let shadow_shape = optional_resource(&brf, &object["shadowShape"])?;
        let callback = object["utilProc"].clone();
        if callback.kind != "symbol" || callback.value.is_empty() {
            return Err(invalid("NT requires a utilProc symbol"));
        }
        let quantity = |key: &str| -> Result<Quantity> {
            let field = &object[key];
            Ok(Quantity {
                raw: field.number()?,
                scaled: field.scaled,
            })
        };
        let movement = Movement {
            turn_rate: number("_turnRate")?,
            min_speed: number("_minSpeed")?,
            corner_speed: number("_cornerSpeed")?,
            max_speed: number("_maxSpeed")?,
            acceleration: quantity("_acc")?,
            deceleration: quantity("_dacc")?,
        };
        let script = optional_resource(&brf, &npc["ctName"])?;
        let npc_block = Npc {
            flags: npc["flags"].number()? as u32,
            script,
            search_frequency: npc["searchFrequencyT"].number()?,
            unready_attack: npc["unreadyAttackT"].number()?,
            attack: npc["attackT"].number()?,
            retarget: npc["retargetT"].number()?,
            zone_dist: npc["zoneDist"].number()?,
        };
        let rows: &[Token] = if npc["hards"].kind == "ptr" {
            brf.block(&npc["hards"].value)?
        } else {
            &[]
        };
        if rows.len() != count as usize * schema::HARDPOINT.len() {
            return Err(invalid("NT hardpoint block does not match its count"));
        }
        let mut mounts = Vec::new();
        for row in rows.chunks_exact(schema::HARDPOINT.len()) {
            mounts.push(parse_mount(&brf, row)?);
        }
        let sensor = mounts
            .iter()
            .find(|mount| mount.kind == MountKind::Sensor)
            .and_then(|mount| mount.store.clone());
        let hit_points = number("hitPoints")?;
        let class = number("obj_class")? as u16;
        let signatures = collect([0, 1, 2, 3, 4].map(|i| number(&format!("sigs[{i}]"))))?;
        let damage = collect([0, 1, 2, 3, 4].map(|i| number(&format!("damage[{i}]"))))?;
        let vector = |prefix: &str| -> Result<[i32; 3]> {
            Ok([
                number(&format!("{prefix}.x"))?,
                number(&format!("{prefix}.y"))?,
                number(&format!("{prefix}.z"))?,
            ])
        };
        if hit_points < 0 || signatures.iter().any(|v| *v < 0) {
            return Err(invalid("negative NT hit points or signature"));
        }
        Ok(Self {
            resource,
            short_name: names[0].clone(),
            name: names[1].clone(),
            class,
            object_flags: object["flags"].number()? as u32,
            callback: callback.value,
            damaged_shape: damaged_shape(class, shape.as_deref()),
            shape,
            shadow_shape,
            hit_points,
            damage,
            signatures,
            max_visible: number("maxVisDist")?,
            year: number("year")?,
            explosion: byte(number("expType")?)?,
            crater: byte(number("craterSize")?)?,
            damage_type: number("dmgType")?,
            debris_damaged: vector("dmgDebrisPos")?,
            debris_destroyed: vector("dstDebrisPos")?,
            movement,
            npc: npc_block,
            mounts,
            sensor,
        })
    }

    /// Weapon hardpoints in file order, each with its mount index.
    pub fn weapons(&self) -> impl Iterator<Item = (usize, &Mount)> {
        self.mounts
            .iter()
            .enumerate()
            .filter(|(_, mount)| mount.kind == MountKind::Weapon)
    }
    /// Carries at least one weapon or a sensor (the plan's "active unit").
    pub fn armed(&self) -> bool {
        self.mounts.iter().any(|m| m.kind != MountKind::Empty)
    }
    /// True for TRUCK and MISTRK: the vehicles that resupply a group.
    pub fn is_supply_truck(&self) -> bool {
        SUPPLY_TRUCKS.contains(&self.resource.as_str())
    }
    /// True for TRUCK, TANKER and MISTRK: every soft supply vehicle.
    pub fn is_truck(&self) -> bool {
        TRUCK_TYPES.contains(&self.resource.as_str())
    }
    pub fn is_ship(&self) -> bool {
        self.class & class::SHIP != 0
    }
    /// A shoulder-fired SAM team (MANPADS: SA-7, SA-14, SA-16, FIM-92,
    /// Mistral in the retail data): a SAM-class unit, not a ship, with the
    /// hit points and the foot pace of men ([`TEAM_MAX_HIT_POINTS`],
    /// [`TEAM_MAX_SPEED`]). No flag marks them; this is the only rule that
    /// parts them from every vehicle and fixed launcher (docs/formats/
    /// surface-units.md, "Shoulder-fired teams").
    pub fn is_shoulder_fired_team(&self) -> bool {
        self.class & class::SAM != 0
            && !self.is_ship()
            && self.hit_points <= TEAM_MAX_HIT_POINTS
            && self.movement.max_speed <= TEAM_MAX_SPEED
    }
}

fn damaged_shape(class: u16, shape: Option<&str>) -> Option<String> {
    if class & class::SHIP == 0 {
        return None;
    }
    Some(format!("{}_A.SH", shape?.strip_suffix(".SH")?))
}

fn byte(value: i32) -> Result<u8> {
    u8::try_from(value).map_err(|_| invalid("NT byte field out of range"))
}

fn collect<const N: usize>(values: [Result<i32>; N]) -> Result<[i32; N]> {
    let mut out = [0; N];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = value?;
    }
    Ok(out)
}

fn valid_resource(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'~' | b'$' | b'&'))
}

pub(crate) fn single_block(
    brf: &Brf,
    token: &Token,
    count: usize,
    what: &str,
) -> Result<Vec<String>> {
    if token.kind != "ptr" || token.scaled {
        return Err(invalid(&format!("NT {what} must be a reference")));
    }
    let strings = brf.strings(&token.value)?;
    if strings.len() != count {
        return Err(invalid(&format!("NT {what} has the wrong string count")));
    }
    Ok(strings)
}

/// A pointer to one resource name, or a zero dword for none.
pub(crate) fn optional_resource(brf: &Brf, token: &Token) -> Result<Option<String>> {
    if token.kind != "ptr" {
        return if token.number()? == 0 && !token.scaled {
            Ok(None)
        } else {
            Err(invalid("NT resource reference must be a pointer or zero"))
        };
    }
    let name = single_block(brf, token, 1, "resource reference")?.remove(0);
    if !valid_resource(&name) {
        return Err(invalid("invalid NT resource reference"));
    }
    Ok(Some(name.to_ascii_uppercase()))
}

fn parse_mount(brf: &Brf, row: &[Token]) -> Result<Mount> {
    let f = fields(row, schema::HARDPOINT)?;
    let store = optional_resource(brf, &f["defaultTypeName"])?;
    let kind = match store.as_deref() {
        None => MountKind::Empty,
        Some(name) if name.ends_with(".JT") => MountKind::Weapon,
        Some(name) if name.ends_with(".SEE") => MountKind::Sensor,
        Some(_) => return Err(invalid("NT hardpoint holds an unreviewed record type")),
    };
    let limit = [f["slewLimitH"].number()?, f["slewLimitP"].number()?];
    if limit.iter().any(|v| *v < 0) {
        return Err(invalid("negative NT slew limit"));
    }
    Ok(Mount {
        location: byte(f["name"].number()?)?,
        flags: f["flags"].number()?,
        position: [
            f["pos.x"].number()?,
            f["pos.y"].number()?,
            f["pos.z"].number()?,
        ],
        slew: [f["slewH"].number()?, f["slewP"].number()?],
        slew_limit: limit,
        kind,
        store,
        count: f["maxItems"].number()?,
        weight_class: f["maxWeight"].number()?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `(slewH, slewP, limitH, limitP, store, maxItems)`.
    type MountSpec<'a> = (i32, i32, i32, i32, Option<&'a str>, i32);

    /// Synthetic NT text. `object` and `npc` override named fields; each mount
    /// is `(slewH, slewP, limitH, limitP, store, maxItems)`.
    pub(crate) fn fixture(
        names: [&str; 3],
        shape: Option<&str>,
        object: &[(&str, &str)],
        npc: &[(&str, &str)],
        mounts: &[MountSpec],
    ) -> String {
        let mut text = String::from("[brent's_relocatable_format]\n");
        let size = TYPE_SIZE_BASE + TYPE_SIZE_PER_MOUNT * mounts.len() as i32;
        for (kind, name) in schema::OBJECT {
            let default = match *name {
                "structType" => "3".to_owned(),
                "typeSize" => size.to_string(),
                "ot_names" => "ot_names".to_owned(),
                "shape" if shape.is_some() => "shape".to_owned(),
                "utilProc" => "_GVProc".to_owned(),
                "hitPoints" => "100".to_owned(),
                _ => "0".to_owned(),
            };
            let value = object
                .iter()
                .find(|(key, _)| key == name)
                .map_or(default, |(_, v)| (*v).to_owned());
            let kind = if *kind == "ptr" && value == "0" {
                "dword"
            } else {
                kind
            };
            text.push_str(&format!("{kind} {value}\n"));
        }
        for (kind, name) in schema::NPC {
            let default = match *name {
                "numHards" => mounts.len().to_string(),
                "hards" => "hards".to_owned(),
                _ => "0".to_owned(),
            };
            let value = npc
                .iter()
                .find(|(key, _)| key == name)
                .map_or(default, |(_, v)| (*v).to_owned());
            let kind = if *kind == "ptr" && value == "0" {
                "dword"
            } else {
                kind
            };
            text.push_str(&format!("{kind} {value}\n"));
        }
        text.push_str(":hards\n");
        for (i, (sh, sp, lh, lp, store, items)) in mounts.iter().enumerate() {
            let pointer = if store.is_some() {
                format!("ptr store{i}")
            } else {
                "dword 0".to_owned()
            };
            text.push_str(&format!(
                "word 8\nword {}\nword 30\nword 0\nword {sh}\nword {sp}\nword {lh}\nword {lp}\n{pointer}\nbyte 0\nword {items}\nbyte 0\n",
                i * 10
            ));
        }
        text.push_str(&format!(
            ":ot_names\nstring \"{}\"\nstring \"{}\"\nstring \"{}\"\n",
            names[0], names[1], names[2]
        ));
        if let Some(shape) = shape {
            text.push_str(&format!(":shape\nstring \"{shape}\"\n"));
        }
        for (i, (.., store, _)) in mounts.iter().enumerate() {
            if let Some(store) = store {
                text.push_str(&format!(":store{i}\nstring \"{store}\"\n"));
            }
        }
        text.push_str("end\n");
        text
    }

    fn sa6() -> SurfaceUnit {
        let text = fixture(
            ["SA-6", "SA-6 Gainful", "SA6.NT"],
            Some("sa6.SH"),
            &[
                ("obj_class", "$1000"),
                ("sigs[3]", "100"),
                ("expType", "21"),
                ("craterSize", "6"),
                ("_turnRate", "910"),
                ("_maxSpeed", "50"),
                ("_acc", "^50"),
            ],
            &[
                ("searchFrequencyT", "40"),
                ("unreadyAttackT", "144"),
                ("attackT", "60"),
                ("retargetT", "32767"),
            ],
            &[(0, 0, 0, 12740, Some("SA6.JT"), 3)],
        );
        SurfaceUnit::parse(text.as_bytes()).unwrap()
    }

    fn sam(hit_points: &str, max_speed: &str, class: &str) -> SurfaceUnit {
        let text = fixture(
            ["SA-7", "SA-7 Grail", "SA7.NT"],
            Some("sa7.SH"),
            &[
                ("obj_class", class),
                ("hitPoints", hit_points),
                ("_maxSpeed", max_speed),
            ],
            &[],
            &[(0, 0, 0, 16380, Some("SA7.JT"), 2)],
        );
        SurfaceUnit::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn a_shoulder_fired_team_is_a_sam_with_the_hit_points_and_pace_of_men() {
        // The retail teams (SA-7, SA-14, SA-16, FIM-92, Mistral): 5 and 10.
        assert!(sam("5", "10", "$1000").is_shoulder_fired_team());
        // A SAM vehicle (SA-9, SA-13): 100 and 50.
        assert!(!sam("100", "50", "$1000").is_shoulder_fired_team());
        assert!(!sa6().is_shoulder_fired_team());
        // The Crotale stands still at a foot's pace but has 100 hit points.
        assert!(!sam("100", "10", "$1000").is_shoulder_fired_team());
        // A soldier with a rifle is not a SAM.
        assert!(!sam("5", "10", "$0040").is_shoulder_fired_team());
    }

    #[test]
    fn reads_identity_class_movement_and_npc_delays() {
        let unit = sa6();
        assert_eq!(unit.resource, "SA6.NT");
        assert_eq!(
            (unit.short_name.as_str(), unit.name.as_str()),
            ("SA-6", "SA-6 Gainful")
        );
        assert_eq!(unit.class, class::SAM);
        assert_eq!(unit.shape.as_deref(), Some("SA6.SH"));
        assert_eq!(unit.callback, "_GVProc");
        assert_eq!((unit.explosion, unit.crater), (21, 6));
        assert_eq!(unit.signatures[3], 100);
        assert_eq!(unit.movement.turn_rate, 910);
        assert_eq!(unit.movement.max_speed, 50);
        assert_eq!(
            unit.movement.acceleration,
            Quantity {
                raw: 50,
                scaled: true
            }
        );
        // 40 / 144 / 60 quarter seconds: 10 s search, 36 s unready, 15 s ready.
        assert_eq!(
            (
                unit.npc.search_frequency,
                unit.npc.unready_attack,
                unit.npc.attack
            ),
            (40, 144, 60)
        );
        assert_eq!(unit.npc.retarget, 32767);
        assert!(unit.npc.script.is_none() && unit.damaged_shape.is_none());
    }

    #[test]
    fn mount_arcs_ammo_and_kind() {
        let unit = sa6();
        assert_eq!(unit.mounts.len(), 1);
        let mount = &unit.mounts[0];
        assert_eq!(mount.kind, MountKind::Weapon);
        assert_eq!(mount.store.as_deref(), Some("SA6.JT"));
        assert_eq!(mount.ammo(), Ammo::Rounds(3));
        assert_eq!(mount.limit_degrees(), [0.0, 70.0]);
        assert!(unit.armed() && unit.sensor.is_none());
        assert_eq!(unit.weapons().map(|(i, _)| i).collect::<Vec<_>>(), [0]);
    }

    #[test]
    fn ship_mounts_keep_rest_direction_and_unlimited_guns() {
        // A Nimitz-style stern gun: rest direction 180 degrees, arc 120.
        let text = fixture(
            ["Eisenhower", "Eisenhower", "NIMZ.NT"],
            Some("nimz.SH"),
            &[
                ("obj_class", "$2000"),
                ("expType", "35"),
                ("hitPoints", "4000"),
            ],
            &[],
            &[
                (0, 0, 21840, 16380, Some("PHALANX.JT"), 32767),
                (32760, 0, 21840, 16380, Some("SEA_SPAR.JT"), 32767),
            ],
        );
        let unit = SurfaceUnit::parse(text.as_bytes()).unwrap();
        assert!(unit.is_ship());
        assert_eq!(unit.damaged_shape.as_deref(), Some("NIMZ_A.SH"));
        assert_eq!(unit.mounts[1].slew_degrees(), [180.0, 0.0]);
        assert_eq!(unit.mounts[1].limit_degrees(), [120.0, 90.0]);
        assert_eq!(unit.mounts[1].ammo(), Ammo::Unlimited);
        assert_eq!(unit.hit_points, 4000);
    }

    #[test]
    fn sensor_hardpoint_and_ai_script() {
        let text = fixture(
            ["GCI Radar", "GCI Radar", "GCI.NT"],
            Some("king.SH"),
            &[("obj_class", "$100"), ("utilProc", "_OBJProc")],
            &[("flags", "$1")],
            &[(0, 0, 0, 0, Some("gcir.SEE"), 1)],
        );
        let unit = SurfaceUnit::parse(text.as_bytes()).unwrap();
        assert_eq!(unit.mounts[0].kind, MountKind::Sensor);
        assert_eq!(unit.sensor.as_deref(), Some("GCIR.SEE"));
        assert_eq!(unit.weapons().count(), 0);
        assert_eq!(unit.npc.flags, 1);
        assert!(unit.armed());
    }

    #[test]
    fn unarmed_units_and_supply_trucks() {
        let text = fixture(
            ["Truck", "Truck", "TRUCK.NT"],
            Some("truck.SH"),
            &[("obj_class", "$200")],
            &[],
            &[],
        );
        let unit = SurfaceUnit::parse(text.as_bytes()).unwrap();
        assert!(unit.mounts.is_empty() && !unit.armed());
        assert!(unit.is_supply_truck());
        let other = fixture(["T", "T", "T72.NT"], Some("t72.SH"), &[], &[], &[]);
        assert!(
            !SurfaceUnit::parse(other.as_bytes())
                .unwrap()
                .is_supply_truck()
        );
        for name in SUPPLY_TRUCKS {
            assert!(name.ends_with(".NT"));
        }
    }

    #[test]
    fn zone_object_has_no_shape() {
        let text = fixture(
            ["M1939 Zone", "M1939 37mm AAA Zone", "A_M1939.NT"],
            None,
            &[("obj_class", "$800"), ("hitPoints", "1")],
            &[("zoneDist", "195")],
            &[(0, 0, 0, 0, Some("A_M1939.JT"), 32767)],
        );
        let unit = SurfaceUnit::parse(text.as_bytes()).unwrap();
        assert!(unit.shape.is_none() && unit.damaged_shape.is_none());
        assert_eq!(unit.npc.zone_dist, 195);
        assert_eq!(unit.hit_points, 1);
    }

    #[test]
    fn named_ai_script_and_shadow_shape() {
        let mut text = fixture(
            ["Sarancha", "Sarancha", "SARAN.NT"],
            Some("saran.SH"),
            &[("obj_class", "$2000")],
            &[("ctName", "ctname")],
            &[],
        );
        text = text.replace("end\n", ":ctname\nstring \"hydro.BI\"\nend\n");
        let unit = SurfaceUnit::parse(text.as_bytes()).unwrap();
        assert_eq!(unit.npc.script.as_deref(), Some("HYDRO.BI"));
        assert_eq!(unit.damaged_shape.as_deref(), Some("SARAN_A.SH"));
    }

    #[test]
    fn rejects_malformed_records() {
        let good = fixture(
            ["A", "A", "A.NT"],
            Some("a.SH"),
            &[],
            &[],
            &[(0, 0, 0, 0, Some("A.JT"), 1)],
        );
        assert!(SurfaceUnit::parse(good.as_bytes()).is_ok());
        // A PT-sized root, a wrong structType, a type size that lies about the
        // hardpoints, an unreviewed store type and a negative arc all fail.
        let bad_type = good.replacen("byte 3\n", "byte 5\n", 1);
        assert!(SurfaceUnit::parse(bad_type.as_bytes()).is_err());
        let bad_size = good.replacen("word 210\n", "word 186\n", 1);
        assert!(SurfaceUnit::parse(bad_size.as_bytes()).is_err());
        let bad_store = good.replace("A.JT", "A.PT");
        assert!(SurfaceUnit::parse(bad_store.as_bytes()).is_err());
        let negative = good.replacen(
            "word 0\nword 0\nword 0\nword 0\nptr store0",
            "word 0\nword 0\nword -5\nword 0\nptr store0",
            1,
        );
        assert!(SurfaceUnit::parse(negative.as_bytes()).is_err());
        let short = good.replace("byte 0\nword 1\nbyte 0\n", "");
        assert!(SurfaceUnit::parse(short.as_bytes()).is_err());
        let wrong_name = good.replace("A.NT", "A.OT");
        assert!(SurfaceUnit::parse(wrong_name.as_bytes()).is_err());
        assert!(SurfaceUnit::parse(b"not brf").is_err());
        assert!(SurfaceUnit::parse(&vec![b'x'; 2 * 1024 * 1024]).is_err());
    }

    #[test]
    fn hostile_input_never_panics() {
        let good = fixture(
            ["A", "A", "A.NT"],
            Some("a.SH"),
            &[],
            &[],
            &[(0, 0, 0, 0, Some("A.JT"), 1)],
        );
        for cut in (0..good.len()).step_by(7) {
            let _ = SurfaceUnit::parse(&good.as_bytes()[..cut]);
        }
        for line in 0..good.lines().count() {
            let mutated = good
                .lines()
                .enumerate()
                .filter(|(i, _)| *i != line)
                .map(|(_, l)| l)
                .collect::<Vec<_>>()
                .join("\n");
            let _ = SurfaceUnit::parse(mutated.as_bytes());
        }
    }
}
