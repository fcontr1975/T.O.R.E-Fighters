# Master server

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

How to build, configure, run and update `tore-master`, the program behind
the game's Internet Lobby. Design of 2026-10-05 for stages I and J of the
[multiplayer plan](multiplayer-plan.md#stages). *Built (I2, 2026-10-05):* the
program, its configuration, listings, browsing, the router test's probes,
telemetry counts, the limits, the status line and files, and the `flood`
tool. *Built (J2, 2026-10-05):* introductions, the Meets to hosts and their
retries ([as built](formats/master-protocol.md#introductions-as-built)).
*Built (J3, 2026-10-05):* the relay, on by default, with its channels,
rates, idle closing and monthly allowance
([as built](formats/master-protocol.md#the-relay-as-built)). Every setting, default
and number is an *agent proposal* unless it is credited to John; John
approved the abuse limits, the ports, the relay cap and where it runs on
2026-10-05 ([decisions](MULTIPLAYER.md#decisions)). How it works inside: [architecture](ARCHITECTURE.md#master-server-and-connectivity).
What it says on the wire: [master protocol](formats/master-protocol.md).

## Contents

- [What it is](#what-it-is)
- [What it needs](#what-it-needs)
- [Building it](#building-it)
  - [Downloading a release](#downloading-a-release)
- [Running it](#running-it)
- [The configuration file](#the-configuration-file)
- [Running it as a service](#running-it-as-a-service)
- [Ports and firewalls](#ports-and-firewalls)
- [The name](#the-name)
- [Logs and statistics](#logs-and-statistics)
- [What the master keeps](#what-the-master-keeps)
- [The relay's monthly allowance](#the-relays-monthly-allowance)
- [Updating](#updating)
- [Testing on one machine](#testing-on-one-machine)
- [Deploying at jroverton.com](#deploying-at-jrovertoncom)
- [When something is wrong](#when-something-is-wrong)
- [Security](#security)

## What it is

One small program that does five things for players on the internet:

- **Lists games.** A host that wants to be found sends its game's summary to
  the master every 30 seconds; the Internet Lobby screen asks the master for
  the list. A game that stops sending is dropped after 90 seconds.
- **Tests routers.** It tells a game how its router maps the game's port, so
  the game knows whether a direct path can work.
- **Introduces players to hosts,** so both can send to each other at the same
  moment and open a path through their routers (hole punching).
- **Relays** the traffic of a player and a host who cannot reach each other
  any other way, for example a player on a phone hotspot behind the carrier's
  shared address (CGNAT). Relaying is what costs transfer.
- **Counts anonymous statistics** sent by games that allow it.

It is not needed for playing on a local network or joining by address:
Direct Connection never talks to it. If the master is down, the Internet
Lobby is empty and everything else works.

It never reads Fighters Anthology's data. The machine that runs it needs no
import, and nothing derived from retail media ever reaches it.

## What it needs

- A Linux machine with a public IPv4 address, and ideally a public IPv6
  address, reachable on two UDP ports. John's decision of 2026-09-28: a
  Linode shared 2 GB plan with 1 TB of transfer a month.
- Very little else. One thread; well under one core even while relaying the
  plan's 64 channels at their full rate (at most about 8 MB/s in and out); a
  few tens of megabytes of memory with every table full. Its loop
  sleeps a millisecond between turns while busy (a datagram or an open relay
  channel within the last second) and ten milliseconds, with its timers every
  25, when idle (optimization run S2). Before that it slept a millisecond
  always: a debug build took 0.7 percent of one
  core of the development machine (Ryzen 9 7900X) over 10 idle seconds, and
  answered a 10-second flood of 2,000 datagrams a second with every limit
  holding (I2, 2026-10-05).
- A name in DNS that the game is built to ask for
  ([the name](#the-name)).

It builds and runs on Windows and macOS too (the tests run there), but only
Linux with systemd is described here.

## Building it

From the repository root, on a machine with the project's Rust toolchain:

```sh
cargo build --release --locked -p tore-master
```

The program is `target/release/tore-master`, one file with no libraries
beside the system's C library. Build it where it will run, or on a system
with an older C library than the server's: a build from a rolling
distribution (such as the development machine's) can refuse to start on a
long-term release ("GLIBC_2.xx not found").

### Downloading a release

The other way to get the program is the release workflow's. Each release tag
also publishes a Linux build of `tore-master` beside the game's packages, on
the [releases page](https://github.com/john-overton/T.O.R.E-Fighters/releases)
(*agent decision:* it is a separate file, not part of the game's package):

- `tore-master-<version>-linux-x86_64.tar.gz`, holding `tore-master`, this
  guide, the license and the third-party notices;
- `tore-master-<version>-linux-x86_64.tar.gz.sha256`, its checksum.

It is built on the same Ubuntu 22.04 runner as the game's Linux package and
checked not to need a newer C library than that release's 2.35, so it runs on
any current long-term Ubuntu (22.04 and later) without building anything:

```sh
curl -LO https://github.com/john-overton/T.O.R.E-Fighters/releases/download/v0.2.2/tore-master-0.2.2-linux-x86_64.tar.gz
curl -LO https://github.com/john-overton/T.O.R.E-Fighters/releases/download/v0.2.2/tore-master-0.2.2-linux-x86_64.tar.gz.sha256
sha256sum -c tore-master-0.2.2-linux-x86_64.tar.gz.sha256
tar -xzf tore-master-0.2.2-linux-x86_64.tar.gz
tore-master-0.2.2-linux-x86_64/tore-master --version
```

(`v0.2.2` stands for the release you want.) A workflow run that is not a tag
(a `release-test/...` branch, or a manual run) builds the same file as the
`master-ubuntu-22.04` artifact without publishing it. The build is unsigned,
like the game's. Run `--check-config` ([updating](#updating)) before it takes
over.

## Running it

```sh
tore-master --config /etc/tore-master/master.conf
tore-master --config master.conf --check-config
tore-master flood 203.0.113.10:26901 10
tore-master --version
```

| Option | Meaning |
| --- | --- |
| `--config FILE` | The configuration file. Without one, every setting has its default |
| `--check-config` | Reads the configuration, says what it would do, and exits: 0 when it is good, 1 with the line and the reason when not. One line says plainly whether the relay is active and its limits |
| `flood TARGET SECONDS` | A load tool: sends every kind of request at a master from many ports of this machine for that long, then prints what came back ([the flood tool](#the-flood-tool)). Only point it at a master you run |
| `--version` | The build: version and commit |

It runs in the foreground and writes its lines to standard output. Typed on
its standard input, `status` prints the status line now, `listings` prints
one line per listing and the count, and `quit` stops it cleanly: it closes
every relay channel (both ends are told), writes the day's telemetry counts
and the relay's month figure, and prints `Stopped`. Ctrl+C and SIGTERM (what
`systemctl stop` sends) end it at once (agent decision: the standard library
cannot catch a signal without unsafe code, which the project forbids). That
loses little: the counts, the minute table and the relay's figure are
written every minute, and the listings are rebuilt within one heartbeat of
a restart. Either way a stop ends the relayed pairs, which time out and
rejoin as after any lost connection.

The start lines, and `--check-config`, say what the relay will do in one
line, for example:

```text
relay ACTIVE: up to 64 channels, 2 per player address, 128 KB/s each way per channel; 800 GB a month (new channels refused from 760 GB, open ones closed at 800 GB)
relay this month (2026-10): 12.7GB of 800 GB relayed
```

With `relay off` the line reads `relay OFF: players who cannot connect
directly cannot join`; with `relay-channels 0` or `relay-month-gb 0` it says
that every relay request is refused. The second line is a start line only:
the figure read back from the state folder.

### The flood tool

`tore-master flood TARGET SECONDS [--rate N] [--ports N]` sends, from 32
ports of this machine and 2,000 datagrams a second in all (the two
options), Registers with no cookie and with a wrong one, Heartbeats, Keeps
and Unregisters with made-up tokens, Browse, Details, Probes to both ports
(the second at TARGET's port + 1), Introduce, Relay request, Relay frames,
Relay close, Reports, random bytes and a Browse in a version no master
speaks. It never answers a Challenge, so it makes no listing, and its
Reports carry the game version `flood`, which the master does not count.

At the same time it asks for the list once a second from another address
of the machine, when there is one: against a master on 127.0.0.1 it asks
from 127.0.0.2, which Linux answers for, so the master sees a second
source. Against a master elsewhere every port of this machine is the same
source, so that check is skipped and said so. It prints a progress line a
second, then:

```text
flood 127.0.0.1:26911 for 10 s from 32 ports at 2000 datagrams a second
sent 19999 datagrams (11168383 bytes): browse 1250, details 1250, ...
answered 293 (16102 bytes): challenge 11, listingdetails 30, page 21, probeanswer 43, unknownlisting 66, unsupported 122
ports answered with more bytes than they sent: 0 (the largest share of its own bytes one port got back: 0.002)
a browse from another address during the flood: answered 10 of 10
limits held
```

It exits 0 when the limits held (no port got more bytes back than it sent,
and the other address's browse, if it ran, was answered at least half the
time), 1 when not. The master's own status lines during the flood show
`dropped(limit)` rising and one `limit` line for the source.

## The configuration file

One setting per line, a name and a value; `#` starts a comment. An unknown
name, a value out of range, or a name given twice is refused with its line,
as the dedicated server's file is ([its rules](DEDICATED-SERVER.md#the-configuration-file)).
Numbers may be written with thousands commas (`100,000`).

| Setting | Values | Default | Meaning |
| --- | --- | --- | --- |
| `listen` | `any` or one address | `any` | Where both ports listen; `any` is every IPv4 and IPv6 address |
| `port` | 1 to 65535 | 26901 | The main UDP port: listings, browsing, introductions, the relay, reports |
| `probe-port` | 1 to 65535, or 0 | 26902 | The second port of the router test; 0 turns the test off (games then never get the relay at once, only after the race). Games send the test's second probe to the main port + 1, so keep it there (agent decision, slices I2 and I3) |
| `state-dir` | a folder | the configuration file's folder | Where the statistics, telemetry counts and the relay's monthly figure are written |
| `max-listings` | 1 to 100,000 | 2,000 | Listings at once |
| `listings-per-source` | 1 to 1,000 | 8 | Listings from one IPv4 address or IPv6 /64 network |
| `heartbeat` | 10 to 120 seconds | 30 | The interval the master tells hosts |
| `keep` | 5 to 60 seconds | 15 | How often hosts keep their router's mapping open |
| `expiry` | 30 to 255 seconds, at least twice `heartbeat` | 90 | A listing not heard from for this long is dropped (255 at most because Listed tells hosts the expiry in one byte; agent decision, I2) |
| `browse-rate` | 1 to 1,000 a second | 20 | Browse and details requests answered per source, with bursts of twice as many |
| `introduce-rate` | 1 to 100 a second | 4 | Introduce requests per source, before the cookie is checked. After it, 30 a minute per source and 10 a second per listing (fixed, agent decision, J2) |
| `answer-rate` | 100 to 100,000 a second | 5,000 | Answers of every kind together |
| `relay` | `on`, `off` | `on` | Whether to relay at all (John, 2026-10-05: on) |
| `relay-channels` | 0 to 1,000 | 64 | Relayed pairs at once |
| `relay-channels-per-source` | 1 to 30 | 2 | Relayed pairs one player's address may have |
| `relay-rate` | 8 to 1,024 KB/s | 128 | Each channel's limit, each way, with bursts of twice as much (1 KB is 1,000 bytes). 128 since 2026-10-06 (John, slice R1; 64 before): a relayed player's busiest second at 60 snapshots a second is 63 to 74 KB/s. A configuration file that sets `relay-rate` keeps its own value |
| `relay-month-gb` | 0 to 100,000 | 800 | Relayed gigabytes sent out each calendar month (UTC): new channels are refused from 95 percent of it, open ones closed at 100 (John, 2026-10-05) |
| `telemetry` | `on`, `off` | `on` | Whether to count the games' anonymous reports |
| `status-interval` | 0 to 3,600 seconds | 60 | How often the status line is written; 0 for never |

An example for the public master:

```text
# /etc/tore-master/master.conf
listen any
port 26901
probe-port 26902
state-dir /var/lib/tore-master
relay-month-gb 800
```

## Running it as a service

On Linux, a systemd unit, `/etc/systemd/system/tore-master.service`:

```ini
[Unit]
Description=T.O.R.E-Fighters master server
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/opt/tore-master/tore-master --config /etc/tore-master/master.conf
DynamicUser=yes
StateDirectory=tore-master
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
MemoryMax=256M

[Install]
WantedBy=multi-user.target
```

Then `sudo systemctl daemon-reload` and `sudo systemctl enable --now
tore-master`. `DynamicUser` runs it as a user of its own that exists only
while it runs; `StateDirectory` gives that user `/var/lib/tore-master`; the
ports are above 1024, so it needs no privilege at all.

## Ports and firewalls

| Port | Protocol | From | Why |
| --- | --- | --- | --- |
| 26901 | UDP | anywhere, IPv4 and IPv6 | Everything the game says to the master, and the relay |
| 26902 | UDP | anywhere, IPv4 and IPv6 | The second half of the router test |
| 22 | TCP | the operator's address | SSH, to look after the machine |

On the machine, with ufw: `sudo ufw allow 26901:26902/udp`. On Linode, a
Cloud Firewall in front of the machine needs the same two UDP ports allowed
inbound; outbound stays open. The master needs no other port. The game's
own port, 26900, is not used on the master machine.

## The name

The game is built with the master's name and port (and a player or server
operator can change it in Options, `--master`, or the server's `master`
setting). The public master is `master.jroverton.com`, port 26901 (John,
2026-10-05; deployed the same day), with an A record for the machine's IPv4
address and an AAAA record for its IPv6 address. With both records, games that have IPv6 reach
it over IPv6 too, which lets them learn their own IPv6 address for direct
connections. A short time to live (300 seconds) while setting up makes a
move to another machine quick. Moving the master later means pointing the
name at the new machine: games ask the name again every 10 minutes, and at
once and then every minute while the master is silent. A host that cannot
reach one of the addresses (a machine or container without an IPv6 route)
uses the other and does not stick on the dead one
([finding the master again](formats/master-protocol.md#finding-the-master-again)).

## Logs and statistics

- **Standard output** (the journal, under systemd: `journalctl -u
  tore-master`): the start lines (version, the sockets bound, the settings
  that differ from the defaults, then `Ready`), one status line every
  `status-interval`, and notable events: each listing made, moved and
  removed with why, a source over a limit (once a minute per source, with
  its address, or its /64 for IPv6, and the limit), each relay channel
  opened or closed (with the two ends' addresses and, on closing, why and
  its bytes each way), a listing's relay channels moving with it to a new
  host (stage K), each relay request refused, the allowance reaching 95 and
  100 percent, and problems writing the state folder.

  ```text
  status listings=41 sources=318 browse/s=2.4 introductions/min=7 punched=5 relayed=2 channels=3 relay-month=12.7GB dropped(limit)=0 invalid=4 in=812.3KB out=95.1KB
  listed id=4f1c2a9be07d3e11 from=203.0.113.5:26900 name="Friday night" listings=42
  moved id=4f1c2a9be07d3e11 from=203.0.113.5:26900 to=203.0.113.5:31877
  unlisted id=4f1c2a9be07d3e11 from=203.0.113.5:31877 name="Friday night" reason=expired listings=41
  limit source=198.51.100.7 over=browse (20 a second, bursts of 40)
  relay opened channel=d966e9a6 host=203.0.113.5:26900 player=198.51.100.20:40112 channels=3
  relay closed channel=d966e9a6 host=203.0.113.5:26900 player=198.51.100.20:40112 reason=closed by an end to-host=100384 to-player=378236 channels=2
  moved id=4f1c2a9be07d3e11 from=203.0.113.5:31877 to=198.51.100.33:26900
  relay moved listing=4f1c2a9be07d3e11 from=203.0.113.5:31877 to=198.51.100.33:26900 channels=1
  relay refused player=198.51.100.21:40007 result=too-many
  relay allowance: 95 percent of 800 GB relayed this month, so new channels are refused
  ```

  In the status line, `listings`, `sources` (remembered sources) and
  `channels` are as they stand; `browse/s` (Browse and Details answered),
  `introductions/min`, `punched`, `relayed`, `dropped(limit)` (requests over
  a source's or a listing's limit, and answers over `answer-rate`),
  `invalid` (datagrams that are not a master packet this master answers:
  damaged, malformed, another version, or one of the master's own kinds) and
  `in`/`out` (bytes) are over the time since the previous line.
  `introductions/min` counts the introductions made (J2); `relayed` the
  relay channels opened (J3), `channels` those open now, and `relay-month`
  the bytes relayed out this month with their headers (B, KB, MB or GB, a
  thousand each). `punched` stays 0: the master cannot see a punch, and
  the games' reports count their paths in the telemetry. A channel's
  `reason` is `closed by an end`, `idle`, `over its rate`, `allowance
  spent` or `the master is stopping`; `to-host` and `to-player` are the
  bytes of the frames it forwarded each way. A listing's `reason` is `unregistered`,
  `expired`, `replaced by a new registration` (the game restarted on the
  same port) or `another listing moved to its address`. The name is quoted
  with its quotes and control characters escaped. A `moved` line says a
  listing's token arrived from a new address: its host's router gave the
  port another outside address, or (stage K) another player's game took the
  mission over when its host was lost or left. A `relay moved` line follows
  it when the listing had relay channels: they now forward to the new
  address.
- **`state-dir/stats/YYYY-MM-DD.tsv`**: one line a minute with the same
  counts and a header line, for graphs. Kept 90 days, then deleted by the
  master.
- **`state-dir/telemetry/YYYY-MM-DD.tsv`**: the day's counts from the games'
  reports, one `name<TAB>number` line each ([what the master
  keeps](#what-the-master-keeps)), rewritten every minute and when the
  master stops, and read back when it starts again the same day. Kept until
  deleted by hand.
- **`state-dir/relay-YYYY-MM.txt`**: the month's relayed bytes, one
  number, written every minute and at `quit`, and read back at start so a
  restart does not forget them.

## What the master keeps

*Agent proposal; telemetry's defaults and contents are John's to decide.*

- **Listings** are kept in memory only, for as long as each host sends
  heartbeats. A master restart forgets them; every host lists itself again
  within one heartbeat.
- **Telemetry** is kept as counts per day: how many sessions, by role and in
  minutes, how long (under 5, 5 to 15, 15 to 30, 30 to 60, 60 minutes and
  up), how many humans, game versions (64 by name, the rest as `other`),
  platforms, how players connected (local, by address, mapped port, IPv6,
  punched, relay) and how long it took, a host's players by how they
  connected, how routers map, which port-mapping method worked, relayed
  kilobytes, and, from stage K, host migrations. The number of distinct
  installs a day is counted with a salt drawn each day and never written
  down, so an install cannot be followed from one day to the next; a master
  restarted during a day can count an install twice that day. No address is
  ever stored with telemetry. With `telemetry off` reports are read and
  dropped.
- **Addresses** appear only in the journal's lines about limits and relay
  channels, for dealing with abuse. *Agent proposal:* keep the journal 14
  days on this machine (`MaxRetentionSec=14day` in
  `/etc/systemd/journald.conf`, which applies to the whole machine).
- The game's README says what is sent and how to turn it off.

## The relay's monthly allowance

Relaying is the only part that costs transfer. Every relayed byte arrives
and leaves once; Linode counts outgoing transfer against the plan (check
this on the account). The master counts what it relays out each calendar
month, UTC:

- At 95 percent of `relay-month-gb` it refuses new channels ("The Internet
  Lobby's relay is full for this month."); at 100 percent it closes the open
  ones (John, 2026-10-05). Players who can connect directly are not
  affected. Each is logged once a month.
- It counts each relayed datagram it sends with its IP and UDP headers (28
  bytes over IPv4, 48 over IPv6), as the provider counts transfer (agent
  decision), so the figure is a little above the game's own bytes.
- The figure survives restarts (`relay-YYYY-MM.txt`) and starts again at
  zero on the first of the month.
- From the [plan's budget](multiplayer-plan.md#bandwidth-budget), a relayed
  player in a full mission costs about 180 MB an hour leaving the master at
  60 snapshots a second (John's default since 2026-10-06), so 800 GB is
  about 4,400 relayed player-hours a month, or 6 relayed players around the
  clock. The plan's 1 TB gives about 5,500 player-hours. Raising
  `relay-rate` to 128 changes none of this: it only caps a channel's busiest
  seconds.
- Set it to about 80 percent of the plan's monthly transfer, less anything
  else the machine serves, so the plan is never exceeded.

## Updating

The current master serves master protocols 1 and 2 together. Released 0.2.0
clients and servers keep working on protocol 1 with their original matching
rule. Protocol-2 clients see the separate game-protocol-71+ population and
match compatibility codes. Replies and relay setup use the appropriate
version. Upgrade this master before rolling out new games, without forcing
existing 0.2.0 servers to upgrade. No extra listener or configuration switch
is required. The [wire contract](formats/master-protocol.md#compatibility-codes-master-protocol-2)
describes both layouts and the frozen legacy golden.

1. Build the new `tore-master` or [download the release's](#downloading-a-release).
2. `tore-master --config /etc/tore-master/master.conf --check-config` with
   the new program. Read its `relay` line: from slice J3's build on it says
   `relay ACTIVE` with its limits (the default, John's choice), or `relay
   OFF` when the configuration turns it off.
3. `sudo systemctl stop tore-master`, replace
   `/opt/tore-master/tore-master`, `sudo systemctl start tore-master`.
4. `journalctl -u tore-master -n 20` shows the new start lines: the same
   `relay` line, and `relay this month (YYYY-MM): ... of 800 GB relayed`,
   the figure carried over from before the update.

Hosts list themselves again within one heartbeat (30 seconds) and browsers
refill. Relayed players are disconnected by the stop (SIGTERM cannot tell
them first; `quit` on the console would); stage K's rejoin brings them back.
The status line's `channels=` says how many are relayed at the moment, so
update when it is 0 if you can. The month's figure survives the update in
`relay-YYYY-MM.txt`.

**The first update with the relay (J3).** A master from before slice J3
read the `relay` settings and dropped every relay packet. The update needs
no change to `master.conf`: the relay is on by default. The machine's
firewall needs nothing new (the relay uses the main port, 26901). Watch the
first `relay opened` and `relay closed` lines in the journal, and the
`relay-month=` figure, which now grows.

**The update for host migration (stage K, slice K8).** What changed: when
a listing moves to a new address, its relay channels now move with it,
keeping their numbers and keys, and the journal says so in a `relay moved`
line after the `moved` line. A game that takes a migrated mission over
heartbeats with the old host's listing token from its own port, and the
master moves the listing there as it always has (at most once a minute per
listing). Nothing else changed: the master protocol is still version 1, no
packet changed, `master.conf` needs nothing new, and the firewall nothing
new. Games from before stage K are served as before. Update the master
before releasing a game with stage K's migration: an older master moves
the listing but leaves its relay channels at the dead host's address, so a
relayed player cannot follow the game to its new host and drops out until
its rejoin.

**Versions.** A master answers every master protocol version it knows, so
one master serves old and new games at once
([versions](formats/master-protocol.md#versions)). When a game release needs
a newer master protocol, update the master first, then release the game.

## Testing on one machine

Everything runs on one development machine without the public master, on
other ports than the public ones so nothing collides:

```text
# master.conf, beside the build
listen 127.0.0.1
port 26911
probe-port 26912
```

```sh
target/debug/tore-master --config master.conf
# A dedicated server that lists itself: in its configuration,
#   broadcast on
#   master 127.0.0.1:26911
target/debug/tore-server --config server.conf --data-dir "$TORE_DATA_DIR"
# The games it lists, headless:
target/debug/tore-app --browse 5 --master 127.0.0.1:26911
# A bot that joins it through an introduction (slice J2), and one through the
# relay (slice J3):
target/debug/tore-bot --master 127.0.0.1:26911 --listing "Friday night" --data-dir "$TORE_DATA_DIR"
target/debug/tore-bot --master 127.0.0.1:26911 --listing "Friday night" --path relay --data-dir "$TORE_DATA_DIR"
# The flood tool against the local master:
target/debug/tore-master flood 127.0.0.1:26911 10
```

The battery's net lane runs the last of these as `net-master-flood`, a
master, a listed server and a bot joining through an introduction as
`net-master-introduce`, and the same with `--path relay` as
`net-master-relay` ([the lane](testing/lane-net.md)); the Rust tests run
the master on the network simulator (`cargo test --locked -p tore-master`),
the punching table among them (`tests/punch.rs`, whose relay rows connect
through the relay), and the relay's own rules (`src/relay_tests.rs`).

In the game, set the master's address in the Internet Lobby's Options (or
start it with `--master 127.0.0.1:26911`). On one machine every direct path
works, so the relay is forced with `--path relay`; routers, punching and
their failures are tested on the network simulator in the Rust tests. Port
mapping is tested against a fake router on loopback; trying it on a real
router changes that router, so it is a manual test.

## Deploying at jroverton.com

What John does, once the slices are built. The choices in the first step are
his ([open questions](MULTIPLAYER.md#open-questions)); the rest follows.

1. **Decide** the machine (a new Linode 2 GB, the agent proposal, or the
   existing jroverton.com server), its region (near most players: relayed
   traffic goes through it), the name (`master.jroverton.com` proposed) and
   the ports (UDP 26901 and 26902 proposed).
2. **Create the machine:** Ubuntu 24.04 LTS on the 2 GB shared plan, an SSH
   key, the region chosen. Linode gives it an IPv4 and an IPv6 address.
3. **DNS:** an A record and an AAAA record for the name, pointing at those
   addresses, time to live 300.
4. **Firewall:** a Linode Cloud Firewall allowing UDP 26901 and 26902 from
   anywhere and TCP 22 from home; on the machine `sudo ufw allow OpenSSH`,
   `sudo ufw allow 26901:26902/udp`, `sudo ufw enable`.
5. **Install:** copy `tore-master` to `/opt/tore-master/` (built on the
   machine, or [downloaded](#downloading-a-release)), write
   `/etc/tore-master/master.conf` (the example above), install the unit
   ([as a service](#running-it-as-a-service)), `sudo systemctl enable --now
   tore-master`.
6. **Check:** `journalctl -u tore-master -f` shows the start lines and a
   status line a minute. From home: `tore-app --browse 5 --master
   master.jroverton.com:26901` answers (with no games yet), and `tore-master
   flood master.jroverton.com:26901 10` reports the limits held (from
   outside, its check of a browse from a second address is skipped: every
   port at home is one source to the master).
7. **Tell the lead** the name and port: the game's default master address is
   set to it, and from that build on the Internet Lobby uses it.
8. **Each month,** glance at the plan's transfer in the Linode Cloud Manager
   and the master's `relay-month=` figure.

The record of the first deployment, on 2026-10-05, with its flood result and
the manual checks still open, is
[the deployment baseline](baselines/master-2026-10-05.md).

## When something is wrong

| What a player sees | Look at |
| --- | --- |
| "The Internet Lobby does not answer" | Is the service running (`systemctl status tore-master`)? Does the name resolve to the machine (`dig master.jroverton.com A` and `AAAA`)? Are UDP 26901 and 26902 open in both firewalls? |
| A hosted game is not in the list | The host's game says why in its lobby's Messages. The host's machine may block the game's outgoing UDP, or the master refused it for a limit (the journal says which source) |
| Listed, but nobody can join | Most joins fall back to the relay; "relay is full" or "switched off" means the relay's settings or the allowance. If even the relay fails, the host's router dropped its mapping: the game keeps it open every 15 seconds, so this points at an unusual router |
| Joins work, but slowly | A relayed player's delay is the trip to the master and on to the host; a master far from both adds it twice |

## Security

- It runs as an unprivileged user of its own, on ports above 1024, writes
  only its `state-dir`, and reads nothing but its configuration file.
- It speaks only UDP, decodes nothing it cannot bound, never answers an
  unproven sender with more bytes than it sent, and limits every source and
  every kind of request ([master protocol](formats/master-protocol.md#security)).
  `tore-master flood` lets the operator see that for themselves.
- Traffic is not encrypted, as with the game's own: listing tokens and relay
  keys can be read by anyone who can watch the network between a game and
  the master. That lets such a person remove or change a listing, nothing
  more; the game's password and the host's checks are unchanged by the
  master.
- If the master is attacked or down, the Internet Lobby is empty and
  relayed players are disconnected. Local games, Direct Connection and
  joining by address work as before.
