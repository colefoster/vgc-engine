//! Opt-in keyed golden mode (`"rng_mode": "keyed"`).
//!
//! The legacy flat mode feeds PS's draws to `Rng::OraclePartial` in PS call
//! order. For a spread move PS rolls every target's accuracy before any
//! target's crit/damage, and walks the user's ally before its foes
//! (sim/battle-actions.ts trySpreadMoveHit; sim/pokemon.ts:809-813
//! getMoveTargets), while the engine resolves target by target, foes first.
//! That gap stays in flat mode and is not claimed fixed here.
//!
//! Keyed mode instead reads per-draw PS provenance recorded by the driver
//! (turn / actor / target / move / decision from PS's own active context and
//! the per-target callback arguments, plus the call site and arguments) into
//! the conformance key contract and replays under `Rng::OracleKeyed`. It is
//! strict: an engine draw with no recorded outcome, or a recorded outcome the
//! engine never consumed, is a divergence even when every HP matches. The
//! only PS-only draw accepted is `BattleActions.selfDrops`' `random(100)` for
//! a move the engine drops unconditionally. No repair pass, no flat fallback.
//!
//! Captures (manager, pinned PS `a5df8274`) are installed into `goldens/`
//! from the inputs staged in `spread-keyed-verification/`.

use std::path::PathBuf;

use serde_json::{json, Value};
use vgc_engine_golden::{run_golden_in_memory, GoldenInput, GoldenReport, PsOutput};

fn goldens() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("goldens")
}

fn read(name: &str) -> Value {
    let p = goldens().join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).expect("json")
}

fn run(input: Value, ps: Value) -> Result<GoldenReport, String> {
    let input: GoldenInput = serde_json::from_value(input).map_err(|e| format!("input: {e}"))?;
    let ps: PsOutput = serde_json::from_value(ps).map_err(|e| format!("ps: {e}"))?;
    run_golden_in_memory(&input, &ps).map_err(|e| e.to_string())
}

fn keyed_divergences(r: &GoldenReport) -> Vec<String> {
    r.diverged.iter().filter(|d| d.kind.starts_with("keyed_")).map(|d| format!("{}: {}", d.kind, d.note)).collect()
}

/// A recorded draw in the driver's keyed shape.
fn rec(turn: u64, actor: &str, target: Value, mv: &str, decision: &str, value: Value, site: &str, op: &str, args: Value) -> Value {
    let mut r = json!({"turn": turn, "actor": actor, "target": target, "move": mv, "decision": decision, "value": value, "site": site, "op": op, "args": args});
    if decision == "accuracy" || decision == "secondary" {
        r["raw_is_bool"] = json!(op == "randomChance");
    }
    r
}

/// A legacy flat golden with no keyed metadata.
const FLAT: &str = "cosmetic-furfrou-base-fur-coat-v2";

const BASE_FLORGES: &str = "spread-keyed-base-florges";
const P1_PROTECT_IMMUNE: &str = "spread-keyed-p1-protect-immune";
const BASE_FURFROU_CC: &str = "spread-keyed-base-furfrou-closecombat";

// ---------------------------------------------------------------------------
// Mode selection and validation (no keyed capture needed)
// ---------------------------------------------------------------------------

/// Flat stays the default, and a `keyed` field on a flat golden's PS output
/// changes nothing.
#[test]
fn flat_default_ignores_keyed_metadata() {
    let input = read(&format!("{FLAT}.input.json"));
    let ps = read(&format!("{FLAT}.ps.json"));
    let base = run(input.clone(), ps.clone()).expect("flat run");
    let mut with_keyed = ps;
    with_keyed["keyed"] = json!([rec(1, "p2a", json!("p1a"), "closecombat", "crit", json!(false), "BattleActions.getDamage (sim/battle-actions.js)", "randomChance", json!([1, 24]))]);
    let again = run(input, with_keyed).expect("flat run with unused keyed metadata");
    assert_eq!((base.turns_run, base.matched, base.diverged.len()), (again.turns_run, again.matched, again.diverged.len()));
}

