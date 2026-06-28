# AGENTS — rules for contributors (human and LLM)

Read this before opening a PR or starting a coding session.

## Core rules

1. **One mechanic per PR.** A "mechanic" = one move, one ability, one item, or one structural change (e.g. weather system, target resolution). Bundling multiple mechanics makes the differential harness unable to attribute regressions. (Same lesson mimikyu learned the hard way — see its `feedback_*` memory on bundled levers.)
2. **Differential test before merge.** Every mechanic PR must add at least one **synthetic golden** test case (in `crates/vgc-engine-golden/goldens/`) that exercises it. The replay-corpus scorer is now a secondary signal — it's dominated by EV-spread recon noise (~14% mean agreement, not improving with mechanic fixes). Goldens are deterministic, full-info, and fail loud on mechanic divergence. See `crates/vgc-engine-golden/README.md` for the workflow.

   Any PR that touches `crates/vgc-engine-core/src/{battle,damage,ability,item,pokemon}.rs` must run `cargo test --workspace --exclude vgc-engine-py` — both golden gates (`corpus_loads_and_runs` and the strict `corpus_zero_divergences`) are default-on, so the workspace CI enforces full HP-level PS parity on every mechanic PR.
3. **Cite the source.** In the commit message, cite the PS file:line and the Bulbapedia URL you used. Disagreements between sources go in `docs/known_divergences.md` with the resolution.
4. **No allocations in the hot loop.** `cargo clippy -- -D clippy::needless_collect -D clippy::box_default` plus a runtime allocation tracker test will fail the PR. Heap work belongs in `Battle::new` and `build.rs`, not `step`.
5. **No `unsafe` without a `// SAFETY:` block** explaining the invariant. Reviewer's job to verify.
6. **Match PS where PS disagrees with the cartridge.** mimikyu's corpus is PS replays; cartridge-faithful behavior is a non-goal except behind a feature flag.

## Workflow for adding a move/ability/item

1. Look it up in PS (`grep -r "id: 'movename'" data/`).
2. Look it up in pkmn/engine if it exists in gen 1/2.
3. Look it up on Bulbapedia for the human-readable explanation.
4. Note any disagreements in `docs/known_divergences.md`.
5. Implement against the PS spec.
6. Add a unit test (deterministic, in-isolation).
7. Add a replay-differential test case (pull a real corpus battle that uses this mechanic).
8. Run the full corpus differential; commit only if the pass rate didn't drop.

## Workflow for adding a generation

After Phase 4 (gen 9 doubles) ships, backfill gens follow:

1. Diff PS `data/mods/genN/{moves,abilities,items}.ts` against gen 9 — that's your work list.
2. Encode the diff as a `Generation` enum match in the relevant dispatcher, not a new crate.
3. Build a replay-differential corpus for that gen (Smogon battle archives, ladder replays).
4. Climb to ≥95% corpus agreement.

## What NOT to do

- Don't fork pkmn/engine; we already considered it (see mimikyu's session log 2026-06-15). Use it as a reference, not a base.
- Don't add a mod by writing Rust code. Mods are data overlays loaded at `Battle::new`.
- Don't add per-gen crates. Generations share a crate and dispatch internally.
- Don't optimize before Phase 4. Correctness gate first, perf gate after.
- Don't trust your memory on a mechanic's RNG outcomes — read PS source. (Bulbapedia is often slightly wrong on probabilities.)
- Don't ship a mechanic without a corresponding `chance` enumeration (the MCTS frontier). It's easier to add it while the mechanic is fresh than to retrofit later.

## ResidualIndex hygiene (PR-EOT3+)

`Battle::residual_index` caches "which active slots have residual X" as a bitset per family (status DOT, item chip, leech seed, ...). The cache is read in `resolve_end_of_turn` to skip phases that have nothing to do.

**Rule:** when you mutate a field the index tracks (`Pokemon::status`, `Pokemon::item`, volatiles, weather, terrain), you must keep the index in sync. Two ways:

1. **Use the canonical setter** (`try_set_status_from_src`, `set_item_from`, `set_weather`, etc.) — they sync automatically. Prefer this.
2. **Call the sync helper** for that family within ~6 lines of the direct write (`battle.sync_status_dot_bit(side, slot)` for status; analogous helpers for future families).

For tests or fixtures that mutate a Pokemon *not* installed in a Battle slot (clones for `calculate_damage`, `effective_speed`, TeamBuilder scratch), add a trailing `// AUDIT-OK: <one-line reason>` comment.

**Static guard:** `bash tools/audit-residual-index/audit.sh` grep-checks every tracked field. Run before opening a PR. Catches missed sync sites that the debug-build `debug_assert_eq!` would otherwise only catch at test time.

**Runtime guard:** `resolve_end_of_turn` re-derives the index from live state and `debug_assert_eq!`s against the cached value under `#[cfg(debug_assertions)]`. Don't disable the assertion — fix the missed sync site.

## For LLM agents specifically

- Default to spawning subagents (Explore / general-purpose) for cross-repo lookups — PS source, pkmn/engine source, Bulbapedia. Don't pull all of PS into the main context.
- Before claiming a mechanic is "implemented per PS," paste the exact PS handler code (or its file:line) into the PR description.
- When PS and pkmn/engine disagree, surface the disagreement to the human before picking a side.
- The mimikyu repo (`~/Dev/mimikyu`) is the consumer and the source of the replay corpus; coordinate breaking changes with whoever owns `/decide`.
