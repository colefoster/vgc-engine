//! PS parity for `Battle::ally_targets`. For each scripted doubles position in
//! `tests/fixtures/ally-targets/` (turns played first), the engine's flag-on
//! `(move, target, tera/mega)` set equals the set PS's Side.chooseMove accepts
//! (sim/side.ts:586-672, sim/battle.ts:2399-2429 validTargetLoc), and the
//! flag-off set equals it minus the partner commands. Positions: plain moves,
//! a heal-blocked Pollen Puff user, Ghost / non-Ghost Curse and Acupressure,
//! a fainted partner, a commanding Tatsugiri partner, Choice-locked and
//! Encored actors, Mega variants (Champions). The PS side is regenerated with
//! `PS_DIST=<ps>/dist/sim node tools/ps-target-choices/target-choices.js
//! <name>.input.json > <name>.ps.json`.

use std::collections::BTreeSet;
use vgc_engine_core::{data, Battle, BattleConfig, Choice, Format, SideRef, TeamBuilder};
use vgc_engine_golden::parse_turn_actions;

fn fixtures() -> Vec<(String, serde_json::Value, serde_json::Value)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ally-targets");
    let mut out = Vec::new();
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if let Some(stem) = name.strip_suffix(".input.json") {
            let read = |q: std::path::PathBuf| serde_json::from_str(&std::fs::read_to_string(q).unwrap()).unwrap();
            out.push((stem.to_string(), read(p.clone()), read(dir.join(format!("{stem}.ps.json")))));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(out.len() >= 7, "fixtures missing");
    out
}

fn build(input: &serde_json::Value) -> Battle {
    let format = input["format"].as_str().unwrap();
    let champions = vgc_engine_core::format_rules::is_champions_format(format);
    let team = |k: &str| TeamBuilder::from_showdown_text_in(input[k]["team"].as_str().unwrap(), champions).unwrap();
    let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, team("p1"), team("p2"));
    b.set_format_id(format);
    if champions {
        // Champions has no Terastallization.
        b.p1.conditions.tera_used = true;
        b.p2.conditions.tera_used = true;
    }
    for turn in input["turns"].as_array().unwrap() {
        let p1 = parse_turn_actions(&turn["p1"], SideRef::P1, 2).unwrap();
        let p2 = parse_turn_actions(&turn["p2"], SideRef::P2, 2).unwrap();
        b.step(&p1, &p2);
    }
    b
}

/// Engine choice -> PS command (relative target: foe slot s = s+1, own slot
/// s = -(s+1)). The engine's targetless choice of an ally-auto move is PS's
/// `-<partner>` (adjacentAlly) or `-<self>` (adjacentAllyOrSelf). Known
/// flag-off divergence kept: non-Ghost Curse is `self` in PS's request
/// (sim/pokemon.ts:999-1002) but the engine still offers foe picks, so they
/// are folded into the targetless command.
fn ps_cmd(b: &Battle, side: SideRef, slot: u8, c: &Choice) -> Option<(u8, bool, String)> {
    let (move_slot, target, ev) = match *c {
        Choice::Move { move_slot, target, .. } => (move_slot, target, ""),
        Choice::Terastallize { move_slot, target, .. } => (move_slot, target, " terastallize"),
        Choice::MegaEvolve { move_slot, target, .. } => (move_slot, target, " mega"),
        _ => return None,
    };
    let mon = b.side(side).active_mon(slot as usize).unwrap();
    let move_id = mon.moves[move_slot as usize];
    let (types, n) = mon.effective_types();
    let non_ghost_curse = move_id == data::move_id::CURSE && !types[..n as usize].contains(&13);
    let rel = |s: SideRef, sl: u8| if s == side { -(sl as i32 + 1) } else { sl as i32 + 1 };
    let loc = match (target, b.moves()[move_id as usize].target) {
        _ if non_ghost_curse => None,
        (Some(t), _) => Some(rel(t.side, t.slot)),
        (None, 2) => Some(rel(side, 1 - slot)),
        (None, 3) => Some(rel(side, slot)),
        (None, _) => None,
    };
    let is_ally = target.is_some_and(|t| t.side == side && t.slot != slot);
    Some((move_slot, is_ally, format!("move {}{}{ev}", move_slot + 1, loc.map(|l| format!(" {l}")).unwrap_or_default())))
}

#[test]
fn flag_on_choices_match_ps_valid_targets() {
    let mut ally_seen = 0;
    for (name, input, ps) in fixtures() {
        let mut b = build(&input);
        for (side, key) in [(SideRef::P1, "p1"), (SideRef::P2, "p2")] {
            for slot in 0..2u8 {
                let Some(want) = ps[key][slot as usize].as_array() else { continue };
                // Known flag-off divergence kept (foe side unchanged): PS
                // also accepts a fainted or commanding foe slot
                // (validTargetLoc ignores state); the engine offers only
                // live, non-commanding foes.
                let foe = side.opposing();
                let dead_foe: Vec<i32> = (0..2u8)
                    .filter(|&s| !b.side(foe).active_mon(s as usize).is_some_and(|m| m.is_alive() && !m.commanding))
                    .map(|s| s as i32 + 1)
                    .collect();
                let target_of = |c: &str| c.split(' ').nth(2).and_then(|t| t.parse::<i32>().ok());
                let want: BTreeSet<String> = want
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .filter(|c| !target_of(c).is_some_and(|t| dead_foe.contains(&t)))
                    .collect();
                b.ally_targets = true;
                let on: Vec<(u8, bool, String)> = b.legal_choices(side, slot).iter().filter_map(|c| ps_cmd(&b, side, slot, c)).collect();
                let got: BTreeSet<String> = on.iter().map(|x| x.2.clone()).collect();
                assert_eq!(got, want, "{name} {key} slot {slot}");
                // Per move, the partner choices come after the move's others.
                for (k, x) in on.iter().enumerate() {
                    if x.1 {
                        ally_seen += 1;
                        assert!(!on[k..].iter().any(|y| y.0 == x.0 && !y.1), "{name} {key} slot {slot}: ally before foe");
                    }
                }
                b.ally_targets = false;
                let off: BTreeSet<String> = b
                    .legal_choices(side, slot)
                    .iter()
                    .filter_map(|c| ps_cmd(&b, side, slot, c))
                    .map(|x| x.2)
                    .collect();
                let ally_loc = -(2 - slot as i32);
                let want_off: BTreeSet<String> = want.iter().filter(|c| target_of(c) != Some(ally_loc)).cloned().collect();
                assert_eq!(off, want_off, "{name} {key} slot {slot} (flag off)");
            }
        }
    }
    assert!(ally_seen > 0, "fixtures exercise ally targets");
}
