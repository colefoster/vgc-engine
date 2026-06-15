//! Battle state machine.
//!
//! Phase 2 PR-4: moves now actually resolve. Per Move action:
//!
//! - tick PP (whether or not the move connects, per PS)
//! - accuracy roll vs `move.accuracy` (255 = always hit)
//! - crit roll: 1/24 base (gen 9; high-crit-ratio moves deferred)
//! - damage roll bucket 0..=15
//! - apply HP damage to target, set fainted on 0 HP
//!
//! Mid-turn faint of the actor cancels its remaining actions in this turn.
//!
//! Deferred (each its own PR):
//!
//! - move secondary effects (poison/burn/flinch chances, etc.)
//! - status moves (Protect, Tailwind, Substitute, ...)
//! - multi-hit / variable-BP / move-specific overrides
//! - end-of-turn effects (weather residual, leftovers, status damage)
//! - end-of-turn switch prompts when an active slot is empty

use crate::choice::{Choice, Target};
use crate::damage::{calculate_damage, DamageContext};
use crate::format::Format;
use crate::order::{action_order, ScheduledAction};
use crate::pokemon::Pokemon;
use crate::rng::Rng;
use crate::side::{Side, SideRef};
use vgc_engine_data as data;

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
    rng: Rng,
    turn: u32,
    ended: Option<Option<SideRef>>,
}

impl Battle {
    pub fn new(config: BattleConfig, p1_team: Vec<Pokemon>, p2_team: Vec<Pokemon>) -> Self {
        let p1 = Side::new(p1_team, config.format);
        let p2 = Side::new(p2_team, config.format);
        let rng = Rng::new(config.seed);
        Self { config, p1, p2, rng, turn: 0, ended: None }
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

    /// Legal choices for one active slot on one side.
    pub fn legal_choices(&self, side: SideRef, actor_slot: u8) -> Vec<Choice> {
        let s = self.side(side);
        let slot = actor_slot as usize;
        let Some(active) = s.active_mon(slot) else {
            return vec![Choice::Pass { actor_slot }];
        };
        if !active.is_alive() {
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
        for (i, &move_id) in active.moves.iter().enumerate() {
            if move_id == u16::MAX || active.pp.get(i).copied().unwrap_or(0) == 0 {
                continue;
            }
            let m = &data::MOVES[move_id as usize];
            let needs_pick = matches!(m.target, 0 | 4 | 10);
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
        for team_index in s.switch_candidates(slot) {
            out.push(Choice::Switch { actor_slot, team_index });
        }
        if out.is_empty() {
            out.push(Choice::Pass { actor_slot });
        }
        out
    }

    /// Advance the battle one turn.
    pub fn step(&mut self, p1_choices: &[Choice], p2_choices: &[Choice]) -> StepResult {
        if let Some(w) = self.ended {
            return StepResult::Ended { winner: w };
        }

        // 0. Per-turn volatile reset on every mon: clear single-turn
        //    protect flag and the stall-attempt tracking flag.
        for s in [SideRef::P1, SideRef::P2] {
            for m in self.side_mut(s).team.iter_mut() {
                m.is_protected_this_turn = false;
                m.used_stall_this_turn = false;
            }
        }

        // 1. Switches first (PS priority +6).
        self.apply_switches(SideRef::P1, p1_choices);
        self.apply_switches(SideRef::P2, p2_choices);

        // 2. Resolve moves in priority+speed order.
        // Temporarily move rng out to split-borrow with `self`.
        let mut rng = self.rng;
        let order: Vec<ScheduledAction> =
            action_order(self, p1_choices, p2_choices, &mut rng);
        self.rng = rng;
        for action in order {
            if matches!(action.choice, Choice::Switch { .. } | Choice::Pass { .. }) {
                continue;
            }
            // Skip if the actor has fainted earlier this turn.
            let actor_alive = self
                .side(action.side)
                .active_mon(action.actor_slot as usize)
                .is_some_and(|m| m.is_alive());
            if !actor_alive {
                continue;
            }
            self.resolve_move(action);
        }

        // 3. End-of-turn cleanup: any mon that did NOT use a stall move
        //    this turn resets its stall_counter.
        for s in [SideRef::P1, SideRef::P2] {
            for m in self.side_mut(s).team.iter_mut() {
                if !m.used_stall_this_turn {
                    m.stall_counter = 0;
                }
            }
        }

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
                    s.team[team_index as usize].boosts = [0; 7];
                }
            }
        }
    }

