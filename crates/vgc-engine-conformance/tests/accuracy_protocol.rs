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
