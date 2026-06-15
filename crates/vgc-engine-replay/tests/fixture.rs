use vgc_engine_replay::{observe_events, CanonicalDefault, Event, Replay, TeamRecon};

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
fn other_variant_preserves_unknown_lines() {
    let r = Replay::from_json(SAMPLE).unwrap();
    // Sample log contains -supereffective / -resisted / -crit / etc. that
    // we don't model; they must land in Other so nothing is silently dropped.
    assert!(
        r.events.iter().any(|e| matches!(e, Event::Other(s) if s.starts_with("-supereffective"))),
        "expected an Other(-supereffective ...) event"
    );
}
