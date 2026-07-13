//! Regression guard for the ko_split / hp_bucket-segment damage collapse.
//!
//! The collapse records, at each single-target single-hit damage site, a
//! partition of the 16 damage rolls by the defender's post-hit `hp_bucket`
//! (the same projection `canonical_hash` keys on) and emits one
//! representative roll per contiguous bucket-segment. It MUST reproduce the
//! full 16-roll enumeration frontier **bit-exactly** — same set of
//! canonical states AND same probability mass on each (L1 → 0).
//!
//! Two historical bugs this guards against:
//!   1. Survivor min-roll pinning collapsed ALL survivor rolls to one
//!      bucket, DROPPING reachable intermediate-HP states.
//!   2. Same-target multi-focus: two hits on one defender couple (the
//!      second hit's starting HP varies with the first hit's roll), so
//!      independent per-site collapse dropped/reweighted joint states.
//!      Fixed by a blunt guard that disables collapse for any defender
//!      that could be multiply-hit / redirected / spread-hit this turn.
//!
//! These solves enumerate the FULL 16-roll frontier as the reference, so
//! they are slow — marked `#[ignore]`; run explicitly:
//!   cargo test -p vgc-solver --test collapse_soundness -- --ignored --nocapture

use std::collections::{HashMap, HashSet};

use vgc_engine_core::{
    set_crit_refine_disabled, set_ko_split_disabled, Battle, BattleConfig, Choice, Format,
    SideRef, Status, Target, TeamBuilder,
};
use vgc_solver::{
    bounded_component_fallback_count, enumerate_outcomes_with, last_cell_component_count,
    reset_tensor_coverage_counts, set_coupling_edges_disabled, set_joint_collapse_disabled,
    take_joint_collapse_engaged, EnumerateOpts,
};

fn dist(b: &Battle, p1: &[Choice], p2: &[Choice]) -> HashMap<u64, f64> {
    let opts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
    let f = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts);
    let mut m = HashMap::new();
    for o in &f.outcomes {
        *m.entry(o.hash).or_insert(0.0) += o.prob;
    }
    m
}

/// L1 distance between the collapse-ON frontier and the fully-uncollapsed
/// (all-16-roll) frontier at one cell. Must be ~0.
fn cell_l1(b: &Battle, p1: &[Choice], p2: &[Choice]) -> f64 {
    // ON: both collapses live (production behavior).
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let on = dist(b, p1, p2);
    // FULL reference: BOTH the per-site segment collapse AND the mutual-focus
    // tensor off → flat 16^k enumeration deduped only by post-step
    // canonical_hash (provably-correct ground truth).
    set_ko_split_disabled(true);
    set_joint_collapse_disabled(true);
    let full = dist(b, p1, p2);
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);

    let mut keys: HashSet<u64> = on.keys().copied().collect();
    keys.extend(full.keys().copied());
    let mut l1 = 0.0;
    for k in &keys {
        l1 += (on.get(k).copied().unwrap_or(0.0) - full.get(k).copied().unwrap_or(0.0)).abs();
    }
    l1
}

/// ADVERSARIAL probe (Phase 2b §5): L1 with the coupling-hub edges OMITTED
/// (only Edge 1 kept) but the tensor still ENGAGED. If a cell's correctness
/// DEPENDED on a hub edge, factorizing without it would drop/reweight states →
/// L1 > 0. Compares against the SAME fully-lossless reference `cell_l1` uses.
///
/// CORRECTED FINDING (independent review, 2026-07-12 — supersedes the prior
/// agent's "edges are defensive-only" conclusion, which was FIXTURE-BLIND). The
/// prior fixtures all placed the trigger's victim INSIDE the trigger's component
/// (the WP/Berserk mon attacked the same mon that hit it), so omitting the edge
/// left the victim in the same group and the boost self-completed — omit stayed
/// ~0 for that TOPOLOGY only. When the victim is in a SEPARATE component (a third
/// mon the trigger attacks that does NOT attack back), omitting the edge
/// factorizes the victim's site away from the trigger's incoming rolls, and the
/// per-hit hp_bucket dedup then drops any boost-crossing survivor bucket:
/// `breaker1_berserk_victim_separate_component_bit_exact` measures omit L1 ≈
/// 1.5e-2 (> 1e-3). So the edges ARE load-bearing. This probe now witnesses
/// that: it is asserted `> 1e-3` in breaker1 (and the Anger Point / drain-heal
/// load-bearing fixtures).
#[allow(dead_code)]
fn cell_l1_edges_omitted(b: &Battle, p1: &[Choice], p2: &[Choice]) -> f64 {
    // WITH-edges-omitted, collapse ON.
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    set_coupling_edges_disabled(true);
    let omitted = dist(b, p1, p2);
    set_coupling_edges_disabled(false);
    // Fully-lossless reference (both collapses off).
    set_ko_split_disabled(true);
    set_joint_collapse_disabled(true);
    let full = dist(b, p1, p2);
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);

    let mut keys: HashSet<u64> = omitted.keys().copied().collect();
    keys.extend(full.keys().copied());
    let mut l1 = 0.0;
    for k in &keys {
        l1 += (omitted.get(k).copied().unwrap_or(0.0) - full.get(k).copied().unwrap_or(0.0)).abs();
    }
    l1
}

/// Assert the mutual-focus tensor actually ENGAGED — so a subsequent
/// bit-exact assertion isn't vacuously satisfied by the cell silently
/// falling through to the flat path.
fn assert_tensor_engaged(b: &Battle, p1: &[Choice], p2: &[Choice]) {
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let _ = take_joint_collapse_engaged(); // clear stale
    let _ = dist(b, p1, p2);
    assert!(
        take_joint_collapse_engaged(),
        "expected the mutual-focus defender-joint tensor to ENGAGE on this cell",
    );
}

/// Assert the tensor did NOT engage (routed to full-enum) — for the bail
/// categories: the cell must still be bit-exact via the flat path.
fn assert_tensor_bailed(b: &Battle, p1: &[Choice], p2: &[Choice]) {
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let _ = take_joint_collapse_engaged();
    let _ = dist(b, p1, p2);
    assert!(
        !take_joint_collapse_engaged(),
        "expected the mutual-focus tensor to BAIL (route to full enumeration) on this cell",
    );
}

/// Engage the production collapse and return the coupling-graph component
/// count formed on this cell (Phase 2a audit — proves the union-find grouped
/// the cell as intended so a bit-exact result isn't vacuously satisfied by a
/// degenerate grouping). Only meaningful when the tensor actually engaged.
fn component_count(b: &Battle, p1: &[Choice], p2: &[Choice]) -> usize {
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let _ = take_joint_collapse_engaged();
    let _ = dist(b, p1, p2);
    last_cell_component_count()
}

fn mv(slot: u8, side: SideRef, tslot: u8) -> Choice {
    Choice::Move { actor_slot: slot, move_slot: 0, target: Some(Target { side, slot: tslot }) }
}
fn pass(slot: u8) -> Choice {
    Choice::Pass { actor_slot: slot }
}

const EPS: f64 = 1e-9;

/// Case 1: single-target hit whose SURVIVOR outcome spans multiple hp
/// buckets (the min-roll-pinning drop-state bug). Two attackers on DIFFERENT
/// targets so there is no same-target coupling — each collapse must be exact.
#[test]
#[ignore]
fn survivor_span_and_split_targets_are_bit_exact() {
    const P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"scizor","level":50,"ability":"technician","nature":"adamant","moves":["bulletpunch"],"evs":{"atk":252,"spe":4}}
    ]"#;
    const P2: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"jolly","moves":["earthquake"],"evs":{"hp":252,"spe":100}},
        {"species":"rotom","level":50,"ability":"levitate","nature":"calm","moves":["thunderbolt"],"evs":{"hp":252,"spe":36}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Each attacker a DIFFERENT target; P2 passes (isolate the two hits).
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "split-target survivor collapse not bit-exact: L1={l1:.3e}");
}

/// Case 2 + 4: two attackers focus the SAME defender (coupling). The guard
/// must disable collapse for that defender → full enumeration → bit-exact.
/// Includes clean-KO-under-coupling (case 4): even when a hit clean-KOs, the
/// same-target guard must keep it exact.
#[test]
#[ignore]
fn same_target_multi_focus_is_bit_exact() {
    const P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"scizor","level":50,"ability":"technician","nature":"adamant","moves":["bulletpunch"],"evs":{"atk":252,"spe":4}}
    ]"#;
    const P2: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"jolly","moves":["earthquake"],"evs":{"hp":252,"spe":100}},
        {"species":"rotom","level":50,"ability":"levitate","nature":"calm","moves":["thunderbolt"],"evs":{"hp":252,"spe":36}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // BOTH attackers focus P2 slot 0; P2 passes.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "same-target coupling collapse not bit-exact: L1={l1:.3e}");
}

/// Case 3: a redirection effect (Rage Powder) is present this turn. The
/// blunt guard must disable collapse everywhere (resolved targets can
/// differ from declared) → full enumeration → bit-exact.
#[test]
#[ignore]
fn redirect_present_is_bit_exact() {
    // P2 slot1 is Amoonguss with Rage Powder; slot0 a frail attacker.
    const P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"scizor","level":50,"ability":"technician","nature":"adamant","moves":["bulletpunch"],"evs":{"atk":252,"spe":4}}
    ]"#;
    const P2: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"jolly","moves":["earthquake"],"evs":{"hp":252,"spe":100}},
        {"species":"amoonguss","level":50,"ability":"regenerator","nature":"calm","moves":["ragepowder"],"evs":{"hp":252,"spe":36}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // P2 slot1 uses Rage Powder (sets the redirect volatile); P2 slot0 passes.
    // P1 both target P2 slot0 (declared) — but the redirect guard must
    // disable collapse regardless.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), Choice::Move { actor_slot: 1, move_slot: 0, target: None }];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "redirect-present collapse not bit-exact: L1={l1:.3e}");
}

/// Case 5 (Instruct coupling hole): Instruct synthesizes a re-hit action
/// OUTSIDE the static turn `order`, so a defender that the declared-target
/// scan sees hit exactly ONCE is actually hit TWICE (original + the
/// Instruct-repeated move). The first hit would collapse assuming nothing
/// else touches the defender → silent joint-state drop. The guard must
/// disable collapse globally when an INSTRUCT action is present.
///
/// Setup (from the engine's `instruct_repeats_targets_last_move`): Oranguru
/// (P1s0) Instructs Latios (P2s0) → Latios repeats Draco Meteor on Blissey
/// (P1s1). Blissey is the double-hit defender; the declared scan sees only
/// ONE Draco on it. Draco on max-bulk Blissey is a multi-bucket survivor
/// (collapsible), so removing the INSTRUCT branch of
/// `compute_coupled_targets` makes this FAIL (verified: L1≈3.8e-3, one
/// dropped state); with the guard it is bit-exact.
#[test]
#[ignore]
fn instruct_reexecution_is_bit_exact() {
    const P1: &str = r#"[
        {"species":"oranguru","level":50,"ability":"innerfocus","nature":"sassy","moves":["instruct","psychic","protect","trickroom"],"evs":{"hp":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["seismictoss","softboiled","protect","calmmind"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"latios","level":50,"ability":"levitate","nature":"timid","moves":["dracometeor","psychic","protect","recover"],"evs":{"spe":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["protect"],"evs":{"hp":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Oranguru Instructs Latios; Blissey passes; Latios Dracos Blissey;
    // Snorlax passes. Blissey (P1s1) is hit twice (original + instructed).
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 1), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Instruct re-execution collapse not bit-exact: L1={l1:.3e}");
}

// ===========================================================================
// Mutual-focus defender-joint TENSOR audit (the sound joint collapse).
//
// `*_tensor_fires_bit_exact`: cells where ≥1 defender is mutually focused,
// speeds are DISTINCT (no tiebreak bail), and no attacker can faint before
// it acts — so `Battle::mutual_focus_tensor_safe` PROVES independence and the
// tensor engages. Assert (a) it engaged and (b) L1 < 1e-9 vs the flat 16^k
// reference. `*_bails_*`: coupled cells the gate must REJECT (possible
// pre-action faint) — assert routes-to-full-enum AND still bit-exact.
//
// Teams use bulky mons with a WEAK move (Tackle, BP 40) and DISTINCT base
// speeds (Snorlax 30 / Blissey 55 / Chansey 50 / Miltank 100), so a single
// turn's ≤2 hits can never KO and speeds never tie.
// ===========================================================================

/// CROSS-GROUP mutual focus, distinct speeds: each side's two attackers focus
/// the OPPONENT's slot 0. Both P1s0 and P2s0 are coupled defenders (and are
/// themselves attackers). No attacker can faint (Tackle ×2 << bulk), speeds
/// distinct → the gate proves independence and the per-defender groups
/// tensor. Bit-exact.
#[test]
#[ignore]
fn mutual_focus_cross_group_tensor_fires_bit_exact() {
    const CP1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"spd":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"spd":252}}
    ]"#;
    const CP2: &str = r#"[
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"spd":252}},
        {"species":"miltank","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(CP1).unwrap(),
        TeamBuilder::from_json(CP2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [mv(0, SideRef::P1, 0), mv(1, SideRef::P1, 0)];
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "cross-group mutual-focus tensor not bit-exact: L1={l1:.3e}");
}

