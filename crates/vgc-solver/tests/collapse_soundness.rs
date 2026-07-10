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
    set_ko_split_disabled, Battle, BattleConfig, Choice, Format, SideRef, Target, TeamBuilder,
};
use vgc_solver::{enumerate_outcomes_with, EnumerateOpts};

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
    set_ko_split_disabled(false);
    let on = dist(b, p1, p2);
    set_ko_split_disabled(true);
    let full = dist(b, p1, p2);
    set_ko_split_disabled(false);

    let mut keys: HashSet<u64> = on.keys().copied().collect();
    keys.extend(full.keys().copied());
    let mut l1 = 0.0;
    for k in &keys {
        l1 += (on.get(k).copied().unwrap_or(0.0) - full.get(k).copied().unwrap_or(0.0)).abs();
    }
    l1
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
