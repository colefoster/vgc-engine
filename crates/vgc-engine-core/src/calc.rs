//! Fast damage-calc API — a terse, alias-friendly front end over
//! [`crate::damage_only`].
//!
//! This is the engine-side half of the `vgc calc chomp lando eq` CLI:
//! a [`QuickMon`] describes an attacker/defender with VGC defaults
//! (level 50, neutral nature, 0 EV / 31 IV, no item, species primary
//! ability), a [`Field`] carries weather/terrain/spread, and
//! [`calc`] returns a [`DamageResult`] — the 16 damage rolls plus
//! range, percentages, and a KO estimate.
//!
//! `QuickMon` builds a [`crate::pokemon::Pokemon`] through the exact
//! same [`crate::build_member`] path the calc-oracle uses
//! (`calc_oracle.rs::observe_scenario`), then hands the pair to
//! `damage_only`. No new damage math lives here — this module is pure
//! input parsing + result shaping.
//!
//! Aliases: species and moves resolve through, in order, (1) an exact
//! dex slug, (2) a curated alias table ([`aliases`]), (3) a `slugify`
//! prefix match against the dex. Ambiguous prefixes error with a
//! "did you mean" list.

use crate::pokemon::{nature_by_slug, Status};
use crate::team::{build_member, slugify, TeamMember};
use crate::{damage_only, DamageQuery, Pokemon, StatSpread};
use crate::{Terrain, Weather};

use vgc_engine_data as data;

/// Curated alias tables mapping common VGC shorthand to dex slugs. Used
/// by [`resolve_species`] / [`resolve_move`] before the fuzzy prefix
/// fallback.
pub mod aliases {
    /// Species shorthand → canonical dex slug. Curated for common Reg
    /// M/B VGC mons; extend freely. Keys are already `slugify`-normal
    /// (lowercase alphanumerics), so a user typing "Lando-T" (→ `landot`)
    /// or "chomp" hits the same entry.
    pub const SPECIES: &[(&str, &str)] = &[
        ("chomp", "garchomp"),
        ("lando", "landorustherian"),
        ("landot", "landorustherian"),
        ("landoi", "landorusincarnate"),
        ("landorus", "landorusincarnate"),
        ("torn", "tornadustherian"),
        ("tornt", "tornadustherian"),
        ("thundy", "thundurustherian"),
        ("thundyt", "thundurustherian"),
        ("ttar", "tyranitar"),
        ("tran", "heatran"),
        ("gambit", "kingambit"),
        ("king", "kingambit"),
        ("tusk", "greattusk"),
        ("wochien", "wochien"),
        ("chiyu", "chiyu"),
        ("chienpao", "chienpao"),
        ("pao", "chienpao"),
        ("tinglu", "tinglu"),
        ("flutter", "fluttermane"),
        ("fmane", "fluttermane"),
        ("valiant", "ironvaliant"),
        ("hands", "ironhands"),
        ("ironhands", "ironhands"),
        ("bundle", "ironbundle"),
        ("treads", "irontreads"),
        ("moon", "roaringmoon"),
        ("rmoon", "roaringmoon"),
        ("dnite", "dragonite"),
        ("dragonite", "dragonite"),
        ("gholdengo", "gholdengo"),
        ("gholed", "gholdengo"),
        ("gengar", "gengar"),
        ("zama", "zamazentacrowned"),
        ("zamazenta", "zamazentacrowned"),
        ("koraidon", "koraidon"),
        ("miraidon", "miraidon"),
        ("calyrex", "calyrexshadow"),
        ("caly", "calyrexshadow"),
        ("calyi", "calyrexice"),
        ("urshifu", "urshifurapidstrike"),
        ("urshi", "urshifurapidstrike"),
        ("ursh", "urshifusinglestrike"),
        ("rilla", "rillaboom"),
        ("rillaboom", "rillaboom"),
        ("incin", "incineroar"),
        ("incineroar", "incineroar"),
        ("pex", "toxapex"),
        ("amoonguss", "amoonguss"),
        ("goose", "amoonguss"),
        ("ogerpon", "ogerpon"),
        ("oger", "ogerpon"),
        ("garg", "garganacl"),
        ("garganacl", "garganacl"),
        ("glowking", "slowkinggalar"),
        ("volc", "volcarona"),
        ("volcarona", "volcarona"),
        ("pelipper", "pelipper"),
        ("peli", "pelipper"),
    ];

    /// Move shorthand → canonical dex slug.
    pub const MOVES: &[(&str, &str)] = &[
        ("eq", "earthquake"),
        ("cc", "closecombat"),
        ("sb", "shadowball"),
        ("mb", "moonblast"),
        ("hp", "heatwave"),
        ("hw", "heatwave"),
        ("ib", "icebeam"),
        ("tb", "thunderbolt"),
        ("tw", "thunderwave"),
        ("wg", "wideguard"),
        ("pw", "powerwhip"),
        ("fp", "flareblitz"),
        ("bb", "bravebird"),
        ("dc", "dragonclaw"),
        ("dm", "dracometeor"),
        ("dp", "dragonpulse"),
        ("eq", "earthquake"),
        ("ep", "earthpower"),
        ("kof", "knockoff"),
        ("ko", "knockoff"),
        ("uturn", "uturn"),
        ("vs", "voltswitch"),
        ("mrock", "meteorbeam"),
        ("wh", "woodhammer"),
        ("gk", "gigadrain"),
        ("dd", "dragondance"),
        ("sd", "swordsdance"),
        ("cm", "calmmind"),
        ("na", "nastyplot"),
        ("np", "nastyplot"),
        ("pt", "protect"),
        ("prot", "protect"),
        ("fg", "fakeout"),
        ("fo", "fakeout"),
        ("ss", "spiritbreak"),
        ("mm", "makeitrain"),
        ("gs", "gigatonhammer"),
        ("ss2", "stealthrock"),
        ("sr", "stealthrock"),
        ("sp", "surgingstrikes"),
        ("ivern", "ivycudgel"),
        ("ivy", "ivycudgel"),
        ("cr", "collisioncourse"),
        ("bt", "bleakwindstorm"),
        ("hurricane", "hurricane"),
        ("scald", "scald"),
        ("psy", "psychic"),
        ("psyshock", "psyshock"),
        ("ld", "lifedew"),
        ("ww", "wildcharge"),
        ("wc", "wildcharge"),
        ("hj", "highhorsepower"),
        ("hhp", "highhorsepower"),
        ("ih", "ironhead"),
        ("fb", "flareblitz"),
        ("ov", "overheat"),
        ("oh", "overheat"),
        ("ff", "flowertrick"),
    ];
}

/// A parsed/built combatant for the fast-calc path: a species with VGC
/// defaults, overridable via [`QuickMon::parse`] or the builder setters.
///
/// Defaults (VGC / gen-9 competitive): level 50, `serious` (neutral)
/// nature, 0 EVs, 31 IVs, no item, species primary ability, no status,
/// no boosts, tera type = species primary, not terastallized.
#[derive(Debug, Clone)]
pub struct QuickMon {
    /// Canonical dex slug (already resolved through aliases/fuzzy).
    pub species: String,
    pub level: u8,
    /// Item dex slug, or `None` for no item.
    pub item: Option<String>,
    /// Ability dex slug, or `None` → species primary ability.
    pub ability: Option<String>,
    /// Nature dex slug (`serious` default = neutral).
    pub nature: String,
    pub evs: StatSpread,
    pub ivs: StatSpread,
    /// Tera type name (`"fire"`, ...) or `None` → species primary type.
    pub tera_type: Option<String>,
    pub terastallized: bool,
    pub status: Status,
    /// Stat-stage boosts, index order Atk/Def/SpA/SpD/Spe/Acc/Eva
    /// (matches `Pokemon::boosts`).
    pub boosts: [i8; 7],
}

/// Error from parsing/resolving a [`QuickMon`] or move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalcError {
    UnknownSpecies(String),
    UnknownMove(String),
    UnknownItem(String),
    UnknownAbility(String),
    /// A fuzzy prefix matched more than one candidate.
    Ambiguous { input: String, candidates: Vec<String> },
    /// A `/`-segment couldn't be classified into any grammar rule.
    BadSegment(String),
    /// The built engine mon failed to construct (bad EV/IV, etc).
    Build(String),
}

