//! vgc-engine-golden — synthetic golden-master differential harness.
//!
//! Replays a fully-specified scripted battle (Showdown export teams + PRNG
//! seed + per-turn action strings) against a PS-recorded ground-truth log
//! produced by `tools/ps-golden-driver/` and reports per-turn divergence.
//!
//! Unlike the replay-corpus scorer in `vgc-engine-replay`, this harness
//! has **no hidden state**: EVs, IVs, natures, abilities, items, and
//! every action are explicit in the input, and the engine's RNG is
//! pinned to the same draw stream PS used (via `Rng::oracle_partial`).
//! Any divergence is therefore a mechanic bug, not a recon-noise
//! artifact.
//!
//! ## Why this is the primary correctness signal
//!
//! The replay-corpus differential (`score-corpus`) maxes out around
//! 15% mean agreement and isn't improving with mechanic fixes —
//! because the dominant error term is EV-spread reconstruction, not
//! engine behavior. The golden tests sidestep that entirely: every PR
//! that touches a mechanic runs the goldens deterministically and
//! either matches PS bit-for-bit on HP/status/faint or it doesn't.
//!
//! ## RNG correspondence
//!
//! PS's PRNG (`sim/prng.ts`) and the engine's Splitmix64 are not the
//! same algorithm, so we don't expect their draws to align by seed
//! alone. Instead, the Node driver records every `Battle.random` /
//! `Battle.randomChance` call from PS, and the harness loads that
//! stream into `Rng::oracle_partial`. When the engine asks for a Crit
//! draw, it pops PS's recorded Crit value; same for damage rolls and
//! accuracy percentages. Engine-only draw sites (no PS analog yet)
//! fall through to a Splitmix fallback seeded from `seed[0]`.

use std::path::Path;

use serde::{Deserialize, Serialize};

pub mod explore;
pub use explore::{run_explore, run_explore_in_memory, ExploreDivergence, ExploreReport};

use vgc_engine_core::{
    Battle, BattleConfig, Choice, Format, Pokemon, Rng, RngEvent, SideRef, Status, StepResult,
    Target, TeamBuilder,
};
use vgc_engine_data as data;

// ---------------------------------------------------------------------------
// Input / PS-output JSON schemas (mirrors tools/ps-golden-driver/driver.js)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GoldenInput {
    pub name: Option<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_seed")]
    pub seed: [u16; 4],
    pub p1: GoldenSide,
    pub p2: GoldenSide,
    #[serde(default)]
    pub turns: Vec<GoldenTurn>,
    /// When true, `turns` may be empty — the harness derives per-turn
    /// actions from the PS event log instead (mirrors the random_play
    /// mode in tools/ps-golden-driver/driver.js, PR-200).
    #[serde(default)]
    pub random_play: bool,
    /// Maximum number of turns to compare under random_play; defaults
    /// to 30 (mirrors the driver's default). The derivation also stops
    /// at the first PS turn where either side fainted, because the
    /// engine has no auto-replacement step and post-faint replacements
    /// would desync the per-side action streams.
    #[serde(default)]
    pub max_turns: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct GoldenSide {
    pub team: String,
}

#[derive(Debug, Deserialize)]
pub struct GoldenTurn {
    #[serde(default)]
    pub p1: serde_json::Value,
    #[serde(default)]
    pub p2: serde_json::Value,
}

fn default_format() -> String { "gen9customgame".into() }
fn default_seed() -> [u16; 4] { [1, 2, 3, 4] }

