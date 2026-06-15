//! Battle state machine.
//!
//! Phase 2 PR-1: real state, switches resolve, moves are accepted but
//! produce no damage. Each call to [`Battle::step`] increments the turn
//! counter and applies switches. End-of-turn effects (weather/status
//! residual damage) land in a later PR.

use crate::choice::{Choice, Target};
use crate::format::Format;
use crate::pokemon::Pokemon;
use crate::side::{Side, SideRef};

#[derive(Debug, Clone)]
pub struct BattleConfig {
    pub format: Format,
    pub seed: u64,
}

impl Default for BattleConfig {
    fn default() -> Self {
        Self { format: Format::Doubles, seed: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    Continue,
    Ended { winner: Option<SideRef> },
}

#[derive(Debug, Clone)]
pub struct Battle {
    pub config: BattleConfig,
    pub p1: Side,
    pub p2: Side,
    turn: u32,
    ended: Option<Option<SideRef>>,
}

impl Battle {
    pub fn new(config: BattleConfig, p1_team: Vec<Pokemon>, p2_team: Vec<Pokemon>) -> Self {
        let p1 = Side::new(p1_team, config.format);
        let p2 = Side::new(p2_team, config.format);
        Self { config, p1, p2, turn: 0, ended: None }
    }

    pub fn turn(&self) -> u32 {
        self.turn
    }

    pub fn seed(&self) -> u64 {
        self.config.seed
    }

    pub fn format(&self) -> Format {
        self.config.format
    }

    pub fn side(&self, side: SideRef) -> &Side {
        match side {
            SideRef::P1 => &self.p1,
            SideRef::P2 => &self.p2,
        }
    }

    fn side_mut(&mut self, side: SideRef) -> &mut Side {
        match side {
            SideRef::P1 => &mut self.p1,
            SideRef::P2 => &mut self.p2,
        }
    }

    /// Legal choices for one active slot on one side. Returns nothing if the
    /// slot is currently empty (the caller should issue `Pass`).
    ///
    /// **No allocation in step()** — this is a *pre-step* enumeration used
    /// by the agent loop, where heap is fine.
    pub fn legal_choices(&self, side: SideRef, actor_slot: u8) -> Vec<Choice> {
        let s = self.side(side);
        let slot = actor_slot as usize;
        let Some(active) = s.active_mon(slot) else {
            return vec![Choice::Pass { actor_slot }];
        };
        if !active.is_alive() {
            // Forced switch if any candidate, else pass.
            let switches: Vec<Choice> = s
                .switch_candidates(slot)
                .map(|team_index| Choice::Switch { actor_slot, team_index })
                .collect();
            if switches.is_empty() {
                return vec![Choice::Pass { actor_slot }];
            }
            return switches;
        }

        let mut out = Vec::with_capacity(8);
        // Moves: one per valid slot. Phase 2 PR-1 emits a single Move with
        // `target = None` for spread / self / ally-side moves, and one Move
        // per opposing active slot for `normal` / `adjacentFoe`.
        for (i, &move_id) in active.moves.iter().enumerate() {
            if move_id == u16::MAX || active.pp.get(i).copied().unwrap_or(0) == 0 {
                continue;
            }
            let m = &vgc_engine_data::MOVES[move_id as usize];
            let needs_pick = matches!(m.target, 0 | 4 | 10); // normal, adjacentFoe, any
            if needs_pick {
                for opp_slot in 0..self.config.format.active_count() as u8 {
                    if self
                        .side(side.opposing())
                        .active_mon(opp_slot as usize)
                        .is_some_and(|m| m.is_alive())
                    {
                        out.push(Choice::Move {
                            actor_slot,
                            move_slot: i as u8,
                            target: Some(Target { side: side.opposing(), slot: opp_slot }),
                        });
                    }
                }
            } else {
                out.push(Choice::Move {
                    actor_slot,
                    move_slot: i as u8,
                    target: None,
                });
            }
        }
        // Switches.
        for team_index in s.switch_candidates(slot) {
            out.push(Choice::Switch { actor_slot, team_index });
        }
        if out.is_empty() {
            // Struggle / no PP — encode later. For now emit Pass.
            out.push(Choice::Pass { actor_slot });
        }
        out
    }

    /// Advance the battle one turn.
    ///
    /// In doubles, callers pass `[choice_slot_a, choice_slot_b]` per side.
    /// Switches resolve in priority order (faster mon switches first — but
    /// switches are PS-priority +6 anyway so they all go before moves).
    /// Moves are accepted but currently produce no damage / status.
    pub fn step(
        &mut self,
        p1_choices: &[Choice],
        p2_choices: &[Choice],
    ) -> StepResult {
        if let Some(w) = self.ended {
            return StepResult::Ended { winner: w };
        }

        // 1. Apply switches first (PS treats switches as priority +6, before
        //    any move). Order within switches doesn't matter — they don't
        //    interact in Phase 2.
        self.apply_switches(SideRef::P1, p1_choices);
        self.apply_switches(SideRef::P2, p2_choices);

        // 2. Resolve moves — stub: do nothing. PP/damage land in next PR.
        //    We still tick PP for issued moves so multi-turn sims advance.
        self.tick_pp(SideRef::P1, p1_choices);
        self.tick_pp(SideRef::P2, p2_choices);

        // 3. End of turn: check loss conditions.
        self.turn = self.turn.saturating_add(1);
        let p1_dead = self.p1.is_defeated();
        let p2_dead = self.p2.is_defeated();
        let winner = match (p1_dead, p2_dead) {
            (true, true) => Some(None),
            (true, false) => Some(Some(SideRef::P2)),
            (false, true) => Some(Some(SideRef::P1)),
            (false, false) => None,
        };
        if let Some(w) = winner {
            self.ended = Some(w);
            return StepResult::Ended { winner: w };
        }
        StepResult::Continue
    }

    fn apply_switches(&mut self, side: SideRef, choices: &[Choice]) {
        for c in choices {
            if let Choice::Switch { actor_slot, team_index } = *c {
                let s = self.side_mut(side);
                if (actor_slot as usize) < s.active.len()
                    && (team_index as usize) < s.team.len()
                    && s.team[team_index as usize].is_alive()
                {
                    s.active[actor_slot as usize] = team_index;
                    // Reset boosts on the incoming mon (PS behavior).
                    s.team[team_index as usize].boosts = [0; 7];
                }
            }
        }
    }

    fn tick_pp(&mut self, side: SideRef, choices: &[Choice]) {
        for c in choices {
            if let Choice::Move { actor_slot, move_slot, .. } = *c {
                if let Some(mon) = self.side_mut(side).active_mon_mut(actor_slot as usize) {
                    if let Some(pp) = mon.pp.get_mut(move_slot as usize) {
                        *pp = pp.saturating_sub(1);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::TeamBuilder;

    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","protect","ironhead"],"evs":{"hp":4,"atk":252,"def":0,"spa":0,"spd":0,"spe":252}},
        {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]},
        {"species":"incineroar","level":50,"ability":"intimidate","item":"safetygoggles","nature":"adamant","moves":["fakeout","knockoff","flareblitz","partingshot"]}
    ]"#;
    const P2: &str = r#"[
        {"species":"flutter-mane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]},
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"]}
    ]"#;

    fn battle() -> Battle {
        let p1 = TeamBuilder::from_json(P1).expect("p1");
        let p2 = TeamBuilder::from_json(P2).expect("p2");
        Battle::new(BattleConfig::default(), p1, p2)
    }

    #[test]
    fn fixture_teams_load() {
        let b = battle();
        assert_eq!(b.p1.team.len(), 3);
        assert_eq!(b.p2.team.len(), 2);
        assert_eq!(b.p1.active, [0, 1]);
        assert_eq!(b.p2.active, [0, 1]);
    }

    #[test]
    fn switch_swaps_active_slot() {
        let mut b = battle();
        let r = b.step(
            &[Choice::Switch { actor_slot: 0, team_index: 2 }, Choice::Pass { actor_slot: 1 }],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert!(matches!(r, StepResult::Continue));
        assert_eq!(b.p1.active[0], 2, "garchomp swapped out for incineroar");
        assert_eq!(b.turn(), 1);
    }

    #[test]
    fn legal_choices_includes_moves_and_switches() {
        let b = battle();
        let cs = b.legal_choices(SideRef::P1, 0);
        // 4 moves × 2 opposing targets for the normal-targeting ones, plus
        // 1 entry per non-target move, plus switches to the one benched mon.
        // Exact count depends on move targeting; assert non-empty + has both kinds.
        assert!(cs.iter().any(|c| matches!(c, Choice::Move { .. })));
        assert!(cs.iter().any(|c| matches!(c, Choice::Switch { .. })));
    }

    #[test]
    fn pp_ticks_on_move_choice() {
        let mut b = battle();
        let before = b.p1.team[0].pp[0];
        b.step(
            &[
                Choice::Move { actor_slot: 0, move_slot: 0, target: Some(Target { side: SideRef::P2, slot: 0 }) },
                Choice::Pass { actor_slot: 1 },
            ],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert_eq!(b.p1.team[0].pp[0], before - 1);
    }
}
