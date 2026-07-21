//! pyo3 bindings.
//!
//! Phase 2 PR-1: exposes real Battle with team loading + switches.
//! Damage/abilities/items still stubs.
//!
//!   import vgc_engine
//!   b = vgc_engine.Battle.from_teams(p1_json, p2_json, format="doubles")
//!   choices = b.legal_choices(side=0, slot=0)
//!   b.step_pass()           # both sides pass — turn ticks
//!
//!   # Fast damage calc (mirrors the `vgc calc` CLI):
//!   r = vgc_engine.calc("chomp", "lando", "eq")
//!   r["min"], r["max"], r["multi_hit"]["label"]

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};
use vgc_engine_core as core;

/// Stable lowercase slug for a weather state.
fn weather_slug(w: core::Weather) -> &'static str {
    match w {
        core::Weather::None => "none",
        core::Weather::Rain => "rain",
        core::Weather::Sun => "sun",
        core::Weather::Sand => "sand",
        core::Weather::Snow => "snow",
    }
}

/// Stable lowercase slug for a terrain state.
fn terrain_slug(t: core::Terrain) -> &'static str {
    match t {
        core::Terrain::None => "none",
        core::Terrain::Electric => "electric",
        core::Terrain::Grassy => "grassy",
        core::Terrain::Psychic => "psychic",
        core::Terrain::Misty => "misty",
    }
}

/// Stable lowercase slug for a persistent status condition.
fn status_slug(s: core::Status) -> &'static str {
    match s {
        core::Status::None => "none",
        core::Status::Sleep => "sleep",
        core::Status::Freeze => "freeze",
        core::Status::Paralysis => "paralysis",
        core::Status::Burn => "burn",
        core::Status::Poison => "poison",
        core::Status::Toxic => "toxic",
    }
}

/// Lowercase type slug for a 0..=17 type code (`data::TYPE_NAMES`).
fn type_slug(code: u8) -> String {
    core::data::TYPE_NAMES
        .get(code as usize)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build the per-mon observation dict for an active Pokémon.
fn observe_active_mon<'py>(
    py: Python<'py>,
    m: &core::Pokemon,
    slot: usize,
    side_tera_used: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("slot", slot)?;
    d.set_item("species", m.species().slug)?;
    d.set_item("fainted", m.fainted)?;
    d.set_item("hp", m.current_hp)?;
    d.set_item("max_hp", m.stats.hp)?;
    d.set_item("status", status_slug(m.status))?;
    // Single counter for the active status: Toxic stage or sleep turns; 0 otherwise.
    let status_counter: u8 = match m.status {
        core::Status::Toxic => m.toxic_counter(),
        core::Status::Sleep => m.sleep_turns(),
        _ => 0,
    };
    d.set_item("status_counter", status_counter)?;

    // boosts[7] order in core: [atk, def, spa, spd, spe, acc, eva].
    let boosts = PyDict::new(py);
    boosts.set_item("atk", m.boosts[0])?;
    boosts.set_item("def", m.boosts[1])?;
    boosts.set_item("spa", m.boosts[2])?;
    boosts.set_item("spd", m.boosts[3])?;
    boosts.set_item("spe", m.boosts[4])?;
    boosts.set_item("acc", m.boosts[5])?;
    boosts.set_item("eva", m.boosts[6])?;
    d.set_item("boosts", boosts)?;

    // Live (post-Tera / type_override) types.
    let (types, n_types) = m.effective_types();
    let types_list = PyList::empty(py);
    for &code in types.iter().take(n_types as usize) {
        types_list.append(type_slug(code))?;
    }
    d.set_item("types", types_list)?;

    // Effective ability ("" if suppressed) and item (None if suppressed / itemless).
    d.set_item("ability", m.effective_ability_slug())?;
    let item_id = m.effective_item_id();
    let item: Option<&'static str> = if item_id == u16::MAX {
        None
    } else {
        core::data::ITEMS.get(item_id as usize).map(|i| i.slug)
    };
    d.set_item("item", item)?;
    d.set_item("tera_used", side_tera_used)?;
    d.set_item("is_terastallized", m.terastallized)?;

    // Move slots: skip empty (u16::MAX) slots. max_pp is the PP-maxed cap the
    // engine builds with (boosted_max_pp), matching the starting PP.
    let moves_list = PyList::empty(py);
    for i in 0..4 {
        let mid = m.moves[i];
        if mid == u16::MAX {
            continue;
        }
        let md = PyDict::new(py);
        md.set_item("id", core::data::MOVES[mid as usize].slug)?;
        md.set_item("pp", m.pp[i])?;
        md.set_item("max_pp", core::boosted_max_pp(mid))?;
        moves_list.append(md)?;
    }
    d.set_item("moves", moves_list)?;

    // Volatiles the engine actually models (turn-counters in turns, flags in bool).
    let vol = PyDict::new(py);
    vol.set_item("substitute_hp", m.substitute_hp())?;
    vol.set_item("taunt", m.taunt_turns())?;
    vol.set_item("encore", m.encore_turns())?;
    vol.set_item("disable", m.disable_turns())?;
    vol.set_item("confusion", m.confusion_turns())?;
    vol.set_item("throat_chop", m.throat_chop_turns())?;
    vol.set_item("heal_block", m.heal_block_turns())?;
    vol.set_item("perish", m.perish_turns())?;
    vol.set_item("leech_seed", m.has_leech_seed())?;
    vol.set_item("salt_cure", m.has_salt_cure())?;
    vol.set_item("protect", m.is_protected_this_turn())?;
    d.set_item("volatiles", vol)?;

    Ok(d)
}

