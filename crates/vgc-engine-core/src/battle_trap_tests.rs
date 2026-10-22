//! Move traps: Mean Look / Block, Jaw Lock, Fairy Lock (PS data/moves.ts,
//! data/conditions.ts `trapped`).

use super::*;
use crate::choice::{Choice, Target};
use crate::pokemon::VolatileKind;
use crate::team::TeamBuilder;

fn t(side: SideRef, slot: u8) -> Target {
    Target { side, slot }
}

fn mv(slot: u8, move_slot: u8, target: Option<Target>) -> Choice {
    Choice::Move { actor_slot: slot, move_slot, target }
}

fn foe() -> Option<Target> {
    Some(t(SideRef::P2, 0))
}

fn logged(format: Format, p1: &str, p2: &str, seed: u64) -> Battle {
    use crate::protocol::{ProtocolMode, ProtocolOptions};
    let opts = ProtocolOptions { timestamp: Some(1), ..ProtocolOptions::new(ProtocolMode::Ps) };
    let id = if format == Format::Singles { "singles" } else { "doubles" };
    Battle::with_protocol(
        BattleConfig { format, seed },
        crate::rng::Rng::new(seed),
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
        &opts,
        id,
        None,
    )
}

fn log_lines(b: &Battle) -> Vec<String> {
    b.protocol_view(crate::protocol::ProtocolView::Omniscient).unwrap()
}

fn can_switch(b: &Battle, side: SideRef, slot: u8) -> bool {
    let mut lc = Vec::new();
    b.legal_choices_into(side, slot, &mut lc);
    lc.iter().any(|c| matches!(c, Choice::Switch { .. }))
}

const TRAPPER: &str = r#"[{"species":"umbreon","level":50,"ability":"synchronize","moves":["meanlook","block","perishsong","splash"]},
    {"species":"blissey","level":50,"ability":"naturalcure","moves":["splash","roar"]}]"#;
const VICTIM: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash","uturn","batonpass","substitute"]},
    {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;

// ---- Mean Look / Block ----

#[test]
fn mean_look_traps_the_target_and_removes_its_switches() {
    // PS data/moves.ts:11492 meanlook onHit: target.addVolatile('trapped',
    // source, move, 'trapper'); data/conditions.ts trapped onStart
    // `-activate|trapped`, onTrapPokemon tryTrap().
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    assert!(can_switch(&b, SideRef::P2, 0));
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P2, 0));
    assert!(!can_switch(&b, SideRef::P2, 0), "no switch while trapped");
    assert!(log_lines(&b).iter().any(|l| l == "|-activate|p2a: Snorlax|trapped"));
    // The trapper itself is free.
    assert!(can_switch(&b, SideRef::P1, 0));
}

#[test]
fn block_traps_like_mean_look() {
    // PS data/moves.ts:1511 block: the same onHit as Mean Look.
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    b.step(&[mv(0, 1, foe())], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P2, 0));
    assert!(!can_switch(&b, SideRef::P2, 0));
}

#[test]
fn mean_look_fails_on_a_ghost() {
    // addVolatile: runStatusImmunity('trapped') fails for a Ghost
    // (data/typechart.ts); the move has no `status`, so no `-immune`, and the
    // false onHit is runMoveEffects' `-fail`.
    let p2 = r#"[{"species":"gengar","level":50,"ability":"cursedbody","moves":["splash"]},
        {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;
    let mut b = logged(Format::Singles, TRAPPER, p2, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(!b.p2.team[0].volatiles.has(VolatileKind::Trapped));
    assert!(can_switch(&b, SideRef::P2, 0));
    let log = log_lines(&b);
    assert!(log.iter().any(|l| l == "|-fail|p1a: Umbreon"), "{log:?}");
    assert!(!log.iter().any(|l| l.starts_with("|-immune|")), "{log:?}");
}

#[test]
fn mean_look_fails_on_an_already_trapped_target() {
    // addVolatile returns false on an existing `trapped` (no onRestart).
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    b.step(&[mv(0, 1, foe())], &[mv(0, 0, None)]);
    let log = log_lines(&b);
    let fails = log.iter().filter(|l| *l == "|-fail|p1a: Umbreon").count();
    assert_eq!(fails, 1, "{log:?}");
    assert!(b.is_trapped(SideRef::P2, 0));
}

#[test]
fn mean_look_is_stopped_by_a_substitute() {
    // No bypasssub: substitute onTryPrimaryHit fails the move.
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    b.p2.team[0].set_substitute_hp(50);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(!b.is_trapped(SideRef::P2, 0));
}

#[test]
fn mean_look_trap_ends_when_its_source_leaves() {
    // sim/pokemon.ts clearVolatile -> removeLinkedVolatiles: the trapper's
    // switch-out deletes the linked `trapped`.
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P2, 0));
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[mv(0, 0, None)]);
    assert!(!b.p2.team[0].volatiles.has(VolatileKind::Trapped));
    assert!(can_switch(&b, SideRef::P2, 0), "released");
}

