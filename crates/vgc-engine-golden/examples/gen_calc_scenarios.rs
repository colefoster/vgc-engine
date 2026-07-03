//! Generate a matrix of calc-oracle scenario JSONs into
//! `tools/calc-oracle/generated/`. Run once (or after adding cases):
//!
//!     cargo run --release -p vgc-engine-golden --example gen_calc_scenarios
//!
//! The suite auto-discovers everything under `tools/calc-oracle/`; on next
//! `cargo test` the new scenarios' calc expectations are populated via
//! node oracle.js and cached alongside the hand-authored ones.
//!
//! Design: pick a handful of Reg M-B relevant attacker archetypes with a
//! signature move, then cross with modifier axes we know exercise
//! damage-formula corners (item, weather, terrain). Each generated
//! scenario is a single-hit setup the harness already supports; no Tera,
//! no defender item, no spread.

use std::path::PathBuf;

use serde_json::json;

fn oracle_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tools")
        .join("calc-oracle")
        .canonicalize()
        .expect("locate calc-oracle dir")
}

/// One row of the matrix — a single attacker archetype + signature move.
struct Archetype {
    /// Slug fragment for scenario filenames (e.g., "chi-yu-heatwave").
    tag: &'static str,
    attacker_species: &'static str,
    attacker_ability: &'static str,
    attacker_nature: &'static str,
    /// EV keys/values inline as `(k, v)` for JSON construction.
    attacker_evs: &'static [(&'static str, u8)],
    defender_species: &'static str,
    defender_ability: &'static str,
    defender_nature: &'static str,
    defender_evs: &'static [(&'static str, u8)],
    move_name: &'static str,
    /// Items to test on THIS attacker (category-appropriate).
    items: &'static [Option<&'static str>],
}

