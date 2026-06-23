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

use std::collections::{BTreeMap, BTreeSet};
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

/// One Pokémon Champions mega-forme data correction.
struct MegaFix {
    /// Slugified mega-forme name (matches the pokedex key / `slugify(name)`).
    forme: &'static str,
    /// Corrected slot-0 ability **slug** (already slugified), or "" to leave
    /// the dump's ability untouched. MUST exist in the abilities dump, else the
    /// mega row would be silently dropped at resolve time — all slugs below are
    /// verified present in `~/Dev/localdex/data/abilities.json`.
    ability: &'static str,
    /// Corrected base Attack stat, or 0 to leave the dump's value untouched.
    atk: u8,
}

/// --- Pokémon Champions mega-forme data corrections ---
///
/// The @pkmn / PS dex dump assigns each Champions Mega Evolution its BASE
/// species' ability (e.g. Mega Dragonite gets Inner Focus, not Multiscale)
/// instead of the mega's intended Champions ability, and Mega Starmie's base
/// Attack is wrong (140, should be 100). We do NOT edit the shared upstream
/// JSON (`~/Dev/localdex`, consumed by other tools); instead we override the
/// affected forme rows here, after the dump is read and before the resolve/emit
/// step. Ability fixes are applied in the MEGA_STONES loop; stat fixes in the
/// SPECIES emit loop.
///
/// Megas whose correct Champions ability is a brand-new one absent from the
/// dump — Meganium (Mega Sol), Feraligatr (Dragonize), Excadrill (Piercing
/// Drill), Eelektross (Eelevate), Pyroar (Fire Mane), Scovillain (Spicy Spray)
/// — are now registered in `EXTRA_ABILITIES` above (which gives them ability
/// ids), so their fixes are included at the bottom of this table.
///
/// Source: serebii.net/pokedex-champions/<species>/ (per-forme ability + stats).
const MEGA_FORME_FIXES: &[MegaFix] = &[
    MegaFix { forme: "raichumegax", ability: "electricsurge", atk: 0 },
    MegaFix { forme: "raichumegay", ability: "noguard", atk: 0 },
    MegaFix { forme: "clefablemega", ability: "magicbounce", atk: 0 },
    MegaFix { forme: "dragonitemega", ability: "multiscale", atk: 0 },
    MegaFix { forme: "skarmorymega", ability: "stalwart", atk: 0 },
    MegaFix { forme: "staraptormega", ability: "contrary", atk: 0 },
    MegaFix { forme: "froslassmega", ability: "snowwarning", atk: 0 },
    MegaFix { forme: "emboarmega", ability: "moldbreaker", atk: 0 },
    MegaFix { forme: "scolipedemega", ability: "shellarmor", atk: 0 },
    MegaFix { forme: "scraftymega", ability: "intimidate", atk: 0 },
    MegaFix { forme: "chandeluremega", ability: "infiltrator", atk: 0 },
    MegaFix { forme: "golurkmega", ability: "unseenfist", atk: 0 },
    MegaFix { forme: "floettemega", ability: "fairyaura", atk: 0 },
    MegaFix { forme: "meowsticmmega", ability: "trace", atk: 0 },
    MegaFix { forme: "meowsticfmega", ability: "trace", atk: 0 },
    MegaFix { forme: "dragalgemega", ability: "regenerator", atk: 0 },
    MegaFix { forme: "hawluchamega", ability: "noguard", atk: 0 },
    MegaFix { forme: "crabominablemega", ability: "ironfist", atk: 0 },
    MegaFix { forme: "falinksmega", ability: "defiant", atk: 0 },
    MegaFix { forme: "glimmoramega", ability: "adaptability", atk: 0 },
    MegaFix { forme: "victreebelmega", ability: "innardsout", atk: 0 },
    MegaFix { forme: "starmiemega", ability: "hugepower", atk: 100 },
    // Champions Mega abilities registered in EXTRA_ABILITIES above. These slugs
    // resolve only because we appended them to the ABILITIES table.
    MegaFix { forme: "meganiummega", ability: "megasol", atk: 0 },
    MegaFix { forme: "feraligatrmega", ability: "dragonize", atk: 0 },
    MegaFix { forme: "excadrillmega", ability: "piercingdrill", atk: 0 },
    MegaFix { forme: "eelektrossmega", ability: "eelevate", atk: 0 },
    MegaFix { forme: "pyroarmega", ability: "firemane", atk: 0 },
    MegaFix { forme: "scovillainmega", ability: "spicyspray", atk: 0 },
];

