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
use vgc_engine_core as core;

fn map_team_err(e: core::TeamLoadError) -> PyErr {
    PyValueError::new_err(format!("{e}"))
}

/// (kind, actor_slot, move_slot_or_team_index, target_side, target_slot).
/// `-1` for unused fields.
type LegalChoice = (String, u8, u8, i8, i8);

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
