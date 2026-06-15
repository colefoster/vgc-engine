# PLAN

Incremental phased roadmap. **Each phase ships a usable artifact and ends with a measurable gate.** Do not start phase N+1 until phase N's gate is green and documented in `data/runs.md` (or this file's "Phase log" section).

## Ordering principle

**Modern-gen first, then backfill.** mimikyu (the consumer) needs gen 9 doubles VGC. Building gens 1→9 in order would burn months on detour work with no payoff. So:

- **Phase 1–4** ship gen 9 doubles end-to-end (minimum viable VGC sim).
- **Phase 5+** backfill earlier gens, share code where mechanics didn't change, only diverge where they did.

Within gen 9 itself we **slice by mechanic coverage, not by feature completeness**: implement the moves/abilities/items that appear most in the mimikyu replay corpus first, climb the long tail later.

## Current phase

**Phase 2** — top-50 mechanic coverage. See `data/corpus_frequency.md` for the
work order.

---

## Phase 0 — Scaffold (DONE 2026-06-15)

- Repo created, docs written, license chosen.
- No Rust code yet.
- **Gate:** docs reviewed, mimikyu team agrees on scope.

## Phase 1 — Skeleton + corpus analysis (~1 week)

- Cargo workspace: `vgc-engine-core` (sim), `vgc-engine-data` (build-time data gen), `vgc-engine-py` (pyo3), `vgc-engine-cli` (debug harness).
- `build.rs` pulls `@pkmn/dex` JSON dumps for gen 9 + Champions mod overlay, generates Rust source for: species table, move table, item table, ability table, type chart.
- Corpus analysis script: scan mimikyu's replay corpus, output frequency tables for moves / abilities / items / formats. **This drives the Phase 2 work order.**
- `Battle::new()` + `step()` stubs that compile but always return `Continue`.
- **Gate:** `cargo test` passes (with empty tests); `python -c "import vgc-engine; vgc-engine.Battle()"` works from mimikyu's venv; corpus frequency report committed to `data/corpus_frequency.md`.

## Phase 2 — Top-50 mechanic coverage, gen 9 doubles (~3–4 weeks)

- Implement the 50 most-frequent moves, 30 most-frequent abilities, 20 most-frequent items from Phase 1's corpus analysis.
- Doubles targeting (`-1`/`-2`/`+1`/`+2` per PS spec), spread move damage rolloff, redirection (Follow Me / Rage Powder), Protect family.
- Weather (Sun/Rain/Sand/Snow), terrain (4 types), screens, Tailwind, Trick Room.
- Status conditions, stat stages, type chart, crit, accuracy/evasion.
- No Tera yet, no Z-moves (gen 7 only), no Dynamax (gen 8 only) — gen 9 doesn't have those except Tera.
- **Gate:** 1000-replay differential vs. PS — ≥80% turn-level agreement on the mimikyu corpus, ≥95% on battles that only use the implemented mechanic set.

## Phase 3 — Long-tail coverage to 95% (~4–6 weeks)

- Climb the corpus frequency curve. Target: implement everything that appears in ≥0.1% of corpus turns.
- Tera (gen 9 signature mechanic).
- Champions VGC mod overlay (sig moves: snap-trap, light-of-ruin, fake-out on Pansage line, etc. per `mimikyu/docs/champions_mod_data.md`).
- All edge-case interactions surfaced by the differential harness.
- **Gate:** ≥95% turn-level agreement on full mimikyu replay corpus (≥10k battles).

## Phase 4 — Performance + MCTS integration (~2–3 weeks)

- Profile, eliminate remaining allocations, vectorize hot paths.
- `Battle: Clone + Copy` if struct size permits (target ≤2KB).
- `chance` feature flag emits enumerated outcome frontier for MCTS.
- pyo3 bindings finalized: `Battle`, `Choice`, `step`, `legal_choices`, `clone`, `serialize`, `deserialize`.
- **Gate:** mimikyu integration PR — `vgc-engine` replaces PS in at least one rollout path, ≥10× wall-clock speedup measured.

## Phase 5+ — Backfill earlier gens (open-ended)

Order TBD; likely **gen 8 → 7 → 6 → 5 → 4 → 3 → 2 → 1** (newest backward, since the delta shrinks each step). Driven by external demand, not the mimikyu roadmap.

Each backfill gen follows the same phase structure (corpus → top-50 → long-tail → perf), against that gen's competitive replay corpus.

---

## Phase log

| date       | phase | result | notes |
|------------|-------|--------|-------|
| 2026-06-15 | 0     | DONE   | repo scaffolded, no code |
| 2026-06-15 | 1     | DONE   | cargo workspace (core/data/py/cli), build.rs codegens 1416 species / 935 moves / 581 items / 311 abilities / 18-type chart from @pkmn/dex; pyo3 wheel installs into mimikyu's .venv-analytics and `import vgc_engine; vgc_engine.Battle()` works; corpus_frequency.md generated from 1,047,787 replays (top-95% cutoffs: 166 moves / 27 abilities / 10 items / 152 species — Phase 2 work order) |
