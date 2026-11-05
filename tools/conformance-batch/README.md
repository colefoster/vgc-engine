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
#    Reuses an existing out_<id>.json only if its provenance matches (below).
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
| `CONF_TEAMS_FILE` | `$MIMIKYU_DIR/data/generated_teams/regmb_random_100.txt` (`MIMIKYU_DIR` defaults to a `mimikyu` checkout next to this repo) | team list (`=== Team N ===` separated) |
| `CONF_JOBS_DIR` | `/tmp/conf-batch/jobs` | job inputs |
| `CONF_OUT_DIR` | `/tmp/conf-batch` | `out_<id>.json` outputs |
| `CONF_RECAPTURE` | unset | `1` re-captures stale outputs (old file kept in `stale/`) |
| `CONF_FORCE` | unset | `1` re-runs every job (old files kept in `stale/`) |
| `PS_DIST` | `/tmp/pokemon-showdown-research/dist/sim` | PS build the driver loads |

## Provenance and stale captures

A battle ID does not cover the oracle, so `run-batch.js` stamps every capture
with `_meta.provenance`:

- `ps_revision`: git HEAD of the PS checkout behind `PS_DIST`
  (`pokemon-showdown@<version>` for an npm install; `-dirty` suffix if tracked
  files are modified);
- `driver_sha256`: sha256 of `ps-golden-driver/conformance-driver.js`;
- `inputs_sha256`: sha256 of the job's format, seed, max_turns and both teams.

An existing output is reused only when all three match the current run. A
legacy capture with no provenance, a changed PS revision, driver or inputs,
or unreadable/truncated JSON is listed under `STALE` with the reason; the batch
exits 1 and the file is left as it is. A dirty or unknown current PS revision
never reuses anything. Re-capture with `CONF_RECAPTURE=1`: each stale file
moves to `stale/out_<id>.<n>.json` (outside the `out_*.json` glob) before the
new capture is written.

Tests: `node --test tools/conformance-batch/provenance.test.js` (no PS needed).

## Re-capturing a suspect golden

If the engine disagrees with PS but agrees with `@smogon/calc` (the independent
oracle), the PS capture may be stale/defective. Re-run that battle with
`CONF_FORCE=1` and a jobs dir holding only its `job_<id>.json`; the old capture
is kept in `stale/`.

The output JSON carries its own `id` field; `_meta.log` holds the PS battle log
(see `../ps-golden-driver/conformance-driver.js` for the capture shape).
