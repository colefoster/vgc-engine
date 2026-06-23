//! vgc-engine comprehensive performance matrix — gen9 **doubles** focus.
//!
//! Broadens the single-scenario `perf_bench` into a benchmark *matrix*:
//!
//!   * **Scenario axis** — "light" (plain attackers, few ability/item procs)
//!     vs "heavy" (weather/terrain setters, Intimidate, redirection,
//!     Protosynthesis/Quark Drive, Trick Room — the expensive code paths) vs
//!     "random" (fuzz-generated legal Champions teams, the varied distribution
//!     ML rollouts actually see). Plus one singles baseline row.
//!   * **Single-thread metrics** — battles/sec, steps/sec, ns/step **p50 + p99**
//!     (latency distribution, not just the mean), `Battle::new()` construction
//!     cost, `Battle::clone()` cost, per-battle heap footprint, and the
//!     alloc/step heap-free probe (AGENTS.md rule 4).
//!   * **Parallel / batched throughput** — the headline for mimikyu's ML
//!     rollouts. Independent battles are shared-nothing (`Battle: Clone + Send`),
//!     so we scale across physical cores with `std::thread::scope` (no rayon
//!     dep) and report aggregate battles/sec + steps/sec and the **scaling
//!     efficiency** at 1, 2, 4, 8, … N cores, plus turns/sec/core vs DESIGN.md's
//!     ≥1M turns/sec/core aspiration.
//!
//! One `step()` == one full turn (both sides choose), the unit compared against
//! pokemon-showdown's "turn" in `tools/perf/ps_bench.js`. The companion
//! orchestrator `tools/perf/perf_matrix.sh` runs the PS side (single-process and
//! N-process) for the head-to-head.
//!
//! Run (release is MANDATORY for meaningful numbers):
//!   cargo run --release -p vgc-engine-golden --example perf_matrix
//!
//! Args:
//!   --st-battles N    battles per single-thread scenario row (default 400)
//!   --par-battles N   total battles for the parallel scaling sweep (default 6000)
//!   --par-scenario S  scenario for the parallel sweep: light|heavy|random (default heavy)
//!   --max-turns T     per-battle safety cap (default 1000)
//!   --seed S          base u64 seed (default 1)
//!   --json-out PATH   JSON artifact path (default target/perf-matrix.json)
//!   --quick           tiny counts for a smoke run

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use vgc_engine_core::{
    battle::{Battle, BattleConfig, StepResult},
    Choice, Format, Pokemon, Rng, SideRef, TeamBuilder,
};
use vgc_engine_golden::fuzz::{pick_side_choices, random_team, LearnsetCache};

// --- counting allocator (gated, so parallel runs aren't poisoned) ----------
//
// A global allocator can't be swapped per-section, so instead the counters are
// behind a flag that is OFF by default. While off, alloc() is a pure
// pass-through plus one relaxed *load* (no cross-thread contention — loads don't
// invalidate cache lines the way fetch_add does). We flip it on only around the
// single-threaded alloc/memory probes, so the parallel scaling numbers reflect
// the real allocator, not atomic contention on our counters.

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

struct CountingAlloc;

// SAFETY: thin pass-through to System; only adds relaxed counters when enabled.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new as u64, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static GA: CountingAlloc = CountingAlloc;

fn counting_on() {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
}
fn counting_off() -> (u64, u64) {
    COUNTING.store(false, Ordering::Relaxed);
    (ALLOCS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed))
}

// --- splitmix helpers (match fuzz.rs so random teams reproduce) ------------

