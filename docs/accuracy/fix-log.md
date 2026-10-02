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

---

# Round 2 (2026-09-29)

Same 1,298-battle sample. From decision 2 on, the PS battles are
regenerated with the new driver (`b5`: same jobs and seeds; `b4` and `b5`
differ only in ability-roll envelopes and trace line numbers, checked
battle by battle).

## Owner decisions

1. **Champions rules come from the format** (`70dfa8d`).
   `format_rules::is_champions_format` (any `gen9champions*` id, plus Reg
   M-B / Reg M-C and aliases) sets `Battle::champions` via
   `Battle::set_format_id`. Every Champions-only delta reads it:
   - move data: `MOVES` (gen 9) vs `MOVES_CHAMPIONS`;
   - Iron Head, Moonblast, Make It Rain, Dire Claw, Freeze-Dry;
   - Salt Cure, Healer, Unseen Fist, paralysis.

   pyo3 `from_teams(format=...)` takes PS ids. Bare `"doubles"` /
   `"singles"` stay Champions, and `champions=` overrides the format.
   - **Caller impact.** Every `from_teams` caller in mimikyu and
     metagame-lab passes bare `"doubles"` / `"singles"` or omits `format`,
     and none passes `champions=`. All of them simulate Champions.
   - With the bare-game-type default they keep Champions move data, and now
     also get 1/8 paralysis, which is correct for Champions (it was 1/4).
   - `vgc_engine.calc` (no format) now uses gen 9 move data.
2. **Ability rolls keyed by holder** (`229c3e2`). See
   `docs/conformance-key-contract.md`, decision `ability`.
   - 47 battles improved; Poison Touch left the offender list.
   - One battle was exposed: `277a7cd2e1`. The engine applies a poison
     residual after the last foe fainted, and PS ended the battle first.
     Before this change, the misdrawn Poison Touch roll hid it.

## Agreement trajectory (round 2)

| after | fully clean | per-turn | first divergences: RNG plumbing / mechanics / decision model |
|---|---|---|---|
| round 1 end | 627 = 48.3% (45.6–51.0) | 7269/7940 = 91.5% (90.9–92.1) | 96 / 483 / 91 |
| D1 format-derived Champions rules | 627 = 48.3% (45.6–51.0) | 91.5% (90.9–92.1) | 96 / 483 / 91 |
| D2 ability rolls keyed by holder (b5) | 664 = 51.2% (48.4–53.9) | 7504/8138 = 92.2% (91.6–92.8) | 95 / 447 / 91 |
| F1 Eject Button: player's pick, after the move | 680 = 52.4% (49.7–55.1) | 7663/8281 = 92.5% (92.0–93.1) | 90 / 454 / 73 |
| F2 second Switch per slot = mid-turn pick | 695 = 53.5% (50.8–56.2) | 7785/8388 = 92.8% (92.2–93.3) | 90 / 461 / 51 |
| F3 Emergency Exit | 711 = 54.8% (52.1–57.5) | 7900/8487 = 93.1% (92.5–93.6) | 91 / 465 / 30 |
| F4 Trick Room switch order | 717 = 55.2% (52.5–57.9) | 7965/8546 = 93.2% (92.6–93.7) | 92 / 458 / 30 |
| F5 ModifyDamage chain (Life Orb, Expert Belt, Friend Guard, resist berries) | 735 = 56.6% (53.9–59.3) | 8100/8663 = 93.5% (93.0–94.0) | 93 / 441 / 28 |
| F6+F7 Steel Roller, Drum Beating | 766 = 59.0% (56.3–61.7) | 8307/8839 = 94.0% (93.5–94.5) | 94 / 409 / 28 |
| F8 Champions move data (BP, type, flags) | 774 = 59.6% (56.9–62.3) | 8359/8883 = 94.1% (93.6–94.6) | 95 / 401 / 28 |
| F9+F10 battle ends at last faint, Champions sleep | **786 = 60.6% (57.9–63.2)** | **8392/8904 = 94.2% (93.7–94.7)** | 93 / 391 / 28 |

## Fixes (round 2)

| # | bug | PS reference | tests added | commit | battles moved |
|---|---|---|---|---|---|
| F1 | Eject Button force-switched to the first bench mon mid-move; the replaced mon kept its queued move | `data/items.ts` ejectbutton (`switchFlag`); `sim/battle.ts` runAction switch request | unit `eject_button_replacement_is_the_players_choice_in_decision_phases`, `ejected_holder_forfeits_its_queued_move`; golden `study-8760be9ed8` | `b55c913` | 23 |
| F2 | A mon that switched in and was ejected the same turn: both switches ran at turn start (harness also dropped the pick) | same | unit `eject_button_pick_after_a_turn_start_switch_in`; golden `study-270f1619dc` | `1e6256d` | 22 |
| F3 | Emergency Exit unimplemented | `data/mods/champions/abilities.ts` emergencyexit; `data/mods/champions/scripts.ts:583`; `sim/battle-actions.ts:542,1132,1395` | unit `emergency_exit_switches_out_when_a_hit_crosses_half_hp`; golden `study-59ccb3d636` | `f0badf0` | 21 |
| F4 | Pre-turn switches fastest-first even under Trick Room | `sim/pokemon.ts:641` getActionSpeed | unit `pre_turn_switches_run_slowest_first_under_trick_room`; golden `study-dc6522e008` | `33e8683` | 9 |
| F5 | Life Orb, Expert Belt, Friend Guard and resist berries rounded one at a time after the ModifyDamage chain | `sim/battle.ts` runEvent → `modify` (one rounding); `data/items.ts` lifeorb / expertbelt; `data/abilities.ts` friendguard | goldens `study-9d848b7db0`, `study-fb56fc4f9c` | `b82a44c` | 20 |
| F6 | Steel Roller unimplemented (no terrain check, never cleared terrain) | `data/moves.ts` steelroller onTry / onHit / onAfterSubDamage | unit `steel_roller_needs_terrain_and_clears_it`; golden `study-8fba727aef` | `739f439` | 34 |
| F7 | Drum Beating's Spe −1 missing | `data/moves.ts` drumbeating secondary | table assertion; golden `study-3d3fc88bdf` | `9753483` | 5 |
| F8 | Remaining Champions move deltas (9 BP, Snap Trap type, slicing / punch / sound flags) | `data/mods/champions/moves.ts` | `champions_move_data_overrides_applied`; golden `study-1f62339620` | `b30410b` | 14 |
| F9 | Actions and residuals continued after a side was out | `sim/battle.ts` faintMessages → checkWin | unit `battle_ends_before_residuals_once_a_side_is_out`; golden `study-11f8f58c63` | `f9eb8ba` | 6 |
| F10 | Champions sleep length (`sample([2, 3, 3])`); harness recovers the unrecorded sample | `data/mods/champions/conditions.ts:22` | unit `champions_sleep_lasts_one_or_two_turns`; golden `study-a6a15861c8` | `443d9d6` | 6 |

No battle diverged earlier after any fix. The one earlier-diverging battle
in a trial of F10 was a harness pairing error: a condition sample was paired
to a Protect stall roll. The fix was to rank condition samples last in the
repair pass.

## What leads now (512 diverged battles)

- **RNG plumbing (93).** Draws PS never supplied, mostly speed ties and
  `sample()` draws outside `Battle.random`.
- **Decision model (28).** Red Card is still a first-bench auto-pick. PS
  drags in a random bench mon with `sample()`, which the keyed envelopes
  don't carry. Revival Blessing (6) is also unmodelled.
- **Mechanics (391).** The offender table is now a long tail:
  - `switch` (65): mostly an earlier HP difference deciding a KO;
  - Grassy Terrain (57) and Life Orb (33): co-occurring, not causes;
  - Close Combat (25), Knock Off (15), Rocky Helmet (15);
  - Baton Pass (11 of 13 uses);
  - Rage Fist, Burn Up's type loss (unimplemented), Leech Life;
  - team PP from the Champions table (not ported).

Notes:
- **Eject Button fallback.** A caller that queues no pick still gets the
  first bench mon.
- **"Cascaded species mismatches after a switch".** This is not one bug.
  - Under Trick Room it was F4.
  - Otherwise it is mostly an earlier HP difference that decides whether a
    KO (and so a replacement) happens. The species check runs before the HP
    check, so these show up as species mismatches.


---

# Round 3 (2026-09-30, branch `mechanics-fixes-3`)

Same 1,298-battle sample and the same PS battles (`b5`). Result: fully
clean battles **60.6% → 65.9%**, per-turn agreement **94.2% → 95.3%**.

## Owner decisions

1. **`calc` takes an optional format** (`89f34d9`).
   - Rust: `calc::Field` has a `champions` flag, on by default, and a
     `Field::format(id)` builder. pyo3: `vgc_engine.calc(..., format=None)`.
   - The format resolves like `Battle.from_teams`
     (`format_rules::champions_for_format_arg`):
     - omitted, `"doubles"`, `"singles"` or a `gen9champions*` id → Champions data;
     - any other PS id (`gen9vgc2025regh`) → standard gen 9;
     - anything else raises `ValueError` / `CalcError::UnknownFormat`.
   - This reverses round 2's side effect: a format-less `calc` is
     Champions again.
   - The `@smogon/calc` oracle harness keeps standard gen 9.
   - **Caller impact: none.** No `vgc_engine.calc` caller exists in
     mimikyu, metagame-lab or vgc-winrates. mimikyu's damage rows come
     from a node calc worker, and vgc-winrates' `calc.py` is its own
     module.
2. **Champions PP** (`031a72e`).
   - Team building takes the format's Champions rule:
     `build_member_in`, `TeamBuilder::from_json_in` / `from_showdown_text_in`.
   - A Champions team gets PS's Champions PP (`data/mods/champions/scripts.ts`):
     - base PP from the mod: Protect / Wish / Spiky Shield … 5, Shell Trap /
       Spin Out 10, everything else capped at 20;
     - max PP `(pp / 5 + 1) * 4`, or base PP for noPPBoosts moves.
     - Examples: Protect 8, Earthquake 12, Scratch 20.
   - Standard formats keep three PP Ups (`pp * 8 / 5`).
   - Wiring:
     - pyo3 `from_teams` builds with the effective Champions flag (format,
       or `champions=`);
     - the accuracy, conformance and golden runners pass it from the format id;
     - metagame-lab's explicit `move_pp_json` overlay still wins.
   - No battle in the sample changed (every first divergence was identical).

## What the 93 "RNG plumbing" battles were

The keyed harness replays every draw PS made; its raw PRNG trace covers
all of them, including `sample()` and speed-tie shuffles. So none of the
93 was a draw PS's records don't reveal. Almost all were **engine draws
PS never makes**: the engine rolled where PS has no roll, and the fallback
stream decided the outcome. Grouped by the missed draw on the divergence
turn:

| cause | battles | fix |
|---|---|---|
| Poison-type Toxic rolled accuracy | 7 | F1 |
| charged Solar Beam released as the submitted move (Heat Wave …) | 6+ | F2 |
| Wide Guard rolled the Protect stall counter | 7 | F5 |
| accuracy rolled before Sap Sipper / Flash Fire / Protect-family TryHit | 3+ | F3, F6 |
| freeze-thaw roll keyed to the previous action | several | F7 |
| whole extra move uses (Accuracy + Crit + Damage) | 26 of the 39 left | none yet: the engine acted where PS didn't (a flinch, fail or target difference earlier in the turn), so these are mechanics |
| Range draws under a stale context (Protect, Trick Room, Infestation …) | 11 of the 39 left | none yet: keying gaps, not unobservable |

**No "unobservable" class was added.** By the owner's definition (PS's
logs don't reveal the draw), nothing in the keyed sample qualifies.
Red Card's pick is observable too:
- the log names the mon that enters (`|drag|`);
- the trace holds the `sample` index.

Keying it needs one of these:
- the engine mirrors PS's `side.pokemon` order, which switches permute (the
  engine's bench is in roster order);
- the driver records the team index of the chosen mon.

Either fixes phazing (Whirlwind, Dragon Tail) too. Neither is done yet.
With 39 RNG-sensitive battles left, the with/without split is small:
excluding them, 856 / 1,259 = 68.0% of battles are clean.

## Fixes (round 3)

