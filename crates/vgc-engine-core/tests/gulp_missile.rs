//! Gulp Missile (Cramorant), standard gen 9. Written before the mechanic;
//! the forme / response tests are expected to fail until it lands.
//!
//! PS a5df8274 data/abilities.ts:1745-1769:
//! - onSourceTryPrimaryHit: a Surf from a base Cramorant holding Gulp
//!   Missile turns it into Cramorant-Gorging at hp <= maxhp/2, else
//!   Cramorant-Gulping, before the primary hit.
//! - onDamagingHit: a Gulping / Gorging holder hit by a damaging move deals
//!   the attacker baseMaxhp/4, then -1 Def (Gulping) or paralysis
//!   (Gorging), then reverts to Cramorant; skipped if the attacker has no HP
//!   or is inactive, or the holder is semi-invulnerable. It runs even when
//!   the hit KOs the holder.
//! - flags cantsuppress / notransform.
//! data/moves.ts:3760-3767 Dive onTryMove: the charge turn loads the forme
//! the same way (and not for a transformed user).
//! Bulbapedia: <https://bulbapedia.bulbagarden.net/wiki/Gulp_Missile_(Ability)>.

use vgc_engine_core::data::species_id::{CRAMORANT, CRAMORANTGORGING, CRAMORANTGULPING};
use vgc_engine_core::{data, Battle, BattleConfig, Choice, Format, SideRef, Status, Target, TeamBuilder};

/// Cramorant (85 Spe) outspeeds Snorlax (30). Snorlax's 4 HP EVs make its
/// max HP 236, a multiple of 4, so the quarter-HP response is exact.
const CRAMORANT_SET: &str = r#"{"species":"cramorant","level":50,"gender":"M","ability":"gulpmissile","nature":"hardy","evs":{"hp":252,"spd":4},"moves":["surf","dive","splash"]}"#;
const BENCH: &str = r#"{"species":"chansey","level":50,"gender":"F","ability":"naturalcure","nature":"hardy","moves":["splash"]}"#;
const SNORLAX: &str = r#"[{"species":"snorlax","level":50,"gender":"M","ability":"thickfat","nature":"hardy","evs":{"hp":4},"moves":["tackle","splash","protect","surf"]}]"#;

const SURF: u8 = 0;
const DIVE: u8 = 1;
const SPLASH: u8 = 2;
const TACKLE: u8 = 0;
const FOE_SPLASH: u8 = 1;
const PROTECT: u8 = 2;
const FOE_SURF: u8 = 3;

fn battle(p1: &str, p2: &str, seed: u64) -> Battle {
    Battle::new(
        BattleConfig { format: Format::Singles, seed },
        TeamBuilder::from_json(p1).unwrap(),
        TeamBuilder::from_json(p2).unwrap(),
    )
}

fn cramorant(seed: u64) -> Battle {
    battle(&format!("[{CRAMORANT_SET},{BENCH}]"), SNORLAX, seed)
}

fn mv(move_slot: u8) -> Choice {
    let target = Some(Target { side: SideRef::P2, slot: 0 });
    Choice::Move { actor_slot: 0, move_slot, target }
}

fn foe(move_slot: u8) -> Choice {
    let target = Some(Target { side: SideRef::P1, slot: 0 });
    Choice::Move { actor_slot: 0, move_slot, target }
}

fn species(b: &Battle) -> u16 {
    b.p1.team[0].species_id
}

/// Surf from full HP while Snorlax Splashes: Gulping.
fn loaded_gulping(seed: u64) -> Battle {
    let mut b = cramorant(seed);
    b.step(&[mv(SURF)], &[foe(FOE_SPLASH)]);
    b
}

#[test]
fn surf_above_half_loads_gulping() {
    for seed in 1..6 {
        let b = loaded_gulping(seed);
        assert!(b.p2.team[0].current_hp < b.p2.team[0].stats.hp, "seed {seed}: Surf hit");
        assert_eq!(species(&b), CRAMORANTGULPING, "seed {seed}");
    }
}

