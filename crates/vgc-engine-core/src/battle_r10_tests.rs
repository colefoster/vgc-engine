//! Accuracy round 10 (docs/accuracy/fix-log.md).

use super::*;
use crate::choice::{Choice, Target};
use crate::team::TeamBuilder;

fn t(side: SideRef, slot: u8) -> Target {
    Target { side, slot }
}

fn mv(slot: u8, move_slot: u8, target: Option<Target>) -> Choice {
    Choice::Move { actor_slot: slot, move_slot, target }
}

fn singles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Singles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

#[test]
fn mega_sol_solar_beam_skips_the_charge() {
    // PS data/moves.ts:17238 solarbeam onTryMove skips the charge when
    // attacker.effectiveWeather() is sun; sim/pokemon.ts:2193 reports sun
    // during a Mega Sol user's move. Study 4598041866.
    let mut b = singles(
        r#"[{"species":"venusaur","level":50,"ability":"overgrow","moves":["solarbeam"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
        3,
    );
    b.p1.team[0].ability_id = data::ability_id::MEGASOL;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].charging_turns, 0);
    assert!(b.p2.team[0].current_hp < b.p2.team[0].stats.hp, "fired on turn 1");
}

#[test]
fn ability_immunities_block_targeted_status_moves() {
    // PS data/abilities.ts sapsipper / soundproof / oblivious onTryHit block
    // status moves too: Sap Sipper (Grass, +1 Atk), Soundproof (sound
    // flag), Oblivious (Attract / Captivate / Taunt). Studies 4b2729d196,
    // f9d89b1fdd (Sap Sipper), 6d3933e698 (Parting Shot into Soundproof),
    // 7bd3433b14 (Taunt into Oblivious).
    let cases = [
        ("sapsipper", "leechseed"),
        ("soundproof", "partingshot"),
        ("oblivious", "taunt"),
    ];
    for (ability, mv_name) in cases {
        let mut b = singles(
            &format!(r#"[{{"species":"whimsicott","level":50,"ability":"prankster","moves":["{mv_name}"]}},
                {{"species":"pikachu","level":50,"ability":"static","moves":["splash"]}}]"#),
            &format!(r#"[{{"species":"slowbro","level":50,"ability":"{ability}","moves":["splash"]}}]"#),
            3,
        );
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
        let foe = &b.p2.team[0];
        let atk = if ability == "sapsipper" { 1 } else { 0 };
        assert_eq!(foe.boosts[0], atk, "{ability}: Atk");
        assert_eq!(foe.boosts[2], 0, "{ability}: SpA");
        assert!(!foe.has_leech_seed(), "{ability}: no seed");
        assert_eq!(foe.taunt_turns(), 0, "{ability}: no taunt");
    }
}

fn doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

#[test]
fn friend_guard_holds_for_the_whole_spread_hit() {
    // PS spreadMoveHit computes every target's damage (getDamage, where
    // friendguard onAnyModifyDamage runs) before any HP is dealt, so a
    // Friend Guard holder the spread move KOs still guards its ally.
    // Studies 8c0a76b5df, 5095129db1, 1cee6e86b6.
    let run = |holder_hp: u16| {
        let mut b = doubles(
            r#"[{"species":"garchomp","level":50,"ability":"roughskin","nature":"adamant","moves":["rockslide"],"evs":{"atk":252}},
                {"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#,
            r#"[{"species":"maushold","level":50,"ability":"friendguard","moves":["splash"]},
                {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
            3,
        );
        b.p2.team[0].current_hp = holder_hp;
        let before = b.p2.team[1].current_hp;
        b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
        before - b.p2.team[1].current_hp
    };
    // Same seed, same rolls: the ally takes the same damage whether or not
    // the holder survives the hit.
    assert_eq!(run(1), run(150));
}

#[test]
fn sitrus_fires_right_after_sand_chip_before_grassy_heal() {
    // PS sandstorm onFieldResidual runs eachEvent('Weather') then
    // eachEvent('Update') (data/conditions.ts sandstorm; sim/battle.ts
    // eachEvent), so Sitrus (onUpdate) eats before Grassy Terrain's heal
    // (residualOrder 5). Study 252c896fb8: 93 -> 82 (sand) -> 127 -> 138.
    let mut b = singles(
        r#"[{"species":"incineroar","level":50,"ability":"intimidate","item":"sitrusberry","moves":["splash"],"evs":{"hp":252}}]"#,
        r#"[{"species":"tyranitar","level":50,"ability":"unnerve","moves":["splash"]}]"#,
        3,
    );
    b.set_weather(crate::weather::Weather::Sand);
    b.weather_turns = 5;
    b.set_terrain(crate::terrain::Terrain::Grassy);
    b.terrain_turns = 5;
    b.p2.team[0].ability_id = data::ability_id::SANDSTREAM;
    let max = b.p1.team[0].stats.hp;
    b.p1.team[0].current_hp = max / 2 + 3;
    let hp = b.p1.team[0].current_hp;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    let sand = (max / 16).max(1);
    assert_eq!(b.p1.team[0].current_hp, hp - sand + max / 4 + max / 16);
}

#[test]
fn trick_fails_against_a_mega_stone_holder() {
    // PS data/moves.ts trick onHit: target.takeItem() is false for a Mega
    // Stone of the holder's species (data/items.ts salamencite onTakeItem).
    // Study 4bb7820163 (Trick into Salamence-Mega).
    let mut b = singles(
        r#"[{"species":"indeedee","level":50,"ability":"psychicsurge","item":"focussash","moves":["trick"]}]"#,
        r#"[{"species":"salamence","level":50,"ability":"intimidate","item":"salamencite","moves":["splash"]}]"#,
        3,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[Choice::MegaEvolve { actor_slot: 0, move_slot: 0, target: None }]);
    assert_eq!(b.p2.team[0].item_id, data::item_id::SALAMENCITE);
    assert_eq!(b.p1.team[0].item_id, data::item_id::FOCUSSASH);
}

#[test]
fn mental_herb_cures_disable() {
    // PS data/items.ts mentalherb onUpdate removes disable among its
    // volatiles. Study 9af2fea75f.
    let mut b = singles(
        r#"[{"species":"sableye","level":50,"ability":"keeneye","moves":["disable"]}]"#,
        r#"[{"species":"farigiraf","level":50,"ability":"armortail","item":"mentalherb","moves":["tackle"]}]"#,
        3,
    );
    // Farigiraf moves first, so Sableye's Disable lands on Tackle.
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p2.team[0].disabled_move_slot(), 255);
    assert_eq!(b.p2.team[0].item_id, u16::MAX);
}

#[test]
fn life_orb_recoil_after_a_disguise_hit() {
    // PS sim/battle-actions.ts:536-539: AfterMoveSecondarySelf (Life Orb's
    // recoil, data/items.ts:3413) runs whenever the move hit, and a hit
    // Disguise absorbs is a hit. Study e5af60d10c.
    let mut b = singles(
        r#"[{"species":"toxtricity","level":50,"ability":"punkrock","item":"lifeorb","moves":["overdrive"]}]"#,
        r#"[{"species":"mimikyu","level":50,"ability":"disguise","moves":["splash"]}]"#,
        3,
    );
    let max = b.p1.team[0].stats.hp;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert!(b.p2.team[0].disguise_busted);
    assert_eq!(b.p1.team[0].current_hp, max - max / 10);
}

#[test]
fn burning_jealousy_burns_a_target_whose_stats_rose_this_turn() {
    // PS data/moves.ts burningjealousy secondary onHit: trySetStatus('brn')
    // when target.statsRaisedThisTurn (set by sim/battle.ts boost, cleared
    // at endTurn). Study 63ba3973ec (Calm Mind, then Burning Jealousy).
    let run = |foe_move: &str| {
        let mut b = singles(
            r#"[{"species":"torkoal","level":50,"ability":"drought","moves":["burningjealousy"]}]"#,
            &format!(r#"[{{"species":"floette","level":50,"ability":"flowerveil","moves":["{foe_move}"]}}]"#),
            3,
        );
        b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
        b.p2.team[0].status
    };
    assert!(matches!(run("calmmind"), Status::Burn));
    assert!(matches!(run("splash"), Status::None));
}

#[test]
fn disable_on_a_target_that_already_moved_lasts_five_turns() {
    // PS data/moves.ts disable condition: duration 5, and onStart takes one
    // off only when the target still has its move queued. Study
    // 9af2fea75f (Eruption disabled on turn 6 ends on turn 10).
    let mut b = singles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["disable","splash"]}]"#,
        r#"[{"species":"jolteon","level":50,"ability":"voltabsorb","moves":["tackle","splash"]}]"#,
        3,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert_eq!(b.p2.team[0].disabled_move_slot(), 0);
    for _ in 0..3 {
        b.step(&[mv(0, 1, None)], &[mv(0, 1, None)]);
    }
    assert_eq!(b.p2.team[0].disabled_move_slot(), 0, "still disabled after 4 residuals");
    b.step(&[mv(0, 1, None)], &[mv(0, 1, None)]);
    assert_eq!(b.p2.team[0].disabled_move_slot(), 255, "ends at the fifth");
}

#[test]
fn solar_power_chips_in_the_weather_step_before_grassy_heal() {
    // PS solarpower onWeather (data/abilities.ts:4403) runs in sunnyday's
    // eachEvent('Weather'), before Grassy Terrain's residual heal.
    // Study 966745efb4: 182 -> 160 -> 171.
    let mut b = singles(
        r#"[{"species":"houndoom","level":50,"ability":"solarpower","moves":["splash"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#,
        3,
    );
    b.set_weather(crate::weather::Weather::Sun);
    b.weather_turns = 5;
    b.set_terrain(crate::terrain::Terrain::Grassy);
    b.terrain_turns = 5;
    let max = b.p1.team[0].stats.hp;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].current_hp, max - max / 8 + max / 16);
}

