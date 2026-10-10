//! `ps-rng` draw-order goldens: PS captures whose seeded replay must match
//! every PS draw (kind and span) and every end-of-turn state.
//!
//! Each file in `tools/accuracy/repros/battles/psrng-*.json` is captured by
//! `tools/accuracy/ps-battle.js tools/accuracy/repros/psrng-order-jobs.jsonl
//! <out> --max-turns 1` (PS `a5df8274`, Champions mod). The draw lists carry
//! PS's own call stacks, so a missing, extra or reordered draw fails here.

#![cfg(feature = "ps-rng")]

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_ps_rng, AccBattle};

fn assert_seeded_matches_ps(name: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut acc: AccBattle = serde_json::from_str(&text).expect("battle json");
    assert!(acc.meta.ok && acc.meta.errors.is_empty(), "{name}: PS errors {:?}", acc.meta.errors);
    // `--max-turns` ends the battle with a forced `|tie` before the last
    // recorded turn is played; the engine would still play it.
    if acc.meta.log.trim_end().ends_with("|tie") && acc.turns.len() > 1 {
        acc.turns.pop();
    }
    let (rep, _) = replay_ps_rng(&acc);
    assert!(rep.engine_error.is_none(), "{name}: engine error {:?}", rep.engine_error);
    assert!(rep.turns_compared > 0, "{name}: no turns compared");
    assert!(rep.divergence.is_none(), "{name}: state diverges: {:?}", rep.divergence);
    assert!(rep.first_draw_div.is_none(), "{name}: draw diverges: {:?}", rep.first_draw_div);
}

/// Roar only sets `forceSwitchFlag`; runAction drags the target in after
/// the move, so the hit loop's two Updates sort the pre-drag actives, and
/// the drag's runSwitch speed-sorts every active (sim/battle.ts:2821-2829;
/// sim/battle-actions.ts:158-178). Equal-Speed leads make every sort draw.
#[test]
fn roar_drags_after_the_hit_loop_and_sorts_in_run_switch() {
    assert_seeded_matches_ps("psrng-roar-drags-after-the-hit-loop-1");
    assert_seeded_matches_ps("psrng-roar-drags-after-the-hit-loop-3");
}
