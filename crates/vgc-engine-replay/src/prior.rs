//! Usage-statistics prior for [`crate::position`]: per species, the
//! probability of each item, ability, move and spread.
//!
//! Sources:
//!
//! * the embedded default, Smogon's `gen9championsvgc2026regmb` chaos stats
//!   for 2026-08, cutoff 0 (`data/smogon-stats/2026-08/gen9championsvgc2026regmb-0.json`,
//!   extracted from Bring Four's `smogon.py` output: top 10 per category);
//! * a Bring Four / chaos-derived JSON file of the same shape (any cutoff:
//!   a `cutoffs` map takes cutoff `0`, else the lowest);
//! * a Smogon moveset `.txt` file (parsed by [`crate::smogon_stats`]).
//!
//! Probabilities are the stats' percentages / 100. Moves are "share of sets
//! with the move" (they sum to about 4).

use std::collections::HashMap;

use serde_json::Value;

use crate::smogon_stats;

const EMBEDDED: &str = include_str!("../../../data/smogon-stats/2026-08/gen9championsvgc2026regmb-0.json");

/// One spread: nature slug, six stat values, probability. `sp` = the values
/// are Champions stat points (0-32), else EVs.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorSpread {
    pub nature: String,
    pub values: [u16; 6],
    pub sp: bool,
    pub p: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpeciesPrior {
    /// Share of teams with the species (0-1); 0 when the source has none.
    pub usage: f64,
    /// `(slug, p)`, most likely first.
    pub items: Vec<(String, f64)>,
    pub abilities: Vec<(String, f64)>,
    pub moves: Vec<(String, f64)>,
    pub spreads: Vec<PriorSpread>,
}

#[derive(Debug, Clone, Default)]
pub struct Prior {
    /// Where it came from (`embedded:...`, a path, or `none`).
    pub source: String,
    species: HashMap<String, SpeciesPrior>,
}

