//! Build-time data generation.
//!
//! Reads `@pkmn/dex` JSON dumps from `$VGC_DEX_DIR` (default
//! `~/Dev/localdex/data`) and emits Rust source for the gen-9 species, move,
//! item, ability, and type-chart tables. The output is `include!`d by
//! `src/lib.rs` at compile time.
//!
//! Sources cited in commits:
//!   @pkmn/dex JSON dump (see ~/Dev/localdex)
//!   Type chart cross-checked against PS `data/typechart.ts`.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const TYPE_NAMES: &[&str] = &[
    "Normal", "Fire", "Water", "Electric", "Grass", "Ice", "Fighting", "Poison",
    "Ground", "Flying", "Psychic", "Bug", "Rock", "Ghost", "Dragon", "Dark",
    "Steel", "Fairy",
];

fn type_index(name: &str) -> Option<usize> {
    TYPE_NAMES.iter().position(|t| t.eq_ignore_ascii_case(name))
}

fn dex_dir() -> PathBuf {
    if let Ok(p) = env::var("VGC_DEX_DIR") {
        return PathBuf::from(p);
    }
    let home = env::var("HOME").expect("HOME unset");
    PathBuf::from(home).join("Dev/localdex/data")
}

fn slugify(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect()
}

fn rust_str_lit(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Deserialize)]
struct MoveJson {
    num: i32,
    name: String,
    #[serde(rename = "type")]
    type_: String,
    category: String,
    #[serde(rename = "basePower")]
    base_power: u32,
    accuracy: serde_json::Value,
    pp: u32,
    #[serde(default)]
    priority: i32,
    target: String,
    #[serde(default, rename = "gen")]
    gen_: u32,
    #[serde(rename = "isNonstandard", default)]
    is_nonstandard: Option<String>,
    /// Presence of a `secondary` object in PS `data/moves.ts`. Drives
    /// Sheer Force eligibility — sheerforce `onModifyMove` deletes the
    /// secondary and applies the BP boost only when one was present.
    #[serde(default)]
    secondary: serde_json::Value,
    /// PS `drain: [num, den]`. Heal `round(damage * num / den)` of the
    /// dealt damage onto the user (gen 5+). Absent on most moves.
    #[serde(default)]
    drain: Option<[u32; 2]>,
    /// PS `recoil: [num, den]`. User takes `round(damage * num / den)`
    /// self-damage from the move (gen 5+). Absent on most moves.
    #[serde(default)]
    recoil: Option<[u32; 2]>,
    /// PS `multihit`: either a fixed integer (Double Hit = 2,
    /// Population Bomb = 10) or a `[min, max]` range (Bullet Seed =
    /// [2, 5]). Null on single-hit moves.
    #[serde(default)]
    multihit: serde_json::Value,
    /// Move flags from PS `data/moves.ts`. We only need a few bits for
    /// gen-9 work so far — `contact` (Rough Skin / Iron Barbs / Rocky
    /// Helmet / Tough Claws / Static / Flame Body / Cute Charm) and
    /// `punch` / `bite` etc. come later as their consumers land.
    #[serde(default)]
    flags: BTreeMap<String, serde_json::Value>,
    /// PS `critRatio` (defaults to 1 in PS). 2 = +1 crit stage
    /// (Slash, Karate Chop, Stone Edge, ...); 3 = +2 (Frost Breath,
    /// Storm Throw — always crit). We store the stage delta
    /// (`critRatio - 1`).
    #[serde(default, rename = "critRatio")]
    crit_ratio: Option<u32>,
}

#[derive(Deserialize)]
struct AbilityJson {
    num: i32,
    name: String,
    #[serde(default, rename = "gen")]
    gen_: u32,
    #[serde(rename = "isNonstandard", default)]
    is_nonstandard: Option<String>,
}

#[derive(Deserialize)]
struct ItemJson {
    num: i32,
    name: String,
    #[serde(default, rename = "gen")]
    gen_: u32,
    #[serde(rename = "isNonstandard", default)]
    is_nonstandard: Option<String>,
}

