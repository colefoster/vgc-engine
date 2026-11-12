# Lossy solver moves: outcome classes, sampled chance, menu-restricted root (2026-11-12)

Three opt-in speed-ups for the anytime endgame solver. All three are off by
default; with them off every existing test and golden passes unchanged
(`cargo test --release --workspace --exclude vgc-engine-py`, the
budget-guarantee tests in Rust and Python).

| move | `SolverConfig` field | `SolverStats` | Python (`solve_endgame_anytime`) |
|---|---|---|---|
| 1. outcome-class chance | `outcome_classes: bool` | `class_cells`, `class_sites`, `class_count`, `class_fallbacks` (+ per reason), `class_lazy_unclassified` | `outcome_classes=False` |
| 2. sampled chance | `chance_samples: Option<u32>` | `sampled_cells`, `samples` | `chance_samples=None` |
| 3. menu-restricted root | `root_menu: Option<RootMenu>` | `menu_rows`, `menu_cols`, `menu_fallbacks` | `row_menu`, `col_menu`, `row_support`, `col_support` (default `None`) |

## Verdict

| move | accuracy vs brute force (401 positions) | speed | keep? |
|---|---|---|---|
| 1. outcome classes | mean abs dv 0.0042 vs served 0.0048; same max (0.2434) and exploitability | 13% fewer replays on the brute set; at 1.5 s on 2v2s, mean completed depth 2.12 vs 1.88 | yes, off by default; worth a play test |
| 2. sampled chance | worse: mean abs dv 0.0129 (N=64) / 0.0239 (N=16), max 0.22 / 0.60, more exploitable roots | deeper at 1.5 s (3.25 at N=16), but the depth buys noise, not accuracy | kept as an option; not recommended for serving |
| 3. menu root | exact once the menu holds the equilibrium support (k >= 3 on 2v2s); k=1 off by up to 0.94 | k=4 solves a depth-1 2v2 in 16% of the unrestricted time; at 1.5 s menu2 completes depth 2.72 vs 1.88 | yes; the gain depends on menu quality |

