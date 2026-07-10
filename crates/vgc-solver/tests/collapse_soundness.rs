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
    set_ko_split_disabled, Battle, BattleConfig, Choice, Format, SideRef, Status, Target,
    TeamBuilder,
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
