# Network wire protocol

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Design of 2026-09-30 for stage D of the [multiplayer plan](../multiplayer-plan.md#stages),
reviewed by John the same day. The transport, from [Overview](#overview) to
[Reliable messages](#reliable-messages), is built in `tore-net` (slice D2; [what
the build settled](#what-the-transport-settled)); the game's sections and
messages, from [Inputs](#inputs) on, are built in `tore-session`'s `wire`
module (slice D6; [what the build settled](#what-the-games-sections-settled)),
the cockpit readout's coding included. This is T.O.R.E's own
protocol. Fighters Anthology's wire format is unknown
([retail spec](../spec/multiplayer.md)) and nothing here tries to match it.
Every choice below is an agent decision unless it is credited to John. How the
host and the client use these messages is in the
[architecture guide](../ARCHITECTURE.md#network-sessions); the rates, delays and
limits a player notices are in the [netcode numbers](../MULTIPLAYER.md#netcode-numbers).

## Contents

- [Overview](#overview)
- [Packets](#packets)
- [Connecting](#connecting)
- [Keepalive](#keepalive)
- [Discovery](#discovery)
- [Through the master (stages I and J)](#through-the-master-stages-i-and-j)
- [Acknowledgements and round trip](#acknowledgements-and-round-trip)
- [Reliable messages](#reliable-messages)
- [What the transport settled](#what-the-transport-settled)
- [Inputs](#inputs)
- [Snapshots](#snapshots)
- [Events](#events)
- [Data link (stage G)](#data-link-stage-g)
- [Quantization](#quantization)
- [What the game's sections settled](#what-the-games-sections-settled)
- [Phase 2: the King's settings, revival, scores and observers](#phase-2-the-kings-settings-revival-scores-and-observers) (designed)
- [Compatibility (stage L)](#compatibility-stage-l) (built under protocol 10)
- [Host migration and rejoin (stage K)](#host-migration-and-rejoin-stage-k) (built under protocol 13)
- [The surface world (protocol 22)](#the-surface-world-protocol-22) (built under protocol 22)
- [The air-to-ground round (protocol 23)](#the-air-to-ground-round-protocol-23) (built under protocol 23)
- [The redrawn airports (protocol 24)](#the-redrawn-airports-protocol-24) (built under protocol 24)
- [The city fill (protocol 25)](#the-city-fill-protocol-25) (built under protocol 25)
- [Protocol 26: explosions sized by weapon](#protocol-26-explosions-sized-by-weapon) (built under protocol 26)
- [Protocol 27 (stores)](#protocol-27-stores) (built under protocol 27)
- [Protocol 28 (host loading)](#protocol-28-host-loading) (built under protocol 28)
- [Protocol 29 (the sensor console)](#protocol-29-the-sensor-console) (built under protocol 29)
- [Protocol 30 (the console's search and keys)](#protocol-30-the-consoles-search-and-keys) (built under protocol 30)
- [Protocol 31 (the gun lock and the centre's pause)](#protocol-31-the-gun-lock-and-the-centres-pause) (built under protocol 31)
- [Limits](#limits)
- [Captures](#captures)
- [Versions](#versions)
- [Security](#security)

## Overview

- **UDP, one port.** A server listens on one UDP port, 26900 by default (a
  setting; not checked against the IANA registry). IPv4 and IPv6 both work
  when the address is given; finding each other across the internet is
  stages I and J ([through the master](#through-the-master-stages-i-and-j)).
- **Small packets.** Every datagram carries at most 1,200 bytes, under the
  smallest common path limits, so nothing is ever fragmented by the network.
- **Every packet stands alone.** A snapshot is coded against states the
  client has already acknowledged, never against a packet that might be lost,
  so a client decodes each packet it receives whatever happened to the others.
- **Bits, little end first.** Fields are packed with `tore-codec`: bits are
  written least significant first into bytes in order. Section bodies start on
  a byte boundary.
- **Four channels in one packet.** Unreliable state (snapshots and inputs),
  events repeated until acknowledged, reliable ordered messages, and the
  header's acknowledgements, which every packet carries.
- **Time is ticks.** Every time on the wire is a host tick (1/120 s) as an
  unsigned 32-bit number, which wraps after 414 days.

## Packets

Every packet starts with a byte-aligned header:

| Field | Size | Meaning |
| --- | --- | --- |
| Checksum | 32 bits | CRC-32 (IEEE) over the protocol id followed by every byte after this field, least significant byte first |
| Kind | 8 bits | One of the kinds below |

The protocol id is not sent. It is the 8 bytes `TORE-NET` followed by the
protocol version as 16 bits, so a packet from another program or another
version fails its checksum and is dropped silently. The kinds a
different version must still read, Connect request, Refuse and the two
discovery kinds, use the fixed id `TORE-HELLO` instead, which carries no
version.

| Kind | Name | Direction | Checksum id |
| --- | --- | --- | --- |
| 1 | Connect request | client to host | `TORE-HELLO` |
| 2 | Challenge | host to client | versioned |
| 3 | Challenge answer | client to host | versioned |
| 4 | Accepted | host to client | versioned |
| 5 | Refuse | host to client | `TORE-HELLO` |
| 6 | Payload | both | versioned |
| 7 | Disconnect | both | versioned |
| 8 | Discover query | anyone to a host | `TORE-HELLO` |
| 9 | Discover answer | host to the asker | `TORE-HELLO` |
| 10 | [Keepalive](#keepalive) (protocol 5; host to client since 28) | both | versioned |
| 11 | [Punch](#punch) (protocol 9) | host to client | versioned |
| 12 | [Reach](#reach-and-reach-answer) (stage K, protocol 13) | game to game | versioned |
| 13 | [Reach answer](#reach-and-reach-answer) (stage K, protocol 13) | game to game | versioned |
| 14 | [Reservation query](#reservation-preflight-protocol-71) | lounge to host | versioned |
| 15 | [Reservation answer](#reservation-preflight-protocol-71) | host to lounge | versioned |

A **Payload** packet, the only kind once connected, continues:

| Field | Size | Meaning |
| --- | --- | --- |
| Connection id | 32 bits | The number the host gave in Accepted; a packet with another number is dropped |
| Sequence | 16 bits | This packet's number in its direction, wrapping |
| Ack | 16 bits | The newest sequence received from the other side |
| Ack bits | 32 bits | Bit i set: sequence `ack - 1 - i` was received |
| Ack delay | 16 bits | Time from receiving `ack` to sending this packet, in units of 16 microseconds (at most 65,534 units, about 1 s); 65,535 means nothing has been received yet, and the ack and ack bits mean nothing (agent decision, D2) |
| Sections | rest | Each: kind 8 bits, length 16 bits (bytes), body |

The payload header is 19 bytes with the checksum and kind. Section kinds:

| Section | Kind | Direction |
| --- | --- | --- |
| [Messages](#reliable-messages) | 1 | both |
| [Inputs](#inputs) | 2 | client to host |
| [Snapshot](#snapshots) | 3 | host to client |
| [Events](#events) | 4 | host to client |
| [Own state](#the-own-aircraft) | 5 | host to client |
| [Filler](#disconnect-reason-8-and-the-filler-section) (stage K, protocol 13) | 6 | client to host |

An empty Payload is a keepalive while a side's loop runs; a joined game
whose loop is stalled sends the [Keepalive](#keepalive) packet instead. A section of an unknown kind, a second
Messages section, a length that runs past the packet, an acknowledgement of a
sequence never sent, or a body that fails its own checks drops the whole
packet, unacknowledged; such packets are counted, and 50 of them within 5 seconds from one
connection end it.

## Connecting

The handshake keeps the host from holding any state, or sending more bytes
than it received, before a client proves it can receive at its address.

```mermaid
sequenceDiagram
  participant C as Client
  participant H as Host
  C->>H: Connect request, padded to 1,000 bytes
  H->>C: Challenge with a cookie, 21 bytes
  C->>H: Challenge answer, cookie echoed, padded
  alt accepted
    H->>C: Accepted, connection id and rates
  else refused
    H->>C: Refuse, reason code and text
  end
```

| Packet | Fields |
| --- | --- |
| Connect request | protocol version (16), client nonce (64), compatibility code (32, since 71), game version (string), game commit (string), zero padding to 1,000 bytes. The version and the nonce come first and never move, so a host of any version can refuse with the nonce |
| Challenge | client nonce (64), cookie (64); 21 bytes |
| Challenge answer | client nonce (64), cookie (64), callsign (string, 1 to 15 printable ASCII characters), password (string, may be empty), game version and game commit again (the host kept nothing from the request), compatibility code (32, since 71), platform (8, protocol 7), [path](#the-path-in-the-challenge-answer) (8, protocol 9), zero padding to 1,000 bytes |
| Accepted | client nonce (64), connection id (32, random, never 0), session id (64), ticks per second (8, always 120), ticks per snapshot (8, 2 by default since slice D12, 4 before; the rate in force on the day of the join, see below), host tick now (32); 31 bytes |
| Refuse | client nonce (64), reason (8), text (string, up to 200 bytes) |
| Disconnect | connection id (32), reason (8); sent three times at once; 10 bytes |

**The ticks per snapshot set both bands' intervals.** A near entity is
sent every snapshot. A far entity is sent every *F* snapshots, where *F* is
the snapshot rate (120 over the ticks per snapshot) divided by 4 and rounded
up: at most 4 times a second (John, 2026-10-06; twice a second before slice
D12). Both ends work *F* out from this byte, so no other field carries it:

| Snapshot rate | Ticks per snapshot | *F*, snapshots | Far interval, ticks | Far updates a second |
| --- | --- | --- | --- | --- |
| 60 (default) | 2 | 15 | 30 | 4 |
| 40 | 3 | 10 | 30 | 4 |
| 30 | 4 | 8 | 32 | 3.75 |
| 24 | 5 | 6 | 30 | 4 |
| 20 | 6 | 5 | 30 | 4 |
| 15 | 8 | 4 | 32 | 3.75 |
| 12 | 10 | 3 | 30 | 4 |
| 10 | 12 | 3 | 36 | 3.33 |

**The King's rate changes it for a flight (protocol 17, slice R1).** In a game
a player hosts the King may turn the snapshot rate to 60, 30 or 20 in the
lobby ([setting 22](#settings-by-number)), so the byte Accepted carried when a
player joined may be old by the time a mission flies. Each flight starts from
the setting in force: the host builds its connections for it, and a player's
game takes 120 over the lobby state's setting 22 when the flight starts. The
host sends every player the lobby state before the first seat of a flight, so
the setting is there first (the lobby state is otherwise sent at most every
250 ms). A setting changes only in the lobby, so what the lobby says as a
flight starts is what the host runs; a player who joins a flight in progress
gets the rate in Accepted. A dedicated server's rate is its file's (any of 10,
12, 15, 20, 24, 30, 40 or 60), carried in setting 22 as given; the King's
change of it is refused.

The client draws a far entity its interval further in the past
([guide](../MULTIPLAYER.md#netcode-numbers)). It takes an entity for far
when the gap between two of its updates is at least 4 snapshots, or *F*
when that is fewer, and never fewer than half of *F* (8 snapshots at 60 a
second), and for near when the gap is 2 snapshots or fewer (*agent
decision*, D12). The rule changed no byte, so the protocol version did not
change (the lead, 2026-10-06: both ends are always the same build).

A request or answer that is not exactly 1,000 bytes, or whose padding is not
zero, is dropped. Accepted and Refuse carry the client's nonce, and the client
takes neither without it, so a stranger who cannot see the traffic cannot
refuse or misdirect a join; the game version and commit are repeated in the
answer because the host's accept decision needs them and it keeps no state
between the two (agent decisions, D2, which also moved the nonce ahead of the
strings).

The **platform** byte names the operating system the player's game runs on:
0 unknown (a build for another system), 1 Windows, 2 macOS, 3 Linux. A game
sends the system it was built for. The host keeps it with the player and
sends it in the [lobby's player list](#messages-as-built), so a lobby can
show it beside the callsign (John's request, 2026-10-05). Any other code
makes the answer malformed, and it is dropped. The host only shows the
platform; nothing in the session depends on it. Its byte, its codes and the
lobby's 3 bits are agent decisions. With a callsign of `c` bytes, a password
of `p`, a game version of `v` and a commit of `m`, the byte is at offset
`29 + c + p + v + m` in protocol 71 (checksum 4, kind 1, nonce 8, cookie 8,
four strings' length bytes and compatibility code 4). The path and token
fields follow it, then zero padding.

The **cookie** is a keyed hash of the client's address and port, its nonce
and the current 10-second time slot, with a key drawn at host start from the
standard library's randomly seeded hasher. The host accepts the current and
the previous slot. It keeps no record of a client until a Challenge answer
with a good cookie arrives. A repeated Challenge answer from the same address
and nonce gets the same Accepted again, never a second connection. A good
answer with a new nonce from an address that already has a connection replaces
it: the game restarted on the same port (agent decision, D2). The
connection id is drawn the same way as the cookie's key. A callsign already in
use gets a suffix (`Viper_2`), shortened first if the whole would pass 15
characters.

A client resends its current step every 250 ms and gives up after 10 seconds
("no answer from the server"). The host answers at most 20 connect requests
and answers a second from one IP address and 200 in all, counted per whole
second of its clock.

**Refusal reasons**, each with a plain-language text the client shows:

| Code | Reason |
| --- | --- |
| 1 | Different protocol version (the text names both) |
| 2 | Different game build (see [versions](#versions)) |
| 3 | Server full |
| 4 | Wrong password |
| 5 | The server is shutting down or restarting the mission |
| 6 | Kicked |

A client whose own retail stall-speed switch is on refuses to connect before
sending anything, and a server refuses to start with it, so that switch never
needs a code.

The content check happens after the mission loads, as a reliable message
(below), because only then can the client compare.

**Disconnect reasons** (the codes are an agent decision, D2): 1 the player
left, 2 timeout (5 seconds without a valid packet, a
[Keepalive](#keepalive) included; a host may exempt a
connection, as a hosting game exempts its own player's over the in-process
link, EF4), 3 too many bad packets,
4 protocol error (a message ahead of its window, or fragments that do not fit
together), 5 content mismatch, 6 server stopping, 7 kicked, and (stage K,
protocol 13) 8 [moved to the new host](#disconnect-reason-8-and-the-filler-section).

## Keepalive

*Built (EF-K), protocol 5; agent decisions unless credited.* A joined game's
loop can stop for longer than the 5-second timeout without the game being
gone: a window dragged on Windows, a long frame, a screenshot on a loaded
machine ([what stalls it, by platform](../ARCHITECTURE.md#a-stalled-game-stays-connected-ef-k)).
While it is stopped, a small thread of the game speaks for it.

| Field | Size | Meaning |
| --- | --- | --- |
| Checksum | 32 bits | As every packet, under the versioned id |
| Kind | 8 bits | 10 |
| Connection id | 32 bits | The number the host gave in Accepted |
| Flags | 8 bits | Since protocol 28: bit 0 set, the host is loading a mission ([host loading](#protocol-28-host-loading)); the other bits 0. A player's keepalive sends 0 |

Ten bytes since protocol 28 (nine before), smaller than the empty Payload
(19 bytes) it stands in for.

- **Who sends it.** Only a game joined over UDP, from a thread that holds a
  clone of the game's own socket, so it comes from the connection's address.
  The thread sends it once the game's loop has not taken a turn for 1
  second, then once a second, and never once the loop has not turned for 60
  seconds (the bound): a game that is truly hung still times out, 5 seconds
  after its last keepalive. The thread only sends this one packet; it never
  reads the socket or touches the client's state, and it stops when the
  connection closes or the session ends. The hosting game's own connection
  has none: it is exempt from the timeout (EF4). `tore-bot` runs the same
  thread for each bot joined over UDP or through the Internet Lobby (slice
  NET, 2026-10-12).
- **A long mission build.** *Slice NET, agent decision; no wire change.* A
  client builds the mission's world in the turn that reads the last of the
  host's Mission message (or a flight's loadouts), which on a slow machine
  takes longer than the 5-second timeout. The keepalive covers the build on
  the host's side, within the same 60-second bound. On the client's side the turn stops reading after that
  datagram: what the host sent during the build is read on the next turn,
  at that turn's time, so the client does not judge the host silent for the
  length of the build ([details](../ARCHITECTURE.md#a-long-mission-build-slice-net)).
- **What the host does.** A Keepalive from the connection's own address with
  its own id counts as hearing from the connection: no acknowledgement, no
  round trip, no rate statistic, no answer. The first since the game's last
  Payload tells the host's session the game has stalled, which flies the
  seat neutral and logs it (the
  [stall rule](../ARCHITECTURE.md#a-stalled-game-stays-connected-ef-k)). One from any
  other address is counted as from an unknown address, one with another id as
  stale, one of another version fails its checksum, and none of them keeps
  anything alive. A host before protocol 5 drops the kind as invalid, but
  versions must match to join anyway.
- **A client** takes one only from its host (protocol 28): from the host's
  address with its connection's id it counts as hearing from the host, and
  marked loading it says the host is building a mission until the host's
  next Payload ([host loading](#protocol-28-host-loading)). Any other is
  counted as from an unknown address, stale or malformed, and keeps nothing
  alive. A joined game's peers router passes it to the client (it dropped
  the kind before protocol 28).

## Reservation preflight (protocol 71)

These private, connectionless packets use the versioned `TORE-NET` checksum.
Discovery kinds 8 and 9 and the master protocol remain separate; the combined
protocol-71 discovery layout also carries the compatibility code described
below. No player is admitted and no slot is occupied by a query. Queries are unicast only, and
share discovery's per-IP and global rate limit. Replies never exceed their
37-byte queries. A token is never returned or written to a client-facing log.

After the standard 5-byte checksum/kind header, little-endian fields are:

| Kind | Fields | Total bytes |
| --- | --- | --- |
| 14 | nonce u64, session id u64, token u128 (low u64 first) | 37 |
| 15, unavailable | nonce u64, requested session id u64, presence u8 = 0 | 22 |
| 15, held | nonce u64, requested session id u64, presence u8 = 1, plane u32, enemy u8 (0 or 1), wing u8 (0 to 2), member u8 (zero based), token seconds u32 (0 to 86400), return seconds u32 (0 to token seconds) | 37 |

Other presence/side values, out-of-range wing/time, trailing bytes and short
packets are rejected. Invalid tokens and foreign sessions return unavailable.
A held reply requires a valid token and a living aircraft reserved for it, or
its still-present connection's reclaimable aircraft before the drop timeout.
It never promotes a remembered lobby slot to a reservation. Token seconds is remaining identity validity. Return seconds is the minimum
of that and any current mission time limit or active empty-mission timeout.
It is an upper bound, not a promise of aircraft survival or unchanged settings.
The caller checks source, session, nonce and freshness; the lounge's 2-second
polling and 3-second freshness limit are in the
[menu specification](../spec/multiplayer-menus.md#callsign-when-returning-to-a-game).

## Discovery

*Built (EF5).* Any host answers "who is hosting here?", whether or not the
asker can join it: the game's Direct Connection screen asks the local network
this way and lists what answers. The two packets are transport packets, not
the session's reliable messages. They sit under the fixed `TORE-HELLO` id,
so a host of any later build still reads the query and answers, and a game
of another build still reads the answer and lists the host marked as another
version. They never change under this protocol or any later one: a different
layout is a different kind. They therefore have a golden file of their own,
`crates/tore-net/discover-golden.txt`, outside the protocol version's
(agent decision); the test `discover_golden` compares it. A host of a build
from before slice EF5 drops a query silently, as it drops any kind it has no
name for (an invalid packet, counted and never answered).

```mermaid
sequenceDiagram
  participant A as Asker
  participant H as Host
  A->>H: Discover query, 1,000 bytes, to the host's game port
  H->>A: Discover answer, never longer than the query
```

| Packet | Fields |
| --- | --- |
| Discover query | asker's protocol version (16), asker's nonce (64), zero padding to exactly 1,000 bytes. The padding must be zero when the asker's version is the host's; another version may put fields in it, which this build ignores |
| Discover answer | nonce (64, the query's), protocol version (16, the host's), flags (8), players (8), capacity (8), session id (64), game version and game commit (strings), compatibility code (32, since 71), game name, mission summary, King's callsign (strings), count (8), then that many callsigns (strings) |

The answer's flags, from the least significant bit: password needed, full (no
place for another player), the callsign list is truncated, then two bits of
phase (0 lobby, 1 flying, 2 closed: the mission has ended or the host is
stopping, and joins are refused), then three zero bits. The summary is
`MissionSpec::summary()`, one line (theater, weather, start, the two sides);
the King is the callsign of the player who may change the mission, empty on a
dedicated server; the callsigns are the connected players in the order they
joined; the session id names one game across the addresses it answers from.
Texts are cut to fit: build texts to 64 bytes, the name to 64, the summary to
200, the King to 15 (the callsign limit).

**The size rule.** No answer is ever longer than its query, whatever the
lobby holds: a query is exactly 1,000 bytes and the host fits its answer to
that. The fixed part of the answer is at most about 440 bytes with every text
at its limit, and the callsigns follow while they fit (the largest lobby,
30 callsigns of 15 characters, takes 480 bytes more, which fits); a list that
would not fit is cut from the end and the answer says so in the truncated flag.
An answer that still does not fit is not sent. A spoofed query therefore
gains nothing by size, as with the handshake's Challenge.

**Who answers.** The host's game port answers, in every phase, from the
lobby's state: a player-hosted game and a dedicated server alike, in the lobby,
flying and after the mission, until the host stops. The host's transport reads a
query, rate-limits it by source address (20 a second per address and 200 in
all, the connect limits but counted apart so a flood of queries cannot take the
joins' allowance, an agent decision) and hands it to the session, which fills
in the answer from its lobby state and queues it for the asker.

**Where it is asked.** A game looks for others by sending a query every two
seconds to the limited broadcast address (255.255.255.255) on the game port, to
this machine's own network address and to its loopback address, and listens
for answers on the same socket, which is bound to the game port when that is
free (so one firewall rule covers everything). Discovery is IPv4 only; IPv6 discovery, port
mapping and the internet are stage J. A host on an IPv6 socket that also takes
IPv4 receives IPv4 broadcast on Linux and macOS (checked on this machine with
a query to the loopback network's broadcast address, which a firewall leaves
alone, and on the Linux and macos-14 CI runners with one to 255.255.255.255);
Windows, whose IPv6 sockets are IPv6 only, gets an IPv4 socket beside it from
the server's socket code, and that socket takes the broadcast (checked on the
Windows CI runner, EF-X;
[each system's sockets](../ARCHITECTURE.md#the-game-port-on-each-system-ef-x)). A machine's firewall may drop incoming broadcast on its real network
interfaces: on the development machine (ufw active) a broadcast to its own
interface never reached its own sockets, and a Windows firewall asks on first
run
([the dedicated server's note](../DEDICATED-SERVER.md#discovery-and-the-firewall)).
Join by address always works without it.

## Through the master (stages I and J)

*Design of 2026-10-05 for stages I and J; agent proposals awaiting John's
review. Built (J2, 2026-10-05, protocol 9):* the Punch, joining from several
addresses at once and the path byte. *Built (J3, 2026-10-05):* the relayed
addresses, with no change to the game's protocol.
What the game's own transport needs so that players can find and reach each
other across the internet. What is said to the master itself
is its own protocol, [master-protocol.md](master-protocol.md); how the pieces
fit together is in the
[architecture guide](../ARCHITECTURE.md#master-server-and-connectivity).

**Master packets never reach the transport.** A host's game port, and the
socket a player joins from, also talk to the master. The game routes every
datagram from the master's addresses to its master code before the
transport reads anything, and sends the master code's datagrams on the same
socket. Were one to reach the transport it would fail the checksum (its id
is `TORE-MASTER`) and be counted as invalid, nothing more.

### Punch

| Field | Size | Meaning |
| --- | --- | --- |
| Checksum | 32 bits | As every packet, under the versioned id |
| Kind | 8 bits | 11 |
| Introduction id | 64 bits | The id the master gave the introduction |

Thirteen bytes, sent by a host to each address of a player the master
introduced, five times 200 ms apart. Its job is to leave the host's router:
going out, it opens the router's mapping for the player's address, so the
player's Connect requests get in. A player's game that is connecting with
that introduction id and receives a Punch from an address it was not told
about adds that address to the ones it tries (the host's router chose
another outside port for the player than for the master). Any other Punch is
counted as unexpected. It is under the versioned id: the master introduces a
player only to a host of the same build.

*As built (J2, agent decisions):* the host's rendezvous sends the punches
itself, on the game port beside its master datagrams: it knows the game's
protocol version from the build it lists, so neither host loop changes and
there is no `Server::punch`. A Meet repeated for an introduction already met
is acknowledged again and never punched again, so an address gets at most
five punches per introduction. Port 0, unspecified and relayed addresses are
never punched. A joining client that receives a Punch with its introduction
id from an address it already tries, or after its race has chosen, ignores
it (the host's five punches outlast most races); one with another id, or to
a join with no introduction, is counted as unexpected; a host counts any
Punch it receives as unexpected.

### Joining from several addresses at once

A player's game told several addresses for one host (its address as the
master saw it, a port its router mapped, its IPv6 address, its local network
address) tries them all at once. `Client::connect` takes one address; stage J
adds `Client::connect_any`, which sends the same Connect request, same nonce,
to every address every 250 ms. The first Challenge or Refuse that carries
the nonce chooses the address, and from then on the client is the ordinary
client of that one address; datagrams from the others are counted as from an
unknown address. The host is unchanged: each request gets a stateless
Challenge, and only the Challenge answer the client sends to the chosen
address starts a connection. Four addresses cost at most 16 KB/s of
requests. The client still gives up after 10 seconds; the game asks the
master for the relay after 3 seconds with no answer
([master protocol](master-protocol.md#relay)).

*As built (J2):* `Client::connect_any(config, targets, introduction, now)`
takes `Target`s, each an address and the [path](#the-path-in-the-challenge-answer)
a join there takes; `Client::connect` is the race of one typed address. Agent
decisions: a race tries at most 12 addresses, the master's (at most 8) and
up to four learned from punches; an address given twice counts once; an
IPv4 sender that a dual-stack socket reports as its IPv4-mapped IPv6 address
is the same sender, here and once connected. Before the race chooses, a
datagram from an address it does not try is counted as from an unknown
address, as after.

### Relayed addresses

Through the relay, each end's transport sees the other at a reserved address
that stands for the relay channel, as the in-process link's peer is
`LINK_ADDRESS`, `[100::]:0` ([the link](../ARCHITECTURE.md#the-host-inside-the-game-stage-e)):
`[100::1:HHHH:LLLL]:0`, where `HHHH:LLLL` is the 32-bit channel number. The
whole `100::/64` prefix is reserved: it is in the IPv6 discard-only prefix
and uses port 0, which no UDP sender can have, and a datagram from a real
socket that claims any address in it is dropped and counted.

- A datagram the transport sends to a relayed address goes to the master as
  a Relay frame of that channel; a Relay frame from the master comes back as
  a datagram from that address. The game's datagram is unchanged inside the
  frame and stays at most 1,200 bytes.
- The handshake, its cookies and rate limits, the reliable messages, the
  5-second timeout and the [Keepalive](#keepalive) work as with any address.
  A stalled relayed game's keepalive thread sends through a wrapper that
  frames for the channel, so the host hears it from the channel's address.
- A host knows a relayed player by the address. Stage K never calculates a
  relayed host (John, 2026-09-28: relayed peers are never the calculated
  host).

*As built (J3):* both ends' routers (`tore_net::master::routed`) do the
wrapping, and `relayed_address` and `channel_of` turn a channel into its
address and back. Only `[100::1:HHHH:LLLL]:0` stands for a channel; the
rest of the prefix is never handed out. A joining player connects to the
relayed address with an ordinary `Client::connect`, so the path reads
"relay" at both ends. The keepalive thread's wrapper is
`tore_net::master::RelayFraming`, over a clone of the joining socket
(`ServerSocket::try_clone`).

### The path in the Challenge answer

*Protocol 9 (slice J2).* One byte after the platform byte: how
the player reached the host, the codes the master's
[reports](master-protocol.md#reports) use: 0 local network, 1 by address, 2
mapped port, 3 IPv6, 4 punched, 5 relay. Any other code makes the answer
malformed. The host keeps it with the player, and *protocol 12 (slice J6)*
puts it in the lobby's player list, 3 bits after the player's platform and
before its build (6 and 7 are invalid), so every player sees how every other
reached the host. A host takes a relayed address as relay whatever the byte
says.

| The address the race chose | Path |
| --- | --- |
| A private address (10/8, 172.16/12, 192.168/16, a link-local address, an IPv6 ULA; the carriers' 100.64/10 is not one), from the Direct Connection list, typed, or the host's Local candidate | 0 local network |
| Typed on Direct Connection, or `--connect`, not on the local network | 1 by address |
| The host's Mapped candidate | 2 mapped port |
| The host's Global IPv6 candidate | 3 IPv6 |
| The host's address as the master saw it | 4 punched: the game cannot tell a punched hole from a port the host forwarded by hand, so both read "punched". *Agent decision (J2):* a seen address that is also the host's own Mapped or Global IPv6 candidate reads as that one, 2 or 3 |
| An address a Punch taught the race | 4 punched |
| A relayed address | 5 relay |

*As built (J2):* the host's transport puts the path in its join details
(`ConnectDetails::path`), a relayed address as the relay whatever the byte
says. *Built (J6):* the lobby's player list carries it, the lobby screen
marks a relayed player and `tore-server`'s log names it on every join. A
host's Report still counts its players by their addresses
(`rendezvous::path_of`) until the hosts' Reports read the path the host keeps.

**Discovery stays IPv4.** *Agent proposal:* local networks carry IPv4
broadcast, and the master covers the internet, so the game does not look for
games on IPv6 multicast.

## Acknowledgements and round trip

- Sequences are compared the wrapping way: `a` is newer than `b` when
  `a - b` taken modulo 65,536 is between 1 and 32,767.
- Every packet acknowledges the newest sequence received and the 32 before it.
  A sent packet is **delivered** when an acknowledgement names it and **lost**
  when 33 newer packets are acknowledged without it. A packet already received,
  or more than 32 behind the newest, is dropped unread: it could not be
  acknowledged.
- **Round trip.** When an acknowledgement names a new newest packet, the
  sample is `now - (time that packet was sent) - ack delay`. It is smoothed as
  TCP does (gain 1/8 for the mean, 1/4 for the mean deviation). Before the
  first sample it is taken as 250 ms; a client whose Challenge answer went out
  once takes the wait for Accepted as its first sample (agent decisions, D2).
  Under reordering the estimate runs a little low, since the newest packet,
  the only one whose delay comes back, is more often a fast one: readings of
  286 to 306 ms, most below 300, on a 300 ms path with arrivals spread by
  plus or minus 15 ms (20 seeds).
- **Loss** is lost over lost plus delivered, over the last 5 seconds.
- **Arrival spread** is measured on snapshot packets (host to client) and on
  input packets (client to host) from the tick each carries, in the manner of
  RFC 3550's interarrival jitter. The transport does not read ticks: the
  session gives it each packet's send time, from its tick, and arrival time.
- Each side sends at least 10 packets a second while its loop runs; with
  nothing else to say it sends an empty Payload. A joined game whose loop is
  stalled sends a [Keepalive](#keepalive) once a second instead.

## Reliable messages

For what must arrive once and in order: the mission, seating, the roster, the
debrief, leaving, (protocol 3) the lobby and (protocol 4) chat.

- Each message has a 16-bit id, counting up from 0 in each direction, a kind
  (8 bits) and a body of at most 256 bytes.
- The sender keeps unacknowledged messages in a window of 256. A message goes
  into a packet's Messages section when it has never been sent or was last sent
  more than `max(1.25 × round trip, 30 ms)` ago. The packet remembers which
  messages it carried; when the packet is delivered, they are acknowledged.
- The receiver hands messages over in id order, holds early ones within the
  window and drops duplicates. An id 256 or more ahead of the next one to hand
  over is a protocol error. An id behind it is a duplicate however far back,
  since a packet that waited on the way can carry an id the sender's window has
  since left far behind (32 packets carry at most 8,160 new ids, so no real
  duplicate is 32,768 back).
- **Large messages** (the mission, the seated state, the roster, the debrief)
  are split into fragments of up to 256 bytes, each its own message; the first
  carries the total length in its record header (at most 64 KB, which the
  window of 256 holds whole). Ordered delivery puts them back together.
- During flight the Messages section takes at most 256 bytes of a snapshot
  packet, and before seating it may fill the packet. The budget is the
  session's per connection; the first due message goes whenever it fits the
  packet, even past the budget, so a whole fragment always moves and
  snapshots keep flowing.

**Message kinds:**

| Kind | Direction | Body |
| --- | --- | --- |
| Mission | host to client | The `MissionSpec` in its text form, the content manifest (resource names and their FNV-1a 64 hashes), the host tick now, the contrail sortie number, and the lobby mission's number (protocol 3) |
| Content refused | client to host | The mission's number, the resource names whose hash differs or which are missing, and the plain reason; in protocol 3 the player stays in the lobby, marked unable (before, the host disconnected with "content mismatch") |
| Take plane (stage D's Ready) | client to host | The mission's number and the plane wanted, or any: in the lobby, hold that slot and mark ready; in flight, fly it now |
| Seat refused | host to client | Reason text (plane taken, destroyed, lost its pilot, not open to humans, no free plane); the client may ask again |
| Seated | host to client | The connection's new [flight](#flights) (protocol 3), seat id, plane id, the tick of the state, the plane's exact state ([own aircraft](#the-own-aircraft)), its loadout (station weapon names, counts, mounts), the full roster, the standing and destroyed ground objects |
| Roster | host to client | Every plane: id, side, wing, member, aircraft key, pilot (AI, or a human's seat and callsign); sent on every change |
| Names | host to client | The [flight](#flights) whose table they extend (protocol 3), and new entries of the connection's name table: weapon records, shapes and sound names used by snapshots and events |
| Notice | host to client | A line of text for the HUD, for example "Mission restarts in 30 seconds" |
| Leave | client to host | The player ends its flight: the debrief, then back in the lobby, still connected (protocol 3; before, the host disconnected the player) |
| Debrief | host to client | The seat's debrief report as the single-player debrief shows it |
| Mission ended | host to client | Why (every human left, time limit, server stopping, ended by the server's operator or the King, the host left the game) and seconds until the next mission, if any; with a next mission the player stays, back in the lobby, otherwise the host disconnects the player once it and the debrief are acknowledged |

The lobby's messages, protocol 3 ([the lobby](../ARCHITECTURE.md#the-lobby)):

| Kind | Direction | Body |
| --- | --- | --- |
| Slot | client to host | The mission's number, and take a plane's slot, take the first free one, leave it, or (protocol 20) take the first free one on a side |
| Loadout | client to host | The mission's number, the slot's plane, and the loadout for it or none (the standard load) |
| Set ready | client to host | The mission's number, and ready or not |
| Change mission | client to host, the King | The new `MissionSpec` text |
| Start | client to host, the King | Nothing |
| Kick | client to host, the King | The player's lobby id and the reason the player is told |
| End mission | client to host, the King | Nothing: the mission ends for everyone, who return to the lobby |
| Lobby | host to client | The lobby's state, sent to every player whenever it changes |
| Refused | host to client | The refused request's message kind and the plain reason |
| Goodbye | host to client | Why the host is about to disconnect the player: kicked (with the King's words) or the host left the game |
| Flight loadouts | host to client | The mission starts flying: each loaded plane and its loadout, which every player builds the lobby's mission again with, and the content manifest entries they add, which the player checks |

Chat's messages, protocol 4 ([chat](../ARCHITECTURE.md#chat)):

| Kind | Direction | Body |
| --- | --- | --- |
| Chat send (25) | client to host | The receiver (All, Friendlies, Enemies, Wing or Target), the line's text and, for one of `CHAT.TXT`'s quick messages, its number (1 to 12) and sound |
| Chat line (26) | host to client | A delivered line: the sender's callsign and where the sender stands to the reader (no side, the reader's side, the other side), whether it is the reader's own line sent back, the receiver, the text and a quick message's sound; or the host's words alone (a refusal, or that no one heard) |

Stage L's messages, protocol 10 ([compatibility](#compatibility-stage-l)):

| Kind | Direction | Body |
| --- | --- | --- |
| Content (37) | client to host | The player's Fighters Anthology build, the T.O.R.E that made its import, and each content item's kind, key and digest; its first message after Accepted |
| Content gaps (38) | host to every player | The host's build and importer, and the items not every human can use, each with the players who cannot |

## What the transport settled

Details slice D2 (`crates/tore-net`) decided where the design left them open.
Each is an agent decision unless it says otherwise.

**The Messages section** (kind 1) is bit packed: a record count (8 bits, 1 to
255), then each record, then zero bits to the byte boundary.

| Field | Bits | Present |
| --- | --- | --- |
| Id | 16 | always |
| Part | 2 | always: 0 a whole message, 1 the first fragment, 2 a later fragment |
| Kind | 8 | whole message and first fragment |
| Total length less one | 16 | first fragment only; the total is 257 to 65,536 bytes |
| Body length | 9 | always: a whole message 0 to 256, a first fragment exactly 256, a later one 1 to 256 |
| Body | 8 a byte | always |

**Who decides what.** The host refuses a different protocol version (code 1)
and, past its connection limit (a setting, default 30), "server full" (code 3)
by itself. Every other decision goes to the session through one call with the
join's details (address, versions, build, callsign, password), which accepts
with the session id, rates and tick, or refuses with a code and text. Making
a callsign unique is the session's job. The session also checks its own
sections before anything in a packet is applied: a section it rejects drops
the whole packet unacknowledged and counts it as bad, so nothing the session
could not read is ever treated as delivered.

**Pace and limits.**

- A packet of due messages alone goes out at most 120 times a second; the
  session's own packets carry messages too. Without the pace a burst of
  messages sent every millisecond let packets be overtaken by more than 32
  newer ones and dropped as too old on a 300 ms path.
- A sender queues at most 8,192 messages, fragments counted one each (2 MB);
  past that, sending is refused and the session waits.
- One read of a socket takes at most 1,024 datagrams, so a flood cannot hold
  the host's loop.
- A Disconnect's three copies go out together.

**Events.** Every Payload a side sends is reported back as delivered or lost
by its sequence, which the snapshot baselines and the event queue need.
Every connection that ends, and every join that fails, reports exactly one
reason: no answer, refused with its code and text, disconnected with its
reason and whether the other side said so, or (on the host) replaced.

**Time and randomness.** Nothing in the transport reads a clock: the caller
passes the time in, which lets the simulator run faster than real time. A real
endpoint draws its nonce, connection ids and cookie key from the standard
library's randomly seeded hasher, each value from a fresh one; tests and the
simulator can seed them instead, which makes a whole session repeat exactly.

**The simulator** joins in-process endpoints by one-way links with a latency,
arrivals spread uniformly within plus or minus a width (reordering follows),
loss, duplication and optional burst loss (two states, good and bad), on a
virtual clock. It can record every datagram with its fate and arrival times.
Routers can stand between endpoints: address translation with each of RFC
4787's mapping and filtering behaviours, nested routers and IPv6 firewalls
([the NAT simulator](../ARCHITECTURE.md#the-nat-simulator)).

## Inputs

The Inputs section carries the player's controls for recent ticks. The client
sends one input packet per rendered frame that stepped at least one tick, at
most 60 a second, and each repeats every tick the host has not acknowledged,
up to 24 ticks (200 ms). A tick's input is lost only when every packet that
carried it is lost.

| Field | Size | Meaning |
| --- | --- | --- |
| Flight | 8 | The connection's [flight](#flights) these inputs fly, the number the Seated message gave; the host drops inputs of another flight (protocol 3) |
| Newest tick | 32 | The last tick in this section |
| Tick count | 5 | How many ticks follow, 1 to 24, oldest first, ending at the newest |
| View offset | 8 | The host tick the player's screen showed when the newest tick was sampled, as ticks before the newest; used for [lag compensation](../ARCHITECTURE.md#hits-and-lag-compensation) |
| Interpolation delay | 6 | The client's interpolation delay in ticks, which bounds the rewind's cap |
| View subject | 1, then 3 (2 before protocol 22) and a varint | Whether the player's view follows another entity (target, wing, external or fly-by view), then its kind and id; the host sends that entity at the full rate. *Built (D6):* the id is a varint, since projectile numbers pass 65,535 in a long mission |
| Mismatch | 32 | The newest snapshot tick whose own-state hash differed from the client's own, or 0 |
| Per tick | | The first tick in full, each later one as a "same as the tick before" bit or its changed fields ([settled](#inputs-as-built)) |
| Commands | | The unacknowledged commands, below ([settled](#inputs-as-built)) |

A tick's continuous controls:

| Control | Coding |
| --- | --- |
| Pitch, roll, yaw | 16 bits each: the value in -1 to 1 times 32,767, rounded |
| Throttle rate | 8 bits: -1 to 1 times 127 |
| Throttle position | 1 bit present, then 16 bits: 0 to 1 times 65,535 |
| Trigger | 1 bit |
| Scope controls | 1 bit changed, then channel (2), range step (4), contact history (1) |

The client rounds its own controls to these steps before its prediction steps
them, so the host and the client step identical numbers. Single player never
rounds.

**Commands** are what a player does once: every `SeatCommand` (weapon
selection, designation, arming, chaff and flares, the trigger key, tower
requests, radio silence, wing orders) and every pilot command (eject, the gear,
flaps, brake, hook, bay, engine, burner, radar, jammer and autopilot switches,
throttle presses). Each has a 16-bit command number, the tick at which the
client applied it in its own prediction, and its coded form. They repeat in
every input packet until the host acknowledges them in a snapshot; the host
applies each exactly once, in number order, at its tick, or on the next tick
if that one is already stepped. A toggle therefore never toggles twice.

## Snapshots

One Snapshot section per snapshot packet, 60 a second by default (30
before slice D12). The host
shares the 1,200 bytes so that the entities always keep their place: after the
headers (55 bytes: the payload header, three section headers and a snapshot
header of at most 21), the cockpit readout takes up to 200 bytes, the events
up to 150 and the messages up to 256, and the entities get the rest, at least
545 bytes; whatever one section leaves unused goes to the entities, and the
events may use what the entities leave ([settled](#snapshots-as-built)). The
exact own state, when due, travels in a second packet. *Agent decision (D10
follow-up):* a connection's snapshot ticks are the ticks whose number modulo
the ticks per snapshot equals its seat number modulo the same, so the seats
share the interval's ticks; the Seated message's seat tells the client its
phase, and baselines (counted in snapshots back) are unaffected.

### Header

| Field | Size | Meaning |
| --- | --- | --- |
| Flight | 8 | The connection's [flight](#flights) the snapshot belongs to (protocol 3); the Events section in the same packet shares it |
| Tick | 32 | The host tick the snapshot shows (after that tick's step) |
| Input received | 32 | The newest input tick received from this player |
| Input margin | 8, signed | Over the inputs received since the last snapshot, the smallest number of ticks by which one arrived before the host needed it; negative when late |
| Inputs repeated | 8 | Ticks since the last snapshot for which the host had no input and repeated the last one |
| Commands applied | 16 | The highest command number applied, all before it applied too |
| Own state hash | 64 | FNV-1a 64 of the player's plane's exact state at this tick, as the exact coder writes it; absent before seating |

### The own aircraft

The player's plane's **exact state** is what the client's prediction needs to
match the host bit for bit: the flight state, the cockpit's turbulence state
and random stream, and the ownship terms the per-plane step reads (the
subsystem hit counts, the sensor and jammer failures, the stores' weight,
whether a release holds the bay open, hit points, the broken section and the
damage in each section; the full list is in the
[architecture](../ARCHITECTURE.md#one-step-for-a-humans-plane)), and the
turn-back and OVERSPEED message clocks.
Every field is coded, the private ones included (the stall scale that depends
on the weight's history, the hybrid model's random state, the systems, the
autopilot, the wreck and the escape; since protocol 19 the powered-lift
state too: body rates, rotor speed and its reference, the rotors' turns (the
blade angle's integral, presentation only), each rotor's induced
velocity and disk tilt, engine output, lift-engine spool, stability level,
trim, attitude reference, trim latch, warning timers and the V-22 corridor
hold), except the write-only trace, the flight
at the start of the tick and the imported tables, which the client already
has ([details](../ARCHITECTURE.md#the-exact-state-of-a-humans-plane)).

Every snapshot carries only its **hash** (in the header). The exact state goes
in an **Own state** section, in its own packet beside the snapshot packet, when:

- the host's tick did something to the plane the client cannot foresee: it
  repeated a missing input, applied a command at another tick than the
  client's, or the plane was hit, jolted by a blast, released a store or had
  its ownship terms change;
- the client's input reported a mismatch; or
- a second has passed since the last one, as a safety net.

Each 64-bit number is coded as the exclusive-or with the same field of a
baseline, with its leading zero bits counted, so an unchanged field costs one
bit and a slowly changing one a few bytes: about 200 to 350 bytes in all
(measured in D4: about 160 to 260 bytes against a state one snapshot back, and
under 500 with no baseline). The
baseline is an earlier exact state the client has acknowledged, named by how
many own states back it was (5 bits, 1 to 31); 0 means no baseline
([settled](#own-state-as-built)). The client decodes it bit for bit. The
section starts with the connection's [flight](#flights) (8 bits, protocol 3),
then its tick (32), its number (16) and how many back its baseline is (5).

### Cockpit readout

The [cockpit readout](../ARCHITECTURE.md#the-flight-screen-draws-a-frame) of
the player's seat, in groups (stores and selection; seeker and tone; weapon
estimates; targets; radar and infrared contacts; visual contacts; map
contacts; RWR emitters and missile records; damage and faults;
countermeasures; airport and NAV; target window; music inputs and mission
result). Each group has a changed bit against the readout of the baseline
snapshot; only changed groups are sent, and lists within a group (contacts,
emitters) are coded entry by entry against the baseline's entry with the same
id. Contact positions are world positions, in whole feet
([as built](#the-cockpit-readout-as-built)).

### Entities

Everything the client draws that moves: aircraft (every plane except the
player's own, human-flown or not), missiles, bombs and rockets in flight,
debris pieces, ejected pilots and, since protocol 22, the surface units that
follow a route (a ground target's column, its cargo ships). Gun rounds are not
entities; they arrive as burst [events](#events). Ground objects that stand
still are built from the mission on every client; their destruction and their
changed states are events ([the surface world](#the-surface-world-protocol-22)).

Each entity record:

| Field | Meaning |
| --- | --- |
| Kind | Aircraft, projectile, debris, pilot or (protocol 22) surface unit. *Built (D6):* records come kind by kind, each kind's count first, so a record carries no kind bits |
| Id | The plane id, projectile number, or the aircraft a debris piece or pilot came from; coded as the difference from the previous record's id of the same kind |
| Removed | 1 bit: the entity is gone; nothing follows |
| Baseline | Snapshots back to the acknowledged state this record is coded against (7 bits since protocol 16, 5 before); 0 is a full record |
| Fields | Coded against the baseline, below |

**Baselines are per entity.** For every connection the host remembers, for
the last 128 snapshot packets it sent, the quantized state it sent for each
entity. When a packet is acknowledged those states become that entity's
acknowledged baseline. A record in a new packet codes against its entity's
newest acknowledged baseline if it is at most 127 snapshots old (2.1 seconds
at 60 a second), else in full. The client keeps every entity's received
states for the last 160 snapshots, since a packet 32 behind the newest can
still arrive with records 127 further back. *Built in protocol 16 (slice
B2):* before it the window was 31 snapshots, the host remembered 64 packets
and the client 64 snapshots ([why it grew](#the-cockpit-readout-as-built)). The host
remembers the quantized values it sent, not the exact ones, so both sides hold
the same baseline and rounding never builds up.

**Prediction from the baseline.** Positions are predicted as the baseline's
position plus its velocity times the ticks between, in whole steps with
integer arithmetic only, and only the difference is sent, as a signed
variable-length number of quantization steps. Velocity, attitude and speed
send their difference from the baseline. Slow fields (the other ten animated
devices, engine flags and rates, damage, wreck phase, airborne and crashed,
a rotorcraft's rotors) are sent only when their group changed, behind one bit
each.

| Kind | Fields |
| --- | --- |
| Aircraft | Aircraft key on first sight; position, velocity, attitude (yaw, pitch, bank); devices; engine (lit, afterburner, flame, thrust-vectoring rates); damage (hit points of the initial, section damage, structural section); airborne, crashed, wreck phase; a rotorcraft's rotors (rotor speed, each main rotor's disk tilt; since protocol 19) |
| Projectile | On first sight: owner, weapon and shape (name table), target, whether it is aimed at this player's plane; then position, velocity, direction. *Built (D6):* velocity replaces speed, so that the prediction needs no trigonometry and both ends agree to the step; the speed is its length |
| Debris | On first sight: owner, drawn model and damage variant; then position, velocity, attitude (*built (D6):* with a velocity, from the picture a tick before, for the prediction) |
| Pilot | On first sight: the aircraft it left; then position, velocity, heading, escape phase |
| Surface unit (protocol 22) | Its unit id is its identity; position, ground-relative velocity, heading, pitch and bank, and whether it is a wreck |

**Priority and relevance.** Each entity has a priority that grows every
snapshot by its relevance weight and resets when it is sent. An entity is due
when its priority reaches the snapshot rate: a near one every snapshot, a far
one 4 times a second at most (twice before slice D12; [the far
interval](#connecting)). The host picks due records in priority order until the
entities' space is full, so what cannot fit waits and is sent first next time,
then writes the chosen records in id order, which the id coding needs. The
relevance bands are the [netcode numbers](../MULTIPLAYER.md#netcode-numbers)'
(John, 2026-09-30; rates raised by John, 2026-10-06). A missile aimed at the player is always sent. An entity
that leaves is sent as Removed until that is acknowledged.

**The first snapshot** after Seated has no baselines, and the host queues
events that describe the mission as it stands: every crater and crash-site
fire, every destroyed ground object, and every effect still showing. Smoke and
contrails already in the sky before the player joined are not sent; they
appear as the client's own regeneration starts.

## Events

Things that happen once, for this player or for everyone. Each connection
numbers its events (16 bits). Every snapshot packet repeats, oldest first,
every event the client has not acknowledged, within the events' share of the
packet (150 bytes, and whatever the entities leave); an event's packet being
delivered acknowledges it. Each event carries its tick, as ticks before the
snapshot's tick.

| Event | For | Fields |
| --- | --- | --- |
| Message | the seat | A HUD line |
| Radio | the seat | Route (radio, airport, direct), speaker label, text, recording stems; since protocol 15 the net it was heard on |
| Tower | the seat | A tower recording stem, or cut the tower off |
| Order voice | the seat | The seat's own order call stems |
| Order reply | the seat | What became of a wing order |
| Weapon cycled | the seat | The weapon page turns |
| Release | the seat | Weapon release sound and station |
| Launch | everyone | Shooter, projectile number, weapon: every missile, rocket and bomb launch, plus surface flak gun reports (the shell itself is still omitted from snapshots). No new fields or event tags. For guided stores, the view rig and the F12 view see a missile that lives less than one snapshot |
| Feedback | the seat | A rumble event |
| Your aircraft exploded | the seat | Which wording ("exploded", "exploded on impact") |
| Wing ejection | everyone | Aircraft, HUD line, friendly |
| Effect | everyone | Kind, position, ticks it shows, explosion type, and (protocol 26) the size it is drawn at |
| Mark | everyone | Crater or crash-site fire, position |
| Ground destroyed | everyone | Ground object id |
| Countermeasure | everyone | Aircraft, chaff or flare, the release geometry the replay viewer flies it from, number left |
| Gun burst | everyone | Shooter, gun station, first tick, last tick (0 while still firing). *Built (D7a):* sent when the burst starts, and again from its first tick with its length once the station has fired no round for its weapon's round interval plus 2 ticks. *Read by the client (D8c):* it makes the burst's rounds again from the first tick at the weapon's cadence ([the client session](../ARCHITECTURE.md#the-client-session)) |
| Sound | everyone | Emission kind, position, the aircraft it came from |
| Link | the seat, about members of its flight | A data link change: a member joined, a lock taken or dropped, an assignment given, cleared or acknowledged, a sort warning (protocol 15, [the data link](#data-link-stage-g)) |
| Surface burst | everyone | A surface unit's gun burst (protocol 22): unit, hardpoint, target, the first round's direction, the burst's rounds and ticks ([the surface world](#the-surface-world-protocol-22)) |
| Surface burst end | everyone | That burst stopped after this many rounds, short of its schedule (protocol 22) |
| Surface unit | everyone | A surface unit's hit points, radar, launcher rails and spare magazines, when they change and at a seat (protocol 22) |
| City flattened | everyone | City fill buildings a ground burst knocked flat, by index, when they fall and at a seat (protocol 25, [the city fill](#the-city-fill-protocol-25)) |
| Stores | everyone | An aircraft's drawn stations and the whole stores still on each, when they change and at a seat (protocol 27, [stores](#protocol-27-stores)) |

Recording stems, weapon and sound names are sent by their index in the
connection's name table (the Names message), which the host fills before the
first use. Radio text is sent as it is: the client never composes calls.

## Data link (stage G)

*Built in protocol 15 (slice G7, 2026-10-06)* ([architecture](../ARCHITECTURE.md#flight-data-link),
[guide](../DATALINK.md)). What the flight data link adds to the wire. Every
choice is an agent decision unless credited. Nothing of it reaches a client
that a human in the same slot would not see: each part is the seat's own
share, computed by the host (`DataLink::readout`, the same share single
player draws).

**Readout parts.** Four parts join the [cockpit readout](#the-cockpit-readout-as-built),
coded like the others (a changed bit each, against the acknowledged
baseline). The record now has 30 parts.

| Part | Place | Kind | Fields |
| --- | --- | --- | --- |
| Link | Second, right after the header, so an assignment is never the part that waits for room | Scalar group of four signed varints | The radar flag; the assigned target plus one (0 for none); the plane that assigned it; acknowledged |
| Link marks | After the contacts | List keyed by target id, at most 32 | Slow fields only: lockers (a mask of the flightmates' member numbers, signed varint), the battle net's lock (a presence bit, then the flight and the member number, signed varints), assigned to (a mask, signed varint) |
| Link mates | After the marks | List keyed by plane id, at most 5 | Slow fields only: member number (3 bits), fuel (normal, joker, bingo, fumes, out: 3 bits), weapons (missiles, guns only, Winchester: 2 bits), damage (none, light, heavy: 2 bits) |
| Link tracks | After the mates | List keyed by target id, at most 24 | Slow field: source (own, flight, network: 2 bits); then position in whole feet and velocity in 1/4 ft/s, each sent as its difference from the baseline's |

*Settled by the build:*

- **Marks and mates before the tracks.** The design put the tracks first. The
  marks carry the locks and the flight's assignments, which change the tick
  they happen and are small; the tracks are the bulky part. Coming first, a
  big track update never holds a lock back.
- **Tracks are not predicted.** A track's position stays where its reporter
  observed it until the next publishing tick (every thirtieth), so it is
  sent as a plain difference: between publishing ticks nothing goes, and on
  one each track costs its move (about 250 feet for a fast fighter) and any
  velocity change. Predicting it from its velocity, as the contacts are,
  would send a correction every snapshot.
- **No battle net flag.** The design's Link group had whether the seat
  monitors the battle net. The readout (`LinkReadout`) has no such field and
  nothing draws it: the seat learns it from the "Monitoring battle net" and
  "Battle net off" lines, which are Message events.
- **Order on the client.** The tracks come back nearest the client's own
  plane first, as the host lists them (in target order with no plane); the
  marks and mates in id order, as the host's are.
- A plane with no radar gets every part all the same, with the radar flag
  down; only its radar scope leaves the marks out (John, 2026-10-05).

**Event.** One new event code, 17, after Sound: Link, a data link journal
entry (`tore_world::datalink::Entry`) about a member of the seat's flight.

| Field | Bits |
| --- | --- |
| What | 3: member 0, lock 1, unlock 2, assigned 3, cleared 4, acknowledged 5, sort warning 6 |
| Plane | varint: the member the change is about (the receiver for an assignment, the warned human for a sort warning) |
| Member | its radar flag, 1 bit |
| Lock, unlock, acknowledged | the target, varint |
| Assigned | the target, the assigner (varints) and the order, coded as an Order reply's |
| Cleared | the target (varint) and why (2 bits: order, receiver lost, target lost, lead changed) |
| Sort warning | the flightmate holding the same lock, then the target (varints) |

*Settled by the build:* the design's delivery bit (link or voice) went with
the tiers (John, 2026-10-05); the assignment carries its order instead, so a
converted replay says it was a sort. Member is an addition, so a converted
replay holds the flight's members as a recorded one does. The host drains
the world's data link journal once a tick (`journal::drain`, beside the score
facts and device notes, so a standby drains it too) and queues each entry
for every seat whose plane is in the entry's flight, after the tick's other
seat events. A seat seated later does not get the entries before its
seating: the readout carries the state they left. Events repeat until
acknowledged, so a lock or an assignment reaches the client in the next
snapshot and is never lost. The sort warning's HUD line and beep are ordinary
Message and Radio events. A captured `Link` event becomes the replay's
`datalink.*` event when the capture converts, with the fields the
single-player recorder writes ([network flights](../REPLAYS.md#network-flights)).

**Radio events** gain the call's net, one bit after "important" (wing 0,
battle 1). The label of a call heard on the battle net already starts with
`Net `, so the client shows it as it is.

**Inputs.** Unchanged in layout: Sort is wing order code 13 (built in G3c)
and Battle net is command code 23 (built in G8); protocol 15 is the version
that carries them, and the golden lists both.

**Room.** Measured on the 15 against 15 mission
(`crates/tore-session/tests/bandwidth.rs`, three minutes, the player sorting
its wing every 5 seconds while it leads, which gave 16 assignments, 12
acknowledgements and 20 locks about its flight): the four parts take 9 bytes
a snapshot on average (the tracks 8), 247 bytes in the first snapshot, which
brings the whole share across, at most 146 bytes in any later snapshot, and at
most 1.8 KB in any one second; the Link events add 8.5 bytes in the packets
that carry any. In all about 280 bytes a second, inside the readout's
200-byte share. Against protocol 14 on the same flight the snapshot packet's
mean is the same (384 bytes) and the readout's record grows from 52 to 56
bytes on average; the snapshots after the first second in which a far entity
waited for room rise from 310 to 392 of 5,400 (from 2 to 10 with no room kept
for messages), at the busy moments when the tracks move.

## Quantization

| Quantity | Step | As in replays |
| --- | --- | --- |
| Positions of aircraft, projectiles, debris, pilots, effects, marks, contacts | 1/32 ft | yes |
| Velocities | 1/64 ft/s | yes |
| Attitude of other aircraft, debris; pilot heading | 2^-16 of a turn (0.0055 degrees) | coarser (replays use 2^-20) |
| Projectile direction | 2^-16 of a turn per angle (heading from +z towards +x, then elevation) | coarser |
| Aircraft speed (a device) | 1/4 ft/s | coarser |
| Devices from 0 to 1 | 1/255 | yes |
| Elevator, aileron, rudder | 1/127 | yes |
| AC-130 gun-mount angles (heading over pi, elevation over a right angle) | 1/32,767 (1/127 before protocol 21) | coarser (replays keep 1/127) |
| Thrust-vectoring rates | 1/4096 rad/s | yes |
| Rotor speed (a share of 100 percent) | 1/1000, up to 2 | yes |
| Rotor disk tilt | 1/256 rad, up to 127 steps either way | not recorded |
| Stick inputs | 1/32,767 | finer (replays use 1/1024) |
| Throttle position | 1/65,535 | finer |
| The own aircraft's flight state | exact | exact |
| Ids, counts, flags, hit points, ticks | exact | yes |

*Built (D6), correcting the design:* a value that cannot be quantized is
clamped, not sent exactly. Not finite is zero, and a position or velocity
beyond 2^40 steps (34 million miles) is held at it; a device level outside its
range is held at its end. The quantized numbers are all both ends ever store,
so a clamped value is simply what the client draws.

## What the game's sections settled

Details slice D6 (`crates/tore-session`, module `wire`) decided where the
design above left them open. Each is an agent decision unless it says
otherwise. Varints are `tore-codec`'s (7 bits a group); a **bucketed** value
is a bucket index then the value in that bucket's width, with the last index
an escape to a varint; strings are a length byte and UTF-8.

### Inputs as built

| Field | Bits |
| --- | --- |
| Newest tick | 32 |
| Tick count | 5, 1 to 24 |
| View offset | 8 |
| Interpolation delay | 6, 0 to 63 |
| View subject | 1, then kind 3 (2 before protocol 22) and id varint |
| Mismatch | 32 |
| First tick | pitch, roll, yaw 16 signed each (-32,767 to 32,767); throttle rate 8 signed (-127 to 127); throttle 1, then 16; trigger 1; powered-lift block (below); scope channel 2 (radar, infrared, visual, and since protocol 23 the HARM page, 3), range step 4 (0 to 5, the scope's six ranges), history 1; gunsight block (protocol 21, below) |
| Each later tick | 1 bit "same as the tick before"; else 9 change bits (pitch, roll, yaw, throttle rate, throttle, trigger, scope, powered lift, gunsight; 8 before protocol 21), then each changed value: a stick as its difference from the tick before (bucketed, 4, 8 or 17 bits), the rest as in the first tick; a changed trigger flips and needs no value |
| Command count | 7, 0 to 64 |
| First command number | 16, when there are commands; the rest follow one by one |
| Each command | ticks before the newest (varint), a 5-bit code and its fields |

Protocol 18 adds a powered-lift block: one present bit, then four signed rate
bytes (-127 through 127) and four optional signed 16-bit positions (-32,767
through 32,767). Axis order is vector pitch, vector yaw, conversion, collective.
Only vector yaw permits negative positions; other positions cover 0 through
32,767. An absent block means zero rates and no absolute positions.
The cockpit stores readout also carries the authoritative six gun angles
(at 1/32,767 since protocol 21, 1/127 before) and linked mask for the
predicting player, whose remote entity is excluded.
Combat command codes 26 and 27 select a gun-group candidate and toggle its
membership. Device mask interpolation is discrete.

Protocol 21 (the AC-130 gunsight, slice S4) adds a **gunsight block**: one
present bit, then the slew's two deflections, x right and y up, each a
signed byte (-127 to 127; -128 is refused), and the target camera's zoom
step in 3 bits (0 the default step, 1 to 6; 7 is refused). The block is
absent when there is no slew and the zoom is 0, and a present block that says
so is refused. The host integrates the look from it
([the gunsight](#the-gunsight)), so single player and a network seat run the
same law. The sight's commands are combat codes 28 (`SightDesignate`,
Backslash) and 29 (`SightPinGround`, Shift+Backslash), with no fields; L
stays `ClearDesignation`. A paused game's neutral frames keep the zoom step
and drop the slew, as they keep the scope.

Protocol 23 (the air-to-ground round) takes in the combat codes its slices
added: 30 is `ToggleAirGround` (Ctrl+R, the A-G radar page), and 31, the
last 5-bit code, is an **extended** combat command: a varint follows, 0
`NearestGround` (Ctrl+X under Easy targeting), 1 `ToggleLaser` (Ctrl+L) and
2 `SightNext` (`/`), none with fields; a reader refuses any other varint.
The mp-fixes round (protocol 24) adds 3 `GunKeyDown` and 4 `GunKeyUp` (Tab, held:
the gun fires whatever store is selected; [the gun
key](../spec/air-to-ground.md#the-gun-key)), also with no fields. The host
keeps the key's state per aircraft; a trigger release (`ReleaseTrigger`) and a
resume start it up.
Extended code 5 is `JettisonTanks`. Protocol 71 adds code 6,
`DesignateVisualTarget`, followed by an unsigned-varint aircraft id. Only a
current ownship visual contact is accepted; the selection is marked visual
for retention, independently of radar support. Unknown extended codes are
refused.
The seat's slew and zoom in the gunsight block steer the targeting sight of
every aircraft with a laser designator or a FLIR too, and codes 28 and 29
work it as they do the AC-130's ([the air-to-ground groups](#the-air-to-ground-groups)).
The delivery mode (CCIP or LVL, Ctrl+M) rides the existing
`ToggleSeekerMode` (code 5).

Set-axis (code 24) and adjust-axis (code 25) commands carry a two-bit axis
code plus an optional position or signed step; neutral-vector (code 26) is a
separate command with no fields. Ordinary
fixed-wing input pays only the absent bit on the first tick.

Protocol 19 (the VTOL overhaul) adds the **powered-lift command**, code 27,
which extends the nearly full 5-bit code space with a 4-bit sub-code and the
sub-code's fields. Hover hold is switch 11 (a switch's 4-bit code has room
for four more). The sub-codes:

| Sub-code | Command | Fields |
| --- | --- | --- |
| 0 | Set the stability level | level, 2 bits: 0 Off, 1 Damper, 2 Attitude (3 is refused) |
| 1 | Cycle the stability level (Off, Damper, Attitude, Off) | none |
| 2 | Trim set (the stick plus trim becomes the trim) | none |
| 3 | Trim adjust | axis, 2 bits: 0 pitch, 1 roll, 2 pedal (3 is refused); step, 16 signed, 1/32,767 of full travel, rounded as an adjust-axis step is |
| 4 | Trim to centre | none |
| 5 | Nozzle step of 10 degrees | 1 bit: down (X) or up (Z) |
| 6 | Nozzle preset | 1 bit: vertical (Shift+X) or forward (Shift+Z) |

Sub-codes 7 to 15 are refused. The powered-lift input block keeps its four
axes and its coding. What a nozzle or conversion position of 0 to 1 means
moves with the overhaul's jet and tiltrotor slices, to 0 to 100 degrees of
nozzle travel and 0 to 97.5 degrees of nacelle angle; until they land, the
flight reads both as 0 to 90 degrees.

The command codes cover every `SeatCommand` (a combat command has its own
5-bit code, with a heat byte, a distance or a target id where it has one; a
wing order has a 4-bit code and its break, approach or formation; code 14 is
Engage IR/Laser and code 15 escapes to three more bits, 0 engage my target's class and
1 Lase my target, the strike orders folded under protocol 25) and every
pilot command (a switch is 4 bits). A reader refuses a change bit whose value
did not change, so every section has one encoding. `quantize_pilot` rounds a
pilot input exactly as the wire does, the throttle commands included (a
position to 1/65,535, a step to 1/32,767, each clamped to its range), and the
client steps what it returns. The host takes the frames and the commands with
their ticks; `InputFrame::seat_input` makes the `SeatInput` of a tick and
`InputsSection::view` the `SeatView` lag compensation reads.

*Settled by the host (D7a):* a client numbers its commands from 1, so a
snapshot header's "commands applied" of 0 means none yet; a command is sent
only once the client's game has reached its tick (a reader refuses a command
after the section's newest tick). Each earlier frame of a section takes the
newest frame's view, as many ticks earlier. The host's buffer rules are in
the [architecture](../ARCHITECTURE.md#the-host-session).

### Snapshots as built

The section is the header (161 bits), one bit for the cockpit readout and,
when it is set, the readout's record ([below](#the-cockpit-readout-as-built)),
then the five kinds in turn
(aircraft, projectiles, debris, pilots and, since protocol 22, surface units):
each kind's count as a varint and its records in id order.

| Record field | Bits |
| --- | --- |
| Id | The first record of a kind: its id; later ones: the id less the previous one less 1. Bucketed unsigned: 2 bits of index, then 0, 4 or 10 bits, or a varint |
| Removed | 1 |
| Baseline | 7: snapshots back, 1 to 127; 0 is a full record (5 bits, 1 to 31, before protocol 16) |
| Full body | The identity fields; position and velocity as six signed varints; the angles, 16 bits each (aircraft, debris and surface units 3, projectiles 2, pilots 1); an aircraft's speed as a signed varint; every slow field |
| Body against a baseline | 1 bit "moved"; if set, the position residuals after the prediction, the velocity, angle and speed differences, each bucketed (position and velocity 3, 6, 10, 14 or 20 bits; angles 3, 6, 9, 12 or 17; speed 3, 6, 10 or 16); then for each group of slow fields a changed bit, and in a changed group a bit per field and each new value |

The identity fields (full records only): an aircraft's type as 1 bit and its
place among the 37 selectable aircraft (6 bits); a projectile's owner
(varint), weapon (12-bit name index), shape (1 and 12), target (1 and a
varint) and whether it is aimed at this player; a debris piece's owner, the
aircraft whose model draws it (1 and 6) and its damage variant (1 and 3); a
pilot's aircraft. An aircraft's slow groups are its devices (present, six
levels at 1/255, three control surfaces at 1/127, the throttle at 1/255, actual vector pitch/conversion/collective at 1/255
and actual vector yaw at 1/127, six normalized gun-mount angles at 1/32,767
in 16 signed bits each (1/127 in 8 bits before protocol 21: a client remakes
an AC-130's rounds along this train, and 1/127 left them up to 0.7 degrees
off the host's) and a three-bit linked-gun mask; an
aircraft without devices sends only the present bit), its engine (lit,
afterburner, flame, three rates as signed varints), its damage (hit points,
initial hit points and six sections as signed varints, the structural
section in 3 bits), its status (airborne, crashed, wreck phase in 2 bits)
and, since protocol 19, its rotors (present; the rotor speed in 11 bits,
thousandths up to 2,000; four disk tilts at 1/256 rad in 8 signed bits each,
the front or main rotor's longitudinal and lateral, then the CH-47's rear
rotor's or the V-22's right proprotor's; an aircraft without rotors sends only
the present bit, and an aircraft whose rotor speed is zero counts as one
without); a pilot's is its escape phase (3 bits); a surface unit's is its
wreck flag (1 bit; it has no identity fields). The client draws a remote
rotorcraft's blades by integrating the received rotor speed (the [rotor
presentation](../spec/rotor-presentation.md#rotor-speed-and-blade-angle)), so
no blade angle is sent.

- **Records parse without their baseline.** Every field against a baseline
  is a self-delimiting difference or a whole new value, so a client that
  lacks a record's baseline reads past it and uses the rest of the packet.
  The host codes only against states the client acknowledged, so this is a
  safety net; the lossy test never needs it.
- **Baselines by snapshot.** A record names its baseline by snapshots back,
  not by packets, since own state and message packets go between snapshots
  and the host cannot know their numbers before it sends them. The ticks
  between are the snapshots back times the ticks per snapshot, and the
  prediction divides the baseline's velocity steps by 240 per tick with
  integer arithmetic, rounding halves up, so both ends agree to the step. The
  host codes against an entity's newest acknowledged state when the gap is 1
  to 127 whole snapshots (31 before protocol 16) and its identity fields
  match, else in full.
- **Ids.** A debris piece's and an ejected pilot's id is the aircraft it came
  from: each aircraft breaks off at most one piece and ejects at most one
  pilot. The player's own pilot is not sent; its escape is part of its plane's
  exact state. *Built (B5, 2026-10-06, no protocol change):* a two-seater's
  second crew member ejects too, and his chute is a pilot entity of its own:
  its id, and the `owner` field of its full record, is the aircraft's id with
  bit 31 set (`wire::entity::CREW_PILOT_BIT`). Unlike the pilot's, it is sent
  to the seat that flies the plane as well, since the exact state does not
  carry it (the client's prediction never needs it; a checkpoint does carry
  it, after the exact state, slice B6). A build that does not
  know the bit sees a pilot of an aircraft that does not exist and draws it
  there; a client that knows it draws two chutes for one aircraft.
- **Removals.** The host sends Removed for every entity the client may know
  that is gone, in every snapshot until a packet carrying the removal is
  delivered; one that comes back is sent in full. On the client the newest of
  a state and a removal wins, whatever order packets arrive in.
- **Priority.** Priorities are whole numbers: a near entity adds the snapshot
  rate each snapshot, a far one 4 (2 before slice D12), and an entity is due
  at the snapshot rate and starts again from nothing when sent, so a far one
  is due every *F* snapshots ([the table above](#connecting)), 4 times a
  second at most at every rate (the table's weights, 1 and 1/15 at 60 a
  second). An entity the connection has never had is due at once. Missiles aimed at the player go first, then removals, then due
  entities by priority; a record is sized with its id's whole value before it
  is chosen, so the written section is never larger. *Correction to the
  design*, which said a band waits only when the packet is full: a far entity
  is sent only when due, so it costs its bytes at its own rate whatever room
  there is.
- **Shares.** The snapshot header takes at most 21 bytes, so the entities'
  least share is 545 bytes. The host keeps the messages' room at the
  caller's figure (256 bytes in flight, less when it knows fewer are
  waiting); the events take whatever the entities leave, and the oldest
  event may use the messages' room too, as a long message may, so it is
  never starved.

### The cockpit readout as built

The readout's record is the baseline (7 bits since protocol 16, 5 before:
snapshots back to the readout the client acknowledged, 1 to 127, or 0 for
none, against the empty readout),
then 40 parts (32 before protocol 23, 31 before protocol 22, 30 before
protocol 21, 26 before protocol 15), each behind a
changed bit, in this order, which is also their importance: header (plane and
tick), link (protocol 15), stores, gunsight (protocol 21,
[below](#the-gunsight)), bomb sight, guided sight, laser, targeting sight,
sight sensor, HARM cue and weapons in flight (protocol 23,
[below](#the-air-to-ground-groups)), countermeasures, damage, seeker, seeker observation, estimates,
estimate observation, targets, displayed target, viewed target, airport,
target window, music, designated enemy, AI locks, painting radars (protocol
22: the ids of the surface radars painting the plane, a list like the AI
locks), inbound missiles, threat
records, emitters, sensor scalars, contacts, link marks, link mates and link
tracks (protocol 15, [the data link](#data-link-stage-g)), strobes, plots,
trails, visual contacts, map; the ground contacts (protocol 23) come right
after the contacts. The client's `CockpitReadout`
comes back from them (`QReadout::readout`; `ClientConnection::cockpit_readout`
gives the newest one around the client's predicted plane, for its flight
frame's `ReadoutSlot::ready`). The host puts each seated player's readout,
built from the tick's flight, in every snapshot.

- **Scalar groups** (stores, the gunsight, damage, the seeker's status and tone, the
  estimates, the target ids, the airport, the target window, the music's
  flags and aims, the locks, the sensor flags) go whole when they changed: a
  count and signed varints.
- **Lists** (contacts, visual and map contacts, plots, strobes, emitters,
  threat records, inbound missiles, and the target rows and seeker
  observations as lists of at most one) go as the ids removed and the entries
  that changed, in id order, each in full or against the baseline's entry
  with its id: one bit and the differences of what moved, bucketed (2 to 24
  bits or a varint), one bit and each slow field that changed. A position
  with a velocity is predicted from it, as the entities' are, so an entry
  flying as predicted is not sent at all.
- **Trails** go as the points each dropped from the front of the baseline's
  trail and the points it added, each a bucketed difference from the one
  before. *Corrected in G7:* when no trail fits the room left, the part
  waits; it used to send its two counts with nothing in them, a few bits
  past the record's room.
- **Room.** The readout takes up to 200 bytes and leaves the rest to the
  entities; when it had more to say, it is coded again with whatever the
  entities left of their share. What still does not fit waits: removals
  first, then new entries, then the largest changes go, and the client keeps
  the rest as the baseline predicts it, which the host's record of what the
  client holds does too, so the next packet catches up from there.
- **Starting again from empty.** *Found in B1:* when the client's newest
  acknowledgement is over 127 snapshots old (31 before protocol 16: a stall
  of the game, or a round trip over a second at 30 Hz or half a second at
  60 Hz; now a stall or round trip of over 2.1 seconds at 60 Hz), the record
  is against the empty readout, and
  in a busy fight what does not fit is empty on the client, not kept. The
  baseline stays so, as the host holds it, but the cockpit shows a scalar
  group that has not arrived since with what it last arrived with for the
  same plane (`ReadoutReceiver::presented`): every group but the AI locks
  and the painting radars always holds values, so an empty one has not
  arrived (the air-to-ground groups of protocol 23 start with a presence
  value for this reason: a bomb sight that went away arrives as "none", not
  as an empty group, which would show the last one). Before this the
  radar page read the sensor flags of such a readout as `NOT INSTALLED`.
  The lists cannot be told apart from an empty list, so they show what came:
  with acknowledgements always over the window late, a busy fight's
  contacts, strobes, plots, trails, visual and map contacts and the link's
  lists do not reach the client. *Built in protocol 16 (slice B2):* so the
  window grew from 31 snapshots (0.52 s at 60 Hz) to 127 (2.1 s), for the
  readout and the entity records alike, enough for a 600 ms round trip and
  a stall of about a second and a half on top. The host keeps 128 sent
  snapshot packets per connection and the client 160 snapshots of readouts
  and entity states, as for the entities above. *Agent decision:* 7 bits,
  the brief's suggestion: 6 (63 snapshots, 1.05 s) would not cover a 600 ms
  round trip with a stall, and 8 (255, 4.25 s) doubles the memory for
  stalls long enough that a full restart costs little next to them. The
  field costs 2 bits more on every record, and a delta against an older
  baseline is bigger than against a fresh one (the figures are in the
  [architecture's B2 row](../ARCHITECTURE.md#how-stage-g-lands)).
- **Steps.** Scope, visual, map, threat and inbound positions in whole feet
  and velocities in 1/4 ft/s; target rows and seeker observations in 1/32 ft
  and 1/64 ft/s, as the entities; plot and strobe angles 2^-12 of a turn;
  emitter bearings 2^-8 of a turn (1.4 degrees), strengths 1/32 and
  distances 1/8 nm, as coarse as the warning receiver draws them; threat
  bearings 1/4 degree; qualities, strengths and floors 1/1024; small angles
  1/4096 rad; ranges whole feet; nautical miles 1/64; seconds 1/64.
- **Not sent.** A contact's bearing, elevation and distance, which the client
  works out around its own predicted plane; the tower's reply and landing
  count. The tower's service travels as its selected airport, its clearance
  and the objects out of action; the client rebuilds a service that answers
  the ILS as the host's does (`tore_sim::airport::Service::presented`). A
  strobe is rebuilt without its line of sight, which only the host's sensors
  use (`Strobe::presented`).
- **Order.** Lists come back in id order, and a list holds one entry per id.

#### The gunsight

*Built in protocol 21 (gunsight slice S4); agent decisions unless credited.*
The AC-130's sight is the host's ([the spec](../spec/ac130-linked-guns.md)):
the seat sends its slew and zoom step in every Inputs frame, the host's
combat step turns the look, resolves Backslash and L, trains the guns and
works out each gun's pipper, and the owner's readout carries the result in
its gunsight scalar group. The group is empty on every other aircraft. Its
37 values, in order:

| Values | Meaning |
| --- | --- |
| 2 | The sight's mode (0 free, 1 pinned, 2 tracked) and the tracked object's id (0 otherwise) |
| 3 | The pin, as an offset from the aim point in 1/8 ft (0 unless pinned; a pin is its own aim point, so it costs three bytes) |
| 2 | The look: the camera's body-relative heading and elevation from sensor dome D, in steps of 2^-20 of a turn. The elevation is never above 0: the camera's gimbal is the hemisphere below the aircraft, so a target or pin above it leaves the look at the limit while the aim point stays true |
| 1 | Travelling back to the default view |
| 4 | The aim point: present, then its position in 1/8 ft |
| 3 x 6 | Each gun's pipper by source slot (25 mm, 40 mm, 105 mm): kind (0 none, 1 ground, 2 air, 3 spent), its point as an offset from the aim point in 1/8 ft, its flight time in 1/64 s and its range in whole feet |
| 1 | The pippers' launch tick, as ticks before the readout's |
| 3 | Each gun's readiness, in the readout's readiness codes (24 is TERRAIN MASK) |
| 2 | The last notice (0 none, 1 no ground point, 2 drop to slew, 3 gimbal limit) and its tick, as ticks before the readout's. The gimbal limit is raised every tick the camera is held at the edge of its gimbal, so its tick stays fresh while it holds |
| 1 | The zoom step, 1 to 6 |

*Agent decisions:* the plan asked for points relative to the aircraft, but
the readout does not carry the host's own position, and the client's
predicted plane is ahead of it; so the aim point is absolute and everything
else rides as an offset from it, which keeps the pippers to two or three
bytes a coordinate. 1/8 ft is a quarter of a target camera pixel at the
narrowest zoom 7,000 feet away, and 2^-20 of a turn (6 microradians) is far
under a pixel. Whether a readiness only advises (the trigger still fires)
is the readiness's own (`Readiness::gun_may_fire`), so it needs no bit. A
reader refuses a group of another length, an unknown mode, impact kind,
readiness or notice, and a zoom step outside 1 to 6.

Remote players need none of this: an AC-130's barrels travel in its entity's
gun devices (protocol 18), so a second player sees them follow the sight.

**The client turns the camera itself** (`tore_session::client::sight`). The
host's look arrives a round trip late, so the client turns its own copy with
the sim's own law (including the gimbal clamp: no predicted, corrected or
drawn look is ever above the horizon) from the same quantized frames its
prediction steps: a free
sight slews or travels home after a predicted L; a pinned one looks at the
host's pin from the dome's place on the predicted plane, turned by the slews
the host's pin does not hold yet; a tracked one holds the host's look, since the target camera
frames a track by itself. When a newer readout arrives, its look (the sight
after the input of the tick before the readout's) is compared with the
client's own for that tick; when they differ by more than 10 microradians
(the wire's step is 6), the client starts again from the host's look and
steps the frames it sent since. A change of over a tenth of the field of view
snaps; a smaller one slides away with a 50 ms time constant, counted in ticks
so a capture replays it the same. The flight frame's readout carries the
client's look in place of the host's. The pipper, the gun marks and the
status stay the host's, a round trip late, like the barrels. In the network
tests (`client::sight_tests`) a gunner slews, pins, fires and slews the pin.
Over 80 ms with no loss the camera never snapped and slid once, when the
pin arrived. Over 300 ms with 5 percent loss each way the slews never
corrected and the pin snapped once: the client cannot predict the pin, and
by the time it arrives the plane has flown a round trip on, so the view
swings to the pin by more than a tenth of the field of view.

Measured on the 15 against 15 mission with 100 ms acknowledgements: the
readout's plain size is 471 to 13,145 bytes, mean 1,895; its record is 4 to
506 bytes, mean 52 (the visual contacts 14 bytes, the map 10, contacts 8,
emitters 7, threat records 6), with 2.7 changes waiting on average, mostly in
the first second while the map and scope come across. Keeping no room for
messages, 61 bytes on average.

#### The air-to-ground groups

*Built in protocol 23 (slice NA); agent decisions unless credited.* The
air-to-ground round's sights and cues are the host's ([the
spec](../spec/air-to-ground.md)): the host runs every solution, and the
owner's readout carries what the HUD, the targeting page and the scopes draw,
so a client shows the cues the host built. Seven scalar groups (15 to 21),
each starting with a presence value (0 none, 1 present) so a part that went
away reads as none:

| Group | Values after the presence value |
| --- | --- |
| Targeting sight (15) | 15: the mode (0 free, 1 pinned, 2 tracked) and the tracked id; the pin as an offset from the aim point in 1/8 ft; the look (heading, elevation) in 2^-20 of a turn; travelling home; the aim point (present, then 1/8 ft); the notice (0 none, 1 no ground point, 2 drop to slew, 3 gimbal limit) and its tick before the readout's; the zoom step 1 to 6 |
| Sight sensor (16) | 10: laser (or FLIR); reach in whole feet; the gimbal (0 the hemisphere, 1 a cone with its search and track half-angles in 2^-20 of a turn); the eye in the aircraft's frame in 1/8 ft; the default look. A group of its own because it changes only with the plane or its pod |
| Laser (17) | 26: designator; the state (0 off, 1 ready, 2 lasing, 3 masked); the mask (0 none, 1 range, 2 gimbal, 3 terrain); the hand setting (0 the automatic rule, 1 on, 2 off); the range and the reach in whole feet; then the spot and the buddy's spot, 10 values each (present, owner plane, side, position in whole feet, target present and id, own, range) |
| Bomb sight (18) | 36: the mode (0 CCIP, 1 LVL), rocket; the CCIP (present; the mean impact in 1/8 ft, the release point as an offset from it, the release velocity in 1/64 ft/s, the fall's seconds in 1/64 s, the arc's range, the slant range and the record's zone-1 maximum in whole feet, the arc's steps 0 to 15; body mass in slugs, drag area and retarder area in square feet, each in 1/16777216 units, steering class 0 to 2 and guidance duration in 1/64 s); the level release (present; the target in 1/8 ft, D and C in whole feet, the seconds to release, the steering in 1/4096 rad, the slant range and the release radius in whole feet, the ground speed in 1/64 ft/s); the warnings as a mask (1 LOW, 2 FRAG, 4 OVERFLY, read back in that order); the last bomb's fall (present, seconds) |
| Guided sight (19) | 18: the guidance (0 supported, 1 active, 2 infrared, 3 emitter, 4 laser, 5 coordinate, 6 command); the aim (present, whole feet); buddy; the ground and slant ranges (present and range each); the reach (present, minimum, maximum); IN RNG; the advisory (0 none, 1 NO LASER, 2 MASK, 3 OUT OF REACH, 4 NO TARGET); the level T (present, steering 1/4096 rad, ground speed 1/64 ft/s) |
| HARM cue (20) | 3, then 6 an emitter: the candidate (present, id), CUED; each emitter in the cue's order, nearest the bore first (id, position in whole feet, painting, emitting), at most 8 |
| Weapons in flight (21) | 2, then 9 a weapon: the tick the plane's last guided weapon ended (present, ticks before the readout's); each of its guided weapons in flight, oldest first, at most 4 (projectile, station, guidance, guiding, seconds to impact present and in 1/64 s, position in whole feet). Never empty, so it has no presence value |

The ground pages ride two values at the end of the Sensors group (the A-G
flag and whether the HARM page is there) and the **ground contacts** list
(list 16, at most 50): each contact as the scope's (the channel, 0 to 3 with
the HARM page, position and velocity in whole feet and 1/4 ft/s, eligible and
destroyed), then its class word and the aircraft and visible flags. Its
position and velocity go as differences, not predicted, since the host
refreshes a listed position four times a second and holds it between. The
client lists them nearest its own plane first, as the host does.

*Agent decisions:* points the HUD draws close to the plane (the sight's aim
and pin, the bomb sight's impact, release and target) are 1/8 ft, as the
gunsight's; points drawn far off or only as text (laser spots, the guided
aim, the HARM cue's emitters, the weapons in flight) are whole feet, as the
scope's. The readiness codes 25 (`NoEmitter`, NO EMITTER) and 26
(`TrackLost`, NO TRACK) ride the Estimates group's readiness as any other.
A reader refuses a group of the wrong length, an unknown mode, state, mask,
setting, guidance, advisory or notice, a zoom step outside 1 to 6, an arc
over 15, a warnings mask over 7, and a HARM cue or a weapons list over its
limit.

**On the client.** The cues are drawn from the host's solution a round trip
late, as the gunsight's pipper is: the CCIP pipper, the LVL marker, the
reach scale and the laser box sit where the host put them for the readout's
tick, around the client's predicted plane. The targeting sight's look is
the host's (the AC-130's own client-side turn, [above](#the-gunsight), is not
extended to it: the targeting page's camera smooths the look, agent
decision). The weapons themselves are projectile entities, as every missile
is: a falling bomb, a pack's bombs, each rocket of a pod and a guided weapon
steering on its spot travel as the host steps them (their scatter and
guidance are the host's), and the client's interpolation draws them; no
event and no client-side motion was needed (agent decision: unlike the
surface round's gun bursts, they are few and not repeated a round a tick).
A guided weapon's `target` stays none.

Measured with `a_laser_bombers_readout_comes_through_a_connection_as_the_host_built_it`
(the synthetic F/A-18D with a laser pod and two laser bombs, the sight
pinned and the laser set by hand, a bomb guiding): the readout the client
reads codes to the host's own numbers on every snapshot.

### Events as built

The section is the count (varint, 1 to 1,024), the first event's number (16
bits), then each event: its number after the first as the difference from the
one before less 1 (bucketed as the ids), its ticks before the snapshot
(varint), a 5-bit code in the table's order (Message 0 to Sound 16, Link
17 since protocol 15, the surface events 18 to 20 since 22, City flattened 21
since 25 and Stores 22 since 27) and its fields. Stem lists are a 6-bit count (at most 32) and 12-bit name indexes. A
rumble's turbulence is 1/255; a gun burst starts at the event's tick and
carries its length as a varint, 0 while still firing; a countermeasure
carries its release position, velocity and attitude, the device's number
(which chose its look) and the owner's devices of that kind left; a radio
call carries a bit for "important" (never silenced) beside its route, an
addition, and since protocol 15 a bit for the battle net after it. The client drops repeats by number (it remembers 4,096) and holds
an event that names a table entry whose Names message has not arrived, with
the events after it, until it does.

### Own state as built

The section is the state's tick (32 bits), its number (16 bits, counting the
connection's own states), its baseline as own states back (5 bits, 0 for
none), then the exact state's own coding and zero padding. The host codes
against the newest acknowledged own state no more than 31 numbers back; the
client keeps its last 64.

### The client as built

*Settled by the client (D8a)*, each an agent decision:

- An Own state section that arrives before the Seated message is accepted
  when it has no baseline or names one the client kept, and is read once the
  seat arrives: the host may already have coded the next against it. (The
  host holds a seat's exact states until its Seated message is acknowledged,
  from D10, so a client of this build never sees one early; a client still
  tolerates it.)
- The view offset is the newest tick less the drawn time's whole tick; the
  interpolation delay is the main delay rounded to whole ticks (a far
  entity's extra delay is not included).
- Commands are numbered from 1 and wrap through 0 as the host's buffer
  expects; a snapshot's "commands applied" acknowledges every number up to it.
- *EF4:* an Inputs section starts at the tick after the newest snapshot's
  as well as after the newest input the host had: the host has stepped the
  ticks before, so they would only arrive late and report a long delay in
  the input margin. A section of a flight's inputs is never sent before the
  Seated message of that flight.

### Messages as built

Kind bytes 1 to 11 in the table's order (Mission to Mission ended), then the
lobby's 12 to 22 in theirs (Slot to Flight loadouts); 23 and 24 (passing the
crown, the King's settings) and 27 to 36 are
[phase 2's](#phase-2-the-kings-settings-revival-scores-and-observers),
protocol 8. The
spec text and the exact state are long byte strings (a varint length); the
exact state in Seated is coded with no baseline and the client decodes it with
its plane's aircraft model. A loadout is the fuel as a 64-bit float, the
loadout screen's cheat bit, each station's weapon, count and quantity, and
since protocol 18 a presence bit and the tank list (at most nine, each a
hardpoint byte, the tank record's name and a 16-bit quantity; absent keeps
the aircraft's source tanks). A gun pod station's quantity counts pods, not
rounds, also in Seated. A roster plane is its id, side, wing (2 bits), place
in the wing, aircraft (6 bits since protocol 18) and pilot (the AI, or a seat and callsign). The Debrief mirrors the
game's debrief report field for field (the damage as a 64-bit float, the ten
kill rows and the eight shot tallies). Mission ended carries its reason in 2
bits (every human left 0, time limit 1, server stopping 2, ended by the
server's operator 3, which slice D7 added under protocol 1 before anything
shipped) and the seconds to the
next mission, if any. *Protocol 3 (EF4):* the reason takes 3 bits and adds the
host left the game (4); the King's End mission is reason 3.

*Built (EF4), the lobby's messages, each an agent decision:*

- **Numbers.** The Mission message's number counts the lobby's missions on
  the host, from 1, raised with each King's change; a flight's start sends
  the same mission again (as Flight loadouts) under the same number. Take
  plane, Slot, Loadout and Set ready carry the number the player last
  received; one for another number is refused with "The mission has
  changed; choose again." (Take plane as a Seat refused, the others as a
  Refused).
- **Content refused** is the mission's number (varint), the names (a count
  and strings), the reason (a string) and a bit: the flight's loadouts
  failed, not the lobby's mission (the host clears that mark when the lobby
  returns). A refusal for an earlier number is logged and otherwise
  ignored.
- **Take plane** is the number (varint), then a presence bit and the plane
  (varint).
- **Slot** is the number, then 2 bits (0 take, then the plane as a varint;
  1 the first free; 2 leave; since protocol 20, 3 a side's first free slot,
  then the side as one bit, 0 Bluefor, the friendly side, 1 Redfor; see
  [side requests](#side-requests)). **Loadout** is the number, the plane (varints),
  a presence bit and the loadout as Seated codes one. **Set ready** is the
  number and a bit. **Change mission** is a long string. **Start** and
  **End mission** are empty. **Kick** is the player's id (8 bits) and the
  reason (a string).
- **Lobby** is the game's name and the mission's summary (strings), the
  number (varint), the phase (2 bits: lobby 0, flying 1, ended 2), the start
  rule (2 bits: the King's start 0, the first ready player 1, flying from the
  start 2), the King's id and the host's id (each a presence bit and 8 bits;
  none on a dedicated server), the receiving player's own id (8 bits), the
  players in the order they connected (a count, then each: id 8 bits,
  callsign, a presence bit and the slot's plane, ready, armed with its own
  loadout, flying (one bit each), a presence bit and why its import cannot
  play the mission, and, since protocol 7, its platform in 3 bits with the
  Challenge answer's codes, 4 to 7 invalid, since protocol 12 its connection
  path in 3 bits, the codes of [the path](#the-path-in-the-challenge-answer),
  6 and 7 invalid, and, since protocol 10, its
  Fighters Anthology build in 2 bits, as [Content](#content) codes it), the
  slots in plane order (a count, then each: plane
  varint, side 1 bit, wing 2, member 8, aircraft 6, a presence bit and the
  holder's id) and the King's settings (a count, then each a number of 8
  bits and a varint value; none in phase 1) and, since protocol 20 (the
  lobby pass's slice K1), one last bit: the mission is locked. A dedicated
  server whose file says `king-mission locked` sets it; a game a player
  hosts never does. The lobby screen then opens the King's Mission... read
  only, with "This server's mission is fixed.". The host's refusals of the
  King's mission and settings changes are unchanged. After the locked bit,
  one more optional field (protocol 52, nr-lobbyui): a presence bit and, when set, 4 bits of seconds left
  on the King's force start, 1 to 10. Absent (one 0 bit) when no countdown
  runs. The host owns the number. **Force start** (kind 55) is empty, from
  the King: it starts that countdown or cancels the one running.
- **Refused** is the request's kind (8 bits) and the reason (a string).
  **Goodbye** is 2 bits (kicked 0, then the reason as a string; the host
  left 1). **Flight loadouts** is a count, then each plane (varint) and its
  loadout as Seated codes one, then the manifest entries the flight's build
  read that the lobby's did not, coded as the Mission message's manifest; a
  player compares its own hashes of those names.
- **Pace.** The host answers at most 20 lobby requests a second from one
  connection (Leave and Content refused aside) and drops the rest
  unanswered; it sends a player the lobby state only after a change, at most
  every 250 ms in the lobby and every second in flight.

### Chat as built

*Built (EF6), protocol 4, each an agent decision unless credited.* Kinds 25
and 26 (23 and 24 stay free for the phase 2 lobby). Chat is reliable, rides
the same messages as the lobby, and shares their pace: a connection's
requests count against the 20 a second the host answers.

- **Chat send** is the receiver in 3 bits (All 0, Friendlies 1, Enemies 2,
  Wing 3, Target 4), the text (a string), and a presence bit; when set, the
  quick message's number in 4 bits (1 to 12) and a presence bit and the
  sound's name (a string). The text of a quick message is the line's, cut to
  `CHAT.TXT`'s 50 characters and with any character outside printable ASCII
  turned into `?` by the sender's game.
- **Chat line** is a bit (0 the host's words: then the text and nothing
  else; 1 a player's line) and, for a player's line: the sender's callsign
  (a string), where the sender stands to the reader in 2 bits (0 neutral: the
  sender or the reader has no plane, 1 the reader's own side, 2 the other
  side), a bit for the reader's own line sent back, the receiver in 3 bits,
  the text (a string), and a presence bit and the sound (a string).
- **The host's rules.** The text is trimmed; empty is dropped without a
  word; more than 80 characters, or any character outside printable ASCII
  (space to `~`), is refused. A quick message's number must be 1 to 12 and
  its sound at most 12 printable characters with no backslash, ending
  `.5K` or `.11K` in any case. A player may send five lines in five seconds
  (a sliding window, counted over the lines the host accepted); the sixth is
  refused and does not count. Every refusal is a line from the host to the
  sender alone, in words, and is noted in the log (once a second at most for
  the same words). A player with no plane (in the lobby, whether or not the
  mission flies) sends only to All.
- **Routing.** The host never sends a line to a connection that is closing.
  All goes to every other connection, in the lobby or flying, seated or not.
  Friendlies goes to the players flying a plane on the sender's side,
  Enemies to those on the other side, Wing to those in the sender's wing
  (side and wing index), Target to the human flying the aircraft the sender
  has designated (an AI-flown or non-aircraft target, or none, reaches no
  one). A player flying hears Friendlies, Enemies, Wing and Target lines;
  one with no plane hears All only. While a mission flies, a connection
  with no plane is an observer (John, 2026-10-01), and an observer's All
  reaches only the other connections with no plane (the lead's reading);
  a flyer's All reaches everyone. A player who left, or whose plane the
  AI flies again, hears nothing more. The sender is sent its own line back
  (no sound), so it sees what went out; when no one else heard it, the host
  follows with "No one hears you."; Target with nothing designated is
  refused ("You have no target designated.").
- **Sounds.** The receivers play a quick message's sound; the sender does
  not hear its own.

### Flights

*Built (EF4), protocol 3, agent decision.* A connection now outlives a
flight: a player who leaves its flight, or whose mission ends, stays in the
lobby and may fly again, and a new mission's ticks start again from 0. So
each seating starts a new **flight** of the connection, numbered by the host
from 1 (8 bits, wrapping): the host's wire state for the connection (entity
and readout baselines, the event queue and its numbers, the name table, the
own-state baselines) starts afresh, the Seated message names the flight,
and every Snapshot and Own state section, every Names message and every
Inputs section carries it. A client starts its own afresh at the Seated
message, at the first section of a later flight or at a Names message of a
later flight, whichever comes first (a packet's messages are read before its
sections, so a Names message the client dropped would leave the flight's
table short), and drops a section of an earlier flight that arrives late (an Own state
section of the client's flight must still name a baseline it has; one of a
later flight must name none). The host drops Inputs of another flight.
Reliable delivery keeps a Names message of the earlier flight before the
Seated message of the next, so the name table never mixes two flights.

## Phase 2: the King's settings, revival, scores and observers

*Designed 2026-10-05; built (F2-0) under **protocol 8**, all of it at once*:
the wire of stage F's phase 2
([architecture](../ARCHITECTURE.md#phase-2-the-rest-of-stage-f)), so the
slices after F2-0 never change the bytes. Every choice is an agent proposal.
Codings follow [what the game's sections
settled](#what-the-games-sections-settled): varints are `tore-codec`'s,
strings a length byte and UTF-8, "a presence bit" a 1 then the value or a 0
alone. The messages are `tore_session::wire::messages`, the settings'
registry `tore_session::settings`.

### New messages

Kinds 23 and 24, kept since EF4, and 27 to 36. Every request a player sends
counts against the 20 a second the host answers, and every refusal is a
Refused (kind 20) with its words.

| Kind | Name | Direction | Body |
| --- | --- | --- | --- |
| 23 | Pass crown | the King to host | The player's lobby id (8 bits) |
| 24 | Settings | the King to host | A count (varint, 0 to 64), then each setting's number (8 bits) and value (varint); a presence bit and the game's name (a string); a presence bit and the password: 1 bit (0 clear it, 1 set it, then a string of 1 to 255 bytes). Then a Realism presence bit; if present, Damage (2 bits: 0 normal, 1 invulnerable, 2 realistic), fifteen boolean flags in Cheats field order (unlimited ammo through easy physics, including guns only), and Enemy AI (3 bits: 0 unchanged, 1 novice, 2 average, 3 experienced, 4 ace). Invalid enum values are refused. The host applies all or none: the first that fails refuses the whole message |
| 27 | Slot lock | the King to host | The mission's number (varint), the plane (varint), the lock (2 bits: 0 open, 1 closed, 2 reserved) and, when reserved, the callsign (a string) |
| 28 | Revive | client to host | Mission number (varint), optional aircraft and optional loadout: fly again after a loss with an atomically validated choice |
| 29 | Revival | host to client | The seat's plane is lost: the rule (2 bits: 0 none, 1 AI slot, 2 revive), the lives left (1 bit unlimited, else 4 bits, 0 to 10), the seconds until it may fly again (varint), and a presence bit and a line of why it waits or cannot ("No lives left.", "Waiting for room for another aircraft.") |
| 30 | Spawned | host to every player | A revival's new plane, which every client adds to its copy of the mission: the plane (varint), the tick (32 bits), its wing (side 1 bit, index 2 bits), its member (8 bits), its aircraft (6 bits, the roster's index), its position (three 64-bit floats, feet), heading and speed (64-bit floats, radians and feet a second), and its loadout as Seated codes one |
| 31 | Scores | host to client | Tally (2 bits: kills 0, damage 1, ratio 2), fight (1 bit: sides 0, free for all 1), optional seconds left (presence bit, varint), kill limit (4 bits, 0 for none), kill owner (2 bits: total 0, side 1, individual 2). Combatants: count varint, at most 1,024; each has lobby id 8 bits, callsign string, left bit, optional AI lineage root (presence bit, varint u32), optional side (presence bit, side bit), then kills, losses, damage in thousandths of an aircraft, and tracked hits (four varints). Each side has kills, losses, damage and hits (eight varints total). Winner: 3 bits, 0 none, 1 side then side bit, 2 human then lobby id 8 bits, 3 AI then root varint u32, 4 draw; 5 to 7 invalid |
| 32 | Results | host to client | Sent once at the mission's end. The end's reason (3 bits, as Mission ended), the rows (a count as a varint, at most 1,024; each: plane varint, side 1 bit, wing index 2 bits, member 8 bits, aircraft 6 bits, a presence bit and the callsign of its last human pilot, a left bit, status 2 bits (alive 0, ejected 1, dead 2, retired 3), damage in thousandths (10 bits), aircraft killed, other kills, friendly fire, then air-to-air launched and hit, gun launched and hit, air-to-ground launched and hit, each a varint), then a presence bit and the final Scores coded as kind 31 |
| 33 | Observe | client to host | 1 bit: 0 stop watching; 1 watch, then the subject (2 bits: 0 none, 1 an aircraft then its id as a varint, 2 a point then x, y and z in whole feet as signed varints) |
| 34 | Observing | host to client | 1 bit: 0 the observer flight has ended (nothing follows); 1 it starts: the connection's new [flight](#flights) (8 bits), the delay in seconds (8 bits), the tick the first snapshot will show (32 bits), the roster as the Roster message codes it, and the destroyed ground objects (a count and varints) |
| 35 | Away | client to host | Nothing: the game has been away (a menu, no focus, a lost controller) for the `idle-ai` setting's seconds |
| 36 | Back | client to host | Nothing: the player touched the flight controls; take the plane back |

A Pass crown, Settings or Slot lock from anyone but the King is refused "Only
the King may do that." Revive and Back are answered by a Seated message (a new
flight) or a Refused. A Slot lock or a Revive that names an earlier mission is
refused "The mission has changed; choose again.", as the lobby's requests are.

*Built (F2-0), each an agent decision:*

- **Not built yet.** Until the slice that builds a request lands, the host
  answers it with a Refused "Not available yet." (the King's three after the
  King check), and a client passes each new host message on as an event
  without acting on it. Pass crown, Settings and Slot lock are built since
  F2-1 ([the King's lobby](../ARCHITECTURE.md#the-kings-lobby-as-built-f2-1)),
  with their refusals in words.
- **Settings.** The count may be 0, for a change of only the name or the
  password (the design said 1 to 64). An empty password is refused by the
  writer and the reader. Setting 5 is refused by number: the password has its
  own field.
- **Revival.** Lives over 10 are refused by the writer and the reader (4 bits
  would hold 15); the rule's code 3 is invalid.
- **Spawned.** The aircraft is the roster's 4-bit code; the floats are the
  world's own (`tore_world::world::revive::Spawn`): position x, y, z in feet,
  then heading and speed.
- **Scores.** The sides are the friendly side's three varints, then the
  enemy side's. The kill limit's 4 bits hold 0 to 15; the registry's values
  are 0 to 10. A code 3 for the tally or the kill owner is invalid.
- **Results.** Damage over 1,000 thousandths is refused by the writer and the
  reader. A row's three shot tallies are air-to-air, then gun, then
  air-to-ground, launched before hit. The largest message (1,024 rows and 64
  scored players) fits a reliable message.
- **Observe.** A subject's code 3 is invalid; a point's coordinates are
  signed varints that must fit 32 bits.

*Built (F2-A), each an agent decision:*

- **Away** is taken from a seated player whose plane is not lost, while the
  mission flies and the `idle-ai` setting is not never; the plane goes to
  the AI at the next tick, and the connection's observer flight (Observing,
  its own plane the subject) starts at that tick: a flying game learns from
  that Observing message that the AI took its plane, whether it sent Away
  or the host counted the setting's seconds with no input from it. Away
  from a player already away is taken and does nothing. The refusals: "The
  mission is not flying.", "The AI flies no idle aircraft in this game.",
  "You are not flying." and "Your aircraft is lost: the AI cannot fly it."
- **Back** from an away player is answered by Observing's end and then a
  Seated message for the same plane (a new flight); Back before the tick
  that would have handed the plane over cancels the Away. The refusals: "The
  AI is not flying your aircraft." and "The AI lost your aircraft while you
  were away." (also sent as a Notice when the loss comes first).
- **The lobby state's away mark** is set while the AI flies a plane kept for
  the player, from the handoff's tick to Back's tick, the reservation's end
  or the mission's end ([how it works](../ARCHITECTURE.md#the-ai-flies-an-idle-players-aircraft)).

*Built (F2-D), each an agent decision:*

- **Results** goes once at the mission's end to every connection that is not
  closing (observers and players in the lobby too), after the final Scores
  and before Mission ended and each player's Debrief. The rows are every
  plane of the mission, retired ones included, friendly side first and by
  wing and member. A row's callsign is the last human who flew the plane,
  kept after the player leaves. Aircraft kills are the kill table's first
  three rows, other kills the rest; friendly fire counts by side; the
  air-to-ground shots hold missiles and bombs together. A retired plane's
  damage is 1,000. The final Scores ride along in PvP and are absent in
  co-op. A client keeps the newest until the next mission or flight.
  ([how it works](../ARCHITECTURE.md#the-multiplayer-debrief))

*Built (F2-V), each an agent decision:*

- **Revival** goes to a seated player at the tick the host finds its plane
  lost, and once more, with the line "Waiting for room for another aircraft.",
  when a revival it asked for waits for room. The line is "No revival in this
  game." under `none` and "No lives left." with none left; the lives the
  registry allows (0 to 10) are all the field carries.
- **Revive** from a player with no lost plane is refused "Your aircraft is not
  lost." (seated) or "You have lost no aircraft; press Join to fly." (in the
  lobby); the rules' refusals are listed in the
  [architecture](../ARCHITECTURE.md#death-revival-and-lives). A Take plane
  (Join) from a player in the lobby whose plane is lost is the same request,
  answered by Seated or a Seat refused.
- **Spawned** goes to every connection at the revival's tick, before the
  reviving player's Seated, so each copy of the mission holds the plane the
  Seated names. A player who joins later is sent every spawned plane still in
  the mission after the Mission and Roster messages. Its tick is the tick of
  the revival (the Seated's tick).

### Changed messages

- **Lobby** (kind 19). The settings list, empty until now, carries every
  setting of the [registry](../ARCHITECTURE.md#the-kings-settings) by number
  (the password as setting 5, 1 when one is set; the password itself never).
  The house id (the field the lobby state calls the host's) and the King's id
  now differ when the crown has passed. Each slot gains its lock (2 bits, after
  the holder: 0 open, 1 closed, 2 reserved, 3 invalid) and, when reserved, the
  callsign (a string). Each player gains two bits after flying: observing (it
  watches the flying mission) and away (the AI flies its plane while it is
  away). *Built (F2-0):* the host sends all twenty settings in number order
  from its store, which starts at co-op's defaults with the configuration's
  name, password, player limit and time limit (a dedicated server's time limit
  as its file gives it, which may lie outside the King's list); every slot is
  open and nobody observes or is away until slices F2-1, F2-O1 and F2-A.
  *Since F2-1* the settings are the King's and a server's file's, each slot
  carries its lock, and the house id is the house's (none on a server).
- **Mission ended** (kind 11) and Results: reason 5 is the kill limit (3 bits
  already).
- **Inputs**: command code 22 is a **wing reply**, followed by its kind in 2
  bits (Engaging 0, Winchester 1, Bingo fuel 2, Need help 3).

### Observer flights

An observer's connection has a flight like a seated one: Observing starts it,
and its Snapshot sections carry its number, with **no own state hash** (the
header's presence bit 0, as before seating), no cockpit readout (its bit 0),
and zero for the input fields. No Own state section and no Inputs section
belong to it; its Events section carries only mission-wide events. The
entities are coded as for a seated player, with the relevance of the observer's
camera ([architecture](../ARCHITECTURE.md#the-observer-view)). With a delay,
the snapshot's tick is the delayed tick, never newer than the host's tick less
the delay. Its snapshot ticks are its lobby id modulo the ticks per snapshot.

*Built (F2-O1), each an agent decision:*

- **With a delay** every observer's snapshot ticks are the delay ring's: the
  ticks whose number is a multiple of the ticks per snapshot (the delay is a
  whole number of seconds, so the host sends them at such ticks too). The
  lobby-id phase above holds without a delay.
- **Observing's tick** is the tick the first snapshot shows; with a delay, and a
  ring younger than the delay, it can be the mission's first frame, sent once
  the delay has passed. Its destroyed ground objects are those destroyed by the
  ring's tail with a delay, and by now without.
- **Events.** The Events section carries the mission-wide events and the AI's
  wing ejections. Each one's tick is no later than the snapshot's; with a delay
  an event goes out once the stream's tick has reached the tick it became
  known (a finished gun burst's tick is its first round's, before that).
- **Order.** Observing's end comes before the Seated message of a plane the
  observer takes, and before Results and Mission ended at the mission's end.
- **Refusals** of Observe are Refused (kind 20) with "The mission is not
  flying; watch once it flies.", "Leave your aircraft before you watch." or
  "There is no plane 12."; a Stop with no watch is ignored. Camera changes
  sooner than half a second after the last wait, and the newest is applied
  when the half second has passed.

### Settings by number

The registry's numbers, which the Lobby and Settings messages carry, with each
value's coding. The names are the configuration file's and the logs'.

| No. | Name | Value |
| --- | --- | --- |
| 1 | `mode` | 0 co-op, 1 PvP |
| 2 | `max-players` | 1 to 30 |
| 3 | `join-in-progress` | 0 off, 1 on |
| 4 | `visibility` | 0 hidden, 1 local, 2 public (a game a player hosts; a dedicated server refuses it) |
| 5 | `password` | 0 none, 1 set (in the lobby state only; Settings carries the text) |
| 6 | `friendly-fire` | 0 off, 1 on |
| 7 | `lock-sides` | 0 off, 1 on (locked once flown), 2 balanced (the host picks the sides; protocol 20). PvP only since protocol 20 |
| 8 | `loadouts` | 0 own, 1 any |
| 9 | `respawn` | 0 none, 1 AI slot, 2 revive |
| 10 | `lives` | 0 to 10, 255 unlimited |
| 11 | `revive-delay` | seconds: 0, 60, 120, 180, 240 or 300 |
| 12 | `revive-distance` | nautical miles: 1, 5, 10, 20, 40, 50, 75, 100 or 150 (50 to 150 since protocol 20; John, 2026-10-09) |
| 13 | `revive-weapons` | 0 missiles, 1 no missiles, 2 guns, 3 half guns |
| 14 | `fight` | 0 sides, 1 free for all |
| 15 | `tally` | 0 kills, 1 damage, 2 ratio |
| 16 | `time-limit` | seconds: 0 none, 60, 300, 600, 900, 1,200 or 1,800 from the King; a dedicated server's file may give any whole minute up to 10,080 (a week), which the lobby state carries as given |
| 17 | `kill-limit` | 0 none, 1, 2, 3, 5, 7 or 10 |
| 18 | `kill-owner` | 0 total, 1 side, 2 player |
| 19 | `observer-delay` | seconds: 0, 10, 30 or 60 |
| 20 | `idle-ai` | seconds: 0 never, 60, 120, 300 or 600 (default 300; John, 2026-10-06, slice F2-O4; no wire change: the value is the same u32, only its list and default moved) |
| 21 | `host` | 0 calculated, or 1 plus the lobby id of the player the King pinned (stage K, [designed](#host-migration-and-rejoin-stage-k)) |
| 22 | `snapshot-rate` | snapshots a second: 60, 30 or 20 from the King of a game a player hosts (default 60); a dedicated server's file may give 10, 12, 15, 20, 24, 30, 40 or 60, which the lobby state carries as given and the King may not change (slice R1, protocol 17: [what the rate sets](#connecting)) |
| 23 | `ai-respawn` | 0 off, 1 on (default on in both modes): a lost AI aircraft respawns under the revival rules; no effect while `respawn` is none (protocol 20; John, 2026-10-09; the respawn itself is the lobby pass's slice R1) |
| 24 | `ai` | 0 off, 1 on; default on. Lobby-only. Off suppresses AI aircraft, AI respawn and idle handoff. The same ready-player minimum applies; unfilled slots remain empty. |
| 25 | `min-players` | 0 (none) to 30: ready slot holders required before mission start, with AI on or off. The server file, rotation or a permitted King sets it; a mode change retains it, capped at capacity. Initial defaults: AI on 0, AI-off co-op 1, AI-off PvP 2. [Rules and UI](../spec/multiplayer-menus.md#minimum-players). |

A number the host does not know, or a value outside its list, is refused with
the setting's name and its values. A setting marked PvP only is refused in
co-op with "`name` applies only in PvP."

### Side requests

*Built (the lobby pass, slice W0), protocol 20; agent decisions unless
credited.* A PvP lobby's Bluefor and Redfor boxes (John, 2026-10-09) send a
Slot with request 3 and the side. A player's side is the side of the slot it
holds; one with no slot has none, so the lobby state's slots say every
player's side and no new message carries it.

- **The slot.** The host gives the side's first free slot in plane order,
  its first wing's lead first, so the first humans on a side lead its
  flights. A slot is free when no other player holds it and the King's lock
  allows this player (a slot kept for the player is free to it alone); while
  the mission flies it must also be flown by the AI for nobody. A request for
  the side the player holds already keeps its slot.
- **Leave** frees the slot and with it the side.
- **Under Autobalance** (`lock-sides balanced`) the host seats every player
  itself, with no message of its own: the lobby state's slots show it. A
  request for the player's own side keeps its slot; the other side, a Take
  of the other side's slot and a Leave are refused "Autobalance picks the
  sides." (slice A1; the rule is in
  [Autobalance as built](../ARCHITECTURE.md#autobalance-as-built-a1)).

Refusals are Refused (kind 20). The refusals every slot request shares (a
player flying, a player whose game cannot play the mission, an old mission
number) come first, as before; then, in this order:

| When | Words |
| --- | --- |
| A co-op game | "Sides are chosen only in a PvP game." |
| `lock-sides balanced`, and the side is not the one the host gave the player | "Autobalance picks the sides." |
| The player holds a slot on the other side | "Leave Bluefor first." or "Leave Redfor first." (it leaves, then joins: John's D3) |
| The side has no slots players may take | "Redfor has no slots players may take." |
| In flight, with the side fixed by lock sides | "Sides are locked until the mission ends." |
| The side has no free slot | "Bluefor is full." or "Redfor is full." |

## Compatibility (stage L)

*Designed 2026-10-05 for stage L; agent proposals awaiting John's review.
Built (L2, 2026-10-05) under **protocol 10**, kinds 37 and 38.* The bytes
that let every player know, before a mission is built, which aircraft,
theaters and weapons every human can use, and which Fighters Anthology build
each imported. Why and how the game uses them is in the
[architecture](../ARCHITECTURE.md#compatibility); the content items and their
digests are defined there. Everything here came in one protocol version, 10,
with two new message kinds, 37 (Content) and 38 (Content gaps). The messages
are `tore_session::wire::messages` (`Content`, `ContentGaps`, `Build`,
`ItemKind`), their limits `messages::content_limits`. Codings follow [what the game's sections
settled](#what-the-games-sections-settled): varints are `tore-codec`'s,
strings a length byte and UTF-8, "a presence bit" a 1 then the value or a 0
alone.

### Content

Player to host, reliable. The player's content, sent once as its first
message after Accepted.

| Field | Coding |
| --- | --- |
| Build | 2 bits: 0 unknown, 1 Fighters Anthology 1.0 (the disc), 2 1.02F; 3 is invalid |
| Importer | A presence bit, then the version and the commit of the T.O.R.E that made the import (two strings, each at most 64 bytes) |
| Items | A count (varint, 1 to 1,024), then each item: its kind (2 bits: 0 aircraft, 1 theater, 2 weapon, 3 shared), its key (a string of 0 to 32 printable ASCII bytes: the selection key, the theater code or the weapon record's name; empty for the shared item and only for it) and its digest (64 bits) |

- Items are sorted by kind and then key, with no key twice in one kind, so
  two equal contents code to equal bytes; a reader refuses any other order.
  At most one shared item.
- An item the player's import cannot load is left out: the player does not
  have it.
- A second Content on the same connection replaces the first (a re-import
  restarts the game, so it does not happen today; the rule keeps the host
  simple). It counts against the 20 requests a second the host answers.
- About 3 KB for 14 aircraft, 16 theaters and about 135 weapons: a dozen
  fragments, once per join.

### Content gaps

Host to every player, reliable. The items not every human can use. Sent to
every player when a player's Content arrives and whenever the gaps change (a
player joins or leaves); each message replaces the last.

| Field | Coding |
| --- | --- |
| Host build | 2 bits, as Content's: the build the host's own import came from |
| Host importer | A presence bit, then the version and commit (two strings, each at most 64 bytes) |
| Gaps | A count (varint, 0 to 1,024), then each gap: its kind (2 bits) and key (a string), as Content codes them; its label (a string of at most 64 bytes: the host's name for the item, or empty when the host lacks it); a bit, 1 when the host lacks the item; and the players who cannot use it (a count, 0 to 64, then each: the lobby id, 8 bits, and a bit: 0 lacks it, 1 has it with another digest) |

- A gap the host lacks may name no player. Any other gap names at least one.
- Gaps are in Content's order. A player's game words every line from these
  and the lobby state ([the words](../ARCHITECTURE.md#the-words)).
- Usually empty: a few bytes.

### The lobby state

**Lobby** (kind 19): each player gains its Fighters Anthology build, 2 bits
after the platform, coded as Content's build. A player whose Content has not
arrived yet reads 0 (unknown). *L5, 2026-10-06:* the game's screens no longer
show the build (John: the audit found no difference a player sees). The field
stays on the wire and `LobbyPlayer::build` stays filled, unused by the lobby;
removing it would need a new protocol version, so it waits for one.

### What does not change

- **Content refused** keeps its bytes. Its reason is now worded by item, and
  the host words the player's `unable` text in the third person before the
  lobby state carries it ([the words](../ARCHITECTURE.md#the-words)).
- **Refused** carries the host's refusal of a Change mission or a Loadout
  that uses an item in a gap, in the same words.
- The handshake, the discovery packets and the master's protocol are
  unchanged: another T.O.R.E build is still refused with code 2, and a
  missing aircraft is never a refusal at the door.

#### Rejoin as built (K5)

*Built (K5, 2026-10-05), no wire change.* Every choice is an agent decision.

- **Token (39)** goes to every connection right after its join, and again to
  a game that rejoins (the same token). Its life is 86,400 seconds.
- **A token in the Challenge answer** that works (kept, not voided, not past 24
  hours since its player was last connected) is admitted whatever the room;
  the password and the build are still checked. A token that does not work
  does not refuse the join: the game is a new player and gets a Notice, for
  example "Your rejoin token has expired: it lasts 24 hours after you were
  last in the game. You join as a new player."
- **Rejoin (54)** is answered with the Token and a Notice ("Welcome back,
  Viper: your aircraft is waiting.") or a Refused in words: the token's own
  words, or "You are already in the game." from a connection that flew, holds
  a lobby slot or is not in the lobby. The player's own token again is
  accepted and changes nothing.
- **Release (53)** from the King frees the reservation on that plane: a
  Notice to the player if connected; "No aircraft is kept for a player on
  that plane." when none.
- **Taking the plane back** is the player's TakePlane (Join) in flight, not a
  host message: the host answers with Seated at the next tick. A connection
  that has a reservation is seated in it whichever plane it asked for.
- **A returning player whose plane was lost** gets a Notice and a Revival
  message, so its game shows the revival prompt; Join then flies it again by
  the King's rules, in a new plane of the lost one's wing.
- **The journal** codes the command that revives a seat from a plane nobody
  holds (`MissionCommand::ReviveLost`) as mission command variant 5, after
  Revive's 4; no stream record, message or packet changed.
- **AI respawn** (the lobby pass's slice R1, under protocol 20): the journal
  codes `MissionCommand::Respawn { root, spawn }` as mission command variant
  6 (the lineage's root plane, then the spawn as Revive codes it); the
  revivals part gains the AI lineages (by root: respawns used, the loss tick,
  whether the log was told) and every lineage's original spawn (position and
  heading); the checkpoint's revival section gains each added plane's root.
  Every connection is sent Spawned (message 30, unchanged) for an AI respawn
  as for a revival.
- **The lead hold** (the lobby pass's slice R2, under protocol 20): the
  journal codes `MissionCommand::LeadHold { on }` as mission command variant
  7 (one bit) and `MissionCommand::LeadLeft { owner }` as variant 8 (the
  owner's kind as a varint, 0 a seat and 1 an away player's plane, then the
  seat or the plane); the checkpoint's revival section gains the hold and its
  owners, and the AI wings section the claims, the wingmen waiting to re-form
  and each leader's stand-in flag. No message, packet or golden sample
  changed: leadership stays host-side.
- **Lineages, objectives and callsigns** (the lobby pass's follow-up F1,
  under protocol 20, no bump): the journal codes
  `MissionCommand::Callsign { seat, callsign }` as mission command variant 9
  (the seat, then the callsign as a string); the host gives it before the
  command that seats a player whenever the mission does not know that name
  for the seat yet. The checkpoint's roster section gains the callsigns, the
  revivals part the players waiting for a lineage's respawn (by join order,
  each with the lineage's root). A late joiner whose slot's plane is lost
  takes the lineage's newest plane, or, while the AI is about to respawn it,
  is sent a **Notice** ("Plane 3 flies again in 0:45: you take it then.") and
  no Seat refused: its game stays joining, and Seated follows once the AI
  respawns the plane. Leave or unready ends the wait. Objectives that follow
  lineages are host state the readout (`TargetBrief`'s objective) and the
  Debrief message already carry. No message, packet or golden sample changed.

## Protocol 28 (host loading)

*Built (slice NET2, 2026-10-13); John's decision unless marked.* A host
builds a mission's world on its own loop: when the King changes the mission
or a setting that rebuilds it (friendly fire, the loadout rule), when a
flight starts with the players' loadouts, and for the next mission after an
end. Its loop sends nothing meanwhile. Before protocol 28 a build longer than
5 seconds dropped every player for silence, and one longer than 1.5 seconds
set the standbys racing to take the game over. John (2026-10-13): keep the
build on the loop and send keepalives during it, not a worker thread ("other
games take a minute or two to load; 5 s is so short").

```mermaid
sequenceDiagram
  participant H as Host loop
  participant T as Loading thread
  participant P as Player (and standby)
  H->>T: start, with every player's loading Keepalive
  Note over H: the mission builds
  T->>P: Keepalive, loading (at 0.25 s)
  T->>P: Keepalive, loading (every 0.5 s)
  Note over P: connection kept, no migration race
  H->>T: stop (the build is done)
  H->>P: Payload (loading ends)
```

- **The packet.** The transport's [Keepalive](#keepalive) gains a flags
  byte; bit 0 says the host is loading. A host sends it to each player with
  that player's connection id, from its own address, so it needs the same
  identity as a Payload.
- **When the host sends it.** *Agent decisions:* a quarter second into a
  build (a quick build sends none), then every half second, until the build
  ends or has run for 3 minutes. The 3-minute bound is *fitted*: John asked
  for load times up to a couple of minutes; the longest build measured is
  7.5 s in a debug build on a loaded machine (slice NET). A host stuck past
  the bound is dropped by its players 5 seconds later and replaced by a
  standby, as before.
- **Who sends it.** A thread the host starts just before the build and stops
  right after it (`tore_net::Loading`), on a clone of the host's socket that
  the program gives it: the dedicated server, a game that hosts (its own
  player through a send-only copy of the in-process link) and a hosting
  `tore-bot`. A relayed player is reached as a Relay frame through the relay
  channels open when the build began. The thread only sends; it never reads
  the socket or touches the host's state.
- **What a player does.** It counts the keepalive as hearing from the host
  (no 5-second timeout), and while the host's newest word since its last
  Payload is a loading keepalive it counts the host lost only after **4
  seconds** of silence instead of 1.5 (*agent decision*: eight missed
  keepalives). Standby 1 takes over at the same 4 seconds and standby 2 three
  seconds later, so a host that dies in its build is still replaced. The
  host's next Payload ends the loading.
- **The log.** A build the thread spoke for logs one line on the host, the
  same in the dedicated server, a hosting game and a hosting bot: "loaded the
  mission in 15.0 s; 3 players kept through it (30 rounds of loading
  keepalives)". A hosting bot's players print "the host is loading a
  mission" and "the host is back after N ms of loading".
- **Not covered.** The first build of a host, before anyone joins, needs
  none. A player's own build is the client side's (slice NET). The build
  itself is no faster: a worker thread is for when load times are worked on.

## Limits of stage L

Stage L's limits are in [Limits](#limits) (the "Stage L (protocol 10)" row).

### Stage L as built

*Built (L2), each an agent decision:*

- **Printable ASCII** in a key is a space to `~`, as chat's.
- **Every bound is refused, never cut.** A key, a label or an importer
  string over its length, a count over its limit, an item count of 0, a
  build or kind code that names nothing, items or gaps out of order or
  twice, and a shared item with a key (or another item without one) fail
  the writer with the same error the reader gives. An importer's version or
  commit may be empty.
- **A gap's players** come by lobby id, ascending, each once, so equal gaps
  code to equal bytes. A gap the host lacks has an empty label; a label
  there is refused.
- **Sizes.** The largest Content (1,024 items with 32-byte keys) is about
  42 KB and fits a message. The largest Content gaps does not: 1,024 gaps
  with the longest key and label naming 64 players each would be about 175
  KB, and the writer refuses any body over 64 KB, as for every message. At
  the usual sizes (a 12-byte key, a short label, one player) 1,024 gaps are
  about 33 KB. The host keeps its gaps inside one message (slice L3).
- **The shared item** is, on a real import, the 299 radio phrase
  recordings (`TORE_RADIO_*`) and nothing else: L1 measured it, and the
  combat effects and creator and cloud tables the design first listed are
  not read by a mission build ([content items](../ARCHITECTURE.md#content-items)).

*Built (L3, 2026-10-05), the sessions, each an agent decision unless
credited:*

- **The client** sends its Content as its first message, at Accepted, before
  the Mission arrives; an import with no item at all sends none (the wire
  wants at least one item) and is never counted. A key the wire cannot
  carry, or items past 1,024, are left out, and an importer's version or
  commit is cut to 64 bytes; no real import has any of these.
- **The host** keeps each player's Content (a second replaces the first)
  and counts every connection whose Content has arrived and that is not
  closing (John, 2026-10-05: every connected player counts). After each
  receive or update it finds the gaps again when a Content arrived or a
  player left; when they changed, every counted player is sent the new
  Content gaps, and a player whose Content just arrived is sent the gaps
  even if they did not change. A player whose Content has not arrived gets
  none until it does.
- **A gap the host lacks** names the players that lack it too; a player that
  has it is not named, since there is nothing to compare its digest with.
- **One message.** When every gap would not fit one reliable message (64 KB)
  or the wire's 1,024, the host keeps the shared item, then the aircraft,
  then the theaters and the weapons last, and cuts the list where it fits,
  in Content's order. Its own refusals still see every gap; a gap left out
  is only not dimmed in a player's creator, and the log says how many were
  left out. At the sizes stage L expects (a dozen players, a few gaps) the
  message is a few hundred bytes.
- **The lobby's build** is each player's Content's build, unknown until it
  arrives.
- **Refused and Content refused** carry the words of the
  [architecture](../ARCHITECTURE.md#the-words): a Change mission whose
  aircraft or theater is in a gap and a Loadout whose weapon is in one are
  refused with "Not everyone can fly the ...: ..." before the mission is
  built; the host words a Content refused's `unable` text in the third
  person from the player's Content and the mission's items, and the player's
  own game keeps its second-person reason. The shared item restricts no
  choice.

## Host migration and rejoin (stage K)

*Designed 2026-10-05; agent proposals awaiting John's review. Built (K0,
2026-10-05) under **protocol 13**, kinds 39 to 54
([as built](#stage-k-as-built)).* The wire of
stage K ([architecture](../ARCHITECTURE.md#host-migration-and-rejoin)): what
lets a standby host follow the mission, a dropped player rejoin with a token,
and every client resume with the game's new host. Slice K0 builds all of it at
once under protocol 13, so the slices after it never change the bytes. Codings follow [what the game's
sections settled](#what-the-games-sections-settled): varints are
`tore-codec`'s, strings a length byte and UTF-8, "a presence bit" a 1 then the
value or a 0 alone. An address and a candidate are coded as the master's
[common fields](master-protocol.md#common-fields) code them.

### Reach and Reach answer

Two transport packets under the versioned id. A Reach asks whether one game
reaches another's joined socket and how long the round trip is; sent the other
way first, it opens the sender's router to the receiver, as a
[Punch](#punch) does.

| Packet | Kind | Fields | Size |
| --- | --- | --- | --- |
| Reach | 12 | Session id (64), nonce (64), the sender's lobby id (8) | 22 bytes |
| Reach answer | 13 | Nonce (64), session id (64), role (8: 0 not hosting, 1 hosting this session now) | 22 bytes |

- **Who answers.** A game's peers router (`tore_net::peers`, in front of its
  joined socket) and a host, each only for its own session's id. A Reach with
  another id, to a game in no session, or past 10 a second from one address,
  is counted and dropped. The answer is never longer than the Reach.
- **What it is for.** A [reach test](../ARCHITECTURE.md#reaching-the-new-host)
  sends five Reaches 200 ms apart to each address and takes the median round
  trip of those answered. A returning old host, or a client, asks a standby
  whether it hosts now: role 1.

### The token in the Challenge answer

After the [path byte](#the-path-in-the-challenge-answer): a byte (0 no token,
1 a token, any other code malformed) and, after a 1, the 128-bit rejoin token;
the zero padding to 1,000 bytes follows. The transport hands it to the
session's gate (`ConnectDetails::token`) and reads nothing into it. A game
sends it only to the session it was issued in
([rejoin](../ARCHITECTURE.md#rejoin-tokens-and-reservations)).

### Disconnect reason 8 and the Filler section

- **Disconnect reason 8**, *moved to the new host*: a client that has resumed
  with a new host tells the old one, if it still has the connection, so an old
  host that is alive does not keep flying the player's plane for it.
- **The Filler section**, Payload section kind 6: any number of zero bytes,
  ignored and counted by the receiver. A game sends it only for an Upload
  test, so the host can measure what arrives.

### New messages

Kinds 39 to 54 (stage L took 37 and 38). Every request a
player sends counts against the 20 a second the host answers, Backlog and
Standby status excepted, and every refusal is a Refused (kind 20) with its
words.

| Kind | Name | Direction | Body |
| --- | --- | --- | --- |
| 39 | Token | host to player | The token (128 bits); its life after the player was last connected, seconds (varint; 86,400) |
| 40 | Candidate | player to host | May host (1 bit); its class: platform (3 bits, the Challenge answer's codes) and processor (3 bits: 0 unknown, 1 x86-64, 2 64-bit ARM, 3 32-bit x86, 4 other); its candidates for its joined socket (a count, 4 bits, at most 3: Local, Mapped, Global IPv6); its router's mapping type (2 bits, the master's codes, 0 unknown); the CPU measure in microseconds a tick (varint, 0 not measured) and the lobby mission's number it measured (varint) |
| 41 | Reach test | host to player | Test id (16); the candidates to try (a count, 2 bits, at most 3; each: its lobby id, 8 bits, and its addresses: a count, 4 bits, at most 8, then each address) |
| 42 | Reach peers | host to a candidate | Test id (16); the players to open its router to (a count, at most 64; each: lobby id, 8 bits, and its addresses as above: the address the host sees it at and its Local candidate) |
| 43 | Reach report | player to host | Test id (16); per candidate (a count, 2 bits): lobby id (8), reached (1 bit) and, when reached, the answering address's index (4 bits) and the median round trip in milliseconds (16) |
| 44 | Upload test | host to player | Test id (16), the rate in bytes a second (varint), the length in milliseconds (16, at most 2,000) |
| 45 | Succession | host to every player | The ready standbys in order (a count, 2 bits, at most 2; each: lobby id, 8 bits; warm, 1 bit; its addresses: a count, 4 bits, at most 8, each a candidate with its kind). Sent on every change |
| 46 | Standby record | host to a standby | One [standby record](#the-standby-stream) |
| 47 | Standby status | standby to host | The newest tick it holds (32); its state (2 bits: 0 building, 1 ready and warm, 2 ready and cold, 3 behind); its mean step cost in microseconds (varint); the last Check's tick (32) and result (2 bits: 0 none, 1 equal, 2 different); needs a checkpoint (1 bit). At most twice a second |
| 48 | Resume | player to new host | Its flight with the old host (8); its newest predicted tick (32, 0 when not flying); the number of the mission it holds (varint) and the FNV-1a 64 of that mission's text (64); watching (1 bit) |
| 49 | Resumed | new host to player | The new [flight](#flights) (8; 0 for a player not flying, and nothing follows); seat (8); plane (varint); tick T (32); the number of the last command the old host applied for the seat (16); the plane's exact state at T (a long byte string, no baseline); the destroyed ground objects (a count and varints) |
| 50 | Backlog | player to new host | The new flight (8); the first tick, T (32); a count of ticks (16, at most 1,200), then each tick's controls coded as the Inputs section codes its ticks, each against the one before, with each tick's view; then the commands not yet applied (a count, at most 256; each: its tick as an offset from T, a varint, and the command as Inputs codes it), numbered from 1 in that order |
| 51 | Host moving | host to every player | The standby taking over (its lobby id, 8) and the last tick the host steps (32) |
| 52 | Taken over | new host to old host | Sent by the new host's own client on its old connection: the new host's lobby id (8) and tick T (32) |
| 53 | Release | the King to host | The plane (varint): its reservation ends |
| 54 | Rejoin | player to host | The token (128): sent after Accepted by a game that joined without sending its token and holds one for this session |
| 55 | Force start | the King to host | Empty. Starts a 10 second force start, or cancels one that is running (nr-lobbyui). The seconds left travel on Lobby, after the mission-locked bit |

**Changed messages.**

- **Accepted** carries the session's id, which a new host keeps: a resuming
  client checks it.
- **Lobby** (kind 19): each player gains its standby mark (2 bits after the
  away bit: 0 none, 1 first, 2 second); each slot gains, after its lock, a
  presence bit and the callsign of the player it is **reserved** for (a
  dropped player's plane, or an idle one's, slice F2-A). The settings carry
  setting 21.
- **Settings** (kind 24) takes setting 21, `host`, from the King; a relayed
  player is refused "Hawk connects through the relay and cannot host."

### The standby stream

The host sends a standby its records, one Standby record message each, in
order on the reliable channel; the standby applies them in that order, so any
prefix it holds is consistent. Each record starts with its type (4 bits).

| Type | Record | Fields |
| --- | --- | --- |
| 0 | Appoint | The role (2 bits: 1 or 2); warm (1 bit); a Check every N ticks (varint, 600); when cold, a checkpoint every N ticks (varint, 1,200); the lobby mission's number (varint) |
| 1 | Dismiss | Nothing: the game is no longer a standby and drops its copy |
| 2 | Flight | The mission's number (varint), the FNV-1a 64 of the flight's spec text (64), the [mission identity](checkpoint.md#restoring) (64) and whether the host's world makes contrail puffs (1 bit, since protocol 53: a dedicated server's makes none): build the world fresh from the spec the player holds, making puffs as the host's does; the journal starts at its tick 0 |
| 3 | Checkpoint begin | Its tick (32), its length in bytes (32), its chunks (16) |
| 4 | Checkpoint chunk | Its index (16), its bytes (a long byte string, at most 4,096) |
| 5 | Ticks | The first tick (32) and a count (8, 1 to 60), then each tick: the changes before the step (a count; each 1 bit: 0 scoring, then on or off, 1 bit; 1 a store cut, then the plane, a varint, and the revival weapons, 2 bits); the mission commands (a count, each by the checkpoint trait's coder of `MissionCommand`); the seat inputs (a count; each: the seat, 8 bits, then against that seat's previous input in the stream: the last command applied, 1 bit when the previous one's and else 16 bits; the controls, a bit when they lie on the wire's grid and then the Inputs section's frame against the previous one's, else each control by the trait, the gunsight's slew and zoom step included since protocol 21; the two command lists behind one presence bit; the view, 1 bit when the previous one's moved on, else 2 bits for none, as Inputs codes it (offset 8 bits, delay 6) or whole by the trait). Protocol 14 ([as built](#stage-k-as-built)) |
| 6 | State | The part (8: 1 players, 2 session, 3 court, 4 scores, 5 revivals, 6 rejoin, 7 candidates, 8 listing), the tick after which it holds (32), its bytes (a long byte string) |
| 7 | Check | The tick (32) and the FNV-1a 64 of the host's `World::checkpoint()` between that tick and the next (64) |
| 8 | Ended | The end's reason (3 bits, as Mission ended): drop the world |
| 9 | Handover | The last tick the host steps (32): take over once it is replayed |

*Agent proposals:*

- **When.** Ticks go with the snapshots, a snapshot interval at a time (30 a
  second). A State record follows the Ticks of the tick in which its part
  changed, in the same send. A Check at every 600th tick.
- **Baselines.** A seat's input codes against its previous input in the
  stream, and against none in the first Ticks record after an Appoint or a
  Flight. The stream is reliable and ordered, so a standby always holds the
  baseline. A checkpoint's chunks interleave with the Ticks that follow its
  tick; a standby buffers those until the checkpoint is whole.
- **Pacing.** At most four chunks unacknowledged; the rate the checkpoint's
  size over 8 seconds, at least 32 KB/s and at most 128 KB/s
  ([architecture](../ARCHITECTURE.md#standbys)). Nothing else in the stream is
  paced.
- **Exact.** The trait's coders destructure every field, so a field added to
  a seat's input or a variant added to a mission command fails to compile
  until the stream carries it. A part's bytes are the host's own coding,
  read by the same build only, like a checkpoint.
- **Refusals.** A Flight or a checkpoint whose mission identity differs from
  the standby's own build of the mission, or that it cannot build, is
  reported in Standby status (state 3) and the host dismisses that standby.

### Resuming

```mermaid
sequenceDiagram
  participant C as Client
  participant N as New host
  C->>N: Connect request, then the Challenge answer with the token
  N->>C: Accepted, with the session's id
  C->>N: Resume: its flight, newest tick, mission
  N->>C: Resumed: new flight, seat, plane, tick T, exact state
  N->>C: Lobby, Roster, and Mission when it differs
  C->>N: Backlog: its controls and commands from T on
  C->>N: Inputs as usual for its later ticks
  N->>C: Snapshots, once it has stepped to the present
```

- **The race.** From the same socket it joined with, the client sends
  Connect requests to every address of the Succession (and any address a
  Reach taught it) every 250 ms; the first Challenge with its nonce chooses,
  as [joining from several addresses](#joining-from-several-addresses-at-once)
  does. A Host moving starts the race at once; 1.5 seconds of silence from
  the host starts it otherwise. A relayed player's game races nothing: it
  keeps its relay channel and sends its Connect requests to the same
  [relayed address](#relayed-addresses), which the master forwards to the new
  host once the listing has moved
  ([master](master-protocol.md#moving-a-listing-stage-k)).
- **The gate** admits a token of an absent player whatever the room, and the
  connection becomes that player.
- **Inputs while resuming.** Until it goes live a resuming host's input
  buffers take ticks up to a second past the present, not past the tick it
  holds, so the Inputs a client sends after its Backlog are kept. A command
  in the Backlog that the old host had applied (a number at or below the one
  Resumed names) is not in it.
- **No snapshot** goes out during the resume window and the fast-forward; the
  first carries the own-state hash as usual and the client checks it.
- An observer that resumes sends Observe again; a player in the lobby gets
  the lobby.

### Stage K as built

*Built (K0, 2026-10-05) under **protocol 13**, kinds 39 to 54.* J6 took
protocol 12 first and versions only go up, so 11 was never used. Every
choice below is an agent decision.

- **Where it is.** The packets are `tore_net::packet` (`Reach`,
  `ReachAnswer`, `ReachRole`, the answer's `token`), the token
  `tore_net::Token` (128 bits, its low 64 written first) and its draw
  `tore_net::TokenSource` (two draws of the endpoint's generator), the
  message bodies `tore_session::wire::migration` with their limits
  (`migration::limits`), and the standby stream `tore_session::journal`.
- **Reach.** The host's transport answers a Reach itself
  (`Server::set_reach_session`, which every `Host` sets to its session).
  One for another session, or to a transport that answers none, is counted
  as unexpected; one past 10 a second from its address as rate limited,
  with a limiter of its own (200 a second in all), so Reaches cannot use up
  the joins' allowance. A host counts a Reach answer as unexpected, and a
  client counts both kinds: the peers router (slice K6) takes them before
  the client sees any.
- **The token byte.** Any code but 0 and 1 makes the Challenge answer
  malformed.
- **Filler.** A host's transport takes section kinds up to 6; the section
  must be all zero bytes, or the packet is bad. The host adds its bytes to a
  count and does nothing else until the upload test.
- **Addresses.** A Candidate's candidates are the master's candidate list
  (a 4-bit count), refused above 3 and refused with a Seen one; a
  Succession's may hold the host's Seen address. A reach target's
  addresses are plain addresses (no kind); Reach peers' count is a varint.
- **Backlog.** Each tick's view is its view offset (8 bits, ticks before
  that tick) and interpolation delay (6 bits), as Inputs codes them. The
  commands' count is a varint, and each command's offset must fall within
  the ticks, so a Backlog of no ticks carries no command.
- **Resumed.** Flight 0 is a player not flying; the writer refuses a
  flying player's flight 0.
- **Standby record.** The message holds the record's bytes whole. Its
  reader checks only that the record's type names one (0 to 9); the
  stream's reader (`journal::StreamReader`) checks the rest, in order.
- **The stream's records.** Appoint's role must be 1 or 2. In a Ticks
  record the counts of changes, mission commands and seat inputs are
  varints, at most 256, 256 and 64; each seat input's own tick must be its
  tick's. A seat's input codes against its last input in the stream moved
  on to this tick (its tick, and its view's by as many ticks, with no
  commands), so an unchanged control costs a bit. *Protocol 14 (slice
  K3):* its controls go as the Inputs section codes them (the wire's
  quantized frame against the last one's, `write_frame`) when every one of
  them lies exactly on the wire's grid, as every input the host steps does,
  and each by the trait otherwise (a -0.0 or a value between two steps), so
  the stream stays exact; its two command lists go behind one presence bit;
  its view, one bit when the last one's moved on, else none, its offset
  (8 bits) and delay (6 bits) as Inputs codes them, or whole when the
  offset passes 255, the delay 63 or the view is ahead of the tick; the
  number of its last command applied, one bit when the last one's; its own
  tick is the record's. Every field of a seat's input and of a pilot's is
  named, so a field added fails to compile until the stream carries it. An
  Appoint and a Flight
  start every baseline afresh; a record refused by the writer or the reader
  changes nothing. A record never uses the checkpoint's shared records.
- **What it costs.** Protocol 13 (measured in the K0 tests and slice K3's
  runs): a steady seat under 9 bytes a tick, 3 of them its seat byte and
  16-bit command number; 13 to 23 bytes a seat a tick with bots' sticks,
  about 50 KB/s to a standby with 30 humans. Protocol 14 (slice K3): a
  steady seat under 2 bytes; 3.4 to 5.6 bytes a seat a tick with bots'
  sticks, about 12.4 KB/s of ticks with 30 humans and 2.3 KB/s with four
  on the real 15 against 15 mission
  ([baseline](../baselines/standby-stream-2026-10-05.md)).
- **The lobby.** The standby mark follows the away bit: each appointed
  standby's role (slice K3). A slot's reservation is filled now from the idle
  aircraft (slice F2-A: an away player's plane), the seam where slice K5's
  table joins it (`Host::reserved_for`).
- **Setting 21.** Its values are 0 to 256: 0 calculated, else 1 plus the
  pinned player's lobby id. *Built (K6):* the registry takes any; the host
  refuses a relayed player, one who turned hosting off, a lobby id nobody
  has and any pin on a dedicated server, in words.
- **Host selection** (slice K6, no wire change). A game sends Candidate
  once the lobby arrives and again whenever it changes. Reach peers names
  every candidate a player reported beside the address the host sees it at,
  not only its Local one, so a candidate's firewall opens to a player's
  IPv6 address too. A Reach report's address index counts the test's
  addresses, then any a candidate's own Reach taught the player (its router's
  port towards that player). An Upload test's burst goes in Filler sections
  of 1,100 bytes, whole packets as they fall due and the rest at the end;
  the client's transport now takes section kinds up to 6, so the Payloads
  it sends can carry Filler (one from a host is ignored). A Reach report for
  a test that ended, and a Candidate from a dedicated server's player, are
  taken and do nothing.
- **The requests.** The Standby status is built (slice K3: one from a game
  that is not a standby is ignored); Release and Rejoin (slice K5, below);
  Candidate and Reach report (slice K6); Resume, Backlog and Taken over
  (slice K4, below). Backlog and Standby status do not count against the 20
  requests a second. A client keeps every Standby record for its standby, in
  order (`Client::take_standby_records`), acts on Reach test, Reach peers and
  Upload test (slice K6), and on Succession, Resumed and Host moving (slice
  K4).
- **Resuming** (slice K4, no wire change). T, in Resumed and Backlog, is the
  tick the new host steps next; Resumed's exact state is the plane's as tick
  T begins (a client's prediction labels it T - 1), and the Backlog's first
  tick is T. A player who resumes after the new host has stepped past T is
  resumed at the host's tick then. The new flight is the client's old flight
  plus one (never 0), so it reads as later. Resumed goes to a player the host
  is resuming who flies and said so (its newest tick not 0) and holds the
  host's mission (number and the FNV-1a 64 of the spec text, the flight's
  loadouts included); any other gets Resumed with flight 0, and the Mission
  again when its own differs (a seated player whose game no longer flies the
  plane sees it go back to the AI and takes one again from the lobby). A
  second Resume or Backlog is refused "You have resumed already."; a
  Backlog from a game the host is not resuming "Nothing is being resumed for
  you here.", one for another flight "That backlog is for another flight.".
  A Backlog's commands are the ones numbered above Resumed's last command,
  each at its own tick or at T when that came earlier, renumbered from 1;
  the Inputs that follow number on from there. Until the host is at the
  present a resumed seat's Inputs are kept with its backlog and fed to it a
  tick at a time; no snapshot goes out. A Taken over is taken only from a
  standby naming its own lobby id, and refused otherwise "Only one of the
  game's standbys can take it over.". An old host's game asks standby 1 with
  a Reach from the host's socket (`host::reach_packet`) and stops when a
  Reach answer says role 1 (`host::hosting_answer`). The client starts a
  fresh wire (no baseline, name or event) on the new connection before
  Resumed; a race reaches an address whose host gives another session's id
  in Accepted no more.
- **The standby's side** (slice K2, `tore_session::standby`). A Check's
  tick is the world's `tick()` when the host hashed it: the tick the next
  step runs, as a checkpoint's tick is. Appointed in flight, a standby gets
  Appoint and then a checkpoint, with no Flight record; it builds the flight
  it holds for the Appoint's mission and the checkpoint's identity checks
  it. An Appoint starts the standby afresh, its parts too, so the host sends
  every part after one; Ended keeps them. A checkpoint's chunks are exactly
  4,096 bytes but the last, may come in any order, each once, and the whole
  is checked by its container's CRC-32, tick and mission identity: damage
  sets "needs a checkpoint" in the next Standby status, and another mission
  is state 3. A Ticks record that cannot be read, or that does not start
  where the last ended, is state 3 too, until an Appoint or a Flight.
- **The records a host makes** (slice K1, no wire change). A Ticks
  record's changes are those the host made since its last step, in order
  (scoring is switched on in tick 0's). State records hold the players,
  session, court, scores and revivals parts; a part is written when its
  coding changes, after the tick that changed it in flight and at once in
  the lobby, so the lobby's parts come before the Flight. A part's tick is
  the last tick the host stepped (0 before any). Moments in a part are the
  host's clock; the session part ends with the clock's reading, which alone
  does not make the part go out again. Connections are named by their
  player's join order. The bytes are this build's own
  ([architecture](../ARCHITECTURE.md#what-moves-with-the-host)).
- **The stream a host sends** (slice K3; protocol 14 only for the seat
  inputs' coding above). After the
  Appoint: a checkpoint when flying (no Flight record), then every state
  part, holding after the last tick stepped. A Ticks record carries the
  ticks stepped in one host update (usually one), sent at the end of that
  update with the snapshots of those ticks, so a host killed between two
  updates has sent every tick it showed its players (*battery fix,
  2026-10-20:* it used to carry a snapshot interval of ticks, and a host
  killed with half an interval waiting lost it); any other record sends
  the waiting ticks first. A Check goes at the first update at
  or after each 600th tick, to warm standbys only. A cold standby's next
  checkpoint begins 1,200 ticks after its last began, or as soon as its last
  has gone out. A checkpoint's chunks go at its bytes over 8 seconds, at
  least 32,000 B/s, and with the rest of the stream within 1 Mbit/s
  allowing 20 percent for the transport's framing and resends; at most four
  are unacknowledged. A standby that reports itself behind gets Dismiss and
  is not appointed again until the next Flight; one whose status asks for a
  checkpoint gets one, at most once 2 seconds after the last went out; a
  warm one that fails two checks gets a fresh Appoint, cold. A Standby
  status from a game that is not a standby is ignored, not refused.

## The surface world (protocol 22)

*Built (slice N1 of the surface round, 2026-10-10); every choice is an agent
decision unless credited.* What a ground target and the theater layouts' air
defenses ([surface defenses](../spec/surface-defenses.md)) put on the wire.
The host runs every surface unit; each player's game builds the same surface
from the mission text and its seed and never steps it.

```mermaid
flowchart LR
    text["Mission text with the seed<br/>the host drew"] --> build["Every game builds<br/>units, batteries, trucks,<br/>parked aircraft, starts"]
    build --> digest{"Seated or Resumed:<br/>the host's digest<br/>equals ours?"}
    digest -->|"no"| refuse["Content refused,<br/>back to the lobby"]
    digest -->|"yes"| fly["Fly: moving units as entities,<br/>bursts, ends and unit states<br/>as events"]
```

- **The seed.** A mission with a ground target whose `surface-seed` is 0 (the
  Quick Mission creator's lobby mission, a server's file, a bot's) gets one
  from the host before its text is sent: drawn from the session id, the
  mission's number and an FNV-1a 64 of its text, never 0. A restart of the
  same mission keeps it, so its layout stays (the creator's rule).
- **The digest.** The Seated message and the Resumed flight end with the
  host's `Surface::digest()` (64 bits): the template and its settings, the
  relocation, every unit with its place, the parked and deck aircraft, the
  supply trucks, the batteries and both sides' starts. A client compares it
  with its own copy's once the mission has loaded; a difference sends Content
  refused with the reason ("Your game places this mission's ground target
  differently from the host's (surface ..., host ...)"), and the player stays
  in the lobby, marked unable, as for a content difference. A resume whose
  digest differs ends the new host's wait with an empty Backlog first.
- **Moving units** are the fifth entity kind: a unit that follows a route
  (`Surface.courses`), sent as long as it is in the mission, moving, arrived
  or wrecked, with the host's Mover's position, velocity (from the picture a
  snapshot before, as for debris), heading, pitch and bank, and a wreck bit.
  The route's leg and the follower's speed command are not sent: a client
  never steps a unit (lead ruling after M1), and a new host takes them from
  the checkpoint. The client draws them in its picture's `surface` list and
  moves the unit's ground row with them, so the views and the target window
  follow the unit. A unit that stands is never sent.
- **Surface burst** (event 18): unit (varint), hardpoint (8), target (1 and a
  varint), the first round's direction (azimuth from +z towards +x, then
  elevation, 16 bits each), the burst's rounds and its span in ticks
  (varints). Round *k* leaves at the first tick at or after *k* × span /
  rounds after the event's tick, the host controller's own schedule. One
  event per burst, sent when its first round flies, never one per round. A
  client remakes the rounds, owned by the unit, from its hardpoint (a moving
  unit's where the picture draws it) with the simulation's round rules;
  later rounds turn from the first by as much as the lead on the target the
  picture draws turns (the host's gunsight solution, a radar gun observing
  every tick and a visual one every half second), and each ends a little past
  the target's range as the host's do. Flak shells are never sent: each burst
  in the air is an Effect of kind Flak with its explosion type, 27 for the
  KS-12 and 28 for the KS-19.
- **Surface burst end** (event 19): unit, hardpoint and the rounds fired, at
  the burst's first tick, when its controller stopped before its schedule's
  end (the target left the gun's gates, the magazine ran dry, the unit
  died). The client takes back the rounds it made past them.
- **Surface unit** (event 20): unit (varint), hit points (signed varint),
  radar emitting (1), a count of hardpoints (at most 64) and for each a
  launcher's loaded rails (1 and a varint) and a gun's spare magazines (1 and
  a varint; a ship's guns have unlimited spares and send none). A gun's
  loaded rounds are not sent: they change with every round. The host sends a
  unit's state when any of these changes, and at every seat, resume and
  observer start for each unit not as the mission built it (full hit points,
  radar off, full rails and spares). Units and placed parked aircraft with a
  combat row are told; the client takes the newest state at or before its
  drawn tick, and a unit it was never told of is as built. A unit's death
  stays the Ground destroyed event's. The game puts the states in its copy of
  the mission, so a launcher's rails empty and refill, a radar's row emits
  and a damaged parked aircraft smokes.
- **Owners.** Surface units own their shots: a SAM is a projectile entity
  whose owner is the unit (`speed_f8` comes from its velocity, so its launch
  look starts when its motor lights), and its launch is a Launch event with
  the unit as the shooter. A client's remade surface rounds keep the unit as
  their owner. The flight views take a moving unit as a subject.
- **Parked aircraft** are built on every client. Their destruction is the
  Ground destroyed event, their crash crater and fire are Mark events and
  their fragment is a debris entity whose damage variant is the piece, as the
  host's combat makes them, so a client calls no kill path of its own.
- **Debrief and readout.** The debrief pilot ends with the name of the
  surface unit that shot it down (1 and a string). The readout's painting
  radars are its scalar group 14. The debrief's objectives already carried the
  ground target's combined Destroy or Protect line.
- **New projectiles** are found by the set alive at the last tick sorted,
  not by the highest number, since surface shots are numbered in a block of
  their own: before this, an aircraft's launch after a SAM's went untold.

## The air-to-ground round (protocol 23)

*Built (slice NA, 2026-10-11); agent decisions unless credited.* The round's
radar pages, sights, laser, bombs and guided weapons work online with the
host running every solution
([guide](../MULTIPLAYER.md#air-to-ground-online), [spec](../spec/air-to-ground.md)).
What travels:

- **The seat's controls.** Combat codes 30 and 31 with its extended
  varints, the scope's HARM channel and the sight's slew, zoom and codes 28
  and 29 on every sight aircraft ([inputs](#inputs-as-built)). The standby
  stream codes a seat's commands as the Inputs section does, so it carries
  them too.
- **The cockpit.** The readout's seven [air-to-ground groups](#the-air-to-ground-groups),
  the ground pages' flags and their contacts, and readiness codes 25 and 26.
  Buddy lasing needs nothing more: a buddy's spot is in the readout of every
  plane that may use it, with its owner, so the targeting page and the HUD
  name the owner's callsign from the lobby.
- **The weapons.** Bombs (a pack's included, each with the host's scatter),
  rocket pod rounds and guided weapons are projectile entities; no event was
  added. Their ends are the Hit, Ground and Effect events every weapon
  already had.
- **Loadouts.** A loadout row's weapon may name a laser designator pod's SEE
  record (`PAVESPK.SEE`, or the retired `PAVEKNF.SEE`, which a host builds as the Pave Spike; X1 writes the pod there), with its
  count and a quantity of 0; the Loadout and Flight loadouts messages carry
  it as any name. The content check counts the pods, and Cheat loading's
  novelty stores (`~BLDR.JT`, `~MOOSE.JT`), as weapon items, so a loadout
  that names one a player lacks or has differently is refused as any other
  weapon is ([stage L](#compatibility-stage-l)). The import's other `~`
  records are never offered and stay out.
- **Migration and rejoin.** The sights, lasers, delivery modes and guided
  flights are in the combat checkpoint, so a standby holds them and a new
  host takes them from it. A takeover builds the flight's loadouts under the
  session's own friendly fire and loadout rule before the session part
  restores every setting (a Cheat loading under `loadouts any` was refused at
  a takeover before). A player who rejoins takes its plane back from the AI,
  which flew it meanwhile, and finds the cockpit as it left it (slice FX-J):
  the host's combat keeps what the player set (the weapon and NAV, the seeker
  and bomb sight modes, the sight, the laser, the designation, the A-G page,
  the radar switch and NAV's airport) while the AI flies, and steps the sight
  and the laser on, so a laser weapon the player left guiding keeps its spot.
  The kept cockpit is in the combat checkpoint (`live::State`'s `kept`, after
  `guided_impacts`), so a standby and a new host carry it; no message
  changes. The rejoining game's readout shows it as any readout does
  ([the architecture's list](../ARCHITECTURE.md#what-a-returning-player-keeps)).

## The redrawn airports (protocol 24)

*Built (slice FX-A of the mp-fixes round, 2026-10-11); agent decisions unless
credited.* John asked for the [redrawn airports](redrawn-airports.md) on for
everyone online once the protocol bumps (2026-10-10) and as the default
(2026-10-11). The airport scene is not on the wire: every game of a session
and the server rebuild it from their own import and build.

- **Fixed on online.** The mission text has no airfield line and always
  means the redraw (`MissionSpec::from_text`), so the King's game, every
  player's, the dedicated server and the bot build the same airfields.
  `TORE_REDRAWN_AIRPORTS=0` applies only to single-player launches and the
  offline tools; nothing online reads it. Carrying the setting in the mission
  text was the alternative; it would let a session fly the retail airfields
  but add a text line, a lobby control and a server setting for a developer
  aid, so it was left out.
- **Why a bump.** A 23 game builds the retail airfields: other buildings
  stand elsewhere, the redraw's added hangars and vehicles are targets with
  ids a 23 game lacks (`0x40F0_0000` up), parked aircraft move onto the
  aprons and the AI's airfield points and ground-start slots differ. The
  version check keeps such games apart. The redraw changes no message's
  bytes; the wire golden changes in the version line, the transport packets
  sealed with it and, for the Guided sight group folded under 24 (FX-H), the
  full readout sample.
- **Checks that still apply.** The Seated message's surface digest covers the
  parked aircraft and starts, so a game whose airfields differed would still
  be refused with Content refused. The theater content item covers the files
  the terrain reads, now including the building and vehicle types the plans
  add.

## The city fill (protocol 25)

*Built (slice CL-CF of the surface close-out, 2026-10-11); agent decisions
unless credited.* John asked for the [city fill](theater.md#city-texture-light-areas-cf0-prototype)
on by default everywhere, online too (2026-10-11). Like the airport scene,
the fill is not on the wire: every game of a session and the server build it
from their own import, the theater layout alone deciding where each building
stands.

- **Fixed on online.** The mission text has no city line and always means
  the fill (`MissionSpec::from_text`), so the King's game, every player's,
  the dedicated server and the bot build the same city. `TORE_CITY_FILL=0`
  applies only to single-player launches and the offline tools; nothing
  online reads it. The development aid `TORE_CITY_FILL_CAP`, the building
  budget, is ignored online the same way: the app passes it in the launch
  overrides (`Overrides::city_fill_cap`), which a mission text never sets,
  so a session's mission always builds the whole fill.
- **Why a bump.** The buildings are scenery but not harmless: an aircraft
  below a roof crashes and a strong enough ground burst knocks buildings
  flat, so a 24 game, which builds no fill, would fly a different world. The
  version check keeps such games apart. The wire golden changes in the
  version line, the transport packets sealed with it and the City flattened
  event below.
- **Knocked-down buildings.** Only the host's world steps combat, so only
  its fill sees a ground burst. **City flattened** (event 21, everyone)
  carries the buildings that fell: a count (at most 256; more go in several
  events), the first building's index in the fill and each later one as its
  gap from the one before less one, ascending. The host sends the buildings
  that fell at the tick they fell (its tracker compares the fill's version
  after each step), and at every seat, resume and observer start one or
  more events with every building already flat. A client knocks them down
  in its copy of the mission as the event arrives, so its prediction, which
  flies through that copy's terrain, passes where the host's aircraft do;
  the game copies the flat set into its own world each frame, so the
  scenery draws them flat. A new flight (a restart) stands every building
  up again before its seat's events. *Agent decision:* building indices
  rather than the burst's centre and radius, since the indices need no
  float rule on the client, and the seat's list is the same event. The
  buildings are also in the world checkpoint, section 12
  ([checkpoint](checkpoint.md#sections)), so the Check hash covers them and
  a standby, a rejoin's restore and a new host after a migration or a
  takeover start with them flat.
- **Folded under 25.** The mission text's `enemy-start ground RUNWAY|auto`
  line (slice CL-R, [dedicated server](../DEDICATED-SERVER.md)), written only
  when Enemy Wing 1 starts on the ground: a 24 game refuses a mission that
  carries it, and the mission hash covers it. The AI airfield sequence's
  checkpoint gains one field, `ground_trim` (the rudder trim an AI aircraft
  learns on the ground, slice CL-AP): the world checkpoint and so the
  Check hash and a standby's restored state carry it, and a 24 build, which
  lacks it, would read a 25 checkpoint wrongly. The Quick Mission
  [front line](../spec/surface-defenses.md#the-front-line) (slice CL-FL) is
  part of the surface every game builds from the mission and its seed: the
  surface digest covers it, so a game that builds no front disagrees with
  one that does and is refused. The knocked-down buildings (review fixes,
  above): the City flattened event (code 21) and the world checkpoint's
  city fill section (section 12), which a 24 build lacks and would refuse.
  `TORE_CITY_FILL_CAP` no longer reaches a session's mission. The
  close-out's other slices changed no wire bytes.
- **AI strike, also under 25.** The AI strike round
  ([AI strike](../spec/ai-strike.md#online-and-replays)) was merged into the
  surface close-out before 25 shipped, so it rides the same bump. Its wing
  orders take order code 14 (Engage IR/Laser) and the escape code 15 with a
  3-bit sub-code (0 engage my target's class, 1 Lase my target); a 24 reader refuses
  both. The mission text's `objective ... strike` line is new, and the
  mission hash covers it. The world checkpoint gains the strike cockpits
  (`StrikeCockpit` in each ownship and kept cockpit, the AI strikers'
  `AiCockpit` list, the AI laser sets), each striker's ground picture and
  `Strike` state, and the AI wings' `handed_tasking`
  ([checkpoint](checkpoint.md#sections)), so the Check hash and a standby's
  restore carry a pass in progress; a 24 build would read them wrongly. No
  packet layout changed.
- **Checks that still apply.** The theater content item lists the files the
  fill is built from (the reference weather layer, every ground placement's
  picture, the kit's definitions and shapes), read without building the
  fill, so a game whose city art or kit differed is told which theater
  differs.

## Protocol 26: explosions sized by weapon

*Built (slice BF of the bug-fix round, 2026-10-12); the size rule is
John's (opinionated, 2026-10-12), the coding an agent decision.* An impact
is drawn at its weapon's size against its explosion type's: guns up to
40 mm at half size, the AC-130's 105 mm like a 500 lb bomb, bombs and warheads by the
cube root of their weight against 500 lb
([explosions](../spec/explosions.md#size-by-weapon)). Only the host's combat
knows which weapon made an effect, so the size travels with it.

- **The Effect event (code 11)** gains, after the explosion type, a 1-bit
  flag and, when it is set, 8 bits: the percent of its type's size the
  explosion is drawn at (1 to 255), or 0 for a dud's inert puff (drawn full
  size, no flash and no sparks). The flag is clear for 100, so a kill, a
  crash, a launch or a flak burst costs one bit more than under 25. A 25
  reader would read the flag as the next event's first bit, so the version
  keeps such games apart.
- **The wire golden** changes in the version line, the transport packets
  sealed with it, and four new Effect events in the sampled events section
  (a 20 mm hit at 50, a large ground strike at 200, a 2,000 lb bomb at 159 and a
  dud at 0), and two new Mark events (a crater at 159 and a pock mark at 50).
- **The Mark event (code 12)** gains, after the position, the same 1-bit
  flag and byte: a crater's drawn scale (craters follow the weapon's yield,
  John, 2026-10-12). A crater of size 0 is a gun round's pock mark. A newly
  seated player's backlog holds every crater and fire but only the newest
  1,000 pock marks (`host::sorting::SEAT_POCK_MARKS`, agent decision): at
  13.5 bytes a mark, a long mission's 20,000 would hold the snapshots' event
  share for a minute ([craters by weapon](../spec/explosions.md#craters-by-weapon)).
- **Not on the wire:** a missile or rocket's motor state. A client dates
  each round from the client tick it first drew it, by the weapon's motor,
  as it does the motor's smoke (`regen::Effects`), and draws the motor's
  flame only while it burns. The light and sparks of an impact
  ([impact light and sparks](../spec/explosions.md#impact-light-and-sparks))
  come from the effect alone.
- **Checkpoints.** The combat effect (`live::Effect`) and the picture's
  `EffectPose` gain the scale, and the picture's `ProjectilePose` its
  motor state ([checkpoint](checkpoint.md)); the Check hash and a
  standby's restore carry them, and a 25 build would read them wrongly.
## Protocol 27 (stores)

*Built (slice BP3 of the bug-fix round, 2026-10-12); agent decisions unless
credited.* John asked for other aircraft's stores on a network client
(2026-10-12): before it, a client drew pylons and stores on its own aircraft
only ([stores on pylons](../spec/stores-on-pylons.md)). What a client needs
to draw them is the aircraft's loadout, which every game of a session builds
the same from the mission text and the flight's loadouts, and the whole
stores still on each station, which changes only when a whole store leaves.
Only the second travels, and only when it changes.

```mermaid
flowchart LR
    tick["Host tick: every aircraft's<br/>drawn stations and stores"] --> same{"Same as last told<br/>(as built if never)?"}
    same -->|"yes"| nothing["Nothing sent"]
    same -->|"no"| event["Stores event<br/>to everyone"]
    seat["A seat, a resume,<br/>an observer's start"] --> standing["Stores event for every<br/>aircraft not as built"]
    event --> client["Client: newest state at or<br/>before the drawn tick,<br/>else as built from its loadout"]
    standing --> client
```

- **Stores** (event 22, everyone): the aircraft (varint), a count (varint, at
  most 64), then for each drawn station, in ascending station order, its
  index (7 bits since protocol 51, 6 before) and the whole stores still on
  it (varint). A drawn station is an external one (centreline, fuselage,
  wing, wingtip) that is not a gun and was loaded at the start; a rocket
  pod counts as one store while it has a rocket left. Tank rows are codes
  55 + hardpoint: hardpoints 0 to 8 are 55 to 63, and hardpoint 9 (the F-16C
  inner wet pair and the Rafale centreline) is 64, which is why the index
  grew to 7 bits. A decoder refuses stations out of order or past 127.
- **When.** The host compares every aircraft's drawn stations each tick with
  what it last told, and with the loadout as built for an aircraft it never
  told, and sends a Stores event for each that differs, at the tick it
  changed: a release that empties a whole store, a jettison, a rearm or a
  respawn. A human-flown plane's come from its cockpit's rounds, an AI
  aircraft's from its weapon service. A player seated later, every seat of a
  resumed flight after a migration and an observer starting to watch get a
  Stores event for every aircraft not as built (the observer's delayed
  stream keeps each aircraft's newest one, as it keeps the craters).
- **The client** keeps each aircraft's told states in tick order and draws
  the newest at or before the tick its picture shows, so a station empties
  when the picture reaches the release, the tick the weapon appears. An
  aircraft never told is drawn as its loadout built it. The loadout is the
  session's copy of the mission's (an AI aircraft's or a plane's flight
  loadout); an aircraft with none there draws no stores. The seat's own
  aircraft is drawn from its cockpit readout, which is newer. A capture's
  conversion and an observer's live recording keep the stores the same way
  ([replays](../REPLAYS.md#stores)).
- **Bandwidth** (measured by the `a_stores_event_is_small_and_checked` test
  and the bots' `stores:` line): the event's code and fields take 14 bytes
  for a fighter's six drawn stations with fewer than 128 stores each, plus
  the events section's 2 to 4 bytes of number and tick. One is sent for each
  whole store that leaves; nothing is sent while nothing changes. The worst
  case, a 64-station loadout with 65,535 on each, is 251 bytes since the
  station index is 7 bits (protocol 51; it was 243 at 6 bits). A real
  loadout has at most a dozen drawn stations and under 128 on each, at most
  about 25 bytes. The six-station example is still 14 bytes. The oldest
  event may use the packet's message room, so a long one is never starved.
- **Why a bump.** A 26 game reads event code 22 as invalid and drops the
  packet's events. The wire golden changes in the version line, the events
  sample (one Stores event added) and the transport packets sealed with the
  version. No checkpoint changes: the host's record of what it told is
  rebuilt by every new host, which then tells every seat what is not as
  built.
- **Numbering.** Built as 26 on its own branch; slice BF's explosion scale
  took 26 first, so the stores take 27 (lead, 2026-10-12).

## Protocol 29 (the sensor console)

*Built (slice nr-orbit, 2026-10-13); the lead numbered it 29.* John asked for the AC-130's sensor console with an
orbit autopilot ([spec](../spec/ac130-linked-guns.md#the-sensor-console-and-the-orbit)).

- **Seat command 28, SensorConsole**, no fields: Ctrl+9 on the AC-130. The
  5-bit command code has three codes left (29 to 31).
- **The exact own-plane state**: the autopilot's mode can be `Orbit` (4), and
  the autopilot gained an `Option<Orbit>` after its loops (nine fields: the
  point and centre, three each, then radius, height, speed, reach, bank,
  climb and throttle, all f64 against the baseline's). A hosted orbit with a
  standing point is predicted exactly by a client; a moving track changes
  the point every tick, so the client is corrected at each snapshot while it
  follows one (corrections too small to show, about the cross-platform
  cost).
- **Why a bump.** An older game reads command 28 as invalid and cannot read
  the longer own-plane state. The wire golden changes in the Inputs command
  sample (one SensorConsole), the version line and the packets sealed with
  the version.

## Protocol 30 (the console's search and keys)

*Built (slice nr-cona, 2026-10-14); the lead numbered it 30.* John asked for slew, zoom, pin and release and heading, altitude and
throttle keys in the sensor console, with the autopilot searching straight
while nothing is pinned ([spec](../spec/ac130-linked-guns.md#the-consoles-keys)).

- **Seat command 29, Console**: a sensor console key, then a 3-bit sub-code
  and an 8-bit value. Sub-codes: 0 Pin (value 0; any other value is
  refused), 1 Heading (signed steps), 2 Altitude (signed steps), 3 Speed
  (percentage 0 to 100; above 100 is refused), 4 SpeedStep (signed steps);
  5 to 7 are refused. The 5-bit command code has two codes left (30 and 31).
- **The exact own-plane state**: the autopilot's `Orbit` gained three fields
  after its throttle: `pinned` (a bool: a point is held, else the search),
  `heading` and `altitude` (f64 against the baseline's: the search's
  commanded heading, radians, and altitude, ft). The orbit's `speed_fps` is
  now also the commanded speed of the search. The client predicts the search
  exactly; a key's change and a pin or release reach it with the host's next
  exact state, one small correction about a round trip later, as the
  console's opening does.
- **Why a bump.** An older game reads command 29 as invalid and cannot read
  the longer own-plane state. The wire golden changes in the Inputs command
  sample (five Console commands), the version line and the packets sealed
  with the version.

## Protocol 31 (the gun lock and the centre's pause)

*Built (slice nr-cona follow-up, 2026-10-14) under protocol 31.* John asked for a gun lock in the AC-130 console and for a
slewed pin to move the orbit only after a 5 s pause
([spec](../spec/ac130-linked-guns.md#the-gun-lock)).

- **Seat command 29, Console:** sub-code 5, GunLock (value 0; any other
  value is refused). Sub-codes 6 and 7 are refused.
- **The exact own-plane state:** the autopilot's `Orbit` gained `still_s`
  (f64 against the baseline's) after `altitude`: the seconds the pin has
  stood still.
- **The readout's gunsight group** gained four values after the zoom step:
  the gun lock flag and its point as an offset from the aim point (1/8 ft).
  The group is 41 values.
- **Why a bump.** An older game refuses sub-code 5, reads the gunsight group
  as the wrong length, and cannot read the longer own-plane state. The wire
  golden changes in the Inputs sample (one GunLock) and the readout sample
  (a gun lock), with the version line. `wire_golden` fails until the bump.

## Limits

Decoders check every count and length against these before reading on.

| Limit | Value |
| --- | --- |
| Datagram | 1,200 bytes |
| Message body, fragment | 256 bytes |
| Reassembled message | 64 KB |
| Unacknowledged messages | 256 |
| Unacknowledged events | 1,024; beyond it the connection ends as too far behind |
| Aircraft per snapshot | 64 |
| Projectiles, debris pieces | 256 each |
| Ejected pilots | 64 |
| Moving surface units (protocol 22) | 128 |
| Hardpoints in a Surface unit event (protocol 22) | 64 |
| Stations in a Stores event (protocol 27) | 64 |
| Ground contacts in a readout (protocol 23) | 50 |
| Emitters of the HARM cue, guided weapons in flight in a readout (protocol 23) | 8, 4 |
| Name table | 4,096 entries |
| String | 255 bytes |
| Input ticks per packet | 24 |
| Commands per packet | 64 |
| Recording stems per radio call or order voice | 32 |
| Stations of a loadout | 64 |
| Planes of a roster | 256 |
| Manifest entries, names in a refusal | 8,192 |
| Destroyed ground objects at seating | 8,192 |
| Seats per host | 30 |
| Players and slots in a lobby state, settings, loadouts at a flight's start | 64 each |
| Chat line (protocol 4) | 80 characters of printable ASCII, 5 lines in 5 seconds a player, a quick message's sound 12 characters |
| Keepalive (protocol 5) | 1 a second from a stalled game, for at most 60 seconds of stall; from a loading host (protocol 28) 2 a second to each player, for at most 3 minutes of build |
| Punch (protocol 9) | 5 to each of at most 8 addresses for each introduction, at most 10 introductions a second; a race tries at most 12 addresses |
| Phase 2 (protocol 8, updated 73) | Settings in one message 64; combatants in Scores 1,024; rows in Results 1,024; a password 255 bytes; Observe at most twice a second from one connection |
| Stage K (protocol 13) | Standbys 2; addresses of a standby or a candidate 8; candidates in a reach test 3, five Reaches to each address, one test every 10 seconds; Reaches answered 10 a second from one address; checkpoint chunks 4,096 bytes, 4 unacknowledged; ticks in a Ticks record 60; a Backlog 1,200 ticks and 256 commands; an Upload test 2 seconds; tokens a game keeps 32 |
| Stage L (protocol 10) | Items in a Content 1,024 (at least 1); a key 32 bytes of printable ASCII; a label 64 bytes; gaps in a Content gaps 1,024; players named in one gap 64; the importer's version and commit 64 bytes each |

## Captures

A client's **capture** is everything its session was given, so it can be run
again offline into the same frames ([recordings and
diagnostics](../ARCHITECTURE.md#recordings-and-diagnostics)). *Built (D8a)*,
`tore_session::client::capture`; the format is an agent decision. It is not
part of the protocol: it never crosses the network, and its own version
number changes on its own.

The file starts with 12 bytes: the 8-byte magic `TORE-CAP`
(`tore_session::capture::MAGIC`, which a game's pruner checks so that it only
ever deletes captures), the capture format's version (16 bits, 3 since powered-lift controls, 4 since the gunsight's slew and zoom, 5 since protocol 22's 3-bit entity kind in the view subject) and the
protocol version (16 bits); a reader refuses another of either. Records follow, each a kind (8 bits), a body length (32 bits) and the
body; a capture cut short ends at its last whole record. Numbers are least
significant byte first, times are nanoseconds of the client's clock (64 bits),
and strings are a 16-bit length and UTF-8.

| Kind | Record | Body |
| --- | --- | --- |
| 1 | Start | The time the join started, the seed of its randomness (64 bits: the nonce comes from it), the server's address, the callsign, the game version and commit, a release-build byte, the plane asked for (a byte, then 32 bits when 1), and whether the client readies by itself (a byte, format 2). Never the password |
| 2 | Receive | The time, the sender's address, then the datagram as it arrived |
| 3 | Update | The time, then the controls as the client rounded them, bit packed: pitch, roll and yaw (16 bits each), the throttle rate (8), the throttle position (1, then 16), the trigger (1), the scope's channel (2), range step (4) and history (1), the powered-lift block in Inputs coding, the gunsight's slew (two signed bytes) and zoom step (3 bits, format 4), the view subject (1, then its kind in 3 bits since format 5, 2 before, and its id as a varint), and the commands (a varint count, then each in its Inputs coding) |
| 4 | Frame | The time a frame was drawn |
| 5 | Leave | The time the player ended the mission |
| 6 | Disconnect | The time the player quit |
| 7 | Sent | The time, then an Inputs section the client sent |
| 8 | Request | The time, then a lobby request the player made (its message kind and body), format 2; the automatic ready's own are not recorded, since the replayed client makes them again |
| 9 | Leave game | The time the player left the game (its flight, then the connection once the debrief is in), format 2 |
| 10 | Race | Right after the Start of a join through the master (protocol 9, J2): the introduction id (64 bits), a count (8 bits), then each address raced (a string) and its [path](#the-path-in-the-challenge-answer) code (a byte). The replay races the same addresses |

A replay (`capture::replay`) starts a client from the Start record, seeded
the same, feeds it every Receive, Update, Frame, Leave, Disconnect, Request
and Leave game at its time with no network, and hands back each frame. The replayed client writes a
capture of its own, which equals the original byte for byte when it behaved
the same: same inputs sent, same frames. A capture holds what the client knew
and nothing of the host, so it needs the same import to replay. The game
converts a capture into a replay ([Network
flights](../REPLAYS.md#network-flights), `--convert-capture`) by running it
again with an observer; the capture format did not change for it.

## Versions

- The **protocol version** is one number in `tore-session`
  (`wire::PROTOCOL_VERSION`, 2 since the readout's coding, 3 since the lobby,
  EF4, 4 since chat, EF6, 5 since the transport's [Keepalive](#keepalive),
  EF-K, 6 since the exact flight state added the overspeed countdown and legacy
  failure RNG, 7 since each player's platform, in the Challenge answer and
  the lobby's player list, 8 since
  [phase 2](#phase-2-the-kings-settings-revival-scores-and-observers), F2-0,
  9 since the transport's [Punch](#punch) and the
  [path byte](#the-path-in-the-challenge-answer) of the Challenge answer,
  J2, 10 since [stage L](#compatibility-stage-l)'s Content and Content gaps
  and each lobby player's Fighters Anthology build, L2, 12 since each lobby
  player's [connection path](#the-path-in-the-challenge-answer), J6, 13
  since [stage K](#host-migration-and-rejoin-stage-k)'s wire, K0, 14 since
  the standby stream's seat inputs code their controls as the Inputs
  section does, K3, 15 since the [flight data link](#data-link-stage-g),
  G7, 16 since the baseline field of the readout and the entity records is
  7 bits, a window of 127 snapshots, B2, 17 since setting 22, the King's
  snapshot rate, R1, 18 since the aircraft variety import: 6-bit aircraft
  codes for the 37 selectable aircraft, the Inputs powered-lift block and
  axis commands 24 to 26, the lift, vector and gun-mount devices, the
  readout's gun aim and linked-gun mask, gun-group combat commands 26 and
  27, the loadout's tank list, and the exact flight state's powered-lift
  controls, 19 since the VTOL overhaul's foundations: the exact flight
  state's powered-lift state, the powered-lift command 27 and its sub-codes,
  hover hold as switch 11, and the aircraft record's rotor group, 20 since
  the lobby pass: the Slot request's [side](#side-requests), setting 7's
  `balanced`, setting 12's 50 to 150 nautical miles and setting 23,
  `ai-respawn`, in the lobby state's list (W0), and the lobby state's last bit,
  whether the mission is locked (K1); 11 was never used). Protocol 20 is the
  whole lobby pass's one bump. Its other slices add no bytes to a message:
  Autobalance (A1) works through the existing slot requests and lobby state,
  and the AI respawn and lead hold commands (R1, R2) are build-exact codings
  of the journal, the checkpoint and the session's state parts, which the wire
  golden does not sample (the journal's codings are listed under
  [Rejoin as built](#rejoin-as-built-k5), beside revival's). 21 since the
  AC-130 gunsight (slice S4): the Inputs frame's
  [gunsight block](#inputs-as-built) and its ninth change bit, the sight's
  combat commands 28 and 29, the readout's [gunsight](#the-gunsight) group
  (31 parts), the standby stream's seat inputs, which carry the sight
  with the rest of the controls, and the six gun-mount angles at 1/32,767 in
  the entity's devices and the readout's stores (slice G5). Protocol 21 is
  the gunsight project's one bump. 22 since the surface round (slice N1,
  [the surface world](#the-surface-world-protocol-22)): the fifth entity kind
  and the 3-bit kind code in the view subject, events 18 to 20, the surface
  digest in the Seated message and the Resumed flight, the debrief pilot's
  shot-down-by name and the readout's painting radars (32 parts). The ground
  target's text lines travel in the mission text. Protocol 22 is the surface
  round's one bump. 23 since the air-to-ground round (slice NA,
  [the round](#the-air-to-ground-round-protocol-23)): the readout's seven
  [air-to-ground groups](#the-air-to-ground-groups), the Sensors group's
  ground page flags and the ground contacts list (40 parts), and, taken into
  23's definition, what the round's slices added under 22: combat code 30,
  the extended code 31 and its varints, the HARM channel (3) and readiness
  codes 25 and 26. A loadout row naming a pod and the content items of the
  pods and the novelty stores change no layout. Protocol 23 is the air-to-ground
  round's one bump. 24 since the mp-fixes round (slice FX-A,
  [the redrawn airports](#the-redrawn-airports-protocol-24)): every game builds
  the redrawn airports, so the airfields' buildings, their added targets, the
  parked aircraft and the AI's airfield points differ from a 23 game's; and,
  folded under 24, the round's other slices: the Tab gun key's extended
  combat commands 3 (`GunKeyDown`) and 4 (`GunKeyUp`) after code 31 (FX-T,
  [inputs as built](#inputs-as-built)) and the readout's Guided sight group
  (19) growing from 15 to 18 values with the level T (present, steering,
  ground speed; FX-H, [the air-to-ground groups](#the-air-to-ground-groups)).
  The combat checkpoint, which the standby stream and a takeover carry,
  also changes under 24 with no message layout change: a missile's
  `Motion.holds_path` (FX-M) and the `kept` map after `guided_impacts`
  (FX-J, rejoin). 25 since the surface close-out (slice CL-CF,
  [the city fill](#the-city-fill-protocol-25)): every game builds the city
  fill, whose buildings aircraft crash into and ground bursts knock flat;
  folded under 25, the mission text's `enemy-start` line (CL-R), which a 24
  game refuses; `ground_trim` in the AI airfield sequence's checkpoint
  (CL-AP); the front line in the surface digest (CL-FL); and the City
  flattened event (code 21) with the world checkpoint's city fill section
  (section 12), which carry the buildings ground bursts knocked flat. 26
  since the bug-fix round (slice BF,
  [explosions sized by weapon](#protocol-26-explosions-sized-by-weapon)):
  the Effect event's scale, and the effect's scale and the round's motor
  state in the checkpoints.
  27 since the stores slice (BP3, [stores](#protocol-27-stores)): the
  Stores event (code 22), every aircraft's drawn stations and the whole
  stores on each, sent when they change and at a seat, so a client draws
  every aircraft's pylons and stores.
  28 since the networking slice NET2 ([host loading](#protocol-28-host-loading)):
  the Keepalive's flags byte, sent by a host building a mission.
  29 since the AC-130 sensor console (slice nr-orbit,
  [the sensor console](#protocol-29-the-sensor-console)): seat command 28,
  and the orbit hold in the exact own-plane state's autopilot;
  30 since the console's search and keys (slice nr-cona,
  [protocol 30](#protocol-30-the-consoles-search-and-keys)): seat command 29,
  and the orbit's `pinned`, `heading` and `altitude`;
  31 since the gun lock and the centre's pause (slice nr-cona follow-up,
  [protocol 31](#protocol-31-the-gun-lock-and-the-centres-pause)): Console
  sub-code 5 (GunLock), the orbit's `still_s`, and the gun lock in the
  readout's gunsight group;
  32 since the airport merge (slice nr-apmerge): no message changes; the
  shared world differs (paired theater airfields merged into single fields),
  so a game of another world must not join;
  33 since the missile identities pass (slice nr-fox2): no message changes;
  the store catalog (R-27 variants, corrected names, carrier rules), IR
  seeker response and radio cooldown keys differ, so an older game must not
  join;
  34 since the player release pass (slice nr-fox3): no message changes;
  player missile release outcomes (advisory MAX RANGE and lock, active
  maddog, SARH support recovery) differ, so an older game must not join;
  35 since the F-15/F-16 stations pass (slice nr-f15): no message changes;
  the F-15C, F-16C and Mi-24 store stations (release positions) moved, so
  an older game must not join;
  36 since the wingman loadouts pass (slice nr-wingload): no message
  changes; AI wingmen take their human lead's loadout (more FlightLoadouts
  entries, mission text `share-flight-loadout`), so an older game must not
  join;
  37 since jettison (slice nr-jett): the extended live command 5
  (JettisonTanks), tank quantities in store stations 55 to 63, the
  sound-only jettison blast and inert falling bodies, so an older game must
  not join;
  38 since the bomb flight model (slice nr-bombphys): bombs carry their
  body (mass, drag, retarder, steering) and fly one shared fitted model, and
  sights read it, so an older game must not join;
  39 since the small guided bombs (slice nr-bombs2): GBU-12, GBU-16 and
  GBU-38 join the store catalog, so an older game must not join;
  40 since the AC-130 damage pass (slice nr-nightfx): the 40 mm and
  105 mm shells do twice the damage, so an older game must not join;
  41 since the contact bounds fix (slice nr-mav): thin line geometry no
  longer inflates surface objects' solid contact boxes, so an older game
  must not join;
  42 since the cleanup round (slices nr-ac105, nr-aijett, nr-fx2,
  nr-sndfix): the 105 mm splash and AI jettison change the shared
  simulation, so an older game must not join;
  43 since the extra stations (slice nr-stations): F-15C centreline and
  wing tank rows, the F-16C inner wet pair and two tank records, so an
  older game must not join;
  44 since the second stations pass (slice nr-stations2): F-15C centreline
  bomb rack, F-22 wing stations, MiG-29 and Rafale centreline tanks and
  three tank records, so an older game must not join;
  45 since the ordnance page pass (slice nr-ordui): the Pave Knife is no
  longer a weapon item and a loadout naming it loads the Pave Spike, so an
  older game must not join;
  46 since the store signature (slice nr-rcs): external stores raise the
  radar signature every radar and seeker reads, so an older game must not
  join;
  47 since the F-16C wingtip AIM-120 (slice nr-f16tip): an older game
  refuses the load, so an older game must not join;
  48 since Player Choice and AI off (slice nr-mpchoice): the ai setting,
  Choice slots, aircraft in loadouts and revivals, and the despawn and
  replacement commands, so an older game must not join;
  49 since the collision credit fix (slice nr-collkill): splash on a
  bystander no longer makes its shooter the last attacker, which changes
  kill credit and the combat checkpoint, so an older game must not join;
  50 since the F-4B gun pod option (slice nr-f4gun): the F-4B's centreline
  row takes one SUU-16, which an older game refuses, and the `Stores` event
  lists a hung gun pod as one store, so an older game must not join;
  51 since the Stores station index (slice nr-basefix): a Stores event's
  station index is 7 bits and can name tank hardpoint 9 (code 64), so an
  older game must not join;
  52 since min-players and the King's force start (slices nr-minplayers,
  nr-lobbyui): the lobby settings list gains min-players, and the Lobby
  message gains the force-start seconds, so an older game must not join;
  53 since the optimization run (slices S7b, S9 and S11): AI aircraft code
  empty visual, map and received-radar lists by default (lean sensors), the
  world checkpoint codes whether the world makes contrail puffs (a dedicated
  server's makes none), and the standby stream's Flight record carries that
  bit, so an older game must not join;
  54 since optimization round 2 (slice r2-prof): AI aircraft observe at 60 Hz
  by default (staggered, held observations carried forward at their velocity),
  so a standby replaying the journal at an older default would part from the
  host, and an older game must not join;
  55 since the F-15C wing pylons (slice v2-f15): the F-15C gains a fifth
  weapon station (its wing row takes a tank or one air-to-air missile), so its
  configuration in the world checkpoint and its lobby loadout carry one more
  station, and an older game must not join;
  56 since the F-15C shoulder rails (slice v2-f15 follow-up): its twin
  shoulder Sidewinders sit and release 0.85 ft either side of the pylon
  instead of 1.75 ft, which changes where those rounds start, so an older game
  must not join;
  57 since the F-15C wing pylons moved inboard (slice v2-f15 second follow-up):
  the shoulder rails are back at 1.75 ft either side of their pylon, and the
  whole wing pylon assembly sits 11 ft from the centreline instead of 16.7 ft,
  which moves those stores' release points, so an older game must not join;
  58 since the F-15C wing pylons moved forward (slice v2-f15 forward pass): the
  whole wing assembly sits 7 ft further forward, just behind the leading edge,
  which moves those stores' release points again, so an older game must not
  join;
  59 since the AI jettison policy (slice bf-aijett): AI aircraft drop only
  their external tanks entering a fight and keep their weapons, shedding
  air-to-ground loads only to get home damaged, on fuel, or when losing a close
  fight, which changes AI behaviour and the simulation, so an older game must
  not join;
  60 since the battery AI fixes (slice bf-ai): a drop-launched missile spares its
  launcher until a second after its motor lights, the AI's terrain look-ahead
  grows with excess speed, and an AI approach on its glide path no longer holds
  high over rising ground, which changes the simulation, so an older game must
  not join;
  61 since AI default loads carry fuel (slice bf-aifuel): AI aircraft on their
  default load start with their tanks full and their real engine count, and AI
  endurance counts external fuel, which changes the simulation, so an older
  game must not join;
  62 since the steering adapter flies a damaged airframe (slice bf-final): an
  AI's stick for a wanted load allows for lost lift and elevator authority,
  which changes damaged AI flight, so an older game must not join;
  63 since shared ground-pad foundations (ground-pads, 2026-10-21): static
  foundations level the terrain used by placement, movement and contact,
  changing simulation checksums, so an older game must not join;
  64 since F-22N stores (2026-10-21): the naval donor gains the F-22A wing
  carrier and internal bays; both use drawn magazine release origins and
  only bay weapons wait for doors, so an older game must not join;
  65 since deferred lobby Settings (2026-10-22): kind 24 gains optional
  Realism, applied with ordinary settings in one mission rebuild;
  67 adds dedicated feedback (2026-10-22): Lobby begins with a feedback
  enabled bit, kind 80 submits feedback, and kind 81 returns the accepted
  flag to its sender. Protocol 66 is reserved for the parallel lobby change.
  71 combines the compatibility-code handshake from the parallel 68 branch
  and private reservation preflight from the parallel 70 branch (2026-10-23).
  Connect request adds a 32-bit compatibility code after its nonce;
  Challenge answer and Discover answer add it after the commit.
  Protocol 71 replaces commit/tag admission with protocol, simulation and
  content identity and adds transport packets 14 and 15, so lounges confirm
  a reclaimable slot instead of trusting a saved token. Earlier 68 through
  70 development numbers are superseded by this combined layout.
  A protocol 71 or later Connect request keeps the code immediately after
  its nonce so a mismatched protocol can still report both codes. Earlier
  requests have no code and report `00000000` in that diagnostic.
  72 adds human visual designation (target-select, 2026-10-23): extended combat
  command 6 carries the target id as an unsigned varint. The host validates its
  ownship's visual contact and keeps a separate visual-selection flag in sensor
  checkpoints and saved pilot picks. AI selection is unchanged. It follows 71
  in 0.2.1rc-1.
  73 adds ongoing FlightReport, cumulative hit counts, departed-player and AI
  score identity, and stable original member numbers on revival (2026-10-23),
  on top of the protocol 72 target-selection and protocol 71 handshake layout.
  The compatibility simulation version is 2 for these rules and checkpoint changes.
  74 changes no bytes (2026-10-24, 0.2.2-rc1): tanks fire at aircraft only up to
  20 degrees, after a lay and with wider aim, and the host migration resume keeps
  a late Backlog, asks one checkpoint per resync and counts its drop timeout from
  the first update. The compatibility simulation version is 3.
  75 (2026-10-27, 0.2.2-rc1): guided ordnance flies on the energy model (new
  projectile, decoy and prediction checkpoint state; inert stores clear target
  and incoming flags), two-sided store points load in pairs, the AI missile
  defense keeps new checkpointed state, bridges and Quick Mission surface
  placement change world geometry, and hosts may publish a retried surface
  seed. The compatibility simulation version is 4.
  76 (2026-10-27, 0.2.2-rc1): the HUD readout carries an optional range-cue
  minimum (estimates grow from 12 to 14 values), the prediction checkpoint
  keeps up to four reachable range regions and advises the one for the present
  range, the AI spaces countermeasure pairs at least 240 ticks apart, chaff
  against radar stores is 1.30 times as effective, and three stores have new
  motor fits. The compatibility simulation version is 5.
  77 changes no message layout (2026-10-28, 0.2.2-rc1): chaff against radar
  stores is 2.60 times as effective instead of 1.30, so decoy rolls differ.
  The compatibility simulation version is 6.
  78 (2026-10-28, 0.2.2-rc1): the mission text can carry a Quick Mission
  `situation advantage|disadvantage` line that older games reject, and the
  offered enemy distances come from a fixed table (a server moves an
  unoffered `separation-nm` to the table's choice). A non-neutral situation
  builds a different start. The compatibility simulation version is 7.
  79 (2026-10-28, 0.2.2-rc1): an AI striker whose target dies before its
  pass retargets at once, and the last 5 seconds before a release are not
  broken off by a defense trigger; the strike checkpoint gains the release
  estimate, commitment and retarget state. The compatibility simulation
  version is 8.
  80 changes no message layout (2026-10-28, 0.2.2-rc1): an AI helicopter
  gives up a pass into a known threat ring seen before release and takes a
  clear stand-off, and a line out of a ring must lead straight out. The
  compatibility simulation version is 9.
  81 (2026-10-29, 0.2.2-rc1): an AI helicopter's threat rings come from each
  site's real launch reach at a helicopter, its stand-off advance from the
  store's flight time, and only a shot from a site it did not know is an
  ambush; helicopters search farther firing ranges and fixed-wing strikers
  release stand-off stores outside known rings and turn away. Checkpoints
  gain the site's helicopter reach and the strike's stand-off plan. The
  compatibility simulation version is 10.
  82 (2026-10-29, 0.2.2-rc1): shoulder-fired SAM teams stay off the AI's and
  the player's sensors until spotted close in or given away by a launch, and
  an AI helicopter ambushes a moving target from ahead of its path.
  Checkpoints gain each pilot's spotted teams, the listing's team flag, the
  AI picture's unmasked teams and the helicopter's move-ahead plan. The
  compatibility simulation version is 11.
  Any change to the bytes raises it. A test
  (`wire_golden`) encodes a fixed set of sections and messages and compares
  them with a committed copy, `crates/tore-session/wire-golden.txt` (since
  protocol 5 it holds one transport packet too, the Keepalive, sealed for the
  version, and since protocol 28 the loading one too; since protocol 7 also
  the Challenge answer's bytes before its padding; since protocol 9 also a Punch; since protocol 13 also a Reach,
  its answer and the Challenge answer's token); when
  they differ it fails and says to raise the version and refresh the copy
  (`TORE_UPDATE_WIRE_GOLDEN=1 cargo test --locked -p tore-session
  wire_golden`), the way the controls list test works. The copy records the
  version, and refreshing changed bytes under the same version is refused.
  The [discovery](#discovery) packets have their own
  golden file.
- The **game build** is the informational version string and commit. Admission
  uses the [compatibility code](../ARCHITECTURE.md#compatibility-code), including
  an explicit simulation version. Simulation and replay changes that can make
  peers disagree raise both that version and the protocol, even without new bytes.
- The **content** is checked after the mission loads, from the manifest in
  the Mission message ([architecture](../ARCHITECTURE.md#a-mission-with-no-window)).

## Security

As the [guide](../MULTIPLAYER.md#transport) says, v1 traffic is not
encrypted: the password keeps strangers out of a server but anyone who can
watch the network can read it. What the protocol does guard against:

- **Spoofed connections and reflection.** The cookie makes a client prove it
  receives at its address before the host keeps anything, and a Challenge is
  21 bytes against a 1,000-byte request; no host reply to a handshake packet
  is larger than it.
- **Forged answers.** Accepted and Refuse carry the client's nonce, so only
  someone who sees the traffic can refuse or misdirect a join.
- **Blind injection.** Once connected, a packet must carry the connection's
  32-bit id and a valid checksum.
- **Keepalives** (protocol 5) need the same identity as a Payload, the
  connection's address and its id, so they add nothing a stranger could not
  already do with an empty Payload: they only keep a connection from timing
  out, are never answered, are smaller than an empty Payload, and a game sends
  at most one a second, for at most a minute of stall. A host's loading
  keepalive (protocol 28) is taken only from the host's address with the
  connection's id; a forged one could at most hold a player's migration off
  for 4 seconds, which an empty Payload from the same spot already could.
- **Malformed packets.** Every decoder is bounded and returns an error instead
  of panicking; a seeded fuzz test feeds each one random and mutated packets.
- **Floods.** Connect requests are rate-limited per address and in total, and
  a connection that sends too many bad packets is closed.
- **Reflection by discovery.** A discover answer is never longer than the query
  it answers, queries are rate-limited per address and in total apart from
  joins, and an old host drops the kind without a word.
- **Punches and the relay** (stage J). A host punches only the addresses the
  master gives it for a player whose address the master has proven, 13 bytes
  five times each; a relayed address cannot be claimed from a real socket
  ([relayed addresses](#relayed-addresses)). The master's own guards are in
  [its protocol](master-protocol.md#security).
- **Rejoin tokens** (stage K, protocol 13) travel unencrypted, as the password
  does: someone who can watch the traffic can take a dropped player's
  reserved aircraft with its token. Accepted for v1 with a trusted host; the
  token is 128 bits from the operating system's randomness, so it cannot be
  guessed.
- **Reach** (stage K) is no reflector: an answer is never longer than its
  Reach, a game answers only its own session's id, and at most 10 a second
  from one address.
- **The standby stream** (stage K) carries the whole mission and session,
  the game's password and every player's token included, to the standbys
  only: games of the same session, trusted as the host is. A client races
  only the addresses its host's Succession named, and resumes only with a
  host that answers with the session's id.

## Inert jettison integration

The next protocol must include extended combat command 5, `JettisonTanks`,
and interpret Stores IDs 55 through 63 as tank quantities at hardpoints 0
through 8. Protocol 51 extends that to hardpoint 9 at ID 64. Sound-only blast ID 39 plays `&BOMB.11K`. Projectile identities
prefixed `JETTISON-` are inert; `TORE-BODY:length:diameter` shape identifiers
request fitted bodies with length and diameter in feet, each finite, positive
and at most 100. The combat checkpoint layout also changes.
See [jettison](../spec/jettison.md). Protocol numbering belongs to the lead.

### Player Choice wire changes awaiting integration

The nr-mpchoice continuation leaves the protocol constant at 46 for the lead to bump.
Loadout (13) adds an optional aircraft after the plane number: one presence bit,
then the existing six-bit aircraft code when present, before the optional loadout.
FlightLoadouts (22) adds a counted list of `(plane varint, aircraft six bits)`
before its loadout list, limited to 64 entries. LobbySlot adds an optional
six-bit AI-default aircraft immediately after its wing, before its member byte.
Presence denotes Player Choice. The aircraft field remains the current selection.
Mission text adds `wing-choice SIDE N` and `plane-aircraft PLANE AIRCRAFT`.
Revive (28) now carries the mission varint, an optional aircraft (presence bit,
then six-bit code), then an optional loadout (presence bit, then the existing
loadout grammar). The host validates the pair atomically. Content catalogue
Weapon items also include parsed `.GAS` tank records, so changed tanks use the
same common-content check as weapons and designator pods. Spawned retains its
existing aircraft and accepted spawn/loadout fields. No tape header or frame
format changes; existing replay files retain their grammar. The wire golden and
protocol number are refreshed by the lead at integration.


## Dedicated feedback messages (67)

Lobby kind 19 begins with one boolean, the operator's feedback enable flag.
Only dedicated servers advertise it. Player-hosted games leave it false.
Kinds 80 (player to host) and 81 (host to sender) use six unsigned varints:
flag tick, submit tick, opened-at Unix milliseconds, submitted-at Unix
milliseconds, seat, plane. Three length-prefixed UTF-8 strings follow:
player (128 bytes), aircraft (64 bytes), and text (4,000 bytes, at most 1,000
Unicode scalar values). Empty text and control characters other than newline
and tab are rejected. The host ignores the supplied identity and submit tick,
replaces them with its own, and accepts only seated players while feedback is
enabled. The shared chat rate limit applies. These messages use the ordinary
reliable channel and are captured for local replay conversion. See
[the replay event](replay-feedback.md).

## Ongoing multiplayer reports (73)

Reliable host-to-client kind 56, FlightReport: tick varint u32, current plane
varint u32, followed by the existing Debrief body and Results body. It is a
statistics snapshot, not a request to end the flight. Sent once per simulated
second, on seating and before a clean departure. Captures retain it as an
ordinary reliable message; the replay file grammar is unchanged.

The current Scores and Results grammar is in the reliable-message table above.
The new fields retain departed humans, identify AI lineages and carry hit totals.
A departed human's lobby id may later be reused, so only non-left human rows
identify current lobby members. Ranking puts a winning historical human first,
even when its old lobby id was reused. Results carry scores in co-op too.
Spawned, roster, snapshot and recording member fields keep
the original flight position on revival, with a new plane id per life. No
additional member field or replay-version change is needed. The session Scores
checkpoint part now carries retained human identities, cumulative debrief
counters, the previous per-plane counter samples, AI counters and last-controller flags, side hit totals and AI lineage tallies.
