//! Action-independence factorability classifier (PR-I.1).
//!
//! See `docs/design/pr-i-action-independence.md` for the design.
//!
//! **Purpose.** Given a `(Battle, joint_choice)` for doubles, decide whether
//! the 4 actor decisions can be enumerated independently — i.e. whether
//! actor A's RNG-draw outcome can change actor B's outcome distribution.
//!
//! **Soundness contract.** This classifier is a *conservative pre-check*.
//! When in doubt, return [`Factorability::NoFactor`]. False negatives ("not
//! factorable" when it actually is) cost only a perf opportunity; false
//! positives produce wrong Nash values and are CATASTROPHIC. Every breaker
//! class catalogued in the design doc §2.2 has an explicit check here; new
//! mechanics need a new check (see `AGENTS.md` rule, design doc §4.2).
//!
//! **API.** [`classify_factorability`] is pure and read-only on `Battle`.
//! It does not mutate, does not allocate beyond the returned `Vec` for
//! [`Factorability::PartialFactor`], and is safe to call from anywhere
//! including the hot solver loop.
//!
//! This crate currently only ships the classifier. PR-I.2 (the tensor
//! enumeration that USES it) is gated on the headline factorability % the
//! `measure_factorability` example reports.

use vgc_engine_core::data;
use vgc_engine_core::{Battle, Choice, Format, Pokemon, SideRef};

/// Result of the factorability pre-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Factorability {
    /// All 4 actor decisions enumerate independently.
    /// The outcome frontier is the tensor product of per-actor frontiers.
    FullyFactor,
    /// Some subsets of actors form independent groups but others don't.
    /// Each inner `Vec<u8>` lists actor indices (0..4) that must enumerate
    /// jointly; the groups are independent of each other. Currently this
    /// classifier emits `PartialFactor` only when the two sides cleanly
    /// split (`[[0,1],[2,3]]`), which is the highest-value common case;
    /// finer per-actor splits would require ability/range analysis the
    /// design doc parks as future work. When no clean side-split applies,
    /// the classifier falls back to [`Factorability::NoFactor`].
    PartialFactor { groups: Vec<Vec<u8>> },
    /// No safe factoring — caller must use the full cross-product
    /// enumeration.
    NoFactor,
}

