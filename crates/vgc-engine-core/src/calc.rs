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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone)]
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

    DamageResult {
        rolls,
        min,
        max,
        defender_max_hp,
        min_pct,
        max_pct,
        ko_chance,
        crit: None,
    }
}

/// Exact 1-hit KO probability: count rolls that meet/exceed the target's
/// remaining HP. All 16 → Guaranteed; none → None; else a rounded pct.
///
/// TODO(PR-2): multi-hit KO (2HKO/3HKO) — needs the residual/EOT model
/// and a survives-N-turns convolution. PR-1 reports the 1-hit case only.
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
}
