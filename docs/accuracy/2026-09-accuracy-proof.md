# Is vgc-engine's disagreement with Showdown just RNG? (2026-09)

**Verdict: no.** When every random outcome is forced to match Pokémon
Showdown's, the engine still diverges from PS in 84% of real-game Reg M-C
battles. About 94% of those first divergences are deterministic mechanics
bugs; only about 5% are RNG plumbing. Two independent checks agree: the
engine running on PS's own PRNG, and outcome-distribution tests.

The engine's RNG primitives are correct:
- a bit-exact PS PRNG port;
- damage-roll, crit, accuracy, secondary, multi-hit, confusion and speed-tie
  distributions that match PS.

What is wrong is how many moves, abilities, items and turn-order rules are
wired. The ~14% replay-corpus figure is a separate matter. It is dominated
by hidden information, not RNG: PS itself only reproduces 33% of real turns
from reconstructed teams.

| | result |
|---|---|
| Forced-RNG conformance, full-info Reg M-C battles | **16.5%** fully clean (214/1,298, 95% CI 14.6–18.6%); **78.7%** of turns match given a matching start (CI 77.5–79.8%) |
| First divergence: mechanics / RNG plumbing / decision model / harness | **93.6% ± 6.1 / 5.3% ± 5.8 / 1.1% ± 2.0 / 0%** (≤ 5.9% upper bound), triage-calibrated |
| Exact PS PRNG replication | **Yes**: Sodium/ChaCha20 and Gen5 LCG bit-exact (`--features ps-rng`) |
| Seeded differential on PS's PRNG | 0.7% of battles fully match. Where the draw streams are aligned through a turn, 86.9% of turns match (412/474). **62 battles diverge on mechanics with bit-identical random numbers**, 55 of them at the same turn, slot and field as the keyed run |
| RNG distributions (8 scenarios, 10k runs each) | damage roll, crit, accuracy 70/90%, 10% burn, 2–5 multi-hit, speed tie and confusion rate all match (p ≥ 0.2). **Full paralysis 25.9% vs PS 12.2%** (Champions uses 1/8) |
| Real logs, turn 1, Phase-2 rule (±5% HP) | PS itself 32.6%; engine 30.2%. Forcing the visible crits and misses adds about 2.5 points; PS-vs-PS with perfect information but its own damage rolls gets 77.7% |

---

## 1. The question and why it needs a better test

The owner's hypothesis: the gap between vgc-engine and PS is mostly
different RNG, not wrong mechanics. That covers a different PRNG, a different
number or order of draws, and different damage-roll or speed-tie handling.

Existing signals can't settle it:
- The replay-corpus scorer (~14%) mixes three things: hidden information
  (spreads, unrevealed items and abilities), unforced randomness and
  mechanics.
- The keyed conformance harness (`docs/ps-comparison-harness-design.md`)
  used random teams and random choices, and it stopped at the first faint.

This study adds four experiments. Each is designed so that one explanation
can be switched off at a time.

## 2. Sample

- **Real games.** 3,000 spectated `gen9championsvgc2026regmc` ladder logs,
  drawn uniformly at random (`shuf --random-source=<(yes 42)`) from hil's
  collector, 2026-09-16 to 2026-09-28. 2,744 lasted ≥ 3 turns; the first
  1,300 of those in the random order form the study sample.
- **Full-information battles** (`tools/accuracy/recon.js`). For each log,
  each side's four brought Pokémon are rebuilt:
  - revealed moves, items and abilities from the log;
  - gaps filled from usage counts over the 3,000 logs;
  - a role-based Champions Stat Point spread with per-game Speed variety.

  PS (`ps-battle.js`) then plays the real turn-by-turn choices while they
  are legal and seeded random legal choices after that. Across the sample,
  48% of decisions came from the real log. Battles run to completion
  (median 8 turns). PS and the engine get identical teams, so hidden
  information cancels out of every engine-vs-PS comparison. PS is
  smogon/pokemon-showdown `a5df8274` with its Champions mod, format
  `gen9championsvgc2026regmc`.
- 1,298 of 1,300 battles replay. Two fail to load because the engine lacks
  the cosmetic forme `Alcremie-Salted-Cream`.

