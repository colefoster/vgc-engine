//! Replay-differential scorer.
//!
//! Per turn N (1..=last):
//!   1. Pull `[p1_choices, p2_choices]` from [`ChoiceExtractor`].
//!   2. Pad each side to `format.active_count()` with `Choice::Pass`.
//!   3. Run `Battle::step` once.
//!   4. Compare the engine's post-step HP fractions per active slot
//!      against the replay's last `HpEvent` per slot for the same turn.
//!
//! Output: a per-turn L1 of HP-fraction error plus a boolean
//! "agreed" verdict against a tolerance threshold. The full-replay
//! summary is the fraction of turns marked agreed — the input to the
//! Phase 2 gate.
//!
//! What this DOES NOT do (yet):
//! - Status / boost / weather verification — only HP fractions.
//! - Tolerate engine-side RNG divergence beyond the L1 tolerance.
//!   Damage rolls and accuracy checks will fall out of agreement
//!   until the engine's RNG is seeded from the replay or the
//!   tolerance is widened. That tuning lands in PR-42+.
//! - Score turn 0 (battle init) — the engine's lead state is fixed
//!   by `Battle::new`, no choices to extract.

use vgc_engine_core::battle::StepResult;
use vgc_engine_core::{Choice, Format, Status};

use crate::event::Event;

use crate::choices::ChoiceExtractor;
use crate::recon::TeamRecon;
use crate::replay::Replay;
use crate::runner::{RunnerError, RunnerInit};
use crate::trace::hp_trace;

/// Default agreement tolerance: per-slot HP fraction within ±5% of
/// the replay value counts as agreed. Roughly the width of one
/// damage-roll bucket (15/16 → 16/16 ≈ 6.3%).
pub const DEFAULT_HP_TOLERANCE: f32 = 0.05;

#[derive(Debug, Clone, Copy)]
pub struct TurnScore {
    pub turn: u32,
    /// Sum of |engine_hp_frac - replay_hp_frac| over the active slots
    /// where both sides have data. `f32::NAN` if no slot had data
    /// (every active mon already fainted at start of turn).
    pub hp_l1: f32,
    /// True when `hp_l1 / compared_slots <= tolerance` AND faint state
    /// matches per slot.
    pub agreed: bool,
    /// Number of active slots (across both sides) that contributed to
    /// `hp_l1`. 0 ⇒ scoring is degenerate for this turn.
    pub compared_slots: u8,
    /// Step ended the battle this turn.
    pub ended: bool,
    /// True when at least one slot's persistent status (slp/brn/par/
    /// psn/tox/frz/None) diverged between engine and replay at end of
    /// turn.
    pub status_diverged: bool,
    /// True when at least one slot's `is_alive()` disagreed with the
    /// replay's `fainted` flag.
    pub faint_diverged: bool,
    /// True when `hp_l1` exceeded the configured tolerance. Stored
    /// separately so the scorer can attribute disagreements per
    /// category (HP vs. faint vs. status) even when several fire at
    /// once.
    pub hp_diverged: bool,
}

#[derive(Debug, Clone)]
pub struct ReplayScore {
    pub replay_id: String,
    pub per_turn: Vec<TurnScore>,
    /// Fraction of turns marked agreed. `0.0` if no turns scored.
    pub agreement_pct: f32,
    /// Total turns the engine actually stepped through. May be < the
    /// replay's turn count if the engine ended the battle early.
    pub turns_run: u32,
}

impl ReplayScore {
    pub fn hp_diverged_turns(&self) -> usize {
        self.per_turn.iter().filter(|t| t.hp_diverged).count()
    }
    pub fn faint_diverged_turns(&self) -> usize {
        self.per_turn.iter().filter(|t| t.faint_diverged).count()
    }
    pub fn status_diverged_turns(&self) -> usize {
        self.per_turn.iter().filter(|t| t.status_diverged).count()
    }

