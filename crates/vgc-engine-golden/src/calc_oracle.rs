//! Reusable core of the `calc_oracle` example, shared with the
//! `calc_oracle_suite` integration test.
//!
//! Given a scenario JSON (attacker + defender + move + trials), returns
//! the 16-roll damage array the engine deals under that field state.
//! The test suite asserts the set is a subset of `@smogon/calc`'s
//! 16-roll expected damage array (`damage ∪ damage_crit`), independent
//! of PS.
//!
//! Uses `vgc_engine_core::damage_only` — the "Option 2" pure-damage
//! API — instead of the pre-existing random-trial back-solver. Each
//! scenario yields exactly sixteen deterministic damage values from
//! sixteen forced-roll runs of the same synthetic battle; the
//! back-out-EOT-tick control trial is gone.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use vgc_engine_core::terrain::Terrain;
use vgc_engine_core::weather::Weather;
use vgc_engine_core::{damage_only, DamageQuery, Pokemon, Status, TeamBuilder};

#[derive(Debug, Clone, Deserialize)]
pub struct PokemonSpec {
    pub species: String,
    #[serde(default = "default_level")]
    pub level: u8,
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub ability: Option<String>,
    #[serde(default)]
    pub nature: Option<String>,
    #[serde(default)]
    pub evs: BTreeMap<String, u8>,
    #[serde(default)]
    pub ivs: BTreeMap<String, u8>,
    #[serde(default)]
    pub tera_type: Option<String>,
    #[serde(default)]
    pub terastallized: bool,
    #[serde(default)]
    pub status: Option<String>,
}

fn default_level() -> u8 { 50 }

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub attacker: PokemonSpec,
    pub defender: PokemonSpec,
    #[serde(rename = "move")]
    pub move_name: String,
    #[serde(default = "default_trials")]
    pub trials: u32,
    #[serde(default)]
    pub field: Option<FieldSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FieldSpec {
    /// One of "Sun" | "Rain" | "Sand" | "Snow".
    #[serde(default)]
    pub weather: Option<String>,
    /// One of "Electric" | "Grassy" | "Psychic" | "Misty".
    #[serde(default)]
    pub terrain: Option<String>,
}

fn default_trials() -> u32 { 200 }

#[derive(Debug, Serialize)]
pub struct Observation {
    pub name: String,
    #[serde(rename = "move")]
    pub move_name: String,
    pub trials: u32,
    pub target_max_hp: u16,
    pub observed_damage: Vec<u16>,
    pub observed_unique: Vec<u16>,
    pub fainted_count: u32,
    pub missed_count: u32,
    pub errors: Vec<String>,
}

/// The subset of `@smogon/calc`'s output the test suite needs.
#[derive(Debug, Deserialize)]
pub struct CalcExpectation {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub damage: Vec<u32>,
    #[serde(default)]
    pub damage_crit: Vec<u32>,
    #[serde(default)]
    pub damage_union: Vec<u32>,
}

fn label_of(k: &str) -> &'static str {
    match k {
        "hp" => "HP", "atk" => "Atk", "def" => "Def",
        "spa" => "SpA", "spd" => "SpD", "spe" => "Spe",
        _ => "",
    }
}

fn render_team(spec: &PokemonSpec, primary_move: &str) -> String {
    let mut s = String::new();
    s.push_str(&spec.species);
    if let Some(item) = &spec.item {
        s.push_str(" @ ");
        s.push_str(item);
    }
    s.push('\n');
    if let Some(ability) = &spec.ability {
        s.push_str("Ability: ");
        s.push_str(ability);
        s.push('\n');
    }
    s.push_str(&format!("Level: {}\n", spec.level));
    if let Some(tt) = &spec.tera_type {
        s.push_str("Tera Type: ");
        s.push_str(tt);
        s.push('\n');
    }
    if !spec.evs.is_empty() {
        let parts: Vec<String> = ["hp", "atk", "def", "spa", "spd", "spe"]
            .iter()
            .filter_map(|k| spec.evs.get(*k).map(|v| format!("{} {}", v, label_of(k))))
            .collect();
        if !parts.is_empty() {
            s.push_str("EVs: ");
            s.push_str(&parts.join(" / "));
            s.push('\n');
        }
    }
    if let Some(n) = &spec.nature {
        s.push_str(n);
        s.push_str(" Nature\n");
    }
    s.push_str("- ");
    s.push_str(primary_move);
    s.push('\n');
    s.push_str("- Splash\n- Splash\n- Splash\n");
    s
}

