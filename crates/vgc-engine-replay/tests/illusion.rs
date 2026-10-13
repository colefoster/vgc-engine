//! Illusion in real replays (docs/protocol.md, "Illusion"). The fixtures are
//! held-out Reg M-C replays with the player names replaced by `p1` / `p2`.

use serde_json::Value;
use vgc_engine_core::{ProtocolView, SideRef};
use vgc_engine_replay::position::{parse_input, reconstruct, ReconOptions, Reconstruction};
use vgc_engine_replay::prior::Prior;

/// Zoroark-Hisui leads as Whimsicott; Dire Claw breaks it on turn 1.
const REVEAL: &str = include_str!("fixtures/illusion_reveal.json");
/// Zoroark-Hisui leads as Camerupt and faints to its own Memento; the real
/// Camerupt comes in on turn 4.
const FAINT: &str = include_str!("fixtures/illusion_faint.json");
/// Zoroark-Hisui leads as Kingambit (Bitter Malice) and switches out for the
/// real one; later it comes in as Blaziken-Mega for the real Blaziken-Mega
/// and shows Life Orb. Never revealed by `|replace|`.
const MEGA: &str = include_str!("fixtures/illusion_mega.json");
/// Zoroark leads as Palafin, switches out for the real Palafin (which is then
/// hit without a reveal), and comes back as Palafin-Hero after Flip Turn.
const SELF_SWITCH: &str = include_str!("fixtures/illusion_selfswitch.json");
/// Zoroark-Hisui comes in as Charizard-Mega-Y for the real one; revealed.
const CHARIZARD: &str = include_str!("fixtures/illusion_charizard.json");

fn lines(f: &str) -> Vec<String> {
    parse_input(f).unwrap().lines
}

fn rec(f: &str, turn: u32, lookahead: bool, viewer: Option<SideRef>) -> Reconstruction {
    let mut o = ReconOptions::new(turn);
    o.lookahead = lookahead;
    o.viewer = viewer;
    reconstruct(&lines(f), &o, &Prior::embedded()).unwrap_or_else(|e| panic!("turn {turn}: {e}"))
}

/// The report entry of `name` on `side`.
fn mon<'a>(r: &'a Reconstruction, side: &str, species_prefix: &str) -> &'a Value {
    r.report["sides"][side]["team"].as_array().unwrap().iter().find(|m| m["species"]["value"].as_str().unwrap().starts_with(species_prefix)).unwrap_or_else(|| panic!("{side} {species_prefix}: {}", r.report["sides"][side]))
}

/// The species in `side`'s active slot `slot` ("a" / "b").
fn active(r: &Reconstruction, side: &str, slot: &str) -> String {
    let tag = format!("{side}{slot}");
    let m = r.report["sides"][side]["team"].as_array().unwrap().iter().find(|m| m["active_slot"] == tag.as_str()).unwrap();
    m["species"]["value"].as_str().unwrap().to_string()
}

fn illusion(r: &Reconstruction) -> Vec<Value> {
    r.report["illusion"].as_array().cloned().unwrap_or_default()
}

fn turns(f: &str) -> Vec<u32> {
    lines(f).iter().filter_map(|l| l.strip_prefix("|turn|")).filter_map(|t| t.parse().ok()).collect()
}

#[test]
fn every_turn_of_every_fixture_rebuilds() {
    for f in [REVEAL, FAINT, MEGA, SELF_SWITCH, CHARIZARD] {
        for n in turns(f) {
            for lookahead in [false, true] {
                for viewer in [None, Some(SideRef::P1), Some(SideRef::P2)] {
                    let r = rec(f, n, lookahead, viewer);
                    let s = r.session(None).unwrap_or_else(|e| panic!("turn {n}: {e}"));
                    assert!(s.request_json(SideRef::P1).is_some() || s.request_json(SideRef::P2).is_some(), "turn {n}");
                    // One mon per active slot.
                    for side in [SideRef::P1, SideRef::P2] {
                        let sd = r.battle.side(side);
                        assert_ne!(sd.active[0], sd.active[1], "turn {n}");
                    }
                }
            }
        }
    }
}

