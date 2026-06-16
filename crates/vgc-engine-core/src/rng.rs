//! Deterministic PRNG.
//!
//! Two variants share one draw-site API:
//!
//! - `Splitmix` — SplitMix64. Tiny (one u64 of state), fast, good
//!   distribution, well suited to the limited speed-tie / damage-roll
//!   / accuracy-roll usage in Phase 2. Phase 4 may swap to PCG64 if
//!   longer streams or jumpability become useful for parallel MCTS.
//! - `Oracle` — a pre-recorded queue of `RngEvent`s. Used by the
//!   corpus differential harness to replace the engine's rolls with
//!   outcomes captured from a Pokémon Showdown run of the same action
//!   sequence (damage roll, crit flag, accuracy, secondary fire,
//!   percent rolls, speed-tie tiebreak). Isolates mechanic divergence
//!   from PRNG divergence. See `docs/plan` "Oracle RNG".

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
}

#[derive(Debug, Clone)]
pub enum Rng {
    Splitmix(u64),
    Oracle(OracleState),
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        // SplitMix64 tolerates seed = 0; no scrambling needed.
        Self::Splitmix(seed)
    }

    pub fn oracle(events: Vec<RngEvent>) -> Self {
        Self::Oracle(OracleState::new(events))
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
        }
    }

    /// True/false uniformly.
    pub fn coin_flip(&mut self) -> bool {
        (self.next_u64() & 1) == 1
    }

    /// Uniform integer in `0..n`. `n == 0` returns 0.
    pub fn range(&mut self, n: u32) -> u32 {
        if n == 0 {
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
        }
    }

    /// Crit hit/miss. Splitmix uses gen-9 base 1/24; Oracle replays
    /// the recorded outcome (which already encodes ability/item/
    /// high-crit-ratio adjustments from the source sim).
    pub fn crit(&mut self) -> bool {
        match self {
            Rng::Splitmix(_) => self.range(24) == 0,
            Rng::Oracle(state) => match state.pop() {
                RngEvent::Crit(v) => v,
                other => panic!("OracleRng: expected Crit, got {other:?}"),
            },
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