#[derive(Debug, Deserialize)]
pub struct PsOutput {
    #[serde(default)]
    pub ok: bool,
    pub events: Vec<PsEvent>,
    pub rng: Vec<PsRngEvent>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PsEvent {
    pub turn: u32,
    pub kind: String,
    #[serde(default)]
    pub actor: Option<String>,
    #[serde(default)]
    pub hp: Option<u32>,
    #[serde(default)]
    pub max: Option<u32>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub stat: Option<String>,
    #[serde(default)]
    pub amount: Option<i32>,
    #[serde(default)]
    pub species: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// PS `|move|<actor>|<name>|<target>` target slot, e.g. `"p2a"`.
    /// Used in random-play action derivation to recover the doubles
    /// targeting token.
    #[serde(default)]
    pub target: Option<String>,
    /// PS `|-miss|<source>|<target>` source actor. The driver emits
    /// `source` (not `actor`) on miss events; this field captures it
    /// for the exploratory comparison harness (PR-203).
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "kind")]
pub enum PsRngEvent {
    Crit { value: bool },
    DamageRoll { value: u8 },
    PercentRoll { value: bool, threshold: u8 },
    Range { value: u32, bound: u32 },
    Tiebreak { value: String },
    Chance { value: bool, num: u32, denom: u32 },
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct SlotSnapshot {
    pub side: u8,   // 1 or 2
    pub slot: char, // 'a' or 'b'
    pub hp: u32,
    pub max: u32,
    pub fainted: bool,
    pub status: String, // "none" | "brn" | "par" | "frz" | "psn" | "tox" | "slp"
}

#[derive(Debug, Serialize, Clone)]
pub struct Divergence {
    pub turn: u32,
    pub kind: String,
    pub ps: Option<SlotSnapshot>,
    pub engine: Option<SlotSnapshot>,
    pub note: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct GoldenReport {
    pub name: String,
    pub turns_run: u32,
    pub matched: usize,
    pub diverged: Vec<Divergence>,
}

impl GoldenReport {
    pub fn is_ok(&self) -> bool {
        self.diverged.is_empty()
    }
}

#[derive(Debug)]
pub enum GoldenError {
    Io(std::io::Error),
    Json(serde_json::Error),
    PsNotOk,
    TeamParse(String),
    BadAction(String),
    BadFormat(String),
}

impl std::fmt::Display for GoldenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoldenError::Io(e) => write!(f, "io: {e}"),
            GoldenError::Json(e) => write!(f, "json: {e}"),
            GoldenError::PsNotOk => write!(f, "ps output ok=false"),
            GoldenError::TeamParse(s) => write!(f, "team parse: {s}"),
            GoldenError::BadAction(s) => write!(f, "bad action: {s}"),
            GoldenError::BadFormat(s) => write!(f, "bad format: {s}"),
        }
    }
}
impl std::error::Error for GoldenError {}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Load a golden input + the PS-recorded ground truth, run the engine
/// through every turn, and produce a per-turn divergence report.
///
/// The check is **HP / status / faint / boosts** parity at end-of-turn
/// for every active slot. This is the smallest unambiguous signal that
/// catches every damage-formula bug, ability bug, item bug, and
/// most secondary-effect bugs. Event-stream parity is a richer signal
/// but the engine doesn't currently emit a PS-shaped log; that's a
/// future PR.
pub fn run_golden(input_path: &Path, ps_path: &Path) -> Result<GoldenReport, GoldenError> {
    let input_bytes = std::fs::read(input_path).map_err(GoldenError::Io)?;
    let input: GoldenInput =
        serde_json::from_slice(&input_bytes).map_err(GoldenError::Json)?;
    let ps_bytes = std::fs::read(ps_path).map_err(GoldenError::Io)?;
    let ps: PsOutput = serde_json::from_slice(&ps_bytes).map_err(GoldenError::Json)?;
    if !ps.ok {
        return Err(GoldenError::PsNotOk);
    }
    run_golden_in_memory(&input, &ps)
}

/// In-memory variant — useful for unit tests that don't want to touch disk.
pub fn run_golden_in_memory(
    input: &GoldenInput,
    ps: &PsOutput,
) -> Result<GoldenReport, GoldenError> {
    let format = parse_format(&input.format)?;
    let p1_team = TeamBuilder::from_showdown_text(&input.p1.team)
        .map_err(|e| GoldenError::TeamParse(format!("p1: {e:?}")))?;
    let p2_team = TeamBuilder::from_showdown_text(&input.p2.team)
        .map_err(|e| GoldenError::TeamParse(format!("p2: {e:?}")))?;

    let active_count = format.active_count();

    // Random-play mode: synthesize `turns` from the PS event stream so the
    // rest of the harness runs unchanged. Stops at first PS turn where
    // either side fainted (post-faint replacement isn't modeled in the
    // engine's step shape — see PR-201 commit message for the why).
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

    // Seed the engine RNG from PS's recorded draws. Splitmix fallback
    // uses seed[0] so unmapped engine-only draws are still deterministic.
    let events = lower_rng_events(&ps.rng);
    let fallback_seed = u64::from(input.seed[0])
        | (u64::from(input.seed[1]) << 16)
        | (u64::from(input.seed[2]) << 32)
        | (u64::from(input.seed[3]) << 48);
    let rng = Rng::oracle_partial(events, fallback_seed);

    let cfg = BattleConfig { format, seed: fallback_seed };
    let mut battle = Battle::with_rng(cfg, rng, p1_team, p2_team);

    let mut report = GoldenReport {
        name: input.name.clone().unwrap_or_else(|| "<unnamed>".into()),
        turns_run: 0,
        matched: 0,
        diverged: Vec::new(),
    };

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

        // Compare end-of-turn HP/status for every active slot the PS log
        // mentions in this turn.
        let ps_snapshots = ps_snapshots_for_turn(&ps.events, turn_no);
        let engine_snapshots = engine_snapshots(&battle, active_count);

        for (key, ps_snap) in &ps_snapshots {
            let eng_snap = engine_snapshots.iter().find(|s| s.side == key.0 && s.slot == key.1);
            match eng_snap {
                Some(eng) if snapshots_equivalent(ps_snap, eng) => {
                    report.matched += 1;
                }
                Some(eng) => {
                    report.diverged.push(Divergence {
                        turn: turn_no,
                        kind: "hp_or_status".into(),
                        ps: Some(ps_snap.clone()),
                        engine: Some(eng.clone()),
                        note: describe_snapshot_diff(ps_snap, eng),
                    });
                }
                None => {
                    report.diverged.push(Divergence {
                        turn: turn_no,
                        kind: "missing_slot".into(),
                        ps: Some(ps_snap.clone()),
                        engine: None,
                        note: "engine has no active mon in this slot".into(),
                    });
                }
            }
        }
    }

    Ok(report)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn parse_format(s: &str) -> Result<Format, GoldenError> {
    // `gen9customgame` / `gen9` → Singles (PS default for these formats
    // unless gametype overridden); `gen9doublescustomgame`,
    // `gen9vgc2024regg`, anything with "doubles" or "vgc" → Doubles.
    let lower = s.to_lowercase();
    if lower.contains("doubles") || lower.contains("vgc") {
        Ok(Format::Doubles)
    } else if lower.contains("custom") || lower.contains("ou") || lower.contains("singles") {
        Ok(Format::Singles)
    } else {
        // Be permissive — default to Singles for unknown formats and let
        // the caller fix it up via explicit "format": "doubles..." names.
        Ok(Format::Singles)
    }
}

