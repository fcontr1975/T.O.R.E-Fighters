//! The shared vocabulary of events, fields, channels, units and outcomes.
//! The recorder writes these names and the exports read them, so both sides
//! agree without a format change. A producer may add fields beyond those
//! listed; readers ignore what they do not know.
//!
//! Conventions: `subject` is the actor (shooter, speaker, sender, the aircraft
//! that changed), `object` the other party (target, killer, recipient).
//! Distances are feet unless the field name says otherwise; `_kt` is knots,
//! `_nm` nautical miles, `_deg` degrees, `_s` seconds. Comms entries share one
//! outcome vocabulary ([`outcome`]).

/// Event kinds.
pub mod kind {
    /// A weapon left its launcher. Subject: shooter. Object: intended target.
    /// Fields: `projectile` (Id), `weapon` (Id of a WeaponInfo), `class`
    /// (Text), `mode` (Text: seeker or fire mode), `range_ft`, `aspect_deg`,
    /// `off_boresight_deg`, `closure_kt`, `shooter_alt_ft`, `target_alt_ft`,
    /// `shooter_speed_kt`, `target_speed_kt` (Num).
    pub const WEAPON_LAUNCH: &str = "weapon.launch";
    /// A guided weapon's own seeker started searching. Subject: shooter.
    /// Object: target. Fields: `projectile`, `weapon`, `range_ft`.
    pub const WEAPON_SEEKER_ACTIVE: &str = "weapon.seeker_active";
    /// An active radar missile locked on by itself. Subject: shooter.
    /// Object: target. Fields: `projectile`, `range_ft`.
    pub const WEAPON_PITBULL: &str = "weapon.pitbull";
    /// A guided weapon lost its target. Subject: shooter. Object: target.
    /// Fields: `projectile`, `reason` (Text), `range_ft`.
    pub const WEAPON_TRACK_LOST: &str = "weapon.track_lost";
    /// A guided weapon followed a decoy. Subject: shooter. Object: the
    /// aircraft that released the decoy. Fields: `projectile`, `decoy`
    /// (Text: chaff or flare), `roll` and `threshold` (Int: the decoy roll
    /// passed when roll < threshold), `roll_bound` (Int: 100 or 10000; absent
    /// older entries use 100), `susceptibility` and `effectiveness`
    /// (Int, percent), `number` (Int: the chaff cartridge or flare that fooled
    /// it, the `number` of its `combat.countermeasure`; absent in recordings
    /// made before it was kept), `reason`.
    pub const WEAPON_DECOYED: &str = "weapon.decoyed";
    /// How a shot ended. Subject: shooter. Object: intended target. Fields:
    /// `projectile`, `result` (Text from [`super::outcome`]: hit, missed,
    /// spoofed, jammed), `damage` (Int), `hp_after` (Int), `reason` (Text),
    /// `miss_ft` (Num: the closest the shot came to its target), `replaces`
    /// (Text from [`super::outcome`]). One per shot, except that a missile
    /// recorded as spoofed which flies on and damages an aircraft gets a
    /// second, `hit`, with `replaces` spoofed: the later one stands. That
    /// hit's `reason` names the chaff or flare that fooled the missile, as in
    /// `it was decoyed by chaff #7 from Enemy 2-1 at 0:13.5 but flew on and
    /// struck anyway`; `#7` is the `number` of that device's
    /// `combat.countermeasure` entry.
    pub const WEAPON_OUTCOME: &str = "weapon.outcome";
    /// Damage landed. Subject: attacker. Object: the aircraft or surface
    /// object hit. Fields: `projectile`, `weapon`, `damage` (Int), `hp_after`
    /// (Int), `section` (Int).
    pub const COMBAT_HIT: &str = "combat.hit";
    /// Something was destroyed. Subject: what was destroyed. Object: killer.
    /// Fields: `weapon` (Id), `projectile` (Id), `reason` (Text).
    pub const COMBAT_DESTROYED: &str = "combat.destroyed";
    /// A warhead burst in the air near its target. Subject: shooter. Object:
    /// target. Fields: `projectile`, `miss_ft`, `damage`.
    pub const COMBAT_AIRBURST: &str = "combat.airburst";
    /// A weapon burst on the ground, once per burst. Subject: shooter. Fields:
    /// `x_ft`, `y_ft`, `z_ft`, and `projectile`, `weapon` when a weapon
    /// vanished that tick.
    pub const COMBAT_GROUND_IMPACT: &str = "combat.ground_impact";
    /// A chaff cartridge or flare left an aircraft. Subject: the aircraft.
    /// Fields: `decoy` (Text: chaff or flare), `number` (Int: the device's
    /// number among the flight's releases, from 1, which sets its look),
    /// `left` (Int: the aircraft's devices of that kind left, when known),
    /// and the aircraft as it released it: `x_ft`, `y_ft`, `z_ft`
    /// ([`super::field::POSITION`]), its velocity
    /// ([`super::field::VELOCITY`]) and its attitude as unit vectors
    /// ([`super::field::BASIS`]), all exact, so a viewer can fly the device
    /// again as combat flew it; and `after_tick` (Int: the combat tick after
    /// whose step it left; its next step is its first). The player's own is
    /// recorded on the tick on screen when it left, an AI aircraft's on the
    /// tick whose AI step released it.
    pub const COMBAT_COUNTERMEASURE: &str = "combat.countermeasure";
    /// A range reset removed every released chaff cloud and flare and
    /// restarted their numbering. Fields: `after_tick`.
    pub const COMBAT_COUNTERMEASURES_CLEARED: &str = "combat.countermeasures_cleared";
    /// An aircraft hit the ground. Subject: the aircraft. Fields: `speed_kt`,
    /// `reason`.
    pub const AIRCRAFT_CRASHED: &str = "aircraft.crashed";
    /// A pilot ejected. Subject: the aircraft. Fields: `reason`.
    pub const AIRCRAFT_EJECTED: &str = "aircraft.ejected";
    /// A pilot died. Subject: the aircraft. Object: killer, if any. Fields:
    /// `reason`.
    pub const AIRCRAFT_PILOT_KILLED: &str = "aircraft.pilot_killed";
    /// Wheels left the ground. Subject: the aircraft. Fields: `airport` (Id).
    pub const AIRCRAFT_TOOK_OFF: &str = "aircraft.took_off";
    /// Wheels touched down and stayed. Subject: the aircraft. Fields:
    /// `airport` (Id), `grade` (Text), `sink_fps` (Num).
    pub const AIRCRAFT_LANDED: &str = "aircraft.landed";
    /// An engine stopped. Subject: the aircraft. Fields: `reason`.
    pub const AIRCRAFT_FLAMEOUT: &str = "aircraft.flameout";
    /// Fuel ran out. Subject: the aircraft.
    pub const AIRCRAFT_FUEL_OUT: &str = "aircraft.fuel_out";
    /// The flight model changed departure mode. Subject: the aircraft.
    /// Fields: `from`, `to` (Text), `reason`.
    pub const FLIGHT_DEPARTURE: &str = "flight.departure";
    /// A stall began or ended. Subject: the aircraft. Fields: `on` (Bool),
    /// `aoa_deg`, `speed_kt`.
    pub const FLIGHT_STALL: &str = "flight.stall";
    /// A spin began or ended. Subject: the aircraft. Fields: `on` (Bool),
    /// `direction` (Text: left or right).
    pub const FLIGHT_SPIN: &str = "flight.spin";
    /// The pilot held the stick at its stop, asking for more G than the
    /// limit allowed. Subject: the aircraft. Fields: `asked` (Num, G: what
    /// the aircraft's envelope offers at this speed), `limit` (Num, G: the
    /// limit applied), `g` (Num: delivered), `reason` (what set the limit).
    pub const FLIGHT_G_LIMIT: &str = "flight.g_limit";
    /// The structure failed. Subject: the aircraft. Fields: `section` (Int),
    /// `g` (Num), `reason`.
    pub const FLIGHT_STRUCTURAL_FAILURE: &str = "flight.structural_failure";
    /// A flight-model effect started or stopped applying. Subject: the
    /// aircraft. Fields: `effect` (Text), `on` (Bool), `factor` (Text: the
    /// factor, limit or state it applied), `reason` (Text: the because
    /// line), `momentary` (Bool: an effect of one moment, such as a
    /// touchdown, which has no "off"). Tacview shows these as debug events.
    pub const FLIGHT_EFFECT: &str = "flight.effect";
    /// AI activity changed. Subject: the aircraft. Fields: `from`, `to`
    /// (Text), `for_s` (Num: how long the previous activity lasted),
    /// `reason`.
    pub const AI_ACTIVITY: &str = "ai.activity";
    /// AI target changed. Subject: the aircraft. Object: the new target.
    /// Fields: `from`, `to` (Id), `priority` (Text), `score` (Num),
    /// `reason`.
    pub const AI_TARGET: &str = "ai.target";
    /// AI weapon service phase changed. Subject: the aircraft. Object:
    /// target. Fields: `from`, `to` (Text), `station` (Int), `reason`.
    pub const AI_WEAPON_PHASE: &str = "ai.weapon_phase";
    /// AI started or changed a defensive reaction. Subject: the aircraft.
    /// Object: the threat's shooter. Fields: `threat` (Id of the projectile),
    /// `reaction` (Text), `range_ft`, `reason`, and for a launch warning
    /// `launch_range_ft` and `delay_s`.
    pub const AI_DEFENSE: &str = "ai.defense";
    /// AI fell back to a simpler behaviour. Subject: the aircraft. Fields:
    /// `from`, `to`, `reason`.
    pub const AI_FALLBACK: &str = "ai.fallback";
    /// An AI strike decision (slice ST-G). Subject: the aircraft. Object: the
    /// target it would attack. Fields: `decision` (Text), `reason` (Text, a
    /// fixed key: `no_laze_manual` when an aircraft with laser bombs, no
    /// designator and no spot drops them by hand as unguided bombs,
    /// `no_laze_rtb` when its laser store cannot fall unguided and it goes
    /// home), `station` (Int) and `weapon` (Text: the store).
    pub const AI_STRIKE: &str = "ai.strike";
    /// AI airfield phase changed (taxi, takeoff, landing). Subject: the
    /// aircraft. Fields: `from`, `to`, `airport` (Id), `reason`.
    pub const AI_AIRFIELD_PHASE: &str = "ai.airfield_phase";
    /// AI decided to eject, or checked and did not. Subject: the aircraft.
    /// Fields: `decision` (Text), `reason`.
    pub const AI_EJECTION: &str = "ai.ejection";
    /// An order between aircraft. Subject: sender. Fields: `message` (Int,
    /// unique per recording), `recipients` (Ids), `order` (Text), `trigger`,
    /// `reason`, and for a single recipient `outcome`. Each recipient's
    /// answer is its own [`COMMS_DELIVERY`].
    pub const COMMS_ORDER: &str = "comms.order";
    /// A request, for example a wingman asking for help. Same fields as an
    /// order.
    pub const COMMS_REQUEST: &str = "comms.request";
    /// A silent data report, for example an attack passed to escorts. Same
    /// fields as an order, plus `about` (Id) and `kept_s` (Num).
    pub const COMMS_REPORT: &str = "comms.report";
    /// One recipient's outcome for an order, request or report. Subject:
    /// recipient. Object: sender. Fields: `message` (Int), `outcome`,
    /// `reason`, `wait_s`.
    pub const COMMS_DELIVERY: &str = "comms.delivery";
    /// A radio call. Subject: speaker. Fields: `speaker` (Text label),
    /// `stems` (Text: the recordings, space separated), `route` (Text, from
    /// [`super::route`]: how the cockpit plays it), `kind` (Text: chatter or
    /// important), `audience` (Text), `trigger` (Text; see
    /// [`super::trigger`]), `rolls` (Text: each random draw with its test),
    /// `heard` (Bool: the player could hear it), `outcome`, `reason`,
    /// `wait_s`, `due_s` (Num: the delay before a queued call is due),
    /// `message` (Int: the call's number, shared by every entry about it).
    /// Text: the words. Each line has one entry per thing that happened to
    /// it, so a line the player heard is the one [`super::heard`] accepts.
    pub const COMMS_RADIO: &str = "comms.radio";
    /// A crew remark in the player's cockpit. Same fields as radio.
    pub const COMMS_CREW: &str = "comms.crew";
    /// A tower line. Same fields as radio, plus `airport` (Id).
    pub const COMMS_TOWER: &str = "comms.tower";
    /// A cockpit HUD message. Fields: `outcome`, `reason`. Text: the message.
    pub const COMMS_HUD: &str = "comms.hud";
    /// A seeker or lock tone changed. Subject: the aircraft. Fields: `tone`
    /// (Text, from [`super::tone`]), `on` (Bool), `strength` (Num: its
    /// loudness from 0 to 1), `surface` (Bool: the weapon aims at surface
    /// targets, which changes the sound an infrared lock plays), `reason`.
    /// A new entry also marks a change in strength of 0.05 or more.
    pub const AUDIO_TONE: &str = "audio.tone";
    /// The stall warning started or stopped. Subject: the aircraft. Fields:
    /// `on` (Bool).
    pub const AUDIO_STALL_WARNING: &str = "audio.stall_warning";
    /// The situation music's inputs changed. Fields: `from`, `to` (Text: the
    /// score they ask for), `outcome`, `reason` (Text: why each input is
    /// on), and every input in [`super::music`] (Bool), so a replay can
    /// hand the music the same inputs.
    pub const AUDIO_MUSIC: &str = "audio.music";
    /// A sound effect played. Subject: the source, if any. Fields: `sound`
    /// (Text), `x_ft`, `y_ft`, `z_ft`, `volume`, and `own` (Bool: released
    /// by the player's own aircraft, so its cockpit hears it centered).
    pub const AUDIO_EFFECT: &str = "audio.effect";
    /// A weapon release sound played. Subject: the aircraft. Fields: `sound`,
    /// `weapon` (Id).
    pub const AUDIO_RELEASE: &str = "audio.release";
    /// An ejection cue played. Subject: the aircraft. Fields: `sound`.
    pub const AUDIO_EJECTION: &str = "audio.ejection";
    /// A device sound played (gear, flaps, brake, hook, bay). Subject: the
    /// aircraft. Fields: `device` (Text), `sound`.
    pub const AUDIO_DEVICE: &str = "audio.device";
    /// A player command reached the game. Subject: the player's aircraft.
    /// Fields: `command` (Text). Text: what it did.
    pub const PLAYER_COMMAND: &str = "player.command";
    /// The target the player's target views (F4, F7, F8) follow changed:
    /// the displayed target, or a dropped one the pilot can still see
    /// within visual range. Subject: the player's aircraft. Object: the new
    /// view target, none when there is none. Fields: `from` (Id: the one
    /// before, when there was one), `held` (Bool: the sensors dropped it
    /// and the views keep it by sight), `reason`. A new entry marks a change
    /// of target or of `held`; the first frame carries the starting state,
    /// so a recording with none predates them.
    pub const PLAYER_VIEW_TARGET: &str = "player.view_target";
    /// The player marked a moment (Ctrl+B). Text: the note, if any.
    pub const PLAYER_BOOKMARK: &str = "player.bookmark";
    /// The game paused.
    pub const SYSTEM_PAUSE: &str = "system.pause";
    /// The game resumed.
    pub const SYSTEM_RESUME: &str = "system.resume";
    /// Time compression changed. Fields: `scale` (Num).
    pub const SYSTEM_TIME_SCALE: &str = "system.time_scale";
    /// A cheat was switched. Fields: `cheat` (Text), `on` (Bool).
    pub const SYSTEM_CHEAT: &str = "system.cheat";
    /// The mission restarted.
    pub const SYSTEM_RESTART: &str = "system.restart";
    /// The mission ended. Fields: `reason` (Text).
    pub const SYSTEM_END: &str = "system.end";
    /// The recorder fell behind and skipped ticks. Fields: `from`, `to`
    /// (Int ticks, the missing range).
    pub const SYSTEM_GAP: &str = "system.gap";
    /// A free-form note, for example what a headless probe does not run.
    pub const SYSTEM_NOTE: &str = "system.note";
    /// A gun burst a network host reported, in a replay converted from a
    /// client's capture. Subject: the shooter. Fields: `station` (Int),
    /// `length_ticks` (Int, absent while the burst was still firing).
    pub const WEAPON_GUN_BURST: &str = "weapon.gun_burst";
    /// The network's figures for the second ending at this tick, in a replay
    /// converted from a client's capture: `client_seconds` (Num) and the
    /// diagnostics log's `stats` fields (`round_trip_ms`, `loss_percent`,
    /// `snapshot_loss_percent`, `snapshot_spread_ms`, `input_margin_ticks`,
    /// `interpolation_delay_ms`, `clock_rate`, `corrections`,
    /// `corrections_shown`, `mismatches`, `frames`, `extrapolated`,
    /// `inputs_repeated`, `bytes_up_per_second`, `bytes_down_per_second`,
    /// `predicted_tick`, `render_tick`, all Num; a figure the log leaves as
    /// `-` is absent).
    pub const NET_STATS: &str = "net.stats";
    /// Any other line of the client's diagnostics log (a join, a seating, a
    /// refusal, a mission, a debrief, a drop). Fields: `client_seconds` (Num),
    /// `kind` (Text: the line's kind). Text: the rest of the line.
    pub const NET_EVENT: &str = "net.event";

