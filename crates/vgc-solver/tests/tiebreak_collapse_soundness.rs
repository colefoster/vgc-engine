//! Regression + soundness guard for the speed-tie commute collapse
//! (`order::tiebreak_commute_safe`, gated in `mark_tied_tiebreaks`).
//!
//! When a tied bracket provably commutes, the engine leaves
//! `DrawSpace::Tiebreak{speeds_tied:false}` so the solver enumerates ONE
//! ordering instead of `2^k`. It MUST reproduce the full `2^k`-ordering
//! frontier **bit-exactly** — same canonical states AND same probability mass
//! (L1 → 0). A false certify silently drops reachable states.
//!
//! `set_tiebreak_collapse_disabled(true)` forces the full `2^k` enumeration as
//! the ground-truth reference. `tiebreak_collapse_count()` gives the
//! anti-vacuous engagement check: the "collapses" cases must ENGAGE it and the
//! "bails" cases must NOT (else a bit-exact assertion is vacuous).
//!
//! Heavy cases (all four mons acting → full 2^4 × damage × crit reference) are
//! `#[ignore]`; run explicitly:
//!   cargo test -p vgc-solver --test tiebreak_collapse_soundness -- --ignored --nocapture

use std::collections::{HashMap, HashSet};

use vgc_engine_core::{
    set_tiebreak_collapse_disabled, tiebreak_collapse_count, Battle, BattleConfig, Choice, Format,
    SideRef, Target, TeamBuilder,
};
use vgc_solver::{enumerate_outcomes_with, EnumerateOpts};

const EPS: f64 = 1e-9;

fn opts() -> EnumerateOpts {
    EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: None }
}

fn dist(b: &Battle, p1: &[Choice], p2: &[Choice]) -> HashMap<u64, f64> {
    let f = enumerate_outcomes_with(b, p1, p2, 0xC0DE, opts());
    let mut m = HashMap::new();
    for o in &f.outcomes {
        *m.entry(o.hash).or_insert(0.0) += o.prob;
    }
    m
}

/// L1 between the collapse-ON frontier and the tiebreak-disabled (full 2^k)
/// reference at one cell. Must be ~0.
fn cell_l1(b: &Battle, p1: &[Choice], p2: &[Choice]) -> f64 {
    set_tiebreak_collapse_disabled(false);
    let on = dist(b, p1, p2);
    set_tiebreak_collapse_disabled(true);
    let full = dist(b, p1, p2);
    set_tiebreak_collapse_disabled(false);

    let mut keys: HashSet<u64> = on.keys().copied().collect();
    keys.extend(full.keys().copied());
    let mut l1 = 0.0;
    for k in &keys {
        l1 += (on.get(k).copied().unwrap_or(0.0) - full.get(k).copied().unwrap_or(0.0)).abs();
    }
    l1
}

/// How many tied brackets the gate collapsed while enumerating this cell with
/// the collapse ENABLED. `> 0` ⇒ the gate engaged (anti-vacuous).
fn collapse_delta(b: &Battle, p1: &[Choice], p2: &[Choice]) -> u64 {
    set_tiebreak_collapse_disabled(false);
    let before = tiebreak_collapse_count();
    let _ = dist(b, p1, p2);
    tiebreak_collapse_count() - before
}

fn mv(slot: u8, side: SideRef, tslot: u8) -> Choice {
    Choice::Move { actor_slot: slot, move_slot: 0, target: Some(Target { side, slot: tslot }) }
}
fn pass(slot: u8) -> Choice {
    Choice::Pass { actor_slot: slot }
}

fn battle(p1: &str, p2: &str) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed: 7 },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

// Two bulky, equal-speed Snorlax (Thick Fat = inert allowlisted, no item) with
// a weak no-secondary move (Tackle) — cannot KO, so a tied bracket of these
// provably commutes.
const SNORLAX_PAIR: &str = r#"[
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"relaxed","moves":["tackle"],"evs":{"hp":252,"def":252}},
    {"species":"snorlax","level":50,"ability":"thickfat","nature":"relaxed","moves":["tackle"],"evs":{"hp":252,"def":252}}
]"#;

