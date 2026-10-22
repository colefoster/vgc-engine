//! Opt-in `Battle::ally_targets`: `legal_choices` offers the doubles partner
//! for `normal` / `any` moves, as PS validTargetLoc (sim/battle.ts:2399-2429)
//! accepts.

use super::*;
use crate::choice::{Choice, Target};
use crate::team::TeamBuilder;

// p1a: tackle (normal), bravebird (any), mefirst (adjacentFoe), earthquake
// (allAdjacent). p1b: helpinghand (adjacentAlly), swordsdance (self),
// rockslide (allAdjacentFoes), pollenpuff (normal).
const P1: &str = r#"[
    {"species":"staraptor","level":50,"teratype":"flying","moves":["tackle","bravebird","mefirst","earthquake"]},
    {"species":"ribombee","level":50,"moves":["helpinghand","swordsdance","rockslide","pollenpuff"]},
    {"species":"snorlax","level":50,"moves":["tackle"]}
]"#;
const P2: &str = r#"[
    {"species":"blissey","level":50,"moves":["tackle"]},
    {"species":"chansey","level":50,"moves":["tackle"]}
]"#;

fn doubles(ally_targets: bool) -> Battle {
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 1 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    b.ally_targets = ally_targets;
    b
}

fn ally_moves(lc: &[Choice], ally: Target) -> Vec<(u8, &'static str)> {
    lc.iter()
        .filter_map(|c| match *c {
            Choice::Move { move_slot, target: Some(t), .. } if t == ally => Some((move_slot, "move")),
            Choice::Terastallize { move_slot, target: Some(t), .. } if t == ally => Some((move_slot, "tera")),
            Choice::MegaEvolve { move_slot, target: Some(t), .. } if t == ally => Some((move_slot, "mega")),
            _ => None,
        })
        .collect()
}

const P1A: Target = Target { side: SideRef::P1, slot: 0 };
const P1B: Target = Target { side: SideRef::P1, slot: 1 };

#[test]
fn flag_off_offers_no_ally_target() {
    let b = doubles(false);
    for slot in 0..2 {
        let lc = b.legal_choices(SideRef::P1, slot);
        assert!(lc.iter().all(|c| !matches!(c,
            Choice::Move { target: Some(t), .. } | Choice::Terastallize { target: Some(t), .. }
                if t.side == SideRef::P1)));
    }
}

#[test]
fn flag_on_is_flag_off_plus_ally_choices() {
    let (off, on) = (doubles(false), doubles(true));
    for slot in 0..2 {
        let a = off.legal_choices(SideRef::P1, slot);
        let b = on.legal_choices(SideRef::P1, slot);
        let ally = Target { side: SideRef::P1, slot: 1 - slot };
        let without: Vec<Choice> = b
            .iter()
            .copied()
            .filter(|c| !matches!(c, Choice::Move { target: Some(t), .. } | Choice::Terastallize { target: Some(t), .. } | Choice::MegaEvolve { target: Some(t), .. } if *t == ally))
            .collect();
        assert_eq!(without, a, "flag on only adds ally choices (slot {slot})");
    }
}

#[test]
fn ally_offered_for_normal_and_any_only() {
    let b = doubles(true);
    // Staraptor: tackle (normal) and bravebird (any) — not mefirst
    // (adjacentFoe) or earthquake (spread). Tera variant of each.
    assert_eq!(
        ally_moves(&b.legal_choices(SideRef::P1, 0), P1B),
        vec![(0, "move"), (0, "tera"), (1, "move"), (1, "tera")]
    );
    // Ribombee: pollenpuff (normal) only — helpinghand is ally-auto,
    // swordsdance self, rockslide spread.
    assert_eq!(ally_moves(&b.legal_choices(SideRef::P1, 1), P1A), vec![(3, "move"), (3, "tera")]);
    // Ally-auto Helping Hand stays targetless.
    assert!(b.legal_choices(SideRef::P1, 1).contains(&Choice::Move { actor_slot: 1, move_slot: 0, target: None }));
}

#[test]
fn ally_choices_follow_the_moves_foe_choices() {
    let b = doubles(true);
    let lc = b.legal_choices(SideRef::P1, 0);
    let tackle: Vec<Option<Target>> = lc
        .iter()
        .filter_map(|c| match *c {
            Choice::Move { move_slot: 0, target, .. } | Choice::Terastallize { move_slot: 0, target, .. } => Some(target),
            _ => None,
        })
        .collect();
    let foe = |slot| Some(Target { side: SideRef::P2, slot });
    // Per move: foe a (move, tera), foe b (move, tera), then ally (move, tera).
    assert_eq!(tackle, vec![foe(0), foe(0), foe(1), foe(1), Some(P1B), Some(P1B)]);
    // Contiguous per move, and the first choice is still a foe-targeted move.
    assert!(matches!(lc[0], Choice::Move { target: Some(t), .. } if t.side == SideRef::P2));
}

#[test]
fn fainted_or_commanding_partner_is_still_offered() {
    // PS validTargetLoc / chooseMove never look at the partner's state
    // (sim/battle.ts:2399-2429); the move then fails or misses.
    let mut b = doubles(true);
    b.p1.team[1].current_hp = 0;
    b.p1.team[1].fainted = true;
    assert_eq!(ally_moves(&b.legal_choices(SideRef::P1, 0), P1B).len(), 4, "fainted partner");
    let mut b = doubles(true);
    b.p1.team[1].commanding = true;
    assert_eq!(ally_moves(&b.legal_choices(SideRef::P1, 0), P1B).len(), 4, "commanding partner");
}

