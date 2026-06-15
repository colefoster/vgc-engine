//! Per-turn choice extractor.
//!
//! Walks a [`TurnView`] and emits each side's actions as engine
//! [`Choice`] values. The extractor tracks active-slot ↔ team-index
//! state between turns so a `Move` event on `p1a` always resolves to
//! the right `actor_slot`, even after mid-battle switches re-seat the
//! lineup.
//!
//! Output is `[Vec<Choice>; 2]` — slot-a choice first, then slot-b. If
//! a slot is empty (post-faint, awaiting forced switch) or the reconned
//! team has no entry for the named move, that choice is omitted; the
//! agreement scorer will count omissions as "diverged" rather than try
//! to fabricate a placeholder.
//!
//! Self-target moves (Protect family) are passed through with
//! `target = Some(<self>)`; the engine's resolver treats self-target
//! status moves specially, so this stays faithful to the log.

use vgc_engine_core::{Choice, MoveSlot, SideRef};

use vgc_engine_core::Target;

use crate::event::{Event, PokeSlot};
use crate::recon::parse_details;
use crate::replay::TurnView;
use crate::runner::RunnerInit;

/// Stateful per-replay extractor. Build with [`ChoiceExtractor::new`]
/// (which seeds the active-slot table from the runner's leads), then
/// call [`Self::extract_turn`] for each `TurnView` in order.
#[derive(Debug)]
pub struct ChoiceExtractor<'a> {
    init: &'a RunnerInit,
    /// `active[side_idx][slot_idx] = team_index`. 255 = empty.
    /// `side_idx`: 0 = p1, 1 = p2. `slot_idx`: 0 = a, 1 = b.
    active: [[u8; 2]; 2],
}

impl<'a> ChoiceExtractor<'a> {
    /// Initial active state mirrors `Side::new` — team indices 0/1 are
    /// the leads, since `RunnerInit::from_replay` reorders teams so the
    /// lead pair sits at the front.
    pub fn new(init: &'a RunnerInit) -> Self {
        // Singles formats only have slot a active; b stays 255.
        let n = match init.format {
            vgc_engine_core::Format::Singles => 1,
            vgc_engine_core::Format::Doubles => 2,
        };
        let mut active = [[255u8; 2]; 2];
        for side_row in active.iter_mut() {
            for (slot, cell) in side_row.iter_mut().enumerate().take(n) {
                *cell = slot as u8;
            }
        }
        Self { init, active }
    }

    /// Snapshot of which team index sits in each active slot. Useful
    /// for the scorer to anchor its own engine-side mirror.
    pub fn active(&self) -> [[u8; 2]; 2] {
        self.active
    }

    /// Walk a turn's events, mutating `active` on Switch/Drag/Faint and
    /// emitting one Choice per Move / Switch action observed.
    ///
    /// Returns `[p1_choices, p2_choices]`. Move events post-faint that
    /// can't be resolved are skipped (logged elsewhere).
    pub fn extract_turn(&mut self, turn: &TurnView<'_>) -> [Vec<Choice>; 2] {
        let mut p1: Vec<Choice> = Vec::new();
        let mut p2: Vec<Choice> = Vec::new();

        for ev in turn.events {
            match ev {
                Event::Move { user, move_name, target } => {
                    if let Some(choice) = self.build_move_choice(user, move_name, target.as_ref()) {
                        match user.player {
                            1 => p1.push(choice),
                            2 => p2.push(choice),
                            _ => {}
                        }
                    }
                }
                Event::Switch { slot, details, .. } => {
                    if let Some(choice) = self.handle_switch(slot, details) {
                        // A mid-turn switch is a player-issued choice the
                        // engine needs to see. The initial lead switches
                        // sit in turn 0 so they don't surface as choices
                        // there either — `current_turn_number == 0`
                        // distinction is handled by the caller (turn 0
                        // is skipped when feeding the engine).
                        match slot.player {
                            1 => p1.push(choice),
                            2 => p2.push(choice),
                            _ => {}
                        }
                    }
                }
                Event::Drag { slot, details, .. } => {
                    // Drag = forced switch from Whirlwind / Roar / Red
                    // Card — not a player choice. Update active state
                    // but don't emit a Choice.
                    let _ = self.handle_switch(slot, details);
                }
                Event::Faint(slot) => {
                    if let Some(p) = side_idx(slot.player)
                        && let Some(s) = slot_idx(slot.slot)
                    {
                        self.active[p][s] = 255;
                    }
                }
                _ => {}
            }
        }

        [p1, p2]
    }

    fn build_move_choice(
        &self,
        user: &PokeSlot,
        move_name: &str,
        target: Option<&PokeSlot>,
    ) -> Option<Choice> {
        let p = side_idx(user.player)?;
        let s = slot_idx(user.slot)?;
        let team_index = self.active[p][s];
        if team_index == 255 {
            return None;
        }
        let team = if user.player == 1 { &self.init.p1_team } else { &self.init.p2_team };
        let member = team.get(team_index as usize)?;
        let slug = move_slugify(move_name);
        let move_slot = member.moves.iter().position(|m| m == &slug)? as MoveSlot;
        let target = target.and_then(build_target);
        Some(Choice::Move {
            actor_slot: s as u8,
            move_slot,
            target,
        })
    }

