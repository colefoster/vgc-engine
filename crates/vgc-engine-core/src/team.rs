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
    compute_stats, nature_by_slug, Nature, Pokemon, StatSpread,
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
    /// Explicitly-set gender as a PS token: `"M"`, `"F"`, or `"N"`
    /// (case-insensitive; also accepts `"male"`/`"female"`/`"genderless"`).
    /// Populated by the Showdown-export parser from the `(M)` / `(F)`
    /// tag after the species. When `None`, gender falls back to the
    /// species' fixed gender or a roll (PS precedence). PS
    /// `sim/pokemon.ts:340`.
    #[serde(default)]
    pub gender: Option<String>,
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

/// Parse an explicit gender token (`M`/`F`/`N` or the long forms).
/// `None` for unrecognized input — the caller falls back to species
/// gender. PS precedence treats only `M`/`F`/`N` as overrides
/// (`sim/pokemon.ts:339`).
fn parse_gender_token(s: &str) -> Option<data::Gender> {
    match s.trim().to_ascii_lowercase().as_str() {
        "m" | "male" => Some(data::Gender::Male),
        "f" | "female" => Some(data::Gender::Female),
        "n" | "genderless" | "none" => Some(data::Gender::Genderless),
        _ => None,
    }
}

/// Max PP for a move id with the standard +3 PP-Ups (PP Max) applied —
/// the PP a competitively-built mon actually has.
///
/// PS `sim/pokemon.ts:351-373` always builds move slots with `ppUps = 3`
/// (the `set.pp`/`set.ppup` fields are NOT consulted by the simulator; it
/// hard-codes 3 PP-ups for every move that can take them). `sim/battle.ts:
/// 2387 calculatePP`: `noPPBoosts` moves keep their base PP, everything else
/// gets `pp * (5 + 3) / 5 = pp * 8 / 5`. All boostable base PPs are multiples
/// of 5, so the integer division is exact (max result 64, fits u8).
///
/// `noPPBoosts` moves in gen 9 (data/moves.ts): Revival Blessing, Sketch,
/// Struggle. Trump Card is additionally forced to 0 PP-ups by pokemon.ts:361.
/// Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/PP_Up>.
pub fn boosted_max_pp(move_id: u16) -> u8 {
    if move_id == u16::MAX {
        return 0;
    }
    let mv = &data::MOVES[move_id as usize];
    if matches!(mv.slug, "revivalblessing" | "sketch" | "struggle" | "trumpcard") {
        return mv.pp;
    }
    // pp * 8 / 5 — exact for every multiple-of-5 base PP. Compute in u16 to
    // avoid the intermediate `pp * 8` overflowing u8 (40 * 8 = 320).
    ((mv.pp as u16 * 8) / 5) as u8
}

