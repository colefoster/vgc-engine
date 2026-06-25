//! Deterministic "should the secondary block run at all?" predicate,
//! extracted from `Battle::resolve_move_with_pending`.
//!
//! Phase A / third helper of the `resolve_move_with_pending` state-machine
//! refactor (see `docs/resolve-move-restructure-plan.md`). Pure / read-only:
//! given the attacker / target snapshots, the move, and the per-hit
//! damage-application bookkeeping (`alive_post`, `hit_sub`), decide whether
//! the caller should enter the secondary-effect dispatch — `apply_secondary_effect`
//! — at all. It does NOT touch RNG and does NOT mutate `Battle`; the per-
//! secondary percent-roll, Serene Grace doubling, Covert Cloak ablation,
//! Inner Focus / Safeguard / Own Tempo / Clear Body vetoes, Substitute-
//! survivor draws, status-immunity gates, King's Rock appended-flinch
//! draw, and Sheer Force's self-secondary skip ALL live inside
//! `apply_secondary_effect` (interleaved with `percent_1_100()` draws so
//! the PsGen5 oracle stays aligned site-for-site).
//!
//! Scope intentionally narrow:
//!   1. **Sheer Force ablation** — attacker has Sheer Force AND the move has
//!      a Sheer-Force-boosted secondary set. PS `data/abilities.ts:sheerforce`
//!      `onModifyMove` clears `move.secondaries` before the secondary block
//!      runs, so the `randomChance` roll never fires. Predicate matches the
//!      BP-boost predicate in `damage.rs` exactly.
//!   2. **Target faint** — `alive_post == false`: the target died to the hit,
//!      so no defender-targeted secondary lands (PS's `target.hp > 0` guard).
//!   3. **Substitute absorption** — `hit_sub == true`: the hit was absorbed
//!      by a Substitute (sound + Infiltrator already cleared the flag
//!      upstream), so opposing secondaries don't land. PS
//!      `sim/battle-actions.ts` short-circuits the secondary block when the
//!      hit went to the sub.
//!
//! The remaining "should this individual secondary proc?" predicates
//! (Shield Dust, Covert Cloak, status immunity, Safeguard, Inner Focus,
//! Clear Body / Hyper Cutter / Big Pecks / Keen Eye, Misty Terrain vs
//! confusion, Own Tempo, King's Rock dedupe vs native flinch, …) sit
//! INSIDE `apply_secondary_effect` and are deliberately interleaved with
//! the per-secondary `percent_1_100()` draws — PS draws unconditionally
//! and then vetoes the volatile, so moving those checks here would
//! change the draw count and break PsGen5 oracle alignment. Future
//! per-secondary extractions need to preserve that draw / veto ordering
//! site-for-site; that's a larger refactor than Phase A.

use crate::battle::Battle;
use crate::damage;
use crate::pokemon::Pokemon;
use vgc_engine_data as data;

/// Outcome of the pre-block secondary-effect gate.
///
/// `Skip` means the caller must NOT invoke `apply_secondary_effect` and
/// must NOT consume any RNG — the entire secondary block is suppressed.
///
/// `Run` means the caller should enter `apply_secondary_effect`, which
/// will internally draw one `percent_1_100()` per secondary against the
/// (Serene-Grace-doubled) `chance` and apply per-secondary vetoes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecondaryProcDecision {
    Skip,
    Run,
}

/// Deterministic gate on the move's secondary-effect block. See module
/// docs for scope. Read-only on `Battle`; the caller owns RNG.
///
/// Inputs mirror the inline call site exactly:
/// - `attacker` is the cloned snapshot taken at the top of the per-target
///   loop; it is what `attacker_has_sheer_force` reads.
/// - `move_def` is the resolved `MoveDef` for the move being applied
///   (the same `m` the caller already has in scope).
/// - `alive_post` is the post-damage liveness check the caller computes
///   from `self.side(tside).active_mon(tslot)`.
/// - `hit_sub` is the substitute-absorption flag the caller computed at
///   damage application time.
pub(crate) fn should_run_secondary_block(
    _battle: &Battle,
    attacker: &Pokemon,
    move_def: &data::MoveDef,
    alive_post: bool,
    hit_sub: bool,
) -> SecondaryProcDecision {
    if !alive_post || hit_sub {
        return SecondaryProcDecision::Skip;
    }
    let sheer_force_strip = damage::attacker_has_sheer_force(attacker)
        && damage::move_is_sheer_force_boosted(move_def);
    if sheer_force_strip {
        return SecondaryProcDecision::Skip;
    }
    SecondaryProcDecision::Run
}
