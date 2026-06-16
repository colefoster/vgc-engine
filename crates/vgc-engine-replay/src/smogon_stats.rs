//! Smogon usage-statistics parser.
//!
//! Source: <https://www.smogon.com/stats/YYYY-MM/moveset/<format>-<cutoff>.txt>
//!
//! Each species is one block delimited by `+----...----+`. Inside the block,
//! sections (`Abilities`, `Items`, `Spreads`, `Moves`, `Teammates`,
//! `Checks and Counters`) each contain pipe-delimited rows of
//! `<name> <percent>%`. Spreads are formatted as
//! `<Nature>:<h>/<a>/<d>/<sa>/<sd>/<sp> <percent>%` where each stat number is
//! the EV bucket — the nominal "EVs ÷ 8" used by the Smogon stats aggregator.
//!
//! ## Spread scaling
//!
//! The six numbers in a spread are EV buckets in steps of 8: `n` represents
//! `min(n * 8, 252)` EVs. Worked example for Basculegion's top spread
//! `Jolly:0/32/2/0/0/32` (12.449% at 1760):
//!
//! * Atk bucket 32 → `32 * 8 = 256`, capped at 252 → 252 Atk EVs.
//! * Def bucket  2 → `2 * 8  = 16`                  → 16  Def EVs.
//! * Spe bucket 32 → capped                          → 252 Spe EVs.
//! * Total ≈ 520 nominal, capped to ≤508 in practice.
//!
//! That matches the canonical "0 HP / 252+ Atk / 16 Def / 0 / 0 / 252 Spe"
//! Jolly Basculegion build that dominates the 1760 ladder.
//!
//! ## Display-name → slug
//!
//! All names (species, items, abilities, moves, natures) are lowercased and
//! stripped of non-alphanumerics to match `vgc-engine-data`'s slug
//! convention. `Charizard-Mega-Y` → `charizardmegay`,
//! `Choice Scarf` → `choicescarf`, `Mold Breaker` → `moldbreaker`.

use serde::{Deserialize, Serialize};

use vgc_engine_core::StatSpread;

/// One Pokémon's aggregated usage profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpeciesUsage {
    pub species: String,
    pub raw_count: u64,
    /// `(ability_slug, weight_pct)`, sorted descending by weight.
    pub abilities: Vec<(String, f32)>,
    /// `(item_slug, weight_pct)`. May include `"other"` for the catch-all bucket.
    pub items: Vec<(String, f32)>,
    /// `(nature_slug, EV-spread, weight_pct)`. May include a synthetic
    /// `("serious", 0/0/0/0/0/0, pct)` entry for the catch-all "Other" line.
    pub spreads: Vec<(String, StatSpread, f32)>,
    /// `(move_slug, weight_pct)`. Smogon-style "% of sets that include this
    /// move" — sums to ~400% across the 4 slots.
    pub moves: Vec<(String, f32)>,
}

/// Full parsed moveset file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SmogonStats {
    pub species: Vec<SpeciesUsage>,
}

impl SmogonStats {
    pub fn by_species(&self, slug: &str) -> Option<&SpeciesUsage> {
        self.species.iter().find(|s| s.species == slug)
    }
}

#[derive(Debug)]
pub enum ParseError {
    /// Malformed header / no species blocks present.
    Empty,
}

