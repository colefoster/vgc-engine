//! vgc-engine throughput benchmark.
//!
//! Runs N random battles to completion (random legal choices each turn, fixed
//! seed) and reports battles/sec, steps/sec, and per-step latency (ns). One
//! `step()` == one full turn (both sides choose), which is the unit compared
//! against pokemon-showdown's "turn" in `tools/perf/ps_bench.js`.
//!
//! A counting global allocator is installed so the bench can verify the claim
//! that `step()` is heap-free (AGENTS.md rule 4). It snapshots allocation
//! counts around each `step()` call and reports allocations attributable to the
//! hot loop separately from driver overhead. The driver enumerates actions via
//! `legal_choices_into` (a reused buffer), so it is heap-free too.
//!
//! Run (release is mandatory for meaningful numbers):
//!   cargo run --release -p vgc-engine-golden --example perf_bench -- --battles 500
//!
//! Args:
//!   --battles N     number of battles to run to completion (default 500)
//!   --seed S        base u64 seed (default 1)
//!   --format F      "doubles" (default) or "singles"
//!   --max-turns T   safety cap per battle (default 1000)
//!   --json          emit a machine-readable VGCBENCH_JSON line on stdout

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use vgc_engine_core::{
    battle::{Battle, BattleConfig, StepResult},
    Choice, Format, Rng, SideRef, TeamBuilder,
};

// --- counting allocator: lets us prove step() is heap-free ----------------

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

struct CountingAlloc;

// SAFETY: this is a thin pass-through to the System allocator; it only adds two
// relaxed atomic counters and forwards every call unchanged, preserving all of
// System's allocator invariants (alignment, size, pointer provenance).
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
        // SAFETY: forwarding the caller's valid Layout to System.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: forwarding the caller's valid (ptr, Layout) pair to System.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new as u64, Ordering::Relaxed);
        // SAFETY: forwarding the caller's valid (ptr, Layout, new_size) to System.
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static GA: CountingAlloc = CountingAlloc;

#[inline]
fn alloc_count() -> u64 {
    ALLOCS.load(Ordering::Relaxed)
}

// --- two full 6-mon gen9 VGC (doubles) teams ------------------------------
// Sets lifted verbatim from the committed goldens so they are guaranteed to
// load against the generated dex data.

const TEAM_A: &str = "\
Koraidon @ Clear Amulet
Ability: Orichalcum Pulse
Level: 50
Tera Type: Fire
EVs: 252 Atk / 4 SpD / 252 Spe
Jolly Nature
- Flare Blitz
- Collision Course
- U-turn
- Protect

Flutter Mane @ Booster Energy
Ability: Protosynthesis
Level: 50
Tera Type: Fairy
EVs: 4 HP / 252 SpA / 252 Spe
Timid Nature
IVs: 0 Atk
- Moonblast
- Shadow Ball
- Dazzling Gleam
- Protect

Iron Hands @ Assault Vest
Ability: Quark Drive
Level: 50
Tera Type: Grass
EVs: 220 HP / 252 Atk / 4 Def / 28 SpD / 4 Spe
Adamant Nature
- Drain Punch
- Wild Charge
- Heavy Slam
- Fake Out

Farigiraf @ Throat Spray
Ability: Armor Tail
Level: 50
Tera Type: Water
EVs: 244 HP / 4 Def / 252 SpA / 4 SpD / 4 Spe
Modest Nature
IVs: 0 Atk
- Hyper Voice
- Psychic
- Helping Hand
- Trick Room

Amoonguss @ Sitrus Berry
Ability: Regenerator
Level: 50
Tera Type: Water
EVs: 244 HP / 156 Def / 100 SpD
Bold Nature
IVs: 0 Atk
- Spore
- Rage Powder
- Pollen Puff
- Protect

Garchomp @ Yache Berry
Ability: Rough Skin
Level: 50
Tera Type: Steel
EVs: 252 Atk / 4 SpD / 252 Spe
Jolly Nature
- Earthquake
- Dragon Claw
- Stone Edge
- Protect
";

