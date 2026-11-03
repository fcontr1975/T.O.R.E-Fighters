# Tanks against aircraft

> **T.O.R.E: Tasteful Opinionated Reverse Engineered.**
> The thing being reverse engineered is the *experience*, not the executable. We
> trace what a player does and what the game does back, down to the numbers they
> would notice. How the original code achieved it is history: useful evidence,
> never a blueprint. If a sentence below reads like an instruction to reproduce
> the original's internals, it is out of date.
> <!-- tore-header v2 -->

Implementation mode, 2026-10-24, branch `rc2-tanks` from `0.2.2-rc1` (950bb916,
protocol 73). Contract: [tanks against
aircraft](../spec/surface-defenses.md#tanks-against-aircraft). John's request:
cut the tanks' accuracy against aircraft by about two thirds with realistic
mechanisms, and stop them firing more than about 20 degrees up.

## What was run

- **Build.** Release `tore-app`, Linux x86_64 (Ryzen 9 7900X); before is
  950bb916, after is the branch.
- **Data.** A fresh import of the retail 1.02F media.
- **The probe.** `--surface-trace IRA QIRRETR --over T72 --solo --skill N --rng
  SEED`: Iraq's withdrawing column, flat desert, its tanks standing still; one
  T-72 armed (its retail records: the 2A46, fire zone 50 to 5,000 ft and to
  3,000 ft up, one round every 7.25 s), every other unit's weapons laid down.
  One fresh world per pass, so each pass meets the tank's first (unready)
  preparation. An AH-64 or F-16C flies a scripted straight line. A hovering
  helicopter holds 60 s. A moving one starts where the tank's preparation ends
  between one loading cycle before the aircraft reaches the gun's 5,000 ft
  reach and one cycle after, spread evenly over the seeds, so half the passes
  meet a tank already waiting at the edge of its reach (an approach seen from
  afar) and half meet it at a random point of its cycle.
- **Counted.** A pass is a kill when the aircraft is destroyed by a tank round.
  One 125 mm hit does 200, which kills the AH-64 and the F-16C, so a kill is a
  hit. Only the tank's hits count (a hit within 1.5 s of one of its rounds):
  `--solo` does not lay down the theater's SAM batteries, and at novice a slow
  pass now and then took a missile late in its run (11 such hits, excluded).
- **Passes.** 60 a row at average experience, 40 at ace and at novice. The
  "cap only" column is a build with only the 20 degree ceiling (lay, ranging
  error and tracking lag set to 0), to show what the ceiling alone does.
- **Ranges.** The gun's fire zone ends at 5,000 ft (1.5 km), so the 2 km hover
  and the 4 km pass are out of reach before and after; they show that, and the
  means leave them out.

## Results

Kill a pass, the share of passes in which the tank killed the aircraft.

### Average experience (skill 1)

60 passes a row.

| Profile | Rounds a pass, before | Rounds, after | Kill a pass, before | Cap only | After | Cut |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Helicopter hover 300 ft, 1.0 km | 4.9 | 5.5 | 33% | 33% | 18% | 45% |
| Helicopter hover 1000 ft, 1.0 km | 4.8 | 5.3 | 43% | 43% | 22% | 50% |
| Helicopter hover 300 ft, 1.5 km | 5.6 | 5.8 | 13% | 13% | 10% | 25% |
| Helicopter hover 1500 ft, 1.0 km | 4.7 | 0.0 | 48% | 0% | 0% | 100% |
| Helicopter hover 300 ft, 0.5 km | 2.5 | 3.6 | 98% | 98% | 80% | 19% |
| Helicopter hover 300 ft, 2.0 km | 0.0 | 0.0 | 0% | 0% | 0% | n/a |
| Helicopter 100 kt 300 ft, 1 km abeam | 4.8 | 5.3 | 38% | 38% | 13% | 65% |
| Helicopter 120 kt 1000 ft, overhead | 3.2 | 0.9 | 95% | 10% | 0% | 100% |
| Helicopter 80 kt 500 ft, 0.5 km abeam | 4.5 | 7.4 | 92% | 92% | 32% | 65% |
| Jet 450 kt 200 ft, overhead | 1.7 | 0.6 | 68% | 10% | 3% | 95% |
| Jet 450 kt 500 ft, 1 km abeam | 1.6 | 0.9 | 12% | 12% | 0% | 100% |
| Jet 500 kt 300 ft, 0.5 km abeam | 1.6 | 1.0 | 18% | 18% | 0% | 100% |
| Jet 400 kt 2500 ft, overhead | 1.6 | 0.0 | 15% | 0% | 0% | 100% |
| Jet 400 kt 1500 ft, 1 km abeam | 1.6 | 0.0 | 12% | 0% | 0% | 100% |
| Jet 500 kt 500 ft, 4 km abeam | 0.0 | 0.0 | 0% | 0% | 0% | n/a |
| **Mean of the 13 profiles in reach** | | | **45%** | 28% | **14%** | **70%** |

Helicopter profiles: mean kill a pass 58% before, 22% after, a cut of 62%.

Jet profiles: mean kill a pass 25% before, 1% after, a cut of 97%.

### Ace (skill 3)

40 passes a row.

| Profile | Rounds a pass, before | Rounds, after | Kill a pass, before | Cap only | After | Cut |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Helicopter hover 300 ft, 1.0 km | 3.0 | 5.2 | 92% | 92% | 40% | 57% |
| Helicopter hover 1000 ft, 1.0 km | 2.8 | 4.5 | 95% | 95% | 62% | 34% |
| Helicopter hover 300 ft, 1.5 km | 4.5 | 6.1 | 68% | 68% | 22% | 67% |
| Helicopter hover 1500 ft, 1.0 km | 2.7 | 0.0 | 95% | 0% | 0% | 100% |
| Helicopter hover 300 ft, 0.5 km | 1.7 | 2.4 | 100% | 100% | 98% | 3% |
| Helicopter hover 300 ft, 2.0 km | 0.0 | 0.0 | 0% | 0% | 0% | n/a |
| Helicopter 100 kt 300 ft, 1 km abeam | 3.2 | 4.8 | 85% | 85% | 25% | 71% |
| Helicopter 120 kt 1000 ft, overhead | 2.4 | 0.9 | 100% | 48% | 5% | 95% |
| Helicopter 80 kt 500 ft, 0.5 km abeam | 2.8 | 5.6 | 100% | 100% | 65% | 35% |
| Jet 450 kt 200 ft, overhead | 1.6 | 0.6 | 90% | 25% | 5% | 94% |
| Jet 450 kt 500 ft, 1 km abeam | 1.5 | 0.9 | 30% | 30% | 0% | 100% |
| Jet 500 kt 300 ft, 0.5 km abeam | 1.5 | 1.0 | 48% | 48% | 0% | 100% |
| Jet 400 kt 2500 ft, overhead | 1.4 | 0.0 | 42% | 0% | 0% | 100% |
| Jet 400 kt 1500 ft, 1 km abeam | 1.4 | 0.0 | 38% | 10% | 0% | 100% |
| Jet 500 kt 500 ft, 4 km abeam | 0.0 | 0.0 | 0% | 0% | 0% | n/a |
| **Mean of the 13 profiles in reach** | | | **76%** | 54% | **25%** | **67%** |

Helicopter profiles: mean kill a pass 92% before, 40% after, a cut of 57%.

Jet profiles: mean kill a pass 50% before, 1% after, a cut of 98%.

### Novice (skill 0)

40 passes a row.

| Profile | Rounds a pass, before | Rounds, after | Kill a pass, before | Cap only | After | Cut |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Helicopter hover 300 ft, 1.0 km | 4.8 | 4.8 | 15% | 15% | 10% | 33% |
| Helicopter hover 1000 ft, 1.0 km | 4.6 | 4.7 | 20% | 20% | 12% | 38% |
| Helicopter hover 300 ft, 1.5 km | 5.0 | 4.9 | 5% | 5% | 5% | 0% |
| Helicopter hover 1500 ft, 1.0 km | 4.6 | 0.0 | 20% | 0% | 0% | 100% |
| Helicopter hover 300 ft, 0.5 km | 3.6 | 4.3 | 65% | 65% | 38% | 42% |
| Helicopter hover 300 ft, 2.0 km | 0.0 | 0.0 | 0% | 0% | 0% | n/a |
| Helicopter 100 kt 300 ft, 1 km abeam | 5.3 | 5.3 | 20% | 20% | 10% | 50% |
| Helicopter 120 kt 1000 ft, overhead | 4.2 | 0.9 | 65% | 5% | 0% | 100% |
| Helicopter 80 kt 500 ft, 0.5 km abeam | 6.4 | 7.7 | 60% | 60% | 25% | 58% |
| Jet 450 kt 200 ft, overhead | 1.8 | 0.0 | 22% | 8% | 0% | 100% |
| Jet 450 kt 500 ft, 1 km abeam | 1.0 | 0.9 | 0% | 0% | 0% | n/a |
| Jet 500 kt 300 ft, 0.5 km abeam | 1.0 | 1.0 | 18% | 18% | 0% | 100% |
| Jet 400 kt 2500 ft, overhead | 1.6 | 0.0 | 15% | 0% | 0% | 100% |
| Jet 400 kt 1500 ft, 1 km abeam | 1.6 | 0.0 | 10% | 0% | 0% | 100% |
| Jet 500 kt 500 ft, 4 km abeam | 0.0 | 0.0 | 0% | 0% | 0% | n/a |
| **Mean of the 12 profiles in reach** | | | **28%** | 18% | **8%** | **70%** |

Helicopter profiles: mean kill a pass 34% before, 12% after, a cut of 63%.

Jet profiles: mean kill a pass 16% before, 0% after, a cut of 100%.

## What these runs show

- The cut is about two thirds at every experience: the mean kill a pass falls
  70 percent at average, 67 percent at ace and 70 percent at novice.
- The ceiling alone takes a little over a third of it (average: 45 to 28
  percent). It ends the shots a tank made as a low jet or a helicopter passed
  over it, its best ones, and every shot at an aircraft high above it: a
  helicopter 1,500 ft up 1 km out is about 24 degrees up and is never engaged,
  and a jet 2,500 ft over a tank is always above 20 degrees inside the gun's
  reach.
- The rest comes from the lay, the ranging error and the tracking lag. A fast
  jet is now very unlikely to be hit at all (1 percent a pass on average,
  from 25), because the lag adds several degrees at its crossing rate and it is
  through the gun's reach before the lay is done. A helicopter hovering close
  is still in real danger (0.5 km: 80 percent a pass at average, 98 at ace;
  1 km: about a fifth at average): it does not cross the sight, so only the
  ranging error and the lay apply.
- No round is wasted on an aircraft out of the ceiling: the rounds a pass go
  down for the jets and the 1,500 ft hover, and up only where the tank lives
  long enough to keep shooting at a helicopter it no longer kills.
- The M1 fires the same aim code (one round every 9.75 s); it was not in a
  probe here, since no template stands an M1 alone against a red player
  (Iraq's American objects stay friendly to the player, and the front line's
  M1 takes the Red tanks first).

## Battery

`surface-tank-aa` holds the rules on this probe: no round at a helicopter 24
degrees up and a full magazine, the first round at a low hovering helicopter 4
s after the lock (1 s and the 3 s lay), every round at a 450 kt jet 200 ft over
the tank at 20 degrees or less, and none at a jet 2,500 ft over it. It fails on
the old build.
