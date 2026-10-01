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

fn doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

// ---- Battle start: the leads' SwitchIn handlers run in Speed order ----
//
// sim/battle-actions.ts:172-184 runSwitch batches every pending switch-in
// and runs their SwitchIn handlers through a speed-sorted fieldEvent, so the
// slowest Surge / weather setter goes last and its field effect stays.

#[test]
fn battle_start_surges_resolve_fastest_first_so_the_slowest_wins() {
    // P1 Pincurchin (Electric Surge, base 15 Spe) is far slower than P2
    // Rillaboom (Grassy Surge, base 85): Grassy goes up first, then Electric.
    let b = doubles(
        r#"[{"species":"pincurchin","level":50,"ability":"electricsurge","moves":["protect"]},{"species":"snorlax","level":50,"moves":["protect"]}]"#,
        r#"[{"species":"rillaboom","level":50,"ability":"grassysurge","moves":["protect"]},{"species":"chansey","level":50,"moves":["protect"]}]"#,
        1,
    );
    assert_eq!(b.terrain, crate::terrain::Terrain::Electric);
}

// ---- Sucker Punch checks the target it actually hits ----
//
// runMove retargets a move aimed at a fainted foe (sim/battle.ts:2437
// getTarget) and useMoveInner applies redirection before the move's onTry
// (data/moves.ts suckerpunch onTry), so the "is it attacking" check reads the
// retargeted / redirected target.

#[test]
fn sucker_punch_at_a_fainted_foe_checks_the_retargeted_foe() {
    // Weavile's Ice Shard KOs the level-1 Magikarp first; Kingambit's Sucker
    // Punch aimed at that slot retargets to Snorlax, which is attacking.
    let mut b = doubles(
        r#"[{"species":"kingambit","level":50,"moves":["suckerpunch"]},{"species":"weavile","level":50,"moves":["iceshard"]}]"#,
        r#"[{"species":"magikarp","level":1,"moves":["splash"]},{"species":"snorlax","level":50,"moves":["tackle"]}]"#,
        3,
    );
    b.step(
        &[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 0)))],
        &[mv(0, 0, None), mv(1, 0, Some(t(SideRef::P1, 1)))],
    );
    assert!(!b.p2.team[0].is_alive(), "Ice Shard should KO the Magikarp");
    let snorlax = &b.p2.team[1];
    assert!(snorlax.current_hp < snorlax.stats.hp, "Sucker Punch should hit the attacking Snorlax");
}