pub(crate) fn lower_rng_events(events: &[PsRngEvent]) -> Vec<RngEvent> {
    let mut out = Vec::with_capacity(events.len());
    for e in events {
        match *e {
            PsRngEvent::Crit { value } => out.push(RngEvent::Crit(value)),
            PsRngEvent::DamageRoll { value } => {
                // PS computes `damage * (100 - random(16)) / 100` —
                // record `random(16) = v` means damage roll = 100 - v.
                // Engine uses `(85 + roll) / 100`, so roll = 15 - v
                // makes `(85 + (15 - v)) = (100 - v)` match PS.
                // sim/battle.ts:2406.
                out.push(RngEvent::DamageRoll(15u8.saturating_sub(value.min(15))));
            }
            PsRngEvent::PercentRoll { value, threshold } => {
                // PS asks randomChance(threshold, 100) and returns true
                // when roll <= threshold. Encode as the smallest passing
                // value for true, or threshold+1 for false. The engine's
                // accuracy gate is `roll <= eff_acc` (same direction).
                let v = if value {
                    threshold.clamp(1, 100)
                } else {
                    threshold.saturating_add(1).clamp(1, 100)
                };
                out.push(RngEvent::PercentRoll(v));
            }
            PsRngEvent::Range { value, bound } => {
                if bound == 16 {
                    // Same mirror-image translation as PsRngEvent::DamageRoll above.
                    out.push(RngEvent::DamageRoll(15u8.saturating_sub((value as u8).min(15))));
                } else {
                    out.push(RngEvent::Range(value));
                }
            }
            PsRngEvent::Tiebreak { ref value } => {
                let v = u64::from_str_radix(value.trim_start_matches("0x"), 16)
                    .or_else(|_| value.parse::<u64>())
                    .unwrap_or(0);
                out.push(RngEvent::Tiebreak(v));
            }
            PsRngEvent::Chance { value, num, denom } => {
                // PS `randomChance(num, denom)` returns true when
                // `random(denom) < num`. Engine consumes a `range(denom)`
                // call at the same site. Pick the smallest value that
                // reproduces PS's outcome: 0 for true (always satisfies
                // `< num` when num >= 1), or `num` for false (smallest
                // `>= num`). Clamped into `[0, denom)`.
                let v = if value {
                    0u32
                } else {
                    num.min(denom.saturating_sub(1))
                };
                out.push(RngEvent::Range(v));
            }
        }
    }
    out
}

pub(crate) fn parse_turn_actions(
    raw: &serde_json::Value,
    side: SideRef,
    active_count: usize,
) -> Result<Vec<Choice>, GoldenError> {
    // Accept either a string (singles or "cmd1, cmd2" for doubles) or
    // an array of per-slot strings.
    let per_slot: Vec<String> = match raw {
        serde_json::Value::Null => vec!["pass".into(); active_count],
        serde_json::Value::String(s) => {
            if active_count == 1 {
                vec![s.clone()]
            } else {
                s.split(',').map(|t| t.trim().to_string()).collect()
            }
        }
        serde_json::Value::Array(arr) => arr
            .iter()
            .map(|v| v.as_str().unwrap_or("pass").to_string())
            .collect(),
        other => return Err(GoldenError::BadAction(format!("unknown action shape: {other}"))),
    };

    let mut out = Vec::with_capacity(active_count);
    for slot in 0..active_count {
        let cmd = per_slot.get(slot).cloned().unwrap_or_else(|| "pass".into());
        out.push(parse_one_action(&cmd, side, slot as u8, active_count)?);
    }
    Ok(out)
}

