//! Goldens from the accuracy study's minimal repros
//! (`tools/accuracy/repros/`, docs/accuracy/2026-09-accuracy-proof.md).
//!
//! Each file in `tools/accuracy/repros/battles/` is a PS battle captured by
//! `tools/accuracy/ps-battle.js` from the matching `repros.jsonl` job (PS
//! `a5df8274`, Champions mod). The engine replays it with every PS random
//! outcome keyed in, so any end-of-turn state difference is a mechanics bug.
//! A repro gets a test here once its bug is fixed.

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle};

fn assert_matches_ps(name: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut acc: AccBattle = serde_json::from_str(&text).expect("battle json");
    // `ps-battle.js --max-turns` ends the battle with a forced `|tie` before
    // the last recorded turn is played; the engine would still play it.
    if acc.meta.log.trim_end().ends_with("|tie") {
        acc.turns.pop();
    }
    let rep = replay_keyed(&acc);
    assert!(rep.engine_error.is_none(), "{name}: engine error {:?}", rep.engine_error);
    assert!(rep.turns_compared > 0, "{name}: no turns compared");
    assert!(rep.divergence.is_none(), "{name}: diverges from PS: {:?}", rep.divergence);
}

/// Flare Blitz KOs a 202-HP Amoonguss: recoil is a third of the 202 HP it
/// actually lost, not of the uncapped damage roll.
#[test]
fn recoil_uses_damage_actually_dealt_on_ko() {
    assert_matches_ps("recoil-uncapped-on-ko");
}

/// Grounded Rillaboom's Grassy Glide into airborne Pelipper gets the Grassy
/// Terrain ×1.3: the boost checks the attacker's grounding.
#[test]
fn terrain_boost_checks_attacker_grounding() {
    assert_matches_ps("terrain-boost-gated-on-defender");
}

/// Grounded Indeedee's Expanding Force in Psychic Terrain hits both foes.
#[test]
fn expanding_force_spreads_in_psychic_terrain() {
    assert_matches_ps("expanding-force-no-spread");
}

/// Rillaboom enters via Parting Shot mid-turn and Fake Outs the next turn:
/// its first move action since switching in, so Corviknight flinches.
#[test]
fn fake_out_works_after_a_mid_turn_switch_in() {
    assert_matches_ps("fake-out-after-pivot-switch-in");
}

/// Study battle 0d752208ee (real Reg M-C log): Incineroar's Flare Blitz
/// recoil takes it below half and it eats its Sitrus Berry that action
/// (PS turn 3: 69 -> 119 -> 131 after Grassy Terrain).
#[test]
fn sitrus_berry_eaten_after_recoil() {
    assert_matches_ps("study-0d752208ee-sitrus-after-recoil");
}

/// Study battle 220a63b680: Archaludon's Electro Shot in Rain fires on
/// turn 1 and hits with the +1 SpA it just gained (PS 88, engine was 134).
#[test]
fn electro_shot_in_rain_uses_its_spa_boost() {
    assert_matches_ps("study-220a63b680-electro-shot-rain");
}

/// Study battle 576126830e: both sides replace a fainted mon at the end of
/// turn 1; both replacements are in before Intimidate fires, so the foe's
/// replacement is intimidated too.
#[test]
fn simultaneous_replacements_enter_before_abilities() {
    assert_matches_ps("study-576126830e-replacement-intimidate");
}

/// Study battle d2fe06dc11: Rillaboom's Grassy Surge replaces Psychic
/// Terrain before the other replacement's Psychic Seed is checked (seeds run
/// at SwitchIn priority -1, after every ability).
#[test]
fn replacement_seed_checked_after_all_abilities() {
    assert_matches_ps("study-d2fe06dc11-replacement-seed-after-surge");
}

