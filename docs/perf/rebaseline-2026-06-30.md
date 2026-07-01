# 2v2 Doubles Rebaseline + PR-L Adaptive Lossy (2026-06-30)

Measured against the post-PR-A..K3 + PR-I.2 + PR-L perf stack via
`cargo run --release -p vgc-solver --example measure_2v2`.

## Headline

PR-L makes the **long-tail "monster" cells auto-engage PR-C's 3-bucket
lossy damage collapse**, leaving the typical 12-combo cells fully
lossless. The win is structural, not aggregate:

- **OHKO d=1 now COMPLETES** (`DepthLimit` at 16.34 s) where the lossless
  reference hits `NodeLimit` at the 30 s cap. Nash value matches
  bit-for-bit (`+0.0005` on both).
- **Midgame d=2 reaches 16,485 recursive nodes** vs 2,751 lossless — 6×
  more work inside the same wall budget.
- **§6 monster cells** (Garchomp Earthquake spread × ally-target
  IronHands Drain Punch) at **3,145,728 raw combos / 61 s lossless**
  become auto-lossy and run in ~µs.

## Measured numbers

`wall_cap = 30 s per solve` for both columns.

| Scenario             | Lossless (`auto_lossy = None`) | Auto-lossy (`Some(10_000)`) |
|----------------------|---|---|
| OHKO d=1             | 31.78 s **CAP**, nodes=549, value=+0.0005   | **16.34 s DepthLimit**, nodes=4,390, value=+0.0005 |
| Midgame d=1          | 30.68 s CAP, nodes=73, value=−0.0005        | 31.60 s CAP, nodes=4,093, value=−0.0005 |
| Midgame d=2          | 30.27 s CAP, nodes=2,751, value=−0.0005     | 30.00 s CAP, nodes=16,485, value=−0.0005 |
| Midgame d=3          | 31.50 s CAP, nodes=69,016, value=−0.0005    | 45.00 s CAP-watchdog, nodes=70,827, value=−0.0005 |
| Switch-heavy d=1     | 34.99 s CAP, nodes=445, value=−0.0005       | 33.26 s CAP, nodes=4,093, value=−0.0005 |

Nash values across all five scenario-depths are identical between the
two configurations (0% delta, well under the 5% acceptance bound).

## Auto-engage counts

At Midgame:

- d=1 cap-bound run: **277** cells auto-engaged
- d=2 cap-bound run: **1,279** cells auto-engaged
- §5 decomposition (cap=30 s): **1,184** of 8,610 payoff calls
  auto-engaged (≈ 13.7 %)

Typical attack/attack cells (~12 raw combos) stay lossless; auto-engage
fires only on the 32k-3.1M combo outliers.

## §2 per-cell sanity

Per-cell wall (attack/attack joint) is unchanged — those cells are
under-threshold and stay lossless:

| Scenario | Per-cell wall |
|---|---|
| OHKO     | 259 µs |
| Midgame  | 125 µs |
| Switch   | 121 µs |

## Default threshold rationale

`SolverConfig::auto_lossy_damage_threshold = Some(10_000)` is the
default. Typical 2v2 cells are 12 raw_combos; monsters are 262k–3.1M.
10k is two orders of magnitude above typical (so a small jitter in
cell composition can't push it across), comfortably above the lazy
re-record loop's expansion overhead, and well below the 262k+ floor
where wall-clock starts running into seconds-per-cell.

The knob is `Option<u32>` — `None` reproduces pre-PR-L behavior
exactly.
