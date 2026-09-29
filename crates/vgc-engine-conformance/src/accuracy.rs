//! Accuracy-proof harness (docs/accuracy/2026-09-accuracy-proof.md).
//!
//! Consumes full PS battles captured by `tools/accuracy/ps-battle.js` and
//! replays them into the engine two ways:
//!
//! * **keyed** — every random outcome PS drew is injected by semantic key
//!   (`Rng::OracleKeyed`), so RNG is neutralized and any state divergence is
//!   either a keying gap (an engine draw PS never recorded under that key, or
//!   vice versa) or a mechanics / harness difference. Unlike the original
//!   conformance runner this plays through faint replacements and mid-turn
//!   pivots (`Battle::decision_phases`), so whole battles are compared.
//! * **ps-rng** (feature `ps-rng`) — the engine runs on PS's own PRNG with the
//!   same seed (`Rng::Ps`), no injection at all. Both sides trace every draw;
//!   the first draw whose call differs (args or site kind) separates
//!   draw-order divergences from mechanics divergences.
//!
//! Both modes compare end-of-turn state (after replacements) per active slot:
//! species, HP, fainted, status, item, ability, boosts; plus field and side
//! conditions (see `diff_turn` in the crate root).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use vgc_engine_core::data;
use std::collections::VecDeque;
use vgc_engine_core::rng::{DrawSpace, RecordedDraw, Rng, RngDecision, RngEvent, RngKey, NO_SLOT};
use vgc_engine_core::{Battle, BattleConfig, Choice, Format, SideRef, StepResult, Target};

use crate::{build_engine_team, build_table_from, decode_slot_ref, diff_turn, Divergence, TurnRecord};

// ---------------------------------------------------------------------------
// Input schema
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AccBattle {
    #[serde(default)]
    pub id: String,
    pub format: String,
    #[serde(default)]
    pub seed: serde_json::Value,
    pub p1team: String,
    pub p2team: String,
    #[serde(default)]
    pub start_raw: Vec<RawDraw>,
    pub turns: Vec<AccTurn>,
    #[serde(rename = "_meta", default)]
    pub meta: Meta,
}

#[derive(Debug, Default, Deserialize)]
pub struct Meta {
    #[serde(default)]
    pub ok: bool,
    #[serde(default)]
    pub ended: bool,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub choice_log: u32,
    #[serde(default)]
    pub choice_fallback: u32,
    #[serde(default)]
    pub replay: Option<String>,
    #[serde(default)]
    pub log: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct SideCmds {
    #[serde(default)]
    pub p1: Vec<String>,
    #[serde(default)]
    pub p2: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct AccTurn {
    #[serde(flatten)]
    pub base: TurnRecord,
    #[serde(default)]
    pub midturn: SideCmds,
    #[serde(default)]
    pub replace: SideCmds,
    #[serde(default)]
    pub raw: Vec<RawDraw>,
    #[serde(default)]
    pub has_state: bool,
}

/// One PS PRNG call from the raw trace.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawDraw {
    pub seq: u32,
    pub turn: u32,
    pub op: String,
    #[serde(default)]
    pub a: f64,
    #[serde(default)]
    pub b: f64,
    #[serde(default)]
    pub raw: u64,
    #[serde(default)]
    pub result: f64,
    #[serde(default)]
    pub site: Vec<String>,
}

// ---------------------------------------------------------------------------
// Shared replay plumbing
// ---------------------------------------------------------------------------

fn is_doubles(format: &str) -> bool {
    format.contains("doubles") || format.contains("vgc")
}

/// Parse one PS choice (already rewritten to original team indices by the
/// driver). Supports `move N [T] [mega|terastallize]`, `switch K`, `pass`.
pub fn parse_choice_ext(s: &str, actor_slot: u8, side: SideRef) -> Result<Choice, String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let foe = match side {
        SideRef::P1 => SideRef::P2,
        SideRef::P2 => SideRef::P1,
    };
    let idx = |t: &str| -> Result<u8, String> {
        let n: u8 = t.parse().map_err(|_| format!("bad index {t:?}"))?;
        n.checked_sub(1).ok_or_else(|| format!("index {t:?} not 1-based"))
    };
    match parts.as_slice() {
        ["pass"] => Ok(Choice::Pass { actor_slot }),
        ["switch", k] => Ok(Choice::Switch { actor_slot, team_index: idx(k)? }),
        ["move", n, rest @ ..] => {
            let move_slot = idx(n)?;
            let mut target = None;
            let mut gimmick = "";
            for r in rest {
                match *r {
                    "mega" => gimmick = "mega",
                    "terastallize" => gimmick = "tera",
                    t => {
                        let v: i8 = t.parse().map_err(|_| format!("bad target {t:?}"))?;
                        target = Some(match v {
                            1 | 2 => Target { side: foe, slot: (v as u8) - 1 },
                            -1 | -2 => Target { side, slot: (-v as u8) - 1 },
                            _ => return Err(format!("bad target {t:?}")),
                        });
                    }
                }
            }
            Ok(match gimmick {
                "mega" => Choice::MegaEvolve { actor_slot, move_slot, target },
                "tera" => Choice::Terastallize { actor_slot, move_slot, target },
                _ => Choice::Move { actor_slot, move_slot, target },
            })
        }
        _ => Err(format!("unrecognized PS choice {s:?}")),
    }
}

fn parse_line(line: &str, side: SideRef) -> Result<Vec<Choice>, String> {
    line.split(',')
        .map(|s| s.trim())
        .enumerate()
        .filter(|(_, s)| !s.is_empty())
        .map(|(slot, s)| parse_choice_ext(s, slot as u8, side))
        .collect()
}

