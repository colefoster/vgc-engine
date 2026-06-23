# Design: PS Comparison Harness — a keyed-outcome correctness oracle

Status: **proposed** (2026-06-23). Not yet implemented. Supersedes synth-score as the primary breadth correctness signal once built.

## TL;DR / recommendation

A new differential harness (new crate `crates/vgc-engine-conformance`) that, per battle:

1. Runs the battle in **Pokémon Showdown first**, with random-but-recorded choices, capturing: the **resolved choice sequence** (incl. post-faint replacements + forced switches) and a **keyed log of every randomized outcome** (hit/miss, crit, damage bucket, secondary proc, multi-hit count, target/redirect pick, speed-tie winner, status durations, …).
2. Replays the **same choice sequence** into vgc-engine with a new **`Rng::OracleKeyed`** variant that resolves each engine draw by a **semantic key** (turn + actor + move + target + decision-type + occurrence index) rather than by queue position.
3. Diffs **full per-turn state** between the two engines.

**The single most important decision: abandon the positional `oracle_partial` queue for this harness and adopt site-keyed outcome injection.** Today's synth-score is capped at ~55% not by mechanics but by the flat queue desyncing the moment the two engines draw a different *number* of randoms. Keyed lookup makes injection order- and count-independent — exactly the "neutralize RNG without bit-exact parity" the project policy demands (we do NOT chase bit-exact LCG draw-parity).

Relationship: **supersedes synth-score** as the breadth signal; **complements `--psgen5`** (kept as the strict bit-exact microscope) and the **calc-oracle** (kept as the PS-independent damage spec).

## The central problem: RNG synchronization

**Why the flat queue fails (root cause of the 55% cap):** `Rng::oracle_partial` holds a `Vec<RngEvent>` consumed by position. PS and vgc-engine don't draw the same number of randoms in the same order (e.g. PS draws accuracy per spread target, or a `randomChance` for an ability the engine doesn't model). Once PS's queue is one ahead, every later pop reads the wrong outcome and the diff is poisoned — `explore.rs`'s `rng-balance` check literally measures this failure. So today's number measures "how often the two LCG streams stayed phase-locked," not mechanic correctness.

**Options:** (A) bit-exact LCG parity (`--psgen5`) — rejected by policy + brittle; (B) flat outcome queue (today) — order-dependent → the cap; (C) distribution comparison — useful supplement, can't localize a bug; **(D) keyed outcome injection — recommended.**

**Keyed injection:** capture each PS outcome tagged with battle context (`turn, actorRef, move, targetRef, decision, occ, outcome`); add `Rng::OracleKeyed { table, fallback }` to the engine; set a small `RngContext` before each move-resolution sub-phase; each draw method looks up `table[(turn,actor,move,target,decision,occ)]`, incrementing `occ`, falling back to deterministic Splitmix (and recording an `unmatched_draws` stat) on miss. Result: an engine-only extra draw misses the table and takes a default *without shifting any other lookup*; a PS-only outcome just leaves an unused entry. No cascade. Start with strict keys, add a coarsening flag if move/target attribution proves noisy.

**Main engine cost:** threading `RngContext` to draw sites (`battle.rs`/`damage.rs`/`ability.rs`). Mitigate by setting context at the few choke points in `step`/`resolve_move`; damage roll + crit are already centralized, accuracy + secondaries are the other two. Can be `--features conform` gated to stay zero-cost in the hot loop.

## Architecture / data flow

```
seed N → team-gen.js ×2 → verify_team → legal Showdown teams
  → PS DRIVER (Node, BattleStream + patched RNG): emits choices[] + keyed outcomes[] + per-turn state[]
  → ENGINE RUNNER (Rust): replay choices[] under Rng::OracleKeyed(outcomes) → per-turn snapshot
  → STATE DIFFER (Rust): normalize + compare + categorize
  → per-battle DivergenceReport → aggregate → JSON + human summary
```

Key change vs. the golden harness: **PS plays once and we record its resolved choices + replacements**, then feed those exact choices into the engine. This removes the golden harness's hard "cut off at first faint / forced switch" limitation — we no longer reproduce PS's RNG-picked replacement, we *replay* the actual mon. Roughly doubles comparable turns.

## PS driver

- PS prebuilt at `/tmp/pokemon-showdown-research/dist`; drive via `dist/sim` (`BattleStream`, `Teams`) — Node only, no rebuild. Pin `PS_DIST` via env for CI.
- Seed: `>start {"formatid":"gen9doublescustomgame","seed":[a,b,c,d]}`. Support singles + doubles.
- Reuse the existing random-player picker in `tools/ps-golden-driver/driver.js` (doubles targeting + forced-switch handling), but additionally **record each emitted choice** for verbatim engine replay.
- **State capture: use `Battle.serialize` (`sim/state.ts`)** for the authoritative full state dump, not protocol-log scraping. Use the omniscient protocol log only for per-turn event context.
- **Outcome capture:** enrich `driver.js`'s `patchRng` (wraps `Battle.prototype.random`/`randomChance`) with the context envelope read from `this.turn`/`this.activeMove`/`this.activePokemon`/`this.activeTarget`.

### Randomized decision points (capture + inject)