## 3. Experiment 1 — forced-RNG conformance (keyed)

**Method.** `accuracy keyed` replays every captured PS battle in the engine:
- Every PS random outcome (accuracy, crit, damage roll, secondary,
  multi-hit, durations, gates) is injected by semantic key
  `(turn, actor, target, move, decision)`, per
  `docs/conformance-key-contract.md`.
- Speed-tie shuffles are recovered from PS's raw PRNG trace.
- A repair pass pairs any engine draw that missed the table with an
  unconsumed PS outcome from the same move use, when their labels differ.
- Unlike the old runner, it plays through:
  - faint replacements and mid-turn pivots (`Battle::decision_phases`);
  - end-of-turn state after replacements: species, HP, faint, status, item,
    ability and all seven boosts per active slot, plus weather, terrain,
    rooms, screens, Tailwind and hazards.
- On the turn PS ends the game, only faints are compared, because PS stops
  mid-turn and the engine finishes the turn.

**Results (n = 1,298).**

- Fully clean to the end of the battle: **214 (16.5%, Wilson 95% CI
  14.6–18.6%)**. All 214 ran to the natural end.
- Turns matched before the first divergence: 4,004 of 5,088 compared, so
  the per-turn agreement given an agreeing start is **78.7% (CI
  77.5–79.8%)**. The median first divergence is on turn 3.

If RNG were the explanation, forcing it would make nearly every battle
clean. It makes one in six clean.

## 4. Experiment 2 — what the first divergence is

**Automated split.** Each diverged battle is replayed under five other
fallback streams; only draws PS never supplied use the fallback stream.
- A divergence that moves is **RNG-sensitive**: an unforced draw caused it.
- A divergence that doesn't move is **deterministic**: it is a mechanics or
  decision difference.
- Deterministic cases with Eject Button, Eject Pack, Red Card, Emergency
  Exit, Wimp Out or Revival Blessing on the turn are flagged separately as
  **decision model**, because the engine auto-picks those replacements.

| class | battles | share |
|---|---|---|
| deterministic | 910 | 84.0% |
| RNG-sensitive | 101 | 9.3% |
| decision-model flag | 73 | 6.7% |

**Calibration by hand triage.** Every flagged case was checked against the
PS and engine source.
- Two earlier rounds (106 cases) found seven harness artifacts, all fixed
  before the final data:
  - Champions ignores IVs, but recon emitted `IVs: 0 Spe`;
  - mid-turn Eject Button switches were replayed as turn-start switches;
  - bool gates were keyed as crits;
  - bool-gate convention was inverted;
  - a replacement step ran when the engine didn't need one;
  - status-move draws had no key context;
  - `Battle.sample` draws were not keyed.
- A final stratified random sample of 51 cases from the final run:

| stratum | sampled | mechanics | RNG plumbing | decision model | harness |
|---|---|---|---|---|---|
| deterministic | 30 | 29 | 1 | 0 | 0 |
| RNG-sensitive | 15 | 11 | 4 | 0 | 0 |
| decision-model flag | 6 | 5 | 0 | 1 | 0 |

Stratum-weighted, the 1,084 first divergences split:

| cause | share of diverged | battles | share of all battles |
|---|---|---|---|
| mechanics | 93.6% ± 6.1 | ~1,015 | 78% |
| RNG plumbing | 5.3% ± 5.8 | ~57 | 4.4% |
| decision model | 1.1% ± 2.0 | ~12 | 0.9% |
| harness | 0% (95% upper bound 5.9%) | 0 | 0% |

Even most "RNG-sensitive" cases are mechanics: 11 of 15. The engine did
something PS didn't (Fake Out failing, no dynamic re-sort, Wide Guard's
stall roll) and drew a random number for it.

**What the RNG plumbing actually is.**
- Rolls made outside a move keep a stale key: Moody, freeze thaw, Yawn's
  sleep length.
- A few speed ties.
- Protect's stall and full-paralysis gates use a different draw shape from
  PS.

None of these is a wrong probability.

**Top offenders.** The table below is the effect that last changed the
diverged slot, from `tools/accuracy/analyze.py`. "Rate" is divergences per
turn the effect appeared in.