    /// Recompute the agreement fraction at a different HP tolerance,
    /// without re-running the engine. `faint_diverged` and
    /// `status_diverged` still veto, only the HP-fraction L1 gate is
    /// reconsidered. Useful for the tolerance-sweep diagnostic.
    pub fn agreement_at(&self, tol: f32) -> f32 {
        if self.per_turn.is_empty() {
            return 0.0;
        }
        let agreed = self
            .per_turn
            .iter()
            .filter(|t| {
                !t.faint_diverged
                    && !t.status_diverged
                    && t.compared_slots > 0
                    && !t.hp_l1.is_nan()
                    && t.hp_l1 <= tol
            })
            .count();
        agreed as f32 / self.per_turn.len() as f32
    }
}

/// End-to-end: parse → recon → init → step turn-by-turn → score.
///
/// `seed` controls the engine's RNG; agreement on damage-roll bound
/// outcomes is sensitive to it but the L1 tolerance forgives one or
/// two buckets of divergence.
pub fn score_replay(
    replay: &Replay,
    recon: &impl TeamRecon,
    seed: u64,
    tolerance: f32,
) -> Result<ReplayScore, RunnerError> {
    score_replay_inner(replay, recon, seed, tolerance, false)
}

/// Like `score_replay` but uses the OracleRng path: extracts
/// `RngEvent`s from the replay (`build_crit_oracle_for_replay`) and
/// feeds them into the engine via `Rng::oracle_partial`. Un-recorded
/// draws (accuracy, secondaries, range, tiebreak) fall back to the
/// Splitmix stream seeded from `seed`.
///
/// Phase-2 scope: the recorded channel is **crit only**. Damage-roll
/// back-solving and percent extraction come in later PRs.
pub fn score_replay_oracle(
    replay: &Replay,
    recon: &impl TeamRecon,
    seed: u64,
    tolerance: f32,
) -> Result<ReplayScore, RunnerError> {
    // PR-95: combined oracle (crit + accuracy) is available via
    // `build_oracle_for_replay`, but defaults to crit-only here because
    // CanonicalDefault recon's damage miscalibration interacts badly
    // with force-hit/force-miss decisions — the engine's wrongly-scaled
    // damage on a force-hit causes earlier-than-PS faints that compound
    // through subsequent turns. Once set reconstruction lands, switch
    // this to `build_oracle_for_replay`.
    let events = crate::oracle::build_crit_oracle_for_replay(replay);
    score_replay_with_events(replay, recon, seed, tolerance, events)
}

/// Like `score_replay`, but the caller supplies the full `RngEvent`
/// queue (typically loaded from a `ps-rng-dump` sidecar via
/// `oracle::load_rng_dump`). Events feed into `Rng::oracle_partial`;
/// un-recorded draws fall back to Splitmix from `seed`.
pub fn score_replay_with_events(
    replay: &Replay,
    recon: &impl TeamRecon,
    seed: u64,
    tolerance: f32,
    events: Vec<vgc_engine_core::rng::RngEvent>,
) -> Result<ReplayScore, RunnerError> {
    let ex_init = RunnerInit::from_replay(replay, recon)?;
    let active_count = match ex_init.format {
        Format::Singles => 1,
        Format::Doubles => 2,
    };
    let mut ex = ChoiceExtractor::new(&ex_init);
    let init2 = RunnerInit::from_replay(replay, recon)?;
    let rng = vgc_engine_core::rng::Rng::oracle_partial(events, seed);
    let mut b = init2.into_battle_with_rng(seed, rng)?;
    score_loop(replay, &mut ex, &mut b, active_count, tolerance)
}

fn score_loop(
    replay: &Replay,
    ex: &mut ChoiceExtractor<'_>,
    b: &mut vgc_engine_core::Battle,
    active_count: usize,
    tolerance: f32,
) -> Result<ReplayScore, RunnerError> {
    let turns = replay.turns();
    let mut per_turn: Vec<TurnScore> = Vec::new();
    let mut agreed_count: u32 = 0;
    let mut turns_run: u32 = 0;
    let mut ended_flag = false;
    let mut replay_status: [[Status; 2]; 2] = [[Status::None; 2]; 2];

    for tv in &turns {
        update_replay_status(&mut replay_status, tv.events);
        if tv.number == 0 {
            let _ = ex.extract_turn(tv);
            continue;
        }
        let [mut p1c, mut p2c] = ex.extract_turn(tv);
        pad_with_pass(&mut p1c, active_count);
        pad_with_pass(&mut p2c, active_count);
        if !ended_flag {
            let r = b.step(&p1c, &p2c);
            ended_flag = matches!(r, StepResult::Ended { .. });
            turns_run += 1;
        }
        let score = score_turn(b, tv, tolerance, &replay_status);
        if score.agreed { agreed_count += 1; }
        per_turn.push(score);
    }
    let agreement_pct = if per_turn.is_empty() {
        0.0
    } else { agreed_count as f32 / per_turn.len() as f32 };
    Ok(ReplayScore {
        replay_id: replay.id.clone(),
        per_turn,
        agreement_pct,
        turns_run,
    })
}

