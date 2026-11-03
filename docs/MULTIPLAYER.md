# Multiplayer

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Status, 2026-10-09. This page began as the 2026-09-28 plan. Each section
carries *Built* notes that say what Milestone 2 has delivered so far and which
agent decisions the build settled; text marked *Designed* or *Planned* is not
built yet.

This is the design guide for Milestone 2 multiplayer. It is built from the
feature spec John wrote on 2026-09-28 and his planning answers the same day.
Every behaviour here is **opinionated**, requested by John on 2026-09-28, unless
it carries one of these marks:

- *Retail gap-fill (agent)*: where the feature spec is silent, retail
  multiplayer's rule applies. That is John's rule (2026-09-28). He named the
  revival and scoring settings, chat keys and receivers, airborne start, and the
  lock-box X and IFF squawk. Other items with this mark are an agent applying
  his rule, awaiting his review.
- *Agent proposal*: a suggestion awaiting John's review, never attributed to him.

- What Fighters Anthology's own multiplayer did: [retail multiplayer](spec/multiplayer.md).
- Where this design departs from retail: [relation to Fighters Anthology](#relation-to-fighters-anthology).
- Delivery stages, code findings and risks: [multiplayer plan](multiplayer-plan.md).

The client now prepares received missions on a background worker and shows a
[loading screen](spec/loading-screen.md). Transport updates and the existing
keepalives continue while it builds. Escape during mission preparation cancels
the join. Host and join content checks are polled too. Remaining validation is listed in that spec.

Loadout requests must obey the [even-paired-load rule](spec/stores-on-pylons.md#even-paired-loads).
The lobby's Load Ordnance page starts from the standard load, or the page
already edited in this session, so there is no saved multiplayer load to
migrate. Live requests, and plane loadouts written
into a server's mission text, with odd paired quantities or obsolete
capacities are rejected (an odd paired count says `Paired stores must be
loaded in pairs.`). The stores event layout is unchanged, but peers need the
same capacity and validation rules.

Bridge HP and destruction use the existing authoritative surface snapshots and
recorded surface HP. The [bridge correction](spec/bridges.md) changes shared
world geometry, including geometry rebuilt for old recordings, without adding
wire fields or changing replay/checkpoint layouts. Bridge END generated ruins
are selected at zero HP and use the same deterministic collapse on every client
and replay; span types retain their retail broken art.

## Contents

- [Goals](#goals)
- [Roles and identity](#roles-and-identity)
- [Lobby and Quick Mission flow](#lobby-and-quick-mission-flow)
  - [The lobby since the lobby pass](#the-lobby-since-the-lobby-pass): layout, scroll bars, Mission... for everyone, the ready hint, PvP sides, Sides and Autobalance
- [Slots, AI fill and handoff](#slots-ai-fill-and-handoff)
  - [AI respawn](#ai-respawn), [revival distance and separation](#revival-distance-and-separation), [lead hold](#lead-hold)
- [Rejoin and observers](#rejoin-and-observers)
- [Comms and chat](#comms-and-chat)
- [Flight data link](#flight-data-link)
- [Sim rules in multiplayer](#sim-rules-in-multiplayer)
  - [Ground targets online](#ground-targets-online): the surface world under protocol 22
  - [Air-to-ground online](#air-to-ground-online): radar pages, sights, the laser, bombs and guided weapons under protocol 23
- [Debrief](#debrief)
- [Architecture](#architecture)
- [Networking](#networking)
- [Replay and telemetry](#replay-and-telemetry)
- [Relation to Fighters Anthology](#relation-to-fighters-anthology)
- [Decisions](#decisions)
- [Open questions](#open-questions)

## Goals

Milestone 2 adds co-op multiplayer on top of the existing Quick Mission creator.
Humans take open wing slots, AI flies everything else, and a mission never
breaks when someone drops, including the host.

- One server core that runs both as a self-hosted headless dedicated server and
  inside a player's game.
- A public lobby with a server browser, backed by a small master server.
- Automatic host selection and seamless host migration.
- Seamless AI and human slot swapping, including join in progress and rejoin.
- A built-in flight data link so wingmen share targets, locks and assignments,
  limited by each aircraft's era.
- Connections that just work for most players, including behind carrier-grade
  NAT (CGNAT).

**Not in v1:**

- Accounts, or any identity beyond a callsign and a short-lived rejoin token.
- Anti-cheat. The host is trusted.
- Voice chat.
- Missions and campaigns beyond Quick Mission. The
  [roadmap](ROADMAP.md#milestone-2-multiplayer) extends multiplayer to them later.
- PvP balancing. PvP ships in v1 as a side choice, but tuning it is not a v1 target.
- Airbase Assault, the retail multiplayer-only mode ([retail](spec/multiplayer.md#airbase-assault)).
- Encryption. Traffic, passwords and tokens travel unencrypted; see
  [transport](#transport).

## Roles and identity

**King** (crown icon) controls the lobby. The mission's creator starts as King
and can hand the crown to another player. If the King leaves without handing it
off, the crown passes to the longest-connected human.

**Host** (house icon) is the machine running the server core. It is calculated
by default ([host selection](#host-selection)). The King can pin a specific
player, including themselves, or leave it calculated. Both roles show in the
lobby player list.

**Callsign.** There are no accounts. Each player types a callsign, which is only
a display name. If it is already taken in the session, the later joiner gets a
suffix: "Viper" and "Viper_2".

**Rejoin token.** On first join the server issues a token that the game saves in
the background. Tokens are specific to one server and expire after 24 hours.

Vehicle placement and canonical airfield ranking follow the deterministic rules
in [surface defenses](spec/surface-defenses.md#jitter). The creator offers only
the distances a fixed table gives for the theater, target and start, and a host
moves a mission's distance that the table leaves out the same way, with a log
line ([keeping the enemy on the map](spec/quick-mission-menu.md#keeping-the-enemy-on-the-map)).
A distance that still does not fit is refused at mission build instead of shortened. CAP checkpoints now include the optional map rectangle
that clips their region. These changes alter surface digests, scene positions and
checkpoint bytes; the release integration must handle the protocol/replay version
bump together with the other 0.2.2-rc1 changes.

The launcher, gun and added-radar footing limit is now a fitted 20%, matching
other vehicles; tanks keep 30% (John requested loosening on 2026-10-26). With a
ground target and a player ground start, the creator offers every distance because
the target determines the enemy position.

Hosts try at most sixteen consecutive seeds after a placement refusal, then send
only the successfully built seed. Startup, King's mission changes, restart and
rotation share this rule; clients never choose their own replacement. Each
successful choice is logged. Distance errors and missing resources do not trigger
seed retries. See [the server seed policy](DEDICATED-SERVER.md#surface-placement-seeds).

## Lobby and Quick Mission flow

v1 multiplayer runs through the existing Quick Mission creator. The creator
builds a mission as usual, then opens its slots to humans.

1. The creator builds the Quick Mission, sets lobby options and becomes King.
2. The session is listed in the server browser, or kept private.
3. Players enter a callsign and pick an open slot. In co-op, humans take blue
   (friendly) flights. In PvP, humans pick any side. Unclaimed slots stay AI.
4. The mission starts. Players can still join mid-game by taking over an AI
   aircraft in an active flight: blue side in co-op, any side in PvP.
5. The mission ends and the debrief shows every human and AI aircraft on one
   results screen.

**Lobby controls** (King only):

| Setting | Notes |
| --- | --- |
| Mode | Co-op (humans on blue) or PvP (humans on any side) |
| Max human players | Default 30; the rest stays AI. Co-op can seat at most 15, see [slots](#slots-ai-fill-and-handoff) |
| Host | Calculated (default) or pinned to a specific player |
| Slot locks | Reserve or close specific slots, including lead |
| Sides | PvP only: Free, Locked once flown (prevents switching sides mid-mission) or Balanced by the host ([Autobalance](#autobalance)) |
| Visibility | Public, private or password |
| Join in progress | On or off |
| Kick | Frees the slot back to AI |
| Release reserved aircraft | Frees a disconnected player's aircraft for others |
| Friendly fire | On or off |
| Respawn rules | None, into an open AI slot, or back at a base |
| Revival | Retail's settings apply to every respawn: lives (0 to 10, or unlimited), delay (0 to 5 minutes), distance from the battle (1 to 40 miles as retail, up to 150 since the lobby pass) and weapons (with missiles, without missiles, guns only, half guns) |
| AI respawn | On or off: whether a lost AI aircraft comes back under the revival rules; greyed while the respawn rule is None ([AI respawn](#ai-respawn)) |
| Minimum players | Settings > Revival: none, or 1 through the human seat capacity. The King edits it between missions, including on dedicated servers with `king-mission open`. Close/F10 applies; Escape discards. AI on and off share the gate and show "Waiting for 2 more ready players (minimum 4)." See [defaults and rules](spec/multiplayer-menus.md#minimum-players). |
| Scoring | PvP: retail's fight type (sides or free for all), kill tally (total kills, total damage or kill ratio), time limit (1 to 30 minutes), kill limit (1 to 10) and kill owner (total, one side or one player) |
| Difficulty and realism | Inherits the Quick Mission settings, locked for all humans |

Timed missions show **Mission Timer: MM:SS** in the in-flight Escape menu.
The countdown also appears at the upper right during the final five minutes,
in cockpit, external views and Watch, turning amber in the last minute.
Hour-long limits use H:MM:SS. Unlimited missions show no countdown. The clock
uses the host's mission tick and current time limit, including late joins,
rejoins and host migration. Watch shows the live deadline while its picture
is delayed or scrubbed back. See the [countdown rules](spec/hud-layout.md#mission-countdown)
for rounding, placement and network clock accuracy. Protocol remains 67.

Settings a player may not change are shown grayed out, as in retail. The
revival and scoring ranges are retail's ([numbers](spec/multiplayer.md#numbers)).

*Built on the wire (EF4, 2026-10-01; the screen is EF8, below):* the lobby
with its King (the hosting player), slots, each player's own loadout, ready
and the King's start, the King's mission change, kick, and the return to the
lobby after each mission with everyone still connected; a dedicated server's
lobby has no King. The King's settings in the table above are phase 2.
([architecture](ARCHITECTURE.md#the-lobby))

*Built (EF7, 2026-10-01): finding and joining.* Choose Activity's Multi menu
opens the **Direct Network Lounge** screen, the first half of the flow above. It
lists the games found on the local network (name, players, whether they are in
the lobby or flying, a lock for a password, another build shown as such), shows
the selected game's mission and players, joins the selected game or a typed
address or name (every address a name gives is tried, and a refusal comes back
as a plain line), and **New** hosts a game from the Quick Mission creator's
current mission. Options holds the port, the password and the game's name, the
"Forward the game port on my router" switch (on by default), and
shows the retail quick messages. The game remembers the callsign, the port, the
game name, the switch, the last eight addresses and "Show full games"; the password is
never kept. A join or New opens the lobby (EF8, below). How it works:
[architecture](ARCHITECTURE.md#the-direct-connection-screen-as-built-ef7).

*Built (EF8, 2026-10-01): the lobby screen.* Join and New open the **lobby**
(the screen reads "Lobby"). Its head shows the game's name and the King's
settings in a line (EF8 also showed the mission's summary and the start rule;
the lobby pass moved those, [below](#the-lobby-since-the-lobby-pass)). **Slots**
lists every aircraft players may take (wing, member, aircraft, who holds it or
AI, a tick when the holder is ready); a click on a free slot takes it, a click on
one's own frees it, and a slot another player holds is dimmed and cannot be
clicked. **Players** lists
the callsigns with the King's crown, the house of the player whose machine runs
the game, a tick for ready and a red cross with the reason (shown under the
lists) for a player whose game cannot play the mission. **Messages** is the
chat box and its line (Enter sends to all). The buttons: **Mission...** opens
the Quick Mission creator, for the King with its OK reading **Accept** (Start
was locked to Airborne until slice CL-R; both sides' start choices are offered
since, see **Start** below) and what a host
cannot take refused before anything is sent, and for everyone else read only
(since the lobby pass); **Loadout** opens Load Ordnance
for the player's own aircraft, with Accept and Cancel, the mission's Guns only
applied and Cheat loading refused on the page; **Ready** toggles (a player who
readies without choosing a loadout flies the standard stores, and Messages says
so); **Fly** (the King, blue when everyone holding a slot is ready) starts the
mission, and says "Not ready: Hawk." otherwise. When Fly would be refused
only because someone holding a slot is not ready, and at least one slot
holder is ready, the button reads **Force Start** instead (below); **Kick** (the King, on a
selected player; since F2-L inside **Players...**) asks for the reason the player will see; **Leave** returns to
Direct Connection after the [role-aware confirmation](spec/multiplayer-menus.md#leaving-a-session).
Ordinary clients see "Are you sure?"; hosts see whether the game ends or
another player takes over. A dedicated server's King can pass the crown by
leaving without ending the game. While a mission flies the lobby stays open to a late joiner or to a
player who ended their flight: take a slot and press **Join** to fly at once,
with the standard stores; the King's Fly then reads **End Mission**. After a
mission each player reads the debrief and returns to the lobby, still
connected, slots and loadouts kept and ready cleared. A dedicated server's
lobby has the King's buttons only when its operator enables a player King;
otherwise Mission... is read only. How it works, the layout and
what was measured:
[architecture](ARCHITECTURE.md#the-lobby-screen-as-built-ef8).

*Designed (phase 2, 2026-10-05; agent proposals awaiting John unless
credited):* the King's settings in the table become one numbered list that the
host keeps and every player sees in the lobby's **Settings...** panel, greyed
for anyone but the King, as retail greys them. Each setting's values and its
co-op and PvP defaults are in the
[architecture](ARCHITECTURE.md#the-kings-settings). Max human players, join in
progress, the password and the visibility (hidden, answering the local
network's search, or public once stage I lists games) change at any time; the
rest only in the lobby. The King passes the crown from
**Players...**, which also kicks; a King who leaves passes it to the
longest-connected player. The *house*, the game that runs the host, is apart
from the crown: its leaving hands over to a ready standby, or ends the game
when none is ready. See [host migration](#host-migration). Difficulty and realism stay the mission's own cheats, set
by the King in the lobby and fixed for the flight. Not in phase 2: the Host row
(pinned or calculated) and releasing a reserved aircraft (stage K), and what
public visibility does on the master server (stage I).

*Built on the wire (F2-1, 2026-10-05):* the King's settings with their phase
rules, PvP's slots on both sides, slot locks (closed or kept for a callsign),
join in progress, lock sides, the player limit, password and visibility (a
hosted game lists itself on the Internet Lobby while it is public), the
loadout rule (Cheat loading under `any`), friendly fire in the flown mission,
the crown passed and passed on, the house apart from it, and a dedicated
server's King with `king first-player` and `king-mission locked`; a server's
file sets every setting by name. How it works and each agent decision:
[architecture](ARCHITECTURE.md#the-kings-lobby-as-built-f2-1).

*Built on the screen (F2-L, 2026-10-05):* **Settings...** opens the panel for
every player (four pages: Game, Revival, Scoring and Realism; a click turns a
row forward, a right click back; greyed for anyone but the King and for a row
that does not apply). Edits stay local across tabs. Close or F10 applies once;
Escape discards. Realism, Friendly Fire and Loadouts share at most one mission
refresh. The [menu specification](spec/multiplayer-menus.md) defines the
names, list geometry, shortcuts and safe ? menu exits. **Players...** (the King, for the player selected in
Players) gives the crown or kicks. The King's right click on a slot closes it,
opens it again, or keeps it for the player selected in Players. While the
mission flies the Loadout button reads **Watch**. The lobby's head sums the
settings up in a line. How it works and each agent decision:
[architecture](ARCHITECTURE.md#the-lobby-panels-as-built-f2-l).

**Start.** Retail multiplayer starts everyone airborne, and so did this game
until the surface close-out. *Since slice CL-R (2026-10-11):* the King's
creator offers the single-player start choices for both sides (John: "add
redfor ground starts, match the quick mission creator styles"): Start and
Airport park Friendly Wing 1 at a Blue field, and Enemy start and Enemy
airport park Enemy Wing 1, the Redfor flight, at a Redfor field, with the
same automatic pick near a ground target
([start placement](spec/surface-defenses.md#start-placement)). A player who
takes a parked plane starts on the ground there, on NAV with that field
selected; that it unlocks Blue's ground start in the lobby too is an agent
decision of the slice. Late joiners take over aircraft already flying. *Retail gap-fill (agent):* until multiplayer
ground starts exist, "back at a base" starts the player airborne near their
side's base. *Designed (phase 2):* this is retail's revival: a new aircraft of
the player's type, airborne, at the revival distance from the battle on the
bearing of its side's start, with the revival weapons
([architecture](ARCHITECTURE.md#death-revival-and-lives)). *Built (F2-V,
2026-10-05):* the HUD says whether and when the player may fly again, Enter or
Join flies again, and the lost aircraft stays with its player until it flies
again or leaves the game.

### The lobby since the lobby pass

*Built (the lobby pass, 2026-10-09).* John's requests of 2026-10-09 changed
the lobby in five ways: the lists scroll, PvP shows its sides, the mission
can be read by everyone, the host can balance the sides, and the screen
says what to do next. His decisions and answers are in the
[decisions table](#made-by-john-on-2026-10-09-for-the-lobby-pass); how each part
works is in the [architecture](ARCHITECTURE.md#the-lobby-screen-as-built-ef8).
This section is what a player sees.

#### Layout

From the top of the screen:

| Part | What it shows |
| --- | --- |
| Game, Rules | The game's name, and the King's settings in one line ("PvP by sides, 5 kills or 10 minutes, revival with unlimited lives, sides locked"). The Mission line and the King line that used to sit between them are gone: the mission is read on **Mission...**, and the start rule moved into the ready hint |
| Slots (left) | Every aircraft players may take, with a scroll bar. In PvP the **Bluefor** and **Redfor** boxes sit on its heading row |
| Players (right) | Callsigns with the crown, the house, ready ticks and platforms, with a scroll bar |
| Hint line | What to do next, or the reason the selected player cannot play |
| Messages and chat line | The game's words and chat, with a scroll bar |
| Ready hint | One dim line above the buttons that says what this player does next so the King can start (below) |
| Buttons | The King: Mission..., Settings..., Players..., Loadout, Ready, Fly, Leave. Everyone else: Mission..., Settings..., Loadout, Ready, Leave, filling the row from the right so Leave is always last |

**Scroll bars.** Slots, Players and Messages use the flight map's arrow bar:
a flat grey track, a darker thumb, and an arrow button at each end (the same
bar the plan view draws, 16 pixels wide, from the map's colours). Drag the
thumb, click the track to move a page, press an arrow to move one row, or
turn the wheel (a list slides its window and leaves the selection where it
is; Messages moves three lines a notch). The arrow keys and the controller
still move the selection, and the list follows it. When everything fits, the
arrows and the track are still drawn and the thumb is absent, so a click
does nothing and nothing shifts. The Settings panel's pages use the same bar
on the right of the rows. Both lounges use it on Games, Players and Messages
on their games lists, in place of the PREV and NEXT rocker, and on Messages.
The bar is drawn by the game, not from slider pieces in the import.

#### Force Start

*Requested by John on 2026-10-16.* When the King's **Fly** would be refused
because not everyone holding a slot is ready, and at least one slot holder
is ready, the button reads **Force Start**. Pressing it starts a 10 second
countdown. The button's place then reads **Starting in 7** (the seconds the
host still has left, the same number for every player). Messages says "The
King is force starting in 10 seconds." Pressing the button again cancels it,
and Messages says "The King cancelled the force start." If everyone holding
a slot becomes ready during the countdown, the mission starts at once.

At zero the mission starts. *Agent decision:* a slot holder who is still not
ready flies in that slot with the loadout they have, the one they chose or
the aircraft's standard stores when they chose none, the same as a player
who readies without opening Loadout. A player with no slot stays in the
lobby and can join in progress as usual. A player whose game cannot play
the mission stays in the lobby too; the host's log names them separately
from the players who were not ready.

**Force Start** is not offered when nobody is ready, when everyone is ready
(the button is **Fly**), or when `min-players` is not met. The ready-player
minimum is not overridden. Empty slots do not block a start, including with
`ai off`. A
dedicated server has no King unless `king first-player`; the crowned player
then force starts under the same rules, and `min-players` still gates the
start. The host owns the countdown. A takeover drops it: it is not written
in the journal. The host logs the start, the cancel, and, at the forced
start, who was not ready ("Force start: Hawk was not ready.").

While the countdown shows, a joiner's buttons shift left so the countdown
sits in the King's Fly place and Leave stays last. Only the King can press
it. Enter on an empty chat line presses it for the King, as it does for Fly.

#### Mission... for everyone

**Mission...** is on every player's button row, enabled once connected and in
every phase (John: no separate "Game info" button).

| Who and when | What opens |
| --- | --- |
| The King, in the lobby | The Quick Mission creator, its OK reading **Accept**, as before, but opened on **the lobby's current mission**, not on the King's last local draft. Accept lays the creator's fields over the lobby's mission, so the cheats and weather overrides the King set in Settings are kept |
| Anyone else, in any phase | The same page, **read only**: "View only" in its header, **Back** in OK's place (Enter and Esc close it too), and every click on a field, left or right, says "Only the King changes the mission." Fields the lobby's mission does not carry (the nationalities and the situation) read "as the King's", dimmed. An aircraft or theater the reader's game lacks is tinted as in the King's creator, so a player sees what they cannot fly. If the King changes the mission while the page is open, it redraws and says "The King changed the mission." |
| The King, while the mission flies | Read only, as above; a click says "The mission can change only in the lobby." |
| The King, on a dedicated server with `king-mission locked` | Read only. The server tells the lobby (protocol 20), so the page opens with "View only: this is the lobby's mission. Back returns to the lobby. This server's mission is fixed." and a click says "This server's mission is fixed." |

The read-only page is built from the mission text the lobby already carries,
and the creator reads it back to the same mission for every mission it can
make (tested over every value and 400 generated missions).

#### The ready hint

The line above the buttons, in the dim face, says what this player does next:

| Who and when | Text |
| --- | --- |
| Lobby, holds no slot | "Take a slot and press Ready so the King can start the mission." |
| Lobby, holds a slot, not ready | "Press Ready so the King can start the mission." |
| Lobby, ready, not the King | "You are ready. The King starts the mission with Fly." |
| The King, not everyone ready, and a force start is allowed | "Someone is not ready: press Force Start to begin in 10 seconds." |
| The King, a force start counting | "The force start is counting down. Press it again to cancel." |
| The King, not everyone ready, and a force start is not allowed | "Press Fly when everyone holding a slot is ready." |
| The King, everyone ready | "Everyone is ready: press Fly to start the mission." |
| A joiner while a force start counts | "The King is force starting. The mission begins when the count ends." |
| A server with no King named | "The mission starts when the King presses Fly and everyone holding a slot is ready." |
| A server that starts at the first ready player | "The mission starts as soon as the first player holding a slot is ready." |
| A server whose mission is always flying | "This server's mission is always flying: take a slot and press Ready to join it." |
| Mission flying, no slot | "The mission is flying: take a slot and press Join." |
| Mission flying, holds a slot | "The mission is flying: press Join to take your aircraft in." |
| Mission flying, in it, or watching | "You are flying the mission." or "You are watching the mission." |
| The player's game cannot play the mission | "Your game cannot play this mission." (the reason itself is in the hint line above) |

The hint line above it keeps its older job, the selected player's detail or
how to take a slot. In PvP, with no side yet, it reads "Check Bluefor or
Redfor to join a side, or click a free slot."

#### PvP sides in the lobby

In co-op every human is Bluefor, so none of this appears. In PvP:

**Colours** (John, 2026-10-09).

| Slot | Bluefor | Redfor |
| --- | --- | --- |
| Open (the AI flies it, anyone may take it) | Light blue text (134, 182, 223) | Light red text (206, 113, 121) |
| Taken, including the reader's own | A solid royal blue bar (36, 81, 186) with the holder's name in white | A solid bright red bar (210, 36, 40) with the holder's name in white |
| Held by the AI for an away player | The bar, with dimmed text | The bar, with dimmed text |
| Closed, or kept for another player | Dimmed, as in co-op | Dimmed, as in co-op |

The wing cell names the side too, "Blue 1 #2" and "Red 1 #2" (John), so the
list reads without colour. In the
Players list a player's callsign takes their side's light colour, and a player
with no side stays plain. A player whose game cannot play the mission is
dimmed rather than red, so red means Redfor only; the cross icon still marks
them.

**The Bluefor and Redfor boxes.** A player's side is the side of the slot they
hold, and a player with no slot has no side. Each box is a retail check box
labelled with its slots taken over its total ("Bluefor 3/5"); a side whose
slots are all held, closed or kept for someone else reads full, as 5/5.

| You | The box | A click |
| --- | --- | --- |
| Have no side, and the side has a free slot | Unchecked, lit | **Checks** the box: you take that side's lowest-numbered free slot (so the first humans on a side lead its flights), and the Slots list shows that side only |
| Have no side, and the side is full | Unchecked, greyed | "Redfor is full." |
| Are on this side | **Checked** | **Unchecks** it: you leave the slot, your ready mark clears and you have no side again |
| Are on the other side | Unchecked, greyed | "Uncheck Bluefor first." Leave your side, then check the other (if it is not full) |
| Are under Autobalance | Your assigned side checked, both greyed | "Autobalance picks the sides." |
| Are flying with sides locked | The side you flew checked, both greyed | "Sides are locked until the mission ends." |
| Cannot play the mission | Both greyed | The reason |

```mermaid
flowchart LR
    N(["No side"]) -->|"check a box, or click a free slot"| S(["On a side"])
    S -->|"uncheck the box, or click your own slot"| N
```

From a side, a click on another free slot of the same side only moves you
inside it, and the other side's box stays greyed until you leave.

With neither box checked the Slots list shows both sides, Bluefor first. A
click on a slot row works as before inside what is shown: a free slot on your
side moves you there, your own slot leaves it (and unchecks the box), and with
no side any free slot joins its side and checks that box. The King's right
click on a slot is unchanged, but a King on a side sees only that side, so
closing a slot on the other side needs an uncheck first. Tab reaches a lit box
after the Slots list, and Space or Enter presses it. A late joiner uses the
boxes the same way, and a free AI aircraft on the side is theirs to Join.

#### Sides: Free, Locked once flown, Balanced by the host

The King's **Sides** row (Settings, Game page; `lock-sides` in a server's
file) used to be Lock sides and now has three values. It is PvP only, and
greyed in co-op.

| Value | `lock-sides` | What it does |
| --- | --- | --- |
| Free | `off` | Players choose and change side whenever the boxes allow it |
| Locked once flown | `on` (the PvP default) | A player may choose freely in the lobby, but once they fly a plane they keep that side until the mission ends: "Sides are locked until the mission ends." |
| Balanced by the host | `balanced` | **Autobalance**: the host gives each player a side and players cannot change it. It also locks sides in flight |

The Rules line says "sides locked" or "sides balanced".

#### Autobalance

*Built (the lobby pass, slice A1; John's rules marked).*

- **Who it counts.** Humans holding a slot (in flight, a plane). Observers and
  players whose game cannot play the mission are neither counted nor seated.
- **Seating.** A player without a slot goes to the side with **fewer humans**
  (John); on a tie, the side with more free slots; on a tie, Bluefor. The host
  gives them that side's lowest-numbered free slot, not ready, and says
  "Autobalance put you on Redfor." A side with no free slot is skipped, so
  uneven sides fill: with 2 Bluefor and 10 Redfor slots, six players sit 2 and
  4. A player who fits nowhere waits without a slot ("Both sides are full."),
  and is seated when one frees.
- **No changing side.** The boxes are greyed, a slot of the other side is
  refused, and so is leaving the slot, with "Autobalance picks the sides." A
  free slot of the player's own side can still be taken.
- **Turning it on** re-deals with the fewest moves. Players keep their side
  while the sides are within one human of each other; otherwise the host moves
  players from the larger side, the most recently joined first and **the King
  last** (John). A moved player is told "Autobalance moved you to Redfor." and
  loses their ready mark, and their loadout if the new aircraft differs. The
  ready marks of players who did not move stand.
- **A player leaving moves nobody** (John). The next joiner goes to the smaller
  side, and the King re-deals by turning Autobalance off and on.
- **A mission change** keeps the slots that survive it, whatever side they
  are on; anyone it frees is seated again by the rule.
- **In flight** a late joiner is seated on a free AI aircraft of the side the
  rule gives; a player's revival follows lock sides.

## Slots, AI fill and handoff

The Quick Mission creator places up to two sides of three wings, with up to five
aircraft per wing: 30 aircraft at most. Each aircraft is a slot. In co-op only
the 15 blue slots can seat humans, so the default of 30 humans is reachable only
in PvP.

Every slot not held by a human is flown by AI. A joining human takes over an AI
aircraft in flight with its current position, speed, fuel, weapons and damage.
A human who drops is replaced by AI at once and the mission continues.

Each wing flies one aircraft type, chosen by the King in the creator.
*Retail gap-fill (agent):* each human chooses the loadout for their own slot on
the normal Load Ordnance screen before the mission starts, as retail players
armed their own aircraft. A late joiner keeps the loadout of the aircraft they
take over.

*Designed (phase 2):* in PvP every aircraft of both sides is a slot. The King
can lock a slot: **closed** (the AI flies it and no human takes it, the lead
included) or **reserved** for one callsign. With join in progress off, nobody
takes an aircraft once the mission flies except to fly again after a loss; with
sides locked once flown (or balanced by the host), a player stays on the side of
the first aircraft it flew. A
revival adds a new aircraft to the player's wing; a mission holds at most 64
aircraft at once, and the oldest wreck that has rested 30 seconds is retired to
make room ([architecture](ARCHITECTURE.md#slots-sides-and-joining)).

**Flight model.** Every aircraft in a multiplayer mission flies the hybrid
flight model, AI included, so a human taking over an AI aircraft never feels its
handling change. Single player is unchanged: airborne AI keep the legacy model
there.

**Lead succession.** If a flight lead is shot down, a human in the flight takes
the lead if there is one; otherwise the next AI member does, and the flight
re-forms on the new leader. The new leader hears the retail "You're the
Wingleader now" call ([radio chatter](spec/radio-chatter.md#youre-the-wingleader-now)).
Single player gains the same succession: the AI takes over from a player who is
shot down. Built in stage B: the AI passes the lead (B3), and a human who takes
it hears the call (B4). In a game with revival a human lead keeps the lead
instead while it is lost or waiting ([lead hold](#lead-hold)).

### AI respawn

*Built (the lobby pass, slice R1; John's rules of 2026-10-09 marked).* A shot
down AI aircraft comes back, in co-op and PvP alike (John).

- **The setting.** **AI respawn** is a row on the Settings panel's Revival page
  (`ai-respawn`, setting 23): on by default, and greyed while Revival is
  `none`, because then nothing comes back (John). With it off, humans still
  revive and the AI does not.
- **The rules are a human's** (John): the same revival delay, and the same
  lives, counted per original aircraft, since an AI aircraft has no player. With
  unlimited lives, the default, the count never shows. Under `ai-slot` an AI
  aircraft respawns as under `revive`, which keeps the pool of free AI
  aircraft that `ai-slot` needs.
- **Where** (John): at its flight's **original spawn point**, with its original
  heading, not near the lead and not at the revival distance. If another
  aircraft is within 2,000 ft of the point, it steps back 1 nm at a time, up to
  five steps, and never off the map. The revival distance below is a human's
  alone.
- **What.** The same aircraft, full fuel, the loadout the lobby gave that slot
  (or the standard load) cut by the revival weapons setting, the flight's own
  skill, and the flight's orders.
- **Joining the flight** (John): the new aircraft is the **last wingman** of
  whoever leads the flight now, so a flight that keeps losing its lead keeps
  passing the lead down, and respawned aircraft queue at the back of that
  line. In a flight of AI alone the lead goes on shifting down as fighters die,
  as it always has.
- **Whose loss it is.** Only an aircraft nobody holds respawns as the AI's:

```mermaid
flowchart TD
    A[An aircraft is lost] --> B{Who held it?}
    B -->|The AI, for nobody| C[AI respawn, if AI respawn is on and Revival is not none]
    B -->|A player in the game| D[That player's own revival, as before]
    B -->|A player who left the game| C
    B -->|The AI, for an away or dropped player| D
```

  A player who is away (the AI flies their plane) keeps their own revival on
  return, so their aircraft is never duplicated. A player who leaves the game
  hands their aircraft's lineage to the AI.
- **Room.** A mission holds 64 aircraft at once. Human revivals are made first
  each tick, and an AI respawn takes the room they leave, retiring the oldest
  wreck that has rested 30 seconds, or waits. Respawns reuse their original
  flight positions, so repeated lives do not exhaust callsign numbers.
- **Nothing is said** on the radio or the HUD for an AI respawn. The server's
  log and the hosting game's say "Red 2-5 respawned in plane 14 at its original
  spawn" and the like. Respawning itself adds to nobody's score, and killing a respawned
  aircraft counts as any AI kill does.

With AI respawn and unlimited lives, a co-op mission ends only on its time
limit or when the King ends it; multiplayer has no "all enemies destroyed" end.

### Revival distance and separation

*John, 2026-10-09: the distance he meant is the **revival distance**, and he
also asked for 75 on the **separation**.*

- **Revival distance** (Settings, Revival page; `revive-distance`): 1, 5, 10, 20
  and 40 nm as retail, and now **50, 75, 100 and 150 nm**. The default stays 10.
  A human revives at this distance from the battle, on the bearing of their
  side's start, as before; John's "original spawn point" rule is for the AI
  alone. A point that would fall off the map, which a long distance near an
  edge can do, is walked back toward the battle until it is on the map (fitted).
  A trip back from 150 nm takes about 15 to 20 minutes at cruise.
- **Separation** (the Quick Mission creator's field 17, `separation-nm` in a
  mission file): now 1, 2, 5, 10, 20, 50, **75**, 100, 150, 200 and 300 nm.
  The creator, the King's included, lists only the distances that fit the
  theater, ground target and start, from a fixed table; a host moves a
  distance the table leaves out to the nearest offered one below it and logs
  it ([keeping the enemy on the map](spec/quick-mission-menu.md#keeping-the-enemy-on-the-map)).
  At 150 nm the sides still meet, about 10 minutes in, so a
  10-minute time limit can end a game first.
- **Situation** (the creator's field 16, `situation` in a mission file):
  advantage, neutral or disadvantage, carried in the mission since 2026-10-28
  and shown in the read-only creator ([situation](spec/quick-mission-menu.md#situation)).

### Lead hold

*Built (the lobby pass, slice R2; John's rules of 2026-10-09 marked).* In a
game with revival (Revival is not `none`), a human who leads a flight **keeps
the lead** while their aircraft is lost, while they wait to revive, while they
sit in the lobby holding their seat, and while they fly back after reviving
(John). Single player and games without revival keep today's rule, where the
lead passes at once. The behaviour is specified in the
[AI spec](spec/ai.md#lead-hold-in-games-with-revival).

```mermaid
flowchart TD
    L(["Leading"]) -->|"their aircraft is lost, or they wait to revive"| H(["Held: a stand-in leads"])
    H -->|"their new aircraft joins the flight"| L
    L -->|"they leave the game"| P(["Passed on"])
    H -->|"they leave the game"| P
    P -->|"the next human in the flight is flying"| L
    P -->|"the next human is waiting to revive"| H
    P -->|"no human is left"| A(["The AI's own succession"])
```

- **While the lead is held**, a **stand-in** leads. It is the lowest-numbered
  flying human, else the lowest-numbered flying AI aircraft, the same order the
  AI has always used. The flight follows the stand-in as it would any lead. An
  AI stand-in flies the mission of opportunity; a human stand-in may give wing
  orders. A stand-in never becomes the owner of the lead.
- **Two lines on the HUD.** A human who stands in reads "You lead the flight
  until Viper flies again." (the lead's callsign; for a lead with no known
  name, the plane's radio label, such as "Red one", and if it has none the
  line says "until its lead flies again"). The owner, when their new aircraft joins, reads "You lead your
  flight again." Both are text only, with no voice, and neither player hears
  "You're the Wingleader now" for them. A player who takes the lead for good,
  because the owner left, hears the call as always.
- **The lead goes back** the moment the owner's new aircraft is in the flight.
  The flight re-forms on it in member order, and the AI wingmen fly back into
  formation, **except one that is engaged in combat, which finishes its fight
  first** and then re-forms (John). The owner then orders them as usual.
- **When the owner leaves the game** (leaves, is kicked or drops and is no
  longer kept), the lead goes to the next human in the flight, who leads at
  once if flying and holds the lead if waiting to revive. With no human left,
  the AI's succession goes on. A player who leaves the flight to the lobby with
  a living aircraft has left the flight the same way; one who leaves with a
  lost aircraft keeps their seat and the lead.
- **Away.** An away player's plane (the AI flying it) keeps the lead, and the
  player gets it back with the plane.

John accepts that a held lead can leave a flight without its leader for a long
time, since revival can be minutes and 150 nm away.

## Rejoin and observers

**Rejoin.** When a human disconnects, AI takes over their aircraft. A wingman
follows the flight; a lead follows the flight plan. The aircraft stays reserved
for that player as long as it is alive, and other joiners cannot take it unless
the King releases it. The player rejoins with their saved token and gets the
aircraft back, with the weapon, sight, laser, designation, A-G page, radar
switch and NAV airport as they left them; a laser bomb they left guiding
keeps following their laser while the AI flies (slice FX-J, 2026-10-11;
[what is kept](ARCHITECTURE.md#what-a-returning-player-keeps)). The radar
scope's channel, range and history are the player's own game settings and
come back with the game. If the aircraft is destroyed, or the mission ends
before they return, they rejoin as an observer until the round ends.

**Observers** use the replay viewer's camera and playback controls on the live
session. This covers players whose aircraft was destroyed, players who join with
no slot available, and pure spectators. Observers cannot chat to the players
flying, but they talk among themselves ([decision](#decisions)). In
PvP the King can set an observer delay so observers cannot relay live positions
to a side. The delay is applied by the host before anything is sent, so an
observer's machine never holds live positions. The host keeps the delayed
picture only while someone watches (John, 2026-10-19): an observer who starts
watching sees the battle from the moment it asked, once the delay has passed.

*Designed (phase 2; the decision above is John's, the rest agent proposals):*
a connection with no plane while the mission flies can **Watch**: the host
sends it snapshots with no plane of its own, near its camera at the full rate
and the rest 4 times a second (twice before D12), delayed by the King's observer delay (0, 10, 30 or
60 seconds, PvP only). The game shows them in the replay viewer in a **live
mode**: the view follows the newest moment, and the player can pause, scrub
back through the last 10 minutes and return to live, never past it. A player
whose aircraft is lost flies again by the King's respawn rule and lives, or
watches when none is left ([architecture](ARCHITECTURE.md#the-observer-view)).
Rejoining with a token is stage K. *Built (F2-O1, 2026-10-05):* the stream, as
described. *Built (F2-O2, 2026-10-05):* the screen: pressing Watch in the lobby
opens the replay viewer on a recording the game grows from the stream. The
playhead follows the newest moment (LIVE on the bar), the last 10 minutes can
be scrubbed, End returns to live and nothing goes past it; Esc opens the
viewer's menu, whose Stop Watching returns to the lobby. Every aircraft is
another's (the observer has no plane), the camera tells the host what to send
at the full rate, and smoke, contrails and gun rounds are not drawn yet
([how it works](ARCHITECTURE.md#the-observer-view)). *Built (F2-O3,
2026-10-06):* an away player's own plane shows on this screen too, over its
flight, and the first flight input takes the plane back
([how it works](ARCHITECTURE.md#the-observer-screen-for-an-away-players-own-plane-f2-o3)).

*Designed (stage K, 2026-10-05; agent proposals awaiting John unless
credited):*

- **The token** is 128 random bits, sent when a player first joins and kept
  by the game in its data folder (at most 32). It is good only in the session
  that issued it, whichever machine hosts that session now, and for 24 hours
  after its player was last connected (John, 2026-09-28). A kick voids it.
  A clean Leave, Return to Main Menu or Exit clears it on the client and host
  (John, 2026-10-23). Ending only a flight while staying in the lobby keeps it. The
  game sends it by itself when it joins that session again: from Direct
  Connection's list or the Internet Lobby, which mark such a game "Rejoin", or
  at the address it came from.
- **Reserved.** A player who drops (silence or a crash) leaves its aircraft to the AI, reserved for it while it is
  alive: its slot reads "AI (Viper away)" and nobody else takes it (John,
  2026-09-28). A player who ends its own flight frees its aircraft. The King's
  "Release reserved aircraft" in the Players panel frees one. A mission's end
  clears them all.
- **Coming back.** With its token the player is itself again: its
  scores, lives, side and, in flight, its reserved aircraft, even when the
  game is full. Agent decision: it uses the current Callsign field, with the
  existing duplicate suffix rules, and the reservation label changes with it.
  A clean departure instead makes the next join a new player.
  [Callsign rules](spec/multiplayer-menus.md#callsign-when-returning-to-a-game).
  Messages say "Welcome back, Viper: your aircraft is waiting."
  If the aircraft was lost meanwhile, the revival rules apply with the lives
  the player had; with none (co-op's default), it watches until the mission
  ends, John's rule.
- A dedicated server holds reservations and takes tokens the same way, for as
  long as it runs.

**What is actually kept (2026-10-23).** Identity is a random 128-bit token
scoped to the host's session. The host maps it to the player's join order,
not an install id, address or callsign. The client remembers an address only
to find the token and query the server. Hosting and dedicated servers use the
same rules:

- A clean Leave, Return to Main Menu or Exit clears identity and frees the
  aircraft. Ending just the flight, while staying in the lobby, keeps identity
  but releases the aircraft.
- After an unclean drop the host normally notices 5 seconds without a packet.
  With AI aircraft enabled, a living aircraft is reserved for that token.
  A reconnect before the timeout can replace its still-present connection and
  reclaim the same aircraft. Other players cannot take a reserved aircraft.
- The reservation has **no independent age countdown**. Return, loss of the
  aircraft, the King's Release, a kick, or mission end clears it. With AI off
  no dropped aircraft is reserved. The token is valid for 86,400 seconds after
  its player was last connected. Expiry blocks reclaiming; it does not itself
  remove the reservation table entry. Release, aircraft loss or mission end
  still clears that entry. The host's record cap can evict its longest-gone
  identities sooner; the client also keeps only its 32 most recent tokens.
  Local expiry counts 24 hours from its last saved grant or handled disconnect;
  an abrupt crash can therefore leave less local time than the host allows.
  [Record limits](ARCHITECTURE.md#rejoin-tokens-and-reservations-as-built-k5).
- Mission deadlines can be much earlier. A dedicated server's default
  `empty-timeout` is 60 seconds, configurable from 0 to 86,400. Hosted games
  use 0. The empty timer starts once nobody is flying, away, awaiting revival
  or resuming, after someone has flown. Reserved aircraft and ordinary
  observers do not keep the mission active. The King's mission time limit
  can also end the round. The server reports the earliest current deadline
  among these limits and token expiry, without adding a new hold timer.
- A departed player's old **lobby slot is remembered, not reserved**. A token
  join attempts to restore it and its loadout only if it is still free and
  allowed. Another player may take it first.
- Mission end, restart or rotation clears aircraft reservations. Tokens remain
  valid in that server session, and a remembered lobby slot is still only a
  best-effort choice in the new mission. A server process restart creates a
  new session and knows none of the old tokens. Host migration carries the
  existing session, tokens, reservations and expiry clock.

**The lounge checks before promising a slot.** On selecting a previous game,
the client sends a private, read-only reservation query to the server. A fresh
reply names the side, wing and member, with the current maximum **return
window** from those deadlines. It says that loss, release or an earlier
mission end can end the hold first. A
confirmed unavailable slot gets a neutral message. No reply means no promise.
A still-valid token without an aircraft may restore scores and lives, so the
neutral message does not falsely promise a new player identity either.
[Polling, timeouts and address limits](spec/multiplayer-menus.md#callsign-when-returning-to-a-game).

*Built (K5, 2026-10-05):* the host's and the client's side of all of this, on
the simulator and in the net lane: tokens, the gate, reservations, Release
and the revival rules on return (the screens, the game's token file and the
Rejoin marks are K7b's, built 2026-10-06, below). `tore-bot --token-file FILE`
keeps and sends a bot's token. The details, each an agent decision, are in the
[architecture](ARCHITECTURE.md#rejoin-tokens-and-reservations-as-built-k5):
the player takes the aircraft back with Join; a game restarted before the host
noticed replaces its old connection; a token that does not work tells the
player in words and it joins as a new one; an away player who quits on purpose
frees its aircraft.

*Built (K7b, 2026-10-06):* the screens. The game keeps its tokens in
`rejoin-v1.conf` in its data folder (32 at most, each good for 24 hours after
the player was last in the game) and sends the right one by itself when it joins
a game it was in. Direct Connection and the Internet Lobby mark such a game
**Rejoin** in the list (the Internet Lobby once the game is selected, since the
master names a game's session only then); Messages says "Welcome back, Viper:
your aircraft is waiting." Both Options panels have **Let my game take over
hosting**, on by default. In the lobby the King's Settings has a **Host** row
(calculated, or a player who is not relayed), a plane kept for a player who
dropped reads "AI (Viper away)" with a lock mark, a game that stands by shows an
outlined house beside the host's solid one, and the Players panel's **Release**
frees a kept plane, for a player who is away or for one who dropped (select the
slot in Slots, then Players...). During a migration the HUD says "Lost contact
with the host. Moving the game to Hawk..." and then "The game moved to Hawk."
([architecture](ARCHITECTURE.md#host-migration-and-rejoin), slice K7b).

## Comms and chat

When a human leads, their wing orders go to all wingmen, human or AI. Human
wingmen receive the order as audio plus a short text line. Human wingmen can send
standard replies and requests: "engaging", "winchester", "bingo", "need help".

v1 keeps today's Alt-key wing orders and adds keys for the human wingmen's
replies and requests. An on-screen comm rose is deferred.

The game's radio today is one shared channel on which the player hears their own
flight. Separate wing and battle-net frequencies (for AWACS) do not exist yet and
are new work.

Text chat is available in the lobby and in flight. Voice chat is out of scope.
Chat uses retail's keys and receivers: `~` to type, and receivers All,
Friendlies, Enemies, Wing and Target ([retail keys](spec/multiplayer.md#what-the-player-sees-and-does)).
Before flight only All is available. *Retail gap-fill (agent):* a
`CHAT.TXT`-style file of up to 12 quick messages. *Built (EF6):* chat runs
through the host in the lobby and in flight, with the `~` line, the receivers,
the top-left window and `CHAT.TXT`'s quick messages on F1 to F12 while the line
is open ([how](ARCHITECTURE.md#chat), [keys](CONTROLS.md#built-in-controls-outside-the-tables)).

Every radio call is generated by the server core and delivered to each player as
a list of recordings. Each machine plays the imported recordings itself and
applies its own listener rule: a player hears their own flight and the nets
their aircraft can receive.

## Flight data link

A wing shares one tactical picture: who is tracking what, who is locked on what,
and which target was assigned to whom. AI and humans read and write the same
picture, so a human in slot 2 gets exactly what the AI in slot 2 would. It works
the same in single player, which keeps AI wingmen honest.

**Shared within a flight:**

- Radar contacts each member is tracking.
- Hard locks and the member holding them.
- Target assignments from lead: "engage my target", "engage bandit X", sort orders.
- Member state: position, heading, fuel state, weapons state and coarse damage.

**Cues:**

| Where | Cue |
| --- | --- |
| Radar | One marker for a contact locked by a wingmate, a different marker for "assigned to you" |
| Target window | A tag such as "2 LOCKED" or "ASSIGNED BY LEAD" |
| HUD | The assigned target's box uses a distinct shape, or pulses until you lock it |
| Sort warning | A short alert when two flight members lock the same bandit |

**Who has it.** *John, 2026-09-28:* the link was gated by era, so that older
aircraft got assignments by voice only and a mixed flight shared what its least
capable member could receive. *John, 2026-10-05:* "All friendlies get data link
regardless of aircraft type (unless radar isn't on the plane which then player
doesn't see)." Every aircraft of a side is linked, in its flight and over the
battle net; the aircraft type holds only a has-a-radar flag, and a player whose
aircraft has no radar sees no link cues on the displays it does not have. The
[data link guide](DATALINK.md#who-shares-it) has the detail.

**Rates.** The shared track picture updates at 4 Hz. Locks and assignments are
sent immediately.

**Radio backing.** Every data link assignment also fires a radio call built from
the imported voice recordings, such as "Two, engage bandit, bearing 270, 15."
Bearing and range are measured from the receiving aircraft. The order is heard
even with a data link, lands in the replay's comms record, and behaves the same
for human and AI wingmen. All data link events are recorded in the replay.

*Designed (stage G, 2026-10-05); slices G0 (the radar flag and the picture), G1 (the engagement table) and G3a (assignments and their calls) are built; the AI does not read the picture yet:* the [data link guide](DATALINK.md)
says what the player sees and hears, who shares what, how the AI
uses the picture, the wing and battle nets, and what changes in single player;
the code design and its slices are in the
[architecture guide](ARCHITECTURE.md#flight-data-link) and the bytes in the
[wire protocol](formats/net-protocol.md#data-link-stage-g). Its choices are agent
proposals awaiting John's review: the call's words ("Two,
attack bandit, bearing 270, 15 miles, angels 20": there is no "engage"
recording), the sort order (Alt+A), the battle net and its monitor key
(Alt+N, off by default), and each single-player change.

## Sim rules in multiplayer

- No pause and no time compression.
- Opening the in-flight menu, losing window focus or unplugging a controller
  does not pause the mission. The controls go neutral meanwhile (John,
  2026-09-30). After the King's `idle-ai` time away, 5 minutes by default
  (never, 1, 2, 5 or 10 minutes; a stalled game counts too), the AI flies the
  aircraft, reserved for the player (John, 2026-10-05; 10 seconds was too
  short for someone on autopilot who steps away, so 5 minutes: John,
  2026-10-06). No flight input takes it back: it is a flight sim and nobody
  should fly AFK. The player watches the AI fly it and comes back on purpose
  from the Escape menu: **Take Back Flight** while the reserved aircraft is
  alive, **Spawn in Aircraft** when none is left (the revival rules), or
  **Leave Game** (John, 2026-10-06; built in F2-A, F2-O3 and F2-O4,
  [architecture](ARCHITECTURE.md#the-ai-flies-an-idle-players-aircraft)).
- Game speed and realism settings are locked by the lobby for everyone.
- *The AC-130's gunsight* (John, 2026-10-09; protocol 21): the host owns the
  whole sight (its look, a pin or track, the guns' train and the pipper), and
  the pilot's seat sends only the slew, the zoom step and the designate and pin
  commands. The camera turns smoothly on the client from its own inputs and
  corrects to the host; the pipper and gun marks are the host's, a round trip
  late, like the barrels. Other players see the barrels follow the sight. The
  AC-130's always-on Easy targeting is part of the aircraft, so it applies even
  when the King turns cheats off. The spec is
  [AC-130 gunsight](spec/ac130-linked-guns.md#the-gunsight), the bytes are in
  [the wire](formats/net-protocol.md#the-gunsight).
- If a flight lead is shot down, a human in the flight takes the lead if there
  is one, otherwise the next AI member ([lead succession](#slots-ai-fill-and-handoff)).
- *Retail gap-fill (agent):* the Cheat menu is available to the King only and
  its settings apply to every human, following retail's host-only rule.
  *Phase 2 design (agent proposal, awaiting John):* the King sets the
  mission's cheats in the lobby, and nobody changes them in flight, so every
  player's prediction runs the same rules all mission long.
- *Designed (phase 2):* friendly fire is a mission setting the King chooses
  (on by default); collisions stay on whatever it says (John, 2026-09-28).
- Leaving never ends the mission for anyone else. *Agent proposal:* only the
  King can end it early.

**Telling friend from foe.** An X appears in the middle of the missile lock box
when the target is on your side, and an IFF squawk (U) on the selected target
answers Friendly for a same-side aircraft, both as in retail. *Retail gap-fill
(agent):* with Show Target Info on, a human's callsign appears beneath the
aircraft's label.

*Built (phase 2, slice F2-C; John took it as designed, 2026-10-05):* the X follows the player's own side, so a player flying
for the enemy sees it on the enemy's aircraft; U answers "IFF: Friendly" for
the player's side and "IFF: no reply" otherwise (fitted); Show Target Info
(the Pref row, Ctrl+T) labels every visible aircraft and object with its
identity, the displayed target with its manoeuvre, in orange, red when it
aims at the player, and a human's callsign beneath
([architecture](ARCHITECTURE.md#friend-or-foe)).

### Ground targets online

*Built (slice N1 of the surface round, protocol 22, 2026-10-10).* A Quick
Mission with a ground target flies online for every player, co-op and PvP,
with the host running every surface unit ([surface
defenses](spec/surface-defenses.md)). What a player sees:

- **One battlefield for everyone.** Each game builds the target, its
  defenses, their jitter and relocation, the SAM batteries, the supply trucks,
  the parked and deck aircraft and both sides' starts from the mission text
  and its seed. The host draws the seed when the mission names none, so the
  layout is new each time a lobby flies a ground target and kept by a
  restart. If a game builds anything differently it says so in plain words
  and stays in the lobby instead of flying a different battlefield ("Your game
  places this mission's ground target differently from the host's...").
- **The front line.** Since slice CL-FL (2026-10-12) the battlefield also
  holds [a front line](spec/surface-defenses.md#the-front-line), a few Blue and
  Red ground units in contact a few miles from the target, built from the same
  seed on every machine and fought by the host like the rest. It adds no wire
  field; it is part of the surface a game builds, so a build without it
  disagrees on the surface digest and is refused in the words above.
- **Area defenses.** Since B23 (2026-10-12) a lobby mission with no ground
  target but an AAA or SAM strength above none flies with [the enemy's area
  defenses](spec/surface-defenses.md#area-defenses), SAM sites and guns on
  Red's side of the front. The host draws their seed as for a target; every
  game builds the same sites, with no wire field added, and a build without
  them is refused in the words above.
- **What moves and fires.** A column of tanks or a convoy of ships drives on
  every screen where the host has it. SAMs launch from their rails, which
  empty and refill when a supply truck rearms them; AAA tracers leave the guns
  and follow their target; flak bursts flash in the air, the KS-19's larger
  than the KS-12's. Radars painting a player show on that player's RWR.
- **Damage and wrecks.** A hit unit's damage, a destroyed unit's wreck and
  fire, a parked aircraft's crater, fire and broken piece, and a damaged
  parked aircraft's smoke reach every player. The debrief's Shot down by row
  names a SAM or gun that downed the player, as in single player.
- **PvP.** The target's units are Redfor: Redfor players defend it (a Protect
  line) and its radars never paint them; Bluefor attacks it (a Destroy line).
  Redfor players start with the enemy group the layout places within 5 nm of
  the target, or, with the mission's enemy ground start, on the ground at the
  Redfor field Enemy Wing 1 parks on (the nearest Red-side field at least 15
  nm from the target when none is picked; slice CL-R). That field's guns and
  SAMs are Red's own and never engage them. A revived Redfor player comes back
  in the air on its side's bearing as before.
- **Host migration and rejoin.** A new host takes the surface from the old
  host's checkpoint (the units' places, routes, hit points, rails, magazines,
  timers and radars), checks every resuming player's surface digest, and
  tells each player and each late joiner every unit not as built.

The bytes are in [the wire's surface world](formats/net-protocol.md#the-surface-world-protocol-22).

### Air-to-ground online

*Built (slice NA of the air-to-ground round, protocol 23, 2026-10-11).*
Everything the round added flies online, co-op and PvP, with the host
working out every solution ([air-to-ground](spec/air-to-ground.md)). What a
player sees:

- **The same cockpit as in single player.** The A-G radar page, the IR
  page's ground objects and the HARM page with their contacts and symbols;
  the targeting sight and its page (LASE, BUDDY, LSR RDY, MASK, the range and
  time to impact); the HUD's CCIP pipper, the level "T", the rocket pipper,
  LOW, FRAG and OVERFLY; the laser box and `LSR` or `BUD`; the guided
  weapons' reach scale and IN RNG; the HARM boresight cue. The keys are the
  same (Ctrl+R, M, T, Ctrl+X under Easy targeting, Backslash and the sight
  keys, `/`, Ctrl+L, Ctrl+M).
- **A round trip late.** The cues are the host's, drawn around the player's
  own aircraft, so they lag the stick by the round trip, as the AC-130's
  pipper does. The targeting page's camera follows the host's look (agent
  decision; the AC-130's own camera turns ahead on the client).
- **Buddy lasing between players.** A laser bomb homes on another player's
  spot of the same side (John, 2026-10-10, decision 6), and the targeting
  page names that player's callsign beside BUDDY.
- **Bombs and guided weapons on every screen.** Every player sees a bomb fall
  (a pack's bombs spread as the host released them), rockets leave their pods
  and a guided weapon steer onto its spot, where the host has them.
- **Pods in the lobby.** The lobby's Load Ordnance offers the Pave Spike, as
  single player does (the Pave Knife was dropped on 2026-10-16; a request naming it
  is built with the Pave Spike on the same station); Cheat loading (the King's `loadouts any`)
  offers the novelty stores too. A player whose import lacks a pod or a
  store someone chose, or has it differently, is told before the flight, as
  for any weapon.
- **Host migration and rejoin.** A new host takes every sight, laser and
  guided weapon in flight from the old host's checkpoint. A player who drops
  and rejoins gets its aircraft back from the AI that flew it meanwhile with
  its cockpit as it left it: the weapon, the sight's track or pin, the
  laser (a hand setting too), the designation and the A-G page. While the
  AI flies, the sight and the laser keep working, so a laser bomb the player
  left guiding keeps following the spot (slice FX-J, agent decision;
  [what is kept](ARCHITECTURE.md#what-a-returning-player-keeps)).

The bytes are in [the wire's air-to-ground round](formats/net-protocol.md#the-air-to-ground-round-protocol-23).

## Debrief

K opens the live score board for every player. It lists human players and AI
by flight callsign, with kills, losses, hits, damage and ratio. AI is marked **AI**;
departed humans are marked **left**. Your numbers accumulate across revivals.
A player leaving, dropping or handing an aircraft to AI does not remove their
contribution from the side totals or the kill limit. Host migration carries
these records too. A different player taking the same lobby number starts
with their own record.

AI kills count under the same weighting as human kills: one for an aircraft,
or two for a human killed with the pilot aboard. Aircraft and helicopters
count toward scores; ground kills remain in the debrief's kill categories.
In PvP, both humans and AI count toward the total, side or individual kill
limit. The individual limit applies to one human or one AI lineage. Co-op
has no kill limit or winner. AI work after taking over a human's plane goes
to the AI row; the human's earlier contribution stays with the human.

The debrief adds **SCORES** and **RESULTS** to the retail pages. SCORES keeps
all listed combatants and side totals, in co-op too. RESULTS lists every
human and AI aircraft, including earlier lives, by side and wing, with type,
status, kills, hit percentage and damage. Compact tables mark AI with **A**
and departed humans with **L**, explained on the page. K displays 12 ranked rows per page; Ctrl+PageUp and Ctrl+PageDown show the rest. A revived aircraft retains
its original flight position, so Red 2 returns as Red 2, for human and AI
pilots alike. Its internal aircraft id still changes.

Leaving before the host ends the mission opens a **PARTIAL REPORT** from the
client's own session record: "You left at 12:34; the mission continued".
Connection failure says "Connection lost". Leave, End Mission, the `?` menu's
Return to Main Menu and Exit all preserve this report. Exit closes the game
after the report is dismissed. The report includes each life and its aircraft,
spawn time and observed ending, revivals, cumulative kills by category,
weapons fired and hits, losses and scores, objectives, aircraft results and
received mission messages. It names the latest statistics time, so a network
outage does not imply knowledge of later events.

A normal mission end uses the full host results. The PLAYER kill and weapon
columns include all the player's lives; physical status describes the latest
life. The wingman column and single-player debrief are unchanged. Full
mission outcome wording still follows objectives; a partial report always
says INCOMPLETE. A player who ends their flight can Join again under the
existing revival and lives rules.

The feature changes follow John's requests of 2026-10-23. Agent presentation
choices and limits have one home in the [debrief spec](spec/debrief.md#multiplayer-mission-record).


## Architecture

There is one netcode path. The **server core** is the authoritative mission: the
flight model of every aircraft, AI, weapons, damage, weather and objectives. It
runs in three ways:

| Mode | Where the core runs | Notes |
| --- | --- | --- |
| Single player | In the game, stepped by the render loop | Pause and time compression work as today |
| Player-hosted | In the host's game, on its own thread | The host's own aircraft is fed directly, with no network delay |
| Dedicated | A headless process with no window, GPU or audio | Configured by file |

The same server core sits at the centre of all three:

```mermaid
flowchart LR
  subgraph single["Single player"]
    direction TB
    s_pilot["Local pilot"] --> s_core["Server core"]
    s_loop["Render loop, with pause<br/>and time compression"] --> s_core
  end
  subgraph hosted["Player-hosted"]
    direction TB
    h_pilot["Host's own pilot,<br/>no network delay"] --> h_core["Server core"]
    h_clock["Own thread,<br/>fixed 120 Hz"] --> h_core
    h_core <-->|"UDP"| h_clients["Remote clients"]
  end
  subgraph dedicated["Dedicated server"]
    direction TB
    d_clock["Headless process,<br/>fixed 120 Hz"] --> d_core["Server core"]
    d_core <-->|"UDP"| d_clients["Remote clients"]
  end
  single ~~~ hosted ~~~ dedicated
```

Other players' games are **clients**. A client predicts its own aircraft locally
and shows everything else interpolated between snapshots from the host. Anything
fixed for dedicated servers fixes player-hosted games too. How one networked
tick flows between a client and the host is drawn in the
[plan](multiplayer-plan.md#client).

### Host selection

By default the host is calculated. Candidates are scored on upload bandwidth and
median ping to all other peers first, then how open their NAT is, then CPU.
Upload weighs heavily, because the host sends state to every client. A peer that
can connect only through the relay is never a calculated host, since every
client's traffic would then flow through the relay. If the best candidate looks
underpowered for the slot count, the King sees a warning. The King can instead
pin a player as host.

*Designed (stage K, 2026-10-05; agent proposals awaiting John unless
credited):* every player's game tells the host its addresses, its system and
processor, and how long its machine takes to step the lobby's mission for two
seconds. The host measures who can reach whom and the round trips between
them, and in the lobby asks the three best candidates for a one-second burst
at the upload the game needs (56 KB/s for each other player at the default
60 snapshots a second, 28 KB/s at 30, and what each
standby's stream needs). A candidate
must not be relayed (John, 2026-09-28), must be reachable by every other
player, and must not have turned off "Let my game take over hosting" in
Options. Candidates rank by upload (passing the test first), then median
round trip, then how open their router is, then CPU. The game stays on the
machine it is on while that machine passes; when it does not and another
does, it moves there in the lobby, never in flight for a better score. The
King's Host row (setting 21) pins a player; a pinned player who leaves falls
back to calculated, and a relayed player cannot be pinned. When no machine
passes, the King reads why in Messages; when no other game can take over, the
King reads "No other game can take over hosting: if you leave, the game
ends." ([architecture](ARCHITECTURE.md#host-selection))

*Built (slice K6, 2026-10-05):* the reports, the reach and upload tests, the
ranking, the pin and its fallback and the King's warnings work on the host;
the move in the lobby needs only its call to the handover (built in slice
K4, `Host::hand_over`), and the Options
switch and the Host row for the screens (slice K7b, built below). A game that hosts
cannot test its own upload in the lobby, so it counts as passing until its
flights show its players missing more than a tenth of its packets (agent
decision). A machine passes the processor test while the busiest minute of
the mission would take under half of one core
([baseline](baselines/host-selection-2026-10-05.md)).

### Host migration

When the host drops, the next-best candidate starts the server core from the
latest state it holds, peers reconnect to it, and the mission continues. Target:
a few seconds of disruption at most, with no mission reset. The AI the old host
was flying keeps flying, because AI runs in the server core. If the King pinned
the host and that player leaves, selection falls back to calculated.

Migration is **exact** (John, 2026-09-28). The new host continues from a
complete checkpoint of the mission: every aircraft, missile and AI pilot,
including what each AI was in the middle of doing and its random numbers. AI
pilots do not change their minds at a migration. The replay format cannot supply
this: replays store what was drawn, rounded, and cannot restart a simulation.
Checkpoints are a new, separate format.

Not every client holds the full picture, because distant aircraft are sent less
often. The host therefore keeps one or two **standby hosts**, the next candidates
in line. It sends them a checkpoint about every 10 seconds, a setting, and every
input it applies in between. A standby that takes over restores the last
checkpoint and replays those inputs to catch up, then carries on:

```mermaid
sequenceDiagram
  participant H as Host
  participant S as Standby host
  participant C as Clients
  loop While the mission runs
    H->>S: Checkpoint of the whole mission, about every 10 s
    H->>S: Every input the host applies, each tick
    H->>C: Snapshots, 60 a second
  end
  Note over H: The host drops
  S->>S: Restore the checkpoint, replay the logged inputs
  C->>S: Reconnect and resend unacknowledged inputs
  S->>C: Snapshots resume from the new host
  Note over S: AI flies the old host's aircraft, reserved for its player
```

When the old and new host run the same operating system and processor type, the
result is exactly what the old host would have computed. Across platforms it can
differ in the last digits, which clients correct like any other prediction
error. See the [plan](multiplayer-plan.md#host-migration-exact-checkpoints).

*Designed (stage K, 2026-10-05; agent proposals awaiting John unless
credited):*

- **Who migrates.** Only a game a player hosts. A dedicated server never
  migrates: when it stops, the game ends, as today.
- **Standbys.** Up to two, the best candidates after the host. A standby on
  the host's own system and processor type simulates the mission alongside
  the host (**warm**, 15 to 40 percent of one core for 30 aircraft), checked
  against the host every 5 seconds, and takes over at once; any other keeps a
  checkpoint every 10 seconds and the inputs since (**cold**), paced at no
  more than 1 Mbit/s, and catches up when it takes over. Everything the host
  does to the mission reaches them through one journal, so a warm standby is
  the host, bit for bit.
- **The host is lost.** After 1.5 seconds without a word from the host, the
  first standby takes over and every player's game moves to it with its token:
  the same session, the same plane, its own aircraft flying on throughout.
  The new host replays the inputs the players' games kept and catches the
  mission up to the present. The second standby takes over only if the first
  does not answer within 3 seconds more. Target: the game flying again within
  5 seconds of the loss, about 2 to 3 seconds with a warm standby.
- **What a player sees.** The HUD says "Lost contact with the host. Moving
  the game to Hawk..." and then "The game moved to Hawk." The other aircraft
  hold still for the gap and then jump to where they are; missiles in flight
  fly on; the debrief keeps every kill from before. If no game can take over
  within 15 seconds, the mission ends with "No other game could take over.
  The host left the game."
- **Leaving on purpose.** When the host's player leaves, the game moves to the
  first standby at once instead of ending, a gap of a round trip or two.
- **What moves.** Players, slots, the King's settings and crown, scores,
  lives, reservations, tokens, the Internet Lobby listing and the relay
  channels. A relayed player keeps its relay channel, which the master moves
  to the new host with the listing. An observer watching with a delay waits
  out the delay again.

*Built in the sessions (slice K4, 2026-10-06), measured on the network
simulator with a warm standby:* standby 1 takes over 1.5 seconds after the
loss, and the players fly on with it 1.5 seconds later, when every seated
player has resumed or the 1.5-second resume window is over (3.0 seconds
after the loss when the old host's own player is lost with it); a handover
takes 20 ms and the players have snapshots again within a quarter of a
second. The game runs it from slice K7a.

*Changed 2026-10-24 (agent decision, 0.2.2):* a player whose game is slow to
send its kept inputs after a takeover (the new host's own game on a busy
machine, say) no longer loses its snapshots for the rest of the flight, and a
standby that needed one fresh checkpoint gets one, rather than one every 10
seconds that it refused, and a takeover that takes seconds to catch up no
longer drops every player the moment it is done: the 5 seconds the players
have to come back count from when the new host can answer them
([architecture](ARCHITECTURE.md#host-migration-and-rejoin)).

The [architecture](ARCHITECTURE.md#host-migration-and-rejoin) has the design
and its slices, the [wire protocol](formats/net-protocol.md#host-migration-and-rejoin-stage-k)
the bytes.

### Dedicated servers

A headless build for Linux, Windows and macOS, configured by file. It can list
itself on the master server or stay private for direct connect. It is the
foundation for later large battles and live campaigns.

A dedicated server simulates flight models, terrain and weapons from the retail
data, so its operator must import their own copy of Fighters Anthology, the same
way the game does. Nothing derived from retail media is ever sent over the
network. Each release publishes the server for Windows, macOS and Linux as its
own download. How to set one up and run it: [dedicated server
guide](DEDICATED-SERVER.md), including
[Docker deployment](DEDICATED-SERVER.md#running-it-in-docker).

## Networking

A session on the internet. Solid arrows are direct traffic; dotted arrows are
traffic the master relays for a player who cannot connect any other way:

```mermaid
flowchart TB
  master["Master server<br/>listings, introductions,<br/>relay, telemetry"]
  host["Host<br/>server core at 120 Hz"]
  a["Client A<br/>predicts its own aircraft,<br/>interpolates the rest"]
  b["Client B<br/>behind CGNAT"]
  host -->|"heartbeat every 30 s"| master
  a -->|"browse and join"| master
  host <-->|"snapshots 60 a second,<br/>inputs 60 a second"| a
  b <-.->|"relayed, both ways"| master
  master <-.->|"relayed, both ways"| host
```

### Master server

A small service, planned for jroverton.com, that lists games, introduces
players to hosts and relays the traffic of those who cannot connect any
other way. Its protocol is versioned from day one. *Designed 2026-10-05;
the master program, `tore-master`, is built (I2, 2026-10-05), with its
introductions (J2) and the relay (J3, on by default), and `tore-bot` joins
through both; the game's own screens are not yet; every item below is an
agent proposal unless credited, and John's decisions of 2026-10-05 are in
the [last table of decisions](#decisions):* the [architecture](ARCHITECTURE.md#master-server-and-connectivity),
the [master's wire](formats/master-protocol.md) and
[running it](MASTER-SERVER.md).

- **The Internet Lounge.** Choose Activity's Multi menu, *Internet...*,
  opens a screen in the Direct Network Lounge's look whose title reads
  INTERNET LOUNGE. It lists the games on the master, refreshed every 15
  seconds: a lock for a password, the name, players over capacity, *Lobby*
  or *Flying*, and a small mark when the master expects the relay. Games of
  another compatibility code or protocol are hidden unless "Show other versions" is ticked, and then
  shown dimmed, as Direct Connection shows them. Selecting a game shows its
  mission and players. **Join** joins it ([connection path](#connection-path)),
  **New** hosts a game that is listed, **Refresh** asks again, **Options**
  holds the port, password and game name (shared with Direct Connection),
  **Master server**, "Forward the game port on my router" and "Send
  anonymous statistics". Master server accepts a host name, IPv4 or IPv6
  address with an optional port (26901 when omitted; use `[::1]:26911` for
  IPv6 with a port). **Default** restores `master.jroverton.com:26901` in
  the field. **OK** saves the choice and queries that master; **Cancel**
  discards the edit. Invalid text keeps Options open and preserves the last
  accepted choice. A valid but unreachable address is named in Messages.
  "Games listed by ..." shows the current master. The saved choice covers
  browsing, hosting with New, introductions, NAT traversal, relay and enabled
  anonymous statistics. See [running your own master](DEDICATED-SERVER.md#running-your-own-master)
  and the [editing rules](spec/multiplayer-menus.md#internet-lounge-master-server).
  Command-line `--master HOST[:PORT]` takes precedence for `--browse` or
  `--host FILE --list` for that invocation, without changing the saved choice.
  Without it, `--browse` uses the saved choice and command-line listed hosting
  uses the public default. `--master` alone is not a lounge override, and
  there is no `TORE_MASTER` environment override.
  *Built (slice I4, 2026-10-05):* the screen is in the
  game; Join asks the master to introduce the player and races the host's
  addresses from the same socket (J2); *built (J5):* when no address answers
  within 3 seconds the game asks for the relay and joins through it, and
  Messages says each step ([the build's notes](ARCHITECTURE.md#joining-through-the-master)).
- **What is listed.** A game hosted from the Internet Lobby is listed; one
  hosted from Direct Connection is not. Once the King's Visibility setting
  exists (stage F phase 2), *public* lists, *private* does not and
  *password* lists with the lock. A dedicated server lists itself when its
  configuration says `broadcast on`, off by default (John, 2026-10-05). The lobby's Messages say
  whether the game is listed and, when the router lets players in, the
  address they reach.
- **Heartbeats.** A listed game sends its summary (name, mission, players,
  King, password, lobby or flying, build) every 30 seconds and 5 seconds
  after a change, and a small packet every 15 seconds that keeps the router's
  port open. A listing not heard from for 90 seconds disappears; a host that
  stops removes it at once.
- **Abuse limits** (the [open question](#open-questions)'s proposal, now
  designed): a host proves its address before it is listed; every request
  is limited per IPv4 address or IPv6 /64 network; at most 8 listings from
  one; the master never answers an unproven sender with more bytes than it
  sent; the relay carries only pairs it introduced, at most 128 KB/s each way
  per pair and within a monthly allowance ([numbers](formats/master-protocol.md#limits)).
- **When it is down,** the Internet Lobby is empty. Local games, Direct
  Connection and joining by address never use the master. A listed game or
  server keeps asking, round the master's addresses, and is listed again
  within 15 seconds of the master answering, with no restart (0.2.2,
  [finding the master again](formats/master-protocol.md#finding-the-master-again)).

### Connection path

A player joining from the Internet Lobby reaches the host the first way that
works:

1. **Port mapping** by the host. *Agent proposal, designed:* a hosting game
   asks its router to forward the game port by UPnP, NAT-PMP or PCP, whichever
   the router speaks, on by default with a switch in Options; the lobby's
   Messages say whether it worked and the address friends can join at. It
   also helps a friend joining a Direct Connection game by address. *Built
   (J4b):* both Options panels have the switch; `tore-app --map-port SECONDS`
   checks a router by itself; a dedicated server does it with `port-mapping on`
   ([how it works](ARCHITECTURE.md#port-mapping-in-the-hosts-as-built-j4b)).
2. **Direct IPv6**, *agent proposal*. Many CGNAT providers, including Starlink
   and T-Mobile Home Internet, give customers public IPv6 addresses, so two IPv6
   players can often connect directly without the relay.
3. **NAT hole punching**, with the master introducing both peers: each starts
   sending to the other at the same moment, which opens both routers.
4. **Relay** through the master for everything else, including CGNAT without
   IPv6. Flight sim state is small, so relay cost stays low.

*Designed 2026-10-05:* the first three are tried at once, every address of
the host together, and the relay is asked for when none answers within 3
seconds, or at once when the master's router test shows punching cannot
work. A join through the relay takes about 4 seconds; most take well under
one. *Built (J2):* the introduction, the host's punches and the race, for
`tore-bot --master --listing` and on the network simulator, where every
pair of routers that can punch connects in about half a second at a 100 ms
round trip ([the punching table](ARCHITECTURE.md#hole-punching)). *Built
(J3, J5):* the relay, for `tore-bot` and for the game's Internet Lobby; the
whole join gives up 15 seconds after Join is pressed.

```mermaid
flowchart LR
  join["A player joins from<br/>the Internet Lobby"] --> race{"One of the host's<br/>addresses answers<br/>within 3 seconds?"}
  race -->|"its mapped port"| direct["Direct connection"]
  race -->|"its IPv6 address"| direct
  race -->|"a punched hole"| direct
  race -->|"none"| relay["Relay through<br/>the master"]
```

**Shown and reported.** *Agent proposal:* the join's last line says how the
player connected ("Connected directly (IPv6).", "Connected through the
relay."), the lobby marks a relayed player beside the platform mark, and the
path is in the network log, the dedicated server's log and the anonymous
statistics. *Built (J5):* the join's lines, a `path` line in the game's
network log and the path in the player's statistics. *Built (J6):* the lobby's
mark for a relayed player, the selected player's line and the dedicated
server's join lines. The paths: local network, by address, mapped port, IPv6,
punched, relay. A port forwarded by hand reads "punched", since the game
cannot tell the two apart. A relayed player is never the calculated host
(John, 2026-09-28).

Direct connect by address stays available for dedicated servers and LAN, and
skips the master entirely. *Built (EF7):* the game's Direct Connection screen
joins a typed address or name, and lists games on the local network, without a
master.

### Netcode model

Host-authoritative. The sim runs at 120 Hz. The host sends state snapshots at
60 Hz by default (John, 2026-10-06, raised from 30 once John's three-machine
test had passed), a dedicated server's setting that a weak uplink can lower
(a game a player hosts has no such setting yet). Each client predicts its own
aircraft and the host corrects it. Other aircraft are interpolated between
snapshots. Snapshots are delta-compressed, with relevance filtering so distant
contacts update less often.

**Hit authority.** Missiles are resolved by the host. Gun hits are resolved by
the host after rewinding targets to where the shooter saw them (lag
compensation).

### Netcode numbers

Stage D design of 2026-09-30, reviewed by John the same day
([decisions](#decisions)). Every number is an *agent proposal* unless it is
credited to John; stage D measures them and its baseline replaces the
estimates. How they are used is in the
[architecture guide](ARCHITECTURE.md#network-sessions), and the bytes in the
[wire protocol](formats/net-protocol.md).

**Rates.**

| Item | Value | Source |
| --- | --- | --- |
| Simulation | 120 ticks a second | John, 2026-09-28 |
| Snapshots | 60 a second to each player (every second tick, each seat on its own tick of the two: seat number modulo the ticks per snapshot, an agent decision so a full server does not build all at once); a server setting of 10, 12, 15, 20, 24, 30, 40 or 60, the rates that divide 120; in a game a player hosts, the King's lobby setting of 60 (default), 30 or 20 | John, 2026-10-06 (30 a second from 2026-09-28 to 2026-10-06; the King's setting, Q56); the other rates are an agent proposal |
| Relay | Each relayed channel up to 128 KB/s each way, bursts of 256 KB (64 KB/s before 2026-10-06); a relayed player's busiest second at 60 snapshots a second is 63 to 74 KB/s | John, 2026-10-06 (Q55) |
| Inputs | Up to 60 packets a second, each repeating every unacknowledged tick up to 24 ticks (200 ms) | Agent proposal |
| Keepalive | At least 10 packets a second each way; a joined game whose loop is stalled sends a [Keepalive](formats/net-protocol.md#keepalive) once a second, for at most 60 seconds (EF-K); a host building a mission sends each player a loading Keepalive twice a second, for at most 3 minutes (protocol 28, slice NET2) | Agent proposal; host loading John 2026-10-13 |
| Packet size | At most 1,200 bytes | Guide |

**Clocks, delays and smoothing.**

| Item | Value |
| --- | --- |
| Input margin | The client keeps its clock ahead of the host so that the smallest margin by which its inputs arrived over the last 2 seconds is 1 tick plus one input packet's interval (3 ticks, 25 ms, at 60 packets a second), and one interval more while loss over the last 10 seconds is above 1 percent; a single lost packet then costs nothing |
| Clock steering | The client's clock runs between 0.98 and 1.02 times real time; it jumps only when more than 250 ms off. *Correction (D8a):* the first margin the host reports after seating also sets the clock outright, since the Seated message can arrive late and seating snaps anyway |
| Missing input | The host repeats the player's last stick, throttle, trigger and scope controls with no commands; a command that arrives late is applied on the next tick. After 60 ticks (half a second) in a row with no input, or once the player's keepalives arrive, the game counts as stalled and the seat flies a paused game's neutral controls until its next input (EF-K follow-up; the threshold is an agent decision) |
| Interpolation delay | Starts at 100 ms and adapts between 50 and 250 ms, keeping the drawn time at least 2 ticks behind the newest snapshot over the last 2 seconds, or 6 ticks while loss over the last 10 seconds is above 1 percent |
| Interpolation | A cubic curve through two snapshots' positions and velocities; attitude turns the short way between them |
| Extrapolation | Up to 250 ms along the last motion when a snapshot is late, then the aircraft holds |
| Own aircraft check | Every snapshot carries a hash of the host's state of the player's plane at tick N, compared with the prediction for tick N; equal means no correction. The exact state follows when the host knows the prediction cannot match, when the client reports a mismatch, and at least once a second |
| Correction blend | The drawn aircraft slides from its old path to the corrected one with a 50 ms time constant: 95 percent of the way within 150 ms |
| Snap | No blend when the correction is over 100 ft or 20 degrees, or when the player is seated |
| Too small to show | A correction under 0.01 ft and 0.01 degrees is applied without a visible blend (last-digit differences between platforms) |
| Across platforms | A client on another operating system or processor than the host differs in the last digits every tick, so it reports a mismatch at each snapshot and receives the exact state every time: about 6 to 10 KB/s more, with corrections too small to show |

**Hits.**

| Item | Value | Source |
| --- | --- | --- |
| Missiles, rockets, bombs | Decided by the host, not rewound | John, 2026-09-28 |
| Gun rounds from a human | Tested against targets as the shooter's screen showed them: the rewind is the whole round trip plus the interpolation delay plus the input margin | John, 2026-09-28; the formula corrects the plan's estimate |
| Lag compensation cap | The part of the rewind beyond the interpolation delay is capped at 250 ms (30 ticks), so the whole rewind is at most 500 ms; the host keeps 1 second of history | Plan (250 ms for latency); agent proposal (how it applies, history) |
| Own tracers | Drawn at once on the shooter's screen; a missile appears when the host launches it, one round trip after the trigger | Agent proposal |

**Relevance.** How often each client hears about each entity (John,
2026-09-30: every snapshot near, a slower rate for the rest, smoothed; John,
2026-10-06: the near band at 60 a second and the rest 4 times a second,
raised from 30 and twice a second, accepting about double the bandwidth):

| Band | Rate | Priority weight |
| --- | --- | --- |
| The player's own flight, anything within 20 nm, anything a friendly sensor tracks, any missile aimed at the player, any missile within 10 nm, and whatever the player's view follows (the target, wing, external and fly-by views' subject) | Every snapshot (60 a second) | 1 |
| Everything else | 4 times a second at most | 1/15 |

A band sets how often an entity is due: a near one every snapshot, a far one
every fifteenth snapshot at 60 a second. At a lower snapshot rate the far band
is the rate over 4, rounded up, so it stays evenly spaced and never above 4 a
second: 3.75 a second at 30 and 15 a second, 3.33 at 10, 4 at the others
(*agent decision*, D12; [the table](formats/net-protocol.md#connecting)). A due entity waits only when the packet is full, and then goes
first next time. In a 30-aircraft LAN mission everything due is expected to fit
every time (measured in D6: it does, but for a snapshot now and then in
missile-heavy moments while 256 bytes are kept for messages). *Correction
(D6):* the design called a band only a priority, which would have sent far
entities at the full rate whenever there was room.
The view rule is an *agent decision*, so that an aircraft the player watches is
never a slow one.

**Smoothing the slow ones** (John asked that they never jitter; the method is
an *agent decision*). An entity sent 4 times a second is drawn further in the
past than the others: its own update interval plus the normal delay, about
300 ms (about 600 ms while it was sent twice a second), on the same curve
through its updates, so the client never has to guess ahead of it and it
moves as smoothly as a near one. When an entity changes band its delay slides
to the new one at no more than a tenth of real time (a quarter second of delay
over two and a half seconds), so its speed never visibly jumps. At 20 nm and
beyond, being a quarter second in the past is invisible; radar, RWR and the
target window come from the host's own readout, not from the drawn aircraft.

**Connections.**

| Item | Value |
| --- | --- |
| Connecting | The client repeats each handshake step every 250 ms and gives up after 10 seconds |
| Dropped player | 5 seconds without a valid packet; the AI takes the plane at once. A game whose loop is stalled (a window dragged, a long frame) keeps its plane for up to 60 seconds through its [keepalive](ARCHITECTURE.md#a-stalled-game-stays-connected-ef-k) (EF-K, agent decision), flown neutral meanwhile. The same 60 seconds cover a game building the mission as it joins or as a flight starts, which a slow machine can take longer than 5 seconds over ([a long mission build](ARCHITECTURE.md#a-long-mission-build-slice-net), slice NET, agent decision). A host building a mission (the King's change, a flight with loadouts, the next mission) keeps every player for up to 3 minutes: it tells them it is loading twice a second, and while it says so a player waits 4 seconds of silence, not 1.5, before it counts the host lost and races the standbys (protocol 28, slice NET2; John 2026-10-13, the 3 minutes fitted, the 4 seconds an agent decision; [wire](formats/net-protocol.md#protocol-28-host-loading)) |
| Players per server | A setting, default 30 (John, 2026-09-28); a co-op mission seats at most its 15 friendly planes |
| Default port | UDP 26900, a setting |

**Stage D acceptance limits.** A server and two headless bot clients fly a
scripted 5-minute fight in the network simulator, once per combination of
round trip (50, 150 and 300 ms, with arrival spread of plus or minus 10 percent
of the one-way delay) and loss (0, 2 and 5 percent each way, plus 1 percent
duplicated packets):

| Measure | Limit |
| --- | --- |
| Own aircraft, same platform, with no late input and no hit, blast or weapon release in the last second | No correction at all: the prediction equals the host bit for bit |
| Own aircraft, all snapshots | At least 99 percent need no visible correction at up to 2 percent loss, 97 percent at 5 percent; outside the second after a hit, blast or release, 99 percent of corrections are under 1 ft |
| Other aircraft | The drawn position against the host's at the same moment: 99 percent of frames within 1 ft at no loss, 3 ft at 2 percent and 10 ft at 5 percent |
| Extrapolated frames | Under 1 percent at 2 percent loss, under 3 percent at 5 percent |
| Inputs the host had to repeat | Under 0.5 percent of ticks at up to 2 percent loss, under 2 percent at 5 percent, after the first 5 seconds |
| Bandwidth | Measured each way per player and recorded against the [plan's budget](multiplayer-plan.md#bandwidth-budget) |

*Measured (D10):* the matrix is a test in `tore-session` (`client/matrix_tests.rs`),
a short form of 60 simulated seconds a cell in the normal suite and the five
minutes an ignored test; every limit holds in all nine cells, with the own
aircraft needing no correction at all after the first second of seating
([baseline](baselines/net-2026-09-30.md)). The mission is the synthetic
fixtures', whose AI never fires, so no bot is hit; corrections after a hit are
covered by the D4 and D8a tests instead.

### Transport

Hand-rolled UDP on the standard library's sockets, identical on Linux, Windows
and macOS. A thin layer on top provides sequence numbers, acks, a reliable
ordered channel (lobby, chat, slot changes, orders) and an unreliable channel
(state). Packets stay under about 1,200 bytes, below common MTU limits. Traffic
is not encrypted in v1, so a password keeps strangers out of a lobby but is not
secret from someone who can watch the network.

### Compatibility handshake

On join, the game compares:

- The protocol and eight-digit [compatibility code](ARCHITECTURE.md#compatibility-code), which must match. Commits and release versions are information only.
- A manifest of content hashes per aircraft, theater and weapon set.

T.O.R.E imports only Fighters Anthology, from the disc's 1.0 build or the 1.02F
patch ([first-run import](spec/first-run-import.md)). USNF-only and ATF Gold
installs cannot run it, so today's differences come from the FA build and, later,
from mods and custom aircraft such as the [F/A-XX](spec/fa-xx.md). The lobby
only offers aircraft and theaters that every human in the session has. A player
with a different playable import is refused with both compatibility codes. The detailed mission check still explains differences after admission.

Since protocol 71, different commits and tags can see and join each other
when their codes match. **Show other versions** includes games whose code
or protocol differs, dimmed and not joinable. The selected game's details
and game lobby show **Code XXXXXXXX**. The code covers the whole playable
import, including unused items, and remains stable when the mission changes. The content is checked once a mission loads: each game builds the
mission from its own import and compares the files it read with the host's. A
1.0 import and a 1.02F import pass it, since every file the simulation reads
is the same in both (slice D3a, [the comparison](formats/esa-installer.md#the-import-pack-under-both-builds)).
[The builds audit](baselines/fa-builds-2026-10-06.md) compared everything the two
imports keep and found nothing a player sees or hears that differs.
A game that fails it stays in the lobby, marked unable, with a list of file
names.

*Designed (stage L, 2026-10-05; agent proposals awaiting John):*

- **Knowing before the mission.** Each game works out its **content**: every
  aircraft, theater and weapon it can load and the shared data every mission
  reads, each with a fingerprint of exactly the files it reads. It sends this
  to the host when it joins, with the Fighters Anthology build it imported
  and the T.O.R.E version that made the import. The host compares every
  player's content with its own and tells everyone what not everyone can use
  ([how](ARCHITECTURE.md#compatibility), [the wire](formats/net-protocol.md#compatibility-stage-l)).
- **The King's choices.** In the lobby, the Quick Mission creator dims an
  aircraft or a theater that some player cannot use and says why ("Not
  everyone can fly the Su-27: Hawk's game has no Su-27."); Load Ordnance does
  the same for a weapon. The host refuses such a mission or loadout with the
  same words.
- **A missing aircraft is explained, not a failed join.** A player joins
  whatever its import holds. If it cannot fly the lobby's mission, it is
  marked unable and every player reads why: "Hawk's game has no Su-27, which
  this mission flies." The player's own screen says what to do: "Re-import
  Fighters Anthology (Pref, Re-import) to add it."
- **Builds are not shown** (John, 2026-10-06: removed; the build audit found
  no difference a player sees). Selecting a player in the lobby shows its
  platform ("Hawk: on Linux."). When a player's content differs from the
  host's, Messages says how, once: "Hawk's game differs from the host's: no
  Su-27." Nothing is said when they are equal, and nothing about which
  Fighters Anthology build a player imported. The build still travels on the
  wire and in the server's log.
- **Where differences come from.** Between 1.0 and 1.02F, none that a player
  can fly or fight with. In practice, an import made by an older T.O.R.E,
  which lacks aircraft added since; a damaged file; and, later, mods. A mod's
  new aircraft is offered only once everyone has it.
- **A dedicated server** keeps the mission its file names; a player who
  cannot fly it is unable, with the words above, and the server's log and
  `tore-server --check` list the content
  ([server guide](DEDICATED-SERVER.md)).
- Single player does not change.

*Built (L4, 2026-10-05):* the game shows all of it. It works out its content
once, in the background, when Direct Connection or the Internet Lobby opens, so
a join does not wait. In the lobby the King's Quick Mission creator dims an
aircraft or a theater not everyone has, and choosing one leaves the choice as
it was and says who lacks it ("Not everyone can fly the Rafale C: Hawk's game
has no Rafale C."); Accept checks the whole mission the same way. Load Ordnance
darkens a weapon not everyone has and refuses it in the same words. Selecting a
player shows its system and how it connected ("Hawk: on Linux. Connected
directly."), an unable player's reason is shown as the host worded it, and
Messages says once how a player's game differs from the host's. Nothing is said
when they are equal. There is no new mark in the player list and no warning on
the Internet Lobby (John, 2026-10-05). *Changed (L5, 2026-10-06):* the lobby no
longer shows a player's Fighters Anthology build, in the hint or in Messages
(the audit of the 1.0 and 1.02F builds found them identical in everything a
player sees or the simulation reads).

## Replay and telemetry

**Replay** stays separate from netcode and records on each local machine. A
client's recording holds what that client saw. Besides flight, AI decisions and
comms, it logs network diagnostics: round-trip time, packet loss, snapshot
arrival and prediction corrections, so a replay can explain a rubber-banding or
desync report. Replays also record slot changes, joins, drops, host migrations
and data link events.

**Telemetry.** The heartbeat and the session end report anonymously to the
master selected in Internet Lounge Options, including a privately operated
master. A listed dedicated server uses its `master` setting; command-line
listed hosting uses its `--master` choice or the public default:

- An anonymous install ID.
- Session length and human count.
- Connection path used: UPnP, punch or relay.
- Host migrations and whether they succeeded.

This measures real use and sizes the relay before it becomes a problem.
*Agent proposal:* a Pref switch turns telemetry off, and the README says what is
sent.

*Designed 2026-10-05 (agent proposals; the default and the contents are
John's to decide):*

- **Who sends.** Only games that use the master: a game hosting or joining
  through the Internet Lobby, and a listed dedicated server. Direct
  Connection and single player send nothing.
- **What.** One report when such a session ends: the install id, whether the
  game played, hosted or served, its version and platform, the session's
  minutes and most humans, how the player connected and how long it took,
  how the router maps the game port, which port-mapping method worked,
  bytes through the relay, a host's players counted by path, and (stage K)
  host migrations and how many failed. A listed game's registration carries
  the install id beside the listing. The bytes are the master's
  [Report](formats/master-protocol.md#reports).
- **The switch.** "Send anonymous statistics" in the Internet Lobby's
  Options, on by default, with one line in Messages the first time the
  screen opens; a dedicated server's `telemetry` setting. Turning it off
  deletes the install id, and turning it on again draws a new one.
- **What the master keeps.** Counts per day, never an address with them;
  distinct installs are counted with a salt drawn each day and never written
  down ([details](MASTER-SERVER.md#what-the-master-keeps)).
- The README says all of this in plain words.

## Relation to Fighters Anthology

Fighters Anthology had multiplayer: up to eight players over LAN or TCP/IP, two
over modem or serial cable ([retail multiplayer](spec/multiplayer.md)). The
design above keeps its roles and several of its rules, and departs from it in
these places:

| Topic | Fighters Anthology | T.O.R.E v1 | Label |
| --- | --- | --- | --- |
| Connections | Serial, modem, IPX LAN, TCP/IP by typed address | Server browser, NAT traversal and relay; direct connect by address | Opinionated, John 2026-09-28 |
| Players | 2 to 8 | Up to 30 humans | Opinionated, John 2026-09-28 |
| Host leaves | Game ends for everyone | Host migration | Opinionated, John 2026-09-28 |
| Client leaves | Game continues | Game continues; a clean departure frees the identity and aircraft, an unclean drop keeps them for rejoin | Opinionated, John 2026-09-28 |
| Mission types | Single Mission, Quick Mission, Airbase Assault | Quick Mission only | Scope, John 2026-09-28 |
| Pause | Any player pauses everyone | No pause | Opinionated, John 2026-09-28 |
| Time compression | None | None | Matches retail |
| Mission control | Host sets parameters and cheats; others see them grayed out | King sets them; others see them grayed out | Matches retail, with the King in the host's role |
| Aircraft choice | Each player picks their own type | Each human takes a slot in a King-built wing | Opinionated, John 2026-09-28 |
| AI | AI wings from the creator | AI flies every open slot and takes over dropped players | Opinionated, John 2026-09-28 |
| Sides | Each player picks Friendly or Enemy | Co-op: humans on blue. PvP: any side | Opinionated, John 2026-09-28 |
| Start | Everyone starts airborne | Airborne, or on the ground at a side's own field when the King's mission parks that side's first wing | Airborne matched retail (John 2026-09-28); ground starts for both sides since John's Redfor ground start request, 2026-10-11 (Blue's in the lobby: agent decision) |
| Death | Revive with Enter; host sets lives, delay, distance and weapons | The spec's respawn rules, with retail's revival settings | Both, John 2026-09-28 |
| Scoring | Kill tally, time and kill limits, kill owner | Retail's scoring settings in PvP | Matches retail, John 2026-09-28 |
| Chat | `~` with five receivers and `CHAT.TXT` | Retail keys and receivers; quick messages | Matches retail, John 2026-09-28; quick messages are a retail gap-fill (agent) |
| Telling friend from foe | X in the lock box, names under callsigns, IFF squawk (U) | All three kept | X and IFF: John 2026-09-28; names: retail gap-fill (agent) |
| Ctrl+Q exit | Ends the mission for everyone | Leaves; only the King ends the mission | Opinionated, agent proposal |

## Decisions

Made by John on 2026-09-28:

| Question | Decision |
| --- | --- |
| Tick rate | Sim at 120 Hz; netcode snapshots at 30 Hz, a setting to raise after testing (raised to 60 Hz by John on 2026-10-06; the row Snapshot rates, last table) |
| Replay and netcode | Separate; replay records locally and logs network diagnostics for troubleshooting |
| Transport | Hand-rolled UDP on standard sockets with a thin reliability layer |
| Hit authority | Missiles resolved by the host; guns resolved by the host rewinding to the shooter's view (lag compensation) |
| Data link | Shared track picture sent at 4 Hz; locks and assignments sent immediately as reliable events |
| Rejoin | Indefinite while the aircraft is alive, reserved for the player; otherwise observe until the round ends |
| Observer mode | Replay controls on the live session; no chat with any team; optional observer delay in PvP |
| Identity | Callsign, no account; duplicates get a suffix (Viper_2); a server-specific rejoin token lasting 24 hours |
| Max humans | Default 30, configurable; revisit after load testing |
| Lobby control | King (crown) controls the lobby and can hand it off; Host (house) is calculated or pinned by the King |
| PvP | In v1: humans pick any side and can take any AI slot mid-game; the King can lock sides |
| Relay | Linode shared 2 GB plan with 1 TB monthly transfer; relayed peers are never the calculated host |

Made by John on 2026-09-28 while planning:

| Question | Decision |
| --- | --- |
| Retail rules | The feature spec wins where it speaks; where it is silent, retail multiplayer's rule fills the gap: revival and scoring settings, chat keys and receivers, airborne start, lock-box X and IFF squawk |
| Migration fidelity | Exact checkpoint, including AI thinking and random number states, rather than rebuilding from a partial state |
| Flight model | Every aircraft in a multiplayer mission flies the hybrid model; single player unchanged |
| Comm rose | Not in v1; keep the Alt-key orders and add reply and request keys |

Made by John on 2026-09-28 at the review of the stage A and B design
([architecture](ARCHITECTURE.md#mission-core-and-seats)):

| Question | Decision |
| --- | --- |
| Missile hits | A missile can hit anything once it leaves the aircraft that fired it: an aircraft that gets in the way, or a new target it shifts to. Single player too |
| Lead succession | A human in the flight takes the lead if there is one; otherwise the next AI member does. Single player too |
| Mission result calls | "Mission accomplished" and "almost home" (the two that exist) are generated by the mission core for every player, whether or not a sound device is present |
| Damage rules | Follow the pilot: a human-flown aircraft keeps today's player rules, an AI-flown one the AI rules; a handoff keeps the fraction of damage |
| Aircraft record | A registry of aircraft flown by the AI or by human seats; a handoff converts stores and damage exactly (agent proposal, approved) |
| Collisions | Always on, whatever the friendly-fire setting |
| Refactor window | Stages A and B may change the AI whenever it makes sense; no other work is going in |

Made by John on 2026-09-29 at the stage B review
([architecture](ARCHITECTURE.md#mission-core-and-seats)):

| Question | Decision |
| --- | --- |
| Wing order call | The call cuts off the wing lines still playing and holds the sender's radio channel for its length, for every seat and whether or not a sound device plays it. Single player too |
| Kill credit | Every shooter is credited: an AI that shoots down a human-flown aircraft gets the kill in the debrief and the mission recording, as a human does. Single player too |
| Merge from main | When the bug bash lands on main, merging it into the multiplayer work is its own stage, a new C, and the later stages move down one letter. Built 2026-09-29 ([how stage C landed](ARCHITECTURE.md#how-stage-c-landed)) |

Made by John on 2026-09-30 at the stage D design review
([architecture](ARCHITECTURE.md#network-sessions)):

| Question | Decision |
| --- | --- |
| Menus in a networked flight | Nothing pauses. While the pause or Esc menu is up the controls go neutral (stick centred, throttle held, trigger released); a window that loses focus counts as paused (agent reading). Whether the AI takes over after a while stays open for stage F. *The lead's application, 2026-10-01:* a joined game whose loop is stalled (a window held, a long frame) counts as paused too: once the host has had no input from it for half a second, or hears its keepalives, it flies the seat with the same neutral controls until the next input ([the stall rule](ARCHITECTURE.md#a-stalled-game-stays-connected-ef-k)) |
| Relevance | 30 updates a second for what is near or tracked, twice a second for everything else, smoothed so those never jitter ([netcode numbers](#netcode-numbers)); raised to 60 and 4 a second by John on 2026-10-06 |
| Recordings of networked flights | In stage D each client keeps a capture of what the network brought, with a diagnostics log, instead of recording a replay live. A capture converts into a replay whose aircraft follow a smooth curve through every update received, using hindsight, rather than what the player saw live; the conversion is built (stage E, [network flights](REPLAYS.md#network-flights)): it runs when the flight ends and by `--convert-capture`. It replaces the 2026-09-28 rule that each machine records its own replay |
| Stage D acceptance | Agents smoke-test a dedicated server with clients on the development machine; John then tests on three machines on his LAN, macOS, Linux and Windows |
| Stage D agent proposals | Approved as designed: the five new crates, UDP port 26900, the server's mission lifecycle, the build match rule, a joining player keeps the plane's loadout, own tracers at once and own missiles when the host launches them, 1.0 and 1.02F imports together once verified, and the retail stall-speed switch refused in networked play |

Made by John on 2026-10-01 for stages E and F
([lobby and hosting](ARCHITECTURE.md#lobby-and-hosting)):

| Question | Decision |
| --- | --- |
| Testing stage D | Networking is done; John tests everything together on his three machines once the menus exist, not stage D from the command line |
| Direct play first | The first mode is a direct, unpublished lobby for friends: games broadcast on the local network are listed, and a player connects directly to an IP address or a domain name. A public lobby is stage I |
| Hosting and the mission | A player hosts from inside the game and builds the mission with the Quick Mission creator as the template |
| The look | Retail backgrounds and pieces, not necessarily retail layouts: the NETWORK CONNECTION background (`NETIPX3`, a grey photograph; the red MODEM CONNECTION photograph was used until 2026-10-05) with its title bar, which a player can reword with a PNG of their own, and the TCP/IP Network connection panel, retail buttons and other pieces |
| Re-import | Expected: every player re-imports once for the new screens' art |
| Phase 1 | Everything the base test in the game needs: Direct Connection, hosting with the creator, the lobby with slots, loadout, ready, chat and start, flight with chat, the debrief and the return; the King's settings are phase 2 |
| Loadouts | Each player chooses their own loadout as part of the lobby and marks ready once it is chosen. Phase 2 adds a setting: any store on any aircraft, or restricted to what each aircraft carries |
| Chat in flight | The `~` key (backtick) opens the chat line, since Enter designates in flight; while it is open Tab chooses the receiver and Enter sends. The chat window is at the top left, coloured green for the player's side, blue for a line to everyone from the player's side, red for a line from the enemy |
| Chat window place | John, 2026-10-01: top left, unless the player uses large instruments (the default layout; the Small layout is six across the bottom). Then it sits on the left side in the gap between the upper-left and lower-left instruments, covering neither. The game chooses from the instruments' layout setting each frame, so changing the setting moves the window; the lines are fitted to the gap at every window shape (fewer lines when the room is short) |
| Observers and chat | John, 2026-10-01: observers cannot chat to the active players, but can talk amongst themselves. While a mission flies, a connection with no plane (in the lobby, not flying) is an observer. *The lead's reading of who hears whom, implemented in EF6:* an observer's lines go only to other observers, and their All reaches only the other plane-less connections; observers still receive the flying players' lines to All; flying players send and receive as before; with nothing flying everyone is in the lobby and All reaches everyone |
| After a mission | Everyone returns to the lobby, still connected |
| Text fields | John, 2026-10-01: no red and grey text areas. Every field a player types in on a multiplayer screen (the lobby's chat line and Kick reason, Connect to, the Options port, password and game name) is a plain grey recessed box with readable panel text, like NEWNET's Callsign field. Retail's red edit control (`EDITL/M/R`, wide-spaced `WHEELFNT`) is not used anywhere, and the widget kit no longer carries it |
| Screen background | John, 2026-10-05, replacing the 2026-10-01 look (the red `MODEM CONNECTION` photograph with `NETIPX3`'s title bar over it): the screens draw `NETIPX3` alone, its grey photograph under its own title bar (its top 77 rows: the chrome, the badge, the "?" help bar). The retail lettering NETWORK CONNECTION is covered with a copy of the bar's own texture (from the player's import, in memory) and the lettering DIRECT NETWORK LOUNGE, shipped as `assets/direct-network-connection-title.png` (the words in Liberation Sans, open licensed, over a dark offset copy as a shadow, on a transparent background, no retail pixels; John first set it in Helvetica and swapped it for this on 2026-10-05 so it ships cleanly), is drawn over it, fixed to the bar's top right. A `DirectNetworkConnection.png` in the data folder, if there, is used instead of the shipped lettering (any size up to 640 wide, fixed to the top right, transparency honoured). That file is the player's own and is never in the repository or a package if it carries retail's badge or texture |
| Platform beside the name | John, 2026-10-05: the lobby shows each player's platform, a Windows, macOS or Linux mark, next to the player's name. *Built (agent decisions):* each game sends the system it was built for when it joins (Windows, macOS, Linux, or unknown for any other), and the host lists it with each player in the lobby state (protocol 7, [the wire](formats/net-protocol.md#connecting)). It is only shown; nothing in the session depends on it. Drawing the marks is the lobby screen's part, a separate change |
| Three-machine test | John, 2026-10-05: ran the local three-platform test (macOS, Linux and Windows, protocol 7 builds from `48d62dac`) and confirmed it works. Recorded from his report: no logs, frame figures or list of what was flown were kept, and which machine hosted was not noted |

Made by John on 2026-10-05 on the designs for the rest of the milestone: the
data link ([DATALINK.md](DATALINK.md)), stage F phase 2
([phase 2](ARCHITECTURE.md#phase-2-the-rest-of-stage-f)) and the master server
and connectivity ([architecture](ARCHITECTURE.md#master-server-and-connectivity),
[operations](MASTER-SERVER.md)):

| Question | Decision |
| --- | --- |
| Who has the data link | Every friendly aircraft, whatever its type: the design's Voice, Flight and Network tiers are dropped. An aircraft with no radar is still linked, but its player sees no link cues on displays it does not have |
| Single-player AI changes of the data link | Approved, each landing with its own baseline report: the AI counts the player's lock, linked wingmen take a target only a flightmate tracks, AI leads share and sort, linked AI yield |
| Order voice | An assignment is called with its geometry ("Two, attack bandit, bearing 270, 15 miles, angels 20"); a blanket attack order to the wingmen is called "Attack bandits" |
| Battle net | In v1: tracks and locks shared across flights of a side, flight leads' reports by voice to seats that monitor it (off by default; the voice and Alt+N built in slice G8); the AWACS report waits for sentry aircraft |
| Data link keys | Alt+A sorts the flight's targets; Alt+N monitors the battle net |
| Where the master runs | John's own Linode at jroverton.com, `master.jroverton.com` (A and AAAA records), UDP 26901 and 26902; John opens the ports and deploys it |
| Telemetry | On by default, with a switch and a one-time notice, sending only what [Replay and telemetry](#replay-and-telemetry) lists |
| A dedicated server's listing | Off by default; the operator turns broadcasting on in its configuration (as OpenRA's servers do) |
| Port mapping | On by default while hosting, with a switch; the mapping is removed when hosting stops |
| Relay cap | New relay channels are refused at 95 percent of 800 GB a month and open ones closed at 100 percent; the relay is on; a stalled relayed game stays connected (framed keepalives); John confirms the plan's transfer allowance on the account |
| Master abuse limits | Approved as in the [master protocol's limits](formats/master-protocol.md) |
| Internet Lounge title | INTERNET LOUNGE, lettered like DIRECT NETWORK LOUNGE; a player's own `InternetLobby.png` takes its place. Game lobbies have their own origin-specific titles ([specification](spec/multiplayer-menus.md)) |
| Master in releases | The release workflow also publishes a Linux `tore-master`, built on Ubuntu |
| Respawn | Retail's revival (a new aircraft out of the battle, with the revival weapons), beside taking a free AI aircraft (`ai-slot`) and no respawn (`none`); pressing Join after a loss obeys the same rules |
| Retail features in single player | U answers IFF, Show Target Info works (off by default), and the reply keys say "You lead this flight." when the player leads |
| Phase 2 defaults | As designed, to start: PvP revives with unlimited lives, no delay, 10 nm, missiles, scored by sides on total kills, kill limit 5, 10 minutes, sides locked; co-op has no revival and friendly fire on |
| Idle aircraft | The AI flies an aircraft whose player has been away 5 minutes by default (a menu, lost focus, a lost controller, a stall), reserved for the player; a King's setting that can be set to never (John, 2026-10-06: 10 seconds was too short for someone on autopilot who steps away; built in F2-O4) |
| A dedicated server's King | None by default; `king first-player` makes the first player King, and the mission can be locked |
| Realism in flight | Fixed for the flight: no in-flight Cheat menu changes, so every client's prediction stays exact |
| Phase 2 keys | Replies Alt+Shift+E (Engaging), Alt+Shift+W (Winchester), Alt+Shift+B (Bingo fuel), Alt+Shift+H (Need help); K the score board; U and Ctrl+T as retail; Enter flies again after a loss |
| Converted replays' effects | John, 2026-10-05: a replay converted from a capture shows smoke, contrails and gun rounds, regenerated as the live client does (a follow-up to the conversion; built, slice E2) |
| Whose plane a converted replay follows | John, 2026-10-05: first the conversion swapped plane numbers so the player's plane was plane 0, as the viewer expected, and recorded the real plane in the header; then, the same day, the viewer learned to follow any seat with the observer screen, which needed it anyway, and the swap ended (built in F2-O2: every plane keeps its id, `draw.player` names the player's) |
| A converted replay checked by ear and eye | John watches a converted replay of a real two-player flight, with sound, once the conversion is merged |
| Checkpoints and presentation | John, 2026-10-05: smoke, contrail and flare puffs are coded exactly in a checkpoint; revisited only if the measured size is over the budget |
| Explanations after a migration | John, 2026-10-05: accepted that the replay's and debug panels' explanations of calls and decisions begun before the checkpoint can be missing on the new host; nothing in play changes |
| Checkpoints are not saves | John, 2026-10-05: a checkpoint is read only by the same build, is never a save file, and gives no quick save |
| The order call's change in single player | John, 2026-10-05: approved the baseline of slice G3a: only the attack orders' recording stems change (an assignment's call with its geometry, "Attack bandits" for Attack on contact); no motion, kill or text changes |
| Bearings in calls | John, 2026-10-05: real-world brevity, three digits spoken one by one ("bearing zero one six", "two seven zero", north "three six zero") |
| The data link's cues in single player | John, 2026-10-05: approved slice G6's baseline (a sort warning's beep and HUD line when the player and a wingman lock one bandit) |
| A radar-less aircraft's link cues | John, 2026-10-05: it keeps the target window's and the HUD's link cues and the sort warning; only the radar scope's marks are absent, since it has no radar |
| The AI counts the player's lock | John, 2026-10-05: approved slice G2's baseline (a wingman choosing after the player locks a bandit takes another one; kills and results unchanged) |
| Content not everyone has | John, 2026-10-05: dimmed in the King's creator and Load Ordnance, and choosing it says who lacks it; every connected player counts, the unable and the watching included; the King can kick |
| Showing a player's Fighters Anthology build | John, 2026-10-05: in the lobby's hint for the selected player and one Messages line when it differs from the host's; no new mark in the Players list; imports made before stage L read it from the import report, with no re-import. **John, 2026-10-06: removed: the audit found no difference a player sees** (the hint says the system and the path; the Messages line stays for items a player lacks or has differently; the wire and the server's log still carry the build) |
| Warning before joining | John, 2026-10-05: the Internet Lobby does not warn in v1 that a game needs content the player lacks; the player joins and reads why in the lobby |
| Standby hosts | John, 2026-10-05: up to two; warm standbys (same system and processor type) step their own copy and take over at once, others are cold and catch up from a checkpoint; a cold standby's stream is paced to at most 1 Mbit/s |
| Checkpoint size | John, 2026-10-05: stage K measures the standby stream before any of the size levers is built |
| A host lost | John, 2026-10-05: detected after 1.5 s of silence; players read "Lost contact with the host. Moving the game to Hawk..." then "The game moved to Hawk."; other aircraft hold still 2 to 3 s while the player's own keeps flying |
| Rejoin tokens | John, 2026-10-05: 128-bit tokens from the standard library's randomly keyed hasher; no new dependency |
| The calculated host in the lobby | John, 2026-10-05: stays where it is while that machine passes the tests, moves only when it fails and another passes; the King can pin a host |
| Reservations | John, 2026-10-05: a dropped player's aircraft is reserved for its rejoin; End Mission releases it; an aircraft lost while its player was away brings the revival rules on return |
| Taking over hosting | John, 2026-10-05: a "Let my game take over hosting" switch in Options, on by default; dedicated servers never migrate (they issue rejoin tokens and hold reservations) |
| The data link in recordings | John, 2026-10-06: approved slice G9's baseline (single-player recordings gain the data link's events and nothing else; the viewer's Comms panel gains a Link chip and the summary a Data link section) |
| AI leads share and sort | John, 2026-10-06: approved slice G4's baseline (AI leads on loose control share and sort targets and AI wingmen locked on one bandit spread out; every probe with an AI-led wing moves, the player's own wing does not) |
| AI leads assign human wingmen | John, 2026-10-06: an AI lead's sort and share deal targets to its human wingmen too, with the cues and the call on that seat's channel; the human is free to ignore it (built in G11) |
| An away player's own plane | John, 2026-10-06: while the AI flies an away player's aircraft, the player watches it on the observer screen (built in F2-O3). No flight input takes it back: it is a flight sim and nobody should fly AFK, so the player comes back only from the observer menu, on purpose (John, 2026-10-06; built in F2-O4) |
| The away player's observer menu | John, 2026-10-06: in place of Stop Watching, the menu offers Take Back Flight (back into the player's own reserved aircraft), Spawn in Aircraft (only when no reserved aircraft is left, under the revival rules) and Leave Game (built in F2-O4) |
| Esc in the observer view | John, 2026-10-06: Esc opens the viewer's pause menu, whose first row is Stop Watching |
| Snapshot rates | John, 2026-10-06: snapshots 60 a second by default, the near band's rate, and the far band 4 times a second, raised from 30 and twice a second; he accepts about double the bandwidth. The setting keeps 10 to 60 a second so a host on a weak uplink can go down (built in D12; [measured](baselines/net-rates-2026-10-06.md)) |
| The King's snapshot rate | John, 2026-10-06 (Q56): a game a player hosts has a lobby setting for the snapshot rate, 60 (default), 30 or 20, so a host on a weak uplink need not host at 60. The King turns it in the lobby only, never in flight. A dedicated server keeps its file's `snapshot-rate`, which the lobby shows read-only (agent decision). When no machine passes the upload test, the King's warning says whether the best machine's figure would fit 30 or 20 (built in R1) |
| The cockpit voice after a loss | John, 2026-10-06 (Q60): the player's own crew voice (a two-seater's RIO or co-pilot, and any crew line spoken from the player's cockpit) falls silent once the player's aircraft is destroyed or the player ejects: no line after that tick, queued ones dropped. The debrief keeps its result (SUCCESS when the wingmen finish the job). Radio from other aircraft is unchanged (built in B5) |
| Two-seat ejection | John, 2026-10-06 (Q60): in a two-seater Shift+E twice ejects the RIO too: two seats and two chutes, the RIO first, a short interval apart, both drawn and recorded; in a networked two-seater the crew follows the same rules. *Agent decisions (B5):* 0.4 seconds apart; the same rule for the AI's ejections; the debrief counts the pilot alone; the second chute rides the pilot-entity stream under the aircraft's id with bit 31 set, so no protocol change (built in B5); a checkpoint keeps the second chute, so a standby and a host taking over mid-descent keep it (B6, no protocol change) |
| The relay's rate | John, 2026-10-06 (Q55): the master's `relay-rate` default goes from 64 to 128 KB/s each way per channel, since a relayed player's busiest second at 60 snapshots a second is 63 to 74 KB/s (D12). A master whose file sets `relay-rate` keeps its own value (built in R1; [the master's guide](MASTER-SERVER.md#the-configuration-file)) |
| Avoidance heading follows the aircraft that set it | John, 2026-10-06 (Q61): the AI's traffic avoidance rule of 2026-09-29 keeps its heading only while the aircraft that set it is still a conflict; a conflict with another aircraft sets a fresh heading, 30 degrees off the current one and away from the new aircraft. Two flightmates that turned right together for one head-on aircraft no longer hold each other on that heading until they collide (`ai-regress-visual-incoming-flap`). Single-player AI fights move (built in B4b; [the AI spec](spec/ai.md#traffic-avoidance)) |

### Made by John on 2026-10-09 for the lobby pass

Side colours, distances, AI respawn and the lobby overhaul
([the lobby](#the-lobby-since-the-lobby-pass), [AI respawn](#ai-respawn),
[revival distance and separation](#revival-distance-and-separation),
[lead hold](#lead-hold)). His request and his answers, as the lead relayed them:

| No. | Decision |
| --- | --- |
| D1 | AI respawn applies in both co-op and PvP. AI follow the same lives and respawn delay rules as humans (lives are counted per original aircraft, since an AI aircraft has no player) |
| D2 | Respawned AI aircraft come back at their flight's original spawn point, not near the current lead. This rule is for the AI alone: humans keep the revival distance (answer 1) |
| D3 | The Bluefor and Redfor buttons are toggles, like check boxes. With no side yet, a side's box is greyed when that side is full; otherwise checking it joins that side (takes a free slot there) and filters the slot list to it. A player on a side may uncheck their own side's box: that leaves the side and frees their slot, and they may then check the other side if it is not full |
| D4 | No separate "Game info" button at the top. The bottom row's Mission... button serves everyone: editable for the King as today, the same screen read only for every other player, so it is enabled for non-Kings |
| D5 | The "Rules:" line stays. Only the King line and the "Mission:" line are removed; the ready-up hint goes between the chat input and the buttons |
| D6 | A host Autobalance option. When on, the host assigns players to Bluefor or Redfor and players cannot change side; their side boxes are disabled and show the assigned side. *Agent decision:* it is built as the third value of the Lock sides setting, now called **Sides** (Free, Locked once flown, Balanced by the host), because balanced sides that a player could change in flight would be pointless |
| Leadership | A shot-down AI aircraft respawns and joins its old flight as a wingman of whoever leads it now. In an AI-only flight the lead keeps shifting down as fighters die. With humans in the flight, a human lead keeps the lead while dead or respawning until that player leaves the game; then it goes to the next human in the flight, and with none left the AI's succession goes on. John accepts that leaders can be out of the fight while they respawn or travel back |
| Answer 1 | The distance he meant is the **revival distance**: it gains 50, 75, 100 and 150 nm. The separation (the starting distance between the sides) also gains 75 nm. D2 is for the AI only |
| Answer 2 | AI respawn gets its own setting row, on by default and greyed when Revival is none |
| Answer 3 | AI wingmen re-form on a returning human lead, unless they are engaged in combat, when they finish the fight first |
| Answer 4 | Autobalance does not re-deal when a player leaves: the next joiner goes to the smaller side, and the King re-deals by turning Autobalance off and on |
| Answer 5 | The ready hint reads "Press Ready so the King can start the mission." for a player holding a slot, with the variants in [the ready hint](#the-ready-hint) |
| Answer 6 | Colours as recommended: taken slots are filled bars, royal blue (36, 81, 186) or bright red (210, 36, 40), with white names; open slots are light blue (134, 182, 223) or light red (206, 113, 121) text; wing labels read "Blue 1 #2" and "Red 1 #2" |
| A locked server | A dedicated server's `king-mission locked` reaches the clients (protocol 20), and the King's Mission... on such a server opens the read-only page, "This server's mission is fixed." (slice K1) |

*Agent decisions of the pass that John has not yet seen in play* (his PvP test
is the check): Autobalance's seating, tie and re-deal rules; the side boxes'
"3/5" counts, their wording and Tab and Space; the Players list tinted by side
and an unable player dimmed instead of red; the King's Mission... opening on
the lobby's mission rather than the King's last draft; the wheel's two rows a
notch and the red bar on Direct Connection's and the Internet Lobby's Messages
boxes; the flat track for an import that lacks the slider pieces; the lead
hold's order of succession, its two HUD lines and the stand-in naming the
owner's plane by radio label ("Red one") instead of the player's callsign;
the AI's lives per original aircraft; and AI wrecks retiring so respawns never
fill the 64 aircraft.

## Open questions

From the feature spec. Mission start, PvP scoring and collisions were settled by
John on 2026-09-28 (see [decisions](#decisions)).

- **Master server abuse limits:** rate limiting and fake listing protection.
  *Agent proposal:* a listing must first answer a challenge sent to its address;
  heartbeats and queries are rate-limited per address; listings expire after
  90 seconds without a heartbeat; the master never answers an unverified sender
  with more bytes than it received.
  *Designed 2026-10-05* on that proposal, with numbers
  ([master server](#master-server), [limits](formats/master-protocol.md#limits)),
  awaiting John's review.

Raised by the stage I and J design (2026-10-05), each with the agent
proposal the design is built on:

- **Where the master runs.** A new Linode 2 GB machine (the plan John chose
  for the relay on 2026-09-28) or the existing jroverton.com server; which
  region; the name; the ports. *Agent proposal:* its own Linode, in the region
  nearest most players, `master.jroverton.com`, UDP 26901 and 26902
  ([deploying](MASTER-SERVER.md#deploying-at-jrovertoncom)).
- **Telemetry's default and contents.** *Agent proposal:* on by default, with
  the switch and a one-time notice, sending only the list in
  [telemetry](#replay-and-telemetry).
- **A dedicated server listing itself.** *Agent proposal:* off unless its
  configuration says `broadcast on` (John chose the name and the default
  on 2026-10-05; see [decisions](#decisions)).
- **Port mapping by default.** The game changing the player's router
  settings while hosting. *Agent proposal:* on, with the switch, removed when
  hosting stops, and said in Messages.
- **The relay's monthly cap.** *Agent proposal:* the relay stops taking new
  pairs at 95 percent of 800 GB a month, so the plan is never exceeded.
- **The Internet Lounge's title.** Resolved by John's 2026-10-22 request:
  INTERNET LOUNGE, with origin-specific game lobby titles as defined in the
  [menu specification](spec/multiplayer-menus.md).

Raised by the stage L design (2026-10-05), each with the agent proposal the
design is built on ([compatibility](ARCHITECTURE.md#compatibility)):

- **Dimmed or hidden.** Should the King's creator hide an aircraft or theater
  that not every player can use, or show it dimmed? *Agent proposal:* dimmed,
  and choosing it says who lacks it, so the King knows why it is not offered.
- **Who counts.** Whose content limits the King's choices? *Agent proposal:*
  every connected player, including one marked unable or watching, since each
  builds the mission; a King who wants the full list can kick.
- **Showing the build.** *Agent proposal, answered by John 2026-10-05, then
  reversed 2026-10-06:* the build was to appear in the lobby's hint and in
  Messages. John removed it after the build audit found nothing a player sees
  (see the decisions table).
- **Before joining.** Should the Internet Lobby warn that a listed game's
  mission uses something the player lacks? *Agent proposal:* not in v1; the
  player joins, stays in the lobby, and reads why there.

Raised while planning (2026-09-28):

- **Menu, focus loss and controller loss in flight.** With no pause, what does
  the aircraft do? *Agent proposal:* the pilot's controls return to neutral
  while the menu is open, and after 10 seconds without input the AI flies the
  aircraft until the player touches the controls again. John settled the first
  half on 2026-09-30 (neutral controls, [decisions](#decisions)) and the
  second on 2026-10-05: the King's setting `idle-ai`, with `never` available,
  built in F2-A. *Changed 2026-10-06 (F2-O4):* 5 minutes by default, and the
  player comes back from the Escape menu, not by touching the controls
  ([architecture](ARCHITECTURE.md#the-ai-flies-an-idle-players-aircraft)).
- **Dedicated server King.** *Agent proposal:* the first human to join a
  dedicated server becomes King, unless its config file fixes the mission and
  locks the settings. Stage D's server always takes its mission and settings
  from its files ([server guide](DEDICATED-SERVER.md)). The stage F design and
  EF4 built the server's lobby with no King: its mission comes from its file
  and its `start` setting starts it ([rules](DEDICATED-SERVER.md#the-lobby)).
  *Designed in phase 2, awaiting John:* no King by default, as built; a
  server's configuration may give the crown to the first player
  (`king first-player`) and may lock its mission (`king-mission locked`)
  ([architecture](ARCHITECTURE.md#the-king-the-crown-and-the-house)).
  John took it on 2026-10-05 ([decisions](#decisions)); *built (F2-1)*: a
  crowned server that has been empty for its empty timeout goes back to its
  file's mission and settings ([server guide](DEDICATED-SERVER.md#the-kings-settings-and-a-king)).
- **No eligible host.** If every peer can connect only through the relay, no one
  can be the calculated host. *Agent proposal:* the King sees a plain warning
  and can pin a relayed host anyway, or use a dedicated server. *Designed in
  stage K, changed:* the King reads "No other game can take over hosting: if
  you leave, the game ends.", and a relayed player cannot be pinned either,
  since every other player would then go through the relay
  ([host selection](ARCHITECTURE.md#host-selection)).

Raised by the stage K design (2026-10-05), each with the agent proposal the
design is built on ([questions](ARCHITECTURE.md#questions-for-john-stage-k)):

- **Standbys.** *Agent proposal:* up to two, the second a fallback.
- **Warm standbys.** *Agent proposal:* a standby on the host's own system and
  processor type simulates alongside the host (15 to 40 percent of one core)
  and takes over at once; any other is cold and catches up.
- **Checkpoint cadence and pacing.** *Agent proposal:* a cold standby gets a
  checkpoint every 10 seconds, paced at no more than 1 Mbit/s; a warm one when
  appointed or after a failed 5-second check.
- **The checkpoint size levers first.** *Agent proposal:* not now; measured in
  the stage's acceptance.
- **What a player sees during a migration.** *Agent proposal:* the two HUD
  lines above; the own aircraft flies on, the others hold still for 2 to 3
  seconds.
- **How long before the game moves.** *Agent proposal:* 1.5 seconds without
  a word from the host.
- **Randomness for tokens** (the plan's `getrandom` question). *Agent
  proposal:* no new dependency; 128-bit tokens from the standard library's
  randomly keyed hasher, which the operating system's random source keys.
- **The calculated host in the lobby.** *Agent proposal:* the game stays where
  it is while that machine can carry its players, and moves only when it
  cannot and another can.
- **Reservations.** *Agent proposal:* a drop reserves the aircraft, ending
  one's own flight does not; an aircraft lost while its player is away brings
  the revival rules on return.
- **"Let my game take over hosting"** in Options. *Agent proposal:* on by
  default.
- **Dedicated servers never migrate.** *Agent proposal:* as stated.
- **Lobby screens.** Retail's connection screens, Players dialog and message
  window exist in the retail media. Research whether their art can be reused, as
  the project rules prefer.
- **Reply keys.** Which keys the human wingmen's replies and requests use. They
  are new actions, so the [controls list](CONTROLS.md) changes with them.
  *Proposed in phase 2, awaiting John:* Alt+Shift+E Engaging, Alt+Shift+W
  Winchester, Alt+Shift+B Bingo fuel, Alt+Shift+H Need help
  ([keys](CONTROLS.md#multiplayer-phase-2)). John answered
  on 2026-10-05 (all as recommended); the keys exist since slice F2-C.

### AI wingmen's starting load

Each human flight's AI members start with that human's accepted ordnance,
including empty stations, tanks and fuel. Other human slots keep their own
loads; explicit mission-file AI loads stay intact. With multiple humans in one
wing, the lowest member slot supplies its AI load. Revival retains the chosen
stores under the revival-weapons rule. Unedited revivals fill internal fuel; an
explicit in-flight selection keeps its chosen fuel. See the
[loadout contract](spec/multiplayer.md#human-flight-loadouts).

## AI aircraft and Player Choice

John requested these options on 2026-10-16. The Revival settings page includes
**AI aircraft**, also available in the hosting creator. Off disables AI aircraft,
AI respawn and idle handoff. The lobby waits for the configurable minimum of
ready slot holders. The King can Force Start once that minimum is ready, even
with empty slots. See [minimum players](spec/multiplayer-menus.md#minimum-players). Co-op is allowed with an empty
enemy air force; ground defenses still operate. Live departures remove the plane
without kill or loss credit; late joins use revive placement.

The creator offers Fixed/Player choice per wing beside its aircraft selector.
For Choice wings that selector is the mandatory AI default. Taking a Choice slot
opens the existing aircraft dialog then Load Ordnance. Loadout reopens the flow.
Slots name the selected aircraft or AI default. Accepted choices pass the shared
content check and travel with the flight build.

A late join to a Choice slot removes its AI aircraft cleanly and creates the
accepted player aircraft at revival placement. Departure returns the lineage to
its wing AI default under the existing AI respawn delay and lives rules, at its
original spawn. These removals award no kill or loss. A drop (a crash, a lost
connection) is not a departure: as in a fixed wing, the AI flies the chosen
aircraft, kept for the player, who takes it back with the rejoin token, also
after a host migration; the lineage returns to the default only when that
reservation ends (the King's Release).

With `respawn revive`, the flight continues beneath a menu while waiting, using the in-flight
Escape menu widget. Loadout lists stations in Load Ordnance display order, then
internal fuel (a click raises it a tenth and wraps from full to a tenth; a
right click lowers it a tenth and wraps from a tenth to full), Standard load and
Done. Select a station to open its compatible
stores and quantities, grouped and ordered like the pre-flight Load Ordnance
catalog. Player Choice slots have one complete Aircraft list in creator/lobby
order. Both lists have a red scroll bar and a case-insensitive search by name
or designation: `120`, `16` and `hornet` are examples. Click Search or press Tab
to type; Tab switches search/results focus. Up/Down or the controller D-pad
moves through results, Enter/A or a click selects. Home/End and Page Up/Down,
the wheel, track clicks and dragging scroll the list. Escape/B clears search
first, then closes a station sub-menu, then discards unsaved edits. M opens
Mission outside search focus; the Mission... button always remains available.
See the [selector specification](spec/quick-mission-menu.md#multiplayer-player-choice)
for counts and presentation decisions.
Done saves the aircraft and loadout for the next Enter-to-revive request.
Close the page and press Escape to open the flight menu: **Loadout...** stays
available for the entire wait, and **Aircraft...** appears for Player Choice
slots. Reopening restores the last saved aircraft, stores and fuel. Cancel or
Escape discards only unsaved edits. **Mission...** still opens the mission brief.
The pages have no timeout, including after the revival delay ends.

With menus closed, press **Enter** or **controller A** after the countdown to
fly again. Closing a page or reaching zero never respawns you. Enter before
zero does nothing and does not queue a revival. The countdown and ready prompt
show lives remaining. Waiting uses no life and has no time limit while the
mission continues. The existing idle-AI rule excludes lost aircraft, so your
dead slot stays yours; the usual away timer applies again once you are flying.
The `ai-slot` rule continues to take an existing aircraft with its existing
revival weapon cut. Closing or reviving releases menu input. Aircraft and loadout are validated together by the host, including
shared content, normal/cheat loading, weight and revival-weapons restrictions.
Explicit choices retain their selected internal fuel; an unedited legacy revive
still starts with full fuel. The host distributes the accepted spawn and every
peer prepares the same simulation resources, including checkpoint restores.
See the [requirements and decisions](spec/quick-mission-menu.md#multiplayer-player-choice).

## Feedback to a dedicated server

An operator can enable **Feedback...** under the first tab of the in-flight
Escape menu. It appears only on dedicated servers with `feedback on`, which
also requires server recording. It is hidden in player-hosted games and
single player. The form keeps multiplayer running, and marks the tick at
which it opened. Submit sends up to 1,000 Unicode characters to the host.
The host supplies your callsign, seat and aircraft, logs the feedback and
keeps it in its recording; your local recording also gets the accepted flag.

The form uses the controls editor's frame and buttons. Tab selects Submit or
Cancel, Escape cancels, and the controller can type through an on-screen
keyboard. See [replay flags and export](REPLAYS.md#player-feedback-flags) and
[server settings](DEDICATED-SERVER.md#recording-and-feedback). Protocol 67
carries the operator's flag and reliable feedback messages.

The unreleased protocol-75 ordnance readout appends the optional contiguous-region minimum to the estimates field. HUD clients use this minimum with the published maximum for the range scale. The lead owns the eventual protocol bump.