#[test]
fn surf_at_or_below_half_loads_gorging() {
    // hp <= maxhp / 2. This Cramorant has an odd 177 max HP, so 88 (below
    // 88.5) is Gorging and 89 Gulping; the exact-half case (176 max HP,
    // Super Fang to 88) is the gulp-superfang-half-gorging-response golden.
    for (extra, want) in [(0u16, CRAMORANTGORGING), (1, CRAMORANTGULPING)] {
        let mut b = cramorant(3);
        let max = b.p1.team[0].stats.hp;
        b.p1.team[0].current_hp = max / 2 + extra;
        b.step(&[mv(SURF)], &[foe(FOE_SPLASH)]);
        assert_eq!(species(&b), want, "hp {} of {max}", max / 2 + extra);
    }
}

#[test]
fn surf_into_protect_loads_nothing() {
    // Protect stops the move before the primary hit (TryPrimaryHit never runs).
    let mut b = cramorant(3);
    b.step(&[mv(SURF)], &[foe(PROTECT)]);
    assert_eq!(b.p2.team[0].current_hp, b.p2.team[0].stats.hp, "precondition: Surf blocked");
    assert_eq!(species(&b), CRAMORANT);
}

#[test]
fn dive_charge_loads_the_forme() {
    let mut b = cramorant(3);
    b.step(&[mv(DIVE)], &[foe(FOE_SPLASH)]);
    assert_eq!(b.p2.team[0].current_hp, b.p2.team[0].stats.hp, "precondition: charge turn, no hit yet");
    assert_eq!(species(&b), CRAMORANTGULPING);
}

#[test]
fn gulping_response_quarter_hp_def_drop_and_revert() {
    for seed in 1..6 {
        let mut b = loaded_gulping(seed);
        assert_eq!(species(&b), CRAMORANTGULPING, "seed {seed}: precondition");
        let max = b.p2.team[0].stats.hp;
        assert_eq!(max % 4, 0, "precondition: exact quarter");
        let before = b.p2.team[0].current_hp;
        b.step(&[mv(SPLASH)], &[foe(TACKLE)]);
        assert!(b.p1.team[0].current_hp < b.p1.team[0].stats.hp, "seed {seed}: Tackle hit");
        assert_eq!(before - b.p2.team[0].current_hp, max / 4, "seed {seed}: attacker loses baseMaxhp/4");
        assert_eq!(b.p2.team[0].boosts[1], -1, "seed {seed}: Def -1");
        assert_eq!(b.p2.team[0].status, Status::None, "seed {seed}");
        assert_eq!(species(&b), CRAMORANT, "seed {seed}: reverted");
    }
}

#[test]
fn gorging_response_paralyzes_and_reverts() {
    let mut b = cramorant(3);
    b.p1.team[0].current_hp = b.p1.team[0].stats.hp / 2;
    b.step(&[mv(SURF)], &[foe(FOE_SPLASH)]);
    assert_eq!(species(&b), CRAMORANTGORGING, "precondition");
    let max = b.p2.team[0].stats.hp;
    let before = b.p2.team[0].current_hp;
    b.step(&[mv(SPLASH)], &[foe(TACKLE)]);
    assert_eq!(before - b.p2.team[0].current_hp, max / 4);
    assert_eq!(b.p2.team[0].status, Status::Paralysis);
    assert_eq!(b.p2.team[0].boosts[1], 0, "no Def drop");
    assert_eq!(species(&b), CRAMORANT);
}

#[test]
fn a_hit_that_kos_the_holder_still_triggers_the_response() {
    let mut b = loaded_gulping(3);
    b.p1.team[0].current_hp = 1;
    let max = b.p2.team[0].stats.hp;
    let before = b.p2.team[0].current_hp;
    b.step(&[mv(SPLASH)], &[foe(TACKLE)]);
    assert!(b.p1.team[0].fainted, "precondition: Tackle KOs Cramorant");
    assert_eq!(before - b.p2.team[0].current_hp, max / 4);
    assert_eq!(b.p2.team[0].boosts[1], -1);
}

