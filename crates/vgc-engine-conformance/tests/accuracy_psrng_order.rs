//! `ps-rng` draw-order goldens: PS captures whose seeded replay must match
//! every PS draw (kind and span) and every end-of-turn state.
//!
//! Each file in `tools/accuracy/repros/battles/psrng-*.json` is captured by
//! `tools/accuracy/ps-battle.js tools/accuracy/repros/psrng-order-jobs.jsonl
//! <out> --max-turns N` (PS `a5df8274`, Champions mod). The draw lists carry
//! PS's own call stacks, so a missing, extra or reordered draw fails here.

#![cfg(feature = "ps-rng")]

use std::path::PathBuf;

use vgc_engine_conformance::accuracy::{replay_ps_rng, AccBattle};

fn assert_seeded_matches_ps(name: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/accuracy/repros/battles")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let acc: AccBattle = serde_json::from_str(&text).expect("battle json");
    assert!(acc.meta.ok && acc.meta.errors.is_empty(), "{name}: PS errors {:?}", acc.meta.errors);
    let (rep, _) = replay_ps_rng(&acc);
    assert!(rep.engine_error.is_none(), "{name}: engine error {:?}", rep.engine_error);
    assert!(rep.turns_compared > 0, "{name}: no turns compared");
    assert!(rep.divergence.is_none(), "{name}: state diverges: {:?}", rep.divergence);
    assert!(rep.first_draw_div.is_none(), "{name}: draw diverges: {:?}", rep.first_draw_div);
}

/// Roar only sets `forceSwitchFlag`; runAction drags the target in after
/// the move, so the hit loop's two Updates sort the pre-drag actives, and
/// the drag's runSwitch speed-sorts every active (sim/battle.ts:2821-2829;
/// sim/battle-actions.ts:158-178). Equal-Speed leads make every sort draw.
#[test]
fn roar_drags_after_the_hit_loop_and_sorts_in_run_switch() {
    assert_seeded_matches_ps("psrng-roar-drags-after-the-hit-loop-1");
    assert_seeded_matches_ps("psrng-roar-drags-after-the-hit-loop-3");
}

/// A Protect whose stall roll fails deletes PS's `stall` volatile
/// (data/moves.ts stall `onStallMove`), so that turn's Residual handler
/// sort (sim/battle.ts fieldEvent) has no entry for it. Turn 2 repeats
/// Protect; the failed one used to add a tied handler and shift the
/// shuffles.
#[test]
fn a_failed_protect_leaves_no_stall_residual_handler() {
    assert_seeded_matches_ps("psrng-failed-protect-drops-stall-2");
    assert_seeded_matches_ps("psrng-failed-protect-drops-stall-5");
}

/// Both Whimsicott (equal Speed, Tailwind on both sides) faint to Flare
/// Blitz. The replacement request's commitChoices sorts the two
/// instaswitch actions on the fainted mons' bare Speed (resolveAction ->
/// getActionSpeed; an inactive mon has no ModifySpe handlers), so they tie
/// and PS shuffles them, and that order sets the switch-in order.
#[test]
fn tied_replacements_shuffle_in_the_switch_commit_sort() {
    assert_seeded_matches_ps("psrng-tied-replacements-sort-on-bare-speed-2");
    assert_seeded_matches_ps("psrng-tied-replacements-sort-on-bare-speed-3");
}

/// Meowstic-F Mega Evolves into Trace facing Palafin (Zero to Hero,
/// `notrace`) and Pelipper. Trace's `possibleTargets` drops the `notrace`
/// foe (data/abilities.ts:5138-5143), so PS draws `random(1)` and copies
/// Drizzle; the engine used to sample both foes.
#[test]
fn trace_skips_a_notrace_foe() {
    assert_seeded_matches_ps("psrng-trace-skips-notrace-foe-1");
    assert_seeded_matches_ps("psrng-trace-skips-notrace-foe-2");
}

/// Alakazam Speed Swaps with Snorlax, Snorlax switches out and back in.
/// clearVolatile's setSpecies recomputes Snorlax's storedStats
/// (sim/pokemon.ts clearVolatile -> setSpecies), so it returns with its
/// own Speed and ties Alakazam (which kept Snorlax's); the engine kept the
/// swapped Speed and moved Snorlax first without a tie.
#[test]
fn speed_swap_ends_when_the_target_switches_out() {
    assert_seeded_matches_ps("psrng-speed-swap-ends-on-switch-out-1");
    assert_seeded_matches_ps("psrng-speed-swap-ends-on-switch-out-2");
}