impl std::fmt::Display for CalcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalcError::UnknownSpecies(s) => write!(f, "unknown species: {s}"),
            CalcError::UnknownMove(s) => write!(f, "unknown move: {s}"),
            CalcError::UnknownItem(s) => write!(f, "unknown item: {s}"),
            CalcError::UnknownAbility(s) => write!(f, "unknown ability: {s}"),
            CalcError::Ambiguous { input, candidates } => {
                write!(f, "'{input}' is ambiguous — did you mean: {}?", candidates.join(", "))
            }
            CalcError::BadSegment(s) => write!(f, "could not parse segment '{s}'"),
            CalcError::Build(s) => write!(f, "failed to build mon: {s}"),
        }
    }
}

impl std::error::Error for CalcError {}

/// Resolve a species token to a canonical dex slug: exact slug → alias
/// table → unique `slugify` prefix match. Errors on unknown / ambiguous.
pub fn resolve_species(input: &str) -> Result<String, CalcError> {
    let slug = slugify(input);
    if data::species_by_slug(&slug).is_some() {
        return Ok(slug);
    }
    if let Some((_, canon)) = aliases::SPECIES.iter().find(|(k, _)| *k == slug) {
        return Ok((*canon).to_string());
    }
    // Fuzzy: unique prefix match against the dex.
    let mut hits: Vec<&str> = data::SPECIES
        .iter()
        .map(|s| s.slug)
        .filter(|s| s.starts_with(&slug))
        .collect();
    hits.sort_unstable();
    hits.dedup();
    match hits.len() {
        1 => Ok(hits[0].to_string()),
        0 => Err(CalcError::UnknownSpecies(input.to_string())),
        _ => Err(CalcError::Ambiguous {
            input: input.to_string(),
            candidates: hits.iter().take(6).map(|s| s.to_string()).collect(),
        }),
    }
}

/// Resolve a move token to a canonical dex slug: exact → alias → unique
/// prefix. Same disambiguation as [`resolve_species`].
pub fn resolve_move(input: &str) -> Result<String, CalcError> {
    let slug = slugify(input);
    if data::move_by_slug(&slug).is_some() {
        return Ok(slug);
    }
    if let Some((_, canon)) = aliases::MOVES.iter().find(|(k, _)| *k == slug) {
        return Ok((*canon).to_string());
    }
    let mut hits: Vec<&str> = data::MOVES
        .iter()
        .map(|m| m.slug)
        .filter(|s| s.starts_with(&slug))
        .collect();
    hits.sort_unstable();
    hits.dedup();
    match hits.len() {
        1 => Ok(hits[0].to_string()),
        0 => Err(CalcError::UnknownMove(input.to_string())),
        _ => Err(CalcError::Ambiguous {
            input: input.to_string(),
            candidates: hits.iter().take(6).map(|s| s.to_string()).collect(),
        }),
    }
}

fn stat_label_index(label: &str) -> Option<usize> {
    // Returns a StatSpread field index 0..=5 (HP/Atk/Def/SpA/SpD/Spe).
    match slugify(label).as_str() {
        "hp" => Some(0),
        "atk" | "attack" | "at" => Some(1),
        "def" | "defense" | "defence" | "df" => Some(2),
        "spa" | "spatk" | "specialattack" | "spattack" => Some(3),
        "spd" | "spdef" | "specialdefense" | "spdefense" => Some(4),
        "spe" | "speed" | "spd2" | "speed2" => Some(5),
        _ => None,
    }
}

fn set_spread_index(sp: &mut StatSpread, idx: usize, v: u8) {
    match idx {
        0 => sp.hp = v,
        1 => sp.atk = v,
        2 => sp.def = v,
        3 => sp.spa = v,
        4 => sp.spd = v,
        _ => sp.spe = v,
    }
}

fn boost_index(label: &str) -> Option<usize> {
    // Pokemon::boosts index order: Atk/Def/SpA/SpD/Spe/Acc/Eva.
    match slugify(label).as_str() {
        "atk" | "attack" | "at" => Some(0),
        "def" | "defense" | "defence" | "df" => Some(1),
        "spa" | "spatk" | "specialattack" => Some(2),
        "spd" | "spdef" | "specialdefense" => Some(3),
        "spe" | "speed" => Some(4),
        "acc" | "accuracy" => Some(5),
        "eva" | "evasion" => Some(6),
        _ => None,
    }
}

fn parse_status(s: &str) -> Option<Status> {
    match slugify(s).as_str() {
        "brn" | "burn" | "burned" => Some(Status::Burn),
        "par" | "paralysis" | "paralyzed" => Some(Status::Paralysis),
        "frz" | "freeze" | "frozen" => Some(Status::Freeze),
        "psn" | "poison" | "poisoned" => Some(Status::Poison),
        "tox" | "toxic" | "badlypoisoned" => Some(Status::Toxic),
        "slp" | "sleep" | "asleep" => Some(Status::Sleep),
        _ => None,
    }
}

impl QuickMon {
    /// A bare species with all VGC defaults. Species resolves through
    /// aliases/fuzzy (so `QuickMon::new("chomp")` works).
    pub fn new(species: &str) -> Result<Self, CalcError> {
        Ok(QuickMon {
            species: resolve_species(species)?,
            level: 50,
            item: None,
            ability: None,
            nature: "serious".to_string(),
            evs: StatSpread::ZERO,
            ivs: StatSpread::MAX_IV,
            tera_type: None,
            terastallized: false,
            status: Status::None,
            boosts: [0; 7],
        })
    }

    /// Builder: set the held item (resolved to a dex slug).
    pub fn item(mut self, item: &str) -> Result<Self, CalcError> {
        let slug = slugify(item);
        if data::item_by_slug(&slug).is_none() {
            return Err(CalcError::UnknownItem(item.to_string()));
        }
        self.item = Some(slug);
        Ok(self)
    }

    /// Builder: set the ability (resolved to a dex slug).
    pub fn ability(mut self, ability: &str) -> Result<Self, CalcError> {
        let slug = slugify(ability);
        if data::ability_by_slug(&slug).is_none() {
            return Err(CalcError::UnknownAbility(ability.to_string()));
        }
        self.ability = Some(slug);
        Ok(self)
    }

    /// Builder: set the nature.
    pub fn nature(mut self, nature: &str) -> Result<Self, CalcError> {
        let slug = slugify(nature);
        if nature_by_slug(&slug).is_none() {
            return Err(CalcError::BadSegment(format!("unknown nature: {nature}")));
        }
        self.nature = slug;
        Ok(self)
    }

    /// Builder: set an EV by stat label (`"atk"`, `"spa"`, ...).
    pub fn ev(mut self, stat: &str, value: u8) -> Result<Self, CalcError> {
        let idx = stat_label_index(stat)
            .ok_or_else(|| CalcError::BadSegment(format!("unknown EV stat: {stat}")))?;
        set_spread_index(&mut self.evs, idx, value);
        Ok(self)
    }

    /// Parse a terse ` / `-delimited spec. Grammar (order-independent
    /// after the leading species):
    ///
    /// ```text
    /// Species [@ Item] [/ Nature] [/ <N> <StatLabel>]... [/ +N Stat|-N Stat]...
    ///         [/ Lvl n] [/ Tera Type] [/ Ability] [/ status]
    /// ```
    ///
    /// Segment classification (first match wins): explicit `Lvl`/`Tera`
    /// prefixes; `+N`/`-N` boost; `<int> <label>` EV; a nature slug; an
    /// ability slug; a status token; else error.
    pub fn parse(input: &str) -> Result<Self, CalcError> {
        // Split off the item (`@`) from the first segment before the
        // ` / ` split, since an item name may itself contain spaces.
        let mut segments = input.split('/').map(|s| s.trim()).filter(|s| !s.is_empty());
        let head = segments
            .next()
            .ok_or_else(|| CalcError::BadSegment(input.to_string()))?;

        // Head = "Species" or "Species @ Item".
        let (species_tok, item_tok) = match head.split_once('@') {
            Some((sp, it)) => (sp.trim(), Some(it.trim())),
            None => (head, None),
        };
        let mut mon = QuickMon::new(species_tok)?;
        if let Some(it) = item_tok {
            mon = mon.item(it)?;
        }

        for seg in segments {
            mon.apply_segment(seg)?;
        }
        Ok(mon)
    }