/// Actor indices used throughout this module:
///   0 = P1 slot 0
///   1 = P1 slot 1
///   2 = P2 slot 0
///   3 = P2 slot 1
///
/// `p1_choices[0]` corresponds to actor 0, `p1_choices[1]` to actor 1,
/// `p2_choices[0]` to actor 2, `p2_choices[1]` to actor 3.
///
/// In singles, the classifier short-circuits to [`Factorability::FullyFactor`]
/// (a single actor per side cannot interact with a non-existent ally;
/// inter-side interactions still happen, but that's the *existing*
/// joint-enumeration concern PR-I doesn't change).
pub fn classify_factorability(
    battle: &Battle,
    p1_choices: &[Choice],
    p2_choices: &[Choice],
) -> Factorability {
    // Singles: factoring is trivially fine — there's nothing to factor
    // across (only inter-side which is the existing concern).
    if !matches!(battle.config.format, Format::Doubles) {
        return Factorability::FullyFactor;
    }
    if p1_choices.len() < 2 || p2_choices.len() < 2 {
        // Defensive: caller didn't supply the expected 2 per side. Don't
        // attempt factoring.
        return Factorability::NoFactor;
    }
    let actors: [(SideRef, &Choice); 4] = [
        (SideRef::P1, &p1_choices[0]),
        (SideRef::P1, &p1_choices[1]),
        (SideRef::P2, &p2_choices[0]),
        (SideRef::P2, &p2_choices[1]),
    ];

    // ---- Joint-action breakers (any actor's chosen move triggers it) ----

    // (G) Speed reorder moves — Trick Room, Tailwind, After You, Quash,
    //     Sticky Web (order.rs:10, battle.rs:286, order.rs:117-123,
    //     battle.rs:316-321, side.rs:45-51, side.rs:143-152).
    // (F) Weather / Terrain setters — downstream Weather Ball / sun-Fire /
    //     rain-Water / terrain-multiplier reads (ability.rs:462-495, 512-534,
    //     damage.rs:1346).
    // (K) Cross-actor reading moves — Sucker Punch (battle.rs:2944-2962),
    //     Quick Guard / Wide Guard (side.rs:81-89).
    // (E) Pivot moves — switch the user out, fire switch-in abilities
    //     mid-turn (U-turn, Volt Switch, Parting Shot, Flip Turn, Baton
    //     Pass; ability dispatch ability.rs:374-425 et al.).
    // (A) Spread / multi-target moves — couple co-targeted slots
    //     (damage.rs:125-129, 2096; battle.rs:3200-3204, 6339).
    if any_move_matches(battle, &actors, |mid, md| {
        is_speed_reorderer(mid)
            || is_field_setter(mid)
            || is_cross_reading(mid)
            || is_pivot(mid)
            || is_spread_target(md.target)
    }) {
        return Factorability::NoFactor;
    }

    // (I) Stat-rebound abilities (Defiant / Competitive / Mirror Armor) or
    //     Mirror Herb on any active mon: a stat-drop secondary firing from
    //     the opposing side routes through them (ability.rs:133-161,
    //     battle.rs:8532, item.rs:981-983, 1350-1357). Conservative: any
    //     rebounder present + any damaging move on the field → NoFactor
    //     (we don't fold per-move stat-drop filtering).
    if any_stat_rebound_present(battle) && any_damaging_move_present(battle, &actors) {
        return Factorability::NoFactor;
    }

    // (E) Eject Button / Eject Pack / Red Card holders on any active slot:
    //     damage on them can swap a mon in mid-turn (item.rs:1200-1301).
    if any_eject_item_present(battle) && any_damaging_move_present(battle, &actors) {
        return Factorability::NoFactor;
    }

    // (C) Redirection: any redirector volatile already on the field
    //     (Follow Me / Rage Powder set THIS turn earlier) OR any Lightning
    //     Rod / Storm Drain ability on a slot (battle.rs:6358-6531).
    //     Conservative: any redirector present at all → NoFactor (no
    //     per-type filtering).
    if any_redirector_volatile(battle) || any_redirector_ability(battle) {
        return Factorability::NoFactor;
    }

    // (D) KO-triggered abilities + variance KO. Conservative: any attacker
    //     carrying Beast Boost / Moxie / Chilling Neigh / Grim Neigh /
    //     Soul-Heart / Battle Bond AND any damaging move present →
    //     NoFactor (battle.rs:5556-5572, 34027-34051). Range-check
    //     refinement parked to a follow-up (design doc §3 R-mit-3).
    if any_ko_trigger_ability(battle) && any_damaging_move_present(battle, &actors) {
        return Factorability::NoFactor;
    }

    // (J/H) Ally-presence damage multipliers — Friend Guard / Power Spot /
    //     Battery / Steely Spirit (damage.rs:189, 197, 202-210, 372). If a
    //     holder is on the field AND any damaging move is present, the
    //     holder's KO would change the partner's distribution.
    if any_ally_presence_holder(battle) && any_damaging_move_present(battle, &actors) {
        return Factorability::NoFactor;
    }
    // (H) Air Balloon — pops from any hit on the holder, changing Ground
    //     immunity for the rest of the turn (item.rs:732-755).
    if any_air_balloon(battle) && any_damaging_move_present(battle, &actors) {
        return Factorability::NoFactor;
    }
    // (H) Weakness Policy — post-hit attacker stat change reorders future
    //     damage downstream (item.rs:758-807).
    if any_weakness_policy(battle) && any_damaging_move_present(battle, &actors) {
        return Factorability::NoFactor;
    }

    // (L) Tiebreak — design doc §2.2 L parks this. The existing enumeration
    //     already marginalizes Tiebreak draws (lib.rs:152); PR-I inherits
    //     that limitation, no new risk introduced. No check needed here.

    // (B) Helping Hand (pokemon.rs:2011-2019, damage.rs:1319). Couples the
    //     boosted pair. If only one side uses HH and all OTHER breakers
    //     above are clear, the two sides are still independent of each
    //     other → PartialFactor [[0,1],[2,3]]. If both sides use HH, same
    //     answer (each pair is internally coupled, sides independent).
    let p1_has_hh = is_helping_hand_choice(battle, SideRef::P1, &actors[0].1)
        || is_helping_hand_choice(battle, SideRef::P1, &actors[1].1);
    let p2_has_hh = is_helping_hand_choice(battle, SideRef::P2, &actors[2].1)
        || is_helping_hand_choice(battle, SideRef::P2, &actors[3].1);

    if p1_has_hh || p2_has_hh {
        return Factorability::PartialFactor {
            groups: vec![vec![0, 1], vec![2, 3]],
        };
    }

    Factorability::FullyFactor
}