#[test]
fn sitrus_eats_when_a_faint_replacement_comes_in_at_half_hp() {
    // PS sitrusberry onUpdate runs in the replacement's runSwitch Update.
    // Study a6f2f065f3 (Farigiraf replacement at 113/227 eats at once).
    let mut b = singles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]},
            {"species":"farigiraf","level":50,"ability":"armortail","item":"sitrusberry","moves":["splash"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["earthquake"]}]"#,
        3,
    );
    b.decision_phases = true;
    b.p1.team[0].current_hp = 1;
    let max = b.p1.team[1].stats.hp;
    b.p1.team[1].current_hp = max / 2;
    let hp = b.p1.team[1].current_hp;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert!(b.needs_replacements());
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[Choice::Pass { actor_slot: 0 }]);
    assert_eq!(b.p1.active_mon(0).unwrap().current_hp, hp + max / 4);
}

#[test]
fn sticky_web_drop_triggers_defiant() {
    // PS stickyweb onSwitchIn: this.boost({spe: -1}, pokemon,
    // pokemon.side.foe.active[0], ...), a foe-sourced drop, so Defiant's
    // onAfterEachBoost fires. Study 7d354ee6d9.
    let mut b = singles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]},
            {"species":"kingambit","level":50,"ability":"defiant","moves":["splash"]}]"#,
        r#"[{"species":"ribombee","level":50,"ability":"shielddust","moves":["stickyweb"]}]"#,
        3,
    );
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert!(b.p1.conditions.sticky_web);
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[mv(0, 0, None)]);
    let m = b.p1.active_mon(0).unwrap();
    assert_eq!((m.boosts[0], m.boosts[4]), (2, -1));
}

