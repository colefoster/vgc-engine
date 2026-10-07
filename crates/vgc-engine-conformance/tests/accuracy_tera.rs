//! Terastallization before moves: seeded / keyed integration captures.
//!
//! PS queues a Terastallize choice as its own order-106 `terastallize`
//! action, before every move (sim/battle-queue.ts:188, :230-235), and
//! runAction ends it with `eachEvent('Update')` (sim/battle.ts:2860-2861).
//! The goldens `tera-before-moves-*` cover HP under OraclePartial;
//! these captures replay the same scenarios under PS's own PRNG (`ps-rng`)
//! and under the keyed oracle.
//!
//! Captures (`tools/accuracy/tera-verification/`, pinned PS a5df8274,
//! `ps-battle.js --max-turns 1`, explicit sodium seeds): the two golden
//! scenarios, and the flinch scenario with a real Chansey / Blissey Speed tie
//! (Blissey 21 Spe IVs: both 70) so the Tera turn draws Update shuffles.
//!
//! `ps-battle.js` pads benches: after Slowbro's KO it sends in Chansey before
//! the end-of-turn snapshot, so the KO capture's final p2b is Chansey.

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_keyed, AccBattle, Replayed};

const FLINCH: &str = "tera-before-moves-flinch";
const RESIST_KO: &str = "tera-before-moves-resist-and-ko";
const SPEED_TIE: &str = "tera-before-moves-flinch-speed-tie";

fn capture(name: &str) -> AccBattle {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let acc: AccBattle = serde_json::from_str(&text).expect("battle json");
    // One complete PS turn: the `--max-turns 1` cap ends at the turn-2
    // marker, after turn 1 was fully played.
    assert!(acc.meta.ok && acc.meta.errors.is_empty(), "{name}: PS errors {:?}", acc.meta.errors);
    assert_eq!(acc.format, "gen9doublescustomgame", "{name}");
    assert_eq!(acc.turns.len(), 1, "{name}");
    assert!(acc.turns[0].has_state, "{name}");
    assert_eq!(acc.meta.last_turn, Some(2), "{name}");
    acc
}

/// The turn-1 log section (between `|turn|1` and `|turn|2`).
fn turn1_log(acc: &AccBattle) -> &str {
    let log = acc.meta.log.as_str();
    let start = log.find("|turn|1\n").expect("turn 1 marker");
    let end = log.find("|turn|2\n").expect("turn 2 marker");
    &log[start..end]
}

fn line_pos(log: &str, line: &str) -> usize {
    log.find(line).unwrap_or_else(|| panic!("missing `{line}` in:\n{log}"))
}

fn assert_state_clean(rep: &Replayed, what: &str) {
    assert!(rep.engine_error.is_none(), "{what}: engine error {:?}", rep.engine_error);
    assert!(
        rep.divergence.is_none(),
        "{what}: {:?}",
        rep.divergence.as_ref().map(|d| (d.turn, &d.slot, &d.field, &d.engine, &d.ps))
    );
    assert_eq!((rep.turns_compared, rep.matched_turns), (1, 1), "{what}");
}

#[test]
fn captures_record_tera_before_every_move() {
    // Flinch: Snorlax Terastallizes, then Weavile's Fake Out is resisted
    // (Steel) and flinches it, so Snorlax never moves.
    for name in [FLINCH, SPEED_TIE] {
        let acc = capture(name);
        let t = &acc.turns[0].base;
        assert_eq!(t.choices.p1, ["move 1 terastallize, move 1"], "{name}");
        assert_eq!(t.choices.p2, ["move 1 1, move 1"], "{name}");
        let log = turn1_log(&acc);
        let tera = line_pos(log, "|-terastallize|p1a: Snorlax|Steel\n");
        let fake_out = line_pos(log, "|move|p2a: Weavile|Fake Out|p1a: Snorlax\n");
        assert!(tera < fake_out, "{name}: Tera before the first move");
        assert!(fake_out < line_pos(log, "|-resisted|p1a: Snorlax\n"), "{name}");
        line_pos(log, "|cant|p1a: Snorlax|flinch\n");
        assert!(!log.contains("|move|p1a: Snorlax"), "{name}: the flinched user never moves");
        assert_eq!(log.matches("|move|").count(), 3, "{name}: Fake Out and two Splashes");
    }

    // Both sides Terastallize before any move (Slowbro, Speed 50, then
    // Snorlax, 45). Ice Shard is resisted by Tera Steel; Flamethrower is
    // super effective on Tera Grass and KOs Slowbro before it moves.
    let acc = capture(RESIST_KO);
    let t = &acc.turns[0].base;
    assert_eq!(t.choices.p1, ["move 1 terastallize, move 1 2"]);
    assert_eq!(t.choices.p2, ["move 1 1, move 1 terastallize"]);
    let log = turn1_log(&acc);
    let slowbro = line_pos(log, "|-terastallize|p2b: Slowbro|Grass\n");
    let snorlax = line_pos(log, "|-terastallize|p1a: Snorlax|Steel\n");
    let ice_shard = line_pos(log, "|move|p2a: Weavile|Ice Shard|p1a: Snorlax\n");
    assert!(slowbro < snorlax && snorlax < ice_shard, "both Teras before the first move");
    assert!(ice_shard < line_pos(log, "|-resisted|p1a: Snorlax\n"));
    let flamethrower = line_pos(log, "|move|p1b: Chandelure|Flamethrower|p2b: Slowbro\n");
    assert!(flamethrower < line_pos(log, "|-supereffective|p2b: Slowbro\n"));
    line_pos(log, "|faint|p2b: Slowbro\n");
    assert!(!log.contains("|move|p2b: Slowbro"), "KO'd before acting");
    // The padded bench replacement, not Slowbro, is the snapshot's p2b.
    assert_eq!(acc.turns[0].replace.p2, ["pass, switch 3"]);
    assert_eq!(acc.turns[0].base.state["p2b"].species.as_deref(), Some("chansey"));
}

