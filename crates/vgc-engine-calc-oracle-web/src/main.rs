//! Local webui for the calc-oracle scenario suite.
//!
//! `cargo run -p vgc-engine-calc-oracle-web` → binds 127.0.0.1:8787 and
//! serves a vanilla-JS single-page app that reads scenarios/cache off
//! disk under `tools/calc-oracle/`, runs them through
//! `vgc_engine_golden::observe_scenario`, and diffs against the
//! `@smogon/calc` expected 16-roll union.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::Value;

use vgc_engine_golden::calc_cache::{
    cache_path_for_stem, collect_scenarios, invalidate_cache_for_stem, load_or_generate_calc,
    oracle_dir, repo_root, scenario_path_for_stem, KNOWN_FAILURES,
};
use vgc_engine_golden::{classify_deltas, observe_scenario, CalcExpectation, Scenario};

// ---------------------------------------------------------------------------
// Static assets — embedded so the binary is a single artifact.
// ---------------------------------------------------------------------------

const INDEX_HTML: &str = include_str!("static_files/index.html");
const APP_JS: &str = include_str!("static_files/app.js");
const STYLE_CSS: &str = include_str!("static_files/style.css");

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ScenarioSummary {
    stem: String,
    name: String,
    attacker_species: String,
    attacker_item: String,
    defender_species: String,
    #[serde(rename = "move")]
    move_name: String,
    weather: String,
    terrain: String,
    source: &'static str, // "hand" | "gen"
    cached: bool,
    known_failure: bool,
}

#[derive(Serialize)]
struct ScenarioDetail {
    scenario: Value,
    calc: Option<Value>,
}