/// Build a single Pokémon from a team-member spec.
pub fn build_member(m: &TeamMember) -> Result<Pokemon, TeamLoadError> {
    let species_id = lookup_species(&m.species)?;
    let species = &data::SPECIES[species_id as usize];
    let nature = lookup_nature(&m.nature)?;
    // `nature.slug` is guaranteed to be in NATURES (it came from there).
    let nature_id = crate::pokemon::nature_id_by_slug(nature.slug)
        .expect("nature from NATURES has a table id");
    let stats = compute_stats(species, m.level, &m.ivs, &m.evs, nature);

    let mut moves = [u16::MAX; 4];
    let mut pp = [0u8; 4];
    for (i, mv) in m.moves.iter().take(4).enumerate() {
        let id = lookup_move(mv)?;
        moves[i] = id;
        // Competitive sets are PP-maxed (PS sim always builds with +3 PP-Ups);
        // current PP starts at the boosted max. See `boosted_max_pp`.
        pp[i] = boosted_max_pp(id);
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

    // Gender precedence (PS `sim/pokemon.ts:340`):
    //   explicit set gender → species fixed gender → ratio'd (rolled).
    // A `Random` species inherits `Random` here; the battle constructor
    // resolves it to Male/Female at `>player` time. An explicit gender on
    // a ratio'd species locks it (no roll).
    let gender = m
        .gender
        .as_deref()
        .and_then(parse_gender_token)
        .unwrap_or(species.gender);

    Ok(Pokemon::with_identity(
        species_id,
        m.level,
        gender,
        moves,
        pp,
        ability_id,
        item_id,
        stats,
        stats.hp,
        m.ivs,
        m.evs,
        nature_id,
        tera_type,
    ))
}

/// Convenience: parse a team JSON document (array of TeamMember) into a
/// ready-to-use Vec<Pokemon>.
pub struct TeamBuilder;

impl TeamBuilder {
    pub fn from_json(s: &str) -> Result<Vec<Pokemon>, TeamLoadError> {
        let specs: Vec<TeamMember> =
            serde_json::from_str(s).map_err(|e| TeamLoadError::Parse(e.to_string()))?;
        Self::finalize(specs)
    }

    /// Parse a Showdown export blob (`Mon @ Item` / `Ability:` / `EVs:` /
    /// `<Nature> Nature` / `- Move` ...) — the same format Pokepaste hands out.
    pub fn from_showdown_text(s: &str) -> Result<Vec<Pokemon>, TeamLoadError> {
        let specs = crate::team_export::parse_showdown_export(s)?;
        Self::finalize(specs)
    }

    fn finalize(specs: Vec<TeamMember>) -> Result<Vec<Pokemon>, TeamLoadError> {
        if specs.is_empty() {
            return Err(TeamLoadError::Empty);
        }
        if specs.len() > 6 {
            return Err(TeamLoadError::TooMany(specs.len()));
        }
        specs.iter().map(build_member).collect()
    }
}

#[cfg(test)]
mod gender_tests {
    use super::*;
    use crate::battle::{Battle, BattleConfig};
    use crate::format::Format;
    use crate::rng::Rng;

    fn member(species: &str, gender: Option<&str>) -> TeamMember {
        TeamMember {
            species: species.into(),
            level: 50,
            ability: None,
            item: None,
            nature: "serious".into(),
            moves: vec!["tackle".into()],
            ivs: StatSpread::MAX_IV,
            evs: StatSpread::default(),
            teratype: None,
            gender: gender.map(|g| g.to_string()),
        }
    }

    #[test]
    fn boosted_max_pp_applies_three_pp_ups() {
        let id = |slug: &str| data::MOVES.iter().position(|m| m.slug == slug).unwrap() as u16;
        // pp * 8 / 5 for boostable moves (exact for multiples of 5).
        assert_eq!(boosted_max_pp(id("closecombat")), 8, "base 5 → 8");
        assert_eq!(boosted_max_pp(id("earthquake")), 16, "base 10 → 16");
        assert_eq!(boosted_max_pp(id("bravebird")), 24, "base 15 → 24");
        assert_eq!(boosted_max_pp(id("protect")), 16, "base 10 → 16");
        assert_eq!(boosted_max_pp(id("scratch")), 56, "base 35 → 56");
        // noPPBoosts moves keep their base PP.
        assert_eq!(boosted_max_pp(id("struggle")), data::MOVES[id("struggle") as usize].pp,
                   "Struggle takes no PP boost");
        assert_eq!(boosted_max_pp(id("sketch")), data::MOVES[id("sketch") as usize].pp,
                   "Sketch takes no PP boost");
    }

    #[test]
    fn build_member_starts_with_pp_maxed_moves() {
        // Competitive sets are PP-maxed; the engine builds current PP at the
        // boosted max (PS sim always uses +3 PP-Ups). Earthquake base 10 → 16.
        let mut m = member("Garchomp", None);
        m.moves = vec!["earthquake".into(), "closecombat".into(), "protect".into()];
        let mon = build_member(&m).unwrap();
        assert_eq!(mon.pp[0], 16, "Earthquake PP-maxed to 16");
        assert_eq!(mon.pp[1], 8, "Close Combat PP-maxed to 8");
        assert_eq!(mon.pp[2], 16, "Protect PP-maxed to 16");
    }

    #[test]
    fn build_member_gender_precedence() {
        // Genderless species → Genderless (PS species.gender = "N").
        assert_eq!(
            build_member(&member("Magnemite", None)).unwrap().gender,
            data::Gender::Genderless
        );
        // Always-male / always-female species.
        assert_eq!(build_member(&member("Tauros", None)).unwrap().gender, data::Gender::Male);
        assert_eq!(
            build_member(&member("Nidoqueen", None)).unwrap().gender,
            data::Gender::Female
        );
        // Ratio'd species, gender unspecified → Random (rolled later at
        // battle construction, NOT in build_member which has no RNG).
        assert_eq!(build_member(&member("Garchomp", None)).unwrap().gender, data::Gender::Random);
        // Explicit set gender overrides everything, including a ratio'd
        // species — no roll happens for it.
        assert_eq!(build_member(&member("Garchomp", Some("F"))).unwrap().gender, data::Gender::Female);
        assert_eq!(build_member(&member("Garchomp", Some("M"))).unwrap().gender, data::Gender::Male);
    }

    #[test]
    fn showdown_export_gender_tag_is_parsed() {
        // PS export gender tag `(F)` after the species locks the gender.
        let team = TeamBuilder::from_showdown_text(
            "Garchomp (F) @ Life Orb\nAbility: Rough Skin\n- Earthquake\n",
        )
        .unwrap();
        assert_eq!(team[0].gender, data::Gender::Female);
    }

    #[test]
    fn gender_rolled_at_construction_psgen5_matches_ps() {
        // PsGen5 mirrors PS's LCG: on seed [1,2,3,4] a `gen9customgame`
        // battle rolls the first ratio'd mon (p1 slot 0) male and the
        // second (p2 slot 0) female — verified against the PS sim. Fixed
        // and genderless mons in between consume NO draw, so a genderless
        // mon ahead of a ratio'd one on the same side does not shift the
        // ratio'd mon's roll.
        let p1 = TeamBuilder::from_json(
            r#"[{"species":"magnemite","moves":["tackle"]},
                {"species":"garchomp","moves":["tackle"]}]"#,
        )
        .unwrap();
        let p2 = TeamBuilder::from_json(r#"[{"species":"amoonguss","moves":["tackle"]}]"#).unwrap();
        let cfg = BattleConfig { format: Format::Singles, seed: 0 };
        let battle = Battle::with_rng(cfg, Rng::ps_gen5([1, 2, 3, 4]), p1, p2);

        // Magnemite: genderless, no draw.
        assert_eq!(battle.p1.team[0].gender, data::Gender::Genderless);
        // Garchomp consumes the FIRST gender draw (random(2)=0 → male).
        assert_eq!(battle.p1.team[1].gender, data::Gender::Male);
        // Amoonguss (p2) consumes the SECOND gender draw (random(2)=1 → female).
        assert_eq!(battle.p2.team[0].gender, data::Gender::Female);
    }

    #[test]
    fn ratio_gender_defaults_male_without_psgen5() {
        // Splitmix battles draw no gender at construction (to keep
        // seed-pinned tests stable); ratio'd mons default to male.
        let p1 = TeamBuilder::from_json(r#"[{"species":"garchomp","moves":["tackle"]}]"#).unwrap();
        let p2 = TeamBuilder::from_json(r#"[{"species":"amoonguss","moves":["tackle"]}]"#).unwrap();
        let cfg = BattleConfig { format: Format::Singles, seed: 42 };
        let battle = Battle::new(cfg, p1, p2);
        assert_eq!(battle.p1.team[0].gender, data::Gender::Male);
        assert_eq!(battle.p2.team[0].gender, data::Gender::Male);
    }
}
