# Showdown protocol log

vgc-engine can write the battle log Pokémon Showdown writes: the same lines,
in the same order, byte for byte, including the `|split|` pairs that give
each side its own view (phase 1). It also produces each side's `|request|`
JSON and the per-player streams PS's BattleStream delivers, takes choices
in PS syntax, and builds a player's partial observation from its own stream
(phase 2, [Requests, streams and sessions](#requests-streams-and-sessions)).
Phase 3 puts PS's BattleStream on top: `vgc sim` speaks `pokemon-showdown
simulate-battle`'s stdio protocol, and `vgc_engine.BattleStream` is the
same in-process ([Use as a local Showdown replacement](#use-as-a-local-showdown-replacement)).
Phase 4 starts a session from any mid-game position, a snapshot or a
kcheck board ([Start from a mid-game position](#start-from-a-mid-game-position)).
A real replay's log becomes such a position at any turn, with the hidden
sets filled from usage stats ([Start from a real replay](#start-from-a-real-replay)).

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

Speed ties. PS breaks speed ties with its PRNG, inside `speedSort`. On
PS's own PRNG (`ps-rng`, `Rng::Ps`) the engine draws the residual
handler sort's and the weather's eachEvent shuffles itself and orders the
residual lines by them, as it orders Terastallization and a spread move's
`|move|` target by its own draws. On any other RNG those ties fall back to
field order; a harness that replays PS can pass PS's recorded shuffles in
with `Battle::protocol_set_oracle_shuffles` (the conformance harness
does).

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
- **Locked moves** (PS `getLockedMove`): the request lists one move
  (`{"move":"Recharge","id":"recharge"}` for Hyper Beam's recharge turn,
  else the two-turn or Outrage-style move) with `trapped: true` and no
  Mega / Tera; `move 1` or `default` answers it. `Battle.legal_choices`
  agrees (0.4.2): a recharging mon has exactly
  `[("recharge", slot, 0, -1, -1)]`, a locked move its one `("move", ...)`
  (a charge keeps its stored target), with no switch. `step_move` takes
  the `"recharge"` tuple; it is `move 1`.
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

## Use as a local Showdown replacement

`vgc sim` is a drop-in for `pokemon-showdown simulate-battle`: the same
stdin protocol (PS's `BattleStream`, sim/battle-stream.ts) and the same
stdout bytes (`BattleTextStream`: each message, a newline, a blank line).
The release CLI builds with `ps-rng` by default, so the `>start` seed
drives PS's own PRNG and a battle plays out as PS plays it.

```sh
cargo build --release -p vgc-engine-cli          # target/release/vgc, target/release/vgc-showdown

# instead of:  node pokemon-showdown simulate-battle --skip-build
target/release/vgc sim                            # --protocol ps (default) | compact
target/release/vgc-showdown simulate-battle       # the same, with PS's command shape
```

Flags: `--protocol ps` writes PS's bytes; `--protocol compact` drops the
cosmetic lines from `update`s (see [Modes](#modes)). `--replay` and
`--spectate` are PS's (only the omniscient / spectator log). `--debug` and
`--skip-build` are accepted and ignored.

Input, as PS takes it (one `read()` of stdin is one chunk: its lines run,
then one update goes out):

| Line | Effect |
|---|---|
| `>start {"formatid":"gen9vgc2025regi","seed":"sodium,<hex>"}` | `seed` may also be `"gen5,<hex>"`, `"a,b,c,d"` or `[a,b,c,d]`; none draws a random one |
| `>player p1 {"name":"Alice","avatar":"1","team":"<packed>"}` | `team` packed (`Teams.pack`) or a JSON array of sets; PS's set edits apply (default name, level, EVs / IVs) |
| `>p1 move 1 2, move 3 terastallize` / `>p1 switch 3` / `>p1 team 1234` / `>p1 default` / `>p1 pass` | PS's choice syntax and `|error|` texts (phase 2) |
| `>p1 undo` | drop p1's choice (Cancel Mod formats), or PS's `Can't undo` error after a trapped / locked reveal |
| `>forcewin p1` / `>forcetie` / `>forcelose p2` / `>tiebreak` | end the battle as PS does, at any stop (team preview, mid-turn) |
| `>chat text` / `>chat-inputlogonly text` | `|chat|text` in the log / input log only |
| `>eval code` | logged like PS, answered `||<<< error: vgc-engine has no JavaScript to evaluate` |
| `>requestlog` / `>requestexport` / `>requestteam p1` | `requesteddata` |
| `>reseed <seed>` | the battle draws from the new seed |

Output: `update\n<log lines>` (with `|split|pN` pairs, `|t:|` wall-clock
seconds), `sideupdate\npN\n|request|<json>` / `|error|...`, and at the end
`end\n{"winner":...,"seed":...,"turns":...,"p1":...,"p2":...,"p1team":[...],"p2team":[...],"score":[...],"inputLog":[...]}`
(PS a5df8274 writes no `log` field). Sending nothing after `end` is fine;
input after it is read and ignored, and the process exits at end of input.

Formats: every gen 9 and Champions format PS has for player-built teams
(`crates/vgc-engine-core/src/ps_formats.json`, generated from PS by
`tools/protocol/ps-formats.js`): the `|tier|` / `|rule|` lines, Team
Preview and `pickedTeamSize`, Open Team Sheets, Cancel Mod. Mechanics are
the engine's: standard gen 9, or Champions for the `champions*` mods.

### From a harness

A harness that runs PS's `simulate-battle` changes one command line:

```sh
- node pokemon-showdown simulate-battle --skip-build
+ /path/to/vgc-engine/target/release/vgc sim
```

A harness that loads PS's `dist/sim` in-process (`BattleStream`,
`getPlayerStreams`) can point its PS directory at
`tools/protocol/ps-shim`, whose `BattleStream` spawns `vgc sim` and whose
other exports (Teams, TeamValidator, Dex, getPlayerStreams) come from a real
PS checkout:

```sh
PS_DIR=tools/protocol/ps-shim PS_REAL_DIR=/path/to/pokemon-showdown \
  VGC_SIM="/path/to/vgc-engine/target/release/vgc sim" node arena.mjs ...
```

### Python

```python
import json, vgc_engine
s = vgc_engine.BattleStream()                     # protocol="ps" | "compact", replay=, spectate=
s.write('>start {"formatid":"gen9championsvgc2026regmc","seed":"sodium,0123456789abcdef0123456789abcdef"}')
s.write('>player p1 ' + json.dumps({"name": "Alice", "team": packed_a}))
s.write('>player p2 ' + json.dumps({"name": "Bob", "team": packed_b}))
streams = s.player_streams()                      # PS getPlayerStreams: omniscient, spectator, p1, p2
for chunk in streams.p1.read_all():               # p1's view of each update, its |request| / |error|
    ...
streams.p1.write("team 1234")                     # sent as ">p1 team 1234"
s.ended, s.input_log
```

Without `player_streams()`, `s.read()` / `s.read_all()` return the raw
messages (`"update\n..."`, `"sideupdate\np1\n..."`, `"end\n..."`). For PS's
draws the wheel needs the `ps-rng` feature
(`maturin build --release --features ps-rng`); without it a seed still
gives a deterministic battle, on the engine's own RNG.

## Phase 3 gate

```sh
cargo build --release -p vgc-engine-cli
PS_DIST=<pokemon-showdown a5df8274>/dist/sim node tools/protocol/sim-diff.js \
  --jobs B/jobs.jsonl,C/jobs.jsonl --count 1000 --out <dir> --vgc target/release/vgc
python3 tools/protocol/sim-diff-report.py <dir>
```

`sim-diff.js` plays each battle on PS in-process with a seeded random
policy over PS's requests: legal choices, plus about 6% invalid ones, 5%
`undo`s, 3% re-choices, `>chat`, an occasional `>forcewin` / `>forcelose` /
`>forcetie`, and `>tiebreak` or `>forcetie` past turn 60. The recorded input
(one write per player message) is then fed write by write to PS's real
`pokemon-showdown simulate-battle` and to `vgc sim`, both with the same
seed, and the two stdouts are compared byte for byte (`|t:|` values
normalized). PS's subprocess output also has to match its in-process run.
A divergence is classed by the PRNG draws: PS's (as `random(m, n)` spans)
against the engine's trace (`VGC_DRAW_TRACE`). If they part before the
first differing output it is **draw order**; if they agree it is
**formatting** when every `|request|` and the end record still match PS,
else a **mechanic**.

Results (branch `ps-protocol-3`, PS a5df8274, 1000 battles, 500 from
each corpus run):

| Format | Identical streams |
|---|---|
| gen9championsvgc2026regmb | 244 / 250 (97.6%) |
| gen9championsvgc2026regmc | 242 / 250 (96.8%) |
| gen9vgc2024regg | 242 / 250 (96.8%) |
| gen9vgc2025regi | 237 / 250 (94.8%) |
| **All** | **965 / 1000 (96.5%)** |

PS's subprocess output matched its in-process run in all 1000. Of the 35
divergent battles:

- **Draw order, 17 (1.7%)**: mostly a damage roll or crit drawn in a
  different order around a spread move or ability (9 first differ at a
  `-damage` line), Cute Charm / Flame Body / Poison Touch contact rolls,
  and Blizzard's accuracy target order.
- **Formatting, 9 (0.9%)**: same draws, same requests and end record,
  different log lines: Sitrus Berry eaten before vs after a switch-in
  ability, replacement switch-in order on a Speed tie, Toxic Spikes vs
  Stealth Rock line order, Lightning Rod redirect line, Revival Blessing
  weather line, one HP-display rounding.
- **Mechanics, 9 (0.9%)**: Receiver and Spirit Shackle's trap are not
  implemented; spread-move drain heals per target, not once; Ditto /
  Imposter request contents; Alcremie cosmetic forme in `|poke|`; Protean
  on a failed move; Expanding Force's Psychic Terrain spread; double
  replacement switch-in order; one end record's `score` (`pokemonLeft`).

### Known limits

- `|debug|` lines (debug / custom-game formats) are not written.
- Random-team formats are unsupported (player-built teams only);
  `>editbattle` is not handled; `|rated|` is the bare line.
- Cosmetic formes (Alcremie, Vivillon, ...) battle as the base forme;
  `|poke|` / `|switch|` details may lack the forme.
- Champions stat points convert to EVs exactly only at level 50.
- Shiny is accepted and not shown.
- `vgc sim` treats each `read()` of stdin as one chunk, as PS does; a
  writer whose separate writes reach the pipe coalesced gets one update
  where PS would also give one (PS's `simulate-battle` reads the same
  way), but a harness that expects one update per line should write one
  chunk at a time and wait for output.
- The `ps-shim` `BattleStream` assumes `vgc sim` flushes once per input
  chunk (it does).

### Cost of `vgc sim`

`vgc sim` runs `Rng::Ps` (one ChaCha20 block per draw, plus PS's draw
emulation) and keeps the protocol log: it is for PS-compatible play, not
search throughput. Nothing in `step()` changed for a battle without a log;
see [Cost](#cost) for the logging-off numbers.

`ps-protocol-3` vs main (`3251ded`), `perf_bench --battles 5000`, logging
off, 6 interleaved runs: ~2,401 vs ~2,376 ns/step (~416.5k vs ~420.8k
steps/s, -1.0%), 0 allocations per step for both. The battles differ
slightly (the branch fixes mechanics that change their course), so
ns/step is the comparable figure.

## Start from a mid-game position

Phase 4 starts a PS session from any position: a `to_json()` snapshot, or
a position built from a kcheck board. `request(side)`, `choose(side, ...)`,
`player_stream(side)` and `observe(side)` then work as in a session that
played up to there.

### Python

```python
js = vgc_engine.position_from_board(board, sets, format="gen9championsvgc2026regmc", side_of_own="p1")
b = vgc_engine.Battle.from_position(js, ps_seed="1,2,3,4", prefix={"p1": p1_lines, "p2": p2_lines},
                                    reveals={"p1": {"0": {"moves": ["Earthquake"], "item": "Life Orb"}}})
b.request("p1"); b.choose("p1", "move 1 1, move 2"); b.player_stream("p1"); b.observe("p1")
```

```
Battle.from_position(battle_json, ps_seed=None, prefix=None, reveals=None,
                     switch_in_effects=False, protocol="ps", format=None, player_names=None)
vgc_engine.position_from_board(board, sets=None, format="gen9championsvgc2026regmc",
                               side_of_own="p1") -> str   # Battle JSON
```

- `battle_json`: a `to_json()` string or dict, or `position_from_board`
  output. It is not checked against the prefix.
- **Turn.** The JSON's `turn` is the number of turns played. The session
  stops at the move request of turn `turn + 1`. If an active has fainted
  and a replacement is available, it stops at the faint-replacement request
  instead, as PS does, before any `|turn|`.
- **Switch-in effects** (Intimidate, weather and terrain setters, Booster
  Energy, ...) are not applied: the position is already on the field.
  `switch_in_effects=True` runs the actives' switch-in abilities and items
  in Speed order, as the battle start does. Their lines follow the prefix.
- **`ps_seed`** replaces the battle's PRNG with PS's, seeded as
  `from_teams(ps_seed=...)` seeds it (`ps-rng` wheels). The continuation is
  deterministic per seed. Without `ps_seed` the snapshot's PRNG state is
  kept (`to_json` includes it). A `position_from_board` battle carries the
  engine's own RNG, so pass `ps_seed` for PS's draws.
- **`format`** picks the preamble, team preview and HP display. Default:
  the JSON's `format_id` (set by `position_from_board`), else Champions Reg
  M-C for a Champions battle, else `gen9vgc2025regi` (doubles) or `gen9ou`.
  A `debug` format would show exact HP to everyone, so it is never the
  default.
- **`player_names`** default to the prefix's `|player|pN|Name|` lines, else
  "Player 1" / "Player 2".
- **`protocol`** is `"ps"` or `"compact"`.

### Prefix framing

The prefix is the history each player has seen. It takes one of two forms:

- `{"p1": [lines], "p2": [lines]}`: each player's stream, as
  `player_stream(side)` gives it. `|request|` and `|error|` lines, and empty
  lines, are dropped. The two streams are merged into one PS log. Equal
  lines are shared. A pair that differs only after its subject (an HP line)
  becomes a `|split|` pair owned by the subject's side. A line only one side
  has is a `|split|` line the other side does not see. Open Team Sheets'
  `|uhtml|` box is one line per player.
- `[lines]`: PS's raw `battle.log`, with `|split|pN` followed by the secret
  and the shared line. A list without `|split|` lines is shown to every
  stream as is. This is the form for synthlog's single spectator log.

Every stream (p1, p2, spectator, omniscient) starts with one `update` chunk:
its view of the prefix, verbatim. Player streams then get the stop's
`|request|` chunk. When the turn resolves, the normal `update` follows, then
the next requests. `player_stream(side)` returns the prefix lines as given,
then `|request|...`. `protocol_log()` includes the prefix.

The log's own bookkeeping is rebuilt from the prefix (its omniscient view)
and the state:

- PS names and the details formes come from `|switch|` / `|drag|` /
  `|replace|` / `|detailschange|`. Mons the prefix never shows get the
  base-species name.
- An Illusion holder's disguise comes from the name its slot last showed.
- The Wish user, a partial trap's move, Zero to Hero's shown message and
  Magic Room's source are rebuilt the same way.
- The PS team order comes from the state (`Side::ps_order`).
- The last `|turn|` number is rebuilt from the prefix.

On `ps-rng`, the state a snapshot drops is rebuilt: the field PS last saw
(its WeatherChange / TerrainChange sorts) and each mon's cached Speed.

**No prefix.** The engine writes a short header in PS's lines:

1. The preamble and team preview's `|poke|`s.
2. `|teamsize|`, `|start`, then the live actives' `|switch|`es (shared
   halves as percentages).
3. Tera, boosts and the common volatiles (`Substitute`, `confusion`, Leech
   Seed, Taunt, Encore, Disable, Yawn, Heal Block, Magnet Rise, Aqua Ring,
   Ingrain, Focus Energy, Salt Cure, `perishN`).
4. Weather, terrain, Trick Room / Gravity / Magic Room / Wonder Room, then
   each side's conditions and hazards.
5. `|turn|N`.

The header names no opponent move, item or ability.

### Reveals

`reveals={"p1": {"<opponent team index>": {"species", "moves": [...], "item",
"ability", "tera"}}, "p2": {...}}`. Keys may be ints or strings.

- **Default.** Reveals are parsed from the stream, which `observe(side)`
  already reads.
- **Override.** The given reveals are merged on top of the stream's in
  `observe(side)`:
  - moves are added (ids, no duplicates);
  - `item` / `ability` / `tera` (the mon's `tera_type`) replace what the
    stream showed;
  - `species` relabels the mon.
- **Matching.** A mon is matched by its PS name, then as an unseen
  team-preview entry of its species. If neither matches, it is added.
- **Scope.** Reveals never change a `|request|`: a request describes only
  the requester's own side.
- **Team indices** are the battle's team order. For `position_from_board`
  that is the actives in board order, then the bench in board order.

### Builder (`position_from_board`)

The board uses mimikyu's kcheck field names (`audit/litmus/kcheck/schema.py`,
`_common.mon` / `field`), so kcheck needs no translation layer.

**`sets`** gives each mon's full set:

- Fields: `species`, `nature`, `evs` (EVs) or `sps` (Champions stat points,
  converted as PS Champions does: `8 * sp - 4` EVs), `ivs`, `item` (the item
  it started with), `ability`, `moves`, `tera`, `gender`, `level`.
- Shapes: parallel lists `{"own_active": [...], "own_bench": [...],
  "opp_active": [...], "opp_bench": [...]}`, or `{"own": {species: set},
  "opp": {species: set}}`.
- A missing set uses the board mon's kit, a Serious nature and no EVs.

**Turn.** Elapsed turns follow kcheck: `<effect>_elapsed` counts the turns
up, the setting turn included. 1 (the default) means the effect was set
after last turn's residuals. 2 means it was set by a move last turn. The
engine counts the turns left including this one:
**`remaining = duration + 1 - elapsed`**.
`ps_board::tests::remaining_matches_the_engine_counters` pins this rule to
the engine's own counters.

| kcheck | Engine | Rule |
|---|---|---|
| `own_active` / `opp_active` | `side.active` | Exactly the format's active count. `side_of_own` picks which side `own_*` plays (p1 by default) |
| `own_bench` / `opp_bench` | rest of the team | Team order: actives, then bench. At most 6 mons |
| mon `species` | `species_id` | A `-Mega` forme builds the base species with its stone and Mega Evolves it (`mega_used`). Another forme than the set's (Palafin-Hero) is applied |
| `hp` / `alive` / `fainted` | `current_hp`, `fainted` | `round(hp * max_hp)`, at least 1 while alive; `hp <= 0` or `alive: false` faints it |
| `status` | `status` | `brn par slp frz psn tox`; `""` / `ok` none |
| `toxic_counter` | ToxicCounter payload | 1-based, cap 15. 0 with `tox`: `toxic_n + 1`, else 1 |
| `sleep_turns_remaining` | Sleep payload | Remaining turns (1-3 Champions, 1-4 gen 9). 0 with `slp`: `max(1, 3 - sleep_elapsed)` |
| `boosts[7]` | `boosts` | `[atk, def, spa, spd, spe, acc, eva]`, actives only |
| `volatile_statuses` (+ `volatile_ages`) | volatiles | Duration volatiles get `duration - age`, age default 1: TAUNT 3, ENCORE 3, DISABLE 4, YAWN 2, THROATCHOP 2, HEALBLOCK 5, MAGNETRISE 5. `volatile_remaining: {"ENCORE": 3}` overrides: PS adds a turn to Taunt / Encore / Disable started after the target moved that turn. ENCORE needs `locked_to_move` or `last_move`; DISABLE takes `disabled_move` (default `last_move`). CONFUSION: `confusion_turns_remaining`, default 2 (hidden). LEECHSEED / YAWN / CURSE / ATTRACT: the source is the opposing slot a. STOCKPILE: `stockpile_layers`, default 1. PERISH1-3 is the count. Also SUBSTITUTE, AQUARING, INGRAIN, FOCUSENERGY (+2 crit stage), CHARGE, SALTCURE, SMACKDOWN, TARSHOT, NORETREAT, MINIMIZE, FLASHFIRE, IMPRISON, GLAIVERUSH, MUSTRECHARGE, PROTOSYNTHESIS*/QUARKDRIVE* (`boosted_stat`, held). Others raise |
| `substitute_hp_frac` | Substitute payload | Fraction of `floor(max_hp / 4)`, up to 4 (a Shed Tail doll). SUBSTITUTE without a fraction is a full doll |
| `turns_on_field` | `turns_active`, `move_actions` | Both `k - 1`. 1 = no move action yet (Fake Out works) |
| `last_move` | `last_used_move_slot` | A kit move, or Struggle. Implies a move action |
| `locked_to_move` | Choice lock (`Locked`) | Needs a Choice item, or ENCORE (the encored move) |
| `item` | `item_id` / `consumed_item` | `""` while the set has an item: consumed or removed, unless `item_confidence < 1` (an opponent's unknown item stays held). `item_consumed: true/false` overrides. A different item than the set's (Trick) is held |
| `ability` | `ability_override` | When it differs from the set's (or the Mega's): Skill Swap, Trace, ... |
| `is_mega` | via `species` | |
| `move_pp` | `pp` | `None` = full |
| `terastallized` | `terastallized`, `tera_used` | Engine extension. Field `tera_used_own/opp` / `mega_used_own/opp` set a side's Tera / Mega use outright (a Tera'd mon that fainted) |
| `field.turn` | `turn` | `turn - 1`: the decision is turn `field.turn` |
| `field.weather` (+ `_elapsed`) | `weather`, `weather_turns` | SunnyDay / RainDance / Sandstorm / Snow / Snowscape. Duration 5, or 8 when a board mon holds the matching rock and has the weather's ability or move |
| `field.terrain` (+ `_elapsed`) | `terrain`, `terrain_turns` | Electric / Grassy / Psychic / Misty. 5, or 8 with Terrain Extender on a mon with the surge ability or terrain move |
| `field.trick_room` (+ `_elapsed`) | `trick_room_turns` | Duration 5, so elapsed ≤ 5 |
| `field.tailwind_own/opp` (+ `_elapsed`) | `tailwind_turns` | Duration 4, so elapsed ≤ 4 |
| `field.screens_own/opp` `[light_screen, reflect, aurora_veil]` (+ `<screen>_<side>_elapsed`) | `*_turns` | 5, or 8 when a mon on that side holds Light Clay and knows the move |
| `<effect>_duration` | | Engine extension: overrides the full duration, e.g. a setter that lost its Light Clay since |
| `field.hazards_own/opp` | side conditions | `stealth_rock`, `spikes` (0-3), `toxic_spikes` (0-2), `sticky_web`, `safeguard` / `mist` (5 turns) |

