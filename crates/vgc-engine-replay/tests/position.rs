//! Real replay -> engine position (docs/protocol.md, "Start from a real replay").

use vgc_engine_core::SideRef;
use vgc_engine_replay::position::{detect_kind, parse_input, reconstruct, InputKind, ReconOptions, Stop};
use vgc_engine_replay::prior::Prior;

const SAMPLE: &str = include_str!("fixtures/sample.json");

fn turns(lines: &[String]) -> Vec<u32> {
    lines.iter().filter_map(|l| l.strip_prefix("|turn|")).filter_map(|t| t.parse().ok()).collect()
}

/// The last HP text the log showed for `ident`'s name on `side` before `cut`.
fn last_shown(lines: &[String], side: &str, name: &str) -> Option<String> {
    lines.iter().rev().find_map(|l| {
        let p: Vec<&str> = l.split('|').collect();
        let k = match p.get(1).copied() {
            Some("switch" | "drag") => 4,
            Some("-damage" | "-heal") => 3,
            _ => return None,
        };
        let id = p.get(2)?;
        (id.starts_with(side) && id.ends_with(&format!(": {name}"))).then(|| p.get(k).map(|s| s.split(' ').next().unwrap_or("").to_string())).flatten()
    })
}

#[test]
fn every_turn_of_a_real_replay_rebuilds_and_opens_a_session() {
    let input = parse_input(SAMPLE).unwrap();
    assert_eq!(detect_kind(&input.lines), InputKind::Spectator);
    let prior = Prior::embedded();
    let ts = turns(&input.lines);
    assert!(ts.len() >= 5);
    for &n in &ts {
        let rec = reconstruct(&input.lines, &ReconOptions::new(n), &prior).unwrap_or_else(|e| panic!("turn {n}: {e}"));
        assert_eq!(rec.battle.turn(), n - 1);
        let cut = input.lines.iter().position(|l| *l == format!("|turn|{n}")).unwrap();
        // The active mons' HP shows as the log last showed it.
        for (side, key) in [(SideRef::P1, "p1"), (SideRef::P2, "p2")] {
            let sd = rec.battle.side(side);
            for slot in 0..2 {
                let m = &sd.team[sd.active[slot] as usize];
                let name = rec.report["sides"][key]["team"][sd.active[slot] as usize]["name"].as_str().unwrap().to_string();
                let shown = last_shown(&input.lines[..cut], key, &name).unwrap();
                if m.fainted {
                    assert_eq!(shown, "0", "turn {n} {key} {name}");
                    continue;
                }
                let pct = (100 * m.current_hp as u32 / m.stats.hp as u32).max(1);
                assert_eq!(format!("{pct}/100"), shown.trim_end_matches(['y', 'g', 'r']), "turn {n} {key} {name}");
            }
        }
        let s = rec.session(None).unwrap();
        assert!(s.request_json(SideRef::P1).is_some() && s.request_json(SideRef::P2).is_some(), "turn {n}");
        // The prefix is the log up to the stop, chat dropped.
        let raw = vgc_engine_core::ps_position::prefix_raw_lines(&rec.prefix).unwrap();
        assert_eq!(raw.last().map(String::as_str), Some(format!("|turn|{n}").as_str()));
        assert!(!raw.iter().any(|l| l.starts_with("|j|") || l.starts_with("|c|")));
    }
}

