# Endgame solver coverage map

Status of `endgame_solve*` / `solve_endgame_exact` / `solve_endgame_anytime` on
real late-game positions, 2026-10-25. Produced by
`crates/vgc-solver/examples/solver_harness.rs` and `scripts/solver_coverage_map.py`.

**Reproduce:** on unraid, not the Mac. Each process is memory-guarded: `ulimit -v`
3.5 GB plus a 1.5 GB RSS check inside the harness.

```
# positions: mimikyu scripts/solver_harness_dump.py (SP) + kcheck-mt endgame fixtures
solver_harness real real_positions.jsonl --budget-ms 1500 --exact-ms 2000 > real.jsonl
solver_harness brute 40 --seed N --max-depth 3 > brute.jsonl
solver_harness brute 40 --seed 22 --only 38                          # per-cell frontier vs real RNG
solver_harness mcdiff real_positions.jsonl --line L [--lossless]    # frontier debug
python3 scripts/solver_coverage_map.py --brute 'brute*.jsonl' --real 'real*.jsonl' --positions real_positions.jsonl
```

## Hard budget guarantees

The 2026-10-20 bug was that `max_depth=2, node_budget=10000` ran for more than 60 s.

**Root cause.**
- `node_budget` counts `solve()` calls, but the time is spent inside single cells. One 2v2 spread-move cell replays up to about 10^5 chance combinations.
- The deadline was only checked between cells and nodes. A single cell overran a 1 s deadline to 2.33 s.
- The exact binding had no deadline at all and held the GIL.

**The fix.**
- A per-solve budget lives in `vgc_solver::budget`: a deadline plus `replay_budget`, a deterministic cap on `step()` replays.
- It is checked before every replay in every enumerator: flat, mutual-focus tensor, factored and compressed. It is also checked in max-actions ranking, in every double-oracle iteration and in the single-ply `solve_turn_budgeted`.
- `endgame_solve_bounded` always returns a well-formed node with a `Truncation` reason: `deadline`, `replay_budget`, `node_budget` or `chance_limit`. The node is the deepest completed iteration. If nothing completed, it is a depth-0 leaf value with a uniform policy over legal joint actions, with provenance `timeout`.
- The Python bindings catch engine panics, release the GIL, and report `truncated` / `truncation_reason` / `elapsed_ms`.
- **Tests:** `crates/vgc-solver/tests/budget_guarantees.rs` and `crates/vgc-engine-py/tests/test_solver_budgets.py`.
  - They cover the real 2v2 repro fixtures, plus a full-HP 2v2 with spread moves and a 10-hit Population Bomb.
  - Paths covered: exact, exact_hp, factored, anytime, compressed anytime and single-ply.
  - They assert latency of budget + 150 ms, legal policies that sum to 1, deterministic replay-budget results, and that the GIL is released.
- On 1031 real positions, the maximum overrun was **0.5 ms** for the served config (1500 ms) and **38 ms** for exact depth 2 (2000 ms).

## Brute-force agreement (generated small endgames)

136 generated positions were compared: singles 1v1 46, singles 2v1 27, singles 2v2 42, doubles 2v2 21. Another 24 were skipped because they went over the bounded reference caps: 300k replays, 20k live outcomes, 50k combos per cell. Reference depth: d3 114, d2 12, d1 10.

**The reference** is a full-matrix LP at every node, with every chance outcome (16 rolls, KO segments and the mutual-focus tensor off) and no TT.

| config | value agrees (abs dv <= 1e-6) | agrees when ref all-terminal | max abs dv | mean abs dv | root policy exploitable > 1e-6 | panics |
|---|---|---|---|---|---|---|
| exact_hp | 136/136 (100%) | 24/24 (100%) | 0.0000 | 0.0000 | 0/136 | 0 |
| default | 123/136 (90.4%) | 24/24 (100%) | 0.0784 | 0.0013 | 2/136 (1.5%) | 0 |
| factored | 123/136 (90.4%) | 24/24 (100%) | 0.0784 | 0.0013 | 2/136 (1.5%) | 0 |
| lossy3 | 115/136 (84.6%) | 23/24 (95.8%) | 0.2396 | 0.0033 | 4/136 (2.9%) | 0 |
| served_anytime (compressed) | 73/136 (53.7%) | 20/24 (83.3%) | 0.2435 | 0.0046 | 15/136 (11.0%) | 0 |

Real-RNG sampling: 682 root cells and 136,400 seeded `step()`s. None landed outside the frontier's buckets.