/// Keyed mode without keyed metadata is an error, not a flat replay.
#[test]
fn keyed_mode_requires_keyed_metadata() {
    let mut input = read(&format!("{FLAT}.input.json"));
    input["rng_mode"] = json!("keyed");
    let ps = read(&format!("{FLAT}.ps.json"));
    assert!(run(input, ps).is_err(), "keyed mode must reject a capture without keyed draws");
}

/// An unknown mode is rejected.
#[test]
fn unknown_rng_mode_is_rejected() {
    let mut input = read(&format!("{FLAT}.input.json"));
    input["rng_mode"] = json!("keyed-ish");
    let ps = read(&format!("{FLAT}.ps.json"));
    assert!(run(input, ps).is_err());
}

/// A keyed record the key contract would drop or default is an error, not a
/// dropped draw: unknown move, slot, decision or operation, a value that
/// doesn't fit its decision, or missing provenance.
#[test]
fn unresolvable_keyed_records_are_rejected() {
    let mut input = read(&format!("{FLAT}.input.json"));
    input["rng_mode"] = json!("keyed");
    let site = "BattleActions.getDamage (sim/battle-actions.js)";
    let crit = |actor: &str, mv: &str, decision: &str, value: Value, op: &str| {
        rec(1, actor, json!("p1a"), mv, decision, value, site, op, json!([1, 24]))
    };
    let mut no_site = crit("p2a", "closecombat", "crit", json!(false), "randomChance");
    no_site.as_object_mut().unwrap().remove("site");
    for bad in [
        crit("p2a", "notamove", "crit", json!(false), "randomChance"),
        crit("p9z", "closecombat", "crit", json!(false), "randomChance"),
        crit("p2a", "closecombat", "critical", json!(false), "randomChance"),
        crit("p2a", "closecombat", "crit", json!(false), "sample"),
        crit("p2a", "closecombat", "crit", json!(7), "randomChance"),
        no_site,
    ] {
        let mut ps = read(&format!("{FLAT}.ps.json"));
        ps["keyed"] = json!([bad.clone()]);
        assert!(run(input.clone(), ps).is_err(), "accepted {bad}");
    }
}

// ---------------------------------------------------------------------------
// Actual keyed captures
// ---------------------------------------------------------------------------

fn capture(name: &str) -> (Value, Value) {
    let input = read(&format!("{name}.input.json"));
    assert_eq!(input["rng_mode"], "keyed", "{name}: keyed input");
    let ps = read(&format!("{name}.ps.json"));
    assert_eq!(ps["ok"], true, "{name}: PS ok");
    assert!(ps["keyed"].as_array().is_some_and(|k| !k.is_empty()), "{name}: keyed draws recorded");
    (input, ps)
}

fn keyed_draws<'a>(ps: &'a Value, turn: u64, actor: &str, mv: &str, decision: &str) -> Vec<&'a Value> {
    ps["keyed"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["turn"] == turn && d["actor"] == actor && d["move"] == mv && d["decision"] == decision)
        .collect()
}

fn assert_strict_clean(r: &GoldenReport, what: &str) {
    assert!(r.diverged.is_empty(), "{what}: {:#?}", r.diverged);
    assert!(r.matched > 0, "{what}: nothing compared");
}

/// Base control (A's two-turn script, plain Florges): Garchomp's turn-2
/// Earthquake (allAdjacent from p2a: ally Slowbro, then p1a, p1b), Disarming
/// Voice (sure-hit allAdjacentFoes from p1b), Draining Kiss, Psychic.
#[test]
fn base_florges_keyed_is_strictly_clean() {
    let (input, ps) = capture(BASE_FLORGES);
    let acc = keyed_draws(&ps, 2, "p2a", "earthquake", "accuracy");
    let mut targets: Vec<&str> = acc.iter().map(|d| d["target"].as_str().unwrap()).collect();
    targets.sort_unstable();
    assert_eq!(targets, ["p1a", "p1b", "p2b"], "earthquake accuracy targets");
    assert_eq!(keyed_draws(&ps, 2, "p2a", "earthquake", "damage").len(), 3);
    assert!(keyed_draws(&ps, 1, "p1b", "disarmingvoice", "accuracy").is_empty(), "sure-hit draws no accuracy");
    let r = run(input, ps).expect("keyed run");
    assert_strict_clean(&r, BASE_FLORGES);
}