fn parse_one_action(
    cmd: &str,
    side: SideRef,
    actor_slot: u8,
    active_count: usize,
) -> Result<Choice, GoldenError> {
    let cmd = cmd.trim();
    let lower = cmd.to_lowercase();
    if lower == "pass" || lower.is_empty() {
        return Ok(Choice::Pass { actor_slot });
    }
    if let Some(rest) = lower.strip_prefix("switch ") {
        let n: u8 = rest
            .trim()
            .parse()
            .map_err(|_| GoldenError::BadAction(format!("bad switch: {cmd}")))?;
        if n < 1 {
            return Err(GoldenError::BadAction(format!("switch index < 1: {cmd}")));
        }
        return Ok(Choice::Switch { actor_slot, team_index: n - 1 });
    }
    if let Some(rest) = lower.strip_prefix("move ") {
        let mut parts = rest.split_whitespace();
        let n: u8 = parts
            .next()
            .ok_or_else(|| GoldenError::BadAction(format!("no move slot: {cmd}")))?
            .parse()
            .map_err(|_| GoldenError::BadAction(format!("bad move slot: {cmd}")))?;
        if n < 1 || n > 4 {
            return Err(GoldenError::BadAction(format!("move slot OOB: {cmd}")));
        }
        let move_slot = n - 1;
        let mut target: Option<Target> = None;
        let mut tera = false;
        for tok in parts {
            if tok == "terastallize" || tok == "tera" {
                tera = true;
                continue;
            }
            if let Ok(t) = tok.parse::<i32>() {
                // PS relative targeting: positive = foe-side slot
                // (1 → foe slot 0, 2 → foe slot 1), negative = ally-side
                // slot (-1 → self side slot 0, -2 → self side slot 1).
                // Singles: no explicit target needed; we leave as None.
                if active_count > 1 {
                    let (tside, tslot) = if t > 0 {
                        (side.opposing(), (t as u8) - 1)
                    } else {
                        (side, ((-t) as u8) - 1)
                    };
                    target = Some(Target { side: tside, slot: tslot });
                }
            }
        }
        return Ok(if tera {
            Choice::Terastallize { actor_slot, move_slot, target }
        } else {
            Choice::Move { actor_slot, move_slot, target }
        });
    }
    Err(GoldenError::BadAction(format!("unrecognized: {cmd}")))
}

fn ps_actor_to_side_slot(s: &str) -> Option<(u8, char)> {
    // "p1a" → (1, 'a'). "p2b" → (2, 'b').
    if s.len() < 3 { return None; }
    let bytes = s.as_bytes();
    if bytes[0] != b'p' { return None; }
    let side = (bytes[1] as char).to_digit(10)? as u8;
    let slot = bytes[2] as char;
    if slot != 'a' && slot != 'b' { return None; }
    Some((side, slot))
}

/// Collect the final per-slot HP+status snapshot the PS log reports for
/// this turn. Multiple events may touch the same slot in one turn (a hit
/// then a heal); we keep the LAST one before turn boundary.
fn ps_snapshots_for_turn(
    events: &[PsEvent],
    turn: u32,
) -> std::collections::BTreeMap<(u8, char), SlotSnapshot> {
    let mut out: std::collections::BTreeMap<(u8, char), SlotSnapshot> =
        std::collections::BTreeMap::new();
    // First pass: seed from the cumulative state up to and including
    // this turn so we always have an "alive at full" baseline.
    for ev in events {
        if ev.turn > turn { break; }
        let Some(actor) = ev.actor.as_deref() else { continue };
        let Some(key) = ps_actor_to_side_slot(actor) else { continue };
        match ev.kind.as_str() {
            "switch" => {
                if let (Some(hp), Some(max)) = (ev.hp, ev.max) {
                    out.insert(key, SlotSnapshot {
                        side: key.0, slot: key.1, hp, max,
                        fainted: hp == 0,
                        status: "none".into(),
                    });
                }
            }
            "damage" | "heal" => {
                let entry = out.entry(key).or_insert(SlotSnapshot {
                    side: key.0, slot: key.1, hp: 0, max: 0,
                    fainted: false, status: "none".into(),
                });
                if let Some(hp) = ev.hp { entry.hp = hp; }
                if let Some(max) = ev.max { entry.max = max; }
                entry.fainted = entry.hp == 0;
            }
            "faint" => {
                let entry = out.entry(key).or_insert(SlotSnapshot {
                    side: key.0, slot: key.1, hp: 0, max: 0,
                    fainted: true, status: "none".into(),
                });
                entry.hp = 0;
                entry.fainted = true;
            }
            "status" => {
                let entry = out.entry(key).or_insert(SlotSnapshot {
                    side: key.0, slot: key.1, hp: 0, max: 0,
                    fainted: false, status: "none".into(),
                });
                if let Some(s) = &ev.status { entry.status = s.clone(); }
            }
            "curestatus" => {
                if let Some(entry) = out.get_mut(&key) {
                    entry.status = "none".into();
                }
            }
            _ => {}
        }
    }
    // Restrict to slots touched on THIS specific turn — we only care
    // about state changes the turn produced. Carryover state from
    // earlier turns is implicit in the entries above.
    let touched: std::collections::BTreeSet<(u8, char)> = events
        .iter()
        .filter(|e| e.turn == turn)
        .filter_map(|e| e.actor.as_deref().and_then(ps_actor_to_side_slot))
        .collect();
    out.retain(|k, _| touched.contains(k));
    out
}

