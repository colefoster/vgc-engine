//! Real replay -> engine position (docs/protocol.md, "Start from a real
//! replay").
//!
//! [`reconstruct`] reads a Showdown protocol log (a replay's `log`, PS's raw
//! `battle.log` with `|split|` pairs, or one player's stream) up to the
//! start of turn N (or the faint-replacement stop before it) and rebuilds the
//! battle there:
//!
//! * what the log shows is copied (species / forme, HP, status, boosts,
//!   volatiles and their turn counts, field effects, items, abilities, moves
//!   and their PP, turns on the field, choice lock, last move);
//! * what it does not show (unrevealed moves, items, abilities, spreads,
//!   unseen brought mons, hidden counters) is filled from a usage prior
//!   conditioned on what was revealed ([`crate::prior`]);
//! * every field carries its provenance (`exact` / `inferred` / `prior` /
//!   `default`) in the report.
//!
//! The state goes through the kcheck board builder
//! ([`vgc_engine_core::ps_board::position_from_board`]) and is then patched
//! with what a board does not carry (PS team order, Protect streak, turns
//! active / move actions, ...). The prefix (the log up to the stop) becomes
//! the position's history, so each player's stream carries the real past.

use std::collections::HashSet;

use serde_json::{json, Map, Value};
use vgc_engine_core::ps_position::{raw_channel, Prefix};
use vgc_engine_core::{data, nature_by_id, Battle, SideRef, StatSpread, Status, TeamMember, VolatileKind};

use crate::prior::{slug, Prior, PriorSampler};

mod build;
mod illusion;
mod recon;
pub use recon::reconstruct;

/// The Illusion decoder alone (docs/protocol.md, "Illusion"): `lines` with
/// the stints that the evidence in `evidence` (`lines`, or a longer log
/// starting with them) gives to the Illusion holder renamed to it.
#[doc(hidden)]
pub fn decode_illusion(lines: &[String], evidence: &[String]) -> Vec<String> {
    illusion::decode(lines, evidence).lines
}

/// Where the position stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The move request of turn N (after `|turn|N`).
    Move,
    /// The faint-replacement request before turn N (end of turn N-1).
    Replace,
}

/// What kind of log was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// PS's raw `battle.log`: `|split|pN` + secret + shared lines.
    Raw,
    /// One channel everyone sees (a replay's `log`): percentages only.
    Spectator,
    /// One player's stream: that side's HP exact, `|request|` lines possible.
    PerSide(SideRef),
}

impl InputKind {
    fn name(self) -> String {
        match self {
            InputKind::Raw => "raw".into(),
            InputKind::Spectator => "spectator".into(),
            InputKind::PerSide(s) => format!("per_side:{}", side_key(s)),
        }
    }
}

#[derive(Debug)]
pub struct ReconOptions {
    pub turn: u32,
    pub stop: Stop,
    /// Only this player's knowledge fills the opponent's hidden info.
    pub viewer: Option<SideRef>,
    /// Full sets per side (team sheets). In Champions formats `evs` are
    /// stat points, as PS Champions exports carry them.
    pub sets: [Option<Vec<TeamMember>>; 2],
    /// `|request|` JSON per side at the stop (exact own team).
    pub requests: [Option<Value>; 2],
    /// Use set facts (moves, items, abilities, brought mons) revealed after
    /// the stop. Off by default: only the log up to the stop is read.
    pub lookahead: bool,
    /// PS format id; default from `|tier|`, else Champions Reg M-C.
    pub format: Option<String>,
    /// Force the input kind (otherwise detected).
    pub kind: Option<InputKind>,
    /// Sample prior-filled fields (spread and nature, unrevealed moves,
    /// item, ability) from the prior with this seed instead of taking the
    /// most likely. `None`: the argmax, unchanged.
    pub prior_sample_seed: Option<u64>,
}

impl ReconOptions {
    pub fn new(turn: u32) -> Self {
        ReconOptions { turn, stop: Stop::Move, viewer: None, sets: [None, None], requests: [None, None], lookahead: false, format: None, kind: None, prior_sample_seed: None }
    }
}

/// A failed reconstruction: a short `bucket` (for failure tables) and detail.
#[derive(Debug, Clone)]
pub struct ReconError {
    pub bucket: String,
    pub detail: String,
}

impl std::fmt::Display for ReconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.bucket, self.detail)
    }
}

fn err(bucket: &str, detail: impl Into<String>) -> ReconError {
    ReconError { bucket: bucket.to_string(), detail: detail.into() }
}

pub struct Reconstruction {
    /// The rebuilt battle (no log attached; see [`Reconstruction::prefix`]).
    pub battle: Battle,
    pub format_id: String,
    pub prefix: Prefix,
    pub player_names: [Option<String>; 2],
    pub report: Value,
    /// The kcheck board and sets the battle was built from.
    pub board: Value,
    pub sets: Value,
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// Shown by the input (log, request, given sets).
    Exact,
    /// Derived from the input by a rule (an HP interval's midpoint, a
    /// stream-counted counter, a guessed team order).
    Inferred,
    /// Filled from the usage prior.
    Prior,
    /// Neither shown nor in the prior: a fixed default.
    Default,
}

impl Src {
    fn as_str(self) -> &'static str {
        match self {
            Src::Exact => "exact",
            Src::Inferred => "inferred",
            Src::Prior => "prior",
            Src::Default => "default",
        }
    }
}

fn prov(v: Value, src: Src, p: Option<f64>) -> Value {
    let mut o = json!({"value": v, "src": src.as_str()});
    if let Some(p) = p {
        o["p"] = json!((p * 1000.0).round() / 1000.0);
    }
    o
}

// ---------------------------------------------------------------------------
// Input files and the session
// ---------------------------------------------------------------------------

/// A log as given: its lines, a `|request|` that came with it, and the
/// decision turn a ps_bot decision file names.
#[derive(Debug, Clone, Default)]
pub struct LogInput {
    pub lines: Vec<String>,
    pub request: Option<(SideRef, Value)>,
    pub turn: Option<u32>,
    /// A decision file's stop: its request's kind (a `forceSwitch` at the
    /// end of a turn is the replacement stop before the next one).
    pub stop: Option<Stop>,
}

/// Read a log: a replay JSON (`log`), a mimikyu ps_bot decision file
/// (`decide_payload.live.protocol_log` + `request`), a JSON list of lines,
/// or plain text.
pub fn parse_input(text: &str) -> Result<LogInput, String> {
    let t = text.trim_start();
    if !(t.starts_with('{') || t.starts_with('[')) {
        return Ok(LogInput { lines: split_lines(text), ..Default::default() });
    }
    let v: Value = serde_json::from_str(t).map_err(|e| format!("log json: {e}"))?;
    if let Some(a) = v.as_array() {
        return Ok(LogInput { lines: a.iter().filter_map(|x| x.as_str()).flat_map(split_lines).collect(), ..Default::default() });
    }
    if let Some(log) = v.get("log").and_then(Value::as_str) {
        return Ok(LogInput { lines: split_lines(log), ..Default::default() });
    }
    let live = v.pointer("/decide_payload/live").or_else(|| v.get("live")).unwrap_or(&v);
    if let Some(pl) = live.get("protocol_log").and_then(Value::as_array) {
        let lines: Vec<String> = pl.iter().filter_map(|x| x.as_str()).flat_map(split_lines).collect();
        let req = live.get("request").filter(|r| r.is_object()).or_else(|| v.get("request").filter(|r| r.is_object())).cloned();
        let request = req.and_then(|r| {
            let side = match r.pointer("/side/id").and_then(Value::as_str)? {
                "p1" => SideRef::P1,
                "p2" => SideRef::P2,
                _ => return None,
            };
            Some((side, r))
        });
        let mut turn = v.get("turn").and_then(Value::as_u64).map(|x| x as u32);
        let mut stop = None;
        if request.as_ref().is_some_and(|(_, r)| r.get("forceSwitch").is_some()) {
            // A faint replacement after the turn's residuals is the stop
            // before the next turn; a switch mid-turn (U-turn, Eject
            // Button) has no position form.
            let last_turn = lines.iter().rposition(|l| l.starts_with("|turn|"));
            let last_upkeep = lines.iter().rposition(|l| l == "|upkeep");
            if last_upkeep > last_turn {
                stop = Some(Stop::Replace);
                turn = turn.map(|t| t + 1);
            } else {
                return Err("a mid-turn switch request (U-turn, Eject Button, ...) has no position form".into());
            }
        } else if turn.is_some() {
            stop = Some(Stop::Move);
        }
        return Ok(LogInput { lines, request, turn, stop });
    }
    Err("log json: no `log` text or `protocol_log` lines".into())
}

impl Reconstruction {
    /// The battle JSON (`to_json` form plus `format_id`), as
    /// `Battle.from_position` takes it.
    pub fn battle_json(&self) -> Value {
        let mut v = serde_json::to_value(&self.battle).unwrap_or(Value::Null);
        v["format_id"] = json!(self.format_id);
        v
    }

    /// The prefix as JSON: a raw line list, or `{"p1": [...], "p2": [...]}`.
    pub fn prefix_json(&self) -> Value {
        match &self.prefix {
            Prefix::None => Value::Null,
            Prefix::Raw(l) => json!(l),
            Prefix::PerSide([a, b]) => json!({"p1": a, "p2": b}),
        }
    }

