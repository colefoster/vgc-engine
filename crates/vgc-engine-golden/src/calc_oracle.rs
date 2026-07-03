//! Reusable core of the `calc_oracle` example, shared with the
//! `calc_oracle_suite` integration test.
//!
//! Given a scenario JSON (attacker + defender + move + trials), runs N
//! battles where the attacker uses the named move into a Splash-using
//! defender and reports every observed damage value. The test suite
//! asserts that set is a subset of `@smogon/calc`'s 16-roll expected
//! damage array (`damage ∪ damage_crit`), independent of PS.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use vgc_engine_core::terrain::Terrain;
use vgc_engine_core::weather::Weather;
use vgc_engine_core::{
    Battle, BattleConfig, Format, Pokemon, SideRef, Status, TeamBuilder,
};

use crate::parse_turn_actions;

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

/// Run `sc.trials` iterations of a one-turn battle and collect every
/// non-faint, non-miss damage observation on the defender.
pub fn observe_scenario(sc: &Scenario) -> Result<Observation, String> {
    let p1_text = render_team(&sc.attacker, &sc.move_name);
    let p2_text = render_team(&sc.defender, "Splash");
    let p1_team = TeamBuilder::from_showdown_text(&p1_text)
        .map_err(|e| format!("p1 team parse: {e:?}"))?;
    let p2_team = TeamBuilder::from_showdown_text(&p2_text)
        .map_err(|e| format!("p2 team parse: {e:?}"))?;

    let p1_choices = parse_turn_actions(
        &serde_json::Value::String("move 1".into()),
        SideRef::P1, 1,
    ).map_err(|e| format!("p1 action: {e}"))?;
    let p2_choices = parse_turn_actions(
        &serde_json::Value::String("move 1".into()),
        SideRef::P2, 1,
    ).map_err(|e| format!("p2 action: {e}"))?;
    // Control: p1 also uses Splash (its move slot 4). Renders the same
    // field state as a real trial but with zero move-damage, so any HP
    // delta on the defender is the pure EOT delta — sand tick, Grassy
    // Terrain heal, whatever. We subtract this from every real trial's
    // observation, back-solving the move-damage the calc oracle wants.
    let p1_splash = parse_turn_actions(
        &serde_json::Value::String("move 4".into()),
        SideRef::P1, 1,
    ).map_err(|e| format!("p1 splash action: {e}"))?;
    let eot_delta_defender: i32 = {
        let cfg = BattleConfig { format: Format::Singles, seed: 0xEC0_DE };
        let mut b = Battle::new(cfg, p1_team.clone(), p2_team.clone());
        if sc.attacker.terastallized { b.p1.team[0].terastallized = true; }
        if sc.defender.terastallized { b.p2.team[0].terastallized = true; }
        if let Some(s) = &sc.attacker.status { b.p1.team[0].status = parse_status(s)?; }
        if let Some(s) = &sc.defender.status { b.p2.team[0].status = parse_status(s)?; }
        if let Some(field) = &sc.field {
            if let Some(w) = &field.weather { b.set_weather(parse_weather(w)?); }
            if let Some(t) = &field.terrain { b.set_terrain(parse_terrain(t)?); }
        }
        // Set defender to half HP so an EOT heal (Grassy Terrain,
        // Leftovers, ...) has room to register — at full HP the heal
        // caps to zero and we'd under-measure the delta. Sand chip and
        // other damage sources are unaffected by HP level.
        let max_hp = b.p2.team[0].stats.hp;
        b.p2.team[0].current_hp = (max_hp / 2).max(1);
        let before = b.p2.team[0].current_hp as i32;
        let _ = b.step(&p1_splash, &p2_choices);
        let after = b.p2.team[0].current_hp as i32;
        // Positive = net damage on defender, negative = net heal.
        before - after
    };

    let mut observed = Vec::with_capacity(sc.trials as usize);
    let mut fainted = 0u32;
    let mut missed = 0u32;
    let mut target_max: u16 = 0;
    let mut errors = Vec::new();

    for i in 0..sc.trials {
        let cfg = BattleConfig { format: Format::Singles, seed: i as u64 };
        let mut battle = Battle::new(cfg, p1_team.clone(), p2_team.clone());
        if sc.attacker.terastallized {
            battle.p1.team[0].terastallized = true;
        }
        if sc.defender.terastallized {
            battle.p2.team[0].terastallized = true;
        }
        if let Some(s) = &sc.attacker.status {
            battle.p1.team[0].status = parse_status(s)?;
        }
        if let Some(s) = &sc.defender.status {
            battle.p2.team[0].status = parse_status(s)?;
        }
        // Force weather/terrain when the scenario declares one, so the
        // generator can cross field states without needing an on-team
        // ability (Drizzle/Drought/Psychic Surge) to set them. Existing
        // hand-authored scenarios that already set the field via an
        // ability just re-assert the same value here (idempotent).
        if let Some(field) = &sc.field {
            if let Some(w) = &field.weather {
                battle.set_weather(parse_weather(w)?);
            }
            if let Some(t) = &field.terrain {
                battle.set_terrain(parse_terrain(t)?);
            }
        }
        let max = max_hp(&battle.p2.team[0]);
        target_max = max;
        // Snapshot pre-step defender status. PS resolves residual-damage
        // status ticks (Burn / Poison / Toxic) at end-of-turn BEFORE we
        // read `current_hp`, so a trial where the move inflicted a fresh
        // status reports `move_damage + tick` — an EOT confound identical
        // to the Grassy Terrain heal-tick harness limitation already noted
        // in the calc-oracle suite. We filter those trials rather than
        // back the tick out: exact tick math varies by ability (Poison
        // Heal, Magic Guard, Heatproof) and status. Status-chance RNG
        // draws are unaffected — only observation is filtered.
        let pre_status = battle.p2.team[0].status;
        let _ = battle.step(&p1_choices, &p2_choices);
        let Some(mon) = battle.p2.active_mon(0) else {
            errors.push(format!("trial {i}: defender slot empty"));
            continue;
        };
        let dmg = max.saturating_sub(mon.current_hp);
        if mon.fainted {
            fainted += 1;
            continue;
        }
        if dmg == 0 {
            missed += 1;
            continue;
        }
        if mon.status != pre_status
            && matches!(
                mon.status,
                Status::Burn | Status::Poison | Status::Toxic
            )
        {
            // Move inflicted a fresh residual-damage status this turn;
            // its tick differs from the control (which assumes any
            // pre-existing status was already there). Drop to avoid
            // mixing an unknown-magnitude tick into the roll.
            continue;
        }
        // Back out the deterministic EOT delta measured by the control
        // trial so the observation is pure move-damage.
        let true_dmg = (dmg as i32) - eot_delta_defender;
        if true_dmg <= 0 {
            // Move dealt no damage this trial (e.g., a miss that also
            // failed to trigger EOT damage subtraction cleanly), or the
            // EOT heal exceeded the move damage. Skip.
            missed += 1;
            continue;
        }
        observed.push(true_dmg as u16);
    }

    let mut unique = observed.clone();
    unique.sort_unstable();
    unique.dedup();

    let mut observed_sorted = observed.clone();
    observed_sorted.sort_unstable();

    Ok(Observation {
        name: sc.name.clone(),
        move_name: sc.move_name.clone(),
        trials: sc.trials,
        target_max_hp: target_max,
        observed_damage: observed_sorted,
        observed_unique: unique,
        fainted_count: fainted,
        missed_count: missed,
        errors,
    })
}

fn max_hp(mon: &Pokemon) -> u16 { mon.stats.hp }

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
