//! Action-order resolution for one battle turn.
//!
//! Pure function over a (Battle snapshot, choices, RNG). PS resolves
//! actions in this order:
//!
//!   1. Switches (PS treats them as priority +6; in our scope they don't
//!      interact, so we order them deterministically by (side, slot)).
//!   2. Moves, sorted by:
//!        - priority bracket (desc) — Fake Out +3 before Tackle 0 before
//!          Trick Room −7
//!        - effective speed (desc) — boosts applied, paralysis halved
//!        - RNG nonce (consistent tiebreak; coin flip in expectation)
//!   3. End-of-turn effects (its own PR).
//!
//! Deferred to subsequent PRs (each adds a single multiplier or override
//! into `effective_speed` / priority computation):
//!   - Trick Room (priority sort *reversed* under Trick Room)
//!   - Tailwind ×2 speed, Swift Swim / Chlorophyll / Sand Rush ×2
//!   - Choice Scarf ×1.5 speed
//!   - Quick Claw / Quark Drive / Custap Berry (priority bumps)
//!   - Stall / Mycelium Might (priority drops)
//!   - Prankster +1 priority to status moves
//!   - Gale Wings +1 priority to Flying-type moves at full HP

use crate::battle::Battle;
use crate::choice::Choice;
use crate::damage::apply_boost;
use crate::pokemon::{Pokemon, Status};
use crate::rng::Rng;
use crate::side::SideRef;
use vgc_engine_data as data;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledAction {
    pub side: SideRef,
    pub actor_slot: u8,
    pub choice: Choice,
}

/// Speed of `mon` after stage boosts, paralysis, side conditions
/// (Tailwind), held item (Choice Scarf), Paradox booster, and weather-
/// keyed speed abilities (Swift Swim / Chlorophyll / Sand Rush / Slush
/// Rush). Trick Room is handled by the comparator at the call site.
pub fn effective_speed(mon: &Pokemon, tailwind_active: bool, weather: crate::weather::Weather) -> u16 {
    let boosted = apply_boost(mon.stats.spe as u32, mon.boosts[4]);
    let after_para = if matches!(mon.status, Status::Paralysis) {
        boosted / 2
    } else {
        boosted
    };
    let after_tailwind = if tailwind_active { after_para * 2 } else { after_para };
    // Choice Scarf: ×1.5 to final speed.
    let item_slug = if mon.item_id == u16::MAX {
        ""
    } else {
        data::ITEMS[mon.item_id as usize].slug
    };
    let after_item = if item_slug == "choicescarf" {
        after_tailwind * 3 / 2
    } else {
        after_tailwind
    };
    // Paradox booster on Spe (index 4): ×1.5 to speed. PS chainModify(1.5)
    // for protosynthesisspe / quarkdrivespe volatile flavors.
    let after_paradox = if mon.boosted_stat == 4 {
        after_item * 3 / 2
    } else {
        after_item
    };
    // Weather speed abilities — PS `data/abilities.ts` `onModifySpe`
    // returns `this.chainModify(2)` for Swift Swim under Rain,
    // Chlorophyll under Sun, Sand Rush under Sand, Slush Rush under Snow.
    // Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Swift_Swim_(Ability)>.
    use crate::weather::Weather;
    let ability_slug = if mon.ability_id == u16::MAX {
        ""
    } else {
        data::ABILITIES[mon.ability_id as usize].slug
    };
    let weather_double = matches!(
        (ability_slug, weather),
        ("swiftswim", Weather::Rain)
            | ("chlorophyll", Weather::Sun)
            | ("sandrush", Weather::Sand)
            | ("slushrush", Weather::Snow)
    );
    let after_weather = if weather_double { after_paradox * 2 } else { after_paradox };
    after_weather.min(u16::MAX as u32) as u16
}