fn score_replay_inner(
    replay: &Replay,
    recon: &impl TeamRecon,
    seed: u64,
    tolerance: f32,
    use_oracle: bool,
) -> Result<ReplayScore, RunnerError> {
    let ex_init = RunnerInit::from_replay(replay, recon)?;
    let active_count = match ex_init.format {
        Format::Singles => 1,
        Format::Doubles => 2,
    };
    let mut ex = ChoiceExtractor::new(&ex_init);
    // Build a second init for the engine — `into_battle` consumes the
    // owner, and `ex` still needs to hold its borrow.
    let init2 = RunnerInit::from_replay(replay, recon)?;
    let mut b = if use_oracle {
        let events = crate::oracle::build_crit_oracle_for_replay(replay);
        let rng = vgc_engine_core::rng::Rng::oracle_partial(events, seed);
        init2.into_battle_with_rng(seed, rng)?
    } else {
        init2.into_battle(seed)?
    };

    let turns = replay.turns();
    let mut per_turn: Vec<TurnScore> = Vec::new();
    let mut agreed_count: u32 = 0;
    let mut turns_run: u32 = 0;
    let mut ended_flag = false;
    let mut replay_status: [[Status; 2]; 2] = [[Status::None; 2]; 2];

    for tv in &turns {
        update_replay_status(&mut replay_status, tv.events);

        if tv.number == 0 {
            // Pre-turn-1 init: walk the extractor so its active state
            // stays in sync with the replay, but don't step the engine.
            let _ = ex.extract_turn(tv);
            continue;
        }

        let [mut p1c, mut p2c] = ex.extract_turn(tv);
        pad_with_pass(&mut p1c, active_count);
        pad_with_pass(&mut p2c, active_count);

        if !ended_flag {
            let r = b.step(&p1c, &p2c);
            ended_flag = matches!(r, StepResult::Ended { .. });
            turns_run += 1;
        }

        let score = score_turn(&b, tv, tolerance, &replay_status);
        if score.agreed {
            agreed_count += 1;
        }
        per_turn.push(score);
    }

    let agreement_pct = if per_turn.is_empty() {
        0.0
    } else {
        agreed_count as f32 / per_turn.len() as f32
    };

    Ok(ReplayScore {
        replay_id: replay.id.clone(),
        per_turn,
        agreement_pct,
        turns_run,
    })
}

fn pad_with_pass(v: &mut Vec<Choice>, n: usize) {
    let actor_slots_used: Vec<u8> = v.iter().map(|c| c.actor_slot()).collect();
    for slot in 0..n as u8 {
        if !actor_slots_used.contains(&slot) {
            v.push(Choice::Pass { actor_slot: slot });
        }
    }
    // Sort by actor_slot so the engine sees a predictable order.
    v.sort_by_key(|c| c.actor_slot());
}