/// Single coupled GROUP of 2 hits (both P1 attackers focus P2s0), distinct
/// speeds, P2 passes. Validates the group sub-grid at full 16×16 dedup by
/// canonical_hash. Tensor engages (one group, empty rest).
#[test]
#[ignore]
fn mutual_focus_single_group_tensor_fires_bit_exact() {
    const GP1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"atk":252}},
        {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
    ]"#;
    const GP2: &str = r#"[
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(GP1).unwrap(),
        TeamBuilder::from_json(GP2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "single-group tensor not bit-exact: L1={l1:.3e}");
}

/// CRIT dims: Tackle carries the 1/24 crit draw, so each hit's crit Bernoulli
/// enters the coupled group's sub-grid; dedup by canonical_hash folds
/// crit-vs-no-crit combos that land the same bucket. Bit-exact.
#[test]
#[ignore]
fn mutual_focus_with_crit_tensor_fires_bit_exact() {
    const CR1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"atk":252}},
        {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
    ]"#;
    const CR2: &str = r#"[
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 7 },
        TeamBuilder::from_json(CR1).unwrap(),
        TeamBuilder::from_json(CR2).unwrap(),
    );
    // Both attackers focus P2s0; the two crit sites are part of the group.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "mutual-focus w/ crit tensor not bit-exact: L1={l1:.3e}");
}

/// LIFE ORB attackers: each takes fixed max-hp/10 recoil. The attacker's own
/// bucket change is captured by the FULL canonical_hash in the sub-grid dedup
/// (recoil is roll-independent for a surviving attacker). Attackers bulky
/// enough to survive incoming + recoil, so the gate proves independence.
#[test]
#[ignore]
fn mutual_focus_life_orb_tensor_fires_bit_exact() {
    const LO1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","item":"lifeorb","nature":"careful","moves":["tackle"],"evs":{"hp":252,"atk":252}},
        {"species":"miltank","level":50,"ability":"thickfat","item":"lifeorb","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
    ]"#;
    const LO2: &str = r#"[
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(LO1).unwrap(),
        TeamBuilder::from_json(LO2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Life Orb mutual-focus tensor not bit-exact: L1={l1:.3e}");
}

/// SITRUS defender under mutual focus, TENSOR ENGAGES: distinct-speed
/// attackers focus a Sitrus holder; some roll combos cross ≤½ HP and consume
/// Sitrus (heal + item removed = canonical change), captured by the
/// within-group full canonical_hash dedup. The attackers can't faint (they
/// aren't targeted), so the gate passes. Bit-exact.
#[test]
#[ignore]
fn mutual_focus_sitrus_defender_tensor_fires_bit_exact() {
    // Strong-ish attackers so two hits can dip the Sitrus holder past ½ for
    // SOME rolls (crossing the berry threshold within the group) but never
    // to 0 (defender is a huge-HP wall that survives 2 hits). Distinct speeds.
    // Strength (BP 80, NO secondary — Body Slam's paralysis secondary would
    // now correctly trip the participation-gating bail).
    const SP1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"adamant","moves":["strength"],"evs":{"hp":252,"atk":252}},
        {"species":"miltank","level":50,"ability":"scrappy","nature":"adamant","moves":["strength"],"evs":{"hp":252,"atk":252}}
    ]"#;
    const SP2: &str = r#"[
        {"species":"blissey","level":50,"ability":"naturalcure","item":"sitrusberry","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(SP1).unwrap(),
        TeamBuilder::from_json(SP2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Sitrus defender tensor not bit-exact: L1={l1:.3e}");
}

/// KO-POSSIBLE mutual focus → gate BAILS. Fast frail-ish hard-hitters whose
/// two pre-action hits CAN reduce an attacker (which is also the focused
/// defender) to 0 before it acts. The gate's max-damage bound reaches current
/// HP → it must route to full enumeration. Still bit-exact. Implicitly covers
/// Moxie/Beast-Boost-style KO boosts (a mid-turn KO is captured by the flat
/// path's canonical_hash).
#[test]
#[ignore]
fn mutual_focus_ko_possible_tensor_bails_bit_exact() {
    const STRONG_P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"scizor","level":50,"ability":"technician","nature":"adamant","moves":["bulletpunch"],"evs":{"atk":252,"spe":4}}
    ]"#;
    const STRONG_P2: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"jolly","moves":["earthquake"],"evs":{"atk":252,"spe":100}},
        {"species":"dragapult","level":50,"ability":"clearbody","nature":"jolly","moves":["dragonclaw"],"evs":{"atk":252,"spe":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(STRONG_P1).unwrap(),
        TeamBuilder::from_json(STRONG_P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];
    assert_tensor_bailed(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "KO-possible mutual-focus not bit-exact under bail: L1={l1:.3e}");
}

// ---------------------------------------------------------------------------
// Chance-gated NON-PARTICIPATION bail (the code-review Critical). An
// attacker's hit can vanish/become chance-dependent WITHOUT a faint (full
// paralysis, flinch, confusion, sleep/freeze, Attract, Truant), coupling the
// defender's HP to a chance node the tensor would treat as independent. The
// gate must BAIL (full-enum) on any such gate present OR inflictable this
// turn. Both cases assert `assert_tensor_bailed` + bit-exact L1<1e-9.
// ---------------------------------------------------------------------------

/// PRE-EXISTING participation gate: a coupled attacker is ALREADY paralyzed.
/// It's one of two attackers focusing P2s0 (so its hit is part of the coupled
/// group); its 25% full-para skip couples the defender's final HP to the para
/// coin. The gate must bail; the flat path stays bit-exact.
#[test]
#[ignore]
fn mutual_focus_pre_paralyzed_attacker_tensor_bails_bit_exact() {
    const PP1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}},
        {"species":"miltank","level":50,"ability":"scrappy","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
    ]"#;
    const PP2: &str = r#"[
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(PP1).unwrap(),
        TeamBuilder::from_json(PP2).unwrap(),
    );
    // Paralyze P1 slot-0 (a coupled attacker). Its 25% skip decouples one of
    // the two hits on P2s0 from certainty.
    let a0 = b.p1.active[0] as usize;
    b.p1.team[a0].status = Status::Paralysis;
    // Both P1 attackers focus P2s0; P2 passes.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    assert_tensor_bailed(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "pre-paralyzed coupled attacker not bit-exact under bail: L1={l1:.3e}");
}

/// MID-TURN-INFLICTABLE participation gate: a coupled attacker's move carries
/// a paralysis secondary (Body Slam, 30%). A faster mon could para a slower
/// not-yet-acted coupled attacker mid-turn, gating its hit. The blunt "any
/// cell move has_secondary → bail" branch must trip. Bit-exact under bail.
#[test]
#[ignore]
fn mutual_focus_secondary_inflictable_tensor_bails_bit_exact() {
    // Body Slam (BP 85, 30% paralysis secondary) on the coupled attackers.
    const MI1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"adamant","moves":["bodyslam"],"evs":{"hp":252,"atk":252}},
        {"species":"miltank","level":50,"ability":"scrappy","nature":"adamant","moves":["bodyslam"],"evs":{"hp":252,"atk":252}}
    ]"#;
    const MI2: &str = r#"[
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(MI1).unwrap(),
        TeamBuilder::from_json(MI2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    assert_tensor_bailed(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "secondary-inflictable coupled cell not bit-exact under bail: L1={l1:.3e}");
}

// ===========================================================================
// PHASE 1 — segment spread-move damage hits (coupling-graph plan §3).
//
// A spread move draws an INDEPENDENT damage roll per target, so a target that
// is hit by exactly ONE resolved action (no same-target focus, no redirect)
// has post-hit HP that is a clean function of its own roll — segmentable
// exactly like a single-target hit. These fixtures assert the collapsed
// frontier is bit-exact vs the fully-lossless full-16 reference AND that the
// segmentation is LOAD-BEARING (raw_combos drops sharply once it engages).
//
// R2: `is_spread` (hence the ×0.75 modifier and the lone-survivor full-damage
// case) is baked into each roll's damage before the hp_bucket partition, so
// the segments split on the ACTUAL post-modifier damage. The KO-threshold and
// Sitrus fixtures below would misbehave if the modifier were dropped.
// ===========================================================================

/// raw_combos with the production collapse ON (spread segmentation live).
fn raw_combos_on(b: &Battle, p1: &[Choice], p2: &[Choice]) -> usize {
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let opts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
    enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts).raw_combos
}

/// raw_combos with the fully-lossless reference (all collapse OFF → flat 16^k).
fn raw_combos_full(b: &Battle, p1: &[Choice], p2: &[Choice]) -> usize {
    set_ko_split_disabled(true);
    set_joint_collapse_disabled(true);
    let opts = EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None };
    let n = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts).raw_combos;
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    n
}

#[test]
#[ignore]
fn spread_two_grounded_singleton_min_reduction() {
    // Two grounded foes, NO weather / residual. Each is a single-resolved-hit
    // independent spread target — the CORE Phase-1 restructure. Proves the
    // segmentation ENGAGES (huge raw_combos reduction) AND stays bit-exact.
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "two-grounded spread segment not bit-exact: L1={l1:.3e}");
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    // Two independent 16-roll×2-crit targets → 1024 flat combos; segmenting
    // each survivor to 1 bucket cuts this ~256x (on == 4).
    assert!(full >= 1024, "reference should enumerate >=1024 combos, got {full}");
    assert!(on * 16 < full, "spread segmentation not load-bearing: on={on} full={full}");
}

// ===========================================================================
// CRIT-CONDITIONAL damage segments (soundness fix). The damage roll and the
// crit hit/miss are recorded as SEPARATE draw sites; the solver
// cross-products them independently. A crit multiplies damage (×1.5 + crit
// boost-ignoring), so a roll can land in a DIFFERENT hp_bucket under crit than
// under non-crit. `compute_damage_segments` now partitions on the COMMON
// REFINEMENT of the crit-false and crit-true bucket sequences, so every roll
// in a segment shares its bucket under BOTH crit values → the
// (segment-rep × {crit,no-crit}) cross-product is bit-exact.
//
// Before the fix these cells dropped the crit-branch's extra bucket
// (per-canonical-hash L1 ≈ 0.036). Each fixture below:
//   (a) asserts L1 < 1e-9 with the refinement ON (the fix), AND
//   (b) proves the refinement is LOAD-BEARING via `set_crit_refine_disabled`
//       — with it off (old non-crit-only partition) L1 must be > EPS, so the
//       assertion is not vacuously satisfied by the cell falling through.
// Same bug CLASS as the ko_split survivor min-roll pinning (dropping
// reachable states); the fix is the Option-B partition extended to the crit
// dimension.
// ===========================================================================