/// Build the lightweight bench-mon dict (enough for switch decisions).
fn observe_bench_mon<'py>(
    py: Python<'py>,
    team_index: usize,
    m: &core::Pokemon,
) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("team_index", team_index)?;
    d.set_item("species", m.species().slug)?;
    d.set_item("hp", m.current_hp)?;
    d.set_item("max_hp", m.stats.hp)?;
    d.set_item("status", status_slug(m.status))?;
    d.set_item("fainted", m.fainted)?;
    Ok(d)
}

/// Build the full per-side observation dict.
fn observe_side<'py>(
    py: Python<'py>,
    battle: &core::Battle,
    s: core::SideRef,
) -> PyResult<Bound<'py, PyDict>> {
    let side = battle.side(s);
    let c = &side.conditions;
    let n_active = battle.format().active_count();

    let d = PyDict::new(py);
    d.set_item("side", s as u8)?;

    let conditions = PyDict::new(py);
    conditions.set_item("reflect", c.reflect_turns)?;
    conditions.set_item("light_screen", c.light_screen_turns)?;
    conditions.set_item("aurora_veil", c.aurora_veil_turns)?;
    conditions.set_item("tailwind", c.tailwind_turns)?;
    conditions.set_item("safeguard", c.safeguard_turns)?;
    conditions.set_item("mist", c.mist_turns)?;
    // Stealth Rock / Sticky Web are binary; Spikes / Toxic Spikes are layer counts.
    conditions.set_item("stealth_rock", c.stealth_rock)?;
    conditions.set_item("spikes", c.spikes_layers)?;
    conditions.set_item("toxic_spikes", c.toxic_spikes_layers)?;
    conditions.set_item("sticky_web", c.sticky_web)?;
    conditions.set_item("wish_pending", battle.has_wish_pending(s))?;
    conditions.set_item("future_pending", battle.has_future_pending(s))?;
    d.set_item("conditions", conditions)?;

    // Active slots — present occupants only.
    let active = PyList::empty(py);
    let mut active_idxs: Vec<u8> = Vec::new();
    for slot in 0..n_active {
        if let Some(m) = side.active_mon(slot) {
            active.append(observe_active_mon(py, m, slot, c.tera_used)?)?;
        }
        let idx = side.active[slot];
        if idx != u8::MAX {
            active_idxs.push(idx);
        }
    }
    d.set_item("active", active)?;

    // Bench — every team member not currently in an active slot.
    let bench = PyList::empty(py);
    for (i, m) in side.team.iter().enumerate() {
        if active_idxs.contains(&(i as u8)) {
            continue;
        }
        bench.append(observe_bench_mon(py, i, m)?)?;
    }
    d.set_item("bench", bench)?;

    // Force-switch hint: an active slot is empty while the side is still alive.
    let must_act =
        (0..n_active).any(|slot| side.active[slot] == u8::MAX) && !side.is_defeated();
    d.set_item("must_act", must_act)?;

    Ok(d)
}

/// Build the full observation dict for a battle. Shared by
/// `PyBattle::observe` and the batched leaf bridge in `solve_endgame`, so
/// the Python value function sees exactly the same schema whether it's
/// inspecting a live battle or scoring a search-frontier leaf.
fn build_observation<'py>(
    py: Python<'py>,
    b: &core::Battle,
) -> PyResult<Bound<'py, PyDict>> {
    let root = PyDict::new(py);
    root.set_item("turn", b.turn())?;
    root.set_item(
        "format",
        match b.format() {
            core::Format::Singles => "singles",
            core::Format::Doubles => "doubles",
        },
    )?;

    let weather = PyDict::new(py);
    weather.set_item("kind", weather_slug(b.weather))?;
    weather.set_item("turns", b.weather_turns)?;
    root.set_item("weather", weather)?;

    let terrain = PyDict::new(py);
    terrain.set_item("kind", terrain_slug(b.terrain))?;
    terrain.set_item("turns", b.terrain_turns)?;
    root.set_item("terrain", terrain)?;

    let field = PyDict::new(py);
    field.set_item("trick_room", b.trick_room_turns)?;
    field.set_item("gravity", b.gravity_turns)?;
    field.set_item("magic_room", b.magic_room_turns)?;
    field.set_item("wonder_room", b.wonder_room_turns)?;
    root.set_item("field", field)?;

    let sides = PyList::empty(py);
    sides.append(observe_side(py, b, core::SideRef::P1)?)?;
    sides.append(observe_side(py, b, core::SideRef::P2)?)?;
    root.set_item("sides", sides)?;

    Ok(root)
}

fn map_team_err(e: core::TeamLoadError) -> PyErr {
    PyValueError::new_err(format!("{e}"))
}

/// (kind, actor_slot, move_slot_or_team_index, target_side, target_slot).
/// `-1` for unused fields.
type LegalChoice = (String, u8, u8, i8, i8);

/// Map a `StepResult` to the wire-level string the Python layer expects.
fn step_result_str(r: core::StepResult) -> &'static str {
    match r {
        core::StepResult::Continue => "continue",
        core::StepResult::Ended { winner: Some(core::SideRef::P1) } => "p1_win",
        core::StepResult::Ended { winner: Some(core::SideRef::P2) } => "p2_win",
        core::StepResult::Ended { winner: None } => "tie",
    }
}