#[derive(Serialize)]
struct RunResult {
    stem: String,
    observed: Vec<u16>,
    expected_union: Vec<u32>,
    expected_noncrit: Vec<u32>,
    expected_crit: Vec<u32>,
    out_of_spec: Vec<u16>,
    pass: bool,
    known_failure: bool,
    target_max_hp: u16,
    desc: String,
    diagnosis: Option<String>,
    delta_histogram: Vec<i32>,
    err: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn summary_for(path: &std::path::Path) -> Option<ScenarioSummary> {
    let stem = path.file_stem()?.to_str()?.to_string();
    let bytes = std::fs::read(path).ok()?;
    let raw: Value = serde_json::from_slice(&bytes).ok()?;
    let name = raw.get("name").and_then(|v| v.as_str()).unwrap_or(&stem).to_string();
    let attacker_species = raw
        .pointer("/attacker/species")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let attacker_item = raw
        .pointer("/attacker/item")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let defender_species = raw
        .pointer("/defender/species")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let move_name = raw.get("move").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let weather = raw
        .pointer("/field/weather")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let terrain = raw
        .pointer("/field/terrain")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // Anything under `tools/calc-oracle/generated/` was produced by
    // `gen_calc_scenarios`; everything else at the top of `calc-oracle/`
    // is hand-authored.
    let source: &'static str = if path
        .components()
        .any(|c| c.as_os_str() == "generated")
    {
        "gen"
    } else {
        "hand"
    };
    let cached = cache_path_for_stem(&stem).exists();
    let known_failure = KNOWN_FAILURES.contains(&stem.as_str());
    Some(ScenarioSummary {
        stem,
        name,
        attacker_species,
        attacker_item,
        defender_species,
        move_name,
        weather,
        terrain,
        source,
        cached,
        known_failure,
    })
}

fn expected_sets(calc: &CalcExpectation) -> (Vec<u32>, Vec<u32>, Vec<u32>) {
    let noncrit: Vec<u32> = calc.damage.clone();
    let crit: Vec<u32> = calc.damage_crit.clone();
    let mut union_set: BTreeSet<u32> = if !calc.damage_union.is_empty() {
        calc.damage_union.iter().copied().collect()
    } else {
        noncrit.iter().chain(crit.iter()).copied().collect()
    };
    // Sorted, dedup'd.
    let union: Vec<u32> = union_set.iter().copied().collect();
    let _ = &mut union_set;
    (union, noncrit, crit)
}

fn desc_for(sc: &Scenario) -> String {
    let mut s = format!("{} → {}", sc.attacker.species, sc.move_name);
    s.push_str(" → ");
    s.push_str(&sc.defender.species);
    if let Some(field) = &sc.field {
        let mut bits = Vec::new();
        if let Some(w) = &field.weather { if !w.is_empty() { bits.push(format!("weather={w}")); } }
        if let Some(t) = &field.terrain { if !t.is_empty() { bits.push(format!("terrain={t}")); } }
        if !bits.is_empty() {
            s.push_str(" [");
            s.push_str(&bits.join(", "));
            s.push(']');
        }
    }
    s
}

fn slugify_name(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}

fn run_stem(stem: &str) -> RunResult {
    let known_failure = KNOWN_FAILURES.contains(&stem);
    let path = match scenario_path_for_stem(stem) {
        Some(p) => p,
        None => {
            return RunResult {
                stem: stem.to_string(),
                observed: vec![],
                expected_union: vec![],
                expected_noncrit: vec![],
                expected_crit: vec![],
                out_of_spec: vec![],
                pass: false,
                known_failure,
                target_max_hp: 0,
                desc: String::new(),
                diagnosis: None,
                delta_histogram: vec![],
                err: Some(format!("scenario not found: {stem}")),
            };
        }
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return err_result(stem, known_failure, format!("read: {e}")),
    };
    let scenario: Scenario = match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(e) => return err_result(stem, known_failure, format!("parse scenario: {e}")),
    };
    let calc = match load_or_generate_calc(&path, false) {
        Ok(Some(c)) => c,
        Ok(None) => return err_result(stem, known_failure, "no cache and node unavailable".into()),
        Err(e) => return err_result(stem, known_failure, e),
    };
    let obs = match observe_scenario(&scenario) {
        Ok(o) => o,
        Err(e) => return err_result(stem, known_failure, format!("observe: {e}")),
    };
    let (union, noncrit, crit) = expected_sets(&calc);
    let expected_set: BTreeSet<u32> = union.iter().copied().collect();
    let out_of_spec: Vec<u16> = obs
        .observed_unique
        .iter()
        .copied()
        .filter(|v| !expected_set.contains(&(*v as u32)))
        .collect();
    let pass = out_of_spec.is_empty()
        && !(obs.observed_unique.is_empty() && expected_set.iter().any(|v| *v > 0));
    let (diagnosis, delta_histogram) = if pass {
        (None, vec![])
    } else {
        let (label, deltas) = classify_deltas(&obs.observed_damage, &union, obs.target_max_hp);
        (Some(label), deltas)
    };
    RunResult {
        stem: stem.to_string(),
        observed: obs.observed_damage,
        expected_union: union,
        expected_noncrit: noncrit,
        expected_crit: crit,
        out_of_spec,
        pass,
        known_failure,
        target_max_hp: obs.target_max_hp,
        desc: desc_for(&scenario),
        diagnosis,
        delta_histogram,
        err: None,
    }
}

fn err_result(stem: &str, known_failure: bool, msg: String) -> RunResult {
    RunResult {
        stem: stem.to_string(),
        observed: vec![],
        expected_union: vec![],
        expected_noncrit: vec![],
        expected_crit: vec![],
        out_of_spec: vec![],
        pass: false,
        known_failure,
        target_max_hp: 0,
        desc: String::new(),
        diagnosis: None,
        delta_histogram: vec![],
        err: Some(msg),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_scenarios() -> Json<Vec<ScenarioSummary>> {
    let mut out = Vec::new();
    for p in collect_scenarios() {
        if let Some(s) = summary_for(&p) {
            out.push(s);
        }
    }
    Json(out)
}

async fn get_scenario(AxumPath(stem): AxumPath<String>) -> Response {
    let Some(path) = scenario_path_for_stem(&stem) else {
        return (StatusCode::NOT_FOUND, format!("no scenario: {stem}")).into_response();
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("read: {e}")).into_response(),
    };
    let scenario: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("parse: {e}")).into_response(),
    };
    let calc_path = cache_path_for_stem(&stem);
    let calc: Option<Value> = if calc_path.exists() {
        std::fs::read(&calc_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    } else {
        None
    };
    Json(ScenarioDetail { scenario, calc }).into_response()
}

