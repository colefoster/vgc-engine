#!/usr/bin/env bash
# Random-play golden batch generator.
#
# For each seed in 0..N-1 (default N=50):
#   1. Generate two teams via team-gen.js (seeds 2*N, 2*N+1)
#   2. Build an input job JSON with `random_play: true`, `max_turns: 30`
#   3. Run driver.js to produce the `.ps.json` ground truth
#   4. Drop the pair under crates/vgc-engine-golden/goldens/random/
#
# Skips seeds where PS rejects the team or the driver reports `ok: false`.
# Prints a summary at the end.
#
# Usage:
#   tools/golden-gen/generate-batch.sh [N]
#     N: number of seeds (default 50)
#
# Outputs:
#   crates/vgc-engine-golden/goldens/random/seed-<N>.input.json
#   crates/vgc-engine-golden/goldens/random/seed-<N>.ps.json

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
N="${1:-50}"
OUT_DIR="${REPO_ROOT}/crates/vgc-engine-golden/goldens/random"
mkdir -p "$OUT_DIR"

GEN="${REPO_ROOT}/tools/golden-gen/team-gen.js"
DRIVER="${REPO_ROOT}/tools/ps-golden-driver/driver.js"

attempted=0
succeeded=0
rejected=0
ps_failed=0

START=$(date +%s)

# Step 1: build per-seed team pairs + input JSON files (cheap; team-gen is fast).
declare -a SEEDS_OK
for seed in $(seq 0 $((N - 1))); do
  attempted=$((attempted + 1))
  t1_seed=$((seed * 2 + 1000))
  t2_seed=$((seed * 2 + 1001))

  team1="$(node "$GEN" "$t1_seed" 2>/dev/null)"
  if [[ -z "$team1" ]]; then
    echo "seed=$seed: team-gen p1 failed" >&2
    rejected=$((rejected + 1))
    continue
  fi
  team2="$(node "$GEN" "$t2_seed" 2>/dev/null)"
  if [[ -z "$team2" ]]; then
    echo "seed=$seed: team-gen p2 failed" >&2
    rejected=$((rejected + 1))
    continue
  fi

  # Build job JSON. Use node to safely JSON-encode the team strings.
  job_json="$(node -e '
    const team1 = require("fs").readFileSync(process.argv[1], "utf8");
    const team2 = require("fs").readFileSync(process.argv[2], "utf8");
    const seed = parseInt(process.argv[3], 10);
    process.stdout.write(JSON.stringify({
      name: `random-${seed}`,
      seed: [seed, 0, 0, 0],
      format: "gen9customgame",
      random_play: true,
      max_turns: 30,
      p1: { team: team1 },
      p2: { team: team2 },
    }, null, 2));
  ' <(echo "$team1") <(echo "$team2") "$seed")"

  input_path="${OUT_DIR}/seed-${seed}.input.json"
  echo "$job_json" > "$input_path"
  SEEDS_OK+=("$seed")
done

# Step 2: run the driver. For N>10, boot PS once via --batch and stream all
# jobs through one Node process (saves ~60s/seed dex load). For small N
# the per-job overhead is fine and keeps the codepath dead-simple.
if [[ ${#SEEDS_OK[@]} -gt 10 ]]; then
  # Build NDJSON: one compact-JSON job per line on stdin to the driver.
  BATCH_IN="${OUT_DIR}/.batch.in.ndjson"
  BATCH_OUT="${OUT_DIR}/.batch.out.ndjson"
  : > "$BATCH_IN"
  for seed in "${SEEDS_OK[@]}"; do
    input_path="${OUT_DIR}/seed-${seed}.input.json"
    # Compact (one-line) JSON so each job is a single NDJSON record.
    node -e '
      const j = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
      process.stdout.write(JSON.stringify(j) + "\n");
    ' "$input_path" >> "$BATCH_IN"
  done

  if ! node "$DRIVER" --batch < "$BATCH_IN" > "$BATCH_OUT" 2>/dev/null; then
    echo "batch driver crashed" >&2
    rm -f "$BATCH_IN" "$BATCH_OUT"
    for seed in "${SEEDS_OK[@]}"; do
      rm -f "${OUT_DIR}/seed-${seed}.input.json"
      ps_failed=$((ps_failed + 1))
    done
  else
    # Split NDJSON output back into per-seed .ps.json files. Order matches
    # SEEDS_OK because the driver processes jobs sequentially.
    i=0
    while IFS= read -r line; do
      seed="${SEEDS_OK[$i]}"
      ps_path="${OUT_DIR}/seed-${seed}.ps.json"
      input_path="${OUT_DIR}/seed-${seed}.input.json"
      echo "$line" > "$ps_path"
      if ! node -e '
        const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
        process.exit(r.ok ? 0 : 1);
      ' "$ps_path" 2>/dev/null; then
        echo "seed=$seed: PS reported errors" >&2
        rm -f "$input_path" "$ps_path"
        ps_failed=$((ps_failed + 1))
      else
        succeeded=$((succeeded + 1))
        printf "."
      fi
      i=$((i + 1))
    done < "$BATCH_OUT"
    rm -f "$BATCH_IN" "$BATCH_OUT"
  fi
else
  for seed in "${SEEDS_OK[@]}"; do
    input_path="${OUT_DIR}/seed-${seed}.input.json"
    ps_path="${OUT_DIR}/seed-${seed}.ps.json"
    if ! node "$DRIVER" "$input_path" > "$ps_path" 2>/dev/null; then
      echo "seed=$seed: driver crashed" >&2
      rm -f "$input_path" "$ps_path"
      ps_failed=$((ps_failed + 1))
      continue
    fi
    if ! node -e '
      const r = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
      process.exit(r.ok ? 0 : 1);
    ' "$ps_path" 2>/dev/null; then
      echo "seed=$seed: PS reported errors" >&2
      rm -f "$input_path" "$ps_path"
      ps_failed=$((ps_failed + 1))
      continue
    fi
    succeeded=$((succeeded + 1))
    printf "."
  done
fi
echo

END=$(date +%s)
echo "--- batch summary ---"
echo "attempted:  $attempted"
echo "succeeded:  $succeeded (saved to $OUT_DIR)"
echo "ps-failed:  $ps_failed (PS rejected team or driver crashed)"
echo "rejected:   $rejected (team-gen produced empty output)"
echo "elapsed:    $((END - START))s"