#[test]
fn shed_shell_escapes_mean_look() {
    // data/items.ts shedshell onTrapPokemon (priority -10): trapped = false.
    let p2 = r#"[{"species":"snorlax","level":50,"ability":"thickfat","item":"shedshell","moves":["splash"]},
        {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;
    let mut b = logged(Format::Singles, TRAPPER, p2, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(b.p2.team[0].volatiles.has(VolatileKind::Trapped), "the volatile still lands");
    assert!(can_switch(&b, SideRef::P2, 0));
}

#[test]
fn a_trapped_mon_still_pivots_out_with_u_turn() {
    // selfSwitch ignores trapping (sim/battle-actions.ts switchIn path).
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    b.step(
        &[mv(0, 3, None)],
        &[mv(0, 1, Some(t(SideRef::P1, 0))), Choice::Switch { actor_slot: 0, team_index: 1 }],
    );
    assert_eq!(b.p2.active[0], 1, "U-turn switched the trapped mon out");
}

#[test]
fn baton_pass_leaves_the_trap_behind() {
    // `trapped` is noCopy (data/conditions.ts), so copyVolatileFrom skips it.
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    b.step(&[mv(0, 3, None)], &[mv(0, 2, None), Choice::Switch { actor_slot: 0, team_index: 1 }]);
    assert_eq!(b.p2.active[0], 1, "Baton Pass switched the trapped mon out");
    assert!(!b.p2.team[1].volatiles.has(VolatileKind::Trapped));
    assert!(can_switch(&b, SideRef::P2, 0));
}

#[test]
fn roar_drags_out_a_trapped_mon() {
    // dragIn does not consult trapping (sim/battle-actions.ts dragIn).
    let p1 = r#"[{"species":"umbreon","level":50,"ability":"synchronize","nature":"timid","evs":{"spe":252},"moves":["meanlook","roar"]}]"#;
    let mut b = logged(Format::Singles, p1, VICTIM, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P2, 0));
    b.step(&[mv(0, 1, foe())], &[mv(0, 0, None)]);
    assert_eq!(b.p2.active[0], 1, "Roar dragged the trapped mon out");
}

#[test]
fn eject_button_switches_out_a_trapped_holder() {
    // data/items.ts ejectbutton onAfterMoveSecondary checks canSwitch only.
    let p1 = r#"[{"species":"umbreon","level":50,"ability":"synchronize","moves":["meanlook","tackle"]}]"#;
    let p2 = r#"[{"species":"snorlax","level":50,"ability":"thickfat","item":"ejectbutton","moves":["splash"]},
        {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;
    let mut b = logged(Format::Singles, p1, p2, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P2, 0));
    b.step(&[mv(0, 1, foe())], &[mv(0, 0, None)]);
    assert_eq!(b.p2.active[0], 1, "Eject Button switched the trapped holder out");
}

#[test]
fn perish_song_faints_a_mean_looked_mon() {
    // data/moves.ts perishsong: perish3 on every active; the trapped foe
    // cannot leave and faints at the count's end (data/conditions.ts
    // perishsong onEnd faint).
    let mut b = logged(Format::Singles, TRAPPER, VICTIM, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    b.step(&[mv(0, 2, None)], &[mv(0, 0, None)]);
    for _ in 0..2 {
        assert!(!can_switch(&b, SideRef::P2, 0));
        b.step(&[mv(0, 3, None)], &[mv(0, 0, None)]);
    }
    assert!(!can_switch(&b, SideRef::P2, 0));
    b.step(&[mv(0, 3, None)], &[mv(0, 0, None)]);
    assert!(!b.p2.team[0].is_alive(), "perish count ran out on the trapped foe");
    assert!(!b.p1.team[0].is_alive(), "and on the trapper");
}

#[test]
fn mean_look_in_doubles_hits_the_chosen_foe_and_can_target_an_ally() {
    let p1 = r#"[{"species":"umbreon","level":50,"ability":"synchronize","moves":["meanlook"]},
        {"species":"blissey","level":50,"ability":"naturalcure","moves":["splash"]},
        {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;
    let p2 = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]},
        {"species":"rillaboom","level":50,"ability":"grassysurge","moves":["splash"]},
        {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;
    let mut b = logged(Format::Doubles, p1, p2, 1);
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 1))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.is_trapped(SideRef::P2, 1));
    assert!(!b.is_trapped(SideRef::P2, 0));
    // An ally trap is released when the (same-side) source leaves.
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.is_trapped(SideRef::P1, 1));
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(!b.is_trapped(SideRef::P1, 1));
    assert!(!b.is_trapped(SideRef::P2, 1));
}

