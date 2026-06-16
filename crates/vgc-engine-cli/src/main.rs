//! `vgc-engine-cli` — debug harness.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vgc_engine_core as core;
use vgc_engine_replay as replay;

fn print_usage() {
    eprintln!(
        "usage: vgc-engine-cli <command> [...]\n\
         \n\
         commands:\n\
           info                      Print data-table sizes.\n\
           load <p1.json> <p2.json>   Load two team JSON files and print the battle repr.\n\
           replay-init <replay.json>  Reconstruct teams from a PS replay and print them.\n\
           score <replay.json>        Run the engine against a replay and print per-turn agreement.\n\
           score-corpus <dir> [N] [--oracle] [--smogon-stats <path>]   Score every replay JSON under <dir> (recursive); optional cap N. --oracle replays PS's crit outcomes via Rng::oracle_partial. --smogon-stats switches recon from CanonicalDefault to SmogonStatsRecon backed by the supplied moveset file.\n\
           help                      This message.\n"
    );
}

fn cmd_info() -> ExitCode {
    println!("species : {}", core::data::SPECIES.len());
    println!("moves   : {}", core::data::MOVES.len());
    println!("items   : {}", core::data::ITEMS.len());
    println!("abilities: {}", core::data::ABILITIES.len());
    println!("types   : {}", core::data::TYPE_NAMES.len());
    ExitCode::SUCCESS
}

fn cmd_load(args: &[String]) -> ExitCode {
    let [p1, p2] = match args {
        [a, b] => [a, b],
        _ => {
            eprintln!("load: expected exactly 2 path arguments");
            return ExitCode::from(2);
        }
    };
    let p1_json = match fs::read_to_string(p1) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {p1}: {e}"); return ExitCode::from(1); }
    };
    let p2_json = match fs::read_to_string(p2) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {p2}: {e}"); return ExitCode::from(1); }
    };
    let p1_team = match core::TeamBuilder::from_json(&p1_json) {
        Ok(t) => t,
        Err(e) => { eprintln!("parse p1: {e}"); return ExitCode::from(1); }
    };
    let p2_team = match core::TeamBuilder::from_json(&p2_json) {
        Ok(t) => t,
        Err(e) => { eprintln!("parse p2: {e}"); return ExitCode::from(1); }
    };
    let b = core::Battle::new(core::BattleConfig::default(), p1_team, p2_team);
    println!("format: {:?}", b.format());
    println!("p1 team ({} mons):", b.p1.team.len());
    for (i, m) in b.p1.team.iter().enumerate() {
        let s = m.species();
        println!(
            "  {}. {} L{} hp={}/{} atk={} def={} spa={} spd={} spe={}",
            i, s.slug, m.level, m.current_hp, m.stats.hp,
            m.stats.atk, m.stats.def, m.stats.spa, m.stats.spd, m.stats.spe,
        );
    }
    println!("p2 team ({} mons):", b.p2.team.len());
    for (i, m) in b.p2.team.iter().enumerate() {
        let s = m.species();
        println!(
            "  {}. {} L{} hp={}/{} atk={} def={} spa={} spd={} spe={}",
            i, s.slug, m.level, m.current_hp, m.stats.hp,
            m.stats.atk, m.stats.def, m.stats.spa, m.stats.spd, m.stats.spe,
        );
    }
    ExitCode::SUCCESS
}

fn cmd_replay_init(args: &[String]) -> ExitCode {
    let [path] = match args {
        [a] => [a],
        _ => {
            eprintln!("replay-init: expected exactly 1 path argument");
            return ExitCode::from(2);
        }
    };
    let json = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {path}: {e}"); return ExitCode::from(1); }
    };
    let r = match replay::Replay::from_json(&json) {
        Ok(r) => r,
        Err(e) => { eprintln!("parse: {e}"); return ExitCode::from(1); }
    };
    let init = match replay::RunnerInit::from_replay(&r, &replay::CanonicalDefault) {
        Ok(i) => i,
        Err(e) => { eprintln!("recon: {e}"); return ExitCode::from(1); }
    };
    println!("replay  : {}", r.id);
    println!("format  : {} ({:?})", r.format, init.format);
    println!("winner  : {}", r.winner.as_deref().unwrap_or("(none)"));
    print_recon_team("p1", &init.p1_team);
    print_recon_team("p2", &init.p2_team);
    ExitCode::SUCCESS
}