    /// Attach the prefix as the battle's log and open a PS session at the
    /// stop. `opts` defaults to PS mode with the prefix's player names.
    pub fn session(&self, opts: Option<vgc_engine_core::ProtocolOptions>) -> Result<vgc_engine_core::ps_session::PsSession, String> {
        let raw = vgc_engine_core::ps_position::prefix_raw_lines(&self.prefix)?;
        let opts = opts.unwrap_or_else(|| {
            let mut o = vgc_engine_core::ProtocolOptions::new(vgc_engine_core::ProtocolMode::Ps);
            let names = vgc_engine_core::ps_position::prefix_player_names(&raw);
            o.player_names = [names[0].clone().unwrap_or_else(|| "Player 1".into()), names[1].clone().unwrap_or_else(|| "Player 2".into())];
            o
        });
        let mut b = self.battle.clone();
        b.attach_position_log(&opts, &self.format_id, raw, false)?;
        let mut s = vgc_engine_core::ps_session::PsSession::from_battle(b);
        s.set_support_cancel(vgc_engine_core::ps_formats::lookup(&self.format_id).map(|f| f.support_cancel).unwrap_or(true));
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn side_key(s: SideRef) -> &'static str {
    match s {
        SideRef::P1 => "p1",
        SideRef::P2 => "p2",
    }
}

fn side_ref(i: usize) -> SideRef {
    if i == 0 { SideRef::P1 } else { SideRef::P2 }
}

fn species_def(name: &str) -> Option<(u16, &'static data::SpeciesDef)> {
    let s = slug(name.trim_end_matches("-*"));
    let id = data::SPECIES.iter().position(|x| x.slug == s).map(|i| i as u16).or_else(|| data::cosmetic_base(&s))?;
    Some((id, &data::SPECIES[id as usize]))
}

fn move_def(name: &str) -> Option<(u16, &'static data::MoveDef)> {
    let s = slug(name);
    data::MOVES.iter().position(|x| x.slug == s).map(|i| (i as u16, &data::MOVES[i]))
}

fn item_name(s: &str) -> Option<&'static str> {
    let s = slug(s);
    data::ITEMS.iter().find(|x| x.slug == s).map(|x| x.name)
}

fn ability_name(s: &str) -> Option<&'static str> {
    let s = slug(s);
    data::ABILITIES.iter().find(|x| x.slug == s).map(|x| x.name)
}

fn move_name(s: &str) -> Option<&'static str> {
    move_def(s).map(|(_, d)| d.name)
}

/// The mega forme's stone, if `species_id` is a Mega forme.
fn mega_of(species_id: u16) -> Option<&'static data::MegaStone> {
    data::MEGA_STONES.iter().find(|m| m.mega_species_id == species_id)
}

/// Same Pokédex entry (forme family).
fn same_family(a: &str, b: &str) -> bool {
    match (species_def(a), species_def(b)) {
        (Some((_, x)), Some((_, y))) => x.num == y.num,
        _ => slug(a) == slug(b),
    }
}

/// `p1a: Name` / `p1: Name` -> (side, slot, name).
fn parse_ident(s: &str) -> Option<(usize, Option<usize>, String)> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 2 || b[0] != b'p' {
        return None;
    }
    let side = match b[1] {
        b'1' => 0,
        b'2' => 1,
        _ => return None,
    };
    let (pos, rest) = s.split_once(':').map(|(a, r)| (a, r.trim_start())).unwrap_or((s, ""));
    let slot = match pos.as_bytes().get(2) {
        Some(b'a') => Some(0),
        Some(b'b') => Some(1),
        Some(b'c') => Some(2),
        _ => None,
    };
    Some((side, slot, rest.to_string()))
}

/// `"Salamence-Mega, L50, M, shiny"` -> (species, level, gender).
fn parse_details(d: &str) -> (String, u8, String) {
    let mut it = d.split(',').map(str::trim);
    let species = it.next().unwrap_or("").to_string();
    let mut level = 100;
    let mut gender = String::new();
    for seg in it {
        if let Some(l) = seg.strip_prefix('L').and_then(|x| x.parse().ok()) {
            level = l;
        } else if seg == "M" || seg == "F" {
            gender = seg.to_string();
        }
    }
    (species, level, gender)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum HpObs {
    Exact(u16, u16),
    /// Displayed percentage and the Champions colour band letter.
    Pct(u16, Option<char>),
}

/// `"82/100"`, `"0 fnt"`, `"45/100 par"`, `"20/100y"` -> (num, den, band, status, fainted).
fn parse_hp(s: &str) -> Option<(u16, u16, Option<char>, Option<String>, bool)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (frac, status) = match s.split_once(' ') {
        Some((a, b)) => (a, Some(b.trim().to_string())),
        None => (s, None),
    };
    if status.as_deref() == Some("fnt") || frac == "0" {
        return Some((0, 100, None, None, true));
    }
    let (n, d) = frac.split_once('/')?;
    let (d, band) = match d.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => (&d[..d.len() - 1], Some(c)),
        _ => (d, None),
    };
    Some((n.parse().ok()?, d.parse().ok()?, band, status, false))
}

/// The HP values with max `max` that PS shows as `pct` (Champions: floor,
/// at least 1, colour band at 20 / 50; else gen 7+: ceil, 99 for a scratch).
fn pct_interval(pct: u16, band: Option<char>, max: u16, champions: bool) -> (u16, u16) {
    let mut lo = u16::MAX;
    let mut hi = 0;
    for h in 1..=max as u32 {
        let m = max as u32;
        let shown = if champions {
            (100 * h / m).max(1)
        } else {
            let p = (100 * h).div_ceil(m);
            if p == 100 && h < m { 99 } else { p }
        };
        if shown != pct as u32 {
            continue;
        }
        if champions {
            let ok = match (shown, band) {
                (20, Some('y')) => h * 5 > m,
                (20, Some('r')) => h * 5 <= m,
                (50, Some('g')) => h * 2 > m,
                (50, Some('y')) => h * 2 <= m,
                _ => true,
            };
            if !ok {
                continue;
            }
        }
        lo = lo.min(h as u16);
        hi = hi.max(h as u16);
    }
    if lo == u16::MAX {
        // Inconsistent with this max HP (a prior spread off the real one):
        // scale the percentage.
        let h = ((pct as u32 * max as u32 + 50) / 100).clamp(1, max as u32) as u16;
        return (h, h);
    }
    (lo, hi)
}

/// PS's shared health text for `hp / max` (protocol.rs `health`).
fn pct_text(hp: u16, max: u16, champions: bool) -> String {
    let (h, m) = (hp as u32, max.max(1) as u32);
    if champions {
        let pct = (100 * h / m).max(1);
        let mut s = format!("{pct}/100");
        if pct == 20 {
            s.push(if h * 5 > m { 'y' } else { 'r' });
        } else if pct == 50 {
            s.push(if h * 2 > m { 'g' } else { 'y' });
        }
        s
    } else {
        let mut pct = (100 * h).div_ceil(m);
        if pct == 100 && h < m {
            pct = 99;
        }
        format!("{pct}/100")
    }
}

const STALL_MOVES: &[&str] = &["protect", "detect", "spikyshield", "kingsshield", "banefulbunker", "burningbulwark", "obstruct", "silktrap", "maxguard", "endure", "wideguard", "quickguard"];

/// Abilities PS always announces when their holder enters: a mon that came
/// in without the message does not have one.
const ANNOUNCED: &[&str] = &[
    "intimidate", "drizzle", "drought", "sandstream", "snowwarning", "electricsurge", "psychicsurge", "grassysurge", "mistysurge", "pressure", "unnerve",
    "moldbreaker", "teravolt", "turboblaze", "neutralizinggas", "airlock", "cloudnine", "intrepidsword", "dauntlessshield", "orichalcumpulse", "hadronengine",
    "asoneglastrier", "asonespectrier", "screencleaner", "darkaura", "fairyaura", "aurabreak", "comatose", "megasol",
];

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Lines of a log text (`\n` separated, `\r` dropped).
pub fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect()
}

/// Detect the input kind: `|split|` lines = raw; exact (non-/100) HP on one
/// side or `|request|` lines = that side's stream; else spectator.
pub fn detect_kind(lines: &[String]) -> InputKind {
    if lines.iter().any(|l| l.starts_with("|split|")) {
        return InputKind::Raw;
    }
    for l in lines.iter().filter(|l| l.starts_with("|request|")) {
        if let Ok(v) = serde_json::from_str::<Value>(&l[9..]) {
            match v.pointer("/side/id").and_then(Value::as_str) {
                Some("p1") => return InputKind::PerSide(SideRef::P1),
                Some("p2") => return InputKind::PerSide(SideRef::P2),
                _ => {}
            }
        }
    }
    let mut exact = [0usize; 2];
    for l in lines {
        let p: Vec<&str> = l.split('|').collect();
        let hp_arg = match p.get(1).copied() {
            Some("switch" | "drag" | "replace") => p.get(4),
            Some("-damage" | "-heal" | "-sethp") => p.get(3),
            _ => None,
        };
        let (Some(id), Some(hp)) = (p.get(2), hp_arg) else { continue };
        if let (Some((side, _, _)), Some((_, den, _, _, false))) = (parse_ident(id), parse_hp(hp)) {
            if den != 100 {
                exact[side] += 1;
            }
        }
    }
    match (exact[0] > 0, exact[1] > 0) {
        (true, false) => InputKind::PerSide(SideRef::P1),
        (false, true) => InputKind::PerSide(SideRef::P2),
        // An omniscient channel (every secret half): exact HP for both.
        (true, true) => InputKind::Raw,
        _ => InputKind::Spectator,
    }
}

