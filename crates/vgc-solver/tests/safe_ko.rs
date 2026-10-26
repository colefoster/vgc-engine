//! Shallow safe-KO search (`solve_safe_ko`).
//!
//! Run in release: `cargo test --release -p vgc-solver --test safe_ko`.
use std::time::{Duration, Instant};

use vgc_engine_core::{Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder};
use vgc_solver::{solve_safe_ko, SafeKoConfig, Truncation};

const SLACK: Duration = Duration::from_millis(150);

fn cfg(horizon: u32) -> SafeKoConfig {
    SafeKoConfig { horizon, ..SafeKoConfig::default() }
}

fn is_move(c: &Choice, slot: u8, move_slot: usize) -> bool {
    matches!(c, Choice::Move { actor_slot, move_slot: m, .. } if *actor_slot == slot && *m as usize == move_slot)
}

/// Garchomp's Earthquake OHKOs every Electric mon on the other side at any
/// roll; Corviknight is immune. Pikachu can Protect, Pichu can't, and every
/// switch-in is just as dead. Nothing on their side can KO ours this turn.
fn spread_ko_through_protect() -> Battle {
    let p1 = TeamBuilder::from_json(r#"[
        {"species":"garchomp","level":100,"ability":"roughskin","item":"lifeorb","nature":"adamant","evs":{"atk":252,"hp":252},"moves":["earthquake","dragonclaw","protect","swordsdance"]},
        {"species":"corviknight","level":100,"ability":"pressure","item":"leftovers","nature":"impish","evs":{"hp":252,"def":252},"moves":["roost","tailwind","protect","irondefense"]}
    ]"#).unwrap();
    let p2 = TeamBuilder::from_json(r#"[
        {"species":"pikachu","level":50,"ability":"static","item":"","nature":"hardy","moves":["thunderbolt","protect","quickattack","growl"]},
        {"species":"pichu","level":50,"ability":"static","item":"","nature":"hardy","moves":["thundershock","charm","sweetkiss","growl"]},
        {"species":"raichu","level":50,"ability":"static","item":"","nature":"hardy","moves":["thunderbolt","protect","quickattack","growl"]},
        {"species":"plusle","level":50,"ability":"plus","item":"","nature":"hardy","moves":["thunderbolt","protect","quickattack","growl"]}
    ]"#).unwrap();
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 7 }, p1, p2);
    b.p1.conditions.tera_used = true;
    b.p2.conditions.tera_used = true;
    b
}

#[test]
fn finds_a_forced_ko_through_protect_and_switch_replies() {
    let b = spread_ko_through_protect();
    let r = solve_safe_ko(&b, SideRef::P1, &cfg(1));
    assert_eq!(r.horizon_completed, 1);
    assert!(r.truncated.is_none());
    let chosen = r.chosen.expect("a safe KO exists");
    assert!(r.chosen_p.unwrap() >= 0.9, "p {:?}", r.chosen_p);
    assert!(chosen.iter().any(|c| is_move(c, 0, 0)), "chosen {chosen:?} is not Earthquake");
    // Dragon Claw into Pikachu is refuted by its Protect.
    let into_pika = |c: &Choice| matches!(c, Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t) } if t.side == SideRef::P2 && t.slot == 0);
    let claws: Vec<_> = r.actions.iter().filter(|a| a.joint.iter().any(into_pika)).collect();
    assert!(!claws.is_empty());
    for a in claws {
        assert!(!a.passed, "Dragon Claw into Protect passed: {:?} p {}", a.joint, a.p);
        assert!(a.refutation.is_some());
    }
    // Every Earthquake action passes (Corviknight can't hurt anything).
    for a in r.actions.iter().filter(|a| a.joint.iter().any(|c| is_move(c, 0, 0))) {
        assert!(a.passed, "{:?} p {}", a.joint, a.p);
    }
    // The same search for the other side finds nothing.
    let o = solve_safe_ko(&b, SideRef::P2, &cfg(1));
    assert_eq!(o.horizon_completed, 1);
    assert!(o.chosen.is_none());
}

fn singles(p1: &str, p2: &str) -> Battle {
    let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 11 }, TeamBuilder::from_json(p1).unwrap(), TeamBuilder::from_json(p2).unwrap());
    b.p1.conditions.tera_used = true;
    b.p2.conditions.tera_used = true;
    b
}

const CHOMP: &str = r#"[{"species":"garchomp","level":50,"ability":"roughskin","item":"","nature":"adamant","moves":["dragonclaw","splash","growl","leer"]}]"#;
const LAX: &str = r#"[{"species":"snorlax","level":100,"ability":"thickfat","item":"","nature":"hardy","moves":["growl","tailwhip","splash","leer"]}]"#;

fn p_at(hp: u16, horizon: u32) -> (f64, bool) {
    let mut b = singles(CHOMP, LAX);
    b.p2.team[0].current_hp = hp;
    let r = solve_safe_ko(&b, SideRef::P1, &cfg(horizon));
    assert_eq!(r.horizon_completed, horizon);
    let claw = r.actions.iter().find(|a| is_move(&a.joint[0], 0, 0)).unwrap();
    (claw.p, claw.passed)
}

