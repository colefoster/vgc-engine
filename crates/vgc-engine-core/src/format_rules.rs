//! Per-format team legality verification.
//!
//! [`verify_team`] checks a parsed team (a slice of [`TeamMember`] — the same
//! intermediate the JSON and Showdown-text loaders produce) against a
//! [`FormatRules`] ruleset and collects **all** rule violations (it does not
//! fail-fast). This is an offline / team-building check; the battle simulator
//! itself trusts the caller to pass legal sets and does not consult these
//! rules in the hot loop.
//!
//! The ruleset is data-driven — adding a new format is a new [`FormatRules`]
//! constant plus a `rules_for` arm.
//!
//! The flagship ruleset is [`REG_M_B`] (Pokémon Champions, Regulation M / B
//! doubles): bring-6-pick-4, level 50, Species & Item clause, 510-EV cap,
//! Terastallization banned, Mega Evolution legal, and species legality gated by
//! an authoritative 208-entry national-dex **allow-list** (the Champions roster
//! — see [`REG_M_B_LEGAL_SPECIES`]). The allow-list subsumes the old
//! tag-derived bans (restricted / mythical / paradox / Treasures of Ruin),
//! since none of those Pokémon are on the roster.
//!
//! Behaviour mirrors PS's `sim/team-validator.ts` where reasonable:
//!   * EV total / per-stat cap — `validateStats` (team-validator.ts:1132).
//!   * Species Clause (by national-dex `num`) — `validateTeam` (line ~135).
//!   * Item Clause — Format rule `Item Clause`.
//!   * Level cap / forced level — `validateSet` (line 605-622).
//!   * Legal-ability check — `validateSet` ability block (line ~700).
//!   * Move legality — `validateSet` / `checkCanLearn` (team-validator.ts).
//!
//! Move legality is checked against the build-time learnset tables
//! ([`data::species_can_learn`]), which pre-merge each species' learnset with
//! its pre-evolution chain and base forme. The check is permissive
//! (transfer-legal / HOME): a move is legal if the species or any pre-evo /
//! base forme can learn it via ANY method in ANY generation. See
//! [`FormatRules::check_move_legality`] for the simplifications vs PS.

use crate::team::{slugify, TeamMember};
use vgc_engine_data as data;

/// A single ruleset, keyed by [`FormatRules::id`]. Data-driven so a new format
/// is a new constant + a `rules_for` arm.
#[derive(Debug, Clone)]
pub struct FormatRules {
    /// Stable id used by [`rules_for`] (e.g. `"regmb"`).
    pub id: &'static str,
    /// Human-readable format name (used in messages).
    pub name: &'static str,
    /// Inclusive team-size bounds. Reg M-B is bring-6-pick-4 → 4..=6.
    pub min_team_size: usize,
    pub max_team_size: usize,
    /// Exact level every set must be at. `None` = unrestricted.
    pub required_level: Option<u8>,
    /// Max sum of EVs across the six stats.
    pub ev_total_limit: u16,
    /// Max EVs in any single stat.
    pub ev_per_stat_limit: u16,
    /// Max legal IV in any stat.
    pub iv_max: u8,
    /// Inclusive move-count bounds per set.
    pub min_moves: usize,
    pub max_moves: usize,
    /// Reject two sets sharing a national-dex `num` (Species Clause).
    pub species_clause: bool,
    /// Reject two sets holding the same item (Item Clause).
    pub item_clause: bool,
    /// When `false`, a set that *specifies* a Tera type is illegal
    /// (Terastallization is unusable in-format).
    pub tera_allowed: bool,
    /// Authoritative species allow-list, keyed by **base national-dex `num`**
    /// (regional formes and megas share their base's `num`, so they are legal
    /// iff the base is listed). When `Some`, this is the *only* species-legality
    /// gate: a set whose `num` is absent is illegal (and the tag-derived bans
    /// below become redundant). When `None`, species legality falls back to the
    /// tag bans + [`extra_banned_num`]. Stored sorted for binary search; a new
    /// format supplies its own slice.
    pub legal_species: Option<&'static [u16]>,
    /// Ban species tagged "Restricted Legendary".
    pub ban_restricted: bool,
    /// Ban species tagged "Mythical".
    pub ban_mythical: bool,
    /// Ban species tagged "Paradox".
    pub ban_paradox: bool,
    /// Extra species banned by national-dex number. Used for bans PS does not
    /// express as a unique tag — e.g. the Treasures of Ruin, which PS tags only
    /// as "Sub-Legendary".
    pub extra_banned_num: &'static [u16],
    /// When `true`, every move in a set must be in the species' (pre-merged)
    /// learnset — see [`data::species_can_learn`]. Move *count*, *duplicates*,
    /// and *unknown moves* are validated regardless of this flag.
    ///
    /// Simplifications vs PS `sim/team-validator.ts` `checkCanLearn` (all
    /// intentionally permissive, in the transfer-legal / HOME direction):
    ///   * No egg-move **parent-chain** validation (a move tagged egg-only is
    ///     accepted without checking a compatible breeding parent exists).
    ///   * No **event-exclusive move-combination** enforcement (each move is
    ///     checked independently; illegal *combinations* of event moves pass).
    ///   * No **per-generation** restriction — a move legal in any gen counts
    ///     (HOME transfers are assumed available).
    ///   * Regional formes additionally inherit their base forme's pool via
    ///     `baseSpecies` (slightly looser than PS, which keeps regional formes'
    ///     learnsets separate). Required so megas validate against their base.
    ///   * Typed Hidden Power variants (`hiddenpowerfire`, …) are not in
    ///     learnsets and would be flagged; only base `hiddenpower` is listed.
    ///     Moot for gen-9 formats (Hidden Power is unobtainable).
    pub check_move_legality: bool,
}