/// One brand-new ability that does NOT exist in the @pkmn / PS dex dump.
struct ExtraAbility {
    /// Slugified ability name (the key used by `MEGA_FORME_FIXES.ability` and
    /// by `ability_by_slug`).
    slug: &'static str,
    /// Display name.
    name: &'static str,
}

/// --- Pokémon Champions Mega abilities absent from the dex dump ---
///
/// The six Champions Mega Evolutions below each gain a brand-new ability that
/// the @pkmn / PS dump has no row for, so they have no `ability_id`. We append
/// them to the `ABILITIES` table AFTER every dump-derived row, so all existing
/// `ability_id::*` indices stay stable and these get fresh trailing ids. Each
/// is then wired to its mega forme in `MEGA_FORME_FIXES` (the forme row would be
/// dropped at resolve time if its ability slug didn't exist — hence registered
/// here first). We do NOT edit the shared upstream JSON (`~/Dev/localdex`).
///
/// Source: serebii.net/pokemonchampions/newabilities.shtml
const EXTRA_ABILITIES: &[ExtraAbility] = &[
    ExtraAbility { slug: "megasol", name: "Mega Sol" },
    ExtraAbility { slug: "dragonize", name: "Dragonize" },
    ExtraAbility { slug: "piercingdrill", name: "Piercing Drill" },
    ExtraAbility { slug: "eelevate", name: "Eelevate" },
    ExtraAbility { slug: "firemane", name: "Fire Mane" },
    ExtraAbility { slug: "spicyspray", name: "Spicy Spray" },
];

/// SCREAMING_SNAKE_CASE-ish Rust identifier for a dex slug, used as the
/// name of a generated id constant (`ability_id::INTIMIDATE`, etc.).
///
/// Slugs are lowercase ASCII alphanumeric only (see `slugify`), so the
/// transform is just an uppercase. Slugs that start with a digit
/// (`10000000voltthunderbolt`) are not valid leading identifier chars, so
/// they are prefixed with `_`. Uppercasing is injective over
/// `[a-z0-9]`, so constant names stay unique within a table.
fn const_ident(slug: &str) -> String {
    let mut out = String::with_capacity(slug.len() + 1);
    if slug.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        out.push('_');
    }
    for c in slug.chars() {
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Emit a `pub mod {mod_name} { pub const NAME: u16 = idx; ... }` block of
/// id constants from a list of `(ident, index)` pairs.
fn emit_id_module(f: &mut fs::File, mod_name: &str, consts: &[(String, usize)]) {
    writeln!(f, "/// Stable table indices for `{mod_name}`, generated alongside the").unwrap();
    writeln!(f, "/// data tables. `TABLE[{mod_name}::NAME as usize].slug` round-trips.").unwrap();
    writeln!(f, "pub mod {mod_name} {{").unwrap();
    for (ident, idx) in consts {
        writeln!(f, "    pub const {ident}: u16 = {idx};").unwrap();
    }
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
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
    /// PS/@pkmn `megaStone`. For a mega stone this is an object mapping the
    /// base species **name** → its Mega forme **name** (e.g. Charizardite X
    /// = `{"Charizard": "Charizard-Mega-X"}`). Null/absent on non-stones.
    /// Multi-key for stones that serve several base formes (Meowstic M/F,
    /// Magearna, Tatsugiri). Drives the `MEGA_STONES` linkage table.
    #[serde(default, rename = "megaStone")]
    mega_stone: Option<BTreeMap<String, String>>,
    /// PS/@pkmn `fling`: `{ basePower, status?, volatileStatus? }`. Present
    /// only on items that can be thrown by the move Fling. Drives the
    /// `fling_bp` / `fling_effect` columns on `ItemDef`.
    #[serde(default)]
    fling: Option<FlingJson>,
    /// PS `isBerry` — true for Berries (Fling makes the TARGET eat them).
    #[serde(default, rename = "isBerry")]
    is_berry: bool,
}

#[derive(Deserialize)]
struct FlingJson {
    #[serde(rename = "basePower")]
    base_power: u32,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "volatileStatus")]
    volatile_status: Option<String>,
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
    /// PS `abilities`: slot key → ability **name** (`"0"`, `"1"`, `"H"`,
    /// `"S"`). Mega/forme transforms read slot `"0"` to set the forme's
    /// fixed ability via the `MEGA_STONES` table. Default empty for forme
    /// stubs that omit it.
    #[serde(default)]
    abilities: BTreeMap<String, String>,
    /// PS `tags` (`data/tags.ts`): species classification labels such as
    /// `"Restricted Legendary"`, `"Mythical"`, `"Paradox"`, `"Sub-Legendary"`,
    /// `"Ultra Beast"`. Surfaced so the format verifier can enforce per-format
    /// species bans without a hand-maintained list. Formes carry the tag
    /// directly (e.g. Calyrex-Ice is tagged `"Restricted Legendary"`).
    #[serde(default)]
    tags: Vec<String>,
    /// PS `prevo`: the species **name** this one evolves from (e.g.
    /// Charizard.prevo = "Charmeleon"). Absent for base-stage species. Drives
    /// the learnset pre-evolution chain-walk — a mon can use any move its
    /// pre-evolutions could learn.
    #[serde(default)]
    prevo: Option<String>,
    /// PS `baseSpecies`: for an alternate forme, the base species **name**
    /// (e.g. Charizard-Mega-X.baseSpecies = "Charizard"). Absent on base
    /// formes. Mega formes have NO learnset of their own, so we merge the base
    /// species' learnset; cosmetic/battle formes likewise share the base pool.
    #[serde(default, rename = "baseSpecies")]
    base_species: Option<String>,
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

