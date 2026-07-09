//! Probe an Instruct coupling scenario: attacker A hits defender D once
//! (declared), Oranguru Instructs A → A re-hits D. If A's first hit is
//! collapsible (survivor spanning buckets) and D survives both, the
//! Instruct guard must disable collapse or states drop. Prints the frontier
//! diff (collapse ON vs full 16-roll) + whether Instruct actually fired.

use std::collections::HashMap;
use vgc_engine_core::{
    set_ko_split_disabled, Battle, BattleConfig, Choice, Format, SideRef, Target, TeamBuilder,
};
use vgc_solver::{diagnose_cell_sites, enumerate_outcomes_with, EnumerateOpts};
use vgc_engine_core::DrawSpace;

// A: bulky-neutral single-target attacker so D SURVIVES one hit (needed for
// the survivor collapse to engage and couple with the Instruct re-hit).
// Adapted from the engine's `instruct_repeats_targets_last_move` test:
// Oranguru (P1s0) Instructs Latios (P2s0) → Latios repeats Draco Meteor on
// Blissey (P1s1). Blissey is the DEFENDER hit twice; the declared scan sees
// only ONE Draco on it → the Instruct guard is what must disable collapse.
const P1: &str = r#"[
    {"species":"oranguru","level":50,"ability":"innerfocus","nature":"sassy","moves":["instruct","psychic","protect","trickroom"],"evs":{"hp":252}},
    {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["seismictoss","softboiled","protect","calmmind"],"evs":{"hp":252,"def":252}}
]"#;
const P2: &str = r#"[
    {"species":"latios","level":50,"ability":"levitate","nature":"timid","moves":["dracometeor","psychic","protect","recover"],"evs":{"spe":252}},
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["protect"],"evs":{"hp":252}}
]"#;

fn dist(b: &Battle, p1: &[Choice], p2: &[Choice]) -> HashMap<u64, f64> {
    let opts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
    let f = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts);
    let mut m = HashMap::new();
    for o in &f.outcomes { *m.entry(o.hash).or_insert(0.0) += o.prob; }
    m
}

fn main() {
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Oranguru (P1s0) Instructs Latios (P2s0); Blissey (P1s1) passes;
    // Latios (P2s0) Draco Meteors Blissey (P1s1); Snorlax passes.
    // Double-hit defender D = Blissey (P1 slot1).
    let p1 = [
        Choice::Move { actor_slot: 0, move_slot: 0, target: Some(Target { side: SideRef::P2, slot: 0 }) }, // Oranguru Instruct -> Latios
        Choice::Pass { actor_slot: 1 },                                                                    // Blissey pass
    ];
    let p2 = [
        Choice::Move { actor_slot: 0, move_slot: 0, target: Some(Target { side: SideRef::P1, slot: 1 }) }, // Latios Draco -> Blissey
        Choice::Pass { actor_slot: 1 },                                                                    // Snorlax pass
    ];

    let d0 = b.p1.team[1].current_hp;
    // With Instruct.
    let mut wi = b.clone();
    let _ = wi.step(&p1, &p2);
    // Without Instruct (Oranguru passes) — baseline single Draco.
    let p1_nopass = [Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }];
    let mut wo = b.clone();
    let _ = wo.step(&p1_nopass, &p2);
    println!(
        "D(Blissey) start={d0}  after 1 Draco (no instruct)={}  after instruct-turn={}  (instruct fired = {})",
        wo.p1.team[1].current_hp, wi.p1.team[1].current_hp,
        wi.p1.team[1].current_hp < wo.p1.team[1].current_hp
    );

    let (sites, raw) = diagnose_cell_sites(&b, &p1, &p2, 0xC0DE);
    let dmg: Vec<String> = sites.iter().filter_map(|(s,_)| match s {
        DrawSpace::UniformDamage { segments, .. } => Some(match segments {
            Some(seg) => format!("seg{}", seg.len),
            None => "None16".into(),
        }),
        _ => None,
    }).collect();
    println!("damage sites (collapse ON): raw={raw} :: {}", dmg.join(" "));

    set_ko_split_disabled(false);
    let on = dist(&b, &p1, &p2);
    set_ko_split_disabled(true);
    let full = dist(&b, &p1, &p2);
    set_ko_split_disabled(false);
    let mut keys: std::collections::HashSet<u64> = on.keys().copied().collect();
    keys.extend(full.keys().copied());
    let mut l1 = 0.0;
    for k in &keys { l1 += (on.get(k).copied().unwrap_or(0.0) - full.get(k).copied().unwrap_or(0.0)).abs(); }
    println!("frontier: on_out={} full_out={} L1={:.4e}{}", on.len(), full.len(), l1,
        if l1 > 1e-9 { "  <<< DIVERGENCE (guard needed)" } else { "  (sound)" });
}