/// Decode `(target_side, target_slot)` (as emitted by `legal_choices`)
/// back into an optional `Target`. A negative side or slot = no target.
fn target_from(target_side: i8, target_slot: i8) -> Option<core::Target> {
    if target_side < 0 || target_slot < 0 {
        return None;
    }
    let side = if target_side == 0 { core::SideRef::P1 } else { core::SideRef::P2 };
    Some(core::Target { side, slot: target_slot as u8 })
}

/// Convert one `legal_choices`-shaped tuple back into a core `Choice`,
/// so a caller can round-trip: read a legal choice → pass it to `step_move`.
fn choice_from_tuple(c: &LegalChoice) -> PyResult<core::Choice> {
    let (kind, actor_slot, arg, ts, tl) = (c.0.as_str(), c.1, c.2, c.3, c.4);
    Ok(match kind {
        "move" => core::Choice::Move { actor_slot, move_slot: arg, target: target_from(ts, tl) },
        "tera" => {
            core::Choice::Terastallize { actor_slot, move_slot: arg, target: target_from(ts, tl) }
        }
        "mega" => {
            core::Choice::MegaEvolve { actor_slot, move_slot: arg, target: target_from(ts, tl) }
        }
        "switch" => core::Choice::Switch { actor_slot, team_index: arg },
        "pass" => core::Choice::Pass { actor_slot },
        other => return Err(PyValueError::new_err(format!("unknown choice kind: {other}"))),
    })
}

#[pyclass(name = "Battle")]
pub struct PyBattle {
    inner: core::Battle,
}

#[pymethods]
impl PyBattle {
    /// Build a battle from two JSON team specs.
    #[staticmethod]
    #[pyo3(signature = (p1_team_json, p2_team_json, format = "doubles", seed = 0))]
    fn from_teams(
        p1_team_json: &str,
        p2_team_json: &str,
        format: &str,
        seed: u64,
    ) -> PyResult<Self> {
        let fmt = match format {
            "singles" => core::Format::Singles,
            "doubles" => core::Format::Doubles,
            other => return Err(PyValueError::new_err(format!("unknown format: {other}"))),
        };
        let p1 = core::TeamBuilder::from_json(p1_team_json).map_err(map_team_err)?;
        let p2 = core::TeamBuilder::from_json(p2_team_json).map_err(map_team_err)?;
        let cfg = core::BattleConfig { format: fmt, seed };
        Ok(Self { inner: core::Battle::new(cfg, p1, p2) })
    }

    #[getter]
    fn turn(&self) -> u32 {
        self.inner.turn()
    }

    #[getter]
    fn seed(&self) -> u64 {
        self.inner.seed()
    }

