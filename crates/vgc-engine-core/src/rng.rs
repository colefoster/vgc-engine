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
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// A compact slot reference shared by the keyed oracle. `0..=3` map to
/// `p1a, p1b, p2a, p2b` (i.e. `side * 2 + slot`); `NO_SLOT` (0xFF) is the
/// sentinel for "self / field / no specific target".
pub type SlotRef = u8;
/// Sentinel slot ref — self-targeting / field draws with no attributable target.
pub const NO_SLOT: SlotRef = 0xFF;

/// The kind of decision a draw resolves. Used as part of the keyed
/// oracle's lookup key so that the engine and the PS driver can agree
/// on *which* randomized decision a recorded outcome belongs to,
/// independent of how many raw PRNG draws either side makes. Crit and
/// Damage are implied by the draw method; Accuracy vs. Secondary (both
/// `percent_1_100` on the engine side) are disambiguated by the battle
/// via [`Rng::set_decision`] before the draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RngDecision {
    /// Hit/miss accuracy check (engine: `percent_1_100`; PS: `random(100)` /
    /// `randomChance(acc, 100)` inside the accuracy step).
    Accuracy,
    /// Critical-hit roll.
    Crit,
    /// 16-bucket damage roll.
    Damage,
    /// Secondary-effect proc (status/flinch/boost chance).
    Secondary,
    /// Generic `range(n)` draw not otherwise classified (multi-hit count,
    /// status duration, confusion self-hit, …). Refined in later phases.
    Range,
    /// Speed-tie / ordering tiebreak (`next_u64`).
    Tiebreak,
}

/// The semantic key under which a randomized outcome is recorded and
/// later looked up. Order- and count-independent: an engine-only extra
/// draw simply misses the table and takes a deterministic fallback
/// without shifting any other lookup (the failure mode the flat
/// positional queue suffers from — see `docs/ps-comparison-harness-design.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RngKey {
    pub turn: u32,
    pub actor: SlotRef,
    pub target: SlotRef,
    /// Engine numeric move id (`data::move_id::*`). The conformance runner
    /// maps PS move slugs to this id when building the table, so the engine
    /// can key on the id it already has in scope — no slug lookup in the
    /// draw path.
    pub move_id: u16,
    pub decision: RngDecision,
}

/// Backing state for [`Rng::OracleKeyed`]: a table of pre-recorded
/// outcomes keyed by [`RngKey`], a live draw context the battle updates
/// as it resolves moves, a Splitmix fallback for unmatched draws, and a
/// running count of how many draws missed the table (a health metric —
/// a high count means the keying / draw-site tagging needs attention).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyedState {
    table: HashMap<RngKey, VecDeque<RngEvent>>,
    ctx_turn: u32,
    ctx_actor: SlotRef,
    ctx_target: SlotRef,
    ctx_move: u16,
    ctx_decision: RngDecision,
    /// The decision of the most recent `take` — for the miss diagnostic, so
    /// crit/damage misses report their real decision, not the stale
    /// `ctx_decision` (which only tracks Accuracy/Secondary).
    last_decision: RngDecision,
    fallback: u64,
    unmatched: u32,
    /// Per-miss log accumulating `(key, space, drawn)` for every keyed
    /// draw that missed the table. Empty by default; the outcome-frontier
    /// enumerator turns on miss-recording to drive its lazy re-record loop
    /// (each `unmatched_total > 0` combo reveals a counter-factual draw
    /// site the record-pass didn't capture). See [`Rng::take_miss_log`].
    miss_log: Vec<RecordedDraw>,
}

impl KeyedState {
    fn key(&self, decision: RngDecision) -> RngKey {
        RngKey {
            turn: self.ctx_turn,
            actor: self.ctx_actor,
            target: self.ctx_target,
            move_id: self.ctx_move,
            decision,
        }
    }

    /// Pop the next recorded outcome for `decision` under the current
    /// context, in FIFO order (so repeats — multi-hit accuracy, N
    /// secondaries — resolve in the order PS recorded them). Returns
    /// `None` on a table miss WITHOUT counting it; callers route a miss
    /// through [`KeyedState::miss`].
    fn take(&mut self, decision: RngDecision) -> Option<RngEvent> {
        self.last_decision = decision;
        let key = self.key(decision);
        self.table.get_mut(&key).and_then(|q| q.pop_front())
    }

    /// Advance the Splitmix fallback WITHOUT counting a miss. Used for
    /// draws that are engine-internal and not PS-recorded outcomes we're
    /// validating — chiefly the per-action speed-tie *nonce* (`next_u64`),
    /// which the engine draws for every action to keep sort keys unique
    /// even when speeds differ and PS draws nothing. A genuine tie whose
    /// order matters surfaces as a state divergence in the differ, not
    /// here, so excluding it keeps `unmatched` measuring only real
    /// outcome draws (accuracy/crit/damage/secondary/range).
    fn fallback(&mut self) -> u64 {
        Rng::splitmix_step(&mut self.fallback)
    }

    /// Record a table miss and advance the Splitmix fallback. Every
    /// counted fallback in the `OracleKeyed` arms routes through here so
    /// `unmatched` stays accurate and the fallback stream is
    /// byte-identical to a same-seeded `Splitmix`.
    /// Record a table miss as a [`RecordedDraw`] in [`KeyedState::miss_log`]
    /// — captures the site's full [`DrawSpace`] and concrete drawn value
    /// for the outcome-frontier enumerator's lazy re-record loop. This is
    /// the LOG-only side: callers compute `drawn` from a prior
    /// `self.fallback()` step and then call `record_miss` to bump
    /// `unmatched` + push the log entry. Splits responsibility from the
    /// legacy `miss()` (which combined fallback + count) so the value
    /// drawn is available before the count bump.
    fn record_miss(
        &mut self,
        decision: RngDecision,
        space: DrawSpace,
        drawn: RngEvent,
    ) {
        self.last_decision = decision;
        self.unmatched += 1;
        if std::env::var_os("VGC_CONF_DEBUG").is_some() {
            eprintln!(
                "UNMATCHED draw: turn={} actor={} target={} move={} decision={:?}",
                self.ctx_turn, self.ctx_actor, self.ctx_target, self.ctx_move, decision,
            );
        }
        self.miss_log.push(RecordedDraw {
            key: self.key(decision),
            space,
            drawn,
        });
    }
}

