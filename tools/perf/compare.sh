#!/usr/bin/env bash
# Head-to-head perf comparison: vgc-engine vs pokemon-showdown.
#
# Runs N random gen9 doubles battles to completion on each engine under a fixed
# seed and prints a battles/sec, steps/sec, ns/step table plus speedup ratios.
#
# Usage:
#   tools/perf/compare.sh [BATTLES] [PS_BATTLES]
#
#   BATTLES     battles for the vgc-engine run (default 2000 — it's fast)
#   PS_BATTLES  battles for the pokemon-showdown run (default 200 — it's slow;
#               throughput is rate-based so the counts need not match)
#
# Env:
#   PS_PATH     pokemon-showdown checkout (default /tmp/pokemon-showdown-research,
#               must be built: `cd $PS_PATH && node build`)
#
# One "step" (vgc-engine) and one "turn" (pokemon-showdown) are the same unit:
# one full turn where both sides choose and the battle advances.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
BATTLES="${1:-2000}"
PS_BATTLES="${2:-200}"
PS_PATH="${PS_PATH:-/tmp/pokemon-showdown-research}"
SEED="1"
PS_SEED="1,2,3,4"

echo ">> Building vgc-engine benchmark (release)..." >&2
cargo build --release -p vgc-engine-golden --example perf_bench >&2

echo ">> Running vgc-engine ($BATTLES battles)..." >&2
ENGINE_JSON="$("$REPO_ROOT/target/release/examples/perf_bench" \
  --battles "$BATTLES" --seed "$SEED" --format doubles --json | grep VGCBENCH_JSON | cut -d' ' -f2-)"

PS_JSON=""
if [ -d "$PS_PATH/dist/sim" ]; then
  echo ">> Running pokemon-showdown ($PS_BATTLES battles)..." >&2
  PS_JSON="$(node "$REPO_ROOT/tools/perf/ps_bench.js" \
    --ps "$PS_PATH" --battles "$PS_BATTLES" --seed "$PS_SEED" --warmup 5 \
    2>/dev/null | grep PSBENCH_JSON | cut -d' ' -f2- || true)"
else
  echo ">> pokemon-showdown not built at $PS_PATH/dist — skipping PS side." >&2
  echo ">>   (cd $PS_PATH && node build) to enable it." >&2
fi

ENGINE_JSON="$ENGINE_JSON" PS_JSON="$PS_JSON" python3 - <<'PY'
import json, os
e = json.loads(os.environ["ENGINE_JSON"])
ps_raw = os.environ.get("PS_JSON", "").strip()
ps = json.loads(ps_raw) if ps_raw else None

def row(name, bps, sps, nsps, turns):
    print(f"{name:<22}{bps:>14,.1f}{sps:>16,.0f}{nsps:>16,.0f}{turns:>15.1f}")

print()
print(f"{'engine':<22}{'battles/sec':>14}{'steps/sec':>16}{'ns/step':>16}{'turns/battle':>15}")
print("-" * 83)
row("vgc-engine", e["battles_per_sec"], e["steps_per_sec"], e["ns_per_step"], e["avg_turns_per_battle"])
if ps:
    row("pokemon-showdown", ps["battles_per_sec"], ps["turns_per_sec"], ps["ns_per_turn"], ps["avg_turns_per_battle"])
    print("-" * 83)
    sp_steps = e["steps_per_sec"] / ps["turns_per_sec"]
    sp_ns = ps["ns_per_turn"] / e["ns_per_step"]
    print(f"\nvgc-engine speedup (per-step throughput): {sp_steps:,.0f}x")
    print(f"vgc-engine speedup (per-step latency):    {sp_ns:,.0f}x")
else:
    print("\n(pokemon-showdown side not run — engine numbers only)")
print(f"\nstep() heap allocations: {e['step_allocs_total']} total "
      f"({e['step_allocs_total']/e['total_steps']:.3f}/step)")
print()
PY