/// Per-species learnset as parsed from `learnsets.json`: move-slug → list of
/// source codes (e.g. `["9M", "9L30", "8M"]`). The codes encode generation +
/// method (M=TM/HM, L=level-up, E=egg, T=tutor, S=event, V=Virtual Console,
/// R=reminder/relearn, D=Dream World). We treat a move as legal if it appears
/// under ANY method in ANY gen — a transfer-legal (HOME) VGC validator does not
/// restrict by acquisition method, so the codes themselves are not stored.
type LearnsetMap = BTreeMap<String, BTreeMap<String, Vec<String>>>;

/// Recursively collect every learnable move id for `slug`, walking the
/// pre-evolution (`prevo`) chain and the base-forme (`baseSpecies`) link, and
/// inserting each resolvable move's table index into `out`.
///
/// Mirrors PS `sim/team-validator.ts` `checkCanLearn`, which walks
/// `species.prevo` and falls back to the base species' learnset for formes that
/// have none (megas, cosmetic/battle formes). `visited` guards against cycles
/// and redundant work. Moves not present in the emitted `MOVES` table (filtered
/// by `keep_gen9`) are skipped — they can never appear in a validated set
/// (unknown moves are rejected earlier by the verifier).
fn collect_learnset(
    slug: &str,
    species: &BTreeMap<String, SpeciesJson>,
    learnsets: &LearnsetMap,
    move_slug_to_idx: &BTreeMap<String, usize>,
    out: &mut BTreeSet<u16>,
    visited: &mut BTreeSet<String>,
) {
    if !visited.insert(slug.to_string()) {
        return;
    }
    if let Some(ls) = learnsets.get(slug) {
        for mv in ls.keys() {
            if let Some(&idx) = move_slug_to_idx.get(mv.as_str()) {
                out.insert(idx as u16);
            }
        }
    }
    if let Some(sp) = species.get(slug) {
        if let Some(prevo) = &sp.prevo {
            collect_learnset(&slugify(prevo), species, learnsets, move_slug_to_idx, out, visited);
        }
        if let Some(base) = &sp.base_species {
            collect_learnset(&slugify(base), species, learnsets, move_slug_to_idx, out, visited);
        }
    }
}

