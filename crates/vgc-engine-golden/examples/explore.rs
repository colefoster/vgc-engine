//! Exploratory comparison aggregator (PR-203).
//!
//! Walks `<dir>` (or `goldens/random/` by default) for `seed-*.input.json`
//! / `seed-*.ps.json` pairs, runs each through `run_explore`, and prints
//! a frequency-sorted punch list of structural divergences.
//!
//! Output shape:
//!   total goldens: N (clean: K, diverged: M)
//!   ===== by-kind =====
//!   move-divergence: <count>
//!   status-divergence: <count>
//!     - slp x 12
//!     - par x 8
//!   faint-divergence: <count>
//!     - Garchomp x 4
//!   ...

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use vgc_engine_golden::{run_explore_with_mode, ExploreDivergence, ExploreMode};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Parse a single optional `--psgen5` flag plus an optional path arg.
    // Default mode: OraclePartial (the historical behavior).
    let mut mode = ExploreMode::OraclePartial;
    let mut path_arg: Option<String> = None;
    for a in args.iter().skip(1) {
        if a == "--psgen5" {
            mode = ExploreMode::PsGen5;
        } else if !a.starts_with('-') {
            path_arg = Some(a.clone());
        }
    }
    let dir = if let Some(p) = path_arg {
        PathBuf::from(p)
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens").join("random")
    };
    eprintln!("explore mode: {mode:?}");

    if !dir.exists() {
        eprintln!("dir not found: {}", dir.display());
        std::process::exit(1);
    }

    let pairs = collect_pairs(&dir);
    if pairs.is_empty() {
        eprintln!("no goldens found in {}", dir.display());
        std::process::exit(1);
    }

    let mut total = 0usize;
    let mut clean = 0usize;
    let mut diverged = 0usize;
    let mut errored = 0usize;

    // kind → total count
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    // kind → label → count
    let mut by_label: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    // For abilities/items hint: which goldens diverged most?
    let mut worst_goldens: Vec<(String, usize)> = Vec::new();
    // PR-224 leverage diagnostic: the FIRST divergence per battle is the
    // one that gates the rest — closing it is the only PR that can move
    // that battle into `clean`. Bucket by `(kind, label)` and sort by
    // frequency to order the mechanic backlog by "battles unblocked".
    let mut first_div_by_kind: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_div_by_kind_label: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

    for (name, input, ps_path) in &pairs {
        total += 1;
        if !ps_path.exists() {
            errored += 1;
            continue;
        }
        match run_explore_with_mode(input, ps_path, mode) {
            Err(e) => {
                eprintln!("{name}: error {e}");
                errored += 1;
            }
            Ok(report) => {
                if report.divergences.is_empty() {
                    clean += 1;
                } else {
                    diverged += 1;
                    worst_goldens.push((name.clone(), report.divergences.len()));
                    // First-divergence picked by earliest turn, then by
                    // position in the report (the order divergences were
                    // appended). The `rng-balance` synthetic event uses
                    // turn=0 — exclude it when picking "first real" so a
                    // misaligned battle still gets credited to the
                    // mechanic that caused the misalignment, not to the
                    // generic balance summary.
                    if let Some(first) = report
                        .divergences
                        .iter()
                        .find(|d| d.kind != "rng-balance")
                        .or_else(|| report.divergences.first())
                    {
                        *first_div_by_kind.entry(first.kind.clone()).or_insert(0) += 1;
                        *first_div_by_kind_label
                            .entry(first.kind.clone())
                            .or_default()
                            .entry(first.label.clone())
                            .or_insert(0) += 1;
                    }
                    for d in &report.divergences {
                        bump_divergence(&mut by_kind, &mut by_label, d);
                    }
                }
            }
        }
    }

    println!("total goldens: {total} (clean: {clean}, diverged: {diverged}, errored: {errored})");
    println!();
    println!("===== by-kind =====");
    let mut ordered: Vec<(String, usize)> = by_kind.iter().map(|(k, v)| (k.clone(), *v)).collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (kind, count) in &ordered {
        println!("{kind}-divergence: {count}");
        if let Some(labels) = by_label.get(kind) {
            let mut lvec: Vec<(String, usize)> = labels.iter().map(|(k, v)| (k.clone(), *v)).collect();
            lvec.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            for (label, c) in lvec.iter().take(12) {
                let label_disp = if label.is_empty() { "<unknown>" } else { label.as_str() };
                println!("    - {label_disp} x {c}");
            }
        }
    }

    println!();
    println!("===== first-divergence leverage (battles blocked) =====");
    // Each battle counted once, by the kind+label of its FIRST diverging
    // event. This is the upper bound on how many `diverged` battles a
    // single mechanic PR could promote to `clean` — anything past the
    // first divergence is dead code until upstream sites align.
    let mut first_ordered: Vec<(String, usize)> =
        first_div_by_kind.iter().map(|(k, v)| (k.clone(), *v)).collect();
    first_ordered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (kind, count) in &first_ordered {
        println!("{kind}-first: {count}");
        if let Some(labels) = first_div_by_kind_label.get(kind) {
            let mut lvec: Vec<(String, usize)> =
                labels.iter().map(|(k, v)| (k.clone(), *v)).collect();
            lvec.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            for (label, c) in lvec.iter().take(12) {
                let label_disp = if label.is_empty() { "<unknown>" } else { label.as_str() };
                println!("    - {label_disp} x {c}");
            }
        }
    }

    println!();
    println!("===== worst goldens =====");
    worst_goldens.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in worst_goldens.iter().take(10) {
        println!("{name}: {n} divergences");
    }
}

fn bump_divergence(
    by_kind: &mut BTreeMap<String, usize>,
    by_label: &mut BTreeMap<String, BTreeMap<String, usize>>,
    d: &ExploreDivergence,
) {
    *by_kind.entry(d.kind.clone()).or_insert(0) += 1;
    *by_label
        .entry(d.kind.clone())
        .or_default()
        .entry(d.label.clone())
        .or_insert(0) += 1;
}

fn collect_pairs(dir: &Path) -> Vec<(String, PathBuf, PathBuf)> {
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf, PathBuf)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it.filter_map(|e| e.ok()).collect::<Vec<_>>(),
        Err(_) => return,
    };
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
            continue;
        }
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        if let Some(stem) = name.strip_suffix(".input.json") {
            let ps_path = p.with_file_name(format!("{stem}.ps.json"));
            let rel = p.strip_prefix(root).unwrap_or(&p);
            let qualified = rel.to_string_lossy().trim_end_matches(".input.json").to_string();
            out.push((qualified, p, ps_path));
        }
    }
}