/// Treasures of Ruin — Wo-Chien (1001), Chien-Pao (1002), Ting-Lu (1003),
/// Chi-Yu (1004). PS tags them only as "Sub-Legendary" (shared with many
/// legal sub-legendaries), so they are banned here by national-dex number.
/// Source: `data/pokedex.json` `num`; Bulbapedia "Treasures of Ruin".
const TREASURES_OF_RUIN: &[u16] = &[1001, 1002, 1003, 1004];

/// Pokémon Champions — Reg M-B legal-species allow-list, by **base national-dex
/// `num`** (208 entries, sorted ascending for binary search).
///
/// Champions ships a curated roster: legendaries / mythicals / paradox / the
/// Treasures of Ruin simply aren't in the game, so this allow-list *is* the
/// species gate and subsumes the tag-based bans. No species is restricted /
/// banned this season — inclusion is the only legality test. Regional formes
/// and megas share their base's `num` and are legal iff the base is listed.
///
/// Source: Serebii Pokémon Champions Pokédex
/// (<https://www.serebii.net/pokemonchampions/pokemon.shtml>); cross-checked
/// vs MetaVGC and pokemon.com's Reg M-B announcement (2026-06, high confidence).
pub const REG_M_B_LEGAL_SPECIES: &[u16] = &[
    3, 6, 9, 15, 18, 24, 25, 26, 36, 38, 45, 59, 65, 68, 71, 80, 94, 115, 121,
    127, 128, 130, 132, 134, 135, 136, 142, 143, 149, 154, 157, 160, 168, 181,
    184, 186, 196, 197, 199, 205, 208, 211, 212, 214, 227, 229, 248, 254, 257,
    260, 279, 282, 302, 303, 306, 308, 310, 319, 323, 324, 334, 350, 351, 354,
    358, 359, 362, 376, 389, 392, 395, 398, 405, 407, 409, 411, 428, 442, 445,
    448, 450, 454, 460, 461, 464, 470, 471, 472, 473, 475, 478, 479, 497, 500,
    503, 505, 510, 512, 514, 516, 518, 530, 531, 534, 545, 547, 553, 560, 563,
    569, 571, 579, 584, 587, 604, 609, 614, 618, 623, 635, 637, 652, 655, 658,
    660, 663, 666, 668, 670, 671, 675, 676, 678, 681, 683, 685, 687, 689, 691,
    693, 695, 697, 699, 700, 701, 702, 706, 707, 709, 711, 713, 715, 724, 727,
    730, 733, 740, 745, 748, 750, 752, 758, 763, 765, 766, 778, 780, 784, 823,
    841, 842, 844, 855, 858, 861, 866, 867, 869, 870, 877, 887, 899, 900, 902,
    903, 904, 908, 911, 914, 925, 934, 936, 937, 939, 952, 956, 959, 964, 968,
    970, 972, 979, 981, 983, 1000, 1013, 1018, 1019,
];

