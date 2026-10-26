//! Shallow "safe KO" search: find our joint action that, against every
//! opponent joint reply, knocks out at least one opposing mon within the
//! horizon while none of ours faints.
//!
//! This is a different objective from the endgame solver. The endgame
//! solver proves a game result; full proofs rarely finish outside 2v2.
//! Here the search stops at the first "solution" (Cole, 2026-10-26: an
//! opposing mon is knocked out and ours isn't), so it can run at any point
//! in the game.
//!
//! Value of a state at horizon `h` for `side` (`own`, `opp` = the fainted
//! counts at the search root):
//!
//! - an outcome where one of ours fainted: 0, whatever else happened;
//! - otherwise, an outcome where an opposing mon fainted: 1 (stop);
//! - otherwise, with turns left: `max_a min_b E[value]` one turn deeper;
//! - otherwise 0.
//!
//! `max_a min_b` is over pure joint actions: the opponent replies knowing
//! our action, the worst case. Chance (rolls, crits, accuracy, speed ties,
//! secondaries) is the outcome frontier's expectation.
//!
//! **Chance compression is conservative here.** A compressed cell drops
//! low-probability branches and renormalizes. The dropped mass counts as
//! failure: a cell's value is `(1 - dropped) * renormalized value`, a lower
//! bound on the true probability. The 3-roll collapse of unsegmented damage
//! sites (spread moves, multi-hit) has no such bound, as in the endgame
//! solver; segmented single-target hits keep KO / not exact.
//!
//! **Budget.** The same deadline / replay budget as the endgame solvers
//! ([`crate::budget`]). Horizons deepen 1, 2, …; a horizon cut short is
//! discarded and the deepest completed one is returned with `truncated` set.
//! Nothing completed = no result.

use std::collections::HashMap;
use std::time::Instant;

use vgc_engine_core::{Battle, Choice, SideRef};

use crate::budget::{self, BudgetGuard, StopReason};
use crate::compress::{enumerate_outcomes_compressed, ChanceCompression};
use crate::recursive::{joint_actions, Truncation};
use crate::{enumerate_outcomes_with, EnumerateOpts};

/// Knobs for [`solve_safe_ko`].
#[derive(Debug, Clone)]
pub struct SafeKoConfig {
    /// Turns of lookahead (1 = this turn only). Deepened 1..=horizon.
    pub horizon: u32,
    /// An action passes when its worst-case probability is at least this.
    pub threshold: f64,
    pub deadline: Option<Instant>,
    pub replay_budget: Option<u64>,
    /// `None` = the lossless enumerator (with the default auto-lossy
    /// threshold, as the endgame solver).
    pub chance_compression: Option<ChanceCompression>,
    pub record_seed: u64,
}

impl Default for SafeKoConfig {
    fn default() -> Self {
        Self {
            horizon: 2,
            threshold: 0.9,
            deadline: None,
            replay_budget: None,
            chance_compression: Some(ChanceCompression { mass_eps: 0.01, max_combos: 64 }),
            record_seed: 0xC0_DE,
        }
    }
}

/// One of our root joint actions at the completed horizon.
#[derive(Debug, Clone)]
pub struct SafeKoAction {
    pub joint: Vec<Choice>,
    /// Exact (up to the compression bound) when `passed`; otherwise an upper
    /// bound below the threshold (the search stops refuting it there).
    pub p: f64,
    pub passed: bool,
    /// The opponent reply that pushed it below the threshold (failed only).
    pub refutation: Option<Vec<Choice>>,
}

#[derive(Debug, Clone)]
pub struct SafeKoResult {
    pub side: SideRef,
    /// 0 = not even horizon 1 finished: no result.
    pub horizon_completed: u32,
    /// Every root action, in `joint_actions` order (empty when nothing finished).
    pub actions: Vec<SafeKoAction>,
    /// The passing action with the highest probability (ties: first in order).
    pub chosen: Option<Vec<Choice>>,
    pub chosen_p: Option<f64>,
    /// A deeper horizon (or horizon 1) was cut short.
    pub truncated: Option<Truncation>,
    pub cells: u64,
    pub chance_mass_dropped_max: f64,
}

fn fainted(b: &Battle, side: SideRef) -> usize {
    b.side(side).team.iter().filter(|m| m.fainted || m.current_hp == 0).count()
}

struct Search<'a> {
    cfg: &'a SafeKoConfig,
    side: SideRef,
    own0: usize,
    opp0: usize,
    /// (canonical hash, horizon) -> exact inner-node value.
    tt: HashMap<(u64, u32), f64>,
    /// Opponent replies that refuted something, tried first (keyed by the
    /// joint's Debug text: `Choice` is not `Hash`).
    history: HashMap<String, u32>,
    cells: u64,
    dropped_max: f64,
}

/// `None` = the budget tripped.
type Val = Option<f64>;

impl<'a> Search<'a> {
    fn opp(&self) -> SideRef {
        if self.side == SideRef::P1 { SideRef::P2 } else { SideRef::P1 }
    }

    fn status(&self, b: &Battle) -> Option<f64> {
        if fainted(b, self.side) > self.own0 {
            Some(0.0)
        } else if fainted(b, self.opp()) > self.opp0 {
            Some(1.0)
        } else if b.is_terminal() || b.needs_replacements() {
            // No faint but no ordinary next turn (a non-faint forced
            // switch, e.g. Eject Button): counted as no KO, conservatively.
            Some(0.0)
        } else {
            None
        }
    }

    fn ordered_replies(&self, replies: &[Vec<Choice>]) -> Vec<usize> {
        let score: Vec<u32> = replies.iter().map(|r| self.history.get(&format!("{r:?}")).copied().unwrap_or(0)).collect();
        let mut idx: Vec<usize> = (0..replies.len()).collect();
        idx.sort_by_key(|&j| std::cmp::Reverse(score[j]));
        idx
    }