#[test]
fn dive_release_does_not_load() {
    // Dive's onTryMove loads only on the initial charge: removeVolatile
    // returns first on the release turn (data/moves.ts:3760-3762). With the
    // forme reset between the turns, the release hits and loads nothing.
    let mut b = cramorant(3);
    b.step(&[mv(DIVE)], &[foe(FOE_SPLASH)]);
    assert_eq!(species(&b), CRAMORANTGULPING, "precondition: charge loaded");
    b.p1.team[0].change_forme(CRAMORANT, true);
    b.step(&[mv(DIVE)], &[foe(FOE_SPLASH)]);
    assert!(b.p2.team[0].current_hp < b.p2.team[0].stats.hp, "precondition: the release hit");
    assert_eq!(species(&b), CRAMORANT);
}

#[test]
fn a_non_contact_hit_triggers_the_response() {
    // onDamagingHit has no contact check: Snorlax's Surf (special) into a
    // loaded Gulping Cramorant still takes the quarter and the Def drop.
    let mut b = loaded_gulping(3);
    let max = b.p2.team[0].stats.hp;
    let before = b.p2.team[0].current_hp;
    b.step(&[mv(SPLASH)], &[foe(FOE_SURF)]);
    assert!(b.p1.team[0].current_hp < b.p1.team[0].stats.hp, "precondition: Surf hit");
    assert_eq!(before - b.p2.team[0].current_hp, max / 4);
    assert_eq!(b.p2.team[0].boosts[1], -1);
    assert_eq!(species(&b), CRAMORANT);
}

#[test]
fn magic_guard_blocks_only_the_quarter_damage() {
    // The quarter is an ordinary this.damage, which Magic Guard stops
    // (indirect damage); the Def drop and the revert still happen.
    // Bulbapedia Gulp Missile, Effect.
    let clefable = r#"[{"species":"clefable","level":50,"gender":"F","ability":"magicguard","nature":"hardy","evs":{"hp":4},"moves":["tackle","splash"]}]"#;
    let mut b = battle(&format!("[{CRAMORANT_SET},{BENCH}]"), clefable, 3);
    b.step(&[mv(SURF)], &[foe(1)]);
    assert_eq!(species(&b), CRAMORANTGULPING, "precondition");
    let before = b.p2.team[0].current_hp;
    b.step(&[mv(SPLASH)], &[foe(0)]);
    assert!(b.p1.team[0].current_hp < b.p1.team[0].stats.hp, "precondition: Tackle hit");
    assert_eq!(b.p2.team[0].current_hp, before, "no quarter damage through Magic Guard");
    assert_eq!(b.p2.team[0].boosts[1], -1, "Def drop still lands");
    assert_eq!(species(&b), CRAMORANT, "still reverts");
}

#[test]
fn surf_into_water_absorb_loads_nothing() {
    // Water Absorb's TryHit stops Surf before the primary hit.
    let vaporeon = r#"[{"species":"vaporeon","level":50,"gender":"F","ability":"waterabsorb","nature":"hardy","moves":["splash"]}]"#;
    let mut b = battle(&format!("[{CRAMORANT_SET}]"), vaporeon, 3);
    b.step(&[mv(SURF)], &[foe(0)]);
    assert_eq!(b.p2.team[0].current_hp, b.p2.team[0].stats.hp, "precondition: absorbed");
    assert_eq!(species(&b), CRAMORANT);
}

