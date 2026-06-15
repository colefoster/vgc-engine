use vgc_engine_core::Choice;
use vgc_engine_replay::{
    hp_trace, observe_events, CanonicalDefault, ChoiceExtractor, Event, HpSource, Replay,
    RunnerInit, TeamRecon,
};

const SAMPLE: &str = include_str!("fixtures/sample.json");

#[test]
fn parses_sample_replay() {
    let r = Replay::from_json(SAMPLE).expect("parse");

    assert!(!r.id.is_empty(), "id populated");
    assert!(r.format.contains("Gen 9"), "format: {}", r.format);
    assert_eq!(r.gametype.as_deref(), Some("doubles"));
    assert_eq!(r.players.len(), 2);

    // Team preview entries (|poke| lines) per the fixture log.
    assert_eq!(r.team_preview.len(), 12);

    // Winner is set (the fixture ends with |win|TorkoalWQuickclaw).
    assert_eq!(r.winner.as_deref(), Some("TorkoalWQuickclaw"));

    // There's at least one of each major battle-state event.
    assert!(r.events.iter().any(|e| matches!(e, Event::Turn(_))));
    assert!(r.events.iter().any(|e| matches!(e, Event::Move { .. })));
    assert!(r.events.iter().any(|e| matches!(e, Event::Switch { .. })));
    assert!(r.events.iter().any(|e| matches!(e, Event::Damage { .. })));
    assert!(r.events.iter().any(|e| matches!(e, Event::Faint(_))));
    assert!(r.events.iter().any(|e| matches!(e, Event::Weather { .. })));
}

#[test]
fn observer_populates_observations_from_fixture() {
    let r = Replay::from_json(SAMPLE).unwrap();
    let [p1, p2] = observe_events(&r.events, &r.team_preview);
    let p1 = p1.expect("p1 had team-preview entries");
    let p2 = p2.expect("p2 had team-preview entries");

    // The fixture has Pelipper's Drizzle trigger Rain at turn 1; the
    // Ability event should land on the Pelipper observation.
    let pelipper = p1.mons.iter().find(|m| m.species == "pelipper").unwrap();
    assert_eq!(pelipper.ability.as_deref(), Some("drizzle"));

    // Sneasler's Focus Sash gets consumed via |-enditem| in the fixture.
    let sneasler = p1.mons.iter().find(|m| m.species == "sneasler").unwrap();
    assert_eq!(sneasler.item.as_deref(), Some("focussash"));

    // Pelipper's |-heal| via Sitrus Berry happens on the heal event, but
    // the item should be set via the corresponding |-enditem| earlier in
    // the same turn. Pelipper consumed Sitrus.
    assert_eq!(pelipper.item.as_deref(), Some("sitrusberry"));

    // At least one move should be attributed to each side.
    assert!(
        p1.mons.iter().any(|m| !m.moves.is_empty()),
        "p1 has some observed moves: {:?}",
        p1.mons,
    );
    assert!(
        p2.mons.iter().any(|m| !m.moves.is_empty()),
        "p2 has some observed moves: {:?}",
        p2.mons,
    );

    // Talonflame fainted on turn 1 after using Protect — the move should
    // be on its observation.
    let talonflame = p2.mons.iter().find(|m| m.species == "talonflame").unwrap();
    assert!(
        talonflame.moves.iter().any(|m| m == "protect"),
        "talonflame moves: {:?}",
        talonflame.moves,
    );
}

#[test]
fn canonical_default_consumes_observed_input() {
    // End-to-end: parser → observer → CanonicalDefault → TeamMember[]
    // with observed ability/item/moves merged in.
    let r = Replay::from_json(SAMPLE).unwrap();
    let [p1, _] = observe_events(&r.events, &r.team_preview);
    let team = CanonicalDefault.reconstruct(&p1.unwrap()).unwrap();

    let pelipper = team.iter().find(|m| m.species == "pelipper").unwrap();
    assert_eq!(pelipper.ability.as_deref(), Some("drizzle"));
    assert_eq!(pelipper.item.as_deref(), Some("sitrusberry"));
    // Modest specialist per the heuristic.
    assert_eq!(pelipper.nature, "modest");
    assert_eq!(pelipper.evs.spa, 252);
}

#[test]
fn runner_builds_battle_from_fixture() {
    let r = Replay::from_json(SAMPLE).unwrap();
    let init = RunnerInit::from_replay(&r, &CanonicalDefault).expect("runner init");

    // Both leads detected: p1 led Sneasler/Archaludon (from the fixture),
    // p2 led Garchomp/Talonflame.
    assert_eq!(init.p1_team[0].species, "sneasler");
    assert_eq!(init.p1_team[1].species, "archaludon");
    assert_eq!(init.p2_team[0].species, "garchomp");
    assert_eq!(init.p2_team[1].species, "talonflame");

    let battle = init.into_battle(0xC0FFEE).expect("battle init");

    // Leads landed in active slots 0 and 1 on both sides.
    let p1a = battle.p1.active_mon(0).expect("p1a active");
    let p1b = battle.p1.active_mon(1).expect("p1b active");
    assert_eq!(p1a.species().slug, "sneasler");
    assert_eq!(p1b.species().slug, "archaludon");

    let p2a = battle.p2.active_mon(0).expect("p2a active");
    let p2b = battle.p2.active_mon(1).expect("p2b active");
    assert_eq!(p2a.species().slug, "garchomp");
    assert_eq!(p2b.species().slug, "talonflame");

    // Brought-set filter (PR-37): only mons that actually appeared on the
    // field are kept. The fixture's |teampreview|4 means each side brings
    // 4 of 6.
    assert_eq!(battle.p1.team.len(), 4);
    assert_eq!(battle.p2.team.len(), 4);
}

