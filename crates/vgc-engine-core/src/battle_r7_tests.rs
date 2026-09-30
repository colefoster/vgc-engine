//! Accuracy round 7 (docs/accuracy/fix-log.md): moves, abilities and items
//! the Reg M-C sample uses that the engine was missing.

use super::*;
use crate::choice::{Choice, Target};
use crate::team::TeamBuilder;

fn t(side: SideRef, slot: u8) -> Target {
    Target { side, slot }
}

fn mv(slot: u8, move_slot: u8, target: Option<Target>) -> Choice {
    Choice::Move { actor_slot: slot, move_slot, target }
}

fn singles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Singles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

fn doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

/// Accuracy rolls the battle made for `move_id` (Recording RNG).
fn accuracy_rolls(b: &Battle, move_id: u16) -> usize {
    b.rng
        .recording_log()
        .expect("Recording RNG")
        .iter()
        .filter(|e| e.key.move_id == move_id && matches!(e.space, crate::rng::DrawSpace::UniformPercent { .. }))
        .count()
}

#[test]
fn simple_doubles_a_stat_change() {
    // PS data/abilities.ts:simple onChangeBoost: `boost[i] *= 2`.
    let mut b = singles(
        r#"[{"species":"bibarel","level":50,"ability":"simple","moves":["swordsdance"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["growl"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].boosts[0], 4 - 2, "+2 doubled to +4, Growl's -1 doubled to -2");
}

#[test]
fn simple_beam_gives_the_target_simple() {
    // PS data/moves.ts:simplebeam onHit: target.setAbility('simple').
    let mut b = singles(
        r#"[{"species":"audino","level":50,"ability":"healer","moves":["simplebeam","growl"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["curse","swordsdance"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].effective_ability_id(), data::ability_id::SIMPLE);
    let atk = b.p2.team[0].boosts[0];
    b.step(&[mv(0, 1, Some(t(SideRef::P2, 0)))], &[mv(0, 1, None)]);
    assert_eq!(b.p2.team[0].boosts[0], atk + 4 - 2, "Swords Dance +4, Growl -2");
}

#[test]
fn simple_beam_fails_on_truant_before_its_accuracy_roll() {
    // PS simplebeam onTryHit (hitStepTryHitEvent, before hitStepAccuracy):
    // Simple / Truant / cantsuppress targets fail with no roll.
    let mut b = singles(
        r#"[{"species":"audino","level":50,"ability":"healer","moves":["simplebeam"]}]"#,
        r#"[{"species":"slaking","level":50,"ability":"truant","moves":["bulkup"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].effective_ability_id(), data::ability_id::TRUANT);
    assert_eq!(accuracy_rolls(&b, data::move_id::SIMPLEBEAM), 0);
}

#[test]
fn simple_beam_rolls_accuracy_and_is_stopped_by_a_substitute() {
    // Substitute's onTryPrimaryHit (data/conditions.ts) fails a status move
    // without bypasssub in the hit loop, after the accuracy roll.
    let mut b = singles(
        r#"[{"species":"audino","level":50,"ability":"healer","moves":["simplebeam","growl"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["substitute","curse"]}]"#,
        1,
    );
    b.step(&[mv(0, 1, None)], &[mv(0, 0, None)]);
    assert!(b.p2.team[0].substitute_hp() > 0);
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 1, None)]);
    assert_eq!(accuracy_rolls(&b, data::move_id::SIMPLEBEAM), 1);
    assert_eq!(b.p2.team[0].effective_ability_id(), data::ability_id::THICKFAT);
}

#[test]
fn entrainment_gives_the_ally_the_users_ability() {
    // PS data/moves.ts:entrainment onHit: target.setAbility(source.ability).
    // The sample's use: Hawlucha passes No Guard to its partner.
    let mut b = doubles(
        r#"[{"species":"hawlucha","level":50,"ability":"noguard","moves":["entrainment"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
        r#"[{"species":"pikachu","level":50,"ability":"static","moves":["growl"]},
            {"species":"raichu","level":50,"ability":"static","moves":["growl"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[1].effective_ability_id(), data::ability_id::NOGUARD);
}

#[test]
fn entrained_intimidate_starts_on_the_target() {
    // setAbility runs the gained ability's onStart (sim/pokemon.ts:1943).
    let mut b = doubles(
        r#"[{"species":"incineroar","level":50,"ability":"intimidate","moves":["entrainment"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
        r#"[{"species":"pikachu","level":50,"ability":"static","moves":["charm"]},
            {"species":"raichu","level":50,"ability":"static","moves":["charm"]}]"#,
        1,
    );
    let before = b.p2.team[0].boosts[0];
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p1.team[1].effective_ability_id(), data::ability_id::INTIMIDATE);
    assert_eq!(b.p2.team[0].boosts[0], before - 1, "the entrained Intimidate fires on gain");
}