#[test]
fn the_report_flags_every_field() {
    let input = parse_input(SAMPLE).unwrap();
    let rec = reconstruct(&input.lines, &ReconOptions::new(3), &Prior::embedded()).unwrap();
    let r = &rec.report;
    assert_eq!(r["input"], "spectator");
    let active = r["active_prior_fields"].as_object().unwrap();
    assert_eq!(active.len(), 4);
    for side in ["p1", "p2"] {
        let team = r["sides"][side]["team"].as_array().unwrap();
        assert_eq!(team.len(), 4);
        for m in team {
            for f in ["species", "item", "ability", "nature", "spread", "status", "boosts", "brought"] {
                let src = m[f]["src"].as_str().unwrap();
                assert!(["exact", "inferred", "prior", "default"].contains(&src), "{f}: {src}");
            }
            // A spectator never sees a spread: the prior fills it.
            assert!(m["prior_fields"].as_array().unwrap().iter().any(|x| x == "spread"));
            for mv in m["moves"].as_array().unwrap() {
                if mv["src"] == "prior" {
                    let p = mv["p"].as_f64().unwrap();
                    assert!((0.0..=1.0).contains(&p));
                }
            }
        }
    }
    assert!(r["n_prior_fields"].as_u64().unwrap() > 0);
    // The default resolves to an explicit, pinnable id with its file hashes.
    assert_eq!(r["format"], "gen9championsvgc2026regmc");
    assert_eq!(r["prior"], vgc_engine_replay::prior::DEFAULT_REGMC);
    let h = r["prior_sha256"].as_str().unwrap();
    assert!(h.split('+').all(|x| x.len() == 64), "{h}");
    let pinned = reconstruct(&input.lines, &ReconOptions::new(3), &Prior::load(vgc_engine_replay::prior::DEFAULT_REGMC).unwrap()).unwrap();
    assert_eq!(pinned.report, rec.report);
}

#[test]
fn a_missing_turn_or_stop_is_an_error() {
    let input = parse_input(SAMPLE).unwrap();
    let e = reconstruct(&input.lines, &ReconOptions::new(99), &Prior::none()).err().unwrap();
    assert_eq!(e.bucket, "no such turn");
    let mut o = ReconOptions::new(2);
    o.stop = Stop::Replace;
    // Turn 1 of the sample ends without a faint.
    if let Err(e) = reconstruct(&input.lines, &o, &Prior::none()) {
        assert_eq!(e.bucket, "no replacement stop");
    }
}

#[test]
fn no_prior_still_builds() {
    let input = parse_input(SAMPLE).unwrap();
    let rec = reconstruct(&input.lines, &ReconOptions::new(2), &Prior::none()).unwrap();
    assert_eq!(rec.report["prior"], "none");
    rec.session(None).unwrap();
}

/// SHA-256 of every reconstruction of the sample (all turns, each viewer,
/// with and without lookahead): battle, prefix, report, board and sets.
fn digest(opts: impl Fn(u32) -> ReconOptions) -> String {
    let input = parse_input(SAMPLE).unwrap();
    let prior = Prior::embedded();
    let mut all = String::new();
    for n in turns(&input.lines) {
        for viewer in [None, Some(SideRef::P1), Some(SideRef::P2)] {
            for lookahead in [false, true] {
                let mut o = opts(n);
                o.viewer = viewer;
                o.lookahead = lookahead;
                let rec = reconstruct(&input.lines, &o, &prior).unwrap();
                // `fairy_lock_turns` is always serialized since 0.5.0 (bincode
                // can't skip fields); drop the zero so the 0.4.1 digest still
                // guards every pre-existing byte.
                let mut battle = rec.battle_json();
                if battle["fairy_lock_turns"] == 0 {
                    battle.as_object_mut().unwrap().remove("fairy_lock_turns");
                }
                for v in [battle, rec.prefix_json(), rec.report.clone(), rec.board.clone(), rec.sets.clone()] {
                    all.push_str(&serde_json::to_string(&v).unwrap());
                    all.push('\n');
                }
            }
        }
    }
    vgc_engine_replay::prior::sha256_hex(all.as_bytes())
}

#[test]
fn argmax_output_is_byte_identical_to_0_4_1() {
    // Recorded at ad420d9 (0.4.1): without `prior_sample_seed` the prior
    // fill is the argmax, unchanged. Re-recorded when the sample's player
    // names were anonymized (names are the only input change).
    assert_eq!(digest(ReconOptions::new), "69cd8824beb3f1f18919906be941b6ffce60d056e576c1534e445633255bb47b");
}

fn sampled(seed: u64) -> ReconOptions {
    let mut o = ReconOptions::new(3);
    o.prior_sample_seed = Some(seed);
    o
}