    /// A plane joined its flight's data link, the first tick the link sees
    /// it. Subject: the plane. Fields: `radar` (Bool: its aircraft has a
    /// radar, so its scope marks the picture). Every aircraft of a side is a
    /// member, so a recording has one at the start for each plane in play.
    pub const DATALINK_MEMBER: &str = "datalink.member";
    /// A member took a radar lock on an aircraft. Subject: the member.
    /// Object: the target.
    pub const DATALINK_LOCK: &str = "datalink.lock";
    /// A member let go of its lock (the target, the member or the lock is
    /// gone). Subject: the member. Object: the target it held.
    pub const DATALINK_UNLOCK: &str = "datalink.unlock";
    /// A flight's lead gave a wingman a target to attack. Subject: the lead.
    /// Object: the wingman. Fields: `target` (Id), `order` (Text: the order
    /// that gave it). The call that voiced it is the order's own
    /// [`COMMS_ORDER`] and radio entries.
    pub const DATALINK_ASSIGN: &str = "datalink.assign";
    /// An assignment ended. Subject: the member that held it. Object: the
    /// target. Fields: `reason` (Text: the order, the member or its target
    /// lost, or the lead changed).
    pub const DATALINK_CLEAR: &str = "datalink.clear";
    /// A member locked the target it was assigned, the first time. Subject:
    /// the member. Object: the target.
    pub const DATALINK_ACKNOWLEDGE: &str = "datalink.acknowledge";
    /// A human was told that a flightmate holds a lock on the same aircraft
    /// the human holds, when the lead did not mean it (a HUD line and a
    /// beep). Subject: the human's plane. Object: the aircraft both hold.
    /// Fields: `other` (Id: the flightmate).
    pub const DATALINK_SORT_WARNING: &str = "datalink.sort_warning";

