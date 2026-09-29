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
