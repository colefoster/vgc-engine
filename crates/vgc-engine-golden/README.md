# vgc-engine-golden

Synthetic golden-master differential harness — the **primary correctness
signal** for engine mechanics. Pairs with `tools/ps-golden-driver/`.

Unlike the replay-corpus scorer in `vgc-engine-replay`, this harness has
**no hidden state**. Every test specifies the full team (EVs / IVs /
nature / ability / item / Tera type) and every per-turn action.
Engine RNG is pinned to PS's recorded draw stream via
`Rng::oracle_partial`. Any divergence is a mechanic bug.

## How a golden test runs

1. `goldens/<name>.input.json` declares the scripted battle (teams in
   Showdown export text + PRNG seed + per-turn actions).
2. `goldens/<name>.ps.json` is the PS-recorded ground truth produced
   by `tools/ps-golden-driver/`: a protocol event log (with raw HP)
   plus the captured PS RNG draws.
3. `cargo test -p vgc-engine-golden` walks every `(input, ps)` pair in
   `goldens/`, runs the engine through every turn, and compares
   end-of-turn HP / status / faint for every active slot the PS log
   touched.

## Test gates

* `corpus_loads_and_runs` — runs by default. Fails on IO / parse /
  driver errors. Always-green: this is the workspace-level gate.
* `corpus_zero_divergences` — `#[ignore]`d by default. Fails on any
  HP / status / faint mismatch. Run with
  `cargo test -p vgc-engine-golden -- --ignored`. Flip to default-on
  once the existing divergence list is triaged into mechanic PRs.

## Adding a new golden

1. Write `goldens/<name>.input.json`:

   ```json
   {
     "name":   "<name>",
     "format": "gen9customgame",
     "seed":   [1, 2, 3, 4],
     "p1": { "team": "<showdown export text>" },
     "p2": { "team": "<showdown export text>" },
     "turns": [
       { "p1": "move 1", "p2": "move 1" }
     ]
   }
   ```

   * `format`: `gen9customgame` (singles, no clauses, any EV spread) or
     `gen9doublescustomgame` (doubles, same).
   * `seed`: PS's PRNG seed, four `u16`s. `[1,2,3,4]` is the default.
   * Each turn action is a PS command string. Singles: `"move 1"`,
     `"switch 3"`, `"pass"`. Doubles: comma-separate per slot,
     `"move 1 1, move 2"` — `move N T` where `T` is PS relative
     targeting (positive = foe slot, negative = ally slot). Spread
     moves (Earthquake, Astral Barrage, etc.) must NOT take a target.

2. Generate the PS ground truth:

   ```bash
   node tools/ps-golden-driver/driver.js \
     crates/vgc-engine-golden/goldens/<name>.input.json \
     > crates/vgc-engine-golden/goldens/<name>.ps.json
   ```

   Inspect the output — `ok: true` and `errors: []` mean PS accepted
   every action. If `ok: false`, the `errors` array names the bad
   command (most common: spread move with target, switch to a slot
   that doesn't exist after team-preview reorder).

3. Run the harness:

   ```bash
   cargo run -p vgc-engine-golden --example inspect
   ```

   This prints per-golden `matched` / `diverged` counts. If `diverged > 0`,
   the engine and PS disagree on at least one slot's end-of-turn state.

## What counts as a divergence

For every active slot the PS log emits an event on during a given turn:
* **HP / max mismatch** → divergence (`kind: hp_or_status`).
* **`fainted` mismatch** → divergence.
* **Status mismatch** (`brn` / `par` / `slp` / etc.) → divergence.
* **Engine has no mon in that slot** when PS does → divergence
  (`kind: missing_slot`).

Boosts and side conditions (Tailwind, Stealth Rock) are not yet
compared — they're an obvious next-PR extension once the HP-level
diff stabilizes.

## Regenerating after a team / action edit

Edit the `input.json`, then re-run the driver to regenerate the
matching `ps.json`. There's no auto-regen — the `ps.json` is
intentionally checked in so the gate stays deterministic across
environments. Drift between the two files always means "rerun the
driver".

## Why this replaces the replay-corpus signal

The replay-corpus scorer in `vgc-engine-replay` runs against Smogon
replay logs. Replays don't expose hidden state (EVs / IVs / nature /
ability for any mon you haven't seen), so we reconstruct it heuristically
(see `crates/vgc-engine-replay/src/recon*.rs`). At present the corpus
sits around 14% mean agreement and isn't improving with mechanic fixes —
because the dominant error term is the recon, not the engine. Goldens
sidestep that entirely.
