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
