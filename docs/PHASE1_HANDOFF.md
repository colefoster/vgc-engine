# Phase 1 handoff prompt

Self-contained brief for the first agent picking up the project. Paste into a fresh session at `~/Dev/vgc-engine`, or use as the body of the Phase 1 tracking issue.

---

You are picking up `vgc-engine`, a brand-new Rust project at `~/Dev/vgc-engine` (GitHub: `colefoster/vgc-engine`, private). It is **completely empty of code** — only docs exist. Your job is **Phase 1** of `docs/PLAN.md`.

## Read first (in this order, ~10 min)

1. `~/Dev/vgc-engine/README.md` — the pitch.
2. `~/Dev/vgc-engine/docs/PLAN.md` — phased roadmap. **You own Phase 1.** Do not start Phase 2.
3. `~/Dev/vgc-engine/docs/DESIGN.md` — language + arch decisions already made (Rust, pyo3, data-oriented, shared-across-gens). Do not re-litigate these.
4. `~/Dev/vgc-engine/docs/AGENTS.md` — workflow rules. **One mechanic per PR, no allocations in step(), cite sources.** These bind you.
5. `~/Dev/vgc-engine/docs/REFERENCES.md` — Pokémon Showdown is the compatibility oracle, pkmn/engine is the design template, Bulbapedia is the human-readable explainer.

If anything in those docs contradicts this prompt, **the docs win** — surface the conflict before proceeding.

## Phase 1 gate (the thing you must deliver)

Quoting `docs/PLAN.md`:

> `cargo test` passes (with empty tests); `python -c "import vgc_engine; vgc_engine.Battle()"` works from mimikyu's venv; corpus frequency report committed to `data/corpus_frequency.md`.

So three concrete artifacts:

### 1. Cargo workspace skeleton

Create the four crates from `docs/DESIGN.md`:

- `vgc-engine-core` — sim. Public API: `Battle::new(config) -> Battle`, `Battle::step(p1: Choice, p2: Choice) -> StepResult`, `Battle::legal_choices(side) -> &[Choice]`. **Stub implementations only** — `step()` returns `Continue` unconditionally; `legal_choices()` returns empty slice. The point is the API shape, not behavior.
- `vgc-engine-data` — build-time data generation. `build.rs` reads `@pkmn/dex` JSON dumps (mimikyu's `~/Dev/localdex/data/` has them) and generates Rust source for: species table, move table, item table, ability table, type chart. Output goes to `OUT_DIR`; the core crate `include!`s it. **Gen 9 only for Phase 1.** Champions mod overlay is Phase 3 — do not include it yet.
- `vgc-engine-py` — `pyo3` bindings. Expose `Battle`, `Choice`, `StepResult` to Python. Build to a wheel installable in mimikyu's venv (path TBD — check `~/Dev/mimikyu/pyproject.toml` for the project venv setup; ask if unclear).
- `vgc-engine-cli` — minimal debug harness. `vgc-engine-cli step --p1 <choice> --p2 <choice>` for poking at the sim from a shell. Stub-fine.

Pin Rust edition to 2024 (or latest stable at time of writing). `cargo new --lib` for each, wired through a workspace `Cargo.toml` at the repo root.

### 2. Make the Python import work

After `maturin develop --release` (or `pip install -e crates/vgc-engine-py/`) in mimikyu's venv:

```bash
python -c "import vgc_engine; b = vgc_engine.Battle(); print(b)"
```

…should print a `Battle` repr without error. That's the gate. No behavior required.

### 3. Corpus frequency report

The mimikyu replay corpus lives somewhere under `~/Dev/mimikyu/data/` — find it (look at `~/Dev/mimikyu/docs/ARCHITECTURE.md` and the shard generation code; ask if you can't locate it within ~15 minutes of searching).

Write a one-off Python script (in `scripts/`, not a crate) that:

- Walks the replay corpus.
- Counts frequency of every distinct **move slug**, **ability slug**, **item slug**, and **species** that actually appears in a turn that was *played* (not just in a team).
- Emits a markdown report to `data/corpus_frequency.md` with four tables, sorted descending, one row per entity: `slug | count | cumulative_pct`.
- Prints the top-N that covers 95% of usage for each category (Phase 2's work-list).

The report's purpose: it is the **work-order document for Phase 2**. Phase 2 implements the top-50 moves, top-30 abilities, top-20 items — that ordering comes from this report. Get it right; it shapes the next month of work.

## What NOT to do in Phase 1

- **No mechanic implementations.** Not one move. Not Tackle, not Protect, nothing. Phase 1 is structure + measurement; Phase 2 is mechanics.
- **No mod overlays.** Champions is Phase 3.
- **No backfill gens.** Gens 1–8 are Phase 5+.
- **No performance tuning.** Phase 4 owns perf. If you find yourself reading `cargo bench` output, you're off-task.
- **No premature abstractions.** Don't design a `Generation` trait yet; we know we need one, but designing it before we have one gen working is divination.
- **Don't fork pkmn/engine.** We considered it and rejected it (docs cite the reasons). Read it as a reference. Do not vendor or submodule it.

## Workflow constraints (from `docs/AGENTS.md` — read it)

- **One concept per PR.** Suggested PR ordering for Phase 1: (1) workspace skeleton + stub API, (2) `build.rs` data-gen pipeline, (3) `pyo3` bindings, (4) corpus frequency script + report.
- **Cite sources in commit messages** when the work touches mechanics or data layout — e.g. "matches PS `data/moves.ts:L1234` (Tackle handler)" or "@pkmn/dex `data/species.json` rev X". You won't touch mechanics in Phase 1, but `build.rs` will cite `@pkmn/dex` and the type chart should cite either PS or Bulbapedia.
- **No `unsafe` without a `// SAFETY:` block.**
- **No `unwrap()` outside tests** — use `?` or `expect("specific message")`.

## When you're done

End-of-phase deliverable: a single GitHub PR or merge commit to `main` that closes Phase 1, plus a one-line update to the "Phase log" table at the bottom of `docs/PLAN.md`:

```
| 2026-06-DD | 1     | DONE   | <one-line summary, link to PR> |
```

Then **stop** and report back. Phase 2 is a separate engagement; Cole will decide whether to continue with you or hand off.

## Ambiguity protocol

If you hit a blocking question (mimikyu venv path, where the replay corpus lives, edition choice, etc.) and have spent more than 10 minutes searching for the answer yourself, **ask**. Don't guess on structural decisions; this is foundational code that will get copied/extended for months. Cole prefers a 30-second clarifying question over a 30-minute wrong path.

If you hit a non-blocking question (naming, file layout within a crate, etc.), make the call yourself and document it in the PR description so it can be revisited.

## Out-of-scope but useful context

- mimikyu (`~/Dev/mimikyu`) is the ML pipeline that will eventually consume this sim. Its `CLAUDE.md` has project context if you need it. Don't modify mimikyu in Phase 1 except possibly the venv to install the wheel.
- The eventual consumer of the pyo3 bindings is mimikyu's `/decide` server (FastAPI) and its MCTS rollout code (TBD). API shape should be cheap to call from a hot Python loop: prefer returning POD structs over Python objects when possible.
- `~/Dev/localdex` is a TS dump of `@pkmn/dex` we already trust. Use its JSON as the input to `build.rs`.

Good luck. Make it boring and correct; we'll make it interesting and fast later.
