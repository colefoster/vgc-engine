#!/usr/bin/env bash
# Audit: every direct write to a field tracked by `ResidualIndex` must be
# followed by a sync helper call (or use the canonical setter that already
# syncs). Run from repo root:
#
#   bash tools/audit-residual-index/audit.sh
#
# Exits 0 if clean, 1 if any direct write isn't matched by a sync within
# ~6 lines. Designed to be cheap and grep-based — not a parser.
#
# When you add a new ResidualIndex family (item_chip, leech_seed, ...),
# add its field name + sync helper to the FIELDS table below.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# field-name => sync-helper-name => RHS regex of "values we care about"
# Only writes whose RHS matches the value-regex are flagged. This keeps
# the audit focused on writes that could create ResidualIndex drift —
# non-tracked statuses (Freeze, Sleep, Paralysis) and incidental clears
# don't need the sync, and the debug-rescan assertion in
# resolve_end_of_turn catches any case the static audit misses.
declare -a FIELDS=(
  'status'      'sync_status_dot_bit'             'Status::(Burn|Poison|Toxic)'
  # PR-LC1: cached effective-weather / effective-terrain on Battle.
  # Any direct write to `.weather` / `.terrain` must be followed by a
  # `sync_weather_terrain_cache` call within ~6 lines (or use the
  # public `set_weather` / `set_terrain` helpers, which sync
  # automatically). The runtime debug-rescan assertion at the top of
  # `resolve_end_of_turn` catches drift the static audit misses.
  'weather'     'sync_weather_terrain_cache'      '(crate::weather::)?Weather::(None|Sun|Rain|Sand|Snow)'
  'terrain'     'sync_weather_terrain_cache'      '(crate::terrain::)?Terrain::(None|Electric|Grassy|Psychic|Misty)'
  # PR-EOT4+ will add: 'item' 'sync_item_chip_bit' 'Item::(Leftovers|BlackSludge|StickyBarb)', etc.
)

ROOTS=(crates/vgc-engine-core/src)

# Files where direct `.status` writes don't touch a Battle's active slots
# (calc-only fixtures, standalone Pokemon). Maintained by hand; add new
# entries with a comment explaining why.
SKIP_FILES_REGEX='/damage\.rs:'

# `team[N]` where N is a non-zero numeric literal references a benched
# Pokemon. Bench writes never affect ResidualIndex (the index only tracks
# active slots; switch-in recomputes from scratch via sync_status_dot_bit
# in do_switch). Skip these.
# Skip both literal bench indices (team[1..5]) and loop-variable indices
# (team[i], team[idx], etc.) — the latter are conventionally bench-walk
# loops in test scaffolding. If a real bug ever hides behind team[var],
# the debug-rescan assertion in resolve_end_of_turn catches it at runtime.
BENCH_SLOT_REGEX='team\[([1-5]|[a-z_][a-z0-9_]*)\]'

fail=0

check_one_field() {
  local field=$1 sync=$2 value_re=$3
  # Match `.field = <value_re>...` writes. Skip:
  #  - lines that are themselves a sync call
  #  - lines tagged with // AUDIT-OK
  #  - files in SKIP_FILES_REGEX
  #  - bench-slot indices (BENCH_SLOT_REGEX)
  local pattern="\\.${field}[[:space:]]*=[[:space:]]*${value_re}"
  while IFS=: read -r file line content; do
    # tolerate optional whitespace; "= =" is == comparison, already excluded by [^=]
    if [[ "$content" =~ //[[:space:]]*AUDIT-OK ]]; then continue; fi
    if [[ "$content" =~ ${sync} ]]; then continue; fi
    # Look for the sync helper in the next 6 lines of the same file.
    if awk -v L="$line" -v sym="$sync" '
        NR>=L && NR<=L+6 && index($0, sym) { found=1; exit }
        END { exit found ? 0 : 1 }
      ' "$file"; then
      continue
    fi
    echo "::error::$file:$line: direct '.$field' write without nearby $sync call"
    echo "    $content"
    fail=1
  done < <(grep -RnE "$pattern" "${ROOTS[@]}" \
            --include='*.rs' \
            | grep -vE '\.status_dot' \
            | grep -vE 'fn sync_' \
            | grep -vE "$SKIP_FILES_REGEX" \
            | grep -vE "$BENCH_SLOT_REGEX" )
}

i=0
while [[ $i -lt ${#FIELDS[@]} ]]; do
  check_one_field "${FIELDS[$i]}" "${FIELDS[$((i+1))]}" "${FIELDS[$((i+2))]}"
  i=$((i+3))
done

if [[ $fail -eq 0 ]]; then
  echo "audit-residual-index: clean"
  exit 0
else
  echo
  echo "audit-residual-index: FAIL"
  echo "Each flagged line must either (a) use the setter that already syncs,"
  echo "or (b) call the listed sync helper within 6 lines, or (c) be marked"
  echo "// AUDIT-OK with a one-line justification."
  exit 1
fi