/// Bit-exact port of Pokémon Showdown's `sim/prng.ts:Gen5RNG` — the
/// in-cartridge LCG used by every numeric-seeded PS battle.
///
/// State is a 64-bit integer advanced by `seed = a * seed + c (mod 2^64)`
/// with the documented constants. `next()` returns the upper 32 bits.
/// `random_n` / `random_range` mirror PS's `random(from, to)` API:
/// `floor(result * range / 2^32) + from`.
///
/// Validated against PS reference vectors generated from
/// `/tmp/pokemon-showdown-research/dist/sim/prng` — see the tests
/// at the bottom of this module.
///
/// PR-209 ships this as a standalone struct so it can be unit-tested
/// against PS without invasive changes to the `Rng` enum's many match
/// arms. Wiring it into `Rng::PsGen5(...)` as a first-class variant is
/// a follow-up; the immediate goal here is the bit-exact LCG and a
/// known-good correspondence to PS's `random()` semantics.
///
/// PS reference: `sim/prng.ts:235-300` (Gen5RNG class).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PsGen5Rng {
    state: u64,
}

impl PsGen5Rng {
    /// LCG multiplier. PS `sim/prng.ts:286`.
    const A: u64 = 0x5D58_8B65_6C07_8965;
    /// LCG increment. PS `sim/prng.ts:287` — `[0, 0, 0x26, 0x9EC3]`
    /// as a 16-bit-chunked u64 is `0x0000_0000_0026_9EC3`.
    const C: u64 = 0x0000_0000_0026_9EC3;

    /// Construct from a PS-style `[u16; 4]` seed array. The PS
    /// representation is big-endian: `[hi, hi_mid, lo_mid, lo]`.
    pub fn new(seed: [u16; 4]) -> Self {
        let state = ((seed[0] as u64) << 48)
            | ((seed[1] as u64) << 32)
            | ((seed[2] as u64) << 16)
            | (seed[3] as u64);
        Self { state }
    }

    /// One LCG step. Returns the next 32-bit draw (upper half of the
    /// advanced state). Matches PS `Gen5RNG::next` exactly.
    pub fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_mul(Self::A).wrapping_add(Self::C);
        (self.state >> 32) as u32
    }

    /// `random(n)`: uniform integer in `[0, n)`. Mirrors PS
    /// `floor(result * n / 2^32)`.
    pub fn random_n(&mut self, n: u32) -> u32 {
        let r = self.next() as u64;
        ((r * n as u64) >> 32) as u32
    }

    /// `random(m, n)`: uniform integer in `[m, n)`. Mirrors PS
    /// `floor(result * (n - m) / 2^32) + m`.
    pub fn random_range(&mut self, m: u32, n: u32) -> u32 {
        self.random_n(n - m) + m
    }

    /// `randomChance(num, denom)`: PS `random(denom) < num`.
    pub fn random_chance(&mut self, num: u32, denom: u32) -> bool {
        self.random_n(denom) < num
    }
}

/// Compact descriptor of the probability space of a single draw site.
///
/// The Recording RNG attaches one of these to every draw it observes so the
/// caller (bruteforce / outcome-frontier layer) can expand each site into its
/// outcomes and probabilities WITHOUT re-deriving stage tables or accuracy
/// math from the keyed events. Designed to enumerate efficiently — uniform
/// spaces stay compact (no 16-entry vector for damage rolls).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrawSpace {
    /// Uniform integer in `0..n`. Drawn outcome is `RngEvent::Range(v)`.
    UniformRange(u32),
    /// Uniform damage bucket `0..16` (`RngEvent::DamageRoll`).
    UniformDamage,
    /// Uniform percent roll `1..=100` (`RngEvent::PercentRoll`).
    /// The hit-threshold (or secondary-proc threshold) is caller-context;
    /// this seam records only the raw 100-outcome uniform. The caller
    /// collapses to hit/miss after dedup.
    UniformPercent,
    /// Crit Bernoulli (`num/denom` for `true`). Stage 3+ guaranteed crits
    /// are NOT recorded (no random draw); same convention as PS / Oracle.
    Crit { num: u32, denom: u32 },
    /// Speed-tie nonce / opaque `u64` draw (`RngEvent::Tiebreak`). The
    /// probability space is `2^64`; enumerators should marginalize this
    /// out (it's an ordering tag, not a meaningful branch).
    Tiebreak,
}

/// One recorded draw site, with its full probability space and the
/// outcome the Recording RNG happened to pick. Carries the same `RngKey`
/// the [`Rng::OracleKeyed`] table is keyed on, so a downstream
/// enumeration layer can: (1) walk `RecordedDraw` entries to build the
/// enumeration cross-product, and (2) inject each combo by populating an
/// `OracleKeyed` table with `(key, alt_event)` pairs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedDraw {
    pub key: RngKey,
    pub space: DrawSpace,
    pub drawn: RngEvent,
}

