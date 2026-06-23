#!/usr/bin/env bash
# Comprehensive perf-matrix orchestrator: vgc-engine vs pokemon-showdown,
# gen9 DOUBLES, with parallel/batched throughput as a first-class metric.
#
# It (1) runs the Rust harness `perf_matrix` (single-thread scenario matrix +
# the multi-core scaling sweep, writing a JSON artifact), then (2) runs the PS
# side both single-process and N-process (PS only scales via process-per-core),
# and (3) prints an apples-to-apples head-to-head.
#
# The engine "random" scenario (fuzz-generated legal Champions teams, fresh per
# battle) is the analog of PS's `gen9randomdoublesbattle` (random team per
# battle), so the parallel sweep is run on "random" here for a fair comparison.
#
# Usage:
#   tools/perf/perf_matrix.sh [ST_BATTLES] [PAR_BATTLES] [PS_BATTLES] [PS_PROC_BATTLES]
#
#   ST_BATTLES       battles per single-thread engine scenario row (default 2000)
#   PAR_BATTLES      total battles for the engine parallel sweep    (default 60000)
#   PS_BATTLES       battles for the PS single-process run          (default 40)
#   PS_PROC_BATTLES  battles per PS worker in the N-process run     (default 30)
#
# Env:
#   PS_PATH  pokemon-showdown checkout (default /tmp/pokemon-showdown-research,
#            must be built: `cd $PS_PATH && node build`)
#
# One engine "step" == one PS "turn" (both sides choose, battle advances).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ST_BATTLES="${1:-2000}"
PAR_BATTLES="${2:-60000}"
PS_BATTLES="${3:-60}"
PS_PROC_BATTLES="${4:-60}"
PS_PATH="${PS_PATH:-/tmp/pokemon-showdown-research}"
SEED="1"
JSON_OUT="$REPO_ROOT/target/perf-matrix.json"

if command -v nproc >/dev/null 2>&1; then
  CORES="$(nproc)"
else
  CORES="$(sysctl -n hw.physicalcpu)"
fi

echo ">> Building vgc-engine perf_matrix (release)..." >&2
cargo build --release -p vgc-engine-golden --example perf_matrix >&2

echo ">> Running vgc-engine matrix (parallel scenario=random for PS parity)..." >&2
"$REPO_ROOT/target/release/examples/perf_matrix" \
  --st-battles "$ST_BATTLES" --par-battles "$PAR_BATTLES" \
  --par-scenario random --seed "$SEED" --json-out "$JSON_OUT"

PS_SINGLE_JSON=""
PS_MULTI_AGG=""
if [ -d "$PS_PATH/dist/sim" ]; then
  echo "" >&2
  echo ">> Running pokemon-showdown single-process ($PS_BATTLES battles)..." >&2
  PS_SINGLE_JSON="$(node "$REPO_ROOT/tools/perf/ps_bench.js" \
    --ps "$PS_PATH" --battles "$PS_BATTLES" --format gen9randomdoublesbattle \
    --seed "1,2,3,4" --warmup 3 2>/dev/null | grep PSBENCH_JSON | cut -d' ' -f2- || true)"

  echo ">> Running pokemon-showdown $CORES processes x $PS_PROC_BATTLES battles (process-per-core)..." >&2
  TMPDIR_PS="$(mktemp -d)"
  t_start=$(python3 -c 'import time; print(time.time())')
  pids=()
  for ((i=0; i<CORES; i++)); do
    # Distinct seed per worker so they don't run identical battles.
    s="$((i+1)),$((i+2)),$((i+3)),$((i+4))"
    node "$REPO_ROOT/tools/perf/ps_bench.js" \
      --ps "$PS_PATH" --battles "$PS_PROC_BATTLES" --format gen9randomdoublesbattle \
      --seed "$s" --warmup 1 >"$TMPDIR_PS/w$i.out" 2>/dev/null &
    pids+=($!)
  done
  for p in "${pids[@]}"; do wait "$p"; done
  t_end=$(python3 -c 'import time; print(time.time())')
  PS_MULTI_AGG="$(TMPDIR_PS="$TMPDIR_PS" T_START="$t_start" T_END="$t_end" CORES="$CORES" python3 - <<'PY'