#[test]
fn entrainment_fails_on_a_shared_ability_before_its_accuracy_roll() {
    // PS entrainment onTryHit: target.ability === source.ability fails.
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["entrainment"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
        r#"[{"species":"pikachu","level":50,"ability":"static","moves":["growl"]},
            {"species":"raichu","level":50,"ability":"static","moves":["growl"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(accuracy_rolls(&b, data::move_id::ENTRAINMENT), 0);
}

#[test]
fn worry_seed_gives_insomnia_and_wakes_the_target() {
    // PS data/moves.ts:worryseed onHit: setAbility('insomnia'), then
    // `if (target.status === 'slp') target.cureStatus()`.
    let mut b = singles(
        r#"[{"species":"breloom","level":50,"ability":"technician","moves":["spore","worryseed"],"evs":{"spe":252}}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(matches!(b.p2.team[0].status, Status::Sleep));
    b.step(&[mv(0, 1, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].effective_ability_id(), data::ability_id::INSOMNIA);
    assert!(!matches!(b.p2.team[0].status, Status::Sleep));
}

#[test]
fn worry_seed_is_immune_on_insomnia_before_its_accuracy_roll() {
    // PS worryseed onTryImmunity (hitStepTryImmunity): Truant / Insomnia.
    let mut b = singles(
        r#"[{"species":"breloom","level":50,"ability":"technician","moves":["worryseed"]}]"#,
        r#"[{"species":"hypno","level":50,"ability":"insomnia","moves":["calmmind"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(accuracy_rolls(&b, data::move_id::WORRYSEED), 0);
}

#[test]
fn magic_powder_makes_the_target_pure_psychic() {
    // PS data/moves.ts:magicpowder onHit: target.setType('Psychic').
    let mut b = singles(
        r#"[{"species":"vivillon","level":50,"ability":"compoundeyes","moves":["magicpowder"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["swordsdance"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    let (types, n) = b.p2.team[0].effective_types();
    assert_eq!((n, types[0]), (1, 10), "pure Psychic");
}

#[test]
fn magic_powder_is_a_powder_move_grass_types_ignore() {
    // hitStepTryImmunity (sim/battle-actions.ts:669): powder moves fail on
    // Grass types before the accuracy roll.
    let mut b = singles(
        r#"[{"species":"vivillon","level":50,"ability":"compoundeyes","moves":["magicpowder"]}]"#,
        r#"[{"species":"venusaur","level":50,"ability":"chlorophyll","moves":["growth"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(accuracy_rolls(&b, data::move_id::MAGICPOWDER), 0);
    assert_eq!(b.p2.team[0].effective_types().1, 2);
}

#[test]
fn octolock_traps_and_drops_def_and_spd_each_turn() {
    // PS data/moves.ts:12960 octolock condition: onTrapPokemon while the
    // source is active; onResidual (order 14) boosts {def: -1, spd: -1}.
    let mut b = singles(
        r#"[{"species":"grapploct","level":50,"ability":"limber","moves":["octolock","bulkup"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]},
            {"species":"pikachu","level":50,"ability":"static","moves":["growl"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P2, 0));
    assert_eq!((b.p2.team[0].boosts[1], b.p2.team[0].boosts[3]), (1 - 1, -1), "Curse +1 Def, Octolock -1/-1");
    b.step(&[mv(0, 1, None)], &[mv(0, 0, None)]);
    assert_eq!((b.p2.team[0].boosts[1], b.p2.team[0].boosts[3]), (2 - 2, -2));
}

#[test]
fn octolock_cannot_trap_a_ghost_and_skips_its_accuracy_roll() {
    // onTryImmunity: dex.getImmunity('trapped', target) — Ghost types.
    let mut b = singles(
        r#"[{"species":"grapploct","level":50,"ability":"limber","moves":["octolock"]}]"#,
        r#"[{"species":"gengar","level":50,"ability":"cursedbody","moves":["calmmind"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(accuracy_rolls(&b, data::move_id::OCTOLOCK), 0);
    assert_eq!(b.p2.team[0].boosts[1], 0);
}

#[test]
fn octolock_ends_when_its_user_leaves() {
    // onResidual: a source that is no longer active ends the lock, no drop.
    let mut b = singles(
        r#"[{"species":"grapploct","level":50,"ability":"limber","moves":["octolock"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
        r#"[{"species":"blissey","level":50,"ability":"naturalcure","moves":["calmmind"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].boosts[1], -1);
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].boosts[1], -1, "no drop once the user left");
    assert!(!b.is_trapped(SideRef::P2, 0));
}

const DARTS_P1: &str = r#"[{"species":"dragapult","level":50,"ability":"clearbody","nature":"jolly","moves":["dragondarts"],"evs":{"spe":252}},
    {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#;

#[test]
fn dragon_darts_hits_each_foe_once_with_an_accuracy_roll_apiece() {
    // PS dragondarts `smartTarget` (data/moves.ts:4118): getSmartTargets
    // (sim/pokemon.ts:757) targets the chosen foe and its ally; each target
    // rolls accuracy in hitStepAccuracy, then hit 1 lands on the first and
    // hit 2 on the second (data/mods/champions/scripts.ts:467).
    let mut b = doubles(
        DARTS_P1,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["swordsdance"]},
            {"species":"dragonite","level":50,"ability":"innerfocus","moves":["dragondance"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(accuracy_rolls(&b, data::move_id::DRAGONDARTS), 2);
    let hit = |m: &Pokemon| m.current_hp < m.stats.hp;
    assert!(hit(&b.p2.team[0]) && hit(&b.p2.team[1]), "one dart each");
}

#[test]
fn dragon_darts_sends_both_hits_at_the_ally_of_an_immune_target() {
    // A Fairy target fails hitStepTypeImmunity; `smartTarget` turns off
    // (sim/battle-actions.ts:607) and the other foe takes both hits.
    let run = |p2: &str| {
        let mut b = doubles(DARTS_P1, p2, 1);
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
        (b.p2.team[0].stats.hp - b.p2.team[0].current_hp, b.p2.team[1].stats.hp - b.p2.team[1].current_hp)
    };
    let (fairy, other) = run(
        r#"[{"species":"clefable","level":50,"ability":"magicguard","moves":["calmmind"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
    );
    let (_, single) = run(
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["swordsdance"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
    );
    assert_eq!(fairy, 0);
    assert!(single > 0);
    assert!(other > single * 3 / 2, "two darts ({other}) vs one ({single})");
}

const QC_HOLDER: &str = r#"[{"species":"garchomp","level":50,"ability":"roughskin","item":"quickclaw","nature":"jolly","moves":["dragonclaw","quickattack"],"evs":{"spe":252}}]"#;
const QC_FOE: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["quickattack","bodyslam"]}]"#;

fn first_mover(b: &Battle, p1: Choice, p2: Choice, rng: &mut crate::rng::Rng) -> SideRef {
    let order = crate::order::action_order(b, &[p1], &[p2], rng);
    order.iter().find(|a| matches!(a.choice, Choice::Move { .. })).unwrap().side
}

#[test]
fn quick_claw_is_fractional_priority_and_cannot_beat_a_priority_move() {
    // PS data/items.ts:4989 quickclaw onFractionalPriority returns 0.1: the
    // holder moves first within its priority bracket, never ahead of a +1
    // move (sim/battle.ts:2647 `priority + fractionalPriority`).
    let b = singles(QC_HOLDER, QC_FOE, 1);
    let mut rng = crate::rng::Rng::oracle_partial(vec![crate::rng::RngEvent::Range(0)], 0);
    let first = first_mover(&b, mv(0, 0, Some(t(SideRef::P2, 0))), mv(0, 0, Some(t(SideRef::P1, 0))), &mut rng);
    assert_eq!(first, SideRef::P2, "Quick Attack (+1) outranks a Quick Claw Dragon Claw (+0.1)");
}

#[test]
fn quick_claw_rolls_for_a_priority_move_too() {
    // PS runs FractionalPriority with relay 0 (sim/battle-queue.ts:249), so
    // quickclaw's `priority <= 0` gate never looks at the move's priority.
    let b = singles(QC_HOLDER, QC_FOE, 1);
    let mut rng = crate::rng::Rng::recording(3);
    let _ = first_mover(&b, mv(0, 1, Some(t(SideRef::P2, 0))), mv(0, 1, Some(t(SideRef::P1, 0))), &mut rng);
    let rolls = rng
        .recording_log()
        .unwrap()
        .iter()
        .filter(|e| matches!(e.space, crate::rng::DrawSpace::UniformRange(5)))
        .count();
    assert_eq!(rolls, 1);
}

#[test]
fn quick_claw_rolls_when_choices_commit_before_switches() {
    // PS rolls FractionalPriority in resolveAction (sim/battle-queue.ts:249),
    // as commitChoices queues each action, before any switch runs; here the
    // Quick Claw roll precedes the switched-in Trace's random pick.
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]},
            {"species":"porygon2","level":50,"ability":"trace","moves":["recover"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","item":"quickclaw","moves":["dragonclaw"]},
            {"species":"pikachu","level":50,"ability":"static","moves":["growl"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(
        &[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)],
        &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)],
    );
    let log = b.rng.recording_log().unwrap();
    let qc = log.iter().position(|e| matches!(e.space, crate::rng::DrawSpace::UniformRange(5))).expect("Quick Claw roll");
    let trace = log.iter().position(|e| matches!(e.space, crate::rng::DrawSpace::UniformRange(2))).expect("Trace pick");
    assert!(qc < trace, "Quick Claw at {qc}, Trace at {trace}");
}

#[test]
fn thermal_exchange_raises_attack_on_a_fire_hit_and_blocks_burn() {
    // PS data/abilities.ts:4990 thermalexchange: onDamagingHit Fire move ->
    // boost({atk: 1}); onSetStatus: no burn.
    let mut b = singles(
        r#"[{"species":"baxcalibur","level":50,"ability":"thermalexchange","moves":["protect","bulkup"],"evs":{"hp":252}}]"#,
        r#"[{"species":"arcanine","level":50,"ability":"justified","moves":["flamethrower","willowisp"]}]"#,
        1,
    );
    b.step(&[mv(0, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p1.team[0].boosts[0], 2, "Bulk Up +1, Thermal Exchange +1");
    b.step(&[mv(0, 1, None)], &[mv(0, 1, Some(t(SideRef::P1, 0)))]);
    assert!(!matches!(b.p1.team[0].status, Status::Burn));
}

#[test]
fn rage_fist_gains_fifty_power_per_hit_taken() {
    // PS data/moves.ts:14583 ragefist basePowerCallback:
    // min(350, 50 + 50 * timesAttacked); timesAttacked counts each hit
    // (data/mods/champions/scripts.ts:566), reset on switch
    // (scripts.ts:169).
    let run = |foe_move: u8| {
        let mut b = singles(
            r#"[{"species":"annihilape","level":50,"ability":"defiant","moves":["ragefist","splash"],"evs":{"hp":252}}]"#,
            r#"[{"species":"slowbro","level":50,"ability":"owntempo","moves":["watergun","splash"],"evs":{"hp":252,"def":252}}]"#,
            5,
        );
        for _ in 0..2 {
            b.step(&[mv(0, 1, None)], &[mv(0, foe_move, Some(t(SideRef::P1, 0)))]);
        }
        let hp = b.p2.team[0].current_hp;
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 1, None)]);
        (hp - b.p2.team[0].current_hp) as u32
    };
    let (fresh, after_two) = (run(1), run(0));
    assert!(after_two * 2 > fresh * 3, "150 BP ({after_two}, or a KO) vs 50 BP ({fresh})");
}

#[test]
fn storm_throw_always_crits_without_a_crit_roll() {
    // PS data/moves.ts stormthrow `willCrit: true`: getDamage sets
    // moveHit.crit without drawing (sim/battle-actions.ts:1638), so the hit
    // ignores the target's Def boost.
    let run = |def_boost: i8| {
        let mut b = singles(
            r#"[{"species":"throh","level":50,"ability":"guts","moves":["stormthrow"]}]"#,
            r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
            9,
        );
        b.p2.team[0].boosts[1] = def_boost;
        b.set_rng(crate::rng::Rng::recording(9));
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
        let crit_draws = b.rng.recording_log().unwrap().iter().filter(|e| e.key.decision == crate::rng::RngDecision::Crit).count();
        (b.p2.team[0].stats.hp - b.p2.team[0].current_hp, crit_draws)
    };
    let (plain, draws) = run(0);
    assert_eq!(draws, 0);
    assert_eq!(run(6).0, plain, "a crit ignores +6 Def");
}

#[test]
fn feint_hits_through_protect_and_lifts_it_for_the_partner() {
    // PS feint: no `protect` flag, so Protect's onTryHit lets it through;
    // breaksProtect removes the target's protect volatile in
    // hitStepBreakProtect (sim/battle-actions.ts:755), so a later move this
    // turn connects.
    let mut b = doubles(
        r#"[{"species":"weavile","level":50,"ability":"pressure","moves":["feint"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["bodyslam"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["protect"]},
            {"species":"pikachu","level":50,"ability":"static","moves":["growl"]}]"#,
        1,
    );
    let full = b.p2.team[0].stats.hp;
    b.step(
        &[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 0)))],
        &[mv(0, 0, None), mv(1, 0, None)],
    );
    let lost = full - b.p2.team[0].current_hp;
    assert!(lost > 0, "Feint and Body Slam both land");
    assert!(!b.p2.team[0].is_protected_this_turn());
}

#[test]
fn phantom_force_strikes_through_protect() {
    // PS phantomforce: breaksProtect and no `protect` flag.
    let mut b = singles(
        r#"[{"species":"dragapult","level":50,"ability":"clearbody","moves":["phantomforce"],"evs":{"spe":252}}]"#,
        r#"[{"species":"gengar","level":50,"ability":"cursedbody","moves":["protect","calmmind"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 1, None)]);
    let hp = b.p2.team[0].current_hp;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(b.p2.team[0].current_hp < hp, "the second-turn strike lands through Protect");
}

#[test]
fn psychic_fangs_shatters_the_targets_screens() {
    // PS data/moves.ts psychicfangs onTryHit: the target side loses Reflect,
    // Light Screen and Aurora Veil before the hit.
    let mut b = singles(
        r#"[{"species":"bruxish","level":50,"ability":"strongjaw","moves":["psychicfangs","splash"]}]"#,
        r#"[{"species":"grimmsnarl","level":50,"ability":"prankster","moves":["reflect","lightscreen"],"evs":{"hp":252}}]"#,
        1,
    );
    b.step(&[mv(0, 1, None)], &[mv(0, 0, None)]);
    assert!(b.p2.conditions.reflect_turns > 0);
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 1, None)]);
    assert_eq!(b.p2.conditions.reflect_turns, 0);
    assert_eq!(b.p2.conditions.light_screen_turns, 0);
}

#[test]
fn stab_follows_the_current_type_after_protean() {
    // PS modifyDamage STAB: pokemon.hasType(type) ||
    // pokemon.getTypes(false, true).includes(type); getTypes reads the
    // current `types`, which setType (Protean on Protect) replaced, so the
    // original Grass type no longer gives STAB.
    let run = |ability: &str| {
        let p1 = format!(r#"[{{"species":"meowscarada","level":50,"ability":"{ability}","moves":["protect","flowertrick"]}}]"#);
        let mut b = singles(&p1, r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#, 4);
        b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
        b.step(&[mv(0, 1, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
        b.p2.team[0].stats.hp - b.p2.team[0].current_hp
    };
    let (protean, overgrow) = (run("protean"), run("overgrow"));
    assert!(protean * 4 < overgrow * 3, "Normal-type Protean user: {protean} vs Grass STAB {overgrow}");
}

#[test]
fn feint_rolls_accuracy_against_a_protecting_target() {
    // Protect doesn't stop Feint in hitStepTryHitEvent, so hitStepAccuracy
    // rolls for it as for any 100%-accurate move.
    let mut b = singles(
        r#"[{"species":"weavile","level":50,"ability":"pressure","moves":["feint"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["protect"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(accuracy_rolls(&b, data::move_id::FEINT), 1);
}

#[test]
fn gooey_slows_a_contact_attacker() {
    // PS data/abilities.ts:1642 gooey onDamagingHit: contact ->
    // boost({spe: -1}, source, target).
    let mut b = singles(
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonclaw","earthquake"]}]"#,
        r#"[{"species":"goodra","level":50,"ability":"gooey","moves":["splash"],"evs":{"hp":252}}]"#,
        1,
    );
    b.step(&[mv(0, 1, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].boosts[4], 0, "Earthquake makes no contact");
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].boosts[4], -1);
}

#[test]
fn rain_dish_heals_a_sixteenth_in_rain() {
    // PS data/abilities.ts:3759 raindish onWeather: heal(baseMaxhp / 16) in
    // rain.
    let mut b = singles(
        r#"[{"species":"pelipper","level":50,"ability":"raindish","moves":["raindance"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonclaw"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    let m = &b.p1.team[0];
    let hp_after_hit_guess = m.current_hp;
    let mut b2 = singles(
        r#"[{"species":"pelipper","level":50,"ability":"keeneye","moves":["raindance"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonclaw"]}]"#,
        1,
    );
    b2.step(&[mv(0, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(hp_after_hit_guess, b2.p1.team[0].current_hp + m.stats.hp / 16);
}

#[test]
fn ice_body_heals_a_sixteenth_in_snow() {
    // PS data/abilities.ts:1955 icebody onWeather: heal(baseMaxhp / 16) in
    // snow.
    let run = |ability: &str| {
        let p1 = format!(r#"[{{"species":"glalie","level":50,"ability":"{ability}","moves":["snowscape"]}}]"#);
        let mut b = singles(&p1, r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonclaw"]}]"#, 1);
        b.step(&[mv(0, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
        (b.p1.team[0].current_hp, b.p1.team[0].stats.hp)
    };
    let ((body, max), (plain, _)) = (run("icebody"), run("innerfocus"));
    assert_eq!(body, plain + max / 16);
}

#[test]
fn double_shock_spends_the_users_electric_type() {
    // PS data/moves.ts doubleshock: self.onHit turns Electric into '???';
    // onTryMove fails a user that isn't Electric.
    let mut b = singles(
        r#"[{"species":"pawmot","level":50,"ability":"ironfist","moves":["doubleshock"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    let (types, n) = b.p1.team[0].effective_types();
    assert_eq!((n, types[0]), (1, 6), "Electric/Fighting -> Fighting");
    let hp = b.p2.team[0].current_hp;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].current_hp, hp, "a non-Electric user's Double Shock fails");
}

#[test]
fn hard_press_power_follows_the_targets_hp() {
    // PS data/moves.ts hardpress basePowerCallback: 100 at full HP, scaling
    // down with the target's remaining HP.
    let mut b = singles(
        r#"[{"species":"archaludon","level":50,"ability":"stamina","moves":["hardpress"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(b.p2.team[0].current_hp < b.p2.team[0].stats.hp, "100 BP at full HP");
}

#[test]
fn ice_spinner_clears_the_terrain() {
    // PS data/moves.ts icespinner onAfterHit: this.field.clearTerrain().
    let mut b = singles(
        r#"[{"species":"weavile","level":50,"ability":"pressure","moves":["icespinner"]}]"#,
        r#"[{"species":"pincurchin","level":50,"ability":"electricsurge","moves":["splash"],"evs":{"hp":252}}]"#,
        1,
    );
    assert!(!matches!(b.terrain, crate::terrain::Terrain::None));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(matches!(b.terrain, crate::terrain::Terrain::None));
}

#[test]
fn seed_sower_sets_grassy_terrain_when_hit() {
    // PS data/abilities.ts:4119 seedsower onDamagingHit:
    // this.field.setTerrain('grassyterrain').
    let mut b = singles(
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonclaw"]}]"#,
        r#"[{"species":"arboliva","level":50,"ability":"seedsower","moves":["splash"],"evs":{"hp":252}}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(matches!(b.terrain, crate::terrain::Terrain::Grassy));
}

#[test]
fn curious_medicine_clears_the_allys_stat_changes_on_entry() {
    // PS data/abilities.ts:772 curiousmedicine onStart: adjacent allies'
    // boosts are cleared.
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]},
            {"species":"pikachu","level":50,"ability":"static","moves":["growl"]},
            {"species":"slowking","level":50,"ability":"curiousmedicine","moves":["splash"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["swordsdance"]},
            {"species":"raichu","level":50,"ability":"static","moves":["growl"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_ne!(b.p1.team[0].boosts, [0; 7]);
    b.step(&[mv(0, 0, None), Choice::Switch { actor_slot: 1, team_index: 2 }], &[mv(0, 0, None), mv(1, 0, None)]);
    // Curse ran again after the switch: only this turn's +1/+1/-1 remain.
    assert_eq!((b.p1.team[0].boosts[1], b.p1.team[0].boosts[4]), (1, -1));
}

#[cfg(feature = "ps-rng")]
#[test]
fn ps_rng_modify_damage_shuffles_tied_screen_handlers() {
    // PS getDamage -> modifyDamage runs runEvent('ModifyDamage') after the
    // damage roll; its handler list holds every side's Reflect / Light Screen
    // / Aurora Veil `onAnyModifyDamage` (subOrder 4, no speed), so two screens
    // tie and speedSort shuffles them: one random(0, 2).
    let p1 = TeamBuilder::from_json(r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["ironhead","splash"]}]"#).unwrap();
    let p2 = TeamBuilder::from_json(r#"[{"species":"grimmsnarl","level":50,"ability":"prankster","moves":["reflect","lightscreen"],"evs":{"hp":252}}]"#).unwrap();
    let mut rng = Rng::ps("sodium,0000000000000000000000000000000a").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    let mut b = Battle::with_rng(BattleConfig { format: Format::Singles, seed: 0 }, rng, p1, p2);
    b.step(&[mv(0, 1, None)], &[mv(0, 0, None)]);
    b.step(&[mv(0, 1, None)], &[mv(0, 1, None)]);
    let _ = b.rng_mut().ps_mut().unwrap().take_trace();
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 1, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    let i = trace.iter().position(|d| d.op == "damage").unwrap_or_else(|| panic!("{:?}", trace.iter().map(|d| (d.op, d.a, d.b, d.move_id)).collect::<Vec<_>>()));
    assert_eq!((trace[i + 1].op, trace[i + 1].a, trace[i + 1].b), ("shuffle", 0, 2), "{trace:#?}");
}

#[test]
fn defiant_triggers_once_per_stat_parting_shot_lowers() {
    // PS boost() runs AfterEachBoost once per stat it changed
    // (sim/battle.ts boost loop), so Parting Shot's -1 Atk / -1 SpA gives a
    // Defiant target two +2 Atk rebounds.
    let mut b = singles(
        r#"[{"species":"incineroar","level":50,"ability":"intimidate","moves":["partingshot"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
        r#"[{"species":"kingambit","level":50,"ability":"defiant","moves":["swordsdance"]}]"#,
        1,
    );
    let start = b.p2.team[0].boosts[0]; // Intimidate -1, Defiant +2
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    // Swords Dance +2; Parting Shot -1 then +2 +2.
    assert_eq!(b.p2.team[0].boosts[0], (start + 2 - 1 + 4).min(6));
}

#[test]
fn roost_grounds_a_flying_type_for_the_rest_of_the_turn() {
    // PS data/moves.ts roost: self volatile `roost` (duration 1) whose
    // onType drops Flying; isGrounded (sim/pokemon.ts) reads
    // hasType('Flying'), so Grassy Terrain heals the roosting Corviknight at
    // the end of the turn.
    let mut b = singles(
        r#"[{"species":"corviknight","level":50,"ability":"pressure","moves":["roost"]}]"#,
        r#"[{"species":"rillaboom","level":50,"ability":"grassysurge","moves":["splash"]}]"#,
        1,
    );
    b.p1.team[0].current_hp = 40;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    let max = b.p1.team[0].stats.hp;
    assert_eq!(b.p1.team[0].current_hp, 40 + max / 2 + max / 16);
}

#[cfg(feature = "ps-rng")]
#[test]
fn ps_rng_earthquake_rolls_for_the_ally_first() {
    // PS getMoveTargets for `allAdjacent` puts adjacentAllies() before
    // adjacentFoes() (sim/pokemon.ts:808-811) and each hit step walks that
    // list, so the ally's damage roll is the first one.
    let p1 = TeamBuilder::from_json(r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["earthquake"]},
        {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#).unwrap();
    let p2 = TeamBuilder::from_json(r#"[{"species":"heatran","level":50,"ability":"flashfire","moves":["splash"]},
        {"species":"blissey","level":50,"ability":"naturalcure","moves":["splash"]}]"#).unwrap();
    let mut rng = Rng::ps("sodium,0000000000000000000000000000000a").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    let mut b = Battle::with_rng(BattleConfig { format: Format::Doubles, seed: 0 }, rng, p1, p2);
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    let first = trace.iter().find(|d| d.op == "damage").map(|d| d.target);
    assert_eq!(first, Some(1), "p1b's roll first");
}

#[test]
fn steel_beam_costs_half_max_hp_rounded() {
    // PS data/moves.ts steelbeam: this.damage(Math.round(source.maxhp / 2)).
    let mut b = singles(
        r#"[{"species":"archaludon","level":50,"ability":"stamina","moves":["steelbeam"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
        1,
    );
    let max = b.p1.team[0].stats.hp;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(max - b.p1.team[0].current_hp, (max + 1) / 2, "max HP {max}");
}

#[test]
fn confusion_self_hit_roll_bucket_zero_is_the_minimum() {
    // PS getConfusionDamage (sim/battle-actions.ts:1850) runs randomizer:
    // floor(base * (100 - random(16)) / 100). An engine damage bucket b is
    // the (85 + b)% roll, so bucket 0 is 85% and bucket 15 is 100%.
    // L50, 40 BP, Atk 100 / Def 100: base = floor(22 * 40 * 100 / 100 / 50) + 2 = 19.
    assert_eq!(crate::damage::confusion_self_hit_damage_for_bucket(50, 100, 0, 100, 0, 0), 16);
    assert_eq!(crate::damage::confusion_self_hit_damage_for_bucket(50, 100, 0, 100, 0, 15), 19);
}

#[test]
fn solar_beam_is_halved_in_rain() {
    // PS data/moves.ts solarbeam onBasePower: chainModify(0.5) in rain,
    // sand or snow (:17249).
    let run = |weather: crate::weather::Weather| {
        let mut b = singles(
            r#"[{"species":"venusaur","level":50,"ability":"overgrow","item":"powerherb","moves":["solarbeam"]}]"#,
            r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
            3,
        );
        b.set_weather(weather);
        b.weather_turns = 5;
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
        b.p2.team[0].stats.hp - b.p2.team[0].current_hp
    };
    let (clear, rain) = (run(crate::weather::Weather::None), run(crate::weather::Weather::Rain));
    assert!(rain * 10 < clear * 6, "rain {rain} vs clear {clear}");
}

#[test]
fn infestation_traps_the_target_it_hit() {
    // PS partiallytrapped is the move's volatileStatus: it lands on the hit
    // target, not the first foe.
    let mut b = doubles(
        r#"[{"species":"toxapex","level":50,"ability":"regenerator","moves":["infestation"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]}]"#,
        r#"[{"species":"sneasler","level":50,"ability":"poisontouch","moves":["swordsdance"]},
            {"species":"golisopod","level":50,"ability":"emergencyexit","moves":["swordsdance"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 1))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.p2.team[1].volatiles.has(crate::pokemon::VolatileKind::PartialTrap));
    assert!(!b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::PartialTrap));
}

#[test]
fn scrappy_fighting_moves_hit_ghosts() {
    // PS data/abilities.ts scrappy onModifyMove: ignoreImmunity for
    // Fighting and Normal, so Ghost's immunity counts as neutral.
    let mut b = singles(
        r#"[{"species":"sirfetchd","level":50,"ability":"scrappy","moves":["closecombat"]}]"#,
        r#"[{"species":"gholdengo","level":50,"ability":"goodasgold","moves":["nastyplot"],"evs":{"hp":252}}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(b.p2.team[0].current_hp < b.p2.team[0].stats.hp);
}

#[test]
fn status_secondaries_match_ps_data() {
    // Chances from PS data/moves.ts `secondary` (Champions dex, a5df8274).
    use Status::{Burn, Paralysis, Poison, Toxic};
    for (slug, want) in [
        ("matchagotcha", Some((Burn, 20))),
        ("sludgewave", Some((Poison, 10))),
        ("zingzap", Option::None),
        ("blueflare", Some((Burn, 20))),
        ("searingshot", Some((Burn, 30))),
        ("spark", Some((Paralysis, 30))),
        ("infernalparade", Some((Burn, 30))),
        ("inferno", Some((Burn, 100))),
        ("poisonfang", Some((Toxic, 50))),
        ("pyroball", Some((Burn, 10))),
        ("volttackle", Some((Paralysis, 10))),
        ("bounce", Some((Paralysis, 30))),
        ("shellsidearm", Some((Poison, 20))),
        ("scald", Some((Burn, 30))),
    ] {
        assert_eq!(status_secondary(slug, true), want, "{slug}");
    }
}

#[test]
fn flinch_chances_match_ps_data() {
    // PS data/moves.ts `secondary: { chance, volatileStatus: 'flinch' }`.
    for (slug, want) in [
        ("zenheadbutt", 20),
        ("extrasensory", 10),
        ("hyperfang", 10),
        ("snore", 30),
        ("rollingkick", 30),
        ("steamroller", 30),
        ("zingzap", 30),
        ("iciclecrash", 30),
        ("mountaingale", 30),
        ("skyattack", 30),
        ("rockslide", 30),
        ("darkpulse", 20),
    ] {
        assert_eq!(flinch_chance(slug, true), Some(want), "{slug}");
    }
}

#[test]
fn stat_drop_secondaries_cover_ps_data() {
    // PS data/moves.ts `secondary: { chance, boosts }` (stat index, delta, chance).
    for (slug, want) in [
        ("crushclaw", (1, -1, 50)),
        ("firelash", (1, -1, 100)),
        ("lowsweep", (4, -1, 100)),
        ("razorshell", (1, -1, 50)),
        ("skittersmack", (2, -1, 100)),
        ("icywind", (4, -1, 100)),
    ] {
        assert_eq!(stat_drop_secondary(slug, true), Some(want), "{slug}");
    }
}

#[test]
fn speed_boost_works_on_the_first_full_turn_after_a_faint_replacement() {
    // PS speedboost onResidual: `if (pokemon.activeTurns) boost`. A
    // replacement enters at the end of the turn, before endTurn increments
    // activeTurns, so it boosts at the next residual.
    let mut b = singles(
        r#"[{"species":"magikarp","level":50,"ability":"swiftswim","moves":["splash"]},
            {"species":"blaziken","level":50,"ability":"speedboost","moves":["protect"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonclaw"],"evs":{"atk":252}}]"#,
        1,
    );
    b.p1.team[0].current_hp = 1;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert!(b.needs_replacements());
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[]);
    b.step(&[mv(0, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p1.team[1].boosts[4], 1);
}
// PS speedboost onResidual: `if (pokemon.activeTurns) boost`. A faint
// replacement enters before endTurn increments activeTurns, so it boosts at
// the next residual (here with the replacement as its own decision step).
#[test]
fn speed_boost_after_a_replacement_between_turns() {
    let mut b = doubles(
        r#"[{"species":"magikarp","level":50,"ability":"swiftswim","moves":["splash"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["curse"]},
            {"species":"blaziken","level":50,"ability":"speedboost","moves":["protect"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonclaw"],"evs":{"atk":252}},
            {"species":"pikachu","level":50,"ability":"static","moves":["growl"]}]"#,
        1,
    );
    b.decision_phases = true;
    b.p1.team[0].current_hp = 1;
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
    assert!(b.needs_replacements());
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 2 }, Choice::Pass { actor_slot: 1 }], &[]);
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
    assert_eq!(b.p1.team[2].boosts[4], 1);
}