/// Lines a battle prefix keeps from a replay or a player stream: chat,
/// joins / leaves, renames, inactivity notes and bare text are dropped;
/// every battle message (and `|t:|`, `|raw|`, `|uhtml|`) stays. A raw
/// `battle.log` is kept whole.
fn keep_for_prefix(l: &str) -> bool {
    if l.is_empty() || l == "|" {
        return true;
    }
    if !l.starts_with('|') {
        return false;
    }
    let tag = l.split('|').nth(1).unwrap_or("");
    !matches!(tag, "j" | "J" | "l" | "L" | "n" | "N" | "c" | "c:" | "chat" | "inactive" | "inactiveoff" | "join" | "leave" | "name" | "error" | "bigerror")
}

/// Index just past the stop in `lines`.
fn cut_index(lines: &[String], turn: u32, stop: Stop) -> Result<usize, ReconError> {
    let tl = format!("|turn|{turn}");
    let found = lines.iter().position(|l| *l == tl);
    // A stream that stops at the replacement request (a player's stream up
    // to its decision) ends after turn N-1's residuals.
    let at_end = found.is_none() && stop == Stop::Replace && lines.iter().rev().find(|l| l.starts_with("|turn|")).is_some_and(|l| *l == format!("|turn|{}", turn.saturating_sub(1)));
    let t = match found {
        Some(t) => t,
        None if at_end => lines.len(),
        None => return Err(err("no such turn", format!("the log has no `|turn|{turn}`"))),
    };
    match stop {
        Stop::Move => Ok(t + 1),
        Stop::Replace => {
            let u = lines[..t].iter().rposition(|l| l == "|upkeep").ok_or_else(|| err("no replacement stop", format!("no end of turn before turn {turn}")))?;
            let has_switch = at_end || lines[u..t].iter().any(|l| l.starts_with("|switch|") || l.starts_with("|drag|"));
            let fainted = lines[..u].iter().rev().take_while(|l| !l.starts_with("|turn|")).any(|l| l.starts_with("|faint|"));
            if !has_switch || !fainted {
                return Err(err("no replacement stop", format!("no faint replacement before turn {turn}")));
            }
            Ok(u + 1)
        }
    }
}

// ---------------------------------------------------------------------------
// The log tracker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Vol {
    /// Board name (TAUNT, ENCORE, ..., PERISH2, PROTOSYNTHESISATK).
    name: String,
    /// Line index of its `-start` (or the move that set it).
    line: usize,
    arg: String,
}

#[derive(Debug, Clone, Default)]
struct Mon {
    preview: String,
    /// Current forme (details).
    species: String,
    /// The set's species (before Mega Evolution).
    set_species: String,
    level: u8,
    gender: String,
    name: Option<String>,
    first_seen: Option<usize>,
    seen: bool,
    hp: Option<HpObs>,
    fainted: bool,
    status: String,
    status_line: usize,
    toxic_ticks: u32,
    sleep_cants: u32,
    boosts: [i8; 7],
    active: Option<usize>,
    item_now: Option<Option<String>>,
    item_orig: Option<String>,
    item_lost: bool,
    item_knocked: bool,
    item_lost_line: Option<usize>,
    item_changed: bool,
    /// PS `lastItem`: the item it used up (ate, consumed) this stint.
    used_item: Option<String>,
    ability_base: Option<String>,
    ability_now: Option<String>,
    moves: Vec<String>,
    pp_used: Vec<u32>,
    tera: Option<String>,
    terastallized: bool,
    mega: bool,
    vols: Vec<Vol>,
    last_move: Option<String>,
    stint_moves: Vec<String>,
    stint_status_move: bool,
    move_actions: u32,
    turns_active: u32,
    switch_line: usize,
    stall_count: u8,
    /// A stall move used this turn, and whether it worked.
    stall_used: bool,
    stall_ok: bool,
    /// A stall move used last turn (the engine's used-this-turn flag at the stop).
    stall_used_last: bool,
    times_attacked: u32,
    must_recharge: bool,
    sub: bool,
    type_change: Option<String>,
    fallen: Option<u8>,
    /// Displayed HP at or below 50% / 25% while alive (pinch-berry checks).
    went_below_half: bool,
    went_below_quarter: bool,
    /// Used two different moves in one stint, or a status move: no Choice item.
    not_choice: bool,
    any_status_move: bool,
    /// Hit a target with a damaging move and showed no Life Orb recoil.
    hit_without_lo: bool,
    /// A two-turn move charged (`-prepare`) and not yet released.
    charging: Option<String>,
    /// The chosen target of its last move (`side * 2 + slot`), if single-target.
    last_target: Option<u8>,
    /// PP used per move during the current stint, and the line each move
    /// was first shown (an Illusion reveal hands them to the real mon).
    stint_pp: Vec<(String, u32)>,
    move_line: Vec<usize>,
    /// Its state before the current switch-in (an Illusion reveal restores it).
    pre_switch: Option<Box<Mon>>,
    /// Stat swaps / splits since it came in (Speed Swap, Power Split, ...):
    /// (line, kind, the other mon).
    stat_ops: Vec<(usize, String, Option<(usize, usize)>)>,
    /// Transformed (Transform / Imposter) into this mon this stint, and the
    /// PP its copied moves used since.
    transformed: Option<(usize, usize)>,
    /// The name it shows while disguised by Illusion.
    disguise: Option<String>,
    trans_pp: Vec<(String, u32)>,
    unsupported: Vec<String>,
    /// Line ranges the Illusion decoder gave this mon (it was disguised
    /// there), with the provenance of that attribution and whether a
    /// `|replace|` showed it.
    illusion: Vec<(usize, usize, Src, bool)>,
    /// Some stint showed this mon's name while the Illusion holder was
    /// behind it: it is in the party (brought).
    disguise_target: bool,
}

#[derive(Debug, Clone, Default)]
struct FieldFx {
    name: String,
    line: usize,
    /// (side, mon index) of the setter, if known.
    setter: Option<(usize, usize)>,
}

#[derive(Debug, Clone, Default)]
struct Tracker {
    champions: bool,
    names: [Option<String>; 2],
    tier: Option<String>,
    mons: [Vec<Mon>; 2],
    teamsize: [Option<usize>; 2],
    showteam: [Option<String>; 2],
    /// Active slot -> mon index.
    slots: [[Option<usize>; 2]; 2],
    /// Switch events in order: (side, slot, mon) — for PS team order.
    switches: Vec<(usize, usize, usize)>,
    /// Ally Switch events: (side, line).
    ally_swaps: Vec<usize>,
    leads: [[Option<usize>; 2]; 2],
    weather: Option<FieldFx>,
    terrain: Option<FieldFx>,
    pseudo: Vec<FieldFx>,
    side_fx: [Vec<FieldFx>; 2],
    mega_used: [bool; 2],
    tera_used: [bool; 2],
    turn_lines: Vec<usize>,
    /// The last `|move|` user (side, mon) and its line, for setters.
    last_mover: Option<(usize, usize)>,
    /// The move being resolved: (side, mon, damaging, hit a foe, Life Orb shown).
    action: Option<(usize, usize, bool, bool, bool)>,
    /// The last `|move|` line's target slot (side, slot).
    last_move_slot_target: Option<(usize, usize)>,
    /// Pending Future Sight / Doom Desire hits: (target side, target slot,
    /// source (side, mon), move, line).
    futures: Vec<(usize, usize, (usize, usize), String, usize)>,
    lines: Vec<String>,
    unsupported: Vec<String>,
}

impl Tracker {
    fn new(champions: bool) -> Self {
        Tracker { champions, ..Default::default() }
    }

    /// The mon a line names; assigns an unseen preview entry on first sight.
    /// The mon a switch line names; assigns an unseen preview entry on
    /// first sight. Two mons may share a name (Floette and
    /// Floette-Eternal): the details species tells them apart.
    fn mon_of(&mut self, ident: &str, details: Option<&str>, line: usize) -> Option<(usize, usize)> {
        let (side, slot, name) = parse_ident(ident)?;
        if name.is_empty() {
            return slot.and_then(|s| self.slots[side][s]).map(|i| (side, i));
        }
        let det_species = details.map(|d| parse_details(d).0).unwrap_or_else(|| name.clone());
        let named: Vec<usize> = (0..self.mons[side].len()).filter(|&i| self.mons[side][i].name.as_deref() == Some(name.as_str())).collect();
        let same_species = |m: &Mon| [&m.species, &m.set_species, &m.preview].iter().any(|x| slug(x) == slug(&det_species));
        if let Some(&i) = named.iter().find(|&&i| same_species(&self.mons[side][i])) {
            return Some((side, i));
        }
        let unnamed_exact = self.mons[side].iter().position(|m| m.name.is_none() && slug(&m.preview) == slug(&det_species));
        if unnamed_exact.is_none() {
            // A forme change since (Mega Evolution): the one mon of that name.
            if let Some(&i) = named.iter().find(|&&i| same_family(&self.mons[side][i].preview, &det_species)) {
                return Some((side, i));
            }
        }
        let pick = unnamed_exact
            .or_else(|| self.mons[side].iter().position(|m| m.name.is_none() && same_family(&m.preview, &det_species)))
            .or_else(|| self.mons[side].iter().position(|m| m.name.is_none() && same_family(&m.preview, &name)));
        let i = match pick {
            Some(i) => i,
            None => {
                self.mons[side].push(Mon { preview: det_species.clone(), species: det_species.clone(), set_species: det_species.clone(), level: 50, ..Default::default() });
                self.mons[side].len() - 1
            }
        };
        let m = &mut self.mons[side][i];
        m.name = Some(name);
        if m.first_seen.is_none() {
            m.first_seen = Some(line);
        }
        Some((side, i))
    }