import json, os, glob
tot_b = tot_t = 0
for f in glob.glob(os.path.join(os.environ["TMPDIR_PS"], "w*.out")):
    for line in open(f):
        if line.startswith("PSBENCH_JSON "):
            d = json.loads(line.split(" ", 1)[1])
            tot_b += d["battles"]; tot_t += d["total_turns"]
wall = float(os.environ["T_END"]) - float(os.environ["T_START"])
print(json.dumps({
    "procs": int(os.environ["CORES"]),
    "battles": tot_b, "total_turns": tot_t, "wall_s": wall,
    "battles_per_sec": tot_b / wall, "turns_per_sec": tot_t / wall,
}))
PY
)"
  rm -rf "$TMPDIR_PS"
else
  echo ">> pokemon-showdown not built at $PS_PATH/dist — skipping PS side." >&2
fi

echo "" >&2
JSON_OUT="$JSON_OUT" PS_SINGLE_JSON="$PS_SINGLE_JSON" PS_MULTI_AGG="$PS_MULTI_AGG" python3 - <<'PY'
import json, os
eng = json.load(open(os.environ["JSON_OUT"]))
ps1 = os.environ.get("PS_SINGLE_JSON", "").strip()
psN = os.environ.get("PS_MULTI_AGG", "").strip()
ps1 = json.loads(ps1) if ps1 else None
psN = json.loads(psN) if psN else None

# Engine "random" single-thread row is the analog of PS gen9randomdoublesbattle.
rnd = next((r for r in eng["single_thread"]
            if r["scenario"] == "random" and r["format"] == "doubles"), None)
peak = eng["peak"]
cores = eng["machine_cores"]

print("================ HEAD-TO-HEAD vs pokemon-showdown (gen9 doubles) ================")
print(f"machine cores: {cores}")
print()
print(f"{'metric':<34}{'vgc-engine':>18}{'pokemon-showdown':>20}{'speedup':>12}")
print("-" * 84)

if rnd and ps1:
    e_sps = rnd["steps_per_sec"]; p_tps = ps1["turns_per_sec"]
    print(f"{'single-thread steps(turns)/sec':<34}{e_sps:>18,.0f}{p_tps:>20,.0f}{e_sps/p_tps:>11,.0f}x")
    print(f"{'single-thread ns/step(turn)':<34}{rnd['ns_per_step_mean']:>18,.0f}"
          f"{ps1['ns_per_turn']:>20,.0f}{ps1['ns_per_turn']/rnd['ns_per_step_mean']:>11,.0f}x")
    print(f"{'single-process battles/sec':<34}{rnd['battles_per_sec']:>18,.1f}"
          f"{ps1['battles_per_sec']:>20,.1f}{rnd['battles_per_sec']/ps1['battles_per_sec']:>11,.0f}x")
elif rnd:
    print("(PS single-process not run — engine random row only)")
    print(f"{'single-thread steps(turns)/sec':<34}{rnd['steps_per_sec']:>18,.0f}{'-':>20}{'-':>12}")

print()
print(f"--- PARALLEL / BATCHED (the ML-rollout headline) — {cores} cores ---")
if peak and psN:
    e_b = peak["battles_per_sec"]; e_s = peak["steps_per_sec"]
    p_b = psN["battles_per_sec"]; p_s = psN["turns_per_sec"]
    print(f"{'aggregate battles/sec':<34}{e_b:>18,.0f}{p_b:>20,.1f}{e_b/p_b:>11,.0f}x")
    print(f"{'aggregate steps(turns)/sec':<34}{e_s:>18,.0f}{p_s:>20,.0f}{e_s/p_s:>11,.0f}x")
    print(f"  vgc-engine: {cores} threads (shared-nothing).  "
          f"PS: {psN['procs']} processes (process-per-core).")
elif peak:
    print(f"{'aggregate steps(turns)/sec':<34}{peak['steps_per_sec']:>18,.0f}{'-':>20}{'-':>12}")
    print("  (PS multi-process not run.)")

print()
print("Honest asymmetries: PS formats every event to protocol strings, validates")
print("teams, and runs under V8 GC; vgc-engine emits no log and is alloc-free in")
print("step(). The 'random' rows generate a fresh team per battle on both sides,")
print("matching gen9randomdoublesbattle. Past ~8 cores this box's efficiency cores")
print("(Apple Silicon P+E split) drag per-core scaling — see scaling_efficiency.")
print()
print(f"JSON artifact: {os.environ['JSON_OUT']}")
PY