const SEED_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix(seed: u64, salt: u64) -> u64 {
    let mut z = seed ^ salt.wrapping_mul(SEED_GAMMA);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// --- curated doubles teams -------------------------------------------------
//
// HEAVY: proc-dense — Intimidate, Sand Stream weather, Orichalcum Pulse sun,
// Protosynthesis/Quark Drive boosts, Rage Powder redirection, Trick Room,
// Booster Energy, contact-punish abilities. Exercises the expensive hooks.
// (Lifted verbatim from perf_bench.rs / the committed goldens.)

const HEAVY_A: &str = "\
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

const HEAVY_B: &str = "\
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

// LIGHT: plain attackers, damaging moves + Protect, abilities that mostly lie
// dormant in a no-weather/no-Intimidate game. Minimal per-event hook traffic.

const LIGHT_A: &str = "\
Garchomp @ Life Orb
Ability: Sand Veil
Level: 50
EVs: 252 Atk / 4 SpD / 252 Spe
Jolly Nature
- Earthquake
- Dragon Claw
- Stone Edge
- Protect

Dragapult @ Choice Specs
Ability: Clear Body
Level: 50
EVs: 4 HP / 252 SpA / 252 Spe
Timid Nature
IVs: 0 Atk
- Shadow Ball
- Draco Meteor
- Flamethrower
- Thunderbolt

Kommo-o @ Leftovers
Ability: Soundproof
Level: 50
EVs: 4 HP / 252 Atk / 252 Spe
Jolly Nature
- Close Combat
- Earthquake
- Iron Head
- Protect

Hydreigon @ Life Orb
Ability: Levitate
Level: 50
EVs: 4 HP / 252 SpA / 252 Spe
Modest Nature
IVs: 0 Atk
- Dark Pulse
- Draco Meteor
- Flamethrower
- Protect

Goodra @ Assault Vest
Ability: Sap Sipper
Level: 50
EVs: 252 HP / 252 SpA / 4 SpD
Modest Nature
IVs: 0 Atk
- Draco Meteor
- Fire Blast
- Thunderbolt
- Sludge Bomb

Haxorus @ Life Orb
Ability: Mold Breaker
Level: 50
EVs: 252 Atk / 4 SpD / 252 Spe
Adamant Nature
- Outrage
- Earthquake
- Iron Head
- Close Combat
";

const LIGHT_B: &str = "\
Gholdengo @ Choice Specs
Ability: Good as Gold
Level: 50
EVs: 4 HP / 252 SpA / 252 Spe
Timid Nature
IVs: 0 Atk
- Make It Rain
- Shadow Ball
- Thunderbolt
- Power Gem

Baxcalibur @ Loaded Dice
Ability: Thermal Exchange
Level: 50
EVs: 252 Atk / 4 SpD / 252 Spe
Jolly Nature
- Glaive Rush
- Icicle Spear
- Earthquake
- Protect

Volcarona @ Leftovers
Ability: Flame Body
Level: 50
EVs: 252 SpA / 4 SpD / 252 Spe
Timid Nature
IVs: 0 Atk
- Fiery Dance
- Bug Buzz
- Giga Drain
- Protect

Kingambit @ Black Glasses
Ability: Defiant
Level: 50
EVs: 252 Atk / 4 SpD / 252 Spe
Adamant Nature
- Kowtow Cleave
- Iron Head
- Sucker Punch
- Protect

Hatterene @ Life Orb
Ability: Magic Bounce
Level: 50
EVs: 252 HP / 252 SpA / 4 SpD
Quiet Nature
IVs: 0 Atk
- Psychic
- Dazzling Gleam
- Mystical Fire
- Protect

Garganacl @ Leftovers
Ability: Purifying Salt
Level: 50
EVs: 252 HP / 4 Atk / 252 SpD
Careful Nature
- Salt Cure
- Rock Slide
- Earthquake
- Protect
";

#[derive(Clone, Copy, PartialEq)]
enum Scenario {
    Light,
    Heavy,
    Random,
}
impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::Light => "light",
            Scenario::Heavy => "heavy",
            Scenario::Random => "random",
        }
    }
    fn parse(s: &str) -> Scenario {
        match s {
            "light" => Scenario::Light,
            "random" => Scenario::Random,
            _ => Scenario::Heavy,
        }
    }
}