async fn run_one(AxumPath(stem): AxumPath<String>) -> Json<RunResult> {
    let stem_owned = stem.clone();
    let r = tokio::task::spawn_blocking(move || run_stem(&stem_owned))
        .await
        .unwrap_or_else(|e| err_result(&stem, false, format!("join: {e}")));
    Json(r)
}

async fn run_all() -> Response {
    // NDJSON stream — spawn_blocking each scenario, send one line at a time.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(16);
    tokio::spawn(async move {
        let stems: Vec<String> = collect_scenarios()
            .into_iter()
            .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
            .collect();
        for stem in stems {
            let r = tokio::task::spawn_blocking({
                let stem = stem.clone();
                move || run_stem(&stem)
            })
            .await
            .unwrap_or_else(|e| err_result(&stem, false, format!("join: {e}")));
            let mut line = match serde_json::to_string(&r) {
                Ok(s) => s,
                Err(e) => format!("{{\"stem\":\"{stem}\",\"err\":\"serialize: {e}\"}}"),
            };
            line.push('\n');
            if tx.send(Ok(line)).await.is_err() {
                break;
            }
        }
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}

async fn put_scenario(
    AxumPath(stem): AxumPath<String>,
    Json(body): Json<Value>,
) -> Response {
    // Validate by parsing into Scenario.
    if let Err(e) = serde_json::from_value::<Scenario>(body.clone()) {
        return (StatusCode::BAD_REQUEST, format!("invalid scenario: {e}")).into_response();
    }
    let Some(path) = scenario_path_for_stem(&stem) else {
        return (StatusCode::NOT_FOUND, format!("no scenario: {stem}")).into_response();
    };
    let pretty = match serde_json::to_string_pretty(&body) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("serialize: {e}")).into_response(),
    };
    if let Err(e) = std::fs::write(&path, pretty) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("write: {e}")).into_response();
    }
    let _ = invalidate_cache_for_stem(&stem);
    // Kick off cache regen in the background; client will POST /api/run.
    let path_bg = path.clone();
    tokio::task::spawn_blocking(move || {
        let _ = load_or_generate_calc(&path_bg, false);
    });
    Json(body).into_response()
}

async fn post_scenario(Json(body): Json<Value>) -> Response {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing name").into_response();
    }
    if let Err(e) = serde_json::from_value::<Scenario>(body.clone()) {
        return (StatusCode::BAD_REQUEST, format!("invalid scenario: {e}")).into_response();
    }
    let stem = format!("scenario-{}", slugify_name(&name));
    let path: PathBuf = oracle_dir().join(format!("{stem}.json"));
    if path.exists() {
        return (StatusCode::CONFLICT, format!("already exists: {stem}")).into_response();
    }
    let pretty = match serde_json::to_string_pretty(&body) {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("serialize: {e}")).into_response(),
    };
    if let Err(e) = std::fs::write(&path, pretty) {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("write: {e}")).into_response();
    }
    let path_bg = path.clone();
    tokio::task::spawn_blocking(move || {
        let _ = load_or_generate_calc(&path_bg, false);
    });
    (StatusCode::CREATED, Json(serde_json::json!({ "stem": stem }))).into_response()
}

// ---------------------------------------------------------------------------
// Git history
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct HistoryEntry {
    sha: String,
    date: String,
    subject: String,
    path: String,
}

