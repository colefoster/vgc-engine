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
