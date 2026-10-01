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

// ---- Imprison ----
//
// PS data/moves.ts:9489 imprison: the user gains the `imprison` volatile;
// foes can't select a move the user also knows (onFoeDisableMove), and one
// already queued fails in BeforeMove (onFoeBeforeMove, priority 4).

const IMPRISON_USER: &str = r#"[{"species":"jolteon","level":50,"ability":"voltabsorb","moves":["imprison","tackle","protect"]}]"#;
const IMPRISON_FOE: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["tackle","protect","splash"]}]"#;

#[test]
fn imprison_stops_a_shared_move_queued_this_turn() {
    let mut b = singles(IMPRISON_USER, IMPRISON_FOE, 1);
    let hp = b.p1.team[0].current_hp;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p1.team[0].current_hp, hp, "Snorlax's Tackle is imprisoned");
}

#[test]
fn imprison_disables_shared_moves_for_foes() {
    let mut b = singles(IMPRISON_USER, IMPRISON_FOE, 1);
    b.step(&[mv(0, 0, None)], &[mv(0, 2, None)]);
    let slots: Vec<u8> = b
        .legal_choices(SideRef::P2, 0)
        .into_iter()
        .filter_map(|c| match c {
            Choice::Move { move_slot, .. } => Some(move_slot),
            _ => None,
        })
        .collect();
    assert!(!slots.contains(&0) && !slots.contains(&1) && slots.contains(&2), "{slots:?}");
}

// ---- Minimize ----
//
// PS data/moves.ts:11926 minimize volatile: moves with flags.minimize (Body
// Slam, Stomp, Heat Crash, Heavy Slam, Dragon Rush, Flying Press ...)
// deal 2x (onSourceModifyDamage) and cannot miss (onAccuracy) against it.

fn body_slam_after(foe_move: u8, seed: u64) -> u32 {
    let mut b = singles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["bodyslam","splash"]}]"#,
        r#"[{"species":"blissey","level":50,"ability":"naturalcure","moves":["minimize","splash"],"evs":{"hp":252,"def":252}}]"#,
        seed,
    );
    b.step(&[mv(0, 1, None)], &[mv(0, foe_move, None)]);
    let hp = b.p2.team[0].current_hp;
    b.step(&[mv(0, 0, foe())], &[mv(0, 1, None)]);
    (hp - b.p2.team[0].current_hp) as u32
}

#[test]
fn minimize_doubles_body_slam_damage() {
    let (minimized, plain) = (body_slam_after(0, 2), body_slam_after(1, 2));
    assert!(minimized * 2 > plain * 3, "2x ({minimized}) vs ({plain})");
}

#[test]
fn body_slam_cannot_miss_a_minimized_target() {
    for seed in 1..30 {
        assert!(body_slam_after(0, seed) > 0, "seed {seed}: Body Slam missed a +2 evasion Minimize user");
    }
}

// ---- Thunder Wave's Ground immunity ----

#[test]
fn thunder_wave_fails_on_a_ground_type_without_an_accuracy_roll() {
    // PS data/moves.ts:19601 thunderwave `ignoreImmunity: false`:
    // hitStepTypeImmunity (sim/battle-actions.ts:654) runs before
    // hitStepAccuracy, so a Ground type is immune and nothing is rolled.
    let mut b = singles(
        r#"[{"species":"jolteon","level":50,"ability":"voltabsorb","moves":["thunderwave"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["splash"]}]"#,
        1,
    );
    b.set_rng(crate::rng::Rng::recording(3));
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].status, Status::None);
    let rolls = b
        .rng
        .recording_log()
        .unwrap()
        .iter()
        .filter(|e| e.key.move_id == data::move_id::THUNDERWAVE)
        .count();
    assert_eq!(rolls, 0);
}

// ---- Shed Tail ----
//
// PS data/moves.ts:16161 shedtail: fails without a switch target, with a
// Substitute up, or at or below half HP (onTryHit, NOT_FAIL); otherwise
// adds a Substitute (floor(maxhp/4) HP), pays ceil(maxhp/2) HP and switches
// out; the replacement gets only the Substitute (copyVolatileFrom
// 'shedtail', sim/pokemon.ts:1246: no boosts).