    /// A surface unit's gun fired a burst, or its launcher a salvo (format
    /// 3; the rounds themselves are also `weapon.launch`). Subject: the
    /// unit. Object: its target. Fields: `weapon` (Text: the record),
    /// `mount` (Int: the hardpoint), `rounds` (Int), `refused` (Int: rounds
    /// the world could not place), `opening` (Bool: a flak battery's opening
    /// barrage), `flak` (Bool), `surface` (Bool, true when the target is a
    /// ground unit or ship of the other side; absent at an aircraft, and in
    /// recordings made before surface units fought each other).
    pub const SURFACE_BURST: &str = "surface.burst";
    /// A surface unit's engagement changed phase (search, track, fire,
    /// reload and so on). Subject: the unit. Object: its target. Fields:
    /// `station` (Int: which of its weapons), `to` (Text), `surface` (Bool,
    /// as for [`SURFACE_BURST`]).
    pub const SURFACE_PHASE: &str = "surface.phase";
    /// A supply truck rearmed every empty rail of a launcher. Subject: the
    /// unit. Fields: `loaded` (Int: missiles on its rails after).
    pub const SURFACE_REARM: &str = "surface.rearm";
    /// A supply truck added a spare magazine to a gun. Subject: the unit.
    /// Fields: `mount` (Int), `reserve` (Int: spare magazines after).
    pub const SURFACE_REFILL: &str = "surface.refill";
    /// A surface radar came on or went off. Subject: the unit. Fields: `on`
    /// (Bool), `reason` (Text).
    pub const SURFACE_RADAR: &str = "surface.radar";
    /// A surface unit was destroyed and lies as a wreck. Subject: the unit.
    /// Object: what destroyed it. Fields: `burning` (Bool), `fire_ft` (Num:
    /// the wreck fire's width when it burns), `hp_before` (Int).
    pub const SURFACE_WRECK: &str = "surface.wreck";

