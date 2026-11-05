//! `Battle` must round-trip through bincode, the format behind the Python
//! `to_bytes` / `from_bytes`. bincode is not self-describing, so any
//! `skip_serializing_if` on a `Battle` field breaks deserialization.

use vgc_engine_core::{Battle, BattleConfig, Format, TeamBuilder, Terrain};

const TEAM: &str = r#"[{"species":"snorlax","level":50,"ability":"thickfat","nature":"hardy","moves":["tackle"]},{"species":"chansey","level":50,"ability":"naturalcure","nature":"hardy","moves":["splash"]},{"species":"cramorant","level":50,"ability":"gulpmissile","nature":"hardy","moves":["surf"]}]"#;

fn roundtrip(b: &Battle) {
    let bytes = bincode::serialize(b).expect("serialize");
    let mut back: Battle = bincode::deserialize(&bytes).expect("deserialize");
    back.rehydrate_caches();
    assert_eq!(serde_json::to_value(b).unwrap(), serde_json::to_value(&back).unwrap());
}

#[test]
fn battle_round_trips_through_bincode() {
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 7 },
        TeamBuilder::from_json(TEAM).unwrap(),
        TeamBuilder::from_json(TEAM).unwrap(),
    );
    roundtrip(&b);
    b.fairy_lock_turns = 2;
    roundtrip(&b);
}

/// A tie is `ended == Some(None)`: JSON must not load it as a running game.
#[test]
fn a_tied_game_stays_over_through_json_and_bincode() {
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 7 },
        TeamBuilder::from_json(TEAM).unwrap(),
        TeamBuilder::from_json(TEAM).unwrap(),
    );
    assert!(serde_json::to_value(&b).unwrap()["ended"].is_null());
    b.set_ended(None);
    let json = serde_json::to_string(&b).unwrap();
    assert_eq!(serde_json::from_str::<Battle>(&json).unwrap().winner(), Some(None));
    let bytes = bincode::serialize(&b).unwrap();
    assert_eq!(bincode::deserialize::<Battle>(&bytes).unwrap().winner(), Some(None));
}

/// Reloading is not a terrain change: Mimicry must not retype its holder over
/// a later Reflect Type.
#[test]
fn a_reload_does_not_rerun_mimicry() {
    let team = TEAM.replace("thickfat", "mimicry");
    let mut b = Battle::new(
        BattleConfig { format: Format::Doubles, seed: 7 },
        TeamBuilder::from_json(&team).unwrap(),
        TeamBuilder::from_json(TEAM).unwrap(),
    );
    b.set_terrain(Terrain::Electric);
    let i = b.p1.active[0] as usize;
    b.p1.team[i].set_type_override(10, None);
    roundtrip(&b);
}
