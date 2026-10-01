//! Accuracy round 10 (docs/accuracy/fix-log.md).

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

#[test]
fn mega_sol_solar_beam_skips_the_charge() {
    // PS data/moves.ts:17238 solarbeam onTryMove skips the charge when
    // attacker.effectiveWeather() is sun; sim/pokemon.ts:2193 reports sun
    // during a Mega Sol user's move. Study 4598041866.
    let mut b = singles(
        r#"[{"species":"venusaur","level":50,"ability":"overgrow","moves":["solarbeam"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
        3,
    );
    b.p1.team[0].ability_id = data::ability_id::MEGASOL;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].charging_turns, 0);
    assert!(b.p2.team[0].current_hp < b.p2.team[0].stats.hp, "fired on turn 1");
}