#[test]
fn aromatic_mist_raises_the_allys_special_defense() {
    // PS data/moves.ts aromaticmist: target adjacentAlly, boosts {spd: 1}.
    // Study 6d4e9037cb.
    let mut b = doubles(
        r#"[{"species":"alcremie","level":50,"ability":"aromaveil","moves":["aromaticmist"]},
            {"species":"sinistcha","level":50,"ability":"hospitality","moves":["splash"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#,
        3,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[1].boosts[3], 1);
    assert_eq!(b.p1.team[0].boosts[3], 0);
}

#[test]
fn stone_axe_sets_rocks_before_life_orb_ko() {
    // PS stoneaxe onAfterHit (source.hp) runs in the hit, before Life Orb's
    // AfterMoveSecondarySelf (sim/battle-actions.ts:536). Study b00eb62aee.
    let mut b = singles(
        r#"[{"species":"kleavor","level":50,"ability":"sharpness","item":"lifeorb","moves":["stoneaxe"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"],"evs":{"hp":252}}]"#,
        3,
    );
    b.p1.team[0].current_hp = 1;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    assert!(b.p2.conditions.stealth_rock);
}

#[test]
fn imposter_copies_the_moves_and_reverts_on_switch_out() {
    // PS sim/pokemon.ts:1305-1326 transformInto: moveSlots become the
    // target's moves at min(5, pp) PP; clearVolatile on switch-out restores
    // the base species, ability and moves. Studies 5abc255afc, ae0f734f2d.
    let mut b = singles(
        r#"[{"species":"ditto","level":50,"ability":"imposter","moves":["transform"]},
            {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#,
        r#"[{"species":"charizard","level":50,"ability":"blaze","moves":["dragondance","splash"]}]"#,
        3,
    );
    let ditto = b.p1.team[0].clone();
    assert_eq!(ditto.species_id, data::species_id::CHARIZARD, "Imposter transformed");
    assert_eq!(ditto.moves[0], data::move_id::DRAGONDANCE);
    assert_eq!(ditto.pp[0], 5);
    b.step(&[mv(0, 0, None)], &[mv(0, 1, None)]);
    assert_eq!(b.p1.team[0].boosts[0], 1, "used the copied Dragon Dance");
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[mv(0, 1, None)]);
    let d = &b.p1.team[0];
    assert_eq!(d.species_id, data::species_id::DITTO);
    assert_eq!(d.moves[0], data::move_id::TRANSFORM);
    assert_eq!(d.effective_ability_id(), data::ability_id::IMPOSTER);
}

#[test]
fn parental_bond_hits_twice_the_second_at_a_quarter() {
    // PS data/abilities.ts parentalbond onPrepareHit: multihit 2 for a
    // single-target damaging move; data/mods/champions/scripts.ts:209
    // modifyDamage: hit 2 at x0.25. Rocky Helmet fires per hit. Studies
    // 91790f18a3, d17595827b.
    let mut b = singles(
        r#"[{"species":"kangaskhan","level":50,"ability":"parentalbond","moves":["doubleedge"],"nature":"adamant","evs":{"atk":252}}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","item":"rockyhelmet","moves":["splash"],"evs":{"hp":252}}]"#,
        3,
    );
    let max = b.p1.team[0].stats.hp;
    let foe_max = b.p2.team[0].stats.hp;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
    let dealt = foe_max - b.p2.team[0].current_hp;
    let helmet = 2 * (max / 6);
    let recoil = (dealt as u32 * 33 + 50) / 100;
    assert_eq!(b.p1.team[0].current_hp as u32, max as u32 - helmet as u32 - recoil);
}

#[test]
fn shield_dust_blocks_fake_out_flinch() {
    // PS data/abilities.ts shielddust onModifySecondaries keeps only self
    // secondaries; Fake Out's flinch is a target secondary. Study
    // e33011c991 (Vivillon still uses Sleep Powder).
    let mut b = singles(
        r#"[{"species":"vivillon","level":50,"ability":"shielddust","moves":["sleeppowder"]}]"#,
        r#"[{"species":"jolteon","level":50,"ability":"voltabsorb","moves":["fakeout"]}]"#,
        3,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    assert!(matches!(b.p2.team[0].status, Status::Sleep));
}

#[test]
fn harvest_restores_a_berry_in_sun() {
    // PS data/abilities.ts:1800 harvest onResidual: in sun (no roll), or on
    // randomChance(1, 2), an itemless holder regains its last berry.
    // Study 7adc8cfb39 (Arboliva).
    let mut b = singles(
        r#"[{"species":"arboliva","level":50,"ability":"harvest","item":"sitrusberry","moves":["splash"]}]"#,
        r#"[{"species":"garchomp","level":50,"ability":"roughskin","moves":["dragonrage"]}]"#,
        3,
    );
    b.set_weather(crate::weather::Weather::Sun);
    b.weather_turns = 5;
    b.p1.team[0].current_hp = b.p1.team[0].stats.hp / 2 + 30;
    b.step(&[mv(0, 0, None)], &[mv(0, 0, None)]);
    assert_eq!(b.p1.team[0].item_id, data::item_id::SITRUSBERRY, "berry regrown");
}

#[test]
fn healer_cures_before_burn_damage() {
    // PS healer onResidualOrder 5 (data/abilities.ts:1817) runs before
    // brn's residual (order 10), so a cured ally takes no burn chip that
    // turn. Same order for Shed Skin / Hydration. Study 0ecfd594d5.
    let mut cured = 0;
    for seed in 0..40 {
        let mut b = doubles(
            r#"[{"species":"aromatisse","level":50,"ability":"healer","moves":["splash"]},
                {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#,
            r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]},
                {"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#,
            seed,
        );
        b.p1.team[1].status = Status::Burn;
        b.sync_status_dot_bit(SideRef::P1, 1);
        let hp = b.p1.team[1].current_hp;
        b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
        if matches!(b.p1.team[1].status, Status::None) {
            cured += 1;
            assert_eq!(b.p1.team[1].current_hp, hp, "seed {seed}: cured before the chip");
        }
    }
    assert!(cured > 0);
}

#[test]
fn status_moves_cannot_miss_a_glaive_rush_user() {
    // PS data/moves.ts glaiverush condition onAccuracy: return true — moves
    // aimed at the user skip the accuracy roll, status moves included.
    // Study 4111a15689 (Sleep Powder into Baxcalibur after Glaive Rush).
    for seed in 0..20 {
        let mut b = singles(
            r#"[{"species":"gengar","level":50,"ability":"cursedbody","moves":["hypnosis"]}]"#,
            r#"[{"species":"baxcalibur","level":50,"ability":"thermalexchange","moves":["splash"]}]"#,
            seed,
        );
        b.p2.team[0].volatiles.add(crate::pokemon::Volatile {
            kind: crate::pokemon::VolatileKind::GlaiveRush,
            turns_remaining: 2,
            payload: 0,
        });
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0)))], &[mv(0, 0, None)]);
        assert!(matches!(b.p2.team[0].status, Status::Sleep), "seed {seed}");
    }
}

// ---- Partial trap: the timer ends before the chip ----
//
// data/conditions.ts:222-247 partiallytrapped: durationCallback random(5, 7),
// onResidualOrder 13. sim/battle.ts:515-522 runs the residual by decrementing
// the condition's duration FIRST; at 0 it ends the condition and skips its
// onResidual. So a timer of 5 / 6 chips 4 / 5 times (turns 1..timer-1) and
// the expiry turn deals nothing. Bulbapedia:
// <https://bulbapedia.bulbagarden.net/wiki/Infestation_(move)> (4-5 turns).

const TRAP_TURNS: usize = 8;

