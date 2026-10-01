//! Accuracy round 9 (docs/accuracy/fix-log.md).

use super::*;
use crate::choice::{Choice, Target};
use crate::team::TeamBuilder;

fn t(side: SideRef, slot: u8) -> Target {
    Target { side, slot }
}

fn mv(slot: u8, move_slot: u8, target: Option<Target>) -> Choice {
    Choice::Move { actor_slot: slot, move_slot, target }
}

#[cfg(feature = "ps-rng")]
fn ps_doubles(p1: &str, p2: &str) -> Battle {
    let mut rng = crate::rng::Rng::ps("sodium,00000000000000000000000000000007").unwrap();
    rng.ps_mut().unwrap().enable_trace();
    Battle::with_rng(
        BattleConfig { format: Format::Doubles, seed: 0 },
        rng,
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

#[cfg(feature = "ps-rng")]
#[test]
fn ps_rng_a_charged_move_resolves_at_its_stored_target_without_a_draw() {
    // sim/side.ts:675-688: a mon locked into a two-turn move gets an action
    // with targetLoc = its stored target, so resolveAction
    // (sim/battle-queue.ts) makes no getRandomTarget draw for it.
    let mut b = ps_doubles(
        r#"[{"species":"archaludon","level":50,"moves":["electroshot"]},{"species":"snorlax","level":50,"moves":["calmmind"]}]"#,
        r#"[{"species":"blissey","level":50,"moves":["calmmind"]},{"species":"chansey","level":50,"moves":["calmmind"]}]"#,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.p1.team[0].charging_turns > 0, "Electro Shot should be charging");
    let _ = b.rng_mut().ps_mut().unwrap().take_trace();
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    let trace = b.rng_mut().ps_mut().unwrap().take_trace().unwrap();
    assert_eq!(trace.iter().filter(|d| d.op == "random_target").count(), 0);
}

fn doubles(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Doubles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

// ---- Battle start: the leads' SwitchIn handlers run in Speed order ----
//
// sim/battle-actions.ts:172-184 runSwitch batches every pending switch-in
// and runs their SwitchIn handlers through a speed-sorted fieldEvent, so the
// slowest Surge / weather setter goes last and its field effect stays.

#[test]
fn battle_start_surges_resolve_fastest_first_so_the_slowest_wins() {
    // P1 Pincurchin (Electric Surge, base 15 Spe) is far slower than P2
    // Rillaboom (Grassy Surge, base 85): Grassy goes up first, then Electric.
    let b = doubles(
        r#"[{"species":"pincurchin","level":50,"ability":"electricsurge","moves":["protect"]},{"species":"snorlax","level":50,"moves":["protect"]}]"#,
        r#"[{"species":"rillaboom","level":50,"ability":"grassysurge","moves":["protect"]},{"species":"chansey","level":50,"moves":["protect"]}]"#,
        1,
    );
    assert_eq!(b.terrain, crate::terrain::Terrain::Electric);
}

// ---- Sucker Punch checks the target it actually hits ----
//
// runMove retargets a move aimed at a fainted foe (sim/battle.ts:2437
// getTarget) and useMoveInner applies redirection before the move's onTry
// (data/moves.ts suckerpunch onTry), so the "is it attacking" check reads the
// retargeted / redirected target.

#[test]
fn sucker_punch_at_a_fainted_foe_checks_the_retargeted_foe() {
    // Weavile's Ice Shard KOs the level-1 Magikarp first; Kingambit's Sucker
    // Punch aimed at that slot retargets to Snorlax, which is attacking.
    let mut b = doubles(
        r#"[{"species":"kingambit","level":50,"moves":["suckerpunch"]},{"species":"weavile","level":50,"moves":["iceshard"]}]"#,
        r#"[{"species":"magikarp","level":1,"moves":["splash"]},{"species":"snorlax","level":50,"moves":["tackle"]}]"#,
        3,
    );
    b.step(
        &[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, Some(t(SideRef::P2, 0)))],
        &[mv(0, 0, None), mv(1, 0, Some(t(SideRef::P1, 1)))],
    );
    assert!(!b.p2.team[0].is_alive(), "Ice Shard should KO the Magikarp");
    let snorlax = &b.p2.team[1];
    assert!(snorlax.current_hp < snorlax.stats.hp, "Sucker Punch should hit the attacking Snorlax");
}

// ---- Toxic: floor(maxhp / 16) * stage ----
//
// data/conditions.ts:159 tox onResidual:
// `this.damage(this.clampIntRange(pokemon.baseMaxhp / 16, 1) * stage)`;
// clampIntRange floors before the multiply.

#[test]
fn toxic_damage_floors_the_sixteenth_before_multiplying() {
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"blissey","level":50,"moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    let hp = b.p1.team[0].stats.hp;
    assert!(hp % 16 >= 8, "fixture needs maxhp % 16 >= 8 (got {hp})");
    b.p1.team[0].status = crate::pokemon::Status::Toxic;
    b.p1.team[0].set_toxic_counter(2);
    b.sync_status_dot_bit(SideRef::P1, 0);
    let before = b.p1.team[0].current_hp;
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(before - b.p1.team[0].current_hp, (hp / 16) * 2);
}

// ---- A gained ability's onStart runs (Skill Swap, Role Play, Trace) ----
//
// Pokemon.setAbility runs the new ability's Start event (sim/pokemon.ts:1943);
// Battle.skillSwap does so for both mons (sim/battle.ts:1311).

#[test]
fn skill_swapped_psychic_surge_sets_the_terrain() {
    // Alakazam Skill Swaps with Indeedee (terrain cleared first): the
    // Psychic Surge it gains sets Psychic Terrain.
    let mut b = doubles(
        r#"[{"species":"alakazam","level":50,"ability":"innerfocus","moves":["skillswap"]},{"species":"snorlax","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"indeedee","level":50,"ability":"psychicsurge","moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.terrain = crate::terrain::Terrain::None;
    b.terrain_turns = 0;
    b.sync_weather_terrain_cache();
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[0].effective_ability_id(), data::ability_id::PSYCHICSURGE);
    assert_eq!(b.terrain, crate::terrain::Terrain::Psychic);
}

#[test]
fn traced_drought_sets_the_sun() {
    // Gardevoir switches in against two Drought leads (weather cleared
    // first) and traces Drought, whose onStart sets the sun.
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]},{"species":"gardevoir","level":50,"ability":"trace","moves":["splash"]}]"#,
        r#"[{"species":"torkoal","level":50,"ability":"drought","moves":["splash"]},{"species":"ninetales","level":50,"ability":"drought","moves":["splash"]}]"#,
        1,
    );
    b.weather = crate::weather::Weather::None;
    b.weather_turns = 0;
    b.sync_weather_terrain_cache();
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[2].effective_ability_id(), data::ability_id::DROUGHT);
    assert_eq!(b.weather, crate::weather::Weather::Sun);
}