const SHED_USER: &str = r#"[{"species":"cyclizar","level":50,"ability":"shedskin","moves":["shedtail","swordsdance"]},
    {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#;

#[test]
fn shed_tail_passes_a_substitute_to_the_replacement() {
    let mut b = singles(SHED_USER, LR_FOE, 1);
    b.step(&[mv(0, 1, None)], &[mv(0, 0, None)]);
    let max = b.p1.team[0].stats.hp;
    b.step(&[mv(0, 0, None), Choice::Switch { actor_slot: 0, team_index: 1 }], &[mv(0, 0, None)]);
    assert_eq!(b.p1.active[0], 1, "Snorlax came in");
    assert_eq!(b.p1.team[0].current_hp, max - max.div_ceil(2), "paid half its HP");
    assert_eq!(b.p1.team[1].substitute_hp(), max / 4, "Substitute from Cyclizar's max HP");
    assert_eq!(b.p1.team[1].boosts[0], 0, "boosts stay behind");
}

#[test]
fn shed_tail_fails_at_half_hp_or_less() {
    let mut b = singles(SHED_USER, LR_FOE, 1);
    let max = b.p1.team[0].stats.hp;
    b.p1.team[0].current_hp = max.div_ceil(2);
    b.step(&[mv(0, 0, None), Choice::Switch { actor_slot: 0, team_index: 1 }], &[mv(0, 0, None)]);
    assert_eq!(b.p1.active[0], 0, "no switch");
    assert_eq!(b.p1.team[0].current_hp, max.div_ceil(2));
}

// ---- Bug Bite / Pluck ----
//
// PS data/moves.ts bugbite / pluck onHit: if the user has HP and the target
// holds a Berry it can lose (takeItem: Sticky Hold blocks), the user eats it
// (singleEvent 'Eat') and the target loses it.

#[test]
fn bug_bite_eats_the_targets_berry() {
    let mut b = singles(
        r#"[{"species":"scizor","level":50,"ability":"technician","moves":["bugbite"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","item":"sitrusberry","moves":["splash"],"evs":{"hp":252,"def":252}}]"#,
        1,
    );
    let max = b.p1.team[0].stats.hp;
    b.p1.team[0].current_hp = max / 2;
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].item_id, u16::MAX, "Sitrus taken");
    assert_eq!(b.p1.team[0].current_hp, max / 2 + max / 4, "Scizor ate it");
}

#[test]
fn sticky_hold_keeps_the_berry_from_pluck() {
    let mut b = singles(
        r#"[{"species":"staraptor","level":50,"ability":"reckless","moves":["pluck"]}]"#,
        r#"[{"species":"gastrodon","level":50,"ability":"stickyhold","item":"sitrusberry","moves":["splash"],"evs":{"hp":252,"def":252}}]"#,
        1,
    );
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert_eq!(b.p2.team[0].item_id, data::item_id::SITRUSBERRY);
}

// ---- Charge (the move) ----

#[test]
fn charge_raises_spd_and_doubles_the_next_electric_move() {
    // PS data/moves.ts charge: boosts spd +1, volatileStatus 'charge'
    // (onBasePower x2 for Electric, removed after an Electric move).
    let run = |first: u8| {
        let mut b = singles(
            r#"[{"species":"ampharos","level":50,"ability":"static","moves":["charge","thunderbolt","splash"]}]"#,
            r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252,"spd":252}}]"#,
            4,
        );
        b.step(&[mv(0, first, None)], &[mv(0, 0, None)]);
        let spd = b.p1.team[0].boosts[3];
        let hp = b.p2.team[0].current_hp;
        b.step(&[mv(0, 1, foe())], &[mv(0, 0, None)]);
        (spd, (hp - b.p2.team[0].current_hp) as u32, b.p1.team[0].is_charged())
    };
    let ((spd, charged, left), (_, plain, _)) = (run(0), run(2));
    assert_eq!(spd, 1);
    assert!(charged * 2 > plain * 3, "x2 ({charged}) vs ({plain})");
    assert!(!left, "used up by Thunderbolt");
}

// ---- Normal Gem ----

#[test]
fn normal_gem_boosts_and_is_used_up() {
    // PS data/items.ts:4324 normalgem onSourceTryPrimaryHit: a Normal
    // damaging move uses the Gem; the gem condition's onBasePower is
    // chainModify([5325, 4096]) (data/conditions.ts:463).
    let run = |item: &str| {
        let mut b = singles(
            &format!(r#"[{{"species":"staraptor","level":50,"ability":"intimidate","item":"{item}","moves":["doubleedge"]}}]"#),
            r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252,"def":252}}]"#,
            5,
        );
        let hp = b.p2.team[0].current_hp;
        b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
        ((hp - b.p2.team[0].current_hp) as u32, b.p1.team[0].item_id)
    };
    let ((gem, left), (plain, _)) = (run("normalgem"), run("lightball"));
    assert_eq!(left, u16::MAX, "Gem used");
    assert!(gem * 100 > plain * 120, "x1.3 ({gem}) vs ({plain})");
}

