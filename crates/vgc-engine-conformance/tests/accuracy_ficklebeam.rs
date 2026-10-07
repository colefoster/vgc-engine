//! Fickle Beam's power-doubling gate in keyed accuracy replay.
//!
//! PS (`data/moves.ts` ficklebeam `onBasePower`) draws
//! `randomChance(3, 10)` and doubles the base power on success. The keyed
//! recorder logs the RESULT as a `range` draw with a bool value. The engine
//! reads it through `Rng::chance_keyed`, which decodes `Range(1)` as pass and
//! `Range(0)` as fail (`vgc-engine-core/src/rng.rs` `chance_keyed`). The
//! accuracy harness must hand that outcome to the engine unchanged, with no
//! repair pass.
//!
//! Fixtures: actual pinned-PS (`a5df8274`) captures of purpose-built quiet
//! doubles. `tools/accuracy/ps-battle.js --max-turns 2`, inputs in
//! `fickle-verification/jobs.jsonl`. Blissey (Splash) and Hydrapple (Fickle
//! Beam only) on each side, no items, explicit genders. Each Hydrapple fires
//! Fickle Beam into the opposing Blissey on both turns, so each capture has
//! four gates. Their only other draws are accuracy, crit, damage and two
//! modelled team-preview queue shuffles at start.
//!
//! - standard (`gen9doublescustomgame`): all four gates fail;
//! - Champions (`gen9championsvgc2026regmb`): turn 1 p2b passes, the other
//!   three fail.

use std::path::PathBuf;

use serde_json::Value;
use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle, Replayed};

const STANDARD: &str = "fickle-gate-standard-fb5d0001";
const CHAMPIONS: &str = "fickle-gate-champions-fbc40001";

fn capture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).expect("battle json")
}

/// The recorded Fickle Beam gates as `(turn, actor, passed)`, in draw order.
fn gates(v: &Value) -> Vec<(u64, String, bool)> {
    v["turns"]
        .as_array()
        .expect("turns")
        .iter()
        .flat_map(|t| t["draws"].as_array().expect("draws").iter())
        .filter(|d| d["move"] == "ficklebeam" && d["decision"] == "range")
        .map(|d| {
            let pass = d["value"].as_bool().expect("Fickle Beam gate recorded as a bool");
            (d["turn"].as_u64().unwrap(), d["actor"].as_str().unwrap().to_string(), pass)
        })
        .collect()
}