/// p1 side: Earthquake into a protected foe and a Flying foe (no draws for
/// either) plus the ally; Blizzard (allAdjacentFoes, 70%) with per-target
/// accuracy, including at least one recorded miss.
#[test]
fn p1_protect_immune_keyed_is_strictly_clean() {
    let (input, ps) = capture(P1_PROTECT_IMMUNE);
    let t1: Vec<&str> = keyed_draws(&ps, 1, "p1a", "earthquake", "accuracy")
        .iter()
        .map(|d| d["target"].as_str().unwrap())
        .collect();
    assert_eq!(t1, ["p1b"], "turn 1: only the ally rolls (p2a protected, p2b immune)");
    let blizzard: Vec<&Value> = (1..=2).flat_map(|t| keyed_draws(&ps, t, "p1b", "blizzard", "accuracy")).collect();
    assert_eq!(blizzard.len(), 3, "Blizzard accuracy: p2b on turn 1, both foes on turn 2");
    assert!(blizzard.iter().any(|d| d["value"] == false), "fixture: a Blizzard miss");
    let r = run(input, ps).expect("keyed run");
    assert_strict_clean(&r, P1_PROTECT_IMMUNE);
}

/// Close Combat's real selfDrops roll is the one accepted PS-only draw.
#[test]
fn base_furfrou_close_combat_keyed_is_strictly_clean() {
    let (input, ps) = capture(BASE_FURFROU_CC);
    let self_drops: Vec<&Value> = ps["keyed"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["site"].as_str().is_some_and(|s| s.starts_with("BattleActions.selfDrops ")))
        .collect();
    assert_eq!(self_drops.len(), 1, "fixture: one selfDrops roll (turn-1 Close Combat)");
    assert_eq!((&self_drops[0]["move"], &self_drops[0]["op"], &self_drops[0]["args"]), (&json!("closecombat"), &json!("random"), &json!([100])));
    let r = run(input, ps).expect("keyed run");
    assert_strict_clean(&r, BASE_FURFROU_CC);
}

/// Strictness: dropping one recorded crit makes the engine's draw miss the
/// table. That is a divergence even if the fallback happens to agree.
#[test]
fn missing_recorded_draw_is_a_divergence() {
    let (input, mut ps) = capture(BASE_FLORGES);
    let keyed = ps["keyed"].as_array_mut().unwrap();
    let i = keyed
        .iter()
        .position(|d| d["turn"] == 2 && d["actor"] == "p2a" && d["move"] == "earthquake" && d["decision"] == "crit")
        .expect("an earthquake crit");
    keyed.remove(i);
    let r = run(input, ps).expect("keyed run");
    assert!(keyed_divergences(&r).iter().any(|k| k.starts_with("keyed_unmatched")), "{:#?}", r.diverged);
}

/// Strictness: a recorded outcome the engine never asks for is a divergence.
#[test]
fn unconsumed_recorded_draw_is_a_divergence() {
    let (input, mut ps) = capture(BASE_FLORGES);
    ps["keyed"].as_array_mut().unwrap().push(rec(
        2, "p2a", json!("p2b"), "earthquake", "accuracy", json!(true),
        "BattleActions.hitStepAccuracy (sim/battle-actions.js)", "randomChance", json!([100, 100]),
    ));
    let r = run(input, ps).expect("keyed run");
    assert!(keyed_divergences(&r).iter().any(|k| k.starts_with("keyed_leftover")), "{:#?}", r.diverged);
}