// ─────────────────────────────────────────────────────────────────────
// Per-class helpers — each cites the engine file:line the breaker reads.
// ─────────────────────────────────────────────────────────────────────

/// Move id for a `Choice::Move` / `Terastallize` / `MegaEvolve`. Returns
/// `None` for `Switch` / `Pass`. `STRUGGLE_MOVE_SLOT` maps to
/// `data::move_id::STRUGGLE` (Struggle is single-target damaging — it
/// participates in damage-related breakers).
fn choice_move_id(battle: &Battle, side: SideRef, c: &Choice) -> Option<u16> {
    let (actor_slot, move_slot) = match *c {
        Choice::Move { actor_slot, move_slot, .. }
        | Choice::Terastallize { actor_slot, move_slot, .. }
        | Choice::MegaEvolve { actor_slot, move_slot, .. } => (actor_slot, move_slot),
        Choice::Switch { .. } | Choice::Pass { .. } => return None,
    };
    // STRUGGLE_MOVE_SLOT = 4 (vgc_engine_core::choice::STRUGGLE_MOVE_SLOT;
    // the `choice` module is private so we inline the sentinel here).
    if move_slot == 4 {
        return Some(data::move_id::STRUGGLE);
    }
    let mon = battle.side(side).active_mon(actor_slot as usize)?;
    let mid = *mon.moves.get(move_slot as usize)?;
    if mid == 0 {
        None
    } else {
        Some(mid)
    }
}

fn move_def(mid: u16) -> Option<&'static data::MoveDef> {
    data::MOVES.get(mid as usize)
}

/// (G) Speed-reorder moves.
fn is_speed_reorderer(mid: u16) -> bool {
    matches!(
        mid,
        data::move_id::TRICKROOM
            | data::move_id::TAILWIND
            | data::move_id::AFTERYOU
            | data::move_id::QUASH
            | data::move_id::STICKYWEB
    )
}

/// (F) Weather / Terrain setters.
fn is_field_setter(mid: u16) -> bool {
    matches!(
        mid,
        data::move_id::SUNNYDAY
            | data::move_id::RAINDANCE
            | data::move_id::SANDSTORM
            | data::move_id::SNOWSCAPE
            | data::move_id::HAIL
            | data::move_id::ELECTRICTERRAIN
            | data::move_id::GRASSYTERRAIN
            | data::move_id::PSYCHICTERRAIN
            | data::move_id::MISTYTERRAIN
    )
}

/// (K) Cross-actor reading moves.
fn is_cross_reading(mid: u16) -> bool {
    matches!(
        mid,
        data::move_id::SUCKERPUNCH
            | data::move_id::QUICKGUARD
            | data::move_id::WIDEGUARD
    )
}

/// (E) Pivot moves.
fn is_pivot(mid: u16) -> bool {
    matches!(
        mid,
        data::move_id::UTURN
            | data::move_id::VOLTSWITCH
            | data::move_id::PARTINGSHOT
            | data::move_id::FLIPTURN
            | data::move_id::BATONPASS
    )
}

/// (A) Spread / multi-target target codes (crates/vgc-engine-data/build.rs:
/// 377-396): 5 = allAdjacent, 6 = allAdjacentFoes, 7 = allies, 8 = allySide,
/// 9 = allyTeam, 11 = foeSide, 12 = all.
fn is_spread_target(target_code: u8) -> bool {
    matches!(target_code, 5 | 6 | 7 | 8 | 9 | 11 | 12)
}

/// (B) Helping Hand.
fn is_helping_hand_choice(battle: &Battle, side: SideRef, c: &Choice) -> bool {
    choice_move_id(battle, side, c) == Some(data::move_id::HELPINGHAND)
}

/// "Damaging" iff base power > 0 and category != Status (2 = Status per
/// build.rs `category_code`).
fn is_damaging(md: &data::MoveDef) -> bool {
    md.category != 2 && md.base_power > 0
}

// ─────────────────────────────────────────────────────────────────────
// Field scans
// ─────────────────────────────────────────────────────────────────────

