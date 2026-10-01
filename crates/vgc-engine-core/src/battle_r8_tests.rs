//! Accuracy round 8 (docs/accuracy/fix-log.md): move-result history, used
//! move slots, the target's queued priority, and the round-7 tail.

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

#[allow(dead_code)]
fn doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

// ---- Stomping Tantrum / Temper Flare: moveLastTurnResult ----
//
// PS data/moves.ts:18050 stompingtantrum (and :19186 temperflare)
// basePowerCallback: `if (pokemon.moveLastTurnResult === false) return
// move.basePower * 2`. moveThisTurnResult is set by useMove / runMove
// (sim/battle-actions.ts:262, :274, :285, :371-374, :507, :616) and moved to
// moveLastTurnResult at the start of each turn (sim/battle.ts:1674);
// switching in clears both (sim/pokemon.ts:1545).

const TANTRUM_USER: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["stompingtantrum","toxic","growl","protect"]},
    {"species":"chansey","level":50,"ability":"naturalcure","moves":["splash"]}]"#;
const TANTRUM_FOE: &str = r#"[{"species":"registeel","level":50,"ability":"lightmetal","moves":["splash","protect","fakeout"],"evs":{"hp":252,"def":252},"nature":"impish"},
    {"species":"registeel","level":50,"ability":"lightmetal","moves":["splash"],"evs":{"hp":252,"def":252},"nature":"impish"}]"#;

/// Damage Stomping Tantrum deals on the last turn after the scripted turns.
fn tantrum_damage(script: &[(Choice, Choice)], seed: u64) -> u32 {
    let mut b = singles(TANTRUM_USER, TANTRUM_FOE, seed);
    for (p1, p2) in script {
        b.step(&[*p1], &[*p2]);
    }
    let foe = b.p2.active[0] as usize;
    let hp = b.p2.team[foe].current_hp;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    (hp - b.p2.team[foe].current_hp) as u32
}

fn foe() -> Option<Target> {
    Some(t(SideRef::P2, 0))
}

#[test]
fn stomping_tantrum_doubles_after_a_move_that_failed() {
    // Toxic into a Steel type fails (immune): moveThisTurnResult false.
    let failed = tantrum_damage(&[(mv(0, 1, foe()), mv(0, 0, None))], 3);
    let landed = tantrum_damage(&[(mv(0, 2, foe()), mv(0, 0, None))], 3);
    assert!(failed * 2 > landed * 3, "150 BP ({failed}) vs 75 BP ({landed})");
}

#[test]
fn stomping_tantrum_is_not_doubled_after_hitting_protect() {
    // Protect's onTryHit returns NOT_FAIL: the move result is null
    // (sim/battle-actions.ts:616), which does not double the power.
    let blocked = tantrum_damage(&[(mv(0, 0, foe()), mv(0, 1, None))], 3);
    let landed = tantrum_damage(&[(mv(0, 2, foe()), mv(0, 0, None))], 3);
    assert!(blocked * 3 < landed * 4 && landed * 3 < blocked * 4, "75 BP both ({blocked}, {landed})");
}

#[test]
fn stomping_tantrum_doubles_after_a_flinch() {
    // Flinch's onBeforeMove returns false: moveThisTurnResult = false
    // (sim/battle-actions.ts:262).
    let flinched = tantrum_damage(&[(mv(0, 0, foe()), mv(0, 2, Some(t(SideRef::P1, 0))))], 3);
    let landed = tantrum_damage(&[(mv(0, 0, foe()), mv(0, 0, None))], 3);
    assert!(flinched * 2 > landed * 3, "150 BP ({flinched}) vs 75 BP ({landed})");
}

#[test]
fn stomping_tantrum_doubles_after_a_failed_protect() {
    // Protect fails when no later action will act (data/moves.ts protect
    // onPrepareHit: `!!this.queue.willAct()`): the foe switches.
    let sw = Choice::Switch { actor_slot: 0, team_index: 1 };
    let failed = tantrum_damage(&[(mv(0, 3, None), sw)], 3);
    let landed = tantrum_damage(&[(mv(0, 2, foe()), sw)], 3);
    assert!(failed * 2 > landed * 3, "150 BP ({failed}) vs 75 BP ({landed})");
}

#[test]
fn stomping_tantrum_only_looks_at_the_previous_turn() {
    // The failure is moved to moveLastTurnResult once, then overwritten by
    // the next turn's (successful) Growl.
    let later = tantrum_damage(&[(mv(0, 1, foe()), mv(0, 0, None)), (mv(0, 2, foe()), mv(0, 0, None))], 3);
    let landed = tantrum_damage(&[(mv(0, 2, foe()), mv(0, 0, None))], 3);
    assert!(later * 3 < landed * 4 && landed * 3 < later * 4, "75 BP both ({later}, {landed})");
}

