//! A game must play the same whether or not it was saved and reloaded at every
//! decision point (see `vgc_engine_conformance::save_load`). The fixture is a
//! sample of `tools/protocol/ps-fuzz.cjs` games chosen so that each field this
//! test made serialize is live at a decision point in at least one of them
//! (game 40 also has a Cloud Nine holder fainting with weather up);
//! `save_load_gate` runs the same check over any number of input logs.
#![cfg(feature = "ps-rng")]

use vgc_engine_conformance::save_load::{check_game, check_splitmix_game, parse_inputs};

const GAMES: &str = include_str!("fixtures/save_load.inputs.jsonl");

#[test]
fn fuzz_games_survive_a_reload_at_every_decision() {
    let games = parse_inputs(GAMES);
    assert!(games.len() >= 20, "fixture has {} games", games.len());
    let mut decisions = 0;
    for (k, g) in games.iter().enumerate() {
        let r = check_game(g, true);
        decisions += r.decisions;
        assert!(r.divergence.is_none(), "game {k}: {}", r.divergence.unwrap());
        assert!(r.lost_fields.is_empty(), "game {k}: a reload changed {:?}", r.lost_fields);
    }
    assert!(decisions > 500, "{decisions} decisions");
}

#[test]
fn splitmix_games_survive_a_reload_at_every_step() {
    let games = parse_inputs(GAMES);
    // The last game with seed 390 leaves a Fling pending at a step's end.
    let seeds = (1..games.len() as u64).chain([390]);
    for (k, (g, seed)) in games.iter().zip(seeds).enumerate() {
        let r = check_splitmix_game(g, seed);
        assert!(r.divergence.is_none(), "game {k}: {}", r.divergence.unwrap());
        assert!(r.lost_fields.is_empty(), "game {k}: a reload changed {:?}", r.lost_fields);
    }
}