/// Backing state for [`Rng::Recording`]: a Splitmix stream that picks the
/// concrete outcome at each site (so `step()` makes deterministic progress
/// along a single execution path), a log of every draw site visited along
/// the way, and the same move-context fields the keyed oracle tracks so
/// the recorded `RngKey`s line up with what `OracleKeyed` would query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingState {
    fallback: u64,
    log: Vec<RecordedDraw>,
    ctx_turn: u32,
    ctx_actor: SlotRef,
    ctx_target: SlotRef,
    ctx_move: u16,
    ctx_decision: RngDecision,
}

impl RecordingState {
    fn key(&self, decision: RngDecision) -> RngKey {
        RngKey {
            turn: self.ctx_turn,
            actor: self.ctx_actor,
            target: self.ctx_target,
            move_id: self.ctx_move,
            decision,
        }
    }

    fn step(&mut self) -> u64 {
        Rng::splitmix_step(&mut self.fallback)
    }

    fn push(&mut self, decision: RngDecision, space: DrawSpace, drawn: RngEvent) {
        self.log.push(RecordedDraw {
            key: self.key(decision),
            space,
            drawn,
        });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Rng {
    Splitmix(u64),
    Oracle(OracleState),
    /// Partial oracle: pop only when the next event's variant matches
    /// the requested draw; otherwise the embedded Splitmix state takes
    /// over for this draw and the queue position is unchanged. Use
    /// this for the corpus harness when only some draw sites are
    /// recorded.
    OraclePartial { state: OracleState, fallback: u64 },
    /// Bit-exact PS Gen5 LCG (PR-209's `PsGen5Rng`). Engine and PS
    /// from the same `[u16; 4]` seed produce identical PRNG values at
    /// every site — the corpus harness can ditch the oracle queue and
    /// score against the engine's own deterministic playthrough.
    PsGen5(PsGen5Rng),
    /// Keyed outcome oracle (conformance harness). Each draw is resolved
    /// by a *semantic* key (turn + actor + move + target + decision)
    /// rather than by queue position, so the injection is independent of
    /// how many raw draws each engine makes. A keyed miss takes a
    /// deterministic Splitmix fallback and bumps `unmatched` without
    /// disturbing any other lookup. See
    /// `docs/ps-comparison-harness-design.md`.
    OracleKeyed(KeyedState),
    /// Recording mode for outcome-frontier enumeration. Picks each draw
    /// deterministically from an embedded Splitmix stream (so `step()`
    /// makes progress along a single path) and appends a [`RecordedDraw`]
    /// to its log capturing the full [`DrawSpace`] and the value drawn.
    /// Used by the bruteforce / matrix-game layer to discover what sites
    /// fire on a given `(state, joint_action)` before enumerating their
    /// cross-product through `OracleKeyed`. See `docs/plan` Phase 4.
    Recording(RecordingState),
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

    /// Construct a PS-Gen5-compatible RNG from a `[u16; 4]` seed —
    /// the same big-endian-quartet format PS accepts as a numeric
    /// `PRNGSeed`. Engine + PS from the same seed produce
    /// bit-identical PRNG sequences at every draw site.
    pub fn ps_gen5(seed: [u16; 4]) -> Self {
        Self::PsGen5(PsGen5Rng::new(seed))
    }

    /// Keyed-oracle constructor for the conformance harness. `table`
    /// maps each [`RngKey`] to its FIFO queue of recorded outcomes (the
    /// runner converts PS-recorded draws — including the PS→engine
    /// damage-bucket flip — before populating it). `fallback_seed` seeds
    /// the Splitmix stream used for any draw that misses the table.
    pub fn oracle_keyed(table: HashMap<RngKey, VecDeque<RngEvent>>, fallback_seed: u64) -> Self {
        Self::OracleKeyed(KeyedState {
            table,
            ctx_turn: 0,
            ctx_actor: NO_SLOT,
            ctx_target: NO_SLOT,
            ctx_move: 0,
            ctx_decision: RngDecision::Range,
            last_decision: RngDecision::Range,
            fallback: fallback_seed,
            unmatched: 0,
            miss_log: Vec::new(),
        })
    }

    /// Drain the miss-log of an `OracleKeyed` Rng — the `(key, space,
    /// drawn)` entries for every keyed draw that missed the table during
    /// this Rng's lifetime. Returns `None` for every other variant. After
    /// the call the log is empty so subsequent draws accumulate fresh.
    ///
    /// Used by the outcome-frontier enumerator's lazy re-record loop: a
    /// non-empty miss-log after a combo replay reveals counter-factual
    /// draw sites the original record-pass didn't see, which the loop
    /// folds into the per-site cross-product and re-enumerates.
    pub fn take_miss_log(&mut self) -> Option<Vec<RecordedDraw>> {
        match self {
            Rng::OracleKeyed(k) => Some(std::mem::take(&mut k.miss_log)),
            _ => None,
        }
    }

    /// Construct a `Recording` Rng seeded by `fallback_seed`. The seam
    /// draws each site deterministically from a Splitmix stream while
    /// appending one [`RecordedDraw`] per visited site to its log.
    /// Read the log via [`Rng::recording_log`] after the host `step()`
    /// call returns.
    pub fn recording(fallback_seed: u64) -> Self {
        Self::Recording(RecordingState {
            fallback: fallback_seed,
            log: Vec::new(),
            ctx_turn: 0,
            ctx_actor: NO_SLOT,
            ctx_target: NO_SLOT,
            ctx_move: 0,
            ctx_decision: RngDecision::Range,
        })
    }

    /// Borrow the [`RecordedDraw`] log on a `Recording` Rng. Returns
    /// `None` for every other variant.
    pub fn recording_log(&self) -> Option<&[RecordedDraw]> {
        match self {
            Rng::Recording(r) => Some(&r.log),
            _ => None,
        }
    }

    /// Take ownership of the recorded draw log. Returns `None` for every
    /// non-`Recording` variant; on `Recording`, leaves the log empty so
    /// the same Rng can continue recording subsequent calls without the
    /// caller carrying stale entries.
    pub fn take_recording_log(&mut self) -> Option<Vec<RecordedDraw>> {
        match self {
            Rng::Recording(r) => Some(std::mem::take(&mut r.log)),
            _ => None,
        }
    }

    /// Set the move-resolution context that subsequent keyed draws are
    /// attributed to. No-op for every variant that doesn't carry a
    /// context, so the battle can call it unconditionally at its
    /// draw-site choke points without branching (production `Splitmix`
    /// battles pay nothing).
    pub fn set_move_context(&mut self, turn: u32, actor: SlotRef, move_id: u16, target: SlotRef) {
        match self {
            Rng::OracleKeyed(k) => {
                k.ctx_turn = turn;
                k.ctx_actor = actor;
                k.ctx_move = move_id;
                k.ctx_target = target;
            }
            Rng::Recording(r) => {
                r.ctx_turn = turn;
                r.ctx_actor = actor;
                r.ctx_move = move_id;
                r.ctx_target = target;
            }
            _ => {}
        }
    }

    /// Set the decision class for the *next* ambiguous draw — used to
    /// distinguish Accuracy from Secondary (both `percent_1_100` on the
    /// engine side). No-op for variants without a context.
    pub fn set_decision(&mut self, decision: RngDecision) {
        match self {
            Rng::OracleKeyed(k) => k.ctx_decision = decision,
            Rng::Recording(r) => r.ctx_decision = decision,
            _ => {}
        }
    }

    /// Number of keyed draws that missed the table and fell back to
    /// Splitmix. `Some(0)` on a clean `OracleKeyed` replay; `None` for
    /// every other variant. A health metric for the conformance harness.
    pub fn unmatched_draws(&self) -> Option<u32> {
        match self {
            Rng::OracleKeyed(k) => Some(k.unmatched),
            _ => None,
        }
    }

    /// For Oracle / OraclePartial variants, return `(consumed, total)` —
    /// how many events the engine has popped from the queue vs. how
    /// many PS originally recorded. Splitmix returns `None`.
    ///
    /// Used by the golden harness to assert per-call balance: if PS
    /// recorded N draws and the engine consumed M ≠ N, the per-call
    /// queue is silently misaligned and every downstream divergence
    /// is unreliable signal.
    pub fn oracle_pops(&self) -> Option<(usize, usize)> {
        match self {
            // OracleKeyed is positionless — use `unmatched_draws` instead.
            Rng::Splitmix(_) | Rng::PsGen5(_) | Rng::OracleKeyed(_) | Rng::Recording(_) => None,
            Rng::Oracle(state) | Rng::OraclePartial { state, .. } => {
                Some((state.pos, state.events.len()))
            }
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
            // PS is 32-bit; widen to u64 for next_u64 callers (mostly
            // tiebreak / coin_flip / range fallbacks).
            Rng::PsGen5(rng) => rng.next() as u64,
            Rng::OracleKeyed(k) => match k.take(RngDecision::Tiebreak) {
                Some(RngEvent::Tiebreak(v)) => v,
                // Uncounted: the speed-tie nonce is engine-internal (drawn
                // per action even with no tie); a real tie's order shows up
                // in the state diff, not the unmatched metric.
                _ => k.fallback(),
            },
            Rng::Recording(r) => {
                let v = r.step();
                r.push(RngDecision::Tiebreak, DrawSpace::Tiebreak, RngEvent::Tiebreak(v));
                v
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
                Rng::Oracle(_) | Rng::OraclePartial { .. } | Rng::OracleKeyed(_) => {
                    // PS doesn't draw at `randomChance(1, 1)` sites.
                }
                Rng::PsGen5(_) => {
                    // Mirror Oracle: PS elides the draw at denom=1.
                }
                Rng::Recording(_) => {
                    // Mirror Oracle: a 1-outcome space has no branch worth
                    // recording.
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
            // Bit-exact PS `random(n)` semantics from PR-209's port.
            Rng::PsGen5(rng) => rng.random_n(n),
            Rng::OracleKeyed(k) => match k.take(RngDecision::Range) {
                Some(RngEvent::Range(v)) if v < n => v,
                _ => {
                    let v = (k.fallback() as u32) % n;
                    k.record_miss(
                        RngDecision::Range,
                        DrawSpace::UniformRange(n),
                        RngEvent::Range(v),
                    );
                    v
                }
            },
            Rng::Recording(r) => {
                let v = (r.step() as u32) % n;
                r.push(RngDecision::Range, DrawSpace::UniformRange(n), RngEvent::Range(v));
                v
            }
        }
    }

    /// Gender roll for a ratio'd species at battle construction.
    /// Returns `0` for male, `1` for female. Mirrors PS's
    /// `this.battle.sample(['M', 'F'])` → `prng.random(2)` (a flat 50/50
    /// draw; PS ignores the numeric `genderRatio`). PS `sim/pokemon.ts:340`,
    /// `sim/prng.ts:132` (`sample` → `random(items.length)`).
    ///
    /// Critically, this draw is variant-sensitive to keep both golden
    /// modes bit-exact:
    ///
    /// * **`PsGen5`** — draws `random(2)` from the bit-exact LCG, so the
    ///   engine consumes the same construction-time draws PS does and the
    ///   downstream mechanic stream stays aligned (PsGen5 is the only mode
    ///   that mirrors PS's actual LCG).
    /// * **`Oracle` / `OraclePartial`** — does NOT pop the queue or touch
    ///   the Splitmix fallback. PS's gender draws go through `prng.sample`,
    ///   which bypasses the `Battle.random` patch the golden driver hooks,
    ///   so they are absent from the recorded oracle queue. Consuming an
    ///   event here would desync the replay. Returns `0` (male) without
    ///   advancing anything, leaving the recorded mechanic stream byte-
    ///   identical.
    /// * **`Splitmix`** — also a non-advancing `0`. Standalone battles
    ///   roll no gender draw at construction so that every existing
    ///   seed-pinned unit test keeps its exact downstream stream. Gender
    ///   gates no implemented mechanic yet, so a deterministic default is
    ///   harmless; revisit when Attract / Cute Charm land.
    pub fn gender_roll(&mut self) -> u8 {
        match self {
            Rng::PsGen5(rng) => rng.random_n(2) as u8,
            Rng::Splitmix(_)
            | Rng::Oracle(_)
            | Rng::OraclePartial { .. }
            // PS gender draws bypass the `Battle.random` patch, so they're
            // absent from the recorded table — don't draw, mirror Oracle.
            | Rng::OracleKeyed(_)
            // Recording follows Oracle: gender is not an enumerated site.
            | Rng::Recording(_) => 0,
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
            // OracleKeyed stores the exact engine-convention bucket, so
            // there's nothing to back-solve — defer to `damage_roll`.
            Rng::Splitmix(_) | Rng::PsGen5(_) | Rng::OracleKeyed(_) | Rng::Recording(_) => self.damage_roll(),
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
            // PS `random(16)` returns 0..=15 — bit-exact.
            Rng::PsGen5(rng) => rng.random_n(16) as u8,
            Rng::OracleKeyed(k) => match k.take(RngDecision::Damage) {
                Some(RngEvent::DamageRoll(v)) => {
                    debug_assert!(v < 16);
                    v
                }
                _ => {
                    let v = (k.fallback() & 0xF) as u8;
                    k.record_miss(
                        RngDecision::Damage,
                        DrawSpace::UniformDamage,
                        RngEvent::DamageRoll(v),
                    );
                    v
                }
            },
            Rng::Recording(r) => {
                let v = (r.step() & 0xF) as u8;
                r.push(RngDecision::Damage, DrawSpace::UniformDamage, RngEvent::DamageRoll(v));
                v
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
            // PS `random(100)` returns 0..=99; engine's percent_1_100
            // is 1..=100. Map by +1 so PS-recorded `randomChance(N, 100)`
            // semantics line up (engine checks `roll <= N`, PS computes
            // `random(100) < N` — adding 1 to the PS draw makes both
            // sides land on the same hit threshold).
            Rng::PsGen5(rng) => (rng.random_n(100) as u8) + 1,
            // Decision (Accuracy vs Secondary) comes from the context the
            // battle set via `set_decision` — both are `percent_1_100` here
            // but distinct sites on the PS side.
            Rng::OracleKeyed(k) => {
                let decision = k.ctx_decision;
                match k.take(decision) {
                    Some(RngEvent::PercentRoll(v)) => v,
                    _ => {
                        let v = ((k.fallback() % 100) as u8) + 1;
                        k.record_miss(
                            decision,
                            DrawSpace::UniformPercent,
                            RngEvent::PercentRoll(v),
                        );
                        v
                    }
                }
            }
            Rng::Recording(r) => {
                let v = ((r.step() % 100) as u8) + 1;
                let decision = r.ctx_decision;
                r.push(decision, DrawSpace::UniformPercent, RngEvent::PercentRoll(v));
                v
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
            Rng::Splitmix(_) | Rng::PsGen5(_) => match stage {
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
            // Honor a recorded crit outcome if PS drew one at this site;
            // otherwise mirror the deterministic stage table (stage 3+ is a
            // guaranteed crit PS never rolls, so it counts no miss).
            Rng::OracleKeyed(k) => match k.take(RngDecision::Crit) {
                Some(RngEvent::Crit(v)) => v,
                _ => {
                    let denom = match stage {
                        0 => 24u32,
                        1 => 8,
                        2 => 2,
                        _ => return true, // stage 3+ never draws
                    };
                    let v = ((k.fallback() as u32) % denom) == 0;
                    k.record_miss(
                        RngDecision::Crit,
                        DrawSpace::Crit { num: 1, denom },
                        RngEvent::Crit(v),
                    );
                    v
                }
            },
            Rng::Recording(r) => {
                let denom = match stage {
                    0 => 24,
                    1 => 8,
                    2 => 2,
                    _ => return true, // guaranteed crit, no draw to record
                };
                let v = ((r.step() as u32) % denom) == 0;
                r.push(
                    RngDecision::Crit,
                    DrawSpace::Crit { num: 1, denom },
                    RngEvent::Crit(v),
                );
                v
            }
        }
    }

    /// Stage-0 crit. Kept as a thin shim around `crit_with_stage(0)`
    /// for existing call sites; new code should prefer the staged form.
    pub fn crit(&mut self) -> bool {
        match self {
            Rng::Splitmix(_) | Rng::PsGen5(_) => self.range(24) == 0,
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
            Rng::OracleKeyed(k) => match k.take(RngDecision::Crit) {
                Some(RngEvent::Crit(v)) => v,
                _ => {
                    let v = ((k.fallback() as u32) % 24) == 0;
                    k.record_miss(
                        RngDecision::Crit,
                        DrawSpace::Crit { num: 1, denom: 24 },
                        RngEvent::Crit(v),
                    );
                    v
                }
            },
            Rng::Recording(r) => {
                let v = ((r.step() as u32) % 24) == 0;
                r.push(
                    RngDecision::Crit,
                    DrawSpace::Crit { num: 1, denom: 24 },
                    RngEvent::Crit(v),
                );
                v
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

    // Reference vectors generated from PS `dist/sim/prng` —
    // see PR-209 commit body for the exact node one-liner.
    #[test]
    fn ps_gen5_next_seed_0() {
        let mut r = PsGen5Rng::new([0, 0, 0, 0]);
        let got: Vec<u32> = (0..10).map(|_| r.next()).collect();
        assert_eq!(
            got,
            vec![
                0, 1904791564, 183838931, 176901684, 3359619440,
                205866298, 3425238665, 580285797, 952588127, 1736004865,
            ],
        );
    }

    #[test]
    fn ps_gen5_next_seed_42() {
        let mut r = PsGen5Rng::new([42, 0, 0, 0]);
        let got: Vec<u32> = (0..10).map(|_| r.next()).collect();
        assert_eq!(
            got,
            vec![
                2324824064, 1059246092, 2461477075, 1813335604, 498186608,
                3292480826, 734723721, 2407560549, 1796822879, 601052417,
            ],
        );
    }

    #[test]
    fn ps_gen5_random_n_16_seed_0() {
        let mut r = PsGen5Rng::new([0, 0, 0, 0]);
        let got: Vec<u32> = (0..4).map(|_| r.random_n(16)).collect();
        assert_eq!(got, vec![0, 7, 0, 0]);
    }

    #[test]
    fn ps_gen5_random_range_damage_roll_seed_0() {
        // PS damage-roll uses `random(85, 101)` — 16 buckets [85, 101).
        let mut r = PsGen5Rng::new([0, 0, 0, 0]);
        let got: Vec<u32> = (0..3).map(|_| r.random_range(85, 101)).collect();
        assert_eq!(got, vec![85, 92, 85]);
    }

    #[test]
    fn ps_gen5_random_chance_uses_random_n() {
        // randomChance(num, denom) ≡ random_n(denom) < num. Smoke-test
        // the wrapper produces the same bit-exact answer for a few
        // representative crit-rate denominators.
        let mut r1 = PsGen5Rng::new([12345, 0, 0, 0]);
        let mut r2 = PsGen5Rng::new([12345, 0, 0, 0]);
        for _ in 0..50 {
            let a = r1.random_chance(1, 24);
            let b = r2.random_n(24) < 1;
            assert_eq!(a, b);
        }
    }

    #[test]
    fn ps_gen5_rng_variant_methods_route_to_lcg() {
        // Same seed → same sequence via the Rng wrapper as via the
        // underlying PsGen5Rng directly. Verifies all the variant
        // arms route through the LCG (not to Splitmix or oracle).
        let mut wrapped = Rng::ps_gen5([0, 0, 0, 0]);
        let mut bare = PsGen5Rng::new([0, 0, 0, 0]);

        // range(16) → random_n(16) bit-exact
        for _ in 0..4 {
            assert_eq!(wrapped.range(16), bare.random_n(16));
        }

        // damage_roll → random_n(16) bit-exact
        let mut wrapped2 = Rng::ps_gen5([42, 7, 0, 0]);
        let mut bare2 = PsGen5Rng::new([42, 7, 0, 0]);
        for _ in 0..8 {
            assert_eq!(wrapped2.damage_roll() as u32, bare2.random_n(16));
        }

        // percent_1_100 → random_n(100) + 1
        let mut wrapped3 = Rng::ps_gen5([1234, 0, 0, 0]);
        let mut bare3 = PsGen5Rng::new([1234, 0, 0, 0]);
        for _ in 0..8 {
            assert_eq!(wrapped3.percent_1_100() as u32, bare3.random_n(100) + 1);
        }

        // No oracle pops on PsGen5.
        assert!(Rng::ps_gen5([0, 0, 0, 0]).oracle_pops().is_none());
    }

    #[test]
    fn gender_roll_psgen5_is_bit_exact_random2() {
        // PsGen5 gender_roll == random_n(2) on the same LCG. Cross-checked
        // against PS: a `gen9customgame` battle on seed [1,2,3,4] rolls the
        // first ratio'd mon (p1 slot 0) male (random(2)=0) and the second
        // (p2 slot 0) female (random(2)=1). See the PR's gender_probe.
        let mut g = Rng::ps_gen5([1, 2, 3, 4]);
        assert_eq!(g.gender_roll(), 0, "first roll male (PS random(2)=0)");
        assert_eq!(g.gender_roll(), 1, "second roll female (PS random(2)=1)");
        // And it really is `random_n(2)` bit-for-bit.
        let mut a = Rng::ps_gen5([7, 8, 9, 10]);
        let mut b = PsGen5Rng::new([7, 8, 9, 10]);
        for _ in 0..16 {
            assert_eq!(a.gender_roll() as u32, b.random_n(2));
        }
    }

    #[test]
    fn gender_roll_non_psgen5_does_not_advance_stream() {
        // Splitmix / Oracle / OraclePartial must NOT consume a draw for
        // gender: PS's gender roll bypasses the recorded oracle queue, and
        // existing seed-pinned tests must keep their exact streams. The
        // method returns a deterministic 0 and leaves the stream untouched.
        let mut rolled = Rng::new(0xFEED);
        assert_eq!(rolled.gender_roll(), 0);
        assert_eq!(rolled.gender_roll(), 0);
        let mut fresh = Rng::new(0xFEED);
        // Two gender_rolls did not move the Splitmix stream at all.
        for _ in 0..8 {
            assert_eq!(rolled.next_u64(), fresh.next_u64());
        }

        // OraclePartial: gender_roll pops nothing and doesn't touch the
        // fallback — the queued mechanic events stay in place.
        let mut op = Rng::oracle_partial(
            vec![RngEvent::PercentRoll(50), RngEvent::DamageRoll(9)],
            0xABCD,
        );
        assert_eq!(op.gender_roll(), 0);
        assert_eq!(op.oracle_pops(), Some((0, 2)), "no queue consumed");
        assert_eq!(op.percent_1_100(), 50);
        assert_eq!(op.damage_roll(), 9);
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

    // ---- keyed oracle (conformance harness) -------------------------------

    fn key(turn: u32, actor: SlotRef, target: SlotRef, mv: u16, d: RngDecision) -> RngKey {
        RngKey { turn, actor, target, move_id: mv, decision: d }
    }

    fn table(entries: Vec<(RngKey, Vec<RngEvent>)>) -> HashMap<RngKey, VecDeque<RngEvent>> {
        entries
            .into_iter()
            .map(|(k, v)| (k, v.into_iter().collect()))
            .collect()
    }

    #[test]
    fn oracle_keyed_resolves_each_decision_under_context() {
        // p1a (0) uses move id 7 on p2a (2), turn 1. Record one outcome
        // per decision class and verify each draw site reads its own.
        let mut t = table(vec![
            (key(1, 0, 2, 7, RngDecision::Crit), vec![RngEvent::Crit(true)]),
            (key(1, 0, 2, 7, RngDecision::Damage), vec![RngEvent::DamageRoll(15)]),
            (key(1, 0, 2, 7, RngDecision::Accuracy), vec![RngEvent::PercentRoll(3)]),
            (key(1, 0, 2, 7, RngDecision::Secondary), vec![RngEvent::PercentRoll(99)]),
        ]);
        // shuffle insertion order doesn't matter for a HashMap; build rng.
        let _ = &mut t;
        let mut r = Rng::oracle_keyed(t, 0xDEAD);
        r.set_move_context(1, 0, 7, 2);

        // Crit and Damage are keyed by method.
        assert!(r.crit_with_stage(0), "recorded crit honored");
        assert_eq!(r.damage_roll(), 15, "recorded damage bucket honored");
        // Accuracy and Secondary share percent_1_100 — decision picks.
        r.set_decision(RngDecision::Accuracy);
        assert_eq!(r.percent_1_100(), 3);
        r.set_decision(RngDecision::Secondary);
        assert_eq!(r.percent_1_100(), 99);
        assert_eq!(r.unmatched_draws(), Some(0), "no fallbacks taken");
    }

    #[test]
    fn oracle_keyed_repeats_resolve_fifo() {
        // Two secondaries under one key resolve in recorded order.
        let t = table(vec![(
            key(2, 2, 0, 11, RngDecision::Secondary),
            vec![RngEvent::PercentRoll(10), RngEvent::PercentRoll(80)],
        )]);
        let mut r = Rng::oracle_keyed(t, 1);
        r.set_move_context(2, 2, 11, 0);
        r.set_decision(RngDecision::Secondary);
        assert_eq!(r.percent_1_100(), 10);
        assert_eq!(r.percent_1_100(), 80);
        // Third draw misses the now-empty queue and falls back.
        let third = r.percent_1_100();
        assert!((1..=100).contains(&third));
        assert_eq!(r.unmatched_draws(), Some(1));
    }

    #[test]
    fn oracle_keyed_miss_falls_back_and_counts() {
        // Empty table → every draw is an unmatched fallback, but still a
        // valid in-range value (and the fallback stream equals Splitmix).
        let mut r = Rng::oracle_keyed(HashMap::new(), 0x1234_5678);
        r.set_move_context(1, 0, 1, 2);
        let mut pure = Rng::new(0x1234_5678);
        r.set_decision(RngDecision::Accuracy);
        assert_eq!(r.percent_1_100(), pure.percent_1_100());
        assert_eq!(r.damage_roll(), pure.damage_roll());
        // Crit also misses (empty table) → fallback matches pure Splitmix.
        assert_eq!(r.crit(), pure.crit());
        assert_eq!(r.unmatched_draws(), Some(3));
    }

    #[test]
    fn oracle_keyed_is_order_and_count_independent() {
        // The core property: an engine-only EXTRA draw (one the table has
        // no entry for) must NOT shift a later matched lookup. Positional
        // queues fail exactly here.
        let t = table(vec![
            (key(1, 0, 2, 5, RngDecision::Crit), vec![RngEvent::Crit(true)]),
            (key(1, 0, 2, 5, RngDecision::Damage), vec![RngEvent::DamageRoll(9)]),
        ]);
        let mut r = Rng::oracle_keyed(t, 7);
        r.set_move_context(1, 0, 5, 2);
        // Engine makes an UNRECORDED accuracy draw first (table has none).
        r.set_decision(RngDecision::Accuracy);
        let _ = r.percent_1_100(); // miss → fallback, must not disturb below
        // The recorded crit + damage still resolve to their recorded values.
        assert!(r.crit_with_stage(0));
        assert_eq!(r.damage_roll(), 9);
        assert_eq!(r.unmatched_draws(), Some(1), "only the accuracy draw missed");
    }

    #[test]
    fn oracle_keyed_context_switch_reattributes() {
        // Same decision+move, different turn → different key, no bleed.
        let t = table(vec![
            (key(1, 0, 2, 3, RngDecision::Damage), vec![RngEvent::DamageRoll(0)]),
            (key(2, 0, 2, 3, RngDecision::Damage), vec![RngEvent::DamageRoll(15)]),
        ]);
        let mut r = Rng::oracle_keyed(t, 9);
        r.set_move_context(1, 0, 3, 2);
        assert_eq!(r.damage_roll(), 0);
        r.set_move_context(2, 0, 3, 2);
        assert_eq!(r.damage_roll(), 15);
        assert_eq!(r.unmatched_draws(), Some(0));
    }

    #[test]
    fn oracle_keyed_range_n1_does_not_consume_or_miss() {
        // range(1) is elided (PS never draws at denom 1) — no table touch,
        // no unmatched bump.
        let mut r = Rng::oracle_keyed(HashMap::new(), 0);
        r.set_move_context(1, 0, 1, 2);
        assert_eq!(r.range(1), 0);
        assert_eq!(r.unmatched_draws(), Some(0));
    }

    // ---- recording (outcome-frontier enumeration seam) -------------------

    #[test]
    fn recording_matches_splitmix_value_for_value() {
        // Recording's deterministic draw stream is byte-identical to a
        // same-seeded Splitmix. The recorder logs alongside the picks but
        // never perturbs the chosen values.
        let mut rec = Rng::recording(0xABCD_1234);
        let mut sm = Rng::new(0xABCD_1234);
        for _ in 0..50 {
            assert_eq!(rec.damage_roll(), sm.damage_roll());
            assert_eq!(rec.range(37), sm.range(37));
            assert_eq!(rec.percent_1_100(), sm.percent_1_100());
            assert_eq!(rec.crit(), sm.crit());
        }
    }

    #[test]
    fn recording_logs_every_site_with_drawn_event() {
        let mut r = Rng::recording(99);
        r.set_move_context(3, 0, 42, 2);

        let d = r.damage_roll();
        r.set_decision(RngDecision::Accuracy);
        let p = r.percent_1_100();
        let c = r.crit_with_stage(1);
        let v = r.range(8);

        let log = r.recording_log().expect("Recording variant carries log");
        assert_eq!(log.len(), 4, "one entry per non-elided draw");

        // Each entry has the right space + drawn value + key context.
        assert_eq!(log[0].space, DrawSpace::UniformDamage);
        assert_eq!(log[0].drawn, RngEvent::DamageRoll(d));
        assert_eq!(log[0].key.decision, RngDecision::Damage);
        assert_eq!(log[0].key.turn, 3);
        assert_eq!(log[0].key.actor, 0);
        assert_eq!(log[0].key.target, 2);
        assert_eq!(log[0].key.move_id, 42);

        assert_eq!(log[1].space, DrawSpace::UniformPercent);
        assert_eq!(log[1].drawn, RngEvent::PercentRoll(p));
        assert_eq!(log[1].key.decision, RngDecision::Accuracy);

        assert_eq!(log[2].space, DrawSpace::Crit { num: 1, denom: 8 });
        assert_eq!(log[2].drawn, RngEvent::Crit(c));
        assert_eq!(log[2].key.decision, RngDecision::Crit);

        assert_eq!(log[3].space, DrawSpace::UniformRange(8));
        assert_eq!(log[3].drawn, RngEvent::Range(v));
        assert_eq!(log[3].key.decision, RngDecision::Range);
    }

    #[test]
    fn recording_stage_3_crit_not_logged() {
        // Guaranteed crit (stage ≥ 3) is not a random draw; mirror PS /
        // OracleKeyed and skip recording so the enumerator doesn't
        // materialize a degenerate 1-outcome dimension.
        let mut r = Rng::recording(1);
        assert!(r.crit_with_stage(3));
        let log = r.recording_log().unwrap();
        assert!(log.is_empty(), "stage-3 crit must not produce a draw site");
    }

    #[test]
    fn recording_range_n1_not_logged() {
        let mut r = Rng::recording(1);
        assert_eq!(r.range(1), 0);
        assert!(r.recording_log().unwrap().is_empty());
    }

    #[test]
    fn recording_gender_roll_not_logged() {
        let mut r = Rng::recording(1);
        assert_eq!(r.gender_roll(), 0);
        assert!(r.recording_log().unwrap().is_empty());
    }

    #[test]
    fn recording_secondary_decision_carried_into_key() {
        // percent_1_100 under set_decision(Secondary) produces a key with
        // Secondary, so an OracleKeyed table built from this log resolves
        // the same site on replay.
        let mut r = Rng::recording(7);
        r.set_move_context(1, 0, 5, 2);
        r.set_decision(RngDecision::Secondary);
        let _ = r.percent_1_100();
        let log = r.recording_log().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].key.decision, RngDecision::Secondary);
    }

    #[test]
    fn recording_log_roundtrips_through_oracle_keyed() {
        // The seam's correctness contract: a recorded log can be replayed
        // through OracleKeyed and reproduce the same draw values byte-for-
        // byte, with zero unmatched draws. This is what the enumeration
        // layer relies on — inject any combo by swapping `drawn`.
        let mut r = Rng::recording(0xFEED_FACE);
        r.set_move_context(1, 0, 11, 2);
        let d = r.damage_roll();
        r.set_decision(RngDecision::Accuracy);
        let p = r.percent_1_100();
        let c = r.crit_with_stage(0);
        let v = r.range(13);
        let log = r.take_recording_log().unwrap();
        // After take, the log is cleared.
        assert!(r.recording_log().unwrap().is_empty());

        // Build an OracleKeyed table from the log, with each drawn event
        // as the sole entry for its key.
        let mut t: HashMap<RngKey, VecDeque<RngEvent>> = HashMap::new();
        for entry in &log {
            t.entry(entry.key).or_default().push_back(entry.drawn);
        }
        let mut replay = Rng::oracle_keyed(t, 0);
        replay.set_move_context(1, 0, 11, 2);
        assert_eq!(replay.damage_roll(), d);
        replay.set_decision(RngDecision::Accuracy);
        assert_eq!(replay.percent_1_100(), p);
        assert_eq!(replay.crit_with_stage(0), c);
        assert_eq!(replay.range(13), v);
        assert_eq!(replay.unmatched_draws(), Some(0), "every site keyed cleanly");
    }
}
