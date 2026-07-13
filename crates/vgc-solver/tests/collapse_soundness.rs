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
    enumerate_outcomes_with, set_joint_collapse_disabled, take_joint_collapse_engaged,
    EnumerateOpts,
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
    // Multi-hit is un-segmented (bail path) → bit-exact by full enumeration.
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "multi-hit + crit not bit-exact: L1={l1:.3e}");
    // Anti-vacuous: multi-hit must NOT segment (raw_combos unchanged vs full).
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    assert_eq!(
        on, full,
        "multi-hit unexpectedly segmented (on={on} full={full}) — the crit \
         refinement must not leak into the multi-hit path"
    );
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

/// ADVERSARIAL — attacker-heal roll coupling. A spread move that heals the
/// attacker by a roll-dependent amount (Shell Bell: `sum_of_dealt_damage / 8`,
/// summed ACROSS spread targets) couples the two independent defenders through
/// the ATTACKER's HP bucket, which the per-defender segment partition does NOT
/// key on. The gate must BAIL (drain / Shell Bell exclusion) → full-16 → still
/// bit-exact. Without the `attacker_heal_roll_coupled` veto this drops
/// attacker-HP states (L1 > 0).
#[test]
#[ignore]
fn spread_shellbell_attacker_heal_bails_bit_exact() {
    // Garchomp holds Shell Bell; Earthquake hits both live foes and heals
    // Garchomp by (dmg0+dmg1)/8. Garchomp must be below max HP so the heal
    // moves its hp_bucket (start it chipped via a self-inflicted setup is
    // hard here; instead rely on the reference/collapse agreeing — the veto
    // forces full-16 so they agree by construction, and we assert the veto
    // actually engaged by checking raw_combos did NOT collapse).
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"shellbell","nature":"adamant","moves":["earthquake"],"evs":{"atk":252,"spe":252}},
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
    assert!(l1 < EPS, "Shell Bell spread must stay bit-exact under bail: L1={l1:.3e}");
    // Anti-vacuous: the attacker-heal veto must have kept us on the FULL path,
    // so raw_combos equals the flat reference (no segmentation collapse).
    let (on, full) = (raw_combos_on(&b, &p1, &p2), raw_combos_full(&b, &p1, &p2));
    assert_eq!(
        on, full,
        "Shell Bell attacker-heal veto did NOT engage: on={on} full={full} (segmentation leaked)"
    );
}

/// REVERTED-BEHAVIOR GUARD (supersedes the removed Phase-2a/2b coupling-graph
/// `breaker1_*` fixtures). The coupling graph tried to collapse cells where a
/// spread-move DEFENDER also acts this turn (a "trigger defender that attacks").
/// That machinery is reverted as a net regression (no speedup; the monster cell
/// is irreducible). The pre-2a behavior — and the behavior we restore — is the
/// whole-cell bail: when a spread move is present the mutual-focus joint tensor
/// sees `compute_coupled_targets == 0b1111` and BAILS, handing the joint
/// dimension to the flat enumeration path, which is always fully lossless.
///
/// Scenario: Garchomp Earthquake hits BOTH live P2 mons (spread), and BOTH P2
/// mons also attack this turn (Snorlax + Blissey Tackle into P1). So each
/// spread-defender is ALSO a same-turn actor — exactly the coupled-defender
/// shape the reverted graph targeted.
///
/// This fixture pins the JOINT-tensor dimension (with the independent
/// per-defender ko_split segment collapse held OFF via `set_ko_split_disabled`).
/// Post-revert the tensor must BAIL (`assert_tensor_bailed`) → the joint
/// dimension is bit-exact vs the fully-lossless reference. That is the exact
/// soundness property the coupling graph was supposed to buy and now doesn't:
/// the flat bail path loses nothing.
///
/// NB: the ORTHOGONAL Phase-1 per-defender spread-segment collapse (ko_split)
/// is deliberately NOT under test here. On THIS coupled cell it is not
/// bit-exact — a defender's own attack depends on whether it survived the
/// spread hit — but that condition PRE-DATES this initiative (present identically
/// on `a160893`, and never fixed by the coupling graph either), so it is out of
/// scope for the revert. It is exercised in isolation by the `spread_*_segment`
/// fixtures on independent (non-acting) defenders.
#[test]
#[ignore]
fn spread_trigger_defender_that_attacks_tensor_bails_bit_exact() {
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
    // P1s0 Earthquake hits both grounded P2 mons (Togekiss ally is EQ-immune).
    // Both P2 mons ALSO attack (Tackle into P1s0) — spread-defenders AND
    // same-turn actors: the coupled-defender-that-attacks shape.
    let p1 = [mv(0, SideRef::P2, 0), pass(1)];
    let p2 = [mv(0, SideRef::P1, 0), mv(1, SideRef::P1, 0)];

    // A spread move present forces the whole-cell joint bail
    // (compute_coupled_targets == 0b1111): the mutual-focus tensor must NOT
    // engage.
    assert_tensor_bailed(&b, &p1, &p2);

    // Isolate the joint dimension: with per-defender ko_split segmentation OFF,
    // the joint tensor's bail must reproduce the fully-lossless frontier
    // bit-exactly (the flat path loses nothing).
    set_ko_split_disabled(true);
    set_joint_collapse_disabled(false);
    let joint_only = dist(&b, &p1, &p2);
    set_ko_split_disabled(true);
    set_joint_collapse_disabled(true);
    let full = dist(&b, &p1, &p2);
    set_ko_split_disabled(false);
    set_joint_collapse_disabled(false);
    let mut keys: HashSet<u64> = joint_only.keys().copied().collect();
    keys.extend(full.keys().copied());
    let l1: f64 = keys
        .iter()
        .map(|k| {
            (joint_only.get(k).copied().unwrap_or(0.0) - full.get(k).copied().unwrap_or(0.0)).abs()
        })
        .sum();
    assert!(
        l1 < EPS,
        "trigger-defender-that-attacks joint bail must be bit-exact vs flat: L1={l1:.3e}"
    );
}
