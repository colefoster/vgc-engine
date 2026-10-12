//! Accuracy round 11 (docs/accuracy/fix-log.md): phase-3 sim-diff mechanics.

use super::*;
use crate::choice::{Choice, Target};
use crate::team::TeamBuilder;

fn t(side: SideRef, slot: u8) -> Target {
    Target { side, slot }
}

fn mv(slot: u8, move_slot: u8, target: Option<Target>) -> Choice {
    Choice::Move { actor_slot: slot, move_slot, target }
}

fn logged_doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    use crate::protocol::{ProtocolMode, ProtocolOptions};
    let opts = ProtocolOptions { timestamp: Some(1), ..ProtocolOptions::new(ProtocolMode::Ps) };
    Battle::with_protocol(
        BattleConfig { format: Format::Doubles, seed },
        crate::rng::Rng::new(seed),
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
        &opts,
        "doubles",
        None,
    )
}

fn log_lines(b: &Battle) -> Vec<String> {
    b.protocol_view(crate::protocol::ProtocolView::Omniscient).unwrap()
}

fn doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

#[test]
fn spread_drain_heals_per_target_including_an_ally() {
    // PS sim/battle.ts spreadDamage: each target's drain is
    // `heal(round(damage * drain))` right after that target's damage, so a
    // spread drain move heals once per target, ally included.
    let mut b = doubles(
        r#"[{"species":"bellibolt","level":50,"ability":"static","moves":["paraboliccharge"]},
            {"species":"chesnaught","level":50,"ability":"overgrow","moves":["splash"]}]"#,
        r#"[{"species":"kommoo","level":50,"ability":"soundproof","moves":["splash"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#,
        7,
    );
    b.p1.team[0].current_hp = 60;
    let before = [b.p1.team[1].current_hp, b.p2.team[0].current_hp, b.p2.team[1].current_hp];
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    let after = [b.p1.team[1].current_hp, b.p2.team[0].current_hp, b.p2.team[1].current_hp];
    assert!(before.iter().zip(after.iter()).all(|(x, y)| x > y), "all three hit: {before:?} {after:?}");
    let heal: u16 = (0..3).map(|i| (before[i] - after[i]).div_ceil(2)).sum();
    assert_eq!(b.p1.team[0].current_hp, 60 + heal, "{before:?} {after:?}");
}

#[test]
fn spirit_shackle_traps_until_its_user_leaves() {
    // PS data/moves.ts:17629 spiritshackle secondary onHit:
    // addVolatile('trapped', source, move, 'trapper'); the link ends when
    // the source switches out. Sim-diff b00297.
    let p1 = r#"[{"species":"decidueye","level":50,"ability":"overgrow","moves":["spiritshackle","splash"]},
        {"species":"pikachu","level":50,"ability":"static","moves":["splash"]},
        {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#;
    let p2 = r#"[{"species":"rillaboom","level":50,"ability":"overgrow","moves":["splash"],"evs":{"hp":252,"def":252}},
        {"species":"gengar","level":50,"ability":"cursedbody","moves":["splash"],"evs":{"hp":252,"def":252}},
        {"species":"amoonguss","level":50,"ability":"regenerator","moves":["splash"]}]"#;
    let mut b = logged_doubles(p1, p2, 3);
    b.step(
        &[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)],
        &[mv(0, 0, None), mv(1, 0, None)],
    );
    assert!(b.is_trapped(SideRef::P2, 0), "Rillaboom trapped");
    assert!(log_lines(&b).iter().any(|l| l == "|-activate|p2a: Rillaboom|trapped"));
    let mut lc = Vec::new();
    b.legal_choices_into(SideRef::P2, 0, &mut lc);
    assert!(!lc.iter().any(|c| matches!(c, Choice::Switch { .. })), "no switch while trapped");
    // A Ghost is immune (runStatusImmunity('trapped')).
    b.step(
        &[mv(0, 0, Some(t(SideRef::P2, 1))), mv(1, 0, None)],
        &[mv(0, 0, None), mv(1, 0, None)],
    );
    assert!(!b.is_trapped(SideRef::P2, 1), "Gengar not trapped");
    assert!(!b.p2.team[1].volatiles.has(crate::pokemon::VolatileKind::Trapped));
    // Decidueye switches out: the trap ends.
    b.step(
        &[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)],
        &[mv(0, 0, None), mv(1, 0, None)],
    );
    assert!(!b.is_trapped(SideRef::P2, 0), "released");
    assert!(!b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::Trapped));
}

