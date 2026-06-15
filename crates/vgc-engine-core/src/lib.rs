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

pub mod ability;
pub mod battle;
pub mod damage;
pub mod item;
pub mod order;
pub mod rng;
pub mod weather;

pub use battle::{Battle, BattleConfig, StepResult};
pub use choice::{Choice, MoveSlot, Target};
pub use damage::{calculate_damage, damage_range, type_effectiveness, DamageContext, TypeEff};
pub use format::Format;
pub use order::{action_order, effective_speed, ScheduledAction};
pub use pokemon::{Nature, Pokemon, Stat, Status};
pub use rng::Rng;
pub use side::{Side, SideRef};
pub use team::{TeamBuilder, TeamLoadError, TeamMember};
pub use weather::Weather;
