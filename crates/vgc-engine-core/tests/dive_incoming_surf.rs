//! Dive's underwater state doubles Surf and Whirlpool against the diver.
//!
//! PS a5df8274 data/moves.ts:3747-3810, dive condition: onInvulnerability
//! lets Surf / Whirlpool reach the user, and onSourceModifyDamage
//! (:3787-3791) returns `this.chainModify(2)` for them. That is the final
//! ModifyDamage chain, not base power: with no other ModifyDamage modifier
//! in play the hit is exactly twice the ordinary one (pokeRound(d * 2)).
//! Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Dive_(move)>.
//!
//! No Gulp Missile here: Golduck dives (Damp, Water), slow Snorlax answers.

use vgc_engine_core::{Battle, BattleConfig, Choice, Format, Rng, RngEvent, SideRef, Target, TeamBuilder};

const GOLDUCK: &str = r#"[{"species":"golduck","level":50,"gender":"M","ability":"damp","nature":"hardy","evs":{"hp":252,"spd":4},"moves":["dive","splash"]}]"#;
const SNORLAX: &str = r#"[{"species":"snorlax","level":50,"gender":"M","ability":"thickfat","nature":"hardy","evs":{"hp":4},"moves":["surf","whirlpool","splash"]}]"#;

const DIVE: u8 = 0;
const SPLASH: u8 = 1;
const SURF: u8 = 0;
const WHIRLPOOL: u8 = 1;

/// The foe's move draws, pinned: accuracy hit, no crit, top damage roll.
fn pinned(seed: u64) -> Battle {
    let events = vec![RngEvent::PercentRoll(1), RngEvent::Crit(false), RngEvent::DamageRoll(15)];
    Battle::with_rng(
        BattleConfig { format: Format::Singles, seed },
        Rng::oracle_partial(events, seed),
        TeamBuilder::from_json(GOLDUCK).unwrap(),
        TeamBuilder::from_json(SNORLAX).unwrap(),
    )
}

fn choice(move_slot: u8, foe: SideRef) -> Choice {
    Choice::Move { actor_slot: 0, move_slot, target: Some(Target { side: foe, slot: 0 }) }
}

/// Golduck uses `own` while Snorlax uses `foe_move`; the hit's damage.
fn hit(own: u8, foe_move: u8) -> u16 {
    let mut b = pinned(7);
    b.step(&[choice(own, SideRef::P2)], &[choice(foe_move, SideRef::P1)]);
    let g = &b.p1.team[0];
    if own == DIVE {
        assert_eq!(g.semi_invuln, 2, "precondition: underwater");
    }
    assert!(g.last_damage_taken > 0, "precondition: the move reached Golduck");
    g.last_damage_taken
}

fn assert_doubled(normal: u16, diving: u16, what: &str) {
    // A 2x ModifyDamage modifier on an otherwise unmodified chain doubles
    // the final damage exactly.
    assert_eq!(diving, 2 * normal, "{what}: diving vs twice the ordinary {normal}");
}

#[test]
fn surf_into_a_diver_doubles() {
    let normal = hit(SPLASH, SURF);
    let diving = hit(DIVE, SURF);
    assert_doubled(normal, diving, "Surf");
}

#[test]
fn whirlpool_into_a_diver_doubles() {
    let normal = hit(SPLASH, WHIRLPOOL);
    let diving = hit(DIVE, WHIRLPOOL);
    assert_doubled(normal, diving, "Whirlpool");
}

#[test]
fn an_ordinary_target_takes_ordinary_surf_damage() {
    // Control: the boost belongs to the dive state, not to Surf or Golduck.
    let a = hit(SPLASH, SURF);
    let b = hit(SPLASH, SURF);
    assert_eq!(a, b);
    assert!(pinned(7).p1.team[0].semi_invuln == 0);
}

#[cfg(feature = "chance")]
#[test]
fn chance_frontier_doubles_every_surf_outcome() {
    let frontier = |own: u8| {
        let b = Battle::new(
            BattleConfig { format: Format::Singles, seed: 7 },
            TeamBuilder::from_json(GOLDUCK).unwrap(),
            TeamBuilder::from_json(SNORLAX).unwrap(),
        );
        let f = b.step_chance(&[choice(own, SideRef::P2)], &[choice(SURF, SideRef::P1)], 7);
        assert_eq!(f.unmatched_total, 0);
        let total: f64 = f.outcomes.iter().map(|o| o.prob).sum();
        assert!((total - 1.0).abs() < 1e-9, "probabilities sum to {total}");
        let dmg = |o: &vgc_engine_core::chance::ChanceOutcome| o.battle.p1.team[0].stats.hp - o.battle.p1.team[0].current_hp;
        (f.outcomes.iter().map(dmg).max().unwrap(), f.outcomes.iter().map(dmg).filter(|&d| d > 0).min().unwrap())
    };
    let (normal_max, normal_min) = frontier(SPLASH);
    let (diving_max, diving_min) = frontier(DIVE);
    assert_doubled(normal_max, diving_max, "frontier max");
    assert_doubled(normal_min, diving_min, "frontier min");
}