fn print_recon_team(label: &str, team: &[core::TeamMember]) {
    println!("{label} team ({} mons brought):", team.len());
    for (i, m) in team.iter().enumerate() {
        let marker = if i < 2 { "*" } else { " " };
        let moves = if m.moves.is_empty() {
            "(no moves observed)".to_string()
        } else {
            m.moves.join(",")
        };
        println!(
            "  {marker}{}. {} L{} nat={} ability={} item={} moves={}",
            i,
            m.species,
            m.level,
            m.nature,
            m.ability.as_deref().unwrap_or("?"),
            m.item.as_deref().unwrap_or("?"),
            moves,
        );
    }
}

fn cmd_score(args: &[String]) -> ExitCode {
    let [path] = match args {
        [a] => [a],
        _ => {
            eprintln!("score: expected exactly 1 path argument");
            return ExitCode::from(2);
        }
    };
    let json = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => { eprintln!("read {path}: {e}"); return ExitCode::from(1); }
    };
    let r = match replay::Replay::from_json(&json) {
        Ok(r) => r,
        Err(e) => { eprintln!("parse: {e}"); return ExitCode::from(1); }
    };
    let score = match replay::score_replay(
        &r,
        &replay::CanonicalDefault,
        0xC0FFEE_DEADBEEF,
        replay::DEFAULT_HP_TOLERANCE,
    ) {
        Ok(s) => s,
        Err(e) => { eprintln!("score: {e}"); return ExitCode::from(1); }
    };

    println!("replay         : {}", score.replay_id);
    println!("turns scored   : {}", score.per_turn.len());
    println!("turns stepped  : {}", score.turns_run);
    println!("agreement      : {:.1}% ({} / {})",
        score.agreement_pct * 100.0,
        score.per_turn.iter().filter(|t| t.agreed).count(),
        score.per_turn.len(),
    );
    println!();
    println!("turn  hp_l1  agreed  slots");
    for t in &score.per_turn {
        let l1 = if t.hp_l1.is_nan() {
            "  nan".to_string()
        } else {
            format!("{:5.3}", t.hp_l1)
        };
        println!(
            "{:>4}  {}  {:>6}  {:>5}",
            t.turn,
            l1,
            if t.agreed { "yes" } else { "no" },
            t.compared_slots,
        );
    }
    ExitCode::SUCCESS
}

