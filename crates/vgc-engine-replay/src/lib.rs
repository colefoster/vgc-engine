//! PS replay-protocol parser.
//!
//! Parses Pokémon Showdown sim-protocol log strings (from replay JSON dumps)
//! into a typed [`Event`] stream plus header metadata ([`Replay`]).
//!
//! Reference: <https://github.com/smogon/pokemon-showdown/blob/master/sim/SIM-PROTOCOL.md>
//!
//! This crate is allowed to allocate; it runs offline against the replay
//! corpus and never inside the engine's `step()` hot loop.

mod choices;
mod event;
mod parser;
mod recon;
mod replay;
mod runner;
mod scorer;
mod trace;

pub use choices::ChoiceExtractor;
pub use event::{Event, PokeSlot};
pub use parser::parse_line;
pub use recon::{
    input_from_team_preview, observe_events, parse_details, CanonicalDefault, PokeObservation,
    ReconError, ReconInput, TeamRecon,
};
pub use replay::{ParseError, PlayerInfo, Replay, TeamPreviewPoke, TurnView};
pub use runner::{RunnerError, RunnerInit};
pub use scorer::{score_replay, ReplayScore, TurnScore, DEFAULT_HP_TOLERANCE};
pub use trace::{hp_trace, parse_hp, HpEvent, HpSource};
