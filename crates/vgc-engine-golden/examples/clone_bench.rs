//! Micro-benchmark: how expensive is `Battle::clone()` vs `step()`?
//!
//! The chance-frontier migration plan assumes clones dominate native-branching
//! cost ("~10 kB state, 16-way damage branch = 160 kB per turn just in clones").
//! That assumed Battle was ~10 kB. Actual measured size (2026-06-26): Battle is
//! 288 bytes on the stack with two Side allocations holding 6 Pokemon × 192
//! bytes = ~2.3 kB heap. Need to confirm the wall-clock ratio before committing
//! to a multi-week CoW refactor.

use std::time::Instant;
use vgc_engine_core::{
    battle::{Battle, BattleConfig, StepResult},
    Choice, Format, Rng, SideRef, TeamBuilder,
};

const TEAM_A: &str = "Gholdengo @ Choice Specs
Ability: Good as Gold
Tera Type: Steel
EVs: 4 HP / 252 SpA / 252 Spe
Modest Nature
- Make It Rain
- Shadow Ball
- Power Gem
- Trick

Iron Hands @ Assault Vest
Ability: Quark Drive
Tera Type: Grass
EVs: 252 HP / 252 Atk / 4 SpD
Adamant Nature
- Drain Punch
- Wild Charge
- Fake Out
- Heavy Slam

Flutter Mane @ Booster Energy
Ability: Protosynthesis
Tera Type: Fairy
EVs: 4 HP / 252 SpA / 252 Spe
Timid Nature
- Moonblast
- Shadow Ball
- Dazzling Gleam
- Protect

Amoonguss @ Sitrus Berry
Ability: Regenerator
Tera Type: Water
EVs: 252 HP / 252 Def / 4 SpD
Bold Nature
- Spore
- Pollen Puff
- Rage Powder
- Protect

Garchomp @ Life Orb
Ability: Rough Skin
Tera Type: Steel
EVs: 4 HP / 252 Atk / 252 Spe
Jolly Nature
- Earthquake
- Dragon Claw
- Stomping Tantrum
- Protect

Rotom-Wash @ Safety Goggles
Ability: Levitate
Tera Type: Water
EVs: 252 HP / 4 Def / 252 SpD
Calm Nature
- Hydro Pump
- Thunderbolt
- Will-O-Wisp
- Protect
";

const TEAM_B: &str = TEAM_A;

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

fn main() {
    let team_a = TeamBuilder::from_showdown_text(TEAM_A).expect("team A loads");
    let team_b = TeamBuilder::from_showdown_text(TEAM_B).expect("team B loads");
    let format = Format::Doubles;
    let active = format.active_count();

    // Build a couple of fresh battles for cloning at different states (turn 1
    // vs deep into a battle — clones at turn N walk slightly different paths
    // because of volatiles, last_damage trackers, etc.)
    let cfg = BattleConfig { format, seed: 12345 };
    let battle_t0 = Battle::new(cfg.clone(), team_a.clone(), team_b.clone());

    // Advance one battle ~10 turns so we measure a real mid-game state.
    let mut battle_mid = battle_t0.clone();
    let mut picker = Rng::new(99);
    let mut p1 = Vec::with_capacity(active);
    let mut p2 = Vec::with_capacity(active);
    let mut lc = Vec::with_capacity(16);
    for _ in 0..10 {
        pick_side(&battle_mid, SideRef::P1, active, &mut picker, &mut p1, &mut lc);
        pick_side(&battle_mid, SideRef::P2, active, &mut picker, &mut p2, &mut lc);
        if matches!(battle_mid.step(&p1, &p2), StepResult::Ended { .. }) {
            break;
        }
    }

    // ---- Clone benchmark (turn-0 and mid-game) -----------------------------
    for (label, b) in [("turn0", &battle_t0), ("mid", &battle_mid)] {
        let n = 200_000;
        let t = Instant::now();
        let mut sink = 0u64;
        for _ in 0..n {
            let c = b.clone();
            sink ^= c.turn() as u64;
        }
        let dt = t.elapsed();
        let ns = dt.as_nanos() / n as u128;
        println!("clone[{label:6}]: {ns:>6} ns/op  (sink={sink})");
    }

    // ---- Step benchmark (mid-game; reuse the same picker pattern) ----------
    {
        let n = 50_000;
        let mut b = battle_mid.clone();
        let mut picker = Rng::new(7);
        let t = Instant::now();
        let mut steps = 0u64;
        let mut sink = 0u64;
        for _ in 0..n {
            pick_side(&b, SideRef::P1, active, &mut picker, &mut p1, &mut lc);
            pick_side(&b, SideRef::P2, active, &mut picker, &mut p2, &mut lc);
            match b.step(&p1, &p2) {
                StepResult::Continue => steps += 1,
                StepResult::Ended { .. } => {
                    // Reset and continue; we want sustained step throughput
                    b = battle_t0.clone();
                    sink ^= 1;
                }
            }
            steps += 1;
        }
        let dt = t.elapsed();
        let ns = dt.as_nanos() / steps as u128;
        println!("step[mid]:       {ns:>6} ns/op  (steps={steps} sink={sink})");
    }
}