/// Pokémon Champions — Regulation M / Regulation B doubles.
///
/// Tera BANNED, Mega LEGAL. Species legality is the authoritative
/// [`REG_M_B_LEGAL_SPECIES`] allow-list (by base dex `num`); the tag-derived
/// bans (`ban_restricted` / `ban_mythical` / `ban_paradox`) and
/// `extra_banned_num` are retained as a harmless secondary check but are
/// **redundant** under the allow-list (none of those Pokémon are on the
/// roster). (Memory: `project_regmb_format_scope`.)
///
/// EV rules for Champions are **unconfirmed**; we assume standard VGC (EV total
/// ≤ 510, ≤ 252 / stat, IVs 0–31) and match PS's 510 total cap.
pub const REG_M_B: FormatRules = FormatRules {
    id: "regmb",
    name: "Pokémon Champions Reg M-B (Doubles)",
    min_team_size: 4,
    max_team_size: 6,
    required_level: Some(50),
    // Champions EV rules unconfirmed — standard VGC assumed (PS uses 510).
    ev_total_limit: 510,
    ev_per_stat_limit: 252,
    iv_max: 31,
    min_moves: 1,
    max_moves: 4,
    species_clause: true,
    item_clause: true,
    tera_allowed: false,
    legal_species: Some(REG_M_B_LEGAL_SPECIES),
    ban_restricted: true,
    ban_mythical: true,
    ban_paradox: true,
    extra_banned_num: TREASURES_OF_RUIN,
    // Learnset tables are generated by build.rs from learnsets.json; validate
    // each move against the species' (pre-merged) learnset.
    check_move_legality: true,
};

/// Look up a ruleset by format id. Accepts the common Reg M / Reg B aliases.
pub fn rules_for(id: &str) -> Option<&'static FormatRules> {
    match id.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
        "regmb" | "regm" | "regb" | "regbm" | "champions" => Some(&REG_M_B),
        _ => None,
    }
}

/// Which rule a [`Violation`] broke. Stable enough to assert on in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    TeamSize,
    SpeciesClause,
    ItemClause,
    Level,
    EvTotal,
    EvPerStat,
    Iv,
    MoveCount,
    DuplicateMove,
    IllegalAbility,
    BannedSpecies,
    TeraNotAllowed,
    MoveLegality,
    UnknownSpecies,
    UnknownAbility,
    UnknownItem,
    UnknownMove,
}

/// A single legality problem with a team. Names the offending set and the rule.
#[derive(Debug, Clone)]
pub struct Violation {
    /// 0-based index of the offending set, or `None` for a team-wide rule
    /// (team size).
    pub member: Option<usize>,
    /// Species as written in the offending set (for messaging). Empty for
    /// team-wide violations.
    pub species: String,
    /// Which rule was broken.
    pub rule: Rule,
    /// Human-readable explanation.
    pub message: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.member {
            Some(i) => write!(f, "[{} (set {})] {}", self.species, i + 1, self.message),
            None => write!(f, "[team] {}", self.message),
        }
    }
}

fn member_label(i: usize, m: &TeamMember) -> String {
    if m.species.trim().is_empty() {
        format!("set {}", i + 1)
    } else {
        m.species.clone()
    }
}

fn ev_total(m: &TeamMember) -> u16 {
    let e = &m.evs;
    e.hp as u16 + e.atk as u16 + e.def as u16 + e.spa as u16 + e.spd as u16 + e.spe as u16
}

const STAT_NAMES: [&str; 6] = ["HP", "Atk", "Def", "SpA", "SpD", "Spe"];

fn ev_array(m: &TeamMember) -> [u16; 6] {
    let e = &m.evs;
    [
        e.hp as u16, e.atk as u16, e.def as u16,
        e.spa as u16, e.spd as u16, e.spe as u16,
    ]
}

fn iv_array(m: &TeamMember) -> [u8; 6] {
    let i = &m.ivs;
    [i.hp, i.atk, i.def, i.spa, i.spd, i.spe]
}

