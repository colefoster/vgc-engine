//! pyo3 bindings — Phase 1 stub.
//!
//! Exposes `Battle`, `Choice`, `StepResult` so mimikyu can:
//!   `import vgc_engine; b = vgc_engine.Battle()`
//!
//! No mechanics — those land in Phase 2.

use pyo3::prelude::*;
use vgc_engine_core as core;

#[pyclass(name = "Choice", eq, eq_int)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyChoice {
    Noop,
}

impl From<PyChoice> for core::Choice {
    fn from(_: PyChoice) -> Self {
        core::Choice::Noop
    }
}

#[pyclass(name = "StepResult", eq, eq_int)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PyStepResult {
    Continue,
    Ended,
}

#[pyclass(name = "Battle")]
pub struct PyBattle {
    inner: core::Battle,
}

#[pymethods]
impl PyBattle {
    #[new]
    #[pyo3(signature = (seed = 0))]
    fn new(seed: u64) -> Self {
        Self {
            inner: core::Battle::new(core::BattleConfig { seed }),
        }
    }

    #[getter]
    fn turn(&self) -> u32 {
        self.inner.turn()
    }

    #[getter]
    fn seed(&self) -> u64 {
        self.inner.seed()
    }

    fn step(&mut self, p1: PyChoice, p2: PyChoice) -> PyStepResult {
        match self.inner.step(p1.into(), p2.into()) {
            core::StepResult::Continue => PyStepResult::Continue,
            core::StepResult::Ended { .. } => PyStepResult::Ended,
        }
    }

    fn legal_choices(&self, _side: u8) -> Vec<PyChoice> {
        Vec::new()
    }

    fn __repr__(&self) -> String {
        format!("Battle(turn={}, seed={})", self.inner.turn(), self.inner.seed())
    }
}

#[pymodule]
fn vgc_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBattle>()?;
    m.add_class::<PyChoice>()?;
    m.add_class::<PyStepResult>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