/// Curated teams pre-parsed once (cheap to clone per battle).
struct Curated {
    light_a: Vec<Pokemon>,
    light_b: Vec<Pokemon>,
    heavy_a: Vec<Pokemon>,
    heavy_b: Vec<Pokemon>,
}
impl Curated {
    fn load() -> Curated {
        Curated {
            light_a: TeamBuilder::from_showdown_text(LIGHT_A).expect("light A loads"),
            light_b: TeamBuilder::from_showdown_text(LIGHT_B).expect("light B loads"),
            heavy_a: TeamBuilder::from_showdown_text(HEAVY_A).expect("heavy A loads"),
            heavy_b: TeamBuilder::from_showdown_text(HEAVY_B).expect("heavy B loads"),
        }
    }
}

/// Teams for battle `idx` under `scenario`. Curated scenarios clone the
/// pre-parsed teams; "random" deterministically fuzz-generates a fresh legal
/// Champions matchup keyed on `idx` (same scheme as fuzz.rs so it reproduces).
fn teams_for(
    scenario: Scenario,
    cur: &Curated,
    base_seed: u64,
    idx: u64,
    cache: &mut LearnsetCache,
) -> (Vec<Pokemon>, Vec<Pokemon>, u64) {
    let seed = base_seed.wrapping_add(idx.wrapping_mul(SEED_GAMMA));
    match scenario {
        Scenario::Light => (cur.light_a.clone(), cur.light_b.clone(), seed),
        Scenario::Heavy => (cur.heavy_a.clone(), cur.heavy_b.clone(), seed),
        Scenario::Random => {
            let (_, a) = random_team(mix(seed, 0x11), true, cache);
            let (_, b) = random_team(mix(seed, 0x22), true, cache);
            (a, b, seed)
        }
    }
}

/// Drive one battle to completion under uniformly-random legal play. Returns
/// the step count. If `lat` is `Some`, records per-step latency (ns) into it.
fn run_battle(
    mut battle: Battle,
    seed: u64,
    active: usize,
    max_turns: u32,
    p1: &mut Vec<Choice>,
    p2: &mut Vec<Choice>,
    mut lat: Option<&mut Vec<u32>>,
) -> u64 {
    let mut picker = Rng::new(seed ^ 0xA5A5_A5A5_5A5A_5A5A);
    let mut steps = 0u64;
    let mut turn = 0u32;
    loop {
        if turn >= max_turns {
            break;
        }
        pick_side_choices(&battle, SideRef::P1, active, &mut picker, p1);
        pick_side_choices(&battle, SideRef::P2, active, &mut picker, p2);
        let res = if let Some(buf) = lat.as_deref_mut() {
            let t = Instant::now();
            let r = battle.step(p1, p2);
            buf.push(t.elapsed().as_nanos() as u32);
            r
        } else {
            battle.step(p1, p2)
        };
        steps += 1;
        turn += 1;
        if matches!(res, StepResult::Ended { .. }) {
            break;
        }
    }
    steps
}

struct StRow {
    scenario: String,
    format: String,
    battles: u32,
    total_steps: u64,
    avg_turns: f64,
    battles_per_sec: f64,
    steps_per_sec: f64,
    ns_mean: f64,
    ns_p50: f64,
    ns_p99: f64,
    new_ns: f64,
    clone_ns: f64,
    per_battle_bytes: u64,
    per_battle_allocs: u64,
    alloc_per_step: f64,
}

fn percentile(sorted: &[u32], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)] as f64
}

