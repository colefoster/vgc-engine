//! Which recorded turn the keyed replay relaxes to a faint-only comparison.
//!
//! PS stops the instant a side runs out, so a battle that ended on a recorded
//! turn only has comparable faints there. A `ps-battle.js --max-turns N` run
//! also has `_meta.ended` (the forced `|tie`), but it ends at the `|turn|N+1`
//! marker (`_meta.lastTurn = N + 1`) after turn N was fully played: turn N
//! gets the full comparison.
//!
//! Fixture: turn 1 of the `recoil-uncapped-on-ko` repro (PS `a5df8274`),
//! which matches PS in full (`accuracy_repros.rs`), with `_meta.lastTurn`
//! set per case and one expected value corrupted.

use std::path::PathBuf;

use serde_json::Value;
use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle, Replayed};

/// Turn 1 only, `_meta.ended = true`, `_meta.lastTurn` = `last_turn`
/// (removed when `None`, as in pre-marker captures).
fn turn_one(last_turn: Option<u32>) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/accuracy/repros/battles/recoil-uncapped-on-ko.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut v: Value = serde_json::from_str(&text).expect("battle json");
    let turns = v["turns"].as_array_mut().expect("turns");
    turns.truncate(1);
    assert_eq!(turns[0]["turn"], 1);
    assert_eq!(turns[0]["has_state"], true);
    let meta = v["_meta"].as_object_mut().expect("_meta");
    meta.insert("ended".into(), Value::Bool(true));
    match last_turn {
        Some(n) => meta.insert("lastTurn".into(), n.into()),
        None => meta.remove("lastTurn"),
    };
    v
}

/// The first live slot of turn 1's expected state (sorted, deterministic).
fn live_slot(v: &Value) -> String {
    let state = v["turns"][0]["state"].as_object().expect("state");
    let mut slots: Vec<&String> = state.keys().filter(|k| state[*k]["fainted"] == false).collect();
    slots.sort();
    slots.first().expect("a live slot").to_string()
}

fn replay(v: Value) -> Replayed {
    let acc: AccBattle = serde_json::from_value(v).expect("AccBattle");
    let rep = replay_keyed(&acc);
    assert!(rep.engine_error.is_none(), "engine error {:?}", rep.engine_error);
    assert_eq!(rep.turns_compared, 1);
    rep
}

/// Corrupt one expected field of `slot` on turn 1.
fn corrupt(v: &mut Value, slot: &str, field: &str) {
    let mon = &mut v["turns"][0]["state"][slot];
    match field {
        "hp" => {
            let (hp, max) = (mon["hp"].as_u64().unwrap(), mon["maxhp"].as_u64().unwrap());
            mon["hp"] = (if hp == max { hp - 1 } else { hp + 1 }).into();
        }
        "ability" => {
            let other = if mon["ability"] == "pressure" { "intimidate" } else { "pressure" };
            mon["ability"] = other.into();
        }
        "boosts" => {
            let atk = mon["boosts"]["atk"].as_i64().unwrap();
            mon["boosts"]["atk"] = (if atk < 6 { atk + 1 } else { atk - 1 }).into();
        }
        "fainted" => mon["fainted"] = true.into(),
        _ => unreachable!(),
    }
}

/// `--max-turns 1`: PS ends at the `|turn|2` marker after turn 1 was played,
/// so a wrong HP, ability or boost on turn 1 is a divergence.
#[test]
fn turn_before_a_cutoff_marker_is_compared_in_full() {
    let clean = turn_one(Some(2));
    let slot = live_slot(&clean);
    let rep = replay(clean.clone());
    assert!(rep.divergence.is_none(), "unmodified turn 1 diverges: {:?}", rep.divergence);
    assert_eq!(rep.matched_turns, 1);

    for field in ["hp", "ability", "boosts"] {
        let mut v = clean.clone();
        corrupt(&mut v, &slot, field);
        let d = replay(v).divergence.unwrap_or_else(|| panic!("wrong {field} on {slot} not detected"));
        assert_eq!((d.turn, d.slot.as_str(), d.field.as_str()), (1, slot.as_str(), field));
    }
}

/// A battle won or tied on turn 1 (`lastTurn = 1`): only faints compare there,
/// as before (the engine finishes the turn's residuals; PS doesn't).
#[test]
fn natural_terminal_turn_stays_faint_only() {
    let clean = turn_one(Some(1));
    let slot = live_slot(&clean);
    for field in ["hp", "ability", "boosts"] {
        let mut v = clean.clone();
        corrupt(&mut v, &slot, field);
        let rep = replay(v);
        assert!(rep.divergence.is_none(), "terminal turn compared {field}: {:?}", rep.divergence);
    }
    let mut v = clean;
    corrupt(&mut v, &slot, "fainted");
    let d = replay(v).divergence.expect("faint mismatch on the terminal turn");
    assert_eq!((d.turn, d.slot.as_str(), d.field.as_str()), (1, slot.as_str(), "fainted"));
}

/// Captures without `_meta.lastTurn` keep the old reading: the last recorded
/// state turn of an ended battle is its terminal turn.
#[test]
fn missing_last_turn_marker_keeps_legacy_terminal_turn() {
    let clean = turn_one(None);
    let slot = live_slot(&clean);
    let mut v = clean.clone();
    corrupt(&mut v, &slot, "hp");
    assert!(replay(v).divergence.is_none());
    let mut v = clean;
    corrupt(&mut v, &slot, "fainted");
    assert_eq!(replay(v).divergence.expect("faint mismatch").field, "fainted");
}