#[derive(Deserialize)]
struct BaseStats {
    hp: u32,
    atk: u32,
    def: u32,
    spa: u32,
    spd: u32,
    spe: u32,
}

#[derive(Deserialize)]
struct SpeciesJson {
    num: i32,
    name: String,
    types: Vec<String>,
    #[serde(rename = "baseStats")]
    base_stats: BaseStats,
    #[serde(default, rename = "gen")]
    gen_: u32,
    #[serde(rename = "isNonstandard", default)]
    is_nonstandard: Option<String>,
    /// PS `weightkg`. Used by Heat Crash / Heavy Slam / Low Kick / Grass
    /// Knot BP scaling, Sky Drop weight cap, Heavy Metal / Light Metal
    /// ability modifiers. JSON value can be a float (e.g. Joltik 0.6 kg);
    /// emit as decigrams (kg * 10, rounded) so we keep one decimal of
    /// precision in a `u16`. Defaults to 0 when missing (forme stubs).
    #[serde(default, rename = "weightkg")]
    weight_kg: f64,
    /// PS `evos: [string, ...]`. Non-empty iff the species can still
    /// evolve — i.e. is Not Fully Evolved. Eviolite's 1.5× Def/SpD
    /// multiplier reads this. Defaults to empty when absent.
    #[serde(default)]
    evos: Vec<String>,
    /// PS `gender` (`sim/dex-species.ts`): `"M"` = always male,
    /// `"F"` = always female, `"N"` = genderless, absent = the species
    /// has a (possibly skewed) gender ratio and gender is rolled per
    /// individual. PS rolls unspecified gender with `sample(['M','F'])`
    /// — a flat 50/50 `random(2)`, ignoring the numeric `genderRatio`
    /// (which exists only for in-game flavor). We therefore collapse the
    /// ratio to a single `Random` category; the numerator is not stored.
    #[serde(default)]
    gender: Option<String>,
}

#[derive(Deserialize)]
struct TypeEntry {
    #[serde(rename = "damageTaken")]
    damage_taken: BTreeMap<String, i32>,
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> BTreeMap<String, T> {
    let bytes = fs::read(path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("parse {}: {}", path.display(), e))
}

fn category_code(s: &str) -> u8 {
    match s {
        "Physical" => 0,
        "Special" => 1,
        "Status" => 2,
        other => panic!("unknown move category: {other}"),
    }
}

/// Map PS target strings to a stable u8.
/// Per PS `sim/dex-moves.ts` MoveTarget union.
fn target_code(s: &str) -> u8 {
    match s {
        "normal" => 0,
        "self" => 1,
        "adjacentAlly" => 2,
        "adjacentAllyOrSelf" => 3,
        "adjacentFoe" => 4,
        "allAdjacent" => 5,
        "allAdjacentFoes" => 6,
        "allies" => 7,
        "allySide" => 8,
        "allyTeam" => 9,
        "any" => 10,
        "foeSide" => 11,
        "all" => 12,
        "randomNormal" => 13,
        "scripted" => 14,
        // Unknown → 255 (will surface in tests when implemented).
        _ => 255,
    }
}

/// Parse PS `multihit` lower bound. Accepts a number (fixed n hits),
/// a `[min, max]` array (random in range), or null/missing (no
/// multihit). Returns 0 when the move is single-hit.
fn multihit_min(v: &serde_json::Value) -> u8 {
    match v {
        serde_json::Value::Number(n) => n.as_u64().map(|x| x.min(u8::MAX as u64) as u8).unwrap_or(0),
        serde_json::Value::Array(a) => a
            .first()
            .and_then(|x| x.as_u64())
            .map(|x| x.min(u8::MAX as u64) as u8)
            .unwrap_or(0),
        _ => 0,
    }
}
fn multihit_max(v: &serde_json::Value) -> u8 {
    match v {
        serde_json::Value::Number(n) => n.as_u64().map(|x| x.min(u8::MAX as u64) as u8).unwrap_or(0),
        serde_json::Value::Array(a) => a
            .get(1)
            .or_else(|| a.first())
            .and_then(|x| x.as_u64())
            .map(|x| x.min(u8::MAX as u64) as u8)
            .unwrap_or(0),
        _ => 0,
    }
}

/// Accuracy: PS uses `true` for "cannot miss"; encode as 255. Otherwise 0..=100.
fn accuracy_code(v: &serde_json::Value) -> u8 {
    match v {
        serde_json::Value::Bool(true) => 255,
        serde_json::Value::Number(n) => n.as_u64().map(|x| x.min(100) as u8).unwrap_or(0),
        _ => 0,
    }
}

fn keep_gen9<'a, T>(
    map: &'a BTreeMap<String, T>,
    get_gen: impl Fn(&T) -> u32,
    get_ns: impl Fn(&T) -> Option<&str>,
) -> Vec<(&'a String, &'a T)> {
    let mut out: Vec<(&'a String, &'a T)> = map
        .iter()
        .filter(|(_, v)| get_gen(v) <= 9)
        .filter(|(_, v)| {
            // Drop CAP/Pokestar/Custom; keep "Past"/"Unobtainable" so the table mirrors
            // @pkmn/dex coverage. Phase-2 format filtering will narrow further.
            !matches!(get_ns(v), Some("CAP") | Some("Pokestar") | Some("Custom"))
        })
        .collect();
    out.sort_by_key(|(k, _)| k.as_str().to_owned());
    out
}