fn main() {
    let dex = dex_dir();
    println!("cargo:rerun-if-env-changed=VGC_DEX_DIR");
    for f in ["moves.json", "abilities.json", "items.json", "pokedex.json", "typechart.json", "learnsets.json"] {
        let p = dex.join(f);
        println!("cargo:rerun-if-changed={}", p.display());
    }

    let moves: BTreeMap<String, MoveJson> = read_json(&dex.join("moves.json"));
    let abilities: BTreeMap<String, AbilityJson> = read_json(&dex.join("abilities.json"));
    let items: BTreeMap<String, ItemJson> = read_json(&dex.join("items.json"));
    let species: BTreeMap<String, SpeciesJson> = read_json(&dex.join("pokedex.json"));
    let typechart: BTreeMap<String, TypeEntry> = read_json(&dex.join("typechart.json"));
    let learnsets: LearnsetMap = read_json(&dex.join("learnsets.json"));

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
    writeln!(f, "    /// PS `flags.slicing = 1`. Boosted by Sharpness (×1.5).").unwrap();
    writeln!(f, "    /// Gen-9 set: Slash, Cut, Air Cutter, Aerial Ace, Night Slash,").unwrap();
    writeln!(f, "    /// Psycho Cut, Sacred Sword, X-Scissor, Leaf Blade, Cross Poison,").unwrap();
    writeln!(f, "    /// Air Slash, Razor Shell, Bitter Blade, Kowtow Cleave, Ceaseless").unwrap();
    writeln!(f, "    /// Edge, Stone Axe, Population Bomb, Aqua Cutter, etc.").unwrap();
    writeln!(f, "    pub is_slicing: bool,").unwrap();
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
    writeln!(f, "    /// PS `flags.sound = 1`. Sound-based move (Hyper Voice, Boomburst,").unwrap();
    writeln!(f, "    /// Bug Buzz, Snarl, Round, Clanging Scales, Disarming Voice, etc.).").unwrap();
    writeln!(f, "    /// Disabled for 2 turns by Throat Chop; boosted by Punk Rock / Liquid").unwrap();
    writeln!(f, "    /// Voice; bypasses Substitute; ignored by Soundproof.").unwrap();
    writeln!(f, "    pub is_sound: bool,").unwrap();
    writeln!(f, "    /// PS `flags.heal = 1`. Blocked by Heal Block.").unwrap();
    writeln!(f, "    pub is_heal: bool,").unwrap();
    writeln!(f, "    /// PS `flags.reflectable = 1`. The move can be bounced back at").unwrap();
    writeln!(f, "    /// its user by Magic Coat (move) / Magic Bounce (ability). Set on").unwrap();
    writeln!(f, "    /// status moves that target a foe — entry hazards (Toxic Spikes,").unwrap();
    writeln!(f, "    /// Spikes, Stealth Rock, Sticky Web), status infliction (Thunder").unwrap();
    writeln!(f, "    /// Wave, Toxic, Will-O-Wisp, Spore), Leech Seed, Taunt, etc. The").unwrap();
    writeln!(f, "    /// canonical reflect-eligibility predicate the Magic Coat / Magic").unwrap();
    writeln!(f, "    /// Bounce PRs read; damaging moves are never reflectable.").unwrap();
    writeln!(f, "    pub is_reflectable: bool,").unwrap();
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
    let mut move_consts: Vec<(String, usize)> = Vec::new();
    // slug → emitted table index, for resolving learnset move ids below.
    let mut move_slug_to_idx: BTreeMap<String, usize> = BTreeMap::new();
    for (slug, m) in &moves_keep {
        // Skip moves whose type isn't in the 18-type set (e.g. "???" placeholder).
        // The constant index must track the emitted-row count, not the
        // position in `moves_keep`, since skipped rows shift later indices.
        let Some(ty) = type_index(&m.type_) else { continue; };
        let move_idx = move_consts.len();
        move_slug_to_idx.insert((*slug).clone(), move_idx);
        move_consts.push((const_ident(slug), move_idx));
        writeln!(
            f,
            "    MoveDef {{ num: {}, name: {}, slug: {}, type_: {}, category: {}, base_power: {}, accuracy: {}, pp: {}, priority: {}, target: {}, has_secondary: {}, has_sheer_force_boost: {}, makes_contact: {}, is_punch: {}, is_bite: {}, is_slicing: {}, is_pulse: {}, is_bullet: {}, is_dance: {}, is_wind: {}, is_powder: {}, is_sound: {}, is_heal: {}, is_reflectable: {}, cannot_use_twice: {}, self_max_hp_recoil_num: {}, self_max_hp_recoil_den: {}, drain_num: {}, drain_den: {}, recoil_num: {}, recoil_den: {}, multihit_min: {}, multihit_max: {}, crit_stage_delta: {} }},",
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
            m.flags.contains_key("slicing"),
            m.flags.contains_key("pulse"),
            m.flags.contains_key("bullet"),
            m.flags.contains_key("dance"),
            m.flags.contains_key("wind"),
            m.flags.contains_key("powder"),
            m.flags.contains_key("sound"),
            m.flags.contains_key("heal"),
            m.flags.contains_key("reflectable"),
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
    emit_id_module(&mut f, "move_id", &move_consts);

    // --- Abilities
    let abilities_keep = keep_gen9(&abilities, |a| a.gen_, |a| a.is_nonstandard.as_deref());
    writeln!(f, "pub struct AbilityDef {{").unwrap();
    writeln!(f, "    pub num: u16,").unwrap();
    writeln!(f, "    pub name: &'static str,").unwrap();
    writeln!(f, "    pub slug: &'static str,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const ABILITIES: &[AbilityDef] = &[").unwrap();
    let mut ability_consts: Vec<(String, usize)> = Vec::new();
    // slug → emitted table index, for resolving mega-forme abilities below.
    let mut ability_slug_to_idx: BTreeMap<String, usize> = BTreeMap::new();
    for (slug, a) in &abilities_keep {
        ability_slug_to_idx.insert((*slug).clone(), ability_consts.len());
        ability_consts.push((const_ident(slug), ability_consts.len()));
        writeln!(
            f,
            "    AbilityDef {{ num: {}, name: {}, slug: {} }},",
            a.num.max(0) as u16,
            rust_str_lit(&a.name),
            rust_str_lit(slug),
        ).unwrap();
    }
    // Brand-new Champions Mega abilities the dump lacks. Appended last so the
    // dump-derived ids above are untouched (these take fresh trailing indices).
    for ea in EXTRA_ABILITIES {
        ability_slug_to_idx.insert(ea.slug.to_string(), ability_consts.len());
        ability_consts.push((const_ident(ea.slug), ability_consts.len()));
        writeln!(
            f,
            "    AbilityDef {{ num: {}, name: {}, slug: {} }},",
            0,
            rust_str_lit(ea.name),
            rust_str_lit(ea.slug),
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();
    emit_id_module(&mut f, "ability_id", &ability_consts);

    // --- Items
    let items_keep = keep_gen9(&items, |i| i.gen_, |i| i.is_nonstandard.as_deref());
    writeln!(f, "pub struct ItemDef {{").unwrap();
    writeln!(f, "    pub num: u16,").unwrap();
    writeln!(f, "    pub name: &'static str,").unwrap();
    writeln!(f, "    pub slug: &'static str,").unwrap();
    writeln!(f, "    /// Fling base power. `255` = the item has no `fling`").unwrap();
    writeln!(f, "    /// field (cannot be thrown). Real BP is 0..=130.").unwrap();
    writeln!(f, "    pub fling_bp: u8,").unwrap();
    writeln!(f, "    /// Fling on-hit effect: 0 none, 1 brn, 2 par, 3 psn,").unwrap();
    writeln!(f, "    /// 4 tox, 5 flinch.").unwrap();
    writeln!(f, "    pub fling_effect: u8,").unwrap();
    writeln!(f, "    /// PS `isBerry` — Fling makes the target eat it.").unwrap();
    writeln!(f, "    pub is_berry: bool,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const ITEMS: &[ItemDef] = &[").unwrap();
    let mut item_consts: Vec<(String, usize)> = Vec::new();
    // slug → emitted table index, for the mega-stone linkage table below.
    let mut item_slug_to_idx: BTreeMap<String, usize> = BTreeMap::new();
    for (slug, i) in &items_keep {
        item_slug_to_idx.insert((*slug).clone(), item_consts.len());
        item_consts.push((const_ident(slug), item_consts.len()));
        let (fling_bp, fling_effect) = match &i.fling {
            None => (255u16, 0u8),
            Some(fl) => {
                let eff = match (fl.status.as_deref(), fl.volatile_status.as_deref()) {
                    (Some("brn"), _) => 1,
                    (Some("par"), _) => 2,
                    (Some("psn"), _) => 3,
                    (Some("tox"), _) => 4,
                    (_, Some("flinch")) => 5,
                    _ => 0,
                };
                (fl.base_power.min(254) as u16, eff)
            }
        };
        writeln!(
            f,
            "    ItemDef {{ num: {}, name: {}, slug: {}, fling_bp: {}, fling_effect: {}, is_berry: {} }},",
            i.num.max(0) as u16,
            rust_str_lit(&i.name),
            rust_str_lit(slug),
            fling_bp,
            fling_effect,
            i.is_berry,
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();
    emit_id_module(&mut f, "item_id", &item_consts);

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
    writeln!(f, "    /// Legal abilities as `ability_id` table indices: slot 0, slot 1,").unwrap();
    writeln!(f, "    /// hidden (`\"0\"`/`\"1\"`/`\"H\"` in the PS dump). `u16::MAX` marks an").unwrap();
    writeln!(f, "    /// absent slot. The format verifier checks a set's ability against").unwrap();
    writeln!(f, "    /// this; the battle sim does not consult it.").unwrap();
    writeln!(f, "    pub legal_abilities: [u16; 3],").unwrap();
    writeln!(f, "    /// PS `tags` includes \"Restricted Legendary\" — banned in").unwrap();
    writeln!(f, "    /// restricted-free formats (e.g. VGC Reg M-B).").unwrap();
    writeln!(f, "    pub restricted: bool,").unwrap();
    writeln!(f, "    /// PS `tags` includes \"Mythical\".").unwrap();
    writeln!(f, "    pub mythical: bool,").unwrap();
    writeln!(f, "    /// PS `tags` includes \"Paradox\".").unwrap();
    writeln!(f, "    pub paradox: bool,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const SPECIES: &[SpeciesDef] = &[").unwrap();
    let mut species_consts: Vec<(String, usize)> = Vec::new();
    // slug → emitted table index, for the mega-stone linkage table below.
    let mut species_slug_to_idx: BTreeMap<String, usize> = BTreeMap::new();
    // Emitted species slugs in SPECIES-row order, for the learnset pool below.
    let mut emitted_species_slugs: Vec<String> = Vec::new();
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
        // The constant index must track the emitted-row count, not the
        // position in `species_keep`, since bad-type rows are skipped and
        // shift later indices (mirrors the MOVES `move_idx` pattern).
        species_slug_to_idx.insert((*slug).clone(), species_consts.len());
        species_consts.push((const_ident(slug), species_consts.len()));
        emitted_species_slugs.push((*slug).clone());
        let bs = &s.base_stats;
        let clamp = |x: u32| x.min(u8::MAX as u32) as u8;
        // Champions mega-forme base-stat correction (see MEGA_FORME_FIXES);
        // currently only Mega Starmie's Attack (140 → 100).
        let atk = MEGA_FORME_FIXES
            .iter()
            .find(|fx| fx.forme == slug.as_str() && fx.atk != 0)
            .map_or(bs.atk, |fx| fx.atk as u32);
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
        // Legal abilities as ability-table indices (slot 0, 1, hidden). An
        // ability dropped by `keep_gen9`, or an absent slot, becomes u16::MAX.
        let resolve_ability = |key: &str| -> u16 {
            s.abilities
                .get(key)
                .map(|name| slugify(name))
                .and_then(|sl| ability_slug_to_idx.get(&sl).copied())
                .map_or(u16::MAX, |i| i as u16)
        };
        let legal_abilities = [
            resolve_ability("0"),
            resolve_ability("1"),
            resolve_ability("H"),
        ];
        let has_tag = |t: &str| s.tags.iter().any(|x| x == t);
        let restricted = has_tag("Restricted Legendary");
        let mythical = has_tag("Mythical");
        let paradox = has_tag("Paradox");
        writeln!(
            f,
            "    SpeciesDef {{ num: {}, name: {}, slug: {}, types: [{}, {}], num_types: {}, base_stats: [{}, {}, {}, {}, {}, {}], weight_dg: {}, is_nfe: {}, gender: {}, legal_abilities: [{}, {}, {}], restricted: {}, mythical: {}, paradox: {} }},",
            s.num.max(0) as u16,
            rust_str_lit(&s.name),
            rust_str_lit(slug),
            t[0], t[1], nt,
            clamp(bs.hp), clamp(atk), clamp(bs.def), clamp(bs.spa), clamp(bs.spd), clamp(bs.spe),
            weight_dg,
            is_nfe,
            gender,
            legal_abilities[0], legal_abilities[1], legal_abilities[2],
            restricted, mythical, paradox,
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();
    emit_id_module(&mut f, "species_id", &species_consts);

    // --- Learnsets: per-species pool of legal move ids, pre-merged at build
    // time with the pre-evolution chain (`prevo`) and base-forme (`baseSpecies`)
    // learnsets so the runtime check (`species_can_learn`) is a single binary
    // search — no chain-walking, no allocation. Source: `@pkmn/dex`
    // `learnsets.json`; merge logic mirrors PS `sim/team-validator.ts`
    // `checkCanLearn` (prevo walk + base-species fallback). We are intentionally
    // permissive (transfer-legal / HOME): a move counts if it appears under ANY
    // method in ANY gen. Each species' slice is stored sorted for binary search.
    let mut learn_pool: Vec<u16> = Vec::new();
    let mut learn_index: Vec<(usize, usize)> = Vec::new();
    for slug in &emitted_species_slugs {
        let mut set: BTreeSet<u16> = BTreeSet::new();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        collect_learnset(slug, &species, &learnsets, &move_slug_to_idx, &mut set, &mut visited);
        let off = learn_pool.len();
        learn_pool.extend(set.iter().copied());
        learn_index.push((off, learn_pool.len() - off));
    }
    writeln!(f, "/// Flat pool of legal move ids. Each species' slice is sorted (for").unwrap();
    writeln!(f, "/// binary search) and pre-merged with its pre-evolution + base-forme").unwrap();
    writeln!(f, "/// learnsets. Indexed via `LEARNSET_INDEX`. See build.rs for derivation.").unwrap();
    writeln!(f, "pub const LEARNSET_POOL: &[u16] = &[").unwrap();
    for chunk in learn_pool.chunks(24) {
        write!(f, "    ").unwrap();
        for id in chunk {
            write!(f, "{}, ", id).unwrap();
        }
        writeln!(f).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "/// `(offset, len)` into `LEARNSET_POOL`, indexed by `species_id`.").unwrap();
    writeln!(f, "/// A species' legal moves are `LEARNSET_POOL[offset..offset+len]`.").unwrap();
    writeln!(f, "pub const LEARNSET_INDEX: &[(u32, u32)] = &[").unwrap();
    for (off, len) in &learn_index {
        writeln!(f, "    ({}, {}),", off, len).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();

    // --- Mega Evolution linkage: stone item → (base species, mega forme,
    // mega forme's ability). Built from PS/@pkmn `item.megaStone`, which maps
    // base-species name → mega-forme name. A holder may Mega Evolve iff it
    // holds the stone AND its species matches `base_species_id`; on transform
    // the engine `set_forme`s to `mega_species_id` (recomputing stats from the
    // forme's base stats + the mon's own EVs/IVs/nature) and overrides ability
    // to `mega_ability_id` (the forme's slot-0 ability). One row per
    // base→forme; stones serving several base formes (Meowstic M/F, Magearna,
    // Tatsugiri) emit multiple rows. Linkage is data-driven: any stone+base+
    // forme that survives `keep_gen9` is included.
    writeln!(f, "/// One mega-stone → forme linkage. See build.rs for derivation.").unwrap();
    writeln!(f, "#[derive(Clone, Copy, Debug)]").unwrap();
    writeln!(f, "pub struct MegaStone {{").unwrap();
    writeln!(f, "    /// `item_id` of the mega stone the holder must carry.").unwrap();
    writeln!(f, "    pub item_id: u16,").unwrap();
    writeln!(f, "    /// `species_id` the holder must be to use this stone.").unwrap();
    writeln!(f, "    pub base_species_id: u16,").unwrap();
    writeln!(f, "    /// `species_id` to `set_forme` to on Mega Evolution.").unwrap();
    writeln!(f, "    /// `ability_id` the mega forme gains (PS slot-0 ability).").unwrap();
    writeln!(f, "    pub mega_species_id: u16,").unwrap();
    writeln!(f, "    pub mega_ability_id: u16,").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "pub const MEGA_STONES: &[MegaStone] = &[").unwrap();
    let mut mega_rows: Vec<(usize, usize, usize, usize)> = Vec::new();
    for (slug, i) in &items_keep {
        let Some(stone) = &i.mega_stone else { continue };
        let Some(&item_idx) = item_slug_to_idx.get(*slug) else { continue };
        for (base_name, forme_name) in stone {
            let base_slug = slugify(base_name);
            let forme_slug = slugify(forme_name);
            let (Some(&base_idx), Some(&forme_idx)) = (
                species_slug_to_idx.get(&base_slug),
                species_slug_to_idx.get(&forme_slug),
            ) else {
                continue;
            };
            // Mega forme's slot-0 ability, resolved to an ability index. A
            // Champions correction (MEGA_FORME_FIXES) overrides the dump's
            // (wrong, base-species) ability when present; else fall back to the
            // dump's slot-0 ability.
            let ability_slug = MEGA_FORME_FIXES
                .iter()
                .find(|fx| forme_slug == fx.forme && !fx.ability.is_empty())
                .map(|fx| fx.ability.to_string())
                .or_else(|| {
                    species
                        .get(&forme_slug)
                        .and_then(|sp| sp.abilities.get("0"))
                        .map(|a| slugify(a))
                });
            let ability_idx =
                ability_slug.and_then(|a_slug| ability_slug_to_idx.get(&a_slug).copied());
            let Some(ability_idx) = ability_idx else { continue };
            mega_rows.push((item_idx, base_idx, forme_idx, ability_idx));
        }
    }
    mega_rows.sort_unstable();
    for (item_idx, base_idx, forme_idx, ability_idx) in &mega_rows {
        writeln!(
            f,
            "    MegaStone {{ item_id: {item_idx}, base_species_id: {base_idx}, mega_species_id: {forme_idx}, mega_ability_id: {ability_idx} }},",
        ).unwrap();
    }
    writeln!(f, "];").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "/// Mega-evolution linkage for a holder: the row whose stone the").unwrap();
    writeln!(f, "/// mon holds AND whose `base_species_id` matches its species, if any.").unwrap();
    writeln!(f, "/// Linear scan over the small `MEGA_STONES` table (alloc-free).").unwrap();
    writeln!(f, "pub fn mega_stone_for(item_id: u16, species_id: u16) -> Option<&'static MegaStone> {{").unwrap();
    writeln!(f, "    MEGA_STONES.iter().find(|m| m.item_id == item_id && m.base_species_id == species_id)").unwrap();
    writeln!(f, "}}").unwrap();
    writeln!(f).unwrap();

    // Quick slug lookup helpers — linear scan. Phase 4 may swap to perfect hash.
    writeln!(f).unwrap();
    writeln!(f, "pub fn move_by_slug(s: &str) -> Option<&'static MoveDef> {{ MOVES.iter().find(|m| m.slug == s) }}").unwrap();
    writeln!(f, "pub fn ability_by_slug(s: &str) -> Option<&'static AbilityDef> {{ ABILITIES.iter().find(|a| a.slug == s) }}").unwrap();
    writeln!(f, "pub fn item_by_slug(s: &str) -> Option<&'static ItemDef> {{ ITEMS.iter().find(|i| i.slug == s) }}").unwrap();
    writeln!(f, "pub fn species_by_slug(s: &str) -> Option<&'static SpeciesDef> {{ SPECIES.iter().find(|s2| s2.slug == s) }}").unwrap();

    // Suppress unused-warning for slugify in tests:
    let _ = slugify("x");
}