| # | bug | PS reference | tests added | commit | battles moved |
|---|---|---|---|---|---|
| F1 | Poison-type Toxic rolled accuracy (90%) | `sim/battle-actions.ts:627,731` | unit `toxic_from_a_poison_type_never_misses_and_draws_no_accuracy` | `8c2167d` | F1+F2: 43 |
| F2 | A charging mon released its move only if the player re-picked the slot, at the submitted target | `sim/side.ts:675-688` (getLockedMove, targetLoc) | unit `charged_move_releases_at_its_charge_target_whatever_the_choice` | `11d1117` | (with F1) |
| F3 | Accuracy rolled before the TryHit checks (Protect family, Wide / Quick Guard, absorbing and immunity abilities, Wonder Guard, Levitate) | `sim/battle-actions.ts:556-577` hit-step order | unit `absorbing_ability_fires_before_the_accuracy_roll` | `30cae01` | 6 |
| F4 | A type-immune target was hit for 0 and still took the secondary (Electroweb's Spe −1 on a Ground-type); the gate uses the −ate type | `sim/battle-actions.ts:654` hitStepTypeImmunity; `data/abilities.ts` aerilate … | units `type_immune_target_takes_no_secondary_effect`, `ate_ability_move_is_not_type_immune_by_its_base_type` | `5c72ba2` | F4–F6: 25 |
| F5 | Wide Guard / Quick Guard rolled the stall counter | `data/moves.ts` wideguard / quickguard (onTry willAct, onHitSide stall) | unit `wide_guard_after_protect_always_succeeds` | `8637685` | (with F4) |
| F6 | Flash Fire's absorb didn't make the move sure-hit on its other targets | `data/abilities.ts:1341` flashfire (`move.accuracy = true`) | unit `flash_fire_absorb_makes_the_move_sure_hit_on_other_targets` | `bb069a0` | (with F4) |
| F7 | Champions freeze: thaw 1/4, guaranteed on the 3rd attempt (was gen 9's uncapped 1/5); thaw roll keyed to the frozen mon's move | `data/mods/champions/conditions.ts` frz | unit `champions_freeze_thaws_by_the_third_move_attempt` | `ecfd530` | 11 |
| F8 | A move that failed its onTry (Sucker Punch, Fake Out, Damp) wasn't recorded as the last move | `sim/battle-actions.ts` runMove (moveUsed before useMove) | unit `a_move_that_fails_its_try_check_is_still_the_last_move` | `fcb6d73` | F8+F9: 16 |
| F9 | Encore hit the first foe instead of its target, didn't override that turn's queued move, and ignored the +1 duration | `data/moves.ts:4724` encore; `sim/battle-actions.ts:228` OverrideAction; `sim/battle.ts:2490` getRandomTarget | unit `encore_hits_its_chosen_target_and_overrides_that_turns_move` (and `encore_expires_after_three_turns` re-seeded) | `2427bf8` | (with F8) |

## Agreement trajectory (round 3)

Wilson 95% CIs, n = 1,298.

| after | fully clean | per-turn | first divergences: RNG plumbing / mechanics / decision model |
|---|---|---|---|
| round 2 end | 786 = 60.6% (57.9–63.2) | 8392/8904 = 94.2% (93.7–94.7) | 93 / 391 / 28 |
| D1 calc format + D2 Champions PP | 786 = 60.6% (57.9–63.2) | 8392/8904 = 94.2% (93.7–94.7) | 93 / 391 / 28 |
| F1+F2 Toxic, charge lock | 814 = 62.7% (60.0–65.3) | 8591/9075 = 94.7% (94.2–95.1) | 72 / 384 / 28 |
| F3 TryHit before accuracy | 815 = 62.8% (60.1–65.4) | 8608/9091 = 94.7% (94.2–95.1) | 66 / 388 / 28 |
| F4–F6 type immunity, Wide Guard, Flash Fire | 835 = 64.3% (61.7–66.9) | 8747/9210 = 95.0% (94.5–95.4) | 55 / 378 / 29 |
| F7 Champions freeze | 843 = 64.9% (62.3–67.5) | 8803/9258 = 95.1% (94.6–95.5) | 43 / 382 / 29 |
| F8+F9 last move on failure, Encore | **856 = 65.9% (63.3–68.5)** | **8904/9346 = 95.3% (94.8–95.7)** | 39 / 373 / 29 |

Excluding the 39 RNG-sensitive battles: 856/1,259 = 68.0% clean.

**Earlier divergences.** Every battle whose first divergence moved
earlier during the round was explained:
- **Latent bugs exposed by a fix.** Each was fixed later in the round:
  - F2 exposed the Encore override (`6022fed02a`, `8ab40b8058`). Before
    F2, the engine never released the charged Solar Beam, so the Encore
    turn matched by accident.
  - F3 exposed three:
    - Flash Fire's sure-hit (`f034755a20`);
    - type immunity (`2f98da05e5`);
    - Wide Guard's roll (`4b44dba71e`).
  - Encore's chosen-target fix exposed F8 (`576126830e`).
- **A first F4 build regressed 60 battles.** `move_type_in_ctx` lacked
  the −ate abilities, so the gate dropped Aerilate Hyper Voice on Ghosts.
  The −ate fix was folded into F4 before any measurement was kept.
- **One battle is still earlier than at round 2's end:** `116b68ea9c`,
  turn 9 → 7. It is RNG-sensitive: a Range draw under a stale Hyper Beam
  context, whose fallback value shifted when the engine stopped making
  extra draws.

`cargo test --workspace --exclude vgc-engine-py` passed at every commit,
with `ps-rng` both off and on. The pyo3 tests
(`crates/vgc-engine-py/tests`) pass.

## What leads now (442 diverged battles)

- **Encore-adjacent and pivots.** On the divergence turn:
  - Emergency Exit 13, Red Card 12, Baton Pass 12 (11 of 13 uses diverge),
    Revival Blessing 7, Eject Button 5;
  - Encore 10, down from 23 before F8/F9.
- **Red Card / phazing pick.** Observable (see above). Needs PS
  `side.pokemon` order or a driver change.
- **Mechanics offenders** (`switch` 57, Grassy Terrain 55, Leftovers 29,
  Life Orb 28) are mostly co-occurring; the analysis in round 2 still holds.
- **Highest divergence rate per use:**
  - Baton Pass 85%, Storm Throw 40%, Dragon Darts 36%, Feint 33%;
  - Stockpile 30%, Roost 25%, Rage Fist 18%;
  - Knock Off 11% (15 battles).
- **Not yet ported:** Burn Up's type loss.
- **26 "RNG plumbing" battles** are really a whole extra move use. The
  engine acts where PS doesn't, so their causes are mechanics upstream on
  the same turn.

---

# Round 4 (2026-09-30, branch `mechanics-fixes-4`)

Same 1,298-battle sample and PS battles (`b5`). Result: fully clean
battles **65.9% → 68.9%**, per-turn agreement **95.3% → 95.8%**. Against
the round-3 end, 48 battles' first divergence moved later and none moved
earlier.

## Owner decision: PS's bench order for random switch-ins (option A)

`81282d1`. PS draws a random switch-in with `sample(possibleSwitches)`
over `side.pokemon` from `active.length` on (`sim/battle.ts:1570-1585`).
Every switch permutes that array: the incoming and outgoing mons swap
places (`sim/battle-actions.ts:131-133`). The engine drew over roster
order.

- `Side::ps_order` now tracks PS's order and `Side::random_switch_candidates`
  is `possibleSwitches`. Whirlwind, Roar, Dragon Tail, Circle Throw and
  Red Card draw from it.
- **Unconditional.** It only reorders the candidates, so the draw count
  and distribution are unchanged. `ps_order` is not game state, so the
  canonical hash ignores it, like any bench permutation.
- **Behind `ps-rng`:** one difference. PS's `sample` draws `random(1)`
  even for a one-mon bench, and the engine's `range(1)` skips that draw,
  so `Rng::Ps` makes the draw explicitly.
- **Keying.** The keyed harness needed no change. The driver already
  records `sample` as a `range` draw under the move-use envelope
  (turn, user, move). The engine draws under the same context, and the
  repair pass pairs any target mismatch.
- **Option B was not needed.** On the divergence turn:
  - Whirlwind, Roar and Circle Throw had no divergences;
  - the one Dragon Tail battle (`9ffd4d9774`) diverges on a battle-start
    terrain order. Its drag already matched.
  - Red Card was a first-bench auto-pick, not a random draw, so the
    ordering alone changed nothing (bench order alone: 0 battles moved).
    Fixing Red Card itself (F1, F3) took it from 12 battles to 1. The one
    left, `7d4d22f12a`, also involves Emergency Exit.
- **Seeded differential (`accuracy psrng`)**, `ps-rng` on. The sample has
  32 PS drag draws. Only 3 are reached with the stream still aligned,
  because most battles desynchronise earlier.
  - Before: 2 of 3 matched through the drag; `b2c0f68e9c` diverged at the
    drag draw itself (a Red Card).
  - After: **3 of 3 match draw for draw.**
  - Whole-sample rate: before 16 clean (1.2%), 1498/2780 turns; after 15
    clean (1.2%), 1524/2807 turns. The clean battle lost, `21d2b3b4ce`, had
    already desynchronised at turn 1 (an ability-roll order draw). It was
    clean by luck; the recharge fix (F8) shifted its later stream.

## Fixes (round 4)

| # | bug | PS reference | tests added | commit | battles moved |
|---|---|---|---|---|---|
| D | Random switch-ins drew in roster order | `sim/battle.ts:1570-1585`; `sim/battle-actions.ts:131-133,162` | units `random_switch_candidates_follow_ps_side_order`, `whirlwind_draw_lands_on_the_mon_ps_would_drag_in` | `81282d1` | 0 |
| F1 | Red Card swapped in the first bench mon mid-hit; PS flags `forceSwitchFlag` and drags a random mon after the action. The card is used up even when DragOut blocks | `data/items.ts:5152-5164`; `sim/battle.ts:2821-2829` | unit `red_card_drags_a_random_bench_mon_after_the_attackers_action` | `8c5ac4d` | 10 |
| F2 | A pivot switch-in that Emergency Exited the same turn reused the slot's first mid-turn pick | `sim/battle.ts:2877-2915` | unit `emergency_exit_after_a_pivot_switch_in_takes_the_next_pick` | `ceb9a33` | 3 |
| F3 | A Red-Carded attacker still took Life Orb recoil (and Shell Bell healed) | `data/items.ts:3414` lifeorb, `:5658` shellbell (`!source.forceSwitchFlag`) | F1's test, corrected | `71d123e` | 4 |
| F4 | Drain healed after Rocky Helmet / Rough Skin and the secondaries; PS heals inside `spreadDamage` | `sim/battle.ts:2170`; `sim/battle-actions.ts:1121` | unit `drain_heals_before_rocky_helmet_hits_back` | `10f3207` | 10 |
| F5 | Emergency Exit didn't fire from residual damage (poison, sand, Leech Seed) | `sim/battle.ts:2813-2816,2862-2868` | unit `emergency_exit_from_residual_damage_asks_for_a_switch` | `8789ec0` | 1 |
| F6 | Swarm boosted Fighting moves (type 6) instead of Bug (11) | `data/abilities.ts` swarm | unit `swarm_boosts_bug_moves_not_fighting_moves` | `590689f` | 2 |
| F7 | Baton Pass unimplemented (user stayed in) | `data/moves.ts:1092-1118`; `sim/pokemon.ts:1246-1268` copyVolatileFrom | unit `baton_pass_switches_out_mid_turn_and_passes_boosts_and_substitute` | `7a0f989` | 12 |
| F8 | The recharge turn ran after the other before-move checks and the move's onTry, so a Fake Out picked on it kept the recharge pending | `data/conditions.ts:364` mustrecharge (priority 11) | unit `recharge_turn_is_spent_whatever_move_was_chosen` | `83c88d8` | 1 |
| F9 | First Impression had no first-turn check | `data/moves.ts` firstimpression onTry | unit `first_impression_fails_after_the_first_move_action` | `4accbf9` | 1 |
| F10 | Burn Up's Fire-type loss (and its fail when not Fire) | `data/moves.ts:2092-2117` | unit `burn_up_strips_the_fire_type_and_then_fails` | `f4ccdda` | 0 (3 uses, none divergent) |
| F11 | Revival Blessing unimplemented | `data/moves.ts:15110`; `sim/side.ts:965-977`; `sim/battle.ts:2781-2797` | unit `revival_blessing_revives_the_picked_fainted_mon_at_half_hp` | `954b67c` | 7 |

## Agreement trajectory (round 4)

Wilson 95% CIs, n = 1,298. Measured after every change.

| after | fully clean | per-turn | first divergences: RNG plumbing / mechanics / decision model |
|---|---|---|---|
| round 3 end | 856 = 65.9% (63.3–68.5) | 8904/9346 = 95.3% (94.8–95.7) | 39 / 373 / 29 |
| D bench order | 856 = 65.9% (63.3–68.5) | 8904/9346 = 95.3% (94.8–95.7) | 39 / 373 / 29 |
| F1 Red Card | 860 = 66.3% (63.6–68.8) | 8934/9372 = 95.3% (94.9–95.7) | 40 / 376 / 21 |
| F2 second mid-turn pick | 863 = 66.5% (63.9–69.0) | 8958/9393 = 95.4% (94.9–95.8) | 40 / 376 / 18 |
| F3 Life Orb on a flagged attacker | 867 = 66.8% (64.2–69.3) | 8974/9405 = 95.4% (95.0–95.8) | 40 / 374 / 16 |
| F4 drain order | 877 = 67.6% (65.0–70.1) | 9037/9458 = 95.5% (95.1–95.9) | 40 / 365 / 15 |
| F5 residual Emergency Exit | 878 = 67.6% (65.0–70.1) | 9043/9463 = 95.6% (95.1–96.0) | 40 / 365 / 14 |
| F6 Swarm | 878 = 67.6% (65.0–70.1) | 9049/9469 = 95.6% (95.1–96.0) | 41 / 365 / 13 |
| F7 Baton Pass | 886 = 68.3% (65.7–70.7) | 9110/9522 = 95.7% (95.2–96.1) | 42 / 357 / 12 |
| F8 recharge order | 887 = 68.3% (65.8–70.8) | 9113/9524 = 95.7% (95.3–96.1) | 42 / 356 / 12 |
| F9 First Impression | 887 = 68.3% (65.8–70.8) | 9115/9526 = 95.7% (95.3–96.1) | 41 / 357 / 12 |
| F10 Burn Up | 887 = 68.3% (65.8–70.8) | 9115/9526 = 95.7% (95.3–96.1) | 41 / 357 / 12 |
| F11 Revival Blessing | **894 = 68.9% (66.3–71.3)** | **9142/9546 = 95.8% (95.3–96.2)** | 41 / 355 / 7 |

**Earlier divergences.** None against the round-3 end. One appeared in a
trial: First Impression alone moved `21d2b3b4ce` from clean to turn 4. It
exposed the recharge-order bug: Sirfetch'd picked First Impression on
its recharge turn, the new gate aborted first, and the recharge carried
into the next turn. F8 fixed that and landed first.

`cargo test --workspace --exclude vgc-engine-py` passed at every commit,
with `ps-rng` both off and on.

## What leads now (404 diverged battles)

- **Pivots and pick-driven effects are nearly gone.** On the divergence
  turn: Emergency Exit 3, Eject Button 3, Revival Blessing 2, Red Card 1,
  Baton Pass 0 (were 13 / 5 / 7 / 12 / 12).
- **Encore (8) is mostly a co-occurring effect.** Its triaged battles
  diverge on other mechanics:
  - a self-boost secondary (Fiery Dance) doesn't apply when the target
    faints;
  - resist berries check the move's base type, not the −ate type (Roseli
    Berry vs Pixilate Hyper Voice), at `battle.rs` `type_resist_berry_fires`
    and `try_consume_type_resist_berry`;
  - First Impression (fixed, F9).
- **Mechanics offenders** (`switch` 55, Grassy Terrain 53, Life Orb 27,
  Leftovers 25) are still mostly co-occurring. Close Combat 25 and Knock
  Off 16 (10.8% of uses) lead the rest.
- **Highest divergence rate per use:**
  - Confusion 44% and Storm Throw 40%;
  - Dragon Darts 36%, Feint 33%, Stockpile 29%;
  - Roost 25% and Rage Fist 18%.

---

# Round 5 (2026-09-30, branch `mechanics-fixes-5`)

Same 1,298-battle sample. The work directory was lost mid-round, so the
PS battles were regenerated from the same replay sample, jobs and seeds
(`recon.js` → `ps-battle.js`, PS `a5df8274`); the round-4 end reproduced
exactly (894 clean, 9142/9546 turns; seeded 15 clean, 1524/2807).

Result: forced-RNG fully clean battles **68.9% → 71.9%**, per-turn
agreement **95.8% → 96.3%**. Seeded differential (`ps-rng`) fully clean
**15 → 488 (1.2% → 37.5%)**, turns **1524/2807 → 5895/6705**.

## API change: Revival Blessing's pick (owner-approved)

`931d84d`. PS asks for Revival Blessing's target in a mid-turn switch
request whose valid answers are the side's fainted Pokémon, active slot
or bench (`sim/side.ts:958-977`). The engine took the pick as a deferred
`Switch` after the move but never advertised it, so API-driven callers
never supplied one and it revived the first fainted mon.

- `Battle::mid_turn_picks(side, choice)` / `mid_turn_picks_into`: the
  `Switch` picks PS would request after a slot's choice. Revival Blessing:
  each fainted party member (none when nothing fainted, since the move
  fails). Self-switch moves: the living bench.
- `vgc-solver` `joint_actions` expands picks through it (it had a slug list
  of pivot moves, which also missed Tera pivots).
- pyo3: `Battle.mid_turn_picks(side, choice_tuple)` → switch tuples.
- Without a pick, Revival Blessing still revives the first fainted mon
  (PS's `autoChoose`).

### Caller impact (read-only survey)

- **mimikyu: no change needed.** Nothing passes `decision_phases=True`,
  so there are no replacement phases or `[move, switch]` slot lists.
  Latent if it ever opts in: `jev_policy.py:1137-1145` filters switches
  to living team indices (would drop revive picks);
  `bss_search_rollout_prototype.py:109-122` looks switch targets up in
  the bench.
- **metagame-lab** (`decision_phases=True` in `src/rust_worker.py:64`):
  - `rust_worker.py:41-49` `command()` joins every tuple of a joint with
    `", "`, so `[move 0, switch 2]` renders as two slots' commands. The
    trailing switch is the same slot's mid-turn pick (for Revival
    Blessing, a revive).
  - `public/app.js:47-52` `actionLabel` splits on `,` by slot, so it
    mislabels the pick as slot 1, and says "Switch to" for a revive.
  - `snapshot()`'s `phase: 'switch' if needs_replacements()` is only a
    label; it stays correct for round 4's Emergency Exit change (a living
    mon forced out), where living slots get `pass`.
  - `step_move(*joints)` and `check-engine.py`'s joint checks need no
    change.

## Fixes (round 5)