**Not mapped** (no engine state, or nothing to rebuild):

- `history`, `last_move_own/opp` and `field.ended`. They belong in the
  prefix.
- `move_confidences` / `item_confidence` / `ability_confidence`. They become
  reveals through the prefix, or through `reveals`. The one exception:
  `item_confidence < 1` keeps an opponent's item held.
- `protect_streak`, `sleep_elapsed`, `toxic_n` and `volatile_ages` past the
  table above.
- `nature` / `evs` on the board mon. They are used only when `sets` gives
  none.

**Errors.** `position_from_board` raises `ValueError` when it cannot build
the board:

- an unknown species, move, item, ability or status;
- the wrong number of actives, or more than 6 mons;
- a board move not in the set;
- a Mega without its stone;
- two Megas or two Tera'd mons on one side;
- boosts, volatiles or a Substitute on a bench mon;
- an unsupported volatile;
- an elapsed count out of range;
- a side with no mon left.

A position needs only the board fields that matter to the scenario. Note
one difference from synthlog: a mon with no `turns_on_field` gets 1 here,
so Fake Out works. synthlog's neutral default gives such a mon 2 in the
log. Set `turns_on_field` explicitly when Fake Out matters.

### Rust

```rust
let raw = ps_position::prefix_raw_lines(&Prefix::PerSide([p1_lines, p2_lines]))?;
let mut b: Battle = serde_json::from_str(&snapshot)?;
b.attach_position_log(&ProtocolOptions::new(ProtocolMode::Ps), "gen9championsvgc2026regmc", raw, false)?;
let mut session = PsSession::from_battle(b);
let built = ps_board::position_from_board(&board, &sets, "gen9championsvgc2026regmc", SideRef::P1)?;
```