/// A selfDrops roll is accepted only for a move the engine drops
/// unconditionally: one attached to Dragon Claw (no selfDrops) is a leftover.
#[test]
fn forged_self_drops_roll_is_a_divergence() {
    let (input, mut ps) = capture(BASE_FLORGES);
    ps["keyed"].as_array_mut().unwrap().push(rec(
        1, "p2a", Value::Null, "dragonclaw", "secondary", json!(50),
        "BattleActions.selfDrops (sim/battle-actions.js)", "random", json!([100]),
    ));
    let r = run(input, ps).expect("keyed run");
    assert!(keyed_divergences(&r).iter().any(|k| k.starts_with("keyed_leftover")), "{:#?}", r.diverged);
}

// ---------------------------------------------------------------------------
// Malformed provenance: values and signatures that don't fit the recorded call
// ---------------------------------------------------------------------------

/// Run `records` in keyed mode on the flat Furfrou input. The record shapes
/// are the actual Furfrou capture's.
fn keyed_on_flat_input(records: Vec<Value>) -> Result<GoldenReport, String> {
    let mut input = read(&format!("{FLAT}.input.json"));
    input["rng_mode"] = json!("keyed");
    let mut ps = read(&format!("{FLAT}.ps.json"));
    ps["keyed"] = json!(records);
    run(input, ps)
}

const DAMAGE_SITE: &str = "BattleActions.modifyDamage (sim/battle-actions.js:1470)";
const CRIT_SITE: &str = "BattleActions.getDamage (sim/battle-actions.js:1397)";
const ACC_SITE: &str = "BattleActions.hitStepAccuracy (sim/battle-actions.js:629)";
const SECONDARY_SITE: &str = "BattleActions.secondaries (sim/battle-actions.js:1139)";
const DURATION_SITE: &str = "Battle.durationCallback (data/moves.js:1)";

/// The actual record shapes are accepted, so validation must not
/// over-reject: accuracy randomChance(100, 100), crit randomChance(1, 24),
/// damage random(16) at 0 and 15, secondary random(100) at 0 and 99.
#[test]
fn actual_record_shapes_are_accepted() {
    let t = || json!("p1a");
    let records = vec![
        rec(1, "p2a", t(), "closecombat", "accuracy", json!(true), ACC_SITE, "randomChance", json!([100, 100])),
        rec(1, "p2a", t(), "closecombat", "crit", json!(false), CRIT_SITE, "randomChance", json!([1, 24])),
        rec(1, "p2a", t(), "closecombat", "damage", json!(0), DAMAGE_SITE, "random", json!([16])),
        rec(1, "p2a", t(), "closecombat", "damage", json!(15), DAMAGE_SITE, "random", json!([16])),
        rec(1, "p2a", t(), "closecombat", "secondary", json!(0), SECONDARY_SITE, "random", json!([100])),
        rec(1, "p2a", t(), "closecombat", "secondary", json!(99), SECONDARY_SITE, "random", json!([100])),
    ];
    // Strict divergences are expected (these aren't this battle's draws);
    // the point is that validation accepts them.
    keyed_on_flat_input(records).expect("actual shapes rejected");
}

/// A value outside its own call's range is rejected, not clamped: damage
/// random(16) = 16 or 99, secondary random(100) = 100, range random(5) = 5.
#[test]
fn out_of_bound_values_are_rejected() {
    let t = || json!("p1a");
    for bad in [
        rec(1, "p2a", t(), "closecombat", "damage", json!(16), DAMAGE_SITE, "random", json!([16])),
        rec(1, "p2a", t(), "closecombat", "damage", json!(99), DAMAGE_SITE, "random", json!([16])),
        rec(1, "p2a", t(), "closecombat", "secondary", json!(100), SECONDARY_SITE, "random", json!([100])),
        rec(1, "p2a", t(), "closecombat", "range", json!(5), DURATION_SITE, "random", json!([5])),
    ] {
        assert!(keyed_on_flat_input(vec![bad.clone()]).is_err(), "accepted {bad}");
    }
}

