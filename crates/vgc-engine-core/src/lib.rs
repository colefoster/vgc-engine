//! vgc-engine-core — Pokémon battle simulator.
//!
//! Phase 2 PR-1: real battle state + team loader + switches. No damage,
//! abilities, items, or status effects yet — those land in follow-up PRs.

#![forbid(unsafe_code)]

pub use vgc_engine_data as data;

/// Push one formatted protocol line when the battle keeps a protocol log;
/// nothing is formatted otherwise (`crate::protocol`).
macro_rules! plog {
    ($b:expr, $($arg:tt)*) => {
        if $b.protocol.on() {
            let __line = format!($($arg)*);
            $b.protocol.add(__line);
        }
    };
}

pub use vgc_engine_data::Gender;

/// `|tag|<ident of the mon in side/slot>{rest}`; `rest` is formatted only
/// when the battle keeps a protocol log.
macro_rules! pmon {
    ($b:expr, $tag:expr, $side:expr, $slot:expr) => {
        if $b.protocol.on() {
            $b.p_mon($tag, $side, $slot, "");
        }
    };
    ($b:expr, $tag:expr, $side:expr, $slot:expr, $($arg:tt)+) => {
        if $b.protocol.on() {
            let __rest = format!($($arg)+);
            $b.p_mon($tag, $side, $slot, &__rest);
        }
    };
}

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
mod battle_protocol;
mod protocol_residual;
pub mod ps_formats;
pub mod ps_json;
pub mod ps_teams;
pub mod ps_request;
pub mod ps_session;
pub mod ps_stream;
pub mod ps_observe;
pub mod ps_position;
pub mod ps_board;
pub mod protocol;
pub mod calc;
pub mod canonical_hash;
mod hash_ser;
#[cfg(feature = "chance")]
pub mod chance;
pub mod damage;
pub mod damage_api;
pub mod item;
pub mod order;
pub mod rng;
#[cfg(feature = "ps-rng")]
pub mod ps_rng;
mod secondary;
pub mod step_machine;
pub mod terrain;
pub mod weather;

pub use battle::{
    ko_split_disabled_state, set_crit_refine_disabled, set_ko_split_disabled, Battle,
    BattleConfig, StepResult,
};
pub use choice::{Choice, MoveSlot, Target, RECHARGE_MOVE_SLOT, STRUGGLE_MOVE_SLOT};
pub use damage::{
    calculate_damage, damage_range, damage_range_in_ctx, type_effectiveness, DamageContext,
    TypeEff,
};
pub use calc::{
    best_move, calc, calc_default, matchup, min_evs_to_ko, min_evs_to_survive, ohko_chance,
    outspeeds, speed_tier, survives, AtkStat, CalcError, DamageResult, DefStat, Field, KoChance,
    Matchup, MoveDamage, MultiHitKo, QuickMon, SpeedContext, SpeedWinner, SurvivalEffect,
};
pub use damage_api::{damage_only, damage_only_detail, damage_only_with, CalcMods, DamageQuery, DamageRolls};
pub use format::Format;
pub use format_rules::{
    rules_for, verify_showdown_text, verify_team, FormatRules, Rule, Violation, REG_M_B,
};
pub use order::{action_order, effective_speed, ScheduledAction};
pub use pokemon::{
    compute_stats, nature_by_id, nature_by_slug, nature_id, nature_id_by_slug, FinalStats,
    Nature, Pokemon, Stat, StatSpread, Status, Volatile, VolatileKind, VolatileSet,
};
pub use rng::{
    expand_uniform_damage, DamageSegments, DrawSpace, RecordedDraw, Rng, RngDecision, RngEvent,
    RngKey, SlotRef, NO_SLOT,
};
pub use protocol::{ProtocolLog, ProtocolMode, ProtocolOptions, ProtocolView, StreamChunk, SyncKind, SyncPoint};
pub use side::{Side, SideRef};
pub use step_machine::{PendingYield, StepCursor, StepPhase, StepProgress};
pub use team::{boosted_max_pp, build_member, build_member_in, champions_max_pp, TeamBuilder, TeamLoadError, TeamMember};
pub use team_export::parse_showdown_export;
pub use terrain::Terrain;
pub use weather::Weather;
