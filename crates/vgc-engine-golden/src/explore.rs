//! Exploratory comparison mode (PR-203).
//!
//! The strict golden gate (HP-exact) is too noisy on random-play
//! goldens to drive triage — at N=50 most random goldens fail and the
//! per-PR signal drowns in damage-roll-ordering noise. This module runs
//! a coarser comparison that ignores HP values entirely and only checks
//! **structural** events per turn:
//!
//!   * `faint` — did the same slot faint this turn on both sides?
//!   * `status` — did the same slot gain the same status this turn?
//!   * `miss`  — did the same actor miss its target this turn?
//!   * `damage`— did the same slot take *any* damage this turn?
//!
//! Output is a list of per-turn divergences with `engine_value` /
//! `ps_value` strings; an aggregator example (`examples/explore.rs`)
//! tallies frequencies into a punch list.
//!
//! Move-choice and switch-choice are NOT compared because the engine's
//! actions are derived from the PS event log (see
//! `derive_turns_from_events`) — they always match by construction.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use vgc_engine_core::{
    Battle, BattleConfig, Rng, SideRef, Status, StepResult, TeamBuilder,
};

use crate::{
    derive_turns_from_events, lower_rng_events, parse_format, parse_turn_actions,
    status_str, GoldenError, GoldenInput, GoldenTurn, PsEvent, PsOutput,
};