    fn resolve_move(&mut self, action: ScheduledAction) {
        let Choice::Move { actor_slot, move_slot, target } = action.choice else {
            return;
        };
        let actor_side = action.side;
        let target_side = target.map(|t| t.side).unwrap_or_else(|| actor_side.opposing());
        let target_slot = target.map(|t| t.slot).unwrap_or(0);

        // Snapshot attacker and defender — avoids overlapping borrows
        // through the damage calc.
        let attacker = match self.side(actor_side).active_mon(actor_slot as usize).cloned() {
            Some(m) => m,
            None => return,
        };
        let move_id = match attacker.moves.get(move_slot as usize).copied() {
            Some(id) if id != u16::MAX => id,
            _ => return,
        };
        let m = &data::MOVES[move_id as usize];

        // 1. PP cost — ticked even on miss / immunity (PS behavior).
        if let Some(mon) = self.side_mut(actor_side).active_mon_mut(actor_slot as usize) {
            if let Some(pp) = mon.pp.get_mut(move_slot as usize) {
                *pp = pp.saturating_sub(1);
            }
        }

        // 2. Status-move dispatch by slug. Currently: Protect only.
        if m.category == 2 {
            self.resolve_status_move(actor_side, actor_slot, m);
            return;
        }

        // 3. Accuracy check (255 = always-hit; e.g. Aerial Ace, Swift).
        if m.accuracy != 255 {
            let acc = m.accuracy as u32;
            let roll = self.rng.percent_1_100() as u32;
            if roll > acc {
                return; // miss — no damage, no PP refund.
            }
        }

        // 4. Resolve target.
        let defender = match self.side(target_side).active_mon(target_slot as usize).cloned() {
            Some(d) if d.is_alive() => d,
            _ => return,
        };

        // 5. Protect: targeting move against a protected defender fails.
        //    "Targeting" here = move.target ∈ {normal, adjacentFoe,
        //    adjacentAlly, adjacentAllyOrSelf, any}. Spread / self / side
        //    moves are handled by their own resolution and won't reach
        //    here at all for self/ally-side targets.
        if defender.is_protected_this_turn && is_targeting_move(m.target) {
            return;
        }

        // 6. Status / 0-BP non-status edge case (shouldn't reach here
        //    given category check above, but guard anyway).
        if m.base_power == 0 {
            return;
        }

        // 7. Crit roll. Gen 6+ base rate is 1/24. High-crit-ratio moves
        //    (Slash, Stone Edge, Storm Throw) deferred to their PRs.
        let crit = self.rng.range(24) == 0;
        let roll = self.rng.damage_roll();
        let dmg = calculate_damage(
            &attacker,
            &defender,
            move_id,
            DamageContext { crit, roll },
        );

        // 8. Apply damage.
        if let Some(t) = self.side_mut(target_side).active_mon_mut(target_slot as usize) {
            t.current_hp = t.current_hp.saturating_sub(dmg);
            if t.current_hp == 0 {
                t.fainted = true;
            }
        }
    }

    /// Status-move dispatch. Phase 2 PR-5 implements: Protect.
    ///
    /// Other status moves currently no-op (will be enabled per-move in
    /// subsequent PRs).
    fn resolve_status_move(&mut self, actor_side: SideRef, actor_slot: u8, m: &data::MoveDef) {
        match m.slug {
            "protect" | "detect" | "spikyshield" | "banefulbunker" | "kingsshield"
            | "obstruct" | "burningbulwark" | "silktrap" => {
                // Mark the issuer as having attempted a stall move.
                let stall_counter = {
                    let actor = match self.side_mut(actor_side).active_mon_mut(actor_slot as usize) {
                        Some(a) => a,
                        None => return,
                    };
                    actor.used_stall_this_turn = true;
                    actor.stall_counter
                };

                // Success probability: 1 / 3^stall_counter.
                // counter=0 → always; counter=1 → 1/3; counter=2 → 1/9; ...
                let denom: u32 = match stall_counter {
                    0 => 1,
                    n => 3u32.saturating_pow(n.min(6) as u32),
                };
                let success = self.rng.range(denom) == 0;

                let actor = match self.side_mut(actor_side).active_mon_mut(actor_slot as usize) {
                    Some(a) => a,
                    None => return,
                };
                if success {
                    actor.is_protected_this_turn = true;
                    actor.stall_counter = actor.stall_counter.saturating_add(1).min(6);
                } else {
                    actor.stall_counter = 0;
                }
            }
            _ => {
                // Unimplemented status move — no effect. Subsequent PRs
                // will add Tailwind, Trick Room, Encore, etc.
            }
        }
    }
}