fn cmd_score_corpus(args: &[String]) -> ExitCode {
    // Optional `--oracle` flag (in any position) enables the OracleRng
    // path: per-replay Crit events are extracted via
    // `replay::build_crit_oracle_for_replay` and fed into the engine
    // via `Rng::oracle_partial`. Falls back to Splitmix for un-recorded
    // draws (accuracy, secondaries, range).
    let mut positional: Vec<&str> = Vec::with_capacity(args.len());
    let mut use_oracle = false;
    let mut use_full_oracle = false;
    let mut rng_dump_dir: Option<&str> = None;
    let mut smogon_stats_path: Option<&str> = None;
    let mut prev_flag: Option<&str> = None;
    for a in args {
        if let Some(flag) = prev_flag.take() {
            match flag {
                "--rng-dump-dir" => rng_dump_dir = Some(a.as_str()),
                "--smogon-stats" => smogon_stats_path = Some(a.as_str()),
                _ => {}
            }
            continue;
        }
        if a == "--oracle" {
            use_oracle = true;
        } else if a == "--full-oracle" {
            use_full_oracle = true;
            use_oracle = true;
        } else if a == "--rng-dump-dir" {
            prev_flag = Some("--rng-dump-dir");
        } else if a == "--smogon-stats" {
            prev_flag = Some("--smogon-stats");
        } else {
            positional.push(a.as_str());
        }
    }
    let (dir, limit) = match positional.as_slice() {
        [d] => (*d, usize::MAX),
        [d, n] => match n.parse::<usize>() {
            Ok(k) => (*d, k),
            Err(_) => {
                eprintln!("score-corpus: <N> must be a positive integer");
                return ExitCode::from(2);
            }
        },
        _ => {
            eprintln!("score-corpus: expected <dir> [N] [--oracle] [--rng-dump-dir <dir>] [--smogon-stats <path>]");
            return ExitCode::from(2);
        }
    };

    // Build the recon strategy up-front. SmogonStatsRecon owns the parsed
    // stats; we hand `score_replay*` a `&dyn TeamRecon` view.
    let smogon_recon: Option<replay::SmogonStatsRecon> = match smogon_stats_path {
        Some(path) => match fs::read_to_string(path) {
            Ok(text) => match replay::parse_smogon_stats(&text) {
                Ok(stats) => {
                    println!("recon         : SmogonStatsRecon ({} species from {path})", stats.species.len());
                    Some(replay::SmogonStatsRecon::new(stats))
                }
                Err(e) => {
                    eprintln!("smogon-stats parse {path}: {e}");
                    return ExitCode::from(1);
                }
            },
            Err(e) => {
                eprintln!("smogon-stats read {path}: {e}");
                return ExitCode::from(1);
            }
        },
        None => {
            println!("recon         : CanonicalDefault");
            None
        }
    };

    let mut files: Vec<PathBuf> = Vec::new();
    if let Err(e) = collect_json_files(Path::new(dir), &mut files) {
        eprintln!("walk {dir}: {e}");
        return ExitCode::from(1);
    }
    files.sort();
    if files.len() > limit {
        files.truncate(limit);
    }
    if files.is_empty() {
        eprintln!("no JSON files under {dir}");
        return ExitCode::from(1);
    }

    let mut sweep_agreements: Vec<replay::ReplayScore> = Vec::new();
    let mut parse_failed: usize = 0;
    let mut recon_failed: usize = 0;
    let mut total_turns: usize = 0;
    let mut hp_div: usize = 0;
    let mut faint_div: usize = 0;
    let mut status_div: usize = 0;
    for path in &files {
        let Ok(json) = fs::read_to_string(path) else {
            parse_failed += 1;
            continue;
        };
        let r = match replay::Replay::from_json(&json) {
            Ok(r) => r,
            Err(_) => { parse_failed += 1; continue; }
        };
        // If a --rng-dump-dir was supplied, look for a sidecar
        // `<replay-id>.rng.json` and load it into the OraclePartial
        // queue. Missing or broken sidecars fall back to the standard
        // log-extraction oracle (or pure Splitmix if --oracle isn't set).
        let sidecar_events: Option<Vec<vgc_engine_core::rng::RngEvent>> = rng_dump_dir
            .and_then(|d| {
                let sidecar = PathBuf::from(d).join(format!("{}.rng.json", r.id));
                if sidecar.exists() {
                    replay::load_rng_dump(&sidecar).ok()
                } else {
                    None
                }
            });

        let scored = match (&smogon_recon, sidecar_events, use_oracle, use_full_oracle) {
            (Some(recon), Some(events), _, _) => replay::score_replay_with_events(
                &r, recon, 0xC0FFEE_DEADBEEF, replay::DEFAULT_HP_TOLERANCE, events,
            ),
            (Some(recon), None, _, true) => replay::score_replay_full_oracle(
                &r, recon, 0xC0FFEE_DEADBEEF, replay::DEFAULT_HP_TOLERANCE,
            ),
            (Some(recon), None, true, false) => replay::score_replay_oracle(
                &r, recon, 0xC0FFEE_DEADBEEF, replay::DEFAULT_HP_TOLERANCE,
            ),
            (Some(recon), None, false, false) => replay::score_replay(
                &r, recon, 0xC0FFEE_DEADBEEF, replay::DEFAULT_HP_TOLERANCE,
            ),
            (None, Some(events), _, _) => replay::score_replay_with_events(
                &r, &replay::CanonicalDefault, 0xC0FFEE_DEADBEEF,
                replay::DEFAULT_HP_TOLERANCE, events,
            ),
            (None, None, _, true) => replay::score_replay_full_oracle(
                &r, &replay::CanonicalDefault, 0xC0FFEE_DEADBEEF,
                replay::DEFAULT_HP_TOLERANCE,
            ),
            (None, None, true, false) => replay::score_replay_oracle(
                &r, &replay::CanonicalDefault, 0xC0FFEE_DEADBEEF,
                replay::DEFAULT_HP_TOLERANCE,
            ),
            (None, None, false, false) => replay::score_replay(
                &r, &replay::CanonicalDefault, 0xC0FFEE_DEADBEEF,
                replay::DEFAULT_HP_TOLERANCE,
            ),
        };
        match scored {
            Ok(s) if !s.per_turn.is_empty() => {
                total_turns += s.per_turn.len();
                hp_div += s.hp_diverged_turns();
                faint_div += s.faint_diverged_turns();
                status_div += s.status_diverged_turns();
                sweep_agreements.push(s);
            }
            Ok(_) => recon_failed += 1,
            Err(e) => {
                if recon_failed < 3 {
                    eprintln!("recon {}: {e}", path.display());
                }
                recon_failed += 1;
            }
        }
    }
    let agreements: Vec<f32> = sweep_agreements.iter().map(|s| s.agreement_pct).collect();

    println!("processed     : {}", files.len());
    println!("parse failed  : {parse_failed}");
    println!("recon failed  : {recon_failed}");
    println!("scored        : {}", agreements.len());
    if agreements.is_empty() {
        println!("(no successful scores)");
        return ExitCode::SUCCESS;
    }

    let mean = agreements.iter().sum::<f32>() / agreements.len() as f32;
    let mut sorted = agreements.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];

    println!("mean agreement   : {:.1}%", mean * 100.0);
    println!("median agreement : {:.1}%", median * 100.0);
    println!();
    println!("tolerance sweep (mean agreement at each HP-L1 tolerance):");
    for tol in [0.05f32, 0.10, 0.20, 0.40] {
        let mean_at: f32 = sweep_agreements
            .iter()
            .map(|score| score.agreement_at(tol))
            .sum::<f32>()
            / sweep_agreements.len() as f32;
        let bar: String = "#".repeat(((mean_at * 40.0) as usize).min(40));
        println!("  ±{:>4.0}%  {:>5.1}%  {bar}", tol * 100.0, mean_at * 100.0);
    }
    println!();
    println!("divergence categories (of {total_turns} total turns):");
    println!("  hp_l1   : {hp_div:>5}  ({:.1}%)", pct(hp_div, total_turns));
    println!("  faint   : {faint_div:>5}  ({:.1}%)", pct(faint_div, total_turns));
    println!("  status  : {status_div:>5}  ({:.1}%)", pct(status_div, total_turns));

    let mut buckets = [0usize; 5]; // 0-20, 20-40, 40-60, 60-80, 80-100
    for a in &agreements {
        let idx = ((*a * 5.0).floor() as usize).min(4);
        buckets[idx] += 1;
    }
    let labels = ["0-20%  ", "20-40% ", "40-60% ", "60-80% ", "80-100%"];
    println!();
    println!("agreement distribution:");
    for (label, n) in labels.iter().zip(buckets.iter()) {
        let bar: String = "#".repeat(((n * 40) / agreements.len()).min(40));
        println!("  {label} {n:>5}  {bar}");
    }
    ExitCode::SUCCESS
}

fn pct(n: usize, d: usize) -> f32 {
    if d == 0 { 0.0 } else { (n as f32 / d as f32) * 100.0 }
}

fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        print_usage();
        return ExitCode::from(2);
    };
    match cmd.as_str() {
        "info" => cmd_info(),
        "load" => cmd_load(&args[1..]),
        "replay-init" => cmd_replay_init(&args[1..]),
        "score" => cmd_score(&args[1..]),
        "score-corpus" => cmd_score_corpus(&args[1..]),
        "help" | "--help" | "-h" => { print_usage(); ExitCode::SUCCESS }
        other => {
            eprintln!("unknown command: {other}");
            print_usage();
            ExitCode::from(2)
        }
    }
}
