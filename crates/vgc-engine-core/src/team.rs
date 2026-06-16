//! Team loader. Accepts a JSON document and produces a Vec<Pokemon>.
//!
//! Schema (one entry per slot, max 6):
//! ```json
//! {
//!   "species": "garchomp",
//!   "level": 50,
//!   "ability": "roughskin",
//!   "item": "lifeorb",
//!   "nature": "adamant",
//!   "moves": ["earthquake","dragonclaw","protect","ironhead"],
//!   "evs": {"hp":4,"atk":252,"def":0,"spa":0,"spd":0,"spe":252},
//!   "ivs": {"hp":31,"atk":31,"def":31,"spa":31,"spd":31,"spe":31}
//! }
//! ```
//!
//! Unknown / empty optional fields default to competitive sane values
//! (level 50, max IVs, zero EVs, Serious nature, ability/item = none).

use serde::Deserialize;

use crate::pokemon::{
    compute_stats, nature_by_slug, Nature, Pokemon, StatSpread, Status,
};

use vgc_engine_data as data;

#[derive(Debug)]
pub enum TeamLoadError {
    Parse(String),
    UnknownSpecies(String),
    UnknownMove(String),
    UnknownAbility(String),
    UnknownItem(String),
    UnknownNature(String),
    Empty,
    TooMany(usize),
}

impl std::fmt::Display for TeamLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeamLoadError::Parse(s) => write!(f, "team json parse error: {s}"),
            TeamLoadError::UnknownSpecies(s) => write!(f, "unknown species: {s}"),
            TeamLoadError::UnknownMove(s) => write!(f, "unknown move: {s}"),
            TeamLoadError::UnknownAbility(s) => write!(f, "unknown ability: {s}"),
            TeamLoadError::UnknownItem(s) => write!(f, "unknown item: {s}"),
            TeamLoadError::UnknownNature(s) => write!(f, "unknown nature: {s}"),
            TeamLoadError::Empty => write!(f, "team is empty"),
            TeamLoadError::TooMany(n) => write!(f, "team has {n} members (max 6)"),
        }
    }
}

impl std::error::Error for TeamLoadError {}

#[derive(Debug, Deserialize)]
pub struct TeamMember {
    pub species: String,
    #[serde(default = "default_level")]
    pub level: u8,
    #[serde(default)]
    pub ability: Option<String>,
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default = "default_nature")]
    pub nature: String,
    #[serde(default)]
    pub moves: Vec<String>,
    #[serde(default = "default_ivs")]
    pub ivs: StatSpread,
    #[serde(default)]
    pub evs: StatSpread,
    /// Tera type as a PS type slug ("fire", "water", "stellar", ...).
    /// Optional — when omitted, defaults to the species' first type
    /// (gen-9 set-build convention). Resolved to a type code 0..=17
    /// (Stellar deferred to its own PR). Case-insensitive.
    #[serde(default)]
    pub teratype: Option<String>,
}

fn default_level() -> u8 { 50 }
fn default_nature() -> String { "serious".to_string() }
fn default_ivs() -> StatSpread { StatSpread::MAX_IV }

fn slugify(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn lookup_species(name: &str) -> Result<u16, TeamLoadError> {
    let slug = slugify(name);
    data::SPECIES
        .iter()
        .position(|s| s.slug == slug)
        .map(|i| i as u16)
        .ok_or_else(|| TeamLoadError::UnknownSpecies(name.to_string()))
}

fn lookup_move(name: &str) -> Result<u16, TeamLoadError> {
    let slug = slugify(name);
    data::MOVES
        .iter()
        .position(|m| m.slug == slug)
        .map(|i| i as u16)
        .ok_or_else(|| TeamLoadError::UnknownMove(name.to_string()))
}

fn lookup_ability(name: &str) -> Result<u16, TeamLoadError> {
    let slug = slugify(name);
    data::ABILITIES
        .iter()
        .position(|a| a.slug == slug)
        .map(|i| i as u16)
        .ok_or_else(|| TeamLoadError::UnknownAbility(name.to_string()))
}

fn lookup_item(name: &str) -> Result<u16, TeamLoadError> {
    let slug = slugify(name);
    data::ITEMS
        .iter()
        .position(|i| i.slug == slug)
        .map(|i| i as u16)
        .ok_or_else(|| TeamLoadError::UnknownItem(name.to_string()))
}

fn lookup_nature(name: &str) -> Result<&'static Nature, TeamLoadError> {
    let slug = slugify(name);
    nature_by_slug(&slug).ok_or_else(|| TeamLoadError::UnknownNature(name.to_string()))
}

