# conformance-batch — stable-ID PS golden batches

Generates and runs batches of PS-conformance battles (the `champions` mod
golden captures the Rust engine is checked against), keyed by a **stable
content-derived ID** instead of a sequential index.

## Why IDs, not `out_00`…`out_49`

The old `/tmp/conf-batch` tooling named battles by loop index. Regenerating the
batch (new teams, more battles, reordered pairings) silently remapped every
index — so a punch-list note like "out_13 = Trevenant Curse" pointed at a
*different* battle after the next regen. Stable IDs fix this: a battle's ID is
`sha1(format, p1team, p2team, seed)[:10]`, so

- the same `(teams, seed)` always produces the same `out_<id>.json`;
- a reference to a battle ID stays valid forever;
- generating **more** battles only ADDS new IDs — existing ones never move.

## Usage

```sh
# 1. Generate jobs (stable IDs). Defaults reproduce the legacy 50-battle batch.
python3 tools/conformance-batch/mkjobs.py [num_pairings] [seeds_per_pairing]

# 2. Run them through the PS golden driver → out_<id>.json.
#    Skips any out_<id>.json that already exists (CONF_FORCE=1 to re-run).
node tools/conformance-batch/run-batch.js

# 3. Check a battle against the engine (filename carries the stable id):
cargo run -p vgc-engine-conformance -- /tmp/conf-batch/out_<id>.json
```

Scale up cheaply: `mkjobs.py 200 2` makes 400 battles; `run-batch.js` only runs
the ones without an output yet, so re-capturing or growing the corpus is
incremental.

## Env overrides

| var | default | meaning |
|-----|---------|---------|
| `CONF_TEAMS_FILE` | `~/Dev/mimikyu/data/generated_teams/regmb_random_100.txt` | team list (`=== Team N ===` separated) |
| `CONF_JOBS_DIR` | `/tmp/conf-batch/jobs` | job inputs |
| `CONF_OUT_DIR` | `/tmp/conf-batch` | `out_<id>.json` outputs |
| `CONF_FORCE` | unset | `1` re-runs jobs whose output already exists |

## Re-capturing a suspect golden

If the engine disagrees with PS but agrees with `@smogon/calc` (the independent
oracle), the PS capture may be stale/defective. Delete that `out_<id>.json` and
re-run `run-batch.js` to regenerate just that battle from the current PS clone.

The output JSON carries its own `id` field; `_meta.log` holds the PS battle log
(see `../ps-golden-driver/conformance-driver.js` for the capture shape).