    /// A bomb, rocket or guided bomb left its station: one entry for each
    /// trigger call, so a pack is one entry and a held rocket pod one entry
    /// for each rocket it fires. Subject: the aircraft. Object: the ground
    /// target it had designated, if any. Fields: `weapon` (Id), `class`
    /// (Text: bomb or rocket), `station` (Int), `count` (Int: the rounds this
    /// call released), `projectile` (Id: the first) and `projectiles` (Ids,
    /// in pack order, so a round's place in the list is its pack index),
    /// `delivery` (Text from [`super::delivery`]), `left` (Int: the rounds
    /// the station holds after, when the aircraft is a flown one), `salvo`
    /// (Int, rockets: this rocket's number in the run fired from the station
    /// without a gap of a second), `range_ft` (Num: to the target at
    /// release). Each round is also an ordinary [`WEAPON_LAUNCH`].
    pub const WEAPON_RELEASE: &str = "weapon.release";
    /// A bomb landed before its fuze armed and did nothing but raise a puff
    /// of dirt (format 3 vocabulary, 2026-10-11). Subject: the shooter.
    /// Fields: `projectile`, `weapon`, `x_ft`, `y_ft`, `z_ft` (where it
    /// landed), `arm_s` (Num: the fuze's arming time), `flown_s` (Num: how
    /// long it fell), `reason`. The shot's [`WEAPON_OUTCOME`] says missed.
    pub const WEAPON_DUD: &str = "weapon.dud";
    /// A cluster bomb burst on the ground and scattered its submunitions
    /// (the replay draws their flashes from the burst's explosion). Subject:
    /// the shooter. Fields: `projectile`, `weapon`, `x_ft`, `y_ft`, `z_ft`,
    /// `explosion` (Int: the explosion type of the burst).
    pub const WEAPON_CLUSTER: &str = "weapon.cluster";
    /// A guided air-to-ground weapon (laser bomb or missile, JDAM, AT-2)
    /// was released or changed its guidance mode. The tree channel of the
    /// same family is [`super::channel::WEAPON_GUIDANCE`]; this is the event
    /// log's account. Subject: the shooter. Object: the object it follows,
    /// when it follows one. Fields: `projectile`, `weapon`, `guidance`
    /// (Text from [`super::guidance`]), `mode` (Text from [`super::guide`]),
    /// `from` (Text: the mode before, absent at release), `released` (Bool:
    /// this is the release), `spot_owner` (Id: whose laser spot a laser
    /// weapon follows) with `own` (Bool: the spot is the shooter's own, not a
    /// buddy's), `stored` (Bool: the point is the one stored at release),
    /// `x_ft`, `y_ft`, `z_ft` (the JDAM's stored point, or the point a laser
    /// weapon last homed on), `reason`.
    pub const WEAPON_GUIDED: &str = "weapon.guided";
    /// An aircraft's laser designator changed state. Subject: the lasing
    /// aircraft. Object: the object the spot sits on, if any. Fields:
    /// `state` (Text from [`super::laser`]), `on` (Bool: the spot is on the
    /// target), `how` (Text: automatic or by hand), `mask` (Text: range,
    /// gimbal or terrain, while masked), `range_ft`, and the spot as
    /// `x_ft`, `y_ft`, `z_ft` while it lases.
    pub const PLAYER_LASER: &str = "player.laser";
    /// A flown aircraft designated a ground target or pinned a ground point,
    /// or gave one up. Subject: the aircraft. Object: the designated ground
    /// object, absent for a pinned point or none. Fields: `channel` (Text
    /// from [`super::designate`]: how), `from` (Id: the object before),
    /// `x_ft`, `y_ft`, `z_ft` (the point of a pin), `reason`. Air targets
    /// are not recorded here (the radar and the views have their own
    /// entries).
    pub const PLAYER_DESIGNATE: &str = "player.designate";
    /// A lasing aircraft's spot moved (FX-R, 2026-10-11). Written while the
    /// laser is on, at most ten times a second and only when the spot has
    /// moved more than a few feet since the last entry, so the replay can
    /// draw where the spot was at any moment; [`PLAYER_LASER`] has the
    /// changes of state. Subject: the lasing aircraft. Object: the object
    /// under the spot, if any. Fields: `x_ft`, `y_ft`, `z_ft`.
    pub const PLAYER_LASER_SPOT: &str = "player.laser_spot";
    /// The AC-130's sensor console and orbit hold (slice nr-orbit,
    /// 2026-10-13): one entry when it opens or closes, and while it is open
    /// whenever the point it circles has moved more than 50 ft, at most once
    /// a second. Subject: the recording's player. Fields: `on` (Bool: the
    /// console is open), and while open the point as `x_ft`, `y_ft`, `z_ft`
    /// and the planned `radius_ft` and `height_ft` above it.
    pub const PLAYER_ORBIT: &str = "player.orbit";
    /// Where a guided weapon in the guiding mode was steering moved (FX-R,
    /// 2026-10-11), sampled like [`PLAYER_LASER_SPOT`]: the spot a laser
    /// weapon follows, the point a JDAM flies to, the target an AT-2 is
    /// steered at. Subject: the shooter. Fields: `projectile`, `spot_owner`
    /// (Id, a laser weapon), `x_ft`, `y_ft`, `z_ft`. The last entry before a
    /// weapon goes blind is the point it was homing on then.
    pub const WEAPON_AIM: &str = "weapon.aim";
    /// A bomb or guided weapon reached the ground (FX-R, 2026-10-11).
    /// Subject: the shooter. Object: the ground target the shooter had
    /// designated, if any. Fields: `projectile`, `weapon`, `x_ft`, `y_ft`,
    /// `z_ft` (where it landed), `guided` (Bool), `mode` (Text from
    /// [`super::guide`], a guided weapon's mode at the end), `aim_x_ft`,
    /// `aim_y_ft`, `aim_z_ft` and `miss_ft` (the point it was steering for
    /// and how far from it the weapon landed, a guided weapon that had
    /// one), `target_ft` (how far from the designated target), `reason`.
    pub const WEAPON_IMPACT: &str = "weapon.impact";