| # | bug | PS reference | tests added | commit | battles moved |
|---|---|---|---|---|---|
| F1 | Self-boost secondaries (Fiery Dance, Power-Up Punch, Flame Charge ...) lost when the hit KOs the target | `data/mods/champions/scripts.ts:385-388`; `sim/battle-actions.ts:1336-1351` | unit `self_boost_secondary_lands_when_the_hit_knocks_the_target_out` | `44f2097` | 6 |
| F2 | Resist berries used the move's data type and the species' types (Pixilate Hyper Voice vs Roseli; Tera) | `data/items.ts` roseliberry et al.; `data/abilities.ts` pixilate | units `resist_berry_checks_the_moves_type_after_pixilate`, `resist_berry_checks_effectiveness_against_the_tera_type` | `027772c` | 4 |
| F3 | A hit's pinch berry was eaten before Knock Off took it | `data/mods/champions/scripts.ts:407-416, :538`; `data/moves.ts:9975` | unit `knock_off_removes_a_sitrus_berry_before_it_can_be_eaten` | `8974684` | 3 |
| F4 | Symbiosis handed over Floette-Eternal's Floettite (so it never Mega Evolved); a Mega-Evolved holder's stone could be knocked off | `data/items.ts:2189-2197`; `data/abilities.ts:4837-4851` | units `symbiosis_cannot_hand_over_the_holders_own_mega_stone`, `knock_off_cannot_remove_a_mega_evolved_holders_stone` | `e7fb7ab` | 15 |
| F5 | Darkest Lariat / Sacred Sword / Chip Away / Nihil Light applied Def and evasion boosts | `data/moves.ts` (`ignoreDefensive`, `ignoreEvasion`); `sim/battle-actions.ts:719, :1691` | units `darkest_lariat_ignores_the_targets_defense_boosts`, `darkest_lariat_ignores_the_targets_evasion` | `6ce1265` | 4 |
| F6 | The sleep (and Champions freeze) counter was wiped on switch-out, so a mon woke on its first attempt back | `data/conditions.ts:47-81`; `data/mods/champions/conditions.ts` | unit `sleep_counter_survives_switching_out` | `83e7d7a` | 8 (1 regressed, fixed by F7) |
| F7 | Sleep from a secondary (Dire Claw, Relic Song ...) or Effect Spore drew its duration from the placeholder RNG | `data/conditions.ts:59`; `data/mods/champions/moves.ts` direclaw | unit `sleep_from_a_secondary_draws_its_duration_from_the_battle_rng` | `d2f60b2` | 2 |
| F8 | Secondaries ran after the DamagingHit reactions and Knock Off (Poison Touch beat Nuzzle's paralysis) | `data/mods/champions/scripts.ts:385-416` | unit `move_secondary_lands_before_the_attackers_poison_touch` | `0746bba` | 7 (1 regressed, see below) |
| F9 | Self-boost secondaries skipped through a Substitute | `data/mods/champions/scripts.ts:350-353` | unit `self_boost_secondary_lands_through_a_substitute` | `ec53afc` | 0 |
| F10 | Poison Touch hit Shield Dust / Covert Cloak holders and skipped its roll on a KO | `data/abilities.ts` poisontouch | unit `covert_cloak_blocks_poison_touch` | `6923fcf` | 1 |
| F11 | Throat Chop, Salt Cure and Psychic Noise skipped their 100% secondary roll | `sim/battle-actions.ts:1343` | ps-rng unit `ps_rng_a_certain_secondary_still_rolls` | `169ea5b` | 0 (draw only) |

The Encore battles triaged in round 4 were F1 and F2; each fix covers
the whole class (every `self` secondary; every type-changing effect via
`move_type_in_ctx` and every type change on the defender via
`effectiveness_for_move_type`).

## Agreement trajectory (round 5, forced-RNG)

Wilson 95% CIs, n = 1,298.

| after | fully clean | per-turn |
|---|---|---|
| round 4 end | 894 = 68.9% (66.3–71.3) | 9142/9546 = 95.8% (95.3–96.2) |
| F1 self-boost on KO | 899 = 69.3% (66.7–71.7) | 9166/9565 = 95.8% (95.4–96.2) |
| F2 resist berries | 901 = 69.4% (66.9–71.9) | 9193/9590 = 95.9% (95.4–96.2) |
| F3 Knock Off vs berry | 903 = 69.6% (67.0–72.0) | 9214/9609 = 95.9% (95.5–96.3) |
| F4 own Mega Stone | 914 = 70.4% (67.9–72.8) | 9310/9694 = 96.0% (95.6–96.4) |
| F5 ignoreDefensive | 918 = 70.7% (68.2–73.1) | 9329/9709 = 96.1% (95.7–96.5) |
| F6 sleep counter | 925 = 71.3% (68.7–73.7) | 9362/9735 = 96.2% (95.8–96.5) |
| F7 secondary sleep duration | 926 = 71.3% (68.8–73.7) | 9371/9743 = 96.2% (95.8–96.5) |
| F8 secondaries before reactions | 932 = 71.8% (69.3–74.2) | 9416/9782 = 96.3% (95.9–96.6) |
| F9 (and the ps-rng commits) | 932 | 9416/9782 |
| F10 Poison Touch | **933 = 71.9% (69.4–74.3)** | **9422/9787 = 96.3% (95.9–96.6)** |
| F11 100% secondary rolls | 933 | 9422/9787 |

**Earlier divergences.**
- F6 moved `b0eee8e256` earlier: its Dire Claw sleep had the wrong
  duration (F7), which the wiped counter hid. F7 fixed it.
- F8 moved `fb2bf66e3b` from clean to turn 4. Its first replay pass now
  matches PS on that turn (Dire Claw's paralysis fails on an Electric
  type, then Poison Touch poisons). The keyed runner's repair pass then
  re-pairs the turn's Range draws while fixing a later miss, and lands on
  a sleep. This is a harness repair artifact, not an engine regression.

`cargo test --workspace --exclude vgc-engine-py` passed with `ps-rng` off
and on at the measured commits; the pyo3 tests pass.

## Part 2: `ps-rng` draw order

The first-divergence walk was tightened first (`b643b3c`): draws must be
on the same turn, and the engine's getTarget draws count as
`random_target`. The old walk matched a PS residual shuffle with an
engine target draw a turn later and reported divergences late.

First divergent PS draw at the start (stricter walk, 1,300 battles): the
Residual handler sort 447, `secondaries` 235, spread accuracy order 211,
runAction `Update` ties 100.

| iteration | commit | seeded clean | turns matched |
|---|---|---|---|
| start (round 4 end) | — | 15 (1.2%) | 1524/2807 |
| Residual handler-sort ties | `b61d007` | 43 | 2201/3456 |
| F8 secondaries before reactions | `0746bba` | 44 | 2301/3555 |
| spread moves in PS hit-step order | `f76886d` | 73 | 3072/4297 |
| target secondaries rolled on KO / Substitute | `3b791df` | 203 | 4157/5252 |
| re-sort covers fainted users' moves | `0609ea4` | 244 | 4443/5497 |
| selfDrops before secondaries and procs | `1530424` | 254 | 4542/5586 |
| Expanding Force: two re-picks, spread order | `7113140` | 304 | 4971/5965 |
| F10 Poison Touch | `6923fcf` | 348 | 5303/6253 |
| resolveAction getTarget for chosen targets | `d92092e` | 461 | 5662/6499 |
| weather / terrain change sorts | `394dfe9` | 464 | 5693/6527 |
| hit-loop `Update` sorts | `8949494` | 465 | 5719/6552 |
| F11 100% secondary rolls | `169ea5b` | **488 (37.5%)** | **5895/6705** |

Tried and reverted: shuffling tied move actions in commitChoices'
`queue.sort`. PS does draw there, but the engine's keys (base priority,
live Speed) differ from PS's (ModifyPriority, cached `speed`): 36
battles moved later, 5 earlier, 3 fewer clean.

Earlier first draw divergences: `f76886d` moved `1e765a1ca2` (the engine
rolls accuracy on a target PS skips, which the old order masked);
`394dfe9` moved 3 (PS sorts on cached `speed`, e.g. without Choice Scarf
at battle start). Every other iteration moved none earlier.

Remaining top first divergent PS draws (935 battles still diverge):