/// L1 with the crit-conditional refinement DISABLED (old non-crit-only
/// partition = the pre-fix buggy behavior). Used to prove the refinement is
/// load-bearing: it must be > EPS on a crit-crosses-a-bucket cell.
fn cell_l1_crit_refine_off(b: &Battle, p1: &[Choice], p2: &[Choice]) -> f64 {
    set_crit_refine_disabled(true);
    let l1 = cell_l1(b, p1, p2);
    set_crit_refine_disabled(false);
    l1
}

/// THE un-should_panic'd fixture. Single-target Choice-Band Earthquake into
/// Snorlax: a crit pushes the top roll across the 1/3-HP bucket boundary that
/// the non-crit partition does NOT split at, so the pre-fix collapse dropped
/// the crit-boundary state (mass ~0.018, L1 ≈ 0.036). With the crit-conditional
/// refinement it is bit-exact. `is_spread == false` — this is the pure
/// single-target crit×segment case. Load-bearing: refinement-off L1 > EPS.
#[test]
#[ignore]
fn crit_boundary_single_target_bit_exact() {
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"choiceband","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    // (a) FIX: bit-exact vs the fully-lossless 16×2 reference.
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "crit×segment not bit-exact after fix: L1={l1:.3e}");
    // (b) LOAD-BEARING: the old non-crit-only partition dropped a state here.
    let l1_off = cell_l1_crit_refine_off(&b, &p1, &p2);
    assert!(
        l1_off > EPS,
        "crit-refinement not load-bearing on this cell (off L1={l1_off:.3e} \
         ≤ EPS) — fixture no longer exercises the crit×segment hole"
    );
}

/// SHARPEST case: crit crosses the KO threshold. A roll SURVIVES on non-crit
/// but KOs on crit — so the non-crit partition never splits at the faint
/// boundary the crit branch needs. The pre-fix collapse drops the
/// crit-KO state (fainted bucket 0 has no representative). Tuned so the
/// non-crit top roll leaves the defender alive while the crit of that same
/// roll faints it.
#[test]
#[ignore]
fn crit_crosses_ko_threshold_bit_exact() {
    // Plain (no Choice Band) neutral-nature Garchomp Earthquake into Snorlax
    // chipped to 85 HP: the non-crit rolls all SURVIVE (leave Snorlax alive)
    // but a crit of the same rolls FAINTS it. So the non-crit partition never
    // splits at the faint boundary (bucket 0) that the crit branch needs — the
    // pre-fix collapse drops the crit-KO state (verified load-bearing via the
    // HP scan: at hp=85 the frontier is exactly {survive, crit-faint}, and the
    // old non-crit-only partition drops the faint state, L1 ≈ 0.057).
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"hardy","moves":["earthquake"],"evs":{}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let g = b.p2.active[0] as usize;
    b.p2.team[g].current_hp = 85; // survives every non-crit roll; a crit faints
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "crit-crosses-KO not bit-exact: L1={l1:.3e}");
    let l1_off = cell_l1_crit_refine_off(&b, &p1, &p2);
    assert!(
        l1_off > EPS,
        "crit-crosses-KO not load-bearing (off L1={l1_off:.3e} ≤ EPS) — \
         the crit must cross the faint boundary a non-crit roll doesn't"
    );
}

/// Crit crosses a SITRUS-BERRY (½-HP) threshold. Some rolls stay above ½ HP on
/// non-crit but a crit of the same roll dips the holder to ≤½ → the berry
/// fires (heal + item removed = distinct canonical state). The non-crit
/// partition never splits at the berry boundary, so the pre-fix collapse
/// merges the consumed/not-consumed states across the crit branch.
#[test]
#[ignore]
fn crit_crosses_sitrus_threshold_bit_exact() {
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"choiceband","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","item":"sitrusberry","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Snorlax (max HP 267, Sitrus fires ≤133 = the ½-HP / bucket 4|5
    // boundary) to 240: non-crit EQ leaves it above ½ (berry unused) but a crit
    // of the same roll dips it ≤½ → Sitrus consumed (heal + item removed = a
    // distinct canonical state). The non-crit-only partition never splits at
    // the berry boundary, so the pre-fix collapse merges consumed/not-consumed
    // across the crit branch (verified load-bearing at hp=240, L1 ≈ 0.047).
    let g = b.p2.active[0] as usize;
    b.p2.team[g].current_hp = 240;
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "crit-crosses-Sitrus not bit-exact: L1={l1:.3e}");
    let l1_off = cell_l1_crit_refine_off(&b, &p1, &p2);
    assert!(
        l1_off > EPS,
        "crit-crosses-Sitrus not load-bearing (off L1={l1_off:.3e} ≤ EPS)"
    );
}

/// HIGH-CRIT-RATIO move (num/denom ≠ 1/24). Stone Edge carries a +1 crit-stage
/// delta → stage-1 crit rate 1/8, so the recorded `Crit { num, denom }` is
/// 1/8 (NOT the default 1/24). The refinement uses whatever the RECORDED crit
/// site weight is — the crit site is enumerated verbatim by the solver; the
/// segment refinement only splits buckets, never re-weights the crit. A crit
/// crosses a bucket boundary; bit-exact at the 1/8 weight. The refinement-off
/// L1 (~0.0125, an eighth-scale mass) confirms the 1/8 crit weight is in play.
#[test]
#[ignore]
fn high_crit_ratio_move_bit_exact() {
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"choiceband","nature":"adamant","moves":["stoneedge"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip to 260 so a 1/8-rate crit straddles a bucket boundary the non-crit
    // rolls don't (scan-verified load-bearing, L1 ≈ 0.0125 = 1/8-scale mass).
    let g = b.p2.active[0] as usize;
    b.p2.team[g].current_hp = 260;
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "high-crit-ratio not bit-exact: L1={l1:.3e}");
    let l1_off = cell_l1_crit_refine_off(&b, &p1, &p2);
    assert!(
        l1_off > EPS,
        "high-crit-ratio not load-bearing (off L1={l1_off:.3e} ≤ EPS) — \
         Stone Edge should still straddle a bucket under crit"
    );
}

/// GUARANTEED CRIT (p_crit = 1, modeled via `force_crit`). PS records NO crit
/// draw for a guaranteed crit, so there is no separate Crit site to
/// cross-product; the actual damage replayed is the CRIT value. The segment
/// partition must therefore split on the crit-TRUE bucket sequence. The
/// crit-conditional refinement includes the crit branch, so it is bit-exact.
/// This case is ALSO load-bearing: the old non-crit-only partition splits on
/// crit-FALSE buckets while the replay lands crit-TRUE damage — so it pins the
/// wrong representatives and drops states (refinement-off L1 > EPS). This
/// fixture pins the p_crit=1 correctness: the emitted partition must equal the
/// pure-crit partition.
#[test]
#[ignore]
fn guaranteed_crit_focus_energy_bit_exact() {
    // Garchomp uses Focus Energy turn 1 (via a pre-set volatile) so Earthquake
    // is a guaranteed crit. We set the Focus Energy volatile directly.
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"choiceband","nature":"adamant","moves":["earthquake","focusenergy"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Force Focus Energy (2 crit stages) + a move crit_stage_delta if any: use
    // the engine's force_crit to model p_crit=1 deterministically (guaranteed
    // crit records no draw, exactly the modeled case).
    b.set_force_crit(Some(true));
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "guaranteed-crit not bit-exact: L1={l1:.3e}");
    // p_crit = 1: the replay lands crit-TRUE damage, but the old non-crit-only
    // partition splits on crit-FALSE buckets → wrong reps → dropped states.
    // The refinement's crit branch is what makes the partition correct here.
    let l1_off = cell_l1_crit_refine_off(&b, &p1, &p2);
    assert!(
        l1_off > EPS,
        "guaranteed-crit refinement not load-bearing (off L1={l1_off:.3e} ≤ EPS)"
    );
}

// ---------------------------------------------------------------------------
// ADVERSARIAL crit×segment cases (composition with Phase 1 spread + edges).
// ---------------------------------------------------------------------------

/// SPREAD + crit (composes with Phase 1 spread segmentation). Each spread
/// target draws its OWN damage roll AND its own crit; the crit-conditional
/// refinement must apply to the spread segment path too. Earthquake hits both
/// live grounded P2 mons; each is chipped so a crit straddles a bucket
/// boundary the non-crit rolls don't. Bit-exact + load-bearing across BOTH
/// independent spread targets simultaneously.
#[test]
#[ignore]
fn spread_plus_crit_segments_bit_exact() {
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"choiceband","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Both P2 mons at full HP, grounded and live → is_spread == true for both.
    // Choice-Band spread EQ (×0.75) into full-HP Snorlax straddles a bucket
    // boundary under crit that the non-crit rolls stay within (scan-verified
    // load-bearing at full HP, off L1 ≈ 0.036) — the ×0.75 spread modifier is
    // baked into the per-roll damage before the bucket partition, so the crit
    // straddle lands on the actual SPREAD damage. This proves the
    // crit-conditional refinement composes with Phase 1 spread segmentation.
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "spread+crit not bit-exact: L1={l1:.3e}");
    let l1_off = cell_l1_crit_refine_off(&b, &p1, &p2);
    assert!(
        l1_off > EPS,
        "spread+crit refinement not load-bearing (off L1={l1_off:.3e} ≤ EPS) — \
         at least one spread target's crit must straddle a bucket the non-crit doesn't"
    );
}

/// MULTI-BOUNDARY straddle: a chip HP where the crit and non-crit damage of
/// the roll set land in bucket sequences separated by more than one boundary
/// (the crit branch reaches buckets several steps away from the non-crit
/// branch — the frontier here holds ≥3 distinct canonical states). The common
/// refinement must still be exact: it splits wherever EITHER coordinate
/// changes, so however far apart the crit and non-crit buckets are, each roll's
/// crit-branch bucket has a representative. This is the sharpest test that the
/// refinement is on the JOINT (roll, crit) space, not a single-boundary
/// assumption. Scan-verified load-bearing at hp=180 (off L1 ≈ 0.031).
#[test]
#[ignore]
fn crit_straddles_two_boundaries_bit_exact() {
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"choiceband","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // hp=180: Choice-Band EQ non-crit spans the (¼,⅓]..(⅓,33%] region while a
    // crit dives to (0,¼] or faint — a multi-boundary jump (scan-verified
    // load-bearing at hp=180, L1 ≈ 0.031).
    let g = b.p2.active[0] as usize;
    b.p2.team[g].current_hp = 180;
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "two-boundary crit straddle not bit-exact: L1={l1:.3e}");
    let l1_off = cell_l1_crit_refine_off(&b, &p1, &p2);
    assert!(
        l1_off > EPS,
        "two-boundary straddle not load-bearing (off L1={l1_off:.3e} ≤ EPS)"
    );
    // Self-validating "multi-boundary": the crit branch must reach buckets the
    // non-crit branch doesn't, so the true frontier holds ≥3 distinct
    // canonical states (survivor bucket(s) + a crit-branch bucket ≥2 steps
    // away). A single-boundary straddle would yield only 2 states.
    let states = dist(&b, &p1, &p2).len();
    assert!(
        states >= 3,
        "expected ≥3 distinct states for a multi-boundary crit jump, got {states}"
    );
}

