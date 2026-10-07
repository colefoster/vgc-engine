//! Team preview: the replay plays only the mons PS brought.
//!
//! PS builds every exported mon at `>player` (sim/side.ts:241-246,
//! :302-309), then the `team` actions rebuild `side.pokemon` from the picks
//! (sim/side.ts:1031-1095 chooseTeam, sim/battle.ts:2750-2757) and `start`
//! prints `|teamsize|<side>|<picked>` (sim/battle.ts:2670-2674).
//! `tools/accuracy/ps-battle.js` answers preview with the identity order
//! (`team 123456`), so in a bring-4 format PS plays the export's first four.
//!
//! Fixture: synthetic Reg M-B pilot case `f3946a404862f219` (pinned PS
//! `a5df8274`, `ps-battle.js --max-turns 4`, teams from mimikyu's
//! `regmb_random_100.txt`). Turn 3 KOs p2's fourth brought mon; with the
//! unbrought Flareon / Metagross on its bench the engine waited for a
//! replacement PS never asked for and turn 4 (Discharge, then Moonlight)
//! never ran.

use std::path::PathBuf;

use serde_json::Value;
use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle, Replayed};

const CAPTURE: &str = "regmb-bring-four-f3946a404862f219";

fn capture() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{CAPTURE}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let v: Value = serde_json::from_str(&text).expect("battle json");
    let log = v["_meta"]["log"].as_str().expect("_meta.log");
    assert!(log.contains("|teamsize|p1|4\n|teamsize|p2|4\n"), "fixture: both sides brought 4");
    for side in ["p1team", "p2team"] {
        let n = v[side].as_str().unwrap().split("\n\n").count();
        assert_eq!(n, 6, "fixture: {side} exports 6");
    }
    v
}

/// `capture()` with its `|teamsize|` lines rewritten (`None` drops the line).
fn with_teamsize(p1: Option<u32>, p2: Option<u32>) -> Value {
    let mut v = capture();
    let log = v["_meta"]["log"].as_str().unwrap().to_string();
    let mut out = String::new();
    for line in log.split_inclusive('\n') {
        let repl = match line.trim_end() {
            "|teamsize|p1|4" => Some(p1),
            "|teamsize|p2|4" => Some(p2),
            _ => None,
        };
        match repl {
            None => out.push_str(line),
            Some(None) => {}
            Some(Some(k)) => out.push_str(&line.replacen("|4", &format!("|{k}"), 1)),
        }
    }
    v["_meta"]["log"] = out.into();
    v
}

fn keyed(v: Value) -> Replayed {
    let acc: AccBattle = serde_json::from_value(v).expect("AccBattle");
    let rep = replay_keyed(&acc);
    assert!(rep.engine_error.is_none(), "engine error {:?}", rep.engine_error);
    assert_eq!(rep.turns_total, 4);
    rep
}

fn assert_clean(rep: &Replayed, what: &str) {
    assert!(rep.divergence.is_none(), "{what}: {:?}", rep.divergence.as_ref().map(|d| (d.turn, &d.slot, &d.field, &d.engine, &d.ps)));
    assert_eq!((rep.turns_compared, rep.matched_turns), (4, 4), "{what}");
    assert!(
        !rep.leftover_keys.iter().any(|k| k.contains("discharge")),
        "{what}: turn-4 Discharge outcomes unconsumed: {:?}",
        rep.leftover_keys
    );
}

/// The full-team replay: turn 4 never runs, so Umbreon keeps turn 3's HP.
fn assert_full_team_stall(rep: &Replayed, what: &str) {
    let d = rep.divergence.as_ref().unwrap_or_else(|| panic!("{what}: expected the full-team stall"));
    assert_eq!(
        (d.turn, d.slot.as_str(), d.field.as_str(), d.engine.as_str(), d.ps.as_str()),
        (4, "p1a", "hp", "133", "170"),
        "{what}"
    );
    assert_eq!(rep.matched_turns, 3, "{what}");
}

/// RED before the fix: turn 4 p1a hp 133 (engine) vs 170 (PS).
#[test]
fn bring_four_capture_plays_turn_four() {
    assert_clean(&keyed(capture()), "capture");
}

/// Each side reads its own line: p1's bench never matters here, p2's does.
#[test]
fn brought_team_size_is_per_side() {
    assert_clean(&keyed(with_teamsize(Some(6), Some(4))), "p1 6 / p2 4");
    assert_clean(&keyed(with_teamsize(None, Some(4))), "p1 absent / p2 4");
    assert_full_team_stall(&keyed(with_teamsize(Some(4), None)), "p1 4 / p2 absent");
}

/// Guards: no recorded team size (legacy captures) or a size covering the
/// whole export keeps the full team.
#[test]
fn absent_or_full_team_size_keeps_the_full_team() {
    assert_full_team_stall(&keyed(with_teamsize(None, None)), "no teamsize lines");
    assert_full_team_stall(&keyed(with_teamsize(Some(6), Some(6))), "teamsize 6");
    assert_full_team_stall(&keyed(with_teamsize(Some(9), Some(9))), "teamsize above the export");
}

/// Seeded replay: PS's construction-time gender draws walk all six exported
/// mons (start_raw seq 0-8, `Side.addPokemon`), so the brought team must be
/// cut after the engine builds the battle, not before.
#[cfg(feature = "ps-rng")]
#[test]
fn bring_four_capture_seeded_keeps_start_draws_aligned() {
    let acc: AccBattle = serde_json::from_value(capture()).expect("AccBattle");
    let (rep, _) = vgc_engine_conformance::accuracy::replay_ps_rng(&acc);
    assert!(rep.engine_error.is_none(), "engine error {:?}", rep.engine_error);
    assert!(rep.divergence.is_none(), "{:?}", rep.divergence.as_ref().map(|d| (d.turn, &d.slot, &d.field, &d.engine, &d.ps)));
    assert_eq!((rep.turns_compared, rep.matched_turns), (4, 4));
    assert!(rep.first_draw_div.is_none(), "{:?}", rep.first_draw_div);
}