fn engine_snapshots(battle: &Battle, active_count: usize) -> Vec<SlotSnapshot> {
    let mut out = Vec::new();
    for (side_ref, side_letter) in [(SideRef::P1, 1u8), (SideRef::P2, 2u8)] {
        let side = match side_ref {
            SideRef::P1 => &battle.p1,
            SideRef::P2 => &battle.p2,
        };
        for slot in 0..active_count {
            let Some(mon) = side.active_mon(slot) else { continue };
            let slot_char = if slot == 0 { 'a' } else { 'b' };
            out.push(SlotSnapshot {
                side: side_letter,
                slot: slot_char,
                hp: mon.current_hp as u32,
                max: mon.stats.hp as u32,
                fainted: mon.fainted,
                status: status_str(mon.status),
            });
        }
    }
    out
}

pub(crate) fn status_str(s: Status) -> String {
    match s {
        Status::None => "none",
        Status::Burn => "brn",
        Status::Freeze => "frz",
        Status::Paralysis => "par",
        Status::Poison => "psn",
        Status::Toxic => "tox",
        Status::Sleep => "slp",
    }.into()
}

fn snapshots_equivalent(ps: &SlotSnapshot, eng: &SlotSnapshot) -> bool {
    // HP tolerance: exact match when both maxes agree (same mon). When
    // they differ (PS used a different team-build path on the same
    // species) we'd want a ratio check, but goldens are full-info so
    // we insist on exact agreement and surface the mismatch otherwise.
    ps.hp == eng.hp
        && ps.max == eng.max
        && ps.fainted == eng.fainted
        && ps.status == eng.status
}

fn describe_snapshot_diff(ps: &SlotSnapshot, eng: &SlotSnapshot) -> String {
    let mut bits = Vec::new();
    if ps.hp != eng.hp || ps.max != eng.max {
        bits.push(format!("hp: ps {}/{} vs engine {}/{}", ps.hp, ps.max, eng.hp, eng.max));
    }
    if ps.fainted != eng.fainted {
        bits.push(format!("fainted: ps {} vs engine {}", ps.fainted, eng.fainted));
    }
    if ps.status != eng.status {
        bits.push(format!("status: ps {} vs engine {}", ps.status, eng.status));
    }
    bits.join("; ")
}

// ---------------------------------------------------------------------------
// Random-play action derivation (PR-201)
// ---------------------------------------------------------------------------
//
// When `input.random_play == true` and `input.turns` is empty, walk the
// PS event log and synthesize the same `GoldenTurn` list a hand-authored
// fixture would have. Each per-side action is a PS-formatted command
// string (`"move N"`, `"switch N"`, `"move N target"`) so the existing
// `parse_one_action` consumes it unchanged.
//
// Stops at the first PS turn where either side fainted. The engine's
// `step()` does not auto-replace a fainted slot, so any later turn would
// require a Switch on the fainted side that the OTHER side didn't make
// — desyncing the per-side action streams. (This is the same wall the
// scripted-driver attempt hit; see PR-200 commit message.) Cutting off
// at the first faint is enough to surface mechanic divergences in the
// HP / status / boost trajectory before any side falls.
//
// PS refs:
//   * sim/side.ts `chooseMove` / `chooseSwitch` — action string shape
//   * sim/battle.ts `nextTurn` — turn boundary emission

