//! Deterministic PRNG.
//!
//! Three variants share one draw-site API:
//!
//! - `Splitmix` — SplitMix64. Tiny (one u64 of state), fast, good
//!   distribution, well suited to the limited speed-tie / damage-roll
//!   / accuracy-roll usage in Phase 2. Phase 4 may swap to PCG64 if
//!   longer streams or jumpability become useful for parallel MCTS.
//! - `Oracle` — a pre-recorded queue of `RngEvent`s; strict. Used by
//!   unit tests where the next draw site is known: a method call
//!   panics if the next event's variant doesn't match. See PR-52.
//! - `OraclePartial` — same queue, but with a Splitmix fallback. When
//!   the queue is exhausted, OR the next event's variant doesn't match
//!   the requested draw, the call falls through to a Splitmix draw and
//!   the queue position does NOT advance. Used by the corpus harness:
//!   only the high-signal draws (crit + damage roll) are extracted
//!   from each replay; accuracy/secondary/speed-tie noise falls back
//!   to Splitmix, but mechanic divergence in the high-signal channels
//!   drops out of the diff. See `docs/plan` "Oracle RNG".

/// One pre-recorded RNG outcome. Each variant maps 1:1 to a draw-site
/// method (`Rng::range`, `Rng::damage_roll`, ...). The Oracle queue is
/// strictly typed: a method call must match the next event's variant
/// or it panics, surfacing the desync immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RngEvent {
    /// Tiebreak token from `next_u64` (speed-tie ordering).
    Tiebreak(u64),
    /// Uniform integer drawn via `range(n)`.
    Range(u32),
    /// Damage-roll bucket 0..=15.
    DamageRoll(u8),
    /// Damage-roll hint: target HP delta observed in the replay
    /// `|-damage|` event. The Rng can back-solve the matching 0..=15
    /// bucket given (`dmg_min`, `dmg_max`) supplied at draw time via
    /// `damage_roll_hint`. Consumed by `damage_roll_hint` only;
    /// `damage_roll` ignores it (falls through to its own branch logic).
    DamageHint(u16),
    /// Percent roll 1..=100.
    PercentRoll(u8),
    /// Crit hit/miss.
    Crit(bool),
}

#[derive(Debug, Clone)]
pub struct OracleState {
    events: Vec<RngEvent>,
    pos: usize,
}

