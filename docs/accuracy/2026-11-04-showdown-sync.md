# Showdown sync, 2026-11-04

The engine's reference moved from Showdown `a5df8274` (2026-09-22) to `16db150a` (2026-11-04). This also fixes gaps that real game replays and Showdown's own test suite exposed.

## Results

Games from the mimikyu arena replayed from their input logs on `vgc sim` and Showdown, comparing logs, each side's requests and the end record. The games use only 6 teams.

| Batch | Games | Byte-identical before | Byte-identical after | State differs before | State differs after |
|---|---|---|---|---|---|
| Models and baselines | 6,000 | 5,923 (98.72%) | 5,984 (99.73%) | 57 | 6 |
| Random vs random | 20,000 | 19,798 (98.99%) | 19,897 (99.49%) | 138 | 68 |

The after games change winner in 3 of 6,000 model games (0.05%) and 22 of 20,000 random games (0.11%). "Before" compares the engine at `c0d81bd` with `a5df8274`, its reference at the time. Against `16db150a`, about 6% of a sample of those games differed in state.

## What changed

| Change | Showdown source |
|---|---|
| `Update` draw before a switch request | `437926906`, `sim/battle.ts` runAction, `sim/battle-actions.ts` runPursuitActivation |
| `AfterMega` handler speed sort draws | `sim/battle-actions.ts` runMegaEvo |
| Static, Flame Body and Poison Point roll even if the attacker fainted | `data/abilities.ts` onDamagingHit |
| Format table and rule text regenerated | `tools/protocol/ps-formats.js` |
| Struggle recoils when a hit does 0 damage | `4148c1187` |
| Sheer Force skips AfterMoveSecondary items outside Champions | `fbafa7251`, `sim/battle-actions.ts` |
| Pickpocket implemented | `data/abilities.ts` pickpocket |
| Mega Sol: Electro Shot charges under rain | `89905975e`, `sim/pokemon.ts` effectiveWeather |
| Future Sight respects Endure and shows effectiveness | `0ac140354`, `data/conditions.ts` futuremove |
| Curse's start and HP cost order outside Champions | `data/moves.ts` curse |
| No Retreat on an already trapped mon | `d12b9c1e9` |
| Supreme Overlord prints no end line without fallen allies | `d39cea805` |
| Prankster doesn't block moves aimed at allies | `sim/battle-actions.ts` hitStepTryImmunity |
| Synchronize shows `-immune` or `-fail` when its reflected status fails | `sim/pokemon.ts` setStatus, `data/abilities.ts` synchronize |
| Requests: no maybeTrapped hint in custom games; Return and Frustration list power | `sim/battle.ts` getRequests, `sim/side.ts` |

## Same-seed results change

The draw changes alter what happens on a given seed. Anything comparing seeded results across this change needs a new baseline, including kcheck-mt. The odds of outcomes shouldn't move from the draw changes alone: with `Update` draws neutralized, `a5df8274` and `16db150a` were byte-identical on 26,000 games.

## Regression gate

`crates/vgc-engine-conformance/tests/ps_suite.rs` replays `tools/protocol/ps-suite/scenarios.jsonl`: scenarios from Showdown's own test suite whose moves, abilities and items exist in Reg M-C, plus targeted additions. It needs `--features ps-rng`, and CI runs it that way. Regenerate with `tools/protocol/ps-suite.js`.

## Known gaps

- `tools/protocol/ps-suite/known-gaps.jsonl` lists 369 suite scenarios that don't match yet: 311 singles and 58 doubles.
- Remaining arena differences include cosmetic line order (same-species Intimidate, Tailwind ending, spread move display targets) and a smaller group of state differences. `-immune` after Synchronize is fixed.
- 47 random-game state differences are a group the triage labels "other" and haven't been root-caused.
- Not implemented because Reg M-C can't reach it: Poison Puppeteer. The `Return 102` move display name in requests is also unmatched.
- A move requested as Return or Frustration assumes 255 happiness, because the engine doesn't model happiness.