| PS site | battles |
|---|---|
| `hitStepAccuracy` (an accuracy roll the engine doesn't make there) | 163 |
| runAction `eachEvent('Update')` ties | 144 |
| `getTarget` → `getRandomTarget` | 113 |
| hit-loop `Update` ties (status moves' hit loops, cached-speed ties) | 78 |
| `resolveAction` → `getRandomTarget` | 59 |
| commitChoices `queue.sort` ties | 54 |
| Residual handler sort (handlers not modelled) | 46 |

---

# Round 6 (2026-09-30, branch `ps-rng-6`)

Same 1,298 / 1,300-battle sample and PS battles as round 5 (the work
directory was intact; the round-5 end reproduced exactly: forced-RNG 933
clean, 9422/9787 turns; seeded 488 clean, 5895/6705).

Result: seeded differential (`ps-rng`) fully clean **488 → 804 (37.5% →
61.8%)**, turns **5895/6705 → 8572/9066**. Forced-RNG fully clean
**71.9% → 75.5%** (933 → 980), per-turn **96.3% → 96.8%** (9422/9787 →
9691/10009); 56 battles improved, none regressed.

## Owner-approved: PS's cached speed under `ps-rng`

`5e78624`. PS speed-sorts on `Pokemon.speed`, which only `updateSpeed()`
refreshes: every active at `commitChoices` (mid-turn switch requests
included), before each gen-8+ re-sort and at the residual, and a switch-in's
own at `insertChoice`; `setSpecies` resets it to the raw Speed stat
(switch-out, faint, Mega Evolution) and a drag never refreshes it. The
engine keeps that cache per team member under `ps-rng` only and every tie
sort reads it. Default builds are unchanged.

## Mechanics fixes (all builds)

| commit | bug | PS reference | tests | default RNG stream |
|---|---|---|---|---|
| `7656392` | no gen-8+ dynamic re-sort: the order was fixed at turn start, so a mid-turn Tailwind / Trick Room / Icy Wind / paralysis never changed who moved next | `sim/battle.ts:2917-2924`, `:2641-2660`; `sim/battle-queue.ts:249, :290` | `tailwind_mid_turn_reorders_the_remaining_moves` | changes (action order; ties keep their order, no new draws) |
| `a254438` | Mega Evolutions ran p1 first, not by Speed | `sim/battle-queue.ts:184`; `sim/battle.ts:404` | `faster_mega_evolves_first` | changes when both sides Mega Evolve |
| `d407d9c` | a fainted mon's queued move sorted on its boosted Speed (faintMessages clears boosts and volatiles) | `sim/battle.ts:2563` | `a_fainted_mons_speed_drops_its_boosts` | no (the action is skipped either way) |
| `151898c` | Glaive Rush's drawback unimplemented (double damage taken, moves against the user can't miss, until its next move) | `data/moves.ts` glaiverush | `glaive_rush_doubles_the_damage_its_user_takes`, `glaive_rush_ends_when_its_user_moves_again`, `moves_against_a_glaive_rush_user_cannot_miss` | changes (no accuracy roll against a Glaive Rush user) |
| `de8a6dd` | Trace always copied the first foe; PS samples among valid adjacent foes | `data/abilities.ts` trace `onUpdate` | `trace_copies_a_random_adjacent_foe` | changes (one draw per Trace) |

Cost: the per-action re-sort adds about 4% to default `perf_bench`
ns/step (interleaved runs on a loaded machine, median 1,700 → 1,772); no
step allocations.

## Part 2: `ps-rng` draw order

| iteration | commit | seeded clean | turns matched | forced-RNG clean |
|---|---|---|---|---|
| start (round 5 end) | `773aa10` | 488 (37.5%) | 5895/6705 | 933 |
| cached `Pokemon.speed` | `5e78624` | 488 | 5893/6703 | 933 |
| team preview's `queue.sort` ties | `2e0538b` | 497 | 6070/6871 | 933 |
| status moves' hit-loop Updates | `2b78d42` | 509 | 6218/7007 | 933 |
| dynamic re-sort (mechanics) | `7656392` | 541 | 6474/7231 | 972 |
| no target draws in singles | `7a7b965` | 541 | 6474/7231 | 972 |
| commitChoices' `queue.sort` ties for every action | `2eb92ea` | 583 | 6841/7556 | 972 |
| PS's own `speedSort` for the turn's order | `58a236a` | 588 | 6920/7630 | 972 |
| switchIn's BeforeSwitchOut Update | `fe44631` | 634 | 7417/8081 | 972 |
| Mega Evolution in Speed order (mechanics) | `a254438` | 636 | 7454/8116 | 972 |
| weather upkeep's two sorts | `508ea83` | 653 | 7615/8260 | 972 |
| a status move into Protect skips the hit loop | `5a05ec4` | 655 | 7624/8267 | 972 |
| fainted mon's cleared Speed (mechanics) | `d407d9c` | 656 | 7628/8270 | 972 |
| per-hit Update still sorts a KO'd target | `0f53589` | 725 | 8005/8578 | 972 |
| no draws after the battle ends | `6105675` | 725 | 8005/8578 | 972 |
| first re-sort's getTargets in queue order | `d62c1f5` | 725 | 8005/8578 | 972 |
| mid-turn switch's Update and runSwitch sorts | `2cb9bbd` | 756 | 8256/8798 | 972 |
| Dire Claw / Tri Attack sample on a KO'd target | `7a8d248` | 775 | 8372/8895 | 972 |
| walk compares `random(m, n)` by span (tool) | `04f2125` | 775 | 8372/8895 | 972 |
| Glaive Rush (mechanics) | `151898c` | 785 | 8419/8932 | 980 |
| Trace's random pick (mechanics) | `de8a6dd` | 795 | 8513/9016 | 980 |
| a forced-out mon's cancelled move leaves the re-sort | `0519d45` | **804 (61.8%)** | **8572/9066** | **980** |

Every commit passed `cargo test --workspace --exclude vgc-engine-py`
with `ps-rng` off and on; the pyo3 tests pass (24).

Nothing was reverted. Earlier first draw divergences, all explained in the
commit messages:
- `5e78624` moved `827d3db65c` earlier (a Mega Floette under Trick Room;
  it is clean again after the following commits).
- `2eb92ea` moved 11 earlier that were aligned by chance: the missing
  BeforeSwitchOut Update and PS's tie-group order, fixed by `fe44631` and
  `58a236a`.
- `fe44631` moved 2 (p1-first Mega Evolution, fixed by `a254438`); `508ea83`
  moved 2 (status moves into Protect, fixed by `5a05ec4`); `0f53589` moved 2
  (draws after a battle-ending KO, fixed by `6105675`).
- Against round 5, only `1d4387d59e` diverges earlier in draws (the engine
  ends that battle a turn early, a state divergence), and only `b48d9ab083`
  diverges earlier in state: its draws already misalign on turn 1, and
  Trace's now-random pick lands on a different draw. No forced-RNG battle
  diverges earlier.

Remaining first divergent PS draws (362 battles; 192 more diverge in state
with every draw aligned to that point): `hitStepAccuracy` 99, runAction
`Update` 33, `getTarget` re-picks 32, `secondaries` 30, the ModifyDamage
handler sort 28, Residual handler sort 27, resolveAction `getRandomTarget`
27, crit 20. See [`ps-rng.md`](ps-rng.md) for why each is hard.

## Decisions for the owner

1. **Unimplemented moves.** Octolock (14 uses in the sample), Simple
   Beam (6), Entrainment (5), Worry Seed (1) and Magic Powder (1) have no
   handler: they do nothing and roll no accuracy. They are the largest
   known cause left in the `hitStepAccuracy` group. Each is its own
   mechanic; not done here.
2. **Quick Claw / Quick Draw draw position.** PS rolls them in
   `resolveAction`, before commitChoices' sort; the engine rolls them at
   queue build. Moving the draw changes the default stream too.

---

# Round 7 (2026-09-30, branch `mechanics-fixes-7`)

Same 1,298 / 1,300-battle sample and PS battles as rounds 5-6; the round-6
end reproduced exactly (forced-RNG 980 clean, 9691/10009 turns; seeded 804
clean, 8572/9066).

Result: forced-RNG fully clean **75.5% → 86.7%** (980 → 1125), per-turn
**96.8% → 98.4%** (9691/10009 → 10653/10826); seeded (`ps-rng`) fully clean
**804 → 974 (61.8% → 74.9%)**, turns 8572/9066 → 9801/10125. Against the
round-6 end, 157 forced-RNG battles improved and none regressed; 209 seeded
battles' first divergent draw moved later and none earlier.

## Owner-approved work

| commit | change | PS reference |
|---|---|---|
| `4decb05` | Simple (needed by Simple Beam) | `data/abilities.ts` simple `onChangeBoost` |
| `cefda5a` | Simple Beam | `data/moves.ts:16482`; `sim/pokemon.ts:1908` setAbility |
| `cdfe319` | Entrainment (gained ability's onStart runs) | `data/moves.ts:4859`; `sim/pokemon.ts:1943` |
| `7297108` | Worry Seed | `data/moves.ts:21050` |
| `d103e96` | Magic Powder | `data/moves.ts:10738`; `sim/battle-actions.ts:669` (powder) |
| `8afd40a` | Octolock (trap, residual Def/SpD drops, Ghost immunity) | `data/moves.ts:12960`; `data/mods/champions/moves.ts:703` |
| `7401247` | Dragon Darts: foe + its ally, one accuracy roll each; both hits on the survivor of a failed step | `data/moves.ts:4118`; `sim/pokemon.ts:757`; `sim/battle-actions.ts:607`; `data/mods/champions/scripts.ts:467` |
| `b99f0b8` | Quick Claw is +0.1 fractional priority (was +1 priority), rolls for any move; Quick Draw first | `data/items.ts:4989`; `data/abilities.ts:3735`; `sim/battle.ts:2647` |
| `ae654f6` | Quick Draw / Quick Claw rolled as the choices commit (resolveAction), before switches | `sim/battle-queue.ts:249` |

Each of the five moves fails in PS's hit-step order: onTryHit / onTryImmunity
before the accuracy roll (no roll), Substitute and onHit failures after it.

## Unimplemented mechanics in the M-C corpus

The scan matched every move, ability and item in the 3,000 sampled logs and
the 1,300 PS teams against the engine's handlers. Implemented this round
because they sat on first-divergence turns: Thermal Exchange (`bd7aa56`),
Rage Fist (`086f094`), always-crit moves (`0ed0029`), Feint / Phantom Force
breaking Protect (`0be918e`, `2a35fd6`), screen breaking (`960c122`),
Gooey / Tangling Hair (`2e8725e`), Rain Dish / Ice Body (`4c001b2`), Double
Shock (`47aa381`), Hard Press (`00b2795`), Ice Spinner (`3d2376e`), Seed
Sower (`00fd08a`), Curious Medicine (`3db7c84`), Roost's type loss
(`f8d0ec3`), Scrappy / Mind's Eye (`6af1241`; main's calc-fixes branch
landed the same fix, the merge keeps main's version).

Still missing (first-divergence co-occurrences in brackets, forced-RNG /
seeded): Stomping Tantrum (2/2, needs a move-result history), Last Resort
(3/3, needs per-slot used flags), Upper Hand (3/2, needs the target's queued
move priority), Imprison (3/3), Minimize's damage-doubling volatile (3/3),
Beak Blast (1), Charge the move (1), Bug Bite / Pluck (1), Shed Tail (1),
Topsy-Turvy, Memento, Fairy Lock, Acupressure, Thief, Lash Out, Assurance,
Payback, Water Shuriken's Ash form; abilities Frisk (no mechanical effect),
Flower Veil, Pickpocket, Innards Out, Gale Wings, Illusion, Surge Surfer;
the type gems (Normal Gem, 3 battles).

## Mechanics fixes (all builds), from the remaining divergences

| commit | bug | PS reference |
|---|---|---|
| `17696b5` | STAB counted the species' types after a retype (Protean) | `data/mods/champions/scripts.ts:233`; `sim/pokemon.ts` getTypes |
| `e764ebf` | Defiant / Competitive rebounded once per drop, not once per lowered stat; White Herb before them | `sim/battle.ts` boost (AfterEachBoost per stat); `data/items.ts:1712` |
| `21c0c0f` | Steel Beam / Mind Blown floored half max HP (PS rounds) | `data/moves.ts:17888`, `:11889` |
| `b13f44f` | Confusion self-hit roll inverted (min roll dealt max) | `sim/battle-actions.ts:1850` |
| `76104ce` | Solar Beam / Blade not halved in rain, sand, snow | `data/moves.ts:17249` |
| `4534ef1` | Partial-trap moves trapped the first foe, not the hit target | `data/conditions.ts` partiallytrapped |
| `29ef7ef` | Status-secondary table vs PS data (Matcha Gotcha 20%, Sludge Wave 10%, Zing Zap, 7 missing) | `data/moves.ts` `secondary` |
| `1ab3bc5` | Flinch table vs PS data (Zen Headbutt, Extrasensory, Hyper Fang, 4 missing ...) | same |
| `e4c05d3` | Missing stat-drop secondaries (Low Sweep, Fire Lash, ...) | same |
| `09cbf3c` | Between-turn replacements kept the switched-in marker for the next turn (no Speed Boost) | `sim/battle.ts:1765`; `data/abilities.ts` speedboost |
| `cf5ca13` | After Ally Switch a foe's move followed the Pokemon, not the target slot | `sim/battle.ts` getTarget / `sim/pokemon.ts` getAtLoc |
| `912fa41` | Prankster's Dark immunity required every foe to be Dark | `sim/battle-actions.ts:674` |

## `ps-rng`

| commit | change |
|---|---|
| `9acb843` | ModifyDamage handler-sort ties (two screens on the field, Friend Guard, items at equal Speed) |
| `dab377f` | Spread-hit window orders each step's draws by PS's target order (ally before foes) |

## Trajectory

| after | forced-RNG clean | turns | seeded clean | turns |
|---|---|---|---|---|
| round 6 end | 980 (75.5%) | 9691/10009 | 804 | 8572/9066 |
| five moves + Dragon Darts (`7401247`) | 992 | 9814/10120 | 813 | 8667/9152 |
| Quick Claw / Draw (`ae654f6`) | 992 | 9814/10120 | 813 | 8667/9152 |
| Thermal Exchange (`bd7aa56`) | 1003 | 9889/10184 | 822 | 8735/9211 |
| Rage Fist ... STAB (`17696b5`) | 1036 | 10076/10338 | 845 | 8879/9332 |
| ModifyDamage ties (`9acb843`) | 1051 | 10179/10426 | 883 | 9205/9620 |
| Defiant per stat (`e764ebf`) | 1059 | 10230/10469 | 889 | 9246/9655 |
| spread target order (`dab377f`) | 1061 | 10255/10492 | 914 | 9428/9812 |
| Steel Beam ... partial trap (`4534ef1`) | 1078 | 10347/10567 | 926 | 9503/9875 |
| Scrappy ... flinch table (`1ab3bc5`) | 1092 | 10429/10635 | 946 | 9617/9969 |
| replacement marker (`09cbf3c`) | 1106 | 10511/10703 | 958 | 9692/10032 |
| Ally Switch targets (`cf5ca13`) | 1119 | 10614/10793 | 968 | 9774/10104 |
| Prankster per target (`912fa41`) | 1124 | 10648/10822 | 973 | 9796/10121 |
| merge of main (`bb081a9`) | **1125 (86.7%)** | **10653/10826** | **974 (74.9%)** | **9801/10125** |

## Default RNG stream changes

Simple Beam, Entrainment, Worry Seed, Magic Powder, Octolock (new accuracy
rolls); Dragon Darts (second target's rolls); Quick Claw semantics and draw
position (owner-approved); Storm Throw / Flower Trick / Frost Breath (no crit
roll); Feint / Phantom Force (rolls against protected targets); Double Shock
(fails without draws); Hard Press (now damages); partial-trap duration draw
only when trapping; status / flinch table corrections (Zing Zap and the
added moves); replacement Moody holders roll a turn earlier; Prankster
per-target block. Commit messages say which.

## Guard exceptions and reverts

- `b19b5be` (allAdjacent targets ally-first in every build) broke the strict
  golden `corpus_zero_divergences` (`doubles-stealth-rock-chip`): the golden
  oracle pops PS's draws in order and PS rolls every target's accuracy
  first. Reverted in `8520bbf`; the same order is applied under `ps-rng`
  only (`dab377f`). `b19b5be` and `f8d0ec3` carry that failing golden.
- `b13f44f` through `e4c05d3` carry a failing `chance` feature test that
  pinned the inverted confusion roll; fixed in `c495bcc`.
- `29ef7ef` moved two seeded battles' first draw earlier (Zing Zap lost its
  paralysis roll, PS rolls a flinch); `1ab3bc5` restored them.
- `vgc-solver`'s `auto_lossy_off_preserves_full_lossless` failed once
  (`960c122`, ps-rng on) and passed on reruns: it reads a process-global
  counter that the unlocked `recursive.rs` tests also bump.
- No battle diverges earlier than at the round-6 end, in either mode.

Every other commit passed `cargo test --workspace --exclude vgc-engine-py`
with `ps-rng` off and on; the pyo3 tests pass (27) on the merge.

## Remaining (seeded: 319 battles whose first divergence is a draw)

`hitStepAccuracy` 87 (mostly where the engine draws a target re-pick first),
`eachEvent` ties 40, `getTarget` re-picks 37, Residual handler sort 26,
`secondaries` 22, resolveAction `getRandomTarget` 22, crit 21. 74 more diverge in state with draws aligned (45 HP, 9
boosts). Note the seeded walk compares draw kinds and spans, not values: a
target given another target's roll shows up as a state divergence (as the
spread-order fix did).

---

# Round 8 (2026-10-01, branch `mechanics-fixes-8`)

Same 1,298 / 1,300-battle sample and PS battles as rounds 5-7; the round-7
end reproduced exactly (forced-RNG 1125 clean, 10653/10826 turns; seeded
974 clean, 9801/10125).

Result: forced-RNG fully clean **86.7% → 88.1%** (1125 → 1144), per-turn
**98.4% → 98.6%** (10653/10826 → 10820/10974); seeded (`ps-rng`) fully
clean **974 → 990 (74.9% → 76.2%)**, turns 9801/10125 → 9925/10233. Against
the round-7 end, 20 forced-RNG battles improved and none regressed; 21
seeded battles' first divergent draw moved later and none earlier (state
divergences: 21 later, none earlier).

## Fixes

| commit | change | PS reference | tests |
|---|---|---|---|
| `4f617d9` | Per-mon move result (`moveThisTurnResult` / `moveLastTurnResult`); Stomping Tantrum and Temper Flare double after a failed move | `sim/battle-actions.ts:262, :274, :285, :371-374, :507, :616`; `sim/battle.ts:1674`; `sim/pokemon.ts:200-230, :1545`; `data/moves.ts:18050, :19186` | 7 unit; repro `stomping-tantrum-after-a-miss`; study `5a4e6093bf` (`e7b5f18`) |
| `77aeb9a` | Used move slots (`moveSlot.used`); Last Resort fails unless every other move was used since switch-in | `sim/pokemon.ts:892`; `sim/battle-actions.ts:139`; `data/moves.ts:10075` | 3 unit; study `b8f3217152` |
| `8fe4aff` | Upper Hand: fails unless the target queued a damaging move with priority > 0; 100% flinch | `data/moves.ts:20196`; `sim/battle.ts` getActionSpeed | 3 unit; repro `upper-hand-flinches-extreme-speed` |
| `30d040a` | Imprison (foes can't select or use the user's moves) | `data/moves.ts:9489` | 2 unit; study `00a2a3d4e1` |
| `abd5160` | Minimize's volatile: x2 and sure-hit for minimize-flagged moves (Body Slam, Heat Crash, Heavy Slam, Stomp ...) | `data/moves.ts:11926` | 2 unit; study `799492fe70` |
| `654e13b` | Thunder Wave: Ground types immune before the accuracy roll (engine paralyzed them) | `data/moves.ts:19601`; `sim/battle-actions.ts:654` | 1 unit |
| `d9bc676` | Shed Tail | `data/moves.ts:16161`; `sim/pokemon.ts:1246` | 2 unit; study `eaedb64b2a` |
| `658889b` | Bug Bite / Pluck eat the target's Berry | `data/moves.ts` bugbite / pluck | 2 unit; study `d2cd6086c6` |
| `22660f4` | Charge (the move): +1 SpD and the charge volatile; a status Electric move ends it | `data/moves.ts` charge | 1 unit; study `7dd9b58cae` |
| `4636d00` | Normal Gem | `data/items.ts:4324`; `data/conditions.ts:463` | 1 unit; study `45afe9c9b0` |
| `bc30a19` | Beak Blast's charge burns contact attackers | `data/moves.ts:1119`; `sim/battle-queue.ts:242`; `sim/battle.ts:2739` | 2 unit; repro `beak-blast-burns-contact` |
| `2c2086c` | `ps-rng`: the priority-charge action's `getRandomTarget` draw | `sim/battle-queue.ts:242, :266` | 1 `ps-rng` unit |
| `b446daf` | Payback doubles against a target that already acted | `data/moves.ts:13190` | 1 unit; study `574176a1f8` |
| `cfe66b0` | Self-heal rounding: `heal: [1, 2]` rounds; weather heals use `modify` | `sim/battle-actions.ts:1209`; `sim/battle.ts:2332` | 2 unit; study `c46a2e1300`; 3 old tests corrected |
| `2c2ef64` | onDamagingHit abilities read the current ability (Simple Beam'd Stamina kept firing) | `data/abilities.ts` stamina et al.; `sim/battle.ts` runEvent | 1 unit; study `aedae40015` |

## Decisions

- **What counts as a failed move.** PS sets the move result at many sites
  (every `return false` in the hit steps, onTry, BeforeMove). The engine
  records it at the same boundaries: Failed by default when a move action
  starts (BeforeMove and onTry failures: flinch, sleep, full paralysis,
  confusion self-hit, Sucker Punch, Fake Out ...); Skipped (PS `null`) for
  recharge, a charge turn, Future Sight, and a move whose every target was
  protected (Protect returns `NOT_FAIL`); damaging moves succeed when any
  target is hit. **Status moves** are the approximation: the engine's
  status handlers don't report success, so a status move succeeds when it
  changed any battle state (boosts, status, volatiles, HP, item, ability,
  types, side and field conditions, pending Wish / Future Sight / switches),
  ignoring PP, the choice lock and the stall counter. Magic-Bounced moves
  fail, protected ones are Skipped, the Protect family reads whether the
  user is protected, and Splash / Celebrate / Hold Hands / Haze always
  succeed. A status move the engine doesn't implement therefore counts as
  failed. The result persists one turn: it moves to the last-turn slot at
  the end of each turn and clears on switch-in.
- **Struggle under Encore (Champions).** Not changed: the Champions mod
  disables Fake Out / First Impression after the first move action
  (`data/mods/champions/moves.ts:352, :384`), so a mon Encored into Fake
  Out is forced to Struggle (4 forced-RNG battles: `6d4e9037cb`,
  `8b67530c3a`, `f75f638eca`, `acdf078ffc`). PS still runs the failing
  Fake Out on the turn right after the Encore and only then Struggles; the
  request-time rule behind that needs a closer look before changing
  `legal_choices`, which would also change the solver's choice sets.

## Default RNG stream changes

Last Resort, Upper Hand, Imprison, Thunder Wave (rolls skipped when they
fail); Upper Hand's flinch roll on a hit; Minimize (no accuracy roll for
minimize-flagged moves); Shed Tail (its user leaves mid-turn); Bug Bite's
stolen Starf Berry roll; the current-ability fix (a replaced or suppressed
contact-chance ability no longer rolls). Commit messages say which.
Stomping Tantrum, Charge, Normal Gem, Beak Blast, Payback and heal rounding
add or remove no draws.