    /// Classify and apply one ` / `-delimited segment. See [`Self::parse`].
    fn apply_segment(&mut self, seg: &str) -> Result<(), CalcError> {
        let lower = seg.to_ascii_lowercase();

        // Explicit prefixes first.
        if let Some(rest) = lower.strip_prefix("lvl").or_else(|| lower.strip_prefix("level")) {
            let n: u8 = rest
                .trim()
                .parse()
                .map_err(|_| CalcError::BadSegment(seg.to_string()))?;
            self.level = n;
            return Ok(());
        }
        if lower.starts_with("tera") {
            // Slice the ORIGINAL segment (not the lowercased copy) so the
            // stored tera-type name keeps its case ("Dark", not "dark").
            let ty = seg[4..].trim();
            // Validate against the 18 type names (+ stellar).
            let ok = ty.eq_ignore_ascii_case("stellar")
                || data::TYPE_NAMES.iter().any(|n| n.eq_ignore_ascii_case(ty));
            if !ok {
                return Err(CalcError::BadSegment(format!("unknown tera type: {ty}")));
            }
            self.tera_type = Some(ty.to_string());
            self.terastallized = true;
            return Ok(());
        }

        // Boost: "+1 Atk" / "-2 spa".
        if seg.starts_with('+') || seg.starts_with('-') {
            let (num, label) = seg.split_at(
                seg.find(|c: char| !(c == '+' || c == '-' || c.is_ascii_digit()))
                    .unwrap_or(seg.len()),
            );
            let stage: i8 = num.parse().map_err(|_| CalcError::BadSegment(seg.to_string()))?;
            let idx = boost_index(label.trim())
                .ok_or_else(|| CalcError::BadSegment(seg.to_string()))?;
            self.boosts[idx] = stage.clamp(-6, 6);
            return Ok(());
        }

        // EV: "252 Atk".
        let mut it = seg.split_whitespace();
        if let (Some(first), Some(second)) = (it.next(), it.next()) {
            if let Ok(v) = first.parse::<u8>() {
                if let Some(idx) = stat_label_index(second) {
                    set_spread_index(&mut self.evs, idx, v);
                    return Ok(());
                }
            }
        }

        // Nature.
        if nature_by_slug(&slugify(seg)).is_some() {
            self.nature = slugify(seg);
            return Ok(());
        }

        // Status.
        if let Some(st) = parse_status(seg) {
            self.status = st;
            return Ok(());
        }

        // Ability (last, since it's the widest table).
        if data::ability_by_slug(&slugify(seg)).is_some() {
            self.ability = Some(slugify(seg));
            return Ok(());
        }

        Err(CalcError::BadSegment(seg.to_string()))
    }

    /// Build the engine [`Pokemon`] this `QuickMon` describes, filling the
    /// species-primary ability when none was set (matching @smogon/calc's
    /// default). `primary_move` seeds `moves[0]` — [`calc`] re-writes it to
    /// the real move id anyway, but `build_member` needs at least one move.
    ///
    /// Public so the calc-oracle harness (`calc_oracle.rs`) can reuse the
    /// exact same builder path the CLI/calc use, instead of rendering a
    /// Showdown-text block and re-parsing it.
    pub fn to_pokemon(&self, primary_move: &str) -> Result<Pokemon, CalcError> {
        self.build(primary_move)
    }

    /// Build the engine [`Pokemon`], filling the species-primary ability
    /// when none was set (matching @smogon/calc's default). `primary_move`
    /// seeds moves[0]; `calc` re-writes it to the real move id anyway, but
    /// it keeps `build_member` happy (needs at least one move).
    fn build(&self, primary_move: &str) -> Result<Pokemon, CalcError> {
        // Default the ability to the species' primary if unset — the
        // engine's `build_member` leaves ability = none otherwise, but
        // @smogon/calc (and real play) assume the mon has its ability.
        let ability = match &self.ability {
            Some(a) => Some(a.clone()),
            None => species_primary_ability(&self.species),
        };
        let member = TeamMember {
            species: self.species.clone(),
            level: self.level,
            ability,
            item: self.item.clone(),
            nature: self.nature.clone(),
            moves: vec![primary_move.to_string()],
            ivs: self.ivs,
            evs: self.evs,
            teratype: self.tera_type.clone(),
            gender: None,
        };
        let mut mon = build_member(&member).map_err(|e| CalcError::Build(e.to_string()))?;
        mon.status = self.status;
        mon.terastallized = self.terastallized;
        mon.boosts = self.boosts;
        Ok(mon)
    }
}

/// The species' primary (slot-0) ability slug, or `None` if the species
/// somehow has no abilities. `@smogon/calc` defaults an unspecified mon
/// to this.
fn species_primary_ability(species_slug: &str) -> Option<String> {
    let sp = data::species_by_slug(species_slug)?;
    let id = sp.legal_abilities[0];
    if id == u16::MAX {
        return None;
    }
    Some(data::ABILITIES[id as usize].slug.to_string())
}

/// Field conditions for a calc: weather, terrain, and whether the move is
/// a Doubles spread hit (×0.75).
#[derive(Debug, Clone, Copy, Default)]
pub struct Field {
    pub weather: Weather,
    pub terrain: Terrain,
    pub spread: bool,
}

impl Field {
    /// No field effects (single-target, clear weather/terrain).
    pub fn none() -> Self {
        Field::default()
    }
    /// Just a weather.
    pub fn weather(w: Weather) -> Self {
        Field { weather: w, ..Field::default() }
    }
    /// Just a terrain.
    pub fn terrain(t: Terrain) -> Self {
        Field { terrain: t, ..Field::default() }
    }
    /// Mark this as a Doubles spread hit (applies the ×0.75 modifier).
    pub fn spread(mut self, on: bool) -> Self {
        self.spread = on;
        self
    }
}

/// 1-hit KO probability, computed exactly from the 16 damage rolls
/// against the defender's current HP.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KoChance {
    /// Every roll KOs.
    Guaranteed,
    /// Some rolls KO. `pct` is `count_ko / 16 * 100`, rounded.
    Chance { pct: u8 },
    /// No roll KOs.
    None,
}

impl KoChance {
    /// Human label. `hits` is the number of this move required (always 1
    /// in PR-1). Produces "guaranteed OHKO" / "87.5% to OHKO" / "no KO".
    pub fn label(&self, hits: u8) -> String {
        let hko = ko_word(hits);
        match self {
            KoChance::Guaranteed => format!("guaranteed {hko}"),
            KoChance::None => "no KO".to_string(),
            KoChance::Chance { pct } => format!("{pct}% to {hko}"),
        }
    }

    /// Exact number of the 16 rolls that KO — the inverse of the rounded
    /// `pct` (each count 0..=16 maps to a distinct rounded percent, so the
    /// round-trip is exact). Used by the EV-threshold search to compare
    /// against a target probability without float rounding.
    fn count16(&self) -> u8 {
        match self {
            KoChance::None => 0,
            KoChance::Guaranteed => 16,
            KoChance::Chance { pct } => ((*pct as u32 * 16 + 50) / 100) as u8,
        }
    }
}

/// Round a count-out-of-16 to a whole percent (matches `ko_from_rolls`).
fn pct16(count: u8) -> u8 {
    ((count as u32 * 100 + 8) / 16) as u8
}

fn ko_word(hits: u8) -> String {
    match hits {
        1 => "OHKO".to_string(),
        2 => "2HKO".to_string(),
        3 => "3HKO".to_string(),
        n => format!("{n}HKO"),
    }
}