fn single_thread_row(
    scenario: Scenario,
    format: Format,
    cur: &Curated,
    base_seed: u64,
    battles: u32,
    max_turns: u32,
) -> StRow {
    let active = format.active_count();
    let mut cache = LearnsetCache::new();
    let mut p1: Vec<Choice> = Vec::with_capacity(active);
    let mut p2: Vec<Choice> = Vec::with_capacity(active);

    // Warmup (settle branch predictors / warm the learnset cache; untimed).
    let warm_base = base_seed ^ 0x00DE_C0DE_5EED_0000_u64;
    for b in 0..(battles.min(30)) {
        let (a, bb, seed) = teams_for(scenario, cur, warm_base, b as u64, &mut cache);
        let battle = Battle::new(BattleConfig { format, seed }, a, bb);
        run_battle(battle, seed, active, max_turns, &mut p1, &mut p2, None);
    }

    // --- throughput pass (untimed per-step → clean mean ns/step) ----------
    let mut total_steps = 0u64;
    let t0 = Instant::now();
    for b in 0..battles {
        let (a, bb, seed) = teams_for(scenario, cur, base_seed, b as u64, &mut cache);
        let battle = Battle::new(BattleConfig { format, seed }, a, bb);
        total_steps += run_battle(battle, seed, active, max_turns, &mut p1, &mut p2, None);
    }
    let secs = t0.elapsed().as_secs_f64();

    // --- latency pass (per-step timed → p50/p99; carries timer overhead) ---
    let mut lat: Vec<u32> = Vec::with_capacity(total_steps as usize + 64);
    for b in 0..battles {
        let (a, bb, seed) = teams_for(scenario, cur, base_seed, b as u64, &mut cache);
        let battle = Battle::new(BattleConfig { format, seed }, a, bb);
        run_battle(battle, seed, active, max_turns, &mut p1, &mut p2, Some(&mut lat));
    }
    lat.sort_unstable();
    let ns_p50 = percentile(&lat, 50.0);
    let ns_p99 = percentile(&lat, 99.0);

    // --- Battle::new() construction cost ----------------------------------
    let (ca, cb, cseed) = teams_for(scenario, cur, base_seed, 0, &mut cache);
    let new_iters = 2000u32;
    let tnew = Instant::now();
    let mut sink = 0u64;
    for _ in 0..new_iters {
        let bt = Battle::new(BattleConfig { format, seed: cseed }, ca.clone(), cb.clone());
        sink = sink.wrapping_add(bt.p1.team.len() as u64);
    }
    std::hint::black_box(sink);
    let new_ns = tnew.elapsed().as_nanos() as f64 / new_iters as f64;

    // --- Battle::clone() cost (mid-battle, a few turns in) ----------------
    let mut warm = Battle::new(BattleConfig { format, seed: cseed }, ca.clone(), cb.clone());
    {
        let mut picker = Rng::new(cseed ^ 1);
        for _ in 0..3 {
            pick_side_choices(&warm, SideRef::P1, active, &mut picker, &mut p1);
            pick_side_choices(&warm, SideRef::P2, active, &mut picker, &mut p2);
            if matches!(warm.step(&p1, &p2), StepResult::Ended { .. }) {
                break;
            }
        }
    }
    let clone_iters = 5000u32;
    let tclone = Instant::now();
    for _ in 0..clone_iters {
        let c = warm.clone();
        std::hint::black_box(&c);
    }
    let clone_ns = tclone.elapsed().as_nanos() as f64 / clone_iters as f64;

    // --- per-battle heap footprint (teams + Battle::new), counting ON -----
    counting_on();
    {
        let (a, bb, seed) = teams_for(scenario, cur, base_seed, 1, &mut cache);
        let bt = Battle::new(BattleConfig { format, seed }, a, bb);
        std::hint::black_box(&bt);
    }
    let (per_battle_allocs, per_battle_bytes) = counting_off();

    // --- alloc/step probe over a fresh battle, counting ON ----------------
    counting_on();
    {
        let (a, bb, seed) = teams_for(scenario, cur, base_seed, 2, &mut cache);
        let mut bt = Battle::new(BattleConfig { format, seed }, a, bb);
        let mut picker = Rng::new(seed ^ 7);
        // Pre-fill choice buffers OUTSIDE the probe window each turn isn't
        // possible (choices change), so we subtract legal_choices' own allocs
        // by measuring the delta strictly around step().
        let before_after = {
            let mut step_allocs = 0u64;
            let mut steps = 0u64;
            let mut turn = 0u32;
            while turn < max_turns {
                pick_side_choices(&bt, SideRef::P1, active, &mut picker, &mut p1);
                pick_side_choices(&bt, SideRef::P2, active, &mut picker, &mut p2);
                let a0 = ALLOCS.load(Ordering::Relaxed);
                let r = bt.step(&p1, &p2);
                step_allocs += ALLOCS.load(Ordering::Relaxed) - a0;
                steps += 1;
                turn += 1;
                if matches!(r, StepResult::Ended { .. }) {
                    break;
                }
            }
            (step_allocs, steps)
        };
        std::hint::black_box(before_after);
        ALLOC_PROBE.store(before_after.0, Ordering::Relaxed);
        STEP_PROBE.store(before_after.1, Ordering::Relaxed);
    }
    let _ = counting_off();
    let sa = ALLOC_PROBE.load(Ordering::Relaxed);
    let ss = STEP_PROBE.load(Ordering::Relaxed).max(1);
    let alloc_per_step = sa as f64 / ss as f64;

    StRow {
        scenario: scenario.name().to_string(),
        format: match format {
            Format::Doubles => "doubles",
            Format::Singles => "singles",
        }
        .to_string(),
        battles,
        total_steps,
        avg_turns: total_steps as f64 / battles as f64,
        battles_per_sec: battles as f64 / secs,
        steps_per_sec: total_steps as f64 / secs,
        ns_mean: (secs * 1e9) / total_steps as f64,
        ns_p50,
        ns_p99,
        new_ns,
        clone_ns,
        per_battle_bytes,
        per_battle_allocs,
        alloc_per_step,
    }
}