/// Toxapex Infestations p2a on turn 1, then everyone Splashes. Returns the
/// trap timer (the counter left after turn 1, plus the one turn-1 tick),
/// p2a's HP after each turn, and whether it was still trapped.
fn infestation_run(target: &str, seed: u64) -> (usize, [u16; TRAP_TURNS], [bool; TRAP_TURNS]) {
    let mut b = doubles(
        r#"[{"species":"toxapex","level":50,"moves":["infestation","splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        &format!(r#"[{target},{{"species":"blissey","level":50,"moves":["splash"]}}]"#),
        seed,
    );
    let (mut hp, mut trapped) = ([0u16; TRAP_TURNS], [false; TRAP_TURNS]);
    let mut timer = 0;
    for turn in 0..TRAP_TURNS {
        let p1_move = if turn == 0 { mv(0, 0, Some(t(SideRef::P2, 0))) } else { mv(0, 1, None) };
        b.step(&[p1_move, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
        let foe = &b.p2.team[0];
        hp[turn] = foe.current_hp;
        let v = foe.volatiles.get(crate::pokemon::VolatileKind::PartialTrap);
        trapped[turn] = v.is_some();
        if turn == 0 {
            timer = v.expect("Infestation traps p2a").payload as usize & 0xFF;
            timer += 1;
        }
    }
    (timer, hp, trapped)
}

#[test]
fn infestation_chips_one_turn_less_than_its_timer() {
    let snorlax = r#"{"species":"snorlax","level":50,"ability":"immunity","moves":["splash"]}"#;
    let mut seen = [false; 2];
    for seed in 0..24 {
        let (timer, hp, trapped) = infestation_run(snorlax, seed);
        assert!((5..=6).contains(&timer), "seed {seed}: timer {timer}");
        seen[timer - 5] = true;
        // Turn 1 (index 0) chips with the hit; turns 2..timer-1 chip alone.
        let chip = hp[0] - hp[1];
        assert!(chip > 0, "seed {seed}: no turn-2 chip");
        for turn in 1..timer - 1 {
            assert_eq!(hp[turn - 1] - hp[turn], chip, "seed {seed}: turn {} chip", turn + 1);
            assert!(trapped[turn], "seed {seed}: freed early on turn {}", turn + 1);
        }
        // The expiry turn ends the trap with no chip; nothing after.
        let expiry = timer - 1;
        assert!(!trapped[expiry], "seed {seed}: still trapped after turn {timer}");
        for turn in expiry..TRAP_TURNS {
            assert_eq!(hp[turn], hp[expiry - 1], "seed {seed}: chip on turn {} (timer {timer})", turn + 1);
        }
    }
    assert_eq!(seen, [true, true], "both timers exercised");
}

#[test]
fn partial_trap_under_magic_guard_still_expires_on_time() {
    // Magic Guard stops the chip, not the timer.
    let clefable = r#"{"species":"clefable","level":50,"ability":"magicguard","moves":["splash"]}"#;
    let mut seen = [false; 2];
    for seed in 0..24 {
        let (timer, hp, trapped) = infestation_run(clefable, seed);
        assert!((5..=6).contains(&timer), "seed {seed}: timer {timer}");
        seen[timer - 5] = true;
        assert!(hp.iter().all(|&h| h == hp[0]), "seed {seed}: Magic Guard took a chip: {hp:?}");
        for (turn, &on) in trapped.iter().enumerate() {
            assert_eq!(on, turn + 1 < timer, "seed {seed}: trapped after turn {} (timer {timer})", turn + 1);
        }
    }
    assert_eq!(seen, [true, true], "both timers exercised");
}

#[test]
fn partial_trap_ends_without_a_chip_when_its_source_faints() {
    // data/conditions.ts partiallytrapped onResidual: a fainted source
    // removes the volatile and deals nothing. Blissey's Seismic Toss KOs a
    // 1-HP Toxapex on turn 2 (no bench: nobody replaces it).
    let mut b = doubles(
        r#"[{"species":"toxapex","level":50,"moves":["infestation","splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"snorlax","level":50,"ability":"immunity","moves":["splash"]},{"species":"blissey","level":50,"moves":["seismictoss","splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 1, None)]);
    assert!(b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::PartialTrap));
    b.p1.team[0].current_hp = 1;
    let before = b.p2.team[0].current_hp;
    b.step(&[mv(0, 1, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, Some(t(SideRef::P1, 0)))]);
    assert!(b.p1.team[0].fainted, "Seismic Toss should KO Toxapex");
    assert_eq!(b.p2.team[0].current_hp, before, "no chip once Toxapex fainted");
    assert!(!b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::PartialTrap));
}

// ---- Champions Encore replaces the queued action at once ----
//
// data/mods/champions/moves.ts:307-339 encore onStart: when the target still
// has a queued move other than the Encored one (and holds no Mental Herb),
// `queue.changeAction` re-queues the Encored move (sim/battle-queue.ts:
// 301-305 -> insertChoice :369-402). resolveAction picks its target there
// (:268-275 getRandomTarget, sim/battle.ts:2490-2522), and the gen-8+
// re-sort orders it by its own priority. Standard gen 9 keeps the old
// onOverrideAction at execution (`encore_hits_its_chosen_target_and_
// overrides_that_turns_move` in battle.rs). Bulbapedia:
// <https://bulbapedia.bulbagarden.net/wiki/Encore_(move)#Pok%C3%A9mon_Champions>.

const CHAMPIONS_ID: &str = "gen9championsvgc2026regmc";

fn encore_battle(rng: crate::rng::Rng, champions: bool, p1: &str, p2: &str) -> Battle {
    let mut b = Battle::with_rng(
        BattleConfig { format: Format::Doubles, seed: 0 },
        rng,
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    );
    b.set_format_id(if champions { CHAMPIONS_ID } else { "gen9doublescustomgame" });
    b
}

#[test]
fn champions_encore_forced_quick_guard_uses_its_own_priority() {
    // Turn 1 Conkeldurr Quick Guards; turn 2 it queues Splash and is Encored.
    // Champions: the forced Quick Guard (+3) goes up before Pikachu's Quick
    // Attack (+1) and blocks it. Standard gen 9: it keeps Splash's 0 and the
    // Quick Attack lands first.
    let p1 = r#"[{"species":"whimsicott","level":50,"ability":"prankster","moves":["encore","splash"]},{"species":"pikachu","level":50,"ability":"static","moves":["quickattack","splash"]}]"#;
    let p2 = r#"[{"species":"conkeldurr","level":50,"ability":"guts","moves":["quickguard","splash"]},{"species":"miltank","level":50,"ability":"sapsipper","moves":["splash"]}]"#;
    for champions in [true, false] {
        for seed in 1..5 {
            let mut b = encore_battle(crate::rng::Rng::Splitmix(seed), champions, p1, p2);
            b.step(&[mv(0, 1, None), mv(1, 1, None)], &[mv(0, 0, None), mv(1, 0, None)]);
            b.step(
                &[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 0)))],
                &[mv(0, 1, None), mv(1, 0, None)],
            );
            let conk = &b.p2.team[0];
            assert!(conk.encore_turns() > 0, "champions={champions} seed {seed}: Encore landed");
            assert_eq!(conk.current_hp == conk.stats.hp, champions, "champions={champions} seed {seed}: hp {}", conk.current_hp);
            assert!(b.encore_requeue.is_none(), "no request outlives the step");
        }
    }
}

// Turn 1 p2a Snorlax Tackles p1b; turn 2 Whimsicott Encores it while it
// queued Splash and Pikachu (p1b, faster than Snorlax) Tackles Miltank.
const ENC_P1: &str = r#"[{"species":"whimsicott","level":50,"ability":"prankster","moves":["encore","splash"]},{"species":"pikachu","level":50,"ability":"static","moves":["tackle","splash"]}]"#;
const ENC_P2: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["tackle","splash"]},{"species":"miltank","level":50,"ability":"sapsipper","moves":["splash"]}]"#;

