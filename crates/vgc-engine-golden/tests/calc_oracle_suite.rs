//! calc-oracle integration test suite.
//!
//! For every `scenario-*.json` in `tools/calc-oracle/`, asserts the
//! engine's observed damage set is a subset of `@smogon/calc`'s expected
//! 16-roll union (`damage ∪ damage_crit`). Any engine value outside that
//! set is a spec-level damage bug.
//!
//! Calc expectations are cached under `tools/calc-oracle/cache/` so CI
//! and cargo test don't need `node_modules/` on the fast path. Cache
//! misses fall back to invoking `node tools/calc-oracle/oracle.js`;
//! if `node` is also unavailable, the specific scenario is skipped
//! with a warning (never a false pass — cached scenarios still run).
//!
//! Env vars:
//!   VGC_CALC_ORACLE_REFRESH=1  — regenerate every cache entry.
//!   VGC_CALC_ORACLE_FILTER=<s> — only run scenarios whose stem contains `<s>`.
//!
//! Known failures: [`KNOWN_FAILURES`] lists scenarios that currently
//! diverge — the check inverts (test fails if a listed scenario starts
//! passing) so fixes surface as green-to-red flips and can be removed
//! from the list.

use std::collections::BTreeSet;
use std::path::Path;

use vgc_engine_golden::calc_cache::{
    collect_scenarios, load_or_generate_calc, oracle_dir, KNOWN_FAILURES,
};
use vgc_engine_golden::{observe_scenario, CalcExpectation, Scenario};

fn load_scenario(path: &Path) -> Result<Scenario, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
}

fn expected_set(calc: &CalcExpectation) -> BTreeSet<u32> {
    if !calc.damage_union.is_empty() {
        calc.damage_union.iter().copied().collect()
    } else {
        calc.damage
            .iter()
            .chain(calc.damage_crit.iter())
            .copied()
            .collect()
    }
}

#[test]
fn every_scenario_stays_within_calc_spec() {
    let scenarios = collect_scenarios();
    assert!(
        !scenarios.is_empty(),
        "no scenarios found under {}",
        oracle_dir().display()
    );

    let refresh = std::env::var("VGC_CALC_ORACLE_REFRESH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let filter = std::env::var("VGC_CALC_ORACLE_FILTER").ok();

    let mut ran = 0;
    let mut skipped = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut unexpected_passes: Vec<String> = Vec::new();

    for path in &scenarios {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if let Some(f) = &filter {
            if !stem.contains(f) {
                continue;
            }
        }

        let scenario = match load_scenario(path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("{stem}: scenario parse: {e}"));
                continue;
            }
        };

        let calc = match load_or_generate_calc(path, refresh) {
            Ok(Some(c)) => c,
            Ok(None) => {
                skipped.push(format!("{stem}: no cache and node unavailable"));
                continue;
            }
            Err(e) => {
                failures.push(format!("{stem}: {e}"));
                continue;
            }
        };

        let obs = match observe_scenario(&scenario) {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("{stem}: observe: {e}"));
                continue;
            }
        };

        let expected = expected_set(&calc);
        assert!(
            !expected.is_empty(),
            "{stem}: calc returned no expected damages"
        );

        let out_of_spec: Vec<u32> = obs
            .observed_unique
            .iter()
            .map(|v| *v as u32)
            .filter(|v| !expected.contains(v))
            .collect();

        let is_known = KNOWN_FAILURES.contains(&stem);
        // Guard against silent false-pass: since `observed_unique` is
        // a subset of `expected`, an EMPTY observed set (engine dealt
        // 0 damage on every roll) would pass the `out_of_spec.is_empty()`
        // check even though the calc says the move deals positive
        // damage. Under the `damage_only` API this should only happen
        // when the calc also expects zero (immunity / true-fail path);
        // otherwise the engine has a spec bug the harness must
        // surface. Skip only when `expected` is entirely zero (rare
        // but valid — some future no-op scenario).
        let expected_positive = expected.iter().any(|v| *v > 0);
        if obs.observed_unique.is_empty() && expected_positive && !is_known {
            failures.push(format!(
                "{stem}: engine observed 0 damage on all 16 rolls, but calc expected non-zero: {:?}",
                expected.iter().copied().collect::<Vec<u32>>()
            ));
            continue;
        }
        if !out_of_spec.is_empty() {
            if !is_known {
                let expected_sorted: Vec<u32> = expected.iter().copied().collect();
                failures.push(format!(
                    "{stem}: {} out-of-spec damage value(s) {:?}\n  expected: {:?}\n  engine observed: {:?}\n  trials: {} (fainted: {}, missed: {})",
                    out_of_spec.len(),
                    out_of_spec,
                    expected_sorted,
                    obs.observed_unique,
                    obs.trials,
                    obs.fainted_count,
                    obs.missed_count,
                ));
            }
        } else if is_known {
            unexpected_passes.push(stem.to_string());
        }
        ran += 1;
    }

    if !skipped.is_empty() {
        eprintln!(
            "calc-oracle: skipped {} scenario(s) (regen `node tools/calc-oracle/oracle.js ...` to populate cache):\n  {}",
            skipped.len(),
            skipped.join("\n  ")
        );
    }

    assert!(
        ran > 0,
        "no scenarios ran (all skipped: {:?}, failed: {:?})",
        skipped,
        failures
    );

    if !failures.is_empty() {
        panic!(
            "{} scenario(s) violated calc spec (out of {} run):\n\n{}",
            failures.len(),
            ran,
            failures.join("\n\n"),
        );
    }

    if !unexpected_passes.is_empty() {
        panic!(
            "{} scenario(s) in KNOWN_FAILURES now pass — remove from the list: {}",
            unexpected_passes.len(),
            unexpected_passes.join(", "),
        );
    }
}