pub fn slug(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

fn pairs(v: Option<&Value>) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = v
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let x = x.as_array()?;
                    let name = slug(x.first()?.as_str()?);
                    let p = x.get(1)?.as_f64()? / 100.0;
                    (!name.is_empty() && name != "other" && name != "nothing").then_some((name, p))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

impl Prior {
    /// No prior: every hidden field falls back to the documented defaults.
    pub fn none() -> Self {
        Prior { source: "none".into(), species: HashMap::new() }
    }

    /// The embedded default (Reg M-B 2026-08, cutoff 0).
    pub fn embedded() -> Self {
        let mut p = Self::from_json_text(EMBEDDED).expect("embedded prior parses");
        p.source = "embedded:smogon gen9championsvgc2026regmb 2026-08 cutoff 0".into();
        p
    }

    /// `"default"` / `""` = embedded, `"none"` = no prior, else a file path
    /// (`.json` Bring Four / chaos-derived, else a Smogon moveset `.txt`).
    pub fn load(spec: &str) -> Result<Self, String> {
        match spec {
            "" | "default" | "embedded" => return Ok(Self::embedded()),
            "none" => return Ok(Self::none()),
            _ => {}
        }
        let text = std::fs::read_to_string(spec).map_err(|e| format!("prior {spec}: {e}"))?;
        let mut p = if text.trim_start().starts_with('{') { Self::from_json_text(&text)? } else { Self::from_moveset_txt(&text)? };
        p.source = spec.to_string();
        Ok(p)
    }

    pub fn from_json_text(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("prior json: {e}"))?;
        let block = if let Some(c) = v.get("cutoffs").and_then(Value::as_object) {
            let key = if c.contains_key("0") { "0".to_string() } else { c.keys().min_by_key(|k| k.parse::<i64>().unwrap_or(i64::MAX)).cloned().unwrap_or_default() };
            c.get(&key).cloned().unwrap_or(Value::Null)
        } else {
            v.clone()
        };
        let sp_obj = block.get("species").and_then(Value::as_object).ok_or("prior json: no `species` map")?;
        let mut species = HashMap::new();
        for (name, e) in sp_obj {
            let mut spreads = Vec::new();
            for s in e.get("spreads").and_then(Value::as_array).into_iter().flatten() {
                let nature = slug(s.get("nature").and_then(Value::as_str).unwrap_or(""));
                let (txt, sp) = match (s.get("stat_points").and_then(Value::as_str), s.get("evs").and_then(Value::as_str)) {
                    (Some(t), _) => (t, true),
                    (None, Some(t)) => (t, false),
                    _ => continue,
                };
                let nums: Vec<u16> = txt.split('/').filter_map(|x| x.trim().parse().ok()).collect();
                if nums.len() != 6 || nature.is_empty() {
                    continue;
                }
                let p = s.get("pct").and_then(Value::as_f64).unwrap_or(0.0) / 100.0;
                spreads.push(PriorSpread { nature, values: [nums[0], nums[1], nums[2], nums[3], nums[4], nums[5]], sp, p });
            }
            spreads.sort_by(|a, b| b.p.total_cmp(&a.p));
            species.insert(
                slug(name),
                SpeciesPrior {
                    usage: e.get("usage_pct").and_then(Value::as_f64).unwrap_or(0.0) / 100.0,
                    items: pairs(e.get("items")),
                    abilities: pairs(e.get("abilities")),
                    moves: pairs(e.get("moves")),
                    spreads,
                },
            );
        }
        Ok(Prior { source: "json".into(), species })
    }

    /// A Smogon moveset `.txt`. Its spread numbers are Champions stat points
    /// in Champions formats (the parser's `n * 8` EVs are mapped back).
    pub fn from_moveset_txt(text: &str) -> Result<Self, String> {
        let st = smogon_stats::parse(text).map_err(|e| e.to_string())?;
        let total: f64 = st.species.iter().map(|s| s.raw_count as f64).sum::<f64>().max(1.0);
        let mut species = HashMap::new();
        for s in st.species {
            let f = |v: &[(String, f32)]| -> Vec<(String, f64)> { v.iter().filter(|(n, _)| n != "other" && n != "nothing").map(|(n, p)| (n.clone(), *p as f64 / 100.0)).collect() };
            let spreads = s
                .spreads
                .iter()
                .filter(|(n, e, _)| !(n == "serious" && e.hp == 0 && e.atk == 0 && e.spe == 0))
                .map(|(n, e, p)| {
                    let back = |x: u8| (x as u16 + 7) / 8;
                    PriorSpread { nature: n.clone(), values: [back(e.hp), back(e.atk), back(e.def), back(e.spa), back(e.spd), back(e.spe)], sp: true, p: *p as f64 / 100.0 }
                })
                .collect();
            species.insert(
                s.species.clone(),
                SpeciesPrior { usage: 4.0 * s.raw_count as f64 / total, items: f(&s.items), abilities: f(&s.abilities), moves: f(&s.moves), spreads },
            );
        }
        Ok(Prior { source: "txt".into(), species })
    }

    /// The prior for a species (display name or slug). A forme the stats
    /// don't list falls back to its base species (`Salamence-Mega` ->
    /// `Salamence`, `Indeedee-F` stays `indeedeef`).
    pub fn get(&self, species: &str) -> Option<&SpeciesPrior> {
        let s = slug(species);
        if let Some(p) = self.species.get(&s) {
            return Some(p);
        }
        for suf in ["mega", "megax", "megay", "megaz"] {
            if let Some(b) = s.strip_suffix(suf) {
                if let Some(p) = self.species.get(b) {
                    return Some(p);
                }
            }
        }
        let base = species.split('-').next().map(slug).unwrap_or_default();
        self.species.get(&base)
    }

    /// The stats keys a mon shown as `species` may be listed under: the
    /// species itself and its Mega formes (stats list a stone holder under
    /// its Mega forme, `Charizard-Mega-Y`). `(key slug, usage)`.
    pub fn keys_for(&self, species: &str) -> Vec<(String, f64)> {
        let s = slug(species);
        let mega = format!("{s}mega");
        let mut out: Vec<(String, f64)> = self.species.iter().filter(|(k, _)| **k == s || k.starts_with(&mega)).map(|(k, v)| (k.clone(), v.usage)).collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.species.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_prior_loads() {
        let p = Prior::embedded();
        let k = p.get("Kingambit").expect("kingambit");
        assert!(k.usage > 0.1);
        assert_eq!(k.abilities[0].0, "defiant");
        assert!(k.spreads[0].sp);
        assert!(p.get("Charizard-Mega-Y").is_some());
        assert!(p.get("Salamence-Mega").is_some());
    }
}