const TEAM_B: &str = "\
Calyrex-Shadow @ Life Orb
Ability: As One (Spectrier)
Level: 50
Tera Type: Psychic
EVs: 4 HP / 252 SpA / 252 Spe
Timid Nature
IVs: 0 Atk
- Astral Barrage
- Psychic
- Nasty Plot
- Protect

Incineroar @ Safety Goggles
Ability: Intimidate
Level: 50
Tera Type: Ghost
EVs: 244 HP / 4 Atk / 4 Def / 4 SpD / 252 Spe
Jolly Nature
- Fake Out
- Flare Blitz
- Knock Off
- Parting Shot

Urshifu @ Focus Sash
Ability: Unseen Fist
Level: 50
Tera Type: Dark
EVs: 4 HP / 252 Atk / 252 Spe
Jolly Nature
- Wicked Blow
- Close Combat
- Sucker Punch
- Protect

Tornadus @ Covert Cloak
Ability: Prankster
Level: 50
Tera Type: Flying
EVs: 252 HP / 4 SpA / 252 Spe
Timid Nature
IVs: 0 Atk
- Tailwind
- Bleakwind Storm
- Protect
- Taunt

Tyranitar @ Leftovers
Ability: Sand Stream
Level: 50
Tera Type: Flying
EVs: 252 HP / 4 Atk / 252 SpD
Careful Nature
- Crunch
- Substitute
- Protect
- Rock Slide

Dragapult @ Choice Specs
Ability: Clear Body
Level: 50
Tera Type: Dragon
EVs: 4 HP / 252 SpA / 252 Spe
Timid Nature
IVs: 0 Atk
- Shadow Ball
- Draco Meteor
- U-turn
- Flamethrower
";

struct Args {
    battles: u32,
    seed: u64,
    format: Format,
    max_turns: u32,
    json: bool,
}

fn parse_args() -> Args {
    let mut a = Args { battles: 500, seed: 1, format: Format::Doubles, max_turns: 1000, json: false };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--battles" => a.battles = it.next().unwrap().parse().unwrap(),
            "--seed" => a.seed = it.next().unwrap().parse().unwrap(),
            "--format" => {
                a.format = match it.next().unwrap().as_str() {
                    "singles" => Format::Singles,
                    _ => Format::Doubles,
                }
            }
            "--max-turns" => a.max_turns = it.next().unwrap().parse().unwrap(),
            "--json" => a.json = true,
            other => eprintln!("ignoring unknown arg: {other}"),
        }
    }
    a
}

/// Build a full set of random legal choices (one per active slot) for `side`,
/// using `picker` for selection. Returns the choices plus whether all slots
/// were forced to Pass (a signal the side can't act).
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
        // Allocation-free: fills the reused `lc` buffer instead of returning a
        // fresh Vec per slot (legal_choices_into clears + writes in place).
        battle.legal_choices_into(side, slot as u8, lc);
        let idx = picker.range(lc.len() as u32) as usize;
        buf.push(lc[idx]);
    }
}