#[test]
fn a_semi_invulnerable_holder_does_not_respond() {
    // Dive's charge loads Gulping; Snorlax's Surf reaches the diver
    // (doubled) but the holder is semi-invulnerable: no response, still
    // loaded.
    let mut b = cramorant(3);
    b.step(&[mv(DIVE)], &[foe(FOE_SURF)]);
    assert!(b.p1.team[0].current_hp < b.p1.team[0].stats.hp, "precondition: Surf hit the diver");
    assert_eq!(b.p2.team[0].current_hp, b.p2.team[0].stats.hp, "no response damage");
    assert_eq!(b.p2.team[0].boosts[1], 0);
    assert_eq!(species(&b), CRAMORANTGULPING, "still loaded");
}

#[test]
fn a_transformed_copy_does_not_load() {
    // Imposter Ditto copies Cramorant; its Surf loads nothing (notransform).
    let ditto = r#"[{"species":"ditto","level":50,"gender":"N","ability":"imposter","nature":"hardy","moves":["transform"]}]"#;
    let mut b = battle(&format!("[{CRAMORANT_SET}]"), ditto, 3);
    assert_eq!(b.p2.team[0].species_id, CRAMORANT, "precondition: transformed");
    b.step(&[mv(SPLASH)], &[foe(SURF)]);
    assert!(b.p1.team[0].current_hp < b.p1.team[0].stats.hp, "precondition: the copy's Surf hit");
    assert_eq!(b.p2.team[0].species_id, CRAMORANT, "copy stays in its copied forme");
    assert_eq!(species(&b), CRAMORANT);
}

#[test]
fn another_species_with_the_ability_does_not_load() {
    // The Surf check needs a base Cramorant (species.name === 'Cramorant').
    let pelipper = r#"{"species":"pelipper","level":50,"gender":"M","ability":"drizzle","nature":"hardy","moves":["surf","splash"]}"#;
    let mut b = battle(&format!("[{pelipper}]"), SNORLAX, 3);
    b.p1.team[0].ability_id = data::ability_id::GULPMISSILE;
    let before = b.p1.team[0].species_id;
    b.step(&[mv(SURF)], &[foe(FOE_SPLASH)]);
    assert_eq!(b.p1.team[0].species_id, before);
}

#[test]
fn switching_out_restores_base_cramorant() {
    // A non-permanent formeChange is undone when the mon leaves the field.
    let mut b = loaded_gulping(3);
    assert_eq!(species(&b), CRAMORANTGULPING, "precondition");
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[foe(FOE_SPLASH)]);
    assert_eq!(b.p1.team[0].species_id, CRAMORANT, "benched Cramorant is base");
}

// The Gorging retaliation is `source.trySetStatus('par', target, move)`
// (data/abilities.ts:1753): the holder is the status source, so the
// attacker's Synchronize (data/abilities.ts synchronize onAfterSetStatus,
// `source.trySetStatus(status, target)`) paralyzes that exact holder.
const UMBREON: &str = r#"{"species":"umbreon","level":50,"gender":"M","ability":"synchronize","nature":"hardy","evs":{"hp":4},"moves":["tackle","splash"]}"#;

#[test]
fn gorging_paralysis_synchronizes_back_onto_the_holder() {
    let mut b = battle(&format!("[{CRAMORANT_SET},{BENCH}]"), &format!("[{UMBREON}]"), 3);
    b.p1.team[0].current_hp = b.p1.team[0].stats.hp / 2;
    b.step(&[mv(SURF)], &[foe(1)]);
    assert_eq!(species(&b), CRAMORANTGORGING, "precondition");
    b.step(&[mv(SPLASH)], &[foe(0)]);
    assert_eq!(b.p2.team[0].status, Status::Paralysis, "precondition: the retaliation paralyzed Umbreon");
    assert_eq!(b.p1.team[0].status, Status::Paralysis, "Synchronize reflects onto the holder");
}