`accuracy`, `crit`, `damage_roll` (PS `random(16)` → engine `roll = 15 - random(16)` mirror), `secondary`, `multihit`, `target_redirect`, `speed_tie`, `status_duration` (sleep 1-3 etc.), `confusion_self_hit`/`thaw`/`para`, `ability_item_proc` (Quick Claw, Flame Body, Static…), `quick_claw` (gen9 `randomChance(1,5)`), `gender` (construction), `tiebreak`. `occ` disambiguates repeats (multi-hit accuracy, two crit rolls, N secondaries per target).

## State comparison (per turn, every slot + field)

- **Per Pokémon:** hp/max, fainted, status + counter (sleep turns, toxic stage), boost stages, current types (Tera/Soak/forme), ability (post-Skill-Swap/Trace), item (post-Knock-Off/Trick/consume), PP per slot, volatiles (Substitute+HP, Leech Seed, confusion, Taunt/Encore/Disable turns, Protect/stall counter, charging), Tera-used, commanding.
- **Per side:** screens/Tailwind/hazards/Safeguard/Mist counters + layers, Wish/Future Sight pending, active slot occupancy (which party index in a/b).
- **Field:** weather/terrain + counters, Trick Room/Gravity/Magic Room/Wonder Room turns (all already public `Battle` fields).
- **Normalization:** a `NOT_MODELLED` allow-list buckets unsupported-mechanic divergences as "expected gap" (drives the work order) rather than failing the gate. HP/PP/counters compared **exactly** (full-info, oracle-fed).

## Metric & divergence handling

- **Headline metric (replaces slot-%):** **% of comparison battles that match PS exactly through natural end** (excluding NOT_MODELLED), plus a **per-mechanic divergence table** by distinct-battle count. Reflects real mechanic correctness because RNG is neutralized.
- **First-divergence isolation:** report earliest diverging turn per battle; stop attributing downstream cascades (93.5% of damage divergences in the N=500 survey were cascade noise).
- **Auto-minimization:** binary-search turn count + shrink to the two mons involved → minimal `seed+turn+teams` repro promoted to a committed scenario fixture.
- **De-dup:** hash on `(mechanic, field_class, first-diverging slug)`.
- **Health metric:** `unmatched_draws` count (high → keying/draw-site needs attention).

## Phased plan

- **Phase 0 — Tracer bullet (~3-4d):** keyed injection end-to-end on ONE simple singles battle (no switches/faints, 1 damaging move + 1 secondary). Add `Rng::OracleKeyed` + minimal `RngContext` (crit/damage/accuracy/secondary). Gate: exact match + `unmatched_draws == 0`. **De-risks the biggest unknown — do keys reconstruct deterministically on both sides.**
- **Phase 1 — Choice replay incl. switches/faints (~3-4d):** capture + replay PS resolved choices verbatim → battles run to natural completion. Gate: 30-turn doubles battle matches or localizes a real divergence.
- **Phase 2 — Full state diff (~4-5d):** expand snapshot to all §state fields via `Battle.serialize`; NOT_MODELLED allow-list. Gate: field-complete diff on scenario suite, no false positives.
- **Phase 3 — Breadth + categorization + metric (~3-4d):** batch N seeds (amortize ~60s dex load), aggregate/categorize/de-dup, headline metric + per-mechanic table, `cargo run -p vgc-engine-conformance` CLI. Gate: N=1000 ranked punch list, stable across reruns.
- **Phase 4 — Minimization + CI + scenario suites (~3-4d):** auto-minimize to fixtures; targeted suites (sleep first — #1 status bug class); CI gate at agreed threshold.

## Open questions (need a decision)

1. **Key strictness** — full `(turn,actor,move,target,decision,occ)` vs coarser. Rec: full + coarsening flag.
2. **State source** — `Battle.serialize` (rec) vs protocol log.
3. **Crate placement** — new `vgc-engine-conformance` (rec, keeps the strict golden gate untouched) vs grow `vgc-engine-golden`.
4. **`RngContext` invasiveness** — threading to draw sites touches battle/damage/ability; feature-gate it?
5. **Doubles target/redirect & speed-tie keying** — least cleanly attributable; may need a global-subqueue fallback. Validate in Phase 1.
6. **NOT_MODELLED governance** — who curates; does the gate fail when an allowlisted mechanic regresses after implementation?
7. **CI cost** — small fixed-seed set per-PR vs large nightly soak.
8. **mimikyu replay corpus** — add a mode that replays *real* corpus battles through the same keyed injection (later phase)?

## Critical files
- `crates/vgc-engine-core/src/rng.rs` — add `Rng::OracleKeyed` + `RngContext`
- `crates/vgc-engine-core/src/battle.rs` — thread context to draw sites; full-state snapshot accessor
- `tools/ps-golden-driver/driver.js` — enrich `patchRng`; record resolved choices; emit `Battle.serialize` state
- `crates/vgc-engine-golden/src/lib.rs` — reuse team/action plumbing; new differ supersedes `run_golden`/`score_synth_corpus`
- `tools/golden-gen/team-gen.js` + `crates/vgc-engine-core/src/format_rules.rs` — legal random matchup generation + verification