/// Capture preconditions: a complete error-free 2-turn PS run, no faint, and
/// draws limited to the four Fickle Beam decisions plus the start shuffles.
fn checked(name: &str, format: &str) -> Value {
    let v = capture(name);
    assert_eq!(v["format"], format, "{name}: format");
    assert_eq!(v["_meta"]["ok"], true, "{name}: PS ok");
    assert!(v["_meta"]["errors"].as_array().unwrap().is_empty(), "{name}: PS errors");
    let turns = v["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2, "{name}: two recorded turns");
    for t in turns {
        assert_eq!(t["has_state"], true, "{name}: turn {} state", t["turn"]);
        for slot in ["p1a", "p1b", "p2a", "p2b"] {
            assert_eq!(t["state"][slot]["fainted"], false, "{name}: turn {} {slot} fainted", t["turn"]);
        }
        for d in t["draws"].as_array().unwrap() {
            assert_eq!(d["move"], "ficklebeam", "{name}: unexpected draw {d}");
            let decision = d["decision"].as_str().unwrap();
            assert!(["accuracy", "crit", "range", "damage"].contains(&decision), "{name}: unexpected decision {d}");
        }
        for r in t["raw"].as_array().unwrap() {
            assert_ne!(r["op"], "shuffle", "{name}: in-turn shuffle {r}");
        }
    }
    let start: Vec<&Value> = v["start_raw"].as_array().unwrap().iter().collect();
    assert!(
        start.iter().all(|r| r["op"] == "shuffle" && r["site"][1].as_str().unwrap().starts_with("BattleQueue.sort@")),
        "{name}: start draws other than team-preview queue shuffles"
    );
    assert_eq!(gates(&v).len(), 4, "{name}: one gate per Hydrapple per turn");
    v
}

fn standard() -> Value {
    checked(STANDARD, "gen9doublescustomgame")
}

fn champions() -> Value {
    checked(CHAMPIONS, "gen9championsvgc2026regmb")
}

fn acc(v: Value) -> AccBattle {
    serde_json::from_value(v).expect("AccBattle")
}

fn assert_clean(rep: &Replayed, what: &str) {
    assert!(rep.engine_error.is_none(), "{what}: engine error {:?}", rep.engine_error);
    assert!(
        rep.divergence.is_none(),
        "{what}: {:?}",
        rep.divergence.as_ref().map(|d| (d.turn, &d.slot, &d.field, &d.engine, &d.ps))
    );
    assert_eq!((rep.turns_total, rep.turns_compared, rep.matched_turns), (2, 2, 2), "{what}: turns");
}

/// The fixture set covers both outcomes, and the expected per-capture gates.
#[test]
fn fixtures_record_both_gate_outcomes() {
    let s = gates(&standard());
    let c = gates(&champions());
    assert!(s.iter().all(|g| !g.2), "standard: all gates fail {s:?}");
    assert_eq!(
        c,
        [
            (1, "p1b".to_string(), false),
            (1, "p2b".to_string(), true),
            (2, "p1b".to_string(), false),
            (2, "p2b".to_string(), false),
        ],
        "champions gates"
    );
    let all: Vec<bool> = s.iter().chain(&c).map(|g| g.2).collect();
    assert!(all.contains(&true) && all.contains(&false), "both outcomes across the set");
}

/// Keyed: every recorded gate reaches the engine as recorded. RED before the
/// fix: the repair pass inverts each failed gate (turn 1 p1a hp engine 228
/// vs PS 279, `repaired` 2).
#[test]
fn keyed_standard_failed_gates_are_not_inverted() {
    let rep = replay_keyed(&acc(standard()));
    assert_clean(&rep, "standard keyed");
    assert_eq!(rep.repaired, 0, "standard keyed: repair pass used");
    assert_eq!(rep.unmatched_total, 0, "standard keyed: missing draws {:?}", rep.miss_keys);
    assert!(rep.leftover_keys.is_empty(), "standard keyed: leftovers {:?}", rep.leftover_keys);
}

/// Keyed, both directions in one capture. RED before the fix: the passed
/// turn-1 p2b gate is read as a fail (turn 1 p1a hp engine 282 vs PS 234).
#[test]
fn keyed_champions_passed_and_failed_gates_are_not_inverted() {
    let rep = replay_keyed(&acc(champions()));
    assert_clean(&rep, "champions keyed");
    assert_eq!(rep.repaired, 0, "champions keyed: repair pass used");
    assert_eq!(rep.unmatched_total, 0, "champions keyed: missing draws {:?}", rep.miss_keys);
    assert!(rep.leftover_keys.is_empty(), "champions keyed: leftovers {:?}", rep.leftover_keys);
}

/// Seeded: the engine on PS's own PRNG never uses the keyed table, so both
/// captures match draw for draw before and after the fix.
#[cfg(feature = "ps-rng")]
#[test]
fn seeded_captures_stay_aligned() {
    for (name, v) in [(STANDARD, standard()), (CHAMPIONS, champions())] {
        let (rep, _) = vgc_engine_conformance::accuracy::replay_ps_rng(&acc(v));
        assert_clean(&rep, name);
        assert!(rep.first_draw_div.is_none(), "{name}: {:?}", rep.first_draw_div);
        assert_eq!(rep.unmatched_total, 0, "{name}: missing draws");
        assert!(rep.leftover_keys.is_empty(), "{name}: leftovers {:?}", rep.leftover_keys);
    }
}