#[test]
fn synchronize_reflects_onto_the_exact_holder_slot_in_doubles() {
    // Holder at p1b, its partner Chansey at p1a; Umbreon (p2a) Tackles p1b.
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 3 },
        TeamBuilder::from_json(&format!("[{BENCH},{CRAMORANT_SET}]")).unwrap(),
        TeamBuilder::from_json(&format!("[{UMBREON},{BENCH}]")).unwrap(),
    );
    let at = |actor_slot: u8, move_slot: u8, target: Option<Target>| Choice::Move { actor_slot, move_slot, target };
    let p1b = Some(Target { side: SideRef::P1, slot: 1 });
    b.p1.team[1].current_hp = b.p1.team[1].stats.hp / 2;
    b.step(&[at(0, 0, None), at(1, SURF, None)], &[at(0, 1, None), at(1, 0, None)]);
    assert_eq!(b.p1.team[1].species_id, CRAMORANTGORGING, "precondition: p1b loaded");
    b.step(&[at(0, 0, None), at(1, SPLASH, None)], &[at(0, 0, p1b), at(1, 0, None)]);
    assert_eq!(b.p2.team[0].status, Status::Paralysis, "precondition: retaliation");
    assert_eq!(b.p1.team[1].status, Status::Paralysis, "reflected onto p1b, the holder");
    assert_eq!(b.p1.team[0].status, Status::None, "not onto its partner");
}

#[test]
fn white_herb_clears_every_drop_after_the_whole_move() {
    // Control (expected green): Snorlax's Close Combat into Gulping takes the
    // Gulp -1 Def and its own -1 Def / -1 SpD; White Herb's onAnyAfterMove
    // (data/items.ts:7699-7701) clears all of them once, after the move.
    let snorlax = r#"[{"species":"snorlax","level":50,"gender":"M","ability":"thickfat","item":"whiteherb","nature":"hardy","evs":{"hp":4},"moves":["closecombat","splash"]}]"#;
    let mut b = battle(&format!("[{CRAMORANT_SET},{BENCH}]"), snorlax, 3);
    b.step(&[mv(SURF)], &[foe(1)]);
    assert_eq!(species(&b), CRAMORANTGULPING, "precondition");
    let max = b.p2.team[0].stats.hp;
    let before = b.p2.team[0].current_hp;
    b.step(&[mv(SPLASH)], &[foe(0)]);
    assert_eq!(before - b.p2.team[0].current_hp, max / 4, "precondition: the response fired");
    assert_eq!(b.p2.team[0].boosts[1], 0, "Def restored");
    assert_eq!(b.p2.team[0].boosts[3], 0, "SpD restored");
    assert_eq!(b.p2.team[0].item_id, u16::MAX, "White Herb consumed once");
    assert_eq!(species(&b), CRAMORANT);
}

// PS builds `baseSpecies` from the set's species (sim/pokemon.ts:322) and
// clearVolatile returns to it (:1559), so a loaded Cramorant whose set was
// Cramorant switches back in as Cramorant, while one whose set species was
// Cramorant-Gulping stays Gulping. Two such states are otherwise equal now
// but differ later, so their canonical hashes must differ.
#[test]
fn loaded_states_with_different_set_formes_hash_apart() {
    let gulping_set = CRAMORANT_SET.replace(r#""species":"cramorant""#, r#""species":"cramorantgulping""#);
    let from_base = loaded_gulping(3);
    let mut from_gulping = battle(&format!("[{gulping_set},{BENCH}]"), SNORLAX, 3);
    from_gulping.step(&[mv(SURF)], &[foe(FOE_SPLASH)]);
    assert_eq!(species(&from_base), CRAMORANTGULPING, "precondition");
    assert_eq!(species(&from_gulping), CRAMORANTGULPING, "precondition");
    assert_eq!(from_base.p1.team[0].current_hp, from_gulping.p1.team[0].current_hp, "precondition");
    assert_eq!(from_base.p2.team[0].current_hp, from_gulping.p2.team[0].current_hp, "precondition");
    // Legacy snapshot without a recorded set species: keeps its forme.
    let mut legacy = from_base.clone();
    legacy.p1.team[0].set_species_id = u16::MAX;
    let hashes = [from_base.canonical_hash(), from_gulping.canonical_hash(), legacy.canonical_hash()];
    // The futures differ: out and back in, on clones of the hashed states.
    let futures: Vec<u16> = [&from_base, &from_gulping, &legacy]
        .into_iter()
        .map(|b| {
            let mut b = b.clone();
            b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[foe(FOE_SPLASH)]);
            b.step(&[Choice::Switch { actor_slot: 0, team_index: 0 }], &[foe(FOE_SPLASH)]);
            species(&b)
        })
        .collect();
    assert_eq!(futures, [CRAMORANT, CRAMORANTGULPING, CRAMORANTGULPING], "Cramorant set reverts; Gulping set and unknown origin keep");
    assert_ne!(hashes[0], hashes[1], "Cramorant vs Gulping set: different futures, different hashes");
    assert_ne!(hashes[0], hashes[2], "Cramorant set vs unknown origin");
}

