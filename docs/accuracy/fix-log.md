# Mechanics fix log (2026-09, branch `mechanics-fixes`)

Follow-up to [`2026-09-accuracy-proof.md`](2026-09-accuracy-proof.md): the
ten most frequent first-divergence bugs from its §9 list, fixed test first.
Ground truth is Pokémon Showdown `a5df8274` with its Champions mod.

**Result:** on the same 1,298-battle forced-RNG sample, fully clean
battles went from **16.5% to 48.3%**, and per-turn agreement went from
**78.7% to 91.5%**.

## Method

- **Sample:** the study's 1,298 battles, replayed with
  `accuracy keyed` + `analyze.py keyed`. The baseline rerun reproduced the
  study: 214 clean (16.5%) and 4,006/5,090 turns (78.7%).
- **Ranking:** bugs were ranked by the first divergences they caused,
  using the study's offender table and triage. Each fix's actual effect is
  the "battles moved" column below: battles whose first divergence
  disappeared or moved later.
- **Tests:** each fix has a failing test written first, in one of two
  forms:
  - a unit test in `battle.rs`;
  - a PS-ground-truth golden in `crates/vgc-engine-conformance/tests/accuracy_repros.rs`.
    Each golden is a committed PS battle in `tools/accuracy/repros/battles/`:
    either a scripted repro from `repros.jsonl`, or a real study battle
    (`study-<id>-…`). The engine must replay it with no divergence.
- **Regression guard:** after every fix,
  `cargo test --workspace --exclude vgc-engine-py` passed both with
  `ps-rng` off and with `ps-rng` on (core, conformance and golden
  features). A per-battle comparison with the previous run confirmed that
  no battle's first divergence moved earlier.

## Fixes

| # | bug | PS reference | tests added | commit | battles moved |
|---|---|---|---|---|---|
| 1 | Recoil and drain computed from overkill damage on a KO | `sim/pokemon.ts:1595` (`damage` returns HP lost); `sim/battle.ts:2137,2170`; `data/mods/champions/scripts.ts:536,554` | golden `recoil-uncapped-on-ko`; unit `drain_on_ko_heals_half_the_hp_the_target_had_left` | `7045b3b` | 228 |
| 2 | Terrain ×1.3 gated on the defender's grounding instead of the attacker's | `data/moves.ts` electricterrain :4533, grassyterrain :7699, psychicterrain :14132 | golden `terrain-boost-gated-on-defender`; 2 unit tests (one replaced a test that pinned the old behaviour) | `3962fc2` | 50 |
| 3 | Expanding Force doesn't spread in Psychic Terrain | `data/moves.ts:4958` `onModifyMove`; `sim/battle-actions.ts:432` re-target draw | golden `expanding-force-no-spread` | `16a043b` | 116 |
| 4 | Fake Out uses turns on field, not move actions since switch-in (it also failed after any switch-in); Mat Block shares the counter | `data/moves.ts:5097` `activeMoveActions > 1`; `sim/battle-actions.ts:138,217` | golden `fake-out-after-pivot-switch-in`; unit `fake_out_works_the_turn_after_a_switch_in` | `88929a0` | 125 |
| 5 | Pinch berries not checked after recoil, Life Orb, Rocky Helmet or residual damage | `sim/battle.ts:2861` (post-action `Update`); `data/items.ts:5752` sitrusberry `onUpdate` | golden `study-0d752208ee`; unit `sitrus_berry_eaten_after_recoil_drops_holder_to_half` | `e36f7b1` | 42 |
| 6 | Electro Shot / Meteor Beam hit with the pre-boost SpA snapshot | `data/moves.ts:4644` boost in `onTryMove` | golden `study-220a63b680`; unit `electro_shot_in_rain_hits_with_its_own_spa_boost` | `1074214` | 53 |
| 7 | Full paralysis 1/4; Champions uses 1/8. Scoped by the new `Battle::champions` flag, off by default | `data/mods/champions/conditions.ts:5` `randomChance(1, 8)`, vs `data/conditions.ts` par `randomChance(1, 4)` | unit `champions_paralysis_full_skip_is_one_in_eight` (the 1/4 test still covers the default) | `c517857` | 0 keyed (see note) |
| 8 | Simultaneous faint replacements fire abilities one at a time | `sim/battle-actions.ts:175-184` `runSwitch` batching; `instaswitch` order 3 vs `runSwitch` 101; item `onSwitchInPriority` -1/-2 | goldens `study-576126830e`, `study-d2fe06dc11`; unit `simultaneous_replacements_enter_before_either_ability_fires` | `91e64fe` | 52 |
| 9 | Weather chips on its expiry turn | `sim/battle.ts:515-521` duration end skips the handler; sandstorm `onFieldResidualOrder: 1` | golden `study-af4174f41f`; unit `sand_does_not_chip_on_the_turn_it_ends` | `8ebbe1c` | 42 |
| 10 | Status moves aimed at the ally hit the first foe; Defiant fired on an ally's drop | `sim/battle.ts` validTargetLoc (`normal` / `any` accept the adjacent ally); `data/abilities.ts` defiant `target.isAlly(source)` | golden `study-75999c4aaa`; unit `status_move_aimed_at_an_ally_hits_the_ally` | `7e9cdd6` | 14 |

