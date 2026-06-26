//! Measure `step_chance` — the solver-side per-cell enumeration cost.

use std::time::Instant;
use vgc_engine_core::{
    battle::{Battle, BattleConfig, StepResult},
    Choice, Format, Rng, SideRef, TeamBuilder,
};

const TEAM: &str = "Gholdengo @ Choice Specs
Ability: Good as Gold
EVs: 4 HP / 252 SpA / 252 Spe
Modest Nature
- Make It Rain
- Shadow Ball
- Power Gem
- Trick

Iron Hands @ Assault Vest
Ability: Quark Drive
EVs: 252 HP / 252 Atk / 4 SpD
Adamant Nature
- Drain Punch
- Wild Charge
- Fake Out
- Heavy Slam

Flutter Mane @ Booster Energy
Ability: Protosynthesis
EVs: 4 HP / 252 SpA / 252 Spe
Timid Nature
- Moonblast
- Shadow Ball
- Dazzling Gleam
- Protect

Amoonguss @ Sitrus Berry
Ability: Regenerator
EVs: 252 HP / 252 Def / 4 SpD
Bold Nature
- Spore
- Pollen Puff
- Rage Powder
- Protect

Garchomp @ Life Orb
Ability: Rough Skin
EVs: 4 HP / 252 Atk / 252 Spe
Jolly Nature
- Earthquake
- Dragon Claw
- Stomping Tantrum
- Protect

Rotom-Wash @ Safety Goggles
Ability: Levitate
EVs: 252 HP / 4 Def / 252 SpD
Calm Nature
- Hydro Pump
- Thunderbolt
- Will-O-Wisp
- Protect
";

fn pick_side(
    battle: &Battle,
    side: SideRef,
    active: usize,
    picker: &mut Rng,
    buf: &mut Vec<Choice>,
    lc: &mut Vec<Choice>,
) {
    buf.clear();
    for slot in 0..active {
        battle.legal_choices_into(side, slot as u8, lc);
        let idx = picker.range(lc.len() as u32) as usize;
        buf.push(lc[idx]);
    }
}

fn bench_format(format: Format) {
    let team = TeamBuilder::from_showdown_text(TEAM).expect("team loads");
    let active = format.active_count();
    let cfg = BattleConfig { format, seed: 12345 };
    let mut battle = Battle::new(cfg, team.clone(), team.clone());

    // Advance ~5 turns to get a representative mid-game state
    let mut picker = Rng::new(42);
    let mut p1 = Vec::with_capacity(active);
    let mut p2 = Vec::with_capacity(active);
    let mut lc = Vec::with_capacity(16);
    for _ in 0..5 {
        pick_side(&battle, SideRef::P1, active, &mut picker, &mut p1, &mut lc);
        pick_side(&battle, SideRef::P2, active, &mut picker, &mut p2, &mut lc);
        if matches!(battle.step(&p1, &p2), StepResult::Ended { .. }) {
            break;
        }
    }

    // Pick one joint action (the first legal action for each slot)
    let mut p1 = Vec::with_capacity(active);
    let mut p2 = Vec::with_capacity(active);
    for slot in 0..active {
        battle.legal_choices_into(SideRef::P1, slot as u8, &mut lc);
        p1.push(lc[0]);
        battle.legal_choices_into(SideRef::P2, slot as u8, &mut lc);
        p2.push(lc[0]);
    }

    // Probe the frontier size + per-call time
    let frontier = battle.step_chance(&p1, &p2, 0);
    println!(
        "[{:7}] frontier outcomes={:>3}  raw_combos={:>5}  unmatched={}  lazy_iters={}",
        format!("{format:?}"),
        frontier.outcomes.len(),
        frontier.raw_combos,
        frontier.unmatched_total,
        frontier.lazy_iterations
    );

    let n = 200;
    let t = Instant::now();
    let mut sink = 0u64;
    for _ in 0..n {
        let f = battle.step_chance(&p1, &p2, 0);
        sink ^= f.outcomes.len() as u64;
    }
    let dt = t.elapsed();
    let us = dt.as_micros() as f64 / n as f64;
    let per_combo_ns = (dt.as_nanos() as f64) / (n as f64 * frontier.raw_combos.max(1) as f64);
    println!(
        "[{:7}] step_chance: {us:>8.1} \u{00b5}s/call  ({per_combo_ns:.0} ns per raw combo, sink={sink})",
        format!("{format:?}")
    );
}

fn main() {
    bench_format(Format::Singles);
    bench_format(Format::Doubles);
}