/// Verify a parsed team against a ruleset. Collects **all** violations; an
/// empty team or a fully legal team returns `Ok(())`.
pub fn verify_team(team: &[TeamMember], rules: &FormatRules) -> Result<(), Vec<Violation>> {
    let mut v: Vec<Violation> = Vec::new();

    // --- Team-wide: size.
    if team.len() < rules.min_team_size || team.len() > rules.max_team_size {
        v.push(Violation {
            member: None,
            species: String::new(),
            rule: Rule::TeamSize,
            message: format!(
                "team has {} Pokémon; {} allows {}–{}",
                team.len(),
                rules.name,
                rules.min_team_size,
                rules.max_team_size
            ),
        });
    }

    // Species Clause is by national-dex num; Item Clause by item id.
    let mut seen_species: Vec<u16> = Vec::new();
    let mut seen_items: Vec<u16> = Vec::new();

    for (i, m) in team.iter().enumerate() {
        let label = member_label(i, m);
        let push = |v: &mut Vec<Violation>, rule: Rule, message: String| {
            v.push(Violation { member: Some(i), species: label.clone(), rule, message });
        };

        // Resolve the set's species once: its table index drives move-legality
        // (in the move loop below) and the banned/clause/ability checks later.
        // `None` ⇒ unknown species (reported by the species block below).
        let species_def = data::species_by_slug(&slugify(&m.species));
        let species_id = species_def
            .map(|sp| data::SPECIES.iter().position(|x| x.slug == sp.slug).unwrap() as u16);

        // --- Level.
        if let Some(req) = rules.required_level {
            if m.level != req {
                push(&mut v, Rule::Level, format!("level {} but {} requires level {}", m.level, rules.name, req));
            }
        }

        // --- EVs.
        let total = ev_total(m);
        if total > rules.ev_total_limit {
            push(&mut v, Rule::EvTotal, format!("{} total EVs exceeds the limit of {}", total, rules.ev_total_limit));
        }
        for (s, &ev) in ev_array(m).iter().enumerate() {
            if ev > rules.ev_per_stat_limit {
                push(&mut v, Rule::EvPerStat, format!("{} EVs in {} exceeds the per-stat limit of {}", ev, STAT_NAMES[s], rules.ev_per_stat_limit));
            }
        }

        // --- IVs.
        for (s, &iv) in iv_array(m).iter().enumerate() {
            if iv > rules.iv_max {
                push(&mut v, Rule::Iv, format!("IV {} in {} is above the maximum of {}", iv, STAT_NAMES[s], rules.iv_max));
            }
        }

        // --- Moves: count, duplicates, unknown, (optional) legality.
        if m.moves.len() < rules.min_moves {
            push(&mut v, Rule::MoveCount, format!("has {} moves; at least {} required", m.moves.len(), rules.min_moves));
        }
        if m.moves.len() > rules.max_moves {
            push(&mut v, Rule::MoveCount, format!("has {} moves; at most {} allowed", m.moves.len(), rules.max_moves));
        }
        let mut seen_moves: Vec<u16> = Vec::new();
        for mv in &m.moves {
            match data::move_by_slug(&slugify(mv)) {
                None => push(&mut v, Rule::UnknownMove, format!("unknown move \"{}\"", mv)),
                Some(def) => {
                    let id = data::MOVES.iter().position(|x| x.slug == def.slug).unwrap() as u16;
                    if seen_moves.contains(&id) {
                        push(&mut v, Rule::DuplicateMove, format!("duplicate move \"{}\"", mv));
                    } else {
                        seen_moves.push(id);
                        // Move legality (learnset). Checked once per distinct
                        // move, only when the format opts in and the species
                        // resolved (an unknown species is reported separately).
                        if rules.check_move_legality {
                            if let Some(sid) = species_id {
                                if !data::species_can_learn(sid, id) {
                                    push(&mut v, Rule::MoveLegality, format!("cannot legally learn \"{}\"", def.name));
                                }
                            }
                        }
                    }
                }
            }
        }

        // --- Item (resolve once, for Item Clause + unknown check).
        let item_id = match m.item.as_deref().filter(|s| !s.is_empty()) {
            None => None,
            Some(item) => match data::item_by_slug(&slugify(item)) {
                None => {
                    push(&mut v, Rule::UnknownItem, format!("unknown item \"{}\"", item));
                    None
                }
                Some(def) => Some(data::ITEMS.iter().position(|x| x.slug == def.slug).unwrap() as u16),
            },
        };
        if rules.item_clause {
            if let Some(id) = item_id {
                if seen_items.contains(&id) {
                    push(&mut v, Rule::ItemClause, format!("duplicate held item \"{}\"", m.item.as_deref().unwrap_or("")));
                } else {
                    seen_items.push(id);
                }
            }
        }

        // --- Tera.
        if !rules.tera_allowed && m.teratype.is_some() {
            push(&mut v, Rule::TeraNotAllowed, format!("specifies a Tera type, but Terastallization is banned in {}", rules.name));
        }

        // --- Species-dependent checks (need a resolvable species).
        let Some(sp) = species_def else {
            push(&mut v, Rule::UnknownSpecies, format!("unknown species \"{}\"", m.species));
            continue;
        };

        // Banned species. When the format has an allow-list, inclusion is the
        // authoritative (and only meaningful) gate — a species whose base dex
        // `num` is absent is not in the format. The tag-derived bans below are
        // then redundant (no banned tag survives the allow-list) but kept as a
        // harmless secondary check for formats that supply no allow-list.
        if let Some(allow) = rules.legal_species {
            if allow.binary_search(&sp.num).is_err() {
                push(
                    &mut v,
                    Rule::BannedSpecies,
                    format!(
                        "{} (dex #{}) is not in the {} legal-species list",
                        sp.name, sp.num, rules.name
                    ),
                );
            }
        }
        let ban_reason = if rules.ban_restricted && sp.restricted {
            Some("a Restricted Legendary")
        } else if rules.ban_mythical && sp.mythical {
            Some("a Mythical")
        } else if rules.ban_paradox && sp.paradox {
            Some("a Paradox Pokémon")
        } else if rules.extra_banned_num.contains(&sp.num) {
            Some("banned in this format")
        } else {
            None
        };
        // Only report a tag ban if the allow-list didn't already flag the set,
        // to avoid two violations for the same species.
        let allow_flagged = rules
            .legal_species
            .is_some_and(|a| a.binary_search(&sp.num).is_err());
        if let Some(reason) = ban_reason {
            if !allow_flagged {
                push(&mut v, Rule::BannedSpecies, format!("{} is {}, which is banned in {}", sp.name, reason, rules.name));
            }
        }

        // Species Clause (by dex num — covers alternate formes).
        if rules.species_clause {
            if seen_species.contains(&sp.num) {
                push(&mut v, Rule::SpeciesClause, format!("a second Pokémon shares dex #{} ({}) — Species Clause", sp.num, sp.name));
            } else {
                seen_species.push(sp.num);
            }
        }

        // Ability legality.
        if let Some(ability) = m.ability.as_deref().filter(|s| !s.is_empty()) {
            match data::ability_by_slug(&slugify(ability)) {
                None => push(&mut v, Rule::UnknownAbility, format!("unknown ability \"{}\"", ability)),
                Some(def) => {
                    let id = data::ABILITIES.iter().position(|x| x.slug == def.slug).unwrap() as u16;
                    if !sp.legal_abilities.contains(&id) {
                        push(&mut v, Rule::IllegalAbility, format!("ability \"{}\" is not a legal ability of {}", def.name, sp.name));
                    }
                }
            }
        }

    }

    if v.is_empty() {
        Ok(())
    } else {
        Err(v)
    }
}

