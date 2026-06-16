//! Team reconstruction from replay observations.
//!
//! PS public-ladder replays don't include EVs/IVs/natures — only species,
//! level, gender, and the moves/items that actually surface in the log.
//! The [`TeamRecon`] trait lets the differential harness plug in a
//! reconstruction strategy:
//!
//! * [`CanonicalDefault`] — pure heuristic from base stats (this PR).
//! * `MimikyuGuesser` (future) — ML-driven set inference.
//! * `MinDivergenceFit` (future) — solve EVs/IVs that best match observed
//!   damage / speed-tier outcomes.
//!
//! Recon output is a `Vec<TeamMember>` consumable by
//! [`vgc_engine_core::TeamBuilder`]. Observed moves / items / abilities
//! land in the input — strategies are free to override or ignore them.

use serde::{Deserialize, Serialize};
use vgc_engine_core::{StatSpread, TeamMember};
use vgc_engine_data as data;

use crate::event::{Event, PokeSlot};
use crate::replay::TeamPreviewPoke;

/// Per-Pokémon known/observed facts that any reconstruction strategy can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokeObservation {
    /// Species slug, e.g. `"pelipper"`. Required.
    pub species: String,
    /// Pokémon level. Defaults to 50 in VGC.
    pub level: u8,
    /// Gender letter from team preview (`'M'`, `'F'`, `'\0'` for genderless).
    pub gender: char,
    /// Ability slug if observed via `|-ability|` or implied by trigger
    /// (e.g. Drizzle starting Rain). `None` if not yet seen.
    pub ability: Option<String>,
    /// Item slug if observed via `|-enditem|`, `|-item|`, or activation.
    pub item: Option<String>,
    /// Moves observed via `|move|` events. May be < 4 if the mon didn't
    /// use its full kit in the replay.
    pub moves: Vec<String>,
}

/// Reconstruction input for one side of a battle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconInput {
    pub player: u8,
    pub mons: Vec<PokeObservation>,
}

#[derive(Debug)]
pub enum ReconError {
    UnknownSpecies(String),
}

impl core::fmt::Display for ReconError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownSpecies(s) => write!(f, "unknown species: {s}"),
        }
    }
}

impl std::error::Error for ReconError {}

/// Strategy: turn observations into a fully-specified team spec.
pub trait TeamRecon {
    fn reconstruct(&self, input: &ReconInput) -> Result<Vec<TeamMember>, ReconError>;
}

/// Pure-heuristic baseline. Picks a nature + EV spread from base stats,
/// max IVs across the board. Leaves ability/item as the runner provided
/// them (None if unobserved) and forwards observed moves verbatim.
///
/// Heuristic:
/// * Offensive stat = whichever of Atk/SpA has the higher base. (Tie → Atk.)
/// * If base Spe ≥ offensive base → Jolly / Timid (speed positive,
///   weaker offense negative).
/// * Else → Adamant / Modest (offense positive, weaker offense negative).
/// * EVs: 252 in offensive stat, 252 in Spe, 4 in HP.
///
/// Crude but deterministic — matches the "Choice Band sweeper" / "Choice
/// Scarf revenge killer" archetypes that dominate the ladder. Will lose
/// agreement points on bulky support sets; that's the gap the mimikyu
/// guesser is meant to close.
#[derive(Debug, Default, Clone, Copy)]
pub struct CanonicalDefault;

impl TeamRecon for CanonicalDefault {
    fn reconstruct(&self, input: &ReconInput) -> Result<Vec<TeamMember>, ReconError> {
        input.mons.iter().map(build_member_spec).collect()
    }
}

