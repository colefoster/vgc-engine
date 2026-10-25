# vgc-engine

A from-scratch, data-oriented Pokémon battle simulator in Rust, with Python bindings. It targets Gen 9 doubles, including the Champions VGC rules, and is built for code that clones a battle and plays out turns thousands of times.

**Product page:** https://vgc-engine.colefoster.ca

## Who it's for

- **Search and planning:** MCTS rollouts, endgame solvers. A `Battle` is a fixed-size struct with no shared state, so `clone()` gives an independent branch.
- **ML pipelines:** self-play and evaluation data from Python via the `vgc_engine` module.
- **Mechanics tooling:** damage, speed and EV questions from the `vgc` CLI, and Champions team legality checks.

It is not a bot. It simulates the turns your code chooses.

## Quickstart

There is no PyPI package or GitHub release yet, so build from source. You need Rust (stable), Python 3.9+ and `curl`. The data build reads the `@pkmn/dex` JSON from `$VGC_DEX_DIR`; this fetches the pinned copy CI uses.

```sh
git clone https://github.com/colefoster/vgc-engine.git
cd vgc-engine

# @pkmn/dex JSON for build.rs (localdex 0.1.1, as in CI)
mkdir -p .dex
curl -fsSL https://registry.npmjs.org/localdex/-/localdex-0.1.1.tgz | tar xz -C .dex
export VGC_DEX_DIR="$PWD/.dex/package/data"

# Build and install the Python module (vgc_engine 0.5.0)
python3 -m venv .venv
.venv/bin/pip install maturin
.venv/bin/maturin build --release -m crates/vgc-engine-py/Cargo.toml \
    -i .venv/bin/python -o dist
.venv/bin/pip install dist/vgc_engine-0.5.0-*.whl
.venv/bin/python -c "import vgc_engine; print('ok')"

# Optional: the vgc CLI
cargo build --release -p vgc-engine-cli --bin vgc
./target/release/vgc calc chomp rillaboom eq
```

Verified from a fresh clone at 0.5.0 on macOS arm64 with Python 3.9 and 3.12 (2026-10-24). On Linux, CI runs `cargo test`; the wheel steps there are untested.

## Status

**Version 0.5.0, pre-1.0.** Gen 9 doubles only; earlier generations have not been started. The API is still wire-level (choices are plain tuples) and may change before 1.0.

- **Accuracy:** on a sealed sample of 1,300 real Reg M-C ladder battles, 97.9% replay identically to Pokémon Showdown on Showdown's PRNG, and 98.9% with random outcomes forced to match. Per-turn agreement is 99.76% and 99.88%. The report rates this fit for local self-play, but not a replacement for Showdown as the final evaluator. See [`docs/accuracy/2026-10-11-accuracy-report.md`](docs/accuracy/2026-10-11-accuracy-report.md).
- **What hasn't passed yet:** the remaining 1–2% of battles diverge in rare interactions (added types, batched double switches, contact-ability procs, some terrain and Life Orb cases). The original phase gates in [`docs/PLAN.md`](docs/PLAN.md) are still formally open; see the [finish-line checklist](docs/finish-line-2026-10-06.md). No speedup is claimed until a representative workload is agreed and measured.
- **0.5.0** adds opt-in ally targets in `legal_choices` (off by default, because it changes the action space) and fixes niche doubles mechanics: Mean Look, Block, Jaw Lock and Fairy Lock traps, Parting Shot and Pollen Puff on an ally, Quick Guard with Triage and Gale Wings, Utility Umbrella, Weakness Policy, and the `to_bytes` / `from_bytes` round-trip. See [`docs/releases.md`](docs/releases.md).
- **Verification:** [`docs/accuracy/fix-log.md`](docs/accuracy/fix-log.md) records each fix with its Showdown source line. [`docs/accuracy/2026-09-accuracy-proof.md`](docs/accuracy/2026-09-accuracy-proof.md) is the earlier forced-RNG study. CI runs the workspace tests, including the strict golden-battle parity gate, on every PR.

## Features

