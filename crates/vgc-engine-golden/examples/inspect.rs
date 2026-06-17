fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens");
    let mut entries: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let p = entry.path();
        let name = p.file_name().unwrap().to_str().unwrap().to_string();
        if let Some(stem) = name.strip_suffix(".input.json") {
            let ps_path = dir.join(format!("{stem}.ps.json"));
            match vgc_engine_golden::run_golden(&p, &ps_path) {
                Ok(r) => {
                    println!(
                        "{stem}: turns={} matched={} diverged={}",
                        r.turns_run,
                        r.matched,
                        r.diverged.len()
                    );
                    for d in r.diverged.iter().take(5) {
                        println!("  turn {} [{}] {}", d.turn, d.kind, d.note);
                    }
                }
                Err(e) => println!("{stem}: error {e}"),
            }
        }
    }
}