**Bug found and fixed (`0871a11`): the bucket representative depended on enumeration order.**
- Frontier dedup kept the *first-replayed* combo's Battle for each `canonical_hash` bucket.
- Example: Dragonite at 35/166 HP takes Fake Out. With the real RNG it ends at 11-15 HP 96% of the time, and at 1-5 HP on a crit. All of those share one bucket.
- Index-order enumeration replays the crit first, so the lossless frontier carried HP 5 at 99% mass. The compressed (best-first) frontier carried HP 15.
- As a result, the same exact 1v1 solved to -0.085 under one and +0.766 under the other.
- Buckets now keep the modal combo (`merge_outcome`). Served max abs dv went from 0.85 to 0.24 and its mean from 0.015 to 0.005.

**Caveat:** the reference also dedups on `canonical_hash`. So it is exact in chance and actions, but not in HP *within* a bucket, and `exact_hp`'s 100% is partly circular. Real-RNG sampling is bucket-blind for the same reason. An exact-HP hash mode for the reference is the next step for true ground truth (see the plan).

**How to read the table.**
- `exact_hp` is the shipped solver with HP bucketing and the TT off. It must match the reference exactly, and does.
- `default` merges survivors by canonical HP bucket: the TT, KO segments and auto-lossy 3-bucket. Its value is exact whenever every reference leaf is terminal. At depth-limited leaves the `hp_ratio` leaf sees the bucket representative's HP, which is a small documented approximation (max abs dv about 0.01).
- `served_anytime` adds chance compression: 3-way unsegmented rolls and best-first truncation at 2% mass. That is a larger approximation (max abs dv about 0.08 at depth-limited leaves). Its root policy is occasionally exploitable inside the exact reference matrix.
- `lossy3` and compression also move KO probability: their 3 representative rolls misweight the KO rolls (crit-KO 1.3% vs the true 0.78% in the Dragonite cell).

## Real positions

The real-position numbers are from the run before `0871a11`. The fix changes values, not latency or legality.

Sources:
- 312 SP endgame turns (both sides at most 2 alive, Switch /decide requests from 2026-10-15 to 10-24, rebuilt exactly as the shadow does).
- 770 kcheck-mt endgame belief battles (realL150 + real150).

| config | budget | finished <= budget+50 ms | max overrun ms | completed depth >= 1 | exact/terminal | panics | illegal policy actions |
|---|---|---|---|---|---|---|---|
| served | 1500 ms anytime, compressed | 1031/1031 (100%) | 0.5 | 1005/1031 (97.5%) | 51/1031 (4.9%) | 0 | 0 |
| exact_d2 | 2000 ms, max_depth 2, node_budget 10k | 1031/1031 (100%) | 38.3 | 315/1031 (30.6%) | 14/1031 (1.4%) | 0 | 0 |

- **Belief battle could not be built:** 39/1082 (3.6%), all SP. See the gaps table.
- **Engine agreement:** for each served principal-variation cell, 48 real-RNG `step()`s were checked against its lossless frontier. 0/858 cells had a sample outside it, with 0 unmatched draws. 173 cells were over 20k combos and skipped.
  - An earlier run compared against the KO-segmented frontier and flagged 1.8% of cells.
  - `mcdiff` traced that to `canonical_hash` keying on `last_damage_taken` / `last_phys_damage`. A segment's representative carries one roll's exact damage, so a real roll elsewhere in the segment hashes differently.
  - Mass and bucketed HP are right. Only Counter, Metal Burst and Comeuppance-exact damage is approximated.
- **Decision point mismatch:** 12/1031 (all SP). At the root, the viewer has no real move, because the engine is at a replacement step and not the move turn.
- **Principal variation vs what happened (SP, per slot):**
  - The solver's top opponent move matched the move the opponent actually used in 77/208 (37%).
  - The solver's top own move matched the move the bot played (the ACT policy) in 92/212 (43%).

## Gaps ranked by frequency in real positions

`positions` is out of 1082 (SP: 312). `failed` means the solver gave no usable answer.

