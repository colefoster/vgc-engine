//! vgc-engine-core — Pokémon battle simulator.
//!
//! Phase 2 PR-1: real battle state + team loader + switches. No damage,
//! abilities, items, or status effects yet — those land in follow-up PRs.

#![forbid(unsafe_code)]

pub use vgc_engine_data as data;
pub use vgc_engine_data::Gender;

mod choice;
mod format;
pub mod format_rules;
mod pokemon;
mod side;
mod team;
mod team_export;

pub mod ability;
mod accuracy;
pub mod battle;
pub mod canonical_hash;
#[cfg(feature = "chance")]
pub mod chance;
pub mod damage;
pub mod item;
pub mod order;
pub mod rng;
mod secondary;
pub mod step_machine;
pub mod terrain;
pub mod weather;

pub use battle::{Battle, BattleConfig, StepResult};
pub use choice::{Choice, MoveSlot, Target};
pub use damage::{
    calculate_damage, damage_range, damage_range_in_ctx, type_effectiveness, DamageContext,
    TypeEff,
};
pub use format::Format;
pub use format_rules::{
    rules_for, verify_showdown_text, verify_team, FormatRules, Rule, Violation, REG_M_B,
};
pub use order::{action_order, effective_speed, ScheduledAction};
pub use pokemon::{
    compute_stats, nature_by_id, nature_by_slug, nature_id, nature_id_by_slug, FinalStats,
    Nature, Pokemon, Stat, StatSpread, Status, Volatile, VolatileKind, VolatileSet,
};
pub use rng::{DrawSpace, RecordedDraw, Rng, RngDecision, RngEvent, RngKey};
pub use side::{Side, SideRef};
pub use step_machine::{PendingYield, StepCursor, StepPhase, StepProgress};
pub use team::{boosted_max_pp, build_member, TeamBuilder, TeamLoadError, TeamMember};
pub use team_export::parse_showdown_export;
pub use terrain::Terrain;
pub use weather::Weather;