#[test]
fn a_reveal_by_damage_hands_the_stint_to_the_holder() {
    // Turn 2: Zoroark-Hisui was Whimsicott until Dire Claw hit it.
    let r = rec(REVEAL, 2, false, None);
    assert_eq!(active(&r, "p1", "a"), "Zoroark-Hisui");
    let z = mon(&r, "p1", "Zoroark");
    // Dire Claw's damage (56/100) and Icy Wind are Zoroark's, inferred.
    let hp = z["hp"]["value"].as_u64().unwrap() as f64 / z["hp"]["max"].as_u64().unwrap() as f64;
    assert!((0.56..0.57).contains(&hp), "{hp}");
    assert_eq!(z["hp"]["src"], "inferred");
    let icy = z["moves"].as_array().unwrap().iter().find(|m| m["value"] == "Icy Wind").unwrap();
    assert_eq!(icy["src"], "inferred");
    assert_eq!(z["ability"]["value"], "Illusion");
    // Whimsicott never came in: full HP on the bench, brought (a disguise
    // is a party member), no moves.
    let w = mon(&r, "p1", "Whimsicott");
    assert!(w["active_slot"].is_null());
    assert_eq!(w["hp"]["value"], w["hp"]["max"]);
    assert_eq!(w["brought"]["src"], "inferred");
    assert!(!w["moves"].as_array().unwrap().iter().any(|m| m["value"] == "Icy Wind" && m["src"] == "exact"));
    let ill = illusion(&r);
    assert_eq!(ill.len(), 1);
    assert_eq!(ill[0]["disguise"], "Whimsicott");
    assert_eq!(ill[0]["evidence"], "replace");
    assert_eq!(ill[0]["src"], "inferred");
    assert_eq!(ill[0]["active"], false);
    let req: Value = serde_json::from_str(&r.session(None).unwrap().request_json(SideRef::P1).unwrap()).unwrap();
    assert!(req["side"]["pokemon"][0]["details"].as_str().unwrap().starts_with("Zoroark-Hisui"));
}

#[test]
fn before_the_reveal_lookahead_decides() {
    // Turn 1: the disguise is all the log has shown.
    let off = rec(REVEAL, 1, false, None);
    assert_eq!(active(&off, "p1", "a"), "Whimsicott");
    assert!(off.report.get("illusion").is_none());
    // With lookahead the later `|replace|` names the holder; the session
    // keeps the disguise (the foe still sees Whimsicott).
    let on = rec(REVEAL, 1, true, None);
    assert_eq!(active(&on, "p1", "a"), "Zoroark-Hisui");
    let ill = illusion(&on);
    assert_eq!((ill[0]["evidence"].as_str(), ill[0]["active"].as_bool()), (Some("replace"), Some(true)));
    let mut s = on.session(None).unwrap();
    let before = s.stream(ProtocolView::Spectator).len();
    s.choose(SideRef::P1, "default").unwrap();
    s.choose(SideRef::P2, "default").unwrap();
    let new: Vec<&String> = s.stream(ProtocolView::Spectator)[before..].iter().collect();
    assert!(new.iter().any(|l| l.contains("p1a: Whimsicott") || l.starts_with("|replace|p1a: Zoroark")), "{new:?}");
    assert!(!new.iter().any(|l| l.starts_with("|switch|p1a: Zoroark")), "{new:?}");
}

#[test]
fn the_viewer_knows_only_what_it_was_shown() {
    // The owner's request names the real active: viewer p1 uses it, viewer
    // p2 does not see it.
    let truth = rec(REVEAL, 1, true, None);
    let req: Value = serde_json::from_str(&truth.session(None).unwrap().request_json(SideRef::P1).unwrap()).unwrap();
    for (viewer, want) in [(SideRef::P1, "Zoroark-Hisui"), (SideRef::P2, "Whimsicott")] {
        let mut o = ReconOptions::new(1);
        o.viewer = Some(viewer);
        o.requests[0] = Some(req.clone());
        let r = reconstruct(&lines(REVEAL), &o, &Prior::embedded()).unwrap();
        assert_eq!(active(&r, "p1", "a"), want, "viewer {viewer:?}");
    }
    // The opponent learns it from the reveal only with lookahead.
    assert_eq!(active(&rec(REVEAL, 1, true, Some(SideRef::P2)), "p1", "a"), "Zoroark-Hisui");
    assert_eq!(active(&rec(REVEAL, 1, false, Some(SideRef::P2)), "p1", "a"), "Whimsicott");
}

#[test]
fn a_holder_that_faints_in_disguise_is_found_when_the_disguise_returns() {
    // Turn 5: the real Camerupt is in; the "Camerupt" that fainted on turn 2
    // was Zoroark-Hisui (Illusion ends silently on a faint).
    let r = rec(FAINT, 5, false, None);
    assert_eq!(active(&r, "p1", "a"), "Camerupt");
    let c = mon(&r, "p1", "Camerupt");
    assert_eq!(c["fainted"], false);
    assert_eq!(c["hp"]["value"], c["hp"]["max"]);
    let z = mon(&r, "p1", "Zoroark");
    assert_eq!(z["fainted"], true);
    assert_eq!(z["hp"]["src"], "inferred");
    assert_eq!(illusion(&r)[0]["evidence"], "faint");
    // Turn 3: the faint is not explained yet, but Memento is a move
    // Camerupt cannot learn and Zoroark-Hisui can.
    let r3 = rec(FAINT, 3, false, None);
    assert_eq!(illusion(&r3)[0]["evidence"], "move");
    assert_eq!(mon(&r3, "p1", "Camerupt")["fainted"], false);
    // Turn 2 (before Memento): the disguise, unless looking ahead.
    assert_eq!(active(&rec(FAINT, 2, false, None), "p1", "b"), "Camerupt");
    assert_eq!(active(&rec(FAINT, 2, true, None), "p1", "b"), "Zoroark-Hisui");
}