static ALLOC_PROBE: AtomicU64 = AtomicU64::new(0);
static STEP_PROBE: AtomicU64 = AtomicU64::new(0);

struct ParPoint {
    threads: usize,
    battles: u32,
    total_steps: u64,
    elapsed_s: f64,
    battles_per_sec: f64,
    steps_per_sec: f64,
    efficiency: f64,
}

fn parallel_sweep(
    scenario: Scenario,
    format: Format,
    cur: &Curated,
    base_seed: u64,
    total_battles: u32,
    max_turns: u32,
    levels: &[usize],
) -> Vec<ParPoint> {
    let active = format.active_count();
    let mut points: Vec<ParPoint> = Vec::new();
    let mut baseline_sps = 0.0f64;

    for (li, &threads) in levels.iter().enumerate() {
        let t0 = Instant::now();
        // Partition battle indices [0, total_battles) across `threads`.
        let chunk = total_battles.div_ceil(threads as u32);
        let total_steps: u64 = std::thread::scope(|s| {
            let mut handles = Vec::with_capacity(threads);
            for t in 0..threads as u32 {
                handles.push(s.spawn(move || {
                    let start = t * chunk;
                    let end = ((t + 1) * chunk).min(total_battles);
                    let mut cache = LearnsetCache::new();
                    let mut p1: Vec<Choice> = Vec::with_capacity(active);
                    let mut p2: Vec<Choice> = Vec::with_capacity(active);
                    let mut steps = 0u64;
                    for b in start..end {
                        let (a, bb, seed) =
                            teams_for(scenario, cur, base_seed, b as u64, &mut cache);
                        let battle = Battle::new(BattleConfig { format, seed }, a, bb);
                        steps += run_battle(
                            battle, seed, active, max_turns, &mut p1, &mut p2, None,
                        );
                    }
                    steps
                }));
            }
            handles.into_iter().map(|h| h.join().unwrap()).sum()
        });
        let secs = t0.elapsed().as_secs_f64();
        let sps = total_steps as f64 / secs;
        if li == 0 {
            baseline_sps = sps;
        }
        points.push(ParPoint {
            threads,
            battles: total_battles,
            total_steps,
            elapsed_s: secs,
            battles_per_sec: total_battles as f64 / secs,
            steps_per_sec: sps,
            efficiency: if baseline_sps > 0.0 {
                (sps / baseline_sps) / threads as f64
            } else {
                0.0
            },
        });
    }
    points
}