/// Result of a single-move damage calc: the 16 rolls plus derived range,
/// percentages, and 1-hit KO estimate.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DamageResult {
    /// The 16 damage rolls, ascending (roll 0..=15).
    pub rolls: [u16; 16],
    pub min: u16,
    pub max: u16,
    /// Defender's max HP (denominator for percentages / KO).
    pub defender_max_hp: u16,
    /// Min damage as a percentage of the defender's max HP (0.1% res).
    pub min_pct: f32,
    pub max_pct: f32,
    /// Exact 1-hit KO probability from the 16 rolls vs current HP.
    pub ko_chance: KoChance,
    /// Minimum hits to KO (2HKO/3HKO/…) plus its exact probability, from
    /// convolving the roll distribution against remaining HP. Subsumes
    /// `ko_chance` (the `hits == 1` case) but reported separately so
    /// callers can show either the terse single-hit verdict or the full
    /// NHKO label.
    pub multi_hit: MultiHitKo,
    /// The crit-row result, when the caller asked for a non-crit calc we
    /// still compute the crit companion so callers can show both. `None`
    /// on a crit calc (no nested crit) or a 0-damage calc.
    pub crit: Option<Box<DamageResult>>,
}

/// Full calc with an explicit [`Field`]. Builds both mons, runs
/// `damage_only`, and shapes the [`DamageResult`] (including a nested
/// crit companion for the non-crit case).
pub fn calc(
    attacker: &QuickMon,
    defender: &QuickMon,
    move_: &str,
    field: Field,
) -> Result<DamageResult, CalcError> {
    let move_slug = resolve_move(move_)?;
    let atk = attacker.build(&move_slug)?;
    let def = defender.build("splash")?;
    let move_id = data::MOVES
        .iter()
        .position(|m| m.slug == move_slug)
        .map(|i| i as u16)
        .ok_or_else(|| CalcError::UnknownMove(move_.to_string()))?;

    let base = shape_result(&atk, &def, move_id, field, false);
    // Companion crit row (only for the non-crit case; a 0-damage move
    // has no meaningful crit either).
    let crit = if base.max > 0 {
        Some(Box::new(shape_result(&atk, &def, move_id, field, true)))
    } else {
        None
    };
    Ok(DamageResult { crit, ..base })
}

/// [`calc`] with no field effects (single-target, clear).
pub fn calc_default(
    attacker: &QuickMon,
    defender: &QuickMon,
    move_: &str,
) -> Result<DamageResult, CalcError> {
    calc(attacker, defender, move_, Field::none())
}

// ---------------------------------------------------------------------------
// Thin helper family (PR-4): terse yes/no wrappers over `calc` /
// `effective_speed` for the common questions a player asks a calculator.
// ---------------------------------------------------------------------------

/// The 1-hit KO probability of `attacker`'s `move_` against `defender`.
/// Thin wrapper over [`calc`] returning just the [`KoChance`].
pub fn ohko_chance(
    attacker: &QuickMon,
    defender: &QuickMon,
    move_: &str,
    field: Field,
) -> Result<KoChance, CalcError> {
    Ok(calc(attacker, defender, move_, field)?.ko_chance)
}

/// Does `defender` **guaranteed-survive** a single hit of `attacker`'s
/// `move_`? True iff even the maximum roll leaves it above 0 HP (i.e. the
/// hit is not a guaranteed OHKO — every roll fails to KO). Zero-damage /
/// immune hits trivially survive.
pub fn survives(
    attacker: &QuickMon,
    defender: &QuickMon,
    move_: &str,
    field: Field,
) -> Result<bool, CalcError> {
    let ko = calc(attacker, defender, move_, field)?.ko_chance;
    // Survives-for-sure = the move never KOs on any roll.
    Ok(matches!(ko, KoChance::None))
}

/// Field context affecting a mon's effective speed: per-side tailwind,
/// weather (Swift Swim / Chlorophyll / Sand Rush / Slush Rush), and Trick
/// Room (which *reverses* the comparison without changing the stat).
#[derive(Debug, Clone, Copy, Default)]
pub struct SpeedContext {
    pub weather: Weather,
    pub tailwind: bool,
    pub trick_room: bool,
}

impl SpeedContext {
    pub fn none() -> Self {
        SpeedContext::default()
    }
}

/// A mon's **effective** speed under `ctx` — boosts, paralysis, Choice
/// Scarf / Iron Ball, Paradox Spe, Unburden, tailwind, and weather speed
/// abilities all folded in (see [`crate::order::effective_speed`]). This
/// is the "speed tier" a player reads off a calc.
///
/// Trick Room does **not** change this number — it flips the *comparison*
/// (see [`outspeeds`]).
pub fn speed_tier(mon: &QuickMon, ctx: SpeedContext) -> Result<u16, CalcError> {
    // Any move keeps `build_member` happy; speed is move-independent.
    let p = mon.to_pokemon("splash")?;
    Ok(crate::order::effective_speed(&p, ctx.tailwind, ctx.weather))
}

/// The winner of a speed comparison between two mons, respecting Trick
/// Room. A speed **tie** (equal effective speed) is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeedWinner {
    /// The first mon (`a`) moves first.
    A,
    /// The second mon (`b`) moves first.
    B,
    /// Equal effective speed — a coin-flip speed tie.
    Tie,
}

/// Does `a` move before `b`? Compares effective speeds under `ctx`,
/// respecting Trick Room (slower moves first). Returns [`SpeedWinner`].
///
/// Only the speed *stat* comparison is modeled — move priority is a
/// per-move property the caller handles separately.
pub fn outspeeds(a: &QuickMon, b: &QuickMon, ctx: SpeedContext) -> Result<SpeedWinner, CalcError> {
    let sa = speed_tier(a, ctx)?;
    let sb = speed_tier(b, ctx)?;
    Ok(match sa.cmp(&sb) {
        std::cmp::Ordering::Equal => SpeedWinner::Tie,
        std::cmp::Ordering::Greater => {
            if ctx.trick_room {
                SpeedWinner::B
            } else {
                SpeedWinner::A
            }
        }
        std::cmp::Ordering::Less => {
            if ctx.trick_room {
                SpeedWinner::A
            } else {
                SpeedWinner::B
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Move selection + matchup summary (PR-5).
// ---------------------------------------------------------------------------

/// One move's calc against a defender, tagged with the move for ranking.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MoveDamage {
    /// The move's dex slug (resolved).
    pub move_slug: String,
    /// Full damage result for this move.
    pub result: DamageResult,
}

/// Calc every move in `moves` from `attacker` into `defender` and return
/// the one that does the most damage, ranked by (in order): the best KO
/// verdict (fewest hits to KO), then max roll, then min roll. Ties break
/// toward the earlier move in `moves`.
///
/// `moves` is the candidate list (names or aliases) — `QuickMon` doesn't
/// carry a moveset, so the caller supplies it. Errors if `moves` is empty
/// or any move fails to resolve.
pub fn best_move(
    attacker: &QuickMon,
    defender: &QuickMon,
    moves: &[&str],
    field: Field,
) -> Result<MoveDamage, CalcError> {
    if moves.is_empty() {
        return Err(CalcError::BadSegment("no candidate moves given".to_string()));
    }
    let mut best: Option<MoveDamage> = None;
    for &mv in moves {
        let slug = resolve_move(mv)?;
        let result = calc(attacker, defender, mv, field)?;
        let cand = MoveDamage { move_slug: slug, result };
        best = Some(match best {
            None => cand,
            Some(cur) => {
                if move_beats(&cand.result, &cur.result) {
                    cand
                } else {
                    cur
                }
            }
        });
    }
    Ok(best.unwrap())
}

/// Is `a` a strictly better attacking result than `b`? Ranks by fewest
/// hits-to-KO (a real KO beats a non-KO; `hits == 0` = never KOs, worst),
/// then higher max roll, then higher min roll.
fn move_beats(a: &DamageResult, b: &DamageResult) -> bool {
    // Map hits-to-KO to a sortable key: 0 (never) is worst → treat as
    // u8::MAX; otherwise fewer hits is better.
    let ka = if a.multi_hit.hits == 0 { u8::MAX } else { a.multi_hit.hits };
    let kb = if b.multi_hit.hits == 0 { u8::MAX } else { b.multi_hit.hits };
    match ka.cmp(&kb) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Greater => false,
        std::cmp::Ordering::Equal => (a.max, a.min) > (b.max, b.min),
    }
}

/// Symmetric two-sided matchup summary: each side's best move into the
/// other, who moves first, and the KO verdicts. Built from two `best_move`
/// calls plus one speed comparison.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Matchup {
    /// `a`'s best move into `b`.
    pub a_best: MoveDamage,
    /// `b`'s best move into `a`.
    pub b_best: MoveDamage,
    /// Who moves first on the speed comparison (respecting Trick Room).
    pub speed_winner: SpeedWinner,
    /// `a`'s effective speed under the context.
    pub a_speed: u16,
    /// `b`'s effective speed under the context.
    pub b_speed: u16,
}

/// Compute a full [`Matchup`] between `a` (with `a_moves`) and `b` (with
/// `b_moves`). `field` shapes the damage calcs; `ctx` the speed compare.
/// Spread is taken from `field.spread` for both sides' damage.
pub fn matchup(
    a: &QuickMon,
    a_moves: &[&str],
    b: &QuickMon,
    b_moves: &[&str],
    field: Field,
    ctx: SpeedContext,
) -> Result<Matchup, CalcError> {
    let a_best = best_move(a, b, a_moves, field)?;
    let b_best = best_move(b, a, b_moves, field)?;
    let a_speed = speed_tier(a, ctx)?;
    let b_speed = speed_tier(b, ctx)?;
    let speed_winner = outspeeds(a, b, ctx)?;
    Ok(Matchup { a_best, b_best, speed_winner, a_speed, b_speed })
}

// ---------------------------------------------------------------------------
// EV optimizers (PR-6): binary-search the minimum EV investment that hits a
// survival / KO threshold, re-running `calc` at each probe.
// ---------------------------------------------------------------------------

/// The 64 legal EV values a single stat can take: 0, 4, 8, …, 252. Binary
/// search walks this grid, not raw 0..=255.
const EV_GRID: [u8; 64] = {
    let mut g = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        g[i] = (i * 4) as u8;
        i += 1;
    }
    g
};

/// A defensive stat a survival calc can invest in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefStat {
    Hp,
    Def,
    Spd,
}