## Guards

- Every commit passed `cargo test --workspace --exclude vgc-engine-py` with
  `ps-rng` off and on, run under the new resource cap (`-j 4`,
  `RUST_TEST_THREADS=4`). At 4 test threads `vgc-solver`'s
  `auto_lossy_off_preserves_full_lossless` fails on every run (the
  process-global counter race noted in round 7); it passes alone and with
  the crate single-threaded, which the test script reruns. The pyo3 tests
  pass (27).
- No battle diverges earlier than at the round-7 end, in either mode.
- One scripted golden was dropped: Thunder Wave into a Ground type replays
  clean even without the fix, because the keyed oracle answers an unmatched
  accuracy draw with a miss; the unit test checks that no roll is made.

## Still unimplemented / not fixed

- Illusion (no mechanical effect in PS's simulator beyond display; the one
  divergence is the log reconstruction).
- Frisk (no mechanical effect); Topsy-Turvy, Memento, Fairy Lock,
  Acupressure, Thief, Lash Out, Assurance, Water Shuriken's Ash form,
  Flower Veil, Pickpocket, Innards Out, Gale Wings, Surge Surfer: unused or
  never on a first-divergence turn in the sample.
- Struggle under Encore in Champions (above).
- Moody (8 forced-RNG battles) is RNG plumbing: the keyed oracle doesn't
  supply Moody's stat picks.
- Round's BP double writes the move-data copy that the damage calc doesn't
  read (`battle.rs` Round block); Payback uses the BP override seam instead.
  Not changed here.
- Seeded first divergent draws (306 battles): `hitStepAccuracy` 88,
  `getTarget` re-picks 35, `eachEvent` ties 33, `fieldEvent` ties 25,
  resolveAction `getRandomTarget` 24, `secondaries` 21, crit 20, stall
  rolls 12.

## Trajectory

| after | forced-RNG clean | turns | seeded clean | turns |
|---|---|---|---|---|
| round 7 end (`45fe4e2`) | 1125 (86.7%) | 10653/10826 | 974 | 9801/10125 |
| Stomping Tantrum (`4f617d9`) | 1126 | 10655/10827 | 975 | 9803/10126 |
| Last Resort (`77aeb9a`) | 1128 | 10688/10858 | 977 | 9821/10142 |
| Upper Hand (`8fe4aff`) | 1131 | 10711/10878 | 978 | 9833/10153 |
| Imprison (`30d040a`) | 1132 | 10728/10894 | 978 | 9840/10160 |
| Minimize (`abd5160`) | 1133 | 10748/10913 | 979 | 9860/10179 |
| Thunder Wave (`654e13b`) | 1133 | 10748/10913 | 979 | 9860/10179 |
| Shed Tail (`d9bc676`) | 1135 | 10758/10921 | 981 | 9870/10187 |
| Bug Bite (`658889b`) | 1136 | 10770/10932 | 982 | 9882/10198 |
| Charge (`22660f4`) | 1137 | 10777/10938 | 983 | 9889/10204 |
| Normal Gem (`4636d00`) | 1138 | 10785/10945 | 983 | 9892/10207 |
| Beak Blast + `ps-rng` draw (`2c2086c`) | 1138 | 10785/10945 | 984 | 9895/10209 |
| Payback (`b446daf`) | 1139 | 10788/10947 | 985 | 9898/10211 |
| heal rounding (`cfe66b0`) | 1143 | 10812/10967 | 988 | 9910/10220 |
| current ability on hit (`2c2ef64`) | **1144 (88.1%)** | **10820/10974** | **990 (76.2%)** | **9925/10233** |

---

# Round 9 (2026-10-01, branch `mechanics-fixes-9`)

Same 1,298 / 1,300-battle sample and PS battles as rounds 5-8; the round-8
end reproduced exactly (forced-RNG 1144 clean, 10820/10974 turns; seeded
990 clean, 9925/10233).

Result: forced-RNG fully clean **88.1% → 96.3%** (1144 → 1250), per-turn
**98.6% → 99.6%** (10820/10974 → 11462/11510); seeded (`ps-rng`) fully
clean **990 → 1123 (76.2% → 86.4%)**, turns 9925/10233 → 10725/10900.
Against the round-8 end, 111 forced-RNG battles improved and none
regressed; 170 seeded battles' first divergent draw moved later and none
earlier.

## Triage of the 154 diverged forced-RNG battles

Each battle's first divergence was traced to a cause (four parallel triage
passes over `show.sh`, the PS log and the PS source). Buckets, largest
first:

| bucket | kind | battles | status after round 9 |
|---|---|---|---|
| leads' switch-in handlers run p1a, p1b, p2a, p2b, not by Speed (Surge / weather setters, White Herb) | mechanic | 18 | 14 fixed (`1348ab3`); 3 White Herb + 1 terrain remain |
| keyed speed ties: the engine consumed only the commit shuffle and on its own move list | harness | 22 | 20 fixed (`77947df`, `c30f490`) |
| a move's checks read the chosen target, not the retargeted / redirected one (Sucker Punch 9; status moves: Good as Gold, Magic Bounce, Follow Me, a fainted ally 5) | mechanic | 14 | fixed (`57e10c8`, `64bc495`) |
| abilities and their RNG picks the keyed oracle could not supply: Moody 8, Trace 6 | RNG plumbing | 14 | 12 fixed (`2941fce`, `2350053`) |
| replaced abilities: a gained ability's onStart (4), Trace / Mummy surviving switch-out (3), reads of the base ability (2) | mechanic | 9 | fixed (`419d6f4`, `d7a25a8`, `91822e3`) |
| other draws keyed wrongly: `random(m, n)` stored with `m`, paralysis / Attract gates under a stale or shared key, confusion length | harness | 8 | 7 fixed (`edb0981`, `9e8c609`, `dfd5fea`) |
| repair-pass aliasing (clean in the seeded run) | harness | 6 | all clean now (downstream of the tie / key fixes) |
| Toxic damage `floor(hp × n / 16)` instead of `floor(hp / 16) × n` | mechanic | 6 | 5 fixed (`a047558`) |
| Explosion / Self-Destruct never fainted the user | mechanic | 4 | fixed (`49c7e82`) |
| partial trap / Leech Seed kept going after the source left or fainted | mechanic | 4 | fixed (`3d869ce`) |
| multi-hit: per-hit damage from move-start snapshots; kept hitting after the user fainted | mechanic | 4 | 2 fixed (`137f489`) |
| Knock Off removed Rocky Helmet before it fired | mechanic | 3 | fixed (`0601d19`) |
| pairs: Lum Berry vs confusion, Friend Guard mid-spread, Sitrus after sand chip, Weather Ball vs Mega Sol, Transform's moves, Trick vs Mega Stone, Toxic Debris hit by an ally, Surge Surfer, Parental Bond | mechanic | 18 | Lum Berry fixed (`70a0d35`); 16 remain |
| one each: Mirror Armor → Competitive, Champions Encore retarget, Aromatic Mist, Solar Power order, Mental Herb vs Disable, Stone Axe + Life Orb KO, Shield Dust vs Fake Out, Life Orb vs Disguise, Sap Sipper / Soundproof / Oblivious status-move gates, Burning Jealousy, Harvest, Sitrus on switch-in | mechanic | 14 | remain |
| duplicate species name in the log (Palafin and Palafin-Hero) | harness | 2 | remain |
| Struggle under Encore (Champions Fake Out) | out of scope | 4 | remain (owner decision) |
| unexplained at triage | unknown | 4 | all clean now |
| **total** | | **154** | **106 fixed, 48 remain** |

## Fixes

| commit | change | PS reference | tests |
|---|---|---|---|
| `2941fce` | Moody's picks keyed as ability draws; the harness recovers residual ability `sample()`s from the raw trace | `data/abilities.ts:2701` | study `e57f4e89e3` |
| `5d53be7` | `ps-rng`: a mon locked into a two-turn move draws no resolveAction target | `sim/side.ts:675-688` | `ps-rng` unit |
| `1348ab3` | Leads' SwitchIn handlers in Speed order, items after abilities | `sim/battle-actions.ts:172-184` | unit; study `047cc41478` |
| `57e10c8` | Sucker Punch / Upper Hand check the retargeted / redirected target | `sim/battle.ts:2437-2487`; `data/moves.ts` suckerpunch | unit; studies `0355ae7c44`, `e5bcb1085f` |
| `a047558` | Toxic floors the sixteenth before the stage multiply | `data/conditions.ts:159` | unit (one old test corrected); study `acb2e577ac` |
| `2350053` | Trace's pick keyed as an ability draw (battle start included) | `data/abilities.ts:5143` | study `43b96379ab` |
| `419d6f4` | Skill Swap / Role Play / Trace run the gained ability's onStart | `sim/pokemon.ts:1943`; `sim/battle.ts:1311` | 2 unit; study `7e8cd717ef` |
| `d7a25a8` | Trace / Mummy / Wandering Spirit set the current ability (lost on switch-out); Intimidate, Defiant, Regenerator, Poison Touch read it | `sim/pokemon.ts` setAbility, clearVolatile | 2 unit; studies `3517310941`, `7e05dfe023` |
| `91822e3` | Hit-time ability checks and the damage calc read the current ability | `data/abilities.ts` soundproof, waterbubble ... | unit; study `5111885ee2` |
| `64bc495` | Single-target status moves: retarget, keep a fainted ally (fail), redirect; Magic Bounce only on its holder | `sim/battle.ts:2437-2487`; `data/moves.ts` followme | 2 unit; studies `53a95d56c8`, `bc20b85092` |
| `49c7e82` | Explosion / Self-Destruct / Misty Explosion faint the user before the hit | `sim/battle-actions.ts:500` | unit; study `4800ac3963` |
| `527363a` | Protect blocks before Magic Bounce reflects | `data/abilities.ts` magicbounce onTryHitPriority | unit; study `97bfceddd1` |
| `3d869ce` | Partial trap ends without its source; Leech Seed needs a live seeder slot | `data/conditions.ts:238`; `data/moves.ts` leechseed | 2 unit; studies `e81e74426e`, `2a77dad1ee` |
| `77947df` | Keyed oracle: PS's tie shuffles replayed in the gen-8+ re-sorts | `sim/battle.ts` runAction queue.sort | study `c1480b7942` |
| `c30f490` | Keyed oracle: PS's commitChoices sort on PS's action list, before switches and Megas | `sim/battle.ts` commitChoices, comparePriority :404 | studies `2aac00140c`, `4338912265` |
| `0601d19` | Hit order: DamagingHit, Knock Off, pinch berries, Red Card / Eject Button | `data/mods/champions/scripts.ts` spreadMoveHit | 2 unit; study `4731c6ae5a` |
| `edb0981` | Harness: two-argument `random(m, n)` keyed as offsets from `m` | `data/conditions.ts` confusion onStart | study `12ea08df40` |
| `9e8c609` | Keyed oracle: the full-paralysis gate drawn apart from the move's range key | `data/mods/champions/conditions.ts:5` | study `fb2bf66e3b` |
| `dfd5fea` | Keyed oracle: the infatuation gate under its own context | `data/moves.ts:706` | study `907733b22f` |
| `137f489` | Multi-hit: live boosts / status per hit; stop when the user faints | `data/mods/champions/scripts.ts` hitStepMoveHitLoop | studies `1b664d1de5`, `622d179172` |
| `70a0d35` | Lum Berry cures confusion; Persim / Lum checked where confusion lands | `data/items.ts` lumberry | unit; study `59e92f3ccd` |
| `68d5440` | `ps-rng`: the first re-sort's getTarget reads a locked move's target | `sim/battle.ts` getActionSpeed | `ps-rng` unit |
| `308b481` | `ps-rng`: Champions Curse's getTarget (self / tracksTarget) | `data/mods/champions/moves.ts` curse | `ps-rng` unit |
| `9a8b075` | `ps-rng`: tied faint replacements draw insertChoice's tie | `sim/battle-queue.ts` insertChoice | `ps-rng` unit |
| `5de743e` | `ps-rng`: a recharging mon's 'recharge' action redraws a random foe in every getTarget | `sim/battle.ts:2437-2533`; `sim/battle-actions.ts:209` | `ps-rng` unit |

## Default RNG stream changes