    #[getter]
    fn format(&self) -> &'static str {
        match self.inner.format() {
            core::Format::Singles => "singles",
            core::Format::Doubles => "doubles",
        }
    }

    /// One pass-only step for both sides (debug helper while moves are unwired).
    fn step_pass(&mut self) -> &'static str {
        let n = self.inner.format().active_count() as u8;
        let p1: Vec<core::Choice> =
            (0..n).map(|i| core::Choice::Pass { actor_slot: i }).collect();
        let p2: Vec<core::Choice> =
            (0..n).map(|i| core::Choice::Pass { actor_slot: i }).collect();
        match self.inner.step(&p1, &p2) {
            core::StepResult::Continue => "continue",
            core::StepResult::Ended { winner } => match winner {
                Some(core::SideRef::P1) => "p1_win",
                Some(core::SideRef::P2) => "p2_win",
                None => "tie",
            },
        }
    }

    /// Switch one active mon on a side; returns step result string.
    fn step_switch(&mut self, side: u8, actor_slot: u8, team_index: u8) -> PyResult<&'static str> {
        let s = match side {
            0 => core::SideRef::P1,
            1 => core::SideRef::P2,
            _ => return Err(PyValueError::new_err("side must be 0 or 1")),
        };
        let switch = core::Choice::Switch { actor_slot, team_index };
        let n = self.inner.format().active_count() as u8;
        let mut p1: Vec<core::Choice> =
            (0..n).map(|i| core::Choice::Pass { actor_slot: i }).collect();
        let mut p2: Vec<core::Choice> =
            (0..n).map(|i| core::Choice::Pass { actor_slot: i }).collect();
        match s {
            core::SideRef::P1 => p1[actor_slot as usize] = switch,
            core::SideRef::P2 => p2[actor_slot as usize] = switch,
        }
        Ok(match self.inner.step(&p1, &p2) {
            core::StepResult::Continue => "continue",
            core::StepResult::Ended { winner: Some(core::SideRef::P1) } => "p1_win",
            core::StepResult::Ended { winner: Some(core::SideRef::P2) } => "p2_win",
            core::StepResult::Ended { winner: None } => "tie",
        })
    }

    /// Execute one full turn from explicit per-side choice lists.
    ///
    /// `p1_choices` / `p2_choices` are lists of the same 5-tuple shape
    /// `legal_choices` returns — `(kind, actor_slot, arg, target_side,
    /// target_slot)` — so a caller round-trips: read legal choices, pick
    /// one per active slot, pass them straight back here. `kind` is one of
    /// `"move"`, `"tera"`, `"mega"`, `"switch"`, `"pass"`. Doubles take up
    /// to two choices per side (one per active slot); singles take one.
    /// Returns the step result string (`"continue"` / `"p1_win"` /
    /// `"p2_win"` / `"tie"`). This is the general stepping primitive;
    /// `step_pass` / `step_switch` remain as convenience helpers.
    #[pyo3(signature = (p1_choices, p2_choices))]
    fn step_move(
        &mut self,
        p1_choices: Vec<LegalChoice>,
        p2_choices: Vec<LegalChoice>,
    ) -> PyResult<&'static str> {
        let p1: Vec<core::Choice> =
            p1_choices.iter().map(choice_from_tuple).collect::<PyResult<_>>()?;
        let p2: Vec<core::Choice> =
            p2_choices.iter().map(choice_from_tuple).collect::<PyResult<_>>()?;
        Ok(step_result_str(self.inner.step(&p1, &p2)))
    }

    /// Deep-copy the battle into an independent `Battle`. Mutating the
    /// clone (or the original) never affects the other — for MCTS-style
    /// state branching in mimikyu.
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }

    /// `copy.deepcopy(battle)` support — same as `clone()`. The `_memo`
    /// arg is accepted and ignored (the state has no shared sub-objects
    /// to track).
    #[pyo3(signature = (_memo = None))]
    fn __deepcopy__(&self, _memo: Option<PyObject>) -> Self {
        Self { inner: self.inner.clone() }
    }

    /// `copy.copy(battle)` support — `Battle` has no interior sharing, so
    /// a shallow copy is a full independent clone too.
    fn __copy__(&self) -> Self {
        Self { inner: self.inner.clone() }
    }

    /// Serialize the full battle state to bincode bytes (compact, fast).
    /// Round-trips via `Battle.from_bytes`.
    fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        let bytes = bincode::serialize(&self.inner)
            .map_err(|e| PyValueError::new_err(format!("serialize failed: {e}")))?;
        Ok(PyBytes::new(py, &bytes))
    }

    /// Reconstruct a battle from bincode bytes produced by `to_bytes`.
    #[staticmethod]
    fn from_bytes(data: &[u8]) -> PyResult<Self> {
        let mut inner: core::Battle = bincode::deserialize(data)
            .map_err(|e| PyValueError::new_err(format!("deserialize failed: {e}")))?;
        // See `from_json`: recompute the `#[serde(skip)]` derived caches.
        inner.rehydrate_caches();
        Ok(Self { inner })
    }

    /// Serialize the full battle state to a JSON string. Slower / larger
    /// than `to_bytes` but human-inspectable; round-trips via `from_json`.
    fn to_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner)
            .map_err(|e| PyValueError::new_err(format!("to_json failed: {e}")))
    }

    /// Reconstruct a battle from a JSON string produced by `to_json`.
    #[staticmethod]
    fn from_json(data: &str) -> PyResult<Self> {
        let mut inner: core::Battle = serde_json::from_str(data)
            .map_err(|e| PyValueError::new_err(format!("from_json failed: {e}")))?;
        // `#[serde(skip)]` derived caches (move_locks / can_mega_evolve /
        // cached weather+terrain) come back at their Default after a
        // round-trip; recompute them from canonical state.
        inner.rehydrate_caches();
        Ok(Self { inner })
    }

    /// Current HP of the active mon in `(side, slot)`, or `None` if the
    /// slot is empty. Convenience accessor for asserting move effects.
    fn active_hp(&self, side: u8, slot: u8) -> PyResult<Option<u16>> {
        let s = match side {
            0 => core::SideRef::P1,
            1 => core::SideRef::P2,
            _ => return Err(PyValueError::new_err("side must be 0 or 1")),
        };
        Ok(self.inner.side(s).active_mon(slot as usize).map(|m| m.current_hp))
    }

    /// Return a list of (kind, actor_slot, move_slot_or_team_index, target_side, target_slot)
    /// tuples describing legal choices for one slot. Phase 2 wire-level Python API;
    /// will be replaced with a proper Choice object once mechanics land.
    fn legal_choices(&self, side: u8, actor_slot: u8) -> PyResult<Vec<LegalChoice>> {
        let s = match side {
            0 => core::SideRef::P1,
            1 => core::SideRef::P2,
            _ => return Err(PyValueError::new_err("side must be 0 or 1")),
        };
        Ok(self
            .inner
            .legal_choices(s, actor_slot)
            .into_iter()
            .map(|c| match c {
                core::Choice::Move { actor_slot, move_slot, target } => {
                    let (ts, tl) = target.map_or((-1, -1), |t| {
                        (
                            if t.side == core::SideRef::P1 { 0 } else { 1 },
                            t.slot as i8,
                        )
                    });
                    ("move".to_string(), actor_slot, move_slot, ts, tl)
                }
                core::Choice::Switch { actor_slot, team_index } => {
                    ("switch".to_string(), actor_slot, team_index, -1, -1)
                }
                core::Choice::Pass { actor_slot } => {
                    ("pass".to_string(), actor_slot, 0, -1, -1)
                }
                core::Choice::Terastallize { actor_slot, move_slot, target } => {
                    let (ts, tl) = target.map_or((-1, -1), |t| {
                        (
                            if t.side == core::SideRef::P1 { 0 } else { 1 },
                            t.slot as i8,
                        )
                    });
                    ("tera".to_string(), actor_slot, move_slot, ts, tl)
                }
                core::Choice::MegaEvolve { actor_slot, move_slot, target } => {
                    let (ts, tl) = target.map_or((-1, -1), |t| {
                        (
                            if t.side == core::SideRef::P1 { 0 } else { 1 },
                            t.slot as i8,
                        )
                    });
                    ("mega".to_string(), actor_slot, move_slot, ts, tl)
                }
            })
            .collect())
    }

    fn active_species(&self, side: u8, slot: u8) -> PyResult<Option<String>> {
        let s = match side {
            0 => core::SideRef::P1,
            1 => core::SideRef::P2,
            _ => return Err(PyValueError::new_err("side must be 0 or 1")),
        };
        Ok(self
            .inner
            .side(s)
            .active_mon(slot as usize)
            .map(|m| m.species().slug.to_string()))
    }

    /// Structured full-state observation as a nested Python dict.
    ///
    /// Full-info (both sides exposed — mimikyu masks itself if it needs
    /// hidden-info play). One `PyDict` is built per call; far cheaper than
    /// `to_json` + parse. Action enumeration stays in `legal_choices`.
    ///
    /// Schema (stable contract):
    /// ```text
    /// {
    ///   "turn": int,
    ///   "format": "singles" | "doubles",
    ///   "weather":  {"kind": str, "turns": int},
    ///   "terrain":  {"kind": str, "turns": int},
    ///   "field":    {"trick_room": int, "gravity": int,
    ///                "magic_room": int, "wonder_room": int},   # remaining turns, 0 = off
    ///   "sides": [
    ///     {
    ///       "side": int,
    ///       "must_act": bool,                                  # an active slot is empty (force switch)
    ///       "conditions": {
    ///         "reflect": int, "light_screen": int, "aurora_veil": int, "tailwind": int,
    ///         "safeguard": int, "mist": int,                   # remaining turns
    ///         "stealth_rock": bool, "spikes": int,             # spikes/toxic_spikes are layer counts
    ///         "toxic_spikes": int, "sticky_web": bool,
    ///         "wish_pending": bool, "future_pending": bool
    ///       },
    ///       "active": [
    ///         {
    ///           "slot": int, "species": str, "fainted": bool,
    ///           "hp": int, "max_hp": int,
    ///           "status": str, "status_counter": int,          # toxic stage / sleep turns; 0 otherwise
    ///           "boosts": {"atk":int,"def":int,"spa":int,"spd":int,"spe":int,"acc":int,"eva":int},
    ///           "types": [str, ...],                           # live (post-Tera) types
    ///           "ability": str,                                # "" if suppressed
    ///           "item": str | None,                            # effective item; None if itemless/suppressed
    ///           "tera_used": bool,                             # this side has spent its Tera
    ///           "is_terastallized": bool,                      # this mon is currently Terastallized
    ///           "moves": [{"id": str, "pp": int, "max_pp": int}, ...],   # empty slots omitted
    ///           "volatiles": {
    ///             "substitute_hp": int, "taunt": int, "encore": int, "disable": int,
    ///             "confusion": int, "throat_chop": int, "heal_block": int, "perish": int,
    ///             "leech_seed": bool, "salt_cure": bool, "protect": bool
    ///           }
    ///         }, ...
    ///       ],
    ///       "bench": [
    ///         {"team_index": int, "species": str, "hp": int, "max_hp": int,
    ///          "status": str, "fainted": bool}, ...
    ///       ]
    ///     },
    ///     { ... side 1 ... }
    ///   ]
    /// }
    /// ```
    fn observe<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        build_observation(py, &self.inner)
    }

    fn __repr__(&self) -> String {
        format!(
            "Battle(turn={}, format={:?}, p1_team={}, p2_team={})",
            self.inner.turn(),
            self.inner.format(),
            self.inner.p1.team.len(),
            self.inner.p2.team.len(),
        )
    }
}