#[test]
fn zoroark_hisui_behind_a_mega_forme() {
    // Turn 2: Bitter Malice (Kingambit cannot learn it) gave the lead away.
    let r2 = rec(MEGA, 2, false, None);
    assert_eq!(active(&r2, "p2", "a"), "Zoroark-Hisui");
    assert_eq!(illusion(&r2)[0]["evidence"], "move");
    // Turn 3: the real Kingambit switched in for it.
    let r3 = rec(MEGA, 3, false, None);
    assert_eq!(active(&r3, "p2", "a"), "Kingambit");
    assert!(mon(&r3, "p2", "Zoroark")["active_slot"].is_null());
    // Turn 10: "Blaziken-Mega" switched in for Blaziken-Mega: one of the
    // two is the holder; the newcomer is the guess (default) ...
    let r10 = rec(MEGA, 10, false, None);
    assert_eq!(active(&r10, "p2", "b"), "Zoroark-Hisui");
    let last = illusion(&r10).last().cloned().unwrap();
    assert_eq!((last["evidence"].as_str(), last["src"].as_str()), (Some("self-switch"), Some("default")));
    assert_eq!(mon(&r10, "p2", "Zoroark")["hp"]["src"], "default");
    // ... which its Life Orb recoil confirms with lookahead: a Mega forme
    // holds its stone.
    let r10l = rec(MEGA, 10, true, None);
    let last = illusion(&r10l).last().cloned().unwrap();
    assert_eq!((last["evidence"].as_str(), last["src"].as_str()), (Some("item"), Some("inferred")));
    let b = mon(&r10l, "p2", "Blaziken");
    assert_eq!(b["species"]["value"], "Blaziken-Mega");
    assert!(b["active_slot"].is_null());
}

#[test]
fn plain_zoroark_and_a_hit_that_did_not_break_a_disguise() {
    // The turn-1 "Palafin" that came in for "Palafin" took Rock Slide with
    // no reveal: it is the real one, so the lead was Zoroark (inferred).
    let r = rec(SELF_SWITCH, 2, false, None);
    assert_eq!(active(&r, "p1", "b"), "Palafin");
    let ill = illusion(&r);
    assert_eq!((ill[0]["evidence"].as_str(), ill[0]["src"].as_str()), (Some("self-switch"), Some("inferred")));
    let p = mon(&r, "p1", "Palafin");
    let hp = p["hp"]["value"].as_u64().unwrap() as f64 / p["hp"]["max"].as_u64().unwrap() as f64;
    assert!((0.74..0.75).contains(&hp), "{hp}");
    let z = mon(&r, "p1", "Zoroark");
    assert_eq!(z["hp"]["value"], z["hp"]["max"]);
    // Turn 3: revealed after Flip Turn brought it back as Palafin-Hero.
    let r3 = rec(SELF_SWITCH, 3, false, None);
    assert_eq!(active(&r3, "p1", "b"), "Zoroark");
    assert_eq!(active(&r3, "p1", "a"), "Palafin-Hero");
}

#[test]
fn a_disguise_of_a_mega_that_switched_out_rebuilds() {
    // Was `unsupported: illusion (unrevealed)`: Zoroark-Hisui came in as
    // Charizard-Mega-Y for the real one, then the real one came back.
    for n in [4, 5] {
        let r = rec(CHARIZARD, n, false, None);
        assert_eq!(active(&r, "p2", "a"), "Charizard-Mega-Y", "turn {n}");
    }
    let r = rec(CHARIZARD, 4, false, None);
    assert_eq!(active(&r, "p2", "b"), "Zoroark-Hisui");
}

#[test]
fn the_rebuild_is_deterministic() {
    for f in [REVEAL, MEGA] {
        for n in turns(f) {
            for seed in [None, Some(7u64)] {
                let run = || {
                    let mut o = ReconOptions::new(n);
                    o.prior_sample_seed = seed;
                    let r = reconstruct(&lines(f), &o, &Prior::embedded()).unwrap();
                    serde_json::to_string(&(r.battle_json(), r.report.clone())).unwrap()
                };
                assert_eq!(run(), run(), "turn {n}");
            }
        }
    }
}