    /// The mon a line names: the one in that slot if it has the name, else
    /// the (active, then any) mon of that name.
    fn mon_by_ident(&self, ident: &str) -> Option<(usize, usize)> {
        let (side, slot, name) = parse_ident(ident)?;
        if let Some(i) = slot.and_then(|s| self.slots[side].get(s).copied().flatten()) {
            if name.is_empty() || self.mons[side][i].name.as_deref() == Some(name.as_str()) || self.mons[side][i].disguise.as_deref() == Some(name.as_str()) {
                return Some((side, i));
            }
        }
        if name.is_empty() {
            return None;
        }
        let named = |i: &usize| self.mons[side][*i].name.as_deref() == Some(name.as_str());
        (0..self.mons[side].len()).filter(named).find(|&i| self.mons[side][i].active.is_some()).or_else(|| (0..self.mons[side].len()).find(named)).map(|i| (side, i))
    }

    fn set_hp(&mut self, side: usize, i: usize, hp: &str, exact_side: [bool; 2]) {
        let Some((n, d, band, status, fnt)) = parse_hp(hp) else { return };
        let m = &mut self.mons[side][i];
        if fnt {
            m.fainted = true;
            m.hp = Some(HpObs::Exact(0, d));
            return;
        }
        m.fainted = false;
        m.hp = Some(if d != 100 || exact_side[side] { HpObs::Exact(n, d) } else { HpObs::Pct(n, band) });
        let frac = n as f64 / d.max(1) as f64;
        if frac <= 0.5 {
            m.went_below_half = true;
        }
        if frac <= 0.25 {
            m.went_below_quarter = true;
        }
        let st = status.unwrap_or_default();
        if st != m.status {
            if st == "tox" && m.status != "tox" || st == "slp" && m.status != "slp" {
                m.toxic_ticks = 0;
                m.sleep_cants = 0;
            }
            m.status = st;
        }
    }

    fn reveal_item(&mut self, side: usize, i: usize, item: &str) {
        let m = &mut self.mons[side][i];
        let it = item.to_string();
        if m.item_orig.is_none() && !m.item_changed && !m.item_lost {
            m.item_orig = Some(it.clone());
        }
        m.item_now = Some(Some(it));
    }

    fn reveal_ability(&mut self, side: usize, i: usize, ab: &str) {
        let m = &mut self.mons[side][i];
        if m.ability_now.is_some() {
            return;
        }
        if m.mega {
            return;
        }
        if m.ability_base.is_none() {
            m.ability_base = Some(ab.to_string());
        }
    }