#[test]
fn receiver_takes_the_fainted_allys_ability() {
    // PS data/abilities.ts:3787 receiver onAllyFaint -> setAbility(ability,
    // target), which prints `-ability` and runs the new ability's Start
    // (sim/pokemon.ts:1934, :1943). Sim-diff b00280.
    let p1 = r#"[{"species":"passimian","level":50,"ability":"receiver","moves":["splash"]},
        {"species":"incineroar","level":50,"ability":"intimidate","moves":["splash"]}]"#;
    let p2 = r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["closecombat"],"evs":{"atk":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#;
    let mut b = logged_doubles(p1, p2, 3);
    b.p1.team[1].current_hp = 1;
    let atk_before = b.p2.team[0].boosts[0];
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    assert!(b.p1.team[1].fainted, "Incineroar KO'd");
    assert_eq!(b.p1.team[0].effective_ability_id(), data::ability_id::INTIMIDATE);
    assert_eq!(b.p2.team[0].boosts[0], atk_before - 1, "received Intimidate starts");
    let log = log_lines(&b);
    let faint = log.iter().position(|l| l == "|faint|p1b: Incineroar").expect("faint line");
    assert_eq!(log[faint + 1], "|-ability|p1a: Passimian|Intimidate|Receiver|[from] ability: Receiver|[of] p1b: Incineroar");
    // The same fainted ally never fires it twice (a later switch-in beside it).
    b.p1.team[0].ability_override = data::ability_id::RECEIVER;
    b.step(&[mv(0, 0, None), Choice::Pass { actor_slot: 1 }], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
    assert_eq!(b.p1.team[0].effective_ability_id(), data::ability_id::RECEIVER);
}

#[test]
fn receiver_ignores_noreceiver_abilities() {
    let p1 = r#"[{"species":"passimian","level":50,"ability":"receiver","moves":["splash"]},
        {"species":"gardevoir","level":50,"ability":"trace","moves":["splash"]}]"#;
    let p2 = r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["closecombat"],"evs":{"atk":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#;
    let mut b = doubles(p1, p2, 3);
    b.p1.team[1].current_hp = 1;
    b.p1.team[1].ability_override = data::ability_id::TRACE;
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    assert!(b.p1.team[1].fainted);
    assert_eq!(b.p1.team[0].effective_ability_id(), data::ability_id::RECEIVER);
}

#[test]
fn protean_skips_a_stall_move_that_fails_its_prepare_hit() {
    // sim/battle-actions.ts:826-828: the move's PrepareHit (`willAct() &&
    // StallMove`) runs before runEvent('PrepareHit') (Protean,
    // data/abilities.ts:3497). Sim-diff b00984: Meowscarada's last-moving
    // Protect fails and stays Grass/Dark.
    // Everyone else Protects first (same +4 bracket, faster), so nothing
    // acts after Meowscarada: `willAct()` is false.
    let p1 = r#"[{"species":"meowscarada","level":50,"ability":"protean","moves":["protect"]},
        {"species":"pikachu","level":50,"ability":"static","moves":["protect","splash"]}]"#;
    let p2 = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["protect","splash"]},
        {"species":"snorlax","level":50,"ability":"thickfat","moves":["protect","splash"]}]"#;
    let mut b = logged_doubles(p1, p2, 3);
    b.p1.team[0].stats.spe = 1;
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(!b.p1.team[0].protean_used, "no retype on a failed Protect");
    assert!(!log_lines(&b).iter().any(|l| l.contains("typechange")));
    // A Protect that works retypes first, then `-singleturn`.
    b.p1.team[0].stats.spe = 400;
    b.step(&[mv(0, 0, None), mv(1, 1, None)], &[mv(0, 1, None), mv(1, 1, None)]);
    assert!(b.p1.team[0].protean_used);
    let log = log_lines(&b);
    let tc = log.iter().position(|l| l == "|-start|p1a: Meowscarada|typechange|Normal|[from] ability: Protean").expect("typechange");
    assert_eq!(log[tc + 1], "|-singleturn|p1a: Meowscarada|Protect");
}

#[test]
fn spread_drain_logs_each_heal_in_ps_target_order() {
    // allAdjacent lists the user's allies before its foes (PS
    // sim/pokemon.ts:809-813) and spreadDamage heals after each target in
    // that order, so each `-heal|...|[from] drain` shows the HP after that
    // target's drain only. Sim-diff b00029 (Parabolic Charge into the
    // user's Chesnaught and a foe).
    let mut b = logged_doubles(
        r#"[{"species":"bellibolt","level":50,"ability":"static","moves":["paraboliccharge"]},
            {"species":"chesnaught","level":50,"ability":"overgrow","moves":["splash"]}]"#,
        r#"[{"species":"kommoo","level":50,"ability":"soundproof","moves":["splash"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#,
        7,
    );
    b.p1.team[0].current_hp = 60;
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    let heals: Vec<(u16, String)> = log_lines(&b)
        .iter()
        .filter(|l| l.starts_with("|-heal|p1a: Bellibolt|") && l.contains("[from] drain"))
        .map(|l| {
            let f: Vec<&str> = l.split('|').collect();
            (f[3].split('/').next().unwrap().parse().unwrap(), f[5].to_string())
        })
        .collect();
    assert_eq!(heals.len(), 3, "{heals:?}");
    assert_eq!(heals[0].1, "[of] p1b: Chesnaught", "{heals:?}");
    assert!(heals.windows(2).all(|w| w[0].0 < w[1].0), "{heals:?}");
    assert_eq!(heals[2].0, b.p1.team[0].current_hp);
}