    /// An aircraft's drawn stations, recorded the first time it is recorded
    /// with any and whenever the layout changes (B3, 2026-10-12): the
    /// `stores.station` entries that follow on the same tick are its
    /// stations in order. Subject: the aircraft. Fields: `count` (Int).
    pub const STORES_LAYOUT: &str = "stores.layout";
    /// One drawn station of the layout before it. Subject: the aircraft.
    /// Fields: `slot` (Int: its place in the layout), `hardpoint` (Int: the
    /// source record), `x_ft`, `y_ft`, `z_ft` (Num: the record's position,
    /// right, up and forward of the reference point), `location` (Int: the
    /// source location byte), `paired` (Bool), `look` (Text: store, pod or
    /// tank), `items` (Int: whole stores on it), `loaded` (Int: whole stores
    /// it started with), `shape` (Text, when the store has one),
    /// `length_ft` and `diameter_ft` (Num, a fitted pod or tank body).
    pub const STORES_STATION: &str = "stores.station";
    /// The whole stores now on each station of the aircraft's layout, when
    /// any changed (a release, a jettison, a rearm). Subject: the aircraft.
    /// Fields: `items` (Ids: one count a station, in layout order).
    pub const STORES_ITEMS: &str = "stores.items";
    /// The HUD the aircraft was showing, sampled at 24 Hz for human cockpits
    /// and on changes for aircraft without a cockpit readout.
    /// (nr-replayhud, John 2026-10-16). Subject: the aircraft. Object: the
    /// display target, when there is one. Fields: `nav`, `armed`, `station`,
    /// `weapon` (Text: the source name), `rounds`, `launch` (Text: cued or
    /// bore), `seeker` (Text: the status label), `delivery` (Text, empty or
    /// CCIP or LVL), `readiness` (Text), `guidance`, `in_range`, `hit` (Int,
    /// percent), `min_ft` (absent older entries use the decoded minimum),
    /// `max_ft`, `band_min`, `band_max` (`band_max` 0 means no
    /// band), `radar`, and `mount_x`, `mount_y`, `mount_z` when the station
    /// is not at the reference point. A recording made before this event
    /// still plays: the HUD camera keeps the derived picture. Version 4
    /// includes target_x/y/z and velocity_x/y/z from the displayed target,
    /// including ground objects. See docs/formats/replay-display.md.
    pub const AIRCRAFT_HUD: &str = "aircraft.hud";
    /// The targeting sight or AC-130 gunsight. Human cockpits use 24 Hz;
    /// legacy and non-cockpit samples remain sparse. Subject:
    /// the aircraft. Object: the tracked object, when the sight tracks one.
    /// Fields: `gunship`, `az`, `el` (Num, radians), `zoom` (Int), `mode`
    /// (Text: free, point, track or return), `x_ft`, `y_ft`, `z_ft` when a
    /// point is held, and for a gunship `r0` `r1` `r2` (rounds), `link`,
    /// `lead` (255 is none), `s0` `s1` `s2` (readiness labels), `radar`,
    /// `range` (the range step), `h0` `e0` `h1` `e1` `h2` `e2` (train
    /// radians, omitted when the trains were not known) and `lock_x`
    /// `lock_y` `lock_z` when the guns are locked. Absent on an older
    /// recording: the Sensor View keeps the derived line of sight. Version 4
    /// adds display_24hz, gimbal_limit, impact0 through impact2 positions,
    /// ranges and spent flags, mark coordinates and packed radar_dots.
    /// See docs/formats/replay-display.md for their bounded grammar.
    pub const AIRCRAFT_SIGHT: &str = "aircraft.sight";

    /// Every kind above, in the order listed.
    pub const ALL: &[&str] = &[
        WEAPON_LAUNCH,
        WEAPON_SEEKER_ACTIVE,
        WEAPON_PITBULL,
        WEAPON_TRACK_LOST,
        WEAPON_DECOYED,
        WEAPON_OUTCOME,
        COMBAT_HIT,
        COMBAT_DESTROYED,
        COMBAT_AIRBURST,
        COMBAT_GROUND_IMPACT,
        COMBAT_COUNTERMEASURE,
        COMBAT_COUNTERMEASURES_CLEARED,
        AIRCRAFT_CRASHED,
        AIRCRAFT_EJECTED,
        AIRCRAFT_PILOT_KILLED,
        AIRCRAFT_TOOK_OFF,
        AIRCRAFT_LANDED,
        AIRCRAFT_FLAMEOUT,
        AIRCRAFT_FUEL_OUT,
        FLIGHT_DEPARTURE,
        FLIGHT_STALL,
        FLIGHT_SPIN,
        FLIGHT_G_LIMIT,
        FLIGHT_STRUCTURAL_FAILURE,
        FLIGHT_EFFECT,
        AI_ACTIVITY,
        AI_TARGET,
        AI_WEAPON_PHASE,
        AI_DEFENSE,
        AI_FALLBACK,
        AI_STRIKE,
        AI_AIRFIELD_PHASE,
        AI_EJECTION,
        COMMS_ORDER,
        COMMS_REQUEST,
        COMMS_REPORT,
        COMMS_DELIVERY,
        COMMS_RADIO,
        COMMS_CREW,
        COMMS_TOWER,
        COMMS_HUD,
        AUDIO_TONE,
        AUDIO_STALL_WARNING,
        AUDIO_MUSIC,
        AUDIO_EFFECT,
        AUDIO_RELEASE,
        AUDIO_EJECTION,
        AUDIO_DEVICE,
        PLAYER_COMMAND,
        PLAYER_VIEW_TARGET,
        PLAYER_BOOKMARK,
        SYSTEM_PAUSE,
        SYSTEM_RESUME,
        SYSTEM_TIME_SCALE,
        SYSTEM_CHEAT,
        SYSTEM_RESTART,
        SYSTEM_END,
        SYSTEM_GAP,
        SYSTEM_NOTE,
        WEAPON_GUN_BURST,
        NET_STATS,
        NET_EVENT,
        DATALINK_MEMBER,
        DATALINK_LOCK,
        DATALINK_UNLOCK,
        DATALINK_ASSIGN,
        DATALINK_CLEAR,
        DATALINK_ACKNOWLEDGE,
        DATALINK_SORT_WARNING,
        SURFACE_BURST,
        SURFACE_PHASE,
        SURFACE_REARM,
        SURFACE_REFILL,
        SURFACE_RADAR,
        SURFACE_WRECK,
        WEAPON_RELEASE,
        WEAPON_DUD,
        WEAPON_CLUSTER,
        WEAPON_GUIDED,
        PLAYER_LASER,
        PLAYER_DESIGNATE,
        PLAYER_LASER_SPOT,
        PLAYER_ORBIT,
        WEAPON_AIM,
        WEAPON_IMPACT,
        STORES_LAYOUT,
        STORES_STATION,
        STORES_ITEMS,
        AIRCRAFT_HUD,
        AIRCRAFT_SIGHT,
    ];
}