fn build_member_spec(obs: &PokeObservation) -> Result<TeamMember, ReconError> {
    let species = data::species_by_slug(&obs.species)
        .ok_or_else(|| ReconError::UnknownSpecies(obs.species.clone()))?;

    // base_stats order: hp, atk, def, spa, spd, spe.
    let bs = &species.base_stats;
    let atk = bs[1];
    let spa = bs[3];
    let spe = bs[5];
    let physical = atk >= spa;
    let (off_pos, off_neg) = if physical { ("atk", "spa") } else { ("spa", "atk") };
    let off_base = if physical { atk } else { spa };
    let speedy = spe >= off_base;
    let (nature, evs) = if speedy {
        let nat = if physical { "jolly" } else { "timid" };
        let mut evs = StatSpread { hp: 4, atk: 0, def: 0, spa: 0, spd: 0, spe: 252 };
        if physical { evs.atk = 252 } else { evs.spa = 252 }
        (nat, evs)
    } else {
        let nat = if physical { "adamant" } else { "modest" };
        let mut evs = StatSpread { hp: 4, atk: 0, def: 0, spa: 0, spd: 0, spe: 252 };
        if physical { evs.atk = 252 } else { evs.spa = 252 }
        (nat, evs)
    };
    let _ = (off_pos, off_neg); // documentation aid; nature slug encodes both

    Ok(TeamMember {
        species: obs.species.clone(),
        level: obs.level,
        ability: obs.ability.clone(),
        item: obs.item.clone(),
        nature: nature.to_string(),
        moves: obs.moves.clone(),
        ivs: StatSpread::MAX_IV,
        evs,
        teratype: None,
    })
}

/// Parse a team-preview `details` string. PS format (see
/// `sim/SIM-PROTOCOL.md`, section "Team preview"):
///
/// ```text
/// <species[-form]>, L<level>, <gender>, shiny, tera:<type>
/// ```
///
/// All segments after the species are optional and may appear in any order.
/// Trailing empty segments (the protocol sometimes emits `|poke|...|` with
/// a trailing item-marker field) are tolerated.
pub fn parse_details(details: &str) -> PokeObservation {
    let mut species = String::new();
    let mut level: u8 = 50;
    let mut gender: char = '\0';
    for (i, seg) in details.split(',').map(str::trim).enumerate() {
        if i == 0 {
            species = slugify_species(seg);
            continue;
        }
        if let Some(lv) = seg.strip_prefix('L')
            && let Ok(n) = lv.parse::<u8>()
        {
            level = n;
            continue;
        }
        if seg == "M" || seg == "F" {
            gender = seg.chars().next().unwrap();
            continue;
        }
        // 'shiny' / 'tera:<type>' / unknown — ignore for now.
    }
    PokeObservation {
        species,
        level,
        gender,
        ability: None,
        item: None,
        moves: Vec::new(),
    }
}

