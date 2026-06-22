//! pyo3 bindings.
//!
//! Phase 2 PR-1: exposes real Battle with team loading + switches.
//! Damage/abilities/items still stubs.
//!
//!   import vgc_engine
//!   b = vgc_engine.Battle.from_teams(p1_json, p2_json, format="doubles")
//!   choices = b.legal_choices(side=0, slot=0)
//!   b.step_pass()           # both sides pass — turn ticks

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use vgc_engine_core as core;

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
        let inner: core::Battle = bincode::deserialize(data)
            .map_err(|e| PyValueError::new_err(format!("deserialize failed: {e}")))?;
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
        let inner: core::Battle = serde_json::from_str(data)
            .map_err(|e| PyValueError::new_err(format!("from_json failed: {e}")))?;
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

#[pymodule]
fn vgc_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBattle>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
