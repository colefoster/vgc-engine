//! Shared scenario/cache helpers for the `calc_oracle_suite` integration
//! test and the `vgc-engine-calc-oracle-web` server. Both need to
//! discover scenarios on disk, resolve stems to paths, and load-or-
//! generate the `@smogon/calc` expectation JSON. Keeping the logic
//! (and the `KNOWN_FAILURES` allow-list) in one place prevents the two
//! call sites drifting.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::CalcExpectation;

/// Scenarios that currently diverge from `@smogon/calc`. When the
/// underlying bug is fixed the test inverts (the "known failure" starts
/// passing) so we know to remove it. Do NOT add without a code comment
/// explaining the root cause — silent allow-list rot masks regressions.
pub const KNOWN_FAILURES: &[&str] = &[
    // Tera Starstorm: engine produces a lower damage cluster ~60% of
    // spec on some rolls, suggesting Stellar first-attack STAB (2.0x)
    // isn't consistently applied on Terapagos-Terastal. Tera is banned
    // in Reg M-B (target format), so this is aspirational coverage.
    "scenario-stellar-tera-offtype",
];

/// Repo root, computed from `CARGO_MANIFEST_DIR` at build time (this
/// crate lives at `crates/vgc-engine-golden`, two levels down).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("canonicalize repo root")
}

pub fn oracle_dir() -> PathBuf {
    repo_root().join("tools").join("calc-oracle")
}

pub fn cache_dir() -> PathBuf {
    oracle_dir().join("cache")
}

/// Discover every `scenario-*.json` file under `tools/calc-oracle/`
/// (recursively into `generated/` etc; skips `cache/` and
/// `node_modules/`). Returns paths sorted lexicographically.
pub fn collect_scenarios() -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_scenarios(&oracle_dir(), &mut out);
    out.sort();
    out
}

fn walk_scenarios(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else { continue };
        if p.is_dir() {
            if name != "node_modules" && name != "cache" {
                walk_scenarios(&p, out);
            }
            continue;
        }
        if name.starts_with("scenario-") && name.ends_with(".json") {
            out.push(p);
        }
    }
}

/// Resolve a bare stem (`scenario-foo`) to an on-disk scenario path.
pub fn scenario_path_for_stem(stem: &str) -> Option<PathBuf> {
    collect_scenarios()
        .into_iter()
        .find(|p| p.file_stem().and_then(|s| s.to_str()) == Some(stem))
}

pub fn cache_path_for_stem(stem: &str) -> PathBuf {
    cache_dir().join(format!("{stem}.calc.json"))
}

pub fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Load the calc expectation from cache, or regenerate via
/// `node tools/calc-oracle/oracle.js <scenario>` if the cache is
/// missing / stale / `refresh=true`. Returns `Ok(None)` when we
/// couldn't produce one (cache miss AND node absent).
pub fn load_or_generate_calc(
    scenario: &Path,
    refresh: bool,
) -> Result<Option<CalcExpectation>, String> {
    let stem = scenario
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("bad scenario name: {}", scenario.display()))?;
    let cache_path = cache_path_for_stem(stem);

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

/// Delete the cached calc JSON for `stem`, if any. Used after a scenario
/// edit to force a fresh oracle run.
pub fn invalidate_cache_for_stem(stem: &str) -> std::io::Result<()> {
    let p = cache_path_for_stem(stem);
    match std::fs::remove_file(&p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