### Phase 4 gate

```sh
cargo build --release -p vgc-engine-conformance --features ps-rng --bin resume_gate
target/release/resume_gate --dirs <B>,<C> --count 3600 --threads 4 --out resume.jsonl
```

`resume_gate` takes each battle record's teams and seed (the phase-1
`ps-battle.js` records, Champions Reg M-C teams) and plays them as Champions
Reg M-B / M-C and gen 9 VGC 2024 G / 2025 I. Each battle runs through
`PsSession` on PS's PRNG with a seeded random policy. The policy's invalid
choices go in too, with a `default` fallback.

**Gate 1, snapshot / resume.** At a random turn stop (a move request, or a
faint-replacement request), the harness snapshots the battle with
`to_json`. It restores the snapshot with `attach_position_log`, with the
real stream so far as the prefix: half the battles get the two player
streams, half PS's raw log. It replays the remaining choice strings in the
same order. The restored battle must match the original byte for byte on:

- both requests at the stop;
- all four streams after the stop;
- the final state.

**Gate 2, builder round trip.** At move stops the harness maps the state
to a kcheck board. Elapsed turns and volatile ages are counted on the
stream, as a kcheck author or synthlog would count them, not taken from the
engine's counters. It rebuilds the board with `position_from_board` and
compares every mapped field.