fn slugify_species(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Convenience: build a [`ReconInput`] from a side's team-preview entries.
/// Observed moves/items/abilities are left empty; call
/// [`observe_events`] to fill them in from the event stream.
pub fn input_from_team_preview(player: u8, preview: &[TeamPreviewPoke]) -> ReconInput {
    let mons = preview
        .iter()
        .filter(|p| p.player == player)
        .map(|p| parse_details(&p.details))
        .collect();
    ReconInput { player, mons }
}

/// Walk the event stream and populate each side's [`PokeObservation`] with
/// the moves / items / abilities the replay actually reveals.
///
/// Returns `[Option<ReconInput>; 2]`, one entry per player (index 0 = p1,
/// index 1 = p2). Each is `Some` if the team-preview contained mons for
/// that player, `None` otherwise.
///
/// Strategy: maintain a slot-letter → species map per player, updated on
/// every `Switch`/`Drag`. Each `Move`/`Ability`/`Item`/`EndItem` event is
/// then attributed to the species currently in that slot. The match against
/// the team-preview is by species slug.
///
/// Observed moves are capped at 4 (PS protocol guarantees no mon legally
/// reveals more); duplicates are collapsed. Items/abilities are recorded
/// on first sight; later changes (Trick, Skill Swap, Mega evolution) are
/// ignored — they'd corrupt the inferred original set.
pub fn observe_events(
    events: &[Event],
    preview: &[TeamPreviewPoke],
) -> [Option<ReconInput>; 2] {
    let mut p1 = input_from_team_preview(1, preview);
    let mut p2 = input_from_team_preview(2, preview);

    // active[player_idx][slot_letter as usize - 'a'] = species_slug, or None.
    // Slot letters in doubles are 'a' or 'b'; we size 4 to be safe (triples
    // / horde formats use 'c'/'d' historically — defensive only).
    let mut active: [[Option<String>; 4]; 2] = Default::default();

    for ev in events {
        match ev {
            Event::Switch { slot, details, .. } | Event::Drag { slot, details, .. } => {
                let species = parse_details(details).species;
                if let Some(cell) = slot_cell(&mut active, slot) {
                    *cell = Some(species);
                }
            }
            Event::Faint(slot) => {
                if let Some(cell) = slot_cell(&mut active, slot) {
                    *cell = None;
                }
            }
            Event::Move { user, move_name, .. } => {
                if let Some(species) = current_species(&active, user)
                    && let Some(input) = input_for(user.player, &mut p1, &mut p2)
                    && let Some(mon) = input.mons.iter_mut().find(|m| m.species == species)
                {
                    let slug = move_slugify(move_name);
                    // Skip Struggle (auto-generated when no PP remains).
                    if slug == "struggle" {
                        continue;
                    }
                    if !mon.moves.iter().any(|m| m == &slug) && mon.moves.len() < 4 {
                        mon.moves.push(slug);
                    }
                }
            }
            Event::Ability { slot, ability, .. } => {
                attribute_ability(&active, slot, ability, &mut p1, &mut p2);
            }
            // `|-weather|RainDance|[from] ability: Drizzle|[of] p1b: Pelipper`
            // and `|-fieldstart|move: Electric Terrain|[from] ability: Electric Surge|[of] ...`
            // reveal abilities without a standalone `|-ability|` event.
            Event::Weather { from: Some(from_str), of: Some(of_slot), .. }
            | Event::FieldStart { from: Some(from_str), of: Some(of_slot), .. } => {
                if let Some(ability) = from_str.strip_prefix("ability: ") {
                    attribute_ability(&active, of_slot, ability, &mut p1, &mut p2);
                }
            }
            Event::Item { slot, item, .. } | Event::EndItem { slot, item, .. } => {
                if let Some(species) = current_species(&active, slot)
                    && let Some(input) = input_for(slot.player, &mut p1, &mut p2)
                    && let Some(mon) = input.mons.iter_mut().find(|m| m.species == species)
                    && mon.item.is_none()
                {
                    mon.item = Some(move_slugify(item));
                }
            }
            _ => {}
        }
    }

    [
        (!p1.mons.is_empty()).then_some(p1),
        (!p2.mons.is_empty()).then_some(p2),
    ]
}

fn attribute_ability(
    active: &[[Option<String>; 4]; 2],
    slot: &PokeSlot,
    ability: &str,
    p1: &mut ReconInput,
    p2: &mut ReconInput,
) {
    if let Some(species) = current_species(active, slot)
        && let Some(input) = input_for(slot.player, p1, p2)
        && let Some(mon) = input.mons.iter_mut().find(|m| m.species == species)
        && mon.ability.is_none()
    {
        mon.ability = Some(move_slugify(ability));
    }
}

fn slot_cell<'a>(
    active: &'a mut [[Option<String>; 4]; 2],
    slot: &PokeSlot,
) -> Option<&'a mut Option<String>> {
    let p = (slot.player as usize).checked_sub(1)?;
    if p >= 2 {
        return None;
    }
    let idx = (slot.slot as u8).checked_sub(b'a')? as usize;
    active[p].get_mut(idx)
}

fn current_species(active: &[[Option<String>; 4]; 2], slot: &PokeSlot) -> Option<String> {
    let p = (slot.player as usize).checked_sub(1)?;
    if p >= 2 {
        return None;
    }
    let idx = (slot.slot as u8).checked_sub(b'a')? as usize;
    active[p].get(idx).and_then(|s| s.clone())
}

