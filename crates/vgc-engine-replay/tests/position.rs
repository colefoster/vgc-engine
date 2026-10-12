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