    /// `max_a min_b`, exact (inner nodes).
    fn node(&mut self, b: &Battle, h: u32) -> Val {
        if h == 0 {
            return Some(0.0);
        }
        let key = (b.canonical_hash(), h);
        if let Some(&v) = self.tt.get(&key) {
            return Some(v);
        }
        let ours = joint_actions(b, self.side);
        let theirs = joint_actions(b, self.opp());
        let mut best = 0.0f64;
        for a in &ours {
            let (v, _) = self.action(b, a, &theirs, h, best)?;
            if v > best {
                best = v;
                if best >= 1.0 - 1e-12 {
                    break;
                }
            }
        }
        self.tt.insert(key, best);
        Some(best)
    }

    /// `min_b E[value]` for our action `a`. Returns early with an upper
    /// bound `<= cutoff` (and the refuting reply) once it can't beat `cutoff`.
    fn action(&mut self, b: &Battle, a: &[Choice], theirs: &[Vec<Choice>], h: u32, cutoff: f64) -> Option<(f64, Option<Vec<Choice>>)> {
        let mut m = 1.0f64;
        for j in self.ordered_replies(theirs) {
            let c = self.cell(b, a, &theirs[j], h, cutoff)?;
            if c < m {
                m = c;
            }
            if m <= cutoff {
                *self.history.entry(format!("{:?}", theirs[j])).or_insert(0) += 1;
                return Some((m, Some(theirs[j].clone())));
            }
        }
        Some((m, None))
    }

    /// Expected value of one joint pair; stops at an upper bound once it
    /// can't exceed `stop_at`.
    fn cell(&mut self, b: &Battle, a: &[Choice], r: &[Choice], h: u32, stop_at: f64) -> Val {
        if budget::check().is_some() {
            return None;
        }
        self.cells += 1;
        let (p1, p2) = if self.side == SideRef::P1 { (a, r) } else { (r, a) };
        let (mut frontier, dropped) = match self.cfg.chance_compression {
            Some(cc) => enumerate_outcomes_compressed(b, p1, p2, self.cfg.record_seed, cc),
            None => (enumerate_outcomes_with(b, p1, p2, self.cfg.record_seed,
                                             EnumerateOpts { lossy_damage_3bucket: false, auto_lossy_damage_threshold: Some(1_000) }), 0.0),
        };
        if budget::check().is_some() || frontier.outcomes.is_empty() {
            return None;
        }
        self.dropped_max = self.dropped_max.max(dropped);
        let keep = 1.0 - dropped;
        // Most probable outcomes first: the bound tightens fastest.
        frontier.outcomes.sort_by(|x, y| y.prob.total_cmp(&x.prob));
        let total: f64 = frontier.outcomes.iter().map(|o| o.prob).sum();
        let (mut acc, mut rest) = (0.0, 1.0);
        for o in &frontier.outcomes {
            let p = o.prob / total;
            let v = match self.status(&o.battle) {
                Some(v) => v,
                None => self.node(&o.battle, h - 1)?,
            };
            acc += p * v;
            rest -= p;
            if keep * (acc + rest.max(0.0)) <= stop_at {
                return Some(keep * (acc + rest.max(0.0)));
            }
        }
        Some(keep * acc)
    }
}

/// See the module docs. `side` is whose action is searched.
pub fn solve_safe_ko(battle: &Battle, side: SideRef, cfg: &SafeKoConfig) -> SafeKoResult {
    let _budget = BudgetGuard::install(cfg.deadline, cfg.replay_budget);
    let mut s = Search {
        cfg,
        side,
        own0: fainted(battle, side),
        opp0: fainted(battle, if side == SideRef::P1 { SideRef::P2 } else { SideRef::P1 }),
        tt: HashMap::new(),
        history: HashMap::new(),
        cells: 0,
        dropped_max: 0.0,
    };
    let mut res = SafeKoResult {
        side,
        horizon_completed: 0,
        actions: Vec::new(),
        chosen: None,
        chosen_p: None,
        truncated: None,
        cells: 0,
        chance_mass_dropped_max: 0.0,
    };
    if battle.is_terminal() || battle.needs_replacements() {
        return res;
    }
    let ours = joint_actions(battle, side);
    let theirs = joint_actions(battle, s.opp());
    // Root actions are refuted at the threshold, not at the best so far, so
    // every action's pass / fail is known (the caller checks its own pick).
    let cutoff = cfg.threshold - 1e-9;
    let mut cut = false;
    'horizon: for h in 1..=cfg.horizon.max(1) {
        let mut acts = Vec::with_capacity(ours.len());
        for a in &ours {
            match s.action(battle, a, &theirs, h, cutoff) {
                Some((p, refutation)) => {
                    let passed = p > cutoff;
                    acts.push(SafeKoAction { joint: a.clone(), p, passed, refutation: if passed { None } else { refutation } });
                }
                None => {
                    cut = true;
                    break 'horizon;
                }
            }
        }
        res.horizon_completed = h;
        res.actions = acts;
    }
    res.truncated = match (cut, budget::check()) {
        (true, Some(StopReason::ReplayBudget)) => Some(Truncation::ReplayBudget),
        (true, _) => Some(Truncation::Deadline),
        (false, _) => None,
    };
    let mut best: Option<&SafeKoAction> = None;
    for x in res.actions.iter().filter(|x| x.passed) {
        if best.map_or(true, |b| x.p > b.p + 1e-12) {
            best = Some(x);
        }
    }
    res.chosen = best.map(|x| x.joint.clone());
    res.chosen_p = best.map(|x| x.p);
    res.cells = s.cells;
    res.chance_mass_dropped_max = s.dropped_max;
    res
}
