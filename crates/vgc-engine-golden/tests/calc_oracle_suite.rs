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

/// Scenarios that currently diverge from `@smogon/calc`. The entry
/// documents the mechanic gap. When the underlying bug is fixed, the
/// test inverts (the "known failure" starts passing) so we know to
/// remove it. Do NOT add to this list without a code comment
/// explaining the root cause — silent allow-list rot masks regressions.
const KNOWN_FAILURES: &[&str] = &[
    // Tera Starstorm: engine produces a lower damage cluster ~60% of
    // spec on some rolls, suggesting Stellar first-attack STAB (2.0x)
    // isn't consistently applied on Terapagos-Terastal. Tera is banned
    // in Reg M-B (target format), so this is aspirational coverage.
    "scenario-stellar-tera-offtype",
    // Beads of Ruin: engine emits calc's expected range PLUS an extra
    // ~1.33× band, suggesting the aura may double-apply when the
    // holder is Chi-Yu (which also has Beads-of-Ruin as its innate).
    "scenario-beadsofruin-chiyu-heatwave",
    // Grassy Terrain heal-tick confound: harness-limitation, not an
    // engine bug. Grounded defender heals 1/16 HP at end-of-turn
    // BEFORE the HP-delta read, so observed damage undercounts by
    // one heal tick. To fix, expose a scenario field that skips EOT
    // effects or reads HP mid-turn.
    "scenario-grassyterrain-rillaboom-woodhammer",
    "scenario-grassyterrain-earthquake-halving",
];

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use vgc_engine_golden::{observe_scenario, CalcExpectation, Scenario};

fn repo_root() -> PathBuf {
    // crates/vgc-engine-golden -> ../../
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize repo root")
}

fn oracle_dir() -> PathBuf {
    repo_root().join("tools").join("calc-oracle")
}

fn cache_dir() -> PathBuf {
    oracle_dir().join("cache")
}

fn collect_scenarios() -> Vec<PathBuf> {
    let dir = oracle_dir();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else { continue };
        if name.starts_with("scenario-") && name.ends_with(".json") {
            out.push(p);
        }
    }
    out.sort();
    out
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Load the calc expectation from cache, or regenerate via node if the
/// cache is missing / stale / a refresh was requested. Returns `Ok(None)`
/// when we couldn't produce one (cache miss AND node absent) so the
/// caller can skip rather than fail.
fn load_or_generate_calc(
    scenario: &Path,
    refresh: bool,
) -> Result<Option<CalcExpectation>, String> {
    let stem = scenario
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("bad scenario name: {}", scenario.display()))?;
    let cache_path = cache_dir().join(format!("{stem}.calc.json"));

    if !refresh && cache_path.exists() {
        let bytes = std::fs::read(&cache_path)
            .map_err(|e| format!("read cache {}: {e}", cache_path.display()))?;
        let expectation: CalcExpectation = serde_json::from_slice(&bytes)
            .map_err(|e| format!("parse cache {}: {e}", cache_path.display()))?;
        return Ok(Some(expectation));
    }

    if !node_available() {
        return Ok(None);
    }

    std::fs::create_dir_all(cache_dir())
        .map_err(|e| format!("mkdir cache: {e}"))?;
    let oracle_js = oracle_dir().join("oracle.js");
    let output = Command::new("node")
        .arg(&oracle_js)
        .arg(scenario)
        .output()
        .map_err(|e| format!("spawn node: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "node oracle.js failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        ));
    }
    std::fs::write(&cache_path, &output.stdout)
        .map_err(|e| format!("write cache {}: {e}", cache_path.display()))?;
    let expectation: CalcExpectation = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parse node output: {e}"))?;
    Ok(Some(expectation))
}

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