fn slugify_simple(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn move_slot_for(mon: &Pokemon, move_name: &str) -> Option<u8> {
    let want = slugify_simple(move_name);
    for (i, &mid) in mon.moves.iter().enumerate() {
        if mid == u16::MAX {
            continue;
        }
        let def = &data::MOVES[mid as usize];
        if def.slug == want {
            return Some(i as u8);
        }
    }
    None
}

fn team_index_for_species(team: &[Pokemon], species_name: &str) -> Option<u8> {
    let want = slugify_simple(species_name);
    for (i, mon) in team.iter().enumerate() {
        let sp = &data::SPECIES[mon.species_id as usize];
        if sp.slug == want {
            return Some(i as u8);
        }
    }
    None
}

fn ps_target_to_relative(actor: &str, target: &str, active_count: usize) -> String {
    // Singles: no explicit target token (engine ignores it).
    if active_count <= 1 {
        return String::new();
    }
    // Doubles relative targeting (PS-style):
    //   positive N = foe-side slot N (1 = foe slot 0, 2 = foe slot 1)
    //   negative N = self-side slot N (-1 = self slot 0, -2 = self slot 1)
    let (asid, _) = match ps_actor_to_side_slot(actor) {
        Some(v) => v,
        None => return String::new(),
    };
    let (tsid, tslot) = match ps_actor_to_side_slot(target) {
        Some(v) => v,
        None => return String::new(),
    };
    let slot_num = match tslot { 'a' => 1, 'b' => 2, _ => return String::new() };
    if tsid == asid {
        format!(" -{slot_num}")
    } else {
        format!(" {slot_num}")
    }
}

pub(crate) fn derive_turns_from_events(
    events: &[PsEvent],
    p1_team: &[Pokemon],
    p2_team: &[Pokemon],
    active_count: usize,
    max_turns: u32,
) -> Vec<GoldenTurn> {
    // First pass: scan events into per-PS-turn buckets keyed by turn number.
    // We care about the FIRST move/switch event per (side, slot) per turn —
    // that's the player's choice for that step.
    use std::collections::BTreeMap;

    // Track which mon currently occupies each (side, slot) so we can resolve
    // move name → move-slot index. Start from team-preview defaults: slot
    // 'a' = team[0], slot 'b' = team[1].
    let mut p1_active: [u8; 2] = [0, 1];
    let mut p2_active: [u8; 2] = [0, 1];

    // (side, slot_char) -> chosen action string (per turn).
    // turn_choices[turn][(side, slot)] = String
    let mut turn_choices: BTreeMap<u32, BTreeMap<(u8, char), String>> = BTreeMap::new();
    // Turns where someone fainted — bail at the first one.
    let mut faint_turn: Option<u32> = None;

    for ev in events {
        if ev.turn == 0 {
            // Pre-turn-1 events (initial switches from team preview); use
            // them to seed active-slot indices but don't emit choices.
            if ev.kind == "switch" {
                if let (Some((side, slot)), Some(species)) =
                    (ev.actor.as_deref().and_then(ps_actor_to_side_slot), ev.species.as_deref())
                {
                    let team = if side == 1 { p1_team } else { p2_team };
                    if let Some(idx) = team_index_for_species(team, species) {
                        let slot_i = if slot == 'a' { 0 } else { 1 };
                        if side == 1 {
                            p1_active[slot_i] = idx;
                        } else {
                            p2_active[slot_i] = idx;
                        }
                    }
                }
            }
            continue;
        }

        if faint_turn.is_some() && ev.turn > faint_turn.unwrap() {
            // We already saw a faint in an earlier turn — don't process
            // anything past it. (We DO let `ev.turn == faint_turn` events
            // through so we can capture the choices that were made BEFORE
            // the faint occurred.)
            break;
        }

        if ev.turn > max_turns {
            break;
        }

        match ev.kind.as_str() {
            "move" => {
                let Some((side, slot)) = ev.actor.as_deref().and_then(ps_actor_to_side_slot) else {
                    continue;
                };
                let key = (side, slot);
                let bucket = turn_choices.entry(ev.turn).or_default();
                if bucket.contains_key(&key) {
                    continue;
                }
                let slot_i = if slot == 'a' { 0 } else { 1 };
                let active_idx = if side == 1 { p1_active[slot_i] } else { p2_active[slot_i] };
                let team = if side == 1 { p1_team } else { p2_team };
                let mon = match team.get(active_idx as usize) {
                    Some(m) => m,
                    None => continue,
                };
                let name = ev.name.as_deref().unwrap_or("");
                let Some(move_slot) = move_slot_for(mon, name) else {
                    // Move not in known move list (Struggle, Sleep Talk
                    // calls, etc.). Skip — engine will Pass this slot,
                    // and the divergence will surface naturally.
                    continue;
                };
                let target_token = ev
                    .target
                    .as_deref()
                    .map(|t| ps_target_to_relative(ev.actor.as_deref().unwrap_or(""), t, active_count))
                    .unwrap_or_default();
                bucket.insert(key, format!("move {}{}", move_slot + 1, target_token));
            }
            "switch" => {
                let Some((side, slot)) = ev.actor.as_deref().and_then(ps_actor_to_side_slot) else {
                    continue;
                };
                let key = (side, slot);
                let species = ev.species.as_deref().unwrap_or("");
                let team = if side == 1 { p1_team } else { p2_team };
                let Some(new_idx) = team_index_for_species(team, species) else {
                    continue;
                };
                // Update active-slot bookkeeping (next move on this slot
                // resolves against the new mon).
                let slot_i = if slot == 'a' { 0 } else { 1 };
                if side == 1 {
                    p1_active[slot_i] = new_idx;
                } else {
                    p2_active[slot_i] = new_idx;
                }
                let bucket = turn_choices.entry(ev.turn).or_default();
                if bucket.contains_key(&key) {
                    // Already had a move/switch this turn — this is a
                    // post-faint replacement. Don't emit a choice for
                    // this turn (the faint stops derivation anyway).
                    continue;
                }
                bucket.insert(key, format!("switch {}", new_idx + 1));
            }
            "faint" => {
                let Some((_side, _slot)) = ev.actor.as_deref().and_then(ps_actor_to_side_slot)
                else {
                    continue;
                };
                if faint_turn.is_none() {
                    faint_turn = Some(ev.turn);
                }
            }
            _ => {}
        }
    }

    // Cut off STRICTLY BEFORE the faint turn. PS's snapshot for that
    // turn includes the post-faint replacement (different mon, different
    // max HP) — comparing it against the engine's still-fainted slot
    // would always look like a divergence even when the mechanics are
    // correct. By stopping the turn before, both sides remain aligned
    // and any reported divergence reflects an actual mechanic bug.
    let cutoff = faint_turn
        .map(|t| t.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(max_turns);
    let mut out = Vec::new();
    for (_turn_no, bucket) in turn_choices.range(1..=cutoff) {
        let p1_slot_a = bucket.get(&(1, 'a')).cloned().unwrap_or_else(|| "pass".into());
        let p2_slot_a = bucket.get(&(2, 'a')).cloned().unwrap_or_else(|| "pass".into());
        let (p1_str, p2_str) = if active_count >= 2 {
            let p1_slot_b = bucket.get(&(1, 'b')).cloned().unwrap_or_else(|| "pass".into());
            let p2_slot_b = bucket.get(&(2, 'b')).cloned().unwrap_or_else(|| "pass".into());
            (format!("{p1_slot_a}, {p1_slot_b}"), format!("{p2_slot_a}, {p2_slot_b}"))
        } else {
            (p1_slot_a, p2_slot_a)
        };
        out.push(GoldenTurn {
            p1: serde_json::Value::String(p1_str),
            p2: serde_json::Value::String(p2_str),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_singles_move_action() {
        let v = serde_json::json!("move 1");
        let cs = parse_turn_actions(&v, SideRef::P1, 1).unwrap();
        assert_eq!(cs.len(), 1);
        assert!(matches!(cs[0], Choice::Move { move_slot: 0, .. }));
    }

    #[test]
    fn parse_doubles_pair_action() {
        let v = serde_json::json!("move 1 1, move 2 2");
        let cs = parse_turn_actions(&v, SideRef::P1, 2).unwrap();
        assert_eq!(cs.len(), 2);
        match cs[0] {
            Choice::Move { move_slot, target: Some(t), .. } => {
                assert_eq!(move_slot, 0);
                assert_eq!(t.side, SideRef::P2);
                assert_eq!(t.slot, 0);
            }
            _ => panic!("expected move with target"),
        }
        match cs[1] {
            Choice::Move { move_slot, target: Some(t), .. } => {
                assert_eq!(move_slot, 1);
                assert_eq!(t.side, SideRef::P2);
                assert_eq!(t.slot, 1);
            }
            _ => panic!("expected move with target"),
        }
    }

    #[test]
    fn parse_switch_action() {
        let v = serde_json::json!("switch 3");
        let cs = parse_turn_actions(&v, SideRef::P1, 1).unwrap();
        match cs[0] {
            Choice::Switch { team_index, .. } => assert_eq!(team_index, 2),
            _ => panic!("expected switch"),
        }
    }

    #[test]
    fn lower_rng_percent_roll_true() {
        let evs = vec![PsRngEvent::PercentRoll { value: true, threshold: 30 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::PercentRoll(30)));
    }

    #[test]
    fn lower_rng_percent_roll_false() {
        let evs = vec![PsRngEvent::PercentRoll { value: false, threshold: 30 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::PercentRoll(31)));
    }

    #[test]
    fn lower_rng_damage_roll_mirrors_ps() {
        // PS: damage * (100 - random(16)) / 100
        // Engine: damage * (85 + roll) / 100
        // To match PS roll r, engine roll = 15 - r so (85 + 15 - r) = 100 - r.
        // sim/battle.ts:2406.
        let evs = vec![PsRngEvent::DamageRoll { value: 0 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::DamageRoll(15)), "PS roll 0 = max damage → engine roll 15");

        let evs = vec![PsRngEvent::DamageRoll { value: 15 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::DamageRoll(0)), "PS roll 15 = min damage → engine roll 0");

        let evs = vec![PsRngEvent::DamageRoll { value: 13 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::DamageRoll(2)), "PS roll 13 → engine roll 2");
    }

    #[test]
    fn lower_rng_range_bound_16_uses_damage_roll_mirror() {
        // Same mirror-image translation applies to Range events with bound=16
        // (which the driver records when it can't statically tell the call was
        // for a damage roll).
        let evs = vec![PsRngEvent::Range { value: 13, bound: 16 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::DamageRoll(2)));
    }

    #[test]
    fn lower_rng_chance_true_maps_to_range_zero() {
        // PS `randomChance(1, 3)` returning true means PS drew 0
        // (only value < 1 in [0,3) is 0). Engine should pop Range(0)
        // at the matching `range(3)` call to reproduce success.
        let evs = vec![PsRngEvent::Chance { value: true, num: 1, denom: 3 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::Range(0)));
    }

    #[test]
    fn lower_rng_chance_false_maps_to_range_num() {
        // PS `randomChance(1, 3)` returning false means PS drew 1 or 2.
        // Pick the smallest value that fails the `< num` check (num).
        let evs = vec![PsRngEvent::Chance { value: false, num: 1, denom: 3 }];
        let out = lower_rng_events(&evs);
        assert!(matches!(out[0], RngEvent::Range(1)));
    }
}

#[cfg(test)]
mod corpus_tests {
    //! Auto-discover every `<name>.input.json` / `<name>.ps.json` pair in
    //! the `goldens/` directory and run them. The corpus is intentionally
    //! one test function (rather than `#[test]` per file) so that adding
    //! a golden requires zero Rust glue — drop the two files in and they
    //! get picked up.
    //!
    //! Two gates, both default-on:
    //!   * `corpus_loads_and_runs` — verifies every golden parses, the
    //!     PS ground truth loads, and the engine completes every turn
    //!     without panicking. Fails on IO / driver / parse errors.
    //!   * `corpus_zero_divergences` — strict mechanic gate. Fails if
    //!     ANY golden has a HP / status / faint mismatch against PS.
    //!
    //! Both gates run on every `cargo test --workspace --exclude
    //! vgc-engine-py` invocation, so the workspace CI enforces full
    //! HP-level PS parity on every mechanic PR.

    use super::*;

    fn collect_goldens() -> Vec<(String, std::path::PathBuf, std::path::PathBuf)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens");
        if !dir.exists() {
            return Vec::new();
        }
        let mut out = Vec::new();
        walk_goldens(&dir, &mut out);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn walk_goldens(
        dir: &std::path::Path,
        out: &mut Vec<(String, std::path::PathBuf, std::path::PathBuf)>,
    ) {
        let entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(it) => it.filter_map(|e| e.ok()).collect(),
            Err(_) => return,
        };
        for entry in entries {
            let p = entry.path();
            if p.is_dir() {
                walk_goldens(&p, out);
                continue;
            }
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let Some(stem) = name.strip_suffix(".input.json") else { continue };
            let ps_path = p.with_file_name(format!("{stem}.ps.json"));
            // Stem includes the subdir for uniqueness (e.g. "random/seed-0").
            let rel = p
                .strip_prefix(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens"))
                .unwrap_or(&p);
            let qualified = rel
                .to_string_lossy()
                .trim_end_matches(".input.json")
                .to_string();
            out.push((qualified, p, ps_path));
            let _ = stem;
        }
    }

    #[test]
    fn corpus_loads_and_runs() {
        let goldens = collect_goldens();
        let mut failures: Vec<String> = Vec::new();
        let mut ran = 0;
        for (stem, input, ps_path) in &goldens {
            if !ps_path.exists() {
                failures.push(format!("{stem}: missing .ps.json"));
                continue;
            }
            ran += 1;
            if let Err(e) = run_golden(input, ps_path) {
                failures.push(format!("{stem}: error {e}"));
            }
        }
        if !failures.is_empty() {
            panic!(
                "{} golden(s) failed to load/run (ran {}):\n{}",
                failures.len(),
                ran,
                failures.join("\n"),
            );
        }
    }

    #[test]
    fn corpus_zero_divergences() {
        let goldens = collect_goldens();
        let mut failures: Vec<String> = Vec::new();
        for (stem, input, ps_path) in &goldens {
            if !ps_path.exists() { continue; }
            match run_golden(input, ps_path) {
                Err(e) => failures.push(format!("{stem}: error {e}")),
                Ok(report) if !report.is_ok() => {
                    let detail = report
                        .diverged
                        .iter()
                        .take(5)
                        .map(|d| format!("    turn {} [{}] {}", d.turn, d.kind, d.note))
                        .collect::<Vec<_>>()
                        .join("\n");
                    failures.push(format!(
                        "{stem}: {} divergences (matched {})\n{}",
                        report.diverged.len(),
                        report.matched,
                        detail,
                    ));
                }
                Ok(_) => {}
            }
        }
        if !failures.is_empty() {
            panic!(
                "{} golden(s) diverged from PS:\n{}",
                failures.len(),
                failures.join("\n\n"),
            );
        }
    }
}
