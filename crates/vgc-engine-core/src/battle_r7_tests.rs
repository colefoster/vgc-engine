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
