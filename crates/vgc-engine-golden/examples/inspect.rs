fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens");
    let mut pairs: Vec<(String, std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    walk(&root, &root, &mut pairs);
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    for (stem, p, ps_path) in &pairs {
        match vgc_engine_golden::run_golden(p, ps_path) {
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

fn walk(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<(String, std::path::PathBuf, std::path::PathBuf)>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it.filter_map(|e| e.ok()).collect::<Vec<_>>(),
        Err(_) => return,
    };
    for entry in entries {
        let p = entry.path();
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
