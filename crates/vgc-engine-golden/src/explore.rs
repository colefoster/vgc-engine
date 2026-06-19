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

/// Which RNG strategy the engine uses for the differential run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreMode {
    /// Consume PS's recorded RNG queue via `Rng::oracle_partial`.
    /// Unmatched variants fall through to a Splitmix fallback seeded
    /// from `input.seed[0]`. This is the default — it lets the engine
    /// piggy-back on PS's draws without needing every draw site
    /// implemented.
    OraclePartial,
    /// Use the bit-exact PS Gen5 LCG (PR-209 / PR-220), constructed
    /// from `input.seed`. Engine and PS draw the same values at the
    /// same sites — no oracle queue. Surfaces draw-site MISALIGNMENT
    /// as direct divergence (engine consumes value X for site A, PS
    /// consumed value X for a different site, etc.). The goal state
    /// once all engine draw sites match PS.
    PsGen5,
}

pub fn run_explore(input_path: &Path, ps_path: &Path) -> Result<ExploreReport, GoldenError> {
    run_explore_with_mode(input_path, ps_path, ExploreMode::OraclePartial)
}

pub fn run_explore_with_mode(
    input_path: &Path,
    ps_path: &Path,
    mode: ExploreMode,
) -> Result<ExploreReport, GoldenError> {
    let input_bytes = std::fs::read(input_path).map_err(GoldenError::Io)?;
    let input: GoldenInput =
        serde_json::from_slice(&input_bytes).map_err(GoldenError::Json)?;
    let ps_bytes = std::fs::read(ps_path).map_err(GoldenError::Io)?;
    let ps: PsOutput = serde_json::from_slice(&ps_bytes).map_err(GoldenError::Json)?;
    if !ps.ok {
        return Err(GoldenError::PsNotOk);
    }
    run_explore_in_memory_with_mode(&input, &ps, mode)
}

pub fn run_explore_in_memory(
    input: &GoldenInput,
    ps: &PsOutput,
) -> Result<ExploreReport, GoldenError> {
    run_explore_in_memory_with_mode(input, ps, ExploreMode::OraclePartial)
}

pub fn run_explore_in_memory_with_mode(
    input: &GoldenInput,
    ps: &PsOutput,
    mode: ExploreMode,
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

    let fallback_seed = u64::from(input.seed[0])
        | (u64::from(input.seed[1]) << 16)
        | (u64::from(input.seed[2]) << 32)
        | (u64::from(input.seed[3]) << 48);
    let rng = match mode {
        ExploreMode::OraclePartial => {
            let events = lower_rng_events(&ps.rng);
            Rng::oracle_partial(events, fallback_seed)
        }
        ExploreMode::PsGen5 => Rng::ps_gen5(input.seed),
    };
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

    // Persistent per-MON HP, keyed by mon identity (side, party index),
    // NOT by raw slot. The slot-keyed `prev` above is wrong for damage
    // detection across a switch: on a switch turn the slot's occupant
    // changes, so the departed mon's HP would be compared against the
    // incoming mon's HP (PR-368). Tracking HP per identity lets us ask
    // the correct question — "did THIS mon lose HP this turn?" — by
    // comparing its post-step HP to its own last-recorded HP (its
    // baseline at the start of the turn). A mon appearing for the first
    // time baselines at its max HP, so any entry-hazard / residual chip
    // it takes the same turn it switches in is still counted as damage.
    let mut hp_by_id: BTreeMap<(u8, u8), u32> = BTreeMap::new();
    seed_hp_by_id(&battle, active_count, &mut hp_by_id);

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
            // "Did THIS mon lose HP this turn?" — compare the slot's
            // current occupant against its OWN last-recorded HP, keyed by
            // mon identity (side, party index), not by slot. Baseline for
            // a never-before-seen mon is its max HP, so chip taken the
            // same turn it switches in still registers (PR-368).
            let id = (k.0, after_s.party_idx);
            let baseline = hp_by_id.get(&id).copied().unwrap_or(after_s.max_hp);
            engine_damage.insert(*k, after_s.hp < baseline);
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
                // Prefer the PS `[from]` attribution as the label
                // (e.g. `"item: Sticky Barb"`, `"confusion"`) so the
                // aggregator bucket-sorts by the missing mechanic, not
                // by which mon happened to be in the slot. Fall back
                // to species when the damage was a direct move hit
                // (no `[from]` tag) — there the species frequency
                // tells us whose damage-roll path diverged.
                let label = ps_turn
                    .damaged_from
                    .get(key)
                    .and_then(|x| x.clone())
                    .or_else(|| ps_turn.species.get(key).cloned())
                    .unwrap_or_default();
                report.divergences.push(ExploreDivergence {
                    turn: turn_no,
                    kind: "damage".into(),
                    actor: format!("p{}{}", key.0, key.1),
                    label,
                    ps_value: if *ps_dmg { "took_damage".into() } else { "no_damage".into() },
                    engine_value: if eng_dmg { "took_damage".into() } else { "no_damage".into() },
                });
            }
        }

        // Record each on-field mon's post-step HP as the baseline for
        // next turn, keyed by mon identity. A mon that switches out keeps
        // its last on-field HP here, so when it returns later we compare
        // against the HP it left with — not a stale slot value.
        for (k, after_s) in &after {
            hp_by_id.insert((k.0, after_s.party_idx), after_s.hp);
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
    /// Party index of the mon occupying this slot — its identity within
    /// the side. A switch swaps this to a different value, which is how
    /// damage detection knows the slot changed hands (PR-368).
    party_idx: u8,
    /// Max HP of the slot occupant; the damage-detection baseline for a
    /// mon appearing on the field for the first time.
    max_hp: u32,
}