#[derive(Debug, Serialize, Clone)]
pub struct ExploreDivergence {
    pub turn: u32,
    /// One of: "faint", "status", "miss", "damage".
    pub kind: String,
    /// Side + slot the divergence is on (e.g. "p1a", "p2b").
    pub actor: String,
    /// Free-form label used by the aggregator to bucket fine-grained
    /// causes (status name for `status`, move name for `miss`,
    /// species for `faint`/`damage`).
    pub label: String,
    /// PS view: e.g. "fainted", "slp", "missed", "took_damage".
    pub ps_value: String,
    /// Engine view: e.g. "alive", "none", "hit", "no_damage".
    pub engine_value: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ExploreReport {
    pub name: String,
    pub turns_run: u32,
    pub structural_match: bool,
    pub divergences: Vec<ExploreDivergence>,
}

pub fn run_explore(input_path: &Path, ps_path: &Path) -> Result<ExploreReport, GoldenError> {
    let input_bytes = std::fs::read(input_path).map_err(GoldenError::Io)?;
    let input: GoldenInput =
        serde_json::from_slice(&input_bytes).map_err(GoldenError::Json)?;
    let ps_bytes = std::fs::read(ps_path).map_err(GoldenError::Io)?;
    let ps: PsOutput = serde_json::from_slice(&ps_bytes).map_err(GoldenError::Json)?;
    if !ps.ok {
        return Err(GoldenError::PsNotOk);
    }
    run_explore_in_memory(&input, &ps)
}

pub fn run_explore_in_memory(
    input: &GoldenInput,
    ps: &PsOutput,
) -> Result<ExploreReport, GoldenError> {
    let format = parse_format(&input.format)?;
    let p1_team = TeamBuilder::from_showdown_text(&input.p1.team)
        .map_err(|e| GoldenError::TeamParse(format!("p1: {e:?}")))?;
    let p2_team = TeamBuilder::from_showdown_text(&input.p2.team)
        .map_err(|e| GoldenError::TeamParse(format!("p2: {e:?}")))?;
    let active_count = format.active_count();

    let derived_turns: Vec<GoldenTurn>;
    let turns_ref: &[GoldenTurn] = if input.random_play && input.turns.is_empty() {
        derived_turns = derive_turns_from_events(
            &ps.events,
            &p1_team,
            &p2_team,
            active_count,
            input.max_turns.unwrap_or(30),
        );
        &derived_turns
    } else {
        &input.turns
    };

    let events = lower_rng_events(&ps.rng);
    let fallback_seed = u64::from(input.seed[0])
        | (u64::from(input.seed[1]) << 16)
        | (u64::from(input.seed[2]) << 32)
        | (u64::from(input.seed[3]) << 48);
    let rng = Rng::oracle_partial(events, fallback_seed);
    let cfg = BattleConfig { format, seed: fallback_seed };
    let mut battle = Battle::with_rng(cfg, rng, p1_team, p2_team);

    let mut report = ExploreReport {
        name: input.name.clone().unwrap_or_else(|| "<unnamed>".into()),
        turns_run: 0,
        structural_match: true,
        divergences: Vec::new(),
    };

    // Pre-step snapshots, keyed by (side, slot_char).
    let mut prev: BTreeMap<(u8, char), SlotState> = engine_state(&battle, active_count);

    let mut ended = false;
    for (i, turn) in turns_ref.iter().enumerate() {
        let turn_no = (i + 1) as u32;
        if ended {
            break;
        }
        let p1c = parse_turn_actions(&turn.p1, SideRef::P1, active_count)?;
        let p2c = parse_turn_actions(&turn.p2, SideRef::P2, active_count)?;
        let r = battle.step(&p1c, &p2c);
        ended = matches!(r, StepResult::Ended { .. });
        report.turns_run += 1;

        let after: BTreeMap<(u8, char), SlotState> = engine_state(&battle, active_count);

        // Engine deltas this turn.
        let mut engine_faint: BTreeMap<(u8, char), bool> = BTreeMap::new();
        let mut engine_status: BTreeMap<(u8, char), Option<String>> = BTreeMap::new();
        let mut engine_damage: BTreeMap<(u8, char), bool> = BTreeMap::new();
        for (k, after_s) in &after {
            let prev_s = prev.get(k).cloned().unwrap_or_default();
            let fainted_now = after_s.fainted && !prev_s.fainted;
            engine_faint.insert(*k, fainted_now);
            let status_change = if after_s.status != prev_s.status && after_s.status != "none" {
                Some(after_s.status.clone())
            } else {
                None
            };
            engine_status.insert(*k, status_change);
            engine_damage.insert(*k, after_s.hp < prev_s.hp);
        }

        // PS deltas this turn.
        let ps_turn = ps_turn_deltas(&ps.events, turn_no);

        // Compare faints.
        for (key, ps_fainted) in &ps_turn.faints {
            let eng_fainted = engine_faint.get(key).copied().unwrap_or(false);
            if eng_fainted != *ps_fainted {
                report.divergences.push(ExploreDivergence {
                    turn: turn_no,
                    kind: "faint".into(),
                    actor: format!("p{}{}", key.0, key.1),
                    label: ps_turn.species.get(key).cloned().unwrap_or_default(),
                    ps_value: if *ps_fainted { "fainted".into() } else { "alive".into() },
                    engine_value: if eng_fainted { "fainted".into() } else { "alive".into() },
                });
            }
        }
        // Engine fainted but PS didn't note it.
        for (key, eng_fainted) in &engine_faint {
            if !*eng_fainted {
                continue;
            }
            if !ps_turn.faints.get(key).copied().unwrap_or(false) {
                report.divergences.push(ExploreDivergence {
                    turn: turn_no,
                    kind: "faint".into(),
                    actor: format!("p{}{}", key.0, key.1),
                    label: ps_turn.species.get(key).cloned().unwrap_or_default(),
                    ps_value: "alive".into(),
                    engine_value: "fainted".into(),
                });
            }
        }

        // Compare statuses.
        for (key, ps_status) in &ps_turn.statuses {
            let eng_status = engine_status.get(key).cloned().unwrap_or(None);
            if eng_status.as_deref() != Some(ps_status.as_str()) {
                report.divergences.push(ExploreDivergence {
                    turn: turn_no,
                    kind: "status".into(),
                    actor: format!("p{}{}", key.0, key.1),
                    label: ps_status.clone(),
                    ps_value: ps_status.clone(),
                    engine_value: eng_status.unwrap_or_else(|| "none".into()),
                });
            }
        }
        // Engine applied a status PS didn't.
        for (key, eng_s) in &engine_status {
            if let Some(s) = eng_s {
                if !ps_turn.statuses.contains_key(key) {
                    report.divergences.push(ExploreDivergence {
                        turn: turn_no,
                        kind: "status".into(),
                        actor: format!("p{}{}", key.0, key.1),
                        label: s.clone(),
                        ps_value: "none".into(),
                        engine_value: s.clone(),
                    });
                }
            }
        }

        // Compare misses: PS recorded `-miss source target` this turn;
        // check if engine's target took damage anyway (false miss → hit
        // divergence). Conversely, PS recorded a hit (damage) but engine
        // target took none.
        for miss in &ps_turn.misses {
            // miss.0 = source actor "p1a", miss.1 = target "p2a", miss.2 = move name
            let Some(t_key) = parse_actor(&miss.1) else { continue };
            let eng_dmg = engine_damage.get(&t_key).copied().unwrap_or(false);
            if eng_dmg {
                report.divergences.push(ExploreDivergence {
                    turn: turn_no,
                    kind: "miss".into(),
                    actor: miss.0.clone(),
                    label: miss.2.clone(),
                    ps_value: "missed".into(),
                    engine_value: "hit".into(),
                });
            }
        }

        // Damage divergence: PS shows target HP dropped this turn (any),
        // but engine target took no damage; or vice versa. Skip targets
        // that fainted (already covered by faint divergence) and skip
        // targets where PS shows a miss (covered by miss divergence).
        for (key, ps_dmg) in &ps_turn.damaged {
            let eng_dmg = engine_damage.get(key).copied().unwrap_or(false);
            let key_was_missed = ps_turn.misses.iter().any(|m| {
                parse_actor(&m.1).as_ref() == Some(key)
            });
            if key_was_missed {
                continue;
            }
            if eng_dmg != *ps_dmg {
                report.divergences.push(ExploreDivergence {
                    turn: turn_no,
                    kind: "damage".into(),
                    actor: format!("p{}{}", key.0, key.1),
                    label: ps_turn.species.get(key).cloned().unwrap_or_default(),
                    ps_value: if *ps_dmg { "took_damage".into() } else { "no_damage".into() },
                    engine_value: if eng_dmg { "took_damage".into() } else { "no_damage".into() },
                });
            }
        }

        prev = after;
    }

    // RNG draw-balance check. The oracle harness silently lets per-call
    // misalignment compound: PS recorded N draws, engine consumed M ≠ N,
    // and every site after the first mismatch read the wrong value.
    // Surface that explicitly so misaligned goldens don't pollute the
    // damage / status frequency map with downstream noise.
    if let Some((engine_pops, _)) = battle.oracle_pops() {
        let ps_draws = ps.rng.len();
        if engine_pops != ps_draws {
            report.divergences.push(ExploreDivergence {
                turn: 0,
                kind: "rng-balance".into(),
                actor: "battle".into(),
                label: format!("delta={}", engine_pops as i64 - ps_draws as i64),
                ps_value: format!("ps_drew={ps_draws}"),
                engine_value: format!("engine_popped={engine_pops}"),
            });
        }
    }

    report.structural_match = report.divergences.is_empty();
    Ok(report)
}

#[derive(Debug, Default, Clone)]
struct SlotState {
    hp: u32,
    fainted: bool,
    status: String,
}

fn engine_state(battle: &Battle, active_count: usize) -> BTreeMap<(u8, char), SlotState> {
    let mut out = BTreeMap::new();
    for (side_ref, side_letter) in [(SideRef::P1, 1u8), (SideRef::P2, 2u8)] {
        let side = match side_ref {
            SideRef::P1 => &battle.p1,
            SideRef::P2 => &battle.p2,
        };
        for slot in 0..active_count {
            let Some(mon) = side.active_mon(slot) else { continue };
            let slot_char = if slot == 0 { 'a' } else { 'b' };
            out.insert((side_letter, slot_char), SlotState {
                hp: mon.current_hp as u32,
                fainted: mon.fainted,
                status: status_to_str(mon.status),
            });
        }
    }
    out
}

fn status_to_str(s: Status) -> String {
    status_str(s)
}

#[derive(Debug, Default)]
struct PsTurnDeltas {
    /// (side, slot) → fainted this turn
    faints: BTreeMap<(u8, char), bool>,
    /// (side, slot) → status that landed this turn
    statuses: BTreeMap<(u8, char), String>,
    /// (side, slot) → took damage this turn
    damaged: BTreeMap<(u8, char), bool>,
    /// (source_actor, target_actor, move_name) tuples for `-miss` events
    misses: Vec<(String, String, String)>,
    /// (side, slot) → species name of the mon in that slot
    species: BTreeMap<(u8, char), String>,
}

fn parse_actor(s: &str) -> Option<(u8, char)> {
    if s.len() < 3 { return None; }
    let bytes = s.as_bytes();
    if bytes[0] != b'p' { return None; }
    let side = (bytes[1] as char).to_digit(10)? as u8;
    let slot = bytes[2] as char;
    if slot != 'a' && slot != 'b' { return None; }
    Some((side, slot))
}

fn ps_turn_deltas(events: &[PsEvent], turn: u32) -> PsTurnDeltas {
    let mut out = PsTurnDeltas::default();
    // Track most recent move name per actor so `-miss` events that
    // omit move name can attribute it.
    let mut last_move_by_actor: BTreeMap<(u8, char), String> = BTreeMap::new();
    // Track current species per slot from cumulative `switch` events.
    let mut species_by_slot: BTreeMap<(u8, char), String> = BTreeMap::new();

    for ev in events {
        // Maintain species table from all earlier switches.
        if ev.kind == "switch" {
            if let Some(actor) = ev.actor.as_deref().and_then(parse_actor) {
                if let Some(sp) = &ev.species {
                    species_by_slot.insert(actor, sp.clone());
                }
            }
        }
        if ev.turn != turn {
            continue;
        }
        match ev.kind.as_str() {
            "move" => {
                if let Some(key) = ev.actor.as_deref().and_then(parse_actor) {
                    if let Some(name) = &ev.name {
                        last_move_by_actor.insert(key, name.clone());
                    }
                }
            }
            "faint" => {
                if let Some(key) = ev.actor.as_deref().and_then(parse_actor) {
                    out.faints.insert(key, true);
                }
            }
            "status" => {
                if let Some(key) = ev.actor.as_deref().and_then(parse_actor) {
                    if let Some(s) = &ev.status {
                        out.statuses.insert(key, s.clone());
                    }
                }
            }
            "damage" => {
                if let Some(key) = ev.actor.as_deref().and_then(parse_actor) {
                    out.damaged.insert(key, true);
                }
            }
            "miss" => {
                // PS driver emits `{ kind: 'miss', source, target }`;
                // PsEvent captures both via `source` (PR-203 addition)
                // and `target`. The most recent move per actor lets us
                // label which move missed.
                let source = ev.source.clone().unwrap_or_default();
                let target = ev.target.clone().unwrap_or_default();
                if !source.is_empty() && !target.is_empty() {
                    let move_name = parse_actor(&source)
                        .and_then(|k| last_move_by_actor.get(&k).cloned())
                        .unwrap_or_default();
                    out.misses.push((source, target, move_name));
                }
            }
            _ => {}
        }
    }

    // Populate species lookup for slots touched this turn.
    for key in out
        .faints
        .keys()
        .chain(out.statuses.keys())
        .chain(out.damaged.keys())
    {
        if let Some(sp) = species_by_slot.get(key) {
            out.species.insert(*key, sp.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PsEvent;

    fn ev(turn: u32, kind: &str, actor: Option<&str>) -> PsEvent {
        PsEvent {
            turn,
            kind: kind.into(),
            actor: actor.map(|s| s.into()),
            hp: None,
            max: None,
            from: None,
            status: None,
            stat: None,
            amount: None,
            species: None,
            name: None,
            target: None,
            source: None,
        }
    }

    #[test]
    fn ps_turn_deltas_picks_up_status_and_faint() {
        let mut events = Vec::new();
        let mut e1 = ev(1, "status", Some("p2a"));
        e1.status = Some("slp".into());
        events.push(e1);
        events.push(ev(1, "faint", Some("p1a")));
        events.push(ev(2, "faint", Some("p2b")));
        let d = ps_turn_deltas(&events, 1);
        assert_eq!(d.statuses.get(&(2, 'a')).cloned(), Some("slp".into()));
        assert_eq!(d.faints.get(&(1, 'a')).copied(), Some(true));
        assert_eq!(d.faints.get(&(2, 'b')).copied(), None);
    }

    #[test]
    fn ps_turn_deltas_records_miss_source_and_target() {
        let mut miss = ev(3, "miss", None);
        miss.source = Some("p1a".into());
        miss.target = Some("p2a".into());
        let mut mv = ev(3, "move", Some("p1a"));
        mv.name = Some("Hurricane".into());
        let events = vec![mv, miss];
        let d = ps_turn_deltas(&events, 3);
        assert_eq!(d.misses.len(), 1);
        assert_eq!(d.misses[0].0, "p1a");
        assert_eq!(d.misses[0].1, "p2a");
        assert_eq!(d.misses[0].2, "Hurricane");
    }
}