fn input_for<'a>(
    player: u8,
    p1: &'a mut ReconInput,
    p2: &'a mut ReconInput,
) -> Option<&'a mut ReconInput> {
    match player {
        1 => Some(p1),
        2 => Some(p2),
        _ => None,
    }
}

fn move_slugify(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_full() {
        let o = parse_details("Pelipper, L50, F");
        assert_eq!(o.species, "pelipper");
        assert_eq!(o.level, 50);
        assert_eq!(o.gender, 'F');
    }

    #[test]
    fn details_genderless() {
        let o = parse_details("Maushold, L50");
        assert_eq!(o.species, "maushold");
        assert_eq!(o.level, 50);
        assert_eq!(o.gender, '\0');
    }

    #[test]
    fn details_form_slug() {
        let o = parse_details("Floette-Eternal, L50, F");
        assert_eq!(o.species, "floetteeternal");
    }

    #[test]
    fn canonical_garchomp_is_adamant_attacker() {
        // Garchomp: atk 130 > spa 80, spe 102 < atk — Adamant attacker.
        // Wait: heuristic compares spe to off_base. spe=102, atk=130, so
        // spe < off_base → Adamant. Verify.
        let recon = CanonicalDefault;
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "garchomp".into(),
                level: 50,
                gender: 'M',
                ability: None,
                item: None,
                moves: vec![],
            }],
        };
        let team = recon.reconstruct(&input).unwrap();
        assert_eq!(team[0].nature, "adamant");
        assert_eq!(team[0].evs.atk, 252);
        assert_eq!(team[0].evs.spe, 252);
    }

    #[test]
    fn canonical_pelipper_is_modest_specialist() {
        // Pelipper: spa 95 > atk 50, spe 65 < spa — Modest.
        let recon = CanonicalDefault;
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "pelipper".into(),
                level: 50,
                gender: 'F',
                ability: Some("drizzle".into()),
                item: None,
                moves: vec![],
            }],
        };
        let team = recon.reconstruct(&input).unwrap();
        assert_eq!(team[0].nature, "modest");
        assert_eq!(team[0].evs.spa, 252);
        assert_eq!(team[0].ability.as_deref(), Some("drizzle"));
    }

    #[test]
    fn canonical_dragapult_is_timid_speed() {
        // Dragapult: spa 100 ≥ atk 120? actually atk 120 > spa 100, so
        // physical. spe 142 > atk 120 → speed-positive → Jolly.
        let recon = CanonicalDefault;
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "dragapult".into(),
                level: 50,
                gender: 'M',
                ability: None,
                item: None,
                moves: vec![],
            }],
        };
        let team = recon.reconstruct(&input).unwrap();
        assert_eq!(team[0].nature, "jolly");
    }

    #[test]
    fn unknown_species_errors() {
        let recon = CanonicalDefault;
        let input = ReconInput {
            player: 1,
            mons: vec![PokeObservation {
                species: "fakemon".into(),
                level: 50,
                gender: '\0',
                ability: None,
                item: None,
                moves: vec![],
            }],
        };
        assert!(matches!(
            recon.reconstruct(&input),
            Err(ReconError::UnknownSpecies(_))
        ));
    }

    #[test]
    fn input_from_team_preview_filters_player() {
        let preview = vec![
            TeamPreviewPoke { player: 1, details: "Pelipper, L50, F".into() },
            TeamPreviewPoke { player: 2, details: "Garchomp, L50, M".into() },
            TeamPreviewPoke { player: 1, details: "Dragonite, L50, M".into() },
        ];
        let inp = input_from_team_preview(1, &preview);
        assert_eq!(inp.mons.len(), 2);
        assert_eq!(inp.mons[0].species, "pelipper");
        assert_eq!(inp.mons[1].species, "dragonite");
    }
}