/// Parse a Pokémon Showdown / Pokepaste team export and verify it against a
/// format ruleset.
///
/// Returns the list of human-readable violation strings — **empty** iff the
/// team is fully legal for `format`. Raises `ValueError` on an unknown format
/// or an unparseable paste.
///
///   import vgc_engine
///   problems = vgc_engine.parse_and_verify(paste_text, format="regmb")
///   assert problems == []   # legal team
#[pyfunction]
#[pyo3(signature = (team_text, format = "regmb"))]
fn parse_and_verify(team_text: &str, format: &str) -> PyResult<Vec<String>> {
    let rules = core::rules_for(format)
        .ok_or_else(|| PyValueError::new_err(format!("unknown format: {format}")))?;
    let team = core::parse_showdown_export(team_text).map_err(map_team_err)?;
    Ok(match core::verify_team(&team, rules) {
        Ok(()) => Vec::new(),
        Err(violations) => violations.iter().map(|v| v.to_string()).collect(),
    })
}

/// Shape a `KoChance` into a small dict: `{"kind": "guaranteed"|"chance"|
/// "none", "pct": int|None}`.
fn ko_chance_dict<'py>(py: Python<'py>, ko: &core::calc::KoChance) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    match ko {
        core::calc::KoChance::Guaranteed => {
            d.set_item("kind", "guaranteed")?;
            d.set_item("pct", 100u8)?;
        }
        core::calc::KoChance::Chance { pct } => {
            d.set_item("kind", "chance")?;
            d.set_item("pct", *pct)?;
        }
        core::calc::KoChance::None => {
            d.set_item("kind", "none")?;
            d.set_item("pct", py.None())?;
        }
    }
    Ok(d)
}