// ---- Jaw Lock ----

const BITER: &str = r#"[{"species":"tyranitar","level":50,"ability":"unnerve","moves":["jawlock","splash"]},
    {"species":"blissey","level":50,"ability":"naturalcure","moves":["splash"]}]"#;
const BITTEN: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash","uturn"],"evs":{"hp":252,"def":252}},
    {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;

#[test]
fn jaw_lock_traps_both_user_and_target() {
    // PS data/moves.ts:9789 jawlock onHit: source first, then target.
    let mut b = logged(Format::Singles, BITER, BITTEN, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P1, 0) && b.is_trapped(SideRef::P2, 0));
    assert!(!can_switch(&b, SideRef::P1, 0) && !can_switch(&b, SideRef::P2, 0));
    let log = log_lines(&b);
    let user = log.iter().position(|l| l == "|-activate|p1a: Tyranitar|trapped");
    let target = log.iter().position(|l| l == "|-activate|p2a: Snorlax|trapped");
    assert!(user.is_some() && target.is_some() && user < target, "{log:?}");
}

#[test]
fn jaw_lock_ghost_immunity_is_per_side() {
    // runStatusImmunity('trapped') on each addVolatile separately.
    let ghost_target = r#"[{"species":"gengar","level":50,"ability":"cursedbody","moves":["splash"],"evs":{"hp":252,"def":252}},
        {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;
    let mut b = logged(Format::Singles, BITER, ghost_target, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(b.is_trapped(SideRef::P1, 0), "user trapped by the Ghost");
    assert!(!b.p2.team[0].volatiles.has(VolatileKind::Trapped), "Ghost target not trapped");

    let ghost_user = r#"[{"species":"gengar","level":50,"ability":"cursedbody","moves":["jawlock"]},
        {"species":"blissey","level":50,"ability":"naturalcure","moves":["splash"]}]"#;
    let mut b = logged(Format::Singles, ghost_user, BITTEN, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(!b.p1.team[0].volatiles.has(VolatileKind::Trapped), "Ghost user not trapped");
    assert!(b.is_trapped(SideRef::P2, 0));
}

#[test]
fn jaw_lock_releases_the_user_when_the_target_leaves() {
    // The target U-turns out (selfSwitch ignores the trap); its link on the
    // user is removed with it.
    let mut b = logged(Format::Singles, BITER, BITTEN, 1);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    b.step(&[mv(0, 1, None)], &[mv(0, 1, Some(t(SideRef::P1, 0))), Choice::Switch { actor_slot: 0, team_index: 1 }]);
    assert_eq!(b.p2.active[0], 1);
    assert!(!b.p1.team[0].volatiles.has(VolatileKind::Trapped));
    assert!(can_switch(&b, SideRef::P1, 0), "user released");
}

#[test]
fn jaw_lock_that_kos_traps_nobody() {
    // addVolatile: a fainted holder (`!this.hp`) and a fainted linked
    // source (`linkedStatus && !source.hp`) both fail.
    let mut b = logged(Format::Singles, BITER, BITTEN, 1);
    b.p2.team[0].current_hp = 1;
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(!b.p2.team[0].is_alive());
    assert!(!b.p1.team[0].volatiles.has(VolatileKind::Trapped));
}

#[test]
fn jaw_lock_into_a_substitute_traps_nobody() {
    // The Substitute's HIT_SUBSTITUTE drops the target before runMoveEffects.
    let mut b = logged(Format::Singles, BITER, BITTEN, 1);
    b.p2.team[0].set_substitute_hp(200);
    b.step(&[mv(0, 0, foe())], &[mv(0, 0, None)]);
    assert!(!b.p1.team[0].volatiles.has(VolatileKind::Trapped));
    assert!(!b.p2.team[0].volatiles.has(VolatileKind::Trapped));
}