The largest served error (0.24 on brute #25 and #33) is within-bucket HP at a
depth-limited leaf (gap 12 of `docs/solver_coverage_map.md`): `default` has it
too, and Move 1 does not touch it. Move 1 fixes the KO-mass part only.

## Measurements

Mac (M4 Pro), each run under `lockf /tmp/sp_heavy.lock` and an RSS watchdog
(3 GB). Nothing ran on unraid or colepc.

```
solver_harness brute 40 --seed S --max-depth 3      # S = 1..12
python3 scripts/solver_coverage_map.py --brute 'brute_s*.jsonl' \
    --configs exact_hp,default,lossy3,served_anytime,classes,sampled16,sampled64,menu1,menu2
solver_harness gen 30 --seed 7 > gen.jsonl          # + the 2 real budget_repro_2v2 battles
HARNESS_LOSSY=1 solver_harness real gen.jsonl --budget-ms 1500
solver_harness menus gen.jsonl --depth 1
```

### Brute-force agreement

401 positions (doubles 2v2 60, singles 1v1 116, 2v1 124, 2v2 101); 79 skipped
over the reference caps. Reference depth: d1 29, d2 44, d3 328. Same reference
and table shape as the coverage map. All lossy rows run the served anytime
config (`mass_eps` 0.02, `max_combos` 64) plus the one move; `sampled*` replaces
compression; `menuK` restricts the root to the top-K actions per side by the
unrestricted served solve's policy mass, then root value.

| config | value agrees (abs dv <= 1e-6) | agrees when ref all-terminal | max abs dv | mean abs dv | root policy exploitable > 1e-6 | panics |
|---|---|---|---|---|---|---|
| exact_hp | 401/401 (100.0%) | 62/62 (100.0%) | 0.0000 | 0.0000 | 0/401 (0.0%) | 0 |
| default | 353/401 (88.0%) | 60/62 (96.8%) | 0.2384 | 0.0021 | 23/401 (5.7%) | 0 |
| lossy3 | 334/401 (83.3%) | 60/62 (96.8%) | 0.2388 | 0.0029 | 28/401 (7.0%) | 0 |
| served_anytime | 182/401 (45.4%) | 50/62 (80.6%) | 0.2434 | 0.0048 | 45/401 (11.2%) | 0 |
| **classes** | 183/401 (45.6%) | 50/62 (80.6%) | 0.2434 | 0.0042 | 45/401 (11.2%) | 0 |
| **sampled16** | 169/401 (42.1%) | 50/62 (80.6%) | 0.6045 | 0.0239 | 73/401 (18.2%) | 0 |
| **sampled64** | 168/401 (41.9%) | 50/62 (80.6%) | 0.2219 | 0.0129 | 59/401 (14.7%) | 0 |
| **menu1** | 176/401 (43.9%) | 50/62 (80.6%) | 0.9413 | 0.0146 | 44/401 (11.0%) | 0 |
| **menu2** | 182/401 (45.4%) | 50/62 (80.6%) | 0.2434 | 0.0048 | 39/401 (9.7%) | 0 |

Real-RNG sampling: 1957 root cells, 391,400 seeded `step()`s, 0 outside the
enumerated frontier.

Work per solve on the brute set (anytime to the reference depth):

| config | mean ms | p90 ms | mean replays | reached ref depth |
|---|---|---|---|---|
| served_anytime | 2.1 | 4.3 | 358 | 351/401 |
| classes | 1.8 | 3.9 | 311 | 351/401 |
| sampled16 | 2.3 | 5.6 | 724 | 349/401 |
| sampled64 | 9.8 | 23.7 | 3208 | 350/401 |
| menu1 | 0.6 | 1.3 | 103 | 306/401 |
| menu2 | 1.4 | 3.1 | 245 | 346/401 |

Brute roots are small (2.4 x 2.1 legal joint actions on average), so menus of
3+ are the full list there; the menu measurement that matters is below.

### 1.5 s budget, late-game 2v2

**The real-position set (SP + kcheck-mt, 1031 positions) is not on this Mac**,
and rebuilding it needs a ps-rng wheel, which this task did not build. Proxy
set: 30 generated doubles 2v2 (`solver_harness gen 30 --seed 7`: Champions,
up to 4 moves per mon, 25-100% HP, 14 x 13 legal joint actions on average)
plus the 2 real 2v2 belief battles in `tests/fixtures/budget_repro_2v2.jsonl`.

| config | on time (<= budget+50 ms) | max overrun ms | depth >= 1 | mean completed depth | deeper than served | shallower | exact/terminal | median replays |
|---|---|---|---|---|---|---|---|---|
| served | 32/32 | 6.3 | 32/32 | 1.88 | - | - | 2 | 136,322 |
| classes | 32/32 | 0.2 | 32/32 | 2.12 | 3 | 0 | 3 | 134,594 |
| sampled16 | 32/32 | 0.6 | 32/32 | 3.25 | 23 | 2 | 7 | 338,918 |
| sampled64 | 32/32 | 0.4 | 32/32 | 2.44 | 16 | 1 | 2 | 347,750 |
| menu2 | 32/32 | 0.2 | 32/32 | 2.72 | 26 | 3 | 7 | 143,577 |
| menu4 | 32/32 | 0.1 | 32/32 | 2.03 | 15 | 3 | 6 | 134,480 |

There is no reference value for these positions, so the table is speed only.

### Move 1 fallback rates

| set | cells classified | fallback cells | unclassified | focus | roll-coupled HP | damage-reading | lazy partial |
|---|---|---|---|---|---|---|---|
| brute (all) | 1823 | 970 (35%) | 902 | 2 | 66 | 0 | 255 |
| brute singles | 1215 | 895 (42%) | 895 | 0 | 0 | 0 | 115 |
| brute doubles | 608 | 75 (11%) | 7 | 2 | 66 | 0 | 140 |
| 1.5 s 2v2 set | 21,274 | 13,703 (39%) | 4535 | 6802 | 2366 | 0 | 7876 |

Rates are over cells that had at least one unsegmented damage draw. The brute
singles fallbacks are Matcha Gotcha (drain, 191 of 204 sampled) and Population
Bomb (multi-hit). On the 2v2 set, focus fire is the largest reason: half of
the fallbacks are a defender taking two hits in one turn, which is exactly
where the 3-roll collapse is still used.

### Move 3: value vs menu size (same depth)

`solver_harness menus gen.jsonl --depth 1`: the 32 positions above, solved to
depth 1 unrestricted and with each top-k menu (served chance, no deadline). The
unrestricted equilibrium support averaged 1.4 (P1) / 1.5 (P2) joint actions.

| k | menu rows / cols | value unchanged (abs <= 1e-6) | mean abs dv | max abs dv | time vs unrestricted (median) |
|---|---|---|---|---|---|
| 1 | 1.0 / 1.0 | 25/32 | 0.0047 | 0.0541 | 0.01 |
| 2 | 2.0 / 2.0 | 29/32 | 0.0013 | 0.0226 | 0.08 |
| 3 | 2.9 / 2.8 | 31/32 | 0.0000 | 0.0001 | 0.12 |
| 4 | 3.8 / 3.7 | 31/32 | 0.0000 | 0.0000 | 0.16 |
| 8 | 6.9 / 6.3 | 32/32 | 0.0000 | 0.0000 | 0.36 |
| 16 | 10.6 / 9.8 | 32/32 | 0.0000 | 0.0000 | 0.57 |

**This is an upper bound.** These menus come from the solver's own
equilibrium, so they contain the support by construction once k reaches it. A
policy network's menu will sometimes miss a support action; then the value
error is the value of the missing action. Menu k=1 on the brute set
(max 0.94) shows the size of that error when the support is cut. The
1.5 s menu rows above also differ in completed depth, so their value
differences from served (mean 0.25 for menu2) are mostly depth, not the menu.

## Move 1: outcome-class chance

**What it replaces.** The compressed path (`compress.rs`) collapses every
*unsegmented* 16-roll damage draw to the rolls {0, 7, 15} at {5, 6, 5}/16. Those
three rolls keep the mean but not the KO mass: if rolls 9-15 KO, the 3-roll
collapse says 5/16 (31%) instead of 7/16 (44%). The engine leaves a draw
unsegmented when its post-hit HP is not a clean function of the one roll under
the engine's queue-time checks: a defender targeted by two actions (focus fire,
spread + single target, possible redirect), Life Orb / Friend Guard, drain or
Shell Bell attackers, multi-hit, fixed damage, Sash / Sturdy / Endure /
Substitute.