| effect | battles | rate | root cause (confirmed in triage) |
|---|---|---|---|
| recoil | 241 | 44% | recoil (and drain) use uncapped damage when the hit KOs |
| Grassy Terrain | 187 | 14% | terrain effects gated on the defender being grounded; Grassy Glide priority not recomputed |
| Life Orb | 132 | 9% | co-occurs with recoil; plus Life Orb rounded outside the modifier chain |
| Expanding Force | 108 | 42% | never becomes a spread move in Psychic Terrain |
| Sitrus Berry | 67 | 18% | pinch berries not checked after recoil, Rocky Helmet or end-of-turn damage |
| Fake Out | 60 | 8% | first-turn check uses `turns_active` (PS: `activeMoveActions`), so it fails the turn after any switch-in |
| Electro Shot | 45 | 38% | rain +1 SpA applied to the live mon but damage uses the pre-boost snapshot |
| drain | 31 | 26% | same uncapped-damage bug as recoil |
| Poison Touch | 17 | 19% | ability procs roll before the move's secondaries (PS: after) |

Per-mechanic counts in the 51-case final triage:

| mechanic | cases |
|---|---|
| recoil/drain on KO | 7 |
| Fake Out | 6 |
| Expanding Force | 4 |
| terrain gated on defender | 4 |
| no dynamic speed/priority re-sort | 4 |
| Champions `slicing` flags (Shadow Claw) | 3 |
| Emergency Exit missing | 3 |
| berries after recoil | 2 |
| each (1 case) | Thermal Exchange, Gooey, Skill Swap re-trigger, resist berry vs -ate type, Upper Hand, Rain Dish, Wide Guard stall roll, ejected mon's queued move run by its replacement, lead-ability order, Poison-type Toxic accuracy, Eject Button before secondaries, Life Orb rounding |

Earlier rounds added:
- Scrappy, Glaive Rush, Baton Pass, Last Resort and Explosion self-KO are
  missing;
- Feint vs Protect, Sucker Punch retarget, Steel Roller, Mental Herb vs
  Disable, Symbiosis passing a mega stone;
- weather chip on its expiry turn, replacement switch-ins firing abilities
  one at a time;
- secondaries skipped on a KO;
- missing Champions move deltas (Snipe Shot 85 BP and others);
- Champions status odds: paralysis 1/8, sleep `sample([2,3,3])`, freeze 1/4
  with a 3-turn cap.

The full list with evidence is in §9.

**Minimal repros** (`tools/accuracy/repros/run.sh`; each scripted, each a
deterministic divergence):

| repro | what happens | engine | PS |
|---|---|---|---|
| `recoil-uncapped-on-ko` | Incineroar's Flare Blitz KOs a Rage-Powdering Amoonguss | 85/202 | 129/202 |
| `expanding-force-no-spread` | Indeedee in Psychic Terrain | hits one foe | hits both; Garchomp 67 vs 104 |
| `terrain-boost-gated-on-defender` | grounded Rillaboom's Grassy Glide into airborne Pelipper | Pelipper 119 | Pelipper 106 |
| `fake-out-after-pivot-switch-in` | Rillaboom enters via Parting Shot, then Fake Outs next turn | Fake Out fails; Tailwind goes up | Corviknight flinches; no Tailwind |
| `full-paralysis-rate` (distribution scenario) | Kingambit fully paralysed | 25.9% | 12.2% |

## 5. Experiment 1b — the replay-corpus number is hidden information, not RNG

The Phase 2 gate is written against real logs: ≥ 80% of turns within ±5%
HP. Can any simulator reach that? `tools/accuracy/realgap.py` compares, at
the end of turn 1, the four on-field Pokémon's HP% and faint status with the
real log:

| simulator | whole turn agrees | per Pokémon |
|---|---|---|
| PS, reconstructed teams, own RNG | 32.6% (CI 30.1–35.2) | 71.2% |
| PS, RNG forced to the log's crits, misses and secondary effects | 35.2% (32.7–37.8) | 72.9% |
| engine (keyed to that PS battle), own RNG | 30.2% (27.8–32.8) | 69.4% |
| engine, forced | 32.4% (29.9–35.0) | 70.9% |
| **PS vs PS**, identical full info and forced outcomes, different seed | **77.7%** | 93.6% |

