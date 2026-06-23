//! Randomized fuzz / property-test harness for the battle engine.
//!
//! Two pieces:
//!
//!   1. A **random legal-team generator** ([`random_team`]) — given a `u64`
//!      seed it deterministically produces a legal gen-9 doubles (or singles)
//!      team: random in-format species (from the Reg M-B Champions allow-list
//!      [`REG_M_B_LEGAL_SPECIES`], or the full dex), a random legal ability
//!      (from the species' `legal_abilities`), 1–4 learnset-legal moves (gated
//!      by [`data::species_can_learn`]), a random held item, random nature,
//!      legal Stat Points (≤66 SP total / ≤32 SP per stat as EVs) and IVs
//!      fixed at 31 (Champions standardizes IVs), level 50.
//!      The team loads into the engine and (in Champions mode) passes
//!      [`verify_team`] against [`REG_M_B`].
//!
//!   2. A **fuzz loop** ([`run_fuzz`]) that, for N seeds, builds two random
//!      teams + a [`Battle`], plays to completion choosing a uniformly random
//!      legal choice for every active slot each turn, and after every
//!      [`Battle::step`] asserts a battery of engine invariants (HP bounds,
//!      faint/HP consistency, non-empty legal-choice sets, termination before
//!      a turn cap, run-to-run determinism, serde round-trip identity, and
//!      clone independence). The harness running green over a large seed range
//!      is itself the primary "no panic / no soft-lock" check.
//!
//! Determinism: every random decision (team build + per-turn action picks +
//! the engine's own RNG) is derived from the battle seed, so a flagged seed
//! reproduces exactly — re-run the standalone `fuzz_battles` example with
//! `--seed S --battles 1` to minimize.

use std::collections::HashMap;

use vgc_engine_core::{
    battle::{Battle, BattleConfig, StepResult},
    build_member, verify_team, Choice, Format, Pokemon, Rng, SideRef, StatSpread, TeamMember,
    REG_M_B,
};
use vgc_engine_data as data;

/// The 25 nature slugs, in PS table order. Every entry resolves via
/// `nature_by_slug`, so a generated set never fails to build on nature.
pub const NATURES: [&str; 25] = [
    "hardy", "lonely", "brave", "adamant", "naughty", "bold", "docile", "relaxed", "impish", "lax",
    "timid", "hasty", "serious", "jolly", "naive", "modest", "mild", "quiet", "bashful", "rash",
    "calm", "gentle", "sassy", "careful", "quirky",
];