/// Compute the engine's 16-roll damage array for the scenario via the
/// `damage_only` API.
///
/// Legacy vestige: [`Observation`] still carries `trials` / `fainted_count`
/// / `missed_count`. The new API produces exactly 16 deterministic
/// values, so `trials = 16` and the two counters are always `0`. The
/// spec-check downstream only reads `observed_unique`, so retaining the
/// fields keeps the wire shape stable for anyone deserializing existing
/// harness dumps.
pub fn observe_scenario(sc: &Scenario) -> Result<Observation, String> {
    let p1_text = render_team(&sc.attacker, &sc.move_name);
    let p2_text = render_team(&sc.defender, "Splash");
    let p1_team = TeamBuilder::from_showdown_text(&p1_text)
        .map_err(|e| format!("p1 team parse: {e:?}"))?;
    let p2_team = TeamBuilder::from_showdown_text(&p2_text)
        .map_err(|e| format!("p2 team parse: {e:?}"))?;

    let mut attacker: Pokemon = p1_team.into_iter().next()
        .ok_or_else(|| "p1 team empty".to_string())?;
    let mut defender: Pokemon = p2_team.into_iter().next()
        .ok_or_else(|| "p2 team empty".to_string())?;

    if sc.attacker.terastallized { attacker.terastallized = true; }
    if sc.defender.terastallized { defender.terastallized = true; }
    if let Some(s) = &sc.attacker.status { attacker.status = parse_status(s)?; }
    if let Some(s) = &sc.defender.status { defender.status = parse_status(s)?; }

    let weather = sc
        .field
        .as_ref()
        .and_then(|f| f.weather.as_ref())
        .map(|w| parse_weather(w))
        .transpose()?
        .unwrap_or(Weather::None);
    let terrain = sc
        .field
        .as_ref()
        .and_then(|f| f.terrain.as_ref())
        .map(|t| parse_terrain(t))
        .transpose()?
        .unwrap_or(Terrain::None);

    // First slot of the built attacker holds the primary move (see
    // `render_team` — primary is line 1, then three Splashes). The
    // `damage_only` API re-writes moves[0] regardless, but we pass
    // the same id so the debug shape is coherent.
    let move_id = attacker.moves[0];
    let target_max = defender.stats.hp;

    let q = DamageQuery {
        attacker,
        defender,
        move_id,
        weather,
        terrain,
        is_crit: false,
        is_spread: false,
    };
    let rolls = damage_only(&q);

    let mut observed: Vec<u16> = rolls.iter().copied().filter(|v| *v > 0).collect();
    observed.sort_unstable();
    let mut unique = observed.clone();
    unique.dedup();

    Ok(Observation {
        name: sc.name.clone(),
        move_name: sc.move_name.clone(),
        // Vestigial after the `damage_only` rewrite — see the doc
        // comment on `Observation`. Left at the deterministic 16 to
        // match the number of engine calls made.
        trials: 16,
        target_max_hp: target_max,
        observed_damage: observed,
        observed_unique: unique,
        fainted_count: 0,
        missed_count: 0,
        errors: Vec::new(),
    })
}

fn parse_weather(s: &str) -> Result<Weather, String> {
    match s.to_ascii_lowercase().as_str() {
        "" | "none" | "clear" => Ok(Weather::None),
        "sun" | "harshsunshine" | "sunny" => Ok(Weather::Sun),
        "rain" => Ok(Weather::Rain),
        "sand" | "sandstorm" => Ok(Weather::Sand),
        "snow" | "hail" => Ok(Weather::Snow),
        other => Err(format!("unknown weather: {other}")),
    }
}

fn parse_terrain(s: &str) -> Result<Terrain, String> {
    match s.to_ascii_lowercase().as_str() {
        "" | "none" => Ok(Terrain::None),
        "electric" => Ok(Terrain::Electric),
        "grassy" => Ok(Terrain::Grassy),
        "psychic" => Ok(Terrain::Psychic),
        "misty" => Ok(Terrain::Misty),
        other => Err(format!("unknown terrain: {other}")),
    }
}

fn parse_status(s: &str) -> Result<Status, String> {
    match s.to_ascii_lowercase().as_str() {
        "none" | "" => Ok(Status::None),
        "brn" | "burn" => Ok(Status::Burn),
        "par" | "paralysis" => Ok(Status::Paralysis),
        "frz" | "freeze" => Ok(Status::Freeze),
        "psn" | "poison" => Ok(Status::Poison),
        "tox" | "toxic" => Ok(Status::Toxic),
        "slp" | "sleep" => Ok(Status::Sleep),
        other => Err(format!("unknown status: {other}")),
    }
}