/// Shape one `DamageResult` row into a Python dict (no nested crit — the
/// caller decides whether to attach the crit companion).
fn damage_result_dict<'py>(
    py: Python<'py>,
    r: &core::calc::DamageResult,
) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    let rolls = PyList::empty(py);
    for &v in r.rolls.iter() {
        rolls.append(v)?;
    }
    d.set_item("rolls", rolls)?;
    d.set_item("min", r.min)?;
    d.set_item("max", r.max)?;
    d.set_item("defender_max_hp", r.defender_max_hp)?;
    d.set_item("min_pct", r.min_pct)?;
    d.set_item("max_pct", r.max_pct)?;
    d.set_item("ko", ko_chance_dict(py, &r.ko_chance)?)?;

    // Multi-hit NHKO (2HKO/3HKO/…) plus its exact probability, and a
    // human label ("guaranteed 2HKO" / "56.3% to 3HKO" / "no KO").
    let mh = PyDict::new(py);
    mh.set_item("hits", r.multi_hit.hits)?;
    mh.set_item("chance", r.multi_hit.chance)?;
    mh.set_item("label", r.multi_hit.label())?;
    d.set_item("multi_hit", mh)?;

    Ok(d)
}

/// Fast damage calc, mirroring the `vgc calc` CLI.
///
/// `attacker` / `defender` accept the terse ` / `-delimited grammar (e.g.
/// `"Garchomp @ Life Orb / Jolly / 252 Atk"`) or a bare species/alias
/// (`"chomp"`). `move_` is a move name or alias (`"eq"`). Optional field:
/// `weather` (`sun`|`rain`|`sand`|`snow`), `terrain`
/// (`electric`|`grassy`|`psychic`|`misty`), `spread` (Doubles ×0.75).
///
/// Returns a dict:
/// ```text
/// {
///   "rolls": [int; 16], "min": int, "max": int,
///   "defender_max_hp": int, "min_pct": float, "max_pct": float,
///   "ko":   {"kind": "guaranteed"|"chance"|"none", "pct": int|None},
///   "multi_hit": {"hits": int, "chance": float, "label": str},
///   "crit": { ...same shape, no nested crit... } | None
/// }
/// ```
///
///   import vgc_engine
///   r = vgc_engine.calc("chomp", "lando", "eq")
///   r["min"], r["max"], r["multi_hit"]["label"]
#[pyfunction]
#[pyo3(signature = (attacker, defender, move_, weather = None, terrain = None, spread = false))]
fn calc<'py>(
    py: Python<'py>,
    attacker: &str,
    defender: &str,
    move_: &str,
    weather: Option<&str>,
    terrain: Option<&str>,
    spread: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let atk = core::calc::QuickMon::parse(attacker)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    let def = core::calc::QuickMon::parse(defender)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

    let mut field = core::calc::Field::none();
    if let Some(w) = weather {
        field.weather = parse_weather(w)?;
    }
    if let Some(t) = terrain {
        field.terrain = parse_terrain(t)?;
    }
    field.spread = spread;

    let r = core::calc::calc(&atk, &def, move_, field)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;

    let d = damage_result_dict(py, &r)?;
    // Attach the crit companion (present on a non-crit, non-zero calc).
    match &r.crit {
        Some(c) => d.set_item("crit", damage_result_dict(py, c)?)?,
        None => d.set_item("crit", py.None())?,
    }
    Ok(d)
}

/// Parse a weather slug into the core enum (Python-facing errors).
fn parse_weather(s: &str) -> PyResult<core::Weather> {
    match s.to_ascii_lowercase().as_str() {
        "sun" | "harshsunshine" => Ok(core::Weather::Sun),
        "rain" => Ok(core::Weather::Rain),
        "sand" | "sandstorm" => Ok(core::Weather::Sand),
        "snow" | "hail" => Ok(core::Weather::Snow),
        other => Err(PyValueError::new_err(format!(
            "unknown weather '{other}' (want sun|rain|sand|snow)"
        ))),
    }
}

/// Parse a terrain slug into the core enum (Python-facing errors).
fn parse_terrain(s: &str) -> PyResult<core::Terrain> {
    match s.to_ascii_lowercase().as_str() {
        "electric" => Ok(core::Terrain::Electric),
        "grassy" => Ok(core::Terrain::Grassy),
        "psychic" => Ok(core::Terrain::Psychic),
        "misty" => Ok(core::Terrain::Misty),
        other => Err(PyValueError::new_err(format!(
            "unknown terrain '{other}' (want electric|grassy|psychic|misty)"
        ))),
    }
}