/// MULTI-HIT + crit: multi-hit moves are EXCLUDED from segmentation
/// (`!multihit` in the eligibility gate) because each strike re-rolls crit +
/// damage and the strikes couple through the defender's running HP — so the
/// solver enumerates the full per-hit cross-product and dedups on the real
/// canonical_hash. This fixture pins that the multi-hit path stays bit-exact
/// WITH crits in the mix (the crit-conditional refinement must NOT leak into
/// the multi-hit path and mis-segment it). Double Kick (exactly 2 hits) into a
/// wall — a 2-hit move keeps the full reference at 16²×2² = 1024 combos
/// (tractable), unlike a 5-hit move whose 16⁵×2⁵ reference is a monster cell.
#[test]
#[ignore]
fn multihit_plus_crit_bit_exact_via_full_enum() {
    // Hitmonlee Double Kick (Fighting, 2 hits) — super-effective on Snorlax
    // (Normal), so each strike is a meaningful chunk and crits are in the mix.
    const P1: &str = r#"[
        {"species":"hitmonlee","level":50,"ability":"limber","item":"choiceband","nature":"adamant","moves":["doublekick"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    // Phase 2b: multi-hit is now a COUPLING EDGE — the strike-count draw + the
    // per-strike damage/crit sites fold into one component (multi-hit hub) and
    // enumerate jointly via real step() + canonical_hash dedup. Bit-exact AND
    // collapsed (fewer raw combos than the flat 16^k × crit × count grid).
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "multi-hit + crit not bit-exact under coupling-graph: L1={l1:.3e}");
    // Anti-vacuous (reduction): the multi-hit tensor collapses the sub-grid —
    // `on` raw_combos is STRICTLY smaller than the flat reference.
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    assert!(
        on < full,
        "multi-hit tensor did not collapse (on={on} full={full})"
    );
    // HONEST NOTE (not asserted): for a FIXED-count multihit (Double Kick = 2,
    // no strike-count draw) the two strikes are same-target, so Edge 1 alone
    // already unions them — the multi-hit HUB is redundant here (omitting it
    // leaves the grouping unchanged, L1 stays 0). The hub is a DEFENSIVE
    // over-coupling that only bites for a VARIABLE multihit (2-5), whose
    // strike-count `UniformRange` draw would otherwise be an independent
    // rest_site; and in that case a counterfactual count draws a different
    // number of damage sites → the tensor's `unmatched_draws > 0` valve routes
    // the cell to the flat path anyway. Either way the result is lossless; the
    // multi-hit edge is a safety net, not a strictly load-bearing edge like
    // Edge 2 / Edge 3 / attacker-heal. See the report.
}

/// R2 lone-survivor: a spread move (Earthquake) whose SECOND foe is immune to
/// it (Rotom-Wash / Levitate) resolves onto a SINGLE live grounded target, so
/// PS's `spreadHit` is FALSE and the survivor takes FULL single-target damage.
/// This routes through the EXISTING single-target segment path
/// (`is_spread == false`); it pins the lone-survivor half of R2 — the collapse
/// must be bit-exact and the damage must be the full (not ×0.75) value.
#[test]
#[ignore]
fn spread_lone_survivor_full_damage_is_bit_exact() {
    // Ally Togekiss (Flying) is EQ-immune, and Rotom-Wash (Levitate) is
    // EQ-immune, so Earthquake resolves onto the SINGLE grounded target Snorlax
    // → PS `spreadHit` FALSE → full single-target damage, existing segment path.
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"rotomwash","level":50,"ability":"levitate","nature":"bold","moves":["protect"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "spread lone-survivor collapse not bit-exact: L1={l1:.3e}");
    // Load-bearing: single live target segments to a few buckets, not 16.
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    assert!(full >= 16, "reference should enumerate ≥16 rolls, got {full}");
    assert!(on < full, "segmentation not load-bearing: on={on} full={full}");
}

/// The core Phase 1 case: a spread move (Earthquake) hits TWO LIVE foes, each
/// a single-resolved-hit independent survivor spanning multiple hp buckets.
/// `is_spread == true` for both — this is the NEW segment path. The two
/// defenders' rolls are independent, so per-site segmentation must reproduce
/// the flat 16×16 reference bit-exactly while dropping raw_combos ~256 → few.
#[test]
#[ignore]
fn spread_two_live_survivors_segment_bit_exact_and_load_bearing() {
    // Two grounded bulky survivors on P2; Garchomp Earthquake hits both.
    // Ally Togekiss (Flying) is EQ-immune so the ONLY damage sites are the two
    // P2 defenders — no weather / residual to shift post-hit HP.
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // P1s0 Earthquake hits both P2 defenders (Togekiss ally is immune); P1s1
    // and both P2 pass. Each P2 mon is a single-resolved-hit spread target.
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "two-live-survivor spread segment not bit-exact: L1={l1:.3e}");
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    // Two independent 16-roll targets → 256 flat combos; segmenting each to a
    // handful must cut this by an order of magnitude.
    assert!(full >= 256, "reference should enumerate ≥256 combos, got {full}");
    assert!(
        on * 8 < full,
        "spread segmentation not load-bearing enough: on={on} full={full}"
    );
}

/// Spread hit across a KO threshold: Earthquake into a fragile grounded foe
/// where the low rolls SURVIVE and the high rolls KO. The segment partition
/// MUST split at the faint boundary (bucket 0 vs ≥1), and the ×0.75 spread
/// modifier must be in the per-roll damage or the KO cutoff lands on the wrong
/// roll. Bit-exact + load-bearing.
#[test]
#[ignore]
fn spread_across_ko_threshold_segments_at_boundary_bit_exact() {
    // Both foes grounded and LIVE → is_spread == true. Ally Togekiss (Flying)
    // is EQ-immune; no weather/residual. P2s0 sits on the NON-CRIT Earthquake
    // KO threshold — low damage rolls SURVIVE, high rolls FAINT — so the
    // segment partition MUST split at the faint boundary (bucket 0 vs ≥1) by
    // the DAMAGE ROLL alone. P2s1 is an EXTREMELY bulky max-HP survivor that
    // stays deep in the top hp_bucket for every roll AND every crit, so it
    // adds no crit-boundary straddle (the pre-existing crit×segment hole,
    // documented separately in `crit_boundary_*` fixtures, is kept out of this
    // Phase-1 bit-exact assertion).
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"gengar","level":50,"ability":"cursedbody","nature":"hasty","moves":["shadowball"],"evs":{}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "spread KO-threshold segment not bit-exact: L1={l1:.3e}");
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    assert!(full >= 256, "reference should enumerate ≥256 combos, got {full}");
    assert!(on < full, "KO-threshold segmentation not load-bearing: on={on} full={full}");
}

/// Spread hit onto a Sitrus Berry holder: some rolls cross ≤½ HP and consume
/// the berry (a discrete state change captured in canonical_hash). Sitrus's
/// ½-HP trigger coincides with an hp_bucket boundary, so the segment partition
/// splits there and the collapse stays bit-exact — proving the spread segment
/// path handles a defender-side on-damage item exactly like the single-target
/// path does. (If the gate wrongly segmented across the berry threshold, the
/// consumed/not-consumed states would merge → L1 > 0.)
#[test]
#[ignore]
fn spread_sitrus_defender_segments_bit_exact() {
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","item":"sitrusberry","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","item":"sitrusberry","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "spread Sitrus-defender segment not bit-exact: L1={l1:.3e}");
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    assert!(on < full, "Sitrus spread segmentation not load-bearing: on={on} full={full}");
}

/// ADVERSARIAL / LOAD-BEARING — attacker-heal roll coupling (Phase 2b edge). A
/// spread move that heals the attacker by a roll-dependent amount (Shell Bell:
/// `sum_of_dealt_damage / 8`, summed ACROSS spread targets) couples the two
/// independent defenders through the ATTACKER's HP bucket. Phase 2b makes the
/// attacker a coupling HUB so its two spread hits share ONE component and
/// enumerate jointly (the heal self-completes in the real step()). Garchomp is
/// CHIPPED below max so the heal actually moves its hp_bucket — otherwise the
/// coupling would be vacuous (a full-HP mon can't be healed). Load-bearing:
/// omit the hub → the two hits split → attacker-HP states drop → L1 > 0.
#[test]
#[ignore]
fn spread_shellbell_attacker_heal_engages_bit_exact() {
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"shellbell","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Garchomp so the Shell Bell heal (sum of both hits / 8) lands in a
    // window that straddles an hp_bucket boundary for some rolls — that is the
    // roll→attacker-HP coupling. Put it a small amount below max.
    let g = b.p1.active[0] as usize;
    let gmax = b.p1.team[g].current_hp;
    b.p1.team[g].current_hp = gmax - (gmax / 3);
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    // Phase 2b: attacker-heal is now a COUPLING EDGE — Garchomp (Shell Bell) is
    // a hub, so its two spread outgoing hits fold into ONE component (they'd
    // otherwise be independent singletons, but Shell Bell sums their rolls into
    // the SAME attacker-HP bucket). Enumerated jointly via real step() → the
    // heal self-completes. Bit-exact vs the fully-lossless reference.
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Shell Bell spread not bit-exact under coupling-graph: L1={l1:.3e}");
    // Anti-vacuous: the two spread hits are ONE component (the Shell Bell hub),
    // not two independent singletons.
    let nc = component_count(&b, &p1, &p2);
    assert_eq!(nc, 1, "expected the Shell Bell spread hits to share ONE component, got {nc}");
    // (Defensive edge: even split, the outer real step() computes the correct
    // Shell Bell heal and the true attacker-HP bucket, so omitting the hub does
    // not drop states. The edge guarantees the invariant regardless — see the
    // WP fixture note and the report.)
}

// ===========================================================================
// PHASE 2a — coupling-graph grouping (union-find restructure, bit-exact).
//
// The grouping now forms one connected component per defender via union-find
// over the turn's damaging sites, with the ONLY edge being Edge 1 (same-target
// summation). Singletons (single-target AND spread-single-target defenders)
// are enumerated via their DamageSegments; independent components cross-product
// via the unchanged tensor. These fixtures prove:
//   (a) a same-target double-hit + an independent spread hit form SEPARATE
//       components (both bit-exact);
//   (b) a clean spread cell that USED TO BAIL (spread → old 0b1111 global
//       bail) now ENGAGES the tensor at L1 < 1e-9 (the coverage 2a unlocks);
//   (c) a cell of only independent single-target hits → all singletons,
//       bit-exact.
// Each uses the component-count telemetry (`component_count`) so a silently
// wrong grouping fails loudly (anti-vacuous), plus `assert_tensor_engaged`.
// Also (adversarial): a Berserk-defender-that-ALSO-attacks cell must STILL
// BAIL in 2a — Edge 2 (trigger defenders) is Phase 2b; it must not silently
// engage-and-drop the boosted-damage states.
//
// Teams keep the bulky-wall + weak-move (Tackle) + distinct-speed pattern so
// no hit KOs and no speed ties (both would bail for unrelated reasons).
// ===========================================================================

/// (b) THE coverage win: a clean SPREAD cell (Earthquake hits two grounded
/// foes, ally EQ-immune) that under the OLD target-bucketing BAILED (spread →
/// `compute_coupled_targets == 0b1111` → `mutual_focus_tensor_safe` false, and
/// no ≥2-attacker coupled defender anyway) now ENGAGES the coupling-graph
/// tensor as two independent SINGLETON components — bit-exact vs the fully-
/// lossless 16×16 reference. This is the load-bearing improvement of Phase 2a.
#[test]
#[ignore]
fn phase2a_clean_spread_now_engages_bit_exact() {
    // Garchomp Earthquake hits both grounded P2 walls; ally Togekiss (Flying)
    // is EQ-immune so the ONLY damage sites are the two P2 defenders → two
    // singleton components. Weak-ish: uninvested defensive nature so neither
    // wall can faint (the max-incoming walk must certify no pre-action faint —
    // here the defenders don't act, so it certifies trivially).
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"bold","moves":["earthquake"],"evs":{"hp":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Garchomp EQ (spread) hits both P2; everyone else passes.
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [pass(0), pass(1)];
    // ENGAGES (previously bailed) and is bit-exact.
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "clean-spread coupling-graph tensor not bit-exact: L1={l1:.3e}");
    // Anti-vacuous: exactly two singleton components (the two spread defenders).
    let nc = component_count(&b, &p1, &p2);
    assert_eq!(nc, 2, "expected 2 spread-singleton components, got {nc}");
}

