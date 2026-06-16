//! vgc-engine-core — Pokémon battle simulator.
//!
//! Phase 2 PR-1: real battle state + team loader + switches. No damage,
//! abilities, items, or status effects yet — those land in follow-up PRs.

#![forbid(unsafe_code)]

pub use vgc_engine_data as data;

mod choice;
mod format;
mod pokemon;
mod side;
mod team;
mod team_export;

pub mod ability;
pub mod battle;
pub mod damage;
pub mod item;
pub mod order;
pub mod rng;
pub mod terrain;
pub mod weather;

pub use battle::{Battle, BattleConfig, StepResult};
pub use choice::{Choice, MoveSlot, Target};
pub use damage::{
    calculate_damage, damage_range, damage_range_in_ctx, type_effectiveness, DamageContext,
    TypeEff,
};
pub use format::Format;
pub use order::{action_order, effective_speed, ScheduledAction};
pub use pokemon::{
    compute_stats, nature_by_slug, FinalStats, Nature, Pokemon, Stat, StatSpread, Status,
    Volatile, VolatileKind, VolatileSet,
};
pub use rng::{Rng, RngEvent};
pub use side::{Side, SideRef};
pub use team::{build_member, TeamBuilder, TeamLoadError, TeamMember};
pub use team_export::parse_showdown_export;
pub use terrain::Terrain;
pub use weather::Weather;