impl OracleState {
    pub fn new(events: Vec<RngEvent>) -> Self {
        Self { events, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.events.len().saturating_sub(self.pos)
    }

    fn pop(&mut self) -> RngEvent {
        let i = self.pos;
        assert!(
            i < self.events.len(),
            "OracleRng underflow: requested event #{i} but queue holds {}",
            self.events.len()
        );
        self.pos += 1;
        self.events[i]
    }

    /// Peek the next event without advancing the position.
    fn peek(&self) -> Option<RngEvent> {
        self.events.get(self.pos).copied()
    }

    /// Conditional pop used by `OraclePartial`: if the next event
    /// matches the predicate, advance and return it; otherwise leave
    /// the position untouched and return `None`. Caller falls through
    /// to its Splitmix branch on `None`.
    fn pop_if(&mut self, matches: impl FnOnce(&RngEvent) -> bool) -> Option<RngEvent> {
        let e = self.peek()?;
        if matches(&e) {
            self.pos += 1;
            Some(e)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub enum Rng {
    Splitmix(u64),
    Oracle(OracleState),
    /// Partial oracle: pop only when the next event's variant matches
    /// the requested draw; otherwise the embedded Splitmix state takes
    /// over for this draw and the queue position is unchanged. Use
    /// this for the corpus harness when only some draw sites are
    /// recorded.
    OraclePartial { state: OracleState, fallback: u64 },
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // SplitMix64 tolerates seed = 0; no scrambling needed.
        Self::Splitmix(seed)
    }

    pub fn oracle(events: Vec<RngEvent>) -> Self {
        Self::Oracle(OracleState::new(events))
    }

    /// Partial-oracle constructor: replay the queued `events` only
    /// when the next event's variant matches the requested draw; on
    /// mismatch or queue exhaustion, fall through to a Splitmix draw
    /// seeded from `fallback_seed`. See module docs.
    pub fn oracle_partial(events: Vec<RngEvent>, fallback_seed: u64) -> Self {
        Self::OraclePartial {
            state: OracleState::new(events),
            fallback: fallback_seed,
        }
    }

    /// Advance a raw Splitmix64 state in place and return the drawn u64.
    /// Shared by all `OraclePartial` fallback arms below so the fallback
    /// stream is byte-identical to a same-seeded `Splitmix` Rng.
    fn splitmix_step(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_u64(&mut self) -> u64 {
        match self {
            Rng::Splitmix(state) => {
                *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = *state;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^ (z >> 31)
            }
            Rng::Oracle(state) => match state.pop() {
                RngEvent::Tiebreak(v) => v,
                other => panic!("OracleRng: expected Tiebreak, got {other:?}"),
            },
            Rng::OraclePartial { state, fallback } => {
                if let Some(RngEvent::Tiebreak(v)) =
                    state.pop_if(|e| matches!(e, RngEvent::Tiebreak(_)))
                {
                    v
                } else {
                    Self::splitmix_step(fallback)
                }
            }
        }
    }

    /// True/false uniformly.
    pub fn coin_flip(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }

    /// Uniform integer in `0..n`. `n == 0` returns 0.
    ///
    /// `n == 1` is a special case: the only possible return is 0, so PS
    /// elides the draw entirely (e.g. the `stall` volatile only calls
    /// `randomChance(1, counter)` when `counter > 1`). Oracle paths
    /// mirror that and skip popping; Splitmix still steps to keep
    /// stand-alone (non-oracle) battles deterministic w.r.t. existing
    /// tests that assume every site advances the PRNG.
    pub fn range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if n == 1 {
            match self {
                Rng::Splitmix(_) => {
                    let _ = self.next_u64();
                }
                Rng::Oracle(_) | Rng::OraclePartial { .. } => {
                    // PS doesn't draw at `randomChance(1, 1)` sites.
                }
            }
            return 0;
        }
        match self {
            Rng::Splitmix(_) => (self.next_u64() as u32) % n,
            Rng::Oracle(state) => match state.pop() {
                RngEvent::Range(v) => {
                    assert!(
                        v < n,
                        "OracleRng: Range event value {v} out of bounds for range({n})"
                    );
                    v
                }
                other => panic!("OracleRng: expected Range, got {other:?}"),
            },
            Rng::OraclePartial { state, fallback } => {
                // Require both variant match AND in-range value; an
                // out-of-range Range event falls through to splitmix
                // rather than panicking (it's almost certainly drift
                // from PS's draw site, not our intent).
                let matched = state.pop_if(|e| {
                    matches!(e, RngEvent::Range(v) if *v < n)
                });
                if let Some(RngEvent::Range(v)) = matched {
                    v
                } else {
                    (Self::splitmix_step(fallback) as u32) % n
                }
            }
        }
    }

    /// Back-solve a damage-roll bucket from an observed damage value.
    /// Given a candidate `(dmg_min, dmg_max)` range (the 16-bucket span
    /// from `damage::damage_range` for a specific attacker/defender/move)
    /// and the observed damage `target` taken from a replay
    /// `|-damage|` event, return the bucket `b ∈ 0..=15` that minimizes
    /// `|f(b) − target|` where `f(b) = dmg_min + (dmg_max − dmg_min) · b / 15`.
    ///
    /// PS computes `damage = base · (85 + roll) / 100` with integer
    /// truncation, so the per-bucket increment is constant within ±1.
    /// The linear-interp back-solve is exact for the mid-range buckets
    /// and rounds correctly at the edges.
    ///
    /// Returns `None` if the observed damage cannot plausibly have come
    /// from this candidate (target outside `[dmg_min − tol, dmg_max +
    /// tol]` where tol is one bucket width). Callers use a `None` to
    /// drop the candidate from a set-recon distribution.
    pub fn back_solve_damage_bucket(
        target: u16,
        dmg_min: u16,
        dmg_max: u16,
    ) -> Option<u8> {
        if dmg_max < dmg_min {
            return None;
        }
        let span = (dmg_max - dmg_min) as u32;
        let bucket_w = (span + 15) / 15; // round up — used for tolerance only
        let t = target as u32;
        let lo = (dmg_min as u32).saturating_sub(bucket_w);
        let hi = (dmg_max as u32).saturating_add(bucket_w);
        if t < lo || t > hi {
            return None;
        }
        if span == 0 {
            // Degenerate: all 16 buckets produce the same number (e.g.
            // 1 HP after rounding). Return bucket 0 if target matches,
            // else `None`.
            return if t == dmg_min as u32 { Some(0) } else { None };
        }
        // b = round((t - dmg_min) * 15 / span). Clamp to 0..=15.
        let delta = t.saturating_sub(dmg_min as u32);
        let num = delta.saturating_mul(15);
        let b = ((num + span / 2) / span).min(15) as u8;
        Some(b)
    }

    /// Check whether an observed `target` damage is plausibly inside the
    /// candidate `[dmg_min, dmg_max]` damage range (with one bucket of
    /// slack on each side, matching `back_solve_damage_bucket`). Used by
    /// the spread-recon observer to drop candidate sets whose damage
    /// span can't produce the observation.
    pub fn damage_range_contains(target: u16, dmg_min: u16, dmg_max: u16) -> bool {
        Self::back_solve_damage_bucket(target, dmg_min, dmg_max).is_some()
    }

    /// Damage roll bucket selected from a replay-observed `target` HP
    /// delta. On `Oracle` / `OraclePartial` variants, if the next event
    /// is a `DamageHint(observed)`, consume it and back-solve the
    /// matching bucket against `(dmg_min, dmg_max)`. On variant
    /// mismatch (or `Splitmix`), fall through to `damage_roll()`.
    ///
    /// Caller responsibility: pass the candidate engine's own damage
    /// range (via `damage::damage_range`) and the replay's observed
    /// damage. The Rng picks the closest bucket; if the observation is
    /// outside the engine's plausible window, falls back to a Splitmix
    /// draw rather than forcing an out-of-range bucket.
    pub fn damage_roll_hint(&mut self, dmg_min: u16, dmg_max: u16) -> u8 {
        match self {
            Rng::Splitmix(_) => self.damage_roll(),
            Rng::Oracle(state) => {
                if let Some(RngEvent::DamageHint(target)) = state.peek() {
                    state.pos += 1;
                    if let Some(b) = Self::back_solve_damage_bucket(target, dmg_min, dmg_max) {
                        return b;
                    }
                    // Out-of-range hint — degrade to mid-bucket. Strict
                    // Oracle never falls through to Splitmix, but an
                    // unsolvable hint is worth surfacing as the safe
                    // middle (rather than panicking).
                    return 7;
                }
                self.damage_roll()
            }
            Rng::OraclePartial { state, fallback } => {
                if let Some(RngEvent::DamageHint(target)) =
                    state.pop_if(|e| matches!(e, RngEvent::DamageHint(_)))
                {
                    if let Some(b) = Self::back_solve_damage_bucket(target, dmg_min, dmg_max) {
                        return b;
                    }
                    // Out-of-range: fall through to Splitmix.
                    return (Self::splitmix_step(fallback) & 0xF) as u8;
                }
                self.damage_roll()
            }
        }
    }

    /// Convenience: a damage roll bucket in `0..=15`.
    pub fn damage_roll(&mut self) -> u8 {
        match self {
            Rng::Splitmix(_) => (self.next_u64() & 0xF) as u8,
            Rng::Oracle(state) => match state.pop() {
                RngEvent::DamageRoll(v) => {
                    assert!(v < 16, "OracleRng: DamageRoll value {v} > 15");
                    v
                }
                other => panic!("OracleRng: expected DamageRoll, got {other:?}"),
            },
            Rng::OraclePartial { state, fallback } => {
                let matched = state.pop_if(|e| matches!(e, RngEvent::DamageRoll(_)));
                if let Some(RngEvent::DamageRoll(v)) = matched {
                    debug_assert!(v < 16);
                    v
                } else {
                    (Self::splitmix_step(fallback) & 0xF) as u8
                }
            }
        }
    }

    /// Convenience: percent roll (1..=100). PS uses 1..=100 inclusive.
    pub fn percent_1_100(&mut self) -> u8 {
        match self {
            Rng::Splitmix(_) => ((self.next_u64() % 100) as u8) + 1,
            Rng::Oracle(state) => match state.pop() {
                RngEvent::PercentRoll(v) => {
                    assert!((1..=100).contains(&v), "OracleRng: PercentRoll {v} out of 1..=100");
                    v
                }
                other => panic!("OracleRng: expected PercentRoll, got {other:?}"),
            },
            Rng::OraclePartial { state, fallback } => {
                let matched = state.pop_if(|e| matches!(e, RngEvent::PercentRoll(_)));
                if let Some(RngEvent::PercentRoll(v)) = matched {
                    debug_assert!((1..=100).contains(&v));
                    v
                } else {
                    ((Self::splitmix_step(fallback) % 100) as u8) + 1
                }
            }
        }
    }

    /// Crit hit/miss at a given crit stage. PS gen-9 table
    /// (`sim/battle.ts` `randomChance` / `data/conditions.ts`):
    ///   stage 0 → 1/24, 1 → 1/8, 2 → 1/2, 3+ → guaranteed crit.
    /// Caller is responsible for summing held item (Scope Lens +1,
    /// Razor Claw +1, Lucky Punch on Chansey +2, Stick on Farfetch'd
    /// +2), ability (Super Luck +1), volatile (Focus Energy / Dire
    /// Hit / Laser Focus → +2 or guaranteed), and move flag
    /// (high-crit-ratio +1). Oracle just replays its recorded outcome
    /// (the source sim already applied the stage).
    pub fn crit_with_stage(&mut self, stage: u8) -> bool {
        match self {
            Rng::Splitmix(_) => match stage {
                0 => self.range(24) == 0,
                1 => self.range(8) == 0,
                2 => self.range(2) == 0,
                _ => true,
            },
            Rng::Oracle(state) => match state.pop() {
                RngEvent::Crit(v) => v,
                other => panic!("OracleRng: expected Crit, got {other:?}"),
            },
            Rng::OraclePartial { state, fallback } => {
                let matched = state.pop_if(|e| matches!(e, RngEvent::Crit(_)));
                if let Some(RngEvent::Crit(v)) = matched {
                    v
                } else {
                    match stage {
                        0 => ((Self::splitmix_step(fallback) as u32) % 24) == 0,
                        1 => ((Self::splitmix_step(fallback) as u32) % 8) == 0,
                        2 => ((Self::splitmix_step(fallback) as u32) % 2) == 0,
                        _ => true,
                    }
                }
            }
        }
    }

    /// Stage-0 crit. Kept as a thin shim around `crit_with_stage(0)`
    /// for existing call sites; new code should prefer the staged form.
    pub fn crit(&mut self) -> bool {
        match self {
            Rng::Splitmix(_) => self.range(24) == 0,
            Rng::Oracle(state) => match state.pop() {
                RngEvent::Crit(v) => v,
                other => panic!("OracleRng: expected Crit, got {other:?}"),
            },
            Rng::OraclePartial { state, fallback } => {
                let matched = state.pop_if(|e| matches!(e, RngEvent::Crit(_)));
                if let Some(RngEvent::Crit(v)) = matched {
                    v
                } else {
                    // Mirror Splitmix's `range(24) == 0` exactly so the
                    // fallback stream stays consistent with a pure
                    // Splitmix Rng on a same-seeded run.
                    ((Self::splitmix_step(fallback) as u32) % 24) == 0
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_from_seed() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn distinct_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        // Astronomically unlikely to match for 100 draws if working.
        let mut differ = false;
        for _ in 0..100 {
            if a.next_u64() != b.next_u64() {
                differ = true;
                break;
            }
        }
        assert!(differ);
    }

    #[test]
    fn percent_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let p = r.percent_1_100();
            assert!((1..=100).contains(&p));
        }
    }

    #[test]
    fn oracle_replays_events_in_order() {
        let mut r = Rng::oracle(vec![
            RngEvent::Tiebreak(0xDEADBEEF),
            RngEvent::PercentRoll(73),
            RngEvent::Crit(true),
            RngEvent::DamageRoll(15),
            RngEvent::Range(2),
        ]);
        assert_eq!(r.next_u64(), 0xDEADBEEF);
        assert_eq!(r.percent_1_100(), 73);
        assert!(r.crit());
        assert_eq!(r.damage_roll(), 15);
        assert_eq!(r.range(4), 2);
    }

    #[test]
    fn oracle_remaining_tracks_position() {
        let mut r = Rng::oracle(vec![RngEvent::Crit(false), RngEvent::Crit(true)]);
        if let Rng::Oracle(s) = &r {
            assert_eq!(s.remaining(), 2);
        }
        assert!(!r.crit());
        if let Rng::Oracle(s) = &r {
            assert_eq!(s.remaining(), 1);
        }
        assert!(r.crit());
        if let Rng::Oracle(s) = &r {
            assert_eq!(s.remaining(), 0);
        }
    }

    #[test]
    #[should_panic(expected = "OracleRng: expected DamageRoll")]
    fn oracle_panics_on_type_mismatch() {
        let mut r = Rng::oracle(vec![RngEvent::Crit(true)]);
        let _ = r.damage_roll();
    }

    #[test]
    #[should_panic(expected = "OracleRng underflow")]
    fn oracle_panics_on_underflow() {
        let mut r = Rng::oracle(vec![]);
        let _ = r.crit();
    }

    #[test]
    fn oracle_range_zero_short_circuits() {
        // n == 0 must not consume an event.
        let mut r = Rng::oracle(vec![RngEvent::Crit(true)]);
        assert_eq!(r.range(0), 0);
        assert!(r.crit());
    }

    #[test]
    fn oracle_partial_pops_on_variant_match() {
        let mut r = Rng::oracle_partial(
            vec![RngEvent::Crit(true), RngEvent::DamageRoll(7)],
            42,
        );
        assert!(r.crit());
        assert_eq!(r.damage_roll(), 7);
    }

    #[test]
    fn oracle_partial_falls_through_on_variant_mismatch() {
        // Queue has a Crit next, but caller asks for a damage_roll →
        // queue position is unchanged and Splitmix supplies the draw.
        let mut r = Rng::oracle_partial(vec![RngEvent::Crit(true)], 42);
        // First call: damage_roll() must NOT consume the Crit.
        let d = r.damage_roll();
        assert!(d < 16);
        // The queued Crit is still there and is the next satisfying call.
        assert!(r.crit());
    }

    #[test]
    fn oracle_partial_falls_through_on_underflow() {
        let mut r = Rng::oracle_partial(vec![], 42);
        // No panic on empty queue — falls through to Splitmix.
        let _ = r.crit();
        let _ = r.damage_roll();
        let _ = r.percent_1_100();
        let _ = r.range(10);
    }

    #[test]
    fn oracle_partial_fallback_matches_pure_splitmix() {
        // With an empty queue, OraclePartial(seed) should produce the
        // exact same stream as Splitmix(seed). Regression guard
        // against `splitmix_step` drifting from `next_u64`.
        let mut partial = Rng::oracle_partial(vec![], 0x1234_5678);
        let mut pure = Rng::new(0x1234_5678);
        for _ in 0..50 {
            assert_eq!(partial.damage_roll(), pure.damage_roll());
            assert_eq!(partial.percent_1_100(), pure.percent_1_100());
            assert_eq!(partial.crit(), pure.crit());
            assert_eq!(partial.range(37), pure.range(37));
        }
    }

    #[test]
    fn oracle_partial_range_out_of_bounds_falls_through() {
        // A Range event whose value is too large for the requested n
        // shouldn't panic — it falls through to Splitmix and stays
        // queued for a later draw with a larger n.
        let mut r = Rng::oracle_partial(vec![RngEvent::Range(8)], 99);
        let v = r.range(4); // 8 ≥ 4 → falls through
        assert!(v < 4);
        // The 8 is still there for a wider range.
        assert_eq!(r.range(16), 8);
    }

    #[test]
    fn crit_stage_3_always_crits() {
        let mut r = Rng::new(0xABCD);
        for _ in 0..1000 {
            assert!(r.crit_with_stage(3));
        }
    }

    #[test]
    fn crit_stage_1_rate_is_one_eighth() {
        let mut r = Rng::new(0xCAFE);
        let mut crits = 0u32;
        let trials = 80_000u32;
        for _ in 0..trials {
            if r.crit_with_stage(1) {
                crits += 1;
            }
        }
        let rate = crits as f64 / trials as f64;
        assert!(
            (rate - 0.125).abs() < 0.01,
            "stage-1 crit rate {rate} too far from 1/8"
        );
    }

    #[test]
    fn back_solve_picks_correct_bucket_at_endpoints() {
        // dmg_min=85, dmg_max=100 (per PS roll formula at base=100).
        // target=85 → bucket 0; target=100 → bucket 15.
        assert_eq!(Rng::back_solve_damage_bucket(85, 85, 100), Some(0));
        assert_eq!(Rng::back_solve_damage_bucket(100, 85, 100), Some(15));
    }

    #[test]
    fn back_solve_picks_midpoint() {
        // Halfway between 85 and 100 = 92.5 → bucket 7 or 8.
        let b = Rng::back_solve_damage_bucket(93, 85, 100).unwrap();
        assert!(b == 7 || b == 8, "bucket near midpoint, got {b}");
    }

    #[test]
    fn back_solve_returns_none_for_out_of_range() {
        // target way above max → not from this candidate.
        assert_eq!(Rng::back_solve_damage_bucket(500, 100, 120), None);
        assert_eq!(Rng::back_solve_damage_bucket(10, 100, 120), None);
    }

    #[test]
    fn back_solve_allows_one_bucket_slack() {
        // dmg_min=100, dmg_max=115 → bucket width 1. target=99 (1 below)
        // is still plausible (rounding).
        assert!(Rng::back_solve_damage_bucket(99, 100, 115).is_some());
        // target=116 (1 above) also plausible.
        assert!(Rng::back_solve_damage_bucket(116, 100, 115).is_some());
        // target=120 (5 above with 1-bucket width 1) — out of range.
        assert_eq!(Rng::back_solve_damage_bucket(120, 100, 115), None);
    }

    #[test]
    fn damage_range_contains_matches_back_solve() {
        assert!(Rng::damage_range_contains(92, 85, 100));
        assert!(!Rng::damage_range_contains(500, 85, 100));
    }

    #[test]
    fn back_solve_handles_degenerate_zero_span() {
        // Both ends equal — common for 1-HP hits.
        assert_eq!(Rng::back_solve_damage_bucket(1, 1, 1), Some(0));
        assert_eq!(Rng::back_solve_damage_bucket(2, 1, 1), None);
    }

    #[test]
    fn back_solve_realistic_garchomp_eq_into_pikachu() {
        // From damage::tests::garchomp_earthquake_vs_pikachu_max_roll
        // baseline: max-roll EQ deals ~250 ish. Use a plausible span and
        // assert mid-range damage maps inside the bucket grid.
        let dmin = 234; // approximate min roll
        let dmax = 276; // approximate max roll
        let target = 255; // mid-ish observed
        let b = Rng::back_solve_damage_bucket(target, dmin, dmax).unwrap();
        assert!(b <= 15);
        // Bucket should land near the middle.
        assert!((6..=9).contains(&b), "expected near mid bucket, got {b}");
    }

    #[test]
    fn damage_roll_hint_oracle_back_solves_to_bucket() {
        // Hint queue: replay observed 92 damage; engine candidate range
        // 85..=100 → midpoint bucket.
        let mut r = Rng::oracle(vec![RngEvent::DamageHint(92)]);
        let b = r.damage_roll_hint(85, 100);
        assert!(b == 7 || b == 8, "expected mid bucket, got {b}");
    }

    #[test]
    fn damage_roll_hint_partial_falls_through_on_mismatch() {
        // Queue has a Crit; damage_roll_hint should not consume it and
        // should fall through to a Splitmix bucket.
        let mut r = Rng::oracle_partial(vec![RngEvent::Crit(true)], 42);
        let b = r.damage_roll_hint(85, 100);
        assert!(b < 16);
        // Crit is still there.
        assert!(r.crit());
    }

    #[test]
    fn damage_roll_hint_oracle_out_of_range_returns_safe_middle() {
        // Hint says 500 dmg, but engine range is 85..=100 → unsolvable.
        // Strict Oracle returns bucket 7 (mid) rather than panicking.
        let mut r = Rng::oracle(vec![RngEvent::DamageHint(500)]);
        assert_eq!(r.damage_roll_hint(85, 100), 7);
    }

    #[test]
    fn damage_roll_hint_splitmix_acts_like_damage_roll() {
        // Splitmix has no hints to consume; hint method = damage_roll.
        let mut r1 = Rng::new(123);
        let mut r2 = Rng::new(123);
        for _ in 0..20 {
            assert_eq!(r1.damage_roll(), r2.damage_roll_hint(85, 100));
        }
    }

    #[test]
    fn range_n1_does_not_consume_oracle_queue() {
        // PS elides `randomChance(1, 1)` — it never draws when the only
        // outcome is 0. Oracle paths must mirror that so subsequent
        // `range(n)` calls see the queue PS actually populated.
        let events = vec![RngEvent::Range(2)];
        let mut r = Rng::oracle_partial(events, 0);
        // First call: range(1) → 0 without popping.
        assert_eq!(r.range(1), 0);
        // Second call: range(3) pops the queued Range(2).
        assert_eq!(r.range(3), 2);
    }

    #[test]
    fn range_n1_splitmix_still_advances_state() {
        // Splitmix battles (no oracle) advance the PRNG on every range
        // site so stand-alone test seeds stay deterministic.
        let mut r1 = Rng::new(42);
        let _ = r1.range(1);
        let after_range1 = r1.next_u64();
        let mut r2 = Rng::new(42);
        let _ = r2.next_u64(); // simulate the range(1) bump
        assert_eq!(r2.next_u64(), after_range1);
    }

    #[test]
    fn splitmix_crit_base_rate() {
        // 1/24 base crit rate over many trials, ±2% tolerance.
        let mut r = Rng::new(0x1234);
        let mut crits = 0u32;
        let trials = 100_000u32;
        for _ in 0..trials {
            if r.crit() {
                crits += 1;
            }
        }
        let rate = crits as f64 / trials as f64;
        let expected = 1.0 / 24.0;
        assert!(
            (rate - expected).abs() < 0.01,
            "crit rate {rate} too far from 1/24"
        );
    }
}