/// Main-phase choices plus any mid-turn forced switches (appended; the engine
/// consumes a Switch that follows a Move for the same slot as that slot's
/// pivot replacement — `Battle::apply_self_switches`).
fn turn_choices(main: &[String], mid: &[String], side: SideRef) -> Result<Vec<Choice>, String> {
    let mut out = Vec::new();
    for l in main {
        out.extend(parse_line(l, side)?);
    }
    // Only a slot that chose a move can pivot; a mid-turn switch for any
    // other slot (Eject Button / Emergency Exit on a mon that switched in)
    // would be read by the engine as a start-of-turn switch. The engine
    // resolves those item/ability switches itself.
    let moved: Vec<u8> = out
        .iter()
        .filter(|c| matches!(c, Choice::Move { .. } | Choice::MegaEvolve { .. } | Choice::Terastallize { .. }))
        .map(|c| c.actor_slot())
        .collect();
    for l in mid {
        out.extend(
            parse_line(l, side)?
                .into_iter()
                .filter(|c| matches!(c, Choice::Switch { .. }) && moved.contains(&c.actor_slot())),
        );
    }
    Ok(out)
}

fn species_slug(b: &Battle, side: SideRef, slot: usize) -> Option<&'static str> {
    b.side(side).active_mon(slot).map(|m| data::SPECIES[m.species_id as usize].slug)
}

/// Species check (the base conformance diff has no identity check), then the
/// shared state diff.
fn diff_full(b: &Battle, t: &AccTurn) -> Result<Option<Divergence>, String> {
    let mut slots: Vec<&String> = t.base.state.keys().collect();
    slots.sort();
    for slot_ref in slots {
        let exp = &t.base.state[slot_ref];
        let Some(ps_sp) = exp.species.as_deref() else { continue };
        let Some((side, slot)) = decode_slot_ref(slot_ref) else { continue };
        let eng = species_slug(b, side, slot).unwrap_or("none");
        if eng != ps_sp && !exp.fainted {
            return Ok(Some(Divergence {
                turn: t.base.turn,
                slot: slot_ref.clone(),
                field: "species",
                engine: eng.to_string(),
                ps: ps_sp.to_string(),
            }));
        }
    }
    diff_turn(b, &t.base)
}

fn diff_terminal(b: &Battle, t: &AccTurn) -> Option<Divergence> {
    let mut slots: Vec<&String> = t.base.state.keys().collect();
    slots.sort();
    for slot_ref in slots {
        let exp = &t.base.state[slot_ref];
        let (side, slot) = decode_slot_ref(slot_ref)?;
        let eng_fainted = b.side(side).active_mon(slot).map(|m| m.fainted || m.current_hp == 0).unwrap_or(true);
        if eng_fainted != exp.fainted {
            return Some(Divergence {
                turn: t.base.turn,
                slot: slot_ref.clone(),
                field: "fainted",
                engine: eng_fainted.to_string(),
                ps: exp.fainted.to_string(),
            });
        }
    }
    None
}