/// splitmix64 finalizer over `seed ^ (salt * GAMMA)` — gives well-separated
/// sub-streams from a single battle seed, so team builds and the action picker
/// don't correlate.
fn mix(seed: u64, salt: u64) -> u64 {
    let mut z = seed ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

// ---------------------------------------------------------------------------
// Learnset cache (data-derived → deterministic; just avoids re-scanning MOVES)
// ---------------------------------------------------------------------------

/// Per-species cache of learnset-legal move ids. Purely a function of the
/// static dex tables, so it never affects determinism — it only spares the
/// generator from re-scanning every move for every Pokémon on every seed.
#[derive(Default)]
pub struct LearnsetCache {
    by_species: HashMap<u16, Vec<u16>>,
}

impl LearnsetCache {
    pub fn new() -> Self {
        Self { by_species: HashMap::new() }
    }

    /// All move ids `species_id` can legally learn (transfer-legal, per
    /// [`data::species_can_learn`]), sorted ascending.
    pub fn learnable(&mut self, species_id: u16) -> &[u16] {
        self.by_species.entry(species_id).or_insert_with(|| {
            (0..data::MOVES.len() as u16)
                .filter(|&mid| data::species_can_learn(species_id, mid))
                .collect()
        })
    }
}

/// True iff `species_id` is the mega-evolved forme of some base species. Such
/// formes share their base's dex `num` and aren't built directly in real play
/// (you build the base + the stone), so the generator skips them as set
/// species — they'd otherwise collide with their base under Species Clause.
fn is_mega_forme(species_id: u16) -> bool {
    data::MEGA_STONES.iter().any(|m| m.mega_species_id == species_id)
}

/// Build the candidate species pool (table indices). In `champions_only` mode
/// this is exactly the Reg M-B allow-list (by base dex `num`); otherwise the
/// whole dex. Mega formes and species with no usable ability are excluded.
fn candidate_species(champions_only: bool) -> Vec<u16> {
    (0..data::SPECIES.len() as u16)
        .filter(|&i| {
            let sp = &data::SPECIES[i as usize];
            if champions_only
                && vgc_engine_core::format_rules::REG_M_B_LEGAL_SPECIES
                    .binary_search(&sp.num)
                    .is_err()
            {
                return false;
            }
            if sp.legal_abilities.iter().all(|&a| a == u16::MAX) {
                return false;
            }
            if is_mega_forme(i) {
                return false;
            }
            true
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Random team generator
// ---------------------------------------------------------------------------

fn random_evs(rng: &mut Rng) -> StatSpread {
    // Champions Stat Points: ≤66 SP total, ≤32 SP per stat. Allocate SP in a
    // random stat order, then convert to EVs (first point costs 4 EV, each
    // additional 8 → EV(S) = 8*S - 4 for S≥1; 32 SP ↔ 252 EV). This round-trips
    // exactly through the verifier's `ev_to_sp`, so generated teams stay legal
    // under the SP budget (`Rule::StatPoints`).
    let mut idx = [0usize, 1, 2, 3, 4, 5];
    for i in (1..6).rev() {
        let j = rng.range((i + 1) as u32) as usize;
        idx.swap(i, j);
    }
    let mut arr = [0u8; 6];
    let mut budget: u16 = 66; // Stat Points
    for &s in &idx {
        let cap = budget.min(32);
        let sp = if cap == 0 { 0 } else { rng.range((cap + 1) as u32) as u16 };
        budget -= sp;
        arr[s] = if sp == 0 { 0 } else { (8 * sp - 4) as u8 };
    }
    StatSpread { hp: arr[0], atk: arr[1], def: arr[2], spa: arr[3], spd: arr[4], spe: arr[5] }
}

fn random_ivs(_rng: &mut Rng) -> StatSpread {
    // Champions standardizes IVs at 31 (no adjustable IVs); the verifier's
    // `Rule::Iv` requires all 31, so generate them fixed.
    StatSpread { hp: 31, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 }
}

fn random_item_slug(rng: &mut Rng, used: &[u16]) -> Option<String> {
    let n = data::ITEMS.len();
    if n == 0 {
        return None;
    }
    // Up to a few tries to find an unused, non-empty item (Item Clause).
    for _ in 0..16 {
        let id = rng.range(n as u32) as u16;
        let def = &data::ITEMS[id as usize];
        if def.slug.is_empty() || used.contains(&id) {
            continue;
        }
        return Some(def.slug.to_string());
    }
    None
}

fn random_member(
    rng: &mut Rng,
    species_id: u16,
    learnable: &[u16],
    used_items: &[u16],
) -> TeamMember {
    let sp = &data::SPECIES[species_id as usize];

    // Ability: uniformly from the species' real (non-sentinel) abilities.
    let abilities: Vec<u16> =
        sp.legal_abilities.iter().copied().filter(|&a| a != u16::MAX).collect();
    let ability = abilities[rng.range(abilities.len() as u32) as usize];
    let ability_slug = data::ABILITIES[ability as usize].slug.to_string();

    // Moves: 1..=min(4, learnable) distinct, via a partial Fisher-Yates over a
    // copy of the learnable pool.
    let mut pool = learnable.to_vec();
    let max_k = pool.len().min(4) as u32;
    let k = (1 + rng.range(max_k)) as usize;
    for i in 0..k {
        let j = i + rng.range((pool.len() - i) as u32) as usize;
        pool.swap(i, j);
    }
    let moves: Vec<String> =
        pool[..k].iter().map(|&mid| data::MOVES[mid as usize].slug.to_string()).collect();

    let nature = NATURES[rng.range(NATURES.len() as u32) as usize].to_string();
    let item = random_item_slug(rng, used_items);

    TeamMember {
        species: sp.slug.to_string(),
        level: 50,
        ability: Some(ability_slug),
        item,
        nature,
        moves,
        ivs: random_ivs(rng),
        evs: random_evs(rng),
        teratype: None, // Tera is banned in Reg M-B; leaving it unset keeps the team legal.
        gender: None,   // let the battle constructor roll ratio'd genders.
    }
}

/// Generate a random legal team for `seed`, returning both the parsed
/// [`TeamMember`] specs (for verification) and the built `Vec<Pokemon>` ready
/// to drop into a [`Battle`]. Deterministic in `seed`.
pub fn random_team(
    seed: u64,
    champions_only: bool,
    cache: &mut LearnsetCache,
) -> (Vec<TeamMember>, Vec<Pokemon>) {
    let mut rng = Rng::new(seed);
    let pool = candidate_species(champions_only);
    let team_size = (4 + rng.range(3)) as usize; // 4..=6

    let mut used_nums: Vec<u16> = Vec::with_capacity(team_size);
    let mut used_items: Vec<u16> = Vec::with_capacity(team_size);
    let mut members: Vec<TeamMember> = Vec::with_capacity(team_size);

    let mut attempts = 0;
    while members.len() < team_size && attempts < team_size * 200 {
        attempts += 1;
        let species_id = pool[rng.range(pool.len() as u32) as usize];
        let sp = &data::SPECIES[species_id as usize];
        // Species Clause: unique by dex num.
        if used_nums.contains(&sp.num) {
            continue;
        }
        let learnable = cache.learnable(species_id).to_vec();
        if learnable.is_empty() {
            continue;
        }
        used_nums.push(sp.num);
        let m = random_member(&mut rng, species_id, &learnable, &used_items);
        if let Some(item) = m.item.as_deref() {
            if let Some(def) = data::item_by_slug(item) {
                if let Some(id) = data::ITEMS.iter().position(|x| x.slug == def.slug) {
                    used_items.push(id as u16);
                }
            }
        }
        members.push(m);
    }

    let pokemon: Vec<Pokemon> = members
        .iter()
        .map(|m| build_member(m).expect("generated member must build"))
        .collect();
    (members, pokemon)
}

// ---------------------------------------------------------------------------
// Action picking
// ---------------------------------------------------------------------------

/// Fill `buf` with one uniformly-random legal choice per active slot for
/// `side`, enforcing the one constraint `legal_choices` doesn't (it's computed
/// per-slot in isolation): two slots can't switch to the **same** bench
/// Pokémon in the same turn (PS rejects that at the UI). When a slot's only
/// remaining legal options are already-claimed switches, it passes.
pub fn pick_side_choices(
    battle: &Battle,
    side: SideRef,
    active: usize,
    picker: &mut Rng,
    buf: &mut Vec<Choice>,
) {
    buf.clear();
    let mut used_switch = [false; 6];
    for slot in 0..active {
        let lc = battle.legal_choices(side, slot as u8);
        // Count choices that don't switch into an already-claimed bench slot.
        let valid = |c: &Choice| match *c {
            Choice::Switch { team_index, .. } => !used_switch[team_index as usize],
            _ => true,
        };
        let valid_count = lc.iter().filter(|c| valid(c)).count();
        let chosen = if valid_count == 0 {
            Choice::Pass { actor_slot: slot as u8 }
        } else {
            let mut nth = picker.range(valid_count as u32);
            let mut pick = lc[0];
            for c in &lc {
                if valid(c) {
                    if nth == 0 {
                        pick = *c;
                        break;
                    }
                    nth -= 1;
                }
            }
            pick
        };
        if let Choice::Switch { team_index, .. } = chosen {
            used_switch[team_index as usize] = true;
        }
        buf.push(chosen);
    }
}

// ---------------------------------------------------------------------------
// Invariant checks
// ---------------------------------------------------------------------------

/// Assert HP bounds and faint/HP consistency over every Pokémon on both
/// teams. Returns a human-readable description on the first violation.
fn check_hp_invariants(battle: &Battle) -> Result<(), String> {
    for (side_name, side) in [("p1", &battle.p1), ("p2", &battle.p2)] {
        for (i, mon) in side.team.iter().enumerate() {
            let max = mon.stats.hp;
            if mon.current_hp > max {
                return Err(format!(
                    "{side_name}.team[{i}] hp {} exceeds max {}",
                    mon.current_hp, max
                ));
            }
            // fainted iff current_hp == 0.
            if mon.fainted != (mon.current_hp == 0) {
                return Err(format!(
                    "{side_name}.team[{i}] fainted={} but hp={} (expected faint iff hp==0)",
                    mon.fainted, mon.current_hp
                ));
            }
        }
    }
    Ok(())
}

/// While the battle is ongoing, every active slot must have ≥1 legal choice
/// (no soft-lock).
fn check_legal_choices_nonempty(battle: &Battle, active: usize) -> Result<(), String> {
    for side in [SideRef::P1, SideRef::P2] {
        for slot in 0..active {
            if battle.legal_choices(side, slot as u8).is_empty() {
                return Err(format!("empty legal_choices for {side:?} slot {slot}"));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Fuzz loop
// ---------------------------------------------------------------------------

/// Knobs for [`run_fuzz`].
#[derive(Debug, Clone)]
pub struct FuzzOptions {
    /// Number of battles (seeds) to run.
    pub battles: u32,
    /// Base seed; battle `b` uses `base_seed + b * GAMMA`.
    pub base_seed: u64,
    pub format: Format,
    /// Per-battle turn cap; a battle that hits it is flagged (possible
    /// non-termination / soft-lock).
    pub max_turns: u32,
    /// Draw species from the Reg M-B Champions allow-list (true) or the full
    /// dex (false). Team verification only runs in Champions mode.
    pub champions_only: bool,
    /// Every `check_every`-th battle additionally runs the expensive checks:
    /// serde round-trip + clone-independence mid-battle, and a full re-run for
    /// determinism. `0` disables the expensive checks.
    pub check_every: u32,
    /// Run [`verify_team`] against [`REG_M_B`] on each generated team
    /// (Champions mode only).
    pub verify_teams: bool,
}

impl Default for FuzzOptions {
    fn default() -> Self {
        Self {
            battles: 1000,
            base_seed: 1,
            format: Format::Doubles,
            max_turns: 1000,
            champions_only: true,
            check_every: 16,
            verify_teams: true,
        }
    }
}

/// Aggregate result of a fuzz run. `violations` empty == green.
#[derive(Debug, Default)]
pub struct FuzzReport {
    pub battles: u32,
    pub completed: u32,
    pub capped: u32,
    pub total_steps: u64,
    /// Of the `capped` battles, how many ended in a PP-exhaustion stall (every
    /// alive active mon out of usable moves with only `Pass` available). Since
    /// `legal_choices` enumerates Struggle in that situation (matching PS), this
    /// should be ~0; a non-zero count flags a regression in that path.
    pub pp_exhaustion_stalls: u32,
    /// Distinct invariant violations (seed-tagged). Capped to keep memory
    /// bounded; `violations_total` is the true count.
    pub violations: Vec<String>,
    pub violations_total: u32,
}

const SEED_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const VIOLATION_CAP: usize = 50;

struct Outcome {
    completed: bool,
    /// Hit the turn cap without ending.
    capped: bool,
    /// Capped *and* classified as a PP-exhaustion stall (no alive active mon on
    /// either side has a usable move left).
    pp_stall: bool,
    steps: u64,
    final_state: String,
}

/// True iff no alive active mon on either side has any `Move`/`Terastallize`/
/// `MegaEvolve` choice left — i.e. progress is impossible because every
/// fighter is reduced to `Pass`. With Struggle enumeration this should never
/// happen (Struggle is a `Choice::Move`); the classifier remains so a genuine
/// livelock or any regression in the Struggle path would still stand out.
fn is_pp_exhaustion_stall(battle: &Battle, active: usize) -> bool {
    for side in [SideRef::P1, SideRef::P2] {
        let s = match side {
            SideRef::P1 => &battle.p1,
            SideRef::P2 => &battle.p2,
        };
        for slot in 0..active {
            let Some(mon) = s.active_mon(slot) else { continue };
            if !mon.is_alive() {
                continue;
            }
            let has_move = battle.legal_choices(side, slot as u8).iter().any(|c| {
                matches!(
                    c,
                    Choice::Move { .. } | Choice::Terastallize { .. } | Choice::MegaEvolve { .. }
                )
            });
            if has_move {
                return false;
            }
        }
    }
    true
}

/// Run one battle for `seed`, asserting per-step invariants. `io_checks`
/// enables the mid-battle serde + clone checks. Returns the outcome (used for
/// the determinism re-run) or the first invariant violation found.
fn run_one(
    seed: u64,
    opts: &FuzzOptions,
    cache: &mut LearnsetCache,
    io_checks: bool,
) -> Result<Outcome, String> {
    let active = opts.format.active_count();
    let (p1_members, p1) = random_team(mix(seed, 0x11), opts.champions_only, cache);
    let (p2_members, p2) = random_team(mix(seed, 0x22), opts.champions_only, cache);

    if opts.verify_teams && opts.champions_only {
        if let Err(v) = verify_team(&p1_members, &REG_M_B) {
            return Err(format!(
                "p1 team failed verifier: {}",
                v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("; ")
            ));
        }
        if let Err(v) = verify_team(&p2_members, &REG_M_B) {
            return Err(format!(
                "p2 team failed verifier: {}",
                v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("; ")
            ));
        }
    }

    let mut battle = Battle::new(BattleConfig { format: opts.format, seed }, p1, p2);
    let mut picker = Rng::new(mix(seed, 0x33));

    let mut p1_buf: Vec<Choice> = Vec::with_capacity(active);
    let mut p2_buf: Vec<Choice> = Vec::with_capacity(active);

    let mut steps = 0u64;
    let mut turn = 0u32;

    loop {
        if turn >= opts.max_turns {
            // Cap reached. Under random play a small fraction of games never
            // terminate (PP exhaustion with no Struggle, or status/Protect
            // loops). This is flagged, not fatal — the per-step
            // `legal_choices`-non-empty check already rules out a true
            // soft-lock (a slot that MUST act but has zero choices).
            let pp_stall = is_pp_exhaustion_stall(&battle, active);
            let final_state = serde_json::to_string(&battle).map_err(|e| e.to_string())?;
            return Ok(Outcome { completed: false, capped: true, pp_stall, steps, final_state });
        }

        // Choices are taken straight from legal_choices, so by construction
        // every choice fed to step() is legal ("choices accepted" invariant).
        pick_side_choices(&battle, SideRef::P1, active, &mut picker, &mut p1_buf);
        pick_side_choices(&battle, SideRef::P2, active, &mut picker, &mut p2_buf);

        // Clone independence: clone, step the clone with its own choices, and
        // assert the original is byte-for-byte unchanged.
        if io_checks && turn == 1 {
            let before = serde_json::to_string(&battle).map_err(|e| e.to_string())?;
            let mut clone = battle.clone();
            let mut cp = Rng::new(mix(seed, 0x44));
            let mut c1 = Vec::with_capacity(active);
            let mut c2 = Vec::with_capacity(active);
            pick_side_choices(&clone, SideRef::P1, active, &mut cp, &mut c1);
            pick_side_choices(&clone, SideRef::P2, active, &mut cp, &mut c2);
            let _ = clone.step(&c1, &c2);
            let after = serde_json::to_string(&battle).map_err(|e| e.to_string())?;
            if before != after {
                return Err("clone independence violated: stepping a clone mutated the original".into());
            }
        }

        let res = battle.step(&p1_buf, &p2_buf);
        steps += 1;
        turn += 1;

        // --- per-step invariants ---
        check_hp_invariants(&battle).map_err(|e| format!("turn {turn}: {e}"))?;

        match res {
            StepResult::Ended { .. } => break,
            StepResult::Continue => {
                check_legal_choices_nonempty(&battle, active)
                    .map_err(|e| format!("turn {turn}: {e}"))?;
            }
        }

        // Serde round-trip mid-battle: serialize → deserialize → re-serialize,
        // assert identical, then continue from the restored state.
        if io_checks && turn == 2 {
            let s1 = serde_json::to_string(&battle).map_err(|e| e.to_string())?;
            let restored: Battle = serde_json::from_str(&s1).map_err(|e| e.to_string())?;
            let s2 = serde_json::to_string(&restored).map_err(|e| e.to_string())?;
            if s1 != s2 {
                return Err(format!("turn {turn}: serde round-trip changed state"));
            }
            battle = restored;
        }
    }

    // Reaching here means the loop broke on `StepResult::Ended` (the turn-cap
    // path returns early), so the battle completed.
    let final_state = serde_json::to_string(&battle).map_err(|e| e.to_string())?;
    Ok(Outcome { completed: true, capped: false, pp_stall: false, steps, final_state })
}

/// Run the full fuzz campaign described by `opts` and return an aggregate
/// report. Never panics on engine behavior — invariant breaches are collected
/// as `violations` strings (an engine panic would propagate as a normal Rust
/// panic, which is itself the headline "no panic" check).
pub fn run_fuzz(opts: FuzzOptions) -> FuzzReport {
    let mut cache = LearnsetCache::new();
    let mut report = FuzzReport { battles: opts.battles, ..Default::default() };

    for b in 0..opts.battles {
        let seed = opts.base_seed.wrapping_add((b as u64).wrapping_mul(SEED_GAMMA));
        let do_io = opts.check_every != 0 && b % opts.check_every == 0;

        match run_one(seed, &opts, &mut cache, do_io) {
            Ok(outcome) => {
                report.total_steps += outcome.steps;
                if outcome.completed {
                    report.completed += 1;
                }
                if outcome.capped {
                    report.capped += 1;
                    if outcome.pp_stall {
                        report.pp_exhaustion_stalls += 1;
                    }
                }
                // Determinism: re-run the same seed (no IO checks) and compare
                // the final serialized state.
                if do_io {
                    match run_one(seed, &opts, &mut cache, false) {
                        Ok(o2) if o2.final_state == outcome.final_state => {}
                        Ok(_) => push_violation(
                            &mut report,
                            format!("seed {seed:#x}: nondeterministic — re-run final state differs"),
                        ),
                        Err(e) => push_violation(
                            &mut report,
                            format!("seed {seed:#x}: determinism re-run errored: {e}"),
                        ),
                    }
                }
            }
            Err(e) => push_violation(&mut report, format!("seed {seed:#x}: {e}")),
        }
    }
    report
}

fn push_violation(report: &mut FuzzReport, msg: String) {
    report.violations_total += 1;
    if report.violations.len() < VIOLATION_CAP {
        report.violations.push(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_champions_team_is_legal_and_builds() {
        let mut cache = LearnsetCache::new();
        for s in 0..200u64 {
            let (members, pokemon) = random_team(mix(s, 0x11), true, &mut cache);
            assert!(
                (4..=6).contains(&members.len()),
                "team size out of range: {}",
                members.len()
            );
            assert_eq!(members.len(), pokemon.len());
            verify_team(&members, &REG_M_B).unwrap_or_else(|v| {
                panic!(
                    "seed {s} generated an illegal Champions team: {}",
                    v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("; ")
                )
            });
        }
    }

    #[test]
    fn random_team_is_deterministic() {
        let mut c1 = LearnsetCache::new();
        let mut c2 = LearnsetCache::new();
        let (m1, _) = random_team(12345, true, &mut c1);
        let (m2, _) = random_team(12345, true, &mut c2);
        assert_eq!(m1.len(), m2.len());
        for (a, b) in m1.iter().zip(m2.iter()) {
            assert_eq!(a.species, b.species);
            assert_eq!(a.ability, b.ability);
            assert_eq!(a.moves, b.moves);
            assert_eq!(a.item, b.item);
            assert_eq!(a.nature, b.nature);
        }
    }

    #[test]
    fn small_fuzz_smoke() {
        let report = run_fuzz(FuzzOptions {
            battles: 50,
            base_seed: 0xABCDEF,
            check_every: 5,
            ..Default::default()
        });
        assert!(
            report.violations.is_empty(),
            "smoke fuzz violations: {}",
            report.violations.join("\n")
        );
    }
}
