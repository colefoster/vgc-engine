//! Solve over several sampled worlds (determinizations of hidden info) and
//! pick our action by expected value across them.
//!
//! The opponent's unrevealed sets are hidden. One belief battle is one
//! guess; a hidden Focus Sash or priority move can flip the best action.
//! [`endgame_solve_worlds`] runs the bounded anytime solve on each world
//! inside one total budget and scores each of our root joint actions by its
//! weighted mean value across worlds, where a world's value for an action is
//! its payoff against that world's equilibrium opponent strategy
//! ([`RootValues`]). The opponent knows its own set, so it plays the
//! equilibrium of the world it is in.
//!
//! The max-min ("robust") action is reported too, but the chosen action is
//! the expected-value argmax.
//!
//! With one world the result reproduces the plain bounded anytime solve:
//! the same node, and `chosen` is the argmax of its own-side policy (every
//! Nash support action scores the node value, so the policy-probability
//! tie-break decides).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use vgc_engine_core::{Battle, Choice, SideRef};

use crate::recursive::{endgame_solve_bounded, BoundedSolve, RootValues, SolverConfig, SolverStats, Truncation};

/// Expected values closer than this are a tie (LP round-off is ~1e-12, the
/// double-oracle convergence tolerance 1e-9).
const EV_TIE: f64 = 1e-6;

/// One world's solve.
#[derive(Debug, Clone)]
pub struct WorldResult {
    pub weight: f64,
    pub solve: BoundedSolve,
    pub stats: SolverStats,
    pub elapsed: Duration,
    /// The time this world was given (its share of what was left).
    pub budget: Option<Duration>,
}

/// Result of [`endgame_solve_worlds`]. Every value here except the per-world
/// solves is from `side`'s perspective (higher is better for `side`).
#[derive(Debug, Clone)]
pub struct WorldsSolve {
    pub side: SideRef,
    pub worlds: Vec<WorldResult>,
    /// Worlds that produced root values and entered the aggregate.
    pub worlds_used: usize,
    /// Weighted mean value of each of our joint actions over the used worlds
    /// (actions legal in every used world, in the first used world's order).
    pub ev: Vec<(Vec<Choice>, f64)>,
    /// Weighted mean of the used worlds' own-side Nash policies.
    pub policy_mix: Vec<(Vec<Choice>, f64)>,
    /// The expected-value argmax; `None` when no world produced root values.
    pub chosen: Option<Vec<Choice>>,
    pub chosen_ev: Option<f64>,
    /// Max-min over the used worlds: the action with the best worst world.
    pub robust: Option<(Vec<Choice>, f64)>,
    /// The first truncated world's reason, if any world was cut short.
    pub truncated: Option<Truncation>,
}

/// Solve each `(battle, weight)` world with the bounded anytime solver
/// inside `cfg`'s total budget and aggregate for `side`.
///
/// Budget: world `i` of `K` gets `1/(K-i)` of the time left before
/// `cfg.deadline` (so time a fast world leaves flows to the later ones) and
/// `cfg.replay_budget / K` replays. The total therefore respects the
/// deadline exactly as a single bounded solve does.
pub fn endgame_solve_worlds(
    worlds: &[(&Battle, f64)],
    side: SideRef,
    cfg: &SolverConfig,
    mut leaf: impl FnMut(&Battle) -> f64,
) -> WorldsSolve {
    let k = worlds.len();
    let mut out: Vec<WorldResult> = Vec::with_capacity(k);
    for (i, &(battle, weight)) in worlds.iter().enumerate() {
        let now = Instant::now();
        let budget = cfg.deadline.map(|d| d.saturating_duration_since(now) / (k - i) as u32);
        let mut wcfg = cfg.clone();
        wcfg.deadline = budget.map(|b| now + b);
        wcfg.replay_budget = cfg.replay_budget.map(|r| r / k as u64);
        let mut tt = HashMap::new();
        let mut stats = SolverStats::default();
        let solve = endgame_solve_bounded(battle, &wcfg, &mut leaf, &mut tt, &mut stats, true);
        out.push(WorldResult { weight, solve, stats, elapsed: now.elapsed(), budget });
    }
    aggregate(out, side)
}

