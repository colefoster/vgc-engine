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
- ✅ Champions pokeRound/chainModify damage rounding (base-power chain + weather/STAB) — out_41 T1 (7b3438f)
- ✅ Knock Off ×1.5 at base-power stage, not final damage — out_44, out_30 (13a2b80)
- ✅ **Iron Head flinch 20% in Champions (was 30%)** — out_23 (4caade9). NOT a
  damage bug: the subagent's "Excadrill-Mega High Horsepower deals 0 damage" was
  a misdiagnosis — a roll of 20 flinched the foe's attacker at 30% but not at
  Champions' 20%, so the attacker never moved. The move/damage code was always
  correct. Lesson: trace before "fixing"; the harness shines at Champions data
  deltas (cf. Make It Rain -2).

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

## OPEN — the rounding tail (low value; off-by-1..3 HP, rarely flips a KO)
- **out_37** (Blizzard+Fake Out on Aurorus, +3) and **out_32** (Giga Drain, −1):
  residual rounding after the base-power chain + STAB/weather pokeRound landed.
  Likely the final `ModifyDamage` chain (Filter/screens/Friend Guard/Life Orb —
  none of Friend Guard/Life Orb/Expert Belt are implemented in damage.rs yet)
  not being accumulated into one pokeRound, OR small residuals amplified by
  ×0.75/÷2. Diminishing returns — this is the bit-exactness tail.
- **out_46** Scolipede-Mega takes too little from Dual Wingbeat (mega Def stat
  delta?) — needs a trace (could be another Champions data delta like Iron Head).
- **out_05** Galar Slowbro ~38 low (partner-missed spread Blizzard + Regenerator)
  — `is_spread` may require >1 target HIT vs PS keeping ×0.75 on a missed
  partner; needs trace. **out_19** (T4) and **out_23 T5** uninvestigated.

## 2026-06-24 — new LEGAL-team batch (regmb_random_100, regenerated)

Re-ran the 50-battle batch on the regenerated legal teams (proper mega-stone
notation, valid sets). 14 clean / 12 real / 19 cascades. Findings:

- ✅ **Mega base-stat audit CLEAN** — all 100 mega formes' base stats match PS's
  pokedex.ts exactly; the one localdex discrepancy (Starmie-Mega atk 140 vs 100)
  is already corrected by `MegaFix starmiemega atk: 100`. So out_46 is NOT a
  Scolipede-Mega stat bug (that hypothesis is dead).
- ✅ **Skill Swap blocked by Protect** — out_45 (7b02a6f).
- ✅ **foe-stat-lowering STATUS moves implemented** (a544cc1) — `foe_debuff_moves`
  table (21 moves) + full gating; out_46 Tickle now matches. Was: 🔴 GAP — — Growl,
  Leer, Tail Whip, Charm, Screech, Tickle, Fake Tears, Metal Sound, Scary Face,
  Feather Dance, Confide, Baby-Doll Eyes, Play Nice, Eerie Impulse, Captivate,
  Cotton Spore, String Shot, Sand Attack, Smokescreen, etc. — ALL fall through
  `resolve_status_move_inner`'s default arm to "no effect" (verified empirically:
  Growl and Tickle both leave the foe at 0 boosts). Found via out_46 (Emolga
  Tickle on Froslass — PS −1 atk/−1 def, engine 0). Needs a `foe_debuff_moves`
  table routed through `apply_boosts` (which already does Mist/Mirror Armor/Clear
  Body) PLUS call-site gating: Protect flag, Substitute block, accuracy, Magic
  Bounce (reflectable), and White Herb / Eject Pack / Defiant reactions. A
  meaty, careful PR — implement deliberately, not rushed.
- Untriaged reals: out_12/out_25 (T1 big HP gaps involving switches + Life
  Orb/Kangaskhan), out_16 (T1 HP), out_06 (T2 freeze secondary not applied),
  out_24/out_34 (T2 boosts), deeper-turn out_13/29/01/48.

## Remaining cascades (harness keying gaps, mostly not engine bugs)
13 battles still carry `unmatched > 0` at turns 2–5 (out_00, out_08, out_14,
out_18, out_20, out_21, out_22, out_27, out_34, out_38, out_39, out_42, out_48).
The big systematic one (spread-secondary target) is fixed; the tail is
per-battle (out_20 ability fairyaura/blaze, out_48 paralysis, deeper-turn draw
attribution). Lower value than the real bugs above.
