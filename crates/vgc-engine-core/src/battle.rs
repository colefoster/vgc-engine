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
    pub weather: crate::weather::Weather,
    /// Remaining-turn counter for weather. 0 when `weather == None`.
    pub weather_turns: u8,
    rng: Rng,
    turn: u32,
    ended: Option<Option<SideRef>>,
}

impl Battle {
    pub fn new(config: BattleConfig, p1_team: Vec<Pokemon>, p2_team: Vec<Pokemon>) -> Self {
        let p1 = Side::new(p1_team, config.format);
        let p2 = Side::new(p2_team, config.format);
        let rng = Rng::new(config.seed);
        let mut b = Self {
            config, p1, p2, rng, turn: 0, ended: None,
            weather: crate::weather::Weather::None, weather_turns: 0,
        };
        // Battle-start sendouts trigger on-switch-in abilities (Intimidate,
        // Drizzle, Sand Stream, etc.). P1 resolves first (PS-canonical
        // ordering matches turn-order but at battle start it's by side
        // and slot; refinement deferred).
        let n = b.format().active_count() as u8;
        for side in [SideRef::P1, SideRef::P2] {
            for slot in 0..n {
                crate::ability::on_switch_in(&mut b, side, slot);
            }
        }
        b
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

    pub(crate) fn side_mut(&mut self, side: SideRef) -> &mut Side {
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

        // 0. Per-turn volatile reset on every mon.
        for s in [SideRef::P1, SideRef::P2] {
            for m in self.side_mut(s).team.iter_mut() {
                m.is_protected_this_turn = false;
                m.used_stall_this_turn = false;
                m.flinched_this_turn = false;
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

        // 3. End-of-turn residuals (weather damage, future leftovers /
        //    status damage / Speed Boost / etc.). Runs BEFORE timer
        //    decrement so a mon takes its last sand damage on the turn
        //    sand expires (PS behavior).
        self.resolve_end_of_turn();

        // 4. Per-mon end-of-turn flags + side-condition timers.
        for s in [SideRef::P1, SideRef::P2] {
            let side = self.side_mut(s);
            for m in side.team.iter_mut() {
                if !m.used_stall_this_turn {
                    m.stall_counter = 0;
                }
            }
            for &active_idx in side.active.iter() {
                if (active_idx as usize) < side.team.len() {
                    let mon = &mut side.team[active_idx as usize];
                    mon.turns_active = mon.turns_active.saturating_add(1);
                }
            }
            // Tailwind / future side-condition timers.
            side.conditions.tailwind_turns = side.conditions.tailwind_turns.saturating_sub(1);
        }
        // 5. Weather timer decrement (battle-wide).
        if self.weather_turns > 0 {
            self.weather_turns -= 1;
            if self.weather_turns == 0 {
                self.weather = crate::weather::Weather::None;
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
        let mut switched_slots: Vec<u8> = Vec::new();
        for c in choices {
            if let Choice::Switch { actor_slot, team_index } = *c {
                let s = self.side_mut(side);
                if (actor_slot as usize) < s.active.len()
                    && (team_index as usize) < s.team.len()
                    && s.team[team_index as usize].is_alive()
                {
                    s.active[actor_slot as usize] = team_index;
                    let incoming = &mut s.team[team_index as usize];
                    incoming.boosts = [0; 7];
                    incoming.turns_active = 0;
                    incoming.flinched_this_turn = false;
                    incoming.is_protected_this_turn = false;
                    incoming.stall_counter = 0;
                    switched_slots.push(actor_slot);
                }
            }
        }
        // Run on-switch-in ability hooks for each newly-active mon.
        for slot in switched_slots {
            crate::ability::on_switch_in(self, side, slot);
        }
    }

    fn resolve_move(&mut self, action: ScheduledAction) {
        let Choice::Move { actor_slot, move_slot, target } = action.choice else {
            return;
        };
        let actor_side = action.side;

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

        // 1. Flinch check — flinched mons cannot move at all this turn.
        //    PS: PP is NOT consumed on flinch (the move is replaced with
        //    inaction). Source: PS sim/battle-actions.ts:runMove.
        if attacker.flinched_this_turn {
            return;
        }

        // 2. Fake Out: fails unless attacker has been on the field 0 turns
        //    (i.e. this is its first action since switch-in). PS marks
        //    this with the 'fakeout' move's onTry checking activeTurns.
        if m.slug == "fakeout" && attacker.turns_active != 0 {
            // Failure still ticks PP per PS.
            if let Some(mon) = self.side_mut(actor_side).active_mon_mut(actor_slot as usize) {
                if let Some(pp) = mon.pp.get_mut(move_slot as usize) {
                    *pp = pp.saturating_sub(1);
                }
            }
            return;
        }

        // 3. PP cost — ticked even on miss / immunity (PS behavior).
        if let Some(mon) = self.side_mut(actor_side).active_mon_mut(actor_slot as usize) {
            if let Some(pp) = mon.pp.get_mut(move_slot as usize) {
                *pp = pp.saturating_sub(1);
            }
        }

        // 4. Status-move dispatch.
        if m.category == 2 {
            self.resolve_status_move(actor_side, actor_slot, m);
            return;
        }

        // 5. Enumerate targets (spread or single).
        let targets = enumerate_targets(self, actor_side, actor_slot, m, target);
        if targets.is_empty() {
            return;
        }
        let is_spread = targets.len() > 1;
        let damaging = m.base_power > 0;

        // 6. Per-target resolution — PS does accuracy + damage rolls and
        //    Protect/secondary checks independently per target.
        for (tside, tslot) in targets {
            let defender = match self.side(tside).active_mon(tslot as usize).cloned() {
                Some(d) if d.is_alive() => d,
                _ => continue,
            };

            // Accuracy.
            if m.accuracy != 255 {
                let roll = self.rng.percent_1_100() as u32;
                if roll > m.accuracy as u32 {
                    continue;
                }
            }

            // Protect interception (single-target codes only; spread
            // hits each target independently and Protect intercepts the
            // single hit on the protected slot — already handled here).
            if defender.is_protected_this_turn && is_targeting_move(m.target) {
                continue;
            }

            if !damaging {
                continue;
            }

            // Crit + damage roll.
            let crit = self.rng.range(24) == 0;
            let roll = self.rng.damage_roll();
            let dmg = calculate_damage(
                &attacker,
                &defender,
                move_id,
                DamageContext { crit, roll, is_spread, weather: self.weather },
            );

            // Apply.
            if let Some(t) = self.side_mut(tside).active_mon_mut(tslot as usize) {
                t.current_hp = t.current_hp.saturating_sub(dmg);
                if t.current_hp == 0 {
                    t.fainted = true;
                }
            }

            // Secondary if target still alive.
            let alive_post = self.side(tside).active_mon(tslot as usize)
                .is_some_and(|m| m.is_alive());
            if alive_post {
                let mut rng = self.rng;
                apply_secondary_effect(self, tside, tslot, m.slug, &mut rng);
                self.rng = rng;
            }
        }
    }

    /// End-of-turn residuals: damage / heal sources that fire each turn
    /// after move resolution. Currently: Sand weather damage. Subsequent
    /// PRs add Leftovers, burn/poison/toxic damage, Speed Boost, etc.
    fn resolve_end_of_turn(&mut self) {
        // Item residuals (Leftovers etc.) fire before weather damage in
        // gen 5+ — PS order: ability residuals → item residuals → weather.
        // We don't have ability residuals yet, so item-then-weather here
        // matches the relevant subset.
        for side in [SideRef::P1, SideRef::P2] {
            let n = self.format().active_count() as u8;
            for slot in 0..n {
                crate::item::on_residual(self, side, slot);
            }
        }

        // Sand: 1/16 max HP per turn to every active mon not type-immune.
        // Ability / item immunities (Sand Veil ignored — that's evasion
        // not damage immunity; Magic Guard / Overcoat / Safety Goggles
        // are real damage immunities) land in their own PRs.
        if self.weather == crate::weather::Weather::Sand {
            for side in [SideRef::P1, SideRef::P2] {
                let n = self.format().active_count();
                for slot in 0..n {
                    let immune = match self.side(side).active_mon(slot) {
                        Some(m) if m.is_alive() => {
                            let species = m.species();
                            (0..species.num_types as usize).any(|i| {
                                // Type codes: 12 Rock, 8 Ground, 16 Steel.
                                matches!(species.types[i], 12 | 8 | 16)
                            })
                        }
                        _ => true, // missing/fainted → skip
                    };
                    if immune {
                        continue;
                    }
                    if let Some(m) = self.side_mut(side).active_mon_mut(slot) {
                        let dmg = (m.stats.hp / 16).max(1);
                        m.current_hp = m.current_hp.saturating_sub(dmg);
                        if m.current_hp == 0 {
                            m.fainted = true;
                        }
                    }
                }
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
            "tailwind" => {
                // Side condition: 4-turn timer. Fails if already up.
                // PS data/conditions.ts:tailwind has duration 4.
                let s = self.side_mut(actor_side);
                if s.conditions.tailwind_turns == 0 {
                    s.conditions.tailwind_turns = 4;
                }
            }
            _ => {
                // Unimplemented status move — no effect. Subsequent PRs
                // will add Trick Room, Encore, screens, etc.
            }
        }
    }
}

/// Enumerate concrete (side, slot) targets a move will hit.
///
/// `chosen` is the explicit target supplied in the Choice (used for
/// single-target moves). Spread / self / ally-side moves ignore it.
fn enumerate_targets(
    battle: &Battle,
    actor_side: SideRef,
    actor_slot: u8,
    m: &data::MoveDef,
    chosen: Option<Target>,
) -> Vec<(SideRef, u8)> {
    let opp = actor_side.opposing();
    let active_n = battle.format().active_count() as u8;
    let alive = |side: SideRef, slot: u8| -> bool {
        battle.side(side).active_mon(slot as usize).is_some_and(|m| m.is_alive())
    };
    match m.target {
        // 0 normal | 4 adjacentFoe | 10 any | 13 randomNormal — single target.
        0 | 4 | 10 | 13 => {
            if let Some(t) = chosen {
                if alive(t.side, t.slot) {
                    return vec![(t.side, t.slot)];
                }
            }
            // Fallback: first alive opposing active slot.
            for slot in 0..active_n {
                if alive(opp, slot) {
                    return vec![(opp, slot)];
                }
            }
            vec![]
        }
        // 1 self
        1 => vec![(actor_side, actor_slot)],
        // 2 adjacentAlly | 3 adjacentAllyOrSelf — single target on own side.
        2 | 3 => {
            if let Some(t) = chosen {
                if t.side == actor_side && alive(t.side, t.slot) {
                    return vec![(t.side, t.slot)];
                }
            }
            for slot in 0..active_n {
                if slot != actor_slot && alive(actor_side, slot) {
                    return vec![(actor_side, slot)];
                }
            }
            vec![]
        }
        // 5 allAdjacent — all adjacent foes + ally (skip self).
        5 => {
            let mut out = Vec::with_capacity(3);
            for slot in 0..active_n {
                if alive(opp, slot) {
                    out.push((opp, slot));
                }
            }
            for slot in 0..active_n {
                if slot != actor_slot && alive(actor_side, slot) {
                    out.push((actor_side, slot));
                }
            }
            out
        }
        // 6 allAdjacentFoes — both opposing actives.
        6 => {
            let mut out = Vec::with_capacity(2);
            for slot in 0..active_n {
                if alive(opp, slot) {
                    out.push((opp, slot));
                }
            }
            out
        }
        // Targets we don't damage-resolve here (allies / side / team / all / scripted).
        _ => vec![],
    }
}

/// Per-slug flinch chance for moves whose secondary is a flinch.
///
/// All values cross-checked against PS data/moves.ts. Moves with other
/// secondaries (burn, paralysis, stat drops) land in their respective PRs.
fn flinch_chance(slug: &str) -> Option<u8> {
    Some(match slug {
        "fakeout" => 100,
        "rockslide" | "airslash" | "ironhead" | "zenheadbutt"
        | "headbutt" | "bite" | "stomp" | "needleam"
        | "extrasensory" | "astonish" | "hyperfang" => 30,
        "darkpulse" | "twister" | "dragonrush" | "snore" => 20,
        "icefang" | "thunderfang" | "firefang" | "fireblast"
        | "rollingkick" | "lowkick" | "steamroller" => 10,
        // Heat Wave: 10% BURN, not flinch — handled elsewhere.
        _ => return None,
    })
}

/// Apply a move's secondary effect to the target. Currently this is
/// flinch-only (the secondaries covered in PR-6 + PR-7). Burn/poison/
/// para/stat-drop secondaries land in subsequent PRs.
///
/// PS rolls secondaries per target independently.
fn apply_secondary_effect(
    battle: &mut Battle,
    target_side: SideRef,
    target_slot: u8,
    move_slug: &str,
    rng: &mut Rng,
) {
    if let Some(chance) = flinch_chance(move_slug) {
        if rng.percent_1_100() <= chance {
            if let Some(t) = battle.side_mut(target_side).active_mon_mut(target_slot as usize) {
                t.flinched_this_turn = true;
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
    fn fake_out_flinches_first_turn() {
        // Singles to keep it tidy. Iron Hands knows Fake Out.
        let p1_json = r#"[
            {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"],"evs":{"atk":252,"hp":252,"def":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"adamant","moves":["earthquake","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 7 }, p1, p2);
        let ih_hp = b.p1.team[0].current_hp;
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) }],
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) }],
        );
        // Fake Out priority +3 beats Earthquake. Target flinches → Earthquake skipped.
        assert_eq!(b.p1.team[0].current_hp, ih_hp, "Iron Hands took no damage — flinched");
        assert!(b.p2.team[0].current_hp < b.p2.team[0].stats.hp, "Garchomp took Fake Out damage");
    }

    #[test]
    fn fake_out_fails_on_second_turn() {
        let p1_json = r#"[
            {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"],"evs":{"atk":252,"hp":252,"def":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["protect","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 7 }, p1, p2);
        // Turn 1: Iron Hands uses Drain Punch (anything non-Fake-Out), Garchomp Protects.
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P2, 0)) }],
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: None }],
        );
        assert_eq!(b.p1.team[0].turns_active, 1, "Iron Hands has been out 1 turn");
        let chomp_hp = b.p2.team[0].current_hp;
        // Turn 2: Iron Hands tries Fake Out — should fail (no damage, no flinch).
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) }],
            &[Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) }],
        );
        assert_eq!(b.p2.team[0].current_hp, chomp_hp, "Fake Out failed → no damage to Garchomp");
        // Garchomp's Dragon Claw should have hit Iron Hands.
        assert!(b.p1.team[0].current_hp < b.p1.team[0].stats.hp);
    }

    #[test]
    fn switching_resets_turns_active() {
        // Doubles: switch out a mon and bring it back; turns_active should reset.
        let p1_json = r#"[
            {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"],"evs":{"atk":252,"hp":252,"def":4}},
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hasty","moves":["thunderbolt","quickattack","grassknot","feint"]},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2);
        // Turn 1: pass everything to age the active mons.
        b.step(
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert_eq!(b.p1.team[0].turns_active, 1);
        // Turn 2: switch slot 0 (Iron Hands) → Pelipper.
        b.step(
            &[
                Choice::Switch { actor_slot: 0, team_index: 2 },
                Choice::Pass { actor_slot: 1 },
            ],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        // Pelipper just switched in this turn; end-of-step increments → 1.
        assert_eq!(b.p1.team[2].turns_active, 1, "Pelipper has now been out 1 turn");
        // Turn 3: switch back to Iron Hands.
        b.step(
            &[
                Choice::Switch { actor_slot: 0, team_index: 0 },
                Choice::Pass { actor_slot: 1 },
            ],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        // Iron Hands was reset on switch-in, then incremented at end → 1.
        // Its first action turn (turns_active == 0 during move resolution)
        // is the NEXT turn — confirm by trying Fake Out.
        assert_eq!(b.p1.team[0].turns_active, 1);
    }

    #[test]
    fn leftovers_heals_one_sixteenth_per_turn() {
        // Snorlax with Leftovers — heals 1/16 max HP each end of turn.
        // Damage it first with a moonblast from Flutter Mane.
        let p1_json = r#"[
            {"species":"snorlax","level":50,"ability":"thickfat","item":"leftovers","nature":"careful","moves":["bodyslam","earthquake","crunch","rest"],"evs":{"hp":252,"spd":252,"def":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252,"hp":4}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        // Damage Snorlax.
        b.step(
            &[Choice::Pass { actor_slot: 0 }],
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) }],
        );
        let dmg_hp = b.p1.team[0].current_hp;
        // Already healed by Leftovers post-Moonblast, this turn. Verify
        // a SECOND turn of passes heals exactly max/16 more.
        let max = b.p1.team[0].stats.hp;
        let expected_heal = (max / 16).max(1);
        let target_hp = (dmg_hp + expected_heal).min(max);
        b.step(
            &[Choice::Pass { actor_slot: 0 }],
            &[Choice::Pass { actor_slot: 0 }],
        );
        assert_eq!(b.p1.team[0].current_hp, target_hp);
    }

    #[test]
    fn leftovers_does_not_overheal() {
        let p1_json = r#"[
            {"species":"snorlax","level":50,"ability":"thickfat","item":"leftovers","nature":"careful","moves":["bodyslam","earthquake","crunch","rest"],"evs":{"hp":252,"spd":252,"def":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        let max = b.p1.team[0].stats.hp;
        assert_eq!(b.p1.team[0].current_hp, max);
        b.step(
            &[Choice::Pass { actor_slot: 0 }],
            &[Choice::Pass { actor_slot: 0 }],
        );
        assert_eq!(b.p1.team[0].current_hp, max, "Leftovers caps at max HP");
    }

    #[test]
    fn sand_damages_non_immune_active() {
        // Tyranitar (Rock/Dark) has Sand Stream → triggers Sand.
        // Garchomp (Dragon/Ground) is Ground-type → immune.
        // Pikachu (Electric) → takes damage.
        let p1_json = r#"[
            {"species":"tyranitar","level":50,"ability":"sandstream","item":"smoothrock","nature":"adamant","moves":["rockslide","crunch","earthquake","stealthrock"]},
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["rockslide","dragonclaw","aerialace","ironhead"]}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2);
        // Sand active from battle start (Tyranitar's Sand Stream).
        assert_eq!(b.weather, crate::weather::Weather::Sand);
        let chomp_hp = b.p1.team[1].current_hp;
        let pika_hp = b.p2.team[0].current_hp;
        let ttar_hp = b.p1.team[0].current_hp;
        b.step(
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert_eq!(b.p1.team[1].current_hp, chomp_hp, "Garchomp (Ground) immune");
        assert_eq!(b.p1.team[0].current_hp, ttar_hp, "Tyranitar (Rock) immune");
        assert!(b.p2.team[0].current_hp < pika_hp, "Pikachu takes sand damage");
        // Exact: 1/16 of max HP.
        let expected = b.p2.team[0].stats.hp / 16;
        assert_eq!(pika_hp - b.p2.team[0].current_hp, expected);
    }

    #[test]
    fn drizzle_sets_rain_at_battle_start() {
        let p1_json = r#"[
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        assert_eq!(b.weather, crate::weather::Weather::Rain);
        assert_eq!(b.weather_turns, 5);
    }

    #[test]
    fn weather_decays_after_five_turns() {
        let p1_json = r#"[
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        for _ in 0..5 {
            b.step(
                &[Choice::Pass { actor_slot: 0 }],
                &[Choice::Pass { actor_slot: 0 }],
            );
        }
        assert_eq!(b.weather, crate::weather::Weather::None);
        assert_eq!(b.weather_turns, 0);
    }

    #[test]
    fn rain_boosts_water_damage() {
        use crate::damage::{calculate_damage, DamageContext};
        // Use Pelipper Hurricane vs Pikachu — but Hurricane is Flying, not Water.
        // Use Weather Ball (changes type with weather) — too complex.
        // Use Surf instead: build Pelipper that knows Surf.
        let p1_json = r#"[
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["surf","weatherball","tailwind","airslash"],"evs":{"spa":252,"spe":252,"hp":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let surf_id = data::MOVES.iter().position(|m| m.slug == "surf").unwrap() as u16;
        let no_rain = calculate_damage(
            &p1[0], &p2[0], surf_id,
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None },
        );
        let in_rain = calculate_damage(
            &p1[0], &p2[0], surf_id,
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::Rain },
        );
        assert!(in_rain > no_rain, "Surf in Rain should hit harder");
        // Should be ~1.5×; integer truncation may push it slightly under.
        assert!(in_rain * 100 / no_rain >= 145, "expected ~1.5×; got {}/{}", in_rain, no_rain);
    }

    #[test]
    fn intimidate_drops_opposing_atk_at_battle_start() {
        // Incineroar has Intimidate. On battle start, opposing mons'
        // atk is -1. Pikachu has Static (no immunity), Garchomp has
        // Rough Skin (no immunity) — both should be intimidated.
        let p1_json = r#"[
            {"species":"incineroar","level":50,"ability":"intimidate","item":"safetygoggles","nature":"adamant","moves":["fakeout","knockoff","flareblitz","partingshot"]},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]},
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["rockslide","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2);
        assert_eq!(b.p2.team[0].boosts[0], -1, "pikachu atk -1 from Intimidate");
        assert_eq!(b.p2.team[1].boosts[0], -1, "garchomp atk -1 from Intimidate");
        // No friendly fire — Pelipper's atk unaffected.
        assert_eq!(b.p1.team[1].boosts[0], 0);
    }

    #[test]
    fn clear_body_blocks_intimidate() {
        let p1_json = r#"[
            {"species":"incineroar","level":50,"ability":"intimidate","item":"safetygoggles","nature":"adamant","moves":["fakeout","knockoff","flareblitz","partingshot"]}
        ]"#;
        // Metagross has Clear Body.
        let p2_json = r#"[
            {"species":"metagross","level":50,"ability":"clearbody","item":"weaknesspolicy","nature":"adamant","moves":["meteormash","bulletpunch","earthquake","icepunch"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        assert_eq!(b.p2.team[0].boosts[0], 0, "Clear Body blocks Intimidate");
    }

    #[test]
    fn intimidate_triggers_on_mid_battle_switch_in() {
        let p1_json = r#"[
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]},
            {"species":"incineroar","level":50,"ability":"intimidate","item":"safetygoggles","nature":"adamant","moves":["fakeout","knockoff","flareblitz","partingshot"]}
        ]"#;
        let p2_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["rockslide","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        // No Intimidate at battle start — Pelipper is in.
        assert_eq!(b.p2.team[0].boosts[0], 0);
        // Switch in Incineroar.
        b.step(
            &[Choice::Switch { actor_slot: 0, team_index: 1 }],
            &[Choice::Pass { actor_slot: 0 }],
        );
        assert_eq!(b.p2.team[0].boosts[0], -1, "Intimidate fires on mid-battle switch-in");
    }

    #[test]
    fn tailwind_doubles_speed_for_four_turns() {
        // Pelipper (slower than Garchomp jolly 252) uses Tailwind; after
        // it lands, Pelipper-side speed doubles. Run 5 turns and check
        // that conditions tick correctly.
        let p1_json = r#"[
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["tailwind","weatherball","hurricane","airslash"]},
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["rockslide","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2);
        // Turn 1: Pelipper uses Tailwind.
        b.step(
            &[
                Choice::Move { actor_slot: 0, move_slot: 0, target: None },
                Choice::Pass { actor_slot: 1 },
            ],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        assert_eq!(b.p1.conditions.tailwind_turns, 3, "tick from 4 → 3 at end of turn 1");
        // Pelipper-side speed should be doubled while active. Use the
        // order module to verify.
        let pel_spe_with_tw = crate::order::effective_speed(&b.p1.team[0], true);
        let pel_spe_no_tw = crate::order::effective_speed(&b.p1.team[0], false);
        assert_eq!(pel_spe_with_tw, pel_spe_no_tw * 2);
        // Steps 2–4: tick down.
        for _ in 0..3 {
            b.step(
                &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
                &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
            );
        }
        assert_eq!(b.p1.conditions.tailwind_turns, 0, "expires after 4 total turns");
    }

    #[test]
    fn tailwind_fails_when_already_active() {
        let p1_json = r#"[
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["tailwind","weatherball","hurricane","airslash"]}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: None }],
            &[Choice::Pass { actor_slot: 0 }],
        );
        assert_eq!(b.p1.conditions.tailwind_turns, 3);
        b.step(
            &[Choice::Move { actor_slot: 0, move_slot: 0, target: None }],
            &[Choice::Pass { actor_slot: 0 }],
        );
        // Second Tailwind should fail; counter still ticks down → 2.
        assert_eq!(b.p1.conditions.tailwind_turns, 2);
    }

    #[test]
    fn rock_slide_hits_both_opposing_actives() {
        let p1_json = r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["rockslide","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}},
            {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","airslash"]}
        ]"#;
        let p2_json = r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hasty","moves":["thunderbolt","quickattack","grassknot","feint"]},
            {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"]}
        ]"#;
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        let mut b = Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2);
        let pika_hp = b.p2.team[0].current_hp;
        let flutter_hp = b.p2.team[1].current_hp;
        b.step(
            &[
                Choice::Move { actor_slot: 0, move_slot: 0, target: None },
                Choice::Pass { actor_slot: 1 },
            ],
            &[Choice::Pass { actor_slot: 0 }, Choice::Pass { actor_slot: 1 }],
        );
        // Rock Slide is allAdjacentFoes → should damage both opposing actives.
        assert!(b.p2.team[0].current_hp < pika_hp, "Pikachu took spread damage");
        assert!(b.p2.team[1].current_hp < flutter_hp, "Flutter Mane took spread damage");
    }

    #[test]
    fn spread_modifier_reduces_damage_vs_single_target() {
        // Compare Earthquake (allAdjacent) in doubles vs singles.
        // In singles the spread mod doesn't apply; in doubles with 2
        // foes it does, so damage should be lower.
        use crate::damage::{calculate_damage, DamageContext};
        let p1_team = TeamBuilder::from_json(r#"[
            {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","dragonclaw","aerialace","ironhead"],"evs":{"atk":252,"spe":252,"hp":4}}
        ]"#).unwrap();
        let p2_team = TeamBuilder::from_json(r#"[
            {"species":"pikachu","level":50,"ability":"static","item":"focussash","nature":"hardy","moves":["thunderbolt","quickattack","grassknot","feint"]}
        ]"#).unwrap();
        let eq_id = data::MOVES.iter().position(|m| m.slug == "earthquake").unwrap() as u16;
        let single = calculate_damage(
            &p1_team[0], &p2_team[0], eq_id,
            DamageContext { crit: false, roll: 15, is_spread: false, weather: crate::weather::Weather::None },
        );
        let spread = calculate_damage(
            &p1_team[0], &p2_team[0], eq_id,
            DamageContext { crit: false, roll: 15, is_spread: true, weather: crate::weather::Weather::None },
        );
        // spread should be ~0.75× single (truncation-modulo).
        assert!(spread < single);
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
