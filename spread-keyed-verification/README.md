# Keyed golden inputs (staged; not in `goldens/` until captured)

These inputs are staged here because the corpus loader treats any
`goldens/*.input.json` without its `.ps.json` as a failure. The manager
captures each one with the pinned PS (`a5df8274`) and installs the input and
the capture into `crates/vgc-engine-golden/goldens/` together.

| input | covers |
|---|---|
| `spread-keyed-base-florges.input.json` | A's two-turn Florges script, seed `[1,2,3,4]`, with plain `Florges`: p2 Earthquake (allAdjacent, ally first), Disarming Voice (sure-hit allAdjacentFoes), Draining Kiss, Psychic secondary |
| `spread-keyed-base-furfrou-closecombat.input.json` | the existing `cosmetic-furfrou-base-fur-coat-v2` input with `rng_mode: "keyed"`: Close Combat's real selfDrops roll |
| `spread-keyed-p1-protect-immune.input.json` | p1 Earthquake into a protected foe, an immune Flying foe and the ally; Blizzard (70%, allAdjacentFoes) |

## Blizzard-miss seed selection (declared before any engine scoring)

`p1-protect-immune-candidates.jsonl` holds the same input with four fixed
seeds, in this order: `[5,6,7,8]`, `[9,10,11,12]`, `[13,14,15,16]`,
`[17,18,19,20]`. Capture all four with PS only. Select the **first**
candidate, in that order, whose PS capture meets all of these:

1. `ok: true`, no `errors`, both turns played, no faint;
2. turn 1's keyed Earthquake accuracy draws name only `p1b` (p2a protected,
   p2b immune);
3. at least one keyed Blizzard accuracy draw is `false`.

The selected candidate is installed as `spread-keyed-p1-protect-immune` with
its seed. Selection never looks at engine results. If no candidate meets
the rule, report that rather than adding seeds.

## Capture check

For each keyed capture, also capture the same input without `rng_mode`.
`rng`, `events` and `log` must be identical between the two runs, apart from
the `|t:|` timestamps in `log`.
