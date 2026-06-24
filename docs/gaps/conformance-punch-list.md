# Conformance punch list (2026-06-24)

Real engine divergences surfaced by the Champions PS-conformance harness on a
50-battle Reg M-B doubles batch (`/tmp/conf-batch`, teams from
`~/Dev/mimikyu/data/generated_teams/regmb_random_100`). Each entry is
`unmatched_draws == 0` (RNG fully keyed → genuine mechanic bug, not a draw
desync). Verify any fix by re-running `cargo run -p vgc-engine-conformance --
/tmp/conf-batch/out_<NN>.json` to CLEAN.

## SHIPPED (2026-06-24)
- ✅ self-stat-drops only when the move connects (Protect/miss/immune) — out_03, out_36 (e62f6b9)
- ✅ Headlong Rush self-drops Def/SpD — out_09 (6d8d0f0)
- ✅ Shell Smash implemented (+White Herb/Eject Pack) — out_02 (4887a61)
- ✅ Minimize / Double Team raise evasion — out_26 (ff391ee)
- ✅ driver: per-target keying for spread secondaries (harness fidelity) — out_13 unmasked (a122565)

## OPEN — clean, contained (ship next)
- **Curse (Ghost-type) self-HP cost** — out_13. Trevenant Curse pays 50% max HP
  (PS 160→80) to curse the target; engine pays nothing. Curse is "not impl"
  (battle.rs:6874). Non-Ghost Curse = +atk/+def/-spe; Ghost Curse = -50% HP,
  target loses 25%/turn.
- **Spiky Shield family share the plain-Protect arm** (battle.rs:~7457) — out_24
  (Fake Out into Spiky Shield: PS chips attacker 1/8 on contact; engine no
  chip). Same arm: Baneful Bunker (poison), King's Shield/Obstruct (−Atk/−Def),
  Burning Bulwark (burn), Silk Trap (−Spe) — all missing their side effect.
- **Burn residual on a mon that switched in that turn** — out_35. Polteageist
  switched in, took Heat Wave (eng matches), burned, but end-of-turn burn DOT
  (1/16 = 8) not applied. Likely residual loop skipping the just-active slot.

## OPEN — delicate, verify carefully (NOT yet shipped)
- **Cluster A: damage-modifier rounding / stage** — out_41, out_44, out_30,
  out_32, out_37 (off-by-1..3). Hypothesis (subagent): engine truncates some
  modifiers (`bp*n/4096`, `dmg*3/2`) instead of PS chainModify→pokeRound, and
  applies **Knock Off ×1.5 to final damage (battle.rs:~5097) instead of base
  power**. CAUTION: memory says damage.rs is calc-oracle-exact — the base
  formula is fine; the bug is specific modifier STAGES/rounding the calc-oracle
  doesn't exercise. Knock-Off-wrong-stage (out_44, out_30) is the most concrete
  and testable piece; the rounding-unification is higher-risk.
- **Cluster D: custom Champions megas** — out_23 (Excadrill-Mega High Horsepower
  deals 0 — Piercing Drill / forme handling?), out_46 (Scolipede-Mega takes too
  little from Dual Wingbeat — mega Def stat delta?). Distinct, need engine trace.
- **Cluster E: spread ×0.75 when partner missed / Regenerator** — out_05.
  Galar Slowbro ~38 HP low across a partner-missed spread Blizzard + Regenerator
  switch-out. Candidate latent bug: `is_spread` (damage.rs:125-129) may require
  >1 target actually HIT, but PS keeps ×0.75 even if the other target
  missed/fainted/Protected. Needs trace.
- **out_19** (T4 p2a hp) — deep multi-turn (Knock Off / switches), uninvestigated.

## Remaining cascades (harness keying gaps, mostly not engine bugs)
13 battles still carry `unmatched > 0` at turns 2–5 (out_00, out_08, out_14,
out_18, out_20, out_21, out_22, out_27, out_34, out_38, out_39, out_42, out_48).
The big systematic one (spread-secondary target) is fixed; the tail is
per-battle (out_20 ability fairyaura/blaze, out_48 paralysis, deeper-turn draw
attribution). Lower value than the real bugs above.
