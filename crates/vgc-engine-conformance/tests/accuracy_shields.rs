//! Shields Down (PS a5df8274 data/abilities.ts:4239-4280): a full-info
//! four-turn comparator capture.
//!
//! `shields-down-meteor-core-cycle-v1` (`tools/accuracy/repros/
//! shields-down-jobs.jsonl`, seed `sodium,…0201`, `ps-battle.js
//! --max-turns 4`): Minior-Yellow leads as Minior-Meteor, Brick Break takes
//! it to 88 (still Meteor), a critical Brick Break to 22 (back to Yellow at
//! residual order 29), Tackle + Heal Pulse to 75 (Meteor again), Brick
//! Break to 27 (Yellow). The capture records each turn's species, so the
//! state diff checks the forme as well as HP. Its RNG stream is separate
//! from the `shields-down-*` goldens' (seed [1,2,3,4]).

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle, Replayed};

const CYCLE: &str = "shields-down-meteor-core-cycle-v1";

fn capture() -> AccBattle {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{CYCLE}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let acc: AccBattle = serde_json::from_str(&text).expect("battle json");
    assert!(acc.meta.ok && acc.meta.errors.is_empty(), "PS errors {:?}", acc.meta.errors);
    assert_eq!(acc.format, "gen9doublescustomgame");
    // Four complete turns: the cap ends at the turn-5 marker.
    assert_eq!(acc.turns.len(), 4);
    assert!(acc.turns.iter().all(|t| t.has_state));
    assert_eq!(acc.meta.last_turn, Some(5));
    acc
}

fn assert_state_clean(rep: &Replayed, what: &str) {
    assert!(rep.engine_error.is_none(), "{what}: engine error {:?}", rep.engine_error);
    assert!(
        rep.divergence.is_none(),
        "{what}: {:?}",
        rep.divergence.as_ref().map(|d| (d.turn, &d.slot, &d.field, &d.engine, &d.ps))
    );
    assert_eq!((rep.turns_compared, rep.matched_turns), (4, 4), "{what}");
}

#[test]
fn capture_records_the_meteor_core_cycle() {
    let acc = capture();
    let p2a: Vec<(u16, Option<&str>)> = acc
        .turns
        .iter()
        .map(|t| {
            let m = &t.base.state["p2a"];
            (m.hp, m.species.as_deref())
        })
        .collect();
    assert_eq!(
        p2a,
        [(88, Some("miniormeteor")), (22, Some("minioryellow")), (75, Some("miniormeteor")), (27, Some("minioryellow"))]
    );
    let log = &acc.meta.log;
    assert_eq!(log.matches("|-formechange|p2a: Minior|Minior-Meteor||[from] ability: Shields Down").count(), 2);
    assert_eq!(log.matches("|-formechange|p2a: Minior|Minior-Yellow||[from] ability: Shields Down").count(), 2);
}

/// Seeded replay under the engine's PS PRNG: every state (HP, species)
/// and every draw matches.
#[cfg(feature = "ps-rng")]
#[test]
fn seeded_replay_matches_states_and_draws() {
    let (rep, _) = vgc_engine_conformance::accuracy::replay_ps_rng(&capture());
    assert_state_clean(&rep, "seeded");
    assert!(rep.first_draw_div.is_none(), "{:?}", rep.first_draw_div);
    assert_eq!(rep.ps_errors, 0);
    assert_eq!((rep.unmatched_total, rep.leftover_upto, rep.repaired), (0, 0, 0));
    assert!(rep.miss_keys.is_empty() && rep.leftover_keys.is_empty());
}

/// Keyed replay: every state matches with no missing draw.
#[test]
fn keyed_replay_matches_states() {
    let rep = replay_keyed(&capture());
    assert_state_clean(&rep, "keyed");
    assert_eq!(rep.unmatched_total, 0, "missing draws {:?}", rep.miss_keys);
    assert_eq!(rep.repaired, 0, "repair aliases");
    assert_eq!(rep.leftover_upto, 0, "leftovers {:?}", rep.leftover_keys);
}