#[test]
fn a_direct_gulping_set_returns_to_gulping_after_its_response() {
    // A set built as Cramorant-Gulping fires its response and reverts to
    // Cramorant (formeChange('cramorant')), but clearVolatile's
    // setSpecies(baseSpecies) returns it to its set species on switch-out
    // regardless of the current forme (sim/pokemon.ts:322, :1559).
    let gulping_set = CRAMORANT_SET.replace(r#""species":"cramorant""#, r#""species":"cramorantgulping""#);
    let mut b = battle(&format!("[{gulping_set},{BENCH}]"), SNORLAX, 3);
    b.step(&[mv(SPLASH)], &[foe(TACKLE)]);
    assert_eq!(b.p2.team[0].boosts[1], -1, "precondition: the response fired");
    assert_eq!(species(&b), CRAMORANT, "precondition: reverted to base");
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 1 }], &[foe(FOE_SPLASH)]);
    b.step(&[Choice::Switch { actor_slot: 0, team_index: 0 }], &[foe(FOE_SPLASH)]);
    assert_eq!(species(&b), CRAMORANTGULPING, "back in as its set forme");
}

#[test]
fn benched_origins_hash_by_content_not_vector_order() {
    // Two benched Cramorant-Gulping with different origins (Cramorant set,
    // Gulping set): swapping their team positions is the same state, while
    // giving both the same origin is not.
    let team = format!("[{BENCH},{CRAMORANT_SET},{CRAMORANT_SET}]");
    let bench = |origins: [u16; 2]| {
        let mut b = battle(&team, SNORLAX, 3);
        for (i, o) in [(1usize, origins[0]), (2, origins[1])] {
            b.p1.team[i].change_forme(CRAMORANTGULPING, true);
            b.p1.team[i].set_species_id = o;
        }
        b
    };
    let a = bench([CRAMORANT, CRAMORANTGULPING]);
    let swapped = bench([CRAMORANTGULPING, CRAMORANT]);
    let same = bench([CRAMORANT, CRAMORANT]);
    assert_eq!(a.canonical_hash(), swapped.canonical_hash(), "bench order is incidental");
    assert_ne!(a.canonical_hash(), same.canonical_hash(), "origins are content");
}

#[cfg(feature = "chance")]
#[test]
fn chance_frontier_agrees_on_the_response() {
    let b = loaded_gulping(3);
    let max = b.p2.team[0].stats.hp;
    let before = b.p2.team[0].current_hp;
    let f = b.step_chance(&[mv(SPLASH)], &[foe(TACKLE)], 7);
    assert_eq!(f.unmatched_total, 0);
    let total: f64 = f.outcomes.iter().map(|o| o.prob).sum();
    assert!((total - 1.0).abs() < 1e-9, "probabilities sum to {total}");
    for o in &f.outcomes {
        assert_eq!(before - o.battle.p2.team[0].current_hp, max / 4);
        assert_eq!(o.battle.p2.team[0].boosts[1], -1);
        assert_eq!(o.battle.p1.team[0].species_id, CRAMORANT);
    }
}