/// `raw_is_bool` must match the op: a bool from randomChance, the raw roll
/// from random. A record stating the other is rejected, not coerced.
#[test]
fn inconsistent_raw_is_bool_is_rejected() {
    let t = || json!("p1a");
    let mut acc = rec(1, "p2a", t(), "closecombat", "accuracy", json!(true), ACC_SITE, "randomChance", json!([100, 100]));
    acc["raw_is_bool"] = json!(false);
    let mut sec = rec(1, "p2a", t(), "closecombat", "secondary", json!(50), SECONDARY_SITE, "random", json!([100]));
    sec["raw_is_bool"] = json!(true);
    for bad in [acc, sec] {
        assert!(keyed_on_flat_input(vec![bad.clone()]).is_err(), "accepted {bad}");
    }
}

/// A two-argument random(m, n) inside its bounds is an actual shape and is
/// accepted (confusion's random(2, 6)).
#[test]
fn two_argument_random_in_bounds_is_accepted() {
    let r = rec(1, "p2a", json!("p1a"), "closecombat", "range", json!(5), DURATION_SITE, "random", json!([2, 6]));
    keyed_on_flat_input(vec![r]).expect("random(2, 6) = 5 rejected");
}

/// The real Close Combat selfDrops roll is only accepted with its own call:
/// moved to another site, or recorded as a randomChance, it is a leftover.
#[test]
fn self_drops_tolerance_needs_the_actual_call() {
    let (input, ps) = capture(BASE_FURFROU_CC);
    let at = ps["keyed"]
        .as_array()
        .unwrap()
        .iter()
        .position(|d| d["site"].as_str().is_some_and(|s| s.starts_with("BattleActions.selfDrops ")))
        .expect("selfDrops record");
    let mut wrong_site = ps.clone();
    wrong_site["keyed"][at]["site"] = json!(SECONDARY_SITE);
    let mut wrong_op = ps.clone();
    wrong_op["keyed"][at]["op"] = json!("randomChance");
    wrong_op["keyed"][at]["args"] = json!([100, 100]);
    wrong_op["keyed"][at]["value"] = json!(true);
    wrong_op["keyed"][at]["raw_is_bool"] = json!(true);
    for (what, ps) in [("site", wrong_site), ("op", wrong_op)] {
        let r = run(input.clone(), ps).expect("keyed run");
        assert!(keyed_divergences(&r).iter().any(|k| k.starts_with("keyed_leftover")), "wrong {what}: {:#?}", r.diverged);
    }
}

/// A signature that can't have produced the recorded decision is rejected:
/// damage not from random(16); a secondary roll not from random(100) or
/// randomChance(n, 100); accuracy or crit not from a two-argument
/// randomChance with 0 < num <= den; random() with empty bounds.
#[test]
fn invalid_signatures_are_rejected() {
    let t = || json!("p1a");
    for bad in [
        rec(1, "p2a", t(), "closecombat", "damage", json!(3), DAMAGE_SITE, "random", json!([100])),
        rec(1, "p2a", t(), "closecombat", "damage", json!(3), DAMAGE_SITE, "randomChance", json!([1, 24])),
        rec(1, "p2a", t(), "closecombat", "secondary", json!(3), SECONDARY_SITE, "random", json!([16])),
        rec(1, "p2a", t(), "closecombat", "crit", json!(false), CRIT_SITE, "randomChance", json!([1])),
        rec(1, "p2a", t(), "closecombat", "crit", json!(false), CRIT_SITE, "randomChance", json!([25, 24])),
        rec(1, "p2a", t(), "closecombat", "crit", json!(false), CRIT_SITE, "randomChance", json!([1, 0])),
        rec(1, "p2a", t(), "closecombat", "accuracy", json!(true), ACC_SITE, "random", json!([100, 100])),
        rec(1, "p2a", t(), "closecombat", "range", json!(0), DURATION_SITE, "random", json!([0])),
        rec(1, "p2a", t(), "closecombat", "range", json!(2), DURATION_SITE, "random", json!([5, 2])),
    ] {
        assert!(keyed_on_flat_input(vec![bad.clone()]).is_err(), "accepted {bad}");
    }
}
