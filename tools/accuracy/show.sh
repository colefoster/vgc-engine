#!/bin/bash
# Triage view for one battle: engine-vs-PS state on the first divergent turn
# (keyed replay, after the repair pass) plus PS's protocol log for that turn.
#
#   tools/accuracy/show.sh <battles_dir> <battle_id> [turn]
#
# Needs a release build of the `accuracy` binary (see README.md).
set -euo pipefail
D=$1; ID=$2; T=${3:-}
BIN=$(dirname "$0")/../../target/release/accuracy
F=$D/out_$ID.json
if [ -z "$T" ]; then
  T=$("$BIN" keyed "$F" --jsonl /dev/stdout 2>/dev/null | python3 -c "import json,sys
for l in sys.stdin:
    l=l.strip()
    if l.startswith('{'):
        r=json.loads(l); print(r['divergence']['turn'] if r['divergence'] else 1); break")
fi
"$BIN" dump "$F" | awk -v t="$T" '/^== turn /{p=($3==t)} /^unmatched draws/{p=1} p'
python3 - "$F" "$T" <<'PY'
import json,sys
d=json.load(open(sys.argv[1])); t=int(sys.argv[2])
cur=0
print("PS log, turn", t)
for l in d['_meta']['log'].split('\n'):
    if l.startswith('|turn|'): cur=int(l[6:]); continue
    if cur==t and not l.startswith('|t:') and l!='|': print('   ',l)
PY
