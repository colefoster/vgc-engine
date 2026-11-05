# Showdown parity rounds, 2026-11-05

Five rounds of fixes against Showdown `16db150a`, measured by replaying uniform random legal Reg M-C games on `vgc sim` and on Showdown with the same seed and input log. PRs: #119 (round 1), #120 (rounds 2-4) and the PR carrying round 5.

## Results

Each row is scored on seeds that no fix was developed against. Every batch is a different random stream, so rows aren't the same games.

| After round | Seeds | Games | Identical | State differs |
|---|---|---|---|---|
| Start (before round 1) | | 300 | 10% | not recorded |
| 1 | 9000 | 500 | 68.6% | 19.6% |
| 2 | 9500 | 1,000 | 83.2% | 11.4% |
| 3 | 9800 | 2,000 | 93.0% | 4.5% |
| 4 | 9900, 12000 | 6,000 | 95.95% | 2.3% |
| 5 | 20000, 21000 | 6,000 | **97.3%** | **1.38%** |

"Identical" means logs, each side's requests and the end record all match byte for byte. "State differs" counts games whose battle state differs at the first difference; the rest differ only in text or line order.

Random legal teams measure coverage of everything legal, including sets nobody plays, so these rates are lower than for realistic games:

| Batch | Games | Identical |
|---|---|---|
| Mimikyu models and baselines | 6,000 | 6,000 |
| Arena random vs random | 20,000 | 19,999 |

The arena batches are fixed and were used as a regression check throughout, so they aren't a holdout.

Fix-time seeds overstate quality. In round 4, the seeds being fixed reached 99% while unused seeds reached 96%. Use the holdout rows.

## Regression gates

- `ps_suite`: 447 scenarios from Showdown's own tests in the gate, 150 known gaps.
- `save_load`, new: every field that crosses a decision point must survive a save and reload. It runs 42 games in CI, and was clean on 10,000 fuzz games at round 4.
- Tests at this commit: 1,875 pass with `ps-rng`, 1,858 with the default build.

## Speed

`perf_bench`, 5 alternating runs, median: 395k steps/s at `2f40f0798`, 364k here (about 8% slower). Runs were noisy on a shared machine. `step()` allocates nothing in the default build. With `PS_RNG=1` it allocates about 2.25 times per step, as before.

## Compatibility

- Same-seed results change. Anything comparing seeded results across this work needs a new baseline, including kcheck-mt.
- Older bincode snapshots don't load.

## Known gaps

- Round 5 stopped at its fix limit, not because the errors ran out. Its stopping rule of two clean fresh batches was not met, and the remaining errors are a long tail of rare causes.
- Known and unfixed: Steel Beam recoil after a 0-damage hit, a partial trap ending silently, Howl's empty `[spread]`, stale `disabled` on fainted mons in requests, and hidden tie draws that haven't all been classified.
- Largest first-difference group on the round 5 holdout: `-damage`, 0.3-0.4% of games.
- `known-gaps.jsonl` was last classified at 153 entries: 130 unreachable in Reg M-C and 23 not yet fixed. The split comes from a legality heuristic.
- Tera formats aren't fuzzed.
- Not covered by the fuzzer: realistic team building, and real ladder replays. The 1,300-replay figure in the README predates this work and hasn't been re-measured.

## Commit history

Many commits fix one mechanic and cite the Showdown source. Some intermediate commits don't build, some were only checked with `cargo check` on the core crate, and one bundles Toxic Chain into the Disguise fix. Bisect on merge commits.
