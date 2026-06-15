//! Single-replay runner: turn a parsed [`Replay`] into a runnable
//! [`Battle`].
//!
//! Pipeline:
//!   replay → `observe_events` → `TeamRecon` → `Vec<TeamMember>` per side
//!   → leads detected from first switches → `build_member` per mon →
//!   `Battle::new`.
//!
//! Per-turn diff & agreement scoring lands in a follow-up PR. This PR
//! is the load-bearing init step: it must succeed on real corpus data
//! before any differential test can run.

use vgc_engine_core::{
    build_member,
    battle::{Battle, BattleConfig},
    Format, TeamLoadError, TeamMember,
};

use crate::event::Event;
use crate::recon::{observe_events, ReconError, ReconInput, TeamRecon};
use crate::replay::Replay;

#[derive(Debug)]
pub enum RunnerError {
    Recon(ReconError),
    TeamLoad(TeamLoadError),
    MissingSide(u8),
    MissingLead { player: u8, slot: char },
}

impl core::fmt::Display for RunnerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Recon(e) => write!(f, "recon: {e}"),
            Self::TeamLoad(e) => write!(f, "team load: {e}"),
            Self::MissingSide(p) => write!(f, "missing team-preview data for p{p}"),
            Self::MissingLead { player, slot } => {
                write!(f, "no lead found for p{player}{slot}")
            }
        }
    }
}

impl std::error::Error for RunnerError {}

impl From<ReconError> for RunnerError {
    fn from(e: ReconError) -> Self { Self::Recon(e) }
}
impl From<TeamLoadError> for RunnerError {
    fn from(e: TeamLoadError) -> Self { Self::TeamLoad(e) }
}

/// Reconstructed inputs ready to feed `Battle::new`. Teams are reordered
/// so the replay's lead-pair occupies indices 0 (slot a) and 1 (slot b),
/// matching `Side::new`'s convention.
#[derive(Debug)]
pub struct RunnerInit {
    pub format: Format,
    pub p1_team: Vec<TeamMember>,
    pub p2_team: Vec<TeamMember>,
}

impl RunnerInit {
    /// Build the init bundle from a parsed replay + a reconstruction
    /// strategy. The strategy fills in EVs/IVs/natures; observed
    /// moves/items/abilities are merged in beforehand via
    /// [`observe_events`].
    pub fn from_replay(
        replay: &Replay,
        recon: &impl TeamRecon,
    ) -> Result<Self, RunnerError> {
        let [p1_obs, p2_obs] = observe_events(&replay.events, &replay.team_preview);
        let p1_obs = p1_obs.ok_or(RunnerError::MissingSide(1))?;
        let p2_obs = p2_obs.ok_or(RunnerError::MissingSide(2))?;

        let mut p1_team = recon.reconstruct(&p1_obs)?;
        let mut p2_team = recon.reconstruct(&p2_obs)?;

        let (p1_leads, p2_leads) = detect_leads(&replay.events);
        reorder_leads(&mut p1_team, &p1_obs, p1_leads, 1)?;
        reorder_leads(&mut p2_team, &p2_obs, p2_leads, 2)?;

        // Doubles is the only format the corpus targets. Singles falls out
        // of the same machinery once we have a singles fixture.
        let format = match replay.gametype.as_deref() {
            Some("singles") => Format::Singles,
            _ => Format::Doubles,
        };

        Ok(RunnerInit { format, p1_team, p2_team })
    }

    /// Instantiate a `Battle` with the given RNG seed. Returns the same
    /// `TeamLoadError` family as `TeamBuilder::from_json` if any
    /// reconstructed move/ability/item slug isn't in the dex.
    pub fn into_battle(self, seed: u64) -> Result<Battle, RunnerError> {
        let p1: Result<Vec<_>, _> = self.p1_team.iter().map(build_member).collect();
        let p2: Result<Vec<_>, _> = self.p2_team.iter().map(build_member).collect();
        let cfg = BattleConfig { format: self.format, seed };
        Ok(Battle::new(cfg, p1?, p2?))
    }
}

/// Lead-pair detected per side as `[species_slug; 2]` for slot a and b.
/// Missing-slot entries are `String::new()`.
type Leads = [String; 2];

/// Walk events up to and including the lead switches at battle start.
/// The first `|switch|pNa: ...` and `|switch|pNb: ...` per player are
/// the leads. Any `|switch|` after `|turn|1` is a mid-battle swap and is
/// ignored.
fn detect_leads(events: &[Event]) -> (Leads, Leads) {
    let mut p1: Leads = Default::default();
    let mut p2: Leads = Default::default();
    let mut filled = 0;
    for ev in events {
        match ev {
            Event::Turn(_) if filled >= 4 => break,
            Event::Switch { slot, details, .. } => {
                let species = crate::recon::parse_details(details).species;
                let (target, idx) = match (slot.player, slot.slot) {
                    (1, 'a') => (&mut p1, 0),
                    (1, 'b') => (&mut p1, 1),
                    (2, 'a') => (&mut p2, 0),
                    (2, 'b') => (&mut p2, 1),
                    _ => continue,
                };
                if target[idx].is_empty() {
                    target[idx] = species;
                    filled += 1;
                }
            }
            _ => {}
        }
    }
    (p1, p2)
}

