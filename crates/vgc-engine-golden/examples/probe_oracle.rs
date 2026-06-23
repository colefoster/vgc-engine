use vgc_engine_golden::*;

fn main() {
    let dir = default_goldens_dir();
    let goldens = collect_goldens_in(&dir);
    for (name, input_p, ps_p) in goldens.iter().take(8) {
        let input: GoldenInput = serde_json::from_slice(&std::fs::read(input_p).unwrap()).unwrap();
        let ps: PsOutput = serde_json::from_slice(&std::fs::read(ps_p).unwrap()).unwrap();
        let rep = run_explore_in_memory(&input, &ps).unwrap();
        let bal = rep.divergences.iter().find(|d| d.kind == "rng-balance");
        let info = bal.map(|d| format!("{} / {}", d.engine_value, d.ps_value)).unwrap_or("balanced".into());
        println!("{name}: ps_rng={} | {info}", ps.rng.len());
    }
}