So on turn 1, starting from 100%:
- damage rolls and other unforced randomness cost about 22 points;
- reconstructing hidden information costs about 43 more;
- engine mechanics cost about 3.

That is why the corpus score sat near 14% and didn't move with mechanic
fixes. The Phase 2 gate is unreachable on spectated logs even for PS.
Forcing the visible RNG buys only about 2.5 points; the gap is the teams.

## 6. Experiment 3 — seeded differential on PS's own PRNG (`ps-rng`)

**Flag.** `vgc-engine-core` feature `ps-rng` (off by default; see
[`ps-rng.md`](ps-rng.md)).
- Adds `Rng::Ps`, a bit-exact port of PS `sim/prng.ts` at `a5df8274`: the
  Sodium/ChaCha20 default and both Gen5 seed formats, with PS's
  `random`/`randomChance`/`sample`/`shuffle` semantics.
- Adds PS-mode emulation of PS's structural draws:
  - `resolveAction` and `getTarget` random targets;
  - `selfDrops` rolls;
  - `insertChoice`, `runSwitch`, `eachEvent('Update')` and queue re-sort
    speed ties;
  - the gen-8+ re-sort `getTarget` calls.
- Adds a per-draw trace with the engine call site (`#[track_caller]`).
- PS's side is traced at `PRNG.random` with its JS stack.
- pyo3: `Battle.from_teams(..., ps_seed=...)` and `battle.ps_seed`.
- Cost: +2.5% per step with the feature compiled in but unused; 2.6× per
  step on `Rng::Ps`.

