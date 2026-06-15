use vgc_engine_replay::{Event, Replay};

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
fn other_variant_preserves_unknown_lines() {
    let r = Replay::from_json(SAMPLE).unwrap();
    // Sample log contains -supereffective / -resisted / -crit / etc. that
    // we don't model; they must land in Other so nothing is silently dropped.
    assert!(
        r.events.iter().any(|e| matches!(e, Event::Other(s) if s.starts_with("-supereffective"))),
        "expected an Other(-supereffective ...) event"
    );
}