/// Seed the per-identity HP table with the starting (lead) mons so the
/// first turn's damage detection has a correct baseline for them.
fn seed_hp_by_id(
    battle: &Battle,
    active_count: usize,
    hp_by_id: &mut BTreeMap<(u8, u8), u32>,
) {
    for (side_ref, side_letter) in [(SideRef::P1, 1u8), (SideRef::P2, 2u8)] {
        let side = match side_ref {
            SideRef::P1 => &battle.p1,
            SideRef::P2 => &battle.p2,
        };
        for slot in 0..active_count {
            let Some(mon) = side.active_mon(slot) else { continue };
            let idx = side.active.get(slot).copied().unwrap_or(u8::MAX);
            hp_by_id.insert((side_letter, idx), mon.current_hp as u32);
        }
    }
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
            // Identity = party index of the slot occupant. A switch
            // swaps `side.active[slot]` to a different party index, so
            // a changed identity here means a new mon entered the slot.
            let occupant = side.active.get(slot).copied().unwrap_or(u8::MAX);
            out.insert((side_letter, slot_char), SlotState {
                hp: mon.current_hp as u32,
                fainted: mon.fainted,
                status: status_to_str(mon.status),
                party_idx: occupant,
                max_hp: mon.stats.hp as u32,
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
    /// (side, slot) → PS `[from]` attribution for the damage event
    /// (e.g. `"item: Sticky Barb"`, `"move: Steel Beam"`, `"confusion"`,
    /// `"psn"`). `None` when PS attributed the damage directly to a
    /// move (the normal `|-damage|` line has no `[from]`). Populated
    /// after PR-214 fixed the protocol parser to recognize `[from] X`
    /// as a single token.
    damaged_from: BTreeMap<(u8, char), Option<String>>,
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
                    // Last writer wins — if multiple damage events hit
                    // the same slot in a turn, keep the most recent
                    // attribution. Usually only one event per slot per
                    // turn except for multi-hit moves (where all hits
                    // share the same source) and chained residuals
                    // (e.g. Sticky Barb + Burn — keeping the later one
                    // is fine for triage).
                    out.damaged_from.insert(key, ev.from.clone());
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

    /// Mirror the in-loop damage rule: a mon "took damage" iff its
    /// post-step HP is below its own last-recorded baseline (or max HP
    /// for a first appearance), keyed by identity — never against the
    /// slot's previous, possibly-different, occupant.
    fn took_damage(hp_by_id: &BTreeMap<(u8, u8), u32>, id: (u8, u8), hp: u32, max_hp: u32) -> bool {
        let baseline = hp_by_id.get(&id).copied().unwrap_or(max_hp);
        hp < baseline
    }

    #[test]
    fn switch_in_does_not_count_as_damage() {
        // Slot p1a held party-idx 0 at 200/200; it switches OUT and
        // party-idx 2 (a different mon, max 300, currently 150/300)
        // switches IN. The incoming mon is at full relative to its own
        // baseline (never recorded → max 300), so 150 < 300 would be a
        // bug ONLY if 150 were a real loss. Here it entered at 150? No —
        // model a clean switch-in at full HP: incoming at 300/300.
        let mut hp_by_id: BTreeMap<(u8, u8), u32> = BTreeMap::new();
        hp_by_id.insert((1, 0), 200); // departed mon's last HP
        // Incoming idx 2, full HP 300, never recorded.
        assert!(
            !took_damage(&hp_by_id, (1, 2), 300, 300),
            "a full-HP switch-in must NOT register as damage"
        );

        // Same mon (idx 0) takes a real hit: 200 -> 150 against its
        // recorded baseline of 200.
        assert!(
            took_damage(&hp_by_id, (1, 0), 150, 200),
            "real HP loss on the same mon must register as damage"
        );

        // Switch-in that takes entry-hazard chip the same turn: incoming
        // idx 3, max 250, lands on Stealth Rock and ends at 220. Baseline
        // is its max (250), so 220 < 250 DOES count — we must not mask
        // real switch-in residual damage.
        assert!(
            took_damage(&hp_by_id, (1, 3), 220, 250),
            "entry-hazard chip on a switch-in must register as damage"
        );
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