/// Event field names.
pub mod field {
    pub const PROJECTILE: &str = "projectile";
    pub const WEAPON: &str = "weapon";
    pub const CLASS: &str = "class";
    pub const MODE: &str = "mode";
    pub const RANGE_FT: &str = "range_ft";
    pub const ASPECT_DEG: &str = "aspect_deg";
    pub const OFF_BORESIGHT_DEG: &str = "off_boresight_deg";
    pub const CLOSURE_KT: &str = "closure_kt";
    pub const SHOOTER_ALT_FT: &str = "shooter_alt_ft";
    pub const TARGET_ALT_FT: &str = "target_alt_ft";
    pub const SHOOTER_SPEED_KT: &str = "shooter_speed_kt";
    pub const TARGET_SPEED_KT: &str = "target_speed_kt";
    pub const RESULT: &str = "result";
    pub const DAMAGE: &str = "damage";
    pub const HP_AFTER: &str = "hp_after";
    pub const MISS_FT: &str = "miss_ft";
    /// The result an outcome withdraws, from [`super::outcome`].
    pub const REPLACES: &str = "replaces";
    pub const SECTION: &str = "section";
    pub const DECOY: &str = "decoy";
    pub const REASON: &str = "reason";
    pub const FROM: &str = "from";
    pub const TO: &str = "to";
    pub const ON: &str = "on";
    pub const DIRECTION: &str = "direction";
    pub const ASKED: &str = "asked";
    pub const LIMIT: &str = "limit";
    pub const G: &str = "g";
    pub const AOA_DEG: &str = "aoa_deg";
    pub const SPEED_KT: &str = "speed_kt";
    pub const SINK_FPS: &str = "sink_fps";
    pub const GRADE: &str = "grade";
    pub const AIRPORT: &str = "airport";
    pub const EFFECT: &str = "effect";
    pub const PRIORITY: &str = "priority";
    pub const SCORE: &str = "score";
    pub const STATION: &str = "station";
    pub const THREAT: &str = "threat";
    pub const REACTION: &str = "reaction";
    pub const DECISION: &str = "decision";
    pub const MESSAGE: &str = "message";
    pub const RECIPIENTS: &str = "recipients";
    pub const ORDER: &str = "order";
    pub const ABOUT: &str = "about";
    pub const KEPT_S: &str = "kept_s";
    pub const SPEAKER: &str = "speaker";
    pub const STEMS: &str = "stems";
    pub const ROUTE: &str = "route";
    pub const TRIGGER: &str = "trigger";
    pub const HEARD: &str = "heard";
    pub const OUTCOME: &str = "outcome";
    pub const WAIT_S: &str = "wait_s";
    pub const TONE: &str = "tone";
    pub const SOUND: &str = "sound";
    pub const DEVICE: &str = "device";
    pub const X_FT: &str = "x_ft";
    pub const Y_FT: &str = "y_ft";
    pub const Z_FT: &str = "z_ft";
    pub const VOLUME: &str = "volume";
    pub const COMMAND: &str = "command";
    pub const SCALE: &str = "scale";
    pub const CHEAT: &str = "cheat";
    pub const FACTOR: &str = "factor";
    pub const MOMENTARY: &str = "momentary";
    pub const FOR_S: &str = "for_s";
    pub const ROLL: &str = "roll";
    pub const ROLLS: &str = "rolls";
    pub const THRESHOLD: &str = "threshold";
    pub const ROLL_BOUND: &str = "roll_bound";
    pub const SUSCEPTIBILITY: &str = "susceptibility";
    pub const EFFECTIVENESS: &str = "effectiveness";
    pub const LAUNCH_RANGE_FT: &str = "launch_range_ft";
    pub const DELAY_S: &str = "delay_s";
    pub const DUE_S: &str = "due_s";
    pub const AUDIENCE: &str = "audience";
    /// A radio call's kind: chatter or important.
    pub const KIND: &str = "kind";
    /// Which producer made a comms entry, from [`super::source`].
    pub const SOURCE: &str = "source";
    /// A seeker tone's loudness, 0 to 1.
    pub const STRENGTH: &str = "strength";
    /// A seeker tone's weapon aims at surface targets; a surface unit's burst
    /// or engagement is at a ground unit or ship.
    pub const SURFACE: &str = "surface";
    /// A released device's number among the flight's releases.
    pub const NUMBER: &str = "number";
    /// Devices of the kind released that are left.
    pub const LEFT: &str = "left";
    /// The combat tick after whose step a released device left, or a range
    /// reset cleared the devices.
    pub const AFTER_TICK: &str = "after_tick";
    /// The player's own aircraft made the sound.
    pub const OWN: &str = "own";
    /// The views keep a target the sensors dropped, by sight.
    pub const HELD: &str = "held";
    /// A plane's aircraft has a radar.
    pub const RADAR: &str = "radar";
    /// The aircraft an assignment names.
    pub const TARGET: &str = "target";
    /// The other member of a pair, such as the flightmate in a sort warning.
    pub const OTHER: &str = "other";
    /// A surface unit's hardpoint.
    pub const MOUNT: &str = "mount";
    /// Rounds in a surface burst.
    pub const ROUNDS: &str = "rounds";
    /// Rounds the world refused.
    pub const REFUSED: &str = "refused";
    /// A flak battery's opening barrage.
    pub const OPENING: &str = "opening";
    /// A flak shell.
    pub const FLAK: &str = "flak";
    /// Missiles on a launcher's rails.
    pub const LOADED: &str = "loaded";
    /// Spare magazines a gun keeps.
    pub const RESERVE: &str = "reserve";
    /// A wreck still burns.
    pub const BURNING: &str = "burning";
    /// NAV is selected: the HUD shows the navigation picture.
    pub const NAV: &str = "nav";
    /// A weapon is armed.
    pub const ARMED: &str = "armed";
    /// Seeker launch mode: `cued` or `bore`.
    pub const LAUNCH: &str = "launch";
    /// The mounted seeker's status label.
    pub const SEEKER: &str = "seeker";
    /// The shot is inside the weapon's range.
    pub const IN_RANGE: &str = "in_range";
    /// Maximum range of the shot, feet. Zero means none was estimated.
    pub const MIN_FT: &str = "min_ft";
    pub const MAX_FT: &str = "max_ft";
    /// Near edge of the firing band, feet.
    pub const BAND_MIN: &str = "band_min";
    /// Far edge of the firing band, feet. Zero means there is no band.
    pub const BAND_MAX: &str = "band_max";
    /// The station's mount, feet right, up and forward.
    pub const MOUNT_X: &str = "mount_x";
    pub const MOUNT_Y: &str = "mount_y";
    pub const MOUNT_Z: &str = "mount_z";
    /// The sight belongs to an AC-130 gunsight.
    pub const GUNSHIP: &str = "gunship";
    /// Sight azimuth, radians, body relative.
    pub const AZ: &str = "az";
    /// Sight elevation, radians, body relative.
    pub const EL: &str = "el";
    /// Sight or sensor zoom step.
    pub const ZOOM: &str = "zoom";
    /// Linked-gun membership, one bit a slot.
    pub const LINK: &str = "link";
    /// The lead gun's slot. 255 means none.
    pub const LEAD: &str = "lead";
    /// A weapon or gun readiness label.
    pub const READINESS: &str = "readiness";
    /// The radar range step, an index on the scope ladder.
    pub const RANGE_STEP: &str = "range";
    /// A wreck fire's width, feet.
    pub const FIRE_FT: &str = "fire_ft";
    /// Hit points before a change.
    pub const HP_BEFORE: &str = "hp_before";
    /// A bomb or rocket's delivery: [`super::delivery`].
    pub const DELIVERY: &str = "delivery";
    /// Rounds one trigger call released.
    pub const COUNT: &str = "count";
    /// The projectiles of a release, in pack order.
    pub const PROJECTILES: &str = "projectiles";
    /// A rocket's number in the run fired from its station.
    pub const SALVO: &str = "salvo";
    /// A guided weapon's guidance: [`super::guidance`].
    pub const GUIDANCE: &str = "guidance";
    /// A guided weapon is at its release.
    pub const RELEASED: &str = "released";
    /// The aircraft whose laser spot a weapon follows.
    pub const SPOT_OWNER: &str = "spot_owner";
    /// The point is the one stored at release.
    pub const STORED: &str = "stored";
    /// The laser's state: [`super::laser`].
    pub const STATE: &str = "state";
    /// How a laser was turned on: automatic or by hand.
    pub const HOW: &str = "how";
    /// A flag: the cockpit is one a human gave back to the AI, which flies
    /// the aircraft while its laser and designation stay as the human left
    /// them (slice FX-K). Absent when a human flies it.
    pub const KEPT: &str = "kept";
    /// Why a laser is masked.
    pub const MASK: &str = "mask";
    /// How a ground target was designated: [`super::designate`].
    pub const CHANNEL: &str = "channel";
    /// A fuze's arming time, seconds.
    pub const ARM_S: &str = "arm_s";
    /// Seconds a weapon flew.
    pub const FLOWN_S: &str = "flown_s";
    /// An explosion's type.
    pub const EXPLOSION: &str = "explosion";
    /// The point a guided weapon was steering for, feet, world axes.
    pub const AIM: [&str; 3] = ["aim_x_ft", "aim_y_ft", "aim_z_ft"];
    /// How far a bomb landed from the target the shooter had designated.
    pub const TARGET_FT: &str = "target_ft";
    /// A flag: the weapon is a guided one.
    pub const GUIDED: &str = "guided";
    /// A position in feet, world axes.
    pub const POSITION: [&str; 3] = [X_FT, Y_FT, Z_FT];
    /// An orbit's planned radius and height above its centre (`player.orbit`).
    pub const RADIUS_FT: &str = "radius_ft";
    pub const HEIGHT_FT: &str = "height_ft";
    /// A velocity in feet per second, world axes.
    pub const VELOCITY: [&str; 3] = ["vx_fps", "vy_fps", "vz_fps"];
    /// An attitude as the right, up and forward unit vectors, world axes.
    pub const BASIS: [&str; 9] = [
        "right_x",
        "right_y",
        "right_z",
        "up_x",
        "up_y",
        "up_z",
        "forward_x",
        "forward_y",
        "forward_z",
    ];
}