/// Reorder `team` in-place so the lead species occupy indices 0 and 1.
/// The lead-species lookup uses observation order from team-preview as
/// the tiebreaker if a species appears twice (it shouldn't under VGC's
/// Species Clause, but be defensive).
fn reorder_leads(
    team: &mut [TeamMember],
    _obs: &ReconInput,
    leads: Leads,
    player: u8,
) -> Result<(), RunnerError> {
    let lead_a_idx = find_unique(team, &leads[0], 0).ok_or(RunnerError::MissingLead {
        player,
        slot: 'a',
    })?;
    if lead_a_idx != 0 {
        team.swap(0, lead_a_idx);
    }
    // Singles: only slot a needs to be a lead.
    if leads[1].is_empty() {
        return Ok(());
    }
    let lead_b_idx = find_unique(team, &leads[1], 1).ok_or(RunnerError::MissingLead {
        player,
        slot: 'b',
    })?;
    if lead_b_idx != 1 {
        team.swap(1, lead_b_idx);
    }
    Ok(())
}

fn find_unique(team: &[TeamMember], species: &str, start: usize) -> Option<usize> {
    team.iter()
        .enumerate()
        .skip(start)
        .find(|(_, m)| m.species == species)
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recon::{CanonicalDefault, PokeObservation};

    fn dummy_member(species: &str) -> TeamMember {
        // Bypass JSON round-trip by reusing CanonicalDefault on a 1-mon obs.
        CanonicalDefault
            .reconstruct(&ReconInput {
                player: 1,
                mons: vec![obs(species)],
            })
            .unwrap()
            .pop()
            .unwrap()
    }

    fn obs(species: &str) -> PokeObservation {
        PokeObservation {
            species: species.into(),
            level: 50,
            gender: '\0',
            ability: None,
            item: None,
            moves: vec![],
        }
    }

    #[test]
    fn reorder_leads_swaps_in_place() {
        let mut team = vec![
            dummy_member("garchomp"),
            dummy_member("pelipper"),
            dummy_member("sneasler"),
            dummy_member("dragonite"),
        ];
        let input = ReconInput {
            player: 1,
            mons: vec![obs("garchomp"), obs("pelipper"), obs("sneasler"), obs("dragonite")],
        };
        let leads = ["sneasler".into(), "pelipper".into()];
        reorder_leads(&mut team, &input, leads, 1).unwrap();
        assert_eq!(team[0].species, "sneasler");
        assert_eq!(team[1].species, "pelipper");
    }

    #[test]
    fn detect_leads_picks_first_four_switches() {
        let events = vec![
            Event::Switch {
                slot: crate::event::PokeSlot { player: 1, slot: 'a', nickname: "Sneasler".into() },
                details: "Sneasler, L50, M".into(),
                hp: "100/100".into(),
            },
            Event::Switch {
                slot: crate::event::PokeSlot { player: 1, slot: 'b', nickname: "Archaludon".into() },
                details: "Archaludon, L50, F".into(),
                hp: "100/100".into(),
            },
            Event::Switch {
                slot: crate::event::PokeSlot { player: 2, slot: 'a', nickname: "Garchomp".into() },
                details: "Garchomp, L50, M".into(),
                hp: "100/100".into(),
            },
            Event::Switch {
                slot: crate::event::PokeSlot { player: 2, slot: 'b', nickname: "Talonflame".into() },
                details: "Talonflame, L50, F".into(),
                hp: "100/100".into(),
            },
            Event::Turn(1),
            // Mid-battle swap that must NOT be picked up as a lead.
            Event::Switch {
                slot: crate::event::PokeSlot { player: 1, slot: 'b', nickname: "Pelipper".into() },
                details: "Pelipper, L50, F".into(),
                hp: "100/100".into(),
            },
        ];
        let (p1, p2) = detect_leads(&events);
        assert_eq!(p1, ["sneasler", "archaludon"]);
        assert_eq!(p2, ["garchomp", "talonflame"]);
    }

    #[test]
    fn unknown_lead_species_errors() {
        let mut team = vec![dummy_member("garchomp")];
        let input = ReconInput { player: 1, mons: vec![obs("garchomp")] };
        let leads = ["pelipper".into(), String::new()];
        assert!(matches!(
            reorder_leads(&mut team, &input, leads, 1),
            Err(RunnerError::MissingLead { player: 1, slot: 'a' })
        ));
    }
}