fn own(rv: &RootValues, side: SideRef) -> &[(Vec<Choice>, f64)] {
    if side == SideRef::P1 { &rv.row } else { &rv.col }
}

/// The EV aggregation of [`endgame_solve_worlds`], split out for tests.
pub fn aggregate(worlds: Vec<WorldResult>, side: SideRef) -> WorldsSolve {
    let sign = if side == SideRef::P1 { 1.0 } else { -1.0 };
    let truncated = worlds.iter().find_map(|w| w.solve.truncated);
    let used: Vec<&WorldResult> = worlds.iter().filter(|w| w.solve.root_values.is_some() && w.weight > 0.0).collect();
    let mut res = WorldsSolve {
        side,
        worlds_used: used.len(),
        ev: Vec::new(),
        policy_mix: Vec::new(),
        chosen: None,
        chosen_ev: None,
        robust: None,
        truncated,
        worlds: Vec::new(),
    };
    if used.is_empty() {
        res.worlds = worlds;
        return res;
    }
    let total_w: f64 = used.iter().map(|w| w.weight).sum();
    let first = own(used[0].solve.root_values.as_ref().unwrap(), side);
    // Per action: (ev, worst world, policy mass). Actions missing from any
    // used world (a belief changed our legal set) are dropped.
    let mut rows: Vec<(Vec<Choice>, f64, f64, f64)> = Vec::new();
    'act: for (a, _) in first {
        let (mut ev, mut worst, mut mass) = (0.0, f64::INFINITY, 0.0);
        for w in &used {
            let vals = own(w.solve.root_values.as_ref().unwrap(), side);
            let Some(v) = vals.iter().find(|(b, _)| b == a).map(|x| sign * x.1) else { continue 'act };
            ev += w.weight * v;
            worst = worst.min(v);
            let pol = if side == SideRef::P1 { &w.solve.node.row_joint_policy } else { &w.solve.node.col_joint_policy };
            mass += w.weight * pol.iter().find(|(b, _)| b == a).map_or(0.0, |x| x.1);
        }
        rows.push((a.clone(), ev / total_w, worst, mass / total_w));
    }
    if rows.is_empty() {
        res.worlds = worlds;
        return res;
    }
    // Last tie-break: position in the first used world's policy, so one
    // world picks exactly what `max(policy, key=prob)` picks.
    let pol0 = if side == SideRef::P1 { &used[0].solve.node.row_joint_policy } else { &used[0].solve.node.col_joint_policy };
    let rank = |a: &Vec<Choice>| pol0.iter().position(|(b, _)| b == a).unwrap_or(usize::MAX);
    let best_ev = rows.iter().map(|r| r.1).fold(f64::NEG_INFINITY, f64::max);
    let chosen = rows
        .iter()
        .filter(|r| r.1 >= best_ev - EV_TIE)
        .min_by(|x, y| y.3.total_cmp(&x.3).then(rank(&x.0).cmp(&rank(&y.0))))
        .unwrap();
    res.chosen = Some(chosen.0.clone());
    res.chosen_ev = Some(chosen.1);
    let best_worst = rows.iter().map(|r| r.2).fold(f64::NEG_INFINITY, f64::max);
    let robust = rows
        .iter()
        .filter(|r| r.2 >= best_worst - EV_TIE)
        .max_by(|x, y| x.1.total_cmp(&y.1).then(x.3.total_cmp(&y.3)))
        .unwrap();
    res.robust = Some((robust.0.clone(), robust.2));
    res.policy_mix = rows.iter().filter(|r| r.3 > 0.0).map(|r| (r.0.clone(), r.3)).collect();
    res.ev = rows.into_iter().map(|r| (r.0, r.1)).collect();
    res.worlds = worlds;
    res
}