/// COMMUTES: p1's two equal-speed Snorlax hit DISTINCT p2 targets, p2 passes.
/// No shared target, no item, inert ability, no possible KO → collapse engages
/// and the one-ordering frontier equals the full 2^k frontier.
#[test]
fn commuting_distinct_targets_collapses_bit_exact() {
    let b = battle(SNORLAX_PAIR, SNORLAX_PAIR);
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 1)];
    let p2 = [pass(0), pass(1)];
    assert!(collapse_delta(&b, &p1, &p2) > 0, "expected the tiebreak collapse to ENGAGE");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "commuting tiebreak collapse not bit-exact: L1={l1:.3e}");
}

/// BAILS — shared target. Both p1 mons focus the same p2 slot → coupled
/// defender (`compute_coupled_targets != 0`) → gate must NOT engage, and the
/// full path stays bit-exact.
#[test]
fn shared_target_bails() {
    let b = battle(SNORLAX_PAIR, SNORLAX_PAIR);
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 0)];
    let p2 = [pass(0), pass(1)];
    assert_eq!(collapse_delta(&b, &p1, &p2), 0, "shared-target cell must NOT collapse");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "shared-target bail not bit-exact: L1={l1:.3e}");
}

/// BAILS — held item. A Life Orb on one mon opens the whole item-hazard class,
/// so the item==none rule must forbid the collapse.
#[test]
fn held_item_bails() {
    const HELD: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","item":"lifeorb","nature":"relaxed","moves":["tackle"],"evs":{"hp":252,"def":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"relaxed","moves":["tackle"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = battle(HELD, SNORLAX_PAIR);
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 1)];
    let p2 = [pass(0), pass(1)];
    assert_eq!(collapse_delta(&b, &p1, &p2), 0, "held-item cell must NOT collapse");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "held-item bail not bit-exact: L1={l1:.3e}");
}

/// BAILS — move carries a secondary (Body Slam's paralysis) → a faster tied mon
/// could inflict para on a slower not-yet-acted one → gate must NOT engage.
#[test]
fn secondary_move_bails() {
    const BODYSLAM: &str = r#"[
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"relaxed","moves":["bodyslam"],"evs":{"hp":252,"def":252}},
        {"species":"snorlax","level":50,"ability":"thickfat","nature":"relaxed","moves":["bodyslam"],"evs":{"hp":252,"def":252}}
    ]"#;
    let b = battle(BODYSLAM, SNORLAX_PAIR);
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 1)];
    let p2 = [pass(0), pass(1)];
    assert_eq!(collapse_delta(&b, &p1, &p2), 0, "secondary-move cell must NOT collapse");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "secondary-move bail not bit-exact: L1={l1:.3e}");
}

/// BAILS — a hit can KO an acting defender (pre-action faint → order-ambiguous).
/// Frail equal-speed Staraptor mirror where Return can OHKO; all four act, so a
/// tied attacker can faint before its own action. Gate must NOT engage.
#[test]
#[ignore]
fn possible_ko_of_acting_defender_bails() {
    const STARAPTOR: &str = r#"[
        {"species":"staraptor","level":50,"ability":"reckless","nature":"adamant","moves":["return"],"evs":{"atk":252,"spe":252}},
        {"species":"staraptor","level":50,"ability":"reckless","nature":"adamant","moves":["return"],"evs":{"atk":252,"spe":252}}
    ]"#;
    let b = battle(STARAPTOR, STARAPTOR);
    // All four act, distinct cross-targets — but Return can OHKO a frail
    // Staraptor that also has a queued action → pre-action faint possible.
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 1)];
    let p2 = [mv(0, SideRef::P1, 0), mv(1, SideRef::P1, 1)];
    assert_eq!(collapse_delta(&b, &p1, &p2), 0, "possible-KO cell must NOT collapse");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "possible-KO bail not bit-exact: L1={l1:.3e}");
}

/// COMMUTES with ALL FOUR acting: bulky Snorlax mirror, distinct cross-targets,
/// no possible KO → the four-way tie collapses and stays bit-exact against the
/// full 2^4 reference.
#[test]
#[ignore]
fn four_way_commuting_collapses_bit_exact() {
    let b = battle(SNORLAX_PAIR, SNORLAX_PAIR);
    let p1 = [mv(0, SideRef::P2, 0), mv(1, SideRef::P2, 1)];
    let p2 = [mv(0, SideRef::P1, 0), mv(1, SideRef::P1, 1)];
    assert!(collapse_delta(&b, &p1, &p2) > 0, "four-way commuting tie must ENGAGE");
    let l1 = cell_l1(&b, &p1, &p2);
    assert!(l1 < EPS, "four-way commuting collapse not bit-exact: L1={l1:.3e}");
}
