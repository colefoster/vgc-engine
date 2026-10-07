//! Hunger Switch / Aura Wheel: full-info comparator captures.
//!
//! PS a5df8274 data/abilities.ts:1896-1907 (residual order 29 toggle),
//! data/moves.ts:775-810 (Aura Wheel: Dark as Morpeko-Hangry, else
//! Electric), sim/pokemon.ts:1559 (clearVolatile -> setSpecies(baseSpecies)
//! on switch-out). Captures from `tools/accuracy/hunger-switch-verification/
//! jobs.jsonl` (`ps-battle.js --max-turns 4`, fixed sodium seeds):
//!
//! - `hunger-switch-switch-reset`: Aura Wheel into Palossand (Electric:
//!   immune), Hangry at residual; switched out (back to Full Belly), back in
//!   (Hangry at residual again); a Dark Aura Wheel then hits for 110 with
//!   the self +1 Spe, and residual turns it Full Belly.
//! - `hunger-switch-direct-hangry-set`: a Morpeko-Hangry set toggles to
//!   Full Belly, then switches back in as Hangry (its set forme) and toggles
//!   to Full Belly again.

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle, Replayed};

const SWITCH_RESET: &str = "hunger-switch-switch-reset";
const DIRECT_HANGRY: &str = "hunger-switch-direct-hangry-set";

fn capture(name: &str, turns: usize) -> AccBattle {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let acc: AccBattle = serde_json::from_str(&text).expect("battle json");
    assert!(acc.meta.ok && acc.meta.errors.is_empty(), "{name}: PS errors {:?}", acc.meta.errors);
    assert_eq!(acc.format, "gen9doublescustomgame", "{name}");
    assert_eq!(acc.turns.len(), turns, "{name}");
    assert!(acc.turns.iter().all(|t| t.has_state), "{name}");
    assert_eq!(acc.meta.last_turn, Some(turns as u32 + 1), "{name}: every turn complete");
    acc
}

/// `(species, hp, Spe stage)` of p1a after each turn.
fn p1a(acc: &AccBattle) -> Vec<(&str, u16, i8)> {
    acc.turns
        .iter()
        .map(|t| {
            let m = &t.base.state["p1a"];
            (m.species.as_deref().unwrap_or(""), m.hp, m.boosts.as_ref().map_or(0, |b| b.spe))
        })
        .collect()
}

fn assert_state_clean(rep: &Replayed, turns: u32, what: &str) {
    assert!(rep.engine_error.is_none(), "{what}: engine error {:?}", rep.engine_error);
    assert!(
        rep.divergence.is_none(),
        "{what}: {:?}",
        rep.divergence.as_ref().map(|d| (d.turn, &d.slot, &d.field, &d.engine, &d.ps))
    );
    assert_eq!((rep.turns_compared, rep.matched_turns), (turns, turns), "{what}");
}

#[test]
fn captures_record_the_toggle_reset_and_typing() {
    let acc = capture(SWITCH_RESET, 4);
    assert_eq!(
        p1a(&acc),
        [("morpekohangry", 134, 0), ("chansey", 325, 0), ("morpekohangry", 134, 0), ("morpeko", 134, 1)]
    );
    let p2a: Vec<u16> = acc.turns.iter().map(|t| t.base.state["p2a"].hp).collect();
    assert_eq!(p2a, [161, 161, 161, 51], "Electric: immune; Dark: 110 damage");
    let log = &acc.meta.log;
    assert!(log.contains("|move|p1a: Morpeko|Aura Wheel|p2a: Palossand\n|-immune|p2a: Palossand\n"));
    assert!(log.contains("|switch|p1a: Morpeko|Morpeko, L50, M|134/134\n"), "switched back in as Full Belly");

    let acc = capture(DIRECT_HANGRY, 3);
    assert_eq!(p1a(&acc), [("morpeko", 134, 0), ("chansey", 325, 0), ("morpeko", 134, 0)]);
    assert!(acc.meta.log.contains("|switch|p1a: Morpeko|Morpeko-Hangry, L50, M|134/134\n|move"), "back in as its set forme");
}

/// Seeded replay under the engine's PS PRNG: every state and every draw
/// (including the switch turns' Update speed-tie shuffles) matches.
#[cfg(feature = "ps-rng")]
#[test]
fn seeded_replay_matches_states_and_draws() {
    for (name, turns) in [(SWITCH_RESET, 4), (DIRECT_HANGRY, 3)] {
        let (rep, _) = vgc_engine_conformance::accuracy::replay_ps_rng(&capture(name, turns));
        assert_state_clean(&rep, turns as u32, name);
        assert!(rep.first_draw_div.is_none(), "{name}: {:?}", rep.first_draw_div);
        assert_eq!(rep.ps_errors, 0, "{name}");
        assert_eq!((rep.unmatched_total, rep.leftover_upto, rep.repaired), (0, 0, 0), "{name}");
        assert!(rep.miss_keys.is_empty() && rep.leftover_keys.is_empty(), "{name}");
    }
}

/// Keyed replay: every state (species, HP, stages) matches with no missing
/// draw. The switch turns' speed-tie shuffles (Chansey and Audino, both 70)
/// reach the keyed table only through the repair pass, as the Tera tie
/// capture's do (accuracy_tera.rs); their counts are reported, not pinned.
#[test]
fn keyed_replay_matches_states() {
    for (name, turns) in [(SWITCH_RESET, 4), (DIRECT_HANGRY, 3)] {
        let rep = replay_keyed(&capture(name, turns));
        assert_state_clean(&rep, turns as u32, name);
        assert_eq!(rep.unmatched_total, 0, "{name}: missing draws {:?}", rep.miss_keys);
    }
}