    /// `[from] item: X` / `[from] ability: X` on a line: the owner is `[of]`
    /// when given, else the subject.
    fn from_reveals(&mut self, p: &[&str], subject: Option<(usize, usize)>, tag: &str) {
        let from = p.iter().find_map(|x| x.strip_prefix("[from] ").or_else(|| x.strip_prefix("[from]")));
        let of = p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id));
        let Some(from) = from else { return };
        if let Some(it) = from.strip_prefix("item: ") {
            // `-item|A|X|[from] ability: Frisk|[of] B`: B's ability, A's item.
            let owner = if tag == "-item" || tag == "-enditem" { subject } else { of.or(subject) };
            if let Some((s, i)) = owner {
                self.reveal_item(s, i, it);
            }
        } else if let Some(ab) = from.strip_prefix("ability: ") {
            let owner = if tag == "-ability" { of } else { of.or(subject) };
            if let Some((s, i)) = owner {
                self.reveal_ability(s, i, ab);
            }
        }
    }

    fn switch_out(&mut self, side: usize, i: usize) {
        let m = &mut self.mons[side][i];
        m.active = None;
        m.boosts = [0; 7];
        m.vols.clear();
        m.sub = false;
        m.must_recharge = false;
        m.type_change = None;
        m.fallen = None;
        m.stall_count = 0;
        m.stall_used = false;
        m.stall_ok = false;
        m.stall_used_last = false;
        m.last_move = None;
        m.stint_moves.clear();
        m.stint_status_move = false;
        m.charging = None;
        m.last_target = None;
        m.transformed = None;
        m.trans_pp.clear();
        m.disguise = None;
        // Skill Swap / Trace / Mummy changes end on switch-out.
        m.ability_now = None;
    }

    fn feed(&mut self, lines: &[String], exact_side: [bool; 2]) {
        self.lines = lines.to_vec();
        for (li, line) in lines.iter().enumerate() {
            let p: Vec<&str> = line.split('|').collect();
            let tag = p.get(1).copied().unwrap_or("");
            if matches!(tag, "move" | "cant" | "turn" | "upkeep" | "switch" | "drag" | "") {
                self.end_action();
            }
            match tag {
                "player" => {
                    if let (Some(id), Some(n)) = (p.get(2), p.get(3)) {
                        let k = if *id == "p1" { 0 } else if *id == "p2" { 1 } else { continue };
                        if !n.is_empty() {
                            self.names[k] = Some(n.to_string());
                        }
                    }
                }
                "tier" => self.tier = p.get(2).map(|s| s.to_string()),
                "poke" => {
                    let side = match p.get(2) {
                        Some(&"p1") => 0,
                        Some(&"p2") => 1,
                        _ => continue,
                    };
                    let (sp, lvl, g) = parse_details(p.get(3).unwrap_or(&""));
                    self.mons[side].push(Mon { preview: sp.clone(), species: sp.clone(), set_species: sp, level: lvl, gender: g, ..Default::default() });
                }
                "teamsize" => {
                    if let (Some(id), Some(n)) = (p.get(2), p.get(3).and_then(|x| x.parse().ok())) {
                        let k = if *id == "p1" { 0 } else { 1 };
                        self.teamsize[k] = Some(n);
                    }
                }
                "showteam" => {
                    if let Some(id) = p.get(2) {
                        let k = if *id == "p1" { 0 } else { 1 };
                        self.showteam[k] = Some(p[3..].join("|"));
                    }
                }
                "switch" | "drag" => {
                    let (Some(id), Some(det)) = (p.get(2), p.get(3)) else { continue };
                    let Some((_, slot, _)) = parse_ident(id) else { continue };
                    let Some(slot) = slot else { continue };
                    let Some((side, mut i)) = self.mon_of(id, Some(det), li) else { continue };
                    // The same mon in the other slot too: one of the two is an
                    // Illusion holder in disguise. The one that used a move its
                    // shown species cannot learn is; else the newcomer.
                    if let Some(other) = self.mons[side][i].active.filter(|&o| o != slot) {
                        let z = (0..self.mons[side].len()).find(|&z| z != i && self.mons[side][z].active.is_none() && ["zoroark", "zorua"].iter().any(|x| slug(&self.mons[side][z].preview).starts_with(x)));
                        match z {
                            Some(z) => {
                                let zp = self.mons[side][z].preview.clone();
                                let zname = self.mons[side][z].name.clone().unwrap_or_else(|| zp.split('-').next().unwrap_or("").to_string());
                                let shown = species_def(&self.mons[side][i].species).map(|x| x.0);
                                let odd_move = self.mons[side][i].stint_moves.iter().any(|mv| match (shown, move_def(mv)) {
                                    (Some(sid), Some((mid, _))) => !data::species_can_learn(sid, mid),
                                    _ => false,
                                });
                                let real = self.illusion_target(side, &zname, &zp, li);
                                if odd_move {
                                    self.illusion_swap(side, other, real);
                                } else {
                                    let name = self.mons[side][i].name.clone();
                                    i = real;
                                    self.mons[side][i].disguise = name;
                                    self.mons[side][i].ability_base = Some("Illusion".into());
                                }
                            }
                            None => self.unsupported.push("illusion (unrevealed)".into()),
                        }
                    }
                    if let Some(prev) = self.slots[side][slot] {
                        if prev != i {
                            self.switch_out(side, prev);
                        }
                    }
                    self.slots[side][slot] = Some(i);
                    if self.leads[side][slot].is_none() && self.turn_lines.is_empty() {
                        self.leads[side][slot] = Some(i);
                    }
                    self.switches.push((side, slot, i));
                    let (sp, lvl, g) = parse_details(det);
                    let mut snap = self.mons[side][i].clone();
                    snap.pre_switch = None;
                    let m = &mut self.mons[side][i];
                    m.pre_switch = Some(Box::new(snap));
                    m.used_item = None;
                    m.stint_pp.clear();
                    m.stat_ops.clear();
                    m.seen = true;
                    if self.champions {
                        // Champions' clearVolatile resets Rage Fist's count.
                        m.times_attacked = 0;
                    }
                    m.active = Some(slot);
                    m.switch_line = li;
                    m.turns_active = 0;
                    m.move_actions = 0;
                    m.toxic_ticks = 0;
                    let disguised = m.disguise.is_some();
                    if !disguised {
                        m.level = lvl;
                        if !g.is_empty() {
                            m.gender = g;
                        }
                        if let Some((sid, _)) = species_def(&sp) {
                            if let Some(st) = mega_of(sid) {
                                m.mega = true;
                                m.set_species = data::SPECIES[st.base_species_id as usize].name.to_string();
                            } else if !m.mega {
                                m.set_species = sp.clone();
                            }
                        }
                        m.species = sp;
                    }
                    // Shed Tail hands its doll to the incoming mon.
                    if p.iter().any(|x| *x == "[from] Shed Tail") {
                        m.sub = true;
                    }
                    if let Some(hp) = p.get(4) {
                        self.set_hp(side, i, hp, exact_side);
                    }
                }
                "replace" => {
                    let (Some(id), Some(det)) = (p.get(2), p.get(3)) else { continue };
                    let Some((side, Some(slot), name)) = parse_ident(id) else { continue };
                    let real = self.illusion_target(side, &name, det, li);
                    self.illusion_swap(side, slot, real);
                    self.mons[side][real].disguise = None;
                    if let Some(hp) = p.get(4) {
                        self.set_hp(side, real, hp, exact_side);
                    }
                }
                "detailschange" | "-formechange" => {
                    let (Some(id), Some(det)) = (p.get(2), p.get(3)) else { continue };
                    let Some((side, i)) = self.mon_by_ident(id) else { continue };
                    let (sp, _, _) = parse_details(det);
                    let m = &mut self.mons[side][i];
                    if let Some((sid, _)) = species_def(&sp) {
                        if mega_of(sid).is_some() {
                            m.mega = true;
                        }
                    }
                    m.species = sp;
                }
                "-mega" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    self.mega_used[side] = true;
                    self.mons[side][i].mega = true;
                    if let Some(stone) = p.get(4).filter(|s| !s.is_empty()) {
                        let st = stone.to_string();
                        self.reveal_item(side, i, &st);
                    }
                }
                "-terastallize" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    self.tera_used[side] = true;
                    let m = &mut self.mons[side][i];
                    m.terastallized = true;
                    m.tera = p.get(3).map(|s| s.to_string());
                }
                "move" => {
                    let Some(id) = p.get(2) else { continue };
                    let Some((side, i)) = self.mon_by_ident(id) else { continue };
                    let mv = p.get(3).copied().unwrap_or("");
                    let from = p.iter().skip(4).find(|x| x.starts_with("[from]")).copied();
                    let locked = from.is_some_and(|f| f.contains("lockedmove"));
                    let called = from.is_some() && !locked;
                    self.last_mover = Some((side, i));
                    if called {
                        continue;
                    }
                    let mslug = slug(mv);
                    let target_id = p.get(4).and_then(|t| self.mon_by_ident(t));
                    self.last_move_slot_target = p.get(4).and_then(|t| parse_ident(t)).and_then(|(ts, sl, _)| sl.map(|sl| (ts, sl)));
                    // Pressure: one more PP per Pressure foe targeted.
                    let extra = if locked { 0 } else { self.pressure_extra(side, &mslug, target_id) };
                    let target_enc = target_id.and_then(|(ts, ti)| self.mons[ts][ti].active.map(|sl| (ts * 2 + sl) as u8));
                    let single = move_def(mv).is_some_and(|(_, d)| matches!(d.target, 0 | 2 | 3 | 4 | 10 | 13));
                    let m = &mut self.mons[side][i];
                    m.move_actions += 1;
                    m.charging = None;
                    m.last_target = if single { target_enc } else { None };
                    if mslug == "struggle" {
                        m.last_move = Some("Struggle".into());
                        continue;
                    }
                    let mname = move_name(mv).unwrap_or(mv).to_string();
                    if m.transformed.is_some() {
                        // A copied move: not its own set's.
                        match m.trans_pp.iter_mut().find(|x| slug(&x.0) == mslug) {
                            Some(x) => x.1 += if locked { 0 } else { 1 + extra },
                            None => m.trans_pp.push((mname.clone(), if locked { 0 } else { 1 + extra })),
                        }
                        m.last_move = Some(mname.clone());
                        if !m.stint_moves.iter().any(|x| slug(x) == mslug) {
                            m.stint_moves.push(mname);
                        }
                        continue;
                    }
                    let k = match m.moves.iter().position(|x| slug(x) == mslug) {
                        Some(k) => k,
                        None => {
                            m.moves.push(mname.clone());
                            m.pp_used.push(0);
                            m.move_line.push(li);
                            m.moves.len() - 1
                        }
                    };
                    if !locked {
                        m.pp_used[k] += 1 + extra;
                        match m.stint_pp.iter_mut().find(|x| slug(&x.0) == mslug) {
                            Some(x) => x.1 += 1 + extra,
                            None => m.stint_pp.push((mname.clone(), 1 + extra)),
                        }
                    }
                    m.last_move = Some(mname.clone());
                    if !m.stint_moves.iter().any(|x| slug(x) == mslug) {
                        m.stint_moves.push(mname);
                    }
                    if m.stint_moves.len() > 1 {
                        m.not_choice = true;
                    }
                    let category = move_def(mv).map(|(_, d)| d.category).unwrap_or(2);
                    if category == 2 {
                        m.stint_status_move = true;
                        m.any_status_move = true;
                        m.not_choice = true;
                    }
                    self.action = Some((side, i, category != 2, false, false));
                    let m = &mut self.mons[side][i];
                    if STALL_MOVES.contains(&mslug.as_str()) {
                        m.stall_used = true;
                        m.stall_ok = false;
                    }
                    if mslug == "minimize" {
                        m.vols.retain(|v| v.name != "MINIMIZE");
                        m.vols.push(Vol { name: "MINIMIZE".into(), line: li, arg: String::new() });
                    }
                    if mslug == "glaiverush" {
                        m.vols.retain(|v| v.name != "GLAIVERUSH");
                        m.vols.push(Vol { name: "GLAIVERUSH".into(), line: li, arg: String::new() });
                    }
                }
                "cant" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let reason = p.get(3).copied().unwrap_or("");
                    let m = &mut self.mons[side][i];
                    m.move_actions += 1;
                    match reason {
                        "slp" => m.sleep_cants += 1,
                        "recharge" => m.must_recharge = false,
                        _ => {}
                    }
                    if let Some(mv) = p.get(4).filter(|x| !x.is_empty() && !x.starts_with('[')) {
                        let mslug = slug(mv);
                        if move_def(mv).is_some() && !m.moves.iter().any(|x| slug(x) == mslug) {
                            m.moves.push(move_name(mv).unwrap_or(mv).to_string());
                            m.pp_used.push(0);
                            m.move_line.push(li);
                        }
                    }
                }
                "faint" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let m = &mut self.mons[side][i];
                    m.fainted = true;
                    m.hp = Some(HpObs::Exact(0, 100));
                    m.vols.clear();
                    m.boosts = [0; 7];
                    m.status.clear();
                }
                "-damage" | "-heal" | "-sethp" => {
                    let Some(id) = p.get(2) else { continue };
                    let Some((side, i)) = self.mon_by_ident(id) else { continue };
                    if let Some(hp) = p.get(3) {
                        self.set_hp(side, i, hp, exact_side);
                    }
                    let from = p.iter().skip(4).find(|x| x.starts_with("[from]")).copied();
                    if tag == "-damage" && from.is_none() {
                        if self.action.is_none_or(|a| (a.0, a.1) != (side, i)) {
                            self.mons[side][i].times_attacked += 1;
                        }
                        if let Some(a) = self.action.as_mut() {
                            if (a.0, a.1) != (side, i) {
                                a.3 = true;
                            }
                        }
                    }
                    if from.is_some_and(|f| f.contains("item: Life Orb")) {
                        if let Some(a) = self.action.as_mut() {
                            if (a.0, a.1) == (side, i) {
                                a.4 = true;
                            }
                        }
                    }
                    if from.is_some_and(|f| f.ends_with("psn")) && self.mons[side][i].status == "tox" {
                        self.mons[side][i].toxic_ticks += 1;
                    }
                    self.from_reveals(&p, Some((side, i)), tag);
                }
                "-status" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let st = p.get(3).copied().unwrap_or("").to_string();
                    let m = &mut self.mons[side][i];
                    m.status = st;
                    m.status_line = li;
                    m.toxic_ticks = 0;
                    m.sleep_cants = 0;
                    self.from_reveals(&p, Some((side, i)), tag);
                }
                "-curestatus" => {
                    if let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        self.mons[side][i].status.clear();
                        self.from_reveals(&p, Some((side, i)), tag);
                    }
                }
                "-cureteam" => {
                    if let Some((side, _)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        for m in self.mons[side].iter_mut() {
                            m.status.clear();
                        }
                    }
                }
                "-boost" | "-unboost" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let k = stat_index(p.get(3).copied().unwrap_or(""));
                    let n: i8 = p.get(4).and_then(|x| x.parse().ok()).unwrap_or(0);
                    if let Some(k) = k {
                        let b = &mut self.mons[side][i].boosts[k];
                        *b = (*b + if tag == "-boost" { n } else { -n }).clamp(-6, 6);
                    }
                    self.from_reveals(&p, Some((side, i)), tag);
                }
                "-setboost" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    if let (Some(k), Some(n)) = (stat_index(p.get(3).copied().unwrap_or("")), p.get(4).and_then(|x| x.parse::<i8>().ok())) {
                        self.mons[side][i].boosts[k] = n.clamp(-6, 6);
                    }
                    self.from_reveals(&p, Some((side, i)), tag);
                }
                "-clearboost" => {
                    if let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        self.mons[side][i].boosts = [0; 7];
                    }
                }
                "-clearallboost" => {
                    for side in 0..2 {
                        for m in self.mons[side].iter_mut().filter(|m| m.active.is_some()) {
                            m.boosts = [0; 7];
                        }
                    }
                }
                "-clearnegativeboost" | "-clearpositiveboost" => {
                    if let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        for b in self.mons[side][i].boosts.iter_mut() {
                            if (tag == "-clearnegativeboost" && *b < 0) || (tag == "-clearpositiveboost" && *b > 0) {
                                *b = 0;
                            }
                        }
                    }
                }
                "-invertboost" => {
                    if let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        for b in self.mons[side][i].boosts.iter_mut() {
                            *b = -*b;
                        }
                    }
                }
                "-swapboost" => {
                    let (Some(a), Some(b)) = (p.get(2).and_then(|id| self.mon_by_ident(id)), p.get(3).and_then(|id| self.mon_by_ident(id))) else { continue };
                    let stats: Vec<usize> = match p.get(4).filter(|s| !s.starts_with('[')) {
                        Some(s) => s.split(',').filter_map(|x| stat_index(x.trim())).collect(),
                        None => (0..7).collect(),
                    };
                    for k in stats {
                        let x = self.mons[a.0][a.1].boosts[k];
                        self.mons[a.0][a.1].boosts[k] = self.mons[b.0][b.1].boosts[k];
                        self.mons[b.0][b.1].boosts[k] = x;
                    }
                }
                "-copyboost" => {
                    let (Some(a), Some(b)) = (p.get(2).and_then(|id| self.mon_by_ident(id)), p.get(3).and_then(|id| self.mon_by_ident(id))) else { continue };
                    self.mons[a.0][a.1].boosts = self.mons[b.0][b.1].boosts;
                }
                "-weather" => {
                    let w = p.get(2).copied().unwrap_or("");
                    if p.iter().any(|x| *x == "[upkeep]") {
                        continue;
                    }
                    if w == "none" || w.is_empty() {
                        self.weather = None;
                    } else {
                        let of = p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id));
                        let from_ability = p.iter().any(|x| x.starts_with("[from] ability:"));
                        let setter = if from_ability { of } else { self.last_mover };
                        self.weather = Some(FieldFx { name: w.to_string(), line: li, setter });
                        self.from_reveals(&p, None, tag);
                    }
                }
                "-fieldstart" => {
                    let fx = p.get(2).map(|s| s.trim_start_matches("move: ").to_string()).unwrap_or_default();
                    let of = p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id));
                    let from_ability = p.iter().any(|x| x.starts_with("[from] ability:"));
                    let setter = if from_ability { of } else { self.last_mover.or(of) };
                    let f = FieldFx { name: fx.clone(), line: li, setter };
                    if fx.ends_with("Terrain") {
                        self.terrain = Some(f);
                    } else {
                        self.pseudo.retain(|x| x.name != fx);
                        self.pseudo.push(f);
                    }
                    self.from_reveals(&p, None, tag);
                }
                "-fieldend" => {
                    let fx = p.get(2).map(|s| s.trim_start_matches("move: ").to_string()).unwrap_or_default();
                    if fx.ends_with("Terrain") {
                        self.terrain = None;
                    } else {
                        self.pseudo.retain(|x| x.name != fx);
                    }
                }
                "-sidestart" | "-sideend" => {
                    let side = match p.get(2).and_then(|s| s.get(..2)) {
                        Some("p1") => 0,
                        Some("p2") => 1,
                        _ => continue,
                    };
                    let fx = p.get(3).map(|s| s.trim_start_matches("move: ").to_string()).unwrap_or_default();
                    if tag == "-sidestart" {
                        let setter = self.last_mover;
                        if !matches!(fx.as_str(), "Spikes" | "Toxic Spikes") {
                            self.side_fx[side].retain(|x| x.name != fx);
                        }
                        self.side_fx[side].push(FieldFx { name: fx, line: li, setter });
                    } else {
                        self.side_fx[side].retain(|x| x.name != fx);
                    }
                }
                "-start" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let label = p.get(3).copied().unwrap_or("");
                    let arg = p.get(4).copied().unwrap_or("").to_string();
                    let low = label.to_ascii_lowercase();
                    if let Some(n) = low.strip_prefix("perish") {
                        let m = &mut self.mons[side][i];
                        m.vols.retain(|v| !v.name.starts_with("PERISH"));
                        if n != "0" {
                            m.vols.push(Vol { name: format!("PERISH{n}"), line: li, arg });
                        }
                        continue;
                    }
                    if let Some(n) = low.strip_prefix("stockpile") {
                        let m = &mut self.mons[side][i];
                        m.vols.retain(|v| v.name != "STOCKPILE");
                        m.vols.push(Vol { name: "STOCKPILE".into(), line: li, arg: n.to_string() });
                        continue;
                    }
                    if let Some(n) = low.strip_prefix("fallen") {
                        self.mons[side][i].fallen = n.parse().ok();
                        continue;
                    }
                    if let Some(st) = low.strip_prefix("protosynthesis").or_else(|| low.strip_prefix("quarkdrive")) {
                        let base = if low.starts_with("proto") { "PROTOSYNTHESIS" } else { "QUARKDRIVE" };
                        let m = &mut self.mons[side][i];
                        m.vols.retain(|v| !v.name.starts_with(base));
                        m.vols.push(Vol { name: format!("{base}{}", st.to_ascii_uppercase()), line: li, arg: String::new() });
                        continue;
                    }
                    if label == "typechange" {
                        self.mons[side][i].type_change = Some(arg.clone());
                        self.from_reveals(&p, Some((side, i)), tag);
                        continue;
                    }
                    if label == "Substitute" {
                        self.mons[side][i].sub = true;
                        continue;
                    }
                    if label == "Power Trick" {
                        self.mons[side][i].stat_ops.push((li, "powertrick".into(), None));
                        continue;
                    }
                    let name = match label {
                        "move: Taunt" => "TAUNT",
                        "Encore" => "ENCORE",
                        "Disable" => "DISABLE",
                        "move: Yawn" => "YAWN",
                        "Throat Chop" => "THROATCHOP",
                        "move: Heal Block" => "HEALBLOCK",
                        "Magnet Rise" => "MAGNETRISE",
                        "confusion" => "CONFUSION",
                        "move: Leech Seed" => "LEECHSEED",
                        "Aqua Ring" | "move: Aqua Ring" => "AQUARING",
                        "move: Ingrain" => "INGRAIN",
                        "move: Focus Energy" => "FOCUSENERGY",
                        "Charge" => "CHARGE",
                        "Salt Cure" => "SALTCURE",
                        "Smack Down" => "SMACKDOWN",
                        "Tar Shot" => "TARSHOT",
                        "move: No Retreat" => "NORETREAT",
                        "ability: Flash Fire" => "FLASHFIRE",
                        "move: Imprison" => "IMPRISON",
                        "Attract" => "ATTRACT",
                        "Curse" => "CURSE",
                        "Minimize" => "MINIMIZE",
                        _ => {
                            // Not a board volatile: a turn-scoped effect, or one
                            // the reconstruction does not carry.
                            if matches!(label, "move: Future Sight" | "move: Doom Desire") {
                                // A slot condition on the target's slot (the line names the user).
                                if let Some((ts, tslot)) = self.last_move_slot_target {
                                    self.futures.retain(|f| (f.0, f.1) != (ts, tslot));
                                    self.futures.push((ts, tslot, (side, i), label.trim_start_matches("move: ").to_string(), li));
                                }
                                continue;
                            }
                            if label == "typeadd" {
                                // The engine keeps two types (battle.rs addType).
                                continue;
                            }
                            if matches!(label, "move: Octolock" | "move: Torment" | "Torment") {
                                let name = if label.ends_with("Octolock") { "?OCTOLOCK" } else { "?TORMENT" };
                                let src = p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id)).map(|(a, b)| format!("{a},{b}")).unwrap_or_default();
                                let m = &mut self.mons[side][i];
                                m.vols.retain(|v| v.name != name);
                                m.vols.push(Vol { name: name.into(), line: li, arg: src });
                                continue;
                            }
                            if label == "move: Dragon Cheer" {
                                let m = &mut self.mons[side][i];
                                m.vols.retain(|v| v.name != "?DRAGONCHEER");
                                m.vols.push(Vol { name: "?DRAGONCHEER".into(), line: li, arg });
                                continue;
                            }
                            if !label.is_empty() {
                                self.mons[side][i].unsupported.push(format!("volatile {label}"));
                            }
                            self.mons[side][i].vols.push(Vol { name: format!("?{label}"), line: li, arg });
                            continue;
                        }
                    };
                    let m = &mut self.mons[side][i];
                    m.vols.retain(|v| v.name != name);
                    m.vols.push(Vol { name: name.into(), line: li, arg });
                    if name == "FLASHFIRE" {
                        self.reveal_ability(side, i, "Flash Fire");
                    }
                    self.from_reveals(&p, Some((side, i)), tag);
                }
                "-end" => {
                    let label = p.get(3).copied().unwrap_or("");
                    if matches!(label, "move: Future Sight" | "move: Doom Desire" | "Future Sight" | "Doom Desire") {
                        if let Some((ts, Some(tslot), _)) = p.get(2).and_then(|id| parse_ident(id)) {
                            self.futures.retain(|f| (f.0, f.1) != (ts, tslot));
                        }
                        continue;
                    }
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let low = label.to_ascii_lowercase();
                    let m = &mut self.mons[side][i];
                    if label == "Substitute" {
                        m.sub = false;
                        if self.action.is_some_and(|a| (a.0, a.1) != (side, i)) {
                            self.mons[side][i].times_attacked += 1;
                        }
                        continue;
                    }
                    if low.starts_with("protosynthesis") || low.starts_with("quarkdrive") {
                        m.vols.retain(|v| !v.name.starts_with("PROTOSYNTHESIS") && !v.name.starts_with("QUARKDRIVE"));
                        continue;
                    }
                    if low.starts_with("stockpile") {
                        m.vols.retain(|v| v.name != "STOCKPILE");
                        continue;
                    }
                    let name = match label {
                        "move: Taunt" => "TAUNT",
                        "Encore" => "ENCORE",
                        "Disable" => "DISABLE",
                        "move: Yawn" => "YAWN",
                        "Throat Chop" => "THROATCHOP",
                        "move: Heal Block" => "HEALBLOCK",
                        "Magnet Rise" => "MAGNETRISE",
                        "confusion" => "CONFUSION",
                        "move: Leech Seed" => "LEECHSEED",
                        "Charge" => "CHARGE",
                        "Salt Cure" => "SALTCURE",
                        "Smack Down" => "SMACKDOWN",
                        "Tar Shot" => "TARSHOT",
                        "Attract" => "ATTRACT",
                        "move: Imprison" => "IMPRISON",
                        "ability: Flash Fire" => "FLASHFIRE",
                        "Octolock" | "move: Octolock" => {
                            m.vols.retain(|v| v.name != "?OCTOLOCK");
                            continue;
                        }
                        "Torment" | "move: Torment" => {
                            m.vols.retain(|v| v.name != "?TORMENT");
                            continue;
                        }
                        other => {
                            let q = format!("?{other}");
                            m.vols.retain(|v| v.name != q);
                            m.unsupported.retain(|u| *u != format!("volatile {other}"));
                            continue;
                        }
                    };
                    m.vols.retain(|v| v.name != name);
                }
                "-item" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let it = p.get(3).copied().unwrap_or("").to_string();
                    let from = p.iter().find(|x| x.starts_with("[from]")).copied().unwrap_or("");
                    if from.contains("move: Trick") || from.contains("move: Switcheroo") || from.contains("Thief") || from.contains("Covet") || from.contains("Pickpocket") || from.contains("Magician") || from.contains("Recycle") || from.contains("Harvest") || from.contains("Pickup") || from.contains("Bestow") {
                        let m = &mut self.mons[side][i];
                        m.item_changed = true;
                        m.item_now = Some(Some(it));
                    } else {
                        self.reveal_item(side, i, &it);
                    }
                    if from.contains("ability: ") {
                        // Frisk: the [of] mon's ability.
                        if let (Some(ab), Some((s2, i2))) = (from.split("ability: ").nth(1), p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id))) {
                            let ab = ab.to_string();
                            self.reveal_ability(s2, i2, &ab);
                        }
                    }
                }
                "-enditem" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let it = p.get(3).copied().unwrap_or("").to_string();
                    let from = p.iter().find(|x| x.starts_with("[from]")).copied().unwrap_or("");
                    {
                        let m = &mut self.mons[side][i];
                        if m.item_orig.is_none() && !m.item_changed && !m.item_lost {
                            m.item_orig = Some(it.clone());
                        }
                        m.item_now = Some(None);
                        m.item_lost = true;
                        m.item_lost_line = Some(li);
                        let taken = ["Knock Off", "stealeat", "Thief", "Covet", "move: Trick", "Switcheroo", "Bestow", "Corrosive Gas", "Incinerate"].iter().any(|x| from.contains(x));
                        m.item_knocked = taken;
                        if !taken {
                            m.used_item = Some(it.clone());
                        }
                        if from.contains("move: Trick") || from.contains("Switcheroo") {
                            m.item_changed = true;
                        }
                    }
                    if from.contains("ability: ") {
                        let of = p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id));
                        if let (Some(ab), Some((s2, i2))) = (from.split("ability: ").nth(1), of) {
                            let ab = ab.to_string();
                            self.reveal_ability(s2, i2, &ab);
                        }
                    }
                }
                "-ability" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let ab = p.get(3).copied().unwrap_or("").to_string();
                    let from = p.iter().find(|x| x.starts_with("[from]")).copied().unwrap_or("");
                    if from.is_empty() {
                        self.reveal_ability(side, i, &ab);
                    } else if let Some(src) = from.strip_prefix("[from] ability: ") {
                        if src == "Trace" || src == "Receiver" || src == "Power of Alchemy" {
                            self.reveal_ability(side, i, src);
                            self.mons[side][i].ability_now = Some(ab);
                        } else {
                            // Mummy / Lingering Aroma / Wandering Spirit: the
                            // subject's ability became `ab`; `src` is the [of]'s.
                            self.mons[side][i].ability_now = Some(ab);
                            if let Some((s2, i2)) = p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id)) {
                                let src = src.to_string();
                                self.reveal_ability(s2, i2, &src);
                            }
                        }
                    } else {
                        // Skill Swap, Role Play, Entrainment, Worry Seed, ...
                        self.mons[side][i].ability_now = Some(ab);
                    }
                }
                "-activate" | "-fieldactivate" => {
                    let subject = p.get(2).and_then(|id| self.mon_by_ident(id));
                    if let (Some((side, i)), Some(lbl)) = (subject, p.get(3)) {
                        if matches!(*lbl, "move: Speed Swap" | "move: Power Split" | "move: Guard Split" | "move: Power Swap" | "move: Guard Swap" | "move: Heart Swap") {
                            let of = p.iter().find_map(|x| x.strip_prefix("[of] ")).and_then(|id| self.mon_by_ident(id));
                            let kind = slug(lbl.trim_start_matches("move: "));
                            if matches!(kind.as_str(), "speedswap" | "powersplit" | "guardsplit") {
                                self.mons[side][i].stat_ops.push((li, kind.clone(), of));
                                if let Some((s2, i2)) = of {
                                    self.mons[s2][i2].stat_ops.push((li, kind, Some((side, i))));
                                }
                            }
                        }
                        if *lbl == "move: Substitute" && p.iter().any(|x| *x == "[damage]") && self.action.is_some_and(|a| (a.0, a.1) != (side, i)) {
                            self.mons[side][i].times_attacked += 1;
                        }
                        if let Some(ab) = lbl.strip_prefix("ability: ") {
                            let ab = ab.to_string();
                            self.reveal_ability(side, i, &ab);
                        } else if let Some(it) = lbl.strip_prefix("item: ") {
                            let it = it.to_string();
                            self.reveal_item(side, i, &it);
                        }
                    }
                    self.from_reveals(&p, subject, tag);
                }
                "-immune" | "-block" | "-fail" | "-clearallboost_" | "-supereffective" | "-resisted" | "-crit" | "-miss" | "-hitcount" | "-notarget" | "-nothing" | "-message" | "-hint" | "-anim" | "-center" | "-combine" | "-waiting" | "-zbroken" => {
                    let subject = p.get(2).and_then(|id| self.mon_by_ident(id));
                    if tag == "-anim" {
                        if let Some((s2, i2)) = subject {
                            self.mons[s2][i2].charging = None;
                        }
                    }
                    self.from_reveals(&p, subject, tag);
                }
                "-singleturn" => {
                    if let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        let lbl = slug(p.get(3).copied().unwrap_or("").trim_start_matches("move: "));
                        if STALL_MOVES.contains(&lbl.as_str()) && self.mons[side][i].stall_used {
                            self.mons[side][i].stall_ok = true;
                        }
                    }
                }
                "-prepare" => {
                    if let Some((s2, i2)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        self.mons[s2][i2].charging = p.get(3).map(|x| x.to_string());
                    }
                }
                "-mustrecharge" => {
                    if let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) {
                        self.mons[side][i].must_recharge = true;
                    }
                }
                "-transform" => {
                    let (Some((side, i)), Some(t)) = (p.get(2).and_then(|id| self.mon_by_ident(id)), p.get(3).and_then(|id| self.mon_by_ident(id))) else { continue };
                    let boosts = self.mons[t.0][t.1].boosts;
                    let m = &mut self.mons[side][i];
                    m.transformed = Some(t);
                    m.trans_pp.clear();
                    m.boosts = boosts;
                    self.from_reveals(&p, Some((side, i)), tag);
                }
                "swap" => {
                    let Some((side, i)) = p.get(2).and_then(|id| self.mon_by_ident(id)) else { continue };
                    let Some(to) = p.get(3).and_then(|x| x.parse::<usize>().ok()) else { continue };
                    let Some(from) = self.mons[side][i].active else { continue };
                    if to > 1 || from == to {
                        continue;
                    }
                    let other = self.slots[side][to];
                    self.slots[side][to] = Some(i);
                    self.slots[side][from] = other;
                    self.mons[side][i].active = Some(to);
                    if let Some(o) = other {
                        self.mons[side][o].active = Some(from);
                    }
                    self.ally_swaps.push(side);
                    self.switches.push((side, usize::MAX, 0));
                }
                "upkeep" => {
                    for side in 0..2 {
                        for m in self.mons[side].iter_mut().filter(|m| m.active.is_some()) {
                            m.turns_active += 1;
                        }
                    }
                    // End of turn: a stall move used this turn keeps its
                    // counter (0 when it failed); otherwise the streak ends.
                    for side in 0..2 {
                        for m in self.mons[side].iter_mut() {
                            if m.stall_used {
                                m.stall_count = if m.stall_ok { m.stall_count.saturating_add(1).min(6) } else { 0 };
                                m.stall_used_last = true;
                            } else {
                                m.stall_count = 0;
                                m.stall_used_last = false;
                            }
                            m.stall_used = false;
                            m.stall_ok = false;
                        }
                    }
                }
                "turn" => {
                    // Glaive Rush's weakness lasts until the end of the next turn.
                    let prev = self.turn_lines.last().copied().unwrap_or(0);
                    for side in 0..2 {
                        for m in self.mons[side].iter_mut() {
                            m.vols.retain(|v| !(v.name == "GLAIVERUSH" && v.line < prev));
                        }
                    }
                    self.turn_lines.push(li);
                }
                _ => {}
            }
        }
    }

    /// Record the decoder's stints on the holder (provenance) and on the
    /// mons it disguised as (brought).
    fn apply_illusion(&mut self, stints: &[illusion::Disguised]) {
        for d in stints {
            let side = d.side;
            let zoro = |m: &Mon| ["zoroark", "zorua"].iter().any(|x| slug(&m.preview).starts_with(x));
            if let Some(h) = (0..self.mons[side].len()).find(|&i| zoro(&self.mons[side][i]) && self.mons[side][i].name.as_deref() == Some(d.holder.as_str())) {
                let m = &mut self.mons[side][h];
                m.illusion.push((d.start, d.end, d.src, d.evidence == illusion::Evidence::Replace));
                if m.ability_base.is_none() {
                    m.ability_base = Some("Illusion".into());
                }
            }
            if let Some(t) = self.mons[side].iter_mut().find(|m| !zoro(m) && m.name.as_deref() == Some(d.disguise.as_str())) {
                t.disguise_target = true;
            } else if let Some(t) = self.mons[side].iter_mut().find(|m| m.name.is_none() && same_family(&m.preview, &d.disguise_species)) {
                t.disguise_target = true;
            }
        }
    }

    /// The real mon behind an Illusion (`|replace|` / a request).
    fn illusion_target(&mut self, side: usize, name: &str, details: &str, line: usize) -> usize {
        let sp = parse_details(details).0;
        if let Some(i) = (0..self.mons[side].len()).find(|&i| self.mons[side][i].name.as_deref() == Some(name) && same_family(&self.mons[side][i].preview, &sp)) {
            return i;
        }
        let i = self.mons[side]
            .iter()
            .position(|m| m.name.is_none() && slug(&m.preview) == slug(&sp))
            .or_else(|| self.mons[side].iter().position(|m| m.name.is_none() && same_family(&m.preview, &sp)))
            .unwrap_or_else(|| {
                self.mons[side].push(Mon { preview: sp.clone(), species: sp.clone(), set_species: sp.clone(), level: 50, ..Default::default() });
                self.mons[side].len() - 1
            });
        let m = &mut self.mons[side][i];
        m.name = Some(name.to_string());
        m.species = sp.clone();
        m.set_species = sp;
        m.first_seen.get_or_insert(line);
        i
    }

    /// The mon in `slot` was `real` in disguise: it takes over the stint
    /// (HP, boosts, volatiles, the moves and PP used since it came in), and
    /// the disguise's mon goes back to its state before that switch-in.
    fn illusion_swap(&mut self, side: usize, slot: usize, real: usize) {
        let Some(fake) = self.slots[side][slot] else { return };
        if fake == real {
            return;
        }
        let d = self.mons[side][fake].clone();
        let mut r = std::mem::take(&mut self.mons[side][real]);
        for (mv, used) in &d.stint_pp {
            match r.moves.iter().position(|x| slug(x) == slug(mv)) {
                Some(k) => r.pp_used[k] += used,
                None => {
                    r.moves.push(mv.clone());
                    r.pp_used.push(*used);
                    r.move_line.push(d.switch_line);
                }
            }
        }
        r.seen = true;
        r.hp = d.hp;
        r.fainted = d.fainted;
        r.status = d.status.clone();
        r.boosts = d.boosts;
        r.vols = d.vols.clone();
        r.sub = d.sub;
        r.active = d.active;
        r.switch_line = d.switch_line;
        r.turns_active = d.turns_active;
        r.move_actions = d.move_actions;
        r.stint_moves = d.stint_moves.clone();
        r.stint_pp = d.stint_pp.clone();
        r.last_move = d.last_move.clone();
        r.last_target = d.last_target;
        r.stall_count = d.stall_count;
        r.stall_used = d.stall_used;
        r.stall_ok = d.stall_ok;
        r.stall_used_last = d.stall_used_last;
        r.charging = d.charging.clone();
        r.must_recharge = d.must_recharge;
        r.times_attacked = d.times_attacked;
        r.ability_base = Some("Illusion".into());
        r.disguise = d.name.clone();
        r.first_seen = Some(r.first_seen.unwrap_or(d.switch_line).min(d.switch_line));
        self.mons[side][real] = r;
        // The disguise: back to before that switch-in.
        let mut back = d.pre_switch.clone().map(|b| *b).unwrap_or_default();
        back.name = d.name.clone();
        back.pre_switch = None;
        self.mons[side][fake] = back;
        self.slots[side][slot] = Some(real);
        for sw in self.switches.iter_mut().rev() {
            if sw.0 == side && sw.1 == slot && sw.2 == fake {
                sw.2 = real;
                break;
            }
        }
        for l in self.leads[side].iter_mut() {
            if *l == Some(fake) && self.mons[side][fake].first_seen.is_none() {
                *l = Some(real);
            }
        }
    }

    fn end_action(&mut self) {
        if let Some((s, i, dmg, hit, lo)) = self.action.take() {
            if dmg && hit && !lo {
                self.mons[s][i].hit_without_lo = true;
            }
        }
    }

    fn pressure_extra(&self, side: usize, mslug: &str, target: Option<(usize, usize)>) -> u32 {
        let foe = 1 - side;
        let pressure = |i: usize| {
            let m = &self.mons[foe][i];
            !m.fainted && m.active.is_some() && m.ability_now.as_deref().or(m.ability_base.as_deref()).is_some_and(|a| slug(a) == "pressure")
        };
        let Some((_, d)) = move_def(mslug) else { return 0 };
        match d.target {
            0 | 4 | 10 | 13 => match target {
                Some((s, i)) if s == foe && pressure(i) => 1,
                _ => 0,
            },
            5 | 6 | 12 => (0..self.mons[foe].len()).filter(|&i| pressure(i)).count() as u32,
            _ => 0,
        }
    }
}