    fn handle_switch(&mut self, slot: &PokeSlot, details: &str) -> Option<Choice> {
        let p = side_idx(slot.player)?;
        let s = slot_idx(slot.slot)?;
        let species = parse_details(details).species;
        let team = if slot.player == 1 { &self.init.p1_team } else { &self.init.p2_team };
        let team_index = team.iter().position(|m| m.species == species)? as u8;
        let prev = self.active[p][s];
        self.active[p][s] = team_index;
        if prev == team_index {
            // First time seating a lead — not a player-issued switch.
            return None;
        }
        Some(Choice::Switch {
            actor_slot: s as u8,
            team_index,
        })
    }
}

fn build_target(slot: &PokeSlot) -> Option<Target> {
    Some(Target {
        side: match slot.player {
            1 => SideRef::P1,
            2 => SideRef::P2,
            _ => return None,
        },
        slot: slot_idx(slot.slot)? as u8,
    })
}

fn side_idx(player: u8) -> Option<usize> {
    match player {
        1 => Some(0),
        2 => Some(1),
        _ => None,
    }
}

fn slot_idx(c: char) -> Option<usize> {
    match c {
        'a' => Some(0),
        'b' => Some(1),
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
    use crate::recon::{CanonicalDefault, PokeObservation, ReconInput, TeamRecon};
    use vgc_engine_core::Format;

    fn obs(species: &str, moves: &[&str]) -> PokeObservation {
        PokeObservation {
            species: species.into(),
            level: 50,
            gender: '\0',
            ability: None,
            item: None,
            moves: moves.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn fake_init() -> RunnerInit {
        let p1 = CanonicalDefault
            .reconstruct(&ReconInput {
                player: 1,
                mons: vec![
                    obs("sneasler", &["fakeout", "rockslide", "closecombat"]),
                    obs("archaludon", &["dracometeor"]),
                    obs("dragonite", &["hurricane"]),
                ],
            })
            .unwrap();
        let p2 = CanonicalDefault
            .reconstruct(&ReconInput {
                player: 2,
                mons: vec![
                    obs("talonflame", &["protect", "acrobatics"]),
                    obs("garchomp", &["protect", "stoneedge"]),
                ],
            })
            .unwrap();
        RunnerInit {
            format: Format::Doubles,
            p1_team: p1,
            p2_team: p2,
        }
    }

    fn slot(player: u8, slot: char, nick: &str) -> PokeSlot {
        PokeSlot { player, slot, nickname: nick.into() }
    }

    #[test]
    fn extract_move_with_target() {
        let init = fake_init();
        let mut ex = ChoiceExtractor::new(&init);
        let turn = TurnView {
            number: 1,
            events: &[Event::Move {
                user: slot(1, 'a', "Sneasler"),
                move_name: "Fake Out".into(),
                target: Some(slot(2, 'a', "Talonflame")),
            }],
        };
        let [p1, p2] = ex.extract_turn(&turn);
        assert_eq!(p2.len(), 0);
        assert_eq!(p1.len(), 1);
        match p1[0] {
            Choice::Move { actor_slot, move_slot, target } => {
                assert_eq!(actor_slot, 0);
                assert_eq!(move_slot, 0); // fakeout is moves[0]
                let t = target.unwrap();
                assert_eq!(t.side, SideRef::P2);
                assert_eq!(t.slot, 0);
            }
            _ => panic!("expected Move"),
        }
    }

    #[test]
    fn extract_skips_unknown_move() {
        let init = fake_init();
        let mut ex = ChoiceExtractor::new(&init);
        let turn = TurnView {
            number: 1,
            events: &[Event::Move {
                user: slot(1, 'a', "Sneasler"),
                move_name: "Earthquake".into(), // not in Sneasler's observed kit
                target: None,
            }],
        };
        let [p1, _] = ex.extract_turn(&turn);
        assert!(p1.is_empty());
    }

    #[test]
    fn extract_switch_updates_active_and_emits_choice() {
        let init = fake_init();
        let mut ex = ChoiceExtractor::new(&init);
        let turn = TurnView {
            number: 2,
            events: &[Event::Switch {
                slot: slot(1, 'b', "Dragonite"),
                details: "Dragonite, L50, M".into(),
                hp: "100/100".into(),
            }],
        };
        let [p1, _] = ex.extract_turn(&turn);
        assert_eq!(p1.len(), 1);
        match p1[0] {
            Choice::Switch { actor_slot, team_index } => {
                assert_eq!(actor_slot, 1);
                assert_eq!(team_index, 2); // dragonite at index 2
            }
            _ => panic!("expected Switch"),
        }
        assert_eq!(ex.active()[0][1], 2);
    }

    #[test]
    fn extract_drag_updates_active_but_no_choice() {
        let init = fake_init();
        let mut ex = ChoiceExtractor::new(&init);
        let turn = TurnView {
            number: 2,
            events: &[Event::Drag {
                slot: slot(2, 'a', "Garchomp"),
                details: "Garchomp, L50, M".into(),
                hp: "100/100".into(),
            }],
        };
        let [_, p2] = ex.extract_turn(&turn);
        assert!(p2.is_empty(), "drag is not a player choice");
        assert_eq!(ex.active()[1][0], 1); // garchomp now in slot p2a
    }

    #[test]
    fn faint_marks_slot_empty() {
        let init = fake_init();
        let mut ex = ChoiceExtractor::new(&init);
        let turn = TurnView {
            number: 2,
            events: &[Event::Faint(slot(2, 'a', "Talonflame"))],
        };
        let _ = ex.extract_turn(&turn);
        assert_eq!(ex.active()[1][0], 255);
    }
}
