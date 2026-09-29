#!/bin/bash
# Minimal repros for engine mechanics bugs found by the accuracy study.
# Each job scripts turn-1 (and turn-2) choices; PS plays it, the engine
# replays it with PS's random outcomes keyed in. Every case should report a
# deterministic (rng_sensitive=false) divergence until the bug is fixed.
#
#   PS_DIST=/path/to/pokemon-showdown/dist/sim tools/accuracy/repros/run.sh
set -euo pipefail
cd "$(dirname "$0")/../../.."
OUT=${OUT:-$(mktemp -d)}
FORCE=1 node tools/accuracy/ps-battle.js tools/accuracy/repros/repros.jsonl "$OUT" --max-turns 2
target/release/accuracy keyed "$OUT" --jsonl /dev/stdout | python3 -c '
import json, sys
for line in sys.stdin:
    if line.startswith("{"):
        r = json.loads(line)
        print("%-34s divergence=%s rng_sensitive=%s" % (r["id"], r["divergence"], r["rng_sensitive"]))'
echo "battles: $OUT (inspect with tools/accuracy/show.sh $OUT <id>)"
