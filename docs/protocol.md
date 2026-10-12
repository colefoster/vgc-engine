# Showdown protocol log

vgc-engine can write the battle log Pokémon Showdown writes: the same lines,
in the same order, byte for byte, including the `|split|` pairs that give
each side its own view (phase 1). It also produces each side's `|request|`
JSON and the per-player streams PS's BattleStream delivers, takes choices
in PS syntax, and builds a player's partial observation from its own stream
(phase 2, [Requests, streams and sessions](#requests-streams-and-sessions)).
Phase 3 adds a BattleStream-compatible stdio front end on top.

**Model input.** A model that should see only its own player's view must
read `Battle.observe(side)` (or `player_stream(side)` / `request(side)`),
never `Battle.observe()`. `observe()` is the full engine state, both teams
exact. See [Observations](#observations).

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

Phase 2 adds three null-checked hooks (the stop after each `step`, the
mid-turn stop, the randomNormal target pick) and builds requests only at
stops. `ps-protocol-2` vs main (`91a9fd5`), 6 interleaved runs: ~452.9k vs
~451.8k steps/s, 0 allocations per step for both.

## Requests, streams and sessions

### Python

```python
b = vgc_engine.Battle.from_teams(p1_json, p2_json, format="gen9championsvgc2026regmc",
                                 seed=7, ps_session=True, player_names=("Alice", "Bob"))
while not b.ended:
    for side in ("p1", "p2"):
        req = b.request(side)          # PS |request| JSON, or None (nothing to choose)
        if req is None:
            continue
        err = b.choose(side, pick(req))  # "team 1234", "move 1 2, switch 3", "move 2 terastallize", "pass", "default"
        if err:                          # PS's exact text: "|error|[Invalid choice] Can't move: ..."
            ...
lines = b.player_stream("p1", since=0)   # p1's stream: updates (p1's secrets), its |request|s and |error|s
obs = b.observe("p1")                    # partial observation, from p1's stream only
```

`ps_session=True` implies `protocol="ps"`. The battle starts at team
preview when the format has one (Champions, gen 9 custom games). Both sides
answer each request; when the last one does, the battle runs to the next
request. `request(side)` is `None` for a `wait` request, after the side's
choice was accepted, and once the battle is over (`b.ended`). A side may
re-choose before the other side answers (Cancel Mod). `player_stream`
takes `"p1"`, `"p2"`, `"spectator"` or `"omniscient"` (or 0 / 1) and a line
offset; `player_chunks` returns whole chunks (one per PS `update` /
`sideupdate`). `force_tie()` is PS's `>forcetie`. `step_*` raise on a
session battle; `observe()`, `legal_choices`, `protocol_log` etc. read the
committed state (the turn's start while a mid-turn request is open).

### Rust

`Battle::protocol_log().unwrap().sync_points()` lists every point PS stops
for input, with both sides' requests (`requests[side]`, the JSON string;
`info[side]`, the same request structured as `ps_request::SideReq`).
`ProtocolLog::stream(view, upto)` renders PS's `getPlayerStreams` chunks.
`ps_session::PsSession` is the choose/request driver the Python binding
wraps; it takes a builder closure (team-preview picks -> `Battle`).

### What matches PS

- **Requests** (`ps_request.rs`): `getRequests` (sim/battle.ts:1418-1465),
  `getSwitchRequestData` and `getMoveRequestData` (sim/pokemon.ts:1083-1196)
  serialized as PS's `JSON.stringify` does: key order, `teamPreview` +
  `maxChosenTeamSize`, `forceSwitch`, `wait`, `active[].moves[]` (`move`,
  `id`, `pp`, `maxpp`, `target`, `disabled`; locked moves, Recharge and
  Struggle in their short forms), `trapped` / `maybeTrapped` (Shadow Tag,
  Arena Trap and Magnet Pull hidden on the last active, species-possible
  trappers, Shed Shell), `maybeDisabled` / `maybeLocked` (Imprison),
  `canMegaEvo`, `canTerastallize`, `side.pokemon[]` in PS's current order
  (team-preview picks, switches, Ally Switch) with `ident`, `details`,
  `condition`, `active`, `stats` (baseStoredStats), `moves`, `baseAbility`,
  `item`, `pokeball`, `ability`, `commanding`, `reviving`, and `teraType` /
  `terastallized` outside Champions. A fainted mon shows what PS's
  clearVolatile leaves (base ability, base moves, no Tera, volatile-free
  disables). `noCancel` follows Cancel Mod (only when one side is asked).
  The simulator never writes `rqid` (PS's server adds it), so neither does
  the engine.
- **Streams**: each `update` with `|split|` resolved for the channel, then
  each side's `|request|` (sim/battle.ts:3267-3274: update first, then the
  requests; PS sends nothing when the log did not grow). The preamble's
  first two updates are split where PS's `>start` / `>player p1` inputs
  end.
- **Choices** (`ps_session.rs`): a port of `Side.choose`, `chooseMove`,
  `chooseSwitch` (number or name), `choosePass`, `chooseTeam`,
  `chooseShift` and `autoChoose` (sim/side.ts:552-1378) against the request
  the side was sent, error texts included: `[Invalid choice]`, and
  `[Unavailable choice]` plus the updated request (`"update":true`) when a
  choice runs into hidden information (a `maybeTrapped` mon that is
  trapped, an Imprisoned move). `move N` names PS's request order;
  `switch N` names PS's current side order.
- **Mid-turn requests** (U-turn, Volt Switch, Parting Shot, Flip Turn,
  Eject Button, Eject Pack, Emergency Exit, Revival Blessing): the engine
  resolves a turn in one `step` with the picks given up front, so the
  session replays the turn from its start with the picks made so far; the
  first unanswered stop the replay records is where PS is waiting, and
  the streams stop there. Replays are deterministic, so what a player
  already saw never changes.

### Observations

`Battle.observe(side)` (`ps_observe.rs`) reads nothing but the side's
stream and its requests:

- **Own team**: the latest request's `side.pokemon` (exact HP, status,
  stats, item, ability, base ability, all moves, Tera type) and the
  actives' PP (`move_slots`); boosts and volatiles from the stream.
- **Opponent**: species from team preview (`revealed: false` until a mon
  switches in), then name, details and HP as the stream shows them (a
  percentage: `max_hp` 100, `hp_exact` false), status, boosts, volatiles.
  `moves` holds only moves the mon was seen using (`|move|`, a `cant`
  naming one); `item` / `ability` are `None` until a line names them
  (`-item`, `-enditem` -> `""`, `-ability`, `[from] item: X` / `[from]
  ability: X` on the source mon, Frisk, Skill Swap, a Mega Stone);
  `terastallized` once it Terastallizes.
- **Field**: `weather`, `terrain`, `field` (pseudo-weathers), each side's
  `conditions` (layers for hazards), `turn`, `ended`, `winner`, and the
  side's raw `request`.

This is the observation to feed a model that must not see hidden
information. `observe()` (no side) is unchanged: the full engine state.

## Phase 2 gate

```sh
node tools/accuracy/ps-battle.js <jobs.jsonl> <outdir>      # now also records _meta.streams and _meta.cmds
ACC_THREADS=4 ./target/release/accuracy protocol <outdir>   # log + per-side requests + streams
ACC_THREADS=4 ./target/release/accuracy session <outdir>    # PS's choice strings through PsSession
```

`protocol` compares each side's `|request|`s position by position (the
harness's own rejected choices and their updated requests set aside) and
every stream chunk by chunk, after the keyed replay. `session` feeds every
choice string PS's players sent, rejected ones included, to a `PsSession`
under the keyed RNG and compares its four streams with PS's, `|error|`
lines and updated requests included. Results at `ps-protocol-2` head (PS
`a5df8274`, Champions Reg M-C, state-clean battles):

| Sample | Requests byte-identical | p1+p2 streams (log stops) | p1+p2 streams (session) |
|---|---|---|---|
| B (development) | **34505 / 34568 (99.82%)** | 1237 / 1287 (96.1%) | **1237 / 1287 (96.1%)** |
| C (held out) | **34337 / 34348 (99.97%)** | 1228 / 1290 (95.2%) | **1229 / 1290 (95.3%)** |

In both samples, every battle whose log is byte-identical also has
byte-identical p1, p2, spectator and omniscient streams through the
session, which rejects exactly the choices PS rejected, with PS's text. A gen 9 Doubles
Custom Game set (40 random battles: Tera, Shadow Tag, Arena Trap, Shed
Shell, U-turn, rejected choices) matches 1126 / 1126 requests; its stream
updates carry the `|debug|` lines of a `debug` format, which the log does
not write.

The phase-2 request gate also found engine state bugs, fixed on this
branch: Champions Night Slash PP, Knock Off on a knocked-out target,
Pressure's PP targets, and a Choice item picked up mid-charge.

### Remaining request divergences

All below 0.5% of a sample (battles with a first mismatch):

- **Cosmetic formes, 2 (B).** Alcremie-Salted-Cream shows as Alcremie (the
  phase-1 `|poke|` bucket).
- **Disabled flag, 1 (B) + 2 (C).** A move PS disabled at endTurn that the
  engine's request still offers (an Encore / lock interaction).
- **PP, 1 (C).** After You's PP under Pressure.
- **One-offs, 1 battle each.** A `noCancel` where PS asked both sides, a
  transformed mon's max PP after switching, an item PS had already
  removed, a missing `forceSwitch`.

Stream divergences beyond these are the phase-1 log buckets.

## What phase 3 still needs

- **Stdio framing.** A `simulate-battle`-compatible front end: `>start`,
  `>player`, `>p1` / `>p2` lines in; `update`, `sideupdate` and `end`
  blocks out. `PsSession` already produces the update / sideupdate chunks
  in PS's order (`stream`); the front end must split `>player` options
  (name, team in packed or JSON form, `seed`, `formatid`) and the `end`
  block (winner, seed, turns, score, inputLog) the engine does not build.
- **Undo / Cancel**: `>p1 undo` and re-choosing (Cancel Mod's
  `cantUndo` after a `maybeTrapped` switch) are not modelled.
- **Formats.** Team preview, `|tier|` and `|rule|` lines exist for the
  Champions formats and gen 9 custom games. Other PS formats (VGC 2025
  Reg I, ...) fall back to a custom-game preamble without team preview;
  `|debug|` lines of debug formats are not written.
- **PS randomness outside replay.** Speed-tie orders the engine doesn't
  draw (residual handlers, eachEvent) follow field order, and a Struggle
  or other random-target move with no target hits the first foe rather
  than a random one, so logs are self-consistent but not PS-identical
  unless PS's draws are supplied. The `ps-rng` feature (PS's own PRNG) is
  the route to that.
