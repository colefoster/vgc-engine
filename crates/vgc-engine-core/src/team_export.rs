//! Pokémon Showdown export-format parser.
//!
//! The export format is the multi-line plaintext used by PS and Pokepaste:
//!
//! ```text
//! Garchomp @ Life Orb
//! Ability: Rough Skin
//! Tera Type: Steel
//! Level: 50
//! EVs: 4 HP / 252 Atk / 252 Spe
//! Adamant Nature
//! - Earthquake
//! - Dragon Claw
//! - Protect
//! - Iron Head
//!
//! Amoonguss @ Sitrus Berry
//! Ability: Regenerator
//! ...
//! ```
//!
//! Sets are separated by blank lines. First line is `Species @ Item` (item
//! optional). Subsequent keyed lines: `Ability:`, `Level:`, `Tera Type:`,
//! `EVs:`, `IVs:`, `<Nature> Nature`. Moves are `- <Move Name>`.
//!
//! Ignored fields (PS-only metadata that doesn't affect simulation):
//! `Shiny:`, `Happiness:`, `Pokeball:`, `Hidden Power:`, `Gigantamax:`,
//! gender tags, and the parenthesized nickname/species disambiguation
//! (we always take the species token).
//!
//! PS reference: `sim/teams.ts:parseExportedTeamLine` (lines 458-560).

use crate::pokemon::StatSpread;
use crate::team::{TeamLoadError, TeamMember};

/// Parse a Showdown export blob into one [`TeamMember`] per non-empty stanza.
///
/// Returns an empty `Vec` for empty input — callers (`TeamBuilder`) reject
/// that with [`TeamLoadError::Empty`].
pub fn parse_showdown_export(s: &str) -> Result<Vec<TeamMember>, TeamLoadError> {
    let mut out: Vec<TeamMember> = Vec::new();
    let mut current: Option<TeamMember> = None;

    for raw in s.lines() {
        let line = raw.trim_end_matches([' ', '\t']).trim_end_matches('\\');
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if let Some(m) = current.take() {
                out.push(m);
            }
            continue;
        }

        if current.is_none() {
            current = Some(parse_header(trimmed)?);
            continue;
        }

        let m = current.as_mut().unwrap();
        apply_line(m, trimmed)?;
    }

    if let Some(m) = current.take() {
        out.push(m);
    }

    Ok(out)
}

fn parse_header(line: &str) -> Result<TeamMember, TeamLoadError> {
    let (left, item) = match line.split_once(" @ ") {
        Some((l, r)) => (l.trim(), Some(r.trim().to_string())),
        None => (line, None),
    };

    let mut species_token = left;

    // Capture the PS gender tag (` (M)` / ` (F)` / ` (N)`) that follows
    // the species/nickname, then strip it off the species token. PS
    // export format: `Nickname (Species) (M) @ Item`.
    let mut gender: Option<String> = None;
    for (tag, g) in [(" (M)", "M"), (" (F)", "F"), (" (N)", "N")] {
        if let Some(stripped) = species_token.strip_suffix(tag) {
            species_token = stripped;
            gender = Some(g.to_string());
        }
    }

    let species = if species_token.ends_with(')') {
        if let Some((_nick, rest)) = species_token.rsplit_once('(') {
            rest.trim_end_matches(')').trim().to_string()
        } else {
            species_token.to_string()
        }
    } else {
        species_token.to_string()
    };

    Ok(TeamMember {
        species,
        level: 50,
        ability: None,
        item: item.filter(|s| !s.is_empty() && s.to_ascii_lowercase() != "noitem"),
        nature: "serious".to_string(),
        moves: Vec::new(),
        ivs: StatSpread::MAX_IV,
        evs: StatSpread::default(),
        teratype: None,
        gender,
    })
}

fn apply_line(m: &mut TeamMember, line: &str) -> Result<(), TeamLoadError> {
    if let Some(rest) = line.strip_prefix("Ability: ") {
        m.ability = Some(rest.trim().to_string());
    } else if let Some(rest) = line.strip_prefix("Level: ") {
        m.level = rest.trim().parse::<u8>().unwrap_or(50);
    } else if let Some(rest) = line.strip_prefix("Tera Type: ") {
        let t = rest.trim().to_string();
        if !t.is_empty() { m.teratype = Some(t); }
    } else if let Some(rest) = line.strip_prefix("EVs: ") {
        m.evs = parse_stats(rest, 0);
    } else if let Some(rest) = line.strip_prefix("IVs: ") {
        m.ivs = parse_stats(rest, 31);
    } else if let Some(nature) = strip_nature_suffix(line) {
        m.nature = nature.to_ascii_lowercase();
    } else if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("~ ")) {
        let mv = normalize_hidden_power(rest.trim());
        if m.moves.len() < 4 {
            m.moves.push(mv);
        }
    }
    // Shiny / Happiness / Pokeball / Hidden Power / Gigantamax / Trait are
    // silently skipped — PS-only metadata the sim doesn't consume.
    Ok(())
}