fn main() {
    let args = parse_args();

    let team_a = TeamBuilder::from_showdown_text(TEAM_A).expect("team A loads");
    let team_b = TeamBuilder::from_showdown_text(TEAM_B).expect("team B loads");
    let active = args.format.active_count();

    // Reusable choice buffers so the driver itself doesn't churn the heap each
    // turn. `lc_buf` is the per-slot legal-choice scratch filled by
    // legal_choices_into — reusing it makes action enumeration heap-free too.
    let mut p1_buf: Vec<Choice> = Vec::with_capacity(active);
    let mut p2_buf: Vec<Choice> = Vec::with_capacity(active);
    let mut lc_buf: Vec<Choice> = Vec::with_capacity(16);

    // --- warmup (let the CPU/branch predictors settle; not timed) ---------
    {
        let mut warm = Battle::new(
            BattleConfig { format: args.format, seed: args.seed },
            team_a.clone(),
            team_b.clone(),
        );
        let mut picker = Rng::new(args.seed ^ 0xDEAD_BEEF);
        for _ in 0..50 {
            pick_side(&warm, SideRef::P1, active, &mut picker, &mut p1_buf, &mut lc_buf);
            pick_side(&warm, SideRef::P2, active, &mut picker, &mut p2_buf, &mut lc_buf);
            if matches!(warm.step(&p1_buf, &p2_buf), StepResult::Ended { .. }) {
                break;
            }
        }
    }

    // --- timed run --------------------------------------------------------
    let mut total_steps: u64 = 0;
    let mut completed: u64 = 0;
    let mut capped: u64 = 0;
    let mut step_allocs: u64 = 0; // allocations charged to step() itself

    let t0 = Instant::now();
    for b in 0..args.battles {
        let seed = args.seed.wrapping_add((b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut battle = Battle::new(
            BattleConfig { format: args.format, seed },
            team_a.clone(),
            team_b.clone(),
        );
        let mut picker = Rng::new(seed ^ 0xA5A5_A5A5_5A5A_5A5A);

        let mut turn = 0u32;
        loop {
            if turn >= args.max_turns {
                capped += 1;
                break;
            }
            pick_side(&battle, SideRef::P1, active, &mut picker, &mut p1_buf, &mut lc_buf);
            pick_side(&battle, SideRef::P2, active, &mut picker, &mut p2_buf, &mut lc_buf);

            // Isolate allocations attributable to the hot loop. The action
            // enumeration above is now heap-free (legal_choices_into into a
            // reused buffer); anything between these two snapshots is step()'s
            // own heap traffic.
            let a_before = alloc_count();
            let res = battle.step(&p1_buf, &p2_buf);
            step_allocs += alloc_count() - a_before;

            total_steps += 1;
            turn += 1;
            if matches!(res, StepResult::Ended { .. }) {
                completed += 1;
                break;
            }
        }
    }
    let elapsed = t0.elapsed();

    let secs = elapsed.as_secs_f64();
    let battles_per_sec = args.battles as f64 / secs;
    let steps_per_sec = total_steps as f64 / secs;
    let ns_per_step = elapsed.as_nanos() as f64 / total_steps as f64;
    let avg_turns = total_steps as f64 / args.battles as f64;

    eprintln!(
        "vgc-engine ({}): {} battles, {} steps in {:.3}s",
        match args.format { Format::Doubles => "doubles", Format::Singles => "singles" },
        args.battles, total_steps, secs
    );
    eprintln!(
        "  {:.1} battles/s | {:.0} steps/s | {:.0} ns/step | avg {:.1} turns/battle",
        battles_per_sec, steps_per_sec, ns_per_step, avg_turns
    );
    let allocs_per_step = step_allocs as f64 / total_steps as f64;
    eprintln!(
        "  completed: {completed}, hit max-turns cap: {capped} | step() allocations: {step_allocs} \
         (={allocs_per_step:.3}/step)"
    );
    if allocs_per_step > 0.01 {
        eprintln!(
            "  NOTE: step() is NOT heap-free — ~{allocs_per_step:.2} alloc/step. Source: \
             order::action_order builds a Vec<ScheduledAction> per turn (order.rs). \
             AGENTS.md rule 4 (\"no allocations in the hot loop\") is not currently met."
        );
    }

    if args.json {
        println!(
            "VGCBENCH_JSON {{\"engine\":\"vgc-engine\",\"format\":\"{}\",\"battles\":{},\
\"total_steps\":{},\"avg_turns_per_battle\":{:.4},\"elapsed_s\":{:.6},\
\"battles_per_sec\":{:.4},\"steps_per_sec\":{:.4},\"ns_per_step\":{:.4},\
\"step_allocs_total\":{},\"completed\":{},\"capped\":{}}}",
            match args.format { Format::Doubles => "doubles", Format::Singles => "singles" },
            args.battles, total_steps, avg_turns, secs,
            battles_per_sec, steps_per_sec, ns_per_step, step_allocs, completed, capped
        );
    }
}