/// Build a single Pokémon from a team-member spec.
pub fn build_member(m: &TeamMember) -> Result<Pokemon, TeamLoadError> {
    let species_id = lookup_species(&m.species)?;
    let species = &data::SPECIES[species_id as usize];
    let nature = lookup_nature(&m.nature)?;
    let stats = compute_stats(species, m.level, &m.ivs, &m.evs, nature);

    let mut moves = [u16::MAX; 4];
    let mut pp = [0u8; 4];
    for (i, mv) in m.moves.iter().take(4).enumerate() {
        let id = lookup_move(mv)?;
        moves[i] = id;
        pp[i] = data::MOVES[id as usize].pp;
    }

    // Tera type: PS type names → typechart index (0..=17). Stellar is
    // gen-9 special (effective only on first hit per type) and isn't
    // in our 18-type chart — represented as 255 for now and consumers
    // gate on the sentinel until the Stellar PR lands.
    let tera_type = match m.teratype.as_deref() {
        None => species.types[0],
        Some(s) => {
            let lower = s.to_ascii_lowercase();
            if lower == "stellar" {
                255
            } else {
                data::TYPE_NAMES
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&lower))
                    .map(|i| i as u8)
                    .unwrap_or(species.types[0])
            }
        }
    };

    let ability_id = m
        .ability
        .as_deref()
        .map(lookup_ability)
        .transpose()?
        .unwrap_or(u16::MAX);
    let item_id = m
        .item
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(lookup_item)
        .transpose()?
        .unwrap_or(u16::MAX);

    Ok(Pokemon {
        species_id,
        level: m.level,
        moves,
        pp,
        ability_id,
        item_id,
        current_hp: stats.hp,
        stats,
        status: Status::None,
        boosts: [0; 7],
        fainted: false,
        is_protected_this_turn: false,
        stall_counter: 0,
        used_stall_this_turn: false,
        turns_active: 0,
        flinched_this_turn: false,
        helping_handed_this_turn: false,
        redirecting_this_turn: false,
        redirecting_is_powder: false,
        damaged_this_turn: false,
        toxic_counter: 0,
        locked_move_slot: 255,
        switched_in_this_turn: false,
        substitute_hp: 0,
        sleep_turns: 0,
        last_used_move_slot: 255,
        encore_turns: 0,
        encored_move_slot: 255,
        boosted_stat: 255,
        booster_locked: false,
        pending_self_switch: false,
        ability_suppressed: false,
        crit_stage_volatile: 0,
        last_attacker: (255, 255),
        last_attacker_category: 255,
        last_damage_taken: 0,
        tera_type,
        terastallized: false,
        semi_invuln: 0,
        charging_turns: 0,
        charging_move_slot: 255,
        must_recharge: false,
        lockin_turns: 0,
        lockin_move_slot: 255,
    })
}

/// Convenience: parse a team JSON document (array of TeamMember) into a
/// ready-to-use Vec<Pokemon>.
pub struct TeamBuilder;

impl TeamBuilder {
    pub fn from_json(s: &str) -> Result<Vec<Pokemon>, TeamLoadError> {
        let specs: Vec<TeamMember> =
            serde_json::from_str(s).map_err(|e| TeamLoadError::Parse(e.to_string()))?;
        if specs.is_empty() {
            return Err(TeamLoadError::Empty);
        }
        if specs.len() > 6 {
            return Err(TeamLoadError::TooMany(specs.len()));
        }
        specs.iter().map(build_member).collect()
    }
}