#[test]
fn speed_tie_capture_draws_update_shuffles_around_the_tera_action() {
    let acc = capture(SPEED_TIE);
    let shuffles: Vec<_> = acc.turns[0].raw.iter().filter(|d| d.op == "shuffle").collect();
    assert_eq!(shuffles.len(), 17, "turn-1 speed-tie shuffles");
    let at = |site: &str| shuffles.iter().filter(|d| d.site.iter().any(|s| s.starts_with(site))).count();
    // runAction's eachEvent('Update') speed-sorts the tied actives
    // (Battle.eachEvent from Battle.runAction) — the shuffle the Tera
    // action's Update adds — besides the commit sort and the re-sorts.
    let update_in_run_action = shuffles
        .iter()
        .filter(|d| d.site.iter().any(|s| s.starts_with("Battle.eachEvent@")) && d.site.iter().any(|s| s.starts_with("Battle.runAction@")))
        .count();
    assert!(update_in_run_action > 0, "runAction Update shuffles: {:?}", shuffles.iter().map(|d| &d.site).collect::<Vec<_>>());
    assert!(at("BattleQueue.sort@") > 0, "queue sorts");
    // The two non-tie captures draw no turn shuffle at all.
    for name in [FLINCH, RESIST_KO] {
        assert!(capture(name).turns[0].raw.iter().all(|d| d.op != "shuffle"), "{name}");
    }
}

/// Seeded replay under the engine's PS PRNG: every state and every draw
/// matches, including the tie capture's 17 shuffles.
#[cfg(feature = "ps-rng")]
#[test]
fn seeded_replay_matches_states_and_draws() {
    for name in [FLINCH, RESIST_KO, SPEED_TIE] {
        let acc = capture(name);
        let (rep, _) = vgc_engine_conformance::accuracy::replay_ps_rng(&acc);
        assert_state_clean(&rep, name);
        assert!(rep.first_draw_div.is_none(), "{name}: {:?}", rep.first_draw_div);
        assert_eq!(rep.ps_errors, 0, "{name}");
        assert_eq!((rep.unmatched_total, rep.leftover_upto, rep.repaired), (0, 0, 0), "{name}");
        assert!(rep.miss_keys.is_empty() && rep.leftover_keys.is_empty(), "{name}");
    }
}

/// Keyed replay: every state matches with no missing draw. Its known
/// labelling limits are pinned exactly, not tolerated in general:
/// - the KO capture leaves Flamethrower's secondary roll unused (PS draws it
///   for the KO'd Slowbro; the engine skips a fainted target);
/// - the tie capture's queue shuffles are only reached through the repair
///   pass (keyed `tie_key`, accuracy.rs), three of them.
#[test]
fn keyed_replay_matches_states_with_known_label_limits() {
    for (name, repaired, leftovers) in [(FLINCH, 0, 0), (RESIST_KO, 0, 1), (SPEED_TIE, 3, 0)] {
        let acc = capture(name);
        let rep = replay_keyed(&acc);
        assert_state_clean(&rep, name);
        assert_eq!(rep.unmatched_total, 0, "{name}: missing draws {:?}", rep.miss_keys);
        assert_eq!(rep.repaired, repaired, "{name}: repair aliases");
        assert_eq!(rep.leftover_upto, leftovers, "{name}: leftovers {:?}", rep.leftover_keys);
    }
    let rep = replay_keyed(&capture(RESIST_KO));
    assert!(
        rep.leftover_keys.iter().all(|k| k.contains("flamethrower")),
        "only Flamethrower's secondary is left over: {:?}",
        rep.leftover_keys
    );
}
