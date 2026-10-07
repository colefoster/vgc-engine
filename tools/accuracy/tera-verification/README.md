# Tera before moves: seeded integration probes

Inputs for `tools/accuracy/ps-battle.js` (one turn each, fixed sodium seeds)
that check the order-106 `terastallize` action (PS a5df8274
sim/battle-queue.ts:188, :230-235) under the seeded `ps-rng` replay, which the
`OraclePartial` goldens can't.

- `jobs.jsonl`: the two golden scenarios as jobs (teams and choices copied from
  `crates/vgc-engine-golden/goldens/tera-before-moves-{resist-and-ko,flinch}.input.json`).
  Neither has a Speed tie among the actives, so no Update shuffle is drawn.
- `jobs-ties.jsonl`: `tera-before-moves-flinch-speed-tie`, the flinch scenario
  with only p2's Blissey changed to 21 Spe IVs. Blissey (base 55) then has
  floor((110 + 21) / 2) + 5 = 70 Speed, equal to p1's Chansey (base 50, 31 IVs:
  floor(131 / 2) + 5 = 70), both neutral-natured and alive. runAction's
  `eachEvent('Update')` after the Tera action (sim/battle.ts:2860-2861) then
  speed-sorts a real tie, so the turn draws the new Update shuffle.