// ---- Trace / Mummy / Wandering Spirit set the current ability only ----
//
// setAbility changes `pokemon.ability`, not `baseAbility`; switching out
// restores the base (sim/pokemon.ts clearVolatile: `this.ability =
// this.baseAbility`).

#[test]
fn a_traced_ability_is_lost_on_switch_out() {
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]},{"species":"gardevoir","level":50,"ability":"trace","moves":["splash"]}]"#,
        r#"[{"species":"torkoal","level":50,"ability":"drought","moves":["splash"]},{"species":"ninetales","level":50,"ability":"drought","moves":["splash"]}]"#,
        1,
    );
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[2].effective_ability_id(), data::ability_id::DROUGHT);
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 0 }, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[2].effective_ability_id(), data::ability_id::TRACE);
}

#[test]
fn a_mummy_ability_is_lost_on_switch_out() {
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"ability":"thickfat","moves":["crunch"]},{"species":"chansey","level":50,"moves":["splash"]},{"species":"blissey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"cofagrigus","level":50,"ability":"mummy","moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[0].effective_ability_id(), data::ability_id::MUMMY);
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[0].effective_ability_id(), data::ability_id::THICKFAT);
}

// ---- Hit-time ability checks read the current ability ----
//
// PS runEvent / hasAbility read pokemon.getAbility() (the current, possibly
// swapped ability), e.g. data/abilities.ts soundproof onTryHit.