/// An offensive stat a KO calc can invest in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtkStat {
    Atk,
    Spa,
}

fn set_def_ev(mon: &mut QuickMon, stat: DefStat, ev: u8) {
    match stat {
        DefStat::Hp => mon.evs.hp = ev,
        DefStat::Def => mon.evs.def = ev,
        DefStat::Spd => mon.evs.spd = ev,
    }
}

fn set_atk_ev(mon: &mut QuickMon, stat: AtkStat, ev: u8) {
    match stat {
        AtkStat::Atk => mon.evs.atk = ev,
        AtkStat::Spa => mon.evs.spa = ev,
    }
}

/// Result of an EV-threshold search ([`min_evs_to_survive`] /
/// [`min_evs_to_ko`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvSearch {
    /// Least EVs (a multiple of 4, in the searched stat) that meet the
    /// target roll-count, or `None` if even 252 EVs fall short.
    pub evs: Option<u8>,
    /// At MAX (252) EVs: the number of the 16 rolls the defender SURVIVES
    /// (`survive`) or the attacker KOs on (`ko`), `0..=16`. When `evs` is
    /// `None` this is the residual — the *best achievable* — so a near-miss
    /// reads as "survives 14/16 even maxed" rather than a flat failure.
    pub rolls_at_max: u8,
    /// [`rolls_at_max`] as a rounded percent (`0..=100`), for display.
    pub pct_at_max: u8,
}

