//! `vgc_engine.BattleStream`: Pokémon Showdown's BattleStream in-process
//! (docs/protocol.md, "Use as a local Showdown replacement").
//!
//! ```python
//! s = vgc_engine.BattleStream()
//! s.write('>start {"formatid":"gen9vgc2025regi","seed":"sodium,0001..."}')
//! s.write('>player p1 {"name":"A","team":"<packed>"}')
//! s.write('>player p2 {"name":"B","team":"<packed>"}')
//! for msg in s.read_all():   # "update\n...", "sideupdate\np1\n|request|{...}", "end\n{...}"
//!     ...
//! streams = s.player_streams()       # like PS getPlayerStreams
//! streams.p1.write("team 1234")      # sent as ">p1 team 1234"
//! streams.p1.read_all()              # p1's chunks: its view of each update, its requests / errors
//! ```

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::collections::VecDeque;
use vgc_engine_core::ps_stream::{player_stream_chunks, BattleStream, Replay, StreamOptions};

const VIEW_NAMES: [&str; 4] = ["omniscient", "spectator", "p1", "p2"];

/// PS's `BattleStream` (sim/battle-stream.ts): write input lines, read the
/// messages it pushes. With `ps-rng` (the wheel's `ps-rng` feature) the
/// `>start` seed drives PS's own PRNG, so the messages match PS's.
#[pyclass(name = "BattleStream", module = "vgc_engine")]
pub struct PyBattleStream {
    inner: BattleStream,
    /// After `player_streams()`: each view's pending chunks.
    views: Option<[VecDeque<String>; 4]>,
}

impl PyBattleStream {
    /// Route every pending message into the four views.
    fn pump(&mut self) {
        let Some(views) = self.views.as_mut() else { return };
        while let Some(m) = self.inner.read() {
            for (v, chunk) in player_stream_chunks(&m) {
                views[v].push_back(chunk);
            }
        }
    }
}

#[pymethods]
impl PyBattleStream {
    /// `protocol` is `"ps"` (PS's bytes) or `"compact"` (no cosmetic
    /// lines); `replay` / `spectate` are simulate-battle's `--replay` /
    /// `--spectate` (only the log, as that channel sees it); `timestamp`
    /// fixes the `|t:|` value (tests).
    #[new]
    #[pyo3(signature = (protocol = "ps", replay = false, spectate = false, keep_alive = false, timestamp = None))]
    fn new(protocol: &str, replay: bool, spectate: bool, keep_alive: bool, timestamp: Option<u64>) -> PyResult<Self> {
        let mode = vgc_engine_core::ProtocolMode::parse(protocol).ok_or_else(|| PyValueError::new_err(format!("unknown protocol: {protocol} (ps | compact)")))?;
        let replay = if spectate { Replay::Spectator } else if replay { Replay::Omniscient } else { Replay::Off };
        Ok(Self { inner: BattleStream::new(StreamOptions { replay, mode, keep_alive, timestamp, trace: false }), views: None })
    }

    /// Write one chunk of input (`>start ...`, `>p1 move 1 1, move 2`, ...;
    /// several lines may go in one chunk, as PS reads them). Raises
    /// ValueError where PS's stream throws (an unknown command, a bad team).
    fn write(&mut self, chunk: &str) -> PyResult<()> {
        self.inner.write(chunk).map_err(PyValueError::new_err)?;
        self.pump();
        Ok(())
    }

    /// The next message, or `None` (`"update\n<lines>"`,
    /// `"sideupdate\np1\n<line>"`, `"end\n<json>"`, `"requesteddata\n..."`).
    fn read(&mut self) -> PyResult<Option<String>> {
        if self.views.is_some() {
            return Err(PyValueError::new_err("this stream was split with player_streams(); read those"));
        }
        Ok(self.inner.read())
    }

    /// Every pending message.
    fn read_all(&mut self) -> PyResult<Vec<String>> {
        if self.views.is_some() {
            return Err(PyValueError::new_err("this stream was split with player_streams(); read those"));
        }
        Ok(self.inner.drain())
    }

    /// True once the stream has ended (after `end`, unless keep_alive).
    #[getter]
    fn ended(&self) -> bool {
        self.inner.is_closed()
    }

    /// PS `battle.inputLog`.
    #[getter]
    fn input_log(&self) -> Vec<String> {
        self.inner.input_log().to_vec()
    }

    /// PS `getPlayerStreams` (sim/battle-stream.ts:395-462): `omniscient`,
    /// `spectator`, `p1`, `p2`. From here on the messages are read through
    /// them; `p1.write(choice)` sends `>p1 <choice>`.
    fn player_streams(slf: Bound<'_, Self>) -> PyResult<PyPlayerStreams> {
        {
            let mut s = slf.borrow_mut();
            if s.views.is_none() {
                s.views = Some(Default::default());
                s.pump();
            }
        }
        let py = slf.py();
        let parent: Py<PyBattleStream> = slf.unbind();
        let mk = |v: usize| Py::new(py, PyPlayerStream { parent: parent.clone_ref(py), view: v });
        Ok(PyPlayerStreams { omniscient: mk(0)?, spectator: mk(1)?, p1: mk(2)?, p2: mk(3)? })
    }
}

/// The four streams `BattleStream.player_streams()` returns.
#[pyclass(name = "PlayerStreams", module = "vgc_engine")]
pub struct PyPlayerStreams {
    #[pyo3(get)]
    omniscient: Py<PyPlayerStream>,
    #[pyo3(get)]
    spectator: Py<PyPlayerStream>,
    #[pyo3(get)]
    p1: Py<PyPlayerStream>,
    #[pyo3(get)]
    p2: Py<PyPlayerStream>,
}

/// One of `getPlayerStreams`' streams.
#[pyclass(name = "PlayerStream", module = "vgc_engine")]
pub struct PyPlayerStream {
    parent: Py<PyBattleStream>,
    view: usize,
}

#[pymethods]
impl PyPlayerStream {
    /// `omniscient`, `spectator`, `p1` or `p2`.
    #[getter]
    fn name(&self) -> &'static str {
        VIEW_NAMES[self.view]
    }

    /// The next chunk of this stream, or `None`.
    fn read(&self, py: Python<'_>) -> Option<String> {
        let mut p = self.parent.borrow_mut(py);
        p.pump();
        p.views.as_mut().and_then(|v| v[self.view].pop_front())
    }

    /// Every pending chunk.
    fn read_all(&self, py: Python<'_>) -> Vec<String> {
        let mut p = self.parent.borrow_mut(py);
        p.pump();
        p.views.as_mut().map(|v| v[self.view].drain(..).collect()).unwrap_or_default()
    }

    /// `omniscient.write` passes input through; `p1` / `p2` prefix each
    /// line with `>p1 ` / `>p2 ` (a choice). `spectator` can't write.
    fn write(&self, py: Python<'_>, data: &str) -> PyResult<()> {
        let chunk = match self.view {
            0 => data.to_string(),
            2 | 3 => {
                let id = if self.view == 2 { "p1" } else { "p2" };
                data.split('\n').map(|l| format!(">{id} {l}")).collect::<Vec<_>>().join("\n")
            }
            _ => return Err(PyValueError::new_err("the spectator stream is read-only")),
        };
        let mut p = self.parent.borrow_mut(py);
        p.inner.write(&chunk).map_err(PyValueError::new_err)?;
        p.pump();
        Ok(())
    }
}
