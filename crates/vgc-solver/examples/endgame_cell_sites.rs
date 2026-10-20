//! Diagnostic: the recorded chance sites of a fixture's root cells.
//!   endgame_cell_sites <fixtures.jsonl> <line index> [max cells]
use std::io::BufRead;
use vgc_engine_core::{Battle, DrawSpace, Rng, SideRef};
use vgc_solver::recursive::joint_actions;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let idx: usize = a[2].parse().unwrap();
    let maxc: usize = a.get(3).map(|v| v.parse().unwrap()).unwrap_or(4);
    let line = std::io::BufReader::new(std::fs::File::open(&a[1]).unwrap()).lines().nth(idx).unwrap().unwrap();
    let rec: serde_json::Value = serde_json::from_str(&line).unwrap();
    let mut b: Battle = serde_json::from_str(rec["battle"].as_str().unwrap()).unwrap();
    b.rehydrate_caches();
    let rows = joint_actions(&b, SideRef::P1);
    let cols = joint_actions(&b, SideRef::P2);
    println!("rows {} cols {}", rows.len(), cols.len());
    let count = |s: &DrawSpace| -> u64 { match s {
        DrawSpace::UniformRange(n) => *n as u64,
        DrawSpace::UniformDamage { segments: Some(g), .. } => g.len as u64,
        DrawSpace::UniformDamage { .. } => 16,
        DrawSpace::UniformPercent { threshold: None } => 100,
        DrawSpace::UniformPercent { threshold: Some(t) } => if *t == 0 || *t >= 100 { 1 } else { 2 },
        DrawSpace::Crit { .. } => 2,
        DrawSpace::Tiebreak { speeds_tied } => if *speeds_tied { 2 } else { 1 },
    }};
    let mut all = Vec::new();
    for r in &rows { for c in &cols {
        let mut x = b.clone();
        x.set_rng(Rng::recording(0xC0DE));
        x.step(r, c);
        let log = x.rng_mut().take_recording_log().unwrap();
        let t: u64 = log.iter().map(|d| count(&d.space)).product();
        all.push((t, r.clone(), c.clone(), log));
    }}
    if a.iter().any(|x| x == "--dropped") {
        let cc = vgc_solver::ChanceCompression::default();
        let mut worst: Vec<(f64, usize)> = Vec::new();
        for (k, (_, r, c, _)) in all.iter().enumerate() {
            let (_f, d) = vgc_solver::enumerate_outcomes_compressed(&b, r, c, 0xC0DE, cc);
            worst.push((d, k));
        }
        worst.sort_by(|x, y| y.0.total_cmp(&x.0));
        let n = worst.len() as f64;
        println!("dropped mean {:.3} >0.05: {}/{}", worst.iter().map(|x| x.0).sum::<f64>() / n,
                 worst.iter().filter(|x| x.0 > 0.05).count(), worst.len());
        for &(d, k) in worst.iter().take(maxc) {
            let (t, r, c, log) = &all[k];
            println!("dropped {:.3} tensor {} {:?} x {:?}", d, t, r, c);
            for e in log { println!("   {:?} {:?} -> {}", e.key, e.space, count(&e.space)); }
        }
        return;
    }
    let mut ts: Vec<u64> = all.iter().map(|x| x.0).collect();
    ts.sort();
    println!("tensor p50 {} p90 {} max {}", ts[ts.len()/2], ts[ts.len()*9/10], ts[ts.len()-1]);
    all.sort_by(|a, b| b.0.cmp(&a.0));
    for (t, r, c, log) in all.iter().take(maxc) {
        println!("cell tensor {} {:?} x {:?}: {} sites", t, r, c, log.len());
        for d in log { println!("   {:?} {:?} -> {}", d.key, d.space, count(&d.space)); }
    }
}