/// Parse a Showdown export blob and verify it against a ruleset in one call.
/// On a parse error returns a single synthetic [`Violation`]; otherwise returns
/// the parsed sets alongside any rule violations.
pub fn verify_showdown_text(
    s: &str,
    rules: &FormatRules,
) -> Result<Vec<TeamMember>, Vec<Violation>> {
    match crate::team_export::parse_showdown_export(s) {
        Err(e) => Err(vec![Violation {
            member: None,
            species: String::new(),
            rule: Rule::UnknownSpecies,
            message: format!("parse error: {e}"),
        }]),
        Ok(team) => match verify_team(&team, rules) {
            Ok(()) => Ok(team),
            Err(v) => Err(v),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pokemon::StatSpread;

    fn base(species: &str) -> TeamMember {
        TeamMember {
            species: species.into(),
            level: 50,
            ability: None,
            item: None,
            nature: "serious".into(),
            moves: vec!["protect".into()],
            ivs: StatSpread::MAX_IV,
            evs: StatSpread::default(),
            teratype: None,
            gender: None,
        }
    }

    /// A fully legal Reg M-B doubles team (4 distinct, in-format mons with
    /// distinct items, legal abilities, level 50, no Tera).
    fn legal_team() -> Vec<TeamMember> {
        let mut incin = base("Incineroar");
        incin.ability = Some("Intimidate".into());
        incin.item = Some("Sitrus Berry".into());
        incin.evs = StatSpread { hp: 252, atk: 0, def: 4, spa: 0, spd: 252, spe: 0 };
        incin.moves = vec!["Fake Out".into(), "Flare Blitz".into(), "Parting Shot".into(), "Knock Off".into()];

        // Talonflame (#663) is on the Champions roster; Amoonguss (#591) is not.
        let mut tflame = base("Talonflame");
        tflame.ability = Some("Gale Wings".into());
        tflame.item = Some("Rocky Helmet".into());
        tflame.moves = vec!["Brave Bird".into(), "Tailwind".into(), "Roost".into(), "Protect".into()];

        let mut chomp = base("Garchomp");
        chomp.ability = Some("Rough Skin".into());
        chomp.item = Some("Life Orb".into());
        chomp.evs = StatSpread { hp: 4, atk: 252, def: 0, spa: 0, spd: 0, spe: 252 };
        chomp.moves = vec!["Earthquake".into(), "Dragon Claw".into(), "Protect".into()];

        let mut tini = base("Flutter Mane"); // paradox — replace below
        tini.species = "Dragonite".into();
        tini.ability = Some("Multiscale".into());
        tini.item = Some("Choice Band".into());
        tini.moves = vec!["Extreme Speed".into(), "Tera Blast".into(), "Stomping Tantrum".into(), "Ice Spinner".into()];

        vec![incin, tflame, chomp, tini]
    }

    fn rules_of(team: &[TeamMember]) -> Vec<Rule> {
        match verify_team(team, &REG_M_B) {
            Ok(()) => vec![],
            Err(v) => v.iter().map(|x| x.rule).collect(),
        }
    }

    #[test]
    fn legal_regmb_team_passes() {
        let team = legal_team();
        assert_eq!(verify_team(&team, &REG_M_B).map_err(|v| {
            v.iter().map(|x| x.to_string()).collect::<Vec<_>>()
        }), Ok(()));
    }

    #[test]
    fn team_size_too_small() {
        let team = vec![base("Garchomp"), base("Amoonguss"), base("Incineroar")];
        // 3 distinct species, but below the 4-mon minimum.
        assert!(rules_of(&team).contains(&Rule::TeamSize));
    }

    #[test]
    fn species_clause_duplicate() {
        let mut team = legal_team();
        team[3] = base("Garchomp"); // duplicate of team[2]
        team[3].ability = Some("Rough Skin".into());
        team[3].item = Some("Focus Sash".into());
        assert!(rules_of(&team).contains(&Rule::SpeciesClause));
    }

    #[test]
    fn species_clause_by_forme_num() {
        // Rotom and Rotom-Heat share dex #479 — Species Clause should fire.
        let mut team = legal_team();
        team[2] = base("Rotom");
        team[2].ability = Some("Levitate".into());
        team[2].item = Some("Focus Sash".into());
        team[3] = base("Rotom-Heat");
        team[3].ability = Some("Levitate".into());
        team[3].item = Some("Choice Scarf".into());
        assert!(rules_of(&team).contains(&Rule::SpeciesClause));
    }

    #[test]
    fn item_clause_duplicate() {
        let mut team = legal_team();
        team[1].item = Some("Sitrus Berry".into()); // already on team[0]
        assert!(rules_of(&team).contains(&Rule::ItemClause));
    }

    #[test]
    fn banned_restricted_legendary() {
        let mut team = legal_team();
        team[0] = base("Calyrex-Shadow");
        team[0].ability = Some("As One (Spectrier)".into());
        team[0].item = Some("Spell Tag".into());
        let rs = rules_of(&team);
        assert!(rs.contains(&Rule::BannedSpecies));
    }

    #[test]
    fn banned_paradox() {
        let mut team = legal_team();
        team[0] = base("Flutter Mane");
        team[0].ability = Some("Protosynthesis".into());
        team[0].item = Some("Booster Energy".into());
        assert!(rules_of(&team).contains(&Rule::BannedSpecies));
    }

    #[test]
    fn banned_treasure_of_ruin() {
        let mut team = legal_team();
        team[0] = base("Chi-Yu");
        team[0].ability = Some("Beads of Ruin".into());
        team[0].item = Some("Choice Scarf".into());
        assert!(rules_of(&team).contains(&Rule::BannedSpecies));
    }

    #[test]
    fn ev_over_252_per_stat() {
        let mut team = legal_team();
        team[0].evs = StatSpread { hp: 255, atk: 0, def: 0, spa: 0, spd: 0, spe: 0 };
        assert!(rules_of(&team).contains(&Rule::EvPerStat));
    }

    #[test]
    fn ev_total_over_cap() {
        let mut team = legal_team();
        // 252×3 = 756 > 510 total, but each stat is within the 252 per-stat cap.
        team[0].evs = StatSpread { hp: 252, atk: 252, def: 252, spa: 0, spd: 0, spe: 0 };
        let rs = rules_of(&team);
        assert!(rs.contains(&Rule::EvTotal));
        assert!(!rs.contains(&Rule::EvPerStat));
    }

    #[test]
    fn ev_total_cap_is_510() {
        // Champions EV rules are unconfirmed; we assume standard VGC (PS = 510).
        assert_eq!(REG_M_B.ev_total_limit, 510);
        // 510 total is legal; 511 is not.
        let mut team = legal_team();
        team[0].evs = StatSpread { hp: 252, atk: 252, def: 6, spa: 0, spd: 0, spe: 0 };
        assert!(!rules_of(&team).contains(&Rule::EvTotal), "510 EVs should be legal");
        team[0].evs = StatSpread { hp: 252, atk: 252, def: 7, spa: 0, spd: 0, spe: 0 };
        assert!(rules_of(&team).contains(&Rule::EvTotal), "511 EVs should be illegal");
    }

    #[test]
    fn allowlist_has_208_sorted_unique_entries() {
        assert_eq!(REG_M_B_LEGAL_SPECIES.len(), 208);
        assert!(
            REG_M_B_LEGAL_SPECIES.windows(2).all(|w| w[0] < w[1]),
            "allow-list must be strictly ascending (sorted + unique) for binary_search"
        );
    }

    #[test]
    fn legal_champions_species_pass() {
        // Incineroar (#727) and Garganacl (#934) are both on the roster.
        let mut team = legal_team();
        team[0] = base("Garganacl");
        team[0].ability = Some("Purifying Salt".into());
        team[0].item = Some("Leftovers".into());
        team[0].moves = vec!["Salt Cure".into(), "Recover".into(), "Protect".into()];
        assert!(!rules_of(&team).contains(&Rule::BannedSpecies), "Garganacl should be in-format");
    }

    #[test]
    fn species_not_in_champions_is_flagged() {
        // Bulbasaur (#1) is in the National Dex but NOT on the Champions roster.
        let mut team = legal_team();
        team[0] = base("Bulbasaur");
        team[0].ability = Some("Overgrow".into());
        team[0].item = Some("Eviolite".into());
        team[0].moves = vec!["Tackle".into()];
        assert!(rules_of(&team).contains(&Rule::BannedSpecies), "Bulbasaur is not in Champions");
    }

    #[test]
    fn restricted_flagged_via_allowlist_not_tag() {
        // Miraidon (#1008, a restricted legendary) is absent from the allow-list,
        // so it is flagged purely by the allow-list — independent of the tag ban.
        let mut team = legal_team();
        team[0] = base("Miraidon");
        team[0].ability = Some("Hadron Engine".into());
        team[0].item = Some("Choice Specs".into());
        team[0].moves = vec!["Electro Drift".into()];
        assert!(!REG_M_B_LEGAL_SPECIES.contains(&1008), "Miraidon should not be in the roster");
        assert!(rules_of(&team).contains(&Rule::BannedSpecies));
    }

    #[test]
    fn mega_forme_of_legal_base_passes() {
        // Mega Charizard Y shares base dex #006 (Charizard is on the roster),
        // so the mega forme is legal by inheriting the base's num.
        let mut team = legal_team();
        team[0] = base("Charizard-Mega-Y");
        team[0].ability = Some("Drought".into());
        team[0].item = Some("Charizardite Y".into());
        team[0].moves = vec!["Heat Wave".into(), "Protect".into()];
        let rs = rules_of(&team);
        assert!(!rs.contains(&Rule::BannedSpecies), "Mega Charizard Y should be in-format: {:?}", rs);
    }

    #[test]
    fn illegal_ability() {
        let mut team = legal_team();
        team[0].ability = Some("Levitate".into()); // Incineroar can't have Levitate
        assert!(rules_of(&team).contains(&Rule::IllegalAbility));
    }

    #[test]
    fn tera_in_regm_is_illegal() {
        let mut team = legal_team();
        team[0].teratype = Some("Grass".into());
        assert!(rules_of(&team).contains(&Rule::TeraNotAllowed));
    }

    #[test]
    fn wrong_level() {
        let mut team = legal_team();
        team[0].level = 100;
        assert!(rules_of(&team).contains(&Rule::Level));
    }

    #[test]
    fn too_many_moves() {
        let mut team = legal_team();
        team[0].moves = vec!["Fake Out".into(), "Flare Blitz".into(), "Parting Shot".into(), "Knock Off".into(), "U-turn".into()];
        assert!(rules_of(&team).contains(&Rule::MoveCount));
    }

    #[test]
    fn no_moves() {
        let mut team = legal_team();
        team[0].moves = vec![];
        assert!(rules_of(&team).contains(&Rule::MoveCount));
    }

    #[test]
    fn duplicate_move() {
        let mut team = legal_team();
        team[0].moves = vec!["Protect".into(), "Protect".into()];
        assert!(rules_of(&team).contains(&Rule::DuplicateMove));
    }

    #[test]
    fn iv_over_31() {
        let mut team = legal_team();
        team[0].ivs = StatSpread { hp: 99, atk: 31, def: 31, spa: 31, spd: 31, spe: 31 };
        assert!(rules_of(&team).contains(&Rule::Iv));
    }

    #[test]
    fn illegal_move_flagged() {
        // Incineroar cannot learn Spore (a Grass status move) — MoveLegality.
        let mut team = legal_team();
        team[0].moves = vec!["Fake Out".into(), "Spore".into(), "Parting Shot".into(), "Knock Off".into()];
        assert!(rules_of(&team).contains(&Rule::MoveLegality));
    }

    #[test]
    fn move_legal_via_prevo_accepted() {
        // Dragonite learns Supersonic ONLY through its pre-evolutions
        // (Dratini / Dragonair). The chain-walk must accept it.
        let mut team = legal_team();
        team[3].moves = vec!["Supersonic".into(), "Extreme Speed".into(), "Ice Spinner".into()];
        let rs = rules_of(&team);
        assert!(!rs.contains(&Rule::MoveLegality), "prevo move wrongly flagged: {:?}", rs);
    }

    #[test]
    fn legal_team_passes_move_legality() {
        // Every move on the legal fixture team is in-learnset → no MoveLegality.
        let team = legal_team();
        let rs = rules_of(&team);
        assert!(!rs.contains(&Rule::MoveLegality), "legal team flagged: {:?}", rs);
    }

    #[test]
    fn rules_for_aliases() {
        assert!(rules_for("regmb").is_some());
        assert!(rules_for("Reg M").is_some());
        assert!(rules_for("gen9ou").is_none());
    }

    #[test]
    fn verify_showdown_text_end_to_end() {
        // A small legal Reg M-B paste verifies clean.
        let paste = "\
Incineroar @ Sitrus Berry
Ability: Intimidate
Level: 50
EVs: 252 HP / 4 Def / 252 SpD
Careful Nature
- Fake Out
- Flare Blitz
- Parting Shot
- Knock Off

Talonflame @ Rocky Helmet
Ability: Gale Wings
Level: 50
- Brave Bird
- Tailwind
- Roost
- Protect

Garchomp @ Life Orb
Ability: Rough Skin
Level: 50
EVs: 4 HP / 252 Atk / 252 Spe
Jolly Nature
- Earthquake
- Dragon Claw
- Protect

Dragonite @ Choice Band
Ability: Multiscale
Level: 50
- Extreme Speed
- Stomping Tantrum
- Ice Spinner
- Aerial Ace
";
        let team = verify_showdown_text(paste, &REG_M_B).expect("legal paste");
        assert_eq!(team.len(), 4);
    }
}
