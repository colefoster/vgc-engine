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
