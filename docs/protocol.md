# Showdown protocol log

vgc-engine can write the battle log Pokémon Showdown writes: the same lines,
in the same order, byte for byte, including the `|split|` pairs that give
each side its own view. This is phase 1 of making the engine a local
drop-in for PS. Phase 2 adds per-side `|request|` JSON and partial
observations in Python. Phase 3 adds a BattleStream-compatible stdio front
end. Both build on the log described here.

The log is opt-in. A battle built without it carries no log, allocates
nothing for it, and only runs a null check at each hook (see
[Cost](#cost)).

## Using it

### Rust

```rust
use vgc_engine_core::{Battle, BattleConfig, ProtocolMode, ProtocolOptions, ProtocolView, Rng};

let opts = ProtocolOptions {
    player_names: ["Alice".into(), "Bob".into()],
    ..ProtocolOptions::new(ProtocolMode::Ps)
};
// format_id picks the preamble (|tier|, |rule|, team preview);
// team_picks are the team-preview choices (0-based, in pick order).
let mut b = Battle::with_protocol(cfg, Rng::new(seed), p1, p2, &opts, "gen9championsvgc2026regmc", Some(&picks));
b.step(&c1, &c2);
let ps_log: &[String] = b.protocol_log().unwrap().raw_lines(); // PS battle.log, |split| pairs included
let p1_view: Vec<String> = b.protocol_view(ProtocolView::P1).unwrap();
```

`ProtocolOptions` fields:

| Field | Meaning |
|---|---|
| `mode` | `Ps` (byte-exact) or `Compact` (see below) |
| `player_names` | `|player|` / `|win|` names; PS defaults to "Player 1" / "Player 2" |
| `nicknames` | per side, per team index; `None` is PS's default (the base species) |
| `timestamp` | a fixed `|t:|` value; `None` uses wall-clock seconds as PS does |
| `format_id` | overrides the preamble's format id |

`Battle::protocol_force_tie()` writes PS's `|tie` (a `--max-turns` cutoff).

### CLI

```sh
vgc battle p1.json p2.json --seed 7 --format gen9championsvgc2026regmc \
    --protocol ps --view omniscient --max-turns 200
```

Plays one seeded battle with random legal choices and prints its log.
Teams are JSON or Showdown exports. `--protocol` is `ps` or `compact`;
`--view` is `omniscient`, `spectator`, `p1` or `p2`.

### Python

```python
b = vgc_engine.Battle.from_teams(p1_json, p2_json, format="regmc",
                                 protocol="ps", player_names=("Alice", "Bob"))
b.step(...)
lines = b.protocol_log(view="p1")   # list[str]; None without protocol=
```

`protocol=None` (the default) keeps no log.

## Modes

`ps` reproduces PS's `battle.log` exactly: `|t:|` timestamps, `|j|`, the
blank `|` separators, `|gametype|`, `|player|`, `|gen|`, `|tier|`,
`|rule|`, `|uhtml|` / `|uhtmlchange|` (open team sheets) and `|-hint|`.

`compact` drops those cosmetic lines (and `|rated|`) and keeps every line
that carries battle information. Compact = ps minus the cosmetic lines,
so a compact log never disagrees with the ps log on order.

## Views

PS `extractChannelMessages` (sim/battle.ts:35-59):

| View | `|split|pN` pair | Use |
|---|---|---|
| `raw_lines()` | the pair as written (`|split|pN`, secret, shared) | PS `battle.log` |
| `omniscient` | secret half | full information |
| `spectator` | shared half | public view |
| `p1` / `p2` | secret half for that side, shared half for the other | a player's view |

Secret halves carry exact HP (`hp/maxhp`). Shared halves carry the public
percentage: Champions formats floor it (minimum 1) with the 20% / 50%
colour bands; standard formats use PS's ceil rule.

## Snapshots, clones and RNG

The log lives in `Battle::protocol: ProtoSlot`, an
`Option<Box<ProtocolLog>>`. It is `#[serde(skip)]`, so `to_bytes` /
`to_json` snapshots are identical with and without a log, and `Clone`
drops it, so solver and search clones never carry or copy it. Logging
draws no random numbers and changes no state: a logged and an unlogged
battle with the same seed and choices stay identical (unit test
`logging_does_not_change_the_battle_or_its_snapshots`).

## Ordering

The engine resolves some things in a different order from PS. The log puts
them back in PS's order.

- **Moves.** PS runs a move's hit steps across all targets, step by step
  (sim/battle-actions.ts trySpreadMoveHit, data/mods/champions/scripts.ts
  spreadMoveHit). The engine resolves target by target. A damaging move's
  lines are buffered in a window keyed by (hit, step, target, handler
  sub-order) and sorted when the move ends. The steps are TryHit
  (priority classes for Psychic Terrain, Protect and default handlers),
  type immunity, accuracy, damage calc, damage, effects, secondaries,
  DamagingHit (with `onDamagingHitOrder`), AfterHit and the hit's Update.
  After the loop come faintMessages, `-hitcount`, recoil, Emergency Exit
  and `selfBoost`.
- **Residuals.** PS's `fieldEvent('Residual')` collects every handler of
  the field, both sides and every active, speed-sorts them (a selection
  sort that shuffles ties) and runs faintMessages after each handler that
  didn't end by duration (sim/battle.ts:484-567). `protocol_residual.rs`
  models that handler list and keys each residual line by its handler's
  position. A faint prints after the first later handler that runs
  faintMessages, or after `|upkeep|` if none does.
- **Faints and wins.** The faint queue keeps PS order: KO order within a
  spread move, the user of Final Gambit / Explosion first. `|win|` follows
  PS's checkWin points: the hit loop only checks when the attacker
  fainted, runMove's end always checks, and residual faints check after
  their handler.

Speed ties. PS breaks speed ties with its PRNG, inside `speedSort`. The
engine does not draw those shuffles, so tied residual handlers and
eachEvent order fall back to field order. A harness that replays PS can
pass PS's recorded shuffles in with `Battle::protocol_set_oracle_shuffles`.
The conformance harness does this. With them, residual ties come out
exactly as PS ordered them.

## Conformance gate

```sh
node tools/accuracy/ps-battle.js <jobs.jsonl> <outdir>   # captures _meta.rawlog (PS battle.log)
ACC_THREADS=4 ./target/release/accuracy protocol <outdir> --jsonl proto.jsonl
```

Each battle is replayed with every PS random outcome keyed in, so the state
cannot drift. The engine's `ps` log is then compared with PS's
`battle.log`, with `|t:|` values normalized. Only state-clean battles
count, meaning the battle state matched PS every turn. The report gives
byte-identical battles, LCS line match, and the first-divergence buckets.

Results at `ps-protocol-1` head (PS `a5df8274`, Champions Reg M-C):

| Sample | State-clean | Byte-identical | Line match |
|---|---|---|---|
| B (development) | 1287 / 1300 | **1237 (96.1%)** | 99.96% |
| C (held out) | 1291 / 1300 | **1228 (95.1%)** | 99.96% |

`tests/accuracy_protocol.rs` replays five checked-in captures
(`tools/accuracy/repros/protocol/`) and requires byte identity. The
captures cover a residual tie shuffle under Grassy Terrain, Illusion,
Dragon Darts, Perish Song faints and a plain win.

### Remaining divergences

No bucket is above 0.5% of a sample. The largest:

- **Simultaneous replacement order, 6 per sample.** Replacements after
  faints are ordered by a PS queue-sort tie shuffle that the replay
  doesn't hand to this path. State is unaffected.
- **Spread drain (Matcha Gotcha, Parabolic Charge), 2-3.** PS heals once
  per target, right after that target's damage. The engine heals once
  for the summed damage.
- **Cosmetic formes in `|poke|`, 2.** For example Alcremie-Salted-Cream.
  The engine's species table folds cosmetic formes into the base species.
- **Random-target display picks, 2.** Eruption and other spread moves
  with all targets protected print an initial random target that the
  keyed replay doesn't always recover.
- **Singletons, 1 battle each.** These include Competitive / Defiant
  zero-change interleaving under a capped stat, substitute-hit message
  order, Air Balloon popping on a substitute hit, Lum Berry curing
  confusion, Role Play / Anticipation / Healer / Mental Herb lines, and
  Speed-tied switch-in ability order.

## Cost

The hooks check `Option<Box<ProtocolLog>>` before formatting anything, and
format only when a log is on. `step()` makes no heap allocations with
logging off: `perf_bench` reports 0 allocations per step.

`perf_bench` (10,000 battles, logging off, interleaved runs) on Apple
Silicon:

| Build | steps/s | vs main |
|---|---|---|
| main (before) | ~460k | — |
| `ps-protocol-1`, `protocol` feature on (default) | ~443k | −3.6% |
| hooks compiled out (`protocol` feature off) | ~450k | within noise of main |

The 3.6% comes from the null checks and the inlined hook stubs in the hot
hit loop. A Rust consumer that never logs can depend on `vgc-engine-core`
with `default-features = false` to compile every hook out. Without the
feature, `ProtoSlot::on()` is constant `false`. The workspace crates and
the Python wheel build with the feature on.

## What phases 2 and 3 still need

- **Per-side `|request|` JSON.** PS's request shape covers active moves
  with PP, disabled and target fields, side pokemon with details,
  condition, stats, item, ability and tera type, `forceSwitch` / `wait`,
  team preview and `rqid`. The engine exposes legal choices but not this
  shape. The side views above are the observation half.
- **Choice parsing and errors.** `>p1 move 1 2, switch 3` parsing,
  `|error|` replies, and team preview / mid-turn switch requests (the
  engine's `decision_phases` mode has the hooks).
- **Stream framing.** `update` / `sideupdate` / `end` blocks for a
  BattleStream-compatible stdio front end.
- **PS randomness outside replay.** Speed-tie orders the engine doesn't
  draw (residual handlers, eachEvent) follow field order, so logs are
  self-consistent but not PS-identical unless PS's draws are supplied.
  The `ps-rng` feature (PS's own PRNG) is the route to that.