Results (branch `ps-protocol-4`, 3600 battles, 900 per format):

| Gate | Result |
|---|---|
| Resume continuations byte-identical | **3600 / 3600** (move stops 2679, replacement stops 921; per-side prefix 1800, raw 1800) |
| Merged per-side prefix renders the real spectator / omniscient streams | 3600 / 3600 |
| Builder round trip, mapped fields identical | **2632 / 2632** positions |
| Positions a board cannot express (excluded) | 47: Speed Swap / Guard Split / Power Split / Power Trick (stats) 24, partial trap 10, Transform 5, Octolock 3, Dragon Cheer 3, Gravity's grounding 2 |

**Gate 3, redaction.** `test_position.py::test_observe_never_leaks_hidden_opponent_info`
restores 30 seeded battles with and without a prefix and plays on. In every
`observe(side)`, each opponent move, item, ability and Tera must have been
shown by the side's own stream, and opponent HP must be a percentage.

Two engine fixes came out of the gates: Ally Switch now swaps
`Side::ps_order` (PS swapPosition), and Light Clay extends only its
holder's screens (PS `durationCallback` reads the source's item).

**Cost.** `perf_bench --battles 5000`, logging off, 6 interleaved runs:
~444.4k (branch) vs ~446.1k (main `b0da64d`) steps/s, within noise, 0
allocations per step for both.

