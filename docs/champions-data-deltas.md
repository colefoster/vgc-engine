# Pokémon Champions data deltas — the real porting target

> **STATUS (2026-06-24): move audit COMPLETE.** All 22 move-data deltas below
> (16 base-power + 5 accuracy + Growth→Grass) are ported via a new
> `champions_move_override` table in `vgc-engine-data/build.rs`; Moonblast
> (30%→10% SpA) and Dire Claw (50%→30%) chances fixed in battle.rs; Iron Head
> (20%) and Make It Rain (−2 SpA) were already done. **`toxicthread` is
> UNIMPLEMENTED** in the engine (a missing move, not a delta — separate gap;
> Champions value is Spe −2 + poison).
>
> **Abilities audit COMPLETE.** Of the 13 overridden abilities, only 2 are real
> behavior deltas: **Healer 30%→50%** (fixed) and **Unseen Fist** — Champions
> reworks it to the Piercing-Drill form (contact moves pierce Protect for 1/4
> damage, vs gen 9's full-damage bypass); it was unimplemented, now wired into
> the Piercing Drill path (fixed). The other 11 are `isNonstandard:null`
> legalizations (6 custom megas) or standard-gen-9 re-declarations
> (angershell/berserk/disguise/naturalcure/**regenerator** — Regenerator IS
> implemented and correct, so out_05 is a different bug). Custom mega
> base-stats (formats-data.ts) not yet audited.


**Conclusion (verified from the canonical PS `champions` mod, 2026-06-24):**
Champions' damage *algorithm* is **mainline gen-9, byte-identical** — `getDamage`
(base formula + base-power `chainModify` event) is inherited unchanged, and the
mod's `modifyDamage`/`spreadMoveHit` overrides match base PS except cosmetic
log text. Our engine already matches this after the pokeRound/chainModify work.
**The stat formula is also correct:** Champions uses `final = base + sp + 20`
(HP `+75`); the engine's `EV = 8·sp − 4` → mainline `floor(ev/4)` bridge is
algebraically identical at level 50 (the only level Reg M-B uses). So **all
Champions-specific differences are DATA** — port the mod, don't touch the math.

Ground truth: `/tmp/pokemon-showdown-research/data/mods/champions/{moves,abilities,items}.ts`
(GitHub: `smogon/pokemon-showdown/tree/master/data/mods/champions`). Reg M-B =
`mod: 'champions'`, ruleset `Flat Rules + VGC Timer + Open Team Sheets` (no
`levelclausemod` → flat stat form; equal to mainline at L50).

## Combat-rebalanced moves (26) — the checklist

Of the mod's 259 move overrides, 212 are just `isNonstandard:"Past"` bans. These
26 actually change combat. `✓` = engine already matches (harness-confirmed).

| move | Champions value | mainline | engine |
|------|-----------------|----------|--------|
| ironhead | flinch **20%** | 30% | ✓ (4caade9) |
| makeitrain | SpA **−2**, **acc 95** | −1, acc 100 | −2 ✓; **acc 95 unverified** |
| anchorshot | BP **90** | 80 | ? |
| appleacid | BP **90** | 80 | ? |
| beakblast | BP **120** | 100 | ? |
| boltbeak | BP **80** | 85 | ? |
| bonerush | BP **30**/hit | 25 | ? |
| clangoroussoul | **acc true** (can't miss) | 100 | ? |
| crabhammer | **acc 95** | 90 | ? |
| direclaw | 30% slp/psn/par | 50% | ? |
| firelash | BP **90** | 80 | ? |
| firstimpression | BP **100** | 90 | ? |
| fishiousrend | BP **80** | 85 | ? |
| geargrind | BP **60**, acc **90** | 50, 85 | ? |
| gravapple | BP **90** | 80 | ? |
| growth | **type Grass** | Normal | ? |
| infernalparade | BP **65** | 60 | ? |
| moonblast | SpA-drop **10%** | 30% | ? |
| mountaingale | BP **120** | 100 | ? |
| nightdaze | BP **90** | 85 | ? |
| psyshieldbash | BP **90** | 70 | ? |
| spiritshackle | BP **90** | 80 | ? |
| syrupbomb | acc **90** | 85 | ? |
| toxicthread | Spe **−2** + psn | −1 + psn | ? |
| tropkick | BP **85** | 70 | ? |

(Verify each `?` against the engine's move data; mainline column is approximate —
the mod file is the truth. Each mismatch is a real bug like Iron Head.)

## Overridden abilities (13)

Champions mega abilities (likely partial in engine): `piercingdrill`, `firemane`,
`dragonize`, `megasol`, `eelevate`, `spicyspray`. Standard abilities re-declared:
`angershell`, `berserk`, `disguise`, `healer`, `naturalcure`, `regenerator`,
`unseenfist`. Note: **Regenerator** is in the override list and is NOT yet
implemented in the engine (cf. conformance out_05). Custom mega base-stats live
in `formats-data.ts` / the mod pokedex.

## Oracles
- **Primary:** the PS `champions`/`championsregma` sim — our conformance harness
  can drive the real `[Gen 9 Champions] VGC 2026 Reg M-B` / Doubles Custom Game
  format directly (already does). Same formula + same mod data = ideal oracle.
- **Calc:** `calc.pokemonshowdown.com` has a Champions mode (`@smogon/calc`,
  `github.com/smogon/damage-calc`) — same data, second cross-check.
- **Open-source tiebreaker:** PokeDD (MIT, `github.com/Seancheey/PokeDD`).
- Data backup (CC-BY, Serebii-verified): `github.com/otterlyclueless/pokemon-champions-data`.
