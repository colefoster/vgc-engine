# vgc-engine quickstart

vgc-engine is a pre-alpha, data-oriented Pokémon battle simulator in Rust.
Gen 9 doubles is the current target. It is conformance-tested against
Pokémon Showdown, which remains the source of truth. Accuracy gates are still
open; treat results as an approximation under active correction.

## Requirements

- Rust 1.85 or later (edition 2024)
- Python 3.9 or later and `maturin` (1.7 to below 2.0), for the Python module

## Build and test the Rust workspace

```sh
# The default workspace excludes the pyo3 crate, which builds via maturin.
cargo test --workspace --exclude vgc-engine-py

# Opt-in features
cargo test -p vgc-engine-core --features chance   # step_chance() frontier
cargo test -p vgc-engine-core --features ps-rng   # Showdown PRNG mode
```

## The `vgc` calc CLI

```sh
cargo run --release -p vgc-engine-cli --bin vgc -- calc chomp lando eq
cargo run --release -p vgc-engine-cli --bin vgc -- calc chomp lando eq --spread --json
cargo run --release -p vgc-engine-cli --bin vgc -- survives chomp lando eq
cargo run --release -p vgc-engine-cli --bin vgc -- outspeeds chomp lando
```

Subcommands: `calc`, `survives`, `outspeeds`, `speed`, `best`, `matchup`,
`survive-evs`, `ko-evs`. Specs accept terse strings such as
`"Garchomp @ Life Orb / Jolly / 252 Atk"` and common aliases.

## Build the Python module

```sh
cd crates/vgc-engine-py
maturin build --release        # or: maturin develop --release (inside a venv)
pip install target/wheels/vgc_engine-*.whl   # path depends on your layout
```

The module is imported as `vgc_engine`.

## Step and branch a battle

```python
import json, vgc_engine

team = json.dumps([{"species": "rillaboom", "moves": ["protect"]}] * 2)

# format: "doubles", "singles", or a Showdown format id.
#   gen9champions* / regmb / regmc  -> Champions rules and move data
#   other ids (e.g. gen9doublescustomgame) -> standard Gen 9
b = vgc_engine.Battle.from_teams(team, team, format="gen9doublescustomgame", seed=7)

# Choices are (kind, actor_slot, move_slot_or_team_index, target_side, target_slot).
p1 = [b.legal_choices(0, s)[0] for s in (0, 1)]
p2 = [b.legal_choices(1, s)[0] for s in (0, 1)]

branch = b.clone()                 # full independent copy
print(branch.step_move(p1, p2))    # "continue" | "p1_win" | "p2_win" | "tie"
print(branch.turn, branch.active_hp(1, 0))

blob = branch.to_bytes()           # compact bincode
same = vgc_engine.Battle.from_bytes(blob)
text = branch.to_json()            # inspectable; round-trips via from_json
```

`copy.copy` and `copy.deepcopy` both produce full clones.

## Solve, verify, calc

```python
sol = vgc_engine.solve_endgame_exact(b, max_depth=4, node_budget=100_000)
sol["value"], sol["provenance"], sol["nodes_visited"]
joint = sol["row_joint_policy"][0][0]     # executable with step_move

sol = vgc_engine.solve_endgame(b, lambda observations: [0.0] * len(observations))

problems = vgc_engine.parse_and_verify(showdown_export_text, format="regmb")  # or "regmc"

r = vgc_engine.calc("chomp", "lando", "eq")
r["min"], r["max"]
```

## Notes

- The Python API is wire-level for now. Choices are tuples and will become
  proper objects later.
- `ps_seed=` on `Battle.from_teams` needs a build with the `ps-rng` feature;
  without it, it raises `ValueError`.
- `step_chance()` currently wraps a record/replay pass; native branching is
  planned.
- Earlier generations are not started.

License: MIT OR Apache-2.0.