### Limits

- Positions mid-turn (a U-turn's switch request) can't be restored: a
  snapshot holds the turn's start, not the picks so far.
- A prefix is trusted. A stream that contradicts the state (another HP, a
  move the mon lacks) is shown as given, and `observe(side)` reads the
  stream.
- PS names of mons the prefix never shows default to the base species
  (PS's default when a set has no nickname).
- Without a prefix, a mid-game Illusion disguise isn't rebuilt (the header
  shows the real mon).
- The builder's item-extension guess sees only current items. Use
  `<effect>_duration` when the setter has lost its extender.
- Confusion's remaining turns, a Stockpile count, and the sources of Leech
  Seed, Curse and Attract are hidden on a board. They get the defaults
  above.

## Start from a real replay

`position_from_replay` reads a Showdown log up to the start of turn N (or
the faint-replacement stop before it) and returns a `Battle` there, through
`Battle.from_position`. What the log shows is copied. What it hides comes
from a usage prior. Every field says which it is.

### Python

```python
b = vgc_engine.position_from_replay(replay["log"], 6, ps_seed="1,2,3,4", viewer="p1")
rep = b.reconstruction_report()
rep["active_prior_fields"]        # {"p1a": ["moves[2]", "spread", ...], "p2a": [...], ...}
b.request("p1"); b.choose("p1", "move 1 1, move 2"); b.player_stream("p1")
```

```
vgc_engine.position_from_replay(log, turn=None, prior=None, ps_seed=None, viewer=None,
                                stop=None, sets=None, request=None, lookahead=False,
                                format=None, protocol="ps", player_names=None) -> Battle
Battle.reconstruction_report() -> dict | None
```

- **`log`**: one of these:
  - a replay's `log` text, or the replay JSON dict;
  - PS's raw `battle.log` (`|split|` pairs), or an omniscient log;
  - one player's stream, as text or a list of lines;
  - a mimikyu ps_bot decision dict (`decide_payload.live.protocol_log` and
    `request`).

  The kind is detected: `|split|` lines mean a raw log. Exact HP on one
  side, or a `|request|`, means that player's stream. Exact HP on both
  sides means an omniscient log. Otherwise it is a spectator log.
- **`turn`**: the decision turn. The position is turn N's move request, so
  `battle.turn` is N-1. A ps_bot decision file supplies its own turn.
- **`stop`**: `"move"` (the default), or `"replace"` for the faint
  replacement after turn N-1's residuals. A decision file with a
  `forceSwitch` request at the end of a turn is a `"replace"` stop.
- **`prior`**: `None` / `"default"` (chosen by format, below), `"none"`, an
  embedded prior id (below), or a stats file path: a chaos-derived JSON
  (Bring Four `smogon.py` shape) or a Smogon moveset `.txt`. `"a+b"` chains
  ids or paths: species `a` lacks come from `b`. Loaded priors are cached
  by spec.
- **`viewer`**: `"p1"` / `"p2"`. Only that player's view fills the
  opponent's hidden info. The viewer's own side uses the `|split|` secret
  lines, the request and the sets when given, else the prior. Without a
  viewer everything given is used for both sides.
- **`sets`**: `{"p1": export, "p2": export}`. Each export is a Showdown
  export or a list of set dicts. In Champions formats `evs` are stat
  points, as PS Champions exports carry them.
- **`request`**: a `|request|` dict (its `side.id` names the side), or
  `{"p1": ..., "p2": ...}`. A request gives that side exactly: which mons
  were brought, PS team order, HP, moves, item, ability and PP.
  Its stats are solved back to a nature and spread.
- **`lookahead=True`** also reads set facts revealed after the stop: moves,
  items, abilities and which mons were brought. Off by default: only the
  log up to the stop is read.
- **`ps_seed`** gives the play-out PS's PRNG, seeded as `from_teams` seeds
  it. Two positions with the same seed and the same choices play out the
  same.
- **Prefix.** The log up to the stop becomes the history (see
  [Prefix framing](#prefix-framing)), so each player's stream carries the
  real past.
  - A raw log is kept whole.
  - A replay loses chat, joins, renames and inactivity notes.
  - A player stream is merged with a copy whose own HP is shown as
    percentages, so the other player never sees that side's exact HP.
- **Errors** raise `ValueError`, with a bucket: `no such turn`,
  `no replacement stop`, `unsupported: ...`, `board`, `request stats`.

The CLI prints the same as JSON: the battle (`from_position`'s input), the
prefix and the report. `--board` adds the kcheck board and sets it was
built from. `--check` opens the session and adds both requests.

```sh
vgc recon --log replay.json --turn 6 [--stop replace] [--viewer p1] [--prior ID|FILE|none] \
          [--sets-p1 team.txt --sets-p2 team.txt] [--lookahead] [--board] [--check]
```

Rust: `vgc_engine_replay::position::{parse_input, reconstruct, ReconOptions}`.
`Reconstruction::session(opts)` opens the PS session.

### Provenance

`reconstruction_report()` has these keys:

- `sides.p1` / `sides.p2`: `team`, a list in team order (the pick order).
  - Each mon has `name`, `team_index` and `active_slot` (`"p1a"` or null).
  - Every field is `{"value", "src", "p"}`: `species`, `brought`, `hp`
    (with `max` and the `interval` the shown percentage allows), `status`,
    `boosts`, `item` (the starting item), `item_now`, `ability`, `moves`
    (one entry per slot), `pp`, `nature`, `spread` (`{"unit": "sp" | "ev",
    "values"}`), `gender` and `tera` (not in Champions).
  - When they apply: `sleep_turns_remaining`, `toxic_counter`,
    `confusion_turns_remaining`, `volatiles`, `last_move`, `turns_active`
    and `move_actions`.
  - `prior_fields` lists the mon's prior or default fields. `n_fields`
    counts the fields reported.
  - Per side: `brought` (exact or prior), `team_order` (exact or inferred)
    and `knowledge` (`full`, or `viewer` for the opponent of `viewer`).
- `active_prior_fields`: `{"p1a": [...], "p1b": [...], ...}`, the active
  mons' prior or default fields.
- `field`: every field effect with its turns up.
- `n_prior_fields` / `n_fields` for the whole position, plus `turn`,
  `stop`, `format`, `input`, `viewer` and `lookahead`.
- `prior`: the prior's resolved id (`"default"` never appears: it is
  replaced by the id it picked), and `prior_sha256`, the SHA-256 of its
  file (`+`-joined in id order for a chain; empty for `"none"`). Passing
  the reported `prior` back as `prior=` reproduces the position.

`src` is one of:

| `src` | Meaning |
|---|---|
| `exact` | Shown by the input: the log, a request, given sets, a team sheet (`|showteam|`) |
| `inferred` | Derived by a rule. An HP percentage becomes the interval's midpoint (lower median). Turn counters are counted on the stream. A bench order is first-seen order |
| `prior` | Filled from the usage prior. `p` is the prior's probability: the share among allowed candidates for items, abilities, spreads and species / Mega formes, a move's share of sets, a brought mon's estimated chance |
| `default` | Neither shown nor in the prior: a fixed default |

**HP.** Exact when a `|split|` secret line, an own-side stream or a request
shows it. Otherwise the shown percentage maps to an interval of HP values
for the mon's max HP. Champions shows `floor(100 * hp / max)`, at least 1,
with the 20 / 50 colour band. Gen 9 shows the ceiling, and 99 for a
scratched full bar. The midpoint is used. With a prior spread, the max HP
is the prior's too.

**Exact from the log.** Copied from the log:

- species and forme, Mega, Tera;
- HP, boosts, status;
- the toxic counter (residual ticks since the status or the switch-in);
- turns asleep (`|cant|...|slp`);
- volatiles, with their ages and PS's extra Taunt / Encore / Disable turn;
- weather, terrain, Trick Room, Gravity, Magic / Wonder Room, Tailwind,
  screens and hazards, with turns up;
- items: revealed, consumed (PS `lastItem` for this stint), knocked off,
  tricked;
- abilities: revealed, and changed by Trace, Skill Swap or Mummy;
- PP used per revealed move, Pressure included;
- turns active and move actions (Champions Fake Out reads them);
- the Protect streak, choice lock, last move and its target;
- a pending two-turn move (its target is hidden: the first live foe);
- Future Sight / Doom Desire, Rage Fist's hit count, Supreme Overlord;
- Speed Swap, Power / Guard Split and Power Trick;
- Transform / Imposter, Illusion when revealed;
- Shed Tail's doll, Octolock, Torment, Dragon Cheer.

### Prior policy

Embedded priors have stable ids. Their files stay in the repo
(`data/smogon-stats/README.md` has URL, month, cut, fetch date and hash):

| Id | Content |
|---|---|
| `smogon:2026-09/gen9championsvgc2026regmc-1500` | Smogon Reg M-C chaos stats, 2026-09, 1500 cut (mimikyu's usage convention). Spreads in stat points |
| `smogon:2026-08/gen9championsvgc2026regmb-0` | Smogon Reg M-B chaos stats, 2026-08, cut 0 |
| `bringfour:2026-09/regmc-reveals` | Bring Four's Reg M-C replay reveal shares (280k games). Reveal-biased, no spreads |

Each Smogon file keeps species at 0.1% usage or more, top 10 per category
(`scripts/trim_smogon_chaos.py`, Bring Four `smogon.py`'s trim).

`"default"` picks by the replay's format:

| Format | Resolves to |
|---|---|
| Reg M-B | `smogon:2026-08/gen9championsvgc2026regmb-0+bringfour:2026-09/regmc-reveals` |
| Reg M-C, and any other | `smogon:2026-09/gen9championsvgc2026regmc-1500+bringfour:2026-09/regmc-reveals` |

The Reg M-C default's fallback covers the 64 rare species under the 0.1%
trim. The Reg M-B one covers M-C additions (Rillaboom, Indeedee,
Salamence, ...). vgc-engine 0.4.0's default, for every format, was
`smogon:2026-08/gen9championsvgc2026regmb-0+bringfour:2026-09/regmc-reveals`.
Pass it as `prior=` to reproduce 0.4.0 results on Reg M-C replays.

The prior is conditioned on what was revealed:

- **Species key.** A Mega Evolved mon uses its Mega forme's stats. Smogon
  lists a stone holder under its Mega forme. Otherwise the most used of the
  species and its Mega formes is chosen; Megas are excluded once the side
  has Mega Evolved, or when the mon's item is known. A Mega key brings its
  stone as the item.
- **Brought mons.** Unseen ones are the most used unseen preview mons.
- **Item.** The most likely item, excluding:
  - items a teammate holds (Item Clause) and Mega stones;
  - Choice items once the mon used two moves in a stint, or a status move;
  - Assault Vest after a status move;
  - Sitrus Berry once its HP was shown at or below 50% without eating one;
  - the pinch berries below 25%;
  - Life Orb after a damaging hit with no recoil shown.
- **Ability.** The most likely legal ability. A mon that came in without
  announcing it has none of the always-announced abilities (Intimidate,
  Drizzle, the surges, Pressure, Unnerve, ...).
- **Moves.** The revealed moves, then the most used moves up to 4.
- **Spread.** The most used spread with a shown nature (team sheet), else
  the most used spread. With no stats, offense and Speed are maxed (the
  `CanonicalDefault` heuristic).
- **Hidden counters.** Sleep turns left default to `max(1, 3 - turns
  asleep)`. Confusion gets 2 turns. These are hidden even with full info.

Speed-order and damage-range constraints (`spread_recon`) are not applied
yet.

### Real replay gates

```sh
cargo build --release -p vgc-engine-conformance --features ps-rng --bin recon_gate
target/release/recon_gate --dirs <B>,<C> --count 1000 --threads 4 --out recon.jsonl
cargo build --release -p vgc-engine-replay --bin recon_corpus
ssh unraid 'cd .../gen9championsvgc2026regmc && find 2026-10-03 ... 2026-10-11 -name "*.json" -print0 | xargs -0 cat | gzip -1' \
  | gunzip | target/release/recon_corpus --threads 3 --out corpus.jsonl
```

`recon_gate` plays `resume_gate`'s forced-RNG battles: phase-1 records,
Champions Reg M-C / M-B, PS's PRNG and a seeded random policy. It stops at
a random move or replacement request. It rebuilds that stop from the
battle's final log alone:

- **Full info**: PS's raw log, both requests at the stop and both team
  sheets. Every state field is compared with the true engine state (gate 1).
  The true PRNG is then put in, which the log cannot show, and the logged
  choices are replayed. All four streams after the stop must match byte
  for byte (gate 2). A second run also puts in the true sleep and confusion
  counters.
- **Hidden sets**: the spectator log only, with the prior. Fields are
  compared per field.

`recon_corpus` rebuilds every turn of every usable replay, at the move
request and at the replacement stop when one exists, and opens the session
there. Usable means a natural end (no forfeit or timer loss) and both sides
showing 4 mons. It reads a directory, or concatenated replay JSON on stdin.

Results (branch `replay-position`):

| Gate | Result |
|---|---|
| 1. Full info: state fields matching the true engine state | **153,316 / 153,565 (99.84%)**, 1000 / 1000 positions rebuilt, 962 with every field |
| 1. Hidden counters (sleep / confusion turns left, prior) | 9 / 17 |
| 1. Rebuilt prefix == PS's log at the stop | 1000 / 1000 |
| 2. Byte-identical continuation (true PRNG put in) | **961 / 1000 (96.1%)**; with the true hidden counters too, 967 / 1000 (96.7%) |
| 3. Held-out Reg M-C, 2026-10-03 .. 10-11 (200,119 replays, 86,555 usable) | **951,980 / 952,256 positions (99.97%)**: move stops 686,226 / 686,405; 86,456 replays rebuilt at every stop |
| 3. Dev window (`mimikyu/data/replays/.../2026-09-*`, 279 usable) | 3,120 / 3,123 |

Reg M-C prior swap (0.4.1, branch `regmc-prior`): the 0.4.0 default
(`smogon:2026-08/...regmb-0+bringfour:...`) against the per-format default.
Gates 1 and 2 are unchanged (99.84%, 96.1%). The held-out sample is every
38th replay of 2026-10-03 .. 10-11 (5,266 replays, 2,261 usable).

| Measure | 0.4.0 default | 0.4.1 default |
|---|---|---|
| Hidden sets, fields matched (1000 battles) | 80.09% | **80.20%** |
| Hidden sets, Reg M-C half (500) | 80.1% | **80.4%**: item 59.4 -> 60.9, ability 77.9 -> 78.1, stats 18.0 -> 20.6 |
| Prior's top spread == true spread (nature + 6 values), Reg M-C half | 18.0% | **20.6%** |
| Prior's top spread == true spread, all 1000 | 17.41% | **18.74%** |
| Held-out sample, positions rebuilt | 24,804 / 24,810 (99.976%) | 24,804 / 24,810 (99.976%) |
| Held-out sample, prior-filled fields per position (mean of 88.1) | 49.68 | 49.62 |

The Reg M-B half uses the same prior as before, so it is unchanged.

- **Gate 1 misses.** The 249 misses come mostly from about a dozen
  positions where an Illusion holder was never revealed before the stop:
  every field of the disguise's mon is wrong. The rest are single fields:
  a failed-Protect streak, a Taunt / Disable / Encore turn, a Substitute's
  HP after hits, PP under Pressure.
- **Gate 2 failure buckets** (39): `-damage` 8, `-activate` 8, request at
  the stop 7, `move` 7, `-end` 4. These are the gate-1 misses above,
  played out.
- **Gate 3 failure buckets** (276): `illusion (ambiguous)` 139,
  `illusion (unrevealed)` 73, `board` 57 (an unrevealed Illusion behind a
  Mega forme), Syrup Bomb 4, Uproar 3.
- **Prior-filled fields per held-out position**: mean 49.6 of 88.1. p10 39,
  p50 50, p90 63, max 75. A spectator log never shows a nature or spread,
  so every active mon has at least `nature` and `spread` prior-filled.
- **Hidden sets** (spectator log + default prior on the conformance
  teams), fields matching the truth mon by mon (80.1% overall):
  - near-exact: species 100%, status 100%, boosts 100%, active 99.8%,
    turns active 99.9%;
  - prior-dependent: ability 77.6%, item 59.9%, HP 52.5%, nature 44.1%,
    stats 17.4%;
  - the whole 4-move list in slot order: 5.8%.

**Cost.** No engine (`vgc-engine-core`) code changed, so step throughput is
unchanged. A rebuild takes about 1 ms (3 threads: about 1,000 positions/s
per thread, session included).

### Replay limits

- **Hidden counters.** Sleep and confusion turns left, a Substitute's HP
  after hits, and a charging move's target are hidden. They get the
  defaults above.
- **Illusion before it breaks.** In a spectator log the disguise is all
  there is. Two mons with the same name active at once are resolved: the
  one that used a move its shown species cannot learn is the Illusion
  holder, else the newcomer. A request fixes the actives. A disguised stint
  that ends before the reveal stays attributed to the disguise.
- **Mid-turn switch requests** (U-turn, Eject Button) have no position
  form, as in phase 4.
- **Not carried.** A few turn-scoped results: `move_last_turn_result`
  (Stomping Tantrum's boost), the last attacker (Counter, Revenge) and the
  third type (`typeadd`, which the engine does not model either). Other
  unknown volatiles fail with `unsupported: volatile ...`.
- **Priors.** The embedded prior is Reg M-B usage, with Reg M-C reveals as
  the fallback; no Reg M-C chaos stats were local. Reveal shares
  understate moves. Pass `prior=` for a better file.