struct Args {
    st_battles: u32,
    par_battles: u32,
    par_scenario: Scenario,
    max_turns: u32,
    seed: u64,
    json_out: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        st_battles: 400,
        par_battles: 6000,
        par_scenario: Scenario::Heavy,
        max_turns: 1000,
        seed: 1,
        json_out: "target/perf-matrix.json".to_string(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--st-battles" => a.st_battles = it.next().unwrap().parse().unwrap(),
            "--par-battles" => a.par_battles = it.next().unwrap().parse().unwrap(),
            "--par-scenario" => a.par_scenario = Scenario::parse(&it.next().unwrap()),
            "--max-turns" => a.max_turns = it.next().unwrap().parse().unwrap(),
            "--seed" => a.seed = it.next().unwrap().parse().unwrap(),
            "--json-out" => a.json_out = it.next().unwrap(),
            "--quick" => {
                a.st_battles = 40;
                a.par_battles = 400;
            }
            other => eprintln!("ignoring unknown arg: {other}"),
        }
    }
    a
}

fn main() {
    let args = parse_args();
    let cur = Curated::load();

    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let mut levels: Vec<usize> = [1usize, 2, 4, 8, cores]
        .into_iter()
        .filter(|&n| n <= cores)
        .collect();
    levels.sort_unstable();
    levels.dedup();

    eprintln!(
        "vgc-engine perf matrix — gen9 doubles | cores={cores} | st-battles={} par-battles={} par-scenario={}",
        args.st_battles,
        args.par_battles,
        args.par_scenario.name()
    );

    // --- single-thread rows -----------------------------------------------
    let mut rows: Vec<StRow> = Vec::new();
    for sc in [Scenario::Light, Scenario::Heavy, Scenario::Random] {
        eprintln!(">> single-thread: {} (doubles)…", sc.name());
        rows.push(single_thread_row(sc, Format::Doubles, &cur, args.seed, args.st_battles, args.max_turns));
    }
    // singles baseline (light)
    eprintln!(">> single-thread: light (singles baseline)…");
    rows.push(single_thread_row(Scenario::Light, Format::Singles, &cur, args.seed, args.st_battles, args.max_turns));

    // --- parallel sweep ----------------------------------------------------
    eprintln!(">> parallel scaling sweep on '{}' across {:?} threads…", args.par_scenario.name(), levels);
    let par = parallel_sweep(
        args.par_scenario,
        Format::Doubles,
        &cur,
        args.seed,
        args.par_battles,
        args.max_turns,
        &levels,
    );

    print_tables(&rows, &par, cores, args.par_scenario.name());
    write_json(&args.json_out, &rows, &par, cores, args.par_scenario.name());
    eprintln!("\nJSON artifact: {}", args.json_out);
}