fn evs_json(evs: &[(&'static str, u8)]) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    for (k, v) in evs {
        m.insert((*k).to_string(), json!(*v));
    }
    serde_json::Value::Object(m)
}

fn main() {
    let out_dir = oracle_dir().join("generated");
    std::fs::create_dir_all(&out_dir).expect("mkdir generated");

    let physical_items: &[Option<&str>] = &[
        None,
        Some("Life Orb"),
        Some("Choice Band"),
        Some("Muscle Band"),
        Some("Expert Belt"),
    ];
    let special_items: &[Option<&str>] = &[
        None,
        Some("Life Orb"),
        Some("Choice Specs"),
        Some("Wise Glasses"),
        Some("Expert Belt"),
    ];

    let archetypes: &[Archetype] = &[
        // Chi-Yu Overheat into Amoonguss — Grass/Poison, 2× Fire → Expert
        // Belt triggers. Chi-Yu's Beads-of-Ruin aura lowers foe SpD (was
        // the bug from #79's harness confound; now safe with the filter).
        Archetype {
            tag: "chiyu-overheat-vs-amoonguss",
            attacker_species: "Chi-Yu",
            attacker_ability: "Beads of Ruin",
            attacker_nature: "Modest",
            attacker_evs: &[("hp", 4), ("spa", 252), ("spe", 252)],
            defender_species: "Amoonguss",
            defender_ability: "Regenerator",
            defender_nature: "Calm",
            defender_evs: &[("hp", 252), ("def", 4), ("spd", 252)],
            move_name: "Overheat",
            items: special_items,
        },
        // Kingambit Iron Head into Flutter Mane — Ghost/Fairy, 2× Steel.
        Archetype {
            tag: "kingambit-ironhead-vs-fluttermane",
            attacker_species: "Kingambit",
            attacker_ability: "Supreme Overlord",
            attacker_nature: "Adamant",
            attacker_evs: &[("hp", 4), ("atk", 252), ("spe", 252)],
            defender_species: "Flutter Mane",
            defender_ability: "Protosynthesis",
            defender_nature: "Timid",
            defender_evs: &[("hp", 252), ("def", 4), ("spd", 252)],
            move_name: "Iron Head",
            items: physical_items,
        },
        // Landorus-Therian Earthquake into Iron Hands — grounded target,
        // 1× (Electric/Fighting, no immunity, no SE). Expert Belt SHOULD
        // NOT trigger here — good control case.
        Archetype {
            tag: "landorus-eq-vs-ironhands",
            attacker_species: "Landorus-Therian",
            attacker_ability: "Intimidate",
            attacker_nature: "Adamant",
            attacker_evs: &[("hp", 4), ("atk", 252), ("spe", 252)],
            defender_species: "Iron Hands",
            defender_ability: "Quark Drive",
            defender_nature: "Careful",
            defender_evs: &[("hp", 252), ("atk", 4), ("spd", 252)],
            move_name: "Earthquake",
            items: physical_items,
        },
    ];

    // Fields to cross — (weather, terrain) pairs. Skip the full 5×5
    // cross; pick the ones that actually alter damage for these moves.
    let fields: &[(Option<&str>, Option<&str>)] = &[
        (None, None),           // clear control
        (Some("Sun"), None),    // Overheat 1.5× (Fire), IronHead unchanged
        (Some("Rain"), None),   // Overheat 0.5×
        (Some("Sand"), None),   // Rock SpD 1.5× (SE-lens for special hits)
        (Some("Snow"), None),   // Ice-type Def 1.5×
        (None, Some("Grassy")), // grounded Grass boost / EQ halve
        (None, Some("Electric")),
        (None, Some("Psychic")),
    ];

    let mut written = 0;
    for arche in archetypes {
        for item in arche.items {
            for (weather, terrain) in fields {
                let item_slug = match item {
                    None => "no-item".to_string(),
                    Some(s) => slugify(s),
                };
                let field_slug = match (weather, terrain) {
                    (None, None) => "clear".to_string(),
                    (Some(w), None) => format!("wx-{}", w.to_lowercase()),
                    (None, Some(t)) => format!("tr-{}", t.to_lowercase()),
                    (Some(w), Some(t)) => format!("wx-{}-tr-{}", w.to_lowercase(), t.to_lowercase()),
                };
                let stem = format!("scenario-gen-{}-{}-{}", arche.tag, item_slug, field_slug);

                let mut attacker = serde_json::Map::new();
                attacker.insert("species".into(), json!(arche.attacker_species));
                attacker.insert("level".into(), json!(50));
                if let Some(i) = item { attacker.insert("item".into(), json!(*i)); }
                attacker.insert("ability".into(), json!(arche.attacker_ability));
                attacker.insert("nature".into(), json!(arche.attacker_nature));
                attacker.insert("evs".into(), evs_json(arche.attacker_evs));

                let mut defender = serde_json::Map::new();
                defender.insert("species".into(), json!(arche.defender_species));
                defender.insert("level".into(), json!(50));
                defender.insert("ability".into(), json!(arche.defender_ability));
                defender.insert("nature".into(), json!(arche.defender_nature));
                defender.insert("evs".into(), evs_json(arche.defender_evs));

                let mut sc = serde_json::Map::new();
                sc.insert("name".into(), json!(stem.clone()));
                sc.insert("attacker".into(), serde_json::Value::Object(attacker));
                sc.insert("defender".into(), serde_json::Value::Object(defender));
                sc.insert("move".into(), json!(arche.move_name));
                sc.insert("trials".into(), json!(500));
                let mut field = serde_json::Map::new();
                if let Some(w) = weather { field.insert("weather".into(), json!(*w)); }
                if let Some(t) = terrain { field.insert("terrain".into(), json!(*t)); }
                if !field.is_empty() {
                    sc.insert("field".into(), serde_json::Value::Object(field));
                }

                let path = out_dir.join(format!("{stem}.json"));
                let json_str = serde_json::to_string_pretty(&serde_json::Value::Object(sc))
                    .expect("serialize scenario");
                std::fs::write(&path, json_str).expect("write scenario");
                written += 1;
            }
        }
    }

    println!("wrote {written} scenarios to {}", out_dir.display());
}

fn slugify(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