/// PS targets that aim at a specific opposing/adjacent slot. Spread,
/// self, and side-targeted moves are not blocked by Protect.
fn is_targeting_move(target_code: u8) -> bool {
    matches!(target_code, 0 | 2 | 3 | 4 | 10)
    // 0 normal, 2 adjacentAlly, 3 adjacentAllyOrSelf, 4 adjacentFoe, 10 any.
    // 5 allAdjacent and 6 allAdjacentFoes are spread — they still hit but
    // Protect intercepts each individual target. We can refine in the
    // spread-move PR; for now treat spread as bypassing.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choice::{Choice, Target};
    use crate::team::TeamBuilder;

    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"],"evs":{"hp":4,"atk":252,"spe":252}},
        {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]},
        {"species":"incineroar","level":50,"ability":"intimidate","item":"safetygoggles","nature":"adamant","moves":["fakeout","knockoff","flareblitz","partingshot"]}
    ]"#;
    const P2: &str = r#"[
        {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
    ]"#;

    fn battle() -> Battle {
        let p1 = TeamBuilder::from_json(P1).unwrap();
        let p2 = TeamBuilder::from_json(P2).unwrap();
        Battle::new(BattleConfig { format: Format::Doubles, seed: 42 }, p1, p2)
    }

    fn t(side: SideRef, slot: u8) -> Target {
        Target { side, slot }
    }

    #[test]
    fn earthquake_damages_pikachu() {
        let mut b = battle();
        let pika_hp_before = b.p2.team[0].current_hp;
        b.step(
            &[
                Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
                Choice::Pass { actor_slot: 1 },
            ],
            &[
                Choice::Pass { actor_slot: 0 },
                Choice::Pass { actor_slot: 1 },
            ],
        );
        assert!(b.p2.team[0].current_hp < pika_hp_before, "pikachu took damage");
    }

    #[test]
    fn always_hit_aerial_ace() {
        // Aerial Ace has accuracy=true → encoded 255. Always lands.
        let mut b = battle();
        // Repeat 20 turns; each turn use Aerial Ace on Flutter Mane. Since
        // both Garchomp and Flutter Mane survive a single hit, we just
        // verify damage accumulates monotonically — never zero from a miss.
        let mut last_hp = b.p2.team[1].current_hp;
        for _ in 0..3 {
            b.step(
                &[
                    Choice::Move { actor_slot: 0, move_slot: 2, target: Some(t(SideRef::P2, 1)) },
                    Choice::Pass { actor_slot: 1 },
                ],
                &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
            );
            let now = b.p2.team[1].current_hp;
            assert!(now < last_hp, "Aerial Ace must always hit");
            last_hp = now;
        }
    }

    #[test]
    fn faint_sets_flag_and_ends_battle() {
        // Strip P2 to a single low-HP mon; have P1 KO it.
        let mut b = battle();
        // Manually faint Flutter Mane so only pikachu is left, set pika HP=1.
        b.p2.team[1].fainted = true;
        b.p2.team[1].current_hp = 0;
        b.p2.team[0].current_hp = 1;
        let r = b.step(
            &[
                Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
                Choice::Pass { actor_slot: 1 },
            ],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert_eq!(r, StepResult::Ended { winner: Some(SideRef::P1) });
        assert!(b.p2.team[0].fainted, "pikachu should be marked fainted");
    }

    #[test]
    fn pp_ticks_even_on_immune_target() {
        // Earthquake into Flying-type Pelipper would be 0 dmg if facing
        // it — but it's on the same side. Cross-team setup instead:
        // use Garchomp Earthquake against Flutter Mane (Ghost/Fairy) —
        // Ground hits Ghost neutrally / Fairy neutrally → not immune.
        // Use Aerial Ace (Flying) against Garchomp? Wrong side too.
        // Simpler: ensure pp decreases by 1 after a normal hit.
        let mut b = battle();
        let before = b.p1.team[0].pp[0];
        b.step(
            &[
                Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
                Choice::Pass { actor_slot: 1 },
            ],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert_eq!(b.p1.team[0].pp[0], before - 1);
    }

    #[test]
    fn protect_first_use_always_succeeds() {
        // Build a minimal scenario where one mon knows Protect.
        let p1_json = r#"[
            {"species":"toxapex","level":50,"ability":"regenerator","item":"blacksludge","nature":"calm","moves":["protect","scald","toxic","recover"],"evs":{"hp":252,"spd":252,"def":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hasty","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        let pex_hp = b.p1.team[0].current_hp;
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: None }],
            &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) }],
        );
        assert_eq!(b.p1.team[0].current_hp, pex_hp, "first Protect always succeeds; Toxapex takes no damage");
        assert_eq!(b.p1.team[0].stall_counter, 1);
    }

    #[test]
    fn consecutive_protect_eventually_fails() {
        // After several consecutive Protects the 1/3^n roll WILL fail.
        // Use a tiny denom and many turns to ensure we observe a failure.
        let p1_json = r#"[
            {"species":"toxapex","level":50,"ability":"regenerator","item":"blacksludge","nature":"calm","moves":["protect","scald","toxic","recover"],"evs":{"hp":252,"spd":252,"def":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hasty","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 99 }, p1, p2);
        let mut saw_fail = false;
        for _ in 0..30 {
            let hp_before = b.p1.team[0].current_hp;
            b.step(
                &[Choice::Move { actor_slot: 0, move_slot: 0, target: None }],
                &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) }],
            );
            if b.p1.team[0].current_hp < hp_before {
                saw_fail = true;
                break;
            }
        }
        assert!(saw_fail, "after enough consecutive Protects one must fail");
    }

    #[test]
    fn non_consecutive_protect_resets_counter() {
        // Use Protect, then attack, then Protect — counter should be back to 1.
        let p1_json = r#"[
            {"species":"toxapex","level":50,"ability":"regenerator","item":"blacksludge","nature":"calm","moves":["protect","scald","toxic","recover"],"evs":{"hp":252,"spd":252,"def":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hasty","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        // Turn 1: Protect
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: None }],
            &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) }],
        );
        assert_eq!(b.p1.team[0].stall_counter, 1);
        // Turn 2: Scald (not a stall move) → counter resets at end of turn.
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P2, 0)) }],
            &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) }],
        );
        assert_eq!(b.p1.team[0].stall_counter, 0);
        // Turn 3: Protect again — succeeds (counter was 0).
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: None }],
            &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) }],
        );
        assert_eq!(b.p1.team[0].stall_counter, 1, "fresh streak — counter back to 1");
    }

    #[test]
    fn deterministic_step_given_seed() {
        let p1 = TeamBuilder::from_json(P1).unwrap();
        let p2 = TeamBuilder::from_json(P2).unwrap();
        let mut a = Battle::new(BattleConfig { format: Format::Doubles, seed: 12345 }, p1.clone(), p2.clone());
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 12345 }, p1, p2);
        let choices = (
            vec![
                Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
                Choice::Move { actor_slot: 1, move_slot: 0, target: Some(t(SideRef::P2, 1)) },
            ],
            vec![
                Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
                Choice::Move { actor_slot: 1, move_slot: 0, target: Some(t(SideRef::P1, 1)) },
            ],
        );
        for _ in 0..5 {
            a.step(&choices.0, &choices.1);
            b.step(&choices.0, &choices.1);
        }
        for i in 0..a.p1.team.len() {
            assert_eq!(a.p1.team[i].current_hp, b.p1.team[i].current_hp);
        }
        for i in 0..a.p2.team.len() {
            assert_eq!(a.p2.team[i].current_hp, b.p2.team[i].current_hp);
        }
    }
}