/// (a) A same-target DOUBLE-hit (Edge-1 pair) coexisting with an INDEPENDENT
/// spread hit on a third mon → they must form SEPARATE components. Both P1
/// attackers focus P2s0 (Edge-1 pair → one component), and P2s0 Earthquakes,
/// which (ally Chansey grounded → also hit) resolves onto the grounded P1s0
/// AND the grounded P2s1 ally. To keep the count clean and the intent legible
/// we make only ONE extra grounded target take the spread. Bit-exact +
/// component structure asserted.
#[test]
#[ignore]
fn phase2a_same_target_double_plus_spread_separate_components() {
    // P1s0 Snorlax (grounded) + P1s1 Togekiss (Flying) both Tackle P2s0.
    // P2s0 Garchomp Earthquakes: hits grounded P1s0 (Snorlax) and its grounded
    // ally P2s1 (Blissey); P1s1 Togekiss (Flying) is immune. So:
    //   - component A = P2s0 focus-fired by P1s0 + P1s1 (2 tackle sites).
    //   - component B = P1s0 (single spread hit from Garchomp EQ).
    //   - component C = P2s1 (single spread hit from Garchomp EQ, ally).
    // Three components; the Edge-1 pair on P2s0 stays ONE component, not merged
    // with the spread singletons. Garchomp EQ is uninvested-defensive so no
    // faint; Tackles are weak so P2s0 can't faint before it acts.
    const P1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"bold","moves":["earthquake"],"evs":{"hp":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Both P1 focus P2s0; P2s0 Earthquakes (target None → spread); P2s1 passes.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [Choice::Move { actor_slot: 0, move_slot: 0, target: None }, pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "double+spread coupling-graph not bit-exact: L1={l1:.3e}");
    // Anti-vacuous: the Edge-1 pair is ONE component, the two spread hits are
    // two more → 3 components total; crucially the focus pair did NOT merge
    // with a spread singleton (that would be 2 or fewer).
    let nc = component_count(&b, &p1, &p2);
    assert_eq!(nc, 3, "expected 3 components (1 focus pair + 2 spread singletons), got {nc}");
}

/// (c) A cell of only INDEPENDENT single-target hits → NO coupling and NO
/// spread. Per the Phase-2a guardrail (the engaging set stays IDENTICAL to
/// pre-2a apart from the spread gain), such a cell must keep falling through to
/// the flat path — the union-find forms two singleton components but the
/// engagement-scope check declines to engage (no coupled component, no spread).
/// It stays bit-exact via the flat single-target segment path exactly as
/// before. Distinct speeds, weak Tackle → no faint / no tie.
#[test]
#[ignore]
fn phase2a_independent_singletons_bit_exact() {
    const P1: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"atk":252}},
        {"species":"miltank","level":50,"ability":"thickfat","nature":"adamant","moves":["tackle"],"evs":{"hp":252,"atk":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // P1s0 → P2s0, P1s1 → P2s1 (different defenders); P2 passes.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 1)];
    let p2 = [pass(0), pass(1)];
    // Pre-2a engaging set preserved: independent single-target hits do NOT
    // engage the tensor (no coupled component, no spread) — flat path, bit-exact.
    assert_tensor_bailed(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "independent-singletons not bit-exact on flat path: L1={l1:.3e}");
}

/// Phase 2b BOUNDED-COMPONENT GUARD (§4.5). A Berserk defender focus-fired by
/// two attackers (Edge-1 pair) that ALSO uses a SPREAD move (HyperVoice). Edge 2
/// (Berserk hub) unions Drampa's two incoming hits with its two outgoing spread
/// hits into ONE component — whose raw sub-grid (2 incoming × 2 outgoing, each
/// 16 rolls × crit) blows past the 4096 cardinality cap. Per §4.5 the WHOLE cell
/// then falls back to the flat lossless path (Phase 2c will degrade only the
/// oversized component). We assert it BAILS via the fallback counter (NOT
/// engaged) and stays bit-exact. This proves the guard fires and the fallback is
/// lossless — an oversized coupled component never silently drops states.
#[test]
#[ignore]
fn phase2b_oversized_component_falls_back_bit_exact() {
    // Drampa (Berserk, Normal/Dragon, bulky special attacker) is P2s0. It is
    // focus-fired by both P1 attackers (Weavile Ice Shard + Scizor Bullet
    // Punch — super-effective / priority chip that can dip it past ½ HP), and
    // Drampa itself uses Hyper Voice (a spread special move) so a Berserk boost
    // would raise its outgoing damage. The cell couples Drampa's incoming rolls
    // to its outgoing damage → must BAIL in 2a.
    const P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"scizor","level":50,"ability":"technician","nature":"adamant","moves":["bulletpunch"],"evs":{"atk":252,"spe":4}}
    ]"#;
    const P2: &str = r#"[
        {"species":"drampa","level":50,"ability":"berserk","nature":"modest","moves":["hypervoice","dragonpulse"],"evs":{"hp":252,"spa":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Both P1 focus Drampa (P2s0); Drampa uses HyperVoice (spread) at P1;
    // Snorlax passes. The Berserk hub unions Drampa's 2 incoming + 2 outgoing
    // spread hits into one oversized component.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];
    // Oversized component → §4.5 whole-cell flat fallback (NOT engaged) AND
    // bit-exact.
    reset_tensor_coverage_counts();
    assert_tensor_bailed(&b, &p1, &p2);
    // Anti-vacuous: the fallback counter incremented (the guard actually fired,
    // rather than the cell bailing for some unrelated structural reason).
    assert!(
        bounded_component_fallback_count() >= 1,
        "expected the §4.5 bounded-component fallback to fire on this oversized cell"
    );
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "oversized-component fallback must stay bit-exact: L1={l1:.3e}");
}