**Results (same 1,298 battles, engine seeded with each battle's PS seed).**

- Bit-exact PRNG: passes (reference vectors in `ps_rng::tests`). Battle
  start and turn 1 stay draw-for-draw aligned in 31% of battles (404/1,298).
- 9 battles match PS in full state (0.7%). Whole-battle bit-exact draw
  parity is not reached. PS draws random numbers in its event system, and
  the engine has no equivalent to hang them on. Ranked by how often each is
  the first divergent draw:

| first divergent PS draw | share | cause |
|---|---|---|
| secondary roll the engine doesn't make there | 17.5% | mostly mechanics: secondaries skipped on KO, missing effects, ability procs before secondaries |
| `useMoveInner` random target | 17.1% | mechanics: Expanding Force turning into a spread move |
| `fieldEvent` handler speed ties | 14.1% | PS sorts residual and switch-in *handlers*; there is nothing to emulate from engine state |
| spread-move accuracy order | 14.1% | PS rolls every target's accuracy before any crit or damage; the engine goes target by target |
| per-hit `eachEvent('Update')` ties in the Champions hit loop | 8.3% | not hooked |
| other | ~29% | long tail |

- **What the trace separates.** Classifying each battle by whether its first
  state divergence comes before its first draw divergence:

| order | battles |
|---|---|
| mechanics first, with bit-identical random numbers through the diverging turn | 62 |
| draw order first (an earlier turn) | 542 |
| same turn | 685 |
| no state divergence | 9 |

  For the 62 mechanics-first battles, the keyed run reports the same first
  divergence (turn, slot, field) in 55 cases. This is two independent
  methods agreeing.
- On turns where the two draw streams are identical through the end of the
  turn, the state matches in **86.9%** (412/474). The misses there are
  mechanics by construction.

Answering the flag's question directly:
- exact PRNG replication works;
- draw-order replication is partial and shows no sign of reaching full
  parity without re-creating PS's event-handler machinery;
- where the draws are identical, mechanics bugs still appear.

The keyed harness remains the right tool for measuring mechanics. `ps-rng`
serves as a microscope for draw-order questions (`accuracy trace`).

## 7. Experiment 4 — distributions (RNG-independent)

**Scenarios** (`tools/accuracy/dist/scenarios`, 10,000 runs per side; PS on
fresh Sodium seeds, the engine on SplitMix; chi-square homogeneity on
per-slot outcome tokens):

| scenario | PS | engine | p |
|---|---|---|---|
| Dragon Claw into Corviknight: 16 rolls × 1/24 crit | 15 outcomes, 158–179 | same | 0.64 |
| Rock Slide 90% accuracy, both targets | 90.1% / 90.1% | 90.0% / 90.3% | 0.27 |
| Hurricane 70% accuracy | 69.7% | 70.9% | 0.36 |
| Flamethrower 10% burn | 9.9% | 10.4% | 0.21 |
| Bullet Seed 2–5 hits | — | — | 0.36 |
| mirror speed tie, 50/50 | 49.8% | 49.6% | 0.84 |
| Swagger confusion: attack proceeds | 72.3% | 72.2% | rate matches; self-hit *damage* differs (mechanics) |
| **full paralysis** | **12.2%** | **25.9%** | 1e-115 |

The engine's randomness is right wherever the rule is right. The paralysis
failure is a Champions rule (`randomChance(1, 8)`, `mods/champions/conditions.ts`)
that the engine doesn't carry; the engine still uses 1/4. That is the one
place where "different RNG" is literally true, and it is a data-and-rule
bug, not a PRNG one. Champions sleep (`sample([2,3,3])`) and freeze (thaw
1/4, cap at 3 turns) are wrong the same way (engine: uniform 2–4 and 1/5).

**Real states.** 200 turn-1 states whose keyed replay matched PS exactly,
2,000 runs each:
- 19 of 251 slot tests reject at α = 0.01; 1% is expected under the null.
  There are 18 Benjamini–Hochberg discoveries, and a KS test of the p-values
  gives p = 1e-4.
- Pooled faint rates match: 2.45% (PS) vs 2.47% (engine).
- Every rejection inspected is a mechanics branch that the single keyed path
  never took:
  - full paralysis on a Trick Room setter (83% vs 71% TR up);
  - Dire Claw's status applied to a target the move never damaged;
  - a 1-HP rounding shift under Aurora Veil;
  - HP offsets on crit branches.

So the distributions match exactly where mechanics are correct, and differ
exactly where they aren't.

## 8. What this means for the Phase 2 gate

1. **Retire "≥ 80% turn agreement on the replay corpus"** as a correctness
   gate. PS itself scores 33% on turn 1 with reconstructed teams, and 78%
   against itself with perfect information but independent damage rolls.
   Any real-log gate measures recon quality and luck.
2. **Gate on the forced-RNG harness instead**, on this real-game Reg M-C
   sample: `accuracy keyed` over ≥ 1,000 battles. Today it stands at
   **78.7% per-turn agreement** and **16.5% of battles clean end to end**.
   Suggested bars:
   - ≥ 95% per-turn;
   - ≥ 80% clean battles;
   - 0 deterministic divergences in the committed repros.
3. **Work order.** Fix the top four mechanics first:
   - recoil/drain cap;
   - Fake Out's first-turn rule;
   - Expanding Force spread and terrain-by-attacker;
   - berries after self-damage.

   Together they are the proximate cause in 23 of the 51 final-triage
   divergences (45%). As the last effect on the diverged slot they show up
   in 241 (recoil), 187 (Grassy Terrain), 108 (Expanding Force), 67 (Sitrus
   Berry) and 60 (Fake Out) battles; these overlap. Then:
   - dynamic speed re-sort;
   - Champions status odds and move deltas;
   - Emergency Exit, Eject Button and Red Card as player decisions;
   - the long tail.

## 9. Engine issues found (for follow-up, not fixed here)

Mechanics (all verified against PS source; file references from triage):

- **Damage and effects**
  - Recoil/drain from uncapped damage on KO (`battle.rs` ~9346, 7107).
  - Terrain gated on the defender being grounded (`damage.rs:132`,
    `battle.rs:5559`).
  - Expanding Force spread missing (`damage.rs:1354`).
  - Electro Shot stale attacker snapshot (`battle.rs:8346`).
  - Secondaries skipped when the target faints (`secondary.rs:80`).
  - Ability onDamagingHit before move secondaries (`battle.rs:6951/6968`).
  - Life Orb rounded outside the modifier chain.
  - Resist berries check the move's base type, not the -ate type
    (`battle.rs:5938`).
- **Turn order and timing**
  - Fake Out uses `turns_active` (`battle.rs:8753`).
  - No gen-8+ dynamic speed or priority re-sort.
  - Lead abilities resolve p1-then-p2 instead of by speed (`battle.rs`
    `with_rng`).
  - Mega evolution order p1-then-p2.
  - Replacement switch-ins fire abilities one at a time.
  - A replacement counts as "switched in" the next turn (Speed Boost).
  - Pinch berries not checked after recoil, Rocky Helmet or end-of-turn
    damage.
  - Weather chip on its expiry turn (`battle.rs:10539`).
  - Sucker Punch doesn't retarget from a fainted slot.
  - An ejected mon's queued move is run by its replacement.
  - Two-turn moves forget their target.
- **Missing or wrong moves, abilities and items**
  - Thermal Exchange, Gooey, Scrappy, Rain Dish, Ice Body, Emergency Exit,
    Wimp Out, Baton Pass, Upper Hand's fail condition, Glaive Rush drawback,
    Last Resort's fail, Explosion self-KO, Flatter.
  - Feint vs Protect; Wide Guard stall roll; Steel Roller's no-terrain fail;
    Poison-type Toxic accuracy.
  - Mental Herb vs Disable; Symbiosis passing a mega stone; Skill Swap
    re-trigger for non-Intimidate abilities.
- **Champions data and rules**
  - Paralysis 1/8; sleep `sample([2,3,3])`; freeze 1/4 with a 3-turn cap.
  - Move deltas: Snipe Shot 85, Astral Barrage, Blood Moon, Dragon Hammer,
    Hyper Drill, Meteor Assault, Revelation Dance, Slash, Triple Dive, Snap
    Trap type, and `slicing` flags on claw moves (`vgc-engine-data/build.rs`
    ignores mod flags).
  - The team loader honours IVs that Champions ignores.
  - Unknown forme `Alcremie-Salted-Cream`.
- **Decision model**
  - Eject Button / Eject Pack auto-pick the first bench mon.
  - Red Card is deterministic (PS: random).
- **RNG plumbing** (no probability is wrong)
  - Keyed context is stale for non-move rolls (Moody, freeze thaw, Yawn).
  - The existing `Rng::PsGen5` returns PS's raw `random(16)` into the
    engine's `85 + bucket` damage formula, so its damage rolls are reversed;
    `Rng::Ps` flips it.
  - Confusion uses `percent ≤ 33` where PS uses `randomChance(1, 3)`
    (33.0% vs 33.3%).

## 10. Threats to validity

- **Reconstructed teams are plausible, not true.** Engine-vs-PS comparisons
  are unaffected (both get the same teams), but the mix of mechanics
  exercised follows what reconstruction produced. Choices follow the real
  game for 48% of decisions, then random legal play.
- **Keyed injection and the repair pass can mispair a draw.** The
  sensitivity test and hand triage bound this. No harness artifacts turned
  up in the final 51-case sample (95% upper bound 5.9%).
- **Only the first divergence per battle is classified.** Per-turn
  agreement conditions on a matching start.
- **The distribution tests use 2,000–10,000 runs.** Effects under about 1–2
  percentage points in rare branches would not be detected.

## 11. Reproduce

See [`tools/accuracy/README.md`](../../tools/accuracy/README.md) for setup:
a built PS checkout in `PS_DIST`, and a replay sample. Then:

```sh
cargo build --release -p vgc-engine-conformance --features ps-rng
node tools/accuracy/recon.js mc $W/jobs.jsonl
node tools/accuracy/ps-battle.js $W/jobs.jsonl $W/b --count 1300
target/release/accuracy keyed $W/b --jsonl $W/keyed.jsonl && python3 tools/accuracy/analyze.py keyed $W/keyed.jsonl $W/b   # exp 1+2
target/release/accuracy psrng $W/b --jsonl $W/psrng.jsonl                                                               # exp 3
# exp 1b (real-log ceiling) and exp 4 (distributions): commands in tools/accuracy/README.md
tools/accuracy/repros/run.sh                                                                                            # repros
```