fn for_each_active<F: FnMut(SideRef, u8, &Pokemon)>(battle: &Battle, mut f: F) {
    for (side, sref) in [(&battle.p1, SideRef::P1), (&battle.p2, SideRef::P2)] {
        for slot in 0..2u8 {
            if let Some(mon) = side.active_mon(slot as usize) {
                if mon.is_alive() {
                    f(sref, slot, mon);
                }
            }
        }
    }
}

fn any_stat_rebound_present(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        let a = mon.effective_ability_id();
        // ability.rs:133-161 (Defiant / Competitive), battle.rs:8532
        // (Mirror Armor).
        if a == data::ability_id::DEFIANT
            || a == data::ability_id::COMPETITIVE
            || a == data::ability_id::MIRRORARMOR
        {
            hit = true;
        }
        // item.rs:981-983, 1350-1357 — Mirror Herb.
        if mon.effective_item_id() == data::item_id::MIRRORHERB {
            hit = true;
        }
    });
    hit
}

fn any_eject_item_present(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        let it = mon.effective_item_id();
        // item.rs:1200-1301.
        if it == data::item_id::EJECTBUTTON
            || it == data::item_id::EJECTPACK
            || it == data::item_id::REDCARD
        {
            hit = true;
        }
    });
    hit
}

fn any_weakness_policy(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        // item.rs:758-807.
        if mon.effective_item_id() == data::item_id::WEAKNESSPOLICY {
            hit = true;
        }
    });
    hit
}

fn any_air_balloon(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        // item.rs:732-755.
        if mon.effective_item_id() == data::item_id::AIRBALLOON {
            hit = true;
        }
    });
    hit
}

fn any_redirector_volatile(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        // pokemon.rs:1977-1992 — Follow Me / Rage Powder volatile.
        if mon.redirecting_this_turn() {
            hit = true;
        }
    });
    hit
}

fn any_redirector_ability(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        // ability.rs:474-531 (Lightning Rod / Storm Drain).
        let a = mon.effective_ability_id();
        if a == data::ability_id::LIGHTNINGROD || a == data::ability_id::STORMDRAIN {
            hit = true;
        }
    });
    hit
}

fn any_ko_trigger_ability(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        // battle.rs:5556-5572, 34027-34051.
        let a = mon.effective_ability_id();
        if a == data::ability_id::BEASTBOOST
            || a == data::ability_id::MOXIE
            || a == data::ability_id::CHILLINGNEIGH
            || a == data::ability_id::GRIMNEIGH
            || a == data::ability_id::SOULHEART
            || a == data::ability_id::BATTLEBOND
        {
            hit = true;
        }
    });
    hit
}

fn any_ally_presence_holder(battle: &Battle) -> bool {
    let mut hit = false;
    for_each_active(battle, |_, _, mon| {
        // damage.rs:189 (Power Spot), 197 (Battery), 202-210 (Steely
        // Spirit), 207-210/372 (Friend Guard).
        let a = mon.effective_ability_id();
        if a == data::ability_id::POWERSPOT
            || a == data::ability_id::BATTERY
            || a == data::ability_id::STEELYSPIRIT
            || a == data::ability_id::FRIENDGUARD
        {
            hit = true;
        }
    });
    hit
}

// ─────────────────────────────────────────────────────────────────────
// Per-actor joint scans
// ─────────────────────────────────────────────────────────────────────

fn any_move_matches<F>(
    battle: &Battle,
    actors: &[(SideRef, &Choice); 4],
    pred: F,
) -> bool
where
    F: Fn(u16, &data::MoveDef) -> bool,
{
    for (side, c) in actors.iter() {
        if let Some(mid) = choice_move_id(battle, *side, c) {
            if let Some(md) = move_def(mid) {
                if pred(mid, md) {
                    return true;
                }
            }
        }
    }
    false
}

fn any_damaging_move_present(battle: &Battle, actors: &[(SideRef, &Choice); 4]) -> bool {
    any_move_matches(battle, actors, |_, md| is_damaging(md))
}

