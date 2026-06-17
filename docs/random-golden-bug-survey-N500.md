# Random golden bug survey — N=500

Generated 2026-06-17 by running `tools/golden-gen/generate-batch.sh 500`
(seeds 0..499) and feeding the output through
`cargo run -p vgc-engine-golden --example explore` (structural
comparison only: move / switch / faint / status / miss / damage —
exact HP values **not** compared).

The hand-picked goldens (5 in `goldens/`) all pass strict-HP. This
survey is the **random-play** signal: what gets exposed when
gen9customgame teams play each other with uniformly-random legal
actions for up to 30 turns.

## Headline numbers

| Metric | Value |
|---|---|
| goldens generated | 493 (7 PS-rejected seeds) |
| **clean** (zero divergences) | **35 (7.1%)** |
| diverged | 457 (92.7%) |
| errored | 1 (seed-391: PS reported errors mid-batch) |

## Divergence breakdown by kind

| Kind | Count | Share |
|---|---|---|
| **damage** | **2990** | **93.5%** |
| status | 202 | 6.3% |
| faint | 4 | 0.1% |
| miss | 1 | <0.1% |

**Damage divergence dominates by 15× over status.** Once damage is
wrong, the chain `damage → HP → faint → choice order` cascades —
explore mode flags every downstream turn as `damage-divergence` too,
inflating the count. Fixing damage at the source is the highest
leverage move.

## Top damage-divergence labels (mon-attributed)

```
Brambleghast x 30   Bayleef     x 18
Amoonguss    x 26   Exeggutor   x 18
Koraidon     x 24   ...
Lapras       x 21
Roaring Moon x 21
Eternatus    x 20
Iron Crown   x 20
Articuno     x 19
Medicham     x 19
Quagsire     x 19
```

These are mons that frequently end up on the "damage looks wrong"
side of an event. Reading them as bug-direction candidates is tricky
because explore mode marks a divergence on the damaged slot
regardless of attribution — a missing attacker-side mechanic on Lapras
shows up as a "Lapras damage" divergence, not a "Lapras's opponent
damage" one. Treat the list as a frequency map of "which mons are in
the wrong-damage events", not "which mons have buggy mechanics".

## Status divergence by sub-kind

| Status | Count | Share |
|---|---|---|
| **slp** (sleep) | **86** | **42.6%** |
| par (paralysis) | 51 | 25.2% |
| brn (burn) | 24 | 11.9% |
| psn (poison) | 17 | 8.4% |
| tox (badly poison) | 13 | 6.4% |
| frz (freeze) | 11 | 5.4% |

**Sleep is the single biggest status bug class.** Likely culprits:
Spore (Amoonguss is in the top damage list — usage strongly correlates),
Sleep Powder, Yawn, Rest. Sleep Clause (gen-9: max one sleeping mon
per side) is plausible; if engine permits two sleeping mons where PS
blocks the second, ~half the slp divergences resolve.

## Worst-offender seeds (debug entry points)

```
seed-252: 35 divergences
seed-432: 26 divergences
seed-207: 23 divergences
seed-221: 23 divergences
seed-130: 22 divergences
seed-308: 22 divergences
seed-312: 22 divergences
seed-475: 22 divergences
seed-27:  20 divergences
seed-281: 20 divergences
```

Each is a single random battle that surfaced many cascading
divergences. Use as triage entry points — picking one and running it
through strict-mode `inspect` will likely reveal a clear mechanic
breakpoint at turn 1 or 2.

## How to reproduce

```bash
tools/golden-gen/generate-batch.sh 500    # ~6 min (after PR-204)
mv crates/vgc-engine-golden/goldens/random /tmp/random-survey/
cargo run --release -p vgc-engine-golden --example explore /tmp/random-survey
```

(The `mv` step keeps the random goldens out of `goldens/` so the
strict-HP gate in `cargo test` doesn't try to enforce them.)

## Triage priorities

In rough leverage order:

1. **Sleep mechanic** (~86 status divergences + an unknown share of the
   downstream damage cascade): audit Spore / Sleep Powder / Yawn /
   Rest / Sleep Clause against PS `data/moves.ts` and
   `sim/pokemon.ts:setStatus`.
2. **Damage roll alignment** (likely 1-2 PS-side rounding / order
   mismatches that explain a large fraction of the 2990 damage events):
   audit `damage.rs` modifier order against PS
   `sim/battle-actions.ts:getDamage`.
3. **Paralysis / Burn** (~75 combined): same playbook as sleep.
4. **Misses / type immunity** (1 divergence here but undercounted —
   downstream damage cascade hides them): individual triage.

Each top-priority item is ideally a one-PR fix + a re-run of this
survey to verify the count drops. A 30%+ reduction in `damage` rows
on the re-run is a strong signal the fix landed broadly.