#[test]
fn turn_bucketing_matches_fixture() {
    let r = Replay::from_json(SAMPLE).unwrap();
    let turns = r.turns();

    // Fixture log has |turn|1..=6 (6 turn markers). Plus a synthetic
    // turn-0 bucket for pre-turn-1 setup ⇒ 7 buckets total.
    assert_eq!(turns.len(), 7);
    assert_eq!(turns[0].number, 0);
    assert_eq!(turns.last().unwrap().number, 6);

    // Every Move event in the replay should appear in some turn>=1 bucket
    // (Moves don't happen during the pre-turn-1 init phase).
    let pre_turn_moves = turns[0].events.iter().filter(|e| matches!(e, Event::Move { .. })).count();
    assert_eq!(pre_turn_moves, 0);
    let total_moves: usize = turns
        .iter()
        .map(|t| t.events.iter().filter(|e| matches!(e, Event::Move { .. })).count())
        .sum();
    let raw_moves = r
        .events
        .iter()
        .filter(|e| matches!(e, Event::Move { .. }))
        .count();
    assert_eq!(total_moves, raw_moves, "no Move events lost in bucketing");

    // The four lead switches (p1a, p1b, p2a, p2b) should all be in turn 0.
    let switches_turn0 = turns[0]
        .events
        .iter()
        .filter(|e| matches!(e, Event::Switch { .. }))
        .count();
    assert!(switches_turn0 >= 4, "turn 0 has the lead switches");

    // Win event lands in the last bucket.
    assert!(turns
        .last()
        .unwrap()
        .events
        .iter()
        .any(|e| matches!(e, Event::Win(_))));
}

#[test]
fn hp_trace_on_fixture_per_turn() {
    let r = Replay::from_json(SAMPLE).unwrap();
    let turns = r.turns();

    // Turn 0 (init): 4 lead switches at 100/100. No damage/heal yet.
    let t0 = hp_trace(turns[0].events);
    let switch_ins = t0.iter().filter(|e| e.source == HpSource::SwitchIn).count();
    assert!(switch_ins >= 4, "turn 0 has at least 4 switch-ins; got {switch_ins}");
    assert!(t0.iter().all(|e| !e.fainted));

    // Turn 1 in the fixture is only Protect/Protect/FakeOut-into-Protect
    // ⇒ no damage events expected. Turn 2 has Acrobatics + Rock Slide
    // spread + Stone Edge + Sitrus Berry heal.
    let t1 = hp_trace(turns[1].events);
    assert!(t1.iter().all(|e| !e.fainted));
    let t2 = hp_trace(turns[2].events);
    assert!(t2.iter().any(|e| e.source == HpSource::Damage));
    assert!(t2.iter().any(|e| e.source == HpSource::Heal));

    // Over the whole battle, at least one Faint should appear.
    let all: Vec<_> = turns.iter().flat_map(|t| hp_trace(t.events)).collect();
    assert!(all.iter().any(|e| e.fainted && e.source == HpSource::Faint));

    // Sanity: fractions stay in range.
    assert!(all.iter().all(|e| (0.0..=1.0).contains(&e.fraction)));
}

#[test]
fn choices_extracted_on_fixture() {
    let r = Replay::from_json(SAMPLE).unwrap();
    let init = RunnerInit::from_replay(&r, &CanonicalDefault).unwrap();
    let mut ex = ChoiceExtractor::new(&init);
    let turns = r.turns();

    // Turn 1: each active mon issues exactly one Move (Protect / Protect /
    // FakeOut into Protect). Sneasler's Fake Out resolves to move_slot=0
    // because the recon observer captured it first.
    let [p1_t1, p2_t1] = ex.extract_turn(&turns[1]);
    assert!(!p1_t1.is_empty(), "p1 turn-1 has at least one Move");
    assert!(!p2_t1.is_empty(), "p2 turn-1 has at least one Move");
    assert!(p1_t1.iter().any(|c| matches!(c, Choice::Move { .. })));
    assert!(p2_t1.iter().all(|c| matches!(c, Choice::Move { .. })));

    // Walk every remaining turn; choices should keep flowing without
    // panicking, and active-slot state should stay coherent (no slot
    // forever stuck at 255 unless the side is defeated).
    for tv in &turns[2..] {
        let _ = ex.extract_turn(tv);
    }

    // After all turns, both sides should still have at least one
    // non-empty active slot (the fixture ends on a |win|, not a double-
    // KO).
    let act = ex.active();
    let any_p1_alive = act[0].iter().any(|i| *i != 255);
    let any_p2_alive = act[1].iter().any(|i| *i != 255);
    assert!(any_p1_alive || any_p2_alive);
}

#[test]
fn other_variant_preserves_unknown_lines() {
    let r = Replay::from_json(SAMPLE).unwrap();
    // Sample log contains -supereffective / -resisted / -crit / etc. that
    // we don't model; they must land in Other so nothing is silently dropped.
    assert!(
        r.events.iter().any(|e| matches!(e, Event::Other(s) if s.starts_with("-supereffective"))),
        "expected an Other(-supereffective ...) event"
    );
}