/// Resolve one turn's action order.
///
/// `p1` and `p2` are the per-active-slot choices for each side. `Pass`
/// choices are dropped (they don't produce actions).
pub fn action_order(
    battle: &Battle,
    p1: &[Choice],
    p2: &[Choice],
    rng: &mut Rng,
) -> Vec<ScheduledAction> {
    let mut switches: Vec<ScheduledAction> = Vec::with_capacity(4);
    // (negative priority, signed speed key, nonce, action) — ascending.
    let mut moves: Vec<(i32, i64, u64, ScheduledAction)> = Vec::with_capacity(4);
    let trick_room = battle.trick_room_turns > 0;

    for (side, choices) in [(SideRef::P1, p1), (SideRef::P2, p2)] {
        for c in choices {
            match *c {
                Choice::Pass { .. } => {}
                Choice::Switch { actor_slot, .. } => {
                    switches.push(ScheduledAction { side, actor_slot, choice: *c });
                }
                Choice::Move { actor_slot, move_slot, .. }
                | Choice::Terastallize { actor_slot, move_slot, .. } => {
                    let tailwind = battle.side(side).conditions.tailwind_turns > 0;
                    let mon = battle.side(side).active_mon(actor_slot as usize);
                    let (priority, speed) = match mon {
                        Some(m) => {
                            let mid = m.moves.get(move_slot as usize).copied().unwrap_or(u16::MAX);
                            let (base_pri, category) = if mid == u16::MAX {
                                (0i32, 2u8)
                            } else {
                                let mv = &data::MOVES[mid as usize];
                                (mv.priority as i32, mv.category)
                            };
                            // Prankster: +1 priority to status moves used
                            // by the holder. Dark-type immunity to the
                            // boosted move is enforced at resolve time
                            // (gen 7+), not here — order-resolution still
                            // uses the bumped priority. PS data/abilities.ts
                            // prankster onModifyPriority.
                            let pri_after_ability = if category == 2 {
                                let ability_slug = if m.ability_id == u16::MAX {
                                    ""
                                } else {
                                    data::ABILITIES[m.ability_id as usize].slug
                                };
                                if ability_slug == "prankster" {
                                    base_pri + 1
                                } else {
                                    base_pri
                                }
                            } else {
                                base_pri
                            };
                            (pri_after_ability, effective_speed(m, tailwind, battle.weather) as i64)
                        }
                        None => (0, 0),
                    };
                    // Trick Room reverses speed sort within a priority
                    // bracket (priority itself is NOT reversed).
                    let speed_key = if trick_room { speed } else { -speed };
                    moves.push((
                        -priority,
                        speed_key,
                        rng.next_u64(),
                        ScheduledAction { side, actor_slot, choice: *c },
                    ));
                }
            }
        }
    }

    moves.sort_by_key(|t| (t.0, t.1, t.2));
    let mut out = switches;
    out.reserve(moves.len());
    out.extend(moves.into_iter().map(|t| t.3));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battle::{Battle, BattleConfig};
    use crate::choice::{Choice, Target};
    use crate::team::TeamBuilder;

    // P1: a fast mon (Pelipper, base 65 spe) and Garchomp (102 spe).
    // P2: Iron Hands (50 spe), Flutter Mane (135 spe).
    const P1: &str = r#"[
        {"species":"garchomp","level":50,"ability":"roughskin","item":"lifeorb","nature":"jolly","moves":["earthquake","extremespeed","protect","ironhead"],"evs":{"spe":252,"atk":252,"hp":4}},
        {"species":"pelipper","level":50,"ability":"drizzle","item":"focussash","nature":"modest","moves":["hurricane","weatherball","tailwind","protect"]}
    ]"#;
    const P2: &str = r#"[
        {"species":"ironhands","level":50,"ability":"quarkdrive","item":"assaultvest","nature":"adamant","moves":["fakeout","drainpunch","thunderpunch","wildcharge"],"evs":{"atk":252,"hp":252,"def":4}},
        {"species":"fluttermane","level":50,"ability":"protosynthesis","item":"choicespecs","nature":"timid","moves":["moonblast","shadowball","dazzlinggleam","mysticalfire"],"evs":{"spa":252,"spe":252,"hp":4}}
    ]"#;

    fn make_battle() -> Battle {
        let p1 = TeamBuilder::from_json(P1).unwrap();
        let p2 = TeamBuilder::from_json(P2).unwrap();
        Battle::new(BattleConfig::default(), p1, p2)
    }

    fn t(side: SideRef, slot: u8) -> Target {
        Target { side, slot }
    }

    #[test]
    fn higher_priority_goes_first() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        // P1 slot 0 (Garchomp): ExtremeSpeed (priority +2)
        // P2 slot 0 (Iron Hands): Drain Punch (priority 0)
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        assert_eq!(order[0].side, SideRef::P1);
        assert_eq!(order[1].side, SideRef::P2);
    }

    #[test]
    fn fake_out_outpaces_extreme_speed() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        // P1 Garchomp ExtremeSpeed (+2)
        // P2 Iron Hands Fake Out (+3) — should go first despite lower speed.
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        assert_eq!(order[0].side, SideRef::P2, "Fake Out (+3) should outpace ExtremeSpeed (+2)");
    }

    #[test]
    fn speed_tiebreaks_same_priority() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        // Same priority (0) for both:
        // P1 Garchomp Earthquake (jolly 252 spe = high)
        // P2 Iron Hands Drain Punch (adamant 0 EV spe = low)
        // P2 Flutter Mane Moonblast (timid 252 spe = highest)
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) },
            Choice::Move { actor_slot: 1, move_slot: 0, target: Some(t(SideRef::P1, 0)) },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        // Flutter Mane should be first (highest speed), Garchomp second, Iron Hands last.
        assert_eq!(order[0].side, SideRef::P2);
        assert_eq!(order[0].actor_slot, 1, "Flutter Mane first by speed");
        assert_eq!(order[1].side, SideRef::P1);
        assert_eq!(order[2].side, SideRef::P2);
        assert_eq!(order[2].actor_slot, 0, "Iron Hands last");
    }

    #[test]
    fn switches_before_moves() {
        let b = make_battle();
        let mut rng = Rng::new(0);
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Switch { actor_slot: 0, team_index: 1 },
            Choice::Pass { actor_slot: 1 },
        ];
        let order = action_order(&b, &p1, &p2, &mut rng);
        assert!(matches!(order[0].choice, Choice::Switch { .. }));
    }

    #[test]
    fn paralysis_halves_speed_for_order() {
        let b = make_battle();
        let mut mon = b.p1.team[0].clone();
        mon.status = Status::Paralysis;
        let before = effective_speed(&b.p1.team[0], false, crate::weather::Weather::None);
        let after = effective_speed(&mon, false, crate::weather::Weather::None);
        assert_eq!(after, before / 2);
    }

    #[test]
    fn tailwind_doubles_speed_for_order() {
        let b = make_battle();
        let base = effective_speed(&b.p1.team[0], false, crate::weather::Weather::None);
        let with_tw = effective_speed(&b.p1.team[0], true, crate::weather::Weather::None);
        assert_eq!(with_tw, base * 2);
    }

    #[test]
    fn swift_swim_doubles_speed_in_rain_only() {
        let b = make_battle();
        let mut mon = b.p1.team[0].clone();
        let ss_id = data::ABILITIES.iter()
            .position(|a| a.slug == "swiftswim").unwrap() as u16;
        mon.ability_id = ss_id;
        let dry = effective_speed(&mon, false, crate::weather::Weather::None);
        let rain = effective_speed(&mon, false, crate::weather::Weather::Rain);
        let sun = effective_speed(&mon, false, crate::weather::Weather::Sun);
        assert_eq!(rain, dry * 2, "Swift Swim doubles in Rain");
        assert_eq!(sun, dry, "Swift Swim no-op in Sun");
    }

    #[test]
    fn chlorophyll_doubles_speed_in_sun_only() {
        let b = make_battle();
        let mut mon = b.p1.team[0].clone();
        let id = data::ABILITIES.iter()
            .position(|a| a.slug == "chlorophyll").unwrap() as u16;
        mon.ability_id = id;
        let dry = effective_speed(&mon, false, crate::weather::Weather::None);
        let sun = effective_speed(&mon, false, crate::weather::Weather::Sun);
        assert_eq!(sun, dry * 2);
    }

    #[test]
    fn deterministic_given_seed() {
        let b = make_battle();
        // Equal-speed equal-priority — depends purely on RNG tiebreak.
        let p1 = [
            Choice::Move { actor_slot: 0, move_slot: 0, target: Some(t(SideRef::P2, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let p2 = [
            Choice::Move { actor_slot: 0, move_slot: 1, target: Some(t(SideRef::P1, 0)) },
            Choice::Pass { actor_slot: 1 },
        ];
        let a = action_order(&b, &p1, &p2, &mut Rng::new(123));
        let b2 = action_order(&b, &p1, &p2, &mut Rng::new(123));
        assert_eq!(a, b2);
    }
}