// ---- Beak Blast ----
//
// PS data/moves.ts:1119 beakblast: priorityChargeCallback (an order-107
// action before any move) adds the beakblast volatile; a contact move that
// hits the holder burns its user (condition onHit); onAfterMove removes it.

#[test]
fn beak_blast_burns_a_contact_attacker_before_it_moves() {
    let mut b = singles(
        r#"[{"species":"toucannon","level":50,"ability":"keeneye","moves":["beakblast"],"evs":{"hp":252,"def":252}}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["bodyslam","earthquake"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p2.team[0].status, Status::Burn, "Body Slam made contact during the charge");
}

#[test]
fn beak_blast_ignores_non_contact_moves() {
    let mut b = singles(
        r#"[{"species":"toucannon","level":50,"ability":"keeneye","moves":["beakblast"],"evs":{"hp":252,"def":252}}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["bodyslam","rockslide"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, foe())], &[mv(0, 1, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p2.team[0].status, Status::None);
}

#[cfg(feature = "ps-rng")]
#[test]
fn ps_rng_beak_blast_charge_action_draws_a_random_target() {
    // resolveAction unshifts a targetless priorityChargeMove action
    // (sim/battle-queue.ts:242) whose getRandomTarget (:266) draws in doubles.
    let draws = |m: &str| {
        let p1 = TeamBuilder::from_json(&format!(
            r#"[{{"species":"toucannon","level":50,"moves":["{m}"]}},{{"species":"snorlax","level":50,"moves":["calmmind"]}}]"#
        ))
        .unwrap();
        let p2 = TeamBuilder::from_json(
            r#"[{"species":"blissey","level":50,"moves":["calmmind"]},{"species":"chansey","level":50,"moves":["calmmind"]}]"#,
        )
        .unwrap();
        let mut rng = crate::rng::Rng::ps("sodium,00000000000000000000000000000007").unwrap();
        rng.ps_mut().unwrap().enable_trace();
        let mut b = Battle::with_rng(BattleConfig { format: Format::Doubles, seed: 0 }, rng, p1, p2);
        b.step(
            &[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)],
            &[mv(0, 0, None), mv(1, 0, None)],
        );
        let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
        trace.iter().filter(|d| d.op == "random_target").count()
    };
    assert_eq!(draws("beakblast"), draws("drillpeck") + 1);
}

// ---- Payback ----

#[test]
fn payback_doubles_against_a_target_that_already_moved() {
    // PS data/moves.ts:13190 payback basePowerCallback: x2 unless the target
    // is newly switched in or will still move this turn.
    let run = |foe_json: &str| {
        let mut b = singles(
            r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["payback"]}]"#,
            foe_json,
            6,
        );
        let hp = b.p2.team[0].current_hp;
        b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
        (hp - b.p2.team[0].current_hp) as u32
    };
    // Both foes are Snorlax-bulk; the faster one moves before Payback.
    let after = run(r#"[{"species":"snorlax","level":50,"ability":"thickfat","nature":"jolly","moves":["splash"],"evs":{"hp":252,"spe":252}}]"#);
    let before = run(r#"[{"species":"snorlax","level":50,"ability":"thickfat","nature":"brave","moves":["splash"],"evs":{"hp":252},"ivs":{"spe":0}}]"#);
    assert!(after * 2 > before * 3, "x2 ({after}) vs ({before})");
}

// ---- Self-heal rounding ----

#[test]
fn half_heal_moves_round_half_up() {
    // PS runMoveEffects (sim/battle-actions.ts:1209): `heal: [1, 2]` heals
    // Math.round(maxhp / 2).
    let mut b = singles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["slackoff"]}]"#,
        LR_FOE,
        1,
    );
    let max = b.p1.team[0].stats.hp;
    assert_eq!(max % 2, 1, "odd max HP for this test");
    b.p1.team[0].current_hp = 1;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].current_hp, 1 + max.div_ceil(2));
}

#[test]
fn weather_heals_use_ps_modify() {
    // PS data/moves.ts moonlight onHit: heal(this.modify(maxhp, factor)),
    // factor 0.667 in sun: tr((maxhp * 2732 + 2047) / 4096).
    let mut b = singles(
        r#"[{"species":"clefable","level":50,"ability":"magicguard","moves":["moonlight"],"evs":{"hp":12}}]"#,
        r#"[{"species":"torkoal","level":50,"ability":"drought","moves":["splash"]}]"#,
        1,
    );
    let max = b.p1.team[0].stats.hp as u32;
    b.p1.team[0].current_hp = 1;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].current_hp as u32, 1 + (max * 2732 + 2047) / 4096, "max {max}");
}
