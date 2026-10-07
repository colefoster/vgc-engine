//! Gulp Missile: a full-info PS capture, replayed keyed and seeded.
//!
//! PS a5df8274 data/abilities.ts:1745-1769. Capture
//! `gulp-superfang-gorging-fullinfo-01` (`tools/accuracy/gulp-missile-
//! verification/jobs.jsonl`, `ps-battle.js --max-turns 3`, seed
//! `sodium,…0d01`): Super Fang leaves Cramorant at exactly half (88/176),
//! Surf loads Cramorant-Gorging, and Snorlax's Tackle into it takes a
//! quarter of its max HP (59) and paralysis while Cramorant reverts.

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle, Replayed};

const NAME: &str = "gulp-superfang-gorging-fullinfo-01";

fn capture() -> AccBattle {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/accuracy/repros/battles").join(format!("{NAME}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let acc: AccBattle = serde_json::from_str(&text).expect("battle json");
    assert!(acc.meta.ok && acc.meta.errors.is_empty(), "PS errors {:?}", acc.meta.errors);
    assert_eq!(acc.format, "gen9customgame");
    assert_eq!(acc.turns.len(), 3);
    assert!(acc.turns.iter().all(|t| t.has_state));
    // `--max-turns 3` ends at the turn-4 marker: turn 3 was fully played.
    assert_eq!(acc.meta.last_turn, Some(4));
    acc
}

fn assert_state_clean(rep: &Replayed, what: &str) {
    assert!(rep.engine_error.is_none(), "{what}: engine error {:?}", rep.engine_error);
    assert!(
        rep.divergence.is_none(),
        "{what}: {:?}",
        rep.divergence.as_ref().map(|d| (d.turn, &d.slot, &d.field, &d.engine, &d.ps))
    );
    assert_eq!((rep.turns_compared, rep.matched_turns), (3, 3), "{what}");
}

#[test]
fn capture_records_load_response_and_revert() {
    let acc = capture();
    let s = |t: usize, slot: &str| &acc.turns[t].base.state[slot];
    // Turn 1: Super Fang, exactly half.
    assert_eq!((s(0, "p1a").species.as_deref(), s(0, "p1a").hp, s(0, "p1a").maxhp), (Some("cramorant"), 88, 176));
    // Turn 2: Surf at hp <= maxhp / 2 loads Gorging.
    assert_eq!(s(1, "p1a").species.as_deref(), Some("cramorantgorging"));
    assert_eq!(s(1, "p2a").hp, 194);
    assert!(s(1, "p2a").status.is_none());
    // Turn 3: Tackle into Gorging: 194 - 59 (= 236 / 4) = 135, paralysis,
    // no Def change, and Cramorant is back to base.
    assert_eq!(s(2, "p1a").species.as_deref(), Some("cramorant"));
    assert_eq!(s(2, "p2a").hp, 135);
    assert_eq!(s(2, "p2a").status.as_deref(), Some("par"));
    assert_eq!(s(2, "p2a").boosts.map(|b| b.def), Some(0));
    let log = acc.meta.log.as_str();
    let pos = |l: &str| log.find(l).unwrap_or_else(|| panic!("missing `{l}`"));
    let load = pos("|-formechange|p1a: Cramorant|Cramorant-Gorging|\n");
    assert!(load < pos("|-damage|p2a: Snorlax|194/236\n"), "loaded before Surf's damage");
    let quarter = pos("|-damage|p2a: Snorlax|135/236|[from] ability: Gulp Missile|[of] p1a: Cramorant\n");
    assert!(quarter < pos("|-status|p2a: Snorlax|par\n"));
    assert!(pos("|-status|p2a: Snorlax|par\n") < pos("|-formechange|p1a: Cramorant|Cramorant|\n"), "reverts last");
}

#[test]
fn keyed_replay_matches_every_state_and_draw() {
    let rep = replay_keyed(&capture());
    assert_state_clean(&rep, "keyed");
    assert_eq!(rep.unmatched_total, 0, "missing draws {:?}", rep.miss_keys);
    assert_eq!(rep.repaired, 0, "repair aliases");
    assert_eq!(rep.leftover_upto, 0, "leftovers {:?}", rep.leftover_keys);
}

#[cfg(feature = "ps-rng")]
#[test]
fn seeded_replay_matches_every_state_and_draw() {
    let (rep, _) = vgc_engine_conformance::accuracy::replay_ps_rng(&capture());
    assert_state_clean(&rep, "seeded");
    assert!(rep.first_draw_div.is_none(), "{:?}", rep.first_draw_div);
    assert_eq!(rep.ps_errors, 0);
}

/// The species check stays exact: a capture claiming a different forme on
/// the load turn diverges.
#[test]
fn a_wrong_forme_still_diverges() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/accuracy/repros/battles").join(format!("{NAME}.json"));
    let mut v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["turns"][1]["state"]["p1a"]["species"] = "cramorantgulping".into();
    let acc: AccBattle = serde_json::from_value(v).unwrap();
    let d = replay_keyed(&acc).divergence.expect("Gulping is not Gorging");
    assert_eq!((d.turn, d.slot.as_str(), d.field.as_str(), d.ps.as_str()), (2, "p1a", "species", "cramorantgulping"));
}
