# Pokémon Champions data deltas — the real porting target

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