/// Result of replaying one battle (either mode).
#[derive(Debug, Clone, Serialize)]
pub struct Replayed {
    pub id: String,
    pub turns_total: u32,
    pub turns_compared: u32,
    /// Turns matched before the first divergence.
    pub matched_turns: u32,
    pub divergence: Option<DivOut>,
    /// Keyed mode: engine draws that missed the table, at or before the
    /// divergence turn (or overall when clean).
    pub unmatched_upto: u32,
    pub unmatched_total: u32,
    /// Keyed mode: PS-recorded outcomes the engine never consumed, at or
    /// before the divergence turn.
    pub leftover_upto: u32,
    /// Keyed mode: engine draws the repair pass paired with an unconsumed PS
    /// outcome from the same move use (see [`replay_keyed`]).
    pub repaired: u32,
    /// Keyed mode: the divergence changes when only the fallback stream for
    /// unforced draws changes — i.e. it is caused by RNG PS never supplied.
    pub rng_sensitive: bool,
    /// Keyed mode: `turn/decision/move/actor>target` of each engine miss and
    /// PS leftover at or before the divergence turn.
    pub miss_keys: Vec<String>,
    pub leftover_keys: Vec<String>,
    /// ps-rng mode: first draw where the two traces disagree.
    pub first_draw_div: Option<DrawDiv>,
    pub engine_error: Option<String>,
    pub ended_naturally: bool,
    pub ps_errors: usize,
    pub choice_log: u32,
    pub choice_fallback: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DivOut {
    pub turn: u32,
    pub slot: String,
    pub field: String,
    pub engine: String,
    pub ps: String,
}

impl From<Divergence> for DivOut {
    fn from(d: Divergence) -> Self {
        DivOut { turn: d.turn, slot: d.slot, field: d.field.to_string(), engine: d.engine, ps: d.ps }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DrawDiv {
    /// Position in the whole-battle draw sequence.
    pub global_index: usize,
    pub turn: u32,
    /// Index of the draw within its turn.
    pub index: usize,
    pub ps: Option<String>,
    pub engine: Option<String>,
    pub ps_kind: String,
    pub engine_kind: String,
}

/// Drive the engine through every recorded turn. `on_turn` runs before each
/// step (e.g. to label the draw trace).
fn drive(
    acc: &AccBattle,
    rng: Rng,
    mut before_step: impl FnMut(&mut Battle, u32),
) -> (Battle, Result<(u32, u32, Option<Divergence>, bool), String>) {
    let champions = acc.format.contains("champions");
    let p1 = build_engine_team(&acc.p1team, champions);
    let p2 = build_engine_team(&acc.p2team, champions);
    let format = if is_doubles(&acc.format) { Format::Doubles } else { Format::Singles };
    let (p1, p2) = match (p1, p2) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            let dummy = Battle::new(BattleConfig { format, seed: 0 }, vec![], vec![]);
            return (dummy, Err(format!("team: {e}")));
        }
    };
    let mut b = Battle::with_rng(BattleConfig { format, seed: 0 }, rng, p1, p2);
    b.decision_phases = true;
    let mut matched = 0u32;
    let mut compared = 0u32;
    let mut ended = false;
    // PS stops the moment a side has nothing left; the engine finishes the
    // turn (residuals, end-of-turn heals, field timers) before reporting the
    // winner. On that terminal turn only who fainted is comparable.
    let terminal = if acc.meta.ended {
        acc.turns.iter().filter(|t| t.has_state).map(|t| t.base.turn).max()
    } else {
        None
    };
    let res = (|| {
        for t in &acc.turns {
            before_step(&mut b, t.base.turn);
            let p1c = turn_choices(&t.base.choices.p1, &t.midturn.p1, SideRef::P1)?;
            let p2c = turn_choices(&t.base.choices.p2, &t.midturn.p2, SideRef::P2)?;
            let mut r = b.step(&p1c, &p2c);
            // Only when the engine itself is waiting for replacements: if its
            // state already diverged (nobody fainted), stepping PS's
            // replacement choices would run a whole extra turn.
            if !matches!(r, StepResult::Ended { .. })
                && (!t.replace.p1.is_empty() || !t.replace.p2.is_empty())
                && b.needs_replacements()
            {
                let rp1 = turn_choices(&t.replace.p1, &[], SideRef::P1)?;
                let rp2 = turn_choices(&t.replace.p2, &[], SideRef::P2)?;
                r = b.step(&rp1, &rp2);
            }
            if !t.has_state {
                // PS never snapshotted this turn (battle ended mid-turn or
                // hit the turn cap); nothing to compare.
                break;
            }
            compared += 1;
            let d = if Some(t.base.turn) == terminal { diff_terminal(&b, t) } else { diff_full(&b, t)? };
            if let Some(d) = d {
                return Ok((matched, compared, Some(d), ended));
            }
            matched += 1;
            if matches!(r, StepResult::Ended { .. }) {
                ended = true;
                break;
            }
        }
        Ok((matched, compared, None, ended))
    })();
    (b, res)
}

fn base_report(acc: &AccBattle) -> Replayed {
    Replayed {
        id: acc.id.clone(),
        turns_total: acc.turns.iter().filter(|t| t.has_state).count() as u32,
        turns_compared: 0,
        matched_turns: 0,
        divergence: None,
        unmatched_upto: 0,
        unmatched_total: 0,
        leftover_upto: 0,
        repaired: 0,
        rng_sensitive: false,
        miss_keys: vec![],
        leftover_keys: vec![],
        first_draw_div: None,
        engine_error: None,
        ended_naturally: false,
        ps_errors: acc.meta.errors.len(),
        choice_log: acc.meta.choice_log,
        choice_fallback: acc.meta.choice_fallback,
    }
}

// ---------------------------------------------------------------------------
// Keyed mode
// ---------------------------------------------------------------------------

/// Key under which PS's queue-sort speed-tie shuffles are stored. The engine
/// draws them with whatever move context is stale at turn start, so they are
/// only ever reached through the repair pass (paired by turn + Tiebreak).
fn tie_key(turn: u32) -> RngKey {
    RngKey { turn, actor: NO_SLOT, target: NO_SLOT, move_id: u16::MAX, decision: RngDecision::Tiebreak }
}

/// PS resolves speed ties with `PRNG.shuffle` inside `speedSort`, which never
/// goes through `Battle.random`, so the keyed envelopes miss them. Recover the
/// ones from `BattleQueue.sort` (action order — the only shuffle the engine
/// mirrors) from the raw trace: `shuffle` draws `random(i, end) = j`, and the
/// engine's Fisher-Yates step takes `j = i + v % span`, so `v = j - i`.
fn add_queue_tiebreaks(table: &mut HashMap<RngKey, VecDeque<RngEvent>>, acc: &AccBattle) {
    for d in acc.turns.iter().flat_map(|t| t.raw.iter()) {
        let queue_sort = d.op == "shuffle" && d.site.get(1).is_some_and(|s| s.contains("BattleQueue.sort"));
        if queue_sort {
            let v = (d.result - d.a).max(0.0) as u64;
            table.entry(tie_key(d.turn)).or_default().push_back(RngEvent::Tiebreak(v));
        }
    }
}

/// Sentinel move id for PS `randomChance` gates recorded as a bool under
/// `range` (stall roll, full paralysis, Poison Touch, ...). The contract
/// stores them as `Range(1)` pass / `Range(0)` fail, which only
/// `Rng::chance_keyed` reads that way — the engine's `range(n) == 0` gates
/// read `Range(0)` as a pass. So they are parked under this sentinel (no
/// engine draw asks for it) and handed out by the repair pass, converted to
/// whatever the engine draw needs.
const BOOL_GATE: u16 = u16::MAX - 1;

fn park_bool_gates(table: &mut Table, acc: &AccBattle) {
    for d in acc.turns.iter().flat_map(|t| t.base.draws.iter()) {
        if d.decision != "range" || !d.value.is_boolean() {
            continue;
        }
        let actor = d.actor.as_deref().and_then(crate::parse_slot_ref).unwrap_or(NO_SLOT);
        let target = d.target.as_deref().and_then(crate::parse_slot_ref).unwrap_or(NO_SLOT);
        let Some(mv) = d.move_slug.as_deref().and_then(|m| data::MOVES.iter().position(|x| x.slug == m)) else { continue };
        let key = RngKey { turn: d.turn, actor, target, move_id: mv as u16, decision: RngDecision::Range };
        let pass = d.value.as_bool().unwrap_or(false);
        if let Some(q) = table.get_mut(&key) {
            // Remove the first matching bool encoding from the direct key.
            if let Some(pos) = q.iter().position(|e| *e == RngEvent::Range(pass as u32)) {
                q.remove(pos);
            }
        }
        table
            .entry(RngKey { move_id: BOOL_GATE, ..key })
            .or_default()
            .push_back(RngEvent::Crit(pass));
    }
}

/// Convert a PS-recorded outcome to what an engine draw of `space` expects.
fn convert(ev: RngEvent, space: &DrawSpace) -> Option<RngEvent> {
    match (space, ev) {
        (DrawSpace::Crit { .. }, RngEvent::Crit(b)) => Some(RngEvent::Crit(b)),
        (DrawSpace::UniformDamage { .. }, RngEvent::DamageRoll(v)) => Some(RngEvent::DamageRoll(v)),
        (DrawSpace::UniformPercent { .. }, RngEvent::PercentRoll(v)) => Some(RngEvent::PercentRoll(v)),
        // A `randomChance` gate PS logged as a bool under `range`.
        (DrawSpace::UniformPercent { .. }, RngEvent::Range(v)) if v <= 1 => {
            Some(RngEvent::PercentRoll(if v == 1 { 1 } else { 100 }))
        }
        (DrawSpace::UniformRange(n), RngEvent::Range(v)) if v < *n => Some(RngEvent::Range(v)),
        // A parked bool gate (see BOOL_GATE): the engine's `range(n)` gates
        // pass on 0; its percent gates pass on a low roll.
        (DrawSpace::UniformRange(n), RngEvent::Crit(b)) if *n > 1 => Some(RngEvent::Range(if b { 0 } else { n - 1 })),
        (DrawSpace::UniformPercent { .. }, RngEvent::Crit(b)) => Some(RngEvent::PercentRoll(if b { 1 } else { 100 })),
        (DrawSpace::Tiebreak { .. }, RngEvent::Tiebreak(v)) => Some(RngEvent::Tiebreak(v)),
        _ => None,
    }
}

type Table = HashMap<RngKey, VecDeque<RngEvent>>;

/// One keyed run: the engine, its outcome, its misses, and what PS recorded
/// that the engine never consumed (the last `n` entries of each queue, since
/// lookups pop from the front).
fn keyed_run(acc: &AccBattle, table: &Table) -> (Battle, Result<(u32, u32, Option<Divergence>, bool), String>, Vec<RecordedDraw>) {
    keyed_run_seeded(acc, table, 0xC0FFEE)
}

fn keyed_run_seeded(
    acc: &AccBattle,
    table: &Table,
    fallback: u64,
) -> (Battle, Result<(u32, u32, Option<Divergence>, bool), String>, Vec<RecordedDraw>) {
    let rng = Rng::oracle_keyed(table.clone(), fallback);
    let (mut b, res) = drive(acc, rng, |_, _| {});
    let misses = b.rng_mut().take_miss_log().unwrap_or_default();
    (b, res, misses)
}

/// Keyed replay with a repair pass. The keyed contract labels draws by
/// `(turn, actor, target, move, decision)`; a few engine sites carry a stale
/// or different label than PS's (ability procs PS logs as `range` bools, a
/// secondary PS keys on a different target, speed-tie shuffles PS never
/// routes through `Battle.random`). Each round pairs every engine draw that
/// missed the table in the earliest turn with an unconsumed PS outcome from
/// the SAME move use (same turn, actor and move; speed ties by turn) whose
/// value fits the engine's draw space, aliases it under the engine's key and
/// replays. Whatever still diverges once no pairing is left was not an
/// RNG-labelling problem. `repaired` counts the aliased draws.
pub fn replay_keyed(acc: &AccBattle) -> Replayed {
    let mut rep = base_report(acc);
    let (table, repaired, b, res, misses) = repaired_table(acc);
    rep.repaired = repaired;
    let mut b = b;
    keyed_finish(acc, &table, &mut rep, &mut b, res, misses);
    rep
}

/// The keyed oracle table after the repair pass, plus the final run.
#[allow(clippy::type_complexity)]
fn repaired_table(
    acc: &AccBattle,
) -> (Table, u32, Battle, Result<(u32, u32, Option<Divergence>, bool), String>, Vec<RecordedDraw>) {
    let (mut table, _unresolved) = build_table_from(acc.turns.iter().flat_map(|t| t.base.draws.iter()));
    add_queue_tiebreaks(&mut table, acc);
    park_bool_gates(&mut table, acc);
    let mut repaired = 0u32;
    let (mut b, mut res, mut misses) = keyed_run(acc, &table);
    for _round in 0..40 {
        if res.is_err() || misses.is_empty() {
            break;
        }
        // Pair misses turn by turn; once a turn yields a pairing, replay
        // before touching later turns (the replay can change them).
        let left = b.rng().keyed_leftovers().unwrap_or_default();
        let mut remaining: HashMap<RngKey, usize> = left.into_iter().collect();
        let mut progress = false;
        let mut turns: Vec<u32> = misses.iter().map(|m| m.key.turn).collect();
        turns.sort_unstable();
        turns.dedup();
        for turn in turns {
        for m in misses.iter().filter(|m| m.key.turn == turn) {
            let mk = m.key;
            // Candidate PS keys from the same move use, best label match first.
            let mut cands: Vec<RngKey> = remaining
                .iter()
                .filter(|(k, n)| {
                    // Same move use; speed ties by turn (the engine draws them
                    // at turn start under the previous turn's stale context).
                    **n > 0
                        && ((k.turn == mk.turn && k.actor == mk.actor && (k.move_id == mk.move_id || k.move_id == BOOL_GATE))
                            || (mk.decision == RngDecision::Tiebreak
                                && k.decision == RngDecision::Tiebreak
                                && (k.turn == mk.turn || k.turn == mk.turn + 1)))
                })
                .map(|(k, _)| *k)
                .collect();
            cands.sort_by_key(|k| (k.decision != mk.decision, k.target != mk.target, k.decision as u8, k.target, k.move_id));
            for k in cands {
                let n = remaining[&k];
                let q = table.get(&k).expect("leftover key in table");
                let idx = q.len() - n;
                if let Some(ev) = convert(q[idx], &m.space) {
                    table.get_mut(&k).unwrap().remove(idx);
                    table.entry(mk).or_default().push_back(ev);
                    *remaining.get_mut(&k).unwrap() -= 1;
                    repaired += 1;
                    progress = true;
                    break;
                }
            }
        }
        if progress {
            break;
        }
        }
        if !progress {
            break;
        }
        (b, res, misses) = keyed_run(acc, &table);
    }
    (table, repaired, b, res, misses)
}

fn keyed_finish(
    acc: &AccBattle,
    table: &Table,
    rep: &mut Replayed,
    b: &mut Battle,
    res: Result<(u32, u32, Option<Divergence>, bool), String>,
    misses: Vec<RecordedDraw>,
) {
    // Sensitivity: rerun with other fallback streams. Only draws PS never
    // supplied use the fallback, so a divergence that moves (or vanishes) with
    // it is caused by unforced RNG; one that is identical under every stream
    // is deterministic given PS's outcomes.
    if let Ok((_, _, first, _)) = &res {
        let sig = |d: &Option<Divergence>| d.as_ref().map(|d| (d.turn, d.slot.clone(), d.field, d.engine.clone()));
        let base = sig(first);
        let mut sensitive = false;
        for seed in [1u64, 2, 3, 0xDEAD_BEEF, 0x5EED] {
            let (_, r2, _) = keyed_run_seeded(acc, table, seed);
            if let Ok((_, _, d2, _)) = r2 {
                if sig(&d2) != base {
                    sensitive = true;
                    break;
                }
            }
        }
        rep.rng_sensitive = sensitive;
    }
    match res {
        Err(e) => rep.engine_error = Some(e),
        Ok((matched, compared, div, ended)) => {
            rep.matched_turns = matched;
            rep.turns_compared = compared;
            rep.ended_naturally = ended;
            let upto = div.as_ref().map(|d| d.turn).unwrap_or(u32::MAX);
            rep.divergence = div.map(Into::into);
            let keys: Vec<RngKey> = misses.iter().map(|r| r.key).collect();
            rep.unmatched_total = keys.len() as u32;
            rep.unmatched_upto = keys.iter().filter(|k| k.turn <= upto).count() as u32;
            let left: Vec<(RngKey, usize)> = b
                .rng()
                .keyed_leftovers()
                .unwrap_or_default()
                .into_iter()
                .filter(|(k, _)| k.turn <= upto && k.turn > 0)
                .collect();
            rep.leftover_upto = left.iter().map(|(_, n)| *n as u32).sum();
            rep.miss_keys = keys.iter().filter(|k| k.turn <= upto).map(key_str).collect();
            rep.leftover_keys = left.iter().map(|(k, n)| format!("{} x{n}", key_str(k))).collect();
            rep.leftover_keys.sort();
        }
    }
}

fn key_str(k: &RngKey) -> String {
    let mv = data::MOVES.get(k.move_id as usize).map(|m| m.slug).unwrap_or("?");
    let mv = if k.move_id == 0 { "<none>" } else { mv };
    let slot = |s: u8| match s {
        0 => "p1a",
        1 => "p1b",
        2 => "p2a",
        3 => "p2b",
        _ => "-",
    };
    format!("t{}/{:?}/{}/{}>{}", k.turn, k.decision, mv, slot(k.actor), slot(k.target))
}

// ---------------------------------------------------------------------------
// ps-rng mode
// ---------------------------------------------------------------------------

/// Normalized kind of a PS draw, from its semantic call site.
pub fn ps_kind(d: &RawDraw) -> String {
    let s0 = d.site.first().map(String::as_str).unwrap_or("");
    let s1 = d.site.get(1).map(String::as_str).unwrap_or("");
    let f = |s: &str| s.split('@').next().unwrap_or("").to_string();
    let fn0 = f(s0);
    if fn0.ends_with("speedSort") {
        let ctx = f(s1);
        let ctx = ctx.rsplit('.').next().unwrap_or("").to_string();
        return format!("shuffle:{ctx}");
    }
    if fn0.ends_with("randomizer") {
        return "damage".into();
    }
    if fn0.ends_with("getDamage") {
        return "crit".into();
    }
    if fn0.ends_with("hitStepAccuracy") || (fn0.ends_with("hitStepMoveHitLoop") && d.op == "randomChance") {
        return "accuracy".into();
    }
    if fn0.ends_with("secondaries") || fn0.ends_with("selfDrops") {
        return "secondary".into();
    }
    if fn0.ends_with("hitStepMoveHitLoop") {
        return "multihit".into();
    }
    if fn0.ends_with("randomFoe") || fn0.ends_with("getRandomTarget") {
        return "random_target".into();
    }
    let short = fn0.rsplit('.').next().unwrap_or("").to_string();
    let loc = s0.split('@').nth(1).unwrap_or("");
    format!("{}:{short}@{loc}", d.op)
}

/// PS-call equivalence of a draw: `random(n)` vs `random(m, n)`.
fn ps_args(d: &RawDraw) -> (u64, u64) {
    if d.op == "shuffle" || d.site.first().is_some_and(|s| s.starts_with("BattleQueue.insertChoice")) {
        // Fisher-Yates steps are compared by span: PS indexes the whole
        // queue / handler list, the engine only its own sub-list.
        return ((d.b - d.a) as u64, 0);
    }
    norm_args(d.a as u64, d.b as u64)
}

/// `random(0, n)` and `random(n)` are the same call.
fn norm_args(a: u64, b: u64) -> (u64, u64) {
    if a == 0 && b > 0 { (b, 0) } else { (a, b) }
}

#[cfg(feature = "ps-rng")]
fn engine_kind(d: &vgc_engine_core::ps_rng::PsDraw) -> String {
    match d.op {
        "crit" => "crit".into(),
        "damage" => "damage".into(),
        "shuffle" => "shuffle".into(),
        "percent" if d.decision == "accuracy" || d.decision == "secondary" => d.decision.into(),
        _ => {
            let file = d.file.rsplit('/').next().unwrap_or(d.file);
            format!("{}:{file}:{}", d.op, d.line)
        }
    }
}

#[cfg(feature = "ps-rng")]
fn engine_args(d: &vgc_engine_core::ps_rng::PsDraw) -> (u64, u64) {
    match d.op {
        // random_chance logs (den, num); PS's PRNG sees random(den).
        "crit" | "chance" => (d.a as u64, 0),
        "shuffle" | "insert_choice" => ((d.b - d.a) as u64, 0),
        _ => norm_args(d.a as u64, d.b as u64),
    }
}

#[cfg(feature = "ps-rng")]
fn describe_engine(d: &vgc_engine_core::ps_rng::PsDraw) -> String {
    let mv = data::MOVES.get(d.move_id as usize).map(|m| m.slug).unwrap_or("?");
    let file = d.file.rsplit('/').next().unwrap_or(d.file);
    format!(
        "{}({},{})={} @{}:{} [actor {} move {} target {} {}]",
        d.op, d.a, d.b, d.result, file, d.line, d.actor, mv, d.target, d.decision
    )
}

pub fn describe_ps(d: &RawDraw) -> String {
    format!("{}({},{})={} @{}", d.op, d.a, d.b, d.result, d.site.join(" < "))
}

/// Replay under the engine's Showdown-compatible PRNG with the battle's seed.
#[cfg(feature = "ps-rng")]
pub fn replay_ps_rng(acc: &AccBattle) -> (Replayed, Vec<vgc_engine_core::ps_rng::PsDraw>) {
    let mut rep = base_report(acc);
    let seed = match &acc.seed {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(","),
        _ => String::new(),
    };
    let mut rng = match Rng::ps(&seed) {
        Ok(r) => r,
        Err(e) => {
            rep.engine_error = Some(e.to_string());
            return (rep, vec![]);
        }
    };
    rng.ps_mut().expect("ps rng").enable_trace();
    let (mut b, res) = drive(acc, rng, |b, turn| {
        if let Some(p) = b.rng_mut().ps_mut() {
            p.set_trace_turn(turn);
        }
    });
    let trace = b.rng_mut().ps_mut().and_then(|p| p.take_trace()).unwrap_or_default();
    match res {
        Err(e) => rep.engine_error = Some(e),
        Ok((matched, compared, div, ended)) => {
            rep.matched_turns = matched;
            rep.turns_compared = compared;
            rep.ended_naturally = ended;
            rep.divergence = div.map(Into::into);
        }
    }
    rep.first_draw_div = first_draw_divergence(acc, &trace);
    (rep, trace)
}

/// Walk both traces turn by turn; report the first position whose PS-level
/// call (args) differs, or where one side has a draw the other lacks. Turn 0
/// (battle start) draws are engine-labelled 1 (the first `step` runs the lead
/// switch-ins' effects inside turn 1 in PS too), so compare on the global
/// sequence instead of per-turn buckets.
#[cfg(feature = "ps-rng")]
pub fn first_draw_divergence(
    acc: &AccBattle,
    eng: &[vgc_engine_core::ps_rng::PsDraw],
) -> Option<DrawDiv> {
    let ps: Vec<&RawDraw> = acc.start_raw.iter().chain(acc.turns.iter().flat_map(|t| t.raw.iter())).collect();
    // Only compare within the turns the engine actually played.
    let last_turn = acc.turns.iter().filter(|t| t.has_state).map(|t| t.base.turn).max().unwrap_or(0);
    let ps: Vec<&RawDraw> = ps.into_iter().filter(|d| d.turn <= last_turn).collect();
    let n = ps.len().max(eng.len());
    let mut turn_start: HashMap<u32, usize> = HashMap::new();
    for i in 0..n {
        let p = ps.get(i);
        let e = eng.get(i);
        let turn = p.map(|d| d.turn).or(e.map(|d| d.turn)).unwrap_or(0);
        let start = *turn_start.entry(turn).or_insert(i);
        let same = match (p, e) {
            (Some(p), Some(e)) => ps_args(p) == engine_args(e) && kinds_compatible(&ps_kind(p), &engine_kind(e)),
            _ => false,
        };
        if !same {
            return Some(DrawDiv {
                global_index: i,
                turn,
                index: i - start,
                ps: p.map(|d| describe_ps(d)),
                engine: e.map(describe_engine),
                ps_kind: p.map(|d| ps_kind(d)).unwrap_or_else(|| "<none>".into()),
                engine_kind: e.map(engine_kind).unwrap_or_else(|| "<none>".into()),
            });
        }
    }
    None
}

/// Kinds agree when both name the same semantic draw, or when the engine side
/// is an unlabelled generic draw (its kind is a file:line) — args equality
/// then carries the comparison.
#[cfg(feature = "ps-rng")]
fn kinds_compatible(ps: &str, eng: &str) -> bool {
    let generic = |k: &str| k.contains(':') && !k.starts_with("shuffle");
    if ps == eng || generic(eng) || generic(ps) {
        return true;
    }
    ps.starts_with("shuffle") && eng == "shuffle"
}

// ---------------------------------------------------------------------------
// Divergence-turn context (for the taxonomy)
// ---------------------------------------------------------------------------

/// Everything that happened in PS's log during `turn`: moves used
/// (actor slot, move id), `[from]` effect names, and the protocol lines.
#[derive(Debug, Default, Clone, Serialize)]
pub struct TurnContext {
    pub moves: Vec<(String, String, String)>,
    pub effects: Vec<String>,
    pub lines: Vec<String>,
}

pub fn turn_context(log: &str, turn: u32) -> TurnContext {
    let mut ctx = TurnContext::default();
    let mut cur = 0u32;
    let start_tag = format!("|turn|{turn}");
    let _ = start_tag;
    for line in log.lines() {
        if let Some(n) = line.strip_prefix("|turn|") {
            cur = n.trim().parse().unwrap_or(cur);
            continue;
        }
        if cur != turn {
            continue;
        }
        if line.starts_with("|t:|") || line.is_empty() || line == "|" {
            continue;
        }
        ctx.lines.push(line.to_string());
        let parts: Vec<&str> = line.split('|').collect();
        if parts.get(1) == Some(&"move") {
            let actor = parts.get(2).map(|s| s.split(':').next().unwrap_or("").to_string()).unwrap_or_default();
            let mv = parts.get(3).map(|s| to_id(s)).unwrap_or_default();
            let tgt = parts.get(4).map(|s| s.split(':').next().unwrap_or("").to_string()).unwrap_or_default();
            ctx.moves.push((actor, mv, tgt));
        }
        for p in &parts {
            if let Some(e) = p.strip_prefix("[from] ") {
                ctx.effects.push(to_id(e.trim_start_matches("item: ").trim_start_matches("ability: ").trim_start_matches("move: ")));
            }
        }
        if matches!(parts.get(1), Some(&"-activate") | Some(&"-start") | Some(&"-fieldstart") | Some(&"-sidestart") | Some(&"-enditem") | Some(&"-item") | Some(&"-ability")) {
            if let Some(e) = parts.get(3) {
                let e = e.trim_start_matches("item: ").trim_start_matches("ability: ").trim_start_matches("move: ");
                ctx.effects.push(to_id(e));
            }
        }
    }
    ctx.effects.sort();
    ctx.effects.dedup();
    ctx
}

pub fn to_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// Species/item/ability of the mon in `slot_ref` at the start of `turn`
/// according to the PS protocol log (for offender attribution).
pub fn slot_mon_in_log(log: &str, slot_ref: &str, turn: u32) -> Option<String> {
    let mut cur = 0u32;
    let mut sp = None;
    let prefix = format!("{slot_ref}: ");
    for line in log.lines() {
        if let Some(n) = line.strip_prefix("|turn|") {
            cur = n.trim().parse().unwrap_or(cur);
            continue;
        }
        if cur > turn {
            break;
        }
        let parts: Vec<&str> = line.split('|').collect();
        if matches!(parts.get(1), Some(&"switch") | Some(&"drag") | Some(&"detailschange"))
            && parts.get(2).is_some_and(|p| p.starts_with(&prefix))
        {
            sp = parts.get(3).map(|d| to_id(d.split(',').next().unwrap_or("")));
        }
    }
    sp
}

// ---------------------------------------------------------------------------
// Debug dump: engine vs PS per slot per turn
// ---------------------------------------------------------------------------

/// Human-readable side-by-side of engine and PS state after every turn of a
/// keyed replay — the first thing to read when triaging a divergence.
pub fn dump_keyed(acc: &AccBattle) -> String {
    use std::fmt::Write;
    let (table, repaired, ..) = repaired_table(acc);
    let rng = Rng::oracle_keyed(table, 0xC0FFEE);
    let champions = acc.format.contains("champions");
    let format = if is_doubles(&acc.format) { Format::Doubles } else { Format::Singles };
    let mut out = String::new();
    let (p1, p2) = match (build_engine_team(&acc.p1team, champions), build_engine_team(&acc.p2team, champions)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return format!("team error: {e}"),
    };
    let mut b = Battle::with_rng(BattleConfig { format, seed: 0 }, rng, p1, p2);
    b.decision_phases = true;
    for t in &acc.turns {
        let p1c = turn_choices(&t.base.choices.p1, &t.midturn.p1, SideRef::P1);
        let p2c = turn_choices(&t.base.choices.p2, &t.midturn.p2, SideRef::P2);
        let (Ok(p1c), Ok(p2c)) = (p1c, p2c) else {
            let _ = writeln!(out, "turn {}: choice parse error", t.base.turn);
            break;
        };
        let _ = writeln!(out, "== turn {}  p1 {:?} mid {:?} rep {:?} | p2 {:?} mid {:?} rep {:?}", t.base.turn,
            t.base.choices.p1, t.midturn.p1, t.replace.p1, t.base.choices.p2, t.midturn.p2, t.replace.p2);
        let mut r = b.step(&p1c, &p2c);
        if !matches!(r, StepResult::Ended { .. }) && (!t.replace.p1.is_empty() || !t.replace.p2.is_empty()) && b.needs_replacements() {
            let rp1 = turn_choices(&t.replace.p1, &[], SideRef::P1).unwrap_or_default();
            let rp2 = turn_choices(&t.replace.p2, &[], SideRef::P2).unwrap_or_default();
            r = b.step(&rp1, &rp2);
        }
        for (label, side, slot) in [("p1a", SideRef::P1, 0), ("p1b", SideRef::P1, 1), ("p2a", SideRef::P2, 0), ("p2b", SideRef::P2, 1)] {
            let eng = b.side(side).active_mon(slot).map(|m| {
                format!("{} {}/{} {:?} {:?}", data::SPECIES[m.species_id as usize].slug, m.current_hp, m.stats.hp, m.status, m.boosts)
            });
            let ps = t.base.state.get(label).map(|s| {
                format!("{} {}/{} {:?} {:?}", s.species.as_deref().unwrap_or("?"), s.hp, s.maxhp, s.status, s.boosts.map(|b| [b.atk, b.def, b.spa, b.spd, b.spe, b.accuracy, b.evasion]))
            });
            let _ = writeln!(out, "  {label} ENG {}\n      PS  {}", eng.unwrap_or_default(), ps.unwrap_or_default());
        }
        if matches!(r, StepResult::Ended { .. }) {
            let _ = writeln!(out, "  (engine: battle ended)");
            break;
        }
    }
    let _ = writeln!(out, "unmatched draws: {:?} (repair pass aliased {repaired})", b.rng().unmatched_draws());
    let mut left: Vec<String> = b
        .rng()
        .keyed_leftovers()
        .unwrap_or_default()
        .iter()
        .map(|(k, n)| format!("{} x{n}", key_str(k)))
        .collect();
    left.sort();
    for l in left {
        let _ = writeln!(out, "  PS-only (never consumed) {l}");
    }
    if let Some(log) = b.rng_mut().take_miss_log() {
        for m in log {
            let mv = data::MOVES.get(m.key.move_id as usize).map(|x| x.slug).unwrap_or("?");
            let _ = writeln!(out, "  miss t{} actor {} target {} {} {:?}", m.key.turn, m.key.actor, m.key.target, mv, m.key.decision);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Distribution mode (experiment 4): one turn from a fixed state, many seeds
// ---------------------------------------------------------------------------

/// Outcome of turn 1 as a comparable token per active slot plus the field:
/// `species:hp:status:boosts` (boosts as 7 signed ints), before any
/// end-of-turn replacement. PS's `ps-dist.js` emits the same tokens.
pub fn outcome_tokens(b: &Battle) -> Vec<String> {
    let mut out = Vec::with_capacity(5);
    for (side, slot) in [(SideRef::P1, 0usize), (SideRef::P1, 1), (SideRef::P2, 0), (SideRef::P2, 1)] {
        let tok = match b.side(side).active_mon(slot) {
            None => "none".to_string(),
            Some(m) => {
                let st = crate::status_token_pub(m.status).unwrap_or("none");
                let bo: Vec<String> = m.boosts.iter().map(|x| x.to_string()).collect();
                format!("{}:{}:{}:{}", data::SPECIES[m.species_id as usize].slug, m.current_hp, st, bo.join(","))
            }
        };
        out.push(tok);
    }
    out.push(format!(
        "w={}|t={}|tr={}",
        crate::weather_token_pub(b.weather).unwrap_or("none"),
        crate::terrain_token_pub(b.terrain).unwrap_or("none"),
        b.trick_room_turns > 0
    ));
    out
}

/// Run turn 1 of `acc` `k` times under independent engine seeds and count
/// each slot's outcome token. Returns `[slot] -> token -> count`.
pub fn distribution(acc: &AccBattle, k: u32, seed0: u64) -> Result<Vec<HashMap<String, u32>>, String> {
    let champions = acc.format.contains("champions");
    let format = if is_doubles(&acc.format) { Format::Doubles } else { Format::Singles };
    let p1 = build_engine_team(&acc.p1team, champions)?;
    let p2 = build_engine_team(&acc.p2team, champions)?;
    let t = acc.turns.first().ok_or("no turns")?;
    let p1c = turn_choices(&t.base.choices.p1, &[], SideRef::P1)?;
    let p2c = turn_choices(&t.base.choices.p2, &[], SideRef::P2)?;
    let mut hist: Vec<HashMap<String, u32>> = vec![HashMap::new(); 5];
    for i in 0..k {
        let seed = seed0 ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut b = Battle::with_rng(BattleConfig { format, seed }, Rng::new(seed), p1.clone(), p2.clone());
        b.decision_phases = true;
        b.step(&p1c, &p2c);
        for (j, tok) in outcome_tokens(&b).into_iter().enumerate() {
            *hist[j].entry(tok).or_default() += 1;
        }
    }
    Ok(hist)
}

/// Engine state after turn 1 (and its replacements) under the repaired keyed
/// oracle, as outcome tokens (`species:hp:status:boosts` per slot) plus each
/// slot's max HP — for the real-log comparison (experiment 1b).
pub fn turn1_state(acc: &AccBattle) -> Result<Vec<(String, u16)>, String> {
    let (table, ..) = repaired_table(acc);
    let champions = acc.format.contains("champions");
    let format = if is_doubles(&acc.format) { Format::Doubles } else { Format::Singles };
    let p1 = build_engine_team(&acc.p1team, champions)?;
    let p2 = build_engine_team(&acc.p2team, champions)?;
    let mut b = Battle::with_rng(BattleConfig { format, seed: 0 }, Rng::oracle_keyed(table, 0xC0FFEE), p1, p2);
    b.decision_phases = true;
    let t = acc.turns.first().ok_or("no turns")?;
    let p1c = turn_choices(&t.base.choices.p1, &t.midturn.p1, SideRef::P1)?;
    let p2c = turn_choices(&t.base.choices.p2, &t.midturn.p2, SideRef::P2)?;
    b.step(&p1c, &p2c);
    let toks = outcome_tokens(&b);
    let mut out = Vec::new();
    for (i, (side, slot)) in [(SideRef::P1, 0usize), (SideRef::P1, 1), (SideRef::P2, 0), (SideRef::P2, 1)].into_iter().enumerate() {
        let max = b.side(side).active_mon(slot).map(|m| m.stats.hp).unwrap_or(0);
        out.push((toks[i].clone(), max));
    }
    Ok(out)
}