/// Turn-2 recorded draws keyed to p2a's Tackle with no target: its
/// getRandomTarget pick.
fn retarget_draws(log: &[crate::rng::RecordedDraw]) -> Vec<usize> {
    use crate::rng::{RngDecision, NO_SLOT};
    log.iter()
        .enumerate()
        .filter(|(_, d)| d.key.actor == 2 && d.key.target == NO_SLOT && d.key.move_id == data::move_id::TACKLE && d.key.decision == RngDecision::Range)
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn champions_encore_draws_the_forced_target_when_it_lands() {
    // Champions: one target draw, at Encore application, before Pikachu's
    // move. Standard gen 9: the same one draw, at Snorlax's execution.
    for champions in [true, false] {
        let mut b = encore_battle(crate::rng::Rng::recording(11), champions, ENC_P1, ENC_P2);
        b.step(&[mv(0, 1, None), mv(1, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
        let _ = b.rng_mut().take_recording_log();
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 1)))], &[mv(0, 1, None), mv(1, 0, None)]);
        let log = b.rng_mut().take_recording_log().unwrap();
        let picks = retarget_draws(&log);
        assert_eq!(picks.len(), 1, "champions={champions}: {log:?}");
        assert_eq!(log[picks[0]].space, crate::rng::DrawSpace::UniformRange(2));
        let pikachu = log.iter().position(|d| d.key.actor == 1).expect("Pikachu's Tackle draws");
        assert_eq!(picks[0] < pikachu, champions, "champions={champions}: pick {} vs Pikachu {pikachu}", picks[0]);
    }
}

#[test]
fn champions_encore_forced_move_hits_the_drawn_foe() {
    use crate::rng::{Rng, RngDecision, RngEvent, RngKey, NO_SLOT};
    let key = RngKey { turn: 2, actor: 2, target: NO_SLOT, move_id: data::move_id::TACKLE, decision: RngDecision::Range };
    for pick in [0u32, 1] {
        let mut table = std::collections::HashMap::new();
        table.insert(key, std::collections::VecDeque::from([RngEvent::Range(pick)]));
        let mut b = encore_battle(Rng::oracle_keyed(table, 5), true, ENC_P1, ENC_P2);
        b.step(&[mv(0, 1, None), mv(1, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
        let before = [b.p1.team[0].current_hp, b.p1.team[1].current_hp];
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 1)))], &[mv(0, 1, None), mv(1, 0, None)]);
        let hit = [b.p1.team[0].current_hp < before[0], b.p1.team[1].current_hp < before[1]];
        assert_eq!(hit, [pick == 0, pick == 1], "pick {pick}");
        assert!(b.rng().keyed_leftovers().unwrap().iter().all(|(k, _)| *k != key), "pick {pick}: not consumed");
        let misses = b.rng_mut().take_miss_log().unwrap();
        assert!(misses.iter().all(|m| m.key != key), "pick {pick}: drawn twice");
    }
}