fn parse_stats(line: &str, default: u8) -> StatSpread {
    let mut s = StatSpread { hp: default, atk: default, def: default, spa: default, spd: default, spe: default };
    for chunk in line.split('/') {
        let parts: Vec<&str> = chunk.split_whitespace().collect();
        if parts.len() < 2 { continue; }
        let v: u16 = parts[0].parse().unwrap_or(default as u16);
        let v = v.min(255) as u8;
        match parts[1].to_ascii_lowercase().as_str() {
            "hp" => s.hp = v,
            "atk" => s.atk = v,
            "def" => s.def = v,
            "spa" | "spatk" => s.spa = v,
            "spd" | "spdef" => s.spd = v,
            "spe" | "spd." | "speed" => s.spe = v,
            _ => {}
        }
    }
    s
}

fn strip_nature_suffix(line: &str) -> Option<&str> {
    for suf in [" Nature", " nature"] {
        if let Some(idx) = line.find(suf) {
            let head = &line[..idx];
            if !head.is_empty() && head.chars().all(|c| c.is_ascii_alphabetic()) {
                return Some(head);
            }
        }
    }
    None
}

fn normalize_hidden_power(mv: &str) -> String {
    if let Some(rest) = mv.strip_prefix("Hidden Power ") {
        if !rest.starts_with('[') {
            return format!("Hidden Power [{}]", rest);
        }
    }
    mv.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Garchomp @ Life Orb
Ability: Rough Skin
Tera Type: Steel
Level: 50
EVs: 4 HP / 252 Atk / 252 Spe
Adamant Nature
- Earthquake
- Dragon Claw
- Protect
- Iron Head

Amoonguss @ Sitrus Berry
Ability: Regenerator
Level: 50
EVs: 252 HP / 100 Def / 156 SpD
Calm Nature
IVs: 0 Atk
- Spore
- Rage Powder
- Pollen Puff
- Protect
";

    #[test]
    fn parses_two_mons() {
        let v = parse_showdown_export(SAMPLE).unwrap();
        assert_eq!(v.len(), 2);

        let g = &v[0];
        assert_eq!(g.species, "Garchomp");
        assert_eq!(g.item.as_deref(), Some("Life Orb"));
        assert_eq!(g.ability.as_deref(), Some("Rough Skin"));
        assert_eq!(g.level, 50);
        assert_eq!(g.nature, "adamant");
        assert_eq!(g.evs.hp, 4);
        assert_eq!(g.evs.atk, 252);
        assert_eq!(g.evs.spe, 252);
        assert_eq!(g.moves, vec!["Earthquake", "Dragon Claw", "Protect", "Iron Head"]);
        assert_eq!(g.teratype.as_deref(), Some("Steel"));

        let a = &v[1];
        assert_eq!(a.species, "Amoonguss");
        assert_eq!(a.ivs.atk, 0);
        assert_eq!(a.ivs.hp, 31);
        assert_eq!(a.evs.def, 100);
        assert_eq!(a.evs.spd, 156);
    }

    #[test]
    fn handles_nickname_paren() {
        let s = "Reggie (Regieleki) @ Light Clay\nAbility: Transistor\n- Electroweb\n";
        let v = parse_showdown_export(s).unwrap();
        assert_eq!(v[0].species, "Regieleki");
    }

    #[test]
    fn handles_gender_tag() {
        let s = "Garchomp (M) @ Life Orb\nAbility: Rough Skin\n- Earthquake\n";
        let v = parse_showdown_export(s).unwrap();
        assert_eq!(v[0].species, "Garchomp");
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(parse_showdown_export("").unwrap().is_empty());
        assert!(parse_showdown_export("\n\n\n").unwrap().is_empty());
    }

    #[test]
    fn no_item_no_at_sign() {
        let s = "Pikachu\nAbility: Static\n- Thunderbolt\n";
        let v = parse_showdown_export(s).unwrap();
        assert_eq!(v[0].species, "Pikachu");
        assert!(v[0].item.is_none());
    }

    #[test]
    fn hidden_power_bracket_normalized() {
        let s = "Latios @ Choice Specs\nAbility: Levitate\n- Hidden Power Fire\n";
        let v = parse_showdown_export(s).unwrap();
        assert_eq!(v[0].moves[0], "Hidden Power [Fire]");
    }

    #[test]
    fn max_4_moves() {
        let s = "Pikachu\n- a\n- b\n- c\n- d\n- e\n";
        let v = parse_showdown_export(s).unwrap();
        assert_eq!(v[0].moves.len(), 4);
    }
}