#[test]
fn a_damage_roll_risk_drops_it_below_the_threshold() {
    // Low HP: every roll KOs. Scan up until the KO needs a high roll.
    assert!(p_at(1, 1).1, "1 HP Snorlax must be a safe KO");
    let max = singles(CHOMP, LAX).p2.team[0].stats.hp;
    let mut partial = None;
    for hp in 1..=max {
        let (p, passed) = p_at(hp, 1);
        if p < 0.9 {
            assert!(!passed);
            if p > 0.05 {
                partial = Some((hp, p));
            }
            break;
        }
    }
    let (hp, p) = partial.expect("the first failing HP is a roll-dependent KO");
    assert!(p < 0.9 && p > 0.05, "hp {hp} p {p}");
    let mut b = singles(CHOMP, LAX);
    b.p2.team[0].current_hp = hp;
    let r = solve_safe_ko(&b, SideRef::P1, &cfg(1));
    assert!(r.chosen.is_none(), "hp {hp} claw p {p}: chosen {:?} p {:?}; {:?}", r.chosen, r.chosen_p,
            r.actions.iter().map(|a| (a.joint.clone(), a.p)).collect::<Vec<_>>());
}

#[test]
fn horizon_two_finds_a_sure_two_hit_ko() {
    let max = singles(CHOMP, LAX).p2.team[0].stats.hp;
    let mut found = false;
    for hp in (1..=max).rev().step_by(3) {
        let (p1, _) = p_at(hp, 1);
        let (p2, passed2) = p_at(hp, 2);
        assert!(p2 + 1e-9 >= p1, "hp {hp}: horizon 2 {p2} < horizon 1 {p1}");
        if p1 < 0.05 && passed2 {
            found = true;
            break;
        }
    }
    assert!(found, "no HP where one hit never KOs and two always do");
}

/// Full-HP 4v4 doubles with spread moves, Protect, Fake Out and a 10-hit
/// Population Bomb (the budget-guarantee fixture's teams plus benches).
fn adversarial_doubles() -> Battle {
    let p1 = TeamBuilder::from_json(r#"[
        {"species":"maushold","level":50,"ability":"technician","item":"widelens","nature":"jolly","moves":["populationbomb","followme","protect","bite"]},
        {"species":"typhlosion","level":50,"ability":"blaze","item":"choicescarf","nature":"timid","moves":["heatwave","eruption","scorchingsands","shadowball"]},
        {"species":"amoonguss","level":50,"ability":"regenerator","item":"rockyhelmet","nature":"calm","moves":["spore","ragepowder","pollenpuff","protect"]},
        {"species":"dragonite","level":50,"ability":"multiscale","item":"lumberry","nature":"adamant","moves":["extremespeed","dragonclaw","protect","firepunch"]}
    ]"#).unwrap();
    let p2 = TeamBuilder::from_json(r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","rockslide","dragonclaw","protect"]},
        {"species":"incineroar","level":50,"ability":"intimidate","item":"sitrusberry","nature":"careful","moves":["fakeout","flareblitz","knockoff","partingshot"]},
        {"species":"rillaboom","level":50,"ability":"grassysurge","item":"assaultvest","nature":"adamant","moves":["fakeout","woodhammer","grassyglide","uturn"]},
        {"species":"urshifu","level":50,"ability":"unseenfist","item":"focussash","nature":"jolly","moves":["wickedblow","closecombat","suckerpunch","protect"]}
    ]"#).unwrap();
    Battle::new(BattleConfig { format: Format::Doubles, seed: 3 }, p1, p2)
}

#[test]
fn respects_the_deadline() {
    let b = adversarial_doubles();
    for ms in [20u64, 200, 1500] {
        for side in [SideRef::P1, SideRef::P2] {
            let t0 = Instant::now();
            let c = SafeKoConfig { deadline: Some(t0 + Duration::from_millis(ms)), ..cfg(2) };
            let r = solve_safe_ko(&b, side, &c);
            let el = t0.elapsed();
            assert!(el <= Duration::from_millis(ms) + SLACK, "{ms} ms budget took {el:?}");
            if r.horizon_completed < 2 {
                assert_eq!(r.truncated, Some(Truncation::Deadline), "{ms} ms: cut short without truncated");
            }
            if r.horizon_completed == 0 {
                assert!(r.chosen.is_none() && r.actions.is_empty());
            } else {
                if let Some(ch) = &r.chosen {
                    assert!(r.actions.iter().any(|a| &a.joint == ch && a.passed));
                }
            }
        }
    }
}

#[test]
fn replay_budget_is_a_hard_stop() {
    let b = adversarial_doubles();
    let r = solve_safe_ko(&b, SideRef::P1, &SafeKoConfig { replay_budget: Some(50), ..cfg(2) });
    assert_eq!(r.horizon_completed, 0);
    assert_eq!(r.truncated, Some(Truncation::ReplayBudget));
    assert!(r.chosen.is_none());
}