#[test]
fn effective_target_gates_the_partner() {
    // sim/pokemon.ts:997-1008: heal-blocked Pollen Puff is adjacentFoe,
    // non-Ghost Curse is self.
    let mut b = doubles(true);
    b.p1.active_mon_mut(1).unwrap().set_heal_block(2);
    assert!(ally_moves(&b.legal_choices(SideRef::P1, 1), P1A).is_empty(), "heal-blocked Pollen Puff");
    let p1 = r#"[{"species":"snorlax","level":50,"moves":["curse","acupressure"]},{"species":"gengar","level":50,"moves":["curse"]}]"#;
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, TeamBuilder::from_json(p1).unwrap(), TeamBuilder::from_json(P2).unwrap());
    b.ally_targets = true;
    // Snorlax: non-Ghost Curse is self, no partner.
    assert!(ally_moves(&b.legal_choices(SideRef::P1, 0), P1B).is_empty());
    // Ghost Curse keeps its data target `normal`: the partner is a valid pick.
    assert_eq!(ally_moves(&b.legal_choices(SideRef::P1, 1), P1A), vec![(0, "move"), (0, "tera")]);
}

#[test]
fn mega_variants_follow_foe_variants() {
    let p1 = r#"[{"species":"charizard","level":50,"item":"charizarditey","teratype":"fire","moves":["tackle"]},{"species":"snorlax","level":50,"moves":["tackle"]}]"#;
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, TeamBuilder::from_json(p1).unwrap(), TeamBuilder::from_json(P2).unwrap());
    b.ally_targets = true;
    let foe = |slot| Some(Target { side: SideRef::P2, slot });
    let got: Vec<(&str, Option<Target>)> = b
        .legal_choices(SideRef::P1, 0)
        .iter()
        .filter_map(|c| match *c {
            Choice::Move { target, .. } => Some(("move", target)),
            Choice::Terastallize { target, .. } => Some(("tera", target)),
            Choice::MegaEvolve { target, .. } => Some(("mega", target)),
            _ => None,
        })
        .collect();
    assert_eq!(got, vec![
        ("move", foe(0)), ("tera", foe(0)), ("mega", foe(0)),
        ("move", foe(1)), ("tera", foe(1)), ("mega", foe(1)),
        ("move", Some(P1B)), ("tera", Some(P1B)), ("mega", Some(P1B)),
    ]);
}

#[test]
fn singles_unaffected() {
    let mk = |on| {
        let mut b = Battle::new(
            BattleConfig { format: Format::Singles, seed: 1 },
            TeamBuilder::from_json(P1).unwrap(),
            TeamBuilder::from_json(P2).unwrap(),
        );
        b.ally_targets = on;
        b
    };
    assert_eq!(mk(true).legal_choices(SideRef::P1, 0), mk(false).legal_choices(SideRef::P1, 0));
}

#[test]
fn clone_keeps_flag_snapshots_are_unchanged() {
    let b = doubles(true);
    assert!(b.clone().ally_targets);
    // Not serialized: snapshots are byte-identical to flag-off ones and
    // load with it off.
    assert_eq!(serde_json::to_string(&b).unwrap(), serde_json::to_string(&doubles(false)).unwrap());
    let back: Battle = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
    assert!(!back.ally_targets);
}

#[test]
fn ally_targeted_tackle_hits_the_partner() {
    let mut b = doubles(true);
    let hp = b.p1.team[1].current_hp;
    let foe_hp = [b.p2.team[0].current_hp, b.p2.team[1].current_hp];
    let pass = |s| Choice::Move { actor_slot: s, move_slot: 1, target: None }; // swordsdance on p1b
    b.step(
        &[Choice::Move { actor_slot: 0, move_slot: 0, target: Some(P1B) }, pass(1)],
        &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
    );
    assert!(b.p1.team[1].current_hp < hp, "partner took the Tackle");
    assert_eq!([b.p2.team[0].current_hp, b.p2.team[1].current_hp], foe_hp);
}

#[test]
fn status_move_at_fainted_or_commanding_partner_does_nothing() {
    // sim/battle.ts:2464-2476 keeps the partner as the target: a fainted one
    // fails the move (sim/battle-actions.ts:510-513), a commanding one makes
    // it miss (sim/battle-actions.ts:625). Neither retargets a foe.
    let p1 = r#"[{"species":"staraptor","level":50,"moves":["charm"]},{"species":"snorlax","level":50,"moves":["tackle"]}]"#;
    for commanding in [false, true] {
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, TeamBuilder::from_json(p1).unwrap(), TeamBuilder::from_json(P2).unwrap());
        if commanding {
            b.p1.team[1].commanding = true;
        } else {
            b.p1.team[1].current_hp = 0;
            b.p1.team[1].fainted = true;
        }
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: Some(P1B) }, Choice::Pass { actor_slot: 1 }],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert_eq!([b.p2.team[0].boosts[0], b.p2.team[1].boosts[0], b.p1.team[1].boosts[0]], [0, 0, 0], "commanding={commanding}");
    }
}
