//! Scenarios from Showdown's own test suite plus targeted additions, with Showdown's output
//! recorded by tools/protocol/ps-suite.js. The log, each side's requests and the end record must
//! match, except `|t:|` timestamps and the end record's input log. A write can group lines into
//! messages differently, so messages are compared flat.
#![cfg(feature = "ps-rng")]

use std::path::PathBuf;

use serde_json::{json, Value};
use vgc_engine_core::ps_stream::{BattleStream, StreamOptions};

fn flatten(messages: &[String]) -> Value {
    let mut log = Vec::new();
    let mut p1 = Vec::new();
    let mut p2 = Vec::new();
    let mut end = Value::Null;
    for message in messages {
        let mut lines = message.trim_end_matches('\n').split('\n');
        match lines.next() {
            Some("update") => {
                for line in lines.filter(|l| !l.is_empty() && *l != "|") {
                    log.push(Value::from(if line.starts_with("|t:|") { "|t:|" } else { line }));
                }
            }
            Some("sideupdate") => {
                let seat = lines.next();
                if let Some(line) = lines.next().filter(|l| l.starts_with("|request|")) {
                    if seat == Some("p1") { p1.push(Value::from(line)) } else { p2.push(Value::from(line)) }
                }
            }
            Some("end") => {
                let body: Vec<&str> = lines.collect();
                end = serde_json::from_str(&body.join("\n")).unwrap_or(Value::Null);
                if let Some(record) = end.as_object_mut() {
                    record.remove("inputLog");
                }
            }
            _ => {}
        }
    }
    json!({ "log": log, "requests": { "p1": p1, "p2": p2 }, "end": end })
}

#[test]
fn engine_matches_showdown_on_the_suite() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/protocol/ps-suite/scenarios.jsonl");
    let text = std::fs::read_to_string(&path).expect("scenarios.jsonl");
    let mut failures = Vec::new();
    let mut total = 0;
    for line in text.lines().filter(|l| !l.is_empty()) {
        total += 1;
        let scenario: Value = serde_json::from_str(line).expect("scenario");
        let name = scenario["game"].as_str().unwrap_or("?");
        let mut stream = BattleStream::new(StreamOptions::default());
        let mut got = Vec::new();
        for input in scenario["input_log"].as_array().expect("input_log") {
            if stream.write(input.as_str().unwrap_or("")).is_err() {
                break;
            }
            while let Some(message) = stream.read() {
                got.push(message);
            }
        }
        if flatten(&got) != scenario["expected"] {
            failures.push(name.to_string());
        }
    }
    assert!(total > 100, "only {total} scenarios loaded");
    assert!(failures.is_empty(), "{} of {total} differ: {:#?}", failures.len(), failures);
}