/// How a bomb or rocket was delivered, for the `delivery` field of
/// `weapon.release`.
pub mod delivery {
    /// A bomb released with the dive pipper (CCIP) as the sight.
    pub const CCIP: &str = "ccip";
    /// A bomb released with the level "T" as the sight.
    pub const LEVEL: &str = "level";
    /// A rocket fired with the rocket pipper.
    pub const ROCKET: &str = "rocket";
    /// A guided bomb, which then flies its own guidance.
    pub const GUIDED: &str = "guided";
    /// Released by an aircraft with no bomb sight of its own (an AI wing or
    /// a plane the recorder cannot see inside).
    pub const UNGUIDED: &str = "unguided";
}

/// How a guided air-to-ground weapon is guided, for the `guidance` field of
/// `weapon.guided`.
pub mod guidance {
    /// Follows a laser spot.
    pub const LASER: &str = "laser";
    /// Flies to a stored point (the JDAMs).
    pub const COORDINATE: &str = "coordinate";
    /// Steered at a designated object by its launcher (the AT-2).
    pub const COMMAND: &str = "command";
}

/// A guided weapon's mode, for the `mode` and `from` fields of
/// `weapon.guided`.
pub mod guide {
    /// A laser weapon released with no spot: it may lock one once.
    pub const WAITING: &str = "waiting";
    /// Homing.
    pub const GUIDING: &str = "guiding";
    /// Guidance lost for good, or never had: it falls or flies on.
    pub const BLIND: &str = "blind";
}

/// A laser designator's state, for the `state` field of `player.laser`.
pub mod laser {
    /// No designator, or the sight holds no point.
    pub const OFF: &str = "off";
    /// The sight holds a point and the laser is off.
    pub const READY: &str = "ready";
    /// The spot is on the target.
    pub const LASING: &str = "lasing";
    /// Wanted, but out of range, outside the gimbal or blocked by terrain.
    pub const MASKED: &str = "masked";
}

/// How a ground target was designated, for the `channel` field of
/// `player.designate`.
pub mod designate {
    /// The air-to-ground radar page holds the object.
    pub const RADAR: &str = "a-g radar";
    /// The infrared (FLIR) page holds it.
    pub const INFRARED: &str = "infrared";
    /// The HARM page holds it.
    pub const HARM: &str = "harm page";
    /// The targeting sight tracks it.
    pub const SIGHT: &str = "sight";
    /// A designation without either: by sight, or the Easy targeting key.
    pub const VISUAL: &str = "visual";
    /// A ground point the sight holds with no object on it.
    pub const PIN: &str = "pin";
    /// The designation or pin was given up.
    pub const CLEARED: &str = "cleared";
}

/// Seeker tone names, for the `tone` field of `audio.tone`.
pub mod tone {
    pub const RADAR_LOCK: &str = "radar lock";
    pub const RADAR_SEARCH: &str = "radar search";
    pub const INFRARED_LOCK: &str = "infrared lock";
    /// An infrared seeker searching for a surface target.
    pub const GROUND: &str = "ground";
    pub const INFRARED_SEARCH: &str = "infrared search";
}

/// How the cockpit plays a comms line, for the `route` field.
pub mod route {
    /// The radio: queued behind other radio speech.
    pub const RADIO: &str = "radio";
    /// Airport speech: the tower's own queue.
    pub const TOWER: &str = "tower";
    /// Played straight into the cockpit, such as the player's death scream.
    pub const DIRECT: &str = "direct";
}

/// Triggers a replay acts on, for the `trigger` field. Other triggers are
/// plain English and may change wording.
pub mod trigger {
    /// The tower's answer to the player's own request, which replaces tower
    /// speech still queued or playing.
    pub const PLAYER_REQUEST: &str = "player request";
    /// The runway under the player's landing clearance became unusable,
    /// which cuts tower speech still queued or playing.
    pub const CLEARANCE_CANCELLED: &str = "landing clearance cancelled";
}