**Design.**
- *Engine probe* (`vgc-engine-core`, `class_probe_begin` / `class_probe_take`,
  thread-local, off by default). On a `Recording` RNG, at every unsegmented
  damage draw whose vetoes are only coupling or calc-internal multipliers (Life
  Orb, Expert Belt and Friend Guard are applied inside `calculate_damage`), the
  engine runs the same `compute_damage_segments` the segmented sites use, with
  the real damage context at the draw site: rolls partitioned by the defender's
  post-hit `hp_bucket` under both crit values. Bucket 0 is "fainted"; the other
  boundaries (1/4, 1/3, 33%, 1/2, full, or the finer buckets of Eruption /
  Endeavor users) are exactly the thresholds the canonical hash keys on. No
  replay, no RNG draw, no state change. Draws with multi-hit, fixed damage,
  Sash / Sturdy / Endure / Sub or a drain / Shell Bell attacker get no probe.
- *Solver* (`enumerate_outcomes_classes`). After the record pass it checks the
  coupling on the recorded draws; if the cell is clean, every probed draw is
  expanded to one representative per class, at the class's median roll (central
  HP), weighted by the class's exact roll count. Everything else (segmented
  draws, crit, accuracy, secondaries, best-first truncation at `mass_eps`,
  lazy re-recording, merge by canonical hash) is the compressed path unchanged.
- *Fallback, per cell, counted.* The whole cell uses the existing 3-roll
  compressed path when:
  - `unclassified`: an unsegmented damage draw has no probe;
  - `focus`: a probed draw's defender takes another damage draw this turn (its
    starting HP for the second hit depends on the first roll);
  - `roll_coupled_hp`: the attacker's move has recoil (its own HP depends on the
    roll), or the defender earlier used a recoil / drain move;
  - `damage_reading`: an active mon knows Counter, Mirror Coat, Metal Burst or
    Comeuppance.
  A draw first seen by a later lazy re-record pass gets 3-roll representatives
  (`class_lazy_unclassified`); the rest of the cell keeps its classes.

**Soundness caveats.** KO mass and threshold-bucket mass are exact for each
classified draw. Still lossy:
- HP inside a class is the median roll's. A later threshold inside the bucket
  (end-of-turn chip, a later hit, Reversal power) is decided by that roll. This
  is the same within-bucket error that the segmented draws already carry.
- Faint-before-acting is not a fallback: the attacker's later draw is simply
  absent in the KO class (the replay is real). But the attacker's own damage
  partition was computed at its record-pass HP; an HP-dependent attacker (pinch
  ability, Eruption) whose HP bucket differs across the first hit's classes is
  partitioned at the wrong bucket. Segmented draws have the same exposure.
- An attacker's roll-dependent HP is tracked only by falling back (recoil,
  drain, Shell Bell).
- The probe trusts that `calculate_damage_with_bp` with the draw-site context is
  the damage applied. `class_probe_matches_the_engine_hit` checks the class
  masses against the lossless frontier on a Life Orb spread fixture.

## Move 2: sampled chance with shared seeds

Each cell is `N` real `step()`s. Sample `k` uses seed `splitmix(node_seed + k)`,
with `node_seed = splitmix(record_seed ^ rotl(node_hash, 17))`, so every cell of
a node uses the same `N` seeds (common random numbers). Samples merge by
canonical hash at mass `1/N` (the bucket's representative is the first sample
in it). Sample 0 runs on a recording RNG, so a cell that draws nothing stops
after one replay. Each sample calls `budget::tick()` first: it is charged to
`replay_budget` and checked against the deadline, and a cut cell is
renormalized over the samples it ran (the solver discards aborted cells
anyway). Unbiased for the cell's one-ply chance expectation over the TT's
states; the cost is variance (a 1/24 crit shows up at k/N).

## Move 3: menu-restricted root

`RootMenu { rows, cols, row_support, col_support }`. At the root only, each
side's joint actions are filtered to the menu (in legal order). Double oracle
then only ever adds menu actions to the support; `*_support` seeds it (a TT seed
from the previous iteration still wins). Illegal menu entries are dropped; a
menu with no legal entry is ignored and counted in `menu_fallbacks`. The
restricted root is a different game from the same state reached inside the
tree, so it is stored in the TT under `hash ^ ROOT_MENU_SALT`. Non-root nodes
are unrestricted. `RootValues` cover menu actions only.

Python: `row_menu` / `col_menu` / `row_support` / `col_support` take lists of
joint actions, each a list of `legal_choices`-style tuples (the shape of a
`row_joint_policy` entry), so a policy's top-k entries can be passed back
directly.