#[test]
fn prior_samples_are_deterministic_per_seed() {
    let with = |seed| move |n| {
        let mut o = ReconOptions::new(n);
        o.prior_sample_seed = Some(seed);
        o
    };
    assert_eq!(digest(with(7)), digest(with(7)));
    assert_ne!(digest(with(7)), digest(with(8)));
    let input = parse_input(SAMPLE).unwrap();
    let rec = reconstruct(&input.lines, &sampled(7), &Prior::embedded()).unwrap();
    assert_eq!(rec.report["prior_sample_seed"], 7);
    let argmax = reconstruct(&input.lines, &ReconOptions::new(3), &Prior::embedded()).unwrap();
    assert!(argmax.report.get("prior_sample_seed").is_none());
}

#[test]
fn prior_samples_keep_revealed_info_and_legality() {
    let input = parse_input(SAMPLE).unwrap();
    let prior = Prior::embedded();
    let argmax = reconstruct(&input.lines, &ReconOptions::new(3), &prior).unwrap();
    let mut spreads = std::collections::HashSet::new();
    for seed in 0..40 {
        let rec = reconstruct(&input.lines, &sampled(seed), &prior).unwrap();
        for side in ["p1", "p2"] {
            let team = rec.report["sides"][side]["team"].as_array().unwrap();
            let items: Vec<&str> = team.iter().filter_map(|m| m["item"]["value"].as_str()).collect();
            assert_eq!(items.len(), items.iter().collect::<std::collections::HashSet<_>>().len(), "Item Clause, seed {seed}: {items:?}");
            for (k, m) in team.iter().enumerate() {
                let moves: Vec<&str> = m["moves"].as_array().unwrap().iter().map(|x| x["value"].as_str().unwrap()).collect();
                assert_eq!(moves.len(), moves.iter().collect::<std::collections::HashSet<_>>().len(), "duplicate move, seed {seed}: {moves:?}");
                let a = &argmax.report["sides"][side]["team"][k];
                for f in ["species", "item", "ability", "nature", "spread"] {
                    if a[f]["src"] == "exact" {
                        assert_eq!(m[f], a[f], "{side} {k} {f} seed {seed}");
                    }
                }
                for (j, mv) in a["moves"].as_array().unwrap().iter().enumerate() {
                    if mv["src"] == "exact" {
                        assert_eq!(m["moves"][j], *mv);
                    }
                }
                if m["spread"]["src"] == "prior" {
                    let p = m["spread"]["p"].as_f64().unwrap();
                    assert!(p > 0.0 && p <= 1.0);
                }
            }
        }
        spreads.insert(rec.report["sides"]["p2"]["team"][0]["spread"]["value"].to_string());
        rec.session(None).unwrap();
    }
    // Garchomp, a common species: different seeds, different spreads.
    assert!(spreads.len() > 3, "{spreads:?}");
}

#[test]
fn prior_sample_frequency_matches_the_prior() {
    let input = parse_input(SAMPLE).unwrap();
    let prior = Prior::embedded();
    let fmt = reconstruct(&input.lines, &ReconOptions::new(3), &prior).unwrap().format_id;
    let spreads = &prior.for_format(&fmt).get("Garchomp").unwrap().spreads;
    let total: f64 = spreads.iter().map(|s| s.p).sum();
    let n = 600;
    let mut counts = vec![0usize; spreads.len()];
    for seed in 0..n {
        let rec = reconstruct(&input.lines, &sampled(1000 + seed), &prior).unwrap();
        let g = &rec.report["sides"]["p2"]["team"][0];
        let vals: Vec<u16> = g["spread"]["value"]["values"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u16).collect();
        let nat = g["nature"]["value"].as_str().unwrap().to_ascii_lowercase();
        let k = spreads.iter().position(|s| s.values.as_slice() == vals.as_slice() && s.nature == nat).unwrap();
        assert!((g["spread"]["p"].as_f64().unwrap() - spreads[k].p).abs() < 1e-3, "p is rounded to 3 places");
        counts[k] += 1;
    }
    for (s, c) in spreads.iter().zip(&counts) {
        let want = s.p / total;
        let got = *c as f64 / n as f64;
        assert!((got - want).abs() < 0.06, "{} {:?}: {got:.3} vs {want:.3}", s.nature, s.values);
    }
}