#[test]
fn a_skill_swapped_soundproof_blocks_sound_moves() {
    let mut b = doubles(
        r#"[{"species":"alakazam","level":50,"ability":"innerfocus","moves":["skillswap","splash"]},{"species":"snorlax","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"exploud","level":50,"ability":"soundproof","moves":["splash"]},{"species":"exploud","level":50,"ability":"scrappy","moves":["hypervoice","splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 1, None)]);
    assert_eq!(b.p1.team[0].effective_ability_id(), data::ability_id::SOUNDPROOF);
    let before = b.p1.team[0].current_hp;
    b.step(&[mv(0, 1, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.p1.team[1].current_hp < b.p1.team[1].stats.hp, "Hyper Voice should hit Snorlax");
    assert_eq!(b.p1.team[0].current_hp, before, "Soundproof (swapped in) blocks Hyper Voice");
}

// ---- Single-target status moves resolve their target like attacks ----
//
// runMove's getTarget retargets a fainted foe (sim/battle.ts:2437) or keeps
// a fainted ally (the move then fails), and useMoveInner's RedirectTarget
// (Follow Me / Rage Powder, data/moves.ts followme onFoeRedirectTarget)
// applies to status moves too. Magic Bounce reflects only the move aimed at
// its holder (data/abilities.ts magicbounce onTryHit).

#[test]
fn follow_me_redirects_a_status_move() {
    let mut b = doubles(
        r#"[{"species":"indeedeef","level":50,"ability":"owntempo","moves":["followme"]},{"species":"snorlax","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"arbok","level":50,"ability":"intimidate","moves":["glare"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, Some(t(SideRef::P1, 1))), mv(1, 0, None)]);
    assert_eq!(b.p1.team[0].status, crate::pokemon::Status::Paralysis, "Glare redirected to Follow Me's user");
    assert_eq!(b.p1.team[1].status, crate::pokemon::Status::None);
}

#[test]
fn magic_bounce_reflects_only_the_move_aimed_at_its_holder() {
    let mut b = doubles(
        r#"[{"species":"arbok","level":50,"ability":"intimidate","moves":["glare"]},{"species":"snorlax","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"hatterene","level":50,"ability":"magicbounce","moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 1))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p2.team[1].status, crate::pokemon::Status::Paralysis, "Glare at Chansey lands");
    assert_eq!(b.p1.team[0].status, crate::pokemon::Status::None, "nothing bounced back");
}

// ---- Explosion / Self-Destruct / Misty Explosion faint the user ----
//
// sim/battle-actions.ts:500: `if (move.selfdestruct === 'always')
// this.battle.faint(pokemon, pokemon, move)` before the hit, so the user
// faints even when every target protects.

#[test]
fn explosion_faints_its_user_even_into_protect() {
    let mut b = doubles(
        r#"[{"species":"snorlax","level":50,"moves":["explosion"]},{"species":"chansey","level":50,"moves":["splash"]},{"species":"blissey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"snorlax","level":50,"moves":["protect"]},{"species":"chansey","level":50,"moves":["protect"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, None), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(!b.p1.team[0].is_alive(), "Explosion's user faints");
    assert_eq!(b.p2.team[0].current_hp, b.p2.team[0].stats.hp);
}

#[test]
fn a_protected_magic_bounce_holder_does_not_reflect() {
    let mut b = doubles(
        r#"[{"species":"arbok","level":50,"ability":"intimidate","moves":["glare"]},{"species":"snorlax","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"hatterene","level":50,"ability":"magicbounce","moves":["protect"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p1.team[0].status, crate::pokemon::Status::None, "Protect blocks before Magic Bounce");
    assert_eq!(b.p2.team[0].status, crate::pokemon::Status::None);
}

// ---- Partial trap and Leech Seed need a live source ----
//
// data/conditions.ts:238 partiallytrapped onResidual ends silently when the
// source is no longer active or has fainted; data/moves.ts leechseed
// onResidual does nothing when the mon in the seeder's slot has fainted.

fn trap_battle() -> Battle {
    doubles(
        r#"[{"species":"toxapex","level":50,"moves":["infestation","splash"]},{"species":"snorlax","level":50,"moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"blissey","level":50,"moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    )
}

#[test]
fn partial_trap_ends_when_its_source_switches_out() {
    let mut b = trap_battle();
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::PartialTrap));
    let before = b.p2.team[0].current_hp;
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 2 }, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p2.team[0].current_hp, before, "no Infestation chip once Toxapex left");
    assert!(!b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::PartialTrap));
}

#[test]
fn leech_seed_does_nothing_while_the_seeders_slot_is_fainted() {
    let mut b = doubles(
        r#"[{"species":"ferrothorn","level":50,"moves":["leechseed"]},{"species":"snorlax","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"blissey","level":50,"moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::LeechSeed), "Leech Seed should land");
    b.p1.team[0].current_hp = 0;
    b.p1.team[0].fainted = true;
    let before = b.p2.team[0].current_hp;
    b.step(&[Choice::Pass { actor_slot: 0 }, mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p2.team[0].current_hp, before, "nothing to leech into");
}

// ---- Knock Off removes the item after the DamagingHit reactions ----
//
// data/mods/champions/scripts.ts spreadMoveHit: runEvent('DamagingHit')
// (Rocky Helmet, Rough Skin ...) runs before the move's AfterHit (Knock Off
// takes the item), and the pinch-berry Update comes after both.

#[test]
fn knock_off_into_rocky_helmet_still_takes_the_helmet_damage() {
    let mut b = doubles(
        r#"[{"species":"incineroar","level":50,"moves":["knockoff"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"snorlax","level":50,"item":"rockyhelmet","moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p2.team[0].item_id, u16::MAX, "Knock Off removes the helmet");
    let inc = &b.p1.team[0];
    assert_eq!(inc.stats.hp - inc.current_hp, inc.stats.hp / 6, "Rocky Helmet hits first");
}

#[test]
fn knock_off_takes_a_sitrus_berry_before_it_can_be_eaten() {
    let mut b = doubles(
        r#"[{"species":"incineroar","level":50,"moves":["knockoff"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"snorlax","level":50,"item":"sitrusberry","moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.p2.team[0].current_hp = b.p2.team[0].stats.hp / 2 + 5;
    let before = b.p2.team[0].current_hp;
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert_eq!(b.p2.team[0].item_id, u16::MAX);
    assert!(b.p2.team[0].current_hp < before, "no Sitrus heal");
}

// ---- Lum Berry cures confusion ----
//
// data/items.ts lumberry: onUpdate eats when `pokemon.status ||
// pokemon.volatiles['confusion']`; onEat cures both.

#[test]
fn lum_berry_cures_confusion() {
    let mut b = doubles(
        r#"[{"species":"gengar","level":50,"moves":["swagger"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        r#"[{"species":"snorlax","level":50,"item":"lumberry","moves":["splash"]},{"species":"chansey","level":50,"moves":["splash"]}]"#,
        1,
    );
    b.set_force_accuracy_hit(Some(true));
    b.step(&[mv(0, 0, Some(t(SideRef::P2, 0))), mv(1, 0, None)], &[mv(0, 0, None), mv(1, 0, None)]);
    assert!(!b.p2.team[0].volatiles.has(crate::pokemon::VolatileKind::Confusion), "Lum Berry cures confusion");
    assert_eq!(b.p2.team[0].item_id, u16::MAX, "and is eaten");
}