fn main() {
    let dex = dex_dir();
    println!("cargo:rerun-if-env-changed=VGC_DEX_DIR");
    for f in ["moves.json", "abilities.json", "items.json", "pokedex.json", "typechart.json"] {
        let p = dex.join(f);
        println!("cargo:rerun-if-changed={}", p.display());
    }

    let moves: BTreeMap<String, MoveJson> = read_json(&dex.join("moves.json"));
    let abilities: BTreeMap<String, AbilityJson> = read_json(&dex.join("abilities.json"));
    let items: BTreeMap<String, ItemJson> = read_json(&dex.join("items.json"));
    let species: BTreeMap<String, SpeciesJson> = read_json(&dex.join("pokedex.json"));
    let typechart: BTreeMap<String, TypeEntry> = read_json(&dex.join("typechart.json"));

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR unset");
    let out_path = Path::new(&out_dir).join("data_tables.rs");
    let mut f = fs::File::create(&out_path).expect("create data_tables.rs");

    writeln!(f, "// @generated by build.rs from @pkmn/dex JSON. Do not edit.").unwrap();
    writeln!(f).unwrap();

    // --- Type chart: 18x18 matrix of damage multipliers, packed.
    // PS damageTaken codes: 0=normal(1x), 1=weak(2x), 2=resist(0.5x), 3=immune(0x).
    writeln!(f, "pub const TYPE_NAMES: [&str; 18] = [").unwrap();
    for n in TYPE_NAMES {
        writeln!(f, "    {},", rust_str_lit(n)).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "/// damage_taken[defender][attacker] using PS codes:").unwrap();
    writeln!(f, "/// 0 = 1x, 1 = 2x (weak), 2 = 0.5x (resist), 3 = 0x (immune).").unwrap();
    writeln!(f, "pub const TYPE_CHART: [[u8; 18]; 18] = [").unwrap();
    for def in TYPE_NAMES {
        let entry = typechart
            .get(*def)
            .unwrap_or_else(|| panic!("typechart missing defender type: {def}"));
        write!(f, "    [").unwrap();
        for atk in TYPE_NAMES {
            let code = entry.damage_taken.get(*atk).copied().unwrap_or(0);
            write!(f, "{}, ", code).unwrap();
        }
        writeln!(f, "],").unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();

    // --- Moves
    let moves_keep = keep_gen9(&moves, |m| m.gen_, |m| m.is_nonstandard.as_deref());
    writeln!(f, "#[derive(Clone, Copy)]").unwrap();
    writeln!(f, "pub struct MoveDef {{").unwrap();
    writeln!(f, "    pub num: u16,").unwrap();
    writeln!(f, "    pub name: &'static str,").unwrap();
    writeln!(f, "    pub slug: &'static str,").unwrap();
    writeln!(f, "    pub type_: u8,").unwrap();
    writeln!(f, "    pub category: u8,").unwrap();
    writeln!(f, "    pub base_power: u16,").unwrap();
    writeln!(f, "    pub accuracy: u8,").unwrap();
    writeln!(f, "    pub pp: u8,").unwrap();
    writeln!(f, "    pub priority: i8,").unwrap();
    writeln!(f, "    pub target: u8,").unwrap();
    writeln!(f, "    /// True iff PS `data/moves.ts` carries a `secondary` block.").unwrap();
    writeln!(f, "    /// Source of truth for Sheer Force eligibility.").unwrap();
    writeln!(f, "    pub has_secondary: bool,").unwrap();
    writeln!(f, "    /// True iff the move is in PS's manual `hasSheerForceBoost: true` list").unwrap();
    writeln!(f, "    /// (gen 9: electroshot, orderup). Sheer Force boosts these even with no").unwrap();
    writeln!(f, "    /// `secondary` field. Hardcoded — @pkmn/dex JSON drops the flag.").unwrap();
    writeln!(f, "    pub has_sheer_force_boost: bool,").unwrap();
    writeln!(f, "    /// PS `flags.contact = 1`. Used by Rough Skin / Iron Barbs / Rocky").unwrap();
    writeln!(f, "    /// Helmet / Tough Claws / Static / Flame Body / Cute Charm.").unwrap();
    writeln!(f, "    pub makes_contact: bool,").unwrap();
    writeln!(f, "    /// PS `flags.punch = 1`. Boosted by Iron Fist (×1.2) and Punching").unwrap();
    writeln!(f, "    /// Glove (×1.1, also removes contact); blocked by no abilities.").unwrap();
    writeln!(f, "    pub is_punch: bool,").unwrap();
    writeln!(f, "    /// PS `flags.bite = 1`. Boosted by Strong Jaw (×1.5).").unwrap();
    writeln!(f, "    pub is_bite: bool,").unwrap();
    writeln!(f, "    /// PS `flags.pulse = 1`. Boosted by Mega Launcher (×1.5).").unwrap();
    writeln!(f, "    pub is_pulse: bool,").unwrap();
    writeln!(f, "    /// PS `flags.bullet = 1`. Blocked by Bulletproof; includes ball/").unwrap();
    writeln!(f, "    /// bomb moves (Shadow Ball, Sludge Bomb, Aura Sphere, etc.).").unwrap();
    writeln!(f, "    pub is_bullet: bool,").unwrap();
    writeln!(f, "    /// PS `flags.dance = 1`. Re-triggers Dancer abilities.").unwrap();
    writeln!(f, "    pub is_dance: bool,").unwrap();
    writeln!(f, "    /// PS `flags.wind = 1`. Triggers Wind Power (charge) / Wind Rider").unwrap();
    writeln!(f, "    /// (+1 Atk, immunity) and is redirected by Storm Drain-style hooks.").unwrap();
    writeln!(f, "    /// Gen-9 set: Gust, Whirlwind, Twister, Aeroblast, Air Cutter,").unwrap();
    writeln!(f, "    /// Hurricane, Fairy Wind, Heat Wave, Icy Wind, Blizzard, Petal").unwrap();
    writeln!(f, "    /// Blizzard, Sandstorm, Tailwind, and the four Storm moves.").unwrap();
    writeln!(f, "    pub is_wind: bool,").unwrap();
    writeln!(f, "    /// PS `flags.powder = 1`. Grass types / Safety Goggles / Overcoat").unwrap();
    writeln!(f, "    /// immune. Currently approximated inline; this flag is the").unwrap();
    writeln!(f, "    /// canonical source.").unwrap();
    writeln!(f, "    pub is_powder: bool,").unwrap();
    writeln!(f, "    /// PS `flags.heal = 1`. Blocked by Heal Block.").unwrap();
    writeln!(f, "    pub is_heal: bool,").unwrap();
    writeln!(f, "    /// PS `flags.cantusetwice = 1`. After using the move, the user").unwrap();
    writeln!(f, "    /// cannot select it again on the next turn (Gigaton Hammer,").unwrap();
    writeln!(f, "    /// Blood Moon). Encoded as a per-mon volatile in the move handler.").unwrap();
    writeln!(f, "    pub cannot_use_twice: bool,").unwrap();
    writeln!(f, "    /// Self-recoil expressed as a fraction of user's max HP, taken").unwrap();
    writeln!(f, "    /// unconditionally after a successful hit — independent of the").unwrap();
    writeln!(f, "    /// damage dealt. Steel Beam / Mind Blown / Chloroblast all use").unwrap();
    writeln!(f, "    /// `maxhp / 2` in PS (`mindBlownRecoil: true` + chloroblast id).").unwrap();
    writeln!(f, "    /// 0 = no max-HP recoil. Distinct from `recoil_num/den` which").unwrap();
    writeln!(f, "    /// scales with damage dealt.").unwrap();
    writeln!(f, "    pub self_max_hp_recoil_num: u8,").unwrap();
    writeln!(f, "    pub self_max_hp_recoil_den: u8,").unwrap();
    writeln!(f, "    /// PS `drain: [num, den]` numerator (0 if move does not drain).").unwrap();
    writeln!(f, "    /// Heal `round(damage * num / den)` of damage dealt onto the user.").unwrap();
    writeln!(f, "    pub drain_num: u8,").unwrap();
    writeln!(f, "    /// PS `drain: [num, den]` denominator (1 sentinel when num == 0).").unwrap();
    writeln!(f, "    pub drain_den: u8,").unwrap();
    writeln!(f, "    /// PS `recoil: [num, den]` numerator (0 if no recoil).").unwrap();
    writeln!(f, "    /// User takes `round(damage * num / den)` self-damage after the hit.").unwrap();
    writeln!(f, "    pub recoil_num: u8,").unwrap();
    writeln!(f, "    /// PS `recoil: [num, den]` denominator (1 sentinel when num == 0).").unwrap();
    writeln!(f, "    pub recoil_den: u8,").unwrap();
    writeln!(f, "    /// PS `multihit` lower bound. 0 = single-hit move (no multihit).").unwrap();
    writeln!(f, "    /// Fixed multihit (Population Bomb = 10) has min == max.").unwrap();
    writeln!(f, "    pub multihit_min: u8,").unwrap();
    writeln!(f, "    /// PS `multihit` upper bound. For range multihit (Bullet Seed").unwrap();
    writeln!(f, "    /// = [2, 5]), Splitmix samples uniformly across [min, max]; PS's").unwrap();
    writeln!(f, "    /// 35/35/15/15 weighting for 2-5 hits is approximated as uniform.").unwrap();
    writeln!(f, "    pub multihit_max: u8,").unwrap();
    writeln!(f, "    /// Crit-stage delta from PS `critRatio` (0 = base,").unwrap();
    writeln!(f, "    /// 1 = high-crit-ratio +1, 2 = always-crit +2). Added to").unwrap();
    writeln!(f, "    /// `Pokemon::effective_crit_stage()` at damage time.").unwrap();
    writeln!(f, "    pub crit_stage_delta: u8,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const MOVES: &[MoveDef] = &[").unwrap();
    for (slug, m) in &moves_keep {
        // Skip moves whose type isn't in the 18-type set (e.g. "???" placeholder).
        let Some(ty) = type_index(&m.type_) else { continue; };
        writeln!(
            f,
            "    MoveDef {{ num: {}, name: {}, slug: {}, type_: {}, category: {}, base_power: {}, accuracy: {}, pp: {}, priority: {}, target: {}, has_secondary: {}, has_sheer_force_boost: {}, makes_contact: {}, is_punch: {}, is_bite: {}, is_pulse: {}, is_bullet: {}, is_dance: {}, is_wind: {}, is_powder: {}, is_heal: {}, cannot_use_twice: {}, self_max_hp_recoil_num: {}, self_max_hp_recoil_den: {}, drain_num: {}, drain_den: {}, recoil_num: {}, recoil_den: {}, multihit_min: {}, multihit_max: {}, crit_stage_delta: {} }},",
            m.num.max(0) as u16,
            rust_str_lit(&m.name),
            rust_str_lit(slug),
            ty,
            category_code(&m.category),
            m.base_power.min(u16::MAX as u32) as u16,
            accuracy_code(&m.accuracy),
            m.pp.min(u8::MAX as u32) as u8,
            m.priority.clamp(i8::MIN as i32, i8::MAX as i32) as i8,
            target_code(&m.target),
            !m.secondary.is_null(),
            matches!(slug.as_str(), "electroshot" | "orderup"),
            m.flags.contains_key("contact"),
            m.flags.contains_key("punch"),
            m.flags.contains_key("bite"),
            m.flags.contains_key("pulse"),
            m.flags.contains_key("bullet"),
            m.flags.contains_key("dance"),
            m.flags.contains_key("wind"),
            m.flags.contains_key("powder"),
            m.flags.contains_key("heal"),
            m.flags.contains_key("cantusetwice"),
            // PS hardcodes `mindBlownRecoil: true` on Mind Blown and Steel Beam,
            // and singles out Chloroblast by id in the same `onAfterMove`. All
            // three apply 1/2 max HP. The @pkmn/dex JSON strips the mindBlownRecoil
            // field, so we hardcode the three known slugs here. Last reviewed
            // gen 9 SV (PS data/moves.ts:17890).
            if matches!(slug.as_str(), "steelbeam" | "mindblown" | "chloroblast") { 1u8 } else { 0u8 },
            if matches!(slug.as_str(), "steelbeam" | "mindblown" | "chloroblast") { 2u8 } else { 1u8 },
            m.drain.map(|[n, _]| n.min(u8::MAX as u32) as u8).unwrap_or(0),
            m.drain.map(|[_, d]| d.min(u8::MAX as u32) as u8).unwrap_or(1),
            m.recoil.map(|[n, _]| n.min(u8::MAX as u32) as u8).unwrap_or(0),
            m.recoil.map(|[_, d]| d.min(u8::MAX as u32) as u8).unwrap_or(1),
            multihit_min(&m.multihit),
            multihit_max(&m.multihit),
            m.crit_ratio.map(|r| r.saturating_sub(1).min(2) as u8).unwrap_or(0),
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();

    // --- Abilities
    let abilities_keep = keep_gen9(&abilities, |a| a.gen_, |a| a.is_nonstandard.as_deref());
    writeln!(f, "pub struct AbilityDef {{").unwrap();
    writeln!(f, "    pub num: u16,").unwrap();
    writeln!(f, "    pub name: &'static str,").unwrap();
    writeln!(f, "    pub slug: &'static str,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const ABILITIES: &[AbilityDef] = &[").unwrap();
    for (slug, a) in &abilities_keep {
        writeln!(
            f,
            "    AbilityDef {{ num: {}, name: {}, slug: {} }},",
            a.num.max(0) as u16,
            rust_str_lit(&a.name),
            rust_str_lit(slug),
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();

    // --- Items
    let items_keep = keep_gen9(&items, |i| i.gen_, |i| i.is_nonstandard.as_deref());
    writeln!(f, "pub struct ItemDef {{").unwrap();
    writeln!(f, "    pub num: u16,").unwrap();
    writeln!(f, "    pub name: &'static str,").unwrap();
    writeln!(f, "    pub slug: &'static str,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const ITEMS: &[ItemDef] = &[").unwrap();
    for (slug, i) in &items_keep {
        writeln!(
            f,
            "    ItemDef {{ num: {}, name: {}, slug: {} }},",
            i.num.max(0) as u16,
            rust_str_lit(&i.name),
            rust_str_lit(slug),
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();

    // --- Species
    let species_keep = keep_gen9(&species, |s| s.gen_, |s| s.is_nonstandard.as_deref());
    writeln!(f, "pub struct SpeciesDef {{").unwrap();
    writeln!(f, "    pub num: u16,").unwrap();
    writeln!(f, "    pub name: &'static str,").unwrap();
    writeln!(f, "    pub slug: &'static str,").unwrap();
    writeln!(f, "    pub types: [u8; 2],").unwrap();
    writeln!(f, "    pub num_types: u8,").unwrap();
    writeln!(f, "    pub base_stats: [u8; 6], // hp, atk, def, spa, spd, spe").unwrap();
    writeln!(f, "    /// Species weight in decigrams (kg * 10, rounded). Used by Heat").unwrap();
    writeln!(f, "    /// Crash / Heavy Slam / Low Kick / Grass Knot BP scaling, Sky").unwrap();
    writeln!(f, "    /// Drop weight cap, Heavy Metal / Light Metal modifiers. Stored").unwrap();
    writeln!(f, "    /// as decigrams so single-decimal PS weights (Joltik 0.6 kg = 6)").unwrap();
    writeln!(f, "    /// round-trip without floats. 0 = unknown (forme stubs).").unwrap();
    writeln!(f, "    pub weight_dg: u16,").unwrap();
    writeln!(f, "    /// True iff this species has at least one evolution (Not Fully").unwrap();
    writeln!(f, "    /// Evolved). Read by Eviolite to apply its 1.5× Def/SpD multiplier.").unwrap();
    writeln!(f, "    /// Sourced from PS `evos` array being non-empty.").unwrap();
    writeln!(f, "    pub is_nfe: bool,").unwrap();
    writeln!(f, "    /// Innate gender category (PS `species.gender`). `Random` means").unwrap();
    writeln!(f, "    /// the species has a gender ratio and an individual's gender is").unwrap();
    writeln!(f, "    /// rolled 50/50 at battle construction (PS `sample(['M','F'])`).").unwrap();
    writeln!(f, "    pub gender: Gender,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const SPECIES: &[SpeciesDef] = &[").unwrap();
    for (slug, s) in &species_keep {
        let mut t = [0u8; 2];
        let nt = s.types.len().min(2) as u8;
        let mut bad_type = false;
        for (i, tn) in s.types.iter().take(2).enumerate() {
            match type_index(tn) {
                Some(idx) => t[i] = idx as u8,
                None => { bad_type = true; break; }
            }
        }
        if bad_type { continue; }
        let bs = &s.base_stats;
        let clamp = |x: u32| x.min(u8::MAX as u32) as u8;
        let weight_dg = ((s.weight_kg * 10.0).round().max(0.0)).min(u16::MAX as f64) as u16;
        let is_nfe = !s.evos.is_empty();
        // PS `species.gender`: "M"/"F"/"N" are fixed; absent ⇒ ratio'd
        // (rolled per individual). Unknown values fall back to Random.
        let gender = match s.gender.as_deref() {
            Some("M") => "Gender::Male",
            Some("F") => "Gender::Female",
            Some("N") => "Gender::Genderless",
            _ => "Gender::Random",
        };
        writeln!(
            f,
            "    SpeciesDef {{ num: {}, name: {}, slug: {}, types: [{}, {}], num_types: {}, base_stats: [{}, {}, {}, {}, {}, {}], weight_dg: {}, is_nfe: {}, gender: {} }},",
            s.num.max(0) as u16,
            rust_str_lit(&s.name),
            rust_str_lit(slug),
            t[0], t[1], nt,
            clamp(bs.hp), clamp(bs.atk), clamp(bs.def), clamp(bs.spa), clamp(bs.spd), clamp(bs.spe),
            weight_dg,
            is_nfe,
            gender,
        ).unwrap();
    }
    writeln!(f, "];").unwrap();

    // Quick slug lookup helpers — linear scan. Phase 4 may swap to perfect hash.
    writeln!(f).unwrap();
    writeln!(f, "pub fn move_by_slug(s: &str) -> Option<&'static MoveDef> {{ MOVES.iter().find(|m| m.slug == s) }}").unwrap();
    writeln!(f, "pub fn ability_by_slug(s: &str) -> Option<&'static AbilityDef> {{ ABILITIES.iter().find(|a| a.slug == s) }}").unwrap();
    writeln!(f, "pub fn item_by_slug(s: &str) -> Option<&'static ItemDef> {{ ITEMS.iter().find(|i| i.slug == s) }}").unwrap();
    writeln!(f, "pub fn species_by_slug(s: &str) -> Option<&'static SpeciesDef> {{ SPECIES.iter().find(|s2| s2.slug == s) }}").unwrap();

    // Suppress unused-warning for slugify in tests:
    let _ = slugify("x");
}
