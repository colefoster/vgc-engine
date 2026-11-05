//! Damage numbers and PRNG draws found with tools/protocol/draw-trace.cjs.

use super::*;
use crate::choice::{Choice, Target};
use crate::team::TeamBuilder;

fn t(side: SideRef, slot: u8) -> Target {
    Target { side, slot }
}

fn mv(slot: u8, move_slot: u8, target: Option<Target>) -> Choice {
    Choice::Move { actor_slot: slot, move_slot, target }
}

fn doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

#[test]
fn a_round_moved_up_by_round_deals_double() {
    // data/moves.ts round basePowerCallback: `move.sourceEffect === 'round'`
    // doubles. Same user stats on both, so across 16 rolls and a crit the
    // chained hit is never below 1.13x; without the double it would be 1.0x.
    const P1: &str = r#"[{"species":"snorlax","level":50,"moves":["round"]},{"species":"snorlax","level":50,"moves":["round"]}]"#;
    const P2: &str = r#"[{"species":"blissey","level":50,"moves":["splash"]},{"species":"blissey","level":50,"moves":["splash"]}]"#;
    let mut doubled = 0;
    for seed in 0..8 {
        let mut b = doubles(P1, P2, seed);
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 1)))], &[mv(0, 0, None), mv(1, 0, None)]);
        let (a, c) = (b.p2.team[0].stats.hp - b.p2.team[0].current_hp, b.p2.team[1].stats.hp - b.p2.team[1].current_hp);
        let (lo, hi) = (a.min(c), a.max(c));
        assert!(lo > 0);
        if hi >= lo * 3 / 2 {
            doubled += 1;
        }
    }
    assert!(doubled >= 7, "{doubled}/8");
}