fn score_turn(
    b: &vgc_engine_core::Battle,
    tv: &crate::replay::TurnView<'_>,
    tol: f32,
    replay_status: &[[Status; 2]; 2],
) -> TurnScore {
    use vgc_engine_core::SideRef;

    let trace = hp_trace(tv.events);
    let mut hp_l1: f32 = 0.0;
    let mut compared: u8 = 0;
    let mut faint_diverged = false;
    let mut status_diverged = false;

    for (side_idx, side_ref) in [SideRef::P1, SideRef::P2].iter().enumerate() {
        let side = match side_ref {
            SideRef::P1 => &b.p1,
            SideRef::P2 => &b.p2,
        };
        for slot in 0..2u8 {
            let last = trace.iter().rfind(|e| {
                e.slot.player as usize == side_idx + 1
                    && slot_letter_idx(e.slot.slot) == Some(slot)
            });
            let Some(replay_hp) = last else { continue };
            let Some(mon) = side.active_mon(slot as usize) else {
                // Engine slot empty; replay said something — treat as a
                // partial divergence the L1 captures.
                if !replay_hp.fainted {
                    hp_l1 += 1.0;
                    compared += 1;
                }
                continue;
            };
            let engine_frac = mon.current_hp as f32 / mon.stats.hp.max(1) as f32;
            hp_l1 += (engine_frac - replay_hp.fraction).abs();
            compared += 1;
            // Faint mismatch tanks the agreement regardless of L1.
            if replay_hp.fainted == mon.is_alive() {
                faint_diverged = true;
            }
            // Status comparison: only when the mon is alive on both sides
            // (a fainted mon's `status` is meaningless).
            if !replay_hp.fainted
                && mon.is_alive()
                && replay_status[side_idx][slot as usize] != mon.status
            {
                status_diverged = true;
            }
        }
    }

    let per_slot_l1 = if compared == 0 {
        f32::NAN
    } else {
        hp_l1 / compared as f32
    };
    let hp_diverged = compared > 0 && !per_slot_l1.is_nan() && per_slot_l1 > tol;
    let agreed = compared > 0 && !faint_diverged && !status_diverged && !hp_diverged;

    TurnScore {
        turn: tv.number,
        hp_l1: per_slot_l1,
        agreed,
        compared_slots: compared,
        ended: false,
        status_diverged,
        faint_diverged,
        hp_diverged,
    }
}

/// Walk events and update the per-(side,slot) persistent status state.
/// Status set by `|-status|`, cleared by `|-curestatus|` / `|faint|`.
/// `|switch|` / `|drag|` reset the slot's status because a different
/// mon is now there (its actual status will be re-asserted by a
/// subsequent `|-status|` if non-clean).
fn update_replay_status(state: &mut [[Status; 2]; 2], events: &[Event]) {
    for ev in events {
        let (slot, new_status, set) = match ev {
            Event::Status { slot, status } => (slot, parse_status(status), true),
            Event::CureStatus { slot, .. } => (slot, Some(Status::None), true),
            Event::Faint(slot) => (slot, Some(Status::None), true),
            Event::Switch { slot, .. } | Event::Drag { slot, .. } => {
                (slot, Some(Status::None), true)
            }
            _ => continue,
        };
        if !set {
            continue;
        }
        let Some(side_idx) = (match slot.player {
            1 => Some(0usize),
            2 => Some(1usize),
            _ => None,
        }) else {
            continue;
        };
        let slot_idx = match slot.slot {
            'a' => 0usize,
            'b' => 1usize,
            _ => continue,
        };
        if let Some(s) = new_status {
            state[side_idx][slot_idx] = s;
        }
    }
}

fn parse_status(code: &str) -> Option<Status> {
    Some(match code {
        "slp" => Status::Sleep,
        "frz" => Status::Freeze,
        "par" => Status::Paralysis,
        "brn" => Status::Burn,
        "psn" => Status::Poison,
        "tox" => Status::Toxic,
        _ => return None,
    })
}

fn slot_letter_idx(c: char) -> Option<u8> {
    match c {
        'a' => Some(0),
        'b' => Some(1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recon::CanonicalDefault;

    const SAMPLE: &str = include_str!("../tests/fixtures/sample.json");

    #[test]
    fn score_fixture_produces_per_turn_results() {
        let r = Replay::from_json(SAMPLE).unwrap();
        let score = score_replay(&r, &CanonicalDefault, 0xDEADBEEF, DEFAULT_HP_TOLERANCE)
            .expect("score");
        // 6 turns in the fixture; expect 6 per-turn entries.
        assert_eq!(score.per_turn.len(), 6);
        assert_eq!(score.turns_run as usize, score.per_turn.len());
        // Every turn's compared_slots should be > 0 — the fixture has
        // active mons throughout.
        assert!(score.per_turn.iter().all(|t| t.compared_slots > 0));
        // Agreement % is in [0, 1].
        assert!(score.agreement_pct >= 0.0 && score.agreement_pct <= 1.0);
    }
}