impl core::fmt::Display for ParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "smogon stats: no species blocks parsed"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse a Smogon moveset stats file.
///
/// Tolerates the trailing `Checks and Counters` and `Teammates` sections by
/// ignoring sections we don't recognize.
pub fn parse(text: &str) -> Result<SmogonStats, ParseError> {
    let mut out = SmogonStats::default();
    let mut current: Option<SpeciesUsage> = None;
    // None = between sections; Some(name) = inside named section.
    let mut section: Option<&'static str> = None;

    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with("+----") {
            // Block separator. The pattern is:
            //   +---+
            //   | <species name> |
            //   +---+
            //   | Raw count: ... |
            //   | Avg. weight: ... |
            //   | Viability Ceiling: ... |
            //   +---+
            //   | Abilities |
            //   | <ability> <pct>% |
            //   +---+
            //   ...
            //   +---+
            // We don't need to track separator state explicitly; section
            // headers ("Abilities", "Items", …) flip `section` directly.
            section = None;
            continue;
        }
        let Some(inner) = strip_pipes(line) else {
            continue;
        };
        let inner = inner.trim();

        // Section header (e.g. "Abilities") — single word/phrase, no percent.
        match inner {
            "Abilities" => { section = Some("abilities"); continue; }
            "Items" => { section = Some("items"); continue; }
            "Spreads" => { section = Some("spreads"); continue; }
            "Moves" => { section = Some("moves"); continue; }
            "Teammates" => { section = Some("teammates"); continue; }
            "Checks and Counters" => { section = Some("checks"); continue; }
            _ => {}
        }

        // Block metadata lines.
        if let Some(rest) = inner.strip_prefix("Raw count:") {
            if let Some(c) = current.as_mut() {
                c.raw_count = rest.trim().parse().unwrap_or(0);
            }
            continue;
        }
        if inner.starts_with("Avg. weight:") || inner.starts_with("Viability Ceiling:") {
            continue;
        }

        // No percent → this is the species-name line at block top.
        if !inner.contains('%') {
            // Flush previous block.
            if let Some(prev) = current.take() {
                out.species.push(prev);
            }
            current = Some(SpeciesUsage {
                species: slugify(inner),
                ..SpeciesUsage::default()
            });
            continue;
        }

        // Data row: "<name> <pct>%". Split on the last space before the
        // percent — names contain spaces (Choice Scarf, Iron Head, etc.).
        let Some(c) = current.as_mut() else { continue };
        let Some((name, pct)) = split_pct(inner) else { continue };

        match section {
            Some("abilities") => {
                c.abilities.push((slugify(name), pct));
            }
            Some("items") => {
                c.items.push((slugify(name), pct));
            }
            Some("moves") => {
                if name == "Other" || name.is_empty() {
                    continue;
                }
                c.moves.push((slugify(name), pct));
            }
            Some("spreads") => {
                if let Some((nat, spread)) = parse_spread(name) {
                    c.spreads.push((nat, spread, pct));
                } else if name == "Other" {
                    c.spreads.push((
                        "serious".to_string(),
                        StatSpread { hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 },
                        pct,
                    ));
                }
            }
            _ => {
                // Teammates / Checks — ignore for recon.
            }
        }
    }

    if let Some(prev) = current.take() {
        out.species.push(prev);
    }
    if out.species.is_empty() {
        return Err(ParseError::Empty);
    }
    Ok(out)
}

/// `"| foo bar |"` → `Some("foo bar")`. Returns None for non-pipe lines.
fn strip_pipes(line: &str) -> Option<&str> {
    let s = line.strip_prefix('|')?.strip_suffix('|')?;
    Some(s)
}

/// `"Choice Scarf 35.006"` → `Some(("Choice Scarf", 35.006))`.
fn split_pct(s: &str) -> Option<(&str, f32)> {
    let s = s.trim();
    let s = s.strip_suffix('%')?;
    let (name, num) = s.rsplit_once(' ')?;
    let pct = num.trim().parse::<f32>().ok()?;
    Some((name.trim(), pct))
}

/// `"Jolly:0/32/2/0/0/32"` → `Some(("jolly", spread))`. Returns None for
/// "Other" or malformed input. Each bucket `n` is converted to `min(n*8, 252)`
/// EVs (see the module-level scaling note).
fn parse_spread(s: &str) -> Option<(String, StatSpread)> {
    let (nature, stats) = s.split_once(':')?;
    let parts: Vec<&str> = stats.split('/').collect();
    if parts.len() != 6 {
        return None;
    }
    let ev = |i: usize| -> u8 {
        let n: u16 = parts[i].trim().parse().unwrap_or(0);
        let v = n.saturating_mul(8);
        if v > 252 { 252 } else { v as u8 }
    };
    Some((
        slugify(nature),
        StatSpread {
            hp: ev(0),
            atk: ev(1),
            def: ev(2),
            spa: ev(3),
            spd: ev(4),
            spe: ev(5),
        },
    ))
}

