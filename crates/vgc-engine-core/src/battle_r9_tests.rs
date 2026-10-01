//! Accuracy round 9 (docs/accuracy/fix-log.md).

use super::*;
use crate::choice::{Choice, Target};
use crate::team::TeamBuilder;

fn t(side: SideRef, slot: u8) -> Target {
    Target { side, slot }
}

fn mv(slot: u8, move_slot: u8, target: Option<Target>) -> Choice {
    Choice::Move { actor_slot: slot, move_slot, target }
}

#[cfg(feature = "ps-rng")]
fn ps_doubles(p1: &str, p2: &str) -> Battle {
    let mut rng = crate::rng::Rng::ps("sodium,00000000000000000000000000000007").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    Battle::with_rng(
        BattleConfig { format: Format::Doubles, seed: 0 },
        rng,
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

#[cfg(feature = "ps-rng")]
#[test]
fn ps_rng_a_charged_move_resolves_at_its_stored_target_without_a_draw() {
    // sim/side.ts:675-688: a mon locked into a two-turn move gets an action
    // with targetLoc = its stored target, so resolveAction
    // (sim/battle-queue.ts) makes no getRandomTarget draw for it.
    let mut b = ps_doubles(
        r#"[{"species":"archaludon","level":50,"moves":["electroshot"]},{"species":"snorlax","level":50,"moves":["calmmind"]}]"#,
        r#"[{"species":"blissey","level":50,"moves":["calmmind"]},{"species":"chansey","level":50,"moves":["calmmind"]}]"#,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.p1.team[0].charging_turns > 0, "Electro Shot should be charging");
    let _ = b.rng_mut().ps_mut().unwrap().take_trace();
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    assert_eq!(trace.iter().filter(|d| d.op == "random_target").count(), 0);
}