fn stat_index(s: &str) -> Option<usize> {
    Some(match s {
        "atk" => 0,
        "def" => 1,
        "spa" => 2,
        "spd" => 3,
        "spe" => 4,
        "accuracy" => 5,
        "evasion" => 6,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percentage_maps_to_the_hp_values_ps_shows_that_way() {
        // Champions: floor, at least 1.
        assert_eq!(pct_interval(50, Some('g'), 200, true), (101, 101));
        assert_eq!(pct_interval(50, Some('y'), 200, true), (100, 100));
        assert_eq!(pct_interval(1, None, 207, true), (1, 4));
        assert_eq!(pct_interval(100, None, 207, true), (207, 207));
        // Gen 9: ceil, 99 for a scratch.
        assert_eq!(pct_interval(99, None, 207, false), (203, 206));
        for max in [100u16, 157, 207, 351] {
            for hp in 1..=max {
                let t = pct_text(hp, max, true);
                let (n, _, band, _, _) = parse_hp(&t).unwrap();
                let (lo, hi) = pct_interval(n, band, max, true);
                assert!(lo <= hp && hp <= hi, "{hp}/{max} shown {t}: {lo}..{hi}");
            }
        }
    }

    #[test]
    fn hp_text_parses() {
        assert_eq!(parse_hp("82/100"), Some((82, 100, None, None, false)));
        assert_eq!(parse_hp("20/100y par"), Some((20, 100, Some('y'), Some("par".into()), false)));
        assert_eq!(parse_hp("0 fnt").map(|x| x.4), Some(true));
        assert_eq!(parse_hp("142/172 slp").map(|x| (x.0, x.1)), Some((142, 172)));
    }

    #[test]
    fn idents_and_details_parse() {
        assert_eq!(parse_ident("p2b: Mr. Mime"), Some((1, Some(1), "Mr. Mime".to_string())));
        assert_eq!(parse_ident("p1: Garchomp").map(|x| x.1), Some(None));
        assert_eq!(parse_details("Salamence-Mega, L50, M, shiny"), ("Salamence-Mega".to_string(), 50, "M".to_string()));
    }
}