/// Serialize a core `Choice` into the same 5-tuple wire shape
/// `legal_choices` emits: `(kind, actor_slot, arg, target_side,
/// target_slot)`. Inverse of `choice_from_tuple`, so `solve_endgame`'s
/// returned policies round-trip straight back into `step_move`.
fn choice_to_tuple(c: &core::Choice) -> LegalChoice {
    let target_tuple = |target: Option<core::Target>| -> (i8, i8) {
        target.map_or((-1, -1), |t| {
            (if t.side == core::SideRef::P1 { 0 } else { 1 }, t.slot as i8)
        })
    };
    match *c {
        core::Choice::Move { actor_slot, move_slot, target } => {
            let (ts, tl) = target_tuple(target);
            ("move".to_string(), actor_slot, move_slot, ts, tl)
        }
        core::Choice::Terastallize { actor_slot, move_slot, target } => {
            let (ts, tl) = target_tuple(target);
            ("tera".to_string(), actor_slot, move_slot, ts, tl)
        }
        core::Choice::MegaEvolve { actor_slot, move_slot, target } => {
            let (ts, tl) = target_tuple(target);
            ("mega".to_string(), actor_slot, move_slot, ts, tl)
        }
        core::Choice::Switch { actor_slot, team_index } => {
            ("switch".to_string(), actor_slot, team_index, -1, -1)
        }
        core::Choice::Pass { actor_slot } => ("pass".to_string(), actor_slot, 0, -1, -1),
    }
}

/// Build the `[(choice_tuple, prob), ...]` Python list for one side's
/// mixed strategy.
fn policy_to_py<'py>(
    py: Python<'py>,
    policy: &[(core::Choice, f64)],
) -> PyResult<Bound<'py, PyList>> {
    let out = PyList::empty(py);
    for (choice, prob) in policy {
        let entry = PyList::empty(py);
        // (kind, actor_slot, arg, target_side, target_slot) tuple + prob.
        let ct = choice_to_tuple(choice);
        entry.append(ct)?;
        entry.append(*prob)?;
        out.append(entry)?;
    }
    Ok(out)
}

/// Solve one turn's Nash equilibrium with a **batched** Python leaf
/// evaluator.
///
/// Runs the double-oracle single-ply solver (`vgc_solver::solve_turn`)
/// over the current battle state. The value function is supplied as a
/// Python callable — the engine stays fully opaque to whatever scores the
/// leaves. Each time the solver expands an outcome frontier it calls
/// `leaf` ONCE with a `list[dict]` of observations (the same schema as
/// `Battle.observe()`), one per frontier state, and expects back a
/// `list[float]` of the same length (row-player payoff per state, by the
/// `+1 = P1 win` / `-1 = P2 win` convention).
///
/// The GIL is held for the duration of the solve (this function runs on
/// the calling Python thread and the callback re-acquires it), and the
/// callback fires once per frontier — batched — never once per leaf.
///
/// Returns a dict:
/// ```text
/// {
///   "value": float,                        # Nash value, P1's perspective
///   "row_policy": [ [choice_tuple, prob], ... ],   # P1 mixed strategy
///   "col_policy": [ [choice_tuple, prob], ... ],   # P2 mixed strategy
///   "iterations": int,
///   "row_support_size": int,
///   "col_support_size": int
/// }
/// ```
/// where `choice_tuple` is the same `(kind, actor_slot, arg,
/// target_side, target_slot)` shape `legal_choices` returns. Returns
/// `None` if either side has no legal choices (terminal state).
///
///   import vgc_engine
///   b = vgc_engine.Battle.from_teams(p1, p2, format="doubles")
///   sol = vgc_engine.solve_endgame(b, lambda obs: [0.0] * len(obs))
///   sol["value"], sol["row_policy"]
#[pyfunction]
#[pyo3(signature = (battle, leaf, record_seed = 0))]
fn solve_endgame<'py>(
    py: Python<'py>,
    battle: &PyBattle,
    leaf: PyObject,
    record_seed: u64,
) -> PyResult<Option<Bound<'py, PyDict>>> {
    // Any error raised by the Python callback (or observation build) is
    // stashed here and re-raised after the solve returns, since the
    // `BatchLeafEval` signature is infallible (`-> Vec<f64>`). `Rc<RefCell>`
    // because `BatchLeafEval` is `Box<dyn FnMut + 'static>` and can't hold
    // a stack borrow. The closure never outlives this function (solve_turn
    // is synchronous), so single-threaded `Rc` is sound.
    let callback_err: std::rc::Rc<std::cell::RefCell<Option<PyErr>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    let value = {
        let err_slot = std::rc::Rc::clone(&callback_err);
        let leaf = leaf.clone_ref(py);

        let batch_leaf: vgc_solver::BatchLeafEval =
            Box::new(move |states: &[&core::Battle]| {
                // If a prior batch already failed, short-circuit to zeros;
                // the stored error is re-raised after the solve unwinds.
                if err_slot.borrow().is_some() {
                    return vec![0.0; states.len()];
                }
                Python::with_gil(|py| match eval_batch_py(py, &leaf, states) {
                    Ok(scores) => scores,
                    Err(e) => {
                        *err_slot.borrow_mut() = Some(e);
                        vec![0.0; states.len()]
                    }
                })
            });

        vgc_solver::solve_turn(&battle.inner, batch_leaf, record_seed)
    };

    if let Some(e) = callback_err.borrow_mut().take() {
        return Err(e);
    }

    let sol = match value {
        Some(s) => s,
        None => return Ok(None),
    };

    let d = PyDict::new(py);
    d.set_item("value", sol.value)?;
    d.set_item("row_policy", policy_to_py(py, &sol.row_policy)?)?;
    d.set_item("col_policy", policy_to_py(py, &sol.col_policy)?)?;
    d.set_item("iterations", sol.iterations)?;
    d.set_item("row_support_size", sol.row_support_size)?;
    d.set_item("col_support_size", sol.col_support_size)?;
    Ok(Some(d))
}