#[test]
fn champions_encore_requeue_guards_draw_nothing() {
    let none = |b: &mut Battle| retarget_draws(&b.rng_mut().take_recording_log().unwrap()).is_empty();
    // Mental Herb: no re-queue; the herb cures Encore and Splash stands.
    let herb = ENC_P2.replacen(r#""ability":"thickfat","#, r#""ability":"thickfat","item":"mentalherb","#, 1);
    let mut b = encore_battle(crate::rng::Rng::recording(3), true, ENC_P1, &herb);
    b.step(&[mv(0, 1, None), mv(1, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    let _ = b.rng_mut().take_recording_log();
    let before = [b.p1.team[0].current_hp, b.p1.team[1].current_hp];
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 1, None)], &[mv(0, 1, None), mv(1, 0, None)]);
    assert!(none(&mut b), "Mental Herb: no target draw");
    assert_eq!(b.p2.team[0].encore_turns(), 0);
    assert_eq!([b.p1.team[0].current_hp, b.p1.team[1].current_hp], before, "Splash, not Tackle");

    // Same queued move: nothing changes; the chosen target stands.
    let mut b = encore_battle(crate::rng::Rng::recording(3), true, ENC_P1, ENC_P2);
    b.step(&[mv(0, 1, None), mv(1, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    let _ = b.rng_mut().take_recording_log();
    let before = b.p1.team[1].current_hp;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    assert!(none(&mut b), "same move: no target draw");
    assert!(b.p1.team[1].current_hp < before, "Tackle hit its chosen target");

    // Target already acted (slow Encore user): duration 4, no re-queue.
    let slow = r#"[{"species":"shuckle","level":50,"ability":"sturdy","moves":["encore","splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#;
    let fast = r#"[{"species":"pikachu","level":50,"ability":"static","moves":["tackle","splash"]},{"species":"miltank","level":50,"ability":"sapsipper","moves":["splash"]}]"#;
    let mut b = encore_battle(crate::rng::Rng::recording(3), true, slow, fast);
    b.step(&[mv(0, 1, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    let _ = b.rng_mut().take_recording_log();
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 1, None), mv(1, 0, None)]);
    let log = b.rng_mut().take_recording_log().unwrap();
    assert!(!log.iter().any(|d| d.key.actor == 2 && d.key.target == crate::rng::NO_SLOT && d.key.move_id == data::move_id::TACKLE));
    assert_eq!(b.p2.team[0].encore_turns(), 3, "4 turns, one spent");
}

#[test]
fn champions_encore_replaces_a_queued_struggle() {
    // data/mods/champions/moves.ts:326 only skips the same move and Mental
    // Herb. Rillaboom's only move is Fake Out, unselectable after its first
    // action (data/mods/champions/moves.ts:352), so turn 2 it must Struggle;
    // Encore turns that back into Fake Out, which then fails (not its first
    // action): no damage to Whimsicott, no Struggle recoil.
    let p1 = r#"[{"species":"whimsicott","level":50,"ability":"prankster","moves":["encore","splash"]},{"species":"pikachu","level":50,"ability":"static","moves":["splash"]}]"#;
    let p2 = r#"[{"species":"rillaboom","level":50,"ability":"overgrow","moves":["fakeout"]},{"species":"miltank","level":50,"ability":"sapsipper","moves":["splash"]}]"#;
    for seed in 1..5 {
        let mut b = encore_battle(crate::rng::Rng::Splitmix(seed), true, p1, p2);
        b.step(&[mv(0, 1, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
        let whimsicott = b.p1.team[0].current_hp;
        assert!(whimsicott < b.p1.team[0].stats.hp, "seed {seed}: turn-1 Fake Out hit");
        let lc = b.legal_choices(SideRef::P2, 0);
        assert!(!lc.is_empty(), "seed {seed}");
        assert!(
            lc.iter().all(|c| matches!(c, Choice::Move { move_slot: crate::choice::STRUGGLE_MOVE_SLOT, .. })),
            "seed {seed}: only Struggle, got {lc:?}"
        );
        b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[lc[0], mv(1, 0, None)]);
        let rilla = &b.p2.team[0];
        assert!(rilla.encore_turns() > 0, "seed {seed}: Encore landed");
        assert_eq!(rilla.encored_move_slot(), 0);
        assert_eq!(b.p1.team[0].current_hp, whimsicott, "seed {seed}: no Struggle damage");
        assert_eq!(rilla.current_hp, rilla.stats.hp, "seed {seed}: no Struggle recoil");
        assert_eq!(rilla.last_used_move_slot, 0, "seed {seed}: last move is still Fake Out");
    }
}

// An Encored adjacentAlly move (target code 2): getRandomTarget samples the
// user's living adjacent allies (sim/battle.ts:2503-2508), not its foes; no
// draw in Singles or without a living ally.
const MIST_P1: &str = r#"[{"species":"alcremie","level":50,"ability":"sweetveil","moves":["aromaticmist","splash"]},{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}]"#;
const MIST_P2: &str = r#"[{"species":"whimsicott","level":50,"ability":"prankster","moves":["encore","splash"]},{"species":"chansey","level":50,"ability":"naturalcure","moves":["splash"]}]"#;

/// Turn-2 recorded draws keyed to p1a's Aromatic Mist with no target.
fn mist_target_draws(log: &[crate::rng::RecordedDraw]) -> Vec<crate::rng::DrawSpace> {
    log.iter()
        .filter(|d| d.key.actor == 0 && d.key.target == crate::rng::NO_SLOT && d.key.move_id == data::move_id::AROMATICMIST && d.key.decision == crate::rng::RngDecision::Range)
        .map(|d| d.space)
        .collect()
}

#[test]
fn encore_forced_adjacent_ally_move_samples_the_ally() {
    for champions in [true, false] {
        let mut b = encore_battle(crate::rng::Rng::recording(7), champions, MIST_P1, MIST_P2);
        b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 1, None), mv(1, 0, None)]);
        assert_eq!(b.p1.team[1].boosts[3], 1, "turn-1 Aromatic Mist on the partner");
        let _ = b.rng_mut().take_recording_log();
        b.step(&[mv(0, 1, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
        let log = b.rng_mut().take_recording_log().unwrap();
        assert!(b.p1.team[0].encore_turns() > 0, "champions={champions}: Encore landed");
        // The pick is a one-outcome `random(1)` (seen in the ps-rng trace
        // below); a one-outcome draw is never recorded, so no foe sample
        // (`UniformRange(2)`) may show up here.
        assert_eq!(mist_target_draws(&log), [], "champions={champions}");
        assert_eq!(b.p1.team[1].boosts[3], 2, "champions={champions}: forced Aromatic Mist on the partner");
    }
}

#[test]
fn encore_forced_adjacent_ally_move_draws_nothing_without_an_ally() {
    // Partner fainted (doubles, nobody to replace it).
    let mut b = encore_battle(crate::rng::Rng::recording(7), true, MIST_P1, MIST_P2);
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 1, None), mv(1, 0, None)]);
    b.p1.team[1].current_hp = 0;
    b.p1.team[1].fainted = true;
    let _ = b.rng_mut().take_recording_log();
    b.step(&[mv(0, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
    let log = b.rng_mut().take_recording_log().unwrap();
    assert!(b.p1.team[0].encore_turns() > 0, "Encore landed");
    assert!(mist_target_draws(&log).is_empty(), "{log:?}");

    // Singles: getRandomTarget's adjacentAlly branch returns no target.
    let mut b = Battle::with_rng(
        BattleConfig { format: Format::Singles, seed: 0 },
        crate::rng::Rng::recording(7),
        TeamBuilder::from_json(r#"[{"species":"alcremie","level":50,"ability":"sweetveil","moves":["aromaticmist","splash"]}]"#).unwrap(),
        TeamBuilder::from_json(r#"[{"species":"whimsicott","level":50,"ability":"prankster","moves":["encore","splash"]}]"#).unwrap(),
    );
    b.set_format_id(CHAMPIONS_ID);
    b.p1.team[0].last_used_move_slot = 0;
    b.step(&[mv(0, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 0)))]);
    let log = b.rng_mut().take_recording_log().unwrap();
    assert!(b.p1.team[0].encore_turns() > 0, "Encore landed");
    assert!(mist_target_draws(&log).is_empty(), "{log:?}");
}

#[cfg(feature = "ps-rng")]
#[test]
fn ps_rng_encore_forced_adjacent_ally_move_draws_random_1() {
    let mut rng = crate::rng::Rng::ps("sodium,00000000000000000000000000000001").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    let mut b = encore_battle(rng, true, MIST_P1, MIST_P2);
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 1, None), mv(1, 0, None)]);
    let _ = b.rng_mut().ps_mut().unwrap().take_trace();
    b.step(&[mv(0, 1, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    let picks: Vec<(u32, u32)> = trace.iter().filter(|d| d.op == "random_target").map(|d| (d.a, d.b)).collect();
    assert_eq!(picks, [(0, 1)], "{trace:?}");
    assert_eq!(b.p1.team[1].boosts[3], 2, "forced Aromatic Mist on the partner");

    // Partner fainted: no living adjacent ally, no draw.
    let mut rng = crate::rng::Rng::ps("sodium,00000000000000000000000000000001").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    let mut b = encore_battle(rng, true, MIST_P1, MIST_P2);
    b.step(&[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)], &[mv(0, 1, None), mv(1, 0, None)]);
    b.p1.team[1].current_hp = 0;
    b.p1.team[1].fainted = true;
    let _ = b.rng_mut().ps_mut().unwrap().take_trace();
    b.step(&[mv(0, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 0))), mv(1, 0, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    assert!(b.p1.team[0].encore_turns() > 0, "Encore landed");
    assert!(!trace.iter().any(|d| d.op == "random_target"), "{trace:?}");
}

#[cfg(feature = "ps-rng")]
#[test]
fn ps_rng_champions_encore_draws_target_then_insertion_at_once() {
    // The target pick is drawn when Encore lands, before Pikachu's move.
    let mut rng = crate::rng::Rng::ps("sodium,00000000000000000000000000000001").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    let mut b = encore_battle(rng, true, ENC_P1, ENC_P2);
    b.step(&[mv(0, 1, None), mv(1, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    let _ = b.rng_mut().ps_mut().unwrap().take_trace();
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 1)))], &[mv(0, 1, None), mv(1, 0, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    let picks: Vec<usize> = trace.iter().enumerate().filter(|(_, d)| d.op == "random_target").map(|(i, _)| i).collect();
    assert_eq!(picks.len(), 1, "{trace:?}");
    let pikachu = trace.iter().position(|d| d.actor == 1).expect("Pikachu's draws");
    assert!(picks[0] < pikachu, "pick {} vs Pikachu {pikachu}", picks[0]);
    // No queued action ties the forced Tackle: no insertion draw.
    assert!(!trace.iter().any(|d| d.op == "insert_choice"), "{trace:?}");

    // p1b Snorlax ties p2a's forced Tackle (same Speed, priority 0) behind
    // the faster Miltank: insertChoice draws random(1, 3) right after the
    // target pick.
    let tie_p1 = r#"[{"species":"whimsicott","level":50,"ability":"prankster","moves":["encore","splash"]},{"species":"snorlax","level":50,"ability":"thickfat","moves":["tackle","splash"]}]"#;
    let mut rng = crate::rng::Rng::ps("sodium,00000000000000000000000000000001").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    let mut b = encore_battle(rng, true, tie_p1, ENC_P2);
    b.step(&[mv(0, 1, None), mv(1, 1, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    let _ = b.rng_mut().ps_mut().unwrap().take_trace();
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 1, None)], &[mv(0, 1, None), mv(1, 0, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    let pick = trace.iter().position(|d| d.op == "random_target").expect("target pick");
    let ins = &trace[pick + 1];
    assert_eq!((ins.op, ins.a, ins.b), ("insert_choice", 1, 3), "{trace:?}");
    assert_eq!(trace.iter().filter(|d| d.op == "insert_choice").count(), 1);
}

// ---- Pre-turn manual switches run in the leaving mon's Speed order ----
//
// Switch actions are order 103 sorted by comparePriority (sim/battle.ts:404)
// on the LEAVING mon's Speed (Trick Room flips it), ties shuffled by
// commitChoices' speedSort (:429). The incoming mon's Speed plays no part.
// Both sides swap a Chansey for a terrain setter: whoever switches in last
// sets the terrain that stays.

fn switch_battle(rng: crate::rng::Rng, p1_lead: &str, p2_lead: &str) -> Battle {
    let p1 = format!(r#"[{p1_lead},{{"species":"pikachu","level":50,"ability":"static","moves":["splash","quickattack"]}},{{"species":"indeedee","level":50,"ability":"psychicsurge","moves":["splash"]}}]"#);
    let p2 = format!(r#"[{p2_lead},{{"species":"snorlax","level":50,"ability":"thickfat","moves":["splash"]}},{{"species":"rillaboom","level":50,"ability":"grassysurge","moves":["splash"]}}]"#);
    let mut b = Battle::with_rng(
        BattleConfig { format: Format::Doubles, seed: 0 },
        rng,
        TeamBuilder::from_json(&p1).unwrap(),
        TeamBuilder::from_json(&p2).unwrap(),
    );
    b.set_format_id(CHAMPIONS_ID);
    b
}

const SW_CHANSEY: &str = r#"{"species":"chansey","level":50,"ability":"naturalcure","moves":["splash"]}"#;

fn switch_turn(b: &mut Battle) {
    b.step(
        &[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)],
        &[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)],
    );
}

#[test]
fn switch_order_tied_outgoing_speed_goes_either_way() {
    // Both Chanseys tie: the order is a coin flip, not the incoming Speed.
    let mut seen = [false; 2];
    for seed in 0..40 {
        let mut b = switch_battle(crate::rng::Rng::Splitmix(seed), SW_CHANSEY, SW_CHANSEY);
        switch_turn(&mut b);
        match b.terrain {
            crate::terrain::Terrain::Grassy => seen[0] = true,
            crate::terrain::Terrain::Psychic => seen[1] = true,
            other => panic!("seed {seed}: {other:?}"),
        }
    }
    assert_eq!(seen, [true, true], "both tie outcomes");
}

#[test]
fn switch_order_follows_the_leaving_mons_speed() {
    // p1's leaving Jolteon outspeeds p2's Chansey: p1 switches first, so
    // Rillaboom enters last (Grassy). Trick Room reverses it (Psychic).
    let jolteon = r#"{"species":"jolteon","level":50,"ability":"voltabsorb","moves":["splash"]}"#;
    for seed in 0..10 {
        for (tr, want) in [(false, crate::terrain::Terrain::Grassy), (true, crate::terrain::Terrain::Psychic)] {
            let mut b = switch_battle(crate::rng::Rng::Splitmix(seed), jolteon, SW_CHANSEY);
            if tr {
                b.trick_room_turns = 5;
            }
            switch_turn(&mut b);
            assert_eq!(b.terrain, want, "seed {seed} trick room {tr}");
        }
    }
}

#[test]
fn switch_order_keyed_commit_tie_is_not_drawn_again() {
    // Keyed oracle: commitChoices' sort draws the one switch tie; the
    // switch executor reuses that order and draws nothing.
    let mut b = switch_battle(crate::rng::Rng::oracle_keyed(Default::default(), 3), SW_CHANSEY, SW_CHANSEY);
    let _ = b.rng_mut().take_miss_log();
    switch_turn(&mut b);
    let misses = b.rng_mut().take_miss_log().unwrap();
    let ties = misses.iter().filter(|m| m.key.decision == crate::rng::RngDecision::Tiebreak).count();
    assert_eq!(ties, 1, "{misses:?}");
    assert!(matches!(b.terrain, crate::terrain::Terrain::Grassy | crate::terrain::Terrain::Psychic));
}

#[cfg(feature = "ps-rng")]
#[test]
fn switch_order_ps_rng_follows_the_commit_shuffle() {
    // commitChoices' speedSort: both switches (order 103, Speed tie) lead
    // the queue, shuffled by one random(0, 2). Draw 0 keeps p1 first
    // (Rillaboom last: Grassy); 1 swaps them (Psychic).
    let mut seen = [false; 2];
    for seed in 1..=8u32 {
        let mut rng = crate::rng::Rng::ps(&format!("sodium,{seed:032x}")).unwrap();
        rng.ps_mut().unwrap().enable_trace();
        let mut b = switch_battle(rng, SW_CHANSEY, SW_CHANSEY);
        let _ = b.rng_mut().ps_mut().unwrap().take_trace();
        switch_turn(&mut b);
        let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
        let commit = trace.iter().find(|d| d.op == "shuffle").expect("commit shuffle");
        assert_eq!((commit.a, commit.b), (0, 2), "seed {seed}: {trace:?}");
        let want = if commit.result == 0 { crate::terrain::Terrain::Grassy } else { crate::terrain::Terrain::Psychic };
        assert_eq!(b.terrain, want, "seed {seed}: draw {}", commit.result);
        seen[commit.result as usize] = true;
    }
    assert_eq!(seen, [true, true], "both tie outcomes across seeds");
}

// ---- Battle start: the leads' SwitchIn handlers follow runSwitch's sort ----
//
// sim/battle-actions.ts:175-184 runSwitch speed-sorts every active mon (tie
// shuffled by speedSort, sim/battle.ts:429) and fires their SwitchIn handlers
// in that order (resolvePriority's speedOrder, :1007-1012, so the handlers
// draw no further tie). Tied surge setters: whoever the shuffle puts last
// sets the terrain that stays. PHASE A: written before the fix.

// p1a Indeedee-F (Psychic Surge) and p2a Rillaboom (Grassy Surge) share base
// Speed 85 (tied); p1b Pikachu is faster, p2b Snorlax slower.
fn lead_tie_team(ps_text_p1: &str, ps_text_p2: &str, rng: crate::rng::Rng) -> Battle {
    Battle::with_rng(
        BattleConfig { format: Format::Doubles, seed: 0 },
        rng,
        TeamBuilder::from_showdown_text_in(ps_text_p1, true).unwrap(),
        TeamBuilder::from_showdown_text_in(ps_text_p2, true).unwrap(),
    )
}

const LEAD_P1: &str = "Indeedee-F\nAbility: Psychic Surge\nLevel: 50\n- Splash\n\nPikachu\nAbility: Static\nLevel: 50\n- Splash\n- Quick Attack\n\nChansey\nAbility: Natural Cure\nLevel: 50\n- Splash";
const LEAD_P2: &str = "Rillaboom\nAbility: Grassy Surge\nLevel: 50\n- Splash\n\nSnorlax\nAbility: Thick Fat\nLevel: 50\n- Splash\n\nBlissey\nAbility: Natural Cure\nLevel: 50\n- Splash";

#[test]
fn lead_switch_in_tie_reaches_both_terrains() {
    // Native Splitmix: the tied leads' order is a coin flip.
    let mut seen = [false; 2];
    for seed in 0..40 {
        let b = lead_tie_team(LEAD_P1, LEAD_P2, crate::rng::Rng::Splitmix(seed));
        assert_eq!(b.p1.team[0].stats.spe, b.p2.team[0].stats.spe, "fixture: surge setters tie");
        match b.terrain {
            crate::terrain::Terrain::Grassy => seen[0] = true,
            crate::terrain::Terrain::Psychic => seen[1] = true,
            other => panic!("seed {seed}: {other:?}"),
        }
    }
    assert_eq!(seen, [true, true], "both tie outcomes");
}

#[test]
fn lead_switch_in_follows_the_faster_setter() {
    // Unequal Speed: the slower setter's surge goes last and stays.
    let fast_rilla = LEAD_P2.replacen("Level: 50\n", "Level: 50\nEVs: 32 Spe\n", 1);
    let fast_indeedee = LEAD_P1.replacen("Level: 50\n", "Level: 50\nEVs: 32 Spe\n", 1);
    for seed in 0..10 {
        let b = lead_tie_team(LEAD_P1, &fast_rilla, crate::rng::Rng::Splitmix(seed));
        assert_eq!(b.terrain, crate::terrain::Terrain::Psychic, "seed {seed}: faster Rillaboom, slower Indeedee last");
        let b = lead_tie_team(&fast_indeedee, LEAD_P2, crate::rng::Rng::Splitmix(seed));
        assert_eq!(b.terrain, crate::terrain::Terrain::Grassy, "seed {seed}: faster Indeedee, slower Rillaboom last");
    }
}

#[test]
fn lead_switch_in_keyed_start_tiebreak_picks_the_order() {
    // Keyed oracle: the leads' runSwitch tie takes PS's offset from the start
    // key (turn 0, no actor / target, move u16::MAX, Tiebreak). After the
    // selection sort puts Pikachu first, the Indeedee-F / Rillaboom tie sits
    // at 1..3: offset 0 keeps Indeedee-F first (Grassy), 1 swaps (Psychic).
    use crate::rng::{Rng, RngDecision, RngEvent, RngKey, NO_SLOT};
    let key = RngKey { turn: 0, actor: NO_SLOT, target: NO_SLOT, move_id: u16::MAX, decision: RngDecision::Tiebreak };
    for (v, want) in [(0u64, crate::terrain::Terrain::Grassy), (1, crate::terrain::Terrain::Psychic)] {
        let mut table = std::collections::HashMap::new();
        table.insert(key, std::collections::VecDeque::from([RngEvent::Tiebreak(v)]));
        let mut b = lead_tie_team(LEAD_P1, LEAD_P2, Rng::oracle_keyed(table, 9));
        assert_eq!(b.terrain, want, "offset {v}");
        assert!(b.rng().keyed_leftovers().unwrap().iter().all(|(k, _)| *k != key), "offset {v}: not consumed");
        let misses = b.rng_mut().take_miss_log().unwrap();
        assert!(!misses.iter().any(|m| m.key.decision == RngDecision::Tiebreak), "offset {v}: extra tie draw {misses:?}");
    }
}

#[cfg(feature = "ps-rng")]
#[test]
fn lead_switch_in_ps_rng_follows_the_run_switch_shuffle() {
    // runSwitch's sort over [p1a Indeedee-F, p1b Pikachu, p2a Rillaboom,
    // p2b Snorlax]: Pikachu first, then the Indeedee-F / Rillaboom tie at
    // positions 1..3, shuffled by one random(1, 3). 1 keeps Indeedee-F ahead
    // (Rillaboom last: Grassy); 2 swaps them (Psychic). Team preview's own
    // tie shuffle (team slot 0: random(0, 2)) comes earlier and does not
    // order the abilities.
    let mut seen = [false; 2];
    for seed in 1..=8u32 {
        let mut rng = crate::rng::Rng::ps(&format!("sodium,{seed:032x}")).unwrap();
        rng.ps_mut().unwrap().enable_trace();
        let mut b = lead_tie_team(LEAD_P1, LEAD_P2, rng);
        let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
        let run_switch = trace
            .iter()
            .find(|d| d.op == "shuffle" && d.a == 1 && d.b == 3)
            .unwrap_or_else(|| panic!("seed {seed}: no runSwitch tie draw: {trace:?}"));
        let want = if run_switch.result == 1 { crate::terrain::Terrain::Grassy } else { crate::terrain::Terrain::Psychic };
        assert_eq!(b.terrain, want, "seed {seed}: runSwitch draw {}", run_switch.result);
        seen[(run_switch.result - 1) as usize] = true;
    }
    assert_eq!(seen, [true, true], "both tie outcomes across seeds");
}