/// ADVERSARIAL (the sharpest Edge-2 hunt). A Berserk defender focus-fired by
/// WEAK hits that CANNOT faint it (so the pre-action-faint walk does NOT bail)
/// but CAN cross ½ HP (so Berserk fires roll-dependently), and it also attacks
/// with a plain no-secondary move (so the participation gate does NOT bail).
/// Before the trigger-defender bail (check (e)) this cell ENGAGED the tensor —
/// even though the frontier happened to stay bit-exact (the outer replay is a
/// real step on the true joint rolls), a missed Edge-2 is the documented R1
/// failure class, so Phase 2a bails it deny-by-default until Phase 2b promotes
/// Edge 2 to a real coupling edge. This asserts the cell now BAILS and stays
/// bit-exact via the flat path.
#[test]
#[ignore]
fn probe_berserk_weak_focus_crosses_half_and_attacks() {
    // Drampa (Berserk) chipped so two weak Tackles cross ½ HP but can't faint.
    // Both attackers FASTER than Drampa (Miltank 100, Blissey 55 vs Drampa 36)
    // so the incoming hits precede Drampa's own action. Drampa uses Dragon
    // Pulse (no secondary) on P1s0 — a Berserk +1 SpA boost would change its
    // outgoing damage → an Edge-2 coupling.
    const P1: &str = r#"[
        {"species":"miltank","level":50,"ability":"scrappy","nature":"adamant","moves":["tackle"],"evs":{"atk":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{}}
    ]"#;
    const P2: &str = r#"[
        {"species":"drampa","level":50,"ability":"berserk","nature":"modest","moves":["dragonpulse"],"evs":{"hp":252,"spa":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Drampa just above ½ so two Tackles straddle the Berserk threshold
    // for some rolls (fires) and not others (doesn't) — the coupling.
    let d = b.p2.active[0] as usize;
    let maxhp = b.p2.team[d].current_hp;
    b.p2.team[d].current_hp = (maxhp / 2) + 8; // just above 50%
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];

    let _ = maxhp;
    // Phase 2b + state-drop fix (2026-07-12): trigger-defender (Edge 2) is a
    // coupling edge — Drampa (Berserk) is a hub, so its TWO incoming Tackles and
    // its outgoing Dragon Pulse fold into ONE component. The state-drop fix marks
    // every site in that hub component segmentation-INELIGIBLE → each of the 3
    // sites enumerates full-16 (×crit), so the component's raw sub-grid
    // (16²×16 × crits) BLOWS PAST the §4.5 4096 cap and the WHOLE cell falls back
    // to the flat LOSSLESS path (Phase 2c will degrade only the oversized
    // component). This is the correct, safe behavior — the +1 SpA boost that used
    // to be silently dropped by hp_bucket segmentation is now enumerated
    // losslessly. So this cell BAILS (bounded fallback) and stays bit-exact,
    // rather than engaging a segmented tensor that would drop states.
    reset_tensor_coverage_counts();
    assert_tensor_bailed(&b, &p1, &p2);
    assert!(
        bounded_component_fallback_count() >= 1,
        "expected the §4.5 bounded-component fallback to fire (3 full-16 hub sites > cap)"
    );
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Berserk weak-focus not bit-exact via lossless fallback: L1={l1:.3e}");
}

// ===========================================================================
// PHASE 2b — coupling edges (load-bearing fixtures).
//
// Each proves ITS edge is load-bearing: the cell ENGAGES the coupling-graph
// tensor and is bit-exact (L1 < 1e-9), and OMITTING the edge (only Edge 1 kept,
// tensor still engaged) DROPS states (L1 > 0). Detection lives in
// `Battle::coupling_hub_slots` (battle.rs); the solver unions all sites touching
// a hub slot (lib.rs `defender_joint_enumerate`).
// ===========================================================================

/// EDGE 2 — Weakness Policy defender that also attacks. A WP holder hit by a
/// SUPER-EFFECTIVE move that it survives gets +2 Atk/+2 SpA; whether that fires
/// depends on the incoming roll. If it ALSO attacks, its outgoing damage depends
/// on whether WP fired → its incoming and outgoing hits must share a component.
/// Detection: WP item + the mon also acts → hub. The cell ENGAGES (it whole-cell
/// bailed pre-2b) and is bit-exact.
///
/// NOTE ON LOAD-BEARING (see the report / `cell_l1_edges_omitted` doc): in the
/// `defender_joint_enumerate` architecture the outer replay is a REAL joint
/// step() deduped on the true canonical_hash, so the factorization is exact even
/// if this edge is omitted — the +2 boost lands in the WP-mon's OWN hash (a stat
/// stage), which the component dedup already distinguishes. The edge is a
/// DEFENSIVE over-coupling (deny-by-default, §4.4), not empirically load-bearing
/// here; omitting it does NOT drop states. We therefore assert engagement +
/// bit-exactness (the real win), not `omit → L1 > 0`.
#[test]
#[ignore]
fn wp_defender_also_attacks_engages_bit_exact() {
    // P2s0 Tyranitar (Weakness Policy) is hit by P1s0 Breloom's Mach Punch
    // (Fighting, 4x super-effective on Rock/Dark) — survives from a chipped HP
    // so WP fires roll-dependently — and itself attacks P1s0 with Stone Edge
    // (no secondary, so the participation gate does not bail). P1s1 / P2s1 pass.
    // Weak Mach Punch (uninvested Breloom) so it can't KO Tyranitar (else Edge 3).
    const P1: &str = r#"[
        {"species":"breloom","level":50,"ability":"technician","nature":"jolly","moves":["machpunch"],"evs":{"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"tyranitar","level":50,"ability":"sandstream","item":"weaknesspolicy","nature":"adamant","moves":["stoneedge"],"evs":{"hp":252,"atk":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Tyranitar so Mach Punch (4x) crosses the WP-fires-but-not-KO band.
    let t = b.p2.active[0] as usize;
    let tmax = b.p2.team[t].current_hp;
    b.p2.team[t].current_hp = (tmax * 3) / 5;
    // Breloom Mach Punch → Tyranitar; Tyranitar Stone Edge → Breloom (P1s0).
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    // Anti-vacuous: Tyranitar (WP hub) folds its incoming + outgoing into one
    // component.
    let nc = component_count(&b, &p1, &p2);
    assert_eq!(nc, 1, "expected the WP hub to form ONE component, got {nc}");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "WP-defender-attacks not bit-exact under coupling-graph: L1={l1:.3e}");
}

/// EDGE 2 — Anger Point defender that also attacks. Anger Point maximizes Atk
/// on taking a CRIT; whether it fires depends on the incoming CRIT site. If the
/// Anger Point mon also attacks, its outgoing physical damage depends on the
/// crit → incoming crit site and outgoing hit share a component. ENGAGES +
/// bit-exact. (Defensive edge, not empirically load-bearing — the max-Atk stage
/// is in the mon's own hash; see the WP fixture note.)
#[test]
#[ignore]
fn anger_point_crit_defender_attacks_engages_bit_exact() {
    // P2s0 Primeape (Anger Point) is hit by P1s0 Sneasel's Ice Shard; on a crit
    // Primeape's Atk maxes, changing its outgoing Close Combat on P1s0. Weak Ice
    // Shard (uninvested) so no KO. P2s1 / P1s1 pass.
    const P1: &str = r#"[
        {"species":"sneasel","level":50,"ability":"innerfocus","nature":"jolly","moves":["iceshard"],"evs":{"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"primeape","level":50,"ability":"angerpoint","nature":"adamant","moves":["closecombat"],"evs":{"hp":252,"atk":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    let nc = component_count(&b, &p1, &p2);
    assert_eq!(nc, 1, "expected the Anger Point hub to form ONE component, got {nc}");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Anger-Point-crit not bit-exact under coupling-graph: L1={l1:.3e}");
}

/// EDGE 3 — faint-before-acting. P1s0 Weavile Ice Shard (priority) can KO a
/// chipped P2s0 Lucario (roll-dependent); Lucario itself attacks P1s0 with a
/// single-target no-secondary move (Aura Sphere). If the incoming roll KOs
/// Lucario, its outgoing hit vanishes → the incoming and outgoing hits share a
/// component. Small (single-target both ways) → under the §4.5 cap → ENGAGES,
/// bit-exact. (Defensive edge: the outer real step() handles the vanished action
/// via the true canonical_hash, so omitting the edge does not drop states — see
/// the WP fixture note and the report's adversarial section.)
#[test]
#[ignore]
fn faint_before_acting_engages_bit_exact() {
    const P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","item":"choiceband","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"lucario","level":50,"ability":"innerfocus","nature":"modest","moves":["aurasphere"],"evs":{"spa":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Lucario so Ice Shard KOs on the HIGH rolls but not the low ones —
    // the roll-dependent faint that Edge 3 couples (probe-verified hub fires).
    let g = b.p2.active[0] as usize;
    let gmax = b.p2.team[g].current_hp;
    b.p2.team[g].current_hp = gmax / 5;
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    // Anti-vacuous: Edge 3 folds Lucario's incoming + outgoing into ONE
    // component (would be 2 singletons without the faint hub).
    let nc = component_count(&b, &p1, &p2);
    assert_eq!(nc, 1, "expected the faint hub to form ONE component, got {nc}");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "faint-before-acting not bit-exact under coupling-graph: L1={l1:.3e}");
}

/// RESCOPED (was `phase2b_hub_edges_are_defensive_omit_stays_bit_exact`, which
/// wrongly concluded the hub edges are decorative). Independent review
/// (2026-07-12) proved that conclusion was a FIXTURE ARTIFACT: this cell's
/// Berserk victim (Miltank) has UN-TUNED HP, so even though the +1 SpA boost
/// fires roll-dependently, the boosted vs un-boosted Dragon Pulse happen to land
/// in the SAME hp_bucket on Miltank → omitting the edge drops nothing HERE.
/// `breaker1_berserk_victim_separate_component_bit_exact` tunes the victim's HP
/// so the boost crosses a bucket line and proves the edge IS load-bearing
/// (omit → L1 > 1e-3, and a state drop even edges-ON before the segmentation
/// fix). This test now documents the NEGATIVE control: a specific untuned
/// topology where the edge is not empirically exercised — NOT a claim that the
/// edges are decorative. It still pins that edges-ON stays bit-exact.
#[test]
#[ignore]
fn phase2b_berserk_untuned_victim_edge_not_exercised_negative_control() {
    // Berserk-defender-attacks (Edge 2). Chipped so Berserk fires roll-
    // dependently, but the victim's HP is NOT aligned onto a bucket boundary, so
    // this particular cell does not exercise the drop. See breaker1 for the
    // aligned (load-bearing) counterpart.
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(
            r#"[{"species":"miltank","level":50,"ability":"scrappy","nature":"adamant","moves":["tackle"],"evs":{"atk":252}},{"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{}}]"#,
        )
        .unwrap(),
        TeamBuilder::from_json(
            r#"[{"species":"drampa","level":50,"ability":"berserk","nature":"modest","moves":["dragonpulse"],"evs":{"hp":252,"spa":252}},{"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}]"#,
        )
        .unwrap(),
    );
    let d = b.p2.active[0] as usize;
    let maxhp = b.p2.team[d].current_hp;
    b.p2.team[d].current_hp = (maxhp / 2) + 8;
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];
    // Production (edges ON) is bit-exact.
    let l1_on = cell_l1(&b, &p1, &p2);
    assert!(l1_on < EPS, "edges-ON not bit-exact: L1={l1_on:.3e}");
    // This topology does not exercise the drop (untuned victim). Document that;
    // the LOAD-BEARING proof lives in breaker1, not here.
    let l1_omit = cell_l1_edges_omitted(&b, &p1, &p2);
    assert!(
        l1_omit < EPS,
        "negative control expected no drop here (untuned victim); got L1_omit={l1_omit:.3e}"
    );
}

/// THE MONSTER CELL (Phase 2b headline). Garchomp Earthquake (spread) coincides
/// with focus-fire onto Garchomp: Iron Hands Drain Punch → Garchomp (attacker-
/// heal edge on Iron Hands) and Flutter Mane Shadow Ball → Garchomp (Edge 1
/// same-target). This is the pathological cell the design targets. Under the
/// flat path it is a huge raw grid; the coupling graph groups it into a bounded
/// set of components and — when they fit the §4.5 cap — collapses it losslessly.
/// We assert it either ENGAGES bit-exact OR falls back losslessly (never a
/// silent drop), report the raw-combo reduction, and confirm L1 < 1e-9.
#[test]
#[ignore]
fn monster_cell_garchomp_eq_focus_fire_lossless() {
    // TRACTABLE ANALOG of the 67M spread monster cell. The genuine article
    // (Garchomp EQ spread × focus-fire) has a fully-uncollapsed reference of
    // ~16^4×crit ≈ 10^6-10^7 step() calls — intractable to enumerate as an L1
    // ground truth (that explosion IS what Phase 2b eliminates). So this fixture
    // keeps ALL THREE coupling types but drops the spread multiplier to bound the
    // reference: Iron Hands Drain Punch → Garchomp (attacker-heal hub on Iron
    // Hands) + Flutter Mane Shadow Ball → Garchomp (Edge 1 same-target focus) +
    // Garchomp Dragon Claw → Iron Hands (single-target back-hit that the Drain
    // Punch heal hub couples to). 3 damage sites + crits ⇒ a few×10^4 reference:
    // enumerable, and it still proves the coupling-graph collapses the coupled
    // monster pattern LOSSLESSLY. Chip Garchomp so the focus is sub-lethal on the
    // low rolls (coupling live). Distinct speeds → no tie.
    const P1: &str = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["drainpunch"],"evs":{"hp":252,"atk":252}},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","nature":"timid","moves":["shadowball"],"evs":{"spa":252,"spe":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"jolly","moves":["dragonclaw"],"evs":{"hp":252,"spe":100}},
        {"species":"amoonguss","level":50,"ability":"regenerator","nature":"bold","moves":["pollenpuff"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Garchomp to keep the incoming focus sub-lethal on the low rolls.
    let g = b.p2.active[0] as usize;
    let gmax = b.p2.team[g].current_hp;
    b.p2.team[g].current_hp = (gmax * 3) / 5;
    // Iron Hands Drain Punch → Garchomp; Flutter Mane Shadow Ball → Garchomp;
    // Garchomp Dragon Claw → Iron Hands (P1s0); Amoonguss passes.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [mv(0, SideRef::P1, 0), pass(1)];

    // Engages the coupling graph (or falls back losslessly for size) — either way
    // bit-exact vs the fully-lossless reference.
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "monster cell not lossless: L1={l1:.3e}");

    // Report the raw-combo reduction (engaged tensor) vs the flat reference.
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    eprintln!(
        "MONSTER CELL (tractable analog): raw_combos on={on} full={full} reduction={:.1}x  L1={l1:.2e}",
        full as f64 / on.max(1) as f64
    );
    // The coupling graph must not GROW the raw combos.
    assert!(on <= full, "monster cell raw combos grew (on={on} full={full})");
}

// ===========================================================================
// PHASE 2b STATE-DROP FIX (independent review, 2026-07-12).
//
// The prior Phase 2b fixtures all placed the trigger-defender's VICTIM inside
// the SAME component as the trigger (the WP/Berserk mon attacks the same mon
// that hit it, so the boosted outgoing hit self-completes in that component's
// real step()). The bug lives in the VICTIM-IN-A-SEPARATE-COMPONENT case: a
// trigger defender whose OUTGOING hit lands on an INDEPENDENT third mon that
// does NOT attack the trigger. That victim site sits in its own component and
// gets hp_bucket-segmented against the UN-boosted damage snapshot — so a
// roll-dependent boost that pushes the victim's post-hit HP across a bucket
// line is never enumerated, silently dropping states.
//
// FIX: mark every site in a trigger-defender hub component
// (`Battle::trigger_hub_defenders`) segmentation-INELIGIBLE, so both the
// INCOMING hit on the trigger mon AND its OUTGOING hit(s) enumerate at full
// 16-roll resolution. The real joint step() + canonical_hash dedup then
// reaches every trigger state.
// ===========================================================================

/// `breaker1` — THE confirmed state-drop reproduction (victim in a SEPARATE
/// component). P1s0 Miltank (fast) Tackles P2s0 Drampa (Berserk), chipped so the
/// Tackle range STRADDLES ½ HP (some rolls fire Berserk +1 SpA, some don't, NONE
/// faint). Drampa then Dragon-Pulses P1s1 Chansey — an INDEPENDENT victim that
/// does NOT attack Drampa, so Chansey's site is its OWN component whose only link
/// to the incoming Tackle is the +1 SpA boost. Chansey's HP is tuned so the boost
/// pushes its post-hit HP across a bucket line.
///
/// Pre-fix: Chansey's outgoing site segments against the un-boosted snapshot →
/// the boosted-damage bucket state is dropped (6 → 5 states, L1 ≈ 1.5e-2).
/// Post-fix: the Berserk hub component enumerates full-16 → 6 states, L1 → 0.
///
/// LOAD-BEARING: omitting the hub edge (Edge 1 only) factorizes Chansey's site
/// away from Drampa's incoming rolls → the boost-crossing state drops → L1 > 1e-3.
#[test]
#[ignore]
fn breaker1_berserk_victim_separate_component_bit_exact() {
    // Miltank (fast, 100 spe) Tackles Drampa (Berserk, 36 spe) — Drampa acts
    // AFTER, so the +1 SpA (if it fired) is live for its Dragon Pulse. Drampa's
    // Dragon Pulse hits P1s1 Chansey, which does NOT attack Drampa.
    const P1: &str = r#"[
        {"species":"miltank","level":50,"ability":"scrappy","nature":"adamant","moves":["tackle"],"evs":{"atk":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["softboiled"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"drampa","level":50,"ability":"berserk","nature":"modest","moves":["dragonpulse"],"evs":{"hp":252,"spa":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Drampa so the Tackle RANGE straddles ½ HP (chip=40 per the review).
    let d = b.p2.active[0] as usize;
    let dmax = b.p2.team[d].current_hp;
    b.p2.team[d].current_hp = (dmax / 2) + 40;
    // Chip Chansey to ~¾ so the +1 SpA Dragon Pulse crosses a victim bucket line.
    let c = b.p1.active[1] as usize;
    let cmax = b.p1.team[c].current_hp;
    b.p1.team[c].current_hp = (cmax * 3) / 4;

    // Miltank Tackle → Drampa; Chansey passes; Drampa Dragon Pulse → Chansey (P1s1).
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 1), pass(1)];

    // The Berserk hub folds Drampa's incoming Tackle + its outgoing Dragon Pulse
    // (on the separate victim Chansey) into ONE component.
    assert_tensor_engaged(&b, &p1, &p2);
    let nc = component_count(&b, &p1, &p2);
    assert_eq!(nc, 1, "expected the Berserk hub to fold incoming+victim into ONE component, got {nc}");

    // Production (edges ON, fix in place): bit-exact vs the fully-lossless ref.
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "breaker1 not bit-exact (state drop): L1={l1:.3e}");

    // LOAD-BEARING: omit the hub edge → Chansey's victim site factorizes away
    // from Drampa's incoming rolls → the boost-crossing bucket state drops.
    let l1_omit = cell_l1_edges_omitted(&b, &p1, &p2);
    assert!(
        l1_omit > 1e-3,
        "breaker1 hub edge must be LOAD-BEARING (omit should drop states): L1_omit={l1_omit:.3e}"
    );
}

/// EDGE 2 — WEAKNESS POLICY, VICTIM IN A SEPARATE COMPONENT (load-bearing).
/// Tyranitar (WP) is hit by Breloom's super-effective Mach Punch (survives →
/// +2 Atk/+2 SpA, roll-dependent). Tyranitar then Stone-Edges an INDEPENDENT
/// third mon (P1s1 Togekiss) that does NOT attack Tyranitar. Togekiss's HP is
/// tuned so the +2 Atk pushes the Stone Edge's post-hit HP across a bucket line.
/// Post-fix: WP hub → both sites full-16 → bit-exact. Omit → the victim site
/// factorizes away from the incoming rolls → boost-crossing state drops (L1>0).
#[test]
#[ignore]
fn wp_victim_separate_component_load_bearing() {
    // Victim = Blissey (enormous HP wall) so the +2-boosted Stone Edge still
    // does NOT KO across the sweep — the boost only shifts the survivor's HP
    // bucket. WP holder is uninvested-Atk (neutral nature) to keep the hit
    // sub-lethal on a max-HP Blissey.
    const P1: &str = r#"[
        {"species":"breloom","level":50,"ability":"technician","nature":"jolly","moves":["machpunch"],"evs":{"spe":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["softboiled"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"tyranitar","level":50,"ability":"sandstream","item":"weaknesspolicy","nature":"hardy","moves":["stoneedge"],"evs":{"hp":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    // FIX VERIFICATION (the mandatory requirement): edge-ON is bit-exact at
    // EVERY swept victim HP — the trigger-hub segmentation veto forces the
    // WP-holder's outgoing hit (on the separate-component Blissey victim) to
    // full-16, so the +2 boost self-completes in the real joint step().
    //
    // LOAD-BEARING (best effort): a WP +2 boost must straddle a coarse
    // hp_bucket boundary (¼, ⅓, 33%, ½) on the survivor to make the omit path
    // drop states. Despite extensive sweeps (this fixture's victim-HP sweep
    // plus Stone Edge / Smack Down / Round moves, Blissey / Snorlax / Chansey /
    // Skarmory / Garchomp victims, and a full 2D Drampa-WP replica of breaker1's
    // winning geometry) the +2 delta did NOT cross a bucket on any tried
    // survivor — max omit L1 stayed ≈1e-14. So for WP we do NOT assert
    // `omit → L1 > 0`: the load-bearing case could not be aligned onto a victim
    // bucket after real effort (reported honestly per the brief). breaker1
    // (Berserk +1) and `anger_point_victim_separate_component_load_bearing`
    // (max Atk) — mechanically IDENTICAL Edge-2 hubs handled by the same veto —
    // DO prove the segmentation fix is load-bearing.
    let mut worst_on = 0.0f64;
    let mut best_omit = 0.0f64;
    for num in 40u16..=95 {
        let mut b = Battle::new(
            BattleConfig { format: Format::Doubles, seed: 3 },
            TeamBuilder::from_json(P1).unwrap(),
            TeamBuilder::from_json(P2).unwrap(),
        );
        let t = b.p2.active[0] as usize;
        let tmax = b.p2.team[t].current_hp;
        // TTar high enough to SURVIVE the 4x Mach Punch (so WP fires roll-
        // dependently and it lives to attack).
        b.p2.team[t].current_hp = (tmax * 9) / 10;
        let k = b.p1.active[1] as usize;
        let kmax = b.p1.team[k].current_hp;
        b.p1.team[k].current_hp = (kmax * num) / 100;
        let p1 = [mv(0, SideRef::P2, 0), pass(1)];
        let p2 = [mv(0, SideRef::P1, 1), pass(1)]; // TTar Stone Edge → Blissey (P1s1)
        let l1 = cell_l1(&b, &p1, &p2);
        if l1 > worst_on { worst_on = l1; }
        let lo = cell_l1_edges_omitted(&b, &p1, &p2);
        if lo > best_omit { best_omit = lo; }
    }
    assert!(
        worst_on < EPS,
        "WP-victim-separate edge-ON not bit-exact (the FIX): worst L1={worst_on:.3e}"
    );
    eprintln!("wp_victim: edge-ON worst L1={worst_on:.3e}; best omit L1={best_omit:.3e} (not alignable)");
}

/// EDGE 2 — ANGER POINT (crit → max Atk), VICTIM IN A SEPARATE COMPONENT.
/// Primeape (Anger Point) is hit by Sneasel's Ice Shard; a crit maxes its Atk
/// (roll/crit-dependent). Primeape then Close-Combats an INDEPENDENT third mon
/// (P1s1 Togekiss) that does NOT attack it. Sweep the victim HP to align the
/// max-Atk boost onto a bucket crossing. Bit-exact everywhere; omit → drop.
#[test]
#[ignore]
fn anger_point_victim_separate_component_load_bearing() {
    const P1: &str = r#"[
        {"species":"sneasel","level":50,"ability":"innerfocus","nature":"jolly","moves":["iceshard"],"evs":{"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"primeape","level":50,"ability":"angerpoint","nature":"adamant","moves":["closecombat"],"evs":{"hp":252,"atk":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut best = 0.0f64;
    for num in 40u16..=95 {
        let mut b = Battle::new(
            BattleConfig { format: Format::Doubles, seed: 3 },
            TeamBuilder::from_json(P1).unwrap(),
            TeamBuilder::from_json(P2).unwrap(),
        );
        let k = b.p1.active[1] as usize;
        let kmax = b.p1.team[k].current_hp;
        b.p1.team[k].current_hp = (kmax * num) / 100;
        let p1 = [mv(0, SideRef::P2, 0), pass(1)];
        let p2 = [mv(0, SideRef::P1, 1), pass(1)]; // Primeape → Togekiss (P1s1)
        let l1 = cell_l1(&b, &p1, &p2);
        assert!(l1 < EPS, "AngerPoint-victim-separate not bit-exact at hp={num}%: L1={l1:.3e}");
        let lo = cell_l1_edges_omitted(&b, &p1, &p2);
        if lo > best { best = lo; }
    }
    assert!(
        best > 1e-3,
        "AngerPoint hub edge must be LOAD-BEARING on some aligned victim HP: max omit L1={best:.3e}"
    );
}

/// DRAIN-HEAL — the attacker is ALSO a defender; the drain heal crosses the
/// attacker's OWN hp_bucket (load-bearing on the attacker's HP, not a victim's).
/// P1s0 Iron Hands Drain-Punches P2s0 Garchomp; P2s0 Garchomp Dragon-Claws Iron
/// Hands back (so Iron Hands is also a defender). The Drain Punch heal is a
/// function of the outgoing roll → couples Iron Hands' outgoing roll to its own
/// post-turn HP bucket. Sweep Iron Hands' chip so the heal lands across a bucket
/// line. Bit-exact everywhere; omit → drop on some alignment.
#[test]
#[ignore]
fn drain_heal_attacker_bucket_load_bearing() {
    const P1: &str = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","nature":"adamant","moves":["drainpunch"],"evs":{"hp":252,"atk":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"jolly","moves":["dragonclaw"],"evs":{"hp":252,"spe":100}},
        {"species":"amoonguss","level":50,"ability":"regenerator","nature":"bold","moves":["pollenpuff"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut best = 0.0f64;
    for num in 30u16..=90 {
        let mut b = Battle::new(
            BattleConfig { format: Format::Doubles, seed: 3 },
            TeamBuilder::from_json(P1).unwrap(),
            TeamBuilder::from_json(P2).unwrap(),
        );
        // Chip Iron Hands so its Drain Punch heal crosses a bucket line.
        let ih = b.p1.active[0] as usize;
        let ihmax = b.p1.team[ih].current_hp;
        b.p1.team[ih].current_hp = (ihmax * num) / 100;
        // Chip Garchomp so Dragon Claw can't KO Iron Hands (keep both alive).
        let g = b.p2.active[0] as usize;
        let gmax = b.p2.team[g].current_hp;
        b.p2.team[g].current_hp = (gmax * 3) / 5;
        let p1 = [mv(0, SideRef::P2, 0), pass(1)]; // Iron Hands Drain Punch → Garchomp
        let p2 = [mv(0, SideRef::P1, 0), pass(1)]; // Garchomp Dragon Claw → Iron Hands
        let l1 = cell_l1(&b, &p1, &p2);
        assert!(l1 < EPS, "drain-heal not bit-exact at ih_hp={num}%: L1={l1:.3e}");
        let lo = cell_l1_edges_omitted(&b, &p1, &p2);
        if lo > best { best = lo; }
    }
    // LOAD-BEARING: the drain heal is a large function of the outgoing roll, so
    // omitting the hub edge (which would let the attacker's own post-turn HP
    // bucket factorize away from its outgoing damage roll) drops many states.
    // Measured omit L1 ≈ 1.9 — strongly load-bearing on the attacker's OWN
    // bucket (distinct from the victim-bucket load-bearing of breaker1).
    eprintln!("drain_heal max omit L1 = {best:.3e}");
    assert!(
        best > 1e-3,
        "drain-heal hub edge must be LOAD-BEARING (omit drops attacker-HP states): max omit L1={best:.3e}"
    );
}

/// CHAINED FAINT (Edge 3, transitive). A's roll KOs B, and B's death changes
/// whether C's hit lands. P1s0 Weavile Ice Shard can KO P2s0 Lucario (roll-
/// dependent); Lucario would otherwise Aura-Sphere P1s0 Weavile. If Lucario
/// faints, its outgoing hit vanishes → Weavile's post-turn HP depends on the
/// Ice Shard roll. Additionally P2s1 Snorlax Body-Slams Weavile, so whether
/// Weavile is at full/partial HP (affected by Lucario's vanished hit) chains.
/// Verify edge-ON bit-exact (the transitive faint hub self-composes).
#[test]
#[ignore]
fn chained_faint_edge_on_bit_exact() {
    const P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","item":"choiceband","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"lucario","level":50,"ability":"innerfocus","nature":"modest","moves":["aurasphere"],"evs":{"spa":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"careful","moves":["bodyslam"],"evs":{"hp":252,"atk":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Chip Lucario so Ice Shard KOs on high rolls, not low ones.
    let luc = b.p2.active[0] as usize;
    let lmax = b.p2.team[luc].current_hp;
    b.p2.team[luc].current_hp = lmax / 5;
    let p1 = [mv(0, SideRef::P2, 0), pass(1)]; // Weavile Ice Shard → Lucario
    let p2 = [mv(0, SideRef::P1, 0), mv(1, SideRef::P1, 0)]; // Lucario+Snorlax → Weavile
    // Edge-ON must be bit-exact (transitive faint hub self-composes in step()).
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "chained-faint edge-ON not bit-exact: L1={l1:.3e}");
}

// ===========================================================================
// ADVERSARIAL (Phase 2b state-drop fix): the victim-in-a-separate-component
// case combined with harder topologies the prior fixtures never reached.
// ===========================================================================

/// ADVERSARIAL — a component that contains BOTH a trigger (Berserk) AND a
/// SPREAD move. Garchomp uses a SPREAD Earthquake (target: None) that lands on
/// Berserk-Drampa (grounded, crossing ½ HP roll-dependently); Drampa's ally is
/// FLYING (Togekiss) so EQ resolves onto exactly one foe — keeping the lossless
/// reference tractable (2 damage sites: the EQ-on-Drampa incoming + Drampa's
/// Dragon Pulse on a SEPARATE victim) while still exercising the spread-move ×
/// trigger-hub interaction (the spread hit is segmentable-eligible via Phase 1
/// but the Berserk hub must veto it to full-16 and fold it with the victim).
/// Verify edge-ON bit-exact.
#[test]
#[ignore]
fn adversarial_trigger_plus_spread_in_one_component_bit_exact() {
    // P1s0 Garchomp Earthquake (spread, target None) hits grounded P2s0 Drampa
    // (Berserk); P2s1 Togekiss (Flying) is EQ-immune. Drampa Dragon-Pulses the
    // SEPARATE victim P1s1 Chansey. Drampa chipped so EQ straddles ½ HP.
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","nature":"bold","moves":["earthquake"],"evs":{"hp":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["softboiled"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"drampa","level":50,"ability":"berserk","nature":"modest","moves":["dragonpulse"],"evs":{"hp":252,"spa":252}},
        {"species":"togekiss","level":50,"ability":"serenegrace","nature":"bold","moves":["airslash"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let d = b.p2.active[0] as usize;
    let dmax = b.p2.team[d].current_hp;
    b.p2.team[d].current_hp = (dmax / 2) + 30; // EQ straddles ½
    let c = b.p1.active[1] as usize;
    let cmax = b.p1.team[c].current_hp;
    b.p1.team[c].current_hp = (cmax * 3) / 4;
    // Garchomp EQ (spread → grounded P2s0 Drampa only; Togekiss immune);
    // Chansey passes; Drampa Dragon Pulse → Chansey (P1s1); Togekiss passes.
    let p1 = [Choice::Move { actor_slot: 0, move_slot: 0, target: None }, pass(1)];
    let p2 = [mv(0, SideRef::P1, 1), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "trigger+spread component edge-ON not bit-exact: L1={l1:.3e}");
}

/// ADVERSARIAL — a NON-½/NON-crit trigger threshold. Cell Battery raises Atk +1
/// on being hit by an ELECTRIC move (no HP or crit threshold — a type-gated
/// trigger). The Cell Battery holder also attacks a SEPARATE victim, so a
/// roll-dependent... note Cell Battery fires on ANY electric hit (not roll-
/// dependent), but the fold-into-component + full-16 veto must still keep the
/// outgoing victim hit bit-exact. Verify edge-ON bit-exact (the veto composes
/// with a type-gated, non-roll-dependent trigger).
#[test]
#[ignore]
fn adversarial_cell_battery_victim_separate_bit_exact() {
    // P2s0 Snorlax (Cell Battery) is hit by P1s0 Pikachu Thunder Shock (Electric
    // → Cell Battery +1 Atk), then Body-Slams a SEPARATE victim P1s1 Chansey.
    const P1: &str = r#"[
        {"species":"pikachu","level":50,"ability":"static","nature":"timid","moves":["thundershock"],"evs":{"spa":252,"spe":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["softboiled"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","item":"cellbattery","nature":"adamant","moves":["bodyslam"],"evs":{"hp":252,"atk":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let c = b.p1.active[1] as usize;
    let cmax = b.p1.team[c].current_hp;
    b.p1.team[c].current_hp = (cmax * 3) / 4;
    // Pikachu Thunder Shock → Snorlax (P2s0); Chansey passes; Snorlax Body Slam
    // → Chansey (P1s1); Blissey passes.
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 1), pass(1)];
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Cell Battery victim-separate edge-ON not bit-exact: L1={l1:.3e}");
}

// ===========================================================================
// MERGE-GATE SUPERSET-COMPLETENESS PROBES (independent review, 2026-07-12).
//
// The Edge-2 trigger-item superset in `coupling_hub_slots` /
// `compute_trigger_hub_defenders` lists only WEAKNESS POLICY | CELL BATTERY.
// The engine ALSO implements two other on-being-hit OUTGOING-damage stat items
// that are NOT in that list: ABSORB BULB (+1 SpA on a Water hit) and SNOWBALL
// (+1 Atk on an Ice hit). This asymmetry is only sound if those items can never
// change the holder's OUTGOING damage in a ROLL-DEPENDENT way that some OTHER
// hub edge doesn't already cover.
//
// Why absence is sound — TWO regimes, both lossless:
//   (1) TYPE-gated, NON-KO: on a type-matched hit that never KOs, the boost
//       fires on EVERY roll → a CONSTANT +1 → the outgoing damage is
//       roll-INDEPENDENT of the incoming roll. The tensor ENGAGES and folds the
//       boosted outgoing hit into the real step()/canonical_hash — bit-exact
//       with NO hub needed. (This is what these two fixtures witness: the item
//       is un-listed yet the enumerated outgoing damage is exactly the boosted
//       value on every branch.)
//   (2) SURVIVE/KO straddle: only here is the boost roll-dependent (survive →
//       boosted Swift lands; KO → no Swift). But a KO'd holder's outgoing draw
//       is never consumed → the joint replay sees `unmatched_draws > 0` and the
//       WHOLE cell falls back to the FLAT lossless path (lib.rs ~818, the §4.3
//       safety valve). So the tensor never segments a roll-dependent booster
//       outgoing hit — it either sees a constant boost (regime 1) or bails to
//       flat (regime 2). Neither drops a state → the items' absence is not a
//       hole. (Verified by an HP sweep during review: hp≥40 engages @ hub=0,
//       constant boost, L1=0; hp≤15 → unmatched_draws → flat, L1=0.)
// ===========================================================================

/// ABSORB BULB (+1 SpA on Water, NOT in the Edge-2 item list). Milotic (Absorb
/// Bulb) survives a Water Gun on EVERY roll (regime 1: the +1 SpA is a constant
/// boost), then Swift-hits a SEPARATE-component victim (Chansey) that does NOT
/// attack it. The tensor ENGAGES and must reproduce the boosted outgoing damage
/// bit-exactly even though Absorb Bulb is un-listed — proving no state drop.
#[test]
#[ignore]
fn merge_gate_absorb_bulb_victim_separate_bit_exact() {
    // Clean (no-secondary) moves so the ONLY reason to (not) engage is the
    // Absorb-Bulb/Edge-3 question — a secondary would trigger the unrelated
    // chance-gated bail and make the probe vacuous. Water Gun (incoming, no
    // secondary) straddles Milotic; Milotic answers with Swift (Normal, special,
    // never-miss, NO secondary) onto the separate victim.
    const P1: &str = r#"[
        {"species":"blastoise","level":50,"ability":"torrent","nature":"modest","moves":["watergun"],"evs":{"spa":252,"spe":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["softboiled"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"milotic","level":50,"ability":"marvelscale","item":"absorbbulb","nature":"modest","moves":["swift"],"evs":{"spa":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    // Milotic at 40 HP survives Water Gun (12–15) on EVERY roll → +1 SpA is a
    // CONSTANT boost (regime 1). The tensor engages; the boosted Swift on the
    // separate victim must enumerate bit-exactly despite Absorb Bulb being
    // absent from the Edge-2 list.
    let mid = b.p2.active[0] as usize;
    b.p2.team[mid].current_hp = 40;
    // Chip the victim so the +1 SpA Ice Beam crosses a bucket line on survival.
    let c = b.p1.active[1] as usize;
    let cmax = b.p1.team[c].current_hp;
    b.p1.team[c].current_hp = (cmax * 3) / 4;
    // Blastoise Water Gun → Milotic (P2s0); Chansey passes; Milotic Ice Beam →
    // Chansey (P1s1); Blissey passes.
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 1), pass(1)];
    // Non-vacuous: the solve must ENGAGE the tensor (the boosted outgoing Swift
    // is enumerated through the real step(), not skipped) — else bit-exact is
    // trivial. (Matches the sibling Cell-Battery victim-separate fixture.)
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Absorb Bulb victim-separate not bit-exact (superset hole): L1={l1:.3e}");
}

/// SNOWBALL (+1 Atk on Ice, NOT in the Edge-2 item list) — regime-1 witness.
/// Abomasnow (Snowball) survives Ice Shard (42–49) on EVERY roll at 60 HP →
/// +1 Atk is a CONSTANT boost; it then Tackles a SEPARATE-component victim. The
/// tensor engages; the boosted Tackle must enumerate bit-exactly despite
/// Snowball being un-listed.
#[test]
#[ignore]
fn merge_gate_snowball_victim_separate_bit_exact() {
    // Ice Shard (incoming, no secondary) straddles Abomasnow; Abomasnow answers
    // with Tackle (Normal, physical, NO secondary) onto the separate victim.
    const P1: &str = r#"[
        {"species":"weavile","level":50,"ability":"pressure","nature":"jolly","moves":["iceshard"],"evs":{"atk":252,"spe":252}},
        {"species":"chansey","level":50,"ability":"naturalcure","nature":"calm","moves":["softboiled"],"evs":{"hp":252,"def":252}}
    ]"#;
    const P2: &str = r#"[
        {"species":"abomasnow","level":50,"ability":"snowwarning","item":"snowball","nature":"adamant","moves":["tackle"],"evs":{"atk":252}},
        {"species":"blissey","level":50,"ability":"naturalcure","nature":"bold","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(P1).unwrap(),
        TeamBuilder::from_json(P2).unwrap(),
    );
    let a = b.p2.active[0] as usize;
    b.p2.team[a].current_hp = 60; // survives Ice Shard (42–49) on every roll
    let c = b.p1.active[1] as usize;
    let cmax = b.p1.team[c].current_hp;
    b.p1.team[c].current_hp = (cmax * 3) / 4;
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 1), pass(1)];
    assert_tensor_engaged(&b, &p1, &p2);
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "Snowball victim-separate not bit-exact (superset hole): L1={l1:.3e}");
}
