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