fn slugify(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
+----------------------------------------+
| Basculegion                            |
+----------------------------------------+
| Raw count: 1749052                     |
| Avg. weight: 0.005804443290610284      |
| Viability Ceiling: 87                  |
+----------------------------------------+
| Abilities                              |
| Adaptability 93.248%                   |
| Swift Swim 6.612%                      |
| Mold Breaker 0.140%                    |
+----------------------------------------+
| Items                                  |
| Choice Scarf 35.006%                   |
| Focus Sash 32.651%                     |
| Other 4.728%                           |
+----------------------------------------+
| Spreads                                |
| Jolly:0/32/2/0/0/32 12.449%            |
| Adamant:2/32/0/0/0/32 12.134%          |
| Other 46.233%                          |
+----------------------------------------+
| Moves                                  |
| Last Respects 99.701%                  |
| Aqua Jet 96.520%                       |
| Wave Crash 70.000%                     |
| Protect 60.000%                        |
+----------------------------------------+
| Teammates                              |
| Sneasler 50.000%                       |
+----------------------------------------+
";

    #[test]
    fn parses_one_species_block() {
        let stats = parse(SAMPLE).unwrap();
        assert_eq!(stats.species.len(), 1);
        let s = &stats.species[0];
        assert_eq!(s.species, "basculegion");
        assert_eq!(s.raw_count, 1_749_052);
    }

    #[test]
    fn abilities_sorted_descending() {
        let stats = parse(SAMPLE).unwrap();
        let s = stats.by_species("basculegion").unwrap();
        assert_eq!(s.abilities[0], ("adaptability".to_string(), 93.248));
        assert_eq!(s.abilities[1].0, "swiftswim");
        assert_eq!(s.abilities[2].0, "moldbreaker");
    }

    #[test]
    fn items_include_choice_scarf_top() {
        let stats = parse(SAMPLE).unwrap();
        let s = stats.by_species("basculegion").unwrap();
        assert_eq!(s.items[0].0, "choicescarf");
        // The "Other" bucket is preserved verbatim (slug `other`); recon
        // strategies must skip it when picking a concrete item.
        assert!(s.items.iter().any(|(slug, _)| slug == "other"));
    }

    #[test]
    fn spreads_decode_to_252_atk_252_spe_jolly() {
        // Bucket 32 → 256 → capped 252. Bucket 2 → 16. Bucket 0 → 0.
        let stats = parse(SAMPLE).unwrap();
        let s = stats.by_species("basculegion").unwrap();
        let (nat, spread, pct) = &s.spreads[0];
        assert_eq!(nat, "jolly");
        assert_eq!(spread.atk, 252);
        assert_eq!(spread.def, 16);
        assert_eq!(spread.spe, 252);
        assert_eq!(spread.hp, 0);
        assert_eq!(spread.spa, 0);
        assert_eq!(spread.spd, 0);
        assert!((*pct - 12.449).abs() < 1e-3);
    }

    #[test]
    fn spread_other_bucket_kept_as_serious_zero() {
        let stats = parse(SAMPLE).unwrap();
        let s = stats.by_species("basculegion").unwrap();
        // Last entry should be the "Other" pseudo-spread, weight 46.233.
        let last = s.spreads.last().unwrap();
        assert_eq!(last.0, "serious");
        assert_eq!(last.1.atk, 0);
        assert!((last.2 - 46.233).abs() < 1e-3);
    }

    #[test]
    fn moves_slugified_and_other_skipped() {
        let stats = parse(SAMPLE).unwrap();
        let s = stats.by_species("basculegion").unwrap();
        let slugs: Vec<_> = s.moves.iter().map(|(m, _)| m.as_str()).collect();
        assert!(slugs.contains(&"lastrespects"));
        assert!(slugs.contains(&"aquajet"));
        assert!(slugs.contains(&"wavecrash"));
        assert!(slugs.contains(&"protect"));
    }

    /// Full live file parses, all top-meta species are present, and the
    /// slugs they parse to actually exist in `vgc-engine-data`.
    #[test]
    fn live_file_parses() {
        const LIVE: &str = include_str!(
            "../../../data/smogon-stats/2026-05/gen9championsvgc2026regma-1760.txt"
        );
        let stats = parse(LIVE).unwrap();
        assert!(stats.species.len() > 100, "expected hundreds of species, got {}", stats.species.len());

        // Top-meta species must be present and resolve in the dex.
        for slug in ["basculegion", "kingambit", "garchomp", "charizardmegay", "sneasler", "incineroar"] {
            let s = stats.by_species(slug).unwrap_or_else(|| panic!("missing {slug}"));
            assert!(
                vgc_engine_data::species_by_slug(&s.species).is_some(),
                "species slug {} not in dex",
                s.species
            );
            assert!(!s.abilities.is_empty(), "{slug} has no abilities");
            assert!(!s.items.is_empty(), "{slug} has no items");
            assert!(!s.moves.is_empty(), "{slug} has no moves");
        }
    }
}