- Standard Gen 9 doubles with Terastallization, and the Champions overlay (Reg M-B, Reg M-C) with Mega Evolution and Stat Points, selected by format id.
- Doubles targeting, spread damage, redirection, the Protect family, weather, terrain, screens, Tailwind and Trick Room.
- Fixed-size state, no heap allocation inside `step()`, and tables generated at build time from `@pkmn/dex`.
- Snapshots to compact bytes or JSON; positions rebuilt from real replays (`position_from_replay`, `vgc recon`).
- `chance` feature: `Battle::step_chance()` returns every next state of a turn with its probability. `vgc-solver` solves small endgames for mixed policies.
- `ps-rng` feature: a bit-exact port of Showdown's PRNG and draw order, for seeded differential testing ([`docs/accuracy/ps-rng.md`](docs/accuracy/ps-rng.md)).
- Showdown protocol output, and `vgc sim`, a local stand-in for `pokemon-showdown simulate-battle` ([`docs/protocol.md`](docs/protocol.md)).

## Example

Python, after the quickstart:

```python
import json, vgc_engine

team = json.dumps([{"species": "rillaboom", "moves": ["protect"]}] * 2)
b = vgc_engine.Battle.from_teams(team, team, format="gen9doublescustomgame", seed=7)

# Choices are (kind, actor_slot, move_or_team_idx, target_side, target_slot)
p1 = [b.legal_choices(0, s)[0] for s in (0, 1)]
p2 = [b.legal_choices(1, s)[0] for s in (0, 1)]

branch = b.clone()                  # independent copy
result = branch.step_move(p1, p2)   # "continue" | "p1_win" | "p2_win" | "tie"
again = vgc_engine.Battle.from_bytes(branch.to_bytes())

r = vgc_engine.calc("chomp", "rillaboom", "eq")   # r["min"], r["max"]
```

CLI and Rust tests (with `VGC_DEX_DIR` set):

```sh
./target/release/vgc survives chomp lando eq
./target/release/vgc outspeeds chomp lando
cargo test --workspace --exclude vgc-engine-py    # what CI runs
cargo test -p vgc-engine-core --features chance
```

The Rust API is documented in [`docs/protocol.md`](docs/protocol.md), "Using it".

## How it relates to Pokémon Showdown

Showdown is the correctness oracle. vgc-engine reimplements its mechanics and protocol in Rust, and is measured against it: the accuracy reports compare full battles turn by turn, and the golden tests replay scripted battles against Showdown-recorded results. Where Showdown and the cartridge disagree, vgc-engine follows Showdown. It is not affiliated with Showdown. Design background, including why this isn't a fork of `pkmn/engine`, is in [`docs/DESIGN.md`](docs/DESIGN.md).

## Project layout

| Path | What it is |
|---|---|
| `crates/vgc-engine-core` | The simulator |
| `crates/vgc-engine-data` | Build-time tables from `@pkmn/dex` |
| `crates/vgc-engine-py` | Python bindings (`vgc_engine`) |
| `crates/vgc-engine-cli` | The `vgc` CLI |
| `crates/vgc-solver` | Outcome frontiers and endgame solver |
| `crates/vgc-engine-replay` | Showdown replay parser |
| `crates/vgc-engine-golden`, `vgc-engine-conformance` | Differential harnesses against Showdown |
| `tools/` | Accuracy, protocol and perf tooling |
| `docs/` | Plan, design, accuracy reports, protocol |
| `product-page/` | The static site at vgc-engine.colefoster.ca |

## Contributing

Read [`docs/AGENTS.md`](docs/AGENTS.md) first. In short: one mechanic per PR, add a golden test for it, cite the Showdown file and line, and keep allocations out of `step()`. [`docs/REFERENCES.md`](docs/REFERENCES.md) lists the sources of truth. The roadmap is [`docs/PLAN.md`](docs/PLAN.md).

## License and attribution

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

Built binaries include data from `@pkmn/dex` and Pokémon Showdown (both MIT). See [`NOTICE`](NOTICE) and [`ATTRIBUTION.md`](ATTRIBUTION.md) for every third-party source.

Pokémon is a trademark of Nintendo, Creatures and GAME FREAK. This is an independent project.