// ─────────────────────────────────────────────────────────────────────
// Tests — safety oracle for every breaker class catalogued in the
// design doc §2.2. Each test exercises a *minimal* fixture where the
// breaker fires and asserts the classifier returns NoFactor (or
// PartialFactor for the Helping Hand side-split case).
//
// We deliberately use a small set of doubles teams parameterized by the
// fields under test (ability, item, move). The clearly-factorable
// baseline (test `baseline_clean_attacks_is_fully_factorable`) confirms
// no false-negative on the simple path.
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use vgc_engine_core::{BattleConfig, Format, Target, TeamBuilder};

    /// Build a doubles battle from two JSON team strings.
    fn doubles_battle(p1_json: &str, p2_json: &str) -> Battle {
        let p1 = TeamBuilder::from_json(p1_json).unwrap();
        let p2 = TeamBuilder::from_json(p2_json).unwrap();
        Battle::new(BattleConfig { format: Format::Doubles, seed: 1 }, p1, p2)
    }

    /// Single-target move targeting a specific foe slot.
    fn mv(actor_slot: u8, move_slot: u8, t_side: SideRef, t_slot: u8) -> Choice {
        Choice::Move {
            actor_slot,
            move_slot,
            target: Some(Target { side: t_side, slot: t_slot }),
        }
    }

    /// A clean 2v2 fixture with vanilla attackers: Tackle in slot 0, no
    /// abilities/items/moves from the breaker list. Used as the
    /// FullyFactor baseline.
    ///
    /// Pikachu has no Lightning Rod, Garchomp has no KO trigger. We pick
    /// species + abilities deliberately neutral.
    fn baseline_p1_json() -> &'static str {
        r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#
    }
    fn baseline_p2_json() -> &'static str {
        r#"[
            {"species":"bidoof","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#
    }

    fn clean_attack_choices() -> ([Choice; 2], [Choice; 2]) {
        // Each side hits the foe's slot 0 with Tackle from both actors.
        let p1 = [
            mv(0, 0, SideRef::P2, 0),
            mv(1, 0, SideRef::P2, 1),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        (p1, p2)
    }

    fn assert_no_factor(b: &Battle, p1: &[Choice], p2: &[Choice], reason: &str) {
        match classify_factorability(b, p1, p2) {
            Factorability::NoFactor => {}
            other => panic!(
                "expected NoFactor for {reason}, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn baseline_clean_attacks_is_fully_factorable() {
        let b = doubles_battle(baseline_p1_json(), baseline_p2_json());
        let (p1, p2) = clean_attack_choices();
        assert_eq!(
            classify_factorability(&b, &p1, &p2),
            Factorability::FullyFactor,
            "clean 4-singleton tackle turn must be FullyFactor",
        );
    }

    // (A) Spread move
    #[test]
    fn spread_move_is_no_factor() {
        // Furret's slot-1 move replaced with Earthquake (target: allAdjacent).
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","earthquake","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 1, SideRef::P2, 0), // Earthquake
            mv(1, 0, SideRef::P2, 1),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "spread move (Earthquake)");
    }

    // (B) Helping Hand → PartialFactor side-split
    #[test]
    fn helping_hand_yields_partial_side_split() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["helpinghand","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P1, 1), // Helping Hand on ally
            mv(1, 0, SideRef::P2, 0),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        match classify_factorability(&b, &p1, &p2) {
            Factorability::PartialFactor { groups } => {
                assert_eq!(groups, vec![vec![0u8, 1], vec![2, 3]]);
            }
            other => panic!("expected PartialFactor side-split, got {:?}", other),
        }
    }

    // (C) Redirector ability (Lightning Rod)
    #[test]
    fn lightning_rod_ability_is_no_factor() {
        let p2_json = r#"[
            {"species":"raichu","level":50,"ability":"lightningrod","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(baseline_p1_json(), p2_json);
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Lightning Rod present");
    }

    // (C) Redirector volatile (Follow Me set earlier — emulate by setting
    // the volatile directly on a mon).
    #[test]
    fn follow_me_volatile_is_no_factor() {
        let mut b = doubles_battle(baseline_p1_json(), baseline_p2_json());
        // Set Follow Me volatile on P2 slot 0.
        let idx = b.p2.active[0] as usize;
        b.p2.team[idx].set_redirecting(true, false);
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Follow Me volatile set");
    }

    // (D) KO-trigger ability (Beast Boost)
    #[test]
    fn beast_boost_is_no_factor() {
        let p1_json = r#"[
            {"species":"kartana","level":50,"ability":"beastboost","item":"choicescarf","nature":"hardy","moves":["leafblade","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Beast Boost on attacker");
    }

    // (E) Pivot move (U-turn)
    #[test]
    fn uturn_pivot_is_no_factor() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["uturn","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P2, 0), // U-turn
            mv(1, 0, SideRef::P2, 1),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "U-turn pivot");
    }

    // (E) Eject Button holder
    #[test]
    fn eject_button_holder_is_no_factor() {
        let p2_json = r#"[
            {"species":"bidoof","level":50,"ability":"unaware","item":"ejectbutton","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(baseline_p1_json(), p2_json);
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Eject Button holder");
    }

    // (F) Weather setter move (Sunny Day)
    #[test]
    fn sunny_day_setter_is_no_factor() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["sunnyday","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P1, 0), // Sunny Day (self-target ok; setter)
            mv(1, 0, SideRef::P2, 0),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "Sunny Day setter");
    }

    // (F) Terrain setter (Electric Terrain)
    #[test]
    fn electric_terrain_setter_is_no_factor() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["electricterrain","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P2, 0),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "Electric Terrain setter");
    }

    // (G) Speed reorder (Trick Room)
    #[test]
    fn trick_room_is_no_factor() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["trickroom","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P2, 0),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "Trick Room");
    }

    // (G) Tailwind
    #[test]
    fn tailwind_is_no_factor() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tailwind","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P2, 0),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "Tailwind");
    }

    // (H) Ally-presence multiplier (Friend Guard)
    #[test]
    fn friend_guard_holder_is_no_factor() {
        let p1_json = r#"[
            {"species":"clefable","level":50,"ability":"friendguard","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Friend Guard ally-presence");
    }

    // (H) Air Balloon holder
    #[test]
    fn air_balloon_is_no_factor() {
        let p2_json = r#"[
            {"species":"bidoof","level":50,"ability":"unaware","item":"airballoon","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(baseline_p1_json(), p2_json);
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Air Balloon holder");
    }

    // (H) Weakness Policy holder
    #[test]
    fn weakness_policy_is_no_factor() {
        let p2_json = r#"[
            {"species":"bidoof","level":50,"ability":"unaware","item":"weaknesspolicy","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(baseline_p1_json(), p2_json);
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Weakness Policy holder");
    }

    // (I) Stat-rebound ability (Defiant)
    #[test]
    fn defiant_rebounder_is_no_factor() {
        let p2_json = r#"[
            {"species":"bisharp","level":50,"ability":"defiant","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(baseline_p1_json(), p2_json);
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Defiant rebounder");
    }

    // (I) Mirror Herb holder
    #[test]
    fn mirror_herb_is_no_factor() {
        let p2_json = r#"[
            {"species":"bidoof","level":50,"ability":"unaware","item":"mirrorherb","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]},
            {"species":"bibarel","level":50,"ability":"unaware","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(baseline_p1_json(), p2_json);
        let (p1, p2) = clean_attack_choices();
        assert_no_factor(&b, &p1, &p2, "Mirror Herb holder");
    }

    // (K) Sucker Punch
    #[test]
    fn sucker_punch_is_no_factor() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["suckerpunch","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P2, 0), // Sucker Punch
            mv(1, 0, SideRef::P2, 1),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "Sucker Punch");
    }

    // (K) Wide Guard
    #[test]
    fn wide_guard_is_no_factor() {
        let p1_json = r#"[
            {"species":"furret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["wideguard","watergun","ember","vinewhip"]},
            {"species":"sentret","level":50,"ability":"runaway","item":"choicescarf","nature":"hardy","moves":["tackle","watergun","ember","vinewhip"]}
        ]"#;
        let b = doubles_battle(p1_json, baseline_p2_json());
        let p1 = [
            mv(0, 0, SideRef::P1, 0), // Wide Guard
            mv(1, 0, SideRef::P2, 1),
        ];
        let p2 = [
            mv(0, 0, SideRef::P1, 0),
            mv(1, 0, SideRef::P1, 1),
        ];
        assert_no_factor(&b, &p1, &p2, "Wide Guard");
    }

    // Singles short-circuit
    #[test]
    fn singles_format_short_circuits_to_fully_factor() {
        let p1 = TeamBuilder::from_json(baseline_p1_json()).unwrap();
        let p2 = TeamBuilder::from_json(baseline_p2_json()).unwrap();
        let b = Battle::new(BattleConfig { format: Format::Singles, seed: 1 }, p1, p2);
        // Use Trick Room — still should report FullyFactor in singles
        // because the doubles classifier doesn't apply.
        let p1c = [mv(0, 0, SideRef::P2, 0)];
        let p2c = [mv(0, 0, SideRef::P1, 0)];
        assert_eq!(
            classify_factorability(&b, &p1c, &p2c),
            Factorability::FullyFactor,
        );
    }
}