`57e10c8` (Sucker Punch / Upper Hand hit or fail differently), `d7a25a8`
(Poison Touch on a replaced ability; a reverted ability's procs),
`91822e3` (absorb / immunity / Skill Link on a current ability),
`64bc495` (retargeted and redirected status moves), `49c7e82` (the user
leaves), `527363a` (no bounced copy), `0601d19` (helmet damage first;
berries such as Starf roll after the contact procs), `137f489` (fewer
hits after a user KO), `70a0d35` (a cured confusion rolls no self-hit),
and `1348ab3` through order only (a lead's Trace pick). Toxic, Trace /
Skill Swap onStart, partial trap and Leech Seed change HP or field state
but add or remove no draw sites. The keyed-oracle and `ps-rng` commits
leave the default stream alone. Commit messages say which.

## Decisions

- **Keyed-only code in the engine.** The keyed oracle now replays PS's
  commitChoices sort (`keyed_commit_sort`) and every re-sort's tie
  shuffles, and draws the paralysis and Attract gates under a context no
  PS draw carries (`u16::MAX - 4`). These run only under
  `Rng::OracleKeyed` (`is_oracle_keyed()`); SplitMix battles keep
  `shuffle_tie_groups` and the stable re-sort. The alternative was to
  regenerate the 1,300 PS battles with a driver that keys these draws;
  that changes the dataset, so it was not done.
- **Moody and Trace samples.** `ps-battle.js` routes `Battle.sample`
  through `Battle.random` from its own wrapper, so the envelope loses the
  ability frame. The harness recovers residual and onUpdate ability
  samples from the raw trace (`add_ability_samples`) rather than changing
  the driver, for the same reason.
- **Pinch berries after the DamagingHit reactions.** Champions'
  spreadMoveHit runs DamagingHit, then AfterHit, then the hit loop's
  Update; the engine now follows that order, so a target's Sitrus is
  eaten after Rough Skin / Rocky Helmet resolve on the attacker.
- **Gates through the guard.** `64bc495` exposed `97bfceddd1` (Magic
  Bounce used to look at the first live foe, which hid Protect's
  priority); fixed in `527363a`. `77947df` put 7 previously clean
  battles out of order (the commit shuffle was replayed on the engine's
  own list); fixed in `c30f490`. A first version of `5de743e` moved 14
  seeded battles earlier (it left out runMove's getTarget for the
  recharge action); the amended commit moves none earlier.

## Guards

- `cargo test --workspace --exclude vgc-engine-py` passes with `ps-rng`
  off and on at the branch head, under the resource cap (`-j 4`,
  `RUST_TEST_THREADS=4`). `vgc-solver`'s
  `auto_lossy_off_preserves_full_lossless` fails at 4 threads as before
  and passes single-threaded (the test script reruns it).
- Harness runs every 2-3 fixes; no battle diverges earlier than at the
  round-8 end in either mode.
- `tools/audit-residual-index/audit.sh` is clean.

## Still unimplemented / not fixed (48 forced-RNG battles)

- Mechanics (37): White Herb at battle start after both Intimidates (3),
  Friend Guard for the second target after its holder faints mid-spread,
  Sitrus straight after sand chip (weather's Update), Weather Ball under
  Mega Sol, Transform / Imposter copying moves, Trick refusing a Mega
  Stone, Toxic Debris hit by an ally, Surge Surfer (needs the terrain in
  `effective_speed`, 39 call sites), Parental Bond (2 each); one each:
  Mirror Armor's reflected drop triggering Competitive, the Champions
  Encore retarget, Aromatic Mist, Solar Power's residual order, Mental
  Herb vs Disable, Stone Axe after a Life Orb KO, Shield Dust vs Fake
  Out's flinch, Life Orb after a Disguise hit, Sap Sipper / Soundproof /
  Oblivious blocking status moves, Burning Jealousy, Harvest, Sitrus on
  switch-in, multi-hit damage after a mid-move burn (2), a Psychic
  Terrain lead order (1), and one former Toxic battle that now diverges
  later on a stat boost (1).
- Harness (5): a pre-turn switch tie and a Gravity / switch tie order
  (2), the Infestation duration on one battle, Palafin / Palafin-Hero
  naming (2).
- RNG plumbing (2): a Moody holder at the battle-start residual, one
  Trace pick.
- Out of scope: Struggle under Encore (4).
- Seeded first divergent draws (151 battles; 24 more diverge in state with
  draws aligned): a random target PS draws that the engine doesn't (15,
  mostly drag-in and residual Update ties), resolveAction / getTarget
  random targets vs accuracy (18), eachEvent ties (20), fieldEvent ties
  (12), commitChoices sort ties (11), secondaries (16), crit and stall
  rolls (the rest).

## Trajectory

| after | forced-RNG clean | turns | seeded clean | turns |
|---|---|---|---|---|
| round 8 end (`d3c5f71`) | 1144 (88.1%) | 10820/10974 | 990 | 9925/10233 |
| Moody, locked targets, lead order (`1348ab3`) | 1164 | 11003/11137 | 1001 | 10047/10344 |
| Sucker Punch, Toxic (`a047558`) | 1179 | 11049/11168 | 1015 | 10093/10376 |
| Trace pick, gained onStart, current ability (`d7a25a8`) | 1191 | 11104/11211 | 1024 | 10126/10400 |
| ability reads, status targets, Explosion (`49c7e82`) | 1201 | 11177/11274 | 1034 | 10201/10465 |
| Magic Bounce / Protect, trap, keyed re-sort ties (`77947df`) | 1220 | 11279/11357 | 1039 | 10226/10485 |
| keyed commit sort, hit order (`0601d19`) | 1239 | 11385/11444 | 1042 | 10247/10503 |
| `random(m, n)`, paralysis / Attract keys (`dfd5fea`) | 1246 | 11435/11487 | 1042 | 10247/10503 |
| multi-hit, Lum Berry (`70a0d35`) | 1250 | 11462/11510 | 1045 | 10272/10525 |
| `ps-rng` locked target, Curse (`308b481`) | 1250 | 11462/11510 | 1099 | 10599/10798 |
| `ps-rng` replacements, recharge (`5de743e`) | **1250 (96.3%)** | **11462/11510** | **1123 (86.4%)** | **10725/10900** |

---

# Round 10 (2026-10-01, branch `mechanics-fixes-10`)

Same 1,298 / 1,300-battle sample and PS battles as rounds 5-9; the round-9
end reproduced exactly (forced-RNG 1250 clean, 11462/11510 turns; seeded
1123 clean, 10725/10900).

Result: forced-RNG fully clean **96.3% → 99.2%** (1250 → 1288), per-turn
**99.6% → 99.9%** (11462/11510 → 11735/11745); seeded (`ps-rng`) fully
clean **1123 → 1164 (86.4% → 89.5%)**, turns 10725/10900 → 11023/11157.
Against the round-9 end, 40 forced-RNG battles improved and none
regressed; 49 seeded battles' first divergent draw moved later and none
earlier.

## Owner decisions carried out

- **Oracle-only engine branches kept** (keyed commit sort, keyed re-sort
  tie shuffles, the `u16::MAX - 4` paralysis / Attract gate context).
  Documented as test-oracle-only on `Rng::is_oracle_keyed`, in
  `docs/conformance-key-contract.md` ("Oracle-only engine branches") and
  linked from `ps-rng.md` (`abe77b4`).
- **Struggle under Encore (Champions).** `legal_choices` applies the
  Champions `onDisableMove` for Fake Out / First Impression
  (`data/mods/champions/moves.ts:352, :384`): once the mon has started a
  move action since switching in, the move is unselectable, so an Encored
  Fake Out user gets Struggle only (`8d00851`). This is PS's general rule,
  so **the solver's choice sets change for every Champions battle**: Fake
  Out / First Impression drop out after the first action, not only under
  Encore. vgc-solver's tests pass. The keyed harness reads PS's `move 1`
  as Struggle when the engine offers only Struggle (`c4268bb`) and aims it
  at PS's logged target (`abed19a`). Three Struggle bugs surfaced:
  recoil rounding (`073f844`), Protect not blocking randomNormal moves
  (`3faad43`, also Outrage / Thrash / Petal Dance / Raging Fury), and
  Struggle's typelessness (`a9c20a9`).
- **Surge Surfer** (`43697b3`): `order::effective_speed` takes the
  terrain; every caller passes its battle's terrain, tests and
  `calc::speed_tier` pass `Terrain::None`. Public API change:
  `vgc-winrates/engine/src/main.rs` calls the 3-argument form and needs
  `Terrain::None` appended when its ENGINE_REF moves past this merge.

## Triage of the 48 diverged forced-RNG battles

Each round-9 divergence, by first-divergence cause:

| cause | battles | status after round 10 |
|---|---|---|
| Struggle under Encore (+ recoil rounding, Protect, typeless) | 4 | fixed |
| Friend Guard for the second spread target after its holder faints | 3 | fixed |
| Sap Sipper / Soundproof / Oblivious vs status moves | 4 | 3 fixed; Oblivious `7bd3433b14` now diverges later (untriaged last-turn Earthquake) |
| White Herb after both lead Intimidates | 2 | fixed |
| Transform / Imposter moves | 2 | fixed |
| Trick vs a Mega Stone holder | 2 | fixed |
| Parental Bond | 2 | fixed |
| Toxic Debris hit by an ally | 2 | fixed |
| one each: Surge Surfer, Mirror Armor → Competitive, Mega Sol, Aromatic Mist, Solar Power order, Mental Herb vs Disable, Stone Axe + Life Orb KO, Shield Dust vs Fake Out, Life Orb vs Disguise, Burning Jealousy, Harvest, Sitrus on switch-in, Sitrus after sand chip, Healer before burn | 14 | fixed |
| Sticky Web → Defiant | 1 | fixed; `7d354ee6d9` now diverges later on the Palafin naming issue |
| cleared as a side effect of the fixes above | 3 | fixed (`33238f5ca3`, `9de74b3c92`, `ddfb361dce`) |
| Palafin and Palafin-Hero on one team (`ps-battle.js` remaps switches by name) | 2 | remain (harness) |
| pre-turn switch tie, Psychic Terrain lead order | 2 | remain (harness) |
| Champions Encore's retarget draw (no keyed context) | 1 | remain (harness) |
| Infestation duration | 1 | remain (harness) |
| Red Card / Emergency Exit replacement pick | 1 | remain (decision model) |
| Trace pick | 1 | remain (RNG plumbing) |
| **total** | **48** | **38 fixed, 10 remain** |

## Fixes

| commit | change | PS reference | tests |
|---|---|---|---|
| `8d00851` | Champions: Fake Out / First Impression unselectable after the first move action | `data/mods/champions/moves.ts:352, :384`; `sim/pokemon.ts:1109` | 2 unit |
| `073f844` | Struggle recoil `round(maxhp / 4)` | `sim/battle-actions.ts:1381` | unit; study `6d4e9037cb` |
| `3faad43` | Protect blocks randomNormal moves | `data/moves.ts` struggle / outrage flags; protect onTryHit | unit |
| `a9c20a9` | Struggle is typeless | `data/moves.ts` struggle onModifyMove | unit; study `8b67530c3a` |
| `43697b3` | Surge Surfer (terrain into `effective_speed`) | `data/abilities.ts:4755` | unit; study `0861359e14` |
| `d2899b4` | Toxic Debris hit by an ally: foes' side | `data/abilities.ts:5106` | unit; study `11b4f8b12c` |
| `ea61c28` | White Herb waits for every batched switch-in ability | `data/items.ts:7692` | unit; studies `1aca10e8ce`, `7fb33f269d` |
| `7851499` | Mirror Armor's bounce runs the source's drop reactions | `data/abilities.ts` mirrorarmor | unit; study `1c4ec0648a` |
| `3a6faee`, `ffe91cf` | Mega Sol: Weather Ball Fire 100; Solar Beam / Blade no charge | `sim/pokemon.ts:2193`; `data/moves.ts:17238, :20701` | 2 unit; study `4598041866` |
| `be1a431` | Sap Sipper / Soundproof / Oblivious block status moves | `data/abilities.ts` sapsipper, soundproof, oblivious | unit; 4 studies |
| `c76d9ba` | Friend Guard holds for the whole spread hit | `sim/battle-actions.ts` spreadMoveHit | unit; 3 studies |
| `ec42536` | HP berries right after sand chip | `data/conditions.ts` sandstorm onFieldResidual | unit; study `252c896fb8` |
| `67cde90` | Trick fails against its own Mega Stone | `data/moves.ts` trick; `data/items.ts` mega stones | unit; studies `4bb7820163`, `6a62ab1faf` |
| `fa9f0c7`, `ff17229` | Mental Herb cures Disable; Disable lasts 5 on a target that already moved | `data/items.ts` mentalherb; `data/moves.ts:3664` | 2 unit; study `9af2fea75f` |
| `d1f5f01` | Life Orb after a Disguise-only hit | `sim/battle-actions.ts:536` | unit; study `e5af60d10c` |
| `e756973` | Burning Jealousy (`stats_raised_this_turn`) | `data/moves.ts` burningjealousy | unit; study `63ba3973ec` |
| `24e7686` | Solar Power / Dry Skin in the weather step | `data/abilities.ts:4403` | unit; study `966745efb4` |
| `ce5b0e3` | HP berries on switch-in | `data/items.ts` sitrusberry onUpdate | unit; study `a6f2f065f3` |
| `7964fbb` | Sticky Web's drop runs Defiant / Competitive / Eject Pack | `data/moves.ts` stickyweb | unit; study `7d354ee6d9` |
| `24eb9ff` | Aromatic Mist | `data/moves.ts` aromaticmist | unit; study `6d4e9037cb` |
| `eec53e8` | Stone Axe / Ceaseless Edge before Life Orb | `data/moves.ts` stoneaxe onAfterHit | unit; study `b00eb62aee` |
| `f25f47d` | Transform / Imposter copy moves; switch-out reverts | `sim/pokemon.ts:1305-1326` | unit; studies `5abc255afc`, `ae0f734f2d` |
| `990f645` | Parental Bond | `data/abilities.ts` parentalbond; `data/mods/champions/scripts.ts:209` | unit; studies `91790f18a3`, `d17595827b` |
| `fa7edd4` | Shield Dust filters target secondaries | `data/abilities.ts` shielddust | unit; study `e33011c991` |
| `c60f758` | Harvest | `data/abilities.ts:1800` | unit; study `7adc8cfb39` |
| `423b27e` | Healer / Hydration / Shed Skin at residual order 5 | `data/abilities.ts:1817` | unit; study `0ecfd594d5` |
| `3d1c44c` | Status moves sure-hit vs Glaive Rush / No Guard | `data/moves.ts` glaiverush onAccuracy | unit; seeded study `4111a15689` |

Unit tests for round 10 live in `crates/vgc-engine-core/src/battle_r10_tests.rs`
(and `damage.rs` / `order.rs` for the damage and speed ones).

## Default RNG stream changes

`3faad43` (blocked randomNormal moves roll nothing), `a9c20a9` (Struggle
into a Ghost rolls), `43697b3` (order, Surge Surfer in Electric Terrain),
`7851499` and `7964fbb` (Eject Pack can now switch), `ffe91cf` (Solar
Beam fires a turn earlier), `be1a431` (blocked status moves roll
nothing), `ec42536`, `24e7686` and `ce5b0e3` (a Starf roll's timing),
`e756973` (Burning Jealousy's secondary roll), `f25f47d` (copied moves),
`990f645` (Parental Bond's second crit / damage roll), `fa7edd4` (no
secondary rolls into Shield Dust), `c60f758` (Harvest's roll),
`423b27e` (cure rolls move earlier), `3d1c44c` (no status accuracy roll
vs Glaive Rush / No Guard). The rest change HP, items or field state
only. Commit messages say which.

## Decisions

- **Fake Out's rule applies everywhere in Champions**, not just under
  Encore (see above): PS has one rule, and an Encore-only version would
  offer illegal choices.
- **Harness limits worked around in the harness, not the engine:** a
  Struggle choice and its target come from PS's request shape and log
  (keyed mode only; the seeded replay lets the engine draw). Draws PS
  makes with no attributable context (Champions Encore's retarget,
  resolveAction's random targets) are not keyed; fixing that needs a
  driver change and a regenerated dataset.
- **Transform state** is a new `Pokemon::transform_base` (species,
  ability, stats, moves, PP), hashed by `canonical_hash`.
- **Parental Bond's 0.25** rides a new `DamageContext::parental_bond_hit`;
  the struct literals in tests and `vgc-engine-replay` were updated
  mechanically.

## Guards

- `cargo test --workspace --exclude vgc-engine-py` passes with `ps-rng`
  off and on at every checkpoint (-j 4, `RUST_TEST_THREADS=4`), except
  vgc-solver's known `auto_lossy_off_preserves_full_lossless` counter
  race, which passes single-threaded (the script reruns it). The script
  now runs `cargo test --no-fail-fast` so a failing crate can't hide
  later ones.
- Ten harness checkpoints; no forced-RNG battle regressed and no seeded
  first divergent draw moved earlier against the round-9 end.
- `tools/audit-residual-index/audit.sh` is clean.

## Still not fixed (10 forced-RNG battles)

- Harness (7): Palafin and Palafin-Hero on one team (3; `ps-battle.js`
  remaps switch targets by species name), a pre-turn switch tie and a
  Psychic Terrain lead order (2), Champions Encore's retarget draw (1),
  the Infestation duration (1).
- Trace pick (1, RNG plumbing), a Red Card / Emergency Exit replacement
  pick (1, decision model), and one untriaged last-turn Earthquake (1).
- Seeded (`ps-rng`): 136 battles diverge, 131 first on a draw.
  `eachEvent` ties 20, resolveAction random targets 19, `secondaries` 18,
  `fieldEvent` ties 14, commitChoices sort ties 14, accuracy 13,
  getTarget random targets 9, crit 8, Champions sleep length 4, the rest
  1-3 each. Example: `0fd2fb8dad` — a fainted user's queued move still
  draws getTarget in the post-move re-sort.

## Trajectory

| after | forced-RNG clean | turns | seeded clean | turns |
|---|---|---|---|---|
| round 9 end (`6d8cb99`) | 1250 (96.3%) | 11462/11510 | 1123 | 10725/10900 |
| Struggle under Encore (`abed19a`) | 1253 | 11494/11539 | 1125 | 10748/10921 |
| Surge Surfer, Toxic Debris, White Herb, Mirror Armor | 1261 | 11544/11581 | 1133 | 10802/10967 |
| Mega Sol, status-move abilities | 1266 | 11593/11625 | 1136 | 10842/11004 |
| Friend Guard, sand berries, Trick, Mental Herb | 1273 | 11633/11658 | 1143 | 10880/11035 |
| Life Orb / Disguise, Burning Jealousy, Disable | 1276 | 11646/11668 | 1144 | 10887/11041 |
| Solar Power order, switch-in berries | 1278 | 11657/11677 | 1145 | 10892/11045 |
| Sticky Web, Aromatic Mist, Stone Axe | 1281 | 11683/11700 | 1147 | 10909/11060 |
| Transform, Parental Bond | 1285 | 11712/11725 | 1151 | 10938/11085 |
| Shield Dust, Harvest | 1286 | 11721/11733 | 1157 | 10985/11126 |
| Healer order, sure-hit status (`3d1c44c`) | **1288 (99.2%)** | **11735/11745** | **1164 (89.5%)** | **11023/11157** |

## Harness fix: switch targets remapped by PS object, not species

This is a harness bug, not an engine bug. It covers the "Palafin and
Palafin-Hero on one team" item above.

- **Cause.** `ps-battle.js` turned PS's `switch N` (current side order)
  into the engine's original-order index by `names.indexOf(baseKey(...))`.
  That relied on Species Clause making base species unique. Reconstructed
  and scripted jobs can break that rule (Palafin + Palafin-Hero), so both
  members collapsed onto the first one.
- **Reproduction.** The parent ran this on official PS `a5df8274e85b0889bf2a9b3422a08b39732374fc`,
  built from `smogon/pokemon-showdown`, with a tiny synthetic Champions
  doubles job. P1 = [Palafin, Palafin-Hero, Snorlax, Chansey]. On turn 1,
  `move 1, switch 3` swaps Hero (slot b) for Snorlax. On turn 2, the same
  command brings Hero back from PS index 3. PS accepted every choice with
  no errors, and turn 2 ended with p1b = `palafinhero`. The recorded engine
  choice was `move 1, switch 1` instead of `move 1, switch 2`. The keyed
  replay's first divergence was turn 2 / p1b species (engine `palafin` vs
  PS `palafinhero`), not RNG-sensitive.
- **Fix.** The new `tools/accuracy/switch-order.js` captures each side's PS
  Pokemon objects right after `>player`, before team preview can reorder
  them. It maps `switch N` to the original index of the object
  `side.pokemon[N-1]` by identity. All three call sites use it: the main
  choice, mid-turn / replacement, and the invalid-choice retry. Each target
  is checked against the request's ident/details, so a stale order throws
  instead of mapping wrong. PS commands, output schema and RNG are
  unchanged. Intent-driven picks (`switchTo`) still choose by species.
- **Tests.** `tools/accuracy/switch-order.test.js` runs an equivalent in-repo
  fixture on real PS (`PS_DIST`) and pure identity cases (identical
  nicknames, forme / Transform changes, invalid targets). The PS case
  failed on the old mapper with `move 1, switch 1` and passes now. The
  corpus has not been rescored.

## Harness fix: a finite `--max-turns` recorded one unplayed turn

This is a harness bug, not an engine bug. It is separate from the Palafin
fix.

- **Cause.** When `|turn|N+1` appears (N = `maxTurns`), the omniscient
  drain writes `>forcetie`. Before this fix, both `driveSide`s had already
  logged their turn N+1 main choices. PS ended the battle with `|tie`
  before playing them, but assembly still emitted turn N+1. That turn
  carried those commands and a state snapshot from before any of its
  actions. The keyed replay then ran the unplayed moves and reported
  turn N+1 HP / faint divergences and missing draws.
- **Reproduction.** The parent ran the 7 `repros/repros.jsonl` jobs on
  official PS `a5df8274…` with `--max-turns 2`. PS accepted every choice
  with no errors. The native outputs had turns 1-3 and 17/21 turns
  matched. All 4 state divergences were on turn 3, and 6 jobs had missing
  draws from that turn. Copies clipped to turns ≤ 2 matched all 14/14
  state snapshots with no missing draws, so no engine defect was shown.
  Some existing repros still use leftover-draw and choice-repair heuristics;
  this is not strict RNG-trace equivalence.
- **Fix.** `driveSide` now leaves a request unanswered when its turn is
  past `maxTurns`, so PS ties at the turn marker and nothing of that turn
  runs or is recorded. Assembly also drops turns past `maxTurns`.
  End-of-turn replacements still carry the old turn number and are kept.
  With no bound (undefined / Infinity), the cutoff never applies, as
  before. `_meta.lastTurn` still reports the cutoff marker (N+1); it is
  not a played turn.
- **Tests.** `tools/accuracy/turn-limit.test.js` runs on real PS
  (`PS_DIST`). With limits 1 and 2, the protocol shows exactly N played
  turns and then `|tie`. The output has turns 1..N with their commands
  and exact end HP (Seismic Toss, 50 per turn), and no errors. Before the
  fix, the output had N+1 turns. Two more cases cover a battle that ends
  naturally on turn 1, with limits 1 and 3; its final turn is kept. Fresh,
  unmodified outputs for the 7 repros now contain exactly turns 1-2:
  14/14 state snapshots match, with no missing draws or PS/engine errors.
  The quiet Palafin fixture also matches 2/2 turns with no missing or
  leftover draws and no repairs. The full study corpus has not been
  regenerated or rescored.

## Scorer fix: the last turn before a `--max-turns` cutoff is compared in full

This is a validation gap in the scorer, not an engine fix. It follows from
the cutoff fix above.

- **Gap.** `drive()` (`accuracy.rs`) compares only faints on the last
  recorded state turn whenever `_meta.ended` is set. That is meant for a
  battle PS ended mid-turn, where the engine still runs the turn's
  residuals. A bounded run also sets `ended` (the forced `|tie`), and since
  the cutoff fix it records only turns 1..N. So the fully played turn N was
  compared on faints alone, and HP, ability, status, item, boost, species,
  field and side differences there went unreported. The bounded-output
  counts in the section above (7 repros, 14/14; Palafin, 2/2) were scored
  that way, so turn 2 was compared on faints only. They need a re-run.
- **Evidence.** The parent ran 24 synthetic one-turn Trace jobs on PS
  `a5df8274…` with sodium seeds `…01`-`…08`. The 8 Ability Shield jobs
  scored 1/1 matched. Copies with only `ended` set to false showed
  turn 1 / p1a / ability: engine `sapsipper` vs PS `voltabsorb` on seeds
  1, 2, 4, 5, 6 and 8. That diagnostic used edited metadata. The
  unmodified outputs were re-run under this fix: the same six ability
  divergences are now reported. The Trace difference is not fixed here.
- **Boundary.** `Meta` now reads the optional `_meta.lastTurn` (the last
  `|turn|` marker PS printed, already written by `ps-battle.js`). Let S be
  the last recorded state turn. The faint-only comparison applies to S
  only when `ended` is set and either `lastTurn` ≤ S (a natural win or tie
  on S) or `lastTurn` is absent (older captures, previous reading). A
  cutoff has `lastTurn = N + 1 > S = N`, so every recorded turn gets the
  full diff. Not-ended battles are unchanged, as are the RNG tables,
  repair, report schema and replay. Old captures that still contain the
  unplayed cap turn have `lastTurn = S`, so they keep the faint-only
  comparison on that turn. `accuracy_repros.rs` already drops that turn,
  which leaves the stored goldens' real last turn compared in full; all
  61 still pass.
- **Tests.** `crates/vgc-engine-conformance/tests/accuracy_terminal.rs`
  uses turn 1 of the `recoil-uncapped-on-ko` capture with `ended` set
  and one expected value corrupted, replayed through `replay_keyed`:
  - `lastTurn = 2`: a wrong HP, ability or boost is reported on turn 1.
    Before the fix, the wrong HP went undetected.
  - `lastTurn = 1` (natural end) and no marker (older captures): those
    corruptions are still ignored, and a wrong faint is still reported.
- The corpus has not been rescored.
- The previous 8 bounded fixtures were rechecked under this full comparison:
  all 16 played state snapshots still match, with no missing draws or errors.
  Their existing leftover-draw / choice-repair heuristics still apply.

## Mechanics fix: Trace copies from a foe holding Ability Shield

This is an engine fix, separate from the harness work above. It found
the Ability Shield Trace divergence that the full last-turn comparison
now reports.

- **Bug.** `ability.rs` Trace dropped any foe holding Ability Shield from
  its candidates. The comment claimed the shield's `onCopyAbility` returns
  false, but PS has no such handler.
- **PS** (`a5df8274e85b0889bf2a9b3422a08b39732374fc`):
  - Ability Shield (`data/items.ts:2-17`) has only `onSetAbility`, which
    protects the holder's own ability.
  - Trace's `onUpdate` (`data/abilities.ts:5136-5145`) filters candidates
    by the noTrace / noability flags only, `sample`s one and calls
    `pokemon.setAbility(ability, target)`.
  - `setAbility` runs SetAbility on the Trace user
    (`sim/pokemon.ts:1915-1931`), so a foe's shield doesn't matter.
  - Bulbapedia agrees: <https://bulbapedia.bulbagarden.net/wiki/Ability_Shield>,
    <https://bulbapedia.bulbagarden.net/wiki/Trace_(Ability)>.
- **Fix.** The target filter is removed. A Trace user holding Ability
  Shield still copies nothing, and the candidate list, the single `sample`
  draw and its key are unchanged. The comments on the Ability Shield
  helper and Trace now say the shield protects the holder's own ability,
  and copying from a shielded foe is allowed.
- **Default RNG stream change.** With a shielded foe present, Trace now
  draws over two candidates where it drew over one (doubles), or draws
  where it previously drew nothing (a lone shielded foe).
- **Evidence (root).**
  - 8 quiet doubles jobs (sodium seeds 1-8): Porygon2 Trace / Snorlax vs
    Miltank Sap Sipper and Lanturn Volt Absorb @ Ability Shield. PS picked
    Lanturn on seeds 1, 2, 4, 5, 6 and 8; the engine copied Sap Sipper.
    The unshielded and two-Trace-holder controls already matched.
  - Singles golden `trace-copies-ability-shield-target`
    (`gen9customgame`, seed `[1,2,3,4]`, generated by root): Porygon2
    traces Volt Absorb and is immune to Lanturn's Thunderbolt, staying at
    160/160 with no status. The engine had 111/160 and paralysis.
- **Tests.** Four new tests in `battle_r9_tests.rs`; all failed before the
  fix and pass after:
  - singles: a lone shielded foe is copied, and Thunderbolt is absorbed
    (8 seeds);
  - keyed: Trace's Ability key with `Range(0)` / `Range(1)` copies Sap
    Sipper / Volt Absorb, and the outcome is consumed;
  - recording: the Trace draw is `UniformRange(2)`;
  - `ps-rng`: one `sample` draw over (0, 2), whose result picks the
    copied ability.
  `ability_shield_blocks_trace_on_user` still passes, and the golden
  corpus gate passes.
- **Not shown.** This doesn't show that the remaining historical
  Trace-pick battle (listed under RNG plumbing in round 10) is fixed. The
  corpus has not been rescored.
- **Root verification.** The unmodified 24 one-turn doubles outputs match
  state and draw traces in seeded `ps-rng` replay after this fix. All 25
  keyed fixtures (those plus the two-turn singleton) match 26/26 played
  turns with no missing draws; keyed replay uses 33 existing repair aliases.
  Workspace: 1501 default / 1536 `ps-rng` tests pass, 0 fail / 32 ignored
  each, including both stored golden gates. Clippy's configured guards and
  the residual-index audit pass. These are bounded fixtures, not a new
  whole-corpus percentage.

## Mechanics fix: a partial trap's last turn deals no chip

This is one structural fix to the shared partial-trap residual. It covers
Infestation and every other move that sets the trap (Whirlpool, Wrap,
Bind, Fire Spin, Sand Tomb, Magma Storm, Clamp, Snap Trap, Thunder Cage).

- **Bug.** `eot_partial_trap` chipped 1/8 max HP and then removed the
  volatile when its counter was at 1. With a 5 / 6 timer that is 5 / 6
  chips; PS gives 4 / 5.
- **PS** (`a5df8274e85b0889bf2a9b3422a08b39732374fc`):
  - `data/conditions.ts:222-247` partiallytrapped: the `durationCallback`
    draws `random(5, 7)`, and the chip runs at onResidualOrder 13.
  - `sim/battle.ts:515-522`: the residual loop decrements a condition's
    duration before its onResidual; at 0 it ends the condition and skips
    the handler.
  - Bulbapedia agrees (4-5 turns of 1/8):
    <https://bulbapedia.bulbagarden.net/wiki/Infestation_(move)>.
- **Fix.** A counter at 1 now removes the trap and deals no chip. The
  removal still happens on the same turn as before. Unchanged: the
  initial `range(2)` draw, the payload layout (counter, source side, slot
  and team index), the early removal when the source leaves or faints,
  and Magic Guard blocking the chip while the timer runs. No RNG changes.
  The volatile set keeps its own presence mask, so no residual-index
  sync is needed. The comments that called the 5-6 timer "5-6 turns" of
  chip now say 4-5 chips.
- **Default RNG stream.** No draws are added or removed, but HP from the
  expiry turn onward changes, and later draws can change with it.
- **Evidence (root).** Four 6-turn Champions doubles jobs on PS (sodium
  seeds 1-4): Toxapex uses Infestation once on Snorlax (Immunity), then
  everyone Splashes. Under the full last-turn scorer each diverged by
  exactly one 29-HP tick, with no missing draws, `rng_sensitive` false
  and no errors:
  - seed 1 (timer 5): PS HP `[201,172,143,114,114,114]`; the engine first
    diverged on turn 5 (85 vs 114).
  - seeds 2-4 (timer 6): PS `[201,172,143,114,85,85]`; the engine first
    diverged on turn 6 (56 vs 85).
  - Goldens generated by root, `infestation-expiry-seed-1` (timer 6) and
    `infestation-expiry-wide-a` (timer 5): these failed
    `corpus_zero_divergences` before the fix and pass now.
- **Tests** (`battle_r10_tests.rs`):
  - `infestation_chips_one_turn_less_than_its_timer`, over 24 seeds
    covering both timers (counter after turn 1, plus 1): an equal chip on
    each turn 2..timer-1, then the trap ends on the timer's turn with no
    chip, and nothing after. Failed before the fix (seed 0, timer 5: a
    turn-5 chip, 84 vs 113); passes now.
  - Guards that passed both before and after:
    `partial_trap_under_magic_guard_still_expires_on_time` (no HP change,
    trap ends on the timer's turn) and
    `partial_trap_ends_without_a_chip_when_its_source_faints`. The
    existing source-switch test also passes.
- **Not shown.** The historical study battle behind round 10's remaining
  "Infestation duration" item has not been rescored on its own. Its old
  duration-key issue may be separate, and the study percentages above
  are unchanged.
- **Root verification.** Unmodified fixed-seed Champions fixtures now match
  24/24 played turns in both keyed and seeded `ps-rng` replay, with no
  missing draws, first draw divergence, or PS/engine errors. The full
  workspace passes 1504 default / 1539 `ps-rng` tests, with 0 failures and
  32 ignored in each mode, including both stored golden gates. Clippy's
  configured allocation guards, the residual-index audit and diff check
  pass. This is bounded fixture evidence, not a whole-corpus rescore.

## Harness fix: Champions Encore's retarget roll is keyed to the Encored mon

This is a recorder (keyed-context) fix, not an engine or simulator change.
It covers the "Champions Encore's retarget draw" harness item in round 10.

- **Bug.** On PS `a5df8274e85b0889bf2a9b3422a08b39732374fc`, Champions
  Encore's onStart (`data/mods/champions/moves.ts:307-339`) replaces the
  target's queued move with the Encored one through
  `queue.changeAction(target, {choice: 'move', moveid})`, with no target.
  `BattleQueue.resolveAction` (`sim/battle-queue.ts:268-275`) then calls
  `battle.getRandomTarget(action.pokemon, action.move)`
  (`sim/battle.ts:2490-2522`, `side.randomFoe()` in doubles). The recorder
  keyed every draw on `activePokemon` / `activeMove`, which are still the
  Encore user and Encore, so this roll was recorded as
  `p1a → p2a, encore, range` instead of belonging to the Encored mon's
  move.
- **Fix** (`conformance-driver.js` `patchRng`).
  - `Battle.prototype.getRandomTarget` is wrapped. It relabels a call only
    when all three hold: the current effect is the `encore` Condition, its
    holder (`effectState.target`) is the `pokemon` argument, and the move
    argument is the forced move (`effectState.move`).
  - For that call only, draws are keyed `actor = holder, target = null,
    move = forced move`. The previous context is restored in `finally`,
    and teardown restores the prototype.
  - Encore's accuracy roll and `insertChoice`'s insertion tie roll keep
    Encore's context, and ordinary target picks are unchanged.
  - No draws, raw trace, queue order or ability / sample classification
    change. `ps-battle.js` leaves the wrapper's own frame (function
    `Battle.getRandomTarget` in `conformance-driver.js`) out of a raw
    draw's semantic sites. Without that, the frame pushed Encore's
    `onStart` out of the six recorded sites. PS's own
    `Battle.getRandomTarget` frame is kept.
- **Evidence (root probe).** Four synthetic Champions doubles jobs
  (sodium seeds 1-4, two turns) where Prankster Whimsicott Encores a
  Snorlax that Tackled on turn 1 and queued Splash on turn 2. During the
  call, `effect` = Encore Condition, `effectState` = `{move: 'tackle',
  target: p2a}` and `activePokemon` = p1a. Seed 1's roll returns p1b.
  Before the fix, the keyed replay missed one range draw in all four jobs
  and had one state failure (seed 3).
- **Tests** (`tools/accuracy/encore-context.test.js`, on real PS via
  `PS_DIST`): equivalent in-repo jobs for seeds 1-4.
  - Turn 2 has exactly one `p2a / null / tackle` range draw. Its value
    equals the raw trace's single `getRandomTarget` draw and picks the foe
    PS's `|move|` line shows was hit; both foes occur across the seeds.
  - That raw draw's sites are exactly PS's stack, from `Side.randomFoe`
    through `Battle.onStart` in `data/mods/champions/moves.js`, with no
    recorder frame. Before the frame filter, the wrapper's frame took the
    place of `onStart`.
  - Encore keeps its accuracy roll and exactly one range draw (the
    insertion tie).
  - Turn 1's chosen-target Tackle keeps its normal context, and there are
    no PS errors.
  - A second test checks that the patched prototypes are restored and that
    a throw inside the wrapped call doesn't leak its context.
  - Before the fix, the first test found no `p2a / tackle` draw (both
    turn-2 range draws were labelled `p1a / encore`) and the second found
    `getRandomTarget` unpatched. Both pass now, and the switch-order and
    turn-limit suites still pass.
- **Not fixed.** The engine's seeded (`ps-rng`) replay still diverges at
  the retarget's timing: PS draws `random(2)` where the engine shuffles
  `(2, 4)`. This fix doesn't change that. Root regenerated the four jobs:
  keyed replay matches 8/8 played turns with no missing draws and 38
  existing repair aliases. Seeded replay still has first draw divergence
  in all four cases (5/8 state matches).
- **Root preservation check.** Before/after PS raw draws, including their
  semantic stack sites, turn states and choices are identical. Protocol
  logs differ only in wall-clock `|t:|` entries. Exactly one keyed Range
  envelope changes per job; its value and the other draws are unchanged.

## Mechanics fix: Champions Encore replaces the queued action at once

This is one Encore mechanic: Champions' immediate queued-action
replacement. Standard gen 9 Encore is unchanged.

- **Bug.** In Champions, when Encore lands on a mon that still has a
  different move queued, PS re-queues the Encored move immediately, with its
  own priority and a target picked then. The engine kept the old queued move
  and swapped in the Encored one only when the mon acted (standard gen 9's
  `onOverrideAction`), at the old move's place in the order, with the target
  drawn at that point.
- **PS** (`a5df8274e85b0889bf2a9b3422a08b39732374fc`):
  - `data/mods/champions/moves.ts:307-339` Encore `onStart`: if the target
    has a queued action whose move differs from the Encored one, and it
    holds no Mental Herb, `queue.changeAction(target, {choice: 'move',
    moveid, order: action.order})` (no target).
  - `sim/battle-queue.ts:301-305` `changeAction` cancels the action and
    calls `insertChoice` (`:369-402`). `insertChoice` resolves the action
    first: a missing target comes from `getRandomTarget` (`:268-275`,
    `sim/battle.ts:2490-2522`). It then draws `random(first, last + 1)`
    for the insertion index when the new action ties queued ones.
  - The gen-8+ re-sort after Encore's action (`getActionSpeed`,
    `sim/battle.ts:2619-2654`) orders it by the forced move's own priority.
  - Bulbapedia agrees for Champions:
    <https://bulbapedia.bulbagarden.net/wiki/Encore_(move)#Pok%C3%A9mon_Champions>.
- **Fix** (`battle.rs`, Champions only, gated as PS at
  `data/mods/champions/moves.ts:326`: a queued move other than the Encored
  one, Struggle included, and no Mental Herb).
  - When Encore lands, `champions_encore_requeue` draws the forced move's
    target. It uses the same `getRandomTarget` code and key as before
    (holder, forced move, no target; now `random_target_for`, shared with
    `encore_override`).
  - Under `ps-rng` it also draws the `insertChoice` tie index. The queue is
    sorted, so that is the count of queued actions ahead of the new one plus
    a `random(first, last + 1)` on a tie, using the re-sort's keys
    (`order::queued_move_key`).
  - It then restores the Encore user's draw context and leaves a Copy
    request (`encore_requeue`).
  - `finalize_move_resolution` applies the request to the unprocessed queue.
    The target's choice becomes the Encored slot and drawn target; its
    Tera / Mega form and fixed sort keys (Encore keeps `order`) stay.
    `queued_move_slot` and `pending_kind` are updated, so Upper Hand and
    Sucker Punch see the forced move. Under `ps-rng` the action also moves
    to the insertion index.
  - The existing post-move re-sort then uses the forced move's priority.
    `encore_override` finds the slots equal and draws nothing more.
- **State and allocations.** The request is a Copy `Option`, set and taken
  within one action's `process_one_action`, and also cleared when each
  turn's queue is built. It is `None` between steps, so there is nothing to
  hash or carry through chance yields. No heap, no `unsafe`, no
  residual-index fields.
- **adjacentAlly forced moves.** `random_target_for` follows PS
  `getRandomTarget` for adjacentAlly (target code 2, `sim/battle.ts:2503-2508`):
  the user's living adjacent ally, drawn as `random(1)`, and no draw in
  Singles or without a living ally. Before, it sampled the foes. Standard
  gen 9's `encore_override` shares the helper, so its Encored adjacentAlly
  moves change the same way. As for a single living foe, the one-outcome
  draw consumes a raw draw (Splitmix, `ps-rng`) but is neither recorded nor
  keyed, so PS's recorded `random(1)` stays an unconsumed keyed entry.
- **RNG.** No new decision kinds. The target pick keeps its key but is drawn
  when Encore lands, so in the default stream it moves ahead of any draws
  made between Encore and the target's action. `ps-rng` gains PS's
  `insertChoice` tie draw.
- **Evidence (root).**
  - Golden `champions-encore-quick-guard-priority` (Champions, seed
    `[1,2,3,4]`): Conkeldurr Quick Guards on turn 1. On turn 2 it queues
    Splash and is Encored, and Pikachu uses Quick Attack on it. PS: the
    forced Quick Guard (+3) goes first and blocks it (180/180). The engine
    had 168/180. It failed `corpus_zero_divergences` before the fix and
    passes now.
  - Four seeded Encore jobs (sodium 1-4): PS draws `getRandomTarget`
    during Encore's `onStart`, where the engine drew a re-sort shuffle.
- **Tests** (`battle_r10_tests.rs`):
  - Champions forced Quick Guard blocks Quick Attack (seeds 1-4), while the
    standard gen 9 control takes the hit.
  - Recording: exactly one `UniformRange(2)` target draw for the forced
    Tackle, before the next actor's draws in Champions and after them in
    standard gen 9.
  - Keyed `Range(0)` / `Range(1)`: the forced Tackle hits p1a / p1b, and
    the outcome is consumed once.
  - `ps-rng`: one `random_target` before the next actor's draws; with a
    Speed tie behind a faster mon, `insert_choice(1, 3)` right after it,
    and none without a tie.
  - Guards: Mental Herb (no re-queue, Encore cured, Splash stands), same
    queued move (no draw, chosen target kept), target already acted
    (duration 4).
  - Before the fix, the Quick Guard test (168 HP), the Recording timing
    test and the `ps-rng` test failed; the keyed and guard tests already
    passed. All pass in default, `ps-rng` and `chance` builds, existing
    Encore tests included.
  - Queued Struggle (added after root review): Rillaboom, whose only move
    Fake Out is unselectable after its first action, takes Struggle from
    `legal_choices` and is Encored into Fake Out, which fails. Whimsicott
    takes no damage and there is no recoil. With the old Struggle exclusion
    Whimsicott fell to 82 HP; root's PS fixture keeps it at 112.
  - adjacentAlly (added after root review): an Encored Aromatic Mist hits
    the partner with no foe sample, and `ps-rng` draws exactly
    `random_target(0, 1)`. With the partner fainted, or in Singles, there
    is no draw. Before, the engine drew a 2-way foe sample (`UniformRange(2)`,
    `random_target(0, 2)`).
  - Root's goldens `champions-encore-replaces-queued-struggle` and
    `champions-encore-targets-adjacent-ally` already passed the first patch,
    so they're smoke coverage only. The golden loader doesn't lower a forced
    Struggle from `move 1`, and the golden scorer doesn't check draw bounds.
- **Limits.**
  - PS's re-resolution also re-runs FractionalPriority (`:246`, a Quick
    Claw / Quick Draw roll); the engine keeps the turn's fractional keys and
    draws nothing.
  - The insertion compares the engine's current keys, not PS's stored
    action values.
  - Under the keyed oracle, PS's insertion tie draw stays unconsumed and
    the action keeps its queue position.
- **Root final verification.** Eleven unmodified actual PS fixtures (four
  fixed-seed retargets, the priority case, four boundary controls and the
  newly queued Struggle / adjacentAlly cases) match 22/22 played turns in
  both modes. Seeded `ps-rng` replay has no normalized first draw
  divergence, missing draws, repairs or errors. Keyed replay has no missing
  draws or errors, but retains 48 existing repair aliases and eight leftover
  draws; strict keyed trace equivalence is not claimed.
  Full workspace excluding Python: 1511 default / 1548 `ps-rng` tests
  pass, with 0 failures and 32 ignored each, including both stored golden
  gates. Release build, configured clippy guards, residual-index audit and
  diff check pass. The historical study corpus was not rescored.

## Mechanics fix: pre-turn manual switch order on Speed ties

This is one structural fix: the order of voluntary start-of-turn switches.
Lead abilities, faint replacements and Encore are not touched.

- **Bug.** `apply_pre_turn_switches` sorted switches by the leaving mon's
  Speed with an unstable sort, so ties came out in an arbitrary fixed order.
  The `ps-rng` and keyed commit sorts already ranked the switch actions,
  with PS's tie shuffle, but kept only the move order.
- **PS** (`a5df8274e85b0889bf2a9b3422a08b39732374fc`):
  - Switch actions are order 103 (`sim/battle-queue.ts`).
  - commitChoices' `queue.sort()` / `speedSort` (`sim/battle.ts:429`)
    orders them by comparePriority (`:404`) on the leaving mon's action
    Speed, flipped under Trick Room, and shuffles exact ties.
  - `switchIn` / `runSwitch` (`sim/battle-actions.ts`) then run them in
    that order. The incoming mon's Speed plays no part.
  - Bulbapedia, <https://bulbapedia.bulbagarden.net/wiki/Switching>: the
    leaving Pokémon's Speed sets the order of manual switches.
- **Evidence (root).** 12 manual-switch jobs on PS: both leaving Chanseys
  tie at 70 Speed, and the incoming Indeedee / Rillaboom have Speeds
  115/115, 147/105 and 115/137. 9 diverged on state. On sodium seed 1, PS
  switches Rillaboom in before Indeedee (Psychic Terrain), where the engine
  did the opposite (Grassy). There were no missing keyed draws, and no
  first draw divergence in the seeded cases with unequal incoming Speeds,
  so the engine was dropping PS's order rather than mis-consuming RNG.
- **Fix** (`battle.rs`).
  - Both commit sorts record the switch actions' sorted order in a fixed
    `commit_switch_order: Option<([(u8, u8); 4], u8)>`, which
    `apply_pre_turn_switches` takes and follows with no new sort or draw.
  - Without a commit sort (Splitmix, Recording, plain oracles) it sorts by
    the leaving mon's Speed with `ps_speed_sort`, shuffling exact ties
    through `speed_sort_draw`, the same speedSort machinery.
  - Only the existing classification is kept: a later switch for a slot
    that already moved is still treated as a mid-turn pick. No new filter
    for invalid or out-of-range switch choices was added; those are still
    handled downstream in `do_switch`. Move, Mega and Tera ordering is
    unchanged. No heap, no `unsafe`.
- **RNG.** `ps-rng` and the keyed oracle draw nothing new. Splitmix and
  Recording now draw one tie shuffle when leaving Speeds are exactly equal.
- **Tests** (`battle_r10_tests.rs`, `switch_order_*`):
  - A Splitmix tie gives both terrains across seeds. The old unstable
    sort drew nothing, so it should give one fixed order, but that wasn't
    run.
  - A faster leaving Jolteon switches first (Grassy), and Trick Room
    reverses it (Psychic), every seed.
  - Keyed: exactly one Tiebreak draw (the commit sort's), none from the
    executor.
  - `ps-rng` (sodium seeds 1-8): the commit `shuffle(0, 2)` decides the
    terrain, and both outcomes occur.
  - Root ran them after the change: 3 default and 4 `ps-rng` tests pass.
    They were written after the fix, so they carry no before-fix red of
    their own.
- **Red evidence** is root's actual-PS conformance, not these tests: on
  gen5 seeds 1 and 5, PS has 175 HP where the old engine had 172. Before
  the fix, 9 of 12 manual-switch jobs diverged, while the other 8
  outgoing-Speed / Trick Room controls matched 16/16. After the fix, all
  24 manual-switch cases match 32/32 turns in keyed and seeded replay:
  no missing draws, errors or first draw divergences. Keyed replay uses
  24 existing repair aliases; seeded replay uses none.
- **Golden.** A tied-Speed golden can't pin the switch tie. The PS golden
  driver patches only `Battle.random`, and `PRNG.shuffle` (which speedSort
  uses) bypasses it, so the `.ps.json` records no Tiebreak entries and the
  engine's fallback can't reproduce PS's shuffle. The first tied input
  failed on that (PS 175 vs engine 172); root archived it and removed it.
  The exact RNG proof for ties comes from the keyed and seeded conformance
  runs. In its place,
  `crates/vgc-engine-golden/goldens/switch-order-speed-terrain.input.json`
  (Champions, seed `[1,2,3,4]`) is an unequal-Speed smoke control, not a
  red regression. P1's leaving Chansey has 10 Speed EVs (80 vs P2's 70),
  so P1 switches to Indeedee first and P2's Rillaboom enters last,
  leaving Grassy Terrain, and Pikachu's Quick Attack on p2a deals damage.
  Root generated its `.ps.json` with the pinned offline PS oracle:
  `ok: true`, no errors, Rillaboom ends at 164/175 HP.
- **Limits.** Lead-order ties (the 4/8 lead cases) are a separate issue
  and remain unfixed. The ps-rng speed key is the cached PS Speed, the
  keyed one the engine's effective Speed, as for moves. No native `chance`
  test of the switch tie was added.
- **Verification (root):** full workspace excluding Python bindings:
  default 1,514 passed; `ps-rng` 1,552 passed; 0 failed and 32 ignored
  each, including both golden gates. Release accuracy build, default
  workspace configured clippy guards, core `ps-rng` clippy, residual-index
  audit and diff check pass. Expanding the workspace clippy run to
  `ps-rng` exposes a pre-existing unnecessary collect in
  `crates/vgc-engine-conformance/src/accuracy.rs:915`; it is outside this
  switch-order change. Historical corpus results remain unchanged.