/// Stable lowercase slug for a solved node's provenance. Mirrors
/// `vgc_solver::Provenance` (the recursive solver's exactness tag) so the
/// Python caller can decide whether to trust the value as an EXACT
/// endgame result (`"terminal"` / `"exact"`) or a budget-capped estimate
/// (`"depth_limit"` / `"node_limit"`).
fn provenance_slug(p: vgc_solver::Provenance) -> &'static str {
    match p {
        vgc_solver::Provenance::Terminal => "terminal",
        vgc_solver::Provenance::Exact => "exact",
        vgc_solver::Provenance::Estimated(vgc_solver::EstReason::DepthLimit) => "depth_limit",
        vgc_solver::Provenance::Estimated(vgc_solver::EstReason::NodeLimit) => "node_limit",
    }
}

/// **Exact recursive endgame solve** — the T2 value oracle.
///
/// Runs `vgc_solver::endgame_solve`, the multi-turn recursive
/// matrix-game solver, from the current `battle` state down to terminal
/// nodes using the crate's Rust-native winner-aware leaf
/// (`hp_ratio_leaf`: `±1` on a decided game, HP-fraction difference at a
/// budget-capped frontier). No Python callback is involved, so this runs
/// entirely in Rust and is safe to call in a hot loop.
///
/// The returned `value` is the Nash value **from P1's perspective, in
/// `[-1, 1]`** (`+1` = certain P1 win, `-1` = certain P2 win, `0` = even
/// / draw). The caller converts to an own-side `[0, 1]` win probability.
///
/// `provenance` reports whether the solve reached terminal exactly:
///   - `"terminal"` — the input state was already decided.
///   - `"exact"`    — every reachable leaf was a real terminal node; the
///                    value is a true minimax win probability. **Trust as T2.**
///   - `"depth_limit"` / `"node_limit"` — at least one branch bottomed
///     out on the `max_depth` / `node_budget` budget and was leaf-estimated
///     via `hp_ratio_leaf`. **NOT an exact value** — the position was too
///     large to solve within budget.
///
/// Returns a dict:
/// ```text
/// {
///   "value": float,          # Nash value, P1 perspective, [-1, 1]
///   "provenance": str,       # terminal | exact | depth_limit | node_limit
///   "depth_remaining": int,  # plies of budget left at the root solve
///   "row_policy": [ [choice_tuple, prob], ... ],   # P1 slot-0 mixed strategy
///   "col_policy": [ [choice_tuple, prob], ... ],   # P2 slot-0 mixed strategy
/// }
/// ```
/// `choice_tuple` is the same `(kind, actor_slot, arg, target_side,
/// target_slot)` shape `legal_choices` returns. For doubles the
/// `row_policy` / `col_policy` are the (lossy) slot-0 projection of the
/// full joint policy.
///
///   import vgc_engine
///   b = vgc_engine.Battle.from_teams(p1, p2, format="doubles")
///   sol = vgc_engine.solve_endgame_exact(b, max_depth=32)
///   sol["value"], sol["provenance"]
#[pyfunction]
#[pyo3(signature = (battle, max_depth = 16, node_budget = 1_000_000, exact_hp = false))]
fn solve_endgame_exact<'py>(
    py: Python<'py>,
    battle: &PyBattle,
    max_depth: u32,
    node_budget: u64,
    exact_hp: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let cfg = vgc_solver::SolverConfig {
        max_depth,
        node_budget,
        exact_hp,
        ..vgc_solver::SolverConfig::default()
    };

    let node = vgc_solver::endgame_solve(&battle.inner, &cfg, vgc_solver::hp_ratio_leaf);

    let d = PyDict::new(py);
    d.set_item("value", node.value)?;
    d.set_item("provenance", provenance_slug(node.provenance))?;
    d.set_item("depth_remaining", node.depth_remaining)?;
    d.set_item("row_policy", policy_to_py(py, &node.row_policy)?)?;
    d.set_item("col_policy", policy_to_py(py, &node.col_policy)?)?;
    Ok(d)
}

/// Bridge one batched leaf call into Python: build a `list[dict]` of
/// observations, invoke the callable, coerce the returned `list[float]`
/// back to a `Vec<f64>`. Errors (wrong length, non-float, exception in
/// the callback) surface as `PyErr` for the caller to re-raise.
fn eval_batch_py(
    py: Python<'_>,
    leaf: &PyObject,
    states: &[&core::Battle],
) -> PyResult<Vec<f64>> {
    let obs = PyList::empty(py);
    for b in states {
        obs.append(build_observation(py, b)?)?;
    }
    let result = leaf.call1(py, (obs,))?;
    let scores: Vec<f64> = result.extract(py)?;
    if scores.len() != states.len() {
        return Err(PyValueError::new_err(format!(
            "leaf returned {} scores for {} states",
            scores.len(),
            states.len()
        )));
    }
    Ok(scores)
}

#[pymodule]
fn vgc_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBattle>()?;
    m.add_function(wrap_pyfunction!(parse_and_verify, m)?)?;
    m.add_function(wrap_pyfunction!(calc, m)?)?;
    m.add_function(wrap_pyfunction!(solve_endgame, m)?)?;
    m.add_function(wrap_pyfunction!(solve_endgame_exact, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