/// Study battle af4174f41f: Tyranitar's Sandstorm runs out at the end of
/// turn 5; PS ends it before chipping, so Sneasler stays at 166.
#[test]
fn weather_ends_before_chip_on_its_last_turn() {
    assert_matches_ps("study-af4174f41f-sand-ends-no-chip");
}

/// Study battle 75999c4aaa: Whimsicott Charms its own Staraptor (turn 3);
/// the engine used to send it to the first foe.
#[test]
fn status_move_aimed_at_ally_hits_the_ally() {
    assert_matches_ps("study-75999c4aaa-charm-on-ally");
}

/// Study battle 8760be9ed8 (real Reg M-C log): an Eject Button holder is
/// replaced by the player's pick (PS `switchFlag` -> switch request), not the
/// first bench mon, and its own queued move is forfeited.
#[test]
fn eject_button_replacement_is_the_players_pick() {
    assert_matches_ps("study-8760be9ed8-eject-button-pick");
}

/// Study battle 270f1619dc (real Reg M-C log): a mon that switched in this
/// turn is hit and ejected by its Eject Button; the second Switch for its
/// slot is the mid-turn pick, not a second turn-start switch.
#[test]
fn eject_button_pick_after_a_turn_start_switch_in() {
    assert_matches_ps("study-270f1619dc-eject-after-switch-in");
}

/// Study battle 59ccb3d636 (real Reg M-C log): a hit drops Golisopod from
/// above half to below it and Emergency Exit switches it out to the
/// player's pick.
#[test]
fn emergency_exit_switches_out_below_half() {
    assert_matches_ps("study-59ccb3d636-emergency-exit");
}

/// Study battle dc6522e008 (real Reg M-C log): both sides switch under Trick
/// Room and the slower leaving mon's switch resolves first, so the order of
/// switch-in Intimidates matches PS.
#[test]
fn pre_turn_switches_are_slowest_first_under_trick_room() {
    assert_matches_ps("study-dc6522e008-trick-room-switch-order");
}

/// Study battle 9d848b7db0 (real Reg M-C log): Life Orb and a Chople Berry
/// on one hit are one chained ModifyDamage modifier with a single rounding.
#[test]
fn life_orb_and_resist_berry_chain_into_one_rounding() {
    assert_matches_ps("study-9d848b7db0-lifeorb-chople-chain");
}

/// Study battle fb56fc4f9c (real Reg M-C log): Life Orb into Aurora Veil,
/// chained into one rounding.
#[test]
fn life_orb_and_aurora_veil_chain_into_one_rounding() {
    assert_matches_ps("study-fb56fc4f9c-lifeorb-veil-chain");
}

/// Study battle 8fba727aef (real Reg M-C log): Steel Roller needs a terrain
/// to work and clears it when it hits.
#[test]
fn steel_roller_needs_and_clears_terrain() {
    assert_matches_ps("study-8fba727aef-steel-roller");
}

/// Study battle 3d3fc88bdf (real Reg M-C log): Drum Beating lowers the
/// target's Speed.
#[test]
fn drum_beating_lowers_speed() {
    assert_matches_ps("study-3d3fc88bdf-drum-beating");
}

/// Study battle 1f62339620 (real Reg M-C log): Champions makes Shadow Claw a
/// slicing move, so Sharpness boosts it (data/mods/champions/moves.ts).
#[test]
fn champions_shadow_claw_is_slicing() {
    assert_matches_ps("study-1f62339620-shadow-claw-slicing");
}

/// Study battle 11f8f58c63 (real Reg M-C log): the battle ends at the last
/// faint; no end-of-turn residual runs on the winner.
#[test]
fn battle_ends_at_the_last_faint() {
    assert_matches_ps("study-11f8f58c63-win-ends-turn");
}

/// Study battle a6a15861c8 (real Reg M-C log): Champions sleep lasts
/// `sample([2, 3, 3])` turns (data/mods/champions/conditions.ts).
#[test]
fn champions_sleep_duration() {
    assert_matches_ps("study-a6a15861c8-champions-sleep");
}