/// Minimum EVs in `stat` for `defender` to **survive** at least
/// `min_survive_rolls` of the 16 damage rolls of `attacker`'s `move_`. Pass
/// `16` for the strict "survives every roll" benchmark; `15` for "survives
/// all but the highest roll", etc. — the count is exact (no rounding).
///
/// Binary-searches the 0..=252 EV grid (survival is monotonic in defensive
/// EVs). Returns [`EvSearch`]: `evs = Some(..)` when the target is reachable,
/// or `None` with `rolls_at_max` = the survive-count at 252 EVs when it
/// isn't — so "can't survive" reads as the truer "survives N/16 even maxed".
pub fn min_evs_to_survive(
    attacker: &QuickMon,
    defender: &QuickMon,
    move_: &str,
    stat: DefStat,
    field: Field,
    min_survive_rolls: u8,
) -> Result<EvSearch, CalcError> {
    // Resolve once so a bad move errors before the search loop.
    resolve_move(move_)?;
    let target = min_survive_rolls.min(16);
    // Number of the 16 rolls the defender SURVIVES at `ev` in `stat`.
    let survive_count = |ev: u8| -> Result<u8, CalcError> {
        let mut d = defender.clone();
        set_def_ev(&mut d, stat, ev);
        Ok(16 - calc(attacker, &d, move_, field)?.ko_chance.count16())
    };

    let sc_max = survive_count(252)?;
    if sc_max < target {
        return Ok(EvSearch { evs: None, rolls_at_max: sc_max, pct_at_max: pct16(sc_max) });
    }
    // Binary search for the least grid index that meets the target.
    let mut lo = 0usize;
    let mut hi = EV_GRID.len() - 1; // known to meet at hi
    while lo < hi {
        let mid = (lo + hi) / 2;
        if survive_count(EV_GRID[mid])? >= target {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(EvSearch { evs: Some(EV_GRID[lo]), rolls_at_max: sc_max, pct_at_max: pct16(sc_max) })
}

/// Minimum EVs in `stat` for `attacker`'s `move_` to **KO** `defender` on at
/// least `min_ko_rolls` of the 16 rolls. Pass `16` for the strict
/// "guaranteed KO" benchmark; `15` for "KOs on all but the lowest roll".
///
/// Binary-searches the 0..=252 EV grid (KO chance is monotonic in offensive
/// EVs). Returns [`EvSearch`]: `evs = Some(..)` when reachable, or `None`
/// with `rolls_at_max` = the KO-count at 252 EVs when not — so "can't KO"
/// reads as the truer "KOs on N/16 even maxed".
pub fn min_evs_to_ko(
    attacker: &QuickMon,
    defender: &QuickMon,
    move_: &str,
    stat: AtkStat,
    field: Field,
    min_ko_rolls: u8,
) -> Result<EvSearch, CalcError> {
    resolve_move(move_)?;
    let target = min_ko_rolls.min(16);
    let ko_count = |ev: u8| -> Result<u8, CalcError> {
        let mut a = attacker.clone();
        set_atk_ev(&mut a, stat, ev);
        Ok(calc(&a, defender, move_, field)?.ko_chance.count16())
    };

    let kc_max = ko_count(252)?;
    if kc_max < target {
        return Ok(EvSearch { evs: None, rolls_at_max: kc_max, pct_at_max: pct16(kc_max) });
    }
    let mut lo = 0usize;
    let mut hi = EV_GRID.len() - 1;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if ko_count(EV_GRID[mid])? >= target {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(EvSearch { evs: Some(EV_GRID[lo]), rolls_at_max: kc_max, pct_at_max: pct16(kc_max) })
}

/// Run `damage_only` for one (crit / non-crit) row and shape the result.
/// Does NOT populate the nested `crit` field (caller handles that).
fn shape_result(
    atk: &Pokemon,
    def: &Pokemon,
    move_id: u16,
    field: Field,
    is_crit: bool,
) -> DamageResult {
    let defender_max_hp = def.stats.hp;
    let q = DamageQuery {
        attacker: atk.clone(),
        defender: def.clone(),
        move_id,
        weather: field.weather,
        terrain: field.terrain,
        is_crit,
        is_spread: field.spread,
    };
    let rolls = damage_only(&q);
    let min = *rolls.iter().min().unwrap();
    let max = *rolls.iter().max().unwrap();
    let denom = defender_max_hp.max(1) as f32;
    let min_pct = (min as f32 / denom) * 100.0;
    let max_pct = (max as f32 / denom) * 100.0;

    // 1-hit KO from the rolls vs the defender's CURRENT hp (full by
    // default; `current_hp` respects a pre-damaged defender if set).
    let ko_chance = ko_from_rolls(&rolls, def.current_hp);
    // Multi-hit KO (2HKO/3HKO/…) via convolution — pure arithmetic on the
    // same rolls, no extra engine calls.
    let multi_hit = multi_hit_ko(&rolls, def.current_hp);

    DamageResult {
        rolls,
        min,
        max,
        defender_max_hp,
        min_pct,
        max_pct,
        ko_chance,
        multi_hit,
        crit: None,
    }
}

/// The number of unmodified hits of this move needed to KO the defender,
/// and the exact probability that that many hits does it.
///
/// "NHKO" in calc parlance: the smallest hit-count `n` for which *some*
/// combination of the 16-roll distribution sums to at least the target's
/// remaining HP, plus `chance` = the exact probability (over iid rolls)
/// that a run of `n` hits reaches the HP. `chance == 1.0` is a
/// *guaranteed* NHKO. `hits == 0` means the move can never KO (max damage
/// is 0, e.g. an immune target).
///
/// This is a pure-arithmetic companion to [`KoChance`] (which is the
/// single-hit case): no extra engine calls, just a convolution of the
/// already-computed roll distribution against remaining HP.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct MultiHitKo {
    /// Minimum hits to KO (1 = OHKO, 2 = 2HKO, ...). `0` = never KOs.
    pub hits: u8,
    /// Exact probability that `hits` unmodified hits KO the target
    /// (0.0..=1.0). `1.0` = guaranteed NHKO.
    pub chance: f32,
}

impl MultiHitKo {
    /// Human label, e.g. "guaranteed 2HKO", "56.3% to 3HKO", "no KO".
    pub fn label(&self) -> String {
        if self.hits == 0 {
            return "no KO".to_string();
        }
        let hko = ko_word(self.hits);
        if self.chance >= 1.0 {
            format!("guaranteed {hko}")
        } else {
            format!("{:.1}% to {hko}", self.chance * 100.0)
        }
    }
}

/// Compute the minimum-hits-to-KO plus its exact probability by
/// convolving the 16 equiprobable damage rolls against `remaining_hp`.
///
/// Each hit independently draws one of the 16 `rolls` uniformly. We build
/// the distribution of the running damage sum hit-by-hit (a discrete
/// convolution, capped at `remaining_hp` so the state space stays tiny),
/// and stop at the first hit-count whose surviving-probability mass drops
/// below 1 — i.e. the first `n` where P(sum ≥ hp) > 0. `chance` is that
/// probability. Guaranteed OHKO short-circuits to `{1, 1.0}`.
///
/// Zero-damage moves (max roll 0) never KO → `{0, 0.0}`. A pre-fainted
/// defender (`remaining_hp == 0`) is a `{1, 1.0}` guaranteed OHKO.
pub fn multi_hit_ko(rolls: &[u16; 16], remaining_hp: u16) -> MultiHitKo {
    if remaining_hp == 0 {
        return MultiHitKo { hits: 1, chance: 1.0 };
    }
    let max_roll = *rolls.iter().max().unwrap();
    if max_roll == 0 {
        return MultiHitKo { hits: 0, chance: 0.0 };
    }
    let hp = remaining_hp as u32;

    // `dist[d]` = probability the running damage sum equals exactly `d`,
    // for d in 0..hp. Mass that reaches/exceeds `hp` is a KO and leaves
    // the tracked state (accumulated into `ko_prob`). Start with all mass
    // at damage 0 (before any hit).
    let mut dist = vec![0.0f64; hp as usize];
    dist[0] = 1.0;
    let mut ko_prob = 0.0f64;
    let each = 1.0f64 / 16.0;

    // Safety cap: hp hits guarantees at least `hp` damage (every roll ≥ 1
    // once max_roll > 0 — but min roll could be 0, so also break when the
    // surviving mass is negligible). `max_roll > 0` guarantees termination
    // because dist mass strictly migrates upward each hit.
    for n in 1..=(hp.max(1) as usize + 1) {
        let mut next = vec![0.0f64; hp as usize];
        for (d, &p) in dist.iter().enumerate() {
            if p == 0.0 {
                continue;
            }
            for &r in rolls.iter() {
                let sum = d as u32 + r as u32;
                if sum >= hp {
                    ko_prob += p * each;
                } else {
                    next[sum as usize] += p * each;
                }
            }
        }
        dist = next;
        if ko_prob > 0.0 {
            // First hit-count that can KO. Round to f32 for the API.
            let chance = ko_prob.min(1.0) as f32;
            return MultiHitKo { hits: n as u8, chance };
        }
    }
    // Unreachable in practice (max_roll > 0 forces a KO within hp hits),
    // but stay total: report a guaranteed KO at the cap.
    MultiHitKo { hits: (hp as u8).max(1), chance: 1.0 }
}

/// Exact 1-hit KO probability: count rolls that meet/exceed the target's
/// remaining HP. All 16 → Guaranteed; none → None; else a rounded pct.
fn ko_from_rolls(rolls: &[u16; 16], remaining_hp: u16) -> KoChance {
    if remaining_hp == 0 {
        return KoChance::Guaranteed;
    }
    let ko = rolls.iter().filter(|&&d| d >= remaining_hp).count();
    match ko {
        16 => KoChance::Guaranteed,
        0 => KoChance::None,
        n => {
            // round(n/16 * 100) to nearest integer percent:
            //   (n*100 + 8) / 16   (add half-denominator for rounding).
            let pct = ((n as u32 * 100 + 8) / 16) as u8;
            KoChance::Chance { pct }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_resolution() {
        assert_eq!(resolve_species("chomp").unwrap(), "garchomp");
        assert_eq!(resolve_species("lando").unwrap(), "landorustherian");
        assert_eq!(resolve_move("eq").unwrap(), "earthquake");
        assert_eq!(resolve_move("cc").unwrap(), "closecombat");
        // Exact slug passes through.
        assert_eq!(resolve_species("Iron Hands").unwrap(), "ironhands");
        assert_eq!(resolve_species("Flutter Mane").unwrap(), "fluttermane");
    }

    #[test]
    fn fuzzy_prefix_and_ambiguity() {
        // Unique prefix resolves ("amoongu" → amoonguss, no other mon
        // shares that prefix).
        assert_eq!(resolve_species("amoongu").unwrap(), "amoonguss");
        // Unknown errors.
        assert!(matches!(
            resolve_species("notamon").unwrap_err(),
            CalcError::UnknownSpecies(_)
        ));
        // Ambiguous prefix errors with candidates.
        match resolve_species("char") {
            Err(CalcError::Ambiguous { candidates, .. }) => {
                assert!(candidates.len() > 1, "expected several charmander-line hits");
            }
            other => panic!("expected ambiguity, got {other:?}"),
        }
    }

    #[test]
    fn parse_terse_fields() {
        let m = QuickMon::parse("Garchomp @ Life Orb / Jolly / 252 Atk / 4 HP / +1 Atk / Lvl 50")
            .unwrap();
        assert_eq!(m.species, "garchomp");
        assert_eq!(m.item.as_deref(), Some("lifeorb"));
        assert_eq!(m.nature, "jolly");
        assert_eq!(m.evs.atk, 252);
        assert_eq!(m.evs.hp, 4);
        assert_eq!(m.boosts[0], 1, "Atk boost stage");
        assert_eq!(m.level, 50);
    }

    #[test]
    fn parse_status_and_tera_and_ability() {
        let m = QuickMon::parse("Kingambit @ Choice Band / Adamant / Defiant / brn / Tera Dark")
            .unwrap();
        assert_eq!(m.item.as_deref(), Some("choiceband"));
        assert_eq!(m.ability.as_deref(), Some("defiant"));
        assert_eq!(m.status, Status::Burn);
        assert_eq!(m.tera_type.as_deref(), Some("Dark"));
        assert!(m.terastallized);
    }

    #[test]
    fn builder_setters() {
        let m = QuickMon::new("chomp")
            .unwrap()
            .item("Life Orb")
            .unwrap()
            .nature("Jolly")
            .unwrap()
            .ev("atk", 252)
            .unwrap()
            .ev("spe", 252)
            .unwrap();
        assert_eq!(m.species, "garchomp");
        assert_eq!(m.item.as_deref(), Some("lifeorb"));
        assert_eq!(m.evs.atk, 252);
        assert_eq!(m.evs.spe, 252);
    }

    #[test]
    fn calc_matches_smogon_cc_lifeorb() {
        // scenario-cc-lifeorb: Lucario Life Orb Close Combat into Garchomp.
        // @smogon/calc: [99,99,101,101,103,105,105,107,107,109,110,110,113,113,114,117].
        let atk = QuickMon::parse(
            "Lucario @ Life Orb / Adamant / Inner Focus / 4 HP / 252 Atk / 252 Spe",
        )
        .unwrap();
        let def = QuickMon::parse(
            "Garchomp / Impish / Sand Veil / 252 HP / 252 Def / 4 SpD",
        )
        .unwrap();
        let r = calc_default(&atk, &def, "Close Combat").unwrap();
        assert_eq!(
            r.rolls,
            [99, 99, 101, 101, 103, 105, 105, 107, 107, 109, 110, 110, 113, 113, 114, 117]
        );
    }

    #[test]
    fn calc_matches_smogon_choiceband_ironhead() {
        // scenario-choiceband-kingambit-ironhead: into Flutter Mane.
        // @smogon/calc: [372,374,380,384,386,392,396,402,404,410,414,420,422,428,432,438].
        let atk = QuickMon::parse(
            "Kingambit @ Choice Band / Adamant / Defiant / 252 HP / 252 Atk / 4 SpD",
        )
        .unwrap();
        let def = QuickMon::parse(
            "Flutter Mane / Timid / Protosynthesis / 4 HP / 252 SpA / 252 Spe",
        )
        .unwrap();
        let r = calc_default(&atk, &def, "iron head").unwrap();
        assert_eq!(
            r.rolls,
            [372, 374, 380, 384, 386, 392, 396, 402, 404, 410, 414, 420, 422, 428, 432, 438]
        );
    }

    #[test]
    fn calc_spread_applies_075() {
        // Garchomp Earthquake into Iron Hands, Doubles spread.
        // @smogon/calc Doubles: [120,122,122,126,126,128,128,132,132,134,134,138,138,140,140,144].
        let atk =
            QuickMon::parse("Garchomp / Jolly / 4 HP / 252 Atk / 252 Spe").unwrap();
        let def =
            QuickMon::parse("Iron Hands / Adamant / 252 HP / 4 Atk / 252 SpD").unwrap();
        let single = calc_default(&atk, &def, "eq").unwrap();
        assert_eq!(single.min, 162, "single-target min");
        let spread = calc(&atk, &def, "eq", Field::none().spread(true)).unwrap();
        assert_eq!(
            spread.rolls,
            [120, 122, 122, 126, 126, 128, 128, 132, 132, 134, 134, 138, 138, 140, 140, 144],
            "spread EQ vs Iron Hands should match @smogon/calc Doubles"
        );
    }

    #[test]
    fn immune_target_reports_no_damage() {
        // chomp EQ vs Lando-T (Flying immune to Ground) → 0, no KO.
        let atk = QuickMon::new("chomp").unwrap();
        let def = QuickMon::new("lando").unwrap();
        let r = calc_default(&atk, &def, "eq").unwrap();
        assert_eq!(r.max, 0);
        assert_eq!(r.ko_chance, KoChance::None);
        assert!(r.crit.is_none(), "no crit companion on a 0-damage calc");
    }

    #[test]
    fn ko_chance_from_rolls() {
        // All rolls exceed 100 → guaranteed.
        let rolls = [120u16; 16];
        assert_eq!(ko_from_rolls(&rolls, 100), KoChance::Guaranteed);
        // None reach 200 → no KO.
        assert_eq!(ko_from_rolls(&rolls, 200), KoChance::None);
        // 8 of 16 reach 130 → 50%.
        let mut mixed = [100u16; 16];
        for x in mixed.iter_mut().take(8) {
            *x = 140;
        }
        assert_eq!(ko_from_rolls(&mixed, 130), KoChance::Chance { pct: 50 });
    }

    #[test]
    fn multi_hit_ko_guaranteed_ohko() {
        // Every roll ≥ 100 → guaranteed OHKO.
        let rolls = [120u16; 16];
        let mh = multi_hit_ko(&rolls, 100);
        assert_eq!(mh.hits, 1);
        assert_eq!(mh.chance, 1.0);
        assert_eq!(mh.label(), "guaranteed OHKO");
    }

    #[test]
    fn multi_hit_ko_zero_damage_never_kos() {
        let rolls = [0u16; 16];
        let mh = multi_hit_ko(&rolls, 100);
        assert_eq!(mh.hits, 0);
        assert_eq!(mh.chance, 0.0);
        assert_eq!(mh.label(), "no KO");
    }

    #[test]
    fn multi_hit_ko_guaranteed_2hko() {
        // All rolls 60; hp 100. One hit (max 60) can't KO; two hits (120)
        // always do → guaranteed 2HKO.
        let rolls = [60u16; 16];
        let mh = multi_hit_ko(&rolls, 100);
        assert_eq!(mh.hits, 2);
        assert_eq!(mh.chance, 1.0);
        assert_eq!(mh.label(), "guaranteed 2HKO");
    }

    #[test]
    fn multi_hit_ko_chance_2hko_probability() {
        // Two-outcome roll: 8×40 and 8×60, hp = 100. A 2HKO needs sum ≥
        // 100. Pairs: 40+40=80 (no), 40+60=100 (yes), 60+40=100 (yes),
        // 60+60=120 (yes). Each roll is p=1/2 for 40 vs 60, so P(KO in 2) =
        // 1 - P(both 40) = 1 - 1/4 = 3/4.
        let mut rolls = [40u16; 16];
        for x in rolls.iter_mut().take(8) {
            *x = 60;
        }
        let mh = multi_hit_ko(&rolls, 100);
        assert_eq!(mh.hits, 2);
        assert!(
            (mh.chance - 0.75).abs() < 1e-4,
            "expected 0.75, got {}",
            mh.chance
        );
    }

    #[test]
    fn multi_hit_ko_uniform_convolution_matches_bruteforce() {
        // Spread EQ vs Iron Hands (from `calc_spread_applies_075`): a
        // non-OHKO whose 2HKO probability we brute-force over all 16² pairs
        // and compare to the convolution.
        let atk = QuickMon::parse("Garchomp / Jolly / 4 HP / 252 Atk / 252 Spe").unwrap();
        let def = QuickMon::parse("Iron Hands / Adamant / 252 HP / 4 Atk / 252 SpD").unwrap();
        let r = calc(&atk, &def, "eq", Field::none().spread(true)).unwrap();
        let hp = r.defender_max_hp as u32;
        assert!(r.max < r.defender_max_hp, "should not be a OHKO");
        // Brute-force 2HKO probability.
        let mut ko = 0u32;
        for &a in r.rolls.iter() {
            for &b in r.rolls.iter() {
                if a as u32 + b as u32 >= hp {
                    ko += 1;
                }
            }
        }
        let brute = ko as f32 / 256.0;
        // If the convolution says 2HKO, its chance must match brute force.
        if r.multi_hit.hits == 2 {
            assert!(
                (r.multi_hit.chance - brute).abs() < 1e-4,
                "conv {} vs brute {}",
                r.multi_hit.chance,
                brute
            );
        } else {
            // Otherwise it must be a guaranteed 2HKO region we short of —
            // fail loudly so we notice a model change.
            panic!("expected a 2HKO for spread EQ vs Iron Hands, got {:?}", r.multi_hit);
        }
    }

    #[test]
    fn survives_and_ohko_chance() {
        // Immune target trivially survives, no OHKO.
        let chomp = QuickMon::new("chomp").unwrap();
        let lando = QuickMon::new("lando").unwrap();
        assert!(survives(&chomp, &lando, "eq", Field::none()).unwrap());
        assert_eq!(
            ohko_chance(&chomp, &lando, "eq", Field::none()).unwrap(),
            KoChance::None
        );

        // Spread EQ vs bulky Iron Hands: not a OHKO → survives.
        let atk = QuickMon::parse("Garchomp / Jolly / 252 Atk").unwrap();
        let def = QuickMon::parse("Iron Hands / Adamant / 252 HP / 252 SpD").unwrap();
        assert!(survives(&atk, &def, "eq", Field::none().spread(true)).unwrap());
    }

    #[test]
    fn speed_tier_boosts_and_scarf() {
        // Base: Jolly max-Spe Garchomp = 169 at level 50 (well-known tier).
        let chomp = QuickMon::parse("Garchomp / Jolly / 252 Spe").unwrap();
        let base = speed_tier(&chomp, SpeedContext::none()).unwrap();
        assert_eq!(base, 169, "Jolly 252 Spe Garchomp base speed tier");

        // Tailwind doubles it.
        let tw = speed_tier(
            &chomp,
            SpeedContext { tailwind: true, ..SpeedContext::none() },
        )
        .unwrap();
        assert_eq!(tw, base * 2);

        // Choice Scarf = ×1.5.
        let scarf = QuickMon::parse("Garchomp @ Choice Scarf / Jolly / 252 Spe").unwrap();
        assert_eq!(speed_tier(&scarf, SpeedContext::none()).unwrap(), base * 3 / 2);
    }

    #[test]
    fn outspeeds_and_trick_room() {
        let fast = QuickMon::parse("Garchomp / Jolly / 252 Spe").unwrap();
        let slow = QuickMon::parse("Iron Hands / Brave / 0 Spe").unwrap();
        // Normal: the faster mon moves first.
        assert_eq!(outspeeds(&fast, &slow, SpeedContext::none()).unwrap(), SpeedWinner::A);
        // Trick Room reverses it.
        assert_eq!(
            outspeeds(
                &fast,
                &slow,
                SpeedContext { trick_room: true, ..SpeedContext::none() }
            )
            .unwrap(),
            SpeedWinner::B
        );
        // Mirror match = speed tie.
        assert_eq!(outspeeds(&fast, &fast, SpeedContext::none()).unwrap(), SpeedWinner::Tie);
    }

    #[test]
    fn best_move_skips_immune_move() {
        // Garchomp into Landorus-Therian (Flying → Ground-immune):
        // Earthquake does 0, Dragon Claw does real damage → best_move must
        // pick Dragon Claw, never the immune Earthquake.
        let atk = QuickMon::parse("Garchomp / Adamant / 252 Atk").unwrap();
        let def = QuickMon::parse("Landorus-Therian / 4 HP").unwrap();
        let best = best_move(&atk, &def, &["earthquake", "dragonclaw"], Field::none()).unwrap();
        assert_eq!(best.move_slug, "dragonclaw", "non-immune move should win");
        assert!(best.result.max > 0);

        // Empty move list errors.
        assert!(best_move(&atk, &def, &[], Field::none()).is_err());
    }

    #[test]
    fn matchup_summary_symmetric() {
        let a = QuickMon::parse("Garchomp / Jolly / 252 Atk / 252 Spe").unwrap();
        let b = QuickMon::parse("Iron Hands / Brave / 252 HP / 252 Atk").unwrap();
        let m = matchup(
            &a,
            &["earthquake"],
            &b,
            &["closecombat", "fakeout"],
            Field::none(),
            SpeedContext::none(),
        )
        .unwrap();
        // Garchomp (Jolly 252 Spe) clearly outspeeds Brave Iron Hands.
        assert_eq!(m.speed_winner, SpeedWinner::A);
        assert!(m.a_speed > m.b_speed);
        assert_eq!(m.a_best.move_slug, "earthquake");
        // Iron Hands' best of {CC, Fake Out} is Close Combat (Fake Out is
        // 40 BP), so it should out-damage.
        assert_eq!(m.b_best.move_slug, "closecombat");
    }

    #[test]
    fn min_evs_to_survive_monotone_and_exact() {
        // Adamant Choice Band Kingambit Iron Head into Flutter Mane is a
        // guaranteed OHKO at 0 HP EVs; find the min HP EVs to survive.
        let atk = QuickMon::parse(
            "Kingambit @ Choice Band / Adamant / 252 Atk",
        )
        .unwrap();
        let def = QuickMon::parse("Flutter Mane / Timid / 252 SpA / 252 Spe").unwrap();
        let got = min_evs_to_survive(&atk, &def, "iron head", DefStat::Hp, Field::none(), 16)
            .unwrap()
            .evs;
        // Whatever the threshold, verify the binary-search invariant: the
        // returned EV survives and one grid step lower does not.
        match got {
            Some(ev) => {
                let mut lives = def.clone();
                lives.evs.hp = ev;
                assert!(
                    matches!(
                        calc(&atk, &lives, "iron head", Field::none()).unwrap().ko_chance,
                        KoChance::None
                    ),
                    "returned EV {ev} should survive"
                );
                if ev >= 4 {
                    let mut dies = def.clone();
                    dies.evs.hp = ev - 4;
                    assert!(
                        !matches!(
                            calc(&atk, &dies, "iron head", Field::none()).unwrap().ko_chance,
                            KoChance::None
                        ),
                        "EV {} (one step below) should NOT survive",
                        ev - 4
                    );
                }
            }
            None => {
                // If unsurvivable even at 252, confirm that directly.
                let mut maxed = def.clone();
                maxed.evs.hp = 252;
                assert!(matches!(
                    calc(&atk, &maxed, "iron head", Field::none()).unwrap().ko_chance,
                    KoChance::Guaranteed | KoChance::Chance { .. }
                ));
            }
        }
    }

    #[test]
    fn min_evs_to_ko_finds_threshold() {
        // Garchomp Earthquake into a bulky Iron Hands: find min Atk EVs to
        // guarantee the OHKO (may be None if it can't OHKO at all).
        let atk = QuickMon::parse("Garchomp / Adamant / 0 Atk").unwrap();
        let def = QuickMon::parse("Flutter Mane / Timid / 4 HP").unwrap();
        // EQ vs frail Flutter Mane OHKOs easily; the min Atk EV to
        // *guarantee* it exists and is well under 252.
        let got = min_evs_to_ko(&atk, &def, "earthquake", AtkStat::Atk, Field::none(), 16)
            .unwrap()
            .evs;
        match got {
            Some(ev) => {
                let mut a = atk.clone();
                a.evs.atk = ev;
                assert!(matches!(
                    calc(&a, &def, "earthquake", Field::none()).unwrap().ko_chance,
                    KoChance::Guaranteed
                ));
                if ev >= 4 {
                    let mut a2 = atk.clone();
                    a2.evs.atk = ev - 4;
                    assert!(!matches!(
                        calc(&a2, &def, "earthquake", Field::none()).unwrap().ko_chance,
                        KoChance::Guaranteed
                    ));
                }
            }
            None => panic!("EQ should guaranteed-OHKO frail Flutter Mane at some Atk EV"),
        }
    }

    #[test]
    fn ev_threshold_monotone_and_residual() {
        // Structural invariants that hold regardless of the exact damage:
        // a lower survival target never needs MORE EVs, and an unreachable
        // strict target reports a residual survival % below 100.
        let atk = QuickMon::parse("Kingambit @ Life Orb / Adamant / 252 Atk").unwrap();
        let def = QuickMon::parse("Flutter Mane / Timid / 252 SpA / 252 Spe").unwrap();
        let strict =
            min_evs_to_survive(&atk, &def, "iron head", DefStat::Hp, Field::none(), 16).unwrap();
        let lenient =
            min_evs_to_survive(&atk, &def, "iron head", DefStat::Hp, Field::none(), 8).unwrap();
        assert!(strict.rolls_at_max <= 16 && strict.pct_at_max <= 100);
        if strict.evs.is_none() {
            assert!(strict.rolls_at_max < 16, "unreachable ⇒ survives < 16/16 even maxed");
        }
        match (strict.evs, lenient.evs) {
            (Some(s), Some(l)) => assert!(l <= s, "lenient target can't need more EVs than strict"),
            (Some(_), None) => panic!("a lower survival target can't be harder than a higher one"),
            (None, _) => {}
        }
    }
}