**Harness fix (`e1c87da`).** The keyed repair pass paired candidates in
HashMap order, so 1–3 battles flipped between identical runs. It now sorts
by the full key, and repeated runs are byte-identical. The same flipping
explains the small "regressions" seen before this fix: runs of the
unchanged binary disagreed on those same battles.

**Note on #7.** The keyed harness injects PS's own outcome for the
paralysis gate, so it cannot see the rate. The study's distribution
scenario can. `full-paralysis-rate` now matches PS: p = 0.14, where it was
1e-115 at an engine rate of 25.9% against PS's 12.2%. The flag is set by
the accuracy and conformance harnesses from the format id, and pyo3
exposes it as `Battle.from_teams(..., champions=True)`.

## Agreement trajectory

Measured after every fix, not only after each batch of 2–3. Wilson 95%
CIs, n = 1,298.

| after | fully clean | per-turn | first divergences: RNG plumbing / mechanics / decision model |
|---|---|---|---|
| baseline (`accuracy-proof`) | 214 = 16.5% (14.6–18.6) | 4006/5090 = 78.7% (77.6–79.8) | 100 / 911 / 73 |
| #1 recoil / drain | 332 = 25.6% (23.3–28.0) | 83.6% (82.7–84.5) | 101 / 785 / 80 |
| #2 terrain grounding | 362 = 27.9% (25.5–30.4) | 84.6% (83.7–85.5) | 103 / 751 / 82 |
| #3 Expanding Force | 433 = 33.4% (30.8–36.0) | 86.9% (86.0–87.7) | 103 / 676 / 85 |
| #4 Fake Out, then the deterministic harness | 495 = 38.1% (35.5–40.8) | 88.7% (87.9–89.4) | 86 / 630 / 86 |
| #5 berries after self-damage | 518 = 39.9% (37.3–42.6) | 89.2% (88.5–89.9) | 90 / 603 / 86 |
| #6 Electro Shot | 553 = 42.6% (39.9–45.3) | 90.1% (89.4–90.8) | 92 / 562 / 90 |
| #7 Champions paralysis | 553 = 42.6% (39.9–45.3) | 90.1% (89.4–90.8) | 91 / 563 / 90 |
| #8 batched replacements | 587 = 45.2% (42.5–47.9) | 90.8% (90.1–91.4) | 96 / 523 / 91 |
| #9 weather expiry | 617 = 47.5% (44.8–50.3) | 91.3% (90.7–91.9) | 96 / 493 / 91 |
| #10 ally-targeted status | **627 = 48.3% (45.6–51.0)** | **7269/7940 = 91.5% (90.9–92.1)** | 96 / 483 / 91 |

## What leads now (671 diverged battles)

- **Decision model (91).** Eject Button / Eject Pack / Red Card /
  Emergency Exit are auto-picked by the engine. Most of the 89 `switch`
  offenders are this or a downstream species mismatch.
- **Contact-ability procs (Poison Touch ~40, Flame Body, Static).** The
  study read these as "ability rolls before secondaries". They are
  actually a draw-keying problem, not a probability one:
  - the engine rolls `percent ≤ 30` under the move's Secondary key;
  - PS rolls `randomChance(3, 10)`, which the harness parks as a bool gate;
  - so the engine consumes the move's Secondary entry. With Close Combat,
    that entry is PS's self-drop roll.

  Switching the engine to PS's draw shape (`chance_keyed(3, 10)`) made it
  worse: 240 battles regressed. The bool-gate pairing is order-sensitive
  when two procs share one move use (Flame Body vs Poison Touch on the
  same Fake Out). The change was reverted, and fixing this needs harness
  keying by ability holder (see Decisions below).
- **Close Combat (59).** Mostly the Poison Touch case above (Sneasler).
- **Co-occurring effects.** Grassy Terrain (71) and Life Orb (42) are mostly
  co-occurring, not causes. Life Orb's rounding outside the ModifyDamage
  chain is still open. PS chains every `ModifyDamage` modifier (Life Orb,
  Expert Belt, screens, Friend Guard, Multiscale, resist berries, …) into
  one `modify()`; the engine rounds each one separately.
- **Long tail from §9:**
  - no dynamic speed re-sort;
  - Champions move deltas and `slicing` flags;
  - Speed Boost on a replacement;
  - Baton Pass (11 of 13 uses diverge);
  - Sucker Punch retargeting.

## Decisions for the owner

1. **Champions rules flag.** `Battle::champions` defaults to off, so gen 9
   formats keep a 1/4 paralysis rate. Champions callers (mimikyu) must pass
   `champions=True`. The engine's data deltas (Iron Head 20%, Make It Rain
   -2) are still applied everywhere. Choose one:
   - make the flag default on;
   - also move the data deltas behind the flag.
2. **Keying ability procs.** The fix for contact-ability procs is to key
   them by `(turn, ability holder, ability)` in both the PS driver and the
   engine, instead of by the active move. That is a keyed-contract change
   (`docs/conformance-key-contract.md`).
