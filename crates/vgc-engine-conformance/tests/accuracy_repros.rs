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
    let acc: AccBattle = serde_json::from_str(&text).expect("battle json");
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