/// The situation music's inputs, as Bool fields of `audio.music`. Each
/// matches the input of the same name the game's music takes.
pub mod music {
    /// The mission has succeeded.
    pub const SUCCEEDED: &str = "succeeded";
    /// The player has ejected.
    pub const EJECTED: &str = "ejected";
    /// The player is on the takeoff roll or climbing out after it.
    pub const LAUNCHING: &str = "launching";
    /// A designated enemy aircraft is within air range.
    pub const AIR_TARGET: &str = "air_target";
    /// A round hit the player recently.
    pub const HIT_RECENTLY: &str = "hit_recently";
    /// A designated enemy beyond air range, an AI aiming a missile at the
    /// player, or a missile guided at the player.
    pub const DANGER: &str = "danger";
    /// The home condition has been reached this flight.
    pub const HOME: &str = "home";
    /// The player is parked, or rolling out after a landing.
    pub const DECK: &str = "deck";
    /// Every input, in the order the game declares them.
    pub const ALL: [&str; 8] = [
        SUCCEEDED,
        EJECTED,
        LAUNCHING,
        AIR_TARGET,
        HIT_RECENTLY,
        DANGER,
        HOME,
        DECK,
    ];
}

/// Whether a `comms.radio`, `comms.crew` or `comms.tower` entry is a line
/// the player heard at its tick: `heard` is not false and the outcome is
/// missing or delivered. A line's queued, held-back and cut-off entries
/// are not, so each heard line counts once.
pub fn heard(event: &crate::Event) -> bool {
    event.flag(field::HEARD) != Some(false)
        && event
            .string(field::OUTCOME)
            .is_none_or(|o| o == outcome::DELIVERED)
}

/// Producers of comms entries, for the `source` field.
pub mod source {
    /// An aircraft's radio call.
    pub const RADIO: &str = "radio";
    /// A wingman's radio answer to the player's order.
    pub const REPLY: &str = "reply";
    /// An AI radio event before it becomes a call; its rules decide which
    /// events are said, so a held one is routine.
    pub const CHATTER: &str = "chatter";
    /// The player's crew, or the wingman coaching a single-seat player.
    pub const CREW: &str = "crew";
    /// Airport speech and the wingmen's airfield status.
    pub const TOWER: &str = "tower";
    /// A text line the AI posts on the HUD.
    pub const HUD: &str = "hud";
    /// A player order to the wing.
    pub const ORDER: &str = "order";
    /// The situation music.
    pub const MUSIC: &str = "music";
}

/// Display tree channels.
pub mod channel {
    /// An AI aircraft's thinking: mission, activity, target and why,
    /// weapon, defense, motion, steering, controls, fuel, recent changes.
    pub const AI_THOUGHT: &str = "ai.thought";
    /// Flight-model telemetry: air data, load, power, drag, and the effects
    /// applied this tick with their because lines.
    pub const FLIGHT_TELEMETRY: &str = "flight.telemetry";
    /// A guided weapon's seeker and steering.
    pub const WEAPON_GUIDANCE: &str = "weapon.guidance";
}

/// Well-known node labels that exports look for in display trees. Values
/// are numbers in the unit named on the node.
pub mod node {
    /// Height above the ground under the aircraft (`ft`). The summary's
    /// lowest height and Tacview's AGL come from here.
    pub const AGL: &str = "AGL";
    /// Mach number (no unit).
    pub const MACH: &str = "Mach";
    /// Angle of attack (`deg`).
    pub const AOA: &str = "AoA";
    /// Sideslip angle (`deg`).
    pub const SIDESLIP: &str = "Sideslip";
    /// True airspeed (`kt`).
    pub const TAS: &str = "TAS";
    /// Load factor (`g`).
    pub const LOAD: &str = "G";
    /// Structural G limit (`g`).
    pub const G_LIMIT: &str = "G limit";
    /// AI activity (Text).
    pub const ACTIVITY: &str = "Activity";
    /// AI target (Id).
    pub const TARGET: &str = "Target";
}

/// Units for tree nodes and documentation.
pub mod unit {
    pub const FT: &str = "ft";
    pub const FT_S: &str = "ft/s";
    pub const KT: &str = "kt";
    pub const NM: &str = "nm";
    pub const DEG: &str = "deg";
    pub const DEG_S: &str = "deg/s";
    pub const G: &str = "g";
    pub const LB: &str = "lb";
    pub const LB_FT2: &str = "lb/ft2";
    pub const S: &str = "s";
    pub const PERCENT: &str = "%";
    pub const RATIO: &str = "x";
}

/// Outcomes shared by weapon results and comms entries.
pub mod outcome {
    pub const HIT: &str = "hit";
    pub const MISSED: &str = "missed";
    pub const SPOOFED: &str = "spoofed";
    pub const JAMMED: &str = "jammed";
    pub const QUEUED: &str = "queued";
    pub const DELIVERED: &str = "delivered";
    pub const DROPPED: &str = "dropped";
    pub const CANCELLED: &str = "cancelled";
    pub const SUPPRESSED: &str = "suppressed";
    pub const REPLACED: &str = "replaced";
    pub const EXPIRED: &str = "expired";
    pub const APPLIED: &str = "applied";
    pub const REJECTED: &str = "rejected";
    /// Said, but the player's radio does not receive it.
    pub const UNHEARD: &str = "unheard";
    /// Delivered, then cut off.
    pub const INTERRUPTED: &str = "interrupted";
    /// An order's answers are in, one delivery per recipient.
    pub const ANSWERED: &str = "answered";
    /// A check that found nothing to say.
    pub const SILENT: &str = "silent";
    /// A state change, not a message.
    pub const NOTED: &str = "noted";
    /// Delivered or offered, but not acted on.
    pub const IGNORED: &str = "ignored";
    /// A recipient that was not asked: bugged out, human or landed.
    pub const SKIPPED: &str = "skipped";
}

#[cfg(test)]
mod tests {
    use super::{field, kind, outcome};
    use crate::Event;

    #[test]
    fn a_line_is_heard_once_at_its_delivery() {
        let line = || Event::new(kind::COMMS_RADIO);
        let said = |result: &str, heard: Option<bool>| {
            let event = line().with(field::OUTCOME, result);
            match heard {
                Some(heard) => event.with(field::HEARD, heard),
                None => event,
            }
        };
        assert!(super::heard(&said(outcome::DELIVERED, Some(true))));
        assert!(super::heard(&line()), "a recording without outcomes");
        assert!(!super::heard(&said(outcome::QUEUED, None)));
        assert!(!super::heard(&said(outcome::INTERRUPTED, Some(true))));
        assert!(!super::heard(&said(outcome::UNHEARD, Some(false))));
        assert!(!super::heard(&line().with(field::HEARD, false)));
    }

    #[test]
    fn kinds_are_unique_dotted_names() {
        let mut seen = std::collections::HashSet::new();
        for kind in super::kind::ALL {
            assert!(seen.insert(*kind), "duplicate kind {kind}");
            let (family, name) = kind.split_once('.').expect("family.name");
            assert!(!family.is_empty() && !name.is_empty());
            assert!(
                kind.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_')
            );
        }
    }
}