#[test]
fn switching_out_clears_the_failed_move() {
    // clearVolatile (sim/pokemon.ts:1545) resets both results on switch-in.
    let sw = |i| Choice::Switch { actor_slot: 0, team_index: i };
    let back = tantrum_damage(
        &[(mv(0, 1, foe()), mv(0, 0, None)), (sw(1), mv(0, 0, None)), (sw(0), mv(0, 0, None))],
        3,
    );
    let landed = tantrum_damage(&[(mv(0, 2, foe()), mv(0, 0, None))], 3);
    assert!(back * 3 < landed * 4 && landed * 3 < back * 4, "75 BP both ({back}, {landed})");
}

#[test]
fn temper_flare_doubles_after_a_move_that_failed() {
    let run = |first: u8| {
        let mut b = singles(
            r#"[{"species":"arcanine","level":50,"ability":"intimidate","moves":["temperflare","toxic","growl"]}]"#,
            TANTRUM_FOE,
            3,
        );
        b.step(&[mv(0, first, foe())], &[mv(0, 0, None)]);
        let hp = b.p2.team[0].current_hp;
        b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
        (hp - b.p2.team[0].current_hp) as u32
    };
    let (failed, landed) = (run(1), run(2));
    assert!(failed * 2 > landed * 3, "150 BP ({failed}) vs 75 BP ({landed})");
}


// ---- Last Resort: moveSlot.used ----
//
// PS data/moves.ts:10075 lastresort onTry: fails unless the user knows at
// least two moves and every other move slot is `used`; deductPP sets `used`
// (sim/pokemon.ts:892) and switching in clears it (sim/battle-actions.ts:139).

const LR_USER: &str = r#"[{"species":"eevee","level":50,"ability":"adaptability","moves":["lastresort","growl","tailwhip"]},
    {"species":"chansey","level":50,"ability":"naturalcure","moves":["splash"]}]"#;
const LR_FOE: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252,"def":252}}]"#;

fn last_resort_hits(script: &[Choice]) -> bool {
    let mut b = singles(LR_USER, LR_FOE, 1);
    for c in script {
        b.step(&[*c], &[mv(0, 0, None)]);
    }
    let hp = b.p2.team[0].current_hp;
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    b.p2.team[0].current_hp < hp
}

#[test]
fn last_resort_fails_until_every_other_move_was_used() {
    assert!(!last_resort_hits(&[]), "no other move used");
    assert!(!last_resort_hits(&[mv(0, 1, foe())]), "Tail Whip not used yet");
    assert!(last_resort_hits(&[mv(0, 1, foe()), mv(0, 2, foe())]), "Growl and Tail Whip used");
}

#[test]
fn last_resort_forgets_used_moves_on_switch_out() {
    let sw = |i| Choice::Switch { actor_slot: 0, team_index: i };
    assert!(!last_resort_hits(&[mv(0, 1, foe()), mv(0, 2, foe()), sw(1), sw(0)]));
}

#[test]
fn last_resort_fails_as_the_only_move() {
    let mut b = singles(
        r#"[{"species":"eevee","level":50,"ability":"adaptability","moves":["lastresort"]}]"#,
        LR_FOE,
        1,
    );
    let hp = b.p2.team[0].current_hp;
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].current_hp, hp);
}

// ---- Upper Hand: the target's queued move priority ----
//
// PS data/moves.ts:20196 upperhand onTry: fails unless the target will
// still move this turn with a damaging move whose priority is above 0.1
// (the move's priority after ModifyPriority, sim/battle.ts getActionSpeed);
// 100% flinch.

fn upper_hand(foe_json: &str, foe_move: u8) -> (bool, bool) {
    let mut b = singles(
        r#"[{"species":"lucario","level":50,"ability":"justified","moves":["upperhand"],"evs":{"hp":252,"def":252}}]"#,
        foe_json,
        1,
    );
    let (hp, foe_hp) = (b.p1.team[0].current_hp, b.p2.team[0].current_hp);
    b.step(&[mv(0, 0, foe())], &[mv(0, foe_move, Some(t(SideRef::P1, 0)))]);
    (b.p2.team[0].current_hp < foe_hp, b.p1.team[0].current_hp < hp)
}

const UH_FOE: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["quickattack","bodyslam"],"evs":{"hp":252}}]"#;

#[test]
fn upper_hand_hits_and_flinches_a_priority_attacker() {
    assert_eq!(upper_hand(UH_FOE, 0), (true, false), "hit, and Quick Attack flinched");
}

#[test]
fn upper_hand_fails_against_a_non_priority_attack() {
    assert_eq!(upper_hand(UH_FOE, 1), (false, true), "fails; Body Slam lands");
}

#[test]
fn upper_hand_fails_against_a_priority_status_move() {
    let foe = r#"[{"species":"whimsicott","level":50,"ability":"prankster","moves":["taunt"],"evs":{"hp":252}}]"#;
    assert!(!upper_hand(foe, 0).0, "Prankster Taunt is +1 but Status");
}