fn print_tables(rows: &[StRow], par: &[ParPoint], cores: usize, par_scenario: &str) {
    println!("\n================ SINGLE-THREAD (per scenario) ================");
    println!(
        "{:<8}{:<9}{:>9}{:>11}{:>14}{:>11}{:>11}{:>11}{:>10}{:>10}{:>12}{:>11}",
        "scen", "format", "battles", "turns/b", "battles/s", "steps/s", "ns p50", "ns p99",
        "ns mean", "new ns", "clone ns", "alloc/stp"
    );
    println!("{}", "-".repeat(127));
    for r in rows {
        println!(
            "{:<8}{:<9}{:>9}{:>11.1}{:>14.1}{:>11.0}{:>11.0}{:>11.0}{:>10.0}{:>10.0}{:>12.0}{:>11.3}",
            r.scenario, r.format, r.battles, r.avg_turns, r.battles_per_sec, r.steps_per_sec,
            r.ns_p50, r.ns_p99, r.ns_mean, r.new_ns, r.clone_ns, r.alloc_per_step
        );
    }
    println!("\nper-battle heap footprint (teams + Battle::new):");
    for r in rows {
        println!(
            "  {:<8} {:<9} {:>7} allocs / {:>8} bytes",
            r.scenario, r.format, r.per_battle_allocs, r.per_battle_bytes
        );
    }
    println!("(ns p50/p99 carry per-step timer overhead; ns mean is from an untimed pass.)");

    println!("\n================ PARALLEL SCALING (scenario='{par_scenario}', doubles) ================");
    println!(
        "{:>8}{:>10}{:>11}{:>15}{:>15}{:>14}{:>13}",
        "threads", "battles", "elapsed s", "battles/s", "steps/s", "steps/s/core", "efficiency"
    );
    println!("{}", "-".repeat(86));
    for p in par {
        println!(
            "{:>8}{:>10}{:>11.3}{:>15.1}{:>15.0}{:>14.0}{:>12.0}%",
            p.threads, p.battles, p.elapsed_s, p.battles_per_sec, p.steps_per_sec,
            p.steps_per_sec / p.threads as f64, p.efficiency * 100.0
        );
    }
    if let Some(peak) = par.last() {
        let per_core = peak.steps_per_sec / peak.threads as f64;
        println!(
            "\nPEAK: {:.0} battles/s, {:.0} steps(turns)/s across {} threads",
            peak.battles_per_sec, peak.steps_per_sec, peak.threads
        );
        println!(
            "  {:.0} turns/sec/core  (DESIGN.md aspiration: >=1,000,000 turns/sec/core -> {:.1}% of target)",
            per_core,
            per_core / 1_000_000.0 * 100.0
        );
        println!("  scaling efficiency at {} cores: {:.0}%", peak.threads, peak.efficiency * 100.0);
    }
    let _ = cores;
}

fn write_json(path: &str, rows: &[StRow], par: &[ParPoint], cores: usize, par_scenario: &str) {
    use serde_json::json;
    let st: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({
                "scenario": r.scenario,
                "format": r.format,
                "battles": r.battles,
                "total_steps": r.total_steps,
                "avg_turns_per_battle": r.avg_turns,
                "battles_per_sec": r.battles_per_sec,
                "steps_per_sec": r.steps_per_sec,
                "ns_per_step_mean": r.ns_mean,
                "ns_per_step_p50": r.ns_p50,
                "ns_per_step_p99": r.ns_p99,
                "battle_new_ns": r.new_ns,
                "battle_clone_ns": r.clone_ns,
                "per_battle_heap_bytes": r.per_battle_bytes,
                "per_battle_heap_allocs": r.per_battle_allocs,
                "step_allocs_per_step": r.alloc_per_step,
            })
        })
        .collect();
    let pj: Vec<_> = par
        .iter()
        .map(|p| {
            json!({
                "threads": p.threads,
                "battles": p.battles,
                "total_steps": p.total_steps,
                "elapsed_s": p.elapsed_s,
                "battles_per_sec": p.battles_per_sec,
                "steps_per_sec": p.steps_per_sec,
                "steps_per_sec_per_core": p.steps_per_sec / p.threads as f64,
                "scaling_efficiency": p.efficiency,
            })
        })
        .collect();
    let peak = par.last();
    let doc = json!({
        "engine": "vgc-engine",
        "machine_cores": cores,
        "parallel_scenario": par_scenario,
        "single_thread": st,
        "parallel_scaling": pj,
        "peak": peak.map(|p| json!({
            "threads": p.threads,
            "battles_per_sec": p.battles_per_sec,
            "steps_per_sec": p.steps_per_sec,
            "turns_per_sec_per_core": p.steps_per_sec / p.threads as f64,
            "efficiency": p.efficiency,
        })),
    });
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, serde_json::to_string_pretty(&doc).unwrap())
        .unwrap_or_else(|e| eprintln!("failed to write {path}: {e}"));
}
