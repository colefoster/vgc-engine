//! Protocol-log goldens: PS battles (PS `a5df8274`, Champions, captured by
//! `tools/accuracy/ps-battle.js` with the full `battle.log` in
//! `_meta.rawlog`) that the engine replays with every PS random outcome
//! keyed in. Its `ps` log must equal PS's log byte for byte (`|t:|`
//! timestamps aside). The set covers a residual tie shuffle under Grassy
//! Terrain, Illusion, Dragon Darts, Perish Song faints and a plain win.
//! See docs/protocol.md.

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_protocol, AccBattle};

#[test]
fn protocol_logs_match_ps_byte_for_byte() {
    for name in ["2e5a883bfc", "13afc9f21f", "dd35730232", "4f679cf70f", "3a7001b7bb"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/accuracy/repros/protocol")
            .join(format!("{name}.json"));
        let acc: AccBattle = serde_json::from_str(&std::fs::read_to_string(&path).expect(name)).expect(name);
        let rec = replay_protocol(&acc);
        assert!(rec.state_clean, "{name}: state diverged ({:?})", rec.engine_error);
        assert!(rec.has_rawlog, "{name}: capture has no PS log");
        assert!(rec.identical, "{name}: first difference {:?}", rec.first_div);
    }
}

// ---------------------------------------------------------------------------
// Phase 2: per-side |request| JSON and player streams. The captures carry
// every chunk PS's getPlayerStreams delivered (`_meta.streams`) and every
// choice string each side sent (`_meta.cmds`). 2874c90eda is a gen 9 Doubles
// Custom Game (Tera, Shadow Tag, Arena Trap, Shed Shell, U-turn, rejected
// choices); its stream updates carry PS's `|debug|` lines, which the log
// doesn't write, so only its requests and errors are compared.
// ---------------------------------------------------------------------------

use vgc_engine_conformance::accuracy::{engine_requests, replay_session};

const CHAMPIONS: [&str; 5] = ["2e5a883bfc", "13afc9f21f", "dd35730232", "4f679cf70f", "3a7001b7bb"];
const GEN9: &str = "2874c90eda";

fn capture(name: &str) -> AccBattle {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/accuracy/repros/protocol").join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect(name)).expect(name)
}

#[test]
fn requests_and_player_streams_match_ps() {
    for name in CHAMPIONS.iter().chain(std::iter::once(&GEN9)) {
        let rec = replay_protocol(&capture(name));
        assert!(rec.state_clean, "{name}: state diverged");
        assert!(rec.has_streams, "{name}: capture has no PS streams");
        assert_eq!(rec.req_match, rec.req_total, "{name}: request {:?}", rec.req_div);
        if *name != GEN9 {
            assert_eq!(rec.streams_identical, [true; 4], "{name}: stream {:?} {:?}", rec.stream_bucket, rec.stream_div);
        }
    }
}

#[test]
fn session_replays_ps_choices_into_ps_streams() {
    for name in CHAMPIONS.iter().chain(std::iter::once(&GEN9)) {
        let rec = replay_session(&capture(name));
        assert!(rec.has_cmds, "{name}: capture has no choice strings");
        assert_eq!(rec.unfed, 0, "{name}: session stopped asking early");
        assert_eq!(rec.eng_errors, rec.ps_errors, "{name}: rejected choices differ");
        assert_eq!(rec.side_chunks_identical, [true; 2], "{name}: requests / errors {:?} {:?}", rec.bucket, rec.div);
        if *name != GEN9 {
            assert_eq!(rec.identical, [true; 4], "{name}: {:?} {:?}", rec.bucket, rec.div);
        }
    }
}

/// Checked-in PS requests (tools/accuracy/repros/protocol/requests/): team
/// preview, a move turn, a forced switch, a one-side switch request
/// (`noCancel`), a trapped mon and a Tera-available turn.
#[test]
fn checked_in_requests_are_byte_identical() {
    for fixture in ["team_preview", "move_turn", "forced_switch", "one_side_switch", "trapped", "tera"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/accuracy/repros/protocol/requests")
            .join(format!("{fixture}.json"));
        let f: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect(fixture)).expect(fixture);
        let side = if f["side"] == "p1" { 0 } else { 1 };
        let index = f["index"].as_u64().unwrap() as usize;
        let reqs = engine_requests(&capture(f["capture"].as_str().unwrap()));
        assert_eq!(reqs[side].get(index).map(String::as_str), f["request"].as_str(), "{fixture}");
    }
}
