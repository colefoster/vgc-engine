#!/usr/bin/env bash
# Generate `.rng.json` sidecars for every replay JSON in a directory by
# driving each one through PS via `dump.js`. Skips replays the action
# extractor can't drive cleanly (the dumper returns `{"ok":false,...}`
# or any in-log `|error|`).
#
# Usage:
#   ./generate-sidecars.sh <corpus-dir> <out-dir>
#
# Output:
#   For each <corpus-dir>/.../<replay-id>.json that drives cleanly,
#   writes <out-dir>/<replay-id>.rng.json. score-corpus then reads
#   them via `--rng-dump-dir <out-dir>`.

set -uo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: $0 <corpus-dir> <out-dir>" >&2
    exit 2
fi

CORPUS="$1"
OUTDIR="$2"
mkdir -p "$OUTDIR"

DUMP_JS="$(cd "$(dirname "$0")" && pwd)/dump.js"

ok_count=0
fail_count=0
total=0

while IFS= read -r -d '' replay; do
    total=$((total+1))
    id=$(basename "$replay" .json)
    out="$OUTDIR/$id.rng.json"
    if [[ -f "$out" ]]; then
        # Already dumped — count as ok.
        ok_count=$((ok_count+1))
        continue
    fi
    # Drive the replay through PS; consider the dump valid only when the
    # PS log contains no |error| lines AND ok==true.
    result=$(jq -nc --slurpfile r "$replay" '{replay: $r[0], seed:[1,2,3,4]}' \
        | node "$DUMP_JS" 2>/dev/null)
    is_ok=$(echo "$result" | jq -r '.ok // false')
    has_error=$(echo "$result" | jq -r '.log // "" | test("\\|error\\|")')
    if [[ "$is_ok" == "true" && "$has_error" == "false" ]]; then
        # Strip the giant `log` field to keep sidecars small.
        echo "$result" | jq -c 'del(.log)' > "$out"
        ok_count=$((ok_count+1))
    else
        fail_count=$((fail_count+1))
    fi
done < <(find "$CORPUS" -name '*.json' -print0)

echo "ok:   $ok_count / $total"
echo "fail: $fail_count / $total"