async fn scenario_history(AxumPath(stem): AxumPath<String>) -> Response {
    let Some(sc_path) = scenario_path_for_stem(&stem) else {
        return (StatusCode::NOT_FOUND, format!("no scenario: {stem}")).into_response();
    };
    let cache_path = cache_path_for_stem(&stem);
    let root = repo_root();
    let mut entries: Vec<HistoryEntry> = Vec::new();
    for (path, label) in [(&sc_path, "scenario"), (&cache_path, "calc")] {
        // git log for this path — read-only.
        let rel = match path.strip_prefix(&root) {
            Ok(p) => p.to_path_buf(),
            Err(_) => path.clone(),
        };
        let out = std::process::Command::new("git")
            .arg("-C").arg(&root)
            .args([
                "log",
                "--follow",
                "--pretty=format:%H%x1f%cI%x1f%s",
                "--",
            ])
            .arg(&rel)
            .output();
        let Ok(out) = out else { continue };
        if !out.status.success() { continue; }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let parts: Vec<&str> = line.split('\u{1f}').collect();
            if parts.len() < 3 { continue; }
            entries.push(HistoryEntry {
                sha: parts[0].to_string(),
                date: parts[1].to_string(),
                subject: parts[2].to_string(),
                path: label.to_string(),
            });
        }
    }
    // Sort newest first by date string (ISO-8601 lexicographic).
    entries.sort_by(|a, b| b.date.cmp(&a.date));
    Json(entries).into_response()
}

// ---------------------------------------------------------------------------
// Batch regen
// ---------------------------------------------------------------------------

async fn regen_matrix() -> Response {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, std::io::Error>>(64);
    tokio::spawn(async move {
        use std::io::{BufRead, BufReader};
        let root = repo_root();
        let mut child = match std::process::Command::new("cargo")
            .arg("run").arg("--release")
            .arg("-p").arg("vgc-engine-golden")
            .arg("--example").arg("gen_calc_scenarios")
            .current_dir(&root)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let line = format!("{{\"line\":\"spawn error: {e}\"}}\n");
                let _ = tx.send(Ok(line)).await;
                return;
            }
        };
        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines().flatten() {
                let escaped = serde_json::to_string(&line).unwrap_or_else(|_| "\"\"".into());
                let out = format!("{{\"line\":{escaped}}}\n");
                if tx.send(Ok(out)).await.is_err() { break; }
            }
        }
        let status = child.wait();
        let done = match status {
            Ok(s) => format!("{{\"done\":true,\"status\":\"{}\"}}\n", s),
            Err(e) => format!("{{\"done\":true,\"err\":\"{e}\"}}\n"),
        };
        let _ = tx.send(Ok(done)).await;
    });
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Static file handlers
// ---------------------------------------------------------------------------

async fn serve_index() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(INDEX_HTML))
        .unwrap()
}
async fn serve_app_js() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/javascript; charset=utf-8")
        .body(Body::from(APP_JS))
        .unwrap()
}
async fn serve_style_css() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "text/css; charset=utf-8")
        .body(Body::from(STYLE_CSS))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Entrypoint
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {}

#[tokio::main]
async fn main() {
    let state = AppState {};
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/app.js", get(serve_app_js))
        .route("/style.css", get(serve_style_css))
        .route("/api/scenarios", get(list_scenarios).post(post_scenario))
        .route("/api/scenarios/:stem", get(get_scenario).put(put_scenario))
        .route("/api/run/:stem", post(run_one))
        .route("/api/run-all", post(run_all))
        .route("/api/scenarios/:stem/history", get(scenario_history))
        .route("/api/regen", post(regen_matrix))
        .with_state(state);

    let addr: SocketAddr = "127.0.0.1:8787".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    eprintln!("calc-oracle webui: http://{addr}");
    axum::serve(listener, app).await.expect("serve");
}

// Keep `State` import warning-free for future middleware.
#[allow(dead_code)]
fn _unused(_s: State<AppState>) {}