| # | situation | status | positions | SP | failed | example id |
|---|---|---|---|---|---|---|
| 1 | Hidden info: unrevealed opponent sets filled from the usage prior (modal belief, not a distribution) | approximate | 1031 | 273 | 0 | `data/switch_runs/2026-10-15/s-222126/game_002/decide/t06_v18.req.json` |
| 2 | Chance compression dropped more than 5% of some cell's mass (served config) | approximate | 982 | 263 | 0 | same |
| 3 | Depth-limited: `hp_ratio` leaf at the horizon; only 4.9% solved to terminal in 1.5 s | approximate | 954 | 256 | 0 | same |
| 4 | SP belief battle carries phantom or unbrought opponent mons: OCR-misread switch-ins become extra "seen" mons, with no `\|teamsize\|` cap. 58 are solved on a wrong board and 21 fail with more than 6 mons | bug (mimikyu `switch_live`) | 79 | 79 | 79 | `data/switch_runs/2026-10-15/s-222126/game_026/decide/t08_v24.req.json` |
| 5 | Too slow: no depth-1 iteration within 1500 ms (2v2, about 35x36 joint actions, spread moves) | unsupported | 26 | 8 | 26 | `data/switch_runs/2026-10-15/s-222126/game_028/decide/t08_v24.req.json` |
| 6 | Decision point mismatch: the root is a replacement step, so the viewer has no move | bug | 12 | 12 | 12 | `data/switch_runs/2026-10-15/s-222126/game_016/decide/t17_v48.req.json` |
| 7 | Belief build refuses volatile Encore (6) and Disable (4) | unsupported | 10 | 10 | 10 | `data/switch_runs/2026-10-22/s-212455/game_041/decide/t09_v25.req.json` |
| 8 | Ability not implemented: Pickpocket | unsupported | 8 | 0 | 0 | `kmt_realL150:gen9championsvgc2026regmc-2701196164@t8\|1000,2000,3000,4000:t8` |
| 9 | Ability not implemented: Compound Eyes (accuracy x1.3 missing, so chance is wrong) | unsupported | 6 | 0 | 0 | `kmt_realL150:gen9championsvgc2026regmc-2701316587@t9\|1000,2000,3000,4000:t12` |
| 10 | Decide request after the game ended: a side has no mon left | unsupported | 6 | 6 | 6 | `data/switch_runs/2026-10-20/s-082859/game_004/decide/t10_v222.req.json` |
| 11 | Frisk (3) and Illuminate (2) are not implemented, but are inert for the solver. Scope Lens (2) and Leek (1) are not referenced as item arms; verify their crit stage | unsupported? | 8 | 6 | 0 | `data/switch_runs/2026-10-23/s-144521/game_019/decide/t06_v18.req.json` |
| 12 | Within-bucket HP: one representative per `canonical_hash` bucket; KO thresholds inside a bucket are mis-solved. Also, the KO-segment representative's exact `last_damage_taken` (Counter / Metal Burst) | approximate | all | | | brute seed 22 #38, `tests/bucket_representative.rs` |
| 13 | Illusion ambiguous (1); SP active mon not in the team (1) | unsupported | 2 | 2 | 2 | `data/switch_runs/2026-10-23/s-192127/game_013/decide/t15_v392.req.json` |

### Situations that are covered

The served config finished at least 1 ply in these situations:
- Weather: rain 142/144, sun 123/126, sand 83/85, snow 28/29.
- Terrain: grassy 191/197, psychic 112/114, electric 17/17.
- Field and side conditions: Trick Room 95/96, Tailwind 108/109, screens 55/58.
- Status: burn, paralysis, sleep, poison and toxic.
- Mechanics: Mega available 179/192, Fake Out 457/467, Protect 975/1000, pivot moves 394/404, multihit 50/54, forced switch 17/17.
- Board sizes: 1v1, 2v1 and 1v2 all 100%. 2v2 327/351; every miss is a 2v2.

## Plan for the gaps

1. **Hidden info (approximate).** Solve over a few sampled belief battles drawn from the prior, not just the modal one. Aggregate the root matrices (determinized), as the depth-1 search driver already does in mimikyu. This is the largest fidelity gap and needs no engine change.
2. **Compression and horizon.** The time goes to chance enumeration.
   - Make the 1.5 s budget go further: cap joint actions with `max_actions` ranking at the root of 2v2, and reuse the TT across consecutive turns of one game.
   - Measure whether `mass_eps` 0.02 should vary with the board size.
3. **SP board (mimikyu, not the engine).**
   - `switch_live` should emit `|teamsize|p2|4`.
   - It should snap OCR species to the 6-mon preview, and drop switch-ins that don't match.
   - The shadow already gates on its own alive counts. It should also refuse when the belief battle's alive counts differ (cheap guard).
4. **Too slow 2v2 (26).** Same levers as item 2. The served result is still well-formed: a depth-0 placeholder that the shadow ignores.
5. **Decision point mismatch.** When the engine root needs replacements and the request is a move request, `position_from_replay` should finish the pending replacement from the log (or the request) before returning.
6. **Encore / Disable.** Model the volatiles in `position_from_replay`: the turns left are known from the log. Encore needs `move_locks`.
7. **Within-bucket HP (approximate).**
   - A bucket's single representative still decides KO races whose threshold falls inside the bucket. This is the residual 0.24 served and 0.08 default error.
   - Options: split segments at the next-ply KO thresholds of the opponent's moves (what `segments` does for the current hit), or add an exact-HP `canonical_hash` mode for `exact_hp` and the harness reference.
   - Replace lossy3's {0,7,15} rolls with KO-aware representatives.
8. **Abilities.** Compound Eyes (accuracy) first. It is the only one listed that changes chance. Pickpocket next. Frisk and Illuminate are inert in battle.
