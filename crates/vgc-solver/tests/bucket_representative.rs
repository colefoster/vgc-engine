//! A frontier outcome stands for every combo that lands in its
//! `canonical_hash` bucket, so its Battle must be the bucket's modal combo,
//! not whichever combo the enumeration order replayed first.
//!
//! Found by the solver harness (docs/solver_coverage_map.md): Dragonite
//! (35/166 HP, Multiscale off) takes Rillaboom's Fake Out. Real RNG leaves it
//! at 11-15 HP ~96% of the time and 1-5 HP on a crit (~4%); HP 1-15 share one
//! bucket. Index-order enumeration replays crit=true first, so the lossless
//! frontier carried HP 5 with 99% mass while the compressed (best-first)
//! frontier carried HP 15: the two solved the same exact 1v1 to -0.085 and
//! +0.766 (Extreme Speed's KO threshold sits inside the bucket).
use vgc_engine_core::{Battle, Choice, SideRef};
use vgc_solver::{enumerate_outcomes_compressed, enumerate_outcomes_with, joint_actions, ChanceCompression, EnumerateOpts};

fn fixture() -> Battle {
    let mut b: Battle = serde_json::from_str(include_str!("fixtures/bucket_rep_dragonite_1v1.json")).unwrap();
    b.rehydrate_caches();
    b
}

fn cell(b: &Battle) -> (Vec<Choice>, Vec<Choice>) {
    // P1 Fire Punch (move 0) vs P2 Fake Out (move 1).
    let r = joint_actions(b, SideRef::P1).into_iter().find(|j| matches!(j[0], Choice::Move { move_slot: 0, .. })).unwrap();
    let c = joint_actions(b, SideRef::P2).into_iter().find(|j| matches!(j[0], Choice::Move { move_slot: 1, .. })).unwrap();
    (r, c)
}

fn modal_hp(f: &vgc_solver::OutcomeFrontier) -> u16 {
    f.outcomes.iter().max_by(|a, b| a.prob.total_cmp(&b.prob)).unwrap().battle.p1.team[0].current_hp
}

#[test]
fn lossless_frontier_bucket_carries_modal_state() {
    let b = fixture();
    let (r, c) = cell(&b);
    for lossy in [false, true] {
        let f = enumerate_outcomes_with(&b, &r, &c, 0xC0_DE, EnumerateOpts { lossy_damage_3bucket: lossy, auto_lossy_damage_threshold: None });
        let hp = modal_hp(&f);
        assert!((11..=15).contains(&hp), "lossy={lossy}: modal outcome HP {hp}, real non-crit range 11-15");
    }
}

#[test]
fn compressed_and_lossless_agree_on_the_representative() {
    let b = fixture();
    let (r, c) = cell(&b);
    let (fc, _) = enumerate_outcomes_compressed(&b, &r, &c, 0xC0_DE, ChanceCompression { mass_eps: 0.0, max_combos: 1 << 20 });
    let fl = enumerate_outcomes_with(&b, &r, &c, 0xC0_DE, EnumerateOpts::default());
    assert!((11..=15).contains(&modal_hp(&fc)) && (11..=15).contains(&modal_hp(&fl)), "compressed {} lossless {}", modal_hp(&fc), modal_hp(&fl));
}
